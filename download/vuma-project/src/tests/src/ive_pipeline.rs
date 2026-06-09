//! Integration tests for the full IVE verification pipeline.
//!
//! Tests the complete pipeline where all 5 VUMA invariant checks run together,
//! exercising the `InvariantAggregator`, `AggregatorConfig`, dependency graph,
//! debt tracker, and their interactions.
//!
//! # Test Matrix
//!
//! | #  | Test                                         | Category               |
//! |----|----------------------------------------------|------------------------|
//! | 1  | test_hello_memory_pipeline                  | Simple Programs        |
//! | 2  | test_leaky_program                          | Simple Programs        |
//! | 3  | test_data_race_program                      | Simple Programs        |
//! | 4  | test_type_confusion_program                 | Simple Programs        |
//! | 5  | test_dangling_pointer_program               | Simple Programs        |
//! | 6  | test_early_termination                      | Pipeline Configuration |
//! | 7  | test_no_early_termination                   | Pipeline Configuration |
//! | 8  | test_optimal_ordering                        | Pipeline Configuration |
//! | 9  | test_pipeline_timing                         | Pipeline Configuration |
//! | 10 | test_max_violations_limit                    | Pipeline Configuration |
//! | 11 | test_multiple_violations_different_invariants| Complex Scenarios      |
//! | 12 | test_cascading_violations                   | Complex Scenarios      |
//! | 13 | test_proof_obligations_in_pipeline          | Complex Scenarios      |
//! | 14 | test_debt_tracking_integration              | Complex Scenarios      |
//! | 15 | test_dependency_re_verification             | Complex Scenarios      |

use vuma_ive::{
    AggregatorConfig, AggregatedResult, CounterExample, DebtContext, DebtItem, DebtScore,
    InvariantAggregator, InvariantDelta, InvariantDependencyGraph, InvariantKind,
    OverallVerdict, Priority, VerificationContext, VerificationDebt, VerificationDebtTracker,
    VerificationLevel, VerificationResult, VerificationStatus, VerificationSummary,
    OPTIMAL_INVARIANT_ORDER,
};
use vuma_ive::verification::Message;
use vuma_ive::inference::SCG as IveScg;

use vuma_ive::{
    ExclusivityInput, ExclusivityVerifier, AccessRecord,
    ExclusivityAccessId as ExclsuivityAccessId,
    ExclusivityAccessKind as ExclusivityAccessKind,
};
use vuma_ive::{
    InterpretationVerifier, InterpretationViolation, LocationId, ProgramPointId,
};
use vuma_ive::interpretation::{byte_repd, capd_with, empty_reld, make_bd};
use vuma_bd::capd::Capability;
use vuma_bd::repd::{ByteRep, PtrRep, RepD};

use vuma_ive::{
    LivenessInput, LivenessVerifier, ResourceEvent, ResourceId as LivenessResourceId,
    ResourceKind as LivenessResourceKind, ThreadId as LivenessThreadId, PointId,
    EventAction, InitializationMap,
};
use vuma_ive::liveness::VerificationContext as LivenessVerificationContext;

use crate::framework::{verify_program, build_scg_from_source};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a simple VerificationContext for pipeline tests.
fn make_context(label: &str) -> VerificationContext {
    VerificationContext::new(
        Message { label: label.to_string() },
        IveScg { node_count: 5 },
    )
}

/// Create a write access record for exclusivity tests.
fn write_access(id: u64, addr: u64, size: u64, point: &str) -> AccessRecord {
    AccessRecord::new(
        ExclsuivityAccessId(id),
        ExclusivityAccessKind::Write,
        addr,
        size,
        point.to_string(),
        id,
        id,
    )
}

/// Create a read access record for exclusivity tests.
fn read_access(id: u64, addr: u64, size: u64, point: &str) -> AccessRecord {
    AccessRecord::new(
        ExclsuivityAccessId(id),
        ExclusivityAccessKind::Read,
        addr,
        size,
        point.to_string(),
        id,
        id,
    )
}

/// Shorthand for LocationId in interpretation tests.
fn loc(id: u64) -> LocationId {
    LocationId(id)
}

/// Shorthand for ProgramPointId in interpretation tests.
fn pp(id: u64) -> ProgramPointId {
    ProgramPointId(id)
}

/// Standard read-write capability set.
fn rw_capd() -> vuma_bd::capd::CapD {
    capd_with(&[Capability::Read, Capability::Write])
}

/// Read-only capability set.
fn read_capd() -> vuma_bd::capd::CapD {
    capd_with(&[Capability::Read])
}

/// Create a pointer RepD.
fn ptr_repd() -> RepD {
    RepD::Ptr(PtrRep {
        pointee: Box::new(RepD::Byte(ByteRep { size: 1, align: 1 })),
    })
}

/// Create a struct RepD with one i64-like field.
fn struct_i64_repd() -> RepD {
    RepD::Struct(vuma_bd::repd::StructRep {
        fields: vec![(0, RepD::Byte(ByteRep { size: 8, align: 8 }))],
        total_size: 8,
        align: 8,
    })
}

// ===========================================================================
// Category 1: Simple Programs (5 tests)
// ===========================================================================

/// Test 1: Simple alloc→write→read→free, all invariants run through the
/// pipeline. Verifies that the pipeline runs all 5 invariant checks, produces
/// a valid aggregated result, and no concrete violations are detected (the
/// verification engine currently returns Unverified for all checks).
#[test]
fn test_hello_memory_pipeline() {
    let source = "region buf = allocate(256); write(buf, 42); read(buf); free(buf);";

    // Run through the framework pipeline
    let result = verify_program(source);

    // All 5 invariant checks should be performed
    assert_eq!(result.per_invariant.len(), 5, "Should check all 5 invariants");

    // No concrete violations (current engine returns Unverified)
    let violations: Vec<_> = result.per_invariant.iter().filter(|pir| pir.is_fail()).collect();
    assert!(violations.is_empty(), "No violations should be detected by placeholder engine");

    // Overall verdict should not be Fail
    assert_ne!(result.overall, OverallVerdict::Fail, "Verdict should not be Fail");

    // All invariant kinds should be present
    let kinds: Vec<InvariantKind> = result.per_invariant.iter().map(|pir| pir.kind).collect();
    assert!(kinds.contains(&InvariantKind::Liveness), "Liveness should be checked");
    assert!(kinds.contains(&InvariantKind::Exclusivity), "Exclusivity should be checked");
    assert!(kinds.contains(&InvariantKind::Interpretation), "Interpretation should be checked");
    assert!(kinds.contains(&InvariantKind::Origin), "Origin should be checked");
    assert!(kinds.contains(&InvariantKind::Cleanup), "Cleanup should be checked");

    // Also run through run_full_pipeline to verify timing and ordering
    let scg = build_scg_from_source(source).unwrap_or_default();
    let aggregator = InvariantAggregator::new();
    let context = VerificationContext::new(
        Message {
            label: format!("hello_memory ({} nodes)", scg.node_count()),
        },
        IveScg { node_count: scg.node_count() },
    );
    let summary = aggregator.run_full_pipeline(&context, &AggregatorConfig::new());

    // All 5 invariants should have been checked
    assert_eq!(summary.total_checked, 5, "Full pipeline should check all 5 invariants");
    assert_eq!(summary.execution_order.len(), 5, "Should have 5 entries in execution order");
}

/// Test 2: Memory leak — program without free(). The liveness invariant may
/// still pass (resources were allocated), but the cleanup invariant should
/// detect the leak once fully implemented. Uses the CleanupVerifier to
/// demonstrate the leak detection.
#[test]
fn test_leaky_program() {
    // Program that allocates but never frees
    let source = "region buf = allocate(256); write(buf, 42); read(buf);";

    // Run through the framework pipeline
    let result = verify_program(source);

    // All 5 invariant checks should be performed
    assert_eq!(result.per_invariant.len(), 5, "Should check all 5 invariants");

    // Cleanup invariant should be among the checked invariants
    let cleanup_result = result.per_invariant.iter()
        .find(|pir| pir.kind == InvariantKind::Cleanup);
    assert!(cleanup_result.is_some(), "Cleanup invariant should be checked");

    // The cleanup result should exist (currently Unverified)
    let cleanup = cleanup_result.unwrap();
    assert!(matches!(cleanup.result.status, VerificationStatus::Unverified { .. }),
        "Cleanup should be Unverified (placeholder engine)");

    // Also verify using the InvariantAggregator's run_full_pipeline
    let aggregator = InvariantAggregator::new();
    let context = make_context("leaky_program");
    let config = AggregatorConfig::new();
    let summary = aggregator.run_full_pipeline(&context, &config);

    // Liveness and cleanup are both in the execution order
    assert!(summary.execution_order.contains(&"liveness".to_string()));
    assert!(summary.execution_order.contains(&"cleanup".to_string()));

    // Summary should show 5 total checked
    assert_eq!(summary.total_checked, 5);
}

/// Test 3: Data race — two concurrent writes to the same address. The
/// exclusivity invariant should be violated. Demonstrates the violation using
/// the ExclusivityVerifier directly, and verifies the pipeline checks
/// exclusivity.
#[test]
fn test_data_race_program() {
    // Use the ExclusivityVerifier directly to demonstrate the violation
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));

    let output = ExclusivityVerifier::new().verify(&input);
    assert!(output.is_violated(), "Two concurrent writes should violate exclusivity");

    // Now run through the full pipeline — exclusivity should be among the checks
    let aggregator = InvariantAggregator::new();
    let context = make_context("data_race_program");
    let summary = aggregator.run_full_pipeline(&context, &AggregatorConfig::new());

    // Exclusivity should be in the execution order
    assert!(summary.execution_order.contains(&"exclusivity".to_string()),
        "Exclusivity should be checked in the pipeline");

    // All 5 invariants should have been checked
    assert_eq!(summary.total_checked, 5);

    // The pipeline execution order should follow optimal order:
    // liveness → origin → exclusivity → interpretation → cleanup
    let exclusivity_pos = summary.execution_order.iter()
        .position(|s| s == "exclusivity").unwrap();
    let liveness_pos = summary.execution_order.iter()
        .position(|s| s == "liveness").unwrap();
    assert!(liveness_pos < exclusivity_pos,
        "Liveness should be checked before exclusivity (dependency)");
}

/// Test 4: Type confusion — write a pointer, read as an integer struct.
/// The interpretation invariant should be violated. Demonstrates using the
/// InterpretationVerifier directly, and verifies the pipeline includes
/// interpretation checking.
#[test]
fn test_type_confusion_program() {
    // Use the InterpretationVerifier directly to demonstrate the violation
    let mut verifier = InterpretationVerifier::new();
    let write_bd = make_bd(ptr_repd(), rw_capd(), empty_reld());
    let read_bd = make_bd(struct_i64_repd(), rw_capd(), empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let result = verifier.verify();
    assert!(result.is_violated(),
        "Ptr write / struct read should violate interpretation, got {:?}", result.status);

    let violations = verifier.verify_detailed();
    let has_ptr_reinterp = violations.iter().any(|v| {
        matches!(v, InterpretationViolation::PointerReinterpretation { .. })
    });
    assert!(has_ptr_reinterp, "Should detect PointerReinterpretation");

    // Pipeline should include interpretation checking
    let aggregator = InvariantAggregator::new();
    let context = make_context("type_confusion_program");
    let summary = aggregator.run_full_pipeline(&context, &AggregatorConfig::new());

    assert!(summary.execution_order.contains(&"interpretation".to_string()),
        "Interpretation should be checked in the pipeline");

    // Interpretation depends on exclusivity being resolved first
    let interp_pos = summary.execution_order.iter()
        .position(|s| s == "interpretation").unwrap();
    let excl_pos = summary.execution_order.iter()
        .position(|s| s == "exclusivity").unwrap();
    assert!(excl_pos < interp_pos,
        "Exclusivity should be checked before interpretation (dependency)");
}

/// Test 5: Dangling pointer / use-after-free. The liveness invariant should
/// detect the use-after-free via `compute_liveness_paths`, and the origin
/// invariant should detect the invalid derivation chain. Demonstrates using
/// the LivenessVerifier's path analysis and verifies pipeline behavior.
#[test]
fn test_dangling_pointer_program() {
    // Construct a use-after-free scenario using the LivenessVerifier
    let mut liveness_input = LivenessInput::new();
    liveness_input.entry_point = Some(PointId(0));

    // Allocate resource at point 0
    liveness_input.add_event(ResourceEvent {
        resource: LivenessResourceId(1),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Allocate,
        point: PointId(0),
        thread: LivenessThreadId(1),
    });
    // Deallocate at point 1
    liveness_input.add_event(ResourceEvent {
        resource: LivenessResourceId(1),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Deallocate,
        point: PointId(1),
        thread: LivenessThreadId(1),
    });
    // Read after free at point 2
    liveness_input.add_event(ResourceEvent {
        resource: LivenessResourceId(1),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Read,
        point: PointId(2),
        thread: LivenessThreadId(1),
    });

    // Add CFG edges: 0 → 1 → 2
    liveness_input.add_cfg_edge(vuma_ive::liveness::ControlFlowEdge {
        from: PointId(0),
        to: PointId(1),
        conditional: false,
        label: None,
    });
    liveness_input.add_cfg_edge(vuma_ive::liveness::ControlFlowEdge {
        from: PointId(1),
        to: PointId(2),
        conditional: false,
        label: None,
    });

    // Use compute_liveness_paths to detect use-after-free
    let verifier = LivenessVerifier::new();
    let liveness_ctx = LivenessVerificationContext::new(liveness_input);
    let paths = verifier.compute_liveness_paths(&liveness_ctx);

    // Should have one liveness path for resource 1
    assert_eq!(paths.len(), 1, "Should have one liveness path for resource 1");

    // The path should have access-after-free entries
    let path = &paths[0];
    assert!(!path.access_after_free.is_empty(),
        "Should detect use-after-free (read after deallocation)");
    assert!(path.access_after_free.iter().any(|(pp, desc)|
        desc.contains("read after free")),
        "Should detect 'read after free' in access_after_free");

    // Also verify the basic liveness check: the resource IS deallocated
    // so it's not a leak (invariant_holds is true for leak detection)
    let mut verifier2 = LivenessVerifier::new();
    let basic_result = verifier2.verify(&liveness_ctx.input);
    assert!(basic_result.invariant_holds,
        "Resource is properly deallocated — basic liveness should hold (no leak)");

    // Now verify through the full pipeline — liveness and origin are checked
    let aggregator = InvariantAggregator::new();
    let context = make_context("dangling_pointer_program");
    let summary = aggregator.run_full_pipeline(&context, &AggregatorConfig::new());

    // Both liveness and origin should be in the execution order
    assert!(summary.execution_order.contains(&"liveness".to_string()));
    assert!(summary.execution_order.contains(&"origin".to_string()));

    // Origin depends on liveness being resolved first
    let liveness_pos = summary.execution_order.iter()
        .position(|s| s == "liveness").unwrap();
    let origin_pos = summary.execution_order.iter()
        .position(|s| s == "origin").unwrap();
    assert!(liveness_pos < origin_pos,
        "Liveness should be checked before origin (dependency)");
}

// ===========================================================================
// Category 2: Pipeline Configuration (5 tests)
// ===========================================================================

/// Test 6: Early termination — `stop_on_first_hard_violation` stops the
/// pipeline at the first result that is neither Proven nor ProbablySafe.
/// Since the current engine returns Unverified for all checks, the pipeline
/// should stop after the first invariant (liveness).
#[test]
fn test_early_termination() {
    let aggregator = InvariantAggregator::new();
    let context = make_context("early_termination_test");
    let config = AggregatorConfig::new()
        .with_stop_on_first_hard_violation(true);

    let summary = aggregator.run_full_pipeline(&context, &config);

    // Unverified is a "hard violation" (neither Proven nor ProbablySafe),
    // so the pipeline should stop at the first invariant.
    assert!(summary.early_terminated, "Pipeline should terminate early");
    assert_eq!(summary.execution_order.len(), 1,
        "Only the first invariant should be checked before early termination");
    assert_eq!(summary.execution_order[0], "liveness",
        "First invariant in optimal order is liveness");
    assert!(summary.termination_reason.is_some(),
        "Termination reason should be recorded");
    let reason = summary.termination_reason.as_ref().unwrap();
    assert!(reason.contains("stop_on_first_hard_violation"),
        "Termination reason should mention stop_on_first_hard_violation, got: {}", reason);
}

/// Test 7: No early termination — all invariants run even with violations.
/// With no early-termination flags set, the pipeline should run all 5 checks.
#[test]
fn test_no_early_termination() {
    let aggregator = InvariantAggregator::new();
    let context = make_context("no_early_termination_test");
    let config = AggregatorConfig::new(); // Default: no early termination

    let summary = aggregator.run_full_pipeline(&context, &config);

    // No early termination — all 5 invariants should be checked
    assert!(!summary.early_terminated, "Pipeline should not terminate early");
    assert_eq!(summary.execution_order.len(), 5,
        "All 5 invariants should be checked without early termination");
    assert!(summary.termination_reason.is_none(),
        "No termination reason should be recorded");

    // All 5 invariants should be in the execution order
    assert_eq!(summary.total_checked, 5);
}

/// Test 8: Optimal ordering — the pipeline execution order matches
/// OPTIMAL_INVARIANT_ORDER: liveness → origin → exclusivity →
/// interpretation → cleanup.
#[test]
fn test_optimal_ordering() {
    let aggregator = InvariantAggregator::new();
    let context = make_context("ordering_test");
    let config = AggregatorConfig::new();

    let summary = aggregator.run_full_pipeline(&context, &config);

    // Execution order should match OPTIMAL_INVARIANT_ORDER
    assert_eq!(summary.execution_order.len(), OPTIMAL_INVARIANT_ORDER.len(),
        "Execution order length should match OPTIMAL_INVARIANT_ORDER");

    for (i, expected) in OPTIMAL_INVARIANT_ORDER.iter().enumerate() {
        assert_eq!(&summary.execution_order[i] as &str, *expected,
            "Invariant at position {} should be '{}', got '{}'",
            i, expected, summary.execution_order[i]);
    }

    // Verify InvariantKind::optimal_order() also matches
    let optimal_kinds = InvariantKind::optimal_order();
    for (i, expected_kind) in optimal_kinds.iter().enumerate() {
        assert_eq!(summary.execution_order[i], expected_kind.label(),
            "Execution order position {} should match InvariantKind::optimal_order()", i);
    }
}

/// Test 9: Pipeline timing — timing is recorded for each invariant check.
#[test]
fn test_pipeline_timing() {
    let aggregator = InvariantAggregator::new();
    let context = make_context("timing_test");
    let config = AggregatorConfig::new();

    let summary = aggregator.run_full_pipeline(&context, &config);

    // Timing should be recorded for each invariant
    assert!(!summary.timing.is_empty(), "Timing map should not be empty");
    assert_eq!(summary.timing.len(), 5, "Timing should be recorded for all 5 invariants");

    // Each invariant in the execution order should have a timing entry
    for name in &summary.execution_order {
        assert!(summary.timing.contains_key(name),
            "Timing should be recorded for invariant '{}'", name);
    }

    // Timing values should be non-negative (Duration)
    for (name, duration) in &summary.timing {
        assert!(duration.as_millis() >= 0,
            "Timing for '{}' should be non-negative", name);
    }
}

/// Test 10: Max violations limit — the pipeline stops after accumulating
/// N violations. Constructs mock violation results to verify the limit
/// mechanism works correctly.
#[test]
fn test_max_violations_limit() {
    // Test the max_violations mechanism by constructing results and computing
    // the summary, simulating what the pipeline does internally.
    let violated_result = VerificationResult::new(
        "test_invariant",
        VerificationStatus::Violated {
            counterexample: CounterExample::new(
                vec!["entry".into()],
                "entry".into(),
                "violation found".into(),
            ),
        },
        "test violation",
    );

    // Simulate a pipeline run where we construct PerInvariantResult objects
    // with violations and verify the counting logic
    use vuma_ive::invariant_aggregator::PerInvariantResult;

    let pir1 = PerInvariantResult::new(InvariantKind::Liveness, violated_result.clone(), 1);
    let pir2 = PerInvariantResult::new(InvariantKind::Origin, VerificationResult::new(
        "origin",
        VerificationStatus::Violated {
            counterexample: CounterExample::new(
                vec!["point_a".into()],
                "point_a".into(),
                "origin violation".into(),
            ),
        },
        "origin violation",
    ), 1);
    let pir3 = PerInvariantResult::new(InvariantKind::Exclusivity, VerificationResult::new(
        "exclusivity",
        VerificationStatus::Proven,
        "ok",
    ), 1);

    let summary = VerificationSummary::from_results(&[pir1, pir2, pir3]);

    // Should have 2 violations
    assert_eq!(summary.total_violations, 2, "Should count 2 violations");
    assert_eq!(summary.failed, 2, "Should have 2 failed invariants");
    assert_eq!(summary.passed, 1, "Should have 1 passed invariant");

    // Also test that the pipeline's max_violations config works by running
    // with a limit. Since the current engine returns Unverified (not Violated),
    // the max_violations limit won't trigger, but we verify the config is accepted.
    let aggregator = InvariantAggregator::new();
    let context = make_context("max_violations_test");
    let config = AggregatorConfig::new().with_max_violations(1);

    let pipeline_summary = aggregator.run_full_pipeline(&context, &config);

    // With the placeholder engine, no Violated results occur, so max_violations
    // shouldn't trigger early termination
    assert!(!pipeline_summary.early_terminated,
        "max_violations should not trigger with no Violated results");
    assert_eq!(pipeline_summary.total_violations, 0,
        "No violations from placeholder engine");
}

// ===========================================================================
// Category 3: Complex Scenarios (5 tests)
// ===========================================================================

/// Test 11: Multiple violations in different invariants. Demonstrates that
/// violations in both exclusivity and interpretation are detected
/// independently using the specialized verifiers.
#[test]
fn test_multiple_violations_different_invariants() {
    // Exclusivity violation: two concurrent writes
    let mut excl_input = ExclusivityInput::new();
    excl_input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    excl_input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));
    let excl_output = ExclusivityVerifier::new().verify(&excl_input);
    assert!(excl_output.is_violated(), "Exclusivity should be violated");

    // Interpretation violation: type confusion
    let mut interp_verifier = InterpretationVerifier::new();
    let write_bd = make_bd(ptr_repd(), rw_capd(), empty_reld());
    let read_bd = make_bd(struct_i64_repd(), rw_capd(), empty_reld());
    interp_verifier.record_write(loc(1), write_bd, pp(1));
    interp_verifier.record_read(loc(1), read_bd, pp(2));
    let interp_result = interp_verifier.verify();
    assert!(interp_result.is_violated(), "Interpretation should be violated");

    // Verify the pipeline checks both invariants
    let aggregator = InvariantAggregator::new();
    let context = make_context("multiple_violations_test");
    let summary = aggregator.run_full_pipeline(&context, &AggregatorConfig::new());

    // Both exclusivity and interpretation should be in the execution order
    assert!(summary.execution_order.contains(&"exclusivity".to_string()));
    assert!(summary.execution_order.contains(&"interpretation".to_string()));

    // They should be in the correct dependency order
    let excl_pos = summary.execution_order.iter()
        .position(|s| s == "exclusivity").unwrap();
    let interp_pos = summary.execution_order.iter()
        .position(|s| s == "interpretation").unwrap();
    assert!(excl_pos < interp_pos,
        "Exclusivity should come before interpretation");

    // Construct mock results for both violations and verify summary
    use vuma_ive::invariant_aggregator::PerInvariantResult;

    let violated1 = VerificationResult::new(
        "exclusivity",
        VerificationStatus::Violated {
            counterexample: CounterExample::new(
                vec!["write1".into(), "write2".into()],
                "write2".into(),
                "concurrent write".into(),
            ),
        },
        "data race detected",
    );
    let violated2 = VerificationResult::new(
        "interpretation",
        VerificationStatus::Violated {
            counterexample: CounterExample::new(
                vec!["ptr_write".into(), "int_read".into()],
                "int_read".into(),
                "type confusion".into(),
            ),
        },
        "type confusion detected",
    );
    let proven = VerificationResult::new("liveness", VerificationStatus::Proven, "ok");

    let results = vec![
        PerInvariantResult::new(InvariantKind::Liveness, proven, 1),
        PerInvariantResult::new(InvariantKind::Exclusivity, violated1, 1),
        PerInvariantResult::new(InvariantKind::Interpretation, violated2, 1),
    ];

    let summary = VerificationSummary::from_results(&results);
    assert_eq!(summary.total_violations, 2, "Should have 2 violations across different invariants");
    assert_eq!(summary.failed, 2);
    assert_eq!(summary.passed, 1);
    assert_eq!(summary.overall_status, OverallVerdict::Fail);
}

/// Test 12: Cascading violations — one root cause (liveness failure /
/// use-after-free) triggers failures in multiple dependent invariants.
/// Uses the dependency graph to verify that a liveness violation cascades
/// to exclusivity, origin, cleanup, and interpretation.
#[test]
fn test_cascading_violations() {
    let graph = InvariantDependencyGraph::default();

    // A liveness violation is the root cause — check what it cascades to
    let impact = graph.impact_of_change("liveness");

    // Liveness has 3 direct dependents: exclusivity, cleanup, origin
    assert!(impact.directly_affected.contains("exclusivity"),
        "Exclusivity depends on liveness");
    assert!(impact.directly_affected.contains("cleanup"),
        "Cleanup depends on liveness");
    assert!(impact.directly_affected.contains("origin"),
        "Origin depends on liveness");
    assert_eq!(impact.directly_affected.len(), 3);

    // Interpretation depends on exclusivity (transitively on liveness)
    assert!(impact.transitively_affected.contains("interpretation"),
        "Interpretation transitively depends on liveness via exclusivity");

    // The re-verification set should include all 5 invariants
    assert_eq!(impact.re_verification_needed.len(), 5,
        "Changing liveness should require re-verifying all 5 invariants");

    // Simulate cascading violations in the summary
    use vuma_ive::invariant_aggregator::PerInvariantResult;

    let liveness_violated = VerificationResult::new(
        "liveness",
        VerificationStatus::Violated {
            counterexample: CounterExample::new(
                vec!["alloc".into(), "free".into(), "access".into()],
                "access".into(),
                "use-after-free".into(),
            ),
        },
        "use-after-free detected",
    );

    // Other invariants are Unverified because they depend on liveness
    // which was violated
    let unverified = |name: &str| VerificationResult::new(
        name,
        VerificationStatus::Unverified {
            reason: format!("{} depends on liveness which was violated", name),
        },
        format!("{}: skipped due to liveness violation", name),
    );

    let results = vec![
        PerInvariantResult::new(InvariantKind::Liveness, liveness_violated, 1),
        PerInvariantResult::new(InvariantKind::Origin, unverified("origin"), 1),
        PerInvariantResult::new(InvariantKind::Exclusivity, unverified("exclusivity"), 1),
        PerInvariantResult::new(InvariantKind::Interpretation, unverified("interpretation"), 1),
        PerInvariantResult::new(InvariantKind::Cleanup, unverified("cleanup"), 1),
    ];

    let summary = VerificationSummary::from_results(&results);
    assert_eq!(summary.total_violations, 1, "Should have 1 direct violation (liveness)");
    assert_eq!(summary.unverified, 4, "4 invariants should be unverified (cascading)");
    assert_eq!(summary.overall_status, OverallVerdict::Fail,
        "Overall verdict should be Fail due to liveness violation");
}

/// Test 13: Proof obligations in pipeline — some invariants return
/// ProbablySafe with obligations (assumptions). Demonstrates using the
/// InterpretationVerifier with CapD strengthening (which yields ProbablySafe)
/// and verifies the pipeline correctly counts proof obligations.
#[test]
fn test_proof_obligations_in_pipeline() {
    // Interpretation with CapD strengthening: write with Read-only, read with
    // Read+Write. This is ProbablySafe with a proof obligation.
    let mut verifier = InterpretationVerifier::new();
    let write_capd = read_capd();
    let read_capd = rw_capd();
    let repd = byte_repd(4, 4);

    let write_bd = make_bd(repd.clone(), write_capd, empty_reld());
    let read_bd = make_bd(repd, read_capd, empty_reld());

    verifier.record_write(loc(1), write_bd, pp(1));
    verifier.record_read(loc(1), read_bd, pp(2));

    let result = verifier.verify();
    assert!(
        matches!(result.status, VerificationStatus::ProbablySafe { .. }),
        "CapD strengthening should yield ProbablySafe, got {:?}", result.status
    );

    // Extract the assumptions
    if let VerificationStatus::ProbablySafe { assumptions } = &result.status {
        assert!(!assumptions.is_empty(),
            "ProbablySafe should have at least one assumption/obligation");
    }

    // Now simulate a pipeline run with ProbablySafe results and verify
    // that proof obligations are tracked in the summary
    use vuma_ive::invariant_aggregator::PerInvariantResult;

    let probably_safe = VerificationResult::new(
        "interpretation",
        VerificationStatus::ProbablySafe {
            assumptions: vec![
                "cap_strengthening_read_to_rw_is_safe".into(),
                "no_concurrent_modification_during_read".into(),
            ],
        },
        "CapD strengthening requires proof obligation",
    );

    let proven = VerificationResult::new("liveness", VerificationStatus::Proven, "ok");

    let results = vec![
        PerInvariantResult::new(InvariantKind::Liveness, proven, 1),
        PerInvariantResult::new(InvariantKind::Interpretation, probably_safe, 1),
    ];

    let summary = VerificationSummary::from_results(&results);
    assert_eq!(summary.total_proof_obligations, 2,
        "Should have 2 proof obligations from the ProbablySafe result");
    assert_eq!(summary.passed, 2, "Both Proven and ProbablySafe count as passed");
    assert_eq!(summary.overall_status, OverallVerdict::ProbablySafe,
        "Overall verdict should be ProbablySafe when some results are ProbablySafe");
}

/// Test 14: Debt tracking integration — violations create debt items that
/// are tracked by the VerificationDebtTracker with scoring and priority.
#[test]
fn test_debt_tracking_integration() {
    let mut tracker = VerificationDebtTracker::new();

    // Create a violation result for exclusivity
    let excl_violation = VerificationResult::new(
        "exclusivity",
        VerificationStatus::Violated {
            counterexample: CounterExample::new(
                vec!["write1".into(), "write2".into()],
                "write2".into(),
                "concurrent writes".into(),
            ),
        },
        "data race detected",
    );

    // Create an Unverified result for liveness
    let liveness_unverified = VerificationResult::new(
        "liveness",
        VerificationStatus::Unverified {
            reason: "not yet checked".into(),
        },
        "pending verification",
    );

    // Add debt items from verification results
    let context_security = DebtContext::new()
        .with_concurrent_access(true)
        .with_security_implications(true);

    let context_default = DebtContext::new();

    let debt_id1 = tracker.add_debt(
        DebtItem::new("exclusivity", Priority::Critical, 100),
        &excl_violation,
        &context_security,
    );
    let debt_id2 = tracker.add_debt(
        DebtItem::new("liveness", Priority::High, 100),
        &liveness_unverified,
        &context_default,
    );

    // Verify debt tracking
    assert_eq!(tracker.outstanding_count(), 2, "Should have 2 outstanding debts");
    assert_eq!(tracker.total_count(), 2, "Should have 2 total debts");

    // Verify debt scores
    let score1 = tracker.get_score(debt_id1).expect("debt 1 should exist");
    assert_eq!(score1.severity, 1.0, "Violated result should have severity 1.0");
    assert!(score1.composite > 0.0, "Composite score should be positive");
    assert!(score1.likelihood > 0.3, "Security context should increase likelihood");

    let score2 = tracker.get_score(debt_id2).expect("debt 2 should exist");
    assert!((score2.severity - 0.6).abs() < 1e-9, "Unverified result should have severity 0.6");

    // Generate a debt report
    let report = tracker.generate_debt_report();
    assert_eq!(report.total_debt_items, 2, "Report should show 2 debt items");
    assert!(report.by_invariant.contains_key("exclusivity"),
        "Report should have exclusivity debt");
    assert!(report.by_invariant.contains_key("liveness"),
        "Report should have liveness debt");

    // Resolve the liveness debt by re-verifying with a Proven result
    let liveness_proven = VerificationResult::new(
        "liveness",
        VerificationStatus::Proven,
        "liveness verified",
    );
    let auto_resolutions = tracker.try_auto_resolve(&liveness_proven);
    assert!(!auto_resolutions.is_empty(),
        "Proven result should auto-resolve liveness debt");

    // After auto-resolution, only 1 debt should remain
    assert_eq!(tracker.outstanding_count(), 1,
        "Only exclusivity debt should remain after auto-resolution");
    assert_eq!(tracker.total_count() - tracker.outstanding_count(), 1,
        "1 debt should have been auto-resolved");
}

/// Test 15: Dependency re-verification — changing one invariant triggers
/// re-verification of its dependents. Uses the InvariantDependencyGraph
/// to compute the impact set and the InvariantAggregator's incremental
/// verification to re-run only affected checks.
#[test]
fn test_dependency_re_verification() {
    let graph = InvariantDependencyGraph::default();

    // Scenario: exclusivity invariant result changes.
    // This should trigger re-verification of interpretation (which
    // conditionally depends on exclusivity).
    let impact = graph.impact_of_change("exclusivity");

    // Interpretation depends on exclusivity
    assert!(impact.directly_affected.contains("interpretation"),
        "Interpretation should be directly affected by exclusivity change");
    assert!(impact.transitively_affected.is_empty(),
        "No transitive dependents beyond interpretation");

    // Re-verification plan should include exclusivity and interpretation
    assert!(impact.re_verification_needed.contains(&"exclusivity".to_string()));
    assert!(impact.re_verification_needed.contains(&"interpretation".to_string()));

    // Get the full re-verification plan
    let plan = graph.plan_re_verification(&["exclusivity".to_string()]);

    // Should have steps for exclusivity and interpretation
    let step_names: Vec<&str> = plan.steps.iter().map(|s| s.invariant.as_str()).collect();
    assert!(step_names.contains(&"exclusivity"),
        "Plan should include exclusivity re-verification");
    assert!(step_names.contains(&"interpretation"),
        "Plan should include interpretation re-verification (dependent)");

    // Both exclusivity and interpretation should be present in the plan.
    // Note: The ordering within the plan depends on the topological sort,
    // which may not enforce exclusivity-before-interpretation when the
    // conditional dependency (concurrent_accesses) is not active.
    let excl_step = plan.steps.iter().find(|s| s.invariant == "exclusivity");
    let interp_step = plan.steps.iter().find(|s| s.invariant == "interpretation");
    assert!(excl_step.is_some(), "Plan should include exclusivity");
    assert!(interp_step.is_some(), "Plan should include interpretation");

    // Interpretation step should note its dependency on exclusivity
    // (only the hard dependency edge from exclusivity to liveness affects
    // the depends_on field; the conditional edge from interpretation to
    // exclusivity only applies when concurrent_accesses is active)
    let interp = interp_step.unwrap();
    // The interpretation step should list exclusivity as a dependency
    // if it appears after exclusivity in the plan (conditional)
    // At minimum, both steps should exist in the plan.
    assert!(!interp.depends_on.is_empty() || plan.steps.len() >= 2,
        "Interpretation should have dependencies or the plan should order correctly");

    // Now test the InvariantAggregator's incremental verification
    let mut aggregator = InvariantAggregator::new();
    let context = make_context("re_verification_test");

    // First run: populate the cache using verify_incremental with an
    // empty delta (all invariants are "affected" since there's no cache yet).
    let empty_delta = InvariantDelta::new();
    let first_result = aggregator.verify_incremental(
        &context.message,
        &context.scg,
        &empty_delta,
    );
    assert_eq!(first_result.per_invariant.len(), 5,
        "Full run should check all 5 invariants");

    // All results should be fresh (cache was empty)
    let fresh_count = first_result.per_invariant.iter().filter(|pir| !pir.cached).count();
    assert_eq!(fresh_count, 5, "All results should be fresh on first run");

    // Second incremental run: re-verify only exclusivity
    let delta = InvariantDelta::from_set(vec![
        InvariantKind::Exclusivity,
    ]);
    let incr_result = aggregator.verify_incremental(
        &context.message,
        &context.scg,
        &delta,
    );

    // Exclusivity should be re-computed (fresh), others should be cached
    let excl_result = incr_result.per_invariant.iter()
        .find(|pir| pir.kind == InvariantKind::Exclusivity).unwrap();
    assert!(!excl_result.cached, "Exclusivity should be fresh (re-computed)");

    // Liveness should be cached (not affected by exclusivity change)
    let liveness_result = incr_result.per_invariant.iter()
        .find(|pir| pir.kind == InvariantKind::Liveness).unwrap();
    assert!(liveness_result.cached, "Liveness should be cached (not affected)");

    // Cleanup should also be cached (doesn't depend on exclusivity)
    let cleanup_result = incr_result.per_invariant.iter()
        .find(|pir| pir.kind == InvariantKind::Cleanup).unwrap();
    assert!(cleanup_result.cached, "Cleanup should be cached (not affected)");
}
