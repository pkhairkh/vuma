//! Capability Descriptors (`CapD`)
//!
//! This module defines the **capability** layer of a Behavioral Descriptor.
//! A `CapD` captures *what operations are permitted* on a value, subject to
//! optional **conditions** that must hold at runtime for the capability to be
//! active.
//!
//! # Capability Lattice
//!
//! `CapD`s form a lattice ordered by set-inclusion on capabilities:
//!
//! ```text
//!   ⊥ = ∅  (no capabilities)
//!   ⊤ = universe of all capabilities
//!   meet(a, b) = a ∩ b
//!   join(a, b) = a ∪ b
//! ```

use crate::context::Context;
use hashbrown::HashSet;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

// ---------------------------------------------------------------------------
// IDs for conditions
// ---------------------------------------------------------------------------

/// Opaque identifier for a phase of execution.
pub type PhaseId = u64;

/// Opaque identifier for an operation.
pub type OpId = u64;

/// Opaque identifier for a lock.
pub type LockId = u64;

/// Opaque identifier for a security level.
pub type SecLevel = u8;

/// Opaque identifier for a memory region.
pub type RegionId = u64;

// ---------------------------------------------------------------------------
// Capability
// ---------------------------------------------------------------------------

/// A fine-grained capability that may be held on a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    /// Permission to read the value.
    Read,
    /// Permission to write (mutate) the value.
    Write,
    /// Permission to execute the value as code.
    Execute,
    /// Permission to iterate over the value (e.g. for-loop).
    Iterate,
    /// Permission to send the value across a concurrency boundary.
    Send,
    /// Permission to persist the value to stable storage.
    Persist,
    /// Permission to serialize the value.
    Serialize,
    /// Permission to deserialize into the value.
    Deserialize,
    /// Permission to compute a hash of the value.
    Hash,
    /// Permission to compare the value for equality/ordering.
    Compare,
    /// Permission to derive a pointer from the value.
    DerivePtr,
    /// Permission to cast the value to a different type.
    Cast,
    /// Permission to fork (clone) the value into a new owner.
    Fork,
    /// Permission to drop (deallocate) the value.
    Drop,
    /// Permission to share the value (shared reference).
    Share,
    /// Permission to move the value (transfer ownership).
    Move,
    /// Permission to pin the value (prevent moves).
    Pin,
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Capability::Read => write!(f, "Read"),
            Capability::Write => write!(f, "Write"),
            Capability::Execute => write!(f, "Execute"),
            Capability::Iterate => write!(f, "Iterate"),
            Capability::Send => write!(f, "Send"),
            Capability::Persist => write!(f, "Persist"),
            Capability::Serialize => write!(f, "Serialize"),
            Capability::Deserialize => write!(f, "Deserialize"),
            Capability::Hash => write!(f, "Hash"),
            Capability::Compare => write!(f, "Compare"),
            Capability::DerivePtr => write!(f, "DerivePtr"),
            Capability::Cast => write!(f, "Cast"),
            Capability::Fork => write!(f, "Fork"),
            Capability::Drop => write!(f, "Drop"),
            Capability::Share => write!(f, "Share"),
            Capability::Move => write!(f, "Move"),
            Capability::Pin => write!(f, "Pin"),
        }
    }
}

// ---------------------------------------------------------------------------
// Condition
// ---------------------------------------------------------------------------

/// A runtime condition that gates the activation of one or more capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Condition {
    /// Capability is active only during the given phase.
    InPhase(PhaseId),
    /// Capability becomes active after the given operation completes.
    AfterOp(OpId),
    /// Capability is active only before the given operation starts.
    BeforeOp(OpId),
    /// Capability is active only when not concurrent with the given operation.
    NotConcurrentWith(OpId),
    /// Capability requires the given lock to be held.
    RequiresLock(LockId),
    /// Capability requires at least the given security clearance.
    SecurityLevel(SecLevel),
    /// Capability is valid only during the given memory region's lifetime.
    ValidDuring(RegionId),
}

impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Condition::InPhase(id) => write!(f, "InPhase({id})"),
            Condition::AfterOp(id) => write!(f, "AfterOp({id})"),
            Condition::BeforeOp(id) => write!(f, "BeforeOp({id})"),
            Condition::NotConcurrentWith(id) => write!(f, "NotConcurrentWith({id})"),
            Condition::RequiresLock(id) => write!(f, "RequiresLock({id})"),
            Condition::SecurityLevel(lvl) => write!(f, "SecurityLevel({lvl})"),
            Condition::ValidDuring(id) => write!(f, "ValidDuring({id})"),
        }
    }
}

// ---------------------------------------------------------------------------
// CapD
// ---------------------------------------------------------------------------

/// A **Capability Descriptor** — the set of permitted operations on a value,
/// together with the runtime conditions under which each capability is active.
///
/// `CapD` forms a lattice with `⊆` as the partial order:
///
/// * `⊥` (bottom) = empty capabilities / no conditions
/// * `⊤` (top)    = all capabilities / all conditions
/// * **meet**     = intersection of capabilities, union of conditions
/// * **join**     = union of capabilities, intersection of conditions
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapD {
    /// The set of capabilities granted.
    pub caps: HashSet<Capability>,
    /// The set of conditions that must hold for any capability to be active.
    pub conditions: HashSet<Condition>,
}

impl CapD {
    /// Construct an empty `CapD` (bottom element of the lattice).
    pub fn empty() -> Self {
        Self {
            caps: HashSet::new(),
            conditions: HashSet::new(),
        }
    }

    /// Construct a `CapD` containing *all* capabilities and no conditions
    /// (top element of the lattice).
    pub fn all() -> Self {
        Self {
            caps: [
                Capability::Read,
                Capability::Write,
                Capability::Execute,
                Capability::Iterate,
                Capability::Send,
                Capability::Persist,
                Capability::Serialize,
                Capability::Deserialize,
                Capability::Hash,
                Capability::Compare,
                Capability::DerivePtr,
                Capability::Cast,
                Capability::Fork,
                Capability::Drop,
                Capability::Share,
                Capability::Move,
                Capability::Pin,
            ]
            .into_iter()
            .collect(),
            conditions: HashSet::new(),
        }
    }

    /// Returns `true` if `self ⊆ other` in the capability lattice.
    ///
    /// This is true when `self.caps ⊆ other.caps` and
    /// `other.conditions ⊆ self.conditions` (fewer conditions ⇒ more
    /// permissive ⇒ higher in the lattice).
    pub fn is_subset(&self, other: &CapD) -> bool {
        self.caps.is_subset(&other.caps) && other.conditions.is_subset(&self.conditions)
    }

    /// Returns `true` if `self ⊇ other` in the capability lattice.
    pub fn is_superset(&self, other: &CapD) -> bool {
        other.is_subset(self)
    }

    /// **Meet** (greatest lower bound) in the capability lattice.
    ///
    /// * Capabilities: intersection
    /// * Conditions: union (more restrictive)
    pub fn meet(&self, other: &CapD) -> CapD {
        CapD {
            caps: self
                .caps
                .intersection(&other.caps)
                .copied()
                .collect(),
            conditions: self
                .conditions
                .union(&other.conditions)
                .copied()
                .collect(),
        }
    }

    /// **Join** (least upper bound) in the capability lattice.
    ///
    /// * Capabilities: union
    /// * Conditions: intersection (less restrictive)
    pub fn join(&self, other: &CapD) -> CapD {
        CapD {
            caps: self.caps.union(&other.caps).copied().collect(),
            conditions: self
                .conditions
                .intersection(&other.conditions)
                .copied()
                .collect(),
        }
    }

    /// Resolve the effective set of capabilities given an execution [`Context`].
    ///
    /// A capability is *active* only when **all** attached conditions are
    /// satisfied by the context.  Conditions that are not relevant to the
    /// current context are conservatively assumed to be unsatisfied.
    pub fn resolve(&self, context: &Context) -> HashSet<Capability> {
        let all_conditions_active = self.conditions.iter().all(|c| context.is_condition_active(c));
        if all_conditions_active {
            self.caps.clone()
        } else {
            HashSet::new()
        }
    }

    /// **Weaken** this descriptor by removing the specified capabilities.
    ///
    /// Returns a new `CapD` with those capabilities excluded.
    pub fn weaken(&self, remove: &[Capability]) -> CapD {
        let remove_set: HashSet<Capability> = remove.iter().copied().collect();
        CapD {
            caps: self.caps.difference(&remove_set).copied().collect(),
            conditions: self.conditions.clone(),
        }
    }

    /// **Widen** this descriptor with `other` to ensure fixpoint convergence
    /// on cyclic data.
    ///
    /// Widening replaces increasing chains with `Top`. If `other` is strictly
    /// above `self` in the lattice (i.e., `self ⊂ other`), the result is
    /// `CapD::all()` (Top). Otherwise, the result is `other` (stable or
    /// decreasing iteration).
    ///
    /// This guarantees that any ascending chain in the CapD lattice converges
    /// in at most two iterations: one to detect the increase, and one to
    /// jump to Top.
    pub fn widen(&self, other: &CapD) -> CapD {
        // If other is strictly above self (strictly more capabilities or
        // strictly fewer conditions), jump to Top to ensure convergence.
        if other.is_superset(self) && other != self {
            CapD::all()
        } else {
            // Stable or decreasing: keep other as the new iterate.
            other.clone()
        }
    }

    /// **Strengthen** this descriptor by adding the specified capabilities
    /// (without adding new conditions).
    pub fn strengthen(&self, add: &[Capability]) -> CapD {
        let mut caps = self.caps.clone();
        for &c in add {
            caps.insert(c);
        }
        CapD {
            caps,
            conditions: self.conditions.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// PartialOrd — lattice order
// ---------------------------------------------------------------------------

impl PartialOrd for CapD {
    /// `self ≤ other` when `self ⊆ other` in the capability lattice.
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self == other {
            Some(Ordering::Equal)
        } else if self.is_subset(other) {
            Some(Ordering::Less)
        } else if self.is_superset(other) {
            Some(Ordering::Greater)
        } else {
            None // incomparable
        }
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl fmt::Display for CapD {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CapD{{")?;
        let mut first = true;
        for c in &self.caps {
            if !first {
                write!(f, ", ")?;
            }
            write!(f, "{c}")?;
            first = false;
        }
        if !self.conditions.is_empty() {
            write!(f, " | ")?;
            let mut first = true;
            for c in &self.conditions {
                if !first {
                    write!(f, ", ")?;
                }
                write!(f, "{c}")?;
                first = false;
            }
        }
        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_bottom() {
        let empty = CapD::empty();
        let all = CapD::all();
        assert!(empty.is_subset(&all));
        assert!(all.is_superset(&empty));
    }

    #[test]
    fn meet_join_laws() {
        let a = CapD {
            caps: [Capability::Read, Capability::Write].into_iter().collect(),
            conditions: HashSet::new(),
        };
        let b = CapD {
            caps: [Capability::Read, Capability::Execute].into_iter().collect(),
            conditions: HashSet::new(),
        };
        let m = a.meet(&b);
        assert!(m.caps.contains(&Capability::Read));
        assert!(!m.caps.contains(&Capability::Write));
        assert!(!m.caps.contains(&Capability::Execute));

        let j = a.join(&b);
        assert!(j.caps.contains(&Capability::Read));
        assert!(j.caps.contains(&Capability::Write));
        assert!(j.caps.contains(&Capability::Execute));
    }

    #[test]
    fn weaken_strengthen() {
        let mut cap = CapD::empty();
        cap = cap.strengthen(&[Capability::Read, Capability::Write]);
        assert!(cap.caps.contains(&Capability::Read));
        cap = cap.weaken(&[Capability::Read]);
        assert!(!cap.caps.contains(&Capability::Read));
        assert!(cap.caps.contains(&Capability::Write));
    }

    #[test]
    fn partial_ord_incomparable() {
        let a = CapD {
            caps: [Capability::Read].into_iter().collect(),
            conditions: HashSet::new(),
        };
        let b = CapD {
            caps: [Capability::Write].into_iter().collect(),
            conditions: HashSet::new(),
        };
        assert_eq!(a.partial_cmp(&b), None);
    }
}
