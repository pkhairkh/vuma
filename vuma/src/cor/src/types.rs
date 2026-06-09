//! Shared type definitions for the COR crate.
//!
//! This module centralizes the identifiers and core data structures that are
//! referenced across multiple COR sub-modules. Keeping them in one place avoids
//! circular dependencies and makes the public API consistent.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Identifier types
// ---------------------------------------------------------------------------

/// Unique identifier for a node in the Semantic Computation Graph (SCG).
pub type NodeId = u64;

/// Unique identifier for a directed edge in the SCG.
pub type EdgeId = u64;

/// Unique identifier for a compiled region (subgraph) within the SCG.
pub type RegionId = u64;

// ---------------------------------------------------------------------------
// Node kinds
// ---------------------------------------------------------------------------

/// Classification of a node in the SCG.
///
/// The node kind determines which optimization passes are applicable.
/// For example, only `Loop` / `LoopHeader` nodes are candidates for loop
/// unrolling, and only `Call` nodes are candidates for inlining.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A function call site — candidate for inlining / outlining.
    Call,
    /// A loop header — candidate for unrolling / vectorization.
    Loop,
    /// A branch / conditional — candidate for likely-branch layout.
    Branch,
    /// A memory operation (load / store) — candidate for prefetch / alignment.
    Memory,
    /// A simple arithmetic / logic computation.
    Compute,
    /// An entry or exit node for a region.
    Entry,
    // Fine-grained control flow kinds (from vuma_scg::ControlKind):
    /// A loop header — identifies loop entry point for unrolling.
    LoopHeader,
    /// A loop exit — identifies loop termination.
    LoopExit,
    /// A join point — where control flow merges after branch/match.
    Join,
    /// A function entry — identifies function boundary.
    FunctionEntry,
    /// A function return — identifies function exit.
    FunctionReturn,
    /// A break/continue jump.
    Jump,
}

// ---------------------------------------------------------------------------
// SCGNode
// ---------------------------------------------------------------------------

/// A node in the Semantic Computation Graph.
///
/// Each node represents a discrete unit of computation (a call, a loop, a
/// branch, a memory operation, etc.). Optimization passes read and mutate
/// node metadata to reflect transformations such as inlining, outlining,
/// loop unrolling, vectorization, prefetch insertion, and cache-line
/// alignment.
#[derive(Debug, Clone)]
pub struct SCGNode {
    /// Unique identifier for this node.
    pub id: NodeId,
    /// Classification of the node.
    pub kind: NodeKind,
    /// Incoming edge IDs.
    pub incoming_edges: Vec<EdgeId>,
    /// Outgoing edge IDs.
    pub outgoing_edges: Vec<EdgeId>,
    /// Size in bytes of the compiled code for this node (0 if not compiled).
    pub code_size: usize,
    /// Whether this node has been inlined into its caller.
    pub is_inlined: bool,
    /// Whether this node has been outlined (moved to a separate cold function).
    pub is_outlined: bool,
    /// Loop unroll factor (1 = not unrolled; 2, 4, 8 = unrolled).
    pub unroll_factor: u32,
    /// Whether this node uses SIMD / vectorized instructions.
    pub is_vectorized: bool,
    /// Cache-line alignment requirement in bytes (0 = default, 64 = 64-byte
    /// aligned for Pi 5 L1 cache lines).
    pub alignment: usize,
    /// Whether prefetch instructions have been inserted for this node.
    pub has_prefetch: bool,
    /// For control nodes, stores the label from the SCG (e.g., "then",
    /// "else", "loop_header").
    pub control_label: Option<String>,
}

impl SCGNode {
    /// Creates a new node with the given ID and kind, with all optimization
    /// metadata set to their default (unoptimised) values.
    pub fn new(id: NodeId, kind: NodeKind) -> Self {
        SCGNode {
            id,
            kind,
            incoming_edges: Vec::new(),
            outgoing_edges: Vec::new(),
            code_size: 0,
            is_inlined: false,
            is_outlined: false,
            unroll_factor: 1,
            is_vectorized: false,
            alignment: 0,
            has_prefetch: false,
            control_label: None,
        }
    }
}

// ---------------------------------------------------------------------------
// SCGEdge
// ---------------------------------------------------------------------------

/// A directed edge in the Semantic Computation Graph.
///
/// Edges connect nodes and carry a weight that estimates the execution
/// frequency of the transition. Loop back-edges have a weight much greater
/// than 1, which the loop optimisation pass uses to identify hot loops.
#[derive(Debug, Clone)]
pub struct SCGEdge {
    /// Unique identifier for this edge.
    pub id: EdgeId,
    /// Source node.
    pub source: NodeId,
    /// Target node.
    pub target: NodeId,
    /// Estimated execution frequency (number of traversals).
    pub weight: u64,
}

impl SCGEdge {
    /// Creates a new edge from `source` to `target` with the given ID.
    pub fn new(id: EdgeId, source: NodeId, target: NodeId) -> Self {
        SCGEdge {
            id,
            source,
            target,
            weight: 1,
        }
    }
}

// ---------------------------------------------------------------------------
// SCG (Semantic Computation Graph)
// ---------------------------------------------------------------------------

/// The Semantic Computation Graph.
///
/// In the full VUMA system this type is provided by the `vuma-scg` crate.
/// The COR crate holds an `Arc<SCG>` so it can share the graph with other
/// subsystems without taking ownership. For compilation purposes we define a
/// working definition here; the `vuma-scg` crate will provide the real
/// definition and a `From` impl to swap it in.
#[derive(Debug, Clone)]
pub struct SCG {
    /// Number of nodes in the graph (diagnostic; kept in sync with `nodes`).
    pub node_count: usize,
    /// Number of edges in the graph (diagnostic; kept in sync with `edges`).
    pub edge_count: usize,
    /// Nodes indexed by [`NodeId`].
    pub nodes: HashMap<NodeId, SCGNode>,
    /// Edges indexed by [`EdgeId`].
    pub edges: HashMap<EdgeId, SCGEdge>,
}

impl SCG {
    /// Creates an empty SCG.
    pub fn new() -> Self {
        SCG {
            node_count: 0,
            edge_count: 0,
            nodes: HashMap::new(),
            edges: HashMap::new(),
        }
    }

    /// Inserts a node into the graph, updating `node_count`.
    pub fn insert_node(&mut self, node: SCGNode) {
        self.nodes.insert(node.id, node);
        self.node_count = self.nodes.len();
    }

    /// Inserts an edge into the graph, updating `edge_count`.
    pub fn insert_edge(&mut self, edge: SCGEdge) {
        self.edges.insert(edge.id, edge);
        self.edge_count = self.edges.len();
    }

    /// Returns a reference to the node with the given ID, if it exists.
    pub fn get_node(&self, id: NodeId) -> Option<&SCGNode> {
        self.nodes.get(&id)
    }

    /// Returns a mutable reference to the node with the given ID.
    pub fn get_node_mut(&mut self, id: NodeId) -> Option<&mut SCGNode> {
        self.nodes.get_mut(&id)
    }

    /// Returns a reference to the edge with the given ID, if it exists.
    pub fn get_edge(&self, id: EdgeId) -> Option<&SCGEdge> {
        self.edges.get(&id)
    }
}

impl Default for SCG {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Compiled region
// ---------------------------------------------------------------------------

/// A compiled region of the SCG, ready for execution.
///
/// `CompiledRegion` holds the machine code (or intermediate representation)
/// produced by the runtime for a specific subgraph of the SCG. The exact
/// representation is architecture-dependent and will be filled in by the
/// code generation layer.
#[derive(Debug, Clone)]
pub struct CompiledRegion {
    /// The region identifier this code was compiled from.
    pub region_id: RegionId,
    /// Architecture-specific code bytes (placeholder).
    pub code: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Delta (incremental change)
// ---------------------------------------------------------------------------

/// Describes an incremental change (delta) to the SCG.
///
/// When the program evolves (e.g. a new definition is added or an existing
/// one is modified), the SCG is updated incrementally. A `Delta` captures
/// just the diff so the COR can recompile only the affected regions.
#[derive(Debug, Clone)]
pub struct Delta {
    /// Nodes that were added.
    pub added_nodes: Vec<NodeId>,
    /// Nodes that were removed.
    pub removed_nodes: Vec<NodeId>,
    /// Edges that were added.
    pub added_edges: Vec<EdgeId>,
    /// Edges that were removed.
    pub removed_edges: Vec<EdgeId>,
}

impl Delta {
    /// Creates an empty delta (no changes).
    pub fn empty() -> Self {
        Delta {
            added_nodes: Vec::new(),
            removed_nodes: Vec::new(),
            added_edges: Vec::new(),
            removed_edges: Vec::new(),
        }
    }

    /// Returns `true` if this delta contains no changes.
    pub fn is_empty(&self) -> bool {
        self.added_nodes.is_empty()
            && self.removed_nodes.is_empty()
            && self.added_edges.is_empty()
            && self.removed_edges.is_empty()
    }
}
