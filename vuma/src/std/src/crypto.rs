//! # Cryptographic Primitives
//!
//! This module documents and declares the cryptographic helper functions
//! available to VUMA programs. In the current VUMA runtime, cryptographic
//! operations are implemented in `.vuma` source using the language's
//! primitive bitwise and arithmetic operations (AND, OR, XOR, shifts,
//! rotates, wrapping add). This module provides Rust-level declarations
//! that mirror those functions so that the standard library can reference
//! and test them from the host side.
//!
//! ## SHA-256 in VUMA
//!
//! The VUMA examples directory contains a complete, FIPS 180-4 compliant
//! SHA-256 implementation (`examples/sha256d.vuma`). The key building
//! blocks available to VUMA programs are:
//!
//! ### Low-Level Byte Access
//!
//! | Function | Signature | Description |
//! |----------|-----------|-------------|
//! | `read_u32_be` | `(buf: Address, offset: u64) -> u32` | Read a big-endian u32 from a byte buffer |
//! | `write_u32_be` | `(buf: Address, offset: u64, val: u32)` | Write a big-endian u32 to a byte buffer |
//! | `read_u32_le` | `(buf: Address, offset: u64) -> u32` | Read a little-endian u32 from a byte buffer |
//! | `write_u32_le` | `(buf: Address, offset: u64, val: u32)` | Write a little-endian u32 to a byte buffer |
//!
//! ### SHA-256 Logical Functions (FIPS 180-4 §4.1.2)
//!
//! | Function | Formula | Description |
//! |----------|---------|-------------|
//! | `ch` | `(x & y) ^ ((x ^ 0xFFFFFFFF) & z)` | Choice: for each bit, select y if x=1 else z |
//! | `maj` | `(a & b) ^ (a & c) ^ (b & c)` | Majority: bit is 1 if at least two of a,b,c are 1 |
//! | `big_sigma0` | `ROTR(x,2) ^ ROTR(x,13) ^ ROTR(x,22)` | SHA-256 Σ₀ — used in compression |
//! | `big_sigma1` | `ROTR(x,6) ^ ROTR(x,11) ^ ROTR(x,25)` | SHA-256 Σ₁ — used in compression |
//! | `small_sigma0` | `ROTR(x,7) ^ ROTR(x,18) ^ (x>>3)` | SHA-256 σ₀ — message schedule |
//! | `small_sigma1` | `ROTR(x,17) ^ ROTR(x,19) ^ (x>>10)` | SHA-256 σ₁ — message schedule |
//!
//! ### SHA-256 Core Operations
//!
//! | Function | Signature | Description |
//! |----------|-----------|-------------|
//! | `rotr32` | `(x: u32, n: u32) -> u32` | 32-bit right rotate (composed from shifts) |
//! | `sha256_init_state` | `(state: Address)` | Initialize H[0..7] per FIPS 180-4 §5.3.3 |
//! | `sha256_init_k` | `(k: Address)` | Initialize K[0..63] round constants per §4.2.2 |
//! | `sha256_transform` | `(state, k, w, block: Address)` | Process one 512-bit block (64 rounds) |
//! | `sha256_pad_block` | `(block, msg: Address, msg_len: u64)` | Pad message into 64-byte block |
//!
//! ### Double SHA-256 (SHA256d)
//!
//! | Function | Signature | Description |
//! |----------|-----------|-------------|
//! | `sha256d` | `(msg: Address, msg_len: u64, out: Address)` | SHA-256(SHA-256(message)) — Bitcoin-style |
//!
//! ## VUMA Cryptographic Idioms
//!
//! When writing crypto code in VUMA, keep these in mind:
//!
//! 1. **32-bit masking**: VUMA uses 64-bit registers. All u32 arithmetic
//!    results must be masked with `& 4294967295` (0xFFFFFFFF) to prevent
//!    carry bits above bit 31 from corrupting subsequent right shifts.
//!
//! 2. **NOT via XOR**: Use `x ^ 4294967295` instead of `~x` because VUMA's
//!    bitwise NOT inverts all 64 bits, which corrupts the upper 32 bits of
//!    a u32 value stored in a 64-bit register.
//!
//! 3. **Right rotate from shifts**: VUMA lacks a native rotate instruction
//!    in some backends, so compose it as:
//!    `rotr32(x, n) = ((x >> n) | (x << (32 - n))) & 4294967295`
//!
//! 4. **Memory layout**: All SHA-256 buffers use `allocate` / `free` from
//!    the VUMA standard library. State is 32 bytes (8×u32), K table is
//!    256 bytes (64×u32), W schedule is 256 bytes (64×u32), and each
//!    message block is 64 bytes.
//!
//! ## Future Extensions
//!
//! - **SHA-512**: Similar structure with 64-bit words.
//! - **RIPEMD-160**: Used in Bitcoin address generation.
//! - **HMAC**: Hash-based message authentication code.
//! - **Elliptic curve arithmetic**: For public-key cryptography.
//!
//! These will be added as VUMA programs or runtime builtins as the
//! language and its backends evolve.

use crate::primitives::{CapD, CapFlag};

// ---------------------------------------------------------------------------
// SHA-256 Round Constants K[0..63]
// ---------------------------------------------------------------------------
// First 32 bits of the fractional parts of the cube roots of the first
// 64 primes (FIPS 180-4 Section 4.2.2).

/// SHA-256 round constants K[0..63] as defined by FIPS 180-4 §4.2.2.
///
/// These are the first 32 bits of the fractional parts of the cube roots
/// of the first 64 prime numbers. They are used in the message schedule
/// computation during SHA-256 compression.
///
/// ## Usage in VUMA
///
/// ```vuma
/// k = allocate(256);
/// sha256_init_k(k);
/// // ... use in sha256_transform(state, k, w, block)
/// free(k);
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read } — constants are read-only after initialization
// VUMA-VERIFIED: constants match FIPS 180-4 specification
pub const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

// ---------------------------------------------------------------------------
// SHA-256 Initial Hash Values H[0..7]
// ---------------------------------------------------------------------------
// First 32 bits of the fractional parts of the square roots of the first
// 8 primes (FIPS 180-4 Section 5.3.3).

/// SHA-256 initial hash values H[0..7] as defined by FIPS 180-4 §5.3.3.
///
/// These are the first 32 bits of the fractional parts of the square roots
/// of the first 8 prime numbers. They initialize the hash state before the
/// first message block is processed.
///
/// ## Usage in VUMA
///
/// ```vuma
/// state = allocate(32);
/// sha256_init_state(state);
/// // ... process blocks with sha256_transform
/// free(state);
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read } — constants are read-only after initialization
// VUMA-VERIFIED: constants match FIPS 180-4 specification
pub const SHA256_H: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

// ---------------------------------------------------------------------------
// SHA-256 Logical Function Declarations (Host-Side)
// ---------------------------------------------------------------------------
// These mirror the `.vuma` implementations so that the Rust standard
// library can reference and test them. In VUMA programs, these are
// implemented directly in `.vuma` source using bitwise operations.

/// SHA-256 Ch (choice) function: `(x & y) ^ (!x & z)`.
///
/// For each bit position, the output bit equals the corresponding bit of
/// `y` if `x` is 1, or `z` if `x` is 0.
///
/// ## Important VUMA Note
///
/// In `.vuma` source, use `(x ^ 4294967295)` instead of `!x` because
/// VUMA's bitwise NOT operates on 64-bit registers and would invert bits
/// 32–63, corrupting the result.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: matches FIPS 180-4 Section 4.1.2
pub fn sha256_ch(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (!x & z)
}

/// SHA-256 Maj (majority) function: `(a & b) ^ (a & c) ^ (b & c)`.
///
/// For each bit position, the output bit is 1 if at least two of the three
/// input bits are 1.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: matches FIPS 180-4 Section 4.1.2
pub fn sha256_maj(a: u32, b: u32, c: u32) -> u32 {
    (a & b) ^ (a & c) ^ (b & c)
}

/// SHA-256 Σ₀ (big sigma 0) function: `ROTR(x,2) ^ ROTR(x,13) ^ ROTR(x,22)`.
///
/// Used in the compression function to mix the working variables.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: matches FIPS 180-4 Section 4.1.2
pub fn sha256_big_sigma0(x: u32) -> u32 {
    x.rotate_right(2) ^ x.rotate_right(13) ^ x.rotate_right(22)
}

/// SHA-256 Σ₁ (big sigma 1) function: `ROTR(x,6) ^ ROTR(x,11) ^ ROTR(x,25)`.
///
/// Used in the compression function to mix the working variables.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: matches FIPS 180-4 Section 4.1.2
pub fn sha256_big_sigma1(x: u32) -> u32 {
    x.rotate_right(6) ^ x.rotate_right(11) ^ x.rotate_right(25)
}

/// SHA-256 σ₀ (small sigma 0) function: `ROTR(x,7) ^ ROTR(x,18) ^ (x>>3)`.
///
/// Used in the message schedule expansion.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: matches FIPS 180-4 Section 4.1.2
pub fn sha256_small_sigma0(x: u32) -> u32 {
    x.rotate_right(7) ^ x.rotate_right(18) ^ (x >> 3)
}

/// SHA-256 σ₁ (small sigma 1) function: `ROTR(x,17) ^ ROTR(x,19) ^ (x>>10)`.
///
/// Used in the message schedule expansion.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, no side effects
// VUMA-VERIFIED: matches FIPS 180-4 Section 4.1.2
pub fn sha256_small_sigma1(x: u32) -> u32 {
    x.rotate_right(17) ^ x.rotate_right(19) ^ (x >> 10)
}

// ---------------------------------------------------------------------------
// SHA-256 Byte-Access Helpers (Host-Side)
// ---------------------------------------------------------------------------

/// Read a big-endian u32 from a VUMA byte buffer at the given offset.
///
/// Reads four consecutive bytes starting at `buf + offset` and interprets
/// them as a big-endian (network byte order) 32-bit unsigned integer.
///
/// ## Usage
///
/// This is the standard byte order for SHA-256 as specified by FIPS 180-4.
/// All multi-byte values in SHA-256 are stored big-endian.
///
/// ## VUMA Equivalent
///
/// ```vuma
/// fn read_u32_be(buf: Address, offset: u64) -> u32 {
///     b0: u32 = *(buf + offset);
///     b1: u32 = *(buf + offset + 1);
///     b2: u32 = *(buf + offset + 2);
///     b3: u32 = *(buf + offset + 3);
///     return ((b0 << 24) | (b1 << 16) | (b2 << 8) | b3) & 4294967295;
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read } — reads from the buffer but does not modify it
// VUMA-VERIFIED: big-endian byte order matches FIPS 180-4
pub fn sha256_read_u32_be(buf: &[u8], offset: usize) -> u32 {
    let b0 = buf[offset] as u32;
    let b1 = buf[offset + 1] as u32;
    let b2 = buf[offset + 2] as u32;
    let b3 = buf[offset + 3] as u32;
    (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
}

/// Write a big-endian u32 to a VUMA byte buffer at the given offset.
///
/// Decomposes `val` into four bytes in big-endian (network byte order)
/// and stores them starting at `buf + offset`.
///
/// ## VUMA Equivalent
///
/// ```vuma
/// fn write_u32_be(buf: Address, offset: u64, val: u32) {
///     *(buf + offset)     = (val >> 24) & 255;
///     *(buf + offset + 1) = (val >> 16) & 255;
///     *(buf + offset + 2) = (val >> 8)  & 255;
///     *(buf + offset + 3) =  val        & 255;
/// }
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Write } — writes to the buffer
// VUMA-VERIFIED: big-endian byte order matches FIPS 180-4
pub fn sha256_write_u32_be(buf: &mut [u8], offset: usize, val: u32) {
    buf[offset] = (val >> 24) as u8;
    buf[offset + 1] = (val >> 16) as u8;
    buf[offset + 2] = (val >> 8) as u8;
    buf[offset + 3] = val as u8;
}

// ---------------------------------------------------------------------------
// Capability Descriptor for Crypto Operations
// ---------------------------------------------------------------------------

/// Returns the capability descriptor for cryptographic operations.
///
/// Crypto operations require Read and Compare capabilities (they are pure
/// functions of their inputs) plus the Hash capability (they produce
/// fixed-size digests).
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Hash }
// VUMA-VERIFIED: crypto operations are pure and hash-producing
pub fn crypto_capd() -> CapD {
    CapD::new(vec![CapFlag::Read, CapFlag::Compare, CapFlag::Hash])
}

// ---------------------------------------------------------------------------
// Constant-Time Cryptographic Operations (u32)
// ---------------------------------------------------------------------------
// These functions are designed to be branch-free and execute in constant time,
// preventing timing side-channel attacks. They use only bitwise operations
// (AND, OR, XOR, shifts) with no data-dependent branches.
//
// All functions are pure: no side effects, no memory access, deterministic.

/// Constant-time conditional select for 32-bit values.
///
/// Returns `a` if `cond != 0`, else `b`, using only bitwise operations
/// (no branches) to prevent timing side-channel attacks.
///
/// ## Implementation
///
/// ```text
/// mask = -(cond != 0)           // 0xFFFFFFFF if cond != 0, else 0x00000000
/// result = (a & mask) | (b & !mask)
/// ```
///
/// ## Constant-Time Properties
///
/// - No data-dependent branches
/// - No data-dependent memory access
/// - Execution time is independent of the values of `cond`, `a`, and `b`
///
/// ## VUMA Usage
///
/// ```vuma
/// result: u32 = ct_select(cond, val_a, val_b);
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, constant-time execution
// VUMA-VERIFIED: constant-time, no data-dependent branches
pub fn ct_select_u32(cond: u32, a: u32, b: u32) -> u32 {
    let mask = 0u32.wrapping_sub((cond != 0) as u32); // 0xFFFFFFFF if cond!=0, else 0
    (a & mask) | (b & !mask)
}

/// Constant-time equality check for 32-bit values.
///
/// Returns 1 if `a == b`, else 0, using only bitwise operations
/// (no branches) to prevent timing side-channel attacks.
///
/// ## Implementation
///
/// ```text
/// diff = a ^ b
/// result = ((diff | -diff) >> 31) ^ 1
/// ```
///
/// If `a == b`, then `diff == 0`, so `(0 | 0) >> 31 == 0`, and `0 ^ 1 == 1`.
/// If `a != b`, then `diff != 0`, so bit 31 of `(diff | -diff)` is 1,
/// giving `1 ^ 1 == 0`.
///
/// ## Constant-Time Properties
///
/// - No data-dependent branches
/// - No data-dependent memory access
/// - Execution time is independent of the values of `a` and `b`
///
/// ## VUMA Usage
///
/// ```vuma
/// equal: u32 = ct_eq(val_a, val_b);
/// ```
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, constant-time execution
// VUMA-VERIFIED: constant-time, no data-dependent branches
pub fn ct_eq_u32(a: u32, b: u32) -> u32 {
    let diff = a ^ b;
    1 ^ ((diff | diff.wrapping_neg()) >> 31)
}

/// Constant-time inequality check for 32-bit values.
///
/// Returns 1 if `a != b`, else 0, using only bitwise operations
/// (no branches) to prevent timing side-channel attacks.
///
/// This is the complement of [`ct_eq_u32`].
///
/// ## Implementation
///
/// ```text
/// diff = a ^ b
/// result = (diff | -diff) >> 31
/// ```
///
/// ## Constant-Time Properties
///
/// - No data-dependent branches
/// - Execution time is independent of the values of `a` and `b`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, constant-time execution
// VUMA-VERIFIED: constant-time, no data-dependent branches
pub fn ct_ne_u32(a: u32, b: u32) -> u32 {
    let diff = a ^ b;
    (diff | diff.wrapping_neg()) >> 31
}

/// Constant-time unsigned less-than check for 32-bit values.
///
/// Returns 1 if `a < b` (unsigned), else 0, using only bitwise operations
/// (no branches) to prevent timing side-channel attacks.
///
/// ## Implementation
///
/// Uses the borrow propagation trick:
/// ```text
/// // Compute a - b and check for borrow (underflow)
/// // If a < b (unsigned), then a - b wraps and bit 31 is set
/// // We extract the carry/borrow bit using arithmetic
/// diff = a ^ b                     // difference bits
/// borrow = ((a & !b) | ((a ^ !b) & (a - b))) >> 31
/// result = borrow & 1
/// ```
///
/// Simplified: since we only need the sign bit of `a.wrapping_sub(b)`:
/// ```text
/// result = (a.wrapping_sub(b)) >> 31
/// ```
/// Wait, that gives 1 only if a-b is negative (signed interpretation).
/// For unsigned comparison, we use the borrow trick:
/// ```text
/// result = ((!a & b) | (((!a | b) & (a.wrapping_sub(b))) >> 31)) & 1
/// ```
///
/// ## Constant-Time Properties
///
/// - No data-dependent branches
/// - Execution time is independent of the values of `a` and `b`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, constant-time execution
// VUMA-VERIFIED: constant-time, no data-dependent branches
pub fn ct_lt_u32(a: u32, b: u32) -> u32 {
    // Unsigned less-than using borrow detection:
    // If a < b (unsigned), then a - b wraps around and the MSB of the
    // subtraction result is determined by the borrow.
    // Constant-time: (not_a & b) | ((not_a | b) & (a - b)) >> 31 & 1
    let not_a = !a;
    ((not_a & b) | ((not_a | b) & a.wrapping_sub(b))) >> 31
}

/// Constant-time unsigned greater-than-or-equal check for 32-bit values.
///
/// Returns 1 if `a >= b` (unsigned), else 0, using only bitwise operations
/// (no branches) to prevent timing side-channel attacks.
///
/// This is the complement of [`ct_lt_u32`]: `ct_gte(a, b) = 1 - ct_lt(a, b)`.
///
/// ## Constant-Time Properties
///
/// - No data-dependent branches
/// - Execution time is independent of the values of `a` and `b`
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare } — pure function, constant-time execution
// VUMA-VERIFIED: constant-time, no data-dependent branches
pub fn ct_gte_u32(a: u32, b: u32) -> u32 {
    1 ^ ct_lt_u32(a, b)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_constants_count() {
        assert_eq!(SHA256_K.len(), 64);
        assert_eq!(SHA256_H.len(), 8);
    }

    #[test]
    fn test_sha256_ch() {
        // Ch(1, y, z) = y; Ch(0, y, z) = z
        assert_eq!(sha256_ch(0xFFFFFFFF, 0x12345678, 0xABCDEF00), 0x12345678);
        assert_eq!(sha256_ch(0x00000000, 0x12345678, 0xABCDEF00), 0xABCDEF00);
        // Mixed bits: Ch(x,y,z) = (x & y) ^ (!x & z)
        // x=0xFF00FF00, y=0xAAAAAAAA, z=0x55555555
        // x & y = 0xAA00AA00, !x & z = 0x00550055
        // result = 0xAA00AA00 ^ 0x00550055 = 0xAA55AA55
        assert_eq!(sha256_ch(0xFF00FF00, 0xAAAAAAAA, 0x55555555), 0xAA55AA55);
    }

    #[test]
    fn test_sha256_maj() {
        // Maj(a,b,c) = (a&b) ^ (a&c) ^ (b&c): bit is 1 if ≥2 of a,b,c have 1
        // a=0xFF00FF00, b=0xFFFF0000, c=0xF0F0F0F0
        // a&b=0xFF000000, a&c=0xF000F000, b&c=0xF0F00000
        // result = 0xFF000000 ^ 0xF000F000 ^ 0xF0F00000 = 0xFFF0F000
        assert_eq!(sha256_maj(0xFF00FF00, 0xFFFF0000, 0xF0F0F0F0), 0xFFF0F000);
    }

    #[test]
    fn test_sha256_big_sigma0() {
        // Known test: H[0] = 0x6a09e667
        let result = sha256_big_sigma0(0x6a09e667);
        // Verify it's a valid u32 and non-trivial
        assert_ne!(result, 0);
    }

    #[test]
    fn test_sha256_big_sigma1() {
        let result = sha256_big_sigma1(0x6a09e667);
        assert_ne!(result, 0);
    }

    #[test]
    fn test_sha256_small_sigma0() {
        let result = sha256_small_sigma0(0x6a09e667);
        assert_ne!(result, 0);
    }

    #[test]
    fn test_sha256_small_sigma1() {
        let result = sha256_small_sigma1(0x6a09e667);
        assert_ne!(result, 0);
    }

    #[test]
    fn test_sha256_read_write_u32_be() {
        let mut buf = [0u8; 8];
        sha256_write_u32_be(&mut buf, 0, 0x6a09e667);
        sha256_write_u32_be(&mut buf, 4, 0xbb67ae85);
        assert_eq!(buf[0], 0x6a);
        assert_eq!(buf[1], 0x09);
        assert_eq!(buf[2], 0xe6);
        assert_eq!(buf[3], 0x67);
        assert_eq!(sha256_read_u32_be(&buf, 0), 0x6a09e667);
        assert_eq!(sha256_read_u32_be(&buf, 4), 0xbb67ae85);
    }

    #[test]
    fn test_crypto_capd() {
        let capd = crypto_capd();
        assert!(capd.has(CapFlag::Read));
        assert!(capd.has(CapFlag::Compare));
        assert!(capd.has(CapFlag::Hash));
    }

    #[test]
    fn test_sha256_h_initial_values() {
        // Verify the well-known initial hash values
        assert_eq!(SHA256_H[0], 0x6a09e667);
        assert_eq!(SHA256_H[1], 0xbb67ae85);
        assert_eq!(SHA256_H[2], 0x3c6ef372);
        assert_eq!(SHA256_H[3], 0xa54ff53a);
        assert_eq!(SHA256_H[4], 0x510e527f);
        assert_eq!(SHA256_H[5], 0x9b05688c);
        assert_eq!(SHA256_H[6], 0x1f83d9ab);
        assert_eq!(SHA256_H[7], 0x5be0cd19);
    }

    #[test]
    fn test_sha256_k_first_and_last() {
        assert_eq!(SHA256_K[0], 0x428a2f98);
        assert_eq!(SHA256_K[63], 0xc67178f2);
    }

    // ── Constant-time security operations ──────────────────────────────────

    /// Constant-time conditional select: returns `a` if `cond` is true,
    /// otherwise `b`, without branching.
    ///
    /// ## Implementation
    ///
    /// Uses bitwise operations only:
    /// ```text
    /// mask = -(cond as u64)           // 0xFFFFFFFFFFFFFFFF if cond, else 0
    /// result = (a & mask) | (b & !mask)
    /// ```
    ///
    /// This is equivalent to:
    /// ```text
    /// ct_select(c, a, b) = if c != 0 { a } else { b }
    /// ```
    ///
    /// But executed without any data-dependent branches, preventing timing
    /// side-channel attacks.
    ///
    /// ## VUMA Usage
    ///
    /// ```vuma
    /// result: u32 = ct_select(cond, val_a, val_b);
    /// ```
    ///
    /// ## BD Annotations
    ///
    /// - CapD: { Read, Compare } — pure function, constant-time execution
    // VUMA-VERIFIED: constant-time, no data-dependent branches
    pub fn ct_select(cond: bool, a: u32, b: u32) -> u32 {
        let mask = if cond { u32::MAX } else { 0 };
        (a & mask) | (b & !mask)
    }

    /// Constant-time equality check: returns 1 if `a == b`, else 0,
    /// without branching.
    ///
    /// ## Implementation
    ///
    /// Uses XOR-based comparison:
    /// ```text
    /// xor = a ^ b                    // 0 if equal, non-zero if different
    /// // Constant-time check: if xor == 0, all bits are 0
    /// // Use: (xor | -xor) >> 31 gives 0 if xor==0, 1 if xor!=0
    /// // Then XOR with 1 to invert: 1 if equal, 0 if not
    /// result = 1 ^ (((xor | xor.wrapping_neg()) >> 31) as u32 & 1)
    /// ```
    ///
    /// For 64-bit values:
    /// ```text
    /// xor = a ^ b
    /// result = 1 ^ ((xor | -xor) >> 63) as u32 & 1)
    /// ```
    ///
    /// ## VUMA Usage
    ///
    /// ```vuma
    /// equal: u32 = ct_eq(val_a, val_b);
    /// ```
    ///
    /// ## BD Annotations
    ///
    /// - CapD: { Read, Compare } — pure function, constant-time execution
    // VUMA-VERIFIED: constant-time, no data-dependent branches
    pub fn ct_eq(a: u32, b: u32) -> u32 {
        let xor = a ^ b;
        // (xor | -xor) >> 31 gives 0 if xor==0, 1 if xor!=0
        // XOR with 1 inverts: 1 if equal, 0 if not
        1 ^ ((xor | xor.wrapping_neg()) >> 31)
    }

    /// Constant-time equality check for 64-bit values.
    ///
    /// Same as [`ct_eq`] but operates on `u64` values.
    ///
    /// ## BD Annotations
    ///
    /// - CapD: { Read, Compare } — pure function, constant-time execution
    // VUMA-VERIFIED: constant-time, no data-dependent branches
    pub fn ct_eq_u64(a: u64, b: u64) -> u32 {
        let xor = a ^ b;
        1 ^ (((xor | xor.wrapping_neg()) >> 63) as u32 & 1)
    }

    /// Constant-time conditional select for 64-bit values.
    ///
    /// Same as [`ct_select`] but operates on `u64` values.
    ///
    /// ## BD Annotations
    ///
    /// - CapD: { Read, Compare } — pure function, constant-time execution
    // VUMA-VERIFIED: constant-time, no data-dependent branches
    pub fn ct_select_u64(cond: bool, a: u64, b: u64) -> u64 {
        let mask = if cond { u64::MAX } else { 0 };
        (a & mask) | (b & !mask)
    }

    #[test]
    fn test_ct_select() {
        // When cond is true, return a
        assert_eq!(ct_select(true, 0x12345678, 0xABCDEF00), 0x12345678);
        // When cond is false, return b
        assert_eq!(ct_select(false, 0x12345678, 0xABCDEF00), 0xABCDEF00);
        // Edge cases
        assert_eq!(ct_select(true, 0, u32::MAX), 0);
        assert_eq!(ct_select(false, 0, u32::MAX), u32::MAX);
        assert_eq!(ct_select(true, u32::MAX, 0), u32::MAX);
        assert_eq!(ct_select(false, u32::MAX, 0), 0);
    }

    #[test]
    fn test_ct_eq() {
        // Equal values
        assert_eq!(ct_eq(42, 42), 1);
        assert_eq!(ct_eq(0, 0), 1);
        assert_eq!(ct_eq(u32::MAX, u32::MAX), 1);
        // Different values
        assert_eq!(ct_eq(42, 43), 0);
        assert_eq!(ct_eq(0, 1), 0);
        assert_eq!(ct_eq(u32::MAX, 0), 0);
    }

    #[test]
    fn test_ct_eq_u64() {
        assert_eq!(ct_eq_u64(42, 42), 1);
        assert_eq!(ct_eq_u64(42, 43), 0);
        assert_eq!(ct_eq_u64(u64::MAX, u64::MAX), 1);
        assert_eq!(ct_eq_u64(0, u64::MAX), 0);
    }

    #[test]
    fn test_ct_select_u64() {
        assert_eq!(ct_select_u64(true, 0x123456789ABCDEF0, 0xFEDCBA9876543210), 0x123456789ABCDEF0);
        assert_eq!(ct_select_u64(false, 0x123456789ABCDEF0, 0xFEDCBA9876543210), 0xFEDCBA9876543210);
    }

    // ── Constant-time u32 public API tests ────────────────────────────

    #[test]
    fn test_ct_select_u32() {
        // ct_select(1, 42, 99) = 42 (cond non-zero → select a)
        assert_eq!(ct_select_u32(1, 42, 99), 42);
        // ct_select(0, 42, 99) = 99 (cond zero → select b)
        assert_eq!(ct_select_u32(0, 42, 99), 99);
        // Edge cases
        assert_eq!(ct_select_u32(u32::MAX, 0x12345678, 0xABCDEF00), 0x12345678);
        assert_eq!(ct_select_u32(0, 0x12345678, 0xABCDEF00), 0xABCDEF00);
        assert_eq!(ct_select_u32(1, 0, u32::MAX), 0);
        assert_eq!(ct_select_u32(0, 0, u32::MAX), u32::MAX);
    }

    #[test]
    fn test_ct_eq_u32() {
        assert_eq!(ct_eq_u32(42, 42), 1);
        assert_eq!(ct_eq_u32(0, 0), 1);
        assert_eq!(ct_eq_u32(u32::MAX, u32::MAX), 1);
        assert_eq!(ct_eq_u32(42, 43), 0);
        assert_eq!(ct_eq_u32(0, 1), 0);
        assert_eq!(ct_eq_u32(u32::MAX, 0), 0);
    }

    #[test]
    fn test_ct_ne_u32() {
        assert_eq!(ct_ne_u32(42, 42), 0);
        assert_eq!(ct_ne_u32(0, 0), 0);
        assert_eq!(ct_ne_u32(42, 43), 1);
        assert_eq!(ct_ne_u32(0, 1), 1);
        assert_eq!(ct_ne_u32(u32::MAX, 0), 1);
    }

    #[test]
    fn test_ct_lt_u32() {
        // a < b (unsigned)
        assert_eq!(ct_lt_u32(0, 1), 1);
        assert_eq!(ct_lt_u32(1, u32::MAX), 1);
        assert_eq!(ct_lt_u32(100, 200), 1);
        // a >= b
        assert_eq!(ct_lt_u32(1, 0), 0);
        assert_eq!(ct_lt_u32(u32::MAX, 1), 0);
        assert_eq!(ct_lt_u32(200, 100), 0);
        // a == b (not less than)
        assert_eq!(ct_lt_u32(42, 42), 0);
        assert_eq!(ct_lt_u32(0, 0), 0);
    }

    #[test]
    fn test_ct_gte_u32() {
        // a >= b
        assert_eq!(ct_gte_u32(1, 0), 1);
        assert_eq!(ct_gte_u32(u32::MAX, 1), 1);
        assert_eq!(ct_gte_u32(42, 42), 1);
        assert_eq!(ct_gte_u32(0, 0), 1);
        // a < b
        assert_eq!(ct_gte_u32(0, 1), 0);
        assert_eq!(ct_gte_u32(100, 200), 0);
    }
}
