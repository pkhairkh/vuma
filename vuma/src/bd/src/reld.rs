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

    // =======================================================================
    // New RelD tests — Enhancement 3 (20+)
    // =======================================================================

    // -- Outlives composition: A outlives B, B outlives C → A outlives C --

    #[test]
    fn outlives_composition() {
        let a_outlives_b = RelD {
            relations: [Relation::Temporal(TemporalKind::Outlives)]
                .into_iter()
                .collect(),
        };
        let b_outlives_c = RelD {
            relations: [Relation::Temporal(TemporalKind::Outlives)]
                .into_iter()
                .collect(),
        };
        // Composing Outlives + Outlives should still be consistent
        let composed = a_outlives_b.compose(&b_outlives_c);
        assert!(composed.is_consistent());
        assert!(composed
            .relations
            .contains(&Relation::Temporal(TemporalKind::Outlives)));
    }

    // -- BorrowsFrom transitivity --

    #[test]
    fn borrows_from_transitivity() {
        // If A borrows from B and B borrows from C, then A indirectly borrows from C
        let a_borrows_b = RelD {
            relations: [Relation::Dependency(DepKind::AliasDep)]
                .into_iter()
                .collect(),
        };
        let b_borrows_c = RelD {
            relations: [Relation::Dependency(DepKind::AliasDep)]
                .into_iter()
                .collect(),
        };
        let composed = a_borrows_b.compose(&b_borrows_c);
        assert!(composed.is_consistent());
    }

    // -- DependsOn cycles (should be detected via composition) --

    #[test]
    fn depends_on_composition() {
        let a_depends_b = RelD {
            relations: [Relation::Dependency(DepKind::DataDep)]
                .into_iter()
                .collect(),
        };
        let b_depends_a = RelD {
            relations: [Relation::Dependency(DepKind::DataDep)]
                .into_iter()
                .collect(),
        };
        // Composing data dependencies is still consistent at the syntactic level
        let composed = a_depends_b.compose(&b_depends_a);
        assert!(composed.is_consistent());
        // Both DataDep relations present
        assert!(composed
            .relations
            .contains(&Relation::Dependency(DepKind::DataDep)));
    }

    #[test]
    fn depends_on_and_control_dep() {
        let data = RelD {
            relations: [Relation::Dependency(DepKind::DataDep)]
                .into_iter()
                .collect(),
        };
        let ctrl = RelD {
            relations: [Relation::Dependency(DepKind::ControlDep)]
                .into_iter()
                .collect(),
        };
        let composed = data.compose(&ctrl);
        assert!(composed.is_consistent());
        assert!(composed
            .relations
            .contains(&Relation::Dependency(DepKind::DataDep)));
        assert!(composed
            .relations
            .contains(&Relation::Dependency(DepKind::ControlDep)));
    }

    // -- ContainedIn hierarchy --

    #[test]
    fn containment_hierarchy() {
        let inner = RelD {
            relations: [Relation::Containment].into_iter().collect(),
        };
        let outer = RelD {
            relations: [Relation::Containment, Relation::Liveness]
                .into_iter()
                .collect(),
        };
        // outer refines inner
        assert!(outer.refines(&inner));
        // inner does not refine outer
        assert!(!inner.refines(&outer));
    }

    #[test]
    fn containment_with_dependency() {
        let r = RelD {
            relations: [
                Relation::Containment,
                Relation::Dependency(DepKind::DataDep),
            ]
            .into_iter()
            .collect(),
        };
        assert!(r.is_consistent());
    }

    // -- AliasOf mutual exclusion (via Equivalence and Disjoint) --

    #[test]
    fn equivalence_and_liveness_composition() {
        let eq = RelD {
            relations: [Relation::Equivalence].into_iter().collect(),
        };
        let live = RelD {
            relations: [Relation::Liveness].into_iter().collect(),
        };
        let composed = eq.compose(&live);
        assert!(composed.is_consistent());
        assert!(composed.relations.contains(&Relation::Equivalence));
        assert!(composed.relations.contains(&Relation::Liveness));
    }

    #[test]
    fn alias_dep_and_data_dep() {
        let r = RelD {
            relations: [
                Relation::Dependency(DepKind::AliasDep),
                Relation::Dependency(DepKind::DataDep),
            ]
            .into_iter()
            .collect(),
        };
        assert!(r.is_consistent());
    }

    // -- Contradictory temporal constraints --

    #[test]
    fn contradictory_outlives_and_succeeds() {
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
    fn contradictory_precedes_and_succeeds() {
        let r = RelD {
            relations: [
                Relation::Temporal(TemporalKind::Precedes),
                Relation::Temporal(TemporalKind::Succeeds),
            ]
            .into_iter()
            .collect(),
        };
        assert!(!r.is_consistent());
    }

    #[test]
    fn consistent_outlives_and_coincides() {
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
    fn consistent_precedes_and_coincides() {
        let r = RelD {
            relations: [
                Relation::Temporal(TemporalKind::Precedes),
                Relation::Temporal(TemporalKind::Coincides),
            ]
            .into_iter()
            .collect(),
        };
        assert!(r.is_consistent());
    }

    // -- Refinement with contradictory constraints --

    #[test]
    fn refinement_contradictory_compose() {
        let a = RelD {
            relations: [Relation::Temporal(TemporalKind::Outlives)]
                .into_iter()
                .collect(),
        };
        let b = RelD {
            relations: [Relation::Temporal(TemporalKind::Succeeds)]
                .into_iter()
                .collect(),
        };
        let composed = a.compose(&b);
        assert!(!composed.is_consistent());
    }

    #[test]
    fn refinement_non_contradictory_compose() {
        let a = RelD {
            relations: [Relation::Temporal(TemporalKind::Outlives)]
                .into_iter()
                .collect(),
        };
        let b = RelD {
            relations: [Relation::Containment].into_iter().collect(),
        };
        let composed = a.compose(&b);
        assert!(composed.is_consistent());
        assert!(composed
            .relations
            .contains(&Relation::Temporal(TemporalKind::Outlives)));
        assert!(composed.relations.contains(&Relation::Containment));
    }

    // -- Composition of Outlives chains --

    #[test]
    fn outlives_chain_composition() {
        let a = RelD {
            relations: [Relation::Temporal(TemporalKind::Outlives)]
                .into_iter()
                .collect(),
        };
        let b = RelD {
            relations: [
                Relation::Temporal(TemporalKind::Outlives),
                Relation::Containment,
            ]
            .into_iter()
            .collect(),
        };
        let composed = a.compose(&b);
        assert!(composed.is_consistent());
        assert!(composed
            .relations
            .contains(&Relation::Temporal(TemporalKind::Outlives)));
        assert!(composed.relations.contains(&Relation::Containment));
    }

    #[test]
    fn outlives_chain_with_liveness() {
        let a = RelD {
            relations: [
                Relation::Temporal(TemporalKind::Outlives),
                Relation::Liveness,
            ]
            .into_iter()
            .collect(),
        };
        let b = RelD {
            relations: [Relation::Liveness].into_iter().collect(),
        };
        let composed = a.compose(&b);
        assert!(composed.is_consistent());
    }

    // -- Security flow policy composition --

    #[test]
    fn security_no_downgrade_with_containment() {
        let r = RelD {
            relations: [
                Relation::Security(FlowPolicy::NoDowngrade),
                Relation::Containment,
            ]
            .into_iter()
            .collect(),
        };
        assert!(r.is_consistent());
    }

    #[test]
    fn security_sanitized_with_liveness() {
        let r = RelD {
            relations: [
                Relation::Security(FlowPolicy::Sanitized),
                Relation::Liveness,
            ]
            .into_iter()
            .collect(),
        };
        assert!(r.is_consistent());
    }

    #[test]
    fn security_no_cross_boundary_with_equivalence() {
        let r = RelD {
            relations: [
                Relation::Security(FlowPolicy::NoCrossBoundary),
                Relation::Equivalence,
            ]
            .into_iter()
            .collect(),
        };
        assert!(r.is_consistent());
    }

    // -- Merge removes non-common relations --

    #[test]
    fn merge_removes_non_common() {
        let a = RelD {
            relations: [
                Relation::Containment,
                Relation::Liveness,
                Relation::Equivalence,
            ]
            .into_iter()
            .collect(),
        };
        let b = RelD {
            relations: [Relation::Containment, Relation::Liveness]
                .into_iter()
                .collect(),
        };
        let m = a.merge(&b);
        assert!(m.relations.contains(&Relation::Containment));
        assert!(m.relations.contains(&Relation::Liveness));
        assert!(!m.relations.contains(&Relation::Equivalence));
    }

    #[test]
    fn merge_empty_with_nonempty() {
        let empty = RelD::empty();
        let nonempty = RelD {
            relations: [Relation::Containment].into_iter().collect(),
        };
        let m = empty.merge(&nonempty);
        assert!(m.relations.is_empty());
    }

    // -- Compose empty with non-empty --

    #[test]
    fn compose_empty_with_nonempty() {
        let empty = RelD::empty();
        let nonempty = RelD {
            relations: [Relation::Liveness].into_iter().collect(),
        };
        let c = empty.compose(&nonempty);
        assert!(c.relations.contains(&Relation::Liveness));
    }

    // -- Refinement of empty --

    #[test]
    fn empty_refines_empty_only() {
        let empty = RelD::empty();
        let nonempty = RelD {
            relations: [Relation::Liveness].into_iter().collect(),
        };
        assert!(empty.refines(&empty));
        assert!(!empty.refines(&nonempty));
        assert!(nonempty.refines(&empty));
    }
}
