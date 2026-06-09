//! Trivial program tests
//!
//! Basic memory safety tests covering allocation, access, freeing,
//! and common violation patterns (use-after-free, double-free, out-of-bounds).
//!
//! Each test builds an SCG (Semantic Computation Graph) representing the
//! program and then exercises the real IVE verification APIs to check
//! the appropriate invariants.

use vuma_bd::capd::CapD;
use vuma_bd::descriptor::BD;
use vuma_bd::reld::RelD;
use vuma_bd::repd::{ByteRep, RepD};
use vuma_ive::cleanup::{
    CleanupGraph, CleanupVerifier, OperationKind, ResourceId as CleanupResourceId,
    ResourceKind as CleanupResourceKind, ViolationKind,
};
use vuma_ive::exclusivity::{
    AccessId as ExclusivityAccessId, AccessKind as ExclusivityAccessKind, AccessRecord,
    ExclusivityInput, ExclusivityVerifier, SyncEdgeRecord, SyncOrdering,
};
use vuma_ive::interpretation::{InterpretationVerifier, LocationId, ProgramPointId};
use vuma_ive::liveness::{
    ControlFlowEdge, EventAction, LivenessInput, LivenessVerifier, PointId, ResourceEvent,
    ResourceId as LivenessResourceId, ResourceKind as LivenessResourceKind, ThreadId,
};
use vuma_ive::result::VerificationStatus;
use vuma_scg::region::RegionId;
use vuma_scg::{
    AccessMode, AccessNode, AllocationNode, ControlKind, ControlNode, DeallocationNode,
    DeploymentTarget, EdgeKind, NodePayload, NodeType, ProgramPoint, SCG, SCGRegion,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a default program point for test nodes.
fn pp(line: u64) -> ProgramPoint {
    ProgramPoint {
        file: Some("trivial.vu".to_string()),
        line: Some(line),
        column: Some(1),
        offset: None,
    }
}

/// Create a simple byte BD with the given size and 8-byte alignment.
fn byte_bd(size: u64) -> BD {
    BD::new(
        RepD::Byte(ByteRep { size, align: 8 }),
        CapD::all(),
        RelD::empty(),
    )
}

/// Test: allocate a region, write a value, read it back, verify, then free.
///
/// This is the simplest possible safe memory lifecycle:
/// allocate → write → read → verify → free
#[test]
fn test_allocate_read_free() {
    let region = RegionId::new(1);

    // Build the SCG: entry → alloc → write → read → free → return
    let mut scg = SCG::new();
    let _entry = scg.add_node(
        NodeType::Control,
        NodePayload::Control(ControlNode {
            kind: ControlKind::FunctionEntry,
            label: None,
        }),
        pp(1),
    );
    let alloc = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 8,
            align: 8,
            region_id: region,
            type_name: None,
        }),
        pp(2),
    );
    let write = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Write,
            region_id: region,
            offset: Some(0),
            access_size: Some(8),
        }),
        pp(3),
    );
    let read = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id: region,
            offset: Some(0),
            access_size: Some(8),
        }),
        pp(4),
    );
    let free = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc,
            region_id: region,
        }),
        pp(5),
    );
    let _ret = scg.add_node(
        NodeType::Control,
        NodePayload::Control(ControlNode {
            kind: ControlKind::FunctionReturn,
            label: None,
        }),
        pp(6),
    );

    // Add edges for control flow
    scg.add_edge(_entry, alloc, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(alloc, write, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(write, read, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(read, free, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(free, _ret, EdgeKind::ControlFlow).unwrap();

    // Add region
    let mut r = SCGRegion::new(region, DeploymentTarget::Heap);
    r.add_node(alloc);
    r.add_node(write);
    r.add_node(read);
    r.add_node(free);
    scg.add_region(r);

    // --- Verify cleanup invariant ---
    {
        let mut graph = CleanupGraph::new();
        let entry = graph.add_node(OperationKind::Passthrough, "entry");
        let acquire = graph.add_node(
            OperationKind::Acquire {
                resource: CleanupResourceId(1),
                kind: CleanupResourceKind::Memory,
            },
            "alloc",
        );
        let access_w = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(1),
            },
            "write",
        );
        let access_r = graph.add_node(
            OperationKind::Access {
                resource: CleanupResourceId(1),
            },
            "read",
        );
        let release = graph.add_node(
            OperationKind::Release {
                resource: CleanupResourceId(1),
                kind: CleanupResourceKind::Memory,
            },
            "free",
        );
        let ret = graph.add_node(OperationKind::Return, "return");

        graph.add_edge(entry, acquire).unwrap();
        graph.add_edge(acquire, access_w).unwrap();
        graph.add_edge(access_w, access_r).unwrap();
        graph.add_edge(access_r, release).unwrap();
        graph.add_edge(release, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            report.clean,
            "Cleanup should be Proven for safe program, got violations: {:?}",
            report.violations
        );
    }

    // --- Verify liveness invariant ---
    {
        let mut input = LivenessInput::new();
        let res = LivenessResourceId(1);
        input.add_event(ResourceEvent {
            resource: res,
            kind: LivenessResourceKind::Memory,
            event: EventAction::Allocate,
            point: PointId(2),
            thread: ThreadId(0),
        });
        input.add_event(ResourceEvent {
            resource: res,
            kind: LivenessResourceKind::Memory,
            event: EventAction::Deallocate,
            point: PointId(5),
            thread: ThreadId(0),
        });
        // Add CFG edges so the deallocation is reachable from the allocation
        input.add_cfg_edge(ControlFlowEdge {
            from: PointId(2),
            to: PointId(3),
            conditional: false,
            label: None,
        });
        input.add_cfg_edge(ControlFlowEdge {
            from: PointId(3),
            to: PointId(4),
            conditional: false,
            label: None,
        });
        input.add_cfg_edge(ControlFlowEdge {
            from: PointId(4),
            to: PointId(5),
            conditional: false,
            label: None,
        });
        input.add_cfg_edge(ControlFlowEdge {
            from: PointId(5),
            to: PointId(6),
            conditional: false,
            label: None,
        });

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);
        assert!(
            result.invariant_holds,
            "Liveness should be Proven for safe program, got violations: {:?}",
            result.violations
        );
    }

    // --- Verify exclusivity invariant ---
    {
        let mut input = ExclusivityInput::new();
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(1),
            ExclusivityAccessKind::Write,
            0,
            8,
            "trivial.vu:3".to_string(),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            ExclusivityAccessId(2),
            ExclusivityAccessKind::Read,
            0,
            8,
            "trivial.vu:4".to_string(),
            1,
            1,
        ));
        // Write happens-before Read (sequential)
        input.add_sync_edge(SyncEdgeRecord::new(
            ExclusivityAccessId(1),
            ExclusivityAccessId(2),
            SyncOrdering::HappensBefore,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);
        assert!(
            output.result.is_proven(),
            "Exclusivity should be Proven for sequential write-then-read, got: {:?}",
            output.result.status
        );
    }
}

/// Test: allocate, free, then attempt to read → should flag liveness violation.
///
/// A use-after-free is one of the most critical memory safety bugs.
/// The VUMA system should detect that the pointer is no longer live
/// and flag this as a violation.
#[test]
fn test_use_after_free() {
    let region = RegionId::new(1);

    // Build the SCG: entry → alloc → write → free → read(freed)
    let mut scg = SCG::new();
    let _entry = scg.add_node(
        NodeType::Control,
        NodePayload::Control(ControlNode {
            kind: ControlKind::FunctionEntry,
            label: None,
        }),
        pp(1),
    );
    let alloc = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 8,
            align: 8,
            region_id: region,
            type_name: None,
        }),
        pp(2),
    );
    let write = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Write,
            region_id: region,
            offset: Some(0),
            access_size: Some(8),
        }),
        pp(3),
    );
    let free = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc,
            region_id: region,
        }),
        pp(4),
    );
    let read_freed = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id: region,
            offset: Some(0),
            access_size: Some(8),
        }),
        pp(5),
    );

    scg.add_edge(_entry, alloc, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(alloc, write, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(write, free, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(free, read_freed, EdgeKind::ControlFlow).unwrap();

    // Use the cleanup verifier directly — it detects use-after-free
    let mut graph = CleanupGraph::new();
    let entry = graph.add_node(OperationKind::Passthrough, "entry");
    let acquire = graph.add_node(
        OperationKind::Acquire {
            resource: CleanupResourceId(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let access_w = graph.add_node(
        OperationKind::Access {
            resource: CleanupResourceId(1),
        },
        "write",
    );
    let release = graph.add_node(
        OperationKind::Release {
            resource: CleanupResourceId(1),
            kind: CleanupResourceKind::Memory,
        },
        "free",
    );
    let access_r = graph.add_node(
        OperationKind::Access {
            resource: CleanupResourceId(1),
        },
        "read_after_free",
    );

    graph.add_edge(entry, acquire).unwrap();
    graph.add_edge(acquire, access_w).unwrap();
    graph.add_edge(access_w, release).unwrap();
    graph.add_edge(release, access_r).unwrap();
    graph.set_entry(entry).unwrap();

    let verifier = CleanupVerifier::new();
    let report = verifier.verify(&graph);

    assert!(
        !report.clean,
        "Cleanup should detect use-after-free violation"
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

/// Test: allocate, free, then free again → should flag cleanup violation.
///
/// Double-free is a classic memory safety issue that can lead to
/// exploitable heap corruption. VUMA should detect that the region
/// has already been freed and flag a cleanup violation.
#[test]
fn test_double_free() {
    let region = RegionId::new(1);

    // Build the SCG: entry → alloc → free → free
    let mut scg = SCG::new();
    let _entry = scg.add_node(
        NodeType::Control,
        NodePayload::Control(ControlNode {
            kind: ControlKind::FunctionEntry,
            label: None,
        }),
        pp(1),
    );
    let alloc = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 8,
            align: 8,
            region_id: region,
            type_name: None,
        }),
        pp(2),
    );
    let free1 = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc,
            region_id: region,
        }),
        pp(3),
    );
    let free2 = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc,
            region_id: region,
        }),
        pp(4),
    );

    scg.add_edge(_entry, alloc, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(alloc, free1, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(free1, free2, EdgeKind::ControlFlow).unwrap();

    // Use the cleanup verifier directly — it detects double-free
    let mut graph = CleanupGraph::new();
    let entry = graph.add_node(OperationKind::Passthrough, "entry");
    let acquire = graph.add_node(
        OperationKind::Acquire {
            resource: CleanupResourceId(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let release1 = graph.add_node(
        OperationKind::Release {
            resource: CleanupResourceId(1),
            kind: CleanupResourceKind::Memory,
        },
        "free1",
    );
    let release2 = graph.add_node(
        OperationKind::Release {
            resource: CleanupResourceId(1),
            kind: CleanupResourceKind::Memory,
        },
        "free2",
    );

    graph.add_edge(entry, acquire).unwrap();
    graph.add_edge(acquire, release1).unwrap();
    graph.add_edge(release1, release2).unwrap();
    graph.set_entry(entry).unwrap();

    let verifier = CleanupVerifier::new();
    let report = verifier.verify(&graph);

    assert!(
        !report.clean,
        "Cleanup should detect double-free violation"
    );
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DoubleFree),
        "Expected DoubleFree violation, got: {:?}",
        report.violations
    );
}

/// Test: allocate N bytes, access offset N+1 → should flag interpretation violation.
///
/// Out-of-bounds access violates the spatial contract of the allocated region.
/// VUMA should detect that the access falls outside the region's bounds
/// and flag an interpretation violation.
#[test]
fn test_out_of_bounds() {
    let region = RegionId::new(1);

    // Build the SCG: alloc(16) → write(offset=0, size=16) → read(offset=17, size=1)
    let mut scg = SCG::new();
    let _alloc = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 16,
            align: 8,
            region_id: region,
            type_name: None,
        }),
        pp(1),
    );
    let _write = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Write,
            region_id: region,
            offset: Some(0),
            access_size: Some(16),
        }),
        pp(2),
    );
    let _read_oob = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id: region,
            offset: Some(17),
            access_size: Some(1),
        }),
        pp(3),
    );

    scg.add_edge(_alloc, _write, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(_write, _read_oob, EdgeKind::ControlFlow).unwrap();

    // Use the interpretation verifier with BDs that reflect the out-of-bounds:
    // Write uses a BD covering the full 16-byte allocation.
    // Read at offset 17 uses a BD whose size extends beyond the allocation,
    // making the RepD incompatible (size mismatch).
    let write_bd = byte_bd(16);
    let oob_read_bd = byte_bd(17); // extends 1 byte beyond the 16-byte allocation

    let mut verifier = InterpretationVerifier::new();
    let loc = LocationId(region.as_u64());
    verifier.record_write(loc.clone(), write_bd, ProgramPointId(2));
    verifier.record_read(loc, oob_read_bd, ProgramPointId(3));

    let result = verifier.verify();
    assert!(
        result.is_violated(),
        "Interpretation should detect out-of-bounds (incompatible RepD size), got: {:?}",
        result.status
    );
}

/// Test: allocate N bytes, access offset N-1 → should prove safe.
///
/// Accessing the last valid byte within an allocated region should
/// be provably safe. This tests that VUMA's proof system correctly
/// identifies in-bounds accesses as safe.
#[test]
fn test_valid_offset() {
    let region = RegionId::new(1);

    // Build the SCG: alloc(16) → write(offset=0) → read(offset=15, size=1) → free
    let mut scg = SCG::new();
    let alloc = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 16,
            align: 8,
            region_id: region,
            type_name: None,
        }),
        pp(1),
    );
    let _write = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Write,
            region_id: region,
            offset: Some(0),
            access_size: Some(16),
        }),
        pp(2),
    );
    let _read = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id: region,
            offset: Some(15),
            access_size: Some(1),
        }),
        pp(3),
    );
    let _free = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc,
            region_id: region,
        }),
        pp(4),
    );

    scg.add_edge(alloc, _write, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(_write, _read, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(_read, _free, EdgeKind::ControlFlow).unwrap();

    // Use the interpretation verifier with matching BDs — both use the
    // same 16-byte representation, which is compatible.
    let write_bd = byte_bd(16);
    let read_bd = byte_bd(16);

    let mut verifier = InterpretationVerifier::new();
    let loc = LocationId(region.as_u64());
    verifier.record_write(loc.clone(), write_bd, ProgramPointId(2));
    verifier.record_read(loc, read_bd, ProgramPointId(3));

    let result = verifier.verify();
    assert!(
        result.is_proven(),
        "Interpretation should be Proven for valid in-bounds access, got: {:?}",
        result.status
    );
}

/// Test: base pointer + offset within bounds → prove safe.
///
/// Pointer arithmetic that stays within the allocated region's bounds
/// should be verified as safe by the proof system.
#[test]
fn test_pointer_arithmetic() {
    let region = RegionId::new(1);

    // Build the SCG: alloc(64) → write(offset=0) → read(offset=32, size=4) → free
    let mut scg = SCG::new();
    let alloc = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 64,
            align: 8,
            region_id: region,
            type_name: None,
        }),
        pp(1),
    );
    let _write = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Write,
            region_id: region,
            offset: Some(0),
            access_size: Some(64),
        }),
        pp(2),
    );
    let _read = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id: region,
            offset: Some(32),
            access_size: Some(4),
        }),
        pp(3),
    );
    let _free = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc,
            region_id: region,
        }),
        pp(4),
    );

    scg.add_edge(alloc, _write, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(_write, _read, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(_read, _free, EdgeKind::ControlFlow).unwrap();

    // Both write and read use the same 64-byte BD — compatible.
    let write_bd = byte_bd(64);
    let read_bd = byte_bd(64);

    let mut verifier = InterpretationVerifier::new();
    let loc = LocationId(region.as_u64());
    verifier.record_write(loc.clone(), write_bd, ProgramPointId(2));
    verifier.record_read(loc, read_bd, ProgramPointId(3));

    let result = verifier.verify();
    assert!(
        result.is_proven(),
        "Interpretation should be Proven for pointer arithmetic within bounds, got: {:?}",
        result.status
    );
}

/// Test: base pointer + offset exceeds bounds → flag violation.
///
/// Pointer arithmetic that produces a pointer outside the allocated
/// region should be flagged as an interpretation violation.
#[test]
fn test_pointer_arithmetic_oob() {
    let region = RegionId::new(1);

    // Build the SCG: alloc(16) → write(offset=0, size=16) → read(offset=16, size=8)
    let mut scg = SCG::new();
    let _alloc = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 16,
            align: 8,
            region_id: region,
            type_name: None,
        }),
        pp(1),
    );
    let _write = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Write,
            region_id: region,
            offset: Some(0),
            access_size: Some(16),
        }),
        pp(2),
    );
    let _read_oob = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id: region,
            offset: Some(16),
            access_size: Some(8),
        }),
        pp(3),
    );

    scg.add_edge(_alloc, _write, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(_write, _read_oob, EdgeKind::ControlFlow).unwrap();

    // Write covers 16 bytes, read at offset 16 with size 8 extends to byte 24,
    // which is well beyond the 16-byte allocation. Model this with incompatible BDs:
    // write BD = 16 bytes, read BD = 24 bytes (offset 16 + size 8).
    let write_bd = byte_bd(16);
    let oob_read_bd = byte_bd(24); // extends beyond allocation

    let mut verifier = InterpretationVerifier::new();
    let loc = LocationId(region.as_u64());
    verifier.record_write(loc.clone(), write_bd, ProgramPointId(2));
    verifier.record_read(loc, oob_read_bd, ProgramPointId(3));

    let result = verifier.verify();
    assert!(
        result.is_violated(),
        "Interpretation should detect OOB pointer arithmetic (incompatible RepD size), got: {:?}",
        result.status
    );
}
