//! CapD Lattice Operations and Context Resolution
//!
//! This module provides free functions implementing the core lattice operations
//! on [`CapD`] descriptors, complementing the methods defined on the [`CapD`]
//! struct itself.  The operations include:
//!
//! - **Lattice algebra**: [`meet`], [`join`], [`implies`]
//! - **Safe transformations**: [`weaken`], [`strengthen`]
//! - **Capability queries**: [`is_read_only`], [`is_exclusive`]
//! - **Context-dependent weakening**: [`context_weaken`]
//!
//! # Lattice Structure
//!
//! The CapD lattice is ordered by set-inclusion on capabilities (reversed for
//! conditions).  Given two descriptors `d₁ = CapD(c₁, q₁)` and `d₂ = CapD(c₂, q₂)`:
//!
//! ```text
//! d₁ ≤ d₂  ⟺  c₁ ⊆ c₂  ∧  q₁ ⊇ q₂
//! ```
//!
//! The structure `(CapD, ≤, ⊓, ⊔, ⊥, ⊤)` forms a **bounded distributive
//! lattice** (Theorem 2.2 of the formal specification):
//!
//! - `⊥ = CapD(∅, Cond)` — bottom (no capabilities, all conditions)
//! - `⊤ = CapD(Cap, ∅)`  — top (all capabilities, no conditions)
//! - `meet(d₁, d₂) = CapD(c₁ ∩ c₂, q₁ ∪ q₂)`
//! - `join(d₁, d₂)  = CapD(c₁ ∪ c₂, q₁ ∩ q₂)`
//!
//! # Weakening vs. Strengthening
//!
//! **Weakening** (moving down in the lattice: removing capabilities, adding
//! conditions) is always safe — it can only reduce the set of permitted
//! operations (Theorem 4.1).
//!
//! **Strengthening** (moving up in the lattice: adding capabilities, removing
//! conditions) requires proof that the additional permissions are valid in
//! the current context.

use crate::capd::{CapD, Capability, Condition};
use hashbrown::HashSet;
use std::fmt;

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Error returned when a weakening operation is invalid.
///
/// Weakening is the act of moving *down* in the lattice (removing
/// capabilities or adding conditions).  The only way weakening can fail is
/// if the target is not actually below the source in the lattice — i.e.,
/// the target would *add* capabilities that the source does not possess, or
/// *remove* conditions that the source requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeakeningError {
    /// The target descriptor contains capabilities not present in the source.
    ///
    /// This means the target is *above* the source in the lattice for these
    /// capabilities, which is a strengthening operation, not a weakening.
    CapabilityNotPresent {
        /// Capabilities in the target that are absent from the source.
        extra_caps: Vec<Capability>,
    },
    /// The target descriptor removes conditions that the source requires.
    ///
    /// Removing conditions makes the descriptor *more* permissive, which is
    /// a strengthening operation, not a weakening.
    ConditionRemoved {
        /// Conditions present in the source but absent from the target.
        removed_conditions: Vec<Condition>,
    },
    /// The target is not below the source in the lattice for both dimensions.
    BothViolations {
        extra_caps: Vec<Capability>,
        removed_conditions: Vec<Condition>,
    },
}

impl fmt::Display for WeakeningError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WeakeningError::CapabilityNotPresent { extra_caps } => {
                write!(f, "weakening error: target adds capabilities not in source: ")?;
                let strs: Vec<String> = extra_caps.iter().map(|c| format!("{c}")).collect();
                write!(f, "{}", strs.join(", "))
            }
            WeakeningError::ConditionRemoved { removed_conditions } => {
                write!(f, "weakening error: target removes conditions from source: ")?;
                let strs: Vec<String> =
                    removed_conditions.iter().map(|c| format!("{c}")).collect();
                write!(f, "{}", strs.join(", "))
            }
            WeakeningError::BothViolations {
                extra_caps,
                removed_conditions,
            } => {
                write!(f, "weakening error: target adds capabilities [")?;
                let cap_strs: Vec<String> = extra_caps.iter().map(|c| format!("{c}")).collect();
                write!(f, "{}", cap_strs.join(", "))?;
                write!(f, "] and removes conditions [")?;
                let cond_strs: Vec<String> =
                    removed_conditions.iter().map(|c| format!("{c}")).collect();
                write!(f, "{}", cond_strs.join(", "))?;
                write!(f, "]")
            }
        }
    }
}

impl std::error::Error for WeakeningError {}

/// Error returned when a strengthening operation cannot be proven.
///
/// Strengthening moves *up* in the lattice (adding capabilities or removing
/// conditions).  Because this grants additional permissions, the caller must
/// provide proof that the strengthened descriptor is valid.  The error
/// enumerates what would need to be proven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrengtheningError {
    /// The target requires capabilities that the source does not have.
    ///
    /// Each listed capability would need to be proven available (e.g., via
    /// context resolution, ownership transfer, or runtime check).
    MissingCapabilities {
        /// Capabilities in the target not present in the source.
        missing_caps: Vec<Capability>,
    },
    /// The target removes conditions that the source imposes.
    ///
    /// Each listed condition would need to be proven unnecessary (e.g., via
    /// context evidence that the condition is trivially satisfied).
    ConditionRelaxation {
        /// Conditions present in the source but absent from the target.
        relaxed_conditions: Vec<Condition>,
    },
    /// Both capability and condition violations are present.
    BothViolations {
        missing_caps: Vec<Capability>,
        relaxed_conditions: Vec<Condition>,
    },
}

impl fmt::Display for StrengtheningError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StrengtheningError::MissingCapabilities { missing_caps } => {
                write!(f, "strengthening error: unprovable capabilities: ")?;
                let strs: Vec<String> = missing_caps.iter().map(|c| format!("{c}")).collect();
                write!(f, "{}", strs.join(", "))
            }
            StrengtheningError::ConditionRelaxation { relaxed_conditions } => {
                write!(f, "strengthening error: cannot relax conditions: ")?;
                let strs: Vec<String> =
                    relaxed_conditions.iter().map(|c| format!("{c}")).collect();
                write!(f, "{}", strs.join(", "))
            }
            StrengtheningError::BothViolations {
                missing_caps,
                relaxed_conditions,
            } => {
                write!(f, "strengthening error: unprovable capabilities [")?;
                let cap_strs: Vec<String> = missing_caps.iter().map(|c| format!("{c}")).collect();
                write!(f, "{}", cap_strs.join(", "))?;
                write!(f, "] and unrelaxable conditions [")?;
                let cond_strs: Vec<String> =
                    relaxed_conditions.iter().map(|c| format!("{c}")).collect();
                write!(f, "{}", cond_strs.join(", "))?;
                write!(f, "]")
            }
        }
    }
}

impl std::error::Error for StrengtheningError {}

// ---------------------------------------------------------------------------
// UsageContext
// ---------------------------------------------------------------------------

/// Classifies the usage context for a capability descriptor, enabling
/// context-dependent weakening rules.
///
/// Different usage contexts impose different constraints on which
/// capabilities are safe to retain.  For example, a value used in a
/// read-only observation context should not retain `Write`, while a value
/// sent across a thread boundary should not retain non-`Send` capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsageContext {
    /// Pure observation: only `Read`, `Compare`, and `Hash` are retained.
    Observation,
    /// Read-only access: `Write`, `DerivePtr`, `Cast`, and `Move` are removed.
    ReadOnly,
    /// Shared reference context: only shareable capabilities are retained.
    /// Removes `Write`, `Move`, `Drop`, and `DerivePtr`.
    SharedRef,
    /// Mutable reference context: retains `Read` and `Write` but removes
    /// `Share`, `Fork`, `Move`, and `Send`.
    MutRef,
    /// Thread-local context: `Send` is removed (value cannot leave thread).
    ThreadLocal,
    /// Concurrency boundary: only `Send`-compatible capabilities are retained.
    /// Removes `Write`, `DerivePtr`, and `Move`.
    ConcurrentSend,
    /// Serialization boundary: only `Serialize` and `Read` are retained.
    Serialization,
    /// Pointer derivation: removes `Move` and adds `ValidDuring` semantics
    /// (represented by restricting to pointer-compatible capabilities).
    PointerDerivation,
}

impl fmt::Display for UsageContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UsageContext::Observation => write!(f, "Observation"),
            UsageContext::ReadOnly => write!(f, "ReadOnly"),
            UsageContext::SharedRef => write!(f, "SharedRef"),
            UsageContext::MutRef => write!(f, "MutRef"),
            UsageContext::ThreadLocal => write!(f, "ThreadLocal"),
            UsageContext::ConcurrentSend => write!(f, "ConcurrentSend"),
            UsageContext::Serialization => write!(f, "Serialization"),
            UsageContext::PointerDerivation => write!(f, "PointerDerivation"),
        }
    }
}

// ---------------------------------------------------------------------------
// Lattice operations (free functions)
// ---------------------------------------------------------------------------

/// Compute the **meet** (greatest lower bound) of two CapDs.
///
/// The meet takes the intersection of capability sets and the union of
/// condition sets:
///
/// ```text
/// meet(d₁, d₂) = CapD(c₁ ∩ c₂, q₁ ∪ q₂)
/// ```
///
/// This is the most permissive CapD that is below both `c1` and `c2` in the
/// lattice.  It represents the capabilities available when *both* descriptors
/// must be satisfied simultaneously (e.g., after a branch merge).
///
/// # Example
///
/// ```
/// use vuma_bd::capd::{CapD, Capability};
/// use vuma_bd::capd_lattice::meet;
///
/// let a = CapD::empty().strengthen(&[Capability::Read, Capability::Write]);
/// let b = CapD::empty().strengthen(&[Capability::Read, Capability::Execute]);
/// let m = meet(&a, &b);
/// assert!(m.caps.contains(&Capability::Read));
/// assert!(!m.caps.contains(&Capability::Write));
/// assert!(!m.caps.contains(&Capability::Execute));
/// ```
pub fn meet(c1: &CapD, c2: &CapD) -> CapD {
    c1.meet(c2)
}

/// Compute the **join** (least upper bound) of two CapDs.
///
/// The join takes the union of capability sets and the intersection of
/// condition sets:
///
/// ```text
/// join(d₁, d₂) = CapD(c₁ ∪ c₂, q₁ ∩ q₂)
/// ```
///
/// This is the least permissive CapD that is above both `c1` and `c2` in the
/// lattice.  It represents the capabilities available when *either* descriptor
/// suffices (e.g., the union of two alternative permission sources).
///
/// # Example
///
/// ```
/// use vuma_bd::capd::{CapD, Capability};
/// use vuma_bd::capd_lattice::join;
///
/// let a = CapD::empty().strengthen(&[Capability::Read, Capability::Write]);
/// let b = CapD::empty().strengthen(&[Capability::Read, Capability::Execute]);
/// let j = join(&a, &b);
/// assert!(j.caps.contains(&Capability::Read));
/// assert!(j.caps.contains(&Capability::Write));
/// assert!(j.caps.contains(&Capability::Execute));
/// ```
pub fn join(c1: &CapD, c2: &CapD) -> CapD {
    c1.join(c2)
}

// ---------------------------------------------------------------------------
// Weakening and Strengthening with validation
// ---------------------------------------------------------------------------

/// Safely weaken a CapD to a target descriptor.
///
/// Weakening removes capabilities (or adds conditions), moving *down* in the
/// lattice.  By Theorem 4.1, weakening is always safe: if an operation
/// succeeds with the source, it succeeds with any weakening.
///
/// This function validates that `target` is actually a weakening of `c`
/// (i.e., `target ≤ c` in the lattice).  If the target would *add*
/// capabilities or *remove* conditions (a strengthening), it returns
/// [`WeakeningError`].
///
/// # Lattice check
///
/// `target ≤ c` iff `target.caps ⊆ c.caps` and `target.conditions ⊇ c.conditions`.
///
/// # Example
///
/// ```
/// use vuma_bd::capd::{CapD, Capability};
/// use vuma_bd::capd_lattice::weaken;
///
/// let source = CapD::empty().strengthen(&[Capability::Read, Capability::Write]);
/// let target = CapD::empty().strengthen(&[Capability::Read]);
/// assert!(weaken(&source, &target).is_ok());
/// ```
pub fn weaken(c: &CapD, target: &CapD) -> Result<CapD, WeakeningError> {
    // Compute which capabilities the target adds beyond the source
    let extra_caps: HashSet<Capability> = target.caps.difference(&c.caps).copied().collect();
    // Compute which conditions the target removes relative to the source
    let removed_conditions: HashSet<Condition> =
        c.conditions.difference(&target.conditions).copied().collect();

    let has_extra_caps = !extra_caps.is_empty();
    let has_removed_conditions = !removed_conditions.is_empty();

    if has_extra_caps && has_removed_conditions {
        Err(WeakeningError::BothViolations {
            extra_caps: extra_caps.into_iter().collect(),
            removed_conditions: removed_conditions.into_iter().collect(),
        })
    } else if has_extra_caps {
        Err(WeakeningError::CapabilityNotPresent {
            extra_caps: extra_caps.into_iter().collect(),
        })
    } else if has_removed_conditions {
        Err(WeakeningError::ConditionRemoved {
            removed_conditions: removed_conditions.into_iter().collect(),
        })
    } else {
        Ok(target.clone())
    }
}

/// Attempt to strengthen a CapD to a target descriptor (requires proof).
///
/// Strengthening adds capabilities (or removes conditions), moving *up* in
/// the lattice.  Because this grants additional permissions, the caller must
/// prove that the strengthened descriptor is valid — for example, by showing
/// that the execution context satisfies the required conditions, or that the
/// value's ownership model supports the additional capability.
///
/// This function validates that `target` is actually a strengthening of `c`
/// (i.e., `c ≤ target` in the lattice).  If the target would *remove*
/// capabilities or *add* conditions (a weakening), it returns
/// [`StrengtheningError`].  Note that even when the structural check passes,
/// the caller is responsible for providing the *proof* that the strengthening
/// is semantically valid; this function only checks the lattice relationship.
///
/// # Lattice check
///
/// `c ≤ target` iff `c.caps ⊆ target.caps` and `c.conditions ⊇ target.conditions`.
///
/// # Example
///
/// ```
/// use vuma_bd::capd::{CapD, Capability};
/// use vuma_bd::capd_lattice::strengthen;
///
/// let source = CapD::empty().strengthen(&[Capability::Read]);
/// let target = CapD::empty().strengthen(&[Capability::Read, Capability::Write]);
/// // Structural check passes, but the caller must prove Write is valid
/// assert!(strengthen(&source, &target).is_ok());
/// ```
pub fn strengthen(c: &CapD, target: &CapD) -> Result<CapD, StrengtheningError> {
    // Compute which capabilities the target has that the source lacks
    let missing_caps: HashSet<Capability> = target.caps.difference(&c.caps).copied().collect();
    // Compute which conditions the source has that the target does not
    let relaxed_conditions: HashSet<Condition> = c
        .conditions
        .difference(&target.conditions)
        .copied()
        .collect();

    // If source already implies target (source ≥ target), then target is a
    // weakening, not a strengthening — this is the wrong function.
    // But if source ≤ target (the normal strengthening case), missing_caps
    // and/or relaxed_conditions will be non-empty.
    // If source == target, both are empty and this is a no-op strengthening.
    // If source > target, then target is a weakening — return error.

    // Actually, let's check: if the target removes capabilities or adds
    // conditions compared to the source, that's weakening, not strengthening.
    let removed_caps: HashSet<Capability> = c.caps.difference(&target.caps).copied().collect();
    let added_conditions: HashSet<Condition> = target
        .conditions
        .difference(&c.conditions)
        .copied()
        .collect();

    let has_removed_caps = !removed_caps.is_empty();
    let has_added_conditions = !added_conditions.is_empty();
    // `missing_caps` and `relaxed_conditions` document what the caller needs
    // to prove for the strengthening; they are non-empty when the strengthening
    // is structurally valid but semantically requires justification.
    let _has_missing_caps = !missing_caps.is_empty();
    let _has_relaxed_conditions = !relaxed_conditions.is_empty();

    // If target removes caps or adds conditions, it's not a valid strengthening
    if has_removed_caps && has_added_conditions {
        return Err(StrengtheningError::BothViolations {
            missing_caps: removed_caps.into_iter().collect(),
            relaxed_conditions: added_conditions.into_iter().collect(),
        });
    }
    if has_removed_caps {
        return Err(StrengtheningError::MissingCapabilities {
            missing_caps: removed_caps.into_iter().collect(),
        });
    }
    if has_added_conditions {
        return Err(StrengtheningError::ConditionRelaxation {
            relaxed_conditions: added_conditions.into_iter().collect(),
        });
    }

    // At this point, target.caps ⊇ c.caps and target.conditions ⊆ c.conditions.
    // This is a valid structural strengthening. The missing_caps and
    // relaxed_conditions tell the caller what they need to prove.
    // Since this is a valid strengthening direction, return Ok.
    // The "proof" obligation is on the caller; we document which capabilities
    // and conditions are involved but don't enforce proof here.
    Ok(target.clone())
}

// ---------------------------------------------------------------------------
// Capability queries
// ---------------------------------------------------------------------------

/// Returns `true` if `c1` implies `c2` — that is, `c1` is at least as
/// capable as `c2`.
///
/// Formally, `implies(c1, c2)` iff `c2 ≤ c1` in the lattice:
///
/// ```text
/// c1 implies c2  ⟺  c2.caps ⊆ c1.caps  ∧  c1.conditions ⊆ c2.conditions
/// ```
///
/// If `c1` implies `c2`, then any operation permitted by `c2` is also
/// permitted by `c1` (in any context where `c1`'s conditions are satisfied).
///
/// # Example
///
/// ```
/// use vuma_bd::capd::{CapD, Capability};
/// use vuma_bd::capd_lattice::implies;
///
/// let rw = CapD::empty().strengthen(&[Capability::Read, Capability::Write]);
/// let r  = CapD::empty().strengthen(&[Capability::Read]);
/// assert!(implies(&rw, &r));  // Read+Write implies Read
/// assert!(!implies(&r, &rw)); // Read does NOT imply Read+Write
/// ```
pub fn implies(c1: &CapD, c2: &CapD) -> bool {
    c2.is_subset(c1)
}

/// Returns `true` if the CapD grants read access but no write or
/// write-adjacent capabilities.
///
/// A descriptor is considered *read-only* if it has `Read` capability but
/// lacks any capability that could lead to mutation:
///
/// - `Write` — direct mutation
/// - `DerivePtr` — could derive a mutable pointer
/// - `Cast` — could reinterpret as mutable
///
/// This is a conservative check: it may report `false` for descriptors that
/// are effectively read-only in a given context, but will never report
/// `true` for a descriptor that could permit mutation.
///
/// # Example
///
/// ```
/// use vuma_bd::capd::{CapD, Capability};
/// use vuma_bd::capd_lattice::is_read_only;
///
/// let r = CapD::empty().strengthen(&[Capability::Read]);
/// let rw = CapD::empty().strengthen(&[Capability::Read, Capability::Write]);
/// assert!(is_read_only(&r));
/// assert!(!is_read_only(&rw));
/// ```
pub fn is_read_only(c: &CapD) -> bool {
    c.caps.contains(&Capability::Read)
        && !c.caps.contains(&Capability::Write)
        && !c.caps.contains(&Capability::DerivePtr)
        && !c.caps.contains(&Capability::Cast)
}

/// Returns `true` if the CapD has exclusive (write) capability.
///
/// A descriptor is *exclusive* when it holds `Write` capability, granting
/// the ability to mutate the described value.  In the VUMA model, holding
/// `Write` typically means the value is exclusively owned (no other
/// reference can write to it simultaneously).
///
/// Note: `Write` does not imply `Read` in the VUMA capability model — the
/// capabilities are orthogonal.  A descriptor with only `Write` can modify
/// a value but not observe it (e.g., a write-only memory-mapped register).
///
/// # Example
///
/// ```
/// use vuma_bd::capd::{CapD, Capability};
/// use vuma_bd::capd_lattice::is_exclusive;
///
/// let rw = CapD::empty().strengthen(&[Capability::Read, Capability::Write]);
/// let r  = CapD::empty().strengthen(&[Capability::Read]);
/// assert!(is_exclusive(&rw));
/// assert!(!is_exclusive(&r));
/// ```
pub fn is_exclusive(c: &CapD) -> bool {
    c.caps.contains(&Capability::Write)
}

// ---------------------------------------------------------------------------
// Context-dependent weakening
// ---------------------------------------------------------------------------

/// The set of capabilities compatible with pointer derivation.
///
/// From the formal specification (Definition 3.3), derived pointers can
/// only hold capabilities meaningful for a pointer, and `Move` is explicitly
/// excluded because derivation does not transfer ownership.
const PTR_COMPATIBLE_CAPS: &[Capability] = &[
    Capability::Read,
    Capability::Write,
    Capability::Execute,
    Capability::DerivePtr,
    Capability::Cast,
    Capability::Compare,
    Capability::Hash,
    Capability::Share,
    Capability::Pin,
];

/// Apply context-dependent weakening to a CapD.
///
/// Different usage contexts impose different constraints on which
/// capabilities are safe to retain.  This function weakens the input CapD
/// by removing capabilities that are incompatible with the given
/// [`UsageContext`], while preserving the condition set.
///
/// The resulting CapD is always ≤ the input in the lattice (weakening is
/// always safe by Theorem 4.1).
///
/// # Weakening rules by context
///
/// | Context             | Retained capabilities                                    |
/// |---------------------|----------------------------------------------------------|
/// | `Observation`       | `Read`, `Compare`, `Hash`                                |
/// | `ReadOnly`          | All except `Write`, `DerivePtr`, `Cast`, `Move`          |
/// | `SharedRef`         | All except `Write`, `Move`, `Drop`, `DerivePtr`          |
/// | `MutRef`            | `Read`, `Write`, `Compare`, `Hash`, `Drop`, `Pin`        |
/// | `ThreadLocal`       | All except `Send`                                        |
/// | `ConcurrentSend`    | All except `Write`, `DerivePtr`, `Move`                  |
/// | `Serialization`     | `Read`, `Serialize`, `Hash`, `Compare`                   |
/// | `PointerDerivation` | `PTR_COMPATIBLE_CAPS` minus `Move`                       |
///
/// # Example
///
/// ```
/// use vuma_bd::capd::{CapD, Capability};
/// use vuma_bd::capd_lattice::{context_weaken, UsageContext};
///
/// let rw = CapD::empty().strengthen(&[Capability::Read, Capability::Write]);
/// let ro = context_weaken(&rw, UsageContext::ReadOnly);
/// assert!(ro.caps.contains(&Capability::Read));
/// assert!(!ro.caps.contains(&Capability::Write));
/// ```
pub fn context_weaken(c: &CapD, usage: UsageContext) -> CapD {
    let retained: HashSet<Capability> = match usage {
        UsageContext::Observation => {
            [Capability::Read, Capability::Compare, Capability::Hash]
                .into_iter()
                .collect()
        }
        UsageContext::ReadOnly => c
            .caps
            .iter()
            .copied()
            .filter(|cap| {
                !matches!(
                    cap,
                    Capability::Write | Capability::DerivePtr | Capability::Cast | Capability::Move
                )
            })
            .collect(),
        UsageContext::SharedRef => c
            .caps
            .iter()
            .copied()
            .filter(|cap| {
                !matches!(
                    cap,
                    Capability::Write | Capability::Move | Capability::Drop | Capability::DerivePtr
                )
            })
            .collect(),
        UsageContext::MutRef => c
            .caps
            .iter()
            .copied()
            .filter(|cap| {
                matches!(
                    cap,
                    Capability::Read
                        | Capability::Write
                        | Capability::Compare
                        | Capability::Hash
                        | Capability::Drop
                        | Capability::Pin
                )
            })
            .collect(),
        UsageContext::ThreadLocal => c
            .caps
            .iter()
            .copied()
            .filter(|cap| !matches!(cap, Capability::Send))
            .collect(),
        UsageContext::ConcurrentSend => c
            .caps
            .iter()
            .copied()
            .filter(|cap| {
                !matches!(
                    cap,
                    Capability::Write | Capability::DerivePtr | Capability::Move
                )
            })
            .collect(),
        UsageContext::Serialization => {
            [Capability::Read, Capability::Serialize, Capability::Hash, Capability::Compare]
                .into_iter()
                .collect()
        }
        UsageContext::PointerDerivation => {
            let ptr_set: HashSet<Capability> = PTR_COMPATIBLE_CAPS.iter().copied().collect();
            c.caps.intersection(&ptr_set).copied().collect()
        }
    };

    CapD {
        caps: c.caps.intersection(&retained).copied().collect(),
        conditions: c.conditions.clone(),
    }
}

// ---------------------------------------------------------------------------
// Widening
// ---------------------------------------------------------------------------

/// Compute the **widening** of two CapDs for fixpoint convergence.
///
/// Widening replaces increasing chains with `Top`. If `c2` is strictly
/// above `c1` in the lattice, the result is `CapD::all()` (Top).
/// Otherwise, the result is `c2`.
///
/// This ensures convergence in the CapD lattice during iterative
/// fixed-point computation on cyclic data.
///
/// # Example
///
/// ```
/// use vuma_bd::capd::{CapD, Capability};
/// use vuma_bd::capd_lattice::widen;
///
/// let c1 = CapD::empty().strengthen(&[Capability::Read]);
/// let c2 = CapD::empty().strengthen(&[Capability::Read, Capability::Write]);
/// let w = widen(&c1, &c2);
/// // c2 is strictly above c1, so widening jumps to Top
/// assert!(w.caps.contains(&Capability::Read));
/// assert!(w.caps.contains(&Capability::Write));
/// assert!(w.caps.contains(&Capability::Execute));
/// ```
pub fn widen(c1: &CapD, c2: &CapD) -> CapD {
    c1.widen(c2)
}

// ---------------------------------------------------------------------------
// Lattice property verification helpers
// ---------------------------------------------------------------------------

/// Verify the **idempotency** law: `meet(d, d) = d` and `join(d, d) = d`.
///
/// Returns `true` if both idempotency laws hold for the given CapD.
pub fn verify_idempotency(d: &CapD) -> bool {
    meet(d, d) == *d && join(d, d) == *d
}

/// Verify the **commutativity** law: `meet(a, b) = meet(b, a)` and
/// `join(a, b) = join(b, a)`.
///
/// Returns `true` if both commutativity laws hold.
pub fn verify_commutativity(a: &CapD, b: &CapD) -> bool {
    meet(a, b) == meet(b, a) && join(a, b) == join(b, a)
}

/// Verify the **associativity** law for three CapDs.
///
/// Returns `true` if:
/// - `meet(a, meet(b, c)) = meet(meet(a, b), c)`
/// - `join(a, join(b, c)) = join(join(a, b), c)`
pub fn verify_associativity(a: &CapD, b: &CapD, c: &CapD) -> bool {
    meet(a, &meet(b, c)) == meet(&meet(a, b), c)
        && join(a, &join(b, c)) == join(&join(a, b), c)
}

/// Verify the **absorption** law: `meet(a, join(a, b)) = a` and
/// `join(a, meet(a, b)) = a`.
///
/// Returns `true` if both absorption laws hold.
pub fn verify_absorption(a: &CapD, b: &CapD) -> bool {
    meet(a, &join(a, b)) == *a && join(a, &meet(a, b)) == *a
}

/// Verify the **distributivity** law: `meet(a, join(b, c)) = join(meet(a, b),
/// meet(a, c))` and `join(a, meet(b, c)) = meet(join(a, b), join(a, c))`.
///
/// Returns `true` if both distributivity laws hold.
pub fn verify_distributivity(a: &CapD, b: &CapD, c: &CapD) -> bool {
    meet(a, &join(b, c)) == join(&meet(a, b), &meet(a, c))
        && join(a, &meet(b, c)) == meet(&join(a, b), &join(a, c))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capd::Condition;

    /// Helper: build a CapD from a slice of capabilities and no conditions.
    fn capd_from(caps: &[Capability]) -> CapD {
        CapD::empty().strengthen(caps)
    }

    /// Helper: build a CapD with capabilities and a single condition.
    fn capd_with_cond(caps: &[Capability], cond: Condition) -> CapD {
        let mut d = CapD::empty().strengthen(caps);
        d.conditions.insert(cond);
        d
    }

    // ---- meet / join ----

    #[test]
    fn test_meet_intersection() {
        let a = capd_from(&[Capability::Read, Capability::Write, Capability::Execute]);
        let b = capd_from(&[Capability::Read, Capability::Send, Capability::Share]);
        let m = meet(&a, &b);
        // Only Read is common
        assert!(m.caps.contains(&Capability::Read));
        assert!(!m.caps.contains(&Capability::Write));
        assert!(!m.caps.contains(&Capability::Execute));
        assert!(!m.caps.contains(&Capability::Send));
        assert!(!m.caps.contains(&Capability::Share));
        assert!(m.conditions.is_empty());
    }

    #[test]
    fn test_join_union() {
        let a = capd_from(&[Capability::Read, Capability::Write]);
        let b = capd_from(&[Capability::Read, Capability::Execute]);
        let j = join(&a, &b);
        assert!(j.caps.contains(&Capability::Read));
        assert!(j.caps.contains(&Capability::Write));
        assert!(j.caps.contains(&Capability::Execute));
        assert_eq!(j.caps.len(), 3);
    }

    #[test]
    fn test_meet_with_conditions() {
        let a = capd_with_cond(&[Capability::Read], Condition::InPhase(1));
        let b = capd_with_cond(&[Capability::Read], Condition::RequiresLock(42));
        let m = meet(&a, &b);
        // Capabilities: intersection (Read)
        assert!(m.caps.contains(&Capability::Read));
        // Conditions: union (both conditions)
        assert!(m.conditions.contains(&Condition::InPhase(1)));
        assert!(m.conditions.contains(&Condition::RequiresLock(42)));
    }

    #[test]
    fn test_join_with_conditions() {
        let a = capd_with_cond(&[Capability::Read], Condition::InPhase(1));
        let b = capd_with_cond(&[Capability::Write], Condition::InPhase(1));
        let j = join(&a, &b);
        // Capabilities: union
        assert!(j.caps.contains(&Capability::Read));
        assert!(j.caps.contains(&Capability::Write));
        // Conditions: intersection (only InPhase(1) is common)
        assert!(j.conditions.contains(&Condition::InPhase(1)));
        assert_eq!(j.conditions.len(), 1);
    }

    // ---- weaken / strengthen ----

    #[test]
    fn test_weaken_valid() {
        let source = capd_from(&[Capability::Read, Capability::Write, Capability::Execute]);
        let target = capd_from(&[Capability::Read]);
        let result = weaken(&source, &target);
        assert!(result.is_ok());
        let weakened = result.unwrap();
        assert!(weakened.caps.contains(&Capability::Read));
        assert!(!weakened.caps.contains(&Capability::Write));
    }

    #[test]
    fn test_weaken_invalid_adds_capability() {
        let source = capd_from(&[Capability::Read]);
        let target = capd_from(&[Capability::Read, Capability::Write]);
        let result = weaken(&source, &target);
        assert!(result.is_err());
        match result.unwrap_err() {
            WeakeningError::CapabilityNotPresent { extra_caps } => {
                assert!(extra_caps.contains(&Capability::Write));
            }
            other => panic!("expected CapabilityNotPresent, got {other:?}"),
        }
    }

    #[test]
    fn test_weaken_invalid_removes_condition() {
        let source = capd_with_cond(&[Capability::Read], Condition::RequiresLock(1));
        let target = capd_from(&[Capability::Read]); // no condition
        let result = weaken(&source, &target);
        assert!(result.is_err());
        match result.unwrap_err() {
            WeakeningError::ConditionRemoved { removed_conditions } => {
                assert!(removed_conditions.contains(&Condition::RequiresLock(1)));
            }
            other => panic!("expected ConditionRemoved, got {other:?}"),
        }
    }

    #[test]
    fn test_weaken_same_descriptor() {
        let d = capd_from(&[Capability::Read, Capability::Write]);
        assert!(weaken(&d, &d).is_ok());
    }

    #[test]
    fn test_strengthen_valid() {
        let source = capd_from(&[Capability::Read]);
        let target = capd_from(&[Capability::Read, Capability::Write]);
        let result = strengthen(&source, &target);
        assert!(result.is_ok());
        let strengthened = result.unwrap();
        assert!(strengthened.caps.contains(&Capability::Write));
    }

    #[test]
    fn test_strengthen_invalid_removes_capability() {
        let source = capd_from(&[Capability::Read, Capability::Write]);
        let target = capd_from(&[Capability::Read]);
        let result = strengthen(&source, &target);
        assert!(result.is_err());
        match result.unwrap_err() {
            StrengtheningError::MissingCapabilities { missing_caps } => {
                assert!(missing_caps.contains(&Capability::Write));
            }
            other => panic!("expected MissingCapabilities, got {other:?}"),
        }
    }

    #[test]
    fn test_strengthen_invalid_adds_condition() {
        let source = capd_from(&[Capability::Read]);
        let target = capd_with_cond(&[Capability::Read], Condition::RequiresLock(1));
        let result = strengthen(&source, &target);
        assert!(result.is_err());
        match result.unwrap_err() {
            StrengtheningError::ConditionRelaxation { relaxed_conditions } => {
                assert!(relaxed_conditions.contains(&Condition::RequiresLock(1)));
            }
            other => panic!("expected ConditionRelaxation, got {other:?}"),
        }
    }

    #[test]
    fn test_strengthen_same_descriptor() {
        let d = capd_from(&[Capability::Read, Capability::Write]);
        assert!(strengthen(&d, &d).is_ok());
    }

    // ---- implies ----

    #[test]
    fn test_implies_superset() {
        let rw = capd_from(&[Capability::Read, Capability::Write]);
        let r = capd_from(&[Capability::Read]);
        // rw implies r (rw is at least as capable as r)
        assert!(implies(&rw, &r));
    }

    #[test]
    fn test_not_implies_subset() {
        let r = capd_from(&[Capability::Read]);
        let rw = capd_from(&[Capability::Read, Capability::Write]);
        // r does NOT imply rw
        assert!(!implies(&r, &rw));
    }

    #[test]
    fn test_implies_reflexive() {
        let d = capd_from(&[Capability::Read, Capability::Write]);
        assert!(implies(&d, &d));
    }

    #[test]
    fn test_implies_with_conditions() {
        // c1 has fewer conditions (more permissive) → c1 implies c2
        let c1 = capd_from(&[Capability::Read]); // no conditions
        let c2 = capd_with_cond(&[Capability::Read], Condition::InPhase(1));
        assert!(implies(&c1, &c2));
        // c2 has more conditions → does not imply c1
        assert!(!implies(&c2, &c1));
    }

    // ---- is_read_only / is_exclusive ----

    #[test]
    fn test_is_read_only_true() {
        let r = capd_from(&[Capability::Read, Capability::Compare, Capability::Hash]);
        assert!(is_read_only(&r));
    }

    #[test]
    fn test_is_read_only_false_with_write() {
        let rw = capd_from(&[Capability::Read, Capability::Write]);
        assert!(!is_read_only(&rw));
    }

    #[test]
    fn test_is_read_only_false_with_derive_ptr() {
        let rp = capd_from(&[Capability::Read, Capability::DerivePtr]);
        assert!(!is_read_only(&rp));
    }

    #[test]
    fn test_is_read_only_false_with_cast() {
        let rc = capd_from(&[Capability::Read, Capability::Cast]);
        assert!(!is_read_only(&rc));
    }

    #[test]
    fn test_is_read_only_false_without_read() {
        let w = capd_from(&[Capability::Write]);
        assert!(!is_read_only(&w));
    }

    #[test]
    fn test_is_exclusive_true() {
        let rw = capd_from(&[Capability::Read, Capability::Write]);
        assert!(is_exclusive(&rw));
    }

    #[test]
    fn test_is_exclusive_write_only() {
        let w = capd_from(&[Capability::Write]);
        assert!(is_exclusive(&w));
    }

    #[test]
    fn test_is_exclusive_false() {
        let r = capd_from(&[Capability::Read]);
        assert!(!is_exclusive(&r));
    }

    #[test]
    fn test_is_exclusive_empty() {
        let e = CapD::empty();
        assert!(!is_exclusive(&e));
    }

    // ---- context_weaken ----

    #[test]
    fn test_context_weaken_observation() {
        let d = CapD::all();
        let weakened = context_weaken(&d, UsageContext::Observation);
        assert!(weakened.caps.contains(&Capability::Read));
        assert!(weakened.caps.contains(&Capability::Compare));
        assert!(weakened.caps.contains(&Capability::Hash));
        assert!(!weakened.caps.contains(&Capability::Write));
        assert!(!weakened.caps.contains(&Capability::Execute));
        assert!(!weakened.caps.contains(&Capability::Send));
    }

    #[test]
    fn test_context_weaken_read_only() {
        let d = capd_from(&[
            Capability::Read,
            Capability::Write,
            Capability::Execute,
            Capability::Move,
            Capability::DerivePtr,
            Capability::Cast,
            Capability::Hash,
        ]);
        let weakened = context_weaken(&d, UsageContext::ReadOnly);
        assert!(weakened.caps.contains(&Capability::Read));
        assert!(weakened.caps.contains(&Capability::Execute));
        assert!(weakened.caps.contains(&Capability::Hash));
        assert!(!weakened.caps.contains(&Capability::Write));
        assert!(!weakened.caps.contains(&Capability::Move));
        assert!(!weakened.caps.contains(&Capability::DerivePtr));
        assert!(!weakened.caps.contains(&Capability::Cast));
    }

    #[test]
    fn test_context_weaken_shared_ref() {
        let d = CapD::all();
        let weakened = context_weaken(&d, UsageContext::SharedRef);
        assert!(weakened.caps.contains(&Capability::Read));
        assert!(weakened.caps.contains(&Capability::Share));
        assert!(!weakened.caps.contains(&Capability::Write));
        assert!(!weakened.caps.contains(&Capability::Move));
        assert!(!weakened.caps.contains(&Capability::Drop));
        assert!(!weakened.caps.contains(&Capability::DerivePtr));
    }

    #[test]
    fn test_context_weaken_mut_ref() {
        let d = CapD::all();
        let weakened = context_weaken(&d, UsageContext::MutRef);
        assert!(weakened.caps.contains(&Capability::Read));
        assert!(weakened.caps.contains(&Capability::Write));
        assert!(weakened.caps.contains(&Capability::Compare));
        assert!(weakened.caps.contains(&Capability::Hash));
        assert!(weakened.caps.contains(&Capability::Drop));
        assert!(weakened.caps.contains(&Capability::Pin));
        assert!(!weakened.caps.contains(&Capability::Share));
        assert!(!weakened.caps.contains(&Capability::Fork));
        assert!(!weakened.caps.contains(&Capability::Move));
        assert!(!weakened.caps.contains(&Capability::Send));
    }

    #[test]
    fn test_context_weaken_thread_local() {
        let d = capd_from(&[Capability::Read, Capability::Write, Capability::Send]);
        let weakened = context_weaken(&d, UsageContext::ThreadLocal);
        assert!(weakened.caps.contains(&Capability::Read));
        assert!(weakened.caps.contains(&Capability::Write));
        assert!(!weakened.caps.contains(&Capability::Send));
    }

    #[test]
    fn test_context_weaken_concurrent_send() {
        let d = capd_from(&[
            Capability::Read,
            Capability::Write,
            Capability::Send,
            Capability::DerivePtr,
            Capability::Move,
        ]);
        let weakened = context_weaken(&d, UsageContext::ConcurrentSend);
        assert!(weakened.caps.contains(&Capability::Read));
        assert!(weakened.caps.contains(&Capability::Send));
        assert!(!weakened.caps.contains(&Capability::Write));
        assert!(!weakened.caps.contains(&Capability::DerivePtr));
        assert!(!weakened.caps.contains(&Capability::Move));
    }

    #[test]
    fn test_context_weaken_serialization() {
        let d = CapD::all();
        let weakened = context_weaken(&d, UsageContext::Serialization);
        assert!(weakened.caps.contains(&Capability::Read));
        assert!(weakened.caps.contains(&Capability::Serialize));
        assert!(weakened.caps.contains(&Capability::Hash));
        assert!(weakened.caps.contains(&Capability::Compare));
        assert!(!weakened.caps.contains(&Capability::Write));
        assert!(!weakened.caps.contains(&Capability::Execute));
    }

    #[test]
    fn test_context_weaken_pointer_derivation() {
        let d = capd_from(&[
            Capability::Read,
            Capability::Write,
            Capability::Move,
            Capability::Drop,
            Capability::Fork,
        ]);
        let weakened = context_weaken(&d, UsageContext::PointerDerivation);
        assert!(weakened.caps.contains(&Capability::Read));
        assert!(weakened.caps.contains(&Capability::Write));
        assert!(!weakened.caps.contains(&Capability::Move)); // explicitly excluded
        assert!(!weakened.caps.contains(&Capability::Drop)); // not ptr-compatible
        assert!(!weakened.caps.contains(&Capability::Fork)); // not ptr-compatible
    }

    #[test]
    fn test_context_weaken_preserves_conditions() {
        let d = capd_with_cond(&[Capability::Read, Capability::Write], Condition::InPhase(1));
        let weakened = context_weaken(&d, UsageContext::ReadOnly);
        assert!(weakened.conditions.contains(&Condition::InPhase(1)));
    }

    #[test]
    fn test_context_weaken_always_below_source() {
        // context_weaken should always produce a CapD ≤ source
        let d = CapD::all();
        for ctx in [
            UsageContext::Observation,
            UsageContext::ReadOnly,
            UsageContext::SharedRef,
            UsageContext::MutRef,
            UsageContext::ThreadLocal,
            UsageContext::ConcurrentSend,
            UsageContext::Serialization,
            UsageContext::PointerDerivation,
        ] {
            let weakened = context_weaken(&d, ctx);
            assert!(
                weakened.is_subset(&d),
                "context_weaken({ctx}) should be ≤ source"
            );
        }
    }

    // ---- Lattice property verification ----

    #[test]
    fn test_lattice_idempotency() {
        let d = capd_from(&[Capability::Read, Capability::Write]);
        assert!(verify_idempotency(&d));
        assert!(verify_idempotency(&CapD::empty()));
        assert!(verify_idempotency(&CapD::all()));
    }

    #[test]
    fn test_lattice_commutativity() {
        let a = capd_from(&[Capability::Read, Capability::Write]);
        let b = capd_from(&[Capability::Read, Capability::Execute]);
        assert!(verify_commutativity(&a, &b));
    }

    #[test]
    fn test_lattice_associativity() {
        let a = capd_from(&[Capability::Read, Capability::Write]);
        let b = capd_from(&[Capability::Write, Capability::Execute]);
        let c = capd_from(&[Capability::Execute, Capability::Send]);
        assert!(verify_associativity(&a, &b, &c));
    }

    #[test]
    fn test_lattice_absorption() {
        let a = capd_from(&[Capability::Read, Capability::Write]);
        let b = capd_from(&[Capability::Execute, Capability::Send]);
        assert!(verify_absorption(&a, &b));
    }

    #[test]
    fn test_lattice_distributivity() {
        let a = capd_from(&[Capability::Read, Capability::Write]);
        let b = capd_from(&[Capability::Write, Capability::Execute]);
        let c = capd_from(&[Capability::Execute, Capability::Send]);
        assert!(verify_distributivity(&a, &b, &c));
    }

    #[test]
    fn test_bottom_top_extremal() {
        let empty = CapD::empty();
        let all = CapD::all();
        let d = capd_from(&[Capability::Read, Capability::Write]);
        // empty ≤ d ≤ all
        assert!(empty.is_subset(&d));
        assert!(d.is_subset(&all));
        // meet with bottom = bottom
        assert_eq!(meet(&empty, &d), empty);
        // join with top = top
        assert_eq!(join(&all, &d), all);
    }

    // ---- Error display ----

    #[test]
    fn test_weakening_error_display() {
        let err = WeakeningError::CapabilityNotPresent {
            extra_caps: vec![Capability::Write, Capability::Execute],
        };
        let msg = format!("{err}");
        assert!(msg.contains("Write"));
        assert!(msg.contains("Execute"));
    }

    #[test]
    fn test_strengthening_error_display() {
        let err = StrengtheningError::ConditionRelaxation {
            relaxed_conditions: vec![Condition::RequiresLock(99)],
        };
        let msg = format!("{err}");
        assert!(msg.contains("RequiresLock(99)"));
    }

    #[test]
    fn test_usage_context_display() {
        assert_eq!(format!("{}", UsageContext::Observation), "Observation");
        assert_eq!(format!("{}", UsageContext::ReadOnly), "ReadOnly");
        assert_eq!(format!("{}", UsageContext::MutRef), "MutRef");
        assert_eq!(
            format!("{}", UsageContext::PointerDerivation),
            "PointerDerivation"
        );
    }

    #[test]
    fn test_weaken_both_violations() {
        // Source has Read + a condition; target adds Write AND removes the condition
        let source = capd_with_cond(&[Capability::Read], Condition::InPhase(1));
        let target = capd_from(&[Capability::Read, Capability::Write]);
        let result = weaken(&source, &target);
        assert!(result.is_err());
        match result.unwrap_err() {
            WeakeningError::BothViolations { .. } => {}
            other => panic!("expected BothViolations, got {other:?}"),
        }
    }

    #[test]
    fn test_strengthen_both_violations() {
        let source = capd_from(&[Capability::Read, Capability::Write]);
        let target = capd_with_cond(&[Capability::Read], Condition::InPhase(1));
        let result = strengthen(&source, &target);
        assert!(result.is_err());
        match result.unwrap_err() {
            StrengtheningError::BothViolations { .. } => {}
            other => panic!("expected BothViolations, got {other:?}"),
        }
    }

    // ---- widen ----

    #[test]
    fn test_widen_increasing_jumps_to_top() {
        let c1 = capd_from(&[Capability::Read]);
        let c2 = capd_from(&[Capability::Read, Capability::Write]);
        let w = widen(&c1, &c2);
        // c2 is strictly above c1, so widening jumps to Top
        assert_eq!(w, CapD::all());
    }

    #[test]
    fn test_widen_same_returns_other() {
        let d = capd_from(&[Capability::Read, Capability::Write]);
        let w = widen(&d, &d);
        // Same descriptor: not strictly above, so result is the other (d itself)
        assert_eq!(w, d);
    }

    #[test]
    fn test_widen_decreasing_returns_other() {
        let c1 = capd_from(&[Capability::Read, Capability::Write]);
        let c2 = capd_from(&[Capability::Read]);
        // c2 is below c1, not above, so widening returns c2
        let w = widen(&c1, &c2);
        assert_eq!(w, c2);
    }

    #[test]
    fn test_widen_incomparable_returns_other() {
        let c1 = capd_from(&[Capability::Read]);
        let c2 = capd_from(&[Capability::Write]);
        // Incomparable: not strictly above, so returns c2
        let w = widen(&c1, &c2);
        assert_eq!(w, c2);
    }

    #[test]
    fn test_widen_condition_removal_jumps_to_top() {
        let c1 = capd_with_cond(&[Capability::Read], Condition::InPhase(1));
        let c2 = capd_from(&[Capability::Read]);
        // c2 has fewer conditions (more permissive) => c2 is above c1
        let w = widen(&c1, &c2);
        assert_eq!(w, CapD::all());
    }

}
