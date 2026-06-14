//! SCG Query Engine
//!
//! This module provides a query interface for the Semantic Computation Graph.
//! Queries allow structured inspection of graph properties, such as finding
//! nodes of a specific type, tracing derivation chains, or identifying
//! access patterns within memory regions.
//!
//! # LLM-Friendly Queries
//!
//! The query engine includes variants specifically designed for LLM consumption:
//!
//! - [`SCGQuery::ListFunctions`] — "What functions does this program define?"
//! - [`SCGQuery::FunctionInputsOutputs`] — "What are the inputs/outputs of function X?"
//! - [`SCGQuery::DataFlowPath`] — "What is the data flow from variable A to variable B?"
//! - [`SCGQuery::CallersOf`] — "Which functions call function X?"

use hashbrown::HashSet;
use smallvec::SmallVec;

use crate::callgraph::CallGraph;
use crate::edge::{EdgeData, EdgeId, EdgeKind};
use crate::graph::SCG;
use crate::node::{NodeId, NodePayload, NodeType};
use crate::region::RegionId;

/// A query that can be executed against an SCG.
///
/// Each variant specifies a different kind of graph inspection operation.
#[derive(Debug, Clone, PartialEq)]
pub enum SCGQuery {
    /// Find all nodes of a given `NodeType`.
    NodesByType(NodeType),
    /// Find all nodes belonging to a specific region.
    NodesByRegion(RegionId),
    /// Find all access nodes that target a specific region.
    AccessNodesToRegion(RegionId),
    /// Find derivation chains starting from a given node.
    ///
    /// A derivation chain is a sequence of edges of kind `EdgeKind::Derivation`
    /// starting from the specified node.
    DerivationChains {
        /// The starting node for the derivation chain search.
        start: NodeId,
        /// The maximum depth of the search (to prevent unbounded traversal).
        max_depth: usize,
    },
    /// Find all edges of a given `EdgeKind`.
    EdgesByKind(EdgeKind),
    /// Find all nodes that are reachable from a given node via data flow edges.
    DataFlowReachable {
        /// The starting node.
        start: NodeId,
        /// The maximum traversal depth.
        max_depth: usize,
    },
    /// Find all allocation nodes that lack a corresponding deallocation.
    LeakedAllocations,
    /// Find all paths between two nodes (up to a maximum length).
    PathsBetween {
        /// The source node.
        from: NodeId,
        /// The target node.
        to: NodeId,
        /// The maximum path length to consider.
        max_length: usize,
    },

    // ── LLM-friendly queries ──────────────────────────────────────────

    /// List all functions defined in the program.
    ///
    /// Returns function entry nodes with their names, return nodes,
    /// and contained node IDs. This answers the question:
    /// "What functions does this program define?"
    ListFunctions,

    /// Find the inputs and outputs of a specific function.
    ///
    /// Inputs are nodes that flow data into the function from outside,
    /// and outputs are nodes whose data flows out of the function.
    /// This answers the question: "What are the inputs/outputs of function X?"
    FunctionInputsOutputs {
        /// The FunctionEntry node ID of the function.
        function: NodeId,
    },

    /// Find the data flow path between two nodes.
    ///
    /// This traces the chain of DataFlow edges from `from` to `to`,
    /// answering the question: "What is the data flow from variable A to variable B?"
    DataFlowPath {
        /// The source node.
        from: NodeId,
        /// The target node.
        to: NodeId,
        /// The maximum traversal depth.
        max_depth: usize,
    },

    /// Find all functions that call a given function.
    ///
    /// This answers the question: "Which functions call function X?"
    CallersOf {
        /// The FunctionEntry node ID of the callee function.
        function: NodeId,
    },
}

/// The result of executing an `SCGQuery`.
///
/// Contains the matched node IDs, edge IDs, and optional structured data
/// depending on the query type.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    /// Node IDs that matched the query.
    pub node_ids: SmallVec<[NodeId; 8]>,
    /// Edge IDs that matched the query.
    pub edge_ids: SmallVec<[EdgeId; 8]>,
    /// Derivation chains found (only populated for `DerivationChains` queries).
    pub derivation_chains: Vec<DerivationChain>,
    /// Paths found (only populated for `PathsBetween` queries).
    pub paths: Vec<Vec<NodeId>>,
    /// Function information (only populated for `ListFunctions`,
    /// `FunctionInputsOutputs`, `CallersOf` queries).
    pub functions: Vec<FunctionInfo>,
    /// Data flow path edges (only populated for `DataFlowPath` queries).
    pub data_flow_edges: Vec<EdgeData>,
}

impl QueryResult {
    /// Creates an empty query result.
    pub fn empty() -> Self {
        Self {
            node_ids: SmallVec::new(),
            edge_ids: SmallVec::new(),
            derivation_chains: Vec::new(),
            paths: Vec::new(),
            functions: Vec::new(),
            data_flow_edges: Vec::new(),
        }
    }

    /// Returns `true` if the result contains no matches.
    pub fn is_empty(&self) -> bool {
        self.node_ids.is_empty()
            && self.edge_ids.is_empty()
            && self.derivation_chains.is_empty()
            && self.paths.is_empty()
            && self.functions.is_empty()
            && self.data_flow_edges.is_empty()
    }
}

/// A derivation chain in the SCG.
///
/// Represents a sequence of nodes connected by `EdgeKind::Derivation` edges,
/// starting from a root node.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivationChain {
    /// The ordered sequence of node IDs in this derivation chain.
    pub nodes: SmallVec<[NodeId; 4]>,
    /// The ordered sequence of edge IDs connecting the nodes.
    pub edges: SmallVec<[EdgeId; 4]>,
}

impl DerivationChain {
    /// Creates a new empty derivation chain.
    pub fn new() -> Self {
        Self {
            nodes: SmallVec::new(),
            edges: SmallVec::new(),
        }
    }

    /// Returns the length of the chain (number of edges).
    pub fn len(&self) -> usize {
        self.edges.len()
    }

    /// Returns `true` if the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Returns the first node in the chain (the root).
    pub fn root(&self) -> Option<&NodeId> {
        self.nodes.first()
    }

    /// Returns the last node in the chain (the leaf).
    pub fn leaf(&self) -> Option<&NodeId> {
        self.nodes.last()
    }
}

impl Default for DerivationChain {
    fn default() -> Self {
        Self::new()
    }
}

/// Information about a function, returned by LLM-friendly queries.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionInfo {
    /// The FunctionEntry node ID.
    pub entry_node_id: NodeId,
    /// The FunctionReturn node ID, if known.
    pub return_node_id: Option<NodeId>,
    /// The function's name/label.
    pub name: Option<String>,
    /// IDs of nodes that belong to this function.
    pub node_ids: Vec<NodeId>,
    /// IDs of functions this function calls (entry node IDs of callees).
    pub calls: Vec<NodeId>,
    /// IDs of functions that call this function (entry node IDs of callers).
    pub called_by: Vec<NodeId>,
    /// Whether this function is recursive.
    pub is_recursive: bool,
    /// Input node IDs (nodes with data flowing into this function from outside).
    pub input_nodes: Vec<NodeId>,
    /// Output node IDs (nodes whose data flows out of this function).
    pub output_nodes: Vec<NodeId>,
}

/// Execute a query against the given SCG.
///
/// This is the primary entry point for the query engine. It dispatches
/// to the appropriate handler based on the query variant.
///
/// # Returns
/// A `QueryResult` containing the matches. If the query references
/// non-existent nodes, the result will be empty.
pub fn execute(scg: &SCG, query: SCGQuery) -> QueryResult {
    match query {
        SCGQuery::NodesByType(node_type) => query_nodes_by_type(scg, node_type),
        SCGQuery::NodesByRegion(region_id) => query_nodes_by_region(scg, region_id),
        SCGQuery::AccessNodesToRegion(region_id) => query_access_nodes_to_region(scg, region_id),
        SCGQuery::DerivationChains { start, max_depth } => {
            query_derivation_chains(scg, start, max_depth)
        }
        SCGQuery::EdgesByKind(kind) => query_edges_by_kind(scg, kind),
        SCGQuery::DataFlowReachable { start, max_depth } => {
            query_data_flow_reachable(scg, start, max_depth)
        }
        SCGQuery::LeakedAllocations => query_leaked_allocations(scg),
        SCGQuery::PathsBetween {
            from,
            to,
            max_length,
        } => query_paths_between(scg, from, to, max_length),

        // LLM-friendly queries
        SCGQuery::ListFunctions => query_list_functions(scg),
        SCGQuery::FunctionInputsOutputs { function } => {
            query_function_inputs_outputs(scg, function)
        }
        SCGQuery::DataFlowPath {
            from,
            to,
            max_depth,
        } => query_data_flow_path(scg, from, to, max_depth),
        SCGQuery::CallersOf { function } => query_callers_of(scg, function),
    }
}

/// Helper to create a basic QueryResult without the new fields.
fn basic_result(
    node_ids: SmallVec<[NodeId; 8]>,
    edge_ids: SmallVec<[EdgeId; 8]>,
    derivation_chains: Vec<DerivationChain>,
    paths: Vec<Vec<NodeId>>,
) -> QueryResult {
    QueryResult {
        node_ids,
        edge_ids,
        derivation_chains,
        paths,
        functions: Vec::new(),
        data_flow_edges: Vec::new(),
    }
}

/// Find all nodes of a given `NodeType`.
fn query_nodes_by_type(scg: &SCG, node_type: NodeType) -> QueryResult {
    let node_ids: SmallVec<[NodeId; 8]> = scg
        .nodes()
        .filter(|n| n.node_type == node_type)
        .map(|n| n.id)
        .collect();
    basic_result(node_ids, SmallVec::new(), Vec::new(), Vec::new())
}

/// Find all nodes belonging to a specific region.
fn query_nodes_by_region(scg: &SCG, region_id: RegionId) -> QueryResult {
    let node_ids: SmallVec<[NodeId; 8]> = scg
        .get_region(region_id)
        .map(|r| r.iter_nodes().copied().collect())
        .unwrap_or_default();
    basic_result(node_ids, SmallVec::new(), Vec::new(), Vec::new())
}

/// Find all access nodes that target a specific region.
///
/// Returns all `NodeType::Access` nodes whose `AccessNode::region_id`
/// matches the given `RegionId`.
pub fn find_access_nodes_to_region(scg: &SCG, region_id: RegionId) -> QueryResult {
    let node_ids: SmallVec<[NodeId; 8]> = scg
        .nodes()
        .filter(|n| matches!(&n.payload, NodePayload::Access(a) if a.region_id == region_id))
        .map(|n| n.id)
        .collect();
    basic_result(node_ids, SmallVec::new(), Vec::new(), Vec::new())
}

fn query_access_nodes_to_region(scg: &SCG, region_id: RegionId) -> QueryResult {
    find_access_nodes_to_region(scg, region_id)
}

/// Find derivation chains starting from a given node.
///
/// Traverses `EdgeKind::Derivation` edges in a depth-first manner,
/// collecting all paths from the start node.
pub fn find_derivation_chains(scg: &SCG, start: NodeId, max_depth: usize) -> Vec<DerivationChain> {
    if scg.get_node(start).is_none() || max_depth == 0 {
        return Vec::new();
    }

    let mut chains = Vec::new();
    let mut current_chain = DerivationChain::new();
    current_chain.nodes.push(start);

    dfs_derivation(scg, start, max_depth, &mut current_chain, &mut chains);

    chains
}

fn dfs_derivation(
    scg: &SCG,
    current: NodeId,
    remaining_depth: usize,
    chain: &mut DerivationChain,
    results: &mut Vec<DerivationChain>,
) {
    if remaining_depth == 0 {
        if !chain.is_empty() {
            results.push(chain.clone());
        }
        return;
    }

    let successors = scg.successors(current).unwrap_or_default();

    // Find derivation edges from current to successors
    let mut derivation_edges: SmallVec<[(NodeId, EdgeId); 4]> = SmallVec::new();
    for &succ in &successors {
        for edge in scg.edges() {
            if edge.source == current
                && edge.target == succ
                && matches!(edge.kind, EdgeKind::Derivation)
            {
                derivation_edges.push((succ, edge.id));
            }
        }
    }

    if derivation_edges.is_empty() {
        // Leaf node: record this chain if it has at least one edge
        if !chain.is_empty() {
            results.push(chain.clone());
        }
        return;
    }

    for (succ, eid) in derivation_edges {
        chain.nodes.push(succ);
        chain.edges.push(eid);
        dfs_derivation(scg, succ, remaining_depth - 1, chain, results);
        chain.nodes.pop();
        chain.edges.pop();
    }
}

fn query_derivation_chains(scg: &SCG, start: NodeId, max_depth: usize) -> QueryResult {
    let chains = find_derivation_chains(scg, start, max_depth);
    let node_ids: SmallVec<[NodeId; 8]> = chains
        .iter()
        .flat_map(|c| c.nodes.iter().copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let edge_ids: SmallVec<[EdgeId; 8]> = chains
        .iter()
        .flat_map(|c| c.edges.iter().copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    basic_result(node_ids, edge_ids, chains, Vec::new())
}

/// Find all edges of a given `EdgeKind`.
fn query_edges_by_kind(scg: &SCG, kind: EdgeKind) -> QueryResult {
    let edge_ids: SmallVec<[EdgeId; 8]> = scg
        .edges()
        .filter(|e| e.kind == kind)
        .map(|e| e.id)
        .collect();
    basic_result(SmallVec::new(), edge_ids, Vec::new(), Vec::new())
}

/// Find all nodes reachable from a start node via data flow edges.
fn query_data_flow_reachable(scg: &SCG, start: NodeId, max_depth: usize) -> QueryResult {
    if scg.get_node(start).is_none() {
        return QueryResult::empty();
    }

    let mut visited: HashSet<NodeId> = HashSet::new();
    let mut frontier: SmallVec<[NodeId; 16]> = SmallVec::new();
    frontier.push(start);

    for _ in 0..max_depth {
        if frontier.is_empty() {
            break;
        }
        let mut next_frontier: SmallVec<[NodeId; 16]> = SmallVec::new();
        for node_id in frontier {
            if visited.insert(node_id) {
                if let Some(succs) = scg.successors(node_id) {
                    for succ in succs {
                        // Only follow data flow edges
                        for edge in scg.edges() {
                            if edge.source == node_id
                                && edge.target == succ
                                && matches!(edge.kind, EdgeKind::DataFlow)
                            {
                                next_frontier.push(succ);
                            }
                        }
                    }
                }
            }
        }
        frontier = next_frontier;
    }

    let node_ids: SmallVec<[NodeId; 8]> = visited.into_iter().collect();
    basic_result(node_ids, SmallVec::new(), Vec::new(), Vec::new())
}

/// Find all allocation nodes that lack a corresponding deallocation.
fn query_leaked_allocations(scg: &SCG) -> QueryResult {
    // Collect all allocation node IDs
    let allocation_ids: HashSet<NodeId> = scg
        .nodes()
        .filter(|n| matches!(n.node_type, NodeType::Allocation))
        .map(|n| n.id)
        .collect();

    // Collect all allocation IDs referenced by deallocation nodes
    let deallocated_ids: HashSet<NodeId> = scg
        .nodes()
        .filter_map(|n| match &n.payload {
            NodePayload::Deallocation(d) => Some(d.allocation_node),
            _ => None,
        })
        .collect();

    let leaked: SmallVec<[NodeId; 8]> = allocation_ids
        .difference(&deallocated_ids)
        .copied()
        .collect();

    basic_result(leaked, SmallVec::new(), Vec::new(), Vec::new())
}

/// Find paths between two nodes using bounded depth-first search.
fn query_paths_between(scg: &SCG, from: NodeId, to: NodeId, max_length: usize) -> QueryResult {
    if scg.get_node(from).is_none() || scg.get_node(to).is_none() {
        return QueryResult::empty();
    }

    let mut paths = Vec::new();
    let mut current_path = vec![from];
    let mut visited = HashSet::new();
    visited.insert(from);

    dfs_paths(
        scg,
        from,
        to,
        max_length,
        &mut current_path,
        &mut visited,
        &mut paths,
    );

    let node_ids: SmallVec<[NodeId; 8]> = paths
        .iter()
        .flat_map(|p| p.iter().copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    QueryResult {
        node_ids,
        edge_ids: SmallVec::new(),
        derivation_chains: Vec::new(),
        paths,
        functions: Vec::new(),
        data_flow_edges: Vec::new(),
    }
}

fn dfs_paths(
    scg: &SCG,
    current: NodeId,
    target: NodeId,
    remaining_length: usize,
    path: &mut Vec<NodeId>,
    visited: &mut HashSet<NodeId>,
    results: &mut Vec<Vec<NodeId>>,
) {
    if current == target {
        results.push(path.clone());
        return;
    }

    if remaining_length == 0 {
        return;
    }

    if let Some(succs) = scg.successors(current) {
        for succ in succs {
            if visited.insert(succ) {
                path.push(succ);
                dfs_paths(
                    scg,
                    succ,
                    target,
                    remaining_length - 1,
                    path,
                    visited,
                    results,
                );
                path.pop();
                visited.remove(&succ);
            }
        }
    }
}

// ── LLM-friendly query implementations ─────────────────────────────────

/// List all functions defined in the program.
fn query_list_functions(scg: &SCG) -> QueryResult {
    let call_graph = CallGraph::build(scg);
    let mut functions = Vec::new();

    for fid in call_graph.functions() {
        let name = call_graph.function_label(&fid).map(|s| s.to_string());
        let return_node_id = call_graph.function_return(&fid);
        let is_recursive = call_graph.is_recursive(&fid);

        // Collect nodes belonging to this function by walking ControlFlow
        let mut func_nodes = Vec::new();
        let mut visited = hashbrown::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(fid.0);
        visited.insert(fid.0);
        while let Some(current) = queue.pop_front() {
            func_nodes.push(current);
            if let Some(succs) = scg.successors(current) {
                for succ in succs {
                    if visited.insert(succ) {
                        // Only follow ControlFlow/DataFlow within the function
                        let is_cf = scg.edges().any(|e| {
                            e.source == current
                                && e.target == succ
                                && matches!(e.kind, EdgeKind::ControlFlow | EdgeKind::DataFlow)
                        });
                        let is_interproc = scg.edges().any(|e| {
                            e.source == current
                                && e.target == succ
                                && matches!(e.kind, EdgeKind::Call { .. } | EdgeKind::Return { .. })
                        });
                        if is_cf && !is_interproc {
                            queue.push_back(succ);
                        }
                    }
                }
            }
        }

        let calls: Vec<NodeId> = call_graph
            .callees(&fid)
            .iter()
            .map(|cge| cge.callee.0)
            .collect();

        let called_by: Vec<NodeId> = call_graph
            .callers(&fid)
            .iter()
            .map(|caller_fid| caller_fid.0)
            .collect();

        functions.push(FunctionInfo {
            entry_node_id: fid.0,
            return_node_id,
            name,
            node_ids: func_nodes,
            calls,
            called_by,
            is_recursive,
            input_nodes: Vec::new(),
            output_nodes: Vec::new(),
        });
    }

    let node_ids: SmallVec<[NodeId; 8]> = functions
        .iter()
        .map(|f| f.entry_node_id)
        .collect();

    QueryResult {
        node_ids,
        edge_ids: SmallVec::new(),
        derivation_chains: Vec::new(),
        paths: Vec::new(),
        functions,
        data_flow_edges: Vec::new(),
    }
}

/// Find the inputs and outputs of a specific function.
fn query_function_inputs_outputs(scg: &SCG, function: NodeId) -> QueryResult {
    let call_graph = CallGraph::build(scg);
    let fid = crate::callgraph::FunctionId(function);

    // Verify this is a function entry
    if !call_graph.functions().any(|f| *f == fid) {
        return QueryResult::empty();
    }

    // Collect all nodes in this function
    let mut func_nodes: HashSet<NodeId> = HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(function);
    func_nodes.insert(function);
    while let Some(current) = queue.pop_front() {
        if let Some(succs) = scg.successors(current) {
            for succ in succs {
                if func_nodes.insert(succ) {
                    let is_cf = scg.edges().any(|e| {
                        e.source == current
                            && e.target == succ
                            && matches!(e.kind, EdgeKind::ControlFlow | EdgeKind::DataFlow)
                    });
                    let is_interproc = scg.edges().any(|e| {
                        e.source == current
                            && e.target == succ
                            && matches!(e.kind, EdgeKind::Call { .. } | EdgeKind::Return { .. })
                    });
                    if is_cf && !is_interproc {
                        queue.push_back(succ);
                    }
                }
            }
        }
    }

    // Input nodes: nodes inside the function that have DataFlow edges from
    // nodes outside the function
    let mut input_nodes = Vec::new();
    for &node_id in &func_nodes {
        if let Some(preds) = scg.predecessors(node_id) {
            for pred in preds {
                if !func_nodes.contains(&pred) {
                    let has_data_flow = scg.edges().any(|e| {
                        e.source == pred
                            && e.target == node_id
                            && matches!(e.kind, EdgeKind::DataFlow | EdgeKind::Return { .. })
                    });
                    if has_data_flow && !input_nodes.contains(&node_id) {
                        input_nodes.push(node_id);
                    }
                }
            }
        }
    }

    // Output nodes: nodes inside the function that have DataFlow edges to
    // nodes outside the function
    let mut output_nodes = Vec::new();
    for &node_id in &func_nodes {
        if let Some(succs) = scg.successors(node_id) {
            for succ in succs {
                if !func_nodes.contains(&succ) {
                    let has_data_flow = scg.edges().any(|e| {
                        e.source == node_id
                            && e.target == succ
                            && matches!(e.kind, EdgeKind::DataFlow | EdgeKind::Call { .. })
                    });
                    if has_data_flow && !output_nodes.contains(&node_id) {
                        output_nodes.push(node_id);
                    }
                }
            }
        }
    }

    let name = call_graph.function_label(&fid).map(|s| s.to_string());
    let return_node_id = call_graph.function_return(&fid);
    let is_recursive = call_graph.is_recursive(&fid);
    let calls: Vec<NodeId> = call_graph.callees(&fid).iter().map(|cge| cge.callee.0).collect();
    let called_by: Vec<NodeId> = call_graph.callers(&fid).iter().map(|cf| cf.0).collect();

    let func_info = FunctionInfo {
        entry_node_id: function,
        return_node_id,
        name,
        node_ids: func_nodes.into_iter().collect(),
        calls,
        called_by,
        is_recursive,
        input_nodes,
        output_nodes,
    };

    QueryResult {
        node_ids: {
            let mut ids: SmallVec<[NodeId; 8]> = SmallVec::new();
            ids.push(function);
            ids
        },
        edge_ids: SmallVec::new(),
        derivation_chains: Vec::new(),
        paths: Vec::new(),
        functions: vec![func_info],
        data_flow_edges: Vec::new(),
    }
}

/// Find the data flow path between two nodes.
fn query_data_flow_path(scg: &SCG, from: NodeId, to: NodeId, max_depth: usize) -> QueryResult {
    if scg.get_node(from).is_none() || scg.get_node(to).is_none() {
        return QueryResult::empty();
    }

    // BFS to find the shortest data-flow path from `from` to `to`
    let mut visited: HashSet<NodeId> = HashSet::new();
    let mut parent: hashbrown::HashMap<NodeId, (NodeId, EdgeId)> = hashbrown::HashMap::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(from);
    visited.insert(from);

    let mut found = false;
    let mut depth = 0;
    while !queue.is_empty() && depth < max_depth && !found {
        let level_size = queue.len();
        for _ in 0..level_size {
            let current = queue.pop_front().unwrap();
            if current == to {
                found = true;
                break;
            }
            if let Some(succs) = scg.successors(current) {
                for succ in succs {
                    if visited.insert(succ) {
                        // Only follow DataFlow edges
                        for edge in scg.edges() {
                            if edge.source == current
                                && edge.target == succ
                                && matches!(edge.kind, EdgeKind::DataFlow)
                            {
                                parent.insert(succ, (current, edge.id));
                                queue.push_back(succ);
                                break;
                            }
                        }
                    }
                }
            }
        }
        depth += 1;
    }

    if !found {
        return QueryResult::empty();
    }

    // Reconstruct the path and collect edges
    let mut path = Vec::new();
    let mut data_flow_edges = Vec::new();
    let mut node_ids_set = HashSet::new();
    let mut current = to;
    while current != from {
        path.push(current);
        node_ids_set.insert(current);
        if let Some((prev, edge_id)) = parent.get(&current) {
            if let Some(edge) = scg.get_edge(*edge_id) {
                data_flow_edges.push(edge.clone());
            }
            current = *prev;
        } else {
            break;
        }
    }
    path.push(from);
    node_ids_set.insert(from);
    path.reverse();

    let node_ids: SmallVec<[NodeId; 8]> = node_ids_set.into_iter().collect();

    QueryResult {
        node_ids,
        edge_ids: SmallVec::new(),
        derivation_chains: Vec::new(),
        paths: vec![path],
        functions: Vec::new(),
        data_flow_edges,
    }
}

/// Find all functions that call a given function.
fn query_callers_of(scg: &SCG, function: NodeId) -> QueryResult {
    let call_graph = CallGraph::build(scg);
    let fid = crate::callgraph::FunctionId(function);

    let callers: SmallVec<[NodeId; 8]> = call_graph
        .callers(&fid)
        .iter()
        .map(|caller_fid| caller_fid.0)
        .collect();

    // Build FunctionInfo for each caller
    let mut functions = Vec::new();
    for caller_fid in call_graph.callers(&fid) {
        let name = call_graph.function_label(caller_fid).map(|s| s.to_string());
        let return_node_id = call_graph.function_return(caller_fid);
        functions.push(FunctionInfo {
            entry_node_id: caller_fid.0,
            return_node_id,
            name,
            node_ids: Vec::new(),
            calls: call_graph.callees(caller_fid).iter().map(|cge| cge.callee.0).collect(),
            called_by: call_graph.callers(caller_fid).iter().map(|cf| cf.0).collect(),
            is_recursive: call_graph.is_recursive(caller_fid),
            input_nodes: Vec::new(),
            output_nodes: Vec::new(),
        });
    }

    QueryResult {
        node_ids: callers,
        edge_ids: SmallVec::new(),
        derivation_chains: Vec::new(),
        paths: Vec::new(),
        functions,
        data_flow_edges: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::graph::SCG;
    use crate::node::{
        AccessNode, AllocationNode, ComputationNode, DeallocationNode, NodePayload, NodeType,
        ProgramPoint,
    };
    use crate::region::{DeploymentTarget, SCGRegion};

    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        }
    }

    #[test]
    fn test_query_nodes_by_type() {
        let mut scg = SCG::new();
        scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: RegionId::new(1),
                type_name: None,
            }),
            pp(),
        );
        scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "sub".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );

        let result = execute(&scg, SCGQuery::NodesByType(NodeType::Computation));
        assert_eq!(result.node_ids.len(), 2);

        let result = execute(&scg, SCGQuery::NodesByType(NodeType::Allocation));
        assert_eq!(result.node_ids.len(), 1);
    }

    #[test]
    fn test_query_access_nodes_to_region() {
        let mut scg = SCG::new();
        let region = RegionId::new(1);
        let other_region = RegionId::new(2);

        scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: crate::node::AccessMode::Read,
                region_id: region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: crate::node::AccessMode::Write,
                region_id: other_region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );

        let result = find_access_nodes_to_region(&scg, region);
        assert_eq!(result.node_ids.len(), 1);
    }

    #[test]
    fn test_query_leaked_allocations() {
        let mut scg = SCG::new();
        let region = RegionId::new(1);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );

        // No deallocation yet
        let result = execute(&scg, SCGQuery::LeakedAllocations);
        assert_eq!(result.node_ids.len(), 1);
        assert!(result.node_ids.contains(&alloc_id));

        // Add deallocation
        scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_id,
                region_id: region,
            }),
            pp(),
        );

        let result = execute(&scg, SCGQuery::LeakedAllocations);
        assert!(result.node_ids.is_empty());
    }

    #[test]
    fn test_query_derivation_chains() {
        let mut scg = SCG::new();
        let region = RegionId::new(1);

        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: region,
            }),
            pp(),
        );
        scg.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();

        let chains = find_derivation_chains(&scg, alloc, 5);
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].len(), 1);
    }

    #[test]
    fn test_query_edges_by_kind() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "a".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "b".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n1, n2, EdgeKind::ControlFlow).unwrap();

        let result = execute(&scg, SCGQuery::EdgesByKind(EdgeKind::DataFlow));
        assert_eq!(result.edge_ids.len(), 1);

        let result = execute(&scg, SCGQuery::EdgesByKind(EdgeKind::ControlFlow));
        assert_eq!(result.edge_ids.len(), 1);
    }

    #[test]
    fn test_query_nodes_by_region() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "a".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let mut region = SCGRegion::new(RegionId::new(1), DeploymentTarget::Heap);
        region.add_node(n1);
        scg.add_region(region);

        let result = execute(&scg, SCGQuery::NodesByRegion(RegionId::new(1)));
        assert_eq!(result.node_ids.len(), 1);
    }
}
