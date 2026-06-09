//! Bridge from `vuma_scg::SCG` to `vuma_cor::types::SCG`.
//!
//! This module provides the `From` implementation that converts the real
//! Semantic Computation Graph defined in the `vuma-scg` crate into the
//! COR-internal representation used by the Continuous Optimization Runtime.
//!
//! ## Node Kind Mapping
//!
//! | `vuma_scg::NodeType` | `vuma_cor::types::NodeKind` | Rationale |
//! |----------------------|-----------------------------|-----------|
//! | Allocation, Deallocation, Access | Memory | Memory operations |
//! | Computation, Cast | Compute | Pure computation |
//! | Control (FunctionEntry, Branch, Join, …) | Entry | Control flow entry/exit |
//! | Effect | Call | Side-effecting, like calls |
//! | Phantom | Entry | Structural / analysis marker |
//!
//! ## Edge Weight Mapping
//!
//! | `vuma_scg::EdgeKind` | Weight | Rationale |
//! |----------------------|--------|-----------|
//! | ControlFlow | 10 | Hot path indicator for loop back-edges |
//! | DataFlow | 1 | Normal data dependency |
//! | Derivation | 1 | Semantic dependency |
//! | Annotation | 1 | Metadata attachment |

use crate::types::{EdgeId, NodeId, NodeKind, SCG, SCGEdge, SCGNode};
use std::collections::HashMap;

/// Maps a `vuma_scg::NodeType` to the COR-internal `NodeKind`.
///
/// The mapping collapses the finer-grained SCG node types into the
/// coarser categories used by the optimization engine.
fn map_node_type(node_type: &vuma_scg::NodeType) -> NodeKind {
    match node_type {
        vuma_scg::NodeType::Allocation
        | vuma_scg::NodeType::Deallocation
        | vuma_scg::NodeType::Access => NodeKind::Memory,

        vuma_scg::NodeType::Computation
        | vuma_scg::NodeType::Cast => NodeKind::Compute,

        vuma_scg::NodeType::Control
        | vuma_scg::NodeType::Phantom => NodeKind::Entry,

        vuma_scg::NodeType::Effect => NodeKind::Call,
    }
}

/// Computes a default edge weight based on the SCG edge kind.
///
/// Control-flow edges are given a higher weight (10) because they
/// represent hot paths — especially loop back-edges — that the
/// optimization engine should prioritise. Data-flow, derivation, and
/// annotation edges receive the baseline weight of 1.
fn edge_weight(kind: &vuma_scg::EdgeKind) -> u64 {
    match kind {
        vuma_scg::EdgeKind::ControlFlow => 10,
        vuma_scg::EdgeKind::DataFlow => 1,
        vuma_scg::EdgeKind::Derivation => 1,
        vuma_scg::EdgeKind::Annotation => 1,
    }
}

impl From<vuma_scg::SCG> for SCG {
    /// Converts a `vuma_scg::SCG` into a `vuma_cor::types::SCG`.
    ///
    /// Each SCG node is mapped to a COR `SCGNode` with its kind derived
    /// from the node type mapping. Each SCG edge is mapped to a COR
    /// `SCGEdge` with a weight determined by the edge kind. Incoming and
    /// outgoing edge lists are populated by iterating over all edges after
    /// the initial node insertion pass.
    fn from(scg: vuma_scg::SCG) -> Self {
        let mut cor_scg = SCG::new();

        // Phase 1: Insert all nodes.
        for node_data in scg.nodes() {
            let id: NodeId = node_data.id.as_u64();
            let kind = map_node_type(&node_data.node_type);
            let node = SCGNode::new(id, kind);
            cor_scg.insert_node(node);
        }

        // Phase 2: Insert all edges and track adjacency.
        //
        // We collect adjacency information first, then update the nodes,
        // because `insert_edge` only updates the edge HashMap but the
        // COR SCGNode stores incoming/outgoing edge IDs on the node itself.
        let mut incoming: HashMap<NodeId, Vec<EdgeId>> = HashMap::new();
        let mut outgoing: HashMap<NodeId, Vec<EdgeId>> = HashMap::new();
        let mut cor_edges: Vec<SCGEdge> = Vec::new();

        for edge_data in scg.edges() {
            let edge_id: EdgeId = edge_data.id.as_u64();
            let source: NodeId = edge_data.source.as_u64();
            let target: NodeId = edge_data.target.as_u64();
            let weight = edge_weight(&edge_data.kind);

            let cor_edge = SCGEdge {
                id: edge_id,
                source,
                target,
                weight,
            };
            cor_edges.push(cor_edge);

            incoming.entry(target).or_default().push(edge_id);
            outgoing.entry(source).or_default().push(edge_id);
        }

        // Insert edges into the SCG.
        for edge in cor_edges {
            cor_scg.insert_edge(edge);
        }

        // Phase 3: Update node adjacency lists.
        for (node_id, inc_edges) in incoming {
            if let Some(node) = cor_scg.get_node_mut(node_id) {
                node.incoming_edges = inc_edges;
            }
        }
        for (node_id, out_edges) in outgoing {
            if let Some(node) = cor_scg.get_node_mut(node_id) {
                node.outgoing_edges = out_edges;
            }
        }

        cor_scg
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vuma_scg::{
        ComputationNode, EdgeKind, NodePayload, NodeType, ProgramPoint, SCG as VumaSCG,
    };

    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: Some("test.vu".to_string()),
            line: Some(1),
            column: Some(1),
            offset: None,
        }
    }

    #[test]
    fn empty_scg_converts_to_empty_cor_scg() {
        let scg = VumaSCG::new();
        let cor_scg: SCG = scg.into();
        assert_eq!(cor_scg.node_count, 0);
        assert_eq!(cor_scg.edge_count, 0);
    }

    #[test]
    fn node_type_mapping() {
        // Computation → Compute
        assert_eq!(map_node_type(&NodeType::Computation), NodeKind::Compute);
        // Cast → Compute
        assert_eq!(map_node_type(&NodeType::Cast), NodeKind::Compute);
        // Allocation → Memory
        assert_eq!(map_node_type(&NodeType::Allocation), NodeKind::Memory);
        // Deallocation → Memory
        assert_eq!(map_node_type(&NodeType::Deallocation), NodeKind::Memory);
        // Access → Memory
        assert_eq!(map_node_type(&NodeType::Access), NodeKind::Memory);
        // Control → Entry
        assert_eq!(map_node_type(&NodeType::Control), NodeKind::Entry);
        // Phantom → Entry
        assert_eq!(map_node_type(&NodeType::Phantom), NodeKind::Entry);
        // Effect → Call
        assert_eq!(map_node_type(&NodeType::Effect), NodeKind::Call);
    }

    #[test]
    fn edge_weight_mapping() {
        assert_eq!(edge_weight(&EdgeKind::ControlFlow), 10);
        assert_eq!(edge_weight(&EdgeKind::DataFlow), 1);
        assert_eq!(edge_weight(&EdgeKind::Derivation), 1);
        assert_eq!(edge_weight(&EdgeKind::Annotation), 1);
    }

    #[test]
    fn nodes_and_edges_converted() {
        let mut scg = VumaSCG::new();

        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: Some("i32".to_string()),
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Effect,
            NodePayload::Effect(vuma_scg::EffectNode {
                effect_kind: "print".to_string(),
                is_observable: true,
            }),
            pp(),
        );

        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();

        let cor_scg: SCG = scg.into();

        assert_eq!(cor_scg.node_count, 2);
        assert_eq!(cor_scg.edge_count, 1);

        // Check node kinds
        let cn1 = cor_scg.get_node(n1.as_u64()).unwrap();
        assert_eq!(cn1.kind, NodeKind::Compute);
        let cn2 = cor_scg.get_node(n2.as_u64()).unwrap();
        assert_eq!(cn2.kind, NodeKind::Call);

        // Check edge
        let edge = cor_scg.edges.values().next().unwrap();
        assert_eq!(edge.source, n1.as_u64());
        assert_eq!(edge.target, n2.as_u64());
        assert_eq!(edge.weight, 1); // DataFlow → weight 1

        // Check adjacency
        assert_eq!(cn1.outgoing_edges.len(), 1);
        assert_eq!(cn2.incoming_edges.len(), 1);
    }

    #[test]
    fn control_flow_edge_gets_higher_weight() {
        let mut scg = VumaSCG::new();

        let n1 = scg.add_node(
            NodeType::Control,
            NodePayload::Control(vuma_scg::ControlNode {
                kind: vuma_scg::ControlKind::LoopHeader,
                label: Some("loop".to_string()),
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "body".to_string(),
                result_type: None,
            }),
            pp(),
        );

        scg.add_edge(n1, n2, EdgeKind::ControlFlow).unwrap();

        let cor_scg: SCG = scg.into();
        let edge = cor_scg.edges.values().next().unwrap();
        assert_eq!(edge.weight, 10); // ControlFlow → weight 10
    }
}
