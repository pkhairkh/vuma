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
// Trait-impl BD compatibility
// ---------------------------------------------------------------------------

/// Check trait-impl BD compatibility.
///
/// For a trait BD and an impl BD to be compatible, the impl must refine the
/// trait.  This means:
///
/// - **RepD**: impl's representation must be compatible with the trait's.
/// - **CapD**: impl must be a subset of trait (impl has ≤ capabilities than
///   trait permits).  An impl cannot grant more capabilities than the trait
///   promises.
/// - **RelD**: impl must refine trait (impl's relations are at least as
///   specific as the trait's).
///
/// # Errors
///
/// Returns a `String` describing the first incompatibility found.
///
/// # Example
///
/// ```
/// use vuma_bd::descriptor::{BD, check_trait_compatibility};
/// use vuma_bd::capd::CapD;
/// use vuma_bd::reld::RelD;
/// use vuma_bd::repd::{RepD, ByteRep};
///
/// let trait_bd = BD::new(
///     RepD::Byte(ByteRep { size: 4, align: 4 }),
///     CapD::all(),
///     RelD::empty(),
/// );
/// let impl_bd = BD::new(
///     RepD::Byte(ByteRep { size: 4, align: 4 }),
///     CapD::empty().strengthen(&[vuma_bd::capd::Capability::Read]),
///     RelD::empty(),
/// );
/// assert!(check_trait_compatibility(&trait_bd, &impl_bd).is_ok());
/// ```
pub fn check_trait_compatibility(trait_bd: &BD, impl_bd: &BD) -> Result<(), String> {
    // Check RepD compatibility
    if !impl_bd.repd.compatible(&trait_bd.repd) {
        return Err(format!(
            "RepD incompatibility: impl has {} but trait requires {}",
            impl_bd.repd, trait_bd.repd
        ));
    }

    // Check CapD: impl must be a subset of trait (impl has ≤ capabilities)
    if !impl_bd.capd.is_subset(&trait_bd.capd) {
        return Err(format!(
            "CapD violation: impl has capabilities not in trait. impl={}, trait={}",
            impl_bd.capd, trait_bd.capd
        ));
    }

    // Check RelD: impl must refine trait
    if !impl_bd.reld.refines(&trait_bd.reld) {
        return Err(format!(
            "RelD violation: impl relations do not refine trait. impl={}, trait={}",
            impl_bd.reld, trait_bd.reld
        ));
    }

    Ok(())
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

    // ----- check_trait_compatibility tests -----

    #[test]
    fn trait_compat_ok() {
        let trait_bd = BD::new(byte_rep(4, 4), CapD::all(), RelD::empty());
        let impl_bd = BD::new(byte_rep(4, 4), read_cap(), RelD::empty());
        assert!(check_trait_compatibility(&trait_bd, &impl_bd).is_ok());
    }

    #[test]
    fn trait_compat_incompatible_repd() {
        let trait_bd = BD::new(byte_rep(4, 4), CapD::all(), RelD::empty());
        let impl_bd = BD::new(byte_rep(8, 8), read_cap(), RelD::empty());
        let err = check_trait_compatibility(&trait_bd, &impl_bd).unwrap_err();
        assert!(err.contains("RepD"));
    }

    #[test]
    fn trait_compat_capd_violation() {
        let trait_bd = BD::new(byte_rep(4, 4), read_cap(), RelD::empty());
        let impl_bd = BD::new(byte_rep(4, 4), read_write_cap(), RelD::empty());
        let err = check_trait_compatibility(&trait_bd, &impl_bd).unwrap_err();
        assert!(err.contains("CapD"));
    }

    #[test]
    fn trait_compat_reld_violation() {
        use crate::reld::Relation;
        let mut trait_reld = RelD::empty();
        trait_reld.relations.insert(Relation::Liveness);
        let trait_bd = BD::new(byte_rep(4, 4), read_cap(), trait_reld);
        let impl_bd = BD::new(byte_rep(4, 4), read_cap(), RelD::empty());
        let err = check_trait_compatibility(&trait_bd, &impl_bd).unwrap_err();
        assert!(err.contains("RelD"));
    }

    #[test]
    fn trait_compat_identical() {
        let bd = BD::new(byte_rep(4, 4), read_cap(), RelD::empty());
        assert!(check_trait_compatibility(&bd, &bd).is_ok());
    }

    #[test]
    fn trait_compat_impl_subset_reld() {
        use crate::reld::{DepKind, Relation};
        let mut trait_reld = RelD::empty();
        trait_reld.relations.insert(Relation::Liveness);
        trait_reld.relations.insert(Relation::Dependency(DepKind::DataDep));
        let impl_bd = BD::new(byte_rep(4, 4), read_cap(), trait_reld.clone());
        let trait_bd = BD::new(byte_rep(4, 4), read_cap(), RelD::empty());
        // impl has more relations → refines trait
        assert!(check_trait_compatibility(&trait_bd, &impl_bd).is_ok());
    }
}
