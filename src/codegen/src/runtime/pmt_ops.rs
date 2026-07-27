//! pmt_ops.rs — Rust-side reference implementations of the 8 PMT/arena
//! externs that VUMA's codegen emits with layout-mangled names.
//!
//! # DEPRECATED (FFI Wave 1 task C)
//!
//! As of FFI Wave 1 task C (No-FFI closure), `src/pipeline.rs` no longer
//! emits `__vuma_state_*` / `__vuma_arena_*` extern calls — PMT/arena ops
//! are now inlined as first-class IR instructions via
//! `ScgStatement::PmtOp(PmtOpStmt)` and lowered to `IRInstr::Alloc` /
//! `IRInstr::Load` / `IRInstr::Store` / `IRInstr::Transform` /
//! `IRInstr::Free`. The 8 functions in this module are kept as
//! reference implementations of the runtime semantics (used by the
//! `__oob_trap` mechanism and as documentation of the original
//! stop-the-bleeding behavior from FFI-0-A), but they are no longer
//! invoked by the codegen pipeline.
//!
//! The `__oob_trap` function (used by the codegen `Transform` lowering
//! for runtime size-mismatch traps) is NOT deprecated.
//!
//! VUMA's pipeline (src/pipeline.rs) lowers StateInit / StateRead /
//! StateWrite / StateTransform / ArenaNew / ArenaAlloc / ArenaGrow /
//! ArenaFree nodes to `Call` nodes whose `func` field is one of:
//!
//!   __vuma_state_init__<L>
//!   __vuma_state_read__<L>__<f>
//!   __vuma_state_write__<L>__<f>
//!   __vuma_state_transform__<in>_to_<out>
//!   __vuma_arena_new
//!   __vuma_arena_alloc__<L>
//!   __vuma_arena_grow
//!   __vuma_arena_free
//!
//! where `<L>` is a layout name (dynamic — chosen at compile time of the
//! VUMA program) and `<f>` is a field name. The layout suffix is dynamic,
//! so we cannot use `#[no_mangle]` to define Rust symbols matching every
//! possible mangled name. Instead, this module defines 8 ordinary
//! `pub unsafe extern "C" fn` functions (`vuma_state_init`,
//! `vuma_state_read`, …) that take raw pointers and sizes — the correct
//! abstraction for the temporary stop-the-bleeding runtime.
//!
//! In standalone ET_EXEC mode, the x86_64 backend patches the mangled
//! `__vuma_state_*` / `__vuma_arena_*` externs to `__ffi_fallback_stub`
//! (`xor eax, eax; ret` → returns 0), so state reads return 0, state
//! writes are dropped, and arena allocation returns NULL. That behavior
//! is intentional and documented; this module provides the *auditable
//! Rust reference implementation* of those 8 ops so the semantics are
//! explicit rather than implicit in the codegen's "return 0" stub.
//!
//! **Wave 1 (FFI-1-C)** will inline these 8 ops as IR instructions and
//! remove the externs entirely; at that point this module will be deleted.

use super::arena::Arena;
use std::alloc::{self, Layout};
use std::collections::HashMap;
use std::sync::Mutex;

/// Per-arena metadata stored in the global [`REGISTRY`], keyed by base
/// pointer (as `usize`).
///
/// - `capacity`: total bytes in the arena's data region.
/// - `bump_offset`: next free byte offset (advanced by `vuma_arena_alloc`).
#[derive(Clone, Copy)]
struct ArenaMeta {
    capacity: u64,
    bump_offset: u64,
}

/// Global registry mapping arena base pointer (as `usize`) → metadata.
///
/// Used by `vuma_state_read` / `vuma_state_write` / `vuma_arena_alloc` /
/// `vuma_arena_grow` / `vuma_arena_free` to bounds-check offsets against
/// the arena's recorded capacity and to track the bump pointer.
///
/// Lock is held only briefly (no I/O under the lock). On poison (e.g. a
/// prior test panicked while holding the lock) the inner data is recovered
/// via `unwrap_or_else(|p| p.into_inner())` so subsequent tests still run.
static REGISTRY: Mutex<Option<HashMap<usize, ArenaMeta>>> = Mutex::new(None);

/// Acquire the REGISTRY lock, lazily initializing the HashMap on first
/// use, and recover from poison (so a panicking test doesn't cascade).
fn registry() -> std::sync::MutexGuard<'static, Option<HashMap<usize, ArenaMeta>>> {
    let mut guard = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

// ── OOB trap ──────────────────────────────────────────────────────────────

/// Trap body, cfg-gated for testability.
///
/// Production (`cfg(not(test))`): `std::process::exit(134)` (SIGABRT —
/// mirrors the codegen `__oob_trap` contract documented in
/// `pmt-formal-spec.md` §7 and `caveats.md` §3 row 7).
///
/// Tests (`cfg(test)`): `panic!("__oob_trap triggered")` so OOB code
/// paths can be exercised via `#[should_panic]`.
#[cfg(not(test))]
fn oob_trap_inner() -> ! {
    std::process::exit(134)
}

#[cfg(test)]
fn oob_trap_inner() -> ! {
    panic!("__oob_trap triggered")
}

/// Rust-side mirror of the codegen-emitted `__oob_trap` stub.
///
/// All PMT op OOB / invalid-argument paths funnel through this function.
/// In production it terminates the process with exit code 134 (SIGABRT),
/// matching the codegen contract. In tests it panics (see
/// `oob_trap_inner`) so OOB paths can be exercised with `#[should_panic]`.
///
/// `#[no_mangle]` makes the symbol available for future runtime linking
/// paths; in standalone ET_EXEC mode the codegen emits its own
/// `__oob_trap` stub and this function is simply not linked in.
///
/// ABI note: `extern "C-unwind"` (rather than `extern "C"`) is used so
/// that the test-mode panic in `oob_trap_inner` can unwind through this
/// frame and be caught by `#[should_panic]`. In production
/// (`cfg(not(test))`) the body calls `std::process::exit(134)` and never
/// unwinds, so the unwind tables are unused — the calling convention is
/// identical to plain `extern "C"`.
#[no_mangle]
pub extern "C-unwind" fn __oob_trap() -> ! {
    oob_trap_inner()
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Validate that `size` is a valid little-endian load width (1, 2, 4, or 8).
/// Traps via [`__oob_trap`] on any other value.
fn validate_size(size: u64) {
    match size {
        1 | 2 | 4 | 8 => {}
        _ => __oob_trap(),
    }
}

/// Bounds-check `offset + size <= capacity`, trapping on OOB or overflow.
fn bounds_check(offset: u64, size: u64, capacity: u64) {
    match offset.checked_add(size) {
        Some(end) if end <= capacity => {}
        _ => __oob_trap(),
    }
}

/// Look up the arena metadata for `base`, trapping if not registered.
///
/// The lock is dropped before any potential trap so a panic in test mode
/// does not poison the [`REGISTRY`] Mutex.
fn lookup_meta(base: *const u8) -> ArenaMeta {
    let key = base as usize;
    let meta = {
        let guard = registry();
        guard.as_ref().and_then(|m| m.get(&key).copied())
    };
    match meta {
        Some(m) => m,
        None => __oob_trap(),
    }
}

// ── 8 PMT op reference implementations ────────────────────────────────────

/// `__vuma_state_init__<L>` — allocate a fresh Arena of `capacity` bytes
/// via [`Arena::create`], register its `(base, capacity)` pair in the
/// global [`REGISTRY`], and return the base pointer.
///
/// Returns null on invalid layout (e.g. `capacity == 0` or capacities
/// that overflow `Layout` arithmetic). On alloc failure (OOM),
/// `Arena::create` traps via `arena_overflow_trap` (exit 1, matching
/// the codegen `__arena_overflow` contract) — that path does NOT return
/// null; the spec's "null on failure" is honored for the layout-failure
/// case only.
///
/// # Safety
/// `capacity` should be a sensible arena size (nonzero, not near
/// `usize::MAX`). The returned pointer is owned by the runtime and must
/// be freed via [`vuma_arena_free`] to avoid leaking the underlying
/// allocation.
///
/// ABI note: `extern "C-unwind"` (rather than `extern "C"`) is used so
/// that test-mode panics from [`__oob_trap`] can unwind through this
/// frame and be caught by `#[should_panic]`. Production callers (C ABI)
/// observe identical behavior — `__oob_trap` exits the process rather
/// than unwinding in `cfg(not(test))` builds.
#[deprecated(note = "PMT/arena ops are now inlined as IR instructions (see FFI-1-C); kept as reference for the runtime semantics only. Will be removed in a future wave.")]
pub unsafe extern "C-unwind" fn vuma_state_init(capacity: u64) -> *mut u8 {
    // Pre-check the layout so we can return null on invalid capacity
    // (e.g. capacity=0 or near usize::MAX). On alloc failure (OOM),
    // Arena::create traps via arena_overflow_trap (exit 1) — that path
    // does not return null.
    if capacity == 0 || Layout::from_size_align(capacity as usize, 8).is_err() {
        return std::ptr::null_mut();
    }
    let arena = Arena::create(capacity as usize);
    let base = arena.base();
    // Forget the Arena so its Drop doesn't dealloc; vuma_arena_free
    // will dealloc directly using the reconstructed Layout.
    std::mem::forget(arena);
    {
        let mut guard = registry();
        guard
            .as_mut()
            .expect("REGISTRY invariant: initialized by registry()")
            .insert(
                base as usize,
                ArenaMeta {
                    capacity,
                    bump_offset: 0,
                },
            );
    }
    base
}

/// `__vuma_state_read__<L>__<f>` — read `size` bytes (1/2/4/8) from
/// `state + offset` little-endian. Traps on OOB or invalid size.
///
/// # Safety
/// `state` must be a valid pointer returned by [`vuma_state_init`] (or
/// [`vuma_arena_new`]), and the `(offset, size)` range must lie within
/// the arena's data region.
///
/// ABI note: `extern "C-unwind"` — see [`vuma_state_init`] for rationale.
#[deprecated(note = "PMT/arena ops are now inlined as IR instructions (see FFI-1-C); kept as reference for the runtime semantics only. Will be removed in a future wave.")]
pub unsafe extern "C-unwind" fn vuma_state_read(state: *const u8, offset: u64, size: u64) -> u64 {
    if state.is_null() {
        __oob_trap();
    }
    validate_size(size);
    let meta = lookup_meta(state);
    bounds_check(offset, size, meta.capacity);
    let ptr = unsafe { state.add(offset as usize) };
    let mut buf = [0u8; 8];
    unsafe {
        std::ptr::copy_nonoverlapping(ptr, buf.as_mut_ptr(), size as usize);
    }
    u64::from_le_bytes(buf)
}

/// `__vuma_state_write__<L>__<f>` — write `size` bytes (1/2/4/8) to
/// `state + offset` little-endian. Traps on OOB or invalid size.
///
/// # Safety
/// `state` must be a valid pointer returned by [`vuma_state_init`] (or
/// [`vuma_arena_new`]), and the `(offset, size)` range must lie within
/// the arena's data region.
///
/// ABI note: `extern "C-unwind"` — see [`vuma_state_init`] for rationale.
#[deprecated(note = "PMT/arena ops are now inlined as IR instructions (see FFI-1-C); kept as reference for the runtime semantics only. Will be removed in a future wave.")]
pub unsafe extern "C-unwind" fn vuma_state_write(state: *mut u8, offset: u64, size: u64, val: u64) {
    if state.is_null() {
        __oob_trap();
    }
    validate_size(size);
    let meta = lookup_meta(state);
    bounds_check(offset, size, meta.capacity);
    let ptr = unsafe { state.add(offset as usize) };
    let buf = val.to_le_bytes();
    unsafe {
        std::ptr::copy_nonoverlapping(buf.as_ptr(), ptr, size as usize);
    }
}

/// `__vuma_state_transform__<in>_to_<out>` — in-place reinterpret.
///
/// If `from_size != to_size`, traps (the codegen should never emit a
/// transform between mismatched sizes — that would be a type error).
/// Otherwise returns `state` unchanged.
///
/// # Safety
/// `state` must be a valid pointer (the function does not dereference
/// it, but it does return it).
///
/// ABI note: `extern "C-unwind"` — see [`vuma_state_init`] for rationale.
#[deprecated(note = "PMT/arena ops are now inlined as IR instructions (see FFI-1-C); kept as reference for the runtime semantics only. Will be removed in a future wave.")]
pub unsafe extern "C-unwind" fn vuma_state_transform(
    state: *mut u8,
    from_size: u64,
    to_size: u64,
) -> *mut u8 {
    if from_size != to_size {
        __oob_trap();
    }
    state
}

/// `__vuma_arena_new` — alias for [`vuma_state_init`]. Allocates a fresh
/// Arena and returns its base pointer.
///
/// # Safety
/// See [`vuma_state_init`].
///
/// ABI note: `extern "C-unwind"` — see [`vuma_state_init`] for rationale.
#[deprecated(note = "PMT/arena ops are now inlined as IR instructions (see FFI-1-C); kept as reference for the runtime semantics only. Will be removed in a future wave.")]
pub unsafe extern "C-unwind" fn vuma_arena_new(capacity: u64) -> *mut u8 {
    #![allow(deprecated)]
    vuma_state_init(capacity)
}

/// `__vuma_arena_alloc__<L>` — bump-allocate `size` bytes with `align`
/// alignment inside the arena region.
///
/// The bump offset is stored in the global [`REGISTRY`] (per-arena
/// `ArenaMeta.bump_offset`). Returns a pointer to the allocated region.
/// Traps on OOB, overflow, invalid alignment, or unknown arena.
///
/// # Alignment
///
/// The bump pointer is first aligned up to `align`, then advanced by
/// `size`. `align` must be a power of two and ≤ 8 (matching the arena's
/// own 8-byte base alignment).
///
/// # Safety
/// `arena` must be a valid pointer returned by [`vuma_arena_new`] or
/// [`vuma_state_init`].
///
/// ABI note: `extern "C-unwind"` — see [`vuma_state_init`] for rationale.
#[deprecated(note = "PMT/arena ops are now inlined as IR instructions (see FFI-1-C); kept as reference for the runtime semantics only. Will be removed in a future wave.")]
pub unsafe extern "C-unwind" fn vuma_arena_alloc(arena: *mut u8, size: u64, align: u64) -> *mut u8 {
    if arena.is_null() {
        __oob_trap();
    }
    if align == 0 || !align.is_power_of_two() || align > 8 {
        __oob_trap();
    }
    let align_mask = align - 1;

    // Single-lock pattern: do the read+modify under the lock and return
    // an Option; the trap (if any) happens AFTER the lock is dropped so
    // a panic in test mode does not poison the Mutex.
    let result: Option<*mut u8> = {
        let mut guard = registry();
        let map = guard
            .as_mut()
            .expect("REGISTRY invariant: initialized by registry()");
        match map.get(&(arena as usize)).copied() {
            None => None,
            Some(meta) => {
                let aligned_offset = (meta.bump_offset + align_mask) & !align_mask;
                match aligned_offset.checked_add(size) {
                    Some(end) if end <= meta.capacity => {
                        map.insert(
                            arena as usize,
                            ArenaMeta {
                                capacity: meta.capacity,
                                bump_offset: end,
                            },
                        );
                        Some(unsafe { arena.add(aligned_offset as usize) })
                    }
                    _ => None,
                }
            }
        }
    };
    match result {
        Some(ptr) => ptr,
        None => __oob_trap(),
    }
}

/// `__vuma_arena_grow` — grow the arena's recorded capacity.
///
/// VUMA's `Arena` is mmap-backed and does not support in-place grow; for
/// the stop-the-bleeding runtime this function simply updates the
/// [`REGISTRY`]'s recorded capacity (the underlying memory is NOT
/// reallocated). Wave 1 (FFI-1-C) will replace this entirely.
///
/// Traps if `new_capacity < current_capacity` (shrinking is forbidden)
/// or if `arena` is not registered.
///
/// # Safety
/// `arena` must be a valid pointer returned by [`vuma_arena_new`] or
/// [`vuma_state_init`]. Callers must not access memory beyond the old
/// capacity until the arena has actually been realloc'd (which this
/// function does NOT do — see the caveat above).
///
/// ABI note: `extern "C-unwind"` — see [`vuma_state_init`] for rationale.
#[deprecated(note = "PMT/arena ops are now inlined as IR instructions (see FFI-1-C); kept as reference for the runtime semantics only. Will be removed in a future wave.")]
pub unsafe extern "C-unwind" fn vuma_arena_grow(arena: *mut u8, new_capacity: u64) -> *mut u8 {
    if arena.is_null() {
        __oob_trap();
    }
    let result: Option<()> = {
        let mut guard = registry();
        let map = guard
            .as_mut()
            .expect("REGISTRY invariant: initialized by registry()");
        match map.get(&(arena as usize)).copied() {
            None => None,
            Some(meta) => {
                if new_capacity < meta.capacity {
                    None
                } else {
                    map.insert(
                        arena as usize,
                        ArenaMeta {
                            capacity: new_capacity,
                            bump_offset: meta.bump_offset,
                        },
                    );
                    Some(())
                }
            }
        }
    };
    match result {
        Some(()) => arena,
        None => __oob_trap(),
    }
}

/// `__vuma_arena_free` — drop the arena from the [`REGISTRY`] and dealloc
/// the underlying memory. Safe no-op if `arena` is null.
///
/// # Safety
/// `arena` must be either null (no-op) or a valid pointer previously
/// returned by [`vuma_arena_new`] or [`vuma_state_init`] that has not
/// already been freed. Use-after-free is undefined behavior.
///
/// ABI note: `extern "C-unwind"` — see [`vuma_state_init`] for rationale.
#[deprecated(note = "PMT/arena ops are now inlined as IR instructions (see FFI-1-C); kept as reference for the runtime semantics only. Will be removed in a future wave.")]
pub unsafe extern "C-unwind" fn vuma_arena_free(arena: *mut u8) {
    if arena.is_null() {
        return;
    }
    let key = arena as usize;
    // Read+remove under the lock; trap outside the lock so a panic in
    // test mode does not poison the Mutex.
    let capacity: Option<u64> = {
        let mut guard = registry();
        guard
            .as_mut()
            .expect("REGISTRY invariant: initialized by registry()")
            .remove(&key)
            .map(|m| m.capacity)
    };
    match capacity {
        Some(capacity) => {
            let layout = match Layout::from_size_align(capacity as usize, 8) {
                Ok(l) => l,
                Err(_) => __oob_trap(),
            };
            unsafe { alloc::dealloc(arena, layout) };
        }
        None => __oob_trap(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_init_and_read_write() {
        let state = unsafe { vuma_state_init(64) };
        assert!(!state.is_null());
        unsafe {
            vuma_state_write(state, 0, 4, 0xdeadbeefu64);
            let val = vuma_state_read(state, 0, 4);
            assert_eq!(val, 0xdeadbeefu64);
        }
        unsafe { vuma_arena_free(state) };
    }

    #[test]
    #[should_panic(expected = "__oob_trap triggered")]
    fn test_state_read_oob_traps() {
        let state = unsafe { vuma_state_init(16) };
        // offset 14 + size 4 = 18 > 16 → trap. (Arena leaked — test
        // process exits before this matters.)
        let _ = unsafe { vuma_state_read(state, 14, 4) };
    }

    #[test]
    #[should_panic(expected = "__oob_trap triggered")]
    fn test_state_write_oob_traps() {
        let state = unsafe { vuma_state_init(16) };
        unsafe {
            vuma_state_write(state, 14, 4, 0x12345678u64);
        }
    }

    #[test]
    fn test_arena_alloc_bumps() {
        let arena = unsafe { vuma_arena_new(64) };
        assert!(!arena.is_null());
        unsafe {
            let p1 = vuma_arena_alloc(arena, 8, 8);
            let p2 = vuma_arena_alloc(arena, 8, 8);
            assert!(!p1.is_null());
            assert!(!p2.is_null());
            let diff = p2 as usize - p1 as usize;
            assert_eq!(diff, 8);
        }
        unsafe { vuma_arena_free(arena) };
    }

    #[test]
    #[should_panic(expected = "__oob_trap triggered")]
    fn test_arena_alloc_overflow_traps() {
        let arena = unsafe { vuma_arena_new(64) };
        let _ = unsafe { vuma_arena_alloc(arena, 100, 8) };
    }

    #[test]
    fn test_arena_free_no_op_on_null() {
        unsafe { vuma_arena_free(std::ptr::null_mut()) };
    }
}
