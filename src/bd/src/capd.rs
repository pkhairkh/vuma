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
use std::collections::HashSet;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// Read a field from a state (typed offset access, not pointer deref).
    StateRead,
    /// Write a field to a state (typed offset access).
    StateWrite,
    /// Transform a state from one layout to another (consumes the input state).
    StateTransform,
    /// Consume a state (it's no longer usable after this — linear ownership).
    StateConsume,
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
            Capability::StateRead => write!(f, "StateRead"),
            Capability::StateWrite => write!(f, "StateWrite"),
            Capability::StateTransform => write!(f, "StateTransform"),
            Capability::StateConsume => write!(f, "StateConsume"),
        }
    }
}

// ---------------------------------------------------------------------------
// Condition
// ---------------------------------------------------------------------------

/// A runtime condition that gates the activation of one or more capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    ///
    /// # Wave 4c note
    ///
    /// `Capability::StateConsume` is intentionally **excluded** from the
    /// top element. Because `StateConsume` is exclusive (once a state is
    /// consumed, no other capability is possible — see [`CapD::join`]),
    /// it cannot coexist with the other capabilities in the lattice top.
    /// Conceptually, `StateConsume` is an *absorbing* element of the join
    /// (a "post-consumption" marker) rather than a member of the universe
    /// of cohabiting capabilities.
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
                Capability::StateRead,
                Capability::StateWrite,
                Capability::StateTransform,
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
            caps: self.caps.intersection(&other.caps).copied().collect(),
            conditions: self.conditions.union(&other.conditions).copied().collect(),
        }
    }

    /// **Join** (least upper bound) in the capability lattice.
    ///
    /// * Capabilities: union
    /// * Conditions: intersection (less restrictive)
    ///
    /// # State-capability lattice rules (Wave 4c)
    ///
    /// Two state-specific rules refine the plain set-union:
    ///
    /// 1. **`StateConsume` is exclusive** — once a state is consumed it is no
    ///    longer usable, so no other capability may coexist with
    ///    `StateConsume`. If either operand carries `StateConsume`, the join
    ///    collapses to `{StateConsume}` (plus the intersection of conditions).
    ///
    /// 2. **`StateRead` + `StateWrite` ⇒ `StateTransform`** — a state
    ///    transformation is, by definition, an operation that both reads and
    ///    writes the state. When the union of two operands contains both
    ///    `StateRead` and `StateWrite`, the join additionally materializes
    ///    `StateTransform`.
    pub fn join(&self, other: &CapD) -> CapD {
        let conditions: HashSet<Condition> = self
            .conditions
            .intersection(&other.conditions)
            .copied()
            .collect();

        // Rule 1: StateConsume is exclusive — if either side has it, the
        // result is {StateConsume} only (with the shared conditions).
        let self_has_consume = self.caps.contains(&Capability::StateConsume);
        let other_has_consume = other.caps.contains(&Capability::StateConsume);
        if self_has_consume || other_has_consume {
            return CapD {
                caps: [Capability::StateConsume].into_iter().collect(),
                conditions,
            };
        }

        // Plain union of capabilities.
        let mut caps: HashSet<Capability> =
            self.caps.union(&other.caps).copied().collect();

        // Rule 2: StateRead + StateWrite ⇒ StateTransform.
        if caps.contains(&Capability::StateRead)
            && caps.contains(&Capability::StateWrite)
        {
            caps.insert(Capability::StateTransform);
        }

        CapD { caps, conditions }
    }

    /// Resolve the effective set of capabilities given an execution [`Context`].
    ///
    /// A capability is *active* only when **all** attached conditions are
    /// satisfied by the context.  Conditions that are not relevant to the
    /// current context are conservatively assumed to be unsatisfied.
    pub fn resolve(&self, context: &Context) -> HashSet<Capability> {
        let all_conditions_active = self
            .conditions
            .iter()
            .all(|c| context.is_condition_active(c));
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
            caps: [Capability::Read, Capability::Execute]
                .into_iter()
                .collect(),
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

    #[test]
    fn all_join_idempotent() {
        // Regression for Wave 4c: CapD::all() must be idempotent under join.
        // This held before adding the exclusive StateConsume rule (which
        // would otherwise collapse all → {StateConsume} and break the
        // lattice law a ⊔ a = a). The fix is to exclude StateConsume from
        // CapD::all().
        let all = CapD::all();
        assert_eq!(all.join(&all), all);
    }

    // =======================================================================
    // New CapD tests — Wave 4c: state-access capabilities
    // =======================================================================

    #[test]
    fn state_capabilities_display() {
        assert_eq!(format!("{}", Capability::StateRead), "StateRead");
        assert_eq!(format!("{}", Capability::StateWrite), "StateWrite");
        assert_eq!(
            format!("{}", Capability::StateTransform),
            "StateTransform"
        );
        assert_eq!(
            format!("{}", Capability::StateConsume),
            "StateConsume"
        );
    }

    #[test]
    fn all_includes_state_capabilities() {
        let all = CapD::all();
        assert!(all.caps.contains(&Capability::StateRead));
        assert!(all.caps.contains(&Capability::StateWrite));
        assert!(all.caps.contains(&Capability::StateTransform));
        // StateConsume is intentionally excluded from the top element because
        // it is exclusive — it cannot coexist with other capabilities.
        assert!(
            !all.caps.contains(&Capability::StateConsume),
            "StateConsume is exclusive and must not be in CapD::all()"
        );
    }

    #[test]
    fn state_capabilities_are_distinct() {
        // The four state capabilities must be distinct enum variants.
        let caps = [
            Capability::StateRead,
            Capability::StateWrite,
            Capability::StateTransform,
            Capability::StateConsume,
        ];
        for (i, a) in caps.iter().enumerate() {
            for (j, b) in caps.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b, "state caps at indices {i} and {j} collided");
                }
            }
        }
    }

    #[test]
    fn join_state_read_and_state_write_yields_state_transform() {
        // StateRead + StateWrite ⇒ StateTransform (a transform reads & writes).
        let r = CapD::empty().strengthen(&[Capability::StateRead]);
        let w = CapD::empty().strengthen(&[Capability::StateWrite]);
        let j = r.join(&w);
        assert!(j.caps.contains(&Capability::StateRead));
        assert!(j.caps.contains(&Capability::StateWrite));
        assert!(
            j.caps.contains(&Capability::StateTransform),
            "join must materialize StateTransform when both StateRead and \
             StateWrite are present"
        );
    }

    #[test]
    fn join_state_read_alone_does_not_synthesize_transform() {
        let r1 = CapD::empty().strengthen(&[Capability::StateRead]);
        let r2 = CapD::empty().strengthen(&[Capability::StateRead]);
        let j = r1.join(&r2);
        assert!(j.caps.contains(&Capability::StateRead));
        assert!(
            !j.caps.contains(&Capability::StateTransform),
            "StateTransform must not be synthesized without StateWrite"
        );
    }

    #[test]
    fn join_state_write_alone_does_not_synthesize_transform() {
        let w1 = CapD::empty().strengthen(&[Capability::StateWrite]);
        let w2 = CapD::empty().strengthen(&[Capability::StateWrite]);
        let j = w1.join(&w2);
        assert!(j.caps.contains(&Capability::StateWrite));
        assert!(
            !j.caps.contains(&Capability::StateTransform),
            "StateTransform must not be synthesized without StateRead"
        );
    }

    #[test]
    fn join_state_consume_is_exclusive_from_self() {
        // Once consumed, nothing else is possible.
        let consume = CapD::empty().strengthen(&[Capability::StateConsume]);
        let read = CapD::empty().strengthen(&[Capability::StateRead]);
        let j = consume.join(&read);
        assert!(
            j.caps.contains(&Capability::StateConsume),
            "StateConsume must survive the join"
        );
        assert_eq!(
            j.caps.len(),
            1,
            "StateConsume is exclusive — no other capability may coexist"
        );
        assert!(!j.caps.contains(&Capability::StateRead));
        assert!(!j.caps.contains(&Capability::StateWrite));
        assert!(!j.caps.contains(&Capability::StateTransform));
    }

    #[test]
    fn join_state_consume_is_exclusive_from_other_side() {
        // Symmetry: StateConsume on the *other* operand must also collapse
        // the result to {StateConsume}.
        let read = CapD::empty().strengthen(&[Capability::StateRead]);
        let consume = CapD::empty().strengthen(&[Capability::StateConsume]);
        let j = read.join(&consume);
        assert_eq!(j.caps.len(), 1);
        assert!(j.caps.contains(&Capability::StateConsume));
        assert!(!j.caps.contains(&Capability::StateRead));
    }

    #[test]
    fn join_state_consume_drops_all_other_caps() {
        // Even when the other operand has many capabilities (including the
        // synthesized StateTransform pair), StateConsume wins outright.
        let rich = CapD::empty()
            .strengthen(&[
                Capability::StateRead,
                Capability::StateWrite,
                Capability::Read,
                Capability::Write,
                Capability::Move,
            ]);
        let consume = CapD::empty().strengthen(&[Capability::StateConsume]);
        let j = rich.join(&consume);
        assert_eq!(j.caps.len(), 1);
        assert!(j.caps.contains(&Capability::StateConsume));
    }

    #[test]
    fn join_state_consume_with_state_consume_still_exclusive() {
        let a = CapD::empty().strengthen(&[Capability::StateConsume]);
        let b = CapD::empty().strengthen(&[Capability::StateConsume]);
        let j = a.join(&b);
        assert_eq!(j.caps.len(), 1);
        assert!(j.caps.contains(&Capability::StateConsume));
    }

    #[test]
    fn join_state_transform_with_state_read_keeps_both() {
        // StateTransform already implies read+write; joining with StateRead
        // keeps StateTransform (and the explicit StateRead).
        let t = CapD::empty().strengthen(&[Capability::StateTransform]);
        let r = CapD::empty().strengthen(&[Capability::StateRead]);
        let j = t.join(&r);
        assert!(j.caps.contains(&Capability::StateTransform));
        assert!(j.caps.contains(&Capability::StateRead));
    }

    #[test]
    fn meet_state_capabilities_is_set_intersection() {
        // meet = intersection. StateRead ∩ StateWrite = ∅.
        let r = CapD::empty().strengthen(&[Capability::StateRead]);
        let w = CapD::empty().strengthen(&[Capability::StateWrite]);
        let m = r.meet(&w);
        assert!(m.caps.is_empty());
    }

    #[test]
    fn meet_state_consume_with_state_consume_keeps_consume() {
        let a = CapD::empty().strengthen(&[Capability::StateConsume]);
        let b = CapD::empty().strengthen(&[Capability::StateConsume]);
        let m = a.meet(&b);
        assert_eq!(m.caps.len(), 1);
        assert!(m.caps.contains(&Capability::StateConsume));
    }

    #[test]
    fn strengthen_with_state_capabilities() {
        let capd = CapD::empty().strengthen(&[
            Capability::StateRead,
            Capability::StateWrite,
            Capability::StateTransform,
            Capability::StateConsume,
        ]);
        assert!(capd.caps.contains(&Capability::StateRead));
        assert!(capd.caps.contains(&Capability::StateWrite));
        assert!(capd.caps.contains(&Capability::StateTransform));
        assert!(capd.caps.contains(&Capability::StateConsume));
    }

    #[test]
    fn weaken_drops_state_capabilities() {
        let capd = CapD::empty().strengthen(&[
            Capability::StateRead,
            Capability::StateWrite,
        ]);
        let weakened = capd.weaken(&[Capability::StateRead]);
        assert!(!weakened.caps.contains(&Capability::StateRead));
        assert!(weakened.caps.contains(&Capability::StateWrite));
    }

    #[test]
    fn join_state_read_write_with_other_condition_propagates_condition() {
        // The state-lattice rules must not interfere with condition
        // intersection. StateRead+StateWrite should still materialize
        // StateTransform AND carry only the shared condition.
        let r = CapD {
            caps: [Capability::StateRead].into_iter().collect(),
            conditions: [Condition::InPhase(7), Condition::RequiresLock(42)]
                .into_iter()
                .collect(),
        };
        let w = CapD {
            caps: [Capability::StateWrite].into_iter().collect(),
            conditions: [Condition::InPhase(7)].into_iter().collect(),
        };
        let j = r.join(&w);
        assert!(j.caps.contains(&Capability::StateRead));
        assert!(j.caps.contains(&Capability::StateWrite));
        assert!(j.caps.contains(&Capability::StateTransform));
        // Conditions: intersection of {InPhase(7), RequiresLock(42)} and
        // {InPhase(7)} = {InPhase(7)}.
        assert_eq!(j.conditions.len(), 1);
        assert!(j.conditions.contains(&Condition::InPhase(7)));
        assert!(!j.conditions.contains(&Condition::RequiresLock(42)));
    }

    #[test]
    fn join_state_consume_preserves_shared_conditions() {
        let a = CapD {
            caps: [Capability::StateConsume].into_iter().collect(),
            conditions: [Condition::InPhase(3), Condition::RequiresLock(1)]
                .into_iter()
                .collect(),
        };
        let b = CapD {
            caps: [Capability::StateRead, Capability::StateWrite]
                .into_iter()
                .collect(),
            conditions: [Condition::InPhase(3)].into_iter().collect(),
        };
        let j = a.join(&b);
        assert_eq!(j.caps.len(), 1);
        assert!(j.caps.contains(&Capability::StateConsume));
        // Even though StateConsume collapses the caps, the shared conditions
        // (InPhase(3)) are still carried.
        assert!(j.conditions.contains(&Condition::InPhase(3)));
        assert!(!j.conditions.contains(&Condition::RequiresLock(1)));
    }
}
