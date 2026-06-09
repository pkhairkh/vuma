//! Integration tests for the Concurrent Exclusivity Verification (IVE module).
//!
//! Comprehensive test suite covering:
//! - Happens-Before graph construction (spawn, join, transitive, independent, mutex)
//! - Data race detection (simple, sync-eliminated, read-write, read-read, multiple)
//! - Deadlock detection (lock order reversal, same order, three-lock cycle,
//!   multi-thread contention, single-thread safe)

use vuma_ive::{
    ConcurrentExclusivityInput, ConcurrentExclusivityOutput, ConcurrentExclusivityVerifier,
    ConcurrentThreadId as ThreadId, HappensBeforeGraph, HBRelation,
    ThreadAccess, VerificationStatus,
    ExclusivityAccessId as AccessId, ExclusivityAccessKind as AccessKind, AccessRecord,
    ConflictKind, SyncEdgeRecord, SyncOrdering,
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

/// Helper: verify a ConcurrentExclusivityInput and return the output.
fn verify(input: &ConcurrentExclusivityInput) -> ConcurrentExclusivityOutput {
    ConcurrentExclusivityVerifier::new().verify(input)
}

// ===========================================================================
// Category 1: Happens-Before (5 tests)
// ===========================================================================

#[test]
fn test_spawn_establishes_hb() {
    // Thread spawn creates a happens-before edge from the parent's accesses
    // to the child's accesses.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);

    // Parent writes, child writes to overlapping range.
    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 8, "spawn_hb:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x1004, 8, "spawn_hb:2"),
        t2.clone(),
    ));

    // t1 spawns t2 — parent's ops happen-before child's.
    input.add_spawn_edge(t1.clone(), t2.clone(), "spawn_point".into());

    // Verify the HB graph directly.
    let hb = HappensBeforeGraph::from_input(&input);
    assert!(
        hb.is_ordered(AccessId(1), AccessId(2)),
        "Spawn should create ordering from parent access to child access"
    );

    // No data race because spawn establishes HB.
    let output = verify(&input);
    assert!(
        output.is_race_free(),
        "Spawn edge should eliminate the race, but got {} races",
        output.race_count()
    );
}

#[test]
fn test_join_establishes_hb() {
    // Thread join creates a happens-before edge from the joinee's accesses
    // to the joiner's post-join accesses.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);

    // Joinee writes, joiner writes to overlapping range.
    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 8, "join_hb:1"),
        t2.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x1004, 8, "join_hb:2"),
        t1.clone(),
    ));

    // t1 joins t2 — joinee's ops happen-before joiner's.
    input.add_join_edge(t1.clone(), t2.clone(), "join_point".into());

    // Verify the HB graph directly.
    let hb = HappensBeforeGraph::from_input(&input);
    assert!(
        hb.is_ordered(AccessId(1), AccessId(2)),
        "Join should create ordering from joinee access to joiner access"
    );

    // No data race because join establishes HB.
    let output = verify(&input);
    assert!(
        output.is_race_free(),
        "Join edge should eliminate the race, but got {} races",
        output.race_count()
    );
}

#[test]
fn test_transitive_hb() {
    // A→B→C transitive closure: if T1 spawns T2 and T2 spawns T3,
    // then T1's accesses are ordered before T3's accesses.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);
    let t3 = ThreadId(3);

    // All three threads write to overlapping ranges.
    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 16, "trans_hb:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x1004, 16, "trans_hb:2"),
        t2.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(3, 0x1008, 16, "trans_hb:3"),
        t3.clone(),
    ));

    // T1 spawns T2, T2 spawns T3.
    input.add_spawn_edge(t1.clone(), t2.clone(), "spawn_1_2".into());
    input.add_spawn_edge(t2.clone(), t3.clone(), "spawn_2_3".into());

    let hb = HappensBeforeGraph::from_input(&input);

    // Direct edges.
    assert!(hb.is_ordered(AccessId(1), AccessId(2)), "T1→T2 should be ordered");
    assert!(hb.is_ordered(AccessId(2), AccessId(3)), "T2→T3 should be ordered");

    // Transitive edge: T1→T3.
    assert!(
        hb.is_ordered(AccessId(1), AccessId(3)),
        "Transitive HB should order T1 before T3"
    );

    // No data races because all are ordered.
    let output = verify(&input);
    assert!(
        output.is_race_free(),
        "Transitive ordering should eliminate all races, got {} races",
        output.race_count()
    );
}

#[test]
fn test_no_hb_between_independent_threads() {
    // Two threads with no spawn/join/sync edges between them should have
    // no happens-before relationship → concurrent → data race.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);

    // Both threads write to overlapping ranges with no synchronization.
    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 8, "indep:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x1004, 8, "indep:2"),
        t2.clone(),
    ));

    let hb = HappensBeforeGraph::from_input(&input);
    assert!(
        hb.are_concurrent(AccessId(1), AccessId(2)),
        "Independent threads should have no HB relationship"
    );

    // Should detect a data race.
    let output = verify(&input);
    assert_eq!(
        output.race_count(),
        1,
        "Independent threads with overlapping writes should produce exactly 1 data race"
    );
    assert_eq!(output.data_races[0].kind, ConflictKind::WriteWrite);
    assert_eq!(output.data_races[0].hb_relation, HBRelation::Concurrent);
}

#[test]
fn test_hb_with_mutex() {
    // Mutex acquire/release creates a happens-before ordering.
    // Two threads write to overlapping ranges, both protected by the same
    // mutex via sync edges. The Mutex sync edge both:
    // (a) puts both accesses in the same lock group, and
    // (b) establishes HB ordering.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);

    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 8, "mutex_hb:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x1004, 8, "mutex_hb:2"),
        t2.clone(),
    ));

    // Mutex sync edge between the two accesses.
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(1),
        AccessId(2),
        SyncOrdering::Mutex(42),
    ));

    // The Mutex sync edge should create HB ordering.
    let hb = HappensBeforeGraph::from_input(&input);
    assert!(
        hb.is_ordered(AccessId(1), AccessId(2)),
        "Mutex sync edge should create HB ordering"
    );

    // No data race — both mutex-protected AND ordered.
    let output = verify(&input);
    assert!(
        output.is_race_free(),
        "Mutex-protected accesses should not be reported as races, got {} races",
        output.race_count()
    );
}

// ===========================================================================
// Category 2: Data Race Detection (5 tests)
// ===========================================================================

#[test]
fn test_simple_data_race() {
    // Two threads write to the same address with no synchronization → race.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);

    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 8, "race_simple:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x1000, 8, "race_simple:2"),
        t2.clone(),
    ));

    let output = verify(&input);

    assert_eq!(
        output.race_count(),
        1,
        "Should detect exactly one data race"
    );
    assert_eq!(output.data_races[0].kind, ConflictKind::WriteWrite);
    assert_eq!(output.data_races[0].hb_relation, HBRelation::Concurrent);
    assert_eq!(output.data_races[0].overlapping_range, (0x1000, 0x1008));

    // The overall result should be Violated.
    assert!(
        matches!(output.result.status, VerificationStatus::Violated { .. }),
        "Data race should produce Violated status"
    );
}

#[test]
fn test_no_race_with_sync() {
    // Sync edge (HappensBefore) between two threads eliminates the race.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);

    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 8, "sync_safe:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x1004, 8, "sync_safe:2"),
        t2.clone(),
    ));

    // Explicit HB sync edge eliminates the race.
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(1),
        AccessId(2),
        SyncOrdering::HappensBefore,
    ));

    let output = verify(&input);
    assert!(
        output.is_race_free(),
        "Sync edge should eliminate the race, but got {} races",
        output.race_count()
    );
    assert!(
        matches!(output.result.status, VerificationStatus::Proven),
        "No races and no deadlocks should produce Proven status"
    );
}

#[test]
fn test_read_write_race() {
    // One thread reads, another writes to the same address → race.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);

    input.add_access(ThreadAccess::new(
        read_access(1, 0x1000, 8, "rw_race:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x1000, 8, "rw_race:2"),
        t2.clone(),
    ));

    let output = verify(&input);

    assert_eq!(
        output.race_count(),
        1,
        "Should detect exactly one read-write data race"
    );
    assert_eq!(output.data_races[0].kind, ConflictKind::WriteRead);
    assert_eq!(output.data_races[0].hb_relation, HBRelation::Concurrent);
}

#[test]
fn test_read_read_no_race() {
    // Two threads read the same address → no race (reads never conflict).
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);

    input.add_access(ThreadAccess::new(
        read_access(1, 0x1000, 8, "rr_safe:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        read_access(2, 0x1000, 8, "rr_safe:2"),
        t2.clone(),
    ));

    let output = verify(&input);
    assert!(
        output.is_race_free(),
        "Two concurrent reads should not produce a race, got {} races",
        output.race_count()
    );
}

#[test]
fn test_multiple_races() {
    // Multiple data races in one program: several pairs of threads writing
    // to overlapping ranges without synchronization.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);
    let t3 = ThreadId(3);

    // T1 and T2 both write to 0x1000 — race 1
    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 8, "multi_race:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x1000, 8, "multi_race:2"),
        t2.clone(),
    ));

    // T1 and T3 both write to 0x1000 — race 2
    input.add_access(ThreadAccess::new(
        write_access(3, 0x1000, 8, "multi_race:3"),
        t3.clone(),
    ));

    // T2 and T3 both write to 0x2000 — race 3
    input.add_access(ThreadAccess::new(
        write_access(4, 0x2000, 8, "multi_race:4"),
        t2.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(5, 0x2000, 8, "multi_race:5"),
        t3.clone(),
    ));

    let output = verify(&input);

    // Races: A1-A2 (WW), A1-A3 (WW), A2-A3 (WW), A4-A5 (WW) = 4 races
    assert_eq!(
        output.race_count(),
        4,
        "Should detect 4 data races across 3 threads, got {}",
        output.race_count()
    );

    // All should be WriteWrite.
    assert_eq!(
        output.write_write_race_count(),
        4,
        "All races should be write-write"
    );
    assert_eq!(output.write_read_race_count(), 0);

    // All should be concurrent.
    for race in &output.data_races {
        assert_eq!(race.hb_relation, HBRelation::Concurrent);
    }
}

// ===========================================================================
// Category 3: Deadlock Detection (5 tests)
// ===========================================================================

#[test]
fn test_simple_deadlock() {
    // Lock order reversal: T1 acquires lock A then B, T2 acquires lock B then A.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);

    // T1 accesses — to associate with lock acquisitions.
    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 8, "deadlock:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x2000, 8, "deadlock:2"),
        t1.clone(),
    ));

    // T2 accesses — to associate with lock acquisitions.
    input.add_access(ThreadAccess::new(
        write_access(3, 0x1000, 8, "deadlock:3"),
        t2.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(4, 0x2000, 8, "deadlock:4"),
        t2.clone(),
    ));

    // T1 acquires lock 10 then lock 20.
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(1),
        AccessId(2),
        SyncOrdering::Mutex(10),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(2),
        AccessId(1),
        SyncOrdering::Mutex(20),
    ));

    // T2 acquires lock 20 then lock 10 (reversed order).
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(3),
        AccessId(4),
        SyncOrdering::Mutex(20),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(4),
        AccessId(3),
        SyncOrdering::Mutex(10),
    ));

    let output = verify(&input);
    assert!(
        !output.deadlock_warnings.is_empty(),
        "Should detect deadlock from lock order reversal"
    );

    // Verify the deadlock warning references the correct threads and locks.
    let dw = &output.deadlock_warnings[0];
    assert!(
        (dw.thread1 == t1 && dw.thread2 == t2) || (dw.thread1 == t2 && dw.thread2 == t1),
        "Deadlock should involve T1 and T2"
    );
}

#[test]
fn test_no_deadlock_same_order() {
    // Both threads acquire locks in the same order → no deadlock.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);

    // T1 accesses.
    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 8, "no_deadlock:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x2000, 8, "no_deadlock:2"),
        t1.clone(),
    ));

    // T2 accesses.
    input.add_access(ThreadAccess::new(
        write_access(3, 0x1000, 8, "no_deadlock:3"),
        t2.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(4, 0x2000, 8, "no_deadlock:4"),
        t2.clone(),
    ));

    // Both T1 and T2 acquire lock 10 then lock 20 (same order).
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(1),
        AccessId(2),
        SyncOrdering::Mutex(10),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(2),
        AccessId(1),
        SyncOrdering::Mutex(20),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(3),
        AccessId(4),
        SyncOrdering::Mutex(10),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(4),
        AccessId(3),
        SyncOrdering::Mutex(20),
    ));

    let output = verify(&input);
    assert!(
        output.deadlock_warnings.is_empty(),
        "Same lock order should not produce deadlock warnings, got {}",
        output.deadlock_count()
    );
}

#[test]
fn test_three_lock_deadlock() {
    // Three-lock deadlock via pairwise order reversal between two threads:
    // T1 acquires lock A → lock B → lock C,
    // T2 acquires lock C → lock A → lock B.
    // Pairwise: A vs C reversed, B vs C reversed → deadlock detected.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);

    // T1 accesses — three accesses for three locks.
    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 8, "three_dead:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x2000, 8, "three_dead:2"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(3, 0x3000, 8, "three_dead:3"),
        t1.clone(),
    ));

    // T2 accesses — three accesses for three locks (reversed order).
    input.add_access(ThreadAccess::new(
        write_access(4, 0x4000, 8, "three_dead:4"),
        t2.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(5, 0x5000, 8, "three_dead:5"),
        t2.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(6, 0x6000, 8, "three_dead:6"),
        t2.clone(),
    ));

    // T1: lock 10 → lock 20 → lock 30
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(1),
        AccessId(2),
        SyncOrdering::Mutex(10),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(2),
        AccessId(3),
        SyncOrdering::Mutex(20),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(3),
        AccessId(1),
        SyncOrdering::Mutex(30),
    ));

    // T2: lock 30 → lock 10 → lock 20 (reversed A↔C, B↔C)
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(4),
        AccessId(5),
        SyncOrdering::Mutex(30),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(5),
        AccessId(6),
        SyncOrdering::Mutex(10),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(6),
        AccessId(4),
        SyncOrdering::Mutex(20),
    ));

    let output = verify(&input);

    // At least one deadlock warning should be detected due to lock order
    // reversal on the A↔C and/or B↔C pairs.
    assert!(
        !output.deadlock_warnings.is_empty(),
        "Three-lock cycle (two threads, reversed order) should produce at least one deadlock warning, got {}",
        output.deadlock_count()
    );
}

#[test]
fn test_deadlock_with_multiple_threads() {
    // 3+ threads with complex lock contention patterns.
    // T1: lock A → lock B → lock C
    // T2: lock C → lock A → lock B
    // T3: lock B → lock C → lock A
    // Multiple pairwise order reversals should be detected.
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);
    let t2 = ThreadId(2);
    let t3 = ThreadId(3);

    // T1 accesses.
    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 4, "multi_thread:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x2000, 4, "multi_thread:2"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(3, 0x3000, 4, "multi_thread:3"),
        t1.clone(),
    ));

    // T2 accesses.
    input.add_access(ThreadAccess::new(
        write_access(4, 0x4000, 4, "multi_thread:4"),
        t2.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(5, 0x5000, 4, "multi_thread:5"),
        t2.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(6, 0x6000, 4, "multi_thread:6"),
        t2.clone(),
    ));

    // T3 accesses.
    input.add_access(ThreadAccess::new(
        write_access(7, 0x7000, 4, "multi_thread:7"),
        t3.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(8, 0x8000, 4, "multi_thread:8"),
        t3.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(9, 0x9000, 4, "multi_thread:9"),
        t3.clone(),
    ));

    // T1: lock 10 → lock 20 → lock 30
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(1), AccessId(2), SyncOrdering::Mutex(10),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(2), AccessId(3), SyncOrdering::Mutex(20),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(3), AccessId(1), SyncOrdering::Mutex(30),
    ));

    // T2: lock 30 → lock 10 → lock 20 (reversed A→C, C→A etc.)
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(4), AccessId(5), SyncOrdering::Mutex(30),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(5), AccessId(6), SyncOrdering::Mutex(10),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(6), AccessId(4), SyncOrdering::Mutex(20),
    ));

    // T3: lock 20 → lock 30 → lock 10
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(7), AccessId(8), SyncOrdering::Mutex(20),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(8), AccessId(9), SyncOrdering::Mutex(30),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(9), AccessId(7), SyncOrdering::Mutex(10),
    ));

    let output = verify(&input);

    // Multiple pairwise reversals should be detected.
    assert!(
        output.deadlock_count() >= 1,
        "Multiple threads with reversed lock orders should produce at least one deadlock warning, got {}",
        output.deadlock_count()
    );

    // Verify that the warnings involve different thread pairs.
    let threads_involved: std::collections::HashSet<_> = output
        .deadlock_warnings
        .iter()
        .flat_map(|w| vec![w.thread1.clone(), w.thread2.clone()])
        .collect();
    assert!(
        threads_involved.len() >= 2,
        "Deadlock warnings should involve at least 2 different threads"
    );
}

#[test]
fn test_no_deadlock_single_thread() {
    // Single-threaded programs cannot have deadlocks (deadlock requires
    // at least 2 threads with lock order reversal).
    let mut input = ConcurrentExclusivityInput::new();
    let t1 = ThreadId(1);

    // Single thread with multiple lock acquisitions.
    input.add_access(ThreadAccess::new(
        write_access(1, 0x1000, 8, "single:1"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(2, 0x2000, 8, "single:2"),
        t1.clone(),
    ));
    input.add_access(ThreadAccess::new(
        write_access(3, 0x3000, 8, "single:3"),
        t1.clone(),
    ));

    // T1 acquires lock 10, 20, 30 in some order.
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(1),
        AccessId(2),
        SyncOrdering::Mutex(10),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(2),
        AccessId(3),
        SyncOrdering::Mutex(20),
    ));
    input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(3),
        AccessId(1),
        SyncOrdering::Mutex(30),
    ));

    let output = verify(&input);

    // No deadlock warnings — deadlock requires at least 2 threads.
    assert!(
        output.deadlock_warnings.is_empty(),
        "Single-threaded program should not produce deadlock warnings, got {}",
        output.deadlock_count()
    );

    // Also no data races (same thread).
    assert!(
        output.is_race_free(),
        "Same-thread accesses should not produce data races"
    );
}
