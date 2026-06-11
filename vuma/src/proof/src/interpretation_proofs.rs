//! # Interpretation Proof Objects
//!
//! Formal proof objects for the VUMA Interpretation Invariant (Invariant 3):
//! every access respects the Representation Descriptor (RepD) of its target.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::models::{
    valid_reinterpretation, Compatibility, DerivationId, ProofAccess, ProofMSG, RepDId,
};
use crate::proof::{
    AccessId, Conclusion, Fact, FactId, Goal, InvariantName, Proof, ProofContext, ProofStep, Target,
};
use crate::rules::InferenceRule;

// ---------------------------------------------------------------------------
// Proof Failure
// ---------------------------------------------------------------------------

/// A failure to prove the interpretation invariant, with diagnostic info.
#[derive(Debug, Clone, Error)]
pub enum ProofFailure {
    #[error("incompatible BD for access {access_id}: {reason}")]
    IncompatibleBD { access_id: AccessId, reason: String },

    #[error("unsafe reinterpretation at derivation {derivation_id}: {reason}")]
    UnsafeReinterpretation {
        derivation_id: DerivationId,
        reason: String,
    },

    #[error("size/alignment violation at derivation {derivation_id}: {reason}")]
    SizeAlignmentViolation {
        derivation_id: DerivationId,
        reason: String,
    },

    #[error("unresolvable derivation {derivation_id}: {reason}")]
    UnresolvableDerivation {
        derivation_id: DerivationId,
        reason: String,
    },

    #[error("uninitialized pointer read at access {access_id}")]
    UninitializedPointerRead { access_id: AccessId },

    #[error("internal proof error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// BDCompatibilityProof
// ---------------------------------------------------------------------------

/// Proof that a specific write-read pair has compatible BDs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BDCompatibilityProof {
    pub write_access_id: AccessId,
    pub read_access_id: AccessId,
    pub write_repd: RepDId,
    pub read_repd: RepDId,
    pub read_addr: u64,
    pub compatibility: Compatibility,
    pub proof: Proof,
}

impl BDCompatibilityProof {
    pub fn new(
        write_access_id: AccessId,
        read_access_id: AccessId,
        write_repd: RepDId,
        read_repd: RepDId,
        read_addr: u64,
        compatibility: Compatibility,
        proof: Proof,
    ) -> Self {
        Self {
            write_access_id,
            read_access_id,
            write_repd,
            read_repd,
            read_addr,
            compatibility,
            proof,
        }
    }

    pub fn is_compatible(&self) -> bool {
        self.compatibility.is_compatible()
    }
}

// ---------------------------------------------------------------------------
// ReinterpretationSafetyProof
// ---------------------------------------------------------------------------

/// Proof that a cast derivation is a safe reinterpretation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReinterpretationSafetyProof {
    pub derivation_id: DerivationId,
    pub source_repd: RepDId,
    pub target_repd: RepDId,
    pub size_ok: bool,
    pub alignment_ok: bool,
    pub reinterpretation_ok: bool,
    pub proof: Proof,
}

impl ReinterpretationSafetyProof {
    pub fn new(
        derivation_id: DerivationId,
        source_repd: RepDId,
        target_repd: RepDId,
        size_ok: bool,
        alignment_ok: bool,
        reinterpretation_ok: bool,
        proof: Proof,
    ) -> Self {
        Self {
            derivation_id,
            source_repd,
            target_repd,
            size_ok,
            alignment_ok,
            reinterpretation_ok,
            proof,
        }
    }

    pub fn is_safe(&self) -> bool {
        self.size_ok && self.alignment_ok && self.reinterpretation_ok
    }
}

// ---------------------------------------------------------------------------
// InterpretationProof
// ---------------------------------------------------------------------------

/// Top-level proof that the interpretation invariant holds for an entire MSG.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InterpretationProof {
    pub bd_compatibility_proofs: Vec<BDCompatibilityProof>,
    pub reinterpretation_safety_proofs: Vec<ReinterpretationSafetyProof>,
    pub proof: Proof,
}

impl InterpretationProof {
    pub fn is_valid(&self) -> bool {
        self.bd_compatibility_proofs
            .iter()
            .all(|p| p.is_compatible())
            && self
                .reinterpretation_safety_proofs
                .iter()
                .all(|p| p.is_safe())
    }
}

// ---------------------------------------------------------------------------
// Interpretation Tactics
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum InterpTactic {
    BDTracing,
    CompatibilityChecking,
    SizeAlignmentVerification,
}

impl InterpTactic {
    pub fn name(&self) -> &'static str {
        match self {
            InterpTactic::BDTracing => "BDTracing",
            InterpTactic::CompatibilityChecking => "CompatibilityChecking",
            InterpTactic::SizeAlignmentVerification => "SizeAlignmentVerification",
        }
    }
}

impl std::fmt::Display for InterpTactic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ---------------------------------------------------------------------------
// Prover: prove_interpretation
// ---------------------------------------------------------------------------

/// Prove the interpretation invariant for the given MSG.
pub fn prove_interpretation(msg: &ProofMSG) -> Result<InterpretationProof, ProofFailure> {
    let mut fact_id: FactId = 0;
    let mut next_fact_id = || -> FactId {
        let id = fact_id;
        fact_id += 1;
        id
    };

    let goal = Goal::new(
        InvariantName::Interpretation,
        Target::FullProgram,
        ProofContext::new("interpretation_prover"),
    );
    let mut top_proof = Proof::new(goal);

    // Tactic 1: BD-tracing
    let mut access_repd_map: std::collections::HashMap<AccessId, RepDId> =
        std::collections::HashMap::new();

    for access in &msg.accesses {
        let effective_repd = msg.repd_of(access.target_derivation).ok_or_else(|| {
            ProofFailure::UnresolvableDerivation {
                derivation_id: access.target_derivation,
                reason: format!(
                    "cannot resolve effective RepD for derivation {} referenced by access {}",
                    access.target_derivation, access.id
                ),
            }
        })?;

        let fid = next_fact_id();
        let fact = Fact::checked(
            fid,
            format!(
                "access {} has effective RepD {} (via derivation {})",
                access.id, effective_repd, access.target_derivation
            ),
        );
        top_proof.add_step(ProofStep::Assume { fact });
        access_repd_map.insert(access.id, effective_repd);
    }

    // Tactic 2: Compatibility-checking
    let mut bd_proofs: Vec<BDCompatibilityProof> = Vec::new();

    let writes: Vec<&ProofAccess> = msg.accesses.iter().filter(|a| a.is_write()).collect();
    let reads: Vec<&ProofAccess> = msg.accesses.iter().filter(|a| a.is_read()).collect();

    for write_access in &writes {
        let write_region = msg
            .region_of(write_access.target_derivation)
            .ok_or_else(|| ProofFailure::UnresolvableDerivation {
                derivation_id: write_access.target_derivation,
                reason: format!("cannot resolve region for write access {}", write_access.id),
            })?;

        let write_addr = msg.addr_of(write_access.target_derivation).ok_or_else(|| {
            ProofFailure::UnresolvableDerivation {
                derivation_id: write_access.target_derivation,
                reason: format!(
                    "cannot resolve address for write access {}",
                    write_access.id
                ),
            }
        })?;

        let write_end = write_addr + write_access.size;

        for read_access in &reads {
            let read_region = match msg.region_of(read_access.target_derivation) {
                Some(r) => r,
                None => continue,
            };

            if read_region != write_region {
                continue;
            }

            let read_addr = match msg.addr_of(read_access.target_derivation) {
                Some(a) => a,
                None => continue,
            };
            let read_end = read_addr + read_access.size;

            if read_addr >= write_end || write_addr >= read_end {
                continue;
            }

            let write_repd_id =
                access_repd_map
                    .get(&write_access.id)
                    .copied()
                    .ok_or_else(|| {
                        ProofFailure::Internal(format!(
                            "BD-tracing did not produce a RepD for write access {}",
                            write_access.id
                        ))
                    })?;

            let read_repd_id = access_repd_map
                .get(&read_access.id)
                .copied()
                .ok_or_else(|| {
                    ProofFailure::Internal(format!(
                        "BD-tracing did not produce a RepD for read access {}",
                        read_access.id
                    ))
                })?;

            let write_repd = msg.get_repd(write_repd_id).ok_or_else(|| {
                ProofFailure::Internal(format!("RepD {} not found in MSG", write_repd_id))
            })?;

            let read_repd = msg.get_repd(read_repd_id).ok_or_else(|| {
                ProofFailure::Internal(format!("RepD {} not found in MSG", read_repd_id))
            })?;

            let compat = write_repd.compatible_with(read_repd, read_addr);

            let compat_goal = Goal::new(
                InvariantName::Interpretation,
                Target::Access(read_access.id),
                ProofContext::new(format!(
                    "compatibility_check::write{}_read{}",
                    write_access.id, read_access.id
                )),
            );
            let mut compat_proof = Proof::new(compat_goal);

            let f1 = Fact::axiom(
                next_fact_id(),
                format!(
                    "write access {} has RepD {} (kind={})",
                    write_access.id, write_repd.id, write_repd.kind
                ),
            );
            let f1_id = f1.id;
            compat_proof.add_step(ProofStep::Assume { fact: f1 });

            let f2 = Fact::axiom(
                next_fact_id(),
                format!(
                    "read access {} expects RepD {} (kind={})",
                    read_access.id, read_repd.id, read_repd.kind
                ),
            );
            let f2_id = f2.id;
            compat_proof.add_step(ProofStep::Assume { fact: f2 });

            if compat.is_compatible() {
                let f3 = Fact::derived(
                    next_fact_id(),
                    format!(
                        "BD of write {} is compatible with BD of read {}",
                        write_access.id, read_access.id
                    ),
                );
                compat_proof.add_step(ProofStep::Infer {
                    from: vec![f1_id, f2_id],
                    rule: InferenceRule::CastValidity,
                    conclusion: f3,
                });
                compat_proof.conclude(Conclusion::Proven);
            } else {
                return Err(ProofFailure::IncompatibleBD {
                    access_id: read_access.id,
                    reason: format!(
                        "write RepD {} incompatible with read RepD {}: {:?}",
                        write_repd.id, read_repd.id, compat
                    ),
                });
            }

            bd_proofs.push(BDCompatibilityProof::new(
                write_access.id,
                read_access.id,
                write_repd_id,
                read_repd_id,
                read_addr,
                compat,
                compat_proof,
            ));
        }
    }

    let compat_fact = Fact::derived(
        next_fact_id(),
        format!(
            "all {} write-read pairs have compatible BDs",
            bd_proofs.len()
        ),
    );
    top_proof.add_step(ProofStep::Infer {
        from: vec![],
        rule: InferenceRule::CastValidity,
        conclusion: compat_fact,
    });

    // Tactic 3: Size-alignment-verification
    let mut reinterpret_proofs: Vec<ReinterpretationSafetyProof> = Vec::new();

    for derivation in &msg.derivations {
        if !derivation.is_cast() {
            continue;
        }

        let target_repd_id = derivation.cast.ok_or_else(|| {
            ProofFailure::Internal(format!(
                "cast derivation {} has no target RepD",
                derivation.id
            ))
        })?;

        let source_repd_id = if let Some(src_did) = derivation.source_derivation {
            msg.repd_of(src_did)
                .ok_or_else(|| ProofFailure::UnresolvableDerivation {
                    derivation_id: src_did,
                    reason: format!(
                        "cannot resolve source RepD for cast derivation {}",
                        derivation.id
                    ),
                })?
        } else if let Some(src_rid) = derivation.source_region {
            let region =
                msg.get_region(src_rid)
                    .ok_or_else(|| ProofFailure::UnresolvableDerivation {
                        derivation_id: derivation.id,
                        reason: format!("source region {} not found", src_rid),
                    })?;
            region
                .default_repd
                .ok_or_else(|| ProofFailure::UnresolvableDerivation {
                    derivation_id: derivation.id,
                    reason: format!("source region {} has no default RepD", src_rid),
                })?
        } else if let Some(src_rid) = derivation.root_region {
            let region =
                msg.get_region(src_rid)
                    .ok_or_else(|| ProofFailure::UnresolvableDerivation {
                        derivation_id: derivation.id,
                        reason: format!("root region {} not found", src_rid),
                    })?;
            region
                .default_repd
                .ok_or_else(|| ProofFailure::UnresolvableDerivation {
                    derivation_id: derivation.id,
                    reason: format!("root region {} has no default RepD", src_rid),
                })?
        } else {
            return Err(ProofFailure::UnresolvableDerivation {
                derivation_id: derivation.id,
                reason: "cast derivation has no source".into(),
            });
        };

        let source_repd = msg.get_repd(source_repd_id).ok_or_else(|| {
            ProofFailure::Internal(format!("source RepD {} not found", source_repd_id))
        })?;
        let target_repd = msg.get_repd(target_repd_id).ok_or_else(|| {
            ProofFailure::Internal(format!("target RepD {} not found", target_repd_id))
        })?;

        let region_id =
            msg.region_of(derivation.id)
                .ok_or_else(|| ProofFailure::UnresolvableDerivation {
                    derivation_id: derivation.id,
                    reason: "cannot resolve root region for cast derivation".into(),
                })?;
        let region =
            msg.get_region(region_id)
                .ok_or_else(|| ProofFailure::UnresolvableDerivation {
                    derivation_id: derivation.id,
                    reason: format!("root region {} not found", region_id),
                })?;

        let resolved_addr =
            msg.addr_of(derivation.id)
                .ok_or_else(|| ProofFailure::UnresolvableDerivation {
                    derivation_id: derivation.id,
                    reason: "cannot resolve address for cast derivation".into(),
                })?;

        let remaining_bytes = region
            .base_addr
            .saturating_add(region.size)
            .saturating_sub(resolved_addr);
        let size_ok = target_repd.size <= remaining_bytes;

        let alignment_ok = if target_repd.alignment > 0 {
            resolved_addr % target_repd.alignment == 0
        } else {
            true
        };

        let reinterpretation_ok = valid_reinterpretation(source_repd, target_repd);

        let cast_goal = Goal::new(
            InvariantName::Interpretation,
            Target::Derivation(derivation.id),
            ProofContext::new(format!("cast_verification::d{}", derivation.id)),
        );
        let mut cast_proof = Proof::new(cast_goal);

        let sf = Fact::axiom(
            next_fact_id(),
            format!(
                "source type RepD {} has layout size={}, alignment={}",
                source_repd.id, source_repd.size, source_repd.alignment
            ),
        );
        let sf_id = sf.id;
        cast_proof.add_step(ProofStep::Assume { fact: sf });

        let tf = Fact::axiom(
            next_fact_id(),
            format!(
                "target type RepD {} has layout size={}, alignment={}",
                target_repd.id, target_repd.size, target_repd.alignment
            ),
        );
        let tf_id = tf.id;
        cast_proof.add_step(ProofStep::Assume { fact: tf });

        if size_ok && alignment_ok && reinterpretation_ok {
            let cf = Fact::derived(next_fact_id(), format!(
                "cast at derivation {} is valid: size_ok={}, alignment_ok={}, reinterpretation_ok={}",
                derivation.id, size_ok, alignment_ok, reinterpretation_ok
            ));
            cast_proof.add_step(ProofStep::Infer {
                from: vec![sf_id, tf_id],
                rule: InferenceRule::CastValidity,
                conclusion: cf,
            });
            cast_proof.conclude(Conclusion::Proven);
        } else {
            if !reinterpretation_ok {
                return Err(ProofFailure::UnsafeReinterpretation {
                    derivation_id: derivation.id,
                    reason: format!(
                        "invalid reinterpretation: {} -> {}",
                        source_repd.kind, target_repd.kind
                    ),
                });
            }
            if !size_ok {
                return Err(ProofFailure::SizeAlignmentViolation {
                    derivation_id: derivation.id,
                    reason: format!(
                        "target size {} exceeds remaining bytes {}",
                        target_repd.size, remaining_bytes
                    ),
                });
            }
            if !alignment_ok {
                return Err(ProofFailure::SizeAlignmentViolation {
                    derivation_id: derivation.id,
                    reason: format!(
                        "address 0x{:x} not aligned to {} bytes",
                        resolved_addr, target_repd.alignment
                    ),
                });
            }
        }

        reinterpret_proofs.push(ReinterpretationSafetyProof::new(
            derivation.id,
            source_repd_id,
            target_repd_id,
            size_ok,
            alignment_ok,
            reinterpretation_ok,
            cast_proof,
        ));
    }

    top_proof.conclude(Conclusion::Proven);

    Ok(InterpretationProof {
        bd_compatibility_proofs: bd_proofs,
        reinterpretation_safety_proofs: reinterpret_proofs,
        proof: top_proof,
    })
}
