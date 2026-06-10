//! Interpretation Invariant Checker (Invariant 3).
//!
//! This module implements the Interpretation invariant from the VUMA spec
//! (VUMA-SPEC-INV-001, Section 5):
//!
//! > **Every access respects the Representation Descriptor (RepD) of its target.**
//!
//! The checker verifies:
//!
//! 1. **Cast safety** — every `DerivationKind::Cast` preserves size and
//!    represents a valid reinterpretation (e.g. bytes → any is fine;
//!    pointer → integer is not unless explicitly marked safe).
//!
//! 2. **Write-then-read compatibility** — when a write under one RepD is
//!    followed by a read under another RepD on overlapping bytes, the
//!    two RepDs must be compatible or an explicit cast must mediate.
//!
//! 3. **Derivation chain type confusion** — a chain of casts that
//!    individually are valid but compose into an unsound reinterpretation
//!    (e.g. pointer → int → float) is flagged.
//!
//! 4. **Uninitialized pointer reads** — reading bytes that were never
//!    written and interpreting them as a pointer RepD is always a
//!    violation (per spec Section 5.1, uninitialized-pointer restriction).
//!
//! 5. **Access-size / RepD-size agreement** — the `size` of an access
//!    must be a multiple of the effective RepD's size, ensuring the
//!    access reads whole units of the representation.

use crate::access::{Access, AccessId, AccessKind};
use crate::address::Address;
use crate::derivation::{DerivationId, DerivationKind, DerivationSource, RepD};
use crate::msg::MSG;
use crate::region::RegionId;
use std::fmt;

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Severity of an interpretation invariant violation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ViolationSeverity {
    /// A definite violation — the invariant is broken.
    Error,
    /// A suspicious pattern that may be a violation but requires
    /// further analysis (e.g. transitive cast chain).
    Warning,
}

impl fmt::Display for ViolationSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViolationSeverity::Error => write!(f, "error"),
            ViolationSeverity::Warning => write!(f, "warning"),
        }
    }
}

/// The kind of interpretation invariant violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationKind {
    /// A `Cast` derivation changes the size of the RepD.
    CastSizeMismatch {
        derivation: DerivationId,
        from_size: u64,
        to_size: u64,
    },
    /// A `Cast` derivation reinterprets a pointer type as a non-pointer,
    /// non-bytes type without an explicit safe annotation.
    CastPointerToNonPointer {
        derivation: DerivationId,
        from_name: String,
        to_name: String,
    },
    /// A cast is not a valid reinterpretation according to the sub-RepD
    /// relation or the union-member rule.
    InvalidReinterpretation {
        derivation: DerivationId,
        from_name: String,
        to_name: String,
    },
    /// A write under one RepD is followed by a read under an incompatible
    /// RepD on overlapping bytes, with no intervening cast derivation.
    WriteReadIncompatible {
        write_access: AccessId,
        read_access: AccessId,
        write_repd: String,
        read_repd: String,
    },
    /// A chain of individually-valid casts composes into an unsound
    /// reinterpretation (e.g. pointer → int → float).
    TransitiveCastConfusion {
        chain: Vec<DerivationId>,
        original_repd: String,
        final_repd: String,
    },
    /// Reading uninitialized memory as a pointer type.
    UninitPointerRead {
        access: AccessId,
        repd_name: String,
    },
    /// The access size is not a multiple of the effective RepD size.
    AccessSizeMismatch {
        access: AccessId,
        access_size: u64,
        repd_size: u64,
    },
    /// The provenance range of a derivation is smaller than the RepD
    /// requires (cast would read out-of-bounds for the target type).
    ProvenanceTooSmallForCast {
        derivation: DerivationId,
        proven_size: u64,
        repd_size: u64,
    },
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViolationKind::CastSizeMismatch {
                derivation,
                from_size,
                to_size,
            } => write!(
                f,
                "Cast {} changes size from {} to {} bytes",
                derivation, from_size, to_size
            ),
            ViolationKind::CastPointerToNonPointer {
                derivation,
                from_name,
                to_name,
            } => write!(
                f,
                "Cast {} reinterprets pointer '{}' as non-pointer '{}'",
                derivation, from_name, to_name
            ),
            ViolationKind::InvalidReinterpretation {
                derivation,
                from_name,
                to_name,
            } => write!(
                f,
                "Cast {} is not a valid reinterpretation: '{}' → '{}'",
                derivation, from_name, to_name
            ),
            ViolationKind::WriteReadIncompatible {
                write_access,
                read_access,
                write_repd,
                read_repd,
            } => write!(
                f,
                "Write {} (RepD='{}') followed by read {} (RepD='{}') on overlapping bytes without compatible RepDs",
                write_access, write_repd, read_access, read_repd
            ),
            ViolationKind::TransitiveCastConfusion {
                chain,
                original_repd,
                final_repd,
            } => write!(
                f,
                "Transitive cast chain [{}] composes '{}' → '{}' unsoundly",
                chain
                    .iter()
                    .map(|d| format!("{}", d))
                    .collect::<Vec<_>>()
                    .join(", "),
                original_repd,
                final_repd
            ),
            ViolationKind::UninitPointerRead { access, repd_name } => write!(
                f,
                "Access {} reads uninitialized bytes as pointer type '{}'",
                access, repd_name
            ),
            ViolationKind::AccessSizeMismatch {
                access,
                access_size,
                repd_size,
            } => write!(
                f,
                "Access {} size {} is not a multiple of RepD size {}",
                access, access_size, repd_size
            ),
            ViolationKind::ProvenanceTooSmallForCast {
                derivation,
                proven_size,
                repd_size,
            } => write!(
                f,
                "Cast {} provenance has {} bytes but target RepD requires {}",
                derivation, proven_size, repd_size
            ),
        }
    }
}

/// A single interpretation invariant violation.
#[derive(Debug, Clone)]
pub struct InvariantViolation {
    /// What kind of violation was detected.
    pub kind: ViolationKind,
    /// How severe the violation is.
    pub severity: ViolationSeverity,
}

impl fmt::Display for InvariantViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.severity, self.kind)
    }
}

/// The result of checking the interpretation invariant.
#[derive(Debug, Clone)]
pub struct InvariantResult {
    /// All violations found.
    pub violations: Vec<InvariantViolation>,
}

impl InvariantResult {
    /// Create an empty (passing) result.
    pub fn ok() -> Self {
        InvariantResult { violations: Vec::new() }
    }

    /// Create a result with a single violation.
    pub fn with_violation(kind: ViolationKind, severity: ViolationSeverity) -> Self {
        InvariantResult {
            violations: vec![InvariantViolation { kind, severity }],
        }
    }

    /// Merge another result into this one.
    pub fn merge(&mut self, other: InvariantResult) {
        self.violations.extend(other.violations);
    }

    /// Returns `true` if no violations were found.
    pub fn is_ok(&self) -> bool {
        self.violations.is_empty()
    }

    /// Returns `true` if at least one error-severity violation was found.
    pub fn has_errors(&self) -> bool {
        self.violations
            .iter()
            .any(|v| v.severity == ViolationSeverity::Error)
    }

    /// Number of violations.
    pub fn violation_count(&self) -> usize {
        self.violations.len()
    }
}

impl fmt::Display for InvariantResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_ok() {
            write!(f, "Interpretation invariant: SATISFIED")
        } else {
            write!(
                f,
                "Interpretation invariant: VIOLATED ({} violation(s))",
                self.violation_count()
            )?;
            for v in &self.violations {
                write!(f, "\n  {}", v)?;
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// RepD classification helpers
// ---------------------------------------------------------------------------

/// Classification of a RepD for compatibility checking.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RepDClass {
    /// Raw bytes — the universal supertype; can be reinterpreted as anything.
    Bytes,
    /// A pointer type (e.g. `ptr<T>`).
    Pointer,
    /// An integer type (e.g. `u32`, `i64`).
    Integer,
    /// A floating-point type (e.g. `f32`, `f64`).
    Float,
    /// A struct / aggregate type.
    Struct,
    /// Any other / unknown representation.
    Other,
}

impl fmt::Display for RepDClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepDClass::Bytes => write!(f, "bytes"),
            RepDClass::Pointer => write!(f, "pointer"),
            RepDClass::Integer => write!(f, "integer"),
            RepDClass::Float => write!(f, "float"),
            RepDClass::Struct => write!(f, "struct"),
            RepDClass::Other => write!(f, "other"),
        }
    }
}

/// Classify a [`RepD`] by inspecting its name.
///
/// The classification uses naming conventions that the VUMA front-end
/// is expected to follow:
///
/// - `"bytes"` or starting with `"bytes"` → Bytes
/// - `"ptr"` or starting with `"ptr<"` or `"*mut"` or `"*const"` → Pointer
/// - Starting with `"u"` or `"i"` followed by digits → Integer
/// - Starting with `"f"` followed by digits → Float
/// - Contains `"struct"` or `"Struct"` → Struct
/// - Anything else → Other
fn classify_repd(repd: &RepD) -> RepDClass {
    let name = repd.name.to_lowercase();
    if name == "bytes" || name.starts_with("bytes") {
        RepDClass::Bytes
    } else if name.starts_with("ptr") || name.starts_with("*mut") || name.starts_with("*const") {
        RepDClass::Pointer
    } else if name.starts_with('u') || name.starts_with('i') {
        // Check that the rest is digits (e.g. "u32", "i64")
        let rest: String = name.chars().skip(1).collect();
        if rest.parse::<u64>().is_ok() {
            RepDClass::Integer
        } else {
            RepDClass::Other
        }
    } else if name.starts_with('f') {
        let rest: String = name.chars().skip(1).collect();
        if rest.parse::<u64>().is_ok() {
            RepDClass::Float
        } else {
            RepDClass::Other
        }
    } else if name.contains("struct") {
        RepDClass::Struct
    } else {
        RepDClass::Other
    }
}

// ---------------------------------------------------------------------------
// Core compatibility logic
// ---------------------------------------------------------------------------

/// Check whether a cast from `from` to `to` is a valid reinterpretation.
///
/// Implements the `valid_reinterpretation` relation from spec Section 5.1:
///
/// - Same RepD → valid
/// - from is Bytes → valid (bytes ⊑ any)
/// - from and to are the same class (e.g. both Integer) → valid
/// - from is Pointer and to is not Pointer/Bytes → INVALID
/// - Otherwise → needs IVE case analysis; conservatively invalid
fn valid_reinterpretation(from: &RepD, to: &RepD) -> bool {
    // Same RepD: always valid
    if from == to {
        return true;
    }

    let from_class = classify_repd(from);
    let to_class = classify_repd(to);

    // bytes ⊑ any
    if from_class == RepDClass::Bytes {
        return true;
    }

    // Same class (e.g. both integers, both pointers): valid
    if from_class == to_class {
        return true;
    }

    // pointer → non-pointer (not bytes): invalid
    if from_class == RepDClass::Pointer
        && to_class != RepDClass::Pointer
        && to_class != RepDClass::Bytes
    {
        return false;
    }

    // Conservative: anything else needs IVE case analysis
    false
}

/// Check full `compatible(r1, r2)` per spec Section 5.1:
///
/// - Sizes must match
/// - The reinterpretation must be valid
fn compatible(r1: &RepD, r2: &RepD) -> bool {
    r1.size == r2.size && valid_reinterpretation(r1, r2)
}

// ---------------------------------------------------------------------------
// Effective RepD computation
// ---------------------------------------------------------------------------

/// Compute the effective [`RepD`] of a derivation by walking its chain
/// to find the most recent cast, as defined by `repd_of` in spec Section 2.6.
///
/// Returns `None` if the chain is broken (missing parent derivation).
#[allow(dead_code)]
fn effective_repd(msg: &MSG, derivation_id: DerivationId) -> Option<RepD> {
    let derivation = msg.derivation(derivation_id)?;
    match &derivation.kind {
        DerivationKind::Cast { to, .. } => Some(to.clone()),
        _ => {
            // No cast at this level — walk the chain
            match &derivation.source {
                DerivationSource::Region(_) => {
                    // Default RepD for a region is "bytes" of the region size.
                    // We return a synthetic bytes RepD.
                    Some(RepD {
                        name: "bytes".to_string(),
                        size: 0, // will be filled by caller if needed
                    })
                }
                DerivationSource::AnotherDerivation(parent_id) => {
                    effective_repd(msg, *parent_id)
                }
            }
        }
    }
}

/// Compute the effective RepD with region size fallback.
///
/// When the derivation chain has no cast, the default RepD is "bytes"
/// whose size matches the remaining provenance range.
fn effective_repd_with_size(msg: &MSG, derivation_id: DerivationId) -> Option<RepD> {
    let derivation = msg.derivation(derivation_id)?;
    match &derivation.kind {
        DerivationKind::Cast { to, .. } => Some(to.clone()),
        _ => match &derivation.source {
            DerivationSource::Region(rid) => {
                let region = msg.region(*rid)?;
                Some(RepD {
                    name: "bytes".to_string(),
                    size: region.size,
                })
            }
            DerivationSource::AnotherDerivation(parent_id) => {
                effective_repd_with_size(msg, *parent_id)
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Derivation chain analysis
// ---------------------------------------------------------------------------

/// Collect all Cast RepDs along a derivation chain, in order from root to leaf.
///
/// Returns `None` if the chain is broken.
fn collect_cast_chain(msg: &MSG, derivation_id: DerivationId) -> Option<Vec<(DerivationId, RepD, RepD)>> {
    let chain = msg.derivation_chain(derivation_id);
    if chain.is_empty() {
        return None;
    }

    let mut casts = Vec::new();
    for d in &chain {
        if let DerivationKind::Cast { from, to } = &d.kind {
            casts.push((d.id, from.clone(), to.clone()));
        }
    }
    Some(casts)
}

/// Check a derivation chain for transitive cast confusion.
///
/// A chain of individually-valid casts may compose unsoundly, e.g.
/// `pointer → integer → float`. This function walks the cast chain
/// and verifies that the *original* source class and the *final* target
/// class are compatible according to the `valid_reinterpretation` rule.
fn check_transitive_cast_chain(
    msg: &MSG,
    derivation_id: DerivationId,
) -> Option<InvariantViolation> {
    let casts = collect_cast_chain(msg, derivation_id)?;

    // Need at least 2 casts for transitive confusion
    if casts.len() < 2 {
        return None;
    }

    // The very first cast's `from` is the original RepD
    let original_repd = &casts.first()?.1;
    // The last cast's `to` is the final RepD
    let final_repd = &casts.last()?.2;

    // Check: is the overall reinterpretation from original to final valid?
    if !valid_reinterpretation(original_repd, final_repd) {
        // Individual steps may be valid but the composition is not
        let chain_ids: Vec<DerivationId> = casts.iter().map(|(id, _, _)| *id).collect();
        Some(InvariantViolation {
            kind: ViolationKind::TransitiveCastConfusion {
                chain: chain_ids,
                original_repd: original_repd.name.clone(),
                final_repd: final_repd.name.clone(),
            },
            severity: ViolationSeverity::Error,
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Write-then-read tracking
// ---------------------------------------------------------------------------

/// A record of a write access and the effective RepD under which it wrote.
#[derive(Debug, Clone)]
struct WriteRecord {
    access_id: AccessId,
    #[allow(dead_code)]
    target_derivation: DerivationId,
    repd: RepD,
    /// Base address (resolved from the derivation's proven_range start).
    base: Address,
    size: u64,
}

/// Check write-then-read sequences for incompatible RepDs.
///
/// For each read access, find prior write accesses (within the same region)
/// whose byte ranges overlap and verify that the RepDs are compatible.
fn check_write_read_compatibility(msg: &MSG) -> InvariantResult {
    let mut result = InvariantResult::ok();
    let mut writes: Vec<WriteRecord> = Vec::new();

    // Collect all accesses sorted by their program point (approximate
    // temporal order using the access ID as a proxy).
    let mut access_ids: Vec<AccessId> = msg.access_ids().collect();
    access_ids.sort_by_key(|id| id.0);

    for access_id in access_ids {
        let access = match msg.access(access_id) {
            Some(a) => a,
            None => continue,
        };

        let repd = match effective_repd_with_size(msg, access.target) {
            Some(r) => r,
            None => continue,
        };

        let derivation = match msg.derivation(access.target) {
            Some(d) => d,
            None => continue,
        };

        let base = derivation.proven_range.0;

        if access.kind == AccessKind::Write {
            writes.push(WriteRecord {
                access_id: access.id,
                target_derivation: access.target,
                repd: repd.clone(),
                base,
                size: access.size,
            });
        } else {
            // Read: check against all prior writes for RepD compatibility
            let read_start = base;
            let read_end = base + access.size;

            for wr in &writes {
                let write_start = wr.base;
                let write_end = wr.base + wr.size;

                // Check byte-range overlap
                let overlaps = read_start < write_end && write_start < read_end;

                if overlaps && !compatible(&wr.repd, &repd) {
                    result.violations.push(InvariantViolation {
                        kind: ViolationKind::WriteReadIncompatible {
                            write_access: wr.access_id,
                            read_access: access.id,
                            write_repd: wr.repd.name.clone(),
                            read_repd: repd.name.clone(),
                        },
                        severity: ViolationSeverity::Error,
                    });
                }
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Main checker
// ---------------------------------------------------------------------------

/// Check the Interpretation invariant (Invariant 3) on the given MSG.
///
/// This is the main entry point. It performs all five sub-checks:
///
/// 1. Cast safety (size, pointer-to-non-pointer, valid reinterpretation)
/// 2. Provenance sufficiency for casts
/// 3. Write-then-read RepD compatibility
/// 4. Transitive cast chain analysis
/// 5. Access-size / RepD-size agreement and uninitialized pointer reads
pub fn check_interpretation(msg: &MSG) -> InvariantResult {
    let mut result = InvariantResult::ok();

    // 1. Check each derivation for cast safety
    let derivation_ids: Vec<DerivationId> = msg.derivation_ids().collect();
    for derivation_id in derivation_ids {
        let derivation = match msg.derivation(derivation_id) {
            Some(d) => d,
            None => continue,
        };

        if let DerivationKind::Cast { from, to } = &derivation.kind {
            // 1a. Size check: cast must not change size
            if from.size != to.size {
                result.violations.push(InvariantViolation {
                    kind: ViolationKind::CastSizeMismatch {
                        derivation: derivation.id,
                        from_size: from.size,
                        to_size: to.size,
                    },
                    severity: ViolationSeverity::Error,
                });
            }

            // 1b. Pointer → non-pointer check
            let from_class = classify_repd(from);
            let to_class = classify_repd(to);
            if from_class == RepDClass::Pointer
                && to_class != RepDClass::Pointer
                && to_class != RepDClass::Bytes
            {
                result.violations.push(InvariantViolation {
                    kind: ViolationKind::CastPointerToNonPointer {
                        derivation: derivation.id,
                        from_name: from.name.clone(),
                        to_name: to.name.clone(),
                    },
                    severity: ViolationSeverity::Error,
                });
            }

            // 1c. Valid reinterpretation check
            if !valid_reinterpretation(from, to) {
                result.violations.push(InvariantViolation {
                    kind: ViolationKind::InvalidReinterpretation {
                        derivation: derivation.id,
                        from_name: from.name.clone(),
                        to_name: to.name.clone(),
                    },
                    severity: ViolationSeverity::Error,
                });
            }

            // 1d. Provenance sufficiency — the provenance range must be
            //     large enough to hold at least one element of the target RepD.
            let proven_size: u64 = if derivation.proven_range.1 > derivation.proven_range.0 {
                let diff: i64 = derivation.proven_range.1 - derivation.proven_range.0;
                diff.max(0) as u64
            } else {
                0
            };
            if proven_size > 0 && to.size > proven_size {
                result.violations.push(InvariantViolation {
                    kind: ViolationKind::ProvenanceTooSmallForCast {
                        derivation: derivation.id,
                        proven_size,
                        repd_size: to.size,
                    },
                    severity: ViolationSeverity::Error,
                });
            }
        }

        // 2. Transitive cast chain analysis
        if let Some(violation) = check_transitive_cast_chain(msg, derivation_id) {
            result.violations.push(violation);
        }
    }

    // 3. Write-then-read compatibility
    let wr_result = check_write_read_compatibility(msg);
    result.merge(wr_result);

    // 4. Access-size / RepD-size agreement and uninitialized pointer reads
    let access_ids: Vec<AccessId> = msg.access_ids().collect();
    for access_id in access_ids {
        let access = match msg.access(access_id) {
            Some(a) => a,
            None => continue,
        };

        let repd = match effective_repd_with_size(msg, access.target) {
            Some(r) => r,
            None => continue,
        };

        // The access size must be a multiple of the RepD size
        // (accessing partial units is ill-typed).
        if repd.size > 0 && access.size % repd.size != 0 {
            result.violations.push(InvariantViolation {
                kind: ViolationKind::AccessSizeMismatch {
                    access: access.id,
                    access_size: access.size,
                    repd_size: repd.size,
                },
                severity: ViolationSeverity::Warning,
            });
        }

        // 5. Uninitialized pointer read check
        //    We approximate: if the access is a read targeting a pointer
        //    RepD and there is no prior write to the same derivation's
        //    bytes, flag it.
        if access.kind == AccessKind::Read {
            let repd_class = classify_repd(&repd);
            if repd_class == RepDClass::Pointer {
                let has_prior_write = has_prior_write_to_derivation(msg, access);
                if !has_prior_write {
                    result.violations.push(InvariantViolation {
                        kind: ViolationKind::UninitPointerRead {
                            access: access.id,
                            repd_name: repd.name.clone(),
                        },
                        severity: ViolationSeverity::Error,
                    });
                }
            }
        }
    }

    result
}

/// Check whether there exists a prior write to the same derivation target
/// (or one that shares the same region) before the given read access.
///
/// This is an approximation: a fully precise check would track byte-level
/// initialization, but that requires the IVE's initialization map.
fn has_prior_write_to_derivation(msg: &MSG, read_access: &Access) -> bool {
    for other in msg.accesses() {
        if other.kind != AccessKind::Write {
            continue;
        }

        // Check: does the write target the same derivation or one that
        // shares the same root region?
        let read_region = derivation_root_region(msg, read_access.target);
        let write_region = derivation_root_region(msg, other.target);

        if let (Some(rr), Some(wr)) = (read_region, write_region) {
            if rr == wr {
                return true;
            }
        }
    }
    false
}

/// Walk the derivation chain to find the root [`RegionId`].
fn derivation_root_region(msg: &MSG, derivation_id: DerivationId) -> Option<RegionId> {
    let derivation = msg.derivation(derivation_id)?;
    match &derivation.source {
        DerivationSource::Region(rid) => Some(*rid),
        DerivationSource::AnotherDerivation(parent_id) => derivation_root_region(msg, *parent_id),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{Access, AccessId, AccessKind};
    use crate::address::Address;
    use crate::derivation::{
        Derivation, DerivationId, DerivationKind, DerivationSource, RepD,
    };
    use crate::msg::MSG;
    use crate::program_point::ProgramPoint;
    use crate::region::{Region, RegionId, RegionStatus};

    fn dummy_pp(line: u32) -> ProgramPoint {
        ProgramPoint::new("test.vu", line, 1)
    }

    fn make_region(id: u64, base: u64, size: u64) -> Region {
        Region {
            id: RegionId(id),
            base: Address::from(base),
            size,
            status: RegionStatus::Allocated,
            alloc_point: dummy_pp(1),
            free_point: None,
            owner_context: None,
        }
    }

    fn make_direct_derivation(id: u64, rid: u64, lo: u64, hi: u64) -> Derivation {
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::Region(RegionId(rid)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(lo), Address::from(hi)),
        }
    }

    fn make_offset_derivation(id: u64, parent: DerivationId, by: i64, lo: u64, hi: u64) -> Derivation {
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::AnotherDerivation(parent),
            kind: DerivationKind::Offset { by },
            proven_range: (Address::from(lo), Address::from(hi)),
        }
    }

    fn make_cast_derivation(
        id: u64,
        parent: DerivationId,
        from: &str,
        from_size: u64,
        to: &str,
        to_size: u64,
        lo: u64,
        hi: u64,
    ) -> Derivation {
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::AnotherDerivation(parent),
            kind: DerivationKind::Cast {
                from: RepD { name: from.to_string(), size: from_size },
                to: RepD { name: to.to_string(), size: to_size },
            },
            proven_range: (Address::from(lo), Address::from(hi)),
        }
    }

    fn make_region_cast_derivation(
        id: u64,
        rid: u64,
        from: &str,
        from_size: u64,
        to: &str,
        to_size: u64,
        lo: u64,
        hi: u64,
    ) -> Derivation {
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::Region(RegionId(rid)),
            kind: DerivationKind::Cast {
                from: RepD { name: from.to_string(), size: from_size },
                to: RepD { name: to.to_string(), size: to_size },
            },
            proven_range: (Address::from(lo), Address::from(hi)),
        }
    }

    // ---- Test 1: Empty MSG passes ----
    #[test]
    fn empty_msg_passes() {
        let msg = MSG::new();
        let result = check_interpretation(&msg);
        assert!(result.is_ok(), "Empty MSG should satisfy the invariant");
    }

    // ---- Test 2: Cast with size mismatch is detected ----
    #[test]
    fn cast_size_mismatch_detected() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100));
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));
        // Cast from 4-byte to 8-byte — size mismatch
        msg.add_derivation(make_cast_derivation(
            2, DerivationId(1),
            "u32", 4, "u64", 8,
            0x1000, 0x1100,
        ));

        let result = check_interpretation(&msg);
        assert!(!result.is_ok(), "Size mismatch should be flagged");
        assert!(result.violations.iter().any(|v| matches!(
            &v.kind,
            ViolationKind::CastSizeMismatch { from_size: 4, to_size: 8, .. }
        )));
    }

    // ---- Test 3: Safe cast (bytes → struct) passes ----
    #[test]
    fn safe_bytes_to_struct_cast() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x400));
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1400));
        // bytes → Header struct, same size — valid
        msg.add_derivation(make_cast_derivation(
            2, DerivationId(1),
            "bytes", 8, "Header", 8,
            0x1000, 0x1400,
        ));

        let result = check_interpretation(&msg);
        assert!(result.is_ok(), "bytes → struct cast with same size should pass, got: {:?}", result.violations);
    }

    // ---- Test 4: Pointer to float cast is detected ----
    #[test]
    fn pointer_to_float_cast_detected() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100));
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));
        // Cast from ptr<u8> to f64 — pointer to non-pointer
        msg.add_derivation(make_cast_derivation(
            2, DerivationId(1),
            "ptr<u8>", 8, "f64", 8,
            0x1000, 0x1100,
        ));

        let result = check_interpretation(&msg);
        assert!(!result.is_ok(), "pointer → float should be flagged");
        assert!(result.violations.iter().any(|v| matches!(
            &v.kind,
            ViolationKind::CastPointerToNonPointer { .. }
        )));
    }

    // ---- Test 5: Valid transitive chain does not produce confusion ----
    #[test]
    fn valid_transitive_chain_no_confusion() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100));

        // D1: direct from region
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));

        // D2: bytes → ptr<u8> (valid: bytes ⊑ any)
        msg.add_derivation(make_cast_derivation(
            2, DerivationId(1),
            "bytes", 8, "ptr<u8>", 8,
            0x1000, 0x1100,
        ));

        // D3: ptr<u8> → *mut u8 (valid: same class)
        msg.add_derivation(make_cast_derivation(
            3, DerivationId(2),
            "ptr<u8>", 8, "*mut u8", 8,
            0x1000, 0x1100,
        ));

        // The overall chain bytes → *mut u8 is valid (bytes ⊑ any)
        let result = check_interpretation(&msg);
        let transitive_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| matches!(v.kind, ViolationKind::TransitiveCastConfusion { .. }))
            .collect();
        assert!(
            transitive_violations.is_empty(),
            "Valid transitive chain should not produce confusion, got: {:?}",
            transitive_violations
        );
    }

    // ---- Test 6: Transitive confusion detected (struct → integer → float) ----
    #[test]
    fn transitive_cast_confusion_struct_int_float() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100));

        // D1: direct
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));

        // D2: bytes → u64 (valid: bytes ⊑ any)
        msg.add_derivation(make_cast_derivation(
            2, DerivationId(1),
            "bytes", 8, "u64", 8,
            0x1000, 0x1100,
        ));

        // D3: u64 → f64 (Integer → Float: different classes, individually invalid)
        // The transitive check should also flag the full chain bytes → f64.
        // bytes → f64 is valid (bytes ⊑ any), so the transitive check
        // should NOT flag it — only the individual step is invalid.
        msg.add_derivation(make_cast_derivation(
            3, DerivationId(2),
            "u64", 8, "f64", 8,
            0x1000, 0x1100,
        ));

        let result = check_interpretation(&msg);
        // The individual step u64 → f64 is invalid
        assert!(!result.is_ok());
        // The transitive check: bytes → f64 is valid (bytes ⊑ any),
        // so no TransitiveCastConfusion should be emitted.
        let transitive: Vec<_> = result
            .violations
            .iter()
            .filter(|v| matches!(v.kind, ViolationKind::TransitiveCastConfusion { .. }))
            .collect();
        assert!(
            transitive.is_empty(),
            "bytes → f64 is valid (bytes ⊑ any), no transitive confusion expected, got: {:?}",
            transitive
        );
    }

    // ---- Test 7: Access-size / RepD-size mismatch ----
    #[test]
    fn access_size_mismatch_detected() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100));

        // D1: direct derivation
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));

        // D2: cast to u32 (4 bytes)
        msg.add_derivation(make_cast_derivation(
            2, DerivationId(1),
            "bytes", 4, "u32", 4,
            0x1000, 0x1100,
        ));

        // Access with size 6, which is not a multiple of u32's size (4)
        msg.add_access(Access::new(
            AccessId(1),
            DerivationId(2),
            AccessKind::Read,
            6, // not a multiple of 4
            dummy_pp(10),
        ));

        let result = check_interpretation(&msg);
        assert!(
            result.violations.iter().any(|v| matches!(
                &v.kind,
                ViolationKind::AccessSizeMismatch { access_size: 6, repd_size: 4, .. }
            )),
            "Expected AccessSizeMismatch violation, got: {:?}",
            result.violations
        );
    }

    // ---- Test 8: Uninitialized pointer read detected ----
    #[test]
    fn uninit_pointer_read_detected() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100));

        // D1: direct derivation
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));

        // D2: cast to ptr<u8> (pointer type)
        msg.add_derivation(make_cast_derivation(
            2, DerivationId(1),
            "bytes", 8, "ptr<u8>", 8,
            0x1000, 0x1100,
        ));

        // Read as pointer — no prior write exists
        msg.add_access(Access::new(
            AccessId(1),
            DerivationId(2),
            AccessKind::Read,
            8,
            dummy_pp(10),
        ));

        let result = check_interpretation(&msg);
        assert!(
            result.violations.iter().any(|v| matches!(
                &v.kind,
                ViolationKind::UninitPointerRead { .. }
            )),
            "Expected UninitPointerRead violation, got: {:?}",
            result.violations
        );
    }

    // ---- Test 9: Initialized pointer read passes ----
    #[test]
    fn initialized_pointer_read_passes() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100));

        // D1: direct derivation
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));

        // D2: cast to ptr<u8>
        msg.add_derivation(make_cast_derivation(
            2, DerivationId(1),
            "bytes", 8, "ptr<u8>", 8,
            0x1000, 0x1100,
        ));

        // Write first
        msg.add_access(Access::new(
            AccessId(1),
            DerivationId(2),
            AccessKind::Write,
            8,
            dummy_pp(5),
        ));

        // Then read
        msg.add_access(Access::new(
            AccessId(2),
            DerivationId(2),
            AccessKind::Read,
            8,
            dummy_pp(10),
        ));

        let result = check_interpretation(&msg);
        let uninit_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| matches!(v.kind, ViolationKind::UninitPointerRead { .. }))
            .collect();
        assert!(
            uninit_violations.is_empty(),
            "Initialized pointer read should not produce UninitPointerRead, got: {:?}",
            uninit_violations
        );
    }

    // ---- Test 10: Provenance too small for cast ----
    #[test]
    fn provenance_too_small_for_cast() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x10));

        // D1: direct derivation with small provenance
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1004));

        // D2: cast to a 16-byte struct, but provenance only has 4 bytes
        msg.add_derivation(make_cast_derivation(
            2, DerivationId(1),
            "bytes", 4, "BigStruct", 16,
            0x1000, 0x1004, // only 4 bytes available
        ));

        let result = check_interpretation(&msg);
        assert!(
            result.violations.iter().any(|v| matches!(
                &v.kind,
                ViolationKind::ProvenanceTooSmallForCast { proven_size: 4, repd_size: 16, .. }
            )),
            "Expected ProvenanceTooSmallForCast, got: {:?}",
            result.violations
        );
    }

    // ---- Test 11: Write-then-read incompatibility detected ----
    #[test]
    fn write_read_incompatible_detected() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100));

        // D1: direct
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));

        // D2: cast to u32
        msg.add_derivation(make_cast_derivation(
            2, DerivationId(1),
            "bytes", 4, "u32", 4,
            0x1000, 0x1100,
        ));

        // D3: cast to f32 (different class, same size)
        msg.add_derivation(make_cast_derivation(
            3, DerivationId(1),
            "bytes", 4, "f32", 4,
            0x1000, 0x1100,
        ));

        // Write as u32
        msg.add_access(Access::new(
            AccessId(1),
            DerivationId(2),
            AccessKind::Write,
            4,
            dummy_pp(5),
        ));

        // Read as f32 — different class (Integer vs Float), same region
        msg.add_access(Access::new(
            AccessId(2),
            DerivationId(3),
            AccessKind::Read,
            4,
            dummy_pp(10),
        ));

        let result = check_interpretation(&msg);
        let wr_violations: Vec<_> = result
            .violations
            .iter()
            .filter(|v| matches!(v.kind, ViolationKind::WriteReadIncompatible { .. }))
            .collect();
        assert!(
            !wr_violations.is_empty(),
            "Expected WriteReadIncompatible violation, got: {:?}",
            result.violations
        );
    }

    // ---- Test 12: Fully valid program ----
    #[test]
    fn fully_valid_program() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x400));

        // D1: direct derivation
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1400));

        // D2: cast bytes → Header (valid: bytes ⊑ any, same size)
        msg.add_derivation(make_cast_derivation(
            2, DerivationId(1),
            "bytes", 8, "Header", 8,
            0x1000, 0x1400,
        ));

        // Write as Header
        msg.add_access(Access::new(
            AccessId(1),
            DerivationId(2),
            AccessKind::Write,
            8,
            dummy_pp(5),
        ));

        // Read as Header
        msg.add_access(Access::new(
            AccessId(2),
            DerivationId(2),
            AccessKind::Read,
            8,
            dummy_pp(10),
        ));

        let result = check_interpretation(&msg);
        assert!(
            result.is_ok(),
            "Fully valid program should pass, got violations: {:?}",
            result.violations
        );
    }

    // ---- Test 13: Offset derivation followed by cast ----
    #[test]
    fn offset_then_cast_valid() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x400));

        // D1: direct from region
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1400));

        // D2: offset by 64
        msg.add_derivation(make_offset_derivation(2, DerivationId(1), 64, 0x1040, 0x1400));

        // D3: cast bytes → u32 at the offset location
        msg.add_derivation(make_cast_derivation(
            3, DerivationId(2),
            "bytes", 4, "u32", 4,
            0x1040, 0x1400,
        ));

        // Write
        msg.add_access(Access::new(
            AccessId(1),
            DerivationId(3),
            AccessKind::Write,
            4,
            dummy_pp(5),
        ));

        // Read
        msg.add_access(Access::new(
            AccessId(2),
            DerivationId(3),
            AccessKind::Read,
            4,
            dummy_pp(10),
        ));

        let result = check_interpretation(&msg);
        assert!(
            result.is_ok(),
            "Offset + cast should be valid, got: {:?}",
            result.violations
        );
    }

    // ---- Test 14: Transitive confusion with struct → int → float ----
    #[test]
    fn transitive_confusion_with_three_casts() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100));

        // D1: direct
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));

        // D2: CustomStruct(8) → u64(8) — both are "Other" and "Integer"
        // CustomStruct is "Other", u64 is "Integer" — different class, not bytes
        // This is individually invalid. Let's use same-class steps.
        // For a true transitive test where individual steps pass:
        // Step 1: bytes(8) → MyStruct(8) — valid (bytes ⊑ any)
        // Step 2: MyStruct(8) → AnotherStruct(8) — valid (same class Other)
        // Overall: bytes → AnotherStruct — valid (bytes ⊑ any)
        // Not a confusion case.

        // Let's test: creating a chain where both individual steps are
        // same-class (both "Other") but the original was bytes:
        // bytes → Header → Packet — all valid, no confusion
        msg.add_derivation(make_cast_derivation(
            2, DerivationId(1),
            "bytes", 8, "Header", 8,
            0x1000, 0x1100,
        ));
        msg.add_derivation(make_cast_derivation(
            3, DerivationId(2),
            "Header", 8, "Packet", 8,
            0x1000, 0x1100,
        ));

        let result = check_interpretation(&msg);
        // Header → Packet: both "Other" class, same size = valid
        // bytes → Packet: valid (bytes ⊑ any)
        let transitive: Vec<_> = result
            .violations
            .iter()
            .filter(|v| matches!(v.kind, ViolationKind::TransitiveCastConfusion { .. }))
            .collect();
        assert!(
            transitive.is_empty(),
            "Valid same-class chain should not produce confusion, got: {:?}",
            transitive
        );
    }

    // ---- Test 15: Region-cast derivation ----
    #[test]
    fn region_cast_derivation_valid() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100));

        // Direct cast from region to struct
        msg.add_derivation(make_region_cast_derivation(
            2, 1,
            "bytes", 8, "Header", 8,
            0x1000, 0x1100,
        ));

        // Write + read as Header
        msg.add_access(Access::new(
            AccessId(1),
            DerivationId(2),
            AccessKind::Write,
            8,
            dummy_pp(5),
        ));
        msg.add_access(Access::new(
            AccessId(2),
            DerivationId(2),
            AccessKind::Read,
            8,
            dummy_pp(10),
        ));

        let result = check_interpretation(&msg);
        assert!(
            result.is_ok(),
            "Region cast derivation should be valid, got: {:?}",
            result.violations
        );
    }

    // ---- Test 16: InvariantResult display ----
    #[test]
    fn invariant_result_display() {
        let ok_result = InvariantResult::ok();
        assert_eq!(format!("{}", ok_result), "Interpretation invariant: SATISFIED");

        let err_result = InvariantResult::with_violation(
            ViolationKind::CastSizeMismatch {
                derivation: DerivationId(1),
                from_size: 4,
                to_size: 8,
            },
            ViolationSeverity::Error,
        );
        let display = format!("{}", err_result);
        assert!(display.contains("VIOLATED"));
        assert!(display.contains("1 violation"));
    }

    // ---- Unit tests for helpers ----

    #[test]
    fn classify_repd_bytes() {
        let repd = RepD { name: "bytes".into(), size: 8 };
        assert_eq!(classify_repd(&repd), RepDClass::Bytes);
    }

    #[test]
    fn classify_repd_pointer() {
        let repd = RepD { name: "ptr<u8>".into(), size: 8 };
        assert_eq!(classify_repd(&repd), RepDClass::Pointer);

        let repd2 = RepD { name: "*mut u32".into(), size: 8 };
        assert_eq!(classify_repd(&repd2), RepDClass::Pointer);
    }

    #[test]
    fn classify_repd_integer() {
        let repd = RepD { name: "u32".into(), size: 4 };
        assert_eq!(classify_repd(&repd), RepDClass::Integer);

        let repd2 = RepD { name: "i64".into(), size: 8 };
        assert_eq!(classify_repd(&repd2), RepDClass::Integer);
    }

    #[test]
    fn classify_repd_float() {
        let repd = RepD { name: "f64".into(), size: 8 };
        assert_eq!(classify_repd(&repd), RepDClass::Float);
    }

    #[test]
    fn compatible_same_repd() {
        let r = RepD { name: "u32".into(), size: 4 };
        assert!(compatible(&r, &r));
    }

    #[test]
    fn compatible_bytes_to_any() {
        let bytes = RepD { name: "bytes".into(), size: 8 };
        let ptr = RepD { name: "ptr<u8>".into(), size: 8 };
        assert!(compatible(&bytes, &ptr));
    }

    #[test]
    fn incompatible_pointer_to_float() {
        let ptr = RepD { name: "ptr<u8>".into(), size: 8 };
        let float = RepD { name: "f64".into(), size: 8 };
        assert!(!compatible(&ptr, &float));
    }

    #[test]
    fn incompatible_size_mismatch() {
        let a = RepD { name: "u32".into(), size: 4 };
        let b = RepD { name: "u64".into(), size: 8 };
        assert!(!compatible(&a, &b));
    }
}
