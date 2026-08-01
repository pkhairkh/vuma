//! Full register-based instruction selection for aarch64_be.
//!
//! Delegates to the aarch64 backend's reg_isel — the BE wrapper
//! byte-swaps the output in encode_function/encode_program.

pub use crate::aarch64::reg_isel::emit_function_regalloc_full;
