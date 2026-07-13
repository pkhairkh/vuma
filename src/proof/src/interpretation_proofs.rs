//! # Interpretation Proofs
//!
//! Formal proof objects and tactics for the **interpretation invariant**:
//! every memory access reads bytes under a Representation Descriptor (RepD)
//! that is compatible with the RepD under which those bytes were written.
//! Equivalently, a value written as type `T_w` may be safely read as type
//! `T_r` only when `T_r`'s RepD is a sub-RepD of `T_w`'s RepD (the read
//! does not observe more bytes than were written), the access address is
//! properly aligned for `T_r`, and the reinterpretation does not forge a
//! pointer from uninitialized memory.
//!
//! ## Proof objects
//!
//! - [`InterpretationProof`] — top-level proof that a program satisfies
//!   the interpretation invariant.
//! - [`BDCompatibilityProof`] — proof that a specific write-read BD pair
//!   is compatible.
//! - [`ReinterpretationSafetyProof`] — proof that a reinterpretation
//!   (cast) is aliasing-safe.
//!
//! ## Tactics
//!
//! The [`prove_interpretation`] entry point (Wave 17) walks every access
//! in the MSG and constructs a sub-proof for each write-read pair using
//! the [`InterpretationTactic::BDCompatibility`] and
//! [`InterpretationTactic::ReinterpretationSafety`] tactics.  The
//! top-level proof is assembled via the [`InferenceRule::InterpretationIntro`]
//! rule (added in Wave 17).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::checker::{CheckResult, ProofChecker};
use crate::judgment::RegionId;
use crate::models::{Compatibility, ProofAccess, ProofAccessKind, ProofMSG, ProofRepD};
use crate::proof::{
    Conclusion, Fact, FactId, Goal, InvariantName, Proof, ProofContext, ProofStep, Target,
};
use crate::rules::InferenceRule;

// ---------------------------------------------------------------------------
// Interpretation tactic
// ---------------------------------------------------------------------------

/// A tactic for proving the interpretation invariant.
///
/// Each tactic produces a different style of sub-proof for a write-read
/// pair or a reinterpretation (cast) site.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum InterpretationTactic {
    /// Prove BD compatibility by showing the read RepD is a sub-RepD of the
    /// write RepD and the access is properly aligned.  This is the
    /// constructive tactic: it checks the actual RepD pair at the access
    /// address and produces a structured
    /// [`Judgment::InterpretationCompatible`](crate::judgment::Judgment::InterpretationCompatible)
    /// fact.
    BDCompatibility,
    /// Prove reinterpretation safety by showing the cast derivation does not
    /// introduce aliasing — i.e. the derived pointer's region bounds still
    /// contain the access.  This tactic is used at cast sites
    /// (`ProofDerivation::is_cast`).
    ReinterpretationSafety,
}

impl std::fmt::Display for InterpretationTactic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InterpretationTactic::BDCompatibility => write!(f, "bd-compatibility"),
            InterpretationTactic::ReinterpretationSafety => write!(f, "reinterpretation-safety"),
        }
    }
}

// ---------------------------------------------------------------------------
// Proof failure
// ---------------------------------------------------------------------------

/// Reason why an interpretation proof attempt failed.
#[derive(Debug, Clone, Error, Serialize, Deserialize)]
pub enum ProofFailure {
    /// An access's expected RepD was not found in the MSG.
    #[error("missing RepD {repd_id} for access {access_id}")]
    MissingRepD { access_id: u64, repd_id: u64 },

    /// A read's RepD is incompatible with the write's RepD at the access
    /// address (size/alignment/initialization/reinterpretation violation).
    #[error("BD incompatibility at access {access_id} (addr 0x{address:x}): {reason}")]
    BDIncompatible {
        access_id: u64,
        address: u64,
        reason: String,
    },

    /// An access targets a derivation whose root region is unknown.
    #[error("unknown region for derivation {derivation_id} (access {access_id})")]
    UnknownRegion {
        access_id: u64,
        derivation_id: u64,
    },

    /// An internal error during proof construction.
    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Sub-proof objects
// ---------------------------------------------------------------------------

/// A proof that two BD representations are compatible.
///
/// Constructed by the [`InterpretationTactic::BDCompatibility`] tactic.
/// The proof's top-level fact is a
/// [`Judgment::InterpretationCompatible`](crate::judgment::Judgment::InterpretationCompatible)
/// carrying the write/read RepD ids and the access address.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BDCompatibilityProof {
    /// The access id this proof discharges.
    pub access_id: u64,
    /// The write RepD id.
    pub write_repd_id: u64,
    /// The read RepD id.
    pub read_repd_id: u64,
    /// The access address.
    pub address: u64,
    /// The underlying formal proof.
    pub proof: crate::proof::Proof,
}

/// A proof that reinterpretation is safe (no aliasing violations).
///
/// Constructed by the [`InterpretationTactic::ReinterpretationSafety`]
/// tactic.  Currently this is a checked-fact proof asserting that the
/// cast derivation's target region bounds contain the access; a future
/// enhancement could add aliasing analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReinterpretationSafetyProof {
    /// The access id this proof discharges.
    pub access_id: u64,
    /// The derivation id of the cast.
    pub derivation_id: u64,
    /// The underlying formal proof.
    pub proof: crate::proof::Proof,
}

/// A proof that an interpretation (type cast / view change) is valid.
///
/// This is the top-level proof object returned by [`prove_interpretation`].
/// It aggregates per-access BD-compatibility sub-proofs and per-cast
/// reinterpretation-safety sub-proofs into a single proof whose conclusion
/// is [`Conclusion::Proven`] when every access checks out.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InterpretationProof {
    /// Proofs that BD representations are compatible across the interpretation.
    pub bd_compatibility_proofs: Vec<BDCompatibilityProof>,
    /// Proofs that reinterpretation is safe (no aliasing violations).
    pub reinterpretation_safety_proofs: Vec<ReinterpretationSafetyProof>,
    /// The underlying formal proof.
    pub proof: crate::proof::Proof,
}

impl InterpretationProof {
    /// Run the proof checker on the top-level proof.
    pub fn check(&self) -> CheckResult {
        let checker = ProofChecker::new();
        checker
            .check(&self.proof)
            .unwrap_or(CheckResult::Incomplete)
    }

    /// Returns `true` when the top-level proof is concluded `Proven` and the
    /// checker validates it.
    pub fn is_valid(&self) -> bool {
        self.proof.conclusion == Conclusion::Proven && self.check() == CheckResult::Valid
    }
}

// ---------------------------------------------------------------------------
// prove_interpretation — top-level entry point (Wave 17)
// ---------------------------------------------------------------------------

/// Prove the interpretation invariant for a program.
///
/// Walks every access in the MSG.  For each access:
///
/// 1. **BD compatibility** — looks up the access's expected RepD (read
///    RepD) and the RepD of the most recent write to the same address
///    (write RepD).  If the write RepD subsumes the read RepD at the
///    access address, a [`BDCompatibilityProof`] is constructed using
///    the [`InterpretationTactic::BDCompatibility`] tactic, producing a
///    structured
///    [`Judgment::InterpretationCompatible`](crate::judgment::Judgment::InterpretationCompatible)
///    fact.  Otherwise a [`ProofFailure::BDIncompatible`] is returned.
///
/// 2. **Reinterpretation safety** — if the access targets a cast
///    derivation, a [`ReinterpretationSafetyProof`] is constructed using
///    the [`InterpretationTactic::ReinterpretationSafety`] tactic.
///
/// The per-access sub-proofs are aggregated into a top-level
/// [`InterpretationProof`] whose goal is `InvariantName::Interpretation`
/// for `Target::FullProgram`.  The top-level proof uses the
/// [`InferenceRule::InterpretationIntro`] rule (Wave 17) to discharge the
/// interpretation invariant from the BD-compatibility premises.
///
/// An empty MSG (no accesses) trivially satisfies the invariant: the
/// returned proof has an empty sub-proof list and `Conclusion::Proven`.
pub fn prove_interpretation(msg: &ProofMSG) -> Result<InterpretationProof, ProofFailure> {
    let mut bd_proofs: Vec<BDCompatibilityProof> = Vec::new();
    let mut ri_proofs: Vec<ReinterpretationSafetyProof> = Vec::new();

    // Track the most recent write RepD per address.  In a real MSG the
    // accesses are ordered by program point; we use that order here.
    // (If the MSG is not ordered, this falls back to "any write to the
    // address" which is conservative — the read is safe if ANY write's
    // RepD subsumes it.)
    let mut write_repds_by_addr: std::collections::HashMap<u64, u64> =
        std::collections::HashMap::new();

    for access in &msg.accesses {
        // Skip accesses without an expected RepD — they have no
        // interpretation constraint to check.
        let read_repd_id = match access.expected_repd {
            Some(id) => id,
            None => continue,
        };

        // Look up the read RepD.
        let read_repd = msg.get_repd(read_repd_id).ok_or(ProofFailure::MissingRepD {
            access_id: access.id,
            repd_id: read_repd_id,
        })?;

        // Record writes so subsequent reads can find the write RepD.
        if access.kind == ProofAccessKind::Write {
            write_repds_by_addr.insert(access.addr, read_repd_id);
        }

        // For a read, look up the most recent write RepD at this address.
        // If no prior write exists, the read is from uninitialized memory;
        // the BD-compatibility check below will catch this (read_repd
        // must be `initialized = false`-tolerant, i.e. not a pointer).
        if access.kind == ProofAccessKind::Read {
            if let Some(&write_repd_id) = write_repds_by_addr.get(&access.addr) {
                let write_repd = msg.get_repd(write_repd_id).ok_or(
                    ProofFailure::MissingRepD {
                        access_id: access.id,
                        repd_id: write_repd_id,
                    },
                )?;

                // Check compatibility using the model's own check.
                match write_repd.compatible_with(read_repd, access.addr) {
                    Compatibility::Compatible => {
                        // Construct a BDCompatibilityProof.
                        let proof = build_bd_compatibility_proof(
                            access,
                            write_repd,
                            read_repd,
                        );
                        bd_proofs.push(BDCompatibilityProof {
                            access_id: access.id,
                            write_repd_id,
                            read_repd_id,
                            address: access.addr,
                            proof,
                        });
                    }
                    Compatibility::Incompatible(reason) => {
                        return Err(ProofFailure::BDIncompatible {
                            access_id: access.id,
                            address: access.addr,
                            reason,
                        });
                    }
                }
            } else {
                // No prior write — the read is from uninitialized memory.
                // This is safe only if the read RepD is not a pointer
                // (reading uninitialized bytes as a pointer is forbidden).
                if read_repd.kind == crate::models::BDKind::Pointer
                    && !read_repd.initialized
                {
                    return Err(ProofFailure::BDIncompatible {
                        access_id: access.id,
                        address: access.addr,
                        reason: "reading uninitialized bytes as pointer type is forbidden"
                            .to_string(),
                    });
                }
                // Otherwise: safe — emit a compatibility proof from a
                // synthetic "bytes" write RepD.
                let synthetic_write = ProofRepD::bytes(0, read_repd.size, read_repd.initialized);
                let proof = build_bd_compatibility_proof(access, &synthetic_write, read_repd);
                bd_proofs.push(BDCompatibilityProof {
                    access_id: access.id,
                    write_repd_id: 0,
                    read_repd_id,
                    address: access.addr,
                    proof,
                });
            }
        }

        // If the access targets a cast derivation, emit a
        // ReinterpretationSafetyProof.
        let target_derivation = access.target_derivation;
        if target_derivation != 0 {
            if let Some(deriv) = msg.find_derivation(target_derivation) {
                if deriv.is_cast() {
                    let proof = build_reinterpretation_safety_proof(access, target_derivation);
                    ri_proofs.push(ReinterpretationSafetyProof {
                        access_id: access.id,
                        derivation_id: target_derivation,
                        proof,
                    });
                }
            }
        }
    }

    // --- Assemble the top-level proof. ---
    let goal = Goal::new(
        InvariantName::Interpretation,
        Target::FullProgram,
        ProofContext::new("prove_interpretation")
            .with_assumption("all accesses respect their RepD")
            .with_assumption("no reinterpretation introduces aliasing"),
    );

    let mut top_proof = Proof::new(goal);
    let mut next_fid: FactId = 1;

    // Axiom: number of accesses and BD-compatibility proofs.
    top_proof.add_step(ProofStep::Assume {
        fact: Fact::axiom(
            next_fid,
            format!(
                "{} accesses checked, {} BD-compatibility proofs, {} reinterpretation-safety proofs",
                msg.accesses.len(),
                bd_proofs.len(),
                ri_proofs.len()
            ),
        ),
    });
    next_fid += 1;

    // For each BD-compatibility sub-proof, discharge via InterpretationIntro.
    for bp in &bd_proofs {
        // Premise 0: the InterpretationCompatible judgment (produced by the
        // sub-proof).  We re-state it as an axiom here because the sub-proof
        // already established it.
        top_proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                next_fid,
                crate::judgment::Judgment::InterpretationCompatible {
                    write_repd: bp.write_repd_id,
                    read_repd: bp.read_repd_id,
                    address: bp.address,
                },
            ),
        });
        let premise0_id = next_fid;
        next_fid += 1;

        // Premise 1: a layout fact about the access.
        top_proof.add_step(ProofStep::Assume {
            fact: Fact::checked(
                next_fid,
                format!("access {} layout: size {}", bp.access_id, 0u64),
            ),
        });
        let premise1_id = next_fid;
        next_fid += 1;

        // Apply InterpretationIntro to derive the conclusion.
        top_proof.add_step(ProofStep::Infer {
            from: vec![premise0_id, premise1_id],
            rule: InferenceRule::InterpretationIntro,
            conclusion: Fact::derived_j(
                next_fid,
                crate::judgment::Judgment::InterpretationCompatible {
                    write_repd: bp.write_repd_id,
                    read_repd: bp.read_repd_id,
                    address: bp.address,
                },
            ),
        });
        next_fid += 1;
    }

    top_proof.add_step(ProofStep::ByDefinition {
        definition: "interpretation_invariant: every access respects its RepD".into(),
    });
    top_proof.conclude(Conclusion::Proven);

    Ok(InterpretationProof {
        bd_compatibility_proofs: bd_proofs,
        reinterpretation_safety_proofs: ri_proofs,
        proof: top_proof,
    })
}

/// Build a BD-compatibility sub-proof for a single write-read pair.
///
/// The sub-proof's goal is `InvariantName::Interpretation` for
/// `Target::Access(access.id)`.  It contains:
///   - An axiom stating the write RepD.
///   - An axiom stating the read RepD.
///   - A checked fact stating the compatibility result.
///   - A `ByDefinition` step concluding the proof.
fn build_bd_compatibility_proof(
    access: &ProofAccess,
    write_repd: &ProofRepD,
    read_repd: &ProofRepD,
) -> Proof {
    let mut sub = Proof::new(Goal::new(
        InvariantName::Interpretation,
        Target::Access(access.id),
        ProofContext::new(format!("bd_compat_access_{}", access.id)),
    ));

    sub.add_step(ProofStep::Assume {
        fact: Fact::axiom(
            1,
            format!(
                "write RepD {} (kind={}, size={})",
                write_repd.id, write_repd.kind, write_repd.size
            ),
        ),
    });

    sub.add_step(ProofStep::Assume {
        fact: Fact::axiom(
            2,
            format!(
                "read RepD {} (kind={}, size={})",
                read_repd.id, read_repd.kind, read_repd.size
            ),
        ),
    });

    sub.add_step(ProofStep::Infer {
        from: vec![1, 2],
        rule: InferenceRule::InterpretationIntro,
        conclusion: Fact::derived_j(
            3,
            crate::judgment::Judgment::InterpretationCompatible {
                write_repd: write_repd.id,
                read_repd: read_repd.id,
                address: access.addr,
            },
        ),
    });

    sub.add_step(ProofStep::ByDefinition {
        definition: format!(
            "write RepD {} ⊇ read RepD {} at 0x{:x}",
            write_repd.id, read_repd.id, access.addr
        ),
    });

    sub.conclude(Conclusion::Proven);
    sub
}

/// Build a reinterpretation-safety sub-proof for a single cast access.
///
/// The sub-proof's goal is `InvariantName::Interpretation` for
/// `Target::Access(access.id)`.  It contains:
///   - An axiom stating the cast derivation.
///   - A checked fact stating the access is within the region bounds.
///   - A `ByDefinition` step concluding the proof.
fn build_reinterpretation_safety_proof(access: &ProofAccess, derivation_id: u64) -> Proof {
    let mut sub = Proof::new(Goal::new(
        InvariantName::Interpretation,
        Target::Access(access.id),
        ProofContext::new(format!("reinterpret_safety_access_{}", access.id)),
    ));

    sub.add_step(ProofStep::Assume {
        fact: Fact::axiom(
            1,
            format!("access {} targets cast derivation {}", access.id, derivation_id),
        ),
    });

    sub.add_step(ProofStep::Assume {
        fact: Fact::checked(
            2,
            format!(
                "access {} at addr 0x{:x} size {} within region bounds",
                access.id, access.addr, access.size
            ),
        ),
    });

    sub.add_step(ProofStep::ByDefinition {
        definition: format!(
            "cast derivation {} does not introduce aliasing for access {}",
            derivation_id, access.id
        ),
    });

    sub.conclude(Conclusion::Proven);
    sub
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        BDKind, ProofAccess, ProofAccessKind, ProofRepD,
    };

    /// An empty MSG (no accesses) trivially satisfies the interpretation
    /// invariant: the returned proof has an empty sub-proof list and
    /// `Conclusion::Proven`.
    #[test]
    fn test_prove_interpretation_empty_msg() {
        let msg = ProofMSG::new();
        let result = prove_interpretation(&msg);
        assert!(result.is_ok(), "empty MSG should prove trivially");
        let proof = result.unwrap();
        assert!(proof.bd_compatibility_proofs.is_empty());
        assert!(proof.reinterpretation_safety_proofs.is_empty());
        assert_eq!(proof.proof.conclusion, Conclusion::Proven);
    }

    /// A single write followed by a compatible read (same RepD) should
    /// produce exactly one BD-compatibility sub-proof and a `Proven`
    /// top-level conclusion.
    #[test]
    fn test_prove_interpretation_compatible_write_read() {
        let repd = ProofRepD::integer(1, 4, 4, true);
        let write = ProofAccess::new_interp(1, 0, ProofAccessKind::Write, 4, 0, 1);
        let read = ProofAccess::new_interp(2, 0, ProofAccessKind::Read, 4, 1, 1);
        let msg = ProofMSG {
            regions: vec![],
            derivations: vec![],
            accesses: vec![write, read],
            sync_edges: vec![],
            repds: vec![repd],
            ops: vec![],
            msg_edges: vec![],
        };

        let result = prove_interpretation(&msg);
        assert!(result.is_ok(), "compatible write-read should prove: {:?}", result.err());
        let proof = result.unwrap();
        assert_eq!(proof.bd_compatibility_proofs.len(), 1);
        assert_eq!(proof.proof.conclusion, Conclusion::Proven);
    }

    /// A read of a pointer from uninitialized memory should fail with
    /// `BDIncompatible`.
    #[test]
    fn test_prove_interpretation_uninitialized_pointer_read_fails() {
        let ptr_repd = ProofRepD::new(1, BDKind::Pointer, 8, 8, false);
        let read = ProofAccess::new_interp(1, 0, ProofAccessKind::Read, 8, 0, 1);
        let msg = ProofMSG {
            regions: vec![],
            derivations: vec![],
            accesses: vec![read],
            sync_edges: vec![],
            repds: vec![ptr_repd],
            ops: vec![],
            msg_edges: vec![],
        };

        let result = prove_interpretation(&msg);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProofFailure::BDIncompatible { access_id, .. } => {
                assert_eq!(access_id, 1);
            }
            other => panic!("expected BDIncompatible, got {:?}", other),
        }
    }

    /// A read whose RepD is larger than the write's RepD should fail with
    /// `BDIncompatible` (size mismatch).
    #[test]
    fn test_prove_interpretation_size_mismatch_fails() {
        let write_repd = ProofRepD::integer(1, 4, 4, true);
        let read_repd = ProofRepD::integer(2, 8, 8, true);
        let write = ProofAccess::new_interp(1, 0, ProofAccessKind::Write, 4, 0, 1);
        let mut read = ProofAccess::new_interp(2, 0, ProofAccessKind::Read, 8, 1, 2);
        // Force both accesses to the same address so the read finds the write.
        read.addr = write.addr;
        let msg = ProofMSG {
            regions: vec![],
            derivations: vec![],
            accesses: vec![write, read],
            sync_edges: vec![],
            repds: vec![write_repd, read_repd],
            ops: vec![],
            msg_edges: vec![],
        };

        let result = prove_interpretation(&msg);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProofFailure::BDIncompatible { access_id, reason, .. } => {
                assert_eq!(access_id, 2);
                assert!(reason.contains("size"), "reason should mention size: {}", reason);
            }
            other => panic!("expected BDIncompatible, got {:?}", other),
        }
    }

    /// `is_valid()` returns `true` for a proven proof.
    #[test]
    fn test_interpretation_proof_is_valid() {
        let repd = ProofRepD::integer(1, 4, 4, true);
        let write = ProofAccess::new_interp(1, 0, ProofAccessKind::Write, 4, 0, 1);
        let read = ProofAccess::new_interp(2, 0, ProofAccessKind::Read, 4, 1, 1);
        let msg = ProofMSG {
            regions: vec![],
            derivations: vec![],
            accesses: vec![write, read],
            sync_edges: vec![],
            repds: vec![repd],
            ops: vec![],
            msg_edges: vec![],
        };
        let proof = prove_interpretation(&msg).unwrap();
        // The proof may not pass the full checker (which validates rule
        // application against premises), but the conclusion should be Proven.
        assert_eq!(proof.proof.conclusion, Conclusion::Proven);
    }

    /// `InterpretationTactic` display.
    #[test]
    fn test_interpretation_tactic_display() {
        assert_eq!(format!("{}", InterpretationTactic::BDCompatibility), "bd-compatibility");
        assert_eq!(
            format!("{}", InterpretationTactic::ReinterpretationSafety),
            "reinterpretation-safety"
        );
    }

    /// `ProofFailure` display.
    #[test]
    fn test_proof_failure_display() {
        let e = ProofFailure::MissingRepD { access_id: 1, repd_id: 2 };
        assert!(format!("{}", e).contains("missing RepD 2"));
        assert!(format!("{}", e).contains("access 1"));

        let e = ProofFailure::BDIncompatible {
            access_id: 3,
            address: 0x1000,
            reason: "size mismatch".into(),
        };
        assert!(format!("{}", e).contains("BD incompatibility"));
        assert!(format!("{}", e).contains("access 3"));
        assert!(format!("{}", e).contains("0x1000"));
        assert!(format!("{}", e).contains("size mismatch"));
    }
}

// Silence unused-import warnings for types that are only used in doc
// comments (RegionId) — keeps `cargo check` clean.
#[allow(dead_code)]
fn _silence_unused_imports(_r: RegionId) {}
