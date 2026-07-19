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
    let mut buf = Vec::with_capacity(HEADER_SIZE + msg.payload.len() + CRC32_SIZE);
    buf.extend_from_slice(&MAGIC);
    buf.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    buf.extend_from_slice(&msg.header.flags.bits().to_le_bytes());
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

pub fn type_hash_str(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
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

    #[derive(Clone, Debug, Default)]
    pub struct MemoryPermissions {
        pub read: bool,
        pub write: bool,
        pub execute: bool,
    }

    #[derive(Clone, Debug)]
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

    pub const CAPABILITY_TOKEN_SIZE: usize = 96;

    impl CapabilityToken {
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
            while buf.len() < CAPABILITY_TOKEN_SIZE {
                buf.push(0);
            }
            buf
        }

        pub fn decode(bytes: &[u8]) -> Result<Self, String> {
            if bytes.len() < CAPABILITY_TOKEN_SIZE {
                return Err("token too short".into());
            }
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
                resource: Resource::Memory(0, 0), // placeholder
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
}

// ── L3: Memory Windows ───────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct MemoryWindow {
    pub source_pid: u64,
    pub target_pid: u64,
    pub source_addr: u64,
    pub target_addr: u64,
    pub size: u64,
    pub permissions: capability::MemoryPermissions,
    pub capability_id: u128,
    pub revocable: bool,
}

// ── L4: Protocol State Machine ──────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ProtocolState {
    Idle,
    WaitingForSend,
    WaitingForRecv,
    Closed,
}

#[derive(Clone, Debug)]
pub struct ProtocolStateMachine {
    pub state: ProtocolState,
    pub allowed_transitions: HashMap<(ProtocolState, u64), ProtocolState>,
}

impl ProtocolStateMachine {
    pub fn new() -> Self {
        Self {
            state: ProtocolState::Idle,
            allowed_transitions: HashMap::new(),
        }
    }

    pub fn check(&mut self, type_hash: u64) -> Result<ProtocolState, String> {
        let key = (self.state.clone(), type_hash);
        match self.allowed_transitions.get(&key) {
            Some(new_state) => {
                self.state = new_state.clone();
                Ok(new_state.clone())
            }
            None => Err(format!("protocol violation: state={:?} type_hash={}", self.state, type_hash)),
        }
    }
}

// ── L5-L8: Worker, State, Error, Crypto (stubs for now) ────────────

#[derive(Clone, Debug)]
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

/// Create a checkpoint of the current process state.
/// In a real implementation, this uses copy-on-write to snapshot memory.
pub fn checkpoint_state(channels: &[(u64, u64, ProtocolState)]) -> Checkpoint {
    let channel_states: Vec<ChannelState> = channels.iter()
        .map(|(id, seq, state)| ChannelState {
            channel_id: *id,
            sequence: *seq,
            protocol_state: state.clone(),
        })
        .collect();
    let mut hash_input = Vec::new();
    for ch in &channel_states {
        hash_input.extend_from_slice(&ch.channel_id.to_le_bytes());
        hash_input.extend_from_slice(&ch.sequence.to_le_bytes());
    }
    let mut integrity_hash = [0u8; 32];
    for (i, byte) in hash_input.iter().enumerate() {
        integrity_hash[i % 32] ^= byte;
    }
    Checkpoint {
        pid: 0, // would be filled by kernel
        channels: channel_states,
        timestamp: 0, // would be filled by kernel
        integrity_hash,
    }
}

// ── L7: Error Encapsulation (fault containment) ──────────────────────

#[derive(Clone, Debug)]
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
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for IpcError {}

// ── L8: Cryptographic Encapsulation (AEAD) ───────────────────────────

/// ChaCha20-Poly1305 AEAD encryption for IPC messages.
/// In a real implementation, this would use a crypto library.
/// For now, this is a stub that passes through plaintext.
pub struct CryptoState {
    pub key: [u8; 32],
    pub nonce_counter: u64,
}

impl CryptoState {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key, nonce_counter: 0 }
    }

    /// Encrypt plaintext (stub: returns plaintext + fake tag)
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let mut output = Vec::with_capacity(plaintext.len() + 12 + 16);
        // Nonce (12 bytes)
        let nonce = self.nonce_counter.to_le_bytes();
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&[0u8; 4]); // padding
        // Ciphertext (same as plaintext for stub)
        output.extend_from_slice(plaintext);
        // Tag (16 bytes, fake)
        output.extend_from_slice(&[0u8; 16]);
        self.nonce_counter += 1;
        output
    }

    /// Decrypt ciphertext (stub: extracts plaintext)
    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, IpcError> {
        if ciphertext.len() < 28 {
            return Err(IpcError::DeserializationError);
        }
        // Skip nonce (12) and tag (16)
        Ok(ciphertext[12..ciphertext.len()-16].to_vec())
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

        let child = capability::CapabilityToken {
            id: rand_u128(),
            source_pid: parent.target_pid, // delegator becomes source
            target_pid: new_target_pid,
            resource: parent.resource.clone(),
            permissions: subset_perms,
            delegation_depth: parent.delegation_depth + 1,
            created_at: 0, // would be now()
            expires_at: parent.expires_at,
            signature: [0u8; 32], // would be HMAC(signing_key, ...)
        };

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
    fn test_type_hash() {
        let h1 = type_hash_str("i32");
        let h2 = type_hash_str("i32");
        let h3 = type_hash_str("i64");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
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
    }
}
