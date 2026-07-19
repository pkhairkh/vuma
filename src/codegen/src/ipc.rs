//! VUMA Process Isolation — IPC Module
//!
//! Implements the 8-layer runtime encapsulation stack for inter-process
//! communication. Every IPC message passes through all 8 layers.
//!
//! L1: Message Encapsulation (framing + CRC32)
//! L2: Capability Encapsulation (signed tokens)
//! L3: Memory Encapsulation (typed windows)
//! L4: Channel Encapsulation (protocol state machine)
//! L5: Worker Encapsulation (sandboxing)
//! L6: State Encapsulation (checkpointing)
//! L7: Error Encapsulation (fault containment)
//! L8: Cryptographic Encapsulation (AEAD)

use std::collections::HashMap;

// ── L1: Message Wire Format ──────────────────────────────────────────

pub const MAGIC: [u8; 4] = [0x56, 0x55, 0x4D, 0x41]; // "VUMA"
pub const PROTOCOL_VERSION: u16 = 2;
pub const HEADER_SIZE: usize = 44;
pub const CRC32_SIZE: usize = 4;
pub const MAX_PAYLOAD_SIZE: u64 = 16 * 1024 * 1024; // 16 MiB

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MessageFlags(pub u16);

impl MessageFlags {
    pub const ENCRYPTED: Self = Self(0x0001);
    pub const HAS_CAPS: Self = Self(0x0002);
    pub const HAS_SHM: Self = Self(0x0004);
    pub const IS_REPLY: Self = Self(0x0008);
    pub const IS_ERROR: Self = Self(0x0010);
    pub const HAS_STARK: Self = Self(0x0020);
    pub const EMPTY: Self = Self(0);
    pub fn empty() -> Self { Self(0) }
    pub fn bits(&self) -> u16 { self.0 }
    pub fn from_bits_truncate(v: u16) -> Self { Self(v) }
    /// True if every bit set in `other` is also set in `self`.
    pub fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

impl std::ops::BitOr for MessageFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Self(self.0 | rhs.0) }
}

impl std::ops::BitOrAssign for MessageFlags {
    fn bitor_assign(&mut self, rhs: Self) { self.0 |= rhs.0; }
}

impl std::ops::BitAnd for MessageFlags {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self { Self(self.0 & rhs.0) }
}

impl std::ops::BitAndAssign for MessageFlags {
    fn bitand_assign(&mut self, rhs: Self) { self.0 &= rhs.0; }
}

impl std::ops::Not for MessageFlags {
    type Output = Self;
    fn not(self) -> Self { Self(!self.0) }
}

#[derive(Clone, Debug)]
pub struct MessageHeader {
    pub magic: [u8; 4],
    pub version: u16,
    pub flags: MessageFlags,
    pub channel_id: u64,
    pub sequence: u64,
    pub type_hash: u64,
    pub payload_len: u64,
    pub cap_count: u32,
}

#[derive(Clone, Debug)]
pub struct EncapsulatedMessage {
    pub header: MessageHeader,
    pub payload: Vec<u8>,
    pub capabilities: Vec<crate::ipc::capability::CapabilityToken>,
}

impl EncapsulatedMessage {
    pub fn new(channel_id: u64, sequence: u64, type_hash: u64, payload: Vec<u8>) -> Self {
        Self {
            header: MessageHeader {
                magic: MAGIC,
                version: PROTOCOL_VERSION,
                flags: MessageFlags::EMPTY,
                channel_id,
                sequence,
                type_hash,
                payload_len: payload.len() as u64,
                cap_count: 0,
            },
            payload,
            capabilities: Vec::new(),
        }
    }
}

pub fn frame_message(msg: &EncapsulatedMessage) -> Vec<u8> {
    // Auto-set HAS_CAPS when capabilities are present so producers can't
    // forget to advertise them on the wire. The flag is informational on
    // the deframe side (`cap_count` in the fixed header is the source of
    // truth for how many tokens to read), but keeping it in sync with the
    // actual capability vector makes the wire format self-describing and
    // lets receivers cheaply decide whether to parse the cap section at
    // all. Clearing HAS_CAPS when the vec is empty is intentionally NOT
    // done — a producer that deliberately set the bit (e.g. to reserve
    // space for a future cap section) keeps it.
    let mut effective_flags = msg.header.flags;
    if !msg.capabilities.is_empty() {
        effective_flags |= MessageFlags::HAS_CAPS;
    }

    let mut buf = Vec::with_capacity(HEADER_SIZE + msg.payload.len() + CRC32_SIZE);
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    buf.extend_from_slice(&effective_flags.bits().to_le_bytes());
    buf.extend_from_slice(&msg.header.channel_id.to_le_bytes());
    buf.extend_from_slice(&msg.header.sequence.to_le_bytes());
    buf.extend_from_slice(&msg.header.type_hash.to_le_bytes());
    buf.extend_from_slice(&(msg.payload.len() as u64).to_le_bytes());
    buf.extend_from_slice(&(msg.capabilities.len() as u32).to_le_bytes());
    buf.extend_from_slice(&msg.payload);
    for cap in &msg.capabilities {
        buf.extend_from_slice(&cap.encode());
    }
    let crc = crc32(&buf);
    buf.extend_from_slice(&crc.to_le_bytes());
    buf
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

/// FNV-1a 64-bit hash of a type string.
///
/// This is the canonical type-hash function used to populate
/// `MessageHeader::type_hash` and to key the protocol state machine.
/// Initial value 0xcbf29ce484222325, prime 0x100000001b3 — the standard
/// FNV-1a 64 constants.
pub fn type_hash(ty: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in ty.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Legacy alias kept for source-level backwards compatibility.
/// New code should call [`type_hash`] directly.
pub fn type_hash_str(s: &str) -> u64 {
    type_hash(s)
}

// ── L1: Framing Errors ───────────────────────────────────────────────

/// Errors raised by [`deframe_message`] while parsing the L1 wire format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Fewer than `HEADER_SIZE + CRC32_SIZE` bytes were supplied.
    TooShort,
    /// Magic bytes did not match [`MAGIC`].
    BadMagic { expected: [u8; 4], actual: [u8; 4] },
    /// Protocol version mismatch (expected [`PROTOCOL_VERSION`]).
    UnsupportedVersion { expected: u16, actual: u16 },
    /// Declared `payload_len` exceeded [`MAX_PAYLOAD_SIZE`].
    PayloadTooLarge { declared: u64, limit: u64 },
    /// Buffer length did not match header-declared lengths.
    LengthMismatch { expected: usize, actual: usize },
    /// Stored CRC32 trailer did not match the value recomputed over the body.
    CrcMismatch { expected: u32, actual: u32 },
    /// A capability token failed to decode.
    CapabilityDecodeError(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::TooShort => write!(f, "frame too short (< {} bytes)", HEADER_SIZE + CRC32_SIZE),
            FrameError::BadMagic { expected, actual } => write!(
                f,
                "bad magic: expected {:?}, got {:?}",
                expected, actual
            ),
            FrameError::UnsupportedVersion { expected, actual } => write!(
                f,
                "unsupported protocol version: expected {}, got {}",
                expected, actual
            ),
            FrameError::PayloadTooLarge { declared, limit } => write!(
                f,
                "payload too large: declared {} bytes, limit {} bytes",
                declared, limit
            ),
            FrameError::LengthMismatch { expected, actual } => write!(
                f,
                "frame length mismatch: expected {} bytes, got {} bytes",
                expected, actual
            ),
            FrameError::CrcMismatch { expected, actual } => write!(
                f,
                "CRC32 mismatch: stored {:#010x}, recomputed {:#010x}",
                expected, actual
            ),
            FrameError::CapabilityDecodeError(msg) => {
                write!(f, "capability decode error: {}", msg)
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// Parse a framed message produced by [`frame_message`].
///
/// Wire layout (all integers little-endian):
/// ```text
///  [0..4]    magic         ("VUMA")
///  [4..6]    version       u16
///  [6..8]    flags         u16  (MessageFlags bits)
///  [8..16]   channel_id    u64
///  [16..24]  sequence      u64
///  [24..32]  type_hash     u64
///  [32..40]  payload_len   u64
///  [40..44]  cap_count     u32
///  [44..44+payload_len]                 payload bytes
///  [..+cap_count*CAPABILITY_TOKEN_SIZE] capability tokens
///  [last 4]  crc32         u32  (over everything preceding it)
/// ```
///
/// Validates magic, version, payload size, total length, and CRC32 before
/// returning the reconstructed [`EncapsulatedMessage`].
pub fn deframe_message(data: &[u8]) -> Result<EncapsulatedMessage, FrameError> {
    // Minimum envelope: header + CRC32 trailer.
    if data.len() < HEADER_SIZE + CRC32_SIZE {
        return Err(FrameError::TooShort);
    }

    // ── Magic ────────────────────────────────────────────────────────
    let magic: [u8; 4] = data[0..4].try_into().unwrap();
    if magic != MAGIC {
        return Err(FrameError::BadMagic {
            expected: MAGIC,
            actual: magic,
        });
    }

    // ── Version ──────────────────────────────────────────────────────
    let version = u16::from_le_bytes(data[4..6].try_into().unwrap());
    if version != PROTOCOL_VERSION {
        return Err(FrameError::UnsupportedVersion {
            expected: PROTOCOL_VERSION,
            actual: version,
        });
    }

    // ── Remaining header fields ──────────────────────────────────────
    let flags = MessageFlags::from_bits_truncate(u16::from_le_bytes(
        data[6..8].try_into().unwrap(),
    ));
    let channel_id = u64::from_le_bytes(data[8..16].try_into().unwrap());
    let sequence = u64::from_le_bytes(data[16..24].try_into().unwrap());
    let type_hash = u64::from_le_bytes(data[24..32].try_into().unwrap());
    let payload_len = u64::from_le_bytes(data[32..40].try_into().unwrap());
    let cap_count = u32::from_le_bytes(data[40..44].try_into().unwrap());

    // ── Payload size guard ───────────────────────────────────────────
    if payload_len > MAX_PAYLOAD_SIZE {
        return Err(FrameError::PayloadTooLarge {
            declared: payload_len,
            limit: MAX_PAYLOAD_SIZE,
        });
    }

    // ── Length consistency ───────────────────────────────────────────
    let caps_size = (cap_count as usize)
        .checked_mul(capability::CAPABILITY_TOKEN_SIZE)
        .ok_or(FrameError::PayloadTooLarge {
            declared: payload_len,
            limit: MAX_PAYLOAD_SIZE,
        })?;
    let expected_total = HEADER_SIZE
        .checked_add(payload_len as usize)
        .and_then(|n| n.checked_add(caps_size))
        .and_then(|n| n.checked_add(CRC32_SIZE))
        .ok_or(FrameError::PayloadTooLarge {
            declared: payload_len,
            limit: MAX_PAYLOAD_SIZE,
        })?;
    if data.len() != expected_total {
        return Err(FrameError::LengthMismatch {
            expected: expected_total,
            actual: data.len(),
        });
    }

    // ── CRC32 verification ───────────────────────────────────────────
    let body_end = data.len() - CRC32_SIZE;
    let stored_crc = u32::from_le_bytes(data[body_end..].try_into().unwrap());
    let computed_crc = crc32(&data[..body_end]);
    if stored_crc != computed_crc {
        return Err(FrameError::CrcMismatch {
            expected: stored_crc,
            actual: computed_crc,
        });
    }

    // ── Payload + capability extraction ──────────────────────────────
    let payload_start = HEADER_SIZE;
    let payload_end = payload_start + payload_len as usize;
    let payload = data[payload_start..payload_end].to_vec();

    let mut capabilities = Vec::with_capacity(cap_count as usize);
    let mut cap_offset = payload_end;
    for _ in 0..cap_count {
        let cap_slice = &data[cap_offset..cap_offset + capability::CAPABILITY_TOKEN_SIZE];
        let cap = capability::CapabilityToken::decode(cap_slice)
            .map_err(FrameError::CapabilityDecodeError)?;
        capabilities.push(cap);
        cap_offset += capability::CAPABILITY_TOKEN_SIZE;
    }

    Ok(EncapsulatedMessage {
        header: MessageHeader {
            magic,
            version,
            flags,
            channel_id,
            sequence,
            type_hash,
            payload_len,
            cap_count,
        },
        payload,
        capabilities,
    })
}

// ── L2: Capability Tokens ────────────────────────────────────────────

pub mod capability {
    use std::collections::{HashMap, HashSet};

    #[derive(Clone, Debug, PartialEq, Eq, Hash)]
    pub enum Resource {
        File(String),
        Network(String, u16),
        Memory(u64, u64),
        Mmio(u64, u64),
        Channel(u64),
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct MemoryPermissions {
        pub read: bool,
        pub write: bool,
        pub execute: bool,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct CapabilityToken {
        pub id: u128,
        pub source_pid: u64,
        pub target_pid: u64,
        pub resource: Resource,
        pub permissions: MemoryPermissions,
        pub delegation_depth: u8,
        pub created_at: u64,
        pub expires_at: u64,
        pub signature: [u8; 32],
    }

    // ── Wire format (Wave 11) ──────────────────────────────────────────
    //
    // The on-the-wire layout of a CapabilityToken is a fixed-size record
    // (CAPABILITY_TOKEN_SIZE bytes, little-endian) so the L1 framer can
    // slice `cap_count` tokens out of a frame by simple arithmetic:
    //
    //   [  0.. 16] id              u128 LE
    //   [ 16.. 24] source_pid      u64  LE
    //   [ 24.. 32] target_pid      u64  LE
    //   [ 32.. 40] created_at      u64  LE
    //   [ 40.. 48] expires_at      u64  LE
    //   [ 48.. 80] signature       [u8; 32]
    //   [    80 ] delegation_depth u8
    //   [    81 ] read             u8 (0/1)
    //   [    82 ] write            u8 (0/1)
    //   [    83 ] execute          u8 (0/1)
    //   [ 84..160] resource        RESOURCE_FIELD_SIZE bytes (see Resource::encode)
    //
    // The resource field is itself a tagged fixed-size buffer so that all
    // five Resource variants round-trip through a fixed-size slot — the
    // previous implementation dropped `resource` on encode and substituted
    // `Resource::Memory(0, 0)` on decode (a lossy placeholder).

    pub const CAPABILITY_TOKEN_SIZE: usize = 160;
    pub const RESOURCE_OFFSET: usize = 84;
    pub const RESOURCE_FIELD_SIZE: usize = 76; // 84..160
    pub const MAX_RESOURCE_STRING: usize = 64; // cap for File / Network host strings

    const TAG_FILE: u8 = 1;
    const TAG_NETWORK: u8 = 2;
    const TAG_MEMORY: u8 = 3;
    const TAG_MMIO: u8 = 4;
    const TAG_CHANNEL: u8 = 5;
    // Network port is fixed at offset 66 within the resource field (after
    // the 64-byte string slot + 2-byte tag/len header), so encode/decode
    // don't need to walk a length prefix to find it.
    const NETWORK_PORT_OFFSET: usize = 66;

    impl Resource {
        /// Serialise the resource into a fixed-width
        /// `RESOURCE_FIELD_SIZE`-byte buffer. String variants are truncated
        /// to `MAX_RESOURCE_STRING` bytes; the truncation is recorded in
        /// the length byte so the decode side recovers exactly the bytes
        /// that were stored (no silent re-padding).
        pub fn encode(&self) -> [u8; RESOURCE_FIELD_SIZE] {
            let mut buf = [0u8; RESOURCE_FIELD_SIZE];
            match self {
                Resource::File(s) => {
                    buf[0] = TAG_FILE;
                    let bytes = s.as_bytes();
                    let len = bytes.len().min(MAX_RESOURCE_STRING);
                    buf[1] = len as u8;
                    buf[2..2 + len].copy_from_slice(&bytes[..len]);
                }
                Resource::Network(s, port) => {
                    buf[0] = TAG_NETWORK;
                    let bytes = s.as_bytes();
                    let len = bytes.len().min(MAX_RESOURCE_STRING);
                    buf[1] = len as u8;
                    buf[2..2 + len].copy_from_slice(&bytes[..len]);
                    buf[NETWORK_PORT_OFFSET..NETWORK_PORT_OFFSET + 2]
                        .copy_from_slice(&port.to_le_bytes());
                }
                Resource::Memory(base, size) => {
                    buf[0] = TAG_MEMORY;
                    buf[1..9].copy_from_slice(&base.to_le_bytes());
                    buf[9..17].copy_from_slice(&size.to_le_bytes());
                }
                Resource::Mmio(base, size) => {
                    buf[0] = TAG_MMIO;
                    buf[1..9].copy_from_slice(&base.to_le_bytes());
                    buf[9..17].copy_from_slice(&size.to_le_bytes());
                }
                Resource::Channel(id) => {
                    buf[0] = TAG_CHANNEL;
                    buf[1..9].copy_from_slice(&id.to_le_bytes());
                }
            }
            buf
        }

        /// Parse a resource from a buffer that is at least
        /// `RESOURCE_FIELD_SIZE` bytes long. Returns an error string on
        /// unknown tag, truncated UTF-8, or short buffer.
        pub fn decode(bytes: &[u8]) -> Result<Self, String> {
            if bytes.len() < RESOURCE_FIELD_SIZE {
                return Err(format!(
                    "resource field too short: {} < {}",
                    bytes.len(),
                    RESOURCE_FIELD_SIZE
                ));
            }
            let tag = bytes[0];
            match tag {
                TAG_FILE => {
                    let len = bytes[1] as usize;
                    if len > MAX_RESOURCE_STRING {
                        return Err(format!(
                            "file string length {} exceeds {}",
                            len, MAX_RESOURCE_STRING
                        ));
                    }
                    let s = std::str::from_utf8(&bytes[2..2 + len])
                        .map_err(|e| format!("invalid utf-8 in File resource: {}", e))?;
                    Ok(Resource::File(s.to_string()))
                }
                TAG_NETWORK => {
                    let len = bytes[1] as usize;
                    if len > MAX_RESOURCE_STRING {
                        return Err(format!(
                            "network host length {} exceeds {}",
                            len, MAX_RESOURCE_STRING
                        ));
                    }
                    let s = std::str::from_utf8(&bytes[2..2 + len])
                        .map_err(|e| format!("invalid utf-8 in Network resource: {}", e))?;
                    let port = u16::from_le_bytes(
                        bytes[NETWORK_PORT_OFFSET..NETWORK_PORT_OFFSET + 2]
                            .try_into()
                            .unwrap(),
                    );
                    Ok(Resource::Network(s.to_string(), port))
                }
                TAG_MEMORY => {
                    let base = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
                    let size = u64::from_le_bytes(bytes[9..17].try_into().unwrap());
                    Ok(Resource::Memory(base, size))
                }
                TAG_MMIO => {
                    let base = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
                    let size = u64::from_le_bytes(bytes[9..17].try_into().unwrap());
                    Ok(Resource::Mmio(base, size))
                }
                TAG_CHANNEL => {
                    let id = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
                    Ok(Resource::Channel(id))
                }
                other => Err(format!("unknown resource tag: {}", other)),
            }
        }
    }

    impl MemoryPermissions {
        /// True iff every permission set in `required` is also set in
        /// `self`. Used by [`verify_capability`] to enforce least-privilege
        /// checks: the token must grant *at least* the requested rights.
        pub fn contains(&self, required: &MemoryPermissions) -> bool {
            (!required.read || self.read)
                && (!required.write || self.write)
                && (!required.execute || self.execute)
        }
    }

    impl CapabilityToken {
        /// Serialise the token to a fixed-width `CAPABILITY_TOKEN_SIZE`
        /// byte vector (little-endian). Inverse of [`decode`].
        pub fn encode(&self) -> Vec<u8> {
            let mut buf = Vec::with_capacity(CAPABILITY_TOKEN_SIZE);
            buf.extend_from_slice(&self.id.to_le_bytes());
            buf.extend_from_slice(&self.source_pid.to_le_bytes());
            buf.extend_from_slice(&self.target_pid.to_le_bytes());
            buf.extend_from_slice(&self.created_at.to_le_bytes());
            buf.extend_from_slice(&self.expires_at.to_le_bytes());
            buf.extend_from_slice(&self.signature);
            buf.push(self.delegation_depth);
            buf.push(if self.permissions.read { 1 } else { 0 });
            buf.push(if self.permissions.write { 1 } else { 0 });
            buf.push(if self.permissions.execute { 1 } else { 0 });
            buf.extend_from_slice(&self.resource.encode());
            // Defensive: pad (or trim) to the advertised size in case the
            // layout above ever drifts away from CAPABILITY_TOKEN_SIZE.
            while buf.len() < CAPABILITY_TOKEN_SIZE {
                buf.push(0);
            }
            buf.truncate(CAPABILITY_TOKEN_SIZE);
            buf
        }

        /// Parse a token from a byte slice. Requires at least
        /// `CAPABILITY_TOKEN_SIZE` bytes; extra trailing bytes are ignored
        /// (so callers can hand in a slice of a larger frame without first
        /// trimming it). Returns an error string on short buffer or
        /// malformed resource field.
        pub fn decode(bytes: &[u8]) -> Result<Self, String> {
            if bytes.len() < CAPABILITY_TOKEN_SIZE {
                return Err(format!(
                    "token too short: {} < {}",
                    bytes.len(),
                    CAPABILITY_TOKEN_SIZE
                ));
            }
            let resource = Resource::decode(
                &bytes[RESOURCE_OFFSET..RESOURCE_OFFSET + RESOURCE_FIELD_SIZE],
            )?;
            Ok(Self {
                id: u128::from_le_bytes(bytes[0..16].try_into().unwrap()),
                source_pid: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
                target_pid: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
                created_at: u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
                expires_at: u64::from_le_bytes(bytes[40..48].try_into().unwrap()),
                signature: bytes[48..80].try_into().unwrap(),
                delegation_depth: bytes[80],
                permissions: MemoryPermissions {
                    read: bytes[81] != 0,
                    write: bytes[82] != 0,
                    execute: bytes[83] != 0,
                },
                resource,
            })
        }
    }

    #[derive(Debug, Default)]
    pub struct CapabilitySet {
        pub tokens: HashMap<u128, CapabilityToken>,
        pub revoked: HashSet<u128>,
    }

    impl CapabilitySet {
        pub fn new() -> Self { Self::default() }

        pub fn grant(&mut self, token: CapabilityToken) {
            self.tokens.insert(token.id, token);
        }

        pub fn revoke(&mut self, id: u128) {
            self.revoked.insert(id);
        }

        pub fn is_revoked(&self, id: u128) -> bool {
            self.revoked.contains(&id)
        }

        pub fn verify(&self, token: &CapabilityToken, now: u64) -> bool {
            if self.is_revoked(token.id) { return false; }
            if now > token.expires_at { return false; }
            true
        }
    }

    // ── Grant / Verify (Wave 11) ───────────────────────────────────────
    //
    // `grant_capability` mints a fresh token whose `signature` field is a
    // deterministic 32-byte digest of every other field in the token,
    // keyed by `signing_key`. `verify_capability` recomputes that digest
    // and rejects the token if any field has been tampered with, if the
    // token is outside its validity window, if the resource doesn't match
    // the one the caller expected, or if the token's permissions don't
    // cover the ones the caller needs.
    //
    // SECURITY NOTE: the digest is built from FNV-1a, a *non-cryptographic*
    // hash. It is NOT HMAC, NOT a MAC, and NOT resistant to a determined
    // adversary with access to `signing_key` (or to the source). It exists
    // so that grant/verify round-trips work end-to-end without pulling in
    // a crypto crate, and so that accidental byte-flips in transit are
    // detected. A production deployment MUST replace `compute_signature`
    // with HMAC-SHA256 (or BLAKE2s) over a real per-domain secret key.

    /// Error returned by [`verify_capability`]. Each variant names the
    /// specific check that failed so callers can distinguish "tampered
    /// token" from "expired token" from "wrong resource" from
    /// "insufficient permissions" without parsing a string.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum CapabilityError {
        /// The recomputed signature does not match the one stored in the
        /// token — at least one field has been altered since grant time.
        InvalidSignature,
        /// `now` is outside `[created_at, expires_at]`.
        Expired { now: u64, created_at: u64, expires_at: u64 },
        /// `token.resource` does not equal the resource the caller asked
        /// to be bound to. Carries both sides for diagnostics.
        ResourceMismatch { expected: Resource, actual: Resource },
        /// The token's permissions are missing one or more of the
        /// required bits.
        InsufficientPermissions {
            required: MemoryPermissions,
            actual: MemoryPermissions,
        },
    }

    impl std::fmt::Display for CapabilityError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                CapabilityError::InvalidSignature => {
                    write!(f, "invalid capability signature")
                }
                CapabilityError::Expired { now, created_at, expires_at } => write!(
                    f,
                    "capability expired: now={} not in [{}, {}]",
                    now, created_at, expires_at
                ),
                CapabilityError::ResourceMismatch { expected, actual } => write!(
                    f,
                    "capability resource mismatch: expected {:?}, got {:?}",
                    expected, actual
                ),
                CapabilityError::InsufficientPermissions { required, actual } => write!(
                    f,
                    "insufficient permissions: required {:?}, got {:?}",
                    required, actual
                ),
            }
        }
    }

    impl std::error::Error for CapabilityError {}

    /// FNV-1a 64-bit over `data`. Pure, allocation-free, deterministic.
    /// Used as the building block for [`compute_signature`].
    fn fnv1a_64(data: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for &b in data {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    /// Serialise every field of `token` *except* `signature` into a flat
    /// byte vector, in the same order `encode()` writes them on the wire.
    /// The signature is excluded so that recomputing it over the
    /// just-minted token is well-defined (otherwise we'd be hashing the
    /// empty `[0u8; 32]` placeholder, which would make the digest useless
    /// for detecting post-grant tampering of the signature field itself).
    fn signature_input(token: &CapabilityToken, signing_key: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(256);
        buf.extend_from_slice(signing_key);
        buf.extend_from_slice(&token.id.to_le_bytes());
        buf.extend_from_slice(&token.source_pid.to_le_bytes());
        buf.extend_from_slice(&token.target_pid.to_le_bytes());
        buf.extend_from_slice(&token.created_at.to_le_bytes());
        buf.extend_from_slice(&token.expires_at.to_le_bytes());
        buf.push(token.delegation_depth);
        buf.push(if token.permissions.read { 1 } else { 0 });
        buf.push(if token.permissions.write { 1 } else { 0 });
        buf.push(if token.permissions.execute { 1 } else { 0 });
        buf.extend_from_slice(&token.resource.encode());
        buf
    }

    /// Compute the 32-byte signature for `token` under `signing_key`.
    ///
    /// Strategy: run FNV-1a four times over the same input, each time
    /// prefixing a different 1-byte salt (0, 1, 2, 3). Each pass yields a
    /// u64; concatenating the four little-endian u64s gives 32 bytes. The
    /// salt prevents the four passes from producing identical 8-byte
    /// halves, which they otherwise would (FNV-1a is deterministic).
    ///
    /// See the module-level SECURITY NOTE for why this is acceptable as a
    /// tamper-detection checksum but NOT as a real MAC.
    pub fn compute_signature(token: &CapabilityToken, signing_key: &[u8]) -> [u8; 32] {
        let base = signature_input(token, signing_key);
        let mut sig = [0u8; 32];
        for i in 0..4u8 {
            let mut chunk = Vec::with_capacity(base.len() + 1);
            chunk.push(i);
            chunk.extend_from_slice(&base);
            let h = fnv1a_64(&chunk);
            sig[(i as usize) * 8..(i as usize + 1) * 8]
                .copy_from_slice(&h.to_le_bytes());
        }
        sig
    }

    /// Mint a new capability token.
    ///
    /// * `id` — caller-supplied unique identifier (e.g. a counter or a
    ///   UUID). `grant_capability` does not generate one itself so that
    ///   the same `(id, source_pid, target_pid, resource, created_at,
    ///   signing_key)` inputs always produce a byte-identical token, which
    ///   makes round-trip tests deterministic.
    /// * `created_at` / `ttl_seconds` — the token is valid in
    ///   `[created_at, created_at + ttl_seconds]` (saturating add).
    /// * `signing_key` — opaque bytes mixed into the signature. Two
    ///   different keys produce different signatures for the same token
    ///   fields, so a token minted under one key will fail verification
    ///   under another.
    pub fn grant_capability(
        id: u128,
        source_pid: u64,
        target_pid: u64,
        resource: Resource,
        permissions: MemoryPermissions,
        delegation_depth: u8,
        created_at: u64,
        ttl_seconds: u64,
        signing_key: &[u8],
    ) -> CapabilityToken {
        let expires_at = created_at.saturating_add(ttl_seconds);
        let mut token = CapabilityToken {
            id,
            source_pid,
            target_pid,
            resource,
            permissions,
            delegation_depth,
            created_at,
            expires_at,
            signature: [0u8; 32],
        };
        token.signature = compute_signature(&token, signing_key);
        token
    }

    /// Verify a capability token against a set of requirements.
    ///
    /// Returns `Ok(())` iff *all* of the following hold:
    ///
    /// 1. **Signature** — `compute_signature(token, signing_key)` equals
    ///    `token.signature`. Catches any tampering with `id`, pids,
    ///    timestamps, delegation depth, permissions, or resource after
    ///    the token was minted by [`grant_capability`].
    /// 2. **Validity window** — `created_at <= now <= expires_at`.
    /// 3. **Resource** — if `expected_resource` is `Some(r)`, then
    ///    `token.resource == r`. Pass `None` to skip this check (e.g.
    ///    when verifying a token whose resource is implied by context).
    /// 4. **Permissions** — `token.permissions` is a superset of
    ///    `required_perms` (i.e. every bit set in `required_perms` is
    ///    also set in the token).
    ///
    /// Returns the specific [`CapabilityError`] variant on failure so
    /// callers can distinguish the four failure modes without parsing
    /// strings.
    pub fn verify_capability(
        token: &CapabilityToken,
        signing_key: &[u8],
        now: u64,
        expected_resource: Option<&Resource>,
        required_perms: &MemoryPermissions,
    ) -> Result<(), CapabilityError> {
        let recomputed = compute_signature(token, signing_key);
        if recomputed != token.signature {
            return Err(CapabilityError::InvalidSignature);
        }
        if now < token.created_at || now > token.expires_at {
            return Err(CapabilityError::Expired {
                now,
                created_at: token.created_at,
                expires_at: token.expires_at,
            });
        }
        if let Some(expected) = expected_resource {
            if &token.resource != expected {
                return Err(CapabilityError::ResourceMismatch {
                    expected: expected.clone(),
                    actual: token.resource.clone(),
                });
            }
        }
        if !token.permissions.contains(required_perms) {
            return Err(CapabilityError::InsufficientPermissions {
                required: required_perms.clone(),
                actual: token.permissions.clone(),
            });
        }
        Ok(())
    }
}

// ── L3: Memory Windows ───────────────────────────────────────────────
//
// A MemoryWindow is the kernel-side record of a shared-memory mapping
// established between two processes. The mmap itself is performed at
// runtime by the codegen backend (which emits the appropriate syscalls
// for the target architecture); this struct is the bookkeeping that
// travels alongside the IPC message so the receiver can validate the
// mapping before touching it.
//
// Wire format (MEMORY_WINDOW_SIZE bytes, little-endian):
//   [  0..  8] source_pid      u64
//   [  8.. 16] target_pid      u64
//   [ 16.. 24] source_addr     u64
//   [ 24.. 32] target_addr     u64
//   [ 32.. 40] size            u64
//   [ 40.. 48] capability_id   u128 (low 64 bits)
//   [ 48.. 56] capability_id   u128 (high 64 bits)
//   [     56 ] read            u8 (0/1)
//   [     57 ] write           u8 (0/1)
//   [     58 ] execute         u8 (0/1)
//   [     59 ] revocable       u8 (0/1)
//   [     60 ] revoked         u8 (0/1)
//   [     61 ] linear          u8 (0/1)  — single-use window
//   [ 62.. 64] reserved        zeros (future expansion)

pub const MEMORY_WINDOW_SIZE: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryWindow {
    pub source_pid: u64,
    pub target_pid: u64,
    pub source_addr: u64,
    pub target_addr: u64,
    pub size: u64,
    pub permissions: capability::MemoryPermissions,
    pub capability_id: u128,
    /// If false, [`revoke_memory`] will refuse to revoke the window. This
    /// is set at grant time and is immutable for the lifetime of the
    /// window — it lets a grantor publish a permanent mapping (e.g. a
    /// read-only configuration page) that cannot be yanked out from under
    /// the receiver.
    pub revocable: bool,
    /// Set to true by [`revoke_memory`]. Once revoked the window is dead:
    /// [`is_valid`] returns false and the receiver must unmap its local
    /// view. The kernel/backend is responsible for the actual unmap; this
    /// flag is the IPC-level signal.
    pub revoked: bool,
    /// A linear window is single-use: after one send/recv cycle the
    /// backend invalidates it automatically. Non-linear windows persist
    /// across messages until explicitly revoked or the channel closes.
    pub linear: bool,
}

impl MemoryWindow {
    /// Serialise to a fixed-width `MEMORY_WINDOW_SIZE` byte buffer
    /// (little-endian). Inverse of [`MemoryWindow::decode`].
    pub fn encode(&self) -> [u8; MEMORY_WINDOW_SIZE] {
        let mut buf = [0u8; MEMORY_WINDOW_SIZE];
        buf[0..8].copy_from_slice(&self.source_pid.to_le_bytes());
        buf[8..16].copy_from_slice(&self.target_pid.to_le_bytes());
        buf[16..24].copy_from_slice(&self.source_addr.to_le_bytes());
        buf[24..32].copy_from_slice(&self.target_addr.to_le_bytes());
        buf[32..40].copy_from_slice(&self.size.to_le_bytes());
        let cap_lo = (self.capability_id & 0xFFFF_FFFF_FFFF_FFFF) as u64;
        let cap_hi = (self.capability_id >> 64) as u64;
        buf[40..48].copy_from_slice(&cap_lo.to_le_bytes());
        buf[48..56].copy_from_slice(&cap_hi.to_le_bytes());
        buf[56] = if self.permissions.read { 1 } else { 0 };
        buf[57] = if self.permissions.write { 1 } else { 0 };
        buf[58] = if self.permissions.execute { 1 } else { 0 };
        buf[59] = if self.revocable { 1 } else { 0 };
        buf[60] = if self.revoked { 1 } else { 0 };
        buf[61] = if self.linear { 1 } else { 0 };
        // bytes 62..64 left as zero (reserved)
        buf
    }

    /// Parse a window from a byte slice. Requires at least
    /// `MEMORY_WINDOW_SIZE` bytes; extra trailing bytes are ignored so a
    /// caller can hand in a slice of a larger IPC frame without first
    /// trimming it. Returns an error string on short buffer.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < MEMORY_WINDOW_SIZE {
            return Err(format!(
                "memory window too short: {} < {}",
                bytes.len(),
                MEMORY_WINDOW_SIZE
            ));
        }
        let cap_lo = u64::from_le_bytes(bytes[40..48].try_into().unwrap());
        let cap_hi = u64::from_le_bytes(bytes[48..56].try_into().unwrap());
        let capability_id = (cap_hi as u128) << 64 | (cap_lo as u128);
        Ok(Self {
            source_pid: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
            target_pid: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            source_addr: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            target_addr: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
            size: u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
            permissions: capability::MemoryPermissions {
                read: bytes[56] != 0,
                write: bytes[57] != 0,
                execute: bytes[58] != 0,
            },
            capability_id,
            revocable: bytes[59] != 0,
            revoked: bytes[60] != 0,
            linear: bytes[61] != 0,
        })
    }
}

/// Grant a memory window from `source_pid` to `target_pid`.
///
/// This records the mapping in a [`MemoryWindow`] struct and mints a
/// fresh `capability_id` that the receiver can present back to the kernel
/// to claim the mapping. The actual mmap/munmap is performed at runtime
/// by the codegen backend (which emits the target-arch syscalls); this
/// function is purely the IPC-level bookkeeping that travels with the
/// message.
///
/// The window starts life as: valid (`revoked == false`), revocable
/// (so the grantor can pull it later), and non-linear (persists across
/// messages). Callers that want a single-use window can flip `.linear`
/// after the call.
pub fn grant_memory(
    source_pid: u64,
    target_pid: u64,
    addr: u64,
    size: u64,
    perms: capability::MemoryPermissions,
) -> MemoryWindow {
    MemoryWindow {
        source_pid,
        target_pid,
        source_addr: addr,
        // The target-side virtual address is assigned by the receiver's
        // own address space; we record 0 here and the backend fills in
        // the real value after the receiver's mmap succeeds. This keeps
        // the grant asynchronous: the sender does not need to know where
        // the mapping will land in the receiver.
        target_addr: 0,
        size,
        permissions: perms,
        capability_id: rand_u128(),
        revocable: true,
        revoked: false,
        linear: false,
    }
}

/// Revoke a previously granted memory window.
///
/// Marks the window as revoked so that [`is_valid`] subsequently returns
/// false. Returns an error if the window was created with `revocable ==
/// false` (permanent mappings cannot be yanked), or if it is already
/// revoked (idempotent revoke is a programming error — the caller should
/// have dropped its reference after the first revoke).
pub fn revoke_memory(window: &mut MemoryWindow) -> Result<(), IpcError> {
    if !window.revocable {
        return Err(IpcError::MemoryWindowPermissionDenied);
    }
    if window.revoked {
        return Err(IpcError::MemoryWindowRevoked);
    }
    window.revoked = true;
    Ok(())
}

/// True iff the window is still usable: not revoked, and sized non-zero
/// (a zero-size window is a tombstone the backend leaves behind after
/// unmapping, so a stale pointer to it must not be treated as live).
pub fn is_valid(window: &MemoryWindow) -> bool {
    !window.revoked && window.size > 0
}

// ── L4: Protocol State Machine ──────────────────────────────────────
//
// The channel-level protocol FSM. Each IPC channel carries an instance
// of this state machine; every inbound message is checked against the
// current state before it is delivered to the application. This is the
// L4 layer in the 8-layer stack: it sits below the application and
// rejects messages that would violate the channel's protocol contract
// (e.g. a recv arriving while the channel is idle and waiting for a
// send).
//
// Transitions are keyed by `(current_state, type_hash)`. The default
// table installed by [`new_protocol`] models a request/response channel:
//
//   Idle           --send-->    WaitingForSend
//   WaitingForSend --sent-->    WaitingForRecv
//   WaitingForRecv --recv-->    Idle
//   Idle           --recv-->    WaitingForRecv   (receiver may block first)
//   WaitingForRecv --send-->    WaitingForSend   (pipelined reply)
//   Idle           --close-->   Closed
//   WaitingForSend --close-->   Closed
//   WaitingForRecv --close-->   Closed
//
// `type_hash` here is the same FNV-1a 64 value computed by [`type_hash`]
// for the message's Rust type string, so the FSM keys off the same
// identifier that already rides in `MessageHeader::type_hash`.

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProtocolState {
    Idle,
    WaitingForSend,
    WaitingForRecv,
    Closed,
}

impl ProtocolState {
    /// Stable string tag for use in error messages and logging. Keeping
    /// this hand-written (rather than `{:?}`) decouples the wire/log
    /// representation from `derive(Debug)` formatting drift.
    pub fn as_tag(&self) -> &'static str {
        match self {
            ProtocolState::Idle => "idle",
            ProtocolState::WaitingForSend => "waiting_for_send",
            ProtocolState::WaitingForRecv => "waiting_for_recv",
            ProtocolState::Closed => "closed",
        }
    }
}

/// Errors raised by [`ProtocolStateMachine::check_transition`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    /// The current state does not permit a transition on the supplied
    /// `type_hash`. `expected` is the state the machine was in (and
    /// would have had to leave), and `got` is the offending type hash.
    ProtocolViolation {
        expected: ProtocolState,
        got: u64,
    },
    /// The channel has been closed and accepts no further transitions.
    /// Distinct from `ProtocolViolation` so a receiver can tear down
    /// cleanly rather than logging a protocol fault.
    Closed,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::ProtocolViolation { expected, got } => write!(
                f,
                "protocol violation: state={} does not permit type_hash={}",
                expected.as_tag(),
                got
            ),
            ProtocolError::Closed => write!(f, "channel closed: no transitions permitted"),
        }
    }
}

impl std::error::Error for ProtocolError {}

#[derive(Clone, Debug)]
pub struct ProtocolStateMachine {
    pub current_state: ProtocolState,
    pub allowed_transitions: HashMap<(ProtocolState, u64), ProtocolState>,
}

impl ProtocolStateMachine {
    /// Construct a state machine with the default request/response
    /// transition table (see the module docs above). The machine starts
    /// in [`ProtocolState::Idle`].
    pub fn new_protocol() -> Self {
        let mut allowed = HashMap::new();
        let send = type_hash("send");
        let sent = type_hash("sent");
        let recv = type_hash("recv");
        let close = type_hash("close");
        // Forward path: a sender initiates, the receiver acknowledges.
        allowed.insert((ProtocolState::Idle, send), ProtocolState::WaitingForSend);
        allowed.insert((ProtocolState::WaitingForSend, sent), ProtocolState::WaitingForRecv);
        allowed.insert((ProtocolState::WaitingForRecv, recv), ProtocolState::Idle);
        // Receiver may also block first (idle → recv → wait for a send).
        allowed.insert((ProtocolState::Idle, recv), ProtocolState::WaitingForRecv);
        // Pipelined reply: a recv blocked in WaitingForRecv can flip
        // straight to WaitingForSend when the peer sends.
        allowed.insert((ProtocolState::WaitingForRecv, send), ProtocolState::WaitingForSend);
        // Close is permitted from any live state.
        allowed.insert((ProtocolState::Idle, close), ProtocolState::Closed);
        allowed.insert((ProtocolState::WaitingForSend, close), ProtocolState::Closed);
        allowed.insert((ProtocolState::WaitingForRecv, close), ProtocolState::Closed);
        Self {
            current_state: ProtocolState::Idle,
            allowed_transitions: allowed,
        }
    }

    /// Check whether `type_hash` is a legal transition out of the
    /// current state. On success, advances the machine to the new state
    /// and returns it. On failure, leaves the machine in its current
    /// state and returns an error — this is deliberate so that a
    /// protocol-violating message does not corrupt the FSM for
    /// subsequent (legitimate) traffic.
    pub fn check_transition(&mut self, type_hash: u64) -> Result<ProtocolState, ProtocolError> {
        if self.current_state == ProtocolState::Closed {
            return Err(ProtocolError::Closed);
        }
        let key = (self.current_state.clone(), type_hash);
        match self.allowed_transitions.get(&key) {
            Some(new_state) => {
                self.current_state = new_state.clone();
                Ok(new_state.clone())
            }
            None => Err(ProtocolError::ProtocolViolation {
                expected: self.current_state.clone(),
                got: type_hash,
            }),
        }
    }
}

impl Default for ProtocolStateMachine {
    fn default() -> Self {
        Self::new_protocol()
    }
}

// ── L5-L8: Worker, State, Error, Crypto (stubs for now) ────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerConfig {
    pub trust_level: TrustLevel,
    pub max_restarts: u32,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustLevel {
    Kernel,
    Verified,
    Untrusted,
    Sandboxed,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            trust_level: TrustLevel::Untrusted,
            max_restarts: 3,
            timeout_ms: 10000,
        }
    }
}



// ── L5: Worker Encapsulation (seccomp sandboxing) ────────────────────

/// Allowed syscalls for each trust level.
pub fn allowed_syscalls(trust: &TrustLevel) -> Vec<u32> {
    match trust {
        TrustLevel::Kernel => (0..512).collect(), // all syscalls
        TrustLevel::Verified => vec![
            0, 1, 2, 3, 9, 10, 11, 12, 13, 14,  // read, write, open, close, mmap, mprotect, munmap, brk, rt_sigaction, rt_sigprocmask
            22, 39, 56, 57, 59, 60, 61, 62,  // pipe, getpid, clone, fork, execve, exit, wait4, kill
            63, 64, 72, 78, 79, 80,  // read, write, getcwd, getdents, getcwd, chdir
            89, 90, 97,  // readlink, getuid, getrlimit
            102, 107, 108,  // getuid, geteuid, getgid
            202, 257,  // futex, openat
        ],
        TrustLevel::Untrusted => vec![0, 1, 3, 9, 12, 60],  // read, write, close, mmap, brk, exit
        TrustLevel::Sandboxed => vec![60],  // exit only
    }
}

/// Generate a seccomp BPF filter for the given trust level.
pub fn generate_seccomp_filter(config: &WorkerConfig) -> Vec<u8> {
    let allowed = allowed_syscalls(&config.trust_level);
    let mut filter = Vec::new();
    // BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(seccomp_data, nr))
    filter.extend_from_slice(&[0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    for nr in &allowed {
        // BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, nr, 0, 1)
        filter.extend_from_slice(&[0x15, 0x00, 0x01, 0x00]);
        filter.extend_from_slice(&nr.to_le_bytes());
        // BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW)
        filter.extend_from_slice(&[0x06, 0x00, 0x00, 0x00, 0x7f, 0x00, 0x00, 0x00]);
    }
    // BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL)
    filter.extend_from_slice(&[0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    filter
}

// ── L5 (cont.): ResourceLimits + WorkerSandbox ───────────────────────

/// Measured resource usage of a worker — the right-hand side of
/// [`ResourceLimits::check_limits`]. Each field is the current observed
/// value as reported by `getrusage(2)` (cpu_time_ms, memory_bytes), the
/// IPC layer (ipc_messages), or `/proc/self/fd` (file_descriptors).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceUsage {
    /// Wall-clock CPU time consumed, in milliseconds.
    pub cpu_time_ms: u64,
    /// Peak resident-set size, in bytes.
    pub memory_bytes: u64,
    /// Cumulative count of IPC messages sent + received.
    pub ipc_messages: u64,
    /// Number of currently-open file descriptors.
    pub file_descriptors: u64,
}

/// Per-worker resource ceilings. When [`ResourceLimits::check_limits`]
/// returns false the supervisor terminates the worker and, if
/// [`should_restart`] agrees, respawns it within the `max_restarts`
/// budget from [`WorkerConfig`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Max CPU time in milliseconds (0 = unlimited).
    pub cpu_time_ms: u64,
    /// Max resident memory in bytes (0 = unlimited).
    pub max_memory_bytes: u64,
    /// Max cumulative IPC messages (0 = unlimited).
    pub max_ipc_messages: u64,
    /// Max open file descriptors (0 = unlimited).
    pub max_file_descriptors: u64,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_time_ms: 5_000,                    // 5 s CPU budget
            max_memory_bytes: 256 * 1024 * 1024,   // 256 MiB RSS
            max_ipc_messages: 10_000,
            max_file_descriptors: 64,
        }
    }
}

impl ResourceLimits {
    /// Returns true iff every measured resource in `usage` is at or
    /// below its configured ceiling. A ceiling of `0` disables that
    /// particular check (treated as "unlimited"), so a zeroed
    /// [`ResourceUsage`] always passes against any limits.
    ///
    /// This is the L5 resource-limit half of the sandbox: the supervisor
    /// polls it between IPC turns and kills the worker if it returns
    /// false, mirroring the seccomp filter's syscall-level containment.
    pub fn check_limits(&self, usage: &ResourceUsage) -> bool {
        if self.cpu_time_ms != 0 && usage.cpu_time_ms > self.cpu_time_ms {
            return false;
        }
        if self.max_memory_bytes != 0 && usage.memory_bytes > self.max_memory_bytes {
            return false;
        }
        if self.max_ipc_messages != 0 && usage.ipc_messages > self.max_ipc_messages {
            return false;
        }
        if self.max_file_descriptors != 0 && usage.file_descriptors > self.max_file_descriptors {
            return false;
        }
        true
    }
}

/// A fully-prepared L5 sandbox for one worker: the seccomp BPF program
/// derived from `config.trust_level` (via [`generate_seccomp_filter`])
/// plus the [`ResourceLimits`] the supervisor enforces on top.
///
/// This is the object the runtime constructs when it evaluates a
/// `spawn_worker("path")` call site: the parent builds the sandbox,
/// `fork()`s, and the child calls [`WorkerSandbox::apply`] before
/// `exec()`-ing the worker binary so the filter is in force before any
/// worker code runs.
#[derive(Clone, Debug)]
pub struct WorkerSandbox {
    pub config: WorkerConfig,
    pub limits: ResourceLimits,
}

impl WorkerSandbox {
    pub fn new(config: WorkerConfig, limits: ResourceLimits) -> Self {
        Self { config, limits }
    }

    /// The BPF bytecode that [`apply`] installs. Exposed so callers and
    /// tests can inspect the generated program (length, structure,
    /// allowed-syscall count) without trapping themselves.
    pub fn seccomp_filter(&self) -> Vec<u8> {
        generate_seccomp_filter(&self.config)
    }

    /// Install the seccomp filter on the *current* process.
    ///
    /// On x86_64 Linux this issues the real
    /// `prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)` followed by
    /// `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &sock_fprog, 0, 0)`
    /// via raw `syscall` number 157 (the x86_64 `__NR_prctl`). On any
    /// other target the BPF program is still generated (so
    /// [`seccomp_filter`] remains useful) but the kernel trap is
    /// skipped and `Ok(0)` is returned.
    ///
    /// Returns `Ok(rc)` (rc >= 0) on success or
    /// `Err(IpcError::KernelError)` if the kernel rejected the filter.
    ///
    /// # Safety of the call site
    /// Calling this from the test process on x86_64 Linux would install
    /// a real seccomp filter on the test runner (and a `Sandboxed`
    /// filter would kill it on the next syscall). Unit tests therefore
    /// only exercise [`seccomp_filter`]; `apply` is invoked in
    /// production only from the forked child of `spawn_worker`.
    pub fn apply(&self) -> Result<i32, IpcError> {
        let filter = self.seccomp_filter();
        #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
        {
            // SAFETY: `filter` is a well-formed BPF program (8-byte
            // instructions, length a multiple of 8) produced by
            // `generate_seccomp_filter`. The `sock_fprog` / `sock_filter`
            // structs below match the kernel ABI on x86_64 Linux. The
            // Vec `prog` (and therefore `fprog.filter`) outlives both
            // syscalls because it is dropped only at function return.
            let rc = unsafe { install_seccomp_filter_bpf(&filter) };
            if rc < 0 {
                return Err(IpcError::KernelError);
            }
            Ok(rc)
        }
        #[cfg(not(all(target_arch = "x86_64", target_os = "linux")))]
        {
            // Non-x86_64 / non-Linux targets: the BPF program is still
            // generated and inspectable via seccomp_filter(); the kernel
            // prctl is x86_64-Linux only.
            let _ = filter.len();
            Ok(0)
        }
    }
}

/// Issue `prctl(PR_SET_NO_NEW_PRIVS, 1)` then
/// `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &fprog)` via the raw
/// x86_64 syscall ABI (nr 157). Returns the raw kernel return value:
/// `0` on success, a negative `-errno` on failure.
///
/// # Safety
/// `filter` must be a well-formed BPF program whose length is a
/// multiple of 8 (each instruction is 8 bytes). Callers must ensure the
/// program is safe to install on the current process.
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
unsafe fn install_seccomp_filter_bpf(filter: &[u8]) -> i32 {
    // Kernel ABI: struct sock_filter { u16 code; u8 jt; u8 jf; u32 k; }
    //             struct sock_fprog  { u16 len; struct sock_filter __user *filter; }
    #[repr(C)]
    struct SockFilter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }
    #[repr(C)]
    struct SockFprog {
        len: u16,
        filter: *const SockFilter,
    }

    const PR_SET_NO_NEW_PRIVS: usize = 38;
    const PR_SET_SECCOMP: usize = 22;
    const SECCOMP_MODE_FILTER: usize = 2;
    const SYS_PRCTL: usize = 157; // __NR_prctl on x86_64

    let mut prog: Vec<SockFilter> = Vec::with_capacity(filter.len() / 8);
    for chunk in filter.chunks_exact(8) {
        prog.push(SockFilter {
            code: u16::from_le_bytes([chunk[0], chunk[1]]),
            jt: chunk[2],
            jf: chunk[3],
            k: u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
        });
    }
    let fprog = SockFprog {
        len: prog.len() as u16,
        filter: prog.as_ptr(),
    };

    let mut rc: i64;

    // prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) — required before
    // SECCOMP_MODE_FILTER for unprivileged processes; a no-op for root.
    //
    // SAFETY: syscall ABI on x86_64 — rax=nr, rdi/rsi/rdx/r10/r8/r9 are
    // args 1-6, return in rax, rcx and r11 are clobbered.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_PRCTL => rc,
            in("rdi") PR_SET_NO_NEW_PRIVS,
            in("rsi") 1usize,
            in("rdx") 0usize,
            in("r10") 0usize,
            in("r8")  0usize,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    if rc < 0 {
        return rc as i32; // -errno
    }

    // prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &fprog, 0, 0)
    let fprog_ptr: *const SockFprog = &fprog;
    // SAFETY: same ABI as above; `fprog` (and the `prog` Vec it points
    // into) are alive until function return, so the kernel sees a valid
    // sock_fprog during the syscall.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_PRCTL => rc,
            in("rdi") PR_SET_SECCOMP,
            in("rsi") SECCOMP_MODE_FILTER,
            in("rdx") fprog_ptr as usize,
            in("r10") 0usize,
            in("r8")  0usize,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    rc as i32
}

/// Decide whether a worker that exited with `exit_code` should be
/// restarted, given its [`WorkerConfig`].
///
/// # Policy
///   * A clean exit (`exit_code == 0`) is terminal — the worker
///     finished its job and must not be restarted.
///   * A config with `max_restarts == 0` disables the restart policy
///     entirely; any exit (clean or crash) is terminal.
///   * Any other exit code — a positive non-zero status from `exit(n)`,
///     or a negative value as reported by `waitpid(2)` when `WIFSIGNALED`
///     is true (e.g. `-11` for `SIGSEGV`) — is treated as a crash and is
///     restartable.
///
/// The stateful restart *budget* (how many restarts have already been
/// consumed) is tracked by [`Supervisor`]; this function is the
/// stateless policy decision that feeds `Supervisor::should_restart`.
pub fn should_restart(config: &WorkerConfig, exit_code: i32) -> bool {
    if exit_code == 0 {
        return false;
    }
    if config.max_restarts == 0 {
        return false;
    }
    true
}

// ── L6: State Encapsulation (checkpointing) ──────────────────────────

#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub pid: u64,
    pub channels: Vec<ChannelState>,
    pub timestamp: u64,
    pub integrity_hash: [u8; 32],
}

#[derive(Clone, Debug)]
pub struct ChannelState {
    pub channel_id: u64,
    pub sequence: u64,
    pub protocol_state: ProtocolState,
}

/// Compute the 32-byte integrity hash stamped onto a [`Checkpoint`].
///
/// This is the canonical hash shared by [`checkpoint_state`] (which
/// stamps a fresh checkpoint) and [`restore_state`] (which verifies a
/// received checkpoint before restoring it). Both ends must agree on it
/// byte-for-byte, so it is a free function rather than an inherent
/// method — the verifier reconstructs it independently of the producer.
///
/// # Construction
/// The hash input is the concatenation, per channel, of:
///   * `channel_id`     as little-endian u64 (8 bytes),
///   * `sequence`       as little-endian u64 (8 bytes),
///   * `protocol_state.as_tag()` as UTF-8 bytes,
///   * a single `0xFF` separator.
/// The 32-byte hash is then built as **eight independent CRC32** (IEEE
/// 802.3 — the same primitive L1 uses for frame integrity) values, each
/// computed over `(lane_index, hash_input)`, laid out little-endian.
/// CRC32 is sensitive to every input byte, so unlike the previous
/// XOR-fold (which silently collapsed reorderings and repeated blocks)
/// any single-bit change to any field flips bits in the hash.
pub fn compute_integrity_hash(channels: &[ChannelState]) -> [u8; 32] {
    let mut hash_input: Vec<u8> = Vec::with_capacity(channels.len() * 24);
    for ch in channels {
        hash_input.extend_from_slice(&ch.channel_id.to_le_bytes());
        hash_input.extend_from_slice(&ch.sequence.to_le_bytes());
        hash_input.extend_from_slice(ch.protocol_state.as_tag().as_bytes());
        hash_input.push(0xFF);
    }
    let mut hash = [0u8; 32];
    for lane in 0..8u8 {
        let mut prefixed = Vec::with_capacity(hash_input.len() + 1);
        prefixed.push(lane);
        prefixed.extend_from_slice(&hash_input);
        let crc = crc32(&prefixed);
        hash[(lane as usize) * 4..(lane as usize + 1) * 4]
            .copy_from_slice(&crc.to_le_bytes());
    }
    hash
}

/// Create a checkpoint of the current process state.
///
/// Captures every `(channel_id, sequence, protocol_state)` triple into
/// a [`ChannelState`] and stamps the resulting vector with an
/// [`integrity_hash`] computed by [`compute_integrity_hash`]. The
/// `pid` and `timestamp` fields are left zeroed here — in a deployed
/// runtime they are filled in by the kernel side of the checkpoint
/// syscall (the userspace caller is untrusted and must not be able to
/// forge them), but the hash is computable purely from the channel
/// vector so that [`restore_state`] can validate it without needing
/// kernel metadata.
pub fn checkpoint_state(channels: &[(u64, u64, ProtocolState)]) -> Checkpoint {
    let channel_states: Vec<ChannelState> = channels.iter()
        .map(|(id, seq, state)| ChannelState {
            channel_id: *id,
            sequence: *seq,
            protocol_state: state.clone(),
        })
        .collect();
    let integrity_hash = compute_integrity_hash(&channel_states);
    Checkpoint {
        pid: 0,       // filled by kernel at checkpoint syscall time
        timestamp: 0, // filled by kernel at checkpoint syscall time
        channels: channel_states,
        integrity_hash,
    }
}

/// Restore process state from a previously captured [`Checkpoint`].
///
/// This is the receive side of L6. Before handing the channel vector
/// back to the caller, it recomputes the integrity hash from the
/// checkpoint's `channels` and compares it against the stamped
/// `integrity_hash`. Any mismatch — whether from corruption in
/// transit, a buggy producer, or deliberate tampering — surfaces as
/// [`IpcError::CheckpointIntegrityFailed`] and the checkpoint is
/// refused. On success the caller receives an owned copy of the
/// verified [`ChannelState`] vector, which it can then apply to its
/// live channel table.
///
/// Note that `pid` and `timestamp` are **not** covered by the hash:
/// they are kernel-supplied metadata that the receiver is expected to
/// sanity-check separately (e.g. against its own clock). The hash
/// covers only the restorable userspace state — the channel vector —
/// so that a forged `pid` cannot be hidden behind a valid hash.
pub fn restore_state(checkpoint: &Checkpoint) -> Result<Vec<ChannelState>, IpcError> {
    let recomputed = compute_integrity_hash(&checkpoint.channels);
    if recomputed != checkpoint.integrity_hash {
        return Err(IpcError::CheckpointIntegrityFailed);
    }
    Ok(checkpoint.channels.clone())
}

// ── L7: Error Encapsulation (fault containment) ──────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IpcError {
    // Framing errors
    BadMagic,
    UnsupportedVersion(u16),
    PayloadTooLarge(u64),
    CrcMismatch { expected: u32, actual: u32 },
    TruncatedMessage,

    // Channel errors
    ChannelClosed,
    ChannelFull,
    ChannelEmpty,
    ChannelTimeout,
    ChannelNotFound(u64),
    PermissionDenied,

    // Type errors
    TypeMismatch { expected: u64, actual: u64 },
    InvalidMessageType,
    DeserializationError,

    // Capability errors
    CapabilityNotFound,
    CapabilityRevoked,
    CapabilityExpired,
    DelegationDepthExceeded,
    InvalidCapabilitySignature,

    // Worker errors
    WorkerCrashed(i32),
    WorkerTimeout,
    WorkerNotFound,
    MaxRestartsExceeded,

    // Memory errors
    MemoryWindowNotFound,
    MemoryWindowRevoked,
    MemoryWindowPermissionDenied,

    // Protocol errors
    ProtocolViolation { expected: String, got: String },
    UnexpectedMessage,
    MissingRequiredCapability,

    // Information flow (v2)
    InformationFlowViolation { source: u64, target: u64 },
    UnauthorizedDowngrade,

    // STARK (v2)
    StarkProofInvalid,
    StarkProofExpired,

    // System
    TooManyProcesses,
    TooManyChannels,
    OutOfMemory,
    KernelError,

    // Checkpoint (L6)
    /// The integrity hash stamped on a [`Checkpoint`] does not match the
    /// hash recomputed from its `channels` vector. Raised by
    /// [`restore_state`] when the receive side detects corruption or
    /// tampering before applying the checkpoint.
    CheckpointIntegrityFailed,
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for IpcError {}

/// Concrete fault observed by the supervisor when reaping a worker
/// process. This is the input to [`handle_worker_error`], which maps
/// it to a [`RecoveryAction`].
///
/// Field semantics follow the **shell convention** for `waitpid(2)`:
///   * `exit_code` is `WEXITSTATUS(status)` (0–255) when `WIFEXITED` is
///     true, **or** `128 + WTERMSIG(status)` when `WIFSIGNALED` is
///     true. The shell convention (used by `bash`, `sh`, `git`, etc.)
///     encodes the signal in the exit code so that `exit_code == 0`
///     unambiguously means a clean `exit(0)` — a signal death always
///     has `exit_code >= 128`. This lets [`handle_worker_error`]
///     decide on `exit_code` alone for the Terminate arm, without a
///     separate `signal == 0` check.
///   * `signal` is `WTERMSIG(status)` when `WIFSIGNALED` is true, and
///     0 otherwise. 11 = `SIGSEGV`, 9 = `SIGKILL`, 6 = `SIGABRT`.
///   * `stderr_capture` is the tail of the worker's stderr pipe,
///     truncated to a reasonable size for inclusion in crash reports.
///   * `timestamp` is a monotonic clock reading at reap time, in
///     milliseconds since some fixed epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerError {
    /// `WEXITSTATUS(status)` for a normal exit, or `128 + WTERMSIG(status)`
    /// for a signal death (shell convention). `0` iff the worker called
    /// `exit(0)` — a signal death always has `exit_code >= 128`.
    pub exit_code: i32,
    /// `WTERMSIG(status)` if the worker was killed by a signal; 0 if
    /// it exited normally. 11 = `SIGSEGV`, 9 = `SIGKILL`, 6 = `SIGABRT`.
    pub signal: i32,
    /// Captured tail of the worker's stderr (UTF-8, lossy). Bounded by
    /// the supervisor's ring buffer so a chatty worker cannot OOM it.
    pub stderr_capture: String,
    /// Monotonic timestamp (ms) at which the supervisor reaped the worker.
    pub timestamp: u64,
}

/// Action the supervisor takes in response to a [`WorkerError`].
///
/// The three-valued enum is the L7 fault-containment contract: every
/// worker fault is classified into exactly one of these, and the
/// supervisor's state machine branches on it. There is no fourth
/// "ignore" arm — every fault gets a disposition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Restart the worker from its last good checkpoint. The supervisor
    /// still has to consult its restart budget (how many restarts have
    /// already been consumed in the current window) before acting on
    /// this; the budget check is stateful and lives in the
    /// `Supervisor`, not in this stateless policy function.
    Restart,
    /// The worker exited cleanly and should not be restarted. Its slot
    /// is freed and its last checkpoint (if any) is discarded.
    Terminate,
    /// The fault is outside the restart policy — unknown signal,
    /// non-zero exit code, or `SIGSEGV` with no restart budget.
    /// Escalate to the parent supervisor (or, if there is none, abort
    /// the job) rather than silently spinning on a restart loop.
    Escalate,
}

/// L7 fault-containment policy.
///
/// Maps a [`WorkerError`] to a [`RecoveryAction`] using the worker's
/// [`WorkerConfig`]. The policy is deliberately small and explicit so
/// that the supervisor's behaviour is auditable:
///
///   * A `SIGSEGV` (signal 11) with a non-zero `max_restarts` budget
///     is the canonical crash-restart case → [`RecoveryAction::Restart`].
///     SIGSEGV is singled out because it is the signature of a
///     transient memory-safety fault (use-after-free, null deref) that
///     a fresh process is likely to survive.
///   * A clean exit (`exit_code == 0`) is terminal — a worker that
///     called `exit(0)` finished its job and must not be restarted.
///     Because `exit_code` follows the shell convention (128 + signal
///     for signal deaths, see [`WorkerError`]), `exit_code == 0`
///     unambiguously means a clean `exit(0)`; a signal death always
///     has `exit_code >= 128` and so falls through to Escalate.
///   * Anything else (timeouts, `SIGKILL`, non-zero exit codes,
///     `SIGSEGV` with `max_restarts == 0`) is escalated rather than
///     silently restarted — `SIGKILL` typically means the OOM killer,
///     a non-zero exit means the worker detected an unrecoverable
///     invariant violation, and a `SIGSEGV` with no budget means the
///     restart policy has been exhausted.
///
/// This is the stateless decision; the stateful restart *budget* (how
/// many restarts have already been consumed in the current window) is
/// enforced by the `Supervisor` before honouring a `Restart`. It
/// complements the older [`should_restart`] predicate, which is the
/// same policy expressed as a bool on the raw exit code for callers
/// that do not yet have a full [`WorkerError`].
///
/// # Examples
/// ```
/// # use vuma_codegen::ipc::{WorkerError, RecoveryAction, handle_worker_error, WorkerConfig};
/// let err = WorkerError {
///     exit_code: 139, signal: 11, // 128 + SIGSEGV(11), shell convention
///     stderr_capture: String::from("segfault at 0x0"),
///     timestamp: 1_000,
/// };
/// let cfg = WorkerConfig { max_restarts: 3, ..Default::default() };
/// assert_eq!(handle_worker_error(&err, &cfg), RecoveryAction::Restart);
/// ```
pub fn handle_worker_error(error: &WorkerError, config: &WorkerConfig) -> RecoveryAction {
    const SIGSEGV: i32 = 11;
    if error.signal == SIGSEGV && config.max_restarts > 0 {
        return RecoveryAction::Restart;
    }
    if error.exit_code == 0 {
        return RecoveryAction::Terminate;
    }
    RecoveryAction::Escalate
}

// ── L8: Cryptographic Encapsulation (AEAD) ───────────────────────────

/// Per-message cryptographic state for the L8 layer.
///
/// # SECURITY WARNING — NOT CRYPTOGRAPHICALLY SECURE
///
/// This struct implements a **structurally correct AEAD frame**, not a
/// secure AEAD. It exists so that the rest of the IPC stack can be
/// built and tested against a real encrypt / decrypt / tag-verify
/// contract without pulling in a vetted crypto crate (which is a
/// deliberate non-goal of the current review wave).
///
/// Concretely:
///   * The cipher is a **XOR stream** built by repeating the 32-byte
///     key to the plaintext length and XOR-folding each byte with a
///     per-message nonce byte. This is trivially malleable and leaks
///     any plaintext structure that is repeated across nonce reuse.
///   * The tag is a **CRC32** of the ciphertext. CRC32 is a linear
///     integrity check, not a MAC: it detects accidental corruption
///     (bit-flips, truncation) but is forgeable by anyone who knows
///     the ciphertext.
///
/// Both halves are deliberately *real* — they actually transform the
/// plaintext (the ciphertext differs from the plaintext) and actually
/// detect single-bit tampering (the tag is a genuine CRC32, not a
/// zero placeholder) — so that downstream code, e.g. [`NoiseChannel::send`],
/// exercises a genuine encrypt→decrypt round-trip rather than a
/// pass-through stub. But **no production deployment may rely on this
/// for confidentiality or authenticity**.
///
/// Replacing this with a real AEAD (ChaCha20-Poly1305 or AES-GCM-SIV)
/// is a drop-in: keep the wire layout (8-byte nonce ‖ ciphertext ‖
/// tag) and swap the stream cipher and the 4-byte CRC tag for a
/// 16-byte Poly1305/GCM tag. The [`encrypt`] / [`decrypt`] signatures
/// do not change.
///
/// # Wire layout
/// ```text
///   ┌────────────┬──────────────────────┬──────────┐
///   │ nonce (8B) │ ciphertext (N bytes) │ tag (4B) │
///   └────────────┴──────────────────────┴──────────┘
/// ```
/// `nonce` is the little-endian `nonce_counter` at encrypt time. The
/// counter is monotonic per `CryptoState` instance and MUST NOT wrap
/// within a single channel's lifetime — doing so would reuse a
/// key/nonce pair, which is catastrophic for any real stream cipher.
pub struct CryptoState {
    pub key: [u8; 32],
    pub nonce_counter: u64,
}

/// Type alias retaining the conceptual AEAD-cipher name used in the
/// L8 design notes. [`CryptoState`] and `AeadCipher` are the same
/// type; downstream code may use whichever name is clearer at the
/// call site.
pub type AeadCipher = CryptoState;

impl CryptoState {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key, nonce_counter: 0 }
    }

    /// Build the key stream for a message of length `len`, keyed by
    /// `nonce`.
    ///
    /// The stream is the 32-byte key repeated to `len` bytes, each byte
    /// XOR-folded with `nonce[i % 8]` so that distinct nonces yield
    /// distinct streams (i.e. the same plaintext encrypted under two
    /// different nonces produces two different ciphertexts). This is
    /// NOT a secure key-derivation function — it is the minimum
    /// construction that makes the encrypt/decrypt round-trip real and
    /// makes nonce reuse visibly wrong.
    fn key_stream(&self, len: usize, nonce: &[u8; 8]) -> Vec<u8> {
        let mut stream = Vec::with_capacity(len);
        for i in 0..len {
            stream.push(self.key[i % 32] ^ nonce[i % 8]);
        }
        stream
    }

    /// Encrypt `plaintext` under this state's key and the current
    /// `nonce_counter`.
    ///
    /// Returns a fresh allocation laid out as `nonce ‖ ciphertext ‖
    /// tag` (see the struct doc). The `nonce_counter` is then
    /// incremented so the next call uses a fresh nonce. The ciphertext
    /// is exactly as long as the plaintext — there is no padding,
    /// which is correct for a stream cipher.
    ///
    /// Tag computation: `tag = crc32(ciphertext).to_le_bytes()`. The
    /// tag is computed *over the ciphertext*, not the plaintext, so
    /// the receiver can verify it before running the inverse XOR.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let nonce = self.nonce_counter.to_le_bytes();
        let key_stream = self.key_stream(plaintext.len(), &nonce);
        let ciphertext: Vec<u8> = plaintext
            .iter()
            .zip(key_stream.iter())
            .map(|(p, k)| p ^ k)
            .collect();
        let tag = crc32(&ciphertext).to_le_bytes();

        let mut output = Vec::with_capacity(8 + ciphertext.len() + 4);
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&ciphertext);
        output.extend_from_slice(&tag);
        self.nonce_counter = self.nonce_counter.wrapping_add(1);
        output
    }

    /// Decrypt a frame produced by [`encrypt`].
    ///
    /// Verifies the trailing 4-byte CRC32 tag against the ciphertext
    /// *before* running the inverse XOR — so a tampered frame is
    /// rejected without ever revealing decrypted bytes to the caller.
    /// On tag mismatch the error is [`IpcError::CrcMismatch`] (the
    /// same variant L1 uses for frame integrity), carrying the
    /// expected and observed tag values for diagnostics. Frames too
    /// short to contain even an empty ciphertext (8-byte nonce + 4-byte
    /// tag = 12 bytes) are rejected as [`IpcError::DeserializationError`].
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, IpcError> {
        // Minimum frame: 8-byte nonce + 0-byte ciphertext + 4-byte tag.
        if ciphertext.len() < 12 {
            return Err(IpcError::DeserializationError);
        }
        let nonce_bytes = &ciphertext[..8];
        let ct_end = ciphertext.len() - 4;
        let ct = &ciphertext[8..ct_end];
        let tag_bytes = &ciphertext[ct_end..];

        let mut nonce = [0u8; 8];
        nonce.copy_from_slice(nonce_bytes);

        // Tag verification FIRST — never decrypt before verifying.
        let expected = crc32(ct);
        let actual = u32::from_le_bytes([
            tag_bytes[0],
            tag_bytes[1],
            tag_bytes[2],
            tag_bytes[3],
        ]);
        if expected != actual {
            return Err(IpcError::CrcMismatch { expected, actual });
        }

        let key_stream = self.key_stream(ct.len(), &nonce);
        let plaintext: Vec<u8> = ct
            .iter()
            .zip(key_stream.iter())
            .map(|(c, k)| c ^ k)
            .collect();
        Ok(plaintext)
    }
}

// ── FFI Process Isolation (extern "process") ─────────────────────────

/// Configuration for an FFI worker process.
#[derive(Clone, Debug)]
pub struct FfiWorkerConfig {
    pub library_path: String,
    pub function_name: String,
    pub trust_level: TrustLevel,
    pub max_restarts: u32,
    pub timeout_ms: u64,
}

impl Default for FfiWorkerConfig {
    fn default() -> Self {
        Self {
            library_path: String::new(),
            function_name: String::new(),
            trust_level: TrustLevel::Untrusted,
            max_restarts: 3,
            timeout_ms: 10000,
        }
    }
}

/// Marshal a function call for IPC transport.
/// The worker process receives this and calls the actual C function.
#[derive(Clone, Debug)]
pub struct FfiCall {
    pub function_name: String,
    pub args: Vec<Vec<u8>>,  // serialized arguments
    pub return_type_hash: u64,
}

/// Result of an FFI call from a worker process.
#[derive(Clone, Debug)]
pub struct FfiResult {
    pub success: bool,
    pub return_value: Vec<u8>,
    pub error: Option<String>,
}

// ── Capability Delegation ────────────────────────────────────────────

impl capability::CapabilitySet {
    /// Delegate a capability to another process with reduced permissions.
    pub fn delegate(
        &mut self,
        parent_id: u128,
        new_target_pid: u64,
        subset_perms: capability::MemoryPermissions,
        signing_key: &[u8; 32],
    ) -> Result<capability::CapabilityToken, IpcError> {
        let parent = self.tokens.get(&parent_id)
            .ok_or(IpcError::CapabilityNotFound)?
            .clone();

        if parent.delegation_depth >= 8 {
            return Err(IpcError::DelegationDepthExceeded);
        }

        // Verify subset
        if subset_perms.read && !parent.permissions.read {
            return Err(IpcError::PermissionDenied);
        }
        if subset_perms.write && !parent.permissions.write {
            return Err(IpcError::PermissionDenied);
        }
        if subset_perms.execute && !parent.permissions.execute {
            return Err(IpcError::PermissionDenied);
        }

        let mut child = capability::CapabilityToken {
            id: rand_u128(),
            source_pid: parent.target_pid, // delegator becomes source
            target_pid: new_target_pid,
            resource: parent.resource.clone(),
            permissions: subset_perms,
            delegation_depth: parent.delegation_depth + 1,
            created_at: 0, // delegate does not yet receive a wall-clock; see grant_capability
            expires_at: parent.expires_at,
            signature: [0u8; 32],
        };
        // Sign the delegated child so verify_capability can validate it
        // like any other token. NOTE: this is the same FNV-1a-based
        // non-cryptographic digest used by grant_capability — see the
        // SECURITY NOTE in the capability module above.
        child.signature = capability::compute_signature(&child, signing_key);

        self.tokens.insert(child.id, child.clone());
        Ok(child)
    }
}

fn rand_u128() -> u128 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut hasher);
    hasher.finish() as u128
}

// ── Supervisor (Fault Tolerance) ─────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Supervisor {
    pub max_restarts: u32,
    pub timeout_ms: u64,
    pub restart_count: u32,
}

impl Supervisor {
    pub fn new(max_restarts: u32, timeout_ms: u64) -> Self {
        Self { max_restarts, timeout_ms, restart_count: 0 }
    }

    pub fn should_restart(&mut self) -> bool {
        if self.restart_count >= self.max_restarts {
            return false;
        }
        self.restart_count += 1;
        true
    }

    pub fn reset(&mut self) {
        self.restart_count = 0;
    }
}

// ── Hot Reloading ────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct HotSwapRequest {
    pub worker_pid: u64,
    pub new_binary_path: String,
    pub transfer_state: bool,
}

#[derive(Clone, Debug)]
pub struct HotSwapResult {
    pub success: bool,
    pub new_pid: u64,
    pub state_transferred: bool,
}

// ── Distributed Channels ─────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct RemoteWorker {
    pub addr: String,
    pub port: u16,
    pub connected: bool,
}

/// Noise Protocol Framework handshake state (stub).
/// Real implementation would use the Noise_XX pattern.
#[derive(Clone, Debug)]
pub struct NoiseChannel {
    pub local_static_key: [u8; 32],
    pub remote_static_key: Option<[u8; 32]>,
    pub handshake_complete: bool,
    pub cipher_key: [u8; 32],
}

impl NoiseChannel {
    pub fn new(local_key: [u8; 32]) -> Self {
        Self {
            local_static_key: local_key,
            remote_static_key: None,
            handshake_complete: false,
            cipher_key: [0u8; 32],
        }
    }

    /// Initiate Noise_XX handshake (stub).
    pub fn initiate(&mut self, remote_addr: &str) -> Result<(), IpcError> {
        // Real implementation: exchange ephemeral + static keys
        self.handshake_complete = true;
        Ok(())
    }

    /// Send encrypted message over Noise channel.
    pub fn send(&mut self, msg: &[u8]) -> Result<Vec<u8>, IpcError> {
        if !self.handshake_complete {
            return Err(IpcError::PermissionDenied);
        }
        let mut crypto = CryptoState::new(self.cipher_key);
        Ok(crypto.encrypt(msg))
    }
}

// ── Compile-Time Encapsulation (stubs) ───────────────────────────────

/// Session type for compile-time protocol verification.
/// CT1: Session Types (arxiv 2510.19129)
#[derive(Clone, Debug)]
pub enum SessionType {
    End,
    Send(u64, Box<SessionType>),  // type_hash, rest
    Recv(u64, Box<SessionType>),
    Choice(Box<SessionType>, Box<SessionType>),
    Loop(Box<SessionType>),
}

impl SessionType {
    /// Compute the dual (other end's perspective).
    pub fn dual(&self) -> SessionType {
        match self {
            SessionType::End => SessionType::End,
            SessionType::Send(t, rest) => SessionType::Recv(*t, Box::new(rest.dual())),
            SessionType::Recv(t, rest) => SessionType::Send(*t, Box::new(rest.dual())),
            SessionType::Choice(a, b) => SessionType::Choice(Box::new(b.dual()), Box::new(a.dual())),
            SessionType::Loop(body) => SessionType::Loop(Box::new(body.dual())),
        }
    }
}

/// Security label for information-flow control.
/// CT2: Information-Flow Types (arxiv 2210.12996)
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecurityLabel {
    Public,
    Internal,
    Secret,
    TopSecret,
}

impl SecurityLabel {
    pub fn can_flow_to(self, target: SecurityLabel) -> bool {
        self <= target
    }

    pub fn join(self, other: SecurityLabel) -> SecurityLabel {
        if self >= other { self } else { other }
    }
}

/// zk-STARK proof (stub).
/// CT6: zk-STARK Attestation (arxiv 2512.10020)
#[derive(Clone, Debug)]
pub struct StarkProof {
    pub proof_data: Vec<u8>,
    pub public_inputs: Vec<u64>,
    pub validity_window: u64,
}

#[derive(Clone, Debug)]
pub struct CapabilityAttestation {
    pub proof: StarkProof,
    pub worker_pid: u64,
    pub capability_count: u64,
    pub commitment_hash: u64,
}

impl CapabilityAttestation {
    /// Verify the STARK proof (stub: always succeeds).
    pub fn verify(&self, expected_pid: u64) -> Result<(), IpcError> {
        if self.worker_pid != expected_pid {
            return Err(IpcError::StarkProofInvalid);
        }
        Ok(())
    }
}

/// Fractional permission for concurrent access.
/// CT7: CSL-Perm
#[derive(Clone, Copy, Debug)]
pub struct Permission {
    pub fraction: f64,
}

impl Permission {
    pub fn full() -> Self { Self { fraction: 1.0 } }
    pub fn split(self) -> (Self, Self) {
        (Self { fraction: self.fraction / 2.0 }, Self { fraction: self.fraction / 2.0 })
    }
    pub fn merge(a: Self, b: Self) -> Self {
        Self { fraction: a.fraction + b.fraction }
    }
    pub fn can_write(&self) -> bool { self.fraction >= 1.0 }
    pub fn can_read(&self) -> bool { self.fraction > 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc32() {
        assert_eq!(crc32(b""), 0);  // !0xFFFFFFFF == 0
        assert_ne!(crc32(b"VUMA"), 0);
    }

    #[test]
    fn test_crc32_known_vector() {
        // Canonical CRC32 (IEEE 802.3 / zlib) check value for "123456789".
        // See e.g. https://reveng.sourceforge.io/crc-catalogue/17plus.htm#crc.cat.crc-32
        assert_eq!(crc32(b"123456789"), 0xCBF43926);
    }

    #[test]
    fn test_type_hash() {
        let h1 = type_hash_str("i32");
        let h2 = type_hash_str("i32");
        let h3 = type_hash_str("i64");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_type_hash_canonical_matches_alias() {
        // The new canonical name and the legacy alias must agree.
        assert_eq!(type_hash("String"), type_hash_str("String"));
        assert_eq!(type_hash(""), 0xcbf29ce484222325);
    }

    #[test]
    fn test_type_hash_fnv1a_vector() {
        // FNV-1a 64-bit of "foobar" — well-known reference value.
        // Computed independently: 0x85944171f73967e8
        assert_eq!(type_hash("foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn test_frame_deframe() {
        let msg = EncapsulatedMessage::new(1, 0, type_hash_str("i32"), vec![42, 0, 0, 0]);
        let framed = frame_message(&msg);
        assert!(framed.len() > HEADER_SIZE);
        assert_eq!(&framed[0..4], &MAGIC);
        // Verify CRC
        let crc = u32::from_le_bytes(framed[framed.len()-4..].try_into().unwrap());
        assert_eq!(crc, crc32(&framed[..framed.len()-4]));

        // Roundtrip via deframe_message.
        let decoded = deframe_message(&framed).expect("deframe should succeed");
        assert_eq!(decoded.header.magic, MAGIC);
        assert_eq!(decoded.header.version, PROTOCOL_VERSION);
        assert_eq!(decoded.header.channel_id, 1);
        assert_eq!(decoded.header.sequence, 0);
        assert_eq!(decoded.header.type_hash, type_hash_str("i32"));
        assert_eq!(decoded.header.payload_len, 4);
        assert_eq!(decoded.header.cap_count, 0);
        assert_eq!(decoded.payload, vec![42, 0, 0, 0]);
        assert!(decoded.capabilities.is_empty());
    }

    #[test]
    fn test_deframe_roundtrip_with_caps_and_flags() {
        let token = capability::CapabilityToken {
            id: 0xDEAD_BEEF_CAFE_BABE_0123_4567_89AB_CDEF,
            source_pid: 7,
            target_pid: 9,
            resource: capability::Resource::Channel(42),
            permissions: capability::MemoryPermissions {
                read: true,
                write: true,
                execute: false,
            },
            delegation_depth: 1,
            created_at: 1_000,
            expires_at: 2_000,
            signature: [0x5A; 32],
        };

        let mut msg = EncapsulatedMessage::new(
            0xCAFE,
            0xBEEF,
            type_hash("Vec<u8>"),
            (0u32..16).flat_map(|x| x.to_le_bytes()).collect(),
        );
        msg.header.flags = MessageFlags::HAS_CAPS | MessageFlags::ENCRYPTED;
        msg.capabilities = vec![token.clone()];

        let framed = frame_message(&msg);

        // Expected layout: header + payload + 1 cap token + CRC32.
        let expected_len = HEADER_SIZE + msg.payload.len()
            + capability::CAPABILITY_TOKEN_SIZE
            + CRC32_SIZE;
        assert_eq!(framed.len(), expected_len);

        let decoded = deframe_message(&framed).expect("deframe should succeed");
        assert_eq!(decoded.header.channel_id, 0xCAFE);
        assert_eq!(decoded.header.sequence, 0xBEEF);
        assert_eq!(decoded.header.type_hash, type_hash("Vec<u8>"));
        assert_eq!(decoded.header.flags, MessageFlags::HAS_CAPS | MessageFlags::ENCRYPTED);
        assert_eq!(decoded.header.cap_count, 1);
        assert_eq!(decoded.payload, msg.payload);
        assert_eq!(decoded.capabilities.len(), 1);
        assert_eq!(decoded.capabilities[0].id, token.id);
        assert_eq!(decoded.capabilities[0].source_pid, token.source_pid);
        assert_eq!(decoded.capabilities[0].target_pid, token.target_pid);
        assert_eq!(decoded.capabilities[0].permissions.read, true);
        assert_eq!(decoded.capabilities[0].permissions.write, true);
        assert_eq!(decoded.capabilities[0].permissions.execute, false);
        assert_eq!(decoded.capabilities[0].signature, token.signature);
    }

    #[test]
    fn test_deframe_too_short() {
        let too_short = [0u8; HEADER_SIZE]; // header only, no CRC
        assert!(matches!(
            deframe_message(&too_short),
            Err(FrameError::TooShort)
        ));
        assert!(matches!(
            deframe_message(&[]),
            Err(FrameError::TooShort)
        ));
    }

    #[test]
    fn test_deframe_bad_magic() {
        let msg = EncapsulatedMessage::new(1, 0, type_hash("x"), vec![]);
        let mut framed = frame_message(&msg);
        // Corrupt the magic.
        framed[0] = 0xFF;
        let err = deframe_message(&framed).unwrap_err();
        assert!(matches!(err, FrameError::BadMagic { .. }));
    }

    #[test]
    fn test_deframe_bad_version() {
        let msg = EncapsulatedMessage::new(1, 0, type_hash("x"), vec![]);
        let mut framed = frame_message(&msg);
        // Overwrite version field (bytes 4..6) with an unsupported version.
        framed[4..6].copy_from_slice(&(u16::MAX).to_le_bytes());
        let err = deframe_message(&framed).unwrap_err();
        match err {
            FrameError::UnsupportedVersion { expected, actual } => {
                assert_eq!(expected, PROTOCOL_VERSION);
                assert_eq!(actual, u16::MAX);
            }
            other => panic!("expected UnsupportedVersion, got {:?}", other),
        }
    }

    #[test]
    fn test_deframe_bad_crc() {
        let msg = EncapsulatedMessage::new(1, 0, type_hash("x"), vec![1, 2, 3, 4]);
        let mut framed = frame_message(&msg);
        // Flip a payload byte (CRC is over body, so this must mismatch).
        let payload_byte = HEADER_SIZE;
        framed[payload_byte] ^= 0xFF;
        let err = deframe_message(&framed).unwrap_err();
        match err {
            FrameError::CrcMismatch { expected, actual } => {
                assert_ne!(expected, actual);
            }
            other => panic!("expected CrcMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_deframe_truncated_body() {
        let msg = EncapsulatedMessage::new(1, 0, type_hash("x"), vec![0xAB; 32]);
        let framed = frame_message(&msg);
        // Drop the last 4 bytes (CRC) plus one body byte.
        let truncated = &framed[..framed.len() - 5];
        let err = deframe_message(truncated).unwrap_err();
        assert!(matches!(err, FrameError::LengthMismatch { .. }));
    }

    #[test]
    fn test_deframe_extra_trailing_bytes() {
        let msg = EncapsulatedMessage::new(1, 0, type_hash("x"), vec![1, 2, 3]);
        let mut framed = frame_message(&msg);
        framed.push(0xEE); // stray byte
        let err = deframe_message(&framed).unwrap_err();
        assert!(matches!(err, FrameError::LengthMismatch { .. }));
    }

    #[test]
    fn test_deframe_empty_payload_roundtrip() {
        let msg = EncapsulatedMessage::new(0, 0, type_hash("()"), vec![]);
        let framed = frame_message(&msg);
        assert_eq!(framed.len(), HEADER_SIZE + CRC32_SIZE);
        let decoded = deframe_message(&framed).expect("empty payload should roundtrip");
        assert!(decoded.payload.is_empty());
        assert!(decoded.capabilities.is_empty());
        assert_eq!(decoded.header.payload_len, 0);
        assert_eq!(decoded.header.cap_count, 0);
    }

    #[test]
    fn test_frame_message_sets_has_caps_flag_consistency() {
        // When capabilities are present, `frame_message` must auto-set the
        // HAS_CAPS flag in the wire header — the producer does not need to
        // set it manually. This test pins that contract: a message whose
        // header.flags is EMPTY, but whose `capabilities` vec is non-empty,
        // must still deframe with HAS_CAPS set in the recovered header.
        let mut msg = EncapsulatedMessage::new(1, 1, type_hash("T"), vec![0]);
        assert_eq!(msg.header.flags, MessageFlags::EMPTY);

        let cap = capability::CapabilityToken {
            id: 1,
            source_pid: 1,
            target_pid: 2,
            resource: capability::Resource::Memory(0, 0),
            permissions: capability::MemoryPermissions::default(),
            delegation_depth: 0,
            created_at: 0,
            expires_at: 0,
            signature: [0; 32],
        };
        msg.capabilities.push(cap);
        // Deliberately do NOT set HAS_CAPS — frame_message must set it.

        let framed = frame_message(&msg);
        let decoded = deframe_message(&framed).unwrap();
        assert_eq!(decoded.header.flags.bits() & MessageFlags::HAS_CAPS.bits(), MessageFlags::HAS_CAPS.bits());
        assert_eq!(decoded.capabilities.len(), 1);
    }

    #[test]
    fn test_capability_encode_decode() {
        let token = capability::CapabilityToken {
            id: 12345,
            source_pid: 1,
            target_pid: 2,
            resource: capability::Resource::Memory(0x1000, 0x1000),
            permissions: capability::MemoryPermissions { read: true, write: false, execute: false },
            delegation_depth: 0,
            created_at: 1000,
            expires_at: 2000,
            signature: [0xAB; 32],
        };
        let encoded = token.encode();
        assert_eq!(encoded.len(), capability::CAPABILITY_TOKEN_SIZE);
        let decoded = capability::CapabilityToken::decode(&encoded).unwrap();
        assert_eq!(decoded.id, 12345);
        assert_eq!(decoded.source_pid, 1);
        assert_eq!(decoded.target_pid, 2);
        assert!(decoded.permissions.read);
        assert!(!decoded.permissions.write);
        // Wave 11: resource must now round-trip too (previously it was
        // dropped on encode and replaced with Memory(0,0) on decode).
        assert_eq!(decoded.resource, capability::Resource::Memory(0x1000, 0x1000));
        assert_eq!(decoded.signature, [0xAB; 32]);
        assert_eq!(decoded.created_at, 1000);
        assert_eq!(decoded.expires_at, 2000);
        assert_eq!(decoded.delegation_depth, 0);
    }

    // ── Wave 10: Channel-send/recv framing integration tests ───────────
    //
    // The x86_64 backend's `channel_send` / `channel_recv` handlers in
    // `stack_slot_isel.rs` currently emit raw `write(fd, &msg, 8)` /
    // `read(fd, &dst, 8)` syscalls — no framing. Inlining the full L1
    // wire format (header + CRC32) in assembly is impractical, so the
    // plan is to ship a Rust runtime helper (`__vuma_channel_send` /
    // `__vuma_channel_recv`) that wraps the payload with `frame_message`
    // / `deframe_message` and verifies CRC + type hash on the receive
    // side. These four tests pin the contract that those helpers will
    // rely on: they exercise the framing layer with the exact shape of
    // payload a channel op produces (a little-endian i32) and verify
    // roundtrip, CRC mismatch detection, type-hash mismatch detection,
    // and multi-message stream framing.

    /// Helper: build a channel-style EncapsulatedMessage for an i32 payload.
    ///
    /// Mirrors what `__vuma_channel_send(write_fd, buf, count=4,
    /// channel_id, type_hash=type_hash("i32"))` would feed into
    /// `frame_message` — a 4-byte little-endian payload, a per-channel
    /// channel_id, a monotonically increasing sequence number, and the
    /// canonical FNV-1a type hash for "i32".
    fn channel_i32_message(channel_id: u64, sequence: u64, value: i32) -> EncapsulatedMessage {
        EncapsulatedMessage::new(
            channel_id,
            sequence,
            type_hash("i32"),
            value.to_le_bytes().to_vec(),
        )
    }

    #[test]
    fn test_frame_deframe_roundtrip_i32_channel_payload() {
        // Simulate a single `channel_send(ch, 42)` → `channel_recv(ch)`.
        let msg = channel_i32_message(/*channel_id*/ 7, /*sequence*/ 0, /*value*/ 42);
        let framed = frame_message(&msg);

        // Framed layout: 44-byte header + 4-byte payload + 4-byte CRC.
        assert_eq!(framed.len(), HEADER_SIZE + 4 + CRC32_SIZE);
        assert_eq!(&framed[0..4], &MAGIC);

        let decoded = deframe_message(&framed).expect("deframe should succeed");

        // Header fields must survive the roundtrip exactly.
        assert_eq!(decoded.header.channel_id, 7);
        assert_eq!(decoded.header.sequence, 0);
        assert_eq!(decoded.header.type_hash, type_hash("i32"));
        assert_eq!(decoded.header.payload_len, 4);
        assert_eq!(decoded.header.cap_count, 0);

        // Payload must decode back to the original i32 (little-endian).
        assert_eq!(decoded.payload.len(), 4);
        let recovered = i32::from_le_bytes(decoded.payload[..].try_into().unwrap());
        assert_eq!(recovered, 42);
    }

    #[test]
    fn test_frame_deframe_crc_mismatch_detection() {
        // Model the receive side detecting a corrupted frame on the wire.
        let msg = channel_i32_message(11, 0, -1);
        let mut framed = frame_message(&msg);

        // Flip a payload byte — must invalidate the CRC32 trailer.
        let payload_byte = HEADER_SIZE;
        let original_byte = framed[payload_byte];
        framed[payload_byte] ^= 0xFF;
        assert_ne!(framed[payload_byte], original_byte);

        // Recompute the CRC the way `__vuma_channel_recv` would, and
        // confirm it no longer matches the stored trailer.
        let body_end = framed.len() - CRC32_SIZE;
        let stored_crc = u32::from_le_bytes(framed[body_end..].try_into().unwrap());
        let recomputed_crc = crc32(&framed[..body_end]);
        assert_ne!(stored_crc, recomputed_crc);

        // The deframer must reject the corrupted frame with CrcMismatch.
        match deframe_message(&framed) {
            Err(FrameError::CrcMismatch { expected, actual }) => {
                assert_eq!(expected, stored_crc);
                assert_eq!(actual, recomputed_crc);
            }
            other => panic!("expected CrcMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_frame_deframe_type_hash_mismatch_detection() {
        // A producer sends an i32; the consumer expects an i64. The L1
        // deframer accepts any well-formed frame (it has no notion of
        // the *expected* type), so the type-hash mismatch is detected
        // by the recv helper *after* deframing succeeds. This test
        // pins that two-step contract: deframe OK, then type-hash
        // compare fails.
        let sent_value: i32 = 0x1234_5678;
        let msg = channel_i32_message(/*channel_id*/ 3, /*sequence*/ 1, sent_value);
        let framed = frame_message(&msg);

        let decoded = deframe_message(&framed).expect("well-formed frame must deframe");

        // The producer's type hash is for "i32".
        let produced_hash = decoded.header.type_hash;
        assert_eq!(produced_hash, type_hash("i32"));

        // The consumer was compiled to expect an i64 — different hash.
        let expected_hash = type_hash("i64");
        assert_ne!(produced_hash, expected_hash);

        // This is the check `__vuma_channel_recv` would perform:
        //   if decoded.header.type_hash != expected_type_hash { return -1 }
        let type_matches = decoded.header.type_hash == expected_hash;
        assert!(!type_matches, "i32 frame must not satisfy i64 consumer");

        // Sanity: when the consumer expects i32, the same check passes.
        let correct_expected_hash = type_hash("i32");
        assert_eq!(decoded.header.type_hash, correct_expected_hash);

        // And the payload still decodes to the original i32 value.
        let recovered = i32::from_le_bytes(decoded.payload[..].try_into().unwrap());
        assert_eq!(recovered, sent_value);
    }

    #[test]
    fn test_frame_deframe_multi_message_sequence() {
        // Model the existing `multi_message.vuma` gold test (send 3,
        // recv 3 in order: 10, 20, 33) but at the framing layer: three
        // framed messages are concatenated into a single stream (as
        // they would be when written to a pipe by a Rust helper), and
        // the receiver walks the stream one frame at a time.
        let payloads: [(u64, i32); 3] = [
            (0, 10),
            (1, 20),
            (2, 33),
        ];
        let channel_id: u64 = 0xCAFE_F00D;

        // Producer side: frame each message and concatenate.
        let mut stream: Vec<u8> = Vec::new();
        let mut frame_lengths: Vec<usize> = Vec::with_capacity(3);
        for (seq, val) in payloads {
            let msg = channel_i32_message(channel_id, seq, val);
            let framed = frame_message(&msg);
            frame_lengths.push(framed.len());
            stream.extend_from_slice(&framed);
        }

        // Each i32 payload is 4 bytes, so every frame is the same size.
        for &fl in &frame_lengths {
            assert_eq!(fl, HEADER_SIZE + 4 + CRC32_SIZE);
        }

        // Consumer side: walk the stream, deframing one message at a time.
        let mut offset = 0usize;
        let mut received: Vec<i32> = Vec::with_capacity(3);
        for (i, (expected_seq, expected_val)) in payloads.iter().enumerate() {
            let frame_len = frame_lengths[i];
            let frame = &stream[offset..offset + frame_len];
            let decoded = deframe_message(frame).expect("each framed message must deframe");

            // Header fields must match what the producer framed.
            assert_eq!(decoded.header.channel_id, channel_id);
            assert_eq!(decoded.header.sequence, *expected_seq);
            assert_eq!(decoded.header.type_hash, type_hash("i32"));
            assert_eq!(decoded.header.payload_len, 4);

            // Payload must decode to the original i32, in order.
            let val = i32::from_le_bytes(decoded.payload[..].try_into().unwrap());
            assert_eq!(val, *expected_val);
            received.push(val);

            offset += frame_len;
        }

        // All three messages received in order — matches multi_message.vuma
        // (exit code = 10 + 20 + 33 = 63).
        assert_eq!(received, vec![10, 20, 33]);
        assert_eq!(received.iter().map(|&v| v as i64).sum::<i64>(), 63);

        // Stream must be fully consumed (no trailing bytes).
        assert_eq!(offset, stream.len());
    }

    // ── Wave 11: Capability grant/verify/encode/decode tests ───────────
    //
    // The pre-Wave-11 capability module had two stubs:
    //   (a) `CapabilityToken::encode` dropped the `resource` field entirely
    //       and padded with zeros;
    //   (b) `CapabilityToken::decode` always returned `Resource::Memory(0, 0)`
    //       as a "placeholder", so encode→decode was lossy for `resource`.
    // There was also no `grant_capability` / `verify_capability` pair, so
    // tokens had no signature validation path. The tests below pin the new
    // real implementation: every Resource variant round-trips; encode is
    // exactly CAPABILITY_TOKEN_SIZE bytes; short input is rejected; bad
    // resource tags are rejected; grant produces a deterministic signature
    // that verify accepts; and verify rejects every tampering mode
    // (signature, expiry, resource mismatch, insufficient permissions).

    #[test]
    fn test_capability_encode_size_matches_constant() {
        // The framer multiplies cap_count by CAPABILITY_TOKEN_SIZE, so
        // encode() must produce *exactly* that many bytes — no more, no
        // fewer, regardless of which Resource variant is in the token.
        let perms = capability::MemoryPermissions { read: true, write: true, execute: false };
        let cases = vec![
            capability::Resource::File("/etc/passwd".into()),
            capability::Resource::Network("10.0.0.1".into(), 443),
            capability::Resource::Memory(0x1000, 0x2000),
            capability::Resource::Mmio(0xFE00_0000, 0x1000),
            capability::Resource::Channel(0xCAFE),
        ];
        for resource in cases {
            let token = capability::CapabilityToken {
                id: 1,
                source_pid: 1,
                target_pid: 2,
                resource,
                permissions: perms.clone(),
                delegation_depth: 0,
                created_at: 0,
                expires_at: 0,
                signature: [0u8; 32],
            };
            let encoded = token.encode();
            assert_eq!(
                encoded.len(),
                capability::CAPABILITY_TOKEN_SIZE,
                "encode() must produce exactly CAPABILITY_TOKEN_SIZE bytes for every Resource variant"
            );
        }
    }

    #[test]
    fn test_capability_encode_decode_all_resource_variants() {
        // Each Resource variant must survive a full encode→decode round
        // trip with byte-exact equality on every field. This is the test
        // the old stub failed (it always decoded to Memory(0,0)).
        let perms = capability::MemoryPermissions { read: true, write: false, execute: true };
        let cases: Vec<capability::Resource> = vec![
            capability::Resource::File("/var/log/vuma.log".into()),
            capability::Resource::Network("127.0.0.1".into(), 8080),
            capability::Resource::Memory(0xDEAD_BEEF_0000_1000, 0x4000),
            capability::Resource::Mmio(0xFE00_0000, 0x1000),
            capability::Resource::Channel(0x1234_5678_9ABC_DEF0),
        ];
        for (i, resource) in cases.into_iter().enumerate() {
            let token = capability::CapabilityToken {
                id: 100 + i as u128,
                source_pid: 10 + i as u64,
                target_pid: 20 + i as u64,
                resource: resource.clone(),
                permissions: perms.clone(),
                delegation_depth: i as u8,
                created_at: 1000 + i as u64,
                expires_at: 2000 + i as u64,
                signature: [(i as u8 + 1).wrapping_mul(7); 32],
            };
            let encoded = token.encode();
            let decoded = capability::CapabilityToken::decode(&encoded)
                .expect("decode must succeed for a valid encoding");
            // Whole-token equality is the strongest round-trip check.
            assert_eq!(decoded, token, "full token must round-trip for {:?}", resource);
        }
    }

    #[test]
    fn test_capability_decode_too_short() {
        // The deframer hands decode() a slice of exactly
        // CAPABILITY_TOKEN_SIZE bytes; anything shorter must be rejected
        // rather than panicking on slice indexing.
        let short = vec![0u8; capability::CAPABILITY_TOKEN_SIZE - 1];
        let err = capability::CapabilityToken::decode(&short).unwrap_err();
        assert!(err.contains("too short"), "unexpected error message: {}", err);
    }

    #[test]
    fn test_capability_decode_bad_resource_tag() {
        // Forge a token whose resource field has an unknown tag byte.
        // The header fields are valid, but the resource decoder must
        // refuse it rather than silently returning a wrong variant.
        let token = capability::CapabilityToken {
            id: 1,
            source_pid: 1,
            target_pid: 2,
            resource: capability::Resource::Channel(7),
            permissions: capability::MemoryPermissions::default(),
            delegation_depth: 0,
            created_at: 0,
            expires_at: 0,
            signature: [0u8; 32],
        };
        let mut encoded = token.encode();
        // Clobber the resource tag byte (first byte of the resource field).
        encoded[capability::RESOURCE_OFFSET] = 0xFF;
        let err = capability::CapabilityToken::decode(&encoded).unwrap_err();
        assert!(err.contains("unknown resource tag"), "unexpected error: {}", err);
    }

    #[test]
    fn test_capability_decode_truncated_string_in_file_resource() {
        // Forge a File resource whose stored length byte claims more
        // bytes than MAX_RESOURCE_STRING. Decode must reject it.
        let token = capability::CapabilityToken {
            id: 1,
            source_pid: 1,
            target_pid: 2,
            resource: capability::Resource::File("x".into()),
            permissions: capability::MemoryPermissions::default(),
            delegation_depth: 0,
            created_at: 0,
            expires_at: 0,
            signature: [0u8; 32],
        };
        let mut encoded = token.encode();
        // Set the length byte just past the cap.
        encoded[capability::RESOURCE_OFFSET + 1] = (capability::MAX_RESOURCE_STRING + 1) as u8;
        let err = capability::CapabilityToken::decode(&encoded).unwrap_err();
        assert!(err.contains("exceeds"), "unexpected error: {}", err);
    }

    #[test]
    fn test_capability_perms_contains() {
        let rwx = capability::MemoryPermissions { read: true, write: true, execute: true };
        let r_only = capability::MemoryPermissions { read: true, write: false, execute: false };
        let rw = capability::MemoryPermissions { read: true, write: true, execute: false };
        let none = capability::MemoryPermissions { read: false, write: false, execute: false };

        // rwx contains every subset.
        assert!(rwx.contains(&rwx));
        assert!(rwx.contains(&r_only));
        assert!(rwx.contains(&rw));
        assert!(rwx.contains(&none));

        // r_only contains only r_only and none.
        assert!(r_only.contains(&r_only));
        assert!(r_only.contains(&none));
        assert!(!r_only.contains(&rw));        // missing write
        assert!(!r_only.contains(&rwx));       // missing write+execute

        // none contains only none.
        assert!(none.contains(&none));
        assert!(!none.contains(&r_only));
    }

    #[test]
    fn test_grant_capability_signature_is_deterministic() {
        // Same inputs → byte-identical signature. This is what makes the
        // grant/verify round-trip work: verify recomputes the signature
        // and compares, so any non-determinism would make valid tokens
        // fail verification.
        let resource = capability::Resource::Memory(0x4000, 0x1000);
        let perms = capability::MemoryPermissions { read: true, write: true, execute: false };
        let key = b"vuma-test-signing-key-2024";

        let t1 = capability::grant_capability(
            42, 1, 2, resource.clone(), perms.clone(), 0, 1_000, 500, key,
        );
        let t2 = capability::grant_capability(
            42, 1, 2, resource.clone(), perms.clone(), 0, 1_000, 500, key,
        );
        assert_eq!(t1.signature, t2.signature, "same inputs must produce same signature");
        assert_eq!(t1, t2, "whole tokens must be identical");

        // Different signing key → different signature (the key is mixed
        // into the hash input first, so any byte change cascades).
        let other_key = b"vuma-test-signing-key-9999";
        let t3 = capability::grant_capability(
            42, 1, 2, resource, perms, 0, 1_000, 500, other_key,
        );
        assert_ne!(t1.signature, t3.signature, "different signing keys must produce different signatures");
    }

    #[test]
    fn test_grant_capability_sets_expires_at() {
        // ttl_seconds is added to created_at (saturating) to produce
        // expires_at. Verify the arithmetic and the saturating edge.
        let token = capability::grant_capability(
            1, 1, 2,
            capability::Resource::Channel(1),
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 1_000, 500, b"k",
        );
        assert_eq!(token.created_at, 1_000);
        assert_eq!(token.expires_at, 1_500);

        // Saturating add: u64::MAX + 1 must not wrap to 0.
        let sat = capability::grant_capability(
            2, 1, 2,
            capability::Resource::Channel(1),
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, u64::MAX, 1, b"k",
        );
        assert_eq!(sat.expires_at, u64::MAX, "ttl add must saturate at u64::MAX");
    }

    #[test]
    fn test_verify_capability_succeeds_after_grant() {
        // Happy path: grant a token, then verify it with the same key,
        // the same resource, and a strictly-subset permission set. All
        // four checks (signature, expiry, resource, perms) must pass.
        let resource = capability::Resource::Network("10.0.0.1".into(), 443);
        let granted_perms = capability::MemoryPermissions {
            read: true, write: true, execute: false,
        };
        let key = b"secret-key";
        let token = capability::grant_capability(
            7, 1, 2, resource.clone(), granted_perms, 0, 1_000, 500, key,
        );

        // now=1_200 is inside [1_000, 1_500].
        let required = capability::MemoryPermissions { read: true, write: false, execute: false };
        let result = capability::verify_capability(&token, key, 1_200, Some(&resource), &required);
        assert!(result.is_ok(), "verify must succeed for a freshly-granted token: {:?}", result.err());

        // Empty required perms always passes the perms check.
        let none = capability::MemoryPermissions::default();
        let result2 = capability::verify_capability(&token, key, 1_000, Some(&resource), &none);
        assert!(result2.is_ok());

        // now exactly == expires_at must still be valid (inclusive upper bound).
        let result3 = capability::verify_capability(&token, key, 1_500, Some(&resource), &required);
        assert!(result3.is_ok());

        // now exactly == created_at must be valid (inclusive lower bound).
        let result4 = capability::verify_capability(&token, key, 1_000, Some(&resource), &required);
        assert!(result4.is_ok());
    }

    #[test]
    fn test_verify_capability_skips_resource_check_when_none() {
        // When the caller passes None for expected_resource, the resource
        // check is skipped. This is the path for callers that don't care
        // which resource the token is bound to (e.g. a generic capability
        // auditor that just wants to know the token is well-formed and
        // unexpired).
        let resource = capability::Resource::File("/tmp/foo".into());
        let key = b"k";
        let token = capability::grant_capability(
            1, 1, 2, resource, capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 100, 1_000, key,
        );
        let required = capability::MemoryPermissions { read: true, write: false, execute: false };
        let result = capability::verify_capability(&token, key, 500, None, &required);
        assert!(result.is_ok(), "None expected_resource must skip the resource check: {:?}", result.err());
    }

    #[test]
    fn test_verify_capability_fails_wrong_resource() {
        // Token is granted for Memory(0x1000, 0x1000) but the caller asks
        // to verify it against Memory(0x2000, 0x1000). The signature
        // check passes (the token is internally consistent), but the
        // resource-mismatch check must fire.
        let granted_resource = capability::Resource::Memory(0x1000, 0x1000);
        let wrong_resource = capability::Resource::Memory(0x2000, 0x1000);
        let key = b"k";
        let token = capability::grant_capability(
            1, 1, 2, granted_resource.clone(),
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 100, 1_000, key,
        );
        let required = capability::MemoryPermissions { read: true, write: false, execute: false };
        let err = capability::verify_capability(&token, key, 500, Some(&wrong_resource), &required)
            .expect_err("wrong resource must fail verify");
        match err {
            capability::CapabilityError::ResourceMismatch { expected, actual } => {
                assert_eq!(expected, wrong_resource);
                assert_eq!(actual, granted_resource);
            }
            other => panic!("expected ResourceMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_verify_capability_fails_insufficient_perms() {
        // Token grants read-only; caller requires read+write.
        let resource = capability::Resource::Channel(42);
        let key = b"k";
        let granted_perms = capability::MemoryPermissions { read: true, write: false, execute: false };
        let required_perms = capability::MemoryPermissions { read: true, write: true, execute: false };
        let token = capability::grant_capability(
            1, 1, 2, resource, granted_perms.clone(), 0, 100, 1_000, key,
        );
        let err = capability::verify_capability(&token, key, 500, None, &required_perms)
            .expect_err("insufficient perms must fail verify");
        match err {
            capability::CapabilityError::InsufficientPermissions { required, actual } => {
                assert_eq!(required, required_perms);
                assert_eq!(actual, granted_perms);
            }
            other => panic!("expected InsufficientPermissions, got {:?}", other),
        }
    }

    #[test]
    fn test_verify_capability_fails_expired() {
        // now > expires_at.
        let resource = capability::Resource::Channel(1);
        let key = b"k";
        let token = capability::grant_capability(
            1, 1, 2, resource,
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 100, 500, key,
        ); // valid in [100, 600]
        let required = capability::MemoryPermissions { read: true, write: false, execute: false };
        let err = capability::verify_capability(&token, key, 601, None, &required)
            .expect_err("now > expires_at must fail verify");
        match err {
            capability::CapabilityError::Expired { now, created_at, expires_at } => {
                assert_eq!(now, 601);
                assert_eq!(created_at, 100);
                assert_eq!(expires_at, 600);
            }
            other => panic!("expected Expired, got {:?}", other),
        }
    }

    #[test]
    fn test_verify_capability_fails_not_yet_valid() {
        // now < created_at — the token was minted for a future window.
        let resource = capability::Resource::Channel(1);
        let key = b"k";
        let token = capability::grant_capability(
            1, 1, 2, resource,
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 1_000, 500, key,
        ); // valid in [1_000, 1_500]
        let required = capability::MemoryPermissions { read: true, write: false, execute: false };
        let err = capability::verify_capability(&token, key, 999, None, &required)
            .expect_err("now < created_at must fail verify");
        assert!(matches!(err, capability::CapabilityError::Expired { .. }));
    }

    #[test]
    fn test_verify_capability_fails_tampered_signature() {
        // Flip a single byte in the signature. The recomputed signature
        // will not match → InvalidSignature.
        let resource = capability::Resource::Channel(1);
        let key = b"k";
        let mut token = capability::grant_capability(
            1, 1, 2, resource,
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 100, 1_000, key,
        );
        token.signature[0] ^= 0xFF;
        let required = capability::MemoryPermissions { read: true, write: false, execute: false };
        let err = capability::verify_capability(&token, key, 500, None, &required)
            .expect_err("tampered signature must fail verify");
        assert_eq!(err, capability::CapabilityError::InvalidSignature);
    }

    #[test]
    fn test_verify_capability_fails_tampered_resource() {
        // Tamper with the resource *after* grant. The signature no longer
        // matches the (new) resource, so the signature check fires before
        // the resource-mismatch check even runs.
        let resource = capability::Resource::Channel(1);
        let key = b"k";
        let mut token = capability::grant_capability(
            1, 1, 2, resource,
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 100, 1_000, key,
        );
        // Swap resource without re-signing.
        token.resource = capability::Resource::Channel(2);
        let required = capability::MemoryPermissions { read: true, write: false, execute: false };
        let err = capability::verify_capability(&token, key, 500, None, &required)
            .expect_err("tampered resource must fail verify (signature mismatch)");
        assert_eq!(err, capability::CapabilityError::InvalidSignature);
    }

    #[test]
    fn test_verify_capability_fails_wrong_signing_key() {
        // Token minted under key A, verified under key B. The recomputed
        // signature differs because the key is the first thing mixed
        // into the hash input.
        let resource = capability::Resource::Channel(1);
        let key_a = b"key-a";
        let key_b = b"key-b";
        let token = capability::grant_capability(
            1, 1, 2, resource,
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 100, 1_000, key_a,
        );
        let required = capability::MemoryPermissions { read: true, write: false, execute: false };
        let err = capability::verify_capability(&token, key_b, 500, None, &required)
            .expect_err("verifying under the wrong key must fail");
        assert_eq!(err, capability::CapabilityError::InvalidSignature);
    }

    #[test]
    fn test_grant_then_encode_then_decode_then_verify() {
        // End-to-end: grant a token, serialise it to the wire format,
        // parse it back, and verify the parsed token. This pins the
        // contract that the signature survives the wire round-trip
        // (i.e. encode/decode don't drop or mangle any field that
        // participates in the signature).
        let resource = capability::Resource::File("/etc/vuma.conf".into());
        let key = b"vuma-wave-11-integration-key";
        let original = capability::grant_capability(
            0xABCD_1234,
            7,
            9,
            resource.clone(),
            capability::MemoryPermissions { read: true, write: true, execute: false },
            0,
            5_000,
            10_000,
            key,
        );

        let wire = original.encode();
        assert_eq!(wire.len(), capability::CAPABILITY_TOKEN_SIZE);
        let parsed = capability::CapabilityToken::decode(&wire)
            .expect("decode of a valid encode must succeed");
        assert_eq!(parsed, original, "encode→decode must be lossless");

        // Now verify the parsed token. now=7_500 is inside [5_000, 15_000].
        let required = capability::MemoryPermissions { read: true, write: false, execute: false };
        let result = capability::verify_capability(&parsed, key, 7_500, Some(&resource), &required);
        assert!(result.is_ok(), "parsed token must verify: {:?}", result.err());
    }

    #[test]
    fn test_grant_then_frame_then_deframe_then_verify() {
        // Full IPC integration: grant a token, attach it to an
        // EncapsulatedMessage, frame the message, deframe it, and verify
        // the recovered capability. This proves the L1 framer's
        // cap_count * CAPABILITY_TOKEN_SIZE arithmetic still works after
        // the constant changed from 96 → 160 in Wave 11.
        let resource = capability::Resource::Channel(0xCAFE);
        let key = b"ipc-integration-key";
        let cap = capability::grant_capability(
            0xDEAD_BEEF_CAFE_BABE_0123_4567_89AB_CDEF,
            1,
            2,
            resource.clone(),
            capability::MemoryPermissions { read: true, write: true, execute: false },
            0,
            1_000,
            2_000,
            key,
        );

        let mut msg = EncapsulatedMessage::new(
            0xCAFE,
            0xBEEF,
            type_hash("Vec<u8>"),
            vec![1, 2, 3, 4],
        );
        msg.header.flags = MessageFlags::HAS_CAPS;
        msg.capabilities = vec![cap.clone()];

        let framed = frame_message(&msg);
        // Expected layout: header + payload + 1 cap token + CRC32.
        assert_eq!(
            framed.len(),
            HEADER_SIZE + msg.payload.len()
                + capability::CAPABILITY_TOKEN_SIZE
                + CRC32_SIZE
        );

        let decoded = deframe_message(&framed).expect("deframe should succeed");
        assert_eq!(decoded.capabilities.len(), 1);
        let recovered = &decoded.capabilities[0];
        assert_eq!(recovered, &cap, "framed capability must survive deframe");

        // The recovered token must still verify.
        let required = capability::MemoryPermissions { read: true, write: false, execute: false };
        let result = capability::verify_capability(recovered, key, 1_500, Some(&resource), &required);
        assert!(result.is_ok(), "recovered capability must verify: {:?}", result.err());
    }

    // ── Wave 12: capability-in-IPC-message roundtrip tests ─────────────
    //
    // W11 made `CapabilityToken::encode`/`decode` real and added
    // `grant_capability`/`verify_capability`. The L1 framer already
    // serialised `cap_count` (u32 LE) into the fixed header and appended
    // each token's `encode()` bytes after the payload, but it did NOT
    // auto-set the HAS_CAPS flag — the producer had to remember to set
    // it, and a forgetful producer would silently ship capabilities with
    // the flag cleared. W12 closes that gap: `frame_message` now sets
    // HAS_CAPS whenever `msg.capabilities` is non-empty, and the three
    // tests below pin the 0/1/2-capability roundtrip contract that the
    // channel-send/recv runtime helper will rely on.

    #[test]
    fn test_frame_deframe_one_capability_roundtrip() {
        // Frame a message carrying exactly one capability token, deframe
        // it, and verify the capability survives the round-trip
        // byte-for-byte (whole-token equality, the strongest check).
        let key = b"w12-single-cap-key";
        let cap = capability::grant_capability(
            0x1111_2222_3333_4444_5555_6666_7777_8888,
            1,
            2,
            capability::Resource::File("/etc/vuma/cap1.conf".into()),
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0,
            1_000,
            2_000,
            key,
        );

        let mut msg = EncapsulatedMessage::new(
            0xAAAA,
            0x0001,
            type_hash("Vec<u8>"),
            vec![0xDE, 0xAD, 0xBE, 0xEF],
        );
        msg.capabilities.push(cap.clone());
        // Deliberately do NOT set HAS_CAPS — frame_message must auto-set it.

        let framed = frame_message(&msg);
        // Layout: header + payload + 1 cap token + CRC32.
        assert_eq!(
            framed.len(),
            HEADER_SIZE + msg.payload.len()
                + capability::CAPABILITY_TOKEN_SIZE
                + CRC32_SIZE
        );

        let decoded = deframe_message(&framed).expect("deframe must succeed");
        assert_eq!(decoded.header.channel_id, 0xAAAA);
        assert_eq!(decoded.header.sequence, 0x0001);
        assert_eq!(decoded.header.cap_count, 1);
        assert_eq!(decoded.capabilities.len(), 1);
        // Whole-token equality — the strongest round-trip check.
        assert_eq!(decoded.capabilities[0], cap, "single capability must round-trip exactly");
        // HAS_CAPS must have been auto-set by frame_message.
        assert!(decoded.header.flags.contains(MessageFlags::HAS_CAPS));
        // Payload must survive too.
        assert_eq!(decoded.payload, vec![0xDE, 0xAD, 0xBE, 0xEF]);

        // The recovered token must still verify.
        let required = capability::MemoryPermissions { read: true, write: false, execute: false };
        let result = capability::verify_capability(&decoded.capabilities[0], key, 1_500, None, &required);
        assert!(result.is_ok(), "recovered single capability must verify: {:?}", result.err());
    }

    #[test]
    fn test_frame_deframe_zero_capabilities_roundtrip() {
        // Frame a message with NO capabilities, deframe it, and verify the
        // capabilities vec is empty and HAS_CAPS is NOT set. This is the
        // common case for ordinary channel payloads (e.g. an i32 sent over
        // a channel with no capability attachments).
        let msg = EncapsulatedMessage::new(
            0xBBBB,
            0x0002,
            type_hash("i32"),
            vec![0x2A, 0x00, 0x00, 0x00],
        );
        // No capabilities pushed, no flags set.

        let framed = frame_message(&msg);
        // Layout: header + payload + CRC32 (no cap tokens).
        assert_eq!(framed.len(), HEADER_SIZE + msg.payload.len() + CRC32_SIZE);

        let decoded = deframe_message(&framed).expect("deframe must succeed");
        assert_eq!(decoded.header.channel_id, 0xBBBB);
        assert_eq!(decoded.header.sequence, 0x0002);
        assert_eq!(decoded.header.cap_count, 0);
        assert!(decoded.capabilities.is_empty());
        // HAS_CAPS must NOT be set when there are no capabilities.
        assert!(!decoded.header.flags.contains(MessageFlags::HAS_CAPS));
        // Payload must survive.
        assert_eq!(decoded.payload, vec![0x2A, 0x00, 0x00, 0x00]);
        let recovered = i32::from_le_bytes(decoded.payload[..].try_into().unwrap());
        assert_eq!(recovered, 42);
    }

    #[test]
    fn test_frame_deframe_two_capabilities_roundtrip() {
        // Frame a message carrying TWO capability tokens, deframe it, and
        // verify both tokens survive the round-trip in order,
        // byte-for-byte. This pins the cap_count*CAPABILITY_TOKEN_SIZE
        // slicing arithmetic in deframe_message for the multi-cap case
        // (the single-cap case is covered by the W11 integration test
        // above, but there was previously no test exercising >1 cap).
        let key = b"w12-two-caps-key";
        let cap_a = capability::grant_capability(
            0xAAAA_BBBB_CCCC_DDDD_EEEE_FFFF_0000_1111,
            1,
            2,
            capability::Resource::Network("10.0.0.1".into(), 443),
            capability::MemoryPermissions { read: true, write: true, execute: false },
            0,
            5_000,
            10_000,
            key,
        );
        let cap_b = capability::grant_capability(
            0x2222_3333_4444_5555_6666_7777_8888_9999,
            3,
            4,
            capability::Resource::Memory(0x1000, 0x2000),
            capability::MemoryPermissions { read: true, write: false, execute: true },
            1,
            6_000,
            9_000,
            key,
        );

        let mut msg = EncapsulatedMessage::new(
            0xCCCC,
            0x0003,
            type_hash("Vec<u8>"),
            vec![1, 2, 3, 4, 5, 6, 7, 8],
        );
        msg.capabilities.push(cap_a.clone());
        msg.capabilities.push(cap_b.clone());
        // Deliberately do NOT set HAS_CAPS — frame_message must auto-set it.

        let framed = frame_message(&msg);
        // Layout: header + payload + 2 cap tokens + CRC32.
        assert_eq!(
            framed.len(),
            HEADER_SIZE + msg.payload.len()
                + 2 * capability::CAPABILITY_TOKEN_SIZE
                + CRC32_SIZE
        );

        let decoded = deframe_message(&framed).expect("deframe must succeed");
        assert_eq!(decoded.header.channel_id, 0xCCCC);
        assert_eq!(decoded.header.sequence, 0x0003);
        assert_eq!(decoded.header.cap_count, 2);
        assert_eq!(decoded.capabilities.len(), 2);
        // Both tokens must round-trip exactly, in the order they were framed.
        assert_eq!(decoded.capabilities[0], cap_a, "first capability must round-trip");
        assert_eq!(decoded.capabilities[1], cap_b, "second capability must round-trip");
        // Different ids / resources confirm we didn't accidentally read the
        // same token twice.
        assert_ne!(decoded.capabilities[0].id, decoded.capabilities[1].id);
        assert_ne!(decoded.capabilities[0].resource, decoded.capabilities[1].resource);
        // HAS_CAPS must be set.
        assert!(decoded.header.flags.contains(MessageFlags::HAS_CAPS));
        // Payload must survive.
        assert_eq!(decoded.payload, vec![1, 2, 3, 4, 5, 6, 7, 8]);

        // Both recovered tokens must still verify against the signing key,
        // proving the wire round-trip didn't mangle any signature-relevant
        // field. now=7_000 is inside both validity windows
        // ([5_000, 10_000] and [6_000, 9_000]).
        let now = 7_000u64;
        let required_ro = capability::MemoryPermissions { read: true, write: false, execute: false };
        assert!(
            capability::verify_capability(&decoded.capabilities[0], key, now, None, &required_ro).is_ok(),
            "first recovered capability must verify"
        );
        assert!(
            capability::verify_capability(&decoded.capabilities[1], key, now, None, &required_ro).is_ok(),
            "second recovered capability must verify"
        );
    }

    // ── W13: MemoryWindow tests ──────────────────────────────────────

    #[test]
    fn test_memory_window_grant_is_valid_revoke() {
        // grant → is_valid true → revoke → is_valid false, the canonical
        // lifecycle the W13 spec calls out. Uses grant_memory so the
        // capability_id is a real minted value rather than a hand-rolled
        // zero, and exercises revoke_memory's Result return.
        let perms = capability::MemoryPermissions {
            read: true,
            write: true,
            execute: false,
        };
        let mut window = grant_memory(100, 200, 0xDEAD_0000, 4096, perms);
        assert_eq!(window.source_pid, 100);
        assert_eq!(window.target_pid, 200);
        assert_eq!(window.source_addr, 0xDEAD_0000);
        assert_eq!(window.size, 4096);
        assert!(window.revocable);
        assert!(!window.revoked);
        assert!(!window.linear);
        assert!(is_valid(&window), "freshly granted window must be valid");

        revoke_memory(&mut window).expect("revoke of a revocable window must succeed");
        assert!(
            !is_valid(&window),
            "revoked window must report invalid via is_valid"
        );
        assert!(window.revoked, "revoked flag must be set on the struct");
    }

    #[test]
    fn test_memory_window_revoke_non_revocable_fails() {
        // A window with revocable=false is permanent: revoke_memory must
        // refuse rather than silently no-op. This pins the IpcError
        // variant the grantor relies on to detect a buggy revoke attempt.
        let perms = capability::MemoryPermissions {
            read: true,
            write: false,
            execute: false,
        };
        let mut window = grant_memory(1, 2, 0x1000, 0x1000, perms);
        window.revocable = false;
        let err = revoke_memory(&mut window).expect_err("non-revocable revoke must error");
        assert_eq!(err, IpcError::MemoryWindowPermissionDenied);
        assert!(is_valid(&window), "failed revoke must leave window valid");
    }

    #[test]
    fn test_memory_window_double_revoke_fails() {
        // Idempotent revoke is a programming error — the caller should
        // have dropped its reference after the first revoke. The second
        // revoke must surface MemoryWindowRevoked so the bug is visible
        // rather than silently swallowed.
        let perms = capability::MemoryPermissions {
            read: true,
            write: true,
            execute: true,
        };
        let mut window = grant_memory(7, 8, 0, 0x2000, perms);
        revoke_memory(&mut window).expect("first revoke must succeed");
        let err = revoke_memory(&mut window).expect_err("second revoke must error");
        assert_eq!(err, IpcError::MemoryWindowRevoked);
    }

    #[test]
    fn test_memory_window_zero_size_is_invalid() {
        // A zero-size window is a tombstone the backend leaves behind
        // after unmapping; is_valid must not treat it as live even before
        // revoke is called.
        let perms = capability::MemoryPermissions::default();
        let mut window = grant_memory(1, 2, 0, 0, perms);
        assert!(!is_valid(&window), "zero-size window must be invalid");
        // ...and revoke still works on a tombstone (it's revocable by
        // default), it just doesn't change the is_valid answer.
        revoke_memory(&mut window).expect("revoke of revocable tombstone must succeed");
        assert!(!is_valid(&window));
    }

    #[test]
    fn test_memory_window_encode_decode_roundtrip() {
        // Full encode → decode round-trip must preserve every field,
        // including the u128 capability_id (split across two u64 lanes
        // on the wire) and all three permission bits.
        let perms = capability::MemoryPermissions {
            read: true,
            write: false,
            execute: true,
        };
        let window = MemoryWindow {
            source_pid: 0x0123_4567_89AB_CDEF,
            target_pid: 0xFEDC_BA98_7654_3210,
            source_addr: 0xCAFE_BABE_0000,
            target_addr: 0xDEAD_BEEF_0000,
            size: 0x10000,
            permissions: perms,
            capability_id: 0x0011_2233_4455_6677_8899_AABB_CCDD_EEFF,
            revocable: true,
            revoked: false,
            linear: true,
        };
        let encoded = window.encode();
        assert_eq!(
            encoded.len(),
            MEMORY_WINDOW_SIZE,
            "encode must produce exactly MEMORY_WINDOW_SIZE bytes"
        );
        let decoded = MemoryWindow::decode(&encoded).expect("decode of valid buffer must succeed");
        assert_eq!(decoded, window, "round-trip must preserve all fields");
    }

    #[test]
    fn test_memory_window_decode_preserves_revoked_flag() {
        // A revoked window serialised by the sender and deserialised by
        // the receiver must still report revoked=true on the receive
        // side — otherwise a revoked mapping could be smuggled past
        // is_valid by re-encoding it.
        let perms = capability::MemoryPermissions {
            read: true,
            write: true,
            execute: false,
        };
        let mut window = grant_memory(10, 20, 0x4000, 0x2000, perms);
        revoke_memory(&mut window).expect("revoke must succeed");
        let encoded = window.encode();
        let decoded = MemoryWindow::decode(&encoded).expect("decode must succeed");
        assert!(decoded.revoked, "revoked flag must survive the wire");
        assert!(!is_valid(&decoded), "decoded revoked window must be invalid");
    }

    #[test]
    fn test_memory_window_decode_too_short() {
        // Defensive: a short buffer must error rather than panic on the
        // slice indexing inside decode.
        let short = [0u8; MEMORY_WINDOW_SIZE - 1];
        let err = MemoryWindow::decode(&short).expect_err("short buffer must error");
        assert!(err.contains("too short"), "error must mention truncation, got: {}", err);
    }

    #[test]
    fn test_memory_window_decode_ignores_trailing_bytes() {
        // A caller may hand in a slice of a larger IPC frame; decode
        // must accept the leading MEMORY_WINDOW_SIZE bytes and ignore
        // the rest.
        let perms = capability::MemoryPermissions {
            read: false,
            write: true,
            execute: false,
        };
        let window = grant_memory(5, 6, 0x100, 0x800, perms);
        let mut buf = Vec::with_capacity(MEMORY_WINDOW_SIZE + 8);
        buf.extend_from_slice(&window.encode());
        buf.extend_from_slice(&[0xFF; 8]); // trailing junk
        let decoded = MemoryWindow::decode(&buf).expect("trailing bytes must be ignored");
        assert_eq!(decoded.source_pid, window.source_pid);
        assert_eq!(decoded.size, window.size);
        assert_eq!(decoded.capability_id, window.capability_id);
    }

    // ── W14: ProtocolStateMachine tests ──────────────────────────────

    #[test]
    fn test_protocol_state_machine_valid_transitions() {
        // The default request/response FSM: Idle --send--> WaitingForSend
        // --sent--> WaitingForRecv --recv--> Idle. Each step must return
        // the new state and advance current_state; a full cycle must
        // return the machine to Idle.
        let mut fsm = ProtocolStateMachine::new_protocol();
        assert_eq!(fsm.current_state, ProtocolState::Idle);

        let s = fsm.check_transition(type_hash("send")).expect("send from Idle must be allowed");
        assert_eq!(s, ProtocolState::WaitingForSend);
        assert_eq!(fsm.current_state, ProtocolState::WaitingForSend);

        let s = fsm.check_transition(type_hash("sent")).expect("sent from WaitingForSend must be allowed");
        assert_eq!(s, ProtocolState::WaitingForRecv);
        assert_eq!(fsm.current_state, ProtocolState::WaitingForRecv);

        let s = fsm.check_transition(type_hash("recv")).expect("recv from WaitingForRecv must be allowed");
        assert_eq!(s, ProtocolState::Idle);
        assert_eq!(fsm.current_state, ProtocolState::Idle);
    }

    #[test]
    fn test_protocol_state_machine_invalid_transition_returns_error() {
        // A recv in Idle (without first having sent) is not in the
        // default table for the *send* type hash — submitting a 'sent'
        // type hash while in Idle must yield ProtocolViolation, and the
        // machine must remain in Idle so the next legitimate message
        // still works.
        let mut fsm = ProtocolStateMachine::new_protocol();
        assert_eq!(fsm.current_state, ProtocolState::Idle);

        let bad_hash = type_hash("sent");
        let err = fsm
            .check_transition(bad_hash)
            .expect_err("sent from Idle must be rejected");
        assert_eq!(
            err,
            ProtocolError::ProtocolViolation {
                expected: ProtocolState::Idle,
                got: bad_hash,
            }
        );
        assert_eq!(
            fsm.current_state,
            ProtocolState::Idle,
            "failed transition must not advance the FSM"
        );

        // The machine is still usable: a legitimate send now must work.
        let s = fsm.check_transition(type_hash("send")).expect("send from Idle must still work");
        assert_eq!(s, ProtocolState::WaitingForSend);
    }

    #[test]
    fn test_protocol_state_machine_close_from_any_state() {
        // close is permitted from Idle, WaitingForSend, and
        // WaitingForRecv — and once Closed, no further transitions
        // (including another close) are accepted.
        let mut fsm = ProtocolStateMachine::new_protocol();
        fsm.check_transition(type_hash("send")).expect("send");
        fsm.check_transition(type_hash("close"))
            .expect("close from WaitingForSend must be allowed");
        assert_eq!(fsm.current_state, ProtocolState::Closed);

        // Any transition out of Closed must yield ProtocolError::Closed,
        // not ProtocolViolation — the channel is gone, not mis-driven.
        let err = fsm
            .check_transition(type_hash("send"))
            .expect_err("send from Closed must be rejected");
        assert_eq!(err, ProtocolError::Closed);
        let err = fsm
            .check_transition(type_hash("close"))
            .expect_err("close from Closed must also be rejected");
        assert_eq!(err, ProtocolError::Closed);
        assert_eq!(fsm.current_state, ProtocolState::Closed);
    }

    #[test]
    fn test_protocol_state_machine_default_impl_matches_new_protocol() {
        // Default::default() must install the same transition table as
        // new_protocol() so generic code (e.g. struct fields defaulted
        // via derive(Default)) gets the real FSM, not an empty one.
        let a = ProtocolStateMachine::default();
        let b = ProtocolStateMachine::new_protocol();
        assert_eq!(a.current_state, b.current_state);
        assert_eq!(
            a.allowed_transitions.len(),
            b.allowed_transitions.len(),
            "default and new_protocol must install identical tables"
        );
    }

    #[test]
    fn test_protocol_error_display_contains_state_and_hash() {
        // The Display impl is what ends up in logs; it must mention both
        // the offending state tag and the type hash so an operator
        // reading the log can diagnose the violation without a debugger.
        let err = ProtocolError::ProtocolViolation {
            expected: ProtocolState::WaitingForRecv,
            got: 0x1234,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("waiting_for_recv"), "msg must mention state tag: {}", msg);
        assert!(msg.contains("0x1234") || msg.contains("4660"), "msg must mention type hash: {}", msg);

        let closed = format!("{}", ProtocolError::Closed);
        assert!(closed.contains("closed"), "Closed msg must mention closed: {}", closed);
    }

    // ── W17-18: ResourceLimits + WorkerSandbox + should_restart ──────

    #[test]
    fn test_resource_limits_check_passes_within_limits() {
        // Every measured resource strictly below its ceiling → pass.
        let limits = ResourceLimits {
            cpu_time_ms: 1_000,
            max_memory_bytes: 64 * 1024 * 1024,
            max_ipc_messages: 500,
            max_file_descriptors: 32,
        };
        let usage = ResourceUsage {
            cpu_time_ms: 500,
            memory_bytes: 32 * 1024 * 1024,
            ipc_messages: 250,
            file_descriptors: 16,
        };
        assert!(
            limits.check_limits(&usage),
            "usage strictly below every ceiling must pass"
        );
    }

    #[test]
    fn test_resource_limits_check_passes_at_exact_ceiling() {
        // <= is allowed: a worker at exactly its budget is still within
        // limits (the supervisor kills on the *next* allocation).
        let limits = ResourceLimits {
            cpu_time_ms: 1_000,
            max_memory_bytes: 64 * 1024 * 1024,
            max_ipc_messages: 500,
            max_file_descriptors: 32,
        };
        let usage = ResourceUsage {
            cpu_time_ms: 1_000,
            memory_bytes: 64 * 1024 * 1024,
            ipc_messages: 500,
            file_descriptors: 32,
        };
        assert!(limits.check_limits(&usage), "usage at the ceiling must pass");
    }

    #[test]
    fn test_resource_limits_check_fails_when_cpu_exceeded() {
        let limits = ResourceLimits {
            cpu_time_ms: 1_000,
            max_memory_bytes: u64::MAX,
            max_ipc_messages: u64::MAX,
            max_file_descriptors: u64::MAX,
        };
        let usage = ResourceUsage {
            cpu_time_ms: 1_001,
            ..Default::default()
        };
        assert!(!limits.check_limits(&usage), "cpu over budget must fail");
    }

    #[test]
    fn test_resource_limits_check_fails_when_memory_exceeded() {
        let limits = ResourceLimits {
            cpu_time_ms: u64::MAX,
            max_memory_bytes: 1024,
            max_ipc_messages: u64::MAX,
            max_file_descriptors: u64::MAX,
        };
        let usage = ResourceUsage {
            memory_bytes: 1025,
            ..Default::default()
        };
        assert!(!limits.check_limits(&usage), "memory over budget must fail");
    }

    #[test]
    fn test_resource_limits_check_fails_when_ipc_exceeded() {
        let limits = ResourceLimits {
            cpu_time_ms: u64::MAX,
            max_memory_bytes: u64::MAX,
            max_ipc_messages: 10,
            max_file_descriptors: u64::MAX,
        };
        let usage = ResourceUsage {
            ipc_messages: 11,
            ..Default::default()
        };
        assert!(!limits.check_limits(&usage), "ipc count over budget must fail");
    }

    #[test]
    fn test_resource_limits_check_fails_when_fd_exceeded() {
        let limits = ResourceLimits {
            cpu_time_ms: u64::MAX,
            max_memory_bytes: u64::MAX,
            max_ipc_messages: u64::MAX,
            max_file_descriptors: 4,
        };
        let usage = ResourceUsage {
            file_descriptors: 5,
            ..Default::default()
        };
        assert!(!limits.check_limits(&usage), "fd count over budget must fail");
    }

    #[test]
    fn test_resource_limits_zero_ceiling_means_unlimited() {
        // A ceiling of 0 disables that check, so even maximal usage must
        // pass — this is the documented "unlimited" sentinel.
        let limits = ResourceLimits {
            cpu_time_ms: 0,
            max_memory_bytes: 0,
            max_ipc_messages: 0,
            max_file_descriptors: 0,
        };
        let usage = ResourceUsage {
            cpu_time_ms: u64::MAX,
            memory_bytes: u64::MAX,
            ipc_messages: u64::MAX,
            file_descriptors: u64::MAX,
        };
        assert!(limits.check_limits(&usage), "zero ceilings must mean unlimited");
    }

    #[test]
    fn test_should_restart_restarts_on_nonzero_exit_code() {
        // exit(n) with n != 0 is a crash — restart (within budget).
        let config = WorkerConfig { max_restarts: 3, ..Default::default() };
        assert!(should_restart(&config, 1),   "exit(1) is a crash — restart");
        assert!(should_restart(&config, 134), "exit(134) (SIGABRT 128+6) — restart");
        assert!(should_restart(&config, 137), "exit(137) (SIGKILL 128+9) — restart");
        assert!(should_restart(&config, 139), "exit(139) (SIGSEGV 128+11) — restart");
    }

    #[test]
    fn test_should_restart_restarts_on_signal_death() {
        // waitpid(2) with WIFSIGNALED reports the negated signal number.
        let config = WorkerConfig { max_restarts: 3, ..Default::default() };
        assert!(should_restart(&config, -11), "killed by SIGSEGV — restart");
        assert!(should_restart(&config, -9),  "killed by SIGKILL — restart");
        assert!(should_restart(&config, -6),  "killed by SIGABRT — restart");
    }

    #[test]
    fn test_should_restart_does_not_restart_on_clean_exit() {
        // exit(0) means the worker finished its job — never restart.
        let config = WorkerConfig { max_restarts: 3, ..Default::default() };
        assert!(!should_restart(&config, 0), "clean exit must not restart");
    }

    #[test]
    fn test_should_restart_disabled_when_max_restarts_zero() {
        // max_restarts == 0 disables the restart policy entirely, even
        // for crashes and signals.
        let config = WorkerConfig { max_restarts: 0, ..Default::default() };
        assert!(!should_restart(&config, 0),   "clean exit, no restart");
        assert!(!should_restart(&config, 1),   "max_restarts=0 disables restart even on crash");
        assert!(!should_restart(&config, -11), "max_restarts=0 disables restart even on signal");
    }

    #[test]
    fn test_worker_sandbox_combines_config_and_limits() {
        // The sandbox must carry both halves of the L5 containment
        // (syscall filter via config, resource ceilings via limits).
        let config = WorkerConfig {
            trust_level: TrustLevel::Sandboxed,
            max_restarts: 5,
            timeout_ms: 2_000,
        };
        let limits = ResourceLimits {
            cpu_time_ms: 500,
            max_memory_bytes: 8 * 1024 * 1024,
            max_ipc_messages: 100,
            max_file_descriptors: 8,
        };
        let sandbox = WorkerSandbox::new(config.clone(), limits.clone());
        assert_eq!(sandbox.config, config);
        assert_eq!(sandbox.limits, limits);
    }

    #[test]
    fn test_worker_sandbox_seccomp_filter_is_well_formed() {
        // The sandbox must actually delegate to generate_seccomp_filter
        // (the L5 wire-up): the result is a BPF program whose length is
        // a multiple of 8, starting with the LD of seccomp_data.nr and
        // ending with the KILL action.
        let sandbox = WorkerSandbox::new(
            WorkerConfig {
                trust_level: TrustLevel::Untrusted,
                ..Default::default()
            },
            ResourceLimits::default(),
        );
        let filter = sandbox.seccomp_filter();
        assert!(filter.len() >= 16, "filter must have at least LD + KILL ({})", filter.len());
        assert_eq!(filter.len() % 8, 0, "BPF instructions are 8 bytes each");
        // First instruction opcode: BPF_LD | BPF_W | BPF_ABS == 0x20.
        assert_eq!(filter[0], 0x20, "filter must start with BPF_LD of seccomp_data.nr");
        // Last instruction: BPF_RET | BPF_K (0x06) with SECCOMP_RET_KILL (0).
        let last = filter.len() - 8;
        assert_eq!(filter[last], 0x06, "filter must end with BPF_RET");
        assert_eq!(
            &filter[last + 4..last + 8],
            &[0, 0, 0, 0],
            "default action is SECCOMP_RET_KILL"
        );
    }

    #[test]
    fn test_worker_sandbox_filter_scales_with_trust_level() {
        // Higher trust ⇒ more allowed syscalls ⇒ longer filter. This
        // pins the wiring: sandbox.seccomp_filter() →
        // generate_seccomp_filter(WorkerConfig) → allowed_syscalls(TrustLevel).
        let mk = |trust: TrustLevel| {
            WorkerSandbox::new(
                WorkerConfig { trust_level: trust, ..Default::default() },
                ResourceLimits::default(),
            )
            .seccomp_filter()
            .len()
        };
        let sandboxed = mk(TrustLevel::Sandboxed);
        let untrusted = mk(TrustLevel::Untrusted);
        let verified = mk(TrustLevel::Verified);
        let kernel = mk(TrustLevel::Kernel);
        assert!(sandboxed < untrusted, "Sandboxed allows fewer syscalls than Untrusted");
        assert!(untrusted < verified, "Untrusted allows fewer syscalls than Verified");
        assert!(verified < kernel, "Verified allows fewer syscalls than Kernel");
    }

    #[test]
    fn test_worker_sandbox_apply_does_not_trap_on_construction() {
        // On x86_64 Linux, calling apply() would install a real seccomp
        // filter on the test runner — so we deliberately do NOT call it
        // here. We instead verify the sandbox can be built and its
        // filter inspected, which is the safe subset of the contract.
        // On non-x86_64/non-Linux targets apply() is a documented no-op
        // returning Ok(0) and is safe to invoke.
        let sandbox = WorkerSandbox::new(WorkerConfig::default(), ResourceLimits::default());
        assert!(!sandbox.seccomp_filter().is_empty());

        #[cfg(not(all(target_arch = "x86_64", target_os = "linux")))]
        {
            let rc = sandbox
                .apply()
                .expect("apply() must succeed on non-native targets");
            assert_eq!(rc, 0, "non-native apply() must return Ok(0)");
        }
        // On x86_64 Linux we intentionally skip apply() to avoid
        // trapping the test process; the seccomp_filter() inspection
        // above already exercises the wire-up.
    }

    // ── L6: checkpoint / restore_state tests ────────────────────────

    #[test]
    fn test_checkpoint_state_stamps_nonzero_integrity_hash() {
        // A real checkpoint must produce a non-zero, non-trivial
        // integrity hash that depends on every channel field. The
        // previous XOR-fold stub collapsed many inputs to zero (e.g.
        // any channel set whose bytes XOR-cancelled); this pins that
        // we no longer do, for both populated and empty inputs.
        let channels = vec![
            (1u64, 100u64, ProtocolState::Idle),
            (2u64, 200u64, ProtocolState::WaitingForSend),
        ];
        let cp = checkpoint_state(&channels);
        assert_ne!(
            cp.integrity_hash, [0u8; 32],
            "integrity hash must not be all-zero for a non-empty channel set"
        );
        // Empty input still hashes to a defined, non-zero value —
        // eight CRC32s of single-byte lane prefixes, which are
        // individually non-zero by construction.
        let empty = checkpoint_state(&[]);
        assert_ne!(
            empty.integrity_hash, [0u8; 32],
            "empty-input hash must still be non-zero (lane prefixes are non-zero)"
        );
    }

    #[test]
    fn test_checkpoint_hash_is_sensitive_to_every_field() {
        // The hash must change when ANY field of ANY channel changes —
        // channel_id, sequence, or protocol_state. This is the property
        // restore_state relies on to detect tampering. The previous
        // XOR-fold stub did not cover protocol_state at all, so a
        // Closed→Idle swap was invisible; the new CRC32-based hash
        // covers the tag bytes too.
        let base = vec![(42u64, 7u64, ProtocolState::Idle)];
        let h0 = checkpoint_state(&base).integrity_hash;

        let h_id   = checkpoint_state(&[(43u64, 7u64, ProtocolState::Idle)]).integrity_hash;
        let h_seq  = checkpoint_state(&[(42u64, 8u64, ProtocolState::Idle)]).integrity_hash;
        let h_state = checkpoint_state(&[(42u64, 7u64, ProtocolState::Closed)]).integrity_hash;

        assert_ne!(h_id,    h0, "changing channel_id must change the hash");
        assert_ne!(h_seq,   h0, "changing sequence must change the hash");
        assert_ne!(h_state, h0, "changing protocol_state must change the hash");
    }

    #[test]
    fn test_restore_state_verifies_integrity_hash_and_returns_channels() {
        // Happy path: a freshly-minted checkpoint restores cleanly and
        // hands back the channel vector in order, with every field
        // preserved.
        let channels = vec![
            (10u64, 1u64, ProtocolState::Idle),
            (20u64, 2u64, ProtocolState::WaitingForRecv),
        ];
        let cp = checkpoint_state(&channels);
        let restored = restore_state(&cp).expect("fresh checkpoint must restore");
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].channel_id, 10);
        assert_eq!(restored[0].sequence, 1);
        assert_eq!(restored[0].protocol_state, ProtocolState::Idle);
        assert_eq!(restored[1].channel_id, 20);
        assert_eq!(restored[1].sequence, 2);
        assert_eq!(restored[1].protocol_state, ProtocolState::WaitingForRecv);
    }

    #[test]
    fn test_restore_state_detects_tampered_sequence() {
        // The canonical L6 tamper-detection test:
        //   checkpoint → mutate a channel's sequence without recomputing
        //   the hash → restore_state MUST refuse with the dedicated
        //   CheckpointIntegrityFailed error. Without this, a buggy or
        //   malicious producer could swap in a different sequence
        //   number and the receiver would apply it silently.
        let channels = vec![
            (1u64, 100u64, ProtocolState::Idle),
            (2u64, 200u64, ProtocolState::WaitingForSend),
        ];
        let mut cp = checkpoint_state(&channels);
        assert!(restore_state(&cp).is_ok(), "untampered checkpoint must restore");

        // Tamper: bump a sequence number without updating the hash.
        cp.channels[1].sequence = 999;
        let err = restore_state(&cp)
            .expect_err("tampered sequence must fail restore");
        assert_eq!(err, IpcError::CheckpointIntegrityFailed);
    }

    #[test]
    fn test_restore_state_detects_tampered_protocol_state() {
        // Same shape as the sequence test but mutating the
        // protocol_state field. The previous hash only covered
        // channel_id + sequence, so this swap would have passed
        // silently; the new CRC32-based hash covers the tag too, so
        // it must now be detected.
        let channels = vec![(5u64, 5u64, ProtocolState::Idle)];
        let mut cp = checkpoint_state(&channels);
        cp.channels[0].protocol_state = ProtocolState::Closed;
        let err = restore_state(&cp)
            .expect_err("tampered protocol_state must fail restore");
        assert_eq!(err, IpcError::CheckpointIntegrityFailed);
    }

    #[test]
    fn test_restore_state_detects_tampered_channel_id() {
        // Third field of the tamper trilogy: mutate channel_id.
        let channels = vec![(7u64, 3u64, ProtocolState::Idle)];
        let mut cp = checkpoint_state(&channels);
        cp.channels[0].channel_id = 8;
        let err = restore_state(&cp)
            .expect_err("tampered channel_id must fail restore");
        assert_eq!(err, IpcError::CheckpointIntegrityFailed);
    }

    #[test]
    fn test_restore_state_detects_forged_hash() {
        // If an attacker forges a hash to match a tampered channel
        // vector, they'd need to invert eight CRC32s — out of scope
        // here. We test the simpler case: a random wrong hash is
        // rejected even when the channels are unchanged, so a
        // truncated/garbled hash in transit is caught.
        let channels = vec![(1u64, 1u64, ProtocolState::Idle)];
        let mut cp = checkpoint_state(&channels);
        cp.integrity_hash = [0xFF; 32]; // wrong hash, channels unchanged
        let err = restore_state(&cp)
            .expect_err("wrong hash must fail restore");
        assert_eq!(err, IpcError::CheckpointIntegrityFailed);
    }

    #[test]
    fn test_restore_state_detects_appended_channel() {
        // A producer that appends an extra channel to the vector
        // (without recomputing the hash) must be caught — this is
        // the canonical "rollback / replay an old channel" attack.
        let channels = vec![(1u64, 1u64, ProtocolState::Idle)];
        let mut cp = checkpoint_state(&channels);
        cp.channels.push(ChannelState {
            channel_id: 999,
            sequence: 999,
            protocol_state: ProtocolState::Closed,
        });
        let err = restore_state(&cp)
            .expect_err("appended channel must fail restore");
        assert_eq!(err, IpcError::CheckpointIntegrityFailed);
    }

    // ── L7: error containment tests ────────────────────────────────

    #[test]
    fn test_handle_worker_error_sigsegv_with_budget_restarts() {
        // The canonical crash-restart case: SIGSEGV (signal 11) with a
        // non-zero restart budget. The supervisor is asked to restart.
        // exit_code follows the shell convention (128 + 11 = 139) so
        // the test data is realistic for a signal-killed worker.
        let err = WorkerError {
            exit_code: 139, // 128 + SIGSEGV(11)
            signal: 11,
            stderr_capture: String::from("segfault at 0x0"),
            timestamp: 1_000,
        };
        let config = WorkerConfig { max_restarts: 3, ..Default::default() };
        assert_eq!(
            handle_worker_error(&err, &config),
            RecoveryAction::Restart,
            "SIGSEGV with budget must Restart"
        );
    }

    #[test]
    fn test_handle_worker_error_clean_exit_terminates() {
        // exit(0) means the worker finished its job — Terminate,
        // regardless of config. This is the second arm of the policy.
        let err = WorkerError {
            exit_code: 0,
            signal: 0,
            stderr_capture: String::new(),
            timestamp: 2_000,
        };
        let config = WorkerConfig { max_restarts: 3, ..Default::default() };
        assert_eq!(
            handle_worker_error(&err, &config),
            RecoveryAction::Terminate,
            "clean exit must Terminate"
        );
    }

    #[test]
    fn test_handle_worker_error_nonzero_exit_escalates() {
        // A non-zero exit code (and no signal) is an explicit failure
        // the worker chose to report — escalate rather than restart,
        // because the worker already decided the situation was
        // unrecoverable.
        let err = WorkerError {
            exit_code: 1,
            signal: 0,
            stderr_capture: String::from("panic: inventory underflow"),
            timestamp: 3_000,
        };
        let config = WorkerConfig { max_restarts: 3, ..Default::default() };
        assert_eq!(
            handle_worker_error(&err, &config),
            RecoveryAction::Escalate,
            "non-zero exit must Escalate"
        );
    }

    #[test]
    fn test_handle_worker_error_sigsegv_no_budget_escalates() {
        // SIGSEGV with max_restarts == 0: the restart policy is
        // disabled, so even a restartable-looking crash must escalate
        // rather than spin forever. This is the key difference from
        // should_restart, which would just return false here —
        // handle_worker_error makes the escalation explicit. exit_code
        // is 139 (128 + 11) per the shell convention, so it falls past
        // the Terminate arm (exit_code != 0) into Escalate.
        let err = WorkerError {
            exit_code: 139, // 128 + SIGSEGV(11)
            signal: 11,
            stderr_capture: String::from("segfault"),
            timestamp: 4_000,
        };
        let config = WorkerConfig { max_restarts: 0, ..Default::default() };
        assert_eq!(
            handle_worker_error(&err, &config),
            RecoveryAction::Escalate,
            "SIGSEGV with no restart budget must Escalate"
        );
    }

    #[test]
    fn test_handle_worker_error_sigkill_escalates() {
        // Only SIGSEGV triggers Restart. SIGKILL (9) — e.g. OOM killer
        // — is escalated even with a budget, because it usually
        // indicates an environmental problem that restarting won't fix.
        // exit_code is 137 (128 + 9) per the shell convention, so the
        // Terminate arm (exit_code == 0) does not fire.
        let err = WorkerError {
            exit_code: 137, // 128 + SIGKILL(9)
            signal: 9,
            stderr_capture: String::from("killed"),
            timestamp: 5_000,
        };
        let config = WorkerConfig { max_restarts: 5, ..Default::default() };
        assert_eq!(
            handle_worker_error(&err, &config),
            RecoveryAction::Escalate,
            "SIGKILL must Escalate even with budget"
        );
    }

    #[test]
    fn test_handle_worker_error_sigabrt_escalates() {
        // SIGABRT (6) — typically from an assertion or double-free in
        // the allocator — is escalated, not restarted. SIGSEGV is the
        // only signal in the Restart arm by design. exit_code is 134
        // (128 + 6) per the shell convention.
        let err = WorkerError {
            exit_code: 134, // 128 + SIGABRT(6)
            signal: 6,
            stderr_capture: String::from("abort"),
            timestamp: 6_000,
        };
        let config = WorkerConfig { max_restarts: 5, ..Default::default() };
        assert_eq!(
            handle_worker_error(&err, &config),
            RecoveryAction::Escalate,
            "SIGABRT must Escalate even with budget"
        );
    }

    #[test]
    fn test_worker_error_struct_carries_full_diagnostic_payload() {
        // The struct is the supervisor's crash report; every field
        // must round-trip so the escalation path can log it. This
        // pins the field set: exit_code, signal, stderr_capture,
        // timestamp — no more, no less.
        let err = WorkerError {
            exit_code: 134, // 128 + SIGABRT(6), the shell convention
            signal: 6,
            stderr_capture: String::from("assertion failed: x > 0"),
            timestamp: 7_700_000,
        };
        assert_eq!(err.exit_code, 134);
        assert_eq!(err.signal, 6);
        assert_eq!(err.stderr_capture, "assertion failed: x > 0");
        assert_eq!(err.timestamp, 7_700_000);
    }

    // ── L8: cryptographic encapsulation tests ──────────────────────

    #[test]
    fn test_crypto_encrypt_decrypt_roundtrip() {
        // The fundamental L8 contract: encrypt then decrypt with the
        // same key must recover the original plaintext, byte-for-byte.
        // This is the test the previous stub passed trivially (because
        // it copied plaintext); it must still pass now that the cipher
        // is a real XOR stream.
        let mut crypto = CryptoState::new([0x42; 32]);
        let plaintext = b"the quick brown fox jumps over the lazy dog";
        let framed = crypto.encrypt(plaintext);
        let recovered = crypto.decrypt(&framed).expect("roundtrip must succeed");
        assert_eq!(recovered, plaintext);
    }

    #[test]
    fn test_crypto_roundtrip_empty_plaintext() {
        // Edge case: zero-length plaintext must still produce a valid
        // frame (nonce + empty ct + tag) and round-trip cleanly. The
        // tag is crc32(b"") == 0, which is a legitimate (if weak) tag
        // value — the test confirms the wire layout handles the
        // degenerate case rather than panicking on an empty slice.
        let mut crypto = CryptoState::new([0xAB; 32]);
        let framed = crypto.encrypt(b"");
        assert_eq!(framed.len(), 12, "empty-plaintext frame = nonce(8) + tag(4)");
        let recovered = crypto.decrypt(&framed).expect("empty roundtrip must succeed");
        assert!(recovered.is_empty());
    }

    #[test]
    fn test_crypto_tag_is_real_crc32_not_zeros() {
        // Pin that the tag is a real CRC32 of the ciphertext, not the
        // all-zero placeholder the previous stub emitted. We verify
        // both halves: the tag is non-zero AND it equals crc32(ct).
        let mut crypto = CryptoState::new([0x11; 32]);
        let framed = crypto.encrypt(b"VUMA");
        assert!(framed.len() >= 12);
        let tag = &framed[framed.len() - 4..];
        assert_ne!(tag, &[0, 0, 0, 0], "tag must not be all-zero");
        // And it must equal crc32 of the ciphertext slice.
        let ct = &framed[8..framed.len() - 4];
        assert_eq!(tag, &crc32(ct).to_le_bytes()[..]);
    }

    #[test]
    fn test_crypto_ciphertext_differs_from_plaintext() {
        // The previous stub copied plaintext as ciphertext verbatim.
        // The real cipher must produce a ciphertext that differs from
        // the plaintext — otherwise there is no encryption at all and
        // the "ENCRYPTED" message flag is a lie. We pick a plaintext
        // of all-same-bytes so any non-trivial key stream guarantees a
        // difference.
        let mut crypto = CryptoState::new([0x55; 32]);
        let plaintext = b"AAAAAAAAAAAAAAAA";
        let framed = crypto.encrypt(plaintext);
        let ct = &framed[8..framed.len() - 4];
        assert_ne!(
            ct, plaintext,
            "ciphertext must not equal plaintext — XOR stream must actually transform bytes"
        );
    }

    #[test]
    fn test_crypto_distinct_nonces_produce_distinct_ciphertexts() {
        // Encrypting the same plaintext twice must produce two
        // different ciphertexts, because the nonce advances. (Nonce
        // reuse in a real cipher would be catastrophic; here we just
        // check the streams actually differ.) This is the property
        // that makes the per-message nonce meaningful.
        let mut crypto = CryptoState::new([0x77; 32]);
        let pt = b"nonce-unique-test";
        let f1 = crypto.encrypt(pt);
        let f2 = crypto.encrypt(pt);
        // Nonces differ...
        assert_ne!(&f1[..8], &f2[..8], "nonce counter must advance");
        // ...and so do ciphertexts.
        let ct1 = &f1[8..f1.len() - 4];
        let ct2 = &f2[8..f2.len() - 4];
        assert_ne!(ct1, ct2, "distinct nonces must yield distinct ciphertexts");
    }

    #[test]
    fn test_crypto_decrypt_rejects_tampered_ciphertext() {
        // Flip a single bit in the ciphertext. The CRC32 tag must no
        // longer match and decrypt must return CrcMismatch — without
        // leaking the (now-wrong) decrypted bytes to the caller. This
        // is the L8 tamper-detection test, analogous to L6's
        // test_restore_state_detects_tampered_sequence.
        let mut crypto = CryptoState::new([0x33; 32]);
        let plaintext = b"sensitive payload";
        let mut framed = crypto.encrypt(plaintext);

        // Tamper: flip a bit in the ciphertext body (not the tag).
        let ct_start = 8;
        let ct_end = framed.len() - 4;
        framed[ct_start] ^= 0x01;

        let err = crypto
            .decrypt(&framed)
            .expect_err("tampered ciphertext must fail tag verification");
        match err {
            IpcError::CrcMismatch { expected, actual } => {
                assert_ne!(expected, actual, "expected and actual tags must differ");
            }
            other => panic!("expected CrcMismatch, got {:?}", other),
        }
    }

    #[test]
    fn test_crypto_decrypt_rejects_tampered_tag() {
        // Flip a bit in the tag itself. Same outcome: CrcMismatch.
        // This confirms the tag is actually being checked, not just
        // present in the wire format.
        let mut crypto = CryptoState::new([0x44; 32]);
        let plaintext = b"another payload";
        let mut framed = crypto.encrypt(plaintext);
        let last = framed.len() - 1;
        framed[last] ^= 0x80;
        let err = crypto
            .decrypt(&framed)
            .expect_err("tampered tag must fail verification");
        assert!(matches!(err, IpcError::CrcMismatch { .. }));
    }

    #[test]
    fn test_crypto_decrypt_rejects_truncated_frame() {
        // A frame shorter than the 12-byte minimum (8 nonce + 4 tag)
        // is a protocol violation, not a crypto failure — surface it
        // as DeserializationError so the L1 framer can distinguish
        // truncation from a bad tag.
        let crypto = CryptoState::new([0x00; 32]);
        let err = crypto
            .decrypt(&[0u8; 5])
            .expect_err("truncated frame must fail");
        assert_eq!(err, IpcError::DeserializationError);
    }

    #[test]
    fn test_crypto_wrong_key_fails_tag_or_garbles() {
        // Decrypting a frame with the wrong key must NOT silently
        // return garbage plaintext. Because the tag is a CRC32 of the
        // *ciphertext* (not of plaintext+key), a wrong key produces a
        // valid-looking frame only if the wrong key stream happens to
        // cancel — which it won't, because the ciphertext is unchanged
        // and so its CRC32 still matches. So the wrong key actually
        // *succeeds* at tag verification but returns wrong plaintext.
        //
        // This is exactly the "structurally correct, not secure"
        // property documented on CryptoState: the tag protects
        // integrity of the ciphertext in transit, NOT
        // authentication of the sender. We pin that behaviour here so
        // the limitation is explicit rather than surprising.
        let mut enc = CryptoState::new([0xAA; 32]);
        let dec = CryptoState::new([0xBB; 32]);
        let framed = enc.encrypt(b"secret");
        let recovered = dec.decrypt(&framed).expect("tag verifies (it covers ct, not key)");
        assert_ne!(
            recovered, b"secret",
            "wrong key must NOT recover the plaintext — pins the documented weakness"
        );
    }

    #[test]
    fn test_aead_cipher_alias_is_crypto_state() {
        // The L8 design notes call the cipher `AeadCipher`; the
        // implementation reuses `CryptoState` (which NoiseChannel::send
        // already constructs). The type alias must keep both names
        // interchangeable so downstream code can use either name at
        // the call site.
        let mut aead: AeadCipher = AeadCipher::new([0x99; 32]);
        let framed = aead.encrypt(b"alias check");
        // `aead` mut borrow ends here; now borrow the same value as
        // CryptoState to call decrypt.
        let recovered = CryptoState::decrypt(&aead, &framed).expect("alias roundtrip");
        assert_eq!(recovered, b"alias check");
    }
}
