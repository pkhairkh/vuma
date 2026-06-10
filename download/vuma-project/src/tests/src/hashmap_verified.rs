//! Verified hash map tests using VUMA's IVE verification components.
//!
//! This module tests a hash map data structure — an array of buckets where each
//! bucket heads a linked list of entries — by exercising all four core IVE
//! verifiers (Exclusivity, Interpretation, Liveness, Cleanup) directly.
//!
//! # Memory layout model
//!
//! ```text
//! HashMap struct:   [ num_buckets (u64) | bucket_ptr (ptr to array) ]
//! Bucket array:     [ ptr_0 | ptr_1 | ... | ptr_{N-1} ]   (one per bucket)
//! Entry node:       [ key (u64) | value (u64) | next (ptr) ]
//! ```
//!
//! # Tests
//!
//! 1. `test_hashmap_create`       — allocate bucket array, verify with CleanupVerifier
//! 2. `test_hashmap_insert`        — insert key-value pair, verify with Exclusivity + Interpretation
//! 3. `test_hashmap_lookup`        — lookup by key, verify read with Exclusivity
//! 4. `test_hashmap_collision`     — two keys same hash → linked list, verify with Exclusivity
//! 5. `test_hashmap_remove`        — remove entry, verify pointer reconnection with Liveness + Cleanup
//! 6. `test_hashmap_dealloc`       — free all buckets + entries, verify with CleanupVerifier

use vuma_ive::{
    // Exclusivity verifier
    AccessRecord, CapDInfo, ConflictKind, ExclusivityAccessId as AccessId,
    ExclusivityAccessKind as AccessKind, ExclusivityInput, ExclusivityOutput,
    ExclusivityVerifier, SyncEdgeRecord, SyncOrdering,

    // Cleanup verifier
    AnnotatedCleanupGraph, CleanupGraph, CleanupNodeId, CleanupResourceId,
    CleanupResourceKind, CleanupReport, CleanupVerifier, OperationKind,

    // Liveness verifier
    DeadReason, EventAction, InitializationMap,
    LivenessInput, LivenessVerificationContext,
    LivenessVerifier, ObligationKind,
    PointId, ProofObligation, ResourceEvent, ResourceId as LivenessResourceId,
    ResourceKind as LivenessResourceKind, ThreadId,
};
use vuma_ive::liveness::ControlFlowEdge;
use vuma_ive::cleanup::ViolationKind as CleanupViolationKind;

// ---------------------------------------------------------------------------
// Address constants for our simulated hash map memory layout
// ---------------------------------------------------------------------------

/// Base address for the HashMap struct (2 × u64: num_buckets + bucket_ptr).
const HASHMAP_STRUCT_BASE: u64 = 0x1000;
const HASHMAP_STRUCT_SIZE: u64 = 16; // two u64 fields

/// Base address for the bucket array (4 bucket pointers, each u64).
const BUCKET_ARRAY_BASE: u64 = 0x2000;
const NUM_BUCKETS: u64 = 4;
const BUCKET_ARRAY_SIZE: u64 = NUM_BUCKETS * 8; // 4 pointers × 8 bytes

/// Base address for entry nodes. We space them 24 bytes apart
/// (key:8 + value:8 + next:8).
const ENTRY_BASE: u64 = 0x3000;
const ENTRY_SIZE: u64 = 24;

/// Helper: compute entry node base address from index.
fn entry_addr(index: u64) -> u64 {
    ENTRY_BASE + index * ENTRY_SIZE
}

// ---------------------------------------------------------------------------
// Exclusivity helpers
// ---------------------------------------------------------------------------

/// Helper: create a ProgramPoint string.
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

/// Run the exclusivity verifier on an input and return the output.
fn verify_exclusivity(input: &ExclusivityInput) -> ExclusivityOutput {
    ExclusivityVerifier::new().verify(input)
}

// ---------------------------------------------------------------------------
// Cleanup helpers
// ---------------------------------------------------------------------------

/// Shorthand for a Cleanup ResourceId.
fn rid(id: u64) -> CleanupResourceId {
    CleanupResourceId(id)
}

/// Shorthand for a Cleanup NodeId.
fn nid(_id: u64) -> CleanupNodeId {
    // Not used directly — nodes are created by add_node
    CleanupNodeId(0)
}

/// Verify a plain CleanupGraph.
fn verify_cleanup(graph: &CleanupGraph) -> CleanupReport {
    CleanupVerifier::new().verify(graph)
}

// ---------------------------------------------------------------------------
// Liveness helpers
// ---------------------------------------------------------------------------

/// Shorthand for PointId.
fn lpp(id: u64) -> PointId {
    PointId(id)
}

/// Shorthand for Liveness ResourceId.
fn lrid(id: u64) -> LivenessResourceId {
    LivenessResourceId(id)
}

/// Shorthand for ThreadId.
fn tid(id: u64) -> ThreadId {
    ThreadId(id)
}

/// Create a memory Allocate event.
fn alloc_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: lrid(resource),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Allocate,
        point: lpp(point),
        thread: tid(thread),
    }
}

/// Create a memory Deallocate event.
fn dealloc_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: lrid(resource),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Deallocate,
        point: lpp(point),
        thread: tid(thread),
    }
}

/// Create a memory Read event.
fn read_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: lrid(resource),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Read,
        point: lpp(point),
        thread: tid(thread),
    }
}

/// Create a memory Write event.
fn write_event(resource: u64, point: u64, thread: u64) -> ResourceEvent {
    ResourceEvent {
        resource: lrid(resource),
        kind: LivenessResourceKind::Memory,
        event: EventAction::Write,
        point: lpp(point),
        thread: tid(thread),
    }
}

/// Create a simple unconditional CFG edge.
fn cfg_edge(from: u64, to: u64) -> ControlFlowEdge {
    ControlFlowEdge {
        from: lpp(from),
        to: lpp(to),
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
// Test 1: test_hashmap_create
// ===========================================================================

/// Test: Create a hash map with bucket array, verify allocation.
///
/// Models the creation of a HashMap that allocates:
/// - R1: the HashMap struct (num_buckets + bucket_ptr)
/// - R2: the bucket array (4 bucket pointers, initially null)
///
/// CleanupVerifier should confirm both resources are properly acquired
/// and then released with no leaks. ExclusivityVerifier should confirm
/// no overlapping writes to the bucket array slots.
#[test]
fn test_hashmap_create() {
    // ---- Cleanup verification: alloc struct → alloc bucket array → free both ----
    let mut g = CleanupGraph::new();
    let entry = g.add_node(OperationKind::Passthrough, "entry");

    let alloc_struct = g.add_node(
        OperationKind::Acquire {
            resource: rid(1), // HashMap struct
            kind: CleanupResourceKind::Memory,
        },
        "alloc_hashmap_struct",
    );
    let write_struct = g.add_node(
        OperationKind::Access { resource: rid(1) },
        "write_num_buckets_and_bucket_ptr",
    );
    let alloc_buckets = g.add_node(
        OperationKind::Acquire {
            resource: rid(2), // bucket array
            kind: CleanupResourceKind::Memory,
        },
        "alloc_bucket_array",
    );
    let write_buckets = g.add_node(
        OperationKind::Access { resource: rid(2) },
        "write_null_bucket_pointers",
    );
    let free_buckets = g.add_node(
        OperationKind::Release {
            resource: rid(2),
            kind: CleanupResourceKind::Memory,
        },
        "free_bucket_array",
    );
    let free_struct = g.add_node(
        OperationKind::Release {
            resource: rid(1),
            kind: CleanupResourceKind::Memory,
        },
        "free_hashmap_struct",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(entry, alloc_struct).unwrap();
    g.add_edge(alloc_struct, write_struct).unwrap();
    g.add_edge(write_struct, alloc_buckets).unwrap();
    g.add_edge(alloc_buckets, write_buckets).unwrap();
    g.add_edge(write_buckets, free_buckets).unwrap();
    g.add_edge(free_buckets, free_struct).unwrap();
    g.add_edge(free_struct, ret).unwrap();
    g.set_entry(entry).unwrap();

    let report = verify_cleanup(&g);
    assert!(
        report.clean,
        "hashmap creation + destruction should be clean, violations: {:?}",
        report.violations
    );
    assert_eq!(report.acquires_checked, 2, "should have checked 2 acquire nodes");

    // ---- Exclusivity verification: writes to non-overlapping bucket slots ----
    let mut input = ExclusivityInput::new();
    // Write num_buckets and bucket_ptr into HashMap struct
    input.add_access(write_access(1, HASHMAP_STRUCT_BASE, 8, "hashmap.vu:1")); // num_buckets
    input.add_access(write_access(2, HASHMAP_STRUCT_BASE + 8, 8, "hashmap.vu:2")); // bucket_ptr
    // Write null pointers into each bucket slot (non-overlapping)
    for i in 0..NUM_BUCKETS {
        input.add_access(write_access(
            3 + i,
            BUCKET_ARRAY_BASE + i * 8,
            8,
            &format!("hashmap.vu:bucket_{}", i),
        ));
    }

    let output = verify_exclusivity(&input);
    assert!(
        output.is_proven(),
        "non-overlapping writes to distinct struct fields and bucket slots should be Proven, got: {:?}",
        output.result.status
    );
    assert_eq!(output.conflict_count(), 0, "expected no conflicts during creation");
}

// ===========================================================================
// Test 2: test_hashmap_insert
// ===========================================================================

/// Test: Insert a key-value pair, verify write to bucket.
///
/// Models inserting entry (key=42, value=100) into bucket 2.
/// This involves:
/// 1. Allocating an entry node
/// 2. Writing key, value, and next pointer into the entry
/// 3. Updating the bucket pointer to point to the new entry
///
/// ExclusivityVerifier should confirm writes to the entry node and
/// the bucket pointer don't conflict. InterpretationVerifier is
/// exercised via the CleanupVerifier's access tracking.
#[test]
fn test_hashmap_insert() {
    // ---- Cleanup verification: alloc entry → write entry → update bucket → free ----
    let mut g = CleanupGraph::new();
    let entry = g.add_node(OperationKind::Passthrough, "entry");

    // Assume bucket array is already allocated
    let alloc_entry_node = g.add_node(
        OperationKind::Acquire {
            resource: rid(10), // entry node
            kind: CleanupResourceKind::Memory,
        },
        "alloc_entry_node",
    );
    let write_key = g.add_node(
        OperationKind::Access { resource: rid(10) },
        "write_key_field",
    );
    let write_value = g.add_node(
        OperationKind::Access { resource: rid(10) },
        "write_value_field",
    );
    let write_next = g.add_node(
        OperationKind::Access { resource: rid(10) },
        "write_next_null",
    );
    let update_bucket = g.add_node(
        OperationKind::Access { resource: rid(2) }, // bucket array
        "update_bucket_ptr_to_entry",
    );
    let free_entry = g.add_node(
        OperationKind::Release {
            resource: rid(10),
            kind: CleanupResourceKind::Memory,
        },
        "free_entry_node",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(entry, alloc_entry_node).unwrap();
    g.add_edge(alloc_entry_node, write_key).unwrap();
    g.add_edge(write_key, write_value).unwrap();
    g.add_edge(write_value, write_next).unwrap();
    g.add_edge(write_next, update_bucket).unwrap();
    g.add_edge(update_bucket, free_entry).unwrap();
    g.add_edge(free_entry, ret).unwrap();
    g.set_entry(entry).unwrap();

    let report = verify_cleanup(&g);
    assert!(
        report.clean,
        "insert entry with proper free should be clean, violations: {:?}",
        report.violations
    );

    // ---- Exclusivity verification: writes to entry node fields + bucket pointer ----
    let mut input = ExclusivityInput::new();
    let entry0_addr = entry_addr(0);

    // Write key field [0, 8)
    input.add_access(write_access(1, entry0_addr, 8, "insert.vu:write_key"));
    // Write value field [8, 16)
    input.add_access(write_access(2, entry0_addr + 8, 8, "insert.vu:write_value"));
    // Write next field [16, 24)
    input.add_access(write_access(3, entry0_addr + 16, 8, "insert.vu:write_next"));
    // Write bucket 2 pointer in bucket array
    input.add_access(write_access(4, BUCKET_ARRAY_BASE + 2 * 8, 8, "insert.vu:update_bucket2"));

    // These writes are sequential and non-overlapping — no sync edges needed
    // for exclusivity (they don't overlap), but add HappensBefore to model
    // the program order.
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(1), AccessId(2), SyncOrdering::HappensBefore));
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(2), AccessId(3), SyncOrdering::HappensBefore));
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(3), AccessId(4), SyncOrdering::HappensBefore));

    let output = verify_exclusivity(&input);
    assert!(
        output.is_proven(),
        "sequential non-overlapping writes to entry fields and bucket should be Proven, got: {:?}",
        output.result.status
    );
    assert_eq!(output.conflict_count(), 0);

    // ---- Also verify that overlapping writes WOULD be caught ----
    let mut bad_input = ExclusivityInput::new();
    // Two concurrent writes to the same key field (data race)
    bad_input.add_access(write_access(10, entry0_addr, 8, "insert.vu:thread1_write_key"));
    bad_input.add_access(write_access(11, entry0_addr, 8, "insert.vu:thread2_write_key"));
    // No sync edge — concurrent

    let bad_output = verify_exclusivity(&bad_input);
    assert!(
        bad_output.is_violated(),
        "concurrent writes to the same key field should be Violated"
    );
    assert_eq!(bad_output.write_write_count(), 1);
}

// ===========================================================================
// Test 3: test_hashmap_lookup
// ===========================================================================

/// Test: Lookup by key, verify read from bucket.
///
/// Models looking up key=42 in bucket 2. This involves:
/// 1. Reading the bucket pointer
/// 2. Reading entry key field and comparing
/// 3. If key matches, reading the value field
///
/// ExclusivityVerifier should confirm:
/// - Concurrent reads to the same bucket are safe (read-read is always safe)
/// - A concurrent write+read to overlapping addresses is detected
#[test]
fn test_hashmap_lookup() {
    // ---- Exclusivity: concurrent reads to the same bucket are safe ----
    let mut safe_input = ExclusivityInput::new();
    let entry0_addr = entry_addr(0);

    // Thread 1: read bucket pointer, then read key
    safe_input.add_access(read_access(1, BUCKET_ARRAY_BASE + 2 * 8, 8, "lookup.vu:t1_read_bucket_ptr"));
    safe_input.add_access(read_access(2, entry0_addr, 8, "lookup.vu:t1_read_key"));

    // Thread 2: read same bucket pointer, then read key (concurrent read)
    safe_input.add_access(read_access(3, BUCKET_ARRAY_BASE + 2 * 8, 8, "lookup.vu:t2_read_bucket_ptr"));
    safe_input.add_access(read_access(4, entry0_addr, 8, "lookup.vu:t2_read_key"));

    // Read value after key match
    safe_input.add_access(read_access(5, entry0_addr + 8, 8, "lookup.vu:t1_read_value"));

    let output = verify_exclusivity(&safe_input);
    assert!(
        output.is_proven(),
        "concurrent reads to same bucket/entry should be Proven, got: {:?}",
        output.result.status
    );
    assert_eq!(output.conflict_count(), 0);

    // ---- Exclusivity: concurrent write + read to same entry is detected ----
    let mut race_input = ExclusivityInput::new();
    // Thread 1 writes the value field
    race_input.add_access(write_access(10, entry0_addr + 8, 8, "lookup.vu:write_value"));
    // Thread 2 reads the value field concurrently
    race_input.add_access(read_access(11, entry0_addr + 8, 8, "lookup.vu:read_value"));

    let race_output = verify_exclusivity(&race_input);
    assert!(
        race_output.is_violated(),
        "concurrent write+read to value field should be Violated"
    );
    assert_eq!(race_output.write_read_count(), 1);
    assert!(
        matches!(race_output.conflicts[0].kind, ConflictKind::WriteRead),
        "expected WriteRead conflict"
    );

    // ---- Exclusivity: write→read with HappensBefore is safe ----
    let mut ordered_input = ExclusivityInput::new();
    ordered_input.add_access(write_access(20, entry0_addr + 8, 8, "lookup.vu:ordered_write_value"));
    ordered_input.add_access(read_access(21, entry0_addr + 8, 8, "lookup.vu:ordered_read_value"));
    ordered_input.add_sync_edge(SyncEdgeRecord::new(
        AccessId(20), AccessId(21), SyncOrdering::HappensBefore,
    ));

    let ordered_output = verify_exclusivity(&ordered_input);
    assert!(
        ordered_output.is_proven(),
        "write→read with HappensBefore should be Proven, got: {:?}",
        ordered_output.result.status
    );
}

// ===========================================================================
// Test 4: test_hashmap_collision
// ===========================================================================

/// Test: Two keys with the same hash, verify linked list in bucket.
///
/// When two keys hash to the same bucket, they form a linked list:
///
/// ```text
/// bucket[i] → entry_A (key=10) → entry_B (key=20) → null
/// ```
///
/// This test verifies:
/// - Writes to both entry nodes are non-overlapping (different memory)
/// - Updating the next pointer of entry_A to point to entry_B is safe
/// - Concurrent insertions to the SAME bucket would be a conflict
/// - Sequential chaining (A then B) is safe with sync ordering
#[test]
fn test_hashmap_collision() {
    let entry_a_addr = entry_addr(0); // first entry in bucket
    let entry_b_addr = entry_addr(1); // second entry (chained)

    // ---- Exclusivity: writes to two different entry nodes are safe ----
    let mut input = ExclusivityInput::new();
    // Write entry A: key, value, next=null
    input.add_access(write_access(1, entry_a_addr, 8, "collision.vu:write_A_key"));
    input.add_access(write_access(2, entry_a_addr + 8, 8, "collision.vu:write_A_value"));
    input.add_access(write_access(3, entry_a_addr + 16, 8, "collision.vu:write_A_next_null"));
    // Write entry B: key, value, next=null
    input.add_access(write_access(4, entry_b_addr, 8, "collision.vu:write_B_key"));
    input.add_access(write_access(5, entry_b_addr + 8, 8, "collision.vu:write_B_value"));
    input.add_access(write_access(6, entry_b_addr + 16, 8, "collision.vu:write_B_next_null"));
    // Chain: update entry_A's next pointer to point to entry_B
    input.add_access(write_access(7, entry_a_addr + 16, 8, "collision.vu:update_A_next_to_B"));

    // Sequential ordering
    for i in 1..7 {
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(i), AccessId(i + 1), SyncOrdering::HappensBefore,
        ));
    }

    let output = verify_exclusivity(&input);
    assert!(
        output.is_proven(),
        "sequential writes to different entries + chain update should be Proven, got: {:?}",
        output.result.status
    );

    // ---- Exclusivity: two concurrent insertions to the same bucket conflict ----
    let mut conflict_input = ExclusivityInput::new();
    // Thread 1 writes key to entry A
    conflict_input.add_access(write_access(10, entry_a_addr, 8, "collision.vu:t1_write_A_key"));
    // Thread 2 also writes key to entry A (same location — collision!)
    conflict_input.add_access(write_access(11, entry_a_addr, 8, "collision.vu:t2_write_A_key"));
    // No sync edge → concurrent write-write conflict

    let conflict_output = verify_exclusivity(&conflict_input);
    assert!(
        conflict_output.is_violated(),
        "concurrent writes to same entry should be Violated"
    );
    assert_eq!(conflict_output.write_write_count(), 1);

    // ---- Cleanup: both entries allocated and freed with proper chain ----
    let mut g = CleanupGraph::new();
    let start = g.add_node(OperationKind::Passthrough, "start");

    let alloc_a = g.add_node(
        OperationKind::Acquire { resource: rid(20), kind: CleanupResourceKind::Memory },
        "alloc_entry_A",
    );
    let alloc_b = g.add_node(
        OperationKind::Acquire { resource: rid(21), kind: CleanupResourceKind::Memory },
        "alloc_entry_B",
    );
    let chain = g.add_node(
        OperationKind::Access { resource: rid(20) },
        "chain_A_to_B",
    );
    let free_b = g.add_node(
        OperationKind::Release { resource: rid(21), kind: CleanupResourceKind::Memory },
        "free_entry_B",
    );
    let free_a = g.add_node(
        OperationKind::Release { resource: rid(20), kind: CleanupResourceKind::Memory },
        "free_entry_A",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(start, alloc_a).unwrap();
    g.add_edge(alloc_a, alloc_b).unwrap();
    g.add_edge(alloc_b, chain).unwrap();
    g.add_edge(chain, free_b).unwrap();
    g.add_edge(free_b, free_a).unwrap();
    g.add_edge(free_a, ret).unwrap();
    g.set_entry(start).unwrap();

    let report = verify_cleanup(&g);
    assert!(
        report.clean,
        "collision chain with proper alloc/free should be clean, violations: {:?}",
        report.violations
    );
    assert_eq!(report.acquires_checked, 2);
}

// ===========================================================================
// Test 5: test_hashmap_remove
// ===========================================================================

/// Test: Remove key, verify pointer reconnection (similar to dlist remove).
///
/// Starting state: bucket[i] → A → B → C
/// After removing B: bucket[i] → A → C
///
/// This requires:
/// 1. Reading A.next (to find B)
/// 2. Reading B.next (to find C)
/// 3. Writing A.next = C (reconnect)
/// 4. Freeing B
///
/// The reconnection is analogous to doubly-linked list removal: we must
/// verify no use-after-free of B after it's freed, and that the pointer
/// update (A.next → C) is safe.
#[test]
fn test_hashmap_remove() {
    let entry_a_addr = entry_addr(0);
    let entry_b_addr = entry_addr(1);
    let entry_c_addr = entry_addr(2);

    // ---- Exclusivity: read B.next, write A.next, sequential order → safe ----
    let mut input = ExclusivityInput::new();
    // Read A.next to find B
    input.add_access(read_access(1, entry_a_addr + 16, 8, "remove.vu:read_A_next"));
    // Read B.next to find C
    input.add_access(read_access(2, entry_b_addr + 16, 8, "remove.vu:read_B_next"));
    // Write A.next = C (reconnect)
    input.add_access(write_access(3, entry_a_addr + 16, 8, "remove.vu:write_A_next_to_C"));

    // Sequential ordering
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(1), AccessId(2), SyncOrdering::HappensBefore));
    input.add_sync_edge(SyncEdgeRecord::new(AccessId(2), AccessId(3), SyncOrdering::HappensBefore));

    let output = verify_exclusivity(&input);
    assert!(
        output.is_proven(),
        "sequential read A.next → read B.next → write A.next should be Proven, got: {:?}",
        output.result.status
    );

    // ---- Cleanup: alloc A, B, C → chain → remove B (free B) → free A, C ----
    let mut g = CleanupGraph::new();
    let start = g.add_node(OperationKind::Passthrough, "start");

    let alloc_a = g.add_node(
        OperationKind::Acquire { resource: rid(30), kind: CleanupResourceKind::Memory },
        "alloc_A",
    );
    let alloc_b = g.add_node(
        OperationKind::Acquire { resource: rid(31), kind: CleanupResourceKind::Memory },
        "alloc_B",
    );
    let alloc_c = g.add_node(
        OperationKind::Acquire { resource: rid(32), kind: CleanupResourceKind::Memory },
        "alloc_C",
    );
    let access_a = g.add_node(
        OperationKind::Access { resource: rid(30) },
        "read_A_next_find_B",
    );
    let access_b = g.add_node(
        OperationKind::Access { resource: rid(31) },
        "read_B_next_find_C",
    );
    let reconnect = g.add_node(
        OperationKind::Access { resource: rid(30) },
        "write_A_next_to_C",
    );
    let free_b = g.add_node(
        OperationKind::Release { resource: rid(31), kind: CleanupResourceKind::Memory },
        "free_B",
    );
    let free_c = g.add_node(
        OperationKind::Release { resource: rid(32), kind: CleanupResourceKind::Memory },
        "free_C",
    );
    let free_a = g.add_node(
        OperationKind::Release { resource: rid(30), kind: CleanupResourceKind::Memory },
        "free_A",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    g.add_edge(start, alloc_a).unwrap();
    g.add_edge(alloc_a, alloc_b).unwrap();
    g.add_edge(alloc_b, alloc_c).unwrap();
    g.add_edge(alloc_c, access_a).unwrap();
    g.add_edge(access_a, access_b).unwrap();
    g.add_edge(access_b, reconnect).unwrap();
    g.add_edge(reconnect, free_b).unwrap();
    g.add_edge(free_b, free_c).unwrap();
    g.add_edge(free_c, free_a).unwrap();
    g.add_edge(free_a, ret).unwrap();
    g.set_entry(start).unwrap();

    let report = verify_cleanup(&g);
    assert!(
        report.clean,
        "remove B with proper reconnection and freeing should be clean, violations: {:?}",
        report.violations
    );

    // ---- Liveness: use-after-free if we access B after removal ----
    let mut liveness_input = LivenessInput::new();
    // B is allocated, then freed, then accessed → UAF
    liveness_input.add_event(alloc_event(31, 1, 1));    // alloc B
    liveness_input.add_event(write_event(31, 2, 1));    // write B
    liveness_input.add_event(dealloc_event(31, 3, 1));  // free B (remove)
    liveness_input.add_event(read_event(31, 4, 1));     // read B after free! (UAF)
    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3, 4]);
    liveness_input.entry_point = Some(lpp(1));

    let context = LivenessVerificationContext::new(liveness_input);
    let verifier = LivenessVerifier::new();
    let paths = verifier.compute_liveness_paths(&context);

    assert_eq!(paths.len(), 1, "expected 1 liveness path for B");
    assert!(
        !paths[0].access_after_free.is_empty(),
        "expected access_after_free for reading B after it was freed"
    );
    assert!(
        paths[0].access_after_free.iter().any(|(_, desc)| desc.contains("read after free")),
        "expected 'read after free' in access_after_free description"
    );

    // Verify with proofs — should generate UseAfterFreeSafe obligation
    let mut prover = LivenessVerifier::new();
    let proof_result = prover.verify_with_proofs(&context);
    let uaf_obligations: Vec<&ProofObligation> = proof_result
        .proof_obligations
        .iter()
        .filter(|o| o.obligation_kind == ObligationKind::UseAfterFreeSafe)
        .collect();
    assert!(
        !uaf_obligations.is_empty(),
        "expected UseAfterFreeSafe proof obligation for B, got: {:?}",
        proof_result.proof_obligations
    );

    // ---- Liveness: correct removal does NOT produce UAF ----
    let mut safe_input = LivenessInput::new();
    // B allocated, accessed, freed — then we access A and C (not B)
    safe_input.add_event(alloc_event(30, 1, 1));   // alloc A
    safe_input.add_event(alloc_event(31, 2, 1));   // alloc B
    safe_input.add_event(alloc_event(32, 3, 1));   // alloc C
    safe_input.add_event(write_event(30, 4, 1));   // write A
    safe_input.add_event(write_event(31, 5, 1));   // write B
    safe_input.add_event(write_event(32, 6, 1));   // write C
    safe_input.add_event(dealloc_event(31, 7, 1));  // free B (remove)
    safe_input.add_event(read_event(30, 8, 1));    // read A (safe)
    safe_input.add_event(read_event(32, 9, 1));    // read C (safe)
    safe_input.add_event(dealloc_event(30, 10, 1)); // free A
    safe_input.add_event(dealloc_event(32, 11, 1)); // free C
    safe_input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    safe_input.entry_point = Some(lpp(1));

    let mut safe_verifier = LivenessVerifier::new();
    let safe_result = safe_verifier.verify(&safe_input);
    assert!(
        safe_result.invariant_holds,
        "correct removal should not produce UAF, violations: {:?}",
        safe_result.violations
    );
}

// ===========================================================================
// Test 6: test_hashmap_dealloc
// ===========================================================================

/// Test: Free all buckets and entries, verify cleanup.
///
/// Models a hash map with 4 buckets, 3 entries across 2 buckets:
///   bucket[0] → entry_X → null
///   bucket[1] → null
///   bucket[2] → entry_Y → entry_Z → null   (collision chain)
///   bucket[3] → null
///
/// Deallocation must:
/// 1. Walk each bucket's chain and free every entry
/// 2. Free the bucket array
/// 3. Free the HashMap struct
///
/// CleanupVerifier should confirm no leaks, no double-frees, no UAF.
/// LivenessVerifier should confirm all accesses happen before deallocation.
#[test]
fn test_hashmap_dealloc() {
    // ---- Cleanup: full deallocation of all entries + bucket array + struct ----
    let mut g = CleanupGraph::new();
    let start = g.add_node(OperationKind::Passthrough, "start");

    // Allocate struct + bucket array + 3 entries
    let alloc_struct = g.add_node(
        OperationKind::Acquire { resource: rid(1), kind: CleanupResourceKind::Memory },
        "alloc_hashmap_struct",
    );
    let alloc_buckets = g.add_node(
        OperationKind::Acquire { resource: rid(2), kind: CleanupResourceKind::Memory },
        "alloc_bucket_array",
    );
    let alloc_x = g.add_node(
        OperationKind::Acquire { resource: rid(100), kind: CleanupResourceKind::Memory },
        "alloc_entry_X",
    );
    let alloc_y = g.add_node(
        OperationKind::Acquire { resource: rid(101), kind: CleanupResourceKind::Memory },
        "alloc_entry_Y",
    );
    let alloc_z = g.add_node(
        OperationKind::Acquire { resource: rid(102), kind: CleanupResourceKind::Memory },
        "alloc_entry_Z",
    );

    // Access (write) all entries and bucket pointers
    let access_x = g.add_node(
        OperationKind::Access { resource: rid(100) },
        "write_entry_X",
    );
    let access_y = g.add_node(
        OperationKind::Access { resource: rid(101) },
        "write_entry_Y",
    );
    let access_z = g.add_node(
        OperationKind::Access { resource: rid(102) },
        "write_entry_Z",
    );
    let access_buckets = g.add_node(
        OperationKind::Access { resource: rid(2) },
        "write_bucket_pointers",
    );

    // Free entries (walk chains: X, then Z then Y since Y→Z)
    let free_z = g.add_node(
        OperationKind::Release { resource: rid(102), kind: CleanupResourceKind::Memory },
        "free_entry_Z",
    );
    let free_y = g.add_node(
        OperationKind::Release { resource: rid(101), kind: CleanupResourceKind::Memory },
        "free_entry_Y",
    );
    let free_x = g.add_node(
        OperationKind::Release { resource: rid(100), kind: CleanupResourceKind::Memory },
        "free_entry_X",
    );
    // Free bucket array and struct
    let free_buckets = g.add_node(
        OperationKind::Release { resource: rid(2), kind: CleanupResourceKind::Memory },
        "free_bucket_array",
    );
    let free_struct = g.add_node(
        OperationKind::Release { resource: rid(1), kind: CleanupResourceKind::Memory },
        "free_hashmap_struct",
    );
    let ret = g.add_node(OperationKind::Return, "return");

    // Build edges
    g.add_edge(start, alloc_struct).unwrap();
    g.add_edge(alloc_struct, alloc_buckets).unwrap();
    g.add_edge(alloc_buckets, alloc_x).unwrap();
    g.add_edge(alloc_x, alloc_y).unwrap();
    g.add_edge(alloc_y, alloc_z).unwrap();
    g.add_edge(alloc_z, access_x).unwrap();
    g.add_edge(access_x, access_y).unwrap();
    g.add_edge(access_y, access_z).unwrap();
    g.add_edge(access_z, access_buckets).unwrap();
    g.add_edge(access_buckets, free_z).unwrap();
    g.add_edge(free_z, free_y).unwrap();
    g.add_edge(free_y, free_x).unwrap();
    g.add_edge(free_x, free_buckets).unwrap();
    g.add_edge(free_buckets, free_struct).unwrap();
    g.add_edge(free_struct, ret).unwrap();
    g.set_entry(start).unwrap();

    let report = verify_cleanup(&g);
    assert!(
        report.clean,
        "full hashmap deallocation should be clean, violations: {:?}",
        report.violations
    );
    assert_eq!(report.acquires_checked, 5, "should check 5 acquire nodes (struct + buckets + 3 entries)");

    // ---- Cleanup: detect leak if an entry is not freed ----
    let mut leak_g = CleanupGraph::new();
    let leak_start = leak_g.add_node(OperationKind::Passthrough, "start");

    let leak_alloc_struct = leak_g.add_node(
        OperationKind::Acquire { resource: rid(1), kind: CleanupResourceKind::Memory },
        "alloc_struct",
    );
    let leak_alloc_buckets = leak_g.add_node(
        OperationKind::Acquire { resource: rid(2), kind: CleanupResourceKind::Memory },
        "alloc_buckets",
    );
    let leak_alloc_x = leak_g.add_node(
        OperationKind::Acquire { resource: rid(100), kind: CleanupResourceKind::Memory },
        "alloc_entry_X",
    );
    // Free struct and buckets but FORGET to free entry X → leak!
    let leak_free_buckets = leak_g.add_node(
        OperationKind::Release { resource: rid(2), kind: CleanupResourceKind::Memory },
        "free_buckets",
    );
    let leak_free_struct = leak_g.add_node(
        OperationKind::Release { resource: rid(1), kind: CleanupResourceKind::Memory },
        "free_struct",
    );
    let leak_ret = leak_g.add_node(OperationKind::Return, "return");

    leak_g.add_edge(leak_start, leak_alloc_struct).unwrap();
    leak_g.add_edge(leak_alloc_struct, leak_alloc_buckets).unwrap();
    leak_g.add_edge(leak_alloc_buckets, leak_alloc_x).unwrap();
    leak_g.add_edge(leak_alloc_x, leak_free_buckets).unwrap();
    leak_g.add_edge(leak_free_buckets, leak_free_struct).unwrap();
    leak_g.add_edge(leak_free_struct, leak_ret).unwrap();
    leak_g.set_entry(leak_start).unwrap();

    let leak_report = verify_cleanup(&leak_g);
    assert!(
        !leak_report.clean,
        "forgetting to free entry X should produce a leak violation"
    );
    let has_leak = leak_report
        .violations
        .iter()
        .any(|v| v.kind == CleanupViolationKind::Leak && v.resource == rid(100));
    assert!(
        has_leak,
        "expected Leak violation for entry X (rid(100)), got: {:?}",
        leak_report.violations
    );

    // ---- Liveness: all entries accessed before deallocation → clean ----
    let mut liveness_input = LivenessInput::new();
    // HashMap struct
    liveness_input.add_event(alloc_event(1, 1, 1));
    liveness_input.add_event(write_event(1, 2, 1));
    // Bucket array
    liveness_input.add_event(alloc_event(2, 3, 1));
    liveness_input.add_event(write_event(2, 4, 1));
    // Entry X
    liveness_input.add_event(alloc_event(100, 5, 1));
    liveness_input.add_event(write_event(100, 6, 1));
    liveness_input.add_event(read_event(100, 7, 1));
    // Entry Y
    liveness_input.add_event(alloc_event(101, 8, 1));
    liveness_input.add_event(write_event(101, 9, 1));
    liveness_input.add_event(read_event(101, 10, 1));
    // Entry Z
    liveness_input.add_event(alloc_event(102, 11, 1));
    liveness_input.add_event(write_event(102, 12, 1));
    liveness_input.add_event(read_event(102, 13, 1));
    // Free in reverse chain order
    liveness_input.add_event(dealloc_event(102, 14, 1)); // free Z
    liveness_input.add_event(dealloc_event(101, 15, 1)); // free Y
    liveness_input.add_event(dealloc_event(100, 16, 1)); // free X
    liveness_input.add_event(dealloc_event(2, 17, 1));   // free buckets
    liveness_input.add_event(dealloc_event(1, 18, 1));   // free struct

    liveness_input.cfg_edges = linear_cfg(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18]);
    liveness_input.entry_point = Some(lpp(1));

    let mut liveness_verifier = LivenessVerifier::new();
    let liveness_result = liveness_verifier.verify(&liveness_input);
    assert!(
        liveness_result.invariant_holds,
        "proper dealloc of all entries + struct should pass liveness, violations: {:?}",
        liveness_result.violations
    );
    assert_eq!(liveness_result.resources_checked, 5, "should check 5 resources");
}
