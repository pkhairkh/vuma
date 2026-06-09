//! Synchronisation edges in the Memory State Graph.
//!
//! A [`SyncEdge`] records an ordering constraint between two memory accesses.
//! When two conflicting accesses are ordered by a synchronisation edge, they
//! are *not* in a data race. The MSG uses these edges to prune the set of
//! candidate data races.

use crate::access::AccessId;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for a synchronisation edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SyncEdgeId(pub u64);

impl fmt::Display for SyncEdgeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SE{}", self.0)
    }
}

/// Unique identifier for a lock/mutex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LockId(pub u64);

impl fmt::Display for LockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "L{}", self.0)
    }
}

/// The kind of ordering that a synchronisation edge enforces.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Ordering {
    /// `access1` happens-before `access2` (sequential program order, join, etc.).
    HappensBefore,
    /// An atomic acquire-release pair: the release on `access1` synchronises
    /// with the acquire on `access2`.
    AtomicAcquireRelease,
    /// Both accesses occur while the same mutex is held, guaranteeing mutual
    /// exclusion.
    MutexLocked(LockId),
}

impl fmt::Display for Ordering {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ordering::HappensBefore => write!(f, "happens-before"),
            Ordering::AtomicAcquireRelease => write!(f, "acq-rel"),
            Ordering::MutexLocked(lid) => write!(f, "mutex({})", lid),
        }
    }
}

/// A synchronisation edge between two accesses.
///
/// The edge direction is significant: it records that `access1` is ordered
/// *before* `access2` according to the given [`Ordering`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncEdge {
    /// Unique identifier for this edge.
    pub id: SyncEdgeId,
    /// The access that happens first.
    pub access1: AccessId,
    /// The access that happens second.
    pub access2: AccessId,
    /// The ordering constraint.
    pub ordering: Ordering,
}

impl SyncEdge {
    /// Create a new synchronisation edge.
    pub fn new(id: SyncEdgeId, access1: AccessId, access2: AccessId, ordering: Ordering) -> Self {
        Self {
            id,
            access1,
            access2,
            ordering,
        }
    }

    /// Returns `true` if this edge establishes that `a1` is ordered before `a2`.
    ///
    /// This is a directional check: the edge only orders `access1 → access2`,
    /// not the reverse.
    pub fn orders(&self, a1: AccessId, a2: AccessId) -> bool {
        self.access1 == a1 && self.access2 == a2
    }
}

impl fmt::Display for SyncEdge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SyncEdge {} {} ─[{}]─▶ {}",
            self.id, self.access1, self.ordering, self.access2,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_directional() {
        let edge = SyncEdge::new(
            SyncEdgeId(1),
            AccessId(10),
            AccessId(20),
            Ordering::HappensBefore,
        );
        assert!(edge.orders(AccessId(10), AccessId(20)));
        assert!(!edge.orders(AccessId(20), AccessId(10)));
    }

    #[test]
    fn mutex_ordering() {
        let edge = SyncEdge::new(
            SyncEdgeId(2),
            AccessId(10),
            AccessId(20),
            Ordering::MutexLocked(LockId(5)),
        );
        assert!(edge.orders(AccessId(10), AccessId(20)));
        assert_eq!(format!("{}", edge.ordering), "mutex(L5)");
    }

    #[test]
    fn display() {
        let edge = SyncEdge::new(
            SyncEdgeId(1),
            AccessId(10),
            AccessId(20),
            Ordering::AtomicAcquireRelease,
        );
        assert_eq!(
            format!("{}", edge),
            "SyncEdge SE1 A10 ─[acq-rel]─▶ A20"
        );
    }
}
