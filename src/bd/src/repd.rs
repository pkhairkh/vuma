//! Representation Descriptors (`RepD`)
//!
//! This module defines the **representation** layer of a Behavioral Descriptor.
//! A `RepD` captures the memory layout, size, alignment, and structural shape
//! of a value — independent of what capabilities or relations it participates in.
//!
//! # Layout
//!
//! | Variant   | Meaning                                          |
//! |-----------|--------------------------------------------------|
//! | `Byte`    | Raw byte sequence with explicit size/alignment   |
//! | `Struct`  | Ordered product of fields at explicit offsets    |
//! | `Array`   | Homogeneous fixed-count sequence                 |
//! | `Enum`    | Tagged union of named variants                    |
//! | `Ptr`     | Pointer to another representation                 |
//! | `Union`   | Overlapping alternatives (max size/alignment)     |
//! | `Func`    | Function signature (params → result)              |

use crate::capd::CapD;
use crate::error_reporting::BdError;
use crate::reld::RelD;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Pointer size in bytes for the 64-bit target model.
pub const POINTER_SIZE: u64 = 8;

// ---------------------------------------------------------------------------
// Leaf structures
// ---------------------------------------------------------------------------

/// Representation of a raw byte sequence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ByteRep {
    /// Size in bytes.
    pub size: u64,
    /// Required alignment in bytes.
    pub align: u64,
}

/// Representation of a struct — an ordered product of fields at fixed offsets.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StructRep {
    /// Fields as `(offset, representation)` pairs, in declaration order.
    pub fields: Vec<(u64, RepD)>,
    /// Total size in bytes (including tail padding).
    pub total_size: u64,
    /// Required alignment in bytes (max of all field alignments).
    pub align: u64,
}

/// Representation of a fixed-count homogeneous array.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArrayRep {
    /// Representation of each element.
    pub element: Box<RepD>,
    /// Number of elements.
    pub count: u64,
}

/// Representation of a tagged union (enum).
///
/// Each variant carries a discriminant tag and its own representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EnumRep {
    /// Variants as `(tag_value, variant_representation)` pairs.
    pub variants: Vec<(u64, RepD)>,
}

/// Representation of a pointer to another representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PtrRep {
    /// Representation of the pointee.
    pub pointee: Box<RepD>,
}

/// Representation of an untagged union — overlapping alternatives.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UnionRep {
    /// All alternative representations.
    pub alternatives: Vec<RepD>,
    /// Maximum size across all alternatives.
    pub max_size: u64,
    /// Maximum alignment across all alternatives.
    pub max_align: u64,
}

/// Representation of a function signature.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FuncRep {
    /// Parameter representations.
    pub params: Vec<RepD>,
    /// Result representation.
    pub result: Box<RepD>,
}

// ---------------------------------------------------------------------------
// Womb Data Model Representations
// ---------------------------------------------------------------------------

/// Representation of a Manifold — multi-dimensional spatial data laid out
/// using a space-filling curve (Z-order or Hilbert) for cache locality.
///
/// The physical memory is a 1D buffer; N-dimensional semantic coordinates
/// are translated to physical offsets via bit-interleaving.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ManifoldSpatialRep {
    /// Number of dimensions (e.g., 2 for a matrix, 3 for a volume).
    pub dimensions: u32,
    /// Size of each dimension.
    pub dim_sizes: Vec<u64>,
    /// Element size in bytes.
    pub element_size: u64,
    /// The space-filling curve used for memory layout.
    pub curve: crate::manifold::SpaceFillingCurve,
    /// The curve order (for Hilbert; log2 of max dim size).
    pub order: u32,
    /// Total buffer size in bytes.
    pub total_bytes: u64,
}

/// Representation of a Gestalt — tagless, context-dependent memory superposition.
///
/// When `degraded` is true, a hidden 1-byte runtime tag is present at
/// `tag_offset`. When false, the IVE proved that no tag is needed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GestaltSuperpositionRep {
    /// All possible variant names.
    pub variants: Vec<String>,
    /// Maximum byte size across all variants.
    pub max_size: u64,
    /// Strictest alignment across all variants.
    pub max_align: u64,
    /// If true, a hidden 1-byte runtime tag was injected by the IVE.
    pub degraded: bool,
    /// Byte offset of the injected tag (if degraded).
    pub tag_offset: Option<u64>,
}

/// Representation of a Concept — relational data with lazily-inferred layout.
///
/// The physical layout (AoS vs SoA) is resolved by the LayoutResolutionPass
/// based on access pattern analysis.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConceptRelationalRep {
    /// Field names in declaration order.
    pub field_names: Vec<String>,
    /// Resolved byte offsets for each field (empty until layout resolved).
    pub field_offsets: Vec<(String, u64)>,
    /// Total size in bytes (0 until layout resolved).
    pub total_size: u64,
    /// Alignment requirement.
    pub align: u64,
    /// Layout strategy: true = SoA, false = AoS.
    pub use_soa: bool,
}

// ---------------------------------------------------------------------------
// RepD enum
// ---------------------------------------------------------------------------

/// A constraint on a generic type parameter within a `RepD::Generic`.
///
/// Each constraint specifies a condition that any concrete type substituted
/// for the generic must satisfy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BDConstraint {
    /// The generic must have at least the given capabilities.
    CapDAtLeast(CapD),
    /// The generic's representation must be compatible with the given `RepD`.
    RepDCompatibleWith(Box<RepD>),
    /// The generic must contain the given relational constraints.
    RelDContains(RelD),
}

impl fmt::Display for BDConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BDConstraint::CapDAtLeast(capd) => write!(f, "CapDAtLeast({capd})"),
            BDConstraint::RepDCompatibleWith(repd) => write!(f, "RepDCompatibleWith({repd})"),
            BDConstraint::RelDContains(reld) => write!(f, "RelDContains({reld})"),
        }
    }
}

/// A **Representation Descriptor** — describes the memory shape of a value.
///
/// Two `RepD`s are [`RepD::compatible`] when they can safely alias the same memory,
/// and one [`RepD::subsumes`] the other when it is at least as permissive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepD {
    /// Raw byte sequence.
    Byte(ByteRep),
    /// Structured product of named fields.
    Struct(StructRep),
    /// Fixed-count homogeneous array.
    Array(ArrayRep),
    /// Tagged union (enum).
    Enum(EnumRep),
    /// Pointer to another representation.
    Ptr(PtrRep),
    /// Untagged union of overlapping alternatives.
    Union(UnionRep),
    /// Function signature.
    Func(FuncRep),
    /// Manifold — multi-dimensional spatial data with space-filling curve layout.
    ManifoldSpatial(ManifoldSpatialRep),
    /// Gestalt — tagless, context-dependent memory superposition.
    GestaltSuperposition(GestaltSuperpositionRep),
    /// Concept — relational data with lazily-inferred layout.
    ConceptRelational(ConceptRelationalRep),
    /// A generic type parameter with a name and optional BD constraints.
    Generic {
        /// Name of the type parameter (e.g., "T").
        name: String,
        /// Constraints that any concrete substitution must satisfy.
        constraints: Vec<BDConstraint>,
    },
}

impl std::hash::Hash for RepD {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Manual Hash implementation since BDConstraint contains types
        // (CapD, RelD) that don't implement Hash.
        std::mem::discriminant(self).hash(state);
        match self {
            RepD::Byte(b) => {
                b.size.hash(state);
                b.align.hash(state);
            }
            RepD::Struct(s) => {
                for (off, rep) in &s.fields {
                    off.hash(state);
                    rep.hash(state);
                }
                s.total_size.hash(state);
                s.align.hash(state);
            }
            RepD::Array(a) => {
                a.element.hash(state);
                a.count.hash(state);
            }
            RepD::Enum(e) => {
                for (tag, rep) in &e.variants {
                    tag.hash(state);
                    rep.hash(state);
                }
            }
            RepD::Ptr(p) => {
                p.pointee.hash(state);
            }
            RepD::Union(u) => {
                for alt in &u.alternatives {
                    alt.hash(state);
                }
                u.max_size.hash(state);
                u.max_align.hash(state);
            }
            RepD::Func(f) => {
                for p in &f.params {
                    p.hash(state);
                }
                f.result.hash(state);
            }
            RepD::Generic {
                name,
                constraints: _,
            } => {
                // Hash only the name; constraints contain non-Hash types.
                name.hash(state);
            }
            RepD::ManifoldSpatial(m) => {
                m.dimensions.hash(state);
                for d in &m.dim_sizes {
                    d.hash(state);
                }
                m.element_size.hash(state);
                m.total_bytes.hash(state);
            }
            RepD::GestaltSuperposition(g) => {
                for v in &g.variants {
                    v.hash(state);
                }
                g.max_size.hash(state);
                g.max_align.hash(state);
                g.degraded.hash(state);
            }
            RepD::ConceptRelational(c) => {
                for f in &c.field_names {
                    f.hash(state);
                }
                c.total_size.hash(state);
                c.align.hash(state);
                c.use_soa.hash(state);
            }
        }
    }
}

impl RepD {
    // -----------------------------------------------------------------------
    // Size & alignment queries
    // -----------------------------------------------------------------------

    /// Returns the size in bytes of this representation.
    ///
    /// For compound types this is the *padded* total size so that arrays of
    /// the type are correctly laid out.
    pub fn size(&self) -> u64 {
        match self {
            RepD::Byte(b) => b.size,
            RepD::Struct(s) => s.total_size,
            RepD::Array(a) => a.element.size() * a.count,
            RepD::Enum(e) => {
                // tag size (u64) + max variant size, aligned to 8
                let max_variant = e.variants.iter().map(|(_, v)| v.size()).max().unwrap_or(0);
                let tag_size = 8u64; // discriminant
                let aligned_variant = align_to(max_variant, 8);
                tag_size + aligned_variant
            }
            RepD::Ptr(_) => POINTER_SIZE,
            RepD::Union(u) => u.max_size,
            RepD::Func(_) => POINTER_SIZE, // function pointer
            RepD::Generic { .. } => 0,     // unknown until substituted
            RepD::ManifoldSpatial(m) => m.total_bytes,
            RepD::GestaltSuperposition(g) => g.max_size,
            RepD::ConceptRelational(c) => c.total_size,
        }
    }

    /// Returns the required alignment in bytes of this representation.
    pub fn alignment(&self) -> u64 {
        match self {
            RepD::Byte(b) => b.align,
            RepD::Struct(s) => s.align,
            RepD::Array(a) => a.element.alignment(),
            RepD::Enum(_) => 8, // discriminant alignment
            RepD::Ptr(_) => POINTER_SIZE,
            RepD::Union(u) => u.max_align,
            RepD::Func(_) => POINTER_SIZE,
            RepD::Generic { .. } => 1, // minimum alignment
            RepD::ManifoldSpatial(m) => m.element_size.max(8),
            RepD::GestaltSuperposition(g) => g.max_align,
            RepD::ConceptRelational(c) => c.align.max(1),
        }
    }

    // -----------------------------------------------------------------------
    // Compatibility & subsumption
    // -----------------------------------------------------------------------

    /// Two representations are **compatible** when they may safely alias the
    /// same memory region without introducing undefined behaviour.
    ///
    /// At a minimum, compatible representations must agree in size and
    /// alignment. Structural compatibility also requires matching shapes.
    ///
    /// A `Generic` RepD is compatible with any RepD that satisfies its
    /// constraints.
    pub fn compatible(&self, other: &RepD) -> bool {
        // Generic is compatible with anything satisfying its constraints.
        if let RepD::Generic { constraints, .. } = self {
            return generic_satisfies_constraints(constraints, other);
        }
        if let RepD::Generic { constraints, .. } = other {
            return generic_satisfies_constraints(constraints, self);
        }
        // Size and alignment must agree for safe aliasing.
        if self.size() != other.size() || self.alignment() != other.alignment() {
            return false;
        }
        // Structural check — same top-level variant or Byte catch-all.
        match (self, other) {
            (RepD::Byte(_), _) | (_, RepD::Byte(_)) => true,
            (RepD::Struct(a), RepD::Struct(b)) => {
                if a.fields.len() != b.fields.len() {
                    return false;
                }
                a.fields
                    .iter()
                    .zip(&b.fields)
                    .all(|((off_a, rep_a), (off_b, rep_b))| {
                        off_a == off_b && rep_a.compatible(rep_b)
                    })
            }
            (RepD::Array(a), RepD::Array(b)) => {
                a.count == b.count && a.element.compatible(&b.element)
            }
            (RepD::Enum(a), RepD::Enum(b)) => {
                if a.variants.len() != b.variants.len() {
                    return false;
                }
                a.variants
                    .iter()
                    .zip(&b.variants)
                    .all(|((ta, va), (tb, vb))| ta == tb && va.compatible(vb))
            }
            (RepD::Ptr(a), RepD::Ptr(b)) => a.pointee.compatible(&b.pointee),
            (RepD::Union(a), RepD::Union(b)) => {
                if a.alternatives.len() != b.alternatives.len() {
                    return false;
                }
                a.alternatives
                    .iter()
                    .zip(&b.alternatives)
                    .all(|(x, y)| x.compatible(y))
            }
            (RepD::Func(a), RepD::Func(b)) => {
                if a.params.len() != b.params.len() {
                    return false;
                }
                a.params.iter().zip(&b.params).all(|(x, y)| x.compatible(y))
                    && a.result.compatible(&b.result)
            }
            _ => false,
        }
    }

    /// `self` **subsumes** `other` when `self` is at least as permissive —
    /// i.e. any value described by `other` can be safely viewed through `self`.
    ///
    /// A `Byte` representation with matching size/alignment subsumes anything.
    /// A `Generic` subsumes any RepD satisfying its constraints.
    /// Structurally, subsumption follows the same rules as [`Self::compatible`]
    /// but is directed (not symmetric).
    pub fn subsumes(&self, other: &RepD) -> bool {
        // Byte with matching size/alignment subsumes all.
        if matches!(self, RepD::Byte(b) if b.size == other.size() && b.align == other.alignment()) {
            return true;
        }
        // Generic subsumes anything satisfying its constraints.
        if let RepD::Generic { constraints, .. } = self {
            return generic_satisfies_constraints(constraints, other);
        }
        match (self, other) {
            (RepD::Struct(a), RepD::Struct(b)) => {
                if a.fields.len() != b.fields.len() {
                    return false;
                }
                a.fields
                    .iter()
                    .zip(&b.fields)
                    .all(|((off_a, rep_a), (off_b, rep_b))| off_a == off_b && rep_a.subsumes(rep_b))
            }
            (RepD::Array(a), RepD::Array(b)) => {
                a.count == b.count && a.element.subsumes(&b.element)
            }
            (RepD::Enum(a), RepD::Enum(b)) => {
                if a.variants.len() != b.variants.len() {
                    return false;
                }
                a.variants
                    .iter()
                    .zip(&b.variants)
                    .all(|((ta, va), (tb, vb))| ta == tb && va.subsumes(vb))
            }
            (RepD::Ptr(a), RepD::Ptr(b)) => a.pointee.subsumes(&b.pointee),
            (RepD::Union(a), RepD::Union(b)) => {
                if a.alternatives.len() != b.alternatives.len() {
                    return false;
                }
                a.alternatives
                    .iter()
                    .zip(&b.alternatives)
                    .all(|(x, y)| x.subsumes(y))
            }
            (RepD::Func(a), RepD::Func(b)) => {
                if a.params.len() != b.params.len() {
                    return false;
                }
                a.params.iter().zip(&b.params).all(|(x, y)| x.subsumes(y))
                    && a.result.subsumes(&b.result)
            }
            _ => false,
        }
    }

    // -----------------------------------------------------------------------
    // Field access (Struct / Enum only)
    // -----------------------------------------------------------------------

    /// Returns the byte offset of the field at the given index (struct only).
    ///
    /// Returns `Err(BdError::InvalidOperation)` if `self` is not a `Struct`
    /// or the index is out of bounds.
    pub fn field_offset(&self, index: usize) -> Result<u64, BdError> {
        match self {
            RepD::Struct(s) => {
                s.fields
                    .get(index)
                    .map(|(off, _)| *off)
                    .ok_or_else(|| BdError::InvalidOperation {
                        operation: format!("field_offset({index})"),
                        detail: format!("index {index} out of bounds ({} fields)", s.fields.len()),
                    })
            }
            other => Err(BdError::InvalidOperation {
                operation: format!("field_offset({index})"),
                detail: format!("expected Struct, got {other}"),
            }),
        }
    }

    /// Returns a reference to the representation of the field at the given
    /// index (struct only).
    ///
    /// Returns `Err(BdError::InvalidOperation)` if `self` is not a `Struct`
    /// or the index is out of bounds.
    pub fn field_rep(&self, index: usize) -> Result<&RepD, BdError> {
        match self {
            RepD::Struct(s) => {
                s.fields
                    .get(index)
                    .map(|(_, rep)| rep)
                    .ok_or_else(|| BdError::InvalidOperation {
                        operation: format!("field_rep({index})"),
                        detail: format!("index {index} out of bounds ({} fields)", s.fields.len()),
                    })
            }
            other => Err(BdError::InvalidOperation {
                operation: format!("field_rep({index})"),
                detail: format!("expected Struct, got {other}"),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl fmt::Display for RepD {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepD::Byte(b) => write!(f, "byte(size={}, align={})", b.size, b.align),
            RepD::Struct(s) => {
                write!(f, "struct(")?;
                for (i, (off, rep)) in s.fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "@{off}: {rep}")?;
                }
                write!(f, ")")
            }
            RepD::Array(a) => write!(f, "array({}, {})", a.element, a.count),
            RepD::Enum(e) => {
                write!(f, "enum(")?;
                for (i, (tag, rep)) in e.variants.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{tag}: {rep}")?;
                }
                write!(f, ")")
            }
            RepD::Ptr(p) => write!(f, "ptr({})", p.pointee),
            RepD::Union(u) => {
                write!(f, "union(")?;
                for (i, alt) in u.alternatives.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{alt}")?;
                }
                write!(f, ")")
            }
            RepD::Func(fn_) => {
                write!(f, "func(")?;
                for (i, p) in fn_.params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {}", fn_.result)
            }
            RepD::Generic { name, constraints } => {
                write!(f, "generic({name}")?;
                for c in constraints {
                    write!(f, ": {c}")?;
                }
                write!(f, ")")
            }
            RepD::ManifoldSpatial(m) => {
                write!(f, "manifold(dims={}, {:?}, curve={:?})", m.dimensions, m.dim_sizes, m.curve)
            }
            RepD::GestaltSuperposition(g) => {
                write!(f, "gestalt({:?}, size={}, degraded={})", g.variants, g.max_size, g.degraded)
            }
            RepD::ConceptRelational(c) => {
                write!(f, "concept({:?}, size={}, soa={})", c.field_names, c.total_size, c.use_soa)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Round `val` up to the next multiple of `align`.
///
/// # Panics
///
/// Panics if `align` is zero.
fn align_to(val: u64, align: u64) -> u64 {
    assert!(align > 0, "align_to: alignment must be non-zero");
    val.div_ceil(align) * align
}

/// Check whether a concrete `RepD` satisfies the constraints of a `Generic`.
///
/// A Generic with no constraints is compatible with anything.  Otherwise,
/// each constraint must be satisfied:
/// - `CapDAtLeast(c)`: the generic's capabilities must include `c` (we cannot
///   check this at the RepD level alone, so we conservatively return `true`).
/// - `RepDCompatibleWith(r)`: the concrete RepD must be compatible with `r`.
/// - `RelDContains(r)`: the generic's relations must include `r` (conservatively
///   `true` at the RepD level).
pub fn generic_satisfies_constraints(constraints: &[BDConstraint], concrete: &RepD) -> bool {
    for constraint in constraints {
        match constraint {
            BDConstraint::CapDAtLeast(_) => {
                // Conservatively: we cannot verify CapD from RepD alone.
                // Return true so compatibility isn't blocked.
            }
            BDConstraint::RepDCompatibleWith(required) => {
                if !concrete.compatible(required) {
                    return false;
                }
            }
            BDConstraint::RelDContains(_) => {
                // Conservatively: we cannot verify RelD from RepD alone.
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_size_alignment() {
        let rep = RepD::Byte(ByteRep { size: 4, align: 4 });
        assert_eq!(rep.size(), 4);
        assert_eq!(rep.alignment(), 4);
    }

    #[test]
    fn pointer_size_constant() {
        assert_eq!(POINTER_SIZE, 8);
    }

    #[test]
    fn struct_field_access() {
        let s = RepD::Struct(StructRep {
            fields: vec![
                (0, RepD::Byte(ByteRep { size: 4, align: 4 })),
                (4, RepD::Byte(ByteRep { size: 1, align: 1 })),
            ],
            total_size: 8,
            align: 4,
        });
        assert_eq!(s.field_offset(0).unwrap(), 0);
        assert_eq!(s.field_offset(1).unwrap(), 4);
        assert_eq!(s.field_rep(1).unwrap().size(), 1);
    }

    #[test]
    fn compatible_same_struct() {
        let a = RepD::Byte(ByteRep { size: 4, align: 4 });
        let b = RepD::Byte(ByteRep { size: 4, align: 4 });
        assert!(a.compatible(&b));
    }

    #[test]
    fn incompatible_different_size() {
        let a = RepD::Byte(ByteRep { size: 4, align: 4 });
        let b = RepD::Byte(ByteRep { size: 8, align: 8 });
        assert!(!a.compatible(&b));
    }

    #[test]
    fn byte_subsumes_any() {
        let byte = RepD::Byte(ByteRep { size: 8, align: 8 });
        let ptr = RepD::Ptr(PtrRep {
            pointee: Box::new(RepD::Byte(ByteRep { size: 1, align: 1 })),
        });
        assert!(byte.subsumes(&ptr));
    }

    // =======================================================================
    // New RepD tests — Enhancement 2 (25+) + Generic tests (12+)
    // =======================================================================

    // -- Struct with nested fields --

    #[test]
    fn struct_with_nested_fields() {
        let inner = RepD::Struct(StructRep {
            fields: vec![
                (0, RepD::Byte(ByteRep { size: 4, align: 4 })),
                (4, RepD::Byte(ByteRep { size: 4, align: 4 })),
            ],
            total_size: 8,
            align: 4,
        });
        let outer = RepD::Struct(StructRep {
            fields: vec![
                (0, inner.clone()),
                (8, RepD::Byte(ByteRep { size: 1, align: 1 })),
            ],
            total_size: 12,
            align: 4,
        });
        assert_eq!(outer.size(), 12);
        assert_eq!(outer.alignment(), 4);
        assert_eq!(outer.field_offset(0).unwrap(), 0);
        assert_eq!(outer.field_offset(1).unwrap(), 8);
    }

    // -- Array with various element types --

    #[test]
    fn array_byte_elements() {
        let arr = RepD::Array(ArrayRep {
            element: Box::new(RepD::Byte(ByteRep { size: 4, align: 4 })),
            count: 10,
        });
        assert_eq!(arr.size(), 40);
        assert_eq!(arr.alignment(), 4);
    }

    #[test]
    fn array_ptr_elements() {
        let arr = RepD::Array(ArrayRep {
            element: Box::new(RepD::Ptr(PtrRep {
                pointee: Box::new(RepD::Byte(ByteRep { size: 1, align: 1 })),
            })),
            count: 5,
        });
        assert_eq!(arr.size(), 40);
        assert_eq!(arr.alignment(), 8);
    }

    #[test]
    fn array_struct_elements() {
        let elem = RepD::Struct(StructRep {
            fields: vec![
                (0, RepD::Byte(ByteRep { size: 4, align: 4 })),
                (4, RepD::Byte(ByteRep { size: 2, align: 2 })),
            ],
            total_size: 8,
            align: 4,
        });
        let arr = RepD::Array(ArrayRep {
            element: Box::new(elem),
            count: 3,
        });
        assert_eq!(arr.size(), 24);
        assert_eq!(arr.alignment(), 4);
    }

    #[test]
    fn array_zero_count() {
        let arr = RepD::Array(ArrayRep {
            element: Box::new(RepD::Byte(ByteRep { size: 4, align: 4 })),
            count: 0,
        });
        assert_eq!(arr.size(), 0);
    }

    // -- Enum with multiple variants --

    #[test]
    fn enum_multiple_variants() {
        let e = RepD::Enum(EnumRep {
            variants: vec![
                (0, RepD::Byte(ByteRep { size: 4, align: 4 })),
                (1, RepD::Byte(ByteRep { size: 8, align: 8 })),
                (2, RepD::Byte(ByteRep { size: 1, align: 1 })),
            ],
        });
        // tag(8) + align_to(max_variant=8, 8) = 8 + 8 = 16
        assert_eq!(e.size(), 16);
        assert_eq!(e.alignment(), 8);
    }

    #[test]
    fn enum_no_variants() {
        let e = RepD::Enum(EnumRep { variants: vec![] });
        // tag(8) + 0
        assert_eq!(e.size(), 8);
        assert_eq!(e.alignment(), 8);
    }

    // -- Ptr size/alignment --

    #[test]
    fn ptr_size_alignment() {
        let p = RepD::Ptr(PtrRep {
            pointee: Box::new(RepD::Byte(ByteRep {
                size: 16,
                align: 16,
            })),
        });
        assert_eq!(p.size(), 8);
        assert_eq!(p.alignment(), 8);
    }

    #[test]
    fn ptr_nested_pointee() {
        let inner = RepD::Ptr(PtrRep {
            pointee: Box::new(RepD::Byte(ByteRep { size: 1, align: 1 })),
        });
        let outer = RepD::Ptr(PtrRep {
            pointee: Box::new(inner),
        });
        assert_eq!(outer.size(), 8);
        assert_eq!(outer.alignment(), 8);
    }

    // -- Union size is max variant --

    #[test]
    fn union_size_is_max_variant() {
        let u = RepD::Union(UnionRep {
            alternatives: vec![
                RepD::Byte(ByteRep { size: 4, align: 4 }),
                RepD::Byte(ByteRep { size: 16, align: 8 }),
                RepD::Byte(ByteRep { size: 2, align: 2 }),
            ],
            max_size: 16,
            max_align: 8,
        });
        assert_eq!(u.size(), 16);
        assert_eq!(u.alignment(), 8);
    }

    #[test]
    fn union_single_alternative() {
        let u = RepD::Union(UnionRep {
            alternatives: vec![RepD::Byte(ByteRep { size: 4, align: 4 })],
            max_size: 4,
            max_align: 4,
        });
        assert_eq!(u.size(), 4);
        assert_eq!(u.alignment(), 4);
    }

    // -- Func size/alignment --

    #[test]
    fn func_size_alignment() {
        let f = RepD::Func(FuncRep {
            params: vec![
                RepD::Byte(ByteRep { size: 4, align: 4 }),
                RepD::Byte(ByteRep { size: 8, align: 8 }),
            ],
            result: Box::new(RepD::Byte(ByteRep { size: 4, align: 4 })),
        });
        assert_eq!(f.size(), 8); // function pointer
        assert_eq!(f.alignment(), 8);
    }

    #[test]
    fn func_no_params() {
        let f = RepD::Func(FuncRep {
            params: vec![],
            result: Box::new(RepD::Byte(ByteRep { size: 0, align: 1 })),
        });
        assert_eq!(f.size(), 8);
    }

    // -- Zero-size types --

    #[test]
    fn zero_size_byte() {
        let z = RepD::Byte(ByteRep { size: 0, align: 1 });
        assert_eq!(z.size(), 0);
        assert_eq!(z.alignment(), 1);
    }

    #[test]
    fn zero_size_struct() {
        let z = RepD::Struct(StructRep {
            fields: vec![],
            total_size: 0,
            align: 1,
        });
        assert_eq!(z.size(), 0);
        assert_eq!(z.alignment(), 1);
    }

    // -- Max alignment --

    #[test]
    fn max_alignment_128() {
        let s = RepD::Struct(StructRep {
            fields: vec![
                (
                    0,
                    RepD::Byte(ByteRep {
                        size: 16,
                        align: 16,
                    }),
                ),
                (16, RepD::Byte(ByteRep { size: 4, align: 4 })),
            ],
            total_size: 32,
            align: 16,
        });
        assert_eq!(s.alignment(), 16);
    }

    // -- Deeply nested structs --

    #[test]
    fn deeply_nested_structs() {
        let leaf = RepD::Byte(ByteRep { size: 1, align: 1 });
        let l1 = RepD::Struct(StructRep {
            fields: vec![(0, leaf)],
            total_size: 1,
            align: 1,
        });
        let l2 = RepD::Struct(StructRep {
            fields: vec![(0, l1)],
            total_size: 1,
            align: 1,
        });
        let l3 = RepD::Struct(StructRep {
            fields: vec![(0, l2)],
            total_size: 1,
            align: 1,
        });
        assert_eq!(l3.size(), 1);
        assert_eq!(l3.alignment(), 1);
    }

    // -- RepD serialization round-trip --

    #[test]
    fn byte_serde_roundtrip() {
        let rep = RepD::Byte(ByteRep { size: 8, align: 8 });
        let json = serde_json::to_string(&rep).unwrap();
        let back: RepD = serde_json::from_str(&json).unwrap();
        assert_eq!(rep, back);
    }

    #[test]
    fn struct_serde_roundtrip() {
        let rep = RepD::Struct(StructRep {
            fields: vec![
                (0, RepD::Byte(ByteRep { size: 4, align: 4 })),
                (
                    4,
                    RepD::Ptr(PtrRep {
                        pointee: Box::new(RepD::Byte(ByteRep { size: 1, align: 1 })),
                    }),
                ),
            ],
            total_size: 12,
            align: 8,
        });
        let json = serde_json::to_string(&rep).unwrap();
        let back: RepD = serde_json::from_str(&json).unwrap();
        assert_eq!(rep, back);
    }

    #[test]
    fn enum_serde_roundtrip() {
        let rep = RepD::Enum(EnumRep {
            variants: vec![
                (0, RepD::Byte(ByteRep { size: 4, align: 4 })),
                (1, RepD::Byte(ByteRep { size: 8, align: 8 })),
            ],
        });
        let json = serde_json::to_string(&rep).unwrap();
        let back: RepD = serde_json::from_str(&json).unwrap();
        assert_eq!(rep, back);
    }

    #[test]
    fn func_serde_roundtrip() {
        let rep = RepD::Func(FuncRep {
            params: vec![RepD::Byte(ByteRep { size: 8, align: 8 })],
            result: Box::new(RepD::Byte(ByteRep { size: 4, align: 4 })),
        });
        let json = serde_json::to_string(&rep).unwrap();
        let back: RepD = serde_json::from_str(&json).unwrap();
        assert_eq!(rep, back);
    }

    #[test]
    fn union_serde_roundtrip() {
        let rep = RepD::Union(UnionRep {
            alternatives: vec![
                RepD::Byte(ByteRep { size: 4, align: 4 }),
                RepD::Byte(ByteRep { size: 8, align: 8 }),
            ],
            max_size: 8,
            max_align: 8,
        });
        let json = serde_json::to_string(&rep).unwrap();
        let back: RepD = serde_json::from_str(&json).unwrap();
        assert_eq!(rep, back);
    }

    #[test]
    fn array_serde_roundtrip() {
        let rep = RepD::Array(ArrayRep {
            element: Box::new(RepD::Byte(ByteRep { size: 4, align: 4 })),
            count: 100,
        });
        let json = serde_json::to_string(&rep).unwrap();
        let back: RepD = serde_json::from_str(&json).unwrap();
        assert_eq!(rep, back);
    }

    #[test]
    fn ptr_serde_roundtrip() {
        let rep = RepD::Ptr(PtrRep {
            pointee: Box::new(RepD::Array(ArrayRep {
                element: Box::new(RepD::Byte(ByteRep { size: 1, align: 1 })),
                count: 10,
            })),
        });
        let json = serde_json::to_string(&rep).unwrap();
        let back: RepD = serde_json::from_str(&json).unwrap();
        assert_eq!(rep, back);
    }

    // -- Compatible/incompatible struct pairs --

    #[test]
    fn compatible_same_struct_layout() {
        let a = RepD::Struct(StructRep {
            fields: vec![
                (0, RepD::Byte(ByteRep { size: 4, align: 4 })),
                (4, RepD::Byte(ByteRep { size: 4, align: 4 })),
            ],
            total_size: 8,
            align: 4,
        });
        let b = RepD::Struct(StructRep {
            fields: vec![
                (0, RepD::Byte(ByteRep { size: 4, align: 4 })),
                (4, RepD::Byte(ByteRep { size: 4, align: 4 })),
            ],
            total_size: 8,
            align: 4,
        });
        assert!(a.compatible(&b));
    }

    #[test]
    fn incompatible_struct_field_offset_mismatch() {
        let a = RepD::Struct(StructRep {
            fields: vec![
                (0, RepD::Byte(ByteRep { size: 4, align: 4 })),
                (4, RepD::Byte(ByteRep { size: 4, align: 4 })),
            ],
            total_size: 8,
            align: 4,
        });
        let b = RepD::Struct(StructRep {
            fields: vec![
                (0, RepD::Byte(ByteRep { size: 4, align: 4 })),
                (8, RepD::Byte(ByteRep { size: 4, align: 4 })),
            ],
            total_size: 12,
            align: 4,
        });
        assert!(!a.compatible(&b));
    }

    // -- Generic tests (12+) --

    #[test]
    fn generic_no_constraints_compatible_with_anything() {
        let g = RepD::Generic {
            name: "T".to_string(),
            constraints: vec![],
        };
        let byte = RepD::Byte(ByteRep { size: 4, align: 4 });
        let ptr = RepD::Ptr(PtrRep {
            pointee: Box::new(byte.clone()),
        });
        assert!(g.compatible(&byte));
        assert!(byte.compatible(&g));
        assert!(g.compatible(&ptr));
        assert!(ptr.compatible(&g));
    }

    #[test]
    fn generic_size_and_alignment() {
        let g = RepD::Generic {
            name: "T".to_string(),
            constraints: vec![],
        };
        assert_eq!(g.size(), 0);
        assert_eq!(g.alignment(), 1);
    }

    #[test]
    fn generic_subsumes_concrete() {
        let g = RepD::Generic {
            name: "T".to_string(),
            constraints: vec![],
        };
        let byte = RepD::Byte(ByteRep { size: 4, align: 4 });
        assert!(g.subsumes(&byte));
    }

    #[test]
    fn generic_with_repd_compatible_constraint_passes() {
        let constraint =
            BDConstraint::RepDCompatibleWith(Box::new(RepD::Byte(ByteRep { size: 4, align: 4 })));
        let g = RepD::Generic {
            name: "T".to_string(),
            constraints: vec![constraint],
        };
        let byte4 = RepD::Byte(ByteRep { size: 4, align: 4 });
        assert!(g.compatible(&byte4));
    }

    #[test]
    fn generic_with_repd_compatible_constraint_fails() {
        let constraint =
            BDConstraint::RepDCompatibleWith(Box::new(RepD::Byte(ByteRep { size: 4, align: 4 })));
        let g = RepD::Generic {
            name: "T".to_string(),
            constraints: vec![constraint],
        };
        let byte8 = RepD::Byte(ByteRep { size: 8, align: 8 });
        assert!(!g.compatible(&byte8));
    }

    #[test]
    fn generic_with_capd_constraint_conservatively_passes() {
        let capd = CapD::empty().strengthen(&[crate::capd::Capability::Read]);
        let constraint = BDConstraint::CapDAtLeast(capd);
        let g = RepD::Generic {
            name: "T".to_string(),
            constraints: vec![constraint],
        };
        let byte = RepD::Byte(ByteRep { size: 4, align: 4 });
        // CapDAtLeast is conservatively accepted at the RepD level
        assert!(g.compatible(&byte));
    }

    #[test]
    fn generic_with_reld_constraint_conservatively_passes() {
        let reld = RelD::empty();
        let constraint = BDConstraint::RelDContains(reld);
        let g = RepD::Generic {
            name: "T".to_string(),
            constraints: vec![constraint],
        };
        let byte = RepD::Byte(ByteRep { size: 4, align: 4 });
        // RelDContains is conservatively accepted at the RepD level
        assert!(g.compatible(&byte));
    }

    #[test]
    fn generic_display() {
        let g = RepD::Generic {
            name: "T".to_string(),
            constraints: vec![],
        };
        assert_eq!(format!("{g}"), "generic(T)");
    }

    #[test]
    fn generic_with_constraints_display() {
        let constraint =
            BDConstraint::RepDCompatibleWith(Box::new(RepD::Byte(ByteRep { size: 4, align: 4 })));
        let g = RepD::Generic {
            name: "T".to_string(),
            constraints: vec![constraint],
        };
        let displayed = format!("{g}");
        assert!(displayed.starts_with("generic(T"));
        assert!(displayed.contains("RepDCompatibleWith"));
    }

    #[test]
    fn generic_serde_roundtrip() {
        let g = RepD::Generic {
            name: "T".to_string(),
            constraints: vec![],
        };
        let json = serde_json::to_string(&g).unwrap();
        let back: RepD = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn generic_with_constraint_serde_roundtrip() {
        let g = RepD::Generic {
            name: "U".to_string(),
            constraints: vec![BDConstraint::RepDCompatibleWith(Box::new(RepD::Byte(
                ByteRep { size: 8, align: 8 },
            )))],
        };
        let json = serde_json::to_string(&g).unwrap();
        let back: RepD = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn generic_two_generics_compatible() {
        let g1 = RepD::Generic {
            name: "T".to_string(),
            constraints: vec![],
        };
        let g2 = RepD::Generic {
            name: "U".to_string(),
            constraints: vec![],
        };
        assert!(g1.compatible(&g2));
    }

    #[test]
    fn generic_multiple_constraints() {
        let constraints = vec![
            BDConstraint::CapDAtLeast(CapD::empty()),
            BDConstraint::RepDCompatibleWith(Box::new(RepD::Byte(ByteRep { size: 4, align: 4 }))),
            BDConstraint::RelDContains(RelD::empty()),
        ];
        let g = RepD::Generic {
            name: "T".to_string(),
            constraints,
        };
        let byte = RepD::Byte(ByteRep { size: 4, align: 4 });
        assert!(g.compatible(&byte));
        let byte8 = RepD::Byte(ByteRep { size: 8, align: 8 });
        assert!(!g.compatible(&byte8));
    }

    #[test]
    fn bdconstraint_display_capd() {
        let capd = CapD::empty().strengthen(&[crate::capd::Capability::Read]);
        let c = BDConstraint::CapDAtLeast(capd);
        let displayed = format!("{c}");
        assert!(displayed.starts_with("CapDAtLeast("));
    }

    #[test]
    fn bdconstraint_display_reld() {
        let c = BDConstraint::RelDContains(RelD::empty());
        let displayed = format!("{c}");
        assert!(displayed.starts_with("RelDContains("));
    }

    // -- Error path tests for field_offset / field_rep --

    #[test]
    fn field_offset_returns_error_for_non_struct() {
        let byte = RepD::Byte(ByteRep { size: 4, align: 4 });
        let result = byte.field_offset(0);
        assert!(result.is_err(), "field_offset on Byte should return Err");
    }

    #[test]
    fn field_rep_returns_error_for_out_of_bounds() {
        let s = RepD::Struct(StructRep {
            fields: vec![
                (0, RepD::Byte(ByteRep { size: 4, align: 4 })),
                (4, RepD::Byte(ByteRep { size: 1, align: 1 })),
            ],
            total_size: 8,
            align: 4,
        });
        let result = s.field_rep(999);
        assert!(
            result.is_err(),
            "field_rep with out-of-bounds index should return Err"
        );
    }

    #[test]
    fn repd_generic_creation() {
        let g = RepD::Generic {
            name: "T".to_string(),
            constraints: vec![],
        };
        assert_eq!(g.size(), 0); // unknown until substituted
        assert_eq!(g.alignment(), 1); // minimum alignment
        if let RepD::Generic { name, constraints } = &g {
            assert_eq!(name, "T");
            assert!(constraints.is_empty());
        } else {
            panic!("expected Generic variant");
        }
    }
}
