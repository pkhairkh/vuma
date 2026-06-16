//! # Cross-Invariant Composition Module
//!
//! This module provides the [`ProofBundle`] type, which aggregates proofs for
//! all five VUMA memory-safety invariants (Liveness, Exclusivity, Cleanup,
//! Origin, Interpretation) into a single bundle. It supports:
//!
//! - **Status queries**: checking whether each invariant has been proven.
//! - **Full verification**: confirming that all five invariants hold.
//! - **Cross-invariant consistency**: verifying that assumptions made by one
//!   invariant's proof are discharged by conclusions (facts) in another
//!   invariant's proof.

use crate::proof::{Conclusion, InvariantName, Proof};
use serde::{Deserialize, Serialize};

/// A bundle of proofs for all five invariants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProofBundle {
    /// The liveness proof, if available.
    pub liveness: Option<crate::liveness_proofs::LivenessProof>,
    /// The exclusivity proof, if available.
    pub exclusivity: Option<crate::exclusivity_proofs::ExclusivityProof>,
    /// The cleanup proof, if available.
    pub cleanup: Option<crate::cleanup_proofs::CleanupProof>,
    /// The origin proof, if available.
    pub origin: Option<crate::origin_proofs::OriginProof>,
    /// The interpretation proof, if available.
    pub interpretation: Option<crate::interpretation_proofs::InterpretationProof>,
}

/// Status of an individual invariant within a ProofBundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InvariantStatus {
    /// Proof successfully established.
    Proven,
    /// Proof attempted but failed.
    Failed(String),
    /// Proof not yet attempted.
    NotAttempted,
}

/// An assumption from one proof that isn't discharged by any other proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnresolvedAssumption {
    /// The invariant that makes this assumption.
    pub source_invariant: InvariantName,
    /// The assumption description.
    pub assumption: String,
    /// Which invariants were checked for a discharging conclusion.
    pub checked_invariants: Vec<InvariantName>,
}

impl ProofBundle {
    /// Create an empty bundle with no proofs.
    pub fn new() -> Self {
        Self {
            liveness: None,
            exclusivity: None,
            cleanup: None,
            origin: None,
            interpretation: None,
        }
    }

    /// Check the status of each invariant.
    ///
    /// Returns a vector of `(InvariantName, InvariantStatus)` pairs, one per
    /// invariant. An invariant is `Proven` if its proof exists and its
    /// top-level `Proof` has `conclusion == Proven`.
    pub fn status(&self) -> Vec<(InvariantName, InvariantStatus)> {
        vec![
            (
                InvariantName::Liveness,
                self.status_of(self.liveness.as_ref().map(|p| &p.proof)),
            ),
            (
                InvariantName::Exclusivity,
                self.status_of(self.exclusivity.as_ref().map(|p| &p.proof)),
            ),
            (
                InvariantName::Cleanup,
                self.status_of(self.cleanup.as_ref().map(|p| &p.proof)),
            ),
            (
                InvariantName::Origin,
                self.status_of(self.origin.as_ref().map(|p| &p.proof)),
            ),
            (
                InvariantName::Interpretation,
                self.status_of(self.interpretation.as_ref().map(|p| &p.proof)),
            ),
        ]
    }

    /// Verify that all invariants are proven.
    ///
    /// Returns `true` only if every invariant has a proof whose conclusion
    /// is `Conclusion::Proven`.
    pub fn all_proven(&self) -> bool {
        self.status()
            .iter()
            .all(|(_, s)| matches!(s, InvariantStatus::Proven))
    }

    /// Check cross-invariant consistency: verify that assumptions
    /// in one proof are discharged by conclusions in another.
    ///
    /// For each proof in the bundle, the method extracts assumptions from the
    /// proof goal's context. It then checks whether any *other* proof's facts
    /// contain a statement that matches (contains) the assumption text. If no
    /// discharging fact is found, the assumption is recorded as unresolved.
    ///
    /// Returns a list of unresolved assumptions.
    pub fn verify_cross_invariant_consistency(&self) -> Vec<UnresolvedAssumption> {
        let mut unresolved = Vec::new();

        // Collect all fact statements from each invariant, keyed by invariant name.
        let all_invariant_facts: Vec<(InvariantName, Vec<String>)> =
            self.collect_all_fact_statements();

        // For each invariant that has a proof, extract its assumptions and check
        // if they are discharged by facts from other invariants.
        let assumption_sources: Vec<(InvariantName, Vec<String>)> = self.collect_all_assumptions();

        for (source_invariant, assumptions) in assumption_sources {
            // Collect fact statements from all *other* invariants.
            let other_fact_strings: Vec<(InvariantName, &Vec<String>)> = all_invariant_facts
                .iter()
                .filter(|(inv, _)| *inv != source_invariant)
                .map(|(inv, facts)| (*inv, facts))
                .collect();

            for assumption in assumptions {
                let mut checked: Vec<InvariantName> =
                    other_fact_strings.iter().map(|(inv, _)| *inv).collect();

                let discharged = other_fact_strings.iter().any(|(_, facts)| {
                    facts
                        .iter()
                        .any(|stmt| stmt.contains(&assumption) || assumption.contains(stmt))
                });

                if !discharged {
                    // Deduplicate for deterministic output
                    let mut seen = std::collections::HashSet::new();
                    checked.retain(|inv| seen.insert(*inv));
                    unresolved.push(UnresolvedAssumption {
                        source_invariant,
                        assumption,
                        checked_invariants: checked,
                    });
                }
            }
        }

        unresolved
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Determine the status of a single invariant from its optional proof.
    fn status_of(&self, proof: Option<&Proof>) -> InvariantStatus {
        match proof {
            None => InvariantStatus::NotAttempted,
            Some(p) => {
                if p.conclusion == Conclusion::Proven {
                    InvariantStatus::Proven
                } else {
                    InvariantStatus::Failed(format!(
                        "conclusion is {:?} (expected Proven)",
                        p.conclusion
                    ))
                }
            }
        }
    }

    /// Collect all fact statement strings from every proof in the bundle.
    ///
    /// For proof types that contain sub-proofs (e.g. LivenessProof has
    /// `access_proofs` and `freed_proofs`), facts from those sub-proofs are
    /// included as well.
    fn collect_all_fact_statements(&self) -> Vec<(InvariantName, Vec<String>)> {
        let mut result = Vec::new();

        // Liveness
        if let Some(ref lp) = self.liveness {
            let mut facts = Self::fact_statements_from_proof(&lp.proof);
            for (_, sub_proof) in &lp.access_proofs {
                facts.extend(Self::fact_statements_from_proof(sub_proof));
            }
            for freed in &lp.freed_proofs {
                facts.extend(Self::fact_statements_from_proof(&freed.proof));
            }
            if let Some(ref dp) = lp.deadlock_proof {
                facts.extend(Self::fact_statements_from_proof(&dp.proof));
            }
            result.push((InvariantName::Liveness, facts));
        }

        // Exclusivity
        if let Some(ref ep) = self.exclusivity {
            let mut facts = Self::fact_statements_from_proof(&ep.proof);
            for (_, _, sub) in &ep.sub_proofs {
                match sub {
                    crate::exclusivity_proofs::ExclusivitySubProof::NoConflict => {}
                    crate::exclusivity_proofs::ExclusivitySubProof::NoAlias(na) => {
                        facts.extend(Self::fact_statements_from_proof(&na.proof));
                    }
                    crate::exclusivity_proofs::ExclusivitySubProof::Synchronized(sp) => {
                        facts.extend(Self::fact_statements_from_proof(&sp.proof));
                    }
                }
            }
            result.push((InvariantName::Exclusivity, facts));
        }

        // Cleanup
        if let Some(ref cp) = self.cleanup {
            let facts = Self::fact_statements_from_proof(&cp.proof);
            result.push((InvariantName::Cleanup, facts));
        }

        // Origin
        if let Some(ref op) = self.origin {
            let facts = Self::fact_statements_from_proof(&op.proof);
            result.push((InvariantName::Origin, facts));
        }

        // Interpretation
        if let Some(ref ip) = self.interpretation {
            let mut facts = Self::fact_statements_from_proof(&ip.proof);
            for bd in &ip.bd_compatibility_proofs {
                facts.extend(Self::fact_statements_from_proof(&bd.proof));
            }
            for ri in &ip.reinterpretation_safety_proofs {
                facts.extend(Self::fact_statements_from_proof(&ri.proof));
            }
            result.push((InvariantName::Interpretation, facts));
        }

        result
    }

    /// Extract all fact statement strings from a [`Proof`] object.
    fn fact_statements_from_proof(proof: &Proof) -> Vec<String> {
        proof
            .all_facts()
            .iter()
            .map(|f| f.statement.clone())
            .collect()
    }

    /// Collect assumptions from each proof's goal context.
    ///
    /// Assumptions come from `proof.goal.context.assumptions`, which is a
    /// `Vec<Judgment>`. Each judgment is converted to a statement string via
    /// [`Judgment::to_statement`].
    fn collect_all_assumptions(&self) -> Vec<(InvariantName, Vec<String>)> {
        let mut result = Vec::new();

        if let Some(ref lp) = self.liveness {
            let assumptions = Self::assumptions_from_proof(&lp.proof);
            result.push((InvariantName::Liveness, assumptions));
        }

        if let Some(ref ep) = self.exclusivity {
            let assumptions = Self::assumptions_from_proof(&ep.proof);
            result.push((InvariantName::Exclusivity, assumptions));
        }

        if let Some(ref cp) = self.cleanup {
            let assumptions = Self::assumptions_from_proof(&cp.proof);
            result.push((InvariantName::Cleanup, assumptions));
        }

        if let Some(ref op) = self.origin {
            let assumptions = Self::assumptions_from_proof(&op.proof);
            result.push((InvariantName::Origin, assumptions));
        }

        if let Some(ref ip) = self.interpretation {
            let assumptions = Self::assumptions_from_proof(&ip.proof);
            result.push((InvariantName::Interpretation, assumptions));
        }

        result
    }

    /// Extract assumption strings from a proof's goal context.
    fn assumptions_from_proof(proof: &Proof) -> Vec<String> {
        proof
            .goal
            .context
            .assumptions
            .iter()
            .map(|j| j.to_statement())
            .collect()
    }
}

impl Default for ProofBundle {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup_proofs::CleanupProof;
    use crate::cleanup_proofs::CleanupTactic;
    use crate::exclusivity_proofs::ExclusivityProof;
    use crate::interpretation_proofs::InterpretationProof;
    use crate::judgment::RegionId;
    use crate::liveness_proofs::LivenessProof;
    use crate::liveness_proofs::LivenessTactic;
    use crate::origin_proofs::OriginProof;
    use crate::proof::{Fact, Goal, Proof, ProofContext, ProofStep, Target};
    use crate::rules::InferenceRule;

    /// An empty bundle should report all invariants as NotAttempted and
    /// `all_proven()` should return `false`.
    #[test]
    fn test_empty_bundle_not_all_proven() {
        let bundle = ProofBundle::new();

        assert!(
            !bundle.all_proven(),
            "empty bundle should not be all_proven"
        );

        let statuses = bundle.status();
        assert_eq!(statuses.len(), 5, "should have 5 invariant statuses");

        for (name, status) in &statuses {
            assert_eq!(
                status,
                &InvariantStatus::NotAttempted,
                "{:?} should be NotAttempted",
                name
            );
        }
    }

    /// A bundle with all five proofs (each with conclusion Proven) should
    /// report `all_proven() == true` and every invariant as Proven.
    #[test]
    fn test_all_proven_bundle() {
        let liveness = make_liveness_proof();
        let exclusivity = make_exclusivity_proof();
        let cleanup = make_cleanup_proof();
        let origin = make_origin_proof();
        let interpretation = make_interpretation_proof();

        let bundle = ProofBundle {
            liveness: Some(liveness),
            exclusivity: Some(exclusivity),
            cleanup: Some(cleanup),
            origin: Some(origin),
            interpretation: Some(interpretation),
        };

        assert!(
            bundle.all_proven(),
            "fully populated bundle should be all_proven"
        );

        for (name, status) in bundle.status() {
            assert_eq!(
                status,
                InvariantStatus::Proven,
                "{:?} should be Proven",
                name
            );
        }
    }

    /// Test cross-invariant assumption checking:
    /// - The origin proof assumes "all derivation chains terminate at live
    ///   regions".
    /// - The liveness proof should have a fact containing "live" which
    ///   discharges that assumption.
    /// - An assumption that no other proof can discharge should appear as
    ///   unresolved.
    #[test]
    fn test_cross_invariant_assumption_checking() {
        // Create an origin proof with assumptions
        let origin = make_origin_proof_with_assumptions();
        // Create a liveness proof that discharges one of them
        let liveness = make_liveness_proof_with_facts();

        let bundle = ProofBundle {
            liveness: Some(liveness),
            exclusivity: None,
            cleanup: None,
            origin: Some(origin),
            interpretation: None,
        };

        let unresolved = bundle.verify_cross_invariant_consistency();

        // The assumption "region region#1 is live" from the origin proof
        // should be discharged by the liveness proof which contains
        // "region region#1 is live" as a fact.
        // The assumption "no tainted data reaches sensitive sinks" from the
        // origin proof should NOT be discharged by the liveness proof.
        let taint_unresolved = unresolved
            .iter()
            .any(|u| u.assumption.contains("tainted data"));
        assert!(
            taint_unresolved,
            "assumption about tainted data should be unresolved since no proof discharges it"
        );

        // The "region region#1 is live" assumption should be discharged
        let live_unresolved = unresolved
            .iter()
            .any(|u| u.assumption.contains("region#1 is live"));
        assert!(
            !live_unresolved,
            "assumption about region#1 being live should be discharged by liveness proof"
        );
    }

    // -----------------------------------------------------------------------
    // Test helpers: construct minimal proof objects
    // -----------------------------------------------------------------------

    fn make_liveness_proof() -> LivenessProof {
        let mut proof = Proof::new(Goal::new(
            InvariantName::Liveness,
            Target::FullProgram,
            ProofContext::new("test::liveness"),
        ));
        proof.conclude(Conclusion::Proven);

        LivenessProof {
            proof,
            access_proofs: Vec::new(),
            freed_proofs: Vec::new(),
            deadlock_proof: None,
            ordering: None,
            tactic: LivenessTactic::PathEnumeration,
        }
    }

    fn make_exclusivity_proof() -> ExclusivityProof {
        let mut proof = Proof::new(Goal::new(
            InvariantName::Exclusivity,
            Target::FullProgram,
            ProofContext::new("test::exclusivity"),
        ));
        proof.conclude(Conclusion::Proven);

        ExclusivityProof {
            proof,
            sub_proofs: Vec::new(),
            tactics_used: Vec::new(),
        }
    }

    fn make_cleanup_proof() -> CleanupProof {
        use std::collections::HashMap;

        let mut proof = Proof::new(Goal::new(
            InvariantName::Cleanup,
            Target::FullProgram,
            ProofContext::new("test::cleanup"),
        ));
        proof.conclude(Conclusion::Proven);

        CleanupProof {
            proof,
            release_map: HashMap::new(),
            tactic: CleanupTactic::PathEnumeration,
        }
    }

    fn make_origin_proof() -> OriginProof {
        let mut proof = Proof::new(Goal::new(
            InvariantName::Origin,
            Target::FullProgram,
            ProofContext::new("test::origin"),
        ));
        proof.conclude(Conclusion::Proven);

        OriginProof {
            proof,
            verified_regions: Vec::new(),
            checked_chains: Vec::new(),
        }
    }

    fn make_interpretation_proof() -> InterpretationProof {
        let mut proof = Proof::new(Goal::new(
            InvariantName::Interpretation,
            Target::FullProgram,
            ProofContext::new("test::interpretation"),
        ));
        proof.conclude(Conclusion::Proven);

        InterpretationProof {
            bd_compatibility_proofs: Vec::new(),
            reinterpretation_safety_proofs: Vec::new(),
            proof,
        }
    }

    /// Create an origin proof with assumptions that reference other invariants.
    fn make_origin_proof_with_assumptions() -> OriginProof {
        let proof = Proof::new(Goal::new(
            InvariantName::Origin,
            Target::FullProgram,
            ProofContext::new("test::origin_with_assumptions")
                .with_assumption("region region#1 is live")
                .with_assumption("no tainted data reaches sensitive sinks"),
        ));
        // Note: we do NOT conclude Proven here — this is just for testing
        // assumption extraction. The conclusion doesn't matter for
        // cross-invariant consistency checking.
        let mut proof = proof;
        proof.conclude(Conclusion::Proven);

        OriginProof {
            proof,
            verified_regions: vec![RegionId(1)],
            checked_chains: vec![(1, RegionId(1))],
        }
    }

    /// An empty ProofBundle should report all invariants as NotAttempted
    /// via the `status()` method.
    #[test]
    fn test_proof_bundle_status_empty() {
        let bundle = ProofBundle::new();
        let statuses = bundle.status();
        assert_eq!(statuses.len(), 5, "should have 5 invariant statuses");
        for (name, status) in &statuses {
            assert_eq!(
                status,
                &InvariantStatus::NotAttempted,
                "{:?} should be NotAttempted in empty bundle",
                name
            );
        }
    }

    /// Serialize and deserialize each ProofEnvelope variant and verify
    /// round-trip correctness.
    #[test]
    fn test_serialization_roundtrip_all_types() {
        use crate::serialization::ProofEnvelope;

        // --- Generic(Proof) ---
        let generic_proof = make_simple_proof_for_serialization();
        let env_generic = ProofEnvelope::Generic(generic_proof.clone());
        let json = env_generic.to_json_string().unwrap();
        let rt: ProofEnvelope = ProofEnvelope::from_json_string(&json).unwrap();
        if let ProofEnvelope::Generic(p) = rt {
            assert_eq!(p, generic_proof);
        } else {
            panic!("expected Generic variant after round-trip");
        }

        // --- Liveness ---
        let liveness = make_liveness_proof();
        let env_liveness = ProofEnvelope::Liveness(liveness.clone());
        let json = env_liveness.to_json_string().unwrap();
        let rt: ProofEnvelope = ProofEnvelope::from_json_string(&json).unwrap();
        if let ProofEnvelope::Liveness(p) = rt {
            assert_eq!(p, liveness);
        } else {
            panic!("expected Liveness variant after round-trip");
        }

        // --- Exclusivity ---
        let exclusivity = make_exclusivity_proof();
        let env_excl = ProofEnvelope::Exclusivity(exclusivity.clone());
        let json = env_excl.to_json_string().unwrap();
        let rt: ProofEnvelope = ProofEnvelope::from_json_string(&json).unwrap();
        if let ProofEnvelope::Exclusivity(p) = rt {
            assert_eq!(p, exclusivity);
        } else {
            panic!("expected Exclusivity variant after round-trip");
        }

        // --- Cleanup ---
        let cleanup = make_cleanup_proof();
        let env_cleanup = ProofEnvelope::Cleanup(cleanup.clone());
        let json = env_cleanup.to_json_string().unwrap();
        let rt: ProofEnvelope = ProofEnvelope::from_json_string(&json).unwrap();
        if let ProofEnvelope::Cleanup(p) = rt {
            assert_eq!(p, cleanup);
        } else {
            panic!("expected Cleanup variant after round-trip");
        }

        // --- Origin ---
        let origin = make_origin_proof();
        let env_origin = ProofEnvelope::Origin(origin.clone());
        let json = env_origin.to_json_string().unwrap();
        let rt: ProofEnvelope = ProofEnvelope::from_json_string(&json).unwrap();
        if let ProofEnvelope::Origin(p) = rt {
            assert_eq!(p, origin);
        } else {
            panic!("expected Origin variant after round-trip");
        }

        // --- Interpretation ---
        let interp = make_interpretation_proof();
        let env_interp = ProofEnvelope::Interpretation(interp.clone());
        let json = env_interp.to_json_string().unwrap();
        let rt: ProofEnvelope = ProofEnvelope::from_json_string(&json).unwrap();
        if let ProofEnvelope::Interpretation(p) = rt {
            assert_eq!(p, interp);
        } else {
            panic!("expected Interpretation variant after round-trip");
        }
    }

    /// Helper: create a simple Proof for serialization round-trip tests.
    fn make_simple_proof_for_serialization() -> Proof {
        let mut proof = Proof::new(Goal::new(
            InvariantName::Liveness,
            Target::Region(RegionId(42)),
            ProofContext::new("test::serialization_roundtrip"),
        ));
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(0, "region is allocated"),
        });
        proof.conclude(Conclusion::Proven);
        proof
    }

    /// Create a liveness proof that has a fact containing "region region#1 is
    /// live", which should discharge the corresponding assumption in the
    /// origin proof.
    fn make_liveness_proof_with_facts() -> LivenessProof {
        let mut proof = Proof::new(Goal::new(
            InvariantName::Liveness,
            Target::Region(RegionId(1)),
            ProofContext::new("test::liveness_with_facts"),
        ));
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(1, "region region#1 is live"),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived(2, "region region#1 is live at PP 0"),
        });
        proof.conclude(Conclusion::Proven);

        LivenessProof {
            proof,
            access_proofs: Vec::new(),
            freed_proofs: Vec::new(),
            deadlock_proof: None,
            ordering: None,
            tactic: LivenessTactic::PathEnumeration,
        }
    }
}
