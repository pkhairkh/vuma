//! Full register-based instruction selection for armeb.
//!
//! Delegates to the arm32 backend's reg_isel — the BE wrapper
//! byte-swaps the output in encode_function/encode_program.

pub use crate::arm32::reg_isel::emit_function_regalloc_full;
