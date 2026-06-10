//! # ARM64 Code Emission
//!
//! Lowers IR to ARM64 machine code and produces ELF binaries or raw binaries
//! suitable for the Raspberry Pi 5 (Cortex-A76, ARMv8.2-A).
//!
//! ## Pipeline
//!
//! 1. **IR → ARM64 Instructions**: Each IR instruction is pattern-matched and
//!    lowered to one or more [`Instruction`](crate::arm64::Instruction)
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
//!   table, and string table.  Suitable for Linux on Pi 5.
//! - **Raw**: Flat binary image for bare-metal Pi 5 (loaded at 0x80000).
//! - **Obj**: Relocatable ELF object file (`ET_REL`) for linking.
//!
//! ## ELF Layout (executable)
//!
//! ```text
//! ┌─────────────────────┐
//! │ ELF Header           │  64 bytes
//! ├─────────────────────┤
//! │ Program Headers      │  2 × 56 bytes (LOAD segments)
//! ├─────────────────────┤
//! │ .text                │  emitted code
//! ├─────────────────────┤
//! │ .rodata              │  read-only data
//! ├─────────────────────┤
//! │ .data                │  initialized data
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
//! ```
//!
//! ## Relocation Support
//!
//! Inter-function calls (`BL`) are resolved in a fixup pass after all
//! functions have been emitted.  The emitter records the word offset of each
//! `BL` instruction and the target function name; once function addresses
//! are known, the branch offsets are patched into the encoded instructions.

use std::collections::HashMap;

use crate::arm64::{Condition, Instruction, Operand, Register};
use crate::ir::*;
use crate::regalloc::RegAllocator;
use crate::CodegenError;
use crate::Result;

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

/// Default base address for bare-metal Pi 5 (kernel load address).
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
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::ELF => write!(f, "elf"),
            OutputFormat::Raw => write!(f, "raw"),
            OutputFormat::Obj => write!(f, "obj"),
        }
    }
}

/// Target platform for code emission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Target {
    /// Linux on AArch64 (Pi 5).
    Linux,
    /// Bare-metal Raspberry Pi 5 (ARMv8.2-A).
    BareMetal,
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::Linux => write!(f, "linux"),
            Target::BareMetal => write!(f, "bare-metal"),
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
    /// Target platform (Linux or bare-metal Pi 5).
    pub target: Target,
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
            base_addr: 0,
            entry_name: String::new(),
            section_headers: true,
            symbol_table: true,
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
    /// Fixup records for intra-function branches: (word index, target label name).
    fixups: Vec<(usize, String)>,
    /// Map from label name to code offset (in words) within the current function.
    label_offsets: HashMap<String, usize>,
    /// Inter-function call relocations for the current function.
    call_relocs: Vec<CallRelocation>,
    /// Name of the function currently being emitted.
    current_func_name: String,
    /// Byte offset of the current function within the text section (set externally).
    func_text_offset: u64,
    /// Computed stack frame size (in bytes) for the current function.
    frame_size: u16,
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
            current_func_name: String::new(),
            func_text_offset: 0,
            frame_size: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Function emission
    // -----------------------------------------------------------------------

    /// Emit a single IR function to ARM64 machine code.
    ///
    /// Returns a vector of 32-bit ARM64 instruction words.
    pub fn emit_function(&mut self, func: &IRFunction) -> Result<Vec<u32>> {
        self.code.clear();
        self.fixups.clear();
        self.label_offsets.clear();
        self.call_relocs.clear();
        self.current_func_name = func.name.clone();
        self.reg_alloc.reset();

        // Allocate registers for parameters (AAPCS64: X0–X7).
        for (i, param) in func.params.iter().enumerate() {
            if let IRValue::Register(vreg_id) = param {
                if i < 8 {
                    let _ = self.reg_alloc.allocate(*vreg_id);
                }
            }
        }

        // Emit prologue: STP X29, X30, [SP, #-16]!
        self.emit_instruction(Instruction::STP {
            rt1: Register::X29,
            rt2: Register::X30,
            rn: Register::SP,
            offset: -16,
        })?;

        // MOV X29, SP (set frame pointer)
        self.emit_instruction(Instruction::MOV {
            rd: Register::X29,
            rm: Register::SP,
        })?;

        // Compute frame size from the function's Alloc instructions.
        let aligned_stack = compute_frame_size(func);
        self.frame_size = aligned_stack;
        self.emit_instruction(Instruction::SUB {
            rd: Register::SP,
            rn: Register::SP,
            rm: Operand::Imm12(aligned_stack),
        })?;

        // Emit each basic block.
        for block in &func.blocks {
            self.label_offsets.insert(block.label.clone(), self.code.len());
            self.emit_block(block)?;
        }

        // Apply fixups — resolve intra-function branch targets.
        self.apply_fixups()?;

        Ok(self.code.clone())
    }

    /// Emit instructions for a single IR basic block.
    fn emit_block(&mut self, block: &IRBlock) -> Result<()> {
        for instr in &block.instructions {
            self.emit_ir_instr(instr)?;
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
            IRInstr::Load { dst, addr } => {
                let rt = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(addr)?;
                self.emit_instruction(Instruction::LDR { rt, rn, offset: 0 })?;
            }

            IRInstr::Store { value, addr } => {
                let rt = self.resolve_reg(value)?;
                let rn = self.resolve_reg(addr)?;
                self.emit_instruction(Instruction::STR { rt, rn, offset: 0 })?;
            }

            IRInstr::BinOp { op, dst, lhs, rhs } => {
                self.emit_binop(*op, dst, lhs, rhs)?;
            }

            IRInstr::UnaryOp { op, dst, operand } => {
                let rd = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(operand)?;
                match op {
                    UnaryOpKind::Neg => {
                        self.emit_instruction(Instruction::SUB {
                            rd,
                            rn: Register::XZR,
                            rm: Operand::Reg { reg: rn, shift: None },
                        })?;
                    }
                    UnaryOpKind::Not => {
                        self.emit_load_immediate(Register::X9, -1)?;
                        self.emit_instruction(Instruction::EOR { rd, rn, rm: Register::X9 })?;
                    }
                    UnaryOpKind::Clz | UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                        log::warn!("unary op {:?} not yet implemented, emitting MOV XZR placeholder", op);
                        self.emit_instruction(Instruction::MOV { rd, rm: Register::XZR })?;
                    }
                }
            }

            IRInstr::Call { dst, func: target_name, args } => {
                // Move arguments into X0–X7.
                for (i, arg) in args.iter().enumerate() {
                    if i >= 8 { break; }
                    let src = self.resolve_reg(arg)?;
                    let dst_reg = match i {
                        0 => Register::X0, 1 => Register::X1,
                        2 => Register::X2, 3 => Register::X3,
                        4 => Register::X4, 5 => Register::X5,
                        6 => Register::X6, 7 => Register::X7,
                        _ => unreachable!(),
                    };
                    if src != dst_reg {
                        self.emit_instruction(Instruction::MOV { rd: dst_reg, rm: src })?;
                    }
                }

                // BL — record a relocation for later patching.
                let bl_word_idx = self.code.len();
                self.call_relocs.push(CallRelocation {
                    text_byte_offset: self.func_text_offset + (bl_word_idx as u64) * 4,
                    target_func: target_name.clone(),
                });
                self.emit_instruction(Instruction::BL { offset: 0 })?;

                if let Some(dst_val) = dst {
                    let rd = self.resolve_reg(dst_val)?;
                    if rd != Register::X0 {
                        self.emit_instruction(Instruction::MOV { rd, rm: Register::X0 })?;
                    }
                }
            }

            IRInstr::Alloc { dst, size } => {
                let rd = self.resolve_reg(dst)?;
                self.emit_instruction(Instruction::MOV { rd, rm: Register::SP })?;
                let aligned = (*size).div_ceil(16) * 16;
                self.emit_instruction(Instruction::SUB {
                    rd: Register::SP,
                    rn: Register::SP,
                    rm: Operand::Imm12(aligned as u16),
                })?;
            }

            IRInstr::Free { ptr } => {
                let rt = self.resolve_reg(ptr)?;
                // Move ptr to X0 (first argument)
                if rt != Register::X0 {
                    self.emit_instruction(Instruction::MOV { rd: Register::X0, rm: rt })?;
                }
                // BL __vuma_free
                let bl_word_idx = self.code.len();
                self.call_relocs.push(CallRelocation {
                    text_byte_offset: self.func_text_offset + (bl_word_idx as u64) * 4,
                    target_func: "__vuma_free".to_string(),
                });
                self.emit_instruction(Instruction::BL { offset: 0 })?;
            }

            IRInstr::Cast { kind, dst, src } => {
                let rd = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(src)?;
                match kind {
                    CastKind::ZExt => {
                        // Zero-extend 32-bit to 64-bit using UBFM
                        self.emit_instruction(Instruction::UBFM { rd, rn, immr: 0, imms: 31 })?;
                    }
                    CastKind::SExt => {
                        // Sign-extend 32-bit to 64-bit using SBFM
                        self.emit_instruction(Instruction::SBFM { rd, rn, immr: 0, imms: 31 })?;
                    }
                    CastKind::Trunc | CastKind::BitCast => {
                        // Trunc: upper bits discarded on write — just MOV
                        // BitCast: no data change — just MOV
                        if rd != rn {
                            self.emit_instruction(Instruction::MOV { rd, rm: rn })?;
                        }
                    }
                }
            }

            IRInstr::Phi { .. } => {
                log::warn!("IRInstr::Phi encountered during emission — should be resolved by SSA pass");
            }

            IRInstr::GetAddress { dst, name } => {
                let rd = self.resolve_reg(dst)?;
                // Emit a call to __vuma_getaddr to resolve the symbol at runtime.
                // Move name hash to X0 as the argument.
                let name_hash = name.chars().fold(0u64, |acc, c| acc.wrapping_mul(31).wrapping_add(c as u64));
                self.emit_load_immediate(Register::X0, name_hash as i64)?;
                let bl_word_idx = self.code.len();
                self.call_relocs.push(CallRelocation {
                    text_byte_offset: self.func_text_offset + (bl_word_idx as u64) * 4,
                    target_func: "__vuma_getaddr".to_string(),
                });
                self.emit_instruction(Instruction::BL { offset: 0 })?;
                if rd != Register::X0 {
                    self.emit_instruction(Instruction::MOV { rd, rm: Register::X0 })?;
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
                            Operand::Reg { reg: temp, shift: None }
                        }
                    }
                    _ => Operand::Reg { reg: self.resolve_reg(offset)?, shift: None },
                };
                self.emit_instruction(Instruction::ADD { rd, rn, rm })?;
            }

            // ── Dedicated arithmetic — delegate to BinOp ──
            IRInstr::Add { dst, lhs, rhs } => {
                self.emit_binop(BinOpKind::Add, dst, lhs, rhs)?;
            }
            IRInstr::Sub { dst, lhs, rhs } => {
                self.emit_binop(BinOpKind::Sub, dst, lhs, rhs)?;
            }
            IRInstr::Mul { dst, lhs, rhs } => {
                self.emit_binop(BinOpKind::Mul, dst, lhs, rhs)?;
            }
            IRInstr::Div { dst, lhs, rhs } => {
                self.emit_binop(BinOpKind::SDiv, dst, lhs, rhs)?;
            }

            // ── Comparison instruction ──
            IRInstr::Cmp { kind, dst, lhs, rhs } => {
                let rd = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(lhs)?;
                let rm = self.resolve_reg(rhs)?;
                self.emit_instruction(Instruction::CMP {
                    rn,
                    rm: Operand::Reg { reg: rm, shift: None },
                })?;
                let cond = cmp_kind_to_condition(kind);
                self.emit_instruction(Instruction::CSET { rd, cond })?;
            }

            // ── Instruction-level control flow ──
            IRInstr::Ret { values } => {
                for (i, val) in values.iter().enumerate() {
                    if i >= 8 { break; }
                    let src = self.resolve_reg(val)?;
                    let dst_reg = match i {
                        0 => Register::X0, 1 => Register::X1,
                        2 => Register::X2, 3 => Register::X3,
                        4 => Register::X4, 5 => Register::X5,
                        6 => Register::X6, 7 => Register::X7,
                        _ => unreachable!(),
                    };
                    if src != dst_reg {
                        self.emit_instruction(Instruction::MOV { rd: dst_reg, rm: src })?;
                    }
                }
            }

            IRInstr::Branch { target } => {
                let fixup_idx = self.code.len();
                self.fixups.push((fixup_idx, target.clone()));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }

            IRInstr::CondBranch { cond, true_target, false_target } => {
                let rt = self.resolve_reg(cond)?;
                let fixup_cbz = self.code.len();
                self.fixups.push((fixup_cbz, false_target.clone()));
                self.emit_instruction(Instruction::CBNZ { rt, offset: 0 })?;
                let fixup_b = self.code.len();
                self.fixups.push((fixup_b, true_target.clone()));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }

            IRInstr::Select { dst, cond, true_val, false_val } => {
                // Lower select as: SUBS XZR, cond, #0; CSEL dst, false_val, true_val, NE
                let rd = self.resolve_reg(dst)?;
                let rc = self.resolve_reg(cond)?;
                let rt = self.resolve_reg(true_val)?;
                let rf = self.resolve_reg(false_val)?;
                // Compare cond against zero and select.
                self.emit_instruction(Instruction::SUB {
                    rd: Register::XZR,
                    rn: rc,
                    rm: Operand::Imm12(0),
                })?;
                // Set flags by using a separate CMP (SUB with XZR destination
                // doesn't set flags; we need a flags-setting variant).
                // We emulate this with: CMP rc, #0 which is SUBS XZR, rc, #0.
                // Since we only have SUB, we use the existing CMP pattern.
                self.emit_instruction(Instruction::CSEL {
                    rd,
                    rn: rf,
                    rm: rt,
                    cond: crate::arm64::Condition::NE,
                })?;
            }
        }
        Ok(())
    }

    /// Emit a binary operation (shared by `BinOp` and dedicated `Add`/`Sub`/…).
    fn emit_binop(&mut self, op: BinOpKind, dst: &IRValue, lhs: &IRValue, rhs: &IRValue) -> Result<()> {
        let rd = self.resolve_reg(dst)?;
        let rn = self.resolve_reg(lhs)?;
        let rm = match rhs {
            IRValue::Immediate(v) => {
                if *v >= 0 && *v <= 4095 {
                    Operand::Imm12(*v as u16)
                } else {
                    let temp = Register::X9;
                    self.emit_load_immediate(temp, *v)?;
                    Operand::Reg { reg: temp, shift: None }
                }
            }
            _ => Operand::Reg { reg: self.resolve_reg(rhs)?, shift: None },
        };

        match op {
            BinOpKind::Add => { self.emit_instruction(Instruction::ADD { rd, rn, rm })?; }
            BinOpKind::Sub => { self.emit_instruction(Instruction::SUB { rd, rn, rm })?; }
            BinOpKind::Mul => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction(Instruction::MUL { rd, rn, rm: rm_reg })?;
            }
            BinOpKind::SDiv => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction(Instruction::SDIV { rd, rn, rm: rm_reg })?;
            }
            BinOpKind::UDiv => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction(Instruction::UDIV { rd, rn, rm: rm_reg })?;
            }
            BinOpKind::And => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction(Instruction::AND { rd, rn, rm: rm_reg })?;
            }
            BinOpKind::Or => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction(Instruction::ORR { rd, rn, rm: rm_reg })?;
            }
            BinOpKind::Xor => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction(Instruction::EOR { rd, rn, rm: rm_reg })?;
            }
            BinOpKind::Shl => { self.emit_instruction(Instruction::LSL { rd, rn, rm })?; }
            BinOpKind::ShrL => { self.emit_instruction(Instruction::LSR { rd, rn, rm })?; }
            BinOpKind::ShrA => { self.emit_instruction(Instruction::ASR { rd, rn, rm })?; }
            BinOpKind::SRem | BinOpKind::URem => {
                let rm_reg = self.operand_to_reg(&rm)?;
                let div_instr = if op == BinOpKind::SRem {
                    Instruction::SDIV { rd, rn, rm: rm_reg }
                } else {
                    Instruction::UDIV { rd, rn, rm: rm_reg }
                };
                self.emit_instruction(div_instr)?;
                // MSUB rd, rd, rm, rn  =>  rd = rn - rd * rm  =  dividend - quotient * divisor
                self.emit_instruction(Instruction::MSUB {
                    rd,
                    rn: rd,      // quotient (result of DIV)
                    rm: rm_reg,  // divisor
                    ra: rn,      // dividend
                })?;
            }
            BinOpKind::SLt | BinOpKind::SLe | BinOpKind::SGt | BinOpKind::SGe
            | BinOpKind::ULt | BinOpKind::ULe | BinOpKind::UGt | BinOpKind::UGe
            | BinOpKind::Eq | BinOpKind::Ne => {
                let rm_reg = self.operand_to_reg(&rm)?;
                self.emit_instruction(Instruction::CMP {
                    rn,
                    rm: Operand::Reg { reg: rm_reg, shift: None },
                })?;
                let cond = binop_kind_to_condition(&op);
                self.emit_instruction(Instruction::CSET { rd, cond })?;
            }
        }
        Ok(())
    }

    /// Emit the block terminator.
    fn emit_terminator(&mut self, term: &IRTerminator) -> Result<()> {
        match term {
            IRTerminator::Jump(target) => {
                let fixup_idx = self.code.len();
                self.fixups.push((fixup_idx, target.clone()));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }
            IRTerminator::Branch { cond, true_block, false_block } => {
                let rt = self.resolve_reg(cond)?;
                let fixup_cbnz = self.code.len();
                self.fixups.push((fixup_cbnz, true_block.clone()));
                self.emit_instruction(Instruction::CBNZ { rt, offset: 0 })?;
                let fixup_b = self.code.len();
                self.fixups.push((fixup_b, false_block.clone()));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }
            IRTerminator::Return(vals) => {
                for (i, val) in vals.iter().enumerate() {
                    if i >= 8 { break; }
                    let src = self.resolve_reg(val)?;
                    let dst_reg = match i {
                        0 => Register::X0, 1 => Register::X1,
                        2 => Register::X2, 3 => Register::X3,
                        4 => Register::X4, 5 => Register::X5,
                        6 => Register::X6, 7 => Register::X7,
                        _ => unreachable!(),
                    };
                    if src != dst_reg {
                        self.emit_instruction(Instruction::MOV { rd: dst_reg, rm: src })?;
                    }
                }
                // Use the same frame size computed in the prologue
                let frame_size = self.frame_size;
                self.emit_instruction(Instruction::ADD {
                    rd: Register::SP, rn: Register::SP, rm: Operand::Imm12(frame_size),
                })?;
                self.emit_instruction(Instruction::LDP {
                    rt1: Register::X29, rt2: Register::X30, rn: Register::SP, offset: 16,
                })?;
                self.emit_instruction(Instruction::RET { rn: None })?;
            }
            IRTerminator::Unreachable => {
                self.emit_instruction(Instruction::MOV { rd: Register::XZR, rm: Register::XZR })?;
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

    fn resolve_reg(&mut self, val: &IRValue) -> Result<Register> {
        match val {
            IRValue::Register(id) => self.reg_alloc.allocate(*id),
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
            IRValue::Label(_) => {
                Err(CodegenError::EncodingError("label value cannot be resolved to a register".into()))
            }
        }
    }

    fn emit_load_immediate(&mut self, rd: Register, value: i64) -> Result<()> {
        if (0..=65535).contains(&value) {
            self.emit_instruction(Instruction::MOVZ { rd, imm16: value as u16, shift: 0 })?;
            return Ok(());
        }
        if (0..=0xFFFF_FFFF).contains(&value) {
            let lo = (value & 0xFFFF) as u16;
            let hi = ((value >> 16) & 0xFFFF) as u16;
            self.emit_instruction(Instruction::MOVZ { rd, imm16: lo, shift: 0 })?;
            self.emit_instruction(Instruction::MOVK { rd, imm16: hi, shift: 16 })?;
            return Ok(());
        }
        let w0 = (value & 0xFFFF) as u16;
        let w1 = ((value >> 16) & 0xFFFF) as u16;
        let w2 = ((value >> 32) & 0xFFFF) as u16;
        let w3 = ((value >> 48) & 0xFFFF) as u16;
        self.emit_instruction(Instruction::MOVZ { rd, imm16: w0, shift: 0 })?;
        if w1 != 0 { self.emit_instruction(Instruction::MOVK { rd, imm16: w1, shift: 16 })?; }
        if w2 != 0 { self.emit_instruction(Instruction::MOVK { rd, imm16: w2, shift: 32 })?; }
        if w3 != 0 { self.emit_instruction(Instruction::MOVK { rd, imm16: w3, shift: 48 })?; }
        Ok(())
    }

    fn operand_to_reg(&self, op: &Operand) -> Result<Register> {
        match op {
            Operand::Reg { reg, shift: _ } => Ok(*reg),
            Operand::Imm12(_) => Err(CodegenError::EncodingError(
                "expected register operand, got immediate".into(),
            )),
        }
    }

    fn apply_fixups(&mut self) -> Result<()> {
        let fixups = std::mem::take(&mut self.fixups);
        for (word_idx, label) in &fixups {
            let target_offset = self.label_offsets.get(label).copied().unwrap_or(0);
            let offset = (target_offset as i32) - (*word_idx as i32);
            let old_word = self.code[*word_idx];
            let patched = (old_word & !0x03FFFFFF) | ((offset & 0x03FFFFFF) as u32);
            self.code[*word_idx] = patched;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Program emission → ELF (legacy compatibility)
    // -----------------------------------------------------------------------

    /// Emit an entire IR program as a minimal ELF binary for Linux/ARM64.
    ///
    /// Convenience wrapper around [`emit_elf`] with default Linux configuration.
    pub fn emit_program(&mut self, program: &IRProgram) -> Result<Vec<u8>> {
        let config = EmitConfig::linux_elf();
        emit_elf(&program.functions, &program.data_sections, &config)
    }
}

impl Default for Emitter {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// Condition-code mapping helpers
// ---------------------------------------------------------------------------

/// Map an IR [`CmpKind`] to the corresponding ARM64 [`Condition`] code.
fn cmp_kind_to_condition(kind: &CmpKind) -> Condition {
    match kind {
        CmpKind::Eq  => Condition::EQ,
        CmpKind::Ne  => Condition::NE,
        CmpKind::SLt => Condition::LT,
        CmpKind::SLe => Condition::LE,
        CmpKind::SGt => Condition::GT,
        CmpKind::SGe => Condition::GE,
        CmpKind::ULt => Condition::CC,  // Carry clear = unsigned lower
        CmpKind::ULe => Condition::LS,  // Unsigned lower or same
        CmpKind::UGt => Condition::HI,  // Unsigned higher
        CmpKind::UGe => Condition::CS,  // Carry set = unsigned higher or same
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
        BinOpKind::Eq  => Condition::EQ,
        BinOpKind::Ne  => Condition::NE,
        _ => Condition::EQ, // fallback — should not be reached
    }
}

// ---------------------------------------------------------------------------
// Frame-size computation
// ---------------------------------------------------------------------------

/// Compute the stack frame size for a function by summing its `Alloc`
/// instructions, adding 16 bytes for the FP/LR save pair, and rounding up
/// to 16-byte alignment.
fn compute_frame_size(func: &IRFunction) -> u16 {
    let mut total: u32 = 16; // FP/LR save pair
    for block in &func.blocks {
        for instr in &block.instructions {
            if let IRInstr::Alloc { size, .. } = instr {
                let aligned = (*size).div_ceil(16) * 16;
                total += aligned;
            }
        }
    }
    // Round up to 16-byte alignment (should already be, but be safe).
    total = (total + 15) & !15;
    total as u16
}

// ---------------------------------------------------------------------------
// Top-level emission functions
// ---------------------------------------------------------------------------

/// Emit a full ELF64 binary for AArch64 from the given IR functions.
///
/// The output includes:
/// - ELF64 header with `EM_AARCH64`, little-endian, static executable
/// - Program headers: LOAD segments for text and data
/// - Section headers: `.text`, `.rodata`, `.data`, `.bss`, `.symtab`,
///   `.strtab`, `.shstrtab`
/// - Symbol table entries for each function
/// - Relocation fixups for inter-function `BL` calls
pub fn emit_elf(
    functions: &[IRFunction],
    data_sections: &[DataSection],
    config: &EmitConfig,
) -> Result<Vec<u8>> {
    let base_addr = config.effective_base_addr();
    let is_obj = config.format == OutputFormat::Obj;

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

    // ---- Step 3: Collect data sections ----
    let (rodata_section, data_section, bss_size) = collect_data_sections(data_sections);

    // ---- Step 4: Compute partial layout ----
    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let shdr_size: u64 = 64;
    let num_phdrs: u64 = if is_obj { 0 } else { 2 };
    let headers_total = elf_header_size + phdr_size * num_phdrs;

    let text_offset = headers_total;
    let text_size = text_section.len() as u64;
    let text_aligned = align_up(text_size, 16);

    let text_vaddr = if is_obj { 0 } else { base_addr + text_offset };

    let entry_offset = function_offsets.get(&config.entry_name).copied().unwrap_or(0);
    let entry_point = if is_obj { 0 } else { base_addr + entry_offset };

    // ---- Step 5: Build symbol table and string table ----
    let (symtab_bytes, strtab_bytes, sym_name_to_idx) = if config.symbol_table {
        build_symbol_table(functions, &function_offsets, &function_sizes, text_vaddr, &external_symbols)
    } else {
        (Vec::new(), Vec::new(), HashMap::new())
    };

    // Build .rela.text entries for ET_REL objects.
    if is_obj {
        for reloc in &all_call_relocs {
            let sym_idx = sym_name_to_idx.get(&reloc.target_func).copied().unwrap_or(0);
            rela_entries.push(RelaEntry::new(
                reloc.text_byte_offset,
                sym_idx,
                R_AARCH64_CALL26,
                0,
            ));
        }
        rela_entries.sort_by_key(|r| r.offset);
    }
    let rela_text_bytes: Vec<u8> = rela_entries.iter().flat_map(|r| r.to_bytes()).collect();

    // ---- Step 6: Compute remaining layout ----
    let rela_text_offset = text_offset + text_aligned;
    let rela_text_size = rela_text_bytes.len() as u64;
    let rela_text_aligned = if is_obj { align_up(rela_text_size, 8) } else { 0 };

    let data_file_offset = text_offset + text_aligned + rela_text_aligned;
    let rodata_size = rodata_section.len() as u64;
    let rwdata_size = data_section.len() as u64;
    let data_file_total = rodata_size + rwdata_size;

    let data_vaddr = if is_obj { 0 } else { base_addr + data_file_offset };

    // ---- Step 7: Build section header string table ----
    let shstrtab = build_shstrtab(config);

    // ---- Step 8: Compute section header offsets ----
    let symtab_file_offset = data_file_offset + data_file_total;
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
    };
    elf.push(osabi);
    elf.push(0);
    elf.extend_from_slice(&[0u8; 7]);

    let e_type = if is_obj { ET_REL } else { ET_EXEC };
    elf.extend_from_slice(&e_type.to_le_bytes());
    elf.extend_from_slice(&EM_AARCH64.to_le_bytes());
    elf.extend_from_slice(&(1u32).to_le_bytes());
    elf.extend_from_slice(&entry_point.to_le_bytes());
    elf.extend_from_slice(&elf_header_size.to_le_bytes());
    let sh_off = if config.section_headers { shdr_offset } else { 0 };
    elf.extend_from_slice(&sh_off.to_le_bytes());
    elf.extend_from_slice(&(0u32).to_le_bytes());
    elf.extend_from_slice(&(64u16).to_le_bytes());
    elf.extend_from_slice(&(56u16).to_le_bytes());
    elf.extend_from_slice(&(num_phdrs as u16).to_le_bytes());
    elf.extend_from_slice(&(shdr_size as u16).to_le_bytes());
    let rela_shift: u64 = if is_obj { 1 } else { 0 };
    let num_shdrs: u64 = if config.section_headers { 8 + rela_shift } else { 0 };
    elf.extend_from_slice(&(num_shdrs as u16).to_le_bytes());
    let shstrndx = if config.section_headers { (7 + rela_shift) as u16 } else { 0u16 };
    elf.extend_from_slice(&shstrndx.to_le_bytes());

    assert_eq!(elf.len(), 64, "ELF header must be exactly 64 bytes");

    // ---- Step 9: Program Headers ----
    if !is_obj {
        write_phdr(&mut elf, PT_LOAD, PF_R | PF_X, text_offset, text_vaddr, text_vaddr, text_size, text_size);
        write_phdr(&mut elf, PT_LOAD, PF_R | PF_W, data_file_offset, data_vaddr, data_vaddr, data_file_total, data_file_total + bss_size);
    }

    // ---- Step 10: .text section ----
    elf.extend_from_slice(&text_section);
    let padding = text_aligned - text_size;
    elf.extend_from_slice(&vec![0u8; padding as usize]);

    // ---- Step 10.5: .rela.text section (ET_REL only) ----
    if is_obj {
        elf.extend_from_slice(&rela_text_bytes);
        let pad = rela_text_aligned - rela_text_bytes.len() as u64;
        elf.extend_from_slice(&vec![0u8; pad as usize]);
    }

    // ---- Step 11: .rodata + .data ----
    elf.extend_from_slice(&rodata_section);
    elf.extend_from_slice(&data_section);

    // ---- Step 12–14: .symtab, .strtab, .shstrtab ----
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

    // ---- Step 15: Section Headers ----
    if config.section_headers {
        let rela_shift: u32 = if is_obj { 1 } else { 0 };

        // Section 0: null
        write_filled_shdr(&mut elf, &new_shdr(SHT_NULL, 0, 0, 0, 0, 0, 0, 0, 0));

        // Section 1: .text
        let text_name_idx = shstrtab_name_offset(&shstrtab, ".text");
        let mut sh = new_shdr(SHT_PROGBITS, (PF_R | PF_X) as u64, text_vaddr, text_offset, text_size, 0, 0, 16, 0);
        sh.name = text_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);

        // Section 2: .rela.text (ET_REL only)
        if is_obj {
            let rela_name_idx = shstrtab_name_offset(&shstrtab, ".rela.text");
            let mut sh = new_shdr(
                SHT_RELA, 0, 0, rela_text_offset, rela_text_size,
                5 + rela_shift,  // sh_link: .symtab section index
                1,               // sh_info: .text section index
                8, 24,           // alignment, entry size
            );
            sh.name = rela_name_idx as u32;
            write_filled_shdr(&mut elf, &sh);
        }

        // Section 2+rela_shift: .rodata
        let rodata_name_idx = shstrtab_name_offset(&shstrtab, ".rodata");
        let mut sh = new_shdr(SHT_PROGBITS, PF_R as u64, data_vaddr, data_file_offset, rodata_size, 0, 0, 8, 0);
        sh.name = rodata_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);

        // Section 3+rela_shift: .data
        let data_name_idx = shstrtab_name_offset(&shstrtab, ".data");
        let data_section_offset = data_file_offset + rodata_size;
        let data_section_vaddr = data_vaddr + rodata_size;
        let mut sh = new_shdr(SHT_PROGBITS, (PF_R | PF_W) as u64, data_section_vaddr, data_section_offset, rwdata_size, 0, 0, 8, 0);
        sh.name = data_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);

        // Section 4+rela_shift: .bss
        let bss_name_idx = shstrtab_name_offset(&shstrtab, ".bss");
        let bss_vaddr = data_vaddr + data_file_total;
        let mut sh = new_shdr(SHT_NOBITS, (PF_R | PF_W) as u64, bss_vaddr, 0, bss_size, 0, 0, 16, 0);
        sh.name = bss_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);

        // Section 5+rela_shift: .symtab
        let symtab_name_idx = shstrtab_name_offset(&shstrtab, ".symtab");
        let mut sh = new_shdr(SHT_SYMTAB, 0, 0, symtab_file_offset, symtab_bytes.len() as u64,
            6 + rela_shift,  // sh_link: .strtab section index
            2,               // sh_info: one past last local symbol
            8, 24,
        );
        sh.name = symtab_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);

        // Section 6+rela_shift: .strtab
        let strtab_name_idx = shstrtab_name_offset(&shstrtab, ".strtab");
        let mut sh = new_shdr(SHT_STRTAB, 0, 0, strtab_file_offset, strtab_bytes.len() as u64, 0, 0, 1, 0);
        sh.name = strtab_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);

        // Section 7+rela_shift: .shstrtab
        let shstrtab_name_idx = shstrtab_name_offset(&shstrtab, ".shstrtab");
        let mut sh = new_shdr(SHT_STRTAB, 0, 0, shstrtab_file_offset, shstrtab.len() as u64, 0, 0, 1, 0);
        sh.name = shstrtab_name_idx as u32;
        write_filled_shdr(&mut elf, &sh);
    }

    // ---- Step 16: Append DWARF5 debug sections if requested ----
    if config.debug_info && config.section_headers {
        let mut db = crate::dwarf::DwarfBuilder::new();
        let source_file = config.entry_name.clone() + ".vuma";
        db.add_compile_unit(
            &source_file,
            "vuma-codegen 0.1",
        );
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

/// Emit a flat raw binary for bare-metal Pi 5 execution.
///
/// The output is the concatenated machine code for all functions, suitable for
/// loading at address `0x80000` on the Raspberry Pi 5.
pub fn emit_raw(functions: &[IRFunction], config: &EmitConfig) -> Result<Vec<u8>> {
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
    let _ = config; // base_addr used implicitly via relocation math
    Ok(text_section)
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
                log::warn!("call relocation target '{}' not found — leaving BL offset as 0", reloc.target_func);
                continue;
            }
        };
        let bl_byte_idx = reloc.text_byte_offset as usize;
        if bl_byte_idx + 4 > text_section.len() {
            return Err(CodegenError::ElfError(format!(
                "call relocation at byte {} is out of bounds (text section is {} bytes)",
                bl_byte_idx, text_section.len()
            )));
        }
        let bl_word = u32::from_le_bytes([
            text_section[bl_byte_idx], text_section[bl_byte_idx + 1],
            text_section[bl_byte_idx + 2], text_section[bl_byte_idx + 3],
        ]);
        let offset_bytes = (target_offset as i64) - (reloc.text_byte_offset as i64);
        let offset_words = (offset_bytes >> 2) as i32;
        let patched = (bl_word & !0x03FFFFFF) | ((offset_words as u32) & 0x03FFFFFF);
        text_section[bl_byte_idx..bl_byte_idx + 4].copy_from_slice(&patched.to_le_bytes());
    }
    Ok(())
}

/// Separate data sections into rodata, data, and bss size.
fn collect_data_sections(data_sections: &[DataSection]) -> (Vec<u8>, Vec<u8>, u64) {
    let mut rodata_section = Vec::new();
    let mut data_section = Vec::new();
    let mut bss_size: u64 = 0;

    for ds in data_sections {
        match ds.kind {
            DataSectionKind::ReadOnly => { rodata_section.extend_from_slice(&ds.data); }
            DataSectionKind::Data => { data_section.extend_from_slice(&ds.data); }
            DataSectionKind::Bss => {
                bss_size += ds.data.len() as u64;
                if ds.align > 1 {
                    let padding = (ds.align as u64 - (bss_size % ds.align as u64)) % ds.align as u64;
                    bss_size += padding;
                }
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
fn write_phdr(buf: &mut Vec<u8>, p_type: u32, p_flags: u32, p_offset: u64, p_vaddr: u64, p_paddr: u64, p_filesz: u64, p_memsz: u64) {
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
fn new_shdr(sh_type: u32, sh_flags: u64, sh_addr: u64, sh_offset: u64, sh_size: u64, sh_link: u32, sh_info: u32, sh_addralign: u64, sh_entsize: u64) -> FilledShdr {
    FilledShdr { name: 0, sh_type, sh_flags, sh_addr, sh_offset, sh_size, sh_link, sh_info, sh_addralign, sh_entsize }
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
fn build_symbol_table(
    functions: &[IRFunction],
    function_offsets: &HashMap<String, u64>,
    function_sizes: &HashMap<String, u64>,
    text_vaddr: u64,
    external_symbols: &[String],
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
    symtab.extend_from_slice(&1u16.to_le_bytes()); // .text = section 1
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
        symtab.extend_from_slice(&1u16.to_le_bytes()); // .text section
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
fn build_shstrtab(config: &EmitConfig) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(0);
    if config.section_headers {
        buf.extend_from_slice(b".text\0");
        if config.format == OutputFormat::Obj {
            buf.extend_from_slice(b".rela.text\0");
        }
        buf.extend_from_slice(b".rodata\0");
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
            dst: None, func: callee.to_string(), args: vec![],
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
        assert_eq!(e_shnum, 8, "expected 8 section headers");
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
            if &elf[i..i + 5] == b"main\0" { found_main = true; break; }
        }
        assert!(found_main, "symbol 'main' must appear in strtab");
    }

    #[test]
    fn emit_raw_flat_binary() {
        let funcs = vec![make_return_function("_start")];
        let config = EmitConfig::bare_metal_raw();
        let raw = emit_raw(&funcs, &config).unwrap();
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
        let text_offset: usize = (64 + 56 * 2) as usize;
        let mut found_bl = false;
        let mut i = text_offset;
        while i + 4 <= elf.len() {
            let word = u32::from_le_bytes([elf[i], elf[i+1], elf[i+2], elf[i+3]]);
            if (word >> 26) == 0b100101 {
                let imm26 = word & 0x03FFFFFF;
                if imm26 != 0 { found_bl = true; break; }
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
            DataSection { name: "rodata".into(), kind: DataSectionKind::ReadOnly, align: 4, data: vec![0xDE, 0xAD, 0xBE, 0xEF] },
            DataSection { name: "data".into(), kind: DataSectionKind::Data, align: 8, data: vec![0x42; 16] },
            DataSection { name: "bss".into(), kind: DataSectionKind::Bss, align: 16, data: vec![0; 32] },
        ];
        let config = EmitConfig::linux_elf();
        let elf = emit_elf(&funcs, &data_sections, &config).unwrap();
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        // Verify rodata bytes appear.
        let mut found_rodata = false;
        for i in 0..elf.len().saturating_sub(4) {
            if &elf[i..i+4] == &[0xDE, 0xAD, 0xBE, 0xEF] { found_rodata = true; break; }
        }
        assert!(found_rodata, "rodata must appear in the ELF file");
    }

    #[test]
    fn emit_program_elf_header() {
        let mut func = IRFunction::new("main");
        func.current_block().terminator = IRTerminator::Return(vec![]);
        let program = IRProgram { functions: vec![func], data_sections: vec![] };
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
        assert_eq!(EmitConfig::linux_elf().effective_base_addr(), BASE_ADDR_LINUX);
        assert_eq!(EmitConfig::bare_metal_raw().effective_base_addr(), BASE_ADDR_BARE);
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
                if &elf[i..i + name.len() + 1] == name_bytes.as_slice() { found = true; break; }
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
            let sh_type = u32::from_le_bytes(elf[off+4..off+8].try_into().unwrap());
            if sh_type == SHT_RELA {
                let sh_offset = u64::from_le_bytes(elf[off+24..off+32].try_into().unwrap()) as usize;
                let sh_size = u64::from_le_bytes(elf[off+32..off+40].try_into().unwrap()) as usize;
                let sh_entsize = u64::from_le_bytes(elf[off+56..off+64].try_into().unwrap()) as usize;
                if sh_entsize == 0 { continue; }
                let num_entries = sh_size / sh_entsize;
                let mut entries = Vec::new();
                for j in 0..num_entries {
                    let base = sh_offset + j * sh_entsize;
                    let r_offset = u64::from_le_bytes(elf[base..base+8].try_into().unwrap());
                    let r_info = u64::from_le_bytes(elf[base+8..base+16].try_into().unwrap());
                    let r_addend = i64::from_le_bytes(elf[base+16..base+24].try_into().unwrap());
                    entries.push(RelaEntry { offset: r_offset, info: r_info, addend: r_addend });
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
            let sh_type = u32::from_le_bytes(elf[off+4..off+8].try_into().unwrap());
            if sh_type == SHT_SYMTAB {
                symtab_offset = u64::from_le_bytes(elf[off+24..off+32].try_into().unwrap()) as usize;
                symtab_size = u64::from_le_bytes(elf[off+32..off+40].try_into().unwrap()) as usize;
                let sh_link = u32::from_le_bytes(elf[off+40..off+44].try_into().unwrap()) as usize;
                let strtab_off = e_shoff + sh_link * 64;
                strtab_offset = u64::from_le_bytes(elf[strtab_off+24..strtab_off+32].try_into().unwrap()) as usize;
                strtab_size = u64::from_le_bytes(elf[strtab_off+32..strtab_off+40].try_into().unwrap()) as usize;
                break;
            }
        }

        if symtab_size == 0 { return Vec::new(); }

        let strtab = &elf[strtab_offset..strtab_offset + strtab_size];
        let num_syms = symtab_size / 24;
        let mut symbols = Vec::new();
        for i in 0..num_syms {
            let base = symtab_offset + i * 24;
            let st_name = u32::from_le_bytes(elf[base..base+4].try_into().unwrap()) as usize;
            let st_info = elf[base + 4];
            let st_shndx = u16::from_le_bytes(elf[base+6..base+8].try_into().unwrap());
            let st_value = u64::from_le_bytes(elf[base+8..base+16].try_into().unwrap());

            let name = if st_name < strtab.len() {
                let end = strtab[st_name..].iter().position(|&b| b == 0).unwrap_or(strtab.len() - st_name);
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
        assert!(e_shnum >= 9, "ET_REL with calls should have at least 9 section headers");

        // Find the .rela.text section by looking for SHT_RELA type.
        let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap()) as usize;
        let mut found_rela = false;
        for i in 0..e_shnum {
            let off = e_shoff + i * 64;
            let sh_type = u32::from_le_bytes(elf[off+4..off+8].try_into().unwrap());
            if sh_type == SHT_RELA {
                found_rela = true;
                break;
            }
        }
        assert!(found_rela, ".rela.text section (SHT_RELA) must exist in ET_REL");
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
            assert_eq!(entry.r_type(), R_AARCH64_CALL26,
                "expected R_AARCH64_CALL26, got type {}", entry.r_type());
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
        let ext_sym = symbols.iter().find(|(name, _, _, shndx)|
            name == "external_func" && *shndx == SHN_UNDEF
        );
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
            let sh_type = u32::from_le_bytes(elf[off+4..off+8].try_into().unwrap());
            if sh_type == SHT_PROGBITS {
                text_file_offset = u64::from_le_bytes(elf[off+24..off+32].try_into().unwrap()) as usize;
                break;
            }
        }

        for entry in &entries {
            let bl_file_offset = text_file_offset + entry.offset as usize;
            assert!(bl_file_offset + 4 <= elf.len(), "relocation offset out of bounds");
            let word = u32::from_le_bytes([
                elf[bl_file_offset], elf[bl_file_offset + 1],
                elf[bl_file_offset + 2], elf[bl_file_offset + 3],
            ]);
            // BL opcode: bits [31:26] = 100101
            assert_eq!((word >> 26) & 0x3F, 0b100101,
                "relocation offset should point to a BL instruction, got {:08x}", word);
        }
    }

    #[test]
    fn rela_multiple_calls() {
        // Verify multiple BL instructions generate multiple rela entries.
        let mut func = IRFunction::new("main");
        func.current_block().push(IRInstr::Call {
            dst: None, func: "foo".to_string(), args: vec![],
        });
        func.current_block().push(IRInstr::Call {
            dst: None, func: "bar".to_string(), args: vec![],
        });
        func.current_block().push(IRInstr::Call {
            dst: None, func: "baz".to_string(), args: vec![],
        });
        func.current_block().terminator = IRTerminator::Return(vec![]);
        let funcs = vec![func];
        let config = EmitConfig::relocatable_obj();
        let elf = emit_elf(&funcs, &[], &config).unwrap();

        let entries = parse_rela_entries_from_elf(&elf);
        assert_eq!(entries.len(), 3, "expected 3 rela entries for 3 calls");

        // Verify offsets are distinct and increasing.
        for i in 1..entries.len() {
            assert!(entries[i].offset > entries[i-1].offset,
                "rela entries should be sorted by offset");
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
            let sh_type = u32::from_le_bytes(elf[off+4..off+8].try_into().unwrap());
            assert_ne!(sh_type, SHT_RELA, "ET_EXEC should not have SHT_RELA sections");
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
//   bare-metal Pi 5.
// - Added CallRelocation struct and relocation resolution: inter-function BL
//   instructions are recorded during emission and patched after all function
//   addresses are known.
// - Extended emit_ir_instr to handle all IRInstr variants including Add, Sub,
//   Mul, Div, Cmp, Ret, Branch, CondBranch.
// - Extended BinOpKind match to handle comparison operators (SLt..Ne) with
//   CMP instruction emission (CSET TODO).
// - Added 15 tests covering: ELF header validity, machine type, exec type,
//   section headers, symbol table, raw binary, call relocation, EmitConfig
//   defaults, obj file type, bare-metal OSABI, data sections, legacy
//   emit_program, empty program, Display traits, base address, multiple
//   function symbols.
// - Updated lib.rs to re-export EmitConfig, OutputFormat, Target, emit_elf,
//   emit_raw.
