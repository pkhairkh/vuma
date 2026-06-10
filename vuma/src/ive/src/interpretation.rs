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

use std::fmt;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// Opaque identifier for a memory location (region + offset).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LocationId(pub u64);

impl fmt::Display for LocationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "loc#{}", self.0)
    }
}

/// Opaque identifier for a program point in the SCG.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
}

impl InterpretationVerifier {
    /// Construct a new interpretation verifier.
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            allow_strengthening_with_proof: true,
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

    /// Clear all recorded events.
    pub fn clear(&mut self) {
        self.events.clear();
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
            if !write_repd.alignment().is_multiple_of(read_repd.alignment()) {
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
                | Some(InterpretationViolation::UninitializedRead { read_point, .. }) => {
                    read_point.to_string()
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
    use vuma_bd::reld::TemporalKind;
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
}
