//! VUMA runtime support modules.
//!
//! These are compiled into the VUMA binary and provide runtime services:
//!   - ffi_scratch: thread-local scratchpad for FFI marshalling
//!   - callback: re-entrancy guard for foreign callbacks (Wave 7)
//!   - vuma_context: the vuma_context_t C-API accessors (Wave 7)
//!   - arena: Rust-level arena allocator for testing and callback path

pub mod ffi_scratch;
pub mod callback;
pub mod vuma_context;
pub mod arena;
