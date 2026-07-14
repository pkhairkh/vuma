//! # Serialization I/O for Proof Objects
//!
//! Provides the hand-written binary codec for the core [`Proof`] artifact and
//! every type it (transitively) references. The former JSON `ProofEnvelope`
//! path (Wave ≤ 42) was removed in Wave 43 when `Serialize, Deserialize`
//! derives were stripped from `Proof` / `Judgment` / `Fact` / `ProofStep`
//! (and their transitive containers `CleanupProof`, `LivenessProof`,
//! `ExclusivityProof`, `OriginProof`, `InterpretationProof`, `ProofBundle`).
//! See the `ProofEnvelope` definition below for the retained in-memory
//! tagged-sum type.

use crate::cleanup_proofs::CleanupProof;
use crate::exclusivity_proofs::ExclusivityProof;
use crate::interpretation_proofs::InterpretationProof;
use crate::liveness_proofs::LivenessProof;
use crate::origin_proofs::OriginProof;
use crate::proof::Proof;

// Note (Wave 43): the `SerializationError` enum (formerly wrapping
// `serde_json::Error` and `std::io::Error`) was removed along with the
// `ProofEnvelope` JSON helpers. The hand-written binary codec below uses
// `BinaryError` for its error type. Callers needing JSON I/O should
// construct a serde-derived peripheral DTO from the `Proof` fields.

// ── ProofEnvelope (Wave 43: JSON path removed) ─────────────────────────
//
// `ProofEnvelope` was previously `#[derive(serde::Serialize, serde::Deserialize)]`
// with JSON helpers (`to_json_string`, `from_json_string`, `to_json_string_pretty`,
// `to_writer`, `from_reader`). These were removed when `Serialize, Deserialize`
// derives were stripped from `Proof` / `Judgment` / `Fact` / `ProofStep` (and
// their transitive containers `CleanupProof`, `LivenessProof`, `ExclusivityProof`,
// `OriginProof`, `InterpretationProof`, `ProofBundle`). The `ProofEnvelope`
// type itself is retained as a tagged sum type for in-memory dispatch; the
// hand-written binary codec (`serialize_proof` / `deserialize_proof` /
// `BinaryWrite` / `BinaryRead` below) is now the canonical (de)serialization
// path. Callers needing JSON output should construct a serde-derived
// peripheral DTO from the `Proof` fields and serialize that instead.

/// A serializable proof envelope that can hold any proof type.
///
/// Formerly serde-derived with an internally-tagged JSON representation
// (#[serde(tag = "type", content = "data")]). The serde derive and JSON
// helpers were removed in Wave 43 (see module comment above). The type is
// retained as an in-memory tagged sum.
#[allow(clippy::large_enum_variant)]
pub enum ProofEnvelope {
    Liveness(LivenessProof),
    Exclusivity(ExclusivityProof),
    Cleanup(CleanupProof),
    Origin(OriginProof),
    Interpretation(InterpretationProof),
    Generic(Proof),
}

// ---------------------------------------------------------------------------
// From<XXXProof> for ProofEnvelope
// ---------------------------------------------------------------------------

impl From<LivenessProof> for ProofEnvelope {
    fn from(proof: LivenessProof) -> Self {
        ProofEnvelope::Liveness(proof)
    }
}

impl From<ExclusivityProof> for ProofEnvelope {
    fn from(proof: ExclusivityProof) -> Self {
        ProofEnvelope::Exclusivity(proof)
    }
}

impl From<CleanupProof> for ProofEnvelope {
    fn from(proof: CleanupProof) -> Self {
        ProofEnvelope::Cleanup(proof)
    }
}

impl From<OriginProof> for ProofEnvelope {
    fn from(proof: OriginProof) -> Self {
        ProofEnvelope::Origin(proof)
    }
}

impl From<InterpretationProof> for ProofEnvelope {
    fn from(proof: InterpretationProof) -> Self {
        ProofEnvelope::Interpretation(proof)
    }
}

impl From<Proof> for ProofEnvelope {
    fn from(proof: Proof) -> Self {
        ProofEnvelope::Generic(proof)
    }
}

// ===========================================================================
// Hand-written binary (de)serialization — Wave 43
// ===========================================================================
//
// Pure-Rust, `serde`-free binary (de)serialization for the core [`Proof`]
// artifact and every type it (transitively) references. This is the minimal
// Wave 43 deliverable for the proof crate: a complete hand-written binary
// path for the recursive proof tree (`Proof` → `ProofStep` → nested
// `Proof`s for `CaseSplit` / `Induction`), plus the supporting types
// (`Goal`, `Target`, `ProofContext`, `Fact`, `Judgment`, `InferenceRule`,
// `Conclusion`, `InvariantName`, `FactKind`, `CapDKind`, and the typed-id
// newtypes).
//
// The `serde` derives on these types are **left in place** for now (the JSON
// `ProofEnvelope` path above still uses them); full feature-gating of every
// derive site in the proof crate is deferred (see worklog `5-a`). Consumers
// that want a `serde`-free binary path can use [`serialize_proof`] /
// [`deserialize_proof`]; consumers that want JSON can use the existing
// `ProofEnvelope` API.
//
// # Binary Format (Version 1, little-endian)
//
// ```text
// [4B]  Magic: "VPRF"
// [4B]  Version: u32 LE  (currently 1)
// --- payload (Proof) ---
// ```
//
// All integers are little-endian. Enums use a `u8` discriminant. `Vec<T>`
// is length-prefixed with a `u32` LE count. `Option<T>` is a `u8` tag
// (`0 = None`, `1 = Some(T)`). `String` is a `u32` LE byte-length prefix
// followed by the UTF-8 bytes. `Box<Proof>` serializes as the inner `Proof`.

use crate::judgment::{
    CapDKind, EventId, Judgment, PointerId, RegionId, ResourceId, VariableId,
};
use crate::proof::{
    AccessId, Conclusion, DerivationId, Fact, FactId, FactKind, Goal, InvariantName,
    ProofContext, ProofStep, Target,
};
use crate::rules::InferenceRule;
use std::fmt;
use std::io::{self, Read, Write};

/// Magic bytes identifying the VUMA proof binary format.
const PROOF_MAGIC: &[u8; 4] = b"VPRF";

/// Current proof binary format version.
const PROOF_VERSION: u32 = 1;

/// Error returned by [`deserialize_proof`] and the `BinaryRead` trait.
#[derive(Debug)]
pub enum BinaryError {
    /// Underlying I/O failure (unexpected EOF, etc.).
    Io(io::Error),
    /// The byte stream contained an invalid enum discriminant or malformed
    /// payload (e.g. a bad magic header, unknown version, or out-of-range
    /// count).
    InvalidData(String),
}

impl fmt::Display for BinaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BinaryError::Io(e) => write!(f, "binary I/O error: {e}"),
            BinaryError::InvalidData(msg) => {
                write!(f, "invalid proof binary data: {msg}")
            }
        }
    }
}

impl std::error::Error for BinaryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BinaryError::Io(e) => Some(e),
            BinaryError::InvalidData(_) => None,
        }
    }
}

impl From<io::Error> for BinaryError {
    fn from(e: io::Error) -> Self {
        BinaryError::Io(e)
    }
}

/// Serialize a value to a writer using the VUMA proof binary format.
pub trait BinaryWrite {
    fn write_binary<W: Write>(&self, writer: &mut W) -> Result<(), BinaryError>;
}

/// Deserialize a value from a reader using the VUMA proof binary format.
pub trait BinaryRead: Sized {
    fn read_binary<R: Read>(reader: &mut R) -> Result<Self, BinaryError>;
}

// ── Primitive helpers ───────────────────────────────────────────────────

#[inline]
fn write_u8<W: Write>(w: &mut W, v: u8) -> Result<(), BinaryError> {
    w.write_all(&[v]).map_err(BinaryError::Io)
}

#[inline]
fn read_u8<R: Read>(r: &mut R) -> Result<u8, BinaryError> {
    let mut buf = [0u8; 1];
    r.read_exact(&mut buf).map_err(BinaryError::Io)?;
    Ok(buf[0])
}

#[inline]
fn write_u32<W: Write>(w: &mut W, v: u32) -> Result<(), BinaryError> {
    w.write_all(&v.to_le_bytes()).map_err(BinaryError::Io)
}

#[inline]
fn read_u32<R: Read>(r: &mut R) -> Result<u32, BinaryError> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf).map_err(BinaryError::Io)?;
    Ok(u32::from_le_bytes(buf))
}

#[inline]
fn write_u64<W: Write>(w: &mut W, v: u64) -> Result<(), BinaryError> {
    w.write_all(&v.to_le_bytes()).map_err(BinaryError::Io)
}

#[inline]
fn read_u64<R: Read>(r: &mut R) -> Result<u64, BinaryError> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf).map_err(BinaryError::Io)?;
    Ok(u64::from_le_bytes(buf))
}

#[inline]
fn write_i64<W: Write>(w: &mut W, v: i64) -> Result<(), BinaryError> {
    w.write_all(&v.to_le_bytes()).map_err(BinaryError::Io)
}

#[inline]
fn read_i64<R: Read>(r: &mut R) -> Result<i64, BinaryError> {
    let mut buf = [0u8; 8];
    r.read_exact(&mut buf).map_err(BinaryError::Io)?;
    Ok(i64::from_le_bytes(buf))
}

#[inline]
fn write_usize_as_u32<W: Write>(w: &mut W, v: usize) -> Result<(), BinaryError> {
    write_u32(w, u32::try_from(v).map_err(|_| {
        BinaryError::InvalidData(format!("usize {v} overflows u32 length prefix"))
    })?)
}

fn write_string<W: Write>(w: &mut W, s: &str) -> Result<(), BinaryError> {
    let bytes = s.as_bytes();
    write_usize_as_u32(w, bytes.len())?;
    w.write_all(bytes).map_err(BinaryError::Io)
}

fn read_string<R: Read>(r: &mut R) -> Result<String, BinaryError> {
    let len = read_u32(r)? as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).map_err(BinaryError::Io)?;
    String::from_utf8(buf).map_err(|e| {
        BinaryError::InvalidData(format!("invalid UTF-8 in string field: {e}"))
    })
}

fn write_opt<T: BinaryWrite, W: Write>(
    w: &mut W,
    opt: &Option<T>,
) -> Result<(), BinaryError> {
    match opt {
        None => write_u8(w, 0),
        Some(v) => {
            write_u8(w, 1)?;
            v.write_binary(w)
        }
    }
}

fn read_opt<T: BinaryRead, R: Read>(r: &mut R) -> Result<Option<T>, BinaryError> {
    match read_u8(r)? {
        0 => Ok(None),
        1 => Ok(Some(T::read_binary(r)?)),
        other => Err(BinaryError::InvalidData(format!(
            "invalid Option tag: {other}"
        ))),
    }
}

fn write_vec<T: BinaryWrite, W: Write>(
    w: &mut W,
    v: &[T],
) -> Result<(), BinaryError> {
    write_usize_as_u32(w, v.len())?;
    for item in v {
        item.write_binary(w)?;
    }
    Ok(())
}

fn read_vec<T: BinaryRead, R: Read>(r: &mut R) -> Result<Vec<T>, BinaryError> {
    let len = read_u32(r)? as usize;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(T::read_binary(r)?);
    }
    Ok(out)
}

fn write_box<T: BinaryWrite, W: Write>(w: &mut W, b: &Box<T>) -> Result<(), BinaryError> {
    b.as_ref().write_binary(w)
}

fn read_box<T: BinaryRead, R: Read>(r: &mut R) -> Result<Box<T>, BinaryError> {
    Ok(Box::new(T::read_binary(r)?))
}

// ── Typed ID newtypes ───────────────────────────────────────────────────
//
// These are all `pub struct Foo(pub u64)` tuple newtypes. We write the inner
// `u64` and reconstruct via the tuple constructor on read.

impl BinaryWrite for RegionId {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_u64(w, self.0)
    }
}
impl BinaryRead for RegionId {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        Ok(RegionId(read_u64(r)?))
    }
}

impl BinaryWrite for ResourceId {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_u64(w, self.0)
    }
}
impl BinaryRead for ResourceId {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        Ok(ResourceId(read_u64(r)?))
    }
}

impl BinaryWrite for PointerId {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_u64(w, self.0)
    }
}
impl BinaryRead for PointerId {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        Ok(PointerId(read_u64(r)?))
    }
}

impl BinaryWrite for VariableId {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_u64(w, self.0)
    }
}
impl BinaryRead for VariableId {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        Ok(VariableId(read_u64(r)?))
    }
}

impl BinaryWrite for EventId {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_u64(w, self.0)
    }
}
impl BinaryRead for EventId {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        Ok(EventId(read_u64(r)?))
    }
}

// ── CapDKind ────────────────────────────────────────────────────────────

impl BinaryWrite for CapDKind {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        let tag: u8 = match self {
            CapDKind::Read => 0,
            CapDKind::Write => 1,
            CapDKind::ReadWrite => 2,
            CapDKind::Execute => 3,
        };
        write_u8(w, tag)
    }
}

impl BinaryRead for CapDKind {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(CapDKind::Read),
            1 => Ok(CapDKind::Write),
            2 => Ok(CapDKind::ReadWrite),
            3 => Ok(CapDKind::Execute),
            other => Err(BinaryError::InvalidData(format!(
                "invalid CapDKind discriminant: {other}"
            ))),
        }
    }
}

// ── Judgment ────────────────────────────────────────────────────────────

impl BinaryWrite for Judgment {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        match self {
            Judgment::Allocated { region } => {
                write_u8(w, 0)?;
                region.write_binary(w)?;
            }
            Judgment::Live { region } => {
                write_u8(w, 1)?;
                region.write_binary(w)?;
            }
            Judgment::Freed { region } => {
                write_u8(w, 2)?;
                region.write_binary(w)?;
            }
            Judgment::Dead { region } => {
                write_u8(w, 3)?;
                region.write_binary(w)?;
            }
            Judgment::Exclusive { resource } => {
                write_u8(w, 4)?;
                resource.write_binary(w)?;
            }
            Judgment::Shared { resource, count } => {
                write_u8(w, 5)?;
                resource.write_binary(w)?;
                write_usize_as_u32(w, *count)?;
            }
            Judgment::NoConflict {
                resource_a,
                resource_b,
            } => {
                write_u8(w, 6)?;
                resource_a.write_binary(w)?;
                resource_b.write_binary(w)?;
            }
            Judgment::Derived {
                pointer,
                from,
                region,
            } => {
                write_u8(w, 7)?;
                pointer.write_binary(w)?;
                from.write_binary(w)?;
                region.write_binary(w)?;
            }
            Judgment::InBounds {
                pointer,
                offset,
                size,
            } => {
                write_u8(w, 8)?;
                pointer.write_binary(w)?;
                write_i64(w, *offset)?;
                write_i64(w, *size)?;
            }
            Judgment::BoundsPreserved {
                pointer,
                offset,
                size,
            } => {
                write_u8(w, 9)?;
                pointer.write_binary(w)?;
                write_i64(w, *offset)?;
                write_i64(w, *size)?;
            }
            Judgment::Initialized { variable } => {
                write_u8(w, 10)?;
                variable.write_binary(w)?;
            }
            Judgment::PreservesCapD {
                resource,
                from_capd,
                to_capd,
            } => {
                write_u8(w, 11)?;
                resource.write_binary(w)?;
                from_capd.write_binary(w)?;
                to_capd.write_binary(w)?;
            }
            Judgment::CastValid {
                resource,
                from_capd,
                to_capd,
            } => {
                write_u8(w, 12)?;
                resource.write_binary(w)?;
                from_capd.write_binary(w)?;
                to_capd.write_binary(w)?;
            }
            Judgment::InterpretationCompatible {
                write_repd,
                read_repd,
                address,
            } => {
                write_u8(w, 13)?;
                write_u64(w, *write_repd)?;
                write_u64(w, *read_repd)?;
                write_u64(w, *address)?;
            }
            Judgment::TemporalOrder { event_a, event_b } => {
                write_u8(w, 14)?;
                event_a.write_binary(w)?;
                event_b.write_binary(w)?;
            }
            Judgment::Assumption { description } => {
                write_u8(w, 15)?;
                write_string(w, description)?;
            }
        }
        Ok(())
    }
}

impl BinaryRead for Judgment {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let tag = read_u8(r)?;
        match tag {
            0 => Ok(Judgment::Allocated {
                region: RegionId::read_binary(r)?,
            }),
            1 => Ok(Judgment::Live {
                region: RegionId::read_binary(r)?,
            }),
            2 => Ok(Judgment::Freed {
                region: RegionId::read_binary(r)?,
            }),
            3 => Ok(Judgment::Dead {
                region: RegionId::read_binary(r)?,
            }),
            4 => Ok(Judgment::Exclusive {
                resource: ResourceId::read_binary(r)?,
            }),
            5 => {
                let resource = ResourceId::read_binary(r)?;
                let count = read_u32(r)? as usize;
                Ok(Judgment::Shared { resource, count })
            }
            6 => {
                let resource_a = ResourceId::read_binary(r)?;
                let resource_b = ResourceId::read_binary(r)?;
                Ok(Judgment::NoConflict {
                    resource_a,
                    resource_b,
                })
            }
            7 => {
                let pointer = PointerId::read_binary(r)?;
                let from = PointerId::read_binary(r)?;
                let region = RegionId::read_binary(r)?;
                Ok(Judgment::Derived {
                    pointer,
                    from,
                    region,
                })
            }
            8 => {
                let pointer = PointerId::read_binary(r)?;
                let offset = read_i64(r)?;
                let size = read_i64(r)?;
                Ok(Judgment::InBounds {
                    pointer,
                    offset,
                    size,
                })
            }
            9 => {
                let pointer = PointerId::read_binary(r)?;
                let offset = read_i64(r)?;
                let size = read_i64(r)?;
                Ok(Judgment::BoundsPreserved {
                    pointer,
                    offset,
                    size,
                })
            }
            10 => Ok(Judgment::Initialized {
                variable: VariableId::read_binary(r)?,
            }),
            11 => {
                let resource = ResourceId::read_binary(r)?;
                let from_capd = CapDKind::read_binary(r)?;
                let to_capd = CapDKind::read_binary(r)?;
                Ok(Judgment::PreservesCapD {
                    resource,
                    from_capd,
                    to_capd,
                })
            }
            12 => {
                let resource = ResourceId::read_binary(r)?;
                let from_capd = CapDKind::read_binary(r)?;
                let to_capd = CapDKind::read_binary(r)?;
                Ok(Judgment::CastValid {
                    resource,
                    from_capd,
                    to_capd,
                })
            }
            13 => {
                let write_repd = read_u64(r)?;
                let read_repd = read_u64(r)?;
                let address = read_u64(r)?;
                Ok(Judgment::InterpretationCompatible {
                    write_repd,
                    read_repd,
                    address,
                })
            }
            14 => {
                let event_a = EventId::read_binary(r)?;
                let event_b = EventId::read_binary(r)?;
                Ok(Judgment::TemporalOrder { event_a, event_b })
            }
            15 => Ok(Judgment::Assumption {
                description: read_string(r)?,
            }),
            other => Err(BinaryError::InvalidData(format!(
                "invalid Judgment discriminant: {other}"
            ))),
        }
    }
}

// ── InvariantName / FactKind / Conclusion ───────────────────────────────

impl BinaryWrite for InvariantName {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        let tag: u8 = match self {
            InvariantName::Liveness => 0,
            InvariantName::Exclusivity => 1,
            InvariantName::Cleanup => 2,
            InvariantName::Origin => 3,
            InvariantName::Interpretation => 4,
        };
        write_u8(w, tag)
    }
}

impl BinaryRead for InvariantName {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(InvariantName::Liveness),
            1 => Ok(InvariantName::Exclusivity),
            2 => Ok(InvariantName::Cleanup),
            3 => Ok(InvariantName::Origin),
            4 => Ok(InvariantName::Interpretation),
            other => Err(BinaryError::InvalidData(format!(
                "invalid InvariantName discriminant: {other}"
            ))),
        }
    }
}

impl BinaryWrite for FactKind {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        let tag: u8 = match self {
            FactKind::Axiom => 0,
            FactKind::Derived => 1,
            FactKind::Assumption => 2,
            FactKind::Checked => 3,
        };
        write_u8(w, tag)
    }
}

impl BinaryRead for FactKind {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(FactKind::Axiom),
            1 => Ok(FactKind::Derived),
            2 => Ok(FactKind::Assumption),
            3 => Ok(FactKind::Checked),
            other => Err(BinaryError::InvalidData(format!(
                "invalid FactKind discriminant: {other}"
            ))),
        }
    }
}

impl BinaryWrite for Conclusion {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        let tag: u8 = match self {
            Conclusion::Proven => 0,
            Conclusion::Refuted => 1,
            Conclusion::Inconclusive => 2,
        };
        write_u8(w, tag)
    }
}

impl BinaryRead for Conclusion {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(Conclusion::Proven),
            1 => Ok(Conclusion::Refuted),
            2 => Ok(Conclusion::Inconclusive),
            other => Err(BinaryError::InvalidData(format!(
                "invalid Conclusion discriminant: {other}"
            ))),
        }
    }
}

// ── InferenceRule ───────────────────────────────────────────────────────

impl BinaryWrite for InferenceRule {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        let tag: u8 = match self {
            InferenceRule::LivenessIntro => 0,
            InferenceRule::LivenessElim => 1,
            InferenceRule::ExclusivityIntro => 2,
            InferenceRule::ExclusivityElim => 3,
            InferenceRule::DerivationTransitivity => 4,
            InferenceRule::BoundsPreservation => 5,
            InferenceRule::CastValidity => 6,
            InferenceRule::InterpretationIntro => 7,
            InferenceRule::TemporalOrdering => 8,
        };
        write_u8(w, tag)
    }
}

impl BinaryRead for InferenceRule {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(InferenceRule::LivenessIntro),
            1 => Ok(InferenceRule::LivenessElim),
            2 => Ok(InferenceRule::ExclusivityIntro),
            3 => Ok(InferenceRule::ExclusivityElim),
            4 => Ok(InferenceRule::DerivationTransitivity),
            5 => Ok(InferenceRule::BoundsPreservation),
            6 => Ok(InferenceRule::CastValidity),
            7 => Ok(InferenceRule::InterpretationIntro),
            8 => Ok(InferenceRule::TemporalOrdering),
            other => Err(BinaryError::InvalidData(format!(
                "invalid InferenceRule discriminant: {other}"
            ))),
        }
    }
}

// ── Target / ProofContext / Goal ────────────────────────────────────────

impl BinaryWrite for Target {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        match self {
            Target::Region(id) => {
                write_u8(w, 0)?;
                id.write_binary(w)?;
            }
            Target::Access(id) => {
                write_u8(w, 1)?;
                write_u64(w, *id)?;
            }
            Target::Derivation(id) => {
                write_u8(w, 2)?;
                write_u64(w, *id)?;
            }
            Target::FullProgram => write_u8(w, 3)?,
        }
        Ok(())
    }
}

impl BinaryRead for Target {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(Target::Region(RegionId::read_binary(r)?)),
            1 => Ok(Target::Access(read_u64(r)? as AccessId)),
            2 => Ok(Target::Derivation(read_u64(r)? as DerivationId)),
            3 => Ok(Target::FullProgram),
            other => Err(BinaryError::InvalidData(format!(
                "invalid Target discriminant: {other}"
            ))),
        }
    }
}

impl BinaryWrite for ProofContext {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_string(w, &self.scope)?;
        write_vec(w, &self.assumptions)
    }
}

impl BinaryRead for ProofContext {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let scope = read_string(r)?;
        let assumptions = read_vec::<Judgment, _>(r)?;
        Ok(ProofContext { scope, assumptions })
    }
}

impl BinaryWrite for Goal {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        self.invariant.write_binary(w)?;
        self.target.write_binary(w)?;
        self.context.write_binary(w)
    }
}

impl BinaryRead for Goal {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let invariant = InvariantName::read_binary(r)?;
        let target = Target::read_binary(r)?;
        let context = ProofContext::read_binary(r)?;
        Ok(Goal {
            invariant,
            target,
            context,
        })
    }
}

// ── Fact ────────────────────────────────────────────────────────────────

impl BinaryWrite for Fact {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_u64(w, self.id as FactId)?;
        write_string(w, &self.statement)?;
        self.kind.write_binary(w)?;
        write_opt(w, &self.judgment)
    }
}

impl BinaryRead for Fact {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let id = read_u64(r)? as FactId;
        let statement = read_string(r)?;
        let kind = FactKind::read_binary(r)?;
        let judgment = read_opt::<Judgment, _>(r)?;
        Ok(Fact {
            id,
            statement,
            kind,
            judgment,
        })
    }
}

// ── ProofStep (recursive via Proof) ─────────────────────────────────────

impl BinaryWrite for ProofStep {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        match self {
            ProofStep::Assume { fact } => {
                write_u8(w, 0)?;
                fact.write_binary(w)?;
            }
            ProofStep::Infer {
                from,
                rule,
                conclusion,
            } => {
                write_u8(w, 1)?;
                write_vec_of_u64(w, from)?;
                rule.write_binary(w)?;
                conclusion.write_binary(w)?;
            }
            ProofStep::CaseSplit { cases } => {
                write_u8(w, 2)?;
                write_vec(w, cases)?;
            }
            ProofStep::Induction { base, step } => {
                write_u8(w, 3)?;
                write_box(w, base)?;
                write_box(w, step)?;
            }
            ProofStep::Contradiction {
                assumption,
                negation,
            } => {
                write_u8(w, 4)?;
                write_u64(w, *assumption as FactId)?;
                write_u64(w, *negation as FactId)?;
            }
            ProofStep::ByDefinition { definition } => {
                write_u8(w, 5)?;
                write_string(w, definition)?;
            }
        }
        Ok(())
    }
}

impl BinaryRead for ProofStep {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(ProofStep::Assume {
                fact: Fact::read_binary(r)?,
            }),
            1 => {
                let from = read_vec_of_u64(r)?;
                let rule = InferenceRule::read_binary(r)?;
                let conclusion = Fact::read_binary(r)?;
                Ok(ProofStep::Infer {
                    from,
                    rule,
                    conclusion,
                })
            }
            2 => Ok(ProofStep::CaseSplit {
                cases: read_vec::<Proof, _>(r)?,
            }),
            3 => {
                let base = read_box::<Proof, _>(r)?;
                let step = read_box::<Proof, _>(r)?;
                Ok(ProofStep::Induction { base, step })
            }
            4 => {
                let assumption = read_u64(r)? as FactId;
                let negation = read_u64(r)? as FactId;
                Ok(ProofStep::Contradiction {
                    assumption,
                    negation,
                })
            }
            5 => Ok(ProofStep::ByDefinition {
                definition: read_string(r)?,
            }),
            other => Err(BinaryError::InvalidData(format!(
                "invalid ProofStep discriminant: {other}"
            ))),
        }
    }
}

// `Vec<FactId>` is `Vec<u64>`; serialize as a length-prefixed u64 vec without
// requiring a `BinaryWrite` impl for the bare `u64` alias.
fn write_vec_of_u64<W: Write>(w: &mut W, v: &[u64]) -> Result<(), BinaryError> {
    write_usize_as_u32(w, v.len())?;
    for item in v {
        write_u64(w, *item)?;
    }
    Ok(())
}

fn read_vec_of_u64<R: Read>(r: &mut R) -> Result<Vec<u64>, BinaryError> {
    let len = read_u32(r)? as usize;
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        out.push(read_u64(r)?);
    }
    Ok(out)
}

// ── Proof ───────────────────────────────────────────────────────────────

impl BinaryWrite for Proof {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        self.goal.write_binary(w)?;
        write_vec(w, &self.steps)?;
        self.conclusion.write_binary(w)
    }
}

impl BinaryRead for Proof {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let goal = Goal::read_binary(r)?;
        let steps = read_vec::<ProofStep, _>(r)?;
        let conclusion = Conclusion::read_binary(r)?;
        Ok(Proof {
            goal,
            steps,
            conclusion,
        })
    }
}

// ── Top-level entry points ──────────────────────────────────────────────

/// Serialize a [`Proof`] to a byte vector with the `VPRF` magic + version
/// header.
pub fn serialize_proof(proof: &Proof) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    let _ = out.write_all(PROOF_MAGIC);
    let _ = out.write_all(&PROOF_VERSION.to_le_bytes());
    let _ = proof.write_binary(&mut out);
    out
}

/// Deserialize a [`Proof`] from a byte slice produced by [`serialize_proof`].
pub fn deserialize_proof(data: &[u8]) -> Result<Proof, BinaryError> {
    let mut cursor = io::Cursor::new(data);
    let mut magic = [0u8; 4];
    cursor
        .read_exact(&mut magic)
        .map_err(BinaryError::Io)?;
    if &magic != PROOF_MAGIC {
        return Err(BinaryError::InvalidData(format!(
            "bad magic header: expected {:?}, got {:?}",
            PROOF_MAGIC, magic
        )));
    }
    let version = read_u32(&mut cursor)?;
    if version != PROOF_VERSION {
        return Err(BinaryError::InvalidData(format!(
            "unsupported proof binary version: got {version}, expected {PROOF_VERSION}"
        )));
    }
    Proof::read_binary(&mut cursor)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judgment::RegionId;
    use crate::liveness_proofs::LivenessProof;
    use crate::liveness_proofs::LivenessTactic;
    use crate::proof::{Conclusion, Fact, Goal, InvariantName, ProofContext, ProofStep, Target};

    /// Helper: create a simple Proof for reuse in tests.
    fn make_simple_proof() -> Proof {
        let mut proof = Proof::new(Goal::new(
            InvariantName::Liveness,
            Target::Region(RegionId(1)),
            ProofContext::new("test::simple"),
        ));
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(0, "region 1 is allocated"),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![0],
            rule: crate::rules::InferenceRule::LivenessIntro,
            conclusion: Fact::derived(1, "region 1 is live"),
        });
        proof.conclude(Conclusion::Proven);
        proof
    }

    // ── JSON-ProofEnvelope round-trip tests (removed in Wave 43) ────────
    // The serde-derived `ProofEnvelope` JSON path (`to_json_string` /
    // `from_json_string` / `to_json_string_pretty`) was removed along with
    // the `Serialize, Deserialize` derives on `Proof` / `Judgment` / `Fact`
    // / `ProofStep`. The hand-written binary codec (`serialize_proof` /
    // `deserialize_proof`) is now the canonical (de)serialization path and
    // is covered by the round-trip tests below.

    // ── Hand-written binary round-trip tests (Wave 43) ───────────────────

    #[test]
    fn test_proof_binary_roundtrip_simple() {
        let proof = make_simple_proof();
        let bytes = serialize_proof(&proof);
        let back = deserialize_proof(&bytes).expect("deserialize");
        assert_eq!(back, proof);
    }

    #[test]
    fn test_proof_binary_roundtrip_empty() {
        let proof = Proof::new(Goal::new(
            InvariantName::Exclusivity,
            Target::FullProgram,
            ProofContext::new("empty::scope"),
        ));
        let bytes = serialize_proof(&proof);
        let back = deserialize_proof(&bytes).expect("deserialize");
        assert_eq!(back, proof);
    }

    #[test]
    fn test_proof_binary_roundtrip_all_step_variants() {
        let goal = Goal::new(
            InvariantName::Interpretation,
            Target::Access(42),
            ProofContext::new("scope::with::assumptions")
                .with_assumption("first assumption")
                .with_assumption("second assumption"),
        );
        let mut proof = Proof::new(goal);
        // Assume
        proof.add_step(ProofStep::Assume {
            fact: Fact::assumption(0, "assume P"),
        });
        // Infer with a non-trivial premise list.
        proof.add_step(ProofStep::Infer {
            from: vec![0, 5, 9],
            rule: crate::rules::InferenceRule::TemporalOrdering,
            conclusion: Fact::derived(1, "derived Q"),
        });
        // Contradiction
        proof.add_step(ProofStep::Contradiction {
            assumption: 0,
            negation: 1,
        });
        // ByDefinition
        proof.add_step(ProofStep::ByDefinition {
            definition: "Q := not P".to_string(),
        });
        // CaseSplit with nested sub-proofs (exercises Proof recursion).
        let case_a = Proof::new(Goal::new(
            InvariantName::Liveness,
            Target::Region(RegionId(7)),
            ProofContext::new("case::a"),
        ));
        let case_b = {
            let mut p = Proof::new(Goal::new(
                InvariantName::Cleanup,
                Target::Derivation(3),
                ProofContext::new("case::b"),
            ));
            p.add_step(ProofStep::Assume {
                fact: Fact::axiom(0, "nested axiom"),
            });
            p.conclude(Conclusion::Refuted);
            p
        };
        proof.add_step(ProofStep::CaseSplit {
            cases: vec![case_a, case_b],
        });
        // Induction (base + step, both recursive).
        let base = Proof::new(Goal::new(
            InvariantName::Origin,
            Target::FullProgram,
            ProofContext::new("induction::base"),
        ));
        let step = {
            let mut p = Proof::new(Goal::new(
                InvariantName::Origin,
                Target::FullProgram,
                ProofContext::new("induction::step"),
            ));
            p.add_step(ProofStep::Infer {
                from: vec![0],
                rule: crate::rules::InferenceRule::DerivationTransitivity,
                conclusion: Fact::derived(1, "inductive step conclusion"),
            });
            p.conclude(Conclusion::Proven);
            p
        };
        proof.add_step(ProofStep::Induction {
            base: Box::new(base),
            step: Box::new(step),
        });
        proof.conclude(Conclusion::Inconclusive);

        let bytes = serialize_proof(&proof);
        let back = deserialize_proof(&bytes).expect("deserialize");
        assert_eq!(back, proof);
    }

    #[test]
    fn test_proof_binary_roundtrip_with_judgments() {
        use crate::judgment::{
            CapDKind, EventId, Judgment, PointerId, RegionId, ResourceId, VariableId,
        };

        let mut proof = Proof::new(Goal::new(
            InvariantName::Liveness,
            Target::Region(RegionId(1)),
            ProofContext::new("j::scope").with_judgment_assumption(Judgment::Allocated {
                region: RegionId(1),
            }),
        ));
        // Exercise every Judgment variant via Fact::with_judgment.
        let judgments = vec![
            Judgment::Allocated { region: RegionId(1) },
            Judgment::Live { region: RegionId(2) },
            Judgment::Freed { region: RegionId(3) },
            Judgment::Dead { region: RegionId(4) },
            Judgment::Exclusive { resource: ResourceId(5) },
            Judgment::Shared {
                resource: ResourceId(6),
                count: 3,
            },
            Judgment::NoConflict {
                resource_a: ResourceId(7),
                resource_b: ResourceId(8),
            },
            Judgment::Derived {
                pointer: PointerId(9),
                from: PointerId(10),
                region: RegionId(11),
            },
            Judgment::InBounds {
                pointer: PointerId(12),
                offset: -16,
                size: 4,
            },
            Judgment::BoundsPreserved {
                pointer: PointerId(13),
                offset: 32,
                size: 8,
            },
            Judgment::Initialized {
                variable: VariableId(14),
            },
            Judgment::PreservesCapD {
                resource: ResourceId(15),
                from_capd: CapDKind::ReadWrite,
                to_capd: CapDKind::Read,
            },
            Judgment::CastValid {
                resource: ResourceId(16),
                from_capd: CapDKind::Write,
                to_capd: CapDKind::Execute,
            },
            Judgment::InterpretationCompatible {
                write_repd: 17,
                read_repd: 18,
                address: 0x1000,
            },
            Judgment::TemporalOrder {
                event_a: EventId(19),
                event_b: EventId(20),
            },
            Judgment::Assumption {
                description: "free-form assumption".to_string(),
            },
        ];
        for (i, j) in judgments.into_iter().enumerate() {
            proof.add_step(ProofStep::Assume {
                fact: Fact::with_judgment(i as u64, j, FactKind::Axiom),
            });
        }
        proof.conclude(Conclusion::Proven);

        let bytes = serialize_proof(&proof);
        let back = deserialize_proof(&bytes).expect("deserialize");
        assert_eq!(back, proof);
    }

    #[test]
    fn test_proof_binary_bad_magic() {
        let proof = make_simple_proof();
        let mut bytes = serialize_proof(&proof);
        bytes[0] = b'X';
        let err = deserialize_proof(&bytes);
        assert!(matches!(err, Err(BinaryError::InvalidData(_))));
    }

    #[test]
    fn test_proof_binary_bad_version() {
        let proof = make_simple_proof();
        let mut bytes = serialize_proof(&proof);
        bytes[4..8].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        let err = deserialize_proof(&bytes);
        assert!(matches!(err, Err(BinaryError::InvalidData(_))));
    }

    #[test]
    fn test_proof_binary_truncated() {
        let proof = make_simple_proof();
        let bytes = serialize_proof(&proof);
        let err = deserialize_proof(&bytes[..bytes.len() - 1]);
        assert!(matches!(err, Err(BinaryError::Io(_))));
    }

    #[test]
    fn test_proof_binary_error_display() {
        let e = BinaryError::InvalidData("boom".into());
        assert!(format!("{e}").contains("boom"));
        let e2 = BinaryError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "x",
        ));
        assert!(format!("{e2}").contains("binary I/O error"));
    }

    #[test]
    fn test_inference_rule_binary_roundtrip_all_variants() {
        let rules = [
            crate::rules::InferenceRule::LivenessIntro,
            crate::rules::InferenceRule::LivenessElim,
            crate::rules::InferenceRule::ExclusivityIntro,
            crate::rules::InferenceRule::ExclusivityElim,
            crate::rules::InferenceRule::DerivationTransitivity,
            crate::rules::InferenceRule::BoundsPreservation,
            crate::rules::InferenceRule::CastValidity,
            crate::rules::InferenceRule::InterpretationIntro,
            crate::rules::InferenceRule::TemporalOrdering,
        ];
        for rule in &rules {
            let mut buf = Vec::new();
            rule.write_binary(&mut buf).expect("write");
            let mut cursor = std::io::Cursor::new(&buf);
            let back = crate::rules::InferenceRule::read_binary(&mut cursor).expect("read");
            assert_eq!(&back, rule);
        }
    }

    #[test]
    fn test_binary_and_json_produce_equivalent_proof_removed_in_wave43() {
        // The serde JSON path was removed in Wave 43; this test now verifies
        // only that the hand-written binary codec round-trips a simple Proof.
        let proof = make_simple_proof();
        let bytes = serialize_proof(&proof);
        let bin_back = deserialize_proof(&bytes).expect("binary deserialize");
        assert_eq!(bin_back, proof);
    }
}
