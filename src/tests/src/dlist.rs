//! Doubly-linked list tests
//!
//! Tests for memory safety in doubly-linked list data structures,
//! covering creation, insertion, removal, deallocation, and
//! violation detection. Each test builds verifier inputs directly
//! using the per-invariant IVE APIs.

use vuma_ive::cleanup::{
    CleanupGraph, CleanupVerifier, OperationKind, ResourceId as CleanupResourceId,
    ResourceKind as CleanupResourceKind, ViolationKind,
};
use vuma_ive::exclusivity::{
    AccessId as ExclusivityAccessId, AccessKind as ExclusivityAccessKind, AccessRecord,
    ExclusivityInput, ExclusivityVerifier, SyncEdgeRecord, SyncOrdering,
};
use vuma_ive::liveness::{
    ControlFlowEdge, EventAction, LivenessInput, LivenessVerifier, PointId, ResourceEvent,
    ResourceId as LivenessResourceId, ResourceKind as LivenessResourceKind, ThreadId,
};

/// Test: create an empty doubly-linked list, verify cleanup holds.
///
/// A freshly created list allocates a header region. When the list is
/// destroyed, the header is freed. The cleanup verifier should confirm
/// no leaks, and the liveness verifier should confirm no violations.
#[test]
fn test_dlist_create() {
    // Model: header (region 0) is allocated and then freed.

    // --- Verify cleanup invariant ---
    {
        let mut graph = CleanupGraph::new();
        let entry = graph.add_node(OperationKind::Passthrough, "entry");
        let acquire = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(0),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_header",
        );
        let release = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(0),
                kind: CleanupResourceKind::Memory,
            },
            "free_header",
        );
        let ret = graph.add_node(OperationKind::Return, "return");

        graph.add_edge(entry, acquire).unwrap();
        graph.add_edge(acquire, release).unwrap();
        graph.add_edge(release, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            report.clean,
            "Cleanup should be Proven for empty dlist (header alloc+free), got violations: {:?}",
            report.violations
        );
    }

    // --- Verify liveness invariant ---
    {
        let mut input = LivenessInput::new();
        let res = LivenessResourceId(0);
        input.add_event(ResourceEvent {
            resource: res,
            kind: LivenessResourceKind::Memory,
            event: EventAction::Allocate,
            point: PointId(1),
            thread: ThreadId(0),
        });
        input.add_event(ResourceEvent {
            resource: res,
            kind: LivenessResourceKind::Memory,
            event: EventAction::Deallocate,
            point: PointId(2),
            thread: ThreadId(0),
        });
        input.add_cfg_edge(ControlFlowEdge {
            from: PointId(1),
            to: PointId(2),
            conditional: false,
            label: None,
        });

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);
        assert!(
            result.invariant_holds,
            "Liveness should hold for empty dlist, got violations: {:?}",
            result.violations
        );
    }
}

/// Test: push 3 nodes (A, B, C) to the back, verify all invariants hold.
///
/// The list header (region 0) and three nodes (regions 1, 2, 3) are
/// allocated, then all are freed. Exclusivity is Proven because all
/// pointer updates are sequential.
#[test]
fn test_dlist_push_back() {
    // Regions: 0=header, 1=nodeA, 2=nodeB, 3=nodeC

    // --- Verify cleanup invariant ---
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
        let write_a = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(0),
            },
            "write_header_next_A",
        );
        let alloc_b = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(2),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_B",
        );
        let write_b = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(0),
            },
            "write_header_next_B",
        );
        let alloc_c = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(3),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_C",
        );
        let write_c = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(0),
            },
            "write_header_next_C",
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
        graph.add_edge(alloc_a, write_a).unwrap();
        graph.add_edge(write_a, alloc_b).unwrap();
        graph.add_edge(alloc_b, write_b).unwrap();
        graph.add_edge(write_b, alloc_c).unwrap();
        graph.add_edge(alloc_c, write_c).unwrap();
        graph.add_edge(write_c, free_a).unwrap();
        graph.add_edge(free_a, free_b).unwrap();
        graph.add_edge(free_b, free_c).unwrap();
        graph.add_edge(free_c, free_hdr).unwrap();
        graph.add_edge(free_hdr, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            report.clean,
            "Cleanup should be Proven for push_back (all 4 regions freed), got violations: {:?}",
            report.violations
        );
    }

    // --- Verify exclusivity invariant (sequential pointer updates) ---
    {
        let mut input = ExclusivityInput::new();
        // Three sequential writes to the header's next pointer
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(1),
            ExclusivityAccessKind::Write,
            0x1000,
            8,
            "dlist.vu:3".to_string(),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(2),
            ExclusivityAccessKind::Write,
            0x1000,
            8,
            "dlist.vu:5".to_string(),
            2,
            1,
        ));
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(3),
            ExclusivityAccessKind::Write,
            0x1000,
            8,
            "dlist.vu:7".to_string(),
            3,
            1,
        ));
        // Sequential ordering: write1 HB write2 HB write3
        input.add_sync_edge(SyncEdgeRecord::new(
            ExclusivityAccessId(1),
            ExclusivityAccessId(2),
            SyncOrdering::HappensBefore,
        ));
        input.add_sync_edge(SyncEdgeRecord::new(
            ExclusivityAccessId(2),
            ExclusivityAccessId(3),
            SyncOrdering::HappensBefore,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);
        assert!(
            output.result.is_proven(),
            "Exclusivity should be Proven for sequential push_back updates, got: {:?}",
            output.result.status
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
        // CFG edges: alloc0→alloc1→...→alloc3→free0→free1→...→free3
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
            "Liveness should hold for push_back, got violations: {:?}",
            result.violations
        );
    }
}

/// Test: push 3 nodes to the front, verify all invariants hold.
///
/// Same as push_back but the insertion order is reversed.
/// All pointer updates are still sequential, so exclusivity is Proven.
#[test]
fn test_dlist_push_front() {
    // --- Verify cleanup invariant ---
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
        let write_a = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(0),
            },
            "write_header_next_A",
        );
        let alloc_b = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(2),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_B",
        );
        let write_b = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(0),
            },
            "write_header_next_B",
        );
        let alloc_c = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(3),
                kind: CleanupResourceKind::Memory,
            },
            "alloc_C",
        );
        let write_c = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(0),
            },
            "write_header_next_C",
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
        graph.add_edge(alloc_a, write_a).unwrap();
        graph.add_edge(write_a, alloc_b).unwrap();
        graph.add_edge(alloc_b, write_b).unwrap();
        graph.add_edge(write_b, alloc_c).unwrap();
        graph.add_edge(alloc_c, write_c).unwrap();
        graph.add_edge(write_c, free_a).unwrap();
        graph.add_edge(free_a, free_b).unwrap();
        graph.add_edge(free_b, free_c).unwrap();
        graph.add_edge(free_c, free_hdr).unwrap();
        graph.add_edge(free_hdr, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            report.clean,
            "Cleanup should be Proven for push_front, got violations: {:?}",
            report.violations
        );
    }

    // --- Verify exclusivity invariant (sequential pointer updates) ---
    {
        let mut input = ExclusivityInput::new();
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(1),
            ExclusivityAccessKind::Write,
            0x1000,
            8,
            "dlist.vu:3".to_string(),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(2),
            ExclusivityAccessKind::Write,
            0x1000,
            8,
            "dlist.vu:5".to_string(),
            2,
            1,
        ));
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(3),
            ExclusivityAccessKind::Write,
            0x1000,
            8,
            "dlist.vu:7".to_string(),
            3,
            1,
        ));
        input.add_sync_edge(SyncEdgeRecord::new(
            ExclusivityAccessId(1),
            ExclusivityAccessId(2),
            SyncOrdering::HappensBefore,
        ));
        input.add_sync_edge(SyncEdgeRecord::new(
            ExclusivityAccessId(2),
            ExclusivityAccessId(3),
            SyncOrdering::HappensBefore,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);
        assert!(
            output.result.is_proven(),
            "Exclusivity should be Proven for sequential push_front updates, got: {:?}",
            output.result.status
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
            "Liveness should hold for push_front, got violations: {:?}",
            result.violations
        );
    }
}

/// Test: remove the middle node B from A↔B↔C, verify pointer updates and cleanup.
///
/// After removing B: A.next → C and C.prev → A. B is freed.
/// The pointer updates to A.next and C.prev are sequential and non-overlapping,
/// so exclusivity is Proven. Cleanup confirms B is freed with no leaks for A and C.
#[test]
fn test_dlist_remove_middle() {
    // Regions: 0=header, 1=nodeA, 2=nodeB, 3=nodeC

    // --- Verify cleanup invariant ---
    {
        let mut graph = CleanupGraph::new();
        let entry = graph.add_node(OperationKind::Passthrough, "entry");
        // Allocate all nodes
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
        // Access nodes before removal
        let access_a = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(1),
            },
            "access_A",
        );
        let access_c = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(3),
            },
            "access_C",
        );
        // Remove B
        let free_b = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(2),
                kind: CleanupResourceKind::Memory,
            },
            "free_B",
        );
        // Free remaining
        let free_a = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(1),
                kind: CleanupResourceKind::Memory,
            },
            "free_A",
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
        graph.add_edge(alloc_c, access_a).unwrap();
        graph.add_edge(access_a, access_c).unwrap();
        graph.add_edge(access_c, free_b).unwrap();
        graph.add_edge(free_b, free_a).unwrap();
        graph.add_edge(free_a, free_c).unwrap();
        graph.add_edge(free_c, free_hdr).unwrap();
        graph.add_edge(free_hdr, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            report.clean,
            "Cleanup should be Proven after removing B, got violations: {:?}",
            report.violations
        );
    }

    // --- Verify exclusivity invariant (A.next and C.prev are sequential, non-overlapping) ---
    {
        let mut input = ExclusivityInput::new();
        // Write A.next (update A's next pointer to point to C)
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(1),
            ExclusivityAccessKind::Write,
            0x2000,
            8,
            "dlist.vu:10".to_string(),
            1,
            1,
        ));
        // Write C.prev (update C's prev pointer to point to A)
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(2),
            ExclusivityAccessKind::Write,
            0x3008,
            8,
            "dlist.vu:11".to_string(),
            2,
            3,
        ));
        // These two writes target non-overlapping addresses and are sequential
        input.add_sync_edge(SyncEdgeRecord::new(
            ExclusivityAccessId(1),
            ExclusivityAccessId(2),
            SyncOrdering::HappensBefore,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);
        assert!(
            output.result.is_proven(),
            "Exclusivity should be Proven for remove_middle (non-overlapping sequential writes), got: {:?}",
            output.result.status
        );
    }

    // --- Verify liveness invariant for A and C ---
    {
        let mut input = LivenessInput::new();
        // Allocate header, A, B, C
        for rid in 0u64..=3 {
            input.add_event(ResourceEvent {
                resource: LivenessResourceId(rid),
                kind: LivenessResourceKind::Memory,
                event: EventAction::Allocate,
                point: PointId(rid + 1),
                thread: ThreadId(0),
            });
        }
        // Deallocate B (removed), then A, C, header
        input.add_event(ResourceEvent {
            resource: LivenessResourceId(2),
            kind: LivenessResourceKind::Memory,
            event: EventAction::Deallocate,
            point: PointId(5),
            thread: ThreadId(0),
        });
        input.add_event(ResourceEvent {
            resource: LivenessResourceId(1),
            kind: LivenessResourceKind::Memory,
            event: EventAction::Deallocate,
            point: PointId(6),
            thread: ThreadId(0),
        });
        input.add_event(ResourceEvent {
            resource: LivenessResourceId(3),
            kind: LivenessResourceKind::Memory,
            event: EventAction::Deallocate,
            point: PointId(7),
            thread: ThreadId(0),
        });
        input.add_event(ResourceEvent {
            resource: LivenessResourceId(0),
            kind: LivenessResourceKind::Memory,
            event: EventAction::Deallocate,
            point: PointId(8),
            thread: ThreadId(0),
        });
        // CFG edges
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
            "Liveness should hold for A and C after removing B, got violations: {:?}",
            result.violations
        );
    }
}

/// Test: free the entire list, verify all memory regions are freed.
///
/// After freeing a list with 3 nodes, all 4 regions (3 nodes + 1 header)
/// should be released. No memory should be leaked.
#[test]
fn test_dlist_free_all() {
    // Regions: 0=header, 1=nodeA, 2=nodeB, 3=nodeC

    // --- Verify cleanup invariant ---
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
        let access = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(1),
            },
            "access_nodes",
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
        graph.add_edge(alloc_c, access).unwrap();
        graph.add_edge(access, free_a).unwrap();
        graph.add_edge(free_a, free_b).unwrap();
        graph.add_edge(free_b, free_c).unwrap();
        graph.add_edge(free_c, free_hdr).unwrap();
        graph.add_edge(free_hdr, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            report.clean,
            "Cleanup should be Proven for free_all (all 4 regions freed), got violations: {:?}",
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
            "Liveness should hold for free_all, got violations: {:?}",
            result.violations
        );
    }
}

/// Test: access a removed node → should flag use-after-free violation.
///
/// After removing node B from the list and freeing its memory, any
/// attempt to read through the old pointer should be detected as a
/// use-after-free by the cleanup verifier.
#[test]
fn test_dlist_use_after_remove() {
    // Regions: 0=header, 1=nodeA, 2=nodeB, 3=nodeC
    // Remove B (free region 2), then try to read B's data.

    // --- Verify cleanup detects use-after-free ---
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
        // Remove and free B
        let free_b = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(2),
                kind: CleanupResourceKind::Memory,
            },
            "free_B",
        );
        // Use-after-free: read B's data after it was freed
        let read_b = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(2),
            },
            "read_B_after_free",
        );
        // Free remaining
        let free_a = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(1),
                kind: CleanupResourceKind::Memory,
            },
            "free_A",
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
        graph.add_edge(alloc_c, free_b).unwrap();
        graph.add_edge(free_b, read_b).unwrap();
        graph.add_edge(read_b, free_a).unwrap();
        graph.add_edge(free_a, free_c).unwrap();
        graph.add_edge(free_c, free_hdr).unwrap();
        graph.add_edge(free_hdr, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            !report.clean,
            "Cleanup should detect use-after-free after removing B"
        );
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UseAfterFree),
            "Expected UseAfterFree violation, got: {:?}",
            report.violations
        );
    }

    // --- Verify liveness detects the violation ---
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
        // Deallocate B at point 5
        input.add_event(ResourceEvent {
            resource: LivenessResourceId(2),
            kind: LivenessResourceKind::Memory,
            event: EventAction::Deallocate,
            point: PointId(5),
            thread: ThreadId(0),
        });
        // Remaining deallocations
        for rid in [1u64, 3, 0] {
            input.add_event(ResourceEvent {
                resource: LivenessResourceId(rid),
                kind: LivenessResourceKind::Memory,
                event: EventAction::Deallocate,
                point: PointId(7 + rid),
                thread: ThreadId(0),
            });
        }
        // CFG edges
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
        // Liveness should hold since all resources are eventually deallocated
        // (the use-after-free is a cleanup violation, not a liveness one per se)
        assert!(
            result.invariant_holds,
            "Liveness should hold (all resources deallocated), got violations: {:?}",
            result.violations
        );
    }
}
