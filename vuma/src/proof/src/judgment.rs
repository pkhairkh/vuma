//! # Structured Judgments
//!
//! Typed judgment forms that replace string-matching in proof rule application.
//! Each variant represents a specific logical statement about the program's
//! memory state, enabling structural matching instead of fragile string
//! comparison.

use serde::{Deserialize, Serialize};

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
        /// The name/identifier of the allocated region.
        region: String,
    },

    /// A region is live (allocated and not yet freed).
    Live {
        /// The name/identifier of the live region.
        region: String,
    },

    /// A region has been freed and its memory returned to the allocator.
    Freed {
        /// The name/identifier of the freed region.
        region: String,
    },

    /// A resource is held under exclusive (mutable) access.
    Exclusive {
        /// The resource under exclusive access.
        resource: String,
    },

    /// A resource is shared among `count` readers.
    Shared {
        /// The shared resource.
        resource: String,
        /// The number of active shared holders.
        count: usize,
    },

    /// A pointer is derived from another pointer within a specific region.
    Derived {
        /// The derived pointer.
        pointer: String,
        /// The source pointer from which it was derived.
        from: String,
        /// The region containing both pointers.
        region: String,
    },

    /// An access at `(pointer + offset)` of `size` bytes is within bounds.
    InBounds {
        /// The base pointer.
        pointer: String,
        /// The byte offset from the pointer.
        offset: i64,
        /// The size of the access in bytes.
        size: i64,
    },

    /// A variable has been initialized with a defined value.
    Initialized {
        /// The name of the initialized variable.
        variable: String,
    },

    /// A transformation preserves a capability derivation property.
    PreservesCapD {
        /// The resource whose capability is preserved.
        resource: String,
        /// The capability kind before the transformation.
        from_capd: CapDKind,
        /// The capability kind after the transformation.
        to_capd: CapDKind,
    },

    /// Event A is ordered before event B in the happens-before relation.
    TemporalOrder {
        /// The earlier event.
        event_a: String,
        /// The later event.
        event_b: String,
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
            Judgment::Exclusive { resource } => format!("exclusive access to {}", resource),
            Judgment::Shared { resource, count } => {
                format!("shared access to {} (count={})", resource, count)
            }
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
            Judgment::Initialized { variable } => format!("variable {} is initialized", variable),
            Judgment::PreservesCapD {
                resource,
                from_capd,
                to_capd,
            } => format!(
                "preserves CapD for {}: {} -> {}",
                resource, from_capd, to_capd
            ),
            Judgment::TemporalOrder { event_a, event_b } => {
                format!("{} happens before {}", event_a, event_b)
            }
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
            region: "r1".into(),
        };
        assert_eq!(j.to_statement(), "region r1 is allocated");
    }

    #[test]
    fn test_live_statement() {
        let j = Judgment::Live {
            region: "r1".into(),
        };
        assert_eq!(j.to_statement(), "region r1 is live");
    }

    #[test]
    fn test_freed_statement() {
        let j = Judgment::Freed {
            region: "r1".into(),
        };
        assert_eq!(j.to_statement(), "region r1 is freed");
    }

    #[test]
    fn test_exclusive_statement() {
        let j = Judgment::Exclusive {
            resource: "lock_L".into(),
        };
        assert_eq!(j.to_statement(), "exclusive access to lock_L");
    }

    #[test]
    fn test_shared_statement() {
        let j = Judgment::Shared {
            resource: "buf".into(),
            count: 3,
        };
        assert_eq!(j.to_statement(), "shared access to buf (count=3)");
    }

    #[test]
    fn test_derived_statement() {
        let j = Judgment::Derived {
            pointer: "p".into(),
            from: "q".into(),
            region: "r1".into(),
        };
        assert_eq!(j.to_statement(), "p derives from q in region r1");
    }

    #[test]
    fn test_inbounds_statement() {
        let j = Judgment::InBounds {
            pointer: "p".into(),
            offset: 16,
            size: 4,
        };
        assert_eq!(j.to_statement(), "inbounds p offset=16 size=4");
    }

    #[test]
    fn test_initialized_statement() {
        let j = Judgment::Initialized {
            variable: "x".into(),
        };
        assert_eq!(j.to_statement(), "variable x is initialized");
    }

    #[test]
    fn test_preserves_capd_statement() {
        let j = Judgment::PreservesCapD {
            resource: "mem".into(),
            from_capd: CapDKind::ReadWrite,
            to_capd: CapDKind::Read,
        };
        assert_eq!(
            j.to_statement(),
            "preserves CapD for mem: readwrite -> read"
        );
    }

    #[test]
    fn test_temporal_order_statement() {
        let j = Judgment::TemporalOrder {
            event_a: "e1".into(),
            event_b: "e2".into(),
        };
        assert_eq!(j.to_statement(), "e1 happens before e2");
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
            region: "r1".into(),
        };
        assert_eq!(format!("{}", j), "region r1 is allocated");
    }
}
