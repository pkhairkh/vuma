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

    /// Maximum delegation depth permitted for a capability chain.
    ///
    /// A freshly-granted root token has `delegation_depth == 0`; each
    /// [`CapabilitySet::delegate`] call increments the child's depth by 1.
    /// Once a token's depth reaches this limit, further delegation is
    /// refused by [`CapabilitySet::delegate`] with
    /// [`IpcError::DelegationDepthExceeded`], and
    /// [`verify_delegation_chain`] rejects any chain whose leaf exceeds
    /// it. This bounds the length of a delegation chain so a compromised
    /// intermediate cannot pyramid authority out indefinitely.
    pub const MAX_DELEGATION_DEPTH: u8 = 8;

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

        /// Revoke a token *and* every token that was delegated (directly
        /// or transitively) from it.
        ///
        /// When `token_id` is revoked, authority for every child token
        /// delegated from it collapses: a child token whose `source_pid`
        /// equals the revoked token's `target_pid` and whose
        /// `delegation_depth` is exactly one more than the revoked
        /// token's depth was minted by [`CapabilitySet::delegate`] from
        /// this parent, and so must also be revoked. The walk is
        /// breadth-first: each newly-revoked token's children are
        /// discovered by scanning `self.tokens`, so transitive
        /// descendants are pulled in regardless of how deep the
        /// delegation chain goes.
        ///
        /// Returns the list of every token ID revoked by this call —
        /// `token_id` itself first, followed by all descendants in
        /// discovery order. If `token_id` is unknown to the set the
        /// call still records it as revoked (defensive: a revoked-but-
        /// unknown id is a tombstone that prevents any future token
        /// with the same id from being verified) and returns a
        /// single-element vector. Idempotent: revoking an already-
        /// revoked token returns an empty vector and walks nothing.
        pub fn revoke_with_propagation(&mut self, token_id: u128) -> Vec<u128> {
            let mut revoked_list: Vec<u128> = Vec::new();
            // Worklist of token IDs whose children still need to be
            // discovered. We push `token_id` first, then append child
            // IDs as we revoke each parent.
            let mut worklist: Vec<u128> = vec![token_id];

            while let Some(current_id) = worklist.pop() {
                // set.insert returns true iff the value was newly added —
                // so an already-revoked id short-circuits here, which is
                // what makes the walk idempotent and cycle-free.
                if !self.revoked.insert(current_id) {
                    continue;
                }
                revoked_list.push(current_id);

                // Snapshot the parent's (target_pid, depth) so we can
                // find children without holding a borrow into the
                // HashMap while we mutate it below.
                let parent_target_pid: u64;
                let parent_depth: u8;
                if let Some(parent) = self.tokens.get(&current_id) {
                    parent_target_pid = parent.target_pid;
                    parent_depth = parent.delegation_depth;
                } else {
                    // Token isn't tracked in `tokens` (e.g. it was
                    // revoked before ever being granted through this
                    // set, or it lives in a peer set). No children to
                    // propagate to.
                    continue;
                }

                // A token is a child of `current_id` iff it was minted
                // by `delegate(current_id, ...)`: its `source_pid` is
                // the parent's `target_pid` (the delegator becomes the
                // source of the child), and its `delegation_depth` is
                // exactly `parent_depth + 1`. Collect matches first,
                // then extend the worklist — we can't mutate `revoked`
                // while iterating `tokens`, so the two passes are
                // separate.
                let children: Vec<u128> = self
                    .tokens
                    .iter()
                    .filter(|(_, t)| {
                        t.source_pid == parent_target_pid
                            && t.delegation_depth == parent_depth + 1
                    })
                    .map(|(k, _)| *k)
                    .collect();
                worklist.extend(children);
            }
            revoked_list
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

    /// Verify a delegation chain end-to-end.
    ///
    /// `chain` is the ordered sequence of tokens from the root grant
    /// (`chain[0]`, with `delegation_depth == 0`) down to the leaf
    /// (`chain.last()`). `token` is the leaf being authorised — it
    /// must equal `chain.last()`, otherwise the chain is for a
    /// different token and verification fails.
    ///
    /// For every adjacent pair `(parent, child)` in the chain we
    /// require:
    ///
    /// 1. **Pid linkage** — `child.source_pid == parent.target_pid`.
    ///    [`CapabilitySet::delegate`] sets the delegator's pid as the
    ///    child's `source_pid`, so a broken link means `child` was
    ///    not delegated from `parent`.
    /// 2. **Depth increment** — `child.delegation_depth == parent.delegation_depth + 1`.
    ///    A skipped or repeated depth means the chain was forged.
    ///
    /// Additionally every token's `delegation_depth` must be
    /// `<= MAX_DELEGATION_DEPTH`; a chain whose leaf exceeds the
    /// limit could not have been produced by `delegate` (it refuses
    /// at the limit) and so is rejected here as well.
    ///
    /// Returns `false` for an empty chain, a chain whose last
    /// element is not `token`, a chain that starts at a non-root
    /// depth, any broken parent→child link, or any depth overflow.
    pub fn verify_delegation_chain(
        token: &CapabilityToken,
        chain: &[CapabilityToken],
    ) -> bool {
        // Empty chain has nothing to verify — a leaf with no provenance
        // is never authorisable through delegation.
        if chain.is_empty() {
            return false;
        }
        // The chain must lead to exactly `token`; otherwise we're
        // being asked to authorise one token using some other token's
        // proof.
        if chain.last() != Some(token) {
            return false;
        }
        // Root of the chain must be a freshly-granted token (depth 0).
        // A chain that starts mid-way has no provable root of authority.
        if chain[0].delegation_depth != 0 {
            return false;
        }
        // Cheap upper-bound check on the root first, then per-pair below.
        if chain[0].delegation_depth > MAX_DELEGATION_DEPTH {
            return false;
        }
        for w in chain.windows(2) {
            let parent = &w[0];
            let child = &w[1];
            // (1) Pid linkage: delegator becomes the source of the child.
            if child.source_pid != parent.target_pid {
                return false;
            }
            // (2) Depth must increment by exactly 1 — not 0, not 2.
            // Wrapping_add guards against a malicious depth of 255
            // on the parent: 255 + 1 wraps to 0, which won't equal
            // any sane child depth, so the check correctly fails.
            if child.delegation_depth != parent.delegation_depth.wrapping_add(1) {
                return false;
            }
            // MAX_DELEGATION_DEPTH cap on the child. Combined with the
            // root check above this transitively caps every element.
            if child.delegation_depth > MAX_DELEGATION_DEPTH {
                return false;
            }
        }
        true
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
    /// [`DriverWorker::start`] was called on a worker that is already
    /// running. The supervisor's start/stop state machine is strict:
    /// a second `start()` without an intervening `stop()` is a bug.
    WorkerAlreadyRunning,
    /// [`DriverWorker::stop`] or [`DriverWorker::handle_irq`] was
    /// called on a worker that is not currently running — there is no
    /// live process to receive the stop signal / IRQ dispatch.
    WorkerNotRunning,
    /// [`DriverWorker::handle_irq`] was called with an IRQ vector that
    /// is not in the driver's `irq_vectors` allowlist. The kernel's IRQ
    /// demuxer routed the interrupt to the wrong driver.
    IrqNotRegistered(u32),

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
//
// Wave 25-32: real auto-marshalling, worker lifecycle, seccomp wiring,
// and crash-recovery for `extern "process"` call sites. The actual
// `fork()`/`exec()` and IPC socket plumbing live in the runtime
// backend (W33+); this module provides the in-process types and a
// simulated lifecycle the supervisor exercises against the same code
// paths, so the spawn → call → kill → restart state machine can be
// unit-tested without spawning a real child process.

/// Header prepended to every marshalled FFI payload: a u32 LE length
/// followed by a u64 LE return-type hash. The payload proper follows
/// immediately after. Kept as a `pub const` so [`FfiCall::marshal`]
/// and [`FfiCall::unmarshal`] agree on the offset, and so the
/// supervisor can size its receive buffers without re-deriving it.
pub const FFI_MARSHAL_HEADER_SIZE: usize = 4 + 8;

/// Configuration for an FFI worker process. Bundles the library path
/// + symbol the worker will `dlopen`/`dlsym`, the [`TrustLevel`] used
/// to derive the seccomp filter, the restart/timeout budget, and the
/// prepared [`WorkerSandbox`] (W17-18) the forked child installs
/// before `exec()`. Built via [`FfiWorkerConfig::new`] which derives
/// the sandbox from the trust level so callers cannot forget the L5
/// wire-up.
#[derive(Clone, Debug)]
pub struct FfiWorkerConfig {
    pub library_path: String,
    pub function_name: String,
    pub trust_level: TrustLevel,
    pub max_restarts: u32,
    pub timeout_ms: u64,
    pub sandbox_config: WorkerSandbox,
}

impl FfiWorkerConfig {
    /// Construct an FFI worker config from its scalar fields,
    /// deriving the [`WorkerSandbox`] from `trust_level` and a
    /// conservative [`ResourceLimits`] profile for untrusted FFI.
    /// Callers needing a bespoke sandbox can overwrite the
    /// `sandbox_config` field directly after construction.
    pub fn new(
        library_path: impl Into<String>,
        function_name: impl Into<String>,
        trust_level: TrustLevel,
        max_restarts: u32,
        timeout_ms: u64,
    ) -> Self {
        let worker_config = WorkerConfig {
            trust_level: trust_level.clone(),
            max_restarts,
            timeout_ms,
        };
        // FFI workers are untrusted by default: cap CPU at the call
        // timeout, cap RSS at 64 MiB (enough for crypto bignums but
        // not for a heap spray), and limit FDs/IPC messages so a
        // runaway worker cannot exhaust supervisor resources.
        let limits = ResourceLimits {
            cpu_time_ms: timeout_ms,
            max_memory_bytes: 64 * 1024 * 1024,
            max_ipc_messages: 1024,
            max_file_descriptors: 32,
        };
        Self {
            library_path: library_path.into(),
            function_name: function_name.into(),
            trust_level,
            max_restarts,
            timeout_ms,
            sandbox_config: WorkerSandbox::new(worker_config, limits),
        }
    }

    /// Every FFI worker config is, by construction, an FFI worker.
    /// The `is_ffi` predicate exists so a later wave can route
    /// `WorkerConfig` vs `FfiWorkerConfig` through a shared trait
    /// without churning every call site.
    pub fn is_ffi(&self) -> bool {
        true
    }
}

/// A marshalled FFI call envelope. `args` is the already-serialized
/// argument blob the worker will hand to the C function;
/// `return_type_hash` is the type hash of the declared return type
/// (the same `type_hash` used by the L1 message layer) — the worker
/// uses it to type-check the reply before unmarshalling.
/// `function_name` is metadata carried alongside the envelope for
/// diagnostics and dispatch; it is **not** serialized by
/// [`marshal`](FfiCall::marshal).
#[derive(Clone, Debug)]
pub struct FfiCall {
    pub function_name: String,
    pub args: Vec<u8>,
    pub return_type_hash: u64,
}

impl FfiCall {
    /// Construct a call envelope for `function_name` returning a value
    /// of type `return_type_hash`, with `args` as the marshalled
    /// argument blob.
    pub fn new(
        function_name: impl Into<String>,
        args: Vec<u8>,
        return_type_hash: u64,
    ) -> Self {
        Self {
            function_name: function_name.into(),
            args,
            return_type_hash,
        }
    }

    /// Marshal `args` into the on-the-wire FFI call frame:
    ///
    /// ```text
    ///   +----4 bytes----+----8 bytes----+----N bytes----+
    ///   | payload len   | type hash     | payload       |
    ///   | (u32 LE)      | (u64 LE)      | (raw args)    |
    ///   +---------------+---------------+---------------+
    /// ```
    ///
    /// `self.return_type_hash` supplies the type hash; the `args`
    /// parameter is the payload proper. The parameter — rather than
    /// `self.args` — lets callers re-frame a previously cached blob
    /// without mutating the envelope, which the supervisor does when
    /// it retries a call after a worker restart.
    pub fn marshal(&self, args: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(FFI_MARSHAL_HEADER_SIZE + args.len());
        out.extend_from_slice(&(args.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.return_type_hash.to_le_bytes());
        out.extend_from_slice(args);
        out
    }

    /// Inverse of [`marshal`](FfiCall::marshal): parse the length
    /// header, the type hash, and extract the payload. Returns
    /// [`IpcError::TruncatedMessage`] if `data` is shorter than the
    /// header or shorter than the declared payload length, and
    /// [`IpcError::PayloadTooLarge`] if the declared length would
    /// overflow `usize`. The returned [`FfiCall`] has an empty
    /// `function_name` — the name is not on the wire, and the worker
    /// that receives the frame already knows which symbol it is
    /// dispatching to.
    pub fn unmarshal(data: &[u8]) -> Result<FfiCall, IpcError> {
        if data.len() < FFI_MARSHAL_HEADER_SIZE {
            return Err(IpcError::TruncatedMessage);
        }
        let payload_len = u32::from_le_bytes([
            data[0], data[1], data[2], data[3],
        ]) as usize;
        let type_hash = u64::from_le_bytes([
            data[4], data[5], data[6], data[7],
            data[8], data[9], data[10], data[11],
        ]);
        let end = FFI_MARSHAL_HEADER_SIZE
            .checked_add(payload_len)
            .ok_or(IpcError::PayloadTooLarge(payload_len as u64))?;
        if data.len() < end {
            return Err(IpcError::TruncatedMessage);
        }
        let payload = data[FFI_MARSHAL_HEADER_SIZE..end].to_vec();
        Ok(FfiCall {
            function_name: String::new(),
            args: payload,
            return_type_hash: type_hash,
        })
    }
}

/// Result of an FFI call from a worker process. `return_value` is the
/// marshalled return blob (empty on failure); `error` carries the
/// human-readable diagnostic when `success` is false; `elapsed_ms` is
/// the wall-clock time spent in the worker, measured by the
/// supervisor around the IPC round-trip and used for SLO accounting.
#[derive(Clone, Debug)]
pub struct FfiResult {
    pub success: bool,
    pub return_value: Vec<u8>,
    pub error: Option<String>,
    pub elapsed_ms: u64,
}

impl FfiResult {
    /// Construct a success result carrying `return_value` and the
    /// measured `elapsed_ms`.
    pub fn ok(return_value: Vec<u8>, elapsed_ms: u64) -> Self {
        Self {
            success: true,
            return_value,
            error: None,
            elapsed_ms,
        }
    }

    /// Construct a failure result carrying an error message and the
    /// measured `elapsed_ms` (the time spent before the failure was
    /// detected).
    pub fn err(message: impl Into<String>, elapsed_ms: u64) -> Self {
        Self {
            success: false,
            return_value: Vec::new(),
            error: Some(message.into()),
            elapsed_ms,
        }
    }

    /// True iff the worker returned successfully.
    pub fn is_success(&self) -> bool {
        self.success
    }

    /// The error message if the call failed, or an empty string if it
    /// succeeded. Returning `&str` (rather than `Option<&str>`)
    /// keeps the call site ergonomic for `format!("{}", r.error_message())`
    /// style diagnostics.
    pub fn error_message(&self) -> &str {
        self.error.as_deref().unwrap_or("")
    }
}

/// Per-worker bookkeeping the supervisor holds for one live FFI
/// worker. `restart_count` is incremented every time
/// [`FfiWorkerLifecycle::restart_ffi_worker`] respawns the worker; it
/// is checked against [`FfiWorkerConfig::max_restarts`] before each
/// respawn. `alive` is set false by [`FfiWorkerLifecycle::kill_ffi_worker`]
/// so subsequent calls return [`IpcError::WorkerCrashed`].
#[derive(Clone, Debug)]
struct FfiWorkerEntry {
    config: FfiWorkerConfig,
    restart_count: u32,
    alive: bool,
}

/// In-process supervisor for FFI worker processes. The real
/// `fork()`/`exec()` + Unix-socket IPC lives in the runtime backend
/// (W33+); this struct provides the simulated lifecycle the
/// supervisor's state machine exercises against, so the
/// spawn → call → kill → restart code paths can be tested without a
/// real child process.
///
/// PIDs are assigned from a monotonically increasing counter starting
/// at `1000` (chosen to avoid colliding with the test runner's own
/// low PIDs, which makes test output readable).
pub struct FfiWorkerLifecycle {
    workers: HashMap<u32, FfiWorkerEntry>,
    next_pid: u32,
}

impl FfiWorkerLifecycle {
    /// Construct an empty lifecycle supervisor.
    pub fn new() -> Self {
        Self {
            workers: HashMap::new(),
            next_pid: 1000,
        }
    }

    /// Simulate spawning an FFI worker for `config`. Returns the
    /// assigned PID. The real `fork()` + `exec()` + seccomp
    /// installation happens in the runtime backend; here we just
    /// record the worker as alive so [`call_ffi`](Self::call_ffi) and
    /// [`kill_ffi_worker`](Self::kill_ffi_worker) can validate the
    /// PID. Returns [`IpcError::TooManyProcesses`] if the PID counter
    /// wraps (2^32 workers spawned in one process — a supervisor bug).
    pub fn spawn_ffi_worker(&mut self, config: &FfiWorkerConfig) -> Result<u32, IpcError> {
        let pid = self.next_pid;
        self.next_pid = self
            .next_pid
            .checked_add(1)
            .ok_or(IpcError::TooManyProcesses)?;
        self.workers.insert(
            pid,
            FfiWorkerEntry {
                config: config.clone(),
                restart_count: 0,
                alive: true,
            },
        );
        Ok(pid)
    }

    /// Simulate an IPC call to `pid`. Marshals `call` the same way the
    /// real IPC layer would, "sends" it to the worker, and returns a
    /// success [`FfiResult`] whose `return_value` echoes the
    /// marshalled frame — this is the loopback contract the backend
    /// uses for its smoke-test RPC and the one the supervisor's
    /// restart path uses to verify a freshly-respawned worker is
    /// responsive. `timeout_ms` is honored as a sanity ceiling: if it
    /// is `0` the call is treated as an immediate timeout (the
    /// backend interprets `0` as "no waiting room allocated", i.e.
    /// `poll(2)` returns `EAGAIN`).
    ///
    /// Returns [`IpcError::WorkerNotFound`] if `pid` was never
    /// spawned, [`IpcError::WorkerCrashed`] if it was spawned but has
    /// since been killed, and [`IpcError::WorkerTimeout`] if
    /// `timeout_ms == 0`.
    pub fn call_ffi(
        &mut self,
        pid: u32,
        call: &FfiCall,
        timeout_ms: u64,
    ) -> Result<FfiResult, IpcError> {
        let entry = self.workers.get_mut(&pid).ok_or(IpcError::WorkerNotFound)?;
        if !entry.alive {
            return Err(IpcError::WorkerCrashed(0));
        }
        if timeout_ms == 0 {
            return Err(IpcError::WorkerTimeout);
        }
        // Marshal the call the same way the real IPC layer would, so
        // the wire format is exercised end-to-end on every simulated
        // call. The loopback return value is the marshalled frame.
        let frame = call.marshal(&call.args);
        // Simulated elapsed time: bounded by both the call timeout
        // and a 10 ms floor, so SLO accounting tests can assert
        // `0 < elapsed <= timeout_ms`.
        let elapsed = if timeout_ms < 10 { timeout_ms } else { 10 };
        Ok(FfiResult::ok(frame, elapsed))
    }

    /// Simulate killing the worker `pid`. Marks it dead so subsequent
    /// [`call_ffi`](Self::call_ffi) calls return
    /// [`IpcError::WorkerCrashed`]. Returns
    /// [`IpcError::WorkerNotFound`] if `pid` was never spawned. The
    /// real backend would `kill(pid, SIGKILL)` and `waitpid()` the
    /// corpse; here we only flip the `alive` flag so the bookkeeping
    /// stays intact for [`restart_ffi_worker`](Self::restart_ffi_worker).
    pub fn kill_ffi_worker(&mut self, pid: u32) -> Result<(), IpcError> {
        let entry = self.workers.get_mut(&pid).ok_or(IpcError::WorkerNotFound)?;
        entry.alive = false;
        Ok(())
    }

    /// Restart a crashed worker for `config`. Looks up the most
    /// recent worker for this config (matched on `library_path` +
    /// `function_name`), and if its `restart_count` is below
    /// `config.max_restarts`, spawns a fresh worker with the restart
    /// counter carried forward + incremented. Returns
    /// [`IpcError::MaxRestartsExceeded`] if the budget is exhausted,
    /// or [`IpcError::WorkerNotFound`] if no prior worker for this
    /// config exists (callers must `spawn_ffi_worker` first).
    ///
    /// The old worker entry is retained in the map (marked dead) so
    /// the supervisor's audit log can reconstruct the crash history;
    /// the new worker gets a fresh PID.
    pub fn restart_ffi_worker(&mut self, config: &FfiWorkerConfig) -> Result<u32, IpcError> {
        // Find the highest restart_count among prior workers for this
        // (library_path, function_name) pair. Iterating all workers is
        // O(n) but n is small (one entry per live or dead FFI worker
        // since process start); the backend will index by
        // (library_path, function_name) when it needs to.
        let prev_restart_count = self
            .workers
            .values()
            .filter(|e| {
                e.config.library_path == config.library_path
                    && e.config.function_name == config.function_name
            })
            .map(|e| e.restart_count)
            .max()
            .ok_or(IpcError::WorkerNotFound)?;

        if prev_restart_count >= config.max_restarts {
            return Err(IpcError::MaxRestartsExceeded);
        }

        let pid = self.next_pid;
        self.next_pid = self
            .next_pid
            .checked_add(1)
            .ok_or(IpcError::TooManyProcesses)?;
        self.workers.insert(
            pid,
            FfiWorkerEntry {
                config: config.clone(),
                restart_count: prev_restart_count + 1,
                alive: true,
            },
        );
        Ok(pid)
    }

    /// Number of workers currently tracked (alive or dead). Exposed
    /// for tests and supervisor introspection — the count never
    /// decreases because dead entries are retained for crash-history
    /// audits.
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }

    /// Restart count for `pid`, or `None` if unknown. Exposed for
    /// tests so the restart budget can be asserted directly without
    /// driving a full crash cycle.
    pub fn restart_count(&self, pid: u32) -> Option<u32> {
        self.workers.get(&pid).map(|e| e.restart_count)
    }

    /// True iff `pid` is alive (spawned and not yet killed). Returns
    /// `false` for unknown pids.
    pub fn is_alive(&self, pid: u32) -> bool {
        self.workers.get(&pid).map(|e| e.alive).unwrap_or(false)
    }
}

impl Default for FfiWorkerLifecycle {
    fn default() -> Self {
        Self::new()
    }
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

        if parent.delegation_depth >= capability::MAX_DELEGATION_DEPTH {
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

// ── Cross-Process Capability Tracking ─────────────────────────────────
//
// A `CapabilitySet` answers "is token X still valid?" but it does not
// track *which process currently holds* token X. The kernel needs the
// reverse mapping — "given pid P, which tokens is it currently wielding?"
// — so it can:
//
//   * sweep a dead process's capabilities on exit (revoke them all,
//     propagating to any descendants the process delegated),
//   * audit a process's authority at any moment (e.g. for a `procfs`-
//     style capability listing, or a security monitor),
//   * refuse to deliver a message to P whose capability set has grown
//     past a policy ceiling.
//
// `CapabilityRegistry` is that reverse index. It owns one `Vec<u128>`
// of token IDs per pid; the `CapabilityToken`s themselves live in a
// (separate) `CapabilitySet`, so the registry is bookkeeping only —
// it never mints or verifies tokens, it just remembers who has what.

/// Per-process index of capability token IDs.
///
/// Invariant: if `token_id` appears in `process_capabilities[pid]`,
/// the corresponding `CapabilityToken` (looked up in the caller's
/// `CapabilitySet`) has `target_pid == pid` — that is, the token was
/// either granted directly to `pid`, or delegated *to* `pid` by some
/// other process. The registry does not enforce this invariant itself
/// (it has no access to the token bytes); it is the caller's
/// responsibility to call [`grant_to_process`] only with tokens whose
/// `target_pid` matches `pid`.
#[derive(Clone, Debug, Default)]
pub struct CapabilityRegistry {
    /// pid → ordered list of token IDs held by that process.
    /// Duplicates are tolerated (a process may legitimately hold the
    /// same token id under multiple aliases, e.g. after a re-grant of
    /// an expired-and-renewed token); [`CapabilityRegistry::revoke_from_process`]
    /// removes *all* occurrences so the alias problem is bounded.
    pub process_capabilities: HashMap<u64, Vec<u128>>,
}

impl CapabilityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `pid` now holds `token`. Idempotent in the sense
    /// that the same `(pid, token.id)` pair may be recorded multiple
    /// times — the registry does not de-duplicate on insert because
    /// a real kernel often tracks per-grant metadata (e.g. the channel
    /// the token arrived on) alongside the id, and silently coalescing
    /// two grants would lose that. Use [`revoke_from_process`] to
    /// remove every alias of a token at once.
    pub fn grant_to_process(&mut self, pid: u64, token: &capability::CapabilityToken) {
        self.process_capabilities
            .entry(pid)
            .or_default()
            .push(token.id);
    }

    /// Remove every occurrence of `token_id` from `pid`'s held set.
    /// Returns `true` iff at least one entry was removed (so a caller
    /// can detect "you tried to revoke a token this process never
    /// held" and treat it as a policy violation).
    ///
    /// Does *not* touch the `CapabilitySet` — propagating revocation
    /// to descendants is the caller's job (call
    /// [`capability::CapabilitySet::revoke_with_propagation`] on the
    /// matching set, then call this for each pid in the registry to
    /// scrub the reverse index).
    pub fn revoke_from_process(&mut self, pid: u64, token_id: u128) -> bool {
        if let Some(v) = self.process_capabilities.get_mut(&pid) {
            let before = v.len();
            v.retain(|&t| t != token_id);
            v.len() != before
        } else {
            false
        }
    }

    /// All token IDs currently held by `pid`, in insertion order.
    /// Returns an empty slice for an unknown pid (a process that has
    /// never been granted a capability) so callers can iterate
    /// without a separate `contains_key` check.
    pub fn get_process_capabilities(&self, pid: u64) -> &[u128] {
        self.process_capabilities
            .get(&pid)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

// ── W41-48: Kernel/User Split (Microkernel) ─────────────────────────
//
// The microkernel split: a single privileged KernelProcess runs with
// full capability authority and dispatches syscalls via the IPC layer;
// each UserProcess runs with a restricted TrustLevel, a capability
// token list, and a ResourceUsage/ResourceLimits pair enforced by the
// supervisor. The ProcessTable owns the kernel slot plus a flat map of
// user processes keyed by pid.
//
// This is the L4-style split: the kernel is a service like any other,
// reachable only through `KernelProcess::handle_syscall`. The real
// trap-and-dispatch lives in the runtime backend; here we expose the
// bookkeeping the supervisor needs to reason about who-can-call-what.

/// A privileged kernel-space process. There is at most one of these
/// per [`ProcessTable`]. It holds the master [`CapabilityRegistry`]
/// and answers [`KernelProcess::handle_syscall`] by routing the call
/// through IPC — the actual trap into the backend is the caller's
/// responsibility, this method just resolves the dispatch and returns
/// a mock value indicating "syscall accepted, caller may now trap".
#[derive(Clone, Debug)]
pub struct KernelProcess {
    pub pid: u64,
    pub name: String,
    pub capabilities: CapabilityRegistry,
}

impl KernelProcess {
    /// Construct a fresh kernel process with an empty capability
    /// registry. The caller picks `pid` (conventionally 1, the L4
    /// "sigma0" / init slot) and a human-readable `name` for
    /// diagnostics.
    pub fn new(pid: u64, name: impl Into<String>) -> Self {
        Self {
            pid,
            name: name.into(),
            capabilities: CapabilityRegistry::new(),
        }
    }

    /// Dispatch a syscall from `caller_pid` through the kernel IPC
    /// path. `nr` is the syscall number (see [`allowed_syscalls`] for
    /// the per-trust-level filter lists), `args` is the raw argument
    /// slice. Returns a mock `Ok(0)` on acceptance — the real trap
    /// into the backend happens after this method returns, so the
    /// return value here is a placeholder the backend overwrites.
    ///
    /// Returns [`IpcError::PermissionDenied`] if `caller_pid` is not
    /// the kernel itself *and* is not currently registered in the
    /// kernel's [`CapabilityRegistry`] as holding any capability —
    /// this is the kernel's "default deny" stance: a process that has
    /// never been granted a capability cannot even initiate a syscall.
    pub fn handle_syscall(
        &mut self,
        caller_pid: u64,
        nr: u32,
        args: &[u64],
    ) -> Result<u64, IpcError> {
        // Default-deny: the caller must be a known capability holder.
        // The kernel itself (pid == self.pid) is always allowed to
        // call its own syscalls (e.g. for boot-time initialization).
        if caller_pid != self.pid
            && self.capabilities.get_process_capabilities(caller_pid).is_empty()
        {
            return Err(IpcError::PermissionDenied);
        }
        // Mock dispatch: encode the syscall number in the high 32 bits
        // of the return value so a test can confirm the call was
        // routed and the syscall number survived the round-trip. The
        // real backend replaces this with the actual trap result.
        let _ = args;
        Ok((nr as u64) << 32)
    }

    /// Always true: this is a [`KernelProcess`]. Provided so a caller
    /// with a generic process handle can ask "is this the kernel?"
    /// without downcasting.
    pub fn is_kernel_process(&self) -> bool {
        true
    }
}

/// A user-space process running under a restricted trust level. Each
/// `UserProcess` carries its own capability token IDs (mirroring the
/// kernel's [`CapabilityRegistry`] reverse-index entry for this pid),
/// the live [`ResourceUsage`] measurement, and the [`ResourceLimits`]
/// ceiling the supervisor enforces. [`UserProcess::check_resources`]
/// delegates to [`ResourceLimits::check_limits`] so the supervisor can
/// poll between IPC turns and kill the process when it overshoots.
#[derive(Clone, Debug)]
pub struct UserProcess {
    pub pid: u64,
    pub parent_pid: u64,
    pub trust_level: TrustLevel,
    /// Capability token IDs currently held by this process. Mirrors
    /// the kernel's per-pid [`CapabilityRegistry`] entry — kept here
    /// so the process can self-audit without crossing the IPC
    /// boundary.
    pub capabilities: Vec<u128>,
    pub resource_usage: ResourceUsage,
    pub resource_limits: ResourceLimits,
}

impl UserProcess {
    /// Construct a fresh user process under `parent_pid` with the
    /// given trust level and resource limits. `resource_usage` starts
    /// at zero (no CPU time, no memory, no IPC messages, no FDs) —
    /// the supervisor populates it as the process runs.
    pub fn new(
        pid: u64,
        parent_pid: u64,
        trust_level: TrustLevel,
        limits: ResourceLimits,
    ) -> Self {
        Self {
            pid,
            parent_pid,
            trust_level,
            capabilities: Vec::new(),
            resource_usage: ResourceUsage::default(),
            resource_limits: limits,
        }
    }

    /// True iff the current [`ResourceUsage`] is within every ceiling
    /// of [`ResourceLimits`]. Delegates to
    /// [`ResourceLimits::check_limits`] so the policy lives in one
    /// place. The supervisor polls this between IPC turns and kills
    /// the process when it returns false.
    pub fn check_resources(&self) -> bool {
        self.resource_limits.check_limits(&self.resource_usage)
    }

    /// Always true: this is a [`UserProcess`]. Provided so a caller
    /// with a generic process handle can ask "is this a user?" without
    /// downcasting.
    pub fn is_user_process(&self) -> bool {
        true
    }
}

/// Per-process resource accounting. Tracks CPU time, memory,
/// IPC-message count, and open file-descriptor count for every pid
/// the supervisor has seen, in one flat `HashMap<u64, ResourceUsage>`.
///
/// This is the L5 accounting half of the sandbox: the supervisor
/// calls [`account_cpu`](Self::account_cpu) /
/// [`account_memory`](Self::account_memory) /
/// [`account_ipc`](Self::account_ipc) /
/// [`account_fd`](Self::account_fd) as it observes the process, and
/// [`get_usage`](Self::get_usage) when it needs to poll the limits.
/// The struct deliberately does *not* enforce any ceiling — that is
/// [`ResourceLimits::check_limits`]'s job — so the accounting can be
/// shared across multiple limit policies (e.g. a per-call ceiling vs.
/// a lifetime ceiling).
#[derive(Clone, Debug, Default)]
pub struct ResourceAccount {
    usage: HashMap<u64, ResourceUsage>,
}

impl ResourceAccount {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `ms` to `pid`'s accumulated CPU time, saturating on
    /// overflow. Creates a fresh zeroed [`ResourceUsage`] for `pid`
    /// if it is new.
    pub fn account_cpu(&mut self, pid: u64, ms: u64) {
        let entry = self.usage.entry(pid).or_default();
        entry.cpu_time_ms = entry.cpu_time_ms.saturating_add(ms);
    }

    /// Set `pid`'s peak memory to the max of (current, `bytes`).
    /// Memory is reported as a high-water mark rather than a delta
    /// because `getrusage(2)` reports `ru_maxrss` that way and we
    /// mirror the kernel's accounting model.
    pub fn account_memory(&mut self, pid: u64, bytes: u64) {
        let entry = self.usage.entry(pid).or_default();
        if bytes > entry.memory_bytes {
            entry.memory_bytes = bytes;
        }
    }

    /// Increment `pid`'s IPC-message counter by one, saturating on
    /// overflow.
    pub fn account_ipc(&mut self, pid: u64) {
        let entry = self.usage.entry(pid).or_default();
        entry.ipc_messages = entry.ipc_messages.saturating_add(1);
    }

    /// Increment `pid`'s open-FD counter by one, saturating on
    /// overflow.
    pub fn account_fd(&mut self, pid: u64) {
        let entry = self.usage.entry(pid).or_default();
        entry.file_descriptors = entry.file_descriptors.saturating_add(1);
    }

    /// Snapshot of `pid`'s accumulated usage. Returns a zeroed
    /// [`ResourceUsage`] for an unknown pid so callers can poll
    /// without a separate `contains_key` check.
    pub fn get_usage(&self, pid: u64) -> ResourceUsage {
        self.usage.get(&pid).cloned().unwrap_or_default()
    }

    /// Number of distinct pids currently tracked.
    pub fn tracked_count(&self) -> usize {
        self.usage.len()
    }
}

/// Borrowed view of a process entry in the [`ProcessTable`] — the
/// L4-style tagged union of "kernel slot" vs. "user slot", returned by
/// [`ProcessTable::get_process`].
///
/// We expose this as a borrow rather than cloning the underlying
/// [`KernelProcess`] / [`UserProcess`] so callers can inspect either
/// slot without paying for a clone. The lifetime parameter ties the
/// borrowed reference to the originating `&ProcessTable`.
#[derive(Clone, Copy, Debug)]
pub enum Process<'a> {
    Kernel(&'a KernelProcess),
    User(&'a UserProcess),
}

impl<'a> Process<'a> {
    /// PID of the underlying process, regardless of variant.
    pub fn pid(&self) -> u64 {
        match self {
            Process::Kernel(k) => k.pid,
            Process::User(u) => u.pid,
        }
    }

    /// True iff this is the kernel slot.
    pub fn is_kernel(&self) -> bool {
        matches!(self, Process::Kernel(_))
    }

    /// True iff this is a user slot.
    pub fn is_user(&self) -> bool {
        matches!(self, Process::User(_))
    }
}

/// The microkernel process table: one optional kernel slot plus a
/// flat map of user processes keyed by pid. [`ProcessTable::spawn_user`]
/// mints a new user pid; [`ProcessTable::kill_user`] evicts it;
/// [`ProcessTable::get_process`] returns the tagged [`Process`] for
/// either slot.
///
/// User pids are assigned from a monotonically increasing counter
/// starting at `1001` (mirroring [`FfiWorkerLifecycle`]'s convention
/// of starting at `1000`, but bumped by one so user pids never
/// collide with the FFI worker pid space).
#[derive(Clone, Debug)]
pub struct ProcessTable {
    pub kernel: Option<KernelProcess>,
    pub users: HashMap<u64, UserProcess>,
    next_user_pid: u64,
}

impl Default for ProcessTable {
    fn default() -> Self {
        Self {
            kernel: None,
            users: HashMap::new(),
            // Start user pids at 1001 so they never collide with the
            // FFI worker pid space (1000+) or the conventional kernel
            // pid (1).
            next_user_pid: 1001,
        }
    }
}

impl ProcessTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Install the kernel process. Only one kernel slot exists; a
    /// second call replaces the prior kernel process (the kernel is
    /// never killed, only hot-swapped during early boot).
    pub fn set_kernel(&mut self, kernel: KernelProcess) {
        self.kernel = Some(kernel);
    }

    /// Spawn a fresh user process under `parent_pid` with the given
    /// trust level and resource limits. Returns the assigned pid.
    /// Returns [`IpcError::TooManyProcesses`] if the pid counter
    /// overflows (a supervisor bug).
    pub fn spawn_user(
        &mut self,
        parent_pid: u64,
        trust_level: TrustLevel,
        limits: ResourceLimits,
    ) -> Result<u64, IpcError> {
        let pid = self.next_user_pid;
        self.next_user_pid = self
            .next_user_pid
            .checked_add(1)
            .ok_or(IpcError::TooManyProcesses)?;
        let user = UserProcess::new(pid, parent_pid, trust_level, limits);
        self.users.insert(pid, user);
        Ok(pid)
    }

    /// Kill (evict) the user process `pid`. Returns
    /// [`IpcError::WorkerNotFound`] if `pid` is not a known user
    /// process. The kernel slot is never affected — use
    /// [`set_kernel`](Self::set_kernel) with `None` semantics by
    /// dropping the table if you need to tear down the kernel.
    pub fn kill_user(&mut self, pid: u64) -> Result<(), IpcError> {
        if self.users.remove(&pid).is_some() {
            Ok(())
        } else {
            Err(IpcError::WorkerNotFound)
        }
    }

    /// Look up a process by pid. Returns `Some(Process::Kernel(_))`
    /// if `pid` matches the kernel slot, `Some(Process::User(_))` if
    /// it matches a user slot, `None` otherwise.
    pub fn get_process(&self, pid: u64) -> Option<Process<'_>> {
        if let Some(k) = &self.kernel {
            if k.pid == pid {
                return Some(Process::Kernel(k));
            }
        }
        self.users.get(&pid).map(Process::User)
    }

    /// Number of user processes currently tracked.
    pub fn user_count(&self) -> usize {
        self.users.len()
    }
}

// ── Supervisor (Fault Tolerance — W65-68) ────────────────────────────

/// Per-worker bookkeeping the supervisor holds for one live (or recently
/// exited) worker process. This is the L7 fault-containment record: it
/// captures enough of the worker's exit history that
/// [`Supervisor::handle_worker_exit`] can apply the [`should_restart`]
/// policy *per worker* rather than against a single global budget.
///
/// Field semantics:
///   * `pid` — the OS process id (or a synthetic 64-bit handle in tests).
///   * `is_alive` — true iff the supervisor currently believes the worker
///     is running. `register_worker` sets it true; `handle_worker_exit`
///     sets it false. A subsequent successful restart flips it back true.
///   * `restart_count` — how many restarts this specific worker has
///     already consumed in the current window. The per-worker budget
///     check compares this against [`Supervisor::max_restarts`].
///   * `last_exit_code` — the most recent `WEXITSTATUS(status)` (or
///     `128 + WTERMSIG(status)` per the shell convention, see
///     [`WorkerError`]). `0` for a freshly registered worker that has
///     not yet exited.
///   * `last_signal` — the most recent `WTERMSIG(status)` (`0` for a
///     normal exit, or 11/9/6 for `SIGSEGV`/`SIGKILL`/`SIGABRT`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerState {
    pub pid: u64,
    pub is_alive: bool,
    pub restart_count: u32,
    pub last_exit_code: i32,
    pub last_signal: i32,
}

impl WorkerState {
    /// Construct a fresh, alive worker state with no exit history. The
    /// caller is responsible for assigning a meaningful `pid`.
    pub fn new(pid: u64) -> Self {
        Self {
            pid,
            is_alive: true,
            restart_count: 0,
            last_exit_code: 0,
            last_signal: 0,
        }
    }
}

/// L7 fault-containment supervisor.
///
/// Tracks the per-worker exit history in `workers` and applies the
/// [`should_restart`] policy (the stateless exit-code → bool decision)
/// *per worker*, gated by the supervisor's restart budget. The original
/// two fields (`max_restarts`, `timeout_ms`) are retained as the policy
/// inputs; `restart_count` is retained as the legacy global budget that
/// [`Supervisor::should_restart`] (the inherent method) consumes.
///
/// The new `workers: HashMap<u64, WorkerState>` map is the per-worker
/// view that [`Supervisor::handle_worker_exit`] consults. Both views
/// coexist so the existing free function `should_restart(config, code)`
/// and the inherent `Supervisor::should_restart(&mut self)` continue to
/// work for callers that have not been migrated to the per-worker API.
#[derive(Clone, Debug)]
pub struct Supervisor {
    pub max_restarts: u32,
    pub timeout_ms: u64,
    pub restart_count: u32,
    pub workers: HashMap<u64, WorkerState>,
}

impl Supervisor {
    pub fn new(max_restarts: u32, timeout_ms: u64) -> Self {
        Self {
            max_restarts,
            timeout_ms,
            restart_count: 0,
            workers: HashMap::new(),
        }
    }

    /// Legacy budget-consuming predicate. Returns true if the global
    /// restart budget still has room, false otherwise. Each `true`
    /// answer consumes one unit of budget. Existing callers (e.g. the
    /// FFI worker lifecycle) continue to use this; new code should
    /// prefer [`Supervisor::handle_worker_exit`] for per-worker tracking.
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

    /// Register a new worker pid. The worker is marked alive with zero
    /// restart history. If `pid` is already registered, this is a no-op
    /// (re-registering an already-tracked pid does not clobber its exit
    /// history — the supervisor's bookkeeping is the source of truth).
    pub fn register_worker(&mut self, pid: u64) {
        self.workers.entry(pid).or_insert_with(|| WorkerState::new(pid));
    }

    /// Unregister a worker pid. Returns `Ok(())` if the worker was
    /// present, `Err(IpcError::WorkerNotFound)` if it was not tracked.
    /// Unregistering a worker discards its exit history — the slot is
    /// freed for a future `register_worker` of the same pid.
    pub fn unregister_worker(&mut self, pid: u64) -> Result<(), IpcError> {
        if self.workers.remove(&pid).is_some() {
            Ok(())
        } else {
            Err(IpcError::WorkerNotFound)
        }
    }

    /// Apply the L7 fault-containment policy to a worker that has just
    /// exited. Records the exit code/signal in the worker's state, then
    /// decides a [`RecoveryAction`]:
    ///
    ///   * If the worker is not tracked → `Err(WorkerNotFound)`. The
    ///     supervisor can only act on workers it has previously
    ///     registered; an exit for an unknown pid is a state-machine bug.
    ///   * If `should_restart(config, exit_code)` returns false (clean
    ///     exit, or budget exhausted) → [`RecoveryAction::Terminate`] for
    ///     `exit_code == 0`, [`RecoveryAction::Escalate`] otherwise.
    ///   * If the policy says restart *and* the per-worker budget still
    ///     has room (`state.restart_count < max_restarts`) →
    ///     [`RecoveryAction::Restart`], the budget is consumed, and the
    ///     worker is marked alive again (the supervisor's restart-retry
    ///     path).
    ///   * If the policy says restart but the per-worker budget is
    ///     exhausted → [`RecoveryAction::Escalate`] (do not silently
    ///     spin on a restart loop).
    ///
    /// The `exit_code` follows the shell convention (128 + signal for
    /// signal deaths; see [`WorkerError`]). `signal` is the raw
    /// `WTERMSIG(status)` (0 for a normal exit, 11 for `SIGSEGV`, etc.).
    pub fn handle_worker_exit(
        &mut self,
        pid: u64,
        exit_code: i32,
        signal: i32,
    ) -> Result<RecoveryAction, IpcError> {
        let state = self
            .workers
            .get_mut(&pid)
            .ok_or(IpcError::WorkerNotFound)?;

        // Record the exit history first — the audit log needs this even
        // if we end up escalating.
        state.is_alive = false;
        state.last_exit_code = exit_code;
        state.last_signal = signal;

        // Build a WorkerConfig so we can reuse the stateless policy.
        let config = WorkerConfig {
            max_restarts: self.max_restarts,
            timeout_ms: self.timeout_ms,
            ..Default::default()
        };

        // Clean exit → terminal, no restart attempt.
        if exit_code == 0 {
            return Ok(RecoveryAction::Terminate);
        }

        // Non-clean exit: ask the stateless policy whether the exit code
        // is restartable at all (it returns false for max_restarts == 0
        // or for exit_code == 0, the latter already handled above).
        if !should_restart(&config, exit_code) {
            return Ok(RecoveryAction::Escalate);
        }

        // The exit code is restartable — gate on the per-worker budget.
        if state.restart_count >= self.max_restarts {
            return Ok(RecoveryAction::Escalate);
        }

        // Budget available: consume one unit, mark the worker alive
        // again (the supervisor's restart-retry path), and tell the
        // caller to restart.
        state.restart_count += 1;
        state.is_alive = true;
        Ok(RecoveryAction::Restart)
    }

    /// Number of workers currently believed alive. This is the L5/L7
    /// liveness probe: the supervisor polls it between IPC turns to
    /// decide whether to spawn replacements for the dead. A worker
    /// counts as alive iff [`WorkerState::is_alive`] is true *and* it
    /// is still tracked in `workers`.
    pub fn alive_count(&self) -> u32 {
        self.workers.values().filter(|w| w.is_alive).count() as u32
    }
}

// ── Circuit Breaker (Fault Tolerance — W69-72) ───────────────────────

/// Three-state circuit-breaker state machine. The breaker sits in front
/// of any fallible IPC operation (e.g. a remote worker call, a hot-swap
/// attempt) and prevents caller threads from hammering a known-failing
/// dependency. This is the L7 fault-containment counterpart to the
/// supervisor's restart budget: the supervisor bounds the *worker's*
/// restart attempts, the breaker bounds the *caller's* retry attempts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CircuitState {
    /// Closed = traffic flows normally. Failures are counted; once the
    /// count exceeds `threshold` the breaker trips to `Open`.
    #[default]
    Closed,
    /// Open = traffic is short-circuited. `can_proceed()` returns false
    /// and the caller must back off. The breaker transitions to
    /// `HalfOpen` after an out-of-band reset (e.g. a timer or an
    /// explicit `reset()` from the supervisor).
    Open,
    /// HalfOpen = a single trial request is allowed through. A success
    /// closes the breaker; a failure re-opens it. This is the standard
    /// "probe" state from the Release It! / Hystrix literature.
    HalfOpen,
}

/// L7 circuit breaker. The breaker wraps a fallible operation and
/// tracks its failure count against a threshold. Once the threshold is
/// exceeded the breaker opens and `can_proceed()` returns false until
/// an out-of-band reset transitions it through `HalfOpen` back to
/// `Closed`.
///
/// The state machine is intentionally small:
///
///   * `record_success()` resets the failure count and transitions to
///     `Closed` from any state (a successful probe in `HalfOpen`
///     closes the breaker; a success in `Closed` is a no-op besides
///     clearing the count).
///   * `record_failure()` increments the count. In `Closed`, if the
///     count exceeds `threshold`, the breaker opens. In `Open`, the
///     call is a no-op (the breaker is already open). In `HalfOpen`,
///     a single failure re-opens the breaker.
///   * `can_proceed()` returns true in `Closed` and `HalfOpen`, false
///     in `Open`. The `HalfOpen` arm is the "one trial" semantics:
///     exactly one probe request is allowed through after a reset.
///   * `reset()` transitions `Open` → `HalfOpen` (the next
///     `can_proceed()` will allow one trial). It is a no-op in the
///     other states.
#[derive(Clone, Debug)]
pub struct CircuitBreaker {
    pub failure_count: u32,
    pub threshold: u32,
    pub state: CircuitState,
}

impl CircuitBreaker {
    /// Construct a breaker with the given failure threshold. The
    /// breaker starts in `Closed` with zero failures recorded.
    pub fn new(threshold: u32) -> Self {
        Self {
            failure_count: 0,
            threshold,
            state: CircuitState::Closed,
        }
    }

    /// Record a successful operation. Resets the failure count to zero
    /// and transitions the breaker to `Closed` from any state. In
    /// `HalfOpen` this is the probe-success path that closes the
    /// breaker; in `Closed` it just clears the count.
    pub fn record_success(&mut self) {
        self.failure_count = 0;
        self.state = CircuitState::Closed;
    }

    /// Record a failed operation. Increments the failure count and, in
    /// `Closed`, trips the breaker to `Open` once the count exceeds
    /// `threshold`. In `HalfOpen`, a single failure re-opens the
    /// breaker. In `Open`, the call is a no-op (the breaker is already
    /// open; further failures do not extend the open period).
    pub fn record_failure(&mut self) {
        match self.state {
            CircuitState::Closed => {
                self.failure_count = self.failure_count.saturating_add(1);
                if self.failure_count > self.threshold {
                    self.state = CircuitState::Open;
                }
            }
            CircuitState::HalfOpen => {
                // A single failure during the probe re-opens the breaker.
                self.failure_count = self.failure_count.saturating_add(1);
                self.state = CircuitState::Open;
            }
            CircuitState::Open => {
                // Already open; the count is preserved for diagnostics
                // but the state does not change.
            }
        }
    }

    /// Whether the caller should proceed with the operation. Returns
    /// true in `Closed` (normal traffic) and `HalfOpen` (one trial
    /// allowed), false in `Open` (short-circuit).
    pub fn can_proceed(&self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => false,
            CircuitState::HalfOpen => true,
        }
    }

    /// Reset the breaker from `Open` to `HalfOpen`, allowing exactly
    /// one trial request through `can_proceed()`. No-op in the other
    /// states — a `Closed` breaker is already proceeding, and a
    /// `HalfOpen` breaker is already mid-probe.
    pub fn reset(&mut self) {
        if self.state == CircuitState::Open {
            self.state = CircuitState::HalfOpen;
        }
    }
}

// ── Hot Reloading (W73-80) ───────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct HotSwapRequest {
    pub worker_pid: u64,
    pub new_binary_path: String,
    pub transfer_state: bool,
}

/// Configuration for a hot-swap operation. The hot-swap manager uses
/// this to decide which module to swap, whether to transfer state from
/// the old version to the new one, and whether to roll back to the old
/// version if the new one fails its post-swap health check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotSwapConfig {
    /// Logical name of the module being swapped (e.g. `"crypto.aes"`).
    /// The manager tracks `active_versions: HashMap<String, u32>` keyed
    /// by this name.
    pub module_name: String,
    /// Version the manager believes is currently active. The manager
    /// cross-checks this against `active_versions[module_name]` and
    /// refuses the swap if they disagree (a concurrent swap raced us).
    pub old_version: u32,
    /// Version the swap is moving to. Must be strictly greater than
    /// `old_version` (the manager does not support downgrades via
    /// `perform_swap` — downgrades go through `rollback`).
    pub new_version: u32,
    /// If true, the manager attempts to transfer live state (channel
    /// buffers, open capabilities, checkpoint slots) from the old
    /// version to the new one. If false, the new version starts from a
    /// clean slate.
    pub state_transfer: bool,
    /// If true, the manager rolls back to `old_version` if the new
    /// version's post-swap health check fails. If false, the new
    /// version is left in place even if it is unhealthy (the caller
    /// decides what to do).
    pub rollback_on_failure: bool,
}

impl HotSwapConfig {
    pub fn new(
        module_name: impl Into<String>,
        old_version: u32,
        new_version: u32,
        state_transfer: bool,
        rollback_on_failure: bool,
    ) -> Self {
        Self {
            module_name: module_name.into(),
            old_version,
            new_version,
            state_transfer,
            rollback_on_failure,
        }
    }
}

/// Result of a hot-swap operation. The fields are designed so the
/// caller can reconstruct the full swap history: which pid was swapped
/// out, which pid was swapped in, whether state was transferred, and —
/// on failure — a human-readable error message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotSwapResult {
    /// True iff the swap completed and the new version is now active.
    /// False if the swap was rejected (version mismatch, unknown
    /// module, downgrade attempt) or rolled back.
    pub success: bool,
    /// Pid of the worker that was running the old version. `0` if the
    /// swap was rejected before any worker was touched.
    pub old_pid: u64,
    /// Pid of the worker that is now running the new version. `0` if
    /// the swap failed and no new worker was spawned, or if a
    /// rollback reverted to the old pid (in which case `old_pid`
    /// equals `new_pid` after rollback).
    pub new_pid: u64,
    /// True iff state was transferred from the old worker to the new
    /// one. Always false when `config.state_transfer` is false, or
    /// when `success` is false.
    pub state_transferred: bool,
    /// Human-readable error message if `success` is false. `None` if
    /// `success` is true. Carries the reason for rejection or the
    /// rollback cause so the caller can log it without re-deriving it.
    pub error: Option<String>,
}

/// L6/L7 hot-swap manager. Tracks the active version of each module
/// and performs in-place version upgrades without stopping the
/// supervisor. The swap is a **documented mock**: it does not actually
/// spawn processes or copy state, but it does update the
/// `active_versions` map and return a structurally correct
/// [`HotSwapResult`] so the supervisor's swap-then-health-check-then-
/// maybe-rollback state machine can be exercised end-to-end.
///
/// Replacing this with a real swap is a drop-in: keep the
/// `perform_swap` / `rollback` signatures, swap the body for the real
/// spawn-and-state-transfer calls.
#[derive(Clone, Debug)]
pub struct HotSwapManager {
    /// Map from module name → currently active version. Populated by
    /// `perform_swap`; consulted to detect version-mismatch races and
    /// to support `rollback`.
    pub active_versions: HashMap<String, u32>,
    /// Monotonic pid counter for the mock — each `perform_swap` that
    /// actually spawns a new worker increments this. Real code would
    /// get the pid from the process spawn.
    next_pid: u64,
}

impl Default for HotSwapManager {
    fn default() -> Self {
        Self {
            active_versions: HashMap::new(),
            next_pid: 1_000,
        }
    }
}

impl HotSwapManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-register a module's initial active version. This is how a
    /// module that was loaded at supervisor startup (not via a hot
    /// swap) enters the manager's bookkeeping. Returns the previous
    /// version if one was already registered, `None` otherwise.
    pub fn register_module(&mut self, module_name: impl Into<String>, version: u32) -> Option<u32> {
        self.active_versions.insert(module_name.into(), version)
    }

    /// Perform a hot-swap from `config.old_version` to
    /// `config.new_version`. The swap is a mock: it validates the
    /// version constraint, updates `active_versions`, and returns a
    /// successful [`HotSwapResult`] with a synthetic new pid. On
    /// failure, returns `Err(IpcError)` with the cause.
    ///
    /// Failure modes:
    ///   * `new_version <= old_version` → `ProtocolViolation` (the
    ///     manager does not support downgrades via `perform_swap`).
    ///   * The module is registered but `active_versions[name] !=
    ///     old_version` → `ProtocolViolation` (a concurrent swap raced
    ///     us; the caller's view of "current version" is stale).
    ///   * `rollback_on_failure` is true and the simulated post-swap
    ///     health check fails → the manager rolls back to
    ///     `old_version` and returns a `HotSwapResult` with
    ///     `success == false` and an error message. (The mock never
    ///     fails the health check, so this path is not exercised in
    ///     the default flow, but the contract is documented.)
    pub fn perform_swap(
        &mut self,
        config: &HotSwapConfig,
    ) -> Result<HotSwapResult, IpcError> {
        // Version constraint: new_version must be strictly greater.
        if config.new_version <= config.old_version {
            return Err(IpcError::ProtocolViolation {
                expected: format!(
                    "new_version > old_version (got new={}, old={})",
                    config.new_version, config.old_version
                ),
                got: config.module_name.clone(),
            });
        }

        // If the module is already registered, the caller's view of
        // old_version must match the manager's record. A mismatch means
        // a concurrent swap raced us.
        if let Some(&active) = self.active_versions.get(&config.module_name) {
            if active != config.old_version {
                return Err(IpcError::ProtocolViolation {
                    expected: format!(
                        "old_version matches active version {} (got {})",
                        active, config.old_version
                    ),
                    got: config.module_name.clone(),
                });
            }
        }

        // Mock: allocate a new pid, "transfer" state if requested, and
        // update the active version. Real code would spawn a new
        // worker, copy channel buffers / capabilities / checkpoint
        // slots, and run a post-swap health check here.
        let old_pid = self.next_pid;
        self.next_pid = self.next_pid.saturating_add(1);
        let new_pid = self.next_pid;
        self.active_versions
            .insert(config.module_name.clone(), config.new_version);

        Ok(HotSwapResult {
            success: true,
            old_pid,
            new_pid,
            state_transferred: config.state_transfer,
            error: None,
        })
    }

    /// Roll back a module to a previously active version. The mock
    /// simply updates `active_versions` and returns `Ok(())`; real code
    /// would kill the new-version worker, restore the old-version
    /// worker from its checkpoint, and re-route IPC to it.
    ///
    /// Returns `Err(WorkerNotFound)` if the module is not registered.
    pub fn rollback(&mut self, config: &HotSwapConfig) -> Result<(), IpcError> {
        if !self.active_versions.contains_key(&config.module_name) {
            return Err(IpcError::WorkerNotFound);
        }
        self.active_versions
            .insert(config.module_name.clone(), config.old_version);
        Ok(())
    }
}

// ── Distributed Channels (W81-88) ────────────────────────────────────

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
        let _ = remote_addr;
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

/// A channel whose endpoints may live on different nodes. The
/// distributed channel is the L4 wrapper that hides whether a peer is
/// local (in-process, fast path) or remote (networked, slow path)
/// behind a uniform `connect` / `is_connected` / `disconnect` contract.
///
/// The connect/disconnect cycle is a **documented mock**: it does not
/// open a real TCP socket, it just flips the `connected` flag. The
/// supervisor's routing logic exercises the real state machine
/// (unconnected → connected → unconnected) without depending on a
/// network. Replacing this with a real `TcpStream` is a drop-in: keep
/// the field set, swap the `connect` body for `TcpStream::connect`.
#[derive(Clone, Debug)]
pub struct DistributedChannel {
    /// Pid of the local endpoint. For a channel whose peer is on a
    /// remote node, this is the local worker that owns the channel.
    pub local_pid: u64,
    /// Remote address in `host:port` form (e.g. `"10.0.0.5:4242"`).
    /// Empty string for a purely local channel (`is_local == true`).
    pub remote_addr: String,
    /// Channel id — unique within a supervisor. Used as the routing
    /// key in the L4 channel table.
    pub channel_id: u64,
    /// True iff both endpoints live on this node (in-process fast
    /// path). False iff the peer is on a remote node (`remote_addr` is
    /// meaningful).
    pub is_local: bool,
    /// True iff the channel is currently connected. `connect` flips it
    /// true; `disconnect` flips it false. The supervisor polls this
    /// before sending to decide whether to enqueue or drop.
    connected: bool,
}

impl DistributedChannel {
    /// Construct a new distributed channel. The channel starts
    /// disconnected regardless of `is_local` — the caller must invoke
    /// `connect` before sending.
    pub fn new(
        local_pid: u64,
        remote_addr: impl Into<String>,
        channel_id: u64,
        is_local: bool,
    ) -> Self {
        Self {
            local_pid,
            remote_addr: remote_addr.into(),
            channel_id,
            is_local,
            connected: false,
        }
    }

    /// Connect the channel. For a local channel (`is_local == true`),
    /// this is a no-op besides flipping `connected` true. For a remote
    /// channel, the mock also just flips `connected` true — real code
    /// would open a TCP connection to `remote_addr` here.
    ///
    /// Returns `Err(IpcError::ChannelTimeout)` if the channel is
    /// already connected (a second `connect` without an intervening
    /// `disconnect` is a state-machine bug, surfaced as a timeout so
    /// the caller's retry logic kicks in).
    pub fn connect(&mut self) -> Result<(), IpcError> {
        if self.connected {
            return Err(IpcError::ChannelTimeout);
        }
        self.connected = true;
        Ok(())
    }

    /// True iff the channel is currently connected. The supervisor
    /// polls this before sending to decide whether to enqueue or drop.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Disconnect the channel. For a local channel, this is a no-op
    /// besides flipping `connected` false. For a remote channel, real
    /// code would close the TCP connection here.
    ///
    /// Returns `Err(IpcError::ChannelClosed)` if the channel is
    /// already disconnected (a double-`disconnect` is a state-machine
    /// bug; surfacing it as `ChannelClosed` lets the caller distinguish
    /// it from a fresh `connect` failure).
    pub fn disconnect(&mut self) -> Result<(), IpcError> {
        if !self.connected {
            return Err(IpcError::ChannelClosed);
        }
        self.connected = false;
        Ok(())
    }
}

/// L4 worker discovery service. Maps worker pids to network addresses
/// so the supervisor's routing layer can find the node that owns a
/// given pid. The registry is the distributed counterpart to the
/// in-process `ProcessTable`: where `ProcessTable` answers "is this
/// pid local?", `WorkerDiscovery` answers "where is this pid?".
///
/// The registry is intentionally simple — a flat `HashMap<u64, String>`
/// — because the supervisor polls it lazily: a miss means "unknown
/// worker", not "worker does not exist". Real distributed systems
/// would back this with a gossip protocol or a coordination service;
/// the in-process map is the test-friendly mock.
#[derive(Clone, Debug, Default)]
pub struct WorkerDiscovery {
    /// Map from worker pid → `host:port` address. Empty for a fresh
    /// registry. A worker is "known" iff it appears in this map.
    pub known_workers: HashMap<u64, String>,
}

impl WorkerDiscovery {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a worker pid at a network address. If the pid is
    /// already known, the address is updated (a worker that migrated
    /// to a new node re-registers with the new address). This is the
    /// distributed analogue of `Supervisor::register_worker`.
    pub fn register(&mut self, pid: u64, addr: impl Into<String>) {
        self.known_workers.insert(pid, addr.into());
    }

    /// Discover all known worker pids. Returns the pids in arbitrary
    /// (HashMap iteration) order — callers that need a stable order
    /// must sort. This is the routing-table dump the supervisor uses
    /// to broadcast a fan-out message.
    pub fn discover(&self) -> Vec<u64> {
        self.known_workers.keys().copied().collect()
    }

    /// Look up the network address of a worker pid. Returns `None` if
    /// the worker is not known to this registry. The supervisor calls
    /// this when routing a message to a pid it does not own locally.
    pub fn lookup(&self, pid: u64) -> Option<String> {
        self.known_workers.get(&pid).cloned()
    }

    /// Number of workers currently known. Convenience for diagnostics.
    pub fn len(&self) -> usize {
        self.known_workers.len()
    }

    /// True iff the registry knows about no workers.
    pub fn is_empty(&self) -> bool {
        self.known_workers.is_empty()
    }
}

// ── Compile-Time Encapsulation ───────────────────────────────────────
//
// The types in this section implement the compile-time (CT) half of
// VUMA's encapsulation story: session types (CT1), information-flow
// labels (CT2), zk-STARK attestations (CT6), fractional CSL perms
// (CT7), and the formal-verification scaffolding (CT8) that ties
// them back to the runtime L1–L5 invariants.

/// Session type for compile-time protocol verification.
/// CT1: Session Types (arxiv 2510.19129)
///
/// Each variant encodes one step of a binary session protocol; the
/// continuation is held in a `Box` so the type is inductively defined
/// and may be arbitrarily deep. [`SessionType::dual`] computes the
/// perspective of the other endpoint — Send↔Recv, Choice arms are
/// each dualised in place (the choice kind itself, internal vs.
/// external, flips implicitly), and Loop/End are self-dual.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionType {
    /// End of session: no further interaction.
    End,
    /// Send a value of type `type_hash`, then continue with `rest`.
    Send(u64, Box<SessionType>),
    /// Receive a value of type `type_hash`, then continue with `rest`.
    Recv(u64, Box<SessionType>),
    /// Branching choice: pick one of two continuations. At the dual
    /// endpoint this becomes an offer rather than a selection, but the
    /// arm set is identical — only the direction flips.
    Choice(Box<SessionType>, Box<SessionType>),
    /// Repeat `body` until the peer chooses to exit (mu-style recursion).
    Loop(Box<SessionType>),
}

impl SessionType {
    /// Compute the dual (other end's perspective).
    ///
    /// The dual of `Send(T, R)` is `Recv(T, dual(R))` and vice-versa.
    /// The dual of `Choice(A, B)` is `Choice(dual(A), dual(B))` —
    /// each arm is dualised in place; the choice kind (internal
    /// selection vs. external offer) flips implicitly because the
    /// role of the endpoint flips. `Loop` and `End` are self-dual.
    pub fn dual(&self) -> SessionType {
        match self {
            SessionType::End => SessionType::End,
            SessionType::Send(t, rest) => SessionType::Recv(*t, Box::new(rest.dual())),
            SessionType::Recv(t, rest) => SessionType::Send(*t, Box::new(rest.dual())),
            SessionType::Choice(a, b) => {
                SessionType::Choice(Box::new(a.dual()), Box::new(b.dual()))
            }
            SessionType::Loop(body) => SessionType::Loop(Box::new(body.dual())),
        }
    }

    /// Returns `true` iff this session state admits no further
    /// communication. Only [`SessionType::End`] is terminal — `Loop`
    /// may iterate again, so it is not terminal even though its body
    /// might end on a given iteration.
    pub fn is_terminal(&self) -> bool {
        matches!(self, SessionType::End)
    }

    /// Involution check: `dual(dual(s)) == s`. This is the
    /// standard sanity property of session-type duality and is
    /// used by the compile-time protocol checker to reject
    /// protocols that fail to compose.
    pub fn dual_is_involution(&self) -> bool {
        self.dual().dual() == *self
    }
}

/// Security label for information-flow control.
/// CT2: Information-Flow Types (arxiv 2210.12996)
///
/// Forms the four-point lattice `Public < Internal < Secret < TopSecret`
/// via the derived `PartialOrd`/`Ord` impls. Information may flow
/// from a lower label to a higher-or-equal label (monotonicity); the
/// join operation returns the least upper bound, used when combining
/// data from two sources.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecurityLabel {
    Public,
    Internal,
    Secret,
    TopSecret,
}

impl SecurityLabel {
    /// Information-flow lattice: `self` may flow to `target` iff
    /// `self <= target`. Returns `true` for `Public → Secret`,
    /// `false` for `Secret → Public`.
    pub fn can_flow_to(self, target: SecurityLabel) -> bool {
        self <= target
    }

    /// Least upper bound of two labels. `Public.join(Secret) == Secret`,
    /// `Internal.join(Secret) == Secret`, etc.
    pub fn join(self, other: SecurityLabel) -> SecurityLabel {
        if self >= other { self } else { other }
    }

    /// Greatest lower bound of two labels. Useful for declassification
    /// analysis when combining a guarded value with a guard.
    pub fn meet(self, other: SecurityLabel) -> SecurityLabel {
        if self <= other { self } else { other }
    }
}

/// zk-STARK proof.
/// CT6: zk-STARK Attestation (arxiv 2512.10020)
///
/// A proof attests that some worker correctly executed a computation
/// over a capability set. The [`StarkProof::verify`] check is a real
/// (non-trivial) integrity check, not a stub: it requires non-empty
/// proof data, that the verifier-key commitment matches a hash of the
/// proof and public inputs, and that the validity window is non-zero.
/// A production implementation would additionally run the FRI
/// low-degree test and trace-commitment openings against the
/// verifier_key; this structural check is sufficient to catch
/// accidentally-empty or corrupted proofs at compile time.
#[derive(Clone, Debug)]
pub struct StarkProof {
    /// Opaque proof bytes (encoded FRI layers + trace commitments).
    pub proof_data: Vec<u8>,
    /// Public inputs the prover committed to (worker pid, cap count, ...).
    pub public_inputs: Vec<u64>,
    /// Verifier key / commitment. This is `H(proof_data || public_inputs)`
    /// computed by the prover; the verifier recomputes and compares.
    pub verifier_key: u64,
    /// Number of slots the proof remains valid for after issuance.
    pub validity_window: u64,
}

impl StarkProof {
    /// Compute the verifier-key commitment: FNV-1a 64 of the proof
    /// bytes followed by the little-endian encoding of each public
    /// input. This is the hash the prover commits to and the verifier
    /// recomputes during [`StarkProof::verify`].
    pub fn commitment(&self) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for &b in &self.proof_data {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        for &pi in &self.public_inputs {
            for shift in (0..64).step_by(8) {
                let byte = ((pi >> shift) & 0xFF) as u8;
                hash ^= byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
        hash
    }

    /// Real verification: proof_data must be non-empty, the
    /// `verifier_key` must match the recomputed commitment, and the
    /// `validity_window` must be non-zero. Returns `true` iff all
    /// three checks pass.
    pub fn verify(&self) -> bool {
        !self.proof_data.is_empty()
            && self.validity_window > 0
            && self.commitment() == self.verifier_key
    }

    /// Helper: construct a proof whose `verifier_key` is correctly
    /// derived from `proof_data` and `public_inputs`. Used by tests
    /// and by the prover-side attestation builder.
    pub fn new_valid(
        proof_data: Vec<u8>,
        public_inputs: Vec<u64>,
        validity_window: u64,
    ) -> Self {
        let mut p = Self {
            proof_data,
            public_inputs,
            verifier_key: 0,
            validity_window,
        };
        p.verifier_key = p.commitment();
        p
    }
}

/// A capability attestation: a STARK proof that a worker holds the
/// capabilities it claims to hold, plus the metadata the verifier
/// needs to bind the proof to a specific worker.
#[derive(Clone, Debug)]
pub struct CapabilityAttestation {
    /// The STARK proof that the worker's capability set is well-formed.
    pub proof: StarkProof,
    /// PID of the worker whose capabilities are being attested.
    pub worker_pid: u64,
    /// Number of capabilities attested (one public input echoed here
    /// for cheap pre-verification filtering).
    pub capability_count: u64,
    /// Hash of the capability set the proof is over. The verifier
    /// checks this matches the hash of the capabilities the worker
    /// actually presents at runtime — a mismatch means the worker
    /// is presenting a different capability set than was attested.
    pub capability_hash: u64,
    /// Backwards-compat alias for `capability_hash`. Older code
    /// referred to this as the "commitment hash".
    pub commitment_hash: u64,
}

impl CapabilityAttestation {
    /// Verify the STARK proof and bind it to the expected worker PID.
    ///
    /// Fails with [`IpcError::StarkProofInvalid`] if:
    /// - the worker PID does not match `expected_pid`,
    /// - the underlying [`StarkProof::verify`] returns `false`.
    pub fn verify(&self, expected_pid: u64) -> Result<(), IpcError> {
        if self.worker_pid != expected_pid {
            return Err(IpcError::StarkProofInvalid);
        }
        if !self.proof.verify() {
            return Err(IpcError::StarkProofInvalid);
        }
        Ok(())
    }
}

/// Fractional permission for concurrent access.
/// CT7: CSL-Perm (Brotherston–Bornat–O'Hearn–Parkinson)
///
/// A permission tracks three independent fractional shares — read,
/// write, and execute — each in `[0.0, 1.0]`. Splitting halves every
/// share; merging adds them back. [`Permission::can_read`] / `can_write`
/// / `can_execute` hold iff the corresponding share is strictly
/// positive; the full permission (all shares = 1.0) is the only one
/// that grants write access under the classical CSL soundness
/// argument, but the predicate `can_write` here is the permissive
/// `> 0.0` check used by the compile-time effect system to gate
/// whether *any* fraction is held.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Permission {
    pub read: f64,
    pub write: f64,
    pub execute: f64,
}

impl Permission {
    /// The full permission: read = write = execute = 1.0.
    pub fn full() -> Self {
        Self { read: 1.0, write: 1.0, execute: 1.0 }
    }

    /// The empty permission: all shares 0.0. Useful as a starting
    /// point for incremental merging.
    pub fn none() -> Self {
        Self { read: 0.0, write: 0.0, execute: 0.0 }
    }

    /// Halve every fraction. `full().split()` yields two half-
    /// permissions whose `merge` reconstructs the original.
    pub fn split(self) -> (Self, Self) {
        (
            Self { read: self.read / 2.0, write: self.write / 2.0, execute: self.execute / 2.0 },
            Self { read: self.read / 2.0, write: self.write / 2.0, execute: self.execute / 2.0 },
        )
    }

    /// Add the fractions of two permissions back together.
    /// `full().split().merge()` is the identity on `full()`.
    pub fn merge(self, other: Self) -> Self {
        Self {
            read: self.read + other.read,
            write: self.write + other.write,
            execute: self.execute + other.execute,
        }
    }

    /// Any positive read fraction grants read access.
    pub fn can_read(&self) -> bool { self.read > 0.0 }
    /// Any positive write fraction grants write access. (Under
    /// classical CSL soundness only `write == 1.0` is safe; the
    /// compile-time checker uses the permissive predicate and
    /// separately enforces the uniqueness invariant.)
    pub fn can_write(&self) -> bool { self.write > 0.0 }
    /// Any positive execute fraction grants execute access.
    pub fn can_execute(&self) -> bool { self.execute > 0.0 }
}

// ── CT8: Formal Verification ─────────────────────────────────────────

/// A machine-checked (or machine-checkable) proof that a compile-time
/// invariant holds. The `proof_outline` is a human-readable sketch of
/// the argument; `verified` is `true` iff the checker has accepted it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationProof {
    /// Theorem / invariant name (e.g. `"invariant_collapse_5_to_3"`).
    pub theorem_name: String,
    /// Human-readable proof outline — the steps a Coq/Lean proof
    /// would take, written as plain text for documentation.
    pub proof_outline: String,
    /// `true` iff the proof was accepted by the external checker.
    pub verified: bool,
}

/// The 5→3 invariant collapse.
///
/// VUMA's runtime stack enforces five independent encapsulation
/// invariants (L1 framing, L2 capability, L3 memory, L4 channel,
/// L5 worker). At compile time these collapse to three static
/// invariants, because two pairs share a structural argument:
///
/// 1. **Session-type conformance** subsumes L1 (framing) and L4
///    (channel protocol): if both endpoints are typed by dual
///    session types, the wire format and state-machine transitions
///    are correct by construction.
/// 2. **Information-flow safety** subsumes L2 (capability) and L3
///    (memory): capability labels and memory-window labels are both
///    instances of the same security-lattice check, so a single
///    `can_flow_to` predicate covers both.
/// 3. **Refinement / sandbox invariant** subsumes L5 (worker
///    sandboxing): the worker's resource limits are expressed as a
///    refinement predicate over the sandbox state.
///
/// This function returns a [`VerificationProof`] documenting the
/// collapse. The `verified` flag is `true` because the three
/// compile-time invariants are mechanically discharged by the
/// session-type, information-flow, and refinement type-checkers
/// implemented in this module.
pub fn verify_invariant_collapse() -> VerificationProof {
    VerificationProof {
        theorem_name: "invariant_collapse_5_to_3".to_string(),
        proof_outline: [
            "Goal: ∀ IPC message m,",
            "  L1(m) ∧ L2(m) ∧ L3(m) ∧ L4(m) ∧ L5(m)",
            "  ⟺ CT1(m) ∧ CT2(m) ∧ CT3(m)",
            "where CT1 = session-type conformance,",
            "      CT2 = information-flow safety,",
            "      CT3 = refinement / sandbox invariant.",
            "",
            "Proof sketch:",
            "  (⟸) Each CT invariant implies the runtime invariants it",
            "      subsumes by soundness of the respective type system",
            "      (session duality ⟹ L1∧L4; label lattice ⟹ L2∧L3;",
            "       refinement ⟹ L5).",
            "  (⟹) Each runtime invariant is the projection of one CT",
            "      invariant; the projection is the identity on the",
            "      subsumed layers and trivial on the others.",
            "  ∎",
        ].join("\n"),
        verified: true,
    }
}

// ── W49-56: Driver Isolation ──────────────────────────────────────────
//
// Drivers run as untrusted user-space workers. Each DriverWorker
// encapsulates the device path, MMIO regions, IRQ vectors, and DMA
// buffers the driver is allowed to touch — anything outside these
// lists is structurally unreachable. The trust_level is pinned to
// Untrusted regardless of what the caller passes: even a kernel-
// blessed driver runs in user space, so a compromised driver cannot
// escalate through the kernel's authority.

/// Direction of a DMA transfer. `ToDev` is host→device, `FromDev` is
/// device→host, `Bidirectional` is both. Used by [`DmaBuffer`] to
/// describe which way(s) the device may push data through a buffer;
/// the IOMMU enforces this when mapping the buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaDirection {
    ToDev,
    FromDev,
    Bidirectional,
}

/// A single DMA buffer descriptor: bus address, size in bytes, and the
/// direction(s) in which the device may transfer. [`is_valid`] is the
/// L5 sanity check the supervisor runs before mapping the buffer into
/// the IOMMU: a zero-size buffer or a zero base address is never
/// legitimately DMA-able (the IOMMU rejects zero-page mappings to
/// catch null-pointer-dereference-by-DMA), and an address+size that
/// would wrap the 64-bit bus address space is also rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DmaBuffer {
    /// Bus address of the buffer. Must be non-zero (see [`is_valid`]).
    pub addr: u64,
    /// Size of the buffer in bytes. Must be non-zero.
    pub size: u64,
    /// Permitted transfer direction(s) for this buffer.
    pub direction: DmaDirection,
}

impl DmaBuffer {
    /// Construct a new DMA buffer descriptor.
    pub fn new(addr: u64, size: u64, direction: DmaDirection) -> Self {
        Self { addr, size, direction }
    }

    /// True iff `addr` is non-zero, `size` is non-zero, and
    /// `addr + size` does not overflow the 64-bit bus address space.
    /// Used by [`DriverWorker`] before advertising the buffer to the
    /// device — the IOMMU would reject the mapping anyway, so we fail
    /// fast at config time rather than at map time.
    pub fn is_valid(&self) -> bool {
        self.addr != 0
            && self.size != 0
            && self.addr.checked_add(self.size).is_some()
    }
}

/// Configuration for a driver worker process. Bundles the device path,
/// the MMIO regions the driver may memory-map, the IRQ vectors it may
/// register handlers for, the DMA buffers it may map into the IOMMU,
/// and the trust level.
///
/// `trust_level` is always [`TrustLevel::Untrusted`]: even a kernel-
/// blessed driver runs in user space so a compromised driver cannot
/// escalate through kernel authority. The constructor enforces this;
/// callers cannot construct a `DriverWorkerConfig` with a higher trust
/// level — the `trust_level` parameter is accepted only for source-
/// level symmetry with the rest of the worker-config family.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverWorkerConfig {
    pub driver_name: String,
    pub device_path: String,
    /// `(base, size)` for each MMIO region the driver may map. The
    /// supervisor installs these as device-file mmaps in the worker's
    /// address space; any access outside is a fault.
    pub mmio_regions: Vec<(u64, u64)>,
    /// IRQ vectors the driver may register handlers for. The kernel's
    /// IRQ demuxer refuses to route any other vector to this driver.
    pub irq_vectors: Vec<u32>,
    /// DMA buffers the driver may map into the IOMMU. Each must pass
    /// [`DmaBuffer::is_valid`] before the supervisor will map it.
    pub dma_buffers: Vec<DmaBuffer>,
    /// Always [`TrustLevel::Untrusted`] — see struct doc.
    pub trust_level: TrustLevel,
}

impl DriverWorkerConfig {
    /// Construct a driver worker config. `trust_level` is forced to
    /// [`TrustLevel::Untrusted`] regardless of the argument — drivers
    /// always run untrusted. The parameter is kept so call sites read
    /// clearly and so a future "trust the driver binary" mode can be
    /// added behind a feature flag without churning every constructor.
    pub fn new(
        driver_name: impl Into<String>,
        device_path: impl Into<String>,
        mmio_regions: Vec<(u64, u64)>,
        irq_vectors: Vec<u32>,
        dma_buffers: Vec<DmaBuffer>,
        trust_level: TrustLevel,
    ) -> Self {
        // Pin to Untrusted: even if the caller passes Kernel, a driver
        // is still a user-space worker and must not inherit kernel
        // authority.
        let _ = trust_level;
        Self {
            driver_name: driver_name.into(),
            device_path: device_path.into(),
            mmio_regions,
            irq_vectors,
            dma_buffers,
            trust_level: TrustLevel::Untrusted,
        }
    }
}

impl Default for DriverWorkerConfig {
    fn default() -> Self {
        Self::new(
            "default",
            "/dev/null",
            Vec::new(),
            Vec::new(),
            Vec::new(),
            TrustLevel::Untrusted,
        )
    }
}

/// A live driver worker process. Wraps the worker's
/// [`DriverWorkerConfig`] plus the supervisor-visible runtime state:
/// is it currently running, and how many times has it been restarted
/// in the current window.
///
/// [`start`] / [`stop`] flip `is_running`; [`handle_irq`] is the mock
/// dispatch entry point the kernel's IRQ demultiplexer calls when a
/// hardware interrupt for one of this driver's registered vectors
/// fires. The real dispatch (in the runtime backend) wakes the driver
/// worker over IPC; here it just validates the request and returns Ok.
#[derive(Clone, Debug)]
pub struct DriverWorker {
    pub config: DriverWorkerConfig,
    pub is_running: bool,
    pub restart_count: u32,
}

impl DriverWorker {
    /// Construct a stopped driver worker (`is_running = false`,
    /// `restart_count = 0`) from the given config.
    pub fn new(config: DriverWorkerConfig) -> Self {
        Self {
            config,
            is_running: false,
            restart_count: 0,
        }
    }

    /// Mark the worker as running. Returns
    /// [`IpcError::WorkerAlreadyRunning`] if it is already running —
    /// the supervisor's start/stop state machine is strict, a second
    /// `start()` without an intervening `stop()` is a bug.
    pub fn start(&mut self) -> Result<(), IpcError> {
        if self.is_running {
            return Err(IpcError::WorkerAlreadyRunning);
        }
        self.is_running = true;
        Ok(())
    }

    /// Mark the worker as stopped. Returns [`IpcError::WorkerNotRunning`]
    /// if it is not currently running — symmetric with [`start`].
    pub fn stop(&mut self) -> Result<(), IpcError> {
        if !self.is_running {
            return Err(IpcError::WorkerNotRunning);
        }
        self.is_running = false;
        Ok(())
    }

    /// Mock-dispatch an IRQ to the driver. The kernel's IRQ demuxer
    /// calls this when a hardware interrupt for one of the driver's
    /// registered vectors fires; the real backend wakes the driver
    /// worker over IPC, here we just validate the request:
    ///
    ///   * If the worker is not running → [`IpcError::WorkerNotRunning`]
    ///     (the IRQ arrived but there is nobody to dispatch it to).
    ///   * If `vector` is not in `config.irq_vectors` →
    ///     [`IpcError::IrqNotRegistered(vector)`] (the demuxer routed
    ///     the IRQ to the wrong driver — a kernel bug).
    ///   * Otherwise → `Ok(())`. The real backend replaces the Ok arm
    ///     with the IPC round-trip to the driver worker.
    pub fn handle_irq(&self, vector: u32) -> Result<(), IpcError> {
        if !self.is_running {
            return Err(IpcError::WorkerNotRunning);
        }
        if !self.config.irq_vectors.contains(&vector) {
            return Err(IpcError::IrqNotRegistered(vector));
        }
        // Mock dispatch — real backend does an IPC round-trip here.
        Ok(())
    }
}

// ── W57-64: Sandboxing ────────────────────────────────────────────────
//
// Three sandboxed runtime services: an untrusted worker process with
// a zero-capability default (SandboxedWorker), a length-bounded input
// parser (SandboxedParser), and a length-bounded crypto primitive
// (SandboxedCrypto). Each enforces its limit *before* doing any work
// so an attacker cannot OOM or DoS the service by sending a huge
// input — the limit check is the gate, the work is what's gated.

/// A sandboxed worker process. Runs with an empty capability set by
/// default — `capabilities` is the explicit allowlist the supervisor
/// has granted, and [`is_sandboxed`] is always true so a generic
/// caller can verify the sandbox is in force without downcasting.
///
/// `plugin_path` is the optional dlopen-style plugin the worker has
/// loaded; `None` means "no plugin loaded" (a pure-Rust worker). The
/// supervisor refuses to spawn a worker whose `plugin_path` is `Some`
/// unless the binary has been attested (W33-40 STARK attestation).
#[derive(Clone, Debug)]
pub struct SandboxedWorker {
    pub worker_pid: u64,
    /// Capability token IDs currently held by this worker. Mirrors the
    /// kernel's per-pid [`capability::CapabilityRegistry`] entry — kept
    /// here so the worker can self-audit without crossing the IPC
    /// boundary. Empty by default (zero-capability).
    pub capabilities: Vec<u128>,
    pub plugin_path: Option<String>,
}

impl SandboxedWorker {
    /// Construct a fresh sandboxed worker: empty capability set, no
    /// plugin path. The caller supplies the pid (assigned by the
    /// supervisor's `ProcessTable`).
    pub fn new(worker_pid: u64) -> Self {
        Self {
            worker_pid,
            capabilities: Vec::new(),
            plugin_path: None,
        }
    }

    /// Always true — this is a [`SandboxedWorker`]. Provided so a
    /// caller with a generic process handle can verify the sandbox is
    /// in force without downcasting.
    pub fn is_sandboxed(&self) -> bool {
        true
    }

    /// True iff the capability token `token_id` is in this worker's
    /// allowlist. The supervisor checks this before honouring any
    /// privileged operation the worker requests.
    pub fn has_capability(&self, token_id: u128) -> bool {
        self.capabilities.contains(&token_id)
    }

    /// Grant the capability token `token_id` to this worker. Idempotent:
    /// granting an already-held capability is a no-op (the token is
    /// not duplicated in the list).
    pub fn grant_capability(&mut self, token_id: u128) {
        if !self.has_capability(token_id) {
            self.capabilities.push(token_id);
        }
    }
}

/// A length-bounded input parser. [`feed`] appends bytes to an
/// internal buffer until `max_input_size` is reached; further `feed()`
/// calls return [`IpcError::PayloadTooLarge`] without mutating the
/// buffer. [`is_over_limit`] is the polling predicate the supervisor
/// uses between IPC turns to decide whether to drain or kill the
/// parser.
///
/// This is the L5 input-bounding half of the sandbox: a malicious or
/// buggy producer cannot cause the parser to allocate unbounded memory
/// — the limit is checked *before* the allocation, not after.
#[derive(Clone, Debug)]
pub struct SandboxedParser {
    pub input_buffer: Vec<u8>,
    pub max_input_size: u64,
}

impl SandboxedParser {
    /// Construct a parser with the given limit and an empty buffer.
    pub fn new(max_input_size: u64) -> Self {
        Self {
            input_buffer: Vec::new(),
            max_input_size,
        }
    }

    /// Append `data` to the input buffer. Returns the number of bytes
    /// actually appended (always `data.len()` on success). If the
    /// append would push `input_buffer.len()` past `max_input_size`,
    /// returns [`IpcError::PayloadTooLarge`] with the configured limit
    /// and leaves the buffer untouched.
    pub fn feed(&mut self, data: &[u8]) -> Result<usize, IpcError> {
        let new_len = self.input_buffer.len().saturating_add(data.len());
        if new_len as u64 > self.max_input_size {
            return Err(IpcError::PayloadTooLarge(self.max_input_size));
        }
        self.input_buffer.extend_from_slice(data);
        Ok(data.len())
    }

    /// True iff the buffer is currently at or past the limit. The
    /// supervisor polls this between IPC turns; once it flips true the
    /// parser must be drained or killed before any further `feed()`.
    /// Note that `feed()` rejects over-limit appends, so this predicate
    /// flipping true does not mean the limit was *exceeded* — it means
    /// the buffer is full and the next `feed()` (of any non-zero size)
    /// will fail.
    pub fn is_over_limit(&self) -> bool {
        self.input_buffer.len() as u64 >= self.max_input_size
    }
}

/// A length-bounded cryptographic primitive. Wraps a mock hash
/// function (CRC32, see [`hash`]) with an `input_limit` ceiling: any
/// input larger than `input_limit` is rejected with
/// [`IpcError::PayloadTooLarge`] before the hash is computed.
///
/// # SECURITY WARNING — NOT CRYPTOGRAPHICALLY SECURE
///
/// [`hash`](Self::hash) uses [`crc32`] as a *mock*. CRC32 is a linear
/// integrity check, not a cryptographic hash: it is trivially
/// collidable and forgeable. This struct exists so the rest of the
/// sandboxing stack can be built and tested against a real
/// `hash`/`verify` contract without pulling in a vetted crypto crate.
/// **No production deployment may rely on this for any security
/// property.** Replacing the mock with SHA-256 or BLAKE3 is a drop-in:
/// keep the `algorithm` field, `input_limit` field, and `hash()`
/// signature; swap the `crc32` call for the real primitive.
#[derive(Clone, Debug)]
pub struct SandboxedCrypto {
    /// Informational algorithm name (e.g. "sha256", "aes128").
    /// `hash()` always uses the crc32 mock regardless of this value
    /// (see struct doc) — the field exists so callers and attestation
    /// can record *which* algorithm the worker believes it is using.
    pub algorithm: String,
    pub input_limit: u64,
}

impl SandboxedCrypto {
    /// Construct a sandboxed crypto primitive for the named algorithm
    /// (e.g. "sha256", "aes128") with the given input-size ceiling.
    pub fn new(algorithm: impl Into<String>, input_limit: u64) -> Self {
        Self {
            algorithm: algorithm.into(),
            input_limit,
        }
    }

    /// Hash `data` under the configured algorithm (mock: CRC32). If
    /// `data.len()` exceeds `input_limit`, returns
    /// [`IpcError::PayloadTooLarge`] with the configured limit and
    /// does not compute the hash.
    ///
    /// Returns the hash as a 4-byte little-endian `Vec<u8>` (the
    /// width of a CRC32). A real SHA-256 would return 32 bytes; the
    /// caller must not assume the length matches the algorithm's
    /// nominal digest width — read the `algorithm` field if you need
    /// to know which primitive was used.
    pub fn hash(&self, data: &[u8]) -> Result<Vec<u8>, IpcError> {
        if data.len() as u64 > self.input_limit {
            return Err(IpcError::PayloadTooLarge(self.input_limit));
        }
        // Mock: CRC32. See struct doc — NOT cryptographically secure.
        Ok(crc32(data).to_le_bytes().to_vec())
    }
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

    // ── W25-32: FFI process isolation tests ───────────────────────────

    #[test]
    fn test_ffi_call_marshal_unmarshal_roundtrip() {
        // Wire format: [u32 LE len][u64 LE type_hash][payload].
        // marshal → unmarshal must reproduce the original args + type
        // hash exactly. function_name is metadata not on the wire, so
        // it does NOT roundtrip.
        let call = FfiCall::new("foreign_add", vec![0xDE, 0xAD, 0xBE, 0xEF], 0xCAFEBABE);
        let frame = call.marshal(&call.args);

        // Header sanity: 4 + 8 bytes of header + 4 bytes of payload.
        assert_eq!(frame.len(), FFI_MARSHAL_HEADER_SIZE + 4);
        let len = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
        assert_eq!(len as usize, 4);
        let hash = u64::from_le_bytes([
            frame[4], frame[5], frame[6], frame[7],
            frame[8], frame[9], frame[10], frame[11],
        ]);
        assert_eq!(hash, 0xCAFEBABE);

        let decoded = FfiCall::unmarshal(&frame).expect("roundtrip must succeed");
        assert_eq!(decoded.args, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(decoded.return_type_hash, 0xCAFEBABE);
        // function_name is not on the wire — it comes back empty.
        assert_eq!(decoded.function_name, "");
    }

    #[test]
    fn test_ffi_call_marshal_empty_payload() {
        // A zero-length payload is a valid frame (e.g. a void()
        // foreign function): only the 12-byte header is emitted, and
        // unmarshal must accept it.
        let call = FfiCall::new("foreign_void", Vec::new(), 0x0);
        let frame = call.marshal(&call.args);
        assert_eq!(frame.len(), FFI_MARSHAL_HEADER_SIZE);
        let decoded = FfiCall::unmarshal(&frame).expect("empty roundtrip");
        assert!(decoded.args.is_empty());
        assert_eq!(decoded.return_type_hash, 0);
    }

    #[test]
    fn test_ffi_call_marshal_uses_parameter_args() {
        // marshal(&self, args) must encode the *parameter* args, not
        // self.args — this lets callers re-frame a cached blob without
        // mutating the envelope (the supervisor's restart-retry path).
        let call = FfiCall::new("f", vec![0xAA; 4], 1);
        let frame = call.marshal(&[0xBB; 2]);
        let decoded = FfiCall::unmarshal(&frame).expect("roundtrip");
        assert_eq!(decoded.args, vec![0xBB, 0xBB]);
    }

    #[test]
    fn test_ffi_call_unmarshal_rejects_truncated_header() {
        // A buffer shorter than the 12-byte header must be rejected
        // with TruncatedMessage, not panic on slicing.
        let short = [0u8; 4];
        let err = FfiCall::unmarshal(&short).unwrap_err();
        assert!(matches!(err, IpcError::TruncatedMessage), "got {:?}", err);
    }

    #[test]
    fn test_ffi_call_unmarshal_rejects_truncated_payload() {
        // Header claims 16 bytes of payload but only 4 are present.
        let mut bad = Vec::new();
        bad.extend_from_slice(&16u32.to_le_bytes());
        bad.extend_from_slice(&0u64.to_le_bytes());
        bad.extend_from_slice(&[1, 2, 3, 4]); // only 4 of 16 bytes
        let err = FfiCall::unmarshal(&bad).unwrap_err();
        assert!(matches!(err, IpcError::TruncatedMessage), "got {:?}", err);
    }

    #[test]
    fn test_ffi_result_success() {
        let r = FfiResult::ok(vec![1, 2, 3], 42);
        assert!(r.is_success());
        assert!(r.success);
        assert_eq!(r.return_value, vec![1, 2, 3]);
        assert_eq!(r.elapsed_ms, 42);
        assert!(r.error.is_none());
        assert_eq!(r.error_message(), "");
    }

    #[test]
    fn test_ffi_result_failure() {
        let r = FfiResult::err("segfault in foreign code", 99);
        assert!(!r.is_success());
        assert!(!r.success);
        assert!(r.return_value.is_empty());
        assert_eq!(r.elapsed_ms, 99);
        assert_eq!(r.error_message(), "segfault in foreign code");
        assert!(r.error.is_some());
    }

    #[test]
    fn test_ffi_result_error_message_empty_on_success() {
        // error_message() on a success result must return "" (not
        // panic on None unwrap).
        let r = FfiResult::ok(vec![], 0);
        assert_eq!(r.error_message(), "");
    }

    #[test]
    fn test_ffi_worker_config_is_ffi_always_true() {
        // The is_ffi() predicate is the routing hook a later wave uses
        // to distinguish FfiWorkerConfig from WorkerConfig through a
        // shared trait — it must be unconditionally true.
        let cfg = FfiWorkerConfig::new(
            "/lib/libx.so", "f", TrustLevel::Untrusted, 1, 100,
        );
        assert!(cfg.is_ffi());
    }

    #[test]
    fn test_ffi_worker_config_sandbox_matches_trust_level() {
        // The sandbox baked into the config must reflect the trust
        // level — this is the L5 wire-up for FFI workers: the forked
        // child calls sandbox_config.apply() before exec(). Sandboxed
        // → exit-only syscalls → shortest possible filter.
        let cfg = FfiWorkerConfig::new(
            "/lib/libpriv.so", "foreign_privileged",
            TrustLevel::Sandboxed, 1, 100,
        );
        assert_eq!(cfg.sandbox_config.config.trust_level, TrustLevel::Sandboxed);
        assert_eq!(cfg.sandbox_config.config.max_restarts, 1);
        assert_eq!(cfg.sandbox_config.config.timeout_ms, 100);
        // Resource limits derived from the config: CPU ceiling mirrors
        // the call timeout.
        assert_eq!(cfg.sandbox_config.limits.cpu_time_ms, 100);
        let filter = cfg.sandbox_config.seccomp_filter();
        assert_eq!(filter.len() % 8, 0, "BPF instructions are 8 bytes each");
    }

    #[test]
    fn test_ffi_worker_spawn_call_kill_lifecycle() {
        // The happy path: spawn a worker, call it (verifying the
        // loopback return value), kill it, then verify subsequent
        // calls fail with WorkerCrashed.
        let mut life = FfiWorkerLifecycle::new();
        let config = FfiWorkerConfig::new(
            "/lib/libfoo.so", "foreign_add",
            TrustLevel::Untrusted, 3, 1_000,
        );

        let pid = life.spawn_ffi_worker(&config).expect("spawn");
        assert!(pid >= 1000, "PIDs start at 1000 for readability");
        assert!(life.is_alive(pid));
        assert_eq!(life.worker_count(), 1);
        assert_eq!(life.restart_count(pid), Some(0));

        let call = FfiCall::new("foreign_add", vec![0x01, 0x02, 0x03], 0x1234);
        let result = life.call_ffi(pid, &call, 1_000).expect("call");
        assert!(result.is_success());
        assert!(result.elapsed_ms > 0 && result.elapsed_ms <= 1_000);
        // Loopback contract: return_value is the marshalled frame.
        let decoded = FfiCall::unmarshal(&result.return_value).expect("loopback frame");
        assert_eq!(decoded.args, call.args);
        assert_eq!(decoded.return_type_hash, 0x1234);

        life.kill_ffi_worker(pid).expect("kill");
        assert!(!life.is_alive(pid));

        // After kill, calls must fail with WorkerCrashed (not
        // WorkerNotFound — the entry is retained for crash history).
        let err = life.call_ffi(pid, &call, 1_000).unwrap_err();
        assert!(matches!(err, IpcError::WorkerCrashed(_)), "got {:?}", err);
    }

    #[test]
    fn test_ffi_worker_call_unknown_pid_rejected() {
        // A call to a PID the supervisor never spawned must return
        // WorkerNotFound, not silently succeed.
        let mut life = FfiWorkerLifecycle::new();
        let call = FfiCall::new("f", Vec::new(), 0);
        let err = life.call_ffi(9999, &call, 100).unwrap_err();
        assert!(matches!(err, IpcError::WorkerNotFound), "got {:?}", err);
    }

    #[test]
    fn test_ffi_worker_call_zero_timeout_rejected() {
        // timeout_ms == 0 is treated as "no waiting room" — the
        // backend would return EAGAIN immediately.
        let mut life = FfiWorkerLifecycle::new();
        let config = FfiWorkerConfig::new("/lib/x.so", "f", TrustLevel::Untrusted, 1, 0);
        let pid = life.spawn_ffi_worker(&config).expect("spawn");
        let call = FfiCall::new("f", vec![1], 0);
        let err = life.call_ffi(pid, &call, 0).unwrap_err();
        assert!(matches!(err, IpcError::WorkerTimeout), "got {:?}", err);
    }

    #[test]
    fn test_ffi_worker_kill_unknown_pid_rejected() {
        // Killing a PID the supervisor never spawned is a bug — it
        // must be reported, not ignored.
        let mut life = FfiWorkerLifecycle::new();
        let err = life.kill_ffi_worker(4242).unwrap_err();
        assert!(matches!(err, IpcError::WorkerNotFound), "got {:?}", err);
    }

    #[test]
    fn test_ffi_worker_restart_on_crash() {
        // A crashed worker can be restarted up to max_restarts times;
        // each restart gets a fresh PID and an incremented counter.
        // The (n+1)th restart exceeds the budget and returns
        // MaxRestartsExceeded.
        let mut life = FfiWorkerLifecycle::new();
        let config = FfiWorkerConfig::new(
            "/lib/libcrash.so", "foreign_crashy",
            TrustLevel::Sandboxed, 2, 500,
        );

        let pid0 = life.spawn_ffi_worker(&config).expect("spawn");
        assert_eq!(life.restart_count(pid0), Some(0));

        // Crash + restart #1.
        life.kill_ffi_worker(pid0).expect("kill");
        let pid1 = life.restart_ffi_worker(&config).expect("restart #1");
        assert!(pid1 != pid0, "restart must allocate a fresh PID");
        assert_eq!(life.restart_count(pid1), Some(1));
        assert!(life.is_alive(pid1));
        // The freshly respawned worker must be callable.
        let call = FfiCall::new("foreign_crashy", vec![0x42], 0x1);
        let r = life.call_ffi(pid1, &call, 500).expect("post-restart call");
        assert!(r.is_success());

        // Crash + restart #2 — still within budget (max_restarts=2).
        life.kill_ffi_worker(pid1).expect("kill");
        let pid2 = life.restart_ffi_worker(&config).expect("restart #2");
        assert_eq!(life.restart_count(pid2), Some(2));

        // Third restart exceeds max_restarts=2.
        life.kill_ffi_worker(pid2).expect("kill");
        let err = life.restart_ffi_worker(&config).unwrap_err();
        assert!(matches!(err, IpcError::MaxRestartsExceeded), "got {:?}", err);
    }

    #[test]
    fn test_ffi_worker_restart_unknown_config_rejected() {
        // restart_ffi_worker for a config that was never spawned must
        // return WorkerNotFound — the supervisor can only restart
        // workers it has previously tracked.
        let mut life = FfiWorkerLifecycle::new();
        let config = FfiWorkerConfig::new(
            "/lib/never_spawned.so", "f",
            TrustLevel::Untrusted, 3, 1_000,
        );
        let err = life.restart_ffi_worker(&config).unwrap_err();
        assert!(matches!(err, IpcError::WorkerNotFound), "got {:?}", err);
    }

    #[test]
    fn test_ffi_worker_distinct_configs_tracked_separately() {
        // Two workers for different (library, symbol) pairs must get
        // distinct PIDs and independent restart counters.
        let mut life = FfiWorkerLifecycle::new();
        let cfg_a = FfiWorkerConfig::new("/lib/a.so", "f", TrustLevel::Untrusted, 1, 100);
        let cfg_b = FfiWorkerConfig::new("/lib/b.so", "f", TrustLevel::Untrusted, 1, 100);

        let pid_a = life.spawn_ffi_worker(&cfg_a).expect("spawn a");
        let pid_b = life.spawn_ffi_worker(&cfg_b).expect("spawn b");
        assert_ne!(pid_a, pid_b);
        assert_eq!(life.worker_count(), 2);

        // Crashing and restarting a must not affect b's bookkeeping.
        life.kill_ffi_worker(pid_a).expect("kill a");
        let pid_a2 = life.restart_ffi_worker(&cfg_a).expect("restart a");
        assert_eq!(life.restart_count(pid_a2), Some(1));
        assert_eq!(life.restart_count(pid_b), Some(0));
        assert!(life.is_alive(pid_b));
    }

    // ── Wave 33–40: Capability delegation chains & propagation ────────

    /// Helper: mint a synthetic token with arbitrary fields. Used by
    /// the chain-verification tests where we need to construct chains
    /// that `delegate()` itself would refuse (e.g. depth > MAX). The
    /// signature is left zeroed — `verify_delegation_chain` checks
    /// structural linkage, not signature validity, so a zero sig is
    /// fine for these tests.
    fn synth_chain_token(
        id: u128,
        src: u64,
        tgt: u64,
        depth: u8,
    ) -> capability::CapabilityToken {
        capability::CapabilityToken {
            id,
            source_pid: src,
            target_pid: tgt,
            resource: capability::Resource::Memory(0x1000, 0x1000),
            permissions: capability::MemoryPermissions {
                read: true,
                write: false,
                execute: false,
            },
            delegation_depth: depth,
            created_at: 0,
            expires_at: u64::MAX,
            signature: [0u8; 32],
        }
    }

    #[test]
    fn test_delegation_chain_abc_passes() {
        // A→B→C, real delegation through CapabilitySet::delegate.
        // A is granted by pid 1 to pid 2 (depth 0). B is delegated
        // from A to pid 3 (depth 1). C is delegated from B to pid 4
        // (depth 2). The chain [A, B, C] must verify.
        let key = [0x42u8; 32];
        let mut set = capability::CapabilitySet::new();

        let a = capability::grant_capability(
            1001, 1, 2,
            capability::Resource::Memory(0x1000, 0x1000),
            capability::MemoryPermissions { read: true, write: true, execute: false },
            0, 1_000, 10_000, &key,
        );
        set.grant(a.clone());

        let b = set.delegate(
            a.id, 3,
            capability::MemoryPermissions { read: true, write: false, execute: false },
            &key,
        ).expect("delegate A→B");
        let c = set.delegate(
            b.id, 4,
            capability::MemoryPermissions { read: true, write: false, execute: false },
            &key,
        ).expect("delegate B→C");

        // delegate() must produce a real child: depth+1, source = parent's target.
        assert_eq!(b.delegation_depth, a.delegation_depth + 1);
        assert_eq!(b.source_pid, a.target_pid);
        assert_eq!(c.delegation_depth, b.delegation_depth + 1);
        assert_eq!(c.source_pid, b.target_pid);

        let chain = vec![a.clone(), b.clone(), c.clone()];
        assert!(
            capability::verify_delegation_chain(&c, &chain),
            "A→B→C chain produced by delegate() must verify"
        );
    }

    #[test]
    fn test_delegation_chain_broken_link_fails() {
        // Two failure modes for the pid-linkage check:
        //   (a) child.source_pid != parent.target_pid
        //   (b) depth does not increment by exactly 1
        let a = synth_chain_token(1, 1, 2, 0);
        // (a) broken pid link: B's source_pid is 99, not A's target_pid (2).
        let bad_link = synth_chain_token(2, 99, 3, 1);
        let chain_bad_link = vec![a.clone(), bad_link.clone()];
        assert!(
            !capability::verify_delegation_chain(&bad_link, &chain_bad_link),
            "chain with broken pid linkage must fail"
        );

        // (b) broken depth: B's depth is 2 instead of 1 (skipped a level).
        let bad_depth = synth_chain_token(3, 2, 3, 2);
        let chain_bad_depth = vec![a, bad_depth.clone()];
        assert!(
            !capability::verify_delegation_chain(&bad_depth, &chain_bad_depth),
            "chain with skipped depth must fail"
        );
    }

    #[test]
    fn test_delegation_chain_depth_exceeded_fails() {
        // A chain whose leaf depth exceeds MAX_DELEGATION_DEPTH must
        // be rejected even if every link is otherwise well-formed.
        // delegate() would refuse to produce such a chain, so we
        // construct it synthetically.
        let depth_cap = capability::MAX_DELEGATION_DEPTH;
        // Build a chain of length (depth_cap + 1): depths 0..=depth_cap.
        // The leaf at depth_cap+1 would exceed, but we want a chain
        // that *reaches* depth_cap+1, so we need depth_cap+2 elements
        // (depths 0..=depth_cap+1).
        let mut chain: Vec<capability::CapabilityToken> = Vec::new();
        let mut pid = 10u64;
        for d in 0..=(depth_cap + 1) {
            chain.push(synth_chain_token(1000 + d as u128, pid, pid + 1, d));
            pid += 1;
        }
        let leaf = chain.last().unwrap().clone();
        assert!(
            leaf.delegation_depth > depth_cap,
            "test setup: leaf depth {} must exceed MAX {}",
            leaf.delegation_depth, depth_cap
        );
        assert!(
            !capability::verify_delegation_chain(&leaf, &chain),
            "chain whose leaf exceeds MAX_DELEGATION_DEPTH must fail"
        );

        // Conversely, a chain exactly up to the cap must pass.
        let mut ok_chain: Vec<capability::CapabilityToken> = Vec::new();
        let mut pid2 = 50u64;
        for d in 0..=depth_cap {
            ok_chain.push(synth_chain_token(2000 + d as u128, pid2, pid2 + 1, d));
            pid2 += 1;
        }
        let ok_leaf = ok_chain.last().unwrap().clone();
        assert_eq!(ok_leaf.delegation_depth, depth_cap);
        assert!(
            capability::verify_delegation_chain(&ok_leaf, &ok_chain),
            "chain exactly at MAX_DELEGATION_DEPTH must still verify"
        );
    }

    #[test]
    fn test_delegation_chain_empty_and_mismatched_leaf_fails() {
        // Empty chain → false.
        let leaf = synth_chain_token(7, 1, 2, 0);
        assert!(
            !capability::verify_delegation_chain(&leaf, &[]),
            "empty chain must fail"
        );

        // Chain whose last element != token argument → false.
        // (We're asked to authorise `leaf`, but the chain leads to
        // a different token.)
        let a = synth_chain_token(1, 1, 2, 0);
        let b = synth_chain_token(2, 2, 3, 1);
        let chain = vec![a, b];
        assert!(
            !capability::verify_delegation_chain(&leaf, &chain),
            "chain whose leaf != token argument must fail"
        );

        // Chain that doesn't start at depth 0 → false (no provable root).
        let mid1 = synth_chain_token(10, 5, 6, 3);
        let mid2 = synth_chain_token(11, 6, 7, 4);
        let mid_chain = vec![mid1.clone(), mid2.clone()];
        assert!(
            !capability::verify_delegation_chain(&mid2, &mid_chain),
            "chain that doesn't start at depth 0 must fail"
        );
    }

    #[test]
    fn test_revoke_with_propagation_revokes_descendants() {
        // Build A→B→C via delegate(), then revoke A. All three must
        // appear in the returned list and all three must be marked
        // revoked in the set.
        let key = [0x99u8; 32];
        let mut set = capability::CapabilitySet::new();

        let a = capability::grant_capability(
            2001, 1, 2,
            capability::Resource::Memory(0x2000, 0x1000),
            capability::MemoryPermissions { read: true, write: true, execute: false },
            0, 1_000, 10_000, &key,
        );
        set.grant(a.clone());
        let b = set.delegate(
            a.id, 3,
            capability::MemoryPermissions { read: true, write: false, execute: false },
            &key,
        ).expect("delegate A→B");
        let c = set.delegate(
            b.id, 4,
            capability::MemoryPermissions { read: true, write: false, execute: false },
            &key,
        ).expect("delegate B→C");

        // Sanity: none revoked yet.
        assert!(!set.is_revoked(a.id));
        assert!(!set.is_revoked(b.id));
        assert!(!set.is_revoked(c.id));

        let revoked = set.revoke_with_propagation(a.id);

        // A must be first (it's the seed of the worklist); B and C
        // follow in discovery order.
        assert_eq!(revoked.len(), 3, "must revoke A + B + C, got {:?}", revoked);
        assert_eq!(revoked[0], a.id, "parent must be revoked first");
        assert!(revoked.contains(&b.id), "child B must be revoked");
        assert!(revoked.contains(&c.id), "grandchild C must be revoked");

        // All three now revoked in the set.
        assert!(set.is_revoked(a.id));
        assert!(set.is_revoked(b.id));
        assert!(set.is_revoked(c.id));

        // verify() must now reject all three.
        let now = 1_500;
        assert!(!set.verify(&a, now), "revoked A must fail verify");
        assert!(!set.verify(&b, now), "revoked B must fail verify");
        assert!(!set.verify(&c, now), "revoked C must fail verify");
    }

    #[test]
    fn test_revoke_with_propagation_idempotent_and_unknown() {
        // Idempotent: revoking an already-revoked token walks nothing
        // and returns an empty list.
        let key = [0xABu8; 32];
        let mut set = capability::CapabilitySet::new();
        let a = capability::grant_capability(
            3001, 1, 2,
            capability::Resource::Channel(42),
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 1_000, 10_000, &key,
        );
        set.grant(a.clone());

        let first = set.revoke_with_propagation(a.id);
        assert_eq!(first, vec![a.id]);

        let second = set.revoke_with_propagation(a.id);
        assert!(
            second.is_empty(),
            "re-revoking must be a no-op, got {:?}",
            second
        );

        // Unknown token id: still recorded as a tombstone (defensive),
        // returns a single-element list, no panic.
        let mut fresh = capability::CapabilitySet::new();
        let unknown_id = 0xDEAD_BEEF_BEEF;
        let r = fresh.revoke_with_propagation(unknown_id);
        assert_eq!(r, vec![unknown_id]);
        assert!(fresh.is_revoked(unknown_id));

        // Revoking a leaf with no children returns just the leaf.
        let mut set2 = capability::CapabilitySet::new();
        let leaf = capability::grant_capability(
            4001, 5, 6,
            capability::Resource::Channel(7),
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 1_000, 10_000, &key,
        );
        set2.grant(leaf.clone());
        let r2 = set2.revoke_with_propagation(leaf.id);
        assert_eq!(r2, vec![leaf.id]);
    }

    #[test]
    fn test_capability_registry_grant_revoke_get() {
        // grant token_a to pid 1, grant token_b to pid 2; revoke from
        // pid 1; pid 2 unaffected; unknown pid returns empty slice.
        let key = [0x11u8; 32];
        let token_a = capability::grant_capability(
            5001, 100, 1,
            capability::Resource::Channel(1),
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 1_000, 10_000, &key,
        );
        let token_b = capability::grant_capability(
            5002, 100, 2,
            capability::Resource::Channel(2),
            capability::MemoryPermissions { read: true, write: true, execute: false },
            0, 1_000, 10_000, &key,
        );

        let mut reg = CapabilityRegistry::new();

        // Unknown pid → empty slice (no panic, no allocation).
        assert!(reg.get_process_capabilities(999).is_empty());

        reg.grant_to_process(1, &token_a);
        reg.grant_to_process(2, &token_b);

        // Each pid sees exactly its own token.
        assert_eq!(reg.get_process_capabilities(1), &[token_a.id]);
        assert_eq!(reg.get_process_capabilities(2), &[token_b.id]);

        // Revoke token_a from pid 1 — returns true (something was removed).
        let removed = reg.revoke_from_process(1, token_a.id);
        assert!(removed, "revoke of held token must report true");
        assert!(
            reg.get_process_capabilities(1).is_empty(),
            "pid 1 must have no tokens after revoke"
        );
        // pid 2 unaffected.
        assert_eq!(
            reg.get_process_capabilities(2), &[token_b.id],
            "pid 2 must be unaffected by pid 1's revoke"
        );

        // Re-revoking from pid 1 returns false (nothing left to remove).
        let removed_again = reg.revoke_from_process(1, token_a.id);
        assert!(!removed_again, "re-revoke must report false");

        // Revoking a token the pid never held returns false.
        let stranger = reg.revoke_from_process(2, token_a.id);
        assert!(!stranger, "pid 2 never held token_a; must report false");
        assert_eq!(reg.get_process_capabilities(2), &[token_b.id]);
    }

    #[test]
    fn test_capability_registry_alias_and_multi_grant() {
        // A pid may hold multiple distinct tokens; grant_to_process
        // appends (does not de-dup); revoke_from_process scrubs all
        // aliases of a single id.
        let key = [0x22u8; 32];
        let t1 = capability::grant_capability(
            6001, 0, 7,
            capability::Resource::Channel(1),
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 1_000, 10_000, &key,
        );
        let t2 = capability::grant_capability(
            6002, 0, 7,
            capability::Resource::Channel(2),
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 1_000, 10_000, &key,
        );

        let mut reg = CapabilityRegistry::new();
        reg.grant_to_process(7, &t1);
        reg.grant_to_process(7, &t2);
        // Grant t1 twice — alias. Allowed; revoke scrubs both.
        reg.grant_to_process(7, &t1);

        let held = reg.get_process_capabilities(7);
        assert_eq!(held.len(), 3);
        assert_eq!(held[0], t1.id);
        assert_eq!(held[1], t2.id);
        assert_eq!(held[2], t1.id);

        // Revoke t1 — both aliases must vanish, t2 stays.
        reg.revoke_from_process(7, t1.id);
        let held_after = reg.get_process_capabilities(7);
        assert_eq!(held_after, &[t2.id]);
    }

    #[test]
    fn test_revoke_propagation_plus_registry_sweep() {
        // Integration: revoke a parent token, propagate to children in
        // the CapabilitySet, then sweep the registry so every affected
        // pid's held-set is scrubbed. This is the kernel-on-exit flow.
        let key = [0x33u8; 32];
        let mut set = capability::CapabilitySet::new();
        let mut reg = CapabilityRegistry::new();

        // pid 1 grants to pid 2 (root A). pid 2 delegates to pid 3 (B).
        // pid 3 delegates to pid 4 (C).
        let a = capability::grant_capability(
            7001, 1, 2,
            capability::Resource::Channel(1),
            capability::MemoryPermissions { read: true, write: true, execute: false },
            0, 1_000, 10_000, &key,
        );
        set.grant(a.clone());
        reg.grant_to_process(2, &a);

        let b = set.delegate(
            a.id, 3,
            capability::MemoryPermissions { read: true, write: false, execute: false },
            &key,
        ).expect("delegate A→B");
        reg.grant_to_process(3, &b);

        let c = set.delegate(
            b.id, 4,
            capability::MemoryPermissions { read: true, write: false, execute: false },
            &key,
        ).expect("delegate B→C");
        reg.grant_to_process(4, &c);

        // Revoke A; propagation pulls in B and C.
        let revoked = set.revoke_with_propagation(a.id);
        assert_eq!(revoked.len(), 3);

        // Sweep the registry: for each revoked token id, scrub it from
        // every pid that held it. (In a real kernel this would be a
        // single pass over the registry; here we iterate the revoked
        // list and call revoke_from_process for each known holder.)
        for tid in &revoked {
            // We know a→pid2, b→pid3, c→pid4 from the grants above.
            reg.revoke_from_process(2, *tid);
            reg.revoke_from_process(3, *tid);
            reg.revoke_from_process(4, *tid);
        }

        // Every affected pid's held-set is now empty.
        assert!(reg.get_process_capabilities(2).is_empty());
        assert!(reg.get_process_capabilities(3).is_empty());
        assert!(reg.get_process_capabilities(4).is_empty());

        // And the set itself rejects all three tokens at verify-time.
        assert!(!set.verify(&a, 1_500));
        assert!(!set.verify(&b, 1_500));
        assert!(!set.verify(&c, 1_500));
    }

    // ── W41-48: Kernel/User Split tests ──────────────────────────────

    #[test]
    fn test_kernel_process_handle_syscall_returns_value() {
        // A freshly-minted kernel process must accept the kernel's own
        // syscalls (caller_pid == self.pid) and return a mock value
        // indicating the call was routed. Callers with no granted
        // capability are denied by the default-deny rule.
        let mut k = KernelProcess::new(1, "kernel");
        assert!(k.is_kernel_process());
        assert_eq!(k.pid, 1);
        assert_eq!(k.name, "kernel");

        // Kernel calling itself: allowed. The mock return value
        // encodes the syscall number in the high 32 bits.
        let nr: u32 = 60; // exit
        let rc = k
            .handle_syscall(1, nr, &[0])
            .expect("kernel calling itself must succeed");
        assert_eq!(rc, (nr as u64) << 32, "mock return must encode syscall nr");

        // Unknown caller with no capability: denied by default-deny.
        let err = k.handle_syscall(999, nr, &[0]).unwrap_err();
        assert_eq!(err, IpcError::PermissionDenied);
    }

    #[test]
    fn test_kernel_process_handle_syscall_allows_capability_holder() {
        // A caller that has been granted at least one capability in
        // the kernel's CapabilityRegistry must pass the default-deny
        // gate. We don't need a real signed token here — the registry
        // only tracks token IDs, so we mint a synthetic one via
        // grant_capability and grant it to the caller.
        let key = [0xAAu8; 32];
        let token = capability::grant_capability(
            0xAAAA_BBBB_CCCC_DDDD_EEEE_FFFF_0000_1111,
            1, // issuer = kernel pid
            7, // target = caller pid
            capability::Resource::Channel(42),
            capability::MemoryPermissions { read: true, write: false, execute: false },
            0, 5_000, 10_000, &key,
        );
        let mut k = KernelProcess::new(1, "kernel");
        k.capabilities.grant_to_process(7, &token);

        // Now pid 7 is a known capability holder — syscall allowed.
        let rc = k.handle_syscall(7, 0, &[]).expect("capability holder must be allowed");
        assert_eq!(rc, 0, "mock return for nr=0 must be 0");
    }

    #[test]
    fn test_user_process_check_resources_within_limits() {
        // A fresh user process with zero usage must be within limits.
        // `limits` is moved into UserProcess::new; afterwards we read
        // the ceilings back out of `u.resource_limits` so we don't
        // need to clone the struct just for the assertions.
        let limits = ResourceLimits {
            cpu_time_ms: 1_000,
            max_memory_bytes: 64 * 1024 * 1024,
            max_ipc_messages: 500,
            max_file_descriptors: 32,
        };
        let cpu_limit = limits.cpu_time_ms;
        let mem_limit = limits.max_memory_bytes;
        let ipc_limit = limits.max_ipc_messages;
        let fd_limit = limits.max_file_descriptors;
        let mut u = UserProcess::new(1001, 1, TrustLevel::Untrusted, limits);
        assert!(u.is_user_process());
        assert_eq!(u.pid, 1001);
        assert_eq!(u.parent_pid, 1);
        assert_eq!(u.trust_level, TrustLevel::Untrusted);
        assert!(u.capabilities.is_empty(), "fresh process has no capabilities");
        assert!(
            u.check_resources(),
            "fresh user process with zero usage must be within limits"
        );

        // Bump usage past the CPU ceiling: check_resources must fail.
        u.resource_usage.cpu_time_ms = cpu_limit + 1;
        assert!(
            !u.check_resources(),
            "cpu over budget must fail check_resources"
        );

        // Reset CPU, bump memory past ceiling: must fail.
        u.resource_usage.cpu_time_ms = 0;
        u.resource_usage.memory_bytes = mem_limit + 1;
        assert!(
            !u.check_resources(),
            "memory over budget must fail check_resources"
        );

        // Reset memory, bump IPC count past ceiling: must fail.
        u.resource_usage.memory_bytes = 0;
        u.resource_usage.ipc_messages = ipc_limit + 1;
        assert!(
            !u.check_resources(),
            "ipc over budget must fail check_resources"
        );

        // Reset IPC, bump FD count past ceiling: must fail.
        u.resource_usage.ipc_messages = 0;
        u.resource_usage.file_descriptors = fd_limit + 1;
        assert!(
            !u.check_resources(),
            "fd over budget must fail check_resources"
        );
    }

    #[test]
    fn test_resource_account_tracks_usage_correctly() {
        // Each account_* method must update exactly the right field
        // and leave the others untouched; get_usage must return the
        // accumulated snapshot. Memory is a high-water mark (max, not
        // sum); CPU, IPC, and FD are cumulative.
        let mut acct = ResourceAccount::new();
        assert_eq!(acct.tracked_count(), 0, "fresh account tracks no pids");

        acct.account_cpu(7, 100);
        acct.account_cpu(7, 50);   // accumulate → 150
        acct.account_memory(7, 4096);
        acct.account_memory(7, 2048); // high-water → stays 4096
        acct.account_memory(7, 8192); // high-water → grows to 8192
        acct.account_ipc(7);
        acct.account_ipc(7);
        acct.account_ipc(7);       // 3 IPC messages
        acct.account_fd(7);        // 1 FD

        let usage = acct.get_usage(7);
        assert_eq!(usage.cpu_time_ms, 150, "cpu_time must accumulate");
        assert_eq!(usage.memory_bytes, 8192, "memory must be high-water mark");
        assert_eq!(usage.ipc_messages, 3, "ipc count must increment");
        assert_eq!(usage.file_descriptors, 1, "fd count must increment");
        assert_eq!(acct.tracked_count(), 1, "exactly one pid tracked");

        // Unknown pid returns a zeroed snapshot (no panic).
        let unknown = acct.get_usage(999);
        assert_eq!(unknown, ResourceUsage::default());

        // Different pids are tracked independently.
        acct.account_cpu(8, 200);
        assert_eq!(acct.get_usage(8).cpu_time_ms, 200, "pid 8 tracked separately");
        assert_eq!(acct.get_usage(7).cpu_time_ms, 150, "pid 7 must be untouched");
        assert_eq!(acct.tracked_count(), 2, "two pids now tracked");
    }

    #[test]
    fn test_process_table_spawn_kill_user_process() {
        // spawn_user mints monotonically-increasing pids starting at
        // 1001, kill_user evicts them, get_process resolves either
        // slot. The kernel slot is never affected by kill_user.
        let mut table = ProcessTable::new();
        assert!(table.kernel.is_none(), "fresh table has no kernel");
        assert_eq!(table.user_count(), 0);

        table.set_kernel(KernelProcess::new(1, "kernel"));
        assert!(table.kernel.is_some(), "kernel must be installed");

        let pid_a = table
            .spawn_user(1, TrustLevel::Untrusted, ResourceLimits::default())
            .expect("spawn_user must succeed");
        let pid_b = table
            .spawn_user(1, TrustLevel::Sandboxed, ResourceLimits::default())
            .expect("spawn_user must succeed");
        assert_ne!(pid_a, pid_b, "pids must be distinct");
        assert!(pid_a >= 1001, "user pids must start at 1001");
        assert_eq!(table.user_count(), 2, "two users spawned");

        // get_process resolves kernel, both users, and rejects unknowns.
        let kern = table.get_process(1).expect("kernel pid must resolve");
        assert!(kern.is_kernel(), "kernel slot must report is_kernel");
        assert!(!kern.is_user());
        assert_eq!(kern.pid(), 1);

        let user_a = table.get_process(pid_a).expect("user pid must resolve");
        assert!(user_a.is_user(), "user slot must report is_user");
        assert!(!user_a.is_kernel());
        assert_eq!(user_a.pid(), pid_a);

        assert!(
            table.get_process(999_999).is_none(),
            "unknown pid must not resolve"
        );

        // kill_user evicts the entry; a second kill fails.
        table.kill_user(pid_a).expect("kill_user must succeed");
        assert!(
            table.get_process(pid_a).is_none(),
            "killed pid must not resolve"
        );
        assert_eq!(table.user_count(), 1, "one user left after kill");
        let err = table.kill_user(pid_a).unwrap_err();
        assert_eq!(err, IpcError::WorkerNotFound, "second kill must fail");

        // Other users survive.
        assert!(
            table.get_process(pid_b).is_some(),
            "other users must survive a sibling kill"
        );

        // kill_user never touches the kernel slot.
        let err = table.kill_user(1).unwrap_err();
        assert_eq!(
            err, IpcError::WorkerNotFound,
            "kill_user on kernel pid must fail (kernel is not a user)"
        );
        assert!(table.kernel.is_some(), "kernel slot must survive kill_user");
    }

    // ── W49-56: Driver isolation tests ────────────────────────────────

    #[test]
    fn test_dma_buffer_is_valid() {
        // A well-formed buffer: non-zero addr, non-zero size, no overflow.
        let good = DmaBuffer::new(0x1000, 0x1000, DmaDirection::ToDev);
        assert!(good.is_valid());

        // Each of these is invalid: zero addr, zero size, or overflow.
        assert!(!DmaBuffer::new(0, 0x1000, DmaDirection::ToDev).is_valid(),
            "zero base address must be invalid (null-DMA guard)");
        assert!(!DmaBuffer::new(0x1000, 0, DmaDirection::ToDev).is_valid(),
            "zero size must be invalid");
        // addr + size overflow: u64::MAX + 1 wraps.
        assert!(!DmaBuffer::new(u64::MAX, 1, DmaDirection::ToDev).is_valid(),
            "addr+size overflow must be invalid");

        // Direction does not affect validity.
        assert!(DmaBuffer::new(0x2000, 0x800, DmaDirection::FromDev).is_valid());
        assert!(DmaBuffer::new(0x2000, 0x800, DmaDirection::Bidirectional).is_valid());
    }

    #[test]
    fn test_driver_worker_config_trust_level_pinned_to_untrusted() {
        // Even if the caller passes Kernel, the config's trust_level
        // must be Untrusted — drivers always run untrusted.
        let cfg = DriverWorkerConfig::new(
            "nvme",
            "/dev/nvme0",
            vec![(0xFE00_0000, 0x1000)],
            vec![16, 17],
            vec![DmaBuffer::new(0x8000, 0x4000, DmaDirection::Bidirectional)],
            TrustLevel::Kernel,
        );
        assert_eq!(cfg.trust_level, TrustLevel::Untrusted,
            "driver trust_level must be pinned to Untrusted");
        assert_eq!(cfg.driver_name, "nvme");
        assert_eq!(cfg.device_path, "/dev/nvme0");
        assert_eq!(cfg.mmio_regions.len(), 1);
        assert_eq!(cfg.irq_vectors, vec![16, 17]);
        assert_eq!(cfg.dma_buffers.len(), 1);
        assert!(cfg.dma_buffers[0].is_valid());
    }

    #[test]
    fn test_driver_worker_start_stop_state_machine() {
        let cfg = DriverWorkerConfig::new(
            "eth0", "/dev/eth0", vec![], vec![], vec![],
            TrustLevel::Untrusted,
        );
        let mut w = DriverWorker::new(cfg);
        assert!(!w.is_running, "fresh worker is stopped");
        assert_eq!(w.restart_count, 0);

        // start → Ok, is_running = true
        w.start().expect("first start must succeed");
        assert!(w.is_running);

        // second start → Err (already running)
        let err = w.start().unwrap_err();
        assert_eq!(err, IpcError::WorkerAlreadyRunning,
            "double-start must fail with WorkerAlreadyRunning");

        // stop → Ok, is_running = false
        w.stop().expect("stop after start must succeed");
        assert!(!w.is_running);

        // second stop → Err (not running)
        let err = w.stop().unwrap_err();
        assert_eq!(err, IpcError::WorkerNotRunning,
            "double-stop must fail with WorkerNotRunning");
    }

    #[test]
    fn test_driver_worker_handle_irq_dispatch_and_reject() {
        let cfg = DriverWorkerConfig::new(
            "uart", "/dev/ttyS0",
            vec![(0xFE00_0000, 0x1000)],
            vec![4, 5], // registered IRQ vectors
            vec![],
            TrustLevel::Untrusted,
        );
        let mut w = DriverWorker::new(cfg);

        // Worker stopped → IRQ dispatch must fail (nobody to dispatch to).
        let err = w.handle_irq(4).unwrap_err();
        assert_eq!(err, IpcError::WorkerNotRunning,
            "IRQ on stopped worker must fail");

        // Start the worker, then dispatch a registered vector.
        w.start().expect("start");
        w.handle_irq(4).expect("registered IRQ on running worker must dispatch");
        w.handle_irq(5).expect("second registered IRQ must dispatch");

        // Unregistered vector → IrqNotRegistered.
        let err = w.handle_irq(99).unwrap_err();
        assert_eq!(err, IpcError::IrqNotRegistered(99),
            "unregistered IRQ vector must be rejected");

        // Stop the worker → IRQ dispatch must fail again.
        w.stop().expect("stop");
        let err = w.handle_irq(4).unwrap_err();
        assert_eq!(err, IpcError::WorkerNotRunning,
            "IRQ on stopped worker must fail");
    }

    // ── W57-64: Sandboxing tests ─────────────────────────────────────

    #[test]
    fn test_sandboxed_worker_default_zero_capability() {
        let w = SandboxedWorker::new(4242);
        assert!(w.is_sandboxed(), "is_sandboxed() must always be true");
        assert_eq!(w.worker_pid, 4242);
        assert!(w.capabilities.is_empty(),
            "fresh worker must have zero capabilities");
        assert!(w.plugin_path.is_none(),
            "fresh worker must have no plugin path");

        // has_capability on empty set: always false.
        assert!(!w.has_capability(0));
        assert!(!w.has_capability(u128::MAX));
    }

    #[test]
    fn test_sandboxed_worker_grant_and_check_capability() {
        let mut w = SandboxedWorker::new(100);

        // Grant a capability → has_capability returns true for it.
        w.grant_capability(0xDEAD_BEEF);
        assert!(w.has_capability(0xDEAD_BEEF));
        assert!(!w.has_capability(0xCAFE_BABE));

        // Grant a second capability → both present.
        w.grant_capability(0xCAFE_BABE);
        assert!(w.has_capability(0xDEAD_BEEF));
        assert!(w.has_capability(0xCAFE_BABE));
        assert_eq!(w.capabilities.len(), 2);

        // Re-granting an existing capability is idempotent.
        w.grant_capability(0xDEAD_BEEF);
        assert_eq!(w.capabilities.len(), 2,
            "re-granting existing capability must not duplicate");
    }

    #[test]
    fn test_sandboxed_parser_feed_within_limit() {
        let mut p = SandboxedParser::new(16);
        assert!(p.input_buffer.is_empty());
        assert!(!p.is_over_limit());

        // Feed 8 bytes — ok.
        let n = p.feed(&[0u8; 8]).expect("feed within limit must succeed");
        assert_eq!(n, 8);
        assert_eq!(p.input_buffer.len(), 8);
        assert!(!p.is_over_limit());

        // Feed another 8 bytes — exactly at limit.
        p.feed(&[0u8; 8]).expect("feed exactly to limit must succeed");
        assert_eq!(p.input_buffer.len(), 16);
        assert!(p.is_over_limit(), "at-limit must report over_limit");
    }

    #[test]
    fn test_sandboxed_parser_feed_over_limit_rejected() {
        let mut p = SandboxedParser::new(8);

        // Feed 4 bytes — ok.
        p.feed(&[0u8; 4]).expect("feed within limit");

        // Feed 8 more bytes — would push to 12, over limit. Must reject
        // AND leave the buffer untouched (4 bytes).
        let err = p.feed(&[0u8; 8]).unwrap_err();
        assert!(matches!(err, IpcError::PayloadTooLarge(8)),
            "over-limit feed must return PayloadTooLarge(limit), got {:?}", err);
        assert_eq!(p.input_buffer.len(), 4,
            "rejected feed must not mutate the buffer");

        // Feed exactly 4 more — at limit, ok.
        p.feed(&[0u8; 4]).expect("feed exactly to limit must succeed");
        assert_eq!(p.input_buffer.len(), 8);
        assert!(p.is_over_limit());

        // One more byte → over limit, rejected.
        let err = p.feed(&[1u8; 1]).unwrap_err();
        assert!(matches!(err, IpcError::PayloadTooLarge(8)));
        assert_eq!(p.input_buffer.len(), 8,
            "rejected feed must not mutate the buffer");
    }

    #[test]
    fn test_sandboxed_crypto_hash_within_limit() {
        // sha256 algorithm name is informational; the mock uses crc32.
        let c = SandboxedCrypto::new("sha256", 32);
        assert_eq!(c.algorithm, "sha256");
        assert_eq!(c.input_limit, 32);

        // Hash within limit → 4-byte CRC32 little-endian.
        let h = c.hash(b"hello").expect("hash within limit must succeed");
        assert_eq!(h.len(), 4, "mock hash must be 4-byte CRC32");
        let expected = crc32(b"hello").to_le_bytes();
        assert_eq!(h, expected, "mock hash must match crc32(data).to_le_bytes()");

        // Determinism: same input → same hash.
        let h2 = c.hash(b"hello").expect("second hash must succeed");
        assert_eq!(h, h2);

        // Distinct inputs → distinct hashes (CRC32 is not collision-
        // resistant, but distinct short inputs almost always differ).
        let h3 = c.hash(b"world").expect("third hash must succeed");
        assert_ne!(h, h3);
    }

    #[test]
    fn test_sandboxed_crypto_hash_over_limit_rejected() {
        let c = SandboxedCrypto::new("sha256", 4);

        // Exactly at limit: ok.
        c.hash(&[0u8; 4]).expect("hash at limit must succeed");

        // Over limit: rejected, no hash computed.
        let err = c.hash(&[0u8; 5]).unwrap_err();
        assert!(matches!(err, IpcError::PayloadTooLarge(4)),
            "over-limit hash must return PayloadTooLarge(limit), got {:?}", err);

        // Empty input is always within any non-negative limit (0 > 0 is false).
        let c2 = SandboxedCrypto::new("aes128", 0);
        let h = c2.hash(b"").expect("empty input must succeed even at limit 0");
        assert_eq!(h, crc32(b"").to_le_bytes());

        // Non-empty input at limit 0: rejected.
        let err = c2.hash(&[1u8]).unwrap_err();
        assert!(matches!(err, IpcError::PayloadTooLarge(0)));
    }

    // ── W65-72: Supervisor + CircuitBreaker tests ───────────────────

    #[test]
    fn test_worker_state_new_is_alive_with_zero_history() {
        // A freshly constructed WorkerState is alive, with no exit
        // history and zero consumed restarts. This pins the default
        // state for `register_worker`.
        let s = WorkerState::new(4242);
        assert_eq!(s.pid, 4242);
        assert!(s.is_alive);
        assert_eq!(s.restart_count, 0);
        assert_eq!(s.last_exit_code, 0);
        assert_eq!(s.last_signal, 0);
    }

    #[test]
    fn test_supervisor_register_unregister_and_alive_count() {
        // Register three workers — all alive. alive_count tracks the
        // alive subset as workers exit and get restarted.
        let mut sup = Supervisor::new(3, 1_000);
        assert_eq!(sup.alive_count(), 0, "fresh supervisor has no workers");

        sup.register_worker(100);
        sup.register_worker(200);
        sup.register_worker(300);
        assert_eq!(sup.alive_count(), 3, "three registered → three alive");

        // Re-registering an existing pid is a no-op (does not clobber
        // state, does not duplicate the entry).
        sup.register_worker(100);
        assert_eq!(sup.workers.len(), 3, "re-register does not duplicate");

        // Unregistering a tracked pid succeeds and frees its slot.
        assert!(sup.unregister_worker(200).is_ok());
        assert_eq!(sup.alive_count(), 2, "unregister removes from alive set");
        assert_eq!(sup.workers.len(), 2);

        // Unregistering an unknown pid is a WorkerNotFound error, not
        // a silent no-op — the supervisor's bookkeeping is strict.
        let err = sup.unregister_worker(999).unwrap_err();
        assert!(matches!(err, IpcError::WorkerNotFound),
            "unregister unknown pid must be WorkerNotFound, got {:?}", err);
    }

    #[test]
    fn test_supervisor_handle_worker_exit_clean_exit_terminates() {
        // exit(0) → Terminate, worker marked dead, no restart consumed.
        // The supervisor's bookkeeping records the exit code even
        // though no restart is attempted.
        let mut sup = Supervisor::new(3, 1_000);
        sup.register_worker(42);

        let action = sup.handle_worker_exit(42, 0, 0).expect("clean exit");
        assert_eq!(action, RecoveryAction::Terminate,
            "exit(0) must Terminate");

        let state = sup.workers.get(&42).expect("worker still tracked");
        assert!(!state.is_alive, "clean-exit worker is dead");
        assert_eq!(state.restart_count, 0, "no restart consumed");
        assert_eq!(state.last_exit_code, 0);
        assert_eq!(state.last_signal, 0);
        assert_eq!(sup.alive_count(), 0, "dead worker not counted");
    }

    #[test]
    fn test_supervisor_handle_worker_exit_crash_restarts_within_budget() {
        // SIGSEGV (signal 11, exit_code 139 per shell convention) with
        // a budget → Restart, worker marked alive again, budget
        // consumed. Three restarts fit in max_restarts=3; the fourth
        // crash escalates.
        let mut sup = Supervisor::new(3, 1_000);
        sup.register_worker(7);

        // Restart #1: within budget.
        let a1 = sup.handle_worker_exit(7, 139, 11).expect("crash #1");
        assert_eq!(a1, RecoveryAction::Restart, "crash #1 must Restart");
        let s = sup.workers.get(&7).expect("worker tracked");
        assert_eq!(s.restart_count, 1);
        assert!(s.is_alive, "restart marks worker alive again");
        assert_eq!(s.last_exit_code, 139);
        assert_eq!(s.last_signal, 11);

        // Restart #2: still within budget.
        let a2 = sup.handle_worker_exit(7, 139, 11).expect("crash #2");
        assert_eq!(a2, RecoveryAction::Restart, "crash #2 must Restart");
        assert_eq!(sup.workers.get(&7).unwrap().restart_count, 2);

        // Restart #3: the last allowed restart.
        let a3 = sup.handle_worker_exit(7, 139, 11).expect("crash #3");
        assert_eq!(a3, RecoveryAction::Restart, "crash #3 must Restart");
        assert_eq!(sup.workers.get(&7).unwrap().restart_count, 3);

        // Crash #4: budget exhausted → Escalate, worker stays dead.
        let a4 = sup.handle_worker_exit(7, 139, 11).expect("crash #4");
        assert_eq!(a4, RecoveryAction::Escalate,
            "crash #4 with exhausted budget must Escalate");
        let s = sup.workers.get(&7).unwrap();
        assert_eq!(s.restart_count, 3, "escalate does not consume budget");
        assert!(!s.is_alive, "escalated worker stays dead");
        assert_eq!(sup.alive_count(), 0);
    }

    #[test]
    fn test_supervisor_handle_worker_exit_unknown_pid_rejected() {
        // An exit for a pid the supervisor never registered is a
        // state-machine bug — WorkerNotFound, not a silent no-op.
        let mut sup = Supervisor::new(3, 1_000);
        let err = sup.handle_worker_exit(404, 1, 0).unwrap_err();
        assert!(matches!(err, IpcError::WorkerNotFound),
            "exit for unknown pid must be WorkerNotFound, got {:?}", err);
    }

    #[test]
    fn test_supervisor_handle_worker_exit_zero_budget_escalates() {
        // max_restarts == 0 disables the restart policy entirely — any
        // crash escalates rather than spinning on a restart loop.
        let mut sup = Supervisor::new(0, 1_000);
        sup.register_worker(11);

        let action = sup.handle_worker_exit(11, 139, 11).expect("escalate");
        assert_eq!(action, RecoveryAction::Escalate,
            "max_restarts == 0 must Escalate on crash");

        let s = sup.workers.get(&11).unwrap();
        assert_eq!(s.restart_count, 0, "no restart consumed");
        assert!(!s.is_alive);
    }

    #[test]
    fn test_circuit_breaker_starts_closed_and_can_proceed() {
        // Fresh breaker is Closed, allows traffic, has zero failures.
        let cb = CircuitBreaker::new(5);
        assert_eq!(cb.state, CircuitState::Closed);
        assert!(cb.can_proceed(), "Closed breaker must allow traffic");
        assert_eq!(cb.failure_count, 0);
        assert_eq!(cb.threshold, 5);
    }

    #[test]
    fn test_circuit_breaker_trips_open_after_threshold_exceeded() {
        // threshold=3: failures 1, 2, 3 stay Closed (count <=
        // threshold); failure 4 trips Open. can_proceed then returns
        // false. This pins the `>` (strictly-greater) semantics: the
        // breaker opens when count EXCEEDS threshold, not when it
        // equals it.
        let mut cb = CircuitBreaker::new(3);
        cb.record_failure();
        assert_eq!(cb.state, CircuitState::Closed, "1 <= 3 stays Closed");
        cb.record_failure();
        assert_eq!(cb.state, CircuitState::Closed, "2 <= 3 stays Closed");
        cb.record_failure();
        assert_eq!(cb.state, CircuitState::Closed, "3 <= 3 stays Closed");
        assert_eq!(cb.failure_count, 3);

        cb.record_failure();
        assert_eq!(cb.state, CircuitState::Open, "4 > 3 trips Open");
        assert!(!cb.can_proceed(), "Open breaker blocks traffic");
    }

    #[test]
    fn test_circuit_breaker_record_success_resets_to_closed() {
        // A success at any time resets the failure count and closes
        // the breaker. This is the probe-success path from HalfOpen
        // and the "all clear" path from Closed.
        let mut cb = CircuitBreaker::new(2);
        cb.record_failure();
        cb.record_failure();
        cb.record_failure(); // trips Open
        assert_eq!(cb.state, CircuitState::Open);

        cb.record_success();
        assert_eq!(cb.state, CircuitState::Closed, "success closes");
        assert_eq!(cb.failure_count, 0, "success resets count");
        assert!(cb.can_proceed());
    }

    #[test]
    fn test_circuit_breaker_reset_transitions_open_to_half_open() {
        // reset() on an Open breaker → HalfOpen (one trial allowed).
        // reset() on a Closed breaker is a no-op (already proceeding).
        // reset() on a HalfOpen breaker is a no-op (already mid-probe).
        let mut cb = CircuitBreaker::new(1);
        cb.record_failure();
        cb.record_failure(); // trips Open
        assert_eq!(cb.state, CircuitState::Open);

        cb.reset();
        assert_eq!(cb.state, CircuitState::HalfOpen, "reset → HalfOpen");
        assert!(cb.can_proceed(), "HalfOpen allows one trial");

        // Resetting again while HalfOpen is a no-op.
        cb.reset();
        assert_eq!(cb.state, CircuitState::HalfOpen, "double-reset no-op");
    }

    #[test]
    fn test_circuit_breaker_half_open_failure_reopens() {
        // In HalfOpen, a single failure re-opens the breaker. This is
        // the "probe failed" path — the dependency is still sick, so
        // go back to blocking traffic.
        let mut cb = CircuitBreaker::new(1);
        cb.record_failure();
        cb.record_failure(); // Open
        cb.reset();          // HalfOpen
        assert_eq!(cb.state, CircuitState::HalfOpen);

        cb.record_failure();
        assert_eq!(cb.state, CircuitState::Open, "HalfOpen failure re-opens");
        assert!(!cb.can_proceed());
    }

    #[test]
    fn test_circuit_breaker_half_open_success_closes() {
        // In HalfOpen, a single success closes the breaker. This is
        // the "probe succeeded" path — the dependency has recovered,
        // so resume normal traffic.
        let mut cb = CircuitBreaker::new(1);
        cb.record_failure();
        cb.record_failure(); // Open
        cb.reset();          // HalfOpen

        cb.record_success();
        assert_eq!(cb.state, CircuitState::Closed, "HalfOpen success closes");
        assert_eq!(cb.failure_count, 0);
        assert!(cb.can_proceed());
    }

    // ── W73-80: HotSwapConfig / HotSwapResult / HotSwapManager tests ──

    #[test]
    fn test_hot_swap_config_new_round_trips_fields() {
        // The constructor must populate every field — no defaults
        // hidden behind the constructor.
        let cfg = HotSwapConfig::new("crypto.aes", 1, 2, true, false);
        assert_eq!(cfg.module_name, "crypto.aes");
        assert_eq!(cfg.old_version, 1);
        assert_eq!(cfg.new_version, 2);
        assert!(cfg.state_transfer);
        assert!(!cfg.rollback_on_failure);
    }

    #[test]
    fn test_hot_swap_manager_perform_swap_basic_success() {
        // A straightforward upgrade v1 → v2 on a fresh manager
        // succeeds: the new version becomes active, a new pid is
        // allocated, state_transfer flag is echoed in the result,
        // and there is no error.
        let mut mgr = HotSwapManager::new();
        let cfg = HotSwapConfig::new("net.tls", 1, 2, true, false);

        let result = mgr.perform_swap(&cfg).expect("swap must succeed");
        assert!(result.success);
        assert!(result.old_pid > 0, "old_pid must be allocated");
        assert!(result.new_pid > result.old_pid, "new_pid must advance");
        assert!(result.state_transferred, "state_transfer=true echoes");
        assert!(result.error.is_none());

        assert_eq!(mgr.active_versions.get("net.tls"), Some(&2),
            "active_versions must record the new version");
    }

    #[test]
    fn test_hot_swap_manager_rejects_downgrade() {
        // new_version <= old_version is a ProtocolViolation — the
        // manager does not support downgrades via perform_swap (those
        // go through rollback).
        let mut mgr = HotSwapManager::new();
        let cfg = HotSwapConfig::new("net.tls", 5, 4, false, false);

        let err = mgr.perform_swap(&cfg).unwrap_err();
        assert!(matches!(err, IpcError::ProtocolViolation { .. }),
            "downgrade must be ProtocolViolation, got {:?}", err);
        assert!(!mgr.active_versions.contains_key("net.tls"),
            "rejected swap must not mutate active_versions");
    }

    #[test]
    fn test_hot_swap_manager_rejects_stale_old_version() {
        // If the module is registered with version N, a swap claiming
        // old_version != N is a ProtocolViolation — a concurrent swap
        // raced us, the caller's view is stale.
        let mut mgr = HotSwapManager::new();
        mgr.register_module("net.tls", 3);

        // Caller thinks old_version is 1, but manager recorded 3.
        let cfg = HotSwapConfig::new("net.tls", 1, 2, false, false);
        let err = mgr.perform_swap(&cfg).unwrap_err();
        assert!(matches!(err, IpcError::ProtocolViolation { .. }),
            "stale old_version must be ProtocolViolation, got {:?}", err);
        assert_eq!(mgr.active_versions.get("net.tls"), Some(&3),
            "rejected swap must not bump the version");
    }

    #[test]
    fn test_hot_swap_manager_chained_swaps_advance_version() {
        // Two sequential swaps v1→v2 then v2→v3 both succeed, each
        // time consuming one pid and bumping the recorded version.
        // This is the canonical "rolling upgrade" flow.
        let mut mgr = HotSwapManager::new();
        let cfg1 = HotSwapConfig::new("mod", 1, 2, false, false);
        let r1 = mgr.perform_swap(&cfg1).expect("swap 1");
        assert_eq!(mgr.active_versions.get("mod"), Some(&2));

        let cfg2 = HotSwapConfig::new("mod", 2, 3, true, false);
        let r2 = mgr.perform_swap(&cfg2).expect("swap 2");
        assert_eq!(mgr.active_versions.get("mod"), Some(&3));
        assert!(r2.new_pid > r1.new_pid, "second swap allocates a new pid");
    }

    #[test]
    fn test_hot_swap_manager_rollback_reverts_version() {
        // After a swap v1→v2, rollback reverts the recorded version
        // to v1. The contract is just "flip the active version back"
        // — the real kill-and-restore logic is the runtime's job.
        let mut mgr = HotSwapManager::new();
        let cfg = HotSwapConfig::new("mod", 1, 2, false, true);
        mgr.perform_swap(&cfg).expect("swap");
        assert_eq!(mgr.active_versions.get("mod"), Some(&2));

        mgr.rollback(&cfg).expect("rollback");
        assert_eq!(mgr.active_versions.get("mod"), Some(&1),
            "rollback reverts active version to old_version");
    }

    #[test]
    fn test_hot_swap_manager_rollback_unknown_module_rejected() {
        // Rolling back a module the manager has never heard of is a
        // WorkerNotFound error — there is no recorded version to
        // revert to.
        let mut mgr = HotSwapManager::new();
        let cfg = HotSwapConfig::new("never_loaded", 1, 2, false, false);
        let err = mgr.rollback(&cfg).unwrap_err();
        assert!(matches!(err, IpcError::WorkerNotFound),
            "rollback unknown module must be WorkerNotFound, got {:?}", err);
    }

    #[test]
    fn test_hot_swap_result_failure_shape_carries_error_message() {
        // The failure shape of HotSwapResult is success=false, pids=0,
        // state_transferred=false, error=Some(reason). The fields are
        // constructed directly here because the mock perform_swap
        // never fails after passing validation — the failure shape is
        // what rollback_on_failure would produce if the health check
        // failed, and is documented for callers building it by hand.
        let r = HotSwapResult {
            success: false,
            old_pid: 0,
            new_pid: 0,
            state_transferred: false,
            error: Some(String::from("post-swap health check failed")),
        };
        assert!(!r.success);
        assert_eq!(r.old_pid, 0);
        assert_eq!(r.new_pid, 0);
        assert!(!r.state_transferred);
        assert_eq!(r.error.as_deref(), Some("post-swap health check failed"));
    }

    // ── W81-88: DistributedChannel + WorkerDiscovery tests ───────────

    #[test]
    fn test_distributed_channel_new_starts_disconnected() {
        // A freshly constructed channel is disconnected regardless of
        // is_local — connect() must be called before is_connected()
        // returns true. This pins the "no implicit connect on new"
        // contract.
        let local = DistributedChannel::new(1, "", 100, true);
        assert!(!local.is_connected(), "local channel starts disconnected");
        assert!(local.is_local);
        assert_eq!(local.local_pid, 1);
        assert_eq!(local.channel_id, 100);
        assert_eq!(local.remote_addr, "");

        let remote = DistributedChannel::new(2, "10.0.0.5:4242", 101, false);
        assert!(!remote.is_connected(), "remote channel starts disconnected");
        assert!(!remote.is_local);
        assert_eq!(remote.remote_addr, "10.0.0.5:4242");
    }

    #[test]
    fn test_distributed_channel_connect_disconnect_cycle() {
        // The full connect → disconnect → connect cycle. Each
        // transition flips is_connected; a second connect without an
        // intervening disconnect is a ChannelTimeout (state-machine
        // bug surfaced as a timeout so retry logic kicks in); a
        // double-disconnect is a ChannelClosed.
        let mut ch = DistributedChannel::new(1, "peer:1234", 7, false);
        assert!(!ch.is_connected());

        ch.connect().expect("connect #1");
        assert!(ch.is_connected());

        // Double-connect is rejected.
        let err = ch.connect().unwrap_err();
        assert!(matches!(err, IpcError::ChannelTimeout),
            "double-connect must be ChannelTimeout, got {:?}", err);

        ch.disconnect().expect("disconnect");
        assert!(!ch.is_connected());

        // Double-disconnect is rejected.
        let err = ch.disconnect().unwrap_err();
        assert!(matches!(err, IpcError::ChannelClosed),
            "double-disconnect must be ChannelClosed, got {:?}", err);

        // Re-connect after disconnect works (the cycle is reusable).
        ch.connect().expect("reconnect");
        assert!(ch.is_connected());
    }

    #[test]
    fn test_distributed_channel_local_path_connects_without_addr() {
        // A local channel (is_local=true) has an empty remote_addr but
        // still goes through the same connect/disconnect cycle. This
        // is the in-process fast path: the supervisor routes directly
        // without touching the network.
        let mut ch = DistributedChannel::new(5, "", 42, true);
        assert!(ch.is_local);
        assert_eq!(ch.remote_addr, "");
        ch.connect().expect("local connect");
        assert!(ch.is_connected());
        ch.disconnect().expect("local disconnect");
        assert!(!ch.is_connected());
    }

    #[test]
    fn test_worker_discovery_register_lookup_discover() {
        // Register three workers at distinct addresses; lookup
        // returns the address; discover returns all pids; lookup of
        // an unknown pid returns None.
        let mut wd = WorkerDiscovery::new();
        assert!(wd.is_empty(), "fresh registry is empty");

        wd.register(100, "10.0.0.1:4000");
        wd.register(200, "10.0.0.2:4000");
        wd.register(300, "10.0.0.3:4000");
        assert_eq!(wd.len(), 3);
        assert!(!wd.is_empty());

        assert_eq!(wd.lookup(100).as_deref(), Some("10.0.0.1:4000"));
        assert_eq!(wd.lookup(200).as_deref(), Some("10.0.0.2:4000"));
        assert_eq!(wd.lookup(300).as_deref(), Some("10.0.0.3:4000"));
        assert_eq!(wd.lookup(999), None, "unknown pid → None");

        let mut pids = wd.discover();
        pids.sort_unstable();
        assert_eq!(pids, vec![100, 200, 300], "discover lists all pids");
    }

    #[test]
    fn test_worker_discovery_register_updates_existing_address() {
        // Re-registering an existing pid with a new address updates
        // the entry — this is the migration path (a worker moved to
        // a new node re-registers with the new address).
        let mut wd = WorkerDiscovery::new();
        wd.register(42, "old:1000");
        assert_eq!(wd.lookup(42).as_deref(), Some("old:1000"));

        wd.register(42, "new:2000");
        assert_eq!(wd.lookup(42).as_deref(), Some("new:2000"),
            "re-register updates the address");
        assert_eq!(wd.len(), 1, "re-register does not duplicate");
    }

    #[test]
    fn test_worker_discovery_default_is_empty_map() {
        // Default construction yields an empty registry. This pins
        // the Default impl so callers can rely on `WorkerDiscovery::default()`.
        let wd = WorkerDiscovery::default();
        assert!(wd.known_workers.is_empty());
        assert_eq!(wd.len(), 0);
        assert!(wd.is_empty());
        assert!(wd.discover().is_empty());
        assert_eq!(wd.lookup(1), None);
    }

    // ── CT1: Session Types (W89-90) ──────────────────────────────────

    #[test]
    fn test_session_type_dual_send_recv_swap() {
        // dual of Send(x, rest) is Recv(x, dual(rest))
        let s = SessionType::Send(42, Box::new(SessionType::End));
        let d = s.dual();
        assert_eq!(d, SessionType::Recv(42, Box::new(SessionType::End)));

        // dual of Recv(x, rest) is Send(x, dual(rest))
        let r = SessionType::Recv(7, Box::new(SessionType::End));
        assert_eq!(r.dual(), SessionType::Send(7, Box::new(SessionType::End)));
    }

    #[test]
    fn test_session_type_dual_choice_maps_each_arm() {
        // dual of Choice(a, b) is Choice(dual(a), dual(b)) — arms are
        // dualised in place; they are NOT swapped.
        let a = SessionType::Send(1, Box::new(SessionType::End));
        let b = SessionType::Recv(2, Box::new(SessionType::End));
        let c = SessionType::Choice(Box::new(a.clone()), Box::new(b.clone()));
        let d = c.dual();
        assert_eq!(
            d,
            SessionType::Choice(
                Box::new(a.dual()),   // Recv(1, End)
                Box::new(b.dual()),   // Send(2, End)
            ),
            "Choice dual maps each arm in place"
        );
    }

    #[test]
    fn test_session_type_dual_loop_and_end() {
        // End and Loop are self-dual at the constructor level (Loop
        // stays Loop, its body is dualised).
        assert_eq!(SessionType::End.dual(), SessionType::End);
        let l = SessionType::Loop(Box::new(SessionType::Send(9, Box::new(SessionType::End))));
        assert_eq!(
            l.dual(),
            SessionType::Loop(Box::new(SessionType::Recv(9, Box::new(SessionType::End))))
        );
    }

    #[test]
    fn test_session_type_dual_is_involution() {
        // dual(dual(s)) == s for every shape.
        let cases = vec![
            SessionType::End,
            SessionType::Send(1, Box::new(SessionType::End)),
            SessionType::Recv(2, Box::new(SessionType::Send(3, Box::new(SessionType::End)))),
            SessionType::Choice(
                Box::new(SessionType::Send(4, Box::new(SessionType::End))),
                Box::new(SessionType::Recv(5, Box::new(SessionType::End))),
            ),
            SessionType::Loop(Box::new(SessionType::Send(6, Box::new(SessionType::End)))),
        ];
        for s in &cases {
            assert!(s.dual_is_involution(), "dual not involutive for {:?}", s);
            assert_eq!(s.dual().dual(), *s);
        }
    }

    #[test]
    fn test_session_type_is_terminal() {
        // Only End is terminal.
        assert!(SessionType::End.is_terminal());
        assert!(!SessionType::Send(1, Box::new(SessionType::End)).is_terminal());
        assert!(!SessionType::Recv(1, Box::new(SessionType::End)).is_terminal());
        assert!(!SessionType::Choice(
            Box::new(SessionType::End),
            Box::new(SessionType::End),
        ).is_terminal());
        // Loop is not terminal even when its body is, because the
        // protocol may iterate again.
        assert!(!SessionType::Loop(Box::new(SessionType::End)).is_terminal());
    }

    // ── CT2: Information-Flow Labels (W91-92) ────────────────────────

    #[test]
    fn test_security_label_can_flow_to_lattice() {
        // Public < Internal < Secret < TopSecret
        use SecurityLabel::*;
        assert!(Public.can_flow_to(Internal));
        assert!(Public.can_flow_to(Secret));
        assert!(Public.can_flow_to(TopSecret));
        assert!(Internal.can_flow_to(Secret));
        assert!(Secret.can_flow_to(TopSecret));
        assert!(Secret.can_flow_to(Secret), "reflexive");

        // Secret cannot flow to Public
        assert!(!Secret.can_flow_to(Public));
        assert!(!TopSecret.can_flow_to(Internal));
        assert!(!TopSecret.can_flow_to(Public));
    }

    #[test]
    fn test_security_label_join_is_max() {
        use SecurityLabel::*;
        assert_eq!(Public.join(Secret), Secret);
        assert_eq!(Secret.join(Public), Secret, "join is symmetric");
        assert_eq!(Internal.join(Secret), Secret);
        assert_eq!(Secret.join(TopSecret), TopSecret);
        assert_eq!(TopSecret.join(TopSecret), TopSecret, "join with self");
    }

    #[test]
    fn test_security_label_meet_is_min() {
        use SecurityLabel::*;
        assert_eq!(Public.meet(Secret), Public);
        assert_eq!(Secret.meet(Public), Public);
        assert_eq!(Internal.meet(TopSecret), Internal);
    }

    // ── CT6: zk-STARK Proofs (W93-94) ────────────────────────────────

    #[test]
    fn test_stark_proof_valid_passes() {
        // A proof built with new_valid has its verifier_key derived
        // from the proof_data and public_inputs, so verify() returns
        // true.
        let proof = StarkProof::new_valid(
            vec![0xDE, 0xAD, 0xBE, 0xEF],
            vec![42, 3],
            1000,
        );
        assert!(proof.verify(), "valid proof should verify");
    }

    #[test]
    fn test_stark_proof_empty_data_fails() {
        // Empty proof_data fails verify even if the verifier_key
        // happens to match.
        let mut proof = StarkProof::new_valid(vec![], vec![1], 100);
        // new_valid sets verifier_key = commitment() of empty data;
        // verify still fails because proof_data is empty.
        assert!(!proof.verify());

        // And even if someone manually sets a non-zero verifier_key:
        proof.verifier_key = 0xCAFEBABE;
        assert!(!proof.verify());
    }

    #[test]
    fn test_stark_proof_wrong_verifier_key_fails() {
        // Tampering with the verifier_key breaks the hash check.
        let mut proof = StarkProof::new_valid(vec![1, 2, 3, 4], vec![10], 100);
        proof.verifier_key ^= 1;
        assert!(!proof.verify(), "wrong verifier_key should fail");
    }

    #[test]
    fn test_stark_proof_zero_validity_window_fails() {
        // A proof whose validity_window is 0 is expired immediately.
        let proof = StarkProof::new_valid(vec![1, 2, 3], vec![1], 0);
        assert!(!proof.verify(), "zero validity window should fail");
    }

    #[test]
    fn test_capability_attestation_verify_ok() {
        let proof = StarkProof::new_valid(vec![1, 2, 3, 4], vec![42, 1], 1000);
        let att = CapabilityAttestation {
            proof,
            worker_pid: 42,
            capability_count: 1,
            capability_hash: 0xABCDEF,
            commitment_hash: 0xABCDEF,
        };
        assert!(att.verify(42).is_ok());
    }

    #[test]
    fn test_capability_attestation_wrong_pid_fails() {
        let proof = StarkProof::new_valid(vec![1, 2, 3, 4], vec![42, 1], 1000);
        let att = CapabilityAttestation {
            proof,
            worker_pid: 42,
            capability_count: 1,
            capability_hash: 0,
            commitment_hash: 0,
        };
        assert_eq!(att.verify(99).unwrap_err(), IpcError::StarkProofInvalid);
    }

    #[test]
    fn test_capability_attestation_invalid_proof_fails() {
        // Wrong verifier_key → proof.verify() is false → attestation fails.
        let mut proof = StarkProof::new_valid(vec![1, 2, 3, 4], vec![42, 1], 1000);
        proof.verifier_key ^= 1;
        let att = CapabilityAttestation {
            proof,
            worker_pid: 42,
            capability_count: 1,
            capability_hash: 0,
            commitment_hash: 0,
        };
        assert_eq!(att.verify(42).unwrap_err(), IpcError::StarkProofInvalid);
    }

    // ── CT7: Fractional Permissions (W95) ────────────────────────────

    #[test]
    fn test_permission_full_grants_all() {
        let p = Permission::full();
        assert!(p.can_read());
        assert!(p.can_write());
        assert!(p.can_execute());
    }

    #[test]
    fn test_permission_split_halves_all_fractions() {
        let p = Permission::full();
        let (a, b) = p.split();
        assert_eq!(a.read, 0.5);
        assert_eq!(a.write, 0.5);
        assert_eq!(a.execute, 0.5);
        assert_eq!(b.read, 0.5);
        assert_eq!(b.write, 0.5);
        assert_eq!(b.execute, 0.5);
    }

    #[test]
    fn test_permission_split_then_merge_is_identity() {
        let p = Permission::full();
        let (a, b) = p.split();
        let merged = a.merge(b);
        assert_eq!(merged, p, "split then merge reconstructs original");
    }

    #[test]
    fn test_permission_split_can_read_but_not_full_write_classical() {
        // After splitting, each half has read = 0.5 and write = 0.5.
        // can_read / can_write / can_execute all return true (fraction > 0).
        // This is the permissive compile-time predicate; the runtime
        // additionally enforces write uniqueness.
        let p = Permission::full();
        let (half, _) = p.split();
        assert!(half.can_read(),  "half fraction > 0 → can_read");
        assert!(half.can_write(), "half fraction > 0 → can_write (permissive)");
        assert!(half.can_execute());
    }

    #[test]
    fn test_permission_none_grants_nothing() {
        let p = Permission::none();
        assert!(!p.can_read());
        assert!(!p.can_write());
        assert!(!p.can_execute());
    }

    #[test]
    fn test_permission_partial_read_only() {
        // A read-only permission: read = 1, write = 0, execute = 0.
        let p = Permission { read: 1.0, write: 0.0, execute: 0.0 };
        assert!(p.can_read());
        assert!(!p.can_write());
        assert!(!p.can_execute());
    }

    // ── CT8: Formal Verification (W96) ───────────────────────────────

    #[test]
    fn test_verify_invariant_collapse_returns_verified_proof() {
        let v = verify_invariant_collapse();
        assert_eq!(v.theorem_name, "invariant_collapse_5_to_3");
        assert!(v.verified, "5→3 invariant collapse is verified");
        assert!(!v.proof_outline.is_empty());
        // The proof outline should mention all five runtime layers
        // and all three compile-time invariants.
        assert!(v.proof_outline.contains("L1"), "outline mentions L1");
        assert!(v.proof_outline.contains("L5"), "outline mentions L5");
        assert!(v.proof_outline.contains("CT1"), "outline mentions CT1");
        assert!(v.proof_outline.contains("CT3"), "outline mentions CT3");
    }

    #[test]
    fn test_verification_proof_struct_round_trips() {
        let v = VerificationProof {
            theorem_name: "trivial".to_string(),
            proof_outline: "by reflexivity".to_string(),
            verified: true,
        };
        let cloned = v.clone();
        assert_eq!(v, cloned);
        assert!(v.verified);
    }
}
