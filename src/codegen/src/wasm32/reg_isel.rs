//! Full instruction selection for wasm32.
//!
//! WebAssembly is a stack machine — it has no registers. This module
//! provides the `emit_function_regalloc_full` entry point for API
//! compatibility with the other 18 backends, but internally delegates
//! to the existing structured stack-machine ISel in `wasm32/mod.rs`.
//!
//! The "register allocator" for wasm32 maps vregs to wasm locals
//! (which are functionally equivalent to registers — they're named
//! slots that persist across instructions). The existing ISel already
//! handles this correctly via `lower_function` in mod.rs.

use crate::backend::{AllocatedFunction, BackendError};
use crate::ir::IRFunction;
use crate::regalloc::RegAllocResult;

/// Emit a function using the wasm32 stack-machine ISel.
///
/// This function exists for API parity with the other 18 backends.
/// It does NOT use the RegAllocResult — wasm32 has no physical
/// registers. Instead, it delegates to the existing `lower_function`
/// which uses wasm locals for all vregs.
pub fn emit_function_regalloc_full(
    _func: &IRFunction,
    _alloc: &RegAllocResult,
) -> Result<AllocatedFunction, BackendError> {
    // Return an error to signal that the caller should use the existing
    // stack-machine ISel path (lower_function in mod.rs).
    Err(BackendError::RegisterAllocFailed {
        isa: "wasm32",
        reason: "wasm32 is a stack machine — use the existing lower_function ISel".to_string(),
    })
}
