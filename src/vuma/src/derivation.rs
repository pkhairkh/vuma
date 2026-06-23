//! Pointer derivation tracking.
//!
//! In VUMA every pointer value is the result of a *derivation* — an
//! operation that produces a new pointer from either a region base or
//! another existing pointer. The [`Derivation`] type records the provenance
//! chain, enabling the system to decide whether a particular pointer access
//! is within bounds.

use crate::address::Address;
use crate::region::RegionId;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for a derivation step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DerivationId(pub u64);

impl fmt::Display for DerivationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "D{}", self.0)
    }
}

/// What a derivation starts from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DerivationSource {
    /// Directly from a region base address (e.g. `&x`).
    Region(RegionId),
    /// Derived from another pointer derivation (e.g. pointer arithmetic).
    AnotherDerivation(DerivationId),
}

/// A representation descriptor for type-level casts.
///
/// `RepD` captures the low-level representation of the type being cast
/// from/to (e.g. `*mut u8` vs `*mut u32`). This is kept intentionally
/// lightweight; the front-end is free to encode whatever it needs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepD {
    /// A human-readable name for the representation (e.g. `"*mut u8"`).
    pub name: String,
    /// Size in bytes of the pointed-to type.
    pub size: u64,
}

/// An arithmetic expression used in [`DerivationKind::Arithmetic`].
///
/// This is a simple expression tree that records pointer arithmetic
/// such as `ptr + n * stride`. It is intentionally minimal — the goal
/// is to record enough information to reconstruct bounds, not to serve
/// as a general expression evaluator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DerivationExpr {
    /// A constant offset (in bytes).
    Constant(i64),
    /// Offset by a variable amount times a stride.
    Scaled { factor: i64, stride: u64 },
    /// Sum of two expressions.
    Add(Box<DerivationExpr>, Box<DerivationExpr>),
    /// Difference of two expressions.
    Sub(Box<DerivationExpr>, Box<DerivationExpr>),
}

/// The kind of derivation operation performed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DerivationKind {
    /// Taking the address of a value: `&x`.
    Direct,
    /// Offset from a base pointer: `ptr.offset(n)`.
    Offset {
        /// Number of bytes to offset.
        by: i64,
    },
    /// Type cast from one representation to another: `ptr as *mut T`.
    Cast {
        /// Source representation.
        from: RepD,
        /// Target representation.
        to: RepD,
    },
    /// General pointer arithmetic.
    Arithmetic {
        /// The arithmetic expression describing the offset.
        expr: DerivationExpr,
    },
}

impl fmt::Display for DerivationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DerivationKind::Direct => write!(f, "direct"),
            DerivationKind::Offset { by } => write!(f, "offset({:+})", by),
            DerivationKind::Cast { from, to } => write!(f, "cast({} → {})", from.name, to.name),
            DerivationKind::Arithmetic { .. } => write!(f, "arithmetic"),
        }
    }
}

/// A single derivation step in the pointer provenance chain.
///
/// Each derivation records:
/// - its unique identifier,
/// - what it was derived from ([`DerivationSource`]),
/// - what kind of derivation was performed ([`DerivationKind`]),
/// - the provenance range `[proven_range.0, proven_range.1)` — the address
///   range that this derivation is *allowed* to access.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Derivation {
    /// Unique identifier for this derivation.
    pub id: DerivationId,
    /// Where this derivation starts from.
    pub source: DerivationSource,
    /// What kind of derivation was performed.
    pub kind: DerivationKind,
    /// The provenance range `[lo, hi)` — addresses this derivation may access.
    pub proven_range: (Address, Address),
}

impl Derivation {
    /// Trace the full derivation chain back to the originating region.
    ///
    /// Given a function that resolves a [`DerivationId`] to its
    /// [`Derivation`], this walks the chain from `self` back to the root,
    /// returning the chain in order `[root, ..., parent, self]`.
    pub fn trace<F>(&self, lookup: F) -> Vec<Derivation>
    where
        F: Fn(DerivationId) -> Option<Derivation>,
    {
        let mut chain = vec![self.clone()];
        let mut current = &self.source;
        loop {
            match current {
                DerivationSource::Region(_) => break,
                DerivationSource::AnotherDerivation(parent_id) => {
                    match lookup(*parent_id) {
                        Some(parent) => {
                            chain.push(parent.clone());
                            current = &chain.last().unwrap().source;
                        }
                        None => break, // broken chain — bail out
                    }
                }
            }
        }
        chain.reverse();
        chain
    }

    /// Walk to the base [`RegionId`] of this derivation.
    ///
    /// Returns `None` if the chain is broken (a parent derivation is missing).
    pub fn base_region<F>(&self, lookup: F) -> Option<RegionId>
    where
        F: Fn(DerivationId) -> Option<Derivation>,
    {
        let mut current_source = self.source.clone();
        loop {
            match current_source {
                DerivationSource::Region(rid) => return Some(rid),
                DerivationSource::AnotherDerivation(parent_id) => match lookup(parent_id) {
                    Some(parent) => current_source = parent.source,
                    None => return None,
                },
            }
        }
    }

    /// Returns `true` if the provenance range of this derivation is non-empty
    /// and well-formed (i.e. `lo < hi`).
    pub fn is_within_bounds(&self) -> bool {
        self.proven_range.0 < self.proven_range.1
    }
}

impl fmt::Display for Derivation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Deriv {} source={:?} kind={} proven=[{}, {})",
            self.id, self.source, self.kind, self.proven_range.0, self.proven_range.1,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_region_derivation(id: u64, rid: u64, kind: DerivationKind) -> Derivation {
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::Region(RegionId(rid)),
            kind,
            proven_range: (Address::from(0x1000_u64), Address::from(0x2000_u64)),
        }
    }

    fn make_chained_derivation(id: u64, parent: DerivationId, kind: DerivationKind) -> Derivation {
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::AnotherDerivation(parent),
            kind,
            proven_range: (Address::from(0x1040_u64), Address::from(0x1080_u64)),
        }
    }

    #[test]
    fn base_region_direct() {
        let d = make_region_derivation(1, 42, DerivationKind::Direct);
        assert_eq!(d.base_region(|_| None), Some(RegionId(42)));
    }

    #[test]
    fn base_region_chain() {
        let d1 = make_region_derivation(1, 10, DerivationKind::Direct);
        let d2 = make_chained_derivation(2, DerivationId(1), DerivationKind::Offset { by: 0x40 });
        let d3 = make_chained_derivation(3, DerivationId(2), DerivationKind::Offset { by: 0x10 });

        let lookup = |id: DerivationId| match id.0 {
            1 => Some(d1.clone()),
            2 => Some(d2.clone()),
            _ => None,
        };

        assert_eq!(d3.base_region(&lookup), Some(RegionId(10)));
    }

    #[test]
    fn trace_chain() {
        let d1 = make_region_derivation(1, 10, DerivationKind::Direct);
        let d2 = make_chained_derivation(2, DerivationId(1), DerivationKind::Offset { by: 0x40 });

        let lookup = |id: DerivationId| match id.0 {
            1 => Some(d1.clone()),
            _ => None,
        };

        let chain = d2.trace(&lookup);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].id, DerivationId(1));
        assert_eq!(chain[1].id, DerivationId(2));
    }

    #[test]
    fn is_within_bounds() {
        let d = Derivation {
            id: DerivationId(1),
            source: DerivationSource::Region(RegionId(1)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(0x1000_u64), Address::from(0x2000_u64)),
        };
        assert!(d.is_within_bounds());

        let bad = Derivation {
            id: DerivationId(2),
            source: DerivationSource::Region(RegionId(1)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(0x2000_u64), Address::from(0x1000_u64)),
        };
        assert!(!bad.is_within_bounds());
    }
}
