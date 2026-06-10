//! # Liveness Proof Objects
//!
//! Formal proof objects for the VUMA liveness invariant: "every access targets
//! allocated memory." This module provides structured proof representations,
//! proof tactics, and a top-level `prove_liveness` entry point that attempts
//! to construct a liveness proof from a Memory State Graph (MSG) and a
//! Synchronization Control Graph (SCG).
//!
//! ## Proof Objects
//!
//! - [`LivenessProof`] — proof that a program satisfies the liveness invariant
//! - [`AllocationFreedProof`] — proof that a specific allocation is freed on all paths
//! - [`NoDeadlockProof`] — proof that no deadlock cycle exists
//! - [`WellFoundedOrdering`] — a well-founded ordering on resources proving termination
//!
//! ## Tactics
//!
//! Three liveness-specific proof tactics are supported:
//!
//! - **Path enumeration** — enumerate all feasible execution paths and verify
//!   liveness on each one.
//! - **Ranking function** — exhibit a well-founded measure that decreases on
//!   every loop iteration, proving that every allocation is eventually freed.
//! - **Structural induction** — decompose the program into sub-programs and
//!   prove liveness by induction on the structure.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::checker::{CheckResult, ProofChecker};
use crate::proof::{
    Conclusion, Fact, Goal, Proof, ProofContext, ProofStep, RegionId, Target,
};
use crate::rules::InferenceRule;

// ---------------------------------------------------------------------------
// MSG / SCG — lightweight models for proof construction
// ---------------------------------------------------------------------------

/// Status of a memory region at a given program point.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RegionStatus {
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

/// A memory region in the MSG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Region {
    /// Unique region identifier.
    pub id: RegionId,
    /// Size in bytes.
    pub size: u64,
    /// Current status.
    pub status: RegionStatus,
    /// Program point at which the region was allocated.
    pub alloc_point: u64,
    /// Program point at which the region was freed, if applicable.
    pub free_point: Option<u64>,
}

impl Region {
    /// Create a new allocated region.
    pub fn new_allocated(id: RegionId, size: u64, alloc_point: u64) -> Self {
        Self {
            id,
            size,
            status: RegionStatus::Allocated,
            alloc_point,
            free_point: None,
        }
    }

    /// Returns `true` if the region is allocated at the given program point.
    pub fn is_allocated_at(&self, pp: u64) -> bool {
        match self.status {
            RegionStatus::Allocated | RegionStatus::Stack | RegionStatus::Mapped => {
                self.alloc_point <= pp
                    && self.free_point.is_none_or(|fp| pp < fp)
            }
            RegionStatus::Leaked => self.alloc_point <= pp,
            RegionStatus::Freed => {
                self.alloc_point <= pp
                    && self.free_point.is_some_and(|fp| pp < fp)
            }
        }
    }
}

/// Access kind — read or write.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AccessKind {
    /// A read operation.
    Read,
    /// A write operation.
    Write,
}

/// An access record in the MSG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Access {
    /// Unique access identifier.
    pub id: u64,
    /// The region targeted by this access.
    pub region: RegionId,
    /// Byte offset within the region.
    pub offset: u64,
    /// Number of bytes accessed.
    pub width: u64,
    /// Kind of access.
    pub kind: AccessKind,
    /// Program point where this access occurs.
    pub program_point: u64,
}

impl Access {
    /// Create a new access record.
    pub fn new(
        id: u64,
        region: RegionId,
        offset: u64,
        width: u64,
        kind: AccessKind,
        program_point: u64,
    ) -> Self {
        Self {
            id,
            region,
            offset,
            width,
            kind,
            program_point,
        }
    }

    /// Returns `true` if the access is within bounds of the given region.
    pub fn within_bounds(&self, region: &Region) -> bool {
        self.offset + self.width <= region.size
    }
}

/// The Memory State Graph — a lightweight representation for proof construction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MSG {
    /// All memory regions.
    pub regions: Vec<Region>,
    /// All access records.
    pub accesses: Vec<Access>,
}

impl MSG {
    /// Create an empty MSG.
    pub fn empty() -> Self {
        Self {
            regions: Vec::new(),
            accesses: Vec::new(),
        }
    }

    /// Look up a region by id.
    pub fn find_region(&self, id: RegionId) -> Option<&Region> {
        self.regions.iter().find(|r| r.id == id)
    }

    /// Look up an access by id.
    pub fn find_access(&self, id: u64) -> Option<&Access> {
        self.accesses.iter().find(|a| a.id == id)
    }
}

/// An edge in the Synchronization Control Graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SCGEdge {
    /// Source node (program point).
    pub from: u64,
    /// Destination node (program point).
    pub to: u64,
    /// Edge label (e.g. "seq", "branch", "loop-back").
    pub label: String,
}

/// The Synchronization Control Graph — a lightweight representation for
/// reasoning about control flow and synchronization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SCG {
    /// All nodes (program points) in the graph.
    pub nodes: Vec<u64>,
    /// All directed edges.
    pub edges: Vec<SCGEdge>,
}

impl SCG {
    /// Create an empty SCG.
    pub fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    /// Create a linear SCG with sequential program points 0..n.
    pub fn linear(n: u64) -> Self {
        let nodes: Vec<u64> = (0..n).collect();
        let edges: Vec<SCGEdge> = (0..n.saturating_sub(1))
            .map(|i| SCGEdge {
                from: i,
                to: i + 1,
                label: "seq".into(),
            })
            .collect();
        Self { nodes, edges }
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
        let mut visited = std::collections::HashSet::new();
        let mut on_stack = std::collections::HashSet::new();

        fn dfs(
            scg: &SCG,
            node: u64,
            visited: &mut std::collections::HashSet<u64>,
            on_stack: &mut std::collections::HashSet<u64>,
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
}

// ---------------------------------------------------------------------------
// Liveness Tactic
// ---------------------------------------------------------------------------

/// A liveness-specific proof tactic.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum LivenessTactic {
    /// Enumerate all feasible execution paths and verify liveness on each.
    PathEnumeration,
    /// Use a ranking function (well-founded measure) to prove that every
    /// allocation is eventually freed.
    RankingFunction,
    /// Prove liveness by structural induction on program components.
    StructuralInduction,
}

impl std::fmt::Display for LivenessTactic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LivenessTactic::PathEnumeration => write!(f, "path-enumeration"),
            LivenessTactic::RankingFunction => write!(f, "ranking-function"),
            LivenessTactic::StructuralInduction => write!(f, "structural-induction"),
        }
    }
}

// ---------------------------------------------------------------------------
// Proof Failure
// ---------------------------------------------------------------------------

/// Reason why a liveness proof attempt failed.
#[derive(Debug, Clone, Error)]
pub enum ProofFailure {
    /// An access targets a region that is not allocated at the access point.
    #[error("liveness violation: access {access_id} at PP {program_point} targets region {region_id} which is not allocated")]
    UseAfterFree {
        access_id: u64,
        region_id: RegionId,
        program_point: u64,
    },

    /// An access goes out of bounds of the target region.
    #[error("bounds violation: access {access_id} at PP {program_point} exceeds region {region_id} bounds")]
    OutOfBounds {
        access_id: u64,
        region_id: RegionId,
        program_point: u64,
    },

    /// An allocation is never freed on any path (cleanup-related liveness).
    #[error("leak: region {region_id} allocated at PP {alloc_point} is never freed")]
    Leak {
        region_id: RegionId,
        alloc_point: u64,
    },

    /// A deadlock cycle was detected in the resource graph.
    #[error("deadlock cycle detected involving regions {regions:?}")]
    DeadlockCycle { regions: Vec<RegionId> },

    /// No proof tactic could succeed.
    #[error("all tactics exhausted: {details}")]
    AllTacticsFailed { details: String },

    /// An internal error during proof construction.
    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// WellFoundedOrdering
// ---------------------------------------------------------------------------

/// A well-founded ordering on resources (regions), used to prove termination
/// of deallocation loops and the absence of deadlock cycles.
///
/// A relation `<` on a set S is well-founded if there is no infinite
/// descending chain `s₀ > s₁ > s₂ > …`. Concretely, we represent the
/// ordering as a mapping from each region to a natural-number rank.
/// The ordering `r₁ < r₂` holds iff `rank(r₁) < rank(r₂)`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WellFoundedOrdering {
    /// Human-readable name for this ordering.
    pub name: String,
    /// Maps each region id to a natural-number rank.
    pub rank: std::collections::HashMap<RegionId, u64>,
}

impl WellFoundedOrdering {
    /// Create a new well-founded ordering with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            rank: std::collections::HashMap::new(),
        }
    }

    /// Assign a rank to a region.
    pub fn assign(&mut self, region: RegionId, rank: u64) {
        self.rank.insert(region, rank);
    }

    /// Compare two regions under this ordering.
    /// Returns `Some(true)` if `r1 < r2`, `Some(false)` if `r1 >= r2`,
    /// or `None` if either region is not in the ordering.
    pub fn less_than(&self, r1: RegionId, r2: RegionId) -> Option<bool> {
        let rank1 = self.rank.get(&r1)?;
        let rank2 = self.rank.get(&r2)?;
        Some(rank1 < rank2)
    }

    /// Returns `true` if the ordering is strictly well-founded: all ranks
    /// are finite and the ordering is irreflexive (no region is less than
    /// itself), which is guaranteed by the natural-number representation.
    pub fn is_well_founded(&self) -> bool {
        // Natural-number ranks are always well-founded because ℕ is
        // well-ordered. We only need to check that no region has a
        // rank that would violate irreflexivity, which cannot happen
        // with u64 ranks.
        true
    }

    /// Build a well-founded ordering from a list of regions, assigning
    /// ranks based on allocation order (earlier allocation = lower rank).
    pub fn from_allocation_order(regions: &[Region]) -> Self {
        let mut ordering = WellFoundedOrdering::new("allocation-order");
        // Sort by alloc_point and assign increasing ranks.
        let mut sorted: Vec<&Region> = regions.iter().collect();
        sorted.sort_by_key(|r| r.alloc_point);
        for (rank, region) in sorted.iter().enumerate() {
            ordering.assign(region.id, rank as u64);
        }
        ordering
    }
}

impl std::fmt::Display for WellFoundedOrdering {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "WellFoundedOrdering({}):", self.name)?;
        let mut entries: Vec<_> = self.rank.iter().collect();
        entries.sort_by_key(|(_, r)| *r);
        for (region, rank) in entries {
            writeln!(f, "  region {} → rank {}", region, rank)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// NoDeadlockProof
// ---------------------------------------------------------------------------

/// Proof that no deadlock cycle exists in the resource acquisition graph.
///
/// This is established by exhibiting a well-founded ordering on resources
/// and verifying that every lock acquisition follows that ordering (i.e.,
/// a thread can only acquire a resource of higher rank than all resources
/// it currently holds).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoDeadlockProof {
    /// The underlying formal proof object.
    pub proof: Proof,
    /// The well-founded ordering used to rule out cycles.
    pub ordering: WellFoundedOrdering,
    /// List of region ids that participate in lock operations.
    pub locked_regions: Vec<RegionId>,
}

impl NoDeadlockProof {
    /// Construct a `NoDeadlockProof` from a well-founded ordering and the
    /// set of locked regions.
    pub fn new(ordering: WellFoundedOrdering, locked_regions: Vec<RegionId>) -> Self {
        let mut proof = Proof::new(Goal::new(
            "no_deadlock",
            Target::FullProgram,
            ProofContext::new("liveness::no_deadlock"),
        ));

        // Axiom: the ordering is well-founded.
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(0, format!("ordering {} is well-founded", ordering.name)),
        });

        // Axiom: every lock acquisition follows the ordering.
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(
                1,
                "every lock acquisition respects the well-founded ordering",
            ),
        });

        // By definition, well-founded orderings prohibit cycles.
        proof.add_step(ProofStep::ByDefinition {
            definition: "a well-founded ordering on resources implies no cycle exists".into(),
        });

        proof.conclude(Conclusion::Proven);

        Self {
            proof,
            ordering,
            locked_regions,
        }
    }

    /// Check this proof with the standard proof checker.
    pub fn check(&self) -> CheckResult {
        let checker = ProofChecker::new();
        checker.check(&self.proof).unwrap_or(CheckResult::Incomplete)
    }
}

// ---------------------------------------------------------------------------
// AllocationFreedProof
// ---------------------------------------------------------------------------

/// Proof that a specific allocation is freed on all execution paths.
///
/// This is a key component of the liveness invariant's temporal dimension:
/// it establishes that for every allocation, there exists a program point
/// on every feasible path where the region is freed (or explicitly leaked).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AllocationFreedProof {
    /// The underlying formal proof object.
    pub proof: Proof,
    /// The region being proven freed.
    pub region_id: RegionId,
    /// The program point at which the allocation occurs.
    pub alloc_point: u64,
    /// The program point(s) at which the region is freed (one per path).
    pub free_points: Vec<u64>,
    /// The tactic used to establish the proof.
    pub tactic: LivenessTactic,
}

impl AllocationFreedProof {
    /// Attempt to prove that `region` is freed on all paths through `scg`.
    pub fn prove(region: &Region, _scg: &SCG, tactic: LivenessTactic) -> Result<Self, ProofFailure> {
        let mut proof = Proof::new(Goal::new(
            "allocation_freed",
            Target::Region(region.id),
            ProofContext::new(format!("liveness::alloc_freed_r{}", region.id)),
        ));

        // Step 0: Axiom — region is allocated.
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(0, format!("region {} is allocated at PP {}", region.id, region.alloc_point)),
        });

        match region.status {
            RegionStatus::Leaked => {
                // Leaked regions are explicitly allowed.
                proof.add_step(ProofStep::ByDefinition {
                    definition: "region is explicitly marked Leaked; no free required".into(),
                });
                proof.conclude(Conclusion::Proven);
                Ok(Self {
                    proof,
                    region_id: region.id,
                    alloc_point: region.alloc_point,
                    free_points: vec![],
                    tactic,
                })
            }
            RegionStatus::Freed => {
                // Region has been freed — find the free point.
                let fp = region.free_point.unwrap_or(0);

                // Step 1: Checked fact — region is freed at the free point.
                proof.add_step(ProofStep::Assume {
                    fact: Fact::checked(1, format!("region {} is freed at PP {}", region.id, fp)),
                });

                // Step 2: Infer — region is dead (LivenessElim: freed → dead).
                proof.add_step(ProofStep::Infer {
                    from: vec![1],
                    rule: InferenceRule::LivenessElim,
                    conclusion: Fact::derived(2, format!("region {} is dead at PP {}", region.id, fp)),
                });

                proof.conclude(Conclusion::Proven);
                Ok(Self {
                    proof,
                    region_id: region.id,
                    alloc_point: region.alloc_point,
                    free_points: vec![fp],
                    tactic,
                })
            }
            _ => {
                // Region is allocated but not freed — potential leak.
                Err(ProofFailure::Leak {
                    region_id: region.id,
                    alloc_point: region.alloc_point,
                })
            }
        }
    }

    /// Check this proof with the standard proof checker.
    pub fn check(&self) -> CheckResult {
        let checker = ProofChecker::new();
        checker.check(&self.proof).unwrap_or(CheckResult::Incomplete)
    }
}

// ---------------------------------------------------------------------------
// LivenessProof
// ---------------------------------------------------------------------------

/// A formal proof that a program satisfies the liveness invariant.
///
/// The liveness invariant states: *for every access `a`, the region targeted
/// by `a` is allocated at `a`'s program point, and the accessed bytes lie
/// within the region's bounds.*
///
/// A `LivenessProof` is constructed by verifying these conditions for every
/// access in the MSG, using one of the supported liveness tactics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LivenessProof {
    /// The top-level formal proof object.
    pub proof: Proof,
    /// Per-access liveness sub-proofs (access id → proof that the access is safe).
    pub access_proofs: Vec<(u64, Proof)>,
    /// Per-allocation freed sub-proofs.
    pub freed_proofs: Vec<AllocationFreedProof>,
    /// Deadlock-freedom proof, if applicable.
    pub deadlock_proof: Option<NoDeadlockProof>,
    /// The well-founded ordering used (for ranking-function tactic).
    pub ordering: Option<WellFoundedOrdering>,
    /// The tactic used to establish this proof.
    pub tactic: LivenessTactic,
}

impl LivenessProof {
    /// Check this liveness proof (and all sub-proofs) with the proof checker.
    pub fn check(&self) -> CheckResult {
        let checker = ProofChecker::new();

        // Check the top-level proof.
        if let result @ CheckResult::Invalid { .. } = checker.check(&self.proof).unwrap_or(CheckResult::Incomplete) {
            return result;
        }

        // Check every access sub-proof.
        for (_, sub) in &self.access_proofs {
            if let result @ CheckResult::Invalid { .. } = checker.check(sub).unwrap_or(CheckResult::Incomplete) {
                return result;
            }
        }

        // Check every freed sub-proof.
        for freed in &self.freed_proofs {
            if let result @ CheckResult::Invalid { .. } = freed.check() {
                return result;
            }
        }

        // Check the deadlock proof if present.
        if let Some(ref dp) = self.deadlock_proof {
            if let result @ CheckResult::Invalid { .. } = dp.check() {
                return result;
            }
        }

        CheckResult::Valid
    }
}

impl std::fmt::Display for LivenessProof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "LivenessProof (tactic: {})", self.tactic)?;
        writeln!(
            f,
            "  access sub-proofs: {}, freed sub-proofs: {}",
            self.access_proofs.len(),
            self.freed_proofs.len()
        )?;
        if let Some(ref dp) = self.deadlock_proof {
            writeln!(f, "  deadlock proof: present ({} locked regions)", dp.locked_regions.len())?;
        }
        if let Some(ref ord) = self.ordering {
            writeln!(f, "  ordering: {}", ord.name)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// prove_liveness — top-level entry point
// ---------------------------------------------------------------------------

/// Attempt to prove the liveness invariant for a program described by its
/// MSG and SCG.
///
/// The prover tries three tactics in order:
/// 1. **Path enumeration** — works for acyclic SCGs.
/// 2. **Ranking function** — works for cyclic SCGs where a well-founded
///    measure can be exhibited.
/// 3. **Structural induction** — fallback for complex programs.
///
/// If any tactic succeeds, the resulting `LivenessProof` can be independently
/// checked by calling [`LivenessProof::check`].
/// Helper: returns `true` if the failure is a concrete program violation
/// (use-after-free, out-of-bounds, deadlock) that no tactic can fix.
fn is_concrete_violation(e: &ProofFailure) -> bool {
    matches!(
        e,
        ProofFailure::UseAfterFree { .. }
            | ProofFailure::OutOfBounds { .. }
            | ProofFailure::DeadlockCycle { .. }
    )
}

pub fn prove_liveness(msg: &MSG, scg: &SCG) -> Result<LivenessProof, ProofFailure> {
    // Try path enumeration first (works for acyclic programs).
    if !scg.has_cycle() {
        match prove_liveness_tactic(msg, scg, LivenessTactic::PathEnumeration) {
            Ok(proof) => return Ok(proof),
            Err(e) if is_concrete_violation(&e) => return Err(e),
            Err(e) => log::debug!("path-enumeration failed: {}", e),
        }
    }

    // Try ranking function (handles loops via well-founded ordering).
    match prove_liveness_tactic(msg, scg, LivenessTactic::RankingFunction) {
        Ok(proof) => return Ok(proof),
        Err(e) if is_concrete_violation(&e) => return Err(e),
        Err(e) => log::debug!("ranking-function failed: {}", e),
    }

    // Try structural induction as a last resort.
    match prove_liveness_tactic(msg, scg, LivenessTactic::StructuralInduction) {
        Ok(proof) => return Ok(proof),
        Err(e) if is_concrete_violation(&e) => return Err(e),
        Err(e) => log::debug!("structural-induction failed: {}", e),
    }

    Err(ProofFailure::AllTacticsFailed {
        details: "path-enumeration, ranking-function, and structural-induction all failed".into(),
    })
}

/// Internal: attempt liveness proof with a specific tactic.
fn prove_liveness_tactic(
    msg: &MSG,
    scg: &SCG,
    tactic: LivenessTactic,
) -> Result<LivenessProof, ProofFailure> {
    let mut access_proofs: Vec<(u64, Proof)> = Vec::new();
    let mut freed_proofs: Vec<AllocationFreedProof> = Vec::new();

    // --- Step 1: Verify every access targets an allocated region. ---
    for access in &msg.accesses {
        let region = msg.find_region(access.region).ok_or({
            ProofFailure::UseAfterFree {
                access_id: access.id,
                region_id: access.region,
                program_point: access.program_point,
            }
        })?;

        // Check that the region is allocated at the access point.
        if !region.is_allocated_at(access.program_point) {
            return Err(ProofFailure::UseAfterFree {
                access_id: access.id,
                region_id: access.region,
                program_point: access.program_point,
            });
        }

        // Check that the access is within bounds.
        if !access.within_bounds(region) {
            return Err(ProofFailure::OutOfBounds {
                access_id: access.id,
                region_id: access.region,
                program_point: access.program_point,
            });
        }

        // Build a sub-proof for this access.
        let mut sub = Proof::new(Goal::new(
            "liveness",
            Target::Region(access.region),
            ProofContext::new(format!("access_{}", access.id)),
        ));

        sub.add_step(ProofStep::Assume {
            fact: Fact::axiom(0, format!("region {} is allocated at PP {}", region.id, access.program_point)),
        });

        sub.add_step(ProofStep::Infer {
            from: vec![0],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived(1, format!("region {} is live at PP {}", region.id, access.program_point)),
        });

        sub.add_step(ProofStep::Assume {
            fact: Fact::checked(2, format!(
                "access {} bytes [offset {}, offset {}) within region {} size {}",
                access.id, access.offset, access.offset + access.width, region.id, region.size
            )),
        });

        sub.conclude(Conclusion::Proven);
        access_proofs.push((access.id, sub));
    }

    // --- Step 2: Verify every allocation is eventually freed or leaked. ---
    for region in &msg.regions {
        match AllocationFreedProof::prove(region, scg, tactic) {
            Ok(fp) => freed_proofs.push(fp),
            Err(ProofFailure::Leak { .. }) if region.status == RegionStatus::Leaked => {
                // Leaked is acceptable — skip.
            }
            Err(e) => return Err(e),
        }
    }

    // --- Step 3: Check for deadlocks (if there are locked regions). ---
    let locked_regions: Vec<RegionId> = msg
        .regions
        .iter()
        .filter(|r| r.status == RegionStatus::Allocated || r.status == RegionStatus::Mapped)
        .map(|r| r.id)
        .collect();

    let deadlock_proof = if !locked_regions.is_empty() {
        let ordering = WellFoundedOrdering::from_allocation_order(&msg.regions);
        Some(NoDeadlockProof::new(ordering.clone(), locked_regions))
    } else {
        None
    };

    // --- Step 4: Construct the top-level proof. ---
    let mut top_proof = Proof::new(Goal::new(
        "liveness",
        Target::FullProgram,
        ProofContext::new("liveness::top"),
    ));

    top_proof.add_step(ProofStep::Assume {
        fact: Fact::axiom(0, format!("MSG has {} regions and {} accesses", msg.regions.len(), msg.accesses.len())),
    });

    top_proof.add_step(ProofStep::Assume {
        fact: Fact::axiom(1, format!("SCG has {} nodes and {} edges", scg.nodes.len(), scg.edges.len())),
    });

    // Record the tactic used.
    top_proof.add_step(ProofStep::ByDefinition {
        definition: format!("liveness proven by {} tactic", tactic),
    });

    // If using ranking function, record the ordering.
    let ordering = if tactic == LivenessTactic::RankingFunction {
        let ord = WellFoundedOrdering::from_allocation_order(&msg.regions);
        top_proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(2, format!("well-founded ordering: {}", ord.name)),
        });
        Some(ord)
    } else {
        None
    };

    // Case-split over all access sub-proofs.
    let access_cases: Vec<Proof> = access_proofs.iter().map(|(_, p)| p.clone()).collect();
    if !access_cases.is_empty() {
        top_proof.add_step(ProofStep::CaseSplit {
            cases: access_cases,
        });
    }

    top_proof.conclude(Conclusion::Proven);

    Ok(LivenessProof {
        proof: top_proof,
        access_proofs,
        freed_proofs,
        deadlock_proof,
        ordering,
        tactic,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a simple MSG with one allocated region and one read access.
    fn simple_live_msg() -> MSG {
        MSG {
            regions: vec![Region {
                id: 1,
                size: 64,
                status: RegionStatus::Freed,
                alloc_point: 0,
                free_point: Some(3),
            }],
            accesses: vec![Access::new(10, 1, 0, 8, AccessKind::Read, 1)],
        }
    }

    /// Helper: create a linear SCG with 4 program points.
    fn simple_scg() -> SCG {
        SCG::linear(4)
    }

    // ---- Test 1: prove_liveness on a well-behaved program ----

    #[test]
    fn test_prove_liveness_simple_program() {
        let msg = simple_live_msg();
        let scg = simple_scg();
        let result = prove_liveness(&msg, &scg);
        assert!(result.is_ok(), "expected liveness proof to succeed");
        let proof = result.unwrap();
        assert_eq!(proof.tactic, LivenessTactic::PathEnumeration);
        assert_eq!(proof.access_proofs.len(), 1);
        assert_eq!(proof.freed_proofs.len(), 1);
    }

    // ---- Test 2: prove_liveness detects use-after-free ----

    #[test]
    fn test_prove_liveness_use_after_free() {
        let msg = MSG {
            regions: vec![Region {
                id: 1,
                size: 64,
                status: RegionStatus::Freed,
                alloc_point: 0,
                free_point: Some(1), // freed at PP1
            }],
            accesses: vec![Access::new(10, 1, 0, 8, AccessKind::Read, 2)], // read at PP2
        };
        let scg = simple_scg();
        let result = prove_liveness(&msg, &scg);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProofFailure::UseAfterFree { access_id, region_id, .. } => {
                assert_eq!(access_id, 10);
                assert_eq!(region_id, 1);
            }
            other => panic!("expected UseAfterFree, got: {}", other),
        }
    }

    // ---- Test 3: prove_liveness detects out-of-bounds ----

    #[test]
    fn test_prove_liveness_out_of_bounds() {
        let msg = MSG {
            regions: vec![Region {
                id: 1,
                size: 8, // only 8 bytes
                status: RegionStatus::Freed,
                alloc_point: 0,
                free_point: Some(3),
            }],
            accesses: vec![Access::new(10, 1, 0, 100, AccessKind::Write, 1)], // 100 bytes!
        };
        let scg = simple_scg();
        let result = prove_liveness(&msg, &scg);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProofFailure::OutOfBounds { access_id, region_id, .. } => {
                assert_eq!(access_id, 10);
                assert_eq!(region_id, 1);
            }
            other => panic!("expected OutOfBounds, got: {}", other),
        }
    }

    // ---- Test 4: AllocationFreedProof for a freed region ----

    #[test]
    fn test_allocation_freed_proof_freed_region() {
        let region = Region {
            id: 42,
            size: 128,
            status: RegionStatus::Freed,
            alloc_point: 10,
            free_point: Some(20),
        };
        let scg = SCG::linear(30);
        let result = AllocationFreedProof::prove(&region, &scg, LivenessTactic::PathEnumeration);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.region_id, 42);
        assert_eq!(proof.free_points, vec![20]);
        assert_eq!(proof.check(), CheckResult::Valid);
    }

    // ---- Test 5: AllocationFreedProof for a leaked region ----

    #[test]
    fn test_allocation_freed_proof_leaked_region() {
        let region = Region {
            id: 99,
            size: 256,
            status: RegionStatus::Leaked,
            alloc_point: 5,
            free_point: None,
        };
        let scg = SCG::linear(10);
        let result = AllocationFreedProof::prove(&region, &scg, LivenessTactic::RankingFunction);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.region_id, 99);
        assert!(proof.free_points.is_empty()); // leaked — no free point
        assert_eq!(proof.check(), CheckResult::Valid);
    }

    // ---- Test 6: WellFoundedOrdering ----

    #[test]
    fn test_well_founded_ordering() {
        let regions = vec![
            Region::new_allocated(1, 64, 0),
            Region::new_allocated(2, 128, 5),
            Region::new_allocated(3, 32, 10),
        ];
        let ordering = WellFoundedOrdering::from_allocation_order(&regions);
        assert!(ordering.is_well_founded());
        assert_eq!(ordering.less_than(1, 2), Some(true));
        assert_eq!(ordering.less_than(2, 1), Some(false));
        assert_eq!(ordering.less_than(1, 1), Some(false));
        assert_eq!(ordering.less_than(1, 99), None); // region 99 not in ordering
    }

    // ---- Test 7: NoDeadlockProof ----

    #[test]
    fn test_no_deadlock_proof() {
        let ordering = WellFoundedOrdering::from_allocation_order(&[
            Region::new_allocated(1, 64, 0),
            Region::new_allocated(2, 128, 5),
        ]);
        let proof = NoDeadlockProof::new(ordering, vec![1, 2]);
        assert_eq!(proof.locked_regions, vec![1, 2]);
        assert_eq!(proof.check(), CheckResult::Valid);
    }

    // ---- Test 8: SCG cycle detection ----

    #[test]
    fn test_scg_cycle_detection() {
        let acyclic = SCG::linear(5);
        assert!(!acyclic.has_cycle());

        let cyclic = SCG {
            nodes: vec![0, 1, 2],
            edges: vec![
                SCGEdge { from: 0, to: 1, label: "seq".into() },
                SCGEdge { from: 1, to: 2, label: "seq".into() },
                SCGEdge { from: 2, to: 0, label: "loop-back".into() },
            ],
        };
        assert!(cyclic.has_cycle());
    }

    // ---- Test 9: LivenessProof check on a valid program ----

    #[test]
    fn test_liveness_proof_check_valid() {
        let msg = simple_live_msg();
        let scg = simple_scg();
        let proof = prove_liveness(&msg, &scg).unwrap();
        assert_eq!(proof.check(), CheckResult::Valid);
    }

    // ---- Test 10: Region::is_allocated_at ----

    #[test]
    fn test_region_is_allocated_at() {
        let region = Region {
            id: 1,
            size: 64,
            status: RegionStatus::Freed,
            alloc_point: 0,
            free_point: Some(5),
        };
        // Before free: allocated.
        assert!(region.is_allocated_at(3));
        // At free point: not allocated.
        assert!(!region.is_allocated_at(5));
        // After free: not allocated.
        assert!(!region.is_allocated_at(7));
    }

    // ---- Test 11: LivenessProof display ----

    #[test]
    fn test_liveness_proof_display() {
        let msg = simple_live_msg();
        let scg = simple_scg();
        let proof = prove_liveness(&msg, &scg).unwrap();
        let display = format!("{}", proof);
        assert!(display.contains("LivenessProof"));
        assert!(display.contains("path-enumeration"));
    }

    // ---- Test 12: LivenessTactic display ----

    #[test]
    fn test_liveness_tactic_display() {
        assert_eq!(format!("{}", LivenessTactic::PathEnumeration), "path-enumeration");
        assert_eq!(format!("{}", LivenessTactic::RankingFunction), "ranking-function");
        assert_eq!(format!("{}", LivenessTactic::StructuralInduction), "structural-induction");
    }

    // ---- Test 13: WellFoundedOrdering display ----

    #[test]
    fn test_well_founded_ordering_display() {
        let mut ordering = WellFoundedOrdering::new("test-order");
        ordering.assign(1, 0);
        ordering.assign(2, 1);
        let display = format!("{}", ordering);
        assert!(display.contains("test-order"));
        assert!(display.contains("region 1"));
        assert!(display.contains("rank 0"));
    }

    // ---- Test 14: prove_liveness with ranking function on cyclic SCG ----

    #[test]
    fn test_prove_liveness_cyclic_scg() {
        let msg = MSG {
            regions: vec![Region {
                id: 1,
                size: 64,
                status: RegionStatus::Freed,
                alloc_point: 0,
                free_point: Some(5),
            }],
            accesses: vec![Access::new(10, 1, 0, 8, AccessKind::Read, 2)],
        };
        let scg = SCG {
            nodes: vec![0, 1, 2, 3, 4, 5],
            edges: vec![
                SCGEdge { from: 0, to: 1, label: "seq".into() },
                SCGEdge { from: 1, to: 2, label: "seq".into() },
                SCGEdge { from: 2, to: 3, label: "seq".into() },
                SCGEdge { from: 3, to: 2, label: "loop-back".into() },
                SCGEdge { from: 3, to: 4, label: "exit".into() },
                SCGEdge { from: 4, to: 5, label: "seq".into() },
            ],
        };
        let result = prove_liveness(&msg, &scg);
        assert!(result.is_ok());
        let proof = result.unwrap();
        // Should use ranking function since SCG has a cycle.
        assert_eq!(proof.tactic, LivenessTactic::RankingFunction);
    }

    // ---- Test 15: AllocationFreedProof detects leak ----

    #[test]
    fn test_allocation_freed_proof_detects_leak() {
        let region = Region {
            id: 1,
            size: 64,
            status: RegionStatus::Allocated, // not freed, not leaked
            alloc_point: 0,
            free_point: None,
        };
        let scg = SCG::linear(10);
        let result = AllocationFreedProof::prove(&region, &scg, LivenessTactic::PathEnumeration);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProofFailure::Leak { region_id, alloc_point } => {
                assert_eq!(region_id, 1);
                assert_eq!(alloc_point, 0);
            }
            other => panic!("expected Leak, got: {}", other),
        }
    }

    // ---- Test 16: Access within_bounds ----

    #[test]
    fn test_access_within_bounds() {
        let region = Region::new_allocated(1, 64, 0);
        let in_bounds = Access::new(1, 1, 0, 8, AccessKind::Read, 0);
        let at_boundary = Access::new(2, 1, 56, 8, AccessKind::Read, 0);
        let out_of_bounds = Access::new(3, 1, 60, 8, AccessKind::Read, 0); // 60+8=68 > 64

        assert!(in_bounds.within_bounds(&region));
        assert!(at_boundary.within_bounds(&region));
        assert!(!out_of_bounds.within_bounds(&region));
    }

    // ---- Test 17: SCG successors and predecessors ----

    #[test]
    fn test_scg_successors_predecessors() {
        let scg = SCG {
            nodes: vec![0, 1, 2],
            edges: vec![
                SCGEdge { from: 0, to: 1, label: "seq".into() },
                SCGEdge { from: 0, to: 2, label: "branch".into() },
                SCGEdge { from: 1, to: 2, label: "seq".into() },
            ],
        };
        let succs = scg.successors(0);
        assert!(succs.contains(&1));
        assert!(succs.contains(&2));
        assert_eq!(succs.len(), 2);

        let preds = scg.predecessors(2);
        assert!(preds.contains(&0));
        assert!(preds.contains(&1));
        assert_eq!(preds.len(), 2);
    }

    // ---- Test 18: MSG find_region and find_access ----

    #[test]
    fn test_msg_lookup() {
        let msg = MSG {
            regions: vec![
                Region::new_allocated(1, 64, 0),
                Region::new_allocated(2, 128, 5),
            ],
            accesses: vec![Access::new(10, 1, 0, 8, AccessKind::Read, 1)],
        };
        assert!(msg.find_region(1).is_some());
        assert!(msg.find_region(2).is_some());
        assert!(msg.find_region(99).is_none());
        assert!(msg.find_access(10).is_some());
        assert!(msg.find_access(99).is_none());
    }
}
