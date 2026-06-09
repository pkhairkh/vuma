//! # VUMA Code Generation Module
//!
//! This crate handles code generation for the VUMA programming language,
//! targeting ARM64 (AArch64) architectures — specifically the Raspberry Pi 5.
//!
//! ## Pipeline
//!
//! 1. **SCG → IR**: Convert Semantic Computation Graph nodes into an
//!    intermediate representation (`scg_to_ir`).
//! 2. **Register Allocation**: Assign physical ARM64 registers to IR values
//!    (`regalloc`).
//! 3. **Emission**: Generate ARM64 machine code and produce ELF binaries
//!    (`emit`).
//!
//! ## Module Layout
//!
//! - `arm64` — ARM64 instruction definitions, register/condition enums, and
//!   binary encoding.
//! - `ir` — Intermediate representation types (functions, blocks, instructions,
//!   terminators, values).
//! - `scg_to_ir` — Translation from SCG nodes to IR.
//! - `regalloc` — Simple register allocator.
//! - `emit` — ARM64 code emitter and ELF generation.

pub mod arm64;
pub mod emit;
pub mod ir;
pub mod regalloc;
pub mod scg_to_ir;

/// Re-export the primary pipeline entry point for convenience.
pub use scg_to_ir::ScgToIr;

/// Error type for code-generation failures.
#[derive(Debug, thiserror::Error)]
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
}

/// Convenience alias for results in this crate.
pub type Result<T> = std::result::Result<T, CodegenError>;
