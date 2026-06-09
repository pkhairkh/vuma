//! Full Behavioral Descriptor (`BD`)
//!
//! This module defines the top-level **Behavioral Descriptor** — a triple
//! `(RepD, CapD, RelD)` that fully characterises what a value *looks like*,
//! *what may be done with it*, and *what relationships it participates in*.
//!
//! # Structure
//!
//! ```text
//! BD = RepD × CapD × RelD
//! ```
//!
//! Two BDs are **compatible** when all three layers are pairwise compatible.
//! One BD **refines** another when every layer is at least as specific.

use crate::capd::CapD;
use crate::reld::RelD;
use crate::repd::RepD;
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// BDId
// ---------------------------------------------------------------------------

/// Opaque identifier for a [`BD`] instance.
///
/// Used as a key in registries and during composition so that structural
/// equality is not needed to identify descriptors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BDId(pub u64);

impl fmt::Display for BDId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BD#{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// BD
// ---------------------------------------------------------------------------

/// A **Behavioral Descriptor** — the complete specification of a value's
/// representation, capabilities, and relations.
///
/// # Ordering
///
/// `BD` supports a refinement ordering:
///
/// ```text
/// bd1 ⊑ bd2  ⟺  bd1.repd.subsumes(bd2.repd)
///                ∧ bd1.capd.is_subset(bd2.capd)
///                ∧ bd1.reld.refines(bd2.reld)
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BD {
    /// Representation descriptor — memory shape.
    pub repd: RepD,
    /// Capability descriptor — permitted operations.
    pub capd: CapD,
    /// Relational descriptor — relationships.
    pub reld: RelD,
}

impl BD {
    /// Construct a new `BD` from its three layers.
    pub fn new(repd: RepD, capd: CapD, reld: RelD) -> Self {
        Self { repd, capd, reld }
    }

    // -----------------------------------------------------------------------
    // Compatibility
    // -----------------------------------------------------------------------

    /// Two BDs are **compatible** when they can safely describe the same
    /// value:
    ///
    /// * representations are [`RepD::compatible`],
    /// * capabilities have a non-empty meet, and
    /// * relations are [`RelD::is_consistent`].
    pub fn compatible(&self, other: &BD) -> bool {
        self.repd.compatible(&other.repd)
            && !self.capd.meet(&other.capd).caps.is_empty()
            && self.reld.compose(&other.reld).is_consistent()
    }

    // -----------------------------------------------------------------------
    // Refinement
    // -----------------------------------------------------------------------

    /// `self` **refines** `other` when `self` is at least as specific in
    /// every layer.
    pub fn refines(&self, other: &BD) -> bool {
        self.repd.subsumes(&other.repd)
            && self.capd.is_subset(&other.capd)
            && self.reld.refines(&other.reld)
    }

    // -----------------------------------------------------------------------
    // Composition
    // -----------------------------------------------------------------------

    /// **Compose** two BDs by combining each layer independently:
    ///
    /// * `repd`: must be compatible (uses `self`'s repd as the result).
    /// * `capd`: meet (intersection of capabilities).
    /// * `reld`: compose (union of relations).
    ///
    /// Returns `None` if the representations are incompatible.
    pub fn compose(&self, other: &BD) -> Option<BD> {
        if !self.repd.compatible(&other.repd) {
            return None;
        }
        Some(BD {
            repd: self.repd.clone(),
            capd: self.capd.meet(&other.capd),
            reld: self.reld.compose(&other.reld),
        })
    }
}

// ---------------------------------------------------------------------------
// Display — projection
// ---------------------------------------------------------------------------

impl fmt::Display for BD {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "BD {{")?;
        writeln!(f, "  repd: {}", self.repd)?;
        writeln!(f, "  capd: {}", self.capd)?;
        writeln!(f, "  reld: {}", self.reld)?;
        write!(f, "}}")
    }
}

#[cfg(test)]
mod tests {
    use crate::capd::{CapD, Capability};
    use crate::reld::{RelD, Relation};
    use crate::repd::{ByteRep, RepD};
    use hashbrown::HashSet;

    use super::*;

    fn byte_rep(size: u64, align: u64) -> RepD {
        RepD::Byte(ByteRep { size, align })
    }

    fn read_cap() -> CapD {
        let mut caps = HashSet::new();
        caps.insert(Capability::Read);
        CapD {
            caps,
            conditions: HashSet::new(),
        }
    }

    fn read_write_cap() -> CapD {
        let mut caps = HashSet::new();
        caps.insert(Capability::Read);
        caps.insert(Capability::Write);
        CapD {
            caps,
            conditions: HashSet::new(),
        }
    }

    fn liveness_reld() -> RelD {
        RelD {
            relations: [Relation::Liveness].into_iter().collect(),
        }
    }

    #[test]
    fn bd_new() {
        let bd = BD::new(byte_rep(4, 4), read_cap(), RelD::empty());
        assert_eq!(bd.repd.size(), 4);
    }

    #[test]
    fn bd_compatible_same() {
        let a = BD::new(byte_rep(8, 8), read_cap(), RelD::empty());
        let b = BD::new(byte_rep(8, 8), read_write_cap(), RelD::empty());
        assert!(a.compatible(&b));
    }

    #[test]
    fn bd_incompatible_different_size() {
        let a = BD::new(byte_rep(4, 4), read_cap(), RelD::empty());
        let b = BD::new(byte_rep(8, 8), read_cap(), RelD::empty());
        assert!(!a.compatible(&b));
    }

    #[test]
    fn bd_refines() {
        // a refines b when a has ⊆ caps AND ⊇ relations.
        // Both use same repd, same reld, but a has fewer caps → a ⊑ b.
        let a = BD::new(byte_rep(4, 4), read_cap(), liveness_reld());
        let b = BD::new(byte_rep(4, 4), read_write_cap(), liveness_reld());
        assert!(a.refines(&b));
        // b does not refine a because b has more caps.
        assert!(!b.refines(&a));
    }

    #[test]
    fn bd_compose() {
        let a = BD::new(byte_rep(4, 4), read_write_cap(), RelD::empty());
        let b = BD::new(byte_rep(4, 4), read_cap(), liveness_reld());
        let composed = a.compose(&b).expect("compatible BDs should compose");
        // meet of caps: Read only
        assert!(composed.capd.caps.contains(&Capability::Read));
        assert!(!composed.capd.caps.contains(&Capability::Write));
        // compose of relds: Liveness
        assert!(composed.reld.relations.contains(&Relation::Liveness));
    }

    #[test]
    fn bd_compose_incompatible_returns_none() {
        let a = BD::new(byte_rep(4, 4), read_cap(), RelD::empty());
        let b = BD::new(byte_rep(8, 8), read_cap(), RelD::empty());
        assert!(a.compose(&b).is_none());
    }

    #[test]
    fn bd_id_display() {
        let id = BDId(42);
        assert_eq!(format!("{id}"), "BD#42");
    }
}
