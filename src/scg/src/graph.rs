//! SCG Graph Structure
//!
//! This module defines the core `SCG` graph type, which wraps a hand-written
//! [`crate::digraph::DiGraph`] (linked-list adjacency; see `digraph.rs`) and
//! provides high-level operations for constructing, querying, and manipulating
//! the Semantic Computation Graph.
//!
//! The graph algorithms `toposort`, `tarjan_scc`, and `has_path_connecting`
//! are also implemented in [`crate::digraph`] — this module no longer reaches
//! into `petgraph`.

use hashbrown::{HashMap, HashSet};

use crate::digraph::{
    has_path_connecting, tarjan_scc, toposort, DiGraph, Direction, EdgeIndex, NodeIndex,
};
use crate::edge::{EdgeData, EdgeId, EdgeKind};
use crate::node::{NodeData, NodeId, NodePayload, NodeType, ProgramPoint};
use crate::region::{RegionId, SCGRegion};

/// A small insertion-ordered set built on top of `HashSet` + `Vec`.
///
/// This replaces the previous `indexmap::IndexSet<NodeId>` usage so that the
/// crate no longer depends on `indexmap`. Membership is O(1) (HashSet) and
/// insertion order is preserved (Vec), matching the `IndexSet` semantics used
/// at the call sites.
pub(crate) struct OrderedSet<T: Eq + std::hash::Hash + Clone> {
    set: HashSet<T>,
    order: Vec<T>,
}

impl<T: Eq + std::hash::Hash + Clone> OrderedSet<T> {
    /// Creates an empty `OrderedSet`.
    pub(crate) fn new() -> Self {
        Self {
            set: HashSet::new(),
            order: Vec::new(),
        }
    }

    /// Inserts a value. Returns `true` if it was not already present.
    pub(crate) fn insert(&mut self, value: T) -> bool {
        if self.set.insert(value.clone()) {
            self.order.push(value);
            true
        } else {
            false
        }
    }

    /// Returns `true` if the set contains the given value.
    pub(crate) fn contains(&self, value: &T) -> bool {
        self.set.contains(value)
    }

    /// Returns the number of elements in the set.
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.order.len()
    }

    /// Returns `true` if the set is empty.
    #[allow(dead_code)]
    pub(crate) fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Iterates over the elements in insertion order.
    pub(crate) fn iter(&self) -> std::slice::Iter<'_, T> {
        self.order.iter()
    }
}

impl<T: Eq + std::hash::Hash + Clone> Default for OrderedSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Eq + std::hash::Hash + Clone> FromIterator<T> for OrderedSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut s = Self::new();
        for v in iter {
            s.insert(v);
        }
        s
    }
}

impl<'a, T: Eq + std::hash::Hash + Clone> IntoIterator for &'a OrderedSet<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Errors that can occur during SCG operations.
#[derive(Debug, Clone, PartialEq)]
pub enum SCGError {
    /// A node with the given `NodeId` was not found in the graph.
    NodeNotFound(NodeId),
    /// An edge with the given `EdgeId` was not found in the graph.
    EdgeNotFound(EdgeId),
    /// A node with the given `NodeId` already exists in the graph.
    DuplicateNode(NodeId),
    /// An edge with the given `EdgeId` already exists in the graph.
    DuplicateEdge(EdgeId),
    /// The graph contains a cycle, preventing topological sort.
    CycleDetected,
    /// A well-formedness validation check failed.
    ValidationFailed(String),
    /// A region with the given `RegionId` was not found.
    RegionNotFound(RegionId),
    /// The specified source or target node for an edge does not exist.
    InvalidEdgeEndpoints {
        /// The source node of the invalid edge.
        source: NodeId,
        /// The target node of the invalid edge.
        target: NodeId,
    },
}

impl std::fmt::Display for SCGError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SCGError::NodeNotFound(id) => write!(f, "node not found: {id}"),
            SCGError::EdgeNotFound(id) => write!(f, "edge not found: {id}"),
            SCGError::DuplicateNode(id) => write!(f, "duplicate node: {id}"),
            SCGError::DuplicateEdge(id) => write!(f, "duplicate edge: {id}"),
            SCGError::CycleDetected => write!(f, "cycle detected in graph"),
            SCGError::ValidationFailed(msg) => write!(f, "validation failed: {msg}"),
            SCGError::RegionNotFound(id) => write!(f, "region not found: {id}"),
            SCGError::InvalidEdgeEndpoints { source, target } => {
                write!(
                    f,
                    "invalid edge endpoints: source={source}, target={target}"
                )
            }
        }
    }
}

impl std::error::Error for SCGError {}

/// Result of validating an SCG's well-formedness.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationResult {
    /// Whether the SCG passed all validation checks.
    pub is_valid: bool,
    /// A list of warnings or informational messages (non-fatal issues).
    pub warnings: Vec<String>,
    /// A list of error messages (fatal issues that invalidate the graph).
    pub errors: Vec<String>,
}

impl ValidationResult {
    /// Creates a successful validation result with no errors.
    pub fn ok() -> Self {
        Self {
            is_valid: true,
            warnings: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Creates a failed validation result with the given error message.
    pub fn fail(error: impl Into<String>) -> Self {
        Self {
            is_valid: false,
            warnings: Vec::new(),
            errors: vec![error.into()],
        }
    }

    /// Adds a warning to the validation result.
    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    /// Adds an error to the validation result and marks it as invalid.
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.is_valid = false;
        self.errors.push(error.into());
        self
    }
}

/// The Semantic Computation Graph.
///
/// `SCG` is the central data structure of the SCG module. It wraps a
/// hand-written [`crate::digraph::DiGraph`] and maintains bidirectional
/// mappings between external `NodeId`/`EdgeId` identifiers and the graph's
/// internal `NodeIndex`/`EdgeIndex` handles.
///
/// # Type Parameters
/// The graph stores `NodeData` as node weights and `EdgeData` as edge weights.
///
/// # Invariants
/// - Every `NodeId` maps to exactly one `NodeIndex`, and vice versa.
/// - Every `EdgeId` maps to exactly one `EdgeIndex`, and vice versa.
/// - All edges connect nodes that exist in the graph.
#[derive(Debug, Clone)]
pub struct SCG {
    /// The underlying directed graph.
    graph: DiGraph<NodeData, EdgeData>,
    /// Mapping from external `NodeId` to internal `NodeIndex`.
    node_id_to_index: HashMap<NodeId, NodeIndex>,
    /// Mapping from internal `NodeIndex` to external `NodeId`.
    node_index_to_id: HashMap<NodeIndex, NodeId>,
    /// Mapping from external `EdgeId` to internal `EdgeIndex`.
    edge_id_to_index: HashMap<EdgeId, EdgeIndex>,
    /// Mapping from internal `EdgeIndex` to external `EdgeId`.
    edge_index_to_id: HashMap<EdgeIndex, EdgeId>,
    /// Regions defined within this SCG.
    regions: HashMap<RegionId, SCGRegion>,
    /// Counter for generating the next `NodeId`.
    next_node_id: u64,
    /// Counter for generating the next `EdgeId`.
    next_edge_id: u64,
}

impl SCG {
    /// Creates a new, empty SCG.
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            node_id_to_index: HashMap::new(),
            node_index_to_id: HashMap::new(),
            edge_id_to_index: HashMap::new(),
            edge_index_to_id: HashMap::new(),
            regions: HashMap::new(),
            next_node_id: 0,
            next_edge_id: 0,
        }
    }

    /// Allocates and returns the next unique `NodeId`.
    fn alloc_node_id(&mut self) -> NodeId {
        let id = NodeId::new(self.next_node_id);
        self.next_node_id += 1;
        id
    }

    /// Allocates and returns the next unique `EdgeId`.
    fn alloc_edge_id(&mut self) -> EdgeId {
        let id = EdgeId::new(self.next_edge_id);
        self.next_edge_id += 1;
        id
    }

    // ── Node Operations ────────────────────────────────────────────

    /// Adds a node to the graph with the given type, payload, and program point.
    ///
    /// A unique `NodeId` is automatically assigned. Returns the assigned `NodeId`.
    pub fn add_node(
        &mut self,
        node_type: NodeType,
        payload: NodePayload,
        program_point: ProgramPoint,
    ) -> NodeId {
        let id = self.alloc_node_id();
        let data = NodeData {
            id,
            node_type,
            annotation: None,
            program_point,
            payload,
        };
        let idx = self.graph.add_node(data);
        self.node_id_to_index.insert(id, idx);
        self.node_index_to_id.insert(idx, id);
        id
    }

    /// Adds a node with a pre-assigned `NodeId`.
    ///
    /// Returns an error if a node with the same `NodeId` already exists.
    pub fn add_node_with_id(
        &mut self,
        id: NodeId,
        node_type: NodeType,
        payload: NodePayload,
        program_point: ProgramPoint,
    ) -> Result<NodeId, SCGError> {
        if self.node_id_to_index.contains_key(&id) {
            return Err(SCGError::DuplicateNode(id));
        }
        let data = NodeData {
            id,
            node_type,
            annotation: None,
            program_point,
            payload,
        };
        let idx = self.graph.add_node(data);
        self.node_id_to_index.insert(id, idx);
        self.node_index_to_id.insert(idx, id);
        // Update next_node_id if necessary to avoid future collisions
        if id.as_u64() >= self.next_node_id {
            self.next_node_id = id.as_u64() + 1;
        }
        Ok(id)
    }

    /// Removes a node from the graph by its `NodeId`.
    ///
    /// All edges connected to this node are also removed.
    /// Returns the removed `NodeData`, or an error if the node was not found.
    pub fn remove_node(&mut self, id: NodeId) -> Result<NodeData, SCGError> {
        let idx = self
            .node_id_to_index
            .remove(&id)
            .ok_or(SCGError::NodeNotFound(id))?;

        // Remove all edge mappings for edges connected to this node
        let edges_to_remove: Vec<EdgeIndex> = self
            .graph
            .edges_directed(idx, Direction::Outgoing)
            .chain(self.graph.edges_directed(idx, Direction::Incoming))
            .map(|e| e.id())
            .collect();

        for eidx in edges_to_remove {
            if let Some(eid) = self.edge_index_to_id.remove(&eidx) {
                self.edge_id_to_index.remove(&eid);
            }
        }

        self.node_index_to_id.remove(&idx);
        let data = self.graph.remove_node(idx).expect("node index was valid");

        // Rebuild index mappings after node removal (harmless no-op for the
        // stable-index DiGraph, but kept for parity with the previous
        // petgraph-backed implementation).
        self.rebuild_index_mappings();

        Ok(data)
    }

    /// Retrieves a reference to the `NodeData` for the given `NodeId`.
    pub fn get_node(&self, id: NodeId) -> Option<&NodeData> {
        self.node_id_to_index
            .get(&id)
            .and_then(|&idx| self.graph.node_weight(idx))
    }

    /// Retrieves a mutable reference to the `NodeData` for the given `NodeId`.
    pub fn get_node_mut(&mut self, id: NodeId) -> Option<&mut NodeData> {
        let idx = *self.node_id_to_index.get(&id)?;
        self.graph.node_weight_mut(idx)
    }

    /// Returns the total number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Returns an iterator over all node IDs in the graph.
    pub fn node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.node_id_to_index.keys().copied()
    }

    /// Returns an iterator over all node data in the graph.
    pub fn nodes(&self) -> impl Iterator<Item = &NodeData> {
        self.graph.node_weights()
    }

    // ── Edge Operations ────────────────────────────────────────────

    /// Adds an edge between two existing nodes.
    ///
    /// A unique `EdgeId` is automatically assigned.
    /// Returns an error if either endpoint node does not exist.
    pub fn add_edge(
        &mut self,
        source: NodeId,
        target: NodeId,
        kind: EdgeKind,
    ) -> Result<EdgeId, SCGError> {
        let source_idx = *self
            .node_id_to_index
            .get(&source)
            .ok_or(SCGError::InvalidEdgeEndpoints { source, target })?;
        let target_idx = *self
            .node_id_to_index
            .get(&target)
            .ok_or(SCGError::InvalidEdgeEndpoints { source, target })?;

        let id = self.alloc_edge_id();
        let data = EdgeData {
            id,
            source,
            target,
            kind,
            label: None,
        };
        let eidx = self.graph.add_edge(source_idx, target_idx, data);
        self.edge_id_to_index.insert(id, eidx);
        self.edge_index_to_id.insert(eidx, id);
        Ok(id)
    }

    /// Adds an edge with a pre-assigned `EdgeId`.
    ///
    /// Returns an error if the edge ID already exists or either endpoint
    /// node does not exist.
    pub fn add_edge_with_id(
        &mut self,
        id: EdgeId,
        source: NodeId,
        target: NodeId,
        kind: EdgeKind,
    ) -> Result<EdgeId, SCGError> {
        if self.edge_id_to_index.contains_key(&id) {
            return Err(SCGError::DuplicateEdge(id));
        }
        let source_idx = *self
            .node_id_to_index
            .get(&source)
            .ok_or(SCGError::InvalidEdgeEndpoints { source, target })?;
        let target_idx = *self
            .node_id_to_index
            .get(&target)
            .ok_or(SCGError::InvalidEdgeEndpoints { source, target })?;

        let data = EdgeData {
            id,
            source,
            target,
            kind,
            label: None,
        };
        let eidx = self.graph.add_edge(source_idx, target_idx, data);
        self.edge_id_to_index.insert(id, eidx);
        self.edge_index_to_id.insert(eidx, id);
        if id.as_u64() >= self.next_edge_id {
            self.next_edge_id = id.as_u64() + 1;
        }
        Ok(id)
    }

    /// Removes an edge from the graph by its `EdgeId`.
    ///
    /// Returns the removed `EdgeData`, or an error if the edge was not found.
    pub fn remove_edge(&mut self, id: EdgeId) -> Result<EdgeData, SCGError> {
        let eidx = self
            .edge_id_to_index
            .remove(&id)
            .ok_or(SCGError::EdgeNotFound(id))?;

        self.edge_index_to_id.remove(&eidx);
        let data = self.graph.remove_edge(eidx).expect("edge index was valid");

        // Note: the hand-written DiGraph uses tombstone slots, so edge
        // removal does not shift any indices. We rebuild edge mappings anyway
        // for parity with the previous petgraph-backed implementation.
        self.rebuild_edge_mappings();

        Ok(data)
    }

    /// Retrieves a reference to the `EdgeData` for the given `EdgeId`.
    pub fn get_edge(&self, id: EdgeId) -> Option<&EdgeData> {
        self.edge_id_to_index
            .get(&id)
            .and_then(|&eidx| self.graph.edge_weight(eidx))
    }

    /// Retrieves a mutable reference to the `EdgeData` for the given `EdgeId`.
    pub fn get_edge_mut(&mut self, id: EdgeId) -> Option<&mut EdgeData> {
        let eidx = *self.edge_id_to_index.get(&id)?;
        self.graph.edge_weight_mut(eidx)
    }

    /// Returns the total number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Returns an iterator over all edge IDs in the graph.
    pub fn edge_ids(&self) -> impl Iterator<Item = EdgeId> + '_ {
        self.edge_id_to_index.keys().copied()
    }

    /// Returns an iterator over all edge data in the graph.
    pub fn edges(&self) -> impl Iterator<Item = &EdgeData> {
        self.graph.edge_weights()
    }

    // ── Interprocedural Edge Helpers ──────────────────────────────

    /// Adds a Call edge from a caller node to a callee's FunctionEntry node.
    ///
    /// This creates an interprocedural call edge in the SCG, representing
    /// a function call from the caller (at `from_node`) to the callee
    /// (whose entry is `to_node`). The `caller_region` identifies the
    /// region in which the caller is executing.
    pub fn add_call_edge(
        &mut self,
        from_node: NodeId,
        to_node: NodeId,
        caller_region: RegionId,
    ) -> Result<EdgeId, SCGError> {
        self.add_edge(
            from_node,
            to_node,
            EdgeKind::Call {
                from_node,
                to_node,
                caller_region,
            },
        )
    }

    /// Adds a Return edge from a callee's FunctionReturn node back to the caller.
    ///
    /// This creates an interprocedural return edge in the SCG, representing
    /// a function return from the callee (at `from_node`) back to the caller
    /// (at `to_node`). The `return_values` lists the node IDs of the
    /// values being returned.
    pub fn add_return_edge(
        &mut self,
        from_node: NodeId,
        to_node: NodeId,
        return_values: Vec<NodeId>,
    ) -> Result<EdgeId, SCGError> {
        self.add_edge(
            from_node,
            to_node,
            EdgeKind::Return {
                from_node,
                to_node,
                return_values,
            },
        )
    }

    /// Returns all edges of the specified kind.
    pub fn edges_of_kind(&self, kind: &EdgeKind) -> Vec<&EdgeData> {
        self.edges().filter(|e| &e.kind == kind).collect()
    }

    /// Returns all Call edges in the graph.
    pub fn call_edges(&self) -> Vec<&EdgeData> {
        self.edges()
            .filter(|e| matches!(e.kind, EdgeKind::Call { .. }))
            .collect()
    }

    /// Returns all Return edges in the graph.
    pub fn return_edges(&self) -> Vec<&EdgeData> {
        self.edges()
            .filter(|e| matches!(e.kind, EdgeKind::Return { .. }))
            .collect()
    }

    /// Finds all FunctionEntry and FunctionReturn nodes in the graph.
    ///
    /// Returns a tuple of (entry_nodes, return_nodes).
    pub fn function_boundary_nodes(&self) -> (Vec<NodeId>, Vec<NodeId>) {
        let mut entries = Vec::new();
        let mut returns = Vec::new();
        for node in self.nodes() {
            if node.node_type == NodeType::Control {
                if let NodePayload::Control(ctrl) = &node.payload {
                    match ctrl.kind {
                        crate::node::ControlKind::FunctionEntry => entries.push(node.id),
                        crate::node::ControlKind::FunctionReturn => returns.push(node.id),
                        _ => {}
                    }
                }
            }
        }
        (entries, returns)
    }

    /// Given a FunctionEntry node, finds the corresponding FunctionReturn node
    /// by looking for the FunctionReturn node reachable via ControlFlow edges
    /// from the entry.
    pub fn find_function_return(&self, entry: NodeId) -> Option<NodeId> {
        let &entry_idx = self.node_id_to_index.get(&entry)?;
        // BFS through ControlFlow edges to find a FunctionReturn node
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(entry_idx);
        visited.insert(entry_idx);

        while let Some(current_idx) = queue.pop_front() {
            for edge_ref in self.graph.edges_directed(current_idx, Direction::Outgoing) {
                let target_idx = edge_ref.target();
                if visited.insert(target_idx) {
                    if let Some(target_id) = self.node_index_to_id.get(&target_idx) {
                        if let Some(node) = self.get_node(*target_id) {
                            if let NodePayload::Control(ctrl) = &node.payload {
                                if ctrl.kind == crate::node::ControlKind::FunctionReturn {
                                    return Some(*target_id);
                                }
                            }
                        }
                    }
                    queue.push_back(target_idx);
                }
            }
        }
        None
    }

    // ── Traversal Operations ───────────────────────────────────────

    /// Returns the `NodeId`s of all direct successors of the given node.
    ///
    /// Successors are nodes reachable via outgoing edges from the specified node.
    pub fn successors(&self, id: NodeId) -> Option<Vec<NodeId>> {
        let &idx = self.node_id_to_index.get(&id)?;
        let succs: Vec<NodeId> = self
            .graph
            .neighbors_directed(idx, Direction::Outgoing)
            .filter_map(|nidx| self.node_index_to_id.get(&nidx).copied())
            .collect();
        Some(succs)
    }

    /// Returns the `NodeId`s of all direct predecessors of the given node.
    ///
    /// Predecessors are nodes that have outgoing edges to the specified node.
    pub fn predecessors(&self, id: NodeId) -> Option<Vec<NodeId>> {
        let &idx = self.node_id_to_index.get(&id)?;
        let preds: Vec<NodeId> = self
            .graph
            .neighbors_directed(idx, Direction::Incoming)
            .filter_map(|nidx| self.node_index_to_id.get(&nidx).copied())
            .collect();
        Some(preds)
    }

    /// Checks whether a path exists from `source` to `target`.
    ///
    /// Returns `None` if either node does not exist.
    /// Returns `Some(true)` if a path exists, `Some(false)` otherwise.
    pub fn find_path(&self, source: NodeId, target: NodeId) -> Option<bool> {
        let &source_idx = self.node_id_to_index.get(&source)?;
        let &target_idx = self.node_id_to_index.get(&target)?;
        Some(has_path_connecting(
            &self.graph,
            source_idx,
            target_idx,
        ))
    }

    /// Returns a topological ordering of the nodes in the graph.
    ///
    /// Returns an error if the graph contains a cycle.
    pub fn topological_sort(&self) -> Result<Vec<NodeId>, SCGError> {
        let sorted: Vec<NodeIndex> =
            toposort(&self.graph).map_err(|_| SCGError::CycleDetected)?;
        let result: Vec<NodeId> = sorted
            .into_iter()
            .filter_map(|idx| self.node_index_to_id.get(&idx).copied())
            .collect();
        Ok(result)
    }

    /// Returns a topological ordering of the nodes in the graph, handling
    /// cycles by grouping nodes into strongly connected components (SCCs).
    ///
    /// Unlike `topological_sort`, this method does not fail when the graph
    /// contains cycles (e.g., from loops). Instead, it:
    /// 1. Computes SCCs using Tarjan's algorithm
    /// 2. Topologically sorts the SCC DAG (SCCs are returned in reverse
    ///    topological order by tarjan_scc)
    /// 3. Within each SCC, orders nodes by their NodeIndex for determinism
    ///
    /// This allows the MSG builder to process cyclic SCGs that arise from
    /// loops in the source program.
    pub fn topological_sort_with_cycles(&self) -> Vec<NodeId> {
        let sccs = tarjan_scc(&self.graph);
        // tarjan_scc returns SCCs in reverse topological order (children first).
        // We need topological order (parents first), so reverse.
        let mut result: Vec<NodeId> = Vec::with_capacity(self.graph.node_count());
        for scc in sccs.into_iter().rev() {
            // Within each SCC, sort by NodeIndex for deterministic ordering
            let mut sorted_scc: Vec<NodeIndex> = scc;
            sorted_scc.sort();
            for idx in sorted_scc {
                if let Some(&node_id) = self.node_index_to_id.get(&idx) {
                    result.push(node_id);
                }
            }
        }
        result
    }

    /// Returns true if the graph contains any cycles.
    ///
    /// Uses `crate::digraph::toposort` — if it fails, the graph has a cycle.
    pub fn has_cycles(&self) -> bool {
        toposort(&self.graph).is_err()
    }

    // ── Region Operations ──────────────────────────────────────────

    /// Adds a region to the SCG.
    pub fn add_region(&mut self, region: SCGRegion) {
        self.regions.insert(region.id, region);
    }

    /// Retrieves a reference to a region by its `RegionId`.
    pub fn get_region(&self, id: RegionId) -> Option<&SCGRegion> {
        self.regions.get(&id)
    }

    /// Retrieves a mutable reference to a region by its `RegionId`.
    pub fn get_region_mut(&mut self, id: RegionId) -> Option<&mut SCGRegion> {
        self.regions.get_mut(&id)
    }

    /// Removes a region from the SCG.
    ///
    /// Returns the removed region, or `None` if not found.
    pub fn remove_region(&mut self, id: RegionId) -> Option<SCGRegion> {
        self.regions.remove(&id)
    }

    /// Returns an iterator over all regions in the SCG.
    pub fn regions(&self) -> impl Iterator<Item = &SCGRegion> {
        self.regions.values()
    }

    /// Returns the number of regions in the SCG.
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    // ── Validation ─────────────────────────────────────────────────

    /// Validates the well-formedness of this SCG.
    ///
    /// Checks the following conditions:
    /// - Every edge's source and target nodes exist in the graph.
    /// - Every allocation has a corresponding deallocation (warning if not).
    /// - No orphan nodes (nodes with no edges) that are not phantom nodes (warning).
    /// - Security boundary regions do not have direct data flow across them.
    /// - Deallocation nodes reference valid allocation nodes.
    pub fn validate(&self) -> ValidationResult {
        let mut result = ValidationResult::ok();

        // Check that all edges reference existing nodes
        for edge_data in self.edges() {
            if self.get_node(edge_data.source).is_none() {
                result = result.with_error(format!(
                    "edge {} references non-existent source node {}",
                    edge_data.id, edge_data.source
                ));
            }
            if self.get_node(edge_data.target).is_none() {
                result = result.with_error(format!(
                    "edge {} references non-existent target node {}",
                    edge_data.id, edge_data.target
                ));
            }
        }

        // Check for allocation/deallocation pairing
        let allocations: OrderedSet<NodeId> = self
            .nodes()
            .filter(|n| matches!(n.node_type, NodeType::Allocation))
            .map(|n| n.id)
            .collect();

        let deallocations_referenced: OrderedSet<NodeId> = self
            .nodes()
            .filter_map(|n| match &n.payload {
                NodePayload::Deallocation(d) => Some(d.allocation_node),
                _ => None,
            })
            .collect();

        for alloc_id in &allocations {
            if !deallocations_referenced.contains(alloc_id) {
                result = result.with_warning(format!(
                    "allocation node {alloc_id} has no corresponding deallocation"
                ));
            }
        }

        // Check deallocation nodes reference valid allocations
        for node_data in self.nodes() {
            if let NodePayload::Deallocation(dealloc) = &node_data.payload {
                if !allocations.contains(&dealloc.allocation_node) {
                    result = result.with_error(format!(
                        "deallocation node {} references non-existent allocation node {}",
                        node_data.id, dealloc.allocation_node
                    ));
                }
            }
        }

        // Check for orphan nodes (warning only, not error)
        for node_data in self.nodes() {
            if !matches!(node_data.node_type, NodeType::Phantom) {
                let succs = self.successors(node_data.id).unwrap_or_default();
                let preds = self.predecessors(node_data.id).unwrap_or_default();
                if succs.is_empty() && preds.is_empty() && self.node_count() > 1 {
                    result =
                        result.with_warning(format!("orphan node {} (no edges)", node_data.id));
                }
            }
        }

        // Check security boundary: no direct data flow across security boundaries
        for edge_data in self.edges() {
            if matches!(edge_data.kind, EdgeKind::DataFlow) {
                if let (Some(src_region), Some(tgt_region)) = (
                    self.find_region_for_node(&edge_data.source),
                    self.find_region_for_node(&edge_data.target),
                ) {
                    if src_region != tgt_region {
                        let src = self.regions.get(&src_region);
                        let tgt = self.regions.get(&tgt_region);
                        if src.is_some_and(|r| r.security_boundary)
                            || tgt.is_some_and(|r| r.security_boundary)
                        {
                            result = result.with_error(format!(
                                "data flow edge {} crosses security boundary between regions {} and {}",
                                edge_data.id, src_region, tgt_region
                            ));
                        }
                    }
                }
            }
        }

        result
    }

    /// Finds the `RegionId` containing the given node, if any.
    fn find_region_for_node(&self, node_id: &NodeId) -> Option<RegionId> {
        for region in self.regions.values() {
            if region.contains_node(node_id) {
                return Some(region.id);
            }
        }
        None
    }

    // ── Merge ──────────────────────────────────────────────────────

    /// Merges another SCG into this one.
    ///
    /// All nodes, edges, and regions from `other` are incorporated into `self`.
    /// Node and edge IDs from `other` are remapped to avoid collisions with
    /// existing IDs in `self`.
    ///
    /// Returns a mapping from old `NodeId`s (in `other`) to new `NodeId`s
    /// (in the merged graph).
    pub fn merge(&mut self, other: SCG) -> HashMap<NodeId, NodeId> {
        let mut node_map: HashMap<NodeId, NodeId> = HashMap::new();
        let mut edge_map: HashMap<EdgeId, EdgeId> = HashMap::new();

        // Add all nodes from other, creating new IDs
        for node_data in other.nodes() {
            let old_id = node_data.id;
            let new_id = self.add_node(
                node_data.node_type.clone(),
                node_data.payload.clone(),
                node_data.program_point.clone(),
            );
            // Copy annotation
            if let Some(ref ann) = node_data.annotation {
                if let Some(new_data) = self.get_node_mut(new_id) {
                    new_data.annotation = Some(ann.clone());
                }
            }
            node_map.insert(old_id, new_id);
        }

        // Add all edges from other, remapping node IDs
        for edge_data in other.edges() {
            let new_source = node_map
                .get(&edge_data.source)
                .copied()
                .unwrap_or(edge_data.source);
            let new_target = node_map
                .get(&edge_data.target)
                .copied()
                .unwrap_or(edge_data.target);

            if let Ok(new_eid) = self.add_edge(new_source, new_target, edge_data.kind.clone()) {
                // Copy label
                if let Some(ref label) = edge_data.label {
                    if let Some(new_edge) = self.get_edge_mut(new_eid) {
                        new_edge.label = Some(label.clone());
                    }
                }
                edge_map.insert(edge_data.id, new_eid);
            }
        }

        // Add all regions from other, remapping node IDs
        for region in other.regions() {
            let new_region_id = RegionId::new(self.next_region_id());
            let mut new_region = SCGRegion::new(new_region_id, region.deployment_target.clone());
            new_region.scope_level = region.scope_level;
            new_region.security_boundary = region.security_boundary;
            for old_node_id in region.iter_nodes() {
                if let Some(&new_node_id) = node_map.get(old_node_id) {
                    new_region.add_node(new_node_id);
                }
            }
            self.add_region(new_region);
        }

        node_map
    }

    /// Returns the next available region ID.
    fn next_region_id(&self) -> u64 {
        let max_id = self.regions.keys().map(|r| r.as_u64()).max().unwrap_or(0);
        max_id + 1
    }

    // ── Internal Helpers ───────────────────────────────────────────

    /// Rebuilds both node and edge index mappings after a node removal.
    ///
    /// The hand-written `DiGraph` uses tombstone slots, so node `NodeIndex`es
    /// are actually stable across removals — but rebuilding is cheap and keeps
    /// the mappings in lock-step with the live node set, so we keep the
    /// helper for clarity and forward-compatibility.
    fn rebuild_index_mappings(&mut self) {
        self.node_id_to_index.clear();
        self.node_index_to_id.clear();
        self.edge_id_to_index.clear();
        self.edge_index_to_id.clear();

        for idx in self.graph.node_indices() {
            if let Some(data) = self.graph.node_weight(idx) {
                self.node_id_to_index.insert(data.id, idx);
                self.node_index_to_id.insert(idx, data.id);
            }
        }

        for eidx in self.graph.edge_indices() {
            if let Some(data) = self.graph.edge_weight(eidx) {
                self.edge_id_to_index.insert(data.id, eidx);
                self.edge_index_to_id.insert(eidx, data.id);
            }
        }
    }

    /// Rebuilds only edge index mappings after an edge removal.
    fn rebuild_edge_mappings(&mut self) {
        self.edge_id_to_index.clear();
        self.edge_index_to_id.clear();

        for eidx in self.graph.edge_indices() {
            if let Some(data) = self.graph.edge_weight(eidx) {
                self.edge_id_to_index.insert(data.id, eidx);
                self.edge_index_to_id.insert(eidx, data.id);
            }
        }
    }
}

impl Default for SCG {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{AllocationNode, ComputationKind, ComputationNode, DeallocationNode, PhantomNode};
    use crate::region::DeploymentTarget;

    fn make_program_point() -> ProgramPoint {
        ProgramPoint {
            file: Some("test.vu".to_string()),
            line: Some(1),
            column: Some(1),
            offset: None,
        }
    }

    #[test]
    fn test_add_and_get_node() {
        let mut scg = SCG::new();
        let id = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("add".to_string()),
                result_type: Some("i32".to_string()),
                tail_call: false,
            }),
            make_program_point(),
        );
        let node = scg.get_node(id).unwrap();
        assert_eq!(node.id, id);
        assert_eq!(node.node_type, NodeType::Computation);
    }

    #[test]
    fn test_remove_node() {
        let mut scg = SCG::new();
        let id = scg.add_node(
            NodeType::Phantom,
            NodePayload::Phantom(PhantomNode {
                purpose: "test".to_string(),
            }),
            make_program_point(),
        );
        let removed = scg.remove_node(id).unwrap();
        assert_eq!(removed.id, id);
        assert!(scg.get_node(id).is_none());
    }

    #[test]
    fn test_remove_nonexistent_node() {
        let mut scg = SCG::new();
        let result = scg.remove_node(NodeId::new(999));
        assert!(matches!(result, Err(SCGError::NodeNotFound(_))));
    }

    #[test]
    fn test_add_and_get_edge() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("f".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("g".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );
        let eid = scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        let edge = scg.get_edge(eid).unwrap();
        assert_eq!(edge.source, n1);
        assert_eq!(edge.target, n2);
    }

    #[test]
    fn test_add_edge_invalid_endpoints() {
        let mut scg = SCG::new();
        let result = scg.add_edge(NodeId::new(99), NodeId::new(100), EdgeKind::DataFlow);
        assert!(matches!(result, Err(SCGError::InvalidEdgeEndpoints { .. })));
    }

    #[test]
    fn test_successors_and_predecessors() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("a".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("b".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("c".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();

        let succs = scg.successors(n1).unwrap();
        assert_eq!(succs.len(), 2);
        assert!(succs.contains(&n2));
        assert!(succs.contains(&n3));

        let preds_n2 = scg.predecessors(n2).unwrap();
        assert_eq!(preds_n2.len(), 1);
        assert!(preds_n2.contains(&n1));
    }

    #[test]
    fn test_find_path() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("x".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("y".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("z".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n3, EdgeKind::DataFlow).unwrap();

        assert_eq!(scg.find_path(n1, n3), Some(true));
        assert_eq!(scg.find_path(n3, n1), Some(false));
    }

    #[test]
    fn test_topological_sort() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("first".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("second".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();

        let sorted = scg.topological_sort().unwrap();
        assert_eq!(sorted.len(), 2);
        // n1 must come before n2
        let pos1 = sorted.iter().position(|&id| id == n1).unwrap();
        let pos2 = sorted.iter().position(|&id| id == n2).unwrap();
        assert!(pos1 < pos2);
    }

    #[test]
    fn test_topological_sort_cycle() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("a".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("b".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n1, EdgeKind::DataFlow).unwrap();

        let result = scg.topological_sort();
        assert!(matches!(result, Err(SCGError::CycleDetected)));
    }

    #[test]
    fn test_validate_clean_graph() {
        let mut scg = SCG::new();
        let region = RegionId::new(1);
        let n1 = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            make_program_point(),
        );
        let n2 = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: n1,
                region_id: region,
            }),
            make_program_point(),
        );
        scg.add_edge(n1, n2, EdgeKind::Derivation).unwrap();

        let result = scg.validate();
        assert!(result.is_valid, "Validation errors: {:?}", result.errors);
    }

    #[test]
    fn test_validate_missing_deallocation() {
        let mut scg = SCG::new();
        scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: RegionId::new(1),
                type_name: None,
            }),
            make_program_point(),
        );
        let result = scg.validate();
        // Should have warnings but still be valid (warnings don't invalidate)
        assert!(result
            .warnings
            .iter()
            .any(|w| w.contains("no corresponding deallocation")));
    }

    #[test]
    fn test_merge_two_scgs() {
        let mut scg1 = SCG::new();
        let n1 = scg1.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("a".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );

        let mut scg2 = SCG::new();
        let n2_old = scg2.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("b".to_string()),
                result_type: None,
                tail_call: false,
            }),
            make_program_point(),
        );

        let node_map = scg1.merge(scg2);
        assert_eq!(scg1.node_count(), 2);
        assert!(node_map.contains_key(&n2_old));
        let n2_new = node_map[&n2_old];
        assert_ne!(n1, n2_new);
        assert!(scg1.get_node(n1).is_some());
        assert!(scg1.get_node(n2_new).is_some());
    }

    #[test]
    fn test_regions() {
        let mut scg = SCG::new();
        let region = SCGRegion::new(RegionId::new(1), DeploymentTarget::Heap);
        scg.add_region(region);
        assert_eq!(scg.region_count(), 1);
        assert!(scg.get_region(RegionId::new(1)).is_some());
    }

    /// Wave 39 regression: the SCG storage now wraps the hand-written
    /// `crate::digraph::DiGraph` instead of `petgraph::DiGraph`. This test
    /// exercises the 17 storage-method surface end-to-end through the SCG API
    /// to confirm the swap is transparent: add/remove node, add/remove edge,
    /// node/edge weight access, node/edge counts, node/edge id iteration,
    /// successors/predecessors (neighbors_directed), find_path
    /// (has_path_connecting), topological_sort (toposort),
    /// topological_sort_with_cycles (tarjan_scc), and has_cycles.
    #[test]
    fn wave39_scg_backed_by_handwritten_digraph() {
        let mut scg = SCG::new();

        // add_node x4 (storage: DiGraph::add_node).
        let mk = |label: &str| {
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other(label.to_string()),
                result_type: None,
                tail_call: false,
            })
        };
        let n1 = scg.add_node(NodeType::Computation, mk("a"), make_program_point());
        let n2 = scg.add_node(NodeType::Computation, mk("b"), make_program_point());
        let n3 = scg.add_node(NodeType::Computation, mk("c"), make_program_point());
        let n4 = scg.add_node(NodeType::Computation, mk("d"), make_program_point());

        // add_edge x4 (storage: DiGraph::add_edge).
        let _e12 = scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        let _e23 = scg.add_edge(n2, n3, EdgeKind::DataFlow).unwrap();
        let _e34 = scg.add_edge(n3, n4, EdgeKind::DataFlow).unwrap();
        // Cross edge n2 → n4.
        let e24 = scg.add_edge(n2, n4, EdgeKind::DataFlow).unwrap();

        // node_count / edge_count.
        assert_eq!(scg.node_count(), 4);
        assert_eq!(scg.edge_count(), 4);

        // node_weight / edge_weight access through SCG wrappers.
        assert_eq!(scg.get_node(n2).unwrap().id, n2);
        assert_eq!(scg.get_edge(e24).unwrap().source, n2);
        assert_eq!(scg.get_edge(e24).unwrap().target, n4);

        // node_ids / edge_ids iteration (storage: node_indices / edge_indices).
        assert_eq!(scg.node_ids().count(), 4);
        assert_eq!(scg.edge_ids().count(), 4);

        // successors / predecessors (storage: neighbors_directed).
        let succs_n2 = scg.successors(n2).unwrap();
        assert_eq!(succs_n2.len(), 2);
        assert!(succs_n2.contains(&n3) && succs_n2.contains(&n4));
        let preds_n4 = scg.predecessors(n4).unwrap();
        assert_eq!(preds_n4.len(), 2);
        assert!(preds_n4.contains(&n2) && preds_n4.contains(&n3));

        // find_path (storage: has_path_connecting, BFS).
        assert_eq!(scg.find_path(n1, n4), Some(true));
        assert_eq!(scg.find_path(n4, n1), Some(false));
        assert_eq!(scg.find_path(n1, n1), Some(true));

        // topological_sort (storage: toposort, Kahn) — acyclic graph.
        let topo = scg.topological_sort().unwrap();
        assert_eq!(topo.len(), 4);
        let pos = |id| topo.iter().position(|&x| x == id).unwrap();
        assert!(pos(n1) < pos(n2));
        assert!(pos(n2) < pos(n3));
        assert!(pos(n3) < pos(n4));

        // has_cycles — currently acyclic.
        assert!(!scg.has_cycles());

        // Introduce a cycle (n4 → n2) to exercise tarjan_scc + cyclic path.
        scg.add_edge(n4, n2, EdgeKind::DataFlow).unwrap();

        // toposort must now fail.
        assert!(matches!(scg.topological_sort(), Err(SCGError::CycleDetected)));
        assert!(scg.has_cycles());

        // topological_sort_with_cycles must still return all 4 nodes (storage:
        // tarjan_scc groups the {n2,n3,n4} cycle, n1 stays singleton).
        let cyclic_topo = scg.topological_sort_with_cycles();
        assert_eq!(cyclic_topo.len(), 4);
        assert!(cyclic_topo.contains(&n1));
        assert!(cyclic_topo.contains(&n2));
        assert!(cyclic_topo.contains(&n3));
        assert!(cyclic_topo.contains(&n4));
        // n1 must precede the cycle (n1 has no predecessors).
        let p1 = cyclic_topo.iter().position(|&x| x == n1).unwrap();
        let p2 = cyclic_topo.iter().position(|&x| x == n2).unwrap();
        assert!(p1 < p2);

        // remove_edge (storage: DiGraph::remove_edge) — drop the back-edge.
        assert!(scg.remove_edge(e24).is_ok());
        assert_eq!(scg.edge_count(), 4); // added one, removed one
        assert!(scg.get_edge(e24).is_none());

        // remove_node cascades edge removal (storage: DiGraph::remove_node).
        let removed = scg.remove_node(n2).unwrap();
        assert_eq!(removed.id, n2);
        assert!(scg.get_node(n2).is_none());
        // n2 had out-edges e23, e24(removed) and in-edge e12, plus the back-edge n4→n2.
        // All those should be gone now.
        assert_eq!(scg.edge_count(), 1); // only e34 (n3→n4) remains
        assert_eq!(scg.node_count(), 3);
    }
}
