//! # Exclusivity Proof Objects
//!
//! Formal proof objects for the **Exclusivity invariant** of the VUMA memory model:
//!
//! > For all accesses a₁, a₂: conflicts(a₁, a₂) ⇒ ordered(a₁, a₂)
//!
//! This module provides three composable proof objects:
//!
//! - [`ExclusivityProof`] — proves that no data race exists across all access pairs.
//! - [`NoAliasProof`] — proves that two derivations do not alias (different root
//!   regions or non-overlapping byte ranges).
//! - [`SynchronizationProof`] — proves that proper synchronization (happens-before,
//!   lock-based, or atomic) exists between two conflicting accesses.
//!
//! The top-level entry point is [`prove_exclusivity`], which attempts to construct
//! an `ExclusivityProof` for a given Memory State Graph (MSG).
//!
//! ## Tactics
//!
//! Four domain-specific tactics drive the proof search:
//!
//! | Tactic | Strategy |
//! |--------|----------|
//! | [`ExclusivityTactic::LocksetAnalysis`] | Verify that all conflicting accesses share a common held lock |
//! | [`ExclusivityTactic::HappensBefore`] | Establish a happens-before ordering via synchronization edges |
//! | [`ExclusivityTactic::OwnershipTransfer`] | Prove exclusive ownership is transferred before the access |
//! | [`ExclusivityTactic::LockGraph`] | Verify lock acquisition order is acyclic (deadlock-freedom) |

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::proof::{
    AccessId, Conclusion, Fact, FactId, Goal, Proof, ProofContext, ProofStep, RegionId,
    Target,
};
use crate::rules::InferenceRule;

// ---------------------------------------------------------------------------
// MSG Types (simplified model for the proof crate)
// ---------------------------------------------------------------------------

/// Unique identifier for a synchronization edge.
pub type SyncEdgeId = u64;

/// Unique identifier for a lock.
pub type LockId = u64;

/// Byte address in the program's memory model.
pub type Addr = u64;

/// The kind of an access operation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AccessKind {
    /// A read operation.
    Read,
    /// A write operation.
    Write,
}

/// The ordering semantics of a synchronization edge (spec §2.5).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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
pub struct SyncEdge {
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

/// An access operation in the MSG (spec §2.4).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Access {
    /// Unique identifier.
    pub id: AccessId,
    /// The derivation being accessed.
    pub derivation_id: u64,
    /// The root region of the derivation.
    pub region: RegionId,
    /// Read or Write.
    pub kind: AccessKind,
    /// Size in bytes of the access.
    pub size: u64,
    /// Starting address (resolved from derivation).
    pub addr: Addr,
    /// Program point of this access.
    pub program_point: u64,
}

/// A derivation in the MSG (spec §2.3).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Derivation {
    /// Unique identifier.
    pub id: u64,
    /// Root region this derivation traces to.
    pub root_region: RegionId,
    /// Byte offset from the root region's base address.
    pub offset: u64,
}

/// A memory region in the MSG (spec §2.2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Region {
    /// Unique identifier.
    pub id: RegionId,
    /// Base address of the region.
    pub base_addr: Addr,
    /// Size in bytes of the region.
    pub size: u64,
}

/// The Memory State Graph — the central data structure for the IVE (spec §2.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MSG {
    /// All memory regions.
    pub regions: Vec<Region>,
    /// All derivations.
    pub derivations: Vec<Derivation>,
    /// All accesses.
    pub accesses: Vec<Access>,
    /// All synchronization edges.
    pub sync_edges: Vec<SyncEdge>,
}

impl MSG {
    /// Create an empty MSG.
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            derivations: Vec::new(),
            accesses: Vec::new(),
            sync_edges: Vec::new(),
        }
    }

    /// Look up a region by id.
    pub fn find_region(&self, id: RegionId) -> Option<&Region> {
        self.regions.iter().find(|r| r.id == id)
    }

    /// Look up an access by id.
    pub fn find_access(&self, id: AccessId) -> Option<&Access> {
        self.accesses.iter().find(|a| a.id == id)
    }

    /// Look up a derivation by id.
    pub fn find_derivation(&self, id: u64) -> Option<&Derivation> {
        self.derivations.iter().find(|d| d.id == id)
    }

    /// Return all sync edges incident to a given access (in either direction).
    pub fn sync_edges_for(&self, access_id: AccessId) -> Vec<&SyncEdge> {
        self.sync_edges
            .iter()
            .filter(|e| e.access1 == access_id || e.access2 == access_id)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Conflict detection
// ---------------------------------------------------------------------------

/// Determine whether two accesses conflict per the spec (§4.1):
/// conflicts(a₁, a₂) iff (is_write(a₁) ∨ is_write(a₂))
///     ∧ region_of(a₁.target) = region_of(a₂.target)
///     ∧ bytes(a₁) ⌣ bytes(a₂)
///     ∧ a₁ ≠ a₂
pub fn conflicts(a1: &Access, a2: &Access) -> bool {
    if a1.id == a2.id {
        return false; // same access
    }
    // At least one must be a write
    if a1.kind == AccessKind::Read && a2.kind == AccessKind::Read {
        return false; // read-read never conflicts
    }
    // Must target the same region
    if a1.region != a2.region {
        return false;
    }
    // Byte ranges must overlap: [a1.addr, a1.addr+a1.size) ⌣ [a2.addr, a2.addr+a2.size)
    byte_ranges_overlap(a1.addr, a1.size, a2.addr, a2.size)
}

/// Check byte-range overlap: [b1, b1+s1) ⌣ [b2, b2+s2)
pub fn byte_ranges_overlap(base1: Addr, size1: u64, base2: Addr, size2: u64) -> bool {
    // Two ranges [b1, e1) and [b2, e2) overlap iff b1 < e2 ∧ b2 < e1
    let e1 = base1.saturating_add(size1);
    let e2 = base2.saturating_add(size2);
    base1 < e2 && base2 < e1
}

/// Check whether the `ordered` relation holds between two accesses,
/// i.e., whether there is a path in the synchronization graph from
/// `a1` to `a2` (transitive closure of SyncEdge).
pub fn is_ordered(msg: &MSG, a1: AccessId, a2: AccessId) -> bool {
    // BFS/DFS through the sync graph from a1 to see if a2 is reachable.
    let mut visited = std::collections::HashSet::new();
    let mut stack = vec![a1];
    while let Some(current) = stack.pop() {
        if current == a2 {
            return true;
        }
        if !visited.insert(current) {
            continue;
        }
        for edge in &msg.sync_edges {
            if edge.access1 == current {
                stack.push(edge.access2);
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Proof Failure
// ---------------------------------------------------------------------------

/// Reason why an exclusivity proof failed.
#[derive(Debug, Clone, Error)]
pub enum ProofFailureReason {
    /// A pair of conflicting accesses was found with no synchronization.
    #[error("data race: access {a1} and access {a2} conflict on region {region} without synchronization")]
    DataRace {
        a1: AccessId,
        a2: AccessId,
        region: RegionId,
    },

    /// Two derivations were found to alias unexpectedly.
    #[error("alias detected: derivations {d1} and {d2} both target region {region}")]
    AliasDetected {
        d1: u64,
        d2: u64,
        region: RegionId,
    },

    /// Lock graph contains a cycle (potential deadlock).
    #[error("lock graph has cycle involving locks {locks:?}")]
    LockCycle { locks: Vec<LockId> },

    /// The proof tactic could not establish the required ordering.
    #[error("tactic {tactic} failed: {reason}")]
    TacticFailed { tactic: String, reason: String },

    /// No applicable tactic was found.
    #[error("no applicable tactic for access pair ({a1}, {a2})")]
    NoApplicableTactic { a1: AccessId, a2: AccessId },
}

/// A proof failure, carrying the reason and an optional counterexample trace.
#[derive(Debug, Clone, Error)]
#[error("exclusivity proof failed: {reason}")]
pub struct ProofFailure {
    /// Why the proof failed.
    pub reason: ProofFailureReason,
    /// Access ids involved in the failure (for diagnostics).
    pub involved_accesses: Vec<AccessId>,
}

impl ProofFailure {
    /// Create a proof failure from a reason.
    pub fn new(reason: ProofFailureReason) -> Self {
        Self {
            reason,
            involved_accesses: Vec::new(),
        }
    }

    /// Create a proof failure with involved accesses.
    pub fn with_accesses(reason: ProofFailureReason, accesses: Vec<AccessId>) -> Self {
        Self {
            reason,
            involved_accesses: accesses,
        }
    }
}

// ---------------------------------------------------------------------------
// NoAliasProof
// ---------------------------------------------------------------------------

/// Method by which non-aliasing was established.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NoAliasMethod {
    /// The derivations trace to different root regions.
    DifferentRegions,
    /// The derivations access non-overlapping byte ranges within the same region.
    NonOverlappingRanges,
    /// Non-aliasing was established by ownership transfer.
    OwnershipDisjoint,
}

/// Proof that two derivations do not alias — they cannot access the same
/// memory location simultaneously.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoAliasProof {
    /// The first derivation id.
    pub derivation1: u64,
    /// The second derivation id.
    pub derivation2: u64,
    /// Method used to establish non-aliasing.
    pub method: NoAliasMethod,
    /// The formal proof object.
    pub proof: Proof,
}

impl NoAliasProof {
    /// Attempt to prove that two derivations do not alias.
    ///
    /// Two derivations alias if they trace to the same root region AND their
    /// byte ranges overlap. If either condition fails, we have a NoAliasProof.
    pub fn prove(msg: &MSG, d1_id: u64, d2_id: u64) -> Result<Self, ProofFailure> {
        let d1 = msg.find_derivation(d1_id).ok_or_else(|| {
            ProofFailure::new(ProofFailureReason::AliasDetected {
                d1: d1_id,
                d2: d2_id,
                region: 0,
            })
        })?;
        let d2 = msg.find_derivation(d2_id).ok_or_else(|| {
            ProofFailure::new(ProofFailureReason::AliasDetected {
                d1: d1_id,
                d2: d2_id,
                region: 0,
            })
        })?;

        let goal = Goal::new(
            "no_alias",
            Target::Derivation(d1_id),
            ProofContext::new("exclusivity::no_alias"),
        );
        let mut proof = Proof::new(goal);

        // Case 1: Different root regions → no alias
        if d1.root_region != d2.root_region {
            proof.add_step(ProofStep::Assume {
                fact: Fact::axiom(1, format!("derivation {} has root region {}", d1_id, d1.root_region)),
            });
            proof.add_step(ProofStep::Assume {
                fact: Fact::axiom(2, format!("derivation {} has root region {}", d2_id, d2.root_region)),
            });
            proof.add_step(ProofStep::Infer {
                from: vec![1, 2],
                rule: InferenceRule::ExclusivityElim,
                conclusion: Fact::derived(
                    3,
                    format!(
                        "no conflict between (exclusive access to region {}) and (exclusive access to region {})",
                        d1.root_region, d2.root_region
                    ),
                ),
            });
            proof.conclude(Conclusion::Proven);

            return Ok(NoAliasProof {
                derivation1: d1_id,
                derivation2: d2_id,
                method: NoAliasMethod::DifferentRegions,
                proof,
            });
        }

        // Case 2: Same region but non-overlapping byte ranges
        let region = msg.find_region(d1.root_region);
        let range1_start = d1.offset;
        let range2_start = d2.offset;
        // We need access sizes — use a default of 1 for derivation-only comparison
        let overlap = if let Some(_r) = region {
            byte_ranges_overlap(range1_start, 1, range2_start, 1)
        } else {
            true // conservatively assume overlap
        };

        if !overlap {
            proof.add_step(ProofStep::Assume {
                fact: Fact::axiom(1, format!("derivation {} offset={}", d1_id, d1.offset)),
            });
            proof.add_step(ProofStep::Assume {
                fact: Fact::axiom(2, format!("derivation {} offset={}", d2_id, d2.offset)),
            });
            proof.add_step(ProofStep::ByDefinition {
                definition: "non-overlapping byte ranges in same region".into(),
            });
            proof.conclude(Conclusion::Proven);

            return Ok(NoAliasProof {
                derivation1: d1_id,
                derivation2: d2_id,
                method: NoAliasMethod::NonOverlappingRanges,
                proof,
            });
        }

        // Cannot prove no-alias: derivations alias
        Err(ProofFailure::new(ProofFailureReason::AliasDetected {
            d1: d1_id,
            d2: d2_id,
            region: d1.root_region,
        }))
    }
}

// ---------------------------------------------------------------------------
// SynchronizationProof
// ---------------------------------------------------------------------------

/// The kind of synchronization established between two accesses.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SynchronizationKind {
    /// Lock-based mutual exclusion (both accesses guarded by the same lock).
    LockBased,
    /// Happens-before ordering (transitive sync edges).
    HappensBefore,
    /// Atomic access with compatible memory ordering.
    Atomic,
    /// Ownership transfer (unique ownership passed between threads).
    OwnershipTransfer,
}

/// Proof that proper synchronization exists between two conflicting accesses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SynchronizationProof {
    /// The first access.
    pub access1: AccessId,
    /// The second access.
    pub access2: AccessId,
    /// The kind of synchronization.
    pub kind: SynchronizationKind,
    /// The lock that guards both accesses (if lock-based).
    pub lock: Option<LockId>,
    /// The sequence of sync edge ids that establish the ordering.
    pub ordering_path: Vec<SyncEdgeId>,
    /// The formal proof object.
    pub proof: Proof,
}

impl SynchronizationProof {
    /// Attempt to prove that two accesses are properly synchronized.
    ///
    /// This tries each synchronization strategy in order:
    /// 1. Lock-based (same lock held for both accesses)
    /// 2. Atomic (direct atomic sync edge between the accesses)
    /// 3. Happens-before (transitive path through sync edges)
    pub fn prove(msg: &MSG, a1_id: AccessId, a2_id: AccessId) -> Result<Self, ProofFailure> {
        let goal = Goal::new(
            "synchronization",
            Target::Access(a1_id),
            ProofContext::new("exclusivity::synchronization"),
        );
        let mut proof = Proof::new(goal);
        let mut fact_id: FactId = 0;

        // Try lock-based synchronization first
        if let Some(lock_id) = find_common_lock(msg, a1_id, a2_id) {
            fact_id += 1;
            proof.add_step(ProofStep::Assume {
                fact: Fact::axiom(fact_id, format!("lock {} acquired on region for access {}", lock_id, a1_id)),
            });
            fact_id += 1;
            proof.add_step(ProofStep::Assume {
                fact: Fact::axiom(fact_id, format!("lock {} acquired on region for access {}", lock_id, a2_id)),
            });
            // Apply ExclusivityIntro for each
            fact_id += 1;
            proof.add_step(ProofStep::Infer {
                from: vec![1],
                rule: InferenceRule::ExclusivityIntro,
                conclusion: Fact::derived(fact_id, format!("exclusive access to region for access {}", a1_id)),
            });
            fact_id += 1;
            proof.add_step(ProofStep::Infer {
                from: vec![2],
                rule: InferenceRule::ExclusivityIntro,
                conclusion: Fact::derived(fact_id, format!("exclusive access to region for access {}", a2_id)),
            });
            proof.conclude(Conclusion::Proven);

            // Collect the sync edges involved
            let path: Vec<SyncEdgeId> = msg
                .sync_edges
                .iter()
                .filter(|e| {
                    e.lock == Some(lock_id)
                        && ((e.access1 == a1_id || e.access2 == a1_id)
                            || (e.access1 == a2_id || e.access2 == a2_id))
                })
                .map(|e| e.id)
                .collect();

            return Ok(SynchronizationProof {
                access1: a1_id,
                access2: a2_id,
                kind: SynchronizationKind::LockBased,
                lock: Some(lock_id),
                ordering_path: path,
                proof,
            });
        }

        // Try atomic synchronization (before happens-before, so direct atomic
        // edges are reported with the more specific Atomic kind)
        if has_atomic_sync(msg, a1_id, a2_id) {
            let path: Vec<SyncEdgeId> = msg
                .sync_edges
                .iter()
                .filter(|e| {
                    e.ordering == SyncOrdering::Atomic
                        && ((e.access1 == a1_id && e.access2 == a2_id)
                            || (e.access1 == a2_id && e.access2 == a1_id))
                })
                .map(|e| e.id)
                .collect();

            fact_id += 1;
            proof.add_step(ProofStep::Assume {
                fact: Fact::axiom(fact_id, format!("atomic sync between access {} and access {}", a1_id, a2_id)),
            });
            proof.conclude(Conclusion::Proven);

            return Ok(SynchronizationProof {
                access1: a1_id,
                access2: a2_id,
                kind: SynchronizationKind::Atomic,
                lock: None,
                ordering_path: path,
                proof,
            });
        }

        // Try happens-before ordering
        if is_ordered(msg, a1_id, a2_id) {
            let path = find_ordering_path(msg, a1_id, a2_id);

            fact_id += 1;
            proof.add_step(ProofStep::Assume {
                fact: Fact::axiom(fact_id, format!("access {} happens before access {}", a1_id, a2_id)),
            });

            // If path length > 1, demonstrate transitivity
            if path.len() > 1 {
                for (i, _edge_id) in path.iter().enumerate().skip(1) {
                    fact_id += 1;
                    proof.add_step(ProofStep::Infer {
                        from: vec![fact_id - 1, fact_id - 1],
                        rule: InferenceRule::TemporalOrdering,
                        conclusion: Fact::derived(
                            fact_id,
                            format!("temporal transitivity step {} for access pair ({}, {})", i, a1_id, a2_id),
                        ),
                    });
                }
            }

            proof.conclude(Conclusion::Proven);

            return Ok(SynchronizationProof {
                access1: a1_id,
                access2: a2_id,
                kind: SynchronizationKind::HappensBefore,
                lock: None,
                ordering_path: path,
                proof,
            });
        }

        // Try reverse ordering
        if is_ordered(msg, a2_id, a1_id) {
            let path = find_ordering_path(msg, a2_id, a1_id);

            fact_id += 1;
            proof.add_step(ProofStep::Assume {
                fact: Fact::axiom(fact_id, format!("access {} happens before access {}", a2_id, a1_id)),
            });
            proof.conclude(Conclusion::Proven);

            return Ok(SynchronizationProof {
                access1: a1_id,
                access2: a2_id,
                kind: SynchronizationKind::HappensBefore,
                lock: None,
                ordering_path: path,
                proof,
            });
        }

        // No synchronization found
        Err(ProofFailure::new(ProofFailureReason::NoApplicableTactic {
            a1: a1_id,
            a2: a2_id,
        }))
    }
}

// ---------------------------------------------------------------------------
// ExclusivityProof
// ---------------------------------------------------------------------------

/// A component of the overall exclusivity proof for a specific conflict pair.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ExclusivitySubProof {
    /// The accesses do not conflict (no write involved or no overlap).
    NoConflict,
    /// The accesses do not alias (different regions or non-overlapping).
    NoAlias(NoAliasProof),
    /// Proper synchronization exists between the accesses.
    Synchronized(SynchronizationProof),
}

/// Proof that the exclusivity invariant holds for a program: no conflicting
/// concurrent accesses exist without proper synchronization.
///
/// Formally: ∀ a₁, a₂ ∈ A: conflicts(a₁, a₂) ⇒ ordered(a₁, a₂)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExclusivityProof {
    /// The top-level formal proof object.
    pub proof: Proof,
    /// Sub-proofs for each pair of potentially conflicting accesses.
    pub sub_proofs: Vec<(AccessId, AccessId, ExclusivitySubProof)>,
    /// The tactic(s) used to establish exclusivity.
    pub tactics_used: Vec<ExclusivityTactic>,
}

/// Domain-specific tactics for proving exclusivity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExclusivityTactic {
    /// **Lockset analysis**: Verify that all conflicting accesses share a common
    /// held lock. If every conflicting pair is guarded by at least one common
    /// lock, exclusivity holds.
    LocksetAnalysis,

    /// **Happens-before**: Establish a happens-before ordering between
    /// conflicting accesses via synchronization edges (fork-join, message
    /// passing, release-acquire).
    HappensBefore,

    /// **Ownership transfer**: Prove that exclusive ownership of a memory
    /// region is transferred between threads (e.g., via send/sync boundaries)
    /// before the receiving thread accesses it.
    OwnershipTransfer,

    /// **Lock graph**: Verify that the lock acquisition order is acyclic,
    /// ensuring deadlock-freedom. A cycle in the lock graph indicates that
    /// two threads could acquire locks in opposite orders, potentially
    /// leading to a deadlock that would violate exclusivity guarantees.
    LockGraph,
}

impl ExclusivityTactic {
    /// Return the human-readable name of this tactic.
    pub fn name(&self) -> &'static str {
        match self {
            ExclusivityTactic::LocksetAnalysis => "LocksetAnalysis",
            ExclusivityTactic::HappensBefore => "HappensBefore",
            ExclusivityTactic::OwnershipTransfer => "OwnershipTransfer",
            ExclusivityTactic::LockGraph => "LockGraph",
        }
    }

    /// Attempt to apply this tactic to a pair of conflicting accesses.
    ///
    /// Returns `Ok(sub_proof)` if the tactic succeeds, or `Err(ProofFailure)`
    /// if it cannot establish exclusivity for this pair.
    pub fn apply(
        &self,
        msg: &MSG,
        a1: &Access,
        a2: &Access,
    ) -> Result<ExclusivitySubProof, ProofFailure> {
        match self {
            ExclusivityTactic::LocksetAnalysis => {
                self.apply_lockset(msg, a1, a2)
            }
            ExclusivityTactic::HappensBefore => {
                self.apply_happens_before(msg, a1, a2)
            }
            ExclusivityTactic::OwnershipTransfer => {
                self.apply_ownership_transfer(msg, a1, a2)
            }
            ExclusivityTactic::LockGraph => {
                self.apply_lock_graph(msg, a1, a2)
            }
        }
    }
}

impl ExclusivityTactic {
    /// Lockset analysis: check if both accesses hold a common lock.
    fn apply_lockset(
        &self,
        msg: &MSG,
        a1: &Access,
        a2: &Access,
    ) -> Result<ExclusivitySubProof, ProofFailure> {
        if let Some(_lock) = find_common_lock(msg, a1.id, a2.id) {
            let sync_proof = SynchronizationProof::prove(msg, a1.id, a2.id)?;
            Ok(ExclusivitySubProof::Synchronized(sync_proof))
        } else {
            Err(ProofFailure::new(ProofFailureReason::TacticFailed {
                tactic: self.name().into(),
                reason: format!(
                    "no common lock held by access {} and access {}",
                    a1.id, a2.id
                ),
            }))
        }
    }

    /// Happens-before: check if ordered(a1, a2) or ordered(a2, a1).
    fn apply_happens_before(
        &self,
        msg: &MSG,
        a1: &Access,
        a2: &Access,
    ) -> Result<ExclusivitySubProof, ProofFailure> {
        if is_ordered(msg, a1.id, a2.id) || is_ordered(msg, a2.id, a1.id) {
            let sync_proof = SynchronizationProof::prove(msg, a1.id, a2.id)?;
            Ok(ExclusivitySubProof::Synchronized(sync_proof))
        } else {
            Err(ProofFailure::new(ProofFailureReason::TacticFailed {
                tactic: self.name().into(),
                reason: format!(
                    "no happens-before ordering between access {} and access {}",
                    a1.id, a2.id
                ),
            }))
        }
    }

    /// Ownership transfer: verify that unique ownership is transferred
    /// between threads before access.
    fn apply_ownership_transfer(
        &self,
        msg: &MSG,
        a1: &Access,
        a2: &Access,
    ) -> Result<ExclusivitySubProof, ProofFailure> {
        // Ownership transfer is modeled as a LockBased synchronization where
        // the "lock" is the ownership token. In a full implementation this
        // would track unique ownership through send/sync boundaries.
        //
        // For the scaffold, we check if there exists a single sync edge
        // between the two accesses that can be interpreted as an ownership
        // transfer.
        let has_transfer = msg.sync_edges.iter().any(|e| {
            (e.access1 == a1.id && e.access2 == a2.id)
                || (e.access1 == a2.id && e.access2 == a1.id)
        });

        if has_transfer {
            let sync_proof = SynchronizationProof::prove(msg, a1.id, a2.id)?;
            Ok(ExclusivitySubProof::Synchronized(sync_proof))
        } else {
            Err(ProofFailure::new(ProofFailureReason::TacticFailed {
                tactic: self.name().into(),
                reason: format!(
                    "no ownership transfer edge between access {} and access {}",
                    a1.id, a2.id
                ),
            }))
        }
    }

    /// Lock graph: verify lock acquisition order is acyclic.
    fn apply_lock_graph(
        &self,
        msg: &MSG,
        a1: &Access,
        a2: &Access,
    ) -> Result<ExclusivitySubProof, ProofFailure> {
        // First, check that lock graph is acyclic.
        if let Some(cycle) = detect_lock_cycle(msg) {
            return Err(ProofFailure::new(ProofFailureReason::LockCycle {
                locks: cycle,
            }));
        }

        // If the lock graph is acyclic and both accesses hold a common lock,
        // the lock graph tactic can establish exclusivity.
        if let Some(_lock) = find_common_lock(msg, a1.id, a2.id) {
            let sync_proof = SynchronizationProof::prove(msg, a1.id, a2.id)?;
            Ok(ExclusivitySubProof::Synchronized(sync_proof))
        } else {
            Err(ProofFailure::new(ProofFailureReason::TacticFailed {
                tactic: self.name().into(),
                reason: format!(
                    "lock graph is acyclic but no common lock for access {} and access {}",
                    a1.id, a2.id
                ),
            }))
        }
    }
}

impl std::fmt::Display for ExclusivityTactic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Find a common lock held by both accesses via Locked sync edges.
fn find_common_lock(msg: &MSG, a1: AccessId, a2: AccessId) -> Option<LockId> {
    let locks_a1: std::collections::HashSet<LockId> = msg
        .sync_edges
        .iter()
        .filter(|e| e.ordering == SyncOrdering::Locked && e.lock.is_some())
        .filter(|e| e.access1 == a1 || e.access2 == a1)
        .filter_map(|e| e.lock)
        .collect();

    for edge in &msg.sync_edges {
        if edge.ordering == SyncOrdering::Locked {
            if let Some(lock) = edge.lock {
                if (edge.access1 == a2 || edge.access2 == a2) && locks_a1.contains(&lock) {
                    return Some(lock);
                }
            }
        }
    }
    None
}

/// Find an ordering path (sequence of sync edge ids) from a1 to a2.
fn find_ordering_path(msg: &MSG, a1: AccessId, a2: AccessId) -> Vec<SyncEdgeId> {
    // BFS to find shortest path from a1 to a2 in the sync graph.
    use std::collections::VecDeque;

    let mut visited = std::collections::HashMap::new();
    let mut queue = VecDeque::new();
    queue.push_back(a1);
    visited.insert(a1, None::<(SyncEdgeId, AccessId)>);

    while let Some(current) = queue.pop_front() {
        if current == a2 {
            // Reconstruct path
            let mut path = Vec::new();
            let mut node = a2;
            while let Some(Some((edge_id, prev))) = visited.get(&node) {
                path.push(*edge_id);
                node = *prev;
            }
            path.reverse();
            return path;
        }

        for edge in &msg.sync_edges {
            if edge.access1 == current && !visited.contains_key(&edge.access2) {
                visited.insert(edge.access2, Some((edge.id, current)));
                queue.push_back(edge.access2);
            }
        }
    }

    Vec::new()
}

/// Check if there is an atomic sync edge between two accesses.
fn has_atomic_sync(msg: &MSG, a1: AccessId, a2: AccessId) -> bool {
    msg.sync_edges.iter().any(|e| {
        e.ordering == SyncOrdering::Atomic
            && ((e.access1 == a1 && e.access2 == a2)
                || (e.access1 == a2 && e.access2 == a1))
    })
}

/// Detect a cycle in the lock graph. Returns the cycle if one exists.
///
/// The lock graph has an edge from lock L₁ to lock L₂ if some access
/// guarded by L₁ also holds L₂ (nested lock acquisition).
fn detect_lock_cycle(msg: &MSG) -> Option<Vec<LockId>> {
    // Collect all locks
    let locks: std::collections::HashSet<LockId> = msg
        .sync_edges
        .iter()
        .filter_map(|e| {
            if e.ordering == SyncOrdering::Locked {
                e.lock
            } else {
                None
            }
        })
        .collect();

    // Build adjacency: for each access, if it appears in multiple Locked edges,
    // those locks are nested (acquired together).
    let mut adj: std::collections::HashMap<LockId, std::collections::HashSet<LockId>> =
        std::collections::HashMap::new();
    for &lock in &locks {
        adj.insert(lock, std::collections::HashSet::new());
    }

    // Group edges by access to find nested locks
    let mut access_locks: std::collections::HashMap<AccessId, Vec<LockId>> =
        std::collections::HashMap::new();
    for edge in &msg.sync_edges {
        if edge.ordering == SyncOrdering::Locked {
            if let Some(lock) = edge.lock {
                access_locks.entry(edge.access1).or_default().push(lock);
                access_locks.entry(edge.access2).or_default().push(lock);
            }
        }
    }

    // Add edges between locks that are held simultaneously
    for (_access, lock_list) in &access_locks {
        for &l1 in lock_list {
            for &l2 in lock_list {
                if l1 != l2 {
                    if let Some(neighbors) = adj.get_mut(&l1) {
                        neighbors.insert(l2);
                    }
                }
            }
        }
    }

    // DFS cycle detection
    let mut white: std::collections::HashSet<LockId> = locks.clone();
    let mut gray: std::collections::HashSet<LockId> = std::collections::HashSet::new();
    let mut black: std::collections::HashSet<LockId> = std::collections::HashSet::new();
    let mut stack: Vec<LockId> = Vec::new();

    for &start in &locks {
        if !white.contains(&start) {
            continue;
        }
        stack.push(start);
        let mut dfs_stack = vec![(start, false)];

        while let Some((node, processed)) = dfs_stack.pop() {
            if processed {
                gray.remove(&node);
                black.insert(node);
                stack.pop();
                continue;
            }

            if gray.contains(&node) {
                // Already gray — this is a cycle
                continue;
            }

            white.remove(&node);
            gray.insert(node);

            // Push post-processing marker
            dfs_stack.push((node, true));

            if let Some(neighbors) = adj.get(&node) {
                for &neighbor in neighbors {
                    if gray.contains(&neighbor) {
                        // Found a cycle — reconstruct it
                        let cycle_start = stack.iter().position(|&l| l == neighbor);
                        if let Some(idx) = cycle_start {
                            let cycle: Vec<LockId> = stack[idx..].to_vec();
                            return Some(cycle);
                        }
                        return Some(vec![node, neighbor]);
                    }
                    if white.contains(&neighbor) {
                        stack.push(neighbor);
                        dfs_stack.push((neighbor, false));
                    }
                }
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// prove_exclusivity — top-level entry point
// ---------------------------------------------------------------------------

/// Attempt to prove the exclusivity invariant for the given MSG.
///
/// For every pair of accesses that conflict (at least one write, same region,
/// overlapping bytes), this function attempts to establish that they are
/// properly synchronized using the available tactics.
///
/// # Returns
///
/// - `Ok(ExclusivityProof)` if all conflicting pairs are properly synchronized.
/// - `Err(ProofFailure)` if any conflicting pair lacks proper synchronization.
pub fn prove_exclusivity(msg: &MSG) -> Result<ExclusivityProof, ProofFailure> {
    let goal = Goal::new(
        "exclusivity",
        Target::FullProgram,
        ProofContext::new("exclusivity::prove"),
    );
    let mut proof = Proof::new(goal);
    let mut sub_proofs: Vec<(AccessId, AccessId, ExclusivitySubProof)> = Vec::new();
    let mut tactics_used: Vec<ExclusivityTactic> = Vec::new();
    let mut fact_id: FactId = 0;

    // Enumerate all pairs of accesses
    for i in 0..msg.accesses.len() {
        for j in (i + 1)..msg.accesses.len() {
            let a1 = &msg.accesses[i];
            let a2 = &msg.accesses[j];

            // Step 1: Check if the accesses conflict
            if !conflicts(a1, a2) {
                sub_proofs.push((a1.id, a2.id, ExclusivitySubProof::NoConflict));
                continue;
            }

            // Step 2: Try NoAliasProof
            if a1.region != a2.region {
                if let Ok(no_alias) = NoAliasProof::prove(msg, a1.derivation_id, a2.derivation_id)
                {
                    fact_id += 1;
                    proof.add_step(ProofStep::Assume {
                        fact: Fact::checked(
                            fact_id,
                            format!(
                                "accesses {} and {} do not alias (different regions)",
                                a1.id, a2.id
                            ),
                        ),
                    });
                    sub_proofs.push((a1.id, a2.id, ExclusivitySubProof::NoAlias(no_alias)));
                    continue;
                }
            }

            // Step 3: Try each tactic in order
            let tactics = [
                ExclusivityTactic::LocksetAnalysis,
                ExclusivityTactic::HappensBefore,
                ExclusivityTactic::OwnershipTransfer,
                ExclusivityTactic::LockGraph,
            ];

            let mut proven = false;
            for tactic in &tactics {
                match tactic.apply(msg, a1, a2) {
                    Ok(sub_proof) => {
                        fact_id += 1;
                        proof.add_step(ProofStep::Assume {
                            fact: Fact::checked(
                                fact_id,
                                format!(
                                    "exclusivity proven for access pair ({}, {}) via {}",
                                    a1.id,
                                    a2.id,
                                    tactic.name()
                                ),
                            ),
                        });
                        if !tactics_used.contains(tactic) {
                            tactics_used.push(*tactic);
                        }
                        sub_proofs.push((a1.id, a2.id, sub_proof));
                        proven = true;
                        break;
                    }
                    Err(_) => {
                        log::debug!(
                            "Tactic {} failed for pair ({}, {})",
                            tactic.name(),
                            a1.id,
                            a2.id
                        );
                    }
                }
            }

            if !proven {
                return Err(ProofFailure::with_accesses(
                    ProofFailureReason::DataRace {
                        a1: a1.id,
                        a2: a2.id,
                        region: a1.region,
                    },
                    vec![a1.id, a2.id],
                ));
            }
        }
    }

    // If we reach here, all conflicting pairs are properly synchronized
    proof.add_step(ProofStep::ByDefinition {
        definition: "all conflicting access pairs are properly synchronized".into(),
    });
    proof.conclude(Conclusion::Proven);

    Ok(ExclusivityProof {
        proof,
        sub_proofs,
        tactics_used,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a minimal MSG with two regions and no accesses.
    fn empty_msg() -> MSG {
        MSG::new()
    }

    /// Helper: build an MSG with a single region and two synchronized accesses.
    fn synchronized_msg() -> MSG {
        MSG {
            regions: vec![Region {
                id: 1,
                base_addr: 0x1000,
                size: 64,
            }],
            derivations: vec![
                Derivation {
                    id: 1,
                    root_region: 1,
                    offset: 0,
                },
                Derivation {
                    id: 2,
                    root_region: 1,
                    offset: 0,
                },
            ],
            accesses: vec![
                Access {
                    id: 1,
                    derivation_id: 1,
                    region: 1,
                    kind: AccessKind::Write,
                    size: 4,
                    addr: 0x1000,
                    program_point: 10,
                },
                Access {
                    id: 2,
                    derivation_id: 2,
                    region: 1,
                    kind: AccessKind::Read,
                    size: 4,
                    addr: 0x1000,
                    program_point: 20,
                },
            ],
            sync_edges: vec![SyncEdge {
                id: 1,
                access1: 1,
                access2: 2,
                ordering: SyncOrdering::Locked,
                lock: Some(100),
            }],
        }
    }

    /// Helper: build an MSG with a data race (no synchronization).
    fn data_race_msg() -> MSG {
        MSG {
            regions: vec![Region {
                id: 1,
                base_addr: 0x1000,
                size: 64,
            }],
            derivations: vec![
                Derivation {
                    id: 1,
                    root_region: 1,
                    offset: 0,
                },
                Derivation {
                    id: 2,
                    root_region: 1,
                    offset: 0,
                },
            ],
            accesses: vec![
                Access {
                    id: 1,
                    derivation_id: 1,
                    region: 1,
                    kind: AccessKind::Write,
                    size: 4,
                    addr: 0x1000,
                    program_point: 10,
                },
                Access {
                    id: 2,
                    derivation_id: 2,
                    region: 1,
                    kind: AccessKind::Read,
                    size: 4,
                    addr: 0x1000,
                    program_point: 20,
                },
            ],
            sync_edges: vec![],
        }
    }

    /// Helper: two accesses to different regions (no conflict).
    fn different_regions_msg() -> MSG {
        MSG {
            regions: vec![
                Region {
                    id: 1,
                    base_addr: 0x1000,
                    size: 64,
                },
                Region {
                    id: 2,
                    base_addr: 0x2000,
                    size: 64,
                },
            ],
            derivations: vec![
                Derivation {
                    id: 1,
                    root_region: 1,
                    offset: 0,
                },
                Derivation {
                    id: 2,
                    root_region: 2,
                    offset: 0,
                },
            ],
            accesses: vec![
                Access {
                    id: 1,
                    derivation_id: 1,
                    region: 1,
                    kind: AccessKind::Write,
                    size: 4,
                    addr: 0x1000,
                    program_point: 10,
                },
                Access {
                    id: 2,
                    derivation_id: 2,
                    region: 2,
                    kind: AccessKind::Write,
                    size: 4,
                    addr: 0x2000,
                    program_point: 20,
                },
            ],
            sync_edges: vec![],
        }
    }

    /// Helper: two read accesses (read-read never conflicts).
    fn read_read_msg() -> MSG {
        MSG {
            regions: vec![Region {
                id: 1,
                base_addr: 0x1000,
                size: 64,
            }],
            derivations: vec![
                Derivation {
                    id: 1,
                    root_region: 1,
                    offset: 0,
                },
                Derivation {
                    id: 2,
                    root_region: 1,
                    offset: 0,
                },
            ],
            accesses: vec![
                Access {
                    id: 1,
                    derivation_id: 1,
                    region: 1,
                    kind: AccessKind::Read,
                    size: 4,
                    addr: 0x1000,
                    program_point: 10,
                },
                Access {
                    id: 2,
                    derivation_id: 2,
                    region: 1,
                    kind: AccessKind::Read,
                    size: 4,
                    addr: 0x1000,
                    program_point: 20,
                },
            ],
            sync_edges: vec![],
        }
    }

    #[test]
    fn test_conflicts_write_read_same_region_overlap() {
        let msg = synchronized_msg();
        let a1 = &msg.accesses[0];
        let a2 = &msg.accesses[1];
        assert!(conflicts(a1, a2));
    }

    #[test]
    fn test_conflicts_read_read_no_conflict() {
        let msg = read_read_msg();
        let a1 = &msg.accesses[0];
        let a2 = &msg.accesses[1];
        assert!(!conflicts(a1, a2));
    }

    #[test]
    fn test_conflicts_different_regions_no_conflict() {
        let msg = different_regions_msg();
        let a1 = &msg.accesses[0];
        let a2 = &msg.accesses[1];
        assert!(!conflicts(a1, a2));
    }

    #[test]
    fn test_byte_ranges_overlap() {
        // [0, 4) and [2, 6) overlap
        assert!(byte_ranges_overlap(0, 4, 2, 4));
        // [0, 4) and [4, 8) do not overlap
        assert!(!byte_ranges_overlap(0, 4, 4, 4));
        // [0, 8) and [3, 5) overlap (contained)
        assert!(byte_ranges_overlap(0, 8, 3, 2));
        // [0, 0) empty range does not overlap
        assert!(!byte_ranges_overlap(0, 0, 0, 4));
    }

    #[test]
    fn test_prove_exclusivity_synchronized() {
        let msg = synchronized_msg();
        let result = prove_exclusivity(&msg);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.proof.conclusion, Conclusion::Proven);
        assert!(!proof.tactics_used.is_empty());
    }

    #[test]
    fn test_prove_exclusivity_data_race() {
        let msg = data_race_msg();
        let result = prove_exclusivity(&msg);
        assert!(result.is_err());
        if let Err(failure) = result {
            assert!(matches!(
                failure.reason,
                ProofFailureReason::DataRace { .. }
            ));
        }
    }

    #[test]
    fn test_prove_exclusivity_no_conflicts() {
        let msg = different_regions_msg();
        let result = prove_exclusivity(&msg);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.proof.conclusion, Conclusion::Proven);
    }

    #[test]
    fn test_prove_exclusivity_read_read() {
        let msg = read_read_msg();
        let result = prove_exclusivity(&msg);
        assert!(result.is_ok());
        let proof = result.unwrap();
        // read-read pairs should be NoConflict
        assert!(proof
            .sub_proofs
            .iter()
            .all(|(_, _, sp)| matches!(sp, ExclusivitySubProof::NoConflict)));
    }

    #[test]
    fn test_no_alias_proof_different_regions() {
        let msg = different_regions_msg();
        let result = NoAliasProof::prove(&msg, 1, 2);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.method, NoAliasMethod::DifferentRegions);
        assert_eq!(proof.proof.conclusion, Conclusion::Proven);
    }

    #[test]
    fn test_synchronization_proof_lock_based() {
        let msg = synchronized_msg();
        let result = SynchronizationProof::prove(&msg, 1, 2);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.kind, SynchronizationKind::LockBased);
        assert_eq!(proof.lock, Some(100));
    }

    #[test]
    fn test_synchronization_proof_no_sync() {
        let msg = data_race_msg();
        let result = SynchronizationProof::prove(&msg, 1, 2);
        assert!(result.is_err());
    }

    #[test]
    fn test_happens_before_tactic() {
        let msg = MSG {
            regions: vec![Region {
                id: 1,
                base_addr: 0x1000,
                size: 64,
            }],
            derivations: vec![
                Derivation {
                    id: 1,
                    root_region: 1,
                    offset: 0,
                },
                Derivation {
                    id: 2,
                    root_region: 1,
                    offset: 0,
                },
            ],
            accesses: vec![
                Access {
                    id: 1,
                    derivation_id: 1,
                    region: 1,
                    kind: AccessKind::Write,
                    size: 4,
                    addr: 0x1000,
                    program_point: 10,
                },
                Access {
                    id: 2,
                    derivation_id: 2,
                    region: 1,
                    kind: AccessKind::Read,
                    size: 4,
                    addr: 0x1000,
                    program_point: 20,
                },
            ],
            sync_edges: vec![SyncEdge {
                id: 1,
                access1: 1,
                access2: 2,
                ordering: SyncOrdering::HappensBefore,
                lock: None,
            }],
        };

        let result = ExclusivityTactic::HappensBefore.apply(
            &msg,
            &msg.accesses[0],
            &msg.accesses[1],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_lock_graph_tactic_with_cycle() {
        // Build an MSG where two locks are held in opposite orders
        let msg = MSG {
            regions: vec![Region {
                id: 1,
                base_addr: 0x1000,
                size: 64,
            }],
            derivations: vec![],
            accesses: vec![
                Access {
                    id: 1,
                    derivation_id: 0,
                    region: 1,
                    kind: AccessKind::Write,
                    size: 4,
                    addr: 0x1000,
                    program_point: 10,
                },
                Access {
                    id: 2,
                    derivation_id: 0,
                    region: 1,
                    kind: AccessKind::Read,
                    size: 4,
                    addr: 0x1000,
                    program_point: 20,
                },
            ],
            sync_edges: vec![
                // Access 1 holds lock A and lock B
                SyncEdge {
                    id: 1,
                    access1: 1,
                    access2: 1,
                    ordering: SyncOrdering::Locked,
                    lock: Some(10),
                },
                SyncEdge {
                    id: 2,
                    access1: 1,
                    access2: 1,
                    ordering: SyncOrdering::Locked,
                    lock: Some(20),
                },
                // Access 2 holds lock B and lock A (reversed order)
                SyncEdge {
                    id: 3,
                    access1: 2,
                    access2: 2,
                    ordering: SyncOrdering::Locked,
                    lock: Some(20),
                },
                SyncEdge {
                    id: 4,
                    access1: 2,
                    access2: 2,
                    ordering: SyncOrdering::Locked,
                    lock: Some(10),
                },
                // Both accesses hold lock 10
                SyncEdge {
                    id: 5,
                    access1: 1,
                    access2: 2,
                    ordering: SyncOrdering::Locked,
                    lock: Some(10),
                },
            ],
        };

        let result = ExclusivityTactic::LockGraph.apply(
            &msg,
            &msg.accesses[0],
            &msg.accesses[1],
        );
        // Lock graph should detect a cycle
        assert!(result.is_err());
        if let Err(failure) = result {
            assert!(matches!(failure.reason, ProofFailureReason::LockCycle { .. }));
        }
    }

    #[test]
    fn test_exclusivity_tactic_display() {
        assert_eq!(
            format!("{}", ExclusivityTactic::LocksetAnalysis),
            "LocksetAnalysis"
        );
        assert_eq!(
            format!("{}", ExclusivityTactic::HappensBefore),
            "HappensBefore"
        );
        assert_eq!(
            format!("{}", ExclusivityTactic::OwnershipTransfer),
            "OwnershipTransfer"
        );
        assert_eq!(format!("{}", ExclusivityTactic::LockGraph), "LockGraph");
    }

    #[test]
    fn test_prove_exclusivity_empty_msg() {
        let msg = empty_msg();
        let result = prove_exclusivity(&msg);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.proof.conclusion, Conclusion::Proven);
        assert!(proof.sub_proofs.is_empty());
    }

    #[test]
    fn test_is_ordered_transitive() {
        let msg = MSG {
            regions: vec![],
            derivations: vec![],
            accesses: vec![
                Access {
                    id: 1,
                    derivation_id: 0,
                    region: 0,
                    kind: AccessKind::Write,
                    size: 4,
                    addr: 0,
                    program_point: 1,
                },
                Access {
                    id: 2,
                    derivation_id: 0,
                    region: 0,
                    kind: AccessKind::Read,
                    size: 4,
                    addr: 0,
                    program_point: 2,
                },
                Access {
                    id: 3,
                    derivation_id: 0,
                    region: 0,
                    kind: AccessKind::Read,
                    size: 4,
                    addr: 0,
                    program_point: 3,
                },
            ],
            sync_edges: vec![
                SyncEdge {
                    id: 1,
                    access1: 1,
                    access2: 2,
                    ordering: SyncOrdering::HappensBefore,
                    lock: None,
                },
                SyncEdge {
                    id: 2,
                    access1: 2,
                    access2: 3,
                    ordering: SyncOrdering::HappensBefore,
                    lock: None,
                },
            ],
        };

        // Transitive: 1 → 2 → 3, so 1 → 3
        assert!(is_ordered(&msg, 1, 3));
        assert!(is_ordered(&msg, 1, 2));
        assert!(is_ordered(&msg, 2, 3));
        // Reverse is not ordered
        assert!(!is_ordered(&msg, 3, 1));
    }

    #[test]
    fn test_find_ordering_path() {
        let msg = MSG {
            regions: vec![],
            derivations: vec![],
            accesses: vec![],
            sync_edges: vec![
                SyncEdge {
                    id: 10,
                    access1: 1,
                    access2: 2,
                    ordering: SyncOrdering::HappensBefore,
                    lock: None,
                },
                SyncEdge {
                    id: 20,
                    access1: 2,
                    access2: 3,
                    ordering: SyncOrdering::HappensBefore,
                    lock: None,
                },
            ],
        };

        let path = find_ordering_path(&msg, 1, 3);
        assert_eq!(path, vec![10, 20]);
    }

    #[test]
    fn test_ownership_transfer_tactic() {
        let msg = MSG {
            regions: vec![Region {
                id: 1,
                base_addr: 0x1000,
                size: 64,
            }],
            derivations: vec![
                Derivation {
                    id: 1,
                    root_region: 1,
                    offset: 0,
                },
                Derivation {
                    id: 2,
                    root_region: 1,
                    offset: 0,
                },
            ],
            accesses: vec![
                Access {
                    id: 1,
                    derivation_id: 1,
                    region: 1,
                    kind: AccessKind::Write,
                    size: 4,
                    addr: 0x1000,
                    program_point: 10,
                },
                Access {
                    id: 2,
                    derivation_id: 2,
                    region: 1,
                    kind: AccessKind::Read,
                    size: 4,
                    addr: 0x1000,
                    program_point: 20,
                },
            ],
            sync_edges: vec![SyncEdge {
                id: 1,
                access1: 1,
                access2: 2,
                ordering: SyncOrdering::HappensBefore,
                lock: None,
            }],
        };

        let result = ExclusivityTactic::OwnershipTransfer.apply(
            &msg,
            &msg.accesses[0],
            &msg.accesses[1],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_atomic_synchronization() {
        let msg = MSG {
            regions: vec![Region {
                id: 1,
                base_addr: 0x1000,
                size: 64,
            }],
            derivations: vec![
                Derivation {
                    id: 1,
                    root_region: 1,
                    offset: 0,
                },
                Derivation {
                    id: 2,
                    root_region: 1,
                    offset: 0,
                },
            ],
            accesses: vec![
                Access {
                    id: 1,
                    derivation_id: 1,
                    region: 1,
                    kind: AccessKind::Write,
                    size: 4,
                    addr: 0x1000,
                    program_point: 10,
                },
                Access {
                    id: 2,
                    derivation_id: 2,
                    region: 1,
                    kind: AccessKind::Read,
                    size: 4,
                    addr: 0x1000,
                    program_point: 20,
                },
            ],
            sync_edges: vec![SyncEdge {
                id: 1,
                access1: 1,
                access2: 2,
                ordering: SyncOrdering::Atomic,
                lock: None,
            }],
        };

        let result = SynchronizationProof::prove(&msg, 1, 2);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.kind, SynchronizationKind::Atomic);
    }

    #[test]
    fn test_proof_failure_display() {
        let failure = ProofFailure::new(ProofFailureReason::DataRace {
            a1: 1,
            a2: 2,
            region: 42,
        });
        let msg = format!("{}", failure);
        assert!(msg.contains("data race"));
        assert!(msg.contains("access 1"));
        assert!(msg.contains("access 2"));
    }

    #[test]
    fn test_no_alias_proof_same_region_non_overlapping() {
        let msg = MSG {
            regions: vec![Region {
                id: 1,
                base_addr: 0x1000,
                size: 64,
            }],
            derivations: vec![
                Derivation {
                    id: 1,
                    root_region: 1,
                    offset: 0,
                },
                Derivation {
                    id: 2,
                    root_region: 1,
                    offset: 10,
                },
            ],
            accesses: vec![],
            sync_edges: vec![],
        };

        let result = NoAliasProof::prove(&msg, 1, 2);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert_eq!(proof.method, NoAliasMethod::NonOverlappingRanges);
    }
}
