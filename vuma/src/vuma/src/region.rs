//! Memory region tracking.
//!
//! A [`Region`] represents a contiguous span of virtual memory that has been
//! allocated, mapped, or otherwise reserved by the program. Regions are the
//! top-level containers in the VUMA memory model; every pointer derivation
//! ultimately traces back to a region.

use crate::address::Address;
use crate::program_point::ProgramPoint;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for a memory region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegionId(pub u64);

impl fmt::Display for RegionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "R{}", self.0)
    }
}

/// The lifecycle status of a memory region.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RegionStatus {
    /// Heap-allocated and currently live.
    Allocated,
    /// Has been explicitly freed.
    Freed,
    /// Stack-allocated frame storage.
    Stack,
    /// Memory-mapped region (e.g. `mmap`).
    Mapped,
    /// Device / MMIO memory.
    Device,
    /// Allocated but never freed (leak detected).
    Leaked,
}

impl fmt::Display for RegionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegionStatus::Allocated => write!(f, "allocated"),
            RegionStatus::Freed => write!(f, "freed"),
            RegionStatus::Stack => write!(f, "stack"),
            RegionStatus::Mapped => write!(f, "mapped"),
            RegionStatus::Device => write!(f, "device"),
            RegionStatus::Leaked => write!(f, "leaked"),
        }
    }
}

/// A contiguous span of virtual memory.
///
/// A region records *where* and *when* it was allocated (and optionally
/// freed), who owns it, and its current lifecycle status.
///
/// # Invariants
///
/// - `size > 0` — zero-sized regions are not represented.
/// - The range `[base, base + size)` must not wrap around the `u64` address
///   space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    /// Unique identifier for this region.
    pub id: RegionId,
    /// Base (lowest) address of the region.
    pub base: Address,
    /// Size in bytes.
    pub size: u64,
    /// Current lifecycle status.
    pub status: RegionStatus,
    /// Source location at which the region was allocated.
    pub alloc_point: ProgramPoint,
    /// Source location at which the region was freed, if applicable.
    pub free_point: Option<ProgramPoint>,
    /// Semantic owner context (e.g. `"thread-3"`, `"GPU-0"`), if known.
    pub owner_context: Option<String>,
}

impl Region {
    /// Returns the end address (exclusive) of this region.
    ///
    /// `end == base + size`
    pub fn end(&self) -> Address {
        self.base + self.size
    }

    /// Returns `true` if `addr` falls within `[base, base + size)`.
    pub fn contains(&self, addr: Address) -> bool {
        addr >= self.base && addr < self.end()
    }

    /// Returns `true` if this region's address range overlaps with `other`.
    pub fn overlaps(&self, other: &Region) -> bool {
        self.base < other.end() && other.base < self.end()
    }

    /// Returns `true` if this region was allocated at the given program point.
    ///
    /// Useful for provenance checks that ask "was the memory that this
    /// pointer came from allocated here?"
    pub fn is_allocated_at(&self, pp: &ProgramPoint) -> bool {
        self.alloc_point == *pp
    }

    /// Returns `true` if this region is currently live (i.e. not freed or
    /// leaked in a way that makes further access invalid).
    pub fn is_live(&self) -> bool {
        matches!(
            self.status,
            RegionStatus::Allocated
                | RegionStatus::Stack
                | RegionStatus::Mapped
                | RegionStatus::Device
        )
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Region {} [{}..{}) 0x{:x} bytes status={} alloc={}",
            self.id,
            self.base,
            self.end(),
            self.size,
            self.status,
            self.alloc_point,
        )?;
        if let Some(ref fp) = self.free_point {
            write!(f, " free={}", fp)?;
        }
        if let Some(ref owner) = self.owner_context {
            write!(f, " owner={}", owner)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_pp(line: u32) -> ProgramPoint {
        ProgramPoint::new("test.vu", line, 1)
    }

    #[test]
    fn contains_address() {
        let r = Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x100,
            status: RegionStatus::Allocated,
            alloc_point: dummy_pp(1),
            free_point: None,
            owner_context: None,
        };
        assert!(r.contains(Address::from(0x1000_u64)));
        assert!(r.contains(Address::from(0x10FF_u64)));
        assert!(!r.contains(Address::from(0x1100_u64))); // end is exclusive
        assert!(!r.contains(Address::from(0x0FFF_u64)));
    }

    #[test]
    fn overlaps() {
        let r1 = Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x100,
            status: RegionStatus::Allocated,
            alloc_point: dummy_pp(1),
            free_point: None,
            owner_context: None,
        };
        let r2 = Region {
            id: RegionId(2),
            base: Address::from(0x1050_u64),
            size: 0x100,
            status: RegionStatus::Allocated,
            alloc_point: dummy_pp(2),
            free_point: None,
            owner_context: None,
        };
        assert!(r1.overlaps(&r2));
        assert!(r2.overlaps(&r1));

        let r3 = Region {
            id: RegionId(3),
            base: Address::from(0x2000_u64),
            size: 0x100,
            status: RegionStatus::Allocated,
            alloc_point: dummy_pp(3),
            free_point: None,
            owner_context: None,
        };
        assert!(!r1.overlaps(&r3));
    }

    #[test]
    fn is_live() {
        let mut r = Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x100,
            status: RegionStatus::Allocated,
            alloc_point: dummy_pp(1),
            free_point: None,
            owner_context: None,
        };
        assert!(r.is_live());

        r.status = RegionStatus::Freed;
        assert!(!r.is_live());

        r.status = RegionStatus::Leaked;
        assert!(!r.is_live());
    }

    #[test]
    fn is_allocated_at() {
        let pp = dummy_pp(42);
        let r = Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x100,
            status: RegionStatus::Allocated,
            alloc_point: pp.clone(),
            free_point: None,
            owner_context: None,
        };
        assert!(r.is_allocated_at(&pp));
        assert!(!r.is_allocated_at(&dummy_pp(99)));
    }
}
