//! Memory address representation for VUMA.
//!
//! [`Address`] is a newtype wrapper around `u64` that represents a virtual
//! memory address. It provides type-safe arithmetic, alignment helpers, and
//! hex-formatted display so that addresses always appear as `0x`-prefixed
//! lowercase hexadecimal (e.g. `0x00007f8a3c001000`).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign, Sub};

/// A virtual memory address.
///
/// This is the fundamental unit of location in the VUMA memory model. All
/// region bounds, derivation proven ranges, and access targets are expressed
/// as [`Address`] values.
///
/// # Examples
///
/// ```
/// use vuma_core::address::Address;
///
/// let base = Address::from(0x1000_u64);
/// let ptr  = base.offset(0x40);
/// assert_eq!(ptr, Address::from(0x1040_u64));
/// assert_eq!(format!("{}", base), "0x0000000000001000");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Address(pub u64);

impl Address {
    /// The null address (`0x0`).
    pub const NULL: Address = Address(0);

    /// Create an [`Address`] from a raw `u64`.
    pub const fn new(raw: u64) -> Self {
        Address(raw)
    }

    /// Offset this address by a signed amount.
    ///
    /// If the resulting address would wrap around the boundaries of `u64`
    /// address space, the operation saturates (clamps to `0` or `MAX`).
    pub fn offset(self, by: i64) -> Address {
        if by >= 0 {
            self.0.saturating_add(by as u64).into()
        } else {
            self.0.saturating_sub((-by) as u64).into()
        }
    }

    /// Returns `true` if this is the null address.
    pub fn is_null(self) -> bool {
        self.0 == 0
    }

    /// Align this address **up** to the given alignment boundary.
    ///
    /// `align` must be a power of two; if it is not, the result is
    /// undefined at the bit level but will still compile. Callers should
    /// ensure the invariant.
    pub fn align_to(self, align: u64) -> Address {
        debug_assert!(align.is_power_of_two(), "alignment must be a power of two");
        let mask = align - 1;
        Address((self.0 + mask) & !mask)
    }

    /// The raw `u64` value.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Trait implementations
// ---------------------------------------------------------------------------

impl From<u64> for Address {
    fn from(v: u64) -> Self {
        Address(v)
    }
}

impl From<Address> for u64 {
    fn from(a: Address) -> Self {
        a.0
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:016x}", self.0)
    }
}

impl fmt::LowerHex for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// `Address + u64` → offset forward by `rhs` bytes.
impl Add<u64> for Address {
    type Output = Address;

    fn add(self, rhs: u64) -> Self::Output {
        Address(self.0 + rhs)
    }
}

/// `Address + Address` is intentionally **not** provided (adding two addresses
/// is semantically wrong). Use `Address + u64` for offsets instead.
///
/// `Address - u64` → offset backward by `rhs` bytes.
impl Sub<u64> for Address {
    type Output = Address;

    fn sub(self, rhs: u64) -> Self::Output {
        Address(self.0 - rhs)
    }
}

/// `Address - Address` → distance in bytes between two addresses.
impl Sub<Address> for Address {
    type Output = i64;

    fn sub(self, rhs: Address) -> Self::Output {
        (self.0 as i64) - (rhs.0 as i64)
    }
}

/// `Address += u64` → in-place forward offset.
impl AddAssign<u64> for Address {
    fn add_assign(&mut self, rhs: u64) {
        self.0 += rhs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_address() {
        assert!(Address::NULL.is_null());
        assert!(!Address::from(1_u64).is_null());
    }

    #[test]
    fn offset_forward() {
        let a = Address::from(0x1000_u64);
        assert_eq!(a.offset(0x40), Address::from(0x1040_u64));
    }

    #[test]
    fn offset_backward() {
        let a = Address::from(0x1040_u64);
        assert_eq!(a.offset(-0x40), Address::from(0x1000_u64));
    }

    #[test]
    fn align_to() {
        let a = Address::from(0x1001_u64);
        assert_eq!(a.align_to(0x1000), Address::from(0x2000_u64));

        let b = Address::from(0x1000_u64);
        assert_eq!(b.align_to(0x1000), Address::from(0x1000_u64));
    }

    #[test]
    fn display_hex() {
        let a = Address::from(0x00007f8a3c001000_u64);
        assert_eq!(format!("{}", a), "0x00007f8a3c001000");
    }

    #[test]
    fn arithmetic() {
        let a = Address::from(0x1000_u64);
        assert_eq!(a + 0x10_u64, Address::from(0x1010_u64));
        assert_eq!(a - 0x10_u64, Address::from(0x0FF0_u64));
        assert_eq!(
            Address::from(0x1010_u64) - Address::from(0x1000_u64),
            0x10_i64
        );
    }

    #[test]
    fn add_assign() {
        let mut a = Address::from(0x1000_u64);
        a += 0x20_u64;
        assert_eq!(a, Address::from(0x1020_u64));
    }
}
