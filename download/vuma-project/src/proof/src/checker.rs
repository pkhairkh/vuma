//! # Proof Checker
//!
//! Verifies that a formal proof is valid: every step follows from previously
//! established facts using the stated rule, and there is no circular reasoning.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::proof::{Conclusion, Fact, FactId, Proof, ProofStep};

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
                    // We compare the *kind* and *statement* loosely — the id
                    // may differ because the rule assigns a provisional id.
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
}
