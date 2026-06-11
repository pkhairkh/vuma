//! # Round-trip Verification Tests
//!
//! Ensures that projection ↔ real SCG conversions are structurally
//! lossless for the subset of types that can be round-tripped.
//!
//! ## What is verified
//!
//! - **Node count and edge count** are preserved through a
//!   `vuma_scg::SCG → projection::SCG → vuma_scg::SCG` round-trip.
//! - **Node types** (`Allocation`, `Deallocation`, `Computation`, `Effect`,
//!   `Access`) are preserved exactly.
//! - **Edge kinds** (`DataFlow`, `ControlFlow`, `Derivation`, `Annotation`)
//!   are preserved exactly.
//! - **Region membership** is preserved (same nodes in same regions).

#[cfg(test)]
mod tests {
    use vuma_scg::{
        AccessMode, AccessNode, AllocationNode, ComputationNode, ControlKind, ControlNode,
        DeallocationNode, DeploymentTarget, EdgeKind, EffectNode, NodeId, NodePayload, NodeType,
        ProgramPoint, RegionId, SCGRegion, SCG,
    };

    /// Helper: a blank program point.
    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        }
    }

    // ── Test 1: Full SCG round-trip ──────────────────────────────────────

    #[test]
    fn test_full_scg_roundtrip() {
        let rid = RegionId::new(1);
        let mut real = SCG::new();

        let alloc = real.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 16,
                region_id: rid,
                type_name: Some("Buffer".to_string()),
            }),
            pp(),
        );
        let comp = real.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "process".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let dealloc = real.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: rid,
            }),
            pp(),
        );

        real.add_edge(alloc, comp, EdgeKind::DataFlow).unwrap();
        real.add_edge(comp, dealloc, EdgeKind::ControlFlow).unwrap();
        real.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();

        let mut region = SCGRegion::new(rid, DeploymentTarget::Heap);
        region.add_node(alloc);
        region.add_node(dealloc);
        real.add_region(region);

        // Forward conversion
        let proj = crate::scg_adapter::from_scg(&real);
        assert_eq!(proj.nodes.len(), 3, "projection should have 3 nodes");
        assert_eq!(proj.edges.len(), 3, "projection should have 3 edges");
        assert_eq!(proj.regions.len(), 1, "projection should have 1 region");

        // Reverse conversion
        let rt = crate::scg_adapter::to_scg(&proj);
        assert_eq!(rt.node_count(), 3, "roundtrip node count");
        assert_eq!(rt.edge_count(), 3, "roundtrip edge count");
        assert_eq!(rt.region_count(), 1, "roundtrip region count");

        // Node types preserved
        assert_eq!(
            rt.get_node(alloc).map(|n| n.node_type.clone()),
            Some(NodeType::Allocation),
        );
        assert_eq!(
            rt.get_node(comp).map(|n| n.node_type.clone()),
            Some(NodeType::Computation),
        );
        assert_eq!(
            rt.get_node(dealloc).map(|n| n.node_type.clone()),
            Some(NodeType::Deallocation),
        );

        // Region membership preserved
        let rt_region = rt.get_region(rid).expect("region should exist");
        assert!(
            rt_region.contains_node(&alloc),
            "alloc should be in region after roundtrip"
        );
        assert!(
            rt_region.contains_node(&dealloc),
            "dealloc should be in region after roundtrip"
        );
    }

    // ── Test 2: All node types round-trip ────────────────────────────────

    #[test]
    fn test_all_node_types_roundtrip() {
        let rid = RegionId::new(1);
        let mut real = SCG::new();

        // Add one of each NodeType
        let types_and_payloads: Vec<(NodeType, NodePayload)> = vec![
            (
                NodeType::Computation,
                NodePayload::Computation(ComputationNode {
                    operation: "add".to_string(),
                    result_type: None,
                    tail_call: false,
                }),
            ),
            (
                NodeType::Allocation,
                NodePayload::Allocation(AllocationNode {
                    size: 64,
                    align: 8,
                    region_id: rid,
                    type_name: None,
                }),
            ),
            (
                NodeType::Deallocation,
                NodePayload::Deallocation(DeallocationNode {
                    allocation_node: NodeId::new(1),
                    region_id: rid,
                }),
            ),
            (
                NodeType::Access,
                NodePayload::Access(AccessNode {
                    mode: AccessMode::Read,
                    region_id: rid,
                    offset: None,
                    access_size: None,
                }),
            ),
            (
                NodeType::Effect,
                NodePayload::Effect(EffectNode {
                    effect_kind: "io_write".to_string(),
                    is_observable: true,
                }),
            ),
            (
                NodeType::Control,
                NodePayload::Control(ControlNode {
                    kind: ControlKind::FunctionEntry,
                    label: Some("main".to_string()),
                }),
            ),
        ];

        let mut node_ids = Vec::new();
        for (nt, payload) in &types_and_payloads {
            let id = real.add_node(nt.clone(), payload.clone(), pp());
            node_ids.push(id);
        }

        let proj = crate::scg_adapter::from_scg(&real);
        assert_eq!(
            proj.nodes.len(),
            types_and_payloads.len(),
            "projection should have one node per type"
        );

        let rt = crate::scg_adapter::to_scg(&proj);
        assert_eq!(
            rt.node_count(),
            types_and_payloads.len(),
            "roundtrip should preserve node count"
        );

        // Verify each node type is preserved
        let expected_types: Vec<NodeType> = types_and_payloads
            .iter()
            .map(|(nt, _)| nt.clone())
            .collect();
        for (i, expected) in expected_types.iter().enumerate() {
            let id = node_ids[i];
            let rt_type = rt.get_node(id).map(|n| n.node_type.clone());
            assert_eq!(
                rt_type.as_ref(),
                Some(expected),
                "Node {} type should be {:?}, got {:?}",
                i,
                expected,
                rt_type
            );
        }
    }

    // ── Test 3: All edge types round-trip ────────────────────────────────

    #[test]
    fn test_all_edge_types_roundtrip() {
        let mut real = SCG::new();

        // Create enough nodes for edges
        let n1 = real.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "a".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = real.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "b".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n3 = real.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "c".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n4 = real.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "d".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );

        // Add edges of each round-trippable kind
        real.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        real.add_edge(n2, n3, EdgeKind::ControlFlow).unwrap();
        real.add_edge(n1, n3, EdgeKind::Derivation).unwrap();
        real.add_edge(n3, n4, EdgeKind::Annotation).unwrap();

        let proj = crate::scg_adapter::from_scg(&real);
        assert_eq!(proj.edges.len(), 4, "projection should have 4 edges");

        // Verify projection edge kinds
        let proj_kinds: Vec<crate::EdgeKind> = proj.edges.iter().map(|e| e.kind.clone()).collect();
        assert!(proj_kinds.contains(&crate::EdgeKind::DataFlow));
        assert!(proj_kinds.contains(&crate::EdgeKind::ControlFlow));
        assert!(proj_kinds.contains(&crate::EdgeKind::Derivation));
        assert!(proj_kinds.contains(&crate::EdgeKind::Annotation));

        // Round-trip back
        let rt = crate::scg_adapter::to_scg(&proj);
        assert_eq!(rt.edge_count(), 4, "roundtrip should preserve edge count");

        // Verify the round-tripped edge kinds
        let rt_kinds: Vec<EdgeKind> = rt.edges().map(|e| e.kind.clone()).collect();
        assert_eq!(
            rt_kinds
                .iter()
                .filter(|k| matches!(k, EdgeKind::DataFlow))
                .count(),
            1,
            "DataFlow edges after roundtrip"
        );
        assert_eq!(
            rt_kinds
                .iter()
                .filter(|k| matches!(k, EdgeKind::ControlFlow))
                .count(),
            1,
            "ControlFlow edges after roundtrip"
        );
        assert_eq!(
            rt_kinds
                .iter()
                .filter(|k| matches!(k, EdgeKind::Derivation))
                .count(),
            1,
            "Derivation edges after roundtrip"
        );
        assert_eq!(
            rt_kinds
                .iter()
                .filter(|k| matches!(k, EdgeKind::Annotation))
                .count(),
            1,
            "Annotation edges after roundtrip"
        );
    }
}
