//! # Hand-written Binary Serialization for BD Key Types
//!
//! Wave 43 — strips the *requirement* that core BD types (`RepD`, `CapD`,
//! `RelD`, `BD`) depend on `serde` derives for their on-disk representation.
//! This module provides pure-Rust, `serde`-free binary (de)serialization via
//! the [`BinaryWrite`] / [`BinaryRead`] traits plus free-function
//! [`serialize_bd`] / [`deserialize_bd`] entry points.
//!
//! # Binary Format (Version 1, little-endian)
//!
//! ```text
//! [4B]  Magic: "VBD\0"
//! [4B]  Version: u32 LE  (currently 1)
//! --- payload (BD) ---
//! ```
//!
//! The payload is the encoding produced by `BinaryWrite for BD`. All integers
//! are little-endian. Enums use a `u8` discriminant. `Vec<T>` / `HashSet<T>`
//! are length-prefixed with a `u32` LE count. `Option<T>` is a `u8` tag
//! (`0 = None`, `1 = Some(T)`). `String` is a `u32` LE byte-length prefix
//! followed by the UTF-8 bytes. `Box<RepD>` serializes as the inner `RepD`.
//!
//! `HashSet` round-trip is order-independent: elements are written in
//! iteration order and rebuilt into a `HashSet` on read; equality compares
//! sets, not sequences.
//!
//! # Status
//!
//! This is the **minimal** Wave 43 deliverable: hand-written binary
//! serializers for the four key BD types + round-trip tests. The `serde`
//! derives on these types (and on the ~40 other BD-internal types) are
//! **left in place** for now; full feature-gating of every derive site is
//! deferred (see worklog `5-a`). Downstream consumers that want a
//! `serde`-free binary path can use this module; consumers that want JSON
//! can still use the `serde` derives.

use crate::capd::{CapD, Capability, Condition, LockId, OpId, PhaseId, RegionId, SecLevel};
use crate::descriptor::{BD, BDId};
use crate::manifold::SpaceFillingCurve;
use crate::reld::{DepKind, FlowPolicy, Relation, RelD, TemporalKind};
use crate::repd::{
    ArrayRep, BDConstraint, ByteRep, ConceptRelationalRep, EnumRep, FuncRep,
    GestaltSuperpositionRep, ManifoldSpatialRep, PtrRep, RepD, StructRep, UnionRep,
};
use std::collections::HashSet;
use std::fmt;
use std::io::{self, Read, Write};

// ── Constants ───────────────────────────────────────────────────────────

/// Magic bytes identifying the VUMA BD binary format.
const MAGIC: &[u8; 4] = b"VBD\0";

/// Current binary format version.
const VERSION: u32 = 1;

// ── Error type ──────────────────────────────────────────────────────────

/// Error returned by [`deserialize_bd`] and the [`BinaryRead`] trait.
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
            BinaryError::InvalidData(msg) => write!(f, "invalid BD binary data: {msg}"),
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

// ── Traits ──────────────────────────────────────────────────────────────

/// Serialize a value to a writer using the VUMA BD binary format.
///
/// Implementations should write a self-delimiting encoding (length-prefixed
/// collections) so that composite types can be read back by [`BinaryRead`].
pub trait BinaryWrite {
    fn write_binary<W: Write>(&self, writer: &mut W) -> Result<(), BinaryError>;
}

/// Deserialize a value from a reader using the VUMA BD binary format.
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
fn write_bool<W: Write>(w: &mut W, v: bool) -> Result<(), BinaryError> {
    write_u8(w, if v { 1 } else { 0 })
}

#[inline]
fn read_bool<R: Read>(r: &mut R) -> Result<bool, BinaryError> {
    Ok(read_u8(r)? != 0)
}

fn write_string<W: Write>(w: &mut W, s: &str) -> Result<(), BinaryError> {
    let bytes = s.as_bytes();
    // Length is the byte count, not the char count.
    write_u32(w, bytes.len() as u32)?;
    w.write_all(bytes).map_err(BinaryError::Io)
}

fn read_string<R: Read>(r: &mut R) -> Result<String, BinaryError> {
    let len = read_u32(r)? as usize;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).map_err(BinaryError::Io)?;
    String::from_utf8(buf)
        .map_err(|e| BinaryError::InvalidData(format!("invalid UTF-8 in string field: {e}")))
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
    let tag = read_u8(r)?;
    match tag {
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
    write_u32(w, v.len() as u32)?;
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

fn write_hashset<T: BinaryWrite, W: Write>(
    w: &mut W,
    s: &HashSet<T>,
) -> Result<(), BinaryError> {
    write_u32(w, s.len() as u32)?;
    for item in s {
        item.write_binary(w)?;
    }
    Ok(())
}

fn read_hashset<T: BinaryRead + std::hash::Hash + Eq, R: Read>(
    r: &mut R,
) -> Result<HashSet<T>, BinaryError> {
    let len = read_u32(r)? as usize;
    let mut out = HashSet::with_capacity(len);
    for _ in 0..len {
        out.insert(T::read_binary(r)?);
    }
    Ok(out)
}

fn write_box<T: BinaryWrite, W: Write>(w: &mut W, b: &Box<T>) -> Result<(), BinaryError> {
    b.as_ref().write_binary(w)
}

fn read_box<T: BinaryRead, R: Read>(r: &mut R) -> Result<Box<T>, BinaryError> {
    Ok(Box::new(T::read_binary(r)?))
}

// ── Capability ──────────────────────────────────────────────────────────

impl BinaryWrite for Capability {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        let tag: u8 = match self {
            Capability::Read => 0,
            Capability::Write => 1,
            Capability::Execute => 2,
            Capability::Iterate => 3,
            Capability::Send => 4,
            Capability::Persist => 5,
            Capability::Serialize => 6,
            Capability::Deserialize => 7,
            Capability::Hash => 8,
            Capability::Compare => 9,
            Capability::DerivePtr => 10,
            Capability::Cast => 11,
            Capability::Fork => 12,
            Capability::Drop => 13,
            Capability::Share => 14,
            Capability::Move => 15,
            Capability::Pin => 16,
        };
        write_u8(w, tag)
    }
}

impl BinaryRead for Capability {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let tag = read_u8(r)?;
        match tag {
            0 => Ok(Capability::Read),
            1 => Ok(Capability::Write),
            2 => Ok(Capability::Execute),
            3 => Ok(Capability::Iterate),
            4 => Ok(Capability::Send),
            5 => Ok(Capability::Persist),
            6 => Ok(Capability::Serialize),
            7 => Ok(Capability::Deserialize),
            8 => Ok(Capability::Hash),
            9 => Ok(Capability::Compare),
            10 => Ok(Capability::DerivePtr),
            11 => Ok(Capability::Cast),
            12 => Ok(Capability::Fork),
            13 => Ok(Capability::Drop),
            14 => Ok(Capability::Share),
            15 => Ok(Capability::Move),
            16 => Ok(Capability::Pin),
            other => Err(BinaryError::InvalidData(format!(
                "invalid Capability discriminant: {other}"
            ))),
        }
    }
}

// ── Condition ───────────────────────────────────────────────────────────

impl BinaryWrite for Condition {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        match self {
            Condition::InPhase(id) => {
                write_u8(w, 0)?;
                write_u64(w, *id)?;
            }
            Condition::AfterOp(id) => {
                write_u8(w, 1)?;
                write_u64(w, *id)?;
            }
            Condition::BeforeOp(id) => {
                write_u8(w, 2)?;
                write_u64(w, *id)?;
            }
            Condition::NotConcurrentWith(id) => {
                write_u8(w, 3)?;
                write_u64(w, *id)?;
            }
            Condition::RequiresLock(id) => {
                write_u8(w, 4)?;
                write_u64(w, *id)?;
            }
            Condition::SecurityLevel(lvl) => {
                write_u8(w, 5)?;
                write_u8(w, *lvl)?;
            }
            Condition::ValidDuring(id) => {
                write_u8(w, 6)?;
                write_u64(w, *id)?;
            }
        }
        Ok(())
    }
}

impl BinaryRead for Condition {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let tag = read_u8(r)?;
        match tag {
            0 => Ok(Condition::InPhase(read_u64(r)? as PhaseId)),
            1 => Ok(Condition::AfterOp(read_u64(r)? as OpId)),
            2 => Ok(Condition::BeforeOp(read_u64(r)? as OpId)),
            3 => Ok(Condition::NotConcurrentWith(read_u64(r)? as OpId)),
            4 => Ok(Condition::RequiresLock(read_u64(r)? as LockId)),
            5 => Ok(Condition::SecurityLevel(read_u8(r)? as SecLevel)),
            6 => Ok(Condition::ValidDuring(read_u64(r)? as RegionId)),
            other => Err(BinaryError::InvalidData(format!(
                "invalid Condition discriminant: {other}"
            ))),
        }
    }
}

// ── CapD ────────────────────────────────────────────────────────────────

impl BinaryWrite for CapD {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_hashset(w, &self.caps)?;
        write_hashset(w, &self.conditions)
    }
}

impl BinaryRead for CapD {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let caps = read_hashset(r)?;
        let conditions = read_hashset(r)?;
        Ok(CapD { caps, conditions })
    }
}

// ── TemporalKind / DepKind / FlowPolicy ─────────────────────────────────

impl BinaryWrite for TemporalKind {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        let tag: u8 = match self {
            TemporalKind::Outlives => 0,
            TemporalKind::Coincides => 1,
            TemporalKind::Precedes => 2,
            TemporalKind::Succeeds => 3,
        };
        write_u8(w, tag)
    }
}

impl BinaryRead for TemporalKind {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(TemporalKind::Outlives),
            1 => Ok(TemporalKind::Coincides),
            2 => Ok(TemporalKind::Precedes),
            3 => Ok(TemporalKind::Succeeds),
            other => Err(BinaryError::InvalidData(format!(
                "invalid TemporalKind discriminant: {other}"
            ))),
        }
    }
}

impl BinaryWrite for DepKind {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        let tag: u8 = match self {
            DepKind::DataDep => 0,
            DepKind::ControlDep => 1,
            DepKind::AliasDep => 2,
        };
        write_u8(w, tag)
    }
}

impl BinaryRead for DepKind {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(DepKind::DataDep),
            1 => Ok(DepKind::ControlDep),
            2 => Ok(DepKind::AliasDep),
            other => Err(BinaryError::InvalidData(format!(
                "invalid DepKind discriminant: {other}"
            ))),
        }
    }
}

impl BinaryWrite for FlowPolicy {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        let tag: u8 = match self {
            FlowPolicy::NoDowngrade => 0,
            FlowPolicy::NoCrossBoundary => 1,
            FlowPolicy::Sanitized => 2,
        };
        write_u8(w, tag)
    }
}

impl BinaryRead for FlowPolicy {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(FlowPolicy::NoDowngrade),
            1 => Ok(FlowPolicy::NoCrossBoundary),
            2 => Ok(FlowPolicy::Sanitized),
            other => Err(BinaryError::InvalidData(format!(
                "invalid FlowPolicy discriminant: {other}"
            ))),
        }
    }
}

// ── Relation ────────────────────────────────────────────────────────────

impl BinaryWrite for Relation {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        match self {
            Relation::Temporal(k) => {
                write_u8(w, 0)?;
                k.write_binary(w)?;
            }
            Relation::Containment => write_u8(w, 1)?,
            Relation::Dependency(k) => {
                write_u8(w, 2)?;
                k.write_binary(w)?;
            }
            Relation::Equivalence => write_u8(w, 3)?,
            Relation::Security(p) => {
                write_u8(w, 4)?;
                p.write_binary(w)?;
            }
            Relation::Liveness => write_u8(w, 5)?,
        }
        Ok(())
    }
}

impl BinaryRead for Relation {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(Relation::Temporal(TemporalKind::read_binary(r)?)),
            1 => Ok(Relation::Containment),
            2 => Ok(Relation::Dependency(DepKind::read_binary(r)?)),
            3 => Ok(Relation::Equivalence),
            4 => Ok(Relation::Security(FlowPolicy::read_binary(r)?)),
            5 => Ok(Relation::Liveness),
            other => Err(BinaryError::InvalidData(format!(
                "invalid Relation discriminant: {other}"
            ))),
        }
    }
}

// ── RelD ────────────────────────────────────────────────────────────────

impl BinaryWrite for RelD {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_hashset(w, &self.relations)
    }
}

impl BinaryRead for RelD {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let relations = read_hashset(r)?;
        Ok(RelD { relations })
    }
}

// ── SpaceFillingCurve ───────────────────────────────────────────────────

impl BinaryWrite for SpaceFillingCurve {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        let tag: u8 = match self {
            SpaceFillingCurve::ZOrder => 0,
            SpaceFillingCurve::Hilbert => 1,
            SpaceFillingCurve::RowMajor => 2,
        };
        write_u8(w, tag)
    }
}

impl BinaryRead for SpaceFillingCurve {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(SpaceFillingCurve::ZOrder),
            1 => Ok(SpaceFillingCurve::Hilbert),
            2 => Ok(SpaceFillingCurve::RowMajor),
            other => Err(BinaryError::InvalidData(format!(
                "invalid SpaceFillingCurve discriminant: {other}"
            ))),
        }
    }
}

// ── RepD leaf structs ───────────────────────────────────────────────────

impl BinaryWrite for ByteRep {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_u64(w, self.size)?;
        write_u64(w, self.align)
    }
}

impl BinaryRead for ByteRep {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let size = read_u64(r)?;
        let align = read_u64(r)?;
        Ok(ByteRep { size, align })
    }
}

// (u64, RepD) pair — used by StructRep and EnumRep.
fn write_offset_repd<W: Write>(w: &mut W, pair: &(u64, RepD)) -> Result<(), BinaryError> {
    write_u64(w, pair.0)?;
    pair.1.write_binary(w)
}

fn read_offset_repd<R: Read>(r: &mut R) -> Result<(u64, RepD), BinaryError> {
    let off = read_u64(r)?;
    let rep = RepD::read_binary(r)?;
    Ok((off, rep))
}

impl BinaryWrite for StructRep {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_u32(w, self.fields.len() as u32)?;
        for pair in &self.fields {
            write_offset_repd(w, pair)?;
        }
        write_u64(w, self.total_size)?;
        write_u64(w, self.align)
    }
}

impl BinaryRead for StructRep {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let len = read_u32(r)? as usize;
        let mut fields = Vec::with_capacity(len);
        for _ in 0..len {
            fields.push(read_offset_repd(r)?);
        }
        let total_size = read_u64(r)?;
        let align = read_u64(r)?;
        Ok(StructRep {
            fields,
            total_size,
            align,
        })
    }
}

impl BinaryWrite for ArrayRep {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_box(w, &self.element)?;
        write_u64(w, self.count)
    }
}

impl BinaryRead for ArrayRep {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let element = read_box(r)?;
        let count = read_u64(r)?;
        Ok(ArrayRep { element, count })
    }
}

impl BinaryWrite for EnumRep {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_u32(w, self.variants.len() as u32)?;
        for pair in &self.variants {
            write_offset_repd(w, pair)?;
        }
        Ok(())
    }
}

impl BinaryRead for EnumRep {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let len = read_u32(r)? as usize;
        let mut variants = Vec::with_capacity(len);
        for _ in 0..len {
            variants.push(read_offset_repd(r)?);
        }
        Ok(EnumRep { variants })
    }
}

impl BinaryWrite for PtrRep {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_box(w, &self.pointee)
    }
}

impl BinaryRead for PtrRep {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let pointee = read_box(r)?;
        Ok(PtrRep { pointee })
    }
}

impl BinaryWrite for UnionRep {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_vec(w, &self.alternatives)?;
        write_u64(w, self.max_size)?;
        write_u64(w, self.max_align)
    }
}

impl BinaryRead for UnionRep {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let alternatives = read_vec(r)?;
        let max_size = read_u64(r)?;
        let max_align = read_u64(r)?;
        Ok(UnionRep {
            alternatives,
            max_size,
            max_align,
        })
    }
}

impl BinaryWrite for FuncRep {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_vec(w, &self.params)?;
        write_box(w, &self.result)
    }
}

impl BinaryRead for FuncRep {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let params = read_vec(r)?;
        let result = read_box(r)?;
        Ok(FuncRep { params, result })
    }
}

impl BinaryWrite for ManifoldSpatialRep {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_u32(w, self.dimensions)?;
        // dim_sizes written as a length-prefixed u64 vec.
        write_u32(w, self.dim_sizes.len() as u32)?;
        for d in &self.dim_sizes {
            write_u64(w, *d)?;
        }
        write_u64(w, self.element_size)?;
        self.curve.write_binary(w)?;
        write_u32(w, self.order)?;
        write_u64(w, self.total_bytes)
    }
}

impl BinaryRead for ManifoldSpatialRep {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let dimensions = read_u32(r)?;
        let dlen = read_u32(r)? as usize;
        let mut dim_sizes = Vec::with_capacity(dlen);
        for _ in 0..dlen {
            dim_sizes.push(read_u64(r)?);
        }
        let element_size = read_u64(r)?;
        let curve = SpaceFillingCurve::read_binary(r)?;
        let order = read_u32(r)?;
        let total_bytes = read_u64(r)?;
        Ok(ManifoldSpatialRep {
            dimensions,
            dim_sizes,
            element_size,
            curve,
            order,
            total_bytes,
        })
    }
}

impl BinaryWrite for GestaltSuperpositionRep {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_vec(w, &self.variants)?;
        write_u64(w, self.max_size)?;
        write_u64(w, self.max_align)?;
        write_bool(w, self.degraded)?;
        write_opt(w, &self.tag_offset)
    }
}

impl BinaryRead for GestaltSuperpositionRep {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let variants = read_vec::<String, _>(r)?;
        let max_size = read_u64(r)?;
        let max_align = read_u64(r)?;
        let degraded = read_bool(r)?;
        let tag_offset = read_opt::<u64, _>(r)?;
        Ok(GestaltSuperpositionRep {
            variants,
            max_size,
            max_align,
            degraded,
            tag_offset,
        })
    }
}

// Manual impls for the Option<u64> and Vec<String> fields used above — these
// piggy-back on the generic helpers, but the helpers need concrete trait
// impls for the element type.
impl BinaryWrite for u64 {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_u64(w, *self)
    }
}

impl BinaryRead for u64 {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        read_u64(r)
    }
}

impl BinaryWrite for String {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_string(w, self)
    }
}

impl BinaryRead for String {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        read_string(r)
    }
}

impl BinaryWrite for ConceptRelationalRep {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_vec(w, &self.field_names)?;
        write_u32(w, self.field_offsets.len() as u32)?;
        for (name, off) in &self.field_offsets {
            write_string(w, name)?;
            write_u64(w, *off)?;
        }
        write_u64(w, self.total_size)?;
        write_u64(w, self.align)?;
        write_bool(w, self.use_soa)
    }
}

impl BinaryRead for ConceptRelationalRep {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let field_names = read_vec::<String, _>(r)?;
        let flen = read_u32(r)? as usize;
        let mut field_offsets = Vec::with_capacity(flen);
        for _ in 0..flen {
            let name = read_string(r)?;
            let off = read_u64(r)?;
            field_offsets.push((name, off));
        }
        let total_size = read_u64(r)?;
        let align = read_u64(r)?;
        let use_soa = read_bool(r)?;
        Ok(ConceptRelationalRep {
            field_names,
            field_offsets,
            total_size,
            align,
            use_soa,
        })
    }
}

// ── BDConstraint ────────────────────────────────────────────────────────

impl BinaryWrite for BDConstraint {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        match self {
            BDConstraint::CapDAtLeast(capd) => {
                write_u8(w, 0)?;
                capd.write_binary(w)?;
            }
            BDConstraint::RepDCompatibleWith(repd) => {
                write_u8(w, 1)?;
                repd.write_binary(w)?;
            }
            BDConstraint::RelDContains(reld) => {
                write_u8(w, 2)?;
                reld.write_binary(w)?;
            }
        }
        Ok(())
    }
}

impl BinaryRead for BDConstraint {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(BDConstraint::CapDAtLeast(CapD::read_binary(r)?)),
            1 => Ok(BDConstraint::RepDCompatibleWith(read_box::<RepD, _>(r)?)),
            2 => Ok(BDConstraint::RelDContains(RelD::read_binary(r)?)),
            other => Err(BinaryError::InvalidData(format!(
                "invalid BDConstraint discriminant: {other}"
            ))),
        }
    }
}

// ── RepD ────────────────────────────────────────────────────────────────

impl BinaryWrite for RepD {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        match self {
            RepD::Byte(b) => {
                write_u8(w, 0)?;
                b.write_binary(w)?;
            }
            RepD::Struct(s) => {
                write_u8(w, 1)?;
                s.write_binary(w)?;
            }
            RepD::Array(a) => {
                write_u8(w, 2)?;
                a.write_binary(w)?;
            }
            RepD::Enum(e) => {
                write_u8(w, 3)?;
                e.write_binary(w)?;
            }
            RepD::Ptr(p) => {
                write_u8(w, 4)?;
                p.write_binary(w)?;
            }
            RepD::Union(u) => {
                write_u8(w, 5)?;
                u.write_binary(w)?;
            }
            RepD::Func(f) => {
                write_u8(w, 6)?;
                f.write_binary(w)?;
            }
            RepD::ManifoldSpatial(m) => {
                write_u8(w, 7)?;
                m.write_binary(w)?;
            }
            RepD::GestaltSuperposition(g) => {
                write_u8(w, 8)?;
                g.write_binary(w)?;
            }
            RepD::ConceptRelational(c) => {
                write_u8(w, 9)?;
                c.write_binary(w)?;
            }
            RepD::Generic { name, constraints } => {
                write_u8(w, 10)?;
                write_string(w, name)?;
                write_vec(w, constraints)?;
            }
        }
        Ok(())
    }
}

impl BinaryRead for RepD {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        match read_u8(r)? {
            0 => Ok(RepD::Byte(ByteRep::read_binary(r)?)),
            1 => Ok(RepD::Struct(StructRep::read_binary(r)?)),
            2 => Ok(RepD::Array(ArrayRep::read_binary(r)?)),
            3 => Ok(RepD::Enum(EnumRep::read_binary(r)?)),
            4 => Ok(RepD::Ptr(PtrRep::read_binary(r)?)),
            5 => Ok(RepD::Union(UnionRep::read_binary(r)?)),
            6 => Ok(RepD::Func(FuncRep::read_binary(r)?)),
            7 => Ok(RepD::ManifoldSpatial(ManifoldSpatialRep::read_binary(r)?)),
            8 => Ok(RepD::GestaltSuperposition(
                GestaltSuperpositionRep::read_binary(r)?,
            )),
            9 => Ok(RepD::ConceptRelational(ConceptRelationalRep::read_binary(
                r,
            )?)),
            10 => {
                let name = read_string(r)?;
                let constraints = read_vec::<BDConstraint, _>(r)?;
                Ok(RepD::Generic { name, constraints })
            }
            other => Err(BinaryError::InvalidData(format!(
                "invalid RepD discriminant: {other}"
            ))),
        }
    }
}

// ── BDId / BD ───────────────────────────────────────────────────────────

impl BinaryWrite for BDId {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        write_u64(w, self.0)
    }
}

impl BinaryRead for BDId {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        Ok(BDId(read_u64(r)?))
    }
}

impl BinaryWrite for BD {
    fn write_binary<W: Write>(&self, w: &mut W) -> Result<(), BinaryError> {
        self.repd.write_binary(w)?;
        self.capd.write_binary(w)?;
        self.reld.write_binary(w)
    }
}

impl BinaryRead for BD {
    fn read_binary<R: Read>(r: &mut R) -> Result<Self, BinaryError> {
        let repd = RepD::read_binary(r)?;
        let capd = CapD::read_binary(r)?;
        let reld = RelD::read_binary(r)?;
        Ok(BD { repd, capd, reld })
    }
}

// ── Top-level entry points ──────────────────────────────────────────────

/// Serialize a [`BD`] to a byte vector with the `VBD\0` magic + version
/// header.
pub fn serialize_bd(bd: &BD) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    // Header writes are infallible for a Vec, but go through the trait path
    // for uniformity.
    let _ = out.write_all(MAGIC);
    let _ = out.write_all(&VERSION.to_le_bytes());
    let _ = bd.write_binary(&mut out);
    out
}

/// Deserialize a [`BD`] from a byte slice produced by [`serialize_bd`].
pub fn deserialize_bd(data: &[u8]) -> Result<BD, BinaryError> {
    let mut cursor = io::Cursor::new(data);
    let mut magic = [0u8; 4];
    cursor.read_exact(&mut magic).map_err(BinaryError::Io)?;
    if &magic != MAGIC {
        return Err(BinaryError::InvalidData(format!(
            "bad magic header: expected {:?}, got {:?}",
            MAGIC, magic
        )));
    }
    let version = read_u32(&mut cursor)?;
    if version != VERSION {
        return Err(BinaryError::InvalidData(format!(
            "unsupported BD binary version: got {version}, expected {VERSION}"
        )));
    }
    BD::read_binary(&mut cursor)
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn byte_bd() -> BD {
        BD::new(
            RepD::Byte(ByteRep { size: 4, align: 4 }),
            CapD::empty(),
            RelD::empty(),
        )
    }

    #[test]
    fn test_bd_roundtrip_byte() {
        let bd = byte_bd();
        let bytes = serialize_bd(&bd);
        let back = deserialize_bd(&bytes).expect("deserialize");
        assert_eq!(back, bd);
    }

    #[test]
    fn test_bd_roundtrip_with_caps_and_relations() {
        let mut caps = HashSet::new();
        caps.insert(Capability::Read);
        caps.insert(Capability::Write);
        caps.insert(Capability::Execute);
        let mut conditions = HashSet::new();
        conditions.insert(Condition::InPhase(7));
        conditions.insert(Condition::RequiresLock(99));
        let capd = CapD {
            caps,
            conditions,
        };
        let mut relations = HashSet::new();
        relations.insert(Relation::Liveness);
        relations.insert(Relation::Dependency(DepKind::DataDep));
        relations.insert(Relation::Temporal(TemporalKind::Outlives));
        relations.insert(Relation::Security(FlowPolicy::NoDowngrade));
        let reld = RelD { relations };
        let bd = BD::new(RepD::Byte(ByteRep { size: 8, align: 8 }), capd, reld);

        let bytes = serialize_bd(&bd);
        let back = deserialize_bd(&bytes).expect("deserialize");
        assert_eq!(back, bd);
    }

    #[test]
    fn test_repd_roundtrip_all_variants() {
        let reps: Vec<RepD> = vec![
            RepD::Byte(ByteRep { size: 1, align: 1 }),
            RepD::Struct(StructRep {
                fields: vec![(0, RepD::Byte(ByteRep { size: 4, align: 4 }))],
                total_size: 4,
                align: 4,
            }),
            RepD::Array(ArrayRep {
                element: Box::new(RepD::Byte(ByteRep { size: 2, align: 2 })),
                count: 10,
            }),
            RepD::Enum(EnumRep {
                variants: vec![(0, RepD::Byte(ByteRep { size: 0, align: 1 }))],
            }),
            RepD::Ptr(PtrRep {
                pointee: Box::new(RepD::Byte(ByteRep { size: 8, align: 8 })),
            }),
            RepD::Union(UnionRep {
                alternatives: vec![
                    RepD::Byte(ByteRep { size: 4, align: 4 }),
                    RepD::Byte(ByteRep { size: 8, align: 8 }),
                ],
                max_size: 8,
                max_align: 8,
            }),
            RepD::Func(FuncRep {
                params: vec![RepD::Byte(ByteRep { size: 8, align: 8 })],
                result: Box::new(RepD::Byte(ByteRep { size: 1, align: 1 })),
            }),
            RepD::ManifoldSpatial(ManifoldSpatialRep {
                dimensions: 2,
                dim_sizes: vec![4, 4],
                element_size: 8,
                curve: SpaceFillingCurve::Hilbert,
                order: 2,
                total_bytes: 128,
            }),
            RepD::GestaltSuperposition(GestaltSuperpositionRep {
                variants: vec!["A".to_string(), "B".to_string()],
                max_size: 16,
                max_align: 8,
                degraded: true,
                tag_offset: Some(0),
            }),
            RepD::ConceptRelational(ConceptRelationalRep {
                field_names: vec!["x".to_string()],
                field_offsets: vec![("x".to_string(), 0)],
                total_size: 8,
                align: 8,
                use_soa: true,
            }),
            RepD::Generic {
                name: "T".to_string(),
                constraints: vec![BDConstraint::CapDAtLeast(CapD::empty())],
            },
        ];

        for rep in &reps {
            let mut buf = Vec::new();
            rep.write_binary(&mut buf).expect("write");
            let mut cursor = io::Cursor::new(&buf);
            let back = RepD::read_binary(&mut cursor).expect("read");
            assert_eq!(&back, rep, "RepD round-trip mismatch");
        }
    }

    #[test]
    fn test_capd_roundtrip() {
        let mut caps = HashSet::new();
        caps.insert(Capability::Fork);
        caps.insert(Capability::Drop);
        caps.insert(Capability::Pin);
        let mut conditions = HashSet::new();
        conditions.insert(Condition::AfterOp(3));
        conditions.insert(Condition::SecurityLevel(2));
        let capd = CapD {
            caps,
            conditions,
        };
        let mut buf = Vec::new();
        capd.write_binary(&mut buf).expect("write");
        let mut cursor = io::Cursor::new(&buf);
        let back = CapD::read_binary(&mut cursor).expect("read");
        assert_eq!(back, capd);
    }

    #[test]
    fn test_reld_roundtrip() {
        let mut relations = HashSet::new();
        for kind in [
            Relation::Containment,
            Relation::Equivalence,
            Relation::Dependency(DepKind::ControlDep),
            Relation::Dependency(DepKind::AliasDep),
            Relation::Temporal(TemporalKind::Coincides),
            Relation::Temporal(TemporalKind::Precedes),
            Relation::Security(FlowPolicy::NoCrossBoundary),
            Relation::Security(FlowPolicy::Sanitized),
        ] {
            relations.insert(kind);
        }
        let reld = RelD { relations };
        let mut buf = Vec::new();
        reld.write_binary(&mut buf).expect("write");
        let mut cursor = io::Cursor::new(&buf);
        let back = RelD::read_binary(&mut cursor).expect("read");
        assert_eq!(back, reld);
    }

    #[test]
    fn test_bd_id_roundtrip() {
        let id = BDId(12345);
        let mut buf = Vec::new();
        id.write_binary(&mut buf).expect("write");
        let mut cursor = io::Cursor::new(&buf);
        let back = BDId::read_binary(&mut cursor).expect("read");
        assert_eq!(back, id);
    }

    #[test]
    fn test_deserialize_bd_bad_magic() {
        let bd = byte_bd();
        let mut bytes = serialize_bd(&bd);
        bytes[0] = b'X';
        let err = deserialize_bd(&bytes);
        assert!(matches!(err, Err(BinaryError::InvalidData(_))));
    }

    #[test]
    fn test_deserialize_bd_bad_version() {
        let bd = byte_bd();
        let mut bytes = serialize_bd(&bd);
        // Overwrite the version field (bytes 4..8) with 0xFFFF_FFFF.
        bytes[4..8].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        let err = deserialize_bd(&bytes);
        assert!(matches!(err, Err(BinaryError::InvalidData(_))));
    }

    #[test]
    fn test_deserialize_bd_truncated() {
        let bd = byte_bd();
        let bytes = serialize_bd(&bd);
        let err = deserialize_bd(&bytes[..bytes.len() - 1]);
        assert!(matches!(err, Err(BinaryError::Io(_))));
    }

    #[test]
    fn test_binary_error_display() {
        let e = BinaryError::InvalidData("boom".into());
        assert!(format!("{e}").contains("boom"));
        let e2 = BinaryError::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "x"));
        assert!(format!("{e2}").contains("binary I/O error"));
    }

    #[test]
    fn test_nested_struct_repd_roundtrip() {
        // A struct containing an array of structs — exercises recursive
        // (RepD-inside-RepD-inside-RepD) encoding.
        let inner = RepD::Struct(StructRep {
            fields: vec![(0, RepD::Byte(ByteRep { size: 1, align: 1 }))],
            total_size: 1,
            align: 1,
        });
        let array = RepD::Array(ArrayRep {
            element: Box::new(inner.clone()),
            count: 3,
        });
        let outer = RepD::Struct(StructRep {
            fields: vec![(0, array), (32, inner)],
            total_size: 64,
            align: 8,
        });
        let bd = BD::new(outer, CapD::empty(), RelD::empty());
        let bytes = serialize_bd(&bd);
        let back = deserialize_bd(&bytes).expect("deserialize");
        assert_eq!(back, bd);
    }

    #[test]
    fn test_gestalt_with_none_tag_offset() {
        let g = GestaltSuperpositionRep {
            variants: vec!["only".to_string()],
            max_size: 4,
            max_align: 4,
            degraded: false,
            tag_offset: None,
        };
        let mut buf = Vec::new();
        g.write_binary(&mut buf).expect("write");
        let mut cursor = io::Cursor::new(&buf);
        let back = GestaltSuperpositionRep::read_binary(&mut cursor).expect("read");
        assert_eq!(back, g);
    }
}
