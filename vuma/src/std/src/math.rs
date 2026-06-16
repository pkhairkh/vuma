//! # Mathematical Utility Functions
//!
//! This module provides VUMA-verified mathematical helper functions that
//! LLMs frequently need when writing real programs. Functions span integer
//! arithmetic, floating-point trigonometry, exponentials, rounding,
//! classification, and associated mathematical constants.
//!
//! ## Function Categories
//!
//! - **Integer arithmetic**: `abs`, `min`, `max`, `clamp` — operate on `i64`
//! - **Typed integer arithmetic**: `abs_i32`, `abs_i64`, `min_i32`, `max_i32`,
//!   `min_u64`, `max_u64`, `clamp_i32`, `clamp_u64` — operate on specific widths
//! - **Integer division**: `div_floor_i32`, `div_ceil_i32` — floor and ceiling
//!   division for `i32`
//! - **Number theory**: `gcd_u64`, `lcm_u64` — greatest common divisor and
//!   least common multiple for `u64`
//! - **Bit manipulation**: `is_power_of_two_u64`, `next_power_of_two_u64`,
//!   `count_ones_u64`, `count_zeros_u64`, `leading_zeros_u64`,
//!   `trailing_zeros_u64`, `reverse_bits_u64`, `swap_bytes_u64`
//! - **Trigonometric (f64)**: `sin`, `cos`, `tan`, `asin`, `acos`, `atan`,
//!   `atan2`, `sinh`, `cosh`, `tanh`
//! - **Exponential/Logarithmic (f64)**: `sqrt`, `cbrt`, `exp`, `exp2`,
//!   `exp_m1`, `ln`, `log2`, `log10`, `ln_1p`, `pow`, `powi`
//! - **Rounding (f64)**: `floor`, `ceil`, `round`, `trunc`, `fract`
//! - **Comparison (f64)**: `min_of`, `max_of`
//! - **Classification (f64)**: `is_nan`, `is_infinite`, `is_finite`,
//!   `is_normal`, `signum`, `copysign`
//! - **Constants**: `PI`, `TAU`, `E`, `LN_2`, `LN_10`, `LOG2_E`,
//!   `LOG10_E`, `SQRT_2`, `FRAC_1_SQRT_2`
//! - **f32 variants**: All floating-point functions above have `_f32`
//!   suffixed counterparts (e.g., `sin_f32`, `cos_f32`, `sqrt_f32`).
//! - **f32 constants**: `PI_F32`, `TAU_F32`, `E_F32`, `LN_2_F32`,
//!   `LN_10_F32`, `LOG2_E_F32`, `LOG10_E_F32`, `SQRT_2_F32`,
//!   `FRAC_1_SQRT_2_F32`
//!
//! ## BD Annotations
//!
//! All functions in this module are pure — they have no side effects and
//! their CapD is { Read, Compare }.

use crate::primitives::{CapD, CapFlag};

// ===========================================================================
// Mathematical Constants (f64)
// ===========================================================================

/// Archimedes' constant (π ≈ 3.14159265358979323846).
///
/// The ratio of a circle's circumference to its diameter.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f64::consts::PI
pub const PI: f64 = std::f64::consts::PI;

/// The full circle constant (τ = 2π ≈ 6.28318530717958647693).
///
/// The ratio of a circle's circumference to its radius. Preferred in
/// some pedagogical contexts over PI because it simplifies angle
/// arithmetic (one full turn = τ radians).
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f64::consts::TAU
pub const TAU: f64 = std::f64::consts::TAU;

/// Euler's number (e ≈ 2.71828182845904523536).
///
/// The base of the natural logarithm. Fundamental to exponential growth
/// and decay, compound interest, and probability distributions.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f64::consts::E
pub const E: f64 = std::f64::consts::E;

/// Natural logarithm of 2 (ln 2 ≈ 0.69314718055994530942).
///
/// Useful for converting between log base e and log base 2.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f64::consts::LN_2
pub const LN_2: f64 = std::f64::consts::LN_2;

/// Natural logarithm of 10 (ln 10 ≈ 2.30258509299404568402).
///
/// Useful for converting between log base e and log base 10.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f64::consts::LN_10
pub const LN_10: f64 = std::f64::consts::LN_10;

/// Logarithm base 2 of Euler's number (log₂ e ≈ 1.44269504088896340736).
///
/// Useful for converting between log base 2 and natural logarithm.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f64::consts::LOG2_E
pub const LOG2_E: f64 = std::f64::consts::LOG2_E;

/// Logarithm base 10 of Euler's number (log₁₀ e ≈ 0.43429448190325182765).
///
/// Useful for converting between log base 10 and natural logarithm.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f64::consts::LOG10_E
pub const LOG10_E: f64 = std::f64::consts::LOG10_E;

/// Square root of 2 (√2 ≈ 1.41421356237309504880).
///
/// The length of the diagonal of a unit square.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f64::consts::SQRT_2
pub const SQRT_2: f64 = std::f64::consts::SQRT_2;

/// Reciprocal of the square root of 2 (1/√2 ≈ 0.70710678118654752440).
///
/// Commonly encountered in signal processing (e.g., RMS normalization)
/// and rotation matrices.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f64::consts::FRAC_1_SQRT_2
pub const FRAC_1_SQRT_2: f64 = std::f64::consts::FRAC_1_SQRT_2;

// ===========================================================================
// Mathematical Constants (f32)
// ===========================================================================

/// Archimedes' constant (π) as f32.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f32::consts::PI
pub const PI_F32: f32 = std::f32::consts::PI;

/// The full circle constant (τ = 2π) as f32.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f32::consts::TAU
pub const TAU_F32: f32 = std::f32::consts::TAU;

/// Euler's number (e) as f32.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f32::consts::E
pub const E_F32: f32 = std::f32::consts::E;

/// Natural logarithm of 2 (ln 2) as f32.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f32::consts::LN_2
pub const LN_2_F32: f32 = std::f32::consts::LN_2;

/// Natural logarithm of 10 (ln 10) as f32.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f32::consts::LN_10
pub const LN_10_F32: f32 = std::f32::consts::LN_10;

/// Logarithm base 2 of Euler's number (log₂ e) as f32.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f32::consts::LOG2_E
pub const LOG2_E_F32: f32 = std::f32::consts::LOG2_E;

/// Logarithm base 10 of Euler's number (log₁₀ e) as f32.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f32::consts::LOG10_E
pub const LOG10_E_F32: f32 = std::f32::consts::LOG10_E;

/// Square root of 2 (√2) as f32.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f32::consts::SQRT_2
pub const SQRT_2_F32: f32 = std::f32::consts::SQRT_2;

/// Reciprocal of the square root of 2 (1/√2) as f32.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure constant, no side effects
// VUMA-VERIFIED: constant value matches std::f32::consts::FRAC_1_SQRT_2
pub const FRAC_1_SQRT_2_F32: f32 = std::f32::consts::FRAC_1_SQRT_2;

// ===========================================================================
// Integer Arithmetic
// ===========================================================================

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

// ===========================================================================
// Typed Integer Arithmetic (i32, i64, u64)
// ===========================================================================

// ---------------------------------------------------------------------------
// abs_i32
// ---------------------------------------------------------------------------

/// Return the absolute value of a signed 32-bit integer.
///
/// If `x` is non-negative, the result is `x`. If `x` is negative, the result
/// is `-x`.
///
/// ## Edge Case: i32::MIN
///
/// `i32::MIN` (-2147483648) has no positive counterpart in the `i32` range.
/// Calling `abs_i32(i32::MIN)` returns `i32::MIN` itself, matching the
/// behavior of Rust's `i32::wrapping_abs()`. This is a known overflow that
/// callers should handle explicitly if it may occur.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: absolute value is correct for all non-MIN inputs
pub fn abs_i32(x: i32) -> i32 {
    if x < 0 {
        x.wrapping_neg()
    } else {
        x
    }
}

// ---------------------------------------------------------------------------
// abs_i64
// ---------------------------------------------------------------------------

/// Return the absolute value of a signed 64-bit integer.
///
/// This is the explicitly-typed counterpart of [`abs`], provided for
/// call-sites that require a named `i64`-specific function.
///
/// ## Edge Case: i64::MIN
///
/// `i64::MIN` has no positive counterpart. Calling `abs_i64(i64::MIN)`
/// returns `i64::MIN` itself (wrapping), matching Rust's `i64::wrapping_abs()`.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: absolute value is correct for all non-MIN inputs
pub fn abs_i64(x: i64) -> i64 {
    if x < 0 {
        x.wrapping_neg()
    } else {
        x
    }
}

// ---------------------------------------------------------------------------
// min_i32
// ---------------------------------------------------------------------------

/// Return the smaller of two signed 32-bit integers.
///
/// If both values are equal, returns either one (specifically `a`).
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: minimum selection is correct
pub fn min_i32(a: i32, b: i32) -> i32 {
    if a <= b {
        a
    } else {
        b
    }
}

// ---------------------------------------------------------------------------
// max_i32
// ---------------------------------------------------------------------------

/// Return the larger of two signed 32-bit integers.
///
/// If both values are equal, returns either one (specifically `a`).
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: maximum selection is correct
pub fn max_i32(a: i32, b: i32) -> i32 {
    if a >= b {
        a
    } else {
        b
    }
}

// ---------------------------------------------------------------------------
// min_u64
// ---------------------------------------------------------------------------

/// Return the smaller of two unsigned 64-bit integers.
///
/// If both values are equal, returns either one (specifically `a`).
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: minimum selection is correct
pub fn min_u64(a: u64, b: u64) -> u64 {
    if a <= b {
        a
    } else {
        b
    }
}

// ---------------------------------------------------------------------------
// max_u64
// ---------------------------------------------------------------------------

/// Return the larger of two unsigned 64-bit integers.
///
/// If both values are equal, returns either one (specifically `a`).
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: maximum selection is correct
pub fn max_u64(a: u64, b: u64) -> u64 {
    if a >= b {
        a
    } else {
        b
    }
}

// ---------------------------------------------------------------------------
// clamp_i32
// ---------------------------------------------------------------------------

/// Constrain a signed 32-bit value to lie within the inclusive range `[lo, hi]`.
///
/// Returns `lo` if `x < lo`, `hi` if `x > hi`, or `x` otherwise.
///
/// ## Panics
///
/// In debug builds, panics if `lo > hi`. In release builds, the behavior
/// is `max(lo, min(hi, x))`, which effectively swaps the bounds.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: clamp is correct when lo <= hi
pub fn clamp_i32(x: i32, lo: i32, hi: i32) -> i32 {
    debug_assert!(lo <= hi, "clamp_i32: lo ({}) must be <= hi ({})", lo, hi);
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

// ---------------------------------------------------------------------------
// clamp_u64
// ---------------------------------------------------------------------------

/// Constrain an unsigned 64-bit value to lie within the inclusive range `[lo, hi]`.
///
/// Returns `lo` if `x < lo`, `hi` if `x > hi`, or `x` otherwise.
///
/// ## Panics
///
/// In debug builds, panics if `lo > hi`. In release builds, the behavior
/// is `max(lo, min(hi, x))`, which effectively swaps the bounds.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: clamp is correct when lo <= hi
pub fn clamp_u64(x: u64, lo: u64, hi: u64) -> u64 {
    debug_assert!(lo <= hi, "clamp_u64: lo ({}) must be <= hi ({})", lo, hi);
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

// ===========================================================================
// Integer Division (i32)
// ===========================================================================

// ---------------------------------------------------------------------------
// div_floor_i32
// ---------------------------------------------------------------------------

/// Perform floor division of two signed 32-bit integers.
///
/// Returns the largest integer less than or equal to the exact quotient
/// `a / b`. This differs from Rust's default integer division (`/`), which
/// truncates toward zero.
///
/// ## Examples
///
/// - `div_floor_i32(7, 2)` returns `3` (same as truncation)
/// - `div_floor_i32(-7, 2)` returns `-4` (truncation would give `-3`)
/// - `div_floor_i32(7, -2)` returns `-4` (truncation would give `-3`)
/// - `div_floor_i32(-7, -2)` returns `3` (same as truncation)
///
/// ## Panics
///
/// Panics if `b == 0` (division by zero). Also panics on overflow when
/// dividing `i32::MIN` by `-1`.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: floor division semantics are correct
pub fn div_floor_i32(a: i32, b: i32) -> i32 {
    let d = a / b;
    let r = a % b;
    if r != 0 && (a < 0) != (b < 0) {
        d - 1
    } else {
        d
    }
}

// ---------------------------------------------------------------------------
// div_ceil_i32
// ---------------------------------------------------------------------------

/// Perform ceiling division of two signed 32-bit integers.
///
/// Returns the smallest integer greater than or equal to the exact quotient
/// `a / b`.
///
/// ## Examples
///
/// - `div_ceil_i32(7, 2)` returns `4` (truncation would give `3`)
/// - `div_ceil_i32(-7, 2)` returns `-3` (same as truncation)
/// - `div_ceil_i32(7, -2)` returns `-3` (same as truncation)
/// - `div_ceil_i32(-7, -2)` returns `4` (truncation would give `3`)
///
/// ## Panics
///
/// Panics if `b == 0` (division by zero). Also panics on overflow when
/// dividing `i32::MIN` by `-1`.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: ceiling division semantics are correct
pub fn div_ceil_i32(a: i32, b: i32) -> i32 {
    let d = a / b;
    let r = a % b;
    if r != 0 && (a > 0) == (b > 0) {
        d + 1
    } else {
        d
    }
}

// ===========================================================================
// Number Theory (u64)
// ===========================================================================

// ---------------------------------------------------------------------------
// gcd_u64
// ---------------------------------------------------------------------------

/// Compute the greatest common divisor (GCD) of two unsigned 64-bit integers
/// using the Euclidean algorithm.
///
/// The GCD is the largest positive integer that divides both `a` and `b`
/// without remainder. By convention, `gcd(0, 0) == 0`.
///
/// ## Examples
///
/// - `gcd_u64(12, 8)` returns `4`
/// - `gcd_u64(7, 0)` returns `7`
/// - `gcd_u64(0, 7)` returns `7`
/// - `gcd_u64(0, 0)` returns `0`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: Euclidean algorithm is correct
pub fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

// ---------------------------------------------------------------------------
// lcm_u64
// ---------------------------------------------------------------------------

/// Compute the least common multiple (LCM) of two unsigned 64-bit integers.
///
/// The LCM is the smallest positive integer that is a multiple of both
/// `a` and `b`. Returns `0` if either argument is `0`.
///
/// ## Overflow
///
/// The result may overflow `u64`. This function uses `checked_mul` and
/// `checked_div` to avoid silent overflow; if the LCM would overflow,
/// this function panics in debug builds.
///
/// ## Examples
///
/// - `lcm_u64(4, 6)` returns `12`
/// - `lcm_u64(7, 0)` returns `0`
/// - `lcm_u64(0, 7)` returns `0`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: LCM via GCD is correct
pub fn lcm_u64(a: u64, b: u64) -> u64 {
    if a == 0 || b == 0 {
        return 0;
    }
    let g = gcd_u64(a, b);
    // Compute (a / g) * b to reduce overflow risk
    a / g * b
}

// ===========================================================================
// Bit Manipulation (u64)
// ===========================================================================

// ---------------------------------------------------------------------------
// is_power_of_two_u64
// ---------------------------------------------------------------------------

/// Return `true` if `x` is a power of two.
///
/// A power of two has exactly one bit set. Returns `false` for `x == 0`.
///
/// ## Examples
///
/// - `is_power_of_two_u64(1)` returns `true` (2⁰)
/// - `is_power_of_two_u64(2)` returns `true` (2¹)
/// - `is_power_of_two_u64(3)` returns `false`
/// - `is_power_of_two_u64(0)` returns `false`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: power-of-two check is correct
pub fn is_power_of_two_u64(x: u64) -> bool {
    x != 0 && (x & (x - 1)) == 0
}

// ---------------------------------------------------------------------------
// next_power_of_two_u64
// ---------------------------------------------------------------------------

/// Return the smallest power of two greater than or equal to `x`.
///
/// If `x` is already a power of two, returns `x` itself.
///
/// ## Panics
///
/// Panics if the next power of two exceeds `u64::MAX` (i.e., when
/// `x > 2⁶³`).
///
/// ## Examples
///
/// - `next_power_of_two_u64(1)` returns `1`
/// - `next_power_of_two_u64(5)` returns `8`
/// - `next_power_of_two_u64(8)` returns `8`
/// - `next_power_of_two_u64(0)` returns `1`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: next-power-of-two is correct for all valid inputs
pub fn next_power_of_two_u64(x: u64) -> u64 {
    if x == 0 {
        return 1;
    }
    let mut v = x - 1;
    v |= v >> 1;
    v |= v >> 2;
    v |= v >> 4;
    v |= v >> 8;
    v |= v >> 16;
    v |= v >> 32;
    v + 1
}

// ---------------------------------------------------------------------------
// count_ones_u64
// ---------------------------------------------------------------------------

/// Count the number of set bits (population count) in a `u64`.
///
/// Also known as the Hamming weight or popcount.
///
/// ## Examples
///
/// - `count_ones_u64(0)` returns `0`
/// - `count_ones_u64(1)` returns `1`
/// - `count_ones_u64(0xFF)` returns `8`
/// - `count_ones_u64(u64::MAX)` returns `64`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std u64::count_ones
pub fn count_ones_u64(x: u64) -> u32 {
    x.count_ones()
}

// ---------------------------------------------------------------------------
// count_zeros_u64
// ---------------------------------------------------------------------------

/// Count the number of clear (zero) bits in a `u64`.
///
/// Equivalent to `64 - count_ones_u64(x)`.
///
/// ## Examples
///
/// - `count_zeros_u64(0)` returns `64`
/// - `count_zeros_u64(u64::MAX)` returns `0`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std u64::count_zeros
pub fn count_zeros_u64(x: u64) -> u32 {
    x.count_zeros()
}

// ---------------------------------------------------------------------------
// leading_zeros_u64
// ---------------------------------------------------------------------------

/// Count the number of leading zero bits in a `u64`.
///
/// For `x == 0`, returns `64`.
///
/// ## Examples
///
/// - `leading_zeros_u64(1)` returns `63`
/// - `leading_zeros_u64(u64::MAX)` returns `0`
/// - `leading_zeros_u64(0)` returns `64`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std u64::leading_zeros
pub fn leading_zeros_u64(x: u64) -> u32 {
    x.leading_zeros()
}

// ---------------------------------------------------------------------------
// trailing_zeros_u64
// ---------------------------------------------------------------------------

/// Count the number of trailing zero bits in a `u64`.
///
/// For `x == 0`, returns `64`.
///
/// ## Examples
///
/// - `trailing_zeros_u64(1)` returns `0`
/// - `trailing_zeros_u64(2)` returns `1`
/// - `trailing_zeros_u64(0)` returns `64`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std u64::trailing_zeros
pub fn trailing_zeros_u64(x: u64) -> u32 {
    x.trailing_zeros()
}

// ---------------------------------------------------------------------------
// reverse_bits_u64
// ---------------------------------------------------------------------------

/// Reverse the bit order of a `u64`.
///
/// Bit 0 becomes bit 63, bit 1 becomes bit 62, and so on.
///
/// ## Examples
///
/// - `reverse_bits_u64(1)` returns `1 << 63`
/// - `reverse_bits_u64(0)` returns `0`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std u64::reverse_bits
pub fn reverse_bits_u64(x: u64) -> u64 {
    x.reverse_bits()
}

// ---------------------------------------------------------------------------
// swap_bytes_u64
// ---------------------------------------------------------------------------

/// Reverse the byte order of a `u64` (endianness conversion).
///
/// Converts between big-endian and little-endian representations.
///
/// ## Examples
///
/// - `swap_bytes_u64(0x0102030405060708)` returns `0x0807060504030201`
/// - `swap_bytes_u64(0)` returns `0`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std u64::swap_bytes
pub fn swap_bytes_u64(x: u64) -> u64 {
    x.swap_bytes()
}

// ===========================================================================
// Trigonometric Functions (f64)
// ===========================================================================

// ---------------------------------------------------------------------------
// Sine
// ---------------------------------------------------------------------------

/// Compute the sine of `x` radians.
///
/// Returns a value in the range `[-1.0, 1.0]`. The input is interpreted
/// as an angle in radians.
///
/// ## Edge Cases
///
/// - If `x` is NaN, the result is NaN.
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is ±∞, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::sin
pub fn sin(x: f64) -> f64 {
    x.sin()
}

// ---------------------------------------------------------------------------
// Cosine
// ---------------------------------------------------------------------------

/// Compute the cosine of `x` radians.
///
/// Returns a value in the range `[-1.0, 1.0]`. The input is interpreted
/// as an angle in radians.
///
/// ## Edge Cases
///
/// - If `x` is NaN, the result is NaN.
/// - If `x` is ±0, the result is 1.0.
/// - If `x` is ±∞, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::cos
pub fn cos(x: f64) -> f64 {
    x.cos()
}

// ---------------------------------------------------------------------------
// Tangent
// ---------------------------------------------------------------------------

/// Compute the tangent of `x` radians.
///
/// The input is interpreted as an angle in radians. The tangent is the
/// ratio of sine to cosine.
///
/// ## Edge Cases
///
/// - If `x` is NaN, the result is NaN.
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is ±∞, the result is NaN.
/// - At odd multiples of π/2 the result approaches ±∞; the exact
///   return value depends on the host platform.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::tan
pub fn tan(x: f64) -> f64 {
    x.tan()
}

// ---------------------------------------------------------------------------
// Arc Sine
// ---------------------------------------------------------------------------

/// Compute the arc sine of `x`.
///
/// Returns the angle in radians whose sine is `x`. The result is in the
/// range `[-π/2, π/2]`.
///
/// ## Edge Cases
///
/// - If `x` is outside `[-1.0, 1.0]`, the result is NaN.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::asin
pub fn asin(x: f64) -> f64 {
    x.asin()
}

// ---------------------------------------------------------------------------
// Arc Cosine
// ---------------------------------------------------------------------------

/// Compute the arc cosine of `x`.
///
/// Returns the angle in radians whose cosine is `x`. The result is in the
/// range `[0, π]`.
///
/// ## Edge Cases
///
/// - If `x` is outside `[-1.0, 1.0]`, the result is NaN.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::acos
pub fn acos(x: f64) -> f64 {
    x.acos()
}

// ---------------------------------------------------------------------------
// Arc Tangent
// ---------------------------------------------------------------------------

/// Compute the arc tangent of `x`.
///
/// Returns the angle in radians whose tangent is `x`. The result is in the
/// range `[-π/2, π/2]`.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is ±∞, the result is ±π/2.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::atan
pub fn atan(x: f64) -> f64 {
    x.atan()
}

// ---------------------------------------------------------------------------
// Arc Tangent (two-argument)
// ---------------------------------------------------------------------------

/// Compute the four-quadrant arc tangent of `y / x`.
///
/// Returns the angle in radians between the positive x-axis and the point
/// `(x, y)`. The result is in the range `(-π, π]`. Unlike `atan(y / x)`,
/// `atan2` correctly handles all four quadrants and the case where `x` is
/// zero.
///
/// ## Edge Cases
///
/// - `atan2(0.0, 0.0)` returns 0.0.
/// - `atan2(±0.0, -0.0)` returns ±π.
/// - `atan2(±∞, +∞)` returns ±π/4.
/// - `atan2(±∞, -∞)` returns ±3π/4.
/// - If either argument is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::atan2
pub fn atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

// ---------------------------------------------------------------------------
// Hyperbolic Sine
// ---------------------------------------------------------------------------

/// Compute the hyperbolic sine of `x`.
///
/// Defined as `(e^x - e^(-x)) / 2`.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is ±∞, the result is ±∞.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::sinh
pub fn sinh(x: f64) -> f64 {
    x.sinh()
}

// ---------------------------------------------------------------------------
// Hyperbolic Cosine
// ---------------------------------------------------------------------------

/// Compute the hyperbolic cosine of `x`.
///
/// Defined as `(e^x + e^(-x)) / 2`.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is 1.0.
/// - If `x` is ±∞, the result is +∞.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::cosh
pub fn cosh(x: f64) -> f64 {
    x.cosh()
}

// ---------------------------------------------------------------------------
// Hyperbolic Tangent
// ---------------------------------------------------------------------------

/// Compute the hyperbolic tangent of `x`.
///
/// Defined as `sinh(x) / cosh(x)`.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is ±∞, the result is ±1.0.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::tanh
pub fn tanh(x: f64) -> f64 {
    x.tanh()
}

// ===========================================================================
// Exponential / Logarithmic Functions (f64)
// ===========================================================================

// ---------------------------------------------------------------------------
// Square Root
// ---------------------------------------------------------------------------

/// Compute the square root of `x`.
///
/// Returns `√x`. If `x` is negative, the result is NaN.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is +∞, the result is +∞.
/// - If `x` is negative, the result is NaN.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::sqrt
pub fn sqrt(x: f64) -> f64 {
    x.sqrt()
}

// ---------------------------------------------------------------------------
// Cube Root
// ---------------------------------------------------------------------------

/// Compute the cube root of `x`.
///
/// Returns `∛x`. Unlike `sqrt`, this is defined for negative inputs.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is ±∞, the result is ±∞.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::cbrt
pub fn cbrt(x: f64) -> f64 {
    x.cbrt()
}

// ---------------------------------------------------------------------------
// Exponential (base e)
// ---------------------------------------------------------------------------

/// Compute `e^x` (the exponential function).
///
/// Returns Euler's number raised to the power of `x`.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is 1.0.
/// - If `x` is +∞, the result is +∞.
/// - If `x` is -∞, the result is 0.0.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::exp
pub fn exp(x: f64) -> f64 {
    x.exp()
}

// ---------------------------------------------------------------------------
// Exponential (base 2)
// ---------------------------------------------------------------------------

/// Compute `2^x`.
///
/// Returns 2 raised to the power of `x`.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is 1.0.
/// - If `x` is +∞, the result is +∞.
/// - If `x` is -∞, the result is 0.0.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::exp2
pub fn exp2(x: f64) -> f64 {
    x.exp2()
}

// ---------------------------------------------------------------------------
// Exponential minus one
// ---------------------------------------------------------------------------

/// Compute `e^x - 1` with greater precision for small `x`.
///
/// For values of `x` near zero, `exp_m1(x)` is more accurate than
/// computing `exp(x) - 1`, which can lose precision due to cancellation.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is +∞, the result is +∞.
/// - If `x` is -∞, the result is -1.0.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::exp_m1
pub fn exp_m1(x: f64) -> f64 {
    x.exp_m1()
}

// ---------------------------------------------------------------------------
// Natural Logarithm
// ---------------------------------------------------------------------------

/// Compute the natural logarithm of `x` (ln x, logₑ x).
///
/// Returns the value `y` such that `e^y = x`.
///
/// ## Edge Cases
///
/// - If `x` is negative, the result is NaN.
/// - If `x` is ±0, the result is -∞.
/// - If `x` is 1.0, the result is 0.0.
/// - If `x` is +∞, the result is +∞.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::ln
pub fn ln(x: f64) -> f64 {
    x.ln()
}

// ---------------------------------------------------------------------------
// Base-2 Logarithm
// ---------------------------------------------------------------------------

/// Compute the base-2 logarithm of `x` (log₂ x).
///
/// Returns the value `y` such that `2^y = x`.
///
/// ## Edge Cases
///
/// - If `x` is negative, the result is NaN.
/// - If `x` is ±0, the result is -∞.
/// - If `x` is 1.0, the result is 0.0.
/// - If `x` is +∞, the result is +∞.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::log2
pub fn log2(x: f64) -> f64 {
    x.log2()
}

// ---------------------------------------------------------------------------
// Base-10 Logarithm
// ---------------------------------------------------------------------------

/// Compute the base-10 logarithm of `x` (log₁₀ x).
///
/// Returns the value `y` such that `10^y = x`.
///
/// ## Edge Cases
///
/// - If `x` is negative, the result is NaN.
/// - If `x` is ±0, the result is -∞.
/// - If `x` is 1.0, the result is 0.0.
/// - If `x` is +∞, the result is +∞.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::log10
pub fn log10(x: f64) -> f64 {
    x.log10()
}

// ---------------------------------------------------------------------------
// Natural Logarithm of (1 + x)
// ---------------------------------------------------------------------------

/// Compute `ln(1 + x)` with greater precision for small `x`.
///
/// For values of `x` near zero, `ln_1p(x)` is more accurate than
/// computing `ln(1.0 + x)`, which can lose precision due to the
/// addition step.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is -1.0, the result is -∞.
/// - If `x` is less than -1.0, the result is NaN.
/// - If `x` is +∞, the result is +∞.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::ln_1p
pub fn ln_1p(x: f64) -> f64 {
    x.ln_1p()
}

// ---------------------------------------------------------------------------
// Power (floating-point exponent)
// ---------------------------------------------------------------------------

/// Compute `x` raised to the power of `y` (x^y).
///
/// Returns `x` raised to the floating-point power `y`.
///
/// ## Edge Cases
///
/// - `pow(0.0, y)` where `y > 0` returns 0.0.
/// - `pow(0.0, y)` where `y < 0` returns +∞.
/// - `pow(x, 0.0)` returns 1.0 for any `x` (including NaN).
/// - `pow(-1.0, ±∞)` returns 1.0.
/// - `pow(1.0, ±∞)` returns 1.0.
/// - `pow(x, NaN)` or `pow(NaN, y)` returns NaN (except where noted).
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::powf
pub fn pow(x: f64, y: f64) -> f64 {
    x.powf(y)
}

// ---------------------------------------------------------------------------
// Power (integer exponent)
// ---------------------------------------------------------------------------

/// Compute `x` raised to the integer power `n` (x^n).
///
/// Uses exponentiation by squaring, which is typically faster than
/// `pow(x, n as f64)` for integer exponents and avoids intermediate
/// rounding.
///
/// ## Edge Cases
///
/// - `powi(x, 0)` returns 1.0 for any `x` (including 0.0 and NaN).
/// - `powi(0.0, n)` where `n > 0` returns 0.0.
/// - `powi(0.0, n)` where `n < 0` returns +∞.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::powi
pub fn powi(x: f64, n: i32) -> f64 {
    x.powi(n)
}

// ===========================================================================
// Rounding Functions (f64)
// ===========================================================================

// ---------------------------------------------------------------------------
// Floor
// ---------------------------------------------------------------------------

/// Return the largest integer less than or equal to `x`.
///
/// Rounds toward negative infinity.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is ±∞, the result is ±∞.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::floor
pub fn floor(x: f64) -> f64 {
    x.floor()
}

// ---------------------------------------------------------------------------
// Ceil
// ---------------------------------------------------------------------------

/// Return the smallest integer greater than or equal to `x`.
///
/// Rounds toward positive infinity.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is ±∞, the result is ±∞.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::ceil
pub fn ceil(x: f64) -> f64 {
    x.ceil()
}

// ---------------------------------------------------------------------------
// Round
// ---------------------------------------------------------------------------

/// Return the nearest integer to `x`. Rounds half-way cases away from zero.
///
/// For example, `round(0.5) == 1.0` and `round(-0.5) == -1.0`.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is ±∞, the result is ±∞.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::round
pub fn round(x: f64) -> f64 {
    x.round()
}

// ---------------------------------------------------------------------------
// Trunc
// ---------------------------------------------------------------------------

/// Return the integer part of `x`, discarding any fractional part.
///
/// Rounds toward zero. Equivalent to `floor` for positive values and
/// `ceil` for negative values.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is ±∞, the result is ±∞.
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::trunc
pub fn trunc(x: f64) -> f64 {
    x.trunc()
}

// ---------------------------------------------------------------------------
// Fract
// ---------------------------------------------------------------------------

/// Return the fractional part of `x`.
///
/// Returns `x - trunc(x)`. The result is in the range `[0.0, 1.0)` for
/// positive `x` and `(-1.0, 0.0]` for negative `x`.
///
/// ## Edge Cases
///
/// - If `x` is ±0, the result is ±0 (preserves sign).
/// - If `x` is ±∞, the result is ±0 (preserves sign of input).
/// - If `x` is NaN, the result is NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::fract
pub fn fract(x: f64) -> f64 {
    x.fract()
}

// ===========================================================================
// Comparison Functions (f64)
// ===========================================================================

// ---------------------------------------------------------------------------
// Min of two f64
// ---------------------------------------------------------------------------

/// Return the smaller of two `f64` values.
///
/// Follows IEEE 754-2008 `minNum` semantics: if either argument is NaN,
/// the other is returned. If both are NaN, NaN is returned.
///
/// ## Edge Cases
///
/// - If either argument is NaN, the non-NaN value is returned.
/// - `-0.0` is considered less than `+0.0`.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::min
pub fn min_of(a: f64, b: f64) -> f64 {
    a.min(b)
}

// ---------------------------------------------------------------------------
// Max of two f64
// ---------------------------------------------------------------------------

/// Return the larger of two `f64` values.
///
/// Follows IEEE 754-2008 `maxNum` semantics: if either argument is NaN,
/// the other is returned. If both are NaN, NaN is returned.
///
/// ## Edge Cases
///
/// - If either argument is NaN, the non-NaN value is returned.
/// - `+0.0` is considered greater than `-0.0`.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::max
pub fn max_of(a: f64, b: f64) -> f64 {
    a.max(b)
}

// ===========================================================================
// Classification Functions (f64)
// ===========================================================================

// ---------------------------------------------------------------------------
// Is NaN
// ---------------------------------------------------------------------------

/// Return `true` if `x` is NaN (Not a Number).
///
/// This is the only reliable way to test for NaN, since NaN != NaN
/// by IEEE 754 definition.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::is_nan
pub fn is_nan(x: f64) -> bool {
    x.is_nan()
}

// ---------------------------------------------------------------------------
// Is Infinite
// ---------------------------------------------------------------------------

/// Return `true` if `x` is positive or negative infinity.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::is_infinite
pub fn is_infinite(x: f64) -> bool {
    x.is_infinite()
}

// ---------------------------------------------------------------------------
// Is Finite
// ---------------------------------------------------------------------------

/// Return `true` if `x` is neither infinite nor NaN.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::is_finite
pub fn is_finite(x: f64) -> bool {
    x.is_finite()
}

// ---------------------------------------------------------------------------
// Is Normal
// ---------------------------------------------------------------------------

/// Return `true` if `x` is a normal floating-point number.
///
/// A number is "normal" if it is neither zero, subnormal, infinite, nor NaN.
/// Normal numbers have full precision in their mantissa.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::is_normal
pub fn is_normal(x: f64) -> bool {
    x.is_normal()
}

// ---------------------------------------------------------------------------
// Signum
// ---------------------------------------------------------------------------

/// Return the sign of `x` as a floating-point number.
///
/// Returns:
/// - `1.0` if `x > 0`
/// - `-1.0` if `x < 0`
/// - `0.0` if `x == 0`
/// - NaN if `x` is NaN
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::signum
pub fn signum(x: f64) -> f64 {
    x.signum()
}

// ---------------------------------------------------------------------------
// Copy Sign
// ---------------------------------------------------------------------------

/// Return a value with the magnitude of `x` and the sign of `y`.
///
/// ## Edge Cases
///
/// - If `x` is NaN, the result is NaN with the sign of `y`.
/// - If `y` is NaN, the sign bit is treated as positive.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f64::copysign
pub fn copysign(x: f64, y: f64) -> f64 {
    x.copysign(y)
}

// ===========================================================================
// f32 Variants — Trigonometric
// ===========================================================================

/// Compute the sine of `x` radians (f32).
///
/// See [`sin`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::sin
pub fn sin_f32(x: f32) -> f32 {
    x.sin()
}

/// Compute the cosine of `x` radians (f32).
///
/// See [`cos`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::cos
pub fn cos_f32(x: f32) -> f32 {
    x.cos()
}

/// Compute the tangent of `x` radians (f32).
///
/// See [`tan`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::tan
pub fn tan_f32(x: f32) -> f32 {
    x.tan()
}

/// Compute the arc sine of `x` (f32).
///
/// See [`asin`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::asin
pub fn asin_f32(x: f32) -> f32 {
    x.asin()
}

/// Compute the arc cosine of `x` (f32).
///
/// See [`acos`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::acos
pub fn acos_f32(x: f32) -> f32 {
    x.acos()
}

/// Compute the arc tangent of `x` (f32).
///
/// See [`atan`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::atan
pub fn atan_f32(x: f32) -> f32 {
    x.atan()
}

/// Compute the four-quadrant arc tangent of `y / x` (f32).
///
/// See [`atan2`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::atan2
pub fn atan2_f32(y: f32, x: f32) -> f32 {
    y.atan2(x)
}

/// Compute the hyperbolic sine of `x` (f32).
///
/// See [`sinh`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::sinh
pub fn sinh_f32(x: f32) -> f32 {
    x.sinh()
}

/// Compute the hyperbolic cosine of `x` (f32).
///
/// See [`cosh`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::cosh
pub fn cosh_f32(x: f32) -> f32 {
    x.cosh()
}

/// Compute the hyperbolic tangent of `x` (f32).
///
/// See [`tanh`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::tanh
pub fn tanh_f32(x: f32) -> f32 {
    x.tanh()
}

// ===========================================================================
// f32 Variants — Exponential / Logarithmic
// ===========================================================================

/// Compute the square root of `x` (f32).
///
/// See [`sqrt`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::sqrt
pub fn sqrt_f32(x: f32) -> f32 {
    x.sqrt()
}

/// Compute the cube root of `x` (f32).
///
/// See [`cbrt`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::cbrt
pub fn cbrt_f32(x: f32) -> f32 {
    x.cbrt()
}

/// Compute `e^x` (f32).
///
/// See [`exp`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::exp
pub fn exp_f32(x: f32) -> f32 {
    x.exp()
}

/// Compute `2^x` (f32).
///
/// See [`exp2`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::exp2
pub fn exp2_f32(x: f32) -> f32 {
    x.exp2()
}

/// Compute `e^x - 1` with greater precision for small `x` (f32).
///
/// See [`exp_m1`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::exp_m1
pub fn exp_m1_f32(x: f32) -> f32 {
    x.exp_m1()
}

/// Compute the natural logarithm of `x` (f32).
///
/// See [`ln`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::ln
pub fn ln_f32(x: f32) -> f32 {
    x.ln()
}

/// Compute the base-2 logarithm of `x` (f32).
///
/// See [`log2`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::log2
pub fn log2_f32(x: f32) -> f32 {
    x.log2()
}

/// Compute the base-10 logarithm of `x` (f32).
///
/// See [`log10`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::log10
pub fn log10_f32(x: f32) -> f32 {
    x.log10()
}

/// Compute `ln(1 + x)` with greater precision for small `x` (f32).
///
/// See [`ln_1p`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::ln_1p
pub fn ln_1p_f32(x: f32) -> f32 {
    x.ln_1p()
}

/// Compute `x` raised to the power of `y` (f32).
///
/// See [`pow`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::powf
pub fn pow_f32(x: f32, y: f32) -> f32 {
    x.powf(y)
}

/// Compute `x` raised to the integer power `n` (f32).
///
/// See [`powi`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::powi
pub fn powi_f32(x: f32, n: i32) -> f32 {
    x.powi(n)
}

// ===========================================================================
// f32 Variants — Rounding
// ===========================================================================

/// Return the largest integer less than or equal to `x` (f32).
///
/// See [`floor`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::floor
pub fn floor_f32(x: f32) -> f32 {
    x.floor()
}

/// Return the smallest integer greater than or equal to `x` (f32).
///
/// See [`ceil`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::ceil
pub fn ceil_f32(x: f32) -> f32 {
    x.ceil()
}

/// Return the nearest integer to `x`, rounding half away from zero (f32).
///
/// See [`round`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::round
pub fn round_f32(x: f32) -> f32 {
    x.round()
}

/// Return the integer part of `x`, discarding the fractional part (f32).
///
/// See [`trunc`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::trunc
pub fn trunc_f32(x: f32) -> f32 {
    x.trunc()
}

/// Return the fractional part of `x` (f32).
///
/// See [`fract`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::fract
pub fn fract_f32(x: f32) -> f32 {
    x.fract()
}

// ===========================================================================
// f32 Variants — Comparison
// ===========================================================================

/// Return the smaller of two `f32` values.
///
/// See [`min_of`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::min
pub fn min_of_f32(a: f32, b: f32) -> f32 {
    a.min(b)
}

/// Return the larger of two `f32` values.
///
/// See [`max_of`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::max
pub fn max_of_f32(a: f32, b: f32) -> f32 {
    a.max(b)
}

// ===========================================================================
// f32 Variants — Classification
// ===========================================================================

/// Return `true` if `x` is NaN (f32).
///
/// See [`is_nan`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::is_nan
pub fn is_nan_f32(x: f32) -> bool {
    x.is_nan()
}

/// Return `true` if `x` is positive or negative infinity (f32).
///
/// See [`is_infinite`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::is_infinite
pub fn is_infinite_f32(x: f32) -> bool {
    x.is_infinite()
}

/// Return `true` if `x` is neither infinite nor NaN (f32).
///
/// See [`is_finite`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::is_finite
pub fn is_finite_f32(x: f32) -> bool {
    x.is_finite()
}

/// Return `true` if `x` is a normal floating-point number (f32).
///
/// See [`is_normal`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::is_normal
pub fn is_normal_f32(x: f32) -> bool {
    x.is_normal()
}

/// Return the sign of `x` as a floating-point number (f32).
///
/// See [`signum`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::signum
pub fn signum_f32(x: f32) -> f32 {
    x.signum()
}

/// Return a value with the magnitude of `x` and the sign of `y` (f32).
///
/// See [`copysign`] for full documentation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: delegates to Rust std f32::copysign
pub fn copysign_f32(x: f32, y: f32) -> f32 {
    x.copysign(y)
}

// ===========================================================================
// Capability Descriptor for Math Operations
// ===========================================================================

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

// ===========================================================================
// Tests
// ===========================================================================

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

    // --- trigonometric f64 tests ---

    #[test]
    fn test_sin_basic() {
        let tolerance = 1e-10;
        assert!((sin(0.0) - 0.0).abs() < tolerance);
        assert!((sin(PI / 2.0) - 1.0).abs() < tolerance);
        assert!((sin(PI) - 0.0).abs() < tolerance);
    }

    #[test]
    fn test_cos_basic() {
        let tolerance = 1e-10;
        assert!((cos(0.0) - 1.0).abs() < tolerance);
        assert!((cos(PI) - (-1.0)).abs() < tolerance);
        assert!((cos(PI / 2.0) - 0.0).abs() < tolerance);
    }

    #[test]
    fn test_tan_basic() {
        let tolerance = 1e-10;
        assert!((tan(0.0) - 0.0).abs() < tolerance);
        assert!((tan(PI / 4.0) - 1.0).abs() < tolerance);
    }

    #[test]
    fn test_asin_basic() {
        let tolerance = 1e-10;
        assert!((asin(0.0) - 0.0).abs() < tolerance);
        assert!((asin(1.0) - PI / 2.0).abs() < tolerance);
    }

    #[test]
    fn test_acos_basic() {
        let tolerance = 1e-10;
        assert!((acos(1.0) - 0.0).abs() < tolerance);
        assert!((acos(0.0) - PI / 2.0).abs() < tolerance);
    }

    #[test]
    fn test_atan_basic() {
        let tolerance = 1e-10;
        assert!((atan(0.0) - 0.0).abs() < tolerance);
        assert!((atan(1.0) - PI / 4.0).abs() < tolerance);
    }

    #[test]
    fn test_atan2_basic() {
        let tolerance = 1e-10;
        assert!((atan2(1.0, 1.0) - PI / 4.0).abs() < tolerance);
        assert!((atan2(1.0, 0.0) - PI / 2.0).abs() < tolerance);
        assert!((atan2(0.0, -1.0) - PI).abs() < tolerance);
    }

    #[test]
    fn test_sinh_basic() {
        let tolerance = 1e-10;
        assert!((sinh(0.0) - 0.0).abs() < tolerance);
    }

    #[test]
    fn test_cosh_basic() {
        let tolerance = 1e-10;
        assert!((cosh(0.0) - 1.0).abs() < tolerance);
    }

    #[test]
    fn test_tanh_basic() {
        let tolerance = 1e-10;
        assert!((tanh(0.0) - 0.0).abs() < tolerance);
    }

    #[test]
    fn test_trig_nan_propagation() {
        assert!(sin(f64::NAN).is_nan());
        assert!(cos(f64::NAN).is_nan());
        assert!(tan(f64::NAN).is_nan());
        assert!(asin(f64::NAN).is_nan());
        assert!(acos(f64::NAN).is_nan());
        assert!(atan(f64::NAN).is_nan());
        assert!(atan2(f64::NAN, 1.0).is_nan());
        assert!(sinh(f64::NAN).is_nan());
        assert!(cosh(f64::NAN).is_nan());
        assert!(tanh(f64::NAN).is_nan());
    }

    #[test]
    fn test_trig_out_of_range() {
        // asin/acos with |x| > 1 should return NaN
        assert!(asin(2.0).is_nan());
        assert!(asin(-2.0).is_nan());
        assert!(acos(2.0).is_nan());
        assert!(acos(-2.0).is_nan());
    }

    // --- exponential/logarithmic f64 tests ---

    #[test]
    fn test_sqrt_basic() {
        let tolerance = 1e-10;
        assert!((sqrt(4.0) - 2.0).abs() < tolerance);
        assert!((sqrt(0.0) - 0.0).abs() < tolerance);
        assert!((sqrt(1.0) - 1.0).abs() < tolerance);
    }

    #[test]
    fn test_sqrt_negative() {
        assert!(sqrt(-1.0).is_nan());
    }

    #[test]
    fn test_cbrt_basic() {
        let tolerance = 1e-10;
        assert!((cbrt(27.0) - 3.0).abs() < tolerance);
        assert!((cbrt(-8.0) - (-2.0)).abs() < tolerance);
        assert!((cbrt(0.0) - 0.0).abs() < tolerance);
    }

    #[test]
    fn test_exp_basic() {
        let tolerance = 1e-10;
        assert!((exp(0.0) - 1.0).abs() < tolerance);
        assert!((exp(1.0) - E).abs() < tolerance);
    }

    #[test]
    fn test_exp2_basic() {
        let tolerance = 1e-10;
        assert!((exp2(0.0) - 1.0).abs() < tolerance);
        assert!((exp2(1.0) - 2.0).abs() < tolerance);
        assert!((exp2(3.0) - 8.0).abs() < tolerance);
    }

    #[test]
    fn test_exp_m1_basic() {
        let tolerance = 1e-10;
        assert!((exp_m1(0.0) - 0.0).abs() < tolerance);
    }

    #[test]
    fn test_ln_basic() {
        let tolerance = 1e-10;
        assert!((ln(1.0) - 0.0).abs() < tolerance);
        assert!((ln(E) - 1.0).abs() < tolerance);
    }

    #[test]
    fn test_log2_basic() {
        let tolerance = 1e-10;
        assert!((log2(1.0) - 0.0).abs() < tolerance);
        assert!((log2(2.0) - 1.0).abs() < tolerance);
        assert!((log2(8.0) - 3.0).abs() < tolerance);
    }

    #[test]
    fn test_log10_basic() {
        let tolerance = 1e-10;
        assert!((log10(1.0) - 0.0).abs() < tolerance);
        assert!((log10(10.0) - 1.0).abs() < tolerance);
        assert!((log10(100.0) - 2.0).abs() < tolerance);
    }

    #[test]
    fn test_ln_1p_basic() {
        let tolerance = 1e-10;
        assert!((ln_1p(0.0) - 0.0).abs() < tolerance);
        assert!((ln_1p(E - 1.0) - 1.0).abs() < 1e-8);
    }

    #[test]
    fn test_pow_basic() {
        let tolerance = 1e-10;
        assert!((pow(2.0, 3.0) - 8.0).abs() < tolerance);
        assert!((pow(4.0, 0.5) - 2.0).abs() < tolerance);
    }

    #[test]
    fn test_powi_basic() {
        let tolerance = 1e-10;
        assert!((powi(2.0, 3) - 8.0).abs() < tolerance);
        assert!((powi(3.0, 0) - 1.0).abs() < tolerance);
        assert!((powi(2.0, -1) - 0.5).abs() < tolerance);
    }

    #[test]
    fn test_exp_log_roundtrip() {
        let tolerance = 1e-10;
        for x in [0.5, 1.0, 2.0, 10.0, 100.0] {
            assert!((ln(exp(x)) - x).abs() < tolerance);
            assert!((log2(exp2(x)) - x).abs() < tolerance);
            assert!((log10(x) - ln(x) / LN_10).abs() < tolerance);
        }
    }

    // --- rounding f64 tests ---

    #[test]
    fn test_floor_basic() {
        assert_eq!(floor(3.7), 3.0);
        assert_eq!(floor(-3.7), -4.0);
        assert_eq!(floor(0.0), 0.0);
    }

    #[test]
    fn test_ceil_basic() {
        assert_eq!(ceil(3.2), 4.0);
        assert_eq!(ceil(-3.2), -3.0);
        assert_eq!(ceil(0.0), 0.0);
    }

    #[test]
    fn test_round_basic() {
        assert_eq!(round(3.5), 4.0);
        assert_eq!(round(3.4), 3.0);
        assert_eq!(round(-3.5), -4.0);
        assert_eq!(round(-3.4), -3.0);
    }

    #[test]
    fn test_trunc_basic() {
        assert_eq!(trunc(3.7), 3.0);
        assert_eq!(trunc(-3.7), -3.0);
        assert_eq!(trunc(0.0), 0.0);
    }

    #[test]
    fn test_fract_basic() {
        let tolerance = 1e-10;
        assert!((fract(3.7) - 0.7).abs() < tolerance);
        assert!((fract(-3.7) - (-0.7)).abs() < tolerance);
        assert!((fract(0.0) - 0.0).abs() < tolerance);
    }

    #[test]
    fn test_rounding_special() {
        assert!(floor(f64::NAN).is_nan());
        assert!(ceil(f64::NAN).is_nan());
        assert!(round(f64::NAN).is_nan());
        assert!(trunc(f64::NAN).is_nan());
        assert!(fract(f64::NAN).is_nan());
    }

    // --- comparison f64 tests ---

    #[test]
    fn test_min_of_basic() {
        assert_eq!(min_of(1.0, 2.0), 1.0);
        assert_eq!(min_of(2.0, 1.0), 1.0);
        assert_eq!(min_of(5.0, 5.0), 5.0);
    }

    #[test]
    fn test_max_of_basic() {
        assert_eq!(max_of(1.0, 2.0), 2.0);
        assert_eq!(max_of(2.0, 1.0), 2.0);
        assert_eq!(max_of(5.0, 5.0), 5.0);
    }

    #[test]
    fn test_min_max_nan() {
        // min_of/max_of should return the non-NaN value
        assert_eq!(min_of(f64::NAN, 1.0), 1.0);
        assert_eq!(min_of(1.0, f64::NAN), 1.0);
        assert_eq!(max_of(f64::NAN, 1.0), 1.0);
        assert_eq!(max_of(1.0, f64::NAN), 1.0);
    }

    // --- classification f64 tests ---

    #[test]
    fn test_is_nan() {
        assert!(is_nan(f64::NAN));
        assert!(!is_nan(0.0));
        assert!(!is_nan(1.0));
        assert!(!is_nan(f64::INFINITY));
    }

    #[test]
    fn test_is_infinite() {
        assert!(is_infinite(f64::INFINITY));
        assert!(is_infinite(f64::NEG_INFINITY));
        assert!(!is_infinite(0.0));
        assert!(!is_infinite(f64::NAN));
    }

    #[test]
    fn test_is_finite() {
        assert!(is_finite(0.0));
        assert!(is_finite(1.0));
        assert!(is_finite(-1e100));
        assert!(!is_finite(f64::INFINITY));
        assert!(!is_finite(f64::NAN));
    }

    #[test]
    fn test_is_normal() {
        assert!(is_normal(1.0));
        assert!(is_normal(-1.0));
        assert!(is_normal(1e100));
        assert!(!is_normal(0.0));
        assert!(!is_normal(f64::INFINITY));
        assert!(!is_normal(f64::NAN));
    }

    #[test]
    fn test_signum() {
        assert_eq!(signum(42.0), 1.0);
        assert_eq!(signum(-42.0), -1.0);
        assert_eq!(signum(0.0), 1.0); // Rust signum: +0.0 has positive sign
        assert_eq!(signum(-0.0), -1.0); // -0.0 has negative sign
        assert!(signum(f64::NAN).is_nan());
    }

    #[test]
    fn test_copysign() {
        assert_eq!(copysign(3.0, 1.0), 3.0);
        assert_eq!(copysign(3.0, -1.0), -3.0);
        assert_eq!(copysign(-3.0, 1.0), 3.0);
        assert_eq!(copysign(-3.0, -1.0), -3.0);
    }

    // --- constants tests ---

    #[test]
    fn test_constants_f64() {
        let tolerance = 1e-10;
        assert!((PI - 3.14159265358979323846).abs() < tolerance);
        assert!((TAU - 2.0 * PI).abs() < tolerance);
        assert!((E - 2.71828182845904523536).abs() < tolerance);
        assert!((LN_2 - 0.69314718055994530942).abs() < tolerance);
        assert!((LN_10 - 2.30258509299404568402).abs() < tolerance);
        assert!((LOG2_E - 1.44269504088896340736).abs() < tolerance);
        assert!((LOG10_E - 0.43429448190325182765).abs() < tolerance);
        assert!((SQRT_2 - 1.41421356237309504880).abs() < tolerance);
        assert!((FRAC_1_SQRT_2 - 1.0 / SQRT_2).abs() < tolerance);
    }

    #[test]
    fn test_constants_f32() {
        let tolerance = 1e-5;
        assert!((PI_F32 - 3.14159265f32).abs() < tolerance);
        assert!((TAU_F32 - 2.0f32 * PI_F32).abs() < tolerance);
        assert!((E_F32 - 2.71828182f32).abs() < tolerance);
    }

    // --- f32 trigonometric tests ---

    #[test]
    fn test_sin_f32_basic() {
        let tolerance = 1e-5;
        assert!((sin_f32(0.0f32) - 0.0f32).abs() < tolerance);
        assert!((sin_f32(PI_F32 / 2.0f32) - 1.0f32).abs() < tolerance);
    }

    #[test]
    fn test_cos_f32_basic() {
        let tolerance = 1e-5;
        assert!((cos_f32(0.0f32) - 1.0f32).abs() < tolerance);
    }

    #[test]
    fn test_tan_f32_basic() {
        let tolerance = 1e-5;
        assert!((tan_f32(0.0f32) - 0.0f32).abs() < tolerance);
    }

    #[test]
    fn test_trig_f32_nan_propagation() {
        assert!(sin_f32(f32::NAN).is_nan());
        assert!(cos_f32(f32::NAN).is_nan());
        assert!(tan_f32(f32::NAN).is_nan());
        assert!(asin_f32(f32::NAN).is_nan());
        assert!(acos_f32(f32::NAN).is_nan());
        assert!(atan_f32(f32::NAN).is_nan());
        assert!(sinh_f32(f32::NAN).is_nan());
        assert!(cosh_f32(f32::NAN).is_nan());
        assert!(tanh_f32(f32::NAN).is_nan());
    }

    // --- f32 exponential/logarithmic tests ---

    #[test]
    fn test_sqrt_f32_basic() {
        let tolerance = 1e-5;
        assert!((sqrt_f32(4.0f32) - 2.0f32).abs() < tolerance);
        assert!(sqrt_f32(-1.0f32).is_nan());
    }

    #[test]
    fn test_cbrt_f32_basic() {
        let tolerance = 1e-5;
        assert!((cbrt_f32(27.0f32) - 3.0f32).abs() < tolerance);
    }

    #[test]
    fn test_exp_f32_basic() {
        let tolerance = 1e-5;
        assert!((exp_f32(0.0f32) - 1.0f32).abs() < tolerance);
    }

    #[test]
    fn test_ln_f32_basic() {
        let tolerance = 1e-5;
        assert!((ln_f32(1.0f32) - 0.0f32).abs() < tolerance);
    }

    #[test]
    fn test_pow_f32_basic() {
        let tolerance = 1e-5;
        assert!((pow_f32(2.0f32, 3.0f32) - 8.0f32).abs() < tolerance);
    }

    #[test]
    fn test_powi_f32_basic() {
        let tolerance = 1e-5;
        assert!((powi_f32(2.0f32, 3) - 8.0f32).abs() < tolerance);
    }

    // --- f32 rounding tests ---

    #[test]
    fn test_floor_f32_basic() {
        assert_eq!(floor_f32(3.7f32), 3.0f32);
        assert_eq!(floor_f32(-3.7f32), -4.0f32);
    }

    #[test]
    fn test_ceil_f32_basic() {
        assert_eq!(ceil_f32(3.2f32), 4.0f32);
        assert_eq!(ceil_f32(-3.2f32), -3.0f32);
    }

    #[test]
    fn test_round_f32_basic() {
        assert_eq!(round_f32(3.5f32), 4.0f32);
    }

    #[test]
    fn test_trunc_f32_basic() {
        assert_eq!(trunc_f32(3.7f32), 3.0f32);
    }

    #[test]
    fn test_fract_f32_basic() {
        let tolerance = 1e-5;
        assert!((fract_f32(3.7f32) - 0.7f32).abs() < tolerance);
    }

    // --- f32 comparison tests ---

    #[test]
    fn test_min_of_f32_basic() {
        assert_eq!(min_of_f32(1.0f32, 2.0f32), 1.0f32);
    }

    #[test]
    fn test_max_of_f32_basic() {
        assert_eq!(max_of_f32(1.0f32, 2.0f32), 2.0f32);
    }

    // --- f32 classification tests ---

    #[test]
    fn test_is_nan_f32() {
        assert!(is_nan_f32(f32::NAN));
        assert!(!is_nan_f32(0.0f32));
    }

    #[test]
    fn test_is_infinite_f32() {
        assert!(is_infinite_f32(f32::INFINITY));
        assert!(!is_infinite_f32(1.0f32));
    }

    #[test]
    fn test_is_finite_f32() {
        assert!(is_finite_f32(0.0f32));
        assert!(!is_finite_f32(f32::INFINITY));
    }

    #[test]
    fn test_is_normal_f32() {
        assert!(is_normal_f32(1.0f32));
        assert!(!is_normal_f32(0.0f32));
    }

    #[test]
    fn test_signum_f32() {
        assert_eq!(signum_f32(42.0f32), 1.0f32);
        assert_eq!(signum_f32(-42.0f32), -1.0f32);
    }

    #[test]
    fn test_copysign_f32() {
        assert_eq!(copysign_f32(3.0f32, -1.0f32), -3.0f32);
        assert_eq!(copysign_f32(-3.0f32, 1.0f32), 3.0f32);
    }

    // --- capd tests ---

    #[test]
    fn test_math_capd() {
        let capd = math_capd();
        assert!(capd.has(CapFlag::Read));
        assert!(capd.has(CapFlag::Compare));
        assert!(!capd.has(CapFlag::Write));
    }

    // =======================================================================
    // Comprehensive additional tests for expanded math module
    // =======================================================================

    // --- Comprehensive trigonometric f64 tests ---

    #[test]
    fn test_sin_comprehensive() {
        let tol = 1e-10;
        // sin(0) = 0
        assert!((sin(0.0) - 0.0).abs() < tol);
        // sin(PI/2) ≈ 1
        assert!((sin(PI / 2.0) - 1.0).abs() < tol);
        // sin(PI) ≈ 0
        assert!((sin(PI) - 0.0).abs() < tol);
        // sin(3*PI/2) ≈ -1
        assert!((sin(3.0 * PI / 2.0) - (-1.0)).abs() < tol);
        // sin(-PI/2) ≈ -1
        assert!((sin(-PI / 2.0) - (-1.0)).abs() < tol);
        // sin(PI/6) ≈ 0.5
        assert!((sin(PI / 6.0) - 0.5).abs() < tol);
    }

    #[test]
    fn test_cos_comprehensive() {
        let tol = 1e-10;
        // cos(0) = 1
        assert!((cos(0.0) - 1.0).abs() < tol);
        // cos(PI/3) ≈ 0.5
        assert!((cos(PI / 3.0) - 0.5).abs() < tol);
        // cos(PI/2) ≈ 0
        assert!((cos(PI / 2.0) - 0.0).abs() < tol);
        // cos(PI) ≈ -1
        assert!((cos(PI) - (-1.0)).abs() < tol);
        // cos(-PI) ≈ -1
        assert!((cos(-PI) - (-1.0)).abs() < tol);
        // cos(2*PI) ≈ 1
        assert!((cos(2.0 * PI) - 1.0).abs() < tol);
    }

    #[test]
    fn test_tan_comprehensive() {
        let tol = 1e-10;
        // tan(0) = 0
        assert!((tan(0.0) - 0.0).abs() < tol);
        // tan(PI/4) ≈ 1
        assert!((tan(PI / 4.0) - 1.0).abs() < tol);
        // tan(-PI/4) ≈ -1
        assert!((tan(-PI / 4.0) - (-1.0)).abs() < tol);
    }

    #[test]
    fn test_sin_cos_identity() {
        // sin²(x) + cos²(x) = 1 for various x
        let tol = 1e-10;
        for x in [0.0, 0.5, 1.0, PI / 4.0, PI / 2.0, PI, 2.0 * PI, -1.0] {
            let sum_sq = sin(x) * sin(x) + cos(x) * cos(x);
            assert!((sum_sq - 1.0).abs() < tol, "sin²({}) + cos²({}) = {}", x, x, sum_sq);
        }
    }

    #[test]
    fn test_atan2_comprehensive() {
        let tol = 1e-10;
        // Quadrant I
        assert!((atan2(1.0, 1.0) - PI / 4.0).abs() < tol);
        // Quadrant II
        assert!((atan2(1.0, -1.0) - 3.0 * PI / 4.0).abs() < tol);
        // Quadrant III
        assert!((atan2(-1.0, -1.0) - (-3.0 * PI / 4.0)).abs() < tol);
        // Quadrant IV
        assert!((atan2(-1.0, 1.0) - (-PI / 4.0)).abs() < tol);
        // Positive y-axis
        assert!((atan2(1.0, 0.0) - PI / 2.0).abs() < tol);
        // Negative y-axis
        assert!((atan2(-1.0, 0.0) - (-PI / 2.0)).abs() < tol);
    }

    #[test]
    fn test_sinh_cosh_tanh_comprehensive() {
        let tol = 1e-10;
        // sinh(0) = 0, cosh(0) = 1, tanh(0) = 0
        assert!((sinh(0.0) - 0.0).abs() < tol);
        assert!((cosh(0.0) - 1.0).abs() < tol);
        assert!((tanh(0.0) - 0.0).abs() < tol);
        // tanh(x) = sinh(x)/cosh(x)
        for x in [0.5, 1.0, 2.0, -1.0] {
            let expected = sinh(x) / cosh(x);
            assert!((tanh(x) - expected).abs() < tol);
        }
        // cosh²(x) - sinh²(x) = 1
        for x in [0.5, 1.0, 2.0] {
            let diff = cosh(x) * cosh(x) - sinh(x) * sinh(x);
            assert!((diff - 1.0).abs() < tol);
        }
    }

    #[test]
    fn test_trig_infinity() {
        // sin/cos of infinity should be NaN
        assert!(sin(f64::INFINITY).is_nan());
        assert!(sin(f64::NEG_INFINITY).is_nan());
        assert!(cos(f64::INFINITY).is_nan());
        assert!(cos(f64::NEG_INFINITY).is_nan());
        assert!(tan(f64::INFINITY).is_nan());
        assert!(tan(f64::NEG_INFINITY).is_nan());
    }

    // --- Comprehensive exponential/logarithmic f64 tests ---

    #[test]
    fn test_sqrt_comprehensive() {
        let tol = 1e-10;
        assert!((sqrt(4.0) - 2.0).abs() < tol);
        assert!((sqrt(9.0) - 3.0).abs() < tol);
        assert!((sqrt(2.0) - SQRT_2).abs() < tol);
        assert!((sqrt(0.25) - 0.5).abs() < tol);
        assert!((sqrt(0.0) - 0.0).abs() < tol);
        assert!((sqrt(1.0) - 1.0).abs() < tol);
        // sqrt of negative is NaN
        assert!(sqrt(-1.0).is_nan());
        assert!(sqrt(-0.001).is_nan());
    }

    #[test]
    fn test_cbrt_comprehensive() {
        let tol = 1e-10;
        assert!((cbrt(27.0) - 3.0).abs() < tol);
        assert!((cbrt(-8.0) - (-2.0)).abs() < tol);
        assert!((cbrt(0.0) - 0.0).abs() < tol);
        assert!((cbrt(1.0) - 1.0).abs() < tol);
        assert!((cbrt(-1.0) - (-1.0)).abs() < tol);
        assert!((cbrt(0.125) - 0.5).abs() < tol);
    }

    #[test]
    fn test_exp_comprehensive() {
        let tol = 1e-10;
        assert!((exp(0.0) - 1.0).abs() < tol);
        assert!((exp(1.0) - E).abs() < tol);
        assert!((exp(-1.0) - (1.0 / E)).abs() < tol);
        assert!((exp(2.0) - E * E).abs() < 1e-9);
        // exp of large negative approaches 0
        assert!(exp(-100.0) < 1e-40);
        // exp of -inf is 0
        assert_eq!(exp(f64::NEG_INFINITY), 0.0);
        // exp of +inf is +inf
        assert_eq!(exp(f64::INFINITY), f64::INFINITY);
    }

    #[test]
    fn test_exp2_comprehensive() {
        let tol = 1e-10;
        assert!((exp2(0.0) - 1.0).abs() < tol);
        assert!((exp2(1.0) - 2.0).abs() < tol);
        assert!((exp2(2.0) - 4.0).abs() < tol);
        assert!((exp2(3.0) - 8.0).abs() < tol);
        assert!((exp2(10.0) - 1024.0).abs() < tol);
        assert!((exp2(-1.0) - 0.5).abs() < tol);
    }

    #[test]
    fn test_exp_m1_comprehensive() {
        let tol = 1e-10;
        assert!((exp_m1(0.0) - 0.0).abs() < tol);
        // For small x, exp_m1(x) ≈ x + x²/2
        let x = 0.001;
        assert!((exp_m1(x) - (x + x * x / 2.0)).abs() < 1e-9);
        // exp_m1(1) = e - 1
        assert!((exp_m1(1.0) - (E - 1.0)).abs() < tol);
    }

    #[test]
    fn test_ln_comprehensive() {
        let tol = 1e-10;
        assert!((ln(1.0) - 0.0).abs() < tol);
        assert!((ln(E) - 1.0).abs() < tol);
        assert!((ln(E * E) - 2.0).abs() < 1e-9);
        // ln of 0 is -inf
        assert_eq!(ln(0.0), f64::NEG_INFINITY);
        // ln of negative is NaN
        assert!(ln(-1.0).is_nan());
    }

    #[test]
    fn test_log2_comprehensive() {
        let tol = 1e-10;
        assert!((log2(1.0) - 0.0).abs() < tol);
        assert!((log2(2.0) - 1.0).abs() < tol);
        assert!((log2(4.0) - 2.0).abs() < tol);
        assert!((log2(8.0) - 3.0).abs() < tol);
        assert!((log2(1024.0) - 10.0).abs() < tol);
        assert!((log2(0.5) - (-1.0)).abs() < tol);
    }

    #[test]
    fn test_log10_comprehensive() {
        let tol = 1e-10;
        assert!((log10(1.0) - 0.0).abs() < tol);
        assert!((log10(10.0) - 1.0).abs() < tol);
        assert!((log10(100.0) - 2.0).abs() < tol);
        assert!((log10(1000.0) - 3.0).abs() < tol);
        assert!((log10(0.1) - (-1.0)).abs() < tol);
    }

    #[test]
    fn test_ln_1p_comprehensive() {
        let tol = 1e-10;
        assert!((ln_1p(0.0) - 0.0).abs() < tol);
        // ln_1p(E-1) should be close to 1.0
        assert!((ln_1p(E - 1.0) - 1.0).abs() < 1e-8);
        // For small x, ln_1p(x) ≈ x
        let x = 1e-10;
        assert!((ln_1p(x) - x).abs() < 1e-15);
        // ln_1p(-1.0) = -inf
        assert_eq!(ln_1p(-1.0), f64::NEG_INFINITY);
        // ln_1p(< -1.0) = NaN
        assert!(ln_1p(-2.0).is_nan());
    }

    #[test]
    fn test_pow_comprehensive() {
        let tol = 1e-10;
        assert!((pow(2.0, 3.0) - 8.0).abs() < tol);
        assert!((pow(4.0, 0.5) - 2.0).abs() < tol);
        assert!((pow(10.0, 0.0) - 1.0).abs() < tol);
        assert!((pow(0.0, 5.0) - 0.0).abs() < tol);
        assert!((pow(1.0, 100.0) - 1.0).abs() < tol);
        assert!((pow(2.0, -1.0) - 0.5).abs() < tol);
    }

    #[test]
    fn test_powi_comprehensive() {
        let tol = 1e-10;
        assert!((powi(2.0, 3) - 8.0).abs() < tol);
        assert!((powi(3.0, 0) - 1.0).abs() < tol);
        assert!((powi(2.0, -1) - 0.5).abs() < tol);
        assert!((powi(5.0, 2) - 25.0).abs() < tol);
        assert!((powi(10.0, 6) - 1000000.0).abs() < tol);
        assert!((powi(0.0, 0) - 1.0).abs() < tol);
    }

    // --- Comprehensive rounding f64 tests ---

    #[test]
    fn test_floor_comprehensive() {
        assert_eq!(floor(3.7), 3.0);
        assert_eq!(floor(3.0), 3.0);
        assert_eq!(floor(-3.7), -4.0);
        assert_eq!(floor(-3.0), -3.0);
        assert_eq!(floor(0.0), 0.0);
        assert_eq!(floor(0.1), 0.0);
        assert_eq!(floor(-0.1), -1.0);
        assert_eq!(floor(999.999), 999.0);
    }

    #[test]
    fn test_ceil_comprehensive() {
        assert_eq!(ceil(3.2), 4.0);
        assert_eq!(ceil(3.0), 3.0);
        assert_eq!(ceil(-3.2), -3.0);
        assert_eq!(ceil(-3.0), -3.0);
        assert_eq!(ceil(0.0), 0.0);
        assert_eq!(ceil(0.1), 1.0);
        assert_eq!(ceil(-0.1), 0.0);
        assert_eq!(ceil(999.001), 1000.0);
    }

    #[test]
    fn test_round_comprehensive() {
        assert_eq!(round(3.5), 4.0);
        assert_eq!(round(3.4), 3.0);
        assert_eq!(round(3.6), 4.0);
        assert_eq!(round(-3.5), -4.0);
        assert_eq!(round(-3.4), -3.0);
        assert_eq!(round(0.5), 1.0);
        assert_eq!(round(-0.5), -1.0);
        assert_eq!(round(0.0), 0.0);
        assert_eq!(round(2.5), 3.0); // rounds half away from zero
    }

    #[test]
    fn test_trunc_comprehensive() {
        assert_eq!(trunc(3.7), 3.0);
        assert_eq!(trunc(3.0), 3.0);
        assert_eq!(trunc(-3.7), -3.0);
        assert_eq!(trunc(-3.0), -3.0);
        assert_eq!(trunc(0.0), 0.0);
        assert_eq!(trunc(0.9), 0.0);
        assert_eq!(trunc(-0.9), 0.0);
    }

    #[test]
    fn test_fract_comprehensive() {
        let tol = 1e-10;
        assert!((fract(3.7) - 0.7).abs() < tol);
        assert!((fract(-3.7) - (-0.7)).abs() < tol);
        assert!((fract(0.0) - 0.0).abs() < tol);
        assert!((fract(1.0) - 0.0).abs() < tol);
        assert!((fract(-1.0) - 0.0).abs() < tol);
        assert!((fract(0.5) - 0.5).abs() < tol);
    }

    #[test]
    fn test_rounding_infinity() {
        // floor/ceil/round/trunc of ±∞ should be ±∞
        assert_eq!(floor(f64::INFINITY), f64::INFINITY);
        assert_eq!(floor(f64::NEG_INFINITY), f64::NEG_INFINITY);
        assert_eq!(ceil(f64::INFINITY), f64::INFINITY);
        assert_eq!(ceil(f64::NEG_INFINITY), f64::NEG_INFINITY);
        assert_eq!(round(f64::INFINITY), f64::INFINITY);
        assert_eq!(round(f64::NEG_INFINITY), f64::NEG_INFINITY);
        assert_eq!(trunc(f64::INFINITY), f64::INFINITY);
        assert_eq!(trunc(f64::NEG_INFINITY), f64::NEG_INFINITY);
    }

    // --- Comprehensive classification f64 tests ---

    #[test]
    fn test_is_nan_comprehensive() {
        assert!(is_nan(f64::NAN));
        assert!(!is_nan(0.0));
        assert!(!is_nan(1.0));
        assert!(!is_nan(-1.0));
        assert!(!is_nan(f64::INFINITY));
        assert!(!is_nan(f64::NEG_INFINITY));
        assert!(!is_nan(1e100));
        assert!(!is_nan(-1e100));
    }

    #[test]
    fn test_is_infinite_comprehensive() {
        assert!(is_infinite(f64::INFINITY));
        assert!(is_infinite(f64::NEG_INFINITY));
        assert!(!is_infinite(0.0));
        assert!(!is_infinite(1.0));
        assert!(!is_infinite(-1.0));
        assert!(!is_infinite(f64::NAN));
        assert!(!is_infinite(1e308));
    }

    #[test]
    fn test_is_finite_comprehensive() {
        assert!(is_finite(0.0));
        assert!(is_finite(1.0));
        assert!(is_finite(-1.0));
        assert!(is_finite(1e100));
        assert!(is_finite(-1e100));
        assert!(is_finite(f64::MIN_POSITIVE));
        assert!(!is_finite(f64::INFINITY));
        assert!(!is_finite(f64::NEG_INFINITY));
        assert!(!is_finite(f64::NAN));
    }

    #[test]
    fn test_is_normal_comprehensive() {
        assert!(is_normal(1.0));
        assert!(is_normal(-1.0));
        assert!(is_normal(1e100));
        assert!(is_normal(f64::MIN_POSITIVE));
        // Zero is not normal
        assert!(!is_normal(0.0));
        // Infinity is not normal
        assert!(!is_normal(f64::INFINITY));
        // NaN is not normal
        assert!(!is_normal(f64::NAN));
        // Subnormals are not normal
        assert!(!is_normal(f64::MIN_POSITIVE / 2.0));
    }

    #[test]
    fn test_signum_comprehensive() {
        assert_eq!(signum(42.0), 1.0);
        assert_eq!(signum(-5.0), -1.0);
        assert_eq!(signum(0.0), 1.0); // +0.0 has positive sign per IEEE 754
        assert_eq!(signum(-0.0), -1.0); // -0.0 has negative sign per IEEE 754
        assert_eq!(signum(0.001), 1.0);
        assert_eq!(signum(-0.001), -1.0);
        assert_eq!(signum(f64::INFINITY), 1.0);
        assert_eq!(signum(f64::NEG_INFINITY), -1.0);
        assert!(signum(f64::NAN).is_nan());
    }

    #[test]
    fn test_copysign_comprehensive() {
        assert_eq!(copysign(3.0, 1.0), 3.0);
        assert_eq!(copysign(3.0, -1.0), -3.0);
        assert_eq!(copysign(-3.0, 1.0), 3.0);
        assert_eq!(copysign(-3.0, -1.0), -3.0);
        assert_eq!(copysign(0.0, -1.0), -0.0);
        assert_eq!(copysign(0.0, 1.0), 0.0);
        // copysign with magnitude 5 and sign of -2 => -5
        assert_eq!(copysign(5.0, -2.0), -5.0);
    }

    #[test]
    fn test_min_of_max_of_comprehensive() {
        // Basic comparisons
        assert_eq!(min_of(1.0, 2.0), 1.0);
        assert_eq!(max_of(1.0, 2.0), 2.0);
        // Equal values
        assert_eq!(min_of(5.0, 5.0), 5.0);
        assert_eq!(max_of(5.0, 5.0), 5.0);
        // Negative values
        assert_eq!(min_of(-5.0, -3.0), -5.0);
        assert_eq!(max_of(-5.0, -3.0), -3.0);
        // Mixed signs
        assert_eq!(min_of(-1.0, 1.0), -1.0);
        assert_eq!(max_of(-1.0, 1.0), 1.0);
        // Zero comparisons
        assert_eq!(min_of(0.0, 1.0), 0.0);
        assert_eq!(max_of(0.0, -1.0), 0.0);
    }

    // --- Comprehensive constants tests ---

    #[test]
    fn test_pi_constant() {
        let tol = 1e-10;
        assert!((PI - 3.14159265358979323846).abs() < tol);
        // PI is approximately 3.14159
        assert!((PI - 3.14159).abs() < 0.00001);
    }

    #[test]
    fn test_e_constant() {
        let tol = 1e-10;
        assert!((E - 2.71828182845904523536).abs() < tol);
        // E is approximately 2.71828
        assert!((E - 2.71828).abs() < 0.00001);
    }

    #[test]
    fn test_tau_constant() {
        let tol = 1e-10;
        assert!((TAU - 2.0 * PI).abs() < tol);
        assert!((TAU - 6.283185307179586).abs() < tol);
    }

    #[test]
    fn test_log_constants() {
        let tol = 1e-10;
        // LN_2 = ln(2)
        assert!((LN_2 - ln(2.0)).abs() < tol);
        // LN_10 = ln(10)
        assert!((LN_10 - ln(10.0)).abs() < tol);
        // LOG2_E = log2(e) = 1/ln(2)
        assert!((LOG2_E - 1.0 / LN_2).abs() < tol);
        // LOG10_E = log10(e) = 1/ln(10)
        assert!((LOG10_E - 1.0 / LN_10).abs() < tol);
    }

    #[test]
    fn test_sqrt_constant() {
        let tol = 1e-10;
        assert!((SQRT_2 - sqrt(2.0)).abs() < tol);
        assert!((FRAC_1_SQRT_2 - 1.0 / SQRT_2).abs() < tol);
        // SQRT_2² = 2
        assert!((SQRT_2 * SQRT_2 - 2.0).abs() < tol);
    }

    #[test]
    fn test_all_f32_constants() {
        let tol32 = 1e-5f32;
        assert!((PI_F32 - std::f32::consts::PI).abs() < tol32);
        assert!((TAU_F32 - std::f32::consts::TAU).abs() < tol32);
        assert!((E_F32 - std::f32::consts::E).abs() < tol32);
        assert!((LN_2_F32 - std::f32::consts::LN_2).abs() < tol32);
        assert!((LN_10_F32 - std::f32::consts::LN_10).abs() < tol32);
        assert!((LOG2_E_F32 - std::f32::consts::LOG2_E).abs() < tol32);
        assert!((LOG10_E_F32 - std::f32::consts::LOG10_E).abs() < tol32);
        assert!((SQRT_2_F32 - std::f32::consts::SQRT_2).abs() < tol32);
        assert!((FRAC_1_SQRT_2_F32 - std::f32::consts::FRAC_1_SQRT_2).abs() < tol32);
        // Verify TAU = 2*PI for f32
        assert!((TAU_F32 - 2.0f32 * PI_F32).abs() < tol32);
    }

    // --- Comprehensive f32 trigonometric tests ---

    #[test]
    fn test_sin_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((sin_f32(0.0f32) - 0.0f32).abs() < tol);
        assert!((sin_f32(PI_F32 / 2.0f32) - 1.0f32).abs() < tol);
        assert!((sin_f32(PI_F32) - 0.0f32).abs() < tol);
        assert!((sin_f32(-PI_F32 / 2.0f32) - (-1.0f32)).abs() < tol);
    }

    #[test]
    fn test_cos_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((cos_f32(0.0f32) - 1.0f32).abs() < tol);
        assert!((cos_f32(PI_F32) - (-1.0f32)).abs() < tol);
        assert!((cos_f32(PI_F32 / 2.0f32) - 0.0f32).abs() < tol);
    }

    #[test]
    fn test_tan_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((tan_f32(0.0f32) - 0.0f32).abs() < tol);
        assert!((tan_f32(PI_F32 / 4.0f32) - 1.0f32).abs() < tol);
        assert!((tan_f32(-PI_F32 / 4.0f32) - (-1.0f32)).abs() < tol);
    }

    #[test]
    fn test_asin_acos_atan_f32() {
        let tol = 1e-5f32;
        assert!((asin_f32(0.0f32) - 0.0f32).abs() < tol);
        assert!((asin_f32(1.0f32) - PI_F32 / 2.0f32).abs() < tol);
        assert!((acos_f32(1.0f32) - 0.0f32).abs() < tol);
        assert!((acos_f32(0.0f32) - PI_F32 / 2.0f32).abs() < tol);
        assert!((atan_f32(0.0f32) - 0.0f32).abs() < tol);
        assert!((atan_f32(1.0f32) - PI_F32 / 4.0f32).abs() < tol);
    }

    #[test]
    fn test_atan2_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((atan2_f32(1.0f32, 1.0f32) - PI_F32 / 4.0f32).abs() < tol);
        assert!((atan2_f32(1.0f32, 0.0f32) - PI_F32 / 2.0f32).abs() < tol);
        assert!((atan2_f32(0.0f32, -1.0f32) - PI_F32).abs() < tol);
        assert!((atan2_f32(-1.0f32, 0.0f32) - (-PI_F32 / 2.0f32)).abs() < tol);
    }

    #[test]
    fn test_sinh_cosh_tanh_f32() {
        let tol = 1e-5f32;
        assert!((sinh_f32(0.0f32) - 0.0f32).abs() < tol);
        assert!((cosh_f32(0.0f32) - 1.0f32).abs() < tol);
        assert!((tanh_f32(0.0f32) - 0.0f32).abs() < tol);
        // tanh(x) should approach 1 for large x
        assert!((tanh_f32(10.0f32) - 1.0f32).abs() < 0.001f32);
        assert!((tanh_f32(-10.0f32) - (-1.0f32)).abs() < 0.001f32);
    }

    #[test]
    fn test_trig_f32_out_of_range() {
        // asin/acos with |x| > 1 should return NaN
        assert!(asin_f32(2.0f32).is_nan());
        assert!(asin_f32(-2.0f32).is_nan());
        assert!(acos_f32(2.0f32).is_nan());
        assert!(acos_f32(-2.0f32).is_nan());
    }

    // --- Comprehensive f32 exponential/logarithmic tests ---

    #[test]
    fn test_sqrt_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((sqrt_f32(4.0f32) - 2.0f32).abs() < tol);
        assert!((sqrt_f32(0.0f32) - 0.0f32).abs() < tol);
        assert!((sqrt_f32(1.0f32) - 1.0f32).abs() < tol);
        assert!((sqrt_f32(9.0f32) - 3.0f32).abs() < tol);
        assert!(sqrt_f32(-1.0f32).is_nan());
    }

    #[test]
    fn test_cbrt_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((cbrt_f32(27.0f32) - 3.0f32).abs() < tol);
        assert!((cbrt_f32(-8.0f32) - (-2.0f32)).abs() < tol);
        assert!((cbrt_f32(0.0f32) - 0.0f32).abs() < tol);
        assert!((cbrt_f32(1.0f32) - 1.0f32).abs() < tol);
    }

    #[test]
    fn test_exp_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((exp_f32(0.0f32) - 1.0f32).abs() < tol);
        assert!((exp_f32(1.0f32) - E_F32).abs() < tol);
        assert!((exp_f32(-1.0f32) - 1.0f32 / E_F32).abs() < tol);
    }

    #[test]
    fn test_exp2_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((exp2_f32(0.0f32) - 1.0f32).abs() < tol);
        assert!((exp2_f32(1.0f32) - 2.0f32).abs() < tol);
        assert!((exp2_f32(3.0f32) - 8.0f32).abs() < tol);
        assert!((exp2_f32(-1.0f32) - 0.5f32).abs() < tol);
    }

    #[test]
    fn test_exp_m1_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((exp_m1_f32(0.0f32) - 0.0f32).abs() < tol);
        assert!((exp_m1_f32(1.0f32) - (E_F32 - 1.0f32)).abs() < tol);
    }

    #[test]
    fn test_ln_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((ln_f32(1.0f32) - 0.0f32).abs() < tol);
        assert!((ln_f32(E_F32) - 1.0f32).abs() < tol);
        assert_eq!(ln_f32(0.0f32), f32::NEG_INFINITY);
        assert!(ln_f32(-1.0f32).is_nan());
    }

    #[test]
    fn test_log2_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((log2_f32(1.0f32) - 0.0f32).abs() < tol);
        assert!((log2_f32(2.0f32) - 1.0f32).abs() < tol);
        assert!((log2_f32(8.0f32) - 3.0f32).abs() < tol);
        assert!((log2_f32(1024.0f32) - 10.0f32).abs() < tol);
    }

    #[test]
    fn test_log10_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((log10_f32(1.0f32) - 0.0f32).abs() < tol);
        assert!((log10_f32(10.0f32) - 1.0f32).abs() < tol);
        assert!((log10_f32(100.0f32) - 2.0f32).abs() < tol);
        assert!((log10_f32(1000.0f32) - 3.0f32).abs() < tol);
    }

    #[test]
    fn test_ln_1p_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((ln_1p_f32(0.0f32) - 0.0f32).abs() < tol);
        assert!((ln_1p_f32(E_F32 - 1.0f32) - 1.0f32).abs() < tol);
    }

    #[test]
    fn test_pow_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((pow_f32(2.0f32, 3.0f32) - 8.0f32).abs() < tol);
        assert!((pow_f32(4.0f32, 0.5f32) - 2.0f32).abs() < tol);
        assert!((pow_f32(10.0f32, 0.0f32) - 1.0f32).abs() < tol);
        assert!((pow_f32(2.0f32, -1.0f32) - 0.5f32).abs() < tol);
    }

    #[test]
    fn test_powi_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((powi_f32(2.0f32, 3) - 8.0f32).abs() < tol);
        assert!((powi_f32(3.0f32, 0) - 1.0f32).abs() < tol);
        assert!((powi_f32(2.0f32, -1) - 0.5f32).abs() < tol);
        assert!((powi_f32(5.0f32, 2) - 25.0f32).abs() < tol);
    }

    #[test]
    fn test_exp_log_f32_roundtrip() {
        let tol = 1e-5f32;
        for x in [0.5f32, 1.0f32, 2.0f32, 5.0f32] {
            assert!((ln_f32(exp_f32(x)) - x).abs() < tol);
            assert!((log2_f32(exp2_f32(x)) - x).abs() < tol);
        }
    }

    // --- Comprehensive f32 rounding tests ---

    #[test]
    fn test_floor_f32_comprehensive() {
        assert_eq!(floor_f32(3.7f32), 3.0f32);
        assert_eq!(floor_f32(-3.7f32), -4.0f32);
        assert_eq!(floor_f32(0.0f32), 0.0f32);
        assert_eq!(floor_f32(3.0f32), 3.0f32);
        assert_eq!(floor_f32(-0.1f32), -1.0f32);
    }

    #[test]
    fn test_ceil_f32_comprehensive() {
        assert_eq!(ceil_f32(3.2f32), 4.0f32);
        assert_eq!(ceil_f32(-3.2f32), -3.0f32);
        assert_eq!(ceil_f32(0.0f32), 0.0f32);
        assert_eq!(ceil_f32(3.0f32), 3.0f32);
        assert_eq!(ceil_f32(0.1f32), 1.0f32);
    }

    #[test]
    fn test_round_f32_comprehensive() {
        assert_eq!(round_f32(3.5f32), 4.0f32);
        assert_eq!(round_f32(3.4f32), 3.0f32);
        assert_eq!(round_f32(-3.5f32), -4.0f32);
        assert_eq!(round_f32(0.0f32), 0.0f32);
        assert_eq!(round_f32(2.5f32), 3.0f32);
    }

    #[test]
    fn test_trunc_f32_comprehensive() {
        assert_eq!(trunc_f32(3.7f32), 3.0f32);
        assert_eq!(trunc_f32(-3.7f32), -3.0f32);
        assert_eq!(trunc_f32(0.0f32), 0.0f32);
        assert_eq!(trunc_f32(0.9f32), 0.0f32);
        assert_eq!(trunc_f32(-0.9f32), 0.0f32);
    }

    #[test]
    fn test_fract_f32_comprehensive() {
        let tol = 1e-5f32;
        assert!((fract_f32(3.7f32) - 0.7f32).abs() < tol);
        assert!((fract_f32(-3.7f32) - (-0.7f32)).abs() < tol);
        assert!((fract_f32(0.0f32) - 0.0f32).abs() < tol);
        assert!((fract_f32(1.0f32) - 0.0f32).abs() < tol);
    }

    #[test]
    fn test_rounding_f32_nan() {
        assert!(floor_f32(f32::NAN).is_nan());
        assert!(ceil_f32(f32::NAN).is_nan());
        assert!(round_f32(f32::NAN).is_nan());
        assert!(trunc_f32(f32::NAN).is_nan());
        assert!(fract_f32(f32::NAN).is_nan());
    }

    #[test]
    fn test_rounding_f32_infinity() {
        assert_eq!(floor_f32(f32::INFINITY), f32::INFINITY);
        assert_eq!(ceil_f32(f32::INFINITY), f32::INFINITY);
        assert_eq!(floor_f32(f32::NEG_INFINITY), f32::NEG_INFINITY);
        assert_eq!(ceil_f32(f32::NEG_INFINITY), f32::NEG_INFINITY);
    }

    // --- Comprehensive f32 comparison tests ---

    #[test]
    fn test_min_of_f32_comprehensive() {
        assert_eq!(min_of_f32(1.0f32, 2.0f32), 1.0f32);
        assert_eq!(min_of_f32(2.0f32, 1.0f32), 1.0f32);
        assert_eq!(min_of_f32(-5.0f32, 3.0f32), -5.0f32);
        assert_eq!(min_of_f32(5.0f32, 5.0f32), 5.0f32);
    }

    #[test]
    fn test_max_of_f32_comprehensive() {
        assert_eq!(max_of_f32(1.0f32, 2.0f32), 2.0f32);
        assert_eq!(max_of_f32(2.0f32, 1.0f32), 2.0f32);
        assert_eq!(max_of_f32(-5.0f32, 3.0f32), 3.0f32);
        assert_eq!(max_of_f32(5.0f32, 5.0f32), 5.0f32);
    }

    #[test]
    fn test_min_max_f32_nan() {
        // min_of/max_of should return the non-NaN value
        assert_eq!(min_of_f32(f32::NAN, 1.0f32), 1.0f32);
        assert_eq!(min_of_f32(1.0f32, f32::NAN), 1.0f32);
        assert_eq!(max_of_f32(f32::NAN, 1.0f32), 1.0f32);
        assert_eq!(max_of_f32(1.0f32, f32::NAN), 1.0f32);
    }

    // --- Comprehensive f32 classification tests ---

    #[test]
    fn test_is_nan_f32_comprehensive() {
        assert!(is_nan_f32(f32::NAN));
        assert!(!is_nan_f32(0.0f32));
        assert!(!is_nan_f32(1.0f32));
        assert!(!is_nan_f32(-1.0f32));
        assert!(!is_nan_f32(f32::INFINITY));
        assert!(!is_nan_f32(f32::NEG_INFINITY));
    }

    #[test]
    fn test_is_infinite_f32_comprehensive() {
        assert!(is_infinite_f32(f32::INFINITY));
        assert!(is_infinite_f32(f32::NEG_INFINITY));
        assert!(!is_infinite_f32(0.0f32));
        assert!(!is_infinite_f32(1.0f32));
        assert!(!is_infinite_f32(f32::NAN));
    }

    #[test]
    fn test_is_finite_f32_comprehensive() {
        assert!(is_finite_f32(0.0f32));
        assert!(is_finite_f32(1.0f32));
        assert!(is_finite_f32(-1.0f32));
        assert!(is_finite_f32(1e30f32));
        assert!(!is_finite_f32(f32::INFINITY));
        assert!(!is_finite_f32(f32::NEG_INFINITY));
        assert!(!is_finite_f32(f32::NAN));
    }

    #[test]
    fn test_is_normal_f32_comprehensive() {
        assert!(is_normal_f32(1.0f32));
        assert!(is_normal_f32(-1.0f32));
        assert!(!is_normal_f32(0.0f32));
        assert!(!is_normal_f32(f32::INFINITY));
        assert!(!is_normal_f32(f32::NAN));
        // Subnormals are not normal
        assert!(!is_normal_f32(f32::MIN_POSITIVE / 2.0f32));
    }

    #[test]
    fn test_signum_f32_comprehensive() {
        assert_eq!(signum_f32(42.0f32), 1.0f32);
        assert_eq!(signum_f32(-5.0f32), -1.0f32);
        assert_eq!(signum_f32(0.0f32), 1.0f32); // +0.0 has positive sign per IEEE 754
        assert_eq!(signum_f32(-0.0f32), -1.0f32); // -0.0 has negative sign per IEEE 754
        assert_eq!(signum_f32(f32::INFINITY), 1.0f32);
        assert_eq!(signum_f32(f32::NEG_INFINITY), -1.0f32);
        assert!(signum_f32(f32::NAN).is_nan());
    }

    #[test]
    fn test_copysign_f32_comprehensive() {
        assert_eq!(copysign_f32(3.0f32, 1.0f32), 3.0f32);
        assert_eq!(copysign_f32(3.0f32, -1.0f32), -3.0f32);
        assert_eq!(copysign_f32(-3.0f32, 1.0f32), 3.0f32);
        assert_eq!(copysign_f32(-3.0f32, -1.0f32), -3.0f32);
        assert_eq!(copysign_f32(5.0f32, -2.0f32), -5.0f32);
    }

    // --- Comprehensive integer arithmetic tests ---

    #[test]
    fn test_abs_comprehensive() {
        assert_eq!(abs(0), 0);
        assert_eq!(abs(1), 1);
        assert_eq!(abs(-1), 1);
        assert_eq!(abs(42), 42);
        assert_eq!(abs(-42), 42);
        assert_eq!(abs(i64::MAX), i64::MAX);
        // i64::MIN wraps
        assert_eq!(abs(i64::MIN), i64::MIN);
    }

    #[test]
    fn test_min_comprehensive() {
        assert_eq!(min(1, 2), 1);
        assert_eq!(min(2, 1), 1);
        assert_eq!(min(-1, 1), -1);
        assert_eq!(min(0, 0), 0);
        assert_eq!(min(i64::MIN, i64::MAX), i64::MIN);
    }

    #[test]
    fn test_max_comprehensive() {
        assert_eq!(max(1, 2), 2);
        assert_eq!(max(2, 1), 2);
        assert_eq!(max(-1, 1), 1);
        assert_eq!(max(0, 0), 0);
        assert_eq!(max(i64::MIN, i64::MAX), i64::MAX);
    }

    #[test]
    fn test_clamp_comprehensive() {
        // Within range
        assert_eq!(clamp(5, 0, 10), 5);
        // Below range
        assert_eq!(clamp(-5, 0, 10), 0);
        // Above range
        assert_eq!(clamp(15, 0, 10), 10);
        // At boundaries
        assert_eq!(clamp(0, 0, 10), 0);
        assert_eq!(clamp(10, 0, 10), 10);
        // Singleton range
        assert_eq!(clamp(42, 7, 7), 7);
        assert_eq!(clamp(7, 7, 7), 7);
        // Negative range
        assert_eq!(clamp(-5, -10, -1), -5);
        assert_eq!(clamp(-15, -10, -1), -10);
    }

    // --- Typed integer arithmetic tests ---

    #[test]
    fn test_abs_i32_comprehensive() {
        assert_eq!(abs_i32(0), 0);
        assert_eq!(abs_i32(1), 1);
        assert_eq!(abs_i32(-1), 1);
        assert_eq!(abs_i32(42), 42);
        assert_eq!(abs_i32(-42), 42);
        assert_eq!(abs_i32(i32::MAX), i32::MAX);
        // i32::MIN wraps
        assert_eq!(abs_i32(i32::MIN), i32::MIN);
    }

    #[test]
    fn test_abs_i64_comprehensive() {
        assert_eq!(abs_i64(0), 0);
        assert_eq!(abs_i64(1), 1);
        assert_eq!(abs_i64(-1), 1);
        assert_eq!(abs_i64(42), 42);
        assert_eq!(abs_i64(-42), 42);
        assert_eq!(abs_i64(i64::MAX), i64::MAX);
        // i64::MIN wraps
        assert_eq!(abs_i64(i64::MIN), i64::MIN);
    }

    #[test]
    fn test_min_i32_comprehensive() {
        assert_eq!(min_i32(1, 2), 1);
        assert_eq!(min_i32(2, 1), 1);
        assert_eq!(min_i32(-1, 1), -1);
        assert_eq!(min_i32(0, 0), 0);
        assert_eq!(min_i32(i32::MIN, i32::MAX), i32::MIN);
    }

    #[test]
    fn test_max_i32_comprehensive() {
        assert_eq!(max_i32(1, 2), 2);
        assert_eq!(max_i32(2, 1), 2);
        assert_eq!(max_i32(-1, 1), 1);
        assert_eq!(max_i32(0, 0), 0);
        assert_eq!(max_i32(i32::MIN, i32::MAX), i32::MAX);
    }

    #[test]
    fn test_min_u64_comprehensive() {
        assert_eq!(min_u64(1, 2), 1);
        assert_eq!(min_u64(2, 1), 1);
        assert_eq!(min_u64(0, 100), 0);
        assert_eq!(min_u64(42, 42), 42);
        assert_eq!(min_u64(0, u64::MAX), 0);
    }

    #[test]
    fn test_max_u64_comprehensive() {
        assert_eq!(max_u64(1, 2), 2);
        assert_eq!(max_u64(2, 1), 2);
        assert_eq!(max_u64(0, 100), 100);
        assert_eq!(max_u64(42, 42), 42);
        assert_eq!(max_u64(0, u64::MAX), u64::MAX);
    }

    #[test]
    fn test_clamp_i32_comprehensive() {
        // Within range
        assert_eq!(clamp_i32(5, 0, 10), 5);
        // Below range
        assert_eq!(clamp_i32(-5, 0, 10), 0);
        // Above range
        assert_eq!(clamp_i32(15, 0, 10), 10);
        // At boundaries
        assert_eq!(clamp_i32(0, 0, 10), 0);
        assert_eq!(clamp_i32(10, 0, 10), 10);
        // Singleton range
        assert_eq!(clamp_i32(42, 7, 7), 7);
        assert_eq!(clamp_i32(7, 7, 7), 7);
        // Negative range
        assert_eq!(clamp_i32(-5, -10, -1), -5);
        assert_eq!(clamp_i32(-15, -10, -1), -10);
    }

    #[test]
    fn test_clamp_u64_comprehensive() {
        // Within range
        assert_eq!(clamp_u64(5, 0, 10), 5);
        // Below range
        assert_eq!(clamp_u64(0, 3, 10), 3);
        // Above range
        assert_eq!(clamp_u64(15, 0, 10), 10);
        // At boundaries
        assert_eq!(clamp_u64(0, 0, 10), 0);
        assert_eq!(clamp_u64(10, 0, 10), 10);
        // Singleton range
        assert_eq!(clamp_u64(42, 7, 7), 7);
        assert_eq!(clamp_u64(7, 7, 7), 7);
        // Large values
        assert_eq!(clamp_u64(u64::MAX, 0, 1000), 1000);
    }

    // --- Integer division tests ---

    #[test]
    fn test_div_floor_i32_comprehensive() {
        // Same-sign divisions (same as truncation)
        assert_eq!(div_floor_i32(7, 2), 3);
        assert_eq!(div_floor_i32(-7, -2), 3);
        assert_eq!(div_floor_i32(6, 3), 2);
        // Different-sign divisions (floor differs from truncation)
        assert_eq!(div_floor_i32(-7, 2), -4);
        assert_eq!(div_floor_i32(7, -2), -4);
        // Exact division
        assert_eq!(div_floor_i32(8, 4), 2);
        assert_eq!(div_floor_i32(-8, 4), -2);
        // Zero dividend
        assert_eq!(div_floor_i32(0, 5), 0);
    }

    #[test]
    fn test_div_ceil_i32_comprehensive() {
        // Same-sign divisions (ceil differs from truncation when remainder ≠ 0)
        assert_eq!(div_ceil_i32(7, 2), 4);
        assert_eq!(div_ceil_i32(-7, -2), 4);
        // Different-sign divisions (same as truncation)
        assert_eq!(div_ceil_i32(-7, 2), -3);
        assert_eq!(div_ceil_i32(7, -2), -3);
        // Exact division
        assert_eq!(div_ceil_i32(8, 4), 2);
        assert_eq!(div_ceil_i32(-8, 4), -2);
        // Zero dividend
        assert_eq!(div_ceil_i32(0, 5), 0);
    }

    // --- Number theory tests ---

    #[test]
    fn test_gcd_u64_comprehensive() {
        assert_eq!(gcd_u64(12, 8), 4);
        assert_eq!(gcd_u64(8, 12), 4); // Order doesn't matter
        assert_eq!(gcd_u64(7, 0), 7);
        assert_eq!(gcd_u64(0, 7), 7);
        assert_eq!(gcd_u64(0, 0), 0);
        assert_eq!(gcd_u64(17, 13), 1); // Both prime
        assert_eq!(gcd_u64(100, 75), 25);
        assert_eq!(gcd_u64(1, 1), 1);
    }

    #[test]
    fn test_lcm_u64_comprehensive() {
        assert_eq!(lcm_u64(4, 6), 12);
        assert_eq!(lcm_u64(6, 4), 12); // Order doesn't matter
        assert_eq!(lcm_u64(7, 0), 0);
        assert_eq!(lcm_u64(0, 7), 0);
        assert_eq!(lcm_u64(0, 0), 0);
        assert_eq!(lcm_u64(3, 5), 15); // Both prime
        assert_eq!(lcm_u64(1, 1), 1);
        assert_eq!(lcm_u64(12, 18), 36);
    }

    // --- Bit manipulation tests ---

    #[test]
    fn test_is_power_of_two_u64_comprehensive() {
        assert!(is_power_of_two_u64(1));
        assert!(is_power_of_two_u64(2));
        assert!(is_power_of_two_u64(4));
        assert!(is_power_of_two_u64(1024));
        assert!(is_power_of_two_u64(1u64 << 63));
        assert!(!is_power_of_two_u64(0));
        assert!(!is_power_of_two_u64(3));
        assert!(!is_power_of_two_u64(5));
        assert!(!is_power_of_two_u64(6));
        assert!(!is_power_of_two_u64(u64::MAX));
    }

    #[test]
    fn test_next_power_of_two_u64_comprehensive() {
        assert_eq!(next_power_of_two_u64(0), 1);
        assert_eq!(next_power_of_two_u64(1), 1);
        assert_eq!(next_power_of_two_u64(2), 2);
        assert_eq!(next_power_of_two_u64(3), 4);
        assert_eq!(next_power_of_two_u64(5), 8);
        assert_eq!(next_power_of_two_u64(8), 8);
        assert_eq!(next_power_of_two_u64(9), 16);
        assert_eq!(next_power_of_two_u64(1023), 1024);
        assert_eq!(next_power_of_two_u64(1024), 1024);
    }

    #[test]
    fn test_count_ones_u64_comprehensive() {
        assert_eq!(count_ones_u64(0), 0);
        assert_eq!(count_ones_u64(1), 1);
        assert_eq!(count_ones_u64(0xFF), 8);
        assert_eq!(count_ones_u64(0xFFFF), 16);
        assert_eq!(count_ones_u64(u64::MAX), 64);
        assert_eq!(count_ones_u64(0b101010), 3);
    }

    #[test]
    fn test_count_zeros_u64_comprehensive() {
        assert_eq!(count_zeros_u64(0), 64);
        assert_eq!(count_zeros_u64(u64::MAX), 0);
        assert_eq!(count_zeros_u64(1), 63);
        assert_eq!(count_zeros_u64(0xFF), 56);
    }

    #[test]
    fn test_leading_zeros_u64_comprehensive() {
        assert_eq!(leading_zeros_u64(0), 64);
        assert_eq!(leading_zeros_u64(1), 63);
        assert_eq!(leading_zeros_u64(u64::MAX), 0);
        assert_eq!(leading_zeros_u64(1u64 << 63), 0);
        assert_eq!(leading_zeros_u64(1u64 << 32), 31);
    }

    #[test]
    fn test_trailing_zeros_u64_comprehensive() {
        assert_eq!(trailing_zeros_u64(0), 64);
        assert_eq!(trailing_zeros_u64(1), 0);
        assert_eq!(trailing_zeros_u64(2), 1);
        assert_eq!(trailing_zeros_u64(4), 2);
        assert_eq!(trailing_zeros_u64(1u64 << 63), 63);
    }

    #[test]
    fn test_reverse_bits_u64_comprehensive() {
        assert_eq!(reverse_bits_u64(0), 0);
        assert_eq!(reverse_bits_u64(1), 1u64 << 63);
        assert_eq!(reverse_bits_u64(1u64 << 63), 1);
        assert_eq!(reverse_bits_u64(u64::MAX), u64::MAX);
        // Double reversal is identity
        assert_eq!(reverse_bits_u64(reverse_bits_u64(0xDEADBEEF)), 0xDEADBEEF);
    }

    #[test]
    fn test_swap_bytes_u64_comprehensive() {
        assert_eq!(swap_bytes_u64(0), 0);
        assert_eq!(swap_bytes_u64(0x0102030405060708), 0x0807060504030201);
        assert_eq!(swap_bytes_u64(0xFF00000000000000), 0x00000000000000FF);
        assert_eq!(swap_bytes_u64(u64::MAX), u64::MAX);
        // Double swap is identity
        assert_eq!(swap_bytes_u64(swap_bytes_u64(0x123456789ABCDEF0)), 0x123456789ABCDEF0);
    }
}
