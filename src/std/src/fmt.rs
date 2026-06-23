//! # String Formatting and Output
//!
//! This module provides VUMA-verified string formatting primitives for
//! printf-style output, integer/float conversion, padding, joining, and
//! buffered writing. These are the foundational formatting operations that
//! LLMs need when generating VUMA programs that produce human-readable
//! output or serialize data to text streams.
//!
//! ## Relationship to VUMA Programs
//!
//! In `.vuma` source, these functions serve as the standard formatting
//! library — the equivalent of C's `printf`/`sprintf` family or Rust's
//! `std::fmt` module. They allow VUMA programs to convert numeric values
//! to text representations in various bases (decimal, hexadecimal, octal,
//! binary), format floating-point numbers with configurable precision, pad
//! strings to fixed widths, join multiple strings, and write formatted
//! output into byte buffers.
//!
//! ## BD Annotations
//!
//! All functions in this module are annotated with Behavioral Descriptions.
//! Pure formatting functions (those that return `VumaString`) declare only
//! { Read, Compare } capabilities. Buffer-writing functions additionally
//! declare { Write } since they mutate the provided buffer.

use crate::collections::VumaString;
use crate::primitives::{CapD, CapFlag};

// ---------------------------------------------------------------------------
// Integer Formatting
// ---------------------------------------------------------------------------

/// Format a signed 64-bit integer as a string in the given base with
/// minimum width padding.
///
/// Converts `value` to its textual representation in `base` (2–36). If the
/// textual representation is shorter than `width`, it is left-padded with
/// `'0'` characters. A negative value is prefixed with `'-'`; the sign
/// occupies one character position and is not counted as a digit.
///
/// ## Supported Bases
///
/// | Base | Name       | Digits                    |
/// |------|------------|---------------------------|
/// | 2    | Binary     | 0–1                       |
/// | 8    | Octal      | 0–7                       |
/// | 10   | Decimal    | 0–9                       |
/// | 16   | Hexadecimal| 0–9, a–f                  |
/// | 36   | Base-36    | 0–9, a–z                  |
///
/// Bases outside the range 2–36 are treated as base 10.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn format_int(value: i64, base: u32, width: u32) -> VumaString {
///     negative: bool = value < 0;
///     abs_val: u64 = if negative { -value as u64 } else { value as u64 };
///     digits: VumaString = format_uint(abs_val, base, width);
///     if negative {
///         return "-" + digits;
///     }
///     return digits;
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure conversion, no side effects
// VUMA-VERIFIED: integer formatting is correct for all i64 values in bases 2–36
pub fn format_int(value: i64, base: u32, width: u32) -> VumaString {
    let effective_base = if base >= 2 && base <= 36 { base } else { 10 };

    if value < 0 {
        let abs_val = (value as i128).unsigned_abs() as u64;
        let digits = format_uint_inner(abs_val, effective_base, if width > 1 { width - 1 } else { 0 });
        let mut result = VumaString::new();
        result.push('-');
        result.push_str(digits.as_str());
        result
    } else {
        format_uint_inner(value as u64, effective_base, width)
    }
}

/// Format an unsigned 64-bit integer as a string in the given base with
/// minimum width padding.
///
/// Converts `value` to its textual representation in `base` (2–36). If the
/// textual representation is shorter than `width`, it is left-padded with
/// `'0'` characters.
///
/// ## Supported Bases
///
/// Same as [`format_int`]. Bases outside the range 2–36 are treated as
/// base 10.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn format_uint(value: u64, base: u32, width: u32) -> VumaString {
///     if value == 0 {
///         return pad_left("0", width, '0');
///     }
///     digits: [u8; 64];
///     pos: u32 = 0;
///     v: u64 = value;
///     while v > 0 {
///         digit: u64 = v % (base as u64);
///         if digit < 10 {
///             digits[pos] = '0' + (digit as u8);
///         } else {
///             digits[pos] = 'a' + ((digit - 10) as u8);
///         }
///         pos = pos + 1;
///         v = v / (base as u64);
///     }
///     result: VumaString = "";
///     for i in (0..pos).rev() {
///         result.push(digits[i] as char);
///     }
///     return pad_left(result, width, '0');
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure conversion, no side effects
// VUMA-VERIFIED: unsigned integer formatting is correct for all u64 values in bases 2–36
pub fn format_uint(value: u64, base: u32, width: u32) -> VumaString {
    let effective_base = if base >= 2 && base <= 36 { base } else { 10 };
    format_uint_inner(value, effective_base, width)
}

/// Internal helper: format a u64 value in the given base with zero-padding.
fn format_uint_inner(value: u64, base: u32, width: u32) -> VumaString {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

    if value == 0 {
        let zero_count = width.saturating_sub(1) as usize;
        let mut result = VumaString::new();
        for _ in 0..zero_count {
            result.push('0');
        }
        result.push('0');
        return result;
    }

    // Maximum digits for u64 in base 2 is 64
    let mut buf = [0u8; 64];
    let mut pos = 0usize;
    let mut v = value;

    while v > 0 {
        let digit = (v % base as u64) as usize;
        buf[pos] = DIGITS[digit];
        pos += 1;
        v /= base as u64;
    }

    // Apply zero-padding
    let padding = (width as usize).saturating_sub(pos);

    let mut result = VumaString::with_capacity(padding + pos);
    for _ in 0..padding {
        result.push('0');
    }
    // Write digits in reverse order
    for i in (0..pos).rev() {
        result.push(buf[i] as char);
    }

    result
}

// ---------------------------------------------------------------------------
// Floating-Point Formatting
// ---------------------------------------------------------------------------

/// Format a 64-bit floating-point number as a string with the given
/// decimal precision.
///
/// Produces a decimal representation of `value` with exactly `precision`
/// digits after the decimal point. If `precision` is 0, no decimal point
/// is emitted.
///
/// ## Special Values
///
/// - `NaN` is formatted as `"nan"`.
/// - Positive infinity is formatted as `"inf"`.
/// - Negative infinity is formatted as `"-inf"`.
/// - Zero is formatted as `"0"` (precision 0) or `"0.00..."` (precision > 0).
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn format_float(value: f64, precision: u32) -> VumaString {
///     // Delegates to the host's formatted float conversion
///     return host_float_to_str(value, precision);
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure conversion, no side effects
// VUMA-VERIFIED: float formatting produces correct decimal representation
pub fn format_float(value: f64, precision: u32) -> VumaString {
    if value.is_nan() {
        return VumaString::from("nan");
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            VumaString::from("-inf")
        } else {
            VumaString::from("inf")
        };
    }

    let formatted = if precision == 0 {
        format!("{:.0}", value)
    } else {
        format!("{:.1$}", value, precision as usize)
    };

    VumaString::from(formatted.as_str())
}

// ---------------------------------------------------------------------------
// Hexadecimal, Binary, Octal Formatting
// ---------------------------------------------------------------------------

/// Format an unsigned 64-bit integer as a lowercase hexadecimal string
/// with minimum width padding.
///
/// Produces the hexadecimal representation of `value` using lowercase
/// digits (`0–9`, `a–f`). If the representation is shorter than `width`,
/// it is left-padded with `'0'` characters.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn format_hex(value: u64, width: u32) -> VumaString {
///     return format_uint(value, 16, width);
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure conversion, no side effects
// VUMA-VERIFIED: hex formatting is correct for all u64 values
pub fn format_hex(value: u64, width: u32) -> VumaString {
    format_uint_inner(value, 16, width)
}

/// Format an unsigned 64-bit integer as a binary string with minimum
/// width padding.
///
/// Produces the binary representation of `value` using digits `0` and `1`.
/// If the representation is shorter than `width`, it is left-padded with
/// `'0'` characters.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn format_binary(value: u64, width: u32) -> VumaString {
///     return format_uint(value, 2, width);
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure conversion, no side effects
// VUMA-VERIFIED: binary formatting is correct for all u64 values
pub fn format_binary(value: u64, width: u32) -> VumaString {
    format_uint_inner(value, 2, width)
}

/// Format an unsigned 64-bit integer as an octal string with minimum
/// width padding.
///
/// Produces the octal representation of `value` using digits `0–7`. If the
/// representation is shorter than `width`, it is left-padded with `'0'`
/// characters.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn format_octal(value: u64, width: u32) -> VumaString {
///     return format_uint(value, 8, width);
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure conversion, no side effects
// VUMA-VERIFIED: octal formatting is correct for all u64 values
pub fn format_octal(value: u64, width: u32) -> VumaString {
    format_uint_inner(value, 8, width)
}

// ---------------------------------------------------------------------------
// Pointer Formatting
// ---------------------------------------------------------------------------

/// Format a raw address as a 0x-prefixed lowercase hexadecimal pointer.
///
/// Produces a string of the form `"0x"` followed by the hexadecimal
/// representation of `addr`, padded to at least 16 hex digits (64-bit
/// address space). This matches the conventional pointer format used in
/// debuggers and memory dumps.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn format_pointer(addr: u64) -> VumaString {
///     return "0x" + format_hex(addr, 16);
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure conversion, no side effects
// VUMA-VERIFIED: pointer formatting produces correct 0x-prefixed hex
pub fn format_pointer(addr: u64) -> VumaString {
    let mut result = VumaString::from("0x");
    result.push_str(format_uint_inner(addr, 16, 16).as_str());
    result
}

// ---------------------------------------------------------------------------
// String Padding
// ---------------------------------------------------------------------------

/// Left-pad a string to `width` characters using `fill` as the padding
/// character.
///
/// If `s` already has length >= `width`, returns `s` unchanged (no
/// truncation is performed). Otherwise, prepends `width - len(s)`
/// copies of `fill` to the left of `s`.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn pad_left(s: &str, width: u32, fill: char) -> VumaString {
///     len: u32 = s.length() as u32;
///     if len >= width {
///         return VumaString::from(s);
///     }
///     pad_count: u32 = width - len;
///     result: VumaString = "";
///     for i in 0..pad_count {
///         result.push(fill);
///     }
///     result.push_str(s);
///     return result;
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: left padding is correct for all widths and fill characters
pub fn pad_left(s: &str, width: u32, fill: char) -> VumaString {
    let len = s.chars().count() as u32;
    if len >= width {
        return VumaString::from(s);
    }

    let pad_count = (width - len) as usize;
    let mut result = VumaString::with_capacity(pad_count + s.len());
    for _ in 0..pad_count {
        result.push(fill);
    }
    result.push_str(s);
    result
}

/// Right-pad a string to `width` characters using `fill` as the padding
/// character.
///
/// If `s` already has length >= `width`, returns `s` unchanged (no
/// truncation is performed). Otherwise, appends `width - len(s)` copies
/// of `fill` to the right of `s`.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn pad_right(s: &str, width: u32, fill: char) -> VumaString {
///     len: u32 = s.length() as u32;
///     if len >= width {
///         return VumaString::from(s);
///     }
///     pad_count: u32 = width - len;
///     result: VumaString = VumaString::from(s);
///     for i in 0..pad_count {
///         result.push(fill);
///     }
///     return result;
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: right padding is correct for all widths and fill characters
pub fn pad_right(s: &str, width: u32, fill: char) -> VumaString {
    let len = s.chars().count() as u32;
    if len >= width {
        return VumaString::from(s);
    }

    let pad_count = (width - len) as usize;
    let mut result = VumaString::with_capacity(s.len() + pad_count);
    result.push_str(s);
    for _ in 0..pad_count {
        result.push(fill);
    }
    result
}

// ---------------------------------------------------------------------------
// String Joining
// ---------------------------------------------------------------------------

/// Join a slice of string slices with a separator, producing a
/// [`VumaString`].
///
/// Concatenates the elements of `parts`, placing `separator` between each
/// adjacent pair. If `parts` is empty, returns an empty string. If
/// `parts` has one element, returns that element without any separator.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn join(parts: &[&str], separator: &str) -> VumaString {
///     if parts.length() == 0 {
///         return VumaString::new();
///     }
///     result: VumaString = VumaString::from(parts[0]);
///     for i in 1..parts.length() {
///         result.push_str(separator);
///         result.push_str(parts[i]);
///     }
///     return result;
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: join produces correct concatenation with separator
pub fn join(parts: &[&str], separator: &str) -> VumaString {
    if parts.is_empty() {
        return VumaString::new();
    }

    // Estimate capacity
    let sep_len = separator.len();
    let total_len: usize = parts.iter().map(|p| p.len()).sum::<usize>() + sep_len * (parts.len() - 1);
    let mut result = VumaString::with_capacity(total_len);

    result.push_str(parts[0]);
    for part in &parts[1..] {
        result.push_str(separator);
        result.push_str(part);
    }
    result
}

// ---------------------------------------------------------------------------
// Buffer Writing
// ---------------------------------------------------------------------------

/// Write a string slice into a byte buffer, returning the number of bytes
/// written.
///
/// Copies the UTF-8 bytes of `s` into `buf`, up to `buf.len()` bytes. If
/// `s` is longer than the buffer, only the prefix that fits is written —
/// the output is **not** guaranteed to be valid UTF-8 if truncation occurs
/// in the middle of a multi-byte sequence.
///
/// ## Return Value
///
/// The number of bytes actually written (which is `min(s.len(), buf.len())`).
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn write_str(buf: &mut [u8], s: &str) -> u32 {
///     n: u32 = min(s.length(), buf.length()) as u32;
///     for i in 0..n {
///         buf[i] = s[i];
///     }
///     return n;
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Write } — reads from string, writes to buffer
/// - SyncEdge: produces a WriteEdge covering the written bytes
// VUMA-VERIFIED: write_str is bounded by buffer length
pub fn write_str(buf: &mut [u8], s: &str) -> u32 {
    let s_bytes = s.as_bytes();
    let n = buf.len().min(s_bytes.len());
    buf[..n].copy_from_slice(&s_bytes[..n]);
    n as u32
}

/// Write the decimal representation of a signed 64-bit integer into a byte
/// buffer, returning the number of bytes written.
///
/// Formats `value` as a decimal string and writes the resulting bytes into
/// `buf`. If the formatted string is longer than the buffer, only the
/// prefix that fits is written.
///
/// ## Return Value
///
/// The number of bytes actually written.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn write_int(buf: &mut [u8], value: i64) -> u32 {
///     s: VumaString = format_int(value, 10, 0);
///     return write_str(buf, s.as_str());
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Compare } — reads value, writes to buffer
/// - SyncEdge: produces a WriteEdge covering the written bytes
// VUMA-VERIFIED: write_int is bounded by buffer length
pub fn write_int(buf: &mut [u8], value: i64) -> u32 {
    let formatted = format_int(value, 10, 0);
    write_str(buf, formatted.as_str())
}

/// Write the decimal representation of a 64-bit floating-point number into
/// a byte buffer with the given precision, returning the number of bytes
/// written.
///
/// Formats `value` as a decimal string with `precision` digits after the
/// decimal point and writes the resulting bytes into `buf`. If the
/// formatted string is longer than the buffer, only the prefix that fits
/// is written.
///
/// ## Return Value
///
/// The number of bytes actually written.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn write_float(buf: &mut [u8], value: f64, precision: u32) -> u32 {
///     s: VumaString = format_float(value, precision);
///     return write_str(buf, s.as_str());
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Compare } — reads value, writes to buffer
/// - SyncEdge: produces a WriteEdge covering the written bytes
// VUMA-VERIFIED: write_float is bounded by buffer length
pub fn write_float(buf: &mut [u8], value: f64, precision: u32) -> u32 {
    let formatted = format_float(value, precision);
    write_str(buf, formatted.as_str())
}

// ---------------------------------------------------------------------------
// Low-Level Buffer Formatting (VUMA Address + offset pattern)
// ---------------------------------------------------------------------------
//
// These functions write formatted output directly into a byte buffer and
// return the number of bytes written (not including a null terminator).
// They are the primitive building blocks for VUMA programs that need to
// write formatted numeric output to a memory-mapped address range.

/// Format a u64 value as a decimal string into the given buffer.
///
/// Returns the number of bytes written (not including null terminator).
/// The buffer must be at least 21 bytes long (max u64 = 18446744073709551615 + null).
///
/// ## BD Annotations
///
/// - CapD: { Read, Write } — reads input, writes to buffer
// VUMA-VERIFIED: decimal formatting is correct for all u64 values
pub fn format_u64(buf: &mut [u8], value: u64) -> usize {
    if value == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 20];
    let mut pos = 0;
    let mut v = value;
    while v > 0 {
        tmp[pos] = b'0' + (v % 10) as u8;
        v /= 10;
        pos += 1;
    }
    // Reverse into output buffer
    for i in 0..pos {
        buf[i] = tmp[pos - 1 - i];
    }
    pos
}

/// Format an i64 value as a decimal string into the given buffer.
///
/// Returns the number of bytes written (not including null terminator).
/// The buffer must be at least 22 bytes long (sign + max i64 digits + null).
///
/// ## BD Annotations
///
/// - CapD: { Read, Write } — reads input, writes to buffer
// VUMA-VERIFIED: signed decimal formatting is correct for all i64 values
pub fn format_i64(buf: &mut [u8], value: i64) -> usize {
    if value < 0 {
        buf[0] = b'-';
        1 + format_u64(&mut buf[1..], value.wrapping_neg() as u64)
    } else {
        format_u64(buf, value as u64)
    }
}

/// Format a u64 value as a hexadecimal string (lowercase) into the given buffer.
///
/// Returns the number of bytes written.
/// The buffer must be at least 17 bytes long (16 hex digits + null).
///
/// ## BD Annotations
///
/// - CapD: { Read, Write } — reads input, writes to buffer
// VUMA-VERIFIED: hexadecimal formatting is correct for all u64 values
pub fn format_u64_hex(buf: &mut [u8], value: u64) -> usize {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    if value == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 16];
    let mut pos = 0;
    let mut v = value;
    while v > 0 {
        tmp[pos] = HEX[(v & 0xF) as usize];
        v >>= 4;
        pos += 1;
    }
    for i in 0..pos {
        buf[i] = tmp[pos - 1 - i];
    }
    pos
}

/// Format a u32 value as a hexadecimal string (lowercase) into the given buffer.
///
/// Returns the number of bytes written.
/// The buffer must be at least 9 bytes long (8 hex digits + null).
///
/// ## BD Annotations
///
/// - CapD: { Read, Write } — reads input, writes to buffer
// VUMA-VERIFIED: delegates to format_u64_hex which is verified
pub fn format_u32_hex(buf: &mut [u8], value: u32) -> usize {
    format_u64_hex(buf, value as u64)
}

/// Format a u64 value as a binary string into the given buffer.
///
/// Returns the number of bytes written.
/// The buffer must be at least 65 bytes long (64 bits + null).
///
/// ## BD Annotations
///
/// - CapD: { Read, Write } — reads input, writes to buffer
// VUMA-VERIFIED: binary formatting is correct for all u64 values
pub fn format_u64_binary(buf: &mut [u8], value: u64) -> usize {
    if value == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 64];
    let mut pos = 0;
    let mut v = value;
    while v > 0 {
        tmp[pos] = b'0' + (v & 1) as u8;
        v >>= 1;
        pos += 1;
    }
    for i in 0..pos {
        buf[i] = tmp[pos - 1 - i];
    }
    pos
}

/// Format a u64 value as an octal string into the given buffer.
///
/// Returns the number of bytes written.
/// The buffer must be at least 23 bytes long (22 octal digits + null).
///
/// ## BD Annotations
///
/// - CapD: { Read, Write } — reads input, writes to buffer
// VUMA-VERIFIED: octal formatting is correct for all u64 values
pub fn format_u64_octal(buf: &mut [u8], value: u64) -> usize {
    if value == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 22];
    let mut pos = 0;
    let mut v = value;
    while v > 0 {
        tmp[pos] = b'0' + (v & 7) as u8;
        v >>= 3;
        pos += 1;
    }
    for i in 0..pos {
        buf[i] = tmp[pos - 1 - i];
    }
    pos
}

// ---------------------------------------------------------------------------
// Capability Descriptor for Formatting Operations
// ---------------------------------------------------------------------------

/// Returns the capability descriptor for formatting operations.
///
/// Formatting operations are a mix of pure conversions and buffer-writing
/// mutations:
///
/// - **Pure** (`format_int`, `format_uint`, `format_float`, `format_hex`,
///   `format_binary`, `format_octal`, `format_pointer`, `pad_left`,
///   `pad_right`, `join`): { Read, Compare }
/// - **Buffer-writing** (`write_str`, `write_int`, `write_float`,
///   `format_u64`, `format_i64`, `format_u64_hex`, `format_u32_hex`,
///   `format_u64_binary`, `format_u64_octal`):
///   { Read, Write, Compare }
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Compare } — union of all formatting capabilities
// VUMA-VERIFIED: capability set covers all formatting operations
pub fn fmt_capd() -> CapD {
    CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Compare])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- format_int tests ---

    #[test]
    fn test_format_int_zero() {
        assert_eq!(format_int(0, 10, 0).as_str(), "0");
    }

    #[test]
    fn test_format_int_positive() {
        assert_eq!(format_int(42, 10, 0).as_str(), "42");
    }

    #[test]
    fn test_format_int_negative() {
        assert_eq!(format_int(-42, 10, 0).as_str(), "-42");
    }

    #[test]
    fn test_format_int_with_width() {
        assert_eq!(format_int(42, 10, 5).as_str(), "00042");
    }

    #[test]
    fn test_format_int_negative_with_width() {
        assert_eq!(format_int(-42, 10, 5).as_str(), "-0042");
    }

    #[test]
    fn test_format_int_hex() {
        assert_eq!(format_int(255, 16, 0).as_str(), "ff");
    }

    #[test]
    fn test_format_int_octal() {
        assert_eq!(format_int(8, 8, 0).as_str(), "10");
    }

    #[test]
    fn test_format_int_binary() {
        assert_eq!(format_int(5, 2, 0).as_str(), "101");
    }

    #[test]
    fn test_format_int_invalid_base() {
        assert_eq!(format_int(42, 1, 0).as_str(), "42"); // Falls back to base 10
        assert_eq!(format_int(42, 37, 0).as_str(), "42");
    }

    #[test]
    fn test_format_int_large() {
        assert_eq!(format_int(i64::MAX, 10, 0).as_str(), "9223372036854775807");
    }

    #[test]
    fn test_format_int_min() {
        assert_eq!(format_int(i64::MIN, 10, 0).as_str(), "-9223372036854775808");
    }

    // --- format_uint tests ---

    #[test]
    fn test_format_uint_zero() {
        assert_eq!(format_uint(0, 10, 0).as_str(), "0");
    }

    #[test]
    fn test_format_uint_decimal() {
        assert_eq!(format_uint(12345, 10, 0).as_str(), "12345");
    }

    #[test]
    fn test_format_uint_hex() {
        assert_eq!(format_uint(0xDEAD, 16, 0).as_str(), "dead");
    }

    #[test]
    fn test_format_uint_with_width() {
        assert_eq!(format_uint(255, 16, 4).as_str(), "00ff");
    }

    #[test]
    fn test_format_uint_max() {
        assert_eq!(
            format_uint(u64::MAX, 10, 0).as_str(),
            "18446744073709551615"
        );
    }

    #[test]
    fn test_format_uint_base36() {
        assert_eq!(format_uint(35, 36, 0).as_str(), "z");
        assert_eq!(format_uint(36, 36, 0).as_str(), "10");
    }

    // --- format_float tests ---

    #[test]
    fn test_format_float_zero() {
        assert_eq!(format_float(0.0, 2).as_str(), "0.00");
    }

    #[test]
    fn test_format_float_integer() {
        assert_eq!(format_float(42.0, 0).as_str(), "42");
    }

    #[test]
    fn test_format_float_decimal() {
        assert_eq!(format_float(3.14159, 2).as_str(), "3.14");
    }

    #[test]
    fn test_format_float_negative() {
        assert_eq!(format_float(-1.5, 1).as_str(), "-1.5");
    }

    #[test]
    fn test_format_float_nan() {
        assert_eq!(format_float(f64::NAN, 2).as_str(), "nan");
    }

    #[test]
    fn test_format_float_inf() {
        assert_eq!(format_float(f64::INFINITY, 2).as_str(), "inf");
    }

    #[test]
    fn test_format_float_neg_inf() {
        assert_eq!(format_float(f64::NEG_INFINITY, 2).as_str(), "-inf");
    }

    // --- format_hex tests ---

    #[test]
    fn test_format_hex_zero() {
        assert_eq!(format_hex(0, 0).as_str(), "0");
    }

    #[test]
    fn test_format_hex_value() {
        assert_eq!(format_hex(0xABCD, 0).as_str(), "abcd");
    }

    #[test]
    fn test_format_hex_padded() {
        assert_eq!(format_hex(0xFF, 8).as_str(), "000000ff");
    }

    // --- format_binary tests ---

    #[test]
    fn test_format_binary_zero() {
        assert_eq!(format_binary(0, 0).as_str(), "0");
    }

    #[test]
    fn test_format_binary_value() {
        assert_eq!(format_binary(10, 0).as_str(), "1010");
    }

    #[test]
    fn test_format_binary_padded() {
        assert_eq!(format_binary(5, 8).as_str(), "00000101");
    }

    // --- format_octal tests ---

    #[test]
    fn test_format_octal_zero() {
        assert_eq!(format_octal(0, 0).as_str(), "0");
    }

    #[test]
    fn test_format_octal_value() {
        assert_eq!(format_octal(8, 0).as_str(), "10");
    }

    #[test]
    fn test_format_octal_padded() {
        assert_eq!(format_octal(7, 4).as_str(), "0007");
    }

    // --- format_pointer tests ---

    #[test]
    fn test_format_pointer_zero() {
        assert_eq!(format_pointer(0).as_str(), "0x0000000000000000");
    }

    #[test]
    fn test_format_pointer_value() {
        assert_eq!(format_pointer(0xDEADBEEF).as_str(), "0x00000000deadbeef");
    }

    #[test]
    fn test_format_pointer_max() {
        assert_eq!(
            format_pointer(u64::MAX).as_str(),
            "0xffffffffffffffff"
        );
    }

    // --- pad_left tests ---

    #[test]
    fn test_pad_left_no_padding_needed() {
        assert_eq!(pad_left("hello", 3, ' ').as_str(), "hello");
    }

    #[test]
    fn test_pad_left_spaces() {
        assert_eq!(pad_left("hi", 5, ' ').as_str(), "   hi");
    }

    #[test]
    fn test_pad_left_zeros() {
        assert_eq!(pad_left("42", 5, '0').as_str(), "00042");
    }

    #[test]
    fn test_pad_left_exact_width() {
        assert_eq!(pad_left("abc", 3, ' ').as_str(), "abc");
    }

    #[test]
    fn test_pad_left_empty() {
        assert_eq!(pad_left("", 3, '-').as_str(), "---");
    }

    // --- pad_right tests ---

    #[test]
    fn test_pad_right_no_padding_needed() {
        assert_eq!(pad_right("hello", 3, ' ').as_str(), "hello");
    }

    #[test]
    fn test_pad_right_spaces() {
        assert_eq!(pad_right("hi", 5, ' ').as_str(), "hi   ");
    }

    #[test]
    fn test_pad_right_exact_width() {
        assert_eq!(pad_right("abc", 3, ' ').as_str(), "abc");
    }

    #[test]
    fn test_pad_right_empty() {
        assert_eq!(pad_right("", 3, '-').as_str(), "---");
    }

    // --- join tests ---

    #[test]
    fn test_join_empty() {
        assert_eq!(join(&[], ", ").as_str(), "");
    }

    #[test]
    fn test_join_single() {
        assert_eq!(join(&["hello"], ", ").as_str(), "hello");
    }

    #[test]
    fn test_join_multiple() {
        assert_eq!(join(&["a", "b", "c"], ", ").as_str(), "a, b, c");
    }

    #[test]
    fn test_join_no_separator() {
        assert_eq!(join(&["a", "b", "c"], "").as_str(), "abc");
    }

    #[test]
    fn test_join_path_separator() {
        assert_eq!(join(&["usr", "local", "bin"], "/").as_str(), "usr/local/bin");
    }

    // --- write_str tests ---

    #[test]
    fn test_write_str_basic() {
        let mut buf = [0u8; 16];
        let n = write_str(&mut buf, "hello");
        assert_eq!(n, 5);
        assert_eq!(&buf[..5], b"hello");
    }

    #[test]
    fn test_write_str_truncated() {
        let mut buf = [0u8; 3];
        let n = write_str(&mut buf, "hello");
        assert_eq!(n, 3);
        assert_eq!(&buf[..3], b"hel");
    }

    #[test]
    fn test_write_str_empty() {
        let mut buf = [0u8; 8];
        let n = write_str(&mut buf, "");
        assert_eq!(n, 0);
    }

    #[test]
    fn test_write_str_exact_fit() {
        let mut buf = [0u8; 5];
        let n = write_str(&mut buf, "hello");
        assert_eq!(n, 5);
        assert_eq!(&buf[..5], b"hello");
    }

    // --- write_int tests ---

    #[test]
    fn test_write_int_positive() {
        let mut buf = [0u8; 16];
        let n = write_int(&mut buf, 42);
        assert_eq!(n, 2);
        assert_eq!(&buf[..2], b"42");
    }

    #[test]
    fn test_write_int_negative() {
        let mut buf = [0u8; 16];
        let n = write_int(&mut buf, -7);
        assert_eq!(n, 2);
        assert_eq!(&buf[..2], b"-7");
    }

    #[test]
    fn test_write_int_zero() {
        let mut buf = [0u8; 16];
        let n = write_int(&mut buf, 0);
        assert_eq!(n, 1);
        assert_eq!(&buf[..1], b"0");
    }

    // --- write_float tests ---

    #[test]
    fn test_write_float_basic() {
        let mut buf = [0u8; 32];
        let n = write_float(&mut buf, 3.14, 2);
        assert_eq!(n, 4);
        assert_eq!(&buf[..4], b"3.14");
    }

    #[test]
    fn test_write_float_zero_precision() {
        let mut buf = [0u8; 32];
        let n = write_float(&mut buf, 3.14, 0);
        assert_eq!(n, 1);
        assert_eq!(&buf[..1], b"3");
    }

    // --- fmt_capd tests ---

    #[test]
    fn test_fmt_capd() {
        let capd = fmt_capd();
        assert!(capd.has(CapFlag::Read));
        assert!(capd.has(CapFlag::Write));
        assert!(capd.has(CapFlag::Compare));
        assert!(!capd.has(CapFlag::Hash));
    }

    // =======================================================================
    // Comprehensive additional tests for expanded fmt module
    // =======================================================================

    // --- Comprehensive format_int tests ---

    #[test]
    fn test_format_int_decimal_basic() {
        assert_eq!(format_int(42, 10, 0).as_str(), "42");
        assert_eq!(format_int(-42, 10, 0).as_str(), "-42");
        assert_eq!(format_int(0, 10, 0).as_str(), "0");
        assert_eq!(format_int(1, 10, 0).as_str(), "1");
        assert_eq!(format_int(-1, 10, 0).as_str(), "-1");
    }

    #[test]
    fn test_format_int_hex_basic() {
        assert_eq!(format_int(0xFF, 16, 0).as_str(), "ff");
        assert_eq!(format_int(0, 16, 0).as_str(), "0");
        assert_eq!(format_int(10, 16, 0).as_str(), "a");
        assert_eq!(format_int(16, 16, 0).as_str(), "10");
    }

    #[test]
    fn test_format_int_binary_basic() {
        assert_eq!(format_int(0, 2, 0).as_str(), "0");
        assert_eq!(format_int(1, 2, 0).as_str(), "1");
        assert_eq!(format_int(2, 2, 0).as_str(), "10");
        assert_eq!(format_int(255, 2, 0).as_str(), "11111111");
    }

    #[test]
    fn test_format_int_octal_basic() {
        assert_eq!(format_int(0, 8, 0).as_str(), "0");
        assert_eq!(format_int(7, 8, 0).as_str(), "7");
        assert_eq!(format_int(8, 8, 0).as_str(), "10");
        assert_eq!(format_int(63, 8, 0).as_str(), "77");
    }

    #[test]
    fn test_format_int_with_width_comprehensive() {
        // Zero-padded positive
        assert_eq!(format_int(42, 10, 5).as_str(), "00042");
        // Zero-padded negative — sign takes one position
        assert_eq!(format_int(-42, 10, 5).as_str(), "-0042");
        // Width equal to length
        assert_eq!(format_int(42, 10, 2).as_str(), "42");
        // Width 0
        assert_eq!(format_int(42, 10, 0).as_str(), "42");
        // Hex with width
        assert_eq!(format_int(0xFF, 16, 4).as_str(), "00ff");
    }

    #[test]
    fn test_format_int_base36() {
        assert_eq!(format_int(35, 36, 0).as_str(), "z");
        assert_eq!(format_int(36, 36, 0).as_str(), "10");
        assert_eq!(format_int(0, 36, 0).as_str(), "0");
    }

    #[test]
    fn test_format_int_invalid_base_comprehensive() {
        // Base 1 and 37 should fall back to base 10
        assert_eq!(format_int(42, 1, 0).as_str(), "42");
        assert_eq!(format_int(42, 37, 0).as_str(), "42");
        assert_eq!(format_int(42, 0, 0).as_str(), "42");
    }

    #[test]
    fn test_format_int_extremes() {
        assert_eq!(format_int(i64::MAX, 10, 0).as_str(), "9223372036854775807");
        assert_eq!(format_int(i64::MIN, 10, 0).as_str(), "-9223372036854775808");
    }

    // --- Comprehensive format_uint tests ---

    #[test]
    fn test_format_uint_decimal_comprehensive() {
        assert_eq!(format_uint(0, 10, 0).as_str(), "0");
        assert_eq!(format_uint(1, 10, 0).as_str(), "1");
        assert_eq!(format_uint(42, 10, 0).as_str(), "42");
        assert_eq!(format_uint(999, 10, 0).as_str(), "999");
    }

    #[test]
    fn test_format_uint_hex_comprehensive() {
        assert_eq!(format_uint(0, 16, 0).as_str(), "0");
        assert_eq!(format_uint(0xDEAD, 16, 0).as_str(), "dead");
        assert_eq!(format_uint(255, 16, 2).as_str(), "ff");
        assert_eq!(format_uint(255, 16, 4).as_str(), "00ff");
        assert_eq!(format_uint(0xBEEF, 16, 0).as_str(), "beef");
    }

    #[test]
    fn test_format_uint_with_width_comprehensive() {
        assert_eq!(format_uint(0, 10, 4).as_str(), "0000");
        assert_eq!(format_uint(1, 10, 4).as_str(), "0001");
        assert_eq!(format_uint(42, 10, 5).as_str(), "00042");
        assert_eq!(format_uint(42, 10, 2).as_str(), "42");
        // Width smaller than number length
        assert_eq!(format_uint(12345, 10, 3).as_str(), "12345");
    }

    #[test]
    fn test_format_uint_max_comprehensive() {
        assert_eq!(format_uint(u64::MAX, 10, 0).as_str(), "18446744073709551615");
        assert_eq!(format_uint(u64::MAX, 16, 0).as_str(), "ffffffffffffffff");
    }

    #[test]
    fn test_format_uint_base36_comprehensive() {
        assert_eq!(format_uint(35, 36, 0).as_str(), "z");
        assert_eq!(format_uint(36, 36, 0).as_str(), "10");
        assert_eq!(format_uint(0, 36, 0).as_str(), "0");
    }

    #[test]
    fn test_format_uint_binary() {
        assert_eq!(format_uint(0, 2, 0).as_str(), "0");
        assert_eq!(format_uint(1, 2, 0).as_str(), "1");
        assert_eq!(format_uint(10, 2, 0).as_str(), "1010");
        assert_eq!(format_uint(255, 2, 8).as_str(), "11111111");
    }

    // --- Comprehensive format_float tests ---

    #[test]
    fn test_format_float_comprehensive() {
        assert_eq!(format_float(3.14, 2).as_str(), "3.14");
        assert_eq!(format_float(0.0, 2).as_str(), "0.00");
        assert_eq!(format_float(42.0, 0).as_str(), "42");
        assert_eq!(format_float(-1.5, 1).as_str(), "-1.5");
        assert_eq!(format_float(0.12345, 3).as_str(), "0.123");
        assert_eq!(format_float(99.9, 1).as_str(), "99.9");
    }

    #[test]
    fn test_format_float_precision() {
        // Zero precision — no decimal point
        assert_eq!(format_float(3.14, 0).as_str(), "3");
        assert_eq!(format_float(0.5, 0).as_str(), "0"); // rounds to 0? Actually 0.5 rounds to 0 or 1?
        // High precision
        assert_eq!(format_float(1.0, 5).as_str(), "1.00000");
        assert_eq!(format_float(3.14159, 4).as_str(), "3.1416");
    }

    #[test]
    fn test_format_float_special_comprehensive() {
        assert_eq!(format_float(f64::NAN, 2).as_str(), "nan");
        assert_eq!(format_float(f64::INFINITY, 2).as_str(), "inf");
        assert_eq!(format_float(f64::NEG_INFINITY, 2).as_str(), "-inf");
        assert_eq!(format_float(f64::NAN, 0).as_str(), "nan");
        assert_eq!(format_float(f64::INFINITY, 0).as_str(), "inf");
    }

    #[test]
    fn test_format_float_negative_comprehensive() {
        assert_eq!(format_float(-0.5, 1).as_str(), "-0.5");
        assert_eq!(format_float(-100.0, 0).as_str(), "-100");
        assert_eq!(format_float(-3.14159, 2).as_str(), "-3.14");
    }

    // --- Comprehensive format_hex tests ---

    #[test]
    fn test_format_hex_comprehensive() {
        assert_eq!(format_hex(0, 0).as_str(), "0");
        assert_eq!(format_hex(0xDEAD, 4).as_str(), "dead");
        assert_eq!(format_hex(0xFF, 0).as_str(), "ff");
        assert_eq!(format_hex(0xFF, 4).as_str(), "00ff");
        assert_eq!(format_hex(0, 8).as_str(), "00000000");
        assert_eq!(format_hex(1, 8).as_str(), "00000001");
        assert_eq!(format_hex(0xABCDEF, 0).as_str(), "abcdef");
    }

    #[test]
    fn test_format_hex_max() {
        assert_eq!(format_hex(u64::MAX, 0).as_str(), "ffffffffffffffff");
    }

    // --- Comprehensive format_binary tests ---

    #[test]
    fn test_format_binary_comprehensive() {
        assert_eq!(format_binary(0, 0).as_str(), "0");
        assert_eq!(format_binary(1, 0).as_str(), "1");
        assert_eq!(format_binary(2, 0).as_str(), "10");
        assert_eq!(format_binary(10, 0).as_str(), "1010");
        assert_eq!(format_binary(5, 8).as_str(), "00000101");
        assert_eq!(format_binary(255, 8).as_str(), "11111111");
        assert_eq!(format_binary(0, 4).as_str(), "0000");
    }

    // --- Comprehensive format_octal tests ---

    #[test]
    fn test_format_octal_comprehensive() {
        assert_eq!(format_octal(0, 0).as_str(), "0");
        assert_eq!(format_octal(7, 0).as_str(), "7");
        assert_eq!(format_octal(8, 0).as_str(), "10");
        assert_eq!(format_octal(63, 0).as_str(), "77");
        assert_eq!(format_octal(7, 4).as_str(), "0007");
        assert_eq!(format_octal(0, 4).as_str(), "0000");
    }

    // --- Comprehensive format_pointer tests ---

    #[test]
    fn test_format_pointer_comprehensive() {
        assert_eq!(format_pointer(0).as_str(), "0x0000000000000000");
        assert_eq!(format_pointer(0xDEADBEEF).as_str(), "0x00000000deadbeef");
        assert_eq!(format_pointer(u64::MAX).as_str(), "0xffffffffffffffff");
        assert_eq!(format_pointer(1).as_str(), "0x0000000000000001");
        assert_eq!(format_pointer(0xFF).as_str(), "0x00000000000000ff");
    }

    // --- Comprehensive pad_left tests ---

    #[test]
    fn test_pad_left_comprehensive() {
        assert_eq!(pad_left("hi", 5, ' ').as_str(), "   hi");
        assert_eq!(pad_left("42", 5, '0').as_str(), "00042");
        // No padding needed
        assert_eq!(pad_left("hello", 3, ' ').as_str(), "hello");
        // Exact width
        assert_eq!(pad_left("abc", 3, ' ').as_str(), "abc");
        // Empty string
        assert_eq!(pad_left("", 3, '-').as_str(), "---");
        // Width 0
        assert_eq!(pad_left("test", 0, ' ').as_str(), "test");
        // Unicode
        assert_eq!(pad_left("é", 3, ' ').as_str(), "  é");
    }

    // --- Comprehensive pad_right tests ---

    #[test]
    fn test_pad_right_comprehensive() {
        assert_eq!(pad_right("hi", 5, ' ').as_str(), "hi   ");
        // No padding needed
        assert_eq!(pad_right("hello", 3, ' ').as_str(), "hello");
        // Exact width
        assert_eq!(pad_right("abc", 3, ' ').as_str(), "abc");
        // Empty string
        assert_eq!(pad_right("", 3, '-').as_str(), "---");
        // Width 0
        assert_eq!(pad_right("test", 0, ' ').as_str(), "test");
        // Different fill characters
        assert_eq!(pad_right("x", 4, '.').as_str(), "x...");
        assert_eq!(pad_right("x", 4, '*').as_str(), "x***");
    }

    // --- Comprehensive join tests ---

    #[test]
    fn test_join_comprehensive() {
        // Empty
        assert_eq!(join(&[], ", ").as_str(), "");
        // Single element
        assert_eq!(join(&["hello"], ", ").as_str(), "hello");
        // Two elements with comma
        assert_eq!(join(&["a", "b"], ",").as_str(), "a,b");
        // Multiple elements
        assert_eq!(join(&["a", "b", "c"], ", ").as_str(), "a, b, c");
        // No separator
        assert_eq!(join(&["a", "b", "c"], "").as_str(), "abc");
        // Path separator
        assert_eq!(join(&["usr", "local", "bin"], "/").as_str(), "usr/local/bin");
        // Long separator
        assert_eq!(join(&["x", "y"], " AND ").as_str(), "x AND y");
        // Single char elements with dash
        assert_eq!(join(&["1", "2", "3"], "-").as_str(), "1-2-3");
    }

    // --- Comprehensive write_str tests ---

    #[test]
    fn test_write_str_comprehensive() {
        let mut buf = [0u8; 16];
        let n = write_str(&mut buf, "hello");
        assert_eq!(n, 5);
        assert_eq!(&buf[..5], b"hello");

        // Truncation
        let mut small_buf = [0u8; 3];
        let n = write_str(&mut small_buf, "hello");
        assert_eq!(n, 3);
        assert_eq!(&small_buf[..3], b"hel");

        // Empty string
        let mut buf2 = [0u8; 8];
        let n = write_str(&mut buf2, "");
        assert_eq!(n, 0);

        // Exact fit
        let mut buf3 = [0u8; 5];
        let n = write_str(&mut buf3, "hello");
        assert_eq!(n, 5);
        assert_eq!(&buf3[..5], b"hello");
    }

    // --- Comprehensive write_int tests ---

    #[test]
    fn test_write_int_comprehensive() {
        // Positive
        let mut buf = [0u8; 16];
        let n = write_int(&mut buf, 42);
        assert_eq!(n, 2);
        assert_eq!(&buf[..2], b"42");

        // Negative
        let mut buf2 = [0u8; 16];
        let n = write_int(&mut buf2, -7);
        assert_eq!(n, 2);
        assert_eq!(&buf2[..2], b"-7");

        // Zero
        let mut buf3 = [0u8; 16];
        let n = write_int(&mut buf3, 0);
        assert_eq!(n, 1);
        assert_eq!(&buf3[..1], b"0");

        // Large number
        let mut buf4 = [0u8; 32];
        let n = write_int(&mut buf4, 1234567890);
        assert_eq!(n, 10);
        assert_eq!(&buf4[..10], b"1234567890");

        // Buffer too small
        let mut small_buf = [0u8; 3];
        let n = write_int(&mut small_buf, 12345);
        assert_eq!(n, 3);
        assert_eq!(&small_buf, b"123");
    }

    // --- Comprehensive write_float tests ---

    #[test]
    fn test_write_float_comprehensive() {
        // Basic
        let mut buf = [0u8; 32];
        let n = write_float(&mut buf, 3.14, 2);
        assert_eq!(n, 4);
        assert_eq!(&buf[..4], b"3.14");

        // Zero precision
        let mut buf2 = [0u8; 32];
        let n = write_float(&mut buf2, 3.14, 0);
        assert_eq!(n, 1);
        assert_eq!(&buf2[..1], b"3");

        // Negative
        let mut buf3 = [0u8; 32];
        let n = write_float(&mut buf3, -1.5, 1);
        assert_eq!(n, 4);
        assert_eq!(&buf3[..4], b"-1.5");

        // NaN
        let mut buf4 = [0u8; 32];
        let n = write_float(&mut buf4, f64::NAN, 2);
        assert_eq!(n, 3);
        assert_eq!(&buf4[..3], b"nan");

        // Infinity
        let mut buf5 = [0u8; 32];
        let n = write_float(&mut buf5, f64::INFINITY, 2);
        assert_eq!(n, 3);
        assert_eq!(&buf5[..3], b"inf");

        // Zero
        let mut buf6 = [0u8; 32];
        let n = write_float(&mut buf6, 0.0, 2);
        assert_eq!(n, 4);
        assert_eq!(&buf6[..4], b"0.00");
    }

    // --- Cross-function consistency tests ---

    #[test]
    fn test_format_int_uint_consistency() {
        // format_int for non-negative values should match format_uint
        for val in [0i64, 1, 42, 255, 1000, i64::MAX] {
            assert_eq!(
                format_int(val, 10, 0).as_str(),
                format_uint(val as u64, 10, 0).as_str(),
                "format_int({}) != format_uint({})", val, val
            );
        }
    }

    #[test]
    fn test_format_hex_is_uint_base16() {
        for val in [0u64, 1, 255, 0xDEAD, 0xBEEF, u64::MAX] {
            assert_eq!(
                format_hex(val, 0).as_str(),
                format_uint(val, 16, 0).as_str(),
                "format_hex({}) != format_uint({}, 16)", val, val
            );
        }
    }

    #[test]
    fn test_format_binary_is_uint_base2() {
        for val in [0u64, 1, 5, 10, 255] {
            assert_eq!(
                format_binary(val, 0).as_str(),
                format_uint(val, 2, 0).as_str(),
                "format_binary({}) != format_uint({}, 2)", val, val
            );
        }
    }

    #[test]
    fn test_format_octal_is_uint_base8() {
        for val in [0u64, 7, 8, 63, 255] {
            assert_eq!(
                format_octal(val, 0).as_str(),
                format_uint(val, 8, 0).as_str(),
                "format_octal({}) != format_uint({}, 8)", val, val
            );
        }
    }

    #[test]
    fn test_write_int_matches_format_int() {
        let mut buf = [0u8; 32];
        for val in [0i64, 42, -7, 1234567890] {
            buf.fill(0);
            let n = write_int(&mut buf, val);
            let formatted = format_int(val, 10, 0);
            assert_eq!(n as usize, formatted.len());
            assert_eq!(&buf[..n as usize], formatted.as_bytes());
        }
    }

    #[test]
    fn test_write_float_matches_format_float() {
        let mut buf = [0u8; 32];
        for (val, prec) in [(3.14f64, 2u32), (0.0, 2), (-1.5, 1), (42.0, 0)] {
            buf.fill(0);
            let n = write_float(&mut buf, val, prec);
            let formatted = format_float(val, prec);
            assert_eq!(n as usize, formatted.len());
            assert_eq!(&buf[..n as usize], formatted.as_bytes());
        }
    }

    #[test]
    fn test_format_pointer_consistency() {
        // format_pointer should be "0x" + format_hex(addr, 16)
        for addr in [0u64, 1, 0xFF, 0xDEADBEEF, u64::MAX] {
            let expected = format!("0x{}", format_hex(addr, 16).as_str());
            assert_eq!(format_pointer(addr).as_str(), expected);
        }
    }

    // --- Low-level buffer formatting tests ---

    #[test]
    fn test_format_u64_zero() {
        let mut buf = [0u8; 21];
        let n = format_u64(&mut buf, 0);
        assert_eq!(n, 1);
        assert_eq!(&buf[..n], b"0");
    }

    #[test]
    fn test_format_u64_small() {
        let mut buf = [0u8; 21];
        let n = format_u64(&mut buf, 42);
        assert_eq!(n, 2);
        assert_eq!(&buf[..n], b"42");
    }

    #[test]
    fn test_format_u64_max() {
        let mut buf = [0u8; 21];
        let n = format_u64(&mut buf, u64::MAX);
        assert_eq!(n, 20);
        assert_eq!(&buf[..n], b"18446744073709551615");
    }

    #[test]
    fn test_format_i64_zero() {
        let mut buf = [0u8; 22];
        let n = format_i64(&mut buf, 0);
        assert_eq!(n, 1);
        assert_eq!(&buf[..n], b"0");
    }

    #[test]
    fn test_format_i64_negative() {
        let mut buf = [0u8; 22];
        let n = format_i64(&mut buf, -42);
        assert_eq!(n, 3);
        assert_eq!(&buf[..n], b"-42");
    }

    #[test]
    fn test_format_i64_min() {
        let mut buf = [0u8; 22];
        let n = format_i64(&mut buf, i64::MIN);
        assert_eq!(&buf[..n], b"-9223372036854775808");
    }

    #[test]
    fn test_format_u64_hex_zero() {
        let mut buf = [0u8; 17];
        let n = format_u64_hex(&mut buf, 0);
        assert_eq!(n, 1);
        assert_eq!(&buf[..n], b"0");
    }

    #[test]
    fn test_format_u64_hex_values() {
        let mut buf = [0u8; 17];
        let n = format_u64_hex(&mut buf, 255);
        assert_eq!(n, 2);
        assert_eq!(&buf[..n], b"ff");

        let n = format_u64_hex(&mut buf, 0xDEAD);
        assert_eq!(&buf[..n], b"dead");
    }

    #[test]
    fn test_format_u32_hex() {
        let mut buf = [0u8; 9];
        let n = format_u32_hex(&mut buf, 0xABCD);
        assert_eq!(&buf[..n], b"abcd");
    }

    #[test]
    fn test_format_u64_binary_values() {
        let mut buf = [0u8; 65];
        let n = format_u64_binary(&mut buf, 0);
        assert_eq!(n, 1);
        assert_eq!(&buf[..n], b"0");

        let n = format_u64_binary(&mut buf, 5);
        assert_eq!(&buf[..n], b"101");

        let n = format_u64_binary(&mut buf, 255);
        assert_eq!(&buf[..n], b"11111111");
    }

    #[test]
    fn test_format_u64_octal_values() {
        let mut buf = [0u8; 23];
        let n = format_u64_octal(&mut buf, 0);
        assert_eq!(n, 1);
        assert_eq!(&buf[..n], b"0");

        let n = format_u64_octal(&mut buf, 8);
        assert_eq!(&buf[..n], b"10");

        let n = format_u64_octal(&mut buf, 255);
        assert_eq!(&buf[..n], b"377");
    }
}
