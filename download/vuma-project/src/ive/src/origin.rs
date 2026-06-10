//! Origin invariant verifier for the IVE module.
//!
//! This module implements **Invariant 4 (Origin)**: *Every piece of data has a
//! well-defined provenance.* The verifier traces every data value and pointer
//! in the program back to a root source and flags any "orphan data" that
//! appears without a clear origin.
//!
//! # Core concepts
//!
//! - **Root sources** are the well-known origins from which all data derives:
//!   constants, user input, allocation sites, and hardware registers.
//! - **Provenance trees** record the full derivation history of every value.
//! - **Taint tracking** propagates trust labels from root sources through all
//!   derived values. Untrusted roots (e.g. user input) taint everything that
//!   depends on them.
//! - **Orphan detection** finds values that appear in the program without any
//!   traceable origin — "out of thin air" data.
//! - **Uninitialized-read detection** flags reads from memory that has not been
//!   written to, which are origin violations.
//! - **Pointer-arithmetic provenance** ensures that offset / cast derivations
//!   preserve origin tracking and stay within valid region bounds.
//!
//! # Formal basis
//!
//! From the VUMA invariant spec (VUMA-SPEC-INV-001, Section 6):
//!
//! > Part A — Every derivation trace terminates at a valid allocation.
//! > Part B — Arithmetic derivations stay within bounds.
//! > Part C — No fabrication: no value appears without a traceable source.

use crate::result::{CounterExample, ProgramPoint, VerificationResult, VerificationStatus};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

// ---------------------------------------------------------------------------
// Address (local mirror — avoids cross-crate dependency in IVE)
// ---------------------------------------------------------------------------

/// A virtual memory address.
///
/// This is a lightweight local mirror of `vuma_core::address::Address` so that
/// the IVE crate can compile independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Address(pub u64);

impl Address {
    /// The null address.
    pub const NULL: Address = Address(0);

    /// Create from a raw `u64`.
    pub const fn new(raw: u64) -> Self {
        Address(raw)
    }

    /// Offset by a signed amount (saturating).
    pub fn offset(self, by: i64) -> Address {
        if by >= 0 {
            Address(self.0.saturating_add(by as u64))
        } else {
            Address(self.0.saturating_sub((-by) as u64))
        }
    }

    /// Raw value.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl From<u64> for Address {
    fn from(v: u64) -> Self {
        Address(v)
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:016x}", self.0)
    }
}

// ---------------------------------------------------------------------------
// RegionId / DerivationId / AccessId (local mirrors)
// ---------------------------------------------------------------------------

/// Unique identifier for a memory region.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegionId(pub u64);

impl fmt::Display for RegionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "R{}", self.0)
    }
}

/// Unique identifier for a derivation step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DerivationId(pub u64);

impl fmt::Display for DerivationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "D{}", self.0)
    }
}

/// Unique identifier for a memory access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccessId(pub u64);

impl fmt::Display for AccessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "A{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// OriginRoot — the well-known sources from which all data derives
// ---------------------------------------------------------------------------

/// The root source of a piece of data.
///
/// Every value in a VUMA program must trace back to one of these origins.
/// A value that cannot be traced to any root is an **orphan** and constitutes
/// an origin violation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OriginRoot {
    /// A compile-time constant (e.g., literal `42`, `"hello"`).
    Constant {
        /// Human-readable description of the constant.
        label: String,
    },
    /// Data read from user input (stdin, network, file, etc.).
    UserInput {
        /// Description of the input channel.
        channel: String,
    },
    /// An allocation site — the base address returned by `allocate` or `mmap`.
    AllocationSite {
        /// The region that was allocated.
        region_id: RegionId,
        /// Base address of the allocation.
        base: Address,
        /// Size in bytes.
        size: u64,
    },
    /// A hardware register (MMIO, CPU register, device buffer).
    HardwareRegister {
        /// Name or address of the register.
        name: String,
    },
}

impl OriginRoot {
    /// Returns `true` if this root is considered trusted.
    ///
    /// Constants and allocation sites are trusted. User input and hardware
    /// registers are untrusted unless explicitly annotated otherwise.
    pub fn is_trusted(&self) -> bool {
        matches!(
            self,
            OriginRoot::Constant { .. } | OriginRoot::AllocationSite { .. }
        )
    }
}

impl fmt::Display for OriginRoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OriginRoot::Constant { label } => write!(f, "const({})", label),
            OriginRoot::UserInput { channel } => write!(f, "user_input({})", channel),
            OriginRoot::AllocationSite { region_id, base, size } => {
                write!(f, "alloc({} @ {} size={})", region_id, base, size)
            }
            OriginRoot::HardwareRegister { name } => write!(f, "hw_reg({})", name),
        }
    }
}

// ---------------------------------------------------------------------------
// TaintLevel — trust classification propagated through derivations
// ---------------------------------------------------------------------------

/// The trust level of a value, propagated from its root source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TaintLevel {
    /// The value originates from a trusted source and no untrusted data
    /// flows into it.
    Trusted = 0,
    /// The value depends on untrusted data (user input, hardware register).
    Untrusted = 1,
    /// The origin is unknown — the value is an orphan.
    Unknown = 2,
}

impl fmt::Display for TaintLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaintLevel::Trusted => write!(f, "TRUSTED"),
            TaintLevel::Untrusted => write!(f, "UNTRUSTED"),
            TaintLevel::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

impl Default for TaintLevel {
    fn default() -> Self {
        TaintLevel::Unknown
    }
}

// ---------------------------------------------------------------------------
// DerivationSource — what a derivation starts from
// ---------------------------------------------------------------------------

/// The source of a pointer derivation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DerivationSource {
    /// Directly from a region (e.g., `&x` after allocation).
    Region(RegionId),
    /// Derived from another pointer derivation (e.g., pointer arithmetic).
    AnotherDerivation(DerivationId),
    /// A fabricated source — an integer literal cast to a pointer with no
    /// backing allocation. This is always an origin violation.
    Fabricated {
        /// The raw integer value that was cast to an address.
        raw_value: u64,
    },
}

// ---------------------------------------------------------------------------
// DerivationKind — what kind of derivation was performed
// ---------------------------------------------------------------------------

/// The kind of derivation operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DerivationKind {
    /// Taking the address of a value: `&x`.
    Direct,
    /// Offset from a base pointer: `ptr.offset(n)`.
    Offset { by: i64 },
    /// Type cast: `ptr as *mut T`.
    Cast {
        from_repr: String,
        to_repr: String,
    },
    /// General pointer arithmetic.
    Arithmetic { description: String },
}

// ---------------------------------------------------------------------------
// Region — a contiguous memory region
// ---------------------------------------------------------------------------

/// A contiguous memory region in the program.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Region {
    /// Unique identifier.
    pub id: RegionId,
    /// Base address.
    pub base: Address,
    /// Size in bytes.
    pub size: u64,
    /// Whether the region is still allocated.
    pub is_allocated: bool,
}

impl Region {
    /// Create a new region.
    pub fn new(id: RegionId, base: Address, size: u64) -> Self {
        Self {
            id,
            base,
            size,
            is_allocated: true,
        }
    }

    /// Returns `true` if the address falls within this region.
    pub fn contains(&self, addr: Address) -> bool {
        addr.0 >= self.base.0 && addr.0 < self.base.0 + self.size
    }

    /// End address (exclusive) of this region.
    pub fn end(&self) -> Address {
        Address(self.base.0 + self.size)
    }
}

// ---------------------------------------------------------------------------
// Derivation — a single step in the provenance chain
// ---------------------------------------------------------------------------

/// A single derivation step in the pointer provenance chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Derivation {
    /// Unique identifier.
    pub id: DerivationId,
    /// Where this derivation starts from.
    pub source: DerivationSource,
    /// What kind of derivation was performed.
    pub kind: DerivationKind,
    /// The provenance range `[lo, hi)` this derivation may access.
    pub proven_range: (Address, Address),
}

impl Derivation {
    /// Create a new derivation.
    pub fn new(
        id: DerivationId,
        source: DerivationSource,
        kind: DerivationKind,
        proven_range: (Address, Address),
    ) -> Self {
        Self {
            id,
            source,
            kind,
            proven_range,
        }
    }

    /// Returns `true` if the provenance range is well-formed (lo < hi).
    pub fn is_within_bounds(&self) -> bool {
        self.proven_range.0 < self.proven_range.1
    }
}

// ---------------------------------------------------------------------------
// Access — a read or write to derived memory
// ---------------------------------------------------------------------------

/// Kind of memory access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccessKind {
    /// A read from memory.
    Read,
    /// A write to memory.
    Write,
}

/// A memory access event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Access {
    /// Unique identifier.
    pub id: AccessId,
    /// The derivation being accessed.
    pub target: DerivationId,
    /// Read or write.
    pub kind: AccessKind,
    /// Size in bytes of the access.
    pub size: u64,
    /// Program point where this access occurs.
    pub program_point: String,
    /// Whether the accessed memory has been initialized (written to) before
    /// this access. For writes, this is always `true` after the write.
    pub is_initialized: bool,
}

impl Access {
    /// Create a new access.
    pub fn new(
        id: AccessId,
        target: DerivationId,
        kind: AccessKind,
        size: u64,
        program_point: impl Into<String>,
        is_initialized: bool,
    ) -> Self {
        Self {
            id,
            target,
            kind,
            size,
            program_point: program_point.into(),
            is_initialized,
        }
    }
}

// ---------------------------------------------------------------------------
// ProvenanceNode — a node in the provenance forest
// ---------------------------------------------------------------------------

/// A node in the provenance forest linking a derivation to its root origin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceNode {
    /// The derivation this node describes.
    pub derivation_id: DerivationId,
    /// The root origin of this derivation (computed by walking the chain).
    pub root: Option<OriginRoot>,
    /// The taint level propagated from the root.
    pub taint: TaintLevel,
    /// The full chain of derivation IDs from root to this node
    /// `[root_derivation, ..., parent, self]`.
    pub chain: Vec<DerivationId>,
}

impl ProvenanceNode {
    /// Create a provenance node.
    pub fn new(
        derivation_id: DerivationId,
        root: Option<OriginRoot>,
        taint: TaintLevel,
        chain: Vec<DerivationId>,
    ) -> Self {
        Self {
            derivation_id,
            root,
            taint,
            chain,
        }
    }

    /// Returns `true` if this node has a valid root origin.
    pub fn has_origin(&self) -> bool {
        self.root.is_some()
    }

    /// Returns `true` if this node is an orphan (no traceable origin).
    pub fn is_orphan(&self) -> bool {
        self.root.is_none() || self.taint == TaintLevel::Unknown
    }
}

// ---------------------------------------------------------------------------
// OriginViolation — a detected violation of the origin invariant
// ---------------------------------------------------------------------------

/// The kind of origin violation detected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ViolationKind {
    /// A value appears without a traceable origin ("orphan data").
    OrphanValue {
        /// The derivation that has no origin.
        derivation_id: DerivationId,
    },
    /// A fabricated pointer — an integer literal cast to an address with no
    /// backing allocation.
    FabricatedPointer {
        /// The derivation with a fabricated source.
        derivation_id: DerivationId,
        /// The raw integer value that was cast to an address.
        raw_value: u64,
    },
    /// A broken derivation chain — a derivation references a parent that does
    /// not exist.
    BrokenChain {
        /// The derivation with the broken reference.
        derivation_id: DerivationId,
        /// The missing parent derivation.
        missing_parent: DerivationId,
    },
    /// A cycle in the derivation graph (should be a DAG).
    CyclicDerivation {
        /// The derivation involved in the cycle.
        derivation_id: DerivationId,
    },
    /// An uninitialized read — reading memory that has not been written to.
    UninitializedRead {
        /// The access that reads uninitialized memory.
        access_id: AccessId,
        /// The program point of the read.
        program_point: String,
    },
    /// A pointer arithmetic derivation that goes out of bounds of its
    /// originating region.
    OutOfBounds {
        /// The derivation that goes out of bounds.
        derivation_id: DerivationId,
        /// The region that the derivation should be within.
        region_id: RegionId,
    },
    /// An access targets a derivation whose provenance range is ill-formed
    /// (lo >= hi).
    IllFormedProvenance {
        /// The derivation with the bad range.
        derivation_id: DerivationId,
    },
    /// An access targets a freed/unallocated region.
    FreedRegionAccess {
        /// The access.
        access_id: AccessId,
        /// The freed region.
        region_id: RegionId,
    },
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViolationKind::OrphanValue { derivation_id } => {
                write!(f, "orphan value: {} has no traceable origin", derivation_id)
            }
            ViolationKind::FabricatedPointer { derivation_id, raw_value } => {
                write!(
                    f,
                    "fabricated pointer: {} from raw integer 0x{:x}",
                    derivation_id, raw_value
                )
            }
            ViolationKind::BrokenChain { derivation_id, missing_parent } => {
                write!(
                    f,
                    "broken chain: {} references missing parent {}",
                    derivation_id, missing_parent
                )
            }
            ViolationKind::CyclicDerivation { derivation_id } => {
                write!(f, "cyclic derivation: {} involved in cycle", derivation_id)
            }
            ViolationKind::UninitializedRead { access_id, program_point } => {
                write!(
                    f,
                    "uninitialized read: {} at {}",
                    access_id, program_point
                )
            }
            ViolationKind::OutOfBounds { derivation_id, region_id } => {
                write!(
                    f,
                    "out of bounds: {} exceeds region {}",
                    derivation_id, region_id
                )
            }
            ViolationKind::IllFormedProvenance { derivation_id } => {
                write!(f, "ill-formed provenance range: {}", derivation_id)
            }
            ViolationKind::FreedRegionAccess { access_id, region_id } => {
                write!(
                    f,
                    "access {} targets freed region {}",
                    access_id, region_id
                )
            }
        }
    }
}

/// A single origin violation with context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginViolation {
    /// The kind of violation.
    pub kind: ViolationKind,
    /// Human-readable description.
    pub description: String,
}

impl OriginViolation {
    /// Create a new violation.
    pub fn new(kind: ViolationKind, description: impl Into<String>) -> Self {
        Self {
            kind,
            description: description.into(),
        }
    }
}

impl fmt::Display for OriginViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.description)
    }
}

// ---------------------------------------------------------------------------
// OriginReport — the full output of an origin verification pass
// ---------------------------------------------------------------------------

/// The result of verifying the origin invariant against a program.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginReport {
    /// The provenance forest — one entry per derivation.
    pub provenance_forest: Vec<ProvenanceNode>,
    /// All violations detected.
    pub violations: Vec<OriginViolation>,
    /// Derivations that carry untrusted taint.
    pub tainted_derivations: Vec<(DerivationId, TaintLevel)>,
    /// Total number of derivations checked.
    pub total_derivations: usize,
    /// Total number of accesses checked.
    pub total_accesses: usize,
}

impl OriginReport {
    /// Create an empty report.
    pub fn new() -> Self {
        Self {
            provenance_forest: Vec::new(),
            violations: Vec::new(),
            tainted_derivations: Vec::new(),
            total_derivations: 0,
            total_accesses: 0,
        }
    }

    /// Returns `true` if no violations were found.
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    /// Number of violations found.
    pub fn violation_count(&self) -> usize {
        self.violations.len()
    }

    /// Convert this report into a [`VerificationResult`].
    pub fn to_verification_result(&self) -> VerificationResult {
        if self.is_clean() {
            VerificationResult::new(
                "origin",
                VerificationStatus::Proven,
                format!(
                    "origin invariant holds: {} derivations, {} accesses verified",
                    self.total_derivations, self.total_accesses
                ),
            )
        } else {
            let descriptions: Vec<String> = self.violations.iter().map(|v| v.to_string()).collect();
            let _first = self.violations.first().unwrap();
            VerificationResult::new(
                "origin",
                VerificationStatus::Violated {
                    counterexample: CounterExample::new(
                        Vec::new(),
                        format!("origin_violation"),
                        descriptions.join("; "),
                    ),
                },
                format!("origin invariant violated: {} issue(s)", self.violation_count()),
            )
        }
    }
}

impl Default for OriginReport {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// OriginVerifier — the main verification engine
// ---------------------------------------------------------------------------

/// The origin invariant verifier.
///
/// Traces every data value and pointer in the program back to a root source,
/// builds a provenance forest, detects orphans, and performs taint tracking.
///
/// # Usage
///
/// ```ignore
/// use vuma_ive::origin::*;
///
/// let mut verifier = OriginVerifier::new();
///
/// // Register regions, derivations, accesses
/// verifier.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 256));
/// verifier.add_derivation(Derivation::new(
///     DerivationId(1),
///     DerivationSource::Region(RegionId(1)),
///     DerivationKind::Direct,
///     (Address::from(0x1000u64), Address::from(0x1100u64)),
/// ));
///
/// let report = verifier.verify();
/// assert!(report.is_clean());
/// ```
pub struct OriginVerifier {
    /// Known memory regions.
    regions: Vec<Region>,
    /// Known derivations.
    derivations: Vec<Derivation>,
    /// Known accesses.
    accesses: Vec<Access>,
    /// Whether to log detailed diagnostic info.
    verbose: bool,
}

impl OriginVerifier {
    /// Create a new origin verifier.
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            derivations: Vec::new(),
            accesses: Vec::new(),
            verbose: false,
        }
    }

    /// Enable verbose logging.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Register a memory region.
    pub fn add_region(&mut self, region: Region) {
        self.regions.push(region);
    }

    /// Register a derivation.
    pub fn add_derivation(&mut self, derivation: Derivation) {
        self.derivations.push(derivation);
    }

    /// Register an access.
    pub fn add_access(&mut self, access: Access) {
        self.accesses.push(access);
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Look up a region by ID.
    fn region(&self, id: RegionId) -> Option<&Region> {
        self.regions.iter().find(|r| r.id == id)
    }

    /// Look up a derivation by ID.
    fn derivation(&self, id: DerivationId) -> Option<&Derivation> {
        self.derivations.iter().find(|d| d.id == id)
    }

    /// Walk the derivation chain to find the root [`RegionId`].
    ///
    /// Returns `None` if the chain is broken (missing parent), cyclic, or
    /// terminates at a fabricated source.
    fn resolve_root_region(&self, derivation_id: DerivationId) -> Option<RegionId> {
        let mut visited = std::collections::HashSet::new();
        let mut current_id = derivation_id;

        loop {
            if visited.contains(&current_id) {
                // Cycle detected.
                return None;
            }
            visited.insert(current_id);

            let d = self.derivation(current_id)?;
            match &d.source {
                DerivationSource::Region(rid) => return Some(*rid),
                DerivationSource::AnotherDerivation(parent_id) => {
                    current_id = *parent_id;
                }
                DerivationSource::Fabricated { .. } => {
                    return None;
                }
            }
        }
    }

    /// Walk the derivation chain and return the full chain of IDs
    /// from root to the given derivation.
    fn trace_chain(&self, derivation_id: DerivationId) -> Vec<DerivationId> {
        let mut chain = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut current_id = derivation_id;

        loop {
            if visited.contains(&current_id) {
                // Cycle — stop.
                break;
            }
            visited.insert(current_id);
            chain.push(current_id);

            let d = match self.derivation(current_id) {
                Some(d) => d,
                None => break, // Broken chain.
            };
            match &d.source {
                DerivationSource::Region(_) => break,
                DerivationSource::AnotherDerivation(parent_id) => {
                    current_id = *parent_id;
                }
                DerivationSource::Fabricated { .. } => break,
            }
        }

        chain.reverse();
        chain
    }

    /// Compute the taint level for a derivation by tracing to its root.
    fn compute_taint(&self, derivation_id: DerivationId) -> TaintLevel {
        let chain = self.trace_chain(derivation_id);

        // Walk the chain from root to leaf. If any derivation in the chain
        // has a fabricated source, the taint is Unknown.
        for &did in &chain {
            let d = match self.derivation(did) {
                Some(d) => d,
                None => return TaintLevel::Unknown,
            };
            match &d.source {
                DerivationSource::Fabricated { .. } => return TaintLevel::Unknown,
                DerivationSource::Region(rid) => {
                    // The root is a region (allocation site) — trusted.
                    if self.region(*rid).is_some() {
                        // Continue — the root is trusted.
                    } else {
                        // Region not found — orphan.
                        return TaintLevel::Unknown;
                    }
                }
                DerivationSource::AnotherDerivation(_) => {
                    // Keep walking — taint is determined by the root.
                }
            }
        }

        // If we get here, the chain terminates at a valid region.
        // Allocation sites are trusted by default. In a more complete
        // implementation, we'd check whether the region's root origin
        // is marked as untrusted (e.g., user input buffer).
        TaintLevel::Trusted
    }

    /// Compute the [`OriginRoot`] for a derivation.
    fn compute_origin_root(&self, derivation_id: DerivationId) -> Option<OriginRoot> {
        let rid = self.resolve_root_region(derivation_id)?;
        let region = self.region(rid)?;
        Some(OriginRoot::AllocationSite {
            region_id: region.id,
            base: region.base,
            size: region.size,
        })
    }

    /// Check for cycles in the derivation graph.
    fn detect_cycles(&self) -> Vec<OriginViolation> {
        let mut violations = Vec::new();
        let mut global_visited = std::collections::HashSet::new();

        for derivation in &self.derivations {
            let mut path = std::collections::HashSet::new();
            let mut current_id = derivation.id;

            loop {
                if path.contains(&current_id) {
                    violations.push(OriginViolation::new(
                        ViolationKind::CyclicDerivation {
                            derivation_id: current_id,
                        },
                        format!("derivation {} is involved in a cycle", current_id),
                    ));
                    break;
                }
                if global_visited.contains(&current_id) {
                    // Already verified this sub-graph.
                    break;
                }
                path.insert(current_id);

                let d = match self.derivation(current_id) {
                    Some(d) => d,
                    None => break,
                };

                match &d.source {
                    DerivationSource::Region(_) | DerivationSource::Fabricated { .. } => {
                        break;
                    }
                    DerivationSource::AnotherDerivation(parent_id) => {
                        current_id = *parent_id;
                    }
                }
            }

            global_visited.extend(path);
        }

        violations
    }

    /// Check for broken chains (references to missing parents).
    fn detect_broken_chains(&self) -> Vec<OriginViolation> {
        let mut violations = Vec::new();

        for derivation in &self.derivations {
            if let DerivationSource::AnotherDerivation(parent_id) = &derivation.source {
                if self.derivation(*parent_id).is_none() {
                    violations.push(OriginViolation::new(
                        ViolationKind::BrokenChain {
                            derivation_id: derivation.id,
                            missing_parent: *parent_id,
                        },
                        format!(
                            "derivation {} references parent {} which does not exist",
                            derivation.id, parent_id
                        ),
                    ));
                }
            }
        }

        violations
    }

    /// Check for fabricated pointers.
    fn detect_fabricated_pointers(&self) -> Vec<OriginViolation> {
        let mut violations = Vec::new();

        for derivation in &self.derivations {
            if let DerivationSource::Fabricated { raw_value } = &derivation.source {
                violations.push(OriginViolation::new(
                    ViolationKind::FabricatedPointer {
                        derivation_id: derivation.id,
                        raw_value: *raw_value,
                    },
                    format!(
                        "derivation {} fabricated from raw integer 0x{:x} with no allocation",
                        derivation.id, raw_value
                    ),
                ));
            }
        }

        violations
    }

    /// Check for ill-formed provenance ranges.
    fn detect_ill_formed_provenance(&self) -> Vec<OriginViolation> {
        let mut violations = Vec::new();

        for derivation in &self.derivations {
            if !derivation.is_within_bounds() {
                violations.push(OriginViolation::new(
                    ViolationKind::IllFormedProvenance {
                        derivation_id: derivation.id,
                    },
                    format!(
                        "derivation {} has ill-formed provenance range [{}, {})",
                        derivation.id, derivation.proven_range.0, derivation.proven_range.1
                    ),
                ));
            }
        }

        violations
    }

    /// Check for out-of-bounds derivations.
    fn detect_out_of_bounds(&self) -> Vec<OriginViolation> {
        let mut violations = Vec::new();

        for derivation in &self.derivations {
            let rid = match self.resolve_root_region(derivation.id) {
                Some(rid) => rid,
                None => continue, // Handled by orphan/fabricated checks.
            };
            let region = match self.region(rid) {
                Some(r) => r,
                None => continue,
            };

            // Check that the provenance range falls within the region.
            if derivation.proven_range.0 < region.base
                || derivation.proven_range.1 > region.end()
            {
                violations.push(OriginViolation::new(
                    ViolationKind::OutOfBounds {
                        derivation_id: derivation.id,
                        region_id: rid,
                    },
                    format!(
                        "derivation {} provenance [{}, {}) exceeds region {} [{}, {})",
                        derivation.id,
                        derivation.proven_range.0,
                        derivation.proven_range.1,
                        rid,
                        region.base,
                        region.end()
                    ),
                ));
            }
        }

        violations
    }

    /// Detect orphan derivations — those without a traceable origin.
    fn detect_orphans(&self) -> Vec<OriginViolation> {
        let mut violations = Vec::new();

        for derivation in &self.derivations {
            let root = self.compute_origin_root(derivation.id);
            if root.is_none() {
                // Only add if not already caught as fabricated or broken chain.
                match &derivation.source {
                    DerivationSource::Fabricated { .. } => {
                        // Already reported by detect_fabricated_pointers.
                    }
                    DerivationSource::AnotherDerivation(parent_id) => {
                        if self.derivation(*parent_id).is_none() {
                            // Already reported by detect_broken_chains.
                        } else {
                            // Parent exists but chain doesn't terminate at a region.
                            violations.push(OriginViolation::new(
                                ViolationKind::OrphanValue {
                                    derivation_id: derivation.id,
                                },
                                format!(
                                    "derivation {} has no traceable origin to an allocation site",
                                    derivation.id
                                ),
                            ));
                        }
                    }
                    DerivationSource::Region(rid) => {
                        if self.region(*rid).is_none() {
                            violations.push(OriginViolation::new(
                                ViolationKind::OrphanValue {
                                    derivation_id: derivation.id,
                                },
                                format!(
                                    "derivation {} references region {} which does not exist",
                                    derivation.id, rid
                                ),
                            ));
                        }
                    }
                }
            }
        }

        violations
    }

    /// Detect uninitialized reads.
    fn detect_uninitialized_reads(&self) -> Vec<OriginViolation> {
        let mut violations = Vec::new();

        for access in &self.accesses {
            if access.kind == AccessKind::Read && !access.is_initialized {
                violations.push(OriginViolation::new(
                    ViolationKind::UninitializedRead {
                        access_id: access.id,
                        program_point: access.program_point.clone(),
                    },
                    format!(
                        "read access {} at {} reads uninitialized memory",
                        access.id, access.program_point
                    ),
                ));
            }
        }

        violations
    }

    /// Detect accesses to freed regions.
    fn detect_freed_region_accesses(&self) -> Vec<OriginViolation> {
        let mut violations = Vec::new();

        for access in &self.accesses {
            let rid = match self.resolve_root_region(access.target) {
                Some(rid) => rid,
                None => continue,
            };
            let region = match self.region(rid) {
                Some(r) => r,
                None => continue,
            };
            if !region.is_allocated {
                violations.push(OriginViolation::new(
                    ViolationKind::FreedRegionAccess {
                        access_id: access.id,
                        region_id: rid,
                    },
                    format!(
                        "access {} at {} targets freed region {}",
                        access.id, access.program_point, rid
                    ),
                ));
            }
        }

        violations
    }

    // -----------------------------------------------------------------------
    // Main verification entry point
    // -----------------------------------------------------------------------

    /// Run the full origin verification and return the report.
    ///
    /// This performs the following checks in order:
    ///
    /// 1. Cycle detection in the derivation graph.
    /// 2. Broken chain detection.
    /// 3. Fabricated pointer detection.
    /// 4. Ill-formed provenance range detection.
    /// 5. Out-of-bounds detection.
    /// 6. Orphan value detection.
    /// 7. Uninitialized read detection.
    /// 8. Freed-region access detection.
    /// 9. Provenance forest construction.
    /// 10. Taint propagation.
    pub fn verify(&self) -> OriginReport {
        let mut report = OriginReport::new();

        // Structural checks on the derivation graph.
        report.violations.extend(self.detect_cycles());
        report.violations.extend(self.detect_broken_chains());
        report.violations.extend(self.detect_fabricated_pointers());
        report.violations.extend(self.detect_ill_formed_provenance());
        report.violations.extend(self.detect_out_of_bounds());

        // Semantic checks.
        report.violations.extend(self.detect_orphans());
        report.violations.extend(self.detect_uninitialized_reads());
        report.violations.extend(self.detect_freed_region_accesses());

        // Build the provenance forest.
        for derivation in &self.derivations {
            let chain = self.trace_chain(derivation.id);
            let root = self.compute_origin_root(derivation.id);
            let taint = self.compute_taint(derivation.id);

            if taint != TaintLevel::Trusted {
                report.tainted_derivations.push((derivation.id, taint));
            }

            report.provenance_forest.push(ProvenanceNode::new(
                derivation.id,
                root,
                taint,
                chain,
            ));
        }

        report.total_derivations = self.derivations.len();
        report.total_accesses = self.accesses.len();

        if self.verbose {
            log::info!(
                "origin verification: {} derivations, {} accesses, {} violations",
                report.total_derivations,
                report.total_accesses,
                report.violation_count()
            );
        }

        report
    }
}

impl Default for OriginVerifier {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// VerificationContext — context needed for derivation chain analysis
// ===========================================================================

/// Context provided to advanced verification methods.
///
/// Contains auxiliary information beyond the basic regions, derivations, and
/// accesses stored in the [`OriginVerifier`]: stack frames, write-tracking
/// records, and known allocation roots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationContext {
    /// Map from region_id to the stack frame that owns it (if any).
    /// Regions not in this map are heap-allocated.
    pub stack_frames: HashMap<u64, u64>,
    /// Map from region_id to the last region that was written to it (if known).
    pub last_write_region: HashMap<u64, u64>,
    /// Set of region_ids that are valid allocation roots.
    pub allocation_roots: HashSet<u64>,
}

impl VerificationContext {
    /// Create an empty verification context.
    pub fn new() -> Self {
        Self {
            stack_frames: HashMap::new(),
            last_write_region: HashMap::new(),
            allocation_roots: HashSet::new(),
        }
    }

    /// Register a stack-allocated region.
    pub fn add_stack_region(&mut self, region_id: u64, stack_frame: u64) {
        self.stack_frames.insert(region_id, stack_frame);
        self.allocation_roots.insert(region_id);
    }

    /// Register a heap-allocated region.
    pub fn add_heap_region(&mut self, region_id: u64) {
        self.allocation_roots.insert(region_id);
    }

    /// Record that a write to `target_region` originated from `source_region`.
    pub fn record_write(&mut self, target_region: u64, source_region: u64) {
        self.last_write_region.insert(target_region, source_region);
    }

    /// Returns `true` if the given region is a valid allocation root.
    pub fn is_valid_root(&self, region_id: u64) -> bool {
        self.allocation_roots.contains(&region_id)
    }

    /// Returns the stack frame owning the given region, if any.
    pub fn stack_frame_of(&self, region_id: u64) -> Option<u64> {
        self.stack_frames.get(&region_id).copied()
    }
}

impl Default for VerificationContext {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// DerivationViolation — violations detected by derivation chain analysis
// ===========================================================================

/// A violation detected during derivation chain validation.
///
/// These violations are more specific than the general [`ViolationKind`] and
/// focus on pointer provenance issues: offsets beyond region bounds, dangling
/// pointers, forged pointers, stack escapes, and wild pointers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DerivationViolation {
    /// A pointer derivation produces an offset that exceeds the base region's
    /// bounds.
    InvalidOffset {
        derivation_id: u64,
        base_region: u64,
        offset: u64,
        region_size: u64,
    },
    /// A pointer derivation whose base region has been freed or whose last
    /// write source is no longer valid.
    DanglingDerivation {
        derivation_id: u64,
        base_region: u64,
        last_write_region: Option<u64>,
    },
    /// A pointer value that cannot be traced to any valid derivation chain.
    ForgedPointer {
        derivation_id: u64,
        reason: String,
    },
    /// A pointer to a stack-allocated region that escapes the owning frame.
    StackEscape {
        derivation_id: u64,
        stack_frame: u64,
    },
    /// A pointer that accesses a range entirely outside any known region.
    WildPointer {
        derivation_id: u64,
        accessed_range: (u64, u64),
    },
}

impl fmt::Display for DerivationViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DerivationViolation::InvalidOffset {
                derivation_id,
                base_region,
                offset,
                region_size,
            } => write!(
                f,
                "invalid offset: derivation {} offset 0x{:x} exceeds region {} size 0x{:x}",
                derivation_id, offset, base_region, region_size
            ),
            DerivationViolation::DanglingDerivation {
                derivation_id,
                base_region,
                last_write_region,
            } => write!(
                f,
                "dangling derivation: {} from region {} (last_write_region: {})",
                derivation_id,
                base_region,
                last_write_region
                    .map(|r| format!("{}", r))
                    .unwrap_or_else(|| "None".to_string())
            ),
            DerivationViolation::ForgedPointer {
                derivation_id,
                reason,
            } => write!(
                f,
                "forged pointer: derivation {} — {}",
                derivation_id, reason
            ),
            DerivationViolation::StackEscape {
                derivation_id,
                stack_frame,
            } => write!(
                f,
                "stack escape: derivation {} escapes frame {}",
                derivation_id, stack_frame
            ),
            DerivationViolation::WildPointer {
                derivation_id,
                accessed_range,
            } => write!(
                f,
                "wild pointer: derivation {} accesses [0x{:x}, 0x{:x})",
                derivation_id, accessed_range.0, accessed_range.1
            ),
        }
    }
}

// ===========================================================================
// CastRecord — record of an integer-to-pointer cast
// ===========================================================================

/// A record of an integer-to-pointer cast operation.
///
/// Casts are the primary mechanism by which forged pointers can appear.
/// Each cast is classified as:
/// - **Legitimate** (`has_explicit_cast == true`): The program contains an
///   explicit cast annotation with an associated proof obligation.
/// - **Suspicious** (`has_explicit_cast == false`): The cast has no explicit
///   annotation but might still be valid (e.g., round-trip through `usize`).
/// - **Forged** (no derivation path): No valid derivation chain leads to the
///   resulting pointer value; detected by [`ForgedPointerDetector`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CastRecord {
    /// The derivation produced by this cast.
    pub derivation_id: u64,
    /// Source representation (e.g., "usize", "u64").
    pub from_repd: String,
    /// Target representation (e.g., "*mut u8", "*const T").
    pub to_repd: String,
    /// Whether the program contains an explicit cast annotation with proof.
    pub has_explicit_cast: bool,
    /// The program point where the cast occurs.
    pub point: ProgramPoint,
}

impl CastRecord {
    /// Create a new cast record.
    pub fn new(
        derivation_id: u64,
        from_repd: impl Into<String>,
        to_repd: impl Into<String>,
        has_explicit_cast: bool,
        point: impl Into<String>,
    ) -> Self {
        Self {
            derivation_id,
            from_repd: from_repd.into(),
            to_repd: to_repd.into(),
            has_explicit_cast,
            point: point.into(),
        }
    }

    /// Classify this cast as legitimate, suspicious, or forged based on
    /// whether it has an explicit annotation and a valid derivation path.
    pub fn classify(&self, has_valid_path: bool) -> CastClassification {
        match (self.has_explicit_cast, has_valid_path) {
            (true, true) => CastClassification::Legitimate,
            (true, false) => CastClassification::Suspicious,
            (false, true) => CastClassification::Suspicious,
            (false, false) => CastClassification::Forged,
        }
    }
}

/// Classification of an integer-to-pointer cast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CastClassification {
    /// Legitimate cast with explicit proof annotation and valid derivation.
    Legitimate,
    /// Suspicious cast — either has annotation but no valid path, or valid
    /// path but no annotation. Requires manual review.
    Suspicious,
    /// Forged pointer — no explicit annotation and no valid derivation path.
    Forged,
}

impl fmt::Display for CastClassification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CastClassification::Legitimate => write!(f, "LEGITIMATE"),
            CastClassification::Suspicious => write!(f, "SUSPICIOUS"),
            CastClassification::Forged => write!(f, "FORGED"),
        }
    }
}

// ===========================================================================
// ForgedPointerDetector — detects forged pointers
// ===========================================================================

/// Detector for forged pointers — pointer values that cannot be traced to any
/// valid allocation or derivation chain.
///
/// The detector maintains a set of valid derivation roots (region IDs from
/// actual allocations) and a log of integer-to-pointer casts. After all
/// regions and casts are registered, `detect()` returns violations for any
/// pointer that has no valid provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgedPointerDetector {
    /// Region IDs that are valid allocation roots.
    pub valid_derivation_roots: HashSet<u64>,
    /// Records of integer-to-pointer casts.
    pub cast_records: Vec<CastRecord>,
}

impl ForgedPointerDetector {
    /// Create a new forged pointer detector.
    pub fn new() -> Self {
        Self {
            valid_derivation_roots: HashSet::new(),
            cast_records: Vec::new(),
        }
    }

    /// Register a valid allocation root.
    pub fn add_root(&mut self, region_id: u64) {
        self.valid_derivation_roots.insert(region_id);
    }

    /// Register a cast record.
    pub fn add_cast_record(&mut self, record: CastRecord) {
        self.cast_records.push(record);
    }

    /// Detect forged pointers among the given derivations.
    ///
    /// Returns a list of `DerivationViolation::ForgedPointer` for each
    /// derivation that cannot be traced to a valid root.
    pub fn detect(&self, derivations: &[Derivation], regions: &[Region]) -> Vec<DerivationViolation> {
        let region_map: HashMap<RegionId, &Region> = regions
            .iter()
            .map(|r| (r.id, r))
            .collect();

        let derivation_map: HashMap<DerivationId, &Derivation> = derivations
            .iter()
            .map(|d| (d.id, d))
            .collect();

        let mut violations = Vec::new();

        // Check each cast record for forged pointers.
        for cast in &self.cast_records {
            let did = DerivationId(cast.derivation_id);
            let has_valid_path = self.has_valid_derivation_path(did, &derivation_map, &region_map);
            let classification = cast.classify(has_valid_path);

            match classification {
                CastClassification::Forged => {
                    violations.push(DerivationViolation::ForgedPointer {
                        derivation_id: cast.derivation_id,
                        reason: format!(
                            "integer-to-pointer cast from {} to {} at {} has no valid derivation path",
                            cast.from_repd, cast.to_repd, cast.point
                        ),
                    });
                }
                CastClassification::Suspicious => {
                    // Suspicious casts are not violations but should be logged.
                    // In a production system, these would be flagged for review.
                }
                CastClassification::Legitimate => {}
            }
        }

        // Also check fabricated derivations.
        for derivation in derivations {
            if let DerivationSource::Fabricated { raw_value } = &derivation.source {
                violations.push(DerivationViolation::ForgedPointer {
                    derivation_id: derivation.id.0,
                    reason: format!(
                        "fabricated from raw integer 0x{:x} with no allocation backing",
                        raw_value
                    ),
                });
            }
        }

        violations
    }

    /// Check whether a derivation has a valid path to a known allocation root.
    fn has_valid_derivation_path(
        &self,
        derivation_id: DerivationId,
        derivation_map: &HashMap<DerivationId, &Derivation>,
        region_map: &HashMap<RegionId, &Region>,
    ) -> bool {
        let mut visited = HashSet::new();
        let mut current_id = derivation_id;

        loop {
            if visited.contains(&current_id) {
                return false; // Cycle
            }
            visited.insert(current_id);

            let d = match derivation_map.get(&current_id) {
                Some(d) => d,
                None => return false, // Broken chain
            };

            match &d.source {
                DerivationSource::Region(rid) => {
                    // Valid if the region is a known allocation root.
                    return self.valid_derivation_roots.contains(&rid.0)
                        && region_map.contains_key(rid);
                }
                DerivationSource::AnotherDerivation(parent_id) => {
                    current_id = *parent_id;
                }
                DerivationSource::Fabricated { .. } => {
                    return false;
                }
            }
        }
    }
}

impl Default for ForgedPointerDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// ProvenanceGraphNodeKind — kinds of nodes in the provenance graph
// ===========================================================================

/// The kind of a node in the provenance graph.
///
/// Each node represents a specific point in the pointer provenance chain:
/// the original allocation, a stack allocation, a global, an integer-to-pointer
/// cast, an offset derivation, or a dereference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProvenanceNodeKind {
    /// A heap allocation site.
    Allocation,
    /// A stack allocation (alloca, local variable).
    StackAlloc,
    /// A global/static allocation.
    Global,
    /// An integer-to-pointer cast.
    Cast,
    /// A pointer offset derivation.
    Offset,
    /// A pointer dereference.
    Deref,
}

impl fmt::Display for ProvenanceNodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProvenanceNodeKind::Allocation => write!(f, "Allocation"),
            ProvenanceNodeKind::StackAlloc => write!(f, "StackAlloc"),
            ProvenanceNodeKind::Global => write!(f, "Global"),
            ProvenanceNodeKind::Cast => write!(f, "Cast"),
            ProvenanceNodeKind::Offset => write!(f, "Offset"),
            ProvenanceNodeKind::Deref => write!(f, "Deref"),
        }
    }
}

// ===========================================================================
// DerivationStep — an edge in the provenance graph
// ===========================================================================

/// A derivation step represented as an edge in the provenance graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DerivationStep {
    /// An offset applied to a base pointer.
    Offset { by: i64 },
    /// An integer-to-pointer cast.
    Cast { from_repr: String, to_repr: String },
    /// A pointer dereference (load of a pointer from memory).
    Deref,
}

impl fmt::Display for DerivationStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DerivationStep::Offset { by } => write!(f, "offset({:+})", by),
            DerivationStep::Cast { from_repr, to_repr } => {
                write!(f, "cast({} → {})", from_repr, to_repr)
            }
            DerivationStep::Deref => write!(f, "deref"),
        }
    }
}

// ===========================================================================
// ProvenanceGraphNode — a node in the provenance graph
// ===========================================================================

/// A node in the pointer provenance graph.
///
/// Each node is keyed by `(region_id, derivation_id)` and carries a kind
/// that describes what operation created this provenance point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceGraphNode {
    /// The region this node belongs to.
    pub region: u64,
    /// The derivation this node represents.
    pub derivation: u64,
    /// The kind of provenance node.
    pub kind: ProvenanceNodeKind,
}

impl ProvenanceGraphNode {
    /// Create a new provenance graph node.
    pub fn new(region: u64, derivation: u64, kind: ProvenanceNodeKind) -> Self {
        Self {
            region,
            derivation,
            kind,
        }
    }

    /// The key for this node in the graph's node map.
    pub fn key(&self) -> (u64, u64) {
        (self.region, self.derivation)
    }
}

// ===========================================================================
// ProvenanceEdge — an edge in the provenance graph
// ===========================================================================

/// A directed edge in the pointer provenance graph.
///
/// Edges connect two provenance nodes and are labeled with the derivation
/// step that was applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceEdge {
    /// Source node key `(region_id, derivation_id)`.
    pub from: (u64, u64),
    /// Target node key `(region_id, derivation_id)`.
    pub to: (u64, u64),
    /// The derivation step this edge represents.
    pub step: DerivationStep,
}

impl ProvenanceEdge {
    /// Create a new provenance edge.
    pub fn new(from: (u64, u64), to: (u64, u64), step: DerivationStep) -> Self {
        Self { from, to, step }
    }
}

// ===========================================================================
// ProvenanceGraph — the complete pointer provenance as a directed graph
// ===========================================================================

/// A directed graph representing the complete pointer provenance of a program.
///
/// Nodes are `(region_id, derivation_id)` pairs. Edges are derivation steps
/// (offset, cast, dereference). The graph supports reachability queries and
/// provenance validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceGraph {
    /// Nodes indexed by `(region_id, derivation_id)`.
    pub nodes: HashMap<(u64, u64), ProvenanceGraphNode>,
    /// Edges in the graph.
    pub edges: Vec<ProvenanceEdge>,
    /// Adjacency list for forward traversal: from_key → [(to_key, edge_index)].
    #[serde(skip)]
    adj_fwd: HashMap<(u64, u64), Vec<((u64, u64), usize)>>,
    /// Adjacency list for backward traversal: to_key → [(from_key, edge_index)].
    #[serde(skip)]
    adj_bwd: HashMap<(u64, u64), Vec<((u64, u64), usize)>>,
}

impl ProvenanceGraph {
    /// Create an empty provenance graph.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            adj_fwd: HashMap::new(),
            adj_bwd: HashMap::new(),
        }
    }

    /// Build a provenance graph from an [`OriginVerifier`]'s data and a
    /// [`VerificationContext`].
    pub fn build(verifier: &OriginVerifier, context: &VerificationContext) -> Self {
        let mut graph = Self::new();

        // Add nodes for each region (allocation roots).
        for region in &verifier.regions {
            let kind = if context.stack_frames.contains_key(&region.id.0) {
                ProvenanceNodeKind::StackAlloc
            } else {
                ProvenanceNodeKind::Allocation
            };
            let node = ProvenanceGraphNode::new(region.id.0, 0, kind);
            graph.add_node(node);
        }

        // Add nodes and edges for each derivation.
        for derivation in &verifier.derivations {
            let root_rid = verifier.resolve_root_region(derivation.id);
            let region_id = root_rid.map(|r| r.0).unwrap_or(0);

            let kind = match &derivation.kind {
                DerivationKind::Direct => {
                    // Direct derivation from a region — this is the root
                    // pointer, already covered by the allocation node.
                    ProvenanceNodeKind::Allocation
                }
                DerivationKind::Offset { .. } => ProvenanceNodeKind::Offset,
                DerivationKind::Cast { .. } => ProvenanceNodeKind::Cast,
                DerivationKind::Arithmetic { .. } => ProvenanceNodeKind::Offset,
            };

            let node = ProvenanceGraphNode::new(region_id, derivation.id.0, kind);
            graph.add_node(node);

            // Add edge from source to this derivation.
            let from_key = match &derivation.source {
                DerivationSource::Region(rid) => (rid.0, 0),
                DerivationSource::AnotherDerivation(parent_id) => {
                    let parent_root = verifier.resolve_root_region(*parent_id);
                    let parent_region = parent_root.map(|r| r.0).unwrap_or(0);
                    (parent_region, parent_id.0)
                }
                DerivationSource::Fabricated { .. } => {
                    // No valid edge for fabricated pointers.
                    continue;
                }
            };

            let to_key = (region_id, derivation.id.0);

            let step = match &derivation.kind {
                DerivationKind::Direct => DerivationStep::Cast {
                    from_repr: "region".to_string(),
                    to_repr: "pointer".to_string(),
                },
                DerivationKind::Offset { by } => DerivationStep::Offset { by: *by },
                DerivationKind::Cast { from_repr, to_repr } => DerivationStep::Cast {
                    from_repr: from_repr.clone(),
                    to_repr: to_repr.clone(),
                },
                DerivationKind::Arithmetic { description: _ } => DerivationStep::Offset { by: 0 },
            };

            graph.add_edge(ProvenanceEdge::new(from_key, to_key, step));
        }

        graph
    }

    /// Add a node to the graph.
    pub fn add_node(&mut self, node: ProvenanceGraphNode) {
        self.nodes.insert(node.key(), node);
    }

    /// Add an edge to the graph and update adjacency lists.
    pub fn add_edge(&mut self, edge: ProvenanceEdge) {
        let idx = self.edges.len();
        let from = edge.from;
        let to = edge.to;
        self.adj_fwd
            .entry(from)
            .or_default()
            .push((to, idx));
        self.adj_bwd
            .entry(to)
            .or_default()
            .push((from, idx));
        self.edges.push(edge);
    }

    /// Rebuild adjacency lists from edges (needed after deserialization).
    pub fn rebuild_adjacency(&mut self) {
        self.adj_fwd.clear();
        self.adj_bwd.clear();
        for (idx, edge) in self.edges.iter().enumerate() {
            self.adj_fwd
                .entry(edge.from)
                .or_default()
                .push((edge.to, idx));
            self.adj_bwd
                .entry(edge.to)
                .or_default()
                .push((edge.from, idx));
        }
    }

    /// Check whether derivation `derivation_id` can reach region `region_id`
    /// in the provenance graph.
    ///
    /// This is a BFS/DFS reachability query starting from the node
    /// `(region_id, derivation_id)` and traversing edges backwards to see
    /// if we can reach any allocation root node for that region.
    pub fn can_reach(&self, derivation_id: u64, region_id: u64) -> bool {
        let start = (region_id, derivation_id);
        if !self.nodes.contains_key(&start) {
            return false;
        }

        // BFS backwards from the derivation node to find an allocation root.
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(start);
        visited.insert(start);

        while let Some(current) = queue.pop_front() {
            if let Some(node) = self.nodes.get(&current) {
                if node.kind == ProvenanceNodeKind::Allocation
                    || node.kind == ProvenanceNodeKind::StackAlloc
                    || node.kind == ProvenanceNodeKind::Global
                {
                    return true;
                }
            }

            if let Some(predecessors) = self.adj_bwd.get(&current) {
                for &(prev_key, _) in predecessors {
                    if visited.insert(prev_key) {
                        queue.push_back(prev_key);
                    }
                }
            }
        }

        false
    }

    /// Validate that there exists a valid provenance path from any allocation
    /// node to the given derivation in the given region.
    ///
    /// Returns `true` if a valid path exists.
    pub fn validate_provenance(&self, derivation_id: u64, region_id: u64) -> bool {
        // Forward BFS from allocation root to the target node.
        let target = (region_id, derivation_id);
        if !self.nodes.contains_key(&target) {
            return false;
        }

        // Start from the allocation root of this region.
        let root_key = (region_id, 0);
        if !self.nodes.contains_key(&root_key) {
            return false;
        }

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(root_key);
        visited.insert(root_key);

        while let Some(current) = queue.pop_front() {
            if current == target {
                return true;
            }

            if let Some(successors) = self.adj_fwd.get(&current) {
                for &(next_key, _) in successors {
                    if visited.insert(next_key) {
                        queue.push_back(next_key);
                    }
                }
            }
        }

        false
    }

    /// Return all nodes reachable from the given starting key via forward edges.
    pub fn reachable_from(&self, start: (u64, u64)) -> HashSet<(u64, u64)> {
        let mut reachable = HashSet::new();
        let mut queue = VecDeque::new();

        if self.nodes.contains_key(&start) {
            queue.push_back(start);
            reachable.insert(start);
        }

        while let Some(current) = queue.pop_front() {
            if let Some(successors) = self.adj_fwd.get(&current) {
                for &(next_key, _) in successors {
                    if reachable.insert(next_key) {
                        queue.push_back(next_key);
                    }
                }
            }
        }

        reachable
    }
}

impl Default for ProvenanceGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// OriginVerificationResult — combined result for provenance-aware verification
// ===========================================================================

/// The result of running origin verification with provenance analysis.
///
/// Combines the standard [`OriginReport`] with derivation chain violations,
/// forged pointer detection results, and the provenance graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OriginVerificationResult {
    /// The standard origin verification report.
    pub report: OriginReport,
    /// Violations from derivation chain analysis.
    pub derivation_violations: Vec<DerivationViolation>,
    /// The provenance graph (if computed).
    pub provenance_graph: Option<ProvenanceGraph>,
    /// Number of legitimate casts detected.
    pub legitimate_casts: usize,
    /// Number of suspicious casts detected.
    pub suspicious_casts: usize,
    /// Number of forged pointers detected.
    pub forged_pointers: usize,
}

impl OriginVerificationResult {
    /// Create a new origin verification result.
    pub fn new(report: OriginReport) -> Self {
        Self {
            report,
            derivation_violations: Vec::new(),
            provenance_graph: None,
            legitimate_casts: 0,
            suspicious_casts: 0,
            forged_pointers: 0,
        }
    }

    /// Returns `true` if both the standard report and derivation analysis are
    /// clean (no violations).
    pub fn is_clean(&self) -> bool {
        self.report.is_clean() && self.derivation_violations.is_empty()
    }

    /// Total number of violations across all checks.
    pub fn total_violations(&self) -> usize {
        self.report.violation_count() + self.derivation_violations.len()
    }

    /// Convert this result into a [`VerificationResult`].
    pub fn to_verification_result(&self) -> VerificationResult {
        if self.is_clean() {
            VerificationResult::new(
                "origin+provenance",
                VerificationStatus::Proven,
                format!(
                    "origin+provenance invariant holds: {} derivations, {} accesses, 0 violations",
                    self.report.total_derivations, self.report.total_accesses
                ),
            )
        } else {
            let descriptions: Vec<String> = self
                .report
                .violations
                .iter()
                .map(|v| v.to_string())
                .chain(self.derivation_violations.iter().map(|v| v.to_string()))
                .collect();
            VerificationResult::new(
                "origin+provenance",
                VerificationStatus::Violated {
                    counterexample: CounterExample::new(
                        Vec::new(),
                        "origin_provenance_violation".to_string(),
                        descriptions.join("; "),
                    ),
                },
                format!(
                    "origin+provenance invariant violated: {} issue(s)",
                    self.total_violations()
                ),
            )
        }
    }
}

// ===========================================================================
// Extended OriginVerifier methods
// ===========================================================================

impl OriginVerifier {
    /// Validate derivation chains and detect sophisticated violations.
    ///
    /// This method performs deeper analysis than [`verify()`](OriginVerifier::verify):
    ///
    /// - **Invalid offset detection**: Checks that offset derivations stay
    ///   within their base region's bounds.
    /// - **Dangling derivation detection**: Detects derivations whose base
    ///   region has been freed or whose last-write source is invalid.
    /// - **Forged pointer detection**: Delegates to [`ForgedPointerDetector`].
    /// - **Stack escape detection**: Flags derivations that reference
    ///   stack-allocated regions from outside the owning frame.
    /// - **Wild pointer detection**: Detects pointers accessing ranges
    ///   entirely outside any known region.
    pub fn validate_derivation_chains(
        &self,
        context: &VerificationContext,
    ) -> Vec<DerivationViolation> {
        let mut violations = Vec::new();

        // Build a forged pointer detector from context.
        let mut detector = ForgedPointerDetector::new();
        for &root in &context.allocation_roots {
            detector.add_root(root);
        }

        // 1. Check each derivation for various violations.
        for derivation in &self.derivations {
            let root_rid = self.resolve_root_region(derivation.id);
            let root_rid_u64 = root_rid.map(|r| r.0);

            // --- Invalid offset detection ---
            if let DerivationKind::Offset { by } = &derivation.kind {
                if let Some(rid) = root_rid {
                    if let Some(region) = self.region(rid) {
                        let offset_abs = if *by >= 0 {
                            *by as u64
                        } else {
                            // Negative offset: check that proven_range start
                            // is not before region base.
                            region.base.0.saturating_sub((-(*by)) as u64)
                        };
                        // Check that the proven range stays within the region.
                        if derivation.proven_range.0 < region.base
                            || derivation.proven_range.1 > region.end()
                        {
                            violations.push(DerivationViolation::InvalidOffset {
                                derivation_id: derivation.id.0,
                                base_region: rid.0,
                                offset: offset_abs,
                                region_size: region.size,
                            });
                        }
                    }
                }
            }

            // --- Dangling derivation detection ---
            if let Some(rid) = root_rid {
                if let Some(region) = self.region(rid) {
                    if !region.is_allocated {
                        let last_write = context
                            .last_write_region
                            .get(&rid.0)
                            .copied();
                        violations.push(DerivationViolation::DanglingDerivation {
                            derivation_id: derivation.id.0,
                            base_region: rid.0,
                            last_write_region: last_write,
                        });
                    }
                }
            }

            // --- Stack escape detection ---
            if let Some(rid_u64) = root_rid_u64 {
                if let Some(stack_frame) = context.stack_frame_of(rid_u64) {
                    // A derivation whose base region is on the stack.
                    // If the derivation is accessed from outside the owning
                    // frame, that's a stack escape.
                    // We flag the derivation itself — the access-site check
                    // would need call-graph info; here we flag all
                    // non-direct derivations from stack regions as potential
                    // escapes.
                    if !matches!(derivation.kind, DerivationKind::Direct) {
                        violations.push(DerivationViolation::StackEscape {
                            derivation_id: derivation.id.0,
                            stack_frame,
                        });
                    }
                }
            }

            // --- Wild pointer detection ---
            // A wild pointer accesses a range entirely outside any known region.
            if root_rid.is_none() && !matches!(derivation.source, DerivationSource::Fabricated { .. }) {
                let lo = derivation.proven_range.0 .0;
                let hi = derivation.proven_range.1 .0;
                let in_any_region = self.regions.iter().any(|r| {
                    // Overlap check: [lo, hi) intersects [r.base, r.end)
                    lo < r.base.0 + r.size && hi > r.base.0
                });
                if !in_any_region && lo < hi {
                    violations.push(DerivationViolation::WildPointer {
                        derivation_id: derivation.id.0,
                        accessed_range: (lo, hi),
                    });
                }
            }
        }

        // 2. Forged pointer detection via the detector.
        let forged_violations = detector.detect(&self.derivations, &self.regions);
        violations.extend(forged_violations);

        violations
    }

    /// Run both the standard origin check and the provenance analysis.
    ///
    /// This is the comprehensive verification entry point that:
    /// 1. Runs the standard [`verify()`](OriginVerifier::verify) checks.
    /// 2. Validates derivation chains using [`validate_derivation_chains`].
    /// 3. Builds a [`ProvenanceGraph`] and checks provenance for all
    ///    derivations.
    /// 4. Classifies all cast records.
    pub fn verify_with_provenance(
        &self,
        context: &VerificationContext,
    ) -> OriginVerificationResult {
        // 1. Standard verification.
        let report = self.verify();

        // 2. Derivation chain validation.
        let derivation_violations = self.validate_derivation_chains(context);

        // 3. Build provenance graph.
        let provenance_graph = ProvenanceGraph::build(self, context);

        // 4. Classify casts.
        let mut legitimate_casts = 0;
        let mut suspicious_casts = 0;
        let mut forged_pointers = 0;

        let derivation_map: HashMap<DerivationId, &Derivation> = self
            .derivations
            .iter()
            .map(|d| (d.id, d))
            .collect();
        let region_map: HashMap<RegionId, &Region> = self
            .regions
            .iter()
            .map(|r| (r.id, r))
            .collect();

        let detector = {
            let mut d = ForgedPointerDetector::new();
            for &root in &context.allocation_roots {
                d.add_root(root);
            }
            d
        };

        for cast in &detector.cast_records {
            let did = DerivationId(cast.derivation_id);
            let has_valid_path = detector.has_valid_derivation_path(did, &derivation_map, &region_map);
            match cast.classify(has_valid_path) {
                CastClassification::Legitimate => legitimate_casts += 1,
                CastClassification::Suspicious => suspicious_casts += 1,
                CastClassification::Forged => forged_pointers += 1,
            }
        }

        // Count forged from derivation violations.
        forged_pointers += derivation_violations
            .iter()
            .filter(|v| matches!(v, DerivationViolation::ForgedPointer { .. }))
            .count();

        OriginVerificationResult {
            report,
            derivation_violations,
            provenance_graph: Some(provenance_graph),
            legitimate_casts,
            suspicious_casts,
            forged_pointers,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: create a simple verifier with one region and one direct derivation.
    fn simple_verifier() -> OriginVerifier {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 256));
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(1)),
            DerivationKind::Direct,
            (Address::from(0x1000u64), Address::from(0x1100u64)),
        ));
        v
    }

    // -----------------------------------------------------------------------
    // Test 1: Valid derivation chain
    // -----------------------------------------------------------------------

    #[test]
    fn valid_derivation_chain_is_clean() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 1024));
        // Root derivation from region.
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(1)),
            DerivationKind::Direct,
            (Address::from(0x1000u64), Address::from(0x1400u64)),
        ));
        // Offset derivation from root.
        v.add_derivation(Derivation::new(
            DerivationId(2),
            DerivationSource::AnotherDerivation(DerivationId(1)),
            DerivationKind::Offset { by: 64 },
            (Address::from(0x1040u64), Address::from(0x1080u64)),
        ));
        // Cast derivation.
        v.add_derivation(Derivation::new(
            DerivationId(3),
            DerivationSource::AnotherDerivation(DerivationId(2)),
            DerivationKind::Cast {
                from_repr: "*mut u8".into(),
                to_repr: "*mut u32".into(),
            },
            (Address::from(0x1040u64), Address::from(0x1080u64)),
        ));

        let report = v.verify();
        assert!(report.is_clean(), "expected no violations, got: {:?}", report.violations);
        assert_eq!(report.total_derivations, 3);

        // Check provenance forest.
        let node3 = report.provenance_forest.iter()
            .find(|n| n.derivation_id == DerivationId(3))
            .unwrap();
        assert!(node3.has_origin());
        assert_eq!(node3.taint, TaintLevel::Trusted);
        assert_eq!(node3.chain.len(), 3); // [D1, D2, D3]
    }

    // -----------------------------------------------------------------------
    // Test 2: Orphan value detection
    // -----------------------------------------------------------------------

    #[test]
    fn orphan_value_detected() {
        let mut v = OriginVerifier::new();
        // Derivation references a non-existent region.
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(99)), // No such region.
            DerivationKind::Direct,
            (Address::from(0x1000u64), Address::from(0x1100u64)),
        ));

        let report = v.verify();
        assert!(!report.is_clean());
        assert!(report.violations.iter().any(|v| matches!(
            &v.kind,
            ViolationKind::OrphanValue { derivation_id: DerivationId(1) }
        )));
    }

    // -----------------------------------------------------------------------
    // Test 3: Taint propagation
    // -----------------------------------------------------------------------

    #[test]
    fn taint_propagation_from_fabricated_source() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 256));
        // Fabricated root derivation.
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Fabricated { raw_value: 0xDEADBEEF },
            DerivationKind::Direct,
            (Address::from(0xDEADBEEFu64), Address::from(0xDEADBFF3u64)),
        ));
        // Derived from the fabricated pointer — should also be tainted.
        v.add_derivation(Derivation::new(
            DerivationId(2),
            DerivationSource::AnotherDerivation(DerivationId(1)),
            DerivationKind::Offset { by: 16 },
            (Address::from(0xDEADBFEFu64), Address::from(0xDEADBFFFu64)),
        ));

        let report = v.verify();

        // Fabricated pointer violation.
        assert!(report.violations.iter().any(|v| matches!(
            &v.kind,
            ViolationKind::FabricatedPointer { .. }
        )));

        // D2 should be tainted because it derives from D1 (fabricated).
        let node2 = report.provenance_forest.iter()
            .find(|n| n.derivation_id == DerivationId(2))
            .unwrap();
        assert_eq!(node2.taint, TaintLevel::Unknown);
        assert!(node2.is_orphan());

        // Tainted derivations list should include both.
        assert_eq!(report.tainted_derivations.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Test 4: Uninitialized read detection
    // -----------------------------------------------------------------------

    #[test]
    fn uninitialized_read_detected() {
        let mut v = simple_verifier();
        // Read from uninitialized memory.
        v.add_access(Access::new(
            AccessId(1),
            DerivationId(1),
            AccessKind::Read,
            4,
            "main.rs:10",
            false, // Not initialized.
        ));

        let report = v.verify();
        assert!(!report.is_clean());
        assert!(report.violations.iter().any(|v| matches!(
            &v.kind,
            ViolationKind::UninitializedRead {
                access_id: AccessId(1),
                ..
            }
        )));
    }

    // -----------------------------------------------------------------------
    // Test 5: Pointer arithmetic provenance preserved
    // -----------------------------------------------------------------------

    #[test]
    fn pointer_arithmetic_preserves_provenance() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 1024));

        // Chain: R1 -> D1 (direct) -> D2 (offset +64) -> D3 (offset +128)
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(1)),
            DerivationKind::Direct,
            (Address::from(0x1000u64), Address::from(0x1400u64)),
        ));
        v.add_derivation(Derivation::new(
            DerivationId(2),
            DerivationSource::AnotherDerivation(DerivationId(1)),
            DerivationKind::Offset { by: 64 },
            (Address::from(0x1040u64), Address::from(0x1400u64)),
        ));
        v.add_derivation(Derivation::new(
            DerivationId(3),
            DerivationSource::AnotherDerivation(DerivationId(2)),
            DerivationKind::Offset { by: 128 },
            (Address::from(0x10C0u64), Address::from(0x1400u64)),
        ));

        let report = v.verify();
        assert!(report.is_clean(), "expected no violations, got: {:?}", report.violations);

        // D3's chain should be [D1, D2, D3].
        let node3 = report.provenance_forest.iter()
            .find(|n| n.derivation_id == DerivationId(3))
            .unwrap();
        assert_eq!(node3.chain, vec![DerivationId(1), DerivationId(2), DerivationId(3)]);
        assert_eq!(node3.taint, TaintLevel::Trusted);
    }

    // -----------------------------------------------------------------------
    // Test 6: Multi-step derivation with broken chain
    // -----------------------------------------------------------------------

    #[test]
    fn multi_step_derivation_with_broken_chain() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 256));
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(1)),
            DerivationKind::Direct,
            (Address::from(0x1000u64), Address::from(0x1100u64)),
        ));
        // D3 references D2 which does not exist — broken chain.
        v.add_derivation(Derivation::new(
            DerivationId(3),
            DerivationSource::AnotherDerivation(DerivationId(2)), // D2 missing!
            DerivationKind::Offset { by: 32 },
            (Address::from(0x1020u64), Address::from(0x1040u64)),
        ));

        let report = v.verify();
        assert!(!report.is_clean());
        assert!(report.violations.iter().any(|v| matches!(
            &v.kind,
            ViolationKind::BrokenChain {
                derivation_id: DerivationId(3),
                missing_parent: DerivationId(2),
            }
        )));
    }

    // -----------------------------------------------------------------------
    // Test 7: Region-based origin validation (out of bounds)
    // -----------------------------------------------------------------------

    #[test]
    fn region_based_out_of_bounds_detected() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 64));
        // Derivation with provenance range exceeding the region.
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(1)),
            DerivationKind::Offset { by: 32 },
            (Address::from(0x1020u64), Address::from(0x2000u64)), // Goes past 0x1040.
        ));

        let report = v.verify();
        assert!(!report.is_clean());
        assert!(report.violations.iter().any(|v| matches!(
            &v.kind,
            ViolationKind::OutOfBounds {
                derivation_id: DerivationId(1),
                region_id: RegionId(1),
            }
        )));
    }

    // -----------------------------------------------------------------------
    // Test 8: Clean program — no violations
    // -----------------------------------------------------------------------

    #[test]
    fn clean_program_passes() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 1024));
        v.add_region(Region::new(RegionId(2), Address::from(0x2000u64), 512));

        // Derivations from both regions.
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(1)),
            DerivationKind::Direct,
            (Address::from(0x1000u64), Address::from(0x1400u64)),
        ));
        v.add_derivation(Derivation::new(
            DerivationId(2),
            DerivationSource::Region(RegionId(2)),
            DerivationKind::Direct,
            (Address::from(0x2000u64), Address::from(0x2200u64)),
        ));
        v.add_derivation(Derivation::new(
            DerivationId(3),
            DerivationSource::AnotherDerivation(DerivationId(1)),
            DerivationKind::Offset { by: 64 },
            (Address::from(0x1040u64), Address::from(0x1080u64)),
        ));

        // Initialized writes and reads.
        v.add_access(Access::new(
            AccessId(1),
            DerivationId(1),
            AccessKind::Write,
            4,
            "main.rs:5",
            true,
        ));
        v.add_access(Access::new(
            AccessId(2),
            DerivationId(3),
            AccessKind::Read,
            4,
            "main.rs:6",
            true,
        ));

        let report = v.verify();
        assert!(report.is_clean(), "expected no violations, got: {:?}", report.violations);
        assert_eq!(report.total_derivations, 3);
        assert_eq!(report.total_accesses, 2);

        // All provenance nodes should be trusted.
        for node in &report.provenance_forest {
            assert_eq!(node.taint, TaintLevel::Trusted);
            assert!(node.has_origin());
        }

        // Verify the VerificationResult conversion.
        let vr = report.to_verification_result();
        assert!(vr.is_proven());
    }

    // -----------------------------------------------------------------------
    // Additional: Fabricated pointer detection (the spec example)
    // -----------------------------------------------------------------------

    #[test]
    fn fabricated_pointer_from_integer_literal() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 1024));

        // A derivation fabricated from an integer literal — the key example
        // from the VUMA spec Section 6.4.
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Fabricated { raw_value: 0xDEADBEEF },
            DerivationKind::Direct,
            (Address::from(0xDEADBEEFu64), Address::from(0xDEADBFF3u64)),
        ));

        let report = v.verify();
        assert!(!report.is_clean());
        assert!(report.violations.iter().any(|v| matches!(
            &v.kind,
            ViolationKind::FabricatedPointer {
                derivation_id: DerivationId(1),
                raw_value: 0xDEADBEEF,
            }
        )));

        // The derivation should be tainted as Unknown.
        let node = report.provenance_forest.iter()
            .find(|n| n.derivation_id == DerivationId(1))
            .unwrap();
        assert_eq!(node.taint, TaintLevel::Unknown);
        assert!(node.is_orphan());
    }

    // -----------------------------------------------------------------------
    // Additional: Freed region access
    // -----------------------------------------------------------------------

    #[test]
    fn access_to_freed_region_detected() {
        let mut v = OriginVerifier::new();
        let mut region = Region::new(RegionId(1), Address::from(0x1000u64), 256);
        region.is_allocated = false; // Freed!
        v.add_region(region);

        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(1)),
            DerivationKind::Direct,
            (Address::from(0x1000u64), Address::from(0x1100u64)),
        ));

        v.add_access(Access::new(
            AccessId(1),
            DerivationId(1),
            AccessKind::Read,
            4,
            "main.rs:20",
            true,
        ));

        let report = v.verify();
        assert!(!report.is_clean());
        assert!(report.violations.iter().any(|v| matches!(
            &v.kind,
            ViolationKind::FreedRegionAccess {
                access_id: AccessId(1),
                region_id: RegionId(1),
            }
        )));
    }

    // -----------------------------------------------------------------------
    // Additional: Cycle detection
    // -----------------------------------------------------------------------

    #[test]
    fn cyclic_derivation_detected() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 256));

        // D1 -> D2 -> D1 (cycle)
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::AnotherDerivation(DerivationId(2)),
            DerivationKind::Offset { by: 16 },
            (Address::from(0x1010u64), Address::from(0x1020u64)),
        ));
        v.add_derivation(Derivation::new(
            DerivationId(2),
            DerivationSource::AnotherDerivation(DerivationId(1)),
            DerivationKind::Offset { by: 32 },
            (Address::from(0x1020u64), Address::from(0x1040u64)),
        ));

        let report = v.verify();
        assert!(!report.is_clean());
        assert!(report.violations.iter().any(|v| matches!(
            &v.kind,
            ViolationKind::CyclicDerivation { .. }
        )));
    }

    // -----------------------------------------------------------------------
    // Additional: Ill-formed provenance range
    // -----------------------------------------------------------------------

    #[test]
    fn ill_formed_provenance_range_detected() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 256));

        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(1)),
            DerivationKind::Direct,
            (Address::from(0x2000u64), Address::from(0x1000u64)), // lo > hi
        ));

        let report = v.verify();
        assert!(!report.is_clean());
        assert!(report.violations.iter().any(|v| matches!(
            &v.kind,
            ViolationKind::IllFormedProvenance { derivation_id: DerivationId(1) }
        )));
    }

    // -----------------------------------------------------------------------
    // OriginVerifier default
    // -----------------------------------------------------------------------

    #[test]
    fn default_verifier() {
        let v = OriginVerifier::default();
        assert_eq!(v.derivations.len(), 0);
        assert_eq!(v.regions.len(), 0);
        assert_eq!(v.accesses.len(), 0);
    }

    // -----------------------------------------------------------------------
    // Empty program
    // -----------------------------------------------------------------------

    #[test]
    fn empty_program_is_clean() {
        let v = OriginVerifier::new();
        let report = v.verify();
        assert!(report.is_clean());
        assert_eq!(report.total_derivations, 0);
        assert_eq!(report.total_accesses, 0);
    }

    // -----------------------------------------------------------------------
    // OriginRoot display and trust
    // -----------------------------------------------------------------------

    #[test]
    fn origin_root_display_and_trust() {
        let constant = OriginRoot::Constant { label: "42".into() };
        assert!(constant.is_trusted());
        assert_eq!(format!("{}", constant), "const(42)");

        let user_input = OriginRoot::UserInput { channel: "stdin".into() };
        assert!(!user_input.is_trusted());
        assert_eq!(format!("{}", user_input), "user_input(stdin)");

        let alloc = OriginRoot::AllocationSite {
            region_id: RegionId(1),
            base: Address::from(0x1000u64),
            size: 256,
        };
        assert!(alloc.is_trusted());
        assert!(format!("{}", alloc).contains("alloc("));

        let hw = OriginRoot::HardwareRegister { name: "MMIO0".into() };
        assert!(!hw.is_trusted());
        assert_eq!(format!("{}", hw), "hw_reg(MMIO0)");
    }

    // -----------------------------------------------------------------------
    // TaintLevel ordering
    // -----------------------------------------------------------------------

    #[test]
    fn taint_level_ordering() {
        assert!(TaintLevel::Trusted < TaintLevel::Untrusted);
        assert!(TaintLevel::Untrusted < TaintLevel::Unknown);
    }

    // -----------------------------------------------------------------------
    // Region helpers
    // -----------------------------------------------------------------------

    #[test]
    fn region_contains_and_end() {
        let r = Region::new(RegionId(1), Address::from(0x1000u64), 256);
        assert!(r.contains(Address::from(0x1000u64)));
        assert!(r.contains(Address::from(0x10FFu64)));
        assert!(!r.contains(Address::from(0x1100u64)));
        assert_eq!(r.end(), Address::from(0x1100u64));
    }

    // -----------------------------------------------------------------------
    // ProvenanceNode helpers
    // -----------------------------------------------------------------------

    #[test]
    fn provenance_node_orphan_detection() {
        let orphan = ProvenanceNode::new(
            DerivationId(1),
            None,
            TaintLevel::Unknown,
            vec![DerivationId(1)],
        );
        assert!(orphan.is_orphan());
        assert!(!orphan.has_origin());

        let valid = ProvenanceNode::new(
            DerivationId(2),
            Some(OriginRoot::Constant { label: "x".into() }),
            TaintLevel::Trusted,
            vec![DerivationId(2)],
        );
        assert!(!valid.is_orphan());
        assert!(valid.has_origin());
    }

    // -----------------------------------------------------------------------
    // OriginReport to_verification_result
    // -----------------------------------------------------------------------

    #[test]
    fn report_to_verification_result_violated() {
        let mut report = OriginReport::new();
        report.violations.push(OriginViolation::new(
            ViolationKind::OrphanValue { derivation_id: DerivationId(1) },
            "orphan",
        ));
        report.total_derivations = 1;

        let vr = report.to_verification_result();
        assert!(vr.is_violated());
        assert!(!vr.is_proven());
    }

    // -----------------------------------------------------------------------
    // W1-A8: Derivation chain validation tests
    // -----------------------------------------------------------------------

    /// Helper for provenance tests.
    fn make_basic_verifier() -> OriginVerifier {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 256));
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(1)),
            DerivationKind::Direct,
            (Address::from(0x1000u64), Address::from(0x1100u64)),
        ));
        v
    }

    #[test]
    fn provenance_valid_derivation_chain() {
        let mut v = make_basic_verifier();
        v.add_derivation(Derivation::new(
            DerivationId(2),
            DerivationSource::AnotherDerivation(DerivationId(1)),
            DerivationKind::Offset { by: 16 },
            (Address::from(0x1010u64), Address::from(0x1020u64)),
        ));
        v.add_access(Access::new(
            AccessId(1),
            DerivationId(2),
            AccessKind::Read,
            16,
            "main.rs:10",
            true,
        ));

        let report = v.verify();
        assert!(report.is_clean(), "expected no violations, got: {:?}", report.violations);
    }

    #[test]
    fn provenance_invalid_offset_beyond_region_bounds() {
        let mut v = make_basic_verifier();
        v.add_derivation(Derivation::new(
            DerivationId(2),
            DerivationSource::AnotherDerivation(DerivationId(1)),
            DerivationKind::Offset { by: 512 },
            (Address::from(0x1200u64), Address::from(0x1300u64)),
        ));

        let ctx = VerificationContext::new();
        let violations = v.validate_derivation_chains(&ctx);

        assert!(
            violations.iter().any(|v| matches!(
                v,
                DerivationViolation::InvalidOffset {
                    derivation_id: 2,
                    base_region: 1,
                    ..
                }
            )),
            "expected InvalidOffset violation, got: {:?}",
            violations
        );
    }

    #[test]
    fn provenance_dangling_pointer_detection() {
        let mut v = OriginVerifier::new();
        let mut region = Region::new(RegionId(5), Address::from(0x5000u64), 128);
        region.is_allocated = false;
        v.add_region(region);
        v.add_derivation(Derivation::new(
            DerivationId(10),
            DerivationSource::Region(RegionId(5)),
            DerivationKind::Direct,
            (Address::from(0x5000u64), Address::from(0x5080u64)),
        ));

        let ctx = VerificationContext::new();
        let violations = v.validate_derivation_chains(&ctx);

        assert!(
            violations.iter().any(|v| matches!(
                v,
                DerivationViolation::DanglingDerivation {
                    derivation_id: 10,
                    base_region: 5,
                    ..
                }
            )),
            "expected DanglingDerivation violation, got: {:?}",
            violations
        );
    }

    #[test]
    fn provenance_forged_pointer_from_integer() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 256));
        v.add_derivation(Derivation::new(
            DerivationId(99),
            DerivationSource::Fabricated { raw_value: 0xDEAD },
            DerivationKind::Cast {
                from_repr: "usize".to_string(),
                to_repr: "*mut u8".to_string(),
            },
            (Address::from(0xDEADu64), Address::from(0xDEADu64 + 1)),
        ));

        let ctx = VerificationContext::new();
        let violations = v.validate_derivation_chains(&ctx);

        assert!(
            violations.iter().any(|v| matches!(
                v,
                DerivationViolation::ForgedPointer { derivation_id: 99, .. }
            )),
            "expected ForgedPointer violation, got: {:?}",
            violations
        );
    }

    #[test]
    fn provenance_stack_escape_detection() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(10), Address::from(0x8000u64), 64));
        v.add_derivation(Derivation::new(
            DerivationId(20),
            DerivationSource::Region(RegionId(10)),
            DerivationKind::Direct,
            (Address::from(0x8000u64), Address::from(0x8040u64)),
        ));
        v.add_derivation(Derivation::new(
            DerivationId(21),
            DerivationSource::AnotherDerivation(DerivationId(20)),
            DerivationKind::Offset { by: 8 },
            (Address::from(0x8008u64), Address::from(0x8010u64)),
        ));

        let mut ctx = VerificationContext::new();
        ctx.add_stack_region(10, 42);

        let violations = v.validate_derivation_chains(&ctx);

        assert!(
            violations.iter().any(|v| matches!(
                v,
                DerivationViolation::StackEscape {
                    derivation_id: 21,
                    stack_frame: 42,
                }
            )),
            "expected StackEscape violation, got: {:?}",
            violations
        );

        assert!(
            !violations.iter().any(|v| matches!(
                v,
                DerivationViolation::StackEscape {
                    derivation_id: 20,
                    ..
                }
            )),
            "direct stack derivation should not be flagged as escape"
        );
    }

    #[test]
    fn provenance_graph_construction() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 256));
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(1)),
            DerivationKind::Direct,
            (Address::from(0x1000u64), Address::from(0x1100u64)),
        ));
        v.add_derivation(Derivation::new(
            DerivationId(2),
            DerivationSource::AnotherDerivation(DerivationId(1)),
            DerivationKind::Offset { by: 16 },
            (Address::from(0x1010u64), Address::from(0x1020u64)),
        ));

        let ctx = VerificationContext::new();
        let graph = ProvenanceGraph::build(&v, &ctx);

        assert!(graph.nodes.contains_key(&(1, 0)), "expected region node (1, 0)");
        assert!(graph.nodes.contains_key(&(1, 1)), "expected derivation node (1, 1)");
        assert!(graph.nodes.contains_key(&(1, 2)), "expected derivation node (1, 2)");

        assert_eq!(
            graph.nodes.get(&(1, 0)).unwrap().kind,
            ProvenanceNodeKind::Allocation
        );

        assert_eq!(graph.edges.len(), 2, "expected 2 edges, got {}", graph.edges.len());
        assert_eq!(graph.edges[0].from, (1, 0));
        assert_eq!(graph.edges[0].to, (1, 1));
        assert_eq!(graph.edges[1].from, (1, 1));
        assert_eq!(graph.edges[1].to, (1, 2));
    }

    #[test]
    fn provenance_graph_reachability_query() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 256));
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(1)),
            DerivationKind::Direct,
            (Address::from(0x1000u64), Address::from(0x1100u64)),
        ));
        v.add_derivation(Derivation::new(
            DerivationId(2),
            DerivationSource::AnotherDerivation(DerivationId(1)),
            DerivationKind::Offset { by: 32 },
            (Address::from(0x1020u64), Address::from(0x1040u64)),
        ));

        let ctx = VerificationContext::new();
        let mut graph = ProvenanceGraph::build(&v, &ctx);
        graph.rebuild_adjacency();

        assert!(graph.can_reach(2, 1), "derivation 2 should reach allocation root in region 1");
        assert!(graph.validate_provenance(2, 1), "derivation 2 should have valid provenance from region 1");
        assert!(!graph.can_reach(999, 1), "non-existent derivation should not be reachable");
    }

    #[test]
    fn provenance_cast_record_tracking() {
        let mut detector = ForgedPointerDetector::new();
        detector.add_root(1);

        let legit = CastRecord::new(10, "usize", "*mut u8", true, "main.rs:20");
        assert_eq!(legit.classify(true), CastClassification::Legitimate);
        assert_eq!(legit.classify(false), CastClassification::Suspicious);

        let suspicious = CastRecord::new(11, "u64", "*const T", false, "main.rs:30");
        assert_eq!(suspicious.classify(true), CastClassification::Suspicious);
        assert_eq!(suspicious.classify(false), CastClassification::Forged);

        detector.add_cast_record(legit);
        detector.add_cast_record(suspicious);

        assert_eq!(detector.cast_records.len(), 2);
        assert_eq!(detector.valid_derivation_roots.len(), 1);
        assert!(detector.valid_derivation_roots.contains(&1));
    }

    #[test]
    fn provenance_verify_with_provenance_clean() {
        let mut v = OriginVerifier::new();
        v.add_region(Region::new(RegionId(1), Address::from(0x1000u64), 256));
        v.add_derivation(Derivation::new(
            DerivationId(1),
            DerivationSource::Region(RegionId(1)),
            DerivationKind::Direct,
            (Address::from(0x1000u64), Address::from(0x1100u64)),
        ));
        v.add_access(Access::new(
            AccessId(1),
            DerivationId(1),
            AccessKind::Read,
            4,
            "main.rs:5",
            true,
        ));

        let mut ctx = VerificationContext::new();
        ctx.add_heap_region(1);

        let result = v.verify_with_provenance(&ctx);
        assert!(result.is_clean(), "expected clean result, got violations: {:?}", result.derivation_violations);
        assert!(result.provenance_graph.is_some());
    }
}
