//! # Wave 4-C — FFI Signature Conformance Test (STRUCTURAL)
//!
//! This is a **structural** test, not a behavioral one. It enforces that the
//! 7 Lean-exported FFI symbols named in `FFI_BRIDGE_PLAN.md` §1 are *present*
//! (resolvable by name) in the test binary after the Lean archive is linked by
//! `build.rs` (Wave 5-A). It does **not** call them or check return values —
//! behavioral parity is Wave 6's job (`tests/pmt_parity_test.rs`).
//!
//! ## Why this exists
//! Round 7 found that `tests/pmt_parity_test.rs` defines its own
//! hand-translated `lean_verify_*` duplicates that **shadow** the real
//! `extern "C"` block in `pmt_check.rs`. Nothing currently *forces* the Lean
//! exports and the Rust externs to agree on a name. This test closes that gap:
//! if the Lean extraction ever renames / drops an export, or `build.rs` fails
//! to link it, this test fails with a per-symbol report.
//!
//! ## Mechanism
//! Uses `dlsym(RTLD_DEFAULT, name)` to resolve each symbol at runtime. No
//! `libc` dev-dependency is introduced (Wave 5-B owns `Cargo.toml`); the
//! `dlsym`/`dlerror` `extern "C"` declarations are declared inline below.
//!
//! ## Feature gating
//! When `pmt-runtime-check` is OFF (the default), the Lean archive is never
//! linked, so a structural check would be meaningless. In that configuration
//! the test compiles to a no-op that prints a skip message and returns Ok.
//! The real check only runs under `--features pmt-runtime-check`.
//!
//! ## Known follow-up (Wave 5)
//! `dlsym` only searches the dynamic symbol table (`.dynsym`). Statically
//! linked symbols land in `.symtab` and are invisible to `dlsym` unless the
//! binary exports them dynamically (`-Wl,--export-dynamic` / `-rdynamic`) or
//! the symbols are referenced via a Rust `extern "C"` block. Until Wave 5-A
//! links the archive and Wave 5-B ensures dynamic export (or relaxes this to a
//! link-time check), this test is expected to report all 7 as missing — that
//! is the BLOCKED state acknowledged by the task spec.

// ===========================================================================
// Expected symbol table — names + documented signatures.
// Source of truth: FFI_BRIDGE_PLAN.md §1 (cross-checked against
// `src/codegen/src/runtime/pmt_check.rs` and `tests/pmt_parity_test.rs`).
// Kept ungated so it doubles as always-compiled living documentation and so
// the no-op skip path can report the expected count.
// ===========================================================================

/// 1. `verified_capacity_check` — Lean `Nat -> Nat -> Nat -> Bool`
///    (capacity/size invariant). Mirrors
///    `verified_capacity_check_correct` in `proof/PMT/Extraction.lean`.
///
/// Expected Rust extern signature:
/// ```ignore
/// extern "C" transform verified_capacity_check(used: u64, size: u64, capacity: u64) -> u8;
/// ```
const SYM_VERIFIED_CAPACITY_CHECK: (&str, &str) = (
    "verified_capacity_check",
    "extern \"C\" fn(u64, u64, u64) -> u8",
);

/// 2. `verified_field_bounds_check` — Lean `Field -> Layout -> Bool`
///    (field offset+size <= total). Mirrors `verified_field_bounds_check_correct`.
///
/// Expected Rust extern signature:
/// ```ignore
/// extern "C" transform verified_field_bounds_check(offset: u64, size: u64, total: u64) -> u8;
/// ```
const SYM_VERIFIED_FIELD_BOUNDS_CHECK: (&str, &str) = (
    "verified_field_bounds_check",
    "extern \"C\" fn(u64, u64, u64) -> u8",
);

/// 3. `verified_linearity_check` — Lean `String -> List String -> Bool`
///    (each use of a linear variable is consumed at most once).
///
/// Expected Rust extern signature (flattened C marshalling, FFI_BRIDGE_PLAN §1
/// option (ii)):
/// ```ignore
/// extern "C" transform verified_linearity_check(
///     var: *const c_char,
///     consumed: *const *const c_char,
///     consumed_len: usize,
/// ) -> u8;
/// ```
const SYM_VERIFIED_LINEARITY_CHECK: (&str, &str) = (
    "verified_linearity_check",
    "extern \"C\" fn(*const c_char, *const *const c_char, usize) -> u8",
);

/// 4. `verified_pmt_check` — Lean composition of 1–3.
///
/// Expected Rust extern signature (aggregate; argument order matches
/// `pmt_check::verified_pmt_check`):
/// ```ignore
/// extern "C" transform verified_pmt_check(
///     used: u64, total: u64, capacity: u64,        // -> capacity check
///     offset: u64, size: u64,                       // -> field-bounds check (total reused)
///     var: *const c_char, consumed: *const *const c_char, consumed_len: usize, // -> linearity
/// ) -> u8;
/// ```
const SYM_VERIFIED_PMT_CHECK: (&str, &str) = (
    "verified_pmt_check",
    "extern \"C\" fn(u64,u64,u64, u64,u64, *const c_char,*const *const c_char,usize) -> u8",
);

/// 5. `lean_verify_transform` — Lean `LayoutRegistry -> StateTransform -> Bool`
///    (boxed Lean objects; Group B marshalling risk per FFI_BRIDGE_PLAN §1).
///
/// Expected Rust extern signature:
/// ```ignore
/// extern "C" transform lean_verify_transform(registry_and_transform: *mut LeanObject) -> u8;
/// ```
const SYM_LEAN_VERIFY_TRANSFORM: (&str, &str) = (
    "lean_verify_transform",
    "extern \"C\" fn(*mut LeanObject) -> u8",
);

/// 6. `lean_verify_state_reads` — Lean
///    `List (String * LayoutInfo) -> List StateRead -> Bool`.
///
/// Expected Rust extern signature:
/// ```ignore
/// extern "C" transform lean_verify_state_reads(
///     layouts: *mut LeanObject,
///     reads: *mut LeanObject,
/// ) -> u8;
/// ```
const SYM_LEAN_VERIFY_STATE_READS: (&str, &str) = (
    "lean_verify_state_reads",
    "extern \"C\" fn(*mut LeanObject, *mut LeanObject) -> u8",
);

/// 7. `lean_verify_state_writes` — Lean
///    `List (String * LayoutInfo) -> List String -> List StateWrite -> Bool`.
///
/// Expected Rust extern signature:
/// ```ignore
/// extern "C" transform lean_verify_state_writes(
///     layouts: *mut LeanObject,
///     consumed: *mut LeanObject,
///     writes: *mut LeanObject,
/// ) -> u8;
/// ```
const SYM_LEAN_VERIFY_STATE_WRITES: (&str, &str) = (
    "lean_verify_state_writes",
    "extern \"C\" fn(*mut LeanObject, *mut LeanObject, *mut LeanObject) -> u8",
);

/// All 7 expected Lean<->Rust FFI symbols, in FFI_BRIDGE_PLAN.md §1 order.
const EXPECTED_SYMBOLS: [(&str, &str); 7] = [
    SYM_VERIFIED_CAPACITY_CHECK,
    SYM_VERIFIED_FIELD_BOUNDS_CHECK,
    SYM_VERIFIED_LINEARITY_CHECK,
    SYM_VERIFIED_PMT_CHECK,
    SYM_LEAN_VERIFY_TRANSFORM,
    SYM_LEAN_VERIFY_STATE_READS,
    SYM_LEAN_VERIFY_STATE_WRITES,
];

// ===========================================================================
// libdl bindings — declared inline so we introduce no `libc` dev-dependency.
// (Wave 5-B owns Cargo.toml; do not edit it here.)
// Entirely feature-gated: only needed for the real (feature-on) check.
// ===========================================================================

#[cfg(feature = "pmt-runtime-check")]
mod dl {
    use std::os::raw::{c_char, c_void};

    /// `RTLD_DEFAULT` — search every loaded image for the symbol.
    /// On glibc and musl this is the null pointer; passing `null_mut()` to
    /// `dlsym` is equivalent to `dlsym(RTLD_DEFAULT, ...)`.
    /// (macOS would need `((void *) -2)`; this workspace is Linux-only.)
    #[allow(dead_code)]
    pub(super) const RTLD_DEFAULT: *mut c_void = std::ptr::null_mut();

    // On GNU/glibc (incl. pre-2.34 where dlsym lives in libdl) link libdl.
    // On musl and other libc flavors dlsym/dlerror are provided by libc directly.
    #[cfg(target_env = "gnu")]
    #[link(name = "dl")]
    extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlerror() -> *mut c_char;
    }

    #[cfg(not(target_env = "gnu"))]
    extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlerror() -> *mut c_char;
    }

    /// Resolve `name` in the process-wide global symbol scope.
    ///
    /// Returns the symbol address on success, or an error string describing
    /// why resolution failed (used for the per-symbol diagnostic in the test
    /// report).
    pub(super) fn resolve_symbol(name: &str) -> Result<*mut c_void, String> {
        let cname = std::ffi::CString::new(name)
            .map_err(|e| format!("CString::new failed: {e}"))?;
        // Clear any stale dlerror state before the lookup.
        unsafe { dlerror() };
        let addr = unsafe { dlsym(RTLD_DEFAULT, cname.as_ptr()) };
        if addr.is_null() {
            let err = unsafe { dlerror() };
            let msg = if err.is_null() {
                "symbol not found (dlerror returned NULL)".to_string()
            } else {
                unsafe { std::ffi::CStr::from_ptr(err) }
                    .to_string_lossy()
                    .into_owned()
            };
            Err(msg)
        } else {
            Ok(addr)
        }
    }
}

// ===========================================================================
// Test entry points (feature-gated; see module docs).
// ===========================================================================

/// Real structural check — only compiled when the Lean archive is intended to
/// be linked (`pmt-runtime-check`). Asserts all 7 symbols resolve via dlsym.
#[cfg(feature = "pmt-runtime-check")]
#[test]
fn ffi_signature_conformance() {
    let total = EXPECTED_SYMBOLS.len();
    let mut missing: Vec<&str> = Vec::new();

    eprintln!(
        "[ffi-sig] structural conformance check: {total} expected Lean FFI symbols \
         (FFI_BRIDGE_PLAN.md §1)"
    );
    for (name, sig) in EXPECTED_SYMBOLS {
        eprintln!("[ffi-sig]   expect `{name}`  ::  {sig}");
        match dl::resolve_symbol(name) {
            Ok(addr) => eprintln!("[ffi-sig]   OK    `{name}` resolved @ {addr:p}"),
            Err(e) => {
                eprintln!("[ffi-sig]   MISS  `{name}` — {e}");
                missing.push(name);
            }
        }
    }

    assert!(
        missing.is_empty(),
        "FFI signature conformance FAILED: {}/{} expected Lean FFI symbols are not \
         resolvable via dlsym(RTLD_DEFAULT): [{}].\n\
         Likely causes:\n  \
           (a) Lean archive not linked yet — build.rs (Wave 5-A) not wired.\n  \
           (b) Symbols are linked but not dynamically exported — add \
         `-Wl,--export-dynamic` (Cargo.toml/`.cargo/config`, Wave 5-B) or reference them \
         via a Rust `extern \"C\"` block.\n  \
           (c) Lean extraction (`proof/PMT/Extraction.lean`) does not export these names.\n\
         See FFI_BRIDGE_PLAN.md §1.",
        missing.len(),
        total,
        missing.join(", "),
    );
}

/// No-op fallback when `pmt-runtime-check` is OFF. The Lean archive is never
/// linked in default builds, so a structural check would be meaningless; we
/// just print a skip message and return Ok.
#[cfg(not(feature = "pmt-runtime-check"))]
#[test]
fn ffi_signature_conformance() {
    eprintln!(
        "[ffi-sig] SKIP: feature `pmt-runtime-check` is OFF — Lean FFI archive is not \
         linked in default builds (Wave 5-A). The {n} expected symbols are documented in \
         FFI_BRIDGE_PLAN.md §1; re-run with `--features pmt-runtime-check` to enforce \
         structural conformance.",
        n = EXPECTED_SYMBOLS.len()
    );
}
