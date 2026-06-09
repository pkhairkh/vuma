//! Shared type definitions for the COR crate.
//!
//! This module centralizes the identifiers and core data structures that are
//! referenced across multiple COR sub-modules. Keeping them in one place avoids
//! circular dependencies and makes the public API consistent.

// No external imports needed; all types in this module are plain data.

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
// SCG (Semantic Computation Graph) placeholder
// ---------------------------------------------------------------------------

/// A placeholder for the Semantic Computation Graph.
///
/// In the full VUMA system this type is provided by the `vuma-scg` crate.
/// The COR crate holds an `Arc<SCG>` so it can share the graph with other
/// subsystems without taking ownership. For compilation purposes we define a
/// minimal stub here; the `vuma-scg` crate will provide the real definition
/// and a `From` impl to swap it in.
#[derive(Debug, Default)]
pub struct SCG {
    /// Number of nodes in the graph (diagnostic).
    pub node_count: usize,
    /// Number of edges in the graph (diagnostic).
    pub edge_count: usize,
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
