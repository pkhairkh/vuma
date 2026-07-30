//! Arena allocator — Rust-level runtime for testing and callback path.
//!
//! This module provides a Rust-level arena allocator that mirrors the
//! codegen-lowered arena builtins (arena_new/arena_alloc/arena_grow/arena_free).
//! It's used for:
//!   - Unit testing the arena model
//!   - The vuma_context callback path
//!
//! The arena is a bump allocator backed by mmap. No per-object malloc/free.
//! arena_alloc bumps an offset; arena_free unmaps the whole region.

//! ## Guard page (Stage 5 hardening — TODO)
//!
//! The intended design places an `mprotect(PROT_NONE)` page immediately
//! after the data region so that bump overflows SIGSEGV rather than
//! silently corrupt adjacent memory. Implementing this safely requires
//! an mmap-backed arena (the system `alloc::alloc` allocator does not
//! guarantee a free page after the returned region, so calling mprotect
//! on it can clobber adjacent heap metadata and SIGSEGV on dealloc).
//! The bounded check in `alloc_raw` is the primary defense today; the
//! mmap-backed guard page is tracked as a follow-up. See INV-PMT-1.
//!
//! ## Trap semantics
//!
//! All arena fault paths (overflow, OOM, invalid layout) terminate the
//! process via [`arena_overflow_trap`], which calls `std::process::exit(1)`.
//! This **mirrors the codegen-emitted `__arena_overflow` stub** that every
//! VUMA backend lowers to `exit(1)` (e.g. x86_64 `sys_exit` with code=1,
//! aarch64 `svc #0` with X8=93/X0=1, …). Previously this module used
//! `std::process::abort()` (SIGABRT, exit 134), which diverged from the
//! codegen trap contract documented in `pmt-formal-spec.md` §7 and
//! `caveats.md` §3 row 7. Aligning the runtime and codegen traps keeps the
//! Iris spec (`wp (call __arena_overflow) { _, False }`) faithful to both
//! execution paths. See also `pmt-fix-proposals.md` §1.

use std::alloc::{self, Layout};
use std::thread::ThreadId;

// When the `pmt-runtime-check` feature is enabled, the Lean-verified PMT
// capacity checker (`super::pmt_check::verified_capacity_check`) replaces
// the hand-written overflow check in `alloc_raw` below. The module itself
// is `#[cfg(feature = "pmt-runtime-check")]` in `runtime/mod.rs`, so the
// import must be gated identically or the file fails to compile with the
// feature off.
#[cfg(feature = "pmt-runtime-check")]
use super::pmt_check;

/// An arena allocator backed by a single mmap'd region.
///
/// # Thread-safety invariant
///
/// `Arena` is **not** `Send` and **not** `Sync`. The arena owns a raw
/// pointer (`base`) into the system heap and mutates interior state
/// (`offset`, `capacity`) without any synchronization. Sending it across
/// threads or sharing it concurrently would be a data race.
///
/// By convention the arena is single-threaded: only the thread that
/// constructed it (`Arena::create`) is permitted to call `alloc_raw`,
/// `grow`, `destroy`, or observe its fields. This invariant was previously
/// "by convention only" (caveats.md §3); it is now enforced at runtime in
/// debug builds via a `ThreadId` check (`debug_assert!`) stored at
/// construction time. In release builds the check is elided — callers
/// remain responsible for honoring the single-thread contract.
///
/// If cross-thread usage is genuinely required in the future, wrap the
/// arena in `Arc<Mutex<Arena>>` (which serializes access) or re-design
/// the allocator to be lock-free. Do **not** restore `unsafe impl Send`.
pub struct Arena {
    /// Base pointer of the mmap'd region.
    base: *mut u8,
    /// Current bump-alloc offset.
    offset: usize,
    /// Total capacity in bytes.
    capacity: usize,
    /// Layout used for the data region (cached so dealloc never re-derives
    /// it, which previously called `.expect()` and could panic).
    layout: Layout,
    /// `ThreadId` of the thread that constructed this arena. Used by the
    /// `assert_owner_thread` helper below to enforce the single-thread
    /// invariant at runtime in debug builds (caveats.md §3).
    created_thread: ThreadId,
}

// Note: no `unsafe impl Send` / `unsafe impl Sync` here. `*mut u8` makes
// `Arena` auto-`!Send` and `!Sync`, which is the correct default. The
// single-thread invariant is enforced additionally by `debug_assert!` in
// every public method (see `assert_owner_thread`). See caveats.md §3.

/// Graceful trap — mirrors the codegen-emitted `__arena_overflow` stub.
///
/// All arena fault paths (overflow, OOM, invalid layout) funnel through
/// this helper so that the runtime arena terminates the process the same
/// way the compiled VUMA program would on an arena overflow: `exit(1)`.
/// This is the userspace analogue of the per-backend `__arena_overflow`
/// syscall stubs (see `codegen/src/x86_64/mod.rs:3646`, `backend.rs:3392`,
/// et al.).
///
/// `exit(1)` is chosen over `std::process::abort()` (SIGABRT, exit 134)
/// because (a) the codegen contract in `pmt-formal-spec.md` §7 specifies
/// exit code 1 for `__arena_overflow`, and (b) `exit` does not raise a
/// signal — safer in kernel/embedded contexts where signal delivery is
/// not guaranteed. The process still does NOT unwind (no Drop), so this
/// is safe to call across FFI / `extern "C"` boundaries.
///
/// See `caveats.md` §3 row 7 and `pmt-fix-proposals.md` §1 for the
/// rationale and prior art.
fn arena_overflow_trap(msg: &str) -> ! {
    eprintln!(
        "vuma arena: {} — trapping (exit 1, mirrors __arena_overflow)",
        msg
    );
    std::process::exit(1);
}

/// Build a layout for the data region; traps (rather than panicking) on
/// invalid size/align.
fn layout_for(capacity: usize) -> Layout {
    Layout::from_size_align(capacity, 8).unwrap_or_else(|_| {
        arena_overflow_trap(&format!("invalid arena layout (capacity={})", capacity))
    })
}

// NOTE (Stage 5): a full mmap-backed guard page is deferred — see module
// doc comment. The bounded check in `alloc_raw` is the primary defense.

impl Arena {
    /// In debug builds, assert that the calling thread is the same thread
    /// that constructed this arena. In release builds this is a no-op;
    /// callers remain responsible for honoring the single-thread contract
    /// (caveats.md §3).
    #[inline]
    fn assert_owner_thread(&self) {
        debug_assert!(
            std::thread::current().id() == self.created_thread,
            "vuma arena: accessed from a thread other than its creator \
             (single-thread invariant violated; see caveats.md §3)"
        );
    }

    /// Create a new arena with the given initial capacity (bytes).
    /// Uses the system allocator (mmap under the hood on most platforms).
    ///
    /// TODO (Stage 5 followup): replace with a direct `mmap` allocation of
    /// `capacity + page_size` and `mprotect(PROT_NONE)` the trailing page
    /// to install a real guard page. The current system-allocator path
    /// cannot safely install a guard page (see module docs).
    pub fn create(capacity: usize) -> Self {
        let layout = layout_for(capacity);
        let base = unsafe { alloc::alloc(layout) };
        if base.is_null() {
            arena_overflow_trap(&format!(
                "arena_create: allocation failed for {} bytes",
                capacity
            ));
        }
        Arena {
            base,
            offset: 0,
            capacity,
            layout,
            created_thread: std::thread::current().id(),
        }
    }

    /// Bump-allocate `size` bytes within the arena. Returns a pointer to
    /// the allocated region. Traps via `__arena_overflow` semantics
    /// (exit 1, does not unwind) if the arena is full or the offset+size
    /// computation overflows `usize`.
    ///
    /// # Panics (debug builds only)
    ///
    /// Panics if invoked from a thread other than the arena's creator —
    /// see `assert_owner_thread` and caveats.md §3.
    pub fn alloc_raw(&mut self, size: usize) -> *mut u8 {
        self.assert_owner_thread();
        let aligned_size = (size + 7) & !7; // align to 8 bytes

        // ── pmt-runtime-check feature wiring ───────────────────────
        //
        // When `pmt-runtime-check` is enabled, the Lean-verified capacity
        // checker (`proof/PMT/Extraction.lean` → `pmt_check.rs`) runs in
        // place of the hand-written `checked_add` + `> capacity` pair.
        // `verified_capacity_check` uses `checked_add` internally and
        // returns `true` iff `used + size ≤ capacity`, so it subsumes BOTH
        // the arithmetic overflow check AND the capacity overflow check in
        // one Lean-verified call. The parity test
        // (`tests/pmt_parity_test.rs`) already establishes the Rust matches
        // the Lean semantics on all test cases.
        //
        // When the feature is OFF, the original hand-written check runs
        // unchanged so existing tests and behavior are bit-for-bit
        // identical.

        #[cfg(feature = "pmt-runtime-check")]
        {
            if !pmt_check::verified_capacity_check(
                self.offset as u64,
                aligned_size as u64,
                self.capacity as u64,
            ) {
                arena_overflow_trap(&format!(
                    "arena_alloc: overflow (offset={}, size={}, capacity={})",
                    self.offset, aligned_size, self.capacity
                ));
            }
            // Recompute `new_offset` via `wrapping_add` — safe because the
            // verified check above has already established
            // `offset + aligned_size ≤ capacity ≤ usize::MAX` on 64-bit
            // targets (the arena runtime is documented as 64-bit-biased;
            // the `as u64` cast is lossless there). The `usize`-typed
            // addition therefore cannot overflow.
            let new_offset = self.offset.wrapping_add(aligned_size);
            let ptr = unsafe { self.base.add(self.offset) };
            self.offset = new_offset;
            ptr
        }

        #[cfg(not(feature = "pmt-runtime-check"))]
        {
            let new_offset = match self.offset.checked_add(aligned_size) {
                Some(v) => v,
                None => {
                    arena_overflow_trap(&format!(
                        "arena_alloc: offset+size overflow (offset={}, size={})",
                        self.offset, aligned_size
                    ));
                }
            };
            if new_offset > self.capacity {
                arena_overflow_trap(&format!(
                    "arena_alloc: overflow (offset={}, size={}, capacity={})",
                    self.offset, aligned_size, self.capacity
                ));
            }
            let ptr = unsafe { self.base.add(self.offset) };
            self.offset = new_offset;
            ptr
        }
    }

    /// Bump-allocate a typed value within the arena. Returns a typed pointer.
    /// The caller is responsible for initializing the memory.
    pub fn alloc<T>(&mut self) -> *mut T {
        self.alloc_raw(std::mem::size_of::<T>()) as *mut T
    }

    /// Grow the arena to at least `min_capacity` bytes. Uses realloc.
    /// Existing allocations remain valid (realloc preserves contents).
    ///
    /// # Panics (debug builds only)
    ///
    /// Panics if invoked from a thread other than the arena's creator.
    pub fn grow(&mut self, min_capacity: usize) {
        self.assert_owner_thread();
        if min_capacity <= self.capacity {
            return;
        }
        let old_layout = self.layout;
        let new_layout = Layout::from_size_align(min_capacity, 8)
            .unwrap_or_else(|_| arena_overflow_trap("arena_grow: invalid new layout"));
        let new_base = unsafe { alloc::realloc(self.base, old_layout, min_capacity) };
        if new_base.is_null() {
            arena_overflow_trap(&format!(
                "arena_grow: realloc failed for {} bytes",
                min_capacity
            ));
        }
        self.base = new_base;
        self.capacity = min_capacity;
        self.layout = new_layout;
    }

    /// Destroy the arena, unmapping all memory. No per-object free.
    /// Consumes self to prevent double-free with Drop.
    ///
    /// # Panics (debug builds only)
    ///
    /// Panics if invoked from a thread other than the arena's creator.
    pub fn destroy(self) {
        self.assert_owner_thread();
        unsafe { alloc::dealloc(self.base, self.layout) }
        // Prevent Drop from running (which would double-free)
        std::mem::forget(self);
    }

    /// Returns the current offset (bytes used).
    ///
    /// # Panics (debug builds only)
    ///
    /// Panics if invoked from a thread other than the arena's creator.
    pub fn used(&self) -> usize {
        self.assert_owner_thread();
        self.offset
    }

    /// Returns the total capacity.
    ///
    /// # Panics (debug builds only)
    ///
    /// Panics if invoked from a thread other than the arena's creator.
    pub fn capacity(&self) -> usize {
        self.assert_owner_thread();
        self.capacity
    }

    /// Returns the base pointer.
    ///
    /// # Panics (debug builds only)
    ///
    /// Panics if invoked from a thread other than the arena's creator.
    pub fn base(&self) -> *mut u8 {
        self.assert_owner_thread();
        self.base
    }

    /// Test-only setter for the bump offset. Used by
    /// `arena_proof_model::tests::mirror_matches_real_alloc_exhaustive_small`
    /// to position the arena at a specific `used` offset before calling
    /// `alloc_raw`, so the mirror can be cross-checked against the real
    /// allocator across a matrix of (capacity, used, size) combinations
    /// without first having to perform `used / 8` real allocations to
    /// reach the desired offset.
    #[cfg(test)]
    pub fn set_offset_for_testing(&mut self, o: usize) {
        self.offset = o;
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        // Skip the thread-id check if we are already unwinding from a
        // panic. A second panic during unwinding would abort the process
        // and mask the original error message (e.g. the debug_assert!
        // message a test is checking for). The dealloc still runs in
        // either case — leaking the region would be a worse failure mode
        // than running dealloc from the "wrong" thread.
        if !std::thread::panicking() {
            self.assert_owner_thread();
        }
        unsafe { alloc::dealloc(self.base, self.layout) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arena_create() {
        let arena = Arena::create(4096);
        assert_eq!(arena.capacity(), 4096);
        assert_eq!(arena.used(), 0);
    }

    #[test]
    fn test_arena_alloc() {
        let mut arena = Arena::create(4096);
        let ptr: *mut u64 = arena.alloc::<u64>();
        assert!(!ptr.is_null());
        unsafe {
            *ptr = 42;
        }
        assert_eq!(unsafe { *ptr }, 42);
        assert_eq!(arena.used(), 8);
    }

    #[test]
    fn test_arena_multiple_alloc() {
        let mut arena = Arena::create(4096);
        let p1: *mut u64 = arena.alloc::<u64>();
        let p2: *mut u64 = arena.alloc::<u64>();
        let p3: *mut u64 = arena.alloc::<u64>();
        unsafe {
            *p1 = 10;
            *p2 = 20;
            *p3 = 30;
            assert_eq!(*p1, 10);
            assert_eq!(*p2, 20);
            assert_eq!(*p3, 30);
        }
        assert_eq!(arena.used(), 24); // 3 × 8 bytes
    }

    #[test]
    fn test_arena_grow() {
        let mut arena = Arena::create(64);
        // Fill the arena
        for _ in 0..7 {
            arena.alloc::<u64>();
        }
        assert_eq!(arena.used(), 56);
        // Grow to 4096
        arena.grow(4096);
        assert_eq!(arena.capacity(), 4096);
        // Allocate more
        let p: *mut u64 = arena.alloc::<u64>();
        unsafe {
            *p = 99;
        }
        assert_eq!(unsafe { *p }, 99);
    }

    // NOTE: The previous `test_arena_overflow` test asserted a panic on
    // overflow. Overflow now calls `arena_overflow_trap` (exit 1, mirroring
    // codegen `__arena_overflow`), which terminates the process without
    // unwinding — so it cannot be tested in-process with `#[should_panic]`.
    // The integration test `tests/arena_overflow_trap_tests.rs` spawns a
    // subprocess and asserts exit code 1.

    #[test]
    fn test_arena_destroy() {
        let arena = Arena::create(4096);
        arena.destroy(); // Should not panic
    }

    /// Verify the `debug_assert!` thread-id check fires when the arena is
    /// accessed from a thread other than its creator (caveats.md §3). The
    /// check is only present in debug builds, so the test is a no-op in
    /// release.
    ///
    /// `Arena` is `!Send` + `!Sync` by design, so `thread::spawn` cannot
    /// move it directly. To exercise the *runtime* check (as opposed to
    /// the static type-system check, which we also get for free), we
    /// obtain a raw `*const Arena` pointer, wrap it in a `Send` newtype,
    /// and unsafely dereference it on a worker thread. This is sound in
    /// this test because (a) the main thread is blocked on `join()` so
    /// there is no concurrent access (no data race), and (b) the
    /// `debug_assert!` fires before any field is actually read.
    #[test]
    fn test_arena_wrong_thread_panics() {
        if !cfg!(debug_assertions) {
            eprintln!("skipped: debug_assertions disabled in release build");
            return;
        }
        let arena = Arena::create(4096);
        // SAFETY wrapper: raw pointers are not `Send` by default, but we
        // promise to only dereference the pointer while the main thread
        // is blocked on `join()` (no concurrent access).
        //
        // The `with_ref` indirection is necessary because Rust 2021's
        // disjoint closure captures would otherwise capture `ptr.0` (the
        // raw `*const Arena`, which is `!Send`) rather than `ptr` (the
        // `SendPtr` newtype, which is `Send` via the unsafe impl below).
        struct SendPtr(*const Arena);
        unsafe impl Send for SendPtr {}
        impl SendPtr {
            fn with_ref<R>(self, f: impl FnOnce(&Arena) -> R) -> R {
                // SAFETY: caller (the test) guarantees the main thread is
                // blocked on `join()` so there is no concurrent access.
                unsafe { f(&*self.0) }
            }
        }
        let ptr = SendPtr(&arena as *const Arena);
        let handle = std::thread::spawn(move || {
            ptr.with_ref(|arena_ref| {
                // Should panic via debug_assert! in `assert_owner_thread`
                // before any field is read.
                let _ = arena_ref.used();
            })
        });
        let err = handle
            .join()
            .expect_err("worker thread should have panicked via debug_assert!");
        let msg = err
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| err.downcast_ref::<&'static str>().copied())
            .unwrap_or("<non-string panic payload>");
        assert!(
            msg.contains("single-thread invariant"),
            "debug_assert! message did not mention single-thread invariant; got: {}",
            msg
        );
        // `arena` is dropped here on the main thread (its creator); Drop's
        // assert_owner_thread passes because we are not unwinding.
    }

    /// Same as above but exercises a `&mut self` method (`alloc_raw`),
    /// confirming the check fires on the write path too.
    #[test]
    fn test_arena_wrong_thread_alloc_panics() {
        if !cfg!(debug_assertions) {
            eprintln!("skipped: debug_assertions disabled in release build");
            return;
        }
        let mut arena = Arena::create(4096);
        struct SendMutPtr(*mut Arena);
        unsafe impl Send for SendMutPtr {}
        impl SendMutPtr {
            fn with_mut<R>(self, f: impl FnOnce(&mut Arena) -> R) -> R {
                // SAFETY: caller (the test) guarantees the main thread is
                // blocked on `join()` so there is no concurrent access.
                unsafe { f(&mut *self.0) }
            }
        }
        let ptr = SendMutPtr(&mut arena as *mut Arena);
        let handle = std::thread::spawn(move || {
            ptr.with_mut(|arena_ref| {
                // Should panic via debug_assert! in `assert_owner_thread`
                // before any field is mutated.
                let _ = arena_ref.alloc_raw(16);
            })
        });
        let err = handle
            .join()
            .expect_err("worker thread should have panicked via debug_assert!");
        let msg = err
            .downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| err.downcast_ref::<&'static str>().copied())
            .unwrap_or("<non-string panic payload>");
        assert!(
            msg.contains("single-thread invariant"),
            "debug_assert! message did not mention single-thread invariant; got: {}",
            msg
        );
    }

    // ── Negative-path test ───────────────────────────────────────────
    //
    // The arena's fault paths (`overflow`, `OOM`, `invalid layout`) all
    // funnel through `arena_overflow_trap` (line 98), which calls
    // `std::process::exit(1)`.  Because `exit(1)` terminates the
    // process without unwinding, the trap CANNOT be tested in-process
    // via `#[should_panic]` — see the comment at line 331 above and
    // the integration test `tests/arena_overflow_trap_tests.rs` that
    // uses subprocess re-exec to assert exit code 1.
    //
    // This test verifies the *precondition* for the trap firing in
    // `layout_for` (line 105): that `Layout::from_size_align(usize::MAX, 8)`
    // returns `Err`.  If a future Rust stdlib change (or a wrapper
    // refactor) caused this to return `Ok`, the `unwrap_or_else` arm
    // in `layout_for` would no longer fire, silently masking a
    // regression in the trap contract.

    /// `Layout::from_size_align(usize::MAX, 8)` must return `Err` —
    /// this is the precondition for `arena_overflow_trap` firing
    /// inside `layout_for` when `Arena::create(usize::MAX)` is
    /// called.  The actual exit-code-1 assertion lives in
    /// `tests/arena_overflow_trap_tests.rs` (subprocess re-exec).
    #[test]
    fn test_negative_arena_create_max_size_would_trigger_overflow_trap() {
        let layout_result = Layout::from_size_align(usize::MAX, 8);
        assert!(
            layout_result.is_err(),
            "Layout::from_size_align(usize::MAX, 8) must return Err — if it \
             ever returns Ok, the `unwrap_or_else` arm in `layout_for` (line \
             106) would no longer fire and `Arena::create(usize::MAX)` would \
             silently succeed instead of trapping, violating the \
             `__arena_overflow` exit-1 contract documented in \
             `caveats.md §3 row 7`"
        );
    }
}
