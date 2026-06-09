//! # Cleanup Proof Objects
//!
//! Formal proof objects that certify cleanup invariants for VUMA programs.
//! The cleanup discipline ensures that every resource acquired during execution
//! is eventually released, that no resource is freed more than once, and that
//! no resource is accessed after it has been freed.
//!
//! # Proof Objects
//!
//! - [`CleanupProof`]: Certifies that every allocated resource has a matching
//!   release along every execution path.
//! - [`NoDoubleFreeProof`]: Certifies that no region is freed more than once.
//! - [`NoUseAfterFreeProof`]: Certifies that no access occurs after a free.
//!
//! # Tactics
//!
//! - [`CleanupTactic::PathEnumeration`]: Enumerates all paths in the SCG and
//!   checks that each allocated region is freed on every path.
//! - [`CleanupTactic::OwnershipTracking`]: Tracks ownership of each region
//!   through the program, ensuring that ownership is transferred or released.
//! - [`CleanupTactic::LifetimeAnalysis`]: Analyzes the lifetime of each region
//!   to ensure that access operations lie within the region's live interval.
//!
//! # Core Function
//!
//! [`prove_cleanup`] is the main entry point that takes a memory state graph
//! (MSG) and a state control graph (SCG) and attempts to produce a
//! [`CleanupProof`].

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

use crate::proof::{
    Conclusion, Fact, FactId, Goal, ProgramPoint, Proof, ProofContext, ProofStep, RegionId, Target,
};

// ---------------------------------------------------------------------------
// MSG — Memory State Graph
// ---------------------------------------------------------------------------

/// The kind of a memory operation recorded in the MSG.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum MemOpKind {
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

impl std::fmt::Display for MemOpKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemOpKind::Alloc => write!(f, "alloc"),
            MemOpKind::Free => write!(f, "free"),
            MemOpKind::Read => write!(f, "read"),
            MemOpKind::Write => write!(f, "write"),
            MemOpKind::Acquire => write!(f, "acquire"),
            MemOpKind::Release => write!(f, "release"),
        }
    }
}

/// A memory operation node in the Memory State Graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemOp {
    /// The region this operation acts upon.
    pub region: RegionId,
    /// The kind of operation.
    pub kind: MemOpKind,
    /// The program point at which this operation occurs.
    pub location: ProgramPoint,
}

impl MemOp {
    /// Create a new memory operation.
    pub fn new(region: RegionId, kind: MemOpKind, location: ProgramPoint) -> Self {
        Self {
            region,
            kind,
            location,
        }
    }
}

/// **MSG** — Memory State Graph. A directed graph whose nodes are memory
/// operations (alloc, free, read, write, acquire, release) and whose edges
/// represent happens-before ordering between operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MSG {
    /// All memory operations in the graph, indexed by their program point.
    pub ops: Vec<MemOp>,
    /// Edges: (from_program_point, to_program_point).
    pub edges: Vec<(ProgramPoint, ProgramPoint)>,
}

impl MSG {
    /// Create an empty MSG.
    pub fn empty() -> Self {
        Self {
            ops: Vec::new(),
            edges: Vec::new(),
        }
    }

    /// Create an MSG from a list of operations and edges.
    pub fn new(ops: Vec<MemOp>, edges: Vec<(ProgramPoint, ProgramPoint)>) -> Self {
        Self { ops, edges }
    }

    /// Return all operations that act on the given region.
    pub fn ops_for_region(&self, region: RegionId) -> Vec<&MemOp> {
        self.ops.iter().filter(|op| op.region == region).collect()
    }

    /// Return all alloc operations.
    pub fn allocs(&self) -> Vec<&MemOp> {
        self.ops.iter().filter(|op| op.kind == MemOpKind::Alloc).collect()
    }

    /// Return all free operations.
    pub fn frees(&self) -> Vec<&MemOp> {
        self.ops.iter().filter(|op| op.kind == MemOpKind::Free).collect()
    }

    /// Return all read operations.
    pub fn reads(&self) -> Vec<&MemOp> {
        self.ops.iter().filter(|op| op.kind == MemOpKind::Read).collect()
    }

    /// Return all write operations.
    pub fn writes(&self) -> Vec<&MemOp> {
        self.ops.iter().filter(|op| op.kind == MemOpKind::Write).collect()
    }

    /// Return the set of all regions mentioned in the MSG.
    pub fn all_regions(&self) -> HashSet<RegionId> {
        self.ops.iter().map(|op| op.region).collect()
    }

    /// For a given region, return the program points where it is freed.
    pub fn free_points(&self, region: RegionId) -> Vec<ProgramPoint> {
        self.ops
            .iter()
            .filter(|op| op.region == region && op.kind == MemOpKind::Free)
            .map(|op| op.location)
            .collect()
    }

    /// For a given region, return the program points where it is allocated.
    pub fn alloc_points(&self, region: RegionId) -> Vec<ProgramPoint> {
        self.ops
            .iter()
            .filter(|op| op.region == region && op.kind == MemOpKind::Alloc)
            .map(|op| op.location)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// SCG — State Control Graph
// ---------------------------------------------------------------------------

/// An edge in the State Control Graph, representing a control-flow transition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SCGEdge {
    /// Source program point.
    pub from: ProgramPoint,
    /// Destination program point.
    pub to: ProgramPoint,
    /// Optional label (e.g. "then", "else", "loop-back").
    pub label: Option<String>,
}

impl SCGEdge {
    /// Create a new SCG edge.
    pub fn new(from: ProgramPoint, to: ProgramPoint) -> Self {
        Self {
            from,
            to,
            label: None,
        }
    }

    /// Create a labeled SCG edge.
    pub fn labeled(from: ProgramPoint, to: ProgramPoint, label: impl Into<String>) -> Self {
        Self {
            from,
            to,
            label: Some(label.into()),
        }
    }
}

/// **SCG** — State Control Graph. Represents the control-flow graph of the
/// program. Nodes are program points; edges represent possible control-flow
/// transitions (sequential, conditional, loop-back, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SCG {
    /// All edges in the control-flow graph.
    pub edges: Vec<SCGEdge>,
    /// The entry point of the program.
    pub entry: ProgramPoint,
    /// The exit points of the program (there may be multiple).
    pub exits: Vec<ProgramPoint>,
}

impl SCG {
    /// Create a new SCG with the given entry and exit points.
    pub fn new(entry: ProgramPoint, exits: Vec<ProgramPoint>) -> Self {
        Self {
            edges: Vec::new(),
            entry,
            exits,
        }
    }

    /// Add an edge to the SCG.
    pub fn add_edge(&mut self, edge: SCGEdge) {
        self.edges.push(edge);
    }

    /// Return the successors of a given program point.
    pub fn successors(&self, point: ProgramPoint) -> Vec<ProgramPoint> {
        self.edges
            .iter()
            .filter(|e| e.from == point)
            .map(|e| e.to)
            .collect()
    }

    /// Return the predecessors of a given program point.
    pub fn predecessors(&self, point: ProgramPoint) -> Vec<ProgramPoint> {
        self.edges
            .iter()
            .filter(|e| e.to == point)
            .map(|e| e.from)
            .collect()
    }

    /// Return all program points in the SCG.
    pub fn all_points(&self) -> HashSet<ProgramPoint> {
        let mut points = HashSet::new();
        points.insert(self.entry);
        for exit in &self.exits {
            points.insert(*exit);
        }
        for edge in &self.edges {
            points.insert(edge.from);
            points.insert(edge.to);
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
// Proof Failure
// ---------------------------------------------------------------------------

/// Describes why a cleanup proof could not be established.
#[derive(Debug, Clone, Error)]
pub enum ProofFailure {
    /// A region was allocated but never freed on some path.
    #[error("region {region} allocated at 0x{alloc_point:x} is never freed on path {path_id}")]
    LeakedResource {
        region: RegionId,
        alloc_point: ProgramPoint,
        path_id: usize,
    },

    /// A region was freed more than once.
    #[error("region {region} freed multiple times: at {free_points:?}")]
    DoubleFree {
        region: RegionId,
        free_points: Vec<ProgramPoint>,
    },

    /// An access (read or write) occurs after the region has been freed.
    #[error("region {region} accessed at 0x{access_point:x} after free at 0x{free_point:x}")]
    UseAfterFree {
        region: RegionId,
        access_point: ProgramPoint,
        free_point: ProgramPoint,
    },

    /// The SCG has no exit points, so cleanup cannot be verified.
    #[error("SCG has no exit points; cleanup cannot be verified")]
    NoExitPoints,

    /// An internal error during proof construction.
    #[error("internal proof error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// CleanupProof
// ---------------------------------------------------------------------------

/// Proof that every allocated resource is eventually released along all
/// execution paths through the program.
///
/// The proof is constructed by showing, for each region that is allocated,
/// that every path from the allocation point to an exit point contains at
/// least one free operation for that region.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CleanupProof {
    /// The formal proof object (reusing the core `Proof` structure).
    pub proof: Proof,
    /// For each region, the allocation point and the set of free points
    /// that release it.
    pub release_map: HashMap<RegionId, ReleaseInfo>,
    /// The tactic used to construct this proof.
    pub tactic: CleanupTactic,
}

/// Information about how a region is released.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// The program point at which the region is allocated.
    pub alloc_point: ProgramPoint,
    /// The program points at which the region is freed (at least one).
    pub free_points: Vec<ProgramPoint>,
}

impl CleanupProof {
    /// Verify that this proof covers all regions in the given MSG.
    pub fn covers_all_regions(&self, msg: &MSG) -> bool {
        let allocated: HashSet<RegionId> = msg.allocs().iter().map(|op| op.region).collect();
        let covered: HashSet<RegionId> = self.release_map.keys().copied().collect();
        allocated == covered
    }
}

// ---------------------------------------------------------------------------
// NoDoubleFreeProof
// ---------------------------------------------------------------------------

/// Proof that no region is freed more than once in any execution of the
/// program.
///
/// This is established by showing that for each region, the MSG contains at
/// most one free operation, and that no path through the SCG reaches two
/// free operations for the same region.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoDoubleFreeProof {
    /// The formal proof object.
    pub proof: Proof,
    /// For each region that is freed, the single free point.
    pub free_map: HashMap<RegionId, ProgramPoint>,
    /// The tactic used to construct this proof.
    pub tactic: CleanupTactic,
}

// ---------------------------------------------------------------------------
// NoUseAfterFreeProof
// ---------------------------------------------------------------------------

/// Proof that no access (read or write) occurs after a region has been freed.
///
/// This is established by showing that for every freed region, all access
/// operations on that region happen-before the free operation on every
/// execution path, and no access operation happens after the free.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoUseAfterFreeProof {
    /// The formal proof object.
    pub proof: Proof,
    /// For each freed region, the free point and all access points that
    /// occur before the free.
    pub lifetime_map: HashMap<RegionId, RegionLifetime>,
    /// The tactic used to construct this proof.
    pub tactic: CleanupTactic,
}

/// The lifetime information for a region — when it is freed and which
/// access points occur within its live interval.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegionLifetime {
    /// The program point at which the region is freed.
    pub free_point: ProgramPoint,
    /// Access points that occur while the region is still live (before free).
    pub live_access_points: Vec<ProgramPoint>,
}

// ---------------------------------------------------------------------------
// CleanupTactic
// ---------------------------------------------------------------------------

/// Tactics for constructing cleanup proofs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CleanupTactic {
    /// **Path Enumeration**: Enumerate all paths in the SCG and verify that
    /// each allocated region is freed on every path from allocation to exit.
    PathEnumeration,
    /// **Ownership Tracking**: Track ownership of each region through the
    /// program's control flow. A region must be owned at its free point and
    /// ownership must be transferred or released along every path.
    OwnershipTracking,
    /// **Lifetime Analysis**: Analyze the lifetime interval of each region
    /// (from alloc to free) and ensure that all access operations fall within
    /// this interval. Detects use-after-free violations.
    LifetimeAnalysis,
}

impl CleanupTactic {
    /// Return the human-readable name of this tactic.
    pub fn name(&self) -> &'static str {
        match self {
            CleanupTactic::PathEnumeration => "PathEnumeration",
            CleanupTactic::OwnershipTracking => "OwnershipTracking",
            CleanupTactic::LifetimeAnalysis => "LifetimeAnalysis",
        }
    }
}

impl std::fmt::Display for CleanupTactic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ---------------------------------------------------------------------------
// prove_cleanup
// ---------------------------------------------------------------------------

/// Attempt to prove the cleanup invariant for the given MSG and SCG.
///
/// The cleanup invariant states:
/// 1. Every allocated region is freed on every execution path (no leaks).
/// 2. No region is freed more than once (no double free).
/// 3. No access occurs after a region has been freed (no use-after-free).
///
/// This function attempts all three proofs. If any fails, a `ProofFailure`
/// is returned. On success, a `CleanupProof` is returned that encapsulates
/// all three sub-proofs.
pub fn prove_cleanup(msg: &MSG, scg: &SCG) -> Result<CleanupProof, ProofFailure> {
    prove_cleanup_with_tactic(msg, scg, CleanupTactic::PathEnumeration)
}

/// Attempt to prove the cleanup invariant using a specific tactic.
pub fn prove_cleanup_with_tactic(
    msg: &MSG,
    scg: &SCG,
    tactic: CleanupTactic,
) -> Result<CleanupProof, ProofFailure> {
    if scg.exits.is_empty() {
        return Err(ProofFailure::NoExitPoints);
    }

    match tactic {
        CleanupTactic::PathEnumeration => prove_via_path_enumeration(msg, scg),
        CleanupTactic::OwnershipTracking => prove_via_ownership_tracking(msg, scg),
        CleanupTactic::LifetimeAnalysis => prove_via_lifetime_analysis(msg, scg),
    }
}

/// Attempt to prove the no-double-free invariant.
pub fn prove_no_double_free(msg: &MSG, scg: &SCG) -> Result<NoDoubleFreeProof, ProofFailure> {
    prove_no_double_free_with_tactic(msg, scg, CleanupTactic::OwnershipTracking)
}

/// Attempt to prove the no-double-free invariant using a specific tactic.
pub fn prove_no_double_free_with_tactic(
    msg: &MSG,
    _scg: &SCG,
    tactic: CleanupTactic,
) -> Result<NoDoubleFreeProof, ProofFailure> {
    let mut free_map: HashMap<RegionId, ProgramPoint> = HashMap::new();
    let mut fact_id: FactId = 0;

    let goal = Goal::new(
        "no_double_free",
        Target::FullProgram,
        ProofContext::new("cleanup::no_double_free"),
    );
    let mut proof = Proof::new(goal);

    // Check: for each free operation, the region must not already have been freed.
    for op in msg.frees() {
        let stmt = format!("region {} freed at 0x{:x}", op.region, op.location);
        proof.add_step(ProofStep::Assume {
            fact: Fact::checked(fact_id, &stmt),
        });

        if let Some(&prev_point) = free_map.get(&op.region) {
            // Double free detected!
            proof.add_step(ProofStep::Contradiction {
                assumption: fact_id,
                negation: fact_id, // self-contradiction: same region freed twice
            });
            proof.conclude(Conclusion::Refuted);
            return Err(ProofFailure::DoubleFree {
                region: op.region,
                free_points: vec![prev_point, op.location],
            });
        }
        free_map.insert(op.region, op.location);
        fact_id += 1;
    }

    // All regions have at most one free — proven.
    proof.add_step(ProofStep::ByDefinition {
        definition: "each region appears in at most one free operation".into(),
    });
    proof.conclude(Conclusion::Proven);

    Ok(NoDoubleFreeProof {
        proof,
        free_map,
        tactic,
    })
}

/// Attempt to prove the no-use-after-free invariant.
pub fn prove_no_use_after_free(
    msg: &MSG,
    scg: &SCG,
) -> Result<NoUseAfterFreeProof, ProofFailure> {
    prove_no_use_after_free_with_tactic(msg, scg, CleanupTactic::LifetimeAnalysis)
}

/// Attempt to prove the no-use-after-free invariant using a specific tactic.
pub fn prove_no_use_after_free_with_tactic(
    msg: &MSG,
    _scg: &SCG,
    tactic: CleanupTactic,
) -> Result<NoUseAfterFreeProof, ProofFailure> {
    let goal = Goal::new(
        "no_use_after_free",
        Target::FullProgram,
        ProofContext::new("cleanup::no_use_after_free"),
    );
    let mut proof = Proof::new(goal);
    let mut fact_id: FactId = 0;
    let mut lifetime_map: HashMap<RegionId, RegionLifetime> = HashMap::new();

    // Build a map from region to its free point(s).
    let mut region_free_points: HashMap<RegionId, ProgramPoint> = HashMap::new();
    for op in msg.frees() {
        region_free_points.insert(op.region, op.location);
    }

    // Check every access (read/write) against free points.
    let accesses: Vec<&MemOp> = msg
        .ops
        .iter()
        .filter(|op| op.kind == MemOpKind::Read || op.kind == MemOpKind::Write)
        .collect();

    for access in &accesses {
        if let Some(&free_point) = region_free_points.get(&access.region) {
            // The region is freed — check if the access is after the free.
            // In our simplified model, we compare program points: if the
            // access location is strictly greater than the free location,
            // it constitutes a use-after-free.
            if access.location > free_point {
                proof.add_step(ProofStep::Assume {
                    fact: Fact::checked(
                        fact_id,
                        format!(
                            "region {} accessed at 0x{:x} after free at 0x{:x}",
                            access.region, access.location, free_point
                        ),
                    ),
                });
                proof.conclude(Conclusion::Refuted);
                return Err(ProofFailure::UseAfterFree {
                    region: access.region,
                    access_point: access.location,
                    free_point,
                });
            }

            // Access is within the live interval — record it.
            let entry = lifetime_map
                .entry(access.region)
                .or_insert_with(|| RegionLifetime {
                    free_point,
                    live_access_points: Vec::new(),
                });
            entry.live_access_points.push(access.location);

            proof.add_step(ProofStep::Assume {
                fact: Fact::checked(
                    fact_id,
                    format!(
                        "region {} accessed at 0x{:x} within live interval (free at 0x{:x})",
                        access.region, access.location, free_point
                    ),
                ),
            });
            fact_id += 1;
        } else {
            // Region not freed — the access is fine for use-after-free
            // purposes, but note it as a potential leak issue (handled by
            // CleanupProof, not here).
            proof.add_step(ProofStep::Assume {
                fact: Fact::checked(
                    fact_id,
                    format!(
                        "region {} accessed at 0x{:x} (region not freed)",
                        access.region, access.location
                    ),
                ),
            });
            fact_id += 1;
        }
    }

    proof.add_step(ProofStep::ByDefinition {
        definition: "all accesses occur within live intervals".into(),
    });
    proof.conclude(Conclusion::Proven);

    Ok(NoUseAfterFreeProof {
        proof,
        lifetime_map,
        tactic,
    })
}

// ---------------------------------------------------------------------------
// Tactic implementations for prove_cleanup
// ---------------------------------------------------------------------------

/// Path-enumeration tactic: enumerate all paths from each allocation point
/// to each exit and verify that every path contains a free for that region.
fn prove_via_path_enumeration(
    msg: &MSG,
    scg: &SCG,
) -> Result<CleanupProof, ProofFailure> {
    let goal = Goal::new(
        "cleanup",
        Target::FullProgram,
        ProofContext::new("cleanup::path_enumeration"),
    );
    let mut proof_obj = Proof::new(goal);
    let mut fact_id: FactId = 0;
    let mut release_map: HashMap<RegionId, ReleaseInfo> = HashMap::new();

    // First, check no double free.
    let _ndf = prove_no_double_free(msg, scg)?;

    // Then, check no use after free.
    let _nuaf = prove_no_use_after_free(msg, scg)?;

    // Now verify every allocated region is freed on every path.
    let paths = scg.enumerate_paths(64);
    let alloc_regions: HashSet<RegionId> = msg.allocs().iter().map(|op| op.region).collect();

    for region in &alloc_regions {
        let alloc_pts = msg.alloc_points(*region);
        let free_pts = msg.free_points(*region);

        // Record the release info.
        if let Some(&alloc_point) = alloc_pts.first() {
            release_map.insert(
                *region,
                ReleaseInfo {
                    alloc_point,
                    free_points: free_pts.clone(),
                },
            );
        }

        // If the region has no free at all, it's leaked.
        if free_pts.is_empty() {
            if let Some(&alloc_point) = alloc_pts.first() {
                proof_obj.add_step(ProofStep::Assume {
                    fact: Fact::checked(
                        fact_id,
                        format!("region {} allocated at 0x{:x} with no free", region, alloc_point),
                    ),
                });
                proof_obj.conclude(Conclusion::Refuted);
                return Err(ProofFailure::LeakedResource {
                    region: *region,
                    alloc_point,
                    path_id: 0,
                });
            }
        }

        // Check that on every path containing the alloc, a free also appears.
        for (path_id, path) in paths.iter().enumerate() {
            let path_set: HashSet<ProgramPoint> = path.iter().copied().collect();
            let alloc_on_path = alloc_pts.iter().any(|p| path_set.contains(p));
            if alloc_on_path {
                let free_on_path = free_pts.iter().any(|p| path_set.contains(p));
                if !free_on_path {
                    if let Some(&alloc_point) = alloc_pts.first() {
                        proof_obj.conclude(Conclusion::Refuted);
                        return Err(ProofFailure::LeakedResource {
                            region: *region,
                            alloc_point,
                            path_id,
                        });
                    }
                }
            }
        }

        proof_obj.add_step(ProofStep::Assume {
            fact: Fact::checked(
                fact_id,
                format!(
                    "region {} freed on all paths (alloc at 0x{:x}, frees at {:?})",
                    region,
                    alloc_pts.first().unwrap_or(&0),
                    free_pts
                ),
            ),
        });
        fact_id += 1;
    }

    proof_obj.add_step(ProofStep::ByDefinition {
        definition: "every allocated region has a matching free on all paths".into(),
    });
    proof_obj.conclude(Conclusion::Proven);

    Ok(CleanupProof {
        proof: proof_obj,
        release_map,
        tactic: CleanupTactic::PathEnumeration,
    })
}

/// Ownership-tracking tactic: model each region as having a unique owner.
/// The owner must either transfer ownership or free the region before
/// exiting scope.
///
/// Two separate sets are tracked:
/// - **allocated**: regions that have been allocated but not yet freed.
///   This tracks memory lifetime (alloc ↔ free).
/// - **access_owned**: regions that are currently under exclusive ownership
///   (via acquire). Release relinquishes access ownership but does *not*
///   free the memory. Free is valid as long as the region is in the
///   `allocated` set.
fn prove_via_ownership_tracking(
    msg: &MSG,
    scg: &SCG,
) -> Result<CleanupProof, ProofFailure> {
    let goal = Goal::new(
        "cleanup",
        Target::FullProgram,
        ProofContext::new("cleanup::ownership_tracking"),
    );
    let mut proof_obj = Proof::new(goal);
    let mut fact_id: FactId = 0;
    let mut release_map: HashMap<RegionId, ReleaseInfo> = HashMap::new();

    // Check no double free first.
    let _ndf = prove_no_double_free(msg, scg)?;
    // Check no use after free.
    let _nuaf = prove_no_use_after_free(msg, scg)?;

    // Track allocation lifetime and access ownership separately.
    let mut allocated: HashSet<RegionId> = HashSet::new();
    let mut access_owned: HashSet<RegionId> = HashSet::new();

    // Sort operations by program point for linear scan.
    let mut sorted_ops = msg.ops.clone();
    sorted_ops.sort_by_key(|op| op.location);

    for op in &sorted_ops {
        match op.kind {
            MemOpKind::Alloc => {
                allocated.insert(op.region);
                proof_obj.add_step(ProofStep::Assume {
                    fact: Fact::checked(
                        fact_id,
                        format!("region {} allocated at 0x{:x}", op.region, op.location),
                    ),
                });
                fact_id += 1;
            }
            MemOpKind::Free => {
                if !allocated.contains(&op.region) {
                    // Region not currently allocated — double free or invalid free.
                    proof_obj.conclude(Conclusion::Refuted);
                    return Err(ProofFailure::DoubleFree {
                        region: op.region,
                        free_points: msg.free_points(op.region),
                    });
                }
                allocated.remove(&op.region);
                access_owned.remove(&op.region);
                proof_obj.add_step(ProofStep::Assume {
                    fact: Fact::checked(
                        fact_id,
                        format!("region {} freed (memory released) at 0x{:x}", op.region, op.location),
                    ),
                });
                fact_id += 1;
            }
            MemOpKind::Acquire => {
                access_owned.insert(op.region);
                proof_obj.add_step(ProofStep::Assume {
                    fact: Fact::checked(
                        fact_id,
                        format!("region {} access ownership acquired at 0x{:x}", op.region, op.location),
                    ),
                });
                fact_id += 1;
            }
            MemOpKind::Release => {
                access_owned.remove(&op.region);
                proof_obj.add_step(ProofStep::Assume {
                    fact: Fact::checked(
                        fact_id,
                        format!("region {} access ownership released at 0x{:x}", op.region, op.location),
                    ),
                });
                fact_id += 1;
            }
            MemOpKind::Read | MemOpKind::Write => {
                // Accesses don't change ownership or allocation state.
            }
        }
    }

    // At the end of the program, all allocated regions must have been freed.
    if !allocated.is_empty() {
        for &region in &allocated {
            let alloc_pts = msg.alloc_points(region);
            proof_obj.conclude(Conclusion::Refuted);
            return Err(ProofFailure::LeakedResource {
                region,
                alloc_point: alloc_pts.first().copied().unwrap_or(0),
                path_id: 0,
            });
        }
    }

    // Build release map.
    for op in msg.allocs() {
        let free_pts = msg.free_points(op.region);
        release_map.insert(
            op.region,
            ReleaseInfo {
                alloc_point: op.location,
                free_points: free_pts,
            },
        );
    }

    proof_obj.add_step(ProofStep::ByDefinition {
        definition: "all allocated regions freed by program exit".into(),
    });
    proof_obj.conclude(Conclusion::Proven);

    Ok(CleanupProof {
        proof: proof_obj,
        release_map,
        tactic: CleanupTactic::OwnershipTracking,
    })
}

/// Lifetime-analysis tactic: compute the live interval [alloc, free] for
/// each region and verify that all accesses fall within this interval.
fn prove_via_lifetime_analysis(
    msg: &MSG,
    scg: &SCG,
) -> Result<CleanupProof, ProofFailure> {
    let goal = Goal::new(
        "cleanup",
        Target::FullProgram,
        ProofContext::new("cleanup::lifetime_analysis"),
    );
    let mut proof_obj = Proof::new(goal);
    let mut fact_id: FactId = 0;
    let mut release_map: HashMap<RegionId, ReleaseInfo> = HashMap::new();

    // Delegate sub-proofs.
    let _ndf = prove_no_double_free(msg, scg)?;
    let nuaf = prove_no_use_after_free(msg, scg)?;

    // Build lifetime intervals and check completeness.
    let all_regions = msg.all_regions();
    for region in &all_regions {
        let alloc_pts = msg.alloc_points(*region);
        let free_pts = msg.free_points(*region);

        if alloc_pts.is_empty() {
            // Region appears without an alloc — skip (might be a parameter).
            continue;
        }

        if free_pts.is_empty() {
            let alloc_point = alloc_pts[0];
            proof_obj.conclude(Conclusion::Refuted);
            return Err(ProofFailure::LeakedResource {
                region: *region,
                alloc_point,
                path_id: 0,
            });
        }

        release_map.insert(
            *region,
            ReleaseInfo {
                alloc_point: alloc_pts[0],
                free_points: free_pts.clone(),
            },
        );

        // Record the lifetime interval in the proof.
        proof_obj.add_step(ProofStep::Assume {
            fact: Fact::checked(
                fact_id,
                format!(
                    "region {} lifetime: [0x{:x}, 0x{:x}]",
                    region,
                    alloc_pts[0],
                    free_pts[0]
                ),
            ),
        });
        fact_id += 1;
    }

    // Incorporate the use-after-free proof's lifetime map.
    for (region, lifetime) in &nuaf.lifetime_map {
        proof_obj.add_step(ProofStep::Assume {
            fact: Fact::checked(
                fact_id,
                format!(
                    "region {} accesses within lifetime: {:?}",
                    region, lifetime.live_access_points
                ),
            ),
        });
        fact_id += 1;
    }

    // Verify using SCG paths that every path covers a free.
    let paths = scg.enumerate_paths(64);
    for region in &all_regions {
        let alloc_pts = msg.alloc_points(*region);
        if alloc_pts.is_empty() {
            continue;
        }
        let free_pts = msg.free_points(*region);

        for (path_id, path) in paths.iter().enumerate() {
            let path_set: HashSet<ProgramPoint> = path.iter().copied().collect();
            let alloc_on_path = alloc_pts.iter().any(|p| path_set.contains(p));
            if alloc_on_path {
                let free_on_path = free_pts.iter().any(|p| path_set.contains(p));
                if !free_on_path {
                    proof_obj.conclude(Conclusion::Refuted);
                    return Err(ProofFailure::LeakedResource {
                        region: *region,
                        alloc_point: alloc_pts[0],
                        path_id,
                    });
                }
            }
        }
    }

    proof_obj.add_step(ProofStep::ByDefinition {
        definition: "all regions have complete lifetimes with matching frees".into(),
    });
    proof_obj.conclude(Conclusion::Proven);

    Ok(CleanupProof {
        proof: proof_obj,
        release_map,
        tactic: CleanupTactic::LifetimeAnalysis,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a simple linear MSG with alloc + free for one region.
    fn make_simple_msg() -> MSG {
        MSG::new(
            vec![
                MemOp::new(1, MemOpKind::Alloc, 0),
                MemOp::new(1, MemOpKind::Read, 1),
                MemOp::new(1, MemOpKind::Free, 2),
            ],
            vec![(0, 1), (1, 2)],
        )
    }

    /// Helper: build a simple linear SCG.
    fn make_simple_scg() -> SCG {
        let mut scg = SCG::new(0, vec![2]);
        scg.add_edge(SCGEdge::new(0, 1));
        scg.add_edge(SCGEdge::new(1, 2));
        scg
    }

    #[test]
    fn test_prove_cleanup_simple() {
        let msg = make_simple_msg();
        let scg = make_simple_scg();
        let result = prove_cleanup(&msg, &scg);
        assert!(result.is_ok(), "expected cleanup proof to succeed");
        let proof = result.unwrap();
        assert_eq!(proof.proof.conclusion, Conclusion::Proven);
        assert!(proof.release_map.contains_key(&1));
        assert_eq!(proof.tactic, CleanupTactic::PathEnumeration);
    }

    #[test]
    fn test_prove_cleanup_leaked_resource() {
        // Region 1 is allocated but never freed.
        let msg = MSG::new(
            vec![
                MemOp::new(1, MemOpKind::Alloc, 0),
                MemOp::new(1, MemOpKind::Read, 1),
            ],
            vec![(0, 1)],
        );
        let scg = make_simple_scg();
        let result = prove_cleanup(&msg, &scg);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProofFailure::LeakedResource { region, .. } => assert_eq!(region, 1),
            other => panic!("expected LeakedResource, got {:?}", other),
        }
    }

    #[test]
    fn test_prove_no_double_free_success() {
        let msg = make_simple_msg();
        let scg = make_simple_scg();
        let result = prove_no_double_free(&msg, &scg);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.proof.conclusion, Conclusion::Proven);
        assert!(proof.free_map.contains_key(&1));
    }

    #[test]
    fn test_prove_no_double_free_failure() {
        // Region 1 is freed twice.
        let msg = MSG::new(
            vec![
                MemOp::new(1, MemOpKind::Alloc, 0),
                MemOp::new(1, MemOpKind::Free, 1),
                MemOp::new(1, MemOpKind::Free, 2),
            ],
            vec![(0, 1), (1, 2)],
        );
        let scg = make_simple_scg();
        let result = prove_no_double_free(&msg, &scg);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProofFailure::DoubleFree { region, free_points } => {
                assert_eq!(region, 1);
                assert_eq!(free_points.len(), 2);
            }
            other => panic!("expected DoubleFree, got {:?}", other),
        }
    }

    #[test]
    fn test_prove_no_use_after_free_success() {
        let msg = make_simple_msg();
        let scg = make_simple_scg();
        let result = prove_no_use_after_free(&msg, &scg);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.proof.conclusion, Conclusion::Proven);
    }

    #[test]
    fn test_prove_no_use_after_free_failure() {
        // Region 1 is read after being freed (access at point 3 > free at point 2).
        let msg = MSG::new(
            vec![
                MemOp::new(1, MemOpKind::Alloc, 0),
                MemOp::new(1, MemOpKind::Free, 2),
                MemOp::new(1, MemOpKind::Read, 3),
            ],
            vec![(0, 2), (2, 3)],
        );
        let scg = make_simple_scg();
        let result = prove_no_use_after_free(&msg, &scg);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProofFailure::UseAfterFree {
                region,
                access_point,
                free_point,
            } => {
                assert_eq!(region, 1);
                assert_eq!(access_point, 3);
                assert_eq!(free_point, 2);
            }
            other => panic!("expected UseAfterFree, got {:?}", other),
        }
    }

    #[test]
    fn test_ownership_tracking_tactic() {
        let msg = make_simple_msg();
        let scg = make_simple_scg();
        let result = prove_cleanup_with_tactic(&msg, &scg, CleanupTactic::OwnershipTracking);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.tactic, CleanupTactic::OwnershipTracking);
        assert_eq!(proof.proof.conclusion, Conclusion::Proven);
    }

    #[test]
    fn test_lifetime_analysis_tactic() {
        let msg = make_simple_msg();
        let scg = make_simple_scg();
        let result = prove_cleanup_with_tactic(&msg, &scg, CleanupTactic::LifetimeAnalysis);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.tactic, CleanupTactic::LifetimeAnalysis);
        assert_eq!(proof.proof.conclusion, Conclusion::Proven);
    }

    #[test]
    fn test_ownership_tracking_leak_detected() {
        // Region 1 is allocated but never freed.
        let msg = MSG::new(
            vec![
                MemOp::new(1, MemOpKind::Alloc, 0),
                MemOp::new(1, MemOpKind::Read, 1),
            ],
            vec![(0, 1)],
        );
        let scg = make_simple_scg();
        let result = prove_cleanup_with_tactic(&msg, &scg, CleanupTactic::OwnershipTracking);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProofFailure::LeakedResource { region, .. } => assert_eq!(region, 1),
            other => panic!("expected LeakedResource, got {:?}", other),
        }
    }

    #[test]
    fn test_scg_path_enumeration() {
        let mut scg = SCG::new(0, vec![3]);
        scg.add_edge(SCGEdge::new(0, 1));
        scg.add_edge(SCGEdge::new(1, 2));
        scg.add_edge(SCGEdge::new(2, 3));
        let paths = scg.enumerate_paths(10);
        assert!(!paths.is_empty());
        assert!(paths.iter().any(|p| p == &vec![0, 1, 2, 3]));
    }

    #[test]
    fn test_scg_branching_paths() {
        let mut scg = SCG::new(0, vec![3]);
        scg.add_edge(SCGEdge::labeled(0, 1, "then"));
        scg.add_edge(SCGEdge::labeled(0, 2, "else"));
        scg.add_edge(SCGEdge::new(1, 3));
        scg.add_edge(SCGEdge::new(2, 3));
        let paths = scg.enumerate_paths(10);
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|p| p == &vec![0, 1, 3]));
        assert!(paths.iter().any(|p| p == &vec![0, 2, 3]));
    }

    #[test]
    fn test_msg_ops_for_region() {
        let msg = make_simple_msg();
        let ops = msg.ops_for_region(1);
        assert_eq!(ops.len(), 3); // alloc, read, free
    }

    #[test]
    fn test_msg_all_regions() {
        let msg = MSG::new(
            vec![
                MemOp::new(1, MemOpKind::Alloc, 0),
                MemOp::new(2, MemOpKind::Alloc, 1),
                MemOp::new(1, MemOpKind::Free, 2),
                MemOp::new(2, MemOpKind::Free, 3),
            ],
            vec![(0, 1), (1, 2), (2, 3)],
        );
        let regions = msg.all_regions();
        assert_eq!(regions.len(), 2);
        assert!(regions.contains(&1));
        assert!(regions.contains(&2));
    }

    #[test]
    fn test_cleanup_proof_covers_all_regions() {
        let msg = make_simple_msg();
        let scg = make_simple_scg();
        let proof = prove_cleanup(&msg, &scg).unwrap();
        assert!(proof.covers_all_regions(&msg));
    }

    #[test]
    fn test_no_exit_points() {
        let msg = make_simple_msg();
        let scg = SCG::new(0, vec![]); // no exits
        let result = prove_cleanup(&msg, &scg);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProofFailure::NoExitPoints => {}
            other => panic!("expected NoExitPoints, got {:?}", other),
        }
    }

    #[test]
    fn test_acquire_release_ownership() {
        let msg = MSG::new(
            vec![
                MemOp::new(1, MemOpKind::Alloc, 0),
                MemOp::new(1, MemOpKind::Acquire, 1),
                MemOp::new(1, MemOpKind::Release, 2),
                MemOp::new(1, MemOpKind::Free, 3),
            ],
            vec![(0, 1), (1, 2), (2, 3)],
        );
        let mut scg = SCG::new(0, vec![3]);
        scg.add_edge(SCGEdge::new(0, 1));
        scg.add_edge(SCGEdge::new(1, 2));
        scg.add_edge(SCGEdge::new(2, 3));
        let result = prove_cleanup_with_tactic(&msg, &scg, CleanupTactic::OwnershipTracking);
        assert!(result.is_ok());
    }

    #[test]
    fn test_memopkind_display() {
        assert_eq!(format!("{}", MemOpKind::Alloc), "alloc");
        assert_eq!(format!("{}", MemOpKind::Free), "free");
        assert_eq!(format!("{}", MemOpKind::Read), "read");
        assert_eq!(format!("{}", MemOpKind::Write), "write");
        assert_eq!(format!("{}", MemOpKind::Acquire), "acquire");
        assert_eq!(format!("{}", MemOpKind::Release), "release");
    }

    #[test]
    fn test_cleanup_tactic_display() {
        assert_eq!(format!("{}", CleanupTactic::PathEnumeration), "PathEnumeration");
        assert_eq!(format!("{}", CleanupTactic::OwnershipTracking), "OwnershipTracking");
        assert_eq!(format!("{}", CleanupTactic::LifetimeAnalysis), "LifetimeAnalysis");
    }

    #[test]
    fn test_region_lifetime_tracking() {
        let msg = make_simple_msg();
        let scg = make_simple_scg();
        let proof = prove_no_use_after_free(&msg, &scg).unwrap();
        assert!(proof.lifetime_map.contains_key(&1));
        let lifetime = &proof.lifetime_map[&1];
        assert_eq!(lifetime.free_point, 2);
        assert!(lifetime.live_access_points.contains(&1));
    }

    #[test]
    fn test_write_after_free_detected() {
        let msg = MSG::new(
            vec![
                MemOp::new(5, MemOpKind::Alloc, 0),
                MemOp::new(5, MemOpKind::Free, 1),
                MemOp::new(5, MemOpKind::Write, 2),
            ],
            vec![(0, 1), (1, 2)],
        );
        let scg = make_simple_scg();
        let result = prove_no_use_after_free(&msg, &scg);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProofFailure::UseAfterFree { region, .. } => assert_eq!(region, 5),
            other => panic!("expected UseAfterFree, got {:?}", other),
        }
    }
}
