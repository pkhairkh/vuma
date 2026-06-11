//! BD Unification Engine
//!
//! This module implements constraint-based unification for Behavioral
//! Descriptors.  It provides:
//!
//! - [`BDVariable`] — symbolic variables standing for unknown BDs
//! - [`BDTerm`] — terms that are either concrete BDs or variables
//! - [`BDConstraint`] — constraints between BD terms (equality, compatibility,
//!   subtyping)
//! - [`BDSolver`] — a constraint solver using unification
//! - [`unify`] — unify two concrete behavioral descriptors
//! - [`solve_constraints`] — solve a system of constraints
//! - [`substitute`] — apply a substitution to a BD
//!
//! # Unification rules
//!
//! | Layer | Equality unification | Result |
//! |-------|----------------------|--------|
//! | RepD  | Same constructor, unify fields recursively | Unified RepD |
//! | CapD  | Meet (intersection of capabilities, union of conditions) | Most restrictive common CapD |
//! | RelD  | Merge (intersection of relations) + consistency check | Greatest common refinement |
//!
//! # Constraint kinds
//!
//! - **Equality** (`=`): two BDs must describe the exact same value.
//! - **Compatibility** (`~`): two BDs must be able to safely alias.
//! - **Subtyping** (`<:`): the left BD must refine the right BD.
//!
//! # Occurs check
//!
//! The solver performs an occurs check when binding a variable to a term,
//! preventing the creation of infinite (recursive) types.  With the current
//! fully-concrete `BD` representation this check is always vacuously
//! satisfied, but it guards against future extensions where BDs may embed
//! variable references.

use crate::capd::CapD;
use crate::descriptor::BD;
use crate::reld::RelD;
use crate::repd::{ArrayRep, ByteRep, EnumRep, FuncRep, PtrRep, RepD, StructRep, UnionRep};
use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// BDVariable
// ---------------------------------------------------------------------------

/// A symbolic variable standing for an unknown Behavioral Descriptor.
///
/// Variables are identified by a unique `id` and may carry a human-readable
/// `name` for debugging.  Two variables are equal iff their `id`s match.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BDVariable {
    /// Unique identifier for this variable.
    pub id: u64,
    /// Optional human-readable name.
    pub name: String,
}

impl BDVariable {
    /// Create a new variable with the given id and name.
    pub fn new(id: u64, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }

    /// Create an anonymous variable with the given id.
    pub fn anon(id: u64) -> Self {
        Self {
            id,
            name: format!("_{id}"),
        }
    }
}

impl fmt::Display for BDVariable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "?{}", self.name)
    }
}

// ---------------------------------------------------------------------------
// BDTerm
// ---------------------------------------------------------------------------

/// A term in the BD constraint system — either a concrete [`BD`] or a
/// symbolic [`BDVariable`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BDTerm {
    /// A concrete, fully-specified behavioral descriptor.
    Concrete(BD),
    /// A symbolic variable standing for an unknown BD.
    Var(BDVariable),
}

impl BDTerm {
    /// Returns `true` if this term is a concrete BD (not a variable).
    pub fn is_concrete(&self) -> bool {
        matches!(self, BDTerm::Concrete(_))
    }

    /// Returns `true` if this term is a variable.
    pub fn is_var(&self) -> bool {
        matches!(self, BDTerm::Var(_))
    }

    /// Extract the concrete BD, if this term is `Concrete`.
    pub fn as_concrete(&self) -> Option<&BD> {
        match self {
            BDTerm::Concrete(bd) => Some(bd),
            BDTerm::Var(_) => None,
        }
    }

    /// Extract the variable, if this term is `Var`.
    pub fn as_var(&self) -> Option<&BDVariable> {
        match self {
            BDTerm::Concrete(_) => None,
            BDTerm::Var(v) => Some(v),
        }
    }
}

impl fmt::Display for BDTerm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BDTerm::Concrete(bd) => write!(f, "{bd}"),
            BDTerm::Var(v) => write!(f, "{v}"),
        }
    }
}

// ---------------------------------------------------------------------------
// BDConstraintKind
// ---------------------------------------------------------------------------

/// The kind of constraint between two BD terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BDConstraintKind {
    /// The two terms must be *equal* — they describe the exact same BD.
    Equality,
    /// The two terms must be *compatible* — they can safely describe the
    /// same value.
    Compatibility,
    /// The left term must *refine* (be at least as specific as) the right
    /// term.
    Subtyping,
}

impl fmt::Display for BDConstraintKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BDConstraintKind::Equality => write!(f, "="),
            BDConstraintKind::Compatibility => write!(f, "~"),
            BDConstraintKind::Subtyping => write!(f, "<:"),
        }
    }
}

// ---------------------------------------------------------------------------
// BDConstraint
// ---------------------------------------------------------------------------

/// A constraint between two BD terms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BDConstraint {
    /// Left-hand side of the constraint.
    pub left: BDTerm,
    /// Right-hand side of the constraint.
    pub right: BDTerm,
    /// Kind of constraint.
    pub kind: BDConstraintKind,
}

impl BDConstraint {
    /// Create a new constraint.
    pub fn new(left: BDTerm, right: BDTerm, kind: BDConstraintKind) -> Self {
        Self { left, right, kind }
    }

    /// Create an equality constraint.
    pub fn equality(left: BDTerm, right: BDTerm) -> Self {
        Self::new(left, right, BDConstraintKind::Equality)
    }

    /// Create a compatibility constraint.
    pub fn compatibility(left: BDTerm, right: BDTerm) -> Self {
        Self::new(left, right, BDConstraintKind::Compatibility)
    }

    /// Create a subtyping constraint (left refines right).
    pub fn subtyping(left: BDTerm, right: BDTerm) -> Self {
        Self::new(left, right, BDConstraintKind::Subtyping)
    }
}

impl fmt::Display for BDConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.left, self.kind, self.right)
    }
}

// ---------------------------------------------------------------------------
// UnificationError
// ---------------------------------------------------------------------------

/// Errors that can arise during BD unification.
#[derive(Debug, Clone, PartialEq)]
pub enum UnificationError {
    /// Representation descriptors have incompatible constructors or fields.
    IncompatibleRepD {
        /// String representation of the first RepD.
        repd1: String,
        /// String representation of the second RepD.
        repd2: String,
        /// Why the two RepDs are incompatible.
        reason: String,
    },
    /// Capability descriptors cannot be reconciled (empty meet with
    /// non-empty inputs).
    IncompatibleCapD {
        /// String representation of the first CapD.
        capd1: String,
        /// String representation of the second CapD.
        capd2: String,
    },
    /// Relational descriptors are internally inconsistent when composed.
    InconsistentRelD {
        /// String representation of the first RelD.
        reld1: String,
        /// String representation of the second RelD.
        reld2: String,
    },
    /// Occurs check failed — would create an infinite type.
    OccursCheckFailed {
        /// The variable that occurs within the term.
        var: BDVariable,
        /// The term in which the variable was found.
        term: String,
    },
    /// A variable has conflicting bindings that cannot be reconciled.
    ConflictingBinding {
        /// The variable with conflicting bindings.
        var: BDVariable,
        /// String representation of the existing binding.
        existing: String,
        /// String representation of the proposed binding.
        proposed: String,
    },
    /// A subtyping constraint is violated: left does not refine right.
    SubtypeViolation {
        /// String representation of the subtype.
        sub: String,
        /// String representation of the supertype.
        sup: String,
    },
    /// General unification failure with a descriptive message.
    Failed(String),
}

impl fmt::Display for UnificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnificationError::IncompatibleRepD {
                repd1,
                repd2,
                reason,
            } => {
                write!(f, "incompatible RepD: {repd1} vs {repd2}: {reason}")
            }
            UnificationError::IncompatibleCapD { capd1, capd2 } => {
                write!(f, "incompatible CapD: {capd1} vs {capd2}")
            }
            UnificationError::InconsistentRelD { reld1, reld2 } => {
                write!(f, "inconsistent RelD: {reld1} vs {reld2}")
            }
            UnificationError::OccursCheckFailed { var, term } => {
                write!(f, "occurs check failed: {var} occurs in {term}")
            }
            UnificationError::ConflictingBinding {
                var,
                existing,
                proposed,
            } => {
                write!(f, "conflicting binding for {var}: {existing} vs {proposed}")
            }
            UnificationError::SubtypeViolation { sub, sup } => {
                write!(f, "subtype violation: {sub} is not a subtype of {sup}")
            }
            UnificationError::Failed(msg) => write!(f, "unification failed: {msg}"),
        }
    }
}

impl std::error::Error for UnificationError {}

// ---------------------------------------------------------------------------
// Core unification of concrete BDs
// ---------------------------------------------------------------------------

/// Unify two concrete behavioral descriptors, producing the most specific
/// common descriptor that is compatible with both.
///
/// # Unification strategy
///
/// | Layer | Operation | Meaning |
/// |-------|-----------|---------|
/// | RepD  | Structural unification | Same constructor, unify fields |
/// | CapD  | Meet | Intersection of capabilities, union of conditions |
/// | RelD  | Merge + consistency | Intersection of relations, must be consistent |
///
/// # Errors
///
/// Returns [`UnificationError`] if the BDs are structurally incompatible
/// (different RepD constructors, empty capability meet, or inconsistent
/// relational merge).
pub fn unify(bd1: &BD, bd2: &BD) -> Result<BD, UnificationError> {
    let mut visited = HashSet::new();
    let repd = unify_repd_with_occurs(&bd1.repd, &bd2.repd, &mut visited)?;
    let capd = unify_capd(&bd1.capd, &bd2.capd)?;
    let reld = unify_reld(&bd1.reld, &bd2.reld)?;
    Ok(BD::new(repd, capd, reld))
}

/// Unify two representation descriptors with occurs-check for recursive types.
///
/// Both must have the same top-level constructor variant.  Fields are
/// unified recursively.  The result preserves the structural shape with
/// each field being the unification of the corresponding input fields.
///
/// The `visited` set tracks (pointer, pointer) pairs of RepDs already
/// being unified to detect recursive type cycles.  If we encounter the
/// same pair again during recursive descent, we assume the recursive
/// structure unifies coinductively (returning the first operand) rather
/// than looping infinitely.
fn unify_repd_with_occurs(
    r1: &RepD,
    r2: &RepD,
    visited: &mut HashSet<(usize, usize)>,
) -> Result<RepD, UnificationError> {
    // Occurs-check: if we've already started unifying this exact pair,
    // assume they unify coinductively (standard treatment for recursive types).
    let ptr1 = r1 as *const RepD as usize;
    let ptr2 = r2 as *const RepD as usize;
    let key = if ptr1 < ptr2 {
        (ptr1, ptr2)
    } else {
        (ptr2, ptr1)
    };
    if visited.contains(&key) {
        // Recursive occurrence — assume unification succeeds coinductively
        return Ok(r1.clone());
    }
    visited.insert(key);

    let result = unify_repd_inner(r1, r2, visited)?;

    // Remove the key after returning so other branches can re-explore
    // different paths through the type graph.
    visited.remove(&key);
    Ok(result)
}

/// Inner RepD unification logic (called after occurs-check).
fn unify_repd_inner(
    r1: &RepD,
    r2: &RepD,
    visited: &mut HashSet<(usize, usize)>,
) -> Result<RepD, UnificationError> {
    match (r1, r2) {
        // Byte: sizes and alignments must match exactly.
        (RepD::Byte(a), RepD::Byte(b)) => {
            if a.size != b.size || a.align != b.align {
                return Err(UnificationError::IncompatibleRepD {
                    repd1: format!("{r1}"),
                    repd2: format!("{r2}"),
                    reason: format!(
                        "byte size/alignment mismatch: ({}, {}) vs ({}, {})",
                        a.size, a.align, b.size, b.align
                    ),
                });
            }
            Ok(RepD::Byte(ByteRep {
                size: a.size,
                align: a.align,
            }))
        }

        // Struct: same number of fields, same offsets, unify each field.
        (RepD::Struct(a), RepD::Struct(b)) => {
            if a.fields.len() != b.fields.len() {
                return Err(UnificationError::IncompatibleRepD {
                    repd1: format!("{r1}"),
                    repd2: format!("{r2}"),
                    reason: format!(
                        "struct field count mismatch: {} vs {}",
                        a.fields.len(),
                        b.fields.len()
                    ),
                });
            }
            let mut fields = Vec::with_capacity(a.fields.len());
            for ((off_a, rep_a), (off_b, rep_b)) in a.fields.iter().zip(&b.fields) {
                if off_a != off_b {
                    return Err(UnificationError::IncompatibleRepD {
                        repd1: format!("{r1}"),
                        repd2: format!("{r2}"),
                        reason: format!("struct field offset mismatch: {off_a} vs {off_b}"),
                    });
                }
                fields.push((*off_a, unify_repd_with_occurs(rep_a, rep_b, visited)?));
            }
            Ok(RepD::Struct(StructRep {
                fields,
                total_size: a.total_size,
                align: a.align,
            }))
        }

        // Array: same count, unify element representations.
        (RepD::Array(a), RepD::Array(b)) => {
            if a.count != b.count {
                return Err(UnificationError::IncompatibleRepD {
                    repd1: format!("{r1}"),
                    repd2: format!("{r2}"),
                    reason: format!("array count mismatch: {} vs {}", a.count, b.count),
                });
            }
            let element = unify_repd_with_occurs(&a.element, &b.element, visited)?;
            Ok(RepD::Array(ArrayRep {
                element: Box::new(element),
                count: a.count,
            }))
        }

        // Enum: same number of variants with matching tags.
        (RepD::Enum(a), RepD::Enum(b)) => {
            if a.variants.len() != b.variants.len() {
                return Err(UnificationError::IncompatibleRepD {
                    repd1: format!("{r1}"),
                    repd2: format!("{r2}"),
                    reason: format!(
                        "enum variant count mismatch: {} vs {}",
                        a.variants.len(),
                        b.variants.len()
                    ),
                });
            }
            let mut variants = Vec::with_capacity(a.variants.len());
            for ((tag_a, rep_a), (tag_b, rep_b)) in a.variants.iter().zip(&b.variants) {
                if tag_a != tag_b {
                    return Err(UnificationError::IncompatibleRepD {
                        repd1: format!("{r1}"),
                        repd2: format!("{r2}"),
                        reason: format!("enum tag mismatch: {tag_a} vs {tag_b}"),
                    });
                }
                variants.push((*tag_a, unify_repd_with_occurs(rep_a, rep_b, visited)?));
            }
            Ok(RepD::Enum(EnumRep { variants }))
        }

        // Ptr: unify pointee representations.
        (RepD::Ptr(a), RepD::Ptr(b)) => {
            let pointee = unify_repd_with_occurs(&a.pointee, &b.pointee, visited)?;
            Ok(RepD::Ptr(PtrRep {
                pointee: Box::new(pointee),
            }))
        }

        // Union: same number of alternatives, unify pairwise.
        (RepD::Union(a), RepD::Union(b)) => {
            if a.alternatives.len() != b.alternatives.len() {
                return Err(UnificationError::IncompatibleRepD {
                    repd1: format!("{r1}"),
                    repd2: format!("{r2}"),
                    reason: format!(
                        "union alternative count mismatch: {} vs {}",
                        a.alternatives.len(),
                        b.alternatives.len()
                    ),
                });
            }
            let mut alternatives = Vec::with_capacity(a.alternatives.len());
            for (alt_a, alt_b) in a.alternatives.iter().zip(&b.alternatives) {
                alternatives.push(unify_repd_with_occurs(alt_a, alt_b, visited)?);
            }
            Ok(RepD::Union(UnionRep {
                alternatives,
                max_size: a.max_size,
                max_align: a.max_align,
            }))
        }

        // Func: same number of params, unify each + result.
        (RepD::Func(a), RepD::Func(b)) => {
            if a.params.len() != b.params.len() {
                return Err(UnificationError::IncompatibleRepD {
                    repd1: format!("{r1}"),
                    repd2: format!("{r2}"),
                    reason: format!(
                        "function param count mismatch: {} vs {}",
                        a.params.len(),
                        b.params.len()
                    ),
                });
            }
            let mut params = Vec::with_capacity(a.params.len());
            for (p_a, p_b) in a.params.iter().zip(&b.params) {
                params.push(unify_repd_with_occurs(p_a, p_b, visited)?);
            }
            let result = unify_repd_with_occurs(&a.result, &b.result, visited)?;
            Ok(RepD::Func(FuncRep {
                params,
                result: Box::new(result),
            }))
        }

        // Generic can be unified with any RepD (substitution).
        // When unifying a Generic with a concrete RepD, return the concrete one.
        (RepD::Generic { .. }, other) | (other, RepD::Generic { .. }) => Ok(other.clone()),

        // Different constructors: incompatible.
        _ => Err(UnificationError::IncompatibleRepD {
            repd1: format!("{r1}"),
            repd2: format!("{r2}"),
            reason: "different RepD constructors".to_string(),
        }),
    }
}

/// Unify two capability descriptors by computing their **meet**
/// (intersection of capabilities, union of conditions).
///
/// The meet represents the most specific set of capabilities that both
/// descriptors agree on.  If both inputs have non-empty capability sets
/// but the meet is empty, the descriptors are incompatible and an error
/// is returned.
fn unify_capd(c1: &CapD, c2: &CapD) -> Result<CapD, UnificationError> {
    let result = c1.meet(c2);
    // If both sides had capabilities but the meet is empty, they share
    // no common capability — this means they cannot describe the same
    // value, so unification fails.
    if !c1.caps.is_empty() && !c2.caps.is_empty() && result.caps.is_empty() {
        return Err(UnificationError::IncompatibleCapD {
            capd1: format!("{c1}"),
            capd2: format!("{c2}"),
        });
    }
    Ok(result)
}

/// Unify two relational descriptors by computing their **merge**
/// (intersection of relations) and checking internal consistency.
///
/// Only relations agreed upon by both sides survive, representing the
/// greatest common refinement.  If the merged result is internally
/// inconsistent (e.g., contradictory temporal constraints), an error
/// is returned.
fn unify_reld(r1: &RelD, r2: &RelD) -> Result<RelD, UnificationError> {
    let merged = r1.merge(r2);
    if !merged.is_consistent() {
        return Err(UnificationError::InconsistentRelD {
            reld1: format!("{r1}"),
            reld2: format!("{r2}"),
        });
    }
    Ok(merged)
}

// ---------------------------------------------------------------------------
// Substitution
// ---------------------------------------------------------------------------

/// Apply a substitution to a concrete BD, replacing any variable references.
///
/// Since `BD` itself does not contain variables (it is fully concrete),
/// this function returns the BD unchanged.  It is included for API
/// completeness and for use with extended BD types that may embed variables.
pub fn substitute(bd: &BD, _subst: &HashMap<BDVariable, BD>) -> BD {
    bd.clone()
}

/// Apply a substitution to a [`BDTerm`], resolving variables through the
/// substitution map by chasing variable chains until a concrete BD or an
/// unbound variable is reached.
pub fn substitute_term(term: &BDTerm, subst: &HashMap<BDVariable, BDTerm>) -> BDTerm {
    match term {
        BDTerm::Concrete(bd) => BDTerm::Concrete(bd.clone()),
        BDTerm::Var(v) => match subst.get(v) {
            Some(resolved) => substitute_term(resolved, subst),
            None => BDTerm::Var(v.clone()),
        },
    }
}

/// Compose two substitutions: `compose(s1, s2)` produces a substitution `s`
/// such that applying `s` is equivalent to applying `s1` then `s2`.
///
/// For each variable `v`:
/// - If `v` appears in `s1`, the result maps `v` to `s2(s1[v])`.
/// - If `v` appears only in `s2`, the result maps `v` to `s2[v]`.
/// - If `v` appears in neither, it is absent from the result.
pub fn compose_subst(
    s1: &HashMap<BDVariable, BDTerm>,
    s2: &HashMap<BDVariable, BDTerm>,
) -> HashMap<BDVariable, BDTerm> {
    let mut result = HashMap::new();
    // All bindings from s1, with s2 applied to their right-hand sides.
    for (v, t) in s1 {
        result.insert(v.clone(), substitute_term(t, s2));
    }
    // All bindings from s2 that are not already in the result.
    for (v, t) in s2 {
        if !result.contains_key(v) {
            result.insert(v.clone(), t.clone());
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Occurs check
// ---------------------------------------------------------------------------

/// Check whether a variable occurs in a term.
///
/// This prevents the creation of infinite types (e.g., `X = BD` containing
/// `X`).  For concrete BDs the check always returns `false` since BDs do
/// not contain variables.  For variable terms, we check transitively through
/// the substitution.
fn occurs_in(var: &BDVariable, term: &BDTerm, subst: &HashMap<BDVariable, BDTerm>) -> bool {
    match term {
        BDTerm::Concrete(_) => false,
        BDTerm::Var(v) => {
            if var == v {
                return true;
            }
            match subst.get(v) {
                Some(resolved) => occurs_in(var, resolved, subst),
                None => false,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// BDSolver
// ---------------------------------------------------------------------------

/// A constraint solver for BD terms using unification.
///
/// The solver maintains a substitution (mapping from variables to terms)
/// and processes constraints one at a time, extending the substitution
/// as needed.
///
/// # Algorithm
///
/// For each constraint:
/// 1. Resolve both sides through the current substitution.
/// 2. If both sides are identical, the constraint is trivially satisfied.
/// 3. If both are concrete BDs, check the constraint directly using the
///    appropriate BD method.
/// 4. If one side is a variable, bind it to the other side (with occurs
///    check).
/// 5. If both are variables, bind one to the other.
///
/// # Example
///
/// ```
/// use vuma_bd::unify::*;
/// use vuma_bd::descriptor::BD;
/// use vuma_bd::repd::{RepD, ByteRep};
/// use vuma_bd::capd::CapD;
/// use vuma_bd::reld::RelD;
///
/// let var = BDVariable::new(0, "T");
/// let bd = BD::new(
///     RepD::Byte(ByteRep { size: 4, align: 4 }),
///     CapD::empty(),
///     RelD::empty(),
/// );
///
/// let constraint = BDConstraint::equality(
///     BDTerm::Var(var.clone()),
///     BDTerm::Concrete(bd.clone()),
/// );
///
/// let result = solve_constraints(vec![constraint]).unwrap();
/// assert_eq!(result.get(&var), Some(&bd));
/// ```
#[derive(Debug, Clone)]
pub struct BDSolver {
    /// Current substitution: variable → term.
    subst: HashMap<BDVariable, BDTerm>,
}

impl BDSolver {
    /// Create a new solver with an empty substitution.
    pub fn new() -> Self {
        Self {
            subst: HashMap::new(),
        }
    }

    /// Returns a reference to the current substitution.
    pub fn substitution(&self) -> &HashMap<BDVariable, BDTerm> {
        &self.subst
    }

    /// Resolve a term through the current substitution, chasing variable
    /// chains until a concrete BD or an unbound variable is reached.
    pub fn resolve(&self, term: &BDTerm) -> BDTerm {
        substitute_term(term, &self.subst)
    }

    /// Resolve a variable to its bound term, if any.
    ///
    /// Follows variable chains transitively.
    pub fn resolve_var(&self, var: &BDVariable) -> Option<BDTerm> {
        match self.subst.get(var) {
            Some(BDTerm::Var(v2)) => {
                // Chase the chain; avoid infinite loops for circular refs.
                let mut visited = HashMap::new();
                visited.insert(var.id, ());
                let mut current = v2;
                loop {
                    if visited.contains_key(&current.id) {
                        // Circular reference — return what we have.
                        return Some(BDTerm::Var(current.clone()));
                    }
                    visited.insert(current.id, ());
                    match self.subst.get(current) {
                        Some(BDTerm::Var(v3)) => current = v3,
                        Some(other) => return Some(other.clone()),
                        None => return Some(BDTerm::Var(current.clone())),
                    }
                }
            }
            Some(other) => Some(other.clone()),
            None => None,
        }
    }

    /// Add and process a single constraint.
    ///
    /// Returns `Ok(())` if the constraint is satisfied, `Err` otherwise.
    /// On success the internal substitution may have been extended.
    pub fn add_constraint(&mut self, constraint: &BDConstraint) -> Result<(), UnificationError> {
        let left = self.resolve(&constraint.left);
        let right = self.resolve(&constraint.right);

        match constraint.kind {
            BDConstraintKind::Equality => self.unify_terms(&left, &right),
            BDConstraintKind::Compatibility => self.check_compatibility(&left, &right),
            BDConstraintKind::Subtyping => self.check_subtyping(&left, &right),
        }
    }

    /// Unify two terms under an equality constraint.
    fn unify_terms(&mut self, t1: &BDTerm, t2: &BDTerm) -> Result<(), UnificationError> {
        // Trivial case: identical terms (after resolution).
        if t1 == t2 {
            return Ok(());
        }

        match (t1, t2) {
            // Both concrete: unify the BDs.
            (BDTerm::Concrete(bd1), BDTerm::Concrete(bd2)) => {
                unify(bd1, bd2)?;
                Ok(())
            }

            // Left is variable: bind it.
            (BDTerm::Var(v), right) => self.bind_variable(v, right),

            // Right is variable: bind it.
            (left, BDTerm::Var(v)) => self.bind_variable(v, left),
        }
    }

    /// Bind a variable to a term, with occurs check.
    ///
    /// If the variable is already bound, the existing binding and the
    /// proposed binding are unified instead.
    fn bind_variable(&mut self, var: &BDVariable, term: &BDTerm) -> Result<(), UnificationError> {
        // Occurs check: prevent infinite types.
        if occurs_in(var, term, &self.subst) {
            return Err(UnificationError::OccursCheckFailed {
                var: var.clone(),
                term: format!("{term}"),
            });
        }

        // If the variable is already bound, unify the existing and
        // proposed bindings.
        if let Some(existing) = self.subst.get(var) {
            let existing_resolved = self.resolve(existing);
            let term_resolved = self.resolve(term);
            if existing_resolved == term_resolved {
                return Ok(());
            }
            return self.unify_terms(&existing_resolved, &term_resolved);
        }

        self.subst.insert(var.clone(), term.clone());
        Ok(())
    }

    /// Check compatibility between two resolved terms.
    ///
    /// For concrete BDs, this uses [`BD::compatible`].  For variables,
    /// compatibility is conservatively assumed (the check is deferred
    /// until the variable is bound).
    fn check_compatibility(&self, t1: &BDTerm, t2: &BDTerm) -> Result<(), UnificationError> {
        match (t1, t2) {
            (BDTerm::Concrete(bd1), BDTerm::Concrete(bd2)) => {
                if bd1.compatible(bd2) {
                    Ok(())
                } else {
                    Err(UnificationError::Failed(format!(
                        "incompatible BDs: {} vs {}",
                        t1, t2
                    )))
                }
            }
            // Variables: conservatively assume compatible.
            (BDTerm::Var(_), _) | (_, BDTerm::Var(_)) => Ok(()),
        }
    }

    /// Check subtyping: `t1` must refine `t2`.
    ///
    /// For concrete BDs, this uses [`BD::refines`].  For variables,
    /// subtyping is conservatively assumed (deferred).
    fn check_subtyping(&self, t1: &BDTerm, t2: &BDTerm) -> Result<(), UnificationError> {
        match (t1, t2) {
            (BDTerm::Concrete(bd1), BDTerm::Concrete(bd2)) => {
                if bd1.refines(bd2) {
                    Ok(())
                } else {
                    Err(UnificationError::SubtypeViolation {
                        sub: format!("{bd1}"),
                        sup: format!("{bd2}"),
                    })
                }
            }
            // Variables: conservatively assume subtype holds.
            (BDTerm::Var(_), _) | (_, BDTerm::Var(_)) => Ok(()),
        }
    }

    /// Solve a batch of constraints, returning the final substitution
    /// mapping each variable to its resolved concrete BD (if possible).
    ///
    /// Returns `Ok(substitution)` if all constraints are satisfied.
    /// Returns `Err(errors)` with all accumulated errors otherwise.
    pub fn solve(
        &mut self,
        constraints: &[BDConstraint],
    ) -> Result<HashMap<BDVariable, BD>, Vec<UnificationError>> {
        let mut errors = Vec::new();
        for constraint in constraints {
            if let Err(e) = self.add_constraint(constraint) {
                errors.push(e);
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        // Collect all variables that appear in the substitution.
        let vars: Vec<BDVariable> = self.subst.keys().cloned().collect();
        let mut result = HashMap::new();
        for var in vars {
            let resolved = self.resolve(&BDTerm::Var(var.clone()));
            match resolved {
                BDTerm::Concrete(bd) => {
                    result.insert(var, bd);
                }
                BDTerm::Var(v) => {
                    // Variable is still free after solving — leave it out
                    // of the concrete result map.  If we wanted to report
                    // free variables we could do so here.
                    let _ = v;
                }
            }
        }
        Ok(result)
    }

    /// Finalize the substitution: collapse variable chains so that every
    /// entry maps directly to a concrete BD (if bound) or to the
    /// representative variable of its equivalence class.
    pub fn finalize(&mut self) {
        let vars: Vec<BDVariable> = self.subst.keys().cloned().collect();
        for var in vars {
            let resolved = self.resolve(&BDTerm::Var(var.clone()));
            self.subst.insert(var, resolved);
        }
    }
}

impl Default for BDSolver {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// solve_constraints — top-level API
// ---------------------------------------------------------------------------

/// Solve a system of BD constraints, returning a mapping from each variable
/// to its unified concrete BD.
///
/// This is a convenience wrapper around [`BDSolver::solve`].
///
/// # Errors
///
/// Returns a vector of all unification errors encountered during solving.
pub fn solve_constraints(
    constraints: Vec<BDConstraint>,
) -> Result<HashMap<BDVariable, BD>, Vec<UnificationError>> {
    let mut solver = BDSolver::new();
    solver.solve(&constraints)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capd::Capability;
    use crate::reld::Relation;

    // -- Helpers ----------------------------------------------------------

    fn byte_rep(size: u64, align: u64) -> RepD {
        RepD::Byte(ByteRep { size, align })
    }

    fn read_cap() -> CapD {
        CapD::empty().strengthen(&[Capability::Read])
    }

    fn read_write_cap() -> CapD {
        CapD::empty().strengthen(&[Capability::Read, Capability::Write])
    }

    fn exec_cap() -> CapD {
        CapD::empty().strengthen(&[Capability::Execute])
    }

    fn read_exec_cap() -> CapD {
        CapD::empty().strengthen(&[Capability::Read, Capability::Execute])
    }

    fn empty_reld() -> RelD {
        RelD::empty()
    }

    fn liveness_reld() -> RelD {
        RelD {
            relations: [Relation::Liveness].into_iter().collect(),
        }
    }

    fn containment_reld() -> RelD {
        RelD {
            relations: [Relation::Containment].into_iter().collect(),
        }
    }

    fn make_bd(repd: RepD, capd: CapD, reld: RelD) -> BD {
        BD::new(repd, capd, reld)
    }

    // -- Test 1: Unify identical BDs ------------------------------------

    #[test]
    fn unify_identical_bds() {
        let a = make_bd(byte_rep(4, 4), read_cap(), empty_reld());
        let b = make_bd(byte_rep(4, 4), read_cap(), empty_reld());
        let result = unify(&a, &b).expect("identical BDs should unify");
        assert_eq!(result.repd.size(), 4);
        assert!(result.capd.caps.contains(&Capability::Read));
        assert!(result.reld.relations.is_empty());
    }

    // -- Test 2: Unify BDs with overlapping capabilities ----------------

    #[test]
    fn unify_overlapping_capabilities() {
        let a = make_bd(byte_rep(8, 8), read_write_cap(), empty_reld());
        let b = make_bd(byte_rep(8, 8), read_exec_cap(), empty_reld());
        let result = unify(&a, &b).expect("overlapping caps should unify");
        // Meet of {Read, Write} ∩ {Read, Execute} = {Read}
        assert!(result.capd.caps.contains(&Capability::Read));
        assert!(!result.capd.caps.contains(&Capability::Write));
        assert!(!result.capd.caps.contains(&Capability::Execute));
    }

    // -- Test 3: Unify BDs with incompatible representations ------------

    #[test]
    fn unify_incompatible_repd() {
        let a = make_bd(byte_rep(4, 4), read_cap(), empty_reld());
        let b = make_bd(
            RepD::Ptr(PtrRep {
                pointee: Box::new(byte_rep(1, 1)),
            }),
            read_cap(),
            empty_reld(),
        );
        let err = unify(&a, &b).unwrap_err();
        assert!(matches!(err, UnificationError::IncompatibleRepD { .. }));
    }

    // -- Test 4: Unify BDs with disjoint capabilities -------------------

    #[test]
    fn unify_disjoint_capabilities() {
        let a = make_bd(byte_rep(4, 4), exec_cap(), empty_reld());
        let b = make_bd(byte_rep(4, 4), read_cap(), empty_reld());
        let err = unify(&a, &b).unwrap_err();
        assert!(matches!(err, UnificationError::IncompatibleCapD { .. }));
    }

    // -- Test 5: Solver with variable binding ---------------------------

    #[test]
    fn solver_variable_binding() {
        let var = BDVariable::new(0, "T");
        let bd = make_bd(byte_rep(4, 4), read_cap(), empty_reld());
        let constraint =
            BDConstraint::equality(BDTerm::Var(var.clone()), BDTerm::Concrete(bd.clone()));
        let result = solve_constraints(vec![constraint]).expect("should solve");
        assert_eq!(result.get(&var), Some(&bd));
    }

    // -- Test 6: Solver with two variables unified ----------------------

    #[test]
    fn solver_two_variables() {
        let var_a = BDVariable::new(0, "A");
        let var_b = BDVariable::new(1, "B");
        let bd = make_bd(byte_rep(8, 8), read_cap(), empty_reld());

        let constraints = vec![
            BDConstraint::equality(BDTerm::Var(var_a.clone()), BDTerm::Concrete(bd.clone())),
            BDConstraint::equality(BDTerm::Var(var_b.clone()), BDTerm::Var(var_a.clone())),
        ];

        let result = solve_constraints(constraints).expect("should solve");
        assert_eq!(result.get(&var_a), Some(&bd));
        assert_eq!(result.get(&var_b), Some(&bd));
    }

    // -- Test 7: Compatibility constraint with concrete BDs -------------

    #[test]
    fn compatibility_constraint_passes() {
        let a = make_bd(byte_rep(8, 8), read_write_cap(), empty_reld());
        let b = make_bd(byte_rep(8, 8), read_cap(), empty_reld());
        let constraint = BDConstraint::compatibility(BDTerm::Concrete(a), BDTerm::Concrete(b));
        let result = solve_constraints(vec![constraint]);
        assert!(result.is_ok());
    }

    // -- Test 8: Subtyping constraint satisfied --------------------------

    #[test]
    fn subtyping_constraint_satisfied() {
        // read_cap ⊆ read_write_cap → a refines b.
        let a = make_bd(byte_rep(4, 4), read_cap(), liveness_reld());
        let b = make_bd(byte_rep(4, 4), read_write_cap(), liveness_reld());
        let constraint = BDConstraint::subtyping(BDTerm::Concrete(a), BDTerm::Concrete(b));
        let result = solve_constraints(vec![constraint]);
        assert!(result.is_ok());
    }

    // -- Test 9: Subtyping constraint violated ---------------------------

    #[test]
    fn subtyping_constraint_violated() {
        // read_write_cap ⊄ read_cap → a does NOT refine b.
        let a = make_bd(byte_rep(4, 4), read_write_cap(), liveness_reld());
        let b = make_bd(byte_rep(4, 4), read_cap(), liveness_reld());
        let constraint = BDConstraint::subtyping(BDTerm::Concrete(a), BDTerm::Concrete(b));
        let result = solve_constraints(vec![constraint]);
        assert!(result.is_err());
    }

    // -- Test 10: Substitute term through substitution -------------------

    #[test]
    fn substitute_term_resolves_variable() {
        let var = BDVariable::new(0, "T");
        let bd = make_bd(byte_rep(4, 4), read_cap(), empty_reld());
        let mut subst = HashMap::new();
        subst.insert(var.clone(), BDTerm::Concrete(bd.clone()));

        let term = BDTerm::Var(var);
        let resolved = substitute_term(&term, &subst);
        assert_eq!(resolved, BDTerm::Concrete(bd));
    }

    // -- Test 11: Compose substitutions ----------------------------------

    #[test]
    fn compose_substitutions() {
        let var_x = BDVariable::new(0, "X");
        let var_y = BDVariable::new(1, "Y");
        let bd = make_bd(byte_rep(8, 8), read_cap(), empty_reld());

        let mut s1 = HashMap::new();
        s1.insert(var_x.clone(), BDTerm::Var(var_y.clone()));

        let mut s2 = HashMap::new();
        s2.insert(var_y.clone(), BDTerm::Concrete(bd.clone()));

        let composed = compose_subst(&s1, &s2);
        let resolved = substitute_term(&BDTerm::Var(var_x), &composed);
        assert_eq!(resolved, BDTerm::Concrete(bd));
    }

    // -- Test 12: Struct RepD unification --------------------------------

    #[test]
    fn struct_repd_unification() {
        let s1 = RepD::Struct(StructRep {
            fields: vec![(0u64, byte_rep(4, 4)), (4u64, byte_rep(2, 2))],
            total_size: 8,
            align: 4,
        });
        let s2 = RepD::Struct(StructRep {
            fields: vec![(0u64, byte_rep(4, 4)), (4u64, byte_rep(2, 2))],
            total_size: 8,
            align: 4,
        });
        let result = unify_repd_with_occurs(&s1, &s2, &mut HashSet::new())
            .expect("identical structs should unify");
        assert_eq!(result.size(), 8);
    }

    // -- Test 13: Array RepD count mismatch ------------------------------

    #[test]
    fn array_repd_count_mismatch() {
        let a1 = RepD::Array(ArrayRep {
            element: Box::new(byte_rep(4, 4)),
            count: 10,
        });
        let a2 = RepD::Array(ArrayRep {
            element: Box::new(byte_rep(4, 4)),
            count: 20,
        });
        let result = unify_repd_with_occurs(&a1, &a2, &mut HashSet::new());
        assert!(result.is_err());
    }

    // -- Test 14: Ptr RepD unification -----------------------------------

    #[test]
    fn ptr_repd_unification() {
        let p1 = RepD::Ptr(PtrRep {
            pointee: Box::new(byte_rep(4, 4)),
        });
        let p2 = RepD::Ptr(PtrRep {
            pointee: Box::new(byte_rep(4, 4)),
        });
        let result = unify_repd_with_occurs(&p1, &p2, &mut HashSet::new())
            .expect("identical ptrs should unify");
        assert_eq!(result.size(), 8); // pointer size
    }

    // -- Test 15: RelD merge produces intersection -----------------------

    #[test]
    fn reld_merge_unification() {
        let r1 = liveness_reld();
        let r2 = containment_reld();
        let result = unify_reld(&r1, &r2).expect("disjoint relds should unify to empty merge");
        // Merge = intersection = ∅ (liveness ∩ containment = nothing)
        assert!(result.relations.is_empty());
    }

    // -- Test 16: BDVariable display -------------------------------------

    #[test]
    fn bd_variable_display() {
        let var = BDVariable::new(42, "T");
        assert_eq!(format!("{var}"), "?T");
    }

    // -- Test 17: BDTerm predicates --------------------------------------

    #[test]
    fn bdterm_predicates() {
        let var = BDVariable::new(0, "X");
        let term_var = BDTerm::Var(var);
        let bd = make_bd(byte_rep(4, 4), read_cap(), empty_reld());
        let term_concrete = BDTerm::Concrete(bd);

        assert!(term_var.is_var());
        assert!(!term_var.is_concrete());
        assert!(term_concrete.is_concrete());
        assert!(!term_concrete.is_var());
        assert!(term_var.as_var().is_some());
        assert!(term_concrete.as_concrete().is_some());
    }

    // -- Test 18: Solver with conflicting bindings -----------------------

    #[test]
    fn solver_conflicting_bindings() {
        let var = BDVariable::new(0, "T");
        let bd1 = make_bd(byte_rep(4, 4), read_cap(), empty_reld());
        let bd2 = make_bd(byte_rep(8, 8), read_write_cap(), empty_reld());

        let constraints = vec![
            BDConstraint::equality(BDTerm::Var(var.clone()), BDTerm::Concrete(bd1)),
            BDConstraint::equality(BDTerm::Var(var), BDTerm::Concrete(bd2)),
        ];

        let result = solve_constraints(constraints);
        assert!(result.is_err());
    }

    // -- Test 19: Substitute on concrete BD is identity ------------------

    #[test]
    fn substitute_concrete_is_identity() {
        let bd = make_bd(byte_rep(4, 4), read_cap(), empty_reld());
        let subst = HashMap::new();
        let result = substitute(&bd, &subst);
        assert_eq!(result, bd);
    }

    // -- Test 20: Multiple constraints solved together -------------------

    #[test]
    fn solve_multiple_constraints() {
        let var_a = BDVariable::new(0, "A");
        let var_b = BDVariable::new(1, "B");
        let var_c = BDVariable::new(2, "C");
        let bd = make_bd(byte_rep(8, 8), read_cap(), empty_reld());

        let constraints = vec![
            BDConstraint::equality(BDTerm::Var(var_a.clone()), BDTerm::Concrete(bd.clone())),
            BDConstraint::equality(BDTerm::Var(var_b.clone()), BDTerm::Concrete(bd.clone())),
            BDConstraint::equality(BDTerm::Var(var_c.clone()), BDTerm::Var(var_a.clone())),
        ];

        let result = solve_constraints(constraints).expect("should solve");
        assert_eq!(result.get(&var_a), Some(&bd));
        assert_eq!(result.get(&var_b), Some(&bd));
        assert_eq!(result.get(&var_c), Some(&bd));
    }

    // -- Test 21: Constraint display -------------------------------------

    #[test]
    fn constraint_display() {
        let var = BDVariable::new(0, "T");
        let bd = make_bd(byte_rep(4, 4), read_cap(), empty_reld());
        let c = BDConstraint::equality(BDTerm::Var(var), BDTerm::Concrete(bd));
        let s = format!("{c}");
        assert!(s.contains("="));
    }

    // -- Test 22: BDSolver default ---------------------------------------

    #[test]
    fn solver_default() {
        let solver = BDSolver::default();
        assert!(solver.substitution().is_empty());
    }

    // -- Test 23: Reflexivity — X = X succeeds --------------------------

    #[test]
    fn reflexivity_x_equals_x() {
        let var = BDVariable::new(0, "X");
        let constraint = BDConstraint::equality(BDTerm::Var(var.clone()), BDTerm::Var(var.clone()));
        let result = solve_constraints(vec![constraint]);
        // X = X is trivially true — the variable remains free (no
        // concrete binding), but the constraint is satisfied.
        assert!(result.is_ok());
    }

    // -- Test 24: Func RepD unification ----------------------------------

    #[test]
    fn func_repd_unification() {
        let f1 = RepD::Func(FuncRep {
            params: vec![byte_rep(4, 4), byte_rep(8, 8)],
            result: Box::new(byte_rep(4, 4)),
        });
        let f2 = RepD::Func(FuncRep {
            params: vec![byte_rep(4, 4), byte_rep(8, 8)],
            result: Box::new(byte_rep(4, 4)),
        });
        let result = unify_repd_with_occurs(&f1, &f2, &mut HashSet::new())
            .expect("identical func sigs should unify");
        assert_eq!(result.size(), 8); // function pointer size
    }

    // -- Test 25: Enum RepD tag mismatch ---------------------------------

    #[test]
    fn enum_repd_tag_mismatch() {
        let e1 = RepD::Enum(EnumRep {
            variants: vec![(0u64, byte_rep(4, 4)), (1u64, byte_rep(4, 4))],
        });
        let e2 = RepD::Enum(EnumRep {
            variants: vec![(0u64, byte_rep(4, 4)), (2u64, byte_rep(4, 4))],
        });
        let result = unify_repd_with_occurs(&e1, &e2, &mut HashSet::new());
        assert!(result.is_err());
    }

    // -- Test 26: Solver finalize collapses chains -----------------------

    #[test]
    fn solver_finalize_collapses_chains() {
        let mut solver = BDSolver::new();
        let var_x = BDVariable::new(0, "X");
        let var_y = BDVariable::new(1, "Y");
        let bd = make_bd(byte_rep(4, 4), read_cap(), empty_reld());

        solver
            .add_constraint(&BDConstraint::equality(
                BDTerm::Var(var_x.clone()),
                BDTerm::Var(var_y.clone()),
            ))
            .unwrap();
        solver
            .add_constraint(&BDConstraint::equality(
                BDTerm::Var(var_y.clone()),
                BDTerm::Concrete(bd.clone()),
            ))
            .unwrap();

        solver.finalize();
        let subst = solver.substitution();
        // After finalize, X should map directly to Concrete(bd).
        assert_eq!(subst.get(&var_x), Some(&BDTerm::Concrete(bd.clone())));
    }

    // -- Test 27: Compatibility constraint fails for incompatible BDs ----

    #[test]
    fn compatibility_constraint_fails() {
        let a = make_bd(byte_rep(4, 4), read_cap(), empty_reld());
        let b = make_bd(byte_rep(8, 8), read_cap(), empty_reld());
        let constraint = BDConstraint::compatibility(BDTerm::Concrete(a), BDTerm::Concrete(b));
        let result = solve_constraints(vec![constraint]);
        assert!(result.is_err());
    }

    // -- Test 28: UnificationError display -------------------------------

    #[test]
    fn unification_error_display() {
        let err = UnificationError::Failed("test error".to_string());
        assert_eq!(format!("{err}"), "unification failed: test error");

        let err = UnificationError::IncompatibleRepD {
            repd1: "byte".to_string(),
            repd2: "ptr".to_string(),
            reason: "different constructors".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("incompatible RepD"));

        let err = UnificationError::SubtypeViolation {
            sub: "a".to_string(),
            sup: "b".to_string(),
        };
        let s = format!("{err}");
        assert!(s.contains("subtype violation"));
    }

    // -- Test 29: BDConstraintKind display --------------------------------

    #[test]
    fn constraint_kind_display() {
        assert_eq!(format!("{}", BDConstraintKind::Equality), "=");
        assert_eq!(format!("{}", BDConstraintKind::Compatibility), "~");
        assert_eq!(format!("{}", BDConstraintKind::Subtyping), "<:");
    }

    // -- Test 30: Mixed constraint kinds ---------------------------------

    #[test]
    fn mixed_constraint_kinds() {
        let var_a = BDVariable::new(0, "A");
        let var_b = BDVariable::new(1, "B");
        let bd1 = make_bd(byte_rep(8, 8), read_cap(), empty_reld());
        let bd2 = make_bd(byte_rep(8, 8), read_write_cap(), empty_reld());

        // A = bd1, A ~ bd2 (compatibility), A <: bd2 (subtyping: read ⊆ read+write)
        let constraints = vec![
            BDConstraint::equality(BDTerm::Var(var_a.clone()), BDTerm::Concrete(bd1)),
            BDConstraint::compatibility(BDTerm::Var(var_b.clone()), BDTerm::Concrete(bd2.clone())),
        ];

        let result = solve_constraints(constraints).expect("should solve");
        assert!(result.get(&var_a).is_some());
        // var_b has no equality binding, so it's free.
        assert!(result.get(&var_b).is_none());
    }
}
