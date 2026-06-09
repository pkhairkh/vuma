//! Integration tests for the LivenessVerifier (IVE module).
//!
//! Comprehensive test suite covering:
//! - Basic liveness (safe access, use-after-free, multiple regions, double-free, uninitialized read)
//! - Path tracking (liveness paths for allocation/deallocation, UAF, multiple resources, dead allocations)
//! - Initialization tracking (full, partial, struct field, array element, multi-write coverage)
//! - Proof obligations (UAF, dead alloc, uninit, safe, multiple issues)

use vuma_ive::{
    DeadReason, EventAction, InitializationMap,
    LivenessInput, LivenessVerificationContext,
    LivenessVerifier, ObligationKind,
    PointId, ProofObligation, ResourceEvent, ResourceId, ResourceKind,
    ThreadId,
};
use vuma_ive::liveness::ControlFlowEdge;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Shorthand for PointId.
fn pp(id: u64) -> PointId {
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
        point: pp(point),
        thread: tid(thread),
    }
}

/// Create a memory Deallocate event.
fn dealloc_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: rid(resource),
        kind: ResourceKind::Memory,
        event: EventAction::Deallocate,
        point: pp(point),
        thread: tid(thread),
    }
}

/// Create a memory Read event.
fn read_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: rid(resource),
        kind: ResourceKind::Memory,
        event: EventAction::Read,
        point: pp(point),
        thread: tid(thread),
    }
}

/// Create a memory Write event.
fn write_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: rid(resource),
        kind: ResourceKind::Memory,
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

// ===========================================================================
// Category 1: Basic Liveness (5 tests)
// ===========================================================================

#[test]
fn test_live_access_safe() {
    // alloc → access → free → should be Proven (no violations)
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));   // alloc R1 at PP1
    input.add_event(write_event(1, 2, 1));    // write R1 at PP2
    input.add_event(read_event(1, 3, 1));     // read R1 at PP3
    input.add_event(dealloc_event(1, 4, 1));  // dealloc R1 at PP4
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);
    input.entry_point = Some(pp(1));

    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify(&input);

    assert!(
        result.invariant_holds,
        "Expected invariant to hold for alloc→access→free, got violations: {:?}",
        result.violations
    );
    assert!(result.violations.is_empty(), "Expected no violations");
    assert_eq!(result.resources_checked, 1);
}

#[test]
fn test_use_after_free() {
    // alloc → free → access → should be detected as use-after-free via
    // compute_liveness_paths and generate a UseAfterFreeSafe obligation
    // via verify_with_proofs.
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));   // alloc R1 at PP1
    input.add_event(dealloc_event(1, 2, 1));  // free R1 at PP2
    input.add_event(read_event(1, 3, 1));     // read R1 at PP3 (UAF!)
    input.cfg_edges = linear_cfg(&[1, 2, 3]);
    input.entry_point = Some(pp(1));

    let context = LivenessVerificationContext::new(input.clone());

    // compute_liveness_paths should detect the access after free
    let verifier = LivenessVerifier::new();
    let paths = verifier.compute_liveness_paths(&context);
    assert_eq!(paths.len(), 1, "Expected 1 liveness path");
    assert_eq!(paths[0].resource_id, 1);
    assert!(
        !paths[0].access_after_free.is_empty(),
        "Expected access_after_free entries for use-after-free"
    );
    assert!(
        paths[0].access_after_free.iter().any(|(_, desc)| desc.contains("read after free")),
        "Expected 'read after free' description in access_after_free"
    );

    // verify_with_proofs should generate UseAfterFreeSafe obligation
    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify_with_proofs(&context);
    let uaf_obligations: Vec<&ProofObligation> = result
        .proof_obligations
        .iter()
        .filter(|o| o.obligation_kind == ObligationKind::UseAfterFreeSafe)
        .collect();
    assert!(
        !uaf_obligations.is_empty(),
        "Expected UseAfterFreeSafe proof obligation, got: {:?}",
        result.proof_obligations
    );
}

#[test]
fn test_multiple_regions_live() {
    // Multiple allocations, all live during access, all properly freed.
    let mut input = LivenessInput::new();
    // R1: alloc → read → dealloc
    input.add_event(alloc_event(1, 1, 1));
    input.add_event(read_event(1, 2, 1));
    input.add_event(dealloc_event(1, 3, 1));
    // R2: alloc → write → read → dealloc
    input.add_event(alloc_event(2, 4, 1));
    input.add_event(write_event(2, 5, 1));
    input.add_event(read_event(2, 6, 1));
    input.add_event(dealloc_event(2, 7, 1));
    // R3: alloc → dealloc (separate region, also clean)
    input.add_event(alloc_event(3, 8, 1));
    input.add_event(write_event(3, 9, 1));
    input.add_event(read_event(3, 10, 1));
    input.add_event(dealloc_event(3, 11, 1));
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    input.entry_point = Some(pp(1));

    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify(&input);

    assert!(
        result.invariant_holds,
        "Expected invariant to hold for multiple live regions, got violations: {:?}",
        result.violations
    );
    assert!(result.violations.is_empty());
    assert_eq!(result.resources_checked, 3);
}

#[test]
fn test_double_free_liveness() {
    // alloc → free → free (double free). The standard verify() won't
    // detect a ResourceLeak (dealloc exists), but detect_dead_allocations
    // will flag it as NeverAccessed (no reads/writes), and
    // verify_with_proofs generates a DeadAllocationNeeded obligation.
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));    // alloc R1 at PP1
    input.add_event(dealloc_event(1, 2, 1));  // free R1 at PP2
    input.add_event(dealloc_event(1, 3, 1));  // free R1 again at PP3 (double free!)
    input.cfg_edges = linear_cfg(&[1, 2, 3]);
    input.entry_point = Some(pp(1));

    let context = LivenessVerificationContext::new(input.clone());

    // Standard verify: no leak (dealloc exists and is reachable)
    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify(&input);
    assert!(
        result.invariant_holds,
        "verify() should not find a leak (dealloc is reachable), got: {:?}",
        result.violations
    );

    // detect_dead_allocations: resource has no read/write → NeverAccessed
    let dead = verifier.detect_dead_allocations(&context);
    assert_eq!(dead.len(), 1, "Expected 1 dead allocation");
    assert_eq!(dead[0].resource_id, 1);
    assert!(
        matches!(dead[0].reason, DeadReason::NeverAccessed),
        "Expected NeverAccessed reason, got: {:?}",
        dead[0].reason
    );

    // verify_with_proofs: should generate DeadAllocationNeeded obligation
    let mut verifier2 = LivenessVerifier::new();
    let result = verifier2.verify_with_proofs(&context);
    let dead_obligations: Vec<&ProofObligation> = result
        .proof_obligations
        .iter()
        .filter(|o| o.obligation_kind == ObligationKind::DeadAllocationNeeded)
        .collect();
    assert!(
        !dead_obligations.is_empty(),
        "Expected DeadAllocationNeeded proof obligation for double-free, got: {:?}",
        result.proof_obligations
    );
}

#[test]
fn test_uninitialized_read() {
    // Read of partially-initialized region → Violated (partial init detected).
    // We set up a region where some bytes are initialized but not all,
    // then a Read event triggers the partial initialization check.
    // The check_partial_initialization method checks from min_start to
    // max_end of the init_map data, so we need at least two non-contiguous
    // initialized ranges to create a detectable gap.
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));    // alloc R1 at PP1
    input.add_event(write_event(1, 2, 1));     // write R1 at PP2 (partial init)
    input.add_event(read_event(1, 3, 1));      // read R1 at PP3 (reads uninitialized bytes!)
    input.add_event(dealloc_event(1, 4, 1));   // dealloc R1 at PP4
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);
    input.entry_point = Some(pp(1));

    // Set up init_map: bytes 0-4 are initialized, and bytes 8-12 are initialized,
    // but bytes 4-8 are NOT. This represents a partial write with a gap.
    let mut init_map = InitializationMap::new();
    init_map.mark_initialized(1, 0, 4);
    init_map.mark_initialized(1, 8, 12);

    let context = LivenessVerificationContext::with_init_map(input, init_map);

    // check_partial_initialization should detect the gap
    let verifier = LivenessVerifier::new();
    let violations = verifier.check_partial_initialization(&context);
    assert_eq!(violations.len(), 1, "Expected 1 partial init violation");
    assert_eq!(violations[0].region_id, 1);
    assert!(
        !violations[0].uninitialized_ranges.is_empty(),
        "Expected uninitialized ranges to be non-empty"
    );
    // The gap should be at bytes 4-8
    assert!(
        violations[0].uninitialized_ranges.contains(&(4, 8)),
        "Expected uninitialized gap at [4,8), got: {:?}",
        violations[0].uninitialized_ranges
    );
}

// ===========================================================================
// Category 2: Path Tracking (5 tests)
// ===========================================================================

#[test]
fn test_liveness_path_alloc_free() {
    // Verify LivenessPath has correct allocation/deallocation points and
    // no access_after_free entries for a clean lifecycle.
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 10, 1));   // alloc R1 at PP10
    input.add_event(write_event(1, 20, 1));    // write R1 at PP20
    input.add_event(read_event(1, 30, 1));     // read R1 at PP30
    input.add_event(dealloc_event(1, 40, 1));  // dealloc R1 at PP40
    input.cfg_edges = linear_cfg(&[10, 20, 30, 40]);
    input.entry_point = Some(pp(10));

    let context = LivenessVerificationContext::new(input);
    let verifier = LivenessVerifier::new();
    let paths = verifier.compute_liveness_paths(&context);

    assert_eq!(paths.len(), 1, "Expected 1 liveness path");
    let path = &paths[0];
    assert_eq!(path.resource_id, 1);
    assert_eq!(path.resource_kind, "memory");
    assert_eq!(path.allocation_point, "PP10");
    assert_eq!(path.deallocation_point, Some("PP40".to_string()));
    assert!(
        path.access_after_free.is_empty(),
        "Expected no access_after_free for clean lifecycle, got: {:?}",
        path.access_after_free
    );
}

#[test]
fn test_liveness_path_use_after_free() {
    // LivenessPath shows access after free: dealloc at PP2, read at PP3
    // with CFG PP1→PP2→PP3, PP3 is reachable from PP2 → UAF detected.
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));    // alloc R1 at PP1
    input.add_event(dealloc_event(1, 2, 1));   // free R1 at PP2
    input.add_event(read_event(1, 3, 1));      // read R1 at PP3 (after free!)
    input.add_event(write_event(1, 4, 1));     // write R1 at PP4 (after free!)
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);
    input.entry_point = Some(pp(1));

    let context = LivenessVerificationContext::new(input);
    let verifier = LivenessVerifier::new();
    let paths = verifier.compute_liveness_paths(&context);

    assert_eq!(paths.len(), 1);
    let path = &paths[0];
    assert_eq!(path.allocation_point, "PP1");
    assert_eq!(path.deallocation_point, Some("PP2".to_string()));
    assert_eq!(
        path.access_after_free.len(),
        2,
        "Expected 2 access-after-free entries (read + write), got: {:?}",
        path.access_after_free
    );
    // Verify both read-after-free and write-after-free are present
    let has_read_af = path.access_after_free.iter().any(|(_, d)| d.contains("read after free"));
    let has_write_af = path.access_after_free.iter().any(|(_, d)| d.contains("write after free"));
    assert!(has_read_af, "Expected read-after-free entry");
    assert!(has_write_af, "Expected write-after-free entry");
}

#[test]
fn test_multiple_resources_liveness_paths() {
    // Multiple resources with different lifecycles:
    //   R1: clean (alloc→write→read→free)
    //   R2: UAF (alloc→free→read)
    //   R3: leaked (alloc only, no free)
    let mut input = LivenessInput::new();
    // R1: clean lifecycle
    input.add_event(alloc_event(1, 1, 1));
    input.add_event(write_event(1, 2, 1));
    input.add_event(read_event(1, 3, 1));
    input.add_event(dealloc_event(1, 4, 1));
    // R2: use-after-free
    input.add_event(alloc_event(2, 5, 1));
    input.add_event(dealloc_event(2, 6, 1));
    input.add_event(read_event(2, 7, 1));
    // R3: leaked (no dealloc)
    input.add_event(alloc_event(3, 8, 1));
    input.add_event(write_event(3, 9, 1));

    // CFG: linear path covering all points
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    input.entry_point = Some(pp(1));

    let context = LivenessVerificationContext::new(input);
    let verifier = LivenessVerifier::new();
    let paths = verifier.compute_liveness_paths(&context);

    assert_eq!(paths.len(), 3, "Expected 3 liveness paths");

    // Find each path by resource_id
    let r1_path = paths.iter().find(|p| p.resource_id == 1).expect("R1 path");
    let r2_path = paths.iter().find(|p| p.resource_id == 2).expect("R2 path");
    let r3_path = paths.iter().find(|p| p.resource_id == 3).expect("R3 path");

    // R1: clean → no access_after_free
    assert!(r1_path.access_after_free.is_empty(),
        "R1 should have no access_after_free");

    // R2: UAF → access_after_free present
    assert!(!r2_path.access_after_free.is_empty(),
        "R2 should have access_after_free (read after dealloc)");

    // R3: leaked → no deallocation_point
    assert!(r3_path.deallocation_point.is_none(),
        "R3 should have no deallocation_point (leaked)");
    assert!(r3_path.access_after_free.is_empty(),
        "R3 should have no access_after_free (never freed)");
}

#[test]
fn test_dead_allocation_never_accessed() {
    // Allocation that's never accessed (no read, no write).
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));    // alloc R1 at PP1
    input.add_event(dealloc_event(1, 2, 1));   // dealloc R1 at PP2
    input.cfg_edges = linear_cfg(&[1, 2]);
    input.entry_point = Some(pp(1));

    let context = LivenessVerificationContext::new(input);
    let verifier = LivenessVerifier::new();
    let dead = verifier.detect_dead_allocations(&context);

    assert_eq!(dead.len(), 1, "Expected 1 dead allocation");
    assert_eq!(dead[0].resource_id, 1);
    assert!(
        matches!(dead[0].reason, DeadReason::NeverAccessed),
        "Expected NeverAccessed, got: {:?}",
        dead[0].reason
    );
    assert_eq!(dead[0].allocation_point, "PP1");
}

#[test]
fn test_dead_allocation_write_only() {
    // Allocation that's only written to, never read from.
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));    // alloc R1 at PP1
    input.add_event(write_event(1, 2, 1));     // write R1 at PP2
    input.add_event(write_event(1, 3, 1));     // write R1 at PP3 again
    input.add_event(dealloc_event(1, 4, 1));   // dealloc R1 at PP4
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);
    input.entry_point = Some(pp(1));

    let context = LivenessVerificationContext::new(input);
    let verifier = LivenessVerifier::new();
    let dead = verifier.detect_dead_allocations(&context);

    assert_eq!(dead.len(), 1, "Expected 1 dead allocation");
    assert_eq!(dead[0].resource_id, 1);
    assert!(
        matches!(dead[0].reason, DeadReason::OnlyWrittenNeverRead),
        "Expected OnlyWrittenNeverRead, got: {:?}",
        dead[0].reason
    );
}

// ===========================================================================
// Category 3: Initialization Tracking (5 tests)
// ===========================================================================

#[test]
fn test_full_initialization() {
    // All bytes initialized before read → no violations.
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));
    input.add_event(write_event(1, 2, 1));     // writes all bytes
    input.add_event(read_event(1, 3, 1));      // reads all bytes
    input.add_event(dealloc_event(1, 4, 1));
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);

    // Init map: full coverage [0, 8)
    let mut init_map = InitializationMap::new();
    init_map.mark_initialized(1, 0, 8);

    let context = LivenessVerificationContext::with_init_map(input, init_map);
    let verifier = LivenessVerifier::new();
    let violations = verifier.check_partial_initialization(&context);

    assert!(
        violations.is_empty(),
        "Expected no partial init violations for fully initialized region, got: {:?}",
        violations
    );
}

#[test]
fn test_partial_initialization() {
    // Some bytes uninitialized: init map covers [0,4) and [8,12),
    // leaving gap at [4,8). A Read event triggers the check.
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));
    input.add_event(write_event(1, 2, 1));
    input.add_event(read_event(1, 3, 1));
    input.add_event(dealloc_event(1, 4, 1));
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);

    // Init map: partial coverage with a gap
    let mut init_map = InitializationMap::new();
    init_map.mark_initialized(1, 0, 4);   // bytes 0-4 initialized
    init_map.mark_initialized(1, 8, 12);  // bytes 8-12 initialized
    // Gap: bytes 4-8 are NOT initialized

    let context = LivenessVerificationContext::with_init_map(input, init_map);
    let verifier = LivenessVerifier::new();
    let violations = verifier.check_partial_initialization(&context);

    assert_eq!(violations.len(), 1, "Expected 1 partial init violation");
    let v = &violations[0];
    assert_eq!(v.region_id, 1);
    assert!(
        v.uninitialized_ranges.iter().any(|&(s, e)| s <= 4 && e >= 8),
        "Expected uninitialized gap covering bytes 4-8, got: {:?}",
        v.uninitialized_ranges
    );
}

#[test]
fn test_struct_field_initialization() {
    // Struct with two fields initialized, padding between them is not.
    // Field 1: bytes [0, 4), padding: bytes [4, 8), Field 2: bytes [8, 12)
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));
    input.add_event(write_event(1, 2, 1));   // write struct fields
    input.add_event(read_event(1, 3, 1));    // read struct (all fields)
    input.add_event(dealloc_event(1, 4, 1));
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);

    // Init map: two struct fields initialized, padding is not
    let mut init_map = InitializationMap::new();
    init_map.mark_initialized(1, 0, 4);   // field 1
    init_map.mark_initialized(1, 8, 12);  // field 2
    // bytes 4-8 (padding) NOT initialized

    let context = LivenessVerificationContext::with_init_map(input, init_map);
    let verifier = LivenessVerifier::new();
    let violations = verifier.check_partial_initialization(&context);

    assert_eq!(violations.len(), 1);
    let v = &violations[0];
    // The uninitialized range should include the padding gap
    assert!(
        v.uninitialized_ranges.contains(&(4, 8)),
        "Expected uninitialized gap at struct padding [4,8), got: {:?}",
        v.uninitialized_ranges
    );
}

#[test]
fn test_array_element_initialization() {
    // Array of 4 elements, each 4 bytes (total 16 bytes).
    // Elements 0 and 2 are initialized, elements 1 and 3 are not.
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));
    input.add_event(write_event(1, 2, 1));   // partial writes
    input.add_event(read_event(1, 3, 1));    // read full array
    input.add_event(dealloc_event(1, 4, 1));
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);

    // Init map: elements 0, 2, 4 initialized (out of 5 elements, each 4 bytes)
    // This creates two gaps: element 1 [4,8) and element 3 [12,16)
    let mut init_map = InitializationMap::new();
    init_map.mark_initialized(1, 0, 4);    // element 0: [0,4)
    init_map.mark_initialized(1, 8, 12);   // element 2: [8,12)
    init_map.mark_initialized(1, 16, 20);  // element 4: [16,20)
    // element 1: [4,8) NOT initialized
    // element 3: [12,16) NOT initialized

    let context = LivenessVerificationContext::with_init_map(input, init_map);
    let verifier = LivenessVerifier::new();
    let violations = verifier.check_partial_initialization(&context);

    assert_eq!(violations.len(), 1);
    let v = &violations[0];
    // Should have two uninitialized ranges: [4,8) and [12,16)
    assert_eq!(
        v.uninitialized_ranges.len(),
        2,
        "Expected 2 uninitialized gaps for array elements 1 and 3, got: {:?}",
        v.uninitialized_ranges
    );
    assert!(
        v.uninitialized_ranges.contains(&(4, 8)),
        "Expected gap at element 1 [4,8)"
    );
    assert!(
        v.uninitialized_ranges.contains(&(12, 16)),
        "Expected gap at element 3 [12,16)"
    );
}

#[test]
fn test_initialization_after_multiple_writes() {
    // Multiple writes that together cover the full region.
    // Write 1: bytes [0,4), Write 2: bytes [4,8). Together they cover [0,8).
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));
    input.add_event(write_event(1, 2, 1));   // first write
    input.add_event(write_event(1, 3, 1));   // second write
    input.add_event(read_event(1, 4, 1));    // read after all writes
    input.add_event(dealloc_event(1, 5, 1));
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5]);

    // Init map: two writes covering the full region contiguously
    let mut init_map = InitializationMap::new();
    init_map.mark_initialized(1, 0, 4);   // first write covers [0,4)
    init_map.mark_initialized(1, 4, 8);   // second write covers [4,8)

    let context = LivenessVerificationContext::with_init_map(input, init_map);
    let verifier = LivenessVerifier::new();
    let violations = verifier.check_partial_initialization(&context);

    assert!(
        violations.is_empty(),
        "Expected no violations when multiple writes cover full region, got: {:?}",
        violations
    );
}

// ===========================================================================
// Category 4: Proof Obligations (5 tests)
// ===========================================================================

#[test]
fn test_proof_obligation_for_uaf() {
    // Use-after-free generates a UseAfterFreeSafe proof obligation.
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));
    input.add_event(dealloc_event(1, 2, 1));
    input.add_event(read_event(1, 3, 1));     // read after free!
    input.cfg_edges = linear_cfg(&[1, 2, 3]);
    input.entry_point = Some(pp(1));

    let context = LivenessVerificationContext::new(input);
    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify_with_proofs(&context);

    let uaf_obligations: Vec<&ProofObligation> = result
        .proof_obligations
        .iter()
        .filter(|o| o.obligation_kind == ObligationKind::UseAfterFreeSafe)
        .collect();

    assert!(
        !uaf_obligations.is_empty(),
        "Expected UseAfterFreeSafe proof obligation, got: {:?}",
        result.proof_obligations
    );
    // The obligation should reference resource 1
    assert_eq!(uaf_obligations[0].resource, rid(1));
    assert!(
        uaf_obligations[0].description.contains("use-after-free"),
        "Obligation description should mention use-after-free: {}",
        uaf_obligations[0].description
    );
}

#[test]
fn test_proof_obligation_for_dead_alloc() {
    // Dead allocation (never accessed) generates DeadAllocationNeeded obligation.
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));
    input.add_event(dealloc_event(1, 2, 1));   // no read/write between alloc and dealloc
    input.cfg_edges = linear_cfg(&[1, 2]);
    input.entry_point = Some(pp(1));

    let context = LivenessVerificationContext::new(input);
    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify_with_proofs(&context);

    let dead_obligations: Vec<&ProofObligation> = result
        .proof_obligations
        .iter()
        .filter(|o| o.obligation_kind == ObligationKind::DeadAllocationNeeded)
        .collect();

    assert!(
        !dead_obligations.is_empty(),
        "Expected DeadAllocationNeeded proof obligation, got: {:?}",
        result.proof_obligations
    );
    assert_eq!(dead_obligations[0].resource, rid(1));
}

#[test]
fn test_proof_obligation_for_uninit() {
    // Uninitialized read generates a FullyInitialized proof obligation.
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));
    input.add_event(write_event(1, 2, 1));     // partial write
    input.add_event(read_event(1, 3, 1));      // read with partial init
    input.add_event(dealloc_event(1, 4, 1));
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);
    input.entry_point = Some(pp(1));

    // Init map with gap: bytes [0,4) initialized, but gap at [4,8)
    let mut init_map = InitializationMap::new();
    init_map.mark_initialized(1, 0, 4);
    init_map.mark_initialized(1, 8, 12);

    let context = LivenessVerificationContext::with_init_map(input, init_map);
    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify_with_proofs(&context);

    let init_obligations: Vec<&ProofObligation> = result
        .proof_obligations
        .iter()
        .filter(|o| o.obligation_kind == ObligationKind::FullyInitialized)
        .collect();

    assert!(
        !init_obligations.is_empty(),
        "Expected FullyInitialized proof obligation, got: {:?}",
        result.proof_obligations
    );
    assert_eq!(init_obligations[0].resource, rid(1));
    assert!(
        init_obligations[0].description.contains("fully initialized"),
        "Obligation description should mention initialization: {}",
        init_obligations[0].description
    );
}

#[test]
fn test_no_proof_obligations_for_safe() {
    // When everything is safe (alloc→write→read→free, full init, no UAF,
    // no dead allocs), verify_with_proofs should produce no enhanced
    // obligations (UseAfterFreeSafe, DeadAllocationNeeded, FullyInitialized).
    let mut input = LivenessInput::new();
    input.add_event(alloc_event(1, 1, 1));
    input.add_event(write_event(1, 2, 1));
    input.add_event(read_event(1, 3, 1));
    input.add_event(dealloc_event(1, 4, 1));
    input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);
    input.entry_point = Some(pp(1));

    // Full initialization coverage
    let mut init_map = InitializationMap::new();
    init_map.mark_initialized(1, 0, 8);

    let context = LivenessVerificationContext::with_init_map(input, init_map);
    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify_with_proofs(&context);

    // No violations
    assert!(
        result.invariant_holds,
        "Expected invariant to hold for safe program, got violations: {:?}",
        result.violations
    );
    assert!(result.violations.is_empty());

    // No enhanced proof obligations
    let enhanced_kinds = [
        ObligationKind::UseAfterFreeSafe,
        ObligationKind::DeadAllocationNeeded,
        ObligationKind::FullyInitialized,
    ];
    let enhanced_obligations: Vec<&ProofObligation> = result
        .proof_obligations
        .iter()
        .filter(|o| enhanced_kinds.contains(&o.obligation_kind))
        .collect();

    assert!(
        enhanced_obligations.is_empty(),
        "Expected no enhanced proof obligations for safe program, got: {:?}",
        enhanced_obligations
    );
}

#[test]
fn test_multiple_proof_obligations() {
    // Multiple issues across different resources generate multiple obligations:
    //   R1: use-after-free (alloc→free→read)
    //   R2: dead allocation (alloc→free, no access)
    //   R3: partial initialization (alloc, partial write, read, free)
    let mut input = LivenessInput::new();

    // R1: use-after-free
    input.add_event(alloc_event(1, 1, 1));
    input.add_event(dealloc_event(1, 2, 1));
    input.add_event(read_event(1, 3, 1));

    // R2: dead allocation (never accessed)
    input.add_event(alloc_event(2, 4, 1));
    input.add_event(dealloc_event(2, 5, 1));

    // R3: partial initialization
    input.add_event(alloc_event(3, 6, 1));
    input.add_event(write_event(3, 7, 1));
    input.add_event(read_event(3, 8, 1));
    input.add_event(dealloc_event(3, 9, 1));

    input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    input.entry_point = Some(pp(1));

    // Init map: R3 has partial coverage
    let mut init_map = InitializationMap::new();
    init_map.mark_initialized(3, 0, 4);
    init_map.mark_initialized(3, 8, 12);
    // Gap at [4,8) for R3

    let context = LivenessVerificationContext::with_init_map(input, init_map);
    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify_with_proofs(&context);

    // Check UseAfterFreeSafe obligation for R1
    let uaf_obligations: Vec<&ProofObligation> = result
        .proof_obligations
        .iter()
        .filter(|o| o.obligation_kind == ObligationKind::UseAfterFreeSafe)
        .collect();
    assert!(
        !uaf_obligations.is_empty(),
        "Expected UseAfterFreeSafe obligation for R1"
    );
    assert!(
        uaf_obligations.iter().any(|o| o.resource == rid(1)),
        "UseAfterFreeSafe obligation should reference R1"
    );

    // Check DeadAllocationNeeded obligation for R2
    let dead_obligations: Vec<&ProofObligation> = result
        .proof_obligations
        .iter()
        .filter(|o| o.obligation_kind == ObligationKind::DeadAllocationNeeded)
        .collect();
    assert!(
        !dead_obligations.is_empty(),
        "Expected DeadAllocationNeeded obligation for R2"
    );
    assert!(
        dead_obligations.iter().any(|o| o.resource == rid(2)),
        "DeadAllocationNeeded obligation should reference R2"
    );

    // Check FullyInitialized obligation for R3
    let init_obligations: Vec<&ProofObligation> = result
        .proof_obligations
        .iter()
        .filter(|o| o.obligation_kind == ObligationKind::FullyInitialized)
        .collect();
    assert!(
        !init_obligations.is_empty(),
        "Expected FullyInitialized obligation for R3"
    );
    assert!(
        init_obligations.iter().any(|o| o.resource == rid(3)),
        "FullyInitialized obligation should reference R3"
    );

    // Total: at least 3 distinct obligation kinds
    let distinct_kinds: Vec<ObligationKind> = result
        .proof_obligations
        .iter()
        .map(|o| o.obligation_kind)
        .collect();
    assert!(
        distinct_kinds.len() >= 3,
        "Expected at least 3 distinct obligation kinds, got: {:?}",
        distinct_kinds
    );
}
