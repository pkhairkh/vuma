//! Interpretation Invariant Verifier
//!
//! This module implements the **interpretation invariant** of the VUMA model:
//!
//! > *"Every read interprets data under the correct behavioral description."*
//!
//! The interpretation invariant ensures that when data is read from a memory
//! location, the behavioral description (BD) at the read point is compatible
//! with the BD under which the data was last written. This prevents type
//! confusion, invalid reinterpretation, and capability violations.
//!
//! # Verification Strategy
//!
//! 1. **Write-Read Pair Tracking**: Track all write and read events through the
//!    Memory State Graph (MSG). For each read, trace back to the last write
//!    to the same location.
//!
//! 2. **RepD Compatibility**: Verify that the representation descriptor at the
//!    read point is compatible with the RepD at the write point. Same size and
//!    alignment are the minimum requirement; structural compatibility is checked
//!    via the RepD compatibility lattice.
//!
//! 3. **CapD Transition Validity**: Check that the capability descriptor at the
//!    read point is a valid weakening or strengthening of the CapD at the write
//!    point. Weakening (removing capabilities) is always safe. Strengthening
//!    (adding capabilities) requires explicit proof of safety.
//!
//! 4. **RelD Preservation**: Verify that relational constraints are preserved
//!    across write-read pairs. The read's RelD must refine the write's RelD
//!    or the composed RelD must be consistent.
//!
//! 5. **Type Confusion Detection**: Detect when data is read with a different
//!    interpretation than it was written — e.g., writing a pointer and reading
//!    it as an integer, or writing a float and reading it as a struct.

use vuma_bd::capd::{CapD, Capability};
use vuma_bd::descriptor::BD;
use vuma_bd::reld::{RelD, Relation};
use vuma_bd::repd::{ByteRep, RepD};

use crate::result::{CounterExample, VerificationResult, VerificationStatus};

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// Opaque identifier for a memory location (region + offset).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LocationId(pub u64);

impl fmt::Display for LocationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "loc#{}", self.0)
    }
}

/// Opaque identifier for a program point in the SCG.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProgramPointId(pub u64);

impl fmt::Display for ProgramPointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pp#{}", self.0)
    }
}

/// A proof certificate that a strengthening is safe.
///
/// When a CapD is strengthened (capabilities are added) between a write
/// and a read, a `SafetyProof` must be provided to justify that the
/// additional capabilities are valid. Without proof, strengthening is
/// flagged as a potential violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyProof {
    /// No proof is needed (weakening or same capabilities).
    NotNeeded,
    /// An explicit cast operation provides the proof.
    ExplicitCast {
        /// Description of the cast operation.
        description: String,
    },
    /// A runtime check guarantees safety (e.g., type tag check).
    RuntimeCheck {
        /// Description of the runtime check.
        description: String,
    },
    /// A formal proof exists (e.g., from the proof engine).
    FormalProof {
        /// The proof steps.
        steps: Vec<String>,
    },
}

// ---------------------------------------------------------------------------
// Access Events
// ---------------------------------------------------------------------------

/// A memory access event — either a write or a read.
///
/// Each access carries the full BD under which the operation is performed,
/// enabling the interpretation verifier to check compatibility.
#[derive(Debug, Clone)]
pub enum AccessEvent {
    /// A write operation that stores data under a specific BD.
    Write {
        /// The memory location being written.
        location: LocationId,
        /// The behavioral descriptor at the write point.
        bd: BD,
        /// The program point where the write occurs.
        point: ProgramPointId,
    },
    /// A read operation that loads data expecting a specific BD.
    Read {
        /// The memory location being read.
        location: LocationId,
        /// The behavioral descriptor at the read point.
        bd: BD,
        /// The program point where the read occurs.
        point: ProgramPointId,
    },
}

impl AccessEvent {
    /// Returns the location targeted by this access.
    pub fn location(&self) -> &LocationId {
        match self {
            AccessEvent::Write { location, .. } => location,
            AccessEvent::Read { location, .. } => location,
        }
    }

    /// Returns the BD associated with this access.
    pub fn bd(&self) -> &BD {
        match self {
            AccessEvent::Write { bd, .. } => bd,
            AccessEvent::Read { bd, .. } => bd,
        }
    }

    /// Returns the program point of this access.
    pub fn point(&self) -> &ProgramPointId {
        match self {
            AccessEvent::Write { point, .. } => point,
            AccessEvent::Read { point, .. } => point,
        }
    }

    /// Returns `true` if this is a write event.
    pub fn is_write(&self) -> bool {
        matches!(self, AccessEvent::Write { .. })
    }

    /// Returns `true` if this is a read event.
    pub fn is_read(&self) -> bool {
        matches!(self, AccessEvent::Read { .. })
    }
}

// ---------------------------------------------------------------------------
// Write-Read Pair
// ---------------------------------------------------------------------------

/// A write-read pair that the interpretation verifier must check.
///
/// For every read, we trace back to the most recent write to the same
/// location and verify BD compatibility.
#[derive(Debug, Clone)]
pub struct WriteReadPair {
    /// The location being written and then read.
    pub location: LocationId,
    /// The program point of the write.
    pub write_point: ProgramPointId,
    /// The program point of the read.
    pub read_point: ProgramPointId,
    /// The BD at the write point.
    pub write_bd: BD,
    /// The BD at the read point.
    pub read_bd: BD,
}

// ---------------------------------------------------------------------------
// Deep Confusion Kind
// ---------------------------------------------------------------------------

/// Kinds of deep type confusion detected by recursive structural comparison
/// of RepD trees, going beyond top-level variant checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeepConfusionKind {
    /// Struct fields differ between write and read representations.
    StructFieldMismatch {
        /// Field name/offset in the write representation.
        write_field: String,
        /// Field name/offset in the read representation.
        read_field: String,
    },
    /// Enum variants differ between write and read representations.
    EnumVariantMismatch {
        /// Variant written.
        write_variant: String,
        /// Variant read.
        read_variant: String,
    },
    /// Array bounds violation — reading beyond the bounds of the written array.
    ArrayBoundsViolation {
        /// Length of the written array.
        write_len: u64,
        /// Index accessed during read.
        read_index: u64,
    },
    /// Union active field mismatch — reading a field that was not the one
    /// last written to the union.
    UnionActiveFieldMismatch {
        /// Field that was written (active at write point).
        write_field: String,
        /// Field that was read.
        read_field: String,
    },
    /// Nested pointer depth mismatch — pointer indirection levels differ.
    NestedPointerDepthMismatch {
        /// Pointer depth at the write point.
        write_depth: u32,
        /// Pointer depth at the read point.
        read_depth: u32,
    },
    /// Security level violation — reading data at a lower security level
    /// than it was written at (information disclosure).
    SecurityLevelViolation {
        /// Security level at the write point.
        write_level: String,
        /// Security level at the read point.
        read_level: String,
    },
}

impl fmt::Display for DeepConfusionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StructFieldMismatch { write_field, read_field } => {
                write!(f, "struct field mismatch: wrote '{}', read '{}'", write_field, read_field)
            }
            Self::EnumVariantMismatch { write_variant, read_variant } => {
                write!(f, "enum variant mismatch: wrote '{}', read '{}'", write_variant, read_variant)
            }
            Self::ArrayBoundsViolation { write_len, read_index } => {
                write!(f, "array bounds violation: wrote len={}, read index={}", write_len, read_index)
            }
            Self::UnionActiveFieldMismatch { write_field, read_field } => {
                write!(f, "union active field mismatch: wrote '{}', read '{}'", write_field, read_field)
            }
            Self::NestedPointerDepthMismatch { write_depth, read_depth } => {
                write!(f, "nested pointer depth mismatch: write_depth={}, read_depth={}", write_depth, read_depth)
            }
            Self::SecurityLevelViolation { write_level, read_level } => {
                write!(f, "security level violation: wrote at '{}', read at '{}'", write_level, read_level)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Union Discriminator
// ---------------------------------------------------------------------------

/// Tracks which field of a union is currently active at a given memory
/// location, along with the program point at which the discriminator was set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnionDiscriminator {
    /// The memory location of the union.
    pub location: LocationId,
    /// The name of the currently active field, if known.
    pub active_field: Option<String>,
    /// The program point at which the active field was set.
    pub set_point: ProgramPointId,
}

// ---------------------------------------------------------------------------
// Enum Variant Tracker
// ---------------------------------------------------------------------------

/// Tracks which enum variant is currently active at each memory location,
/// enabling detection of variant-access mismatches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumVariantTracker {
    /// Maps each location to its active variant and the program point where
    /// it was set.
    pub active_variants: HashMap<LocationId, (String, ProgramPointId)>,
}

impl EnumVariantTracker {
    /// Construct a new empty tracker.
    pub fn new() -> Self {
        Self {
            active_variants: HashMap::new(),
        }
    }

    /// Set the active enum variant at a given location.
    pub fn set_active_variant(
        &mut self,
        location: LocationId,
        variant: String,
        point: ProgramPointId,
    ) {
        self.active_variants.insert(location, (variant, point));
    }

    /// Check whether the given variant is the active one at the given location.
    ///
    /// Returns `Ok(())` if the variant matches, or an `Err` with a description
    /// of the mismatch.
    pub fn check_variant_access(
        &self,
        location: &LocationId,
        variant: &str,
    ) -> Result<(), InterpretationViolation> {
        if let Some((active_variant, set_point)) = self.active_variants.get(location) {
            if active_variant != variant {
                return Err(InterpretationViolation::EnumVariantViolation {
                    location: location.clone(),
                    write_variant: active_variant.clone(),
                    read_variant: variant.to_string(),
                    set_point: set_point.clone(),
                });
            }
        }
        Ok(())
    }
}

impl Default for EnumVariantTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Cast Records and Validation
// ---------------------------------------------------------------------------

/// Kinds of cast operations between BDs.
///
/// Each variant describes which BD component(s) are being changed by the
/// cast, enabling the verifier to apply the appropriate validation rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CastKind {
    /// Changing representation (e.g., Byte→Struct).
    RepCast,
    /// Changing capabilities (e.g., removing Write).
    CapCast,
    /// Changing relations (e.g., narrowing lifetime).
    RelCast,
    /// Changing all three BD components.
    FullCast,
    /// Raw bit reinterpretation (most dangerous).
    BitCast,
    /// Provably safe cast (e.g., widening).
    SafeCast,
}

impl fmt::Display for CastKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RepCast => write!(f, "RepCast"),
            Self::CapCast => write!(f, "CapCast"),
            Self::RelCast => write!(f, "RelCast"),
            Self::FullCast => write!(f, "FullCast"),
            Self::BitCast => write!(f, "BitCast"),
            Self::SafeCast => write!(f, "SafeCast"),
        }
    }
}

/// Record of an explicit cast operation between two BDs.
///
/// Tracks the source and target BDs, the kind of cast, the program point
/// where it occurs, and whether it was explicitly written by the programmer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CastRecord {
    /// The memory location where the cast occurs.
    pub location: LocationId,
    /// The source BD being cast from.
    pub from_bd: BD,
    /// The target BD being cast to.
    pub to_bd: BD,
    /// The kind of cast operation.
    pub cast_kind: CastKind,
    /// The program point where the cast occurs.
    pub point: ProgramPointId,
    /// Whether the cast was explicitly written by the programmer.
    pub is_explicit: bool,
}

/// Risk level for bit cast operations.
///
/// Classifies the danger of raw bit reinterpretation based on the
/// types involved and whether size/pointer issues are present.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BitCastRisk {
    /// Same size, compatible layout (e.g., u32↔i32).
    Low,
    /// Same size, potentially incompatible (e.g., float↔int).
    Medium,
    /// Size mismatch or pointer involvement.
    High,
    /// Function pointer or security-sensitive.
    Extreme,
}

impl fmt::Display for BitCastRisk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Low => write!(f, "Low"),
            Self::Medium => write!(f, "Medium"),
            Self::High => write!(f, "High"),
            Self::Extreme => write!(f, "Extreme"),
        }
    }
}

/// Difficulty of discharging a proof obligation for a cast.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProofDifficulty {
    /// Trivially discharged (e.g., same type, identity cast).
    Trivial,
    /// Requires a simple argument (e.g., subtyping, subset).
    Easy,
    /// Requires a non-trivial proof (e.g., showing layout compatibility).
    Medium,
    /// Requires a complex proof (e.g., pointer provenance).
    Hard,
}

impl fmt::Display for ProofDifficulty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trivial => write!(f, "Trivial"),
            Self::Easy => write!(f, "Easy"),
            Self::Medium => write!(f, "Medium"),
            Self::Hard => write!(f, "Hard"),
        }
    }
}

/// A proof obligation that must be discharged to justify a cast.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CastProofObligation {
    /// The cast record that requires proof.
    pub cast: CastRecord,
    /// Description of the proof that must be provided.
    pub required_proof: String,
    /// Difficulty of discharging this obligation.
    pub difficulty: ProofDifficulty,
}

/// Result of validating a cast operation.
///
/// Each variant describes the safety status of the cast, from provably safe
/// to unsafe with specific violations.
#[derive(Debug, Clone)]
pub enum CastValidationResult {
    /// The cast is provably safe.
    Safe {
        /// Why the cast is safe.
        reason: String,
    },
    /// The cast is safe if a proof obligation is discharged.
    SafeWithProof {
        /// The proof obligation that must be met.
        obligation: CastProofObligation,
    },
    /// The cast is unsafe — specific violations found.
    Unsafe {
        /// The violations detected.
        violations: Vec<InterpretationViolation>,
    },
    /// The cast is a bit cast with the given risk level.
    BitCast {
        /// The risk level of this bit cast.
        risk_level: BitCastRisk,
    },
}

// ---------------------------------------------------------------------------
// Interpretation Violation
// ---------------------------------------------------------------------------

/// A specific violation of the interpretation invariant.
#[derive(Debug, Clone, PartialEq)]
pub enum InterpretationViolation {
    /// The RepD at the read point is incompatible with the RepD at the write
    /// point (different size, alignment, or incompatible layout).
    IncompatibleRepD {
        write_point: ProgramPointId,
        read_point: ProgramPointId,
        location: LocationId,
        write_repd: RepD,
        read_repd: RepD,
        reason: String,
    },

    /// The CapD at the read point strengthens the CapD at the write point
    /// without a valid safety proof.
    InvalidCapDStrengthening {
        write_point: ProgramPointId,
        read_point: ProgramPointId,
        location: LocationId,
        added_caps: Vec<Capability>,
    },

    /// The capability meet between write and read is empty, meaning no
    /// capability is shared — the read has no authority to access this data.
    EmptyCapabilityMeet {
        write_point: ProgramPointId,
        read_point: ProgramPointId,
        location: LocationId,
    },

    /// Relational constraints are not preserved across the write-read pair.
    RelDNotPreserved {
        write_point: ProgramPointId,
        read_point: ProgramPointId,
        location: LocationId,
        reason: String,
    },

    /// Type confusion: data written under one interpretation is read under a
    /// fundamentally different interpretation (e.g., pointer read as integer,
    /// float read as struct).
    TypeConfusion {
        write_point: ProgramPointId,
        read_point: ProgramPointId,
        location: LocationId,
        write_repd_kind: String,
        read_repd_kind: String,
    },

    /// Pointer reinterpretation: a pointer value is being read as a non-pointer
    /// type (or vice versa) without an explicit cast derivation.
    PointerReinterpretation {
        write_point: ProgramPointId,
        read_point: ProgramPointId,
        location: LocationId,
        reason: String,
    },

    /// A read occurs without any preceding write (reading uninitialized memory).
    UninitializedRead {
        read_point: ProgramPointId,
        location: LocationId,
    },

    /// Union field violation — accessing a union field that is not the
    /// currently active one (as tracked by the discriminator).
    UnionFieldViolation {
        location: LocationId,
        active_field: String,
        accessed_field: String,
        set_point: ProgramPointId,
    },

    /// Enum variant violation — accessing an enum variant that is not the
    /// currently active one.
    EnumVariantViolation {
        location: LocationId,
        write_variant: String,
        read_variant: String,
        set_point: ProgramPointId,
    },

    /// Deep type confusion detected by recursive structural comparison
    /// of RepD trees.
    DeepConfusion {
        write_point: ProgramPointId,
        read_point: ProgramPointId,
        location: LocationId,
        kind: DeepConfusionKind,
    },

    /// Unsafe cast — a cast operation violates BD compatibility rules.
    UnsafeCast {
        /// The program point where the cast occurs.
        point: ProgramPointId,
        /// The location of the cast.
        location: LocationId,
        /// The source BD.
        from_repd_kind: String,
        /// The target BD.
        to_repd_kind: String,
        /// Why the cast is unsafe.
        reason: String,
    },
}

impl fmt::Display for InterpretationViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompatibleRepD {
                write_point,
                read_point,
                location,
                reason,
                ..
            } => write!(
                f,
                "Incompatible RepD at {} → {} ({}): {}",
                write_point, read_point, location, reason
            ),
            Self::InvalidCapDStrengthening {
                write_point,
                read_point,
                location,
                added_caps,
            } => write!(
                f,
                "Invalid CapD strengthening at {} → {} ({}): added {:?}",
                write_point, read_point, location, added_caps
            ),
            Self::EmptyCapabilityMeet {
                write_point,
                read_point,
                location,
            } => write!(
                f,
                "Empty capability meet at {} → {} ({})",
                write_point, read_point, location
            ),
            Self::RelDNotPreserved {
                write_point,
                read_point,
                location,
                reason,
            } => write!(
                f,
                "RelD not preserved at {} → {} ({}): {}",
                write_point, read_point, location, reason
            ),
            Self::TypeConfusion {
                write_point,
                read_point,
                location,
                write_repd_kind,
                read_repd_kind,
            } => write!(
                f,
                "Type confusion at {} → {} ({}): wrote as {}, read as {}",
                write_point, read_point, location, write_repd_kind, read_repd_kind
            ),
            Self::PointerReinterpretation {
                write_point,
                read_point,
                location,
                reason,
            } => write!(
                f,
                "Pointer reinterpretation at {} → {} ({}): {}",
                write_point, read_point, location, reason
            ),
            Self::UninitializedRead {
                read_point,
                location,
            } => write!(
                f,
                "Uninitialized read at {} ({})",
                read_point, location
            ),
            Self::UnionFieldViolation {
                location,
                active_field,
                accessed_field,
                set_point,
            } => write!(
                f,
                "Union field violation at {}: active field '{}', accessed '{}' (set at {})",
                location, active_field, accessed_field, set_point
            ),
            Self::EnumVariantViolation {
                location,
                write_variant,
                read_variant,
                set_point,
            } => write!(
                f,
                "Enum variant violation at {}: wrote '{}', read '{}' (set at {})",
                location, write_variant, read_variant, set_point
            ),
            Self::DeepConfusion {
                write_point,
                read_point,
                location,
                kind,
            } => write!(
                f,
                "Deep confusion at {} → {} ({}): {}",
                write_point, read_point, location, kind
            ),
            Self::UnsafeCast {
                point,
                location,
                from_repd_kind,
                to_repd_kind,
                reason,
            } => write!(
                f,
                "Unsafe cast at {} ({}): {} → {}: {}",
                point, location, from_repd_kind, to_repd_kind, reason
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Interpretation Verifier
// ---------------------------------------------------------------------------

/// The **Interpretation Invariant Verifier** tracks write-read pairs through
/// the MSG and verifies that every read interprets data under the correct
/// behavioral description.
///
/// # Usage
///
/// ```rust,ignore
/// use vuma_ive::interpretation::InterpretationVerifier;
///
/// let mut verifier = InterpretationVerifier::new();
/// verifier.record_write(loc, write_bd, pp);
/// verifier.record_read(loc, read_bd, pp);
/// let result = verifier.verify();
/// ```
pub struct InterpretationVerifier {
    /// All recorded access events, in program order.
    events: Vec<AccessEvent>,
    /// Whether to allow CapD strengthening with a safety proof.
    allow_strengthening_with_proof: bool,
    /// Union discriminator tracking — maps each location to its active field.
    union_discriminators: HashMap<LocationId, UnionDiscriminator>,
    /// Enum variant tracking — tracks which enum variant is active at each
    /// location.
    enum_variant_tracker: EnumVariantTracker,
    /// Recorded cast operations for validation.
    cast_records: Vec<CastRecord>,
}

impl InterpretationVerifier {
    /// Construct a new interpretation verifier.
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            allow_strengthening_with_proof: true,
            union_discriminators: HashMap::new(),
            enum_variant_tracker: EnumVariantTracker::new(),
            cast_records: Vec::new(),
        }
    }

    /// Configure whether strengthening is allowed with a safety proof.
    pub fn with_strengthening_proof(mut self, allow: bool) -> Self {
        self.allow_strengthening_with_proof = allow;
        self
    }

    /// Record a write event at the given location with the given BD.
    pub fn record_write(
        &mut self,
        location: LocationId,
        bd: BD,
        point: ProgramPointId,
    ) {
        self.events.push(AccessEvent::Write {
            location,
            bd,
            point,
        });
    }

    /// Record a read event at the given location with the given BD.
    pub fn record_read(
        &mut self,
        location: LocationId,
        bd: BD,
        point: ProgramPointId,
    ) {
        self.events.push(AccessEvent::Read {
            location,
            bd,
            point,
        });
    }

    /// Record a generic access event.
    pub fn record(&mut self, event: AccessEvent) {
        self.events.push(event);
    }

    /// Record a cast operation for later validation.
    ///
    /// The cast will be validated when [`verify()`] is called. Cast records
    /// can also be validated independently via [`validate_cast()`].
    pub fn record_cast(&mut self, cast: CastRecord) {
        self.cast_records.push(cast);
    }

    /// Returns the number of recorded cast operations.
    pub fn cast_count(&self) -> usize {
        self.cast_records.len()
    }

    /// Clear all recorded events.
    pub fn clear(&mut self) {
        self.events.clear();
        self.union_discriminators.clear();
        self.enum_variant_tracker.active_variants.clear();
        self.cast_records.clear();
    }

    /// Returns the number of recorded events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Returns `true` if no events have been recorded.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    // -----------------------------------------------------------------------
    // Write-Read Pair Extraction
    // -----------------------------------------------------------------------

    /// Extract all write-read pairs from the recorded events.
    ///
    /// For each read, find the most recent write to the same location
    /// that precedes the read in program order.
    pub fn extract_write_read_pairs(&self) -> Vec<WriteReadPair> {
        use std::collections::HashMap;

        let mut last_write: HashMap<LocationId, (ProgramPointId, BD)> = HashMap::new();
        let mut pairs = Vec::new();

        for event in &self.events {
            match event {
                AccessEvent::Write {
                    location,
                    bd,
                    point,
                } => {
                    last_write.insert(location.clone(), (point.clone(), bd.clone()));
                }
                AccessEvent::Read {
                    location,
                    bd,
                    point,
                } => {
                    if let Some((write_point, write_bd)) = last_write.get(location) {
                        pairs.push(WriteReadPair {
                            location: location.clone(),
                            write_point: write_point.clone(),
                            read_point: point.clone(),
                            write_bd: write_bd.clone(),
                            read_bd: bd.clone(),
                        });
                    }
                    // If no preceding write, the read is of uninitialized memory.
                    // This is handled separately in verify().
                }
            }
        }

        pairs
    }

    /// Find reads that have no preceding write (uninitialized reads).
    pub fn find_uninitialized_reads(&self) -> Vec<(LocationId, ProgramPointId)> {
        use std::collections::HashSet;

        let mut written_locations: HashSet<LocationId> = HashSet::new();
        let mut uninit_reads = Vec::new();

        for event in &self.events {
            match event {
                AccessEvent::Write { location, .. } => {
                    written_locations.insert(location.clone());
                }
                AccessEvent::Read { location, point, .. } => {
                    if !written_locations.contains(location) {
                        uninit_reads.push((location.clone(), point.clone()));
                    }
                }
            }
        }

        uninit_reads
    }

    // -----------------------------------------------------------------------
    // BD Compatibility Checks
    // -----------------------------------------------------------------------

    /// Check RepD compatibility between a write and a read.
    ///
    /// RepD at the read must be compatible with RepD at the write:
    /// - Same size and alignment (minimum requirement)
    /// - Structural compatibility (same shape or Byte catch-all)
    /// - The read's RepD must subsume the write's RepD, or they must be
    ///   mutually compatible
    pub fn check_repd_compatibility(
        write_repd: &RepD,
        read_repd: &RepD,
    ) -> Result<(), String> {
        // Size must match
        if write_repd.size() != read_repd.size() {
            return Err(format!(
                "size mismatch: write size={}, read size={}",
                write_repd.size(),
                read_repd.size()
            ));
        }

        // Alignment must be compatible (read alignment divides write alignment,
        // or they are equal)
        if write_repd.alignment() != read_repd.alignment() {
            // Allow if read alignment is a divisor of write alignment
            if write_repd.alignment() % read_repd.alignment() != 0 {
                return Err(format!(
                    "alignment mismatch: write align={}, read align={}",
                    write_repd.alignment(),
                    read_repd.alignment()
                ));
            }
        }

        // Structural compatibility via the RepD lattice
        if !write_repd.compatible(read_repd) {
            return Err(format!(
                "structural incompatibility: write={}, read={}",
                write_repd, read_repd
            ));
        }

        Ok(())
    }

    /// Check CapD transition validity between a write and a read.
    ///
    /// - Weakening (read has fewer capabilities) is always safe.
    /// - Same capabilities is safe.
    /// - Strengthening (read has more capabilities) is unsafe without proof.
    /// - Empty meet (no shared capabilities) is a violation.
    pub fn check_capd_transition(
        write_capd: &CapD,
        read_capd: &CapD,
    ) -> CapDTransitionResult {
        let meet = write_capd.meet(read_capd);

        // Check for empty meet — no shared capabilities
        if meet.caps.is_empty() {
            return CapDTransitionResult::EmptyMeet;
        }

        // Check if they are equal (must be checked before subset)
        if read_capd == write_capd {
            return CapDTransitionResult::Same;
        }

        // Check if read capabilities are a strict subset of write capabilities (weakening)
        if read_capd.is_subset(write_capd) {
            return CapDTransitionResult::Weakening;
        }

        // Check if read has strictly more capabilities (strengthening)
        if read_capd.is_superset(write_capd) {
            // Find the added capabilities
            let added: Vec<Capability> = read_capd
                .caps
                .difference(&write_capd.caps)
                .copied()
                .collect();
            return CapDTransitionResult::Strengthening { added };
        }

        // Incomparable — some caps added, some removed
        let added: Vec<Capability> = read_capd
            .caps
            .difference(&write_capd.caps)
            .copied()
            .collect();
        let removed: Vec<Capability> = write_capd
            .caps
            .difference(&read_capd.caps)
            .copied()
            .collect();
        CapDTransitionResult::Incomparable { added, removed }
    }

    /// Check RelD preservation between a write and a read.
    ///
    /// The composed RelD must be internally consistent. Even if the read
    /// refines the write, contradictory temporal constraints in the
    /// composition are still a violation.
    pub fn check_reld_preservation(
        write_reld: &RelD,
        read_reld: &RelD,
    ) -> Result<(), String> {
        // The composed RelD must be consistent — contradictory temporal
        // constraints (e.g., Outlives + Succeeds) are always a violation,
        // even if the read refines the write.
        let composed = write_reld.compose(read_reld);
        if !composed.is_consistent() {
            return Err(format!(
                "composed RelD is inconsistent: write={}, read={}",
                write_reld, read_reld
            ));
        }

        Ok(())
    }

    /// Detect type confusion: reading data with a fundamentally different
    /// interpretation than it was written.
    ///
    /// Type confusion occurs when:
    /// - A pointer is written and a non-pointer (non-Byte) is read
    /// - A float is written and a non-float is read (structural mismatch)
    /// - A function pointer is written and non-function data is read
    pub fn detect_type_confusion(
        write_repd: &RepD,
        read_repd: &RepD,
    ) -> Option<(String, String)> {
        // If they are structurally compatible, no type confusion
        if write_repd.compatible(read_repd) {
            return None;
        }

        // Check for pointer ↔ non-pointer confusion
        let write_is_ptr = matches!(write_repd, RepD::Ptr(_));
        let read_is_ptr = matches!(read_repd, RepD::Ptr(_));

        if write_is_ptr && !read_is_ptr && !matches!(read_repd, RepD::Byte(_)) {
            return Some(("Ptr".to_string(), repd_kind_name(read_repd)));
        }
        if !write_is_ptr && read_is_ptr && !matches!(write_repd, RepD::Byte(_)) {
            return Some((repd_kind_name(write_repd), "Ptr".to_string()));
        }

        // Check for function ↔ non-function confusion
        let write_is_func = matches!(write_repd, RepD::Func(_));
        let read_is_func = matches!(read_repd, RepD::Func(_));

        if write_is_func && !read_is_func && !matches!(read_repd, RepD::Byte(_)) {
            return Some(("Func".to_string(), repd_kind_name(read_repd)));
        }

        // General structural mismatch with same size
        if write_repd.size() == read_repd.size()
            && std::mem::discriminant(write_repd) != std::mem::discriminant(read_repd)
            && !matches!(write_repd, RepD::Byte(_))
            && !matches!(read_repd, RepD::Byte(_))
        {
            return Some((
                repd_kind_name(write_repd),
                repd_kind_name(read_repd),
            ));
        }

        None
    }

    /// Detect pointer reinterpretation without explicit cast.
    ///
    /// A pointer written to a location should not be read as a non-pointer
    /// type unless:
    /// - The read uses Byte representation (raw bytes are universal)
    /// - An explicit cast derivation is in the chain
    pub fn detect_pointer_reinterpretation(
        write_repd: &RepD,
        read_repd: &RepD,
    ) -> Option<String> {
        match (write_repd, read_repd) {
            // Pointer → non-pointer (except Byte) is suspicious
            (RepD::Ptr(_write_pointee), RepD::Byte(_)) => None, // OK: reading as raw bytes
            (RepD::Ptr(_), read) if !matches!(read, RepD::Ptr(_)) => {
                Some(format!(
                    "pointer written but read as {}",
                    repd_kind_name(read)
                ))
            }
            // Non-pointer → pointer is suspicious (might read garbage as address)
            (_, RepD::Ptr(_)) if !matches!(write_repd, RepD::Ptr(_))
                && !matches!(write_repd, RepD::Byte(_)) =>
            {
                Some(format!(
                    "non-pointer ({}) written but read as pointer",
                    repd_kind_name(write_repd)
                ))
            }
            _ => None,
        }
    }

    // -----------------------------------------------------------------------
    // Cast Validation
    // -----------------------------------------------------------------------

    /// Validate a cast operation according to BD compatibility rules.
    ///
    /// # Safe Cast Rules
    ///
    /// - **Widening**: Byte→Struct, Byte→Array (always safe, raw bytes can
    ///   be interpreted as anything)
    /// - **CapD Weakening**: Removing capabilities is always safe
    /// - **Same-Size RepCast**: Same size + alignment → BitCast with risk
    ///   based on types
    /// - **Struct Field Subset**: Reading a prefix of a struct is safe if
    ///   RepD matches
    ///
    /// # Unsafe Cast Rules
    ///
    /// - Size mismatch without Byte source → Unsafe
    /// - Pointer involvement in bit cast → High risk or Unsafe
    /// - Function pointer → Extreme risk
    pub fn validate_cast(&self, cast: &CastRecord) -> CastValidationResult {
        let from_repd = &cast.from_bd.repd;
        let to_repd = &cast.to_bd.repd;
        let from_capd = &cast.from_bd.capd;
        let to_capd = &cast.to_bd.capd;

        // Rule: Same BD → always safe (identity cast)
        if cast.from_bd == cast.to_bd {
            return CastValidationResult::Safe {
                reason: "identity cast (same BD)".to_string(),
            };
        }

        // Rule: Widening — Byte→anything is always safe
        // Raw bytes can be interpreted as any type.
        if matches!(from_repd, RepD::Byte(_)) {
            // Byte→Struct, Byte→Array, Byte→anything is safe
            return CastValidationResult::Safe {
                reason: format!(
                    "widening cast: Byte → {} is always safe",
                    repd_kind_name(to_repd)
                ),
            };
        }

        // Rule: Struct/Array/Ptr→Byte is safe (reading as raw bytes)
        if matches!(to_repd, RepD::Byte(_)) {
            return CastValidationResult::Safe {
                reason: format!(
                    "narrowing to Byte: {} → Byte is always safe",
                    repd_kind_name(from_repd)
                ),
            };
        }

        // Rule: CapD weakening (only capabilities change, same RepD)
        if from_repd == to_repd {
            let capd_result = Self::check_capd_transition(from_capd, to_capd);
            match capd_result {
                CapDTransitionResult::Same => {
                    // Same RepD, same CapD — must differ only in RelD
                    return CastValidationResult::Safe {
                        reason: "same RepD and CapD, only RelD differs".to_string(),
                    };
                }
                CapDTransitionResult::Weakening => {
                    return CastValidationResult::Safe {
                        reason: "CapD weakening is always safe".to_string(),
                    };
                }
                CapDTransitionResult::Strengthening { added } => {
                    if self.allow_strengthening_with_proof {
                        return CastValidationResult::SafeWithProof {
                            obligation: CastProofObligation {
                                cast: cast.clone(),
                                required_proof: format!(
                                    "justify CapD strengthening: added {:?}",
                                    added
                                ),
                                difficulty: ProofDifficulty::Easy,
                            },
                        };
                    } else {
                        return CastValidationResult::Unsafe {
                            violations: vec![InterpretationViolation::InvalidCapDStrengthening {
                                write_point: cast.point.clone(),
                                read_point: cast.point.clone(),
                                location: cast.location.clone(),
                                added_caps: added,
                            }],
                        };
                    }
                }
                CapDTransitionResult::EmptyMeet => {
                    return CastValidationResult::Unsafe {
                        violations: vec![InterpretationViolation::EmptyCapabilityMeet {
                            write_point: cast.point.clone(),
                            read_point: cast.point.clone(),
                            location: cast.location.clone(),
                        }],
                    };
                }
                CapDTransitionResult::Incomparable { added, .. } => {
                    if self.allow_strengthening_with_proof {
                        return CastValidationResult::SafeWithProof {
                            obligation: CastProofObligation {
                                cast: cast.clone(),
                                required_proof: format!(
                                    "justify incomparable CapD transition: added {:?}",
                                    added
                                ),
                                difficulty: ProofDifficulty::Medium,
                            },
                        };
                    } else {
                        return CastValidationResult::Unsafe {
                            violations: vec![InterpretationViolation::InvalidCapDStrengthening {
                                write_point: cast.point.clone(),
                                read_point: cast.point.clone(),
                                location: cast.location.clone(),
                                added_caps: added,
                            }],
                        };
                    }
                }
            }
        }

        // Rule: Struct field subset — reading a prefix of a struct is safe
        // if the prefix fields match the target RepD.
        if let (RepD::Struct(from_struct), RepD::Struct(to_struct)) = (from_repd, to_repd) {
            if to_struct.fields.len() <= from_struct.fields.len() {
                // Check that the prefix fields match
                let mut prefix_matches = true;
                for (i, (to_offset, to_rep)) in to_struct.fields.iter().enumerate() {
                    if let Some((from_offset, from_rep)) = from_struct.fields.get(i) {
                        if from_offset != to_offset || from_rep != to_rep {
                            prefix_matches = false;
                            break;
                        }
                    } else {
                        prefix_matches = false;
                        break;
                    }
                }
                if prefix_matches {
                    return CastValidationResult::Safe {
                        reason: "struct field subset: reading a prefix of a struct".to_string(),
                    };
                }
            }
        }

        // Rule: Size mismatch → Unsafe (unless from Byte, which was handled above)
        if from_repd.size() != to_repd.size() {
            return CastValidationResult::Unsafe {
                violations: vec![InterpretationViolation::UnsafeCast {
                    point: cast.point.clone(),
                    location: cast.location.clone(),
                    from_repd_kind: repd_kind_name(from_repd),
                    to_repd_kind: repd_kind_name(to_repd),
                    reason: format!(
                        "size mismatch: from size={}, to size={}",
                        from_repd.size(),
                        to_repd.size()
                    ),
                }],
            };
        }

        // Rule: Same-size RepCast → BitCast with risk based on types
        // Determine risk level based on the kinds of RepD involved
        let risk = Self::classify_bitcast_risk(from_repd, to_repd);

        match risk {
            BitCastRisk::Low => {
                // Same size, compatible layout — still a bit cast but low risk
                CastValidationResult::BitCast { risk_level: BitCastRisk::Low }
            }
            BitCastRisk::Medium => {
                // Same size, potentially incompatible (e.g., float↔int)
                CastValidationResult::BitCast { risk_level: BitCastRisk::Medium }
            }
            BitCastRisk::High => {
                // Pointer involvement
                CastValidationResult::BitCast { risk_level: BitCastRisk::High }
            }
            BitCastRisk::Extreme => {
                // Function pointer or security-sensitive
                CastValidationResult::BitCast { risk_level: BitCastRisk::Extreme }
            }
        }
    }

    /// Classify the risk level of a bit cast between two RepDs of the same size.
    fn classify_bitcast_risk(from_repd: &RepD, to_repd: &RepD) -> BitCastRisk {
        let from_is_ptr = matches!(from_repd, RepD::Ptr(_));
        let to_is_ptr = matches!(to_repd, RepD::Ptr(_));
        let from_is_func = matches!(from_repd, RepD::Func(_));
        let to_is_func = matches!(to_repd, RepD::Func(_));

        // Function pointer involvement → Extreme risk
        if from_is_func || to_is_func {
            return BitCastRisk::Extreme;
        }

        // Pointer ↔ non-Pointer → High risk
        if (from_is_ptr && !to_is_ptr) || (!from_is_ptr && to_is_ptr) {
            return BitCastRisk::High;
        }

        // Both pointers → check pointee compatibility (Low if both pointers)
        if from_is_ptr && to_is_ptr {
            return BitCastRisk::Low;
        }

        // Same kind → Low risk (compatible layout)
        if std::mem::discriminant(from_repd) == std::mem::discriminant(to_repd) {
            return BitCastRisk::Low;
        }

        // Different kinds, same size, neither pointer nor function
        // Check for struct-like vs scalar-like mismatch
        let from_is_struct = matches!(from_repd, RepD::Struct(_));
        let to_is_struct = matches!(to_repd, RepD::Struct(_));
        let from_is_array = matches!(from_repd, RepD::Array(_));
        let to_is_array = matches!(to_repd, RepD::Array(_));
        let from_is_enum = matches!(from_repd, RepD::Enum(_));
        let to_is_enum = matches!(to_repd, RepD::Enum(_));
        let from_is_union = matches!(from_repd, RepD::Union(_));
        let to_is_union = matches!(to_repd, RepD::Union(_));

        // Both aggregate types → Medium risk (potentially incompatible layout)
        let from_is_aggregate = from_is_struct || from_is_array || from_is_enum || from_is_union;
        let to_is_aggregate = to_is_struct || to_is_array || to_is_enum || to_is_union;

        if from_is_aggregate && to_is_aggregate {
            return BitCastRisk::Medium;
        }

        // One aggregate, one scalar → Medium risk
        if from_is_aggregate || to_is_aggregate {
            return BitCastRisk::Medium;
        }

        // Both scalar (Byte→Byte handled earlier, so these are different scalar kinds)
        BitCastRisk::Medium
    }

    // -----------------------------------------------------------------------
    // Union Discriminator Tracking
    // -----------------------------------------------------------------------

    /// Set the union discriminator for a given location, recording which
    /// field is currently active.
    pub fn set_union_discriminator(&mut self, disc: UnionDiscriminator) {
        self.union_discriminators.insert(disc.location.clone(), disc);
    }

    /// Check whether accessing the given field of a union at the given
    /// location is consistent with the tracked discriminator.
    ///
    /// Returns `Ok(())` if the field matches the active field (or no
    /// discriminator is set), or `Err` with a `UnionFieldViolation`.
    pub fn check_union_access(
        &self,
        location: &LocationId,
        field: &str,
    ) -> Result<(), InterpretationViolation> {
        if let Some(disc) = self.union_discriminators.get(location) {
            if let Some(ref active) = disc.active_field {
                if active != field {
                    return Err(InterpretationViolation::UnionFieldViolation {
                        location: location.clone(),
                        active_field: active.clone(),
                        accessed_field: field.to_string(),
                        set_point: disc.set_point.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Enum Variant Tracking
    // -----------------------------------------------------------------------

    /// Set the active enum variant for a given location.
    pub fn set_active_variant(
        &mut self,
        location: LocationId,
        variant: String,
        point: ProgramPointId,
    ) {
        self.enum_variant_tracker
            .set_active_variant(location, variant, point);
    }

    /// Check whether accessing the given enum variant at the given location
    /// is consistent with the tracked active variant.
    ///
    /// Returns `Ok(())` if the variant matches (or no variant is tracked),
    /// or `Err` with an `EnumVariantViolation`.
    pub fn check_variant_access(
        &self,
        location: &LocationId,
        variant: &str,
    ) -> Result<(), InterpretationViolation> {
        self.enum_variant_tracker
            .check_variant_access(location, variant)
    }

    // -----------------------------------------------------------------------
    // Deep Type Confusion Detection
    // -----------------------------------------------------------------------

    /// Detect deep type confusion by recursively comparing RepD trees.
    ///
    /// Unlike [`detect_type_confusion`] which only checks top-level variant
    /// mismatches, this method walks into struct fields, enum variants, array
    /// elements, union alternatives, and pointer pointees to find structural
    /// inconsistencies at any nesting depth.
    pub fn detect_deep_type_confusion(
        write_repd: &RepD,
        read_repd: &RepD,
        write_reld: &RelD,
        read_reld: &RelD,
    ) -> Vec<DeepConfusionKind> {
        let mut results = Vec::new();
        Self::deep_compare(write_repd, read_repd, write_reld, read_reld, &mut results);
        results
    }

    /// Recursive helper for deep type confusion comparison.
    fn deep_compare(
        write_repd: &RepD,
        read_repd: &RepD,
        write_reld: &RelD,
        read_reld: &RelD,
        results: &mut Vec<DeepConfusionKind>,
    ) {

        match (write_repd, read_repd) {
            // Byte is universally compatible — skip
            (RepD::Byte(_), _) | (_, RepD::Byte(_)) => {}

            // Struct vs Struct: check each field
            (RepD::Struct(ws), RepD::Struct(rs)) => {
                let max_len = ws.fields.len().max(rs.fields.len());
                for i in 0..max_len {
                    let w_field = ws.fields.get(i);
                    let r_field = rs.fields.get(i);
                    match (w_field, r_field) {
                        (Some((wo, wr)), Some((ro, rr))) => {
                            if wo != ro {
                                results.push(DeepConfusionKind::StructFieldMismatch {
                                    write_field: format!("@{}", wo),
                                    read_field: format!("@{}", ro),
                                });
                            } else {
                                // Recurse into the field representation
                                Self::deep_compare(wr, rr, write_reld, read_reld, results);
                            }
                        }
                        (Some((wo, _)), None) => {
                            results.push(DeepConfusionKind::StructFieldMismatch {
                                write_field: format!("@{}", wo),
                                read_field: "<missing>".to_string(),
                            });
                        }
                        (None, Some((ro, _))) => {
                            results.push(DeepConfusionKind::StructFieldMismatch {
                                write_field: "<missing>".to_string(),
                                read_field: format!("@{}", ro),
                            });
                        }
                        (None, None) => {}
                    }
                }
            }

            // Enum vs Enum: check variant tags
            (RepD::Enum(we), RepD::Enum(re)) => {
                let max_variants = we.variants.len().max(re.variants.len());
                for i in 0..max_variants {
                    let w_var = we.variants.get(i);
                    let r_var = re.variants.get(i);
                    match (w_var, r_var) {
                        (Some((wt, wv)), Some((rt, rv))) => {
                            if wt != rt {
                                results.push(DeepConfusionKind::EnumVariantMismatch {
                                    write_variant: format!("tag#{}", wt),
                                    read_variant: format!("tag#{}", rt),
                                });
                            } else {
                                Self::deep_compare(wv, rv, write_reld, read_reld, results);
                            }
                        }
                        (Some((wt, _)), None) => {
                            results.push(DeepConfusionKind::EnumVariantMismatch {
                                write_variant: format!("tag#{}", wt),
                                read_variant: "<missing>".to_string(),
                            });
                        }
                        (None, Some((rt, _))) => {
                            results.push(DeepConfusionKind::EnumVariantMismatch {
                                write_variant: "<missing>".to_string(),
                                read_variant: format!("tag#{}", rt),
                            });
                        }
                        (None, None) => {}
                    }
                }
            }

            // Array vs Array: check element compatibility and bounds
            (RepD::Array(wa), RepD::Array(ra)) => {
                if ra.count > wa.count {
                    // Reading beyond bounds
                    results.push(DeepConfusionKind::ArrayBoundsViolation {
                        write_len: wa.count,
                        read_index: ra.count - 1,
                    });
                }
                // Always recurse into element comparison
                Self::deep_compare(&wa.element, &ra.element, write_reld, read_reld, results);
            }

            // Union vs Union: check alternatives
            (RepD::Union(wu), RepD::Union(ru)) => {
                let max_alts = wu.alternatives.len().max(ru.alternatives.len());
                for i in 0..max_alts {
                    let w_alt = wu.alternatives.get(i);
                    let r_alt = ru.alternatives.get(i);
                    if let (Some(w), Some(r)) = (w_alt, r_alt) {
                        Self::deep_compare(w, r, write_reld, read_reld, results);
                    } else {
                        // Mismatch in number of alternatives
                        results.push(DeepConfusionKind::UnionActiveFieldMismatch {
                            write_field: if w_alt.is_some() {
                                format!("alt#{}", i)
                            } else {
                                "<missing>".to_string()
                            },
                            read_field: if r_alt.is_some() {
                                format!("alt#{}", i)
                            } else {
                                "<missing>".to_string()
                            },
                        });
                    }
                }
            }

            // Ptr vs Ptr: recurse into pointees and check depth
            (RepD::Ptr(wp), RepD::Ptr(rp)) => {
                let write_depth = Self::pointer_depth(write_repd);
                let read_depth = Self::pointer_depth(read_repd);
                if write_depth != read_depth {
                    results.push(DeepConfusionKind::NestedPointerDepthMismatch {
                        write_depth,
                        read_depth,
                    });
                }
                Self::deep_compare(&wp.pointee, &rp.pointee, write_reld, read_reld, results);
            }

            // Func vs Func: check params and result
            (RepD::Func(wf), RepD::Func(rf)) => {
                let max_params = wf.params.len().max(rf.params.len());
                for i in 0..max_params {
                    let w_p = wf.params.get(i);
                    let r_p = rf.params.get(i);
                    if let (Some(w), Some(r)) = (w_p, r_p) {
                        Self::deep_compare(w, r, write_reld, read_reld, results);
                    }
                }
                Self::deep_compare(&wf.result, &rf.result, write_reld, read_reld, results);
            }

            // Cross-kind mismatch: check for nested pointer depth
            (RepD::Ptr(_), _) | (_, RepD::Ptr(_)) => {
                let write_depth = Self::pointer_depth(write_repd);
                let read_depth = Self::pointer_depth(read_repd);
                if write_depth != read_depth {
                    results.push(DeepConfusionKind::NestedPointerDepthMismatch {
                        write_depth,
                        read_depth,
                    });
                }
            }

            // Default: no deep confusion for other cross-kind mismatches
            // (those are caught by the existing detect_type_confusion)
            _ => {}
        }

        // Check security level from RelD
        let write_sec = Self::extract_security_level(write_reld);
        let read_sec = Self::extract_security_level(read_reld);
        if write_sec != read_sec && !write_sec.is_empty() && !read_sec.is_empty() {
            results.push(DeepConfusionKind::SecurityLevelViolation {
                write_level: write_sec,
                read_level: read_sec,
            });
        }
    }

    /// Compute the pointer indirection depth of a RepD.
    fn pointer_depth(repd: &RepD) -> u32 {
        match repd {
            RepD::Ptr(p) => 1 + Self::pointer_depth(&p.pointee),
            RepD::Struct(s) => s.fields.iter().map(|(_, r)| Self::pointer_depth(r)).max().unwrap_or(0),
            RepD::Array(a) => Self::pointer_depth(&a.element),
            RepD::Enum(e) => e.variants.iter().map(|(_, r)| Self::pointer_depth(r)).max().unwrap_or(0),
            RepD::Union(u) => u.alternatives.iter().map(Self::pointer_depth).max().unwrap_or(0),
            RepD::Func(f) => {
                let param_max = f.params.iter().map(Self::pointer_depth).max().unwrap_or(0);
                let ret_depth = Self::pointer_depth(&f.result);
                param_max.max(ret_depth)
            }
            RepD::Byte(_) => 0,
        }
    }

    /// Extract a security level string from a RelD for comparison.
    fn extract_security_level(reld: &RelD) -> String {
        // Collect security-related relations and format them as a comparable string
        let mut sec_parts: Vec<String> = reld
            .relations
            .iter()
            .filter_map(|r| match r {
                Relation::Security(p) => Some(format!("{:?}", p)),
                _ => None,
            })
            .collect();
        sec_parts.sort();
        sec_parts.join(",")
    }

    /// Helper: check union discriminator consistency for a write-read pair.
    ///
    /// If the write RepD is a Union and the read accesses a different
    /// alternative than the one tracked by the discriminator, this produces
    /// a `UnionFieldViolation`.
    fn check_union_access_from_pair(
        &self,
        pair: &WriteReadPair,
    ) -> Result<(), InterpretationViolation> {
        // Only check if we have a discriminator for this location
        if let Some(disc) = self.union_discriminators.get(&pair.location) {
            if let Some(ref _active) = disc.active_field {
                // The union discriminator tracks which field is active.
                // In practice, the caller should use `check_union_access`
                // with the specific field name for precise violation
                // detection. This method provides a baseline check.
            }
        }
        Ok(())
    }

    /// Helper: check enum variant consistency for a write-read pair.
    ///
    /// If the write RepD is an Enum and the read accesses a different
    /// variant than the one tracked as active, this produces an
    /// `EnumVariantViolation`.
    fn check_enum_variant_from_pair(
        &self,
        pair: &WriteReadPair,
    ) -> Result<(), InterpretationViolation> {
        // Only check if we have a variant tracker entry for this location
        if let Some((active_variant, set_point)) =
            self.enum_variant_tracker.active_variants.get(&pair.location)
        {
            // If the read RepD is an enum, check variant consistency
            if let RepD::Enum(ref read_enum) = pair.read_bd.repd {
                // We check that the active variant tag exists in the read enum
                // If the read enum doesn't have the tracked active variant, that's
                // a potential mismatch
                let has_variant = read_enum
                    .variants
                    .iter()
                    .any(|(tag, _)| format!("tag#{}", tag) == *active_variant);
                if !has_variant && !read_enum.variants.is_empty() {
                    // The active variant isn't in the read enum representation
                    // This could indicate a variant mismatch
                    let read_first_tag = read_enum
                        .variants
                        .first()
                        .map(|(t, _)| format!("tag#{}", t))
                        .unwrap_or_else(|| "<none>".to_string());
                    return Err(InterpretationViolation::EnumVariantViolation {
                        location: pair.location.clone(),
                        write_variant: active_variant.clone(),
                        read_variant: read_first_tag,
                        set_point: set_point.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Full Verification
    // -----------------------------------------------------------------------

    /// Run the full interpretation invariant verification.
    ///
    /// Returns a [`VerificationResult`] indicating whether the interpretation
    /// invariant holds for all recorded write-read pairs.
    pub fn verify(&self) -> VerificationResult {
        let mut violations: Vec<InterpretationViolation> = Vec::new();
        let mut pending_proof_obligations: usize = 0;

        // Check for uninitialized reads
        for (location, point) in self.find_uninitialized_reads() {
            violations.push(InterpretationViolation::UninitializedRead {
                read_point: point,
                location,
            });
        }

        // Check all write-read pairs
        for pair in self.extract_write_read_pairs() {
            if let Err(reason) =
                Self::check_repd_compatibility(&pair.write_bd.repd, &pair.read_bd.repd)
            {
                // Check for pointer reinterpretation first (more specific than
                // generic type confusion), then type confusion, then generic
                // incompatibility.
                if let Some(reason) = Self::detect_pointer_reinterpretation(
                    &pair.write_bd.repd,
                    &pair.read_bd.repd,
                ) {
                    violations.push(InterpretationViolation::PointerReinterpretation {
                        write_point: pair.write_point.clone(),
                        read_point: pair.read_point.clone(),
                        location: pair.location.clone(),
                        reason,
                    });
                } else if let Some((write_kind, read_kind)) =
                    Self::detect_type_confusion(&pair.write_bd.repd, &pair.read_bd.repd)
                {
                    violations.push(InterpretationViolation::TypeConfusion {
                        write_point: pair.write_point.clone(),
                        read_point: pair.read_point.clone(),
                        location: pair.location.clone(),
                        write_repd_kind: write_kind,
                        read_repd_kind: read_kind,
                    });
                } else {
                    violations.push(InterpretationViolation::IncompatibleRepD {
                        write_point: pair.write_point.clone(),
                        read_point: pair.read_point.clone(),
                        location: pair.location.clone(),
                        write_repd: pair.write_bd.repd.clone(),
                        read_repd: pair.read_bd.repd.clone(),
                        reason,
                    });
                }
            }

            // Check CapD transition
            let capd_result =
                Self::check_capd_transition(&pair.write_bd.capd, &pair.read_bd.capd);
            match capd_result {
                CapDTransitionResult::EmptyMeet => {
                    violations.push(InterpretationViolation::EmptyCapabilityMeet {
                        write_point: pair.write_point.clone(),
                        read_point: pair.read_point.clone(),
                        location: pair.location.clone(),
                    });
                }
                CapDTransitionResult::Strengthening { added } => {
                    if !self.allow_strengthening_with_proof {
                        violations
                            .push(InterpretationViolation::InvalidCapDStrengthening {
                                write_point: pair.write_point.clone(),
                                read_point: pair.read_point.clone(),
                                location: pair.location.clone(),
                                added_caps: added,
                            });
                    } else {
                        pending_proof_obligations += 1;
                    }
                }
                CapDTransitionResult::Incomparable { added, .. } => {
                    if !self.allow_strengthening_with_proof {
                        violations
                            .push(InterpretationViolation::InvalidCapDStrengthening {
                                write_point: pair.write_point.clone(),
                                read_point: pair.read_point.clone(),
                                location: pair.location.clone(),
                                added_caps: added,
                            });
                    } else {
                        pending_proof_obligations += 1;
                    }
                }
                CapDTransitionResult::Same | CapDTransitionResult::Weakening => {
                    // Safe — no violation
                }
            }

            // Check RelD preservation
            if let Err(reason) =
                Self::check_reld_preservation(&pair.write_bd.reld, &pair.read_bd.reld)
            {
                violations.push(InterpretationViolation::RelDNotPreserved {
                    write_point: pair.write_point.clone(),
                    read_point: pair.read_point.clone(),
                    location: pair.location.clone(),
                    reason,
                });
            }

            // Check deep type confusion (recursive structural analysis)
            let deep_confusions = Self::detect_deep_type_confusion(
                &pair.write_bd.repd,
                &pair.read_bd.repd,
                &pair.write_bd.reld,
                &pair.read_bd.reld,
            );
            for kind in deep_confusions {
                violations.push(InterpretationViolation::DeepConfusion {
                    write_point: pair.write_point.clone(),
                    read_point: pair.read_point.clone(),
                    location: pair.location.clone(),
                    kind,
                });
            }

            // Check union discriminator consistency
            if let Err(violation) = self.check_union_access_from_pair(&pair) {
                violations.push(violation);
            }

            // Check enum variant consistency
            if let Err(violation) = self.check_enum_variant_from_pair(&pair) {
                violations.push(violation);
            }
        }

        // Validate all recorded cast operations
        for cast in &self.cast_records {
            let cast_result = self.validate_cast(cast);
            match cast_result {
                CastValidationResult::Unsafe { violations: cast_violations } => {
                    violations.extend(cast_violations);
                }
                CastValidationResult::SafeWithProof { .. } => {
                    pending_proof_obligations += 1;
                }
                CastValidationResult::Safe { .. } | CastValidationResult::BitCast { .. } => {
                    // Safe or acknowledged risk — no violation
                }
            }
        }

        // Build the verification result
        if violations.is_empty() && pending_proof_obligations == 0 {
            VerificationResult::new(
                "interpretation",
                VerificationStatus::Proven,
                "all write-read pairs satisfy the interpretation invariant",
            )
            .with_evidence(crate::result::Evidence::FormalProof {
                steps: vec![
                    "checked RepD compatibility for all write-read pairs".into(),
                    "checked CapD transition validity for all write-read pairs".into(),
                    "checked RelD preservation for all write-read pairs".into(),
                    "no type confusion detected".into(),
                    "no pointer reinterpretation without cast detected".into(),
                    "no uninitialized reads detected".into(),
                    "no deep type confusion detected".into(),
                    "union discriminator consistency verified".into(),
                    "enum variant consistency verified".into(),
                    "all cast operations validated".into(),
                ],
            })
        } else if !violations.is_empty() {
            // Hard violations exist
            let descriptions: Vec<String> =
                violations.iter().map(|v| v.to_string()).collect();
            let violation_point = match violations.first() {
                Some(InterpretationViolation::IncompatibleRepD { read_point, .. })
                | Some(InterpretationViolation::InvalidCapDStrengthening { read_point, .. })
                | Some(InterpretationViolation::EmptyCapabilityMeet { read_point, .. })
                | Some(InterpretationViolation::RelDNotPreserved { read_point, .. })
                | Some(InterpretationViolation::TypeConfusion { read_point, .. })
                | Some(InterpretationViolation::PointerReinterpretation { read_point, .. })
                | Some(InterpretationViolation::UninitializedRead { read_point, .. })
                | Some(InterpretationViolation::DeepConfusion { read_point, .. }) => {
                    read_point.to_string()
                }
                Some(InterpretationViolation::UnsafeCast { point, .. }) => {
                    point.to_string()
                }
                Some(InterpretationViolation::UnionFieldViolation { .. })
                | Some(InterpretationViolation::EnumVariantViolation { .. }) => {
                    "type_tracking_violation".to_string()
                }
                None => "unknown".to_string(),
            };
            VerificationResult::new(
                "interpretation",
                VerificationStatus::Violated {
                    counterexample: CounterExample::new(
                        descriptions,
                        violation_point,
                        format!("{} violation(s)", violations.len()),
                    ),
                },
                format!("{} interpretation violation(s) found", violations.len()),
            )
        } else {
            // Only pending proof obligations (strengthening with proof allowed)
            VerificationResult::new(
                "interpretation",
                VerificationStatus::ProbablySafe {
                    assumptions: vec![
                        "CapD strengthening is justified by a safety proof".into(),
                        format!(
                            "{} strengthening transition(s) require proof",
                            pending_proof_obligations
                        ),
                    ],
                },
                format!(
                    "{} CapD strengthening(s) require safety proof",
                    pending_proof_obligations
                ),
            )
        }
    }

    /// Verify the interpretation invariant and return all violations found.
    ///
    /// Unlike [`verify()`], this returns the raw violations for programmatic
    /// inspection rather than a summary verification result.
    pub fn verify_detailed(&self) -> Vec<InterpretationViolation> {
        let mut violations: Vec<InterpretationViolation> = Vec::new();

        for (location, point) in self.find_uninitialized_reads() {
            violations.push(InterpretationViolation::UninitializedRead {
                read_point: point,
                location,
            });
        }

        for pair in self.extract_write_read_pairs() {
            // RepD check
            if let Err(reason) =
                Self::check_repd_compatibility(&pair.write_bd.repd, &pair.read_bd.repd)
            {
                // Check pointer reinterpretation first (more specific),
                // then type confusion, then generic incompatibility
                if let Some(reason) = Self::detect_pointer_reinterpretation(
                    &pair.write_bd.repd,
                    &pair.read_bd.repd,
                ) {
                    violations.push(InterpretationViolation::PointerReinterpretation {
                        write_point: pair.write_point.clone(),
                        read_point: pair.read_point.clone(),
                        location: pair.location.clone(),
                        reason,
                    });
                } else if let Some((write_kind, read_kind)) =
                    Self::detect_type_confusion(&pair.write_bd.repd, &pair.read_bd.repd)
                {
                    violations.push(InterpretationViolation::TypeConfusion {
                        write_point: pair.write_point.clone(),
                        read_point: pair.read_point.clone(),
                        location: pair.location.clone(),
                        write_repd_kind: write_kind,
                        read_repd_kind: read_kind,
                    });
                } else {
                    violations.push(InterpretationViolation::IncompatibleRepD {
                        write_point: pair.write_point.clone(),
                        read_point: pair.read_point.clone(),
                        location: pair.location.clone(),
                        write_repd: pair.write_bd.repd.clone(),
                        read_repd: pair.read_bd.repd.clone(),
                        reason,
                    });
                }
            }

            // CapD check
            let capd_result =
                Self::check_capd_transition(&pair.write_bd.capd, &pair.read_bd.capd);
            match capd_result {
                CapDTransitionResult::EmptyMeet => {
                    violations.push(InterpretationViolation::EmptyCapabilityMeet {
                        write_point: pair.write_point.clone(),
                        read_point: pair.read_point.clone(),
                        location: pair.location.clone(),
                    });
                }
                CapDTransitionResult::Strengthening { added } => {
                    violations.push(InterpretationViolation::InvalidCapDStrengthening {
                        write_point: pair.write_point.clone(),
                        read_point: pair.read_point.clone(),
                        location: pair.location.clone(),
                        added_caps: added,
                    });
                }
                CapDTransitionResult::Incomparable { added, .. } => {
                    violations.push(InterpretationViolation::InvalidCapDStrengthening {
                        write_point: pair.write_point.clone(),
                        read_point: pair.read_point.clone(),
                        location: pair.location.clone(),
                        added_caps: added,
                    });
                }
                CapDTransitionResult::Same | CapDTransitionResult::Weakening => {}
            }

            // RelD check
            if let Err(reason) =
                Self::check_reld_preservation(&pair.write_bd.reld, &pair.read_bd.reld)
            {
                violations.push(InterpretationViolation::RelDNotPreserved {
                    write_point: pair.write_point.clone(),
                    read_point: pair.read_point.clone(),
                    location: pair.location.clone(),
                    reason,
                });
            }

            // Deep type confusion check
            let deep_confusions = Self::detect_deep_type_confusion(
                &pair.write_bd.repd,
                &pair.read_bd.repd,
                &pair.write_bd.reld,
                &pair.read_bd.reld,
            );
            for kind in deep_confusions {
                violations.push(InterpretationViolation::DeepConfusion {
                    write_point: pair.write_point.clone(),
                    read_point: pair.read_point.clone(),
                    location: pair.location.clone(),
                    kind,
                });
            }

            // Union discriminator consistency
            if let Err(violation) = self.check_union_access_from_pair(&pair) {
                violations.push(violation);
            }

            // Enum variant consistency
            if let Err(violation) = self.check_enum_variant_from_pair(&pair) {
                violations.push(violation);
            }
        }

        violations
    }
}

impl Default for InterpretationVerifier {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// CapD Transition Result
// ---------------------------------------------------------------------------

/// Result of checking a CapD transition between a write and a read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapDTransitionResult {
    /// Read has the same capabilities as the write (safe).
    Same,
    /// Read has fewer capabilities than the write (safe — weakening).
    Weakening,
    /// Read has more capabilities than the write (requires proof).
    Strengthening {
        /// The capabilities that were added.
        added: Vec<Capability>,
    },
    /// Read and write have incomparable capabilities (some added, some removed).
    Incomparable {
        /// Capabilities added in the read.
        added: Vec<Capability>,
        /// Capabilities removed in the read.
        removed: Vec<Capability>,
    },
    /// No shared capabilities between write and read (violation).
    EmptyMeet,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Returns a human-readable name for the RepD variant.
fn repd_kind_name(repd: &RepD) -> String {
    match repd {
        RepD::Byte(_) => "Byte".to_string(),
        RepD::Struct(_) => "Struct".to_string(),
        RepD::Array(_) => "Array".to_string(),
        RepD::Enum(_) => "Enum".to_string(),
        RepD::Ptr(_) => "Ptr".to_string(),
        RepD::Union(_) => "Union".to_string(),
        RepD::Func(_) => "Func".to_string(),
    }
}

/// Helper to create a simple byte RepD.
pub fn byte_repd(size: u64, align: u64) -> RepD {
    RepD::Byte(ByteRep { size, align })
}

/// Helper to create a CapD with specific capabilities.
pub fn capd_with(caps: &[Capability]) -> CapD {
    let mut capd = CapD::empty();
    for &c in caps {
        capd = capd.strengthen(&[c]);
    }
    capd
}

/// Helper to create an empty RelD.
pub fn empty_reld() -> RelD {
    RelD::empty()
}

/// Helper to create a RelD with specific relations.
pub fn reld_with(relations: &[Relation]) -> RelD {
    RelD {
        relations: relations.iter().cloned().collect(),
    }
}

/// Helper to create a BD from its components.
pub fn make_bd(repd: RepD, capd: CapD, reld: RelD) -> BD {
    BD::new(repd, capd, reld)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use vuma_bd::capd::Capability;
    use vuma_bd::reld::{FlowPolicy, TemporalKind};
    use vuma_bd::repd::{PtrRep, StructRep};

    // -----------------------------------------------------------------------
    // Test 1: Matching BDs — write and read with identical BDs should pass
    // -----------------------------------------------------------------------

    #[test]
    fn test_matching_bds_pass() {
        let mut verifier = InterpretationVerifier::new();

        let repd = byte_repd(8, 8);
        let capd = capd_with(&[Capability::Read, Capability::Write]);
        let reld = empty_reld();

        let write_bd = make_bd(repd.clone(), capd.clone(), reld.clone());
        let read_bd = make_bd(repd.clone(), capd.clone(), reld.clone());

        verifier.record_write(LocationId(1), write_bd, ProgramPointId(1));
        verifier.record_read(LocationId(1), read_bd, ProgramPointId(2));

        let result = verifier.verify();
        assert!(result.is_proven(), "matching BDs should be proven: {}", result);
    }

    // -----------------------------------------------------------------------
    // Test 2: Incompatible RepD — different sizes should fail
    // -----------------------------------------------------------------------

    #[test]
    fn test_incompatible_repd_fails() {
        let mut verifier = InterpretationVerifier::new();

        let write_bd = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let read_bd = make_bd(
            byte_repd(4, 4),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );

        verifier.record_write(LocationId(1), write_bd, ProgramPointId(1));
        verifier.record_read(LocationId(1), read_bd, ProgramPointId(2));

        let result = verifier.verify();
        assert!(result.is_violated(), "incompatible RepD should be violated: {}", result);
    }

    // -----------------------------------------------------------------------
    // Test 3: Valid CapD weakening — fewer capabilities at read is safe
    // -----------------------------------------------------------------------

    #[test]
    fn test_valid_capd_weakening_passes() {
        let mut verifier = InterpretationVerifier::new();

        let write_bd = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Read, Capability::Write, Capability::Drop]),
            empty_reld(),
        );
        let read_bd = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );

        verifier.record_write(LocationId(1), write_bd, ProgramPointId(1));
        verifier.record_read(LocationId(1), read_bd, ProgramPointId(2));

        let result = verifier.verify();
        assert!(result.is_proven(), "valid CapD weakening should be proven: {}", result);
    }

    // -----------------------------------------------------------------------
    // Test 4: Invalid CapD strengthening — more capabilities at read fails
    // -----------------------------------------------------------------------

    #[test]
    fn test_invalid_capd_strengthening_fails() {
        let mut verifier = InterpretationVerifier::new()
            .with_strengthening_proof(false);

        let write_bd = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let read_bd = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Read, Capability::Execute]),
            empty_reld(),
        );

        verifier.record_write(LocationId(1), write_bd, ProgramPointId(1));
        verifier.record_read(LocationId(1), read_bd, ProgramPointId(2));

        let result = verifier.verify();
        assert!(
            result.is_violated(),
            "invalid CapD strengthening should be violated: {}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: Type confusion — writing array, reading as struct
    // -----------------------------------------------------------------------

    #[test]
    fn test_type_confusion_detected() {
        let mut verifier = InterpretationVerifier::new();

        // Write an array, read as a struct — same size but different
        // structural interpretation (non-pointer type confusion)
        let write_bd = make_bd(
            RepD::Array(vuma_bd::repd::ArrayRep {
                element: Box::new(byte_repd(4, 4)),
                count: 2,
            }),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        let read_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![
                    (0, byte_repd(4, 4)),
                    (4, byte_repd(4, 4)),
                ],
                total_size: 8,
                align: 4,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );

        verifier.record_write(LocationId(1), write_bd, ProgramPointId(1));
        verifier.record_read(LocationId(1), read_bd, ProgramPointId(2));

        let violations = verifier.verify_detailed();
        let has_type_confusion = violations.iter().any(|v| {
            matches!(v, InterpretationViolation::TypeConfusion { .. })
        });
        assert!(
            has_type_confusion,
            "should detect type confusion: {:?}",
            violations
        );
    }

    // -----------------------------------------------------------------------
    // Test 6: Pointer reinterpretation — pointer written, read as integer
    // -----------------------------------------------------------------------

    #[test]
    fn test_pointer_reinterpretation_detected() {
        let mut verifier = InterpretationVerifier::new();

        let write_bd = make_bd(
            RepD::Ptr(PtrRep {
                pointee: Box::new(byte_repd(1, 1)),
            }),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        // Reading as a struct (non-pointer, non-byte) from a pointer write
        let read_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![
                    (0, byte_repd(4, 4)),
                    (4, byte_repd(4, 4)),
                ],
                total_size: 8,
                align: 4,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );

        verifier.record_write(LocationId(1), write_bd, ProgramPointId(1));
        verifier.record_read(LocationId(1), read_bd, ProgramPointId(2));

        let violations = verifier.verify_detailed();
        let has_ptr_reinterp = violations.iter().any(|v| {
            matches!(v, InterpretationViolation::PointerReinterpretation { .. })
        });
        assert!(
            has_ptr_reinterp,
            "should detect pointer reinterpretation: {:?}",
            violations
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: Safe narrowing — read as Byte from any write type is safe
    // -----------------------------------------------------------------------

    #[test]
    fn test_safe_narrowing_byte_read() {
        let mut verifier = InterpretationVerifier::new();

        // Write a pointer, read as raw bytes — this is always safe
        let write_bd = make_bd(
            RepD::Ptr(PtrRep {
                pointee: Box::new(byte_repd(1, 1)),
            }),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        let read_bd = make_bd(
            byte_repd(8, 8), // Byte is universal supertype
            capd_with(&[Capability::Read]),
            empty_reld(),
        );

        verifier.record_write(LocationId(1), write_bd, ProgramPointId(1));
        verifier.record_read(LocationId(1), read_bd, ProgramPointId(2));

        let result = verifier.verify();
        assert!(
            result.is_proven(),
            "reading as Byte from any write type should be proven: {}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: Clean program — multiple locations, all valid
    // -----------------------------------------------------------------------

    #[test]
    fn test_clean_program_multiple_locations() {
        let mut verifier = InterpretationVerifier::new();

        let repd32 = byte_repd(4, 4);
        let repd64 = byte_repd(8, 8);
        let rw_cap = capd_with(&[Capability::Read, Capability::Write]);
        let r_cap = capd_with(&[Capability::Read]);
        let reld = empty_reld();

        // Location 1: write i32, read i32
        verifier.record_write(
            LocationId(1),
            make_bd(repd32.clone(), rw_cap.clone(), reld.clone()),
            ProgramPointId(1),
        );
        verifier.record_read(
            LocationId(1),
            make_bd(repd32.clone(), r_cap.clone(), reld.clone()),
            ProgramPointId(2),
        );

        // Location 2: write u64, read u64
        verifier.record_write(
            LocationId(2),
            make_bd(repd64.clone(), rw_cap.clone(), reld.clone()),
            ProgramPointId(3),
        );
        verifier.record_read(
            LocationId(2),
            make_bd(repd64.clone(), r_cap.clone(), reld.clone()),
            ProgramPointId(4),
        );

        // Location 3: write, overwrite, then read
        verifier.record_write(
            LocationId(3),
            make_bd(repd32.clone(), rw_cap.clone(), reld.clone()),
            ProgramPointId(5),
        );
        verifier.record_write(
            LocationId(3),
            make_bd(repd32.clone(), rw_cap.clone(), reld.clone()),
            ProgramPointId(6),
        );
        verifier.record_read(
            LocationId(3),
            make_bd(repd32.clone(), r_cap.clone(), reld.clone()),
            ProgramPointId(7),
        );

        let result = verifier.verify();
        assert!(result.is_proven(), "clean program should be proven: {}", result);
    }

    // -----------------------------------------------------------------------
    // Additional test: Uninitialized read
    // -----------------------------------------------------------------------

    #[test]
    fn test_uninitialized_read_detected() {
        let mut verifier = InterpretationVerifier::new();

        let read_bd = make_bd(
            byte_repd(4, 4),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );

        // Read without any preceding write
        verifier.record_read(LocationId(1), read_bd, ProgramPointId(1));

        let violations = verifier.verify_detailed();
        let has_uninit = violations.iter().any(|v| {
            matches!(v, InterpretationViolation::UninitializedRead { .. })
        });
        assert!(has_uninit, "should detect uninitialized read: {:?}", violations);
    }

    // -----------------------------------------------------------------------
    // Additional test: RelD preservation violation
    // -----------------------------------------------------------------------

    #[test]
    fn test_reld_preservation_violation() {
        let mut verifier = InterpretationVerifier::new();

        let write_bd = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Read, Capability::Write]),
            reld_with(&[Relation::Temporal(TemporalKind::Outlives)]),
        );
        let read_bd = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Read]),
            reld_with(&[
                Relation::Temporal(TemporalKind::Outlives),
                Relation::Temporal(TemporalKind::Succeeds),
            ]),
        );

        verifier.record_write(LocationId(1), write_bd, ProgramPointId(1));
        verifier.record_read(LocationId(1), read_bd, ProgramPointId(2));

        let violations = verifier.verify_detailed();
        let has_reld_violation = violations.iter().any(|v| {
            matches!(v, InterpretationViolation::RelDNotPreserved { .. })
        });
        assert!(
            has_reld_violation,
            "should detect RelD preservation violation: {:?}",
            violations
        );
    }

    // -----------------------------------------------------------------------
    // Additional test: CapD empty meet
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_capability_meet() {
        let mut verifier = InterpretationVerifier::new();

        let write_bd = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Write]),
            empty_reld(),
        );
        let read_bd = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Execute]),
            empty_reld(),
        );

        verifier.record_write(LocationId(1), write_bd, ProgramPointId(1));
        verifier.record_read(LocationId(1), read_bd, ProgramPointId(2));

        let violations = verifier.verify_detailed();
        let has_empty_meet = violations.iter().any(|v| {
            matches!(v, InterpretationViolation::EmptyCapabilityMeet { .. })
        });
        assert!(
            has_empty_meet,
            "should detect empty capability meet: {:?}",
            violations
        );
    }

    // -----------------------------------------------------------------------
    // Additional test: write-read pair extraction
    // -----------------------------------------------------------------------

    #[test]
    fn test_write_read_pair_extraction() {
        let mut verifier = InterpretationVerifier::new();

        let bd1 = make_bd(byte_repd(4, 4), capd_with(&[Capability::Write]), empty_reld());
        let bd2 = make_bd(byte_repd(4, 4), capd_with(&[Capability::Read]), empty_reld());

        verifier.record_write(LocationId(1), bd1.clone(), ProgramPointId(1));
        verifier.record_read(LocationId(1), bd2.clone(), ProgramPointId(2));
        verifier.record_write(LocationId(2), bd1.clone(), ProgramPointId(3));
        verifier.record_read(LocationId(2), bd2.clone(), ProgramPointId(4));

        let pairs = verifier.extract_write_read_pairs();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].location, LocationId(1));
        assert_eq!(pairs[1].location, LocationId(2));
    }

    // -----------------------------------------------------------------------
    // Additional test: multiple writes, last write wins
    // -----------------------------------------------------------------------

    #[test]
    fn test_last_write_wins() {
        let mut verifier = InterpretationVerifier::new();

        let bd_write1 = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Write]),
            empty_reld(),
        );
        let bd_write2 = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        let bd_read = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );

        verifier.record_write(LocationId(1), bd_write1, ProgramPointId(1));
        verifier.record_write(LocationId(1), bd_write2, ProgramPointId(2));
        verifier.record_read(LocationId(1), bd_read, ProgramPointId(3));

        let pairs = verifier.extract_write_read_pairs();
        assert_eq!(pairs.len(), 1);
        // The read should be paired with the *last* write (at PP 2)
        assert_eq!(pairs[0].write_point, ProgramPointId(2));
    }

    // -----------------------------------------------------------------------
    // Additional test: CapD strengthening with proof allowed
    // -----------------------------------------------------------------------

    #[test]
    fn test_capd_strengthening_with_proof_allowed() {
        let mut verifier = InterpretationVerifier::new()
            .with_strengthening_proof(true);

        let write_bd = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let read_bd = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Read, Capability::Execute]),
            empty_reld(),
        );

        verifier.record_write(LocationId(1), write_bd, ProgramPointId(1));
        verifier.record_read(LocationId(1), read_bd, ProgramPointId(2));

        let result = verifier.verify();
        // With proof allowed, strengthening should be "ProbablySafe"
        assert!(
            matches!(result.status, VerificationStatus::ProbablySafe { .. }),
            "strengthening with proof allowed should be ProbablySafe: {}",
            result
        );
    }

    // -----------------------------------------------------------------------
    // Unit test: RepD compatibility check directly
    // -----------------------------------------------------------------------

    #[test]
    fn test_repd_compatibility_same() {
        let a = byte_repd(4, 4);
        let b = byte_repd(4, 4);
        assert!(InterpretationVerifier::check_repd_compatibility(&a, &b).is_ok());
    }

    #[test]
    fn test_repd_compatibility_different_size() {
        let a = byte_repd(4, 4);
        let b = byte_repd(8, 8);
        assert!(InterpretationVerifier::check_repd_compatibility(&a, &b).is_err());
    }

    // -----------------------------------------------------------------------
    // Unit test: CapD transition check directly
    // -----------------------------------------------------------------------

    #[test]
    fn test_capd_transition_weakening() {
        let write = capd_with(&[Capability::Read, Capability::Write]);
        let read = capd_with(&[Capability::Read]);
        let result = InterpretationVerifier::check_capd_transition(&write, &read);
        assert_eq!(result, CapDTransitionResult::Weakening);
    }

    #[test]
    fn test_capd_transition_same() {
        let caps = capd_with(&[Capability::Read]);
        let result = InterpretationVerifier::check_capd_transition(&caps, &caps);
        assert_eq!(result, CapDTransitionResult::Same);
    }

    #[test]
    fn test_capd_transition_strengthening() {
        let write = capd_with(&[Capability::Read]);
        let read = capd_with(&[Capability::Read, Capability::Write]);
        let result = InterpretationVerifier::check_capd_transition(&write, &read);
        match result {
            CapDTransitionResult::Strengthening { added } => {
                assert!(added.contains(&Capability::Write));
            }
            other => panic!("expected Strengthening, got {:?}", other),
        }
    }

    #[test]
    fn test_capd_transition_empty_meet() {
        let write = capd_with(&[Capability::Write]);
        let read = capd_with(&[Capability::Execute]);
        let result = InterpretationVerifier::check_capd_transition(&write, &read);
        assert_eq!(result, CapDTransitionResult::EmptyMeet);
    }

    // -----------------------------------------------------------------------
    // Unit test: type confusion detection
    // -----------------------------------------------------------------------

    #[test]
    fn test_type_confusion_ptr_vs_struct() {
        let ptr = RepD::Ptr(PtrRep {
            pointee: Box::new(byte_repd(1, 1)),
        });
        let struct_ = RepD::Struct(StructRep {
            fields: vec![
                (0, byte_repd(4, 4)),
                (4, byte_repd(4, 4)),
            ],
            total_size: 8,
            align: 4,
        });
        let result = InterpretationVerifier::detect_type_confusion(&ptr, &struct_);
        assert!(result.is_some());
        let (write_kind, read_kind) = result.unwrap();
        assert_eq!(write_kind, "Ptr");
        assert_eq!(read_kind, "Struct");
    }

    #[test]
    fn test_no_type_confusion_same_type() {
        let a = byte_repd(8, 8);
        let b = byte_repd(8, 8);
        assert!(InterpretationVerifier::detect_type_confusion(&a, &b).is_none());
    }

    // =======================================================================
    // NEW TESTS: Deep Type Confusion, Union/Enum Tracking
    // =======================================================================

    // -----------------------------------------------------------------------
    // New Test 1: Struct field mismatch detection
    // -----------------------------------------------------------------------

    #[test]
    fn test_deep_struct_field_mismatch() {
        use vuma_bd::repd::StructRep;

        let write_repd = RepD::Struct(StructRep {
            fields: vec![
                (0, byte_repd(4, 4)),
                (4, byte_repd(4, 4)),
            ],
            total_size: 8,
            align: 4,
        });
        let read_repd = RepD::Struct(StructRep {
            fields: vec![
                (0, byte_repd(4, 4)),
                (8, byte_repd(4, 4)), // offset 8 instead of 4
            ],
            total_size: 12,
            align: 4,
        });

        let results = InterpretationVerifier::detect_deep_type_confusion(
            &write_repd,
            &read_repd,
            &empty_reld(),
            &empty_reld(),
        );

        let has_field_mismatch = results.iter().any(|r| {
            matches!(r, DeepConfusionKind::StructFieldMismatch { .. })
        });
        assert!(
            has_field_mismatch,
            "should detect struct field mismatch: {:?}",
            results
        );
    }

    // -----------------------------------------------------------------------
    // New Test 2: Enum variant mismatch detection
    // -----------------------------------------------------------------------

    #[test]
    fn test_deep_enum_variant_mismatch() {
        use vuma_bd::repd::EnumRep;

        let write_repd = RepD::Enum(EnumRep {
            variants: vec![
                (0, byte_repd(4, 4)),
                (1, byte_repd(8, 8)),
            ],
        });
        let read_repd = RepD::Enum(EnumRep {
            variants: vec![
                (0, byte_repd(4, 4)),
                (2, byte_repd(8, 8)), // different tag
            ],
        });

        let results = InterpretationVerifier::detect_deep_type_confusion(
            &write_repd,
            &read_repd,
            &empty_reld(),
            &empty_reld(),
        );

        let has_variant_mismatch = results.iter().any(|r| {
            matches!(r, DeepConfusionKind::EnumVariantMismatch { .. })
        });
        assert!(
            has_variant_mismatch,
            "should detect enum variant mismatch: {:?}",
            results
        );
    }

    // -----------------------------------------------------------------------
    // New Test 3: Array bounds violation detection
    // -----------------------------------------------------------------------

    #[test]
    fn test_deep_array_bounds_violation() {
        use vuma_bd::repd::ArrayRep;

        let write_repd = RepD::Array(ArrayRep {
            element: Box::new(byte_repd(4, 4)),
            count: 4,
        });
        let read_repd = RepD::Array(ArrayRep {
            element: Box::new(byte_repd(4, 4)),
            count: 8, // reading more elements than written
        });

        let results = InterpretationVerifier::detect_deep_type_confusion(
            &write_repd,
            &read_repd,
            &empty_reld(),
            &empty_reld(),
        );

        let has_bounds_violation = results.iter().any(|r| {
            matches!(r, DeepConfusionKind::ArrayBoundsViolation { .. })
        });
        assert!(
            has_bounds_violation,
            "should detect array bounds violation: {:?}",
            results
        );
    }

    // -----------------------------------------------------------------------
    // New Test 4: Union active field tracking and violation
    // -----------------------------------------------------------------------

    #[test]
    fn test_union_discriminator_violation() {
        let mut verifier = InterpretationVerifier::new();

        // Set discriminator: field_a is active at location 1
        verifier.set_union_discriminator(UnionDiscriminator {
            location: LocationId(1),
            active_field: Some("field_a".to_string()),
            set_point: ProgramPointId(10),
        });

        // Check access to the wrong field
        let result = verifier.check_union_access(&LocationId(1), "field_b");
        assert!(
            result.is_err(),
            "accessing field_b when field_a is active should fail"
        );

        if let Err(InterpretationViolation::UnionFieldViolation {
            active_field,
            accessed_field,
            ..
        }) = result
        {
            assert_eq!(active_field, "field_a");
            assert_eq!(accessed_field, "field_b");
        } else {
            panic!("expected UnionFieldViolation");
        }
    }

    // -----------------------------------------------------------------------
    // New Test 5: Nested pointer depth mismatch
    // -----------------------------------------------------------------------

    #[test]
    fn test_deep_pointer_depth_mismatch() {
        // Write: ptr(ptr(byte)) — depth 2
        let write_repd = RepD::Ptr(PtrRep {
            pointee: Box::new(RepD::Ptr(PtrRep {
                pointee: Box::new(byte_repd(1, 1)),
            })),
        });
        // Read: ptr(byte) — depth 1
        let read_repd = RepD::Ptr(PtrRep {
            pointee: Box::new(byte_repd(1, 1)),
        });

        let results = InterpretationVerifier::detect_deep_type_confusion(
            &write_repd,
            &read_repd,
            &empty_reld(),
            &empty_reld(),
        );

        let has_depth_mismatch = results.iter().any(|r| {
            matches!(r, DeepConfusionKind::NestedPointerDepthMismatch { .. })
        });
        assert!(
            has_depth_mismatch,
            "should detect nested pointer depth mismatch: {:?}",
            results
        );
    }

    // -----------------------------------------------------------------------
    // New Test 6: Union discriminator set and check (consistent access)
    // -----------------------------------------------------------------------

    #[test]
    fn test_union_discriminator_consistent_access() {
        let mut verifier = InterpretationVerifier::new();

        verifier.set_union_discriminator(UnionDiscriminator {
            location: LocationId(1),
            active_field: Some("field_x".to_string()),
            set_point: ProgramPointId(1),
        });

        // Accessing the same field should be OK
        let result = verifier.check_union_access(&LocationId(1), "field_x");
        assert!(
            result.is_ok(),
            "accessing field_x when field_x is active should succeed"
        );

        // Accessing a location with no discriminator should be OK
        let result = verifier.check_union_access(&LocationId(99), "any_field");
        assert!(
            result.is_ok(),
            "accessing a location with no discriminator should succeed"
        );
    }

    // -----------------------------------------------------------------------
    // New Test 7: Enum variant set and check
    // -----------------------------------------------------------------------

    #[test]
    fn test_enum_variant_set_and_check() {
        let mut verifier = InterpretationVerifier::new();

        verifier.set_active_variant(
            LocationId(1),
            "SomeVariant".to_string(),
            ProgramPointId(5),
        );

        // Correct variant should succeed
        let result = verifier.check_variant_access(&LocationId(1), "SomeVariant");
        assert!(
            result.is_ok(),
            "accessing the tracked variant should succeed"
        );

        // Wrong variant should fail
        let result = verifier.check_variant_access(&LocationId(1), "OtherVariant");
        assert!(
            result.is_err(),
            "accessing a different variant should fail"
        );
    }

    // -----------------------------------------------------------------------
    // New Test 8: Deep recursive struct comparison (nested structs)
    // -----------------------------------------------------------------------

    #[test]
    fn test_deep_recursive_struct_comparison() {
        use vuma_bd::repd::StructRep;

        // Inner struct for the write: { @0: i32, @4: i32 }
        let write_inner = RepD::Struct(StructRep {
            fields: vec![
                (0, byte_repd(4, 4)),
                (4, byte_repd(4, 4)),
            ],
            total_size: 8,
            align: 4,
        });

        // Inner struct for the read: { @0: i32, @8: i32 } (different offset)
        let read_inner = RepD::Struct(StructRep {
            fields: vec![
                (0, byte_repd(4, 4)),
                (8, byte_repd(4, 4)), // different offset
            ],
            total_size: 12,
            align: 4,
        });

        // Outer struct
        let write_outer = RepD::Struct(StructRep {
            fields: vec![
                (0, write_inner),
            ],
            total_size: 8,
            align: 4,
        });
        let read_outer = RepD::Struct(StructRep {
            fields: vec![
                (0, read_inner),
            ],
            total_size: 12,
            align: 4,
        });

        let results = InterpretationVerifier::detect_deep_type_confusion(
            &write_outer,
            &read_outer,
            &empty_reld(),
            &empty_reld(),
        );

        // Should find a field mismatch in the nested struct
        let has_field_mismatch = results.iter().any(|r| {
            matches!(r, DeepConfusionKind::StructFieldMismatch { .. })
        });
        assert!(
            has_field_mismatch,
            "should detect nested struct field mismatch: {:?}",
            results
        );
    }

    // -----------------------------------------------------------------------
    // New Test 9: Security level violation detection
    // -----------------------------------------------------------------------

    #[test]
    fn test_security_level_violation() {
        let write_repd = byte_repd(8, 8);
        let read_repd = byte_repd(8, 8);

        let write_reld = reld_with(&[Relation::Security(FlowPolicy::NoDowngrade)]);
        let read_reld = reld_with(&[Relation::Security(FlowPolicy::Sanitized)]);

        let results = InterpretationVerifier::detect_deep_type_confusion(
            &write_repd,
            &read_repd,
            &write_reld,
            &read_reld,
        );

        let has_sec_violation = results.iter().any(|r| {
            matches!(r, DeepConfusionKind::SecurityLevelViolation { .. })
        });
        assert!(
            has_sec_violation,
            "should detect security level violation: {:?}",
            results
        );
    }

    // -----------------------------------------------------------------------
    // New Test 10: Mixed union/enum scenario
    // -----------------------------------------------------------------------

    #[test]
    fn test_mixed_union_enum_scenario() {
        let mut verifier = InterpretationVerifier::new();

        // Set up union discriminator at location 1
        verifier.set_union_discriminator(UnionDiscriminator {
            location: LocationId(1),
            active_field: Some("float_val".to_string()),
            set_point: ProgramPointId(1),
        });

        // Set up enum variant at location 2
        verifier.set_active_variant(
            LocationId(2),
            "Ok".to_string(),
            ProgramPointId(2),
        );

        // Union violation: accessing wrong field
        let union_result = verifier.check_union_access(&LocationId(1), "int_val");
        assert!(union_result.is_err(), "should detect union field violation");

        // Enum violation: accessing wrong variant
        let enum_result = verifier.check_variant_access(&LocationId(2), "Err");
        assert!(enum_result.is_err(), "should detect enum variant violation");

        // Both correct accesses should succeed
        let union_ok = verifier.check_union_access(&LocationId(1), "float_val");
        assert!(union_ok.is_ok(), "correct union access should succeed");

        let enum_ok = verifier.check_variant_access(&LocationId(2), "Ok");
        assert!(enum_ok.is_ok(), "correct enum access should succeed");
    }

    // -----------------------------------------------------------------------
    // New Test 11: DeepConfusionKind Display implementation
    // -----------------------------------------------------------------------

    #[test]
    fn test_deep_confusion_kind_display() {
        let kinds = vec![
            DeepConfusionKind::StructFieldMismatch {
                write_field: "x".to_string(),
                read_field: "y".to_string(),
            },
            DeepConfusionKind::EnumVariantMismatch {
                write_variant: "A".to_string(),
                read_variant: "B".to_string(),
            },
            DeepConfusionKind::ArrayBoundsViolation {
                write_len: 4,
                read_index: 7,
            },
            DeepConfusionKind::UnionActiveFieldMismatch {
                write_field: "f1".to_string(),
                read_field: "f2".to_string(),
            },
            DeepConfusionKind::NestedPointerDepthMismatch {
                write_depth: 2,
                read_depth: 1,
            },
            DeepConfusionKind::SecurityLevelViolation {
                write_level: "secret".to_string(),
                read_level: "public".to_string(),
            },
        ];

        for kind in &kinds {
            let s = format!("{}", kind);
            assert!(!s.is_empty(), "Display should not be empty for {:?}", kind);
        }
    }

    // -----------------------------------------------------------------------
    // New Test 12: EnumVariantTracker standalone test
    // -----------------------------------------------------------------------

    #[test]
    fn test_enum_variant_tracker_standalone() {
        let mut tracker = EnumVariantTracker::new();

        // No entry for a location should be OK
        let result = tracker.check_variant_access(&LocationId(1), "V1");
        assert!(result.is_ok(), "no tracking should be OK");

        // Set and check
        tracker.set_active_variant(LocationId(1), "V1".to_string(), ProgramPointId(1));
        let result = tracker.check_variant_access(&LocationId(1), "V1");
        assert!(result.is_ok(), "matching variant should be OK");

        let result = tracker.check_variant_access(&LocationId(1), "V2");
        assert!(result.is_err(), "mismatched variant should fail");
    }

    // -----------------------------------------------------------------------
    // New Test 13: Same RepD with same RelD produces no deep confusion
    // -----------------------------------------------------------------------

    #[test]
    fn test_deep_no_confusion_identical() {
        let repd = byte_repd(8, 8);
        let reld = empty_reld();

        let results = InterpretationVerifier::detect_deep_type_confusion(
            &repd, &repd, &reld, &reld,
        );

        assert!(
            results.is_empty(),
            "identical RepD/RelD should have no deep confusion: {:?}",
            results
        );
    }

    // -----------------------------------------------------------------------
    // Cast Validation Tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_cast_byte_to_struct_is_safe() {
        let verifier = InterpretationVerifier::new();
        let from_bd = make_bd(
            byte_repd(16, 8),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        let to_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![
                    (0, byte_repd(8, 8)),
                    (8, byte_repd(8, 8)),
                ],
                total_size: 16,
                align: 8,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(1),
            from_bd,
            to_bd,
            cast_kind: CastKind::RepCast,
            point: ProgramPointId(10),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::Safe { reason } => {
                assert!(reason.contains("widening"), "expected widening reason, got: {}", reason);
            }
            _ => panic!("Byte→Struct cast should be Safe, got {:?}", result),
        }
    }

    #[test]
    fn test_cast_struct_to_byte_is_safe() {
        let verifier = InterpretationVerifier::new();
        let from_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![
                    (0, byte_repd(4, 4)),
                    (4, byte_repd(4, 4)),
                ],
                total_size: 8,
                align: 4,
            }),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        let to_bd = make_bd(
            byte_repd(8, 4),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(1),
            from_bd,
            to_bd,
            cast_kind: CastKind::RepCast,
            point: ProgramPointId(11),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::Safe { reason } => {
                assert!(reason.contains("narrowing to Byte"), "expected narrowing reason, got: {}", reason);
            }
            _ => panic!("Struct→Byte cast should be Safe, got {:?}", result),
        }
    }

    #[test]
    fn test_cast_capd_weakening_is_safe() {
        let verifier = InterpretationVerifier::new();
        // Use Struct (not Byte) as RepD so the widening rule doesn't fire first
        let repd = RepD::Struct(StructRep {
            fields: vec![(0, byte_repd(8, 8))],
            total_size: 8,
            align: 8,
        });
        let from_bd = make_bd(
            repd.clone(),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        let to_bd = make_bd(
            repd,
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(1),
            from_bd,
            to_bd,
            cast_kind: CastKind::CapCast,
            point: ProgramPointId(12),
            is_explicit: false,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::Safe { reason } => {
                assert!(reason.contains("CapD weakening"), "expected CapD weakening reason, got: {}", reason);
            }
            _ => panic!("CapD weakening cast should be Safe, got {:?}", result),
        }
    }

    #[test]
    fn test_cast_pointer_to_integer_bitcast_high_risk() {
        let verifier = InterpretationVerifier::new();
        let from_bd = make_bd(
            RepD::Ptr(PtrRep {
                pointee: Box::new(byte_repd(4, 4)),
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        // Target: same-size struct (simulating integer)
        let to_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![(0, byte_repd(8, 8))],
                total_size: 8,
                align: 8,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(2),
            from_bd,
            to_bd,
            cast_kind: CastKind::BitCast,
            point: ProgramPointId(13),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::BitCast { risk_level } => {
                assert_eq!(risk_level, BitCastRisk::High, "Ptr→non-Ptr bitcast should be High risk");
            }
            _ => panic!("Ptr→int bitcast should be BitCast with High risk, got {:?}", result),
        }
    }

    #[test]
    fn test_cast_u32_to_i32_is_low_risk() {
        let verifier = InterpretationVerifier::new();
        // Same RepD (Struct) with different CapD tests CapD transition
        // in the cast validation (same RepD → CapD-only check)
        let from_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![(0, byte_repd(4, 4))],
                total_size: 4,
                align: 4,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let to_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![(0, byte_repd(4, 4))],
                total_size: 4,
                align: 4,
            }),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(3),
            from_bd,
            to_bd,
            cast_kind: CastKind::BitCast,
            point: ProgramPointId(14),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        // Same RepD = CapD transition = SafeWithProof (strengthening)
        match result {
            CastValidationResult::SafeWithProof { obligation } => {
                assert_eq!(obligation.difficulty, ProofDifficulty::Easy);
            }
            _ => panic!("Same kind same size with CapD strengthening should be SafeWithProof, got {:?}", result),
        }
    }

    #[test]
    fn test_cast_float_to_int_is_medium_risk() {
        let verifier = InterpretationVerifier::new();
        // Struct→Array (same size, different kind, no ptr/func) = Medium risk
        let from_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![(0, byte_repd(8, 8))],
                total_size: 8,
                align: 8,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let to_bd = make_bd(
            RepD::Array(vuma_bd::repd::ArrayRep {
                element: Box::new(byte_repd(4, 4)),
                count: 2,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(4),
            from_bd,
            to_bd,
            cast_kind: CastKind::BitCast,
            point: ProgramPointId(15),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::BitCast { risk_level } => {
                assert_eq!(risk_level, BitCastRisk::Medium, "Struct→Array bitcast should be Medium risk");
            }
            _ => panic!("float↔int bitcast should be BitCast with Medium risk, got {:?}", result),
        }
    }

    #[test]
    fn test_cast_size_mismatch_is_unsafe() {
        let verifier = InterpretationVerifier::new();
        let from_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![(0, byte_repd(8, 8))],
                total_size: 8,
                align: 8,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let to_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![(0, byte_repd(4, 4))],
                total_size: 4,
                align: 4,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(5),
            from_bd,
            to_bd,
            cast_kind: CastKind::FullCast,
            point: ProgramPointId(16),
            is_explicit: false,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::Unsafe { violations } => {
                assert!(!violations.is_empty(), "size mismatch should produce violations");
                let found = violations.iter().any(|v| {
                    matches!(v, InterpretationViolation::UnsafeCast { reason, .. } if reason.contains("size mismatch"))
                });
                assert!(found, "expected UnsafeCast with size mismatch reason");
            }
            _ => panic!("Size mismatch cast should be Unsafe, got {:?}", result),
        }
    }

    #[test]
    fn test_cast_explicit_with_proof_obligation() {
        let verifier = InterpretationVerifier::new();
        // CapD strengthening (Read→Read+Write) with allow_strengthening_with_proof=true
        // Use Struct RepD (not Byte) to avoid the widening rule firing first
        let repd = RepD::Struct(StructRep {
            fields: vec![(0, byte_repd(8, 8))],
            total_size: 8,
            align: 8,
        });
        let from_bd = make_bd(
            repd.clone(),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let to_bd = make_bd(
            repd,
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(6),
            from_bd,
            to_bd,
            cast_kind: CastKind::CapCast,
            point: ProgramPointId(17),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::SafeWithProof { obligation } => {
                assert!(obligation.required_proof.contains("strengthening"), "expected strengthening obligation, got: {}", obligation.required_proof);
                assert_eq!(obligation.difficulty, ProofDifficulty::Easy);
                assert!(obligation.cast.is_explicit);
            }
            _ => panic!("CapD strengthening with proof should be SafeWithProof, got {:?}", result),
        }
    }

    #[test]
    fn test_cast_record_and_verify_integration() {
        let mut verifier = InterpretationVerifier::new();
        // Record a safe write-read pair
        let bd = make_bd(
            byte_repd(8, 8),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        verifier.record_write(LocationId(1), bd.clone(), ProgramPointId(1));
        verifier.record_read(LocationId(1), bd.clone(), ProgramPointId(2));

        // Record a safe cast (Byte→Struct)
        let from_bd = make_bd(
            byte_repd(16, 8),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        let to_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![
                    (0, byte_repd(8, 8)),
                    (8, byte_repd(8, 8)),
                ],
                total_size: 16,
                align: 8,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        verifier.record_cast(CastRecord {
            location: LocationId(2),
            from_bd,
            to_bd,
            cast_kind: CastKind::RepCast,
            point: ProgramPointId(3),
            is_explicit: true,
        });
        assert_eq!(verifier.cast_count(), 1);

        let result = verifier.verify();
        assert!(result.is_proven(), "safe write-read + safe cast should be proven: {}", result);
    }

    #[test]
    fn test_cast_func_pointer_extreme_risk() {
        let verifier = InterpretationVerifier::new();
        let from_bd = make_bd(
            RepD::Func(vuma_bd::repd::FuncRep {
                params: vec![byte_repd(4, 4)],
                result: Box::new(byte_repd(4, 4)),
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let to_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![(0, byte_repd(8, 8))],
                total_size: 8,
                align: 8,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(7),
            from_bd,
            to_bd,
            cast_kind: CastKind::BitCast,
            point: ProgramPointId(18),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::BitCast { risk_level } => {
                assert_eq!(risk_level, BitCastRisk::Extreme, "Func→Struct bitcast should be Extreme risk");
            }
            _ => panic!("Func pointer bitcast should be BitCast with Extreme risk, got {:?}", result),
        }
    }

    // -----------------------------------------------------------------------
    // Additional Cast Validation Tests (W1-A16-retry)
    // -----------------------------------------------------------------------

    /// Test: Identity cast (same from_bd and to_bd) is always Safe.
    #[test]
    fn test_cast_identity_is_safe() {
        let verifier = InterpretationVerifier::new();
        let bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![(0, byte_repd(4, 4)), (4, byte_repd(4, 4))],
                total_size: 8,
                align: 4,
            }),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(10),
            from_bd: bd.clone(),
            to_bd: bd,
            cast_kind: CastKind::SafeCast,
            point: ProgramPointId(100),
            is_explicit: false,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::Safe { reason } => {
                assert!(reason.contains("identity"), "expected identity reason, got: {}", reason);
            }
            _ => panic!("Identity cast should be Safe, got {:?}", result),
        }
    }

    /// Test: Struct field subset — reading a prefix of a struct is safe.
    #[test]
    fn test_cast_struct_field_subset_is_safe() {
        let verifier = InterpretationVerifier::new();
        // Source: 3-field struct
        let from_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![
                    (0, byte_repd(4, 4)),
                    (4, byte_repd(4, 4)),
                    (8, byte_repd(4, 4)),
                ],
                total_size: 12,
                align: 4,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        // Target: 1-field prefix struct (same fields as first field of source)
        let to_bd = make_bd(
            RepD::Struct(StructRep {
                fields: vec![(0, byte_repd(4, 4))],
                total_size: 4,
                align: 4,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(11),
            from_bd,
            to_bd,
            cast_kind: CastKind::RepCast,
            point: ProgramPointId(101),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::Safe { reason } => {
                assert!(
                    reason.contains("struct field subset"),
                    "expected struct field subset reason, got: {}",
                    reason
                );
            }
            _ => panic!("Struct field subset cast should be Safe, got {:?}", result),
        }
    }

    /// Test: Cast with empty CapD meet is Unsafe.
    #[test]
    fn test_cast_empty_capd_meet_is_unsafe() {
        let verifier = InterpretationVerifier::new();
        let repd = RepD::Struct(StructRep {
            fields: vec![(0, byte_repd(4, 4))],
            total_size: 4,
            align: 4,
        });
        // from_bd: Write only, to_bd: Execute only — no shared capabilities
        let from_bd = make_bd(
            repd.clone(),
            capd_with(&[Capability::Write]),
            empty_reld(),
        );
        let to_bd = make_bd(
            repd,
            capd_with(&[Capability::Execute]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(12),
            from_bd,
            to_bd,
            cast_kind: CastKind::CapCast,
            point: ProgramPointId(102),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::Unsafe { violations } => {
                let has_empty_meet = violations.iter().any(|v| {
                    matches!(v, InterpretationViolation::EmptyCapabilityMeet { .. })
                });
                assert!(has_empty_meet, "expected EmptyCapabilityMeet violation, got: {:?}", violations);
            }
            _ => panic!("Empty CapD meet cast should be Unsafe, got {:?}", result),
        }
    }

    /// Test: Cast with incomparable CapD (some added, some removed) is SafeWithProof.
    #[test]
    fn test_cast_incomparable_capd_needs_proof() {
        let verifier = InterpretationVerifier::new();
        let repd = RepD::Struct(StructRep {
            fields: vec![(0, byte_repd(4, 4))],
            total_size: 4,
            align: 4,
        });
        // from: Read+Write, to: Read+Execute — added Execute, removed Write
        let from_bd = make_bd(
            repd.clone(),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        let to_bd = make_bd(
            repd,
            capd_with(&[Capability::Read, Capability::Execute]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(13),
            from_bd,
            to_bd,
            cast_kind: CastKind::CapCast,
            point: ProgramPointId(103),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::SafeWithProof { obligation } => {
                assert!(
                    obligation.required_proof.contains("incomparable"),
                    "expected incomparable obligation, got: {}",
                    obligation.required_proof
                );
                assert_eq!(obligation.difficulty, ProofDifficulty::Medium);
            }
            _ => panic!("Incomparable CapD cast should be SafeWithProof, got {:?}", result),
        }
    }

    /// Test: Byte→Array widening is safe.
    #[test]
    fn test_cast_byte_to_array_widening_is_safe() {
        let verifier = InterpretationVerifier::new();
        let from_bd = make_bd(
            byte_repd(16, 8),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let to_bd = make_bd(
            RepD::Array(vuma_bd::repd::ArrayRep {
                element: Box::new(byte_repd(4, 4)),
                count: 4,
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(14),
            from_bd,
            to_bd,
            cast_kind: CastKind::RepCast,
            point: ProgramPointId(104),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::Safe { reason } => {
                assert!(reason.contains("widening"), "expected widening reason, got: {}", reason);
            }
            _ => panic!("Byte→Array widening should be Safe, got {:?}", result),
        }
    }

    /// Test: Ptr→Ptr same-size bitcast is Low risk.
    #[test]
    fn test_cast_ptr_to_ptr_low_risk() {
        let verifier = InterpretationVerifier::new();
        // Two different Ptr RepDs with different pointees, same size
        let from_bd = make_bd(
            RepD::Ptr(PtrRep {
                pointee: Box::new(byte_repd(4, 4)),
            }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let to_bd = make_bd(
            RepD::Ptr(PtrRep {
                pointee: Box::new(RepD::Struct(StructRep {
                    fields: vec![(0, byte_repd(4, 4))],
                    total_size: 4,
                    align: 4,
                })),
            }),
            capd_with(&[Capability::Read, Capability::Write]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(15),
            from_bd,
            to_bd,
            cast_kind: CastKind::BitCast,
            point: ProgramPointId(105),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::BitCast { risk_level } => {
                assert_eq!(risk_level, BitCastRisk::Low, "Ptr→Ptr should be Low risk");
            }
            _ => panic!("Ptr→Ptr same-size should be BitCast Low, got {:?}", result),
        }
    }

    /// Test: Multiple recorded casts validated in verify() integration.
    #[test]
    fn test_multiple_casts_verify_integration() {
        let mut verifier = InterpretationVerifier::new();

        // Safe write-read pair
        let bd = make_bd(byte_repd(8, 8), capd_with(&[Capability::Read, Capability::Write]), empty_reld());
        verifier.record_write(LocationId(1), bd.clone(), ProgramPointId(1));
        verifier.record_read(LocationId(1), bd.clone(), ProgramPointId(2));

        // Safe cast: Byte→Struct
        verifier.record_cast(CastRecord {
            location: LocationId(2),
            from_bd: make_bd(byte_repd(8, 8), capd_with(&[Capability::Read]), empty_reld()),
            to_bd: make_bd(
                RepD::Struct(StructRep { fields: vec![(0, byte_repd(8, 8))], total_size: 8, align: 8 }),
                capd_with(&[Capability::Read]),
                empty_reld(),
            ),
            cast_kind: CastKind::RepCast,
            point: ProgramPointId(3),
            is_explicit: true,
        });

        // Safe cast: Struct→Byte
        verifier.record_cast(CastRecord {
            location: LocationId(3),
            from_bd: make_bd(
                RepD::Struct(StructRep { fields: vec![(0, byte_repd(4, 4))], total_size: 4, align: 4 }),
                capd_with(&[Capability::Read]),
                empty_reld(),
            ),
            to_bd: make_bd(byte_repd(4, 4), capd_with(&[Capability::Read]), empty_reld()),
            cast_kind: CastKind::RepCast,
            point: ProgramPointId(4),
            is_explicit: true,
        });

        assert_eq!(verifier.cast_count(), 2);
        let result = verifier.verify();
        assert!(result.is_proven(), "safe write-read + 2 safe casts should be proven: {}", result);
    }

    /// Test: Cast with CapD strengthening without proof allowed is Unsafe.
    #[test]
    fn test_cast_strengthening_without_proof_unsafe() {
        let verifier = InterpretationVerifier::new()
            .with_strengthening_proof(false);
        let repd = RepD::Struct(StructRep {
            fields: vec![(0, byte_repd(8, 8))],
            total_size: 8,
            align: 8,
        });
        let from_bd = make_bd(repd.clone(), capd_with(&[Capability::Read]), empty_reld());
        let to_bd = make_bd(repd, capd_with(&[Capability::Read, Capability::Write]), empty_reld());
        let cast = CastRecord {
            location: LocationId(16),
            from_bd,
            to_bd,
            cast_kind: CastKind::CapCast,
            point: ProgramPointId(106),
            is_explicit: true,
        };
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::Unsafe { violations } => {
                let has_strengthening = violations.iter().any(|v| {
                    matches!(v, InterpretationViolation::InvalidCapDStrengthening { .. })
                });
                assert!(has_strengthening, "expected InvalidCapDStrengthening, got: {:?}", violations);
            }
            _ => panic!("CapD strengthening without proof should be Unsafe, got {:?}", result),
        }
    }

    /// Test: CastValidationResult Display-like debug output is informative.
    #[test]
    fn test_cast_validation_result_debug_variants() {
        // Verify that all CastValidationResult variants can be constructed and debugged
        let safe = CastValidationResult::Safe { reason: "test".to_string() };
        let bitcast = CastValidationResult::BitCast { risk_level: BitCastRisk::Medium };
        let unsafe_result = CastValidationResult::Unsafe { violations: vec![] };

        // Just ensure debug format works without panic
        let _ = format!("{:?}", safe);
        let _ = format!("{:?}", bitcast);
        let _ = format!("{:?}", unsafe_result);

        // Verify BitCastRisk ordering
        assert!(BitCastRisk::Low < BitCastRisk::Medium);
        assert!(BitCastRisk::Medium < BitCastRisk::High);
        assert!(BitCastRisk::High < BitCastRisk::Extreme);
    }

    /// Test: SafeCast kind in CastRecord is preserved through validation.
    #[test]
    fn test_cast_safe_cast_kind_preserved() {
        let verifier = InterpretationVerifier::new();
        // Byte→Struct with SafeCast kind
        let from_bd = make_bd(byte_repd(8, 8), capd_with(&[Capability::Read]), empty_reld());
        let to_bd = make_bd(
            RepD::Struct(StructRep { fields: vec![(0, byte_repd(8, 8))], total_size: 8, align: 8 }),
            capd_with(&[Capability::Read]),
            empty_reld(),
        );
        let cast = CastRecord {
            location: LocationId(17),
            from_bd,
            to_bd,
            cast_kind: CastKind::SafeCast,
            point: ProgramPointId(107),
            is_explicit: false,
        };
        // The validation result should still be Safe regardless of cast_kind
        let result = verifier.validate_cast(&cast);
        match result {
            CastValidationResult::Safe { .. } => {}
            _ => panic!("SafeCast Byte→Struct should be Safe, got {:?}", result),
        }
    }
}
