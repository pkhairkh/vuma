//! # Structured Judgments
//!
//! Typed judgment forms that replace string-matching in proof rule application.
//! Each variant represents a specific logical statement about the program's
//! memory state, enabling structural matching instead of fragile string
//! comparison.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Typed ID newtypes
// ---------------------------------------------------------------------------

/// Unique identifier for a memory region.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RegionId(pub u64);

/// Unique identifier for a resource (lock, buffer, etc.).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(pub u64);

/// Unique identifier for a pointer derivation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PointerId(pub u64);

/// Unique identifier for a variable.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VariableId(pub u64);

/// Unique identifier for an event in the happens-before relation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventId(pub u64);

impl std::fmt::Display for RegionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "region#{}", self.0)
    }
}

impl std::fmt::Display for ResourceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "resource#{}", self.0)
    }
}

impl std::fmt::Display for PointerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pointer#{}", self.0)
    }
}

impl std::fmt::Display for VariableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "variable#{}", self.0)
    }
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "event#{}", self.0)
    }
}

impl From<u64> for RegionId {
    fn from(v: u64) -> Self {
        RegionId(v)
    }
}

impl From<RegionId> for u64 {
    fn from(v: RegionId) -> Self {
        v.0
    }
}

// ---------------------------------------------------------------------------
// CapDKind
// ---------------------------------------------------------------------------

/// Capability derivation kind — used in `PreservesCapD` judgments to track
/// what capability property is being preserved across a transformation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum CapDKind {
    /// Read capability — allows observing the value.
    Read,
    /// Write capability — allows mutating the value.
    Write,
    /// Read-write capability — allows both observing and mutating.
    ReadWrite,
    /// Execute capability — allows running code at the target address.
    Execute,
}

impl std::fmt::Display for CapDKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapDKind::Read => write!(f, "read"),
            CapDKind::Write => write!(f, "write"),
            CapDKind::ReadWrite => write!(f, "readwrite"),
            CapDKind::Execute => write!(f, "execute"),
        }
    }
}

// ---------------------------------------------------------------------------
// Judgment
// ---------------------------------------------------------------------------

/// A structured logical judgment about a VUMA program's memory state.
///
/// Each variant carries typed fields that identify the entities involved,
/// enabling precise structural matching in inference rules rather than
/// error-prone string pattern matching.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Judgment {
    /// A region has been allocated and is available for use.
    Allocated {
        /// The identifier of the allocated region.
        region: RegionId,
    },

    /// A region is live (allocated and not yet freed).
    Live {
        /// The identifier of the live region.
        region: RegionId,
    },

    /// A region has been freed and its memory returned to the allocator.
    Freed {
        /// The identifier of the freed region.
        region: RegionId,
    },

    /// A region is dead (freed and no longer accessible).
    Dead {
        /// The identifier of the dead region.
        region: RegionId,
    },

    /// A resource is held under exclusive (mutable) access.
    Exclusive {
        /// The resource under exclusive access.
        resource: ResourceId,
    },

    /// A resource is shared among `count` readers.
    Shared {
        /// The shared resource.
        resource: ResourceId,
        /// The number of active shared holders.
        count: usize,
    },

    /// Two resources do not conflict (their exclusive access regions are
    /// non-overlapping).
    NoConflict {
        /// The first resource.
        resource_a: ResourceId,
        /// The second resource.
        resource_b: ResourceId,
    },

    /// A pointer is derived from another pointer within a specific region.
    Derived {
        /// The derived pointer.
        pointer: PointerId,
        /// The source pointer from which it was derived.
        from: PointerId,
        /// The region containing both pointers.
        region: RegionId,
    },

    /// An access at `(pointer + offset)` of `size` bytes is within bounds.
    InBounds {
        /// The base pointer.
        pointer: PointerId,
        /// The byte offset from the pointer.
        offset: i64,
        /// The size of the access in bytes.
        size: i64,
    },

    /// Bounds are preserved for an access: the access lies within the
    /// region's known bounds.
    BoundsPreserved {
        /// The base pointer.
        pointer: PointerId,
        /// The byte offset from the pointer.
        offset: i64,
        /// The size of the access in bytes.
        size: i64,
    },

    /// A variable has been initialized with a defined value.
    Initialized {
        /// The identifier of the initialized variable.
        variable: VariableId,
    },

    /// A transformation preserves a capability derivation property.
    PreservesCapD {
        /// The resource whose capability is preserved.
        resource: ResourceId,
        /// The capability kind before the transformation.
        from_capd: CapDKind,
        /// The capability kind after the transformation.
        to_capd: CapDKind,
    },

    /// A cast (type reinterpretation) is valid for the given resource and
    /// capability derivation.
    CastValid {
        /// The resource being cast.
        resource: ResourceId,
        /// The capability kind before the cast.
        from_capd: CapDKind,
        /// The capability kind after the cast.
        to_capd: CapDKind,
    },

    /// Event A is ordered before event B in the happens-before relation.
    TemporalOrder {
        /// The earlier event.
        event_a: EventId,
        /// The later event.
        event_b: EventId,
    },

    /// A general-purpose assumption expressed as a free-form description.
    /// Used when a structured judgment form is not available for the
    /// particular assumption being made.
    Assumption {
        /// Human-readable description of the assumption.
        description: String,
    },
}

impl Judgment {
    /// Produce a human-readable statement string from this judgment.
    ///
    /// This is used to populate `Fact.statement` for backward compatibility
    /// with the string-based system.
    pub fn to_statement(&self) -> String {
        match self {
            Judgment::Allocated { region } => format!("region {} is allocated", region),
            Judgment::Live { region } => format!("region {} is live", region),
            Judgment::Freed { region } => format!("region {} is freed", region),
            Judgment::Dead { region } => format!("region {} is dead", region),
            Judgment::Exclusive { resource } => format!("exclusive access to {}", resource),
            Judgment::Shared { resource, count } => {
                format!("shared access to {} (count={})", resource, count)
            }
            Judgment::NoConflict {
                resource_a,
                resource_b,
            } => format!("no conflict between {} and {}", resource_a, resource_b),
            Judgment::Derived {
                pointer,
                from,
                region,
            } => format!("{} derives from {} in region {}", pointer, from, region),
            Judgment::InBounds {
                pointer,
                offset,
                size,
            } => format!("inbounds {} offset={} size={}", pointer, offset, size),
            Judgment::BoundsPreserved {
                pointer,
                offset,
                size,
            } => format!("bounds preserved: inbounds {} offset={} size={}", pointer, offset, size),
            Judgment::Initialized { variable } => format!("variable {} is initialized", variable),
            Judgment::PreservesCapD {
                resource,
                from_capd,
                to_capd,
            } => format!(
                "preserves CapD for {}: {} -> {}",
                resource, from_capd, to_capd
            ),
            Judgment::CastValid {
                resource,
                from_capd,
                to_capd,
            } => format!(
                "cast is valid: preserves CapD for {}: {} -> {}",
                resource, from_capd, to_capd
            ),
            Judgment::TemporalOrder { event_a, event_b } => {
                format!("{} happens before {}", event_a, event_b)
            }
            Judgment::Assumption { description } => description.clone(),
        }
    }
}

impl std::fmt::Display for Judgment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_statement())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allocated_statement() {
        let j = Judgment::Allocated {
            region: RegionId(1),
        };
        assert_eq!(j.to_statement(), "region region#1 is allocated");
    }

    #[test]
    fn test_live_statement() {
        let j = Judgment::Live {
            region: RegionId(1),
        };
        assert_eq!(j.to_statement(), "region region#1 is live");
    }

    #[test]
    fn test_freed_statement() {
        let j = Judgment::Freed {
            region: RegionId(1),
        };
        assert_eq!(j.to_statement(), "region region#1 is freed");
    }

    #[test]
    fn test_dead_statement() {
        let j = Judgment::Dead {
            region: RegionId(1),
        };
        assert_eq!(j.to_statement(), "region region#1 is dead");
    }

    #[test]
    fn test_no_conflict_statement() {
        let j = Judgment::NoConflict {
            resource_a: ResourceId(1),
            resource_b: ResourceId(2),
        };
        assert_eq!(j.to_statement(), "no conflict between resource#1 and resource#2");
    }

    #[test]
    fn test_bounds_preserved_statement() {
        let j = Judgment::BoundsPreserved {
            pointer: PointerId(3),
            offset: 16,
            size: 4,
        };
        assert_eq!(j.to_statement(), "bounds preserved: inbounds pointer#3 offset=16 size=4");
    }

    #[test]
    fn test_cast_valid_statement() {
        let j = Judgment::CastValid {
            resource: ResourceId(2),
            from_capd: CapDKind::ReadWrite,
            to_capd: CapDKind::Read,
        };
        assert_eq!(
            j.to_statement(),
            "cast is valid: preserves CapD for resource#2: readwrite -> read"
        );
    }

    #[test]
    fn test_exclusive_statement() {
        let j = Judgment::Exclusive {
            resource: ResourceId(10),
        };
        assert_eq!(j.to_statement(), "exclusive access to resource#10");
    }

    #[test]
    fn test_shared_statement() {
        let j = Judgment::Shared {
            resource: ResourceId(5),
            count: 3,
        };
        assert_eq!(j.to_statement(), "shared access to resource#5 (count=3)");
    }

    #[test]
    fn test_derived_statement() {
        let j = Judgment::Derived {
            pointer: PointerId(1),
            from: PointerId(2),
            region: RegionId(1),
        };
        assert_eq!(
            j.to_statement(),
            "pointer#1 derives from pointer#2 in region region#1"
        );
    }

    #[test]
    fn test_inbounds_statement() {
        let j = Judgment::InBounds {
            pointer: PointerId(3),
            offset: 16,
            size: 4,
        };
        assert_eq!(j.to_statement(), "inbounds pointer#3 offset=16 size=4");
    }

    #[test]
    fn test_initialized_statement() {
        let j = Judgment::Initialized {
            variable: VariableId(7),
        };
        assert_eq!(j.to_statement(), "variable variable#7 is initialized");
    }

    #[test]
    fn test_preserves_capd_statement() {
        let j = Judgment::PreservesCapD {
            resource: ResourceId(2),
            from_capd: CapDKind::ReadWrite,
            to_capd: CapDKind::Read,
        };
        assert_eq!(
            j.to_statement(),
            "preserves CapD for resource#2: readwrite -> read"
        );
    }

    #[test]
    fn test_temporal_order_statement() {
        let j = Judgment::TemporalOrder {
            event_a: EventId(1),
            event_b: EventId(2),
        };
        assert_eq!(j.to_statement(), "event#1 happens before event#2");
    }

    #[test]
    fn test_capd_display() {
        assert_eq!(format!("{}", CapDKind::Read), "read");
        assert_eq!(format!("{}", CapDKind::Write), "write");
        assert_eq!(format!("{}", CapDKind::ReadWrite), "readwrite");
        assert_eq!(format!("{}", CapDKind::Execute), "execute");
    }

    #[test]
    fn test_judgment_display() {
        let j = Judgment::Allocated {
            region: RegionId(1),
        };
        assert_eq!(format!("{}", j), "region region#1 is allocated");
    }

    #[test]
    fn test_id_display_formats() {
        assert_eq!(format!("{}", RegionId(0)), "region#0");
        assert_eq!(format!("{}", ResourceId(1)), "resource#1");
        assert_eq!(format!("{}", PointerId(2)), "pointer#2");
        assert_eq!(format!("{}", VariableId(3)), "variable#3");
        assert_eq!(format!("{}", EventId(4)), "event#4");
    }

    #[test]
    fn test_region_id_from_u64() {
        let rid: RegionId = 42.into();
        assert_eq!(rid, RegionId(42));
        let val: u64 = rid.into();
        assert_eq!(val, 42);
    }
}
