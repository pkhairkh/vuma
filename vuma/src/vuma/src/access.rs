//! Memory access events.
//!
//! An [`Access`] records a single read or write operation against memory at a
//! particular program point. Accesses are the leaf nodes of the Memory State
//! Graph (MSG) and are the key inputs to data-race and aliasing analyses.

use crate::address::Address;
use crate::derivation::DerivationId;
use crate::program_point::ProgramPoint;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for a memory access event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccessId(pub u64);

impl fmt::Display for AccessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "A{}", self.0)
    }
}

/// The kind of memory access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccessKind {
    /// A read from memory.
    Read,
    /// A write to memory.
    Write,
}

impl fmt::Display for AccessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AccessKind::Read => write!(f, "read"),
            AccessKind::Write => write!(f, "write"),
        }
    }
}

/// A single memory access event.
///
/// An access targets a specific [`DerivationId`] (which carries the
/// provenance chain), has a kind (read/write), a size in bytes, and occurs
/// at a particular [`ProgramPoint`].
///
/// # Conflict detection
///
/// Two accesses *conflict* if their byte ranges overlap and at least one
/// of them is a write. This is the classic condition for a potential
/// data race (absent synchronisation).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Access {
    /// Unique identifier for this access.
    pub id: AccessId,
    /// The pointer derivation that this access targets.
    pub target: DerivationId,
    /// Read or write.
    pub kind: AccessKind,
    /// Number of bytes accessed.
    pub size: u64,
    /// Where in the source program this access occurs.
    pub program_point: ProgramPoint,
}

impl Access {
    /// Create a new access event.
    pub fn new(
        id: AccessId,
        target: DerivationId,
        kind: AccessKind,
        size: u64,
        program_point: ProgramPoint,
    ) -> Self {
        Self {
            id,
            target,
            kind,
            size,
            program_point,
        }
    }

    /// Returns the byte range `[start, start + size)` for this access,
    /// given the base address of the derivation it targets.
    ///
    /// **Note:** The caller must supply the actual resolved base address;
    /// this method only computes the offset range.
    pub fn byte_range_at(&self, base: Address) -> (Address, Address) {
        let start = base;
        let end = base + self.size;
        (start, end)
    }

    /// Returns `true` if this access conflicts with `other`.
    ///
    /// Two accesses conflict when:
    /// 1. Their byte ranges overlap, **and**
    /// 2. At least one of them is a [`AccessKind::Write`].
    ///
    /// `self_base` and `other_base` are the resolved base addresses for
    /// each access's target derivation.
    pub fn conflicts_with(&self, other: &Access, self_base: Address, other_base: Address) -> bool {
        let (s_start, s_end) = self.byte_range_at(self_base);
        let (o_start, o_end) = other.byte_range_at(other_base);

        // Ranges overlap?
        let overlaps = s_start < o_end && o_start < s_end;

        // At least one write?
        let has_write = self.kind == AccessKind::Write || other.kind == AccessKind::Write;

        overlaps && has_write
    }
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Access {} target={} kind={} size={} @ {}",
            self.id, self.target, self.kind, self.size, self.program_point,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_pp(line: u32) -> ProgramPoint {
        ProgramPoint::new("test.vu", line, 1)
    }

    #[test]
    fn byte_range() {
        let a = Access::new(
            AccessId(1),
            DerivationId(10),
            AccessKind::Read,
            8,
            dummy_pp(1),
        );
        let base = Address::from(0x1000_u64);
        let (start, end) = a.byte_range_at(base);
        assert_eq!(start, Address::from(0x1000_u64));
        assert_eq!(end, Address::from(0x1008_u64));
    }

    #[test]
    fn conflicts_with_write_read_overlap() {
        let write = Access::new(
            AccessId(1),
            DerivationId(10),
            AccessKind::Write,
            8,
            dummy_pp(1),
        );
        let read = Access::new(
            AccessId(2),
            DerivationId(10),
            AccessKind::Read,
            4,
            dummy_pp(2),
        );

        let base1 = Address::from(0x1000_u64);
        let base2 = Address::from(0x1004_u64);

        // Overlap [0x1000, 0x1008) ∩ [0x1004, 0x1008) = [0x1004, 0x1008)
        assert!(write.conflicts_with(&read, base1, base2));
    }

    #[test]
    fn no_conflict_two_reads() {
        let r1 = Access::new(
            AccessId(1),
            DerivationId(10),
            AccessKind::Read,
            8,
            dummy_pp(1),
        );
        let r2 = Access::new(
            AccessId(2),
            DerivationId(10),
            AccessKind::Read,
            8,
            dummy_pp(2),
        );

        let base = Address::from(0x1000_u64);
        assert!(!r1.conflicts_with(&r2, base, base));
    }

    #[test]
    fn no_conflict_disjoint_ranges() {
        let w1 = Access::new(
            AccessId(1),
            DerivationId(10),
            AccessKind::Write,
            4,
            dummy_pp(1),
        );
        let w2 = Access::new(
            AccessId(2),
            DerivationId(11),
            AccessKind::Write,
            4,
            dummy_pp(2),
        );

        let base1 = Address::from(0x1000_u64);
        let base2 = Address::from(0x2000_u64);

        assert!(!w1.conflicts_with(&w2, base1, base2));
    }
}
