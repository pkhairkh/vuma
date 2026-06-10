//! Verified Arena Allocator Tests (W2-A3)
//!
//! This module tests arena allocator patterns using VUMA's IVE verification.
//! Arena allocators are a special case for cleanup verification because they
//! intentionally "leak" individual blocks — all blocks are freed at once when
//! the arena is destroyed. The `LeakAnnotation` system with `LeakReason::Arena`
//! suppresses individual leak warnings, and full arena deallocation satisfies
//! the cleanup invariant.
//!
//! # Test Summary
//!
//! | # | Test                        | Key Verification                                        |
//! |---|-----------------------------|---------------------------------------------------------|
//! | 1 | test_arena_alloc            | Allocate from arena, verify liveness                    |
//! | 2 | test_arena_multiple_allocs  | Multiple allocations from same arena, all annotated     |
//! | 3 | test_arena_access_after_alloc | Access allocated memory, verify interpretation        |
//! | 4 | test_arena_no_individual_free | Arena doesn't free individual blocks (intentional)    |
//! | 5 | test_arena_dealloc_all      | Free entire arena at once, cleanup invariant satisfied  |
//! | 6 | test_arena_reuse            | Allocate, mark reusable, allocate again from same spot  |
//! | 7 | test_arena_aliasing         | Two pointers into same arena region                     |
//! | 8 | test_arena_full_lifecycle   | Create arena, alloc, access, dealloc all — full flow    |

use vuma_ive::{
    AnnotatedCleanupGraph, CleanupGraph, CleanupReport, CleanupResourceId,
    CleanupResourceKind, CleanupVerifier, LeakAnnotation, LeakReason, OperationKind,
};
use vuma_ive::cleanup::ViolationKind as CleanupViolationKind;
use vuma_ive::{
    EventAction, LivenessInput, LivenessVerificationContext,
    LivenessVerifier, PointId, ResourceEvent, ResourceId as LivenessResourceId,
    ResourceKind as LivenessResourceKind, ThreadId,
};
use vuma_ive::liveness::ControlFlowEdge;
use vuma_ive::{
    InterpretationVerifier, LocationId, ProgramPointId,
};
use vuma_ive::interpretation::{
    byte_repd, capd_with, empty_reld, make_bd,
};
use vuma_bd::capd::Capability;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Shorthand for a cleanup ResourceId.
fn rid(id: u64) -> CleanupResourceId {
    CleanupResourceId(id)
}


/// Verify a plain CleanupGraph and return the report.
fn verify(graph: &CleanupGraph) -> CleanupReport {
    CleanupVerifier::new().verify(graph)
}

/// Verify an annotated graph and return the report.
fn verify_annotated(annotated: &AnnotatedCleanupGraph) -> CleanupReport {
    CleanupVerifier::new().verify_annotated(annotated)
}

/// Shorthand for liveness PointId.
fn pp(id: u64) -> PointId {
    PointId(id)
}

/// Shorthand for liveness ResourceId.
fn lrid(id: u64) -> LivenessResourceId {
    LivenessResourceId(id)
}

/// Shorthand for liveness ThreadId.
fn tid(id: u64) -> ThreadId {
    ThreadId(id)
}

/// Create a memory Allocate event.
fn alloc_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: lrid(resource),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Allocate,
        point: pp(point),
        thread: tid(thread),
    }
}

/// Create a memory Deallocate event.
fn dealloc_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: lrid(resource),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Deallocate,
        point: pp(point),
        thread: tid(thread),
    }
}

/// Create a memory Read event.
fn read_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: lrid(resource),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Read,
        point: pp(point),
        thread: tid(thread),
    }
}

/// Create a memory Write event.
fn write_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: lrid(resource),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Write,
        point: pp(point),
        thread: tid(thread),
    }
}

/// Create a simple unconditional CFG edge.
fn cfg_edge(from: u64, to: u64) -> ControlFlowEdge {
    ControlFlowEdge {
        from: pp(from),
        to: pp(to),
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

/// Shorthand for interpretation LocationId.
fn loc(id: u64) -> LocationId {
    LocationId(id)
}

/// Shorthand for interpretation ProgramPointId.
fn ipp(id: u64) -> ProgramPointId {
    ProgramPointId(id)
}

/// Standard read-write capability set.
fn rw_capd() -> vuma_bd::capd::CapD {
    capd_with(&[Capability::Read, Capability::Write])
}

/// Build a LeakAnnotation with Arena reason and a reviewer.
fn arena_annotation(resource: u64, point: &str, reviewer: &str) -> LeakAnnotation {
    LeakAnnotation {
        resource: rid(resource),
        reason: LeakReason::Arena,
        annotation_point: point.to_string(),
        reviewer: Some(reviewer.to_string()),
    }
}

// ===========================================================================
// Test 1: test_arena_alloc — Allocate from arena, verify liveness
// ===========================================================================

#[test]
fn test_arena_alloc() {
    // Scenario: Create an arena, allocate a block from it, and access it.
    // Without LeakAnnotation, the arena block is a leak. With Arena annotation,
    // the leak is suppressed.
    //
    // CleanupGraph: arena_create → arena_alloc → access → return
    // (No individual free — the arena frees everything at once.)
    let mut g = CleanupGraph::new();
    let arena_create = g.add_node(
        OperationKind::Acquire {
            resource: rid(100),  // arena itself
            kind: CleanupResourceKind::Custom("arena".to_string()),
        },
        "arena_create",
    );
    let arena_alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),    // block allocated from arena
            kind: CleanupResourceKind::Memory,
        },
        "arena_alloc",
    );
    let access = g.add_node(
        OperationKind::Access {
            resource: rid(1),
        },
        "access_block",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(arena_create, arena_alloc).unwrap();
    g.add_edge(arena_alloc, access).unwrap();
    g.add_edge(access, ret).unwrap();
    g.set_entry(arena_create).unwrap();

    // --- Without annotation: should report leak for both resources ---
    let plain_report = verify(&g);
    assert!(
        !plain_report.clean,
        "Unannotated arena alloc should report leak violations"
    );
    let leak_resources: Vec<CleanupResourceId> = plain_report
        .violations
        .iter()
        .filter(|v| v.kind == CleanupViolationKind::Leak)
        .map(|v| v.resource)
        .collect();
    assert!(
        leak_resources.contains(&rid(1)),
        "Arena-allocated block should be reported as leaked without annotation"
    );
    assert!(
        leak_resources.contains(&rid(100)),
        "Arena itself should be reported as leaked without annotation"
    );

    // --- With Arena annotation: leaks are suppressed ---
    let mut annotated = AnnotatedCleanupGraph::new(g);
    annotated
        .add_leak_annotation(arena_annotation(1, "arena_alloc", "arena_reviewer"))
        .unwrap();
    annotated
        .add_leak_annotation(LeakAnnotation {
            resource: rid(100),
            reason: LeakReason::Arena,
            annotation_point: "arena_create".to_string(),
            reviewer: Some("arena_reviewer".to_string()),
        })
        .unwrap();

    let annotated_report = verify_annotated(&annotated);
    assert!(
        annotated_report.clean,
        "Arena-annotated resources should be clean, violations: {:?}",
        annotated_report.violations
    );
    assert_eq!(
        annotated_report.intentional_leaks.len(), 2,
        "Both arena resources should be recorded as intentional leaks"
    );
    assert!(
        annotated_report.intentional_leaks.iter().all(|a| a.reason == LeakReason::Arena),
        "All intentional leaks should have Arena reason"
    );
    assert!(
        annotated_report.unannotated_leaks.is_empty(),
        "No unannotated leaks expected"
    );

    // --- Liveness verification: the allocated block is live during access ---
    let mut liveness_input = LivenessInput::new();
    liveness_input.add_event(alloc_event(1, 1, 1));   // arena alloc
    liveness_input.add_event(write_event(1, 2, 1));    // write to block
    liveness_input.add_event(read_event(1, 3, 1));     // read from block
    // No dealloc — arena frees everything at once (not modeled in liveness here)
    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3]);
    liveness_input.entry_point = Some(pp(1));

    let context = LivenessVerificationContext::new(liveness_input);
    let liveness_verifier = LivenessVerifier::new();
    let paths = liveness_verifier.compute_liveness_paths(&context);

    // The block should have an allocation_point and no access_after_free
    // (since it's never individually freed)
    assert_eq!(paths.len(), 1, "Expected 1 liveness path");
    assert_eq!(paths[0].resource_id, 1);
    assert!(
        paths[0].access_after_free.is_empty(),
        "No use-after-free expected (block is never individually freed)"
    );
}

// ===========================================================================
// Test 2: test_arena_multiple_allocs — Multiple allocations from same arena
// ===========================================================================

#[test]
fn test_arena_multiple_allocs() {
    // Scenario: An arena with 3 allocated blocks. None are individually freed.
    // All are annotated with LeakReason::Arena.
    //
    // CleanupGraph: arena_create → alloc1 → access1 → alloc2 → access2 → alloc3 → access3 → return
    let mut g = CleanupGraph::new();
    let arena_create = g.add_node(
        OperationKind::Acquire {
            resource: rid(100),
            kind: CleanupResourceKind::Custom("arena".to_string()),
        },
        "arena_create",
    );

    let mut prev = arena_create;
    for i in 1..=3 {
        let alloc = g.add_node(
            OperationKind::Acquire {
                resource: rid(i),
                kind: CleanupResourceKind::Memory,
            },
            format!("arena_alloc{}", i),
        );
        let access = g.add_node(
            OperationKind::Access { resource: rid(i) },
            format!("access{}", i),
        );
        g.add_edge(prev, alloc).unwrap();
        g.add_edge(alloc, access).unwrap();
        prev = access;
    }

    let ret = g.add_node(OperationKind::Return, "return");
    g.add_edge(prev, ret).unwrap();
    g.set_entry(arena_create).unwrap();

    // Without annotation: 4 leaks (arena + 3 blocks)
    let plain_report = verify(&g);
    assert!(
        !plain_report.clean,
        "Unannotated arena with multiple allocs should report leaks"
    );
    let leak_count = plain_report
        .violations
        .iter()
        .filter(|v| v.kind == CleanupViolationKind::Leak)
        .count();
    assert_eq!(
        leak_count, 4,
        "Expected 4 leaks (arena + 3 blocks), got {}",
        leak_count
    );

    // With Arena annotation for all 4 resources: all suppressed
    let mut annotated = AnnotatedCleanupGraph::new(g);
    annotated
        .add_leak_annotation(LeakAnnotation {
            resource: rid(100),
            reason: LeakReason::Arena,
            annotation_point: "arena_create".to_string(),
            reviewer: Some("arena_team".to_string()),
        })
        .unwrap();
    for i in 1..=3 {
        annotated
            .add_leak_annotation(arena_annotation(i, &format!("arena_alloc{}", i), "arena_team"))
            .unwrap();
    }

    let annotated_report = verify_annotated(&annotated);
    assert!(
        annotated_report.clean,
        "All arena-annotated resources should be clean, violations: {:?}",
        annotated_report.violations
    );
    assert_eq!(
        annotated_report.intentional_leaks.len(), 4,
        "All 4 resources should be recorded as intentional leaks"
    );
    assert!(
        annotated_report.intentional_leaks.iter().all(|a| a.reason == LeakReason::Arena),
        "All intentional leaks should have Arena reason"
    );

    // Verify the verification result status
    let vr = annotated_report.to_verification_result();
    assert!(
        matches!(vr.status, vuma_ive::VerificationStatus::ProbablySafe { .. }),
        "Annotated arena leaks should result in ProbablySafe, got {:?}",
        vr.status
    );
}

// ===========================================================================
// Test 3: test_arena_access_after_alloc — Access allocated memory
// ===========================================================================

#[test]
fn test_arena_access_after_alloc() {
    // Scenario: Allocate from arena, write, then read. Verify:
    // 1. Cleanup: Arena annotation suppresses the leak
    // 2. Interpretation: Write then read with matching BDs is valid
    // 3. Liveness: Access occurs while the block is live
    let mut g = CleanupGraph::new();
    let arena_alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "arena_alloc",
    );
    let write_node = g.add_node(
        OperationKind::Access { resource: rid(1) },
        "write_block",
    );
    let read_node = g.add_node(
        OperationKind::Access { resource: rid(1) },
        "read_block",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(arena_alloc, write_node).unwrap();
    g.add_edge(write_node, read_node).unwrap();
    g.add_edge(read_node, ret).unwrap();
    g.set_entry(arena_alloc).unwrap();

    // --- Cleanup verification with annotation ---
    let mut annotated = AnnotatedCleanupGraph::new(g);
    annotated
        .add_leak_annotation(arena_annotation(1, "arena_alloc", "access_checker"))
        .unwrap();

    let report = verify_annotated(&annotated);
    assert!(
        report.clean,
        "Arena-annotated alloc with access should be clean, violations: {:?}",
        report.violations
    );
    assert_eq!(report.intentional_leaks.len(), 1);

    // --- Interpretation verification ---
    // Write and then read the same location with matching BDs → Proven
    let mut interp_verifier = InterpretationVerifier::new();
    let bd = make_bd(byte_repd(8, 8), rw_capd(), empty_reld());

    interp_verifier.record_write(loc(1), bd.clone(), ipp(1));
    interp_verifier.record_read(loc(1), bd.clone(), ipp(2));

    let interp_result = interp_verifier.verify();
    assert!(
        interp_result.is_proven(),
        "Write then read with matching BDs should be Proven, got {:?}",
        interp_result.status
    );

    // --- Liveness verification: access while block is live ---
    let mut liveness_input = LivenessInput::new();
    liveness_input.add_event(alloc_event(1, 1, 1));
    liveness_input.add_event(write_event(1, 2, 1));
    liveness_input.add_event(read_event(1, 3, 1));
    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3]);
    liveness_input.entry_point = Some(pp(1));

    let context = LivenessVerificationContext::new(liveness_input);
    let liveness_verifier = LivenessVerifier::new();
    let paths = liveness_verifier.compute_liveness_paths(&context);

    assert_eq!(paths.len(), 1);
    assert!(
        paths[0].access_after_free.is_empty(),
        "No use-after-free: access occurs while block is live (arena not yet destroyed)"
    );
}

// ===========================================================================
// Test 4: test_arena_no_individual_free — Arena doesn't free individual blocks
// ===========================================================================

#[test]
fn test_arena_no_individual_free() {
    // Scenario: Arena allocations intentionally lack individual frees.
    // This is the defining characteristic of arena allocators: they trade
    // individual deallocation for bulk deallocation.
    //
    // CleanupGraph: alloc1 → alloc2 → return (no individual Release nodes)
    let mut g = CleanupGraph::new();
    let alloc1 = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "arena_alloc_1",
    );
    let alloc2 = g.add_node(
        OperationKind::Acquire {
            resource: rid(2),
            kind: CleanupResourceKind::Memory,
        },
        "arena_alloc_2",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(alloc1, alloc2).unwrap();
    g.add_edge(alloc2, ret).unwrap();
    g.set_entry(alloc1).unwrap();

    // Verify there are NO Release nodes for the individual arena blocks
    assert!(
        g.release_nodes_for(rid(1)).is_empty(),
        "Arena block 1 should have no individual Release node"
    );
    assert!(
        g.release_nodes_for(rid(2)).is_empty(),
        "Arena block 2 should have no individual Release node"
    );

    // Without annotation: both are leaks
    let plain_report = verify(&g);
    assert!(!plain_report.clean);
    let leak_count = plain_report
        .violations
        .iter()
        .filter(|v| v.kind == CleanupViolationKind::Leak)
        .count();
    assert_eq!(leak_count, 2, "Both arena blocks should be leaked without annotation");

    // Quick reachability check: no release is reachable for either block
    let verifier = CleanupVerifier::new();
    let unreachable = verifier.quick_check_reachability(&g);
    assert_eq!(
        unreachable.len(), 2,
        "Both arena blocks should have unreachable releases"
    );
    let unreachable_resources: Vec<CleanupResourceId> =
        unreachable.iter().map(|(_, r)| *r).collect();
    assert!(
        unreachable_resources.contains(&rid(1)),
        "Block 1 should have unreachable release"
    );
    assert!(
        unreachable_resources.contains(&rid(2)),
        "Block 2 should have unreachable release"
    );

    // With Arena annotation: both leaks are suppressed (intentional)
    let mut annotated = AnnotatedCleanupGraph::new(g);
    annotated
        .add_leak_annotation(arena_annotation(1, "arena_alloc_1", "arena_auditor"))
        .unwrap();
    annotated
        .add_leak_annotation(arena_annotation(2, "arena_alloc_2", "arena_auditor"))
        .unwrap();

    let annotated_report = verify_annotated(&annotated);
    assert!(
        annotated_report.clean,
        "Arena-annotated intentional leaks should be clean, violations: {:?}",
        annotated_report.violations
    );
    assert_eq!(annotated_report.intentional_leaks.len(), 2);
    assert!(annotated_report.unannotated_leaks.is_empty());

    // The verification result should be ProbablySafe (not Proven) because
    // the leaks are annotated, meaning they rely on the assumption that
    // the arena will eventually be freed.
    let vr = annotated_report.to_verification_result();
    match &vr.status {
        vuma_ive::VerificationStatus::ProbablySafe { assumptions } => {
            assert_eq!(
                assumptions.len(), 2,
                "Should have 2 assumptions (one per annotated leak)"
            );
            assert!(
                assumptions.iter().all(|a| a.contains("intentionally leaked")),
                "All assumptions should mention intentional leak"
            );
        }
        other => panic!(
            "Expected ProbablySafe for annotated arena leaks, got {:?}",
            other
        ),
    }
}

// ===========================================================================
// Test 5: test_arena_dealloc_all — Free entire arena at once
// ===========================================================================

#[test]
fn test_arena_dealloc_all() {
    // Scenario: Arena allocates blocks 1 and 2, then dealloc_all releases
    // both blocks and the arena itself. This demonstrates that when the
    // arena DOES free everything at once, the cleanup invariant is fully
    // satisfied (Proven, not just ProbablySafe).
    //
    // CleanupGraph:
    //   arena_create → alloc1 → access1 → alloc2 → access2 → dealloc_all → return
    //
    // dealloc_all releases: arena (res100), block1 (res1), block2 (res2)
    let mut g = CleanupGraph::new();
    let arena_create = g.add_node(
        OperationKind::Acquire {
            resource: rid(100),
            kind: CleanupResourceKind::Custom("arena".to_string()),
        },
        "arena_create",
    );
    let alloc1 = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "arena_alloc_1",
    );
    let access1 = g.add_node(
        OperationKind::Access { resource: rid(1) },
        "access_1",
    );
    let alloc2 = g.add_node(
        OperationKind::Acquire {
            resource: rid(2),
            kind: CleanupResourceKind::Memory,
        },
        "arena_alloc_2",
    );
    let access2 = g.add_node(
        OperationKind::Access { resource: rid(2) },
        "access_2",
    );
    // dealloc_all releases all arena resources
    let dealloc_block1 = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "dealloc_block1",
    );
    let dealloc_block2 = g.add_node(
        OperationKind::Release {
            resource: rid(2),
            kind: CleanupResourceKind::Memory,
        },
        "dealloc_block2",
    );
    let dealloc_arena = g.add_node(
        OperationKind::Release {
            resource: rid(100),
            kind: CleanupResourceKind::Custom("arena".to_string()),
        },
        "dealloc_arena",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(arena_create, alloc1).unwrap();
    g.add_edge(alloc1, access1).unwrap();
    g.add_edge(access1, alloc2).unwrap();
    g.add_edge(alloc2, access2).unwrap();
    g.add_edge(access2, dealloc_block1).unwrap();
    g.add_edge(dealloc_block1, dealloc_block2).unwrap();
    g.add_edge(dealloc_block2, dealloc_arena).unwrap();
    g.add_edge(dealloc_arena, ret).unwrap();
    g.set_entry(arena_create).unwrap();

    // --- Without annotation: clean (everything is freed) ---
    let plain_report = verify(&g);
    assert!(
        plain_report.clean,
        "Arena with dealloc_all should be clean without annotations, violations: {:?}",
        plain_report.violations
    );
    assert!(plain_report.violations.is_empty());

    // --- With annotation: also clean, but now we check that the
    //     annotations are validated (annotated but actually freed → issue) ---
    let mut annotated = AnnotatedCleanupGraph::new(g);
    // Even though the blocks ARE freed, we annotate them as Arena to
    // test that validate_annotations flags AnnotatedButFreed.
    annotated
        .add_leak_annotation(arena_annotation(1, "arena_alloc_1", "dealloc_checker"))
        .unwrap();
    annotated
        .add_leak_annotation(arena_annotation(2, "arena_alloc_2", "dealloc_checker"))
        .unwrap();

    let annotated_report = verify_annotated(&annotated);
    // The report should still be clean — the resources ARE freed,
    // so no violations exist. The annotation just becomes redundant.
    assert!(
        annotated_report.clean,
        "Arena with dealloc_all should be clean even with annotations, violations: {:?}",
        annotated_report.violations
    );

    // Validate annotations: should flag AnnotatedButFreed for both blocks
    let issues = CleanupVerifier::new().validate_annotations(&annotated);
    let annotated_but_freed: Vec<_> = issues
        .iter()
        .filter(|i| matches!(i.issue, vuma_ive::AnnotationIssueKind::AnnotatedButFreed))
        .collect();
    assert_eq!(
        annotated_but_freed.len(), 2,
        "Both arena blocks should be flagged as AnnotatedButFreed (they are actually freed)"
    );

    // The cleanup invariant is fully satisfied → Proven status
    let vr = annotated_report.to_verification_result();
    assert!(
        matches!(vr.status, vuma_ive::VerificationStatus::Proven),
        "Full arena dealloc should result in Proven status, got {:?}",
        vr.status
    );
}

// ===========================================================================
// Test 6: test_arena_reuse — Allocate, mark reusable, allocate again
// ===========================================================================

#[test]
fn test_arena_reuse() {
    // Scenario: Arena allocators support "reset" or "reuse" where a block is
    // logically freed (marked reusable) and then the same memory region is
    // allocated again. In VUMA's cleanup model, each logical allocation gets
    // its own ResourceId, so reuse is modeled as:
    //   alloc(res1) → access → release(res1) → alloc(res2) → access → return
    //
    // This tests that:
    // - Release of res1 then Acquire of res2 is valid (no double-free)
    // - Access to res2 after res1 was released is valid (no use-after-free)
    // - The reuse pattern (arena reset + re-allocate) satisfies invariants
    let mut g = CleanupGraph::new();
    let arena_create = g.add_node(
        OperationKind::Acquire {
            resource: rid(100),
            kind: CleanupResourceKind::Custom("arena".to_string()),
        },
        "arena_create",
    );
    // First allocation from the arena
    let alloc1 = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "arena_alloc_first",
    );
    let access1 = g.add_node(
        OperationKind::Access { resource: rid(1) },
        "access_first",
    );
    // Arena reset: releases block 1 (marks the memory region as reusable)
    let release1 = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "arena_reset_release1",
    );
    // Re-allocate from the same arena region (new resource ID, same memory)
    let alloc2 = g.add_node(
        OperationKind::Acquire {
            resource: rid(2),  // new resource ID for the reused region
            kind: CleanupResourceKind::Memory,
        },
        "arena_alloc_reuse",
    );
    let access2 = g.add_node(
        OperationKind::Access { resource: rid(2) },
        "access_reused",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(arena_create, alloc1).unwrap();
    g.add_edge(alloc1, access1).unwrap();
    g.add_edge(access1, release1).unwrap();
    g.add_edge(release1, alloc2).unwrap();
    g.add_edge(alloc2, access2).unwrap();
    g.add_edge(access2, ret).unwrap();
    g.set_entry(arena_create).unwrap();

    let report = verify(&g);

    // res1 is properly freed, res2 is leaked (never released), arena is leaked
    let leak_count = report
        .violations
        .iter()
        .filter(|v| v.kind == CleanupViolationKind::Leak)
        .count();
    assert_eq!(
        leak_count, 2,
        "Expected 2 leaks (arena + res2), got {}",
        leak_count
    );

    // No double-free
    let has_double_free = report
        .violations
        .iter()
        .any(|v| v.kind == CleanupViolationKind::DoubleFree);
    assert!(
        !has_double_free,
        "No double-free expected (res1 released once, res2 never released)"
    );

    // No use-after-free: res2 is a new resource, access is valid
    let has_uaf = report
        .violations
        .iter()
        .any(|v| v.kind == CleanupViolationKind::UseAfterFree);
    assert!(
        !has_uaf,
        "No use-after-free expected (res2 is a new resource, access is valid)"
    );

    // With Arena annotation, the arena and res2 leaks are suppressed
    let mut annotated = AnnotatedCleanupGraph::new(g);
    annotated
        .add_leak_annotation(LeakAnnotation {
            resource: rid(100),
            reason: LeakReason::Arena,
            annotation_point: "arena_create".to_string(),
            reviewer: Some("reuse_auditor".to_string()),
        })
        .unwrap();
    annotated
        .add_leak_annotation(arena_annotation(2, "arena_alloc_reuse", "reuse_auditor"))
        .unwrap();

    let annotated_report = verify_annotated(&annotated);
    assert!(
        annotated_report.clean,
        "Arena-reused allocation with annotation should be clean, violations: {:?}",
        annotated_report.violations
    );
    assert_eq!(annotated_report.intentional_leaks.len(), 2);

    // --- Liveness: verify reuse pattern is valid ---
    let mut liveness_input = LivenessInput::new();
    // First lifecycle (res1)
    liveness_input.add_event(alloc_event(1, 1, 1));
    liveness_input.add_event(write_event(1, 2, 1));
    liveness_input.add_event(dealloc_event(1, 3, 1)); // arena reset releases res1
    // Second lifecycle (res2 = reuse of same memory region)
    liveness_input.add_event(alloc_event(2, 4, 1));   // re-allocate with new ID
    liveness_input.add_event(read_event(2, 5, 1));     // access reused region (safe)
    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5]);
    liveness_input.entry_point = Some(pp(1));

    let context = LivenessVerificationContext::new(liveness_input);
    let liveness_verifier = LivenessVerifier::new();
    let paths = liveness_verifier.compute_liveness_paths(&context);

    // There should be liveness paths for both resources
    assert!(
        paths.len() >= 2,
        "Expected at least 2 liveness paths for reused resources, got {}",
        paths.len()
    );

    // res1: properly freed → no access-after-free
    let res1_path = paths.iter().find(|p| p.resource_id == 1);
    if let Some(p) = res1_path {
        assert!(
            p.access_after_free.is_empty(),
            "res1 should have no access-after-free"
        );
    }

    // res2: never individually freed → no access-after-free (it's still live)
    let res2_path = paths.iter().find(|p| p.resource_id == 2);
    if let Some(p) = res2_path {
        assert!(
            p.access_after_free.is_empty(),
            "res2 should have no access-after-free (never freed individually)"
        );
    }
}

// ===========================================================================
// Test 7: test_arena_aliasing — Two pointers into same arena region
// ===========================================================================

#[test]
fn test_arena_aliasing() {
    // Scenario: Two pointers (aliases) point into the same arena region.
    // Both access the same resource. This is common in arena allocators
    // where multiple references point into the same arena block.
    //
    // CleanupGraph:
    //   arena_alloc → access_ptr1 → access_ptr2 → return
    // Both access nodes reference the same resource.
    let mut g = CleanupGraph::new();
    let arena_create = g.add_node(
        OperationKind::Acquire {
            resource: rid(100),
            kind: CleanupResourceKind::Custom("arena".to_string()),
        },
        "arena_create",
    );
    let arena_alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "arena_alloc",
    );
    let access_ptr1 = g.add_node(
        OperationKind::Access { resource: rid(1) },
        "access_via_ptr1",
    );
    let access_ptr2 = g.add_node(
        OperationKind::Access { resource: rid(1) },
        "access_via_ptr2",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(arena_create, arena_alloc).unwrap();
    g.add_edge(arena_alloc, access_ptr1).unwrap();
    g.add_edge(access_ptr1, access_ptr2).unwrap();
    g.add_edge(access_ptr2, ret).unwrap();
    g.set_entry(arena_create).unwrap();

    // Both accesses are before any release → no use-after-free
    let plain_report = verify(&g);
    let has_uaf = plain_report
        .violations
        .iter()
        .any(|v| v.kind == CleanupViolationKind::UseAfterFree);
    assert!(
        !has_uaf,
        "No use-after-free expected for aliased arena pointers (both access before free)"
    );

    // Leaks: arena + block, both annotated
    let mut annotated = AnnotatedCleanupGraph::new(g);
    annotated
        .add_leak_annotation(arena_annotation(1, "arena_alloc", "alias_auditor"))
        .unwrap();
    annotated
        .add_leak_annotation(LeakAnnotation {
            resource: rid(100),
            reason: LeakReason::Arena,
            annotation_point: "arena_create".to_string(),
            reviewer: Some("alias_auditor".to_string()),
        })
        .unwrap();

    let annotated_report = verify_annotated(&annotated);
    assert!(
        annotated_report.clean,
        "Aliased arena accesses with annotation should be clean, violations: {:?}",
        annotated_report.violations
    );

    // --- Interpretation: both pointers access with compatible BDs ---
    let mut interp_verifier = InterpretationVerifier::new();
    let write_bd = make_bd(byte_repd(8, 8), rw_capd(), empty_reld());
    let read_bd = make_bd(byte_repd(8, 8), rw_capd(), empty_reld());

    // Write via ptr1
    interp_verifier.record_write(loc(1), write_bd.clone(), ipp(1));
    // Read via ptr1
    interp_verifier.record_read(loc(1), read_bd.clone(), ipp(2));
    // Read via ptr2 (alias)
    interp_verifier.record_read(loc(1), read_bd.clone(), ipp(3));

    let interp_result = interp_verifier.verify();
    assert!(
        interp_result.is_proven(),
        "Aliased arena pointer accesses with matching BDs should be Proven, got {:?}",
        interp_result.status
    );

    // --- Liveness: both accesses are while the block is live ---
    let mut liveness_input = LivenessInput::new();
    liveness_input.add_event(alloc_event(1, 1, 1));
    liveness_input.add_event(write_event(1, 2, 1));    // write via ptr1
    liveness_input.add_event(read_event(1, 3, 1));     // read via ptr1
    liveness_input.add_event(read_event(1, 4, 1));     // read via ptr2 (alias)
    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);
    liveness_input.entry_point = Some(pp(1));

    let context = LivenessVerificationContext::new(liveness_input);
    let liveness_verifier = LivenessVerifier::new();
    let paths = liveness_verifier.compute_liveness_paths(&context);

    assert_eq!(paths.len(), 1);
    assert!(
        paths[0].access_after_free.is_empty(),
        "No use-after-free for aliased arena pointers"
    );
}

// ===========================================================================
// Test 8: test_arena_full_lifecycle — Create arena, alloc, access, dealloc all
// ===========================================================================

#[test]
fn test_arena_full_lifecycle() {
    // Scenario: Complete arena lifecycle:
    // 1. Create arena
    // 2. Allocate two blocks
    // 3. Access both blocks (write then read)
    // 4. Deallocate the entire arena (frees all blocks + arena)
    //
    // This verifies all three invariants simultaneously:
    // - Cleanup: No leaks, no double-free, no use-after-free
    // - Liveness: All accesses occur while resources are live
    // - Interpretation: Access patterns are valid
    let mut g = CleanupGraph::new();
    let arena_create = g.add_node(
        OperationKind::Acquire {
            resource: rid(100),
            kind: CleanupResourceKind::Custom("arena".to_string()),
        },
        "arena_create",
    );
    let alloc1 = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_block1",
    );
    let write1 = g.add_node(
        OperationKind::Access { resource: rid(1) },
        "write_block1",
    );
    let read1 = g.add_node(
        OperationKind::Access { resource: rid(1) },
        "read_block1",
    );
    let alloc2 = g.add_node(
        OperationKind::Acquire {
            resource: rid(2),
            kind: CleanupResourceKind::Memory,
        },
        "alloc_block2",
    );
    let write2 = g.add_node(
        OperationKind::Access { resource: rid(2) },
        "write_block2",
    );
    let read2 = g.add_node(
        OperationKind::Access { resource: rid(2) },
        "read_block2",
    );
    // Dealloc all: blocks first, then arena
    let free1 = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "arena_free_block1",
    );
    let free2 = g.add_node(
        OperationKind::Release {
            resource: rid(2),
            kind: CleanupResourceKind::Memory,
        },
        "arena_free_block2",
    );
    let free_arena = g.add_node(
        OperationKind::Release {
            resource: rid(100),
            kind: CleanupResourceKind::Custom("arena".to_string()),
        },
        "arena_destroy",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    // Wire up the full lifecycle
    g.add_edge(arena_create, alloc1).unwrap();
    g.add_edge(alloc1, write1).unwrap();
    g.add_edge(write1, read1).unwrap();
    g.add_edge(read1, alloc2).unwrap();
    g.add_edge(alloc2, write2).unwrap();
    g.add_edge(write2, read2).unwrap();
    g.add_edge(read2, free1).unwrap();
    g.add_edge(free1, free2).unwrap();
    g.add_edge(free2, free_arena).unwrap();
    g.add_edge(free_arena, ret).unwrap();
    g.set_entry(arena_create).unwrap();

    // --- Cleanup verification: everything properly freed → clean ---
    let cleanup_report = verify(&g);
    assert!(
        cleanup_report.clean,
        "Full arena lifecycle should be clean, violations: {:?}",
        cleanup_report.violations
    );
    assert!(cleanup_report.violations.is_empty());
    assert_eq!(cleanup_report.acquires_checked, 3); // arena + 2 blocks

    // Verify the result is Proven (no assumptions needed)
    let vr = cleanup_report.to_verification_result();
    assert!(
        matches!(vr.status, vuma_ive::VerificationStatus::Proven),
        "Full arena lifecycle should be Proven, got {:?}",
        vr.status
    );

    // No double-free
    let has_double_free = cleanup_report
        .violations
        .iter()
        .any(|v| v.kind == CleanupViolationKind::DoubleFree);
    assert!(!has_double_free, "No double-free in full lifecycle");

    // No use-after-free
    let has_uaf = cleanup_report
        .violations
        .iter()
        .any(|v| v.kind == CleanupViolationKind::UseAfterFree);
    assert!(!has_uaf, "No use-after-free in full lifecycle");

    // --- Liveness verification: all accesses while live ---
    let mut liveness_input = LivenessInput::new();
    // Block 1 lifecycle
    liveness_input.add_event(alloc_event(1, 1, 1));
    liveness_input.add_event(write_event(1, 2, 1));
    liveness_input.add_event(read_event(1, 3, 1));
    liveness_input.add_event(dealloc_event(1, 7, 1));
    // Block 2 lifecycle
    liveness_input.add_event(alloc_event(2, 4, 1));
    liveness_input.add_event(write_event(2, 5, 1));
    liveness_input.add_event(read_event(2, 6, 1));
    liveness_input.add_event(dealloc_event(2, 8, 1));
    // Linear CFG through all operations
    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5, 6, 7, 8]);
    liveness_input.entry_point = Some(pp(1));

    let mut liveness_verifier = LivenessVerifier::new();
    let liveness_result = liveness_verifier.verify(&liveness_input);

    assert!(
        liveness_result.invariant_holds,
        "Full arena lifecycle liveness should hold, violations: {:?}",
        liveness_result.violations
    );
    assert!(liveness_result.violations.is_empty());
    assert_eq!(liveness_result.resources_checked, 2); // 2 blocks tracked

    // Liveness paths: both blocks should be clean
    let context = LivenessVerificationContext::new(liveness_input);
    let liveness_verifier2 = LivenessVerifier::new();
    let paths = liveness_verifier2.compute_liveness_paths(&context);
    assert_eq!(paths.len(), 2, "Expected 2 liveness paths");

    for path in &paths {
        assert!(
            path.access_after_free.is_empty(),
            "Block {} should have no access-after-free in full lifecycle",
            path.resource_id
        );
        assert!(
            path.deallocation_point.is_some(),
            "Block {} should have a deallocation point",
            path.resource_id
        );
    }

    // --- Interpretation verification: write then read is valid ---
    let mut interp_verifier = InterpretationVerifier::new();
    let bd = make_bd(byte_repd(8, 8), rw_capd(), empty_reld());

    // Block 1: write then read
    interp_verifier.record_write(loc(1), bd.clone(), ipp(1));
    interp_verifier.record_read(loc(1), bd.clone(), ipp(2));

    // Block 2: write then read
    interp_verifier.record_write(loc(2), bd.clone(), ipp(3));
    interp_verifier.record_read(loc(2), bd.clone(), ipp(4));

    let interp_result = interp_verifier.verify();
    assert!(
        interp_result.is_proven(),
        "Full lifecycle interpretation should be Proven, got {:?}",
        interp_result.status
    );

    // --- Also test with LeakAnnotation (before dealloc): should still work ---
    // If we annotate blocks as Arena BEFORE they get freed by dealloc_all,
    // the cleanup still passes (they're freed), and validate_annotations
    // flags AnnotatedButFreed.
    let mut annotated = AnnotatedCleanupGraph::new(g.clone());
    annotated
        .add_leak_annotation(arena_annotation(1, "alloc_block1", "lifecycle_auditor"))
        .unwrap();
    annotated
        .add_leak_annotation(arena_annotation(2, "alloc_block2", "lifecycle_auditor"))
        .unwrap();

    let annotated_report = verify_annotated(&annotated);
    assert!(
        annotated_report.clean,
        "Full lifecycle with Arena annotations should still be clean"
    );

    // The annotations are redundant since resources ARE freed
    let issues = CleanupVerifier::new().validate_annotations(&annotated);
    let annotated_but_freed_count = issues
        .iter()
        .filter(|i| matches!(i.issue, vuma_ive::AnnotationIssueKind::AnnotatedButFreed))
        .count();
    assert_eq!(
        annotated_but_freed_count, 2,
        "Both blocks should be flagged as AnnotatedButFreed in full lifecycle"
    );
}
