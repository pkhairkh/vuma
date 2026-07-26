//! VUMA runtime support modules.
//!
//! These are compiled into the VUMA binary and provide runtime services:
//!   - ffi_scratch: thread-local scratchpad for FFI marshalling
//! - callback: re-entrancy guard for foreign callbacks
//! - vuma_context: the vuma_context_t C-API accessors
//!   - arena: Rust-level arena allocator for testing and callback path

pub mod arena;
pub mod callback;
pub mod ffi_scratch;
#[cfg(feature = "pmt-runtime-check")]
pub mod pmt_check;
pub mod vuma_context;
