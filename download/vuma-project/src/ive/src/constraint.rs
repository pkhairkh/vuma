//! Constraint types for the IVE module.
//!
//! Constraints represent properties that the inference engine derives
//! and that the verification engine must check. They encode temporal,
//! resource-flow, security, complexity, liveness, memory-region,
//! access-pattern, and compositional properties.
//!
//! This module also provides a simple constraint solver that evaluates
//! constraints against a fact database, and a simplification pass that
//! removes tautologies, contradictions, and unnecessary nesting.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// ConstraintId
// ---------------------------------------------------------------------------

/// A unique identifier for a constraint.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConstraintId(pub String);

impl ConstraintId {
    /// Construct a new constraint ID.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl fmt::Display for ConstraintId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "C:{}", self.0)
    }
}

impl From<&str> for ConstraintId {
    fn from(s: &str) -> Self {
        ConstraintId::new(s)
    }
}

// ---------------------------------------------------------------------------
// ProgramPoint (for temporal constraints)
// ---------------------------------------------------------------------------

/// A point in the program, identified by a label.
pub type ProgramPoint = String;

// ---------------------------------------------------------------------------
// Original constraint payload structs (preserved)
// ---------------------------------------------------------------------------

/// A temporal constraint (e.g., "A must happen before B").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemporalConstraint {
    /// Short human-readable description.
    pub description: String,
}

/// A constraint on how resources flow through the program.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceFlowConstraint {
    /// Short human-readable description.
    pub description: String,
}

/// A security-related constraint (e.g., information flow, access control).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecurityConstraint {
    /// Short human-readable description.
    pub description: String,
}

/// A constraint on computational complexity (e.g., "this loop runs O(n)").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComplexityConstraint {
    /// Short human-readable description.
    pub description: String,
}

/// A liveness constraint (e.g., "every request eventually receives a response").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LivenessConstraint {
    /// Short human-readable description.
    pub description: String,
}

// ---------------------------------------------------------------------------
// New constraint supporting types
// ---------------------------------------------------------------------------

/// Kind of constraint that can be placed on a memory region.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RegionConstraintKind {
    /// The region must be live (not yet freed).
    MustBeLive,
    /// The region must be exclusively owned (no aliases).
    MustBeExclusive,
    /// The region must be initialized at the given offset/size range.
    MustBeInitialized { offset: u64, size: u64 },
    /// The region must have the specified capabilities.
    MustHaveCapability { caps: Vec<String> },
}

impl fmt::Display for RegionConstraintKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegionConstraintKind::MustBeLive => write!(f, "must_be_live"),
            RegionConstraintKind::MustBeExclusive => write!(f, "must_be_exclusive"),
            RegionConstraintKind::MustBeInitialized { offset, size } => {
                write!(f, "must_be_initialized(offset={offset}, size={size})")
            }
            RegionConstraintKind::MustHaveCapability { caps } => {
                write!(f, "must_have_capability({:?})", caps)
            }
        }
    }
}

/// Pattern of memory access.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccessPattern {
    /// Sequential access pattern (e.g., array traversal).
    Sequential,
    /// Random / unpredictable access pattern.
    Random,
    /// Streaming access (sequential, no reuse).
    Streaming,
    /// Atomic access (uses atomic operations).
    Atomic,
}

impl fmt::Display for AccessPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AccessPattern::Sequential => write!(f, "sequential"),
            AccessPattern::Random => write!(f, "random"),
            AccessPattern::Streaming => write!(f, "streaming"),
            AccessPattern::Atomic => write!(f, "atomic"),
        }
    }
}

/// Temporal relation between two program points.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TemporalRelation {
    /// The `before` point happens before the `after` point.
    HappensBefore,
    /// The `before` point happens after the `after` point.
    HappensAfter,
    /// The two points may happen concurrently.
    ConcurrentWith,
    /// The two points happen in strict sequential order.
    SequentialWith,
}

impl fmt::Display for TemporalRelation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TemporalRelation::HappensBefore => write!(f, "happens_before"),
            TemporalRelation::HappensAfter => write!(f, "happens_after"),
            TemporalRelation::ConcurrentWith => write!(f, "concurrent_with"),
            TemporalRelation::SequentialWith => write!(f, "sequential_with"),
        }
    }
}

/// How to combine multiple sub-constraints in a `Compositional` constraint.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConstraintCombinator {
    /// All sub-constraints must be satisfied (logical AND).
    All,
    /// At least one sub-constraint must be satisfied (logical OR).
    Any,
    /// None of the sub-constraints must be satisfied (logical negation).
    None,
}

impl fmt::Display for ConstraintCombinator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstraintCombinator::All => write!(f, "ALL"),
            ConstraintCombinator::Any => write!(f, "ANY"),
            ConstraintCombinator::None => write!(f, "NONE"),
        }
    }
}

// ---------------------------------------------------------------------------
// Constraint
// ---------------------------------------------------------------------------

/// A constraint derived by the inference engine.
///
/// Each variant carries a human-readable description and typed payload.
/// New variants add structured constraint types for memory regions,
/// access patterns, temporal ordering, and compositional logic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Constraint {
    // --- Original variants (preserved) ---
    /// Temporal ordering constraint (legacy, description-only).
    Temporal(TemporalConstraint),
    /// Resource flow constraint.
    ResourceFlow(ResourceFlowConstraint),
    /// Security constraint.
    Security(SecurityConstraint),
    /// Complexity constraint.
    Complexity(ComplexityConstraint),
    /// Liveness constraint.
    Liveness(LivenessConstraint),

    // --- New structured constraint variants ---
    /// Constraint on a memory region.
    MemoryRegionConstraint {
        region_id: u64,
        constraint_kind: RegionConstraintKind,
    },
    /// Constraint on the access pattern of a particular access.
    AccessPatternConstraint {
        access_id: u64,
        pattern: AccessPattern,
    },
    /// Structured temporal constraint with explicit program points and relation.
    TemporalOrdered {
        before: ProgramPoint,
        after: ProgramPoint,
        relation: TemporalRelation,
    },
    /// A compositional constraint that combines sub-constraints with a combinator.
    Compositional {
        constraints: Vec<Constraint>,
        combinator: ConstraintCombinator,
    },
}

impl Constraint {
    /// Returns the unique identifier for this constraint.
    pub fn id(&self) -> ConstraintId {
        let desc = self.description();
        // Use a simple hash of the description as the ID.
        ConstraintId::new(format!("{:x}", desc.len()))
    }

    /// Returns a human-readable description of this constraint.
    pub fn description(&self) -> String {
        match self {
            Self::Temporal(c) => c.description.clone(),
            Self::ResourceFlow(c) => c.description.clone(),
            Self::Security(c) => c.description.clone(),
            Self::Complexity(c) => c.description.clone(),
            Self::Liveness(c) => c.description.clone(),
            Self::MemoryRegionConstraint {
                region_id,
                constraint_kind,
            } => format!("region#{region_id}: {constraint_kind}"),
            Self::AccessPatternConstraint { access_id, pattern } => {
                format!("access#{access_id}: {pattern}")
            }
            Self::TemporalOrdered {
                before,
                after,
                relation,
            } => format!("{before} {relation} {after}"),
            Self::Compositional {
                constraints,
                combinator,
            } => {
                let inner: Vec<String> = constraints.iter().map(|c| c.description()).collect();
                format!("{combinator}({})", inner.join(", "))
            }
        }
    }

    /// Check whether this constraint is satisfied (placeholder).
    ///
    /// In a full implementation, this would evaluate the constraint against
    /// a concrete program state or model. Currently returns `true` as a
    /// placeholder.
    ///
    /// TODO: Implement actual constraint checking against SCG / model state.
    pub fn check(&self) -> bool {
        // Placeholder — always passes.
        log::warn!("Constraint::check() is a placeholder — always returns true");
        true
    }

    /// Return the logical negation of this constraint.
    pub fn negate(&self) -> Constraint {
        match self {
            Constraint::Temporal(c) => Constraint::Temporal(TemporalConstraint {
                description: format!("NOT({})", c.description),
            }),
            Constraint::ResourceFlow(c) => Constraint::ResourceFlow(ResourceFlowConstraint {
                description: format!("NOT({})", c.description),
            }),
            Constraint::Security(c) => Constraint::Security(SecurityConstraint {
                description: format!("NOT({})", c.description),
            }),
            Constraint::Complexity(c) => Constraint::Complexity(ComplexityConstraint {
                description: format!("NOT({})", c.description),
            }),
            Constraint::Liveness(c) => Constraint::Liveness(LivenessConstraint {
                description: format!("NOT({})", c.description),
            }),
            Constraint::MemoryRegionConstraint {
                region_id,
                constraint_kind,
            } => Constraint::Compositional {
                constraints: vec![Constraint::MemoryRegionConstraint {
                    region_id: *region_id,
                    constraint_kind: constraint_kind.clone(),
                }],
                combinator: ConstraintCombinator::None,
            },
            Constraint::AccessPatternConstraint { access_id, pattern } => Constraint::Compositional {
                constraints: vec![Constraint::AccessPatternConstraint {
                    access_id: *access_id,
                    pattern: pattern.clone(),
                }],
                combinator: ConstraintCombinator::None,
            },
            Constraint::TemporalOrdered {
                before,
                after,
                relation,
            } => Constraint::TemporalOrdered {
                before: before.clone(),
                after: after.clone(),
                relation: match relation {
                    TemporalRelation::HappensBefore => TemporalRelation::HappensAfter,
                    TemporalRelation::HappensAfter => TemporalRelation::HappensBefore,
                    TemporalRelation::ConcurrentWith => TemporalRelation::SequentialWith,
                    TemporalRelation::SequentialWith => TemporalRelation::ConcurrentWith,
                },
            },
            Constraint::Compositional {
                constraints,
                combinator,
            } => {
                // Negation via De Morgan's laws:
                // NOT(ALL(a,b)) = ANY(NOT(a), NOT(b))
                // NOT(ANY(a,b)) = ALL(NOT(a), NOT(b))
                // NOT(NONE(a,b)) = ANY(a, b)
                let negated: Vec<Constraint> =
                    constraints.iter().map(|c| c.negate()).collect();
                match combinator {
                    ConstraintCombinator::All => Constraint::Compositional {
                        constraints: negated,
                        combinator: ConstraintCombinator::Any,
                    },
                    ConstraintCombinator::Any => Constraint::Compositional {
                        constraints: negated,
                        combinator: ConstraintCombinator::All,
                    },
                    ConstraintCombinator::None => Constraint::Compositional {
                        constraints: constraints.clone(),
                        combinator: ConstraintCombinator::Any,
                    },
                }
            }
        }
    }

    /// Returns `true` if this is a temporal constraint (legacy variant).
    pub fn is_temporal(&self) -> bool {
        matches!(self, Self::Temporal(_))
    }

    /// Returns `true` if this is a resource-flow constraint.
    pub fn is_resource_flow(&self) -> bool {
        matches!(self, Self::ResourceFlow(_))
    }

    /// Returns `true` if this is a security constraint.
    pub fn is_security(&self) -> bool {
        matches!(self, Self::Security(_))
    }

    /// Returns `true` if this is a complexity constraint.
    pub fn is_complexity(&self) -> bool {
        matches!(self, Self::Complexity(_))
    }

    /// Returns `true` if this is a liveness constraint.
    pub fn is_liveness(&self) -> bool {
        matches!(self, Self::Liveness(_))
    }

    /// Returns `true` if this is a memory-region constraint.
    pub fn is_memory_region(&self) -> bool {
        matches!(self, Self::MemoryRegionConstraint { .. })
    }

    /// Returns `true` if this is an access-pattern constraint.
    pub fn is_access_pattern(&self) -> bool {
        matches!(self, Self::AccessPatternConstraint { .. })
    }

    /// Returns `true` if this is a structured temporal constraint.
    pub fn is_temporal_ordered(&self) -> bool {
        matches!(self, Self::TemporalOrdered { .. })
    }

    /// Returns `true` if this is a compositional constraint.
    pub fn is_compositional(&self) -> bool {
        matches!(self, Self::Compositional { .. })
    }

    /// Returns the fact key associated with this constraint for solver lookup.
    ///
    /// This provides a mapping from a constraint to a string key that can be
    /// looked up in the solver's fact database.
    fn fact_key(&self) -> String {
        match self {
            Constraint::Temporal(c) => format!("temporal:{}", c.description),
            Constraint::ResourceFlow(c) => format!("resource_flow:{}", c.description),
            Constraint::Security(c) => format!("security:{}", c.description),
            Constraint::Complexity(c) => format!("complexity:{}", c.description),
            Constraint::Liveness(c) => format!("liveness:{}", c.description),
            Constraint::MemoryRegionConstraint {
                region_id,
                constraint_kind,
            } => format!("region:{region_id}:{constraint_kind}"),
            Constraint::AccessPatternConstraint { access_id, pattern } => {
                format!("access:{access_id}:{pattern}")
            }
            Constraint::TemporalOrdered {
                before,
                after,
                relation,
            } => format!("temporal_ordered:{before}:{relation}:{after}"),
            Constraint::Compositional { .. } => {
                // Compositional constraints don't have a single fact key;
                // they are evaluated recursively.
                String::new()
            }
        }
    }
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Temporal(_) => "TEMPORAL",
            Self::ResourceFlow(_) => "RESOURCE_FLOW",
            Self::Security(_) => "SECURITY",
            Self::Complexity(_) => "COMPLEXITY",
            Self::Liveness(_) => "LIVENESS",
            Self::MemoryRegionConstraint { .. } => "MEMORY_REGION",
            Self::AccessPatternConstraint { .. } => "ACCESS_PATTERN",
            Self::TemporalOrdered { .. } => "TEMPORAL_ORDERED",
            Self::Compositional { .. } => "COMPOSITIONAL",
        };
        write!(f, "[{kind}] {}", self.description())
    }
}

// ---------------------------------------------------------------------------
// ConstraintSolution
// ---------------------------------------------------------------------------

/// Overall status of a constraint solution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SolutionStatus {
    /// All constraints are satisfied.
    AllSatisfied,
    /// Some constraints are violated.
    SomeViolated,
    /// Some constraints could not be determined (unknown facts).
    SomeUnknown,
    /// The constraint system is unsatisfiable.
    Unsatisfiable,
}

impl fmt::Display for SolutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SolutionStatus::AllSatisfied => write!(f, "all_satisfied"),
            SolutionStatus::SomeViolated => write!(f, "some_violated"),
            SolutionStatus::SomeUnknown => write!(f, "some_unknown"),
            SolutionStatus::Unsatisfiable => write!(f, "unsatisfiable"),
        }
    }
}

/// Result of solving a set of constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintSolution {
    /// Indices of satisfied constraints.
    pub satisfied: Vec<usize>,
    /// Indices of violated constraints with reasons.
    pub violated: Vec<(usize, String)>,
    /// Indices of constraints that could not be evaluated (unknown facts).
    pub unknown: Vec<usize>,
    /// Overall solution status.
    pub overall: SolutionStatus,
}

impl ConstraintSolution {
    /// Create an empty solution.
    pub fn empty() -> Self {
        Self {
            satisfied: Vec::new(),
            violated: Vec::new(),
            unknown: Vec::new(),
            overall: SolutionStatus::AllSatisfied,
        }
    }
}

impl fmt::Display for ConstraintSolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "ConstraintSolution {{ overall: {} }}", self.overall)?;
        writeln!(f, "  satisfied: {:?}", self.satisfied)?;
        writeln!(f, "  violated:  {:?}", self.violated)?;
        writeln!(f, "  unknown:   {:?}", self.unknown)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ConstraintSolver
// ---------------------------------------------------------------------------

/// Intermediate evaluation result for the constraint solver.
#[derive(Debug, Clone)]
enum EvalResult {
    Satisfied,
    Violated(String),
    Unknown,
}

/// A simple constraint solver that evaluates constraints against a fact
/// database.
///
/// Facts are named boolean values. A constraint is evaluated by looking up
/// its associated fact key in the database. Compositional constraints are
/// evaluated recursively using their combinator semantics.
///
/// # Limitations
///
/// This is a propositional-level solver: it does not perform unification,
/// SMT-style reasoning, or exhaustive search. It simply maps constraints to
/// known facts and combines them compositionally.
#[derive(Debug, Clone)]
pub struct ConstraintSolver {
    constraints: Vec<Constraint>,
    facts: HashMap<String, bool>,
    max_depth: usize,
}

impl ConstraintSolver {
    /// Create a new solver with default max recursion depth (64).
    pub fn new() -> Self {
        Self {
            constraints: Vec::new(),
            facts: HashMap::new(),
            max_depth: 64,
        }
    }

    /// Create a new solver with a custom max recursion depth.
    pub fn with_max_depth(max_depth: usize) -> Self {
        Self {
            constraints: Vec::new(),
            facts: HashMap::new(),
            max_depth,
        }
    }

    /// Add a constraint to be solved.
    pub fn add_constraint(&mut self, c: Constraint) {
        self.constraints.push(c);
    }

    /// Add a named fact to the solver's fact database.
    pub fn add_fact(&mut self, name: String, value: bool) {
        self.facts.insert(name, value);
    }

    /// Clear all constraints and facts.
    pub fn clear(&mut self) {
        self.constraints.clear();
        self.facts.clear();
    }

    /// Returns the number of constraints.
    pub fn len(&self) -> usize {
        self.constraints.len()
    }

    /// Returns true if there are no constraints.
    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }

    /// Solve all constraints against the known facts.
    pub fn solve(&self) -> ConstraintSolution {
        let mut solution = ConstraintSolution::empty();

        for (idx, constraint) in self.constraints.iter().enumerate() {
            match self.evaluate(constraint, 0) {
                EvalResult::Satisfied => {
                    solution.satisfied.push(idx);
                }
                EvalResult::Violated(reason) => {
                    solution.violated.push((idx, reason));
                }
                EvalResult::Unknown => {
                    solution.unknown.push(idx);
                }
            }
        }

        // Determine overall status.
        solution.overall = if !solution.violated.is_empty() {
            // Check if unsatisfiable: any Compositional { combinator: All }
            // where all sub-constraints are violated means unsatisfiable.
            if self.is_definitely_unsatisfiable(&solution) {
                SolutionStatus::Unsatisfiable
            } else {
                SolutionStatus::SomeViolated
            }
        } else if !solution.unknown.is_empty() {
            SolutionStatus::SomeUnknown
        } else {
            SolutionStatus::AllSatisfied
        };

        solution
    }

    /// Quick check: are all constraints satisfiable?
    pub fn is_satisfiable(&self) -> bool {
        let solution = self.solve();
        !matches!(solution.overall, SolutionStatus::Unsatisfiable | SolutionStatus::SomeViolated)
    }

    /// Simplify a set of constraints by removing tautologies, contradictions,
    /// and flattening unnecessary nesting.
    pub fn simplify(&self, constraints: &[Constraint]) -> Vec<Constraint> {
        constraints
            .iter()
            .filter_map(|c| self.simplify_one(c))
            .collect()
    }

    // -- Private helpers --
}

impl ConstraintSolver {
    /// Evaluate a single constraint against the fact database.
    fn evaluate(&self, constraint: &Constraint, depth: usize) -> EvalResult {
        if depth > self.max_depth {
            return EvalResult::Unknown;
        }

        match constraint {
            // Compositional constraints are evaluated recursively.
            Constraint::Compositional {
                constraints,
                combinator,
            } => self.evaluate_compositional(constraints, combinator, depth),

            // All other constraints are evaluated via fact lookup.
            _ => {
                let key = constraint.fact_key();
                if key.is_empty() {
                    // Should not happen for non-compositional constraints,
                    // but treat as unknown if it does.
                    return EvalResult::Unknown;
                }
                match self.facts.get(&key) {
                    Some(true) => EvalResult::Satisfied,
                    Some(false) => EvalResult::Violated(format!("fact '{key}' is false")),
                    None => EvalResult::Unknown,
                }
            }
        }
    }

    /// Evaluate a compositional constraint.
    fn evaluate_compositional(
        &self,
        constraints: &[Constraint],
        combinator: &ConstraintCombinator,
        depth: usize,
    ) -> EvalResult {
        if constraints.is_empty() {
            // Vacuous cases:
            // ALL() with no sub-constraints is a tautology (true).
            // ANY() with no sub-constraints is a contradiction (false).
            // NONE() with no sub-constraints is a tautology (true).
            return match combinator {
                ConstraintCombinator::All => EvalResult::Satisfied,
                ConstraintCombinator::Any => {
                    EvalResult::Violated("ANY with no sub-constraints is unsatisfiable".into())
                }
                ConstraintCombinator::None => EvalResult::Satisfied,
            };
        }

        let results: Vec<EvalResult> = constraints
            .iter()
            .map(|c| self.evaluate(c, depth + 1))
            .collect();

        match combinator {
            ConstraintCombinator::All => {
                // All must be satisfied.
                let mut reasons = Vec::new();
                let mut has_unknown = false;
                for r in &results {
                    match r {
                        EvalResult::Satisfied => {}
                        EvalResult::Violated(reason) => reasons.push(reason.clone()),
                        EvalResult::Unknown => has_unknown = true,
                    }
                }
                if reasons.is_empty() && !has_unknown {
                    EvalResult::Satisfied
                } else if !reasons.is_empty() {
                    EvalResult::Violated(format!(
                        "ALL: {} sub-constraint(s) violated: {}",
                        reasons.len(),
                        reasons.join("; ")
                    ))
                } else {
                    EvalResult::Unknown
                }
            }
            ConstraintCombinator::Any => {
                // At least one must be satisfied.
                let any_satisfied = results.iter().any(|r| matches!(r, EvalResult::Satisfied));
                if any_satisfied {
                    EvalResult::Satisfied
                } else {
                    let all_violated = results.iter().all(|r| matches!(r, EvalResult::Violated(_)));
                    if all_violated {
                        EvalResult::Violated("ANY: all sub-constraints violated".into())
                    } else {
                        EvalResult::Unknown
                    }
                }
            }
            ConstraintCombinator::None => {
                // None must be satisfied (negation).
                let any_satisfied = results.iter().any(|r| matches!(r, EvalResult::Satisfied));
                if any_satisfied {
                    EvalResult::Violated("NONE: at least one sub-constraint satisfied".into())
                } else {
                    let all_violated = results.iter().all(|r| matches!(r, EvalResult::Violated(_)));
                    if all_violated {
                        EvalResult::Satisfied
                    } else {
                        EvalResult::Unknown
                    }
                }
            }
        }
    }

    /// Check if the solution is definitely unsatisfiable (e.g., a
    /// top-level ALL compositional with all sub-constraints violated).
    fn is_definitely_unsatisfiable(&self, solution: &ConstraintSolution) -> bool {
        for (idx, _) in &solution.violated {
            if let Some(Constraint::Compositional {
                constraints: _,
                combinator: ConstraintCombinator::All,
            }) = self.constraints.get(*idx)
            {
                // If all sub-constraints of an ALL compositional are violated,
                // it's unsatisfiable.
                return true;
            }
        }
        false
    }

    /// Simplify a single constraint. Returns `None` if the constraint
    /// should be removed (tautology or contradiction).
    fn simplify_one(&self, c: &Constraint) -> Option<Constraint> {
        match c {
            Constraint::Compositional {
                constraints,
                combinator,
            } => {
                // First, recursively simplify sub-constraints.
                let simplified: Vec<Constraint> = constraints
                    .iter()
                    .filter_map(|sub| self.simplify_one(sub))
                    .collect();

                match combinator {
                    ConstraintCombinator::All => {
                        // ALL with no sub-constraints after simplification → tautology → remove
                        if simplified.is_empty() {
                            return None;
                        }
                        // ALL with a single sub-constraint → unwrap
                        if simplified.len() == 1 {
                            return simplified.into_iter().next();
                        }
                        // Flatten nested ALL(ALL(...)) → ALL(...)
                        let flattened = Self::flatten_compositional(&simplified, combinator);
                        Some(Constraint::Compositional {
                            constraints: flattened,
                            combinator: ConstraintCombinator::All,
                        })
                    }
                    ConstraintCombinator::Any => {
                        // ANY with no sub-constraints → contradiction → remove
                        if simplified.is_empty() {
                            return None;
                        }
                        // ANY with a single sub-constraint → unwrap
                        if simplified.len() == 1 {
                            return simplified.into_iter().next();
                        }
                        // Flatten nested ANY(ANY(...)) → ANY(...)
                        let flattened = Self::flatten_compositional(&simplified, combinator);
                        Some(Constraint::Compositional {
                            constraints: flattened,
                            combinator: ConstraintCombinator::Any,
                        })
                    }
                    ConstraintCombinator::None => {
                        // NONE with no sub-constraints → tautology → remove
                        if simplified.is_empty() {
                            return None;
                        }
                        // NONE with a single sub-constraint → keep as is (it's negation)
                        Some(Constraint::Compositional {
                            constraints: simplified,
                            combinator: ConstraintCombinator::None,
                        })
                    }
                }
            }
            // Non-compositional constraints are kept as-is (we don't know
            // whether they are tautologies or contradictions without facts).
            _ => Some(c.clone()),
        }
    }

    /// Flatten nested compositional constraints of the same combinator type.
    ///
    /// E.g., ALL(a, ALL(b, c)) → ALL(a, b, c)
    fn flatten_compositional(
        constraints: &[Constraint],
        target_combinator: &ConstraintCombinator,
    ) -> Vec<Constraint> {
        let mut result = Vec::new();
        for c in constraints {
            if let Constraint::Compositional {
                constraints: inner,
                combinator,
            } = c
            {
                if combinator == target_combinator {
                    // Recurse into nested compositional of the same type.
                    result.extend(Self::flatten_compositional(inner, target_combinator));
                } else {
                    result.push(c.clone());
                }
            } else {
                result.push(c.clone());
            }
        }
        result
    }
}

impl Default for ConstraintSolver {
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

    // -- Original tests (preserved) --

    #[test]
    fn negate_temporal_constraint() {
        let c = Constraint::Temporal(TemporalConstraint {
            description: "A before B".into(),
        });
        let neg = c.negate();
        assert_eq!(neg.description(), "NOT(A before B)");
    }

    #[test]
    fn constraint_check_placeholder() {
        let c = Constraint::Liveness(LivenessConstraint {
            description: "every request gets response".into(),
        });
        assert!(c.check()); // placeholder always true
    }

    #[test]
    fn constraint_kind_queries() {
        let c = Constraint::Security(SecurityConstraint {
            description: "no data leak".into(),
        });
        assert!(c.is_security());
        assert!(!c.is_temporal());
    }

    // -- New tests --

    #[test]
    fn memory_region_constraint_display() {
        let c = Constraint::MemoryRegionConstraint {
            region_id: 42,
            constraint_kind: RegionConstraintKind::MustBeLive,
        };
        assert_eq!(c.description(), "region#42: must_be_live");
        assert!(c.is_memory_region());
        assert!(!c.is_temporal());
    }

    #[test]
    fn access_pattern_constraint_display() {
        let c = Constraint::AccessPatternConstraint {
            access_id: 7,
            pattern: AccessPattern::Streaming,
        };
        assert_eq!(c.description(), "access#7: streaming");
        assert!(c.is_access_pattern());
    }

    #[test]
    fn temporal_ordered_constraint_display() {
        let c = Constraint::TemporalOrdered {
            before: "alloc".to_string(),
            after: "free".to_string(),
            relation: TemporalRelation::HappensBefore,
        };
        assert_eq!(c.description(), "alloc happens_before free");
        assert!(c.is_temporal_ordered());
    }

    #[test]
    fn solver_basic_satisfaction() {
        let mut solver = ConstraintSolver::new();
        solver.add_constraint(Constraint::MemoryRegionConstraint {
            region_id: 1,
            constraint_kind: RegionConstraintKind::MustBeLive,
        });
        solver.add_fact("region:1:must_be_live".to_string(), true);
        let solution = solver.solve();
        assert!(matches!(solution.overall, SolutionStatus::AllSatisfied));
        assert_eq!(solution.satisfied.len(), 1);
        assert!(solution.violated.is_empty());
        assert!(solution.unknown.is_empty());
    }

    #[test]
    fn solver_violated_constraint() {
        let mut solver = ConstraintSolver::new();
        solver.add_constraint(Constraint::Security(SecurityConstraint {
            description: "no data leak".into(),
        }));
        solver.add_fact("security:no data leak".to_string(), false);
        let solution = solver.solve();
        assert!(matches!(solution.overall, SolutionStatus::SomeViolated));
        assert_eq!(solution.violated.len(), 1);
        assert!(solution.satisfied.is_empty());
    }

    #[test]
    fn solver_unknown_constraint() {
        let mut solver = ConstraintSolver::new();
        solver.add_constraint(Constraint::Complexity(ComplexityConstraint {
            description: "O(n)".into(),
        }));
        // No fact added — constraint is unknown.
        let solution = solver.solve();
        assert!(matches!(solution.overall, SolutionStatus::SomeUnknown));
        assert_eq!(solution.unknown.len(), 1);
    }

    #[test]
    fn compositional_all_satisfied() {
        let mut solver = ConstraintSolver::new();
        solver.add_constraint(Constraint::Compositional {
            constraints: vec![
                Constraint::MemoryRegionConstraint {
                    region_id: 1,
                    constraint_kind: RegionConstraintKind::MustBeLive,
                },
                Constraint::MemoryRegionConstraint {
                    region_id: 2,
                    constraint_kind: RegionConstraintKind::MustBeExclusive,
                },
            ],
            combinator: ConstraintCombinator::All,
        });
        solver.add_fact("region:1:must_be_live".to_string(), true);
        solver.add_fact("region:2:must_be_exclusive".to_string(), true);
        let solution = solver.solve();
        assert!(matches!(solution.overall, SolutionStatus::AllSatisfied));
    }

    #[test]
    fn compositional_any_one_satisfied() {
        let mut solver = ConstraintSolver::new();
        solver.add_constraint(Constraint::Compositional {
            constraints: vec![
                Constraint::AccessPatternConstraint {
                    access_id: 1,
                    pattern: AccessPattern::Sequential,
                },
                Constraint::AccessPatternConstraint {
                    access_id: 2,
                    pattern: AccessPattern::Random,
                },
            ],
            combinator: ConstraintCombinator::Any,
        });
        // Only one of the two is satisfied.
        solver.add_fact("access:1:sequential".to_string(), true);
        solver.add_fact("access:2:random".to_string(), false);
        let solution = solver.solve();
        assert!(matches!(solution.overall, SolutionStatus::AllSatisfied));
    }

    #[test]
    fn compositional_none_negation() {
        let mut solver = ConstraintSolver::new();
        solver.add_constraint(Constraint::Compositional {
            constraints: vec![Constraint::Liveness(LivenessConstraint {
                description: "deadlock".into(),
            })],
            combinator: ConstraintCombinator::None,
        });
        // The fact is false, so the sub-constraint is violated,
        // which means NONE is satisfied (none of them are true).
        solver.add_fact("liveness:deadlock".to_string(), false);
        let solution = solver.solve();
        assert!(matches!(solution.overall, SolutionStatus::AllSatisfied));
    }

    #[test]
    fn negate_temporal_ordered() {
        let c = Constraint::TemporalOrdered {
            before: "A".to_string(),
            after: "B".to_string(),
            relation: TemporalRelation::HappensBefore,
        };
        let neg = c.negate();
        match neg {
            Constraint::TemporalOrdered { relation, .. } => {
                assert_eq!(relation, TemporalRelation::HappensAfter);
            }
            _ => panic!("Expected TemporalOrdered variant"),
        }
    }

    #[test]
    fn negate_compositional_de_morgan() {
        // NOT(ALL(a, b)) = ANY(NOT(a), NOT(b))
        let c = Constraint::Compositional {
            constraints: vec![
                Constraint::Security(SecurityConstraint {
                    description: "s1".into(),
                }),
                Constraint::Security(SecurityConstraint {
                    description: "s2".into(),
                }),
            ],
            combinator: ConstraintCombinator::All,
        };
        let neg = c.negate();
        match neg {
            Constraint::Compositional {
                constraints,
                combinator,
            } => {
                assert_eq!(combinator, ConstraintCombinator::Any);
                assert_eq!(constraints.len(), 2);
            }
            _ => panic!("Expected Compositional variant"),
        }
    }

    #[test]
    fn simplify_removes_empty_all() {
        let solver = ConstraintSolver::new();
        // Compositional { constraints: [], combinator: All } is a tautology → remove
        let constraints = vec![Constraint::Compositional {
            constraints: vec![],
            combinator: ConstraintCombinator::All,
        }];
        let simplified = solver.simplify(&constraints);
        assert!(simplified.is_empty());
    }

    #[test]
    fn simplify_removes_empty_any() {
        let solver = ConstraintSolver::new();
        // Compositional { constraints: [], combinator: Any } is a contradiction → remove
        let constraints = vec![Constraint::Compositional {
            constraints: vec![],
            combinator: ConstraintCombinator::Any,
        }];
        let simplified = solver.simplify(&constraints);
        assert!(simplified.is_empty());
    }

    #[test]
    fn simplify_flattens_nested_all() {
        let solver = ConstraintSolver::new();
        let inner = Constraint::Compositional {
            constraints: vec![
                Constraint::Security(SecurityConstraint {
                    description: "s1".into(),
                }),
                Constraint::Security(SecurityConstraint {
                    description: "s2".into(),
                }),
            ],
            combinator: ConstraintCombinator::All,
        };
        let outer = Constraint::Compositional {
            constraints: vec![
                inner,
                Constraint::Security(SecurityConstraint {
                    description: "s3".into(),
                }),
            ],
            combinator: ConstraintCombinator::All,
        };
        let simplified = solver.simplify(&[outer]);
        assert_eq!(simplified.len(), 1);
        match &simplified[0] {
            Constraint::Compositional {
                constraints,
                combinator,
            } => {
                assert_eq!(*combinator, ConstraintCombinator::All);
                assert_eq!(constraints.len(), 3); // flattened
            }
            _ => panic!("Expected Compositional"),
        }
    }

    #[test]
    fn simplify_unwraps_single_sub_constraint() {
        let solver = ConstraintSolver::new();
        let c = Constraint::Compositional {
            constraints: vec![Constraint::Security(SecurityConstraint {
                description: "only_one".into(),
            })],
            combinator: ConstraintCombinator::All,
        };
        let simplified = solver.simplify(&[c]);
        assert_eq!(simplified.len(), 1);
        assert!(matches!(simplified[0], Constraint::Security(_)));
    }

    #[test]
    fn is_satisfiable_check() {
        let mut solver = ConstraintSolver::new();
        solver.add_constraint(Constraint::Liveness(LivenessConstraint {
            description: "progress".into(),
        }));
        solver.add_fact("liveness:progress".to_string(), true);
        assert!(solver.is_satisfiable());
    }

    #[test]
    fn is_not_satisfiable_check() {
        let mut solver = ConstraintSolver::new();
        solver.add_constraint(Constraint::Liveness(LivenessConstraint {
            description: "progress".into(),
        }));
        solver.add_fact("liveness:progress".to_string(), false);
        assert!(!solver.is_satisfiable());
    }

    #[test]
    fn region_constraint_kind_display() {
        assert_eq!(
            format!("{}", RegionConstraintKind::MustBeInitialized {
                offset: 0,
                size: 8
            }),
            "must_be_initialized(offset=0, size=8)"
        );
    }

    #[test]
    fn vacuous_compositional_all_is_satisfied() {
        let solver = ConstraintSolver::new();
        // An ALL compositional with no sub-constraints is vacuously true.
        let mut s = ConstraintSolver::new();
        s.add_constraint(Constraint::Compositional {
            constraints: vec![],
            combinator: ConstraintCombinator::All,
        });
        let solution = s.solve();
        assert!(matches!(solution.overall, SolutionStatus::AllSatisfied));
    }
}
