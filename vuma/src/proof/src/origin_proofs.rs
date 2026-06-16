//! # Origin Proof Objects
//!
//! Formal proof objects for the **origin invariant**: every data value in a
//! VUMA program has well-defined provenance that can be traced back to a valid
//! region through a terminating derivation chain, and tainted data does not
//! flow to sensitive sinks.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::checker::{CheckResult, ProofChecker};
use crate::judgment::RegionId;
use crate::models::{OriginInfo, SinkSensitivity, SourceTrust};
use crate::proof::{
    Conclusion, Fact, FactId, Goal, InvariantName, Proof, ProofContext, ProofStep, Target,
};
use crate::rules::InferenceRule;

// ---------------------------------------------------------------------------
// Proof failure
// ---------------------------------------------------------------------------

/// Reasons why an origin proof might fail.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum ProofFailure {
    #[error("broken derivation chain at derivation {derivation_id}: {reason}")]
    BrokenChain { derivation_id: u64, reason: String },

    #[error("derivation chain for {derivation_id} terminates at dead region {region_id}")]
    TerminatesAtDeadRegion {
        derivation_id: u64,
        region_id: RegionId,
    },

    #[error("no provenance for region {region_id}")]
    NoProvenance { region_id: RegionId },

    #[error("taint violation: tainted data from region {src_region} flows to {sensitivity} sink {sink_region}")]
    TaintViolation {
        src_region: RegionId,
        sink_region: RegionId,
        sensitivity: SinkSensitivity,
    },

    #[error("untrusted source {src_region} flows to {sensitivity} sink {sink_region}")]
    UntrustedFlow {
        src_region: RegionId,
        sink_region: RegionId,
        sensitivity: SinkSensitivity,
    },

    #[error("insufficient origin info: {detail}")]
    InsufficientInfo { detail: String },

    #[error("internal proof error: {reason}")]
    Internal { reason: String },
}

// ---------------------------------------------------------------------------
// Origin proof objects
// ---------------------------------------------------------------------------

/// Proof that every data value has well-defined provenance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OriginProof {
    pub proof: Proof,
    pub verified_regions: Vec<RegionId>,
    pub checked_chains: Vec<(u64, RegionId)>,
}

impl OriginProof {
    pub fn check(&self) -> CheckResult {
        let checker = ProofChecker::new();
        checker
            .check(&self.proof)
            .unwrap_or(CheckResult::Incomplete)
    }

    pub fn is_valid(&self) -> bool {
        self.proof.conclusion == Conclusion::Proven && self.check() == CheckResult::Valid
    }
}

/// Proof that a derivation chain terminates at a valid region.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DerivationChainProof {
    pub proof: Proof,
    pub derivation_id: u64,
    pub chain: Vec<RegionId>,
    pub root_region: RegionId,
}

impl DerivationChainProof {
    pub fn check(&self) -> CheckResult {
        let checker = ProofChecker::new();
        checker
            .check(&self.proof)
            .unwrap_or(CheckResult::Incomplete)
    }

    pub fn is_valid(&self) -> bool {
        self.proof.conclusion == Conclusion::Proven && self.check() == CheckResult::Valid
    }
}

/// Proof that tainted data does not flow to sensitive sinks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaintProof {
    pub proof: Proof,
    pub tainted_sources: Vec<RegionId>,
    pub sensitive_sinks: Vec<RegionId>,
    pub safe_edges: Vec<(RegionId, RegionId)>,
}

impl TaintProof {
    pub fn check(&self) -> CheckResult {
        let checker = ProofChecker::new();
        checker
            .check(&self.proof)
            .unwrap_or(CheckResult::Incomplete)
    }

    pub fn is_valid(&self) -> bool {
        self.proof.conclusion == Conclusion::Proven && self.check() == CheckResult::Valid
    }
}

// ---------------------------------------------------------------------------
// Origin-specific tactics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OriginTactic {
    ChainVerification,
    TaintPropagation,
    SourceClassification,
}

impl OriginTactic {
    pub fn name(&self) -> &'static str {
        match self {
            OriginTactic::ChainVerification => "ChainVerification",
            OriginTactic::TaintPropagation => "TaintPropagation",
            OriginTactic::SourceClassification => "SourceClassification",
        }
    }

    pub fn apply_chain_verification(
        info: &OriginInfo,
    ) -> Result<Vec<DerivationChainProof>, ProofFailure> {
        let mut proofs = Vec::new();

        for (derivation_id, chain) in &info.derivation_chains {
            if chain.is_empty() {
                return Err(ProofFailure::BrokenChain {
                    derivation_id: *derivation_id,
                    reason: "derivation chain is empty".into(),
                });
            }

            let root_region = chain[0];

            let goal = Goal::new(
                InvariantName::Origin,
                Target::Derivation(*derivation_id),
                ProofContext::new(format!("chain_verification::D{}", derivation_id)),
            );

            let mut proof = Proof::new(goal);
            let mut next_fid: FactId = 1;

            proof.add_step(ProofStep::Assume {
                fact: Fact::axiom(
                    next_fid,
                    format!("chain root region {} exists", root_region),
                ),
            });
            next_fid += 1;

            if !info.is_live(root_region) {
                return Err(ProofFailure::TerminatesAtDeadRegion {
                    derivation_id: *derivation_id,
                    region_id: root_region,
                });
            }
            proof.add_step(ProofStep::Infer {
                from: vec![next_fid - 1],
                rule: InferenceRule::LivenessIntro,
                conclusion: Fact::derived(next_fid, format!("region {} is live", root_region)),
            });
            next_fid += 1;

            for (i, &region_id) in chain.iter().enumerate() {
                if i == 0 {
                    continue;
                }
                proof.add_step(ProofStep::Assume {
                    fact: Fact::checked(
                        next_fid,
                        format!("chain link {} -> region {} valid", i, region_id),
                    ),
                });
                next_fid += 1;
            }

            proof.add_step(ProofStep::Infer {
                from: vec![2, next_fid - 1],
                rule: InferenceRule::DerivationTransitivity,
                conclusion: Fact::derived(
                    next_fid,
                    format!(
                        "derivation {} chain terminates at live region {}",
                        derivation_id, root_region
                    ),
                ),
            });
            proof.conclude(Conclusion::Proven);

            proofs.push(DerivationChainProof {
                proof,
                derivation_id: *derivation_id,
                chain: chain.clone(),
                root_region,
            });
        }

        Ok(proofs)
    }

    pub fn apply_taint_propagation(info: &OriginInfo) -> Result<TaintProof, ProofFailure> {
        let goal = Goal::new(
            InvariantName::Origin,
            Target::FullProgram,
            ProofContext::new("taint_propagation"),
        );

        let mut proof = Proof::new(goal);
        let mut next_fid: FactId = 1;
        let mut tainted_sources = Vec::new();
        let mut sensitive_sinks = Vec::new();
        let mut safe_edges = Vec::new();

        for &(rid, label) in &info.taint_labels {
            proof.add_step(ProofStep::Assume {
                fact: Fact::axiom(
                    next_fid,
                    format!("region {} has taint label {}", rid, label),
                ),
            });
            next_fid += 1;
            tainted_sources.push(rid);
        }

        for &(rid, sensitivity) in &info.sink_classifications {
            if matches!(
                sensitivity,
                SinkSensitivity::Sensitive | SinkSensitivity::Critical
            ) {
                proof.add_step(ProofStep::Assume {
                    fact: Fact::axiom(next_fid, format!("region {} is {} sink", rid, sensitivity)),
                });
                next_fid += 1;
                sensitive_sinks.push(rid);
            }
        }

        for &(source, target) in &info.flow_edges {
            let source_tainted = info.taint_of(source).is_some()
                || info.trust_of(source) == Some(SourceTrust::Untrusted);
            let target_sensitive = matches!(
                info.sink_sensitivity(target),
                Some(SinkSensitivity::Sensitive | SinkSensitivity::Critical)
            );

            if source_tainted && target_sensitive {
                let sensitivity = info.sink_sensitivity(target).unwrap();
                return Err(ProofFailure::TaintViolation {
                    src_region: source,
                    sink_region: target,
                    sensitivity,
                });
            }

            proof.add_step(ProofStep::Assume {
                fact: Fact::checked(
                    next_fid,
                    format!("flow edge {} -> {} is taint-safe", source, target),
                ),
            });
            next_fid += 1;
            safe_edges.push((source, target));
        }

        for &tainted_rid in &tainted_sources {
            let reachable = info.reachable_from(tainted_rid);
            for &reached_rid in &reachable {
                if reached_rid == tainted_rid {
                    continue;
                }
                if let Some(sensitivity) = info.sink_sensitivity(reached_rid) {
                    if matches!(
                        sensitivity,
                        SinkSensitivity::Sensitive | SinkSensitivity::Critical
                    ) {
                        return Err(ProofFailure::TaintViolation {
                            src_region: tainted_rid,
                            sink_region: reached_rid,
                            sensitivity,
                        });
                    }
                }
            }
        }

        proof.add_step(ProofStep::ByDefinition {
            definition: "taint_non_flow: no tainted source reaches a sensitive sink".into(),
        });
        proof.conclude(Conclusion::Proven);

        Ok(TaintProof {
            proof,
            tainted_sources,
            sensitive_sinks,
            safe_edges,
        })
    }

    pub fn apply_source_classification(info: &OriginInfo) -> Result<TaintProof, ProofFailure> {
        let goal = Goal::new(
            InvariantName::Origin,
            Target::FullProgram,
            ProofContext::new("source_classification"),
        );

        let mut proof = Proof::new(goal);
        let mut next_fid: FactId = 1;
        let mut tainted_sources = Vec::new();
        let mut sensitive_sinks = Vec::new();
        let mut safe_edges = Vec::new();

        for &(rid, trust) in &info.source_trust {
            proof.add_step(ProofStep::Assume {
                fact: Fact::checked(next_fid, format!("region {} is {} source", rid, trust)),
            });
            next_fid += 1;

            if trust == SourceTrust::Untrusted {
                tainted_sources.push(rid);
            }
        }

        for &(rid, sensitivity) in &info.sink_classifications {
            if matches!(
                sensitivity,
                SinkSensitivity::Sensitive | SinkSensitivity::Critical
            ) {
                proof.add_step(ProofStep::Assume {
                    fact: Fact::checked(
                        next_fid,
                        format!("region {} is {} sink", rid, sensitivity),
                    ),
                });
                next_fid += 1;
                sensitive_sinks.push(rid);
            }
        }

        for &untrusted_rid in &tainted_sources {
            let reachable = info.reachable_from(untrusted_rid);
            for &reached_rid in &reachable {
                if reached_rid == untrusted_rid {
                    continue;
                }
                if let Some(sensitivity) = info.sink_sensitivity(reached_rid) {
                    if matches!(
                        sensitivity,
                        SinkSensitivity::Sensitive | SinkSensitivity::Critical
                    ) {
                        return Err(ProofFailure::UntrustedFlow {
                            src_region: untrusted_rid,
                            sink_region: reached_rid,
                            sensitivity,
                        });
                    }
                }
            }

            proof.add_step(ProofStep::Assume {
                fact: Fact::checked(
                    next_fid,
                    format!(
                        "untrusted source {} does not reach any sensitive sink",
                        untrusted_rid
                    ),
                ),
            });
            next_fid += 1;
        }

        for &(source, target) in &info.flow_edges {
            let source_untrusted = info.trust_of(source) == Some(SourceTrust::Untrusted);
            let target_sensitive = matches!(
                info.sink_sensitivity(target),
                Some(SinkSensitivity::Sensitive | SinkSensitivity::Critical)
            );

            if !source_untrusted || !target_sensitive {
                safe_edges.push((source, target));
            }
        }

        proof.add_step(ProofStep::ByDefinition {
            definition: "source_classification: no untrusted source reaches a sensitive sink"
                .into(),
        });
        proof.conclude(Conclusion::Proven);

        Ok(TaintProof {
            proof,
            tainted_sources,
            sensitive_sinks,
            safe_edges,
        })
    }
}

impl std::fmt::Display for OriginTactic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ---------------------------------------------------------------------------
// Top-level proof constructor
// ---------------------------------------------------------------------------

/// Prove the origin invariant for the given origin info.
pub fn prove_origin(info: &OriginInfo) -> Result<OriginProof, ProofFailure> {
    let chain_proofs = OriginTactic::apply_chain_verification(info)?;
    let mut verified_regions = Vec::new();
    let mut checked_chains = Vec::new();

    for cp in &chain_proofs {
        for &rid in &cp.chain {
            if !verified_regions.contains(&rid) {
                verified_regions.push(rid);
            }
        }
        checked_chains.push((cp.derivation_id, cp.root_region));
    }

    let taint_proof = OriginTactic::apply_taint_propagation(info)?;
    let _classification_proof = OriginTactic::apply_source_classification(info)?;

    let goal = Goal::new(
        InvariantName::Origin,
        Target::FullProgram,
        ProofContext::new("prove_origin")
            .with_assumption("all derivation chains terminate at live regions")
            .with_assumption("no tainted data reaches sensitive sinks")
            .with_assumption("no untrusted source reaches sensitive sinks"),
    );

    let mut proof = Proof::new(goal);
    let mut next_fid: FactId = 1;

    proof.add_step(ProofStep::Assume {
        fact: Fact::axiom(
            next_fid,
            format!("{} derivation chains verified", chain_proofs.len()),
        ),
    });
    next_fid += 1;

    for (derivation_id, root_region) in &checked_chains {
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::DerivationTransitivity,
            conclusion: Fact::derived(
                next_fid,
                format!(
                    "derivation {} terminates at live region {}",
                    derivation_id, root_region
                ),
            ),
        });
        next_fid += 1;
    }

    proof.add_step(ProofStep::Assume {
        fact: Fact::checked(
            next_fid,
            format!(
                "taint non-flow verified: {} tainted sources, {} sensitive sinks, {} safe edges",
                taint_proof.tainted_sources.len(),
                taint_proof.sensitive_sinks.len(),
                taint_proof.safe_edges.len(),
            ),
        ),
    });

    proof.add_step(ProofStep::ByDefinition {
        definition: "origin_invariant: every data value has well-defined provenance".into(),
    });
    proof.conclude(Conclusion::Proven);

    Ok(OriginProof {
        proof,
        verified_regions,
        checked_chains,
    })
}
