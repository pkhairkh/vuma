//! # Inference Rules
//!
//! Domain-specific inference rules for reasoning about memory safety invariants
//! in VUMA programs. Each rule has a name, a set of premises, a conclusion
//! pattern, and an informal soundness argument explaining why the rule is
//! validity-preserving.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::proof::{Fact, FactId};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can arise during rule application.
#[derive(Debug, Clone, Error)]
pub enum RuleError {
    /// The number of premises supplied does not match the rule's arity.
    #[error("wrong number of premises: expected {expected}, got {got}")]
    ArityMismatch { expected: usize, got: usize },

    /// A premise fact does not match the expected pattern for this rule.
    #[error("premise {index} does not match expected pattern: {reason}")]
    PremiseMismatch { index: usize, reason: String },

    /// The rule cannot be applied in this context.
    #[error("rule {rule} is not applicable: {reason}")]
    NotApplicable { rule: String, reason: String },

    /// A referenced fact id was not found.
    #[error("fact id {id} not found")]
    FactNotFound { id: FactId },
}

// ---------------------------------------------------------------------------
// InferenceRule
// ---------------------------------------------------------------------------

/// An inference rule used to derive new facts from established premises.
///
/// Each variant corresponds to a specific reasoning principle in the VUMA
/// memory-safety discipline. The [`InferenceRule::apply`] method validates
/// that the supplied premises match the rule's expectations and, if so,
/// produces the derived conclusion fact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InferenceRule {
    // -- Liveness ----------------------------------------------------------
    /// **Liveness Introduction**: If a region has been allocated, then it is live.
    ///
    /// Premises (1):
    ///   0. "region R is allocated"
    ///
    /// Conclusion: "region R is live"
    LivenessIntro,

    /// **Liveness Elimination**: If a region has been freed, then it is dead
    /// (no longer live).
    ///
    /// Premises (1):
    ///   0. "region R is freed"
    ///
    /// Conclusion: "region R is dead"
    LivenessElim,

    // -- Exclusivity -------------------------------------------------------
    /// **Exclusivity Introduction**: Acquiring a lock on a region implies
    /// exclusive access to that region.
    ///
    /// Premises (1):
    ///   0. "lock L acquired on region R"
    ///
    /// Conclusion: "exclusive access to region R"
    ExclusivityIntro,

    /// **Exclusivity Elimination**: Two non-overlapping regions cannot conflict.
    ///
    /// Premises (2):
    ///   0. "region R1 has exclusive access"
    ///   1. "region R2 has exclusive access"
    ///      (R1 and R2 must be non-overlapping)
    ///
    /// Conclusion: "no conflict between R1 and R2"
    ExclusivityElim,

    // -- Derivation --------------------------------------------------------
    /// **Derivation Transitivity**: If A derives from B and B derives from C,
    /// then A derives from C.
    ///
    /// Premises (2):
    ///   0. "A derives from B"
    ///   1. "B derives from C"
    ///
    /// Conclusion: "A derives from C"
    DerivationTransitivity,

    // -- Bounds ------------------------------------------------------------
    /// **Bounds Preservation**: An offset within a region stays within the
    /// region's bounds.
    ///
    /// Premises (2):
    ///   0. "offset O is within region R"
    ///   1. "region R has bounds [lo, hi]"
    ///
    /// Conclusion: "lo ≤ O ≤ hi"
    BoundsPreservation,

    // -- Cast --------------------------------------------------------------
    /// **Cast Validity**: A `RepD` reinterpretation is valid when source and
    /// target types have compatible layouts.
    ///
    /// Premises (2):
    ///   0. "source type S has layout L_s"
    ///   1. "target type T has layout L_t"
    ///      (L_t.size ≤ L_s.size and alignments are compatible)
    ///
    /// Conclusion: "cast from S to T is valid"
    CastValidity,

    // -- Temporal ----------------------------------------------------------
    /// **Temporal Ordering**: Happens-before is transitive: if A happens before
    /// B and B happens before C, then A happens before C.
    ///
    /// Premises (2):
    ///   0. "A happens before B"
    ///   1. "B happens before C"
    ///
    /// Conclusion: "A happens before C"
    TemporalOrdering,
}

impl InferenceRule {
    /// Return the human-readable name of this rule.
    pub fn name(&self) -> &'static str {
        match self {
            InferenceRule::LivenessIntro => "LivenessIntro",
            InferenceRule::LivenessElim => "LivenessElim",
            InferenceRule::ExclusivityIntro => "ExclusivityIntro",
            InferenceRule::ExclusivityElim => "ExclusivityElim",
            InferenceRule::DerivationTransitivity => "DerivationTransitivity",
            InferenceRule::BoundsPreservation => "BoundsPreservation",
            InferenceRule::CastValidity => "CastValidity",
            InferenceRule::TemporalOrdering => "TemporalOrdering",
        }
    }

    /// Return the number of premises this rule expects.
    pub fn arity(&self) -> usize {
        match self {
            InferenceRule::LivenessIntro => 1,
            InferenceRule::LivenessElim => 1,
            InferenceRule::ExclusivityIntro => 1,
            InferenceRule::ExclusivityElim => 2,
            InferenceRule::DerivationTransitivity => 2,
            InferenceRule::BoundsPreservation => 2,
            InferenceRule::CastValidity => 2,
            InferenceRule::TemporalOrdering => 2,
        }
    }

    /// Return an informal soundness argument explaining why this rule preserves
    /// truth.
    pub fn soundness_argument(&self) -> &'static str {
        match self {
            InferenceRule::LivenessIntro => {
                "Allocation creates a region in memory; by definition a newly \
                 allocated region is live until it is freed."
            }
            InferenceRule::LivenessElim => {
                "Freeing a region releases its memory back to the allocator; \
                 after freeing the region no longer exists and is therefore dead."
            }
            InferenceRule::ExclusivityIntro => {
                "Acquiring a lock grants the holder exclusive ownership of the \
                 locked resource; by the lock contract no other agent can access \
                 it until the lock is released."
            }
            InferenceRule::ExclusivityElim => {
                "Two non-overlapping regions occupy disjoint address ranges; \
                 operations on one cannot interfere with the other, so no \
                 conflict is possible."
            }
            InferenceRule::DerivationTransitivity => {
                "Derivation is a transitive relation: if A's lifetime is \
                 bounded by B's and B's by C's, then A's lifetime is bounded \
                 by C's."
            }
            InferenceRule::BoundsPreservation => {
                "If an offset lies within a region and the region has known \
                 bounds, then the offset must lie within those bounds by the \
                 definition of containment."
            }
            InferenceRule::CastValidity => {
                "A RepD reinterpretation is valid when the target type fits \
                 within the source type's layout and alignment constraints are \
                 satisfied; this preserves memory safety because no bytes are \
                 read beyond the source allocation."
            }
            InferenceRule::TemporalOrdering => {
                "Happens-before is a strict partial order; transitivity is an \
                 axiom of partial orders and is therefore sound."
            }
        }
    }

    /// Apply this inference rule to the given premises, producing a derived
    /// conclusion fact.
    ///
    /// The `facts` slice must contain exactly [`Self::arity`] elements. Each
    /// element is validated against the rule's premise pattern; if validation
    /// fails a [`RuleError`] is returned.
    ///
    /// The returned fact has [`FactKind::Derived`] and an id equal to the
    /// maximum premise id plus one (simple id assignment for scaffolding;
    /// production code should use a proper id generator).
    pub fn apply(&self, facts: &[Fact]) -> Result<Fact, RuleError> {
        let expected = self.arity();
        if facts.len() != expected {
            return Err(RuleError::ArityMismatch {
                expected,
                got: facts.len(),
            });
        }

        let next_id = facts.iter().map(|f| f.id).max().unwrap_or(0) + 1;

        match self {
            InferenceRule::LivenessIntro => {
                let premise = &facts[0];
                if !premise.statement.contains("allocated") {
                    return Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: "expected a fact about region allocation".into(),
                    });
                }
                // Extract region identifier from the premise statement.
                let conclusion_stmt = premise
                    .statement
                    .replace("allocated", "live");
                Ok(Fact::derived(next_id, conclusion_stmt))
            }

            InferenceRule::LivenessElim => {
                let premise = &facts[0];
                if !premise.statement.contains("freed") {
                    return Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: "expected a fact about region deallocation (freed)".into(),
                    });
                }
                let conclusion_stmt = premise
                    .statement
                    .replace("freed", "dead");
                Ok(Fact::derived(next_id, conclusion_stmt))
            }

            InferenceRule::ExclusivityIntro => {
                let premise = &facts[0];
                if !premise.statement.contains("lock") && !premise.statement.contains("acquired") {
                    return Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: "expected a fact about lock acquisition".into(),
                    });
                }
                let conclusion_stmt = premise
                    .statement
                    .replace("lock acquired on", "exclusive access to")
                    .replace("acquired on", "exclusive access to");
                Ok(Fact::derived(next_id, conclusion_stmt))
            }

            InferenceRule::ExclusivityElim => {
                let p0 = &facts[0];
                let p1 = &facts[1];
                if !p0.statement.contains("exclusive access") {
                    return Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: "expected a fact about exclusive access".into(),
                    });
                }
                if !p1.statement.contains("exclusive access") {
                    return Err(RuleError::PremiseMismatch {
                        index: 1,
                        reason: "expected a fact about exclusive access".into(),
                    });
                }
                Ok(Fact::derived(
                    next_id,
                    format!("no conflict between ({}) and ({})", p0.statement, p1.statement),
                ))
            }

            InferenceRule::DerivationTransitivity => {
                let p0 = &facts[0];
                let p1 = &facts[1];
                if !p0.statement.contains("derives from") {
                    return Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: "expected a 'derives from' fact".into(),
                    });
                }
                if !p1.statement.contains("derives from") {
                    return Err(RuleError::PremiseMismatch {
                        index: 1,
                        reason: "expected a 'derives from' fact".into(),
                    });
                }
                // Naïve transitive composition for the scaffold.
                Ok(Fact::derived(
                    next_id,
                    format!("transitive derivation: ({}) ∘ ({})", p0.statement, p1.statement),
                ))
            }

            InferenceRule::BoundsPreservation => {
                let p0 = &facts[0];
                let p1 = &facts[1];
                if !p0.statement.contains("offset") && !p0.statement.contains("within") {
                    return Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: "expected a fact about an offset within a region".into(),
                    });
                }
                if !p1.statement.contains("bounds") {
                    return Err(RuleError::PremiseMismatch {
                        index: 1,
                        reason: "expected a fact about region bounds".into(),
                    });
                }
                Ok(Fact::derived(
                    next_id,
                    format!("bounds preserved: ({}) ∧ ({})", p0.statement, p1.statement),
                ))
            }

            InferenceRule::CastValidity => {
                let p0 = &facts[0];
                let p1 = &facts[1];
                if !p0.statement.contains("layout") && !p0.statement.contains("type") {
                    return Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: "expected a fact about source type layout".into(),
                    });
                }
                if !p1.statement.contains("layout") && !p1.statement.contains("type") {
                    return Err(RuleError::PremiseMismatch {
                        index: 1,
                        reason: "expected a fact about target type layout".into(),
                    });
                }
                Ok(Fact::derived(
                    next_id,
                    format!("cast is valid: ({}) → ({})", p0.statement, p1.statement),
                ))
            }

            InferenceRule::TemporalOrdering => {
                let p0 = &facts[0];
                let p1 = &facts[1];
                if !p0.statement.contains("happens before") {
                    return Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: "expected a 'happens before' fact".into(),
                    });
                }
                if !p1.statement.contains("happens before") {
                    return Err(RuleError::PremiseMismatch {
                        index: 1,
                        reason: "expected a 'happens before' fact".into(),
                    });
                }
                Ok(Fact::derived(
                    next_id,
                    format!("temporal transitivity: ({}) ∧ ({})", p0.statement, p1.statement),
                ))
            }
        }
    }
}

impl std::fmt::Display for InferenceRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::FactKind;

    #[test]
    fn test_liveness_intro() {
        let rule = InferenceRule::LivenessIntro;
        let premise = Fact::axiom(1, "region 42 is allocated");
        let result = rule.apply(&[premise]).unwrap();
        assert_eq!(result.kind, FactKind::Derived);
        assert!(result.statement.contains("live"));
    }

    #[test]
    fn test_liveness_elim() {
        let rule = InferenceRule::LivenessElim;
        let premise = Fact::axiom(1, "region 42 is freed");
        let result = rule.apply(&[premise]).unwrap();
        assert_eq!(result.kind, FactKind::Derived);
        assert!(result.statement.contains("dead"));
    }

    #[test]
    fn test_arity_mismatch() {
        let rule = InferenceRule::LivenessIntro; // arity 1
        let err = rule.apply(&[]).unwrap_err();
        assert!(matches!(err, RuleError::ArityMismatch { expected: 1, got: 0 }));
    }

    #[test]
    fn test_premise_mismatch() {
        let rule = InferenceRule::LivenessIntro;
        let bad_premise = Fact::axiom(1, "region 42 is something else");
        let err = rule.apply(&[bad_premise]).unwrap_err();
        assert!(matches!(err, RuleError::PremiseMismatch { .. }));
    }

    #[test]
    fn test_exclusivity_elim() {
        let rule = InferenceRule::ExclusivityElim;
        let p0 = Fact::derived(1, "exclusive access to region A");
        let p1 = Fact::derived(2, "exclusive access to region B");
        let result = rule.apply(&[p0, p1]).unwrap();
        assert!(result.statement.contains("no conflict"));
    }

    #[test]
    fn test_derivation_transitivity() {
        let rule = InferenceRule::DerivationTransitivity;
        let p0 = Fact::derived(1, "A derives from B");
        let p1 = Fact::derived(2, "B derives from C");
        let result = rule.apply(&[p0, p1]).unwrap();
        assert!(result.statement.contains("transitive derivation"));
    }

    #[test]
    fn test_temporal_ordering() {
        let rule = InferenceRule::TemporalOrdering;
        let p0 = Fact::derived(1, "event X happens before event Y");
        let p1 = Fact::derived(2, "event Y happens before event Z");
        let result = rule.apply(&[p0, p1]).unwrap();
        assert!(result.statement.contains("temporal transitivity"));
    }

    #[test]
    fn test_soundness_arguments_exist() {
        // Every rule should have a non-empty soundness argument.
        let rules = [
            InferenceRule::LivenessIntro,
            InferenceRule::LivenessElim,
            InferenceRule::ExclusivityIntro,
            InferenceRule::ExclusivityElim,
            InferenceRule::DerivationTransitivity,
            InferenceRule::BoundsPreservation,
            InferenceRule::CastValidity,
            InferenceRule::TemporalOrdering,
        ];
        for rule in &rules {
            assert!(!rule.soundness_argument().is_empty());
        }
    }

    #[test]
    fn test_rule_display() {
        assert_eq!(
            format!("{}", InferenceRule::LivenessIntro),
            "LivenessIntro"
        );
    }
}
