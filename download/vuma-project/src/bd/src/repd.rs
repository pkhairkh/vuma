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
// RepD enum
// ---------------------------------------------------------------------------

/// A **Representation Descriptor** — describes the memory shape of a value.
///
/// Two `RepD`s are [`compatible`] when they can safely alias the same memory,
/// and one [`subsumes`] the other when it is at least as permissive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
                let max_variant = e
                    .variants
                    .iter()
                    .map(|(_, v)| v.size())
                    .max()
                    .unwrap_or(0);
                let tag_size = 8u64; // discriminant
                let aligned_variant = align_to(max_variant, 8);
                tag_size + aligned_variant
            }
            RepD::Ptr(_) => POINTER_SIZE,
            RepD::Union(u) => u.max_size,
            RepD::Func(_) => POINTER_SIZE, // function pointer
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
    pub fn compatible(&self, other: &RepD) -> bool {
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
    /// Structurally, subsumption follows the same rules as [`compatible`]
    /// but is directed (not symmetric).
    pub fn subsumes(&self, other: &RepD) -> bool {
        // Byte with matching size/alignment subsumes all.
        if matches!(self, RepD::Byte(b) if b.size == other.size() && b.align == other.alignment())
        {
            return true;
        }
        match (self, other) {
            (RepD::Struct(a), RepD::Struct(b)) => {
                if a.fields.len() != b.fields.len() {
                    return false;
                }
                a.fields
                    .iter()
                    .zip(&b.fields)
                    .all(|((off_a, rep_a), (off_b, rep_b))| {
                        off_a == off_b && rep_a.subsumes(rep_b)
                    })
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
    /// # Panics
    ///
    /// Panics if `self` is not a `Struct` or the index is out of bounds.
    pub fn field_offset(&self, index: usize) -> u64 {
        match self {
            RepD::Struct(s) => s
                .fields
                .get(index)
                .map(|(off, _)| *off)
                .unwrap_or_else(|| panic!("field_offset: index {index} out of bounds")),
            other => panic!("field_offset: expected Struct, got {other}"),
        }
    }

    /// Returns a reference to the representation of the field at the given
    /// index (struct only).
    ///
    /// # Panics
    ///
    /// Panics if `self` is not a `Struct` or the index is out of bounds.
    pub fn field_rep(&self, index: usize) -> &RepD {
        match self {
            RepD::Struct(s) => s
                .fields
                .get(index)
                .map(|(_, rep)| rep)
                .unwrap_or_else(|| panic!("field_rep: index {index} out of bounds")),
            other => panic!("field_rep: expected Struct, got {other}"),
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
    (val + align - 1) / align * align
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
        assert_eq!(s.field_offset(0), 0);
        assert_eq!(s.field_offset(1), 4);
        assert_eq!(s.field_rep(1).size(), 1);
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
}
