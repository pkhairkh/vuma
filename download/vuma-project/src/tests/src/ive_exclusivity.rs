//! Integration tests for the ExclusivityVerifier (IVE module).
//!
//! Comprehensive test suite covering:
//! - Basic alias detection (write-write, write-read, read-read, partial overlap)
//! - Sync edge handling (HappensBefore, Atomic, Mutex, transitive ordering)
//! - CapD lattice integration (read-only, locked, unlocked, meet, join)
//! - Interference graph analysis (construction, components, clustering, display)
//! - Complex scenarios (multiple resources, cycles, large addresses, zero-size, mixed orderings)

use vuma_ive::{
    AccessRecord, CapDInfo, ConflictKind, ExclusivityAccessId as AccessId,
    ExclusivityAccessKind as AccessKind, ExclusivityInput, ExclusivityOutput,
    ExclusivityVerifier, SyncEdgeRecord, SyncOrdering, VerificationStatus,
};

/// Helper: create a ProgramPoint from a string.
fn pp(s: &str) -> String {
    s.to_string()
}

/// Helper: create a write access record.
fn write_access(id: u64, addr: u64, size: u64, point: &str) -> AccessRecord {
    AccessRecord::new(AccessId(id), AccessKind::Write, addr, size, pp(point), id, id)
}

/// Helper: create a read access record.
fn read_access(id: u64, addr: u64, size: u64, point: &str) -> AccessRecord {
    AccessRecord::new(AccessId(id), AccessKind::Read, addr, size, pp(point), id, id)
}

/// Helper: verify an ExclusivityInput and return the output.
fn verify(input: &ExclusivityInput) -> ExclusivityOutput {
    ExclusivityVerifier::new().verify(input)
}

// ===========================================================================
// Category 1: Basic Alias Detection (5 tests)
// ===========================================================================

#[test]
fn test_two_writes_same_address() {
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));

    let output = verify(&input);

    assert!(output.is_violated(), "Two concurrent writes to same address should be Violated");
    assert_eq!(output.write_write_count(), 1, "Expected exactly 1 WriteWrite conflict");
    assert_eq!(output.conflict_count(), 1);
    assert!(matches!(output.conflicts[0].kind, ConflictKind::WriteWrite));
}

#[test]
fn test_write_read_same_address() {
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(read_access(2, 0x1000, 4, "test.vu:2"));

    let output = verify(&input);

    assert!(output.is_violated(), "Concurrent write+read to same address should be Violated");
    assert_eq!(output.write_read_count(), 1, "Expected exactly 1 WriteRead conflict");
    assert_eq!(output.conflict_count(), 1);
    assert!(matches!(output.conflicts[0].kind, ConflictKind::WriteRead));
}

#[test]
fn test_two_reads_same_address() {
    let mut input = ExclusivityInput::new();
    input.add_access(read_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(read_access(2, 0x1000, 4, "test.vu:2"));

    let output = verify(&input);

    assert!(output.is_proven(), "Concurrent reads to same address should be Proven");
    assert_eq!(output.conflict_count(), 0);
}

#[test]
fn test_non_overlapping_writes() {
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x2000, 4, "test.vu:2"));

    let output = verify(&input);

    assert!(output.is_proven(), "Non-overlapping writes should be Proven");
    assert_eq!(output.conflict_count(), 0);
}

#[test]
fn test_partial_overlap() {
    let mut input = ExclusivityInput::new();
    // Write to [0x1000, 0x1008)
    input.add_access(write_access(1, 0x1000, 8, "test.vu:1"));
    // Write to [0x1004, 0x100C) — overlaps 4 bytes with the first
    input.add_access(write_access(2, 0x1004, 8, "test.vu:2"));

    let output = verify(&input);

    assert!(output.is_violated(), "Partially overlapping writes should be Violated");
    assert_eq!(output.write_write_count(), 1);
    assert_eq!(output.conflict_count(), 1);
    // Overlap range should be [0x1004, 0x1008)
    assert_eq!(output.conflicts[0].overlap_start, 0x1004);
    assert_eq!(output.conflicts[0].overlap_end, 0x1008);
}

// ===========================================================================
// Category 2: Sync Edge Handling (5 tests)
// ===========================================================================

#[test]
fn test_happens_before_ordering() {
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(read_access(2, 0x1000, 4, "test.vu:2"));
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(1), AccessId(2), SyncOrdering::HappensBefore));

    let output = verify(&input);

    assert!(output.is_proven(), "Write→Read with HappensBefore should be Proven");
    assert_eq!(output.conflict_count(), 0);
}

#[test]
fn test_atomic_ordering() {
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(read_access(2, 0x1000, 4, "test.vu:2"));
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(1), AccessId(2), SyncOrdering::Atomic));

    let output = verify(&input);

    assert!(output.is_proven(), "Write→Read with Atomic ordering should be Proven");
    assert_eq!(output.conflict_count(), 0);
}

#[test]
fn test_mutex_protection() {
    // Two writes to the same address, both with CapD locked by the same mutex.
    // The verifier treats same-lock CapD conditions as "probably safe" because
    // mutual exclusion is assumed but not formally proven.
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));
    input.set_capability(AccessId(1), CapDInfo::write_locked(1));
    input.set_capability(AccessId(2), CapDInfo::write_locked(1));

    let output = verify(&input);

    assert!(
        matches!(output.result.status, VerificationStatus::ProbablySafe { .. }),
        "Two writes protected by same mutex CapD should be ProbablySafe"
    );
    assert_eq!(output.conflict_count(), 1, "Conflict should be recorded but marked as lock-protected");
}

#[test]
fn test_different_mutexes() {
    // Two writes to the same address, each locked by a different mutex.
    // Different mutexes do NOT provide mutual exclusion between each other.
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));
    input.set_capability(AccessId(1), CapDInfo::write_locked(1));
    input.set_capability(AccessId(2), CapDInfo::write_locked(2));

    let output = verify(&input);

    assert!(output.is_violated(), "Two writes under different mutexes should be Violated");
    assert_eq!(output.write_write_count(), 1);
}

#[test]
fn test_transitive_ordering() {
    // A→B→C sync chain. Write at A, Read at C. Transitive closure should
    // establish that A is ordered before C, so no conflict.
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1")); // A1 = Write
    input.add_access(read_access(2, 0x2000, 4, "test.vu:2"));  // A2 = dummy (different addr)
    input.add_access(read_access(3, 0x1000, 4, "test.vu:3"));  // A3 = Read at same addr as A1

    // Sync chain: A1 → A2 → A3
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(1), AccessId(2), SyncOrdering::HappensBefore));
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(2), AccessId(3), SyncOrdering::HappensBefore));

    let output = verify(&input);

    assert!(output.is_proven(), "Transitive HappensBefore A1→A2→A3 should make A1 ordered before A3");
    assert_eq!(output.conflict_count(), 0);
}

// ===========================================================================
// Category 3: CapD Lattice Integration (5 tests)
// ===========================================================================

#[test]
fn test_read_only_capd() {
    // Two Write-kind accesses whose CapD overrides say read-only.
    // The CapD takes precedence: access_has_write_capability returns false
    // when CapD.can_write is false, even though the access kind is Write.
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));
    input.set_capability(AccessId(1), CapDInfo::read_only());
    input.set_capability(AccessId(2), CapDInfo::read_only());

    let output = verify(&input);

    assert!(
        output.is_proven(),
        "Two Write-kind accesses with read-only CapD should be Proven (CapD overrides kind)"
    );
    assert_eq!(output.conflict_count(), 0);
}

#[test]
fn test_write_locked_capd() {
    // Two writes to the same address, both with CapD locked by mutex 1.
    // This demonstrates CapD-based mutex protection: the conflict is
    // recorded but treated as "probably safe" because the same mutex
    // guarantees mutual exclusion at runtime.
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 8, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 8, "test.vu:2"));
    input.set_capability(AccessId(1), CapDInfo::write_locked(1));
    input.set_capability(AccessId(2), CapDInfo::write_locked(1));

    let output = verify(&input);

    assert!(
        matches!(output.result.status, VerificationStatus::ProbablySafe { .. }),
        "Two writes with CapD locked by same mutex should be ProbablySafe"
    );
    assert_eq!(output.conflict_count(), 1, "Conflict should be recorded as lock-protected");
    assert!(matches!(output.conflicts[0].kind, ConflictKind::WriteWrite));
}

#[test]
fn test_write_unlocked_capd() {
    // Two writes to the same address, one with CapD (write without lock
    // condition) and one without CapD. Since neither has write_requires_lock
    // pointing to the same mutex, both_protected_by_same_lock returns false.
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));
    // Only one access has CapD info; the other has none.
    input.set_capability(AccessId(1), CapDInfo::write_only());

    let output = verify(&input);

    assert!(output.is_violated(), "Unlocked write CapD without same-lock protection should be Violated");
    assert_eq!(output.write_write_count(), 1);
}

#[test]
fn test_capd_meet_in_exclusivity() {
    // Two writes whose CapD meet (intersection) preserves write capability
    // under the same mutex. Both have write_locked(1); their meet has
    // can_write=true and write_requires_lock=Some(1), confirming they
    // are compatible under the same mutex.
    let cap1 = CapDInfo::write_locked(1);
    let cap2 = CapDInfo::write_locked(1);
    let meet = cap1.meet(&cap2);
    assert!(meet.has_write(), "Meet of two write_locked(1) should have write capability");
    assert_eq!(meet.write_requires_lock, Some(1), "Meet should preserve same lock condition");

    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));
    input.set_capability(AccessId(1), cap1);
    input.set_capability(AccessId(2), cap2);

    let output = verify(&input);

    assert!(
        matches!(output.result.status, VerificationStatus::ProbablySafe { .. }),
        "Compatible CapDs (same lock) should yield ProbablySafe"
    );
}

#[test]
fn test_capd_join_in_exclusivity() {
    // Two writes whose CapDs join to include Write unconditionally.
    // Cap1: write_only (can_write=true, no lock)
    // Cap2: read_write (can_write=true, no lock)
    // Their join: can_write=true, write_requires_lock=None → unconditional Write
    // No lock protection → Violated.
    let cap1 = CapDInfo::write_only();
    let cap2 = CapDInfo::read_write();
    let join = cap1.join(&cap2);
    assert!(join.has_write(), "Join should include Write capability");
    assert_eq!(join.write_requires_lock, None, "Join should have no lock condition");

    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));
    input.set_capability(AccessId(1), cap1);
    input.set_capability(AccessId(2), cap2);

    let output = verify(&input);

    assert!(output.is_violated(), "Join includes Write unconditionally → no lock protection → Violated");
    assert_eq!(output.write_write_count(), 1);
}

// ===========================================================================
// Category 4: Interference Graph Analysis (5 tests)
// ===========================================================================

#[test]
fn test_interference_graph_construction() {
    // Build 3 independent conflict pairs and verify the interference graph.
    let mut input = ExclusivityInput::new();
    // Pair 1: overlapping writes at 0x1000
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));
    // Pair 2: overlapping writes at 0x2000
    input.add_access(write_access(3, 0x2000, 4, "test.vu:3"));
    input.add_access(write_access(4, 0x2000, 4, "test.vu:4"));
    // Pair 3: overlapping writes at 0x3000
    input.add_access(write_access(5, 0x3000, 4, "test.vu:5"));
    input.add_access(write_access(6, 0x3000, 4, "test.vu:6"));

    let output = verify(&input);

    assert_eq!(output.interference_graph.conflict_count(), 3, "Expected 3 conflicts");
    assert_eq!(output.interference_graph.node_count(), 6, "Expected 6 nodes in interference graph");

    // Verify each pair is conflicting
    assert!(output.interference_graph.are_conflicting(AccessId(1), AccessId(2)));
    assert!(output.interference_graph.are_conflicting(AccessId(3), AccessId(4)));
    assert!(output.interference_graph.are_conflicting(AccessId(5), AccessId(6)));

    // Verify non-pairs are NOT conflicting
    assert!(!output.interference_graph.are_conflicting(AccessId(1), AccessId(3)));
    assert!(!output.interference_graph.are_conflicting(AccessId(2), AccessId(4)));
}

#[test]
fn test_connected_components() {
    // 4 accesses forming 2 disconnected components:
    //   Component 1: A1↔A2 (conflict at 0x1000)
    //   Component 2: A3↔A4 (conflict at 0x2000)
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));
    input.add_access(write_access(3, 0x2000, 4, "test.vu:3"));
    input.add_access(write_access(4, 0x2000, 4, "test.vu:4"));

    let output = verify(&input);

    let components = output.interference_graph.connected_components();
    assert_eq!(components.len(), 2, "Expected 2 connected components");

    // Each component should have 2 nodes
    let mut sizes: Vec<usize> = components.iter().map(|c| c.len()).collect();
    sizes.sort();
    assert_eq!(sizes, vec![2, 2]);
}

#[test]
fn test_no_conflicts_empty_graph() {
    // Non-conflicting accesses should produce an empty interference graph.
    let mut input = ExclusivityInput::new();
    input.add_access(read_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(read_access(2, 0x1000, 4, "test.vu:2"));
    input.add_access(write_access(3, 0x2000, 4, "test.vu:3"));
    input.add_access(write_access(4, 0x3000, 4, "test.vu:4"));

    let output = verify(&input);

    assert!(output.interference_graph.is_empty(), "No conflicts should yield empty interference graph");
    assert_eq!(output.interference_graph.conflict_count(), 0);
    assert_eq!(output.interference_graph.node_count(), 0);
}

#[test]
fn test_conflict_clustering() {
    // 6 accesses forming 2 clusters of 3:
    //   Cluster A: A1, A2, A3 all write to 0x1000 (3 pairwise conflicts)
    //   Cluster B: A4, A5, A6 all write to 0x2000 (3 pairwise conflicts)
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));
    input.add_access(write_access(3, 0x1000, 4, "test.vu:3"));
    input.add_access(write_access(4, 0x2000, 4, "test.vu:4"));
    input.add_access(write_access(5, 0x2000, 4, "test.vu:5"));
    input.add_access(write_access(6, 0x2000, 4, "test.vu:6"));

    let output = verify(&input);

    // 3 pairwise conflicts per cluster = 6 total
    assert_eq!(output.interference_graph.conflict_count(), 6);
    assert_eq!(output.interference_graph.node_count(), 6);

    // 2 connected components (the two clusters)
    let components = output.interference_graph.connected_components();
    assert_eq!(components.len(), 2);

    // Each component should have 3 nodes
    let mut sizes: Vec<usize> = components.iter().map(|c| c.len()).collect();
    sizes.sort();
    assert_eq!(sizes, vec![3, 3]);

    // Verify cluster A edges
    assert!(output.interference_graph.are_conflicting(AccessId(1), AccessId(2)));
    assert!(output.interference_graph.are_conflicting(AccessId(1), AccessId(3)));
    assert!(output.interference_graph.are_conflicting(AccessId(2), AccessId(3)));

    // Verify cluster B edges
    assert!(output.interference_graph.are_conflicting(AccessId(4), AccessId(5)));
    assert!(output.interference_graph.are_conflicting(AccessId(4), AccessId(6)));
    assert!(output.interference_graph.are_conflicting(AccessId(5), AccessId(6)));

    // No cross-cluster edges
    assert!(!output.interference_graph.are_conflicting(AccessId(1), AccessId(4)));
}

#[test]
fn test_interference_graph_display() {
    // Build a known interference graph and check its Display format.
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));
    input.add_access(write_access(3, 0x2000, 4, "test.vu:3"));
    input.add_access(write_access(4, 0x2000, 4, "test.vu:4"));

    let output = verify(&input);

    let display = format!("{}", output.interference_graph);
    assert_eq!(display, "InterferenceGraph { nodes: 4, edges: 2 }");
}

// ===========================================================================
// Category 5: Complex Scenarios (5 tests)
// ===========================================================================

#[test]
fn test_multiple_resources() {
    // 3 resources, 8 accesses, mixed safe/unsafe patterns:
    //   Resource 1 (0x1000): A1=Write, A2=Write → conflict (WriteWrite)
    //   Resource 2 (0x2000): A3=Read, A4=Read → safe (both reads)
    //   Resource 2 (0x2000): A5=Write, A6=Read → conflict (WriteRead)
    //   Resource 3 (0x3000): A7=Write, A8=Write with HappensBefore → safe (ordered)
    let mut input = ExclusivityInput::new();
    // Resource 1: conflicting writes
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));
    // Resource 2: safe reads
    input.add_access(read_access(3, 0x2000, 4, "test.vu:3"));
    input.add_access(read_access(4, 0x2000, 4, "test.vu:4"));
    // Resource 2: conflicting write+read
    input.add_access(write_access(5, 0x2000, 4, "test.vu:5"));
    input.add_access(read_access(6, 0x2000, 4, "test.vu:6"));
    // Resource 3: ordered writes
    input.add_access(write_access(7, 0x3000, 4, "test.vu:7"));
    input.add_access(write_access(8, 0x3000, 4, "test.vu:8"));
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(7), AccessId(8), SyncOrdering::HappensBefore));

    let output = verify(&input);

    assert!(output.is_violated(), "Should be Violated due to hard conflicts");
    // Conflicts: A1-A2 (WriteWrite), A5-A6 (WriteRead), A3-A5 (WriteRead), A4-A5 (WriteRead)
    // A3 and A4 are reads at 0x2000, A5 is a write at 0x2000 → A3-A5 and A4-A5 are conflicts
    // A6 is a read at 0x2000, A5 is a write → A5-A6 is a conflict
    // A6 also overlaps with A5 but A3, A4, A5, A6 are all at 0x2000
    // A3(Read) vs A5(Write) → conflict, A4(Read) vs A5(Write) → conflict, A5(Write) vs A6(Read) → conflict
    // Total: A1-A2, A3-A5, A4-A5, A5-A6 = 4 conflicts
    assert_eq!(output.conflict_count(), 4, "Expected 4 conflicts across mixed resources");
    assert_eq!(output.write_write_count(), 1, "Expected 1 WriteWrite conflict (A1-A2)");
    assert_eq!(output.write_read_count(), 3, "Expected 3 WriteRead conflicts (A3-A5, A4-A5, A5-A6)");
}

#[test]
fn test_cyclic_sync_edges() {
    // Sync edges forming a cycle: A1→A2, A2→A3, A3→A1.
    // The transitive closure should make all pairs ordered in both
    // directions, eliminating all conflicts.
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2")); // would conflict with A1
    input.add_access(read_access(3, 0x2000, 4, "test.vu:3"));  // dummy for cycle

    // Cyclic sync edges
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(1), AccessId(2), SyncOrdering::HappensBefore));
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(2), AccessId(3), SyncOrdering::HappensBefore));
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(3), AccessId(1), SyncOrdering::HappensBefore));

    let output = verify(&input);

    assert!(
        output.is_proven(),
        "Cyclic sync edges should make all accesses transitively ordered → Proven"
    );
    assert_eq!(output.conflict_count(), 0);
}

#[test]
fn test_large_address_space() {
    // Accesses at very different addresses — no overlaps.
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x0000_0000, 8, "test.vu:1"));
    input.add_access(write_access(2, 0x1000_0000, 8, "test.vu:2"));
    input.add_access(write_access(3, 0x8000_0000, 8, "test.vu:3"));
    input.add_access(write_access(4, 0xFFFF_FFF0, 8, "test.vu:4"));

    let output = verify(&input);

    assert!(output.is_proven(), "Writes at non-overlapping far-apart addresses should be Proven");
    assert_eq!(output.conflict_count(), 0);
}

#[test]
fn test_zero_size_access() {
    // An access with size 0 produces an empty byte range [addr, addr),
    // which does not overlap with any other range (including at the same
    // base address) because s_start == s_end causes the overlap check
    // s_start < o_end && o_start < s_end to fail when o_start >= s_start.
    let mut input = ExclusivityInput::new();
    input.add_access(write_access(1, 0x1000, 0, "test.vu:1")); // zero-size
    input.add_access(write_access(2, 0x1000, 4, "test.vu:2"));  // normal write at same addr

    let output = verify(&input);

    assert!(
        output.is_proven(),
        "Zero-size access should not conflict with overlapping access at same base address"
    );
    assert_eq!(output.conflict_count(), 0);
}

#[test]
fn test_mixed_ordering_types() {
    // A program using HappensBefore + Atomic + Mutex sync edges together.
    //
    // A1(Write@0x1000) --HappensBefore--> A2(Read@0x1000)  → ordered, safe
    // A3(Write@0x2000) --Atomic---------> A4(Read@0x2000)  → ordered, safe
    // A5(Write@0x3000) --Mutex(1)-------> A6(Write@0x3000) → ordered via sync edge, safe
    // A7(Write@0x4000)                   → no sync, no conflict (different addresses)
    let mut input = ExclusivityInput::new();
    // Pair 1: HappensBefore
    input.add_access(write_access(1, 0x1000, 4, "test.vu:1"));
    input.add_access(read_access(2, 0x1000, 4, "test.vu:2"));
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(1), AccessId(2), SyncOrdering::HappensBefore));

    // Pair 2: Atomic
    input.add_access(write_access(3, 0x2000, 4, "test.vu:3"));
    input.add_access(read_access(4, 0x2000, 4, "test.vu:4"));
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(3), AccessId(4), SyncOrdering::Atomic));

    // Pair 3: Mutex sync edge (creates ordering, so Proven not ProbablySafe)
    input.add_access(write_access(5, 0x3000, 4, "test.vu:5"));
    input.add_access(write_access(6, 0x3000, 4, "test.vu:6"));
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(5), AccessId(6), SyncOrdering::Mutex(1)));

    // Isolated write (no conflict with anything)
    input.add_access(write_access(7, 0x4000, 4, "test.vu:7"));

    let output = verify(&input);

    assert!(
        output.is_proven(),
        "All overlapping accesses are properly ordered via mixed sync types → Proven"
    );
    assert_eq!(output.conflict_count(), 0);
    assert!(output.interference_graph.is_empty());
}
