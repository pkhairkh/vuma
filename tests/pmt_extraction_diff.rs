//! # Wave 6-B — Differential test between the two Rust copies of the PMT checkers
//!
//! **Problem (Round 7 finding):** `tests/pmt_parity_test.rs` (Copy A —
//! hand-translated `lean_*` functions) and `proof/extracted/pmt_check.rs`
//! (Copy B — extracted/API `verified_*` functions) are TWO independent Rust
//! translations of the same Lean-verified logic. The existing parity test
//! only checks each copy against hand-computed expected values — it does
//! NOT check that the two copies agree with *each other*. They can drift
//! silently.
//!
//! **This file closes that gap.** For a corpus of 30 inputs (20 edge cases
//! + 10 deterministic random-ish inputs), it runs BOTH copies and asserts
//! they produce identical `bool` output.
//!
//! ## The two copies compared
//!
//! | Lean function | Copy A (`pmt_parity_test.rs`) | Copy B (`pmt_check.rs`) |
//! |----------------|-------------------------------|------------------------|
//! | `verified_capacity_check` | `lean_capacity_check` (pub) | `verified_capacity_check` (pub) |
//! | `verified_field_bounds_check` | `lean_field_bounds_check` (pub) | `verified_field_bounds_check` (pub) |
//! | `verified_linearity_check` | `lean_linearity_check` (pub) | `verified_linearity_check` (pub) |
//! | `verified_pmt_check` (composed) | manual `&&` composition | `verified_pmt_check` (pub) |
//!
//! ## Linkage strategy
//!
//! Copy A lives in `tests/pmt_parity_test.rs` and its functions are
//! **private** (not `pub`). To enable the diff test, we added `pub` to
//! the three `lean_*` function definitions (minimal, harmless change:
//! they're in a test crate, not a library). Copy A is included via
//! `#[path]` so its public functions are reachable as `parity_test::*`.
//!
//! Copy B lives in `proof/extracted/pmt_check.rs` which is **not compiled
//! into any crate** (see Wave 4-B NEEDS_FOLLOWUP #3, documented in
//! `tests/pmt_runtime_ffi_smoke.rs`). We include it via `#[path]` so its
//! public `verified_*` functions are reachable as `pmt_check::*`.
//!
//! ## Feature gate
//!
//! The test is gated behind `#[cfg(feature = "pmt-runtime-check")]` per the
//! task spec. Copy B's file also contains a `lean_ffi` module (extern "C"
//! declarations for the 7 Lean `@[export]` symbols) that is compiled when
//! this feature is on. The stub archive `liblean_extraction.a` (compiled
//! by `build.rs` from `proof/extracted/lean_stub.c`) provides those
//! symbols. Because `cargo:rustc-link-lib` from `build.rs` does not
//! propagate to integration-test crates (rlib limitation — see
//! `pmt_runtime_ffi_smoke.rs` comment), we add an explicit
//! `#[link(name = "lean_extraction", kind = "static")]` anchor below so the
//! linker searches the stub archive and resolves `lean_ffi`'s extern
//! symbols. The stub returns hardcoded values that are never read by this
//! test — we only compare the two **pure-Rust** copies.
//!
//! ## State verifiers — NOT diffed here
//!
//! Copy A has `lean_verify_state_reads`, `lean_verify_state_writes`, and
//! `lean_verify_transform` (pure-Rust hand-translations). Copy B does NOT
//! have pure-Rust equivalents — it only has `extern "C"` FFI declarations
//! (in `lean_ffi`) that call the Lean-compiled C code. Comparing Copy A's
//! pure-Rust verifiers against Copy B's FFI calls would require real Lean
//! linkage (not the stub) and Lean-object marshalling (Wave 6 TODO). This
//! diff test therefore focuses on the 3 PMT-level checks + the composed
//! check, where both copies have pure-Rust implementations.

#![cfg(feature = "pmt-runtime-check")]

// ===========================================================================
// Copy B — proof/extracted/pmt_check.rs (public `verified_*` functions).
// Included via #[path] because this file is not part of any crate.
// ===========================================================================
#[path = "../proof/extracted/pmt_check.rs"]
mod pmt_check;

// ===========================================================================
// Copy A — tests/pmt_parity_test.rs (public `lean_*` functions).
// Included via #[path]. NOTE: This also pulls in Copy A's
// `#[cfg(test)] mod tests` block (~30 parity tests). Since `cargo test`
// sets `cfg(test)`, those tests also run as part of this binary. They are
// harmless (they already pass in pmt_parity_test.rs) and serve as
// additional regression coverage.
// ===========================================================================
#[path = "pmt_parity_test.rs"]
mod parity_test;

// ===========================================================================
// Link anchor — ensure `liblean_extraction.a` (the stub) is searched by
// the linker so that `pmt_check::lean_ffi`'s `extern "C"` declarations
// resolve. Without this, the build.rs `cargo:rustc-link-lib` directive
// (which targets the library crate, not integration tests) does not
// propagate, causing "undefined symbol" link errors for the 7 Lean export
// symbols. The anchor function is never called; it exists solely to
// attach the `#[link]` attribute. See `pmt_runtime_ffi_smoke.rs` for the
// same pattern.
// ===========================================================================
#[link(name = "lean_extraction", kind = "static")]
extern "C" {
    #[link_name = "lean_verify_state_writes"]
    fn _diff_link_anchor() -> u8;
}

// ===========================================================================
// Corpus — 30 inputs total (20 edge cases + 10 deterministic random-ish)
// ===========================================================================

/// Capacity-check corpus: `(used, size, capacity)`.
/// 8 edge cases + 4 random-ish = 12 entries.
const CAPACITY_INPUTS: &[(u64, u64, u64)] = &[
    // --- edge cases ---
    (0, 0, 0),                          // EC1: all-zero / empty
    (0, 0, u64::MAX),                   // EC2: max-cap, zero usage
    (u64::MAX, 0, u64::MAX),            // EC3: max-used, zero-size
    (u64::MAX, 1, u64::MAX),            // EC4: overflow (MAX+1 wraps)
    (u64::MAX, u64::MAX, u64::MAX),     // EC5: overflow (MAX+MAX wraps)
    (1, u64::MAX, 0),                   // EC6: overflow into zero-cap
    (1000, 24, 1024),                   // EC7: exact fit (1000+24=1024)
    (1024, 1, 1024),                    // EC8: one byte over
    // --- random-ish ---
    (42, 58, 100),                      // R1: sums exactly to cap
    (999, 1, 1000),                     // R2: one below cap
    (31337, 1337, 100000),              // R3: mid-range
    (65535, 1, 65536),                  // R4: power-of-two boundary
];

/// Field-bounds-check corpus: `(offset, size, total)`.
/// 7 edge cases + 3 random-ish = 10 entries.
const FIELD_BOUNDS_INPUTS: &[(u64, u64, u64)] = &[
    // --- edge cases ---
    (0, 0, 0),                          // EC9: all-zero
    (0, 0, 1),                          // EC10: zero-size field in size-1 layout
    (u64::MAX, 1, u64::MAX),            // EC11: overflow (MAX+1)
    (u64::MAX, u64::MAX, 0),            // EC12: overflow into zero-total
    (8, 8, 16),                         // EC13: exact fit (8+8=16)
    (12, 8, 16),                        // EC14: one byte over (12+8=20>16)
    (0, u64::MAX, u64::MAX),            // EC15: max field in max layout (exact)
    // --- random-ish ---
    (7, 9, 16),                         // R5: exact fit
    (0, 4, 8),                          // R6: small field, small layout
    (255, 1, 256),                      // R7: byte boundary
];

/// Linearity-check corpus: `(var, consumed)`.
/// 5 edge cases + 3 random-ish = 8 entries.
const LINEARITY_INPUTS: &[(&str, &[&str])] = &[
    // --- edge cases ---
    ("x", &[]),                         // EC16: empty consumed list
    ("x", &["x"]),                      // EC17: UAF — var is consumed
    ("", &[""]),                        // EC18: empty-string var, consumed
    ("", &[]),                          // EC19: empty-string var, empty consumed
    ("x", &["a", "b", "c", "d", "e", "f", "g", "h"]), // EC20: long list, var absent
    // --- random-ish ---
    ("foo", &["bar", "baz"]),           // R8: var absent
    ("key", &["lock", "key", "door"]),  // R9: var present (consumed)
    ("data", &[]),                      // R10: var absent, empty consumed
];

/// Composed PMT-check corpus: `(used, capacity, offset, size, total, var, consumed)`.
/// Reuses edge-case combinations of the 3 sub-checks. 5 entries (subset
/// of the 30, testing the composed `verified_pmt_check` path).
const PMT_COMPOSED_INPUTS: &[(u64, u64, u64, u64, u64, &str, &[&str])] = &[
    // All pass: cap ok, bounds ok, linearity ok
    (0, 1024, 0, 4, 16, "x", &["a", "b"]),
    // Cap fails (overflow)
    (u64::MAX, 1024, 0, 4, 16, "x", &[]),
    // Bounds fails (OOB)
    (0, 1024, 12, 8, 16, "x", &[]),
    // Linearity fails (UAF — var consumed)
    (0, 1024, 0, 4, 16, "x", &["x"]),
    // All zero/empty (boundary)
    (0, 0, 0, 0, 0, "", &[]),
];

// ===========================================================================
// Differential tests — 35 assertions across 4 test functions
// ===========================================================================

/// Differential test for `capacity_check`: Copy A (`lean_capacity_check`)
/// vs Copy B (`verified_capacity_check`). Both should return the same
/// `bool` for every input in `CAPACITY_INPUTS`.
#[test]
fn diff_capacity_check() {
    let mut count = 0;
    for &(used, size, cap) in CAPACITY_INPUTS {
        let copy_a = parity_test::lean_capacity_check(used, size, cap);
        let copy_b = pmt_check::verified_capacity_check(used, size, cap);
        assert_eq!(
            copy_a, copy_b,
            "capacity_check DRIFT at ({used}, {size}, {cap}): \
             parity_test={copy_a} pmt_check={copy_b}"
        );
        count += 1;
    }
    assert_eq!(count, 12, "expected 12 capacity inputs");
}

/// Differential test for `field_bounds_check`: Copy A
/// (`lean_field_bounds_check`) vs Copy B (`verified_field_bounds_check`).
#[test]
fn diff_field_bounds_check() {
    let mut count = 0;
    for &(offset, size, total) in FIELD_BOUNDS_INPUTS {
        let copy_a = parity_test::lean_field_bounds_check(offset, size, total);
        let copy_b = pmt_check::verified_field_bounds_check(offset, size, total);
        assert_eq!(
            copy_a, copy_b,
            "field_bounds_check DRIFT at ({offset}, {size}, {total}): \
             parity_test={copy_a} pmt_check={copy_b}"
        );
        count += 1;
    }
    assert_eq!(count, 10, "expected 10 field-bounds inputs");
}

/// Differential test for `linearity_check`: Copy A
/// (`lean_linearity_check`) vs Copy B (`verified_linearity_check`).
#[test]
fn diff_linearity_check() {
    let mut count = 0;
    for &(var, consumed) in LINEARITY_INPUTS {
        let copy_a = parity_test::lean_linearity_check(var, consumed);
        let copy_b = pmt_check::verified_linearity_check(var, consumed);
        assert_eq!(
            copy_a, copy_b,
            "linearity_check DRIFT at (\"{var}\", {consumed:?}): \
             parity_test={copy_a} pmt_check={copy_b}"
        );
        count += 1;
    }
    assert_eq!(count, 8, "expected 8 linearity inputs");
}

/// Differential test for the composed PMT check. Copy A composes the
/// three sub-checks manually with `&&`; Copy B uses `verified_pmt_check`.
/// Note the argument-order subtlety: `verified_pmt_check(used, capacity,
/// offset, size, total, ...)` internally calls
/// `verified_capacity_check(used, total, capacity)` (i.e. `total` as the
/// "size" argument), checking that the layout fits within capacity. Copy
/// A's manual composition mirrors this exactly.
#[test]
fn diff_pmt_check_composed() {
    for &(used, capacity, offset, size, total, var, consumed) in PMT_COMPOSED_INPUTS {
        // Copy A: manual composition (mirrors pmt_parity_test.rs pattern)
        let copy_a = parity_test::lean_capacity_check(used, total, capacity)
            && parity_test::lean_field_bounds_check(offset, size, total)
            && parity_test::lean_linearity_check(var, consumed);
        // Copy B: composed function
        let copy_b = pmt_check::verified_pmt_check(
            used, capacity, offset, size, total, var, consumed,
        );
        assert_eq!(
            copy_a, copy_b,
            "pmt_check DRIFT at (used={used}, cap={capacity}, off={offset}, \
             sz={size}, total={total}, var=\"{var}\", consumed={consumed:?}): \
             parity_test={copy_a} pmt_check={copy_b}"
        );
    }
}

/// Sanity check: verify the link anchor compiles and the stub archive is
/// linked (i.e. the `lean_ffi` extern symbols resolve). If this test
/// compiles, the linkage strategy works.
#[test]
fn link_anchor_compiles() {
    // If this function compiles and the binary links, the #[link] anchor
    // successfully pulled in liblean_extraction.a and resolved lean_ffi's
    // extern declarations. We do NOT call _diff_link_anchor (it maps to
    // the stub's lean_verify_state_writes which ignores its args and
    // returns 1 — calling it would be safe but pointless). The mere fact
    // that the binary linked is the assertion.
    let _ = std::mem::size_of::<fn() -> u8>();
}
