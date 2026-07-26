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
