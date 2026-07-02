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
    /// A struct definition node (type declaration).
    StructDef,
    /// An enum definition node (type declaration).
    EnumDef,
    /// A pattern match node (control flow via discriminant dispatch).
    Match,
    /// A constant-time security operation node (ct_select, ct_eq).
    ConstantTime,
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
            NodeType::StructDef => write!(f, "StructDef"),
            NodeType::EnumDef => write!(f, "EnumDef"),
            NodeType::Match => write!(f, "Match"),
            NodeType::ConstantTime => write!(f, "ConstantTime"),
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
    /// Payload for `NodeType::StructDef`.
    StructDef(StructDefNode),
    /// Payload for `NodeType::EnumDef`.
    EnumDef(EnumDefNode),
    /// Payload for `NodeType::Match`.
    Match(MatchNode),
    /// Payload for `NodeType::ConstantTime`.
    ConstantTime(ConstantTimeNode),
}

/// The kind of computation performed by a [`ComputationNode`].
///
/// This enum classifies computation into broad categories. The generic
/// `Other` variant preserves backward compatibility for arbitrary
/// operations expressed as strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ComputationKind {
    /// A generic / unclassified operation (backward-compatible).
    Other(String),
    /// Struct field access: loads or stores a field at a known byte offset
    /// from a struct base pointer.
    StructAccess {
        /// Name of the struct type.
        struct_name: String,
        /// Name of the field being accessed.
        field_name: String,
        /// Byte offset of the field within the struct.
        field_offset: u64,
        /// Size of the field in bytes.
        field_size: u64,
    },
    /// Enum tag read: extracts the discriminant from a tagged union.
    EnumTag {
        /// Name of the enum type.
        enum_name: String,
        /// Type of the tag (discriminant), e.g. `"u32"`.
        tag_type: String,
        /// Size of the tag in bytes.
        tag_size: u64,
    },
    /// Match dispatch: selects one of several arms based on comparing
    /// a discriminant value. Lowered to if/else chains in the codegen.
    MatchNode {
        /// Number of match arms.
        arm_count: usize,
        /// Type of the scrutinee (subject expression).
        subject_type: String,
    },
}

impl ComputationKind {
    /// Returns a human-readable name for this computation kind.
    pub fn label(&self) -> String {
        match self {
            ComputationKind::Other(op) => op.clone(),
            ComputationKind::StructAccess { struct_name, field_name, .. } => {
                format!("struct_access::{}::{}", struct_name, field_name)
            }
            ComputationKind::EnumTag { enum_name, .. } => {
                format!("enum_tag::{}", enum_name)
            }
            ComputationKind::MatchNode { arm_count, .. } => {
                format!("match_dispatch({}_arms)", arm_count)
            }
        }
    }
}

/// Data specific to a computation node.
///
/// Represents a pure computational operation such as arithmetic,
/// function invocation, or data transformation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ComputationNode {
    /// The specific kind of computation performed.
    ///
    /// When set to `ComputationKind::Other(s)`, this is backward-compatible
    /// with the old string-based `operation` field.
    pub kind: ComputationKind,
    /// An optional type signature for the computation's result.
    pub result_type: Option<String>,
    /// Whether this computation is a tail call (the last action before return).
    /// Set by the TailCallOptimization transform.
    pub tail_call: bool,
}

/// Helper struct for deserializing `ComputationNode` with backward compatibility
/// for the old `operation` string field.
#[derive(Deserialize)]
struct ComputationNodeHelper {
    kind: Option<ComputationKind>,
    /// Legacy field: the old string-based operation name.
    /// If `kind` is absent but `operation` is present, it is converted to
    /// `ComputationKind::Other(operation)`.
    operation: Option<String>,
    result_type: Option<String>,
    #[serde(default)]
    tail_call: bool,
}

impl<'de> Deserialize<'de> for ComputationNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let helper = ComputationNodeHelper::deserialize(deserializer)?;
        let kind = helper
            .kind
            .or_else(|| helper.operation.map(ComputationKind::Other))
            .ok_or_else(|| serde::de::Error::missing_field("kind"))?;
        Ok(ComputationNode {
            kind,
            result_type: helper.result_type,
            tail_call: helper.tail_call,
        })
    }
}

impl ComputationNode {
    /// Create a new ComputationNode with a generic (string) operation.
    ///
    /// This is a convenience constructor that wraps the operation string
    /// in `ComputationKind::Other`.
    pub fn new(operation: &str, result_type: Option<String>, tail_call: bool) -> Self {
        Self {
            kind: ComputationKind::Other(operation.to_string()),
            result_type,
            tail_call,
        }
    }

    /// Returns the operation label as a string.
    ///
    /// For backward compatibility, this returns the inner string for
    /// `ComputationKind::Other` and a generated label for specific kinds.
    pub fn operation(&self) -> String {
        self.kind.label()
    }

    // ── Typed classification accessors ────────────────────────────
    //
    // These methods classify the `ComputationKind::Other(label)` string
    // into typed categories, replacing ad-hoc `strip_prefix` parsing
    // scattered throughout `src/pipeline.rs`.
    //
    // Each method returns `Option<T>` — `None` means the label doesn't
    // match that category.

    /// Returns `Some(name)` if this node is a parameter declaration
    /// (label format: `"param <name>"`).
    pub fn as_param(&self) -> Option<&str> {
        if let ComputationKind::Other(label) = &self.kind {
            label.strip_prefix("param ").map(|s| s.trim())
        } else {
            None
        }
    }

    /// Returns `Some(value)` if this node is a literal constant
    /// (label format: `"lit_<number>"` or `"lit_true"` / `"lit_false"`).
    pub fn as_literal(&self) -> Option<i64> {
        if let ComputationKind::Other(label) = &self.kind {
            if let Some(num_str) = label.strip_prefix("lit_") {
                if let Ok(num) = num_str.parse::<i64>() {
                    return Some(num);
                }
                if num_str == "true" {
                    return Some(1);
                }
                if num_str == "false" {
                    return Some(0);
                }
            }
            // Also try parsing the label directly as a number
            if let Ok(num) = label.parse::<i64>() {
                return Some(num);
            }
        }
        None
    }

    /// Returns `Some(())` if this node is a parameter or literal
    /// (i.e., should be skipped during statement generation).
    pub fn is_param_or_literal(&self) -> bool {
        self.as_param().is_some() || self.as_literal().is_some()
    }

    /// Returns `Some(name)` if this node is a parameter declaration,
    /// returning the trimmed parameter name.
    pub fn param_name(&self) -> Option<String> {
        self.as_param().map(|s| s.to_string())
    }
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

/// Data specific to a struct definition node.
///
/// Represents a struct type declaration with named fields. Structs are lowered
/// to flat memory layouts where fields are laid out sequentially with proper
/// alignment padding between fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructDefNode {
    /// Struct name.
    pub name: String,
    /// Fields in declaration order: (name, type_name, byte_offset, byte_size).
    /// Offsets are computed during layout resolution.
    pub fields: Vec<StructFieldInfo>,
    /// Total size in bytes (including tail padding).
    pub total_size: u64,
    /// Alignment requirement in bytes.
    pub alignment: u64,
}

/// Information about a single field in a struct definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructFieldInfo {
    /// Field name.
    pub name: String,
    /// Type name of the field.
    pub type_name: String,
    /// Byte offset from the start of the struct.
    pub offset: u64,
    /// Size of the field in bytes.
    pub size: u64,
}

/// Data specific to an enum definition node.
///
/// Represents an enum type declaration with named variants. Enums are lowered
/// to tagged unions: a discriminant (tag) field followed by a union of the
/// variant payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumDefNode {
    /// Enum name.
    pub name: String,
    /// Variants in declaration order.
    pub variants: Vec<EnumVariantInfo>,
    /// Tag type (discriminant size, typically u32 or u8).
    pub tag_type: String,
    /// Size of the tag field in bytes.
    pub tag_size: u64,
    /// Size of the largest payload in bytes.
    pub max_payload_size: u64,
    /// Total size of the tagged union in bytes (tag + payload + padding).
    pub total_size: u64,
    /// Alignment requirement in bytes.
    pub alignment: u64,
}

/// Information about a single variant in an enum definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumVariantInfo {
    /// Variant name.
    pub name: String,
    /// Discriminant value (index).
    pub discriminant: u64,
    /// Optional payload type name.
    pub payload_type: Option<String>,
    /// Size of the payload in bytes (0 if no payload).
    pub payload_size: u64,
}

/// Data specific to a pattern match node.
///
/// Represents a match expression that dispatches control flow based on
/// the discriminant of an enum value, with optional payload extraction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchNode {
    /// The expression being matched (the discriminant).
    pub subject: String,
    /// The match arms.
    pub arms: Vec<MatchArmInfo>,
    /// Type of the subject expression (for enum tag dispatch).
    pub subject_type: String,
}

/// Information about a single arm of a match expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchArmInfo {
    /// Pattern to match against.
    pub pattern: MatchPatternInfo,
    /// Optional guard expression.
    pub guard: Option<String>,
    /// Body statements for this arm.
    pub body: Vec<String>,
}

/// Pattern information for a match arm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MatchPatternInfo {
    /// Wildcard pattern: `_`
    Wildcard,
    /// Literal pattern: matches an exact integer value.
    Lit(i64),
    /// Identifier pattern: binds the value to a name.
    Ident(String),
    /// Enum variant pattern: `Some(v)` or `None`
    Enum {
        /// Variant name.
        variant: String,
        /// Optional binding for the variant payload.
        binding: Option<String>,
    },
    /// Struct pattern: `Point { x, y }`
    Struct {
        /// Struct name.
        name: String,
        /// Field bindings.
        fields: Vec<String>,
    },
    /// Or pattern: `1 | 2 | 3`
    Or(Vec<MatchPatternInfo>),
}

/// Data specific to a constant-time security operation node.
///
/// Represents operations that execute in constant time to prevent
/// timing side-channel attacks. These are lowered to branch-free
/// bitwise operations in the backends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstantTimeNode {
    /// The constant-time operation kind.
    pub op: ConstantTimeOp,
    /// Result variable name.
    pub dst: String,
    /// Operand variable names.
    pub operands: Vec<String>,
}

/// Kinds of constant-time operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConstantTimeOp {
    /// Constant-time conditional select: `ct_select(cond, a, b)`.
    CtSelect,
    /// Constant-time equality check: `ct_eq(a, b)`.
    CtEq,
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

    #[test]
    fn test_computation_kind_other() {
        let kind = ComputationKind::Other("add".to_string());
        assert_eq!(kind.label(), "add");
    }

    #[test]
    fn test_computation_kind_struct_access() {
        let kind = ComputationKind::StructAccess {
            struct_name: "Point".to_string(),
            field_name: "x".to_string(),
            field_offset: 0,
            field_size: 4,
        };
        assert_eq!(kind.label(), "struct_access::Point::x");
    }

    #[test]
    fn test_computation_kind_enum_tag() {
        let kind = ComputationKind::EnumTag {
            enum_name: "Option".to_string(),
            tag_type: "u32".to_string(),
            tag_size: 4,
        };
        assert_eq!(kind.label(), "enum_tag::Option");
    }

    #[test]
    fn test_computation_kind_match_node() {
        let kind = ComputationKind::MatchNode {
            arm_count: 3,
            subject_type: "Option".to_string(),
        };
        assert_eq!(kind.label(), "match_dispatch(3_arms)");
    }

    #[test]
    fn test_computation_node_new() {
        let node = ComputationNode::new("mul", Some("i64".to_string()), false);
        assert_eq!(node.operation(), "mul");
        assert_eq!(node.result_type, Some("i64".to_string()));
        assert!(!node.tail_call);
    }

    #[test]
    fn test_computation_node_struct_access() {
        let node = ComputationNode {
            kind: ComputationKind::StructAccess {
                struct_name: "Rect".to_string(),
                field_name: "w".to_string(),
                field_offset: 8,
                field_size: 4,
            },
            result_type: Some("u32".to_string()),
            tail_call: false,
        };
        assert_eq!(node.operation(), "struct_access::Rect::w");
    }

    #[test]
    fn test_computation_node_enum_tag() {
        let node = ComputationNode {
            kind: ComputationKind::EnumTag {
                enum_name: "Result".to_string(),
                tag_type: "u32".to_string(),
                tag_size: 4,
            },
            result_type: Some("u32".to_string()),
            tail_call: false,
        };
        assert_eq!(node.operation(), "enum_tag::Result");
    }

    #[test]
    fn test_computation_node_match_node() {
        let node = ComputationNode {
            kind: ComputationKind::MatchNode {
                arm_count: 2,
                subject_type: "Option".to_string(),
            },
            result_type: Some("i32".to_string()),
            tail_call: false,
        };
        assert_eq!(node.operation(), "match_dispatch(2_arms)");
    }
}

// ---------------------------------------------------------------------------
// NodeVisitor trait — central dispatch to eliminate DRY violations
// ---------------------------------------------------------------------------

/// A visitor trait for dispatching on `NodePayload` without requiring
/// every consumer to match all 25+ variants.
///
/// This eliminates the "11 duplicated match statements" problem identified
/// in the codebase audit. New `NodeType` variants only need to be added
/// here; consumers implement `visit_default()` for fallback behavior.
///
/// ## Usage
///
/// ```ignore
/// struct MyVisitor;
/// impl NodeVisitor for MyVisitor {
///     type Output = String;
///     fn visit_default(&mut self, _payload: &NodePayload) -> Self::Output {
///         "unknown".to_string()
///     }
///     fn visit_computation(&mut self, c: &ComputationNode) -> Self::Output {
///         c.kind.label()
///     }
///     // ... override only the variants you care about
/// }
///
/// let label = MyVisitor.dispatch(&node.payload);
/// ```
pub trait NodeVisitor {
    /// The output type of the visitor.
    type Output;

    /// Default fallback for any payload variant not explicitly handled.
    fn visit_default(&mut self, payload: &NodePayload) -> Self::Output;

    // ── Existing node types (override as needed) ──
    fn visit_computation(&mut self, c: &ComputationNode, payload: &NodePayload) -> Self::Output { let _ = c; self.visit_default(payload) }
    fn visit_allocation(&mut self, a: &AllocationNode, payload: &NodePayload) -> Self::Output { let _ = a; self.visit_default(payload) }
    fn visit_deallocation(&mut self, d: &DeallocationNode, payload: &NodePayload) -> Self::Output { let _ = d; self.visit_default(payload) }
    fn visit_access(&mut self, a: &AccessNode, payload: &NodePayload) -> Self::Output { let _ = a; self.visit_default(payload) }
    fn visit_cast(&mut self, c: &CastNode, payload: &NodePayload) -> Self::Output { let _ = c; self.visit_default(payload) }
    fn visit_effect(&mut self, e: &EffectNode, payload: &NodePayload) -> Self::Output { let _ = e; self.visit_default(payload) }
    fn visit_control(&mut self, c: &ControlNode, payload: &NodePayload) -> Self::Output { let _ = c; self.visit_default(payload) }
    fn visit_phantom(&mut self, p: &PhantomNode, payload: &NodePayload) -> Self::Output { let _ = p; self.visit_default(payload) }
    fn visit_vtable(&mut self, v: &VTableNode, payload: &NodePayload) -> Self::Output { let _ = v; self.visit_default(payload) }
    fn visit_closure_env(&mut self, c: &ClosureEnvNode, payload: &NodePayload) -> Self::Output { let _ = c; self.visit_default(payload) }

    /// Central dispatch — calls the appropriate visit_* method.
    /// This is the ONLY match statement that needs updating when a new
    /// NodePayload variant is added.
    fn dispatch(&mut self, payload: &NodePayload) -> Self::Output {
        match payload {
            NodePayload::Computation(c) => self.visit_computation(c, payload),
            NodePayload::Allocation(a) => self.visit_allocation(a, payload),
            NodePayload::Deallocation(d) => self.visit_deallocation(d, payload),
            NodePayload::Access(a) => self.visit_access(a, payload),
            NodePayload::Cast(c) => self.visit_cast(c, payload),
            NodePayload::Effect(e) => self.visit_effect(e, payload),
            NodePayload::Control(c) => self.visit_control(c, payload),
            NodePayload::Phantom(p) => self.visit_phantom(p, payload),
            NodePayload::VTable(v) => self.visit_vtable(v, payload),
            NodePayload::ClosureEnv(c) => self.visit_closure_env(c, payload),
            _ => self.visit_default(payload),
        }
    }
}
