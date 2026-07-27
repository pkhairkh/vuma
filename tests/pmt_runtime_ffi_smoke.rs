//! # Wave 5-C — FFI Behavioral Smoke Test
//!
//! Companion to Wave 4-C's *structural* conformance test
//! (`tests/ffi_signature_conformance.rs`, which checks symbol *presence* via
//! `dlsym`). This file is the **behavioral** smoke test: it actually *calls*
//! one extracted Lean function of each ABI flavour through the Rust `extern
//! "C"` boundary on a tiny input and asserts the returned `u8`.
//!
//! ## What is exercised
//! Two of the seven `_prim` primitive-signature wrappers added in Wave 4-A
//! (`@[export ..._prim]` in `proof/PMT/Extraction.lean` §9):
//!
//! | test | Lean export | C ABI | input | flavour |
//! |------|-------------|-------|-------|---------|
//! | `smoke_lean_verified_capacity_check_prim` | `lean_verified_capacity_check_prim` | `(u64,u64,u64)->u8` | `(0,10,100)` valid | Group A — fully primitive |
//! | `smoke_lean_verify_state_reads_prim` | `lean_verify_state_reads_prim` | `(lean_object*,lean_object*)->u8` | empty registry + empty reads | Group B — boxed `String` args |
//!
//! The capacity test passes genuine `u64` values (no Lean runtime needed).
//! The state-reads test passes null pointers for the two boxed-`String` args:
//! this is sound ONLY against the linkage stub (which ignores its arguments);
//! against real Lean it would need `lean_mk_string`-built `lean_object*`
//! strings (see "Real-Lean path" below).
//!
//! ## Stub vs real-Lean linkage (HONEST contract)
//!
//! `build.rs` (Wave 4-D) emits the `lean_ffi_linked` cfg **only** when the
//! real `lake build` → `lean --emit-c` → `cc::Build` pipeline succeeds and
//! `lean_runtime` objects are linked. In every environment reachable today
//! that pipeline falls back to the **stub** (`proof/extracted/lean_stub.c`),
//! so `lean_ffi_linked` is NOT set and the linked symbols are the stub's
//! hardcoded returns:
//!   - capacity-style checks (`lean_verified_*_prim`) → `0` (fail-closed)
//!   - state verifiers (`lean_verify_*_prim`)         → `1` (true)
//!
//! > **When `lean_ffi_linked` cfg is set (real Lean linkage), these tests
//! > assert real Lean outputs. When the stub is used, they assert the stub's
//! > hardcoded returns.**
//!
//! Concretely:
//!   - `smoke_lean_verified_capacity_check_prim`: input `(0,10,100)` is a
//!     *valid* case (`0+10 ≤ 100`). Real Lean returns `1` (true); the stub
//!     returns `0` (fail-closed). The expected value is cfg-selected so the
//!     assertion is correct in both regimes.
//!   - `smoke_lean_verify_state_reads_prim`: empty reads vacuously pass, so
//!     real Lean returns `1`; the stub also returns `1`. (Same value in both
//!     regimes, but the test is still meaningful: it proves the FFI call
//!     itself returns a sane `u8`, not garbage.)
//!
//! ## Why `#[ignore]` under the stub
//!
//! Because `lean_ffi_linked` is unset (stub path), both tests are tagged
//! `#[cfg_attr(not(lean_ffi_linked), ignore)]`: `cargo test` skips them by
//! default so CI never reports a green tick that merely echoes a hardcoded
//! stub value. Run them explicitly with
//! `cargo test --features pmt-runtime-check --test pmt_runtime_ffi_smoke --
//! --ignored` to exercise the full Rust→C ABI→return plumbing against the
//! C-compiled stub. When real Lean is linked (`lean_ffi_linked`), the
//! `#[ignore]` drops out automatically and the tests run in CI asserting
//! real Lean outputs.
//!
//! ## Real-Lean path TODO (Wave 6)
//!
//! When `lean_ffi_linked` is genuinely set, `lean_verify_state_reads_prim`
//! must receive real boxed Lean strings (built via `lean_mk_string` from the
//! linked `lean_runtime`), not null pointers. That marshalling helper and the
//! `lean_runtime` link are Wave 6 work; until then the `lean_ffi_linked`
//! branch of the state-reads test would crash on a null dereference if it
//! ever ran. The branch is therefore currently dead code (the cfg is never
//! set) — it exists to document the intended real-Lean assertion.

// ===========================================================================
// FFI extern declarations — the two `_prim` wrappers under test.
//
// These are declared HERE (not imported from `proof/extracted/pmt_check.rs`)
// because (a) that file is not compiled into any crate today (Wave 4-B
// NEEDS_FOLLOWUP #3), and (b) a direct `extern "C"` block forces *link-time*
// symbol resolution from `liblean_extraction.a`, which is more robust than
// the `dlsym`/`.dynsym` approach used by Wave 4-C's structural test.
// ===========================================================================

#[cfg(feature = "pmt-runtime-check")]
mod lean_ffi_prim {
    use std::ffi::c_void;

    // Link the archive compiled by `build.rs` (Wave 4-D) directly from this
    // test crate. We use an explicit `#[link]` rather than relying on the
    // build script's `cargo:rustc-link-lib` directive because that directive
    // is recorded on the package's *library* target (`libvuma`, an rlib) and
    // rlibs do NOT propagate native link-libs to their dependents — so an
    // integration test linking `libvuma` would otherwise get the `-L` search
    // path but NOT the `-llean_extraction` flag, leaving the `_prim` symbols
    // undefined at link time (confirmed empirically: rust-lld reported
    // `undefined symbol: lean_verify_state_reads_prim`). The build script's
    // emitted `-L native=<OUT_DIR>` search path IS visible to this test, so
    // `liblean_extraction.a` (stub or real) is found here.
    //
    // `kind = "static"` matches the `cc::Build::compile("lean_extraction")`
    // output (a static archive); double-linking is harmless: the linker only
    // extracts archive members that satisfy undefined references.
    #[link(name = "lean_extraction", kind = "static")]
    extern "C" {
        /// `@[export lean_verified_capacity_check_prim]` — Lean
        /// `(used size capacity : UInt64) : Bool`. Fully primitive C ABI:
        /// `(uint64_t, uint64_t, uint64_t) -> uint8_t`. No `lean_runtime`
        /// needed (UInt64 unboxes to `uint64_t`).
        pub fn lean_verified_capacity_check_prim(
            used: u64,
            size: u64,
            capacity: u64,
        ) -> u8;

        /// `@[export lean_verify_state_reads_prim]` — Lean
        /// `(registry reads : String) : Bool`. String args stay boxed as
        /// `lean_object*` (`*mut c_void` here); `Bool` returns `uint8_t`.
        /// Real-Lean calls require `lean_mk_string`-built args (Wave 6).
        pub fn lean_verify_state_reads_prim(
            registry: *mut c_void,
            reads: *mut c_void,
        ) -> u8;
    }
}

// ===========================================================================
// Lean module initialiser (Wave C-1).
//
// The `_prim` externs above are called DIRECTLY by this test (NOT through
// `verification.rs::verify_pmt_via_lean`, which internally calls
// `lean_ffi::init()`). Without `initialize_PMT` the Lean runtime is
// uninitialised and even the fully-primitive `lean_verified_capacity_check_prim`
// (which takes only `u64`s, no Lean objects) SIGSEGVs: Lean's module
// initialisers register internal tables the runtime consults on every entry.
// This module mirrors `verification.rs::lean_ffi::init()` but is self-contained
// so the test does not depend on the `ive` crate internals.
//
// The linkage stub (`lean_stub.c`) does NOT export `initialize_PMT`, so the
// extern declaration AND the call are gated on `lean_ffi_linked`; under the
// stub `ensure_init()` is a no-op (the stub symbols need no runtime init).
// ===========================================================================

#[cfg(feature = "pmt-runtime-check")]
mod lean_init {
    use std::ffi::c_void;

    /// Guards exactly-once execution of `initialize_PMT` across threads.
    #[cfg(lean_ffi_linked)]
    static INIT: std::sync::Once = std::sync::Once::new();

    // `initialize_PMT` is exported by `proof/.lake/build/ir/PMT.c` (Lean's C
    // backend) and is present in `liblean_extraction.a` ONLY on the real-Lean
    // path (when `lean_ffi_linked` is set by build.rs). The linkage stub does
    // NOT define this symbol, so the extern must be gated to avoid an
    // undefined-symbol link error on the stub path.
    #[cfg(lean_ffi_linked)]
    extern "C" {
        fn initialize_PMT(builtin: u8, w: *mut c_void) -> *mut c_void;
        fn lean_mk_string(s: *const std::ffi::c_char) -> *mut c_void;
    }

    #[cfg(lean_ffi_linked)]
    pub fn str_to_lean(s: &str) -> *mut c_void {
        use std::ffi::CString;
        let sanitized: String = s.chars().map(|c| if c == '\0' { '?' } else { c }).collect();
        let c_str = CString::new(sanitized).unwrap_or_else(|_| CString::new("").unwrap());
        unsafe { lean_mk_string(c_str.as_ptr()) }
    }

    #[cfg(not(lean_ffi_linked))]
    pub fn str_to_lean(_s: &str) -> *mut c_void { std::ptr::null_mut() }

    /// Invoke `initialize_PMT` exactly once (thread-safe via `Once`) before
    /// any `_prim` extern call. Required on the real-Lean path so the Lean
    /// runtime is initialised; a no-op under the stub.
    #[cfg(lean_ffi_linked)]
    pub fn ensure_init() {
        INIT.call_once(|| {
            // `builtin = 1` => run Lean's standard builtin initialisers.
            // A null return indicates init failure (Lean convention); we
            // proceed regardless — the subsequent `_prim` call will then
            // surface the failure as a test abort rather than a silent skip.
            let _res = unsafe { initialize_PMT(1, std::ptr::null_mut()) };
        });
    }

    /// No-op stub-path variant: `lean_stub.c` neither exports
    /// `initialize_PMT` nor needs Lean runtime initialisation.
    #[cfg(not(lean_ffi_linked))]
    pub fn ensure_init() {}
}

// ===========================================================================
// Smoke tests — only compiled when the Lean archive is linked
// (`pmt-runtime-check` feature on; `build.rs` compiles `lean_stub.c` or the
// real extracted C into `liblean_extraction.a`).
// ===========================================================================

#[cfg(feature = "pmt-runtime-check")]
mod smoke {
    use super::lean_ffi_prim::{lean_verified_capacity_check_prim, lean_verify_state_reads_prim};
    use super::lean_init;
    use std::ffi::c_void;

    // The `#[ignore]` message is inlined into the cfg_attr below (the
    // `ignore` attribute requires a string literal, not a const).
    //
    // "When `lean_ffi_linked` cfg is set (real Lean linkage), these tests
    //  assert real Lean outputs. When stub is used, they assert the stub's
    //  hardcoded returns."  See the module docs above for the full contract.

    /// Behavioral smoke test #1 — call `lean_verify_state_reads_prim`
    /// (Group B, boxed-`String` ABI) with an empty registry and empty reads
    /// list, and assert the returned `u8`.
    ///
    /// Input: two null pointers (the stub ignores its arguments; this models
    /// "empty env_list, empty reads"). Real-Lean path must replace these
    /// with `lean_mk_string`-built empty Lean strings (Wave 6 TODO).
    #[test]
    #[cfg_attr(not(lean_ffi_linked), ignore = "stub-linked (lean_ffi_linked NOT set): asserts the linkage stub\'s hardcoded return, not a real Lean output. Run with `--ignored` to exercise the Rust->C ABI plumbing against the C-compiled stub. When real Lean is linked, this ignore is dropped and the test asserts the real Lean output.")]
    // C-1: under real Lean this prim takes 2x boxed lean_object* (Lean
    // String) args. We marshal empty Lean strings via lean_mk_string
    // (exposed in the lean_init module above). Empty reads vacuously
    // pass -> real Lean returns 1 (true).
    fn smoke_lean_verify_state_reads_prim() {
        // C-1: initialise the Lean runtime before any `_prim` call. Under
        // the stub (`lean_ffi_linked` NOT set) this is a no-op.
        lean_init::ensure_init();

        // Marshal empty Lean strings (real Lean) or null (stub).
        let env: *mut c_void = lean_init::str_to_lean("");
        let reads: *mut c_void = lean_init::str_to_lean("");

        // SAFETY: FFI call into the linked `liblean_extraction.a` symbol.
        // Under the stub the function ignores its arguments and returns a
        // hardcoded `u8`. Under real Lean the args must be valid boxed Lean
        // strings (Wave 6 marshalling); the real-Lean branch is dead code
        // until `lean_ffi_linked` is set.
        let result: u8 = unsafe { lean_verify_state_reads_prim(env, reads) };

        // Empty reads vacuously pass -> real Lean returns 1 (true).
        // The state-verifier stub also returns 1 (true). Same value in both
        // regimes; the test still proves the FFI call yields a sane u8.
        #[cfg(lean_ffi_linked)]
        let expected: u8 = 1; // real Lean output
        #[cfg(not(lean_ffi_linked))]
        let expected: u8 = 1; // stub: state verifier -> 1 (true)

        assert_eq!(
            result, expected,
            "lean_verify_state_reads_prim(empty, empty) returned {result}, \
             expected {expected}"
        );
    }

    /// Behavioral smoke test #2 — call `lean_verified_capacity_check_prim`
    /// (Group A, fully-primitive `u64` ABI) with a valid capacity case and
    /// assert the returned `u8`.
    ///
    /// Input: `(used=0, size=10, capacity=100)` — valid because `0+10 ≤ 100`
    /// with no overflow.
    #[test]
    #[cfg_attr(not(lean_ffi_linked), ignore = "stub-linked (lean_ffi_linked NOT set): asserts the linkage stub\'s hardcoded return, not a real Lean output. Run with `--ignored` to exercise the Rust->C ABI plumbing against the C-compiled stub. When real Lean is linked, this ignore is dropped and the test asserts the real Lean output.")]
    fn smoke_lean_verified_capacity_check_prim() {
        // C-1: initialise the Lean runtime BEFORE the `_prim` call. The
        // capacity prim takes only `u64`s (no Lean objects) but still
        // requires `initialize_PMT` to have run — Lean's module
        // initialisers register internal tables the runtime consults on
        // every entry, and without them even this primitive call
        // SIGSEGVs. Under the stub (`lean_ffi_linked` NOT set) this is a
        // no-op.
        lean_init::ensure_init();

        let (used, size, capacity): (u64, u64, u64) = (0, 10, 100);

        // SAFETY: FFI call into the linked `liblean_extraction.a` symbol.
        // Fully primitive ABI (u64 x3 -> u8); no Lean runtime involvement.
        let result: u8 =
            unsafe { lean_verified_capacity_check_prim(used, size, capacity) };

        // The input is a VALID capacity case (0+10 <= 100), so real Lean
        // returns 1 (true). The capacity-style stub is fail-closed and
        // returns 0 regardless of input — hence the cfg-selected expected
        // value and the #[ignore] on the stub path (documented honestly:
        // against the stub this asserts a hardcoded 0, not a real check).
        #[cfg(lean_ffi_linked)]
        let expected: u8 = 1; // real Lean: valid capacity -> true
        #[cfg(not(lean_ffi_linked))]
        let expected: u8 = 0; // stub: fail-closed -> 0

        assert_eq!(
            result, expected,
            "lean_verified_capacity_check_prim({used}, {size}, {capacity}) \
             returned {result}, expected {expected} \
             (stub=fail-closed-0 / real-Lean=valid-1)"
        );
    }
}
