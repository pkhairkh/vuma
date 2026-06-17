//! Execution Context for Capability Resolution
//!
//! This module defines the [`Context`] struct that represents the runtime
//! execution state used to resolve conditional capabilities in a [`CapD`].
//!
//! A capability guarded by a [`Condition`] is only *active* when the current
//! context satisfies that condition.  For example, a `Condition::InPhase(3)`
//! is satisfied only when phase 3 is listed among [`Context::active_phases`].

use crate::capd::{CapD, Capability, Condition, LockId, OpId, PhaseId, RegionId, SecLevel};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// Runtime execution context used for [`CapD::resolve`].
///
/// Each field captures a dimension of the execution state that may appear
/// in a [`Condition`].  Unknown conditions are conservatively treated as
/// *not* satisfied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    /// The set of phases that are currently active.
    ///
    /// `BTreeSet` (not `HashSet`) for deterministic iteration (W35).
    pub active_phases: BTreeSet<PhaseId>,
    /// The set of operations that have already completed.
    pub completed_ops: BTreeSet<OpId>,
    /// The set of locks that are currently held.
    pub active_locks: BTreeSet<LockId>,
    /// The current security clearance level.
    pub current_security_level: SecLevel,
    /// The set of memory regions whose lifetimes are currently active.
    pub current_region: BTreeSet<RegionId>,
}

impl Context {
    /// Construct an empty context (no phases, no completed ops, no locks,
    /// security level 0, no active regions).
    pub fn empty() -> Self {
        Self {
            active_phases: BTreeSet::new(),
            completed_ops: BTreeSet::new(),
            active_locks: BTreeSet::new(),
            current_security_level: 0,
            current_region: BTreeSet::new(),
        }
    }

    /// Returns `true` when the given [`Condition`] is satisfied by this
    /// execution context.
    ///
    /// Unknown condition variants are conservatively treated as unsatisfied
    /// so that newly added conditions are safe by default.
    pub fn is_condition_active(&self, cond: &Condition) -> bool {
        match cond {
            Condition::InPhase(phase) => self.active_phases.contains(phase),
            Condition::AfterOp(op) => self.completed_ops.contains(op),
            Condition::BeforeOp(op) => !self.completed_ops.contains(op),
            Condition::NotConcurrentWith(op) => !self.completed_ops.contains(op),
            Condition::RequiresLock(lock) => self.active_locks.contains(lock),
            Condition::SecurityLevel(required) => self.current_security_level >= *required,
            Condition::ValidDuring(region) => self.current_region.contains(region),
        }
    }

    /// Resolve the effective set of capabilities from a [`CapD`] against
    /// this context.
    ///
    /// This is a convenience wrapper around [`CapD::resolve`].
    pub fn resolve_capabilities(&self, capd: &CapD) -> BTreeSet<Capability> {
        capd.resolve(self)
    }

    /// Activate a phase, returning a new context.
    pub fn with_phase(&self, phase: PhaseId) -> Self {
        let mut ctx = self.clone();
        ctx.active_phases.insert(phase);
        ctx
    }

    /// Mark an operation as completed, returning a new context.
    pub fn with_completed_op(&self, op: OpId) -> Self {
        let mut ctx = self.clone();
        ctx.completed_ops.insert(op);
        ctx
    }

    /// Acquire a lock, returning a new context.
    pub fn with_lock(&self, lock: LockId) -> Self {
        let mut ctx = self.clone();
        ctx.active_locks.insert(lock);
        ctx
    }

    /// Set the security level, returning a new context.
    pub fn with_security_level(&self, level: SecLevel) -> Self {
        let mut ctx = self.clone();
        ctx.current_security_level = level;
        ctx
    }

    /// Activate a region, returning a new context.
    pub fn with_region(&self, region: RegionId) -> Self {
        let mut ctx = self.clone();
        ctx.current_region.insert(region);
        ctx
    }
}

impl fmt::Display for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Context{{phases: {:?}, ops: {:?}, locks: {:?}, sec: {}, regions: {:?}}}",
            self.active_phases,
            self.completed_ops,
            self.active_locks,
            self.current_security_level,
            self.current_region,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_context_satisfies_nothing() {
        let ctx = Context::empty();
        assert!(!ctx.is_condition_active(&Condition::InPhase(0)));
        assert!(!ctx.is_condition_active(&Condition::AfterOp(0)));
        assert!(!ctx.is_condition_active(&Condition::RequiresLock(0)));
        assert!(!ctx.is_condition_active(&Condition::ValidDuring(0)));
    }

    #[test]
    fn with_phase_satisfies_in_phase() {
        let ctx = Context::empty().with_phase(42);
        assert!(ctx.is_condition_active(&Condition::InPhase(42)));
        assert!(!ctx.is_condition_active(&Condition::InPhase(99)));
    }

    #[test]
    fn security_level_check() {
        let ctx = Context::empty().with_security_level(3);
        assert!(ctx.is_condition_active(&Condition::SecurityLevel(2)));
        assert!(ctx.is_condition_active(&Condition::SecurityLevel(3)));
        assert!(!ctx.is_condition_active(&Condition::SecurityLevel(4)));
    }

    #[test]
    fn before_op_condition() {
        let ctx = Context::empty().with_completed_op(10);
        // BeforeOp(10) is false because op 10 has completed
        assert!(!ctx.is_condition_active(&Condition::BeforeOp(10)));
        // BeforeOp(20) is true because op 20 has not completed
        assert!(ctx.is_condition_active(&Condition::BeforeOp(20)));
    }
}
