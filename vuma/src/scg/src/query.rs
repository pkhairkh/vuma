//! SCG Query Engine
//!
//! This module provides a query interface for the Semantic Computation Graph.
//! Queries allow structured inspection of graph properties, such as finding
//! nodes of a specific type, tracing derivation chains, or identifying
//! access patterns within memory regions.

use hashbrown::HashSet;
use smallvec::SmallVec;

use crate::edge::{EdgeId, EdgeKind};
use crate::graph::SCG;
use crate::node::{NodeId, NodeType, NodePayload};
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
}

impl QueryResult {
    /// Creates an empty query result.
    pub fn empty() -> Self {
        Self {
            node_ids: SmallVec::new(),
            edge_ids: SmallVec::new(),
            derivation_chains: Vec::new(),
            paths: Vec::new(),
        }
    }

    /// Returns `true` if the result contains no matches.
    pub fn is_empty(&self) -> bool {
        self.node_ids.is_empty()
            && self.edge_ids.is_empty()
            && self.derivation_chains.is_empty()
            && self.paths.is_empty()
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
    }
}

/// Find all nodes of a given `NodeType`.
fn query_nodes_by_type(scg: &SCG, node_type: NodeType) -> QueryResult {
    let node_ids: SmallVec<[NodeId; 8]> = scg
        .nodes()
        .filter(|n| n.node_type == node_type)
        .map(|n| n.id)
        .collect();
    QueryResult {
        node_ids,
        edge_ids: SmallVec::new(),
        derivation_chains: Vec::new(),
        paths: Vec::new(),
    }
}

/// Find all nodes belonging to a specific region.
fn query_nodes_by_region(scg: &SCG, region_id: RegionId) -> QueryResult {
    let node_ids: SmallVec<[NodeId; 8]> = scg
        .get_region(region_id)
        .map(|r| r.iter_nodes().copied().collect())
        .unwrap_or_default();
    QueryResult {
        node_ids,
        edge_ids: SmallVec::new(),
        derivation_chains: Vec::new(),
        paths: Vec::new(),
    }
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
    QueryResult {
        node_ids,
        edge_ids: SmallVec::new(),
        derivation_chains: Vec::new(),
        paths: Vec::new(),
    }
}

fn query_access_nodes_to_region(scg: &SCG, region_id: RegionId) -> QueryResult {
    find_access_nodes_to_region(scg, region_id)
}

/// Find derivation chains starting from a given node.
///
/// Traverses `EdgeKind::Derivation` edges in a depth-first manner,
/// collecting all paths from the start node.
pub fn find_derivation_chains(
    scg: &SCG,
    start: NodeId,
    max_depth: usize,
) -> Vec<DerivationChain> {
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
    QueryResult {
        node_ids,
        edge_ids,
        derivation_chains: chains,
        paths: Vec::new(),
    }
}

/// Find all edges of a given `EdgeKind`.
fn query_edges_by_kind(scg: &SCG, kind: EdgeKind) -> QueryResult {
    let edge_ids: SmallVec<[EdgeId; 8]> = scg
        .edges()
        .filter(|e| e.kind == kind)
        .map(|e| e.id)
        .collect();
    QueryResult {
        node_ids: SmallVec::new(),
        edge_ids,
        derivation_chains: Vec::new(),
        paths: Vec::new(),
    }
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
    QueryResult {
        node_ids,
        edge_ids: SmallVec::new(),
        derivation_chains: Vec::new(),
        paths: Vec::new(),
    }
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

    QueryResult {
        node_ids: leaked,
        edge_ids: SmallVec::new(),
        derivation_chains: Vec::new(),
        paths: Vec::new(),
    }
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

    dfs_paths(scg, from, to, max_length, &mut current_path, &mut visited, &mut paths);

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
                dfs_paths(scg, succ, target, remaining_length - 1, path, visited, results);
                path.pop();
                visited.remove(&succ);
            }
        }
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
                result_type: None, tail_call: false }),
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
                result_type: None, tail_call: false }),
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
                result_type: None, tail_call: false }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "b".to_string(),
                result_type: None, tail_call: false }),
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
                result_type: None, tail_call: false }),
            pp(),
        );
        let mut region = SCGRegion::new(RegionId::new(1), DeploymentTarget::Heap);
        region.add_node(n1);
        scg.add_region(region);

        let result = execute(&scg, SCGQuery::NodesByRegion(RegionId::new(1)));
        assert_eq!(result.node_ids.len(), 1);
    }
}
