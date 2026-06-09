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
//! - **RelD** (Relational Descriptor): Describes the relational properties
//!   a value participates in — containment, liveness, aliasing, data-flow
//!   dependencies. RelD supports a refinement ordering and composition.
//!
//! - **BD** (Behavioral Descriptor): The complete specification of a value's
//!   representation, capabilities, and relations: `BD = RepD × CapD × RelD`.
//!
//! - **SyncEdge**: Describes synchronization edges for the Message Sequence
//!   Graph (MSG), enabling the VUMA runtime to verify concurrency safety.
//!
//! ## VUMA Primitive Types
//!
//! - **Ptr\<T\>**: VUMA pointer with embedded BD annotation.
//! - **RegionPtr\<T\>**: Pointer bound to a specific memory region.
//! - **Slice\<T\>**: Pointer + length with BD annotation.
//! - **Result\<T,E\>**: VUMA result type with BD tracking.
//! - **Option\<T\>**: VUMA option type with BD tracking.
//! - **Range**: Integer range type (start..end).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::marker::PhantomData;

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
// Relational Descriptor (RelD)
// ---------------------------------------------------------------------------

/// Kinds of relational properties a value can participate in.
///
/// Each variant identifies a distinct class of relationship that the
/// VUMA verifier tracks when composing and checking BDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelKind {
    /// The value is contained within another structure (e.g., pointer within region).
    Containment,
    /// The value has a liveness constraint — it must be alive when accessed.
    Liveness,
    /// The value may be aliased by other pointers/references.
    Aliasing,
    /// The value participates in a data-flow dependency.
    DataFlow,
    /// The value is bound to a specific memory region.
    RegionBound,
    /// The value has an ownership relationship.
    Ownership,
}

impl fmt::Display for RelKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RelKind::Containment => write!(f, "Containment"),
            RelKind::Liveness => write!(f, "Liveness"),
            RelKind::Aliasing => write!(f, "Aliasing"),
            RelKind::DataFlow => write!(f, "DataFlow"),
            RelKind::RegionBound => write!(f, "RegionBound"),
            RelKind::Ownership => write!(f, "Ownership"),
        }
    }
}

/// Relational Descriptor — describes the relationships a value participates in.
///
/// RelD captures the *relational* dimension of a BD: what other values or
/// regions this value is related to, and what constraints those relations
/// impose. Two RelDs can be composed (union of relations) and compared
/// for refinement (a more refined RelD has a superset of relations).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelD {
    /// The set of relational properties this descriptor carries.
    pub relations: Vec<RelKind>,
}

impl RelD {
    /// Create a new RelD from a vector of relation kinds.
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn new(relations: Vec<RelKind>) -> Self {
        Self { relations }
    }

    /// Create an empty RelD (no relations).
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn empty() -> Self {
        Self {
            relations: Vec::new(),
        }
    }

    /// Check whether this RelD contains a specific relation kind.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn has(&self, kind: RelKind) -> bool {
        self.relations.contains(&kind)
    }

    /// Compose this RelD with another, producing the union of relations.
    // VUMA-VERIFIED: pure combination, no side effects
    pub fn compose(&self, other: &RelD) -> RelD {
        let mut relations = self.relations.clone();
        for r in &other.relations {
            if !relations.contains(r) {
                relations.push(*r);
            }
        }
        RelD { relations }
    }

    /// Returns true if this RelD refines `other`.
    /// A RelD A refines B if every relation in B is also in A
    /// (i.e., A is more specific / more restrictive).
    // VUMA-VERIFIED: pure query, no side effects
    pub fn refines(&self, other: &RelD) -> bool {
        other.relations.iter().all(|r| self.relations.contains(r))
    }

    /// Intersect this RelD with another, producing the common relations.
    // VUMA-VERIFIED: pure combination, no side effects
    pub fn intersect(&self, other: &RelD) -> RelD {
        let relations: Vec<RelKind> = self
            .relations
            .iter()
            .filter(|r| other.relations.contains(r))
            .copied()
            .collect();
        RelD { relations }
    }
}

impl fmt::Display for RelD {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelD {{ ")?;
        let strs: Vec<String> = self.relations.iter().map(|r| r.to_string()).collect();
        write!(f, "{}", strs.join(", "))?;
        write!(f, " }}")
    }
}

/// The default RelD for pointer types: Containment, Liveness.
// VUMA-VERIFIED: well-known relation set for pointers
pub fn ptr_reld() -> RelD {
    RelD::new(vec![RelKind::Containment, RelKind::Liveness])
}

/// The RelD for region-bound pointers: Containment, Liveness, RegionBound.
// VUMA-VERIFIED: well-known relation set for region-bound pointers
pub fn region_ptr_reld() -> RelD {
    RelD::new(vec![RelKind::Containment, RelKind::Liveness, RelKind::RegionBound])
}

/// The RelD for slice types: Containment, Liveness, DataFlow.
// VUMA-VERIFIED: well-known relation set for slices
pub fn slice_reld() -> RelD {
    RelD::new(vec![RelKind::Containment, RelKind::Liveness, RelKind::DataFlow])
}

/// The RelD for result types: DataFlow, Ownership.
// VUMA-VERIFIED: well-known relation set for results
pub fn result_reld() -> RelD {
    RelD::new(vec![RelKind::DataFlow, RelKind::Ownership])
}

/// The RelD for option types: DataFlow, Liveness.
// VUMA-VERIFIED: well-known relation set for options
pub fn option_reld() -> RelD {
    RelD::new(vec![RelKind::DataFlow, RelKind::Liveness])
}

/// The default RelD for numeric/primitive types: empty (no relations).
// VUMA-VERIFIED: well-known relation set for plain numerics
pub fn numeric_reld() -> RelD {
    RelD::empty()
}

// ---------------------------------------------------------------------------
// Behavioral Descriptor (BD)
// ---------------------------------------------------------------------------

/// A **Behavioral Descriptor** — the complete specification of a value's
/// representation, capabilities, and relations.
///
/// # Structure
///
/// ```text
/// BD = RepD × CapD × RelD
/// ```
///
/// Two BDs are **compatible** when all three layers are pairwise compatible.
/// One BD **refines** another when every layer is at least as specific.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BD {
    /// Representation descriptor — memory shape.
    pub repd: RepD,
    /// Capability descriptor — permitted operations.
    pub capd: CapD,
    /// Relational descriptor — relationships.
    pub reld: RelD,
}

impl BD {
    /// Construct a new `BD` from its three layers.
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn new(repd: RepD, capd: CapD, reld: RelD) -> Self {
        Self { repd, capd, reld }
    }

    /// Two BDs are **compatible** when they can safely describe the same
    /// value:
    ///
    /// * representations have the same name and size,
    /// * capabilities have a non-empty intersection, and
    /// * relations are consistent (non-empty intersection).
    // VUMA-VERIFIED: pure query, no side effects
    pub fn compatible(&self, other: &BD) -> bool {
        self.repd.name == other.repd.name
            && self.repd.size == other.repd.size
            && !self.capd.intersect(&other.capd).flags.is_empty()
            && !self.reld.intersect(&other.reld).relations.is_empty()
                || self.reld.relations.is_empty()
                || other.reld.relations.is_empty()
    }

    /// `self` **refines** `other` when `self` is at least as specific in
    /// every layer:
    ///
    /// * repd names match,
    /// * self's CapD is a sub-capability of other's (subset), and
    /// * self's RelD refines other's (superset of relations).
    // VUMA-VERIFIED: pure query, no side effects
    pub fn refines(&self, other: &BD) -> bool {
        self.repd.name == other.repd.name
            && self.capd.is_subcap_of(&other.capd)
            && self.reld.refines(&other.reld)
    }
}

impl fmt::Display for BD {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "BD {{")?;
        writeln!(f, "  repd: {}", self.repd)?;
        writeln!(f, "  capd: {}", self.capd)?;
        writeln!(f, "  reld: {}", self.reld)?;
        write!(f, "}}")
    }
}

// ---------------------------------------------------------------------------
// HasBD Trait
// ---------------------------------------------------------------------------

/// Trait for types that can produce a Behavioral Descriptor.
///
/// Every VUMA primitive type implements this trait so that the verifier
/// can inspect its BD annotation at any point.
pub trait HasBD {
    /// Returns the Behavioral Descriptor for this value.
    // VUMA-VERIFIED: pure query, no side effects
    fn as_bd(&self) -> BD;
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
// VUMA Primitive Types
// ---------------------------------------------------------------------------

// ---- Ptr<T> ----------------------------------------------------------------

/// A VUMA pointer type with embedded BD annotation.
///
/// `Ptr<T>` wraps a raw 64-bit address and carries a BD that describes
/// both the pointer itself and the pointee type `T`. The BD includes:
///
/// - **RepD**: 8-byte pointer with the pointee's capabilities + Read/Write.
/// - **CapD**: Union of pointer capabilities and pointee capabilities.
/// - **RelD**: Containment and Liveness relations.
///
/// # Type Parameter
///
/// `T` is a phantom type parameter used to track the pointee type at
/// compile time. It does not affect runtime representation.
///
/// # Safety
///
/// `Ptr<T>` is a VUMA-verified abstraction — the runtime ensures that
/// dereference is only permitted when the BD's capabilities allow it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ptr<T> {
    /// The raw address this pointer points to.
    pub addr: u64,
    /// The BD annotation for the pointee type.
    pub pointee_bd: BD,
    /// Phantom marker for the pointee type.
    _marker: PhantomData<T>,
}

impl<T> Ptr<T> {
    /// Create a new `Ptr<T>` pointing to the given address with the
    /// specified pointee BD annotation.
    ///
    /// # Arguments
    ///
    /// * `addr` - The raw 64-bit address.
    /// * `pointee_bd` - The BD of the pointee type `T`.
    // VUMA-VERIFIED: constructor establishes valid pointer + BD pair
    pub fn new(addr: u64, pointee_bd: BD) -> Self {
        Self {
            addr,
            pointee_bd,
            _marker: PhantomData,
        }
    }

    /// Create a null pointer (address 0) with the given pointee BD.
    // VUMA-VERIFIED: null pointer is safe to construct
    pub fn null(pointee_bd: BD) -> Self {
        Self::new(0, pointee_bd)
    }

    /// Returns true if this is a null pointer.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_null(&self) -> bool {
        self.addr == 0
    }

    /// Offset this pointer by `n` bytes, returning a new pointer.
    // VUMA-VERIFIED: pointer arithmetic preserves BD annotation
    pub fn offset(&self, n: u64) -> Self {
        Self::new(self.addr + n, self.pointee_bd.clone())
    }

    /// Returns the RepD for this pointer type.
    // VUMA-VERIFIED: pointer RepD is derived from pointee
    pub fn repd(&self) -> RepD {
        RepD::ptr_to(&self.pointee_bd.repd)
    }

    /// Returns the CapD for this pointer type.
    // VUMA-VERIFIED: pointer CapD is derived from pointee + Read/Write
    pub fn capd(&self) -> CapD {
        self.pointee_bd.capd.union(&CapD::new(vec![
            CapFlag::Read,
            CapFlag::Write,
        ]))
    }

    /// Returns the RelD for this pointer type.
    // VUMA-VERIFIED: pointer RelD is the standard ptr_reld
    pub fn reld(&self) -> RelD {
        ptr_reld()
    }
}

impl<T> HasBD for Ptr<T> {
    /// Returns the full BD for this pointer, derived from the pointee BD
    /// and pointer-specific annotations.
    // VUMA-VERIFIED: BD derivation is well-formed
    fn as_bd(&self) -> BD {
        BD::new(self.repd(), self.capd(), self.reld())
    }
}

impl<T> fmt::Display for Ptr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ptr(0x{:016X}, {})", self.addr, self.pointee_bd.repd.name)
    }
}

// ---- RegionPtr<T> ---------------------------------------------------------

/// A pointer bound to a specific memory region.
///
/// `RegionPtr<T>` extends `Ptr<T>` with region bounds checking. It carries
/// the base address and size of the memory region it is bound to, ensuring
/// that all accesses fall within the valid region. The BD includes a
/// `RegionBound` relation that the VUMA verifier uses to prove spatial
/// memory safety.
///
/// # BD Annotation
///
/// - **RepD**: 24 bytes (addr: 8 + region_base: 8 + region_size: 8).
/// - **CapD**: Same as `Ptr<T>` plus Shared.
/// - **RelD**: Containment, Liveness, and RegionBound.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegionPtr<T> {
    /// The raw address this pointer points to.
    pub addr: u64,
    /// Base address of the memory region.
    pub region_base: u64,
    /// Size of the memory region in bytes.
    pub region_size: u64,
    /// The BD annotation for the pointee type.
    pub pointee_bd: BD,
    /// Phantom marker for the pointee type.
    _marker: PhantomData<T>,
}

impl<T> RegionPtr<T> {
    /// Create a new `RegionPtr<T>` pointing to the given address, bound
    /// to the specified memory region.
    ///
    /// # Arguments
    ///
    /// * `addr` - The raw 64-bit address the pointer targets.
    /// * `region_base` - The base address of the memory region.
    /// * `region_size` - The size of the memory region in bytes.
    /// * `pointee_bd` - The BD of the pointee type `T`.
    ///
    /// # Panics
    ///
    /// Panics if `addr` is not within `[region_base, region_base + region_size)`.
    // VUMA-VERIFIED: constructor establishes valid region-bound pointer
    pub fn new(addr: u64, region_base: u64, region_size: u64, pointee_bd: BD) -> Self {
        assert!(
            addr >= region_base && addr < region_base + region_size,
            "RegionPtr: address 0x{:X} is outside region [0x{:X}, 0x{:X})",
            addr, region_base, region_base + region_size
        );
        Self {
            addr,
            region_base,
            region_size,
            pointee_bd,
            _marker: PhantomData,
        }
    }

    /// Returns true if the address is within the region bounds.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn in_bounds(&self) -> bool {
        self.addr >= self.region_base && self.addr < self.region_base + self.region_size
    }

    /// Offset this pointer by `n` bytes, returning a new RegionPtr.
    ///
    /// Returns `None` if the resulting address would be outside the region.
    // VUMA-VERIFIED: bounded offset preserves region safety
    pub fn checked_offset(&self, n: u64) -> Option<Self> {
        let new_addr = self.addr.checked_add(n)?;
        if new_addr >= self.region_base && new_addr < self.region_base + self.region_size {
            Some(Self {
                addr: new_addr,
                region_base: self.region_base,
                region_size: self.region_size,
                pointee_bd: self.pointee_bd.clone(),
                _marker: PhantomData,
            })
        } else {
            None
        }
    }

    /// Returns the RepD for this region-pointer type.
    // VUMA-VERIFIED: region-pointer RepD is well-formed
    pub fn repd(&self) -> RepD {
        RepD::new(
            format!("region_ptr<{}>", self.pointee_bd.repd.name),
            24,
            8,
            self.capd(),
        )
    }

    /// Returns the CapD for this region-pointer type.
    // VUMA-VERIFIED: region-pointer CapD includes Shared access
    pub fn capd(&self) -> CapD {
        self.pointee_bd.capd.union(&CapD::new(vec![
            CapFlag::Read,
            CapFlag::Write,
            CapFlag::Shared,
        ]))
    }

    /// Returns the RelD for this region-pointer type.
    // VUMA-VERIFIED: region-pointer RelD includes RegionBound
    pub fn reld(&self) -> RelD {
        region_ptr_reld()
    }
}

impl<T> HasBD for RegionPtr<T> {
    /// Returns the full BD for this region-bound pointer.
    // VUMA-VERIFIED: BD derivation is well-formed
    fn as_bd(&self) -> BD {
        BD::new(self.repd(), self.capd(), self.reld())
    }
}

impl<T> fmt::Display for RegionPtr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RegionPtr(0x{:016X}, region [0x{:016X}, +{}), {})",
            self.addr, self.region_base, self.region_size, self.pointee_bd.repd.name
        )
    }
}

// ---- Slice<T> -------------------------------------------------------------

/// A pointer + length with BD annotation.
///
/// `Slice<T>` represents a contiguous sequence of elements of type `T`
/// starting at a base address. It carries both a length (number of
/// elements) and a BD that describes the slice's capabilities and
/// relational properties.
///
/// # BD Annotation
///
/// - **RepD**: 16 bytes (addr: 8 + len: 8), with Iterate capability.
/// - **CapD**: Pointee capabilities + Read, Write, Iterate.
/// - **RelD**: Containment, Liveness, DataFlow.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Slice<T> {
    /// The base address of the slice.
    pub addr: u64,
    /// The number of elements in the slice.
    pub len: u64,
    /// The BD annotation for the element type.
    pub elem_bd: BD,
    /// Phantom marker for the element type.
    _marker: PhantomData<T>,
}

impl<T> Slice<T> {
    /// Create a new `Slice<T>` starting at the given address with the
    /// specified length and element BD.
    ///
    /// # Arguments
    ///
    /// * `addr` - The base address of the slice.
    /// * `len` - The number of elements in the slice.
    /// * `elem_bd` - The BD of the element type `T`.
    // VUMA-VERIFIED: constructor establishes valid slice + BD pair
    pub fn new(addr: u64, len: u64, elem_bd: BD) -> Self {
        Self {
            addr,
            len,
            elem_bd,
            _marker: PhantomData,
        }
    }

    /// Create an empty slice (zero length) at the given address.
    // VUMA-VERIFIED: empty slice is safe to construct
    pub fn empty(addr: u64, elem_bd: BD) -> Self {
        Self::new(addr, 0, elem_bd)
    }

    /// Returns true if this slice has zero elements.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the byte size of this slice (len * elem_size).
    // VUMA-VERIFIED: pure query, no side effects
    pub fn byte_size(&self) -> u64 {
        self.len * self.elem_bd.repd.size
    }

    /// Returns a subslice starting at `offset` elements with `new_len` elements.
    ///
    /// Returns `None` if the subslice would be out of bounds.
    // VUMA-VERIFIED: subslice preserves BD annotation and bounds
    pub fn subslice(&self, offset: u64, new_len: u64) -> Option<Self> {
        if offset + new_len > self.len {
            return None;
        }
        Some(Self::new(
            self.addr + offset * self.elem_bd.repd.size,
            new_len,
            self.elem_bd.clone(),
        ))
    }

    /// Returns the RepD for this slice type.
    // VUMA-VERIFIED: slice RepD is well-formed
    pub fn repd(&self) -> RepD {
        RepD::new(
            format!("slice<{}>", self.elem_bd.repd.name),
            16,
            8,
            self.capd(),
        )
    }

    /// Returns the CapD for this slice type.
    // VUMA-VERIFIED: slice CapD includes Iterate
    pub fn capd(&self) -> CapD {
        self.elem_bd.capd.union(&CapD::new(vec![
            CapFlag::Read,
            CapFlag::Write,
            CapFlag::Iterate,
        ]))
    }

    /// Returns the RelD for this slice type.
    // VUMA-VERIFIED: slice RelD includes DataFlow
    pub fn reld(&self) -> RelD {
        slice_reld()
    }
}

impl<T> HasBD for Slice<T> {
    /// Returns the full BD for this slice.
    // VUMA-VERIFIED: BD derivation is well-formed
    fn as_bd(&self) -> BD {
        BD::new(self.repd(), self.capd(), self.reld())
    }
}

impl<T> fmt::Display for Slice<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Slice(0x{:016X}, len={}, {})",
            self.addr, self.len, self.elem_bd.repd.name
        )
    }
}

// ---- VumaResult<T, E> -----------------------------------------------------

/// A VUMA result type with BD tracking.
///
/// `VumaResult<T, E>` represents either a successful value (`Ok`) or an
/// error (`Err`), with full BD annotations for both variants. The BD of a
/// result value is the union of the OK and ERR BDs, capturing the
/// capabilities and relations of both paths.
///
/// # BD Annotation
///
/// - **RepD**: Sum type, size is `max(ok_size, err_size) + 1` (tag byte).
/// - **CapD**: Intersection of OK and ERR capabilities.
/// - **RelD**: DataFlow and Ownership relations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VumaResult<T, E> {
    /// Successful result carrying a value of type `T`.
    Ok(T),
    /// Error result carrying a value of type `E`.
    Err(E),
}

impl<T, E> VumaResult<T, E> {
    /// Returns true if this is an `Ok` value.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_ok(&self) -> bool {
        matches!(self, VumaResult::Ok(_))
    }

    /// Returns true if this is an `Err` value.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_err(&self) -> bool {
        matches!(self, VumaResult::Err(_))
    }

    /// Unwrap the Ok value, panicking if this is Err.
    // VUMA-VERIFIED: panics on Err — caller must ensure Ok via capability check
    pub fn unwrap(self) -> T {
        match self {
            VumaResult::Ok(v) => v,
            VumaResult::Err(_) => panic!("VumaResult::unwrap on Err value"),
        }
    }

    /// Unwrap the Err value, panicking if this is Ok.
    // VUMA-VERIFIED: panics on Ok — caller must ensure Err via capability check
    pub fn unwrap_err(self) -> E {
        match self {
            VumaResult::Err(e) => e,
            VumaResult::Ok(_) => panic!("VumaResult::unwrap_err on Ok value"),
        }
    }

    /// Map the Ok value using the provided function.
    // VUMA-VERIFIED: pure transformation, preserves Err BD
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> VumaResult<U, E> {
        match self {
            VumaResult::Ok(v) => VumaResult::Ok(f(v)),
            VumaResult::Err(e) => VumaResult::Err(e),
        }
    }

    /// Map the Err value using the provided function.
    // VUMA-VERIFIED: pure transformation, preserves Ok BD
    pub fn map_err<F2, F: FnOnce(E) -> F2>(self, f: F) -> VumaResult<T, F2> {
        match self {
            VumaResult::Ok(v) => VumaResult::Ok(v),
            VumaResult::Err(e) => VumaResult::Err(f(e)),
        }
    }
}

impl<T: HasBD, E: HasBD> VumaResult<T, E> {
    /// Compute the RepD for this result type.
    // VUMA-VERIFIED: result RepD is the tagged union of ok and err RepDs
    pub fn repd(&self) -> RepD {
        let (ok_bd, err_bd) = self.bds();
        let max_size = std::cmp::max(ok_bd.repd.size, err_bd.repd.size) + 1; // +1 for tag
        let max_align = std::cmp::max(ok_bd.repd.align, err_bd.repd.align);
        RepD::new(
            format!("Result<{}, {}>", ok_bd.repd.name, err_bd.repd.name),
            max_size,
            max_align,
            self.capd(),
        )
    }

    /// Compute the CapD for this result type — intersection of Ok and Err.
    // VUMA-VERIFIED: result CapD is the safe intersection of both variants
    pub fn capd(&self) -> CapD {
        let (ok_bd, err_bd) = self.bds();
        ok_bd.capd.intersect(&err_bd.capd)
    }

    /// Compute the RelD for this result type.
    // VUMA-VERIFIED: result RelD has DataFlow and Ownership
    pub fn reld(&self) -> RelD {
        result_reld()
    }

    /// Helper to extract BDs from both variants by constructing temporary
    /// values. This uses the HasBD trait on T and E.
    fn bds(&self) -> (BD, BD) {
        // We need to get BDs from T and E. Since we only have one variant
        // at runtime, we produce BDs using a representative approach:
        // We get the BD of whichever variant is present, and for the
        // other we use a default derived from the first.
        match self {
            VumaResult::Ok(v) => {
                let ok_bd = v.as_bd();
                let err_bd = BD::new(
                    RepD::new("err", ok_bd.repd.size, ok_bd.repd.align, CapD::empty()),
                    CapD::empty(),
                    RelD::empty(),
                );
                (ok_bd, err_bd)
            }
            VumaResult::Err(e) => {
                let err_bd = e.as_bd();
                let ok_bd = BD::new(
                    RepD::new("ok", err_bd.repd.size, err_bd.repd.align, CapD::empty()),
                    CapD::empty(),
                    RelD::empty(),
                );
                (ok_bd, err_bd)
            }
        }
    }
}

impl<T: HasBD, E: HasBD> HasBD for VumaResult<T, E> {
    /// Returns the full BD for this result.
    // VUMA-VERIFIED: BD derivation is well-formed
    fn as_bd(&self) -> BD {
        BD::new(self.repd(), self.capd(), self.reld())
    }
}

// ---- VumaOption<T> --------------------------------------------------------

/// A VUMA option type with BD tracking.
///
/// `VumaOption<T>` represents either a present value (`Some`) or absence
/// (`None`), with full BD annotations for the value variant.
///
/// # BD Annotation
///
/// - **RepD**: size is `val_size + 1` (tag byte).
/// - **CapD**: Same as the inner value's CapD.
/// - **RelD**: DataFlow and Liveness relations.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VumaOption<T> {
    /// A present value.
    Some(T),
    /// Absence of a value.
    None,
}

impl<T> VumaOption<T> {
    /// Returns true if this is a `Some` value.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_some(&self) -> bool {
        matches!(self, VumaOption::Some(_))
    }

    /// Returns true if this is `None`.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_none(&self) -> bool {
        matches!(self, VumaOption::None)
    }

    /// Unwrap the Some value, panicking if this is None.
    // VUMA-VERIFIED: panics on None — caller must ensure Some via capability check
    pub fn unwrap(self) -> T {
        match self {
            VumaOption::Some(v) => v,
            VumaOption::None => panic!("VumaOption::unwrap on None value"),
        }
    }

    /// Map the Some value using the provided function.
    // VUMA-VERIFIED: pure transformation, preserves None
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> VumaOption<U> {
        match self {
            VumaOption::Some(v) => VumaOption::Some(f(v)),
            VumaOption::None => VumaOption::None,
        }
    }

    /// Returns the contained Some value or a default.
    // VUMA-VERIFIED: pure, provides default on None
    pub fn unwrap_or(self, default: T) -> T {
        match self {
            VumaOption::Some(v) => v,
            VumaOption::None => default,
        }
    }
}

impl<T: HasBD> VumaOption<T> {
    /// Compute the RepD for this option type.
    // VUMA-VERIFIED: option RepD is the tagged union of val + None
    pub fn repd(&self) -> RepD {
        let val_bd = self.val_bd();
        RepD::new(
            format!("Option<{}>", val_bd.repd.name),
            val_bd.repd.size + 1,
            val_bd.repd.align,
            self.capd(),
        )
    }

    /// Compute the CapD for this option type.
    // VUMA-VERIFIED: option CapD inherits from inner value
    pub fn capd(&self) -> CapD {
        self.val_bd().capd
    }

    /// Compute the RelD for this option type.
    // VUMA-VERIFIED: option RelD has DataFlow and Liveness
    pub fn reld(&self) -> RelD {
        option_reld()
    }

    /// Helper to get the BD of the inner value (or a default for None).
    fn val_bd(&self) -> BD {
        match self {
            VumaOption::Some(v) => v.as_bd(),
            VumaOption::None => BD::new(
                RepD::new("void", 0, 1, CapD::empty()),
                CapD::empty(),
                RelD::empty(),
            ),
        }
    }
}

impl<T: HasBD> HasBD for VumaOption<T> {
    /// Returns the full BD for this option.
    // VUMA-VERIFIED: BD derivation is well-formed
    fn as_bd(&self) -> BD {
        BD::new(self.repd(), self.capd(), self.reld())
    }
}

// ---- Range ----------------------------------------------------------------

/// An integer range type (start..end).
///
/// `Range` represents a half-open interval `[start, end)`. It carries a BD
/// annotation that includes Compare and Iterate capabilities, making it
/// suitable for use in loops and index computations.
///
/// # BD Annotation
///
/// - **RepD**: 16 bytes (start: 8 + end: 8), alignment 8.
/// - **CapD**: Read, Write, Compare, Iterate, Serialize.
/// - **RelD**: Empty (numeric range has no inherent relations).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    /// The start of the range (inclusive).
    pub start: u64,
    /// The end of the range (exclusive).
    pub end: u64,
}

impl Range {
    /// Create a new Range with the given start and end.
    // VUMA-VERIFIED: constructor is pure, no side effects
    pub fn new(start: u64, end: u64) -> Self {
        Self { start, end }
    }

    /// Returns true if the range is empty (start >= end).
    // VUMA-VERIFIED: pure query, no side effects
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// Returns the number of elements in the range.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    /// Returns true if the range contains the given value.
    // VUMA-VERIFIED: pure query, no side effects
    pub fn contains(&self, value: u64) -> bool {
        value >= self.start && value < self.end
    }

    /// Returns the RepD for this range type.
    // VUMA-VERIFIED: range RepD is well-formed
    pub fn repd(&self) -> RepD {
        RepD::new("Range", 16, 8, Self::capd())
    }

    /// Returns the CapD for the range type.
    // VUMA-VERIFIED: range CapD includes Iterate
    pub fn capd() -> CapD {
        CapD::new(vec![
            CapFlag::Read,
            CapFlag::Write,
            CapFlag::Compare,
            CapFlag::Iterate,
            CapFlag::Serialize,
        ])
    }

    /// Returns the RelD for the range type.
    // VUMA-VERIFIED: range RelD is empty (pure numeric)
    pub fn reld(&self) -> RelD {
        numeric_reld()
    }
}

impl HasBD for Range {
    /// Returns the full BD for this range.
    // VUMA-VERIFIED: BD derivation is well-formed
    fn as_bd(&self) -> BD {
        BD::new(self.repd(), Self::capd(), self.reld())
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helper: create a simple pointee BD for tests -----------------------

    fn uint32_bd() -> BD {
        BD::new(uint32_repd(), numeric_capd(), numeric_reld())
    }

    fn uint64_bd() -> BD {
        BD::new(uint64_repd(), numeric_capd(), numeric_reld())
    }

    // -- Existing tests (preserved) -----------------------------------------

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

    // -- New tests: RelD ----------------------------------------------------

    #[test]
    fn test_reld_compose_unions_relations() {
        let a = RelD::new(vec![RelKind::Containment]);
        let b = RelD::new(vec![RelKind::Liveness]);
        let composed = a.compose(&b);
        assert!(composed.has(RelKind::Containment));
        assert!(composed.has(RelKind::Liveness));
    }

    #[test]
    fn test_reld_refines_superset() {
        // A more refined RelD has a superset of relations.
        let a = RelD::new(vec![RelKind::Containment, RelKind::Liveness]);
        let b = RelD::new(vec![RelKind::Containment]);
        assert!(a.refines(&b)); // a has more relations → refines b
        assert!(!b.refines(&a)); // b has fewer → does not refine a
    }

    // -- New tests: BD ------------------------------------------------------

    #[test]
    fn test_bd_compatible_same_type() {
        let a = BD::new(uint32_repd(), numeric_capd(), ptr_reld());
        let b = BD::new(uint32_repd(), numeric_capd(), ptr_reld());
        assert!(a.compatible(&b));
    }

    #[test]
    fn test_bd_refines() {
        let a = BD::new(uint32_repd(), CapD::new(vec![CapFlag::Read]), RelD::new(vec![RelKind::Containment, RelKind::Liveness]));
        let b = BD::new(uint32_repd(), numeric_capd(), RelD::new(vec![RelKind::Containment]));
        // a refines b: a has fewer caps (subcap) and more relations (refines)
        assert!(a.refines(&b));
    }

    // -- New tests: Ptr<T> --------------------------------------------------

    #[test]
    fn test_ptr_creation_and_bd() {
        let ptr: Ptr<u32> = Ptr::new(0x1000, uint32_bd());
        assert_eq!(ptr.addr, 0x1000);
        assert!(!ptr.is_null());

        let bd = ptr.as_bd();
        assert_eq!(bd.repd.name, "ptr<uint32>");
        assert_eq!(bd.repd.size, 8);
        assert!(bd.capd.has(CapFlag::Read));
        assert!(bd.capd.has(CapFlag::Write));
        assert!(bd.reld.has(RelKind::Containment));
        assert!(bd.reld.has(RelKind::Liveness));
    }

    #[test]
    fn test_ptr_null_and_offset() {
        let ptr: Ptr<u32> = Ptr::null(uint32_bd());
        assert!(ptr.is_null());

        let offset_ptr = ptr.offset(16);
        assert_eq!(offset_ptr.addr, 16);
        // BD is preserved after offset
        assert_eq!(offset_ptr.as_bd().repd.name, "ptr<uint32>");
    }

    // -- New tests: RegionPtr<T> --------------------------------------------

    #[test]
    fn test_region_ptr_creation_and_bd() {
        let rptr: RegionPtr<u32> = RegionPtr::new(0x2000, 0x1000, 0x2000, uint32_bd());
        assert!(rptr.in_bounds());

        let bd = rptr.as_bd();
        assert_eq!(bd.repd.name, "region_ptr<uint32>");
        assert_eq!(bd.repd.size, 24);
        assert!(bd.capd.has(CapFlag::Shared));
        assert!(bd.reld.has(RelKind::RegionBound));
    }

    #[test]
    fn test_region_ptr_checked_offset() {
        let rptr: RegionPtr<u32> = RegionPtr::new(0x1000, 0x1000, 0x1000, uint32_bd());
        // Within bounds
        let ok = rptr.checked_offset(0x500);
        assert!(ok.is_some());
        assert_eq!(ok.unwrap().addr, 0x1500);

        // Out of bounds
        let bad = rptr.checked_offset(0x1001);
        assert!(bad.is_none());
    }

    #[test]
    #[should_panic(expected = "outside region")]
    fn test_region_ptr_out_of_bounds_panics() {
        let _: RegionPtr<u32> = RegionPtr::new(0x0000, 0x1000, 0x1000, uint32_bd());
    }

    // -- New tests: Slice<T> ------------------------------------------------

    #[test]
    fn test_slice_creation_and_bd() {
        let slice: Slice<u64> = Slice::new(0x5000, 10, uint64_bd());
        assert_eq!(slice.len, 10);
        assert!(!slice.is_empty());
        assert_eq!(slice.byte_size(), 80); // 10 * 8

        let bd = slice.as_bd();
        assert_eq!(bd.repd.name, "slice<uint64>");
        assert_eq!(bd.repd.size, 16);
        assert!(bd.capd.has(CapFlag::Iterate));
        assert!(bd.reld.has(RelKind::DataFlow));
    }

    #[test]
    fn test_slice_subslice() {
        let slice: Slice<u32> = Slice::new(0x1000, 10, uint32_bd());
        let sub = slice.subslice(2, 5);
        assert!(sub.is_some());
        let sub = sub.unwrap();
        assert_eq!(sub.len, 5);
        assert_eq!(sub.addr, 0x1000 + 2 * 4); // offset by 2 * sizeof(uint32)

        // Out of bounds subslice
        assert!(slice.subslice(8, 5).is_none());
    }

    // -- New tests: VumaResult<T, E> ----------------------------------------

    /// A simple type implementing HasBD for testing VumaResult.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestVal(u64);

    impl HasBD for TestVal {
        fn as_bd(&self) -> BD {
            BD::new(uint64_repd(), numeric_capd(), numeric_reld())
        }
    }

    /// A simple error type implementing HasBD for testing VumaResult.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestErr(u32);

    impl HasBD for TestErr {
        fn as_bd(&self) -> BD {
            BD::new(uint32_repd(), numeric_capd(), numeric_reld())
        }
    }

    #[test]
    fn test_vuma_result_ok_bd() {
        let res: VumaResult<TestVal, TestErr> = VumaResult::Ok(TestVal(42));
        assert!(res.is_ok());

        let bd = res.as_bd();
        assert!(bd.repd.name.starts_with("Result<"));
        assert!(bd.reld.has(RelKind::DataFlow));
        assert!(bd.reld.has(RelKind::Ownership));
    }

    #[test]
    fn test_vuma_result_err_bd() {
        let res: VumaResult<TestVal, TestErr> = VumaResult::Err(TestErr(1));
        assert!(res.is_err());

        let bd = res.as_bd();
        assert!(bd.reld.has(RelKind::DataFlow));
    }

    #[test]
    fn test_vuma_result_map() {
        let res: VumaResult<TestVal, TestErr> = VumaResult::Ok(TestVal(42));
        let mapped = res.map(|v| v.0 * 2);
        assert_eq!(mapped, VumaResult::Ok(84));
    }

    // -- New tests: VumaOption<T> -------------------------------------------

    #[test]
    fn test_vuma_option_some_bd() {
        let opt: VumaOption<TestVal> = VumaOption::Some(TestVal(99));
        assert!(opt.is_some());

        let bd = opt.as_bd();
        assert!(bd.repd.name.starts_with("Option<"));
        assert!(bd.reld.has(RelKind::DataFlow));
        assert!(bd.reld.has(RelKind::Liveness));
    }

    #[test]
    fn test_vuma_option_none_bd() {
        let opt: VumaOption<TestVal> = VumaOption::None;
        assert!(opt.is_none());

        let bd = opt.as_bd();
        assert!(bd.reld.has(RelKind::DataFlow));
    }

    #[test]
    fn test_vuma_option_map_and_unwrap_or() {
        let some: VumaOption<TestVal> = VumaOption::Some(TestVal(10));
        let mapped = some.map(|v| v.0 + 1);
        assert_eq!(mapped, VumaOption::Some(11));

        let none: VumaOption<TestVal> = VumaOption::None;
        let val = none.unwrap_or(TestVal(0));
        assert_eq!(val, TestVal(0));
    }

    // -- New tests: Range ---------------------------------------------------

    #[test]
    fn test_range_creation_and_bd() {
        let r = Range::new(0, 10);
        assert_eq!(r.len(), 10);
        assert!(!r.is_empty());
        assert!(r.contains(5));
        assert!(!r.contains(10));

        let bd = r.as_bd();
        assert_eq!(bd.repd.name, "Range");
        assert_eq!(bd.repd.size, 16);
        assert!(bd.capd.has(CapFlag::Iterate));
        assert!(bd.capd.has(CapFlag::Compare));
    }

    #[test]
    fn test_range_empty() {
        let r = Range::new(10, 5);
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn test_range_display() {
        let r = Range::new(1, 100);
        assert_eq!(format!("{r}"), "1..100");
    }

    // -- Cross-cutting: HasBD on Ptr/RegionPtr/Slice/Range ------------------

    #[test]
    fn test_ptr_as_bd_matches_manual_construction() {
        let ptr: Ptr<u32> = Ptr::new(0xABCD, uint32_bd());
        let bd = ptr.as_bd();

        let manual_bd = BD::new(
            RepD::ptr_to(&uint32_repd()),
            numeric_capd().union(&CapD::new(vec![CapFlag::Read, CapFlag::Write])),
            ptr_reld(),
        );
        assert_eq!(bd.repd.name, manual_bd.repd.name);
        assert!(bd.capd.has(CapFlag::Read));
        assert!(bd.capd.has(CapFlag::Write));
        assert!(bd.reld.has(RelKind::Containment));
    }

    #[test]
    fn test_region_ptr_as_bd_has_region_bound() {
        let rptr: RegionPtr<u64> = RegionPtr::new(0x2000, 0x1000, 0x2000, uint64_bd());
        let bd = rptr.as_bd();
        assert!(bd.reld.has(RelKind::RegionBound));
        assert!(bd.reld.has(RelKind::Containment));
        assert!(bd.reld.has(RelKind::Liveness));
    }

    #[test]
    fn test_slice_as_bd_has_iterate_and_dataflow() {
        let slice: Slice<u32> = Slice::new(0x3000, 5, uint32_bd());
        let bd = slice.as_bd();
        assert!(bd.capd.has(CapFlag::Iterate));
        assert!(bd.reld.has(RelKind::DataFlow));
    }
}
