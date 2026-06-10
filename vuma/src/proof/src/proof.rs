//! # Proof Objects
//!
//! Core data structures for representing formal proofs about VUMA memory safety
//! invariants. A proof demonstrates that a particular goal (safety property)
//! holds by chaining inference steps from axioms and assumptions to a conclusion.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Identifier types
// ---------------------------------------------------------------------------

/// Unique identifier for a region in the program's memory model.
pub type RegionId = u64;

/// Unique identifier for an access (read/write) operation.
pub type AccessId = u64;

/// Unique identifier for a derivation chain.
pub type DerivationId = u64;

/// Unique identifier for a fact within a proof.
pub type FactId = u64;

/// Name of an invariant (e.g. "liveness", "exclusivity", "bounds_safety").
pub type InvariantName = String;

/// A program point — line/column or offset — used for locating violations.
pub type ProgramPoint = u64;

// ---------------------------------------------------------------------------
// ProofContext
// ---------------------------------------------------------------------------

/// Contextual information that scopes a proof goal, such as the function name,
/// module path, or surrounding assumptions that are in scope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProofContext {
    /// Human-readable scope name (e.g. "main::process_buffer").
    pub scope: String,
    /// Assumptions inherited from the enclosing scope.
    pub assumptions: Vec<String>,
}

impl ProofContext {
    /// Create a new proof context with the given scope name and no assumptions.
    pub fn new(scope: impl Into<String>) -> Self {
        Self {
            scope: scope.into(),
            assumptions: Vec::new(),
        }
    }

    /// Add an assumption to this context.
    pub fn with_assumption(mut self, assumption: impl Into<String>) -> Self {
        self.assumptions.push(assumption.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Target
// ---------------------------------------------------------------------------

/// The target of a proof goal — what the proof is about.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Target {
    /// Prove an invariant holds for a specific memory region.
    Region(RegionId),
    /// Prove an invariant holds for a specific access operation.
    Access(AccessId),
    /// Prove an invariant holds for a specific derivation chain.
    Derivation(DerivationId),
    /// Prove an invariant holds for the entire program (global invariant).
    FullProgram,
}

// ---------------------------------------------------------------------------
// Goal
// ---------------------------------------------------------------------------

/// A proof goal: the statement that we want to prove.
///
/// A goal pairs an invariant name with a target and a context. For example,
/// "prove `liveness` for `Region(42)` in the context of `main::alloc_buffer`".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Goal {
    /// The invariant that must hold.
    pub invariant: InvariantName,
    /// The target (region, access, derivation, or full program).
    pub target: Target,
    /// The proof context carrying scope and inherited assumptions.
    pub context: ProofContext,
}

impl Goal {
    /// Create a new proof goal.
    pub fn new(
        invariant: impl Into<String>,
        target: Target,
        context: ProofContext,
    ) -> Self {
        Self {
            invariant: invariant.into(),
            target,
            context,
        }
    }
}

// ---------------------------------------------------------------------------
// FactKind / Fact
// ---------------------------------------------------------------------------

/// How a fact was established.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FactKind {
    /// An axiom — accepted without proof.
    Axiom,
    /// Derived from other facts via an inference rule.
    Derived,
    /// Assumed for the purpose of conditional or inductive proofs.
    Assumption,
    /// Mechanically checked by the verifier.
    Checked,
}

impl std::fmt::Display for FactKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FactKind::Axiom => write!(f, "axiom"),
            FactKind::Derived => write!(f, "derived"),
            FactKind::Assumption => write!(f, "assumption"),
            FactKind::Checked => write!(f, "checked"),
        }
    }
}

/// A fact within a proof — a logical statement together with its kind and id.
///
/// A fact optionally carries a structured [`Judgment`] that enables precise
/// structural matching in inference rules. When `judgment` is `Some`, rules
/// match on the typed judgment variant instead of performing fragile string
/// comparison on `statement`. When `judgment` is `None`, the rule falls back
/// to string-based matching for backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fact {
    /// Unique identifier for this fact within the proof.
    pub id: FactId,
    /// The logical statement, expressed as a human-readable string.
    pub statement: String,
    /// How this fact was established.
    pub kind: FactKind,
    /// Optional structured judgment for precise structural matching.
    /// When present, inference rules match on the judgment variant
    /// rather than performing string pattern matching.
    #[serde(default)]
    pub judgment: Option<super::judgment::Judgment>,
}

impl Fact {
    /// Create a new fact without a structured judgment.
    pub fn new(id: FactId, statement: impl Into<String>, kind: FactKind) -> Self {
        Self {
            id,
            statement: statement.into(),
            kind,
            judgment: None,
        }
    }

    /// Create a new fact with a structured judgment.
    ///
    /// The `statement` field is populated from the judgment's `to_statement()`
    /// method, ensuring consistency between the string and structured
    /// representations.
    pub fn with_judgment(id: FactId, judgment: super::judgment::Judgment, kind: FactKind) -> Self {
        let stmt = judgment.to_statement();
        Self {
            id,
            statement: stmt,
            kind,
            judgment: Some(judgment),
        }
    }

    /// Convenience constructor for an axiom without a structured judgment.
    pub fn axiom(id: FactId, statement: impl Into<String>) -> Self {
        Self::new(id, statement, FactKind::Axiom)
    }

    /// Convenience constructor for an axiom with a structured judgment.
    pub fn axiom_j(id: FactId, judgment: super::judgment::Judgment) -> Self {
        Self::with_judgment(id, judgment, FactKind::Axiom)
    }

    /// Convenience constructor for a derived fact without a structured judgment.
    pub fn derived(id: FactId, statement: impl Into<String>) -> Self {
        Self::new(id, statement, FactKind::Derived)
    }

    /// Convenience constructor for a derived fact with a structured judgment.
    pub fn derived_j(id: FactId, judgment: super::judgment::Judgment) -> Self {
        Self::with_judgment(id, judgment, FactKind::Derived)
    }

    /// Convenience constructor for an assumption without a structured judgment.
    pub fn assumption(id: FactId, statement: impl Into<String>) -> Self {
        Self::new(id, statement, FactKind::Assumption)
    }

    /// Convenience constructor for an assumption with a structured judgment.
    pub fn assumption_j(id: FactId, judgment: super::judgment::Judgment) -> Self {
        Self::with_judgment(id, judgment, FactKind::Assumption)
    }

    /// Convenience constructor for a checked fact without a structured judgment.
    pub fn checked(id: FactId, statement: impl Into<String>) -> Self {
        Self::new(id, statement, FactKind::Checked)
    }

    /// Convenience constructor for a checked fact with a structured judgment.
    pub fn checked_j(id: FactId, judgment: super::judgment::Judgment) -> Self {
        Self::with_judgment(id, judgment, FactKind::Checked)
    }
}

// ---------------------------------------------------------------------------
// ProofStep
// ---------------------------------------------------------------------------

/// A single step in a proof. Each step represents a logical inference from
/// previously established facts to a new fact, or a structural proof technique
/// such as case splitting or induction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ProofStep {
    /// Assume a fact without proof (for conditional reasoning).
    Assume {
        /// The fact being assumed.
        fact: Fact,
    },

    /// Infer a new fact from premises using an inference rule.
    Infer {
        /// The fact ids of the premises.
        from: Vec<FactId>,
        /// The inference rule being applied.
        rule: super::rules::InferenceRule,
        /// The conclusion drawn from this inference.
        conclusion: Fact,
    },

    /// Split the proof into multiple cases; all must hold.
    CaseSplit {
        /// Sub-proofs, one per case. Every case must conclude `Proven`.
        cases: Vec<Proof>,
    },

    /// Proof by induction: a base case and an inductive step.
    Induction {
        /// The base case proof.
        base: Box<Proof>,
        /// The inductive step proof.
        step: Box<Proof>,
    },

    /// Derive a contradiction from an assumption and its negation,
    /// thereby refuting the assumption.
    Contradiction {
        /// The id of the original assumption.
        assumption: FactId,
        /// The id of the fact that negates the assumption.
        negation: FactId,
    },

    /// Appeal to a definition — the conclusion holds by definitional expansion.
    ByDefinition {
        /// The name or text of the definition being invoked.
        definition: String,
    },
}

// ---------------------------------------------------------------------------
// Conclusion
// ---------------------------------------------------------------------------

/// The outcome of a proof attempt.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Conclusion {
    /// The goal has been proven.
    Proven,
    /// The goal has been refuted (a counterexample exists).
    Refuted,
    /// The proof is incomplete — neither proven nor refuted.
    Inconclusive,
}

impl std::fmt::Display for Conclusion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Conclusion::Proven => write!(f, "proven"),
            Conclusion::Refuted => write!(f, "refuted"),
            Conclusion::Inconclusive => write!(f, "inconclusive"),
        }
    }
}

// ---------------------------------------------------------------------------
// Proof
// ---------------------------------------------------------------------------

/// A structured formal proof.
///
/// A proof starts from a goal and proceeds through a sequence of steps to
/// reach a conclusion. Each step is either a direct inference or a structural
/// technique (induction, case split, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Proof {
    /// The goal this proof is trying to establish.
    pub goal: Goal,
    /// The sequence of proof steps.
    pub steps: Vec<ProofStep>,
    /// The final conclusion of the proof.
    pub conclusion: Conclusion,
}

impl Proof {
    /// Create a new (initially empty) proof for the given goal.
    pub fn new(goal: Goal) -> Self {
        Self {
            goal,
            steps: Vec::new(),
            conclusion: Conclusion::Inconclusive,
        }
    }

    /// Append a proof step.
    pub fn add_step(&mut self, step: ProofStep) {
        self.steps.push(step);
    }

    /// Set the conclusion of this proof.
    pub fn conclude(&mut self, conclusion: Conclusion) {
        self.conclusion = conclusion;
    }

    /// Collect all facts introduced by this proof (including nested sub-proofs).
    pub fn all_facts(&self) -> Vec<&Fact> {
        let mut facts = Vec::new();
        self.collect_facts_into(&mut facts);
        facts
    }

    /// Recursive helper for `all_facts`.
    fn collect_facts_into<'a>(&'a self, out: &mut Vec<&'a Fact>) {
        for step in &self.steps {
            match step {
                ProofStep::Assume { fact } => {
                    out.push(fact);
                }
                ProofStep::Infer { conclusion, .. } => {
                    out.push(conclusion);
                }
                ProofStep::CaseSplit { cases } => {
                    for case in cases {
                        case.collect_facts_into(out);
                    }
                }
                ProofStep::Induction { base, step: ind_step } => {
                    base.collect_facts_into(out);
                    ind_step.collect_facts_into(out);
                }
                ProofStep::Contradiction { .. } => {
                    // No new fact is introduced; contradiction discharges an assumption.
                }
                ProofStep::ByDefinition { .. } => {
                    // No explicit fact introduced; definitional equality is implicit.
                }
            }
        }
    }

    /// Look up a fact by its id within this proof.
    pub fn find_fact(&self, id: FactId) -> Option<&Fact> {
        self.all_facts().into_iter().find(|f| f.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_goal_construction() {
        let goal = Goal::new(
            "liveness",
            Target::Region(42),
            ProofContext::new("main::alloc"),
        );
        assert_eq!(goal.invariant, "liveness");
        assert_eq!(goal.target, Target::Region(42));
    }

    #[test]
    fn test_fact_convenience_constructors() {
        let axiom = Fact::axiom(1, "region 42 is allocated");
        assert_eq!(axiom.kind, FactKind::Axiom);

        let derived = Fact::derived(2, "region 42 is live");
        assert_eq!(derived.kind, FactKind::Derived);
    }

    #[test]
    fn test_proof_step_assume() {
        let step = ProofStep::Assume {
            fact: Fact::assumption(0, "P"),
        };
        if let ProofStep::Assume { fact } = step {
            assert_eq!(fact.id, 0);
        } else {
            panic!("expected Assume step");
        }
    }

    #[test]
    fn test_conclusion_display() {
        assert_eq!(format!("{}", Conclusion::Proven), "proven");
        assert_eq!(format!("{}", Conclusion::Refuted), "refuted");
        assert_eq!(format!("{}", Conclusion::Inconclusive), "inconclusive");
    }

    #[test]
    fn test_proof_collect_facts() {
        let mut proof = Proof::new(Goal::new(
            "exclusivity",
            Target::FullProgram,
            ProofContext::new("top"),
        ));
        proof.add_step(ProofStep::Assume {
            fact: Fact::assumption(1, "region A is live"),
        });
        proof.add_step(ProofStep::Assume {
            fact: Fact::assumption(2, "region B is live"),
        });

        let facts = proof.all_facts();
        assert_eq!(facts.len(), 2);
    }
}
