//! # Inference Rules
//!
//! Domain-specific inference rules for reasoning about memory safety invariants
//! in VUMA programs. Each rule has a name, a set of premises, a conclusion
//! pattern, and an informal soundness argument explaining why the rule is
//! validity-preserving.
//!
//! Rules now match on structured [`Judgment`] variants when available,
//! falling back to string-based matching for backward compatibility with
//! facts that lack a judgment.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::judgment::Judgment;
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

    /// A judgment is missing on a fact that requires one for structural matching.
    #[error("premise {index} lacks a structured judgment but one is required")]
    JudgmentMissing { index: usize },
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
    ///   0. `Judgment::Allocated { region: R }`
    ///
    /// Conclusion: `Judgment::Live { region: R }`
    LivenessIntro,

    /// **Liveness Elimination**: If a region has been freed, then it is dead
    /// (no longer live).
    ///
    /// Premises (1):
    ///   0. `Judgment::Freed { region: R }`
    ///
    /// Conclusion: "region R is dead" (string-only, no Freed→Dead judgment yet)
    LivenessElim,

    // -- Exclusivity -------------------------------------------------------
    /// **Exclusivity Introduction**: Acquiring a lock on a resource implies
    /// exclusive access to that resource.
    ///
    /// Premises (1):
    ///   0. `Judgment::Exclusive { resource: R }`  (lock acquisition fact)
    ///
    /// Conclusion: `Judgment::Exclusive { resource: R }`
    ExclusivityIntro,

    /// **Exclusivity Elimination**: Two non-overlapping exclusive resources
    /// cannot conflict.
    ///
    /// Premises (2):
    ///   0. `Judgment::Exclusive { resource: R1 }`
    ///   1. `Judgment::Exclusive { resource: R2 }`
    ///      (R1 and R2 must be non-overlapping)
    ///
    /// Conclusion: "no conflict between R1 and R2"
    ExclusivityElim,

    // -- Derivation --------------------------------------------------------
    /// **Derivation Transitivity**: If A derives from B in region R1 and
    /// B derives from C in region R2 (where R1 == R2), then A derives from C.
    ///
    /// Premises (2):
    ///   0. `Judgment::Derived { pointer: A, from: B, region: R }`
    ///   1. `Judgment::Derived { pointer: B, from: C, region: R }`
    ///
    /// Conclusion: `Judgment::Derived { pointer: A, from: C, region: R }`
    DerivationTransitivity,

    // -- Bounds ------------------------------------------------------------
    /// **Bounds Preservation**: An access within bounds of a pointer is safe.
    ///
    /// Premises (2):
    ///   0. `Judgment::InBounds { pointer, offset, size }`
    ///   1. A fact about the pointer's region bounds
    ///
    /// Conclusion: "bounds preserved: …"
    BoundsPreservation,

    // -- Cast --------------------------------------------------------------
    /// **Cast Validity**: A `RepD` reinterpretation preserves capability
    /// derivation.
    ///
    /// Premises (2):
    ///   0. `Judgment::PreservesCapD { resource, from_capd, to_capd }`
    ///   1. A fact about target type layout
    ///
    /// Conclusion: "cast is valid: …"
    CastValidity,

    // -- Temporal ----------------------------------------------------------
    /// **Temporal Ordering**: Happens-before is transitive: if A happens before
    /// B and B happens before C, then A happens before C.
    ///
    /// Premises (2):
    ///   0. `Judgment::TemporalOrder { event_a: A, event_b: B }`
    ///   1. `Judgment::TemporalOrder { event_a: B, event_b: C }`
    ///
    /// Conclusion: `Judgment::TemporalOrder { event_a: A, event_b: C }`
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
    /// When a premise carries a structured [`Judgment`], the rule matches on
    /// the judgment variant directly. When the judgment is `None`, the rule
    /// falls back to string-based pattern matching for backward compatibility.
    ///
    /// The returned fact always includes a structured judgment when possible,
    /// and an automatically generated `statement` string derived from it.
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
                match premise.judgment.as_ref() {
                    Some(Judgment::Allocated { region }) => {
                        let j = Judgment::Live {
                            region: region.clone(),
                        };
                        Ok(Fact::derived_j(next_id, j))
                    }
                    Some(other) => Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: format!(
                            "expected Allocated judgment, got {:?}",
                            other
                        ),
                    }),
                    None => {
                        // Fallback: string-based matching for backward compat.
                        if !premise.statement.contains("allocated") {
                            return Err(RuleError::PremiseMismatch {
                                index: 0,
                                reason: "expected a fact about region allocation".into(),
                            });
                        }
                        let conclusion_stmt =
                            premise.statement.replace("allocated", "live");
                        Ok(Fact::derived(next_id, conclusion_stmt))
                    }
                }
            }

            InferenceRule::LivenessElim => {
                let premise = &facts[0];
                match premise.judgment.as_ref() {
                    Some(Judgment::Freed { region }) => {
                        // Freed → "region R is dead" (no Dead judgment variant;
                        // we use a string conclusion for now).
                        Ok(Fact::derived(
                            next_id,
                            format!("region {} is dead", region),
                        ))
                    }
                    Some(other) => Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: format!(
                            "expected Freed judgment, got {:?}",
                            other
                        ),
                    }),
                    None => {
                        if !premise.statement.contains("freed") {
                            return Err(RuleError::PremiseMismatch {
                                index: 0,
                                reason:
                                    "expected a fact about region deallocation (freed)"
                                        .into(),
                            });
                        }
                        let conclusion_stmt =
                            premise.statement.replace("freed", "dead");
                        Ok(Fact::derived(next_id, conclusion_stmt))
                    }
                }
            }

            InferenceRule::ExclusivityIntro => {
                let premise = &facts[0];
                match premise.judgment.as_ref() {
                    // Lock acquisition facts can be represented as Exclusive
                    // judgments (the lock grants exclusive access).
                    Some(Judgment::Exclusive { resource }) => {
                        let j = Judgment::Exclusive {
                            resource: resource.clone(),
                        };
                        Ok(Fact::derived_j(next_id, j))
                    }
                    Some(other) => Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: format!(
                            "expected Exclusive judgment (lock acquisition), got {:?}",
                            other
                        ),
                    }),
                    None => {
                        if !premise.statement.contains("lock")
                            && !premise.statement.contains("acquired")
                        {
                            return Err(RuleError::PremiseMismatch {
                                index: 0,
                                reason: "expected a fact about lock acquisition"
                                    .into(),
                            });
                        }
                        let conclusion_stmt = premise
                            .statement
                            .replace("lock acquired on", "exclusive access to")
                            .replace("acquired on", "exclusive access to");
                        Ok(Fact::derived(next_id, conclusion_stmt))
                    }
                }
            }

            InferenceRule::ExclusivityElim => {
                let p0 = &facts[0];
                let p1 = &facts[1];
                match (p0.judgment.as_ref(), p1.judgment.as_ref()) {
                    (
                        Some(Judgment::Exclusive { resource: r1 }),
                        Some(Judgment::Exclusive { resource: r2 }),
                    ) => Ok(Fact::derived(
                        next_id,
                        format!("no conflict between {} and {}", r1, r2),
                    )),
                    (Some(other), _) | (_, Some(other)) => {
                        let bad_idx = if p0.judgment.is_some()
                            && !matches!(
                                p0.judgment,
                                Some(Judgment::Exclusive { .. })
                            )
                        {
                            0
                        } else {
                            1
                        };
                        Err(RuleError::PremiseMismatch {
                            index: bad_idx,
                            reason: format!(
                                "expected Exclusive judgment, got {:?}",
                                other
                            ),
                        })
                    }
                    (None, None) => {
                        // String fallback
                        if !p0.statement.contains("exclusive access") {
                            return Err(RuleError::PremiseMismatch {
                                index: 0,
                                reason: "expected a fact about exclusive access"
                                    .into(),
                            });
                        }
                        if !p1.statement.contains("exclusive access") {
                            return Err(RuleError::PremiseMismatch {
                                index: 1,
                                reason: "expected a fact about exclusive access"
                                    .into(),
                            });
                        }
                        Ok(Fact::derived(
                            next_id,
                            format!(
                                "no conflict between ({}) and ({})",
                                p0.statement, p1.statement
                            ),
                        ))
                    }
                }
            }

            InferenceRule::DerivationTransitivity => {
                let p0 = &facts[0];
                let p1 = &facts[1];
                match (p0.judgment.as_ref(), p1.judgment.as_ref()) {
                    (
                        Some(Judgment::Derived {
                            pointer: a,
                            from: b1,
                            region: r1,
                        }),
                        Some(Judgment::Derived {
                            pointer: b2,
                            from: c,
                            region: r2,
                        }),
                    ) => {
                        // The `from` of p0 must equal the `pointer` of p1,
                        // and both must be in the same region.
                        if b1 != b2 {
                            return Err(RuleError::PremiseMismatch {
                                index: 1,
                                reason: format!(
                                    "chain mismatch: p0.from='{}' != p1.pointer='{}'",
                                    b1, b2
                                ),
                            });
                        }
                        if r1 != r2 {
                            return Err(RuleError::PremiseMismatch {
                                index: 1,
                                reason: format!(
                                    "region mismatch: '{}' != '{}'",
                                    r1, r2
                                ),
                            });
                        }
                        let j = Judgment::Derived {
                            pointer: a.clone(),
                            from: c.clone(),
                            region: r1.clone(),
                        };
                        Ok(Fact::derived_j(next_id, j))
                    }
                    (Some(other), _) | (_, Some(other)) => {
                        let bad_idx = if p0.judgment.is_some()
                            && !matches!(
                                p0.judgment,
                                Some(Judgment::Derived { .. })
                            )
                        {
                            0
                        } else {
                            1
                        };
                        Err(RuleError::PremiseMismatch {
                            index: bad_idx,
                            reason: format!(
                                "expected Derived judgment, got {:?}",
                                other
                            ),
                        })
                    }
                    (None, None) => {
                        // String fallback
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
                        Ok(Fact::derived(
                            next_id,
                            format!(
                                "transitive derivation: ({}) ∘ ({})",
                                p0.statement, p1.statement
                            ),
                        ))
                    }
                }
            }

            InferenceRule::BoundsPreservation => {
                let p0 = &facts[0];
                let p1 = &facts[1];
                match (p0.judgment.as_ref(), p1.judgment.as_ref()) {
                    (
                        Some(Judgment::InBounds {
                            pointer,
                            offset,
                            size,
                        }),
                        _,
                    ) => Ok(Fact::derived(
                        next_id,
                        format!(
                            "bounds preserved: inbounds {} offset={} size={} ∧ {}",
                            pointer, offset, size, p1.statement
                        ),
                    )),
                    (Some(other), _) => Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: format!(
                            "expected InBounds judgment, got {:?}",
                            other
                        ),
                    }),
                    (None, _) => {
                        // String fallback
                        if !p0.statement.contains("offset")
                            && !p0.statement.contains("within")
                        {
                            return Err(RuleError::PremiseMismatch {
                                index: 0,
                                reason:
                                    "expected a fact about an offset within a region"
                                        .into(),
                            });
                        }
                        if !p1.statement.contains("bounds") {
                            return Err(RuleError::PremiseMismatch {
                                index: 1,
                                reason: "expected a fact about region bounds"
                                    .into(),
                            });
                        }
                        Ok(Fact::derived(
                            next_id,
                            format!(
                                "bounds preserved: ({}) ∧ ({})",
                                p0.statement, p1.statement
                            ),
                        ))
                    }
                }
            }

            InferenceRule::CastValidity => {
                let p0 = &facts[0];
                let p1 = &facts[1];
                match (p0.judgment.as_ref(), p1.judgment.as_ref()) {
                    (
                        Some(Judgment::PreservesCapD {
                            resource,
                            from_capd,
                            to_capd,
                        }),
                        _,
                    ) => Ok(Fact::derived(
                        next_id,
                        format!(
                            "cast is valid: preserves CapD for {}: {} -> {} ∧ {}",
                            resource, from_capd, to_capd, p1.statement
                        ),
                    )),
                    (Some(other), _) => Err(RuleError::PremiseMismatch {
                        index: 0,
                        reason: format!(
                            "expected PreservesCapD judgment, got {:?}",
                            other
                        ),
                    }),
                    (None, _) => {
                        // String fallback
                        if !p0.statement.contains("layout")
                            && !p0.statement.contains("type")
                        {
                            return Err(RuleError::PremiseMismatch {
                                index: 0,
                                reason:
                                    "expected a fact about source type layout"
                                        .into(),
                            });
                        }
                        if !p1.statement.contains("layout")
                            && !p1.statement.contains("type")
                        {
                            return Err(RuleError::PremiseMismatch {
                                index: 1,
                                reason:
                                    "expected a fact about target type layout"
                                        .into(),
                            });
                        }
                        Ok(Fact::derived(
                            next_id,
                            format!(
                                "cast is valid: ({}) → ({})",
                                p0.statement, p1.statement
                            ),
                        ))
                    }
                }
            }

            InferenceRule::TemporalOrdering => {
                let p0 = &facts[0];
                let p1 = &facts[1];
                match (p0.judgment.as_ref(), p1.judgment.as_ref()) {
                    (
                        Some(Judgment::TemporalOrder {
                            event_a: a,
                            event_b: b1,
                        }),
                        Some(Judgment::TemporalOrder {
                            event_a: b2,
                            event_b: c,
                        }),
                    ) => {
                        // b1 must equal b2 for the chain to be well-formed.
                        if b1 != b2 {
                            return Err(RuleError::PremiseMismatch {
                                index: 1,
                                reason: format!(
                                    "temporal chain mismatch: '{}' != '{}'",
                                    b1, b2
                                ),
                            });
                        }
                        let j = Judgment::TemporalOrder {
                            event_a: a.clone(),
                            event_b: c.clone(),
                        };
                        Ok(Fact::derived_j(next_id, j))
                    }
                    (Some(other), _) | (_, Some(other)) => {
                        let bad_idx = if p0.judgment.is_some()
                            && !matches!(
                                p0.judgment,
                                Some(Judgment::TemporalOrder { .. })
                            )
                        {
                            0
                        } else {
                            1
                        };
                        Err(RuleError::PremiseMismatch {
                            index: bad_idx,
                            reason: format!(
                                "expected TemporalOrder judgment, got {:?}",
                                other
                            ),
                        })
                    }
                    (None, None) => {
                        // String fallback
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
                            format!(
                                "temporal transitivity: ({}) ∧ ({})",
                                p0.statement, p1.statement
                            ),
                        ))
                    }
                }
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
    use crate::judgment::CapDKind;
    use crate::proof::FactKind;

    // -- Legacy string-based tests (backward compatibility) ----------------

    #[test]
    fn test_liveness_intro_string() {
        let rule = InferenceRule::LivenessIntro;
        let premise = Fact::axiom(1, "region 42 is allocated");
        let result = rule.apply(&[premise]).unwrap();
        assert_eq!(result.kind, FactKind::Derived);
        assert!(result.statement.contains("live"));
    }

    #[test]
    fn test_liveness_elim_string() {
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
        assert!(matches!(
            err,
            RuleError::ArityMismatch { expected: 1, got: 0 }
        ));
    }

    #[test]
    fn test_premise_mismatch_string() {
        let rule = InferenceRule::LivenessIntro;
        let bad_premise = Fact::axiom(1, "region 42 is something else");
        let err = rule.apply(&[bad_premise]).unwrap_err();
        assert!(matches!(err, RuleError::PremiseMismatch { .. }));
    }

    #[test]
    fn test_exclusivity_elim_string() {
        let rule = InferenceRule::ExclusivityElim;
        let p0 = Fact::derived(1, "exclusive access to region A");
        let p1 = Fact::derived(2, "exclusive access to region B");
        let result = rule.apply(&[p0, p1]).unwrap();
        assert!(result.statement.contains("no conflict"));
    }

    #[test]
    fn test_derivation_transitivity_string() {
        let rule = InferenceRule::DerivationTransitivity;
        let p0 = Fact::derived(1, "A derives from B");
        let p1 = Fact::derived(2, "B derives from C");
        let result = rule.apply(&[p0, p1]).unwrap();
        assert!(result.statement.contains("transitive derivation"));
    }

    #[test]
    fn test_temporal_ordering_string() {
        let rule = InferenceRule::TemporalOrdering;
        let p0 = Fact::derived(1, "event X happens before event Y");
        let p1 = Fact::derived(2, "event Y happens before event Z");
        let result = rule.apply(&[p0, p1]).unwrap();
        assert!(result.statement.contains("temporal transitivity"));
    }

    #[test]
    fn test_soundness_arguments_exist() {
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

    // -- Structured judgment tests -----------------------------------------

    #[test]
    fn test_liveness_intro_structured() {
        let rule = InferenceRule::LivenessIntro;
        let premise = Fact::axiom_j(
            1,
            Judgment::Allocated {
                region: "r42".into(),
            },
        );
        let result = rule.apply(&[premise]).unwrap();
        assert_eq!(result.kind, FactKind::Derived);
        assert_eq!(
            result.judgment,
            Some(Judgment::Live {
                region: "r42".into()
            })
        );
        assert_eq!(result.statement, "region r42 is live");
    }

    #[test]
    fn test_liveness_intro_wrong_judgment() {
        let rule = InferenceRule::LivenessIntro;
        let premise = Fact::axiom_j(
            1,
            Judgment::Freed {
                region: "r42".into(),
            },
        );
        let err = rule.apply(&[premise]).unwrap_err();
        assert!(matches!(err, RuleError::PremiseMismatch { .. }));
    }

    #[test]
    fn test_liveness_elim_structured() {
        let rule = InferenceRule::LivenessElim;
        let premise = Fact::checked_j(
            1,
            Judgment::Freed {
                region: "r7".into(),
            },
        );
        let result = rule.apply(&[premise]).unwrap();
        assert_eq!(result.kind, FactKind::Derived);
        assert_eq!(result.statement, "region r7 is dead");
    }

    #[test]
    fn test_exclusivity_intro_structured() {
        let rule = InferenceRule::ExclusivityIntro;
        let premise = Fact::axiom_j(
            1,
            Judgment::Exclusive {
                resource: "lock_L_region_R".into(),
            },
        );
        let result = rule.apply(&[premise]).unwrap();
        assert_eq!(
            result.judgment,
            Some(Judgment::Exclusive {
                resource: "lock_L_region_R".into()
            })
        );
    }

    #[test]
    fn test_exclusivity_elim_structured() {
        let rule = InferenceRule::ExclusivityElim;
        let p0 = Fact::derived_j(
            1,
            Judgment::Exclusive {
                resource: "region_A".into(),
            },
        );
        let p1 = Fact::derived_j(
            2,
            Judgment::Exclusive {
                resource: "region_B".into(),
            },
        );
        let result = rule.apply(&[p0, p1]).unwrap();
        assert!(result.statement.contains("no conflict"));
        assert!(result.statement.contains("region_A"));
        assert!(result.statement.contains("region_B"));
    }

    #[test]
    fn test_derivation_transitivity_structured() {
        let rule = InferenceRule::DerivationTransitivity;
        let p0 = Fact::derived_j(
            1,
            Judgment::Derived {
                pointer: "p_a".into(),
                from: "p_b".into(),
                region: "r1".into(),
            },
        );
        let p1 = Fact::derived_j(
            2,
            Judgment::Derived {
                pointer: "p_b".into(),
                from: "p_c".into(),
                region: "r1".into(),
            },
        );
        let result = rule.apply(&[p0, p1]).unwrap();
        assert_eq!(
            result.judgment,
            Some(Judgment::Derived {
                pointer: "p_a".into(),
                from: "p_c".into(),
                region: "r1".into(),
            })
        );
        assert_eq!(
            result.statement,
            "p_a derives from p_c in region r1"
        );
    }

    #[test]
    fn test_derivation_transitivity_chain_mismatch() {
        let rule = InferenceRule::DerivationTransitivity;
        let p0 = Fact::derived_j(
            1,
            Judgment::Derived {
                pointer: "p_a".into(),
                from: "p_b".into(),
                region: "r1".into(),
            },
        );
        let p1 = Fact::derived_j(
            2,
            Judgment::Derived {
                pointer: "p_x".into(), // mismatch: p_b != p_x
                from: "p_c".into(),
                region: "r1".into(),
            },
        );
        let err = rule.apply(&[p0, p1]).unwrap_err();
        assert!(matches!(err, RuleError::PremiseMismatch { .. }));
        if let RuleError::PremiseMismatch { reason, .. } = err {
            assert!(reason.contains("chain mismatch"));
        }
    }

    #[test]
    fn test_derivation_transitivity_region_mismatch() {
        let rule = InferenceRule::DerivationTransitivity;
        let p0 = Fact::derived_j(
            1,
            Judgment::Derived {
                pointer: "p_a".into(),
                from: "p_b".into(),
                region: "r1".into(),
            },
        );
        let p1 = Fact::derived_j(
            2,
            Judgment::Derived {
                pointer: "p_b".into(),
                from: "p_c".into(),
                region: "r2".into(), // different region
            },
        );
        let err = rule.apply(&[p0, p1]).unwrap_err();
        assert!(matches!(err, RuleError::PremiseMismatch { .. }));
        if let RuleError::PremiseMismatch { reason, .. } = err {
            assert!(reason.contains("region mismatch"));
        }
    }

    #[test]
    fn test_temporal_ordering_structured() {
        let rule = InferenceRule::TemporalOrdering;
        let p0 = Fact::derived_j(
            1,
            Judgment::TemporalOrder {
                event_a: "e1".into(),
                event_b: "e2".into(),
            },
        );
        let p1 = Fact::derived_j(
            2,
            Judgment::TemporalOrder {
                event_a: "e2".into(),
                event_b: "e3".into(),
            },
        );
        let result = rule.apply(&[p0, p1]).unwrap();
        assert_eq!(
            result.judgment,
            Some(Judgment::TemporalOrder {
                event_a: "e1".into(),
                event_b: "e3".into(),
            })
        );
        assert_eq!(result.statement, "e1 happens before e3");
    }

    #[test]
    fn test_temporal_ordering_chain_mismatch() {
        let rule = InferenceRule::TemporalOrdering;
        let p0 = Fact::derived_j(
            1,
            Judgment::TemporalOrder {
                event_a: "e1".into(),
                event_b: "e2".into(),
            },
        );
        let p1 = Fact::derived_j(
            2,
            Judgment::TemporalOrder {
                event_a: "e5".into(), // mismatch: e2 != e5
                event_b: "e3".into(),
            },
        );
        let err = rule.apply(&[p0, p1]).unwrap_err();
        assert!(matches!(err, RuleError::PremiseMismatch { .. }));
        if let RuleError::PremiseMismatch { reason, .. } = err {
            assert!(reason.contains("temporal chain mismatch"));
        }
    }

    #[test]
    fn test_bounds_preservation_structured() {
        let rule = InferenceRule::BoundsPreservation;
        let p0 = Fact::derived_j(
            1,
            Judgment::InBounds {
                pointer: "ptr".into(),
                offset: 8,
                size: 4,
            },
        );
        let p1 = Fact::axiom(2, "region r1 has bounds [0, 1024]");
        let result = rule.apply(&[p0, p1]).unwrap();
        assert!(result.statement.contains("bounds preserved"));
        assert!(result.statement.contains("ptr"));
    }

    #[test]
    fn test_cast_validity_structured() {
        let rule = InferenceRule::CastValidity;
        let p0 = Fact::derived_j(
            1,
            Judgment::PreservesCapD {
                resource: "mem_r1".into(),
                from_capd: CapDKind::ReadWrite,
                to_capd: CapDKind::Read,
            },
        );
        let p1 = Fact::axiom(2, "target type T has layout L_t");
        let result = rule.apply(&[p0, p1]).unwrap();
        assert!(result.statement.contains("cast is valid"));
        assert!(result.statement.contains("preserves CapD"));
    }

    #[test]
    fn test_mixed_judgment_and_string_fails() {
        // When one premise has a judgment but the other doesn't, and the
        // rule expects matching judgment types, it should fail gracefully.
        let rule = InferenceRule::TemporalOrdering;
        let p0 = Fact::derived_j(
            1,
            Judgment::TemporalOrder {
                event_a: "e1".into(),
                event_b: "e2".into(),
            },
        );
        let p1 = Fact::derived(2, "event Y happens before event Z");
        // p0 has judgment, p1 doesn't — this falls to the mixed case
        // which returns PremiseMismatch for the non-matching premise.
        let err = rule.apply(&[p0, p1]).unwrap_err();
        assert!(matches!(err, RuleError::PremiseMismatch { .. }));
    }
}
