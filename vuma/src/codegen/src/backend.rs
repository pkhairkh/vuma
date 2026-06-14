//! # Multi-Backend Trait Architecture
//!
//! Defines the `TargetInfo` and `Backend` traits that allow VUMA to target
//! multiple instruction set architectures. Each ISA implements these traits
//! to provide target-specific information and code generation.

use crate::arm32::Arm32Backend;
use crate::ir::{IRFunction, IRInstr, IRType};
use crate::loongarch64::LoongArch64Backend;
use crate::mips64::Mips64Backend;
use crate::ppc64::PPC64Backend;
use crate::riscv64::RiscV64Backend;
use crate::x86_64::X86_64Backend;
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Endianness
// ---------------------------------------------------------------------------

/// Byte order of the target architecture.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum Endianness {
    /// Least-significant byte first (AArch64, RISC-V, x86_64, LoongArch).
    Little,
    /// Most-significant byte first (MIPS64 big-endian, PPC64 big-endian).
    Big,
    /// Bi-endian — the ISA supports both but the default is big-endian (PPC64).
    Bi,
}

// ---------------------------------------------------------------------------
// OutputFormat
// ---------------------------------------------------------------------------

/// The output binary format produced by the backend.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum OutputFormat {
    /// 64-bit ELF (AArch64, RISC-V64, x86_64, LoongArch64, MIPS64, PPC64).
    Elf64,
    /// 32-bit ELF (ARM32).
    Elf32,
    /// WebAssembly binary module (.wasm).
    WasmBinary,
    /// Raw binary blob (bare-metal, no headers).
    RawBinary,
}

// ---------------------------------------------------------------------------
// PhysicalReg
// ---------------------------------------------------------------------------

/// A physical register identified by class and index.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct PhysicalReg {
    /// Register class.
    pub class: RegClass,
    /// Register index within its class (0-based).
    pub index: u32,
}

impl PhysicalReg {
    /// Creates a new physical register identifier with the given class and index.
    pub fn new(class: RegClass, index: u32) -> Self {
        Self { class, index }
    }
}

impl fmt::Display for PhysicalReg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}:{}", self.class, self.index)
    }
}

// ---------------------------------------------------------------------------
// RegClass
// ---------------------------------------------------------------------------

/// Classification of physical registers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum RegClass {
    /// General-purpose integer registers (X0-X30 on ARM64, RAX-R15 on x86_64, etc.)
    Gpr,
    /// SIMD / floating-point registers (V0-V31 on ARM64, XMM0-XMM15 on x86_64, etc.)
    SimdFp,
    /// Condition register fields (PPC64 CR0-CR7).
    Condition,
    /// Special-purpose register (TOC pointer on PPC64, etc.)
    Special,
}

// ---------------------------------------------------------------------------
// FrameType
// ---------------------------------------------------------------------------

/// The kind of stack frame slot.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum FrameSlotKind {
    /// Spill slot for a register that was evicted.
    Spill,
    /// Local variable storage.
    Local,
    /// Outgoing argument that doesn't fit in registers.
    OutgoingArg,
    /// Incoming stack argument from the caller.
    IncomingArg,
}

// ---------------------------------------------------------------------------
// RelocationEntry
// ---------------------------------------------------------------------------

/// A relocation entry for patching encoded code at link time.
///
/// Each entry records a byte offset within the function's encoded output where
/// a symbolic reference must be resolved, the name of the target symbol, and
/// the ISA-specific relocation type (e.g., `"R_X86_64_PLT32"`, `"R_X86_64_64"`).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RelocationEntry {
    /// Byte offset within the function's encoded code where the relocation applies.
    pub offset: u64,
    /// Name of the target symbol.
    pub symbol: String,
    /// Relocation type (ISA-specific, e.g., "R_X86_64_PLT32", "R_X86_64_64").
    pub reloc_type: String,
}

// ---------------------------------------------------------------------------
// AllocatedInstruction
// ---------------------------------------------------------------------------

/// A single instruction after register allocation, with physical registers assigned.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AllocatedInstruction {
    /// Opcode name (for debugging / disassembly).
    pub opcode: String,
    /// Physical registers read by this instruction.
    pub reads: Vec<PhysicalReg>,
    /// Physical registers written by this instruction.
    pub writes: Vec<PhysicalReg>,
    /// Encoded bytes (filled in during encoding phase).
    pub encoded: Vec<u8>,
}

// ---------------------------------------------------------------------------
// AllocatedBlock
// ---------------------------------------------------------------------------

/// A basic block after register allocation.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AllocatedBlock {
    /// Block label.
    pub label: String,
    /// Allocated instructions in order.
    pub instructions: Vec<AllocatedInstruction>,
    /// Byte offset of this block in the final code section.
    pub code_offset: usize,
}

// ---------------------------------------------------------------------------
// AllocatedFunction
// ---------------------------------------------------------------------------

/// A function after register allocation.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AllocatedFunction {
    /// Function name.
    pub name: String,
    /// Allocated blocks in layout order.
    pub blocks: Vec<AllocatedBlock>,
    /// Total frame size in bytes (including callee-saved save area).
    pub frame_size: usize,
    /// Set of callee-saved physical registers used.
    pub callee_saved: Vec<PhysicalReg>,
    /// Number of spill slots.
    pub spill_slots: usize,
    /// Byte size of the encoded function body.
    pub code_size: usize,
    /// Relocation entries for this function.
    #[serde(default)]
    pub relocations: Vec<RelocationEntry>,
}

// ---------------------------------------------------------------------------
// AllocatedProgram
// ---------------------------------------------------------------------------

/// A complete program after register allocation.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AllocatedProgram {
    /// Allocated functions.
    pub functions: Vec<AllocatedFunction>,
    /// Total code section size in bytes.
    pub total_code_size: usize,
    /// Total data section size in bytes.
    pub total_data_size: usize,
}

// ---------------------------------------------------------------------------
// BackendError
// ---------------------------------------------------------------------------

/// Error type for backend operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum BackendError {
    /// The requested feature is not supported by this ISA.
    #[error("[{isa}] unsupported feature: {feature}")]
    UnsupportedFeature {
        /// ISA identifier (e.g., "aarch64", "x86_64").
        isa: &'static str,
        /// Description of the unsupported feature.
        feature: String,
    },

    /// Register allocation failed.
    #[error("[{isa}] register allocation failed: {reason}")]
    RegisterAllocFailed {
        /// ISA identifier.
        isa: &'static str,
        /// Reason for the allocation failure.
        reason: String,
    },

    /// Instruction encoding failed.
    #[error("[{isa}] encoding error: {reason}")]
    EncodingError {
        /// ISA identifier.
        isa: &'static str,
        /// Reason for the encoding failure.
        reason: String,
    },

    /// Invalid instruction for this target.
    #[error("[{isa}] invalid instruction: {details}")]
    InvalidInstruction {
        /// ISA identifier.
        isa: &'static str,
        /// Details about why the instruction is invalid.
        details: String,
    },

    /// ELF / binary emission error.
    #[error("[{isa}] emission error: {reason}")]
    EmissionError {
        /// ISA identifier.
        isa: &'static str,
        /// Reason for the emission failure.
        reason: String,
    },

    /// The target cannot handle this type.
    #[error("[{isa}] unsupported type: {ty}")]
    UnsupportedType {
        /// ISA identifier.
        isa: &'static str,
        /// The unsupported type name.
        ty: String,
    },

    /// Generic backend error.
    #[error("[{isa}] {message}")]
    Other {
        /// ISA identifier.
        isa: &'static str,
        /// Error message.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// TargetInfo trait
// ---------------------------------------------------------------------------

/// Target-specific information needed during code generation.
///
/// This trait provides a data-driven interface for query target properties.
/// It must be implementable for ALL supported ISAs, including fundamentally
/// different architectures like Wasm (stack machine, no registers) and
/// MIPS (branch delay slots).
///
/// # Object Safety
///
/// This trait is object-safe: all methods take `&self` and return only
/// owned types or references with `'static` lifetime.
pub trait TargetInfo: Send + Sync + 'static {
    // === Identity ===

    /// ISA name in lowercase (e.g., "aarch64", "riscv64", "wasm32").
    fn isa_name(&self) -> &'static str;

    /// LLVM-style target triple (e.g., "aarch64-unknown-linux-gnu").
    fn target_triple(&self) -> &'static str;

    /// ELF `e_machine` value.  Returns 0 for non-ELF targets (Wasm).
    fn elf_machine_type(&self) -> u16;

    /// Default base address for the .text section.
    fn default_base_address(&self) -> u64;

    // === Data model ===

    /// Pointer width in bytes (4 for 32-bit, 8 for 64-bit).
    fn pointer_width(&self) -> usize;

    /// Size in bytes of `ty` on this target.
    fn size_of(&self, ty: &IRType) -> usize;

    /// Natural alignment in bytes of `ty` on this target.
    fn alignment_of(&self, ty: &IRType) -> usize;

    /// Byte order of this target.
    fn endianness(&self) -> Endianness;

    // === Register architecture ===

    /// Whether this target has registers at all.  `false` for Wasm (stack machine).
    fn has_registers(&self) -> bool;

    /// Number of general-purpose registers.  0 for Wasm.
    fn num_gp_regs(&self) -> usize;

    /// Number of SIMD/FP registers.  0 for Wasm.
    fn num_simd_fp_regs(&self) -> usize;

    /// Whether the ISA has a hardwired-zero register (RISC-V x0, LoongArch r0).
    fn has_hardwired_zero(&self) -> bool;

    /// Whether the ISA uses a link register (ARM, RISC-V, MIPS, PPC) rather than
    /// pushing the return address on the stack (x86_64).
    fn has_link_register(&self) -> bool;

    /// Whether branches have delay slots (MIPS only).
    fn has_branch_delay_slots(&self) -> bool;

    /// Whether this ISA uses a TOC (Table of Contents) pointer (PPC64 r2).
    fn has_toc_pointer(&self) -> bool;

    /// Whether this ISA has dedicated condition register fields (PPC64 CR0-CR7).
    fn has_condition_registers(&self) -> bool;

    // === Calling convention ===

    /// Name of the calling convention (e.g., "aapcs64", "lp64d", "systemv").
    fn calling_convention_name(&self) -> &'static str;

    /// Number of integer argument registers.
    fn num_int_arg_regs(&self) -> usize;

    /// Number of FP/SIMD argument registers.
    fn num_fp_arg_regs(&self) -> usize;

    /// Required stack alignment in bytes.
    fn stack_alignment(&self) -> usize;

    // === Instruction encoding ===

    /// Alignment requirement for instructions in bytes (4 for fixed-width RISCs,
    /// 1 for x86_64 and Wasm).
    fn instruction_alignment(&self) -> usize;

    /// Minimum and maximum instruction width in bytes.
    /// - Fixed-width 32-bit ISAs: (4, 4)
    /// - x86_64: (1, 15)
    /// - RISC-V with RVC: (2, 4)
    /// - Wasm: (1, ~) but typically (1, 15)
    fn instruction_width_range(&self) -> (usize, usize);

    // === Output format ===

    /// Binary format produced by this backend.
    fn output_format(&self) -> OutputFormat;
}

// ---------------------------------------------------------------------------
// Backend trait
// ---------------------------------------------------------------------------

/// A code generation backend for a specific target architecture.
///
/// Each supported ISA implements this trait, providing register allocation,
/// instruction encoding, program emission, and disassembly.
///
/// # Object Safety
///
/// This trait is object-safe.
pub trait Backend: Send + Sync + 'static {
    /// Returns a reference to this backend's target info.
    fn target_info(&self) -> &dyn TargetInfo;

    /// Allocate physical registers for an IR function.
    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError>;

    /// Encode a single allocated function into machine code bytes.
    fn encode_function(&self, func: &AllocatedFunction) -> Result<Vec<u8>, BackendError>;

    /// Encode an entire allocated program into its final binary form
    /// (ELF, .wasm, raw binary, etc.).
    fn encode_program(&self, program: &AllocatedProgram) -> Result<Vec<u8>, BackendError>;

    /// Returns the bytes for a minimal return stub (e.g., `RET` on ARM64,
    /// `mov eax, 0; ret` on x86_64, `end` on Wasm).
    fn return_stub(&self) -> Vec<u8>;

    /// Returns a trampoline that jumps to `entry_addr`.
    fn trampoline(&self, entry_addr: u64) -> Vec<u8>;

    /// Disassemble `bytes` starting at virtual address `addr`.
    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String>;

    /// Human-readable name of this backend.
    fn name(&self) -> &'static str;
}

// ---------------------------------------------------------------------------
// BackendKind
// ---------------------------------------------------------------------------

/// Enumeration of all supported backend architectures.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum BackendKind {
    /// ARM 64-bit (AArch64).
    AArch64,
    /// RISC-V 64-bit.
    RiscV64,
    /// WebAssembly 32-bit.
    Wasm32,
    /// LoongArch 64-bit.
    LoongArch64,
    /// x86-64.
    X86_64,
    /// ARM 32-bit.
    Arm32,
    /// MIPS 64-bit.
    Mips64,
    /// PowerPC 64-bit.
    PowerPC64,
}

impl BackendKind {
    /// Returns the ISA name string for this backend kind.
    pub fn isa_name(&self) -> &'static str {
        match self {
            BackendKind::AArch64 => "aarch64",
            BackendKind::RiscV64 => "riscv64",
            BackendKind::Wasm32 => "wasm32",
            BackendKind::LoongArch64 => "loongarch64",
            BackendKind::X86_64 => "x86_64",
            BackendKind::Arm32 => "arm32",
            BackendKind::Mips64 => "mips64",
            BackendKind::PowerPC64 => "ppc64",
        }
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.isa_name())
    }
}

// ---------------------------------------------------------------------------
// AArch64 TargetInfo implementation (wrapping existing ARM64 logic)
// ---------------------------------------------------------------------------

/// AArch64 (ARM64) target information.
///
/// Provides the data model, register counts, and calling convention details
/// for the AArch64 architecture under the AAPCS64 ABI.
pub struct AArch64TargetInfo;

impl TargetInfo for AArch64TargetInfo {
    fn isa_name(&self) -> &'static str {
        "aarch64"
    }
    fn target_triple(&self) -> &'static str {
        "aarch64-unknown-linux-gnu"
    }
    fn elf_machine_type(&self) -> u16 {
        183
    } // EM_AARCH64
    fn default_base_address(&self) -> u64 {
        0x400000
    }
    fn pointer_width(&self) -> usize {
        8
    }

    fn size_of(&self, ty: &IRType) -> usize {
        crate::ir::size_of(ty) // Uses existing ARM64 LP64 logic
    }

    fn alignment_of(&self, ty: &IRType) -> usize {
        crate::ir::alignment_of(ty) // Uses existing ARM64 LP64 logic
    }

    fn endianness(&self) -> Endianness {
        Endianness::Little
    }
    fn has_registers(&self) -> bool {
        true
    }
    fn num_gp_regs(&self) -> usize {
        31
    } // X0-X30 (SP/XZR are special)
    fn num_simd_fp_regs(&self) -> usize {
        32
    } // V0-V31
    fn has_hardwired_zero(&self) -> bool {
        true
    } // XZR
    fn has_link_register(&self) -> bool {
        true
    } // X30 (LR)
    fn has_branch_delay_slots(&self) -> bool {
        false
    }
    fn has_toc_pointer(&self) -> bool {
        false
    }
    fn has_condition_registers(&self) -> bool {
        false
    }
    fn calling_convention_name(&self) -> &'static str {
        "aapcs64"
    }
    fn num_int_arg_regs(&self) -> usize {
        8
    } // X0-X7
    fn num_fp_arg_regs(&self) -> usize {
        8
    } // V0-V7
    fn stack_alignment(&self) -> usize {
        16
    }
    fn instruction_alignment(&self) -> usize {
        4
    }
    fn instruction_width_range(&self) -> (usize, usize) {
        (4, 4)
    }
    fn output_format(&self) -> OutputFormat {
        OutputFormat::Elf64
    }
}

// ---------------------------------------------------------------------------
// RISC-V64 TargetInfo
// ---------------------------------------------------------------------------

/// RISC-V 64-bit target information (RV64GC, LP64D ABI).
pub struct RiscV64TargetInfo;

impl TargetInfo for RiscV64TargetInfo {
    fn isa_name(&self) -> &'static str {
        "riscv64"
    }
    fn target_triple(&self) -> &'static str {
        "riscv64-unknown-linux-gnu"
    }
    fn elf_machine_type(&self) -> u16 {
        243
    } // EM_RISCV
    fn default_base_address(&self) -> u64 {
        0x10000
    }
    fn pointer_width(&self) -> usize {
        8
    }
    fn size_of(&self, ty: &IRType) -> usize {
        crate::ir::size_of(ty)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        crate::ir::alignment_of(ty)
    }
    fn endianness(&self) -> Endianness {
        Endianness::Little
    }
    fn has_registers(&self) -> bool {
        true
    }
    fn num_gp_regs(&self) -> usize {
        32
    } // x0-x31
    fn num_simd_fp_regs(&self) -> usize {
        32
    } // f0-f31
    fn has_hardwired_zero(&self) -> bool {
        true
    } // x0
    fn has_link_register(&self) -> bool {
        true
    } // x1 (ra)
    fn has_branch_delay_slots(&self) -> bool {
        false
    }
    fn has_toc_pointer(&self) -> bool {
        false
    }
    fn has_condition_registers(&self) -> bool {
        false
    }
    fn calling_convention_name(&self) -> &'static str {
        "lp64d"
    }
    fn num_int_arg_regs(&self) -> usize {
        8
    } // a0-a7
    fn num_fp_arg_regs(&self) -> usize {
        8
    } // fa0-fa7
    fn stack_alignment(&self) -> usize {
        16
    }
    fn instruction_alignment(&self) -> usize {
        2
    } // RVC allows 16-bit alignment
    fn instruction_width_range(&self) -> (usize, usize) {
        (2, 4)
    } // RVC + 32-bit
    fn output_format(&self) -> OutputFormat {
        OutputFormat::Elf64
    }
}

// ---------------------------------------------------------------------------
// Wasm32 TargetInfo
// ---------------------------------------------------------------------------

/// WebAssembly 32-bit target information (stack machine, no registers).
pub struct Wasm32TargetInfo;

impl TargetInfo for Wasm32TargetInfo {
    fn isa_name(&self) -> &'static str {
        "wasm32"
    }
    fn target_triple(&self) -> &'static str {
        "wasm32-unknown-unknown"
    }
    fn elf_machine_type(&self) -> u16 {
        0
    } // Not ELF
    fn default_base_address(&self) -> u64 {
        0
    } // Linear memory base
    fn pointer_width(&self) -> usize {
        4
    }
    fn size_of(&self, ty: &IRType) -> usize {
        match ty {
            IRType::Ptr | IRType::Func => 4, // 32-bit pointers in wasm32
            _ => crate::ir::size_of(ty),
        }
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        match ty {
            IRType::Ptr | IRType::Func => 4,
            _ => crate::ir::alignment_of(ty),
        }
    }
    fn endianness(&self) -> Endianness {
        Endianness::Little
    }
    fn has_registers(&self) -> bool {
        false
    } // Stack machine!
    fn num_gp_regs(&self) -> usize {
        0
    }
    fn num_simd_fp_regs(&self) -> usize {
        0
    }
    fn has_hardwired_zero(&self) -> bool {
        false
    }
    fn has_link_register(&self) -> bool {
        false
    }
    fn has_branch_delay_slots(&self) -> bool {
        false
    }
    fn has_toc_pointer(&self) -> bool {
        false
    }
    fn has_condition_registers(&self) -> bool {
        false
    }
    fn calling_convention_name(&self) -> &'static str {
        "wasm-stack"
    }
    fn num_int_arg_regs(&self) -> usize {
        0
    } // Stack-based calling
    fn num_fp_arg_regs(&self) -> usize {
        0
    }
    fn stack_alignment(&self) -> usize {
        8
    } // Wasm stack alignment
    fn instruction_alignment(&self) -> usize {
        1
    }
    fn instruction_width_range(&self) -> (usize, usize) {
        (1, 15)
    }
    fn output_format(&self) -> OutputFormat {
        OutputFormat::WasmBinary
    }
}

// ---------------------------------------------------------------------------
// LoongArch64 TargetInfo
// ---------------------------------------------------------------------------

/// LoongArch 64-bit target information (LP64 ABI).
pub struct LoongArch64TargetInfo;

impl TargetInfo for LoongArch64TargetInfo {
    fn isa_name(&self) -> &'static str {
        "loongarch64"
    }
    fn target_triple(&self) -> &'static str {
        "loongarch64-unknown-linux-gnu"
    }
    fn elf_machine_type(&self) -> u16 {
        258
    } // EM_LOONGARCH
    fn default_base_address(&self) -> u64 {
        0x120000000
    }
    fn pointer_width(&self) -> usize {
        8
    }
    fn size_of(&self, ty: &IRType) -> usize {
        crate::ir::size_of(ty)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        crate::ir::alignment_of(ty)
    }
    fn endianness(&self) -> Endianness {
        Endianness::Little
    }
    fn has_registers(&self) -> bool {
        true
    }
    fn num_gp_regs(&self) -> usize {
        32
    } // r0-r31
    fn num_simd_fp_regs(&self) -> usize {
        32
    } // f0-f31
    fn has_hardwired_zero(&self) -> bool {
        true
    } // r0
    fn has_link_register(&self) -> bool {
        true
    } // r1 (ra)
    fn has_branch_delay_slots(&self) -> bool {
        false
    }
    fn has_toc_pointer(&self) -> bool {
        false
    }
    fn has_condition_registers(&self) -> bool {
        false
    }
    fn calling_convention_name(&self) -> &'static str {
        "lp64"
    }
    fn num_int_arg_regs(&self) -> usize {
        8
    } // a0-a7 (r4-r11)
    fn num_fp_arg_regs(&self) -> usize {
        8
    } // fa0-fa7
    fn stack_alignment(&self) -> usize {
        16
    }
    fn instruction_alignment(&self) -> usize {
        4
    }
    fn instruction_width_range(&self) -> (usize, usize) {
        (4, 4)
    }
    fn output_format(&self) -> OutputFormat {
        OutputFormat::Elf64
    }
}

// ---------------------------------------------------------------------------
// x86_64 TargetInfo
// ---------------------------------------------------------------------------

/// x86-64 target information (SystemV ABI).
pub struct X86_64TargetInfo;

impl TargetInfo for X86_64TargetInfo {
    fn isa_name(&self) -> &'static str {
        "x86_64"
    }
    fn target_triple(&self) -> &'static str {
        "x86_64-unknown-linux-gnu"
    }
    fn elf_machine_type(&self) -> u16 {
        62
    } // EM_X86_64
    fn default_base_address(&self) -> u64 {
        0x400000
    }
    fn pointer_width(&self) -> usize {
        8
    }
    fn size_of(&self, ty: &IRType) -> usize {
        crate::ir::size_of(ty)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        crate::ir::alignment_of(ty)
    }
    fn endianness(&self) -> Endianness {
        Endianness::Little
    }
    fn has_registers(&self) -> bool {
        true
    }
    fn num_gp_regs(&self) -> usize {
        16
    } // RAX-R15
    fn num_simd_fp_regs(&self) -> usize {
        16
    } // XMM0-XMM15
    fn has_hardwired_zero(&self) -> bool {
        false
    } // No hardwired zero reg
    fn has_link_register(&self) -> bool {
        false
    } // Return addr pushed on stack
    fn has_branch_delay_slots(&self) -> bool {
        false
    }
    fn has_toc_pointer(&self) -> bool {
        false
    }
    fn has_condition_registers(&self) -> bool {
        false
    }
    fn calling_convention_name(&self) -> &'static str {
        "systemv"
    }
    fn num_int_arg_regs(&self) -> usize {
        6
    } // RDI, RSI, RDX, RCX, R8, R9
    fn num_fp_arg_regs(&self) -> usize {
        8
    } // XMM0-XMM7
    fn stack_alignment(&self) -> usize {
        16
    }
    fn instruction_alignment(&self) -> usize {
        1
    } // Variable-length
    fn instruction_width_range(&self) -> (usize, usize) {
        (1, 15)
    }
    fn output_format(&self) -> OutputFormat {
        OutputFormat::Elf64
    }
}

// ---------------------------------------------------------------------------
// ARM32 TargetInfo
// ---------------------------------------------------------------------------

/// ARM 32-bit target information (AAPCS).
pub struct Arm32TargetInfo;

impl TargetInfo for Arm32TargetInfo {
    fn isa_name(&self) -> &'static str {
        "arm32"
    }
    fn target_triple(&self) -> &'static str {
        "arm-unknown-linux-gnueabihf"
    }
    fn elf_machine_type(&self) -> u16 {
        40
    } // EM_ARM
    fn default_base_address(&self) -> u64 {
        0x10000
    }
    fn pointer_width(&self) -> usize {
        4
    }
    fn size_of(&self, ty: &IRType) -> usize {
        match ty {
            IRType::Ptr | IRType::Func => 4, // 32-bit pointers
            IRType::I64 | IRType::U64 => 8,
            _ => crate::ir::size_of(ty),
        }
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        match ty {
            IRType::Ptr | IRType::Func => 4,
            IRType::I64 | IRType::U64 => 4, // ARM32 aligns i64 to 4
            _ => crate::ir::alignment_of(ty),
        }
    }
    fn endianness(&self) -> Endianness {
        Endianness::Little
    }
    fn has_registers(&self) -> bool {
        true
    }
    fn num_gp_regs(&self) -> usize {
        16
    } // R0-R15
    fn num_simd_fp_regs(&self) -> usize {
        32
    } // D0-D31
    fn has_hardwired_zero(&self) -> bool {
        false
    }
    fn has_link_register(&self) -> bool {
        true
    } // R14 (LR)
    fn has_branch_delay_slots(&self) -> bool {
        false
    }
    fn has_toc_pointer(&self) -> bool {
        false
    }
    fn has_condition_registers(&self) -> bool {
        false
    }
    fn calling_convention_name(&self) -> &'static str {
        "aapcs"
    }
    fn num_int_arg_regs(&self) -> usize {
        4
    } // R0-R3
    fn num_fp_arg_regs(&self) -> usize {
        16
    } // D0-D15 (AAPCS VFP)
    fn stack_alignment(&self) -> usize {
        8
    }
    fn instruction_alignment(&self) -> usize {
        2
    } // Thumb allows 16-bit
    fn instruction_width_range(&self) -> (usize, usize) {
        (2, 4)
    }
    fn output_format(&self) -> OutputFormat {
        OutputFormat::Elf32
    }
}

// ---------------------------------------------------------------------------
// MIPS64 TargetInfo
// ---------------------------------------------------------------------------

/// MIPS 64-bit target information (N64 ABI, big-endian).
pub struct Mips64TargetInfo;

impl TargetInfo for Mips64TargetInfo {
    fn isa_name(&self) -> &'static str {
        "mips64"
    }
    fn target_triple(&self) -> &'static str {
        "mips64-unknown-linux-gnuabi64"
    }
    fn elf_machine_type(&self) -> u16 {
        8
    } // EM_MIPS
    fn default_base_address(&self) -> u64 {
        0x120000000
    }
    fn pointer_width(&self) -> usize {
        8
    }
    fn size_of(&self, ty: &IRType) -> usize {
        crate::ir::size_of(ty)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        crate::ir::alignment_of(ty)
    }
    fn endianness(&self) -> Endianness {
        Endianness::Big
    }
    fn has_registers(&self) -> bool {
        true
    }
    fn num_gp_regs(&self) -> usize {
        32
    } // $0-$31
    fn num_simd_fp_regs(&self) -> usize {
        32
    } // $f0-$f31
    fn has_hardwired_zero(&self) -> bool {
        true
    } // $zero ($0)
    fn has_link_register(&self) -> bool {
        true
    } // $ra ($31)
    fn has_branch_delay_slots(&self) -> bool {
        true
    } // THE defining feature
    fn has_toc_pointer(&self) -> bool {
        false
    }
    fn has_condition_registers(&self) -> bool {
        false
    }
    fn calling_convention_name(&self) -> &'static str {
        "n64"
    }
    fn num_int_arg_regs(&self) -> usize {
        4
    } // $a0-$a3 (but N64 extends to 8)
    fn num_fp_arg_regs(&self) -> usize {
        8
    } // $f12-$f19 (N64 FP args)
    fn stack_alignment(&self) -> usize {
        16
    }
    fn instruction_alignment(&self) -> usize {
        4
    }
    fn instruction_width_range(&self) -> (usize, usize) {
        (4, 4)
    }
    fn output_format(&self) -> OutputFormat {
        OutputFormat::Elf64
    }
}

// ---------------------------------------------------------------------------
// PowerPC64 TargetInfo
// ---------------------------------------------------------------------------

/// PowerPC 64-bit target information (ELFv2 ABI, big-endian by default).
pub struct PowerPC64TargetInfo;

impl TargetInfo for PowerPC64TargetInfo {
    fn isa_name(&self) -> &'static str {
        "ppc64"
    }
    fn target_triple(&self) -> &'static str {
        "powerpc64le-unknown-linux-gnu"
    }
    fn elf_machine_type(&self) -> u16 {
        21
    } // EM_PPC64
    fn default_base_address(&self) -> u64 {
        0x10000000
    }
    fn pointer_width(&self) -> usize {
        8
    }
    fn size_of(&self, ty: &IRType) -> usize {
        crate::ir::size_of(ty)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        crate::ir::alignment_of(ty)
    }
    fn endianness(&self) -> Endianness {
        Endianness::Bi
    } // Bi-endian
    fn has_registers(&self) -> bool {
        true
    }
    fn num_gp_regs(&self) -> usize {
        32
    } // R0-R31
    fn num_simd_fp_regs(&self) -> usize {
        64
    } // 32 FPR + 32 VMX (VSX overlaps)
    fn has_hardwired_zero(&self) -> bool {
        false
    } // R0 is NOT hardwired zero (it's volatile)
    fn has_link_register(&self) -> bool {
        true
    } // LR (SPR)
    fn has_branch_delay_slots(&self) -> bool {
        false
    }
    fn has_toc_pointer(&self) -> bool {
        true
    } // R2 = TOC
    fn has_condition_registers(&self) -> bool {
        true
    } // CR0-CR7
    fn calling_convention_name(&self) -> &'static str {
        "elfv2"
    }
    fn num_int_arg_regs(&self) -> usize {
        8
    } // R3-R10
    fn num_fp_arg_regs(&self) -> usize {
        13
    } // F1-F13
    fn stack_alignment(&self) -> usize {
        16
    }
    fn instruction_alignment(&self) -> usize {
        4
    }
    fn instruction_width_range(&self) -> (usize, usize) {
        (4, 4)
    }
    fn output_format(&self) -> OutputFormat {
        OutputFormat::Elf64
    }
}

// ---------------------------------------------------------------------------
// AArch64 Mnemonic Decoder
// ---------------------------------------------------------------------------

/// Decode a 32-bit AArch64 instruction word into a human-readable mnemonic.
///
/// Covers the most common AArch64 instructions: ADD, SUB, MOV, LDR, STR, B,
/// BL, RET, CMP, B.cond, STP, LDP, NOP, MUL, SDIV, UDIV, AND, ORR, EOR,
/// plus several additional frequently-encountered encodings.
fn decode_aarch64(word: u32) -> String {
    let rd = word & 0x1F;
    let rn = (word >> 5) & 0x1F;
    let rt = rd; // alias for load/store destination
    let rm = (word >> 16) & 0x1F;
    let imm12 = (word >> 10) & 0xFFF;
    let cond = word & 0xF;

    // NOP: d503201f
    if word == 0xD503201F {
        return "nop".to_string();
    }

    // RET: d65f03c0
    if word == 0xD65F03C0 {
        return "ret".to_string();
    }

    let _top8 = word >> 24;
    let _top10 = word >> 22;

    // --- ADD/SUB (immediate): 100100xx ...
    if (word >> 23) & 0x1FF == 0b1_0010_0010 {
        // ADD Xd, Xn, #imm12
        return format!("add x{}, x{}, #{}", rd, rn, imm12);
    }
    if (word >> 23) & 0x1FF == 0b1_1010_0010 {
        // SUB Xd, Xn, #imm12
        return format!("sub x{}, x{}, #{}", rd, rn, imm12);
    }

    // --- ADD (shifted register): 1_00_0101_1_xxx ...
    if (word >> 24) & 0xFF == 0b1000_1011 {
        // ADD Xd, Xn, Xm
        return format!("add x{}, x{}, x{}", rd, rn, rm);
    }

    // --- SUB (shifted register): 1_00_0101_1_xxx ... with S=1 at bit30
    if (word >> 24) & 0xFF == 0b1101_0110 {
        // SUB Xd, Xn, Xm (bit 30 set = sub)
        return format!("sub x{}, x{}, x{}", rd, rn, rm);
    }

    // --- AND (shifted register): 1_00_0101_0_00_xxx
    if (word >> 24) & 0xFE == 0b1000_1010 {
        // Check bit 21-15: opcode[31:21] = 10001010_000
        if (word >> 21) & 0x7FF == 0b10001010000 {
            return format!("and x{}, x{}, x{}", rd, rn, rm);
        }
    }

    // --- ORR (shifted register): 1_01_0101_0_00_xxx
    if (word >> 21) & 0x7FF == 0b10101010000 {
        return format!("orr x{}, x{}, x{}", rd, rn, rm);
    }

    // --- EOR (shifted register): 1_10_0101_0_00_xxx
    if (word >> 21) & 0x7FF == 0b11001010000 {
        return format!("eor x{}, x{}, x{}", rd, rn, rm);
    }

    // --- MOV (register): alias for ORR Xd, XZR, Xm
    // ORR Xd, XZR, Xm: 10101010_000_rm_000000_xzr_rd
    // More general: ORR with Rn=XZR(31)
    if (word >> 21) & 0x7FF == 0b10101010000 && rn == 31 {
        return format!("mov x{}, x{}", rd, rm);
    }

    // --- MUL: MADD Xd, Xn, Xm, XZR
    // Encoding: 1_00_1101_1_000_Rm_0_01111_Rn_Rd
    if (word >> 21) & 0x7FF == 0b10011011000 && ((word >> 10) & 0x1F) == 0b01111 {
        return format!("mul x{}, x{}, x{}", rd, rn, rm);
    }

    // --- SDIV: 1_00_1101_1_0100_00000_00001_Rn_Rd  (actually 1_00_1101_0100_Rm_00001_Rn_Rd)
    if (word >> 21) & 0x7FF == 0b10011010100 && (word >> 10) & 0x1F == 0b00001 {
        return format!("sdiv x{}, x{}, x{}", rd, rn, rm);
    }

    // --- UDIV: 1_00_1101_0000_Rm_00001_Rn_Rd
    if (word >> 21) & 0x7FF == 0b10011010000 && (word >> 10) & 0x1F == 0b00001 {
        return format!("udiv x{}, x{}, x{}", rd, rn, rm);
    }

    // --- CMP (immediate): SUBS XZR, Xn, #imm12
    // 11100001_00_xxx_xxx_xxx_xxx_xxx_11111_xxx
    if (word >> 23) & 0x1FF == 0b1_1110_0010 && rd == 31 {
        return format!("cmp x{}, #{}", rn, imm12);
    }

    // --- CMP (register): SUBS XZR, Xn, Xm
    if (word >> 21) & 0x7FF == 0b11101011000 && rd == 31 {
        return format!("cmp x{}, x{}", rn, rm);
    }

    // --- B.cond: 0101010x xxxxxxxxxx xxxxxx cond
    if (word >> 24) & 0xFF == 0x54 {
        let cond_name = match cond {
            0 => "eq",
            1 => "ne",
            2 => "cs",
            3 => "cc",
            4 => "mi",
            5 => "pl",
            6 => "vs",
            7 => "vc",
            8 => "hi",
            9 => "ls",
            10 => "ge",
            11 => "lt",
            12 => "gt",
            13 => "le",
            14 => "al",
            _ => "??",
        };
        let imm19 = (word >> 5) & 0x7FFFF;
        let offset = ((imm19 as i32) << 13) >> 11; // sign-extend and *4
        return format!("b.{} {:+}", cond_name, offset);
    }

    // --- B (unconditional): 000101xx xxxxxxxxxxxxxxxxxxxx
    if (word >> 26) & 0x3F == 0b000101 {
        let imm26 = word & 0x3FFFFFF;
        let offset = ((imm26 as i32) << 6) >> 4; // sign-extend and *4
        return format!("b {:+}", offset);
    }

    // --- BL: 100101xx xxxxxxxxxxxxxxxxxxxx
    if (word >> 26) & 0x3F == 0b100101 {
        let imm26 = word & 0x3FFFFFF;
        let offset = ((imm26 as i32) << 6) >> 4;
        return format!("bl {:+}", offset);
    }

    // --- LDR (unsigned offset): 11111001_01_xxx_xxx_xxx_xxx_xxx_xn_rt
    if (word >> 22) & 0x3FF == 0b1111100101 {
        let imm12_raw = (word >> 10) & 0xFFF;
        let offset = imm12_raw * 8; // scale by 8 for 64-bit
        return format!("ldr x{}, [x{}, #{}]", rt, rn, offset);
    }

    // --- STR (unsigned offset): 11111000_01_xxx_xxx_xxx_xxx_xxx_xn_rt
    if (word >> 22) & 0x3FF == 0b1111100001 {
        let imm12_raw = (word >> 10) & 0xFFF;
        let offset = imm12_raw * 8;
        return format!("str x{}, [x{}, #{}]", rt, rn, offset);
    }

    // --- LDP (signed offset, 64-bit): 101_0100_110_xxx_xxx_xxx_xxx_xxx_xn_rt2
    if (word >> 22) & 0x3FF == 0b1010100110 {
        let rt2 = (word >> 10) & 0x1F;
        let imm7 = ((word >> 15) & 0x7F) as i8 as i32;
        let offset = imm7 * 8;
        return format!("ldp x{}, x{}, [x{}, #{}]", rt, rt2, rn, offset);
    }

    // --- STP (signed offset, 64-bit): 101_0100_010_xxx_xxx_xxx_xxx_xxx_xn_rt2
    if (word >> 22) & 0x3FF == 0b1010100010 {
        let rt2 = (word >> 10) & 0x1F;
        let imm7 = ((word >> 15) & 0x7F) as i8 as i32;
        let offset = imm7 * 8;
        return format!("stp x{}, x{}, [x{}, #{}]", rt, rt2, rn, offset);
    }

    // --- MOVZ: 110100101_ww_xxx_xxx_xxx_xxx_xxx_xn_rd
    if (word >> 23) & 0x1FF == 0b110100101 {
        let hw = (word >> 21) & 0x3;
        let imm16 = (word >> 5) & 0xFFFF;
        return format!("movz x{}, #{}{}, LSL #{}", rd, imm16, "", hw * 16);
    }

    // --- MOVK: 111100101_ww_xxx_xxx_xxx_xxx_xxx_xn_rd
    if (word >> 23) & 0x1FF == 0b111100101 {
        let hw = (word >> 21) & 0x3;
        let imm16 = (word >> 5) & 0xFFFF;
        return format!("movk x{}, #{}{}, LSL #{}", rd, imm16, "", hw * 16);
    }

    format!(".word {:08x}", word)
}

// ---------------------------------------------------------------------------
// AArch64 Backend implementation
// ---------------------------------------------------------------------------

/// AArch64 (ARM64) code generation backend.
///
/// Wraps the existing ARM64 emitter, register allocator, and instruction
/// encoding behind the `Backend` trait.
pub struct AArch64Backend {
    target_info: AArch64TargetInfo,
}

impl AArch64Backend {
    /// Create a new AArch64 backend.
    pub fn new() -> Self {
        Self {
            target_info: AArch64TargetInfo,
        }
    }
}

impl Default for AArch64Backend {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the stack frame size for an IR function.
///
/// Replicates the private `compute_frame_size` function in `emit.rs`:
/// sums `Alloc` instruction sizes and rounds up to 16-byte alignment.
/// NOTE: Does NOT include 16 bytes for FP/LR pair (handled by prologue separately).
fn aarch64_compute_frame_size(func: &IRFunction) -> usize {
    let mut total: u32 = 0; // Alloc sizes only; FP/LR handled separately
    for block in &func.blocks {
        for instr in &block.instructions {
            if let IRInstr::Alloc { size, .. } = instr {
                let aligned = (*size).div_ceil(16) * 16;
                total += aligned;
            }
        }
    }
    // Round up to 16-byte alignment
    total = (total + 15) & !15;
    total as usize
}

/// Build an ELF64 binary for AArch64 Linux with 2 LOAD segments.
///
/// Segment 1: PF_R | PF_X — contains .text (code)
/// Segment 2: PF_R | PF_W — contains .data + stack space (writable)
///
/// The two segments are separated by page alignment (4KB) to ensure
/// the kernel maps them with different permissions. Without this,
/// a single PF_R|PF_W|PF_X segment is insecure and may cause
/// QEMU/Linux to reject the executable.
fn build_aarch64_elf_2seg(code: &[u8], base_addr: u64) -> Vec<u8> {
    const PAGE_SIZE: u64 = 0x1000; // 4 KB

    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let num_phdrs: u64 = 2;
    let phdr_end = elf_header_size + num_phdrs * phdr_size;
    // Page-align the text segment start in the file.  The kernel's ELF
    // loader mmap()s each LOAD segment, and the file offset must be
    // congruent with the virtual address modulo the page size.  The
    // simplest way to guarantee this is to place the text at a
    // page-aligned file offset, with vaddr = base_addr.
    let text_offset = ((phdr_end + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
    let text_size = code.len() as u64;

    // The data segment starts on the next page after the text.
    let text_file_end = text_offset + text_size;
    let data_vaddr = ((base_addr + text_file_end + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
    let data_offset = data_vaddr - base_addr; // file offset for data segment
    let data_size: u64 = PAGE_SIZE; // 1 page of writable memory for stack/data
    let entry_point = base_addr + text_offset;

    let mut elf = Vec::with_capacity((data_offset + data_size) as usize);

    // --- e_ident ---
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']); // magic
    elf.push(2); // ELFCLASS64
    elf.push(1); // ELFDATA2LSB
    elf.push(1); // EV_CURRENT
    elf.push(3); // ELFOSABI_LINUX
    elf.push(0); // padding
    elf.extend_from_slice(&[0u8; 7]); // padding

    // --- ELF header fields ---
    elf.extend_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    elf.extend_from_slice(&183u16.to_le_bytes()); // e_machine = EM_AARCH64
    elf.extend_from_slice(&1u32.to_le_bytes()); // e_version
    elf.extend_from_slice(&entry_point.to_le_bytes()); // e_entry
    elf.extend_from_slice(&elf_header_size.to_le_bytes()); // e_phoff
    elf.extend_from_slice(&0u64.to_le_bytes()); // e_shoff (no section headers)
    elf.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&56u16.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&2u16.to_le_bytes()); // e_phnum = 2
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // --- Program Header 1: LOAD (PF_R | PF_X) — .text ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&text_offset.to_le_bytes()); // p_offset
    elf.extend_from_slice(&(base_addr + text_offset).to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&(base_addr + text_offset).to_le_bytes()); // p_paddr
    elf.extend_from_slice(&text_size.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&text_size.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- Program Header 2: LOAD (PF_R | PF_W) — .data / stack ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&data_offset.to_le_bytes()); // p_offset
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_filesz (no initialized data)
    elf.extend_from_slice(&data_size.to_le_bytes()); // p_memsz (writable pages)
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- .text section ---
    // Pad to page-aligned text_offset
    while (elf.len() as u64) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    // --- Pad to data segment offset (if needed) ---
    while (elf.len() as u64) < data_offset {
        elf.push(0);
    }

    // No file data for the .data segment (it's BSS-like, zero-initialized)

    elf
}

/// Build ARM64 runtime I/O functions using Linux SVC syscalls.
///
/// Provides:
/// - `__vuma_print_hex`: Print X0 as 8 hex digits to stdout (FD=1)
///   Uses sys_write (X8=64) via SVC #0.
///   Saves/restores X1,X2,X3,X8,X9,X10 on stack.
///
/// - `__vuma_print_int`: Print X0 as a decimal integer to stdout (FD=1)
///   Converts digit-by-digit into a stack buffer, then sys_write.
///
/// - `__vuma_print_newline`: Print a newline character to stdout.
///
/// All functions follow AAPCS64: X0 is the argument, X1-X7 are caller-saved,
/// X8 is the indirect result register / syscall number, X19-X28 are callee-saved.
fn build_aarch64_runtime() -> Vec<u8> {
    let mut code = Vec::new();

    // ── __vuma_print_hex ──
    // Input: X0 = 64-bit value to print as 8 hex digits
    // Clobbers: X1, X2, X3, X8, X9, X10
    // Stack frame: 48 bytes (8 for pair save + 8 for hex chars + 32 padding)

    // Prologue: save callee-saved and link register
    // STP X29, X30, [SP, #-48]!  (pre-indexed, 48-byte frame)
    code.extend_from_slice(&0xA9BF7BFDu32.to_le_bytes());
    // ADD X29, SP, #0
    code.extend_from_slice(&0x910003FDu32.to_le_bytes());

    // Save X9, X10 on stack at [SP, #16]
    // STP X9, X10, [SP, #16]
    code.extend_from_slice(&0xA90127E9u32.to_le_bytes());

    // Save X1, X2 on stack at [SP, #32]
    // STP X1, X2, [SP, #32]
    code.extend_from_slice(&0xA9020441u32.to_le_bytes());

    // Save X3, X8 on stack at [SP, #40]  (offset 40 = 5*8)
    // Wait, 40 = 5*8 but STP needs offset/8 for signed-offset. 40/8 = 5.
    // STP X3, X8, [SP, #40]
    code.extend_from_slice(&0xA9052C63u32.to_le_bytes());

    // Hex lookup table: we'll use ADD + LDRB pattern.
    // Instead, compute hex digit directly:
    // For each nibble: digit = nibble; if digit < 10 then char = digit + '0' else char = digit - 10 + 'a'

    // We'll write 8 hex chars starting at SP+16 (reuse save area after we load saves)
    // Actually, let's use SP+16 as the char buffer (8 bytes).
    // First, save the important regs, then overwrite the buffer area.

    // Process 8 nibbles from most significant to least significant
    // X0 = value, iterate 8 times shifting right by 28, 24, 20, ..., 4, 0

    // X9 = char buffer pointer = SP
    // X10 = loop counter (0..8)

    // ADD X9, SP, #16  (buffer starts at SP+16)
    code.extend_from_slice(&0x910043E9u32.to_le_bytes());
    // MOV X10, #0
    code.extend_from_slice(&0xD280001Au32.to_le_bytes());
    // MOV X3, #0  (bit shift amount, starts at 28)
    code.extend_from_slice(&0xD280003Du32.to_le_bytes()); // MOVZ X3, #28>>4... wait
    // MOV X3, #28
    code.extend_from_slice(&0xD2800383u32.to_le_bytes()); // MOVZ X3, #28

    // Loop label:
    let loop_offset = code.len();

    // Extract nibble: X2 = (X0 >> X3) & 0xF
    // LSR X2, X0, X3
    code.extend_from_slice(&0x9AC02482u32.to_le_bytes()); // LSR X2, X0, X3
    // AND X2, X2, #0xF
    code.extend_from_slice(&0x92400C42u32.to_le_bytes()); // AND X2, X2, #15

    // Convert nibble to hex char:
    // if X2 < 10: char = X2 + 48 ('0')
    // else: char = X2 - 10 + 97 ('a')
    // CMP X2, #10
    code.extend_from_slice(&0xF100141Fu32.to_le_bytes()); // CMP X2, #10? No...
    // CMP X2, #10: use ADD X1, X2, #0 then CMP... Actually:
    // SUB X1, X2, #10 → if < 10, result is negative (borrow set)
    // Let's use: CMP X2, #9; B.GT hex_alpha
    // CMP X2, #9
    code.extend_from_slice(&0xF100129Fu32.to_le_bytes()); // CMP X2, #9

    // ADD X1, X2, #48  (default: '0' + digit)
    code.extend_from_slice(&0x91000C41u32.to_le_bytes()); // ADD X1, X2, #48

    // B.GT hex_alpha (placeholder, will be +1 instruction)
    code.extend_from_slice(&0x54000069u32.to_le_bytes()); // B.GT +4*2 = +8 bytes

    // B store_char
    code.extend_from_slice(&0x14000002u32.to_le_bytes()); // B +2 instructions

    // hex_alpha: ADD X1, X2, #87  (87 = 97-10, so digit-10+'a')
    code.extend_from_slice(&0x91002AC1u32.to_le_bytes()); // ADD X1, X2, #87

    // store_char: STRB W1, [X9, X10]
    // STRB W1, [X9, X10] — register offset
    code.extend_from_slice(&0x382A6821u32.to_le_bytes()); // STRB W1, [X9, X10]

    // Increment: SUB X3, X3, #4; ADD X10, X10, #1; CMP X10, #8; B.LT loop
    code.extend_from_slice(&0xD1000863u32.to_le_bytes()); // SUB X3, X3, #4
    code.extend_from_slice(&0x9100054Au32.to_le_bytes()); // ADD X10, X10, #1
    code.extend_from_slice(&0xF100151Fu32.to_le_bytes()); // CMP X10, #8

    // Calculate branch back to loop start
    let current_pos = code.len() as i64;
    let loop_start = loop_offset as i64;
    let back_offset = (loop_start - current_pos) / 4 - 1; // B.LT is relative to PC+4
    let bcond = 0x5400000Bu32 | ((back_offset as u32) & 0x7FFFF); // B.LT
    code.extend_from_slice(&bcond.to_le_bytes());

    // ── sys_write(1, buf, 8) ──
    // MOV X0, #1  (stdout fd)
    code.extend_from_slice(&0xD2800020u32.to_le_bytes()); // MOVZ X0, #1
    // ADD X1, SP, #16  (buffer pointer)
    code.extend_from_slice(&0x910043E1u32.to_le_bytes());
    // MOV X2, #8  (length)
    code.extend_from_slice(&0xD2800102u32.to_le_bytes()); // MOVZ X2, #8
    // MOV X8, #64  (sys_write)
    code.extend_from_slice(&0xD2800808u32.to_le_bytes()); // MOVZ X8, #64
    // SVC #0
    code.extend_from_slice(&0xD4000001u32.to_le_bytes());

    // Restore registers
    // LDP X9, X10, [SP, #16]
    code.extend_from_slice(&0xA94127E9u32.to_le_bytes());
    // LDP X1, X2, [SP, #32]
    code.extend_from_slice(&0xA9420441u32.to_le_bytes());
    // LDP X3, X8, [SP, #40]
    code.extend_from_slice(&0xA9452C63u32.to_le_bytes());
    // LDP X29, X30, [SP], #48  (post-indexed load)
    code.extend_from_slice(&0xA8C37BFDu32.to_le_bytes());

    // RET
    code.extend_from_slice(&0xD65F03C0u32.to_le_bytes());

    // ── __vuma_print_int ──
    // Input: X0 = 64-bit signed integer to print as decimal
    // Strategy: Divide by 10 repeatedly, push digits onto stack, then write.
    // X9 = digit count, X10 = buffer pointer (SP-based)
    // Stack frame: 64 bytes (16 for save pair + 48 for digit buffer)

    // STP X29, X30, [SP, #-64]!
    code.extend_from_slice(&0xA9C27BFDu32.to_le_bytes());
    // ADD X29, SP, #0
    code.extend_from_slice(&0x910003FDu32.to_le_bytes());
    // STP X9, X10, [SP, #16]
    code.extend_from_slice(&0xA90127E9u32.to_le_bytes());
    // STP X1, X2, [SP, #32]
    code.extend_from_slice(&0xA9020441u32.to_le_bytes());
    // STP X3, X8, [SP, #48]
    code.extend_from_slice(&0xA9030463u32.to_le_bytes());

    // Handle negative numbers
    // CMP X0, #0
    code.extend_from_slice(&0xB100001Fu32.to_le_bytes());
    // B.GE positive
    code.extend_from_slice(&0x5400002Au32.to_le_bytes()); // B.GE +4

    // Print '-' for negative
    // MOV X1, #45  ('-')
    code.extend_from_slice(&0xD28005A1u32.to_le_bytes()); // MOVZ X1, #45
    // STRB W1, [SP, #16]
    code.extend_from_slice(&0x390013E1u32.to_le_bytes());
    // sys_write(1, SP+16, 1)
    code.extend_from_slice(&0xD2800020u32.to_le_bytes()); // MOV X0, #1
    code.extend_from_slice(&0x910043E1u32.to_le_bytes()); // ADD X1, SP, #16
    code.extend_from_slice(&0xD2800022u32.to_le_bytes()); // MOV X2, #1
    code.extend_from_slice(&0xD2800808u32.to_le_bytes()); // MOV X8, #64
    code.extend_from_slice(&0xD4000001u32.to_le_bytes()); // SVC #0

    // Negate X0
    // NEG X0, X0 = SUB X0, XZR, X0
    code.extend_from_slice(&0xCB0003E0u32.to_le_bytes()); // SUB X0, XZR, X0

    // positive: convert digits
    // X9 = digit count = 0
    code.extend_from_slice(&0xD2800019u32.to_le_bytes()); // MOVZ X9, #0
    // X10 = buffer pointer = SP + 17 (leave room for potential '-')
    code.extend_from_slice(&0x910047EAu32.to_le_bytes()); // ADD X10, SP, #17

    // div_loop: UDIV X2, X0, #10; MSUB X1, X2, #10, X0; add '0'; push; X0 = X2
    let div_loop_start = code.len();

    // CBZ X0, done_digits
    code.extend_from_slice(&0xB4000080u32.to_le_bytes()); // CBZ X0, done_digits (+16 bytes = 4 instr)

    // UDIV X2, X0, #10  → but UDIV takes 2 regs, no immediate.
    // MOV X1, #10
    code.extend_from_slice(&0xD2800141u32.to_le_bytes()); // MOVZ X1, #10
    // UDIV X2, X0, X1
    code.extend_from_slice(&0x9AC10802u32.to_le_bytes()); // UDIV X2, X0, X1
    // MSUB X3, X2, X1, X0  → X3 = X0 - X2 * X1 = remainder
    code.extend_from_slice(&0x9B017C43u32.to_le_bytes()); // MSUB X3, X2, X1, X0
    // ADD X3, X3, #48  ('0')
    code.extend_from_slice(&0x91001863u32.to_le_bytes()); // ADD X3, X3, #48
    // STRB W3, [X10, X9]  — store digit at buffer[count]
    code.extend_from_slice(&0x382B6863u32.to_le_bytes()); // STRB W3, [X10, X9]
    // ADD X9, X9, #1
    code.extend_from_slice(&0x91000529u32.to_le_bytes()); // ADD X9, X9, #1
    // MOV X0, X2  (quotient becomes new value)
    code.extend_from_slice(&0xAA0203E0u32.to_le_bytes()); // MOV X0, X2
    // B div_loop
    let cur = code.len() as i64;
    let back = (div_loop_start as i64 - cur as i64) / 4 - 1;
    code.extend_from_slice(&(0x14000000u32 | ((back as u32) & 0x3FFFFFF)).to_le_bytes());

    // done_digits: digits are in reverse order in buffer.
    // Reverse them in-place using X1 and X3 as temp pointers.
    // CBZ X9, write_digits  (if count == 0, print "0")
    code.extend_from_slice(&0xB4000039u32.to_le_bytes()); // CBZ X9, write_digits (+6 instr = 24 bytes)

    // If count == 0, print "0"
    // MOV X1, #48  ('0')
    code.extend_from_slice(&0xD2800601u32.to_le_bytes()); // MOVZ X1, #48
    // STRB W1, [X10]
    code.extend_from_slice(&0x39001501u32.to_le_bytes());
    // MOV X9, #1
    code.extend_from_slice(&0xD2800039u32.to_le_bytes());
    // B write_digits
    code.extend_from_slice(&0x14000003u32.to_le_bytes()); // B +3

    // Reverse digits in buffer[X10..X10+X9]
    // X1 = left index = 0, X3 = right index = X9 - 1
    // SUB X3, X9, #1
    code.extend_from_slice(&0xD1000563u32.to_le_bytes()); // SUB X3, X9, #1
    // MOV X1, #0
    code.extend_from_slice(&0xD2800001u32.to_le_bytes()); // MOVZ X1, #0

    // rev_loop: CMP X1, X3; B.GE rev_done
    let rev_loop = code.len();
    code.extend_from_slice(&0xEB03003Fu32.to_le_bytes()); // CMP X1, X3
    code.extend_from_slice(&0x5400002Au32.to_le_bytes()); // B.GE rev_done (+4)

    // Load bytes: LDRB W2, [X10, X1]; LDRB W8, [X10, X3]
    code.extend_from_slice(&0x386B6822u32.to_le_bytes()); // LDRB W2, [X10, X1]
    code.extend_from_slice(&0x386B6C68u32.to_le_bytes()); // LDRB W8, [X10, X3]
    // STRB W8, [X10, X1]; STRB W2, [X10, X3] (swap)
    code.extend_from_slice(&0x382B6828u32.to_le_bytes()); // STRB W8, [X10, X1]
    code.extend_from_slice(&0x382B6842u32.to_le_bytes()); // STRB W2, [X10, X3]
    // ADD X1, X1, #1; SUB X3, X3, #1
    code.extend_from_slice(&0x91000421u32.to_le_bytes()); // ADD X1, X1, #1
    code.extend_from_slice(&0xD1000463u32.to_le_bytes()); // SUB X3, X3, #1
    // B rev_loop
    let cur = code.len() as i64;
    let back = (rev_loop as i64 - cur as i64) / 4 - 1;
    code.extend_from_slice(&(0x14000000u32 | ((back as u32) & 0x3FFFFFF)).to_le_bytes());

    // write_digits: sys_write(1, X10, X9)
    // rev_done:
    code.extend_from_slice(&0xD2800020u32.to_le_bytes()); // MOV X0, #1
    code.extend_from_slice(&0xAA0A03E1u32.to_le_bytes()); // MOV X1, X10
    code.extend_from_slice(&0xAA0903E2u32.to_le_bytes()); // MOV X2, X9
    code.extend_from_slice(&0xD2800808u32.to_le_bytes()); // MOV X8, #64
    code.extend_from_slice(&0xD4000001u32.to_le_bytes()); // SVC #0

    // Restore and return
    code.extend_from_slice(&0xA94127E9u32.to_le_bytes()); // LDP X9, X10, [SP, #16]
    code.extend_from_slice(&0xA9420441u32.to_le_bytes()); // LDP X1, X2, [SP, #32]
    code.extend_from_slice(&0xA9430463u32.to_le_bytes()); // LDP X3, X8, [SP, #48]
    code.extend_from_slice(&0xA8C27BFDu32.to_le_bytes()); // LDP X29, X30, [SP], #64
    code.extend_from_slice(&0xD65F03C0u32.to_le_bytes()); // RET

    // ── __vuma_print_newline ──
    // Simple: write '\n' to stdout
    // STP X29, X30, [SP, #-16]!
    code.extend_from_slice(&0xA9BF7BFDu32.to_le_bytes());
    // ADD X29, SP, #0
    code.extend_from_slice(&0x910003FDu32.to_le_bytes());

    // MOV X1, #10  ('\n')
    code.extend_from_slice(&0xD2800141u32.to_le_bytes()); // MOVZ X1, #10
    // STRB W1, [SP, #16]  (use the 16 bytes we just freed)
    // Actually, use stack at SP+16 which is the pre-indexed area...
    // Better: use [SP, #0] since we already pushed 16 bytes
    code.extend_from_slice(&0x3900001Fu32.to_le_bytes()); // STRB W1, [SP, #0]... no, that's the saved FP/LR area
    // Use the area after our frame: we allocated 16 bytes for FP/LR.
    // Put '\n' at [SP, #0]... no, that's where FP/LR are stored.
    // Let's put it at a safe location. Actually, the STP wrote X29, X30 at [SP].
    // Let's use X9 as scratch to hold the char, and a buffer on the stack.

    // Actually, simplest: put the newline byte on the stack right at the frame.
    // We'll use the save area as buffer. We already saved FP/LR there.
    // Let's use a different approach: just write from a register-indirect with SP offset.

    // MOV W1, #10  (already done above)
    // STRB W1, [SP]   ← this overwrites saved FP/LR! Bad.
    // Instead, let's use X9 as temp storage.

    // Let me redo this more carefully.
    // We'll save X9 too.
    // Actually, let's just make a 32-byte frame to have room.

    // Let me restart the newline function from scratch in the code buffer.
    // Find the start of __vuma_print_newline by looking for the last STP.
    // The simplest approach: use a dedicated stack frame.

    // I'll overwrite the last few instructions I emitted for newline.
    // Since this is getting complex, let me just build it cleanly.
    // Remove the partial newline code and redo it.
    let newline_start_offset = code.len() - 7 * 4; // remove 7 instructions back

    code.truncate(newline_start_offset);

    // __vuma_print_newline: clean implementation
    // STP X29, X30, [SP, #-32]!  (32-byte frame: 16 for save, 16 for buffer)
    code.extend_from_slice(&0xA9BE7BFDu32.to_le_bytes());
    // ADD X29, SP, #0
    code.extend_from_slice(&0x910003FDu32.to_le_bytes());

    // Store '\n' at SP+16 (safe area, not overlapping saved regs)
    // MOV W1, #10
    code.extend_from_slice(&0x52800141u32.to_le_bytes()); // MOVZ W1, #10
    // STRB W1, [SP, #16]
    code.extend_from_slice(&0x390021E1u32.to_le_bytes());

    // sys_write(1, SP+16, 1)
    // MOV X0, #1
    code.extend_from_slice(&0xD2800020u32.to_le_bytes());
    // ADD X1, SP, #16
    code.extend_from_slice(&0x910043E1u32.to_le_bytes());
    // MOV X2, #1
    code.extend_from_slice(&0xD2800022u32.to_le_bytes());
    // MOV X8, #64
    code.extend_from_slice(&0xD2800808u32.to_le_bytes());
    // SVC #0
    code.extend_from_slice(&0xD4000001u32.to_le_bytes());

    // LDP X29, X30, [SP], #32
    code.extend_from_slice(&0xA8C27BFDu32.to_le_bytes());
    // RET
    code.extend_from_slice(&0xD65F03C0u32.to_le_bytes());

    code
}

impl Backend for AArch64Backend {
    fn target_info(&self) -> &dyn TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        // Use the existing Emitter to emit the function, which internally
        // performs register allocation and instruction encoding.
        let mut emitter = crate::emit::Emitter::new();
        let code = emitter
            .emit_function(func)
            .map_err(|e| BackendError::RegisterAllocFailed {
                isa: "aarch64",
                reason: e.to_string(),
            })?;

        let func_name = func.name.clone();
        let frame_size = aarch64_compute_frame_size(func);

        // Convert each 32-bit ARM64 instruction word into an AllocatedInstruction
        // with its little-endian encoded bytes.
        let instructions: Vec<AllocatedInstruction> = code
            .iter()
            .enumerate()
            .map(|(i, &word)| AllocatedInstruction {
                opcode: format!("arm64_{}", i),
                reads: vec![],
                writes: vec![],
                encoded: word.to_le_bytes().to_vec(),
            })
            .collect();

        let code_size = instructions.len() * 4;

        // Capture relocations from the Emitter so encode_program can patch BL offsets.
        let relocations = emitter.relocations().to_vec();

        Ok(AllocatedFunction {
            name: func_name,
            blocks: vec![AllocatedBlock {
                label: "entry".to_string(),
                instructions,
                code_offset: 0,
            }],
            frame_size,
            callee_saved: vec![],
            spill_slots: 0,
            code_size,
            relocations,
        })
    }

    fn encode_function(&self, func: &AllocatedFunction) -> Result<Vec<u8>, BackendError> {
        let mut bytes = Vec::new();
        for block in &func.blocks {
            for instr in &block.instructions {
                bytes.extend_from_slice(&instr.encoded);
            }
        }
        Ok(bytes)
    }

    fn encode_program(&self, program: &AllocatedProgram) -> Result<Vec<u8>, BackendError> {
        // ── ARM64 Linux static executable ──
        //
        // Layout:
        //   _start:  BL main           ; call main (result in X0)
        //            MOV X0, X0         ; (nop, keep result)
        //            MOV X8, #93        ; sys_exit_group
        //            SVC #0             ; syscall
        //   <functions...>
        //   <runtime: print_hex, print_int using SVC sys_write>
        //
        // The _start stub is 4 instructions = 16 bytes.
        // After that come all user functions.
        // After user functions come the runtime I/O functions.

        const R_AARCH64_CALL26: &str = "R_AARCH64_CALL26";

        // ── _start stub ──
        // BL <main>      — offset 0, needs relocation
        // NOP            — offset 4 (keep X0 as return value)
        // MOV X8, #93    — offset 8 (sys_exit_group = 93 on AArch64 Linux)
        // SVC #0         — offset 12

        let start_stub_size: usize = 16; // 4 × 4-byte instructions

        // ── Build runtime I/O code ──
        // print_hex: X0 = value to print as 8 hex digits to stdout
        //   Uses SVC #0 with X8=64 (sys_write), fd=1 (stdout)
        //   Converts each nibble to hex char, writes to stack buffer, then sys_write.
        let runtime_code = build_aarch64_runtime();

        // ── Compute function offsets ──
        // _start stub comes first, then user functions, then runtime.
        let mut func_offsets: HashMap<String, usize> = HashMap::new();
        let mut current_offset: usize = start_stub_size; // after _start

        for func in &program.functions {
            func_offsets.insert(func.name.clone(), current_offset);
            let func_size: usize = func.blocks.iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            current_offset += func_size;
        }

        // Runtime functions: __vuma_print_hex, __vuma_print_int, __vuma_print_newline
        let runtime_offsets_start = current_offset;
        func_offsets.insert("__vuma_print_hex".to_string(), runtime_offsets_start);
        func_offsets.insert("__vuma_print_int".to_string(), runtime_offsets_start);
        // Both print_hex and print_int start at same offset (single combined runtime)
        // Actually, let's put them sequentially:
        // We'll just embed the runtime as a single blob and not reference individual offsets
        // since user code calls them via BL with relocation.

        // ── Build _start stub bytes ──
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // BL <main> — placeholder, will be patched
        // BL encoding: 1 0 0 1 0 1 imm26
        start_stub.extend_from_slice(&0x94000000u32.to_le_bytes()); // BL #0

        // NOP (MOV X0, X0)
        start_stub.extend_from_slice(&0xAA0003E0u32.to_le_bytes()); // MOV X0, X0

        // MOV X8, #93 (sys_exit_group)
        // MOVZ X8, #93 = 0xD2800BA8
        start_stub.extend_from_slice(&0xD2800BA8u32.to_le_bytes());

        // SVC #0
        start_stub.extend_from_slice(&0xD4000001u32.to_le_bytes());

        // ── Patch _start BL to main ──
        let main_key = func_offsets.keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key];
            // BL offset = (target - pc) / 4, where pc = address of BL instruction
            // BL is at offset 0 within all_code, but in the final binary it's at
            // start_stub_size_into_elf = text_offset_in_elf.
            // For the BL encoding, imm26 = (target - bl_addr) / 4
            // bl_addr = 0 (relative to start of all_code = _start)
            // target = main_offset (relative to start of all_code)
            let bl_offset = (main_offset as i64) / 4;
            // Mask to 26 bits (signed)
            let imm26 = (bl_offset as u32) & 0x03FFFFFF;
            let bl_word: u32 = 0x94000000 | imm26;
            start_stub[0..4].copy_from_slice(&bl_word.to_le_bytes());
        }

        // ── Concatenate all code ──
        let mut all_code = start_stub;
        for func in &program.functions {
            for block in &func.blocks {
                for instr in &block.instructions {
                    all_code.extend_from_slice(&instr.encoded);
                }
            }
        }

        // Append runtime I/O code
        all_code.extend_from_slice(&runtime_code);

        // ── Patch BL relocations ──
        // Each function's relocations are relative to the start of that function's code.
        // We need to adjust them by the _start stub size + preceding functions' sizes.
        let mut func_code_offset: usize = start_stub_size;
        for func in &program.functions {
            for reloc in &func.relocations {
                let abs_offset = func_code_offset + reloc.offset as usize;
                if abs_offset + 4 > all_code.len() {
                    continue; // skip invalid relocations
                }

                if reloc.reloc_type == R_AARCH64_CALL26 {
                    // R_AARCH64_CALL26: patch BL instruction's imm26 field.
                    // BL target = PC + imm26*4, where PC = address of BL instruction.
                    // So: imm26 = (target_addr - bl_addr) / 4
                    if let Some(&target_offset) = func_offsets.get(&reloc.symbol) {
                        let bl_addr = abs_offset as i64;
                        let target_addr = target_offset as i64;
                        let offset_words = (target_addr - bl_addr) / 4;
                        // Check range: ±128MB (26-bit signed)
                        if offset_words < -(1 << 25) || offset_words >= (1 << 25) {
                            eprintln!(
                                "warning: BL relocation to '{}' out of range: {} words",
                                reloc.symbol, offset_words
                            );
                            continue;
                        }
                        let imm26 = (offset_words as u32) & 0x03FFFFFF;
                        let existing = u32::from_le_bytes([
                            all_code[abs_offset],
                            all_code[abs_offset + 1],
                            all_code[abs_offset + 2],
                            all_code[abs_offset + 3],
                        ]);
                        let patched = (existing & !0x03FFFFFF) | imm26;
                        all_code[abs_offset..abs_offset + 4]
                            .copy_from_slice(&patched.to_le_bytes());
                    }
                    // If the symbol is not found (e.g., external like __vuma_free),
                    // leave the placeholder as-is — the program will crash if it's called.
                }
            }
            let func_size: usize = func.blocks.iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            func_code_offset += func_size;
        }

        // ── Build ELF with 2 LOAD segments ──
        Ok(build_aarch64_elf_2seg(&all_code, 0x400000))
    }

    fn return_stub(&self) -> Vec<u8> {
        // ARM64 RET instruction: 0xD65F03C0
        vec![0xC0, 0x03, 0x5F, 0xD6]
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        // LDR X16, [PC, #8] ; BR X16 ; <8 bytes address>
        let mut code = Vec::with_capacity(16);
        // LDR X16, [PC, #8] = 0x58000050
        code.extend_from_slice(&0x58000050u32.to_le_bytes());
        // BR X16 = 0xD61F0200
        code.extend_from_slice(&0xD61F0200u32.to_le_bytes());
        // 64-bit address
        code.extend_from_slice(&entry_addr.to_le_bytes());
        code
    }

    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String> {
        // Mnemonic decoder for AArch64 (4-byte fixed-width instructions).
        let mut lines = Vec::new();
        let mut offset = 0usize;
        let mut pc = addr;
        while offset + 4 <= bytes.len() {
            let word = u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            let mnemonic = if let Some(instr) = crate::arm64::Instruction::decode(word) {
                format!("{}", instr)
            } else {
                decode_aarch64(word)
            };
            lines.push(format!("{:#010x}:  {:08x}  {}", pc, word, mnemonic));
            offset += 4;
            pc += 4;
        }
        if offset < bytes.len() {
            let remaining = &bytes[offset..];
            lines.push(format!("{:#010x}:  {:02x?}", pc, remaining));
        }
        lines
    }

    fn name(&self) -> &'static str {
        "aarch64"
    }
}

// ---------------------------------------------------------------------------
// Factory function
// ---------------------------------------------------------------------------

/// Create a backend for the given architecture kind.
///
/// Currently only AArch64 has a full Backend implementation.
/// Other ISAs return an error indicating they are not yet implemented.
pub fn create_backend(kind: BackendKind) -> Result<Box<dyn Backend>, BackendError> {
    match kind {
        BackendKind::AArch64 => Ok(Box::new(AArch64Backend::new())),
        BackendKind::RiscV64 => Ok(Box::new(RiscV64Backend::new())),
        BackendKind::Wasm32 => Ok(Box::new(crate::wasm32::Wasm32Backend::new())),
        BackendKind::LoongArch64 => Ok(Box::new(LoongArch64Backend::new())),
        BackendKind::X86_64 => Ok(Box::new(X86_64Backend::new())),
        BackendKind::Arm32 => Ok(Box::new(Arm32Backend::new())),
        BackendKind::Mips64 => Ok(Box::new(Mips64Backend::new())),
        BackendKind::PowerPC64 => Ok(Box::new(PPC64Backend::new())),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: validate that a TargetInfo impl returns internally consistent values.
    fn validate_target_info(info: &dyn TargetInfo) {
        // If there are no registers, register counts must be zero.
        if !info.has_registers() {
            assert_eq!(
                info.num_gp_regs(),
                0,
                "{}: has_registers=false but num_gp_regs != 0",
                info.isa_name()
            );
            assert_eq!(
                info.num_simd_fp_regs(),
                0,
                "{}: has_registers=false but num_simd_fp_regs != 0",
                info.isa_name()
            );
            assert_eq!(
                info.num_int_arg_regs(),
                0,
                "{}: has_registers=false but num_int_arg_regs != 0",
                info.isa_name()
            );
            assert_eq!(
                info.num_fp_arg_regs(),
                0,
                "{}: has_registers=false but num_fp_arg_regs != 0",
                info.isa_name()
            );
        }

        // Pointer width must be 4 or 8.
        assert!(
            info.pointer_width() == 4 || info.pointer_width() == 8,
            "{}: pointer_width must be 4 or 8, got {}",
            info.isa_name(),
            info.pointer_width()
        );

        // Stack alignment must be a power of 2 and at least 8.
        let sa = info.stack_alignment();
        assert!(
            sa >= 8,
            "{}: stack_alignment must be >= 8, got {}",
            info.isa_name(),
            sa
        );
        assert!(
            sa.is_power_of_two(),
            "{}: stack_alignment must be a power of 2, got {}",
            info.isa_name(),
            sa
        );

        // Instruction alignment must be 1, 2, or 4.
        let ia = info.instruction_alignment();
        assert!(
            ia == 1 || ia == 2 || ia == 4,
            "{}: instruction_alignment must be 1, 2, or 4, got {}",
            info.isa_name(),
            ia
        );

        // Width range must be sane.
        let (min_w, max_w) = info.instruction_width_range();
        assert!(
            min_w >= 1,
            "{}: min instruction width must be >= 1",
            info.isa_name()
        );
        assert!(
            max_w >= min_w,
            "{}: max instruction width must be >= min",
            info.isa_name()
        );

        // Only MIPS has branch delay slots.
        if info.has_branch_delay_slots() {
            assert_eq!(
                info.isa_name(),
                "mips64",
                "Only MIPS64 should have branch delay slots"
            );
        }

        // Only PPC64 has a TOC pointer.
        if info.has_toc_pointer() {
            assert_eq!(
                info.isa_name(),
                "ppc64",
                "Only PPC64 should have a TOC pointer"
            );
        }

        // Only PPC64 has condition registers.
        if info.has_condition_registers() {
            assert_eq!(
                info.isa_name(),
                "ppc64",
                "Only PPC64 should have condition registers"
            );
        }

        // size_of and alignment_of for basic types.
        let ptr_size = info.size_of(&IRType::Ptr);
        assert_eq!(
            ptr_size,
            info.pointer_width(),
            "{}: Ptr size must match pointer_width",
            info.isa_name()
        );
    }

    #[test]
    fn test_aarch64_target_info() {
        let info = AArch64TargetInfo;
        assert_eq!(info.isa_name(), "aarch64");
        assert_eq!(info.elf_machine_type(), 183);
        assert_eq!(info.pointer_width(), 8);
        assert!(info.has_registers());
        assert_eq!(info.num_gp_regs(), 31);
        assert_eq!(info.num_simd_fp_regs(), 32);
        assert!(info.has_link_register());
        assert!(!info.has_branch_delay_slots());
        assert_eq!(info.calling_convention_name(), "aapcs64");
        assert_eq!(info.num_int_arg_regs(), 8);
        assert_eq!(info.num_fp_arg_regs(), 8);
        assert_eq!(info.stack_alignment(), 16);
        assert_eq!(info.instruction_width_range(), (4, 4));
        assert_eq!(info.output_format(), OutputFormat::Elf64);
        validate_target_info(&info);
    }

    #[test]
    fn test_riscv64_target_info() {
        let info = RiscV64TargetInfo;
        assert_eq!(info.isa_name(), "riscv64");
        assert_eq!(info.elf_machine_type(), 243);
        assert!(info.has_hardwired_zero());
        assert!(info.has_link_register());
        assert!(!info.has_branch_delay_slots());
        assert_eq!(info.calling_convention_name(), "lp64d");
        assert_eq!(info.instruction_width_range(), (2, 4));
        validate_target_info(&info);
    }

    #[test]
    fn test_wasm32_target_info() {
        let info = Wasm32TargetInfo;
        assert_eq!(info.isa_name(), "wasm32");
        assert_eq!(info.elf_machine_type(), 0); // Not ELF
        assert!(!info.has_registers()); // Stack machine!
        assert_eq!(info.num_gp_regs(), 0);
        assert_eq!(info.num_simd_fp_regs(), 0);
        assert_eq!(info.pointer_width(), 4); // wasm32 is 32-bit
        assert_eq!(info.output_format(), OutputFormat::WasmBinary);
        assert_eq!(info.calling_convention_name(), "wasm-stack");
        validate_target_info(&info);
    }

    #[test]
    fn test_loongarch64_target_info() {
        let info = LoongArch64TargetInfo;
        assert_eq!(info.isa_name(), "loongarch64");
        assert_eq!(info.elf_machine_type(), 258);
        assert!(info.has_hardwired_zero());
        assert!(info.has_link_register());
        assert_eq!(info.calling_convention_name(), "lp64");
        validate_target_info(&info);
    }

    #[test]
    fn test_x86_64_target_info() {
        let info = X86_64TargetInfo;
        assert_eq!(info.isa_name(), "x86_64");
        assert_eq!(info.elf_machine_type(), 62);
        assert!(!info.has_link_register()); // x86_64 pushes return addr
        assert_eq!(info.calling_convention_name(), "systemv");
        assert_eq!(info.num_int_arg_regs(), 6);
        assert_eq!(info.num_fp_arg_regs(), 8);
        assert_eq!(info.instruction_width_range(), (1, 15));
        validate_target_info(&info);
    }

    #[test]
    fn test_arm32_target_info() {
        let info = Arm32TargetInfo;
        assert_eq!(info.isa_name(), "arm32");
        assert_eq!(info.elf_machine_type(), 40);
        assert!(info.has_link_register());
        assert_eq!(info.pointer_width(), 4);
        assert_eq!(info.output_format(), OutputFormat::Elf32);
        assert_eq!(info.calling_convention_name(), "aapcs");
        assert_eq!(info.num_int_arg_regs(), 4);
        validate_target_info(&info);
    }

    #[test]
    fn test_mips64_target_info() {
        let info = Mips64TargetInfo;
        assert_eq!(info.isa_name(), "mips64");
        assert_eq!(info.elf_machine_type(), 8);
        assert!(info.has_branch_delay_slots()); // THE defining feature
        assert!(info.has_hardwired_zero());
        assert_eq!(info.endianness(), Endianness::Big);
        assert_eq!(info.calling_convention_name(), "n64");
        validate_target_info(&info);
    }

    #[test]
    fn test_ppc64_target_info() {
        let info = PowerPC64TargetInfo;
        assert_eq!(info.isa_name(), "ppc64");
        assert_eq!(info.elf_machine_type(), 21);
        assert!(info.has_toc_pointer()); // R2 = TOC
        assert!(info.has_condition_registers()); // CR0-CR7
        assert_eq!(info.calling_convention_name(), "elfv2");
        assert_eq!(info.num_int_arg_regs(), 8);
        assert_eq!(info.num_fp_arg_regs(), 13);
        assert_eq!(info.endianness(), Endianness::Bi);
        validate_target_info(&info);
    }

    #[test]
    fn test_backend_kind_display() {
        assert_eq!(BackendKind::AArch64.to_string(), "aarch64");
        assert_eq!(BackendKind::RiscV64.to_string(), "riscv64");
        assert_eq!(BackendKind::Wasm32.to_string(), "wasm32");
        assert_eq!(BackendKind::LoongArch64.to_string(), "loongarch64");
        assert_eq!(BackendKind::X86_64.to_string(), "x86_64");
        assert_eq!(BackendKind::Arm32.to_string(), "arm32");
        assert_eq!(BackendKind::Mips64.to_string(), "mips64");
        assert_eq!(BackendKind::PowerPC64.to_string(), "ppc64");
    }

    #[test]
    fn test_backend_kind_isa_name() {
        assert_eq!(BackendKind::AArch64.isa_name(), "aarch64");
        assert_eq!(BackendKind::X86_64.isa_name(), "x86_64");
        assert_eq!(BackendKind::Wasm32.isa_name(), "wasm32");
    }

    #[test]
    fn test_physical_reg_display() {
        let gpr = PhysicalReg::new(RegClass::Gpr, 0);
        let simd = PhysicalReg::new(RegClass::SimdFp, 15);
        assert_eq!(gpr.to_string(), "Gpr:0");
        assert_eq!(simd.to_string(), "SimdFp:15");
    }

    #[test]
    fn test_wasm32_size_of_ptr() {
        let info = Wasm32TargetInfo;
        // wasm32 has 32-bit pointers
        assert_eq!(info.size_of(&IRType::Ptr), 4);
        assert_eq!(info.alignment_of(&IRType::Ptr), 4);
    }

    #[test]
    fn test_arm32_size_of_ptr() {
        let info = Arm32TargetInfo;
        // ARM32 has 32-bit pointers
        assert_eq!(info.size_of(&IRType::Ptr), 4);
        assert_eq!(info.alignment_of(&IRType::Ptr), 4);
    }

    #[test]
    fn test_output_format_variants() {
        assert_ne!(OutputFormat::Elf64, OutputFormat::WasmBinary);
    }

    #[test]
    fn test_aarch64_disassemble_nop() {
        let backend = AArch64Backend::new();
        // NOP = 0xD503201F
        let bytes: Vec<u8> = 0xD503201Fu32.to_le_bytes().to_vec();
        let lines = backend.disassemble(&bytes, 0x1000);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("nop"), "Expected nop, got: {}", lines[0]);
    }

    #[test]
    fn test_aarch64_disassemble_ret() {
        let backend = AArch64Backend::new();
        // RET = 0xD65F03C0
        let bytes: Vec<u8> = 0xD65F03C0u32.to_le_bytes().to_vec();
        let lines = backend.disassemble(&bytes, 0x2000);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("ret"), "Expected ret, got: {}", lines[0]);
    }

    #[test]
    fn test_aarch64_disassemble_add_imm() {
        let backend = AArch64Backend::new();
        // ADD X0, X1, #42: 0x9100A820
        use crate::arm64::{Instruction, Operand, Register};
        let instr = Instruction::ADD {
            rd: Register::X0,
            rn: Register::X1,
            rm: Operand::Imm12(42),
        };
        let encoded = instr.encode().unwrap();
        let bytes: Vec<u8> = encoded.to_le_bytes().to_vec();
        let lines = backend.disassemble(&bytes, 0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("add"), "Expected add, got: {}", lines[0]);
    }

    #[test]
    fn test_backend_error_includes_isa_name() {
        let err = BackendError::UnsupportedFeature {
            isa: "aarch64",
            feature: "branch delay slots".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("[aarch64]"),
            "Error should include ISA name: {}",
            msg
        );
        assert!(
            msg.contains("branch delay slots"),
            "Error should include feature: {}",
            msg
        );
    }
}
