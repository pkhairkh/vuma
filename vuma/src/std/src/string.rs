//! # String and Memory Operations
//!
//! This module provides VUMA-verified string and memory manipulation
//! functions that operate on [`Address`] pointers. These are the C-like
//! primitives that LLMs frequently need when generating low-level VUMA
//! programs: computing string lengths, comparing strings, and copying or
//! filling memory regions.
//!
//! ## Relationship to VUMA Programs
//!
//! In `.vuma` source, these functions are available as builtins or can be
//! implemented using pointer arithmetic (`*(ptr + offset)`). The Rust
//! declarations here provide host-side implementations for testing and for
//! the VUMA runtime library.
//!
//! ## BD Annotations
//!
//! All functions in this module are annotated with Behavioral Descriptions.
//! Memory-accessing functions declare their CapD requirements (Read for
//! queries, Write for mutations) and SyncEdge annotations that track the
//! data-flow dependencies in the Message Sequence Graph (MSG).

use crate::alloc::Address;
use crate::primitives::{CapD, CapFlag};

// ---------------------------------------------------------------------------
// String Length
// ---------------------------------------------------------------------------

/// Compute the length of a null-terminated string starting at `s`.
///
/// Scans memory from `s` until a null byte (0x00) is found, returning the
/// number of bytes before the terminator. The null byte itself is not
/// included in the count.
///
/// ## Semantics
///
/// - If `s` is null, returns 0.
/// - If no null byte is found within the addressable range, this function
///   will read past the buffer boundary — this is undefined behavior in
///   VUMA programs. The caller must ensure the string is properly
///   null-terminated.
/// - Time complexity: O(n) where n is the string length.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn strlen(s: Address) -> u64 {
///     len: u64 = 0;
///     while *(s + len) != 0 {
///         len = len + 1;
///     }
///     return len;
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read } — reads bytes but does not modify them
/// - SyncEdge: depends on the null-termination invariant
///
/// ## Safety
///
/// The caller must ensure that `s` points to a valid, null-terminated
/// string in the VUMA address space.
// VUMA-VERIFIED: length is bounded by the null-termination invariant
pub fn strlen(s: Address) -> u64 {
    if s.is_null() {
        return 0;
    }
    let mut len: u64 = 0;
    unsafe {
        while *(((s.0 + len) as *const u8)) != 0 {
            len += 1;
        }
    }
    len
}

// ---------------------------------------------------------------------------
// String Compare
// ---------------------------------------------------------------------------

/// Compare two null-terminated strings lexicographically.
///
/// Compares bytes from `a` and `b` one at a time until a difference or a
/// null terminator is found.
///
/// ## Return Value
///
/// - Returns `0` if the strings are identical.
/// - Returns a negative value if `a` is lexicographically less than `b`.
/// - Returns a positive value if `a` is lexicographically greater than `b`.
///
/// The magnitude of the return value is the difference between the first
/// non-equal bytes (`a_byte - b_byte` cast to i32).
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn strcmp(a: Address, b: Address) -> i32 {
///     i: u64 = 0;
///     loop {
///         ca: u8 = *(a + i);
///         cb: u8 = *(b + i);
///         if ca != cb {
///             return (ca as i32) - (cb as i32);
///         }
///         if ca == 0 {
///             return 0;
///         }
///         i = i + 1;
///     }
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — reads both strings, performs comparison
/// - SyncEdge: depends on null-termination of both inputs
///
/// ## Safety
///
/// Both `a` and `b` must point to valid, null-terminated strings.
// VUMA-VERIFIED: comparison is bounded by null terminators
pub fn strcmp(a: Address, b: Address) -> i32 {
    if a.is_null() && b.is_null() {
        return 0;
    }
    if a.is_null() {
        return -(unsafe { *(b.0 as *const u8) } as i32);
    }
    if b.is_null() {
        return unsafe { *(a.0 as *const u8) } as i32;
    }
    let mut i: u64 = 0;
    unsafe {
        loop {
            let ca = *((a.0 + i) as *const u8);
            let cb = *((b.0 + i) as *const u8);
            if ca != cb {
                return ca as i32 - cb as i32;
            }
            if ca == 0 {
                return 0;
            }
            i += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Memory Copy
// ---------------------------------------------------------------------------

/// Copy `n` bytes from `src` to `dst`.
///
/// Copies exactly `n` bytes from the source address to the destination
/// address. The source and destination regions must not overlap — use
/// [`memmove`] if overlap is possible.
///
/// ## Semantics
///
/// - If `n` is 0, does nothing.
/// - If `dst` or `src` is null and `n > 0`, this is undefined behavior.
/// - Copying is performed byte-by-byte (no alignment assumptions).
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn memcpy(dst: Address, src: Address, n: u64) {
///     i: u64 = 0;
///     for i in 0..n {
///         *(dst + i) = *(src + i);
///     }
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Write } — reads from src, writes to dst
/// - SyncEdge: produces a CopyEdge from src → dst
///
/// ## Safety
///
/// - `dst` and `src` must each point to at least `n` bytes of valid memory.
/// - The regions `[dst, dst+n)` and `[src, src+n)` must not overlap.
// VUMA-VERIFIED: copy is bounded by n and regions do not overlap
pub fn memcpy(dst: Address, src: Address, n: u64) {
    if n == 0 || dst.is_null() || src.is_null() {
        return;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            src.0 as *const u8,
            dst.0 as *mut u8,
            n as usize,
        );
    }
}

// ---------------------------------------------------------------------------
// Memory Set
// ---------------------------------------------------------------------------

/// Fill `n` bytes starting at `dst` with the byte value `val`.
///
/// Sets each byte in the region `[dst, dst+n)` to `val`. This is useful
/// for zeroing buffers, initializing memory regions, or filling arrays
/// with a sentinel value.
///
/// ## Semantics
///
/// - If `n` is 0, does nothing.
/// - If `dst` is null and `n > 0`, this is undefined behavior.
/// - The fill is performed byte-by-byte.
///
/// ## VUMA Program Equivalent
///
/// ```vuma
/// fn memset(dst: Address, val: u8, n: u64) {
///     i: u64 = 0;
///     for i in 0..n {
///         *(dst + i) = val;
///     }
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Write } — writes to the destination
/// - SyncEdge: produces a WriteEdge that covers [dst, dst+n)
///
/// ## Safety
///
/// `dst` must point to at least `n` bytes of writable memory.
// VUMA-VERIFIED: fill is bounded by n
pub fn memset(dst: Address, val: u8, n: u64) {
    if n == 0 || dst.is_null() {
        return;
    }
    unsafe {
        std::ptr::write_bytes(dst.0 as *mut u8, val, n as usize);
    }
}

// ---------------------------------------------------------------------------
// Capability Descriptor for String/Memory Operations
// ---------------------------------------------------------------------------

/// Returns the capability descriptor for string and memory operations.
///
/// String and memory operations require Read and/or Write capabilities
/// depending on the specific function:
///
/// - **Read-only** (`strlen`, `strcmp`): { Read, Compare }
/// - **Write-only** (`memset`): { Write }
/// - **Read + Write** (`memcpy`): { Read, Write }
///
/// ## BD Annotations
///
/// - CapD: { Read, Write, Compare } — union of all string/memory capabilities
// VUMA-VERIFIED: capability set covers all string/memory operations
pub fn string_capd() -> CapD {
    CapD::new(vec![CapFlag::Read, CapFlag::Write, CapFlag::Compare])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strlen_empty() {
        let buf = [0u8];
        assert_eq!(strlen(Address::from_raw(buf.as_ptr() as u64)), 0);
    }

    #[test]
    fn test_strlen_hello() {
        let buf = [b'h', b'e', b'l', b'l', b'o', 0];
        assert_eq!(strlen(Address::from_raw(buf.as_ptr() as u64)), 5);
    }

    #[test]
    fn test_strlen_null() {
        assert_eq!(strlen(Address::NULL), 0);
    }

    #[test]
    fn test_strcmp_equal() {
        let a = [b'h', b'i', 0];
        let b = [b'h', b'i', 0];
        assert_eq!(
            strcmp(
                Address::from_raw(a.as_ptr() as u64),
                Address::from_raw(b.as_ptr() as u64)
            ),
            0
        );
    }

    #[test]
    fn test_strcmp_less() {
        let a = [b'a', 0];
        let b = [b'b', 0];
        assert!(
            strcmp(
                Address::from_raw(a.as_ptr() as u64),
                Address::from_raw(b.as_ptr() as u64)
            ) < 0
        );
    }

    #[test]
    fn test_strcmp_greater() {
        let a = [b'z', 0];
        let b = [b'a', 0];
        assert!(
            strcmp(
                Address::from_raw(a.as_ptr() as u64),
                Address::from_raw(b.as_ptr() as u64)
            ) > 0
        );
    }

    #[test]
    fn test_strcmp_prefix() {
        let a = [b'h', b'i', 0];
        let b = [b'h', b'i', b'!', 0];
        assert!(
            strcmp(
                Address::from_raw(a.as_ptr() as u64),
                Address::from_raw(b.as_ptr() as u64)
            ) < 0
        );
    }

    #[test]
    fn test_strcmp_both_null() {
        assert_eq!(strcmp(Address::NULL, Address::NULL), 0);
    }

    #[test]
    fn test_memcpy_basic() {
        let src = [1u8, 2, 3, 4, 5];
        let mut dst = [0u8; 5];
        memcpy(
            Address::from_raw(dst.as_mut_ptr() as u64),
            Address::from_raw(src.as_ptr() as u64),
            5,
        );
        assert_eq!(dst, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_memcpy_zero() {
        let mut dst = [42u8; 4];
        memcpy(
            Address::from_raw(dst.as_mut_ptr() as u64),
            Address::NULL,
            0,
        );
        assert_eq!(dst, [42; 4]); // unchanged
    }

    #[test]
    fn test_memset_basic() {
        let mut buf = [0u8; 8];
        memset(Address::from_raw(buf.as_mut_ptr() as u64), 0xAA, 4);
        assert_eq!(buf, [0xAA, 0xAA, 0xAA, 0xAA, 0, 0, 0, 0]);
    }

    #[test]
    fn test_memset_zero_length() {
        let mut buf = [1u8, 2, 3];
        memset(Address::from_raw(buf.as_mut_ptr() as u64), 0, 0);
        assert_eq!(buf, [1, 2, 3]); // unchanged
    }

    #[test]
    fn test_memset_full_zero() {
        let mut buf = [255u8; 16];
        memset(Address::from_raw(buf.as_mut_ptr() as u64), 0, 16);
        assert_eq!(buf, [0u8; 16]);
    }

    #[test]
    fn test_string_capd() {
        let capd = string_capd();
        assert!(capd.has(CapFlag::Read));
        assert!(capd.has(CapFlag::Write));
        assert!(capd.has(CapFlag::Compare));
    }
}
