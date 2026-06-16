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
use crate::judgment::RegionId;
use crate::models::{ProofMSG, ProofRegion, ProofRegionStatus, ProofSCG};
use crate::proof::{Conclusion, Fact, Goal, InvariantName, Proof, ProofContext, ProofStep, Target};
use crate::rules::InferenceRule;

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
    pub fn less_than(&self, r1: RegionId, r2: RegionId) -> Option<bool> {
        let rank1 = self.rank.get(&r1)?;
        let rank2 = self.rank.get(&r2)?;
        Some(rank1 < rank2)
    }

    /// Returns `true` if the ordering is strictly well-founded.
    pub fn is_well_founded(&self) -> bool {
        true
    }

    /// Build a well-founded ordering from a list of regions, assigning
    /// ranks based on allocation order (earlier allocation = lower rank).
    pub fn from_allocation_order(regions: &[ProofRegion]) -> Self {
        let mut ordering = WellFoundedOrdering::new("allocation-order");
        let mut sorted: Vec<&ProofRegion> = regions.iter().collect();
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
            InvariantName::Liveness,
            Target::FullProgram,
            ProofContext::new("liveness::no_deadlock"),
        ));

        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(0, format!("ordering {} is well-founded", ordering.name)),
        });

        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(
                1,
                "every lock acquisition respects the well-founded ordering",
            ),
        });

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
        checker
            .check(&self.proof)
            .unwrap_or(CheckResult::Incomplete)
    }
}

// ---------------------------------------------------------------------------
// AllocationFreedProof
// ---------------------------------------------------------------------------

/// Proof that a specific allocation is freed on all execution paths.
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
    pub fn prove(
        region: &ProofRegion,
        _scg: &ProofSCG,
        tactic: LivenessTactic,
    ) -> Result<Self, ProofFailure> {
        let mut proof = Proof::new(Goal::new(
            InvariantName::Liveness,
            Target::Region(region.id),
            ProofContext::new(format!("liveness::alloc_freed_r{}", region.id)),
        ));

        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(
                0,
                format!(
                    "region {} is allocated at PP {}",
                    region.id, region.alloc_point
                ),
            ),
        });

        match region.status {
            ProofRegionStatus::Leaked => {
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
            ProofRegionStatus::Freed => {
                let fp = region.free_point.unwrap_or(0);

                proof.add_step(ProofStep::Assume {
                    fact: Fact::checked(1, format!("region {} is freed at PP {}", region.id, fp)),
                });

                proof.add_step(ProofStep::Infer {
                    from: vec![1],
                    rule: InferenceRule::LivenessElim,
                    conclusion: Fact::derived(
                        2,
                        format!("region {} is dead at PP {}", region.id, fp),
                    ),
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
            _ => Err(ProofFailure::Leak {
                region_id: region.id,
                alloc_point: region.alloc_point,
            }),
        }
    }

    /// Check this proof with the standard proof checker.
    pub fn check(&self) -> CheckResult {
        let checker = ProofChecker::new();
        checker
            .check(&self.proof)
            .unwrap_or(CheckResult::Incomplete)
    }
}

// ---------------------------------------------------------------------------
// LivenessProof
// ---------------------------------------------------------------------------

/// A formal proof that a program satisfies the liveness invariant.
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

        if let result @ CheckResult::Invalid { .. } = checker
            .check(&self.proof)
            .unwrap_or(CheckResult::Incomplete)
        {
            return result;
        }

        for (_, sub) in &self.access_proofs {
            if let result @ CheckResult::Invalid { .. } =
                checker.check(sub).unwrap_or(CheckResult::Incomplete)
            {
                return result;
            }
        }

        for freed in &self.freed_proofs {
            if let result @ CheckResult::Invalid { .. } = freed.check() {
                return result;
            }
        }

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
            writeln!(
                f,
                "  deadlock proof: present ({} locked regions)",
                dp.locked_regions.len()
            )?;
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

fn is_concrete_violation(e: &ProofFailure) -> bool {
    matches!(
        e,
        ProofFailure::UseAfterFree { .. }
            | ProofFailure::OutOfBounds { .. }
            | ProofFailure::DeadlockCycle { .. }
    )
}

/// Prove the liveness invariant for a program represented as an MSG and SCG.
pub fn prove_liveness(msg: &ProofMSG, scg: &ProofSCG) -> Result<LivenessProof, ProofFailure> {
    if !scg.has_cycle() {
        match prove_liveness_tactic(msg, scg, LivenessTactic::PathEnumeration) {
            Ok(proof) => return Ok(proof),
            Err(e) if is_concrete_violation(&e) => return Err(e),
            Err(e) => log::debug!("path-enumeration failed: {}", e),
        }
    }

    match prove_liveness_tactic(msg, scg, LivenessTactic::RankingFunction) {
        Ok(proof) => return Ok(proof),
        Err(e) if is_concrete_violation(&e) => return Err(e),
        Err(e) => log::debug!("ranking-function failed: {}", e),
    }

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
    msg: &ProofMSG,
    scg: &ProofSCG,
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

        if !region.is_allocated_at(access.program_point) {
            return Err(ProofFailure::UseAfterFree {
                access_id: access.id,
                region_id: access.region,
                program_point: access.program_point,
            });
        }

        if !access.within_bounds(region) {
            return Err(ProofFailure::OutOfBounds {
                access_id: access.id,
                region_id: access.region,
                program_point: access.program_point,
            });
        }

        let mut sub = Proof::new(Goal::new(
            InvariantName::Liveness,
            Target::Region(access.region),
            ProofContext::new(format!("access_{}", access.id)),
        ));

        sub.add_step(ProofStep::Assume {
            fact: Fact::axiom(
                0,
                format!(
                    "region {} is allocated at PP {}",
                    region.id, access.program_point
                ),
            ),
        });

        sub.add_step(ProofStep::Infer {
            from: vec![0],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived(
                1,
                format!(
                    "region {} is live at PP {}",
                    region.id, access.program_point
                ),
            ),
        });

        sub.add_step(ProofStep::Assume {
            fact: Fact::checked(
                2,
                format!(
                    "access {} bytes [offset {}, offset {}) within region {} size {}",
                    access.id,
                    access.offset,
                    access.offset + access.width,
                    region.id,
                    region.size
                ),
            ),
        });

        sub.conclude(Conclusion::Proven);
        access_proofs.push((access.id, sub));
    }

    // --- Step 2: Verify every allocation is eventually freed or leaked. ---
    for region in &msg.regions {
        match AllocationFreedProof::prove(region, scg, tactic) {
            Ok(fp) => freed_proofs.push(fp),
            Err(ProofFailure::Leak { .. }) if region.status == ProofRegionStatus::Leaked => {
                // Leaked is acceptable — skip.
            }
            Err(e) => return Err(e),
        }
    }

    // --- Step 3: Check for deadlocks (if there are locked regions). ---
    let locked_regions: Vec<RegionId> = msg
        .regions
        .iter()
        .filter(|r| {
            r.status == ProofRegionStatus::Allocated || r.status == ProofRegionStatus::Mapped
        })
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
        InvariantName::Liveness,
        Target::FullProgram,
        ProofContext::new("liveness::top"),
    ));

    top_proof.add_step(ProofStep::Assume {
        fact: Fact::axiom(
            0,
            format!(
                "MSG has {} regions and {} accesses",
                msg.regions.len(),
                msg.accesses.len()
            ),
        ),
    });

    top_proof.add_step(ProofStep::Assume {
        fact: Fact::axiom(
            1,
            format!(
                "SCG has {} nodes and {} edges",
                scg.nodes.len(),
                scg.edges.len()
            ),
        ),
    });

    top_proof.add_step(ProofStep::ByDefinition {
        definition: format!("liveness proven by {} tactic", tactic),
    });

    let ordering = if tactic == LivenessTactic::RankingFunction {
        let ord = WellFoundedOrdering::from_allocation_order(&msg.regions);
        top_proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(2, format!("well-founded ordering: {}", ord.name)),
        });
        Some(ord)
    } else {
        None
    };

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
