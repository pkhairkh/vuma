//! # Primitive BD Definitions
//!
//! This module defines the Behavioral Description (BD) types and primitive
//! representation descriptors (RepDs) for the VUMA standard library.
//!
//! ## Core Concepts
//!
//! - **RepD** (Representation Descriptor): Describes the memory layout and
//!   structural representation of a type. Each primitive type has a canonical
//!   RepD that encodes its size, alignment, and structural properties.
//!
//! - **CapD** (Capability Descriptor): Describes the set of capabilities
//!   (operations) that a type supports. Capabilities form a lattice where
//!   more capable types subsume less capable ones.
//!
//! - **SyncEdge**: Describes synchronization edges for the Message Sequence
//!   Graph (MSG), enabling the VUMA runtime to verify concurrency safety.

use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// Capability Flags
// ---------------------------------------------------------------------------

/// Individual capability flags that compose a CapD.
///
/// Each flag represents a distinct operational capability that a type may
/// support. The VUMA verifier uses these flags to ensure that operations
/// are only performed on types that possess the required capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CapFlag {
    /// Type supports read operations.
    Read,
    /// Type supports write/mutation operations.
    Write,
    /// Type supports equality/comparison operations.
    Compare,
    /// Type supports hashing operations.
    Hash,
    /// Type supports serialization to/from byte streams.
    Serialize,
    /// Type supports iteration over its elements.
    Iterate,
    /// Type supports send/transfer across concurrency boundaries.
    Send,
    /// Type supports receive operations across concurrency boundaries.
    Receive,
    /// Type supports exclusive (mutable) access.
    Exclusive,
    /// Type supports shared (immutable) access.
    Shared,
}

impl fmt::Display for CapFlag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapFlag::Read => write!(f, "Read"),
            CapFlag::Write => write!(f, "Write"),
            CapFlag::Compare => write!(f, "Compare"),
            CapFlag::Hash => write!(f, "Hash"),
            CapFlag::Serialize => write!(f, "Serialize"),
            CapFlag::Iterate => write!(f, "Iterate"),
            CapFlag::Send => write!(f, "Send"),
            CapFlag::Receive => write!(f, "Receive"),
            CapFlag::Exclusive => write!(f, "Exclusive"),
            CapFlag::Shared => write!(f, "Shared"),
        }
    }
}

// ---------------------------------------------------------------------------
// Capability Descriptor (CapD)
// ---------------------------------------------------------------------------

/// Capability Descriptor — the set of capabilities a type supports.
///
/// CapD is the central mechanism by which the VUMA verifier ensures
/// capability-safe operations. Every type in the VUMA stdlib carries a CapD
/// that declares what operations are valid on values of that type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CapD {
    /// The set of capability flags this descriptor carries.
    pub flags: Vec<CapFlag>,
}

impl CapD {
    /// Create a new CapD from a vector of capability flags.
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn new(flags: Vec<CapFlag>) -> Self {
        Self { flags }
    }

    /// Create an empty CapD (no capabilities).
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn empty() -> Self {
        Self { flags: Vec::new() }
    }

    /// Check whether this CapD contains a specific capability flag.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn has(&self, flag: CapFlag) -> bool {
        self.flags.contains(&flag)
    }

    /// Check whether this CapD contains all of the specified capability flags.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn has_all(&self, flags: &[CapFlag]) -> bool {
        flags.iter().all(|f| self.flags.contains(f))
    }

    /// Merge this CapD with another, producing the union of capabilities.
    // VUMA-VERIFIED: pure combination, no side effects
    pub fn union(&self, other: &CapD) -> CapD {
        let mut flags = self.flags.clone();
        for f in &other.flags {
            if !flags.contains(f) {
                flags.push(*f);
            }
        }
        CapD { flags }
    }

    /// Intersect this CapD with another, producing the common capabilities.
    // VUMA-VERIFIED: pure combination, no side effects
    pub fn intersect(&self, other: &CapD) -> CapD {
        let flags: Vec<CapFlag> = self
            .flags
            .iter()
            .filter(|f| other.flags.contains(f))
            .copied()
            .collect();
        CapD { flags }
    }

    /// Returns true if this CapD is a sub-capability of (i.e., subsumed by) `other`.
    /// A CapD A is subsumed by B if every flag in A is also in B.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_subcap_of(&self, other: &CapD) -> bool {
        self.flags.iter().all(|f| other.flags.contains(f))
    }
}

impl fmt::Display for CapD {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CapD {{ ")?;
        let strs: Vec<String> = self.flags.iter().map(|f| f.to_string()).collect();
        write!(f, "{}", strs.join(", "))?;
        write!(f, " }}")
    }
}

/// The default CapD for numeric types: Read, Write, Compare, Hash, Serialize.
// VUMA-VERIFIED: well-known capability set for numeric primitives
pub fn numeric_capd() -> CapD {
    CapD::new(vec![
        CapFlag::Read,
        CapFlag::Write,
        CapFlag::Compare,
        CapFlag::Hash,
        CapFlag::Serialize,
    ])
}

/// The default CapD for string types: Read, Write, Iterate, Compare, Hash, Serialize, Send.
// VUMA-VERIFIED: well-known capability set for string types
pub fn string_capd() -> CapD {
    CapD::new(vec![
        CapFlag::Read,
        CapFlag::Write,
        CapFlag::Iterate,
        CapFlag::Compare,
        CapFlag::Hash,
        CapFlag::Serialize,
        CapFlag::Send,
    ])
}

// ---------------------------------------------------------------------------
// Representation Descriptor (RepD)
// ---------------------------------------------------------------------------

/// Representation Descriptor — describes the memory layout of a type.
///
/// RepD encodes the structural properties of a type's in-memory
/// representation, including its name, size in bytes, alignment requirement,
/// and the CapD that declares its supported operations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepD {
    /// The name of this type (e.g., "uint8", "float64", "ptr").
    pub name: String,
    /// Size of the type in bytes.
    pub size: u64,
    /// Alignment requirement in bytes.
    pub align: u64,
    /// Capability descriptor for this type.
    pub capd: CapD,
}

impl RepD {
    /// Create a new RepD with the given name, size, alignment, and capabilities.
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn new(name: impl Into<String>, size: u64, align: u64, capd: CapD) -> Self {
        Self {
            name: name.into(),
            size,
            align,
            capd,
        }
    }

    /// Create a pointer RepD that points to the given pointee RepD.
    /// A pointer has size 8 (64-bit) and alignment 8, with the same
    /// capabilities as the pointee plus Read and Write for dereference.
    // VUMA-VERIFIED: pointer construction preserves pointee capabilities
    pub fn ptr_to(pointee: &RepD) -> Self {
        let ptr_capd = pointee.capd.union(&CapD::new(vec![
            CapFlag::Read,
            CapFlag::Write,
        ]));
        Self {
            name: format!("ptr<{}>", pointee.name),
            size: 8,
            align: 8,
            capd: ptr_capd,
        }
    }

    /// Returns true if this RepD's CapD subsumes the given CapD.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn supports(&self, required: &CapD) -> bool {
        required.is_subcap_of(&self.capd)
    }
}

impl fmt::Display for RepD {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RepD {{ name: {}, size: {}, align: {}, {} }}",
            self.name, self.size, self.align, self.capd
        )
    }
}

// ---------------------------------------------------------------------------
// Synchronization Edge (SyncEdge)
// ---------------------------------------------------------------------------

/// Synchronization Edge for the Message Sequence Graph (MSG).
///
/// SyncEdge describes a happens-before relationship between two operations,
/// enabling the VUMA runtime to verify that concurrent accesses are properly
/// ordered and data-race-free.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SyncEdge {
    /// The source operation identifier.
    pub from: String,
    /// The destination operation identifier.
    pub to: String,
    /// The kind of synchronization relationship.
    pub kind: SyncEdgeKind,
}

/// The kind of synchronization relationship between two operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SyncEdgeKind {
    /// Sequential happens-before (program order).
    Seq,
    /// Lock acquisition/release ordering.
    LockOrder,
    /// Channel send/receive ordering.
    ChannelOrder,
    /// Memory fence / barrier ordering.
    Fence,
    /// Atomic operation ordering.
    Atomic,
}

impl SyncEdge {
    /// Create a new SyncEdge.
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn new(from: impl Into<String>, to: impl Into<String>, kind: SyncEdgeKind) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            kind,
        }
    }
}

// ---------------------------------------------------------------------------
// Primitive RepD Constructors
// ---------------------------------------------------------------------------

/// Returns the RepD for `uint8` (unsigned 8-bit integer).
// VUMA-VERIFIED: canonical primitive representation
pub fn uint8_repd() -> RepD {
    RepD::new("uint8", 1, 1, numeric_capd())
}

/// Returns the RepD for `uint16` (unsigned 16-bit integer).
// VUMA-VERIFIED: canonical primitive representation
pub fn uint16_repd() -> RepD {
    RepD::new("uint16", 2, 2, numeric_capd())
}

/// Returns the RepD for `uint32` (unsigned 32-bit integer).
// VUMA-VERIFIED: canonical primitive representation
pub fn uint32_repd() -> RepD {
    RepD::new("uint32", 4, 4, numeric_capd())
}

/// Returns the RepD for `uint64` (unsigned 64-bit integer).
// VUMA-VERIFIED: canonical primitive representation
pub fn uint64_repd() -> RepD {
    RepD::new("uint64", 8, 8, numeric_capd())
}

/// Returns the RepD for `int8` (signed 8-bit integer).
// VUMA-VERIFIED: canonical primitive representation
pub fn int8_repd() -> RepD {
    RepD::new("int8", 1, 1, numeric_capd())
}

/// Returns the RepD for `int16` (signed 16-bit integer).
// VUMA-VERIFIED: canonical primitive representation
pub fn int16_repd() -> RepD {
    RepD::new("int16", 2, 2, numeric_capd())
}

/// Returns the RepD for `int32` (signed 32-bit integer).
// VUMA-VERIFIED: canonical primitive representation
pub fn int32_repd() -> RepD {
    RepD::new("int32", 4, 4, numeric_capd())
}

/// Returns the RepD for `int64` (signed 64-bit integer).
// VUMA-VERIFIED: canonical primitive representation
pub fn int64_repd() -> RepD {
    RepD::new("int64", 8, 8, numeric_capd())
}

/// Returns the RepD for `float32` (32-bit IEEE 754 floating-point).
// VUMA-VERIFIED: canonical primitive representation
pub fn float32_repd() -> RepD {
    RepD::new("float32", 4, 4, numeric_capd())
}

/// Returns the RepD for `float64` (64-bit IEEE 754 floating-point).
// VUMA-VERIFIED: canonical primitive representation
pub fn float64_repd() -> RepD {
    RepD::new("float64", 8, 8, numeric_capd())
}

/// Returns the RepD for `bool` (1-byte boolean).
// VUMA-VERIFIED: canonical primitive representation
pub fn bool_repd() -> RepD {
    RepD::new("bool", 1, 1, CapD::new(vec![
        CapFlag::Read,
        CapFlag::Write,
        CapFlag::Compare,
        CapFlag::Hash,
        CapFlag::Serialize,
    ]))
}

/// Returns the RepD for `byte` (raw 8-bit byte, no Compare/Hash by default).
// VUMA-VERIFIED: canonical primitive representation
pub fn byte_repd() -> RepD {
    RepD::new("byte", 1, 1, CapD::new(vec![
        CapFlag::Read,
        CapFlag::Write,
        CapFlag::Serialize,
    ]))
}

/// Returns the RepD for a pointer to the given pointee type.
///
/// The pointer inherits the pointee's capabilities and adds Read and Write
/// to support dereference operations.
// VUMA-VERIFIED: pointer RepD is well-formed and capability-preserving
pub fn ptr_repd(pointee: RepD) -> RepD {
    RepD::ptr_to(&pointee)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_numeric_capd_has_expected_flags() {
        let capd = numeric_capd();
        assert!(capd.has(CapFlag::Read));
        assert!(capd.has(CapFlag::Write));
        assert!(capd.has(CapFlag::Compare));
        assert!(capd.has(CapFlag::Hash));
        assert!(capd.has(CapFlag::Serialize));
        assert!(!capd.has(CapFlag::Iterate));
        assert!(!capd.has(CapFlag::Send));
    }

    #[test]
    fn test_string_capd_has_expected_flags() {
        let capd = string_capd();
        assert!(capd.has(CapFlag::Read));
        assert!(capd.has(CapFlag::Write));
        assert!(capd.has(CapFlag::Iterate));
        assert!(capd.has(CapFlag::Compare));
        assert!(capd.has(CapFlag::Hash));
        assert!(capd.has(CapFlag::Serialize));
        assert!(capd.has(CapFlag::Send));
    }

    #[test]
    fn test_uint8_repd_properties() {
        let repd = uint8_repd();
        assert_eq!(repd.name, "uint8");
        assert_eq!(repd.size, 1);
        assert_eq!(repd.align, 1);
        assert!(repd.supports(&numeric_capd()));
    }

    #[test]
    fn test_float64_repd_properties() {
        let repd = float64_repd();
        assert_eq!(repd.name, "float64");
        assert_eq!(repd.size, 8);
        assert_eq!(repd.align, 8);
    }

    #[test]
    fn test_ptr_repd_inherits_pointee_caps() {
        let pointee = uint32_repd();
        let ptr = ptr_repd(pointee);
        assert_eq!(ptr.name, "ptr<uint32>");
        assert_eq!(ptr.size, 8);
        assert_eq!(ptr.align, 8);
        assert!(ptr.capd.has(CapFlag::Read));
        assert!(ptr.capd.has(CapFlag::Write));
    }

    #[test]
    fn test_capd_union() {
        let a = CapD::new(vec![CapFlag::Read]);
        let b = CapD::new(vec![CapFlag::Write]);
        let union = a.union(&b);
        assert!(union.has(CapFlag::Read));
        assert!(union.has(CapFlag::Write));
    }

    #[test]
    fn test_capd_intersect() {
        let a = CapD::new(vec![CapFlag::Read, CapFlag::Write]);
        let b = CapD::new(vec![CapFlag::Write, CapFlag::Hash]);
        let inter = a.intersect(&b);
        assert!(inter.has(CapFlag::Write));
        assert!(!inter.has(CapFlag::Read));
        assert!(!inter.has(CapFlag::Hash));
    }

    #[test]
    fn test_capd_subcap() {
        let a = CapD::new(vec![CapFlag::Read]);
        let b = CapD::new(vec![CapFlag::Read, CapFlag::Write]);
        assert!(a.is_subcap_of(&b));
        assert!(!b.is_subcap_of(&a));
    }
}
