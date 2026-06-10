//! SCG Node Types
//!
//! This module defines all node types used in the Semantic Computation Graph.
//! Nodes represent operations, allocations, accesses, and control flow points
//! within the SCG, each carrying type-specific metadata.

use serde::{Deserialize, Serialize};

use crate::region::RegionId;

/// Unique identifier for a node within the SCG.
///
/// `NodeId` is a newtype wrapper around `u64`, providing type safety
/// to prevent accidental confusion with other identifiers (e.g., `EdgeId`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u64);

impl NodeId {
    /// Creates a new `NodeId` from a `u64` value.
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Returns the underlying `u64` value.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeId({})", self.0)
    }
}

/// Classification of a node's semantic role within the SCG.
///
/// Each variant corresponds to a distinct category of operation or
/// structural element in the computation graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeType {
    /// A pure computation node (e.g., arithmetic, function call).
    Computation,
    /// A memory allocation node.
    Allocation,
    /// A memory deallocation node.
    Deallocation,
    /// A memory access node (read or write).
    Access,
    /// A type cast or coercion node.
    Cast,
    /// A side-effecting node (e.g., I/O, volatile access).
    Effect,
    /// A control flow node (e.g., branch, loop header, join).
    Control,
    /// A phantom node used for structural or analysis purposes.
    Phantom,
    /// A virtual method table node for dynamic dispatch.
    VTable,
    /// A closure environment node capturing variables.
    ClosureEnv,
}

impl std::fmt::Display for NodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeType::Computation => write!(f, "Computation"),
            NodeType::Allocation => write!(f, "Allocation"),
            NodeType::Deallocation => write!(f, "Deallocation"),
            NodeType::Access => write!(f, "Access"),
            NodeType::Cast => write!(f, "Cast"),
            NodeType::Effect => write!(f, "Effect"),
            NodeType::Control => write!(f, "Control"),
            NodeType::Phantom => write!(f, "Phantom"),
            NodeType::VTable => write!(f, "VTable"),
            NodeType::ClosureEnv => write!(f, "ClosureEnv"),
        }
    }
}

/// Optional reference to a BD (Behavioral Descriptor) annotation.
///
/// When present, this links a node to its behavioral specification
/// in the BD subsystem for formal verification purposes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BDReference {
    /// The identifier of the referenced behavioral descriptor.
    pub bd_id: u64,
    /// An optional version tag for the BD reference.
    pub version: Option<u64>,
}

/// A point in the source program corresponding to this node.
///
/// Used for traceability from SCG nodes back to the original
/// source code location.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProgramPoint {
    /// The source file identifier or path.
    pub file: Option<String>,
    /// The line number within the source file (1-based).
    pub line: Option<u64>,
    /// The column number within the source line (1-based).
    pub column: Option<u64>,
    /// An optional byte offset from the start of the file.
    pub offset: Option<u64>,
}

/// Core data associated with every SCG node.
///
/// `NodeData` is the universal node payload stored in the graph.
/// It carries the node's identity, type classification, optional
/// BD annotation reference, and source program point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeData {
    /// The unique identifier of this node.
    pub id: NodeId,
    /// The semantic classification of this node.
    pub node_type: NodeType,
    /// Optional reference to a Behavioral Descriptor annotation.
    pub annotation: Option<BDReference>,
    /// The source program point this node corresponds to.
    pub program_point: ProgramPoint,
    /// Type-specific data payload, varying by `node_type`.
    pub payload: NodePayload,
}

/// Type-specific payload data for each `NodeType` variant.
///
/// Each variant holds the concrete data relevant to that kind of node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodePayload {
    /// Payload for `NodeType::Computation`.
    Computation(ComputationNode),
    /// Payload for `NodeType::Allocation`.
    Allocation(AllocationNode),
    /// Payload for `NodeType::Deallocation`.
    Deallocation(DeallocationNode),
    /// Payload for `NodeType::Access`.
    Access(AccessNode),
    /// Payload for `NodeType::Cast`.
    Cast(CastNode),
    /// Payload for `NodeType::Effect`.
    Effect(EffectNode),
    /// Payload for `NodeType::Control`.
    Control(ControlNode),
    /// Payload for `NodeType::Phantom`.
    Phantom(PhantomNode),
    /// Payload for `NodeType::VTable`.
    VTable(VTableNode),
    /// Payload for `NodeType::ClosureEnv`.
    ClosureEnv(ClosureEnvNode),
}

/// Data specific to a computation node.
///
/// Represents a pure computational operation such as arithmetic,
/// function invocation, or data transformation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComputationNode {
    /// A textual or symbolic representation of the operation performed.
    pub operation: String,
    /// An optional type signature for the computation's result.
    pub result_type: Option<String>,
    /// Whether this computation is a tail call (the last action before return).
    /// Set by the TailCallOptimization transform.
    pub tail_call: bool,
}

/// Data specific to an allocation node.
///
/// Represents a memory allocation request with size, alignment,
/// and region association.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllocationNode {
    /// The size of the allocation in bytes.
    pub size: u64,
    /// The alignment requirement in bytes (must be a power of two).
    pub align: u64,
    /// The memory region in which the allocation occurs.
    pub region_id: RegionId,
    /// An optional type name for the allocated object.
    pub type_name: Option<String>,
}

/// Data specific to a deallocation node.
///
/// Represents a memory deallocation, paired with a prior allocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeallocationNode {
    /// The `NodeId` of the corresponding allocation node.
    pub allocation_node: NodeId,
    /// The memory region from which the memory is deallocated.
    pub region_id: RegionId,
}

/// Access mode for memory access nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccessMode {
    /// Read-only access.
    Read,
    /// Write-only access.
    Write,
    /// Read-write access.
    ReadWrite,
}

/// Data specific to a memory access node.
///
/// Represents a read, write, or read-write access to memory
/// within a specific region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessNode {
    /// The access mode (read, write, or read-write).
    pub mode: AccessMode,
    /// The memory region being accessed.
    pub region_id: RegionId,
    /// An optional offset into the region (in bytes).
    pub offset: Option<u64>,
    /// An optional size of the accessed memory (in bytes).
    pub access_size: Option<u64>,
}

/// Data specific to a type cast node.
///
/// Represents a type conversion or coercion from one type to another.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CastNode {
    /// The source type being cast from.
    pub from_type: String,
    /// The target type being cast to.
    pub to_type: String,
    /// Whether the cast is lossless (preserves all information).
    pub is_lossless: bool,
}

/// Data specific to a side-effecting node.
///
/// Represents operations that have observable side effects
/// beyond their return value, such as I/O or volatile access.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectNode {
    /// A textual description of the side effect.
    pub effect_kind: String,
    /// Whether the effect is observable from outside the program.
    pub is_observable: bool,
}

/// Control flow kind for control nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ControlKind {
    /// A conditional branch.
    Branch,
    /// A loop header (entry point of a loop).
    LoopHeader,
    /// A loop exit point.
    LoopExit,
    /// A join point where control flow merges.
    Join,
    /// A function entry point.
    FunctionEntry,
    /// A function return point.
    FunctionReturn,
    /// An unconditional jump.
    Jump,
    /// A switch/match decision point that dispatches to multiple cases.
    Switch,
    /// A single case arm of a switch/match.
    SwitchCase,
    /// A closure entry point.
    ClosureEntry,
    /// A closure return point.
    ClosureReturn,
    /// A future poll point (await).
    FuturePoll,
    /// A waker registration node for async state machines.
    WakerRegistration,
    /// A state machine state transition point.
    StateTransition,
}

/// Data specific to a control flow node.
///
/// Represents points in the graph where control flow decisions
/// are made or merged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlNode {
    /// The specific kind of control flow operation.
    pub kind: ControlKind,
    /// An optional label for the control flow point.
    pub label: Option<String>,
}

/// Data specific to a phantom node.
///
/// Phantom nodes are structural placeholders used for analysis,
/// visualization, or as attachment points for metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhantomNode {
    /// A textual description of the phantom node's purpose.
    pub purpose: String,
}

/// Data specific to a vtable node for dynamic dispatch.
///
/// Represents a virtual method table used in trait object dispatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VTableNode {
    /// The trait name this vtable implements.
    pub trait_name: String,
    /// The concrete type the vtable is instantiated for.
    pub concrete_type: String,
    /// List of method entry node IDs in the vtable.
    pub method_entries: Vec<NodeId>,
}

/// Data specific to a closure environment node.
///
/// Represents the captured environment of a closure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClosureEnvNode {
    /// Names of captured variables.
    pub captured_vars: Vec<String>,
    /// Whether each capture is by move (true) or by borrow (false).
    pub capture_modes: Vec<bool>,
    /// The NodeId of the closure entry this environment belongs to.
    pub closure_entry: Option<NodeId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_id_creation_and_display() {
        let id = NodeId::new(42);
        assert_eq!(id.as_u64(), 42);
        assert_eq!(format!("{id}"), "NodeId(42)");
    }

    #[test]
    fn test_node_type_display() {
        assert_eq!(format!("{}", NodeType::Computation), "Computation");
        assert_eq!(format!("{}", NodeType::Allocation), "Allocation");
        assert_eq!(format!("{}", NodeType::Control), "Control");
    }

    #[test]
    fn test_allocation_node() {
        let region = RegionId::new(1);
        let alloc = AllocationNode {
            size: 1024,
            align: 8,
            region_id: region,
            type_name: Some("MyStruct".to_string()),
        };
        assert_eq!(alloc.size, 1024);
        assert_eq!(alloc.align, 8);
    }

    #[test]
    fn test_access_node_modes() {
        let region = RegionId::new(1);
        let read_access = AccessNode {
            mode: AccessMode::Read,
            region_id: region,
            offset: Some(0),
            access_size: Some(4),
        };
        assert_eq!(read_access.mode, AccessMode::Read);
    }

    #[test]
    fn test_cast_node() {
        let cast = CastNode {
            from_type: "i32".to_string(),
            to_type: "i64".to_string(),
            is_lossless: true,
        };
        assert!(cast.is_lossless);
    }
}
