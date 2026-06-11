//! # Cleanup Proof Objects
//!
//! Formal proof objects that certify cleanup invariants for VUMA programs.
//! The cleanup discipline ensures that every resource acquired during execution
//! is eventually released, that no resource is freed more than once, and that
//! no resource is accessed after it has been freed.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

use crate::judgment::RegionId;
use crate::models::{
    ProofMemOp, ProofMemOpKind, ProofMSG, ProofSCG,
};
use crate::proof::{
    Conclusion, Fact, FactId, Goal, InvariantName, ProgramPoint, Proof, ProofContext, ProofStep,
    Target,
};

// ---------------------------------------------------------------------------
// Proof Failure
// ---------------------------------------------------------------------------

/// Describes why a cleanup proof could not be established.
#[derive(Debug, Clone, Error)]
pub enum ProofFailure {
    #[error("region {region} allocated at 0x{alloc_point:x} is never freed on path {path_id}")]
    LeakedResource {
        region: RegionId,
        alloc_point: ProgramPoint,
        path_id: usize,
    },

    #[error("region {region} freed multiple times: at {free_points:?}")]
    DoubleFree {
        region: RegionId,
        free_points: Vec<ProgramPoint>,
    },

    #[error("region {region} accessed at 0x{access_point:x} after free at 0x{free_point:x}")]
    UseAfterFree {
        region: RegionId,
        access_point: ProgramPoint,
        free_point: ProgramPoint,
    },

    #[error("SCG has no exit points; cleanup cannot be verified")]
    NoExitPoints,

    #[error("internal proof error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// CleanupProof
// ---------------------------------------------------------------------------

/// Proof that every allocated resource is eventually released along all
/// execution paths through the program.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CleanupProof {
    pub proof: Proof,
    pub release_map: HashMap<RegionId, ReleaseInfo>,
    pub tactic: CleanupTactic,
}

/// Information about how a region is released.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub alloc_point: ProgramPoint,
    pub free_points: Vec<ProgramPoint>,
}

impl CleanupProof {
    /// Verify that this proof covers all regions in the given MSG.
    pub fn covers_all_regions(&self, msg: &ProofMSG) -> bool {
        let allocated: HashSet<RegionId> = msg.allocs().iter().map(|op| op.region).collect();
        let covered: HashSet<RegionId> = self.release_map.keys().copied().collect();
        allocated == covered
    }
}

// ---------------------------------------------------------------------------
// NoDoubleFreeProof
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoDoubleFreeProof {
    pub proof: Proof,
    pub free_map: HashMap<RegionId, ProgramPoint>,
    pub tactic: CleanupTactic,
}

// ---------------------------------------------------------------------------
// NoUseAfterFreeProof
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoUseAfterFreeProof {
    pub proof: Proof,
    pub lifetime_map: HashMap<RegionId, RegionLifetime>,
    pub tactic: CleanupTactic,
}

/// The lifetime information for a region.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegionLifetime {
    pub free_point: ProgramPoint,
    pub live_access_points: Vec<ProgramPoint>,
}

// ---------------------------------------------------------------------------
// CleanupTactic
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CleanupTactic {
    PathEnumeration,
    OwnershipTracking,
    LifetimeAnalysis,
}

impl CleanupTactic {
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

pub fn prove_cleanup(msg: &ProofMSG, scg: &ProofSCG) -> Result<CleanupProof, ProofFailure> {
    prove_cleanup_with_tactic(msg, scg, CleanupTactic::PathEnumeration)
}

pub fn prove_cleanup_with_tactic(
    msg: &ProofMSG,
    scg: &ProofSCG,
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

pub fn prove_no_double_free(msg: &ProofMSG, scg: &ProofSCG) -> Result<NoDoubleFreeProof, ProofFailure> {
    prove_no_double_free_with_tactic(msg, scg, CleanupTactic::OwnershipTracking)
}

pub fn prove_no_double_free_with_tactic(
    msg: &ProofMSG,
    _scg: &ProofSCG,
    tactic: CleanupTactic,
) -> Result<NoDoubleFreeProof, ProofFailure> {
    let mut free_map: HashMap<RegionId, ProgramPoint> = HashMap::new();

    let goal = Goal::new(
        InvariantName::Cleanup,
        Target::FullProgram,
        ProofContext::new("cleanup::no_double_free"),
    );
    let mut proof = Proof::new(goal);

    for (fact_id, op) in (0_u64..).zip(msg.frees()) {
        let stmt = format!("region {} freed at 0x{:x}", op.region, op.location);
        proof.add_step(ProofStep::Assume {
            fact: Fact::checked(fact_id, &stmt),
        });

        if let Some(&prev_point) = free_map.get(&op.region) {
            proof.add_step(ProofStep::Contradiction {
                assumption: fact_id,
                negation: fact_id,
            });
            proof.conclude(Conclusion::Refuted);
            return Err(ProofFailure::DoubleFree {
                region: op.region,
                free_points: vec![prev_point, op.location],
            });
        }
        free_map.insert(op.region, op.location);
    }

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

pub fn prove_no_use_after_free(
    msg: &ProofMSG,
    scg: &ProofSCG,
) -> Result<NoUseAfterFreeProof, ProofFailure> {
    prove_no_use_after_free_with_tactic(msg, scg, CleanupTactic::LifetimeAnalysis)
}

pub fn prove_no_use_after_free_with_tactic(
    msg: &ProofMSG,
    _scg: &ProofSCG,
    tactic: CleanupTactic,
) -> Result<NoUseAfterFreeProof, ProofFailure> {
    let goal = Goal::new(
        InvariantName::Cleanup,
        Target::FullProgram,
        ProofContext::new("cleanup::no_use_after_free"),
    );
    let mut proof = Proof::new(goal);
    let mut fact_id: FactId = 0;
    let mut lifetime_map: HashMap<RegionId, RegionLifetime> = HashMap::new();

    let mut region_free_points: HashMap<RegionId, ProgramPoint> = HashMap::new();
    for op in msg.frees() {
        region_free_points.insert(op.region, op.location);
    }

    let accesses: Vec<&ProofMemOp> = msg
        .ops
        .iter()
        .filter(|op| op.kind == ProofMemOpKind::Read || op.kind == ProofMemOpKind::Write)
        .collect();

    for access in &accesses {
        if let Some(&free_point) = region_free_points.get(&access.region) {
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
            proof.add_step(ProofStep::Assume {
                fact: Fact::checked(
                    fact_id,
                    format!("region {} accessed at 0x{:x} (region not freed)", access.region, access.location),
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
// Tactic implementations
// ---------------------------------------------------------------------------

fn prove_via_path_enumeration(
    msg: &ProofMSG,
    scg: &ProofSCG,
) -> Result<CleanupProof, ProofFailure> {
    let goal = Goal::new(
        InvariantName::Cleanup,
        Target::FullProgram,
        ProofContext::new("cleanup::path_enumeration"),
    );
    let mut proof_obj = Proof::new(goal);
    let mut release_map: HashMap<RegionId, ReleaseInfo> = HashMap::new();

    let _ndf = prove_no_double_free(msg, scg)?;
    let _nuaf = prove_no_use_after_free(msg, scg)?;

    let paths = scg.enumerate_paths(64);
    let alloc_regions: HashSet<RegionId> = msg.allocs().iter().map(|op| op.region).collect();

    for (fact_id, region) in (0_u64..).zip(alloc_regions.iter()) {
        let alloc_pts = msg.alloc_points(*region);
        let free_pts = msg.free_points(*region);

        if let Some(&alloc_point) = alloc_pts.first() {
            release_map.insert(
                *region,
                ReleaseInfo {
                    alloc_point,
                    free_points: free_pts.clone(),
                },
            );
        }

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

fn prove_via_ownership_tracking(
    msg: &ProofMSG,
    scg: &ProofSCG,
) -> Result<CleanupProof, ProofFailure> {
    let goal = Goal::new(
        InvariantName::Cleanup,
        Target::FullProgram,
        ProofContext::new("cleanup::ownership_tracking"),
    );
    let mut proof_obj = Proof::new(goal);
    let mut fact_id: FactId = 0;
    let mut release_map: HashMap<RegionId, ReleaseInfo> = HashMap::new();

    let _ndf = prove_no_double_free(msg, scg)?;
    let _nuaf = prove_no_use_after_free(msg, scg)?;

    let mut allocated: HashSet<RegionId> = HashSet::new();
    let mut access_owned: HashSet<RegionId> = HashSet::new();

    let mut sorted_ops = msg.ops.clone();
    sorted_ops.sort_by_key(|op| op.location);

    for op in &sorted_ops {
        match op.kind {
            ProofMemOpKind::Alloc => {
                allocated.insert(op.region);
                proof_obj.add_step(ProofStep::Assume {
                    fact: Fact::checked(
                        fact_id,
                        format!("region {} allocated at 0x{:x}", op.region, op.location),
                    ),
                });
                fact_id += 1;
            }
            ProofMemOpKind::Free => {
                if !allocated.contains(&op.region) {
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
            ProofMemOpKind::Acquire => {
                access_owned.insert(op.region);
                proof_obj.add_step(ProofStep::Assume {
                    fact: Fact::checked(
                        fact_id,
                        format!("region {} access ownership acquired at 0x{:x}", op.region, op.location),
                    ),
                });
                fact_id += 1;
            }
            ProofMemOpKind::Release => {
                access_owned.remove(&op.region);
                proof_obj.add_step(ProofStep::Assume {
                    fact: Fact::checked(
                        fact_id,
                        format!("region {} access ownership released at 0x{:x}", op.region, op.location),
                    ),
                });
                fact_id += 1;
            }
            ProofMemOpKind::Read | ProofMemOpKind::Write => {}
        }
    }

    if !allocated.is_empty() {
        if let Some(&region) = allocated.iter().next() {
            let alloc_pts = msg.alloc_points(region);
            proof_obj.conclude(Conclusion::Refuted);
            return Err(ProofFailure::LeakedResource {
                region,
                alloc_point: alloc_pts.first().copied().unwrap_or(0),
                path_id: 0,
            });
        }
    }

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

fn prove_via_lifetime_analysis(
    msg: &ProofMSG,
    scg: &ProofSCG,
) -> Result<CleanupProof, ProofFailure> {
    let goal = Goal::new(
        InvariantName::Cleanup,
        Target::FullProgram,
        ProofContext::new("cleanup::lifetime_analysis"),
    );
    let mut proof_obj = Proof::new(goal);
    let mut fact_id: FactId = 0;
    let mut release_map: HashMap<RegionId, ReleaseInfo> = HashMap::new();

    let _ndf = prove_no_double_free(msg, scg)?;
    let nuaf = prove_no_use_after_free(msg, scg)?;

    let all_regions = msg.all_regions();
    for region in &all_regions {
        let alloc_pts = msg.alloc_points(*region);
        let free_pts = msg.free_points(*region);

        if alloc_pts.is_empty() {
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

        proof_obj.add_step(ProofStep::Assume {
            fact: Fact::checked(
                fact_id,
                format!(
                    "region {} lifetime: [0x{:x}, 0x{:x}]",
                    region, alloc_pts[0], free_pts[0]
                ),
            ),
        });
        fact_id += 1;
    }

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
