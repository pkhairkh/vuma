//! Determinism, thread-safety, and negative-coverage tests for IVE + BD.
//!
//! This module guards three properties that previous Waves established
//! but did not previously have explicit test coverage:
//!
//! 1. **Cross-run determinism** — running BD inference and IVE
//!    verification on the same SCG twice in the same process must
//!    produce bit-identical results (same BD map, same constraints,
//!    same verdicts, same counterexamples). This is the regression
//!    guard for the W1-c fix that swapped `HashMap` -> `BTreeMap` in
//!    the IVE-level fixpoint data. If a future commit silently
//!    re-introduces a `HashMap` whose iteration order leaks into the
//!    result, these tests will fail.
//!
//! 2. **Thread-safety** — running IVE verification on the same SCG
//!    from 4 concurrent threads must produce identical results in
//!    every thread. This catches accidental `thread_local!` caches
//!    that carry state across calls, `lazy_static` / `OnceCell`
//!    globals that get polluted by one thread's run, and any other
//!    shared mutable state.
//!
//! 3. **Loop determinism** — running verification 10 times in a
//!    tight loop in the same thread must produce identical results
//!    every iteration. This catches `thread_local!` caches that
//!    accumulate state across calls within a single thread (a class
//!    of bug the cross-run test alone would not catch).
//!
//! 4. **Negative coverage for the five core invariants** — one
//!    UNSAFE program per invariant (liveness / cleanup / exclusivity
//!    / origin / interpretation), each asserting the verifier
//!    catches the violation it is supposed to catch. These guard
//!    against a verifier silently regressing into a vacuous "always
//!    Proven" implementation.
//!
//! 5. **Metamorphic pair** — a safe program (alloc -> write -> read ->
//!    free) is verified clean; removing the `free` (alloc -> write ->
//!    read) is verified dirty. This is the minimal sound/unsound
//!    pair: a one-line change to the input flips the verdict.
//!
//! # Cross-process determinism (documentation only)
//!
//! True cross-process determinism cannot be tested in-process because
//! the test binary itself is a single process. The W1-c algorithmic
//! invariant that establishes cross-process determinism is:
//!
//! - All iteration over fixpoint data in the IVE seam uses
//!   `BTreeMap` (ordered by `NodeId`), so two processes running the
//!   same SCG through the same code path produce identical
//!   `InferenceResult.bd_map`, `InferenceResult.constraints`, and
//!   `Vec<VerificationResult>`.
//!
//! - The underlying `vuma_bd::BDInferenceEngine` still uses
//!   `hashbrown::HashMap` internally, but the fixpoint algorithm
//!   converges to the same BD values regardless of internal
//!   iteration order (the final BDs are a function of the SCG
//!   structure, not the iteration order), and the seam converts the
//!   HashMap into a BTreeMap before exposing it. Any future change
//!   that makes the underlying engine's output depend on iteration
//!   order will be caught by the in-process cross-run test below.
//!
//! If a future commit needs to verify cross-process determinism
//! (e.g. after a reproducibility bug report), run
//! `cargo test -p vuma-tests --lib determinism -- --nocapture`
//! in two separate shells and diff the printed `format!("{:?}", ...)`
//! strings; they should match byte-for-byte.

use std::sync::Arc;
use std::thread;

use vuma_bd::capd::CapD;
use vuma_bd::descriptor::BD;
use vuma_bd::reld::RelD;
use vuma_bd::repd::{ByteRep, RepD};
use vuma_ive::cleanup::{
    CleanupGraph, CleanupVerifier, OperationKind, ResourceId as CleanupResourceId,
    ResourceKind as CleanupResourceKind, ViolationKind as CleanupViolationKind,
};
use vuma_ive::exclusivity::{
    AccessId as ExclusivityAccessId, AccessKind as ExclusivityAccessKind, AccessRecord,
    ExclusivityInput, ExclusivityVerifier,
};
use vuma_ive::interpretation::{InterpretationVerifier, LocationId, ProgramPointId};
use vuma_ive::liveness::{
    ControlFlowEdge as LivenessCfgEdge, EventAction, LivenessInput, LivenessVerifier, PointId,
    ResourceEvent, ResourceId as LivenessResourceId, ResourceKind as LivenessResourceKind, ThreadId,
};
use vuma_ive::origin::{
    Access as OriginAccess, AccessId as OriginAccessId, AccessKind as OriginAccessKind,
    Address as OriginAddress, Derivation, DerivationId, DerivationKind, DerivationSource,
    OriginVerifier, Region as OriginRegion, RegionId as OriginRegionId,
    ViolationKind as OriginViolationKind,
};
use vuma_ive::{InferenceEngine, VerificationEngine, VerificationInput, VerificationResult};
use vuma_scg::region::RegionId;
use vuma_scg::{
    AccessMode, AccessNode, AllocationNode, ComputationNode, ControlKind, ControlNode,
    DeallocationNode, DeploymentTarget, EdgeKind, NodePayload, NodeType, ProgramPoint, SCGRegion,
    SCG,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Construct a `ProgramPoint` for the given line in `determinism.vu`.
fn pp(line: u64) -> ProgramPoint {
    ProgramPoint {
        file: Some("determinism.vu".to_string()),
        line: Some(line),
        column: Some(1),
        offset: None,
    }
}

/// Construct a `RegionId`.
fn region(n: u64) -> RegionId {
    RegionId::new(n)
}

/// Construct a flat byte `BD` with the given size and 8-byte alignment,
/// full capabilities, and empty relations. Used as a stand-in for an
/// inferred BD when feeding the interpretation verifier directly.
fn byte_bd(size: u64) -> BD {
    BD::new(
        RepD::Byte(ByteRep { size, align: 8 }),
        CapD::all(),
        RelD::empty(),
    )
}

/// Build a moderately rich SCG used by the determinism tests.
///
/// Structure:
///
/// ```text
///   entry -CF-> alloc_1 -CF-> write_1 -CF-> read_1 -CF-> free_1
///                                                                |
///                                                                CF
///                                                                v
///   alloc_2 -CF-> write_2 -CF-> read_2 -CF-> free_2 -CF-> computation -CF-> ret
///      ^ DF                               | DF                  ^ DF
///      |                                  v                      |
///      `----(alloc_2 also feeds write_2)  (read_2 --DF--> computation)
/// ```
///
/// The graph has:
/// - 2 regions, 2 allocations, 2 deallocations
/// - 4 accesses (write_1, read_1, write_2, read_2)
/// - 1 computation node consuming both reads via DataFlow fan-in
/// - DataFlow + ControlFlow edges
///
/// This is rich enough to exercise:
/// - BD inference over DataFlow fan-in (the computation node has 2 inputs)
/// - Exclusivity pairwise conflict check across 4 accesses in 2 regions
/// - Cleanup path-sensitive DFS through the dealloc chain
/// - Liveness use-after-free / leak analysis across 2 regions
fn build_rich_scg() -> SCG {
    let mut scg = SCG::new();
    let r1 = region(1);
    let r2 = region(2);

    // Region 1: entry -> alloc_1 -> write_1 -> read_1 -> free_1
    let entry = scg.add_node(
        NodeType::Control,
        NodePayload::Control(ControlNode {
            kind: ControlKind::FunctionEntry,
            label: None,
        }),
        pp(1),
    );
    let alloc_1 = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 8,
            align: 8,
            region_id: r1,
            type_name: Some("i64".to_string()),
        }),
        pp(2),
    );
    let write_1 = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Write,
            region_id: r1,
            offset: Some(0),
            access_size: Some(8),
        }),
        pp(3),
    );
    let read_1 = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id: r1,
            offset: Some(0),
            access_size: Some(8),
        }),
        pp(4),
    );
    let free_1 = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc_1,
            region_id: r1,
        }),
        pp(5),
    );

    // Region 2: alloc_2 -> write_2 -> read_2 -> free_2
    let alloc_2 = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 16,
            align: 8,
            region_id: r2,
            type_name: Some("[u8; 16]".to_string()),
        }),
        pp(6),
    );
    let write_2 = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Write,
            region_id: r2,
            offset: Some(0),
            access_size: Some(16),
        }),
        pp(7),
    );
    let read_2 = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id: r2,
            offset: Some(0),
            access_size: Some(8),
        }),
        pp(8),
    );
    let free_2 = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc_2,
            region_id: r2,
        }),
        pp(9),
    );

    // Computation node consuming both reads via DataFlow fan-in.
    let computation = scg.add_node(
        NodeType::Computation,
        NodePayload::Computation(ComputationNode::new(
            "merge",
            Some("i64".to_string()),
            false,
        )),
        pp(10),
    );

    let ret = scg.add_node(
        NodeType::Control,
        NodePayload::Control(ControlNode {
            kind: ControlKind::FunctionReturn,
            label: None,
        }),
        pp(11),
    );

    // ControlFlow chain (sequential execution):
    //   entry -> alloc_1 -> write_1 -> read_1 -> free_1
    //          -> alloc_2 -> write_2 -> read_2 -> free_2 -> computation -> ret
    scg.add_edge(entry, alloc_1, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(alloc_1, write_1, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(write_1, read_1, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(read_1, free_1, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(free_1, alloc_2, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(alloc_2, write_2, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(write_2, read_2, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(read_2, free_2, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(free_2, computation, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(computation, ret, EdgeKind::ControlFlow).unwrap();

    // DataFlow: allocations feed their writes; writes feed reads;
    // reads feed the computation (fan-in).
    scg.add_edge(alloc_1, write_1, EdgeKind::DataFlow).unwrap();
    scg.add_edge(write_1, read_1, EdgeKind::DataFlow).unwrap();
    scg.add_edge(alloc_2, write_2, EdgeKind::DataFlow).unwrap();
    scg.add_edge(write_2, read_2, EdgeKind::DataFlow).unwrap();
    scg.add_edge(read_1, computation, EdgeKind::DataFlow).unwrap();
    scg.add_edge(read_2, computation, EdgeKind::DataFlow).unwrap();

    // Register regions.
    let mut reg1 = SCGRegion::new(r1, DeploymentTarget::Heap);
    reg1.add_node(alloc_1);
    reg1.add_node(write_1);
    reg1.add_node(read_1);
    reg1.add_node(free_1);
    scg.add_region(reg1);

    let mut reg2 = SCGRegion::new(r2, DeploymentTarget::Heap);
    reg2.add_node(alloc_2);
    reg2.add_node(write_2);
    reg2.add_node(read_2);
    reg2.add_node(free_2);
    scg.add_region(reg2);

    scg
}

/// Structured comparison of two `InferenceResult`s for the determinism
/// tests. Compares every field that could be affected by iteration-order
/// non-determinism, with field-specific failure messages.
fn assert_inference_results_equal(
    name: &str,
    a: &vuma_ive::InferenceResult,
    b: &vuma_ive::InferenceResult,
) {
    assert_eq!(
        a.bd_map, b.bd_map,
        "{}: BD map differs between runs (HashMap-iteration-order regression?)",
        name
    );
    assert_eq!(
        a.constraints, b.constraints,
        "{}: constraint set differs between runs",
        name
    );
    assert_eq!(
        a.iterations, b.iterations,
        "{}: fixpoint iteration count differs between runs",
        name
    );
    assert_eq!(
        a.warnings, b.warnings,
        "{}: warnings differ between runs",
        name
    );
    // `InferenceError` does not implement `PartialEq`, so compare via
    // `Debug`. The `Debug` output is deterministic because the enum
    // variants only contain ordered containers and primitives.
    assert_eq!(
        format!("{:?}", a.errors),
        format!("{:?}", b.errors),
        "{}: errors differ between runs",
        name
    );
}

// ---------------------------------------------------------------------------
// 1. Cross-run determinism
// ---------------------------------------------------------------------------

/// Run BD inference + IVE verification on the same SCG twice in the same
/// process. The two runs MUST produce identical BD maps, identical
/// constraint sets, identical iteration counts, and identical
/// verification verdicts (including counterexample descriptions).
///
/// This is the primary regression guard for the W1-c BTreeMap fix.
#[test]
fn test_cross_run_determinism() {
    let scg = build_rich_scg();

    // Run 1
    let inference_1 = InferenceEngine::new().infer(&scg);
    let verify_1 = VerificationEngine::new();
    let input_1 = VerificationInput::from_scg(scg.clone());
    let results_1 = verify_1.verify_all(&input_1);

    // Run 2 — fresh engine, fresh input, same SCG.
    let inference_2 = InferenceEngine::new().infer(&scg);
    let verify_2 = VerificationEngine::new();
    let input_2 = VerificationInput::from_scg(scg.clone());
    let results_2 = verify_2.verify_all(&input_2);

    // Structured field-by-field comparison (better failure messages).
    assert_inference_results_equal("cross-run", &inference_1, &inference_2);

    // Whole-result `Debug` comparison (catches any field we forgot).
    assert_eq!(
        format!("{:?}", inference_1),
        format!("{:?}", inference_2),
        "cross-run: InferenceResult Debug output differs"
    );

    // Verification results: identical verdicts, messages, and
    // counterexamples across both runs.
    assert_eq!(
        results_1.len(),
        results_2.len(),
        "cross-run: verify_all returned different result counts"
    );
    for (i, (r1, r2)) in results_1.iter().zip(results_2.iter()).enumerate() {
        assert_eq!(
            r1, r2,
            "cross-run: verification result {} differs between runs\n\
             run 1: {}\n\
             run 2: {}",
            i, r1, r2
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Thread-safety (4 concurrent threads)
// ---------------------------------------------------------------------------

/// Run IVE verification on the same SCG from 4 threads concurrently.
/// All 4 threads MUST return identical `Vec<VerificationResult>`.
///
/// Each thread clones the SCG (so the SCG itself is not shared
/// mutably) and constructs its own `VerificationEngine`. If the
/// verification pipeline reads from any process-global mutable state
/// (`lazy_static`, `OnceCell`, an unsynchronised `static mut`), or
/// from a `thread_local!` cache that accumulates state across calls,
/// the threads' results will diverge.
///
/// Note on `thread_local!`: a per-thread cache would not cause
/// divergence between threads (each thread has its own) but would
/// cause divergence across calls within a thread — that case is
/// covered by `test_loop_determinism_10_runs`. The thread-safety
/// test specifically targets cross-thread interference from
/// process-global state.
#[test]
fn test_thread_safety_4_threads() {
    let scg = Arc::new(build_rich_scg());

    // Each thread clones the SCG (cheap relative to verification) and
    // runs the full pipeline. We capture both the inference Debug
    // string and the verification results.
    let mut handles = Vec::new();
    for thread_id in 0..4u32 {
        let scg_clone = Arc::clone(&scg);
        let handle = thread::spawn(move || {
            let local_scg = (*scg_clone).clone();
            let inference = InferenceEngine::new().infer(&local_scg);
            let verification = VerificationEngine::new();
            let input = VerificationInput::from_scg(local_scg);
            let results = verification.verify_all(&input);
            (thread_id, format!("{:?}", inference), results)
        });
        handles.push(handle);
    }

    let mut all_inference: Vec<(u32, String)> = Vec::new();
    let mut all_results: Vec<(u32, Vec<VerificationResult>)> = Vec::new();
    for handle in handles {
        let (tid, inference_debug, results) = handle.join().expect("worker thread panicked");
        all_inference.push((tid, inference_debug));
        all_results.push((tid, results));
    }

    // Sort by thread_id so the comparison is stable regardless of
    // which thread finishes first (this itself guards against the
    // test being flaky due to thread scheduling).
    all_inference.sort_by_key(|(t, _)| *t);
    all_results.sort_by_key(|(t, _)| *t);

    // All 4 inference Debug strings must match.
    let baseline_inference = &all_inference[0].1;
    for (tid, inference_debug) in &all_inference[1..] {
        assert_eq!(
            inference_debug, baseline_inference,
            "thread-safety: thread {}'s InferenceResult differs from thread 0's",
            tid
        );
    }

    // All 4 verification result vectors must match element-by-element.
    let baseline_results = &all_results[0].1;
    for (tid, results) in &all_results[1..] {
        assert_eq!(
            results.len(),
            baseline_results.len(),
            "thread-safety: thread {} returned a different number of results",
            tid
        );
        for (i, (r_thread, r_baseline)) in results.iter().zip(baseline_results.iter()).enumerate() {
            assert_eq!(
                r_thread, r_baseline,
                "thread-safety: thread {}'s verification result {} differs from thread 0's\n\
                 thread 0: {}\n\
                 thread {}: {}",
                tid, i, r_baseline, tid, r_thread
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Loop determinism (10 runs in the same thread)
// ---------------------------------------------------------------------------

/// Run IVE verification on the same SCG 10 times in a tight loop in
/// the same thread. All 10 runs MUST produce identical results.
///
/// This catches `thread_local!` caches that accumulate state across
/// calls within a single thread (the cross-run test alone would not
/// catch this, because both runs see the same cached state and would
/// agree — but the cache would mask a real bug if the *first* call
/// ever returned a different answer than steady-state).
#[test]
fn test_loop_determinism_10_runs() {
    let scg = build_rich_scg();

    let mut last_inference: Option<String> = None;
    let mut last_results: Option<Vec<VerificationResult>> = None;

    for run in 0..10u32 {
        let inference = InferenceEngine::new().infer(&scg);
        let verification = VerificationEngine::new();
        let input = VerificationInput::from_scg(scg.clone());
        let results = verification.verify_all(&input);

        let inference_debug = format!("{:?}", inference);

        if let Some(prev_inference) = &last_inference {
            assert_eq!(
                &inference_debug, prev_inference,
                "loop-determinism: InferenceResult at run {} differs from run 0",
                run
            );
        }
        if let Some(prev_results) = &last_results {
            assert_eq!(
                results.len(),
                prev_results.len(),
                "loop-determinism: result count at run {} differs from run 0",
                run
            );
            for (i, (r, p)) in results.iter().zip(prev_results.iter()).enumerate() {
                assert_eq!(
                    r, p,
                    "loop-determinism: verification result {} at run {} differs from run 0\n\
                     run 0: {}\n\
                     run {}: {}",
                    i, run, p, run, r
                );
            }
        }

        last_inference = Some(inference_debug);
        last_results = Some(results);
    }

    // Sanity: we actually ran 10 iterations.
    assert!(last_results.is_some(), "loop-determinism: never ran");
}

// ---------------------------------------------------------------------------
// 4. Per-invariant negative tests
// ---------------------------------------------------------------------------

/// **Liveness (UseAfterFree)** — alloc -> free -> read.
///
/// Builds a `LivenessInput` modelling: allocate R1 at PP1, deallocate
/// R1 at PP2, read R1 at PP3, with CFG edges PP1->PP2->PP3. The
/// `LivenessVerifier` MUST flag a `UseAfterFree` violation for R1 at
/// PP3 (after dealloc at PP2).
#[test]
fn test_liveness_use_after_free_negative() {
    let mut input = LivenessInput::new();
    let r1 = LivenessResourceId(1);
    let t0 = ThreadId(0);

    input.add_event(ResourceEvent {
        resource: r1,
        kind: LivenessResourceKind::Memory,
        event: EventAction::Allocate,
        point: PointId(1),
        thread: t0,
    });
    input.add_event(ResourceEvent {
        resource: r1,
        kind: LivenessResourceKind::Memory,
        event: EventAction::Deallocate,
        point: PointId(2),
        thread: t0,
    });
    input.add_event(ResourceEvent {
        resource: r1,
        kind: LivenessResourceKind::Memory,
        event: EventAction::Read,
        point: PointId(3),
        thread: t0,
    });

    // CFG: PP1 -> PP2 -> PP3
    input.add_cfg_edge(LivenessCfgEdge {
        from: PointId(1),
        to: PointId(2),
        conditional: false,
        label: None,
    });
    input.add_cfg_edge(LivenessCfgEdge {
        from: PointId(2),
        to: PointId(3),
        conditional: false,
        label: None,
    });
    input.entry_point = Some(PointId(1));

    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify(&input);

    assert!(
        !result.invariant_holds,
        "liveness: UseAfterFree program must NOT hold, got: {:?}",
        result.violations
    );
    let has_uaf = result.violations.iter().any(|v| {
        matches!(
            v,
            vuma_ive::liveness::LivenessViolation::UseAfterFree {
                resource,
                access_point,
                dealloc_point,
                ..
            } if *resource == r1
                && *access_point == PointId(3)
                && *dealloc_point == PointId(2)
        )
    });
    assert!(
        has_uaf,
        "liveness: expected UseAfterFree for R1 at PP3 (after dealloc at PP2), got: {:?}",
        result.violations
    );
}

/// **Cleanup (Leak)** — alloc, no free.
///
/// Builds a `CleanupGraph`: entry -> Acquire(R1) -> Access(R1) -> Return
/// (no Release). The `CleanupVerifier` MUST flag a `Leak` violation
/// for R1 (resource acquired but never released on the path).
#[test]
fn test_cleanup_leak_negative() {
    let mut graph = CleanupGraph::new();
    let r1 = CleanupResourceId(1);

    let entry = graph.add_node(OperationKind::Passthrough, "entry");
    let alloc = graph.add_node(
        OperationKind::Acquire {
            resource: r1,
            kind: CleanupResourceKind::Memory,
        },
        "alloc",
    );
    let access = graph.add_node(OperationKind::Access { resource: r1 }, "write");
    let ret = graph.add_node(OperationKind::Return, "return");

    graph.add_edge(entry, alloc).unwrap();
    graph.add_edge(alloc, access).unwrap();
    graph.add_edge(access, ret).unwrap();
    graph.set_entry(entry).unwrap();

    let verifier = CleanupVerifier::new();
    let report = verifier.verify(&graph);

    assert!(
        !report.clean,
        "cleanup: leak program must NOT be clean, got: {:?}",
        report.violations
    );
    assert!(
        report
            .violations
            .iter()
            .any(|v| v.kind == CleanupViolationKind::Leak && v.resource == r1),
        "cleanup: expected Leak violation for R1, got: {:?}",
        report.violations
    );
}

/// **Exclusivity (write-write conflict)** — two writes to the same
/// byte range without a sync edge.
///
/// Builds an `ExclusivityInput` with two write accesses to address
/// 0x1000, size 8, no sync edge and no program-order edge. The
/// `ExclusivityVerifier` MUST flag a write-write conflict.
#[test]
fn test_exclusivity_write_write_conflict_negative() {
    let mut input = ExclusivityInput::new();

    input.add_access(AccessRecord::new(
        ExclusivityAccessId(1),
        ExclusivityAccessKind::Write,
        0x1000,
        8,
        "determinism.vu:1".to_string(),
        1,
        1,
    ));
    input.add_access(AccessRecord::new(
        ExclusivityAccessId(2),
        ExclusivityAccessKind::Write,
        0x1000,
        8,
        "determinism.vu:2".to_string(),
        1,
        1,
    ));
    // No sync edge, no program-order edge — the two writes are
    // concurrent and alias the same byte range.

    let verifier = ExclusivityVerifier::new();
    let output = verifier.verify(&input);

    assert!(
        output.is_violated(),
        "exclusivity: two concurrent writes must be Violated, got: {:?}",
        output.result.status
    );
    assert!(
        output.write_write_count() > 0,
        "exclusivity: expected at least one WriteWrite conflict, got {} conflicts: {:?}",
        output.conflict_count(),
        output.conflicts
    );
}

/// **Origin (FabricatedPointer)** — access via a fabricated pointer.
///
/// Builds an `OriginVerifier` with one derivation whose source is
/// `DerivationSource::Fabricated { raw_value: 0xDEADBEEF }`, plus
/// a read access targeting that derivation. The verifier MUST flag
/// a `FabricatedPointer` violation.
#[test]
fn test_origin_fabricated_pointer_negative() {
    let mut verifier = OriginVerifier::new();

    // One real region (so the verifier has something to compare
    // against; the fabricated pointer is not derived from it).
    verifier.add_region(OriginRegion::new(
        OriginRegionId(1),
        OriginAddress::new(0x1000),
        256,
    ));

    // Fabricated derivation: an integer literal cast to an address
    // with no backing allocation.
    verifier.add_derivation(Derivation::new(
        DerivationId(1),
        DerivationSource::Fabricated {
            raw_value: 0xDEADBEEF,
        },
        DerivationKind::Direct,
        (
            OriginAddress::new(0xDEADBEEF),
            OriginAddress::new(0xDEADBFF3),
        ),
    ));

    // Read access through the fabricated pointer.
    verifier.add_access(OriginAccess::new(
        OriginAccessId(1),
        DerivationId(1),
        OriginAccessKind::Read,
        8,
        "determinism.vu:1",
        false,
    ));

    let report = verifier.verify();

    assert!(
        !report.is_clean(),
        "origin: fabricated-pointer program must NOT be clean, got: {} violations",
        report.violations.len()
    );
    let has_fab = report.violations.iter().any(|v| {
        matches!(
            &v.kind,
            OriginViolationKind::FabricatedPointer {
                derivation_id: DerivationId(1),
                ..
            }
        )
    });
    assert!(
        has_fab,
        "origin: expected FabricatedPointer for DerivationId(1), got: {:?}",
        report
            .violations
            .iter()
            .map(|v| &v.kind)
            .collect::<Vec<_>>()
    );
}

/// **Interpretation (RepD mismatch)** — read u64 from u8 allocation.
///
/// Builds an `InterpretationVerifier` with a write of a 1-byte BD
/// (modelling a `u8` allocation) followed by a read of an 8-byte BD
/// (modelling a `u64` read). The RepDs are incompatible (size
/// mismatch), so the verifier MUST flag a violated result.
#[test]
fn test_interpretation_repdu_mismatch_negative() {
    let mut verifier = InterpretationVerifier::new();
    let loc = LocationId(1);

    // Write a 1-byte value (u8).
    verifier.record_write(loc.clone(), byte_bd(1), ProgramPointId(1));
    // Read it back as an 8-byte value (u64) — type confusion.
    verifier.record_read(loc, byte_bd(8), ProgramPointId(2));

    let result = verifier.verify();

    assert!(
        result.is_violated(),
        "interpretation: u8-then-u64 read must be Violated, got: {:?}",
        result.status
    );
}

// ---------------------------------------------------------------------------
// 5. Metamorphic test (safe vs unsafe pair)
// ---------------------------------------------------------------------------

/// Build a "safe" SCG: entry -> alloc -> write -> read -> free -> return,
/// all via ControlFlow edges. Cleanup invariant should be Proven.
///
/// If `include_free` is `false`, the `free` node is omitted and the
/// chain is entry -> alloc -> write -> read -> return. Cleanup invariant
/// should be Violated (Leak).
fn build_metamorphic_scg(include_free: bool) -> SCG {
    let mut scg = SCG::new();
    let r1 = region(1);

    let entry = scg.add_node(
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
            region_id: r1,
            type_name: Some("i64".to_string()),
        }),
        pp(2),
    );
    let write = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Write,
            region_id: r1,
            offset: Some(0),
            access_size: Some(8),
        }),
        pp(3),
    );
    let read = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id: r1,
            offset: Some(0),
            access_size: Some(8),
        }),
        pp(4),
    );

    let mut chain_end = read;
    if include_free {
        let free = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: r1,
            }),
            pp(5),
        );
        scg.add_edge(read, free, EdgeKind::ControlFlow).unwrap();
        chain_end = free;
    }

    let ret = scg.add_node(
        NodeType::Control,
        NodePayload::Control(ControlNode {
            kind: ControlKind::FunctionReturn,
            label: None,
        }),
        pp(6),
    );

    scg.add_edge(entry, alloc, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(alloc, write, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(write, read, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(chain_end, ret, EdgeKind::ControlFlow).unwrap();

    // Region registration (needed for cleanup graph extraction).
    let mut reg = SCGRegion::new(r1, DeploymentTarget::Heap);
    reg.add_node(alloc);
    reg.add_node(write);
    reg.add_node(read);
    scg.add_region(reg);

    scg
}

/// Find the cleanup-result entry in a `verify_all` result vector.
///
/// `verify_all` returns results in the order:
/// `[origin, liveness, exclusivity, interpretation, cleanup]`.
/// This helper extracts the cleanup entry by name to be robust
/// against ordering changes.
fn find_cleanup_result(results: &[VerificationResult]) -> &VerificationResult {
    results
        .iter()
        .find(|r| r.invariant == "cleanup")
        .expect("verify_all must produce a cleanup result")
}

/// **Metamorphic pair** — the minimal sound/unsound pair.
///
/// Take a safe program (alloc -> write -> read -> free) and verify it
/// passes the cleanup invariant. Make ONE small change (remove the
/// `free`) and verify the cleanup invariant now fails.
///
/// This is the simplest possible sound/unsound pair: a one-node
/// change to the input flips the verdict from clean to violated.
/// If the verifier ever regresses into a vacuous "always Proven" or
/// "always Violated" implementation, this test catches it.
#[test]
fn test_metamorphic_safe_vs_unsafe() {
    // Safe variant: alloc -> write -> read -> free -> return.
    let safe_scg = build_metamorphic_scg(true);
    let safe_input = VerificationInput::from_scg(safe_scg);
    let safe_results = VerificationEngine::new().verify_all(&safe_input);
    let safe_cleanup = find_cleanup_result(&safe_results);

    assert!(
        !safe_cleanup.is_violated(),
        "metamorphic: safe program (alloc->write->read->free) should NOT be Violated by cleanup, \
         got: {} — {}",
        safe_cleanup.status,
        safe_cleanup.message
    );

    // Unsafe variant: alloc -> write -> read -> return (no free).
    let unsafe_scg = build_metamorphic_scg(false);
    let unsafe_input = VerificationInput::from_scg(unsafe_scg);
    let unsafe_results = VerificationEngine::new().verify_all(&unsafe_input);
    let unsafe_cleanup = find_cleanup_result(&unsafe_results);

    assert!(
        unsafe_cleanup.is_violated(),
        "metamorphic: unsafe program (alloc->write->read, no free) MUST be Violated by cleanup, \
         got: {} — {}",
        unsafe_cleanup.status,
        unsafe_cleanup.message
    );

    // The verdicts MUST differ. (A regression where both are Proven
    // or both are Violated would also be caught above, but this
    // explicit inequality makes the metamorphic property loud.)
    assert_ne!(
        safe_cleanup.status, unsafe_cleanup.status,
        "metamorphic: safe and unsafe programs must produce different cleanup verdicts"
    );
}
