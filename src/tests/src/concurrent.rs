//! Concurrent access tests
//!
//! Tests for memory safety under concurrent access patterns,
//! covering shared reads, read-write conflicts, mutex protection,
//! and lock-free data structures. Each test builds verifier inputs
//! directly using the per-invariant IVE APIs.

use vuma_ive::exclusivity::{
    AccessId as ExclusivityAccessId, AccessKind as ExclusivityAccessKind, AccessRecord, CapDInfo,
    ExclusivityInput, ExclusivityVerifier, SyncEdgeRecord, SyncOrdering,
};
use vuma_ive::liveness::{
    ControlFlowEdge, EventAction, LivenessInput, LivenessVerifier, PointId, ResourceEvent,
    ResourceId as LivenessResourceId, ResourceKind as LivenessResourceKind, ThreadId,
};

/// Test: two concurrent reads of the same region → should prove safe.
///
/// Multiple concurrent reads of a shared memory region are inherently
/// safe because no mutation occurs. The exclusivity verifier should
/// prove that concurrent reads never conflict.
#[test]
fn test_two_reads_same_region() {
    let mut input = ExclusivityInput::new();

    // Two reads to the same address on different threads, no sync edge.
    input.add_access(AccessRecord::new(
        ExclusivityAccessId(1),
        ExclusivityAccessKind::Read,
        0x1000,
        8,
        "concurrent.vu:1".to_string(),
        1,
        1,
    ));
    input.add_access(AccessRecord::new(
        ExclusivityAccessId(2),
        ExclusivityAccessKind::Read,
        0x1000,
        8,
        "concurrent.vu:2".to_string(),
        2,
        1,
    ));
    // No sync edges — these are truly concurrent reads.

    let verifier = ExclusivityVerifier::new();
    let output = verifier.verify(&input);
    assert!(
        output.result.is_proven(),
        "Exclusivity should be Proven for two concurrent reads (reads never conflict), got: {:?}",
        output.result.status
    );
    assert_eq!(
        output.conflict_count(),
        0,
        "No conflicts should be detected for concurrent reads"
    );
}

/// Test: concurrent read + write to the same region → should flag exclusivity violation.
///
/// A concurrent read and write to the same memory region creates a
/// data race. The exclusivity verifier should detect this as a
/// write-read conflict since the write requires exclusive access but
/// the read holds shared access.
#[test]
fn test_read_write_same_region() {
    let mut input = ExclusivityInput::new();

    // Write and Read to the same address, no sync edge.
    input.add_access(AccessRecord::new(
        ExclusivityAccessId(1),
        ExclusivityAccessKind::Write,
        0x1000,
        8,
        "concurrent.vu:1".to_string(),
        1,
        1,
    ));
    input.add_access(AccessRecord::new(
        ExclusivityAccessId(2),
        ExclusivityAccessKind::Read,
        0x1000,
        8,
        "concurrent.vu:2".to_string(),
        2,
        1,
    ));
    // No sync edges — these are truly concurrent.

    let verifier = ExclusivityVerifier::new();
    let output = verifier.verify(&input);
    assert!(
        output.result.is_violated(),
        "Exclusivity should be Violated for concurrent read+write (data race), got: {:?}",
        output.result.status
    );
    assert!(
        output.write_read_count() > 0,
        "Expected at least one write-read conflict, got: {}",
        output.write_read_count()
    );
}

/// Test: mutex-protected access → should prove safe or probably safe.
///
/// When a mutex guards access to a shared region, the mutual
/// exclusion guarantees that reads and writes cannot occur
/// simultaneously. The exclusivity verifier should recognize the
/// mutex protection via CapD conditions and report ProbablySafe
/// or Proven.
#[test]
fn test_mutex_protected_access() {
    let mut input = ExclusivityInput::new();

    // Write and Read to the same address, both protected by mutex 42.
    input.add_access(AccessRecord::new(
        ExclusivityAccessId(1),
        ExclusivityAccessKind::Write,
        0x1000,
        8,
        "concurrent.vu:1".to_string(),
        1,
        1,
    ));
    input.add_access(AccessRecord::new(
        ExclusivityAccessId(2),
        ExclusivityAccessKind::Read,
        0x1000,
        8,
        "concurrent.vu:2".to_string(),
        2,
        1,
    ));
    // No sync edges — they are concurrent, BUT both are protected by same mutex.
    input.set_capability(ExclusivityAccessId(1), CapDInfo::write_locked(42));
    input.set_capability(ExclusivityAccessId(2), CapDInfo::write_locked(42));

    let verifier = ExclusivityVerifier::new();
    let output = verifier.verify(&input);

    // The verifier should recognize mutex protection and report ProbablySafe or Proven.
    let is_safe = output.result.is_proven()
        || matches!(
            output.result.status,
            vuma_ive::result::VerificationStatus::ProbablySafe { .. }
        );
    assert!(
        is_safe,
        "Exclusivity should be ProbablySafe or Proven for mutex-protected access, got: {:?}",
        output.result.status
    );
}

/// Test: single producer, single consumer ring buffer → should prove safe.
///
/// A lock-free SPSC ring buffer is safe because the producer only
/// writes to the tail and the consumer only reads from the head.
/// The happens-before edges ensure correct ordering, so no data race
/// exists despite the absence of locks.
#[test]
fn test_lock_free_ring_buffer() {
    let mut input = ExclusivityInput::new();

    // Producer writes to tail slot (address 0x2000)
    input.add_access(AccessRecord::new(
        ExclusivityAccessId(1),
        ExclusivityAccessKind::Write,
        0x2000,
        8,
        "concurrent.vu:5".to_string(),
        1,
        2,
    ));
    // Consumer reads from head slot (address 0x3000) — different address, no overlap
    input.add_access(AccessRecord::new(
        ExclusivityAccessId(2),
        ExclusivityAccessKind::Read,
        0x3000,
        8,
        "concurrent.vu:10".to_string(),
        2,
        3,
    ));
    // Producer also writes to shared counter (address 0x4000)
    input.add_access(AccessRecord::new(
        ExclusivityAccessId(3),
        ExclusivityAccessKind::Write,
        0x4000,
        4,
        "concurrent.vu:6".to_string(),
        3,
        4,
    ));
    // Consumer reads shared counter (address 0x4000) — same address as producer's counter write
    input.add_access(AccessRecord::new(
        ExclusivityAccessId(4),
        ExclusivityAccessKind::Read,
        0x4000,
        4,
        "concurrent.vu:11".to_string(),
        4,
        4,
    ));
    // HappensBefore: producer's counter write HB consumer's counter read
    input.add_sync_edge(SyncEdgeRecord::new(
        ExclusivityAccessId(3),
        ExclusivityAccessId(4),
        SyncOrdering::HappensBefore,
    ));

    let verifier = ExclusivityVerifier::new();
    let output = verifier.verify(&input);
    assert!(
        output.result.is_proven(),
        "Exclusivity should be Proven for SPSC ring buffer with HB edges, got: {:?}",
        output.result.status
    );

    // Also verify liveness — no resource leaks
    let mut liveness_input = LivenessInput::new();
    let buf_res = LivenessResourceId(1);
    liveness_input.add_event(ResourceEvent {
        resource: buf_res,
        kind: LivenessResourceKind::Memory,
        event: EventAction::Allocate,
        point: PointId(1),
        thread: ThreadId(0),
    });
    liveness_input.add_event(ResourceEvent {
        resource: buf_res,
        kind: LivenessResourceKind::Memory,
        event: EventAction::Deallocate,
        point: PointId(2),
        thread: ThreadId(0),
    });
    liveness_input.add_cfg_edge(ControlFlowEdge {
        from: PointId(1),
        to: PointId(2),
        conditional: false,
        label: None,
    });

    let mut liveness_verifier = LivenessVerifier::new();
    let liveness_result = liveness_verifier.verify(&liveness_input);
    assert!(
        liveness_result.invariant_holds,
        "Liveness should hold for SPSC ring buffer, got violations: {:?}",
        liveness_result.violations
    );
}
