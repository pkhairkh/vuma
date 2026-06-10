//! # Interpretation Proof Objects
//!
//! Formal proof objects for the VUMA Interpretation Invariant (Invariant 3):
//! every access respects the Representation Descriptor (RepD) of its target.
//!
//! The interpretation invariant guarantees that:
//! - Every read uses the correct Byte Descriptor (BD) for the memory it accesses.
//! - Write-read pairs have compatible BDs: a reader sees the BD that the writer
//!   established (or a compatible reinterpretation thereof).
//! - Cast derivations (reinterpretations) are safe: size, alignment, and semantic
//!   compatibility constraints are all satisfied.
//!
//! # Core Proof Objects
//!
//! - [`InterpretationProof`]: Top-level proof that all reads in an MSG use
//!   correct BDs.
//! - [`BDCompatibilityProof`]: Proof that a specific write-read pair has
//!   compatible BDs.
//! - [`ReinterpretationSafetyProof`]: Proof that a specific cast derivation
//!   is a safe reinterpretation.
//!
//! # Prover
//!
//! The [`prove_interpretation`] function constructs an `InterpretationProof`
//! from a Memory State Graph (MSG) by:
//! 1. Tracing BDs through derivation chains (BD-tracing tactic).
//! 2. Checking compatibility for every write-read pair (compatibility-checking
//!    tactic).
//! 3. Verifying size and alignment for every cast (size-alignment-verification
//!    tactic).
//!
//! # Tactics
//!
//! - [`InterpTactic::BDTracing`]: Walk derivation chains to compute the
//!   effective RepD/BD for each access.
//! - [`InterpTactic::CompatibilityChecking`]: For each write-read pair that
//!   targets overlapping bytes, verify BD compatibility.
//! - [`InterpTactic::SizeAlignmentVerification`]: For each cast derivation,
//!   verify size ≤ source remaining and address alignment.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::proof::{
    AccessId, Conclusion, Fact, FactId, Goal, Proof, ProofContext, ProofStep, RegionId,
    Target,
};
use crate::rules::InferenceRule;

// ---------------------------------------------------------------------------
// Representation Descriptor (RepD)
// ---------------------------------------------------------------------------

/// Unique identifier for a Representation Descriptor.
pub type RepDId = u64;

/// Unique identifier for a Derivation within the MSG.
pub type DerivationId = u64;

/// Program point identifier.
pub type ProgramPoint = u64;

/// Byte descriptor category — the high-level classification of how a range of
/// bytes is interpreted.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum BDKind {
    /// Raw bytes — the universal supertype; any RepD can be read as bytes.
    Bytes,
    /// Integer type (signed or unsigned, any width).
    Integer,
    /// Floating-point type.
    Float,
    /// Pointer type — requires initialization check.
    Pointer,
    /// Struct or compound type with named fields.
    Struct,
    /// Union type — the BD may be any of the member types.
    Union,
    /// A custom/user-defined representation.
    Custom,
}

impl std::fmt::Display for BDKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BDKind::Bytes => write!(f, "bytes"),
            BDKind::Integer => write!(f, "integer"),
            BDKind::Float => write!(f, "float"),
            BDKind::Pointer => write!(f, "pointer"),
            BDKind::Struct => write!(f, "struct"),
            BDKind::Union => write!(f, "union"),
            BDKind::Custom => write!(f, "custom"),
        }
    }
}

/// A Representation Descriptor (RepD) — describes how a range of bytes is
/// interpreted, including size, alignment, kind, and whether the bytes are
/// initialized.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepD {
    /// Unique identifier for this RepD.
    pub id: RepDId,
    /// High-level classification of the byte interpretation.
    pub kind: BDKind,
    /// Size in bytes of the described value.
    pub size: u64,
    /// Required alignment in bytes.
    pub alignment: u64,
    /// Whether the described bytes are known to be initialized.
    pub initialized: bool,
}

impl RepD {
    /// Create a new RepD.
    pub fn new(id: RepDId, kind: BDKind, size: u64, alignment: u64, initialized: bool) -> Self {
        Self {
            id,
            kind,
            size,
            alignment,
            initialized,
        }
    }

    /// Convenience: create a bytes RepD.
    pub fn bytes(id: RepDId, size: u64, initialized: bool) -> Self {
        Self::new(id, BDKind::Bytes, size, 1, initialized)
    }

    /// Convenience: create an integer RepD.
    pub fn integer(id: RepDId, size: u64, alignment: u64, initialized: bool) -> Self {
        Self::new(id, BDKind::Integer, size, alignment, initialized)
    }

    /// Convenience: create a pointer RepD.
    pub fn pointer(id: RepDId, size: u64, alignment: u64, initialized: bool) -> Self {
        Self::new(id, BDKind::Pointer, size, alignment, initialized)
    }

    /// Check whether this RepD is a sub-RepD of `other` (i.e., `self ⊑ other`).
    /// Bytes is the universal supertype: any RepD ⊑ bytes.
    pub fn is_sub_repd_of(&self, other: &RepD) -> bool {
        if self.id == other.id {
            return true;
        }
        // bytes is the universal supertype.
        if other.kind == BDKind::Bytes {
            return true;
        }
        // Same kind with equal or larger size is a valid sub-RepD.
        if self.kind == other.kind && self.size <= other.size {
            return true;
        }
        false
    }

    /// Check compatibility between two RepDs for a write-read pair.
    ///
    /// `compatible(r_write, r_read)` holds iff:
    /// - Sizes match,
    /// - Alignment of the read's RepD divides the access address,
    /// - The read's RepD is a valid reinterpretation of the write's RepD,
    /// - If the read expects a pointer, the bytes must be initialized.
    pub fn compatible_with(&self, read: &RepD, access_addr: u64) -> Compatibility {
        // Size check: read size must not exceed write size.
        if read.size > self.size {
            return Compatibility::Incompatible(format!(
                "size mismatch: write BD size {} < read BD size {}",
                self.size, read.size
            ));
        }

        // Alignment check: the access address must satisfy the read BD's alignment.
        if read.alignment > 0 && !access_addr.is_multiple_of(read.alignment) {
            return Compatibility::Incompatible(format!(
                "alignment violation: address 0x{:x} not aligned to {} bytes",
                access_addr, read.alignment
            ));
        }

        // Pointer initialization check: reading as pointer requires initialized bytes.
        if read.kind == BDKind::Pointer && !self.initialized {
            return Compatibility::Incompatible(
                "reading uninitialized bytes as pointer type is forbidden".into(),
            );
        }

        // Valid reinterpretation check.
        if !valid_reinterpretation(self, read) {
            return Compatibility::Incompatible(format!(
                "invalid reinterpretation: {} -> {}",
                self.kind, read.kind
            ));
        }

        Compatibility::Compatible
    }
}

// ---------------------------------------------------------------------------
// Compatibility result
// ---------------------------------------------------------------------------

/// Result of a BD compatibility check between a write's BD and a read's BD.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Compatibility {
    /// The BDs are compatible.
    Compatible,
    /// The BDs are incompatible, with a reason.
    Incompatible(String),
}

impl Compatibility {
    /// Returns true if compatible.
    pub fn is_compatible(&self) -> bool {
        matches!(self, Compatibility::Compatible)
    }
}

// ---------------------------------------------------------------------------
// Valid reinterpretation check
// ---------------------------------------------------------------------------

/// Check whether reinterpreting `source` as `target` is semantically valid.
///
/// Rules (from the VUMA Invariant Spec §5.1):
/// - Same RepD: always valid.
/// - Source ⊑ target (sub-RepD): valid.
/// - Pointer → non-pointer, non-bytes: invalid.
/// - Other cases: need IVE case analysis (conservatively invalid here).
fn valid_reinterpretation(source: &RepD, target: &RepD) -> bool {
    // Same RepD → always valid.
    if source.id == target.id {
        return true;
    }
    // Source is sub-RepD of target → valid.
    if source.is_sub_repd_of(target) {
        return true;
    }
    // Pointer → non-pointer, non-bytes: invalid.
    if source.kind == BDKind::Pointer
        && target.kind != BDKind::Pointer
        && target.kind != BDKind::Bytes
    {
        return false;
    }
    // Bytes → anything: valid (bytes is the universal source).
    if source.kind == BDKind::Bytes {
        return true;
    }
    // Union member: if target is a union, valid.
    if target.kind == BDKind::Union {
        return true;
    }
    // Conservative: anything else needs IVE analysis → reject.
    false
}

// ---------------------------------------------------------------------------
// Memory State Graph (simplified model for proofs)
// ---------------------------------------------------------------------------

/// Region status in the memory model.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RegionStatus {
    /// Heap-allocated and live.
    Allocated,
    /// Freed.
    Freed,
    /// Stack-allocated.
    Stack,
    /// Memory-mapped.
    Mapped,
}

/// A memory region — a contiguous range of bytes with an associated default RepD.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Region {
    pub id: RegionId,
    pub base_addr: u64,
    pub size: u64,
    pub status: RegionStatus,
    pub default_repd: RepDId,
    pub alloc_point: ProgramPoint,
    pub free_point: Option<ProgramPoint>,
}

impl Region {
    /// Create a new region.
    pub fn new(
        id: RegionId,
        base_addr: u64,
        size: u64,
        status: RegionStatus,
        default_repd: RepDId,
    ) -> Self {
        Self {
            id,
            base_addr,
            size,
            status,
            default_repd,
            alloc_point: 0,
            free_point: None,
        }
    }

    /// Returns the address range [base_addr, base_addr + size).
    pub fn range(&self) -> std::ops::Range<u64> {
        self.base_addr..self.base_addr + self.size
    }
}

/// A derivation — computation of an address from a region or another derivation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Derivation {
    pub id: DerivationId,
    /// The source: either a region id (Some(region_id), None for derivation)
    /// or a derivation id (None, Some(derivation_id)).
    pub source_region: Option<RegionId>,
    pub source_derivation: Option<DerivationId>,
    pub offset: i64,
    /// If this is a cast derivation, the target RepD.
    pub cast: Option<RepDId>,
}

impl Derivation {
    /// Create a base derivation (source = region, offset = 0, no cast).
    pub fn base(region_id: RegionId) -> Self {
        Self {
            id: 0,
            source_region: Some(region_id),
            source_derivation: None,
            offset: 0,
            cast: None,
        }
    }

    /// Create an offset derivation.
    pub fn offset_from_region(region_id: RegionId, offset: i64) -> Self {
        Self {
            id: 0,
            source_region: Some(region_id),
            source_derivation: None,
            offset,
            cast: None,
        }
    }

    /// Create a cast derivation.
    pub fn cast_from(derivation_id: DerivationId, target_repd: RepDId) -> Self {
        Self {
            id: 0,
            source_region: None,
            source_derivation: Some(derivation_id),
            offset: 0,
            cast: Some(target_repd),
        }
    }

    /// Returns true if this is a cast derivation.
    pub fn is_cast(&self) -> bool {
        self.cast.is_some()
    }

    /// Returns true if this is a base derivation.
    pub fn is_base(&self) -> bool {
        self.source_region.is_some() && self.source_derivation.is_none() && self.offset == 0 && self.cast.is_none()
    }
}

/// Access kind — read or write.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AccessKind {
    Read,
    Write,
}

/// A memory access.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Access {
    pub id: AccessId,
    pub target_derivation: DerivationId,
    pub kind: AccessKind,
    pub size: u64,
    pub program_point: ProgramPoint,
    /// The RepD expected by this access.
    pub expected_repd: RepDId,
}

impl Access {
    /// Create a new access.
    pub fn new(
        id: AccessId,
        target: DerivationId,
        kind: AccessKind,
        size: u64,
        pp: ProgramPoint,
        expected_repd: RepDId,
    ) -> Self {
        Self {
            id,
            target_derivation: target,
            kind,
            size,
            program_point: pp,
            expected_repd,
        }
    }

    /// Returns true if this is a write access.
    pub fn is_write(&self) -> bool {
        self.kind == AccessKind::Write
    }

    /// Returns true if this is a read access.
    pub fn is_read(&self) -> bool {
        self.kind == AccessKind::Read
    }
}

/// A Memory State Graph — the central data structure for interpretation proofs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MSG {
    pub regions: Vec<Region>,
    pub derivations: Vec<Derivation>,
    pub accesses: Vec<Access>,
    pub repds: Vec<RepD>,
}

impl MSG {
    /// Create an empty MSG.
    pub fn new() -> Self {
        Self {
            regions: Vec::new(),
            derivations: Vec::new(),
            accesses: Vec::new(),
            repds: Vec::new(),
        }
    }

    /// Look up a region by id.
    pub fn get_region(&self, id: RegionId) -> Option<&Region> {
        self.regions.iter().find(|r| r.id == id)
    }

    /// Look up a derivation by id.
    pub fn get_derivation(&self, id: DerivationId) -> Option<&Derivation> {
        self.derivations.iter().find(|d| d.id == id)
    }

    /// Look up an access by id.
    pub fn get_access(&self, id: AccessId) -> Option<&Access> {
        self.accesses.iter().find(|a| a.id == id)
    }

    /// Look up a RepD by id.
    pub fn get_repd(&self, id: RepDId) -> Option<&RepD> {
        self.repds.iter().find(|r| r.id == id)
    }

    /// Resolve the root region of a derivation by walking the source chain.
    pub fn region_of(&self, derivation_id: DerivationId) -> Option<RegionId> {
        let mut current_id = derivation_id;
        let mut visited = std::collections::HashSet::new();
        loop {
            if !visited.insert(current_id) {
                // Cycle detected — should not happen in well-formed MSG.
                return None;
            }
            let deriv = self.get_derivation(current_id)?;
            if let Some(rid) = deriv.source_region {
                return Some(rid);
            }
            if let Some(did) = deriv.source_derivation {
                current_id = did;
            } else {
                // No source — malformed derivation.
                return None;
            }
        }
    }

    /// Compute the effective RepD of a derivation by walking the chain and
    /// finding the most recent cast, or falling back to the region's default.
    pub fn repd_of(&self, derivation_id: DerivationId) -> Option<RepDId> {
        let mut current_id = derivation_id;
        let mut visited = std::collections::HashSet::new();
        loop {
            if !visited.insert(current_id) {
                return None; // cycle
            }
            let deriv = self.get_derivation(current_id)?;
            // If this derivation has a cast, that determines the effective RepD.
            if let Some(cast_repd) = deriv.cast {
                return Some(cast_repd);
            }
            // Walk to the source.
            if let Some(rid) = deriv.source_region {
                let region = self.get_region(rid)?;
                return Some(region.default_repd);
            }
            if let Some(did) = deriv.source_derivation {
                current_id = did;
            } else {
                return None;
            }
        }
    }

    /// Resolve the concrete address of a derivation.
    pub fn addr_of(&self, derivation_id: DerivationId) -> Option<u64> {
        let mut current_id = derivation_id;
        let mut cumulative_offset: i64 = 0;
        let mut visited = std::collections::HashSet::new();
        loop {
            if !visited.insert(current_id) {
                return None; // cycle
            }
            let deriv = self.get_derivation(current_id)?;
            cumulative_offset += deriv.offset;
            if let Some(rid) = deriv.source_region {
                let region = self.get_region(rid)?;
                return Some(
                    (region.base_addr as i64 + cumulative_offset) as u64,
                );
            }
            if let Some(did) = deriv.source_derivation {
                current_id = did;
            } else {
                return None;
            }
        }
    }
}

impl Default for MSG {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Proof Failure
// ---------------------------------------------------------------------------

/// A failure to prove the interpretation invariant, with diagnostic info.
#[derive(Debug, Clone, Error)]
pub enum ProofFailure {
    /// A read access uses an incompatible BD.
    #[error("incompatible BD for access {access_id}: {reason}")]
    IncompatibleBD {
        access_id: AccessId,
        reason: String,
    },

    /// A cast derivation is not a safe reinterpretation.
    #[error("unsafe reinterpretation at derivation {derivation_id}: {reason}")]
    UnsafeReinterpretation {
        derivation_id: DerivationId,
        reason: String,
    },

    /// A size or alignment constraint was violated.
    #[error("size/alignment violation at derivation {derivation_id}: {reason}")]
    SizeAlignmentViolation {
        derivation_id: DerivationId,
        reason: String,
    },

    /// A derivation could not be resolved (malformed MSG).
    #[error("unresolvable derivation {derivation_id}: {reason}")]
    UnresolvableDerivation {
        derivation_id: DerivationId,
        reason: String,
    },

    /// Reading uninitialized memory as a pointer type.
    #[error("uninitialized pointer read at access {access_id}")]
    UninitializedPointerRead { access_id: AccessId },

    /// Internal error during proof construction.
    #[error("internal proof error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// BDCompatibilityProof
// ---------------------------------------------------------------------------

/// Proof that a specific write-read pair has compatible BDs.
///
/// This proof demonstrates that when a write establishes a BD for a range of
/// bytes, and a subsequent read accesses those bytes, the read's expected BD
/// is compatible with the write's BD.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BDCompatibilityProof {
    /// The write access.
    pub write_access_id: AccessId,
    /// The read access.
    pub read_access_id: AccessId,
    /// The BD established by the write.
    pub write_repd: RepDId,
    /// The BD expected by the read.
    pub read_repd: RepDId,
    /// The resolved address of the read.
    pub read_addr: u64,
    /// The compatibility result.
    pub compatibility: Compatibility,
    /// The formal proof object.
    pub proof: Proof,
}

impl BDCompatibilityProof {
    /// Construct a BD compatibility proof for a write-read pair.
    pub fn new(
        write_access_id: AccessId,
        read_access_id: AccessId,
        write_repd: RepDId,
        read_repd: RepDId,
        read_addr: u64,
        compatibility: Compatibility,
        proof: Proof,
    ) -> Self {
        Self {
            write_access_id,
            read_access_id,
            write_repd,
            read_repd,
            read_addr,
            compatibility,
            proof,
        }
    }

    /// Returns true if the proof establishes compatibility.
    pub fn is_compatible(&self) -> bool {
        self.compatibility.is_compatible()
    }
}

// ---------------------------------------------------------------------------
// ReinterpretationSafetyProof
// ---------------------------------------------------------------------------

/// Proof that a cast derivation is a safe reinterpretation.
///
/// A cast is safe when:
/// 1. The target RepD's size does not exceed the remaining bytes from the
///    offset to the end of the source region.
/// 2. The resolved address satisfies the target RepD's alignment.
/// 3. The semantic reinterpretation is valid (e.g., not pointer → float).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReinterpretationSafetyProof {
    /// The cast derivation being proven safe.
    pub derivation_id: DerivationId,
    /// The source RepD (before the cast).
    pub source_repd: RepDId,
    /// The target RepD (after the cast).
    pub target_repd: RepDId,
    /// Size check: target size ≤ remaining bytes in source region.
    pub size_ok: bool,
    /// Alignment check: resolved address satisfies target alignment.
    pub alignment_ok: bool,
    /// Semantic reinterpretation check: the cast is semantically valid.
    pub reinterpretation_ok: bool,
    /// The formal proof object.
    pub proof: Proof,
}

impl ReinterpretationSafetyProof {
    /// Construct a reinterpretation safety proof.
    pub fn new(
        derivation_id: DerivationId,
        source_repd: RepDId,
        target_repd: RepDId,
        size_ok: bool,
        alignment_ok: bool,
        reinterpretation_ok: bool,
        proof: Proof,
    ) -> Self {
        Self {
            derivation_id,
            source_repd,
            target_repd,
            size_ok,
            alignment_ok,
            reinterpretation_ok,
            proof,
        }
    }

    /// Returns true if all safety checks pass.
    pub fn is_safe(&self) -> bool {
        self.size_ok && self.alignment_ok && self.reinterpretation_ok
    }
}

// ---------------------------------------------------------------------------
// InterpretationProof
// ---------------------------------------------------------------------------

/// Top-level proof that the interpretation invariant holds for an entire MSG.
///
/// This proof aggregates:
/// - Individual [`BDCompatibilityProof`]s for every write-read pair.
/// - Individual [`ReinterpretationSafetyProof`]s for every cast derivation.
/// - A formal proof object tying everything together.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InterpretationProof {
    /// BD compatibility proofs for each write-read pair.
    pub bd_compatibility_proofs: Vec<BDCompatibilityProof>,
    /// Reinterpretation safety proofs for each cast derivation.
    pub reinterpretation_safety_proofs: Vec<ReinterpretationSafetyProof>,
    /// The top-level formal proof object.
    pub proof: Proof,
}

impl InterpretationProof {
    /// Returns true if all sub-proofs succeed.
    pub fn is_valid(&self) -> bool {
        self.bd_compatibility_proofs.iter().all(|p| p.is_compatible())
            && self
                .reinterpretation_safety_proofs
                .iter()
                .all(|p| p.is_safe())
    }
}

// ---------------------------------------------------------------------------
// Interpretation Tactics
// ---------------------------------------------------------------------------

/// Tactics specific to interpretation invariant proofs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum InterpTactic {
    /// **BD-tracing**: Walk derivation chains to compute the effective RepD/BD
    /// for each access. This tactic resolves `repd_of(d)` for every derivation
    /// `d` referenced by an access, and produces facts of the form
    /// "derivation D has effective RepD R".
    BDTracing,

    /// **Compatibility-checking**: For each write-read pair that targets
    /// overlapping bytes in the same region, verify that the read's expected
    /// BD is compatible with the write's BD. Produces facts of the form
    /// "BD of write W is compatible with BD of read R".
    CompatibilityChecking,

    /// **Size-alignment-verification**: For each cast derivation, verify:
    /// (a) target RepD size ≤ remaining bytes in source region,
    /// (b) resolved address satisfies target RepD alignment.
    /// Produces facts of the form "cast at derivation D satisfies size/alignment".
    SizeAlignmentVerification,
}

impl InterpTactic {
    /// Return the human-readable name of this tactic.
    pub fn name(&self) -> &'static str {
        match self {
            InterpTactic::BDTracing => "BDTracing",
            InterpTactic::CompatibilityChecking => "CompatibilityChecking",
            InterpTactic::SizeAlignmentVerification => "SizeAlignmentVerification",
        }
    }
}

impl std::fmt::Display for InterpTactic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

// ---------------------------------------------------------------------------
// Prover: prove_interpretation
// ---------------------------------------------------------------------------

/// Prove the interpretation invariant for the given MSG.
///
/// This function:
/// 1. Traces BDs through derivation chains (BDTracing tactic).
/// 2. Checks compatibility for every write-read pair (CompatibilityChecking tactic).
/// 3. Verifies size/alignment for every cast derivation (SizeAlignmentVerification tactic).
///
/// Returns `Ok(InterpretationProof)` if all checks pass, or `Err(ProofFailure)`
/// with a diagnostic on the first failure.
pub fn prove_interpretation(msg: &MSG) -> Result<InterpretationProof, ProofFailure> {
    let mut fact_id: FactId = 0;
    let mut next_fact_id = || -> FactId {
        let id = fact_id;
        fact_id += 1;
        id
    };

    // -- Top-level proof structure --
    let goal = Goal::new(
        "interpretation",
        Target::FullProgram,
        ProofContext::new("interpretation_prover"),
    );
    let mut top_proof = Proof::new(goal);

    // =======================================================================
    // Tactic 1: BD-tracing
    // =======================================================================
    // For each access, resolve the effective RepD of its target derivation
    // and record it as a fact.
    let mut access_repd_map: std::collections::HashMap<AccessId, RepDId> =
        std::collections::HashMap::new();

    for access in &msg.accesses {
        let effective_repd = msg
            .repd_of(access.target_derivation)
            .ok_or_else(|| ProofFailure::UnresolvableDerivation {
                derivation_id: access.target_derivation,
                reason: format!(
                    "cannot resolve effective RepD for derivation {} referenced by access {}",
                    access.target_derivation, access.id
                ),
            })?;

        let fid = next_fact_id();
        let fact = Fact::checked(
            fid,
            format!(
                "access {} has effective RepD {} (via derivation {})",
                access.id, effective_repd, access.target_derivation
            ),
        );
        top_proof.add_step(ProofStep::Assume { fact });
        access_repd_map.insert(access.id, effective_repd);
    }

    // =======================================================================
    // Tactic 2: Compatibility-checking
    // =======================================================================
    // For every (write, read) pair where both target the same region and
    // overlapping bytes, verify BD compatibility.
    let mut bd_proofs: Vec<BDCompatibilityProof> = Vec::new();

    let writes: Vec<&Access> = msg.accesses.iter().filter(|a| a.is_write()).collect();
    let reads: Vec<&Access> = msg.accesses.iter().filter(|a| a.is_read()).collect();

    for write_access in &writes {
        let write_region = msg
            .region_of(write_access.target_derivation)
            .ok_or_else(|| ProofFailure::UnresolvableDerivation {
                derivation_id: write_access.target_derivation,
                reason: format!(
                    "cannot resolve region for write access {}",
                    write_access.id
                ),
            })?;

        let write_addr = msg
            .addr_of(write_access.target_derivation)
            .ok_or_else(|| ProofFailure::UnresolvableDerivation {
                derivation_id: write_access.target_derivation,
                reason: format!(
                    "cannot resolve address for write access {}",
                    write_access.id
                ),
            })?;

        let write_end = write_addr + write_access.size;

        for read_access in &reads {
            let read_region = match msg.region_of(read_access.target_derivation) {
                Some(r) => r,
                None => continue, // skip unresolvable reads; they'll be caught above
            };

            if read_region != write_region {
                continue; // different regions, no BD compatibility needed
            }

            let read_addr = match msg.addr_of(read_access.target_derivation) {
                Some(a) => a,
                None => continue,
            };
            let read_end = read_addr + read_access.size;

            // Check byte-range overlap.
            if read_addr >= write_end || write_addr >= read_end {
                continue; // no overlap
            }

            // Both accesses overlap in the same region — check BD compatibility.
            let write_repd_id = access_repd_map
                .get(&write_access.id)
                .copied()
                .ok_or_else(|| ProofFailure::Internal(format!(
                    "BD-tracing did not produce a RepD for write access {}",
                    write_access.id
                )))?;

            let read_repd_id = access_repd_map
                .get(&read_access.id)
                .copied()
                .ok_or_else(|| ProofFailure::Internal(format!(
                    "BD-tracing did not produce a RepD for read access {}",
                    read_access.id
                )))?;

            let write_repd = msg
                .get_repd(write_repd_id)
                .ok_or_else(|| ProofFailure::Internal(format!(
                    "RepD {} not found in MSG",
                    write_repd_id
                )))?;

            let read_repd = msg
                .get_repd(read_repd_id)
                .ok_or_else(|| ProofFailure::Internal(format!(
                    "RepD {} not found in MSG",
                    read_repd_id
                )))?;

            let compat = write_repd.compatible_with(read_repd, read_addr);

            // Build a sub-proof for this pair.
            let compat_goal = Goal::new(
                "bd_compatibility",
                Target::Access(read_access.id),
                ProofContext::new(format!(
                    "compatibility_check::write{}_read{}",
                    write_access.id, read_access.id
                )),
            );
            let mut compat_proof = Proof::new(compat_goal);

            let f1 = Fact::axiom(next_fact_id(), format!(
                "write access {} has RepD {} (kind={})",
                write_access.id, write_repd.id, write_repd.kind
            ));
            let f1_id = f1.id;
            compat_proof.add_step(ProofStep::Assume { fact: f1 });

            let f2 = Fact::axiom(next_fact_id(), format!(
                "read access {} expects RepD {} (kind={})",
                read_access.id, read_repd.id, read_repd.kind
            ));
            let f2_id = f2.id;
            compat_proof.add_step(ProofStep::Assume { fact: f2 });

            if compat.is_compatible() {
                let f3 = Fact::derived(next_fact_id(), format!(
                    "BD of write {} is compatible with BD of read {}",
                    write_access.id, read_access.id
                ));
                compat_proof.add_step(ProofStep::Infer {
                    from: vec![f1_id, f2_id],
                    rule: InferenceRule::CastValidity,
                    conclusion: f3,
                });
                compat_proof.conclude(Conclusion::Proven);
            } else {
                return Err(ProofFailure::IncompatibleBD {
                    access_id: read_access.id,
                    reason: format!(
                        "write RepD {} incompatible with read RepD {}: {:?}",
                        write_repd.id,
                        read_repd.id,
                        compat
                    ),
                });
            }

            bd_proofs.push(BDCompatibilityProof::new(
                write_access.id,
                read_access.id,
                write_repd_id,
                read_repd_id,
                read_addr,
                compat,
                compat_proof,
            ));
        }
    }

    // Add a fact summarizing the compatibility check.
    let compat_fact = Fact::derived(
        next_fact_id(),
        format!(
            "all {} write-read pairs have compatible BDs",
            bd_proofs.len()
        ),
    );
    top_proof.add_step(ProofStep::Infer {
        from: vec![],
        rule: InferenceRule::CastValidity,
        conclusion: compat_fact,
    });

    // =======================================================================
    // Tactic 3: Size-alignment-verification
    // =======================================================================
    // For each cast derivation, verify size ≤ remaining bytes and alignment.
    let mut reinterpret_proofs: Vec<ReinterpretationSafetyProof> = Vec::new();

    for derivation in &msg.derivations {
        if !derivation.is_cast() {
            continue;
        }

        let target_repd_id = derivation
            .cast
            .ok_or_else(|| ProofFailure::Internal(format!(
                "cast derivation {} has no target RepD",
                derivation.id
            )))?;

        // Resolve the source RepD (the RepD of the source derivation).
        let source_repd_id = if let Some(src_did) = derivation.source_derivation {
            msg.repd_of(src_did).ok_or_else(|| {
                ProofFailure::UnresolvableDerivation {
                    derivation_id: src_did,
                    reason: format!(
                        "cannot resolve source RepD for cast derivation {}",
                        derivation.id
                    ),
                }
            })?
        } else if let Some(src_rid) = derivation.source_region {
            let region = msg.get_region(src_rid).ok_or_else(|| {
                ProofFailure::UnresolvableDerivation {
                    derivation_id: derivation.id,
                    reason: format!("source region {} not found", src_rid),
                }
            })?;
            region.default_repd
        } else {
            return Err(ProofFailure::UnresolvableDerivation {
                derivation_id: derivation.id,
                reason: "cast derivation has no source".into(),
            });
        };

        let source_repd = msg.get_repd(source_repd_id).ok_or_else(|| {
            ProofFailure::Internal(format!("source RepD {} not found", source_repd_id))
        })?;
        let target_repd = msg.get_repd(target_repd_id).ok_or_else(|| {
            ProofFailure::Internal(format!("target RepD {} not found", target_repd_id))
        })?;

        // Size check: target size ≤ region remaining bytes.
        let region_id = msg.region_of(derivation.id).ok_or_else(|| {
            ProofFailure::UnresolvableDerivation {
                derivation_id: derivation.id,
                reason: "cannot resolve root region for cast derivation".into(),
            }
        })?;
        let region = msg.get_region(region_id).ok_or_else(|| {
            ProofFailure::UnresolvableDerivation {
                derivation_id: derivation.id,
                reason: format!("root region {} not found", region_id),
            }
        })?;

        let resolved_addr = msg.addr_of(derivation.id).ok_or_else(|| {
            ProofFailure::UnresolvableDerivation {
                derivation_id: derivation.id,
                reason: "cannot resolve address for cast derivation".into(),
            }
        })?;

        let remaining_bytes = region
            .base_addr
            .saturating_add(region.size)
            .saturating_sub(resolved_addr);
        let size_ok = target_repd.size <= remaining_bytes;

        // Alignment check.
        let alignment_ok = if target_repd.alignment > 0 {
            resolved_addr % target_repd.alignment == 0
        } else {
            true // alignment 0 means no constraint
        };

        // Semantic reinterpretation check.
        let reinterpretation_ok = valid_reinterpretation(source_repd, target_repd);

        // Build sub-proof for this cast.
        let cast_goal = Goal::new(
            "reinterpretation_safety",
            Target::Derivation(derivation.id),
            ProofContext::new(format!("cast_verification::d{}", derivation.id)),
        );
        let mut cast_proof = Proof::new(cast_goal);

        let sf = Fact::axiom(next_fact_id(), format!(
            "source type RepD {} has layout size={}, alignment={}",
            source_repd.id, source_repd.size, source_repd.alignment
        ));
        let sf_id = sf.id;
        cast_proof.add_step(ProofStep::Assume { fact: sf });

        let tf = Fact::axiom(next_fact_id(), format!(
            "target type RepD {} has layout size={}, alignment={}",
            target_repd.id, target_repd.size, target_repd.alignment
        ));
        let tf_id = tf.id;
        cast_proof.add_step(ProofStep::Assume { fact: tf });

        if size_ok && alignment_ok && reinterpretation_ok {
            let cf = Fact::derived(next_fact_id(), format!(
                "cast at derivation {} is valid: size_ok={}, alignment_ok={}, reinterpretation_ok={}",
                derivation.id, size_ok, alignment_ok, reinterpretation_ok
            ));
            cast_proof.add_step(ProofStep::Infer {
                from: vec![sf_id, tf_id],
                rule: InferenceRule::CastValidity,
                conclusion: cf,
            });
            cast_proof.conclude(Conclusion::Proven);
        } else {
            if !reinterpretation_ok {
                return Err(ProofFailure::UnsafeReinterpretation {
                    derivation_id: derivation.id,
                    reason: format!(
                        "invalid reinterpretation: {} -> {}",
                        source_repd.kind, target_repd.kind
                    ),
                });
            }
            if !size_ok {
                return Err(ProofFailure::SizeAlignmentViolation {
                    derivation_id: derivation.id,
                    reason: format!(
                        "target size {} exceeds remaining bytes {}",
                        target_repd.size, remaining_bytes
                    ),
                });
            }
            if !alignment_ok {
                return Err(ProofFailure::SizeAlignmentViolation {
                    derivation_id: derivation.id,
                    reason: format!(
                        "address 0x{:x} not aligned to {} bytes",
                        resolved_addr, target_repd.alignment
                    ),
                });
            }
            // Should be unreachable, but just in case.
            return Err(ProofFailure::UnsafeReinterpretation {
                derivation_id: derivation.id,
                reason: "unknown cast safety failure".into(),
            });
        }

        reinterpret_proofs.push(ReinterpretationSafetyProof::new(
            derivation.id,
            source_repd_id,
            target_repd_id,
            size_ok,
            alignment_ok,
            reinterpretation_ok,
            cast_proof,
        ));
    }

    // Add a fact summarizing the size/alignment check.
    let sa_fact = Fact::derived(
        next_fact_id(),
        format!(
            "all {} cast derivations satisfy size/alignment/reinterpretation constraints",
            reinterpret_proofs.len()
        ),
    );
    top_proof.add_step(ProofStep::Infer {
        from: vec![],
        rule: InferenceRule::BoundsPreservation,
        conclusion: sa_fact,
    });

    // Conclude the top-level proof.
    top_proof.conclude(Conclusion::Proven);

    Ok(InterpretationProof {
        bd_compatibility_proofs: bd_proofs,
        reinterpretation_safety_proofs: reinterpret_proofs,
        proof: top_proof,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a minimal MSG with one region, one base derivation,
    /// and one read access. The RepD is bytes, all checks should pass.
    fn simple_msg() -> MSG {
        let repd_bytes = RepD::bytes(1, 64, true);
        let region = Region::new(10, 0x1000, 64, RegionStatus::Allocated, repd_bytes.id);
        let deriv = Derivation {
            id: 20,
            source_region: Some(region.id),
            source_derivation: None,
            offset: 0,
            cast: None,
        };
        let access = Access::new(30, deriv.id, AccessKind::Read, 8, 1, repd_bytes.id);

        MSG {
            regions: vec![region],
            derivations: vec![deriv],
            accesses: vec![access],
            repds: vec![repd_bytes],
        }
    }

    #[test]
    fn test_repd_compatible_same() {
        let write_repd = RepD::bytes(1, 8, true);
        let read_repd = RepD::bytes(2, 8, true);
        let result = write_repd.compatible_with(&read_repd, 0x1000);
        assert!(result.is_compatible());
    }

    #[test]
    fn test_repd_incompatible_size() {
        let write_repd = RepD::bytes(1, 4, true);
        let read_repd = RepD::integer(2, 8, 8, true);
        let result = write_repd.compatible_with(&read_repd, 0x1000);
        assert!(!result.is_compatible());
        if let Compatibility::Incompatible(reason) = result {
            assert!(reason.contains("size mismatch"));
        }
    }

    #[test]
    fn test_repd_incompatible_alignment() {
        let write_repd = RepD::integer(1, 8, 8, true);
        let read_repd = RepD::integer(2, 8, 8, true);
        // Address 0x1001 is not 8-byte aligned.
        let result = write_repd.compatible_with(&read_repd, 0x1001);
        assert!(!result.is_compatible());
        if let Compatibility::Incompatible(reason) = result {
            assert!(reason.contains("alignment"));
        }
    }

    #[test]
    fn test_repd_uninitialized_pointer_read() {
        let write_repd = RepD::bytes(1, 8, false); // uninitialized
        let read_repd = RepD::pointer(2, 8, 8, true);
        let result = write_repd.compatible_with(&read_repd, 0x1000);
        assert!(!result.is_compatible());
        if let Compatibility::Incompatible(reason) = result {
            assert!(reason.contains("uninitialized") && reason.contains("pointer"));
        }
    }

    #[test]
    fn test_prove_interpretation_simple_pass() {
        let msg = simple_msg();
        let result = prove_interpretation(&msg);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert!(proof.is_valid());
        assert!(proof.proof.conclusion == Conclusion::Proven);
    }

    #[test]
    fn test_prove_interpretation_with_write_read_pair() {
        let repd_bytes = RepD::bytes(1, 64, true);
        let region = Region::new(10, 0x1000, 64, RegionStatus::Allocated, repd_bytes.id);
        let deriv = Derivation {
            id: 20,
            source_region: Some(region.id),
            source_derivation: None,
            offset: 0,
            cast: None,
        };
        let write_access = Access::new(30, deriv.id, AccessKind::Write, 8, 1, repd_bytes.id);
        let read_access = Access::new(31, deriv.id, AccessKind::Read, 8, 2, repd_bytes.id);

        let msg = MSG {
            regions: vec![region],
            derivations: vec![deriv],
            accesses: vec![write_access, read_access],
            repds: vec![repd_bytes],
        };

        let result = prove_interpretation(&msg);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert!(proof.is_valid());
        // Should have one BD compatibility proof (one write-read pair).
        assert_eq!(proof.bd_compatibility_proofs.len(), 1);
    }

    #[test]
    fn test_prove_interpretation_cast_pass() {
        let repd_bytes = RepD::bytes(1, 64, true);
        let repd_header = RepD::new(2, BDKind::Struct, 8, 4, true);
        let region = Region::new(10, 0x1000, 64, RegionStatus::Allocated, repd_bytes.id);

        // Base derivation from region.
        let base_deriv = Derivation {
            id: 20,
            source_region: Some(region.id),
            source_derivation: None,
            offset: 0,
            cast: None,
        };
        // Cast derivation: bytes → Header struct.
        let cast_deriv = Derivation {
            id: 21,
            source_region: None,
            source_derivation: Some(base_deriv.id),
            offset: 0,
            cast: Some(repd_header.id),
        };

        let access = Access::new(30, cast_deriv.id, AccessKind::Read, 8, 1, repd_header.id);

        let msg = MSG {
            regions: vec![region],
            derivations: vec![base_deriv, cast_deriv],
            accesses: vec![access],
            repds: vec![repd_bytes.clone(), repd_header.clone()],
        };

        let result = prove_interpretation(&msg);
        assert!(result.is_ok());
        let proof = result.unwrap();
        assert!(proof.is_valid());
        assert_eq!(proof.reinterpretation_safety_proofs.len(), 1);
        assert!(proof.reinterpretation_safety_proofs[0].is_safe());
    }

    #[test]
    fn test_prove_interpretation_cast_size_fail() {
        let repd_bytes = RepD::bytes(1, 4, true); // only 4 bytes
        let repd_large = RepD::new(2, BDKind::Struct, 128, 1, true); // needs 128 bytes
        let region = Region::new(10, 0x1000, 4, RegionStatus::Allocated, repd_bytes.id);

        let base_deriv = Derivation {
            id: 20,
            source_region: Some(region.id),
            source_derivation: None,
            offset: 0,
            cast: None,
        };
        let cast_deriv = Derivation {
            id: 21,
            source_region: None,
            source_derivation: Some(base_deriv.id),
            offset: 0,
            cast: Some(repd_large.id),
        };

        let msg = MSG {
            regions: vec![region],
            derivations: vec![base_deriv, cast_deriv],
            accesses: vec![],
            repds: vec![repd_bytes, repd_large],
        };

        let result = prove_interpretation(&msg);
        assert!(result.is_err());
        if let Err(ProofFailure::SizeAlignmentViolation { reason, .. }) = result {
            assert!(reason.contains("exceeds remaining bytes"));
        } else {
            panic!("expected SizeAlignmentViolation, got {:?}", result);
        }
    }

    #[test]
    fn test_prove_interpretation_pointer_to_float_fail() {
        let repd_ptr = RepD::pointer(1, 8, 8, true);
        let repd_float = RepD::new(2, BDKind::Float, 8, 8, true);
        // Pointer → Float is invalid (pointer to non-pointer, non-bytes).
        assert!(!valid_reinterpretation(&repd_ptr, &repd_float));
    }

    #[test]
    fn test_prove_interpretation_uninitialized_pointer_read_fail() {
        let repd_bytes_uninit = RepD::bytes(1, 8, false);
        let repd_ptr = RepD::pointer(2, 8, 8, true);
        let region = Region::new(10, 0x1000, 64, RegionStatus::Allocated, repd_bytes_uninit.id);

        let base_deriv = Derivation {
            id: 20,
            source_region: Some(region.id),
            source_derivation: None,
            offset: 0,
            cast: None,
        };
        let cast_deriv = Derivation {
            id: 21,
            source_region: None,
            source_derivation: Some(base_deriv.id),
            offset: 0,
            cast: Some(repd_ptr.id),
        };

        let write_access = Access::new(30, base_deriv.id, AccessKind::Write, 8, 1, repd_bytes_uninit.id);
        let read_access = Access::new(31, cast_deriv.id, AccessKind::Read, 8, 2, repd_ptr.id);

        let msg = MSG {
            regions: vec![region],
            derivations: vec![base_deriv, cast_deriv],
            accesses: vec![write_access, read_access],
            repds: vec![repd_bytes_uninit, repd_ptr],
        };

        let result = prove_interpretation(&msg);
        assert!(result.is_err());
        match result.unwrap_err() {
            ProofFailure::IncompatibleBD { reason, .. } => {
                assert!(reason.contains("uninitialized") || reason.contains("pointer"));
            }
            other => panic!("expected IncompatibleBD, got {:?}", other),
        }
    }

    #[test]
    fn test_repd_sub_repd_bytes_supertype() {
        let repd_int = RepD::integer(1, 4, 4, true);
        let repd_bytes = RepD::bytes(2, 4, true);
        // Integer ⊑ Bytes (bytes is universal supertype).
        assert!(repd_int.is_sub_repd_of(&repd_bytes));
    }

    #[test]
    fn test_repd_sub_repd_same_kind() {
        let repd_int32 = RepD::integer(1, 4, 4, true);
        let repd_int64 = RepD::integer(2, 8, 8, true);
        // int32 ⊑ int64 (smaller size, same kind).
        assert!(repd_int32.is_sub_repd_of(&repd_int64));
    }

    #[test]
    fn test_msg_region_of_and_addr() {
        let repd = RepD::bytes(1, 64, true);
        let region = Region::new(10, 0x1000, 64, RegionStatus::Allocated, repd.id);
        let d1 = Derivation {
            id: 20,
            source_region: Some(region.id),
            source_derivation: None,
            offset: 32,
            cast: None,
        };
        let d2 = Derivation {
            id: 21,
            source_region: None,
            source_derivation: Some(d1.id),
            offset: 16,
            cast: None,
        };

        let msg = MSG {
            regions: vec![region],
            derivations: vec![d1, d2],
            accesses: vec![],
            repds: vec![repd],
        };

        assert_eq!(msg.region_of(20), Some(10));
        assert_eq!(msg.region_of(21), Some(10));
        assert_eq!(msg.addr_of(20), Some(0x1020)); // 0x1000 + 32
        assert_eq!(msg.addr_of(21), Some(0x1030)); // 0x1000 + 32 + 16
    }

    #[test]
    fn test_msg_repd_of_with_cast() {
        let repd_bytes = RepD::bytes(1, 64, true);
        let repd_header = RepD::new(2, BDKind::Struct, 8, 4, true);
        let region = Region::new(10, 0x1000, 64, RegionStatus::Allocated, repd_bytes.id);

        let base = Derivation {
            id: 20,
            source_region: Some(region.id),
            source_derivation: None,
            offset: 0,
            cast: None,
        };
        let cast_d = Derivation {
            id: 21,
            source_region: None,
            source_derivation: Some(base.id),
            offset: 0,
            cast: Some(repd_header.id),
        };

        let msg = MSG {
            regions: vec![region],
            derivations: vec![base, cast_d],
            accesses: vec![],
            repds: vec![repd_bytes.clone(), repd_header.clone()],
        };

        // Base derivation should have the region's default RepD.
        assert_eq!(msg.repd_of(20), Some(repd_bytes.id));
        // Cast derivation should have the cast's RepD.
        assert_eq!(msg.repd_of(21), Some(repd_header.id));
    }

    #[test]
    fn test_interp_tactic_display() {
        assert_eq!(format!("{}", InterpTactic::BDTracing), "BDTracing");
        assert_eq!(
            format!("{}", InterpTactic::CompatibilityChecking),
            "CompatibilityChecking"
        );
        assert_eq!(
            format!("{}", InterpTactic::SizeAlignmentVerification),
            "SizeAlignmentVerification"
        );
    }

    #[test]
    fn test_bd_kind_display() {
        assert_eq!(format!("{}", BDKind::Bytes), "bytes");
        assert_eq!(format!("{}", BDKind::Pointer), "pointer");
        assert_eq!(format!("{}", BDKind::Integer), "integer");
    }

    #[test]
    fn test_compatibility_result() {
        assert!(Compatibility::Compatible.is_compatible());
        assert!(!Compatibility::Incompatible("test".into()).is_compatible());
    }

    #[test]
    fn test_reinterpretation_safety_proof_checks() {
        let proof = ReinterpretationSafetyProof::new(
            42, 1, 2, true, true, true, Proof::new(Goal::new(
                "reinterpretation_safety",
                Target::Derivation(42),
                ProofContext::new("test"),
            )),
        );
        assert!(proof.is_safe());

        let proof_bad = ReinterpretationSafetyProof::new(
            42, 1, 2, true, false, true, Proof::new(Goal::new(
                "reinterpretation_safety",
                Target::Derivation(42),
                ProofContext::new("test"),
            )),
        );
        assert!(!proof_bad.is_safe());
    }

    #[test]
    fn test_region_range() {
        let region = Region::new(1, 0x1000, 64, RegionStatus::Allocated, 0);
        assert_eq!(region.range(), 0x1000..0x1040);
    }

    #[test]
    fn test_access_convenience_methods() {
        let read = Access::new(1, 10, AccessKind::Read, 4, 1, 0);
        let write = Access::new(2, 10, AccessKind::Write, 4, 2, 0);
        assert!(read.is_read());
        assert!(!read.is_write());
        assert!(write.is_write());
        assert!(!write.is_read());
    }

    #[test]
    fn test_derivation_convenience_methods() {
        let base = Derivation::base(1);
        assert!(base.is_base());
        assert!(!base.is_cast());

        let offset_d = Derivation::offset_from_region(1, 32);
        assert!(!offset_d.is_base());
        assert!(!offset_d.is_cast());

        let cast_d = Derivation::cast_from(1, 99);
        assert!(cast_d.is_cast());
        assert!(!cast_d.is_base());
    }
}
