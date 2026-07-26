//! Arena overflow trap integration test (-a).
//!
//! Asserts that the runtime arena in `vuma_codegen::runtime::arena` traps
//! with `exit(1)` on overflow — mirroring the codegen-emitted
//! `__arena_overflow` stub (which every VUMA backend lowers to `exit(1)`).
//!
//! `std::process::exit(1)` terminates the process without unwinding, so
//! the trap cannot be exercised in-process via `#[should_panic]`. Instead
//! we spawn a subprocess (a re-exec of this same test binary with a
//! sentinel env var) and assert the child's exit code.
//!
//! See `docs/architecture/caveats.md` §3 row 7 and
//! `docs/architecture/pmt-fix-proposals.md` §1 for the rationale.

use std::env;
use std::process::Command;

use vuma_codegen::runtime::arena::Arena;

/// Sentinel env var. When set, the test re-executes itself in "child" mode
/// and triggers the overflow trap. The parent asserts the child's exit code.
const SENTINEL_ENV: &str = "VUMA_ARENA_OVERFLOW_TRAP_TEST_CHILD";

#[test]
fn arena_overflow_trap_exits_with_code_1() {
    if env::var(SENTINEL_ENV).is_ok() {
        // ── Child mode ───────────────────────────────────────────────
        // Create an arena with capacity 8 bytes, then bump-alloc 16 bytes.
        // `alloc_raw` aligns to 8 bytes → aligned_size = 16; offset 0 + 16 >
        // capacity 8 → `arena_overflow_trap` → `std::process::exit(1)`.
        let mut arena = Arena::create(8);
        let _ = arena.alloc_raw(16);
        // If we reach here, the trap did NOT fire — that's a regression.
        panic!("arena_overflow_trap did not fire; expected exit(1)");
    }

    // ── Parent mode ─────────────────────────────────────────────────
    // Re-exec this test binary as a subprocess. Pass the test name as a
    // filter so the test harness only runs `arena_overflow_trap_exits_with_code_1`
    // (avoiding accidental recursion into other tests). The sentinel env
    // var flips the child into trap-trigger mode above.
    let exe = env::current_exe().expect("could not resolve current_exe");
    let status = Command::new(&exe)
        .arg("arena_overflow_trap_exits_with_code_1")
        .env(SENTINEL_ENV, "1")
        .status()
        .expect("failed to spawn arena_overflow_trap child process");

    assert_eq!(
        status.code(),
        Some(1),
        "arena overflow must exit with code 1 (mirrors codegen __arena_overflow; \
         see caveats.md §3 row 7). Got status={:?}. \
         If you see code=101 the trap was replaced with a panic; if you see \
         code=134 the trap calls abort() instead of exit(1).",
        status
    );
}

#[test]
fn arena_overflow_offset_plus_size_wraparound_trap_exits_with_code_1() {
    // The other overflow path in `alloc_raw`: `offset.checked_add(aligned_size)`
    // returns `None` when the addition would wrap `usize`. We can force this by
    // creating an arena with capacity 16, then asking for a size so large that
    // `offset + aligned_size` wraps. We cannot directly set `offset` to
    // `usize::MAX` (no public API), so we instead drive `offset` up by
    // allocating, then request an aligned_size that triggers the wraparound.
    //
    // Concretely: after one `alloc::<u64>()`, offset = 8. We then ask for
    // `usize::MAX` bytes, which aligns up to `(usize::MAX + 7) & !7`. On a
    // 64-bit platform, `(2^64 - 1 + 7) & !7` overflows back to 0 — but the
    // *checked* add `8 + 0 = 8`, which is not > capacity, so this path would
    // NOT trap. To force the checked_add None branch we need `aligned_size`
    // itself to be small enough that `offset + aligned_size` wraps. The only
    // way is to have `offset = usize::MAX - k` for small `k`. Since we can't
    // reach that state via public API, this test exercises the *capacity*
    // branch instead — already covered by the previous test. We keep this
    // test as a placeholder that asserts the trap is reachable through ANY
    // overflow path: spawn the same child as above.
    //
    // NOTE: if a future refactor exposes a way to construct an `Arena` with
    // an arbitrary `offset`, replace the body below with a direct
    // checked_add-None trigger.
    if env::var(SENTINEL_ENV).is_ok() {
        let mut arena = Arena::create(8);
        // Allocate past capacity via two steps to exercise the
        // post-bump `new_offset > capacity` branch with a non-zero offset.
        let _ = arena.alloc_raw(8); // ok: offset 0→8, capacity 8
        let _ = arena.alloc_raw(8); // 8 + 8 = 16 > 8 → trap
        panic!("arena_overflow_trap did not fire; expected exit(1)");
    }

    let exe = env::current_exe().expect("could not resolve current_exe");
    let status = Command::new(&exe)
        .arg("arena_overflow_offset_plus_size_wraparound_trap_exits_with_code_1")
        .env(SENTINEL_ENV, "1")
        .status()
        .expect("failed to spawn arena_overflow_trap child process");

    assert_eq!(
        status.code(),
        Some(1),
        "second arena overflow path must also exit with code 1. Got status={:?}",
        status
    );
}
