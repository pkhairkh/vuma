//! # Exclusivity Proof Objects
//!
//! Formal proof objects for the **Exclusivity invariant** of the VUMA memory model:
//!
//! > For all accesses a₁, a₂: conflicts(a₁, a₂) ⇒ ordered(a₁, a₂)
//!
//! This module provides three composable proof objects:
//!
//! - [`ExclusivityProof`] — proves that no data race exists across all access pairs.
//! - [`NoAliasProof`] — proves that two derivations do not alias.
//! - [`SynchronizationProof`] — proves that proper synchronization exists
//!   between two conflicting accesses.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::judgment::RegionId;
use crate::models::{
    LockId, ProofAccess, ProofAccessKind, ProofMSG, SyncEdgeId, SyncOrdering, Addr,
};
use crate::proof::{
    AccessId, Conclusion, Fact, FactId, Goal, InvariantName, Proof, ProofContext, ProofStep, Target,
};
use crate::rules::InferenceRule;

// ---------------------------------------------------------------------------
// Conflict detection
// ---------------------------------------------------------------------------

/// Determine whether two accesses conflict per the spec (§4.1).
pub fn conflicts(a1: &ProofAccess, a2: &ProofAccess) -> bool {
    if a1.id == a2.id {
        return false;
    }
    if a1.kind == ProofAccessKind::Read && a2.kind == ProofAccessKind::Read {
        return false;
    }
    if a1.region != a2.region {
        return false;
    }
    byte_ranges_overlap(a1.addr, a1.size, a2.addr, a2.size)
}

/// Check byte-range overlap: [b1, b1+s1) ⌣ [b2, b2+s2)
pub fn byte_ranges_overlap(base1: Addr, size1: u64, base2: Addr, size2: u64) -> bool {
    let e1 = base1.saturating_add(size1);
    let e2 = base2.saturating_add(size2);
    base1 < e2 && base2 < e1
}

/// Check whether the `ordered` relation holds between two accesses.
pub fn is_ordered(msg: &ProofMSG, a1: AccessId, a2: AccessId) -> bool {
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
    #[error("data race: access {a1} and access {a2} conflict on region {region} without synchronization")]
    DataRace {
        a1: AccessId,
        a2: AccessId,
        region: RegionId,
    },

    #[error("alias detected: derivations {d1} and {d2} both target region {region}")]
    AliasDetected {
        d1: u64,
        d2: u64,
        region: RegionId,
    },

    #[error("lock graph has cycle involving locks {locks:?}")]
    LockCycle { locks: Vec<LockId> },

    #[error("tactic {tactic} failed: {reason}")]
    TacticFailed { tactic: String, reason: String },

    #[error("no applicable tactic for access pair ({a1}, {a2})")]
    NoApplicableTactic { a1: AccessId, a2: AccessId },
}

/// A proof failure, carrying the reason and an optional counterexample trace.
#[derive(Debug, Clone, Error)]
#[error("exclusivity proof failed: {reason}")]
pub struct ProofFailure {
    pub reason: ProofFailureReason,
    pub involved_accesses: Vec<AccessId>,
}

impl ProofFailure {
    pub fn new(reason: ProofFailureReason) -> Self {
        Self {
            reason,
            involved_accesses: Vec::new(),
        }
    }

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
    DifferentRegions,
    NonOverlappingRanges,
    OwnershipDisjoint,
}

/// Proof that two derivations do not alias.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoAliasProof {
    pub derivation1: u64,
    pub derivation2: u64,
    pub method: NoAliasMethod,
    pub proof: Proof,
}

impl NoAliasProof {
    /// Attempt to prove that two derivations do not alias.
    pub fn prove(msg: &ProofMSG, d1_id: u64, d2_id: u64) -> Result<Self, ProofFailure> {
        let d1 = msg.find_derivation(d1_id).ok_or_else(|| {
            ProofFailure::new(ProofFailureReason::AliasDetected {
                d1: d1_id,
                d2: d2_id,
                region: RegionId::from(0u64),
            })
        })?;
        let d2 = msg.find_derivation(d2_id).ok_or_else(|| {
            ProofFailure::new(ProofFailureReason::AliasDetected {
                d1: d1_id,
                d2: d2_id,
                region: RegionId::from(0u64),
            })
        })?;

        let goal = Goal::new(
            InvariantName::Exclusivity,
            Target::Derivation(d1_id),
            ProofContext::new("exclusivity::no_alias"),
        );
        let mut proof = Proof::new(goal);

        // Use effective_root_region for both exclusivity-style and interpretation-style derivations
        let d1_root = d1.effective_root_region();
        let d2_root = d2.effective_root_region();

        // Case 1: Different root regions → no alias
        if let (Some(r1), Some(r2)) = (d1_root, d2_root) {
            if r1 != r2 {
                proof.add_step(ProofStep::Assume {
                    fact: Fact::axiom(1, format!("derivation {} has root region {}", d1_id, r1)),
                });
                proof.add_step(ProofStep::Assume {
                    fact: Fact::axiom(2, format!("derivation {} has root region {}", d2_id, r2)),
                });
                proof.add_step(ProofStep::Infer {
                    from: vec![1, 2],
                    rule: InferenceRule::ExclusivityElim,
                    conclusion: Fact::derived(
                        3,
                        format!(
                            "no conflict between (exclusive access to region {}) and (exclusive access to region {})",
                            r1, r2
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
            let range1_start = d1.offset as u64;
            let range2_start = d2.offset as u64;
            let region = msg.find_region(r1);
            let overlap = if region.is_some() {
                byte_ranges_overlap(range1_start, 1, range2_start, 1)
            } else {
                true
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
        }

        // Cannot prove no-alias
        Err(ProofFailure::new(ProofFailureReason::AliasDetected {
            d1: d1_id,
            d2: d2_id,
            region: d1_root.unwrap_or(RegionId::from(0u64)),
        }))
    }
}

// ---------------------------------------------------------------------------
// SynchronizationProof
// ---------------------------------------------------------------------------

/// The kind of synchronization established between two accesses.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SynchronizationKind {
    LockBased,
    HappensBefore,
    Atomic,
    OwnershipTransfer,
}

/// Proof that proper synchronization exists between two conflicting accesses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SynchronizationProof {
    pub access1: AccessId,
    pub access2: AccessId,
    pub kind: SynchronizationKind,
    pub lock: Option<LockId>,
    pub ordering_path: Vec<SyncEdgeId>,
    pub proof: Proof,
}

impl SynchronizationProof {
    /// Attempt to prove that two accesses are properly synchronized.
    pub fn prove(msg: &ProofMSG, a1_id: AccessId, a2_id: AccessId) -> Result<Self, ProofFailure> {
        let goal = Goal::new(
            InvariantName::Exclusivity,
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

        // Try atomic synchronization
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
    NoConflict,
    NoAlias(NoAliasProof),
    Synchronized(SynchronizationProof),
}

/// Proof that the exclusivity invariant holds for a program.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExclusivityProof {
    pub proof: Proof,
    pub sub_proofs: Vec<(AccessId, AccessId, ExclusivitySubProof)>,
    pub tactics_used: Vec<ExclusivityTactic>,
}

/// Domain-specific tactics for proving exclusivity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExclusivityTactic {
    LocksetAnalysis,
    HappensBefore,
    OwnershipTransfer,
    LockGraph,
}

impl ExclusivityTactic {
    pub fn name(&self) -> &'static str {
        match self {
            ExclusivityTactic::LocksetAnalysis => "LocksetAnalysis",
            ExclusivityTactic::HappensBefore => "HappensBefore",
            ExclusivityTactic::OwnershipTransfer => "OwnershipTransfer",
            ExclusivityTactic::LockGraph => "LockGraph",
        }
    }

    pub fn apply(
        &self,
        msg: &ProofMSG,
        a1: &ProofAccess,
        a2: &ProofAccess,
    ) -> Result<ExclusivitySubProof, ProofFailure> {
        match self {
            ExclusivityTactic::LocksetAnalysis => self.apply_lockset(msg, a1, a2),
            ExclusivityTactic::HappensBefore => self.apply_happens_before(msg, a1, a2),
            ExclusivityTactic::OwnershipTransfer => self.apply_ownership_transfer(msg, a1, a2),
            ExclusivityTactic::LockGraph => self.apply_lock_graph(msg, a1, a2),
        }
    }

    fn apply_lockset(
        &self,
        msg: &ProofMSG,
        a1: &ProofAccess,
        a2: &ProofAccess,
    ) -> Result<ExclusivitySubProof, ProofFailure> {
        if find_common_lock(msg, a1.id, a2.id).is_some() {
            let sync_proof = SynchronizationProof::prove(msg, a1.id, a2.id)?;
            Ok(ExclusivitySubProof::Synchronized(sync_proof))
        } else {
            Err(ProofFailure::new(ProofFailureReason::TacticFailed {
                tactic: self.name().into(),
                reason: format!("no common lock held by access {} and access {}", a1.id, a2.id),
            }))
        }
    }

    fn apply_happens_before(
        &self,
        msg: &ProofMSG,
        a1: &ProofAccess,
        a2: &ProofAccess,
    ) -> Result<ExclusivitySubProof, ProofFailure> {
        if is_ordered(msg, a1.id, a2.id) || is_ordered(msg, a2.id, a1.id) {
            let sync_proof = SynchronizationProof::prove(msg, a1.id, a2.id)?;
            Ok(ExclusivitySubProof::Synchronized(sync_proof))
        } else {
            Err(ProofFailure::new(ProofFailureReason::TacticFailed {
                tactic: self.name().into(),
                reason: format!("no happens-before ordering between access {} and access {}", a1.id, a2.id),
            }))
        }
    }

    fn apply_ownership_transfer(
        &self,
        msg: &ProofMSG,
        a1: &ProofAccess,
        a2: &ProofAccess,
    ) -> Result<ExclusivitySubProof, ProofFailure> {
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
                reason: format!("no ownership transfer edge between access {} and access {}", a1.id, a2.id),
            }))
        }
    }

    fn apply_lock_graph(
        &self,
        msg: &ProofMSG,
        a1: &ProofAccess,
        a2: &ProofAccess,
    ) -> Result<ExclusivitySubProof, ProofFailure> {
        if let Some(cycle) = detect_lock_cycle(msg) {
            return Err(ProofFailure::new(ProofFailureReason::LockCycle { locks: cycle }));
        }

        if find_common_lock(msg, a1.id, a2.id).is_some() {
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

fn find_common_lock(msg: &ProofMSG, a1: AccessId, a2: AccessId) -> Option<LockId> {
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

fn find_ordering_path(msg: &ProofMSG, a1: AccessId, a2: AccessId) -> Vec<SyncEdgeId> {
    use std::collections::VecDeque;

    let mut visited = std::collections::HashMap::new();
    let mut queue = VecDeque::new();
    queue.push_back(a1);
    visited.insert(a1, None::<(SyncEdgeId, AccessId)>);

    while let Some(current) = queue.pop_front() {
        if current == a2 {
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

fn has_atomic_sync(msg: &ProofMSG, a1: AccessId, a2: AccessId) -> bool {
    msg.sync_edges.iter().any(|e| {
        e.ordering == SyncOrdering::Atomic
            && ((e.access1 == a1 && e.access2 == a2)
                || (e.access1 == a2 && e.access2 == a1))
    })
}

fn detect_lock_cycle(msg: &ProofMSG) -> Option<Vec<LockId>> {
    let locks: std::collections::HashSet<LockId> = msg
        .sync_edges
        .iter()
        .filter_map(|e| {
            if e.ordering == SyncOrdering::Locked { e.lock } else { None }
        })
        .collect();

    let mut adj: std::collections::HashMap<LockId, std::collections::HashSet<LockId>> =
        std::collections::HashMap::new();
    for &lock in &locks {
        adj.insert(lock, std::collections::HashSet::new());
    }

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

    for lock_list in access_locks.values() {
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
                continue;
            }

            white.remove(&node);
            gray.insert(node);
            dfs_stack.push((node, true));

            if let Some(neighbors) = adj.get(&node) {
                for &neighbor in neighbors {
                    if gray.contains(&neighbor) {
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
pub fn prove_exclusivity(msg: &ProofMSG) -> Result<ExclusivityProof, ProofFailure> {
    let goal = Goal::new(
        InvariantName::Exclusivity,
        Target::FullProgram,
        ProofContext::new("exclusivity::prove"),
    );
    let mut proof = Proof::new(goal);
    let mut sub_proofs: Vec<(AccessId, AccessId, ExclusivitySubProof)> = Vec::new();
    let mut tactics_used: Vec<ExclusivityTactic> = Vec::new();
    let mut fact_id: FactId = 0;

    for i in 0..msg.accesses.len() {
        for j in (i + 1)..msg.accesses.len() {
            let a1 = &msg.accesses[i];
            let a2 = &msg.accesses[j];

            if !conflicts(a1, a2) {
                sub_proofs.push((a1.id, a2.id, ExclusivitySubProof::NoConflict));
                continue;
            }

            if a1.region != a2.region {
                if let Ok(no_alias) = NoAliasProof::prove(msg, a1.derivation_id, a2.derivation_id) {
                    fact_id += 1;
                    proof.add_step(ProofStep::Assume {
                        fact: Fact::checked(
                            fact_id,
                            format!("accesses {} and {} do not alias (different regions)", a1.id, a2.id),
                        ),
                    });
                    sub_proofs.push((a1.id, a2.id, ExclusivitySubProof::NoAlias(no_alias)));
                    continue;
                }
            }

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
                                format!("exclusivity proven for access pair ({}, {}) via {}", a1.id, a2.id, tactic.name()),
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
                        log::debug!("Tactic {} failed for pair ({}, {})", tactic.name(), a1.id, a2.id);
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

    proof.conclude(Conclusion::Proven);

    Ok(ExclusivityProof {
        proof,
        sub_proofs,
        tactics_used,
    })
}
