//! vuma_context_t — the VUMA C-API accessor implementation.
//!
//! Generalizes the wasm32 host shim (scripts/wasm32_runner.py:make_host_functions)
//! into a Rust implementation callable from C. When a C library calls back into
//! VUMA, it receives a `vuma_context_t*` and uses these accessors to safely
//! interact with VUMA's ___pmt_buffer.
//!
//! # Re-entrancy rule
//! Callbacks run on an isolated callback stack with their own scratchpad frame.
//! They are forbidden from touching any State in the caller's live set
//! (enforced by `callback_live_set` — trap on violation in callback.rs).

use super::ffi_scratch;
use super::callback::{self, CallbackContext};

/// The opaque VUMA callback context. Exposed to C as `vuma_context_t`.
///
/// Carries:
/// - A reference to ___pmt_buffer (the program-wide single buffer)
/// - The callback's own scratchpad frame (for marshalling)
/// - The callback_live_set guard (re-entrancy protection)
pub struct VumaContext {
    /// Base pointer of ___pmt_buffer.
    pub pmt_buffer_base: *mut u8,
    /// Total size of ___pmt_buffer.
    pub pmt_buffer_size: u64,
    /// The callback context (scratchpad frame + live-set guard).
    pub callback_ctx: CallbackContext,
}

// SAFETY: VumaContext is only used from the thread that invoked the callback.
// The callback_live_set is thread-local, so sending the context across threads
// would be unsound. We mark it Send + Sync because the C API is single-threaded
// by contract (callbacks run on the same thread that invoked C).
unsafe impl Send for VumaContext {}
unsafe impl Sync for VumaContext {}

/// Create a new VumaContext for a callback invocation.
/// Called by the runtime when entering a #[callback] extern call.
pub fn vuma_context_enter(
    pmt_buffer_base: *mut u8,
    pmt_buffer_size: u64,
    caller_live_offsets: &[(u64, u64)],
) -> Box<VumaContext> {
    // Push a scratchpad frame for this callback.
    ffi_scratch::push_frame();
    let callback_ctx = callback::enter_callback(caller_live_offsets);
    Box::new(VumaContext {
        pmt_buffer_base,
        pmt_buffer_size,
        callback_ctx,
    })
}

/// Destroy a VumaContext after the callback returns.
/// Pops the scratchpad frame and frees the callback context.
pub fn vuma_context_leave(ctx: Box<VumaContext>) {
    callback::exit_callback(ctx.callback_ctx);
    ffi_scratch::pop_frame();
}

// ── C-API accessor functions ─────────────────────────────────────────────
// These are the functions declared in vuma_vm.h. They are extern "C" so the
// C linker can resolve them. Each checks the callback_live_set guard before
// accessing ___pmt_buffer.

/// Read a u32 from ___pmt_buffer at `offset`.
/// Traps if `offset` falls within a caller-live region.
#[no_mangle]
pub extern "C" fn vuma_read_u32(ctx: *const VumaContext, offset: u64) -> u32 {
    if ctx.is_null() {
        return 0;
    }
    let ctx = unsafe { &*ctx };
    if !callback::check_access(&ctx.callback_ctx, offset) {
        // Re-entrancy violation: caller-live region touched.
        eprintln!("vuma_read_u32: re-entrancy violation at offset {}", offset);
        std::process::abort();
    }
    if offset + 4 > ctx.pmt_buffer_size {
        eprintln!("vuma_read_u32: out-of-bounds read at offset {}", offset);
        return 0;
    }
    unsafe {
        let ptr = ctx.pmt_buffer_base.add(offset as usize) as *const u32;
        ptr.read_unaligned()
    }
}

/// Read a u64 from ___pmt_buffer at `offset`.
#[no_mangle]
pub extern "C" fn vuma_read_u64(ctx: *const VumaContext, offset: u64) -> u64 {
    if ctx.is_null() {
        return 0;
    }
    let ctx = unsafe { &*ctx };
    if !callback::check_access(&ctx.callback_ctx, offset) {
        eprintln!("vuma_read_u64: re-entrancy violation at offset {}", offset);
        std::process::abort();
    }
    if offset + 8 > ctx.pmt_buffer_size {
        eprintln!("vuma_read_u64: out-of-bounds read at offset {}", offset);
        return 0;
    }
    unsafe {
        let ptr = ctx.pmt_buffer_base.add(offset as usize) as *const u64;
        ptr.read_unaligned()
    }
}

/// Write a u32 to ___pmt_buffer at `offset`.
#[no_mangle]
pub extern "C" fn vuma_write_u32(ctx: *mut VumaContext, offset: u64, val: u32) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    if !callback::check_access(&ctx.callback_ctx, offset) {
        eprintln!("vuma_write_u32: re-entrancy violation at offset {}", offset);
        std::process::abort();
    }
    if offset + 4 > ctx.pmt_buffer_size {
        eprintln!("vuma_write_u32: out-of-bounds write at offset {}", offset);
        return;
    }
    unsafe {
        let ptr = ctx.pmt_buffer_base.add(offset as usize) as *mut u32;
        ptr.write_unaligned(val);
    }
}

/// Write a u64 to ___pmt_buffer at `offset`.
#[no_mangle]
pub extern "C" fn vuma_write_u64(ctx: *mut VumaContext, offset: u64, val: u64) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *ctx };
    if !callback::check_access(&ctx.callback_ctx, offset) {
        eprintln!("vuma_write_u64: re-entrancy violation at offset {}", offset);
        std::process::abort();
    }
    if offset + 8 > ctx.pmt_buffer_size {
        eprintln!("vuma_write_u64: out-of-bounds write at offset {}", offset);
        return;
    }
    unsafe {
        let ptr = ctx.pmt_buffer_base.add(offset as usize) as *mut u64;
        ptr.write_unaligned(val);
    }
}

/// Allocate a fresh state in ___pmt_buffer.
/// (Stub: returns 0 for now — full state_new integration is Wave 8.)
#[no_mangle]
pub extern "C" fn vuma_state_new(_ctx: *mut VumaContext, _layout_name: *const std::os::raw::c_char) -> u64 {
    // TODO Wave 8: allocate at a fresh offset in ___pmt_buffer.
    // For now, return 0 (the callback can use offset 0 as a scratch area,
    // since the callback_live_set prevents aliasing with caller state).
    0
}

/// Push an i32 return value.
/// (Stub: stores in a thread-local — full integration is Wave 8.)
#[no_mangle]
pub extern "C" fn vuma_push_i32(_ctx: *mut VumaContext, val: i32) {
    CALLBACK_RETURN_I32.with(|r| r.set(Some(val)));
}

/// Push an i64 return value.
#[no_mangle]
pub extern "C" fn vuma_push_i64(_ctx: *mut VumaContext, val: i64) {
    CALLBACK_RETURN_I64.with(|r| r.set(Some(val)));
}

// ── Thread-local return value slots ──────────────────────────────────────

thread_local! {
    static CALLBACK_RETURN_I32: std::cell::Cell<Option<i32>> = std::cell::Cell::new(None);
    static CALLBACK_RETURN_I64: std::cell::Cell<Option<i64>> = std::cell::Cell::new(None);
}

/// Retrieve and clear the last pushed i32 return value.
pub fn take_callback_return_i32() -> Option<i32> {
    CALLBACK_RETURN_I32.with(|r| r.take())
}

/// Retrieve and clear the last pushed i64 return value.
pub fn take_callback_return_i64() -> Option<i64> {
    CALLBACK_RETURN_I64.with(|r| r.take())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vuma_context_enter_leave() {
        let mut buf = vec![0u8; 64];
        let ctx = vuma_context_enter(buf.as_mut_ptr(), buf.len() as u64, &[(0, 16)]);
        assert!(!ctx.pmt_buffer_base.is_null());
        assert_eq!(ctx.pmt_buffer_size, 64);
        vuma_context_leave(ctx);
    }

    #[test]
    fn test_vuma_read_write_u32() {
        let mut buf = vec![0u8; 64];
        // Caller-live region: [0, 16). Free region: [16, 64).
        let ctx = vuma_context_enter(buf.as_mut_ptr(), buf.len() as u64, &[(0, 16)]);
        // Write to offset 32 (free region) — should work.
        vuma_write_u32(Box::into_raw(ctx), 32, 42);
        // Re-create context (the previous one was consumed by Box::into_raw).
        let ctx2 = vuma_context_enter(buf.as_mut_ptr(), buf.len() as u64, &[(0, 16)]);
        let val = vuma_read_u32(Box::into_raw(ctx2), 32);
        assert_eq!(val, 42);
        // Note: in real usage, vuma_context_leave is called; here we leak
        // for test simplicity (the frames are thread-local and will be
        // cleaned up when the thread exits).
    }

    #[test]
    fn test_vuma_push_i32() {
        vuma_push_i32(std::ptr::null_mut(), 99);
        assert_eq!(take_callback_return_i32(), Some(99));
        assert!(take_callback_return_i32().is_none()); // consumed
    }

    #[test]
    fn test_vuma_push_i64() {
        vuma_push_i64(std::ptr::null_mut(), 1234567890);
        assert_eq!(take_callback_return_i64(), Some(1234567890));
    }

    #[test]
    fn test_null_context_returns_zero() {
        assert_eq!(vuma_read_u32(std::ptr::null(), 0), 0);
        assert_eq!(vuma_read_u64(std::ptr::null(), 0), 0);
        vuma_write_u32(std::ptr::null_mut(), 0, 42); // no-op, no crash
    }
}
