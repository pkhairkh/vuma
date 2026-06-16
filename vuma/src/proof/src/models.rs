//! # Shared Proof Model Types
//!
//! Unified model types for the VUMA proof subsystem. These types consolidate
//! the MSG, SCG, Region, Access, Derivation, and related types that were
//! previously duplicated across the proof sub-modules (liveness, exclusivity,
//! cleanup, origin, interpretation).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::judgment::RegionId;
use crate::proof::{AccessId, ProgramPoint};

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// Unique identifier for a synchronization edge.
pub type SyncEdgeId = u64;

/// Unique identifier for a lock.
pub type LockId = u64;

/// Byte address in the program's memory model.
pub type Addr = u64;

/// Unique identifier for a Representation Descriptor.
pub type RepDId = u64;

/// Unique identifier for a Derivation within the MSG.
pub type DerivationId = u64;

/// Unique identifier for a taint label.
pub type TaintLabelId = u64;

// ---------------------------------------------------------------------------
// ProofRegionStatus
// ---------------------------------------------------------------------------

/// Status of a memory region at a given program point.
///
/// This enum is the union of all region status variants used across the proof
/// sub-modules. The `Leaked` variant is used by the liveness prover; the
/// interpretation prover uses only Allocated/Freed/Stack/Mapped.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ProofRegionStatus {
    /// Region has been allocated and is still live.
    Allocated,
    /// Region has been freed and is no longer accessible.
    Freed,
    /// Region is a stack-allocated frame.
    Stack,
    /// Region is a memory-mapped region.
    Mapped,
    /// Region is intentionally never freed (arena, global).
    Leaked,
}

// ---------------------------------------------------------------------------
// ProofAccessKind
// ---------------------------------------------------------------------------

/// Access kind — read or write.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ProofAccessKind {
    /// A read operation.
    Read,
    /// A write operation.
    Write,
}

// ---------------------------------------------------------------------------
// ProofRegion
// ---------------------------------------------------------------------------

/// A memory region — a contiguous range of bytes.
///
/// This unified type covers all region fields used across the proof
/// sub-modules. Fields that are not used by a particular module can be
/// left at their default values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofRegion {
    /// Unique region identifier (typed).
    pub id: RegionId,
    /// Human-readable name (optional, for diagnostics).
    pub name: Option<String>,
    /// Size in bytes.
    pub size: u64,
    /// Base address of the region.
    pub base_addr: u64,
    /// Current status (liveness/interpretation).
    pub status: ProofRegionStatus,
    /// Program point at which the region was allocated.
    pub alloc_point: u64,
    /// Program point at which the region was freed, if applicable.
    pub free_point: Option<u64>,
    /// Default Representation Descriptor id (interpretation).
    pub default_repd: Option<RepDId>,
    /// Security boundary label (optional, for information-flow analysis).
    pub security_boundary: Option<String>,
}

impl ProofRegion {
    /// Create a new allocated region (liveness-style constructor).
    pub fn new_allocated(id: impl Into<RegionId>, size: u64, alloc_point: u64) -> Self {
        Self {
            id: id.into(),
            name: None,
            size,
            base_addr: 0,
            status: ProofRegionStatus::Allocated,
            alloc_point,
            free_point: None,
            default_repd: None,
            security_boundary: None,
        }
    }

    /// Create a new region with full fields (interpretation-style constructor).
    pub fn new(
        id: impl Into<RegionId>,
        base_addr: u64,
        size: u64,
        status: ProofRegionStatus,
        default_repd: RepDId,
    ) -> Self {
        Self {
            id: id.into(),
            name: None,
            size,
            base_addr,
            status,
            alloc_point: 0,
            free_point: None,
            default_repd: Some(default_repd),
            security_boundary: None,
        }
    }

    /// Create a minimal region (exclusivity-style constructor).
    pub fn minimal(id: impl Into<RegionId>, base_addr: u64, size: u64) -> Self {
        Self {
            id: id.into(),
            name: None,
            size,
            base_addr,
            status: ProofRegionStatus::Allocated,
            alloc_point: 0,
            free_point: None,
            default_repd: None,
            security_boundary: None,
        }
    }

    /// Returns `true` if the region is allocated at the given program point.
    pub fn is_allocated_at(&self, pp: u64) -> bool {
        match self.status {
            ProofRegionStatus::Allocated | ProofRegionStatus::Stack | ProofRegionStatus::Mapped => {
                self.alloc_point <= pp && self.free_point.is_none_or(|fp| pp < fp)
            }
            ProofRegionStatus::Leaked => self.alloc_point <= pp,
            ProofRegionStatus::Freed => {
                self.alloc_point <= pp && self.free_point.is_some_and(|fp| pp < fp)
            }
        }
    }

    /// Returns the address range [base_addr, base_addr + size).
    pub fn range(&self) -> std::ops::Range<u64> {
        self.base_addr..self.base_addr + self.size
    }
}

// ---------------------------------------------------------------------------
// ProofAccess
// ---------------------------------------------------------------------------

/// A memory access record.
///
/// This unified type covers all access fields used across the proof
/// sub-modules. Fields that are not relevant for a particular proof
/// kind can be left at their default values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofAccess {
    /// Unique access identifier.
    pub id: u64,
    /// The region targeted by this access (liveness, exclusivity).
    pub region: RegionId,
    /// Byte offset within the region (liveness).
    pub offset: u64,
    /// Number of bytes accessed (liveness calls this `width`).
    pub width: u64,
    /// Kind of access.
    pub kind: ProofAccessKind,
    /// Program point where this access occurs.
    pub program_point: u64,
    /// The derivation being accessed (exclusivity).
    pub derivation_id: u64,
    /// Starting address (resolved from derivation, exclusivity).
    pub addr: u64,
    /// Size in bytes (exclusivity, interpretation).
    pub size: u64,
    /// The target derivation (interpretation).
    pub target_derivation: DerivationId,
    /// The RepD expected by this access (interpretation).
    pub expected_repd: Option<RepDId>,
}

impl ProofAccess {
    /// Create a new access record (liveness-style constructor).
    pub fn new_liveness(
        id: u64,
        region: impl Into<RegionId>,
        offset: u64,
        width: u64,
        kind: ProofAccessKind,
        program_point: u64,
    ) -> Self {
        Self {
            id,
            region: region.into(),
            offset,
            width,
            kind,
            program_point,
            derivation_id: 0,
            addr: 0,
            size: width,
            target_derivation: 0,
            expected_repd: None,
        }
    }

    /// Create a new access record (interpretation-style constructor).
    pub fn new_interp(
        id: AccessId,
        target: DerivationId,
        kind: ProofAccessKind,
        size: u64,
        pp: ProgramPoint,
        expected_repd: RepDId,
    ) -> Self {
        Self {
            id,
            region: RegionId::from(0u64),
            offset: 0,
            width: size,
            kind,
            program_point: pp,
            derivation_id: 0,
            addr: 0,
            size,
            target_derivation: target,
            expected_repd: Some(expected_repd),
        }
    }

    /// Returns `true` if the access is within bounds of the given region.
    pub fn within_bounds(&self, region: &ProofRegion) -> bool {
        self.offset + self.width <= region.size
    }

    /// Returns true if this is a write access.
    pub fn is_write(&self) -> bool {
        self.kind == ProofAccessKind::Write
    }

    /// Returns true if this is a read access.
    pub fn is_read(&self) -> bool {
        self.kind == ProofAccessKind::Read
    }
}

// ---------------------------------------------------------------------------
// ProofDerivation
// ---------------------------------------------------------------------------

/// A derivation — computation of an address from a region or another derivation.
///
/// This unified type covers both the simplified exclusivity derivation
/// (root_region + offset) and the full interpretation derivation
/// (source_region/source_derivation + offset + optional cast).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofDerivation {
    /// Unique identifier.
    pub id: u64,
    /// Root region this derivation traces to (exclusivity style).
    pub root_region: Option<RegionId>,
    /// The source region, if this derivation comes directly from a region
    /// (interpretation style).
    pub source_region: Option<RegionId>,
    /// The source derivation, if this derivation comes from another
    /// derivation (interpretation style).
    pub source_derivation: Option<DerivationId>,
    /// Byte offset from the source.
    pub offset: i64,
    /// If this is a cast derivation, the target RepD.
    pub cast: Option<RepDId>,
}

impl ProofDerivation {
    /// Create an exclusivity-style derivation (root_region + offset).
    pub fn simple(id: u64, root_region: impl Into<RegionId>, offset: u64) -> Self {
        Self {
            id,
            root_region: Some(root_region.into()),
            source_region: None,
            source_derivation: None,
            offset: offset as i64,
            cast: None,
        }
    }

    /// Create a base derivation (source = region, offset = 0, no cast).
    pub fn base(region_id: impl Into<RegionId>) -> Self {
        Self {
            id: 0,
            root_region: None,
            source_region: Some(region_id.into()),
            source_derivation: None,
            offset: 0,
            cast: None,
        }
    }

    /// Create an offset derivation from a region.
    pub fn offset_from_region(region_id: impl Into<RegionId>, offset: i64) -> Self {
        Self {
            id: 0,
            root_region: None,
            source_region: Some(region_id.into()),
            source_derivation: None,
            offset,
            cast: None,
        }
    }

    /// Create a cast derivation.
    pub fn cast_from(derivation_id: DerivationId, target_repd: RepDId) -> Self {
        Self {
            id: 0,
            root_region: None,
            source_region: None,
            source_derivation: Some(derivation_id),
            offset: 0,
            cast: Some(target_repd),
        }
    }

    /// Returns true if this is a cast derivation.
    pub fn is_cast(&self) -> bool {
        self.cast.is_some()
    }

    /// Returns true if this is a base derivation.
    pub fn is_base(&self) -> bool {
        self.source_region.is_some()
            && self.source_derivation.is_none()
            && self.offset == 0
            && self.cast.is_none()
    }

    /// Get the effective root region — returns root_region if set (exclusivity
    /// style), or source_region if set (interpretation style).
    pub fn effective_root_region(&self) -> Option<RegionId> {
        self.root_region.or(self.source_region)
    }
}

// ---------------------------------------------------------------------------
// ProofSyncEdge / SyncOrdering
// ---------------------------------------------------------------------------

/// The ordering semantics of a synchronization edge (spec §2.5).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SyncOrdering {
    /// a₁ completes before a₂ begins (sequential consistency, fork-join, message passing).
    HappensBefore,
    /// a₁ and a₂ access the same atomic variable with compatible memory ordering.
    Atomic,
    /// a₁ and a₂ are guarded by the same lock; mutual exclusion is guaranteed.
    Locked,
}

/// A synchronization edge between two accesses (spec §2.5).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofSyncEdge {
    /// Unique identifier for this edge.
    pub id: SyncEdgeId,
    /// The first access in the ordering.
    pub access1: AccessId,
    /// The second access in the ordering.
    pub access2: AccessId,
    /// The ordering semantics.
    pub ordering: SyncOrdering,
    /// If ordering is Locked, the lock that guards both accesses.
    pub lock: Option<LockId>,
}

// ---------------------------------------------------------------------------
// ProofMemOp / ProofMemOpKind
// ---------------------------------------------------------------------------

/// The kind of a memory operation recorded in the MSG.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ProofMemOpKind {
    /// Allocate a new memory region.
    Alloc,
    /// Free (release) a memory region.
    Free,
    /// Read from a memory region.
    Read,
    /// Write to a memory region.
    Write,
    /// Acquire ownership of a region (e.g. via lock or borrow).
    Acquire,
    /// Release ownership of a region.
    Release,
}

impl std::fmt::Display for ProofMemOpKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProofMemOpKind::Alloc => write!(f, "alloc"),
            ProofMemOpKind::Free => write!(f, "free"),
            ProofMemOpKind::Read => write!(f, "read"),
            ProofMemOpKind::Write => write!(f, "write"),
            ProofMemOpKind::Acquire => write!(f, "acquire"),
            ProofMemOpKind::Release => write!(f, "release"),
        }
    }
}

/// A memory operation node in the Memory State Graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofMemOp {
    /// The region this operation acts upon.
    pub region: RegionId,
    /// The kind of operation.
    pub kind: ProofMemOpKind,
    /// The program point at which this operation occurs.
    pub location: ProgramPoint,
}

impl ProofMemOp {
    /// Create a new memory operation.
    pub fn new(region: impl Into<RegionId>, kind: ProofMemOpKind, location: ProgramPoint) -> Self {
        Self {
            region: region.into(),
            kind,
            location,
        }
    }
}

// ---------------------------------------------------------------------------
// ProofSCGEdge / ProofSCG
// ---------------------------------------------------------------------------

/// An edge in the Synchronization/State Control Graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofSCGEdge {
    /// Source program point.
    pub from: u64,
    /// Destination program point.
    pub to: u64,
    /// Optional label (e.g. "seq", "then", "else", "loop-back").
    pub label: Option<String>,
}

impl ProofSCGEdge {
    /// Create a new SCG edge.
    pub fn new(from: u64, to: u64) -> Self {
        Self {
            from,
            to,
            label: None,
        }
    }

    /// Create a labeled SCG edge.
    pub fn labeled(from: u64, to: u64, label: impl Into<String>) -> Self {
        Self {
            from,
            to,
            label: Some(label.into()),
        }
    }

    /// Create an SCG edge with a required label (liveness-style).
    pub fn with_label(from: u64, to: u64, label: impl Into<String>) -> Self {
        Self {
            from,
            to,
            label: Some(label.into()),
        }
    }
}

/// The Synchronization/State Control Graph.
///
/// This unified type supports both the liveness-style SCG (nodes + edges)
/// and the cleanup-style SCG (edges + entry + exits).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofSCG {
    /// All nodes (program points) in the graph (liveness style).
    pub nodes: Vec<u64>,
    /// All directed edges.
    pub edges: Vec<ProofSCGEdge>,
    /// The entry point of the program (cleanup style).
    pub entry: u64,
    /// The exit points of the program (cleanup style).
    pub exits: Vec<u64>,
}

impl ProofSCG {
    /// Create an empty SCG.
    pub fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            entry: 0,
            exits: Vec::new(),
        }
    }

    /// Create a linear SCG with sequential program points 0..n.
    pub fn linear(n: u64) -> Self {
        let nodes: Vec<u64> = (0..n).collect();
        let edges: Vec<ProofSCGEdge> = (0..n.saturating_sub(1))
            .map(|i| ProofSCGEdge::with_label(i, i + 1, "seq"))
            .collect();
        Self {
            nodes,
            edges,
            entry: 0,
            exits: if n > 0 { vec![n - 1] } else { vec![] },
        }
    }

    /// Create a new SCG with the given entry and exit points (cleanup style).
    pub fn with_entry_exits(entry: ProgramPoint, exits: Vec<ProgramPoint>) -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            entry,
            exits,
        }
    }

    /// Add an edge to the SCG.
    pub fn add_edge(&mut self, edge: ProofSCGEdge) {
        self.edges.push(edge);
    }

    /// Return all successors of the given node.
    pub fn successors(&self, node: u64) -> Vec<u64> {
        self.edges
            .iter()
            .filter(|e| e.from == node)
            .map(|e| e.to)
            .collect()
    }

    /// Return all predecessors of the given node.
    pub fn predecessors(&self, node: u64) -> Vec<u64> {
        self.edges
            .iter()
            .filter(|e| e.to == node)
            .map(|e| e.from)
            .collect()
    }

    /// Detect whether the SCG contains a cycle (indicating a loop).
    pub fn has_cycle(&self) -> bool {
        let mut visited = HashSet::new();
        let mut on_stack = HashSet::new();

        fn dfs(
            scg: &ProofSCG,
            node: u64,
            visited: &mut HashSet<u64>,
            on_stack: &mut HashSet<u64>,
        ) -> bool {
            if on_stack.contains(&node) {
                return true;
            }
            if visited.contains(&node) {
                return false;
            }
            visited.insert(node);
            on_stack.insert(node);
            for succ in scg.successors(node) {
                if dfs(scg, succ, visited, on_stack) {
                    return true;
                }
            }
            on_stack.remove(&node);
            false
        }

        for &node in &self.nodes {
            if dfs(self, node, &mut visited, &mut on_stack) {
                return true;
            }
        }
        false
    }

    /// Return all program points in the SCG.
    pub fn all_points(&self) -> HashSet<u64> {
        let mut points = HashSet::new();
        points.insert(self.entry);
        for exit in &self.exits {
            points.insert(*exit);
        }
        for edge in &self.edges {
            points.insert(edge.from);
            points.insert(edge.to);
        }
        for &node in &self.nodes {
            points.insert(node);
        }
        points
    }

    /// Enumerate all paths from entry to any exit point (bounded by max_depth
    /// to avoid infinite loops in cyclic graphs).
    pub fn enumerate_paths(&self, max_depth: usize) -> Vec<Vec<ProgramPoint>> {
        let mut paths = Vec::new();
        let mut current = vec![vec![self.entry]];
        let exit_set: HashSet<ProgramPoint> = self.exits.iter().copied().collect();

        for _ in 0..max_depth {
            let mut next = Vec::new();
            for path in &current {
                let last = *path.last().unwrap();
                if exit_set.contains(&last) {
                    paths.push(path.clone());
                    continue;
                }
                let succs = self.successors(last);
                if succs.is_empty() {
                    // Dead end — treat as a terminal path.
                    paths.push(path.clone());
                } else {
                    for s in succs {
                        let mut new_path = path.clone();
                        new_path.push(s);
                        next.push(new_path);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            current = next;
        }
        // Any remaining incomplete paths also count.
        for path in current {
            let last = *path.last().unwrap();
            if !exit_set.contains(&last) {
                paths.push(path);
            }
        }
        paths
    }
}

// ---------------------------------------------------------------------------
// ProofRepD / BDKind / Compatibility
// ---------------------------------------------------------------------------

/// Byte descriptor category — the high-level classification of how a range of
/// bytes is interpreted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum BDKind {
    /// Raw bytes — the universal supertype; any RepD can be read as bytes.
    Bytes,
    /// Integer type (signed or unsigned, any width).
    Integer,
    /// Floating-point type.
    Float,
    /// Pointer type — requires initialization check.
    Pointer,
    /// Struct or compound type with named fields.
    Struct,
    /// Union type — the BD may be any of the member types.
    Union,
    /// A custom/user-defined representation.
    Custom,
}

impl std::fmt::Display for BDKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BDKind::Bytes => write!(f, "bytes"),
            BDKind::Integer => write!(f, "integer"),
            BDKind::Float => write!(f, "float"),
            BDKind::Pointer => write!(f, "pointer"),
            BDKind::Struct => write!(f, "struct"),
            BDKind::Union => write!(f, "union"),
            BDKind::Custom => write!(f, "custom"),
        }
    }
}

/// A Representation Descriptor (RepD) — describes how a range of bytes is
/// interpreted, including size, alignment, kind, and whether the bytes are
/// initialized.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofRepD {
    /// Unique identifier for this RepD.
    pub id: RepDId,
    /// High-level classification of the byte interpretation.
    pub kind: BDKind,
    /// Size in bytes of the described value.
    pub size: u64,
    /// Required alignment in bytes.
    pub alignment: u64,
    /// Whether the described bytes are known to be initialized.
    pub initialized: bool,
}

impl ProofRepD {
    /// Create a new RepD.
    pub fn new(id: RepDId, kind: BDKind, size: u64, alignment: u64, initialized: bool) -> Self {
        Self {
            id,
            kind,
            size,
            alignment,
            initialized,
        }
    }

    /// Convenience: create a bytes RepD.
    pub fn bytes(id: RepDId, size: u64, initialized: bool) -> Self {
        Self::new(id, BDKind::Bytes, size, 1, initialized)
    }

    /// Convenience: create an integer RepD.
    pub fn integer(id: RepDId, size: u64, alignment: u64, initialized: bool) -> Self {
        Self::new(id, BDKind::Integer, size, alignment, initialized)
    }

    /// Convenience: create a pointer RepD.
    pub fn pointer(id: RepDId, size: u64, alignment: u64, initialized: bool) -> Self {
        Self::new(id, BDKind::Pointer, size, alignment, initialized)
    }

    /// Check whether this RepD is a sub-RepD of `other` (i.e., `self ⊑ other`).
    pub fn is_sub_repd_of(&self, other: &ProofRepD) -> bool {
        if self.id == other.id {
            return true;
        }
        if other.kind == BDKind::Bytes {
            return true;
        }
        if self.kind == other.kind && self.size <= other.size {
            return true;
        }
        false
    }

    /// Check compatibility between two RepDs for a write-read pair.
    pub fn compatible_with(&self, read: &ProofRepD, access_addr: u64) -> Compatibility {
        if read.size > self.size {
            return Compatibility::Incompatible(format!(
                "size mismatch: write BD size {} < read BD size {}",
                self.size, read.size
            ));
        }

        if read.alignment > 0 && !access_addr.is_multiple_of(read.alignment) {
            return Compatibility::Incompatible(format!(
                "alignment violation: address 0x{:x} not aligned to {} bytes",
                access_addr, read.alignment
            ));
        }

        if read.kind == BDKind::Pointer && !self.initialized {
            return Compatibility::Incompatible(
                "reading uninitialized bytes as pointer type is forbidden".into(),
            );
        }

        if !valid_reinterpretation(self, read) {
            return Compatibility::Incompatible(format!(
                "invalid reinterpretation: {} -> {}",
                self.kind, read.kind
            ));
        }

        Compatibility::Compatible
    }
}

/// Result of a BD compatibility check between a write's BD and a read's BD.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Compatibility {
    /// The BDs are compatible.
    Compatible,
    /// The BDs are incompatible, with a reason.
    Incompatible(String),
}

impl Compatibility {
    /// Returns true if compatible.
    pub fn is_compatible(&self) -> bool {
        matches!(self, Compatibility::Compatible)
    }
}

/// Check whether reinterpreting `source` as `target` is semantically valid.
pub fn valid_reinterpretation(source: &ProofRepD, target: &ProofRepD) -> bool {
    if source.id == target.id {
        return true;
    }
    if source.is_sub_repd_of(target) {
        return true;
    }
    if source.kind == BDKind::Pointer
        && target.kind != BDKind::Pointer
        && target.kind != BDKind::Bytes
    {
        return false;
    }
    if source.kind == BDKind::Bytes {
        return true;
    }
    if target.kind == BDKind::Union {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// SourceTrust / SinkSensitivity (origin module)
// ---------------------------------------------------------------------------

/// Classification of a data source's trust level.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SourceTrust {
    /// Data from a trusted source (e.g. initialised memory, kernel-provided buffer).
    Trusted,
    /// Data from an untrusted source (e.g. user input, network packet).
    Untrusted,
    /// Data whose trust level is unknown / cannot be determined.
    Unknown,
}

impl std::fmt::Display for SourceTrust {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceTrust::Trusted => write!(f, "trusted"),
            SourceTrust::Untrusted => write!(f, "untrusted"),
            SourceTrust::Unknown => write!(f, "unknown"),
        }
    }
}

/// Classification of a sink's sensitivity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SinkSensitivity {
    /// The sink is public — no restriction on data flowing here.
    Public,
    /// The sink is sensitive — tainted data must not flow here.
    Sensitive,
    /// The sink is highly sensitive — even indirectly tainted data is barred.
    Critical,
}

impl std::fmt::Display for SinkSensitivity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SinkSensitivity::Public => write!(f, "public"),
            SinkSensitivity::Sensitive => write!(f, "sensitive"),
            SinkSensitivity::Critical => write!(f, "critical"),
        }
    }
}

// ---------------------------------------------------------------------------
// OriginInfo (origin module)
// ---------------------------------------------------------------------------

/// A lightweight view into the Memory State Graph for origin proof purposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginInfo {
    /// Regions that are known to be live (allocated, stack, mapped, device).
    pub live_regions: Vec<RegionId>,
    /// Regions that are known to be dead (freed or leaked).
    pub dead_regions: Vec<RegionId>,
    /// Derivation chains: each entry is (derivation_id, chain_of_region_ids).
    pub derivation_chains: Vec<(u64, Vec<RegionId>)>,
    /// Taint assignments: each entry maps a region to its taint label.
    pub taint_labels: Vec<(RegionId, TaintLabelId)>,
    /// Sink classifications: each entry maps a region to its sensitivity.
    pub sink_classifications: Vec<(RegionId, SinkSensitivity)>,
    /// Source trust levels: each entry maps a region to its trust level.
    pub source_trust: Vec<(RegionId, SourceTrust)>,
    /// Flow edges: (source_region, target_region) indicating data flow.
    pub flow_edges: Vec<(RegionId, RegionId)>,
}

impl OriginInfo {
    /// Create an empty `OriginInfo`.
    pub fn new() -> Self {
        Self {
            live_regions: Vec::new(),
            dead_regions: Vec::new(),
            derivation_chains: Vec::new(),
            taint_labels: Vec::new(),
            sink_classifications: Vec::new(),
            source_trust: Vec::new(),
            flow_edges: Vec::new(),
        }
    }

    /// Check whether a region is live.
    pub fn is_live(&self, rid: RegionId) -> bool {
        self.live_regions.contains(&rid)
    }

    /// Check whether a region is dead.
    pub fn is_dead(&self, rid: RegionId) -> bool {
        self.dead_regions.contains(&rid)
    }

    /// Look up the derivation chain for a given derivation id.
    pub fn chain_for(&self, derivation_id: u64) -> Option<&Vec<RegionId>> {
        self.derivation_chains
            .iter()
            .find(|(id, _)| *id == derivation_id)
            .map(|(_, chain)| chain)
    }

    /// Return the taint label for a region, if any.
    pub fn taint_of(&self, rid: RegionId) -> Option<TaintLabelId> {
        self.taint_labels
            .iter()
            .find(|(r, _)| *r == rid)
            .map(|(_, label)| *label)
    }

    /// Return the sink sensitivity for a region, if any.
    pub fn sink_sensitivity(&self, rid: RegionId) -> Option<SinkSensitivity> {
        self.sink_classifications
            .iter()
            .find(|(r, _)| *r == rid)
            .map(|(_, s)| *s)
    }

    /// Return the trust level of a source region, if classified.
    pub fn trust_of(&self, rid: RegionId) -> Option<SourceTrust> {
        self.source_trust
            .iter()
            .find(|(r, _)| *r == rid)
            .map(|(_, t)| *t)
    }

    /// Return all regions that receive data from the given source region.
    pub fn flow_targets(&self, source: RegionId) -> Vec<RegionId> {
        self.flow_edges
            .iter()
            .filter(|(s, _)| *s == source)
            .map(|(_, t)| *t)
            .collect()
    }

    /// Return all regions that send data to the given target region.
    pub fn flow_sources(&self, target: RegionId) -> Vec<RegionId> {
        self.flow_edges
            .iter()
            .filter(|(_, t)| *t == target)
            .map(|(s, _)| *s)
            .collect()
    }

    /// Transitively compute all regions reachable from `source` via flow edges.
    pub fn reachable_from(&self, source: RegionId) -> Vec<RegionId> {
        let mut visited = Vec::new();
        let mut stack = vec![source];
        while let Some(current) = stack.pop() {
            if visited.contains(&current) {
                continue;
            }
            visited.push(current);
            for target in self.flow_targets(current) {
                if !visited.contains(&target) {
                    stack.push(target);
                }
            }
        }
        visited
    }
}

impl Default for OriginInfo {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for [`OriginInfo`].
#[derive(Debug, Clone, Default)]
pub struct OriginInfoBuilder {
    info: OriginInfo,
}

impl OriginInfoBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a live region.
    pub fn live_region(mut self, rid: impl Into<RegionId>) -> Self {
        self.info.live_regions.push(rid.into());
        self
    }

    /// Add a dead region.
    pub fn dead_region(mut self, rid: impl Into<RegionId>) -> Self {
        self.info.dead_regions.push(rid.into());
        self
    }

    /// Add a derivation chain.
    pub fn derivation_chain(mut self, derivation_id: u64, chain: Vec<RegionId>) -> Self {
        self.info.derivation_chains.push((derivation_id, chain));
        self
    }

    /// Add a taint label.
    pub fn taint_label(mut self, rid: impl Into<RegionId>, label: TaintLabelId) -> Self {
        self.info.taint_labels.push((rid.into(), label));
        self
    }

    /// Add a sink classification.
    pub fn sink_classification(
        mut self,
        rid: impl Into<RegionId>,
        sensitivity: SinkSensitivity,
    ) -> Self {
        self.info
            .sink_classifications
            .push((rid.into(), sensitivity));
        self
    }

    /// Add a source trust level.
    pub fn source_trust(mut self, rid: impl Into<RegionId>, trust: SourceTrust) -> Self {
        self.info.source_trust.push((rid.into(), trust));
        self
    }

    /// Add a flow edge.
    pub fn flow_edge(mut self, source: impl Into<RegionId>, target: impl Into<RegionId>) -> Self {
        self.info.flow_edges.push((source.into(), target.into()));
        self
    }

    /// Build the `OriginInfo`.
    pub fn build(self) -> OriginInfo {
        self.info
    }
}

// ---------------------------------------------------------------------------
// ProofMSG — Memory State Graph
// ---------------------------------------------------------------------------

/// The Memory State Graph — the central data structure for proof construction.
///
/// This unified type supports all the different MSG representations used
/// across the proof sub-modules. Fields that are not used by a particular
/// proof kind can be left empty.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofMSG {
    /// All memory regions (liveness, exclusivity, interpretation).
    pub regions: Vec<ProofRegion>,
    /// All derivations (exclusivity, interpretation).
    pub derivations: Vec<ProofDerivation>,
    /// All access records (liveness, exclusivity, interpretation).
    pub accesses: Vec<ProofAccess>,
    /// All synchronization edges (exclusivity).
    pub sync_edges: Vec<ProofSyncEdge>,
    /// All Representation Descriptors (interpretation).
    pub repds: Vec<ProofRepD>,
    /// Memory operations (cleanup).
    pub ops: Vec<ProofMemOp>,
    /// MSG edges: (from_program_point, to_program_point) (cleanup).
    pub msg_edges: Vec<(ProgramPoint, ProgramPoint)>,
}

impl Default for ProofMSG {
    fn default() -> Self {
        Self::new()
    }
}

impl ProofMSG {
    /// Create an empty MSG.
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            derivations: Vec::new(),
            accesses: Vec::new(),
            sync_edges: Vec::new(),
            repds: Vec::new(),
            ops: Vec::new(),
            msg_edges: Vec::new(),
        }
    }

    /// Create an empty MSG (alias for `new()`).
    pub fn empty() -> Self {
        Self::new()
    }

    /// Create an MSG from a list of operations and edges (cleanup style).
    pub fn from_ops(ops: Vec<ProofMemOp>, edges: Vec<(ProgramPoint, ProgramPoint)>) -> Self {
        Self {
            ops,
            msg_edges: edges,
            ..Self::new()
        }
    }

    // -----------------------------------------------------------------------
    // Region lookups
    // -----------------------------------------------------------------------

    /// Look up a region by id.
    pub fn find_region(&self, id: RegionId) -> Option<&ProofRegion> {
        self.regions.iter().find(|r| r.id == id)
    }

    /// Alias for `find_region` (interpretation style naming).
    pub fn get_region(&self, id: RegionId) -> Option<&ProofRegion> {
        self.find_region(id)
    }

    // -----------------------------------------------------------------------
    // Access lookups
    // -----------------------------------------------------------------------

    /// Look up an access by id.
    pub fn find_access(&self, id: u64) -> Option<&ProofAccess> {
        self.accesses.iter().find(|a| a.id == id)
    }

    /// Alias for `find_access` (interpretation style naming).
    pub fn get_access(&self, id: AccessId) -> Option<&ProofAccess> {
        self.find_access(id)
    }

    // -----------------------------------------------------------------------
    // Derivation lookups
    // -----------------------------------------------------------------------

    /// Look up a derivation by id.
    pub fn find_derivation(&self, id: u64) -> Option<&ProofDerivation> {
        self.derivations.iter().find(|d| d.id == id)
    }

    /// Alias for `find_derivation` (interpretation style naming).
    pub fn get_derivation(&self, id: DerivationId) -> Option<&ProofDerivation> {
        self.find_derivation(id)
    }

    // -----------------------------------------------------------------------
    // RepD lookups (interpretation)
    // -----------------------------------------------------------------------

    /// Look up a RepD by id.
    pub fn get_repd(&self, id: RepDId) -> Option<&ProofRepD> {
        self.repds.iter().find(|r| r.id == id)
    }

    // -----------------------------------------------------------------------
    // Sync edge lookups (exclusivity)
    // -----------------------------------------------------------------------

    /// Return all sync edges incident to a given access (in either direction).
    pub fn sync_edges_for(&self, access_id: AccessId) -> Vec<&ProofSyncEdge> {
        self.sync_edges
            .iter()
            .filter(|e| e.access1 == access_id || e.access2 == access_id)
            .collect()
    }

    // -----------------------------------------------------------------------
    // Interpretation-specific: resolve derivation chains
    // -----------------------------------------------------------------------

    /// Resolve the root region of a derivation by walking the source chain.
    pub fn region_of(&self, derivation_id: DerivationId) -> Option<RegionId> {
        let mut current_id = derivation_id;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(current_id) {
                return None;
            }
            let deriv = self.get_derivation(current_id)?;
            // If this derivation has an explicit source_region, return it.
            if let Some(rid) = deriv.source_region {
                return Some(rid);
            }
            // If it has a root_region (exclusivity style), return it.
            if let Some(rid) = deriv.root_region {
                return Some(rid);
            }
            if let Some(did) = deriv.source_derivation {
                current_id = did;
            } else {
                return None;
            }
        }
    }

    /// Compute the effective RepD of a derivation by walking the chain and
    /// finding the most recent cast, or falling back to the region's default.
    pub fn repd_of(&self, derivation_id: DerivationId) -> Option<RepDId> {
        let mut current_id = derivation_id;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(current_id) {
                return None;
            }
            let deriv = self.get_derivation(current_id)?;
            if let Some(cast_repd) = deriv.cast {
                return Some(cast_repd);
            }
            if let Some(rid) = deriv.source_region {
                let region = self.get_region(rid)?;
                return region.default_repd;
            }
            if let Some(rid) = deriv.root_region {
                // For exclusivity-style derivations, no default RepD.
                let region = self.get_region(rid)?;
                return region.default_repd;
            }
            if let Some(did) = deriv.source_derivation {
                current_id = did;
            } else {
                return None;
            }
        }
    }

    /// Resolve the concrete address of a derivation.
    pub fn addr_of(&self, derivation_id: DerivationId) -> Option<u64> {
        let mut current_id = derivation_id;
        let mut cumulative_offset: i64 = 0;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(current_id) {
                return None;
            }
            let deriv = self.get_derivation(current_id)?;
            cumulative_offset += deriv.offset;
            if let Some(rid) = deriv.source_region {
                let region = self.get_region(rid)?;
                return Some((region.base_addr as i64 + cumulative_offset) as u64);
            }
            if let Some(did) = deriv.source_derivation {
                current_id = did;
            } else {
                return None;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Cleanup-specific: operation queries
    // -----------------------------------------------------------------------

    /// Return all operations that act on the given region.
    pub fn ops_for_region(&self, region: RegionId) -> Vec<&ProofMemOp> {
        self.ops.iter().filter(|op| op.region == region).collect()
    }

    /// Return all alloc operations.
    pub fn allocs(&self) -> Vec<&ProofMemOp> {
        self.ops
            .iter()
            .filter(|op| op.kind == ProofMemOpKind::Alloc)
            .collect()
    }

    /// Return all free operations.
    pub fn frees(&self) -> Vec<&ProofMemOp> {
        self.ops
            .iter()
            .filter(|op| op.kind == ProofMemOpKind::Free)
            .collect()
    }

    /// Return all read operations.
    pub fn reads(&self) -> Vec<&ProofMemOp> {
        self.ops
            .iter()
            .filter(|op| op.kind == ProofMemOpKind::Read)
            .collect()
    }

    /// Return all write operations.
    pub fn writes(&self) -> Vec<&ProofMemOp> {
        self.ops
            .iter()
            .filter(|op| op.kind == ProofMemOpKind::Write)
            .collect()
    }

    /// Return the set of all regions mentioned in the MSG.
    pub fn all_regions(&self) -> HashSet<RegionId> {
        self.ops.iter().map(|op| op.region).collect()
    }

    /// For a given region, return the program points where it is freed.
    pub fn free_points(&self, region: RegionId) -> Vec<ProgramPoint> {
        self.ops
            .iter()
            .filter(|op| op.region == region && op.kind == ProofMemOpKind::Free)
            .map(|op| op.location)
            .collect()
    }

    /// For a given region, return the program points where it is allocated.
    pub fn alloc_points(&self, region: RegionId) -> Vec<ProgramPoint> {
        self.ops
            .iter()
            .filter(|op| op.region == region && op.kind == ProofMemOpKind::Alloc)
            .map(|op| op.location)
            .collect()
    }
}
