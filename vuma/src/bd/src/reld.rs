//! Relational Descriptors (`RelD`)
//!
//! This module defines the **relational** layer of a Behavioral Descriptor.
//! A `RelD` captures the *relationships* that a value participates in —
//! temporal ordering, containment, data/control dependencies, equivalence
//! classes, security boundaries, and liveness guarantees.
//!
//! # Relations
//!
//! | Relation      | Meaning                                            |
//! |---------------|----------------------------------------------------|
//! | `Temporal`    | Ordering constraints between lifetime events        |
//! | `Containment` | One value is contained within another               |
//! | `Dependency`  | Data, control, or alias dependencies                |
//! | `Equivalence` | Values are observationally equivalent               |
//! | `Security`    | Information-flow / boundary constraints             |
//! | `Liveness`    | The value is guaranteed to eventually be usable     |

use hashbrown::HashSet;
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// Temporal kinds
// ---------------------------------------------------------------------------

/// Kinds of temporal ordering between two lifetimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TemporalKind {
    /// The first lifetime outlives the second.
    Outlives,
    /// The two lifetimes coincide (start and end together).
    Coincides,
    /// The first lifetime precedes (ends before) the second.
    Precedes,
    /// The first lifetime succeeds (starts after) the second.
    Succeeds,
}

impl fmt::Display for TemporalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TemporalKind::Outlives => write!(f, "Outlives"),
            TemporalKind::Coincides => write!(f, "Coincides"),
            TemporalKind::Precedes => write!(f, "Precedes"),
            TemporalKind::Succeeds => write!(f, "Succeeds"),
        }
    }
}

// ---------------------------------------------------------------------------
// Dependency kinds
// ---------------------------------------------------------------------------

/// Kinds of dependency between two operations or values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DepKind {
    /// Data dependency — one value flows into the other.
    DataDep,
    /// Control dependency — one value's existence depends on a branch.
    ControlDep,
    /// Alias dependency — the two values may alias the same memory.
    AliasDep,
}

impl fmt::Display for DepKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DepKind::DataDep => write!(f, "DataDep"),
            DepKind::ControlDep => write!(f, "ControlDep"),
            DepKind::AliasDep => write!(f, "AliasDep"),
        }
    }
}

// ---------------------------------------------------------------------------
// Flow policies
// ---------------------------------------------------------------------------

/// Information-flow policies governing how data may move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FlowPolicy {
    /// Data must not be downgraded to a less-restrictive label.
    NoDowngrade,
    /// Data must not cross a security boundary.
    NoCrossBoundary,
    /// Data must be sanitized before crossing a boundary.
    Sanitized,
}

impl fmt::Display for FlowPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlowPolicy::NoDowngrade => write!(f, "NoDowngrade"),
            FlowPolicy::NoCrossBoundary => write!(f, "NoCrossBoundary"),
            FlowPolicy::Sanitized => write!(f, "Sanitized"),
        }
    }
}

// ---------------------------------------------------------------------------
// Relation
// ---------------------------------------------------------------------------

/// A relation that a value participates in.
///
/// Each variant carries just enough metadata to identify the kind and
/// (optionally) the flavour of the relation.  The actual *endpoints* of
/// each relation are tracked externally — the `RelD` merely enumerates
/// *which* relations exist.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Relation {
    /// Temporal ordering constraint.
    Temporal(TemporalKind),
    /// Containment (one value is nested inside another).
    Containment,
    /// Dependency edge (data, control, or alias).
    Dependency(DepKind),
    /// Observational equivalence.
    Equivalence,
    /// Security / information-flow boundary.
    Security(FlowPolicy),
    /// Liveness guarantee — the value is eventually usable.
    Liveness,
}

impl fmt::Display for Relation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Relation::Temporal(k) => write!(f, "Temporal({k})"),
            Relation::Containment => write!(f, "Containment"),
            Relation::Dependency(k) => write!(f, "Dependency({k})"),
            Relation::Equivalence => write!(f, "Equivalence"),
            Relation::Security(p) => write!(f, "Security({p})"),
            Relation::Liveness => write!(f, "Liveness"),
        }
    }
}

// ---------------------------------------------------------------------------
// RelD
// ---------------------------------------------------------------------------

/// A **Relational Descriptor** — the set of relations a value participates in.
///
/// `RelD` supports a refinement ordering (`⊑`): `a ⊑ b` when every
/// relation in `a` is also present (or refined) in `b`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelD {
    /// The set of relations.
    pub relations: HashSet<Relation>,
}

impl RelD {
    /// Construct an empty `RelD` (no relations).
    pub fn empty() -> Self {
        Self {
            relations: HashSet::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Refinement
    // -----------------------------------------------------------------------

    /// `self` **refines** `other` when every relation in `other` is present
    /// in `self` (or is refined by a more specific relation in `self`).
    ///
    /// A simple approximation: `other.relations ⊆ self.relations`.
    pub fn refines(&self, other: &RelD) -> bool {
        other.relations.is_subset(&self.relations)
    }

    // -----------------------------------------------------------------------
    // Composition
    // -----------------------------------------------------------------------

    /// **Compose** two relational descriptors by taking the union of their
    /// relations.  This models the combination of two independent views of
    /// the same value.
    pub fn compose(&self, other: &RelD) -> RelD {
        RelD {
            relations: self.relations.union(&other.relations).cloned().collect(),
        }
    }

    // -----------------------------------------------------------------------
    // Consistency
    // -----------------------------------------------------------------------

    /// Returns `true` when the set of relations is internally consistent.
    ///
    /// Inconsistency can arise from contradictory temporal constraints:
    /// e.g. `Temporal(Outlives)` and `Temporal(Succeeds)` on the same pair
    /// would be inconsistent.  At this level we perform a conservative
    /// syntactic check.
    pub fn is_consistent(&self) -> bool {
        // Check for contradictory temporal pairs.
        let has_outlives = self
            .relations
            .contains(&Relation::Temporal(TemporalKind::Outlives));
        let has_succeeds = self
            .relations
            .contains(&Relation::Temporal(TemporalKind::Succeeds));
        if has_outlives && has_succeeds {
            return false;
        }

        let has_precedes = self
            .relations
            .contains(&Relation::Temporal(TemporalKind::Precedes));
        let _has_coincides = self
            .relations
            .contains(&Relation::Temporal(TemporalKind::Coincides));
        // Coincides is compatible with everything, but precedes + succeeds
        // is contradictory.
        if has_precedes && has_succeeds {
            return false;
        }

        // Security: Sanitized is only meaningful if a boundary exists.
        // (This is a lightweight heuristic — not a full IFC check.)
        true
    }

    // -----------------------------------------------------------------------
    // Merge
    // -----------------------------------------------------------------------

    /// **Merge** two relational descriptors, returning the intersection of
    /// their relations.  This models the greatest common refinement — only
    /// relations agreed upon by both sides survive.
    pub fn merge(&self, other: &RelD) -> RelD {
        RelD {
            relations: self
                .relations
                .intersection(&other.relations)
                .cloned()
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl fmt::Display for RelD {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelD{{")?;
        let mut first = true;
        for r in &self.relations {
            if !first {
                write!(f, ", ")?;
            }
            write!(f, "{r}")?;
            first = false;
        }
        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_refines_empty() {
        let a = RelD::empty();
        let b = RelD::empty();
        assert!(a.refines(&b));
    }

    #[test]
    fn compose_adds_relations() {
        let a = RelD {
            relations: [Relation::Containment].into_iter().collect(),
        };
        let b = RelD {
            relations: [Relation::Liveness].into_iter().collect(),
        };
        let c = a.compose(&b);
        assert!(c.relations.contains(&Relation::Containment));
        assert!(c.relations.contains(&Relation::Liveness));
    }

    #[test]
    fn inconsistent_temporal() {
        let r = RelD {
            relations: [
                Relation::Temporal(TemporalKind::Outlives),
                Relation::Temporal(TemporalKind::Succeeds),
            ]
            .into_iter()
            .collect(),
        };
        assert!(!r.is_consistent());
    }

    #[test]
    fn consistent_temporal() {
        let r = RelD {
            relations: [
                Relation::Temporal(TemporalKind::Outlives),
                Relation::Temporal(TemporalKind::Coincides),
            ]
            .into_iter()
            .collect(),
        };
        assert!(r.is_consistent());
    }

    #[test]
    fn merge_keeps_common() {
        let a = RelD {
            relations: [Relation::Containment, Relation::Liveness]
                .into_iter()
                .collect(),
        };
        let b = RelD {
            relations: [Relation::Containment, Relation::Equivalence]
                .into_iter()
                .collect(),
        };
        let m = a.merge(&b);
        assert!(m.relations.contains(&Relation::Containment));
        assert!(!m.relations.contains(&Relation::Liveness));
        assert!(!m.relations.contains(&Relation::Equivalence));
    }
}
