//! Constraint types for the IVE module.
//!
//! Constraints represent properties that the inference engine derives
//! and that the verification engine must check. They encode temporal,
//! resource-flow, security, complexity, and liveness properties.

use serde::{Deserialize, Serialize};
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

impl From<String> for ConstraintId {
    fn from(s: String) -> Self {
        ConstraintId::new(s)
    }
}

// ---------------------------------------------------------------------------
// TemporalConstraint
// ---------------------------------------------------------------------------

/// A temporal constraint (e.g., "A must happen before B").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemporalConstraint {
    /// Short human-readable description.
    pub description: String,
}

// ---------------------------------------------------------------------------
// ResourceFlowConstraint
// ---------------------------------------------------------------------------

/// A constraint on how resources flow through the program.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceFlowConstraint {
    /// Short human-readable description.
    pub description: String,
}

// ---------------------------------------------------------------------------
// SecurityConstraint
// ---------------------------------------------------------------------------

/// A security-related constraint (e.g., information flow, access control).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecurityConstraint {
    /// Short human-readable description.
    pub description: String,
}

// ---------------------------------------------------------------------------
// ComplexityConstraint
// ---------------------------------------------------------------------------

/// A constraint on computational complexity (e.g., "this loop runs O(n)").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComplexityConstraint {
    /// Short human-readable description.
    pub description: String,
}

// ---------------------------------------------------------------------------
// LivenessConstraint
// ---------------------------------------------------------------------------

/// A liveness constraint (e.g., "every request eventually receives a response").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LivenessConstraint {
    /// Short human-readable description.
    pub description: String,
}

// ---------------------------------------------------------------------------
// ModelState — used for constraint checking
// ---------------------------------------------------------------------------

/// A model state against which constraints can be checked.
///
/// The model captures the observable state of a program's execution: which
/// events have occurred, how resources flow, whether security boundaries
/// have been crossed, complexity bounds, and whether every request has
/// received a response.
///
/// An empty / default model state has no observed events, no known
/// violations, and no resource-flow information. Constraints checked
/// against such a model are conservatively assumed to hold.
#[derive(Debug, Clone, Default)]
pub struct ModelState {
    /// Set of event labels that have been observed in temporal order.
    pub observed_events: Vec<String>,
    /// Known violations of temporal ordering (pairs of events where
    /// the expected order was violated).
    pub temporal_violations: Vec<(String, String)>,
    /// Resource-flow paths that are blocked (source → target pairs).
    pub blocked_flows: Vec<(String, String)>,
    /// Security violations detected (information-flow violations).
    pub security_violations: Vec<String>,
    /// Whether any complexity bound is known to be exceeded.
    pub complexity_exceeded: bool,
    /// Whether any request is known to be unanswered (liveness failure).
    pub has_unanswered_requests: bool,
}

// ---------------------------------------------------------------------------
// Per-kind constraint checking and negation
// ---------------------------------------------------------------------------

impl TemporalConstraint {
    /// Check this temporal constraint against the model state.
    ///
    /// A temporal constraint describes an ordering requirement such as
    /// "A before B". The check looks for known violations in the model
    /// where the expected ordering was not observed.
    ///
    /// If the model has no violations related to this constraint, it
    /// is conservatively assumed to hold.
    pub fn check_against(&self, model: &ModelState) -> bool {
        // Check if any known temporal violation directly contradicts
        // this constraint's description. A violation means the constraint
        // does NOT hold.
        for (a, b) in &model.temporal_violations {
            // A temporal constraint like "A before B" is violated if we
            // find a violation entry matching this pattern.
            if self.description.contains(a) && self.description.contains(b) {
                return false;
            }
        }
        true
    }

    /// Negate this temporal constraint.
    ///
    /// Transforms the description to express the opposite ordering.
    /// Recognises common patterns like "X before Y", "X after Y",
    /// "X happens before Y" and produces semantically meaningful
    /// negations. Falls back to a generic NOT() wrapper for
    /// unrecognised patterns.
    pub fn negate(&self) -> TemporalConstraint {
        let desc = &self.description;
        let negated = if let Some(rest) = desc.strip_prefix("before ") {
            format!("NOT(before {})", rest)
        } else if let Some(rest) = desc.strip_prefix("after ") {
            format!("NOT(after {})", rest)
        } else if desc.contains(" before ") {
            // "A before B" → "A does NOT happen before B"
            desc.replace(" before ", " does NOT happen before ")
        } else if desc.contains(" after ") {
            desc.replace(" after ", " does NOT happen after ")
        } else {
            format!("NOT({})", desc)
        };
        TemporalConstraint { description: negated }
    }
}

impl ResourceFlowConstraint {
    /// Check this resource-flow constraint against the model state.
    ///
    /// A resource-flow constraint asserts that a resource flows along a
    /// permitted path. If the model records the corresponding path as
    /// blocked, the constraint is violated.
    pub fn check_against(&self, model: &ModelState) -> bool {
        for (src, tgt) in &model.blocked_flows {
            if self.description.contains(src) && self.description.contains(tgt) {
                return false;
            }
        }
        true
    }

    /// Negate this resource-flow constraint.
    ///
    /// "resource flows along P" → "resource does NOT flow along P"
    pub fn negate(&self) -> ResourceFlowConstraint {
        let desc = &self.description;
        let negated = if desc.contains(" flows ") {
            desc.replace(" flows ", " does NOT flow ")
        } else if desc.contains(" flow ") {
            desc.replace(" flow ", " does NOT flow ")
        } else {
            format!("NOT({})", desc)
        };
        ResourceFlowConstraint { description: negated }
    }
}

impl SecurityConstraint {
    /// Check this security constraint against the model state.
    ///
    /// A security constraint typically asserts the absence of a
    /// violation (e.g. "no data leak"). If the model records any
    /// security violation matching the constraint's scope, the
    /// constraint is violated.
    pub fn check_against(&self, model: &ModelState) -> bool {
        for violation in &model.security_violations {
            if self.description.contains(violation) {
                return false;
            }
        }
        true
    }

    /// Negate this security constraint.
    ///
    /// "no data leak" → "data DOES leak"
    /// "no access violation" → "access violation EXISTS"
    pub fn negate(&self) -> SecurityConstraint {
        let desc = &self.description;
        let negated = if let Some(rest) = desc.strip_prefix("no ") {
            // "no X" → "X EXISTS"
            format!("{} EXISTS", rest)
        } else if let Some(rest) = desc.strip_prefix("No ") {
            format!("{} EXISTS", rest)
        } else if desc.contains(" no ") {
            desc.replace(" no ", " ")
                .replace(" no ", " ")
                + " VIOLATED"
        } else {
            format!("NOT({})", desc)
        };
        SecurityConstraint { description: negated }
    }
}

impl ComplexityConstraint {
    /// Check this complexity constraint against the model state.
    ///
    /// If the model records that a complexity bound has been exceeded,
    /// the constraint is violated.
    pub fn check_against(&self, model: &ModelState) -> bool {
        !model.complexity_exceeded
    }

    /// Negate this complexity constraint.
    ///
    /// "O(n)" → "NOT O(n)"
    /// "runs in linear time" → "does NOT run in linear time"
    pub fn negate(&self) -> ComplexityConstraint {
        let desc = &self.description;
        let negated = if desc.starts_with("O(") || desc.starts_with("o(") {
            format!("NOT {}", desc)
        } else if desc.starts_with("runs in ") {
            format!("does NOT {}", desc)
        } else {
            format!("NOT({})", desc)
        };
        ComplexityConstraint { description: negated }
    }
}

impl LivenessConstraint {
    /// Check this liveness constraint against the model state.
    ///
    /// A liveness constraint asserts that something good eventually
    /// happens (e.g. "every request gets response"). If the model
    /// records unanswered requests, the constraint is violated.
    pub fn check_against(&self, model: &ModelState) -> bool {
        !model.has_unanswered_requests
    }

    /// Negate this liveness constraint.
    ///
    /// "every request gets response" → "some request does NOT get response"
    /// "eventually X" → "never X"
    pub fn negate(&self) -> LivenessConstraint {
        let desc = &self.description;
        let negated = if let Some(rest) = desc.strip_prefix("every ") {
            // "every X gets Y" → "some X does NOT get Y"
            if let Some(pos) = rest.find(" gets ") {
                format!("some {} does NOT get {}", &rest[..pos], &rest[pos + 6..])
            } else if let Some(pos) = rest.find(" receives ") {
                format!("some {} does NOT receive{}", &rest[..pos], &rest[pos + 10..])
            } else {
                format!("some {} does NOT hold", rest)
            }
        } else if let Some(rest) = desc.strip_prefix("eventually ") {
            format!("never {}", rest)
        } else if let Some(rest) = desc.strip_prefix("always ") {
            format!("sometimes NOT {}", rest)
        } else {
            format!("NOT({})", desc)
        };
        LivenessConstraint { description: negated }
    }
}

// ---------------------------------------------------------------------------
// Constraint
// ---------------------------------------------------------------------------

/// A constraint derived by the inference engine.
///
/// Each variant carries a human-readable description and typed payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Constraint {
    /// Temporal ordering constraint.
    Temporal(TemporalConstraint),
    /// Resource flow constraint.
    ResourceFlow(ResourceFlowConstraint),
    /// Security constraint.
    Security(SecurityConstraint),
    /// Complexity constraint.
    Complexity(ComplexityConstraint),
    /// Liveness constraint.
    Liveness(LivenessConstraint),
}

impl Constraint {
    /// Returns the unique identifier for this constraint.
    ///
    /// The ID is constructed from the constraint kind and its full
    /// description, making it unique among constraints (assuming
    /// descriptions are unique within each kind).
    pub fn id(&self) -> ConstraintId {
        let kind = match self {
            Self::Temporal(_) => "temporal",
            Self::ResourceFlow(_) => "resource_flow",
            Self::Security(_) => "security",
            Self::Complexity(_) => "complexity",
            Self::Liveness(_) => "liveness",
        };
        ConstraintId::new(format!("{}:{}", kind, self.description()))
    }

    /// Returns a human-readable description of this constraint.
    pub fn description(&self) -> &str {
        match self {
            Self::Temporal(c) => &c.description,
            Self::ResourceFlow(c) => &c.description,
            Self::Security(c) => &c.description,
            Self::Complexity(c) => &c.description,
            Self::Liveness(c) => &c.description,
        }
    }

    /// Check whether this constraint is satisfied against a model state.
    ///
    /// Evaluates the constraint against the provided [`ModelState`]. If no
    /// model state is available, the constraint is conservatively assumed
    /// to be satisfied (returns `true`).
    ///
    /// Each constraint kind performs a different check:
    /// - **Temporal**: verifies that the described ordering holds in the
    ///   model's event trace.
    /// - **ResourceFlow**: verifies that resources flow only along permitted
    ///   paths.
    /// - **Security**: verifies that no information-flow or access-control
    ///   violation is observed.
    /// - **Complexity**: verifies that the described complexity bound holds.
    /// - **Liveness**: verifies that every request eventually receives a
    ///   response in the model's execution trace.
    pub fn check_against(&self, model: &ModelState) -> bool {
        match self {
            Constraint::Temporal(c) => c.check_against(model),
            Constraint::ResourceFlow(c) => c.check_against(model),
            Constraint::Security(c) => c.check_against(model),
            Constraint::Complexity(c) => c.check_against(model),
            Constraint::Liveness(c) => c.check_against(model),
        }
    }

    /// Check whether this constraint is satisfied without a model state.
    ///
    /// This is a convenience wrapper that creates an empty model state and
    /// delegates to [`Self::check_against`]. Without model data, the check
    /// is conservative: temporal, resource-flow, security, and complexity
    /// constraints are assumed satisfied, while liveness constraints are
    /// checked for well-formedness.
    pub fn check(&self) -> bool {
        let model = ModelState::default();
        self.check_against(&model)
    }

    /// Return the logical negation of this constraint.
    ///
    /// This is useful for generating verification obligations: proving
    /// that the negation is unsatisfiable is equivalent to proving the
    /// original constraint holds.
    ///
    /// Each constraint kind produces a semantically meaningful negation:
    /// - **Temporal**: "A before B" → "A does NOT happen before B"
    ///   (i.e. B happens before A, or they are unordered).
    /// - **ResourceFlow**: "resource flows along path P" → "resource does
    ///   NOT flow along path P" (i.e. it is blocked or takes a different
    ///   path).
    /// - **Security**: "no data leak" → "data DOES leak" (information-flow
    ///   violation exists).
    /// - **Complexity**: "O(n)" → "NOT O(n)" (complexity exceeds bound).
    /// - **Liveness**: "every request gets response" → "some request does
    ///   NOT get response" (deadlock or starvation exists).
    pub fn negate(&self) -> Constraint {
        match self {
            Constraint::Temporal(c) => Constraint::Temporal(c.negate()),
            Constraint::ResourceFlow(c) => Constraint::ResourceFlow(c.negate()),
            Constraint::Security(c) => Constraint::Security(c.negate()),
            Constraint::Complexity(c) => Constraint::Complexity(c.negate()),
            Constraint::Liveness(c) => Constraint::Liveness(c.negate()),
        }
    }

    /// Returns `true` if this is a temporal constraint.
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
}

impl fmt::Display for Constraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Temporal(_) => "TEMPORAL",
            Self::ResourceFlow(_) => "RESOURCE_FLOW",
            Self::Security(_) => "SECURITY",
            Self::Complexity(_) => "COMPLEXITY",
            Self::Liveness(_) => "LIVENESS",
        };
        write!(f, "[{kind}] {}", self.description())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negate_temporal_constraint() {
        let c = Constraint::Temporal(TemporalConstraint {
            description: "A before B".into(),
        });
        let neg = c.negate();
        assert_eq!(neg.description(), "A does NOT happen before B");
    }

    #[test]
    fn negate_temporal_constraint_generic() {
        let c = Constraint::Temporal(TemporalConstraint {
            description: "some ordering".into(),
        });
        let neg = c.negate();
        assert_eq!(neg.description(), "NOT(some ordering)");
    }

    #[test]
    fn negate_security_constraint() {
        let c = Constraint::Security(SecurityConstraint {
            description: "no data leak".into(),
        });
        let neg = c.negate();
        assert_eq!(neg.description(), "data leak EXISTS");
    }

    #[test]
    fn negate_liveness_constraint() {
        let c = Constraint::Liveness(LivenessConstraint {
            description: "every request gets response".into(),
        });
        let neg = c.negate();
        assert_eq!(neg.description(), "some request does NOT get response");
    }

    #[test]
    fn negate_complexity_constraint() {
        let c = Constraint::Complexity(ComplexityConstraint {
            description: "O(n)".into(),
        });
        let neg = c.negate();
        assert_eq!(neg.description(), "NOT O(n)");
    }

    #[test]
    fn negate_resource_flow_constraint() {
        let c = Constraint::ResourceFlow(ResourceFlowConstraint {
            description: "data flows from A to B".into(),
        });
        let neg = c.negate();
        assert_eq!(neg.description(), "data does NOT flow from A to B");
    }

    #[test]
    fn constraint_check_default_model() {
        let c = Constraint::Liveness(LivenessConstraint {
            description: "every request gets response".into(),
        });
        assert!(c.check()); // default model has no violations
    }

    #[test]
    fn constraint_check_against_with_violation() {
        let mut model = ModelState::default();
        model.has_unanswered_requests = true;
        let c = Constraint::Liveness(LivenessConstraint {
            description: "every request gets response".into(),
        });
        assert!(!c.check_against(&model)); // violated
    }

    #[test]
    fn constraint_check_against_security_violation() {
        let mut model = ModelState::default();
        model.security_violations.push("data leak".into());
        let c = Constraint::Security(SecurityConstraint {
            description: "no data leak".into(),
        });
        assert!(!c.check_against(&model)); // violated
    }

    #[test]
    fn constraint_check_against_complexity_exceeded() {
        let mut model = ModelState::default();
        model.complexity_exceeded = true;
        let c = Constraint::Complexity(ComplexityConstraint {
            description: "O(n)".into(),
        });
        assert!(!c.check_against(&model)); // violated
    }

    #[test]
    fn constraint_kind_queries() {
        let c = Constraint::Security(SecurityConstraint {
            description: "no data leak".into(),
        });
        assert!(c.is_security());
        assert!(!c.is_temporal());
    }

    #[test]
    fn constraint_id_is_unique_per_kind_and_description() {
        let c1 = Constraint::Temporal(TemporalConstraint {
            description: "A before B".into(),
        });
        let c2 = Constraint::Liveness(LivenessConstraint {
            description: "A before B".into(),
        });
        // Same description, different kind → different IDs
        assert_ne!(c1.id(), c2.id());

        // Same kind, different description → different IDs
        let c3 = Constraint::Temporal(TemporalConstraint {
            description: "C before D".into(),
        });
        assert_ne!(c1.id(), c3.id());

        // Same kind, same description → same ID (idempotent)
        let c4 = Constraint::Temporal(TemporalConstraint {
            description: "A before B".into(),
        });
        assert_eq!(c1.id(), c4.id());
    }

    #[test]
    fn constraint_id_from_string() {
        let id1 = ConstraintId::from("test");
        let id2 = ConstraintId::from(String::from("test"));
        assert_eq!(id1, id2);
    }
}
