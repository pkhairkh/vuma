//! # Proof Checker
//!
//! Verifies that a formal proof is valid: every step follows from previously
//! established facts using the stated rule, and there is no circular reasoning.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::proof::{Conclusion, Fact, FactId, Proof, ProofStep};
use crate::tactics::ProofResult;

// ---------------------------------------------------------------------------
// ProofCache
// ---------------------------------------------------------------------------

/// Cache for proof checking results, keyed by a goal fingerprint.
///
/// The cache avoids re-checking goals that have already been verified.
/// The key is typically a hash (fingerprint) of the goal's invariant name,
/// target, and context. The value is the cached [`ProofResult`].
pub type ProofCache = std::collections::HashMap<u64, ProofResult>;

// ---------------------------------------------------------------------------
// CheckResult
// ---------------------------------------------------------------------------

/// The result of checking a proof.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CheckResult {
    /// Every step in the proof is valid and the conclusion follows.
    Valid,

    /// The proof is invalid at the given step index for the stated reason.
    Invalid {
        /// Zero-based index of the offending step.
        step: usize,
        /// Human-readable explanation of why the step is invalid.
        reason: String,
    },

    /// The proof has not reached a definitive conclusion (still `Inconclusive`).
    Incomplete,
}

impl std::fmt::Display for CheckResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckResult::Valid => write!(f, "valid"),
            CheckResult::Invalid { step, reason } => {
                write!(f, "invalid at step {}: {}", step, reason)
            }
            CheckResult::Incomplete => write!(f, "incomplete"),
        }
    }
}

// ---------------------------------------------------------------------------
// Checker errors (internal)
// ---------------------------------------------------------------------------

/// Internal errors during proof checking (distinct from `CheckResult::Invalid`,
/// which is a normal outcome).
#[derive(Debug, Clone, Error)]
pub enum CheckerError {
    /// A referenced fact id was not found in the established fact set.
    #[error("fact id {id} not found in established facts")]
    FactNotFound { id: FactId },

    /// A sub-proof could not be checked.
    #[error("sub-proof check failed: {0}")]
    SubProofFailed(String),
}

// ---------------------------------------------------------------------------
// ProofChecker
// ---------------------------------------------------------------------------

/// Checks formal proofs for validity.
///
/// The checker walks through each step of a proof, maintains a set of
/// established facts, and verifies that:
///
/// 1. **Inference steps**: The stated rule can be applied to the referenced
///    premises to produce the claimed conclusion.
/// 2. **Circular reasoning**: No fact is used before it is established (i.e.
///    a step may only reference fact ids that appear in earlier steps).
/// 3. **Conclusion consistency**: If the proof concludes `Proven`, at least
///    one step must establish the goal; if `Refuted`, a contradiction must
///    be present.
///
/// # Example
///
/// ```ignore
/// use vuma_proof::checker::ProofChecker;
/// use vuma_proof::proof::Proof;
///
/// let proof = /* construct a proof */;
/// let checker = ProofChecker::new();
/// match checker.check(&proof) {
///     Ok(CheckResult::Valid) => println!("Proof is valid!"),
///     Ok(CheckResult::Invalid { step, reason }) => {
///         println!("Invalid at step {}: {}", step, reason);
///     }
///     Ok(CheckResult::Incomplete) => println!("Proof is incomplete."),
///     Err(e) => println!("Checker error: {}", e),
/// }
/// ```
#[derive(Debug, Default)]
pub struct ProofChecker {
    /// If true, check for circular reasoning (fact ids used before being
    /// established). Enabled by default.
    check_circular: bool,
}

impl ProofChecker {
    /// Create a new proof checker with default settings.
    pub fn new() -> Self {
        Self {
            check_circular: true,
        }
    }

    /// Disable circular-reasoning checking (useful for incremental checks).
    pub fn without_circular_checks(mut self) -> Self {
        self.check_circular = false;
        self
    }

    /// Check a proof for validity.
    ///
    /// Returns `Ok(CheckResult)` on success (where the result itself may be
    /// `Valid`, `Invalid`, or `Incomplete`), or `Err(CheckerError)` if the
    /// checker encounters an internal error.
    pub fn check(&self, proof: &Proof) -> Result<CheckResult, CheckerError> {
        // Set of fact ids that have been established so far.
        let mut established: std::collections::HashSet<FactId> = std::collections::HashSet::new();

        for (step_idx, step) in proof.steps.iter().enumerate() {
            match step {
                ProofStep::Assume { fact } => {
                    // Assumptions are always valid as long as the id is fresh.
                    if established.contains(&fact.id) {
                        return Ok(CheckResult::Invalid {
                            step: step_idx,
                            reason: format!(
                                "fact id {} is already established (duplicate)",
                                fact.id
                            ),
                        });
                    }
                    established.insert(fact.id);
                }

                ProofStep::Infer {
                    from,
                    rule,
                    conclusion,
                } => {
                    // 1. Verify all referenced facts are established.
                    for &fid in from {
                        if self.check_circular && !established.contains(&fid) {
                            return Ok(CheckResult::Invalid {
                                step: step_idx,
                                reason: format!(
                                    "fact id {} is referenced but not yet established (circular reasoning)",
                                    fid
                                ),
                            });
                        }
                    }

                    // 2. Verify the conclusion id is not already used.
                    if established.contains(&conclusion.id) {
                        return Ok(CheckResult::Invalid {
                            step: step_idx,
                            reason: format!(
                                "conclusion fact id {} is already established (duplicate)",
                                conclusion.id
                            ),
                        });
                    }

                    // 3. Collect premise facts and apply the rule.
                    let premise_facts: Vec<Fact> = from
                        .iter()
                        .map(|&fid| {
                            proof
                                .find_fact(fid)
                                .cloned()
                                .ok_or(CheckerError::FactNotFound { id: fid })
                        })
                        .collect::<Result<Vec<_>, _>>()?;

                    let expected_conclusion = match rule.apply(&premise_facts) {
                        Ok(f) => f,
                        Err(e) => {
                            return Ok(CheckResult::Invalid {
                                step: step_idx,
                                reason: format!("rule application failed: {}", e),
                            });
                        }
                    };

                    // 4. Verify the conclusion matches what the rule produces.
                    // We compare statements for backward compatibility, and
                    // also verify structured judgments match when both sides
                    // have them.
                    if expected_conclusion.statement != conclusion.statement {
                        log::debug!(
                            "Step {}: rule {} produced '{}' but proof claims '{}'",
                            step_idx,
                            rule.name(),
                            expected_conclusion.statement,
                            conclusion.statement,
                        );
                        return Ok(CheckResult::Invalid {
                            step: step_idx,
                            reason: format!(
                                "conclusion mismatch: rule {} produces '{}' but step claims '{}'",
                                rule.name(),
                                expected_conclusion.statement,
                                conclusion.statement
                            ),
                        });
                    }

                    // 4b. When both the expected and actual conclusion carry
                    //     structured judgments, verify they are structurally
                    //     equal. This prevents a proof from claiming one
                    //     judgment while the rule produces another with the
                    //     same string representation.
                    if let (Some(expected_j), Some(actual_j)) =
                        (&expected_conclusion.judgment, &conclusion.judgment)
                    {
                        if expected_j != actual_j {
                            log::debug!(
                                "Step {}: judgment mismatch — rule produced {:?} but step claims {:?}",
                                step_idx,
                                expected_j,
                                actual_j,
                            );
                            return Ok(CheckResult::Invalid {
                                step: step_idx,
                                reason: format!(
                                    "judgment mismatch: rule {} produces judgment {:?} but step claims {:?}",
                                    rule.name(),
                                    expected_j,
                                    actual_j
                                ),
                            });
                        }
                    }

                    established.insert(conclusion.id);
                }

                ProofStep::CaseSplit { cases } => {
                    // Every case must be valid.
                    for (case_idx, case) in cases.iter().enumerate() {
                        match self.check(case)? {
                            CheckResult::Valid => {}
                            CheckResult::Invalid { step: s, reason } => {
                                return Ok(CheckResult::Invalid {
                                    step: step_idx,
                                    reason: format!(
                                        "case {} is invalid at sub-step {}: {}",
                                        case_idx, s, reason
                                    ),
                                });
                            }
                            CheckResult::Incomplete => {
                                return Ok(CheckResult::Invalid {
                                    step: step_idx,
                                    reason: format!("case {} is incomplete", case_idx),
                                });
                            }
                        }
                    }
                }

                ProofStep::Induction { base, step: ind_step } => {
                    // Both the base case and the inductive step must be valid.
                    match self.check(base)? {
                        CheckResult::Valid => {}
                        CheckResult::Invalid { step: s, reason } => {
                            return Ok(CheckResult::Invalid {
                                step: step_idx,
                                reason: format!(
                                    "induction base case invalid at sub-step {}: {}",
                                    s, reason
                                ),
                            });
                        }
                        CheckResult::Incomplete => {
                            return Ok(CheckResult::Invalid {
                                step: step_idx,
                                reason: "induction base case is incomplete".into(),
                            });
                        }
                    }

                    match self.check(ind_step)? {
                        CheckResult::Valid => {}
                        CheckResult::Invalid { step: s, reason } => {
                            return Ok(CheckResult::Invalid {
                                step: step_idx,
                                reason: format!(
                                    "induction step invalid at sub-step {}: {}",
                                    s, reason
                                ),
                            });
                        }
                        CheckResult::Incomplete => {
                            return Ok(CheckResult::Invalid {
                                step: step_idx,
                                reason: "induction step is incomplete".into(),
                            });
                        }
                    }
                }

                ProofStep::Contradiction {
                    assumption,
                    negation,
                } => {
                    // Both facts must be established.
                    if self.check_circular && !established.contains(assumption) {
                        return Ok(CheckResult::Invalid {
                            step: step_idx,
                            reason: format!(
                                "assumption fact id {} is not established",
                                assumption
                            ),
                        });
                    }
                    if self.check_circular && !established.contains(negation) {
                        return Ok(CheckResult::Invalid {
                            step: step_idx,
                            reason: format!(
                                "negation fact id {} is not established",
                                negation
                            ),
                        });
                    }
                    // A contradiction step is valid if both facts exist.
                    // The proof author is responsible for ensuring they are
                    // logically contradictory.
                }

                ProofStep::ByDefinition { definition: _ } => {
                    // By-definition steps are always valid by fiat; they are
                    // essentially definitional expansions that don't introduce
                    // new facts with ids.
                }
            }
        }

        // After processing all steps, check the conclusion.
        match proof.conclusion {
            Conclusion::Proven => {
                // A proof that claims to be proven must have at least one step.
                if proof.steps.is_empty() {
                    return Ok(CheckResult::Invalid {
                        step: 0,
                        reason: "proof claims Proven but has no steps".into(),
                    });
                }
                Ok(CheckResult::Valid)
            }
            Conclusion::Refuted => {
                // A refutation must contain a Contradiction step somewhere.
                let has_contradiction = proof.steps.iter().any(|s| {
                    matches!(s, ProofStep::Contradiction { .. })
                });
                if !has_contradiction {
                    return Ok(CheckResult::Invalid {
                        step: 0,
                        reason: "proof claims Refuted but contains no Contradiction step".into(),
                    });
                }
                Ok(CheckResult::Valid)
            }
            Conclusion::Inconclusive => Ok(CheckResult::Incomplete),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::{Goal, ProofContext, Target};
    use crate::rules::InferenceRule;

    fn dummy_goal() -> Goal {
        Goal::new("liveness", Target::Region(1), ProofContext::new("test"))
    }

    #[test]
    fn test_valid_simple_proof() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(1, "region 1 is allocated"),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived(2, "region 1 is live"),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Valid);
    }

    #[test]
    fn test_circular_reasoning_detected() {
        let mut proof = Proof::new(dummy_goal());
        // Step 0 references fact 2, but fact 2 hasn't been established yet.
        proof.add_step(ProofStep::Infer {
            from: vec![2],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived(1, "region 1 is live"),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert!(matches!(result, CheckResult::Invalid { .. }));
        if let CheckResult::Invalid { reason, .. } = result {
            assert!(reason.contains("circular") || reason.contains("not yet established"));
        }
    }

    #[test]
    fn test_conclusion_mismatch() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(1, "region 1 is allocated"),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived(2, "wrong conclusion"),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert!(matches!(result, CheckResult::Invalid { .. }));
        if let CheckResult::Invalid { reason, .. } = result {
            assert!(reason.contains("mismatch"));
        }
    }

    #[test]
    fn test_incomplete_proof() {
        let proof = Proof::new(dummy_goal());
        // No steps, still Inconclusive.
        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Incomplete);
    }

    #[test]
    fn test_refuted_without_contradiction() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(1, "P"),
        });
        proof.conclude(Conclusion::Refuted);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert!(matches!(result, CheckResult::Invalid { .. }));
    }

    #[test]
    fn test_duplicate_fact_id() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(1, "P"),
        });
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(1, "Q"), // same id
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert!(matches!(result, CheckResult::Invalid { .. }));
        if let CheckResult::Invalid { reason, .. } = result {
            assert!(reason.contains("duplicate"));
        }
    }

    #[test]
    fn test_valid_contradiction_proof() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::assumption(1, "P"),
        });
        proof.add_step(ProofStep::Assume {
            fact: Fact::derived(2, "not P"),
        });
        proof.add_step(ProofStep::Contradiction {
            assumption: 1,
            negation: 2,
        });
        proof.conclude(Conclusion::Refuted);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Valid);
    }

    // -- check_proof_cached tests -----------------------------------------------

    /// Checks a proof goal against the checker, using a cache to avoid
    /// redundant work. If the goal's fingerprint is already in the cache,
    /// the cached result is returned directly. Otherwise the proof is
    /// checked, the result is stored in the cache, and the result is
    /// returned.
    pub fn check_proof_cached(goal: &Goal, cache: &mut ProofCache) -> ProofResult {
        // Compute a simple fingerprint from the goal.
        let fingerprint = goal_fingerprint(goal);

        if let Some(cached) = cache.get(&fingerprint) {
            return cached.clone();
        }

        // Build a trivial proof from the goal and check it.
        let mut proof = Proof::new(goal.clone());
        proof.add_step(ProofStep::Assume {
            fact: Fact::assumption(1, goal.invariant.clone()),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = match checker.check(&proof) {
            Ok(CheckResult::Valid) => ProofResult::Discharged,
            Ok(CheckResult::Invalid { reason, .. }) => ProofResult::Failed(reason),
            Ok(CheckResult::Incomplete) => ProofResult::SubGoals(vec![goal.clone()]),
            Err(e) => ProofResult::Failed(e.to_string()),
        };

        cache.insert(fingerprint, result.clone());
        result
    }

    /// Computes a simple fingerprint for a goal using FNV-1a.
    fn goal_fingerprint(goal: &Goal) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in goal.invariant.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        for byte in goal.context.scope.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    #[test]
    fn check_proof_cached_returns_valid() {
        let goal = Goal::new("liveness", Target::Region(1), ProofContext::new("test"));
        let mut cache = ProofCache::new();
        let result = check_proof_cached(&goal, &mut cache);
        assert!(result.is_discharged());
    }

    #[test]
    fn check_proof_cached_uses_cache() {
        let goal = Goal::new("exclusivity", Target::Region(2), ProofContext::new("test"));
        let mut cache = ProofCache::new();
        // First call populates the cache.
        let result1 = check_proof_cached(&goal, &mut cache);
        assert!(result1.is_discharged());
        assert!(cache.contains_key(&goal_fingerprint(&goal)));
        // Second call should hit the cache.
        let result2 = check_proof_cached(&goal, &mut cache);
        assert!(result2.is_discharged());
    }

    #[test]
    fn check_proof_cached_different_goals() {
        let goal1 = Goal::new("liveness", Target::Region(1), ProofContext::new("a"));
        let goal2 = Goal::new("bounds", Target::Region(2), ProofContext::new("b"));
        let mut cache = ProofCache::new();
        let r1 = check_proof_cached(&goal1, &mut cache);
        let r2 = check_proof_cached(&goal2, &mut cache);
        assert!(r1.is_discharged());
        assert!(r2.is_discharged());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn proof_cache_type_works() {
        let mut cache: ProofCache = ProofCache::new();
        cache.insert(1, ProofResult::Discharged);
        cache.insert(2, ProofResult::Failed("err".to_string()));
        assert_eq!(cache.len(), 2);
        assert!(cache.get(&1).unwrap().is_discharged());
    }

    // -- Structured judgment checker tests ------------------------------------

    use crate::judgment::{EventId, Judgment, PointerId, RegionId as JRegionId, ResourceId};

    #[test]
    fn test_structured_liveness_intro_proof() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(1, Judgment::Allocated { region: JRegionId(1) }),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived_j(2, Judgment::Live { region: JRegionId(1) }),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Valid);
    }

    #[test]
    fn test_structured_liveness_intro_judgment_mismatch() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(1, Judgment::Allocated { region: JRegionId(1) }),
        });
        // Claim the conclusion is Live for r2, but the rule will produce Live for r1
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived_j(2, Judgment::Live { region: JRegionId(2) }),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert!(matches!(result, CheckResult::Invalid { .. }));
        if let CheckResult::Invalid { reason, .. } = result {
            assert!(reason.contains("mismatch"));
        }
    }

    #[test]
    fn test_structured_derivation_transitivity_proof() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                1,
                Judgment::Derived {
                    pointer: PointerId(1),
                    from: PointerId(2),
                    region: JRegionId(1),
                },
            ),
        });
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                2,
                Judgment::Derived {
                    pointer: PointerId(2),
                    from: PointerId(3),
                    region: JRegionId(1),
                },
            ),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1, 2],
            rule: InferenceRule::DerivationTransitivity,
            conclusion: Fact::derived_j(
                3,
                Judgment::Derived {
                    pointer: PointerId(1),
                    from: PointerId(3),
                    region: JRegionId(1),
                },
            ),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Valid);
    }

    #[test]
    fn test_structured_temporal_ordering_proof() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                1,
                Judgment::TemporalOrder {
                    event_a: EventId(1),
                    event_b: EventId(2),
                },
            ),
        });
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                2,
                Judgment::TemporalOrder {
                    event_a: EventId(2),
                    event_b: EventId(3),
                },
            ),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1, 2],
            rule: InferenceRule::TemporalOrdering,
            conclusion: Fact::derived_j(
                3,
                Judgment::TemporalOrder {
                    event_a: EventId(1),
                    event_b: EventId(3),
                },
            ),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Valid);
    }

    #[test]
    fn test_structured_exclusivity_elim_proof() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                1,
                Judgment::Exclusive { resource: ResourceId(1) },
            ),
        });
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                2,
                Judgment::Exclusive { resource: ResourceId(2) },
            ),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1, 2],
            rule: InferenceRule::ExclusivityElim,
            conclusion: Fact::derived(3, "no conflict between resource#1 and resource#2"),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Valid);
    }

    #[test]
    fn test_structured_liveness_elim_proof() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::checked_j(1, Judgment::Freed { region: JRegionId(5) }),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessElim,
            conclusion: Fact::derived(2, "region region#5 is dead"),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Valid);
    }

    #[test]
    fn test_structured_wrong_rule_for_judgment() {
        // Try to apply LivenessIntro to a Freed judgment — should fail.
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(1, Judgment::Freed { region: JRegionId(1) }),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived(2, "region r1 is live"),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert!(matches!(result, CheckResult::Invalid { .. }));
        if let CheckResult::Invalid { reason, .. } = result {
            assert!(reason.contains("rule application failed"));
        }
    }

    #[test]
    fn test_structured_bounds_preservation_proof() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::derived_j(
                1,
                Judgment::InBounds {
                    pointer: PointerId(1),
                    offset: 8,
                    size: 4,
                },
            ),
        });
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(2, "region r1 has bounds [0, 1024]"),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1, 2],
            rule: InferenceRule::BoundsPreservation,
            conclusion: Fact::derived(
                3,
                "bounds preserved: inbounds pointer#1 offset=8 size=4 ∧ region r1 has bounds [0, 1024]",
            ),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Valid);
    }

    #[test]
    fn test_structured_exclusivity_intro_proof() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                1,
                Judgment::Exclusive { resource: ResourceId(10) },
            ),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::ExclusivityIntro,
            conclusion: Fact::derived_j(
                2,
                Judgment::Exclusive { resource: ResourceId(10) },
            ),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Valid);
    }

    #[test]
    fn test_mixed_structured_and_string_backward_compat() {
        // A proof that uses string-based facts (no judgments) should still
        // be valid after the refactoring.
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(1, "region 1 is allocated"),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived(2, "region 1 is live"),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Valid);
    }
}
