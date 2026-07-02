//! SCG Serialization / Deserialization
//!
//! This module provides three serialization formats for the Semantic Computation Graph:
//!
//! - **Binary**: A compact, versioned binary format suitable for persistent storage
//!   and network transfer. The format includes a magic header and version number
//!   for forward/backward compatibility.
//! - **JSON**: A human-readable JSON representation using serde, ideal for debugging
//!   and interoperability.
//! - **DOT**: A Graphviz DOT format for visualizing the graph structure.
//!
//! # Binary Format (Version 1)
//!
//! ```text
//! [4B]  Magic: "VSCG"
//! [4B]  Version: u32 LE
//! [8B]  Next node ID counter: u64 LE
//! [8B]  Next edge ID counter: u64 LE
//! [4B]  Node count: u32 LE
//! [4B]  Edge count: u32 LE
//! [4B]  Region count: u32 LE
//! --- Nodes (repeated Node count times) ---
//! --- Edges (repeated Edge count times) ---
//! --- Regions (repeated Region count times) ---
//! ```

use crate::edge::{EdgeData, EdgeId, EdgeKind};
use crate::graph::SCG;
use crate::node::{
    AccessMode, AccessNode, AllocationNode, BDReference, CastNode, ClosureEnvNode,
    ComputationKind, ComputationNode, ConstantTimeNode, ConstantTimeOp, ControlKind, ControlNode,
    DeallocationNode, EffectNode, EnumDefNode, EnumVariantInfo, MatchArmInfo,
    MatchNode, MatchPatternInfo, NodeData, NodeId, NodePayload, NodeType, PhantomNode,
    ProgramPoint, StructDefNode, StructFieldInfo, VTableNode,
};
use crate::region::{DeploymentTarget, RegionId, SCGRegion};

// ── Constants ───────────────────────────────────────────────────────────

/// Magic bytes identifying the VUMA SCG binary format.
const MAGIC: &[u8; 4] = b"VSCG";

/// Current binary format version.
const FORMAT_VERSION: u32 = 1;

/// Minimum supported version for deserialization (backward compat).
const MIN_SUPPORTED_VERSION: u32 = 1;

// ── Tags for enum discriminants ─────────────────────────────────────────

const NODE_TYPE_COMPUTATION: u32 = 0;
const NODE_TYPE_ALLOCATION: u32 = 1;
const NODE_TYPE_DEALLOCATION: u32 = 2;
const NODE_TYPE_ACCESS: u32 = 3;
const NODE_TYPE_CAST: u32 = 4;
const NODE_TYPE_EFFECT: u32 = 5;
const NODE_TYPE_CONTROL: u32 = 6;
const NODE_TYPE_PHANTOM: u32 = 7;
const NODE_TYPE_VTABLE: u32 = 8;
const NODE_TYPE_CLOSURE_ENV: u32 = 9;
const NODE_TYPE_STRUCT_DEF: u32 = 10;
const NODE_TYPE_ENUM_DEF: u32 = 11;
const NODE_TYPE_MATCH: u32 = 12;
const NODE_TYPE_CONSTANT_TIME: u32 = 13;

const EDGE_KIND_DATA_FLOW: u32 = 0;
const EDGE_KIND_CONTROL_FLOW: u32 = 1;
const EDGE_KIND_DERIVATION: u32 = 2;
const EDGE_KIND_ANNOTATION: u32 = 3;
const EDGE_KIND_DISPATCH: u32 = 4;
const EDGE_KIND_CALL: u32 = 5;
const EDGE_KIND_RETURN: u32 = 6;

const ACCESS_MODE_READ: u32 = 0;
const ACCESS_MODE_WRITE: u32 = 1;
const ACCESS_MODE_READ_WRITE: u32 = 2;

const CONTROL_KIND_BRANCH: u32 = 0;
const CONTROL_KIND_LOOP_HEADER: u32 = 1;
const CONTROL_KIND_LOOP_EXIT: u32 = 2;
const CONTROL_KIND_JOIN: u32 = 3;
const CONTROL_KIND_FUNCTION_ENTRY: u32 = 4;
const CONTROL_KIND_FUNCTION_RETURN: u32 = 5;
const CONTROL_KIND_JUMP: u32 = 6;
const CONTROL_KIND_SWITCH: u32 = 7;
const CONTROL_KIND_SWITCH_CASE: u32 = 8;
const CONTROL_KIND_CLOSURE_ENTRY: u32 = 9;
const CONTROL_KIND_CLOSURE_RETURN: u32 = 10;
const CONTROL_KIND_FUTURE_POLL: u32 = 11;
const CONTROL_KIND_WAKER_REGISTRATION: u32 = 12;
const CONTROL_KIND_STATE_TRANSITION: u32 = 13;

const DEPLOYMENT_TARGET_HEAP: u32 = 0;
const DEPLOYMENT_TARGET_STACK: u32 = 1;
const DEPLOYMENT_TARGET_GPU: u32 = 2;
const DEPLOYMENT_TARGET_SHARED: u32 = 3;
const DEPLOYMENT_TARGET_PERSISTED: u32 = 4;
const DEPLOYMENT_TARGET_CUSTOM: u32 = 5;

// ── Error type ──────────────────────────────────────────────────────────

/// Errors that can occur during SCG deserialization.
#[derive(Debug, Clone, PartialEq)]
pub enum DeserializeError {
    /// The data does not start with the expected magic bytes.
    InvalidMagic {
        /// The expected magic bytes.
        expected: Vec<u8>,
        /// The magic bytes actually found.
        found: Vec<u8>,
    },
    /// The format version is not supported by this implementation.
    UnsupportedVersion {
        /// The version number found in the data.
        version: u32,
        /// The minimum version supported by this implementation.
        min_supported: u32,
    },
    /// An unexpected end of input was reached.
    UnexpectedEof {
        /// Description of what was being read when EOF was encountered.
        context: String,
    },
    /// A value was out of the valid range for its type.
    InvalidValue {
        /// Name of the field that had the invalid value.
        field: String,
        /// String representation of the invalid value.
        value: String,
    },
    /// A string could not be decoded as valid UTF-8.
    InvalidUtf8 {
        /// Description of what was being read when the invalid UTF-8 was encountered.
        context: String,
        /// The underlying UTF-8 error message.
        source: String,
    },
    /// An I/O error occurred during deserialization.
    IoError(String),
    /// A JSON serialization/deserialization error.
    JsonError(String),
    /// An internal consistency error in the serialized data.
    ConsistencyError(String),
}

impl std::fmt::Display for DeserializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeserializeError::InvalidMagic { expected, found } => {
                write!(
                    f,
                    "invalid magic: expected {:?}, found {:?}",
                    expected, found
                )
            }
            DeserializeError::UnsupportedVersion {
                version,
                min_supported,
            } => {
                write!(
                    f,
                    "unsupported format version {} (minimum supported: {})",
                    version, min_supported
                )
            }
            DeserializeError::UnexpectedEof { context } => {
                write!(f, "unexpected end of input: {}", context)
            }
            DeserializeError::InvalidValue { field, value } => {
                write!(f, "invalid value for '{}': {}", field, value)
            }
            DeserializeError::InvalidUtf8 { context, source } => {
                write!(f, "invalid UTF-8 in '{}': {}", context, source)
            }
            DeserializeError::IoError(msg) => write!(f, "I/O error: {}", msg),
            DeserializeError::JsonError(msg) => write!(f, "JSON error: {}", msg),
            DeserializeError::ConsistencyError(msg) => write!(f, "consistency error: {}", msg),
        }
    }
}

impl std::error::Error for DeserializeError {}

impl From<serde_json::Error> for DeserializeError {
    fn from(e: serde_json::Error) -> Self {
        DeserializeError::JsonError(e.to_string())
    }
}

// ── Binary Reader ───────────────────────────────────────────────────────

/// Helper for reading structured binary data from a byte slice.
struct BinaryReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BinaryReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    #[allow(dead_code)] // part of BinaryReader API for future serialization needs
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn read_bytes(&mut self, n: usize, context: &str) -> Result<&'a [u8], DeserializeError> {
        if self.pos + n > self.data.len() {
            return Err(DeserializeError::UnexpectedEof {
                context: context.to_string(),
            });
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn read_u8(&mut self, context: &str) -> Result<u8, DeserializeError> {
        let bytes = self.read_bytes(1, context)?;
        Ok(bytes[0])
    }

    fn read_bool(&mut self, context: &str) -> Result<bool, DeserializeError> {
        let val = self.read_u8(context)?;
        Ok(val != 0)
    }

    fn read_u32_le(&mut self, context: &str) -> Result<u32, DeserializeError> {
        let bytes = self.read_bytes(4, context)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn read_u64_le(&mut self, context: &str) -> Result<u64, DeserializeError> {
        let bytes = self.read_bytes(8, context)?;
        Ok(u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn read_string(&mut self, context: &str) -> Result<String, DeserializeError> {
        let len = self.read_u32_le(&format!("{}.length", context))? as usize;
        if len == 0 {
            return Ok(String::new());
        }
        let bytes = self.read_bytes(len, context)?;
        std::str::from_utf8(bytes)
            .map(|s| s.to_string())
            .map_err(|e| DeserializeError::InvalidUtf8 {
                context: context.to_string(),
                source: e.to_string(),
            })
    }

    fn read_optional_string(&mut self, context: &str) -> Result<Option<String>, DeserializeError> {
        let has = self.read_bool(&format!("{}.has", context))?;
        if has {
            self.read_string(context).map(Some)
        } else {
            Ok(None)
        }
    }

    fn read_optional_u64(&mut self, context: &str) -> Result<Option<u64>, DeserializeError> {
        let has = self.read_bool(&format!("{}.has", context))?;
        if has {
            self.read_u64_le(context).map(Some)
        } else {
            Ok(None)
        }
    }
}

// ── Binary Writer ───────────────────────────────────────────────────────

/// Helper for writing structured binary data into a byte buffer.
struct BinaryWriter {
    buf: Vec<u8>,
}

impl BinaryWriter {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(256),
        }
    }

    fn write_bytes(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    #[allow(dead_code)] // part of BinaryWriter API for future serialization needs
    fn write_u8(&mut self, val: u8) {
        self.buf.push(val);
    }

    fn write_bool(&mut self, val: bool) {
        self.buf.push(if val { 1 } else { 0 });
    }

    fn write_u32_le(&mut self, val: u32) {
        self.buf.extend_from_slice(&val.to_le_bytes());
    }

    fn write_u64_le(&mut self, val: u64) {
        self.buf.extend_from_slice(&val.to_le_bytes());
    }

    fn write_string(&mut self, s: &str) {
        let bytes = s.as_bytes();
        self.write_u32_le(bytes.len() as u32);
        self.buf.extend_from_slice(bytes);
    }

    fn write_optional_string(&mut self, opt: &Option<String>) {
        match opt {
            Some(s) => {
                self.write_bool(true);
                self.write_string(s);
            }
            None => {
                self.write_bool(false);
            }
        }
    }

    fn write_optional_u64(&mut self, opt: &Option<u64>) {
        match opt {
            Some(v) => {
                self.write_bool(true);
                self.write_u64_le(*v);
            }
            None => {
                self.write_bool(false);
            }
        }
    }

    fn into_vec(self) -> Vec<u8> {
        self.buf
    }
}

// ── Tag conversions ─────────────────────────────────────────────────────

fn node_type_to_tag(nt: &NodeType) -> u32 {
    match nt {
        NodeType::Computation => NODE_TYPE_COMPUTATION,
        NodeType::Allocation => NODE_TYPE_ALLOCATION,
        NodeType::Deallocation => NODE_TYPE_DEALLOCATION,
        NodeType::Access => NODE_TYPE_ACCESS,
        NodeType::Cast => NODE_TYPE_CAST,
        NodeType::Effect => NODE_TYPE_EFFECT,
        NodeType::Control => NODE_TYPE_CONTROL,
        NodeType::Phantom => NODE_TYPE_PHANTOM,
        NodeType::VTable => NODE_TYPE_VTABLE,
        NodeType::ClosureEnv => NODE_TYPE_CLOSURE_ENV,
        NodeType::StructDef => NODE_TYPE_STRUCT_DEF,
        NodeType::EnumDef => NODE_TYPE_ENUM_DEF,
        NodeType::Match => NODE_TYPE_MATCH,
        NodeType::ConstantTime => NODE_TYPE_CONSTANT_TIME,
    }
}

fn tag_to_node_type(tag: u32) -> Result<NodeType, DeserializeError> {
    match tag {
        NODE_TYPE_COMPUTATION => Ok(NodeType::Computation),
        NODE_TYPE_ALLOCATION => Ok(NodeType::Allocation),
        NODE_TYPE_DEALLOCATION => Ok(NodeType::Deallocation),
        NODE_TYPE_ACCESS => Ok(NodeType::Access),
        NODE_TYPE_CAST => Ok(NodeType::Cast),
        NODE_TYPE_EFFECT => Ok(NodeType::Effect),
        NODE_TYPE_CONTROL => Ok(NodeType::Control),
        NODE_TYPE_PHANTOM => Ok(NodeType::Phantom),
        NODE_TYPE_VTABLE => Ok(NodeType::VTable),
        NODE_TYPE_CLOSURE_ENV => Ok(NodeType::ClosureEnv),
        NODE_TYPE_STRUCT_DEF => Ok(NodeType::StructDef),
        NODE_TYPE_ENUM_DEF => Ok(NodeType::EnumDef),
        NODE_TYPE_MATCH => Ok(NodeType::Match),
        NODE_TYPE_CONSTANT_TIME => Ok(NodeType::ConstantTime),
        _ => Err(DeserializeError::InvalidValue {
            field: "NodeType".to_string(),
            value: format!("{}", tag),
        }),
    }
}

fn edge_kind_to_tag(ek: &EdgeKind) -> u32 {
    match ek {
        EdgeKind::DataFlow => EDGE_KIND_DATA_FLOW,
        EdgeKind::ControlFlow => EDGE_KIND_CONTROL_FLOW,
        EdgeKind::Derivation => EDGE_KIND_DERIVATION,
        EdgeKind::Annotation => EDGE_KIND_ANNOTATION,
        EdgeKind::Dispatch => EDGE_KIND_DISPATCH,
        EdgeKind::Call { .. } => EDGE_KIND_CALL,
        EdgeKind::Return { .. } => EDGE_KIND_RETURN,
    }
}

fn tag_to_edge_kind(tag: u32) -> Result<EdgeKind, DeserializeError> {
    match tag {
        EDGE_KIND_DATA_FLOW => Ok(EdgeKind::DataFlow),
        EDGE_KIND_CONTROL_FLOW => Ok(EdgeKind::ControlFlow),
        EDGE_KIND_DERIVATION => Ok(EdgeKind::Derivation),
        EDGE_KIND_ANNOTATION => Ok(EdgeKind::Annotation),
        EDGE_KIND_DISPATCH => Ok(EdgeKind::Dispatch),
        // Note: Call/Return edges carry extra data that cannot be fully
        // round-tripped through the simple tag serialization. We deserialize
        // them as best-effort with default inner NodeId values. For full
        // fidelity, use the JSON/serde path instead.
        EDGE_KIND_CALL => Ok(EdgeKind::Call {
            from_node: crate::node::NodeId::new(0),
            to_node: crate::node::NodeId::new(0),
            caller_region: crate::region::RegionId::new(0),
        }),
        EDGE_KIND_RETURN => Ok(EdgeKind::Return {
            from_node: crate::node::NodeId::new(0),
            to_node: crate::node::NodeId::new(0),
            return_values: vec![],
        }),
        _ => Err(DeserializeError::InvalidValue {
            field: "EdgeKind".to_string(),
            value: format!("{}", tag),
        }),
    }
}

fn access_mode_to_tag(am: &AccessMode) -> u32 {
    match am {
        AccessMode::Read => ACCESS_MODE_READ,
        AccessMode::Write => ACCESS_MODE_WRITE,
        AccessMode::ReadWrite => ACCESS_MODE_READ_WRITE,
    }
}

fn tag_to_access_mode(tag: u32) -> Result<AccessMode, DeserializeError> {
    match tag {
        ACCESS_MODE_READ => Ok(AccessMode::Read),
        ACCESS_MODE_WRITE => Ok(AccessMode::Write),
        ACCESS_MODE_READ_WRITE => Ok(AccessMode::ReadWrite),
        _ => Err(DeserializeError::InvalidValue {
            field: "AccessMode".to_string(),
            value: format!("{}", tag),
        }),
    }
}

fn control_kind_to_tag(ck: &ControlKind) -> u32 {
    match ck {
        ControlKind::Branch => CONTROL_KIND_BRANCH,
        ControlKind::LoopHeader => CONTROL_KIND_LOOP_HEADER,
        ControlKind::LoopExit => CONTROL_KIND_LOOP_EXIT,
        ControlKind::Join => CONTROL_KIND_JOIN,
        ControlKind::FunctionEntry => CONTROL_KIND_FUNCTION_ENTRY,
        ControlKind::FunctionReturn => CONTROL_KIND_FUNCTION_RETURN,
        ControlKind::Jump => CONTROL_KIND_JUMP,
        ControlKind::Switch => CONTROL_KIND_SWITCH,
        ControlKind::SwitchCase => CONTROL_KIND_SWITCH_CASE,
        ControlKind::ClosureEntry => CONTROL_KIND_CLOSURE_ENTRY,
        ControlKind::ClosureReturn => CONTROL_KIND_CLOSURE_RETURN,
        ControlKind::FuturePoll => CONTROL_KIND_FUTURE_POLL,
        ControlKind::WakerRegistration => CONTROL_KIND_WAKER_REGISTRATION,
        ControlKind::StateTransition => CONTROL_KIND_STATE_TRANSITION,
    }
}

fn tag_to_control_kind(tag: u32) -> Result<ControlKind, DeserializeError> {
    match tag {
        CONTROL_KIND_BRANCH => Ok(ControlKind::Branch),
        CONTROL_KIND_LOOP_HEADER => Ok(ControlKind::LoopHeader),
        CONTROL_KIND_LOOP_EXIT => Ok(ControlKind::LoopExit),
        CONTROL_KIND_JOIN => Ok(ControlKind::Join),
        CONTROL_KIND_FUNCTION_ENTRY => Ok(ControlKind::FunctionEntry),
        CONTROL_KIND_FUNCTION_RETURN => Ok(ControlKind::FunctionReturn),
        CONTROL_KIND_JUMP => Ok(ControlKind::Jump),
        CONTROL_KIND_SWITCH => Ok(ControlKind::Switch),
        CONTROL_KIND_SWITCH_CASE => Ok(ControlKind::SwitchCase),
        CONTROL_KIND_CLOSURE_ENTRY => Ok(ControlKind::ClosureEntry),
        CONTROL_KIND_CLOSURE_RETURN => Ok(ControlKind::ClosureReturn),
        CONTROL_KIND_FUTURE_POLL => Ok(ControlKind::FuturePoll),
        CONTROL_KIND_WAKER_REGISTRATION => Ok(ControlKind::WakerRegistration),
        CONTROL_KIND_STATE_TRANSITION => Ok(ControlKind::StateTransition),
        _ => Err(DeserializeError::InvalidValue {
            field: "ControlKind".to_string(),
            value: format!("{}", tag),
        }),
    }
}

fn deployment_target_to_tag(dt: &DeploymentTarget) -> u32 {
    match dt {
        DeploymentTarget::Heap => DEPLOYMENT_TARGET_HEAP,
        DeploymentTarget::Stack => DEPLOYMENT_TARGET_STACK,
        DeploymentTarget::Gpu => DEPLOYMENT_TARGET_GPU,
        DeploymentTarget::Shared => DEPLOYMENT_TARGET_SHARED,
        DeploymentTarget::Persisted => DEPLOYMENT_TARGET_PERSISTED,
        DeploymentTarget::Custom(_) => DEPLOYMENT_TARGET_CUSTOM,
    }
}

fn tag_to_deployment_target(
    tag: u32,
    reader: &mut BinaryReader,
) -> Result<DeploymentTarget, DeserializeError> {
    match tag {
        DEPLOYMENT_TARGET_HEAP => Ok(DeploymentTarget::Heap),
        DEPLOYMENT_TARGET_STACK => Ok(DeploymentTarget::Stack),
        DEPLOYMENT_TARGET_GPU => Ok(DeploymentTarget::Gpu),
        DEPLOYMENT_TARGET_SHARED => Ok(DeploymentTarget::Shared),
        DEPLOYMENT_TARGET_PERSISTED => Ok(DeploymentTarget::Persisted),
        DEPLOYMENT_TARGET_CUSTOM => {
            let name = reader.read_string("DeploymentTarget.Custom")?;
            Ok(DeploymentTarget::Custom(name))
        }
        _ => Err(DeserializeError::InvalidValue {
            field: "DeploymentTarget".to_string(),
            value: format!("{}", tag),
        }),
    }
}

// ── Intermediate serializable representation ────────────────────────────

/// Intermediate representation used for JSON serialization.
///
/// This struct flattens the SCG's internal petgraph representation into
/// a simple structure that can be serialized with serde.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SerializedSCG {
    /// Format version for forward/backward compatibility.
    version: u32,
    /// All nodes in the graph.
    nodes: Vec<NodeData>,
    /// All edges in the graph.
    edges: Vec<EdgeData>,
    /// All regions in the graph.
    regions: Vec<SCGRegion>,
    /// The next node ID counter.
    next_node_id: u64,
    /// The next edge ID counter.
    next_edge_id: u64,
}

// ── Extract SCG into intermediate representation ────────────────────────

fn scg_to_serialized(scg: &SCG) -> SerializedSCG {
    let nodes: Vec<NodeData> = scg.nodes().cloned().collect();
    let edges: Vec<EdgeData> = scg.edges().cloned().collect();
    let regions: Vec<SCGRegion> = scg.regions().cloned().collect();

    // Compute the next_node_id and next_edge_id from the SCG.
    // We derive these from the maximum observed IDs + 1, since SCG doesn't
    // expose the counters directly.
    let next_node_id = nodes.iter().map(|n| n.id.as_u64() + 1).max().unwrap_or(0);
    let next_edge_id = edges.iter().map(|e| e.id.as_u64() + 1).max().unwrap_or(0);

    SerializedSCG {
        version: FORMAT_VERSION,
        nodes,
        edges,
        regions,
        next_node_id,
        next_edge_id,
    }
}

/// Reconstruct an SCG from the intermediate representation.
fn serialized_to_scg(s: SerializedSCG) -> Result<SCG, DeserializeError> {
    let mut scg = SCG::new();

    // Add nodes first (using their pre-assigned IDs)
    for node_data in &s.nodes {
        scg.add_node_with_id(
            node_data.id,
            node_data.node_type.clone(),
            node_data.payload.clone(),
            node_data.program_point.clone(),
        )
        .map_err(|e| DeserializeError::ConsistencyError(e.to_string()))?;

        // Restore annotation if present
        if let Some(ref ann) = node_data.annotation {
            if let Some(nd) = scg.get_node_mut(node_data.id) {
                nd.annotation = Some(ann.clone());
            }
        }
    }

    // Add edges (using their pre-assigned IDs)
    for edge_data in &s.edges {
        scg.add_edge_with_id(
            edge_data.id,
            edge_data.source,
            edge_data.target,
            edge_data.kind.clone(),
        )
        .map_err(|e| DeserializeError::ConsistencyError(e.to_string()))?;

        // Restore label if present
        if let Some(ref label) = edge_data.label {
            if let Some(ed) = scg.get_edge_mut(edge_data.id) {
                ed.label = Some(label.clone());
            }
        }
    }

    // Add regions
    for region in s.regions {
        scg.add_region(region);
    }

    Ok(scg)
}

// ── Binary serialization ────────────────────────────────────────────────

/// Serializes an SCG to a versioned binary format.
///
/// The binary format starts with a magic header (`VSCG`), a version number,
/// and then the graph contents encoded as little-endian integers and
/// length-prefixed strings.
///
/// # Example
///
/// ```
/// use vuma_scg::*;
/// let mut scg = SCG::new();
/// let id = scg.add_node(
///     NodeType::Computation,
///     NodePayload::Computation(ComputationNode {
///         kind: ComputationKind::Other("add".to_string()),
///         result_type: None,
///         tail_call: false }),
///     ProgramPoint { file: None, line: None, column: None, offset: None },
/// );
/// let bytes = vuma_scg::serialize::serialize_scg(&scg);
/// assert!(&bytes[..4] == b"VSCG");
/// ```
pub fn serialize_scg(scg: &SCG) -> Vec<u8> {
    let s = scg_to_serialized(scg);
    let mut w = BinaryWriter::new();

    // Header
    w.write_bytes(MAGIC);
    w.write_u32_le(FORMAT_VERSION);
    w.write_u64_le(s.next_node_id);
    w.write_u64_le(s.next_edge_id);
    w.write_u32_le(s.nodes.len() as u32);
    w.write_u32_le(s.edges.len() as u32);
    w.write_u32_le(s.regions.len() as u32);

    // Nodes
    for node in &s.nodes {
        write_node(&mut w, node);
    }

    // Edges
    for edge in &s.edges {
        write_edge(&mut w, edge);
    }

    // Regions
    for region in &s.regions {
        write_region(&mut w, region);
    }

    w.into_vec()
}

/// Deserializes an SCG from the versioned binary format.
///
/// Returns an error if the magic bytes don't match, the version is not
/// supported, or the data is malformed.
pub fn deserialize_scg(data: &[u8]) -> Result<SCG, DeserializeError> {
    let mut reader = BinaryReader::new(data);

    // Read and validate magic
    let magic = reader.read_bytes(4, "magic")?;
    if magic != MAGIC {
        return Err(DeserializeError::InvalidMagic {
            expected: MAGIC.to_vec(),
            found: magic.to_vec(),
        });
    }

    // Read and validate version
    let version = reader.read_u32_le("version")?;
    if version < MIN_SUPPORTED_VERSION {
        return Err(DeserializeError::UnsupportedVersion {
            version,
            min_supported: MIN_SUPPORTED_VERSION,
        });
    }
    // Forward compatibility: we could handle unknown future versions here
    // by reading only the fields we understand. For now, we require
    // version == FORMAT_VERSION for strictness, but allow future versions
    // with a warning-style approach (we just try to read the v1 layout).

    // Read counters and counts
    let next_node_id = reader.read_u64_le("next_node_id")?;
    let next_edge_id = reader.read_u64_le("next_edge_id")?;
    let node_count = reader.read_u32_le("node_count")? as usize;
    let edge_count = reader.read_u32_le("edge_count")? as usize;
    let region_count = reader.read_u32_le("region_count")? as usize;

    // Read nodes
    let mut nodes = Vec::with_capacity(node_count);
    for i in 0..node_count {
        nodes.push(read_node(&mut reader, i)?);
    }

    // Read edges
    let mut edges = Vec::with_capacity(edge_count);
    for i in 0..edge_count {
        edges.push(read_edge(&mut reader, i)?);
    }

    // Read regions
    let mut regions = Vec::with_capacity(region_count);
    for i in 0..region_count {
        regions.push(read_region(&mut reader, i)?);
    }

    let serialized = SerializedSCG {
        version,
        nodes,
        edges,
        regions,
        next_node_id,
        next_edge_id,
    };

    serialized_to_scg(serialized)
}

// ── Binary: Node serialization ──────────────────────────────────────────

fn write_node(w: &mut BinaryWriter, node: &NodeData) {
    w.write_u64_le(node.id.as_u64());
    w.write_u32_le(node_type_to_tag(&node.node_type));

    // Annotation (BDReference)
    write_optional_bd_reference(w, &node.annotation);

    // Program point
    write_program_point(w, &node.program_point);

    // Payload
    write_payload(w, &node.payload);
}

fn read_node(reader: &mut BinaryReader, index: usize) -> Result<NodeData, DeserializeError> {
    let ctx = || format!("node[{}]", index);

    let id = NodeId::new(reader.read_u64_le(&ctx())?);
    let node_type = {
        let tag = reader.read_u32_le(&format!("{}.node_type", ctx()))?;
        tag_to_node_type(tag)?
    };
    let annotation = read_optional_bd_reference(reader, &format!("{}.annotation", ctx()))?;
    let program_point = read_program_point(reader, &format!("{}.program_point", ctx()))?;
    let payload = read_payload(reader, &node_type, &format!("{}.payload", ctx()))?;

    Ok(NodeData {
        id,
        node_type,
        annotation,
        program_point,
        payload,
    })
}

fn write_optional_bd_reference(w: &mut BinaryWriter, opt: &Option<BDReference>) {
    match opt {
        Some(bd) => {
            w.write_bool(true);
            w.write_u64_le(bd.bd_id);
            w.write_optional_u64(&bd.version);
        }
        None => {
            w.write_bool(false);
        }
    }
}

fn read_optional_bd_reference(
    reader: &mut BinaryReader,
    context: &str,
) -> Result<Option<BDReference>, DeserializeError> {
    let has = reader.read_bool(&format!("{}.has", context))?;
    if has {
        let bd_id = reader.read_u64_le(&format!("{}.bd_id", context))?;
        let version = reader.read_optional_u64(&format!("{}.version", context))?;
        Ok(Some(BDReference { bd_id, version }))
    } else {
        Ok(None)
    }
}

fn write_program_point(w: &mut BinaryWriter, pp: &ProgramPoint) {
    w.write_optional_string(&pp.file);
    w.write_optional_u64(&pp.line);
    w.write_optional_u64(&pp.column);
    w.write_optional_u64(&pp.offset);
}

fn read_program_point(
    reader: &mut BinaryReader,
    context: &str,
) -> Result<ProgramPoint, DeserializeError> {
    let file = reader.read_optional_string(&format!("{}.file", context))?;
    let line = reader.read_optional_u64(&format!("{}.line", context))?;
    let column = reader.read_optional_u64(&format!("{}.column", context))?;
    let offset = reader.read_optional_u64(&format!("{}.offset", context))?;
    Ok(ProgramPoint {
        file,
        line,
        column,
        offset,
    })
}

fn write_payload(w: &mut BinaryWriter, payload: &NodePayload) {
    match payload {
        NodePayload::Computation(c) => {
            w.write_u32_le(NODE_TYPE_COMPUTATION);
            w.write_string(&c.kind.label());
            w.write_optional_string(&c.result_type);
        }
        NodePayload::Allocation(a) => {
            w.write_u32_le(NODE_TYPE_ALLOCATION);
            w.write_u64_le(a.size);
            w.write_u64_le(a.align);
            w.write_u64_le(a.region_id.as_u64());
            w.write_optional_string(&a.type_name);
        }
        NodePayload::Deallocation(d) => {
            w.write_u32_le(NODE_TYPE_DEALLOCATION);
            w.write_u64_le(d.allocation_node.as_u64());
            w.write_u64_le(d.region_id.as_u64());
        }
        NodePayload::Access(a) => {
            w.write_u32_le(NODE_TYPE_ACCESS);
            w.write_u32_le(access_mode_to_tag(&a.mode));
            w.write_u64_le(a.region_id.as_u64());
            w.write_optional_u64(&a.offset);
            w.write_optional_u64(&a.access_size);
        }
        NodePayload::Cast(c) => {
            w.write_u32_le(NODE_TYPE_CAST);
            w.write_string(&c.from_type);
            w.write_string(&c.to_type);
            w.write_bool(c.is_lossless);
        }
        NodePayload::Effect(e) => {
            w.write_u32_le(NODE_TYPE_EFFECT);
            w.write_string(&e.effect_kind);
            w.write_bool(e.is_observable);
        }
        NodePayload::Control(c) => {
            w.write_u32_le(NODE_TYPE_CONTROL);
            w.write_u32_le(control_kind_to_tag(&c.kind));
            w.write_optional_string(&c.label);
        }
        NodePayload::Phantom(p) => {
            w.write_u32_le(NODE_TYPE_PHANTOM);
            w.write_string(&p.purpose);
        }
        NodePayload::VTable(v) => {
            w.write_u32_le(NODE_TYPE_VTABLE);
            w.write_string(&v.trait_name);
            w.write_string(&v.concrete_type);
            w.write_u64_le(v.method_entries.len() as u64);
            for &entry_id in &v.method_entries {
                w.write_u64_le(entry_id.as_u64());
            }
        }
        NodePayload::ClosureEnv(e) => {
            w.write_u32_le(NODE_TYPE_CLOSURE_ENV);
            w.write_u64_le(e.captured_vars.len() as u64);
            for var in &e.captured_vars {
                w.write_string(var);
            }
            w.write_u64_le(e.capture_modes.len() as u64);
            for &mode in &e.capture_modes {
                w.write_bool(mode);
            }
            w.write_optional_u64(&e.closure_entry.map(|id| id.as_u64()));
        }
        NodePayload::StructDef(s) => {
            w.write_u32_le(NODE_TYPE_STRUCT_DEF);
            w.write_string(&s.name);
            w.write_u64_le(s.fields.len() as u64);
            for f in &s.fields {
                w.write_string(&f.name);
                w.write_string(&f.type_name);
                w.write_u64_le(f.offset);
                w.write_u64_le(f.size);
            }
            w.write_u64_le(s.total_size);
            w.write_u64_le(s.alignment);
        }
        NodePayload::EnumDef(e) => {
            w.write_u32_le(NODE_TYPE_ENUM_DEF);
            w.write_string(&e.name);
            w.write_u64_le(e.variants.len() as u64);
            for v in &e.variants {
                w.write_string(&v.name);
                w.write_u64_le(v.discriminant);
                w.write_optional_string(&v.payload_type);
                w.write_u64_le(v.payload_size);
            }
            w.write_string(&e.tag_type);
            w.write_u64_le(e.tag_size);
            w.write_u64_le(e.max_payload_size);
            w.write_u64_le(e.total_size);
            w.write_u64_le(e.alignment);
        }
        NodePayload::Match(m) => {
            w.write_u32_le(NODE_TYPE_MATCH);
            w.write_string(&m.subject);
            w.write_string(&m.subject_type);
            w.write_u64_le(m.arms.len() as u64);
            for arm in &m.arms {
                write_match_pattern(w, &arm.pattern);
                w.write_optional_string(&arm.guard);
                w.write_u64_le(arm.body.len() as u64);
                for stmt in &arm.body {
                    w.write_string(stmt);
                }
            }
        }
        NodePayload::ConstantTime(ct) => {
            w.write_u32_le(NODE_TYPE_CONSTANT_TIME);
            w.write_u32_le(match ct.op {
                ConstantTimeOp::CtSelect => 0,
                ConstantTimeOp::CtEq => 1,
            });
            w.write_string(&ct.dst);
            w.write_u64_le(ct.operands.len() as u64);
            for op in &ct.operands {
                w.write_string(op);
            }
        }
    }
}

fn write_match_pattern(w: &mut BinaryWriter, pattern: &MatchPatternInfo) {
    match pattern {
        MatchPatternInfo::Wildcard => {
            w.write_u32_le(0);
        }
        MatchPatternInfo::Lit(v) => {
            w.write_u32_le(1);
            w.write_u64_le(*v as u64);
        }
        MatchPatternInfo::Ident(name) => {
            w.write_u32_le(2);
            w.write_string(name);
        }
        MatchPatternInfo::Enum { variant, binding } => {
            w.write_u32_le(3);
            w.write_string(variant);
            w.write_optional_string(binding);
        }
        MatchPatternInfo::Struct { name, fields } => {
            w.write_u32_le(4);
            w.write_string(name);
            w.write_u64_le(fields.len() as u64);
            for f in fields {
                w.write_string(f);
            }
        }
        MatchPatternInfo::Or(patterns) => {
            w.write_u32_le(5);
            w.write_u64_le(patterns.len() as u64);
            for p in patterns {
                write_match_pattern(w, p);
            }
        }
    }
}

fn read_payload(
    reader: &mut BinaryReader,
    expected_type: &NodeType,
    context: &str,
) -> Result<NodePayload, DeserializeError> {
    let tag = reader.read_u32_le(&format!("{}.tag", context))?;
    let payload_type = tag_to_node_type(tag)?;

    // Validate consistency between node_type and payload tag
    if &payload_type != expected_type {
        return Err(DeserializeError::ConsistencyError(format!(
            "node_type {:?} does not match payload tag {:?}",
            expected_type, payload_type
        )));
    }

    match payload_type {
        NodeType::Computation => {
            let operation = reader.read_string(&format!("{}.kind", context))?;
            let result_type = reader.read_optional_string(&format!("{}.result_type", context))?;
            Ok(NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other(operation),
                result_type,
                tail_call: false,
            }))
        }
        NodeType::Allocation => {
            let size = reader.read_u64_le(&format!("{}.size", context))?;
            let align = reader.read_u64_le(&format!("{}.align", context))?;
            let region_id = RegionId::new(reader.read_u64_le(&format!("{}.region_id", context))?);
            let type_name = reader.read_optional_string(&format!("{}.type_name", context))?;
            Ok(NodePayload::Allocation(AllocationNode {
                size,
                align,
                region_id,
                type_name,
            }))
        }
        NodeType::Deallocation => {
            let allocation_node =
                NodeId::new(reader.read_u64_le(&format!("{}.allocation_node", context))?);
            let region_id = RegionId::new(reader.read_u64_le(&format!("{}.region_id", context))?);
            Ok(NodePayload::Deallocation(DeallocationNode {
                allocation_node,
                region_id,
            }))
        }
        NodeType::Access => {
            let mode_tag = reader.read_u32_le(&format!("{}.mode", context))?;
            let mode = tag_to_access_mode(mode_tag)?;
            let region_id = RegionId::new(reader.read_u64_le(&format!("{}.region_id", context))?);
            let offset = reader.read_optional_u64(&format!("{}.offset", context))?;
            let access_size = reader.read_optional_u64(&format!("{}.access_size", context))?;
            Ok(NodePayload::Access(AccessNode {
                mode,
                region_id,
                offset,
                access_size,
            }))
        }
        NodeType::Cast => {
            let from_type = reader.read_string(&format!("{}.from_type", context))?;
            let to_type = reader.read_string(&format!("{}.to_type", context))?;
            let is_lossless = reader.read_bool(&format!("{}.is_lossless", context))?;
            Ok(NodePayload::Cast(CastNode {
                from_type,
                to_type,
                is_lossless,
            }))
        }
        NodeType::Effect => {
            let effect_kind = reader.read_string(&format!("{}.effect_kind", context))?;
            let is_observable = reader.read_bool(&format!("{}.is_observable", context))?;
            Ok(NodePayload::Effect(EffectNode {
                effect_kind,
                is_observable,
            }))
        }
        NodeType::Control => {
            let kind_tag = reader.read_u32_le(&format!("{}.kind", context))?;
            let kind = tag_to_control_kind(kind_tag)?;
            let label = reader.read_optional_string(&format!("{}.label", context))?;
            Ok(NodePayload::Control(ControlNode { kind, label }))
        }
        NodeType::Phantom => {
            let purpose = reader.read_string(&format!("{}.purpose", context))?;
            Ok(NodePayload::Phantom(PhantomNode { purpose }))
        }
        NodeType::VTable => {
            let trait_name = reader.read_string(&format!("{}.trait_name", context))?;
            let concrete_type = reader.read_string(&format!("{}.concrete_type", context))?;
            let count = reader.read_u64_le(&format!("{}.method_count", context))?;
            let mut method_entries = Vec::with_capacity(count as usize);
            for i in 0..count as usize {
                method_entries.push(NodeId::new(
                    reader.read_u64_le(&format!("{}.method[{}]", context, i))?,
                ));
            }
            Ok(NodePayload::VTable(VTableNode {
                trait_name,
                concrete_type,
                method_entries,
            }))
        }
        NodeType::ClosureEnv => {
            let var_count = reader.read_u64_le(&format!("{}.var_count", context))?;
            let mut captured_vars = Vec::with_capacity(var_count as usize);
            for i in 0..var_count as usize {
                captured_vars
                    .push(reader.read_string(&format!("{}.captured_var[{}]", context, i))?);
            }
            let mode_count = reader.read_u64_le(&format!("{}.mode_count", context))?;
            let mut capture_modes = Vec::with_capacity(mode_count as usize);
            for i in 0..mode_count as usize {
                capture_modes.push(reader.read_bool(&format!("{}.capture_mode[{}]", context, i))?);
            }
            let closure_entry = reader
                .read_optional_u64(&format!("{}.closure_entry", context))?
                .map(NodeId::new);
            Ok(NodePayload::ClosureEnv(ClosureEnvNode {
                captured_vars,
                capture_modes,
                closure_entry,
            }))
        }
        NodeType::StructDef => {
            let name = reader.read_string(&format!("{}.name", context))?;
            let field_count = reader.read_u64_le(&format!("{}.field_count", context))?;
            let mut fields = Vec::with_capacity(field_count as usize);
            for i in 0..field_count as usize {
                let f_name = reader.read_string(&format!("{}.field[{}].name", context, i))?;
                let f_type = reader.read_string(&format!("{}.field[{}].type", context, i))?;
                let f_offset = reader.read_u64_le(&format!("{}.field[{}].offset", context, i))?;
                let f_size = reader.read_u64_le(&format!("{}.field[{}].size", context, i))?;
                fields.push(StructFieldInfo {
                    name: f_name,
                    type_name: f_type,
                    offset: f_offset,
                    size: f_size,
                });
            }
            let total_size = reader.read_u64_le(&format!("{}.total_size", context))?;
            let alignment = reader.read_u64_le(&format!("{}.alignment", context))?;
            Ok(NodePayload::StructDef(StructDefNode {
                name,
                fields,
                total_size,
                alignment,
            }))
        }
        NodeType::EnumDef => {
            let name = reader.read_string(&format!("{}.name", context))?;
            let variant_count = reader.read_u64_le(&format!("{}.variant_count", context))?;
            let mut variants = Vec::with_capacity(variant_count as usize);
            for i in 0..variant_count as usize {
                let v_name = reader.read_string(&format!("{}.variant[{}].name", context, i))?;
                let discriminant = reader.read_u64_le(&format!("{}.variant[{}].discriminant", context, i))?;
                let payload_type = reader.read_optional_string(&format!("{}.variant[{}].payload_type", context, i))?;
                let payload_size = reader.read_u64_le(&format!("{}.variant[{}].payload_size", context, i))?;
                variants.push(EnumVariantInfo {
                    name: v_name,
                    discriminant,
                    payload_type,
                    payload_size,
                });
            }
            let tag_type = reader.read_string(&format!("{}.tag_type", context))?;
            let tag_size = reader.read_u64_le(&format!("{}.tag_size", context))?;
            let max_payload_size = reader.read_u64_le(&format!("{}.max_payload_size", context))?;
            let total_size = reader.read_u64_le(&format!("{}.total_size", context))?;
            let alignment = reader.read_u64_le(&format!("{}.alignment", context))?;
            Ok(NodePayload::EnumDef(EnumDefNode {
                name,
                variants,
                tag_type,
                tag_size,
                max_payload_size,
                total_size,
                alignment,
            }))
        }
        NodeType::Match => {
            let subject = reader.read_string(&format!("{}.subject", context))?;
            let subject_type = reader.read_string(&format!("{}.subject_type", context))?;
            let arm_count = reader.read_u64_le(&format!("{}.arm_count", context))?;
            let mut arms = Vec::with_capacity(arm_count as usize);
            for i in 0..arm_count as usize {
                let pattern = read_match_pattern(reader, &format!("{}.arm[{}].pattern", context, i))?;
                let guard = reader.read_optional_string(&format!("{}.arm[{}].guard", context, i))?;
                let body_count = reader.read_u64_le(&format!("{}.arm[{}].body_count", context, i))?;
                let mut body = Vec::with_capacity(body_count as usize);
                for j in 0..body_count as usize {
                    body.push(reader.read_string(&format!("{}.arm[{}].body[{}]", context, i, j))?);
                }
                arms.push(MatchArmInfo {
                    pattern,
                    guard,
                    body,
                });
            }
            Ok(NodePayload::Match(MatchNode {
                subject,
                arms,
                subject_type,
            }))
        }
        NodeType::ConstantTime => {
            let op_tag = reader.read_u32_le(&format!("{}.op", context))?;
            let op = match op_tag {
                0 => ConstantTimeOp::CtSelect,
                1 => ConstantTimeOp::CtEq,
                _ => return Err(DeserializeError::InvalidValue {
                    field: format!("{}.op", context),
                    value: format!("{}", op_tag),
                }),
            };
            let dst = reader.read_string(&format!("{}.dst", context))?;
            let operand_count = reader.read_u64_le(&format!("{}.operand_count", context))?;
            let mut operands = Vec::with_capacity(operand_count as usize);
            for i in 0..operand_count as usize {
                operands.push(reader.read_string(&format!("{}.operand[{}]", context, i))?);
            }
            Ok(NodePayload::ConstantTime(ConstantTimeNode {
                op,
                dst,
                operands,
            }))
        }
        // Womb data model variants were removed; tags 14..=25 now fall
        // through to the `tag_to_node_type` error path above before
        // reaching this match.
    }
}

fn read_match_pattern(
    reader: &mut BinaryReader,
    context: &str,
) -> Result<MatchPatternInfo, DeserializeError> {
    let tag = reader.read_u32_le(&format!("{}.tag", context))?;
    match tag {
        0 => Ok(MatchPatternInfo::Wildcard),
        1 => {
            let v = reader.read_u64_le(&format!("{}.value", context))? as i64;
            Ok(MatchPatternInfo::Lit(v))
        }
        2 => {
            let name = reader.read_string(&format!("{}.name", context))?;
            Ok(MatchPatternInfo::Ident(name))
        }
        3 => {
            let variant = reader.read_string(&format!("{}.variant", context))?;
            let binding = reader.read_optional_string(&format!("{}.binding", context))?;
            Ok(MatchPatternInfo::Enum { variant, binding })
        }
        4 => {
            let name = reader.read_string(&format!("{}.name", context))?;
            let field_count = reader.read_u64_le(&format!("{}.field_count", context))?;
            let mut fields = Vec::with_capacity(field_count as usize);
            for i in 0..field_count as usize {
                fields.push(reader.read_string(&format!("{}.field[{}]", context, i))?);
            }
            Ok(MatchPatternInfo::Struct { name, fields })
        }
        5 => {
            let pattern_count = reader.read_u64_le(&format!("{}.pattern_count", context))?;
            let mut patterns = Vec::with_capacity(pattern_count as usize);
            for i in 0..pattern_count as usize {
                patterns.push(read_match_pattern(reader, &format!("{}.pattern[{}]", context, i))?);
            }
            Ok(MatchPatternInfo::Or(patterns))
        }
        _ => Err(DeserializeError::InvalidValue {
            field: format!("{}.tag", context),
            value: format!("{}", tag),
        }),
    }
}

// ── Binary: Edge serialization ──────────────────────────────────────────

fn write_edge(w: &mut BinaryWriter, edge: &EdgeData) {
    w.write_u64_le(edge.id.as_u64());
    w.write_u64_le(edge.source.as_u64());
    w.write_u64_le(edge.target.as_u64());
    w.write_u32_le(edge_kind_to_tag(&edge.kind));
    w.write_optional_string(&edge.label);
}

fn read_edge(reader: &mut BinaryReader, index: usize) -> Result<EdgeData, DeserializeError> {
    let ctx = || format!("edge[{}]", index);

    let id = EdgeId::new(reader.read_u64_le(&ctx())?);
    let source = NodeId::new(reader.read_u64_le(&format!("{}.source", ctx()))?);
    let target = NodeId::new(reader.read_u64_le(&format!("{}.target", ctx()))?);
    let kind = {
        let tag = reader.read_u32_le(&format!("{}.kind", ctx()))?;
        tag_to_edge_kind(tag)?
    };
    let label = reader.read_optional_string(&format!("{}.label", ctx()))?;

    Ok(EdgeData {
        id,
        source,
        target,
        kind,
        label,
    })
}

// ── Binary: Region serialization ────────────────────────────────────────

fn write_region(w: &mut BinaryWriter, region: &SCGRegion) {
    w.write_u64_le(region.id.as_u64());
    w.write_u32_le(region.nodes.len() as u32);
    for node_id in &region.nodes {
        w.write_u64_le(node_id.as_u64());
    }
    w.write_u32_le(region.scope_level);
    w.write_bool(region.security_boundary);
    w.write_u32_le(deployment_target_to_tag(&region.deployment_target));
    if let DeploymentTarget::Custom(ref name) = region.deployment_target {
        w.write_string(name);
    }
}

fn read_region(reader: &mut BinaryReader, index: usize) -> Result<SCGRegion, DeserializeError> {
    let ctx = || format!("region[{}]", index);

    let id = RegionId::new(reader.read_u64_le(&ctx())?);
    let node_count = reader.read_u32_le(&format!("{}.node_count", ctx()))? as usize;
    let mut nodes = hashbrown::HashSet::with_capacity(node_count);
    for j in 0..node_count {
        let nid = NodeId::new(reader.read_u64_le(&format!("{}.nodes[{}]", ctx(), j))?);
        nodes.insert(nid);
    }
    let scope_level = reader.read_u32_le(&format!("{}.scope_level", ctx()))?;
    let security_boundary = reader.read_bool(&format!("{}.security_boundary", ctx()))?;
    let dt_tag = reader.read_u32_le(&format!("{}.deployment_target", ctx()))?;
    let deployment_target = tag_to_deployment_target(dt_tag, reader)?;

    Ok(SCGRegion {
        id,
        nodes,
        scope_level,
        security_boundary,
        deployment_target,
    })
}

// ── JSON serialization ──────────────────────────────────────────────────

/// Serializes an SCG to a JSON string for debugging and interoperability.
///
/// The JSON representation uses the same intermediate format as the binary
/// serialization, but rendered as human-readable JSON.
pub fn serialize_scg_json(scg: &SCG) -> String {
    let s = scg_to_serialized(scg);
    serde_json::to_string_pretty(&s).unwrap_or_else(|e| {
        // Fallback: return an error object if serialization fails
        format!(r#"{{"error": "serialization failed: {}"}}"#, e)
    })
}

/// Deserializes an SCG from a JSON string.
///
/// Returns an error if the JSON is malformed or contains inconsistent data.
pub fn deserialize_scg_json(json: &str) -> Result<SCG, DeserializeError> {
    let s: SerializedSCG = serde_json::from_str(json)?;
    serialized_to_scg(s)
}

// ── DOT (Graphviz) serialization ────────────────────────────────────────

/// Serializes an SCG to Graphviz DOT format for visualization.
///
/// Produces a directed graph (`digraph`) where:
/// - Nodes are rendered with labels showing their type and key payload info.
/// - Edges are styled by kind (solid for data flow, dashed for control flow,
///   dotted for derivation, bold for annotation).
/// - Regions are rendered as subgraph clusters.
///
/// # Example
///
/// ```
/// use vuma_scg::*;
/// let mut scg = SCG::new();
/// let n1 = scg.add_node(
///     NodeType::Computation,
///     NodePayload::Computation(ComputationNode {
///         kind: ComputationKind::Other("add".to_string()),
///         result_type: None,
///         tail_call: false }),
///     ProgramPoint { file: None, line: None, column: None, offset: None },
/// );
/// let dot = vuma_scg::serialize::serialize_scg_dot(&scg);
/// assert!(dot.contains("digraph SCG"));
/// ```
pub fn serialize_scg_dot(scg: &SCG) -> String {
    let mut out = String::with_capacity(4096);

    // Graph header
    out.push_str("digraph SCG {\n");
    out.push_str("    rankdir=TB;\n");
    out.push_str("    node [shape=record, fontname=\"monospace\"];\n");
    out.push_str("    edge [fontname=\"monospace\"];\n");
    out.push('\n');

    // Group nodes by region for subgraph clustering
    let mut node_to_region: std::collections::HashMap<NodeId, RegionId> =
        std::collections::HashMap::new();
    for region in scg.regions() {
        for node_id in region.iter_nodes() {
            node_to_region.insert(*node_id, region.id);
        }
    }

    // Render regions as subgraph clusters
    for region in scg.regions() {
        let cluster_name = format!("cluster_region_{}", region.id.as_u64());
        out.push_str(&format!(
            "    subgraph {} {{\n",
            dot_escape_id(&cluster_name)
        ));
        let boundary_label = if region.security_boundary {
            " [SECURITY BOUNDARY]"
        } else {
            ""
        };
        out.push_str(&format!(
            "        label=\"Region {} ({}){}\";\n",
            region.id.as_u64(),
            region.deployment_target,
            boundary_label
        ));
        out.push_str(&format!(
            "        style=dashed;\n        color={};\n",
            if region.security_boundary {
                "red"
            } else {
                "blue"
            }
        ));

        for node_id in region.iter_nodes() {
            if let Some(node_data) = scg.get_node(*node_id) {
                out.push_str(&format!("        {}\n", format_dot_node(node_data)));
            }
        }
        out.push_str("    }\n\n");
    }

    // Render nodes not in any region
    let mut unassigned: Vec<&NodeData> = Vec::new();
    for node_data in scg.nodes() {
        if !node_to_region.contains_key(&node_data.id) {
            unassigned.push(node_data);
        }
    }
    if !unassigned.is_empty() {
        out.push_str("    subgraph cluster_unassigned {\n");
        out.push_str("        label=\"Unassigned\";\n        style=dotted;\n        color=gray;\n");
        for node_data in &unassigned {
            out.push_str(&format!("        {}\n", format_dot_node(node_data)));
        }
        out.push_str("    }\n\n");
    }

    // Render edges
    for edge_data in scg.edges() {
        let style = match edge_data.kind {
            EdgeKind::DataFlow => "solid",
            EdgeKind::ControlFlow => "dashed",
            EdgeKind::Derivation => "dotted",
            EdgeKind::Annotation => "bold",
            EdgeKind::Dispatch => "bold dashed",
            EdgeKind::Call { .. } => "bold solid",
            EdgeKind::Return { .. } => "bold dotted",
        };
        let color = match edge_data.kind {
            EdgeKind::DataFlow => "black",
            EdgeKind::ControlFlow => "blue",
            EdgeKind::Derivation => "gray",
            EdgeKind::Annotation => "purple",
            EdgeKind::Dispatch => "red",
            EdgeKind::Call { .. } => "green",
            EdgeKind::Return { .. } => "orange",
        };
        let kind_str = format!("{}", edge_data.kind);
        let label = edge_data.label.as_deref().unwrap_or(&kind_str);
        out.push_str(&format!(
            "    {} -> {} [style={}, color={}, label=\"{}\"];\n",
            dot_escape_id(&format!("n{}", edge_data.source.as_u64())),
            dot_escape_id(&format!("n{}", edge_data.target.as_u64())),
            style,
            color,
            dot_escape_label(label),
        ));
    }

    out.push_str("}\n");
    out
}

/// Formats a single node for DOT output.
fn format_dot_node(node: &NodeData) -> String {
    let label = format_node_label(node);
    format!(
        "{} [label=\"{}\"];",
        dot_escape_id(&format!("n{}", node.id.as_u64())),
        dot_escape_label(&label)
    )
}

/// Creates a human-readable label for a node.
fn format_node_label(node: &NodeData) -> String {
    let type_info = match &node.payload {
        NodePayload::Computation(c) => {
            let rt = c
                .result_type
                .as_deref()
                .map(|t| format!(":{}", t))
                .unwrap_or_default();
            format!("{}{}", c.kind.label(), rt)
        }
        NodePayload::Allocation(a) => {
            let tn = a
                .type_name
                .as_deref()
                .map(|t| format!(" {}", t))
                .unwrap_or_default();
            format!("alloc {}B align={}{}", a.size, a.align, tn)
        }
        NodePayload::Deallocation(_) => "dealloc".to_string(),
        NodePayload::Access(a) => {
            let offset = a.offset.map(|o| format!("+{}", o)).unwrap_or_default();
            format!("{:?}{} @r{}", a.mode, offset, a.region_id.as_u64())
        }
        NodePayload::Cast(c) => format!("{} -> {}", c.from_type, c.to_type),
        NodePayload::Effect(e) => format!("eff({})", e.effect_kind),
        NodePayload::Control(c) => {
            let lbl = c
                .label
                .as_deref()
                .map(|l| format!(" {}", l))
                .unwrap_or_default();
            format!("{:?}{}", c.kind, lbl)
        }
        NodePayload::Phantom(p) => format!("phantom({})", p.purpose),
        NodePayload::VTable(v) => format!("vtable({} for {})", v.trait_name, v.concrete_type),
        NodePayload::ClosureEnv(e) => format!("closure_env({:?})", e.captured_vars),
        NodePayload::StructDef(s) => format!("struct_def({})", s.name),
        NodePayload::EnumDef(e) => format!("enum_def({})", e.name),
        NodePayload::Match(m) => format!("match({})", m.subject),
        NodePayload::ConstantTime(ct) => format!("ct_{:?}", ct.op),
    };

    let ann = node
        .annotation
        .as_ref()
        .map(|bd| format!(" [BD:{}]", bd.bd_id))
        .unwrap_or_default();

    format!(
        "n{}: {}\\n{}{}",
        node.id.as_u64(),
        node.node_type,
        type_info,
        ann
    )
}

/// Escapes a string for use as a DOT identifier.
fn dot_escape_id(s: &str) -> String {
    s.replace('-', "_")
}

/// Escapes a string for use as a DOT label (inside double quotes).
fn dot_escape_label(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace('<', "\\<")
        .replace('>', "\\>")
        .replace('|', "\\|")
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a default ProgramPoint for testing.
    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: Some("test.vu".to_string()),
            line: Some(1),
            column: Some(1),
            offset: None,
        }
    }

    /// Helper: create a minimal SCG with a single computation node.
    fn minimal_scg() -> SCG {
        let mut scg = SCG::new();
        scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            pp(),
        );
        scg
    }

    /// Helper: create a complex SCG with multiple node types, edges, and regions.
    fn complex_scg() -> SCG {
        let mut scg = SCG::new();
        let region_id = RegionId::new(1);

        // Allocation node
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 16,
                region_id,
                type_name: Some("Buffer".to_string()),
            }),
            ProgramPoint {
                file: Some("main.vu".to_string()),
                line: Some(10),
                column: Some(5),
                offset: Some(200),
            },
        );

        // Computation node
        let comp = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("write".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );

        // Access node (read)
        let access = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::ReadWrite,
                region_id,
                offset: Some(32),
                access_size: Some(8),
            }),
            pp(),
        );

        // Cast node
        let cast = scg.add_node(
            NodeType::Cast,
            NodePayload::Cast(CastNode {
                from_type: "i32".to_string(),
                to_type: "i64".to_string(),
                is_lossless: true,
            }),
            pp(),
        );

        // Effect node
        let effect = scg.add_node(
            NodeType::Effect,
            NodePayload::Effect(EffectNode {
                effect_kind: "print".to_string(),
                is_observable: true,
            }),
            pp(),
        );

        // Control node
        let ctrl = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::Branch,
                label: Some("if_ok".to_string()),
            }),
            pp(),
        );

        // Deallocation node
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id,
            }),
            pp(),
        );

        // Phantom node
        let phantom = scg.add_node(
            NodeType::Phantom,
            NodePayload::Phantom(PhantomNode {
                purpose: "debug_marker".to_string(),
            }),
            pp(),
        );

        // Add edges of all kinds
        scg.add_edge(alloc, comp, EdgeKind::DataFlow).unwrap();
        scg.add_edge(ctrl, comp, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();
        scg.add_edge(phantom, comp, EdgeKind::Annotation).unwrap();
        scg.add_edge(comp, access, EdgeKind::DataFlow).unwrap();
        scg.add_edge(access, cast, EdgeKind::DataFlow).unwrap();
        scg.add_edge(cast, effect, EdgeKind::DataFlow).unwrap();

        // Add edge with label
        let edge_id = scg.add_edge(comp, effect, EdgeKind::ControlFlow).unwrap();
        if let Some(e) = scg.get_edge_mut(edge_id) {
            e.label = Some("then_branch".to_string());
        }

        // Add annotation to a node
        if let Some(n) = scg.get_node_mut(alloc) {
            n.annotation = Some(BDReference {
                bd_id: 42,
                version: Some(3),
            });
        }

        // Add region
        let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);
        region.scope_level = 1;
        region.security_boundary = true;
        region.add_node(alloc);
        region.add_node(dealloc);
        region.add_node(access);
        scg.add_region(region);

        // Add another region with custom deployment target
        let region2_id = RegionId::new(2);
        let mut region2 = SCGRegion::new(region2_id, DeploymentTarget::Custom("TPU".to_string()));
        region2.add_node(comp);
        scg.add_region(region2);

        scg
    }

    // ── Test 1: Binary round-trip of an empty SCG ──────────────────────

    #[test]
    fn test_binary_roundtrip_empty() {
        let scg = SCG::new();
        let bytes = serialize_scg(&scg);
        let restored = deserialize_scg(&bytes).unwrap();
        assert_eq!(restored.node_count(), 0);
        assert_eq!(restored.edge_count(), 0);
        assert_eq!(restored.region_count(), 0);
    }

    // ── Test 2: Binary round-trip of a minimal SCG ─────────────────────

    #[test]
    fn test_binary_roundtrip_minimal() {
        let scg = minimal_scg();
        let bytes = serialize_scg(&scg);
        let restored = deserialize_scg(&bytes).unwrap();

        assert_eq!(restored.node_count(), 1);
        let node = restored.nodes().next().unwrap();
        assert_eq!(node.node_type, NodeType::Computation);
        if let NodePayload::Computation(ref c) = node.payload {
            assert_eq!(c.kind.label(), "add");
            assert_eq!(c.result_type, Some("i32".to_string()));
        } else {
            panic!("Expected Computation payload");
        }
    }

    // ── Test 3: Binary round-trip of a complex SCG ─────────────────────

    #[test]
    fn test_binary_roundtrip_complex() {
        let scg = complex_scg();
        let bytes = serialize_scg(&scg);
        let restored = deserialize_scg(&bytes).unwrap();

        // Check node count
        assert_eq!(restored.node_count(), scg.node_count());
        assert_eq!(restored.edge_count(), scg.edge_count());
        assert_eq!(restored.region_count(), scg.region_count());

        // Check all node types are present
        let types: std::collections::HashSet<NodeType> =
            restored.nodes().map(|n| n.node_type.clone()).collect();
        assert!(types.contains(&NodeType::Allocation));
        assert!(types.contains(&NodeType::Computation));
        assert!(types.contains(&NodeType::Access));
        assert!(types.contains(&NodeType::Cast));
        assert!(types.contains(&NodeType::Effect));
        assert!(types.contains(&NodeType::Control));
        assert!(types.contains(&NodeType::Deallocation));
        assert!(types.contains(&NodeType::Phantom));

        // Check BD annotation preserved
        let alloc_node = restored
            .nodes()
            .find(|n| matches!(n.node_type, NodeType::Allocation))
            .unwrap();
        assert!(alloc_node.annotation.is_some());
        let bd = alloc_node.annotation.as_ref().unwrap();
        assert_eq!(bd.bd_id, 42);
        assert_eq!(bd.version, Some(3));

        // Check edge with label
        let labeled_edges: Vec<&EdgeData> =
            restored.edges().filter(|e| e.label.is_some()).collect();
        assert_eq!(labeled_edges.len(), 1);
        assert_eq!(labeled_edges[0].label, Some("then_branch".to_string()));

        // Check regions
        let region1 = restored.get_region(RegionId::new(1)).unwrap();
        assert_eq!(region1.node_count(), 3);
        assert!(region1.security_boundary);
        assert_eq!(region1.scope_level, 1);

        let region2 = restored.get_region(RegionId::new(2)).unwrap();
        assert_eq!(
            region2.deployment_target,
            DeploymentTarget::Custom("TPU".to_string())
        );
    }

    // ── Test 4: Binary deserialization rejects invalid magic ────────────

    #[test]
    fn test_binary_invalid_magic() {
        let data = b"BADG\x01\x00\x00\x00"; // wrong magic, version 1
        let result = deserialize_scg(data);
        assert!(matches!(result, Err(DeserializeError::InvalidMagic { .. })));
    }

    // ── Test 5: Binary deserialization rejects truncated data ───────────

    #[test]
    fn test_binary_truncated_data() {
        let scg = minimal_scg();
        let mut bytes = serialize_scg(&scg);
        // Truncate to just the header (16 bytes) — no node data
        bytes.truncate(16);
        let result = deserialize_scg(&bytes);
        assert!(matches!(
            result,
            Err(DeserializeError::UnexpectedEof { .. })
        ));
    }

    // ── Test 6: JSON round-trip of a complex SCG ───────────────────────

    #[test]
    fn test_json_roundtrip_complex() {
        let scg = complex_scg();
        let json = serialize_scg_json(&scg);
        let restored = deserialize_scg_json(&json).unwrap();

        assert_eq!(restored.node_count(), scg.node_count());
        assert_eq!(restored.edge_count(), scg.edge_count());
        assert_eq!(restored.region_count(), scg.region_count());

        // Verify a specific node
        let comp_node = restored
            .nodes()
            .find(|n| matches!(n.node_type, NodeType::Computation))
            .unwrap();
        if let NodePayload::Computation(ref c) = comp_node.payload {
            assert_eq!(c.kind.label(), "write");
        } else {
            panic!("Expected Computation payload");
        }

        // Verify a specific edge
        let dataflow_edges: Vec<&EdgeData> = restored
            .edges()
            .filter(|e| matches!(e.kind, EdgeKind::DataFlow))
            .collect();
        assert_eq!(dataflow_edges.len(), 4);
    }

    // ── Test 7: JSON deserialization rejects malformed JSON ─────────────

    #[test]
    fn test_json_malformed() {
        let result = deserialize_scg_json("{invalid json");
        assert!(matches!(result, Err(DeserializeError::JsonError(_))));
    }

    // ── Test 8: DOT output contains expected elements ──────────────────

    #[test]
    fn test_dot_output() {
        let scg = complex_scg();
        let dot = serialize_scg_dot(&scg);

        // Basic structure
        assert!(dot.starts_with("digraph SCG"));
        assert!(dot.contains("rankdir=TB"));

        // All node types appear
        assert!(dot.contains("Computation"));
        assert!(dot.contains("Allocation"));
        assert!(dot.contains("Deallocation"));
        assert!(dot.contains("Access"));
        assert!(dot.contains("Cast"));
        assert!(dot.contains("Effect"));
        assert!(dot.contains("Control"));
        assert!(dot.contains("Phantom"));

        // Edge styles
        assert!(dot.contains("style=solid")); // DataFlow
        assert!(dot.contains("style=dashed")); // ControlFlow
        assert!(dot.contains("style=dotted")); // Derivation
        assert!(dot.contains("style=bold")); // Annotation

        // Region clusters
        assert!(dot.contains("subgraph cluster_region_1"));
        assert!(dot.contains("SECURITY BOUNDARY"));
        assert!(dot.contains("subgraph cluster_region_2"));

        // Custom deployment target
        assert!(dot.contains("TPU"));
    }

    // ── Test 9: Binary header validation ───────────────────────────────

    #[test]
    fn test_binary_header_correct() {
        let scg = minimal_scg();
        let bytes = serialize_scg(&scg);

        // Check magic
        assert_eq!(&bytes[0..4], b"VSCG");

        // Check version (u32 LE)
        let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        assert_eq!(version, FORMAT_VERSION);
    }

    // ── Test 10: Cross-format consistency ──────────────────────────────

    #[test]
    fn test_cross_format_consistency() {
        let scg = complex_scg();

        // Serialize to binary and back
        let bin_bytes = serialize_scg(&scg);
        let from_bin = deserialize_scg(&bin_bytes).unwrap();

        // Serialize to JSON and back
        let json_str = serialize_scg_json(&scg);
        let from_json = deserialize_scg_json(&json_str).unwrap();

        // Both should have the same counts
        assert_eq!(from_bin.node_count(), from_json.node_count());
        assert_eq!(from_bin.edge_count(), from_json.edge_count());
        assert_eq!(from_bin.region_count(), from_json.region_count());

        // Check nodes match by type
        let bin_types: std::collections::HashSet<NodeType> =
            from_bin.nodes().map(|n| n.node_type.clone()).collect();
        let json_types: std::collections::HashSet<NodeType> =
            from_json.nodes().map(|n| n.node_type.clone()).collect();
        assert_eq!(bin_types, json_types);
    }

    // ── Test 11: Empty SCG JSON round-trip ─────────────────────────────

    #[test]
    fn test_json_roundtrip_empty() {
        let scg = SCG::new();
        let json = serialize_scg_json(&scg);
        let restored = deserialize_scg_json(&json).unwrap();
        assert_eq!(restored.node_count(), 0);
        assert_eq!(restored.edge_count(), 0);
        assert_eq!(restored.region_count(), 0);
    }

    // ── Test 12: DOT output for empty SCG ──────────────────────────────

    #[test]
    fn test_dot_empty() {
        let scg = SCG::new();
        let dot = serialize_scg_dot(&scg);
        assert!(dot.contains("digraph SCG"));
        assert!(dot.ends_with("}\n"));
    }

    // ── Test 13: ProgramPoint with all optional fields ─────────────────

    #[test]
    fn test_binary_program_point_full() {
        let mut scg = SCG::new();
        scg.add_node(
            NodeType::Phantom,
            NodePayload::Phantom(PhantomNode {
                purpose: "test".to_string(),
            }),
            ProgramPoint {
                file: Some("deep/path/test.vu".to_string()),
                line: Some(42),
                column: Some(7),
                offset: Some(1234),
            },
        );
        let bytes = serialize_scg(&scg);
        let restored = deserialize_scg(&bytes).unwrap();

        let node = restored.nodes().next().unwrap();
        assert_eq!(
            node.program_point.file,
            Some("deep/path/test.vu".to_string())
        );
        assert_eq!(node.program_point.line, Some(42));
        assert_eq!(node.program_point.column, Some(7));
        assert_eq!(node.program_point.offset, Some(1234));
    }

    // ── Test 14: DeserializeError display ──────────────────────────────

    #[test]
    fn test_deserialize_error_display() {
        let err = DeserializeError::InvalidMagic {
            expected: b"VSCG".to_vec(),
            found: b"BADG".to_vec(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("invalid magic"));

        let err = DeserializeError::UnsupportedVersion {
            version: 99,
            min_supported: 1,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("unsupported format version"));

        let err = DeserializeError::UnexpectedEof {
            context: "node[0]".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("unexpected end of input"));

        let err = DeserializeError::InvalidValue {
            field: "EdgeKind".to_string(),
            value: "42".to_string(),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("invalid value"));
    }

    // ── Test 15: Binary round-trip preserves edge sources/targets ──────

    #[test]
    fn test_binary_preserves_edge_endpoints() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("f".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("g".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let eid = scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();

        let bytes = serialize_scg(&scg);
        let restored = deserialize_scg(&bytes).unwrap();

        let edge = restored.get_edge(eid).unwrap();
        assert_eq!(edge.source, n1);
        assert_eq!(edge.target, n2);
        assert_eq!(edge.kind, EdgeKind::DataFlow);
    }
}
