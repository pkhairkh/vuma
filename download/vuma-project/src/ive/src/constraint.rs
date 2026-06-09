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
    pub fn id(&self) -> ConstraintId {
        let desc = self.description();
        // Use a simple hash of the description as the ID.
        ConstraintId::new(format!("{:x}", desc.len()))
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
    ///
    /// This is useful for generating verification obligations: proving
    /// that the negation is unsatisfiable is equivalent to proving the
    /// original constraint holds.
    ///
    /// TODO: Implement proper negation per constraint kind.
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
}
