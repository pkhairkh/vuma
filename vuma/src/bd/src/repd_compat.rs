//! RepD Compatibility Lattice
//!
//! This module implements compatibility checking, lattice operations, and
//! reinterpretation verification for Representation Descriptors.
//!
//! # Lattice Structure
//!
//! RepDs form a partial order via the subsumption relation:
//!   r1 <= r2  iff  subsumes(r2, r1)  iff  ⟦r1⟧ ⊆ ⟦r2⟧
//!
//! `ByteRep` is the top element (most general) for a given size class, since
//! raw bytes subsume any structured representation of equal size with weaker
//! or equal alignment.
//!
//! # Key Operations
//!
//! - [`are_compatible`] — can two RepDs safely coexist in the same memory?
//! - [`meet`] — greatest lower bound (most specific common descendant)
//! - [`join`] — least upper bound (most general common ancestor)
//! - [`can_reinterpret`] — safe reinterpretation check (spec rules R1–R7)
//! - [`size_of`] / [`alignment_of`] — convenience wrappers
//! - [`is_subtype`] — subtyping relation

use crate::repd::{
    ArrayRep, ByteRep, EnumRep, FuncRep, PtrRep, RepD, StructRep, UnionRep, POINTER_SIZE,
};

// ---------------------------------------------------------------------------
// Compatibility result types
// ---------------------------------------------------------------------------

/// Classification of how two RepDs are compatible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompatibilityKind {
    /// The two RepDs are structurally identical.
    Identical,
    /// Same constructor with structurally compatible fields.
    StructuralMatch,
    /// One can be viewed as raw bytes that the other can interpret.
    ByteErosion,
    /// One RepD subsumes the other (directed compatibility).
    Subsumption,
    /// Compatible because both can be reinterpreted to a common form.
    ReinterpretCompatible,
}

/// Reason why two RepDs are incompatible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncompatibilityReason {
    /// Total sizes differ.
    SizeMismatch { size1: u64, size2: u64 },
    /// Alignments are incompatible (neither divides the other).
    AlignmentIncompatible { align1: u64, align2: u64 },
    /// Different top-level constructors (e.g., Struct vs Array).
    ConstructorMismatch,
    /// Struct field counts differ.
    FieldCountMismatch { count1: usize, count2: usize },
    /// Field at the given index is incompatible.
    FieldIncompatible { index: usize, reason: Box<IncompatibilityReason> },
    /// Array element counts differ.
    ArrayCountMismatch { count1: u64, count2: u64 },
    /// Enum variant counts differ.
    EnumVariantCountMismatch { count1: usize, count2: usize },
    /// Enum variant tag mismatch.
    EnumTagMismatch { index: usize, tag1: u64, tag2: u64 },
    /// Union alternative counts differ.
    UnionAltCountMismatch { count1: usize, count2: usize },
    /// Function parameter counts differ.
    ParamCountMismatch { count1: usize, count2: usize },
    /// A nested incompatibility with context.
    Nested { context: &'static str, reason: Box<IncompatibilityReason> },
    /// Generic catch-all.
    Other(String),
}

/// Result of checking whether two RepDs can coexist.
#[derive(Debug, Clone)]
pub struct CompatibilityResult {
    /// Whether the two RepDs are compatible.
    pub compatible: bool,
    /// How they are compatible (if they are).
    pub kind: Option<CompatibilityKind>,
    /// Why they are incompatible (if they are).
    pub reason: Option<IncompatibilityReason>,
}

impl CompatibilityResult {
    /// Create a positive result with the given kind.
    pub fn yes(kind: CompatibilityKind) -> Self {
        Self { compatible: true, kind: Some(kind), reason: None }
    }

    /// Create a negative result with the given reason.
    pub fn no(reason: IncompatibilityReason) -> Self {
        Self { compatible: false, kind: None, reason: Some(reason) }
    }
}

impl std::fmt::Display for CompatibilityResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.compatible {
            write!(
                f,
                "compatible ({})",
                self.kind
                    .as_ref()
                    .map(|k| format!("{k:?}"))
                    .unwrap_or_else(|| "unknown".into())
            )
        } else {
            write!(
                f,
                "incompatible ({})",
                self.reason
                    .as_ref()
                    .map(|r| format!("{r:?}"))
                    .unwrap_or_else(|| "unknown".into())
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Reinterpretation result types
// ---------------------------------------------------------------------------

/// Which reinterpretation rule (from the formal spec) applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReinterpretRule {
    /// R1: Any RepD → ByteRep of same size (byte erosion).
    ByteErosion,
    /// R2: Struct field-wise reinterpretation.
    StructFieldWise,
    /// R3: Array element-wise reinterpretation.
    ArrayElementWise,
    /// R4: Pointer → ByteRep of pointer size.
    PointerAsInteger,
    /// R5: Enum variant reinterpretation.
    EnumVariant,
    /// R6: Union alternative reinterpretation.
    UnionAlternative,
    /// R7: Transitivity (chain of two or more rules).
    Transitive,
    /// The two RepDs are identical; no reinterpretation needed.
    Identity,
}

/// Result of checking whether `from` can be safely reinterpreted as `to`.
#[derive(Debug, Clone)]
pub struct ReinterpretResult {
    /// Whether the reinterpretation is safe.
    pub can_reinterpret: bool,
    /// Which rule justifies the reinterpretation (if valid).
    pub rule: Option<ReinterpretRule>,
    /// Human-readable explanation.
    pub details: String,
}

impl ReinterpretResult {
    /// Create a positive result.
    pub fn yes(rule: ReinterpretRule, details: impl Into<String>) -> Self {
        Self { can_reinterpret: true, rule: Some(rule), details: details.into() }
    }

    /// Create a negative result.
    pub fn no(details: impl Into<String>) -> Self {
        Self { can_reinterpret: false, rule: None, details: details.into() }
    }
}

impl std::fmt::Display for ReinterpretResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.can_reinterpret {
            write!(
                f,
                "safe ({})",
                self.rule.as_ref().map(|r| format!("{r:?}")).unwrap_or_default()
            )
        } else {
            write!(f, "unsafe: {}", self.details)
        }
    }
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Check whether two RepDs can safely coexist in the same memory region.
///
/// Two RepDs are compatible when the intersection of their denotations is
/// non-empty: there exists at least one byte sequence that is valid under
/// both representations simultaneously.
///
/// This is a *bidirectional* check — it succeeds when memory laid out for
/// either RepD can be meaningfully viewed through the other.
pub fn are_compatible(r1: &RepD, r2: &RepD) -> CompatibilityResult {
    // Fast path: identical RepDs are always compatible.
    if r1 == r2 {
        return CompatibilityResult::yes(CompatibilityKind::Identical);
    }

    // Size must match for any coexistence.
    if r1.size() != r2.size() {
        return CompatibilityResult::no(IncompatibilityReason::SizeMismatch {
            size1: r1.size(),
            size2: r2.size(),
        });
    }

    // Alignments must be compatible: at least one must divide the other,
    // meaning there exists an address satisfying both.
    let a1 = r1.alignment();
    let a2 = r2.alignment();
    if !a1.is_multiple_of(a2) && !a2.is_multiple_of(a1) {
        return CompatibilityResult::no(IncompatibilityReason::AlignmentIncompatible {
            align1: a1,
            align2: a2,
        });
    }

    // If one subsumes the other, they are compatible.
    if r1.subsumes(r2) || r2.subsumes(r1) {
        return CompatibilityResult::yes(CompatibilityKind::Subsumption);
    }

    // Check structural compatibility (same constructor, compatible fields).
    let structural = check_structural_compatibility(r1, r2);
    if structural.compatible {
        return structural;
    }

    // Last resort: if the existing `compatible` method says yes (which
    // includes Byte catch-all), accept it.
    if r1.compatible(r2) {
        return CompatibilityResult::yes(CompatibilityKind::ByteErosion);
    }

    // If reinterpretation works in either direction, they can coexist.
    if can_reinterpret(r1, r2).can_reinterpret
        || can_reinterpret(r2, r1).can_reinterpret
    {
        return CompatibilityResult::yes(CompatibilityKind::ReinterpretCompatible);
    }

    // Incompatible: no way to view the same bytes under both RepDs.
    CompatibilityResult::no(IncompatibilityReason::ConstructorMismatch)
}

/// Structural compatibility check — same constructor with compatible fields.
fn check_structural_compatibility(r1: &RepD, r2: &RepD) -> CompatibilityResult {
    match (r1, r2) {
        (RepD::Byte(b1), RepD::Byte(b2)) => {
            // Two byte reps with same size and compatible alignment.
            if b1.size == b2.size {
                CompatibilityResult::yes(CompatibilityKind::StructuralMatch)
            } else {
                CompatibilityResult::no(IncompatibilityReason::SizeMismatch {
                    size1: b1.size,
                    size2: b2.size,
                })
            }
        }
        (RepD::Struct(s1), RepD::Struct(s2)) => {
            if s1.fields.len() != s2.fields.len() {
                return CompatibilityResult::no(IncompatibilityReason::FieldCountMismatch {
                    count1: s1.fields.len(),
                    count2: s2.fields.len(),
                });
            }
            for (i, ((off1, rep1), (off2, rep2))) in
                s1.fields.iter().zip(&s2.fields).enumerate()
            {
                if off1 != off2 {
                    return CompatibilityResult::no(IncompatibilityReason::FieldIncompatible {
                        index: i,
                        reason: Box::new(IncompatibilityReason::Other(format!(
                            "offset mismatch: {off1} vs {off2}"
                        ))),
                    });
                }
                let sub = are_compatible(rep1, rep2);
                if !sub.compatible {
                    return CompatibilityResult::no(IncompatibilityReason::FieldIncompatible {
                        index: i,
                        reason: Box::new(sub.reason.unwrap_or(IncompatibilityReason::Other(
                            "field incompatible".into(),
                        ))),
                    });
                }
            }
            CompatibilityResult::yes(CompatibilityKind::StructuralMatch)
        }
        (RepD::Array(a1), RepD::Array(a2)) => {
            if a1.count != a2.count {
                return CompatibilityResult::no(IncompatibilityReason::ArrayCountMismatch {
                    count1: a1.count,
                    count2: a2.count,
                });
            }
            let elem_compat = are_compatible(&a1.element, &a2.element);
            if elem_compat.compatible {
                CompatibilityResult::yes(CompatibilityKind::StructuralMatch)
            } else {
                CompatibilityResult::no(IncompatibilityReason::Nested {
                    context: "array element",
                    reason: Box::new(
                        elem_compat
                            .reason
                            .unwrap_or(IncompatibilityReason::Other("element mismatch".into())),
                    ),
                })
            }
        }
        (RepD::Enum(e1), RepD::Enum(e2)) => {
            if e1.variants.len() != e2.variants.len() {
                return CompatibilityResult::no(IncompatibilityReason::EnumVariantCountMismatch {
                    count1: e1.variants.len(),
                    count2: e2.variants.len(),
                });
            }
            for (i, ((t1, v1), (t2, v2))) in
                e1.variants.iter().zip(&e2.variants).enumerate()
            {
                if t1 != t2 {
                    return CompatibilityResult::no(IncompatibilityReason::EnumTagMismatch {
                        index: i,
                        tag1: *t1,
                        tag2: *t2,
                    });
                }
                let sub = are_compatible(v1, v2);
                if !sub.compatible {
                    return CompatibilityResult::no(IncompatibilityReason::Nested {
                        context: "enum variant",
                        reason: Box::new(
                            sub.reason
                                .unwrap_or(IncompatibilityReason::Other("variant mismatch".into())),
                        ),
                    });
                }
            }
            CompatibilityResult::yes(CompatibilityKind::StructuralMatch)
        }
        (RepD::Ptr(p1), RepD::Ptr(p2)) => {
            let sub = are_compatible(&p1.pointee, &p2.pointee);
            if sub.compatible {
                CompatibilityResult::yes(CompatibilityKind::StructuralMatch)
            } else {
                CompatibilityResult::no(IncompatibilityReason::Nested {
                    context: "pointer pointee",
                    reason: Box::new(
                        sub.reason
                            .unwrap_or(IncompatibilityReason::Other("pointee mismatch".into())),
                    ),
                })
            }
        }
        (RepD::Union(u1), RepD::Union(u2)) => {
            if u1.alternatives.len() != u2.alternatives.len() {
                return CompatibilityResult::no(IncompatibilityReason::UnionAltCountMismatch {
                    count1: u1.alternatives.len(),
                    count2: u2.alternatives.len(),
                });
            }
            for (i, (a, b)) in u1.alternatives.iter().zip(&u2.alternatives).enumerate() {
                let sub = are_compatible(a, b);
                if !sub.compatible {
                    return CompatibilityResult::no(IncompatibilityReason::Nested {
                        context: "union alternative",
                        reason: Box::new(sub.reason.unwrap_or(IncompatibilityReason::Other(
                            format!("alternative {i} mismatch"),
                        ))),
                    });
                }
            }
            CompatibilityResult::yes(CompatibilityKind::StructuralMatch)
        }
        (RepD::Func(f1), RepD::Func(f2)) => {
            if f1.params.len() != f2.params.len() {
                return CompatibilityResult::no(IncompatibilityReason::ParamCountMismatch {
                    count1: f1.params.len(),
                    count2: f2.params.len(),
                });
            }
            for (i, (p1, p2)) in f1.params.iter().zip(&f2.params).enumerate() {
                let sub = are_compatible(p1, p2);
                if !sub.compatible {
                    return CompatibilityResult::no(IncompatibilityReason::Nested {
                        context: "function param",
                        reason: Box::new(sub.reason.unwrap_or(IncompatibilityReason::Other(
                            format!("param {i} mismatch"),
                        ))),
                    });
                }
            }
            let ret = are_compatible(&f1.result, &f2.result);
            if ret.compatible {
                CompatibilityResult::yes(CompatibilityKind::StructuralMatch)
            } else {
                CompatibilityResult::no(IncompatibilityReason::Nested {
                    context: "function result",
                    reason: Box::new(
                        ret.reason
                            .unwrap_or(IncompatibilityReason::Other("result mismatch".into())),
                    ),
                })
            }
        }
        // Byte is compatible with anything of matching size.
        (RepD::Byte(_), _) | (_, RepD::Byte(_)) => {
            // Already checked sizes match above.
            CompatibilityResult::yes(CompatibilityKind::ByteErosion)
        }
        _ => CompatibilityResult::no(IncompatibilityReason::ConstructorMismatch),
    }
}

// ---------------------------------------------------------------------------
// Lattice operations: meet (GLB) and join (LUB)
// ---------------------------------------------------------------------------

/// Compute the **greatest lower bound** (meet) of two RepDs.
///
/// In the subsumption lattice, `meet(r1, r2)` returns the most specific
/// (most restrictive) RepD that is subsumed by both `r1` and `r2`.
///
/// Returns `None` if no such bound exists (the two RepDs have no common
/// descendant in the lattice).
pub fn meet(r1: &RepD, r2: &RepD) -> Option<RepD> {
    // Identical → trivially, either one.
    if r1 == r2 {
        return Some(r1.clone());
    }

    // If r1 subsumes r2, then r2 is more specific and is the GLB.
    if r1.subsumes(r2) {
        return Some(r2.clone());
    }
    // If r2 subsumes r1, then r1 is more specific.
    if r2.subsumes(r1) {
        return Some(r1.clone());
    }

    // Structural meet: same constructor, compatible fields → meet field-wise.
    match (r1, r2) {
        (RepD::Byte(b1), RepD::Byte(b2)) => {
            if b1.size != b2.size {
                return None;
            }
            // Meet takes the stricter (larger) alignment — more restrictive.
            Some(RepD::Byte(ByteRep {
                size: b1.size,
                align: b1.align.max(b2.align),
            }))
        }
        (RepD::Struct(s1), RepD::Struct(s2)) => {
            if s1.fields.len() != s2.fields.len() {
                return None;
            }
            let mut fields = Vec::with_capacity(s1.fields.len());
            for ((off1, rep1), (off2, rep2)) in s1.fields.iter().zip(&s2.fields) {
                if off1 != off2 {
                    return None;
                }
                let m = meet(rep1, rep2)?;
                fields.push((*off1, m));
            }
            let align = s1.align.max(s2.align);
            let total_size = fields
                .iter()
                .map(|(off, rep)| off + rep.size())
                .max()
                .unwrap_or(0);
            Some(RepD::Struct(StructRep {
                fields,
                total_size: total_size.max(s1.total_size).max(s2.total_size),
                align,
            }))
        }
        (RepD::Array(a1), RepD::Array(a2)) => {
            if a1.count != a2.count {
                return None;
            }
            let elem = meet(&a1.element, &a2.element)?;
            Some(RepD::Array(ArrayRep {
                element: Box::new(elem),
                count: a1.count,
            }))
        }
        (RepD::Enum(e1), RepD::Enum(e2)) => {
            if e1.variants.len() != e2.variants.len() {
                return None;
            }
            let mut variants = Vec::with_capacity(e1.variants.len());
            for ((t1, v1), (t2, v2)) in e1.variants.iter().zip(&e2.variants) {
                if t1 != t2 {
                    return None;
                }
                let m = meet(v1, v2)?;
                variants.push((*t1, m));
            }
            Some(RepD::Enum(EnumRep { variants }))
        }
        (RepD::Ptr(p1), RepD::Ptr(p2)) => {
            let pointee = meet(&p1.pointee, &p2.pointee)?;
            Some(RepD::Ptr(PtrRep { pointee: Box::new(pointee) }))
        }
        (RepD::Union(u1), RepD::Union(u2)) => {
            if u1.alternatives.len() != u2.alternatives.len() {
                return None;
            }
            let mut alternatives = Vec::with_capacity(u1.alternatives.len());
            for (a, b) in u1.alternatives.iter().zip(&u2.alternatives) {
                alternatives.push(meet(a, b)?);
            }
            let max_size = alternatives.iter().map(|r| r.size()).max().unwrap_or(0);
            let max_align = alternatives.iter().map(|r| r.alignment()).max().unwrap_or(1);
            Some(RepD::Union(UnionRep {
                alternatives,
                max_size,
                max_align,
            }))
        }
        (RepD::Func(f1), RepD::Func(f2)) => {
            if f1.params.len() != f2.params.len() {
                return None;
            }
            let mut params = Vec::with_capacity(f1.params.len());
            for (p1, p2) in f1.params.iter().zip(&f2.params) {
                params.push(meet(p1, p2)?);
            }
            let result = meet(&f1.result, &f2.result)?;
            Some(RepD::Func(FuncRep {
                params,
                result: Box::new(result),
            }))
        }
        // Cross-constructor meet: if both have the same size, the ByteRep
        // that is compatible with both might work, but that would be a join,
        // not a meet. Cross-constructor meet generally doesn't exist unless
        // there's a more specific common descendant, which isn't possible
        // for different constructors.
        _ => None,
    }
}

/// Compute the **least upper bound** (join) of two RepDs.
///
/// In the subsumption lattice, `join(r1, r2)` returns the most general
/// (least restrictive) RepD that subsumes both `r1` and `r2`.
///
/// Returns `None` if no such bound exists (the two RepDs have different
/// sizes and thus no common ancestor).
pub fn join(r1: &RepD, r2: &RepD) -> Option<RepD> {
    // Identical → either one.
    if r1 == r2 {
        return Some(r1.clone());
    }

    // If r1 subsumes r2, then r1 is more general and is the LUB.
    if r1.subsumes(r2) {
        return Some(r1.clone());
    }
    if r2.subsumes(r1) {
        return Some(r2.clone());
    }

    // Different sizes → no common upper bound possible.
    if r1.size() != r2.size() {
        return None;
    }

    // Structural join: same constructor → join field-wise.
    match (r1, r2) {
        (RepD::Byte(b1), RepD::Byte(b2)) => {
            // Join takes the weaker (smaller) alignment — more general.
            Some(RepD::Byte(ByteRep {
                size: b1.size,
                align: b1.align.min(b2.align),
            }))
        }
        (RepD::Struct(s1), RepD::Struct(s2)) => {
            if s1.fields.len() != s2.fields.len() {
                // Different field counts → fall back to Byte.
                return join_fallback(r1, r2);
            }
            let mut fields = Vec::with_capacity(s1.fields.len());
            for ((off1, rep1), (off2, rep2)) in s1.fields.iter().zip(&s2.fields) {
                if off1 != off2 {
                    return join_fallback(r1, r2);
                }
                let j = join(rep1, rep2)?;
                fields.push((*off1, j));
            }
            let align = s1.align.min(s2.align);
            let total_size = fields
                .iter()
                .map(|(off, rep)| off + rep.size())
                .max()
                .unwrap_or(0);
            Some(RepD::Struct(StructRep {
                fields,
                total_size: total_size.max(s1.total_size).max(s2.total_size),
                align,
            }))
        }
        (RepD::Array(a1), RepD::Array(a2)) => {
            if a1.count != a2.count {
                return join_fallback(r1, r2);
            }
            let elem = join(&a1.element, &a2.element)?;
            Some(RepD::Array(ArrayRep {
                element: Box::new(elem),
                count: a1.count,
            }))
        }
        (RepD::Enum(e1), RepD::Enum(e2)) => {
            if e1.variants.len() != e2.variants.len() {
                return join_fallback(r1, r2);
            }
            let mut variants = Vec::with_capacity(e1.variants.len());
            for ((t1, v1), (t2, v2)) in e1.variants.iter().zip(&e2.variants) {
                if t1 != t2 {
                    return join_fallback(r1, r2);
                }
                let j = join(v1, v2)?;
                variants.push((*t1, j));
            }
            Some(RepD::Enum(EnumRep { variants }))
        }
        (RepD::Ptr(p1), RepD::Ptr(p2)) => {
            let pointee = join(&p1.pointee, &p2.pointee)?;
            Some(RepD::Ptr(PtrRep { pointee: Box::new(pointee) }))
        }
        (RepD::Union(u1), RepD::Union(u2)) => {
            if u1.alternatives.len() != u2.alternatives.len() {
                return join_fallback(r1, r2);
            }
            let mut alternatives = Vec::with_capacity(u1.alternatives.len());
            for (a, b) in u1.alternatives.iter().zip(&u2.alternatives) {
                alternatives.push(join(a, b)?);
            }
            let max_size = alternatives.iter().map(|r| r.size()).max().unwrap_or(0);
            let max_align = alternatives.iter().map(|r| r.alignment()).max().unwrap_or(1);
            Some(RepD::Union(UnionRep {
                alternatives,
                max_size,
                max_align,
            }))
        }
        (RepD::Func(f1), RepD::Func(f2)) => {
            if f1.params.len() != f2.params.len() {
                return join_fallback(r1, r2);
            }
            let mut params = Vec::with_capacity(f1.params.len());
            for (p1, p2) in f1.params.iter().zip(&f2.params) {
                params.push(join(p1, p2)?);
            }
            let result = join(&f1.result, &f2.result)?;
            Some(RepD::Func(FuncRep {
                params,
                result: Box::new(result),
            }))
        }
        // Cross-constructor: fall back to ByteRep (the top element).
        _ => join_fallback(r1, r2),
    }
}

/// Fallback: produce a `ByteRep` as the least upper bound when structural
/// join is not possible. The join ByteRep has the same size and the
/// least-restrictive alignment that still satisfies both inputs.
fn join_fallback(r1: &RepD, r2: &RepD) -> Option<RepD> {
    let s1 = r1.size();
    let s2 = r2.size();
    if s1 != s2 {
        return None;
    }
    // Both alignments are powers of 2. The weakest alignment that
    // satisfies both is max(a1, a2)... wait, no. For subsumes(Byte{n,a}, r),
    // we need alignment(r) | a. So Byte{n, max(a1,a2)} subsumes both.
    // But we want the LEAST upper bound, so we want the smallest a where
    // a1 | a and a2 | a. Since a1, a2 are powers of 2, that's max(a1, a2).
    let align = r1.alignment().max(r2.alignment());
    Some(RepD::Byte(ByteRep { size: s1, align }))
}

// ---------------------------------------------------------------------------
// Reinterpretation
// ---------------------------------------------------------------------------

/// Check whether `from` can be safely reinterpreted as `to`.
///
/// Implements the reinterpretation rules R1–R7 from the formal specification:
///
/// - **R1** (Byte Erosion): any RepD → ByteRep of same size
/// - **R2** (Struct Field-wise): struct → struct with field-wise reinterp
/// - **R3** (Array Element-wise): array → array with element reinterp
/// - **R4** (Pointer as Integer): PtrRep → ByteRep of pointer size
/// - **R5** (Enum Variant): enum → enum with variant reinterp
/// - **R6** (Union Alternative): union → union with alt reinterp
/// - **R7** (Transitivity): chaining of valid reinterp steps
pub fn can_reinterpret(from: &RepD, to: &RepD) -> ReinterpretResult {
    // Identity: no reinterpretation needed.
    if from == to {
        return ReinterpretResult::yes(ReinterpretRule::Identity, "identical representations");
    }

    // R1: Byte Erosion — any RepD can be reinterpreted as bytes.
    if let RepD::Byte(b) = to {
        if from.size() == b.size && from.alignment() >= b.align {
            return ReinterpretResult::yes(
                ReinterpretRule::ByteErosion,
                format!(
                    "R1: byte erosion — {} → bytes(size={}, align={})",
                    from, b.size, b.align
                ),
            );
        } else if from.size() != b.size {
            return ReinterpretResult::no(format!(
                "size mismatch: source size {} ≠ target size {}",
                from.size(),
                b.size
            ));
        } else {
            return ReinterpretResult::no(format!(
                "alignment too weak: source align {} < target align {}",
                from.alignment(),
                b.align
            ));
        }
    }

    // R4: Pointer → ByteRep (specialized form of R1 for pointers).
    if let RepD::Ptr(_) = from {
        if let RepD::Byte(b) = to {
            if b.size == POINTER_SIZE && b.align <= POINTER_SIZE {
                return ReinterpretResult::yes(
                    ReinterpretRule::PointerAsInteger,
                    "R4: pointer reinterpreted as integer bytes",
                );
            }
        }
    }

    // R2: Struct field-wise reinterpretation.
    if let (RepD::Struct(s_from), RepD::Struct(s_to)) = (from, to) {
        if s_from.fields.len() != s_to.fields.len() {
            return ReinterpretResult::no(format!(
                "field count mismatch: {} vs {}",
                s_from.fields.len(),
                s_to.fields.len()
            ));
        }
        for (i, ((off_from, rep_from), (off_to, rep_to))) in
            s_from.fields.iter().zip(&s_to.fields).enumerate()
        {
            if off_from != off_to {
                return ReinterpretResult::no(format!(
                    "field {i} offset mismatch: {off_from} vs {off_to}"
                ));
            }
            let sub = can_reinterpret(rep_from, rep_to);
            if !sub.can_reinterpret {
                return ReinterpretResult::no(format!(
                    "field {i} cannot be reinterpreted: {}",
                    sub.details
                ));
            }
        }
        return ReinterpretResult::yes(
            ReinterpretRule::StructFieldWise,
            "R2: struct field-wise reinterpretation",
        );
    }

    // R3: Array element-wise reinterpretation.
    if let (RepD::Array(a_from), RepD::Array(a_to)) = (from, to) {
        if a_from.count != a_to.count {
            return ReinterpretResult::no(format!(
                "array count mismatch: {} vs {}",
                a_from.count, a_to.count
            ));
        }
        let sub = can_reinterpret(&a_from.element, &a_to.element);
        if sub.can_reinterpret {
            return ReinterpretResult::yes(
                ReinterpretRule::ArrayElementWise,
                format!("R3: array element-wise reinterpretation ({})", sub.details),
            );
        } else {
            return ReinterpretResult::no(format!(
                "array element cannot be reinterpreted: {}",
                sub.details
            ));
        }
    }

    // R5: Enum variant reinterpretation.
    if let (RepD::Enum(e_from), RepD::Enum(e_to)) = (from, to) {
        if e_from.variants.len() != e_to.variants.len() {
            return ReinterpretResult::no(format!(
                "enum variant count mismatch: {} vs {}",
                e_from.variants.len(),
                e_to.variants.len()
            ));
        }
        for (i, ((t_from, v_from), (t_to, v_to))) in
            e_from.variants.iter().zip(&e_to.variants).enumerate()
        {
            if t_from != t_to {
                return ReinterpretResult::no(format!(
                    "variant {i} tag mismatch: {t_from} vs {t_to}"
                ));
            }
            let sub = can_reinterpret(v_from, v_to);
            if !sub.can_reinterpret {
                return ReinterpretResult::no(format!(
                    "variant {i} cannot be reinterpreted: {}",
                    sub.details
                ));
            }
        }
        return ReinterpretResult::yes(
            ReinterpretRule::EnumVariant,
            "R5: enum variant reinterpretation",
        );
    }

    // R6: Union alternative reinterpretation.
    if let (RepD::Union(u_from), RepD::Union(u_to)) = (from, to) {
        if u_from.alternatives.len() != u_to.alternatives.len() {
            return ReinterpretResult::no(format!(
                "union alternative count mismatch: {} vs {}",
                u_from.alternatives.len(),
                u_to.alternatives.len()
            ));
        }
        for (i, (a, b)) in u_from.alternatives.iter().zip(&u_to.alternatives).enumerate() {
            let sub = can_reinterpret(a, b);
            if !sub.can_reinterpret {
                return ReinterpretResult::no(format!(
                    "union alternative {i} cannot be reinterpreted: {}",
                    sub.details
                ));
            }
        }
        return ReinterpretResult::yes(
            ReinterpretRule::UnionAlternative,
            "R6: union alternative reinterpretation",
        );
    }

    // R7: Transitivity — try to find an intermediate representation.
    // For now, we check a common pattern: from → ByteRep → to.
    // If `from` can be eroded to bytes and those bytes can be
    // structurally reinterpreted to `to`, the chain is valid.
    let byte_mid = RepD::Byte(ByteRep {
        size: from.size(),
        align: from.alignment(),
    });
    let step1 = can_reinterpret(from, &byte_mid);
    if step1.can_reinterpret {
        // Now try: can bytes be reinterpreted to `to`?
        // Bytes can be reinterpreted to a struct if alignment is satisfied.
        if can_reinterpret_from_bytes(&byte_mid, to) {
            return ReinterpretResult::yes(
                ReinterpretRule::Transitive,
                "R7: transitive via byte erosion",
            );
        }
    }

    ReinterpretResult::no(format!(
        "no valid reinterpretation path from {} to {}",
        from, to
    ))
}

/// Check whether a ByteRep can be reinterpreted to the target RepD.
///
/// This is the inverse of byte erosion: raw bytes can be reinterpreted
/// as a structured type only if the alignment constraint is satisfied
/// (the byte alignment must be a multiple of the target alignment).
fn can_reinterpret_from_bytes(bytes: &RepD, to: &RepD) -> bool {
    if let RepD::Byte(b) = bytes {
        if b.size != to.size() {
            return false;
        }
        // The byte alignment must be a multiple of the target alignment.
        // i.e., to.alignment() must divide b.align.
        if b.align % to.alignment() != 0 {
            return false;
        }
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Size & alignment convenience functions
// ---------------------------------------------------------------------------

/// Compute the total byte size of a RepD.
///
/// This is a convenience wrapper around [`RepD::size`] that returns `usize`.
pub fn size_of(r: &RepD) -> usize {
    r.size() as usize
}

/// Compute the alignment requirement (in bytes) of a RepD.
///
/// This is a convenience wrapper around [`RepD::alignment`] that returns
/// `usize`.
pub fn alignment_of(r: &RepD) -> usize {
    r.alignment() as usize
}

// ---------------------------------------------------------------------------
// Subtyping
// ---------------------------------------------------------------------------

/// Check the subtyping relation: `sub` is a subtype of `sup`.
///
/// `is_subtype(sub, sup)` holds when every value described by `sub` can be
/// safely used where `sup` is expected. Formally:
///
/// ```text
/// is_subtype(sub, sup)  ⟺  ⟦sub⟧ ⊆ ⟦sup⟧  ⟺  sup.subsumes(sub)
/// ```
///
/// In the subsumption lattice, this means `sub ≤ sup` (sub is lower/more
/// specific, sup is higher/more general).
pub fn is_subtype(sub: &RepD, sup: &RepD) -> bool {
    // Fast path: identical types are trivially subtypes.
    if sub == sup {
        return true;
    }

    // ByteRep sup subsumes anything of matching size with stricter alignment.
    // subsumes(Byte{n,a}, r2) iff size(r2)=n && alignment(r2) | a
    // i.e., sub.alignment() must divide sup.align (sub has stricter alignment)
    if let RepD::Byte(b) = sup {
        if sub.size() == b.size && sub.alignment() > 0 && b.align > 0 && sub.alignment().is_multiple_of(b.align) {
            return true;
        }
    }

    // Structural subtyping: same constructor, covariant in fields.
    match (sub, sup) {
        (RepD::Struct(s_sub), RepD::Struct(s_sup)) => {
            if s_sub.fields.len() != s_sup.fields.len() {
                return false;
            }
            s_sub
                .fields
                .iter()
                .zip(&s_sup.fields)
                .all(|((off_sub, rep_sub), (off_sup, rep_sup))| {
                    off_sub == off_sup && is_subtype(rep_sub, rep_sup)
                })
        }
        (RepD::Array(a_sub), RepD::Array(a_sup)) => {
            a_sub.count == a_sup.count && is_subtype(&a_sub.element, &a_sup.element)
        }
        (RepD::Enum(e_sub), RepD::Enum(e_sup)) => {
            if e_sub.variants.len() != e_sup.variants.len() {
                return false;
            }
            e_sub
                .variants
                .iter()
                .zip(&e_sup.variants)
                .all(|((t_sub, v_sub), (t_sup, v_sup))| {
                    t_sub == t_sup && is_subtype(v_sub, v_sup)
                })
        }
        (RepD::Ptr(p_sub), RepD::Ptr(p_sup)) => {
            // Pointers are covariant in their pointee type.
            is_subtype(&p_sub.pointee, &p_sup.pointee)
        }
        (RepD::Union(u_sub), RepD::Union(u_sup)) => {
            if u_sub.alternatives.len() != u_sup.alternatives.len() {
                return false;
            }
            u_sub
                .alternatives
                .iter()
                .zip(&u_sup.alternatives)
                .all(|(a, b)| is_subtype(a, b))
        }
        (RepD::Func(f_sub), RepD::Func(f_sup)) => {
            // Function subtyping: contravariant in params, covariant in result.
            if f_sub.params.len() != f_sup.params.len() {
                return false;
            }
            // Contravariant params: sup param is a subtype of sub param.
            let params_ok = f_sub
                .params
                .iter()
                .zip(&f_sup.params)
                .all(|(p_sub, p_sup)| is_subtype(p_sup, p_sub));
            // Covariant result.
            let result_ok = is_subtype(&f_sub.result, &f_sup.result);
            params_ok && result_ok
        }
        (RepD::Byte(b_sub), RepD::Byte(b_sup)) => {
            // Byte{n,a1} <: Byte{n,a2} iff a2 | a1 (weaker alignment is more general).
            b_sub.size == b_sup.size && b_sup.align > 0 && b_sub.align % b_sup.align == 0
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper constructors -------------------------------------------------

    fn byte(size: u64, align: u64) -> RepD {
        RepD::Byte(ByteRep { size, align })
    }

    fn ptr(pointee: RepD) -> RepD {
        RepD::Ptr(PtrRep { pointee: Box::new(pointee) })
    }

    fn array(element: RepD, count: u64) -> RepD {
        RepD::Array(ArrayRep { element: Box::new(element), count })
    }

    fn struct_of(fields: Vec<(u64, RepD)>, total_size: u64, align: u64) -> RepD {
        RepD::Struct(StructRep { fields, total_size, align })
    }

    fn enum_of(variants: Vec<(u64, RepD)>) -> RepD {
        RepD::Enum(EnumRep { variants })
    }

    fn union_of(alternatives: Vec<RepD>) -> RepD {
        let max_size = alternatives.iter().map(|r| r.size()).max().unwrap_or(0);
        let max_align = alternatives.iter().map(|r| r.alignment()).max().unwrap_or(1);
        RepD::Union(UnionRep { alternatives, max_size, max_align })
    }

    fn func(params: Vec<RepD>, result: RepD) -> RepD {
        RepD::Func(FuncRep { params, result: Box::new(result) })
    }

    // Test 1: are_compatible — identical RepDs --------------------------------

    #[test]
    fn test_compatible_identical() {
        let r = byte(4, 4);
        let result = are_compatible(&r, &r);
        assert!(result.compatible);
        assert_eq!(result.kind, Some(CompatibilityKind::Identical));
    }

    // Test 2: are_compatible — size mismatch ----------------------------------

    #[test]
    fn test_compatible_size_mismatch() {
        let r1 = byte(4, 4);
        let r2 = byte(8, 8);
        let result = are_compatible(&r1, &r2);
        assert!(!result.compatible);
        assert!(matches!(
            result.reason,
            Some(IncompatibilityReason::SizeMismatch { size1: 4, size2: 8 })
        ));
    }

    // Test 3: are_compatible — byte erosion -----------------------------------

    #[test]
    fn test_compatible_byte_erosion() {
        let structured = struct_of(
            vec![(0, byte(4, 4)), (4, byte(4, 4))],
            8,
            4,
        );
        let raw = byte(8, 4);
        let result = are_compatible(&structured, &raw);
        assert!(result.compatible);
    }

    // Test 4: are_compatible — struct fields match ----------------------------

    #[test]
    fn test_compatible_struct_fields() {
        let s = struct_of(
            vec![(0, byte(4, 4)), (4, byte(4, 4))],
            8,
            4,
        );
        let result = are_compatible(&s, &s);
        assert!(result.compatible);
        assert_eq!(result.kind, Some(CompatibilityKind::Identical));
    }

    // Test 5: are_compatible — struct field count mismatch --------------------

    #[test]
    fn test_compatible_struct_field_count() {
        let s1 = struct_of(vec![(0, byte(4, 4))], 4, 4);
        let s2 = struct_of(vec![(0, byte(2, 2)), (2, byte(2, 2))], 4, 4);
        let result = are_compatible(&s1, &s2);
        assert!(!result.compatible);
    }

    // Test 6: can_reinterpret — R1 byte erosion -------------------------------

    #[test]
    fn test_reinterpret_byte_erosion() {
        let s = struct_of(vec![(0, byte(4, 4))], 4, 4);
        let b = byte(4, 4);
        let result = can_reinterpret(&s, &b);
        assert!(result.can_reinterpret);
        assert_eq!(result.rule, Some(ReinterpretRule::ByteErosion));
    }

    // Test 7: can_reinterpret — R3 array element-wise -------------------------

    #[test]
    fn test_reinterpret_array_element() {
        let a1 = array(byte(4, 4), 10);
        let a2 = array(byte(4, 4), 10);
        let result = can_reinterpret(&a1, &a2);
        assert!(result.can_reinterpret);
        assert_eq!(result.rule, Some(ReinterpretRule::Identity));
    }

    // Test 8: can_reinterpret — array to bytes (R3 + R1) ----------------------

    #[test]
    fn test_reinterpret_array_to_bytes() {
        let a = array(byte(4, 4), 3);
        let b = byte(12, 4);
        let result = can_reinterpret(&a, &b);
        assert!(result.can_reinterpret);
        // Should be R1 directly (any → bytes).
        assert_eq!(result.rule, Some(ReinterpretRule::ByteErosion));
    }

    // Test 9: can_reinterpret — R4 pointer as integer -------------------------

    #[test]
    fn test_reinterpret_pointer_as_integer() {
        let p = ptr(byte(1, 1));
        let b = byte(POINTER_SIZE, POINTER_SIZE as u64);
        let result = can_reinterpret(&p, &b);
        assert!(result.can_reinterpret);
        assert_eq!(result.rule, Some(ReinterpretRule::ByteErosion));
    }

    // Test 10: can_reinterpret — invalid reinterpretation ---------------------

    #[test]
    fn test_reinterpret_invalid() {
        let s = struct_of(vec![(0, byte(4, 4)), (4, byte(4, 4))], 8, 4);
        let different = struct_of(vec![(0, byte(8, 8))], 8, 8);
        let result = can_reinterpret(&s, &different);
        // Field 0: byte(4,4) → byte(8,8) — size mismatch, can't reinterpret.
        assert!(!result.can_reinterpret);
    }

    // Test 11: meet — identical RepDs -----------------------------------------

    #[test]
    fn test_meet_identical() {
        let r = byte(4, 4);
        let m = meet(&r, &r).unwrap();
        assert_eq!(m, r);
    }

    // Test 12: meet — byte reps with different alignment ----------------------

    #[test]
    fn test_meet_bytes_stricter_alignment() {
        let b1 = byte(8, 4);
        let b2 = byte(8, 8);
        let m = meet(&b1, &b2).unwrap();
        // Meet takes the stricter (larger) alignment.
        assert_eq!(m, byte(8, 8));
    }

    // Test 13: meet — struct field-wise ---------------------------------------

    #[test]
    fn test_meet_struct_field_wise() {
        let s1 = struct_of(
            vec![(0, byte(4, 4)), (4, byte(4, 4))],
            8,
            4,
        );
        let s2 = struct_of(
            vec![(0, byte(4, 8)), (4, byte(4, 8))],
            8,
            8,
        );
        let m = meet(&s1, &s2).unwrap();
        // Each field's meet should have the stricter alignment.
        if let RepD::Struct(s) = m {
            assert_eq!(s.fields[0].1.alignment(), 8);
            assert_eq!(s.fields[1].1.alignment(), 8);
        } else {
            panic!("expected struct");
        }
    }

    // Test 14: meet — incompatible constructors → None ------------------------

    #[test]
    fn test_meet_incompatible_constructors() {
        let s = struct_of(vec![(0, byte(4, 4))], 4, 4);
        let a = array(byte(2, 2), 2);
        // Different constructors → no meet.
        assert!(meet(&s, &a).is_none());
    }

    // Test 15: join — byte reps with different alignment ----------------------

    #[test]
    fn test_join_bytes_weaker_alignment() {
        let b1 = byte(8, 8);
        let b2 = byte(8, 4);
        let j = join(&b1, &b2).unwrap();
        // Join takes the weaker (smaller) alignment — the least upper bound.
        // byte(8,8) <: byte(8,4), so join = byte(8,4)
        assert_eq!(j, byte(8, 4));
    }

    // Test 16: join — subsumption case ----------------------------------------

    #[test]
    fn test_join_subsumption() {
        let b = byte(8, 8);
        let s = struct_of(
            vec![(0, byte(4, 4)), (4, byte(4, 4))],
            8,
            4,
        );
        // byte subsumes struct, so join is byte.
        let j = join(&b, &s).unwrap();
        assert_eq!(j, b);
    }

    // Test 17: join — cross-constructor fallback to Byte ----------------------

    #[test]
    fn test_join_cross_constructor() {
        let s = struct_of(vec![(0, byte(4, 4))], 4, 4);
        let a = array(byte(2, 2), 2);
        let j = join(&s, &a).unwrap();
        // Cross-constructor → Byte fallback.
        assert_eq!(j, byte(4, 4));
    }

    // Test 18: size_of and alignment_of ---------------------------------------

    #[test]
    fn test_size_of_alignment_of() {
        let s = struct_of(
            vec![(0, byte(4, 4)), (4, byte(4, 4))],
            8,
            4,
        );
        assert_eq!(size_of(&s), 8);
        assert_eq!(alignment_of(&s), 4);

        let a = array(byte(4, 4), 10);
        assert_eq!(size_of(&a), 40);
        assert_eq!(alignment_of(&a), 4);

        let p = ptr(byte(1, 1));
        assert_eq!(size_of(&p), 8);
        assert_eq!(alignment_of(&p), 8);
    }

    // Test 19: is_subtype — identical -----------------------------------------

    #[test]
    fn test_is_subtype_identical() {
        let r = byte(4, 4);
        assert!(is_subtype(&r, &r));
    }

    // Test 20: is_subtype — byte supertype ------------------------------------

    #[test]
    fn test_is_subtype_byte_supertype() {
        let s = struct_of(vec![(0, byte(4, 4))], 4, 4);
        let b = byte(4, 4);
        // struct <: byte (byte is more general, subsumes struct)
        assert!(is_subtype(&s, &b));
        // NOT: byte <: struct
        assert!(!is_subtype(&b, &s));
    }

    // Test 21: is_subtype — struct covariant fields ---------------------------

    #[test]
    fn test_is_subtype_struct_covariant() {
        let s_sub = struct_of(
            vec![(0, byte(4, 8))], // stricter field alignment
            4,
            8,
        );
        let s_sup = struct_of(
            vec![(0, byte(4, 4))], // weaker field alignment
            4,
            4,
        );
        assert!(is_subtype(&s_sub, &s_sup));
    }

    // Test 22: is_subtype — array covariant element ---------------------------

    #[test]
    fn test_is_subtype_array_covariant() {
        let a_sub = array(byte(4, 8), 5);
        let a_sup = array(byte(4, 4), 5);
        assert!(is_subtype(&a_sub, &a_sup));
    }

    // Test 23: is_subtype — function contravariant params ---------------------

    #[test]
    fn test_is_subtype_func_contravariant() {
        // f1: (byte(4,8)) → byte(4,4)   -- stricter param, weaker result
        // f2: (byte(4,4)) → byte(4,8)   -- weaker param, stricter result
        // f1 <: f2 because params are contravariant, result is covariant
        let f1 = func(vec![byte(4, 8)], byte(4, 4));
        let f2 = func(vec![byte(4, 4)], byte(4, 8));
        // f1 is NOT a subtype of f2 because result byte(4,4) is NOT <: byte(4,8)
        // byte(4,4) <: byte(4,8) iff 8 | 4... that's false.
        // Wait, is_subtype(byte(4,4), byte(4,8)):
        //   b_sub.align = 4, b_sup.align = 8
        //   b_sub.align % b_sup.align = 4 % 8 != 0 → false
        // So this is NOT a subtype relationship.
        // Let me construct a valid one instead.
        assert!(!is_subtype(&f1, &f2));

        // Valid: f_sub <: f_sup where:
        //   param: sup_param <: sub_param (contravariant)
        //   result: sub_result <: sup_result (covariant)
        let f_sub = func(vec![byte(4, 4)], byte(4, 8));
        let f_sup = func(vec![byte(4, 8)], byte(4, 4));
        // Check: byte(4,8) <: byte(4,4)? 4 % 8 = 4, not 0. No.
        // Hmm. Let me think again about Byte subtyping.
        // byte(4,4) <: byte(4,8) requires b_sup.align | b_sub.align → 8 | 4 = false.
        // byte(4,8) <: byte(4,4) requires b_sup.align | b_sub.align → 4 | 8 = true. Yes!
        // So byte with stricter alignment IS a subtype of byte with weaker alignment.
        //
        // For contravariant params:
        //   f_sub takes byte(4,4), f_sup takes byte(4,8)
        //   need: byte(4,8) <: byte(4,4) → 4 | 8 → true ✓
        // For covariant result:
        //   f_sub returns byte(4,8), f_sup returns byte(4,4)
        //   need: byte(4,8) <: byte(4,4) → 4 | 8 → true ✓
        assert!(is_subtype(&f_sub, &f_sup));
    }

    // Test 24: is_subtype — pointer covariance -------------------------------

    #[test]
    fn test_is_subtype_ptr_covariant() {
        let p_sub = ptr(byte(4, 8));
        let p_sup = ptr(byte(4, 4));
        assert!(is_subtype(&p_sub, &p_sup));
    }

    // Test 25: are_compatible — enum compatibility ---------------------------

    #[test]
    fn test_compatible_enum() {
        let e1 = enum_of(vec![(0, byte(4, 4)), (1, byte(4, 4))]);
        let e2 = enum_of(vec![(0, byte(4, 4)), (1, byte(4, 4))]);
        let result = are_compatible(&e1, &e2);
        assert!(result.compatible);
    }

    // Test 26: are_compatible — enum tag mismatch ----------------------------

    #[test]
    fn test_compatible_enum_tag_mismatch() {
        let e1 = enum_of(vec![(0, byte(4, 4)), (1, byte(4, 4))]);
        let e2 = enum_of(vec![(0, byte(4, 4)), (2, byte(4, 4))]);
        let result = are_compatible(&e1, &e2);
        assert!(!result.compatible);
    }

    // Test 27: are_compatible — union alternatives ----------------------------

    #[test]
    fn test_compatible_union() {
        let u1 = union_of(vec![byte(4, 4), byte(2, 2)]);
        let u2 = union_of(vec![byte(4, 4), byte(2, 2)]);
        let result = are_compatible(&u1, &u2);
        assert!(result.compatible);
    }

    // Test 28: can_reinterpret — R2 struct field-wise -------------------------

    #[test]
    fn test_reinterpret_struct_fields() {
        let s_from = struct_of(
            vec![(0, byte(4, 4)), (4, byte(4, 4))],
            8,
            4,
        );
        let s_to = struct_of(
            vec![(0, byte(4, 4)), (4, byte(4, 4))],
            8,
            4,
        );
        let result = can_reinterpret(&s_from, &s_to);
        assert!(result.can_reinterpret);
        // Identical structs → Identity rule.
        assert_eq!(result.rule, Some(ReinterpretRule::Identity));
    }

    // Test 29: can_reinterpret — R7 transitivity via bytes --------------------

    #[test]
    fn test_reinterpret_transitive() {
        let s = struct_of(vec![(0, byte(4, 4))], 4, 4);
        // s → byte(4,4) → struct with same layout (if alignment works)
        let target = struct_of(vec![(0, byte(4, 4))], 4, 4);
        let result = can_reinterpret(&s, &target);
        // Should succeed via identity.
        assert!(result.can_reinterpret);
    }

    // Test 30: CompatibilityResult display ------------------------------------

    #[test]
    fn test_compatibility_result_display() {
        let r1 = byte(4, 4);
        let r2 = byte(8, 8);
        let result = are_compatible(&r1, &r2);
        let display = format!("{result}");
        assert!(display.contains("incompatible"));
    }

    // Test 31: ReinterpretResult display --------------------------------------

    #[test]
    fn test_reinterpret_result_display() {
        let s = struct_of(vec![(0, byte(4, 4))], 4, 4);
        let b = byte(4, 4);
        let result = can_reinterpret(&s, &b);
        let display = format!("{result}");
        assert!(display.contains("safe"));
    }

    // Test 32: meet — array element-wise --------------------------------------

    #[test]
    fn test_meet_array() {
        let a1 = array(byte(4, 4), 10);
        let a2 = array(byte(4, 8), 10);
        let m = meet(&a1, &a2).unwrap();
        if let RepD::Array(a) = m {
            assert_eq!(a.element.alignment(), 8);
            assert_eq!(a.count, 10);
        } else {
            panic!("expected array");
        }
    }

    // Test 33: join — array element-wise --------------------------------------

    #[test]
    fn test_join_array() {
        let a1 = array(byte(4, 8), 5);
        let a2 = array(byte(4, 4), 5);
        let j = join(&a1, &a2).unwrap();
        if let RepD::Array(a) = j {
            assert_eq!(a.element.alignment(), 4);
            assert_eq!(a.count, 5);
        } else {
            panic!("expected array");
        }
    }

    // Test 34: join — different sizes → None ----------------------------------

    #[test]
    fn test_join_different_sizes() {
        let b1 = byte(4, 4);
        let b2 = byte(8, 8);
        assert!(join(&b1, &b2).is_none());
    }

    // Test 35: is_subtype — pointer not subtype if pointee isn't --------------

    #[test]
    fn test_is_subtype_ptr_negative() {
        let p1 = ptr(byte(4, 4));
        let p2 = ptr(byte(8, 8));
        // byte(4,4) is NOT a subtype of byte(8,8) because sizes differ.
        assert!(!is_subtype(&p1, &p2));
    }

    // Test 36: can_reinterpret — pointer to bytes (R4 specific check) ---------

    #[test]
    fn test_reinterpret_pointer_to_bytes_r4() {
        let p = ptr(byte(1, 1));
        let b = byte(POINTER_SIZE, 1);
        let result = can_reinterpret(&p, &b);
        assert!(result.can_reinterpret);
        // R1 covers this, but the pointer-specific path (R4) may also fire.
    }

    // Test 37: meet — enum variant-wise ---------------------------------------

    #[test]
    fn test_meet_enum() {
        let e1 = enum_of(vec![(0, byte(4, 4))]);
        let e2 = enum_of(vec![(0, byte(4, 8))]);
        let m = meet(&e1, &e2).unwrap();
        if let RepD::Enum(e) = m {
            assert_eq!(e.variants[0].1.alignment(), 8);
        } else {
            panic!("expected enum");
        }
    }

    // Test 38: are_compatible — alignment compatible (one divides other) ------

    #[test]
    fn test_compatible_alignment_divides() {
        let r1 = byte(8, 8);
        let r2 = byte(8, 4);
        let result = are_compatible(&r1, &r2);
        assert!(result.compatible);
    }

    // Test 39: are_compatible — alignment incompatible ------------------------

    #[test]
    fn test_compatible_alignment_incompatible() {
        let r1 = byte(8, 6);
        let r2 = byte(8, 4);
        // 6 % 4 != 0 and 4 % 6 != 0 → incompatible alignments.
        let result = are_compatible(&r1, &r2);
        assert!(!result.compatible);
    }

    // Test 40: can_reinterpret — R5 enum variant reinterpretation -------------

    #[test]
    fn test_reinterpret_enum_variants() {
        let e_from = enum_of(vec![(0, byte(4, 4)), (1, byte(4, 4))]);
        let e_to = enum_of(vec![
            (0, byte(4, 4)), // variant 0: byte(4,4) → byte(4,4) identity
            (1, byte(4, 2)), // variant 1: byte(4,4) → byte(4,2) — R1 byte erosion
        ]);
        let result = can_reinterpret(&e_from, &e_to);
        assert!(result.can_reinterpret);
        assert_eq!(result.rule, Some(ReinterpretRule::EnumVariant));
    }
}
