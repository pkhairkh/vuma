//! # VUMA Code Generation Module
//!
//! This crate handles code generation for the VUMA programming language,
//! supporting multiple instruction set architectures (AArch64, RISC-V64,
//! Wasm32, LoongArch64, x86_64, ARM32, MIPS64, PowerPC64).
//!
//! ## Pipeline
//!
//! 1. **SCG → IR**: Convert Semantic Computation Graph nodes into an
//!    intermediate representation (`scg_to_ir`).
//! 2. **Register Allocation**: Assign physical registers to IR values
//!    (`regalloc`), or stack-based lowering for Wasm.
//! 3. **Emission**: Generate target machine code and produce ELF / Wasm
//!    binaries (`emit`, `backend`).
//!
//! ## Module Layout
//!
//! - `backend` — Multi-architecture trait definitions (`TargetInfo`, `Backend`)
//!   and per-ISA target info implementations.
//! - `arm64` — ARM64 instruction definitions, register/condition enums, and
//!   binary encoding.
//! - `ir` — Intermediate representation types (functions, blocks, instructions,
//!   terminators, values).
//! - `scg_to_ir` — Translation from SCG nodes to IR.
//! - `regalloc` — Simple register allocator.
//! - `emit` — ARM64 code emitter and ELF generation.
//! - `memory_safety` — Compile-time and runtime memory safety checks (E041–E050).

pub mod arm32;
pub mod arm64;
pub mod backend;
pub mod control_flow;
pub mod dwarf;
pub mod emit;
pub mod ir;
pub mod loongarch64;
pub mod memory_safety;
pub mod mips64;
pub mod opt;
pub mod ppc64;
pub mod regalloc;
pub mod riscv64;
pub mod riscv32;
pub mod scg_to_ir;
pub mod target_desc;
pub mod wasm32;
pub mod womb;
pub mod x86_64;
pub mod x86_32;

/// Re-export the primary pipeline entry point for convenience.
pub use scg_to_ir::ScgToIr;

/// Re-export commonly used IR types.
pub use ir::{CastKind, DataSectionKind};

/// Re-export multi-architecture backend types.
pub use backend::{
    create_backend, AArch64Backend, AllocatedBlock, AllocatedFunction, AllocatedInstruction,
    AllocatedProgram, Backend, BackendError, BackendKind, Endianness, OutputFormat, PhysicalReg,
    RegClass, TargetInfo,
};

/// Re-export ARM 32-bit backend types.
pub use arm32::Arm32Backend;

/// Re-export RISC-V 64-bit backend types.
pub use riscv64::RiscV64Backend;

/// Re-export Wasm32 backend types.
pub use wasm32::Wasm32Backend;

/// Re-export the Wasm32 compile_to_wasm convenience function.
pub use wasm32::compile_to_wasm;

/// Re-export LoongArch64 backend types.
pub use loongarch64::LoongArch64Backend;

/// Re-export x86_64 backend types.
pub use x86_64::X86_64Backend;

/// Re-export MIPS64 backend types.
pub use mips64::Mips64Backend;

/// Re-export PowerPC64 backend types.
pub use ppc64::PPC64Backend;

/// Re-export target description types.
pub use target_desc::{
    CallingConventionDesc, InstCategoryDesc, RegDesc, TargetDesc, TargetDescRegistry,
};

/// Re-export memory safety types.
pub use memory_safety::{
    BoundsCheckSite, MemorySafetyAnalyzer, MemorySafetyConfig, MemorySafetyReport,
    MemorySafetyViolation,
};

/// Error type for code-generation failures.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CodegenError {
    /// An unsupported or invalid instruction was encountered.
    #[error("invalid instruction: {0}")]
    InvalidInstruction(String),

    /// Register allocation failed (e.g. all registers are in use).
    #[error("register allocation failed: {0}")]
    RegisterAllocFailed(String),

    /// Failed to encode an ARM64 instruction.
    #[error("encoding error: {0}")]
    EncodingError(String),

    /// An IR translation error occurred.
    #[error("IR translation error: {0}")]
    TranslationError(String),

    /// ELF emission error.
    #[error("ELF emission error: {0}")]
    ElfError(String),

    /// An unknown variable was referenced during SCG → IR translation.
    #[error("unknown variable '{name}' referenced in SCG")]
    UnknownVariable {
        /// Name of the unresolved variable.
        name: String,
    },

    /// A required WASM section was not found during module generation.
    #[error("WASM section not found: {section}")]
    WasmSectionNotFound {
        /// Name of the missing WASM section.
        section: String,
    },

    /// A relocation references a symbol that could not be resolved during
    /// program encoding. This is a fatal error — the binary cannot be
    /// correctly linked without resolving all relocations.
    #[error("unresolved relocation: symbol '{symbol}' in function '{function}' at offset 0x{offset:X} ({reloc_type})")]
    UnresolvedRelocation {
        /// Name of the unresolved symbol.
        symbol: String,
        /// Name of the function containing the reference.
        function: String,
        /// Byte offset within the function where the relocation applies.
        offset: u64,
        /// Relocation type string (e.g., "R_AARCH64_CALL26").
        reloc_type: String,
    },
}

/// Convenience alias for results in this crate.
pub type Result<T> = std::result::Result<T, CodegenError>;
