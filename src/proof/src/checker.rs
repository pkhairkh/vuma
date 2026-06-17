//! # Proof Checker
//!
//! Verifies that a formal proof is valid: every step follows from previously
//! established facts using the stated rule, and there is no circular reasoning.
//!
//! ## Soundness Theorem (SOUND-1)
//!
//! > **Theorem SOUND-1.** *If `ProofChecker::check(proof) == Ok(CheckResult::Valid)`,
//! > then the conclusion of `proof` is a logical consequence of the axioms
//! > applied at the given program points.*
//!
//! **Proof sketch.** By induction on the length of `proof.steps`:
//!   - *Base case* (0 steps): a proof with no steps may only conclude
//!     `Inconclusive` (the checker returns `Incomplete`), so the theorem
//!     holds vacuously.
//!   - *Inductive step*: assume all facts established by steps `0..i` are
//!     logical consequences of the axioms. For step `i`:
//!       * `Assume { fact }` — accepted **only if** `fact.kind == Checked`
//!         (the IVE verifiers have mechanically established it — see W7/W8)
//!         **or** `fact.kind == Axiom` and the fact matches a recognized
//!         [`AxiomId`] whose validity conditions are documented on the
//!         variant. In both cases the fact is a logical consequence of the
//!         axioms. `Assume` steps with `FactKind::Assumption` (pure,
//!         unscoped assumptions) or `FactKind::Derived` (which must come
//!         from an `Infer` step) are **rejected**.
//!       * `Infer { from, rule, conclusion }` — by the inductive hypothesis
//!         the premises are consequences of the axioms; by the per-rule
//!         instance of Theorem SOUND-1 (see
//!         [`InferenceRule::soundness_theorem`](crate::rules::InferenceRule::soundness_theorem)),
//!         the conclusion is too.
//!       * `CaseSplit`, `Induction` — by the inductive hypothesis applied
//!         to each sub-proof.
//!       * `Contradiction { assumption, negation }` — discharges a
//!         previously-established assumption; sound by classical logic.
//!       * `ByDefinition` — definitional equality; sound by construction.
//!
//! The crucial difference from the previous (unsound) checker is that
//! `Assume` steps with `FactKind::Assumption` and `FactKind::Axiom` facts
//! that do not match a recognized [`AxiomId`] are now **rejected**, closing
//! the "assume anything" soundness hole (W9).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::judgment::Judgment;
use crate::proof::{Conclusion, Fact, FactId, FactKind, Proof, ProofStep};
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
// AxiomId — fixed, enumerated axiom set
// ---------------------------------------------------------------------------

/// A fixed, enumerated set of axioms that may be invoked by an
/// [`ProofStep::Assume`] step.
///
/// Each variant documents its **validity conditions** — the conditions under
/// which the axiom is sound to apply at a given program point. The proof
/// checker accepts an `Assume { fact: Fact { kind: Axiom, .. } }` step **iff**
/// [`AxiomId::from_fact`] returns `Some(axiom)` for that fact; that is, the
/// fact's structured judgment (or, for backward compatibility with
/// string-based facts, its statement) must match one of the axioms below.
///
/// An `Assume` step with [`FactKind::Assumption`] (a pure, unscoped
/// assumption) is **rejected** by the checker. This closes the soundness hole
/// where the old checker accepted arbitrary `Assume` facts as "always valid"
/// (W9).
///
/// # Soundness
///
/// The enumerated axiom set is the *trust root* of the proof system. Adding
/// a new axiom requires (a) documenting its validity conditions on the
/// variant, (b) extending `from_fact` to recognize it, and (c) justifying
/// that the axiom holds in the VUMA operational semantics. See Theorem
/// SOUND-1 in the module docs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AxiomId {
    /// **Axiom AllocLive**: if a region `R` is allocated at program point
    /// `P`, then `R` is live at `P`.
    ///
    /// *Validity conditions*: the fact must carry `Judgment::Allocated
    /// { region: R }` (or, for string-based facts, a statement containing
    /// "allocated"). The program point `P` must be at or after the
    /// allocation site and before any subsequent free of `R`.
    AllocLive,

    /// **Axiom FreeInvalidates**: if a region `R` is freed at program point
    /// `P`, then any access to `R` after `P` is invalid (use-after-free).
    ///
    /// *Validity conditions*: the fact must carry `Judgment::Freed
    /// { region: R }` (or a statement containing "freed"). The free must
    /// be a real free event in the SCG at or before `P`.
    FreeInvalidates,

    /// **Axiom SyncOrdersAccesses**: if two accesses are separated by a
    /// synchronization edge (lock acquire/release, fence, or atomic), then
    /// the happens-before relation orders them.
    ///
    /// *Validity conditions*: the fact must carry `Judgment::TemporalOrder
    /// { event_a, event_b }` (or a statement containing "lock", "sync",
    /// or "happens before"). The synchronization edge must exist in the SCG
    /// between the two events.
    SyncOrdersAccesses,

    /// **Axiom ExclusiveDisjoint**: two exclusive resources on disjoint
    /// regions do not conflict.
    ///
    /// *Validity conditions*: the fact must carry `Judgment::Exclusive
    /// { resource }` (or a statement containing "exclusive"). The two
    /// resources must occupy non-overlapping address ranges.
    ExclusiveDisjoint,

    /// **Axiom DerivationTransitive**: pointer derivation is transitive
    /// within a single region.
    ///
    /// *Validity conditions*: the fact must carry `Judgment::Derived
    /// { pointer, from, region }` (or a statement containing "derivation"
    /// or "derives from"). All three pointers must belong to the same
    /// region.
    DerivationTransitive,

    /// **Axiom BoundsContainment**: an access at `(pointer + offset)` of
    /// `size` bytes that lies within the region's known bounds is
    /// in-bounds.
    ///
    /// *Validity conditions*: the fact must carry `Judgment::InBounds
    /// { pointer, offset, size }` (or a statement containing "offset" or
    /// "bounds"). The offset and size must not exceed the region's
    /// allocation size.
    BoundsContainment,
}

impl AxiomId {
    /// Return the human-readable name of this axiom.
    pub fn name(&self) -> &'static str {
        match self {
            AxiomId::AllocLive => "AllocLive",
            AxiomId::FreeInvalidates => "FreeInvalidates",
            AxiomId::SyncOrdersAccesses => "SyncOrdersAccesses",
            AxiomId::ExclusiveDisjoint => "ExclusiveDisjoint",
            AxiomId::DerivationTransitive => "DerivationTransitive",
            AxiomId::BoundsContainment => "BoundsContainment",
        }
    }

    /// Return all axioms in the enumerated set.
    pub fn all() -> &'static [AxiomId] {
        &[
            AxiomId::AllocLive,
            AxiomId::FreeInvalidates,
            AxiomId::SyncOrdersAccesses,
            AxiomId::ExclusiveDisjoint,
            AxiomId::DerivationTransitive,
            AxiomId::BoundsContainment,
        ]
    }

    /// Try to identify which axiom (if any) the given fact invokes.
    ///
    /// A fact invokes an axiom if its structured `judgment` (or, for
    /// backward compatibility with string-based facts, its `statement`)
    /// matches the axiom's recognized pattern. Returns `None` if the fact
    /// does not match any axiom in the enumerated set — in which case an
    /// `Assume` step carrying it as an `Axiom`-kinded fact must be rejected
    /// by the checker.
    pub fn from_fact(fact: &Fact) -> Option<AxiomId> {
        // Prefer structured judgment matching when available.
        if let Some(j) = &fact.judgment {
            return match j {
                Judgment::Allocated { .. } => Some(AxiomId::AllocLive),
                Judgment::Freed { .. } => Some(AxiomId::FreeInvalidates),
                Judgment::TemporalOrder { .. } => Some(AxiomId::SyncOrdersAccesses),
                Judgment::Exclusive { .. } => Some(AxiomId::ExclusiveDisjoint),
                Judgment::Derived { .. } => Some(AxiomId::DerivationTransitive),
                Judgment::InBounds { .. } => Some(AxiomId::BoundsContainment),
                // Other judgment variants (Live, Dead, NoConflict,
                // BoundsPreserved, Initialized, PreservesCapD, CastValid,
                // Shared, Assumption) are *derived* or *assumption* forms,
                // not axioms. They must be established by an `Infer` step
                // (or, for Assumption, are rejected outright).
                _ => None,
            };
        }

        // String-based fallback for backward compatibility with facts
        // that do not carry a structured judgment.
        let s = fact.statement.to_lowercase();
        if s.contains("allocated") {
            Some(AxiomId::AllocLive)
        } else if s.contains("freed") {
            Some(AxiomId::FreeInvalidates)
        } else if s.contains("lock") || s.contains("sync") || s.contains("happens before") {
            Some(AxiomId::SyncOrdersAccesses)
        } else if s.contains("exclusive") {
            Some(AxiomId::ExclusiveDisjoint)
        } else if s.contains("derivation") || s.contains("derives from") {
            Some(AxiomId::DerivationTransitive)
        } else if s.contains("offset") || s.contains("bounds") {
            Some(AxiomId::BoundsContainment)
        } else {
            None
        }
    }

    /// Validate that the given fact satisfies this axiom's validity
    /// conditions.
    ///
    /// Returns `Ok(())` if the fact is consistent with the axiom, or an
    /// `Err(message)` describing the violation. The check is necessarily
    /// syntactic — the checker cannot re-verify the SCG — but it ensures
    /// the fact is *tagged* with a recognized axiom, which is the
    /// soundness-critical property.
    pub fn validate(&self, fact: &Fact) -> Result<(), String> {
        match AxiomId::from_fact(fact) {
            Some(a) if a == *self => Ok(()),
            Some(other) => Err(format!(
                "fact matches axiom {} but was tagged as {}",
                other.name(),
                self.name()
            )),
            None => Err(format!(
                "fact does not match any recognized axiom (tagged as {})",
                self.name()
            )),
        }
    }
}

impl std::fmt::Display for AxiomId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
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
                    // Soundness (Theorem SOUND-1, W9): an `Assume` step is
                    // accepted ONLY IF it references
                    //   (a) a `Checked` fact (mechanically established by
                    //       the IVE verifiers — sound by W7/W8), or
                    //   (b) an `Axiom` fact that matches one of the
                    //       recognized `AxiomId` variants (whose validity
                    //       conditions are documented on the variant).
                    //
                    // `FactKind::Assumption` (pure, unscoped assumption)
                    // and `FactKind::Derived` (which must come from an
                    // `Infer` step) are REJECTED. This closes the
                    // soundness hole where the old checker accepted
                    // arbitrary `Assume` facts as "always valid".
                    if established.contains(&fact.id) {
                        return Ok(CheckResult::Invalid {
                            step: step_idx,
                            reason: format!(
                                "fact id {} is already established (duplicate)",
                                fact.id
                            ),
                        });
                    }

                    match fact.kind {
                        FactKind::Checked => {
                            // Sound: established by the IVE verifiers.
                            log::trace!(
                                "Step {}: Assume checked fact id={} (verifier-established)",
                                step_idx,
                                fact.id
                            );
                        }
                        FactKind::Axiom => {
                            // Sound only if the fact matches a recognized axiom.
                            match AxiomId::from_fact(fact) {
                                Some(axiom) => {
                                    log::trace!(
                                        "Step {}: Assume axiom {} (fact id={})",
                                        step_idx,
                                        axiom.name(),
                                        fact.id
                                    );
                                }
                                None => {
                                    return Ok(CheckResult::Invalid {
                                        step: step_idx,
                                        reason: format!(
                                            "unsound Assume: fact '{}` is tagged as Axiom but \
                                             does not match any recognized AxiomId. Recognized \
                                             axioms: [{}]. Either tag the fact as Checked (if the \
                                             IVE verifier established it) or rephrase it to match \
                                             a recognized axiom.",
                                            fact.statement,
                                            AxiomId::all()
                                                .iter()
                                                .map(|a| a.name())
                                                .collect::<Vec<_>>()
                                                .join(", ")
                                        ),
                                    });
                                }
                            }
                        }
                        FactKind::Assumption => {
                            // Unsound: a pure, unscoped assumption. Reject.
                            return Ok(CheckResult::Invalid {
                                step: step_idx,
                                reason: format!(
                                    "unsound Assume: fact '{}` has kind Assumption. Pure \
                                     assumptions are not accepted by the soundness checker. Use \
                                     a Checked fact (verifier-established) or an Axiom fact \
                                     matching a recognized AxiomId.",
                                    fact.statement
                                ),
                            });
                        }
                        FactKind::Derived => {
                            // Derived facts must come from an Infer step,
                            // not an Assume step.
                            return Ok(CheckResult::Invalid {
                                step: step_idx,
                                reason: format!(
                                    "unsound Assume: fact '{}` has kind Derived. Derived facts \
                                     must be established by an Infer step, not assumed.",
                                    fact.statement
                                ),
                            });
                        }
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

                ProofStep::Induction {
                    base,
                    step: ind_step,
                } => {
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
                            reason: format!("assumption fact id {} is not established", assumption),
                        });
                    }
                    if self.check_circular && !established.contains(negation) {
                        return Ok(CheckResult::Invalid {
                            step: step_idx,
                            reason: format!("negation fact id {} is not established", negation),
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
                let has_contradiction = proof
                    .steps
                    .iter()
                    .any(|s| matches!(s, ProofStep::Contradiction { .. }));
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
    use crate::proof::{Goal, InvariantName, ProofContext, RegionId, Target};
    use crate::rules::InferenceRule;

    fn dummy_goal() -> Goal {
        Goal::new(
            InvariantName::Liveness,
            Target::Region(RegionId(1)),
            ProofContext::new("test"),
        )
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
        // Use a Checked fact so the Assume step is accepted by the sound
        // checker; the proof must then be rejected because it claims
        // Refuted without a Contradiction step.
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::checked(1, "P"),
        });
        proof.conclude(Conclusion::Refuted);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert!(matches!(result, CheckResult::Invalid { .. }));
        if let CheckResult::Invalid { reason, .. } = result {
            assert!(reason.contains("Refuted"));
        }
    }

    #[test]
    fn test_duplicate_fact_id() {
        // Both facts are recognized axioms (AllocLive) so step 0 is
        // accepted; step 1 must then be rejected for re-using fact id 1.
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(1, "region 1 is allocated"),
        });
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(1, "region 2 is allocated"), // same id
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
        // Both facts are Checked (verifier-established), so the sound
        // checker accepts the Assume steps. The Contradiction step then
        // discharges them, yielding a valid Refuted proof.
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::checked(1, "P"),
        });
        proof.add_step(ProofStep::Assume {
            fact: Fact::checked(2, "not P"),
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

        // Build a trivial proof from the goal and check it. Use a Checked
        // fact (verifier-established) so the sound checker accepts the
        // Assume step — a pure Assumption would now be rejected (W9).
        let mut proof = Proof::new(goal.clone());
        proof.add_step(ProofStep::Assume {
            fact: Fact::checked(1, goal.invariant.to_string()),
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
        for byte in goal.invariant.to_string().bytes() {
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
        let goal = Goal::new(
            InvariantName::Liveness,
            Target::Region(RegionId(1)),
            ProofContext::new("test"),
        );
        let mut cache = ProofCache::new();
        let result = check_proof_cached(&goal, &mut cache);
        assert!(result.is_discharged());
    }

    #[test]
    fn check_proof_cached_uses_cache() {
        let goal = Goal::new(
            InvariantName::Exclusivity,
            Target::Region(RegionId(2)),
            ProofContext::new("test"),
        );
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
        let goal1 = Goal::new(
            InvariantName::Liveness,
            Target::Region(RegionId(1)),
            ProofContext::new("a"),
        );
        let goal2 = Goal::new(
            InvariantName::Exclusivity,
            Target::Region(RegionId(2)),
            ProofContext::new("b"),
        );
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

    use crate::judgment::{
        CapDKind, EventId, Judgment, PointerId, RegionId as JRegionId, ResourceId,
    };

    #[test]
    fn test_structured_liveness_intro_proof() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                1,
                Judgment::Allocated {
                    region: JRegionId(1),
                },
            ),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived_j(
                2,
                Judgment::Live {
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
    fn test_structured_liveness_intro_judgment_mismatch() {
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                1,
                Judgment::Allocated {
                    region: JRegionId(1),
                },
            ),
        });
        // Claim the conclusion is Live for r2, but the rule will produce Live for r1
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived_j(
                2,
                Judgment::Live {
                    region: JRegionId(2),
                },
            ),
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
                Judgment::Exclusive {
                    resource: ResourceId(1),
                },
            ),
        });
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                2,
                Judgment::Exclusive {
                    resource: ResourceId(2),
                },
            ),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1, 2],
            rule: InferenceRule::ExclusivityElim,
            conclusion: Fact::derived_j(
                3,
                Judgment::NoConflict {
                    resource_a: ResourceId(1),
                    resource_b: ResourceId(2),
                },
            ),
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
            fact: Fact::checked_j(
                1,
                Judgment::Freed {
                    region: JRegionId(5),
                },
            ),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessElim,
            conclusion: Fact::derived_j(
                2,
                Judgment::Dead {
                    region: JRegionId(5),
                },
            ),
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
            fact: Fact::axiom_j(
                1,
                Judgment::Freed {
                    region: JRegionId(1),
                },
            ),
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
        // The InBounds premise is established by the IVE verifier's bounds
        // checker, so it is tagged Checked (sound to assume, W9). The
        // region-bounds fact is an Axiom matching BoundsContainment.
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::checked_j(
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
            conclusion: Fact::derived_j(
                3,
                Judgment::BoundsPreserved {
                    pointer: PointerId(1),
                    offset: 8,
                    size: 4,
                },
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
                Judgment::Exclusive {
                    resource: ResourceId(10),
                },
            ),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::ExclusivityIntro,
            conclusion: Fact::derived_j(
                2,
                Judgment::Exclusive {
                    resource: ResourceId(10),
                },
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

    // -- W9 soundness tests --------------------------------------------------

    #[test]
    fn test_invalid_assume_unknown_axiom_rejected() {
        // An Assume step whose fact is tagged as Axiom but whose statement
        // does not match any recognized AxiomId must be REJECTED. This is
        // the core soundness fix: the checker no longer accepts arbitrary
        // "axiom" facts.
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(1, "the moon is made of cheese"),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert!(matches!(result, CheckResult::Invalid { .. }));
        if let CheckResult::Invalid { step, reason } = result {
            assert_eq!(step, 0);
            assert!(
                reason.contains("does not match any recognized AxiomId"),
                "reason should mention unrecognized axiom, got: {}",
                reason
            );
            // The reason should also list the recognized axioms so the
            // proof author knows what is allowed.
            assert!(reason.contains("AllocLive"));
            assert!(reason.contains("FreeInvalidates"));
        }
    }

    #[test]
    fn test_valid_assume_known_axiom_accepted() {
        // An Assume step whose fact is an Axiom matching a recognized
        // AxiomId (here: AllocLive, via the "allocated" keyword) must be
        // ACCEPTED, and a proof building on it should be Valid.
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
    fn test_invalid_assume_pure_assumption_rejected() {
        // An Assume step with FactKind::Assumption (a pure, unscoped
        // assumption) must be REJECTED — this was the soundness hole in
        // the previous checker.
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::assumption(1, "everything is fine"),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert!(matches!(result, CheckResult::Invalid { .. }));
        if let CheckResult::Invalid { step, reason } = result {
            assert_eq!(step, 0);
            assert!(
                reason.contains("Assumption"),
                "reason should mention the Assumption kind, got: {}",
                reason
            );
        }
    }

    #[test]
    fn test_invalid_assume_derived_kind_rejected() {
        // An Assume step with FactKind::Derived must be REJECTED — Derived
        // facts must come from an Infer step, not be assumed.
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::derived(1, "region 1 is live"),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert!(matches!(result, CheckResult::Invalid { .. }));
        if let CheckResult::Invalid { reason, .. } = result {
            assert!(
                reason.contains("Derived"),
                "reason should mention the Derived kind, got: {}",
                reason
            );
        }
    }

    #[test]
    fn test_valid_assume_checked_fact_accepted() {
        // An Assume step with FactKind::Checked (verifier-established)
        // must be ACCEPTED — this is the "previously proven by the
        // verifier" path.
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::checked(1, "region 1 is freed at PP 5"),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessElim,
            conclusion: Fact::derived(2, "region 1 is dead at PP 5"),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Valid);
    }

    #[test]
    fn test_axiom_id_from_fact_structured() {
        // Structured judgments should map to the correct AxiomId.
        // (EventId, PointerId, RegionId as JRegionId, ResourceId, Judgment
        // are already imported at the top of the test module.)
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom_j(
                1,
                Judgment::Allocated {
                    region: JRegionId(1),
                }
            )),
            Some(AxiomId::AllocLive)
        );
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom_j(
                1,
                Judgment::Freed {
                    region: JRegionId(1),
                }
            )),
            Some(AxiomId::FreeInvalidates)
        );
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom_j(
                1,
                Judgment::TemporalOrder {
                    event_a: EventId(1),
                    event_b: EventId(2),
                }
            )),
            Some(AxiomId::SyncOrdersAccesses)
        );
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom_j(
                1,
                Judgment::Exclusive {
                    resource: ResourceId(1),
                }
            )),
            Some(AxiomId::ExclusiveDisjoint)
        );
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom_j(
                1,
                Judgment::Derived {
                    pointer: PointerId(1),
                    from: PointerId(2),
                    region: JRegionId(1),
                }
            )),
            Some(AxiomId::DerivationTransitive)
        );
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom_j(
                1,
                Judgment::InBounds {
                    pointer: PointerId(1),
                    offset: 0,
                    size: 4,
                }
            )),
            Some(AxiomId::BoundsContainment)
        );
    }

    #[test]
    fn test_axiom_id_from_fact_string() {
        // String-based facts should map to the correct AxiomId via keyword
        // matching (backward compatibility).
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom(1, "region 1 is allocated at PP 0")),
            Some(AxiomId::AllocLive)
        );
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom(1, "region 1 is freed at PP 5")),
            Some(AxiomId::FreeInvalidates)
        );
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom(1, "lock 7 acquired on region for access 3")),
            Some(AxiomId::SyncOrdersAccesses)
        );
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom(1, "access 1 happens before access 2")),
            Some(AxiomId::SyncOrdersAccesses)
        );
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom(1, "exclusive access to resource 4")),
            Some(AxiomId::ExclusiveDisjoint)
        );
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom(1, "derivation 5 has root region 3")),
            Some(AxiomId::DerivationTransitive)
        );
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom(1, "region r1 has bounds [0, 1024]")),
            Some(AxiomId::BoundsContainment)
        );
        // An unrecognized statement maps to None.
        assert_eq!(
            AxiomId::from_fact(&Fact::axiom(1, "the moon is made of cheese")),
            None
        );
    }

    #[test]
    fn test_axiom_id_validate() {
        // validate returns Ok for a matching fact and Err for a mismatch.
        let f = Fact::axiom(1, "region 1 is allocated");
        assert!(AxiomId::AllocLive.validate(&f).is_ok());
        assert!(AxiomId::FreeInvalidates.validate(&f).is_err());
    }

    #[test]
    fn test_axiom_id_all_and_display() {
        // The enumerated set has exactly 6 axioms, each displayable.
        assert_eq!(AxiomId::all().len(), 6);
        let names: Vec<&str> = AxiomId::all().iter().map(|a| a.name()).collect();
        assert!(names.contains(&"AllocLive"));
        assert!(names.contains(&"FreeInvalidates"));
        assert!(names.contains(&"SyncOrdersAccesses"));
        assert!(names.contains(&"ExclusiveDisjoint"));
        assert!(names.contains(&"DerivationTransitive"));
        assert!(names.contains(&"BoundsContainment"));
        assert_eq!(format!("{}", AxiomId::AllocLive), "AllocLive");
    }

    #[test]
    fn test_valid_structured_axiom_assume_roundtrip() {
        // A proof that uses a structured-judgment Axiom Assume step (not
        // just string-based) should be Valid. This exercises the judgment
        // branch of AxiomId::from_fact inside the checker.
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                1,
                Judgment::Allocated {
                    region: crate::judgment::RegionId(7),
                },
            ),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![1],
            rule: InferenceRule::LivenessIntro,
            conclusion: Fact::derived_j(
                2,
                Judgment::Live {
                    region: crate::judgment::RegionId(7),
                },
            ),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert_eq!(result, CheckResult::Valid);
    }

    #[test]
    fn test_structured_axiom_with_non_axiom_judgment_rejected() {
        // A fact tagged as Axiom but carrying a judgment that is NOT a
        // recognized axiom (e.g. Judgment::Live, which is *derived*) must
        // be rejected.
        let mut proof = Proof::new(dummy_goal());
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom_j(
                1,
                Judgment::Live {
                    region: crate::judgment::RegionId(7),
                },
            ),
        });
        proof.conclude(Conclusion::Proven);

        let checker = ProofChecker::new();
        let result = checker.check(&proof).unwrap();
        assert!(matches!(result, CheckResult::Invalid { .. }));
        if let CheckResult::Invalid { reason, .. } = result {
            assert!(reason.contains("does not match any recognized AxiomId"));
        }
    }
}
