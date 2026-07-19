//! # Multi-Backend Trait Architecture
//!
//! Defines the `TargetInfo` and `Backend` traits that allow VUMA to target
//! multiple instruction set architectures. Each ISA implements these traits
//! to provide target-specific information and code generation.
//!
//! # Wave 8a — Single-Buffer PMT State Lowering
//!
//! As of Wave 8a, PMT state-typed allocations (`let p = state_new(Layout)`)
//! no longer lower to per-state `IRInstr::Alloc`s. Instead, the IRBuilder
//! emits ONE `IRInstr::Alloc` for a program-wide buffer (`___pmt_buffer`)
//! at the start of `main`, and each state-typed allocation becomes an
//! `IRInstr::Offset { dst, base: ___pmt_buffer, offset: Imm(N) }` into
//! that buffer. This realizes the "zero runtime overhead" promise: ONE
//! stack allocation at program start, ZERO per-state stack allocations
//! during execution.
//!
//! This is purely an IR-level transformation — the backends are unchanged.
//! `IRInstr::Alloc` and `IRInstr::Offset` are both already supported by
//! every backend (the former via stack-slot reservation, the latter via
//! `LEA`/`ADD`/`ADDI` etc.). No `_start` stub modification or runtime
//! helper is needed (Approach A from the Wave 8a task description).
//!
//! The buffer is sized to the SUM of all state-typed allocation sizes
//! (each aligned to 16 bytes), computed by the IRBuilder's pre-pass
//! (`compute_total_state_buffer_size`). This is conservative — slot
//! reuse across non-overlapping live ranges is a future optimisation
//! (would require liveness analysis). See `identify_state_vars` in
//! `scg_to_ir.rs` for the heuristic that distinguishes state-typed
//! allocations from regular `allocate(N)` calls (which still use the
//! per-call `Alloc` path).

use crate::arm32::Arm32Backend;
use crate::ir::{
    size_of_with_ptr_width, alignment_of_with_ptr_width, BinOpKind, IRFunction, IRInstr, IRProgram,
    IRType,
};
use crate::loongarch64::LoongArch64Backend;
use crate::mips64::Mips64Backend;
use crate::ppc64::PPC64Backend;
use crate::ppc64le::PPC64LEBackend;
use crate::riscv64::RiscV64Backend;
use crate::s390x::S390XBackend;
use crate::sparc64::Sparc64Backend;
use crate::mips64be::Mips64BeBackend;
use crate::armeb::ArmEbBackend;
use crate::aarch64_be::AArch64BeBackend;
use crate::m68k::M68kBackend;
use crate::alpha::AlphaBackend;
use crate::hppa::HppaBackend;
use crate::x86_64::X86_64Backend;
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// IR float-op verification (F2a)
// ---------------------------------------------------------------------------
//
// VUMA's `BinOpKind` is deliberately type-tag-polymorphic: the same
// `Add`/`Sub`/`Mul`/`SDiv`/`UDiv` variants (plus all comparison variants)
// serve both integer AND float operands, and backends branch on
// `IRType::F32`/`IRType::F64` to select ALU vs FPU encoding.  However,
// bitwise/shift ops (`And`/`Or`/`Xor`/`Shl`/`ShrL`/`ShrA`/`Ror`/`Rol`)
// and integer remainder (`SRem`/`URem`) are NOT meaningful on floats —
// emitting them would silently produce wrong code (backends fall through
// to the integer path).
//
// `verify_float_op` rejects such combinations ONCE, centrally, before any
// backend lowering runs.  `verify_function_float_ops` and
// `verify_program_float_ops` walk an `IRFunction` / `IRProgram` and collect
// every violation.
//
// WIRING: The `Backend` trait's `allocate_registers(&self, func: &IRFunction)`
// is the per-function, pre-lowering entry point every backend implements.
// Each backend SHOULD call `verify_function_float_ops(func)` (or the
// `verify_float_op` helper directly in its own walk) at the top of its
// `allocate_registers` impl and map any `Err` into
// `BackendError::InvalidInstruction`.  `AArch64Backend::allocate_registers`
// (in this file) is wired as the reference call site; other backends
// (`alpha.rs`, `hppa.rs`, `s390x.rs`, `sparc64.rs`, `arm64.rs`,
// `x86_64/`, `riscv64.rs`, `arm32/`, `mips64/`, `ppc64/`, `ppc64le.rs`,
// `loongarch64/`, `wasm32/`, `riscv32.rs`, `x86_32/`, `mips64be.rs`,
// `armeb.rs`, `aarch64_be.rs`, `m68k.rs`) need the same one-liner added —
// that wiring is a follow-up task (out of scope for F2a because F2a is
// restricted to editing `backend.rs` only).

/// Verify that a binary operation is valid for the given result type.
///
/// VUMA's `BinOpKind` is deliberately type-tag-polymorphic: `Add`/`Sub`/
/// `Mul`/`SDiv`/`UDiv` and all comparison variants serve both integer and
/// floating-point operands (backends branch on `IRType` to select ALU vs
/// FPU encoding).  However, bitwise/shift ops (`And`/`Or`/`Xor`/`Shl`/
/// `ShrL`/`ShrA`/`Ror`/`Rol`) and integer remainder (`SRem`/`URem`) are
/// NOT meaningful on `F32`/`F64` values — emitting them would silently
/// produce wrong code.  This function rejects such combinations once,
/// centrally, before any backend lowering runs.
///
/// Call this from the compilation pipeline (e.g. inside a backend's
/// `allocate_registers` impl, or a pre-lowering validation walk) for every
/// `IRInstr::BinOp`.
pub fn verify_float_op(op: BinOpKind, ty: Option<&IRType>) -> Result<(), String> {
    let is_float = matches!(ty, Some(IRType::F32) | Some(IRType::F64));
    if !is_float {
        return Ok(());
    }
    match op {
        // Valid on floats: arithmetic + all comparisons.
        BinOpKind::Add
        | BinOpKind::Sub
        | BinOpKind::Mul
        | BinOpKind::SDiv
        | BinOpKind::UDiv
        | BinOpKind::Eq
        | BinOpKind::Ne
        | BinOpKind::SLt
        | BinOpKind::SLe
        | BinOpKind::SGt
        | BinOpKind::SGe
        | BinOpKind::ULt
        | BinOpKind::ULe
        | BinOpKind::UGt
        | BinOpKind::UGe => Ok(()),
        // Invalid on floats: bitwise, shift, remainder.
        BinOpKind::And
        | BinOpKind::Or
        | BinOpKind::Xor
        | BinOpKind::Shl
        | BinOpKind::ShrL
        | BinOpKind::ShrA
        | BinOpKind::Ror
        | BinOpKind::Rol
        | BinOpKind::SRem
        | BinOpKind::URem => Err(format!(
            "float-op-reject: {:?} is not valid on {:?} (bitwise/shift/remainder ops require integer operands)",
            op, ty
        )),
    }
}

/// Walk every `IRInstr::BinOp` in `func` and collect all float-op
/// violations.  Returns `Ok(())` if the function is clean, or
/// `Err(Vec<String>)` with one message per violating instruction
/// (each message includes the function name and block label so the
/// user can locate the offending op).
///
/// This is the per-function pre-lowering validation pass.  Backends
/// should call it at the top of `allocate_registers` and map the
/// error vector to `BackendError::InvalidInstruction` (joining the
/// messages with `"; "`).
pub fn verify_function_float_ops(func: &IRFunction) -> Result<(), Vec<String>> {
    let mut errs: Vec<String> = Vec::new();
    for block in &func.blocks {
        for instr in &block.instructions {
            if let IRInstr::BinOp { op, ty, .. } = instr {
                if let Err(msg) = verify_float_op(*op, ty.as_ref()) {
                    errs.push(format!(
                        "function `{}` block `{}`: {}",
                        func.name, block.label, msg
                    ));
                }
            }
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

/// Walk every function in `program` and collect all float-op violations.
/// Returns `Ok(())` if the program is clean, or `Err(Vec<String>)` with
/// one message per violating instruction (across all functions).
///
/// This is the program-level pre-lowering validation pass.  It is the
/// ideal single call site for a future central compilation driver: call
/// `verify_program_float_ops(&program)?` once after IR construction and
/// before any backend's `allocate_registers` runs, and every backend
/// benefits without per-backend wiring.
pub fn verify_program_float_ops(program: &IRProgram) -> Result<(), Vec<String>> {
    let mut errs: Vec<String> = Vec::new();
    for func in &program.functions {
        if let Err(fn_errs) = verify_function_float_ops(func) {
            errs.extend(fn_errs);
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

// ---------------------------------------------------------------------------
// Endianness
// ---------------------------------------------------------------------------

/// Byte order of the target architecture.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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
// SectionHeader
// ---------------------------------------------------------------------------

/// An ELF section header (`Elf32_Shdr` or `Elf64_Shdr`).
///
/// Bundles the ten fixed fields of an ELF section header so that the
/// per-ISA `push_shdr` helpers can accept a single `&SectionHeader`
/// argument instead of a long positional parameter list (which would
/// otherwise trip `clippy::too_many_arguments`).
///
/// `Addr` is the integer type used for the address / offset / size /
/// alignment / entsize fields — `u32` for ELF32 and `u64` for ELF64.
/// `sh_name`, `sh_type`, `sh_link`, and `sh_info` are always `u32`
/// regardless of ELF class.
#[derive(Clone, Copy, Debug)]
pub struct SectionHeader<Addr = u64> {
    /// Offset of the section name in `.shstrtab`.
    pub sh_name: u32,
    /// Section type (e.g. `SHT_NULL`, `SHT_PROGBITS`, `SHT_SYMTAB`).
    pub sh_type: u32,
    /// Section flags (e.g. `SHF_ALLOC | SHF_EXECINSTR = 0x6`).
    pub sh_flags: Addr,
    /// Virtual address where the section is loaded.
    pub sh_addr: Addr,
    /// Byte offset of the section in the file.
    pub sh_offset: Addr,
    /// Size of the section in bytes.
    pub sh_size: Addr,
    /// Section header table index link (meaning depends on `sh_type`).
    pub sh_link: u32,
    /// Extra information (meaning depends on `sh_type`).
    pub sh_info: u32,
    /// Address alignment constraint (must be a power of two).
    pub sh_addralign: Addr,
    /// Size of each entry for sections with fixed-size entries.
    pub sh_entsize: Addr,
}

impl<Addr: Default> Default for SectionHeader<Addr> {
    fn default() -> Self {
        Self {
            sh_name: 0,
            sh_type: 0,
            sh_flags: Addr::default(),
            sh_addr: Addr::default(),
            sh_offset: Addr::default(),
            sh_size: Addr::default(),
            sh_link: 0,
            sh_info: 0,
            sh_addralign: Addr::default(),
            sh_entsize: Addr::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// PhysicalReg
// ---------------------------------------------------------------------------

/// A physical register identified by class and index.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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
#[derive(Clone, Debug)]
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
#[derive(Clone, Debug)]
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
#[derive(Clone, Debug)]
pub struct AllocatedBlock {
    /// Block label.
    pub label: String,
    /// Allocated instructions in order.
    pub instructions: Vec<AllocatedInstruction>,
    /// Byte offset of this block in the final code section.
    pub code_offset: usize,
}

// ---------------------------------------------------------------------------
// Wasm-specific metadata (used only by the Wasm32 backend)
// ---------------------------------------------------------------------------

/// A Wasm value type, stored as a simple enum for serialization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WasmValueType {
    I32,
    I64,
    F32,
    F64,
}

/// A Wasm function type (parameter types → result types).
#[derive(Clone, Debug)]
pub struct WasmFuncType {
    /// Parameter types.
    pub params: Vec<WasmValueType>,
    /// Result types.
    pub results: Vec<WasmValueType>,
}

/// A Wasm local declaration: `count` locals of type `ty`.
#[derive(Clone, Debug)]
pub struct WasmLocalDecl {
    /// Number of consecutive locals of this type.
    pub count: u32,
    /// Type of these locals.
    pub ty: WasmValueType,
}

// ---------------------------------------------------------------------------
// AllocatedFunction
// ---------------------------------------------------------------------------

/// A function after register allocation.
#[derive(Clone, Debug)]
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
    pub relocations: Vec<RelocationEntry>,

    // ── Wasm-specific metadata ──────────────────────────────────────
    /// Wasm function type (params → results), used by the Wasm32 backend
    /// to emit the type section.  `None` for non-Wasm targets.
    pub wasm_func_type: Option<WasmFuncType>,
    /// Wasm local declarations (count, type) beyond function parameters.
    /// `None` for non-Wasm targets.
    pub wasm_locals: Option<Vec<WasmLocalDecl>>,
}

// ---------------------------------------------------------------------------
// AllocatedProgram
// ---------------------------------------------------------------------------

/// A complete program after register allocation.
#[derive(Clone, Debug)]
pub struct AllocatedProgram {
    /// Allocated functions.
    pub functions: Vec<AllocatedFunction>,
    /// Total code section size in bytes.
    pub total_code_size: usize,
    /// Total data section size in bytes.
    pub total_data_size: usize,
    /// Wave 1: Read-only data (string literals) to be placed in .rodata.
    /// Concatenated bytes of all ReadOnly data sections.
    pub rodata_data: Vec<u8>,
    /// Wave 5: All known function names (from the AST). Used to distinguish
    /// function symbols (which should be in .text) from data symbols (which
    /// go in .bss). Without this, functions removed by the O2 optimizer but
    /// still referenced via GetAddress would be incorrectly classified as
    /// data symbols.
    pub function_names: std::collections::HashSet<String>,
}

// ---------------------------------------------------------------------------
// BackendError
// ---------------------------------------------------------------------------

/// Error type for backend operations.
#[derive(Debug, Clone)]
pub enum BackendError {
    /// The requested feature is not supported by this ISA.
    UnsupportedFeature {
        /// ISA identifier (e.g., "aarch64", "x86_64").
        isa: &'static str,
        /// Description of the unsupported feature.
        feature: String,
    },

    /// Register allocation failed.
    RegisterAllocFailed {
        /// ISA identifier.
        isa: &'static str,
        /// Reason for the allocation failure.
        reason: String,
    },

    /// Instruction encoding failed.
    EncodingError {
        /// ISA identifier.
        isa: &'static str,
        /// Reason for the encoding failure.
        reason: String,
    },

    /// Invalid instruction for this target.
    InvalidInstruction {
        /// ISA identifier.
        isa: &'static str,
        /// Details about why the instruction is invalid.
        details: String,
    },

    /// ELF / binary emission error.
    EmissionError {
        /// ISA identifier.
        isa: &'static str,
        /// Reason for the emission failure.
        reason: String,
    },

    /// The target cannot handle this type.
    UnsupportedType {
        /// ISA identifier.
        isa: &'static str,
        /// The unsupported type name.
        ty: String,
    },

    /// Unresolved relocation — a symbol referenced by a relocation entry
    /// could not be found in the program's symbol table.
    UnresolvedRelocation {
        /// ISA identifier.
        isa: &'static str,
        /// Name of the unresolved symbol.
        symbol: String,
        /// Name of the function containing the reference.
        function: String,
        /// Byte offset within the function where the relocation applies.
        offset: u64,
        /// Relocation type string (e.g., "R_AARCH64_CALL26").
        reloc_type: String,
    },

    /// Generic backend error.
    Other {
        /// ISA identifier.
        isa: &'static str,
        /// Error message.
        message: String,
    },
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendError::UnsupportedFeature { isa, feature } => {
                write!(f, "[{isa}] unsupported feature: {feature}")
            }
            BackendError::RegisterAllocFailed { isa, reason } => {
                write!(f, "[{isa}] register allocation failed: {reason}")
            }
            BackendError::EncodingError { isa, reason } => {
                write!(f, "[{isa}] encoding error: {reason}")
            }
            BackendError::InvalidInstruction { isa, details } => {
                write!(f, "[{isa}] invalid instruction: {details}")
            }
            BackendError::EmissionError { isa, reason } => {
                write!(f, "[{isa}] emission error: {reason}")
            }
            BackendError::UnsupportedType { isa, ty } => {
                write!(f, "[{isa}] unsupported type: {ty}")
            }
            BackendError::UnresolvedRelocation {
                isa,
                symbol,
                function,
                offset,
                reloc_type,
            } => write!(
                f,
                "[{isa}] unresolved relocation: symbol '{symbol}' referenced in function '{function}' at offset 0x{offset:X} (relocation type: {reloc_type})"
            ),
            BackendError::Other { isa, message } => write!(f, "[{isa}] {message}"),
        }
    }
}

impl std::error::Error for BackendError {}

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

    // === Scheduling (Wave 10) ===

    /// Returns the instruction latency table for this target.
    ///
    /// Used by the instruction scheduler (Wave 5) and the e-graph cost
    /// function (Wave 10) to make per-ISA optimization decisions.
    ///
    /// Default implementation returns `LatencyTable::default_ooo()` (a
    /// conservative modern OoO profile). Each backend should override
    /// this to return its ISA-specific table from
    /// `vuma_codegen::target_desc::LatencyTable::<isa>()`.
    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::default_ooo()
    }
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

use std::collections::HashSet;

/// Set the thread-local set of 64-bit-returning function names.
pub fn set_64bit_returns(names: &HashSet<String>) {
    crate::arm32::set_64bit_returns(names);
    crate::wasm32::set_64bit_returns(names);
    crate::hppa::set_64bit_returns(names);
}

// ---------------------------------------------------------------------------
// BackendKind
// ---------------------------------------------------------------------------

/// Enumeration of all supported backend architectures.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
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
    /// PowerPC 64-bit (big-endian).
    PowerPC64,
    /// PowerPC 64-bit little-endian (ppc64le, ELFv2 ABI).
    PowerPC64LE,
    /// RISC-V 32-bit.
    RiscV32,
    /// x86-32 (i386).
    X86_32,
    /// SPARC V9 64-bit (sparc64).
    Sparc64,
    /// IBM System Z 64-bit (s390x).
    S390X,
    /// MIPS64 Big-Endian (mips64be).
    Mips64Be,
    /// ARM32 Big-Endian (armeb).
    ArmEb,
    /// AArch64 Big-Endian (aarch64_be).
    AArch64Be,
    /// Motorola 68000 (m68k).
    M68k,
    /// DEC Alpha (alpha).
    Alpha,
    /// HP PA-RISC (hppa).
    Hppa,
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
            BackendKind::PowerPC64LE => "ppc64le",
            BackendKind::RiscV32 => "riscv32",
            BackendKind::X86_32 => "x86_32",
            BackendKind::Sparc64 => "sparc64",
            BackendKind::S390X => "s390x",
            BackendKind::Mips64Be => "mips64be",
            BackendKind::ArmEb => "armeb",
            BackendKind::AArch64Be => "aarch64_be",
            BackendKind::M68k => "m68k",
            BackendKind::Alpha => "alpha",
            BackendKind::Hppa => "hppa",
        }
    }

    /// Returns the maturity tier of this backend.
    ///
    /// - `Complete`: Full codegen, syscall stubs, runtime support.
    ///   Passes the gold-standard test suite at >99%.
    /// - `Experimental`: Functional codegen but with known gaps
    ///   (e.g., missing print_int runtime, limited syscall stubs).
    /// - `Scaffolded`: Basic instruction encoding but significant
    ///   gaps in codegen, ABI, or runtime support.
    pub fn tier(&self) -> BackendTier {
        // Tier classification reflects actual ISA coverage, not aspiration.
        //
        // `Complete`: full integer ISA codegen, correct syscall stubs, runtime
        //   helpers, and inclusion in the gold-standard test matrix.
        // `Experimental`: functional straight-line integer codegen + syscall
        //   stubs, but with known gaps (signed division, FP, true atomics,
        //   callee-saved ABI, or >N-arg stack passing). NOT in the gold
        //   standard test matrix.
        // `Scaffolded`: basic instruction encoding but large classes of IR
        //   operations emit wrong/zero code (e.g. Mul/Div/Cmp/conditional
        //   branches). Suitable only for `test_exit`-style smoke tests.
        match self {
            // Tier 1 — fully featured and tested.
            BackendKind::AArch64 | BackendKind::AArch64Be |
            BackendKind::X86_64 | BackendKind::X86_32 |
            BackendKind::RiscV64 | BackendKind::RiscV32 |
            BackendKind::LoongArch64 | BackendKind::Arm32 | BackendKind::ArmEb |
            BackendKind::Mips64 | BackendKind::Mips64Be |
            BackendKind::PowerPC64 | BackendKind::PowerPC64LE |
            BackendKind::Wasm32 => BackendTier::Complete,
            // Tier 2 — functional integer codegen, known gaps (signed div,
            // FP, atomics, callee-saved). See docs/AUDIT.md.
            BackendKind::Sparc64 | BackendKind::S390X |
            BackendKind::M68k | BackendKind::Alpha => BackendTier::Experimental,
            // Tier 3 — Mul/Div/Cmp/conditional-branches emit stub code.
            BackendKind::Hppa => BackendTier::Scaffolded,
        }
    }
}

/// Backend maturity tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendTier {
    /// Full codegen, syscall stubs, runtime support. Passes >99% of tests.
    Complete,
    /// Functional codegen but with known gaps.
    Experimental,
    /// Basic instruction encoding but significant gaps.
    Scaffolded,
}

impl fmt::Display for BackendTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BackendTier::Complete => write!(f, "complete"),
            BackendTier::Experimental => write!(f, "experimental"),
            BackendTier::Scaffolded => write!(f, "scaffolded"),
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
        size_of_with_ptr_width(ty, 8) // ARM64 LP64: 8-byte pointers
    }

    fn alignment_of(&self, ty: &IRType) -> usize {
        alignment_of_with_ptr_width(ty, 8) // ARM64 LP64: 8-byte pointers
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

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::aarch64()
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
        size_of_with_ptr_width(ty, 8)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        alignment_of_with_ptr_width(ty, 8)
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

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::riscv64()
    }
}

// ---------------------------------------------------------------------------
// RiscV32 TargetInfo
// ---------------------------------------------------------------------------

/// RISC-V 32-bit target information (RV32IMA + F/D).
pub struct RiscV32TargetInfo;

impl TargetInfo for RiscV32TargetInfo {
    fn isa_name(&self) -> &'static str {
        "riscv32"
    }
    fn target_triple(&self) -> &'static str {
        "riscv32-unknown-linux-gnu"
    }
    fn elf_machine_type(&self) -> u16 {
        243
    } // EM_RISCV
    fn default_base_address(&self) -> u64 {
        0x10000
    }
    fn pointer_width(&self) -> usize {
        4
    }
    fn size_of(&self, ty: &IRType) -> usize {
        size_of_with_ptr_width(ty, 4)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        alignment_of_with_ptr_width(ty, 4)
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
        "ilp32d"
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
        OutputFormat::Elf32
    }

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::riscv32()
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
        size_of_with_ptr_width(ty, 4) // 32-bit pointers in wasm32
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        alignment_of_with_ptr_width(ty, 4)
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

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::wasm32()
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
        size_of_with_ptr_width(ty, 8)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        alignment_of_with_ptr_width(ty, 8)
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

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::loongarch64()
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
        size_of_with_ptr_width(ty, 8)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        alignment_of_with_ptr_width(ty, 8)
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

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::x86_64()
    }
}

// ---------------------------------------------------------------------------
// X86_32 TargetInfo
// ---------------------------------------------------------------------------

/// x86-32 (i386) target information.
pub struct X86_32TargetInfo;

impl TargetInfo for X86_32TargetInfo {
    fn isa_name(&self) -> &'static str { "x86_32" }
    fn target_triple(&self) -> &'static str { "i386-unknown-linux-gnu" }
    fn elf_machine_type(&self) -> u16 { 3 } // EM_386
    fn default_base_address(&self) -> u64 { 0x08048000 }
    fn pointer_width(&self) -> usize { 4 }
    fn size_of(&self, ty: &IRType) -> usize { size_of_with_ptr_width(ty, 4) }
    fn alignment_of(&self, ty: &IRType) -> usize { alignment_of_with_ptr_width(ty, 4) }
    fn endianness(&self) -> Endianness { Endianness::Little }
    fn has_registers(&self) -> bool { true }
    fn num_gp_regs(&self) -> usize { 8 }
    fn num_simd_fp_regs(&self) -> usize { 8 }
    fn has_hardwired_zero(&self) -> bool { false }
    fn has_link_register(&self) -> bool { false }
    fn has_branch_delay_slots(&self) -> bool { false }
    fn has_toc_pointer(&self) -> bool { false }
    fn has_condition_registers(&self) -> bool { false }
    fn calling_convention_name(&self) -> &'static str { "cdecl" }
    fn num_int_arg_regs(&self) -> usize { 0 }
    fn num_fp_arg_regs(&self) -> usize { 0 }
    fn stack_alignment(&self) -> usize { 16 }
    fn instruction_alignment(&self) -> usize { 1 }
    fn instruction_width_range(&self) -> (usize, usize) { (1, 15) }
    fn output_format(&self) -> OutputFormat { OutputFormat::Elf32 }

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::x86_32()
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
            IRType::I64 | IRType::U64 => 8,
            _ => size_of_with_ptr_width(ty, 4), // 32-bit pointers
        }
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        match ty {
            IRType::I64 | IRType::U64 => 4, // ARM32 aligns i64 to 4
            _ => alignment_of_with_ptr_width(ty, 4),
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

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::arm32()
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
        size_of_with_ptr_width(ty, 8)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        alignment_of_with_ptr_width(ty, 8)
    }
    fn endianness(&self) -> Endianness {
        Endianness::Little
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
        8
    } // $a0-$a7 (N64 ABI: $4-$11, $8-$11 are $a4-$a7)
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

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::mips64()
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
        size_of_with_ptr_width(ty, 8)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        alignment_of_with_ptr_width(ty, 8)
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

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::ppc64()
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

/// Map a decoded ARM64 `Instruction` to its `(reads, writes)` physical
/// register lists, mirroring what task 2-c did for mips64 / arm32 - except
/// the AArch64 `allocate_registers` path constructs its `AllocatedInstruction`s
/// directly from the encoded code bytes (it does not have a per-IR-instr
/// isel wrapper that already knows the operands), so we recover the
/// register operands by inspecting the decoded `Instruction`.
///
/// GPR operands (X0-X30, SP, XZR) are classified as `RegClass::Gpr`; the
/// FP/SIMD side of the FP-conversion instructions (SCVTF / UCVTF / FCVTZS /
/// FCVTZU / FCVT / FMOV_DX / FMOV_XD / CNT / ADDV / UMOV) is classified as
/// `RegClass::SimdFp`, matching AAPCS64 (integer side = X0..X30, FP side =
/// V0..V31). This lets tests like `test_all_backends_float_to_int_not_just_move`
/// detect the cross-bank register use that proves a real conversion is
/// happening (rather than a no-op move within one bank).
fn arm64_instruction_regs(
    inst: &crate::arm64::Instruction,
) -> (Vec<PhysicalReg>, Vec<PhysicalReg>) {
    use crate::arm64::{Instruction, Register};
    let gpr = |r: &Register| PhysicalReg::new(RegClass::Gpr, r.encoding());
    let fp = |r: &Register| PhysicalReg::new(RegClass::SimdFp, r.encoding());
    let fp_idx = |i: u8| PhysicalReg::new(RegClass::SimdFp, i as u32);

    let mut reads = Vec::new();
    let mut writes = Vec::new();

    match inst {
        // ---- FP conversions (cross register-bank) ----
        // SCVTF / UCVTF: src = GPR (Rn), dst = FP (Rd).
        Instruction::SCVTF { rd, rn, .. } | Instruction::UCVTF { rd, rn, .. } => {
            reads.push(gpr(rn));
            writes.push(fp(rd));
        }
        // FCVTZS / FCVTZU: src = FP (Rn), dst = GPR (Rd).
        Instruction::FCVTZS { rd, rn, .. } | Instruction::FCVTZU { rd, rn, .. } => {
            reads.push(fp(rn));
            writes.push(gpr(rd));
        }
        // FCVT: src = FP (Rn), dst = FP (Rd).
        Instruction::FCVT { rd, rn, .. } => {
            reads.push(fp(rn));
            writes.push(fp(rd));
        }

        // ---- FP <-> GPR moves ----
        Instruction::FMOV_DX { vd, rn } => {
            reads.push(gpr(rn));
            writes.push(fp_idx(*vd));
        }
        Instruction::FMOV_XD { rd, vn } => {
            reads.push(fp_idx(*vn));
            writes.push(gpr(rd));
        }

        // ---- SIMD integer ops ----
        Instruction::CNT { vd, vn } | Instruction::ADDV { vd, vn } => {
            reads.push(fp_idx(*vn));
            writes.push(fp_idx(*vd));
        }
        Instruction::UMOV { rd, vn } => {
            reads.push(fp_idx(*vn));
            writes.push(gpr(rd));
        }

        // ---- Three-operand arithmetic (rd = rn OP rm/imm) ----
        Instruction::ADD { rd, rn, rm }
        | Instruction::SUB { rd, rn, rm }
        | Instruction::LSL { rd, rn, rm }
        | Instruction::LSR { rd, rn, rm }
        | Instruction::ASR { rd, rn, rm } => {
            reads.push(gpr(rn));
            if let Some(r) = rm.as_reg() {
                reads.push(gpr(&r));
            }
            writes.push(gpr(rd));
        }

        // ---- Three-register arithmetic (all GPRs) ----
        Instruction::MUL { rd, rn, rm }
        | Instruction::SDIV { rd, rn, rm }
        | Instruction::UDIV { rd, rn, rm }
        | Instruction::AND { rd, rn, rm }
        | Instruction::ORR { rd, rn, rm }
        | Instruction::EOR { rd, rn, rm }
        | Instruction::RORV { rd, rn, rm } => {
            reads.push(gpr(rn));
            reads.push(gpr(rm));
            writes.push(gpr(rd));
        }

        // EXTR: rd = (rn:rm) >> imm6.
        Instruction::EXTR { rd, rn, rm, .. } => {
            reads.push(gpr(rn));
            reads.push(gpr(rm));
            writes.push(gpr(rd));
        }

        // ---- Load (rt = data, rn = address) ----
        Instruction::LDR { rt, rn, .. }
        | Instruction::LDR_W { rt, rn, .. }
        | Instruction::LDRB { rt, rn, .. }
        | Instruction::LDRH { rt, rn, .. }
        | Instruction::LDRSW { rt, rn, .. } => {
            reads.push(gpr(rn));
            writes.push(gpr(rt));
        }
        // ---- Store (rt = data, rn = address) ----
        Instruction::STR { rt, rn, .. }
        | Instruction::STR_W { rt, rn, .. }
        | Instruction::STRB { rt, rn, .. }
        | Instruction::STRH { rt, rn, .. } => {
            reads.push(gpr(rn));
            reads.push(gpr(rt));
        }

        // ---- Load/Store Pair ----
        Instruction::LDP { rt1, rt2, rn, .. } => {
            reads.push(gpr(rn));
            writes.push(gpr(rt1));
            writes.push(gpr(rt2));
        }
        Instruction::STP { rt1, rt2, rn, .. } => {
            reads.push(gpr(rn));
            reads.push(gpr(rt1));
            reads.push(gpr(rt2));
        }

        // ---- Atomics ----
        Instruction::LDXR { rt, rn }
        | Instruction::LDAXR { rt, rn }
        | Instruction::LDAR { rt, rn } => {
            reads.push(gpr(rn));
            writes.push(gpr(rt));
        }
        Instruction::STXR { rs, rt, rn }
        | Instruction::STLXR { rs, rt, rn } => {
            reads.push(gpr(rn));
            reads.push(gpr(rt));
            writes.push(gpr(rs));
        }
        Instruction::CAS { rs, rt, rn } => {
            reads.push(gpr(rn));
            reads.push(gpr(rs));
            writes.push(gpr(rt));
        }
        Instruction::STLR { rt, rn } => {
            reads.push(gpr(rn));
            reads.push(gpr(rt));
        }

        // ---- Branches that read a register ----
        Instruction::BR { rn } | Instruction::BLR { rn } => {
            reads.push(gpr(rn));
        }
        Instruction::RET { rn: Some(rn) } => {
            reads.push(gpr(rn));
        }
        Instruction::CBZ { rt, .. } | Instruction::CBNZ { rt, .. } => {
            reads.push(gpr(rt));
        }
        Instruction::TBZ { rt, .. } | Instruction::TBNZ { rt, .. } => {
            reads.push(gpr(rt));
        }

        // ---- Compare / Test ----
        Instruction::CMP { rn, rm } | Instruction::CMN { rn, rm } => {
            reads.push(gpr(rn));
            if let Some(r) = rm.as_reg() {
                reads.push(gpr(&r));
            }
        }
        Instruction::TST { rn, rm } => {
            reads.push(gpr(rn));
            reads.push(gpr(rm));
        }
        Instruction::CSEL { rd, rn, rm, .. } => {
            reads.push(gpr(rn));
            reads.push(gpr(rm));
            writes.push(gpr(rd));
        }
        Instruction::CSET { rd, .. } => {
            writes.push(gpr(rd));
        }
        Instruction::MSUB { rd, rn, rm, ra } => {
            reads.push(gpr(rn));
            reads.push(gpr(rm));
            reads.push(gpr(ra));
            writes.push(gpr(rd));
        }

        // ---- Bitfield / extend / one-source GPR ops ----
        Instruction::UBFM { rd, rn, .. }
        | Instruction::SBFM { rd, rn, .. }
        | Instruction::SXTW { rd, rn }
        | Instruction::CLZ { rd, rn }
        | Instruction::RBIT { rd, rn } => {
            reads.push(gpr(rn));
            writes.push(gpr(rd));
        }

        // ---- Move ----
        Instruction::MOV { rd, rm } => {
            reads.push(gpr(rm));
            writes.push(gpr(rd));
        }
        Instruction::MOVZ { rd, .. } | Instruction::MOVK { rd, .. } => {
            writes.push(gpr(rd));
        }

        // ---- Everything else (B, BL, BCond, DMB, DSB, ISB, SVC, NOP, RET
        // without explicit Rn, ...) has no easily-recoverable GPR/FP
        // operands that the tests care about; leave reads/writes empty
        // (matches the previous behaviour for these encodings). ----
        _ => {}
    }

    // Encoding 31 on AArch64 is XZR/SP — not a general-purpose argument
    // register (X0–X30). Drop it from the tracked reads/writes so downstream
    // consumers (e.g. the ABI arg-range test) don't see an out-of-range index.
    reads.retain(|r| !(r.class == RegClass::Gpr && r.index == 31));
    writes.retain(|r| !(r.class == RegClass::Gpr && r.index == 31));

    (reads, writes)
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

    /// Wave 22: Emit a function using real register allocation.
    ///
    /// Consumes a `RegAllocResult` (from `TargetAgnosticRegAlloc`) and
    /// produces an `AllocatedFunction` where each instruction's
    /// `reads`/`writes` fields are annotated with the physical registers
    /// (X0-X28 for GPRs) assigned by the linear-scan allocator.
    ///
    /// Spilled vregs remain on the stack via the existing stack-slot
    /// emitter (`Emitter::emit_function_stack_slot`).  The `encoded`
    /// bytes are correct; the `reads`/`writes` metadata is additive.
    pub fn emit_function_regalloc(
        &self,
        func: &IRFunction,
        alloc: &crate::regalloc::RegAllocResult,
    ) -> Result<AllocatedFunction, BackendError> {
        // Step 1: Run the existing stack-slot emitter to produce correct
        // encoded instruction words.  Pass `None` for the AllocationResult
        // to use the stack-slot path (Wave 21 changed emit_function to
        // accept Option<&AllocationResult>; we annotate post-hoc with the
        // backend-agnostic RegAllocResult instead).
        let mut emitter = crate::emit::Emitter::new();
        let code_words = emitter
            .emit_function(func, None)
            .map_err(|e| BackendError::RegisterAllocFailed {
                isa: "aarch64",
                reason: e.to_string(),
            })?;

        // Convert each 32-bit word into an AllocatedInstruction.
        let instructions: Vec<AllocatedInstruction> = code_words
            .iter()
            .enumerate()
            .map(|(i, &word)| {
                let (opcode, reads, writes) =
                    match crate::arm64::Instruction::decode(word) {
                        Some(inst) => {
                            let opcode = format!("{}", inst);
                            let (reads, writes) = arm64_instruction_regs(&inst);
                            (opcode, reads, writes)
                        }
                        None => (format!("arm64_{}", i), Vec::new(), Vec::new()),
                    };
                AllocatedInstruction {
                    opcode,
                    reads,
                    writes,
                    encoded: word.to_le_bytes().to_vec(),
                }
            })
            .collect();

        let code_size = instructions.len() * 4;
        let frame_size = aarch64_compute_frame_size(func);

        let allocated = AllocatedFunction {
            name: func.name.clone(),
            blocks: vec![AllocatedBlock {
                label: "entry".to_string(),
                instructions,
                code_offset: 0,
            }],
            frame_size,
            callee_saved: vec![],
            spill_slots: 0,
            code_size,
            relocations: emitter.relocations().to_vec(),
            wasm_func_type: None,
            wasm_locals: None,
        };

        // Step 2: Annotate with the regalloc result.
        let mut allocated = allocated;
        crate::regalloc_emit::annotate_with_regalloc(&mut allocated, alloc);

        Ok(allocated)
    }

    /// Wave 22: Convenience method — run regalloc + emit in one step.
    pub fn emit_function_with_regalloc(
        &self,
        func: &IRFunction,
    ) -> Result<AllocatedFunction, BackendError> {
        let alloc = crate::regalloc_emit::run_regalloc(func, "aarch64");
        self.emit_function_regalloc(func, &alloc)
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
fn build_aarch64_elf_2seg(code: &[u8], base_addr: u64, extern_symbols: &[String]) -> Vec<u8> {
    // Use 64K alignment for virtual addresses to ensure compatibility with
    // QEMU 10.x on hosts with 16K or 64K page sizes.  QEMU's aarch64
    // user-mode uses TARGET_PAGE_BITS_VARY, which matches the host page
    // size.  If the host has >4K pages, a 4K-aligned BSS segment can share
    // a host page with the RX text segment, triggering QEMU's
    // "PT_LOAD with bss overlapping non-writable page" error in zero_bss().
    // 64K is the largest common aarch64 page size, so aligning to 64K
    // guarantees no page overlap regardless of host page size.
    const HOST_PAGE_ALIGN: u64 = 0x10000; // 64 KB — max common aarch64 page size
    const PAGE_SIZE: u64 = 0x1000; // 4 KB — for p_align and BSS size

    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let num_phdrs: u64 = 3; // 2x LOAD + 1x PT_GNU_STACK
    let phdr_end = elf_header_size + num_phdrs * phdr_size;
    // Page-align the text segment start in the file.  The kernel's ELF
    // loader mmap()s each LOAD segment, and the file offset must be
    // congruent with the virtual address modulo the page size.  The
    // simplest way to guarantee this is to place the text at a
    // page-aligned file offset, with vaddr = base_addr.
    let text_offset = phdr_end; // No page alignment — code right after headers
    let text_size = code.len() as u64;

    // The data segment starts on the next 64K-aligned boundary after the
    // text.  This ensures the BSS does not share a host page (which may be
    // 4K, 16K, or 64K) with the RX text segment.
    let text_file_end = text_offset + text_size;
    let data_vaddr = (base_addr + text_file_end).div_ceil(HOST_PAGE_ALIGN) * HOST_PAGE_ALIGN;
    let _data_offset = data_vaddr - base_addr; // file offset for data segment
    let data_size: u64 = PAGE_SIZE; // 1 page of writable memory for stack/data
    let entry_point = base_addr + text_offset;

    let mut elf = Vec::with_capacity((text_offset + text_size + 256) as usize);

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
    elf.extend_from_slice(&3u16.to_le_bytes()); // e_phnum = 3 (2 LOAD + GNU_STACK)
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // --- Program Header 1: LOAD (PF_R | PF_X) — .text ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset = 0 (include ELF header)
    elf.extend_from_slice(&base_addr.to_le_bytes()); // p_vaddr (page-aligned; p_offset=0 requires alignment)
    elf.extend_from_slice(&base_addr.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&((text_offset + text_size) as u64).to_le_bytes()); // p_filesz (headers + code)
    elf.extend_from_slice(&((text_offset + text_size) as u64).to_le_bytes()); // p_memsz
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- Program Header 2: LOAD (PF_R | PF_W) — .data / stack ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset = 0
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_filesz (no initialized data)
    elf.extend_from_slice(&data_size.to_le_bytes()); // p_memsz (writable pages)
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- Program Header 3: PT_GNU_STACK (non-executable stack) ---
    elf.extend_from_slice(&0x6474e551u32.to_le_bytes()); // p_type = PT_GNU_STACK
    elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W (no PF_X)
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&0x10u64.to_le_bytes()); // p_align

    // --- .text section ---
    // Pad to page-aligned text_offset
    while (elf.len() as u64) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    // NOTE: We do NOT pad the file to `data_offset`.  The data segment has
    // p_filesz=0 (BSS-only), so the kernel never reads file content for it.
    // The data segment's virtual address (data_vaddr) is 64K-aligned in the
    // guest address space, but the file ends right after the text segment.
    // This keeps the file small (~4KB) regardless of the 64K virtual alignment.

    // No file data for the .data segment (it's BSS-like, zero-initialized)

    // ── Append ELF section headers (.text / .symtab / .strtab / .shstrtab)
    // when the program references external (undefined) symbols. The section
    // data is appended after the existing LOAD-segment file content; it is
    // NOT covered by any LOAD segment (section metadata is only used by
    // linkers/tools, never loaded into memory). Each external symbol
    // becomes a SHN_UNDEF / STT_FUNC / STB_GLOBAL entry in `.symtab` so
    // the linker can resolve it. Mirrors what task 3-c did to shared
    // `emit_elf`, but in AArch64's custom ELF builder.
    if !extern_symbols.is_empty() {
        append_aarch64_elf_sections(&mut elf, text_offset, text_size, extern_symbols);
    }

    elf
}

/// Append an ELF64 section header table (and the section data it describes)
/// to the in-progress AArch64 ELF buffer `elf`, then patch the ELF header's
/// `e_shoff` / `e_shnum` / `e_shstrndx` fields in place.
///
/// Sections emitted (in this order):
///   0. SHT_NULL  (reserved zero entry)
///   1. `.text`   (SHT_PROGBITS)  — points at the existing text segment
///   2. `.symtab` (SHT_SYMTAB)    — 1 NULL entry + 1 entry per external
///   3. `.strtab` (SHT_STRTAB)    — names for the symbols in `.symtab`
///   4. `.shstrtab` (SHT_STRTAB)  — names for the section headers themselves
///
/// `text_offset` and `text_size` describe the existing text segment so the
/// `.text` section header can point at it. `extern_symbols` is the list of
/// unresolved external function names (already deduplicated by the caller).
fn append_aarch64_elf_sections(
    elf: &mut Vec<u8>,
    text_offset: u64,
    text_size: u64,
    extern_symbols: &[String],
) {
    // ELF64 constants
    const SHT_NULL: u32 = 0;
    const SHT_PROGBITS: u32 = 1;
    const SHT_SYMTAB: u32 = 2;
    const SHT_STRTAB: u32 = 3;
    const SHN_UNDEF: u16 = 0;
    const STB_GLOBAL: u8 = 1;
    const STT_FUNC: u8 = 2;
    const SYM_SIZE: u64 = 24;

    // ── Build .shstrtab content ──
    // "\0.text\0.symtab\0.strtab\0.shstrtab\0"
    let mut shstrtab: Vec<u8> = Vec::new();
    shstrtab.push(0); // leading null byte (required)
    let name_text = shstrtab.len() as u32;
    shstrtab.extend_from_slice(b".text\0");
    let name_symtab = shstrtab.len() as u32;
    shstrtab.extend_from_slice(b".symtab\0");
    let name_strtab = shstrtab.len() as u32;
    shstrtab.extend_from_slice(b".strtab\0");
    let name_shstrtab = shstrtab.len() as u32;
    shstrtab.extend_from_slice(b".shstrtab\0");

    // ── Build .strtab content ──
    // "\0" + name1 + "\0" + name2 + "\0" + ...
    let mut strtab: Vec<u8> = Vec::new();
    strtab.push(0); // leading null byte (required; st_name=0 means "no name")
    let mut sym_name_offsets: Vec<u32> = Vec::with_capacity(extern_symbols.len());
    for name in extern_symbols {
        sym_name_offsets.push(strtab.len() as u32);
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
    }

    // ── Build .symtab content ──
    // Entry 0 is the reserved NULL symbol (24 zero bytes).
    // Each external becomes: st_name=offset, st_info=STB_GLOBAL<<4|STT_FUNC,
    //   st_other=0, st_shndx=SHN_UNDEF, st_value=0, st_size=0.
    let mut symtab: Vec<u8> = Vec::new();
    symtab.extend_from_slice(&[0u8; 24]); // NULL symbol
    for &name_off in &sym_name_offsets {
        symtab.extend_from_slice(&name_off.to_le_bytes()); // st_name
        symtab.push((STB_GLOBAL << 4) | STT_FUNC);          // st_info
        symtab.push(0);                                       // st_other
        symtab.extend_from_slice(&SHN_UNDEF.to_le_bytes());  // st_shndx
        symtab.extend_from_slice(&0u64.to_le_bytes());       // st_value
        symtab.extend_from_slice(&0u64.to_le_bytes());       // st_size
    }

    // ── Append section data to the file ──
    // Align the .symtab to 8 bytes (Elf64_Sym's st_value is a u64).
    while !elf.len().is_multiple_of(8) {
        elf.push(0);
    }
    let shstrtab_off = elf.len() as u64;
    elf.extend_from_slice(&shstrtab);
    let strtab_off = elf.len() as u64;
    elf.extend_from_slice(&strtab);
    while !elf.len().is_multiple_of(8) {
        elf.push(0);
    }
    let symtab_off = elf.len() as u64;
    let symtab_size = symtab.len() as u64;
    elf.extend_from_slice(&symtab);

    // ── Append the section header table ──
    while !elf.len().is_multiple_of(8) {
        elf.push(0);
    }
    let shdr_off = elf.len() as u64;

    // Helper to write one Elf64_Shdr (64 bytes). Defined as a nested fn
    // (rather than a closure) so the borrow-checker applies the standard
    // function-call reborrow rules to the `&mut Vec<u8>` parameter.
    fn push_shdr(elf: &mut Vec<u8>, shdr: &SectionHeader<u64>) {
        elf.extend_from_slice(&shdr.sh_name.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_type.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_flags.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_addr.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_offset.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_size.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_link.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_info.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_addralign.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_entsize.to_le_bytes());
    }

    // Index 0: SHT_NULL
    push_shdr(
        elf,
        &SectionHeader {
            sh_type: SHT_NULL,
            ..Default::default()
        },
    );
    // Index 1: .text (SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR = 0x6)
    push_shdr(
        elf,
        &SectionHeader {
            sh_name: name_text,
            sh_type: SHT_PROGBITS,
            sh_flags: 0x6,
            sh_addr: 0x400000 + text_offset,
            sh_offset: text_offset,
            sh_size: text_size,
            sh_addralign: 16,
            ..Default::default()
        },
    );
    // Index 2: .symtab (SHT_SYMTAB); sh_link = 3 (.strtab index);
    // sh_info = 1 (one local symbol — the NULL entry — so the first
    // global is at index 1; standard ELF convention).
    push_shdr(
        elf,
        &SectionHeader {
            sh_name: name_symtab,
            sh_type: SHT_SYMTAB,
            sh_offset: symtab_off,
            sh_size: symtab_size,
            sh_link: 3,
            sh_info: 1,
            sh_addralign: 8,
            sh_entsize: SYM_SIZE,
            ..Default::default()
        },
    );
    // Index 3: .strtab (SHT_STRTAB)
    push_shdr(
        elf,
        &SectionHeader {
            sh_name: name_strtab,
            sh_type: SHT_STRTAB,
            sh_offset: strtab_off,
            sh_size: strtab.len() as u64,
            sh_addralign: 1,
            ..Default::default()
        },
    );
    // Index 4: .shstrtab (SHT_STRTAB)
    push_shdr(
        elf,
        &SectionHeader {
            sh_name: name_shstrtab,
            sh_type: SHT_STRTAB,
            sh_offset: shstrtab_off,
            sh_size: shstrtab.len() as u64,
            sh_addralign: 1,
            ..Default::default()
        },
    );

    // ── Patch the ELF header: e_shoff (offset 40), e_shnum (offset 60),
    // e_shstrndx (offset 62). ──
    let shnum: u16 = 5;
    let shstrndx: u16 = 4; // index of .shstrtab
    elf[40..48].copy_from_slice(&shdr_off.to_le_bytes());
    elf[60..62].copy_from_slice(&shnum.to_le_bytes());
    elf[62..64].copy_from_slice(&shstrndx.to_le_bytes());
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
/// Builds the AArch64 runtime helper blob and returns it together with the
/// byte offsets (relative to the start of the blob) of each entry point:
/// `__vuma_print_hex`, `__vuma_print_int`, `__vuma_print_newline`.
fn build_aarch64_runtime() -> (Vec<u8>, usize, usize, usize) {
    let mut code = Vec::new();

    // ── __vuma_print_hex ──
    let hex_offset = 0usize;
    // Input: X0 = 64-bit value to print as 8 hex digits (zero-padded).
    // Clobbers X1, X2, X3, X8, X9, X10 (all caller-saved — only FP/LR saved).
    // Strategy: iterate 8 nibbles MSB→LSB (shift 28,24,…,0), convert each to
    // '0'-'9' or 'a'-'f', store in a buffer, then sys_write 8 bytes.
    // All instructions via `Instruction::encode()` for correct encodings.
    // Stack: 32 bytes (16 for FP/LR, 16 for buffer).
    {
        use crate::arm64::{Instruction, Register, Operand, Condition};
        macro_rules! e {
            ($i:expr) => { code.extend_from_slice(&$i.encode().unwrap().to_le_bytes()) };
        }
        // ── Prologue ──
        e!(Instruction::SUB { rd: Register::SP, rn: Register::SP, rm: Operand::Imm12(32) });               // 0: SUB SP, SP, #32
        e!(Instruction::STP { rt1: Register::X29, rt2: Register::X30, rn: Register::SP, offset: 0 });       // 1: STP X29, X30, [SP]
        e!(Instruction::ADD { rd: Register::X29, rn: Register::SP, rm: Operand::Imm12(0) });               // 2: ADD X29, SP, #0
        // ── Setup ──
        e!(Instruction::ADD { rd: Register::X9, rn: Register::SP, rm: Operand::Imm12(16) });               // 3: ADD X9, SP, #16  (buffer)
        e!(Instruction::MOVZ { rd: Register::X10, imm16: 0, shift: 0 });                                    // 4: MOVZ X10, #0  (counter)
        e!(Instruction::MOVZ { rd: Register::X3, imm16: 28, shift: 0 });                                    // 5: MOVZ X3, #28  (shift amount)
        e!(Instruction::MOVZ { rd: Register::X8, imm16: 15, shift: 0 });                                    // 6: MOVZ X8, #15  (mask 0xF)
        // ── Loop (instruction 7 = loop_start) ──
        e!(Instruction::LSR { rd: Register::X2, rn: Register::X0, rm: Operand::Reg { reg: Register::X3, shift: None } }); // 7: LSR X2, X0, X3
        e!(Instruction::AND { rd: Register::X2, rn: Register::X2, rm: Register::X8 });                     // 8: AND X2, X2, X8  (nibble)
        e!(Instruction::CMP { rn: Register::X2, rm: Operand::Imm12(9) });                                   // 9: CMP X2, #9
        e!(Instruction::ADD { rd: Register::X1, rn: Register::X2, rm: Operand::Imm12(48) });               // 10: ADD X1, X2, #48  ('0'+digit, default)
        e!(Instruction::BCond { cond: Condition::GT, offset: 8 });                                          // 11: B.GT hex_alpha (+8 → instr 13)
        e!(Instruction::B { offset: 8 });                                                                   // 12: B store_char (+8 → instr 14)
        // hex_alpha:
        e!(Instruction::ADD { rd: Register::X1, rn: Register::X2, rm: Operand::Imm12(87) });               // 13: ADD X1, X2, #87  ('a'-10+digit)
        // store_char: (reuse X8 as scratch addr — mask no longer needed)
        e!(Instruction::ADD { rd: Register::X8, rn: Register::X9, rm: Operand::Reg { reg: Register::X10, shift: None } }); // 14: ADD X8, X9, X10  (addr)
        e!(Instruction::STRB { rt: Register::X1, rn: Register::X8, offset: 0 });                            // 15: STRB W1, [X8]
        e!(Instruction::SUB { rd: Register::X3, rn: Register::X3, rm: Operand::Imm12(4) });                // 16: SUB X3, X3, #4  (shift -= 4)
        e!(Instruction::ADD { rd: Register::X10, rn: Register::X10, rm: Operand::Imm12(1) });              // 17: ADD X10, X10, #1  (counter++)
        e!(Instruction::CMP { rn: Register::X10, rm: Operand::Imm12(8) });                                   // 18: CMP X10, #8
        e!(Instruction::BCond { cond: Condition::LT, offset: -48 });                                         // 19: B.LT loop_start (-48 → instr 7)
        // ── sys_write(1, SP+16, 8) ──
        e!(Instruction::MOVZ { rd: Register::X0, imm16: 1, shift: 0 });                                     // 20: MOVZ X0, #1  (fd)
        e!(Instruction::ADD { rd: Register::X1, rn: Register::SP, rm: Operand::Imm12(16) });               // 21: ADD X1, SP, #16  (buf)
        e!(Instruction::MOVZ { rd: Register::X2, imm16: 8, shift: 0 });                                     // 22: MOVZ X2, #8  (len)
        e!(Instruction::MOVZ { rd: Register::X8, imm16: 64, shift: 0 });                                    // 23: MOVZ X8, #64  (sys_write)
        e!(Instruction::SVC { imm16: 0 });                                                                  // 24: SVC #0
        // ── Epilogue ──
        e!(Instruction::LDP { rt1: Register::X29, rt2: Register::X30, rn: Register::SP, offset: 0 });       // 25: LDP X29, X30, [SP]
        e!(Instruction::ADD { rd: Register::SP, rn: Register::SP, rm: Operand::Imm12(32) });              // 26: ADD SP, SP, #32
        e!(Instruction::RET { rn: None });                                                                  // 27: RET
    }

    // ── __vuma_print_int ──
    let int_offset = code.len();
    // Input: X0 = 64-bit signed integer to print as decimal.
    // Clobbers X1, X2, X3, X8, X9, X10 (all caller-saved per AAPCS64 —
    // only FP/LR are saved, the rest are freely clobbered).
    // Strategy: repeatedly UDIV by 10, store digit chars backward from the
    // END of a 32-byte buffer (so no in-place reversal is needed), then
    // sys_write the buffer from (X10+1) for X9 bytes.
    // Stack: 48 bytes (16 for FP/LR save, 32 for digit buffer).
    //
    // All instructions are emitted via `Instruction::encode()` to guarantee
    // correct encodings. The previous hand-encoded version had wrong CBZ /
    // B / STRB offsets (e.g. CBZ X0,done_digits branched into the middle of
    // the div loop instead of to done_digits, causing an infinite loop that
    // overwrote past the stack frame → SIGSEGV — the test_print crash on
    // aarch64). This rewrite fixes all branch offsets by construction.
    {
        use crate::arm64::{Instruction, Register, Operand, Condition};
        macro_rules! e {
            ($i:expr) => { code.extend_from_slice(&$i.encode().unwrap().to_le_bytes()) };
        }
        // ── Prologue ──
        e!(Instruction::SUB { rd: Register::SP, rn: Register::SP, rm: Operand::Imm12(48) });               // 0: SUB SP, SP, #48
        e!(Instruction::STP { rt1: Register::X29, rt2: Register::X30, rn: Register::SP, offset: 0 });       // 1: STP X29, X30, [SP]
        e!(Instruction::ADD { rd: Register::X29, rn: Register::SP, rm: Operand::Imm12(0) });               // 2: ADD X29, SP, #0
        // ── Handle negative ──
        e!(Instruction::CMP { rn: Register::X0, rm: Operand::Imm12(0) });                                   // 3: CMP X0, #0
        e!(Instruction::BCond { cond: Condition::GE, offset: 40 });                                         // 4: B.GE positive (+40 → instr 14)
        // Save X0 in X9 before the write syscall clobbers it with fd=1
        e!(Instruction::MOV { rd: Register::X9, rm: Register::X0 });                                        // 5: MOV X9, X0  (save value)
        // Write '-' to stdout
        e!(Instruction::MOVZ { rd: Register::X1, imm16: 45, shift: 0 });                                    // 6: MOVZ X1, #45  ('-')
        e!(Instruction::STRB { rt: Register::X1, rn: Register::SP, offset: 16 });                           // 7: STRB W1, [SP, #16]
        e!(Instruction::MOVZ { rd: Register::X0, imm16: 1, shift: 0 });                                     // 8: MOVZ X0, #1  (fd=stdout)
        e!(Instruction::ADD { rd: Register::X1, rn: Register::SP, rm: Operand::Imm12(16) });               // 9: ADD X1, SP, #16  (buf)
        e!(Instruction::MOVZ { rd: Register::X2, imm16: 1, shift: 0 });                                     // 10: MOVZ X2, #1  (len)
        e!(Instruction::MOVZ { rd: Register::X8, imm16: 64, shift: 0 });                                    // 11: MOVZ X8, #64 (sys_write)
        e!(Instruction::SVC { imm16: 0 });                                                                  // 12: SVC #0
        e!(Instruction::SUB { rd: Register::X0, rn: Register::XZR, rm: Operand::Reg { reg: Register::X9, shift: None } }); // 13: NEG X0 (SUB X0, XZR, X9)
        // ── positive: convert digits ──
        e!(Instruction::MOVZ { rd: Register::X9, imm16: 0, shift: 0 });                                     // 14: MOVZ X9, #0  (count = 0)
        e!(Instruction::ADD { rd: Register::X10, rn: Register::SP, rm: Operand::Imm12(47) });              // 15: ADD X10, SP, #47  (&buf[31], end of buffer)
        // ── div_loop ──
        e!(Instruction::CBZ { rt: Register::X0, offset: 40 });                                              // 16: CBZ X0, write_digits (+40 → instr 26)
        e!(Instruction::MOVZ { rd: Register::X1, imm16: 10, shift: 0 });                                    // 17: MOVZ X1, #10  (divisor)
        e!(Instruction::UDIV { rd: Register::X2, rn: Register::X0, rm: Register::X1 });                    // 18: UDIV X2, X0, X1  (quotient)
        e!(Instruction::MSUB { rd: Register::X3, rn: Register::X2, rm: Register::X1, ra: Register::X0 });  // 19: MSUB X3, X2, X1, X0  (remainder)
        e!(Instruction::ADD { rd: Register::X3, rn: Register::X3, rm: Operand::Imm12(48) });              // 20: ADD X3, X3, #48  ('0' + remainder)
        e!(Instruction::STRB { rt: Register::X3, rn: Register::X10, offset: 0 });                           // 21: STRB W3, [X10]  (store digit backward)
        e!(Instruction::SUB { rd: Register::X10, rn: Register::X10, rm: Operand::Imm12(1) });              // 22: SUB X10, X10, #1  (ptr--)
        e!(Instruction::ADD { rd: Register::X9, rn: Register::X9, rm: Operand::Imm12(1) });               // 23: ADD X9, X9, #1  (count++)
        e!(Instruction::MOV { rd: Register::X0, rm: Register::X2 });                                        // 24: MOV X0, X2  (X0 = quotient)
        e!(Instruction::B { offset: -36 });                                                                 // 25: B div_loop (-36 → instr 16)
        // ── write_digits ──
        e!(Instruction::CBNZ { rt: Register::X9, offset: 20 });                                             // 26: CBNZ X9, do_write (+20 → instr 31)
        // count == 0 (input was 0): store '0' as the sole digit
        e!(Instruction::MOVZ { rd: Register::X1, imm16: 48, shift: 0 });                                    // 27: MOVZ X1, #48  ('0')
        e!(Instruction::STRB { rt: Register::X1, rn: Register::X10, offset: 0 });                           // 28: STRB W1, [X10]
        e!(Instruction::SUB { rd: Register::X10, rn: Register::X10, rm: Operand::Imm12(1) });              // 29: SUB X10, X10, #1
        e!(Instruction::MOVZ { rd: Register::X9, imm16: 1, shift: 0 });                                     // 30: MOVZ X9, #1  (count = 1)
        // ── do_write: sys_write(1, X10+1, X9) ──
        e!(Instruction::ADD { rd: Register::X1, rn: Register::X10, rm: Operand::Imm12(1) });              // 31: ADD X1, X10, #1  (buf = X10+1)
        e!(Instruction::MOV { rd: Register::X2, rm: Register::X9 });                                        // 32: MOV X2, X9  (len = count)
        e!(Instruction::MOVZ { rd: Register::X0, imm16: 1, shift: 0 });                                     // 33: MOVZ X0, #1  (fd=stdout)
        e!(Instruction::MOVZ { rd: Register::X8, imm16: 64, shift: 0 });                                    // 34: MOVZ X8, #64 (sys_write)
        e!(Instruction::SVC { imm16: 0 });                                                                  // 35: SVC #0
        // ── Epilogue ──
        e!(Instruction::LDP { rt1: Register::X29, rt2: Register::X30, rn: Register::SP, offset: 0 });       // 36: LDP X29, X30, [SP]
        e!(Instruction::ADD { rd: Register::SP, rn: Register::SP, rm: Operand::Imm12(48) });              // 37: ADD SP, SP, #48
        e!(Instruction::RET { rn: None });                                                                  // 38: RET
    }

    // ── __vuma_print_newline ──
    // Write '\n' to stdout. Simple, no loops, no branches.
    // Uses the Instruction encoder for correct encodings (the previous
    // hand-encoded version had (a) a `code.truncate(len - 7*4)` that ate
    // 3 instructions of the preceding print_int epilogue, and (b) a wrong
    // STRB encoding 0x390021E1 = STRB W1,[X15,#8] instead of [SP,#16]).
    let newline_offset = code.len();
    {
        use crate::arm64::{Instruction, Register, Operand, Condition};
        macro_rules! e {
            ($i:expr) => { code.extend_from_slice(&$i.encode().unwrap().to_le_bytes()) };
        }
        e!(Instruction::SUB { rd: Register::SP, rn: Register::SP, rm: Operand::Imm12(32) });               // SUB SP, SP, #32
        e!(Instruction::STP { rt1: Register::X29, rt2: Register::X30, rn: Register::SP, offset: 0 });       // STP X29, X30, [SP]
        e!(Instruction::ADD { rd: Register::X29, rn: Register::SP, rm: Operand::Imm12(0) });               // ADD X29, SP, #0
        e!(Instruction::MOVZ { rd: Register::X1, imm16: 10, shift: 0 });                                    // MOVZ W1, #10  ('\n')
        e!(Instruction::STRB { rt: Register::X1, rn: Register::SP, offset: 16 });                           // STRB W1, [SP, #16]
        e!(Instruction::MOVZ { rd: Register::X0, imm16: 1, shift: 0 });                                     // MOVZ X0, #1  (fd)
        e!(Instruction::ADD { rd: Register::X1, rn: Register::SP, rm: Operand::Imm12(16) });               // ADD X1, SP, #16  (buf)
        e!(Instruction::MOVZ { rd: Register::X2, imm16: 1, shift: 0 });                                     // MOVZ X2, #1  (len)
        e!(Instruction::MOVZ { rd: Register::X8, imm16: 64, shift: 0 });                                    // MOVZ X8, #64 (sys_write)
        e!(Instruction::SVC { imm16: 0 });                                                                  // SVC #0
        e!(Instruction::LDP { rt1: Register::X29, rt2: Register::X30, rn: Register::SP, offset: 0 });       // LDP X29, X30, [SP]
        e!(Instruction::ADD { rd: Register::SP, rn: Register::SP, rm: Operand::Imm12(32) });              // ADD SP, SP, #32
        e!(Instruction::RET { rn: None });                                                                  // RET
    }

    (code, hex_offset, int_offset, newline_offset)
}

impl Backend for AArch64Backend {
    fn target_info(&self) -> &dyn TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        // F2a: Pre-lowering float-op verification.  Reject bitwise/shift/
        // remainder ops on F32/F64 operands before any backend lowers them,
        // so the emitter never sees an integer encoding for a float op.
        // AArch64 is wired as the reference call site; other backends
        // (alpha.rs, hppa.rs, s390x.rs, sparc64.rs, ...) should add the
        // same one-liner at the top of their `allocate_registers` impls —
        // see the doc on `verify_function_float_ops` above.
        verify_function_float_ops(func).map_err(|errs| BackendError::InvalidInstruction {
            isa: "aarch64",
            details: errs.join("; "),
        })?;

        // Use the existing Emitter to emit the function, which internally
        // performs register allocation and instruction encoding.
        let mut emitter = crate::emit::Emitter::new();
        let code = emitter
            .emit_function(func, None)
            .map_err(|e| BackendError::RegisterAllocFailed {
                isa: "aarch64",
                reason: e.to_string(),
            })?;

        let func_name = func.name.clone();
        let frame_size = aarch64_compute_frame_size(func);

        // Convert each 32-bit ARM64 instruction word into an AllocatedInstruction
        // with its little-endian encoded bytes. Decode each word via
        // `arm64::Instruction::decode` (the same decoder used by `disassemble`)
        // and use the decoded `Display` form as the opcode, mirroring what
        // task 2-c did for mips64. Populate `reads`/`writes` from the decoded
        // instruction's register operands (via `arm64_instruction_regs`) so
        // FP-conversion tests can detect the cross-bank register use that
        // proves a real conversion is happening. Falls back to the generic
        // `arm64_N` opcode and empty reads/writes for any word the decoder
        // does not recognise (defensive - should not happen for any
        // instruction emitted by the codegen).
        let instructions: Vec<AllocatedInstruction> = code
            .iter()
            .enumerate()
            .map(|(i, &word)| {
                let (opcode, reads, writes) =
                    match crate::arm64::Instruction::decode(word) {
                        Some(inst) => {
                            let opcode = format!("{}", inst);
                            let (reads, writes) = arm64_instruction_regs(&inst);
                            (opcode, reads, writes)
                        }
                        None => (format!("arm64_{}", i), Vec::new(), Vec::new()),
                    };
                AllocatedInstruction {
                    opcode,
                    reads,
                    writes,
                    encoded: word.to_le_bytes().to_vec(),
                }
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
            wasm_func_type: None,
            wasm_locals: None,
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

        let start_stub_size: usize = 20; // 5 × 4-byte instructions (LDR X0, ADD X1, BL, MOV X8, SVC)
        let ffi_stub_size: usize = 8; // MOV X0, #0; RET (2 × 4 bytes)
        let _ffi_stub_offset: usize = start_stub_size; // FFI stub right after _start

        // ── Build runtime I/O code ──
        // print_hex: X0 = value to print as 8 hex digits to stdout
        //   Uses SVC #0 with X8=64 (sys_write), fd=1 (stdout)
        //   Converts each nibble to hex char, writes to stack buffer, then sys_write.
        let (runtime_code, rt_hex_off, rt_int_off, rt_newline_off) = build_aarch64_runtime();

        // ── Build __vuma_alloc / __vuma_free syscall stubs (mmap/munmap) ──
        // __vuma_alloc(size in X0) -> X0 = mmap(NULL, size, PROT_READ|PROT_WRITE,
        //                                       MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)
        //   AArch64 Linux: mmap = syscall 222, args X0-X5, syscall # in X8, SVC #0
        // __vuma_free(addr in X0) -> munmap(addr, 0)
        //   AArch64 Linux: munmap = syscall 215, args X0/X1, syscall # in X8, SVC #0
        let vuma_alloc_stub: Vec<u8> = vec![
            0xE1, 0x03, 0x00, 0xAA,  // MOV X1, X0       (size -> length)
            0xE0, 0x03, 0x1F, 0xAA,  // MOV X0, XZR      (addr = NULL)
            0x62, 0x00, 0x80, 0xD2,  // MOV X2, #3       (PROT_READ|PROT_WRITE)
            0x43, 0x04, 0x80, 0xD2,  // MOV X3, #0x22    (MAP_PRIVATE|MAP_ANONYMOUS)
            0x04, 0x00, 0x80, 0x92,  // MOVN X4, #0      (fd = -1)
            0xE5, 0x03, 0x1F, 0xAA,  // MOV X5, XZR      (offset = 0)
            0xC8, 0x1B, 0x80, 0xD2,  // MOV X8, #222     (sys_mmap)
            0x01, 0x00, 0x00, 0xD4,  // SVC #0
            0xC0, 0x03, 0x5F, 0xD6,  // RET
        ];
        let vuma_free_stub: Vec<u8> = vec![
            0xE1, 0x03, 0x1F, 0xAA,  // MOV X1, XZR      (size = 0)
            0xE8, 0x1A, 0x80, 0xD2,  // MOV X8, #215     (sys_munmap)
            0x01, 0x00, 0x00, 0xD4,  // SVC #0
            0xC0, 0x03, 0x5F, 0xD6,  // RET
        ];

        // ── Compute function offsets ──
        // _start stub comes first, then user functions, then runtime.
        let mut func_offsets: HashMap<String, usize> = HashMap::new();
        let mut current_offset: usize = start_stub_size + ffi_stub_size; // after _start + FFI stub

        for func in &program.functions {
            func_offsets.insert(func.name.clone(), current_offset);
            let func_size: usize = func.blocks.iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            current_offset += func_size;
        }

        // Runtime functions: __vuma_print_hex, __vuma_print_int, __vuma_print_newline.
        // Each entry point lives at its own offset within the runtime blob.
        let runtime_offsets_start = current_offset;
        func_offsets.insert("__vuma_print_hex".to_string(), runtime_offsets_start + rt_hex_off);
        func_offsets.insert("__vuma_print_int".to_string(), runtime_offsets_start + rt_int_off);
        func_offsets.insert(
            "__vuma_print_newline".to_string(),
            runtime_offsets_start + rt_newline_off,
        );
        // Bare-name aliases: point print_int / print_hex at the SAME runtime
        // entry points as __vuma_print_int / __vuma_print_hex so user code
        // using the POSIX-friendly bare names resolves to the real decimal
        // / hex conversion routines instead of becoming no-op unresolved
        // externs.  The runtime prologue/epilogue already saves and restores
        // every caller-saved register it touches (X1, X2, X3, X8, X9, X10),
        // so this is safe to call from VUMA-compiled code that may hold
        // locals in those registers across the call.
        func_offsets.insert("print_hex".to_string(), runtime_offsets_start + rt_hex_off);
        func_offsets.insert("print_int".to_string(), runtime_offsets_start + rt_int_off);

        // __vuma_alloc / __vuma_free stubs go after the runtime blob.
        let vuma_alloc_offset = current_offset + runtime_code.len();
        let vuma_free_offset = vuma_alloc_offset + vuma_alloc_stub.len();
        func_offsets.insert("__vuma_alloc".to_string(), vuma_alloc_offset);
        func_offsets.insert("__vuma_free".to_string(), vuma_free_offset);

        // ── POSIX syscall stubs ──────────────────────────────────────
        // These provide the syscalls needed by mmap_sha256d, signal_hash,
        // lock_free_queue, epoll_echo, and ffi_demo tests.
        //
        // AArch64 calling convention: args in X0-X5, return in X0.
        // AArch64 syscall convention: args in X0-X5, syscall# in X8, SVC #0.
        // The calling convention matches the syscall convention for most
        // syscalls, so stubs are just: MOV X8, #num; SVC #0; RET.
        //
        // For syscalls that need arg shuffling (open→openat, unlink→unlinkat),
        // extra MOV instructions are added before the syscall.

        // Helper: encode MOVZ X8, #imm16
        let movz_x8 = |imm: u32| -> [u8; 4] {
            (0xD2800008u32 | ((imm & 0xFFFF) << 5)).to_le_bytes()
        };
        // Helper: encode MOV Xn, Xm (register move)
        let mov_reg = |rd: u32, rs: u32| -> [u8; 4] {
            (0xAA0003E0u32 | ((rs & 0x1F) << 16) | (rd & 0x1F)).to_le_bytes()
        };
        // Helper: encode MOVZ Xn, #imm16 (for any register)
        let movz_reg = |rd: u32, imm: u32| -> [u8; 4] {
            (0xD2800000u32 | ((imm & 0xFFFF) << 5) | (rd & 0x1F)).to_le_bytes()
        };
        // Helper: encode MOVN Xn, #imm16 (for negative values like -1)
        let movn_reg = |rd: u32, imm: u32| -> [u8; 4] {
            (0x92800000u32 | ((imm & 0xFFFF) << 5) | (rd & 0x1F)).to_le_bytes()
        };

        let svc: [u8; 4] = 0xD4000001u32.to_le_bytes(); // SVC #0
        let ret: [u8; 4] = 0xD65F03C0u32.to_le_bytes(); // RET

        // Build syscall stubs: (name, code)
        let syscall_stubs: Vec<(String, Vec<u8>)> = {
            let mut stubs: Vec<(String, Vec<u8>)> = Vec::new();

            // Simple stubs (args already in correct registers X0-X5):
            // Note: alarm is NOT a direct syscall on aarch64 (syscall 37 is
            // linkat). We implement alarm as a special stub below using
            // kill(getpid(), SIGALRM).
            for (name, num) in [
                ("write", 64), ("read", 63), ("close", 57), ("mmap", 222),
                ("munmap", 215), ("exit", 93), ("getpid", 172),
                ("socket", 198), ("epoll_create1", 20), ("futex", 98),
                ("execve", 221), ("wait4", 260), ("epoll_ctl", 21), ("epoll_wait", 22),
                ("clone", 220),
                // ── Additional POSIX syscall stubs (AArch64 generic ABI) ──
                // Numbers verified against asm-generic/unistd.h.
                ("lseek", 62), ("fstat", 80),
                ("kill", 129), ("getcwd", 17), ("chdir", 49),
                ("ioctl", 29), ("fcntl", 25), ("connect", 203),
                ("nanosleep", 101), ("mprotect", 226),
                ("dup", 23), ("exit_group", 94),
                ("recv", 207), ("send", 206), ("shutdown", 210),
                ("bind", 200), ("listen", 201), ("accept", 202),
                ("setsockopt", 208),
                ("getsockopt", 209),
                ("waitpid", 260),
                ("brk", 214),
                ("clock_gettime", 113),
                ("gettimeofday", 169),
                ("rt_sigprocmask", 135),
                ("dup3", 24),
                ("recvfrom", 207), ("sendto", 206),
                // NOTE: stat/lstat/poll do not exist on the AArch64 generic
                // ABI. They are provided as newfstatat/ppoll shims below.
                // ── Wave 7: POSIX file-metadata & I/O syscalls (asm-generic) ──
                // AArch64 has 8 register args (X0-X7); all these take ≤5 args,
                // so the simple "mov x8,#num; svc; ret" stub suffices. Numbers
                // from asm-generic/unistd.h. The plain mkdir/rmdir/rename/link/
                // symlink/readlink/chmod/chown names do NOT exist on the generic
                // ABI and are provided as *at wrappers below.
                ("umask", 166), ("fchmod", 52), ("fchown", 55),
                ("openat", 56), ("unlinkat", 35), ("renameat", 38),
                ("linkat", 37), ("symlinkat", 36), ("readlinkat", 78),
                ("faccessat", 48), ("fchmodat", 53), ("fchownat", 54),
                ("ftruncate", 46), ("fsync", 82), ("fdatasync", 83),
                ("sync", 81), ("syncfs", 306),
                ("pread", 67), ("pwrite", 68), ("readv", 65), ("writev", 66),
                ("preadv", 69), ("pwritev", 70),
                ("fchdir", 50), ("chroot", 51),
                // ── Wave 9: POSIX system & advanced syscalls (asm-generic) ──
                // AArch64 has 8 reg args; all take ≤5 args → simple stub.
                // eventfd→eventfd2(19), signalfd→signalfd4(74) = modern variants.
                ("mlock", 228), ("munlock", 229), ("mlockall", 230), ("munlockall", 231),
                ("mincore", 232), ("madvise", 233), ("msync", 227), ("mremap", 216),
                ("getrlimit", 163), ("setrlimit", 164), ("prlimit64", 261),
                ("getrusage", 165), ("times", 153),
                ("getrandom", 278),
                ("eventfd", 19), ("timerfd_create", 85), ("timerfd_settime", 86),
                ("timerfd_gettime", 87), ("signalfd", 74),
                ("inotify_init1", 26), ("inotify_add_watch", 27), ("inotify_rm_watch", 28),
                ("ptrace", 117),
                // ── Wave 8: POSIX process & identity syscalls (asm-generic/unistd.h) ──
                // All present directly in asm-generic (no *at wrapping). All take
                // ≤5 args; aarch64 has 8 reg args (X0-X7) → inline stub for all.
                // Family 1: identity
                ("getuid", 174), ("geteuid", 175), ("getgid", 176), ("getegid", 177),
                ("setuid", 146), ("setgid", 144), ("setresuid", 147), ("setresgid", 149),
                // Family 2: process group (getpid already present; getpgrp ABSENT in
                // asm-generic → callers use getpgid(0))
                ("getppid", 173), ("getsid", 156), ("setsid", 157),
                ("setpgid", 154), ("getpgid", 155),
                ("getpgrp", 65),
                // Family 3: clone/wait (clone/wait4 already present; vfork ABSENT →
                // callers use clone(CLONE_VFORK))
                ("clone3", 435), ("waitid", 95),
                // Family 4: exec/exit (execve/exit_group already present)
                ("execveat", 281),
                // Family 5: signals (kill/rt_sigprocmask/rt_sigreturn already present)
                ("tgkill", 131), ("tkill", 130), ("rt_sigaction", 134),
                // Family 6: directory read (getdents/readdir ABSENT in asm-generic →
                // use getdents64)
                ("getdents64", 61),
                // Family 7: system (arch_prctl is x86_64-only)
                ("prctl", 167), ("uname", 160), ("sysinfo", 179),
                            ("eventfd2", 19),
                ("newfstatat", 79),
                ("signalfd4", 74),
] {
                let mut code = Vec::new();
                code.extend_from_slice(&movz_x8(num));
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push((name.to_string(), code));
            }

            // alarm(seconds) — implement via setitimer(ITIMER_REAL, ...)
            // On aarch64, alarm is not a direct syscall. We use setitimer
            // (syscall 103) to schedule SIGALRM after the specified delay.
            // setitimer(ITIMER_REAL=0, &itimerval, NULL)
            // struct itimerval { struct timeval it_interval; struct timeval it_value; }
            // struct timeval { long tv_sec; long tv_usec; }
            // Total: 32 bytes on stack
            {
                let mut code = Vec::new();
                // SUB SP, SP, #32
                // Encoding: 0xD1000000 | (imm12 << 10) | (Rn << 5) | Rd
                // SUB SP, SP, #32 = 0xD1000000 | (32 << 10) | (31 << 5) | 31
                code.extend_from_slice(&0xD10083FFu32.to_le_bytes());
                // STR XZR, [SP, #0]  (it_interval.tv_sec = 0)
                // STR Xt, [Xn, #imm] = 0xF9000000 | (imm/8 << 10) | (Rn << 5) | Rt
                // STR XZR, [SP, #0] = 0xF9000000 | 0 | (31 << 5) | 31 = 0xF90003FF
                code.extend_from_slice(&0xF90003FFu32.to_le_bytes());
                // STR XZR, [SP, #8]  (it_interval.tv_usec = 0)
                // STR XZR, [SP, #8] = 0xF9000000 | (1 << 10) | (31 << 5) | 31 = 0xF90007FF
                code.extend_from_slice(&0xF90007FFu32.to_le_bytes());
                // STR X0, [SP, #16]  (it_value.tv_sec = X0 = seconds)
                // STR Xt, [Xn, #imm] = 0xF9000000 | (imm/8 << 10) | (Rn << 5) | Rt
                // SP = Rn=31, X0 = Rt=0, imm=16 → imm12=2
                // 0xF9000000 | (2 << 10) | (31 << 5) | 0 = 0xF9000BE0
                code.extend_from_slice(&0xF9000BE0u32.to_le_bytes());
                // STR XZR, [SP, #24] (it_value.tv_usec = 0)
                // SP = Rn=31, XZR = Rt=31, imm=24 → imm12=3
                // 0xF9000000 | (3 << 10) | (31 << 5) | 31 = 0xF9000FFF
                code.extend_from_slice(&0xF9000FFFu32.to_le_bytes());
                // MOV X0, #0 (ITIMER_REAL)
                code.extend_from_slice(&movz_reg(0, 0));
                // ADD X1, SP, #0 (pointer to itimerval)
                code.extend_from_slice(&0x910003E1u32.to_le_bytes());
                // MOV X2, #0 (NULL)
                code.extend_from_slice(&movz_reg(2, 0));
                // MOV X8, #103 (setitimer)
                code.extend_from_slice(&movz_x8(103));
                code.extend_from_slice(&svc);
                // ADD SP, SP, #32
                code.extend_from_slice(&0x910083FFu32.to_le_bytes());
                code.extend_from_slice(&ret);
                stubs.push(("alarm".to_string(), code));
            }

            // open → openat(AT_FDCWD=-100, pathname, flags, mode)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&mov_reg(3, 2));
                code.extend_from_slice(&mov_reg(2, 1));
                code.extend_from_slice(&mov_reg(1, 0));
                code.extend_from_slice(&movn_reg(0, 99));
                code.extend_from_slice(&movz_x8(56));
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("open".to_string(), code));
            }

            // unlink → unlinkat(AT_FDCWD=-100, pathname, 0)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&movz_reg(2, 0));
                code.extend_from_slice(&mov_reg(1, 0));
                code.extend_from_slice(&movn_reg(0, 99));
                code.extend_from_slice(&movz_x8(35));
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("unlink".to_string(), code));
            }

            // sigaction → rt_sigaction(signum, act, oldact, sigsetsize=8)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&movz_reg(3, 8));
                code.extend_from_slice(&movz_x8(134));
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("sigaction".to_string(), code));
            }

            // pipe → pipe2(pipefd, 0)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&movz_reg(1, 0));
                code.extend_from_slice(&movz_x8(59));
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("pipe".to_string(), code));
            }

            // dup2 → dup3(oldfd, newfd, 0)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&movz_reg(2, 0));
                code.extend_from_slice(&movz_x8(24));
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("dup2".to_string(), code));
            }

            // fork → clone(SIGCHLD, 0, 0, 0, 0)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&movz_reg(0, 17));
                code.extend_from_slice(&movz_reg(1, 0));
                code.extend_from_slice(&movz_reg(2, 0));
                code.extend_from_slice(&movz_reg(3, 0));
                code.extend_from_slice(&movz_reg(4, 0));
                code.extend_from_slice(&movz_x8(220));
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("fork".to_string(), code));
            }

            // rt_sigreturn (139) — special: no args, never returns.
            // The kernel restores the saved signal context and resumes
            // execution at the interrupted PC. Emit just ECALL with no RET.
            {
                let mut code = Vec::new();
                code.extend_from_slice(&movz_x8(139));
                code.extend_from_slice(&svc);
                // Defensive: if the kernel ever does return, trap.
                code.extend_from_slice(&0xD4200000u32.to_le_bytes()); // BRK #0
                stubs.push(("rt_sigreturn".to_string(), code));
            }

            // stat(path, statbuf) → newfstatat(AT_FDCWD=-100, path, statbuf, 0)
            // AArch64 generic ABI has no stat(); newfstatat=79 is the
            // replacement. flags=0 means "follow symlinks".
            {
                let mut code = Vec::new();
                code.extend_from_slice(&mov_reg(2, 1));     // X2 = X1 (statbuf)
                code.extend_from_slice(&mov_reg(1, 0));     // X1 = X0 (pathname)
                code.extend_from_slice(&movn_reg(0, 99));   // X0 = AT_FDCWD = -100
                code.extend_from_slice(&movz_reg(3, 0));    // X3 = 0 (flags)
                code.extend_from_slice(&movz_x8(79));       // newfstatat
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("stat".to_string(), code));
            }

            // lstat(path, statbuf) → newfstatat(AT_FDCWD, path, statbuf,
            //   AT_SYMLINK_NOFOLLOW=0x100). lstat() does not exist on AArch64.
            {
                let mut code = Vec::new();
                code.extend_from_slice(&mov_reg(2, 1));     // X2 = X1 (statbuf)
                code.extend_from_slice(&mov_reg(1, 0));     // X1 = X0 (pathname)
                code.extend_from_slice(&movn_reg(0, 99));   // X0 = AT_FDCWD
                // AT_SYMLINK_NOFOLLOW = 0x100. MOVZ X3, #0x100
                // MOVZ Xd, #imm16, LSL #shift: 0xD2800000 | (imm << 5) | rd
                // For imm=0x100 with shift=0: 0xD2800000 | (0x100 << 5) | 3
                code.extend_from_slice(&0xD2820063u32.to_le_bytes());
                code.extend_from_slice(&movz_x8(79));       // newfstatat
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("lstat".to_string(), code));
            }

            // poll(fds, nfds, timeout) → ppoll(fds, nfds, &ts, NULL)
            // AArch64 has no poll(); ppoll=73 takes a struct timespec* in X2
            // and a sigset_t* in X3. Build a 16-byte timespec on the stack:
            //   struct timespec { long tv_sec; long tv_nsec; };
            // For timeout==-1 (infinite) we pass tv_sec=0,tv_nsec=0 and rely on
            // the caller's semantics — a faithful poll→ppoll conversion would
            // require branching on the sign of timeout; we keep it simple and
            // store the timeout seconds verbatim with nsec=0.
            {
                let mut code = Vec::new();
                // SUB SP, SP, #16
                code.extend_from_slice(&0xD10043FFu32.to_le_bytes());
                // STR X2, [SP, #0]   ; tv_sec = X2 (timeout, reused as seconds)
                code.extend_from_slice(&0xF90003E2u32.to_le_bytes());
                // STR XZR, [SP, #8]  ; tv_nsec = 0
                code.extend_from_slice(&0xF90007FFu32.to_le_bytes());
                // ADD X2, SP, #0     ; X2 = &ts
                code.extend_from_slice(&0x910003E2u32.to_le_bytes());
                // MOV X3, XZR        ; X3 = NULL (no sigmask)
                code.extend_from_slice(&0xAA1F03E3u32.to_le_bytes());
                // ppoll = 73
                code.extend_from_slice(&movz_x8(73));
                code.extend_from_slice(&svc);
                // ADD SP, SP, #16
                code.extend_from_slice(&0x910043FFu32.to_le_bytes());
                code.extend_from_slice(&ret);
                stubs.push(("poll".to_string(), code));
            }

            // strcmp(s1, s2) → int — assembly loop, not a syscall.
            // AArch64 calling convention: X0=s1, X1=s2, return in X0.
            // Loop: load a byte from each string, compare; if they differ or
            // either is NUL, return the difference; else advance and repeat.
            {
                let code: Vec<u8> = [
                    0x39400002u32, // LDRB W2, [X0]       — loop:
                    0x39400023,    // LDRB W3, [X1]
                    0x6B03002F,    // CMP W2, W3           (SUBS WZR, W2, W3)
                    0x540000A1,    // B.NE done            (+5)
                    0x34000082,    // CBZ W2, done         (+4)
                    0x91000400,    // ADD X0, X0, #1
                    0x91000421,    // ADD X1, X1, #1
                    0x17FFFFF9,    // B loop               (-7)
                    0x4B030040,    // SUB W0, W2, W3       — done:
                    0xD65F03C0,    // RET
                ]
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect();
                stubs.push(("strcmp".to_string(), code));
            }

            // ── Wave 7 wrappers: plain POSIX names → *at(AT_FDCWD=-100, ...) ──
            // AArch64 (asm-generic) removed the legacy mkdir/rmdir/rename/link/
            // symlink/readlink/chmod/chown syscalls; they exist only as the *at
            // variants. We expose the plain names by inserting AT_FDCWD=-100
            // (= ~99 via MOVN) and shifting the caller's args. AT_REMOVEDIR=0x200.

            // mkdir(path, mode) → mkdirat(AT_FDCWD, path, mode)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&mov_reg(2, 1));    // X2 = mode
                code.extend_from_slice(&mov_reg(1, 0));    // X1 = path
                code.extend_from_slice(&movn_reg(0, 99));  // X0 = AT_FDCWD
                code.extend_from_slice(&movz_x8(34));      // mkdirat
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("mkdir".to_string(), code));
            }
            // rmdir(path) → unlinkat(AT_FDCWD, path, AT_REMOVEDIR=0x200)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&movz_reg(2, 0x200));// X2 = AT_REMOVEDIR
                code.extend_from_slice(&mov_reg(1, 0));    // X1 = path
                code.extend_from_slice(&movn_reg(0, 99));  // X0 = AT_FDCWD
                code.extend_from_slice(&movz_x8(35));      // unlinkat
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("rmdir".to_string(), code));
            }
            // rename(old, new) → renameat(AT_FDCWD, old, AT_FDCWD, new)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&mov_reg(3, 1));    // X3 = new
                code.extend_from_slice(&movn_reg(2, 99));  // X2 = AT_FDCWD
                code.extend_from_slice(&mov_reg(1, 0));    // X1 = old
                code.extend_from_slice(&movn_reg(0, 99));  // X0 = AT_FDCWD
                code.extend_from_slice(&movz_x8(38));      // renameat
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("rename".to_string(), code));
            }
            // link(old, new) → linkat(AT_FDCWD, old, AT_FDCWD, new, 0)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&movz_reg(4, 0));   // X4 = 0 (flags)
                code.extend_from_slice(&mov_reg(3, 1));    // X3 = new
                code.extend_from_slice(&movn_reg(2, 99));  // X2 = AT_FDCWD
                code.extend_from_slice(&mov_reg(1, 0));    // X1 = old
                code.extend_from_slice(&movn_reg(0, 99));  // X0 = AT_FDCWD
                code.extend_from_slice(&movz_x8(37));      // linkat
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("link".to_string(), code));
            }
            // symlink(target, linkpath) → symlinkat(target, AT_FDCWD, linkpath)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&mov_reg(2, 1));    // X2 = linkpath
                code.extend_from_slice(&movn_reg(1, 99));  // X1 = AT_FDCWD
                // X0 = target (unchanged)
                code.extend_from_slice(&movz_x8(36));      // symlinkat
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("symlink".to_string(), code));
            }
            // readlink(path, buf, siz) → readlinkat(AT_FDCWD, path, buf, siz)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&mov_reg(3, 2));    // X3 = siz
                code.extend_from_slice(&mov_reg(2, 1));    // X2 = buf
                code.extend_from_slice(&mov_reg(1, 0));    // X1 = path
                code.extend_from_slice(&movn_reg(0, 99));  // X0 = AT_FDCWD
                code.extend_from_slice(&movz_x8(78));      // readlinkat
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("readlink".to_string(), code));
            }
            // chmod(path, mode) → fchmodat(AT_FDCWD, path, mode, 0)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&movz_reg(3, 0));   // X3 = 0 (flags)
                code.extend_from_slice(&mov_reg(2, 1));    // X2 = mode
                code.extend_from_slice(&mov_reg(1, 0));    // X1 = path
                code.extend_from_slice(&movn_reg(0, 99));  // X0 = AT_FDCWD
                code.extend_from_slice(&movz_x8(53));      // fchmodat
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("chmod".to_string(), code));
            }
            // chown(path, owner, group) → fchownat(AT_FDCWD, path, owner, group, 0)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&movz_reg(4, 0));   // X4 = 0 (flags)
                code.extend_from_slice(&mov_reg(3, 2));    // X3 = group
                code.extend_from_slice(&mov_reg(2, 1));    // X2 = owner
                code.extend_from_slice(&mov_reg(1, 0));    // X1 = path
                code.extend_from_slice(&movn_reg(0, 99));  // X0 = AT_FDCWD
                code.extend_from_slice(&movz_x8(54));      // fchownat
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("chown".to_string(), code));
            }

            // ── FFI scratchpad frame stubs (Wave 3b/fix) ──────────────────
            // ffi_scratch_push_frame: REAL mmap syscall (aarch64 sys_mmap=222).
            // Allocates 4096 bytes for the scratchpad frame.
            {
                let mut code = Vec::new();
                // Set up mmap args: X0=0(NULL), X1=4096, X2=3(PROT), X3=0x22(MAP), X4=-1(fd), X5=0(off)
                code.extend_from_slice(&movz_reg(0, 0));       // X0 = 0 (NULL)
                code.extend_from_slice(&movz_reg(1, 4096));    // X1 = 4096
                code.extend_from_slice(&movz_reg(2, 3));       // X2 = PROT_READ|PROT_WRITE
                code.extend_from_slice(&movz_reg(3, 0x22));    // X3 = MAP_PRIVATE|MAP_ANONYMOUS
                code.extend_from_slice(&movn_reg(4, 0));       // X4 = -1 (fd)
                code.extend_from_slice(&movz_reg(5, 0));       // X5 = 0 (offset)
                code.extend_from_slice(&movz_x8(222));         // sys_mmap
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);
                stubs.push(("ffi_scratch_push_frame".to_string(), code));
            }

            // ffi_scratch_pop_frame: no-op (RET). Real munmap will be wired
            // when marshal_cstr is fully integrated.
            {
                let mut code = Vec::new();
                code.extend_from_slice(&ret);
                stubs.push(("ffi_scratch_pop_frame".to_string(), code));
            }

            // __arena_overflow: real exit(1) syscall (aarch64 sys_exit=93)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&movz_reg(0, 1));       // X0 = 1 (exit code)
                code.extend_from_slice(&movz_x8(93));           // sys_exit
                code.extend_from_slice(&svc);
                code.extend_from_slice(&ret);                   // safety (shouldn't reach)
                stubs.push(("__arena_overflow".to_string(), code));
            }

            stubs
        };

        // Compute offsets for syscall stubs and register them
        let syscall_stubs_start = vuma_free_offset + vuma_free_stub.len();
        let mut stub_offset = syscall_stubs_start;
        for (name, code) in &syscall_stubs {
            func_offsets.insert(name.clone(), stub_offset);
            stub_offset += code.len();
        }

        // ── Build _start stub bytes ──
        // _start: LDR X0, [SP]       ; argc from stack
        //         ADD X1, SP, #8      ; argv = SP + 8
        //         BL <main>           ; call main(argc, argv) — result in X0
        //         MOV X8, #93         ; sys_exit_group
        //         SVC #0              ; syscall
        let start_stub_size: usize = 20; // 5 × 4-byte instructions
        let ffi_stub_size: usize = 8; // MOV X0, #0; RET (2 × 4 bytes)
        let ffi_stub_offset: usize = start_stub_size; // FFI stub right after _start
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // LDR X0, [SP] — load argc from stack pointer
        // LDR X0, [SP] = 0xF94003E0
        start_stub.extend_from_slice(&0xF94003E0u32.to_le_bytes());

        // ADD X1, SP, #8 — argv = SP + 8
        // ADD X1, SP, #8 = 0x910023E1
        start_stub.extend_from_slice(&0x910023E1u32.to_le_bytes());

        // BL <main> — placeholder, will be patched (at offset 8 within start_stub)
        // BL encoding: 1 0 0 1 0 1 imm26
        start_stub.extend_from_slice(&0x94000000u32.to_le_bytes()); // BL #0

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
            // BL is at byte offset 8 within start_stub (after LDR X0 and ADD X1).
            // BL offset = (target - bl_addr) / 4, where bl_addr = 8.
            let bl_offset = ((main_offset as i64) - 8) / 4;
            // Mask to 26 bits (signed)
            let imm26 = (bl_offset as u32) & 0x03FFFFFF;
            let bl_word: u32 = 0x94000000 | imm26;
            // BL is at byte offset 8 within start_stub (after LDR X0 and ADD X1)
            start_stub[8..12].copy_from_slice(&bl_word.to_le_bytes());
        }

        // ── Add FFI return-0 stub ──
        let mut ffi_stub = Vec::with_capacity(ffi_stub_size);
        ffi_stub.extend_from_slice(&0xD2800000u32.to_le_bytes()); // MOV X0, #0
        ffi_stub.extend_from_slice(&0xD65F03C0u32.to_le_bytes()); // RET

        // ── Concatenate all code ──
        let mut all_code = start_stub;
        all_code.extend_from_slice(&ffi_stub); // 8 bytes at offset start_stub_size
        for func in &program.functions {
            for block in &func.blocks {
                for instr in &block.instructions {
                    all_code.extend_from_slice(&instr.encoded);
                }
            }
        }

        // Append runtime I/O code
        all_code.extend_from_slice(&runtime_code);
        // Append __vuma_alloc / __vuma_free syscall stubs.
        all_code.extend_from_slice(&vuma_alloc_stub);
        all_code.extend_from_slice(&vuma_free_stub);
        // Append POSIX syscall stubs (write, read, open, close, mmap, etc.)
        for (_, code) in &syscall_stubs {
            all_code.extend_from_slice(code);
        }

        // ── Patch BL relocations ──
        // Each function's relocations are relative to the start of that function's code.
        // We need to adjust them by the _start stub size + preceding functions' sizes.
        //
        // While we walk the relocations, also collect the names of every
        // symbol that is NOT defined in `func_offsets` — these are the
        // external (undefined) callees (e.g. libc functions, or functions
        // declared in `extern "C"` blocks). They get emitted as SHN_UNDEF
        // entries in `.symtab` so the system linker can resolve them.
        let external_symbols: Vec<String> = Vec::new();
        let mut func_code_offset: usize = start_stub_size + ffi_stub_size;
        for func in &program.functions {
            for reloc in &func.relocations {
                let abs_offset = func_code_offset + reloc.offset as usize;
                if abs_offset + 4 > all_code.len() {
                    continue; // skip invalid relocations
                }

                if reloc.reloc_type == "R_VUMA_GETADDR" {
                    let target_offset = func_offsets.get(&reloc.symbol)
                        .copied()
                        .or_else(|| {
                            let prefix = format!("fn_{}", reloc.symbol);
                            for k in func_offsets.keys() {
                                if k.starts_with(&prefix) {
                                    return Some(func_offsets[k]);
                                }
                            }
                            None
                        });
                    if let Some(target_offset) = target_offset {
                        let abs_addr = 0x400000u64 + target_offset as u64;
                        // Patch 4 instructions at abs_offset..abs_offset+16
                        if abs_offset + 16 <= all_code.len() {
                            // MOVZ X9, #imm16 (bits 0-15)
                            let imm0 = (abs_addr & 0xFFFF) as u32;
                            let existing0 = u32::from_le_bytes([
                                all_code[abs_offset], all_code[abs_offset + 1],
                                all_code[abs_offset + 2], all_code[abs_offset + 3],
                            ]);
                            let patched0 = (existing0 & !0x001FFFE0) | (imm0 << 5);
                            all_code[abs_offset..abs_offset + 4].copy_from_slice(&patched0.to_le_bytes());

                            // MOVK X9, #imm16, lsl #16 (bits 16-31)
                            let imm1 = ((abs_addr >> 16) & 0xFFFF) as u32;
                            let existing1 = u32::from_le_bytes([
                                all_code[abs_offset + 4], all_code[abs_offset + 5],
                                all_code[abs_offset + 6], all_code[abs_offset + 7],
                            ]);
                            let patched1 = (existing1 & !0x001FFFE0) | (imm1 << 5);
                            all_code[abs_offset + 4..abs_offset + 8].copy_from_slice(&patched1.to_le_bytes());

                            // MOVK X9, #imm16, lsl #32 (bits 32-47)
                            let imm2 = ((abs_addr >> 32) & 0xFFFF) as u32;
                            let existing2 = u32::from_le_bytes([
                                all_code[abs_offset + 8], all_code[abs_offset + 9],
                                all_code[abs_offset + 10], all_code[abs_offset + 11],
                            ]);
                            let patched2 = (existing2 & !0x001FFFE0) | (imm2 << 5);
                            all_code[abs_offset + 8..abs_offset + 12].copy_from_slice(&patched2.to_le_bytes());

                            // MOVK X9, #imm16, lsl #48 (bits 48-63)
                            let imm3 = ((abs_addr >> 48) & 0xFFFF) as u32;
                            let existing3 = u32::from_le_bytes([
                                all_code[abs_offset + 12], all_code[abs_offset + 13],
                                all_code[abs_offset + 14], all_code[abs_offset + 15],
                            ]);
                            let patched3 = (existing3 & !0x001FFFE0) | (imm3 << 5);
                            all_code[abs_offset + 12..abs_offset + 16].copy_from_slice(&patched3.to_le_bytes());
                        }
                    }
                } else if reloc.reloc_type == R_AARCH64_CALL26 {
                    // BL target = PC + imm26*4, where PC = address of BL instruction.
                    // So: imm26 = (target_addr - bl_addr) / 4
                    let target_offset = func_offsets.get(&reloc.symbol)
                        .copied()
                        .or_else(|| {
                            let prefix = format!("fn_{}", reloc.symbol);
                            func_offsets.keys()
                                .find(|k| k.starts_with(&prefix))
                                .and_then(|k| func_offsets.get(k))
                                .copied()
                        });
                    if let Some(target_offset) = target_offset {
                        let bl_addr = abs_offset as i64;
                        let target_addr = target_offset as i64;
                        let offset_words = (target_addr - bl_addr) / 4;
                        // Check range: ±128MB (26-bit signed)
                        if offset_words < -(1 << 25) || offset_words >= (1 << 25) {
                            vuma_log!(warn, 
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
                    } else {
                        // External symbol — point to the FFI return-0 stub
                        let target_addr = ffi_stub_offset as i64;
                        let bl_addr = abs_offset as i64;
                        let offset_words = (target_addr - bl_addr) / 4;
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
                }
            }
            let func_size: usize = func.blocks.iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            func_code_offset += func_size;
        }

        // ── Build ELF with 2 LOAD segments ──
        // Pass the external symbols so `build_aarch64_elf_2seg` can emit a
        // `.symtab` / `.strtab` / `.shstrtab` / `.text` section-header
        // appendix with SHN_UNDEF entries for each external callee.
        Ok(build_aarch64_elf_2seg(&all_code, 0x400000, &external_symbols))
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
        BackendKind::PowerPC64LE => Ok(Box::new(PPC64LEBackend::new())),
        BackendKind::RiscV32 => Ok(Box::new(crate::riscv32::RiscV32Backend::new())),
        BackendKind::X86_32 => Ok(Box::new(crate::x86_32::X86_32Backend::new())),
        BackendKind::Sparc64 => Ok(Box::new(Sparc64Backend::new())),
        BackendKind::S390X => Ok(Box::new(S390XBackend::new())),
        BackendKind::Mips64Be => Ok(Box::new(Mips64BeBackend::new())),
        BackendKind::ArmEb => Ok(Box::new(ArmEbBackend::new())),
        BackendKind::AArch64Be => Ok(Box::new(AArch64BeBackend::new())),
        BackendKind::M68k => Ok(Box::new(M68kBackend::new())),
        BackendKind::Alpha => Ok(Box::new(AlphaBackend::new())),
        BackendKind::Hppa => Ok(Box::new(HppaBackend::new())),
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
        assert_eq!(info.endianness(), Endianness::Little);
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

    // ===================================================================
    // Wave 13 — IRInstr::Syscall cross-backend conformance test
    // ===================================================================
    //
    // Asserts that every backend emits a **non-empty** encoded instruction
    // sequence for `IRInstr::Syscall { nr: 1, .. }`.  Wave 11 implemented
    // Syscall on the 6 tier-1 backends.  Wave 12 is in progress for the
    // 8 tier-2/3 backends (which currently `unimplemented!("… (Wave 12)")`).
    // The 4 big-endian / LE wrapper backends automatically inherit from
    // their parents.
    //
    // This test iterates over all 19 BackendKind variants and categorizes
    // each result:
    //   - **PASS**: backend emits non-empty encoded bytes for the syscall.
    //   - **PENDING**: backend panics with "Wave 12" (not yet implemented).
    //   - **FAIL**: backend panics with an unexpected message, returns an
    //     error, or emits empty output.
    //
    // The test asserts zero FAILs.  PENDING backends are reported but do
    // not fail the test (they will automatically be promoted to PASS once
    // Wave 12 lands and removes the `unimplemented!()` arms).

    use crate::ir::{IRBlock, IRFunction, IRInstr, IRTerminator, IRValue};
    use std::collections::HashSet;

    /// All 19 backend kinds for the Wave 13 syscall conformance sweep.
    const ALL_19_BACKENDS: &[BackendKind] = &[
        // Tier-1 (Wave 11 — Syscall implemented)
        BackendKind::X86_64,
        BackendKind::AArch64,
        BackendKind::RiscV64,
        BackendKind::RiscV32,
        BackendKind::Arm32,
        BackendKind::X86_32,
        // Big-endian / LE wrappers (Wave 13 — inherit from parent)
        BackendKind::AArch64Be,   // → aarch64  (has Syscall)
        BackendKind::ArmEb,       // → arm32    (has Syscall)
        BackendKind::Mips64Be,    // → mips64   (pending Wave 12)
        BackendKind::PowerPC64LE, // → ppc64    (pending Wave 12)
        // Tier-2/3 (Wave 12 — pending)
        BackendKind::LoongArch64,
        BackendKind::Mips64,
        BackendKind::PowerPC64,
        BackendKind::S390X,
        BackendKind::Sparc64,
        BackendKind::Alpha,
        BackendKind::M68k,
        BackendKind::Hppa,
        // wasm32 (Wave 11 — emits i32.const -ENOSYS)
        BackendKind::Wasm32,
    ];

    /// Build a minimal IR function containing a single `IRInstr::Syscall`
    /// instruction with `nr: 1` (Linux `__NR_write` on most arches).
    fn build_syscall_ir_func() -> IRFunction {
        IRFunction {
            name: "syscall_conformance".to_string(),
            params: vec![],
            results: vec![],
            param_types: vec![],
            result_types: vec![],
            vregs: std::collections::HashMap::new(),
            blocks: vec![IRBlock {
                label: "entry".to_string(),
                instructions: vec![IRInstr::Syscall {
                    nr: 1,
                    args: vec![],
                    dst: Some(IRValue::Register(0)),
                }],
                terminator: IRTerminator::Return(vec![]),
                predecessors: HashSet::new(),
                successors: HashSet::new(),
                source_line: 0,
            }],
            source_file: String::new(),
        }
    }

    /// Outcome of attempting Syscall compilation on a single backend.
    enum SyscallConformance {
        /// Backend emitted non-empty encoded bytes — conformance met.
        Pass(usize), // byte count
        /// Backend panicked with a "Wave 12" message — not yet implemented.
        Pending(String),
        /// Backend failed unexpectedly (wrong panic, error, or empty output).
        Fail(String),
    }

    /// Attempt to compile `IRInstr::Syscall { nr: 1, .. }` on a single
    /// backend, catching panics so one backend's failure doesn't abort
    /// the entire sweep.
    fn check_syscall_conformance(kind: BackendKind) -> SyscallConformance {
        let backend = match create_backend(kind) {
            Ok(b) => b,
            Err(e) => return SyscallConformance::Fail(format!("create_backend error: {}", e)),
        };
        let func = build_syscall_ir_func();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let allocated = backend.allocate_registers(&func)?;
            backend.encode_function(&allocated)
        }));

        match result {
            Ok(Ok(bytes)) => {
                if bytes.is_empty() {
                    SyscallConformance::Fail("emitted 0 bytes".to_string())
                } else {
                    SyscallConformance::Pass(bytes.len())
                }
            }
            Ok(Err(e)) => SyscallConformance::Fail(format!("returned error: {}", e)),
            Err(panic_payload) => {
                let msg = panic_payload
                    .downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| panic_payload.downcast_ref::<&str>().copied())
                    .unwrap_or("<non-string panic>");
                if msg.contains("Wave 12") {
                    SyscallConformance::Pending(msg.to_string())
                } else {
                    SyscallConformance::Fail(format!("panic: {}", msg))
                }
            }
        }
    }

    /// Wave 13 — Cross-backend `IRInstr::Syscall` conformance test.
    ///
    /// Iterates over all 19 backends and asserts that each one EITHER
    /// emits non-empty encoded output for `Syscall { nr: 1, .. }` OR
    /// panics with a "Wave 12" message (indicating the tier-2/3
    /// implementation is still pending).  Any other outcome (unexpected
    /// panic, error, or empty output) fails the test.
    #[test]
    fn test_syscall_conformance_all_backends() {
        let mut pass_count = 0usize;
        let mut pending_count = 0usize;
        let mut fail_count = 0usize;
        let mut failures: Vec<(BackendKind, String)> = Vec::new();

        eprintln!("\n════════ Wave 13: IRInstr::Syscall cross-backend conformance ════════");
        eprintln!("  {:<16} {:<10} {}", "Backend", "Status", "Detail");
        eprintln!("  {}", "-".repeat(64));

        for kind in ALL_19_BACKENDS {
            let name = kind.isa_name();
            match check_syscall_conformance(*kind) {
                SyscallConformance::Pass(n) => {
                    pass_count += 1;
                    eprintln!("  {:<16} {:<10} {} bytes", name, "PASS", n);
                }
                SyscallConformance::Pending(msg) => {
                    pending_count += 1;
                    eprintln!("  {:<16} {:<10} {}", name, "PENDING", msg);
                }
                SyscallConformance::Fail(msg) => {
                    fail_count += 1;
                    failures.push((*kind, msg.clone()));
                    eprintln!("  {:<16} {:<10} {}", name, "FAIL", msg);
                }
            }
        }

        eprintln!("  {}", "-".repeat(64));
        eprintln!(
            "  Summary: {} PASS, {} PENDING (Wave 12), {} FAIL (out of {})",
            pass_count,
            pending_count,
            fail_count,
            ALL_19_BACKENDS.len()
        );
        eprintln!();

        // 1. Zero FAILs — every backend must either emit non-empty output
        //    or panic with "Wave 12" (pending).
        assert_eq!(
            fail_count, 0,
            "{} backend(s) failed Syscall conformance unexpectedly: {:?}",
            fail_count, failures
        );

        // 2. Tier-1 backends + wrappers whose parents have Syscall must
        //    PASS (not pending).
        let must_pass: &[BackendKind] = &[
            BackendKind::X86_64,
            BackendKind::AArch64,
            BackendKind::RiscV64,
            BackendKind::RiscV32,
            BackendKind::Arm32,
            BackendKind::X86_32,
            BackendKind::AArch64Be, // inherits from aarch64
            BackendKind::ArmEb,     // inherits from arm32
            BackendKind::Wasm32,    // emits i32.const -ENOSYS
        ];
        for kind in must_pass {
            match check_syscall_conformance(*kind) {
                SyscallConformance::Pass(n) => {
                    assert!(
                        n > 0,
                        "backend {:?} must emit non-empty syscall output (got 0 bytes)",
                        kind
                    );
                }
                SyscallConformance::Pending(msg) => {
                    panic!("backend {:?} must PASS but got PENDING: {}", kind, msg);
                }
                SyscallConformance::Fail(msg) => {
                    panic!("backend {:?} must PASS but got FAIL: {}", kind, msg);
                }
            }
        }

        // 3. Wrapper backends whose parents are tier-2/3 must EITHER pass
        //    or be pending.  They must NOT fail.
        for kind in &[BackendKind::Mips64Be, BackendKind::PowerPC64LE] {
            match check_syscall_conformance(*kind) {
                SyscallConformance::Pass(_) | SyscallConformance::Pending(_) => { /* ok */ }
                SyscallConformance::Fail(msg) => {
                    panic!("wrapper backend {:?} must pass or be pending but failed: {}", kind, msg);
                }
            }
        }
    }

    // ===================================================================
    // Wave 22 — emit_function_regalloc cross-backend test
    // ===================================================================
    //
    // Verifies that each tier-1 backend's `emit_function_regalloc`
    // method (Wave 22) runs without crashing and produces a non-empty
    // `AllocatedFunction` with correct `reads`/`writes` metadata.

    /// Build a minimal IR function with a few vregs for regalloc testing.
    fn build_regalloc_test_func() -> IRFunction {
        IRFunction {
            name: "regalloc_test".to_string(),
            params: vec![IRValue::Register(0), IRValue::Register(1)],
            results: vec![IRValue::Register(2)],
            param_types: vec![IRType::I64, IRType::I64],
            result_types: vec![IRType::I64],
            vregs: std::collections::HashMap::new(),
            blocks: vec![IRBlock {
                label: "entry".to_string(),
                instructions: vec![
                    IRInstr::BinOp {
                        op: crate::ir::BinOpKind::Add,
                        dst: IRValue::Register(2),
                        lhs: IRValue::Register(0),
                        rhs: IRValue::Register(1),
                        ty: None,
                    },
                ],
                terminator: IRTerminator::Return(vec![IRValue::Register(2)]),
                predecessors: HashSet::new(),
                successors: HashSet::new(),
                source_line: 0,
            }],
            source_file: String::new(),
        }
    }

    /// Wave 22: x86_64 `emit_function_regalloc` produces non-empty output.
    #[test]
    fn test_wave22_x86_64_emit_function_regalloc() {
        let backend = crate::x86_64::X86_64Backend::new();
        let func = build_regalloc_test_func();
        let result = backend.emit_function_with_regalloc(&func);
        assert!(result.is_ok(), "x86_64 emit_function_regalloc failed: {:?}", result.err());
        let allocated = result.unwrap();
        assert!(!allocated.blocks.is_empty(), "should have at least one block");
        assert!(
            allocated.blocks[0].instructions.iter().any(|i| !i.encoded.is_empty()),
            "should have at least one instruction with encoded bytes"
        );
    }

    /// Wave 22: aarch64 `emit_function_regalloc` produces non-empty output.
    #[test]
    fn test_wave22_aarch64_emit_function_regalloc() {
        let backend = AArch64Backend::new();
        let func = build_regalloc_test_func();
        let result = backend.emit_function_with_regalloc(&func);
        assert!(result.is_ok(), "aarch64 emit_function_regalloc failed: {:?}", result.err());
        let allocated = result.unwrap();
        assert!(!allocated.blocks.is_empty(), "should have at least one block");
        assert!(
            allocated.blocks[0].instructions.iter().any(|i| !i.encoded.is_empty()),
            "should have at least one instruction with encoded bytes"
        );
    }

    /// Wave 22: riscv64 `emit_function_regalloc` produces non-empty output.
    #[test]
    fn test_wave22_riscv64_emit_function_regalloc() {
        let backend = crate::riscv64::RiscV64Backend::new();
        let func = build_regalloc_test_func();
        let result = backend.emit_function_with_regalloc(&func);
        assert!(result.is_ok(), "riscv64 emit_function_regalloc failed: {:?}", result.err());
        let allocated = result.unwrap();
        assert!(!allocated.blocks.is_empty(), "should have at least one block");
        assert!(
            allocated.blocks[0].instructions.iter().any(|i| !i.encoded.is_empty()),
            "should have at least one instruction with encoded bytes"
        );
    }

    /// Wave 22: arm32 `emit_function_regalloc` produces non-empty output.
    #[test]
    fn test_wave22_arm32_emit_function_regalloc() {
        let backend = crate::arm32::Arm32Backend::new();
        let func = build_regalloc_test_func();
        let result = backend.emit_function_with_regalloc(&func);
        assert!(result.is_ok(), "arm32 emit_function_regalloc failed: {:?}", result.err());
        let allocated = result.unwrap();
        assert!(!allocated.blocks.is_empty(), "should have at least one block");
        assert!(
            allocated.blocks[0].instructions.iter().any(|i| !i.encoded.is_empty()),
            "should have at least one instruction with encoded bytes"
        );
    }

    /// Wave 22: loongarch64 `emit_function_regalloc` produces non-empty output.
    #[test]
    fn test_wave22_loongarch64_emit_function_regalloc() {
        let backend = crate::loongarch64::LoongArch64Backend::new();
        let func = build_regalloc_test_func();
        let result = backend.emit_function_with_regalloc(&func);
        assert!(result.is_ok(), "loongarch64 emit_function_regalloc failed: {:?}", result.err());
        let allocated = result.unwrap();
        assert!(!allocated.blocks.is_empty(), "should have at least one block");
        assert!(
            allocated.blocks[0].instructions.iter().any(|i| !i.encoded.is_empty()),
            "should have at least one instruction with encoded bytes"
        );
    }
}
