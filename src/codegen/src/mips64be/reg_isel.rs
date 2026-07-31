//! Full register-based instruction selection for mips64be.
//!
//! Delegates to the mips64 backend's reg_isel — the BE wrapper
//! byte-swaps the output in encode_function/encode_program.

pub use crate::mips64::reg_isel::emit_function_regalloc_full;
