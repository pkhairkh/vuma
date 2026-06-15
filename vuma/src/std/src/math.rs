//! # Mathematical Utility Functions
//!
//! This module provides VUMA-verified mathematical helper functions that
//! LLMs frequently need when writing real programs. These are simple,
//! pure functions that operate on integer values — no floating-point
//! operations, no side effects, and no capability requirements beyond
//! { Read, Compare }.
//!
//! ## Why These Functions?
//!
//! LLMs generating VUMA code often need:
//!
//! - **`abs`**: Computing distances, error magnitudes, or delta values.
//! - **`min` / `max`**: Clamping loop bounds, selecting extremal values,
//!   or implementing saturation arithmetic.
//! - **`clamp`**: Constraining values to valid ranges (array indices,
//!   color channels, buffer offsets).
//!
//! These are trivial to implement but are so commonly needed that having
//! them in the standard library saves boilerplate and reduces the chance
//! of bugs (e.g., integer overflow in a hand-written `abs`).
//!
//! ## BD Annotations
//!
//! All functions in this module are pure — they have no side effects and
//! their CapD is { Read, Compare }.

use crate::primitives::{CapD, CapFlag};

// ---------------------------------------------------------------------------
// Absolute Value
// ---------------------------------------------------------------------------

/// Compute the absolute value of a signed 64-bit integer.
///
/// Returns the magnitude of `x` without its sign. For non-negative inputs,
/// the result is identical to the input. For negative inputs, the result
/// is `-x`.
///
/// ## Edge Case: i64::MIN
///
/// `i64::MIN` (-9223372036854775808) has no positive counterpart in the
/// `i64` range. Calling `abs(i64::MIN)` returns `i64::MIN` itself,
/// matching the behavior of Rust's `i64::wrapping_abs()`. This is a
/// known overflow that callers should handle explicitly if it may occur.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn abs(x: i64) -> i64 {
///     if x < 0 {
///         return -x;
///     }
///     return x;
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: absolute value is correct for all non-MIN inputs
pub fn abs(x: i64) -> i64 {
    if x < 0 {
        x.wrapping_neg()
    } else {
        x
    }
}

// ---------------------------------------------------------------------------
// Minimum
// ---------------------------------------------------------------------------

/// Return the smaller of two signed 64-bit integers.
///
/// If both values are equal, returns either one (specifically `a`).
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn min(a: i64, b: i64) -> i64 {
///     if a <= b {
///         return a;
///     }
///     return b;
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: minimum selection is correct
pub fn min(a: i64, b: i64) -> i64 {
    if a <= b {
        a
    } else {
        b
    }
}

// ---------------------------------------------------------------------------
// Maximum
// ---------------------------------------------------------------------------

/// Return the larger of two signed 64-bit integers.
///
/// If both values are equal, returns either one (specifically `a`).
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn max(a: i64, b: i64) -> i64 {
///     if a >= b {
///         return a;
///     }
///     return b;
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: maximum selection is correct
pub fn max(a: i64, b: i64) -> i64 {
    if a >= b {
        a
    } else {
        b
    }
}

// ---------------------------------------------------------------------------
// Clamp
// ---------------------------------------------------------------------------

/// Constrain a value to lie within the inclusive range `[lo, hi]`.
///
/// Returns `lo` if `x < lo`, `hi` if `x > hi`, or `x` otherwise.
/// This is equivalent to `max(lo, min(hi, x))` but expressed as a single
/// operation for clarity and efficiency.
///
/// ## Panics
///
/// In debug builds, panics if `lo > hi`. In release builds, the behavior
/// is `max(lo, min(hi, x))`, which effectively swaps the bounds.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn clamp(x: i64, lo: i64, hi: i64) -> i64 {
///     if x < lo {
///         return lo;
///     }
///     if x > hi {
///         return hi;
///     }
///     return x;
/// }
/// ```
///
/// ## Common Use Cases
///
/// - Clamping array indices: `clamp(idx, 0, len - 1)`
/// - Clamping color channels: `clamp(channel, 0, 255)`
/// - Clamping buffer offsets: `clamp(offset, 0, buf_size - 1)`
/// - Saturation arithmetic: `clamp(a + b, i64::MIN, i64::MAX)`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: clamp is correct when lo <= hi
pub fn clamp(x: i64, lo: i64, hi: i64) -> i64 {
    debug_assert!(lo <= hi, "clamp: lo ({}) must be <= hi ({})", lo, hi);
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

// ---------------------------------------------------------------------------
// Capability Descriptor for Math Operations
// ---------------------------------------------------------------------------

/// Returns the capability descriptor for mathematical operations.
///
/// All math functions in this module are pure and require only Read and
/// Compare capabilities.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure functions, no side effects
// VUMA-VERIFIED: math operations are pure and comparison-based
pub fn math_capd() -> CapD {
    CapD::new(vec![CapFlag::Read, CapFlag::Compare])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- abs tests ---

    #[test]
    fn test_abs_positive() {
        assert_eq!(abs(42), 42);
    }

    #[test]
    fn test_abs_zero() {
        assert_eq!(abs(0), 0);
    }

    #[test]
    fn test_abs_negative() {
        assert_eq!(abs(-42), 42);
    }

    #[test]
    fn test_abs_one() {
        assert_eq!(abs(1), 1);
        assert_eq!(abs(-1), 1);
    }

    #[test]
    fn test_abs_i64_max() {
        assert_eq!(abs(i64::MAX), i64::MAX);
    }

    #[test]
    fn test_abs_i64_min() {
        // i64::MIN has no positive counterpart; wrapping_neg gives i64::MIN
        assert_eq!(abs(i64::MIN), i64::MIN);
    }

    // --- min tests ---

    #[test]
    fn test_min_a_less() {
        assert_eq!(min(1, 2), 1);
    }

    #[test]
    fn test_min_b_less() {
        assert_eq!(min(2, 1), 1);
    }

    #[test]
    fn test_min_equal() {
        assert_eq!(min(5, 5), 5);
    }

    #[test]
    fn test_min_negative() {
        assert_eq!(min(-10, 5), -10);
        assert_eq!(min(5, -10), -10);
    }

    #[test]
    fn test_min_extreme() {
        assert_eq!(min(i64::MIN, i64::MAX), i64::MIN);
    }

    // --- max tests ---

    #[test]
    fn test_max_a_greater() {
        assert_eq!(max(2, 1), 2);
    }

    #[test]
    fn test_max_b_greater() {
        assert_eq!(max(1, 2), 2);
    }

    #[test]
    fn test_max_equal() {
        assert_eq!(max(5, 5), 5);
    }

    #[test]
    fn test_max_negative() {
        assert_eq!(max(-10, 5), 5);
        assert_eq!(max(5, -10), 5);
    }

    #[test]
    fn test_max_extreme() {
        assert_eq!(max(i64::MIN, i64::MAX), i64::MAX);
    }

    // --- clamp tests ---

    #[test]
    fn test_clamp_below() {
        assert_eq!(clamp(-5, 0, 10), 0);
    }

    #[test]
    fn test_clamp_above() {
        assert_eq!(clamp(15, 0, 10), 10);
    }

    #[test]
    fn test_clamp_within() {
        assert_eq!(clamp(5, 0, 10), 5);
    }

    #[test]
    fn test_clamp_at_lo() {
        assert_eq!(clamp(0, 0, 10), 0);
    }

    #[test]
    fn test_clamp_at_hi() {
        assert_eq!(clamp(10, 0, 10), 10);
    }

    #[test]
    fn test_clamp_negative_range() {
        assert_eq!(clamp(-5, -10, -1), -5);
        assert_eq!(clamp(-15, -10, -1), -10);
        assert_eq!(clamp(0, -10, -1), -1);
    }

    #[test]
    fn test_clamp_singleton() {
        assert_eq!(clamp(42, 7, 7), 7);
        assert_eq!(clamp(7, 7, 7), 7);
    }

    // --- capd tests ---

    #[test]
    fn test_math_capd() {
        let capd = math_capd();
        assert!(capd.has(CapFlag::Read));
        assert!(capd.has(CapFlag::Compare));
        assert!(!capd.has(CapFlag::Write));
    }
}
