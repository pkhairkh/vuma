//! Integration tests for the CleanupVerifier (IVE module).
//!
//! Comprehensive test suite covering:
//! - Basic cleanup (alloc/free, leak, double-free, use-after-free, multiple resources)
//! - Conditional paths (if/else branches, error paths, nested conditionals, early return)
//! - Leak annotations (arena, global cache, singleton, annotated-but-freed, mixed)
//! - Complex scenarios (lock resource, file handle, reachability, cyclic graph, large graph)

use vuma_ive::{
    AnnotatedCleanupGraph, AnnotationIssueKind, CleanupGraph, CleanupNodeId, CleanupResourceId,
    CleanupResourceKind, CleanupReport, CleanupVerifier, LeakAnnotation, LeakReason, OperationKind,
};
use vuma_ive::cleanup::ViolationKind as CleanupViolationKind;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Shorthand for a ResourceId.
fn rid(id: u64) -> CleanupResourceId {
    CleanupResourceId(id)
}

/// Shorthand for a NodeId.
fn nid(id: u64) -> CleanupNodeId {
    CleanupNodeId(id)
}

/// Verify a plain CleanupGraph and return the report.
fn verify(graph: &CleanupGraph) -> CleanupReport {
    CleanupVerifier::new().verify(graph)
}

/// Verify an annotated graph and return the report.
fn verify_annotated(annotated: &AnnotatedCleanupGraph) -> CleanupReport {
    CleanupVerifier::new().verify_annotated(annotated)
}

/// Validate annotations and return the issues.
fn validate_annotations(annotated: &AnnotatedCleanupGraph) -> Vec<vuma_ive::AnnotationIssue> {
    CleanupVerifier::new().validate_annotations(annotated)
}

/// Build a simple linear graph: entry → acquire → access → release → return.
/// Returns the graph and the node IDs in order.
fn build_simple_alloc_free() -> (CleanupGraph, [CleanupNodeId; 5]) {
    let mut g = CleanupGraph::new();
    let entry = g.add_node(OperationKind::Passthrough, "entry");
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let access = g.add_node(
        OperationKind::Access {
            resource: rid(1),
        },
        "access",
    );
    let free = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(entry, alloc).unwrap();
    g.add_edge(alloc, access).unwrap();
    g.add_edge(access, free).unwrap();
    g.add_edge(free, ret).unwrap();
    g.set_entry(entry).unwrap();

    (g, [entry, alloc, access, free, ret])
}

// ===========================================================================
// Category 1: Basic Cleanup (5 tests)
// ===========================================================================

#[test]
fn test_simple_alloc_free() {
    // alloc → access → free → return → clean
    let (graph, _) = build_simple_alloc_free();
    let report = verify(&graph);

    assert!(report.clean, "Simple alloc→access→free→return should be clean");
    assert!(report.violations.is_empty(), "No violations expected");
    assert_eq!(report.acquires_checked, 1);
}

#[test]
fn test_memory_leak() {
    // alloc → return (no free) → Leak violation
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(alloc, ret).unwrap();
    g.set_entry(alloc).unwrap();

    let report = verify(&g);

    assert!(!report.clean, "Alloc without free should not be clean");
    assert_eq!(report.violations.len(), 1, "Expected exactly one violation");

    let v = &report.violations[0];
    assert_eq!(v.kind, CleanupViolationKind::Leak);
    assert_eq!(v.resource, rid(1));
}

#[test]
fn test_double_free() {
    // alloc → free → free → DoubleFree violation
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let free1 = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free1",
    );
    let free2 = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free2",
    );

    g.add_edge(alloc, free1).unwrap();
    g.add_edge(free1, free2).unwrap();
    g.set_entry(alloc).unwrap();

    let report = verify(&g);

    assert!(!report.clean, "Double free should not be clean");
    let has_double_free = report
        .violations
        .iter()
        .any(|v| v.kind == CleanupViolationKind::DoubleFree);
    assert!(
        has_double_free,
        "Expected a DoubleFree violation, got: {:?}",
        report.violations
    );
}

#[test]
fn test_use_after_free() {
    // alloc → free → access → UseAfterFree violation
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let free = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free",
    );
    let access = g.add_node(
        OperationKind::Access {
            resource: rid(1),
        },
        "access",
    );

    g.add_edge(alloc, free).unwrap();
    g.add_edge(free, access).unwrap();
    g.set_entry(alloc).unwrap();

    let report = verify(&g);

    assert!(!report.clean, "Use after free should not be clean");
    let has_uaf = report
        .violations
        .iter()
        .any(|v| v.kind == CleanupViolationKind::UseAfterFree);
    assert!(
        has_uaf,
        "Expected a UseAfterFree violation, got: {:?}",
        report.violations
    );
}

#[test]
fn test_multiple_resources() {
    // 3 resources, all properly freed → clean
    let mut g = CleanupGraph::new();
    let entry = g.add_node(OperationKind::Passthrough, "entry");

    let mut allocs = Vec::new();
    let mut frees = Vec::new();

    for i in 1..=3 {
        let alloc = g.add_node(
            OperationKind::Acquire {
                resource: rid(i),
                kind: CleanupResourceKind::Memory,
            },
            format!("alloc{}", i),
        );
        let free = g.add_node(
            OperationKind::Release {
                resource: rid(i),
                kind: CleanupResourceKind::Memory,
            },
            format!("free{}", i),
        );
        allocs.push(alloc);
        frees.push(free);
    }

    let ret = g.add_node(OperationKind::Return, "return");

    // Chain: entry → alloc1 → free1 → alloc2 → free2 → alloc3 → free3 → return
    g.add_edge(entry, allocs[0]).unwrap();
    g.add_edge(allocs[0], frees[0]).unwrap();
    g.add_edge(frees[0], allocs[1]).unwrap();
    g.add_edge(allocs[1], frees[1]).unwrap();
    g.add_edge(frees[1], allocs[2]).unwrap();
    g.add_edge(allocs[2], frees[2]).unwrap();
    g.add_edge(frees[2], ret).unwrap();
    g.set_entry(entry).unwrap();

    let report = verify(&g);

    assert!(report.clean, "All 3 resources properly freed should be clean");
    assert!(report.violations.is_empty());
    assert_eq!(report.acquires_checked, 3);
}

// ===========================================================================
// Category 2: Conditional Paths (5 tests)
// ===========================================================================

#[test]
fn test_both_branches_free() {
    // if/else both free the resource → clean
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let branch = g.add_node(
        OperationKind::Branch {
            condition: "flag".to_string(),
        },
        "branch",
    );
    let free_if = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_if",
    );
    let free_else = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_else",
    );
    let join = g.add_node(OperationKind::Join, "join");
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(alloc, branch).unwrap();
    g.add_edge(branch, free_if).unwrap();
    g.add_edge(branch, free_else).unwrap();
    g.add_edge(free_if, join).unwrap();
    g.add_edge(free_else, join).unwrap();
    g.add_edge(join, ret).unwrap();
    g.set_entry(alloc).unwrap();

    let report = verify(&g);

    assert!(
        report.clean,
        "Both branches freeing resource should be clean, violations: {:?}",
        report.violations
    );
}

#[test]
fn test_one_branch_leaks() {
    // if frees, else doesn't → Leak on else path
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let branch = g.add_node(
        OperationKind::Branch {
            condition: "flag".to_string(),
        },
        "branch",
    );
    let free_if = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_if",
    );
    let ret_else = g.add_node(OperationKind::Return, "ret_else");
    let join = g.add_node(OperationKind::Join, "join");
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(alloc, branch).unwrap();
    g.add_edge(branch, free_if).unwrap();   // if-branch: frees
    g.add_edge(branch, ret_else).unwrap();  // else-branch: leaks (returns without free)
    g.add_edge(free_if, join).unwrap();
    g.add_edge(ret_else, join).unwrap();
    g.add_edge(join, ret).unwrap();
    g.set_entry(alloc).unwrap();

    let report = verify(&g);

    assert!(!report.clean, "One branch leaking should not be clean");
    let has_leak = report
        .violations
        .iter()
        .any(|v| v.kind == CleanupViolationKind::Leak);
    assert!(has_leak, "Expected a Leak violation on the else path");
}

#[test]
fn test_error_path_cleanup() {
    // error path also frees → clean
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let branch = g.add_node(
        OperationKind::Branch {
            condition: "ok".to_string(),
        },
        "branch",
    );
    let free_ok = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_ok",
    );
    let free_err = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_err",
    );
    let ret_ok = g.add_node(OperationKind::Return, "ret_ok");
    let ret_err = g.add_node(
        OperationKind::ErrorReturn {
            description: "error".to_string(),
        },
        "ret_err",
    );

    g.add_edge(alloc, branch).unwrap();
    g.add_edge(branch, free_ok).unwrap();
    g.add_edge(branch, free_err).unwrap();
    g.add_edge(free_ok, ret_ok).unwrap();
    g.add_edge(free_err, ret_err).unwrap();
    g.set_entry(alloc).unwrap();

    let report = verify(&g);

    assert!(
        report.clean,
        "Both ok and error paths freeing should be clean, violations: {:?}",
        report.violations
    );
}

#[test]
fn test_nested_conditionals() {
    // nested if/else/else if, all paths free → clean
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let branch1 = g.add_node(
        OperationKind::Branch {
            condition: "a".to_string(),
        },
        "branch_a",
    );
    let free_a = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_a",
    );
    let branch2 = g.add_node(
        OperationKind::Branch {
            condition: "b".to_string(),
        },
        "branch_b",
    );
    let free_b1 = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_b1",
    );
    let free_b2 = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_b2",
    );
    let join = g.add_node(OperationKind::Join, "join");
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(alloc, branch1).unwrap();
    // If a is true → free_a
    g.add_edge(branch1, free_a).unwrap();
    // If a is false → branch on b
    g.add_edge(branch1, branch2).unwrap();
    // If b is true → free_b1
    g.add_edge(branch2, free_b1).unwrap();
    // If b is false → free_b2
    g.add_edge(branch2, free_b2).unwrap();
    // All paths converge at join → return
    g.add_edge(free_a, join).unwrap();
    g.add_edge(free_b1, join).unwrap();
    g.add_edge(free_b2, join).unwrap();
    g.add_edge(join, ret).unwrap();
    g.set_entry(alloc).unwrap();

    let report = verify(&g);

    assert!(
        report.clean,
        "All nested conditional paths freeing should be clean, violations: {:?}",
        report.violations
    );
}

#[test]
fn test_early_return_leak() {
    // early return without freeing → Leak
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let branch = g.add_node(
        OperationKind::Branch {
            condition: "early".to_string(),
        },
        "branch",
    );
    let early_ret = g.add_node(OperationKind::Return, "early_return");
    let free = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(alloc, branch).unwrap();
    g.add_edge(branch, early_ret).unwrap(); // early return — leak!
    g.add_edge(branch, free).unwrap();      // normal path — frees
    g.add_edge(free, ret).unwrap();
    g.set_entry(alloc).unwrap();

    let report = verify(&g);

    assert!(!report.clean, "Early return without freeing should not be clean");
    let has_leak = report
        .violations
        .iter()
        .any(|v| v.kind == CleanupViolationKind::Leak);
    assert!(has_leak, "Expected a Leak violation on the early return path");
}

// ===========================================================================
// Category 3: Leak Annotations (5 tests)
// ===========================================================================

#[test]
fn test_arena_annotation() {
    // Arena annotation suppresses leak warning
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "arena_alloc",
    );
    let ret = g.add_node(OperationKind::Return, "return");
    g.add_edge(alloc, ret).unwrap();
    g.set_entry(alloc).unwrap();

    let mut annotated = AnnotatedCleanupGraph::new(g);
    annotated
        .add_leak_annotation(LeakAnnotation {
            resource: rid(1),
            reason: LeakReason::Arena,
            annotation_point: "arena_alloc".to_string(),
            reviewer: Some("alice".to_string()),
        })
        .unwrap();

    let report = verify_annotated(&annotated);

    assert!(
        report.clean,
        "Arena-annotated leak should be clean, violations: {:?}",
        report.violations
    );
    assert_eq!(report.intentional_leaks.len(), 1);
    assert_eq!(report.intentional_leaks[0].reason, LeakReason::Arena);
    assert!(report.unannotated_leaks.is_empty());
}

#[test]
fn test_global_cache_annotation() {
    // GlobalCache annotation
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(10),
            kind: CleanupResourceKind::Memory,
        },
        "cache_alloc",
    );
    let ret = g.add_node(OperationKind::Return, "return");
    g.add_edge(alloc, ret).unwrap();
    g.set_entry(alloc).unwrap();

    let mut annotated = AnnotatedCleanupGraph::new(g);
    annotated
        .add_leak_annotation(LeakAnnotation {
            resource: rid(10),
            reason: LeakReason::GlobalCache,
            annotation_point: "cache_alloc".to_string(),
            reviewer: Some("bob".to_string()),
        })
        .unwrap();

    let report = verify_annotated(&annotated);

    assert!(
        report.clean,
        "GlobalCache-annotated leak should be clean, violations: {:?}",
        report.violations
    );
    assert_eq!(report.intentional_leaks.len(), 1);
    assert_eq!(report.intentional_leaks[0].reason, LeakReason::GlobalCache);
}

#[test]
fn test_singleton_annotation() {
    // Singleton annotation
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(42),
            kind: CleanupResourceKind::Memory,
        },
        "singleton_alloc",
    );
    let ret = g.add_node(OperationKind::Return, "return");
    g.add_edge(alloc, ret).unwrap();
    g.set_entry(alloc).unwrap();

    let mut annotated = AnnotatedCleanupGraph::new(g);
    annotated
        .add_leak_annotation(LeakAnnotation {
            resource: rid(42),
            reason: LeakReason::Singleton,
            annotation_point: "singleton_alloc".to_string(),
            reviewer: Some("carol".to_string()),
        })
        .unwrap();

    let report = verify_annotated(&annotated);

    assert!(
        report.clean,
        "Singleton-annotated leak should be clean, violations: {:?}",
        report.violations
    );
    assert_eq!(report.intentional_leaks.len(), 1);
    assert_eq!(report.intentional_leaks[0].reason, LeakReason::Singleton);
}

#[test]
fn test_annotation_validation_annotated_but_freed() {
    // Resource annotated as leak but actually freed → AnnotationIssue::AnnotatedButFreed
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let free = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free",
    );
    let ret = g.add_node(OperationKind::Return, "return");
    g.add_edge(alloc, free).unwrap();
    g.add_edge(free, ret).unwrap();
    g.set_entry(alloc).unwrap();

    let mut annotated = AnnotatedCleanupGraph::new(g);
    annotated
        .add_leak_annotation(LeakAnnotation {
            resource: rid(1),
            reason: LeakReason::Arena,
            annotation_point: "alloc".to_string(),
            reviewer: Some("dave".to_string()),
        })
        .unwrap();

    // verify_annotated: the resource is actually freed, so no leak violation
    // is produced. The report should be clean because there are no violations.
    let report = verify_annotated(&annotated);
    assert!(report.clean, "Freed resource should produce no violations");

    // validate_annotations: should flag AnnotatedButFreed
    let issues = validate_annotations(&annotated);
    assert!(
        issues
            .iter()
            .any(|i| matches!(i.issue, AnnotationIssueKind::AnnotatedButFreed)),
        "Expected AnnotatedButFreed issue, got: {:?}",
        issues
    );
}

#[test]
fn test_mixed_annotated_unannotated() {
    // Some resources annotated, some not
    let mut g = CleanupGraph::new();
    // Resource 1: annotated leak (arena)
    let alloc1 = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc1",
    );
    // Resource 2: unannotated leak
    let alloc2 = g.add_node(
        OperationKind::Acquire {
            resource: rid(2),
            kind: CleanupResourceKind::Memory,
        },
        "alloc2",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(alloc1, alloc2).unwrap();
    g.add_edge(alloc2, ret).unwrap();
    g.set_entry(alloc1).unwrap();

    let mut annotated = AnnotatedCleanupGraph::new(g);
    annotated
        .add_leak_annotation(LeakAnnotation {
            resource: rid(1),
            reason: LeakReason::Arena,
            annotation_point: "alloc1".to_string(),
            reviewer: Some("eve".to_string()),
        })
        .unwrap();

    let report = verify_annotated(&annotated);

    assert!(!report.clean, "Unannotated resource 2 should make report not clean");
    assert_eq!(report.intentional_leaks.len(), 1, "Exactly 1 intentional leak (res1)");
    assert_eq!(report.unannotated_leaks.len(), 1, "Exactly 1 unannotated leak (res2)");

    let leak_resource = report.unannotated_leaks[0].resource;
    assert_eq!(leak_resource, rid(2), "Unannotated leak should be for resource 2");
}

// ===========================================================================
// Category 4: Complex Scenarios (5 tests)
// ===========================================================================

#[test]
fn test_lock_resource() {
    // Lock acquire/release (not memory) — clean
    let mut g = CleanupGraph::new();
    let lock = g.add_node(
        OperationKind::Acquire {
            resource: rid(100),
            kind: CleanupResourceKind::Lock,
        },
        "lock_acquire",
    );
    let unlock = g.add_node(
        OperationKind::Release {
            resource: rid(100),
            kind: CleanupResourceKind::Lock,
        },
        "lock_release",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(lock, unlock).unwrap();
    g.add_edge(unlock, ret).unwrap();
    g.set_entry(lock).unwrap();

    let report = verify(&g);

    assert!(report.clean, "Lock acquire/release should be clean");
    assert!(report.violations.is_empty());
}

#[test]
fn test_file_handle() {
    // File open/close tracking — clean
    let mut g = CleanupGraph::new();
    let open = g.add_node(
        OperationKind::Acquire {
            resource: rid(200),
            kind: CleanupResourceKind::FileHandle,
        },
        "file_open",
    );
    let access = g.add_node(
        OperationKind::Access {
            resource: rid(200),
        },
        "file_read",
    );
    let close = g.add_node(
        OperationKind::Release {
            resource: rid(200),
            kind: CleanupResourceKind::FileHandle,
        },
        "file_close",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(open, access).unwrap();
    g.add_edge(access, close).unwrap();
    g.add_edge(close, ret).unwrap();
    g.set_entry(open).unwrap();

    let report = verify(&g);

    assert!(report.clean, "File open/read/close should be clean");
    assert!(report.violations.is_empty());
}

#[test]
fn test_reachability_check() {
    // Quick reachability check: acquire with reachable release → no issue
    // acquire with unreachable release → flagged
    let mut g = CleanupGraph::new();
    // Resource 1: acquire → release (reachable)
    let alloc1 = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc1",
    );
    let free1 = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free1",
    );

    // Resource 2: acquire → no reachable release
    let alloc2 = g.add_node(
        OperationKind::Acquire {
            resource: rid(2),
            kind: CleanupResourceKind::Memory,
        },
        "alloc2",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    // Path: alloc1 → free1 → alloc2 → return (alloc2 has no release reachable)
    g.add_edge(alloc1, free1).unwrap();
    g.add_edge(free1, alloc2).unwrap();
    g.add_edge(alloc2, ret).unwrap();
    g.set_entry(alloc1).unwrap();

    let verifier = CleanupVerifier::new();
    let unreachable = verifier.quick_check_reachability(&g);

    // Only resource 2 should be unreachable
    assert_eq!(unreachable.len(), 1, "Expected exactly 1 unreachable acquire");
    assert_eq!(unreachable[0].1, rid(2), "Resource 2 should be unreachable");
}

#[test]
fn test_cyclic_graph() {
    // Cyclic control flow graph (loop) with proper cleanup → clean
    let mut g = CleanupGraph::new();
    let alloc = g.add_node(
        OperationKind::Acquire {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let branch = g.add_node(
        OperationKind::Branch {
            condition: "loop_cond".to_string(),
        },
        "loop_cond",
    );
    let access = g.add_node(
        OperationKind::Access {
            resource: rid(1),
        },
        "access",
    );
    let free = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    // alloc → branch → access → branch (loop back)
    // branch → free → return (exit)
    g.add_edge(alloc, branch).unwrap();
    g.add_edge(branch, access).unwrap();   // continue loop
    g.add_edge(access, branch).unwrap();   // loop back
    g.add_edge(branch, free).unwrap();     // exit loop
    g.add_edge(free, ret).unwrap();
    g.set_entry(alloc).unwrap();

    let report = verify(&g);

    // The cycle detection prevents infinite traversal. Once we exit the loop,
    // the resource is freed. This should be clean.
    assert!(
        report.clean,
        "Cyclic graph with proper cleanup should be clean, violations: {:?}",
        report.violations
    );
}

#[test]
fn test_large_graph() {
    // 50 nodes, many paths — all resources properly freed → clean
    let mut g = CleanupGraph::new();
    let verifier = CleanupVerifier::new().with_max_path_length(512);

    let entry = g.add_node(OperationKind::Passthrough, "entry");
    let mut current = entry;

    // Create a chain of 10 resources, each with a conditional branch where
    // both paths free the resource, then join.
    for i in 1..=10 {
        let alloc = g.add_node(
            OperationKind::Acquire {
                resource: rid(i),
                kind: CleanupResourceKind::Memory,
            },
            format!("alloc{}", i),
        );
        let branch = g.add_node(
            OperationKind::Branch {
                condition: format!("cond{}", i),
            },
            format!("branch{}", i),
        );
        let free_t = g.add_node(
            OperationKind::Release {
                resource: rid(i),
                kind: CleanupResourceKind::Memory,
            },
            format!("free{}_true", i),
        );
        let free_f = g.add_node(
            OperationKind::Release {
                resource: rid(i),
                kind: CleanupResourceKind::Memory,
            },
            format!("free{}_false", i),
        );
        let join = g.add_node(OperationKind::Join, format!("join{}", i));

        g.add_edge(current, alloc).unwrap();
        g.add_edge(alloc, branch).unwrap();
        g.add_edge(branch, free_t).unwrap();
        g.add_edge(branch, free_f).unwrap();
        g.add_edge(free_t, join).unwrap();
        g.add_edge(free_f, join).unwrap();

        current = join;
    }

    let ret = g.add_node(OperationKind::Return, "return");
    g.add_edge(current, ret).unwrap();
    g.set_entry(entry).unwrap();

    // Graph should have 10*(1+1+2+1) + 2 = 52 nodes
    assert!(g.node_count() >= 50, "Graph should have at least 50 nodes, got {}", g.node_count());

    let report = verifier.verify(&g);

    assert!(
        report.clean,
        "Large graph with all resources freed on all paths should be clean, violations: {:?}",
        report.violations
    );
    assert_eq!(report.acquires_checked, 10);
    // 2^10 = 1024 paths in theory, but cycle detection and dedup may reduce this
    assert!(report.paths_explored > 0, "Should explore at least one path");
}
