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
    // ═══════════════════════════════════════════════════════════════════
    // WOMB DATA MODELS — LLM-native replacements for structs/unions/arrays
    // ═══════════════════════════════════════════════════════════════════
    /// A Concept declaration (replaces struct). Fields are relational edges,
    /// not fixed offsets. Layout is lazily inferred by the IVE.
    ConceptDecl,
    /// A field within a Concept. Connected to its ConceptDecl via RelD edges.
    ConceptField,
    /// An access to a Concept field. Triggers layout resolution.
    ConceptAccess,
    /// A Gestalt declaration (replaces union/enum). Tagless superposition
    /// with proof-based interpretation.
    GestaltDecl,
    /// An interpretation of a Gestalt as a specific variant.
    /// The IVE must prove the context guarantees this variant.
    GestaltInterpret,
    /// A context assertion used by the IVE to prove Gestalt safety.
    ContextAssert,
    /// A Manifold declaration (replaces arrays/tensors). Multi-dimensional
    /// spatial data with space-filling curve memory layout.
    ManifoldDecl,
    /// A query into a Manifold (N-dimensional coordinate → value).
    ManifoldQuery,
    /// A slice of a Manifold (sub-region extraction).
    ManifoldSlice,
    /// Attaches Aura metadata to a Concept or Manifold.
    AuraAttach,
    /// Queries Aura metadata at runtime.
    AuraQuery,
    /// Updates Aura metadata.
    AuraUpdate,
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
            NodeType::ConceptDecl => write!(f, "ConceptDecl"),
            NodeType::ConceptField => write!(f, "ConceptField"),
            NodeType::ConceptAccess => write!(f, "ConceptAccess"),
            NodeType::GestaltDecl => write!(f, "GestaltDecl"),
            NodeType::GestaltInterpret => write!(f, "GestaltInterpret"),
            NodeType::ContextAssert => write!(f, "ContextAssert"),
            NodeType::ManifoldDecl => write!(f, "ManifoldDecl"),
            NodeType::ManifoldQuery => write!(f, "ManifoldQuery"),
            NodeType::ManifoldSlice => write!(f, "ManifoldSlice"),
            NodeType::AuraAttach => write!(f, "AuraAttach"),
            NodeType::AuraQuery => write!(f, "AuraQuery"),
            NodeType::AuraUpdate => write!(f, "AuraUpdate"),
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
    // ═══════════════════════════════════════════════════════════════════
    // WOMB DATA MODELS
    // ═══════════════════════════════════════════════════════════════════
    /// Payload for `NodeType::ConceptDecl`.
    ConceptDecl(ConceptDeclNode),
    /// Payload for `NodeType::ConceptField`.
    ConceptField(ConceptFieldNode),
    /// Payload for `NodeType::ConceptAccess`.
    ConceptAccess(ConceptAccessNode),
    /// Payload for `NodeType::GestaltDecl`.
    GestaltDecl(GestaltDeclNode),
    /// Payload for `NodeType::GestaltInterpret`.
    GestaltInterpret(GestaltInterpretNode),
    /// Payload for `NodeType::ContextAssert`.
    ContextAssert(ContextAssertNode),
    /// Payload for `NodeType::ManifoldDecl`.
    ManifoldDecl(ManifoldDeclNode),
    /// Payload for `NodeType::ManifoldQuery`.
    ManifoldQuery(ManifoldQueryNode),
    /// Payload for `NodeType::ManifoldSlice`.
    ManifoldSlice(ManifoldSliceNode),
    /// Payload for `NodeType::AuraAttach`.
    AuraAttach(AuraAttachNode),
    /// Payload for `NodeType::AuraQuery`.
    AuraQuery(AuraQueryNode),
    /// Payload for `NodeType::AuraUpdate`.
    AuraUpdate(AuraUpdateNode),
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

// ═══════════════════════════════════════════════════════════════════════════
// MODEL 1: Concept — Relational data with lazy layout inference
// ═══════════════════════════════════════════════════════════════════════════

/// A Concept declaration. Fields are NOT laid out in memory until the IVE
/// resolves the access patterns. The Concept is a set of relational edges,
/// not a rigid memory block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConceptDeclNode {
    /// User-visible name of the Concept (e.g., "Point", "Entity").
    pub name: String,
    /// Names of the fields, in declaration order. The actual byte offsets
    /// are resolved lazily by the LayoutResolutionPass.
    pub field_names: Vec<String>,
    /// The region ID where instances of this Concept will be allocated.
    pub region_id: crate::region::RegionId,
    /// Layout strategy hint. The IVE may override this based on access patterns.
    pub layout_hint: ConceptLayoutHint,
    /// Set to true once the IVE has resolved the physical layout.
    pub layout_resolved: bool,
    /// Resolved byte offsets for each field (populated by LayoutResolutionPass).
    pub resolved_offsets: Vec<(String, u64)>,
    /// Total resolved size in bytes (0 until layout_resolved).
    pub resolved_size: u64,
    /// Resolved alignment requirement (0 until layout_resolved).
    pub resolved_align: u64,
}

/// Layout strategy for a Concept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConceptLayoutHint {
    /// Array of Structs (AoS) — fields packed together. Best when fields
    /// are accessed together.
    AoS,
    /// Struct of Arrays (SoA) — each field in a separate array. Best when
    /// fields are accessed independently in loops.
    SoA,
    /// Let the IVE decide based on access pattern analysis.
    Auto,
}

/// A field within a Concept. Connected to its ConceptDecl via a RelD edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConceptFieldNode {
    /// Name of the field.
    pub name: String,
    /// Name of the Concept this field belongs to.
    pub concept_name: String,
    /// Behavioral descriptor for the field's value type.
    /// Initially abstract; resolved during BD inference.
    pub repd: Option<String>,
    /// Access frequency counter, used by LayoutResolutionPass to decide
    /// AoS vs SoA. Incremented on each ConceptAccess.
    pub access_count: u64,
    /// True if this field is frequently accessed independently (SoA hint).
    pub independent_access: bool,
    /// True if this field is frequently accessed with other fields (AoS hint).
    pub co_accessed: bool,
}

/// An access to a Concept field. Triggers layout resolution if not yet done.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConceptAccessNode {
    /// The variable holding the Concept instance pointer.
    pub base_ptr: String,
    /// Name of the Concept type.
    pub concept_name: String,
    /// Name of the field being accessed.
    pub field_name: String,
    /// Result variable (where the loaded value goes, or value to store).
    pub result_var: String,
    /// True for write, false for read.
    pub is_write: bool,
    /// Resolved byte offset (populated after layout resolution).
    pub resolved_offset: Option<u64>,
    /// Access size in bytes (from the field's RepD).
    pub access_size: Option<u64>,
}

// ═══════════════════════════════════════════════════════════════════════════
// MODEL 2: Gestalt — Tagless, context-dependent memory superposition
// ═══════════════════════════════════════════════════════════════════════════

/// A Gestalt declaration. Memory is superimposed — the same bytes can be
/// interpreted as different variants depending on context. The IVE proves
/// which variant is active, eliminating runtime tags when possible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GestaltDeclNode {
    /// User-visible name of the Gestalt (e.g., "Message", "Event").
    pub name: String,
    /// All possible interpretations (variant names).
    pub variants: Vec<String>,
    /// The maximum byte size across all variants (for allocation).
    pub max_size: u64,
    /// The strictest alignment across all variants.
    pub max_align: u64,
    /// If true, the IVE could not prove all interpretations and has
    /// injected a hidden 1-byte runtime tag.
    pub degraded: bool,
    /// The byte offset of the injected tag (if degraded). Typically 0.
    pub tag_offset: Option<u64>,
}

/// An interpretation of a Gestalt as a specific variant.
/// The IVE must prove the context guarantees this variant is active.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GestaltInterpretNode {
    /// The variable holding the Gestalt instance pointer.
    pub base_ptr: String,
    /// Name of the Gestalt type.
    pub gestalt_name: String,
    /// The variant being interpreted as.
    pub variant_name: String,
    /// Result variable for the interpreted value.
    pub result_var: String,
    /// True if the IVE proved this interpretation is safe (no runtime check).
    pub proven_safe: bool,
    /// If degraded and not proven, a runtime tag check is emitted.
    pub requires_tag_check: bool,
}

/// A context assertion used by the IVE to prove Gestalt safety.
/// These are inserted by the compiler at branch points where the active
/// variant becomes known (e.g., after a match or if-else).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextAssertNode {
    /// Name of the Gestalt being asserted.
    pub gestalt_name: String,
    /// The variant asserted to be active.
    pub variant_name: String,
    /// The variable holding the Gestalt instance.
    pub base_ptr: String,
    /// A string representation of the proof condition.
    pub proof_condition: String,
}

// ═══════════════════════════════════════════════════════════════════════════
// MODEL 3: Manifold — Multi-dimensional spatial data with space-filling curves
// ═══════════════════════════════════════════════════════════════════════════

/// A Manifold declaration. Multi-dimensional data where memory layout
/// uses space-filling curves (Z-order or Hilbert) for cache locality.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifoldDeclNode {
    /// User-visible name of the Manifold (e.g., "Grid", "Tensor").
    pub name: String,
    /// Number of dimensions (e.g., 2 for a matrix, 3 for a volume).
    pub dimensions: u32,
    /// Size of each dimension (e.g., [4, 4] for a 4×4 grid).
    pub dim_sizes: Vec<u64>,
    /// Element size in bytes (e.g., 4 for u32).
    pub element_size: u64,
    /// Total number of elements (product of dim_sizes).
    pub total_elements: u64,
    /// Total buffer size in bytes.
    pub total_bytes: u64,
    /// The space-filling curve used for memory layout.
    pub curve: SpaceFillingCurve,
    /// Locality hints: which dimensions are frequently queried together.
    /// E.g., [(0, 1)] means dim 0 and dim 1 are co-queried.
    pub locality_hints: Vec<(u32, u32)>,
}

/// Space-filling curve type for Manifold memory layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SpaceFillingCurve {
    /// Z-order (Morton) curve — simplest, good cache locality for 2D.
    ZOrder,
    /// Hilbert curve — better locality than Z-order, more complex to compute.
    Hilbert,
    /// Row-major (standard) — for backward compatibility with plain arrays.
    RowMajor,
}

/// A query into a Manifold: given N-dimensional coordinates, load a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifoldQueryNode {
    /// The variable holding the Manifold base pointer.
    pub base_ptr: String,
    /// Name of the Manifold type.
    pub manifold_name: String,
    /// Coordinate values (one per dimension).
    pub coordinates: Vec<String>,
    /// Result variable for the loaded value.
    pub result_var: String,
    /// True for write, false for read.
    pub is_write: bool,
    /// The value to store (if write).
    pub value: Option<String>,
}

/// A slice of a Manifold: extract a sub-region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifoldSliceNode {
    /// The variable holding the source Manifold base pointer.
    pub src_ptr: String,
    /// Name of the Manifold type.
    pub manifold_name: String,
    /// Start coordinate for each dimension.
    pub start_coords: Vec<String>,
    /// End coordinate for each dimension.
    pub end_coords: Vec<String>,
    /// Result variable for the new Manifold (sub-region).
    pub result_ptr: String,
}

// ═══════════════════════════════════════════════════════════════════════════
// MODEL 4: Aura — Self-describing metadata for runtime introspection
// ═══════════════════════════════════════════════════════════════════════════

/// Attaches Aura metadata to a Concept or Manifold. The metadata is stored
/// in a hidden header before the base pointer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuraAttachNode {
    /// The variable holding the base pointer (Concept or Manifold).
    pub base_ptr: String,
    /// Schema hash — identifies the structure of the metadata.
    pub schema_hash: u64,
    /// Schema version — for forward/backward compatibility.
    pub version: u32,
    /// Total size of the base allocation (for bounds checking).
    pub bounds_size: u64,
    /// Name of the metadata schema (for debugging/introspection).
    pub schema_name: String,
    /// Result variable for the new pointer (with Aura header).
    pub result_ptr: String,
}

/// Queries Aura metadata at runtime. Returns a value from the AuraHeader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuraQueryNode {
    /// The variable holding the pointer (with Aura header).
    pub ptr: String,
    /// What metadata field to query.
    pub field: AuraField,
    /// Result variable for the queried value.
    pub result_var: String,
}

/// Fields available in the AuraHeader for runtime introspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AuraField {
    /// The schema hash identifying the metadata structure.
    SchemaHash,
    /// The schema version number.
    Version,
    /// The bounds size of the base allocation.
    BoundsSize,
    /// The schema name string (returned as a pointer).
    SchemaName,
}

/// Updates Aura metadata at runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuraUpdateNode {
    /// The variable holding the pointer (with Aura header).
    pub ptr: String,
    /// What metadata field to update.
    pub field: AuraField,
    /// The new value.
    pub value: String,
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
