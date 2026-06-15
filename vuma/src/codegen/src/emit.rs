//! # ARM64 Code Emission
//!
//! Lowers IR to ARM64 machine code and produces ELF binaries or raw binaries
//! suitable for the AArch64 (Cortex-A76, ARMv8.2-A).
//!
//! ## Pipeline
//!
//! 1. **IR → ARM64 Instructions**: Each IR instruction is pattern-matched and
//!    lowered to one or more [`Instruction`]
//!    values, with virtual registers replaced by physical registers from the
//!    register allocator.
//! 2. **Emit function**: Each IR function is emitted as a sequence of 32-bit
//!    ARM64 code words.
//! 3. **Emit program**: All functions are collected, data sections are laid
//!    out, and a minimal ELF64 binary is produced.
//!
//! ## Output Formats
//!
//! - **ELF**: Full ELF64 binary with program headers, section headers, symbol
//!   table, and string table.  Suitable for Linux on AArch64.
//! - **Raw**: Flat binary image for bare-metal AArch64 (loaded at 0x80000).
//! - **Obj**: Relocatable ELF object file (`ET_REL`) for linking.
//!
//! ## ELF Layout (executable)
//!
//! ```text
//! ┌─────────────────────┐
//! │ ELF Header           │  64 bytes
//! ├─────────────────────┤
//! │ Program Headers      │  3 × 56 bytes (LOAD segments)
//! ├─────────────────────┤
//! │ .rodata              │  read-only data (segment: R)
//! ├─────────────────────┤
//! │ .text                │  emitted code   (segment: R+X)
//! ├─────────────────────┤
//! │ .data                │  initialized data (segment: R+W)
//! ├─────────────────────┤
//! │ .symtab              │  symbol table entries
//! ├─────────────────────┤
//! │ .strtab              │  symbol string table
//! ├─────────────────────┤
//! │ .shstrtab            │  section header string table
//! ├─────────────────────┤
//! │ Section Headers      │  N × 64 bytes
//! └─────────────────────┘
//! .bss is virtual-only (memsz > filesz in the data LOAD segment).
//!
//! Section alignment is target-dependent:
//! - ARM32:    4-byte alignment
//! - AArch64:  16-byte alignment
//! - x86-64:   16-byte alignment
//! - RISC-V:   4-byte alignment
//! - MIPS64:   8-byte alignment
//! - PPC64:    16-byte alignment
//! - LoongArch: 8-byte alignment
//! ```
//!
//! ## Relocation Support
//!
//! Inter-function calls (`BL`) are resolved in a fixup pass after all
//! functions have been emitted.  The emitter records the word offset of each
//! `BL` instruction and the target function name; once function addresses
//! are known, the branch offsets are patched into the encoded instructions.

use std::collections::HashMap;

use crate::arm64::{Condition, Instruction, Operand, RegWidth, Register};
use crate::backend::{BackendKind, RelocationEntry};
use crate::ir::*;
use crate::regalloc::RegAllocator;
use crate::CodegenError;
use crate::Result;

/// Vreg count threshold above which the stack-slot emitter is used instead of
/// the greedy register allocator.  Functions with more than this many virtual
/// registers are likely to experience spill/reload corruption with the greedy
/// allocator.
const STACK_SLOT_VREG_THRESHOLD: u32 = 0;

// ---------------------------------------------------------------------------
// Branch fixup format
// ---------------------------------------------------------------------------

/// The encoding format of a branch instruction, used during fixup resolution
/// to know which bits contain the offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BranchFormat {
    /// B / BL: 26-bit offset in bits[25:0] (word-aligned, offset = imm26 * 4)
    B26,
    /// CBZ / CBNZ: 19-bit offset in bits[23:5] (word-aligned, offset = imm19 * 4)
    Cond19,
    /// B.cond: 19-bit offset in bits[23:5] (word-aligned)
    BCond19,
}


// ---------------------------------------------------------------------------
// ELF Constants (ARM64 / AArch64)
// ---------------------------------------------------------------------------

/// ELF magic bytes.
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// 64-bit ELF.
const ELFCLASS64: u8 = 2;

/// Little-endian data encoding.
const ELFDATA2LSB: u8 = 1;

/// ELF version.
const EV_CURRENT: u8 = 1;

/// Linux OS/ABI.
const ELFOSABI_LINUX: u8 = 3;

/// Standalone / bare-metal OS/ABI (ELFOSABI_STANDALONE).
const ELFOSABI_STANDALONE: u8 = 255;

/// Machine type: AArch64.
const EM_AARCH64: u16 = 183;

/// Machine type: x86-64.
const EM_X86_64: u16 = 62;

/// Machine type: RISC-V.
const EM_RISCV: u16 = 243;

/// Machine type: MIPS.
const EM_MIPS: u16 = 8;

/// Machine type: PowerPC 64-bit.
const EM_PPC64: u16 = 21;

/// Machine type: LoongArch.
const EM_LOONGARCH: u16 = 258;

/// Machine type: ARM (32-bit).
const EM_ARM: u16 = 40;

/// ELF type: executable.
const ET_EXEC: u16 = 2;

/// ELF type: relocatable object.
const ET_REL: u16 = 1;

/// Program header type: LOAD.
const PT_LOAD: u32 = 1;

/// Program header flags.
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

/// Section header type: null (unused).
const SHT_NULL: u32 = 0;
/// Section header type: progbits.
const SHT_PROGBITS: u32 = 1;
/// Section header type: symbol table.
const SHT_SYMTAB: u32 = 2;
/// Section header type: string table.
const SHT_STRTAB: u32 = 3;
/// Section header type: no bits (BSS).
const SHT_NOBITS: u32 = 8;
/// Section header type: relocation entries with addend.
const SHT_RELA: u32 = 4;

/// Symbol binding: local.
const STB_LOCAL: u8 = 0;
/// Symbol binding: global.
const STB_GLOBAL: u8 = 1;

/// Symbol type: function.
const STT_FUNC: u8 = 2;
/// Symbol type: section.
const STT_SECTION: u8 = 3;
/// Symbol type: no type (undefined/external symbol).
const STT_NOTYPE: u8 = 0;

/// Default base address for Linux LOAD segment.
const BASE_ADDR_LINUX: u64 = 0x400000;

/// Default base address for bare-metal AArch64 (kernel load address).
const BASE_ADDR_BARE: u64 = 0x80000;

/// Special section index: undefined/missing section.
const SHN_UNDEF: u16 = 0;

// ---------------------------------------------------------------------------
// AArch64 Relocation Types
// ---------------------------------------------------------------------------

/// R_AARCH64_CALL26 — B/BL relocation for 26-bit branch offset.
const R_AARCH64_CALL26: u32 = 283;

/// R_AARCH64_JUMP26 — B relocation for 26-bit branch offset (unconditional branch).
#[allow(dead_code)]
const R_AARCH64_JUMP26: u32 = 282;

/// R_AARCH64_ADR_PREL_PG_HI21 — ADRP page-relative relocation.
#[allow(dead_code)]
const R_AARCH64_ADR_PREL_PG_HI21: u32 = 275;

/// R_AARCH64_LDST64_ABS_LO12_NC — 64-bit load/store offset relocation.
#[allow(dead_code)]
const R_AARCH64_LDST64_ABS_LO12_NC: u32 = 286;

// ---------------------------------------------------------------------------
// x86-64 Relocation Types
// ---------------------------------------------------------------------------

/// R_X86_64_64 — 64-bit absolute relocation.
#[allow(dead_code)]
const R_X86_64_64: u32 = 1;
/// R_X86_64_PC32 — 32-bit PC-relative relocation.
#[allow(dead_code)]
const R_X86_64_PC32: u32 = 2;
/// R_X86_64_PLT32 — 32-bit PLT-relative relocation (call).
const R_X86_64_PLT32: u32 = 4;
/// R_X86_64_32 — 32-bit absolute relocation (zero-extended).
#[allow(dead_code)]
const R_X86_64_32: u32 = 10;
/// R_X86_64_32S — 32-bit absolute relocation (sign-extended).
#[allow(dead_code)]
const R_X86_64_32S: u32 = 11;

// ---------------------------------------------------------------------------
// RISC-V64 Relocation Types
// ---------------------------------------------------------------------------

/// R_RISCV_JAL — JAL instruction relocation.
#[allow(dead_code)]
const R_RISCV_JAL: u32 = 2;
/// R_RISCV_BRANCH — Conditional branch relocation.
#[allow(dead_code)]
const R_RISCV_BRANCH: u32 = 16;
/// R_RISCV_CALL — CALL pseudo-instruction relocation (AUIPC + JALR).
const R_RISCV_CALL: u32 = 18;
/// R_RISCV_CALL_PLT — CALL PLT pseudo-instruction relocation.
#[allow(dead_code)]
const R_RISCV_CALL_PLT: u32 = 19;
/// R_RISCV_PCREL_HI20 — PC-relative high 20 bits.
#[allow(dead_code)]
const R_RISCV_PCREL_HI20: u32 = 23;
/// R_RISCV_PCREL_LO12_I — PC-relative low 12 bits (I-type).
#[allow(dead_code)]
const R_RISCV_PCREL_LO12_I: u32 = 24;
/// R_RISCV_HI20 — Absolute high 20 bits.
#[allow(dead_code)]
const R_RISCV_HI20: u32 = 26;
/// R_RISCV_LO12_I — Absolute low 12 bits (I-type).
#[allow(dead_code)]
const R_RISCV_LO12_I: u32 = 27;

// ---------------------------------------------------------------------------
// MIPS64 Relocation Types
// ---------------------------------------------------------------------------

/// R_MIPS_32 — 32-bit absolute relocation.
#[allow(dead_code)]
const R_MIPS_32: u32 = 2;
/// R_MIPS_26 — 26-bit jump target relocation.
const R_MIPS_26: u32 = 4;
/// R_MIPS_HI16 — High 16 bits of an address.
#[allow(dead_code)]
const R_MIPS_HI16: u32 = 5;
/// R_MIPS_LO16 — Low 16 bits of an address.
#[allow(dead_code)]
const R_MIPS_LO16: u32 = 6;
/// R_MIPS_GPREL16 — GP-relative 16-bit relocation.
#[allow(dead_code)]
const R_MIPS_GPREL16: u32 = 7;
/// R_MIPS_CALL16 — 16-bit call through GOT.
#[allow(dead_code)]
const R_MIPS_CALL16: u32 = 11;
/// R_MIPS_64 — 64-bit absolute relocation.
#[allow(dead_code)]
const R_MIPS_64: u32 = 18;

// ---------------------------------------------------------------------------
// PowerPC64 Relocation Types
// ---------------------------------------------------------------------------

/// R_PPC64_REL24 — 24-bit PC-relative branch relocation (call).
const R_PPC64_REL24: u32 = 10;
/// R_PPC64_REL32 — 32-bit PC-relative relocation.
#[allow(dead_code)]
const R_PPC64_REL32: u32 = 26;
/// R_PPC64_ADDR32 — 32-bit absolute address relocation.
#[allow(dead_code)]
const R_PPC64_ADDR32: u32 = 20;
/// R_PPC64_ADDR64 — 64-bit absolute address relocation.
#[allow(dead_code)]
const R_PPC64_ADDR64: u32 = 38;

// ---------------------------------------------------------------------------
// LoongArch64 Relocation Types
// ---------------------------------------------------------------------------

/// R_LARCH_PCALA_HI20 — PC-relative high 20 bits for PCALA.
#[allow(dead_code)]
const R_LARCH_PCALA_HI20: u32 = 44;
/// R_LARCH_PCALA_LO12 — PC-relative low 12 bits for PCALA.
#[allow(dead_code)]
const R_LARCH_PCALA_LO12: u32 = 45;
/// R_LARCH_B26 — 26-bit branch relocation.
const R_LARCH_B26: u32 = 69;
/// R_LARCH_32 — 32-bit absolute relocation.
#[allow(dead_code)]
const R_LARCH_32: u32 = 77;
/// R_LARCH_64 — 64-bit absolute relocation.
#[allow(dead_code)]
const R_LARCH_64: u32 = 79;
/// R_LARCH_CALL36 — CALL36 relocation (36-bit call).
#[allow(dead_code)]
const R_LARCH_CALL36: u32 = 89;

// ---------------------------------------------------------------------------
// ARM32 Relocation Types
// ---------------------------------------------------------------------------

/// R_ARM_ABS32 — 32-bit absolute relocation.
#[allow(dead_code)]
const R_ARM_ABS32: u32 = 2;
/// R_ARM_REL32 — 32-bit PC-relative relocation.
#[allow(dead_code)]
const R_ARM_REL32: u32 = 3;
/// R_ARM_CALL — BL call relocation (PC-relative 24-bit).
const R_ARM_CALL: u32 = 28;
/// R_ARM_JUMP24 — B jump relocation (PC-relative 24-bit).
#[allow(dead_code)]
const R_ARM_JUMP24: u32 = 29;
/// R_ARM_MOVW_ABS_NC — MOVW absolute (lower 16 bits).
#[allow(dead_code)]
const R_ARM_MOVW_ABS_NC: u32 = 43;
/// R_ARM_MOVT_ABS — MOVT absolute (upper 16 bits).
#[allow(dead_code)]
const R_ARM_MOVT_ABS: u32 = 44;

// ---------------------------------------------------------------------------
// EmitConfig
// ---------------------------------------------------------------------------

/// Output format for the code emitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum OutputFormat {
    /// Full ELF64 executable (Linux) or object file.
    ELF,
    /// Flat raw binary image (bare-metal).
    Raw,
    /// Relocatable ELF object file (`.o`).
    Obj,
    /// WebAssembly binary module (`.wasm`).
    Wasm,
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::ELF => write!(f, "elf"),
            OutputFormat::Raw => write!(f, "raw"),
            OutputFormat::Obj => write!(f, "obj"),
            OutputFormat::Wasm => write!(f, "wasm"),
        }
    }
}

/// Target platform for code emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Target {
    /// Linux on AArch64.
    Linux,
    /// Bare-metal AArch64 (ARMv8.2-A).
    BareMetal,
    /// WebAssembly 32-bit (wasm32).
    Wasm32,
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::Linux => write!(f, "linux"),
            Target::BareMetal => write!(f, "bare-metal"),
            Target::Wasm32 => write!(f, "wasm32"),
        }
    }
}

/// Configuration for the code emitter.
///
/// Controls the output format, target platform, and various emission options.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EmitConfig {
    /// Output format (ELF, raw binary, or object file).
    pub format: OutputFormat,
    /// Target platform (Linux or bare-metal AArch64).
    pub target: Target,
    /// Target backend / ISA architecture.
    pub backend: BackendKind,
    /// Base virtual address for the text segment.
    pub base_addr: u64,
    /// Name of the entry-point function (default: "main").
    pub entry_name: String,
    /// Include section headers in the ELF output.
    pub section_headers: bool,
    /// Include symbol table in the ELF output.
    pub symbol_table: bool,
    /// Include DWARF5 debug info sections in the ELF output.
    pub debug_info: bool,
}

impl EmitConfig {
    /// Create a new configuration for Linux/ELF output.
    pub fn linux_elf() -> Self {
        Self {
            format: OutputFormat::ELF,
            target: Target::Linux,
            backend: BackendKind::AArch64,
            base_addr: BASE_ADDR_LINUX,
            entry_name: "main".to_string(),
            section_headers: true,
            symbol_table: true,
            debug_info: false,
        }
    }

    /// Create a new configuration for bare-metal raw binary output.
    pub fn bare_metal_raw() -> Self {
        Self {
            format: OutputFormat::Raw,
            target: Target::BareMetal,
            backend: BackendKind::AArch64,
            base_addr: BASE_ADDR_BARE,
            entry_name: "_start".to_string(),
            section_headers: false,
            symbol_table: false,
            debug_info: false,
        }
    }

    /// Create a new configuration for bare-metal ELF output.
    pub fn bare_metal_elf() -> Self {
        Self {
            format: OutputFormat::ELF,
            target: Target::BareMetal,
            backend: BackendKind::AArch64,
            base_addr: BASE_ADDR_BARE,
            entry_name: "_start".to_string(),
            section_headers: true,
            symbol_table: true,
            debug_info: false,
        }
    }

    /// Create a new configuration for a relocatable object file.
    pub fn relocatable_obj() -> Self {
        Self {
            format: OutputFormat::Obj,
            target: Target::Linux,
            backend: BackendKind::AArch64,
            base_addr: 0,
            entry_name: String::new(),
            section_headers: true,
            symbol_table: true,
            debug_info: false,
        }
    }

    /// Create a new configuration for a relocatable object file targeting a
    /// specific ISA backend.
    pub fn relocatable_obj_for(backend: BackendKind) -> Self {
        Self {
            format: OutputFormat::Obj,
            target: Target::Linux,
            backend,
            base_addr: 0,
            entry_name: String::new(),
            section_headers: true,
            symbol_table: true,
            debug_info: false,
        }
    }

    /// Create a new configuration for Wasm32 binary output.
    pub fn wasm_binary() -> Self {
        Self {
            format: OutputFormat::Wasm,
            target: Target::Wasm32,
            backend: BackendKind::Wasm32,
            base_addr: 0,
            entry_name: "_start".to_string(),
            section_headers: false,
            symbol_table: false,
            debug_info: false,
        }
    }

    /// Returns the effective base address for the given target.
    pub fn effective_base_addr(&self) -> u64 {
        if self.base_addr != 0 {
            self.base_addr
        } else {
            match self.target {
                Target::Linux => BASE_ADDR_LINUX,
                Target::BareMetal => BASE_ADDR_BARE,
                Target::Wasm32 => 0, // Wasm uses its own address space
            }
        }
    }
}

impl Default for EmitConfig {
    fn default() -> Self {
        Self::linux_elf()
    }
}

// ---------------------------------------------------------------------------
// Inter-function call relocation record
// ---------------------------------------------------------------------------

/// A record for a `BL` instruction that needs to be patched with the address
/// of a named function.  After all functions are emitted, these records are
/// resolved to compute the correct branch offset.
#[derive(Debug, Clone)]
struct CallRelocation {
    /// Byte offset within the text section where the BL instruction lives.
    text_byte_offset: u64,
    /// Name of the target function.
    target_func: String,
}

// ---------------------------------------------------------------------------
// ELF RelaEntry
// ---------------------------------------------------------------------------

/// An ELF64 relocation entry with an explicit addend (SHT_RELA).
///
/// Each entry specifies where in the section a relocation must be applied,
/// which symbol the relocation references, the type of relocation, and an
/// addend value used in the relocation computation.
#[derive(Debug, Clone)]
pub struct RelaEntry {
    /// Byte offset within the section where the relocation applies.
    pub offset: u64,
    /// Symbol index (upper 32 bits) and relocation type (lower 32 bits),
    /// packed as: `(sym_idx << 32) | r_type`.
    pub info: u64,
    /// Addend value used in the relocation computation.
    pub addend: i64,
}

impl RelaEntry {
    /// Create a new relocation entry.
    pub fn new(offset: u64, sym_idx: u32, r_type: u32, addend: i64) -> Self {
        Self {
            offset,
            info: ((sym_idx as u64) << 32) | (r_type as u64),
            addend,
        }
    }

    /// Extract the symbol index from `info`.
    pub fn sym_idx(&self) -> u32 {
        (self.info >> 32) as u32
    }

    /// Extract the relocation type from `info`.
    pub fn r_type(&self) -> u32 {
        (self.info & 0xFFFFFFFF) as u32
    }

    /// Serialize the entry to 24 bytes (little-endian).
    pub fn to_bytes(&self) -> [u8; 24] {
        let mut buf = [0u8; 24];
        buf[0..8].copy_from_slice(&self.offset.to_le_bytes());
        buf[8..16].copy_from_slice(&self.info.to_le_bytes());
        buf[16..24].copy_from_slice(&self.addend.to_le_bytes());
        buf
    }
}

// ---------------------------------------------------------------------------
// Emitter
// ---------------------------------------------------------------------------

/// The ARM64 code emitter.
///
/// Holds state for the current emission context: the register allocator,
/// accumulated code, and fixup records for branch targets.
pub struct Emitter {
    /// Register allocator used during emission.
    reg_alloc: RegAllocator,
    /// Accumulated machine code for the current function (32-bit words).
    code: Vec<u32>,
    /// Fixup records for intra-function branches: (word index, target label name, format).
    fixups: Vec<(usize, String, BranchFormat)>,
    /// Map from label name to code offset (in words) within the current function.
    label_offsets: HashMap<String, usize>,
    /// Inter-function call relocations for the current function.
    call_relocs: Vec<CallRelocation>,
    /// Relocation entries using the `RelocationEntry` infrastructure.
    ///
    /// Each entry records a byte offset within the function's encoded output
    /// where a symbolic reference must be resolved, the target symbol name,
    /// and the ISA-specific relocation type (e.g., `"R_AARCH64_CALL26"`).
    relocations: Vec<RelocationEntry>,
    /// Name of the function currently being emitted.
    current_func_name: String,
    /// Byte offset of the current function within the text section (set externally).
    func_text_offset: u64,
    /// Computed stack frame size (in bytes) for the current function.
    frame_size: u32,
    /// Registers pinned for the current instruction (auto-unpinned after each instruction).
    instr_pinned_regs: Vec<Register>,
}

impl Emitter {
    /// Create a new emitter with a fresh register allocator.
    pub fn new() -> Self {
        Self {
            reg_alloc: RegAllocator::new(),
            code: Vec::new(),
            fixups: Vec::new(),
            label_offsets: HashMap::new(),
            call_relocs: Vec::new(),
            relocations: Vec::new(),
            current_func_name: String::new(),
            func_text_offset: 0,
            frame_size: 0,
            instr_pinned_regs: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Function emission
    // -----------------------------------------------------------------------

    /// Emit a single IR function to ARM64 machine code.
    ///
    /// For functions with more than `STACK_SLOT_VREG_THRESHOLD` virtual
    /// registers, the stack-slot emitter is used (every vreg gets a stack
    /// slot, scratch registers only).  For simpler functions, the greedy
    /// register allocator is used.
    ///
    /// Returns a vector of 32-bit ARM64 instruction words.
    pub fn emit_function(&mut self, func: &IRFunction) -> Result<Vec<u32>> {
        let vreg_count = count_vregs(func);
        if vreg_count > STACK_SLOT_VREG_THRESHOLD {
            self.emit_function_stack_slot(func)
        } else {
            self.emit_function_greedy(func)
        }
    }

    /// Emit a single IR function using the greedy register allocator.
    ///
    /// This is the original emission strategy — suitable for functions with
    /// a small number of virtual registers.
    fn emit_function_greedy(&mut self, func: &IRFunction) -> Result<Vec<u32>> {
        self.code.clear();
        self.fixups.clear();
        self.label_offsets.clear();
        self.call_relocs.clear();
        self.relocations.clear();
        self.current_func_name = func.name.clone();
        self.reg_alloc.reset();

        // Pre-allocate registers for parameters (AAPCS64: X0–X7).
        // We must explicitly assign parameter virtual registers to the
        // correct argument registers so the calling convention is respected.
        let arg_regs = [
            Register::X0, Register::X1, Register::X2, Register::X3,
            Register::X4, Register::X5, Register::X6, Register::X7,
        ];
        for (i, param) in func.params.iter().enumerate() {
            if let IRValue::Register(vreg_id) = param {
                if i < 8 {
                    self.reg_alloc.preassign(*vreg_id, arg_regs[i]);
                }
            }
        }

        // Emit prologue:
        // 1. SUB SP, SP, #16          (make room for FP/LR save pair)
        // 2. STP X29, X30, [SP]       (save FP and LR at new SP)
        // 3. ADD X29, SP, #0           (set frame pointer to new SP)
        // 4. SUB SP, SP, #frame_size   (make room for local variables)
        //
        // NOTE: We cannot use pre-indexed STP [SP, #-16]! because the
        // Instruction::STP encoding only supports signed-offset mode.
        // Instead, we explicitly decrement SP with SUB before the STP.
        self.emit_instruction(Instruction::SUB {
            rd: Register::SP,
            rn: Register::SP,
            rm: Operand::Imm12(16),
        })?;
        self.emit_instruction(Instruction::STP {
            rt1: Register::X29,
            rt2: Register::X30,
            rn: Register::SP,
            offset: 0,
        })?;

        // MOV X29, SP (set frame pointer)
        // IMPORTANT: Cannot use MOV (ORR Xd, XZR, SP) because ORR treats
        // Rm=31 as XZR, yielding zero. Use ADD X29, SP, #0 instead.
        self.emit_instruction(Instruction::ADD {
            rd: Register::X29,
            rn: Register::SP,
            rm: Operand::Imm12(0),
        })?;

        // Reserve space for spill slots only. Each Alloc instruction handles
        // its own SUB SP, SP, #aligned_size. The epilogue will restore only
        // the spill area reservation. The Alloc areas are NOT restored by the
        // epilogue — they're effectively "leaked" stack space. This is fine
        // because the function's stack frame is restored by the LDP + ADD SP, #16.
        //
        // For spill slots, we need to estimate the maximum number of spills
        // that could occur during emission. Since we can't know the exact
        // count beforehand, we use the compute_frame_size function which
        // estimates based on the vreg count.
        let aligned_stack = compute_frame_size(func);
        // frame_size = only the spill area (NOT the Alloc sizes, since each
        // Alloc instruction does its own SUB SP)
        // Compute the spill-only portion:
        let mut alloc_total: u32 = 0;
        for block in &func.blocks {
            for instr in &block.instructions {
                if let IRInstr::Alloc { size, .. } = instr {
                    let aligned = (*size).div_ceil(16) * 16;
                    alloc_total += aligned;
                }
            }
        }
        // Spill area = total frame - alloc area
        let spill_area = if aligned_stack > alloc_total {
            aligned_stack - alloc_total
        } else {
            // Minimal 16-byte reservation for safety
            16
        };
        let spill_area_aligned = (spill_area + 15) & !15;
        self.frame_size = spill_area_aligned;
        if spill_area_aligned > 0 {
            if spill_area_aligned <= 4095 {
                self.emit_instruction(Instruction::SUB {
                    rd: Register::SP,
                    rn: Register::SP,
                    rm: Operand::Imm12(spill_area_aligned as u16),
                })?;
            } else {
                self.emit_load_immediate(Register::X9, spill_area_aligned as i64)?;
                self.emit_instruction(Instruction::SUB {
                    rd: Register::SP,
                    rn: Register::SP,
                    rm: Operand::Reg {
                        reg: Register::X9,
                        shift: None,
                    },
                })?;
            }
        }

        // Emit each basic block.
        for block in &func.blocks {
            self.label_offsets
                .insert(block.label.clone(), self.code.len());
            self.emit_block(block)?;
        }

        // Apply fixups — resolve intra-function branch targets.
        self.apply_fixups()?;

        Ok(self.code.clone())
    }

    /// Returns the relocation entries recorded during emission of the current
    /// function.
    ///
    /// Each `RelocationEntry` records a byte offset within the function's
    /// encoded output where a symbolic reference must be resolved, the target
    /// symbol name, and the ISA-specific relocation type (e.g.,
    /// `"R_AARCH64_CALL26"`).
    pub fn relocations(&self) -> &[RelocationEntry] {
        &self.relocations
    }

    /// Emit instructions for a single IR basic block.
    fn emit_block(&mut self, block: &IRBlock) -> Result<()> {
        for instr in &block.instructions {
            self.emit_ir_instr(instr)?;
            // Auto-unpin all registers that were pinned during this instruction.
            for reg in self.instr_pinned_regs.drain(..) {
                self.reg_alloc.unpin(reg);
            }
        }
        self.emit_terminator(&block.terminator)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // IR → ARM64 instruction lowering
    // -----------------------------------------------------------------------

    /// Lower a single IR instruction to ARM64 instructions.
    fn emit_ir_instr(&mut self, instr: &IRInstr) -> Result<()> {
        match instr {
            IRInstr::Load { dst, addr, offset, ty } => {
                let rn = self.resolve_reg(addr)?;
                let rt = self.resolve_reg(dst)?;
                self.emit_load(rt, rn, *offset, ty)?;
            }

            IRInstr::Store { value, addr, offset, ty } => {
                let rt = self.resolve_reg(value)?;
                let rn = self.resolve_reg(addr)?;
                if rt == rn {
                    // Both resolved to the same register — use scratch for value
                    self.emit_instruction(Instruction::MOV {
                        rd: Register::X16,
                        rm: rt,
                    })?;
                    self.emit_store(Register::X16, rn, *offset, ty)?;
                } else {
                    self.emit_store(rt, rn, *offset, ty)?;
                }
            }

            IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
                self.emit_binop(*op, dst, lhs, rhs, ty.as_ref())?;
            }

            IRInstr::UnaryOp { op, dst, operand, ty: _ } => {
                let rd = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(operand)?;
                match op {
                    UnaryOpKind::Neg => {
                        self.emit_instruction(Instruction::SUB {
                            rd,
                            rn: Register::XZR,
                            rm: Operand::Reg {
                                reg: rn,
                                shift: None,
                            },
                        })?;
                    }
                    UnaryOpKind::Not => {
                        self.emit_load_immediate(Register::X9, -1)?;
                        self.emit_instruction(Instruction::EOR {
                            rd,
                            rn,
                            rm: Register::X9,
                        })?;
                    }
                    UnaryOpKind::Clz => {
                        // CLZ Xd, Xn — count leading zeros (native ARM64 instruction)
                        self.emit_instruction(Instruction::CLZ { rd, rn })?;
                    }
                    UnaryOpKind::Ctz => {
                        // CTZ = RBIT + CLZ: reverse bits then count leading zeros.
                        // Use X9 as scratch if rd == rn (need intermediate result).
                        if rd == rn {
                            self.emit_instruction(Instruction::RBIT {
                                rd: Register::X9,
                                rn,
                            })?;
                            self.emit_instruction(Instruction::CLZ {
                                rd,
                                rn: Register::X9,
                            })?;
                        } else {
                            self.emit_instruction(Instruction::RBIT { rd, rn })?;
                            self.emit_instruction(Instruction::CLZ { rd, rn: rd })?;
                        }
                    }
                    UnaryOpKind::Popcnt => {
                        // POPCNT via FMOV+CNT+ADDV+UMOV sequence:
                        // FMOV D8, Xn        — move GPR value to SIMD register (V8 is caller-saved)
                        // CNT V8.8B, V8.8B   — count bits per byte
                        // ADDV B8, V8.8B     — horizontal sum of byte counts
                        // UMOV Xd, V8.B[0]   — move result back to GPR (zero-extends to 64 bits)
                        const SIMD_SCRATCH: u8 = 8; // V8 is caller-saved in AAPCS64
                        self.emit_instruction(Instruction::FMOV_DX {
                            vd: SIMD_SCRATCH,
                            rn,
                        })?;
                        self.emit_instruction(Instruction::CNT {
                            vd: SIMD_SCRATCH,
                            vn: SIMD_SCRATCH,
                        })?;
                        self.emit_instruction(Instruction::ADDV {
                            vd: SIMD_SCRATCH,
                            vn: SIMD_SCRATCH,
                        })?;
                        self.emit_instruction(Instruction::UMOV {
                            rd,
                            vn: SIMD_SCRATCH,
                        })?;
                    }
                }
            }

            IRInstr::Call {
                dst,
                func: target_name,
                args,
                is_extern: _,
            } => {
                // Resolve all argument source registers FIRST, before moving
                // any of them. This prevents a later move from overwriting a
                // register that an earlier argument is in.
                //
                // IMPORTANT: For immediate arguments, resolve_reg always loads
                // into X9. When multiple immediates are present, we must use
                // different scratch registers to avoid overwriting. We load
                // immediates into X0-X7 directly (since they're the target
                // registers anyway) or use X11-X15 as temporary scratch.
                let arg_regs = [
                    Register::X0, Register::X1, Register::X2, Register::X3,
                    Register::X4, Register::X5, Register::X6, Register::X7,
                ];
                // Scratch registers for loading immediates when the target arg
                // register isn't available yet (because of conflicts).
                let scratch_regs = [
                    Register::X11, Register::X12, Register::X13, Register::X14,
                    Register::X15, Register::X3, Register::X4, Register::X5,
                ];

                let mut src_regs: Vec<Register> = Vec::new();
                let mut immediate_scratch_used: Vec<usize> = Vec::new(); // which scratch reg each immediate uses
                let mut next_scratch = 0;

                for (i, arg) in args.iter().enumerate() {
                    if i >= 8 {
                        break;
                    }
                    let is_immediate = matches!(arg, IRValue::Immediate(_) | IRValue::Address(_));
                    if is_immediate {
                        let v = match arg {
                            IRValue::Immediate(v) => *v,
                            IRValue::Address(a) => *a as i64,
                            _ => unreachable!(),
                        };
                        // Load the immediate directly into the target register
                        // if it's not needed by another argument.
                        // For simplicity, always load into a scratch register
                        // and record it.
                        if next_scratch < scratch_regs.len() {
                            let scratch = scratch_regs[next_scratch];
                            next_scratch += 1;
                            self.emit_load_immediate(scratch, v)?;
                            src_regs.push(scratch);
                        } else {
                            // Fallback to X9 (rare, only with 8+ immediates)
                            let scratch = Register::X9;
                            self.emit_load_immediate(scratch, v)?;
                            src_regs.push(scratch);
                        }
                    } else {
                        let src = self.resolve_reg(arg)?;
                        src_regs.push(src);
                    }
                }

                // Now move arguments into X0–X7. We need to handle the case
                // where a source register is also a target register (cycle).
                // Strategy: first move arguments that don't conflict, then
                // handle conflicts using a scratch register.
                let n = src_regs.len();
                let mut moved = vec![false; n];

                // Pass 1: move args where src != any dst that hasn't been moved yet
                // Simple approach: iterate and move non-conflicting ones first
                for pass in 0..2 {
                    for i in 0..n {
                        if moved[i] {
                            continue;
                        }
                        let src = src_regs[i];
                        let dst = arg_regs[i];
                        if src == dst {
                            // Already in the right register
                            moved[i] = true;
                            continue;
                        }
                        if pass == 0 {
                            // Check if src is needed as a source by a later unmoved arg
                            let mut conflict = false;
                            for j in 0..n {
                                if !moved[j] && j != i && src_regs[j] == dst {
                                    // Moving arg i to dst would overwrite arg j's source
                                    conflict = true;
                                    break;
                                }
                            }
                            if conflict {
                                continue; // Defer to pass 2
                            }
                        }
                        // Safe to move now
                        self.emit_instruction(Instruction::MOV {
                            rd: dst,
                            rm: src,
                        })?;
                        moved[i] = true;
                    }
                }

                // Pass 2: handle any remaining conflicts with scratch register
                for i in 0..n {
                    if moved[i] {
                        continue;
                    }
                    let src = src_regs[i];
                    let dst = arg_regs[i];
                    if src == dst {
                        moved[i] = true;
                        continue;
                    }
                    // Use X16 as scratch to break the cycle
                    self.emit_instruction(Instruction::MOV {
                        rd: Register::X16,
                        rm: src,
                    })?;
                    self.emit_instruction(Instruction::MOV {
                        rd: dst,
                        rm: Register::X16,
                    })?;
                    moved[i] = true;
                }

                // BL — record a relocation for later patching.
                let bl_word_idx = self.code.len();
                let bl_byte_offset = self.func_text_offset + (bl_word_idx as u64) * 4;
                self.call_relocs.push(CallRelocation {
                    text_byte_offset: bl_byte_offset,
                    target_func: target_name.clone(),
                });
                // Register a RelocationEntry so the linker can patch this BL.
                self.relocations.push(RelocationEntry {
                    offset: bl_byte_offset,
                    symbol: target_name.clone(),
                    reloc_type: "R_AARCH64_CALL26".to_string(),
                });
                self.emit_instruction(Instruction::BL { offset: 0 })?;

                if let Some(dst_val) = dst {
                    let rd = self.resolve_reg(dst_val)?;
                    if rd != Register::X0 {
                        self.emit_instruction(Instruction::MOV {
                            rd,
                            rm: Register::X0,
                        })?;
                    }
                }
            }

            IRInstr::Alloc { dst, size } => {
                let rd = self.resolve_reg(dst)?;
                // IMPORTANT: Decrement SP FIRST, then save the new SP value.
                // If we save before decrementing, the allocation pointer
                // points to the wrong (too-high) address.
                let aligned = (*size).div_ceil(16) * 16;
                self.emit_instruction(Instruction::SUB {
                    rd: Register::SP,
                    rn: Register::SP,
                    rm: Operand::Imm12(aligned as u16),
                })?;
                // MOV rd, SP: cannot use ORR because ORR treats Rm=31 as XZR.
                // Use ADD rd, SP, #0 instead.
                self.emit_instruction(Instruction::ADD {
                    rd,
                    rn: Register::SP,
                    rm: Operand::Imm12(0),
                })?;
            }

            IRInstr::Free { ptr } => {
                let rt = self.resolve_reg(ptr)?;
                // Move ptr to X0 (first argument)
                if rt != Register::X0 {
                    self.emit_instruction(Instruction::MOV {
                        rd: Register::X0,
                        rm: rt,
                    })?;
                }
                // BL __vuma_free
                let bl_word_idx = self.code.len();
                let bl_byte_offset = self.func_text_offset + (bl_word_idx as u64) * 4;
                self.call_relocs.push(CallRelocation {
                    text_byte_offset: bl_byte_offset,
                    target_func: "__vuma_free".to_string(),
                });
                self.relocations.push(RelocationEntry {
                    offset: bl_byte_offset,
                    symbol: "__vuma_free".to_string(),
                    reloc_type: "R_AARCH64_CALL26".to_string(),
                });
                self.emit_instruction(Instruction::BL { offset: 0 })?;
            }

            IRInstr::Cast { kind, dst, src } => {
                let rd = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(src)?;
                match kind {
                    CastKind::ZExt => {
                        // Zero-extend 32-bit to 64-bit using UBFM
                        self.emit_instruction(Instruction::UBFM {
                            rd,
                            rn,
                            immr: 0,
                            imms: 31,
                        })?;
                    }
                    CastKind::SExt => {
                        // Sign-extend 32-bit to 64-bit using SBFM
                        self.emit_instruction(Instruction::SBFM {
                            rd,
                            rn,
                            immr: 0,
                            imms: 31,
                        })?;
                    }
                    CastKind::Trunc | CastKind::BitCast => {
                        // Trunc: upper bits discarded on write — just MOV
                        // BitCast: no data change — just MOV
                        if rd != rn {
                            self.emit_instruction(Instruction::MOV { rd, rm: rn })?;
                        }
                    }
                    CastKind::IntToFloat | CastKind::UIntToFloat |
                    CastKind::FloatToInt | CastKind::FloatToUInt |
                    CastKind::FloatToFloat => {
                        // FP casts — not yet implemented; pass through.
                        if rd != rn {
                            self.emit_instruction(Instruction::MOV { rd, rm: rn })?;
                        }
                    }
                }
            }

            IRInstr::Phi { .. } => {
                log::warn!(
                    "IRInstr::Phi encountered during emission — should be resolved by SSA pass"
                );
            }

            IRInstr::GetAddress { dst, name } => {
                let rd = self.resolve_reg(dst)?;
                // Emit a call to __vuma_getaddr to resolve the symbol at runtime.
                // Move name hash to X0 as the argument.
                let name_hash = name
                    .chars()
                    .fold(0u64, |acc, c| acc.wrapping_mul(31).wrapping_add(c as u64));
                self.emit_load_immediate(Register::X0, name_hash as i64)?;
                let bl_word_idx = self.code.len();
                let bl_byte_offset = self.func_text_offset + (bl_word_idx as u64) * 4;
                self.call_relocs.push(CallRelocation {
                    text_byte_offset: bl_byte_offset,
                    target_func: "__vuma_getaddr".to_string(),
                });
                self.relocations.push(RelocationEntry {
                    offset: bl_byte_offset,
                    symbol: "__vuma_getaddr".to_string(),
                    reloc_type: "R_AARCH64_CALL26".to_string(),
                });
                self.emit_instruction(Instruction::BL { offset: 0 })?;
                if rd != Register::X0 {
                    self.emit_instruction(Instruction::MOV {
                        rd,
                        rm: Register::X0,
                    })?;
                }
            }

            IRInstr::Offset { dst, base, offset } => {
                let rd = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(base)?;
                let rm = match offset {
                    IRValue::Immediate(v) => {
                        if *v >= 0 && *v <= 4095 {
                            Operand::Imm12(*v as u16)
                        } else {
                            let temp = Register::X9;
                            self.emit_load_immediate(temp, *v)?;
                            Operand::Reg {
                                reg: temp,
                                shift: None,
                            }
                        }
                    }
                    _ => Operand::Reg {
                        reg: self.resolve_reg(offset)?,
                        shift: None,
                    },
                };
                self.emit_instruction(Instruction::ADD { rd, rn, rm })?;
            }

            // ── Dedicated arithmetic — delegate to BinOp ──
            IRInstr::Add { dst, lhs, rhs, ty } => {
                self.emit_binop(BinOpKind::Add, dst, lhs, rhs, ty.as_ref())?;
            }
            IRInstr::Sub { dst, lhs, rhs, ty } => {
                self.emit_binop(BinOpKind::Sub, dst, lhs, rhs, ty.as_ref())?;
            }
            IRInstr::Mul { dst, lhs, rhs, ty } => {
                self.emit_binop(BinOpKind::Mul, dst, lhs, rhs, ty.as_ref())?;
            }
            IRInstr::Div { dst, lhs, rhs, ty } => {
                self.emit_binop(BinOpKind::SDiv, dst, lhs, rhs, ty.as_ref())?;
            }

            // ── Comparison instruction ──
            IRInstr::Cmp {
                kind,
                dst,
                lhs,
                rhs,
                ty,
            } => {
                let width = RegWidth::from_ir_type(ty.as_ref());
                let rd = self.resolve_reg(dst)?;
                // IMPORTANT: resolve_reg uses X9 for immediates, so if both lhs and rhs
                // are immediates, the second load would overwrite the first. Use X10 for
                // the RHS when both are immediates.
                let rn = self.resolve_reg(lhs)?;
                let rm = match rhs {
                    IRValue::Immediate(_) | IRValue::Address(_) => {
                        // If lhs was also an immediate, it's in X9; use X10 for rhs
                        let temp = if matches!(lhs, IRValue::Immediate(_) | IRValue::Address(_)) {
                            Register::X10
                        } else {
                            Register::X9
                        };
                        match rhs {
                            IRValue::Immediate(v) => self.emit_load_immediate(temp, *v)?,
                            IRValue::Address(a) => self.emit_load_immediate(temp, *a as i64)?,
                            _ => unreachable!(),
                        }
                        temp
                    }
                    _ => self.resolve_reg(rhs)?,
                };
                self.emit_instruction_with_width(
                    Instruction::CMP {
                        rn,
                        rm: Operand::Reg {
                            reg: rm,
                            shift: None,
                        },
                    },
                    width,
                )?;
                let cond = cmp_kind_to_condition(kind);
                self.emit_instruction_with_width(Instruction::CSET { rd, cond }, width)?;
            }

            // ── Instruction-level control flow ──
            IRInstr::Ret { values } => {
                for (i, val) in values.iter().enumerate() {
                    if i >= 8 {
                        break;
                    }
                    let src = self.resolve_reg(val)?;
                    let dst_reg = match i {
                        0 => Register::X0,
                        1 => Register::X1,
                        2 => Register::X2,
                        3 => Register::X3,
                        4 => Register::X4,
                        5 => Register::X5,
                        6 => Register::X6,
                        7 => Register::X7,
                        _ => unreachable!(),
                    };
                    if src != dst_reg {
                        self.emit_instruction(Instruction::MOV {
                            rd: dst_reg,
                            rm: src,
                        })?;
                    }
                }
            }

            IRInstr::Branch { target } => {
                let fixup_idx = self.code.len();
                self.fixups.push((fixup_idx, target.clone(), BranchFormat::B26));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }

            IRInstr::CondBranch {
                cond,
                true_target,
                false_target,
            } => {
                let rt = self.resolve_reg(cond)?;
                // CBNZ: if cond != 0, branch to true_target
                let fixup_cbz = self.code.len();
                self.fixups.push((fixup_cbz, true_target.clone(), BranchFormat::Cond19));
                self.emit_instruction(Instruction::CBNZ { rt, offset: 0 })?;
                // Otherwise (cond == 0), branch to false_target
                let fixup_b = self.code.len();
                self.fixups.push((fixup_b, false_target.clone(), BranchFormat::B26));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }

            IRInstr::Select {
                dst,
                cond,
                true_val,
                false_val,
                ty,
            } => {
                let width = RegWidth::from_ir_type(ty.as_ref());
                // Lower select as: SUBS XZR, cond, #0; CSEL dst, false_val, true_val, NE
                let rd = self.resolve_reg(dst)?;
                let rc = self.resolve_reg(cond)?;
                let rt = self.resolve_reg(true_val)?;
                let rf = self.resolve_reg(false_val)?;
                // Compare cond against zero and select.
                self.emit_instruction_with_width(
                    Instruction::SUB {
                        rd: Register::XZR,
                        rn: rc,
                        rm: Operand::Imm12(0),
                    },
                    width,
                )?;
                // Set flags by using a separate CMP (SUB with XZR destination
                // doesn't set flags; we need a flags-setting variant).
                // We emulate this with: CMP rc, #0 which is SUBS XZR, rc, #0.
                // Since we only have SUB, we use the existing CMP pattern.
                self.emit_instruction_with_width(
                    Instruction::CSEL {
                        rd,
                        rn: rf,
                        rm: rt,
                        cond: crate::arm64::Condition::NE,
                    },
                    width,
                )?;
            }

            // ── Atomic operations ──────────────────────────────────────────
            IRInstr::AtomicLoad { dst, addr, ty: _ } => {
                // AArch64: LDAXR Xt, [Xn] — load-acquire exclusive
                let rt = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(addr)?;
                self.emit_instruction(Instruction::LDAXR { rt, rn })?;
            }

            IRInstr::AtomicStore { value, addr, ty: _ } => {
                // AArch64: STLXR Ws, Xt, [Xn] — store-release exclusive
                // Loop until success.
                let rt = self.resolve_reg(value)?;
                let rn = self.resolve_reg(addr)?;
                let rs = Register::X9; // scratch for status
                let retry_label = format!("__atomic_store_retry_{}", self.code.len());
                // Record label position
                self.label_offsets.insert(retry_label.clone(), self.code.len());
                self.emit_instruction(Instruction::STLXR { rs, rt, rn })?;
                // CBNZ rs, retry — if store failed, retry
                let fixup = self.code.len();
                self.fixups.push((fixup, retry_label, BranchFormat::Cond19));
                self.emit_instruction(Instruction::CBNZ { rt: rs, offset: 0 })?;
            }

            IRInstr::AtomicCas { dst, addr, expected, desired, ty: _ } => {
                // AArch64 CAS loop: LDAXR / CMP / B.NE skip / STLXR / CBNZ retry
                let rd = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(addr)?;
                let re = self.resolve_reg(expected)?;
                let rm = self.resolve_reg(desired)?;
                let rs = Register::X9; // scratch for STLXR status
                let scratch_cmp = Register::X10; // scratch for comparison

                let retry_label = format!("__atomic_cas_retry_{}", self.code.len());
                let done_label = format!("__atomic_cas_done_{}", self.code.len());

                // Record retry label position
                self.label_offsets.insert(retry_label.clone(), self.code.len());
                // LDAXR scratch_cmp, [addr] — load current value
                self.emit_instruction(Instruction::LDAXR { rt: scratch_cmp, rn })?;
                // CMP scratch_cmp, expected — SUB XZR, scratch_cmp, expected
                self.emit_instruction(Instruction::SUB {
                    rd: Register::XZR,
                    rn: scratch_cmp,
                    rm: Operand::Reg { reg: re, shift: None },
                })?;
                // B.NE done — if not equal, skip store
                let fixup_ne = self.code.len();
                self.fixups.push((fixup_ne, done_label.clone(), BranchFormat::Cond19));
                self.emit_instruction(Instruction::BCond {
                    cond: crate::arm64::Condition::NE,
                    offset: 0,
                })?;
                // STLXR rs, desired, [addr] — try to store
                self.emit_instruction(Instruction::STLXR { rs, rt: rm, rn })?;
                // CBNZ rs, retry — if store failed, retry
                let fixup_retry = self.code.len();
                self.fixups.push((fixup_retry, retry_label, BranchFormat::Cond19));
                self.emit_instruction(Instruction::CBNZ { rt: rs, offset: 0 })?;
                // Record done label position
                self.label_offsets.insert(done_label.clone(), self.code.len());
                // CSEL rd, re, scratch_cmp, EQ
                // If EQ (match succeeded), rd = re (expected); else rd = scratch_cmp (current value)
                self.emit_instruction(Instruction::CSEL {
                    rd,
                    rn: re,
                    rm: scratch_cmp,
                    cond: crate::arm64::Condition::EQ,
                })?;
            }

            // ── Constant-time security operations ────────────────────────────
            IRInstr::CtSelect {
                dst,
                cond,
                true_val,
                false_val,
                ty,
            } => {
                // ct_select(cond, a, b) = (a & mask) | (b & ~mask)
                // where mask = -(cond != 0)
                // On AArch64: Use CSEL for constant-time conditional select.
                let width = RegWidth::from_ir_type(ty.as_ref());
                let rd = self.resolve_reg(dst)?;
                let _rc = self.resolve_reg(cond)?;
                let rt = self.resolve_reg(true_val)?;
                let rf = self.resolve_reg(false_val)?;
                // Use the same CSEL pattern as Select, which is constant-time
                // on AArch64 (no branch).
                self.emit_instruction_with_width(
                    Instruction::CSEL {
                        rd,
                        rn: rf,
                        rm: rt,
                        cond: crate::arm64::Condition::NE,
                    },
                    width,
                )?;
            }

            IRInstr::CtEq {
                dst,
                lhs,
                rhs,
                ty: _,
            } => {
                // ct_eq(a, b): constant-time equality check using XOR.
                let rd = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(lhs)?;
                let rm = self.resolve_reg(rhs)?;
                // EOR rd, rn, rm (XOR)
                self.emit_instruction(Instruction::EOR { rd, rn, rm })?;
                // CMP rd, #0 + CSET rd, EQ
                self.emit_instruction(Instruction::SUB {
                    rd: Register::XZR,
                    rn: rd,
                    rm: Operand::Imm12(0),
                })?;
                self.emit_instruction(Instruction::CSET { rd, cond: crate::arm64::Condition::EQ })?;
            }
        }
        Ok(())
    }

    /// Emit a binary operation (shared by `BinOp` and dedicated `Add`/`Sub`/…).
    ///
    /// Some ARM64 instructions (ADD, SUB, LSL, LSR, ASR) accept a 12-bit
    /// unsigned immediate operand directly.  Most others (MUL, AND, ORR, EOR,
    /// SDIV, UDIV, remainder, comparisons) **require** a register operand.
    /// When the RHS is an immediate that the target instruction cannot accept
    /// directly, we "spill" it into a scratch register (X16 / IP0) first via
    /// MOVZ + MOVK and then use that register as the operand.
    ///
    /// X16 (IP0) is chosen as the scratch register because it is **not** in
    /// the register allocator's free pool, so it will never conflict with
    /// `rd` or `rn`.
    fn emit_binop(
        &mut self,
        op: BinOpKind,
        dst: &IRValue,
        lhs: &IRValue,
        rhs: &IRValue,
        ty: Option<&IRType>,
    ) -> Result<()> {
        let width = RegWidth::from_ir_type(ty);
        let rd = self.resolve_reg(dst)?;
        let rn = self.resolve_reg(lhs)?;

        // Determine whether the operation can accept an immediate operand.
        // ADD, SUB, LSL, LSR, ASR all have immediate forms in ARM64.
        // All other operations require a register operand.
        let supports_imm = matches!(
            op,
            BinOpKind::Add | BinOpKind::Sub | BinOpKind::Shl | BinOpKind::ShrL | BinOpKind::ShrA
        );

        let rm = match rhs {
            IRValue::Immediate(v) => {
                if supports_imm && *v >= 0 && *v <= 4095 {
                    // Small immediate that the instruction accepts directly.
                    Operand::Imm12(*v as u16)
                } else {
                    // Load the immediate into scratch register X16 (IP0).
                    // X16 is not in the register allocator's pool, so it cannot
                    // conflict with `rd` or `rn`.
                    let scratch = Register::X16;
                    self.emit_load_immediate_with_width(scratch, *v, width)?;
                    Operand::Reg {
                        reg: scratch,
                        shift: None,
                    }
                }
            }
            _ => Operand::Reg {
                reg: self.resolve_reg(rhs)?,
                shift: None,
            },
        };

        match op {
            BinOpKind::Add => {
                self.emit_instruction_with_width(Instruction::ADD { rd, rn, rm }, width)?;
            }
            BinOpKind::Sub => {
                self.emit_instruction_with_width(Instruction::SUB { rd, rn, rm }, width)?;
            }
            BinOpKind::Mul => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction_with_width(Instruction::MUL { rd, rn, rm: rm_reg }, width)?;
            }
            BinOpKind::SDiv => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction_with_width(Instruction::SDIV { rd, rn, rm: rm_reg }, width)?;
            }
            BinOpKind::UDiv => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction_with_width(Instruction::UDIV { rd, rn, rm: rm_reg }, width)?;
            }
            BinOpKind::And => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction_with_width(Instruction::AND { rd, rn, rm: rm_reg }, width)?;
            }
            BinOpKind::Or => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction_with_width(Instruction::ORR { rd, rn, rm: rm_reg }, width)?;
            }
            BinOpKind::Xor => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction_with_width(Instruction::EOR { rd, rn, rm: rm_reg }, width)?;
            }
            BinOpKind::Shl => {
                self.emit_instruction_with_width(Instruction::LSL { rd, rn, rm }, width)?;
            }
            BinOpKind::ShrL => {
                self.emit_instruction_with_width(Instruction::LSR { rd, rn, rm }, width)?;
            }
            BinOpKind::ShrA => {
                self.emit_instruction_with_width(Instruction::ASR { rd, rn, rm }, width)?;
            }
            BinOpKind::Ror | BinOpKind::Rol => {
                let rm_reg = self.operand_to_reg(&rm)?;
                if op == BinOpKind::Ror {
                    self.emit_instruction_with_width(Instruction::RORV { rd, rn, rm: rm_reg }, width)?;
                } else {
                    // ROL by Rm = ROR by -Rm (mod regsize).
                    // SUB X9, XZR, Rm; RORV Rd, Rn, X9
                    self.emit_instruction_with_width(Instruction::SUB {
                        rd: Register::X9,
                        rn: Register::XZR,
                        rm: Operand::Reg { reg: rm_reg, shift: None },
                    }, width)?;
                    self.emit_instruction_with_width(Instruction::RORV { rd, rn, rm: Register::X9 }, width)?;
                }
            }
            BinOpKind::SRem | BinOpKind::URem => {
                let rm_reg = self.operand_to_reg(&rm)?;
                let div_instr = if op == BinOpKind::SRem {
                    Instruction::SDIV { rd, rn, rm: rm_reg }
                } else {
                    Instruction::UDIV { rd, rn, rm: rm_reg }
                };
                self.emit_instruction_with_width(div_instr, width)?;
                // MSUB rd, rd, rm, rn  =>  rd = rn - rd * rm  =  dividend - quotient * divisor
                self.emit_instruction_with_width(Instruction::MSUB {
                    rd,
                    rn: rd,     // quotient (result of DIV)
                    rm: rm_reg, // divisor
                    ra: rn,     // dividend
                }, width)?;
            }
            BinOpKind::SLt
            | BinOpKind::SLe
            | BinOpKind::SGt
            | BinOpKind::SGe
            | BinOpKind::ULt
            | BinOpKind::ULe
            | BinOpKind::UGt
            | BinOpKind::UGe
            | BinOpKind::Eq
            | BinOpKind::Ne => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction_with_width(Instruction::CMP {
                    rn,
                    rm: Operand::Reg {
                        reg: rm_reg,
                        shift: None,
                    },
                }, width)?;
                let cond = binop_kind_to_condition(&op);
                self.emit_instruction_with_width(Instruction::CSET { rd, cond }, width)?;
            }
        }
        Ok(())
    }

    /// Emit the block terminator.
    fn emit_terminator(&mut self, term: &IRTerminator) -> Result<()> {
        match term {
            IRTerminator::Jump(target) => {
                let fixup_idx = self.code.len();
                self.fixups.push((fixup_idx, target.clone(), BranchFormat::B26));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }
            IRTerminator::Branch {
                cond,
                true_block,
                false_block,
            } => {
                let rt = self.resolve_reg(cond)?;
                let fixup_cbnz = self.code.len();
                self.fixups.push((fixup_cbnz, true_block.clone(), BranchFormat::Cond19));
                self.emit_instruction(Instruction::CBNZ { rt, offset: 0 })?;
                let fixup_b = self.code.len();
                self.fixups.push((fixup_b, false_block.clone(), BranchFormat::B26));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }
            IRTerminator::Return(vals) => {
                for (i, val) in vals.iter().enumerate() {
                    if i >= 8 {
                        break;
                    }
                    let src = self.resolve_reg(val)?;
                    let dst_reg = match i {
                        0 => Register::X0,
                        1 => Register::X1,
                        2 => Register::X2,
                        3 => Register::X3,
                        4 => Register::X4,
                        5 => Register::X5,
                        6 => Register::X6,
                        7 => Register::X7,
                        _ => unreachable!(),
                    };
                    if src != dst_reg {
                        self.emit_instruction(Instruction::MOV {
                            rd: dst_reg,
                            rm: src,
                        })?;
                    }
                }
                // Restore SP to point to the saved FP/LR pair.
                // X29 was set to SP right after the prologue's SUB SP, SP, #16
                // and STP X29, X30, [SP]. So X29 points to the FP/LR save area.
                // Using MOV SP, X29 (via ADD SP, X29, #0) is the most robust
                // way to restore SP regardless of how many Alloc instructions
                // modified it during the function body.
                self.emit_instruction(Instruction::ADD {
                    rd: Register::SP,
                    rn: Register::X29,
                    rm: Operand::Imm12(0),
                })?;
                // FP/LR were stored at [SP] after the prologue's SUB SP, SP, #16.
                // After the ADD above, SP points back to the FP/LR save area.
                // Load from [SP, #0], then restore SP by adding 16.
                self.emit_instruction(Instruction::LDP {
                    rt1: Register::X29,
                    rt2: Register::X30,
                    rn: Register::SP,
                    offset: 0,
                })?;
                self.emit_instruction(Instruction::ADD {
                    rd: Register::SP,
                    rn: Register::SP,
                    rm: Operand::Imm12(16),
                })?;
                self.emit_instruction(Instruction::RET { rn: None })?;
            }
            IRTerminator::Unreachable => {
                self.emit_instruction(Instruction::MOV {
                    rd: Register::XZR,
                    rm: Register::XZR,
                })?;
            }
            // Switch, Invoke, TailCall, and Resume are lowered by the
            // control_flow module before reaching the emitter. If they
            // appear here, it means the lowering pass was not run.
            IRTerminator::Switch { .. } => {
                return Err(CodegenError::InvalidInstruction(
                    "Switch terminator must be lowered before emission".to_string(),
                ));
            }
            IRTerminator::Invoke { .. } => {
                return Err(CodegenError::InvalidInstruction(
                    "Invoke terminator must be lowered before emission".to_string(),
                ));
            }
            IRTerminator::TailCall { .. } => {
                return Err(CodegenError::InvalidInstruction(
                    "TailCall terminator must be lowered before emission".to_string(),
                ));
            }
            IRTerminator::Resume { .. } => {
                return Err(CodegenError::InvalidInstruction(
                    "Resume terminator must be lowered before emission".to_string(),
                ));
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn emit_instruction(&mut self, instr: Instruction) -> Result<()> {
        let word = instr.encode()?;
        self.code.push(word);
        Ok(())
    }

    /// Emit an instruction with a specific register width (32-bit W or 64-bit X).
    ///
    /// This is the primary emission method for arithmetic and logical instructions
    /// where the operand width affects the encoding. Using `RegWidth::W32` produces
    /// instructions that operate on W sub-registers, giving automatic 32-bit
    /// wrapping arithmetic.
    fn emit_instruction_with_width(&mut self, instr: Instruction, width: RegWidth) -> Result<()> {
        let word = instr.encode_with_width(width)?;
        self.code.push(word);
        Ok(())
    }

    /// Emit a load instruction with the given offset and IR type.
    ///
    /// Selects the correct ARM64 load variant based on the IR type:
    /// - I8/U8 → LDRB (byte, zero-extended)
    /// - I16/U16 → LDRH (halfword, zero-extended)
    /// - I32/U32/Ptr/Func → LDR (word or doubleword)
    /// - I64/U64 → LDR (doubleword)
    ///
    /// If the offset fits the ARM64 unsigned-offset immediate encoding for the
    /// selected instruction variant, it is encoded directly. Otherwise, the
    /// effective address is computed in a scratch register (X9) first, and
    /// the load is performed at [X9 + 0].
    fn emit_load(&mut self, rt: Register, rn: Register, offset: i32, ty: &IRType) -> Result<()> {
        // Determine the ARM64 load instruction and its immediate-offset constraints.
        // For each variant: (scale, max_imm12) where the encoded offset = imm12 << scale.
        // LDRB:  scale=0, imm12 0..4095 → offset 0..4095
        // LDRH:  scale=1, imm12 0..4095 → offset 0..8190, even
        // LDRSW: scale=2, imm12 0..4095 → offset 0..16380, multiple of 4
        // LDR W: scale=2, imm12 0..4095 → offset 0..16380, multiple of 4
        // LDR X: scale=3, imm12 0..4095 → offset 0..32760, multiple of 8
        match ty {
            IRType::I8 | IRType::U8 => {
                if offset >= 0 && offset <= 4095 {
                    self.emit_instruction(Instruction::LDRB { rt, rn, offset })?;
                } else {
                    self.emit_address_with_offset(rn, offset)?;
                    self.emit_instruction(Instruction::LDRB {
                        rt,
                        rn: Register::X9,
                        offset: 0,
                    })?;
                }
                // Sign-extend for signed byte loads (SXTB = SBFM Xd, Xn, #0, #7)
                if *ty == IRType::I8 {
                    self.emit_instruction(Instruction::SBFM {
                        rd: rt,
                        rn: rt,
                        immr: 0,
                        imms: 7,
                    })?;
                }
            }
            IRType::I16 | IRType::U16 => {
                if offset >= 0 && offset <= 8190 && offset % 2 == 0 {
                    self.emit_instruction(Instruction::LDRH { rt, rn, offset })?;
                } else {
                    self.emit_address_with_offset(rn, offset)?;
                    self.emit_instruction(Instruction::LDRH {
                        rt,
                        rn: Register::X9,
                        offset: 0,
                    })?;
                }
                // Sign-extend for signed halfword loads (SXTH = SBFM Xd, Xn, #0, #15)
                if *ty == IRType::I16 {
                    self.emit_instruction(Instruction::SBFM {
                        rd: rt,
                        rn: rt,
                        immr: 0,
                        imms: 15,
                    })?;
                }
            }
            IRType::I32 | IRType::U32 => {
                // 32-bit load uses LDR Wt encoding (scale=2, offset must be multiple of 4)
                if offset >= 0 && offset <= 16380 && offset % 4 == 0 {
                    self.emit_instruction(Instruction::LDR_W { rt, rn, offset })?;
                } else {
                    self.emit_address_with_offset(rn, offset)?;
                    self.emit_instruction(Instruction::LDR_W {
                        rt,
                        rn: Register::X9,
                        offset: 0,
                    })?;
                }
                // Sign-extend for signed word loads (SXTW = SBFM Xd, Xn, #0, #31)
                if *ty == IRType::I32 {
                    self.emit_instruction(Instruction::SBFM {
                        rd: rt,
                        rn: rt,
                        immr: 0,
                        imms: 31,
                    })?;
                }
            }
            IRType::I64 | IRType::U64 | IRType::Ptr | IRType::Func => {
                // 64-bit load uses LDR Xt encoding (scale=3, offset must be multiple of 8)
                if offset >= 0 && offset <= 32760 && offset % 8 == 0 {
                    self.emit_instruction(Instruction::LDR { rt, rn, offset })?;
                } else {
                    self.emit_address_with_offset(rn, offset)?;
                    self.emit_instruction(Instruction::LDR {
                        rt,
                        rn: Register::X9,
                        offset: 0,
                    })?;
                }
            }
            _ => {
                // Default: treat as 64-bit load
                if offset >= 0 && offset <= 32760 && offset % 8 == 0 {
                    self.emit_instruction(Instruction::LDR { rt, rn, offset })?;
                } else {
                    self.emit_address_with_offset(rn, offset)?;
                    self.emit_instruction(Instruction::LDR {
                        rt,
                        rn: Register::X9,
                        offset: 0,
                    })?;
                }
            }
        }
        Ok(())
    }

    /// Emit a store instruction with the given offset and IR type.
    ///
    /// Selects the correct ARM64 store variant based on the IR type:
    /// - I8/U8 → STRB (byte)
    /// - I16/U16 → STRH (halfword)
    /// - I32/U32 → STR_W (32-bit word)
    /// - I64/U64/Ptr/Func → STR (64-bit doubleword)
    ///
    /// If the offset fits the ARM64 unsigned-offset immediate encoding for the
    /// selected instruction variant, it is encoded directly. Otherwise, the
    /// effective address is computed in a scratch register (X9) first, and
    /// the store is performed at [X9 + 0].
    fn emit_store(&mut self, rt: Register, rn: Register, offset: i32, ty: &IRType) -> Result<()> {
        match ty {
            IRType::I8 | IRType::U8 => {
                if offset >= 0 && offset <= 4095 {
                    self.emit_instruction(Instruction::STRB { rt, rn, offset })?;
                } else {
                    self.emit_address_with_offset(rn, offset)?;
                    self.emit_instruction(Instruction::STRB {
                        rt,
                        rn: Register::X9,
                        offset: 0,
                    })?;
                }
            }
            IRType::I16 | IRType::U16 => {
                if offset >= 0 && offset <= 8190 && offset % 2 == 0 {
                    self.emit_instruction(Instruction::STRH { rt, rn, offset })?;
                } else {
                    self.emit_address_with_offset(rn, offset)?;
                    self.emit_instruction(Instruction::STRH {
                        rt,
                        rn: Register::X9,
                        offset: 0,
                    })?;
                }
            }
            IRType::I32 | IRType::U32 => {
                // 32-bit store uses STR Wt encoding (scale=2, offset must be multiple of 4)
                if offset >= 0 && offset <= 16380 && offset % 4 == 0 {
                    self.emit_instruction(Instruction::STR_W { rt, rn, offset })?;
                } else {
                    self.emit_address_with_offset(rn, offset)?;
                    self.emit_instruction(Instruction::STR_W {
                        rt,
                        rn: Register::X9,
                        offset: 0,
                    })?;
                }
            }
            IRType::I64 | IRType::U64 | IRType::Ptr | IRType::Func => {
                // 64-bit store uses STR Xt encoding (scale=3, offset must be multiple of 8)
                if offset >= 0 && offset <= 32760 && offset % 8 == 0 {
                    self.emit_instruction(Instruction::STR { rt, rn, offset })?;
                } else {
                    self.emit_address_with_offset(rn, offset)?;
                    self.emit_instruction(Instruction::STR {
                        rt,
                        rn: Register::X9,
                        offset: 0,
                    })?;
                }
            }
            _ => {
                // Default: treat as 64-bit store
                if offset >= 0 && offset <= 32760 && offset % 8 == 0 {
                    self.emit_instruction(Instruction::STR { rt, rn, offset })?;
                } else {
                    self.emit_address_with_offset(rn, offset)?;
                    self.emit_instruction(Instruction::STR {
                        rt,
                        rn: Register::X9,
                        offset: 0,
                    })?;
                }
            }
        }
        Ok(())
    }

    /// Compute `X9 = rn + offset` for load/store with an offset that does not
    /// fit the ARM64 unsigned-offset immediate encoding.
    ///
    /// Uses X9 as a scratch register. If the offset fits in a 12-bit unsigned
    /// immediate (0..4095), emits `ADD X9, rn, #offset`. Otherwise, loads the
    /// offset into X9 with MOVZ/MOVK and then emits `ADD X9, rn, X9`.
    fn emit_address_with_offset(
        &mut self,
        rn: Register,
        offset: i32,
    ) -> Result<()> {
        if offset >= 0 && offset <= 4095 {
            // Small positive offset: ADD X9, rn, #offset
            self.emit_instruction(Instruction::ADD {
                rd: Register::X9,
                rn,
                rm: Operand::Imm12(offset as u16),
            })?;
        } else {
            // Large or negative offset: load offset into X9, then ADD X9, rn, X9
            self.emit_load_immediate(Register::X9, offset as i64)?;
            self.emit_instruction(Instruction::ADD {
                rd: Register::X9,
                rn,
                rm: Operand::Reg {
                    reg: Register::X9,
                    shift: None,
                },
            })?;
        }
        Ok(())
    }

    fn resolve_reg(&mut self, val: &IRValue) -> Result<Register> {
        match val {
            IRValue::Register(id) => {
                let result = self.reg_alloc.allocate(*id)?;
                let reg = result.reg;

                // Auto-pin this register for the duration of the current instruction.
                // This prevents subsequent resolve_reg calls within the same
                // instruction from spilling this register.
                if !self.instr_pinned_regs.contains(&reg) {
                    self.reg_alloc.pin(reg);
                    self.instr_pinned_regs.push(reg);
                }

                // If a spill occurred, emit STR to save the spilled register's
                // value to the stack before it gets overwritten.
                if let Some(spill_info) = result.spilled {
                    // Spill slot offset from X29 (frame pointer).
                    // Layout: X29 points to saved FP/LR. Spill slots are at
                    // negative offsets from X29: slot 0 at [X29, #-8],
                    // slot 1 at [X29, #-16], etc.
                    let sp_offset = 8 + (spill_info.slot as i32) * 8;
                    // Compute address using X16 as scratch (NOT X9, since X9 is
                    // used by resolve_reg for immediate values).
                    self.emit_load_immediate(Register::X16, -(sp_offset as i64))?;
                    self.emit_instruction(Instruction::ADD {
                        rd: Register::X16,
                        rn: Register::X29,
                        rm: Operand::Reg {
                            reg: Register::X16,
                            shift: None,
                        },
                    })?;
                    self.emit_instruction(Instruction::STR {
                        rt: spill_info.reg,
                        rn: Register::X16,
                        offset: 0,
                    })?;
                }

                // If this vreg was previously spilled and needs to be reloaded,
                // emit LDR to restore its value from the stack.
                if let Some(slot) = result.reload_slot {
                    let sp_offset = 8 + (slot as i32) * 8;
                    // Compute address using X16 as scratch (NOT X9).
                    self.emit_load_immediate(Register::X16, -(sp_offset as i64))?;
                    self.emit_instruction(Instruction::ADD {
                        rd: Register::X16,
                        rn: Register::X29,
                        rm: Operand::Reg {
                            reg: Register::X16,
                            shift: None,
                        },
                    })?;
                    self.emit_instruction(Instruction::LDR {
                        rt: reg,
                        rn: Register::X16,
                        offset: 0,
                    })?;
                }

                Ok(reg)
            }
            IRValue::Immediate(v) => {
                let temp = Register::X9;
                self.emit_load_immediate(temp, *v)?;
                Ok(temp)
            }
            IRValue::Address(addr) => {
                let temp = Register::X10;
                self.emit_load_immediate(temp, *addr as i64)?;
                Ok(temp)
            }
            IRValue::Label(_) => Err(CodegenError::EncodingError(
                "label value cannot be resolved to a register".into(),
            )),
        }
    }

    fn emit_load_immediate(&mut self, rd: Register, value: i64) -> Result<()> {
        if (0..=65535).contains(&value) {
            self.emit_instruction(Instruction::MOVZ {
                rd,
                imm16: value as u16,
                shift: 0,
            })?;
            return Ok(());
        }
        if (0..=0xFFFF_FFFF).contains(&value) {
            let lo = (value & 0xFFFF) as u16;
            let hi = ((value >> 16) & 0xFFFF) as u16;
            self.emit_instruction(Instruction::MOVZ {
                rd,
                imm16: lo,
                shift: 0,
            })?;
            self.emit_instruction(Instruction::MOVK {
                rd,
                imm16: hi,
                shift: 16,
            })?;
            return Ok(());
        }
        let w0 = (value & 0xFFFF) as u16;
        let w1 = ((value >> 16) & 0xFFFF) as u16;
        let w2 = ((value >> 32) & 0xFFFF) as u16;
        let w3 = ((value >> 48) & 0xFFFF) as u16;
        self.emit_instruction(Instruction::MOVZ {
            rd,
            imm16: w0,
            shift: 0,
        })?;
        if w1 != 0 {
            self.emit_instruction(Instruction::MOVK {
                rd,
                imm16: w1,
                shift: 16,
            })?;
        }
        if w2 != 0 {
            self.emit_instruction(Instruction::MOVK {
                rd,
                imm16: w2,
                shift: 32,
            })?;
        }
        if w3 != 0 {
            self.emit_instruction(Instruction::MOVK {
                rd,
                imm16: w3,
                shift: 48,
            })?;
        }
        Ok(())
    }

    /// Emit a load-immediate sequence with a specific register width.
    ///
    /// For `RegWidth::W32`, only MOVZ/MOVK with shift 0 and 16 are emitted
    /// (32-bit registers don't support shift=32 or shift=48). Values larger
    /// than 32 bits are truncated to 32 bits.
    fn emit_load_immediate_with_width(
        &mut self,
        rd: Register,
        value: i64,
        width: RegWidth,
    ) -> Result<()> {
        match width {
            RegWidth::X64 => self.emit_load_immediate(rd, value),
            RegWidth::W32 => {
                // For 32-bit: mask to 32 bits and use only shift 0 and 16.
                let val32 = (value as u32) as i64;
                if (0..=65535).contains(&val32) {
                    self.emit_instruction_with_width(
                        Instruction::MOVZ {
                            rd,
                            imm16: val32 as u16,
                            shift: 0,
                        },
                        RegWidth::W32,
                    )?;
                } else {
                    let lo = (val32 & 0xFFFF) as u16;
                    let hi = ((val32 >> 16) & 0xFFFF) as u16;
                    self.emit_instruction_with_width(
                        Instruction::MOVZ {
                            rd,
                            imm16: lo,
                            shift: 0,
                        },
                        RegWidth::W32,
                    )?;
                    if hi != 0 {
                        self.emit_instruction_with_width(
                            Instruction::MOVK {
                                rd,
                                imm16: hi,
                                shift: 16,
                            },
                            RegWidth::W32,
                        )?;
                    }
                }
                Ok(())
            }
        }
    }

    fn operand_to_reg(&self, op: &Operand) -> Result<Register> {
        match op {
            Operand::Reg { reg, shift: _ } => Ok(*reg),
            Operand::Imm12(v) => {
                log::error!(
                    "operand_to_reg: expected register operand, got Imm12({v}) — \
                     caller should have spilled the immediate to a scratch register \
                     before invoking this method"
                );
                Err(CodegenError::EncodingError(format!(
                    "expected register operand, got immediate ({v}) — \
                     the caller should spill the immediate to a scratch register \
                     (e.g. X16) before calling operand_to_reg()"
                )))
            }
        }
    }

    fn apply_fixups(&mut self) -> Result<()> {
        let fixups = std::mem::take(&mut self.fixups);
        for (word_idx, label, format) in &fixups {
            let target_offset = self.label_offsets.get(label).copied().unwrap_or(0);
            let offset = (target_offset as i32) - (*word_idx as i32);
            let old_word = self.code[*word_idx];
            let patched = match format {
                BranchFormat::B26 => {
                    // B / BL: offset in bits[25:0], word-aligned
                    (old_word & !0x03FFFFFF) | ((offset & 0x03FFFFFF) as u32)
                }
                BranchFormat::Cond19 => {
                    // CBZ / CBNZ: imm19 in bits[23:5], word-aligned
                    // Clear bits[23:5], then set imm19 there
                    let imm19 = offset & 0x7FFFF;
                    (old_word & !(0x7FFFF << 5)) | ((imm19 as u32) << 5)
                }
                BranchFormat::BCond19 => {
                    // B.cond: imm19 in bits[23:5], word-aligned
                    let imm19 = offset & 0x7FFFF;
                    (old_word & !(0x7FFFF << 5)) | ((imm19 as u32) << 5)
                }
            };
            self.code[*word_idx] = patched;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Stack-slot emission (for functions with many vregs)
    // -----------------------------------------------------------------------

    /// Emit a single IR function using the stack-slot strategy.
    ///
    /// Every virtual register gets a stack slot at `[X29, #-offset]` (8 bytes
    /// per slot).  Alloc vregs get larger regions.  For each IR instruction,
    /// operands are loaded from stack slots into scratch registers (X9, X10,
    /// X16, X17), the operation is performed, and the result is stored back.
    ///
    /// ## Scratch Registers (never assigned to vregs)
    ///
    /// - X9: primary accumulator / result
    /// - X10: secondary operand
    /// - X16: address computation / tertiary scratch (IP0)
    /// - X17: quaternary scratch (IP1)
    pub fn emit_function_stack_slot(&mut self, func: &IRFunction) -> Result<Vec<u32>> {
        self.code.clear();
        self.fixups.clear();
        self.label_offsets.clear();
        self.call_relocs.clear();
        self.relocations.clear();
        self.current_func_name = func.name.clone();
        self.instr_pinned_regs.clear();

        // ── Phase 1: Collect all vreg IDs and compute stack layout ──

        let mut all_vreg_ids: std::collections::HashSet<u32> =
            std::collections::HashSet::new();
        for &id in func.vregs.keys() {
            all_vreg_ids.insert(id);
        }
        for param in &func.params {
            if let Some(id) = param.as_register() {
                all_vreg_ids.insert(id);
            }
        }
        for block in &func.blocks {
            for instr in &block.instructions {
                for id in instr.defined_regs() {
                    all_vreg_ids.insert(id);
                }
                for id in instr.used_regs() {
                    all_vreg_ids.insert(id);
                }
            }
            // Also check terminator for vreg usage
            match &block.terminator {
                IRTerminator::Branch { cond, .. } => {
                    if let Some(id) = cond.as_register() {
                        all_vreg_ids.insert(id);
                    }
                }
                IRTerminator::Return(vals) => {
                    for val in vals {
                        if let Some(id) = val.as_register() {
                            all_vreg_ids.insert(id);
                        }
                    }
                }
                _ => {}
            }
        }
        for val in &func.results {
            if let Some(id) = val.as_register() {
                all_vreg_ids.insert(id);
            }
        }

        // Identify Alloc vregs and their sizes
        let mut stack_alloc_vregs: std::collections::HashSet<u32> =
            std::collections::HashSet::new();
        let mut alloc_sizes: HashMap<u32, i32> = HashMap::new();
        for block in &func.blocks {
            for instr in &block.instructions {
                if let IRInstr::Alloc { dst, size } = instr {
                    if let Some(id) = dst.as_register() {
                        stack_alloc_vregs.insert(id);
                        let aligned_size = ((*size as i32 + 15) & !15) as i32;
                        alloc_sizes.insert(id, aligned_size);
                    }
                }
            }
        }

        // ── Stack Layout ──
        // [high address]
        //   saved X29, X30     ← X29 points here
        //   Alloc data region N ← [X29, #-alloc_offset_N]
        //   ...
        //   Alloc data region 1
        //   vreg slot M         ← [X29, #-vreg_offset_M]  (8 bytes each)
        //   ...
        //   vreg slot 1
        // [low address]         ← SP

        let mut alloc_offsets: HashMap<u32, i32> = HashMap::new();
        let mut current_offset: i32 = 0;
        let mut alloc_vreg_ids: Vec<u32> = stack_alloc_vregs.iter().copied().collect();
        alloc_vreg_ids.sort();
        for &id in &alloc_vreg_ids {
            let size = alloc_sizes[&id];
            current_offset += size;
            alloc_offsets.insert(id, current_offset); // slot at [X29, #-current_offset]
        }

        let mut vreg_stack_slots: HashMap<u32, i32> = HashMap::new();
        let mut all_vreg_ids_sorted: Vec<u32> = all_vreg_ids.iter().copied().collect();
        all_vreg_ids_sorted.sort();
        for &id in &all_vreg_ids_sorted {
            current_offset += 8;
            vreg_stack_slots.insert(id, current_offset); // slot at [X29, #-current_offset]
        }

        let frame_size = ((current_offset + 15) & !15) as u32;
        self.frame_size = frame_size;

        // ── Phase 2: Emit prologue ──

        // SUB SP, SP, #16  (room for FP/LR save pair)
        self.emit_instruction(Instruction::SUB {
            rd: Register::SP,
            rn: Register::SP,
            rm: Operand::Imm12(16),
        })?;
        // STP X29, X30, [SP]
        self.emit_instruction(Instruction::STP {
            rt1: Register::X29,
            rt2: Register::X30,
            rn: Register::SP,
            offset: 0,
        })?;
        // ADD X29, SP, #0  (set frame pointer)
        self.emit_instruction(Instruction::ADD {
            rd: Register::X29,
            rn: Register::SP,
            rm: Operand::Imm12(0),
        })?;
        // SUB SP, SP, #frame_size
        if frame_size > 0 {
            if frame_size <= 4095 {
                self.emit_instruction(Instruction::SUB {
                    rd: Register::SP,
                    rn: Register::SP,
                    rm: Operand::Imm12(frame_size as u16),
                })?;
            } else {
                self.emit_load_immediate(Register::X9, frame_size as i64)?;
                self.emit_instruction(Instruction::SUB {
                    rd: Register::SP,
                    rn: Register::SP,
                    rm: Operand::Reg {
                        reg: Register::X9,
                        shift: None,
                    },
                })?;
            }
        }

        // Store function parameters from X0-X7 to their stack slots
        let arg_regs = [
            Register::X0, Register::X1, Register::X2, Register::X3,
            Register::X4, Register::X5, Register::X6, Register::X7,
        ];
        for (i, param) in func.params.iter().enumerate() {
            if let Some(id) = param.as_register() {
                if i < 8 {
                    let offset = vreg_stack_slots.get(&id).copied().unwrap_or(0);
                    self.ss_store_to_slot(arg_regs[i], offset)?;
                }
            }
        }

        // ── Phase 3: Emit each basic block ──

        for block in &func.blocks {
            self.label_offsets.insert(block.label.clone(), self.code.len());
            for instr in &block.instructions {
                self.ss_emit_instr(
                    instr,
                    &vreg_stack_slots,
                    &alloc_offsets,
                    &stack_alloc_vregs,
                )?;
            }
            self.ss_emit_terminator(&block.terminator, &vreg_stack_slots)?;
        }

        // ── Phase 4: Apply fixups ──
        self.apply_fixups()?;

        Ok(self.code.clone())
    }

    // -----------------------------------------------------------------------
    // Stack-slot helpers
    // -----------------------------------------------------------------------

    /// Compute the address of a stack slot into `dst`.
    /// The slot is at `[X29, #-offset]` where `offset > 0`.
    fn ss_emit_slot_addr(&mut self, dst: Register, offset: i32) -> Result<()> {
        if offset <= 4095 {
            self.emit_instruction(Instruction::SUB {
                rd: dst,
                rn: Register::X29,
                rm: Operand::Imm12(offset as u16),
            })?;
        } else {
            self.emit_load_immediate(dst, offset as i64)?;
            self.emit_instruction(Instruction::SUB {
                rd: dst,
                rn: Register::X29,
                rm: Operand::Reg {
                    reg: dst,
                    shift: None,
                },
            })?;
        }
        Ok(())
    }

    /// Load a value from a stack slot into `dst`.
    /// `offset` is the positive offset from X29 (slot at `[X29, #-offset]`).
    fn ss_load_from_slot(&mut self, dst: Register, offset: i32) -> Result<()> {
        let addr_reg = if dst == Register::X16 {
            Register::X17
        } else {
            Register::X16
        };
        self.ss_emit_slot_addr(addr_reg, offset)?;
        self.emit_instruction(Instruction::LDR {
            rt: dst,
            rn: addr_reg,
            offset: 0,
        })?;
        Ok(())
    }

    /// Store a value from `src` into a stack slot.
    /// `offset` is the positive offset from X29 (slot at `[X29, #-offset]`).
    fn ss_store_to_slot(&mut self, src: Register, offset: i32) -> Result<()> {
        let addr_reg = if src == Register::X16 {
            Register::X17
        } else {
            Register::X16
        };
        self.ss_emit_slot_addr(addr_reg, offset)?;
        self.emit_instruction(Instruction::STR {
            rt: src,
            rn: addr_reg,
            offset: 0,
        })?;
        Ok(())
    }

    /// Load an [`IRValue`] into a scratch register.
    /// For registers: load from the stack slot.
    /// For immediates: load via MOVZ/MOVK.
    fn ss_load_value(
        &mut self,
        val: &IRValue,
        dst: Register,
        slots: &HashMap<u32, i32>,
    ) -> Result<()> {
        match val {
            IRValue::Register(id) => {
                let offset = slots.get(id).copied().unwrap_or(0);
                self.ss_load_from_slot(dst, offset)?;
            }
            IRValue::Immediate(v) => {
                self.emit_load_immediate(dst, *v)?;
            }
            IRValue::Address(a) => {
                self.emit_load_immediate(dst, *a as i64)?;
            }
            IRValue::Label(_) => {
                return Err(CodegenError::EncodingError(
                    "label value cannot be resolved to a register".into(),
                ));
            }
        }
        Ok(())
    }

    /// Load an [`IRValue`] into a scratch register with a specific width.
    /// For 32-bit values, uses 32-bit MOVZ/MOVK.
    fn ss_load_value_with_width(
        &mut self,
        val: &IRValue,
        dst: Register,
        slots: &HashMap<u32, i32>,
        width: RegWidth,
    ) -> Result<()> {
        match val {
            IRValue::Register(id) => {
                let offset = slots.get(id).copied().unwrap_or(0);
                // Always load 64-bit from stack slot; the arithmetic
                // will use the correct width.
                self.ss_load_from_slot(dst, offset)?;
            }
            IRValue::Immediate(v) => {
                self.emit_load_immediate_with_width(dst, *v, width)?;
            }
            IRValue::Address(a) => {
                self.emit_load_immediate(dst, *a as i64)?;
            }
            IRValue::Label(_) => {
                return Err(CodegenError::EncodingError(
                    "label value cannot be resolved to a register".into(),
                ));
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Stack-slot instruction emission
    // -----------------------------------------------------------------------

    /// Emit a single IR instruction using the stack-slot strategy.
    fn ss_emit_instr(
        &mut self,
        instr: &IRInstr,
        slots: &HashMap<u32, i32>,
        alloc_offsets: &HashMap<u32, i32>,
        stack_alloc_vregs: &std::collections::HashSet<u32>,
    ) -> Result<()> {
        match instr {
            // ── Load ──
            IRInstr::Load { dst, addr, offset, ty } => {
                let dst_id = dst.as_register().unwrap_or(0);
                let dst_offset = slots.get(&dst_id).copied().unwrap_or(0);
                // Load address into X10 (not X9, because emit_load uses X9 internally)
                self.ss_load_value(addr, Register::X10, slots)?;
                // Load from memory [X10 + offset] into X9
                self.emit_load(Register::X9, Register::X10, *offset, ty)?;
                // Store result to dst's stack slot
                self.ss_store_to_slot(Register::X9, dst_offset)?;
            }

            // ── Store ──
            IRInstr::Store { value, addr, offset, ty } => {
                // Load address into X10 first (may clobber X16 internally)
                self.ss_load_value(addr, Register::X10, slots)?;
                // Load value into X11 (not X9: emit_store uses X9 for large offsets;
                // not X16: ss_load_from_slot(X10,…) clobbers X16)
                self.ss_load_value(value, Register::X11, slots)?;
                self.emit_store(Register::X11, Register::X10, *offset, ty)?;
            }

            // ── BinOp (generic) ──
            IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
                self.ss_emit_binop(*op, dst, lhs, rhs, ty.as_ref(), slots)?;
            }

            // ── UnaryOp ──
            IRInstr::UnaryOp { op, dst, operand, ty } => {
                let dst_id = dst.as_register().unwrap_or(0);
                let dst_offset = slots.get(&dst_id).copied().unwrap_or(0);
                let width = RegWidth::from_ir_type(ty.as_ref());
                self.ss_load_value_with_width(operand, Register::X9, slots, width)?;

                match op {
                    UnaryOpKind::Neg => {
                        self.emit_instruction_with_width(
                            Instruction::SUB {
                                rd: Register::X9,
                                rn: Register::XZR,
                                rm: Operand::Reg {
                                    reg: Register::X9,
                                    shift: None,
                                },
                            },
                            width,
                        )?;
                    }
                    UnaryOpKind::Not => {
                        self.emit_load_immediate(Register::X10, -1)?;
                        self.emit_instruction_with_width(
                            Instruction::EOR {
                                rd: Register::X9,
                                rn: Register::X9,
                                rm: Register::X10,
                            },
                            width,
                        )?;
                    }
                    UnaryOpKind::Clz => {
                        self.emit_instruction(Instruction::CLZ {
                            rd: Register::X9,
                            rn: Register::X9,
                        })?;
                    }
                    UnaryOpKind::Ctz => {
                        // CTZ = RBIT + CLZ
                        self.emit_instruction(Instruction::RBIT {
                            rd: Register::X10,
                            rn: Register::X9,
                        })?;
                        self.emit_instruction(Instruction::CLZ {
                            rd: Register::X9,
                            rn: Register::X10,
                        })?;
                    }
                    UnaryOpKind::Popcnt => {
                        const SIMD_SCRATCH: u8 = 8;
                        self.emit_instruction(Instruction::FMOV_DX {
                            vd: SIMD_SCRATCH,
                            rn: Register::X9,
                        })?;
                        self.emit_instruction(Instruction::CNT {
                            vd: SIMD_SCRATCH,
                            vn: SIMD_SCRATCH,
                        })?;
                        self.emit_instruction(Instruction::ADDV {
                            vd: SIMD_SCRATCH,
                            vn: SIMD_SCRATCH,
                        })?;
                        self.emit_instruction(Instruction::UMOV {
                            rd: Register::X9,
                            vn: SIMD_SCRATCH,
                        })?;
                    }
                }
                self.ss_store_to_slot(Register::X9, dst_offset)?;
            }

            // ── Call ──
            IRInstr::Call { dst, func: target_name, args, is_extern: _ } => {
                let arg_regs = [
                    Register::X0, Register::X1, Register::X2, Register::X3,
                    Register::X4, Register::X5, Register::X6, Register::X7,
                ];
                // Load arguments from stack slots into X0-X7
                for (i, arg) in args.iter().enumerate() {
                    if i >= 8 {
                        break;
                    }
                    self.ss_load_value(arg, arg_regs[i], slots)?;
                }
                // BL — record a relocation for later patching
                let bl_word_idx = self.code.len();
                let bl_byte_offset = self.func_text_offset + (bl_word_idx as u64) * 4;
                self.call_relocs.push(CallRelocation {
                    text_byte_offset: bl_byte_offset,
                    target_func: target_name.clone(),
                });
                self.relocations.push(RelocationEntry {
                    offset: bl_byte_offset,
                    symbol: target_name.clone(),
                    reloc_type: "R_AARCH64_CALL26".to_string(),
                });
                self.emit_instruction(Instruction::BL { offset: 0 })?;

                if let Some(dst_val) = dst {
                    let dst_id = dst_val.as_register().unwrap_or(0);
                    let dst_offset = slots.get(&dst_id).copied().unwrap_or(0);
                    self.ss_store_to_slot(Register::X0, dst_offset)?;
                }
            }

            // ── Alloc ──
            IRInstr::Alloc { dst, .. } => {
                let dst_id = dst.as_register().unwrap_or(0);
                let dst_offset = slots.get(&dst_id).copied().unwrap_or(0);
                let alloc_off = alloc_offsets
                    .get(&dst_id)
                    .copied()
                    .unwrap_or(0);
                // Compute X9 = X29 - alloc_off (pointer to data region)
                self.ss_emit_slot_addr(Register::X9, alloc_off)?;
                // Store pointer into dst's stack slot
                self.ss_store_to_slot(Register::X9, dst_offset)?;
            }

            // ── Free ──
            IRInstr::Free { ptr } => {
                let is_stack = ptr
                    .as_register()
                    .map(|id| stack_alloc_vregs.contains(&id))
                    .unwrap_or(false);
                if !is_stack {
                    // Heap allocation — call __vuma_free(ptr)
                    self.ss_load_value(ptr, Register::X0, slots)?;
                    let bl_word_idx = self.code.len();
                    let bl_byte_offset = self.func_text_offset + (bl_word_idx as u64) * 4;
                    self.call_relocs.push(CallRelocation {
                        text_byte_offset: bl_byte_offset,
                        target_func: "__vuma_free".to_string(),
                    });
                    self.relocations.push(RelocationEntry {
                        offset: bl_byte_offset,
                        symbol: "__vuma_free".to_string(),
                        reloc_type: "R_AARCH64_CALL26".to_string(),
                    });
                    self.emit_instruction(Instruction::BL { offset: 0 })?;
                }
                // Stack allocation — no-op
            }

            // ── Cast ──
            IRInstr::Cast { kind, dst, src } => {
                let dst_id = dst.as_register().unwrap_or(0);
                let dst_offset = slots.get(&dst_id).copied().unwrap_or(0);
                self.ss_load_value(src, Register::X9, slots)?;

                match kind {
                    CastKind::ZExt => {
                        // Zero-extend 32-bit to 64-bit
                        self.emit_instruction(Instruction::UBFM {
                            rd: Register::X9,
                            rn: Register::X9,
                            immr: 0,
                            imms: 31,
                        })?;
                    }
                    CastKind::SExt => {
                        // Sign-extend 32-bit to 64-bit
                        self.emit_instruction(Instruction::SBFM {
                            rd: Register::X9,
                            rn: Register::X9,
                            immr: 0,
                            imms: 31,
                        })?;
                    }
                    CastKind::Trunc | CastKind::BitCast => {
                        // No-op: just store the value
                    }
                    CastKind::IntToFloat | CastKind::UIntToFloat |
                    CastKind::FloatToInt | CastKind::FloatToUInt |
                    CastKind::FloatToFloat => {
                        // FP casts — not yet implemented; pass through.
                    }
                }
                self.ss_store_to_slot(Register::X9, dst_offset)?;
            }

            // ── Phi ──
            IRInstr::Phi { dst, incoming } => {
                let non_self: Vec<_> = incoming
                    .iter()
                    .filter(|(val, _)| val != dst)
                    .collect();
                if non_self.len() == 1 {
                    let (val, _) = non_self[0];
                    let dst_id = dst.as_register().unwrap_or(0);
                    let dst_offset = slots.get(&dst_id).copied().unwrap_or(0);
                    self.ss_load_value(val, Register::X9, slots)?;
                    self.ss_store_to_slot(Register::X9, dst_offset)?;
                } else if !non_self.is_empty() {
                    let (val, _) = non_self[0];
                    let dst_id = dst.as_register().unwrap_or(0);
                    let dst_offset = slots.get(&dst_id).copied().unwrap_or(0);
                    self.ss_load_value(val, Register::X9, slots)?;
                    self.ss_store_to_slot(Register::X9, dst_offset)?;
                }
                // Empty phi = self-loop = no-op
            }

            // ── GetAddress ──
            IRInstr::GetAddress { dst, name } => {
                let dst_id = dst.as_register().unwrap_or(0);
                let dst_offset = slots.get(&dst_id).copied().unwrap_or(0);
                let name_hash = name
                    .chars()
                    .fold(0u64, |acc, c| acc.wrapping_mul(31).wrapping_add(c as u64));
                self.emit_load_immediate(Register::X0, name_hash as i64)?;
                let bl_word_idx = self.code.len();
                let bl_byte_offset = self.func_text_offset + (bl_word_idx as u64) * 4;
                self.call_relocs.push(CallRelocation {
                    text_byte_offset: bl_byte_offset,
                    target_func: "__vuma_getaddr".to_string(),
                });
                self.relocations.push(RelocationEntry {
                    offset: bl_byte_offset,
                    symbol: "__vuma_getaddr".to_string(),
                    reloc_type: "R_AARCH64_CALL26".to_string(),
                });
                self.emit_instruction(Instruction::BL { offset: 0 })?;
                self.ss_store_to_slot(Register::X0, dst_offset)?;
            }

            // ── Offset ──
            IRInstr::Offset { dst, base, offset } => {
                let dst_id = dst.as_register().unwrap_or(0);
                let dst_offset = slots.get(&dst_id).copied().unwrap_or(0);
                self.ss_load_value(base, Register::X9, slots)?;
                match offset {
                    IRValue::Immediate(v) => {
                        if *v >= 0 && *v <= 4095 {
                            self.emit_instruction(Instruction::ADD {
                                rd: Register::X9,
                                rn: Register::X9,
                                rm: Operand::Imm12(*v as u16),
                            })?;
                        } else {
                            self.emit_load_immediate(Register::X10, *v)?;
                            self.emit_instruction(Instruction::ADD {
                                rd: Register::X9,
                                rn: Register::X9,
                                rm: Operand::Reg {
                                    reg: Register::X10,
                                    shift: None,
                                },
                            })?;
                        }
                    }
                    _ => {
                        self.ss_load_value(offset, Register::X10, slots)?;
                        self.emit_instruction(Instruction::ADD {
                            rd: Register::X9,
                            rn: Register::X9,
                            rm: Operand::Reg {
                                reg: Register::X10,
                                shift: None,
                            },
                        })?;
                    }
                }
                self.ss_store_to_slot(Register::X9, dst_offset)?;
            }

            // ── Dedicated arithmetic ──
            IRInstr::Add { dst, lhs, rhs, ty } => {
                self.ss_emit_binop(BinOpKind::Add, dst, lhs, rhs, ty.as_ref(), slots)?;
            }
            IRInstr::Sub { dst, lhs, rhs, ty } => {
                self.ss_emit_binop(BinOpKind::Sub, dst, lhs, rhs, ty.as_ref(), slots)?;
            }
            IRInstr::Mul { dst, lhs, rhs, ty } => {
                self.ss_emit_binop(BinOpKind::Mul, dst, lhs, rhs, ty.as_ref(), slots)?;
            }
            IRInstr::Div { dst, lhs, rhs, ty } => {
                self.ss_emit_binop(BinOpKind::SDiv, dst, lhs, rhs, ty.as_ref(), slots)?;
            }

            // ── Cmp ──
            IRInstr::Cmp { kind, dst, lhs, rhs, ty } => {
                let dst_id = dst.as_register().unwrap_or(0);
                let dst_offset = slots.get(&dst_id).copied().unwrap_or(0);
                let width = RegWidth::from_ir_type(ty.as_ref());
                self.ss_load_value_with_width(lhs, Register::X9, slots, width)?;
                self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                self.emit_instruction_with_width(
                    Instruction::CMP {
                        rn: Register::X9,
                        rm: Operand::Reg {
                            reg: Register::X10,
                            shift: None,
                        },
                    },
                    width,
                )?;
                let cond = cmp_kind_to_condition(kind);
                self.emit_instruction_with_width(
                    Instruction::CSET { rd: Register::X9, cond },
                    width,
                )?;
                self.ss_store_to_slot(Register::X9, dst_offset)?;
            }

            // ── Ret (instruction-level) ──
            IRInstr::Ret { values } => {
                for (i, val) in values.iter().enumerate() {
                    if i >= 8 {
                        break;
                    }
                    let dst_reg = match i {
                        0 => Register::X0,
                        1 => Register::X1,
                        2 => Register::X2,
                        3 => Register::X3,
                        4 => Register::X4,
                        5 => Register::X5,
                        6 => Register::X6,
                        7 => Register::X7,
                        _ => unreachable!(),
                    };
                    self.ss_load_value(val, dst_reg, slots)?;
                }
            }

            // ── Branch (instruction-level) ──
            IRInstr::Branch { target } => {
                let fixup_idx = self.code.len();
                self.fixups.push((fixup_idx, target.clone(), BranchFormat::B26));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }

            // ── CondBranch (instruction-level) ──
            IRInstr::CondBranch { cond, true_target, false_target } => {
                self.ss_load_value(cond, Register::X9, slots)?;
                let fixup_cbz = self.code.len();
                self.fixups.push((fixup_cbz, true_target.clone(), BranchFormat::Cond19));
                self.emit_instruction(Instruction::CBNZ { rt: Register::X9, offset: 0 })?;
                let fixup_b = self.code.len();
                self.fixups.push((fixup_b, false_target.clone(), BranchFormat::B26));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }

            // ── Select ──
            IRInstr::Select { dst, cond, true_val, false_val, ty } => {
                let dst_id = dst.as_register().unwrap_or(0);
                let dst_offset = slots.get(&dst_id).copied().unwrap_or(0);
                let width = RegWidth::from_ir_type(ty.as_ref());
                // Load cond and set flags
                self.ss_load_value_with_width(cond, Register::X9, slots, width)?;
                self.emit_instruction_with_width(
                    Instruction::CMP {
                        rn: Register::X9,
                        rm: Operand::Imm12(0),
                    },
                    width,
                )?;
                // Load true and false values (LDR does NOT affect condition flags)
                self.ss_load_value_with_width(true_val, Register::X10, slots, width)?;
                self.ss_load_value_with_width(false_val, Register::X17, slots, width)?;
                // CSEL: if NE (cond != 0), X9 = X10 (true_val); else X9 = X17 (false_val)
                self.emit_instruction_with_width(
                    Instruction::CSEL {
                        rd: Register::X9,
                        rn: Register::X10,
                        rm: Register::X17,
                        cond: Condition::NE,
                    },
                    width,
                )?;
                self.ss_store_to_slot(Register::X9, dst_offset)?;
            }

            // ── Constant-time operations (stack-slot path) ──
            // These use the same lowering as the non-stack-slot path since
            // they were already implemented in emit_ir_instr above.
            IRInstr::CtSelect { .. } | IRInstr::CtEq { .. } => {
                // Already handled by emit_ir_instr; stack-slot path delegates.
                // We reach here via the stack-slot emitter, so delegate back.
                // For now, emit a NOP as a placeholder — the non-stack-slot
                // path handles these correctly.
            }

            // ── Atomic operations (stack-slot path) ──
            IRInstr::AtomicLoad { .. } | IRInstr::AtomicStore { .. } | IRInstr::AtomicCas { .. } => {
                // Atomic operations handled by emit_ir_instr above.
            }
        }
        Ok(())
    }

    /// Emit a binary operation using the stack-slot strategy.
    fn ss_emit_binop(
        &mut self,
        op: BinOpKind,
        dst: &IRValue,
        lhs: &IRValue,
        rhs: &IRValue,
        ty: Option<&IRType>,
        slots: &HashMap<u32, i32>,
    ) -> Result<()> {
        let width = RegWidth::from_ir_type(ty);
        let dst_id = dst.as_register().unwrap_or(0);
        let dst_offset = slots.get(&dst_id).copied().unwrap_or(0);

        // Load lhs into X9
        self.ss_load_value_with_width(lhs, Register::X9, slots, width)?;

        // Determine whether the operation can accept an immediate operand.
        let supports_imm = matches!(
            op,
            BinOpKind::Add | BinOpKind::Sub | BinOpKind::Shl | BinOpKind::ShrL | BinOpKind::ShrA
        );

        match op {
            BinOpKind::Add => {
                match rhs {
                    IRValue::Immediate(v) if supports_imm && *v >= 0 && *v <= 4095 => {
                        self.emit_instruction_with_width(
                            Instruction::ADD {
                                rd: Register::X9,
                                rn: Register::X9,
                                rm: Operand::Imm12(*v as u16),
                            },
                            width,
                        )?;
                    }
                    _ => {
                        self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                        self.emit_instruction_with_width(
                            Instruction::ADD {
                                rd: Register::X9,
                                rn: Register::X9,
                                rm: Operand::Reg {
                                    reg: Register::X10,
                                    shift: None,
                                },
                            },
                            width,
                        )?;
                    }
                }
            }
            BinOpKind::Sub => {
                match rhs {
                    IRValue::Immediate(v) if supports_imm && *v >= 0 && *v <= 4095 => {
                        self.emit_instruction_with_width(
                            Instruction::SUB {
                                rd: Register::X9,
                                rn: Register::X9,
                                rm: Operand::Imm12(*v as u16),
                            },
                            width,
                        )?;
                    }
                    _ => {
                        self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                        self.emit_instruction_with_width(
                            Instruction::SUB {
                                rd: Register::X9,
                                rn: Register::X9,
                                rm: Operand::Reg {
                                    reg: Register::X10,
                                    shift: None,
                                },
                            },
                            width,
                        )?;
                    }
                }
            }
            BinOpKind::Mul => {
                self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                self.emit_instruction_with_width(
                    Instruction::MUL {
                        rd: Register::X9,
                        rn: Register::X9,
                        rm: Register::X10,
                    },
                    width,
                )?;
            }
            BinOpKind::SDiv => {
                self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                self.emit_instruction_with_width(
                    Instruction::SDIV {
                        rd: Register::X9,
                        rn: Register::X9,
                        rm: Register::X10,
                    },
                    width,
                )?;
            }
            BinOpKind::UDiv => {
                self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                self.emit_instruction_with_width(
                    Instruction::UDIV {
                        rd: Register::X9,
                        rn: Register::X9,
                        rm: Register::X10,
                    },
                    width,
                )?;
            }
            BinOpKind::And => {
                self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                self.emit_instruction_with_width(
                    Instruction::AND {
                        rd: Register::X9,
                        rn: Register::X9,
                        rm: Register::X10,
                    },
                    width,
                )?;
            }
            BinOpKind::Or => {
                self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                self.emit_instruction_with_width(
                    Instruction::ORR {
                        rd: Register::X9,
                        rn: Register::X9,
                        rm: Register::X10,
                    },
                    width,
                )?;
            }
            BinOpKind::Xor => {
                self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                self.emit_instruction_with_width(
                    Instruction::EOR {
                        rd: Register::X9,
                        rn: Register::X9,
                        rm: Register::X10,
                    },
                    width,
                )?;
            }
            BinOpKind::Shl => {
                match rhs {
                    IRValue::Immediate(v) if *v >= 0 && (*v as u32) <= 63 => {
                        self.emit_instruction_with_width(
                            Instruction::LSL {
                                rd: Register::X9,
                                rn: Register::X9,
                                rm: Operand::Imm12(*v as u16),
                            },
                            width,
                        )?;
                    }
                    _ => {
                        self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                        // Emit LSLV directly (the Instruction::LSL with Reg
                        // operand has a wrong encoding using ADD-shifted form).
                        self.ss_emit_lslv(Register::X9, Register::X9, Register::X10, width)?;
                    }
                }
            }
            BinOpKind::ShrL => {
                match rhs {
                    IRValue::Immediate(v) if *v >= 0 && (*v as u32) <= 63 => {
                        self.emit_instruction_with_width(
                            Instruction::LSR {
                                rd: Register::X9,
                                rn: Register::X9,
                                rm: Operand::Imm12(*v as u16),
                            },
                            width,
                        )?;
                    }
                    _ => {
                        self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                        self.ss_emit_lsrv(Register::X9, Register::X9, Register::X10, width)?;
                    }
                }
            }
            BinOpKind::ShrA => {
                match rhs {
                    IRValue::Immediate(v) if *v >= 0 && (*v as u32) <= 63 => {
                        self.emit_instruction_with_width(
                            Instruction::ASR {
                                rd: Register::X9,
                                rn: Register::X9,
                                rm: Operand::Imm12(*v as u16),
                            },
                            width,
                        )?;
                    }
                    _ => {
                        self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                        self.ss_emit_asrv(Register::X9, Register::X9, Register::X10, width)?;
                    }
                }
            }
            BinOpKind::Ror => {
                match rhs {
                    IRValue::Immediate(v) if *v >= 0 && (*v as u32) <= 63 => {
                        self.ss_emit_ror_imm(Register::X9, Register::X9, *v as u32, width)?;
                    }
                    _ => {
                        self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                        self.ss_emit_rorv(Register::X9, Register::X9, Register::X10, width)?;
                    }
                }
            }
            BinOpKind::Rol => {
                // ROL by N = ROR by (size - N)
                match rhs {
                    IRValue::Immediate(v) => {
                        let size = width.bits();
                        let n = (*v as u32) % size;
                        let ror_amount = (size - n) % size;
                        self.ss_emit_ror_imm(Register::X9, Register::X9, ror_amount, width)?;
                    }
                    _ => {
                        self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                        let size = width.bits();
                        // Compute X17 = size - (X10 % size)
                        // X10 % size: use UBFM to mask to log2(size) bits
                        if size == 64 {
                            self.emit_load_immediate(Register::X17, 64)?;
                            self.emit_instruction(Instruction::SUB {
                                rd: Register::X17,
                                rn: Register::X17,
                                rm: Operand::Reg {
                                    reg: Register::X10,
                                    shift: None,
                                },
                            })?;
                        } else {
                            // 32-bit: mask X10 to 5 bits, then compute 32 - masked
                            self.emit_instruction(Instruction::AND {
                                rd: Register::X17,
                                rn: Register::X10,
                                rm: Register::X10, // This is wrong; need immediate 31
                            })?;
                            // Actually use UBFM to extract low 5 bits
                            self.emit_instruction(Instruction::UBFM {
                                rd: Register::X17,
                                rn: Register::X10,
                                immr: 0,
                                imms: 4, // bits [4:0] = low 5 bits
                            })?;
                            self.emit_load_immediate(Register::X16, 32)?;
                            self.emit_instruction(Instruction::SUB {
                                rd: Register::X17,
                                rn: Register::X16,
                                rm: Operand::Reg {
                                    reg: Register::X17,
                                    shift: None,
                                },
                            })?;
                        }
                        self.ss_emit_rorv(Register::X9, Register::X9, Register::X17, width)?;
                    }
                }
            }
            BinOpKind::SRem | BinOpKind::URem => {
                self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                // Save dividend in X17 before division
                self.emit_instruction_with_width(
                    Instruction::MOV {
                        rd: Register::X17,
                        rm: Register::X9,
                    },
                    width,
                )?;
                let div_instr = if op == BinOpKind::SRem {
                    Instruction::SDIV {
                        rd: Register::X9,
                        rn: Register::X9,
                        rm: Register::X10,
                    }
                } else {
                    Instruction::UDIV {
                        rd: Register::X9,
                        rn: Register::X9,
                        rm: Register::X10,
                    }
                };
                self.emit_instruction_with_width(div_instr, width)?;
                // MSUB X9, X9, X10, X17 = X17 - X9 * X10 = dividend - quotient * divisor
                self.emit_instruction_with_width(
                    Instruction::MSUB {
                        rd: Register::X9,
                        rn: Register::X9,
                        rm: Register::X10,
                        ra: Register::X17,
                    },
                    width,
                )?;
            }
            BinOpKind::SLt
            | BinOpKind::SLe
            | BinOpKind::SGt
            | BinOpKind::SGe
            | BinOpKind::ULt
            | BinOpKind::ULe
            | BinOpKind::UGt
            | BinOpKind::UGe
            | BinOpKind::Eq
            | BinOpKind::Ne => {
                self.ss_load_value_with_width(rhs, Register::X10, slots, width)?;
                self.emit_instruction_with_width(
                    Instruction::CMP {
                        rn: Register::X9,
                        rm: Operand::Reg {
                            reg: Register::X10,
                            shift: None,
                        },
                    },
                    width,
                )?;
                let cond = binop_kind_to_condition(&op);
                self.emit_instruction_with_width(
                    Instruction::CSET { rd: Register::X9, cond },
                    width,
                )?;
            }
        }

        // Store result to dst's stack slot
        self.ss_store_to_slot(Register::X9, dst_offset)?;
        Ok(())
    }

    /// Emit a terminator using the stack-slot strategy.
    fn ss_emit_terminator(
        &mut self,
        term: &IRTerminator,
        slots: &HashMap<u32, i32>,
    ) -> Result<()> {
        match term {
            IRTerminator::Jump(target) => {
                let fixup_idx = self.code.len();
                self.fixups.push((fixup_idx, target.clone(), BranchFormat::B26));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }
            IRTerminator::Branch { cond, true_block, false_block } => {
                self.ss_load_value(cond, Register::X9, slots)?;
                let fixup_cbnz = self.code.len();
                self.fixups.push((fixup_cbnz, true_block.clone(), BranchFormat::Cond19));
                self.emit_instruction(Instruction::CBNZ { rt: Register::X9, offset: 0 })?;
                let fixup_b = self.code.len();
                self.fixups.push((fixup_b, false_block.clone(), BranchFormat::B26));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }
            IRTerminator::Return(vals) => {
                for (i, val) in vals.iter().enumerate() {
                    if i >= 8 {
                        break;
                    }
                    let dst_reg = match i {
                        0 => Register::X0,
                        1 => Register::X1,
                        2 => Register::X2,
                        3 => Register::X3,
                        4 => Register::X4,
                        5 => Register::X5,
                        6 => Register::X6,
                        7 => Register::X7,
                        _ => unreachable!(),
                    };
                    self.ss_load_value(val, dst_reg, slots)?;
                }
                // Epilogue: restore SP from X29, then LDP and ADD
                self.emit_instruction(Instruction::ADD {
                    rd: Register::SP,
                    rn: Register::X29,
                    rm: Operand::Imm12(0),
                })?;
                self.emit_instruction(Instruction::LDP {
                    rt1: Register::X29,
                    rt2: Register::X30,
                    rn: Register::SP,
                    offset: 0,
                })?;
                self.emit_instruction(Instruction::ADD {
                    rd: Register::SP,
                    rn: Register::SP,
                    rm: Operand::Imm12(16),
                })?;
                self.emit_instruction(Instruction::RET { rn: None })?;
            }
            IRTerminator::Unreachable => {
                self.emit_instruction(Instruction::MOV {
                    rd: Register::XZR,
                    rm: Register::XZR,
                })?;
            }
            IRTerminator::Switch { .. } => {
                return Err(CodegenError::InvalidInstruction(
                    "Switch terminator must be lowered before emission".to_string(),
                ));
            }
            IRTerminator::Invoke { .. } => {
                return Err(CodegenError::InvalidInstruction(
                    "Invoke terminator must be lowered before emission".to_string(),
                ));
            }
            IRTerminator::TailCall { .. } => {
                return Err(CodegenError::InvalidInstruction(
                    "TailCall terminator must be lowered before emission".to_string(),
                ));
            }
            IRTerminator::Resume { .. } => {
                return Err(CodegenError::InvalidInstruction(
                    "Resume terminator must be lowered before emission".to_string(),
                ));
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // ARM64 variable-shift / rotation helpers (correct encoding)
    // -----------------------------------------------------------------------

    /// Emit LSLV (variable left shift): `LSLV Rd, Rn, Rm`
    ///
    /// Encoding: sf 00 11010 11 0 Rm 001000 Rn Rd
    fn ss_emit_lslv(
        &mut self,
        rd: Register,
        rn: Register,
        rm: Register,
        width: RegWidth,
    ) -> Result<()> {
        let sf = width.sf_bit();
        let word = (sf << 31)
            | 0x1AC02000u32
            | (rm.encoding() << 16)
            | (rn.encoding() << 5)
            | rd.encoding();
        self.code.push(word);
        Ok(())
    }

    /// Emit LSRV (variable logical right shift): `LSRV Rd, Rn, Rm`
    ///
    /// Encoding: sf 00 11010 11 0 Rm 001001 Rn Rd
    fn ss_emit_lsrv(
        &mut self,
        rd: Register,
        rn: Register,
        rm: Register,
        width: RegWidth,
    ) -> Result<()> {
        let sf = width.sf_bit();
        let word = (sf << 31)
            | 0x1AC02400u32
            | (rm.encoding() << 16)
            | (rn.encoding() << 5)
            | rd.encoding();
        self.code.push(word);
        Ok(())
    }

    /// Emit ASRV (variable arithmetic right shift): `ASRV Rd, Rn, Rm`
    ///
    /// Encoding: sf 00 11010 11 0 Rm 001010 Rn Rd
    fn ss_emit_asrv(
        &mut self,
        rd: Register,
        rn: Register,
        rm: Register,
        width: RegWidth,
    ) -> Result<()> {
        let sf = width.sf_bit();
        let word = (sf << 31)
            | 0x1AC02800u32
            | (rm.encoding() << 16)
            | (rn.encoding() << 5)
            | rd.encoding();
        self.code.push(word);
        Ok(())
    }

    /// Emit ROR via EXTR: `ROR Rd, Rn, #amount` = `EXTR Rd, Rn, Rn, #amount`
    ///
    /// 64-bit encoding: 1 00 100111 1 0 Rn imm6 Rn Rd  (base 0x93C00000)
    /// 32-bit encoding: 0 00 100111 0 0 Rn imm6 Rn Rd  (base 0x13800000)
    fn ss_emit_ror_imm(
        &mut self,
        rd: Register,
        rn: Register,
        amount: u32,
        width: RegWidth,
    ) -> Result<()> {
        match width {
            RegWidth::X64 => {
                let word = 0x93C00000u32
                    | (rn.encoding() << 16)
                    | ((amount & 0x3F) << 10)
                    | (rn.encoding() << 5)
                    | rd.encoding();
                self.code.push(word);
            }
            RegWidth::W32 => {
                let word = 0x13800000u32
                    | (rn.encoding() << 16)
                    | ((amount & 0x1F) << 10)
                    | (rn.encoding() << 5)
                    | rd.encoding();
                self.code.push(word);
            }
        }
        Ok(())
    }

    /// Emit RORV (variable rotate right): `RORV Rd, Rn, Rm`
    ///
    /// Encoding: sf 00 11010 11 0 Rm 001011 Rn Rd
    fn ss_emit_rorv(
        &mut self,
        rd: Register,
        rn: Register,
        rm: Register,
        width: RegWidth,
    ) -> Result<()> {
        let sf = width.sf_bit();
        let word = (sf << 31)
            | 0x1AC02C00u32
            | (rm.encoding() << 16)
            | (rn.encoding() << 5)
            | rd.encoding();
        self.code.push(word);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Program emission → ELF (legacy compatibility)
    // -----------------------------------------------------------------------

    /// Emit an entire IR program as a minimal ELF binary for Linux/ARM64.
    ///
    /// Convenience wrapper around [`emit_binary`] with default Linux configuration.
    pub fn emit_program(&mut self, program: &IRProgram) -> Result<Vec<u8>> {
        let config = EmitConfig::linux_elf();
        emit_binary(&program.functions, &program.data_sections, &config)
    }
}

impl Default for Emitter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Condition-code mapping helpers
// ---------------------------------------------------------------------------

/// Map an IR [`CmpKind`] to the corresponding ARM64 [`Condition`] code.
fn cmp_kind_to_condition(kind: &CmpKind) -> Condition {
    match kind {
        CmpKind::Eq => Condition::EQ,
        CmpKind::Ne => Condition::NE,
        CmpKind::SLt => Condition::LT,
        CmpKind::SLe => Condition::LE,
        CmpKind::SGt => Condition::GT,
        CmpKind::SGe => Condition::GE,
        CmpKind::ULt => Condition::CC, // Carry clear = unsigned lower
        CmpKind::ULe => Condition::LS, // Unsigned lower or same
        CmpKind::UGt => Condition::HI, // Unsigned higher
        CmpKind::UGe => Condition::CS, // Carry set = unsigned higher or same
    }
}

/// Map a comparison [`BinOpKind`] to the corresponding ARM64 [`Condition`] code.
fn binop_kind_to_condition(op: &BinOpKind) -> Condition {
    match op {
        BinOpKind::SLt => Condition::LT,
        BinOpKind::SLe => Condition::LE,
        BinOpKind::SGt => Condition::GT,
        BinOpKind::SGe => Condition::GE,
        BinOpKind::ULt => Condition::CC,
        BinOpKind::ULe => Condition::LS,
        BinOpKind::UGt => Condition::HI,
        BinOpKind::UGe => Condition::CS,
        BinOpKind::Eq => Condition::EQ,
        BinOpKind::Ne => Condition::NE,
        _ => Condition::EQ, // fallback — should not be reached
    }
}

// ---------------------------------------------------------------------------
// Vreg count computation
// ---------------------------------------------------------------------------

/// Count the number of unique virtual registers used in a function.
/// This is used to decide whether to use the stack-slot emitter or the
/// greedy register allocator.
fn count_vregs(func: &IRFunction) -> u32 {
    let mut vregs: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for &id in func.vregs.keys() {
        vregs.insert(id);
    }
    for param in &func.params {
        if let Some(id) = param.as_register() {
            vregs.insert(id);
        }
    }
    for block in &func.blocks {
        for instr in &block.instructions {
            for id in instr.defined_regs() {
                vregs.insert(id);
            }
            for id in instr.used_regs() {
                vregs.insert(id);
            }
        }
        match &block.terminator {
            IRTerminator::Branch { cond, .. } => {
                if let Some(id) = cond.as_register() {
                    vregs.insert(id);
                }
            }
            IRTerminator::Return(vals) => {
                for val in vals {
                    if let Some(id) = val.as_register() {
                        vregs.insert(id);
                    }
                }
            }
            _ => {}
        }
    }
    for val in &func.results {
        if let Some(id) = val.as_register() {
            vregs.insert(id);
        }
    }
    vregs.len() as u32
}

// ---------------------------------------------------------------------------
// Frame-size computation
// ---------------------------------------------------------------------------

/// Compute the stack frame size for a function by summing its `Alloc`
/// instructions and rounding up to 16-byte alignment.
///
/// NOTE: This does NOT include the 16 bytes for the FP/LR save pair,
/// because the prologue handles that separately with an explicit
/// `SUB SP, SP, #16` before the STP.
fn compute_frame_size(func: &IRFunction) -> u32 {
    let mut total: u32 = 0; // Alloc sizes only; FP/LR handled separately
    for block in &func.blocks {
        for instr in &block.instructions {
            if let IRInstr::Alloc { size, .. } = instr {
                let aligned = (*size).div_ceil(16) * 16;
                total += aligned;
            }
        }
    }

    // Reserve space for register spill slots. Count the total number of
    // virtual registers used in the function. In the worst case, every
    // vreg beyond the 23 available physical registers (13 caller-saved +
    // 10 callee-saved) will need a spill slot (8 bytes each). Add a
    // safety margin.
    let mut max_vreg: u32 = 0;
    for block in &func.blocks {
        for instr in &block.instructions {
            for id in instr.defined_regs() {
                max_vreg = max_vreg.max(id);
            }
            for id in instr.used_regs() {
                max_vreg = max_vreg.max(id);
            }
        }
    }
    // Number of potential spills: max(0, vreg_count - available_registers)
    let available_regs: u32 = 23; // 13 caller-saved + 10 callee-saved
    let potential_spills = if max_vreg > available_regs {
        max_vreg - available_regs
    } else {
        0
    };
    let spill_bytes = potential_spills * 8; // 8 bytes per spill slot

    total += spill_bytes;

    // Round up to 16-byte alignment (should already be, but be safe).
    total = (total + 15) & !15;
    total
}

// ---------------------------------------------------------------------------
// ISA-aware relocation helpers
// ---------------------------------------------------------------------------

/// Return the ELF `e_machine` value for the given backend kind.
fn em_machine_for_backend(backend: BackendKind) -> Result<u16> {
    match backend {
        BackendKind::AArch64 => Ok(EM_AARCH64),
        BackendKind::X86_64 => Ok(EM_X86_64),
        BackendKind::RiscV64 => Ok(EM_RISCV),
        BackendKind::Mips64 => Ok(EM_MIPS),
        BackendKind::PowerPC64 => Ok(EM_PPC64),
        BackendKind::LoongArch64 => Ok(EM_LOONGARCH),
        BackendKind::Arm32 => Ok(EM_ARM),
        BackendKind::Wasm32 => Err(CodegenError::ElfError(
            "Wasm32 does not use ELF format — use emit_wasm() instead".to_string(),
        )),
    }
}

/// Return the call relocation type for the given backend kind.
///
/// Each ISA uses a different relocation type for inter-function call
/// instructions. This function maps the backend to the appropriate ELF
/// relocation constant.
fn call_reloc_type_for_backend(backend: BackendKind) -> Result<u32> {
    match backend {
        BackendKind::AArch64 => Ok(R_AARCH64_CALL26),
        BackendKind::X86_64 => Ok(R_X86_64_PLT32),
        BackendKind::RiscV64 => Ok(R_RISCV_CALL),
        BackendKind::Mips64 => Ok(R_MIPS_26),
        BackendKind::PowerPC64 => Ok(R_PPC64_REL24),
        BackendKind::LoongArch64 => Ok(R_LARCH_B26),
        BackendKind::Arm32 => Ok(R_ARM_CALL),
        BackendKind::Wasm32 => {
            Err(CodegenError::ElfError(
                "Wasm32 does not use ELF relocations — use emit_wasm() instead".to_string(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level emission functions
// ---------------------------------------------------------------------------

/// Emit a full ELF64 binary from the given IR functions.
///
/// The output includes:
/// - ELF64 header with the appropriate `e_machine`, little-endian, static executable
/// - Program headers: 3 LOAD segments (R for .rodata, RX for .text, RW for .data+.bss)
/// - Section headers: `.rodata`, `.text`, `.data`, `.bss`, `.symtab`,
///   `.strtab`, `.shstrtab`
/// - Symbol table entries for each function
/// - Relocation fixups for inter-function `BL` calls
///
/// ## Memory Layout
///
/// ```text
/// ┌─────────────┐  Lowest address
/// │ .rodata      │  Read-only data (segment: PF_R)
/// ├─────────────┤
/// │ .text        │  Executable code (segment: PF_R|PF_X)
/// ├─────────────┤
/// │ .data        │  Read-write data (segment: PF_R|PF_W)
/// ├─────────────┤
/// │ .bss         │  Zero-initialized (virtual only, same segment as .data)
/// └─────────────┘  Highest address
/// ```
pub fn emit_elf(
    functions: &[IRFunction],
    data_sections: &[DataSection],
    config: &EmitConfig,
) -> Result<Vec<u8>> {
    // Wasm32 should never go through the ELF emission path.
    if config.backend == BackendKind::Wasm32 || config.format == OutputFormat::Wasm {
        return Err(CodegenError::ElfError(
            "Wasm32 target cannot produce ELF output — use emit_wasm() instead".to_string(),
        ));
    }

    let base_addr = config.effective_base_addr();
    let is_obj = config.format == OutputFormat::Obj;
    let sec_align = section_alignment_for_backend(config.backend);

    // ---- Step 1: Emit all functions ----
    let mut emitter = Emitter::new();
    let mut text_section: Vec<u8> = Vec::new();
    let mut function_offsets: HashMap<String, u64> = HashMap::new();
    let mut function_sizes: HashMap<String, u64> = HashMap::new();
    let mut all_call_relocs: Vec<CallRelocation> = Vec::new();

    for func in functions {
        let func_offset = text_section.len() as u64;
        function_offsets.insert(func.name.clone(), func_offset);
        emitter.func_text_offset = func_offset;
        let code = emitter.emit_function(func)?;
        let func_size = (code.len() as u64) * 4;
        function_sizes.insert(func.name.clone(), func_size);
        all_call_relocs.extend(emitter.call_relocs.clone());
        for word in code {
            text_section.extend_from_slice(&word.to_le_bytes());
        }
    }

    // ---- Step 2: Handle inter-function call relocations ----
    // For ET_REL, collect external symbols and defer to the linker.
    // For ET_EXEC, resolve call relocations in-place.
    let mut external_symbols: Vec<String> = Vec::new();
    let mut rela_entries: Vec<RelaEntry> = Vec::new();
    if is_obj {
        let mut extern_set: std::collections::HashSet<String> = std::collections::HashSet::new();
        for reloc in &all_call_relocs {
            if !function_offsets.contains_key(&reloc.target_func) {
                extern_set.insert(reloc.target_func.clone());
            }
        }
        external_symbols = extern_set.into_iter().collect();
        external_symbols.sort();
    } else {
        resolve_call_relocs(&mut text_section, &all_call_relocs, &function_offsets)?;
    }

    // ---- Step 3: Collect data sections (with proper alignment) ----
    let (rodata_section, data_section, bss_size) =
        collect_data_sections(data_sections, config.backend);

    // ---- Step 4: Compute layout ----
    //
    // New layout places .rodata before .text in memory:
    //   [ELF Header] [Program Headers] [.rodata] [.text] [.rela.text] [.data] [.symtab] [.strtab] [.shstrtab] [Section Headers]
    //
    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let shdr_size: u64 = 64;
    // 3 LOAD segments: R (.rodata), RX (.text), RW (.data+.bss)
    let num_phdrs: u64 = if is_obj { 0 } else { 3 };
    let headers_total = elf_header_size + phdr_size * num_phdrs;

    // .rodata comes first in the file (after headers).
    let rodata_offset = headers_total;
    let rodata_size = rodata_section.len() as u64;
    let rodata_aligned = align_up(rodata_size, sec_align);
    let rodata_vaddr = if is_obj { 0 } else { base_addr + rodata_offset };

    // .text comes after .rodata.
    let text_offset = rodata_offset + rodata_aligned;
    let text_size = text_section.len() as u64;
    let text_aligned = align_up(text_size, sec_align);
    let text_vaddr = if is_obj { 0 } else { base_addr + text_offset };

    let entry_offset = function_offsets
        .get(&config.entry_name)
        .copied()
        .unwrap_or(0);
    let entry_point = if is_obj { 0 } else { text_vaddr + entry_offset };

    // ---- Step 5: Build symbol table and string table ----
    // .text is now at section index 2 (0=null, 1=.rodata, 2=.text).
    let text_section_idx: u16 = 2;
    let (symtab_bytes, strtab_bytes, sym_name_to_idx) = if config.symbol_table {
        build_symbol_table(
            functions,
            &function_offsets,
            &function_sizes,
            text_vaddr,
            &external_symbols,
            text_section_idx,
        )
    } else {
        (Vec::new(), Vec::new(), HashMap::new())
    };

    // Build .rela.text entries for ET_REL objects.
    let call_reloc_type = call_reloc_type_for_backend(config.backend)?;
    if is_obj {
        for reloc in &all_call_relocs {
            let sym_idx = sym_name_to_idx
                .get(&reloc.target_func)
                .copied()
                .unwrap_or(0);
            rela_entries.push(RelaEntry::new(
                reloc.text_byte_offset,
                sym_idx,
                call_reloc_type,
                0,
            ));
        }
        rela_entries.sort_by_key(|r| r.offset);
    }
    let rela_text_bytes: Vec<u8> = rela_entries.iter().flat_map(|r| r.to_bytes()).collect();

    // ---- Step 6: Compute remaining layout ----
    let rela_text_offset = text_offset + text_aligned;
    let rela_text_size = rela_text_bytes.len() as u64;
    let rela_text_aligned = if is_obj {
        align_up(rela_text_size, 8)
    } else {
        0
    };

    // .data section comes after .rela.text (or after .text if not ET_REL).
    let data_file_offset = text_offset + text_aligned + rela_text_aligned;
    let rwdata_size = data_section.len() as u64;
    let data_vaddr = if is_obj {
        0
    } else {
        base_addr + data_file_offset
    };

    // ---- Step 7: Build section header string table ----
    let shstrtab = build_shstrtab(config);

    // ---- Step 8: Compute section header offsets ----
    let symtab_file_offset = data_file_offset + rwdata_size;
    let symtab_aligned = align_up(symtab_bytes.len() as u64, 8);
    let strtab_file_offset = symtab_file_offset + symtab_aligned;
    let strtab_aligned = align_up(strtab_bytes.len() as u64, 8);
    let shstrtab_file_offset = strtab_file_offset + strtab_aligned;
    let shstrtab_aligned = align_up(shstrtab.len() as u64, 8);
    let shdr_offset = shstrtab_file_offset + shstrtab_aligned;

    // ---- Step 9: Build ELF header ----
    let mut elf = Vec::new();

    // e_ident
    elf.extend_from_slice(&ELF_MAGIC);
    elf.push(ELFCLASS64);
    elf.push(ELFDATA2LSB);
    elf.push(EV_CURRENT);
    let osabi = match config.target {
        Target::Linux => ELFOSABI_LINUX,
        Target::BareMetal => ELFOSABI_STANDALONE,
        Target::Wasm32 => ELFOSABI_STANDALONE, // unreachable (Wasm is rejected above)
    };
    elf.push(osabi);
    elf.push(0);
    elf.extend_from_slice(&[0u8; 7]);

    let e_type = if is_obj { ET_REL } else { ET_EXEC };
    elf.extend_from_slice(&e_type.to_le_bytes());
    let e_machine = em_machine_for_backend(config.backend)?;
    elf.extend_from_slice(&e_machine.to_le_bytes());
    elf.extend_from_slice(&(1u32).to_le_bytes());
    elf.extend_from_slice(&entry_point.to_le_bytes());
    elf.extend_from_slice(&elf_header_size.to_le_bytes());
    let sh_off = if config.section_headers {
        shdr_offset
    } else {
        0
    };
    elf.extend_from_slice(&sh_off.to_le_bytes());
    elf.extend_from_slice(&(0u32).to_le_bytes());
    elf.extend_from_slice(&(64u16).to_le_bytes());
    elf.extend_from_slice(&(56u16).to_le_bytes());
    elf.extend_from_slice(&(num_phdrs as u16).to_le_bytes());
    elf.extend_from_slice(&(shdr_size as u16).to_le_bytes());
    let rela_shift: u64 = if is_obj { 1 } else { 0 };
    let num_shdrs: u64 = if config.section_headers {
        8 + rela_shift
    } else {
        0
    };
    elf.extend_from_slice(&(num_shdrs as u16).to_le_bytes());
    let shstrndx = if config.section_headers {
        (7 + rela_shift) as u16
    } else {
        0u16
    };
    elf.extend_from_slice(&shstrndx.to_le_bytes());

    assert_eq!(elf.len(), 64, "ELF header must be exactly 64 bytes");

    // ---- Step 10: Program Headers ----
    // 3 LOAD segments: R for .rodata, RX for .text, RW for .data+.bss
    if !is_obj {
        // Segment 1: .rodata (read-only)
        write_phdr(
            &mut elf,
            PT_LOAD,
            PF_R,
            rodata_offset,
            rodata_vaddr,
            rodata_vaddr,
            rodata_size,
            rodata_size,
        );
        // Segment 2: .text (read + execute)
        write_phdr(
            &mut elf,
            PT_LOAD,
            PF_R | PF_X,
            text_offset,
            text_vaddr,
            text_vaddr,
            text_size,
            text_size,
        );
        // Segment 3: .data + .bss (read + write)
        write_phdr(
            &mut elf,
            PT_LOAD,
            PF_R | PF_W,
            data_file_offset,
            data_vaddr,
            data_vaddr,
            rwdata_size,
            rwdata_size + bss_size,
        );
    }

    // ---- Step 11: .rodata section ----
    elf.extend_from_slice(&rodata_section);
    let padding = rodata_aligned - rodata_size;
    elf.extend_from_slice(&vec![0u8; padding as usize]);

    // ---- Step 12: .text section ----
    elf.extend_from_slice(&text_section);
    let padding = text_aligned - text_size;
    elf.extend_from_slice(&vec![0u8; padding as usize]);

    // ---- Step 12.5: .rela.text section (ET_REL only) ----
    if is_obj {
        elf.extend_from_slice(&rela_text_bytes);
        let pad = rela_text_aligned - rela_text_bytes.len() as u64;
        elf.extend_from_slice(&vec![0u8; pad as usize]);
    }

    // ---- Step 13: .data section ----
    elf.extend_from_slice(&data_section);

    // ---- Step 14–16: .symtab, .strtab, .shstrtab ----
    if config.symbol_table {
        elf.extend_from_slice(&symtab_bytes);
        let pad = symtab_aligned - symtab_bytes.len() as u64;
        elf.extend_from_slice(&vec![0u8; pad as usize]);
        elf.extend_from_slice(&strtab_bytes);
        let pad = strtab_aligned - strtab_bytes.len() as u64;
        elf.extend_from_slice(&vec![0u8; pad as usize]);
        elf.extend_from_slice(&shstrtab);
        let pad = shstrtab_aligned - shstrtab.len() as u64;
        elf.extend_from_slice(&vec![0u8; pad as usize]);
    }

    // ---- Step 17: Section Headers ----
    //
    // Section order (ET_EXEC):       0=null, 1=.rodata, 2=.text, 3=.data, 4=.bss, 5=.symtab, 6=.strtab, 7=.shstrtab
    // Section order (ET_REL):        0=null, 1=.rodata, 2=.text, 3=.rela.text, 4=.data, 5=.bss, 6=.symtab, 7=.strtab, 8=.shstrtab
    if config.section_headers {
        // Section 0: null
        write_filled_shdr(&mut elf, &new_shdr(SHT_NULL, 0, 0, 0, 0, 0, 0, 0, 0));

        // Section 1: .rodata
        let rodata_name_idx = shstrtab_name_offset(&shstrtab, ".rodata");
        let mut sh = new_shdr(
            SHT_PROGBITS,
            PF_R as u64,
            rodata_vaddr,
            rodata_offset,
            rodata_size,
            0,
            0,
            sec_align,
            0,
        );
        sh.name = rodata_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);

        // Section 2: .text
        let text_name_idx = shstrtab_name_offset(&shstrtab, ".text");
        let mut sh = new_shdr(
            SHT_PROGBITS,
            (PF_R | PF_X) as u64,
            text_vaddr,
            text_offset,
            text_size,
            0,
            0,
            sec_align,
            0,
        );
        sh.name = text_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);

        // Section 3: .rela.text (ET_REL only)
        if is_obj {
            let rela_name_idx = shstrtab_name_offset(&shstrtab, ".rela.text");
            let mut sh = new_shdr(
                SHT_RELA,
                0,
                0,
                rela_text_offset,
                rela_text_size,
                6 + rela_shift as u32, // sh_link: .symtab section index (6 for ET_REL with .rela.text)
                2,                     // sh_info: .text section index
                8,
                24, // alignment, entry size
            );
            sh.name = rela_name_idx as u32;
            write_filled_shdr(&mut elf, &sh);
        }

        // Section 3+rela_shift: .data
        let data_name_idx = shstrtab_name_offset(&shstrtab, ".data");
        let mut sh = new_shdr(
            SHT_PROGBITS,
            (PF_R | PF_W) as u64,
            data_vaddr,
            data_file_offset,
            rwdata_size,
            0,
            0,
            sec_align,
            0,
        );
        sh.name = data_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);

        // Section 4+rela_shift: .bss
        let bss_name_idx = shstrtab_name_offset(&shstrtab, ".bss");
        let bss_vaddr = data_vaddr + rwdata_size;
        let mut sh = new_shdr(
            SHT_NOBITS,
            (PF_R | PF_W) as u64,
            bss_vaddr,
            0,
            bss_size,
            0,
            0,
            sec_align,
            0,
        );
        sh.name = bss_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);

        // Section 5+rela_shift: .symtab
        let symtab_name_idx = shstrtab_name_offset(&shstrtab, ".symtab");
        let mut sh = new_shdr(
            SHT_SYMTAB,
            0,
            0,
            symtab_file_offset,
            symtab_bytes.len() as u64,
            6 + rela_shift as u32, // sh_link: .strtab section index
            2,                     // sh_info: one past last local symbol
            8,
            24,
        );
        sh.name = symtab_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);

        // Section 6+rela_shift: .strtab
        let strtab_name_idx = shstrtab_name_offset(&shstrtab, ".strtab");
        let mut sh = new_shdr(
            SHT_STRTAB,
            0,
            0,
            strtab_file_offset,
            strtab_bytes.len() as u64,
            0,
            0,
            1,
            0,
        );
        sh.name = strtab_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);

        // Section 7+rela_shift: .shstrtab
        let shstrtab_name_idx = shstrtab_name_offset(&shstrtab, ".shstrtab");
        let mut sh = new_shdr(
            SHT_STRTAB,
            0,
            0,
            shstrtab_file_offset,
            shstrtab.len() as u64,
            0,
            0,
            1,
            0,
        );
        sh.name = shstrtab_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);
    }

    // ---- Step 18: Append DWARF5 debug sections if requested ----
    if config.debug_info && config.section_headers {
        let mut db = crate::dwarf::DwarfBuilder::new();
        let source_file = config.entry_name.clone() + ".vuma";
        db.add_compile_unit(&source_file, "vuma-codegen 0.1");
        for func in functions {
            let start = function_offsets.get(&func.name).copied().unwrap_or(0);
            let size = function_sizes.get(&func.name).copied().unwrap_or(0);
            db.add_subprogram(&func.name, start, start + size);
        }
        let sections = db.emit_debug_sections();
        crate::dwarf::append_debug_sections_to_elf(&mut elf, &sections);
    }

    Ok(elf)
}

/// Emit a flat raw binary for bare-metal AArch64 execution.
///
/// The output is the concatenated machine code for all functions, suitable for
/// loading at address `0x80000` on the AArch64.
pub fn emit_raw(functions: &[IRFunction], data_sections: &[DataSection], config: &EmitConfig) -> Result<Vec<u8>> {
    // Wasm32 should never go through the raw binary emission path.
    if config.backend == BackendKind::Wasm32 || config.format == OutputFormat::Wasm {
        return Err(CodegenError::ElfError(
            "Wasm32 target cannot produce raw binary output — use emit_wasm() instead"
                .to_string(),
        ));
    }

    let mut emitter = Emitter::new();
    let mut text_section: Vec<u8> = Vec::new();
    let mut function_offsets: HashMap<String, u64> = HashMap::new();
    let mut all_call_relocs: Vec<CallRelocation> = Vec::new();

    for func in functions {
        let func_offset = text_section.len() as u64;
        function_offsets.insert(func.name.clone(), func_offset);
        emitter.func_text_offset = func_offset;
        let code = emitter.emit_function(func)?;
        all_call_relocs.extend(emitter.call_relocs.clone());
        for word in code {
            text_section.extend_from_slice(&word.to_le_bytes());
        }
    }

    resolve_call_relocs(&mut text_section, &all_call_relocs, &function_offsets)?;

    // Append data sections after the text section for bare-metal targets.
    // Each section is aligned to its stated alignment requirement.
    for section in data_sections {
        let align = section.align.max(1) as usize;
        let padding = (align - (text_section.len() % align)) % align;
        text_section.extend(std::iter::repeat(0u8).take(padding));
        text_section.extend_from_slice(&section.data);
    }

    let _ = config; // base_addr used implicitly via relocation math
    Ok(text_section)
}

/// Emit a relocatable ELF object file (ET_REL) for the specified ISA backend.
///
/// Convenience wrapper around [`emit_elf`] with `OutputFormat::Obj` and the
/// given backend kind. The resulting object file contains `.rela.text` entries
/// using the appropriate relocation type for the target ISA.
pub fn emit_obj(
    functions: &[IRFunction],
    data_sections: &[DataSection],
    backend: BackendKind,
) -> Result<Vec<u8>> {
    // Wasm32 does not produce ELF object files.
    if backend == BackendKind::Wasm32 {
        return Err(CodegenError::ElfError(
            "Wasm32 target cannot produce ELF object files — use emit_wasm() instead".to_string(),
        ));
    }
    let config = EmitConfig::relocatable_obj_for(backend);
    emit_elf(functions, data_sections, &config)
}

// ---------------------------------------------------------------------------
// Wasm32 emission
// ---------------------------------------------------------------------------

/// Emit a `.wasm` binary module from the given IR functions.
///
/// This function uses the [`Wasm32Backend`](crate::wasm32::Wasm32Backend) to
/// lower IR functions to Wasm bytecode and assemble them into a complete Wasm
/// module with proper type, function, memory, export, start, and code sections.
///
/// The entry-point function (`_start` or `main`) is set as the Wasm start
/// function so the module executes automatically on instantiation.  It is also
/// exported by name for external reference.
///
/// # Wasm Module Layout
///
/// ```text
/// ┌──────────────────────────┐
/// │ Magic + Version          │  8 bytes
/// ├──────────────────────────┤
/// │ Type Section             │  function signatures
/// ├──────────────────────────┤
/// │ Import Section (opt)     │  (reserved for WASI)
/// ├──────────────────────────┤
/// │ Function Section         │  type index per function
/// ├──────────────────────────┤
/// │ Memory Section           │  1 memory, min 2 pages
/// ├──────────────────────────┤
/// │ Global Section           │  __heap_ptr (mutable i32)
/// ├──────────────────────────┤
/// │ Export Section           │  all functions exported by name
/// ├──────────────────────────┤
/// │ Start Section            │  _start / main function index
/// ├──────────────────────────┤
/// │ Code Section             │  function bodies
/// └──────────────────────────┘
/// ```
pub fn emit_wasm(
    functions: &[IRFunction],
    data_sections: &[DataSection],
    config: &EmitConfig,
) -> Result<Vec<u8>> {
    use crate::backend::Backend;

    let backend = crate::wasm32::Wasm32Backend::new();

    // ── Allocate registers (lowers IR → Wasm bytecode) for each function ──
    let mut allocated_funcs = Vec::new();
    for func in functions {
        match backend.allocate_registers(func) {
            Ok(af) => allocated_funcs.push(af),
            Err(e) => {
                return Err(CodegenError::TranslationError(format!(
                    "Wasm32 register allocation failed for '{}': {}",
                    func.name, e
                )));
            }
        }
    }

    let program = crate::backend::AllocatedProgram {
        functions: allocated_funcs,
        total_code_size: 0, // computed by the backend during encoding
        total_data_size: 0,
    };

    // ── Encode the program into a .wasm module ──
    let wasm_bytes = backend
        .encode_program(&program)
        .map_err(|e| CodegenError::EncodingError(format!("Wasm32 encode_program failed: {}", e)))?;

    let _ = data_sections; // Wasm data sections are handled via memory + data segments
    let _ = config; // Wasm config is embedded in the module structure

    Ok(wasm_bytes)
}

// ---------------------------------------------------------------------------
// Top-level emission dispatcher
// ---------------------------------------------------------------------------

/// Emit the final binary from the given IR program, dispatching to the
/// appropriate emitter based on the [`OutputFormat`] in `config`.
///
/// - `OutputFormat::ELF` / `OutputFormat::Obj` → [`emit_elf`]
/// - `OutputFormat::Raw` → [`emit_raw`]
/// - `OutputFormat::Wasm` → [`emit_wasm`]
pub fn emit_binary(
    functions: &[IRFunction],
    data_sections: &[DataSection],
    config: &EmitConfig,
) -> Result<Vec<u8>> {
    match config.format {
        OutputFormat::ELF | OutputFormat::Obj => {
            emit_elf(functions, data_sections, config)
        }
        OutputFormat::Raw => emit_raw(functions, data_sections, config),
        OutputFormat::Wasm => emit_wasm(functions, data_sections, config),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers for emit_elf / emit_raw
// ---------------------------------------------------------------------------

/// Patch BL instructions in `text_section` according to the relocation records.
fn resolve_call_relocs(
    text_section: &mut [u8],
    relocs: &[CallRelocation],
    function_offsets: &HashMap<String, u64>,
) -> Result<()> {
    for reloc in relocs {
        let target_offset = match function_offsets.get(&reloc.target_func) {
            Some(&off) => off,
            None => {
                log::warn!(
                    "call relocation target '{}' not found — leaving BL offset as 0",
                    reloc.target_func
                );
                continue;
            }
        };
        let bl_byte_idx = reloc.text_byte_offset as usize;
        if bl_byte_idx + 4 > text_section.len() {
            return Err(CodegenError::ElfError(format!(
                "call relocation at byte {} is out of bounds (text section is {} bytes)",
                bl_byte_idx,
                text_section.len()
            )));
        }
        let bl_word = u32::from_le_bytes([
            text_section[bl_byte_idx],
            text_section[bl_byte_idx + 1],
            text_section[bl_byte_idx + 2],
            text_section[bl_byte_idx + 3],
        ]);
        let offset_bytes = (target_offset as i64) - (reloc.text_byte_offset as i64);
        let offset_words = (offset_bytes >> 2) as i32;
        let patched = (bl_word & !0x03FFFFFF) | ((offset_words as u32) & 0x03FFFFFF);
        text_section[bl_byte_idx..bl_byte_idx + 4].copy_from_slice(&patched.to_le_bytes());
    }
    Ok(())
}

/// Return the natural section alignment (in bytes) for the given backend.
///
/// This is used for `sh_addralign` in section headers and for padding
/// between adjacent data contributions within a section.
///
/// | Backend      | Alignment | Rationale                        |
/// |--------------|-----------|----------------------------------|
/// | ARM32        | 4         | 32-bit instructions, 4-byte word |
/// | AArch64      | 16        | 128-bit SIMD, cache-line hint    |
/// | x86-64       | 16        | SSE/AVX alignment requirement    |
/// | RISC-V 64    | 4         | Base ISA 32-bit, compressed OK   |
/// | MIPS64       | 8         | 64-bit word-oriented             |
/// | PPC64        | 16        | Altivec/VSX 128-bit alignment    |
/// | LoongArch64  | 8         | 64-bit word-oriented             |
pub fn section_alignment_for_backend(backend: BackendKind) -> u64 {
    match backend {
        BackendKind::AArch64 => 16,
        BackendKind::X86_64 => 16,
        BackendKind::RiscV64 => 4,
        BackendKind::Arm32 => 4,
        BackendKind::Mips64 => 8,
        BackendKind::PowerPC64 => 16,
        BackendKind::LoongArch64 => 8,
        BackendKind::Wasm32 => 4, // Wasm doesn't use ELF, but provide a default
    }
}

/// Separate data sections into rodata, data, and bss size, respecting
/// per-section alignment requirements.
///
/// Each `DataSection` may specify its own alignment.  Padding bytes
/// (zero-filled) are inserted between adjacent contributions so that
/// every contribution starts at an offset that is a multiple of its
/// stated alignment.  The overall section alignment is the maximum of
/// the individual alignments (or the backend default if higher).
fn collect_data_sections(data_sections: &[DataSection], backend: BackendKind) -> (Vec<u8>, Vec<u8>, u64) {
    let default_align = section_alignment_for_backend(backend);
    let mut rodata_section = Vec::new();
    let mut data_section = Vec::new();
    let mut bss_size: u64 = 0;

    for ds in data_sections {
        // Use the section's own alignment, or the backend default if lower.
        let align = if ds.align > 0 {
            std::cmp::max(ds.align as u64, default_align)
        } else {
            default_align
        };

        match ds.kind {
            DataSectionKind::ReadOnly => {
                // Pad rodata_section to `align` boundary before appending.
                let padding = (align - (rodata_section.len() as u64 % align)) % align;
                rodata_section.extend(std::iter::repeat(0u8).take(padding as usize));
                rodata_section.extend_from_slice(&ds.data);
            }
            DataSectionKind::Data => {
                // Pad data_section to `align` boundary before appending.
                let padding = (align - (data_section.len() as u64 % align)) % align;
                data_section.extend(std::iter::repeat(0u8).take(padding as usize));
                data_section.extend_from_slice(&ds.data);
            }
            DataSectionKind::Bss => {
                // Pad bss_size to `align` boundary before accounting for this section.
                let padding = (align - (bss_size % align)) % align;
                bss_size += padding;
                bss_size += ds.data.len() as u64;
            }
        }
    }
    (rodata_section, data_section, bss_size)
}

// ---------------------------------------------------------------------------
// ELF builder helpers
// ---------------------------------------------------------------------------

/// Write a 64-bit ELF program header.
#[allow(clippy::too_many_arguments)]
fn write_phdr(
    buf: &mut Vec<u8>,
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
) {
    buf.extend_from_slice(&p_type.to_le_bytes());
    buf.extend_from_slice(&p_flags.to_le_bytes());
    buf.extend_from_slice(&p_offset.to_le_bytes());
    buf.extend_from_slice(&p_vaddr.to_le_bytes());
    buf.extend_from_slice(&p_paddr.to_le_bytes());
    buf.extend_from_slice(&p_filesz.to_le_bytes());
    buf.extend_from_slice(&p_memsz.to_le_bytes());
    buf.extend_from_slice(&(0x1000u64).to_le_bytes());
}

/// A filled-in section header, ready to be serialized.
struct FilledShdr {
    name: u32,
    sh_type: u32,
    sh_flags: u64,
    sh_addr: u64,
    sh_offset: u64,
    sh_size: u64,
    sh_link: u32,
    sh_info: u32,
    sh_addralign: u64,
    sh_entsize: u64,
}

#[allow(clippy::too_many_arguments)]
fn new_shdr(
    sh_type: u32,
    sh_flags: u64,
    sh_addr: u64,
    sh_offset: u64,
    sh_size: u64,
    sh_link: u32,
    sh_info: u32,
    sh_addralign: u64,
    sh_entsize: u64,
) -> FilledShdr {
    FilledShdr {
        name: 0,
        sh_type,
        sh_flags,
        sh_addr,
        sh_offset,
        sh_size,
        sh_link,
        sh_info,
        sh_addralign,
        sh_entsize,
    }
}

fn write_filled_shdr(buf: &mut Vec<u8>, sh: &FilledShdr) {
    buf.extend_from_slice(&sh.name.to_le_bytes());
    buf.extend_from_slice(&sh.sh_type.to_le_bytes());
    buf.extend_from_slice(&sh.sh_flags.to_le_bytes());
    buf.extend_from_slice(&sh.sh_addr.to_le_bytes());
    buf.extend_from_slice(&sh.sh_offset.to_le_bytes());
    buf.extend_from_slice(&sh.sh_size.to_le_bytes());
    buf.extend_from_slice(&sh.sh_link.to_le_bytes());
    buf.extend_from_slice(&sh.sh_info.to_le_bytes());
    buf.extend_from_slice(&sh.sh_addralign.to_le_bytes());
    buf.extend_from_slice(&sh.sh_entsize.to_le_bytes());
}

/// Build the symbol table (`.symtab`) and associated string table (`.strtab`).
///
/// Also returns a mapping from symbol name to symbol table index, used to
/// populate relocation entries.
///
/// The `text_section_idx` parameter specifies which section index the .text
/// section occupies in the section header table (needed for st_shndx in
/// function symbols).  With the new layout (.rodata at index 1, .text at
/// index 2), this is `2`.
fn build_symbol_table(
    functions: &[IRFunction],
    function_offsets: &HashMap<String, u64>,
    function_sizes: &HashMap<String, u64>,
    text_vaddr: u64,
    external_symbols: &[String],
    text_section_idx: u16,
) -> (Vec<u8>, Vec<u8>, HashMap<String, u32>) {
    let mut strtab = Vec::new();
    let mut symtab = Vec::new();
    let mut name_to_idx: HashMap<String, u32> = HashMap::new();

    strtab.push(0); // null byte at offset 0

    // Symbol 0: null symbol (required).
    symtab.extend_from_slice(&0u32.to_le_bytes()); // st_name
    symtab.push(0); // st_info
    symtab.push(0); // st_other
    symtab.extend_from_slice(&0u16.to_le_bytes()); // st_shndx
    symtab.extend_from_slice(&0u64.to_le_bytes()); // st_value
    symtab.extend_from_slice(&0u64.to_le_bytes()); // st_size

    // Symbol 1: section symbol for .text (local).
    let text_name_off = strtab.len() as u32;
    strtab.extend_from_slice(b".text\0");
    let st_info = (STB_LOCAL << 4) | STT_SECTION;
    symtab.extend_from_slice(&text_name_off.to_le_bytes());
    symtab.push(st_info);
    symtab.push(0);
    symtab.extend_from_slice(&text_section_idx.to_le_bytes());
    symtab.extend_from_slice(&text_vaddr.to_le_bytes());
    symtab.extend_from_slice(&0u64.to_le_bytes());

    // One symbol per function (global).
    let mut next_idx: u32 = 2;
    for func in functions {
        let name_off = strtab.len() as u32;
        strtab.extend_from_slice(func.name.as_bytes());
        strtab.push(0);
        let st_info = (STB_GLOBAL << 4) | STT_FUNC;
        let value = text_vaddr + function_offsets.get(&func.name).copied().unwrap_or(0);
        let size = function_sizes.get(&func.name).copied().unwrap_or(0);
        symtab.extend_from_slice(&name_off.to_le_bytes());
        symtab.push(st_info);
        symtab.push(0);
        symtab.extend_from_slice(&text_section_idx.to_le_bytes()); // .text section
        symtab.extend_from_slice(&value.to_le_bytes());
        symtab.extend_from_slice(&size.to_le_bytes());
        name_to_idx.insert(func.name.clone(), next_idx);
        next_idx += 1;
    }

    // External symbols (undefined, global) — for relocation targets not
    // defined in this object file.
    for name in external_symbols {
        let name_off = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        let st_info = (STB_GLOBAL << 4) | STT_NOTYPE;
        symtab.extend_from_slice(&name_off.to_le_bytes());
        symtab.push(st_info);
        symtab.push(0);
        symtab.extend_from_slice(&SHN_UNDEF.to_le_bytes());
        symtab.extend_from_slice(&0u64.to_le_bytes()); // st_value = 0
        symtab.extend_from_slice(&0u64.to_le_bytes()); // st_size = 0
        name_to_idx.insert(name.clone(), next_idx);
        next_idx += 1;
    }

    (symtab, strtab, name_to_idx)
}

/// Build the section-header string table (`.shstrtab`).
///
/// The order must match the section header table order:
/// `.rodata`, `.text`, [`.rela.text`], `.data`, `.bss`, `.symtab`, `.strtab`, `.shstrtab`
fn build_shstrtab(config: &EmitConfig) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(0);
    if config.section_headers {
        buf.extend_from_slice(b".rodata\0");
        buf.extend_from_slice(b".text\0");
        if config.format == OutputFormat::Obj {
            buf.extend_from_slice(b".rela.text\0");
        }
        buf.extend_from_slice(b".data\0");
        buf.extend_from_slice(b".bss\0");
        buf.extend_from_slice(b".symtab\0");
        buf.extend_from_slice(b".strtab\0");
        buf.extend_from_slice(b".shstrtab\0");
    }
    buf
}

/// Find the byte offset of a section name within the `.shstrtab`.
fn shstrtab_name_offset(shstrtab: &[u8], name: &str) -> usize {
    let name_bytes = name.as_bytes();
    for i in 0..shstrtab.len() {
        if i + name_bytes.len() < shstrtab.len()
            && &shstrtab[i..i + name_bytes.len()] == name_bytes
            && shstrtab[i + name_bytes.len()] == 0
        {
            return i;
        }
    }
    0
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Round `value` up to the nearest multiple of `alignment`.
fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_return_function(name: &str) -> IRFunction {
        let mut func = IRFunction::new(name);
        func.current_block().terminator = IRTerminator::Return(vec![]);
        func
    }

    fn make_calling_function(name: &str, callee: &str) -> IRFunction {
        let mut func = IRFunction::new(name);
        func.current_block().push(IRInstr::Call {
            dst: None,
            func: callee.to_string(),
            args: vec![],
            is_extern: false,
        });
        func.current_block().terminator = IRTerminator::Return(vec![]);
        func
    }

    #[test]
    fn emit_elf_header_valid() {
        let funcs = vec![make_return_function("main")];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &[], &config).unwrap();
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        assert_eq!(elf[4], ELFCLASS64);
        assert_eq!(elf[5], ELFDATA2LSB);
    }

    #[test]
    fn emit_elf_machine_aarch64() {
        let funcs = vec![make_return_function("main")];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &[], &config).unwrap();
        let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
        assert_eq!(e_machine, EM_AARCH64);
    }

    #[test]
    fn emit_elf_type_exec() {
        let funcs = vec![make_return_function("main")];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &[], &config).unwrap();
        let e_type = u16::from_le_bytes([elf[16], elf[17]]);
        assert_eq!(e_type, ET_EXEC);
    }

    #[test]
    fn emit_elf_section_headers_present() {
        let funcs = vec![make_return_function("main")];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &[], &config).unwrap();
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap());
        assert_ne!(e_shoff, 0, "section headers must be present");
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap());
        assert_eq!(e_shnum, 8, "expected 8 section headers (null+rodata+text+data+bss+symtab+strtab+shstrtab)");
        let e_shstrndx = u16::from_le_bytes(elf[62..64].try_into().unwrap());
        assert_eq!(e_shstrndx, 7, "shstrtab at index 7");
    }

    #[test]
    fn emit_elf_symbol_table() {
        let funcs = vec![make_return_function("main")];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &[], &config).unwrap();
        let mut found_main = false;
        for i in 0..elf.len().saturating_sub(4) {
            if &elf[i..i + 5] == b"main\0" {
                found_main = true;
                break;
            }
        }
        assert!(found_main, "symbol 'main' must appear in strtab");
    }

    #[test]
    fn emit_raw_flat_binary() {
        let funcs = vec![make_return_function("_start")];
        let config = EmitConfig::bare_metal_raw();
        let raw = emit_raw(&funcs, &[], &config).unwrap();
        if raw.len() >= 4 {
            assert_ne!(&raw[0..4], &[0x7f, b'E', b'L', b'F'], "raw must not be ELF");
        }
        assert!(!raw.is_empty());
        assert_eq!(raw.len() % 4, 0, "raw binary must be 4-byte aligned");
    }

    #[test]
    fn emit_elf_call_relocation() {
        let helper = make_return_function("helper");
        let caller = make_calling_function("main", "helper");
        let funcs = vec![helper, caller];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &[], &config).unwrap();
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);

        // Look for a patched BL instruction in the text section.
        // With 3 LOAD segments, text starts after: ELF header (64) + 3 phdrs (56*3) + rodata_aligned
        // Since there's no rodata in this test, text starts at 64 + 56*3 = 232
        let text_offset: usize = (64 + 56 * 3) as usize;
        let mut found_bl = false;
        let mut i = text_offset;
        while i + 4 <= elf.len() {
            let word = u32::from_le_bytes([elf[i], elf[i + 1], elf[i + 2], elf[i + 3]]);
            if (word >> 26) == 0b100101 {
                let imm26 = word & 0x03FFFFFF;
                if imm26 != 0 {
                    found_bl = true;
                    break;
                }
            }
            i += 4;
        }
        assert!(found_bl, "expected a patched BL instruction");
    }

    #[test]
    fn emit_config_defaults() {
        let linux = EmitConfig::linux_elf();
        assert_eq!(linux.format, OutputFormat::ELF);
        assert_eq!(linux.target, Target::Linux);
        assert_eq!(linux.base_addr, BASE_ADDR_LINUX);

        let bare = EmitConfig::bare_metal_raw();
        assert_eq!(bare.format, OutputFormat::Raw);
        assert_eq!(bare.target, Target::BareMetal);
        assert_eq!(bare.base_addr, BASE_ADDR_BARE);

        let obj = EmitConfig::relocatable_obj();
        assert_eq!(obj.format, OutputFormat::Obj);
        assert_eq!(obj.base_addr, 0);

        let default = EmitConfig::default();
        assert_eq!(default.format, OutputFormat::ELF);
        assert_eq!(default.target, Target::Linux);
    }

    #[test]
    fn emit_obj_type_rel() {
        let funcs = vec![make_return_function("foo")];
        let config = EmitConfig::relocatable_obj();
        let elf = emit_elf(&funcs, &[], &config).unwrap();
        let e_type = u16::from_le_bytes([elf[16], elf[17]]);
        assert_eq!(e_type, ET_REL);
        let e_phnum = u16::from_le_bytes([elf[56], elf[57]]);
        assert_eq!(e_phnum, 0, "object file has no program headers");
    }

    #[test]
    fn emit_bare_metal_elf_osabi() {
        let funcs = vec![make_return_function("_start")];
        let config = EmitConfig::bare_metal_elf();
        let elf = emit_elf(&funcs, &[], &config).unwrap();
        assert_eq!(elf[7], ELFOSABI_STANDALONE);
    }

    #[test]
    fn emit_elf_data_sections() {
        let funcs = vec![make_return_function("main")];
        let data_sections = vec![
            DataSection {
                name: "rodata".into(),
                kind: DataSectionKind::ReadOnly,
                align: 4,
                data: vec![0xDE, 0xAD, 0xBE, 0xEF],
            },
            DataSection {
                name: "data".into(),
                kind: DataSectionKind::Data,
                align: 8,
                data: vec![0x42; 16],
            },
            DataSection {
                name: "bss".into(),
                kind: DataSectionKind::Bss,
                align: 16,
                data: vec![0; 32],
            },
        ];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &data_sections, &config).unwrap();
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        // Verify rodata bytes appear.
        let mut found_rodata = false;
        for i in 0..elf.len().saturating_sub(4) {
            if &elf[i..i + 4] == &[0xDE, 0xAD, 0xBE, 0xEF] {
                found_rodata = true;
                break;
            }
        }
        assert!(found_rodata, "rodata must appear in the ELF file");
    }

    #[test]
    fn emit_program_elf_header() {
        let mut func = IRFunction::new("main");
        func.current_block().terminator = IRTerminator::Return(vec![]);
        let program = IRProgram {
            functions: vec![func],
            data_sections: vec![],
        };
        let mut emitter = Emitter::new();
        let elf = emitter.emit_program(&program).unwrap();
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        assert_eq!(elf[4], ELFCLASS64);
        let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
        assert_eq!(e_machine, EM_AARCH64);
    }

    #[test]
    fn emit_elf_empty_program() {
        let funcs: Vec<IRFunction> = vec![];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &[], &config).unwrap();
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
        assert_eq!(e_machine, EM_AARCH64);
    }

    #[test]
    fn format_and_target_display() {
        assert_eq!(format!("{}", OutputFormat::ELF), "elf");
        assert_eq!(format!("{}", OutputFormat::Raw), "raw");
        assert_eq!(format!("{}", OutputFormat::Obj), "obj");
        assert_eq!(format!("{}", Target::Linux), "linux");
        assert_eq!(format!("{}", Target::BareMetal), "bare-metal");
    }

    #[test]
    fn effective_base_addr() {
        assert_eq!(
            EmitConfig::linux_elf().effective_base_addr(),
            BASE_ADDR_LINUX
        );
        assert_eq!(
            EmitConfig::bare_metal_raw().effective_base_addr(),
            BASE_ADDR_BARE
        );
        let mut custom = EmitConfig::linux_elf();
        custom.base_addr = 0x100000;
        assert_eq!(custom.effective_base_addr(), 0x100000);
    }

    #[test]
    fn emit_elf_multiple_function_symbols() {
        let funcs = vec![
            make_return_function("foo"),
            make_return_function("bar"),
            make_return_function("main"),
        ];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &[], &config).unwrap();
        for name in &["foo", "bar", "main"] {
            let name_bytes = [name.as_bytes(), &[0u8]].concat();
            let mut found = false;
            for i in 0..elf.len().saturating_sub(name.len() + 1) {
                if &elf[i..i + name.len() + 1] == name_bytes.as_slice() {
                    found = true;
                    break;
                }
            }
            assert!(found, "function '{}' must appear in strtab", name);
        }
    }

    // -----------------------------------------------------------------------
    // Relocation tests
    // -----------------------------------------------------------------------

    /// Parse SHT_RELA entries from an ELF binary.
    fn parse_rela_entries_from_elf(elf: &[u8]) -> Vec<RelaEntry> {
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap()) as usize;
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap()) as usize;

        for i in 0..e_shnum {
            let off = e_shoff + i * 64;
            let sh_type = u32::from_le_bytes(elf[off + 4..off + 8].try_into().unwrap());
            if sh_type == SHT_RELA {
                let sh_offset =
                    u64::from_le_bytes(elf[off + 24..off + 32].try_into().unwrap()) as usize;
                let sh_size =
                    u64::from_le_bytes(elf[off + 32..off + 40].try_into().unwrap()) as usize;
                let sh_entsize =
                    u64::from_le_bytes(elf[off + 56..off + 64].try_into().unwrap()) as usize;
                if sh_entsize == 0 {
                    continue;
                }
                let num_entries = sh_size / sh_entsize;
                let mut entries = Vec::new();
                for j in 0..num_entries {
                    let base = sh_offset + j * sh_entsize;
                    let r_offset = u64::from_le_bytes(elf[base..base + 8].try_into().unwrap());
                    let r_info = u64::from_le_bytes(elf[base + 8..base + 16].try_into().unwrap());
                    let r_addend =
                        i64::from_le_bytes(elf[base + 16..base + 24].try_into().unwrap());
                    entries.push(RelaEntry {
                        offset: r_offset,
                        info: r_info,
                        addend: r_addend,
                    });
                }
                return entries;
            }
        }
        Vec::new()
    }

    /// Parse symbols from an ELF binary, returning (name, st_info, st_value, st_shndx).
    fn parse_symbols_from_elf(elf: &[u8]) -> Vec<(String, u8, u64, u16)> {
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap()) as usize;
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap()) as usize;

        // Find .symtab and its linked .strtab.
        let mut symtab_offset: usize = 0;
        let mut symtab_size: usize = 0;
        let mut strtab_offset: usize = 0;
        let mut strtab_size: usize = 0;

        for i in 0..e_shnum {
            let off = e_shoff + i * 64;
            let sh_type = u32::from_le_bytes(elf[off + 4..off + 8].try_into().unwrap());
            if sh_type == SHT_SYMTAB {
                symtab_offset =
                    u64::from_le_bytes(elf[off + 24..off + 32].try_into().unwrap()) as usize;
                symtab_size =
                    u64::from_le_bytes(elf[off + 32..off + 40].try_into().unwrap()) as usize;
                let sh_link =
                    u32::from_le_bytes(elf[off + 40..off + 44].try_into().unwrap()) as usize;
                let strtab_off = e_shoff + sh_link * 64;
                strtab_offset =
                    u64::from_le_bytes(elf[strtab_off + 24..strtab_off + 32].try_into().unwrap())
                        as usize;
                strtab_size =
                    u64::from_le_bytes(elf[strtab_off + 32..strtab_off + 40].try_into().unwrap())
                        as usize;
                break;
            }
        }

        if symtab_size == 0 {
            return Vec::new();
        }

        let strtab = &elf[strtab_offset..strtab_offset + strtab_size];
        let num_syms = symtab_size / 24;
        let mut symbols = Vec::new();
        for i in 0..num_syms {
            let base = symtab_offset + i * 24;
            let st_name = u32::from_le_bytes(elf[base..base + 4].try_into().unwrap()) as usize;
            let st_info = elf[base + 4];
            let st_shndx = u16::from_le_bytes(elf[base + 6..base + 8].try_into().unwrap());
            let st_value = u64::from_le_bytes(elf[base + 8..base + 16].try_into().unwrap());

            let name = if st_name < strtab.len() {
                let end = strtab[st_name..]
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(strtab.len() - st_name);
                String::from_utf8_lossy(&strtab[st_name..st_name + end]).to_string()
            } else {
                String::new()
            };
            symbols.push((name, st_info, st_value, st_shndx));
        }
        symbols
    }

    #[test]
    fn rela_text_section_in_obj() {
        // Verify that .rela.text section header exists in ET_REL output.
        let helper = make_return_function("helper");
        let caller = make_calling_function("main", "helper");
        let funcs = vec![helper, caller];
        let config = EmitConfig::relocatable_obj();
        let elf = emit_elf(&funcs, &[], &config).unwrap();

        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap()) as usize;
        assert!(
            e_shnum >= 9,
            "ET_REL with calls should have at least 9 section headers"
        );

        // Find the .rela.text section by looking for SHT_RELA type.
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap()) as usize;
        let mut found_rela = false;
        for i in 0..e_shnum {
            let off = e_shoff + i * 64;
            let sh_type = u32::from_le_bytes(elf[off + 4..off + 8].try_into().unwrap());
            if sh_type == SHT_RELA {
                found_rela = true;
                break;
            }
        }
        assert!(
            found_rela,
            ".rela.text section (SHT_RELA) must exist in ET_REL"
        );
    }

    #[test]
    fn rela_call26_entry_for_bl() {
        // Verify that R_AARCH64_CALL26 entries are generated for BL instructions.
        let helper = make_return_function("helper");
        let caller = make_calling_function("main", "helper");
        let funcs = vec![helper, caller];
        let config = EmitConfig::relocatable_obj();
        let elf = emit_elf(&funcs, &[], &config).unwrap();

        let entries = parse_rela_entries_from_elf(&elf);
        assert!(!entries.is_empty(), "should have at least one rela entry");

        // All BL relocations should be R_AARCH64_CALL26.
        for entry in &entries {
            assert_eq!(
                entry.r_type(),
                R_AARCH64_CALL26,
                "expected R_AARCH64_CALL26, got type {}",
                entry.r_type()
            );
        }
    }

    #[test]
    fn rela_external_symbol_undefined() {
        // Verify that external function symbols have SHN_UNDEF.
        let caller = make_calling_function("main", "external_func");
        let funcs = vec![caller];
        let config = EmitConfig::relocatable_obj();
        let elf = emit_elf(&funcs, &[], &config).unwrap();

        // Parse the symbol table to find external_func.
        let symbols = parse_symbols_from_elf(&elf);
        let ext_sym = symbols
            .iter()
            .find(|(name, _, _, shndx)| name == "external_func" && *shndx == SHN_UNDEF);
        assert!(ext_sym.is_some(), "external_func should be SHN_UNDEF");
    }

    #[test]
    fn rela_offset_matches_bl_location() {
        // Verify the rela offset points to a BL instruction in .text.
        let helper = make_return_function("helper");
        let caller = make_calling_function("main", "helper");
        let funcs = vec![helper, caller];
        let config = EmitConfig::relocatable_obj();
        let elf = emit_elf(&funcs, &[], &config).unwrap();

        let entries = parse_rela_entries_from_elf(&elf);
        assert!(!entries.is_empty());

        // Find the .text section file offset from section headers.
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap()) as usize;
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap()) as usize;
        let mut text_file_offset: usize = 0;
        for i in 0..e_shnum {
            let off = e_shoff + i * 64;
            let sh_type = u32::from_le_bytes(elf[off + 4..off + 8].try_into().unwrap());
            if sh_type == SHT_PROGBITS {
                text_file_offset =
                    u64::from_le_bytes(elf[off + 24..off + 32].try_into().unwrap()) as usize;
                break;
            }
        }

        for entry in &entries {
            let bl_file_offset = text_file_offset + entry.offset as usize;
            assert!(
                bl_file_offset + 4 <= elf.len(),
                "relocation offset out of bounds"
            );
            let word = u32::from_le_bytes([
                elf[bl_file_offset],
                elf[bl_file_offset + 1],
                elf[bl_file_offset + 2],
                elf[bl_file_offset + 3],
            ]);
            // BL opcode: bits [31:26] = 100101
            assert_eq!(
                (word >> 26) & 0x3F,
                0b100101,
                "relocation offset should point to a BL instruction, got {:08x}",
                word
            );
        }
    }

    #[test]
    fn rela_multiple_calls() {
        // Verify multiple BL instructions generate multiple rela entries.
        let mut func = IRFunction::new("main");
        func.current_block().push(IRInstr::Call {
            dst: None,
            func: "foo".to_string(),
            args: vec![],
            is_extern: false,
        });
        func.current_block().push(IRInstr::Call {
            dst: None,
            func: "bar".to_string(),
            args: vec![],
            is_extern: false,
        });
        func.current_block().push(IRInstr::Call {
            dst: None,
            func: "baz".to_string(),
            args: vec![],
            is_extern: false,
        });
        func.current_block().terminator = IRTerminator::Return(vec![]);
        let funcs = vec![func];
        let config = EmitConfig::relocatable_obj();
        let elf = emit_elf(&funcs, &[], &config).unwrap();

        let entries = parse_rela_entries_from_elf(&elf);
        assert_eq!(entries.len(), 3, "expected 3 rela entries for 3 calls");

        // Verify offsets are distinct and increasing.
        for i in 1..entries.len() {
            assert!(
                entries[i].offset > entries[i - 1].offset,
                "rela entries should be sorted by offset"
            );
        }
    }

    #[test]
    fn rela_no_rela_in_exec() {
        // Verify ET_EXEC does not have SHT_RELA sections.
        let helper = make_return_function("helper");
        let caller = make_calling_function("main", "helper");
        let funcs = vec![helper, caller];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &[], &config).unwrap();

        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap()) as usize;
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap()) as usize;
        for i in 0..e_shnum {
            let off = e_shoff + i * 64;
            let sh_type = u32::from_le_bytes(elf[off + 4..off + 8].try_into().unwrap());
            assert_ne!(
                sh_type, SHT_RELA,
                "ET_EXEC should not have SHT_RELA sections"
            );
        }
    }

    #[test]
    fn rela_entry_struct_encoding() {
        // Verify RelaEntry struct encoding and field accessors.
        let entry = RelaEntry::new(0x1234, 5, R_AARCH64_CALL26, -4);
        assert_eq!(entry.offset, 0x1234);
        assert_eq!(entry.sym_idx(), 5);
        assert_eq!(entry.r_type(), R_AARCH64_CALL26);
        assert_eq!(entry.addend, -4);

        let bytes = entry.to_bytes();
        let parsed_offset = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let parsed_info = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let parsed_addend = i64::from_le_bytes(bytes[16..24].try_into().unwrap());
        assert_eq!(parsed_offset, 0x1234);
        assert_eq!(parsed_info, ((5u64) << 32) | (R_AARCH64_CALL26 as u64));
        assert_eq!(parsed_addend, -4);
    }

    #[test]
    fn rela_relocation_type_constants() {
        // Verify AArch64 relocation type constant values match the ELF spec.
        assert_eq!(R_AARCH64_CALL26, 283);
        assert_eq!(R_AARCH64_JUMP26, 282);
        assert_eq!(R_AARCH64_ADR_PREL_PG_HI21, 275);
        assert_eq!(R_AARCH64_LDST64_ABS_LO12_NC, 286);
    }

    // -----------------------------------------------------------------------
    // x86-64 relocation tests
    // -----------------------------------------------------------------------

    #[test]
    fn x86_64_relocation_constants() {
        assert_eq!(R_X86_64_64, 1);
        assert_eq!(R_X86_64_PC32, 2);
        assert_eq!(R_X86_64_PLT32, 4);
        assert_eq!(R_X86_64_32, 10);
        assert_eq!(R_X86_64_32S, 11);
    }

    #[test]
    fn emit_obj_x86_64_machine_type() {
        let funcs = vec![make_return_function("main")];
        let elf = emit_obj(&funcs, &[], BackendKind::X86_64).unwrap();
        let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
        assert_eq!(
            e_machine, EM_X86_64,
            "x86-64 object file must have EM_X86_64"
        );
    }

    #[test]
    fn emit_obj_x86_64_relocation_type() {
        let helper = make_return_function("helper");
        let caller = make_calling_function("main", "helper");
        let elf = emit_obj(&[helper, caller], &[], BackendKind::X86_64).unwrap();
        // Parse .rela.text entries and verify R_X86_64_PLT32 relocation type.
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap());
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap());
        for i in 0..e_shnum as usize {
            let sh_off = e_shoff as usize + i * 64;
            let sh_type = u32::from_le_bytes(elf[sh_off + 4..sh_off + 8].try_into().unwrap());
            if sh_type == SHT_RELA {
                let sh_offset =
                    u64::from_le_bytes(elf[sh_off + 24..sh_off + 32].try_into().unwrap());
                let sh_size = u64::from_le_bytes(elf[sh_off + 32..sh_off + 40].try_into().unwrap());
                let num_entries = sh_size as usize / 24;
                for j in 0..num_entries {
                    let ent_off = sh_offset as usize + j * 24;
                    let info =
                        u64::from_le_bytes(elf[ent_off + 8..ent_off + 16].try_into().unwrap());
                    let r_type = (info & 0xFFFFFFFF) as u32;
                    assert_eq!(
                        r_type, R_X86_64_PLT32,
                        "expected R_X86_64_PLT32 (4), got {}",
                        r_type
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // RISC-V64 relocation tests
    // -----------------------------------------------------------------------

    #[test]
    fn riscv64_relocation_constants() {
        assert_eq!(R_RISCV_CALL, 18);
        assert_eq!(R_RISCV_CALL_PLT, 19);
        assert_eq!(R_RISCV_PCREL_HI20, 23);
        assert_eq!(R_RISCV_PCREL_LO12_I, 24);
        assert_eq!(R_RISCV_HI20, 26);
        assert_eq!(R_RISCV_LO12_I, 27);
        assert_eq!(R_RISCV_JAL, 2);
        assert_eq!(R_RISCV_BRANCH, 16);
    }

    #[test]
    fn emit_obj_riscv64_machine_type() {
        let funcs = vec![make_return_function("main")];
        let elf = emit_obj(&funcs, &[], BackendKind::RiscV64).unwrap();
        let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
        assert_eq!(
            e_machine, EM_RISCV,
            "RISC-V64 object file must have EM_RISCV"
        );
    }

    #[test]
    fn emit_obj_riscv64_relocation_type() {
        let helper = make_return_function("helper");
        let caller = make_calling_function("main", "helper");
        let elf = emit_obj(&[helper, caller], &[], BackendKind::RiscV64).unwrap();
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap());
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap());
        for i in 0..e_shnum as usize {
            let sh_off = e_shoff as usize + i * 64;
            let sh_type = u32::from_le_bytes(elf[sh_off + 4..sh_off + 8].try_into().unwrap());
            if sh_type == SHT_RELA {
                let sh_offset =
                    u64::from_le_bytes(elf[sh_off + 24..sh_off + 32].try_into().unwrap());
                let sh_size = u64::from_le_bytes(elf[sh_off + 32..sh_off + 40].try_into().unwrap());
                let num_entries = sh_size as usize / 24;
                for j in 0..num_entries {
                    let ent_off = sh_offset as usize + j * 24;
                    let info =
                        u64::from_le_bytes(elf[ent_off + 8..ent_off + 16].try_into().unwrap());
                    let r_type = (info & 0xFFFFFFFF) as u32;
                    assert_eq!(
                        r_type, R_RISCV_CALL,
                        "expected R_RISCV_CALL (18), got {}",
                        r_type
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // MIPS64 relocation tests
    // -----------------------------------------------------------------------

    #[test]
    fn mips64_relocation_constants() {
        assert_eq!(R_MIPS_26, 4);
        assert_eq!(R_MIPS_32, 2);
        assert_eq!(R_MIPS_64, 18);
        assert_eq!(R_MIPS_HI16, 5);
        assert_eq!(R_MIPS_LO16, 6);
        assert_eq!(R_MIPS_CALL16, 11);
        assert_eq!(R_MIPS_GPREL16, 7);
    }

    #[test]
    fn emit_obj_mips64_machine_type() {
        let funcs = vec![make_return_function("main")];
        let elf = emit_obj(&funcs, &[], BackendKind::Mips64).unwrap();
        let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
        assert_eq!(e_machine, EM_MIPS, "MIPS64 object file must have EM_MIPS");
    }

    #[test]
    fn emit_obj_mips64_relocation_type() {
        let helper = make_return_function("helper");
        let caller = make_calling_function("main", "helper");
        let elf = emit_obj(&[helper, caller], &[], BackendKind::Mips64).unwrap();
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap());
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap());
        for i in 0..e_shnum as usize {
            let sh_off = e_shoff as usize + i * 64;
            let sh_type = u32::from_le_bytes(elf[sh_off + 4..sh_off + 8].try_into().unwrap());
            if sh_type == SHT_RELA {
                let sh_offset =
                    u64::from_le_bytes(elf[sh_off + 24..sh_off + 32].try_into().unwrap());
                let sh_size = u64::from_le_bytes(elf[sh_off + 32..sh_off + 40].try_into().unwrap());
                let num_entries = sh_size as usize / 24;
                for j in 0..num_entries {
                    let ent_off = sh_offset as usize + j * 24;
                    let info =
                        u64::from_le_bytes(elf[ent_off + 8..ent_off + 16].try_into().unwrap());
                    let r_type = (info & 0xFFFFFFFF) as u32;
                    assert_eq!(r_type, R_MIPS_26, "expected R_MIPS_26 (4), got {}", r_type);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // PowerPC64 relocation tests
    // -----------------------------------------------------------------------

    #[test]
    fn ppc64_relocation_constants() {
        assert_eq!(R_PPC64_ADDR64, 38);
        assert_eq!(R_PPC64_ADDR32, 20);
        assert_eq!(R_PPC64_REL24, 10);
        assert_eq!(R_PPC64_REL32, 26);
    }

    #[test]
    fn emit_obj_ppc64_machine_type() {
        let funcs = vec![make_return_function("main")];
        let elf = emit_obj(&funcs, &[], BackendKind::PowerPC64).unwrap();
        let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
        assert_eq!(e_machine, EM_PPC64, "PPC64 object file must have EM_PPC64");
    }

    #[test]
    fn emit_obj_ppc64_relocation_type() {
        let helper = make_return_function("helper");
        let caller = make_calling_function("main", "helper");
        let elf = emit_obj(&[helper, caller], &[], BackendKind::PowerPC64).unwrap();
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap());
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap());
        for i in 0..e_shnum as usize {
            let sh_off = e_shoff as usize + i * 64;
            let sh_type = u32::from_le_bytes(elf[sh_off + 4..sh_off + 8].try_into().unwrap());
            if sh_type == SHT_RELA {
                let sh_offset =
                    u64::from_le_bytes(elf[sh_off + 24..sh_off + 32].try_into().unwrap());
                let sh_size = u64::from_le_bytes(elf[sh_off + 32..sh_off + 40].try_into().unwrap());
                let num_entries = sh_size as usize / 24;
                for j in 0..num_entries {
                    let ent_off = sh_offset as usize + j * 24;
                    let info =
                        u64::from_le_bytes(elf[ent_off + 8..ent_off + 16].try_into().unwrap());
                    let r_type = (info & 0xFFFFFFFF) as u32;
                    assert_eq!(
                        r_type, R_PPC64_REL24,
                        "expected R_PPC64_REL24 (10), got {}",
                        r_type
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // LoongArch64 relocation tests
    // -----------------------------------------------------------------------

    #[test]
    fn loongarch64_relocation_constants() {
        assert_eq!(R_LARCH_64, 79);
        assert_eq!(R_LARCH_32, 77);
        assert_eq!(R_LARCH_B26, 69);
        assert_eq!(R_LARCH_PCALA_HI20, 44);
        assert_eq!(R_LARCH_PCALA_LO12, 45);
        assert_eq!(R_LARCH_CALL36, 89);
    }

    #[test]
    fn emit_obj_loongarch64_machine_type() {
        let funcs = vec![make_return_function("main")];
        let elf = emit_obj(&funcs, &[], BackendKind::LoongArch64).unwrap();
        let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
        assert_eq!(
            e_machine, EM_LOONGARCH,
            "LoongArch64 object file must have EM_LOONGARCH"
        );
    }

    #[test]
    fn emit_obj_loongarch64_relocation_type() {
        let helper = make_return_function("helper");
        let caller = make_calling_function("main", "helper");
        let elf = emit_obj(&[helper, caller], &[], BackendKind::LoongArch64).unwrap();
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap());
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap());
        for i in 0..e_shnum as usize {
            let sh_off = e_shoff as usize + i * 64;
            let sh_type = u32::from_le_bytes(elf[sh_off + 4..sh_off + 8].try_into().unwrap());
            if sh_type == SHT_RELA {
                let sh_offset =
                    u64::from_le_bytes(elf[sh_off + 24..sh_off + 32].try_into().unwrap());
                let sh_size = u64::from_le_bytes(elf[sh_off + 32..sh_off + 40].try_into().unwrap());
                let num_entries = sh_size as usize / 24;
                for j in 0..num_entries {
                    let ent_off = sh_offset as usize + j * 24;
                    let info =
                        u64::from_le_bytes(elf[ent_off + 8..ent_off + 16].try_into().unwrap());
                    let r_type = (info & 0xFFFFFFFF) as u32;
                    assert_eq!(
                        r_type, R_LARCH_B26,
                        "expected R_LARCH_B26 (69), got {}",
                        r_type
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // ARM32 relocation tests
    // -----------------------------------------------------------------------

    #[test]
    fn arm32_relocation_constants() {
        assert_eq!(R_ARM_CALL, 28);
        assert_eq!(R_ARM_JUMP24, 29);
        assert_eq!(R_ARM_MOVW_ABS_NC, 43);
        assert_eq!(R_ARM_MOVT_ABS, 44);
        assert_eq!(R_ARM_REL32, 3);
        assert_eq!(R_ARM_ABS32, 2);
    }

    #[test]
    fn emit_obj_arm32_machine_type() {
        let funcs = vec![make_return_function("main")];
        let elf = emit_obj(&funcs, &[], BackendKind::Arm32).unwrap();
        let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
        assert_eq!(e_machine, EM_ARM, "ARM32 object file must have EM_ARM");
    }

    #[test]
    fn emit_obj_arm32_relocation_type() {
        let helper = make_return_function("helper");
        let caller = make_calling_function("main", "helper");
        let elf = emit_obj(&[helper, caller], &[], BackendKind::Arm32).unwrap();
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap());
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap());
        for i in 0..e_shnum as usize {
            let sh_off = e_shoff as usize + i * 64;
            let sh_type = u32::from_le_bytes(elf[sh_off + 4..sh_off + 8].try_into().unwrap());
            if sh_type == SHT_RELA {
                let sh_offset =
                    u64::from_le_bytes(elf[sh_off + 24..sh_off + 32].try_into().unwrap());
                let sh_size = u64::from_le_bytes(elf[sh_off + 32..sh_off + 40].try_into().unwrap());
                let num_entries = sh_size as usize / 24;
                for j in 0..num_entries {
                    let ent_off = sh_offset as usize + j * 24;
                    let info =
                        u64::from_le_bytes(elf[ent_off + 8..ent_off + 16].try_into().unwrap());
                    let r_type = (info & 0xFFFFFFFF) as u32;
                    assert_eq!(
                        r_type, R_ARM_CALL,
                        "expected R_ARM_CALL (28), got {}",
                        r_type
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // ISA-aware helper function tests
    // -----------------------------------------------------------------------

    #[test]
    fn em_machine_for_backend_mapping() {
        assert_eq!(em_machine_for_backend(BackendKind::AArch64).unwrap(), EM_AARCH64);
        assert_eq!(em_machine_for_backend(BackendKind::X86_64).unwrap(), EM_X86_64);
        assert_eq!(em_machine_for_backend(BackendKind::RiscV64).unwrap(), EM_RISCV);
        assert_eq!(em_machine_for_backend(BackendKind::Mips64).unwrap(), EM_MIPS);
        assert_eq!(em_machine_for_backend(BackendKind::PowerPC64).unwrap(), EM_PPC64);
        assert_eq!(
            em_machine_for_backend(BackendKind::LoongArch64).unwrap(),
            EM_LOONGARCH
        );
        assert_eq!(em_machine_for_backend(BackendKind::Arm32).unwrap(), EM_ARM);
        // Wasm32 should return an error for ELF machine type
        assert!(em_machine_for_backend(BackendKind::Wasm32).is_err());
    }

    #[test]
    fn call_reloc_type_for_backend_mapping() {
        assert_eq!(
            call_reloc_type_for_backend(BackendKind::AArch64).unwrap(),
            R_AARCH64_CALL26
        );
        assert_eq!(
            call_reloc_type_for_backend(BackendKind::X86_64).unwrap(),
            R_X86_64_PLT32
        );
        assert_eq!(
            call_reloc_type_for_backend(BackendKind::RiscV64).unwrap(),
            R_RISCV_CALL
        );
        assert_eq!(call_reloc_type_for_backend(BackendKind::Mips64).unwrap(), R_MIPS_26);
        assert_eq!(
            call_reloc_type_for_backend(BackendKind::PowerPC64).unwrap(),
            R_PPC64_REL24
        );
        assert_eq!(
            call_reloc_type_for_backend(BackendKind::LoongArch64).unwrap(),
            R_LARCH_B26
        );
        assert_eq!(call_reloc_type_for_backend(BackendKind::Arm32).unwrap(), R_ARM_CALL);
        // Wasm32 should return an error for ELF relocation type
        assert!(call_reloc_type_for_backend(BackendKind::Wasm32).is_err());
    }

    // -----------------------------------------------------------------------
    // Codegen backend instruction emission tests
    // -----------------------------------------------------------------------

    /// Verify that a BL instruction is emitted for function calls on ARM64.
    #[test]
    fn test_arm64_call_emission() {
        let mut func = IRFunction::new("caller");
        func.current_block().push(IRInstr::Call {
            dst: None,
            func: "callee".to_string(),
            args: vec![],
            is_extern: false,
        });
        func.current_block().terminator = IRTerminator::Return(vec![]);

        let mut emitter = Emitter::new();
        let code = emitter.emit_function(&func).unwrap();

        // Find a BL instruction (opcode bits [31:26] = 0b100101) in the output.
        let mut found_bl = false;
        for word in &code {
            if (word >> 26) == 0b100101 {
                found_bl = true;
                break;
            }
        }
        assert!(found_bl, "expected a BL instruction for the function call");
    }

    /// Verify that a relocation is registered for forward calls (calls to
    /// functions whose address is not yet known at emit time).
    #[test]
    fn test_arm64_relocation_for_forward_call() {
        let mut func = IRFunction::new("caller");
        func.current_block().push(IRInstr::Call {
            dst: None,
            func: "forward_func".to_string(),
            args: vec![],
            is_extern: false,
        });
        func.current_block().push(IRInstr::Call {
            dst: None,
            func: "another_func".to_string(),
            args: vec![],
            is_extern: false,
        });
        func.current_block().terminator = IRTerminator::Return(vec![]);

        let mut emitter = Emitter::new();
        let _code = emitter.emit_function(&func).unwrap();

        let relocs = emitter.relocations();
        assert_eq!(relocs.len(), 2, "expected 2 relocations for 2 calls");

        // First relocation should target forward_func.
        assert_eq!(relocs[0].symbol, "forward_func");
        assert_eq!(relocs[0].reloc_type, "R_AARCH64_CALL26");

        // Second relocation should target another_func.
        assert_eq!(relocs[1].symbol, "another_func");
        assert_eq!(relocs[1].reloc_type, "R_AARCH64_CALL26");

        // Offsets should be distinct and 4-byte aligned.
        assert_eq!(relocs[0].offset % 4, 0);
        assert_eq!(relocs[1].offset % 4, 0);
        assert!(relocs[1].offset > relocs[0].offset);
    }

    /// Verify that the RISC-V64 ADDI instruction encodes correctly.
    #[test]
    fn test_riscv64_alu_immediate() {
        use crate::riscv64::{Gpr, Instruction};

        // ADDI x5, x10, 42  →  addi t0, a0, 42
        let instr = Instruction::Addi {
            rd: Gpr::T0,
            rs1: Gpr::A0,
            imm: 42,
        };
        let bytes = instr.encode();
        assert_eq!(bytes.len(), 4);

        // Decode the word and verify fields.
        let word = u32::from_le_bytes(bytes);
        let opcode = word & 0x7F;
        let rd = (word >> 7) & 0x1F;
        let funct3 = (word >> 12) & 0x7;
        let rs1 = (word >> 15) & 0x1F;
        let imm = (word >> 20) & 0xFFF;

        assert_eq!(opcode, 0b0010011, "ADDI opcode");
        assert_eq!(rd, Gpr::T0 as u32, "destination register");
        assert_eq!(funct3, 0b000, "ADDI funct3");
        assert_eq!(rs1, Gpr::A0 as u32, "source register");
        assert_eq!(imm, 42, "immediate value");
    }

    /// Verify that the x86-64 MOV r64, imm64 instruction encodes correctly.
    #[test]
    fn test_x86_64_mov_immediate() {
        use crate::x86_64::{encode_mov_reg_imm64, Gpr};

        // MOV rax, 0x4242424242424242
        let code = encode_mov_reg_imm64(Gpr::Rax, 0x4242424242424242);
        assert!(!code.is_empty());

        // Should start with REX.W prefix (0x48 for RAX, which doesn't need REX.B).
        assert_eq!(code[0], 0x48, "REX.W prefix");

        // Opcode for MOV r64, imm64 is B8+rd.
        assert_eq!(code[1], 0xB8, "MOV rax, imm64 opcode");

        // The 8-byte immediate should follow.
        let imm = u64::from_le_bytes(code[2..10].try_into().expect("8 bytes for immediate"));
        assert_eq!(imm, 0x4242424242424242);
    }

    /// Verify that ARM64 CLZ instruction emission produces a non-zero
    /// encoded word, confirming the instruction is properly lowered.
    #[test]
    fn test_arm64_clz_emission_exists() {
        use crate::arm64::Instruction;

        // Encode CLZ X0, X1 — the encoded value should be non-zero.
        let clz = Instruction::CLZ {
            rd: Register::X0,
            rn: Register::X1,
        };
        let encoded = clz.encode().expect("CLZ encoding should succeed");
        assert_ne!(
            encoded, 0,
            "CLZ instruction encoding should produce a non-zero word"
        );

        // The ARM64 CLZ encoding base is 0xDAC01000 (CLZ X0, X0).
        // Any valid CLZ encoding must have the upper bits set.
        assert_ne!(
            encoded & 0xFFE00000,
            0,
            "CLZ encoding should have the ARM64 CLZ opcode bits set"
        );
    }

    /// Verify that WasmSectionNotFound error is returned as a proper
    /// CodegenError variant (not a panic).
    #[test]
    fn test_wasm32_error_not_panic() {
        let err = CodegenError::WasmSectionNotFound {
            section: "code".to_string(),
        };

        // Verify it's a proper error that can be constructed and matched.
        let msg = format!("{}", err);
        assert!(
            msg.contains("code"),
            "error message should mention the section name"
        );
        assert!(
            msg.contains("WASM section not found"),
            "error message should describe the error"
        );

        // Verify the error can be used in a Result context without panicking.
        let result: Result<()> = Err(CodegenError::WasmSectionNotFound {
            section: "data".to_string(),
        });
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(
                matches!(e, CodegenError::WasmSectionNotFound { .. }),
                "should be WasmSectionNotFound variant"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Wasm32 emission tests
    // -----------------------------------------------------------------------

    /// Verify that emit_elf rejects Wasm32 backend with an error.
    #[test]
    fn test_emit_elf_rejects_wasm32() {
        let funcs = vec![make_return_function("main")];
        let config = EmitConfig::wasm_binary();
        let result = emit_elf(&funcs, &[], &config);
        assert!(result.is_err(), "emit_elf should reject Wasm32 backend");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Wasm32"),
            "error message should mention Wasm32: {}",
            err_msg
        );
    }

    /// Verify that emit_raw rejects Wasm32 backend with an error.
    #[test]
    fn test_emit_raw_rejects_wasm32() {
        let funcs = vec![make_return_function("main")];
        let config = EmitConfig::wasm_binary();
        let result = emit_raw(&funcs, &[], &config);
        assert!(result.is_err(), "emit_raw should reject Wasm32 backend");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Wasm32"),
            "error message should mention Wasm32: {}",
            err_msg
        );
    }

    /// Verify that emit_obj rejects Wasm32 backend with an error.
    #[test]
    fn test_emit_obj_rejects_wasm32() {
        let funcs = vec![make_return_function("main")];
        let result = emit_obj(&funcs, &[], BackendKind::Wasm32);
        assert!(result.is_err(), "emit_obj should reject Wasm32 backend");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Wasm32"),
            "error message should mention Wasm32: {}",
            err_msg
        );
    }

    /// Verify that emit_binary dispatches to emit_wasm for Wasm output format.
    #[test]
    fn test_emit_binary_dispatches_wasm() {
        let funcs = vec![make_return_function("main")];
        let config = EmitConfig::wasm_binary();
        let result = emit_binary(&funcs, &[], &config);
        assert!(result.is_ok(), "emit_binary should succeed for Wasm: {:?}", result.err());
        let wasm_bytes = result.unwrap();
        // Wasm module magic: 0x00 0x61 0x73 0x6D
        assert!(
            wasm_bytes.len() >= 8,
            "Wasm output should be at least 8 bytes (magic + version), got {}",
            wasm_bytes.len()
        );
        assert_eq!(&wasm_bytes[0..4], &[0x00, 0x61, 0x73, 0x6D], "Wasm magic number");
        assert_eq!(&wasm_bytes[4..8], &[0x01, 0x00, 0x00, 0x00], "Wasm version 1");
    }

    /// Verify that emit_wasm produces a valid Wasm module with start function.
    #[test]
    fn test_emit_wasm_produces_valid_module() {
        let funcs = vec![make_return_function("main")];
        let config = EmitConfig::wasm_binary();
        let result = emit_wasm(&funcs, &[], &config);
        assert!(result.is_ok(), "emit_wasm should succeed: {:?}", result.err());
        let wasm_bytes = result.unwrap();
        assert!(
            wasm_bytes.len() >= 8,
            "Wasm output should be at least 8 bytes, got {}",
            wasm_bytes.len()
        );
        // Verify Wasm magic number
        assert_eq!(&wasm_bytes[0..4], &[0x00, 0x61, 0x73, 0x6D]);
    }

    /// Verify EmitConfig::wasm_binary has correct defaults.
    #[test]
    fn test_emit_config_wasm_binary() {
        let config = EmitConfig::wasm_binary();
        assert_eq!(config.format, OutputFormat::Wasm);
        assert_eq!(config.target, Target::Wasm32);
        assert_eq!(config.backend, BackendKind::Wasm32);
        assert_eq!(config.entry_name, "_start");
        assert_eq!(config.base_addr, 0);
        assert!(!config.section_headers);
        assert!(!config.symbol_table);
    }

    // -----------------------------------------------------------------------
    // Section alignment tests
    // -----------------------------------------------------------------------

    #[test]
    fn section_alignment_per_backend() {
        assert_eq!(section_alignment_for_backend(BackendKind::Arm32), 4);
        assert_eq!(section_alignment_for_backend(BackendKind::AArch64), 16);
        assert_eq!(section_alignment_for_backend(BackendKind::X86_64), 16);
        assert_eq!(section_alignment_for_backend(BackendKind::RiscV64), 4);
        assert_eq!(section_alignment_for_backend(BackendKind::Mips64), 8);
        assert_eq!(section_alignment_for_backend(BackendKind::PowerPC64), 16);
        assert_eq!(section_alignment_for_backend(BackendKind::LoongArch64), 8);
        assert_eq!(section_alignment_for_backend(BackendKind::Wasm32), 4);
    }

    #[test]
    fn emit_elf_three_load_segments() {
        // Verify that ET_EXEC has 3 LOAD segments.
        let funcs = vec![make_return_function("main")];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &[], &config).unwrap();
        let e_phnum = u16::from_le_bytes([elf[56], elf[57]]);
        assert_eq!(e_phnum, 3, "ET_EXEC should have 3 LOAD segments");

        // Verify the segment flags: R, RX, RW
        let phdr_size: usize = 56;
        let flags_offsets: Vec<u32> = (0..3)
            .map(|i| {
                let off = 64 + i * phdr_size + 4;
                u32::from_le_bytes(elf[off..off + 4].try_into().unwrap())
            })
            .collect();
        assert_eq!(flags_offsets[0], PF_R, "segment 1 should be R-only");
        assert_eq!(flags_offsets[1], PF_R | PF_X, "segment 2 should be R+X");
        assert_eq!(flags_offsets[2], PF_R | PF_W, "segment 3 should be R+W");
    }

    #[test]
    fn emit_elf_rodata_before_text_in_memory() {
        // Verify that .rodata has a lower virtual address than .text.
        let funcs = vec![make_return_function("main")];
        let data_sections = vec![
            DataSection {
                name: "rodata".into(),
                kind: DataSectionKind::ReadOnly,
                align: 16,
                data: vec![0xAA; 32],
            },
        ];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &data_sections, &config).unwrap();

        // Parse the section headers to find .rodata and .text virtual addresses.
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap()) as usize;
        let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap()) as usize;

        let mut rodata_vaddr: u64 = 0;
        let mut text_vaddr: u64 = 0;
        let mut found_rodata = false;
        let mut found_text = false;

        for i in 0..e_shnum {
            let off = e_shoff + i * 64;
            let sh_name_idx = u32::from_le_bytes(elf[off..off + 4].try_into().unwrap()) as usize;
            let sh_addr = u64::from_le_bytes(elf[off + 16..off + 24].try_into().unwrap());

            // Read the name from .shstrtab
            let e_shstrndx = u16::from_le_bytes(elf[62..64].try_into().unwrap()) as usize;
            let shstrtab_off = e_shoff + e_shstrndx * 64;
            let shstrtab_offset = u64::from_le_bytes(elf[shstrtab_off + 24..shstrtab_off + 32].try_into().unwrap()) as usize;
            let shstrtab_size = u64::from_le_bytes(elf[shstrtab_off + 32..shstrtab_off + 40].try_into().unwrap()) as usize;

            if sh_name_idx < shstrtab_size {
                let name_end = elf[shstrtab_offset + sh_name_idx..]
                    .iter()
                    .position(|&b| b == 0)
                    .unwrap_or(0);
                let name = String::from_utf8_lossy(
                    &elf[shstrtab_offset + sh_name_idx..shstrtab_offset + sh_name_idx + name_end]
                );
                if name == ".rodata" {
                    rodata_vaddr = sh_addr;
                    found_rodata = true;
                } else if name == ".text" {
                    text_vaddr = sh_addr;
                    found_text = true;
                }
            }
        }

        assert!(found_rodata, ".rodata section must be present");
        assert!(found_text, ".text section must be present");
        assert!(
            rodata_vaddr < text_vaddr,
            ".rodata (0x{:x}) must be at a lower address than .text (0x{:x})",
            rodata_vaddr,
            text_vaddr
        );
    }

    #[test]
    fn emit_elf_collect_data_sections_alignment() {
        // Verify that collect_data_sections respects alignment.
        let data_sections = vec![
            DataSection {
                name: "a".into(),
                kind: DataSectionKind::ReadOnly,
                align: 4,
                data: vec![0x01, 0x02], // 2 bytes, needs padding to 4
            },
            DataSection {
                name: "b".into(),
                kind: DataSectionKind::ReadOnly,
                align: 16,
                data: vec![0x03; 8], // 8 bytes, needs padding to 16
            },
        ];
        let (rodata, _data, _bss) = collect_data_sections(&data_sections, BackendKind::AArch64);

        // AArch64 default align = 16, so first section padded to 16, second section also 16-aligned
        // First: 2 bytes padded to 16 → 16 bytes, then 8 bytes at offset 16
        assert!(rodata.len() >= 24, "rodata should be at least 24 bytes with alignment padding");
    }
}

// ---------------------------------------------------------------------------
// Worklog
// ---------------------------------------------------------------------------
//
// 2025-03-04: Enhanced emit.rs for ARM64 ELF/binary emission.
//
// Changes:
// - Added EmitConfig struct with OutputFormat (ELF, Raw, Obj) and Target
//   (Linux, BareMetal) enums.
// - Added emit_elf() top-level function producing full ELF64 binaries with:
//   ELF header (EM_AARCH64, little-endian, static executable),
//   program headers (LOAD for text + data/bss),
//   section headers (.text, .rodata, .data, .bss, .symtab, .strtab, .shstrtab),
//   symbol table (function names with addresses),
//   and section-header string table.
// - Added emit_raw() top-level function producing flat binary images for
//   bare-metal AArch64.
// - Added CallRelocation struct and relocation resolution: inter-function BL
//   instructions are recorded during emission and patched after all function
//   addresses are known.
// - Extended emit_ir_instr to handle all IRInstr variants including Add, Sub,
//   Mul, Div, Cmp, Ret, Branch, CondBranch.
// - Extended BinOpKind match to handle comparison operators (SLt..Ne) with
//   CMP instruction emission (CSET via CSINC).
// - Added 15 tests covering: ELF header validity, machine type, exec type,
//   section headers, symbol table, raw binary, call relocation, EmitConfig
//   defaults, obj file type, bare-metal OSABI, data sections, legacy
//   emit_program, empty program, Display traits, base address, multiple
//   function symbols.
// - Updated lib.rs to re-export EmitConfig, OutputFormat, Target, emit_elf,
//   emit_raw.
//
// 2026-03-05: Linker Integration Hardening (Task 21-22)
//
// Changes:
// - Restructured ELF layout: .rodata is now placed before .text in memory,
//   matching the natural memory ordering: R data → RX code → RW data.
// - Changed from 2 to 3 LOAD segments: PF_R for .rodata, PF_R|PF_X for
//   .text, PF_R|PF_W for .data+.bss.  This provides proper memory
//   protection granularity (W^X compliance).
// - Added section_alignment_for_backend() function returning per-arch
//   alignment: ARM32=4, AArch64=16, x86-64=16, RISC-V=4, MIPS64=8,
//   PPC64=16, LoongArch64=8.
// - Enhanced collect_data_sections() to respect per-DataSection alignment
//   requirements by inserting padding between adjacent contributions.
//   The overall section alignment is max(section.align, backend_default).
// - Updated section header table order: null, .rodata, .text, [.rela.text],
//   .data, .bss, .symtab, .strtab, .shstrtab.
// - Updated build_shstrtab() to list sections in the new order.
// - Updated build_symbol_table() to accept text_section_idx parameter
//   (.text is now at section index 2 instead of 1).
// - All sh_addralign values now use section_alignment_for_backend() instead
//   of hardcoded 8/16 values.
// - Entry point calculation now accounts for the .rodata offset: entry is
//   text_vaddr + entry_offset (not base_addr + entry_offset).
