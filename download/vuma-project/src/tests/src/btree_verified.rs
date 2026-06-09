//! Verified binary tree implementation using VUMA's IVE verification.
//!
//! This module tests a binary tree data structure by manually constructing
//! IVE verification inputs (exclusivity, liveness, cleanup) and asserting
//! expected results. Each test models the memory operations of a binary
//! tree operation (insert, traverse, remove, dealloc) as IVE inputs and
//! verifies that the invariants hold (or are intentionally violated for
//! negative tests).
//!
//! # Binary Tree Node Layout
//!
//! Each node occupies a region of memory. For our model:
//!
//! ```text
//! struct BTreeNode {
//!     value: u64,        // offset 0, 8 bytes
//!     left_ptr: u64,     // offset 8, 8 bytes
//!     right_ptr: u64,    // offset 16, 8 bytes
//!     parent_ptr: u64,   // offset 24, 8 bytes
//! }
//! ```
//!
//! Total node size: 32 bytes.
//!
//! # Test Coverage
//!
//! 1. test_btree_insert_root — Insert root node, verify invariants
//! 2. test_btree_insert_left_right — Insert left and right children
//! 3. test_btree_traverse_inorder — In-order traversal, verify all reads are from live memory
//! 4. test_btree_remove_leaf — Remove leaf node, verify no use-after-free
//! 5. test_btree_remove_internal — Remove internal node with children, verify pointer reconnection
//! 6. test_btree_dealloc_all — Post-order deallocation, verify cleanup invariant
//! 7. test_btree_aliasing — Two pointers to same subtree through different paths
//! 8. test_btree_full_lifecycle — Create, insert, traverse, remove, dealloc

use vuma_ive::{
    // Exclusivity
    AccessRecord, CapDInfo, ConflictKind,
    ExclusivityAccessId as AccessId, ExclusivityAccessKind as AccessKind,
    ExclusivityInput, ExclusivityOutput, ExclusivityVerifier,
    SyncEdgeRecord, SyncOrdering, VerificationStatus,
    // Liveness
    DeadReason, EventAction, InitializationMap,
    LivenessInput, LivenessVerificationContext,
    LivenessVerifier, ObligationKind,
    PointId, ProofObligation, ResourceEvent, ResourceId, ResourceKind,
    ThreadId,
    // Cleanup
    AnnotatedCleanupGraph, CleanupGraph, CleanupNodeId, CleanupResourceId,
    CleanupResourceKind, CleanupReport, CleanupVerifier, LeakAnnotation, LeakReason,
    OperationKind,
};
use vuma_ive::liveness::ControlFlowEdge;
use vuma_ive::cleanup::ViolationKind as CleanupViolationKind;

// ---------------------------------------------------------------------------
// Constants for binary tree node layout
// ---------------------------------------------------------------------------

/// Size of a single binary tree node in bytes.
const NODE_SIZE: u64 = 32;

/// Offset of the value field within a node.
const _VALUE_OFFSET: u64 = 0;
/// Offset of the left_ptr field within a node.
const LEFT_PTR_OFFSET: u64 = 8;
/// Offset of the right_ptr field within a node.
const RIGHT_PTR_OFFSET: u64 = 16;
/// Offset of the parent_ptr field within a node.
const PARENT_PTR_OFFSET: u64 = 24;

// ---------------------------------------------------------------------------
// Helpers — Exclusivity
// ---------------------------------------------------------------------------

/// Create a ProgramPoint string.
fn pp(s: &str) -> String {
    s.to_string()
}

/// Create a write access record for a binary tree operation.
fn btree_write(id: u64, addr: u64, size: u64, point: &str, region_id: u64) -> AccessRecord {
    AccessRecord::new(AccessId(id), AccessKind::Write, addr, size, pp(point), id, region_id)
}

/// Create a read access record for a binary tree operation.
fn btree_read(id: u64, addr: u64, size: u64, point: &str, region_id: u64) -> AccessRecord {
    AccessRecord::new(AccessId(id), AccessKind::Read, addr, size, pp(point), id, region_id)
}

/// Verify an ExclusivityInput and return the output.
fn verify_exclusivity(input: &ExclusivityInput) -> ExclusivityOutput {
    ExclusivityVerifier::new().verify(input)
}

// ---------------------------------------------------------------------------
// Helpers — Liveness
// ---------------------------------------------------------------------------

/// Shorthand for PointId.
fn pid(id: u64) -> PointId {
    PointId(id)
}

/// Shorthand for ResourceId.
fn rid(id: u64) -> ResourceId {
    ResourceId(id)
}

/// Shorthand for ThreadId.
fn tid(id: u64) -> ThreadId {
    ThreadId(id)
}

/// Create a memory Allocate event.
fn alloc_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: rid(resource),
        kind: ResourceKind::Memory,
        event: EventAction::Allocate,
        point: pid(point),
        thread: tid(thread),
    }
}

/// Create a memory Deallocate event.
fn dealloc_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: rid(resource),
        kind: ResourceKind::Memory,
        event: EventAction::Deallocate,
        point: pid(point),
        thread: tid(thread),
    }
}

/// Create a memory Read event.
fn read_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: rid(resource),
        kind: ResourceKind::Memory,
        event: EventAction::Read,
        point: pid(point),
        thread: tid(thread),
    }
}

/// Create a memory Write event.
fn write_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: rid(resource),
        kind: ResourceKind::Memory,
        event: EventAction::Write,
        point: pid(point),
        thread: tid(thread),
    }
}

/// Create a simple unconditional CFG edge.
fn cfg_edge(from: u64, to: u64) -> ControlFlowEdge {
    ControlFlowEdge {
        from: pid(from),
        to: pid(to),
        conditional: false,
        label: None,
    }
}

/// Build a linear CFG from a sequence of point IDs.
fn linear_cfg(points: &[u64]) -> Vec<ControlFlowEdge> {
    points
        .windows(2)
        .map(|w| cfg_edge(w[0], w[1]))
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers — Cleanup
// ---------------------------------------------------------------------------

/// Shorthand for a Cleanup ResourceId.
fn crid(id: u64) -> CleanupResourceId {
    CleanupResourceId(id)
}

/// Verify a plain CleanupGraph and return the report.
fn verify_cleanup(graph: &CleanupGraph) -> CleanupReport {
    CleanupVerifier::new().verify(graph)
}

// ===========================================================================
// Test 1: test_btree_insert_root — Insert root node, verify invariants
// ===========================================================================

/// Insert a root node into an empty binary tree and verify:
/// - **Exclusivity**: A single write to the node's memory has no conflicts.
/// - **Liveness**: The allocation is paired with a deallocation; no leaks.
/// - **Cleanup**: Acquire → Access → Release → Return is clean.
#[test]
fn test_btree_insert_root() {
    // Memory layout: root node at address 0x1000, size 32 bytes
    let root_addr: u64 = 0x1000;
    let root_region: u64 = 1;

    // --- Exclusivity: single write to root node fields ---
    let mut excl_input = ExclusivityInput::new();
    // Write the value field (offset 0, 8 bytes)
    excl_input.add_access(btree_write(1, root_addr, 8, "btree.rs:insert_root_value", root_region));
    // Write the left_ptr field (offset 8, 8 bytes) — null
    excl_input.add_access(btree_write(2, root_addr + LEFT_PTR_OFFSET, 8, "btree.rs:insert_root_left", root_region));
    // Write the right_ptr field (offset 16, 8 bytes) — null
    excl_input.add_access(btree_write(3, root_addr + RIGHT_PTR_OFFSET, 8, "btree.rs:insert_root_right", root_region));
    // Write the parent_ptr field (offset 24, 8 bytes) — null
    excl_input.add_access(btree_write(4, root_addr + PARENT_PTR_OFFSET, 8, "btree.rs:insert_root_parent", root_region));
    // All writes are sequential (happens-before)
    excl_input.add_sync_edge(SyncEdgeRecord::new(AccessId(1), AccessId(2), SyncOrdering::HappensBefore));
    excl_input.add_sync_edge(SyncEdgeRecord::new(AccessId(2), AccessId(3), SyncOrdering::HappensBefore));
    excl_input.add_sync_edge(SyncEdgeRecord::new(AccessId(3), AccessId(4), SyncOrdering::HappensBefore));

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "Sequential writes to non-overlapping fields of root node should be Proven, got: {:?}",
        excl_output.result.status
    );
    assert_eq!(excl_output.conflict_count(), 0, "No conflicts expected for sequential field writes");

    // --- Liveness: alloc → write → read → dealloc ---
    let mut liveness_input = LivenessInput::new();
    liveness_input.add_event(alloc_event(1, 1, 1));   // alloc root at PP1
    liveness_input.add_event(write_event(1, 2, 1));    // write root at PP2
    liveness_input.add_event(read_event(1, 3, 1));     // read root at PP3
    liveness_input.add_event(dealloc_event(1, 4, 1));  // dealloc root at PP4
    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);
    liveness_input.entry_point = Some(pid(1));

    let mut liveness_verifier = LivenessVerifier::new();
    let liveness_result = liveness_verifier.verify(&liveness_input);
    assert!(
        liveness_result.invariant_holds,
        "Root node insert should have no liveness violations, got: {:?}",
        liveness_result.violations
    );
    assert!(liveness_result.violations.is_empty());

    // --- Cleanup: acquire → access → release → return ---
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let alloc = cg.add_node(
        OperationKind::Acquire {
            resource: crid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_root",
    );
    let access = cg.add_node(
        OperationKind::Access { resource: crid(1) },
        "access_root",
    );
    let free = cg.add_node(
        OperationKind::Release {
            resource: crid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_root",
    );
    let ret = cg.add_node(OperationKind::Return, "return");
    cg.add_edge(entry, alloc).unwrap();
    cg.add_edge(alloc, access).unwrap();
    cg.add_edge(access, free).unwrap();
    cg.add_edge(free, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let cleanup_report = verify_cleanup(&cg);
    assert!(cleanup_report.clean, "Root node lifecycle should be clean");
    assert!(cleanup_report.violations.is_empty());
}

// ===========================================================================
// Test 2: test_btree_insert_left_right — Insert left and right children
// ===========================================================================

/// After inserting root, insert left child (address 0x2000) and right child
/// (address 0x3000). Verify:
/// - **Exclusivity**: Writes to different node addresses are non-overlapping
///   and should produce no conflicts. Writes to update parent/child pointers
///   are sequential (happens-before).
/// - **Liveness**: All three allocations are paired with deallocations.
/// - **Cleanup**: Three resources acquired, accessed, and released — clean.
#[test]
fn test_btree_insert_left_right() {
    let root_addr: u64 = 0x1000;
    let left_addr: u64 = 0x2000;
    let right_addr: u64 = 0x3000;

    // --- Exclusivity: writes to root, left, and right nodes ---
    let mut excl_input = ExclusivityInput::new();
    // Phase 1: Write root fields
    excl_input.add_access(btree_write(1, root_addr + LEFT_PTR_OFFSET, 8, "btree.rs:root_left_ptr", 1));
    excl_input.add_access(btree_write(2, root_addr + RIGHT_PTR_OFFSET, 8, "btree.rs:root_right_ptr", 1));
    // Phase 2: Write left child fields
    excl_input.add_access(btree_write(3, left_addr, 8, "btree.rs:left_value", 2));
    excl_input.add_access(btree_write(4, left_addr + PARENT_PTR_OFFSET, 8, "btree.rs:left_parent", 2));
    // Phase 3: Write right child fields
    excl_input.add_access(btree_write(5, right_addr, 8, "btree.rs:right_value", 3));
    excl_input.add_access(btree_write(6, right_addr + PARENT_PTR_OFFSET, 8, "btree.rs:right_parent", 3));

    // Sequential ordering: root → left → right
    excl_input.add_sync_edge(SyncEdgeRecord::new(AccessId(1), AccessId(2), SyncOrdering::HappensBefore));
    excl_input.add_sync_edge(SyncEdgeRecord::new(AccessId(2), AccessId(3), SyncOrdering::HappensBefore));
    excl_input.add_sync_edge(SyncEdgeRecord::new(AccessId(3), AccessId(4), SyncOrdering::HappensBefore));
    excl_input.add_sync_edge(SyncEdgeRecord::new(AccessId(4), AccessId(5), SyncOrdering::HappensBefore));
    excl_input.add_sync_edge(SyncEdgeRecord::new(AccessId(5), AccessId(6), SyncOrdering::HappensBefore));

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "Sequential writes to distinct nodes should be Proven, got: {:?}",
        excl_output.result.status
    );
    assert_eq!(excl_output.conflict_count(), 0);

    // --- Liveness: alloc root → write root → alloc left → write left →
    //     alloc right → write right → read all → dealloc all ---
    let mut liveness_input = LivenessInput::new();
    // Root: PP1-4
    liveness_input.add_event(alloc_event(1, 1, 1));
    liveness_input.add_event(write_event(1, 2, 1));
    liveness_input.add_event(read_event(1, 3, 1));
    liveness_input.add_event(dealloc_event(1, 4, 1));
    // Left: PP5-8
    liveness_input.add_event(alloc_event(2, 5, 1));
    liveness_input.add_event(write_event(2, 6, 1));
    liveness_input.add_event(read_event(2, 7, 1));
    liveness_input.add_event(dealloc_event(2, 8, 1));
    // Right: PP9-12
    liveness_input.add_event(alloc_event(3, 9, 1));
    liveness_input.add_event(write_event(3, 10, 1));
    liveness_input.add_event(read_event(3, 11, 1));
    liveness_input.add_event(dealloc_event(3, 12, 1));
    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    liveness_input.entry_point = Some(pid(1));

    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify(&liveness_input);
    assert!(
        result.invariant_holds,
        "Left/right child insertion should have no liveness violations, got: {:?}",
        result.violations
    );
    assert_eq!(result.resources_checked, 3);

    // --- Cleanup: three resources, each with acquire → access → release ---
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let mut last = entry;
    for i in 1..=3 {
        let alloc = cg.add_node(
            OperationKind::Acquire {
                resource: crid(i),
                kind: CleanupResourceKind::Memory,
            },
            format!("alloc_node{}", i),
        );
        let access = cg.add_node(
            OperationKind::Access { resource: crid(i) },
            format!("access_node{}", i),
        );
        let free = cg.add_node(
            OperationKind::Release {
                resource: crid(i),
                kind: CleanupResourceKind::Memory,
            },
            format!("free_node{}", i),
        );
        cg.add_edge(last, alloc).unwrap();
        cg.add_edge(alloc, access).unwrap();
        cg.add_edge(access, free).unwrap();
        last = free;
    }
    let ret = cg.add_node(OperationKind::Return, "return");
    cg.add_edge(last, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let report = verify_cleanup(&cg);
    assert!(report.clean, "Three-node tree lifecycle should be clean");
    assert_eq!(report.acquires_checked, 3);
}

// ===========================================================================
// Test 3: test_btree_traverse_inorder — In-order traversal, verify all reads
//          are from live memory
// ===========================================================================

/// Build a 3-node tree (root + left + right), then perform an in-order
/// traversal: read left → read root → read right. Verify:
/// - **Exclusivity**: All reads to overlapping addresses don't conflict
///   (reads never conflict with each other).
/// - **Liveness**: All reads occur while the node is live (between alloc
///   and dealloc), producing no use-after-free.
/// - **Cleanup**: All three nodes freed after traversal.
#[test]
fn test_btree_traverse_inorder() {
    let root_addr: u64 = 0x1000;
    let left_addr: u64 = 0x2000;
    let right_addr: u64 = 0x3000;

    // --- Exclusivity: reads during in-order traversal ---
    let mut excl_input = ExclusivityInput::new();
    // In-order: left value, left parent_ptr, root value, root left_ptr, root right_ptr,
    //           right value, right parent_ptr
    excl_input.add_access(btree_read(1, left_addr, 8, "btree.rs:traverse_left_val", 2));
    excl_input.add_access(btree_read(2, left_addr + PARENT_PTR_OFFSET, 8, "btree.rs:traverse_left_parent", 2));
    excl_input.add_access(btree_read(3, root_addr, 8, "btree.rs:traverse_root_val", 1));
    excl_input.add_access(btree_read(4, root_addr + LEFT_PTR_OFFSET, 8, "btree.rs:traverse_root_left", 1));
    excl_input.add_access(btree_read(5, root_addr + RIGHT_PTR_OFFSET, 8, "btree.rs:traverse_root_right", 1));
    excl_input.add_access(btree_read(6, right_addr, 8, "btree.rs:traverse_right_val", 3));
    excl_input.add_access(btree_read(7, right_addr + PARENT_PTR_OFFSET, 8, "btree.rs:traverse_right_parent", 3));

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "In-order traversal reads should be Proven (reads never conflict)"
    );
    assert_eq!(excl_output.conflict_count(), 0);

    // --- Liveness: alloc all → write all → traverse (read all) → dealloc all ---
    let mut liveness_input = LivenessInput::new();
    // Setup phase: allocate and write all three nodes
    liveness_input.add_event(alloc_event(1, 1, 1));    // alloc root
    liveness_input.add_event(write_event(1, 2, 1));     // write root
    liveness_input.add_event(alloc_event(2, 3, 1));    // alloc left
    liveness_input.add_event(write_event(2, 4, 1));     // write left
    liveness_input.add_event(alloc_event(3, 5, 1));    // alloc right
    liveness_input.add_event(write_event(3, 6, 1));     // write right
    // Traversal: read all nodes (in-order)
    liveness_input.add_event(read_event(2, 7, 1));      // read left
    liveness_input.add_event(read_event(1, 8, 1));      // read root
    liveness_input.add_event(read_event(3, 9, 1));      // read right
    // Teardown: dealloc all
    liveness_input.add_event(dealloc_event(2, 10, 1));  // dealloc left
    liveness_input.add_event(dealloc_event(1, 11, 1));  // dealloc root
    liveness_input.add_event(dealloc_event(3, 12, 1));  // dealloc right
    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    liveness_input.entry_point = Some(pid(1));

    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify(&liveness_input);
    assert!(
        result.invariant_holds,
        "In-order traversal should have no liveness violations, got: {:?}",
        result.violations
    );
    assert!(result.violations.is_empty());

    // Verify no use-after-free via liveness paths
    let context = LivenessVerificationContext::new(liveness_input);
    let paths = verifier.compute_liveness_paths(&context);
    for path in &paths {
        assert!(
            path.access_after_free.is_empty(),
            "Node {} should have no access-after-free during traversal, got: {:?}",
            path.resource_id,
            path.access_after_free
        );
    }

    // --- Cleanup: all three nodes properly freed ---
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let mut last = entry;
    // Acquire all nodes
    for i in 1..=3 {
        let alloc = cg.add_node(
            OperationKind::Acquire {
                resource: crid(i),
                kind: CleanupResourceKind::Memory,
            },
            format!("alloc_node{}", i),
        );
        let access = cg.add_node(
            OperationKind::Access { resource: crid(i) },
            format!("traverse_node{}", i),
        );
        cg.add_edge(last, alloc).unwrap();
        cg.add_edge(alloc, access).unwrap();
        last = access;
    }
    // Release all nodes (post-order for a tree)
    for i in [2u64, 1, 3] {
        let free = cg.add_node(
            OperationKind::Release {
                resource: crid(i),
                kind: CleanupResourceKind::Memory,
            },
            format!("free_node{}", i),
        );
        cg.add_edge(last, free).unwrap();
        last = free;
    }
    let ret = cg.add_node(OperationKind::Return, "return");
    cg.add_edge(last, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let report = verify_cleanup(&cg);
    assert!(report.clean, "Post-traversal cleanup should be clean");
}

// ===========================================================================
// Test 4: test_btree_remove_leaf — Remove leaf node, verify no use-after-free
// ===========================================================================

/// Create a tree with root + left child. Remove the left child (a leaf).
/// Verify:
/// - **Cleanup**: No use-after-free — the leaf is freed and then never
///   accessed again.
/// - **Liveness**: After freeing the left child, no further reads of that
///   region occur.
/// - **Exclusivity**: The write to update root's left_ptr (to null) after
///   removing the leaf is properly ordered.
///
/// Also includes a negative test: accessing the leaf after it's freed
/// should produce a use-after-free violation.
#[test]
fn test_btree_remove_leaf() {
    let root_addr: u64 = 0x1000;
    let left_addr: u64 = 0x2000;

    // --- Positive test: correct removal (no UAF) ---
    // Cleanup graph: alloc_root → alloc_left → access_both → free_left →
    //   write_root_left_ptr_null → access_root → free_root → return
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let alloc_root = cg.add_node(
        OperationKind::Acquire {
            resource: crid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_root",
    );
    let alloc_left = cg.add_node(
        OperationKind::Acquire {
            resource: crid(2),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_left",
    );
    let access_both = cg.add_node(
        OperationKind::Access { resource: crid(1) },
        "access_root",
    );
    let access_left = cg.add_node(
        OperationKind::Access { resource: crid(2) },
        "access_left_before_remove",
    );
    let free_left = cg.add_node(
        OperationKind::Release {
            resource: crid(2),
            kind: CleanupResourceKind::Memory,
        },
        "free_left",
    );
    let update_root = cg.add_node(
        OperationKind::Access { resource: crid(1) },
        "update_root_left_ptr",
    );
    let access_root_final = cg.add_node(
        OperationKind::Access { resource: crid(1) },
        "access_root_after_remove",
    );
    let free_root = cg.add_node(
        OperationKind::Release {
            resource: crid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_root",
    );
    let ret = cg.add_node(OperationKind::Return, "return");

    cg.add_edge(entry, alloc_root).unwrap();
    cg.add_edge(alloc_root, alloc_left).unwrap();
    cg.add_edge(alloc_left, access_both).unwrap();
    cg.add_edge(access_both, access_left).unwrap();
    cg.add_edge(access_left, free_left).unwrap();
    cg.add_edge(free_left, update_root).unwrap();
    cg.add_edge(update_root, access_root_final).unwrap();
    cg.add_edge(access_root_final, free_root).unwrap();
    cg.add_edge(free_root, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let report = verify_cleanup(&cg);
    assert!(
        report.clean,
        "Correct leaf removal should be clean, violations: {:?}",
        report.violations
    );

    // --- Exclusivity: update root's left_ptr after freeing left ---
    let mut excl_input = ExclusivityInput::new();
    // Write to root.left_ptr to null it out
    excl_input.add_access(btree_write(1, root_addr + LEFT_PTR_OFFSET, 8, "btree.rs:null_left_ptr", 1));
    // Read root value after removal
    excl_input.add_access(btree_read(2, root_addr, 8, "btree.rs:read_root_val", 1));
    // Sequential: write then read
    excl_input.add_sync_edge(SyncEdgeRecord::new(AccessId(1), AccessId(2), SyncOrdering::HappensBefore));

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "Sequential write then read on root should be Proven"
    );

    // --- Negative test: UAF after freeing leaf ---
    let mut cg_uaf = CleanupGraph::new();
    let alloc_root_uaf = cg_uaf.add_node(
        OperationKind::Acquire {
            resource: crid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_root",
    );
    let alloc_left_uaf = cg_uaf.add_node(
        OperationKind::Acquire {
            resource: crid(2),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_left",
    );
    let free_left_uaf = cg_uaf.add_node(
        OperationKind::Release {
            resource: crid(2),
            kind: CleanupResourceKind::Memory,
        },
        "free_left",
    );
    // Access left AFTER it's freed → UseAfterFree!
    let access_left_uaf = cg_uaf.add_node(
        OperationKind::Access { resource: crid(2) },
        "access_left_after_free",
    );
    let free_root_uaf = cg_uaf.add_node(
        OperationKind::Release {
            resource: crid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_root",
    );
    let ret_uaf = cg_uaf.add_node(OperationKind::Return, "return");

    cg_uaf.add_edge(alloc_root_uaf, alloc_left_uaf).unwrap();
    cg_uaf.add_edge(alloc_left_uaf, free_left_uaf).unwrap();
    cg_uaf.add_edge(free_left_uaf, access_left_uaf).unwrap();
    cg_uaf.add_edge(access_left_uaf, free_root_uaf).unwrap();
    cg_uaf.add_edge(free_root_uaf, ret_uaf).unwrap();
    cg_uaf.set_entry(alloc_root_uaf).unwrap();

    let uaf_report = verify_cleanup(&cg_uaf);
    assert!(!uaf_report.clean, "Access after free should not be clean");
    let has_uaf = uaf_report
        .violations
        .iter()
        .any(|v| v.kind == CleanupViolationKind::UseAfterFree);
    assert!(
        has_uaf,
        "Expected UseAfterFree violation, got: {:?}",
        uaf_report.violations
    );

    // --- Liveness: verify UAF is detected in liveness paths ---
    let mut liveness_input = LivenessInput::new();
    liveness_input.add_event(alloc_event(1, 1, 1));    // alloc root
    liveness_input.add_event(alloc_event(2, 2, 1));    // alloc left
    liveness_input.add_event(dealloc_event(2, 3, 1));  // free left
    liveness_input.add_event(read_event(2, 4, 1));     // read left after free → UAF
    liveness_input.add_event(dealloc_event(1, 5, 1));  // free root
    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5]);
    liveness_input.entry_point = Some(pid(1));

    let context = LivenessVerificationContext::new(liveness_input);
    let verifier = LivenessVerifier::new();
    let paths = verifier.compute_liveness_paths(&context);

    let left_path = paths.iter().find(|p| p.resource_id == 2).expect("left path");
    assert!(
        !left_path.access_after_free.is_empty(),
        "Left child should have access-after-free entries after removal"
    );
}

// ===========================================================================
// Test 5: test_btree_remove_internal — Remove internal node with children,
//          verify pointer reconnection
// ===========================================================================

/// Build a tree: root → left → left_left (3 levels). Remove the middle
/// node (left), reconnecting root.left to left_left. Verify:
/// - **Exclusivity**: Writes to root.left_ptr and left_left.parent_ptr
///   are sequential and non-overlapping.
/// - **Cleanup**: The removed node is freed, no UAF since reconnected
///   pointers are updated before further access.
/// - **Liveness**: No use-after-free for the removed node; remaining
///   nodes are properly connected.
#[test]
fn test_btree_remove_internal() {
    let root_addr: u64 = 0x1000;
    let left_addr: u64 = 0x2000;
    let left_left_addr: u64 = 0x3000;

    // --- Exclusivity: pointer reconnection writes ---
    let mut excl_input = ExclusivityInput::new();
    // Step 1: Write root.left_ptr = left_left (was left)
    excl_input.add_access(btree_write(1, root_addr + LEFT_PTR_OFFSET, 8, "btree.rs:reconnect_root_left", 1));
    // Step 2: Write left_left.parent_ptr = root (was left)
    excl_input.add_access(btree_write(2, left_left_addr + PARENT_PTR_OFFSET, 8, "btree.rs:reconnect_ll_parent", 3));
    // Step 3: Read the reconnected root.left_ptr
    excl_input.add_access(btree_read(3, root_addr + LEFT_PTR_OFFSET, 8, "btree.rs:verify_root_left", 1));
    // Step 4: Read the reconnected left_left.parent_ptr
    excl_input.add_access(btree_read(4, left_left_addr + PARENT_PTR_OFFSET, 8, "btree.rs:verify_ll_parent", 3));

    // Sequential: reconnect writes happen before verification reads
    excl_input.add_sync_edge(SyncEdgeRecord::new(AccessId(1), AccessId(2), SyncOrdering::HappensBefore));
    excl_input.add_sync_edge(SyncEdgeRecord::new(AccessId(2), AccessId(3), SyncOrdering::HappensBefore));
    excl_input.add_sync_edge(SyncEdgeRecord::new(AccessId(3), AccessId(4), SyncOrdering::HappensBefore));

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "Sequential pointer reconnection should be Proven, got: {:?}",
        excl_output.result.status
    );
    assert_eq!(excl_output.conflict_count(), 0);

    // --- Cleanup: correct internal node removal ---
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let alloc_root = cg.add_node(
        OperationKind::Acquire {
            resource: crid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_root",
    );
    let alloc_left = cg.add_node(
        OperationKind::Acquire {
            resource: crid(2),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_left",
    );
    let alloc_ll = cg.add_node(
        OperationKind::Acquire {
            resource: crid(3),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_left_left",
    );
    // Access all before removal
    let access_root = cg.add_node(OperationKind::Access { resource: crid(1) }, "access_root");
    let access_left = cg.add_node(OperationKind::Access { resource: crid(2) }, "access_left");
    let access_ll = cg.add_node(OperationKind::Access { resource: crid(3) }, "access_ll");
    // Free the internal node (left)
    let free_left = cg.add_node(
        OperationKind::Release {
            resource: crid(2),
            kind: CleanupResourceKind::Memory,
        },
        "free_left",
    );
    // Access remaining nodes after reconnection
    let access_root2 = cg.add_node(OperationKind::Access { resource: crid(1) }, "access_root_after");
    let access_ll2 = cg.add_node(OperationKind::Access { resource: crid(3) }, "access_ll_after");
    // Free remaining
    let free_ll = cg.add_node(
        OperationKind::Release {
            resource: crid(3),
            kind: CleanupResourceKind::Memory,
        },
        "free_ll",
    );
    let free_root = cg.add_node(
        OperationKind::Release {
            resource: crid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_root",
    );
    let ret = cg.add_node(OperationKind::Return, "return");

    cg.add_edge(entry, alloc_root).unwrap();
    cg.add_edge(alloc_root, alloc_left).unwrap();
    cg.add_edge(alloc_left, alloc_ll).unwrap();
    cg.add_edge(alloc_ll, access_root).unwrap();
    cg.add_edge(access_root, access_left).unwrap();
    cg.add_edge(access_left, access_ll).unwrap();
    cg.add_edge(access_ll, free_left).unwrap();
    cg.add_edge(free_left, access_root2).unwrap();
    cg.add_edge(access_root2, access_ll2).unwrap();
    cg.add_edge(access_ll2, free_ll).unwrap();
    cg.add_edge(free_ll, free_root).unwrap();
    cg.add_edge(free_root, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let report = verify_cleanup(&cg);
    assert!(
        report.clean,
        "Internal node removal with reconnection should be clean, violations: {:?}",
        report.violations
    );

    // --- Liveness: verify remaining nodes are properly tracked ---
    let mut liveness_input = LivenessInput::new();
    liveness_input.add_event(alloc_event(1, 1, 1));    // alloc root
    liveness_input.add_event(alloc_event(2, 2, 1));    // alloc left
    liveness_input.add_event(alloc_event(3, 3, 1));    // alloc left_left
    liveness_input.add_event(write_event(1, 4, 1));     // write root
    liveness_input.add_event(write_event(2, 5, 1));     // write left
    liveness_input.add_event(write_event(3, 6, 1));     // write left_left
    liveness_input.add_event(read_event(1, 7, 1));      // read root
    liveness_input.add_event(read_event(2, 8, 1));      // read left (last access before free)
    liveness_input.add_event(read_event(3, 9, 1));      // read left_left
    liveness_input.add_event(dealloc_event(2, 10, 1));  // free left
    liveness_input.add_event(read_event(1, 11, 1));     // read root (still alive)
    liveness_input.add_event(read_event(3, 12, 1));     // read left_left (still alive)
    liveness_input.add_event(dealloc_event(3, 13, 1));  // free left_left
    liveness_input.add_event(dealloc_event(1, 14, 1));  // free root
    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]);
    liveness_input.entry_point = Some(pid(1));

    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify(&liveness_input);
    assert!(
        result.invariant_holds,
        "Internal node removal should have no liveness violations, got: {:?}",
        result.violations
    );
}

// ===========================================================================
// Test 6: test_btree_dealloc_all — Post-order deallocation, verify cleanup
//          invariant
// ===========================================================================

/// Build a 7-node complete binary tree (3 levels), then deallocate in
/// post-order (left subtree, right subtree, root). Verify:
/// - **Cleanup**: All 7 resources properly freed — no leaks, no UAF,
///   no double-free.
/// - **Liveness**: All allocations paired with deallocations.
/// - The post-order traversal ensures children are freed before their
///   parent, which is the correct order for a tree.
#[test]
fn test_btree_dealloc_all() {
    // 7-node complete binary tree:
    //       R(1)
    //      /    \
    //    L(2)   R(3)
    //   / \     / \
    //  4   5   6   7

    // --- Cleanup: post-order dealloc of 7-node tree ---
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");
    let mut last = entry;

    // Phase 1: Acquire all 7 nodes
    let mut acquire_nodes = Vec::new();
    for i in 1..=7 {
        let alloc = cg.add_node(
            OperationKind::Acquire {
                resource: crid(i),
                kind: CleanupResourceKind::Memory,
            },
            format!("alloc_node{}", i),
        );
        let access = cg.add_node(
            OperationKind::Access { resource: crid(i) },
            format!("access_node{}", i),
        );
        cg.add_edge(last, alloc).unwrap();
        cg.add_edge(alloc, access).unwrap();
        last = access;
        acquire_nodes.push(alloc);
    }

    // Phase 2: Release all nodes in post-order: 4, 5, 2, 6, 7, 3, 1
    let post_order = [4u64, 5, 2, 6, 7, 3, 1];
    for &i in &post_order {
        let free = cg.add_node(
            OperationKind::Release {
                resource: crid(i),
                kind: CleanupResourceKind::Memory,
            },
            format!("free_node{}", i),
        );
        cg.add_edge(last, free).unwrap();
        last = free;
    }

    let ret = cg.add_node(OperationKind::Return, "return");
    cg.add_edge(last, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let report = verify_cleanup(&cg);
    assert!(
        report.clean,
        "Post-order dealloc of 7-node tree should be clean, violations: {:?}",
        report.violations
    );
    assert_eq!(report.acquires_checked, 7);
    assert!(report.violations.is_empty());

    // --- Liveness: all 7 resources allocated and deallocated ---
    let mut liveness_input = LivenessInput::new();
    // Allocate all nodes (PP1-7)
    for i in 1..=7 {
        liveness_input.add_event(alloc_event(i, i as u64, 1));
        liveness_input.add_event(write_event(i, i as u64 + 10, 1));
        liveness_input.add_event(read_event(i, i as u64 + 20, 1));
    }
    // Deallocate in post-order (PP31-37)
    for (idx, &i) in post_order.iter().enumerate() {
        liveness_input.add_event(dealloc_event(i, 31 + idx as u64, 1));
    }

    // Linear CFG covering all operations
    let mut all_points: Vec<u64> = (1..=7)           // allocs
        .chain(11..=17)                               // writes
        .chain(21..=27)                               // reads
        .chain(31..=37)                               // deallocs
        .collect();
    liveness_input.cfg_edges = linear_cfg(&all_points);
    liveness_input.entry_point = Some(pid(1));

    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify(&liveness_input);
    assert!(
        result.invariant_holds,
        "7-node tree with post-order dealloc should have no violations, got: {:?}",
        result.violations
    );
    assert_eq!(result.resources_checked, 7);

    // Verify all liveness paths have no access-after-free
    let context = LivenessVerificationContext::new(liveness_input);
    let paths = verifier.compute_liveness_paths(&context);
    for path in &paths {
        assert!(
            path.access_after_free.is_empty(),
            "Node {} should have no access-after-free, got: {:?}",
            path.resource_id,
            path.access_after_free
        );
    }
}

// ===========================================================================
// Test 7: test_btree_aliasing — Two pointers to same subtree through
//          different paths
// ===========================================================================

/// Model a scenario where two different pointers (e.g., a direct child
/// pointer and a cached pointer) point to the same subtree node. This
/// creates aliasing — two concurrent write accesses to the same memory
/// region should be detected as a conflict by the exclusivity verifier.
///
/// Verify:
/// - **Exclusivity**: Two concurrent writes to the same node via different
///   paths produce a WriteWrite conflict (if not synchronized).
/// - **Exclusivity**: If synchronized (happens-before), the same pattern
///   is Proven.
/// - **Exclusivity**: Two concurrent reads to the same node via different
///   paths are safe (reads never conflict).
#[test]
fn test_btree_aliasing() {
    let node_addr: u64 = 0x2000;
    let node_region: u64 = 2;

    // --- Scenario 1: Two concurrent writes via aliasing paths → Violated ---
    let mut excl_input = ExclusivityInput::new();
    // Path A: parent.left_ptr dereference → write to node value
    excl_input.add_access(btree_write(1, node_addr, 8, "btree.rs:alias_write_A", node_region));
    // Path B: cached_ptr dereference → write to same node value
    excl_input.add_access(btree_write(2, node_addr, 8, "btree.rs:alias_write_B", node_region));
    // No sync edge → concurrent writes

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_violated(),
        "Two concurrent writes to same node via aliasing paths should be Violated, got: {:?}",
        excl_output.result.status
    );
    assert_eq!(excl_output.write_write_count(), 1, "Expected 1 WriteWrite conflict");
    assert_eq!(excl_output.conflict_count(), 1);

    // --- Scenario 2: Same pattern but with happens-before → Proven ---
    let mut excl_input2 = ExclusivityInput::new();
    excl_input2.add_access(btree_write(3, node_addr, 8, "btree.rs:alias_write_A_sync", node_region));
    excl_input2.add_access(btree_write(4, node_addr, 8, "btree.rs:alias_write_B_sync", node_region));
    excl_input2.add_sync_edge(SyncEdgeRecord::new(AccessId(3), AccessId(4), SyncOrdering::HappensBefore));

    let excl_output2 = verify_exclusivity(&excl_input2);
    assert!(
        excl_output2.is_proven(),
        "Aliasing writes with happens-before should be Proven, got: {:?}",
        excl_output2.result.status
    );
    assert_eq!(excl_output2.conflict_count(), 0);

    // --- Scenario 3: Two concurrent reads via aliasing paths → Proven ---
    let mut excl_input3 = ExclusivityInput::new();
    excl_input3.add_access(btree_read(5, node_addr, 8, "btree.rs:alias_read_A", node_region));
    excl_input3.add_access(btree_read(6, node_addr, 8, "btree.rs:alias_read_B", node_region));

    let excl_output3 = verify_exclusivity(&excl_input3);
    assert!(
        excl_output3.is_proven(),
        "Two concurrent reads via aliasing paths should be Proven (reads don't conflict)"
    );
    assert_eq!(excl_output3.conflict_count(), 0);

    // --- Scenario 4: Write through one path, read through another → conflict ---
    let mut excl_input4 = ExclusivityInput::new();
    excl_input4.add_access(btree_write(7, node_addr, 8, "btree.rs:alias_write_C", node_region));
    excl_input4.add_access(btree_read(8, node_addr, 8, "btree.rs:alias_read_C", node_region));

    let excl_output4 = verify_exclusivity(&excl_input4);
    assert!(
        excl_output4.is_violated(),
        "Concurrent write+read via aliasing paths should be Violated"
    );
    assert_eq!(excl_output4.write_read_count(), 1, "Expected 1 WriteRead conflict");

    // --- Scenario 5: Alias with mutex protection → ProbablySafe ---
    let mut excl_input5 = ExclusivityInput::new();
    excl_input5.add_access(btree_write(9, node_addr, 8, "btree.rs:alias_mutex_A", node_region));
    excl_input5.add_access(btree_write(10, node_addr, 8, "btree.rs:alias_mutex_B", node_region));
    excl_input5.set_capability(AccessId(9), CapDInfo::write_locked(42));
    excl_input5.set_capability(AccessId(10), CapDInfo::write_locked(42));

    let excl_output5 = verify_exclusivity(&excl_input5);
    assert!(
        matches!(excl_output5.result.status, VerificationStatus::ProbablySafe { .. }),
        "Aliasing writes protected by same mutex should be ProbablySafe, got: {:?}",
        excl_output5.result.status
    );
    assert_eq!(excl_output5.conflict_count(), 1, "Conflict recorded but lock-protected");
}

// ===========================================================================
// Test 8: test_btree_full_lifecycle — Create, insert, traverse, remove, dealloc
// ===========================================================================

/// Full lifecycle test combining all operations on a binary tree:
/// 1. Create root node
/// 2. Insert left and right children
/// 3. Traverse in-order
/// 4. Remove left child (leaf)
/// 5. Deallocate remaining nodes (right, root)
///
/// Verify all three IVE invariants throughout the lifecycle:
/// - **Exclusivity**: All overlapping accesses are properly ordered.
/// - **Liveness**: No leaks, no use-after-free.
/// - **Cleanup**: Clean graph from start to finish.
#[test]
fn test_btree_full_lifecycle() {
    let root_addr: u64 = 0x1000;
    let left_addr: u64 = 0x2000;
    let right_addr: u64 = 0x3000;

    // === Phase 1: Exclusivity — full lifecycle access sequence ===
    let mut excl_input = ExclusivityInput::new();
    // Step 1: Create root (writes to root fields)
    excl_input.add_access(btree_write(1, root_addr, 8, "btree.rs:lc_root_val", 1));
    excl_input.add_access(btree_write(2, root_addr + LEFT_PTR_OFFSET, 8, "btree.rs:lc_root_left", 1));
    excl_input.add_access(btree_write(3, root_addr + RIGHT_PTR_OFFSET, 8, "btree.rs:lc_root_right", 1));
    // Step 2: Insert left child
    excl_input.add_access(btree_write(4, left_addr, 8, "btree.rs:lc_left_val", 2));
    excl_input.add_access(btree_write(5, left_addr + PARENT_PTR_OFFSET, 8, "btree.rs:lc_left_parent", 2));
    // Step 3: Insert right child
    excl_input.add_access(btree_write(6, right_addr, 8, "btree.rs:lc_right_val", 3));
    excl_input.add_access(btree_write(7, right_addr + PARENT_PTR_OFFSET, 8, "btree.rs:lc_right_parent", 3));
    // Step 4: In-order traversal (reads)
    excl_input.add_access(btree_read(8, left_addr, 8, "btree.rs:lc_trav_left", 2));
    excl_input.add_access(btree_read(9, root_addr, 8, "btree.rs:lc_trav_root", 1));
    excl_input.add_access(btree_read(10, right_addr, 8, "btree.rs:lc_trav_right", 3));
    // Step 5: Remove left — null out root.left_ptr
    excl_input.add_access(btree_write(11, root_addr + LEFT_PTR_OFFSET, 8, "btree.rs:lc_null_left", 1));
    // Step 6: Verify after removal
    excl_input.add_access(btree_read(12, root_addr, 8, "btree.rs:lc_verify_root", 1));
    excl_input.add_access(btree_read(13, root_addr + RIGHT_PTR_OFFSET, 8, "btree.rs:lc_verify_right", 1));

    // All operations are sequential
    for i in 1..13 {
        excl_input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(i), AccessId(i + 1), SyncOrdering::HappensBefore,
        ));
    }

    let excl_output = verify_exclusivity(&excl_input);
    assert!(
        excl_output.is_proven(),
        "Full lifecycle with sequential operations should be Proven, got: {:?}",
        excl_output.result.status
    );
    assert_eq!(excl_output.conflict_count(), 0);

    // === Phase 2: Liveness — full lifecycle events ===
    let mut liveness_input = LivenessInput::new();
    // Phase A: Create root
    liveness_input.add_event(alloc_event(1, 1, 1));   // PP1: alloc root
    liveness_input.add_event(write_event(1, 2, 1));    // PP2: write root
    // Phase B: Insert left
    liveness_input.add_event(alloc_event(2, 3, 1));   // PP3: alloc left
    liveness_input.add_event(write_event(2, 4, 1));    // PP4: write left
    liveness_input.add_event(write_event(1, 5, 1));    // PP5: update root.left_ptr
    // Phase C: Insert right
    liveness_input.add_event(alloc_event(3, 6, 1));   // PP6: alloc right
    liveness_input.add_event(write_event(3, 7, 1));    // PP7: write right
    liveness_input.add_event(write_event(1, 8, 1));    // PP8: update root.right_ptr
    // Phase D: Traverse
    liveness_input.add_event(read_event(2, 9, 1));     // PP9: traverse left
    liveness_input.add_event(read_event(1, 10, 1));    // PP10: traverse root
    liveness_input.add_event(read_event(3, 11, 1));    // PP11: traverse right
    // Phase E: Remove left
    liveness_input.add_event(write_event(1, 12, 1));   // PP12: null root.left_ptr
    liveness_input.add_event(dealloc_event(2, 13, 1)); // PP13: free left
    // Phase F: Verify remaining
    liveness_input.add_event(read_event(1, 14, 1));    // PP14: read root
    liveness_input.add_event(read_event(3, 15, 1));    // PP15: read right
    // Phase G: Dealloc remaining
    liveness_input.add_event(dealloc_event(3, 16, 1)); // PP16: free right
    liveness_input.add_event(dealloc_event(1, 17, 1)); // PP17: free root

    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17]);
    liveness_input.entry_point = Some(pid(1));

    let mut verifier = LivenessVerifier::new();
    let liveness_result = verifier.verify(&liveness_input);
    assert!(
        liveness_result.invariant_holds,
        "Full lifecycle should have no liveness violations, got: {:?}",
        liveness_result.violations
    );
    assert!(liveness_result.violations.is_empty());
    assert_eq!(liveness_result.resources_checked, 3);

    // Verify no use-after-free across the full lifecycle
    let context = LivenessVerificationContext::new(liveness_input);
    let paths = verifier.compute_liveness_paths(&context);
    for path in &paths {
        assert!(
            path.access_after_free.is_empty(),
            "Node {} should have no access-after-free in full lifecycle, got: {:?}",
            path.resource_id,
            path.access_after_free
        );
    }

    // === Phase 3: Cleanup — full lifecycle graph ===
    let mut cg = CleanupGraph::new();
    let entry = cg.add_node(OperationKind::Passthrough, "entry");

    // Acquire root
    let alloc_root = cg.add_node(
        OperationKind::Acquire {
            resource: crid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_root",
    );
    let access_root_init = cg.add_node(
        OperationKind::Access { resource: crid(1) },
        "init_root",
    );
    // Acquire left
    let alloc_left = cg.add_node(
        OperationKind::Acquire {
            resource: crid(2),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_left",
    );
    let access_left = cg.add_node(
        OperationKind::Access { resource: crid(2) },
        "init_left",
    );
    // Acquire right
    let alloc_right = cg.add_node(
        OperationKind::Acquire {
            resource: crid(3),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_right",
    );
    let access_right = cg.add_node(
        OperationKind::Access { resource: crid(3) },
        "init_right",
    );
    // Traverse (access all three)
    let traverse_root = cg.add_node(
        OperationKind::Access { resource: crid(1) },
        "traverse_root",
    );
    let traverse_left = cg.add_node(
        OperationKind::Access { resource: crid(2) },
        "traverse_left",
    );
    let traverse_right = cg.add_node(
        OperationKind::Access { resource: crid(3) },
        "traverse_right",
    );
    // Remove left (free + update root)
    let free_left = cg.add_node(
        OperationKind::Release {
            resource: crid(2),
            kind: CleanupResourceKind::Memory,
        },
        "free_left",
    );
    let update_root = cg.add_node(
        OperationKind::Access { resource: crid(1) },
        "update_root_after_remove",
    );
    // Dealloc remaining
    let free_right = cg.add_node(
        OperationKind::Release {
            resource: crid(3),
            kind: CleanupResourceKind::Memory,
        },
        "free_right",
    );
    let free_root = cg.add_node(
        OperationKind::Release {
            resource: crid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_root",
    );
    let ret = cg.add_node(OperationKind::Return, "return");

    // Wire edges: entry → alloc_root → init_root → alloc_left → init_left →
    //   alloc_right → init_right → traverse_root → traverse_left →
    //   traverse_right → free_left → update_root → free_right → free_root → return
    cg.add_edge(entry, alloc_root).unwrap();
    cg.add_edge(alloc_root, access_root_init).unwrap();
    cg.add_edge(access_root_init, alloc_left).unwrap();
    cg.add_edge(alloc_left, access_left).unwrap();
    cg.add_edge(access_left, alloc_right).unwrap();
    cg.add_edge(alloc_right, access_right).unwrap();
    cg.add_edge(access_right, traverse_root).unwrap();
    cg.add_edge(traverse_root, traverse_left).unwrap();
    cg.add_edge(traverse_left, traverse_right).unwrap();
    cg.add_edge(traverse_right, free_left).unwrap();
    cg.add_edge(free_left, update_root).unwrap();
    cg.add_edge(update_root, free_right).unwrap();
    cg.add_edge(free_right, free_root).unwrap();
    cg.add_edge(free_root, ret).unwrap();
    cg.set_entry(entry).unwrap();

    let cleanup_report = verify_cleanup(&cg);
    assert!(
        cleanup_report.clean,
        "Full lifecycle cleanup should be clean, violations: {:?}",
        cleanup_report.violations
    );
    assert_eq!(cleanup_report.acquires_checked, 3);
    assert!(cleanup_report.violations.is_empty());

    // === Phase 4: Verify cleanup result converts to Proven ===
    let verification_result = cleanup_report.to_verification_result();
    assert!(
        verification_result.is_proven(),
        "Full lifecycle should produce Proven verification result, got: {:?}",
        verification_result.status
    );
}
