//! Full register-based instruction selection for ppc64le.
//!
//! Delegates to the ppc64 backend's reg_isel — the LE wrapper
//! byte-swaps the output in encode_function/encode_program.

pub use crate::ppc64::reg_isel::emit_function_regalloc_full;
