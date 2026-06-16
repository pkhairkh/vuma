//! # VUMA Projection System
//!
//! The Projection System provides multiple ways to view, interact with, and edit
//! the Semantic Computation Graph (SCG) — the central intermediate representation
//! of the VUMA language.
//!
//! ## Projections
//!
//! - **Textual** — Renders SCG nodes as type-like annotations in a language style
//!   (Rust-like, C-like, or custom), suitable for code review and documentation.
//! - **Visual** — Produces ASCII/Unicode art diagrams of dataflow, message passing,
//!   and call graphs for CLI-based inspection.
//! - **Conversational** — Generates natural-language descriptions of program behavior,
//!   explains changes, and suggests modifications.
//! - **Diff** — Computes and describes differences between two SCG snapshots,
//!   producing human-readable summaries such as *"The authentication flow now
//!   requires 2FA for admin accounts"*.
//! - **Bidirectional** — Enables round-trip editing: apply textual edits back to the
//!   SCG while preserving semantics or explicitly flagging semantic changes.
//!
//! ## Core Types
//!
//! This crate defines lightweight placeholder types for the SCG, Behavioural Descriptor
//! (BD), and related structures so that it compiles independently. When the `vuma-scg`
//! and `vuma-bd` crates are available, these placeholders should be replaced with
//! re-exports.

pub mod bidirectional;
pub mod conversational;
pub mod diff;
pub mod scg_adapter;
pub mod textual;
pub mod verification;
pub mod visual;

// ── Re-exports ───────────────────────────────────────────────────────────────

pub use bidirectional::{
    BidirectionalEditor, BidirectionalProjection, ConflictTracker, EditError, ProjectionSource,
    SemanticFlag, VisualEdit,
};
pub use conversational::{
    session_from_scg, AIExplainerOutput, AggregatedResult, ConversationalProjection,
    ConversationalSession, SCGEdit, SuggestionEngine, Verbosity, Violation, ViolationSeverity,
};
pub use diff::{
    project_diff, project_diff_conversational, project_diff_visual, ChangeGroup, DiffProjection,
    ImpactLevel, SCGDiff,
};
pub use textual::{ProjectionStyle, TemplateEngine, TextualConfig, TextualProjection};
pub use visual::VisualProjection;

// ── Placeholder types ────────────────────────────────────────────────────────
// These types will be replaced by re-exports from vuma-scg / vuma-bd once those
// crates are available in the workspace.

/// Unique identifier for an SCG node.
pub type NodeId = u64;

/// Unique identifier for an SCG edge.
pub type EdgeId = u64;

/// Unique identifier for a region (group of nodes).
pub type RegionId = u64;

/// Unique identifier for a Behavioural Descriptor.
pub type BdId = u64;

/// A Behavioural Descriptor — metadata annotation on an SCG node.
///
/// BDs capture capabilities, memory layout, safety invariants, and other
/// semantic properties. In textual projection they are rendered as type-like
/// annotations (e.g. `@Send + 'static`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BehaviouralDescriptor {
    /// Unique identifier for this BD.
    pub id: BdId,
    /// Human-readable name (e.g. `"Send"`, `"Pin"`).
    pub name: String,
    /// Categorisation of the descriptor (e.g. capability, memory, safety).
    pub kind: BdKind,
    /// Optional parameter string (e.g. lifetime `"'static"`).
    pub parameter: Option<String>,
}

/// The kind / category of a Behavioural Descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BdKind {
    /// A capability (e.g. `Send`, `Sync`, `Unpin`).
    Capability,
    /// A memory layout property (e.g. `aligned`, `pinned`).
    MemoryLayout,
    /// A safety invariant (e.g. `unsafe_deref`, `noalias`).
    Safety,
    /// A relational property (e.g. `borrows_from(X)`).
    Relation,
    /// A custom / user-defined descriptor.
    Custom,
}

/// A node in the Semantic Computation Graph.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SCGNode {
    /// Unique identifier.
    pub id: NodeId,
    /// Human-readable label (e.g. `"auth_handler"`).
    pub label: String,
    /// The kind of computation this node represents.
    pub kind: NodeKind,
    /// Behavioural descriptors attached to this node.
    pub bds: Vec<BehaviouralDescriptor>,
    /// IDs of regions this node belongs to.
    pub regions: Vec<RegionId>,
}

/// The kind of an SCG node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum NodeKind {
    /// A function or closure entry point.
    Function,
    /// A data value / allocation.
    Value,
    /// A message-send operation.
    MessageSend,
    /// A message-receive operation.
    MessageReceive,
    /// A control-flow merge point.
    Merge,
    /// A side-effecting operation.
    Effect,
    /// A module / namespace boundary.
    Module,
    /// A memory allocation operation.
    Allocation,
    /// A memory deallocation operation.
    Deallocation,
    /// A memory access (read/write) operation.
    Access,
    /// A pure computation step.
    Computation,
}

/// A directed edge in the SCG.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SCGEdge {
    /// Unique identifier.
    pub id: EdgeId,
    /// Source node.
    pub source: NodeId,
    /// Target node.
    pub target: NodeId,
    /// The kind of dependency this edge represents.
    pub kind: EdgeKind,
}

/// The kind of an SCG edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum EdgeKind {
    /// Data-flow dependency.
    DataFlow,
    /// Control-flow edge.
    ControlFlow,
    /// Message-passing channel.
    Message,
    /// Borrow / lending relationship.
    Borrow,
    /// Call relationship.
    Call,
    /// Derivation / computed-from relationship.
    Derivation,
    /// Annotation / metadata attachment.
    Annotation,
}

/// A region — a named group of related nodes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SCGRegion {
    /// Unique identifier.
    pub id: RegionId,
    /// Human-readable name.
    pub name: String,
    /// Nodes belonging to this region.
    pub nodes: Vec<NodeId>,
}

/// The Semantic Computation Graph — the central IR of the VUMA language.
///
/// The SCG is a directed graph where nodes represent computations or values
/// and edges represent dependencies (data-flow, control-flow, messages, borrows).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SCG {
    /// All nodes, indexed by [`NodeId`].
    pub nodes: Vec<SCGNode>,
    /// All edges, indexed by [`EdgeId`].
    pub edges: Vec<SCGEdge>,
    /// All regions.
    pub regions: Vec<SCGRegion>,
}

impl SCG {
    /// Creates an empty SCG.
    pub fn empty() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            regions: Vec::new(),
        }
    }

    /// Looks up a node by its identifier.
    pub fn get_node(&self, id: NodeId) -> Option<&SCGNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Looks up a region by its identifier.
    pub fn get_region(&self, id: RegionId) -> Option<&SCGRegion> {
        self.regions.iter().find(|r| r.id == id)
    }

    /// Returns all edges originating from the given node.
    pub fn outgoing_edges(&self, node_id: NodeId) -> Vec<&SCGEdge> {
        self.edges.iter().filter(|e| e.source == node_id).collect()
    }

    /// Returns all edges targeting the given node.
    pub fn incoming_edges(&self, node_id: NodeId) -> Vec<&SCGEdge> {
        self.edges.iter().filter(|e| e.target == node_id).collect()
    }
}

/// An edit range within a textual projection.
///
/// Represented as byte offsets into the projection string.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EditRange {
    /// Start byte offset (inclusive).
    pub start: usize,
    /// End byte offset (exclusive).
    pub end: usize,
    /// The replacement text.
    pub replacement: String,
}
