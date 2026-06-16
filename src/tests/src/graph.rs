//! Graph structure tests
//!
//! Tests for memory safety in graph data structures with adjacency
//! lists, covering creation, edge manipulation, traversal,
//! and cycle handling. Each test builds verifier inputs directly
//! using the per-invariant IVE APIs.

use vuma_ive::cleanup::{
    CleanupGraph, CleanupVerifier, OperationKind, ResourceId as CleanupResourceId,
    ResourceKind as CleanupResourceKind,
};
use vuma_ive::exclusivity::{
    AccessId as ExclusivityAccessId, AccessKind as ExclusivityAccessKind, AccessRecord,
    ExclusivityInput, ExclusivityVerifier, SyncEdgeRecord, SyncOrdering,
};
use vuma_ive::liveness::{
    ControlFlowEdge, EventAction, LivenessInput, LivenessVerifier, PointId, ResourceEvent,
    ResourceId as LivenessResourceId, ResourceKind as LivenessResourceKind, ThreadId,
};

/// Test: create a graph with 3 nodes, verify cleanup holds.
///
/// A freshly created graph allocates a header and vertex nodes.
/// When destroyed, all regions are freed. The cleanup verifier should
/// confirm no leaks.
#[test]
fn test_graph_create() {
    // Regions: 0=graph header, 1=vertexA, 2=vertexB, 3=vertexC

    // --- Verify cleanup invariant ---
    {
        let mut graph = CleanupGraph::new();
        let entry = graph.add_node(OperationKind::Passthrough, "entry");
        let alloc_hdr = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(0),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_graph_header",
        );
        let alloc_a = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(1),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_vertexA",
        );
        let alloc_b = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(2),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_vertexB",
        );
        let alloc_c = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(3),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_vertexC",
        );
        let free_a = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(1),
                kind: CleanupResourceKind::Memory,
            },
            "free_vertexA",
        );
        let free_b = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(2),
                kind: CleanupResourceKind::Memory,
            },
            "free_vertexB",
        );
        let free_c = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(3),
                kind: CleanupResourceKind::Memory,
            },
            "free_vertexC",
        );
        let free_hdr = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(0),
                kind: CleanupResourceKind::Memory,
            },
            "free_graph_header",
        );
        let ret = graph.add_node(OperationKind::Return, "return");

        graph.add_edge(entry, alloc_hdr).unwrap();
        graph.add_edge(alloc_hdr, alloc_a).unwrap();
        graph.add_edge(alloc_a, alloc_b).unwrap();
        graph.add_edge(alloc_b, alloc_c).unwrap();
        graph.add_edge(alloc_c, free_a).unwrap();
        graph.add_edge(free_a, free_b).unwrap();
        graph.add_edge(free_b, free_c).unwrap();
        graph.add_edge(free_c, free_hdr).unwrap();
        graph.add_edge(free_hdr, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            report.clean,
            "Cleanup should be Proven for graph_create (all regions freed), got violations: {:?}",
            report.violations
        );
    }

    // --- Verify liveness invariant ---
    {
        let mut input = LivenessInput::new();
        for rid in 0u64..=3 {
            input.add_event(ResourceEvent {
                resource: LivenessResourceId(rid),
                kind: LivenessResourceKind::Memory,
                event: EventAction::Allocate,
                point: PointId(rid + 1),
                thread: ThreadId(0),
            });
        }
        for rid in 0u64..=3 {
            input.add_event(ResourceEvent {
                resource: LivenessResourceId(rid),
                kind: LivenessResourceKind::Memory,
                event: EventAction::Deallocate,
                point: PointId(rid + 5),
                thread: ThreadId(0),
            });
        }
        for i in 1..=7 {
            input.add_cfg_edge(ControlFlowEdge {
                from: PointId(i),
                to: PointId(i + 1),
                conditional: false,
                label: None,
            });
        }

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);
        assert!(
            result.invariant_holds,
            "Liveness should hold for graph_create, got violations: {:?}",
            result.violations
        );
    }
}

/// Test: add an edge between vertices, verify exclusivity holds.
///
/// Adding an edge updates adjacency list pointers. Since these are
/// sequential pointer updates, the exclusivity verifier should
/// confirm no data races.
#[test]
fn test_graph_add_edge() {
    // --- Verify exclusivity invariant (sequential pointer updates) ---
    {
        let mut input = ExclusivityInput::new();

        // Write: add A→B edge (update A's adjacency list)
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(1),
            ExclusivityAccessKind::Write,
            0x2000, // A's adjacency list pointer
            8,
            "graph.vu:5".to_string(),
            1,
            1,
        ));
        // Write: add B→A edge (update B's adjacency list, undirected graph)
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(2),
            ExclusivityAccessKind::Write,
            0x3000, // B's adjacency list pointer — non-overlapping
            8,
            "graph.vu:6".to_string(),
            2,
            2,
        ));
        // Sequential ordering
        input.add_sync_edge(SyncEdgeRecord::new(
            ExclusivityAccessId(1),
            ExclusivityAccessId(2),
            SyncOrdering::HappensBefore,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);
        assert!(
            output.result.is_proven(),
            "Exclusivity should be Proven for add_edge (sequential non-overlapping writes), got: {:?}",
            output.result.status
        );
    }

    // --- Verify cleanup invariant (edge allocation freed) ---
    {
        let mut graph = CleanupGraph::new();
        let entry = graph.add_node(OperationKind::Passthrough, "entry");
        let alloc_hdr = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(0),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_graph",
        );
        let alloc_a = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(1),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_A",
        );
        let alloc_b = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(2),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_B",
        );
        let alloc_edge = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(3),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_edge_A_B",
        );
        let access_edge = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(3),
            },
            "access_edge",
        );
        let free_edge = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(3),
                kind: CleanupResourceKind::Memory,
            },
            "free_edge",
        );
        let free_a = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(1),
                kind: CleanupResourceKind::Memory,
            },
            "free_A",
        );
        let free_b = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(2),
                kind: CleanupResourceKind::Memory,
            },
            "free_B",
        );
        let free_hdr = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(0),
                kind: CleanupResourceKind::Memory,
            },
            "free_graph",
        );
        let ret = graph.add_node(OperationKind::Return, "return");

        graph.add_edge(entry, alloc_hdr).unwrap();
        graph.add_edge(alloc_hdr, alloc_a).unwrap();
        graph.add_edge(alloc_a, alloc_b).unwrap();
        graph.add_edge(alloc_b, alloc_edge).unwrap();
        graph.add_edge(alloc_edge, access_edge).unwrap();
        graph.add_edge(access_edge, free_edge).unwrap();
        graph.add_edge(free_edge, free_a).unwrap();
        graph.add_edge(free_a, free_b).unwrap();
        graph.add_edge(free_b, free_hdr).unwrap();
        graph.add_edge(free_hdr, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            report.clean,
            "Cleanup should be Proven for add_edge, got violations: {:?}",
            report.violations
        );
    }
}

/// Test: remove an edge, verify cleanup holds and liveness holds.
///
/// After removing an edge, the edge's memory should be properly freed.
/// The cleanup verifier should confirm no leaks, and the liveness
/// verifier should confirm no violations for the remaining vertices.
#[test]
fn test_graph_remove_edge() {
    // Regions: 0=graph header, 1=vertexA, 2=vertexB, 3=edge A→B

    // --- Verify cleanup invariant ---
    {
        let mut graph = CleanupGraph::new();
        let entry = graph.add_node(OperationKind::Passthrough, "entry");
        let alloc_hdr = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(0),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_graph",
        );
        let alloc_a = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(1),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_A",
        );
        let alloc_b = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(2),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_B",
        );
        let alloc_edge = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(3),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_edge",
        );
        let access_edge = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(3),
            },
            "access_edge",
        );
        // Remove edge: free edge memory
        let free_edge = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(3),
                kind: CleanupResourceKind::Memory,
            },
            "free_edge",
        );
        // Free remaining
        let free_a = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(1),
                kind: CleanupResourceKind::Memory,
            },
            "free_A",
        );
        let free_b = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(2),
                kind: CleanupResourceKind::Memory,
            },
            "free_B",
        );
        let free_hdr = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(0),
                kind: CleanupResourceKind::Memory,
            },
            "free_graph",
        );
        let ret = graph.add_node(OperationKind::Return, "return");

        graph.add_edge(entry, alloc_hdr).unwrap();
        graph.add_edge(alloc_hdr, alloc_a).unwrap();
        graph.add_edge(alloc_a, alloc_b).unwrap();
        graph.add_edge(alloc_b, alloc_edge).unwrap();
        graph.add_edge(alloc_edge, access_edge).unwrap();
        graph.add_edge(access_edge, free_edge).unwrap();
        graph.add_edge(free_edge, free_a).unwrap();
        graph.add_edge(free_a, free_b).unwrap();
        graph.add_edge(free_b, free_hdr).unwrap();
        graph.add_edge(free_hdr, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            report.clean,
            "Cleanup should be Proven after remove_edge, got violations: {:?}",
            report.violations
        );
    }

    // --- Verify liveness invariant ---
    {
        let mut input = LivenessInput::new();
        for rid in 0u64..=3 {
            input.add_event(ResourceEvent {
                resource: LivenessResourceId(rid),
                kind: LivenessResourceKind::Memory,
                event: EventAction::Allocate,
                point: PointId(rid + 1),
                thread: ThreadId(0),
            });
        }
        // Edge is freed first (removed), then vertices and header
        input.add_event(ResourceEvent {
            resource: LivenessResourceId(3),
            kind: LivenessResourceKind::Memory,
            event: EventAction::Deallocate,
            point: PointId(5),
            thread: ThreadId(0),
        });
        for rid in [1u64, 2, 0] {
            input.add_event(ResourceEvent {
                resource: LivenessResourceId(rid),
                kind: LivenessResourceKind::Memory,
                event: EventAction::Deallocate,
                point: PointId(6 + rid),
                thread: ThreadId(0),
            });
        }
        for i in 1..=8 {
            input.add_cfg_edge(ControlFlowEdge {
                from: PointId(i),
                to: PointId(i + 1),
                conditional: false,
                label: None,
            });
        }

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);
        assert!(
            result.invariant_holds,
            "Liveness should hold after remove_edge, got violations: {:?}",
            result.violations
        );
    }
}

/// Test: DFS/BFS traversal with pointer chasing.
///
/// Graph traversal involves following pointers between nodes and
/// their adjacency lists. The exclusivity verifier should prove
/// that all pointer reads are safe (reads never conflict), and
/// the liveness verifier should confirm no violations.
#[test]
fn test_graph_traverse() {
    // --- Verify exclusivity invariant (reads are safe) ---
    {
        let mut input = ExclusivityInput::new();

        // Read A's adjacency list
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(1),
            ExclusivityAccessKind::Read,
            0x2000,
            8,
            "graph.vu:15".to_string(),
            1,
            1,
        ));
        // Read B's adjacency list
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(2),
            ExclusivityAccessKind::Read,
            0x3000,
            8,
            "graph.vu:16".to_string(),
            2,
            2,
        ));
        // Read C's adjacency list
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(3),
            ExclusivityAccessKind::Read,
            0x4000,
            8,
            "graph.vu:17".to_string(),
            3,
            3,
        ));
        // Read D's adjacency list
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(4),
            ExclusivityAccessKind::Read,
            0x5000,
            8,
            "graph.vu:18".to_string(),
            4,
            4,
        ));
        // All reads — no sync edges needed since reads never conflict

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);
        assert!(
            output.result.is_proven(),
            "Exclusivity should be Proven for graph traversal (all reads), got: {:?}",
            output.result.status
        );
    }

    // --- Verify liveness invariant ---
    {
        let mut input = LivenessInput::new();
        // Regions: 0=header, 1-4=vertices
        for rid in 0u64..=4 {
            input.add_event(ResourceEvent {
                resource: LivenessResourceId(rid),
                kind: LivenessResourceKind::Memory,
                event: EventAction::Allocate,
                point: PointId(rid + 1),
                thread: ThreadId(0),
            });
        }
        for rid in 0u64..=4 {
            input.add_event(ResourceEvent {
                resource: LivenessResourceId(rid),
                kind: LivenessResourceKind::Memory,
                event: EventAction::Deallocate,
                point: PointId(rid + 6),
                thread: ThreadId(0),
            });
        }
        for i in 1..=9 {
            input.add_cfg_edge(ControlFlowEdge {
                from: PointId(i),
                to: PointId(i + 1),
                conditional: false,
                label: None,
            });
        }

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);
        assert!(
            result.invariant_holds,
            "Liveness should hold for graph traversal, got violations: {:?}",
            result.violations
        );
    }
}

/// Test: cyclic graph. Verify liveness holds (no deadlock since reads don't block).
///
/// A cycle (A → B → C → A) in a graph creates reference cycles
/// that could lead to memory leaks, but doesn't create deadlocks
/// since the liveness model tracks memory resources, not lock-based
/// synchronization. All regions are properly freed, so liveness holds.
#[test]
fn test_graph_cycle() {
    // Regions: 0=header, 1=A, 2=B, 3=C, 4=edge A→B, 5=edge B→C, 6=edge C→A

    // --- Verify liveness invariant (no deadlock) ---
    {
        let mut input = LivenessInput::new();
        for rid in 0u64..=6 {
            input.add_event(ResourceEvent {
                resource: LivenessResourceId(rid),
                kind: LivenessResourceKind::Memory,
                event: EventAction::Allocate,
                point: PointId(rid + 1),
                thread: ThreadId(0),
            });
        }
        // Free edges first, then vertices, then header
        for rid in [4u64, 5, 6, 1, 2, 3, 0] {
            input.add_event(ResourceEvent {
                resource: LivenessResourceId(rid),
                kind: LivenessResourceKind::Memory,
                event: EventAction::Deallocate,
                point: PointId(8 + rid),
                thread: ThreadId(0),
            });
        }
        // Build a linear CFG to ensure all deallocations are reachable
        for i in 1..=13 {
            input.add_cfg_edge(ControlFlowEdge {
                from: PointId(i),
                to: PointId(i + 1),
                conditional: false,
                label: None,
            });
        }

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);
        assert!(
            result.invariant_holds,
            "Liveness should hold for cyclic graph (no deadlock from reads), got violations: {:?}",
            result.violations
        );
    }

    // --- Verify cleanup invariant (all regions freed despite cycle) ---
    {
        let mut graph = CleanupGraph::new();
        let entry = graph.add_node(OperationKind::Passthrough, "entry");
        let alloc_hdr = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(0),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_header",
        );
        let alloc_a = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(1),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_A",
        );
        let alloc_b = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(2),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_B",
        );
        let alloc_c = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(3),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_C",
        );
        let alloc_e1 = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(4),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_edge_AB",
        );
        let alloc_e2 = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(5),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_edge_BC",
        );
        let alloc_e3 = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(6),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_edge_CA",
        );
        let free_e1 = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(4),
                kind: CleanupResourceKind::Memory,
            },
            "free_edge_AB",
        );
        let free_e2 = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(5),
                kind: CleanupResourceKind::Memory,
            },
            "free_edge_BC",
        );
        let free_e3 = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(6),
                kind: CleanupResourceKind::Memory,
            },
            "free_edge_CA",
        );
        let free_a = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(1),
                kind: CleanupResourceKind::Memory,
            },
            "free_A",
        );
        let free_b = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(2),
                kind: CleanupResourceKind::Memory,
            },
            "free_B",
        );
        let free_c = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(3),
                kind: CleanupResourceKind::Memory,
            },
            "free_C",
        );
        let free_hdr = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(0),
                kind: CleanupResourceKind::Memory,
            },
            "free_header",
        );
        let ret = graph.add_node(OperationKind::Return, "return");

        graph.add_edge(entry, alloc_hdr).unwrap();
        graph.add_edge(alloc_hdr, alloc_a).unwrap();
        graph.add_edge(alloc_a, alloc_b).unwrap();
        graph.add_edge(alloc_b, alloc_c).unwrap();
        graph.add_edge(alloc_c, alloc_e1).unwrap();
        graph.add_edge(alloc_e1, alloc_e2).unwrap();
        graph.add_edge(alloc_e2, alloc_e3).unwrap();
        graph.add_edge(alloc_e3, free_e1).unwrap();
        graph.add_edge(free_e1, free_e2).unwrap();
        graph.add_edge(free_e2, free_e3).unwrap();
        graph.add_edge(free_e3, free_a).unwrap();
        graph.add_edge(free_a, free_b).unwrap();
        graph.add_edge(free_b, free_c).unwrap();
        graph.add_edge(free_c, free_hdr).unwrap();
        graph.add_edge(free_hdr, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            report.clean,
            "Cleanup should be Proven for cyclic graph (all regions freed), got violations: {:?}",
            report.violations
        );
    }
}
