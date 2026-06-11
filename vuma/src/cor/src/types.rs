//! Shared type definitions for the COR crate.
//!
//! This module centralizes the identifiers and core data structures that are
//! referenced across multiple COR sub-modules. Keeping them in one place avoids
//! circular dependencies and makes the public API consistent.

use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
// Delta (incremental change) — field-level change tracking
// ---------------------------------------------------------------------------

/// A single field-level change within a node or edge.
///
/// `FieldChange` records the name of the field that changed, its value
/// before the change, and its value after. This enables the COR to make
/// fine-grained recompilation decisions — e.g. only re-optimize a node
/// whose `is_inlined` flag flipped, without touching unrelated nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldChange {
    /// Name of the field that changed.
    pub field_name: String,
    /// Value of the field before the change.
    pub old_value: String,
    /// Value of the field after the change.
    pub new_value: String,
}

/// Information about an added node, carrying both its ID and optionally
/// the node kind and code size for richer delta tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDelta {
    /// The unique identifier of the added node.
    pub node_id: NodeId,
    /// The kind of the added node, if known at delta construction time.
    pub kind: Option<NodeKind>,
    /// Code size of the added node, if known at delta construction time.
    pub code_size: Option<usize>,
}

impl NodeDelta {
    /// Creates a `NodeDelta` from just a node ID.
    pub fn from_id(id: NodeId) -> Self {
        NodeDelta {
            node_id: id,
            kind: None,
            code_size: None,
        }
    }
}

impl From<NodeId> for NodeDelta {
    fn from(id: NodeId) -> Self {
        NodeDelta::from_id(id)
    }
}

/// Information about an added edge, carrying both its ID and optionally
/// the source/target nodes for richer delta tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeDelta {
    /// The unique identifier of the added edge.
    pub edge_id: EdgeId,
    /// Source node of the added edge, if known at delta construction time.
    pub source: Option<NodeId>,
    /// Target node of the added edge, if known at delta construction time.
    pub target: Option<NodeId>,
}

impl EdgeDelta {
    /// Creates an `EdgeDelta` from just an edge ID.
    pub fn from_id(id: EdgeId) -> Self {
        EdgeDelta {
            edge_id: id,
            source: None,
            target: None,
        }
    }
}

impl From<EdgeId> for EdgeDelta {
    fn from(id: EdgeId) -> Self {
        EdgeDelta::from_id(id)
    }
}

/// Describes field-level modifications to a single node.
///
/// When a node's metadata changes (e.g. `is_inlined` goes from `false` to
/// `true`), the delta records each changed field as a [`FieldChange`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeModification {
    /// The ID of the modified node.
    pub node_id: u64,
    /// The individual field-level changes.
    pub field_changes: Vec<FieldChange>,
}

/// Describes field-level modifications to a single edge.
///
/// When an edge's metadata changes (e.g. `weight` increases from 1 to 100),
/// the delta records the change as a [`FieldChange`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeModification {
    /// The ID of the modified edge.
    pub edge_id: u64,
    /// The individual field-level changes.
    pub field_changes: Vec<FieldChange>,
}

/// Describes changes to a compiled region.
///
/// When a region's metadata changes (e.g. its optimization label or
/// deployment target), the delta records the change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionDelta {
    /// The region that changed.
    pub region_id: RegionId,
    /// The individual field-level changes.
    pub field_changes: Vec<FieldChange>,
}

/// Describes an incremental change (delta) to the SCG.
///
/// When the program evolves (e.g. a new definition is added or an existing
/// one is modified), the SCG is updated incrementally. A `Delta` captures
/// just the diff so the COR can recompile only the affected regions.
///
/// # Field-level tracking
///
/// In addition to tracking which nodes/edges were added or removed, the
/// delta now records *which fields* of a node or edge changed. This enables
/// fine-grained incremental recompilation: if only `is_inlined` changed on
/// a node, the recompilation can skip code-generation passes that are
/// unrelated to inlining.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delta {
    /// Nodes that were added (IDs only for backward compatibility; use
    /// [`NodeDelta`] for richer information).
    pub added_nodes: Vec<NodeId>,
    /// Nodes that were removed.
    pub removed_nodes: Vec<NodeId>,
    /// Nodes whose fields were modified (field-level granularity).
    pub modified_nodes: Vec<NodeModification>,
    /// Edges that were added (IDs only for backward compatibility; use
    /// [`EdgeDelta`] for richer information).
    pub added_edges: Vec<EdgeId>,
    /// Edges that were removed.
    pub removed_edges: Vec<EdgeId>,
    /// Edges whose fields were modified (field-level granularity).
    pub modified_edges: Vec<EdgeModification>,
    /// Region-level changes (e.g. optimization label, deployment target).
    pub region_changes: Vec<RegionDelta>,
}

impl Delta {
    /// Creates an empty delta (no changes).
    pub fn empty() -> Self {
        Delta {
            added_nodes: Vec::new(),
            removed_nodes: Vec::new(),
            modified_nodes: Vec::new(),
            added_edges: Vec::new(),
            removed_edges: Vec::new(),
            modified_edges: Vec::new(),
            region_changes: Vec::new(),
        }
    }

    /// Returns `true` if this delta contains no changes.
    pub fn is_empty(&self) -> bool {
        self.added_nodes.is_empty()
            && self.removed_nodes.is_empty()
            && self.modified_nodes.is_empty()
            && self.added_edges.is_empty()
            && self.removed_edges.is_empty()
            && self.modified_edges.is_empty()
            && self.region_changes.is_empty()
    }

    /// Returns the total number of individual field changes across all
    /// modified nodes and edges.
    pub fn total_field_changes(&self) -> usize {
        let node_changes: usize = self.modified_nodes.iter().map(|m| m.field_changes.len()).sum();
        let edge_changes: usize = self.modified_edges.iter().map(|m| m.field_changes.len()).sum();
        let region_changes: usize = self.region_changes.iter().map(|r| r.field_changes.len()).sum();
        node_changes + edge_changes + region_changes
    }
}

impl Default for Delta {
    fn default() -> Self {
        Delta::empty()
    }
}

// ---------------------------------------------------------------------------
// diff_nodes — field-level diffing
// ---------------------------------------------------------------------------

/// Computes field-level differences between two nodes.
///
/// Given an old and a new version of the same node, this function compares
/// every field and returns a [`FieldChange`] for each field that differs.
/// The node `id` itself is **not** compared — it is assumed that both
/// nodes have the same ID (callers should verify this).
///
/// # Example
///
/// ```
/// use vuma_cor::types::{SCGNode, NodeKind, diff_nodes};
///
/// let old = SCGNode::new(1, NodeKind::Compute);
/// let mut new = old.clone();
/// new.is_inlined = true;
///
/// let changes = diff_nodes(&old, &new);
/// assert_eq!(changes.len(), 1);
/// assert_eq!(changes[0].field_name, "is_inlined");
/// assert_eq!(changes[0].old_value, "false");
/// assert_eq!(changes[0].new_value, "true");
/// ```
pub fn diff_nodes(old: &SCGNode, new: &SCGNode) -> Vec<FieldChange> {
    let mut changes = Vec::new();

    macro_rules! field_diff {
        ($field:expr, $old:expr, $new:expr) => {
            let old_str = $old.to_string();
            let new_str = $new.to_string();
            if old_str != new_str {
                changes.push(FieldChange {
                    field_name: $field.to_owned(),
                    old_value: old_str,
                    new_value: new_str,
                });
            }
        };
    }

    field_diff!("kind", format!("{:?}", old.kind), format!("{:?}", new.kind));

    // Incoming / outgoing edges — compare as sorted comma-separated IDs.
    field_diff!(
        "incoming_edges",
        format!("{:?}", old.incoming_edges),
        format!("{:?}", new.incoming_edges)
    );
    field_diff!(
        "outgoing_edges",
        format!("{:?}", old.outgoing_edges),
        format!("{:?}", new.outgoing_edges)
    );

    field_diff!("code_size", old.code_size, new.code_size);
    field_diff!("is_inlined", old.is_inlined, new.is_inlined);
    field_diff!("is_outlined", old.is_outlined, new.is_outlined);
    field_diff!("unroll_factor", old.unroll_factor, new.unroll_factor);
    field_diff!("is_vectorized", old.is_vectorized, new.is_vectorized);
    field_diff!("alignment", old.alignment, new.alignment);
    field_diff!("has_prefetch", old.has_prefetch, new.has_prefetch);

    // control_label: compare Option<String>
    field_diff!(
        "control_label",
        old.control_label.as_deref().unwrap_or("None"),
        new.control_label.as_deref().unwrap_or("None")
    );

    changes
}

/// Computes field-level differences between two edges.
///
/// Given an old and a new version of the same edge, this function compares
/// every field and returns a [`FieldChange`] for each field that differs.
/// The edge `id` itself is **not** compared.
pub fn diff_edges(old: &SCGEdge, new: &SCGEdge) -> Vec<FieldChange> {
    let mut changes = Vec::new();

    if old.source != new.source {
        changes.push(FieldChange {
            field_name: "source".to_owned(),
            old_value: old.source.to_string(),
            new_value: new.source.to_string(),
        });
    }
    if old.target != new.target {
        changes.push(FieldChange {
            field_name: "target".to_owned(),
            old_value: old.target.to_string(),
            new_value: new.target.to_string(),
        });
    }
    if old.weight != new.weight {
        changes.push(FieldChange {
            field_name: "weight".to_owned(),
            old_value: old.weight.to_string(),
            new_value: new.weight.to_string(),
        });
    }

    changes
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_modification_detects_field_changes() {
        // Create two nodes with the same ID but different field values.
        let old = SCGNode::new(1, NodeKind::Compute);
        let mut new = old.clone();

        // Change several fields.
        new.is_inlined = true;
        new.unroll_factor = 4;
        new.code_size = 128;

        // Compute the diff.
        let changes = diff_nodes(&old, &new);

        // Should detect exactly the three changed fields.
        assert_eq!(changes.len(), 3, "expected 3 field changes, got {:?}", changes);

        // Verify each change.
        let change_map: std::collections::HashMap<&str, &FieldChange> = changes
            .iter()
            .map(|c| (c.field_name.as_str(), c))
            .collect();

        assert!(change_map.contains_key("is_inlined"));
        assert_eq!(change_map["is_inlined"].old_value, "false");
        assert_eq!(change_map["is_inlined"].new_value, "true");

        assert!(change_map.contains_key("unroll_factor"));
        assert_eq!(change_map["unroll_factor"].old_value, "1");
        assert_eq!(change_map["unroll_factor"].new_value, "4");

        assert!(change_map.contains_key("code_size"));
        assert_eq!(change_map["code_size"].old_value, "0");
        assert_eq!(change_map["code_size"].new_value, "128");
    }

    #[test]
    fn test_delta_field_level_diff() {
        // Build two nodes, diff them, and construct a Delta with the
        // modifications.
        let old_node = SCGNode::new(42, NodeKind::Call);
        let mut new_node = old_node.clone();
        new_node.is_inlined = true;
        new_node.is_outlined = false; // same as default, should NOT appear

        let field_changes = diff_nodes(&old_node, &new_node);
        assert_eq!(field_changes.len(), 1, "only is_inlined should differ");

        // Construct a Delta with the modification.
        let delta = Delta {
            modified_nodes: vec![NodeModification {
                node_id: 42,
                field_changes,
            }],
            ..Delta::empty()
        };

        // Verify delta is not empty and contains the right modification.
        assert!(!delta.is_empty());
        assert_eq!(delta.modified_nodes.len(), 1);
        assert_eq!(delta.modified_nodes[0].node_id, 42);
        assert_eq!(delta.modified_nodes[0].field_changes.len(), 1);
        assert_eq!(delta.modified_nodes[0].field_changes[0].field_name, "is_inlined");
        assert_eq!(delta.total_field_changes(), 1);

        // An empty delta should be empty.
        let empty = Delta::empty();
        assert!(empty.is_empty());
        assert_eq!(empty.total_field_changes(), 0);
    }

    #[test]
    fn test_diff_nodes_no_changes() {
        let node = SCGNode::new(5, NodeKind::Memory);
        let changes = diff_nodes(&node, &node);
        assert!(changes.is_empty(), "identical nodes should have no changes");
    }

    #[test]
    fn test_diff_nodes_kind_change() {
        let old = SCGNode::new(10, NodeKind::Compute);
        let new = SCGNode::new(10, NodeKind::Call);
        let changes = diff_nodes(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field_name, "kind");
        assert_eq!(changes[0].old_value, "Compute");
        assert_eq!(changes[0].new_value, "Call");
    }

    #[test]
    fn test_diff_edges_weight_change() {
        let old = SCGEdge::new(1, 10, 20);
        let mut new = old.clone();
        new.weight = 100;
        let changes = diff_edges(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field_name, "weight");
        assert_eq!(changes[0].old_value, "1");
        assert_eq!(changes[0].new_value, "100");
    }

    #[test]
    fn test_diff_edges_no_changes() {
        let edge = SCGEdge::new(1, 10, 20);
        let changes = diff_edges(&edge, &edge);
        assert!(changes.is_empty(), "identical edges should have no changes");
    }

    #[test]
    fn test_delta_with_multiple_modifications() {
        // Node modification.
        let old_node = SCGNode::new(1, NodeKind::Loop);
        let mut new_node = old_node.clone();
        new_node.unroll_factor = 8;
        new_node.is_vectorized = true;

        // Edge modification.
        let old_edge = SCGEdge::new(100, 1, 2);
        let mut new_edge = old_edge.clone();
        new_edge.weight = 5000;

        let delta = Delta {
            added_nodes: vec![50, 51],
            removed_nodes: vec![99],
            modified_nodes: vec![NodeModification {
                node_id: 1,
                field_changes: diff_nodes(&old_node, &new_node),
            }],
            added_edges: vec![200],
            removed_edges: vec![300],
            modified_edges: vec![EdgeModification {
                edge_id: 100,
                field_changes: diff_edges(&old_edge, &new_edge),
            }],
            region_changes: vec![RegionDelta {
                region_id: 1,
                field_changes: vec![FieldChange {
                    field_name: "optimization_label".to_owned(),
                    old_value: "basic".to_owned(),
                    new_value: "aggressive".to_owned(),
                }],
            }],
        };

        assert!(!delta.is_empty());
        assert_eq!(delta.added_nodes.len(), 2);
        assert_eq!(delta.removed_nodes.len(), 1);
        assert_eq!(delta.modified_nodes.len(), 1);
        assert_eq!(delta.modified_nodes[0].field_changes.len(), 2);
        assert_eq!(delta.modified_edges.len(), 1);
        assert_eq!(delta.modified_edges[0].field_changes.len(), 1);
        assert_eq!(delta.region_changes.len(), 1);

        // total_field_changes: 2 (node) + 1 (edge) + 1 (region) = 4
        assert_eq!(delta.total_field_changes(), 4);
    }

    #[test]
    fn test_node_delta_from_id() {
        let nd = NodeDelta::from_id(42);
        assert_eq!(nd.node_id, 42);
        assert_eq!(nd.kind, None);
        assert_eq!(nd.code_size, None);
    }

    #[test]
    fn test_edge_delta_from_id() {
        let ed = EdgeDelta::from_id(99);
        assert_eq!(ed.edge_id, 99);
        assert_eq!(ed.source, None);
        assert_eq!(ed.target, None);
    }

    #[test]
    fn test_field_change_equality() {
        let fc1 = FieldChange {
            field_name: "is_inlined".to_owned(),
            old_value: "false".to_owned(),
            new_value: "true".to_owned(),
        };
        let fc2 = FieldChange {
            field_name: "is_inlined".to_owned(),
            old_value: "false".to_owned(),
            new_value: "true".to_owned(),
        };
        assert_eq!(fc1, fc2);
    }

    #[test]
    fn test_delta_default_impl() {
        let delta = Delta::default();
        assert!(delta.is_empty());
        assert_eq!(delta.total_field_changes(), 0);
    }

    #[test]
    fn test_diff_nodes_control_label_change() {
        let old = SCGNode::new(1, NodeKind::Branch);
        let mut new = old.clone();
        new.control_label = Some("then".to_owned());
        let changes = diff_nodes(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field_name, "control_label");
        assert_eq!(changes[0].old_value, "None");
        assert_eq!(changes[0].new_value, "then");
    }
}
