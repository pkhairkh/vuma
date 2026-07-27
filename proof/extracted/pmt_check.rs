//! pmt_check.rs — Rust translation of Lean-verified PMT checkers.
//!
//! These functions are hand-translated from proof/PMT/Extraction.lean.
//! Each Rust function mirrors a Lean function that has a machine-checked
//! soundness theorem. The parity test (tests/pmt_parity_test.rs) verifies
//! the Rust matches the Lean semantics on all test cases.
//!
//! This is NOT FFI extraction — it's a verified-by-parity-test hand translation.
//! The Lean proofs establish the mathematical correctness; the parity test
//! establishes the Rust matches the Lean.
//!
//! ## Wave 1 task IVE-1-B: IVE state-verifier FFI surface
//!
//! In addition to the hand-translated PMT-level checkers above, this file
//! now declares the `extern "C"` FFI surface for the 3 IVE state verifiers
//! that are `@[export]`-ed from `proof/PMT/Extraction.lean`:
//!
//!   - `lean_verify_transform` — extracted from `PMT.IVE.Soundness.verify_transform`
//!   - `lean_verify_state_reads` — extracted from `PMT.IVE.Soundness.verify_state_reads`
//!   - `lean_verify_state_writes` — extracted from `PMT.IVE.Soundness.verify_state_writes`
//!
//! The actual FFI call requires Lean's C backend output (produced by
//! `lake build` in `proof/.lake/build/ir/`) to be linked into the Rust
//! binary. When the `pmt-runtime-check` feature is enabled AND the Lean
//! C output is linked, `verification.rs` routes the 3 verifiers through
//! these FFI declarations. When the feature is off (or the Lean C output
//! is not linked), the hand-written Rust verifiers in `state_read.rs`,
//! `state_write.rs`, `state_transform.rs` are used.
//!
//! The Lean C output is NOT automatically linked in the current build
//! (this requires a `build.rs` script that compiles `proof/.lake/build/ir/*.c`
//! into a static library). For now, the `extern "C"` declarations below
//! serve as the FFI contract; the parity test in `tests/pmt_parity_test.rs`
//! compares the hand-written Rust verifiers against the expected Lean
//! semantics (computed by hand from the Lean definitions). When the
//! build-system integration is complete (Wave 1 task IVE-1-D), the
//! parity test will call the actual extracted C functions.

/// Verified capacity check: returns true iff used + size ≤ capacity.
/// Lean: `verified_capacity_check(used size capacity : Nat) : Bool`
/// Soundness: verified_capacity_check_correct (proof/PMT/Extraction.lean)
#[inline]
pub fn verified_capacity_check(used: u64, size: u64, capacity: u64) -> bool {
    used.checked_add(size).map_or(false, |sum| sum <= capacity)
}

/// Verified field-bounds check: returns true iff offset + size ≤ total.
/// Lean: `verified_field_bounds_check(f : Field) (layout : Layout) : Bool`
#[inline]
pub fn verified_field_bounds_check(offset: u64, size: u64, total: u64) -> bool {
    offset.checked_add(size).map_or(false, |sum| sum <= total)
}

/// Verified linearity check: returns true iff var is NOT in consumed.
/// Lean: `verified_linearity_check(var : String) (consumed : List String) : Bool`
#[inline]
pub fn verified_linearity_check(var: &str, consumed: &[&str]) -> bool {
    !consumed.iter().any(|c| *c == var)
}

/// Composed PMT check: all three sub-checks.
/// Lean: `verified_pmt_check(...) : Bool`
#[inline]
pub fn verified_pmt_check(
    used: u64, capacity: u64,
    offset: u64, size: u64, total: u64,
    var: &str, consumed: &[&str],
) -> bool {
    verified_capacity_check(used, total, capacity)
        && verified_field_bounds_check(offset, size, total)
        && verified_linearity_check(var, consumed)
}

// ─────────────────────────────────────────────────────────────────────
// IVE state-verifier FFI surface (Wave 1 task IVE-1-B)
// ─────────────────────────────────────────────────────────────────────
//
// These `extern "C"` declarations match the `@[export]`-ed Lean functions
// in `proof/PMT/Extraction.lean` §8. The actual symbols are present in
// `proof/.lake/build/ir/PMT/Extraction.c` after `lake build` (Lean's C
// backend emits them). Linking this C output into the Rust binary requires
// a `build.rs` script (deferred to Wave 1 task IVE-1-D's parity-test
// harness). For now, these declarations document the FFI contract and
// allow `verification.rs` to conditionally route through them.
//
// Lean type → C type mapping (Lean 4.21 unboxed representation):
//   - `Bool`         → `uint8_t` (0 = false, 1 = true)
//   - `Nat`          → `lean_object*` (boxed, arbitrary precision)
//   - `String`       → `lean_object*` (boxed Lean string)
//   - `List α`       → `lean_object*` (boxed cons-list)
//   - structures     → `lean_object*` (boxed struct)
//
// For the FFI surface, we use `lean_object*` (opaque pointer) for all
// boxed types. The Rust-side marshalling helpers (below) convert between
// Rust's native types and the Lean boxed representation. The marshalling
// is only used when the `pmt-runtime-check` feature is enabled AND the
// Lean C output is linked; otherwise the hand-written Rust verifiers
// are used directly.

#[cfg(feature = "pmt-runtime-check")]
mod lean_ffi {
    /// Opaque pointer to a Lean object (boxed `Nat`, `String`, `List`,
    /// struct, etc.). Matches Lean 4.21's `lean_object*` type.
    pub type LeanObject = std::ffi::c_void;

    // ─────────────────────────────────────────────────────────────
    // Group A — `@[export lean_verified_*]` wrappers (functions 1-4).
    // Per the FFI docstrings in `proof/PMT/Extraction.lean` §1-§4,
    // every argument is a boxed `lean_object*` (Lean's C ABI boxes
    // `BitVec 64`, `Nat`, `Field`, `Layout`, `String`, `List α`); the
    // `Bool` return is marshalled as `uint8_t` (0 = false, 1 = true).
    // Wave 4-A may later add Lean-side unboxing wrappers exposing
    // primitive C signatures; until then these externs match the
    // *current* `@[export]`'d signatures exactly (all `lean_object*`,
    // return `u8`).
    // ─────────────────────────────────────────────────────────────
    extern "C" {
        /// `@[export lean_verified_capacity_check]` — Lean signature
        /// `(used size capacity : BitVec 64) : Bool`. FFI docstring:
        /// `lean_verified_capacity_check(lean_object*, lean_object*,
        /// lean_object*) -> uint8_t`.
        pub fn lean_verified_capacity_check(
            used: *mut LeanObject,
            size: *mut LeanObject,
            capacity: *mut LeanObject,
        ) -> u8;

        /// `@[export lean_verified_field_bounds_check]` — Lean signature
        /// `(f : Field) (layout : Layout) : Bool`. FFI docstring:
        /// `lean_verified_field_bounds_check(lean_object*,
        /// lean_object*) -> uint8_t`.
        pub fn lean_verified_field_bounds_check(
            f: *mut LeanObject,
            layout: *mut LeanObject,
        ) -> u8;

        /// `@[export lean_verified_linearity_check]` — Lean signature
        /// `(var : String) (consumed : List String) : Bool`. FFI
        /// docstring: `lean_verified_linearity_check(lean_object*,
        /// lean_object*) -> uint8_t`.
        pub fn lean_verified_linearity_check(
            var: *mut LeanObject,
            consumed: *mut LeanObject,
        ) -> u8;

        /// `@[export lean_verified_pmt_check]` — Lean signature
        /// `(used capacity : Nat) (f : Field) (layout : Layout)
        /// (var : String) (consumed : List String) : Bool`. FFI
        /// docstring: six `lean_object*` arguments (used, capacity, f,
        /// layout, var, consumed) returning `uint8_t`.
        pub fn lean_verified_pmt_check(
            used: *mut LeanObject,
            capacity: *mut LeanObject,
            f: *mut LeanObject,
            layout: *mut LeanObject,
            var: *mut LeanObject,
            consumed: *mut LeanObject,
        ) -> u8;

        // ─────────────────────────────────────────────────────────
        // Group B — IVE state verifiers carrying complex Lean structs
        // (functions 5-7). All arguments are opaque boxed
        // `lean_object*`; the `Bool` return is `uint8_t`.
        // ─────────────────────────────────────────────────────────

        /// `@[export lean_verify_transform]` — Lean signature
        /// `(layouts : LayoutRegistry) (t : StateTransform) : Bool`.
        /// Two boxed `lean_object*` args, returns `uint8_t` (the
        /// `valid` field; 0 = false, 1 = true).
        pub fn lean_verify_transform(
            layouts: *mut LeanObject,
            t: *mut LeanObject,
        ) -> u8;

        /// `@[export lean_verify_state_reads]` — Lean signature
        /// `(env_list : List (String × LayoutInfo))
        /// (reads : List StateRead) : Bool`. Two boxed `lean_object*`
        /// args, returns `uint8_t` (1 iff all reads pass).
        pub fn lean_verify_state_reads(
            env_list: *mut LeanObject,
            reads: *mut LeanObject,
        ) -> u8;

        /// `@[export lean_verify_state_writes]` — Lean signature
        /// `(env_list : List (String × LayoutInfo))
        /// (consumed : List String) (writes : List StateWrite) : Bool`.
        /// Three boxed `lean_object*` args, returns `uint8_t` (1 iff
        /// all writes pass).
        pub fn lean_verify_state_writes(
            env_list: *mut LeanObject,
            consumed: *mut LeanObject,
            writes: *mut LeanObject,
        ) -> u8;
    }

    // ─────────────────────────────────────────────────────────────
    // Rust-side safe-bool wrappers. Each dereferences the extern
    // (returning `u8`) and normalises to `bool` via `!= 0`.
    //
    // # Safety (all wrappers)
    // The caller must ensure (a) the Lean C backend output
    // (`proof/.lake/build/.../PMT/Extraction.c`) is linked into the
    // binary, AND (b) every `*mut LeanObject` argument was constructed
    // by the Wave 4 `marshal` submodule (or Lean runtime) with the
    // exact boxed representation Lean expects. These are only invoked
    // when `feature = "pmt-runtime-check"` is on AND the
    // `lean_ffi_linked` cfg is set (Wave 5 build.rs).
    // ─────────────────────────────────────────────────────────────

    /// Wrapper for `lean_verified_capacity_check`.
    #[inline]
    pub unsafe fn call_lean_verified_capacity_check(
        used: *mut LeanObject,
        size: *mut LeanObject,
        capacity: *mut LeanObject,
    ) -> bool {
        unsafe { lean_verified_capacity_check(used, size, capacity) != 0 }
    }

    /// Wrapper for `lean_verified_field_bounds_check`.
    #[inline]
    pub unsafe fn call_lean_verified_field_bounds_check(
        f: *mut LeanObject,
        layout: *mut LeanObject,
    ) -> bool {
        unsafe { lean_verified_field_bounds_check(f, layout) != 0 }
    }

    /// Wrapper for `lean_verified_linearity_check`.
    #[inline]
    pub unsafe fn call_lean_verified_linearity_check(
        var: *mut LeanObject,
        consumed: *mut LeanObject,
    ) -> bool {
        unsafe { lean_verified_linearity_check(var, consumed) != 0 }
    }

    /// Wrapper for `lean_verified_pmt_check`.
    #[inline]
    pub unsafe fn call_lean_verified_pmt_check(
        used: *mut LeanObject,
        capacity: *mut LeanObject,
        f: *mut LeanObject,
        layout: *mut LeanObject,
        var: *mut LeanObject,
        consumed: *mut LeanObject,
    ) -> bool {
        unsafe { lean_verified_pmt_check(used, capacity, f, layout, var, consumed) != 0 }
    }

    /// Wrapper for `lean_verify_transform` — takes the boxed
    /// `LayoutRegistry` and boxed `StateTransform`.
    #[inline]
    pub unsafe fn call_lean_verify_transform(
        layouts: *mut LeanObject,
        t: *mut LeanObject,
    ) -> bool {
        unsafe { lean_verify_transform(layouts, t) != 0 }
    }

    /// Wrapper for `lean_verify_state_reads`.
    #[inline]
    pub unsafe fn call_lean_verify_state_reads(
        env_list: *mut LeanObject,
        reads: *mut LeanObject,
    ) -> bool {
        unsafe { lean_verify_state_reads(env_list, reads) != 0 }
    }

    /// Wrapper for `lean_verify_state_writes`.
    #[inline]
    pub unsafe fn call_lean_verify_state_writes(
        env_list: *mut LeanObject,
        consumed: *mut LeanObject,
        writes: *mut LeanObject,
    ) -> bool {
        unsafe { lean_verify_state_writes(env_list, consumed, writes) != 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_check_pass() { assert!(verified_capacity_check(0, 16, 1024)); }
    #[test]
    fn capacity_check_fail() { assert!(!verified_capacity_check(1000, 100, 1024)); }
    #[test]
    fn capacity_check_overflow() { assert!(!verified_capacity_check(u64::MAX, 1, u64::MAX)); }
    #[test]
    fn field_bounds_pass() { assert!(verified_field_bounds_check(0, 4, 16)); }
    #[test]
    fn field_bounds_fail() { assert!(!verified_field_bounds_check(12, 8, 16)); }
    #[test]
    fn linearity_pass() { assert!(verified_linearity_check("x", &["a", "b"])); }
    #[test]
    fn linearity_fail() { assert!(!verified_linearity_check("a", &["a", "b"])); }
    #[test]
    fn pmt_check_all_pass() { assert!(verified_pmt_check(0, 1024, 0, 4, 16, "x", &["a", "b"])); }
    #[test]
    fn pmt_check_cap_fail() { assert!(!verified_pmt_check(1000, 1000, 0, 4, 16, "x", &["a", "b"])); }
}
