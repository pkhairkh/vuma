//! Concurrent Exclusivity Verification for the IVE module.
//!
//! This module extends the single-threaded exclusivity check with thread-aware
//! analysis. It implements:
//!
//! - **Happens-before graph** construction from sync edges, thread spawn/join,
//!   and transitive closure.
//! - **Data race detection** between concurrent (unordered) accesses from
//!   different threads.
//! - **Deadlock detection** (basic) for lock-order reversal patterns.
//!
//! # Core Concepts
//!
//! Two accesses from different threads form a **data race** if:
//! 1. Their byte ranges overlap,
//! 2. At least one is a write,
//! 3. They are not ordered by a happens-before relationship.
//!
//! The happens-before relation is derived from:
//! - Explicit sync edges (acquire-release, mutex),
//! - Thread spawn (parent's ops happen-before child's),
//! - Thread join (child's ops happen-before joiner's post-join ops),
//! - Transitivity.

use crate::exclusivity::{
    AccessId, AccessKind, AccessRecord, CapDInfo, ConflictKind, InterferenceGraph,
    SyncEdgeRecord, SyncOrdering,
};
use crate::result::{CounterExample, Evidence, ProgramPoint, VerificationResult, VerificationStatus};
use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// ThreadId — unique identifier for a thread
// ---------------------------------------------------------------------------

/// Unique identifier for a thread in the concurrent exclusivity analysis.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ThreadId(pub u64);

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "T{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// ThreadAccess — an access paired with its owning thread
// ---------------------------------------------------------------------------

/// A memory access event annotated with the thread that performs it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadAccess {
    /// The access record.
    pub access: AccessRecord,
    /// The thread performing this access.
    pub thread: ThreadId,
}

impl ThreadAccess {
    /// Create a new thread access.
    pub fn new(access: AccessRecord, thread: ThreadId) -> Self {
        Self { access, thread }
    }
}

// ---------------------------------------------------------------------------
// ConcurrentExclusivityInput — the input for concurrent exclusivity verification
// ---------------------------------------------------------------------------

/// Input for the concurrent exclusivity verifier.
///
/// Contains all thread-annotated accesses, synchronization edges, capability
/// descriptors, and thread lifecycle edges (spawn/join).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConcurrentExclusivityInput {
    /// All memory access events, each annotated with a thread ID.
    pub accesses: Vec<ThreadAccess>,
    /// Explicit synchronization edges between accesses.
    pub sync_edges: Vec<SyncEdgeRecord>,
    /// Capability descriptors indexed by access ID.
    pub capabilities: HashMap<AccessId, CapDInfo>,
    /// Thread spawn edges: (parent, child, spawn_point).
    /// The parent's operations before the spawn happen-before the child's.
    pub thread_spawn_edges: Vec<(ThreadId, ThreadId, ProgramPoint)>,
    /// Thread join edges: (joiner, joinee, join_point).
    /// The joinee's operations happen-before the joiner's post-join operations.
    pub thread_join_edges: Vec<(ThreadId, ThreadId, ProgramPoint)>,
}

impl ConcurrentExclusivityInput {
    /// Create an empty input.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a thread-annotated access.
    pub fn add_access(&mut self, access: ThreadAccess) {
        self.accesses.push(access);
    }

    /// Add a sync edge.
    pub fn add_sync_edge(&mut self, edge: SyncEdgeRecord) {
        self.sync_edges.push(edge);
    }

    /// Set the capability for an access.
    pub fn set_capability(&mut self, access_id: AccessId, cap: CapDInfo) {
        self.capabilities.insert(access_id, cap);
    }

    /// Add a thread spawn edge.
    pub fn add_spawn_edge(&mut self, parent: ThreadId, child: ThreadId, point: ProgramPoint) {
        self.thread_spawn_edges.push((parent, child, point));
    }

    /// Add a thread join edge.
    pub fn add_join_edge(&mut self, joiner: ThreadId, joinee: ThreadId, point: ProgramPoint) {
        self.thread_join_edges.push((joiner, joinee, point));
    }

    /// Build a map from AccessId to the ThreadId that owns it.
    pub fn access_thread_map(&self) -> HashMap<AccessId, ThreadId> {
        self.accesses
            .iter()
            .map(|ta| (ta.access.id, ta.thread.clone()))
            .collect()
    }

    /// Build a map from AccessId to the ThreadAccess.
    pub fn access_by_id(&self) -> HashMap<AccessId, &ThreadAccess> {
        self.accesses
            .iter()
            .map(|ta| (ta.access.id, ta))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// HBRelation — happens-before relationship between two accesses
// ---------------------------------------------------------------------------

/// Describes the happens-before relationship between two accesses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HBRelation {
    /// No ordering exists — the accesses are concurrent (potential race).
    Concurrent,
    /// One happens before the other — not a race.
    Ordered,
}

impl fmt::Display for HBRelation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HBRelation::Concurrent => write!(f, "concurrent"),
            HBRelation::Ordered => write!(f, "ordered"),
        }
    }
}

// ---------------------------------------------------------------------------
// DataRace — a detected data race
// ---------------------------------------------------------------------------

/// A detected data race between two concurrent accesses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataRace {
    /// The first access in the race.
    pub access1: ThreadAccess,
    /// The second access in the race.
    pub access2: ThreadAccess,
    /// The overlapping byte range (start, end).
    pub overlapping_range: (u64, u64),
    /// The kind of conflict (write-write or write-read).
    pub kind: ConflictKind,
    /// The happens-before relationship between the two accesses.
    pub hb_relation: HBRelation,
}

impl fmt::Display for DataRace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DataRace: {} on T{} vs {} on T{} at [0x{:x}, 0x{:x}) ({}, {})",
            self.access1.access.id,
            self.access1.thread.0,
            self.access2.access.id,
            self.access2.thread.0,
            self.overlapping_range.0,
            self.overlapping_range.1,
            self.kind,
            self.hb_relation,
        )
    }
}

// ---------------------------------------------------------------------------
// DeadlockWarning — a potential deadlock
// ---------------------------------------------------------------------------

/// A warning about a potential deadlock due to lock order reversal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadlockWarning {
    /// The first thread involved.
    pub thread1: ThreadId,
    /// The second thread involved.
    pub thread2: ThreadId,
    /// The first lock (acquired by thread1 first, thread2 second).
    pub lock1: u64,
    /// The second lock (acquired by thread2 first, thread1 second).
    pub lock2: u64,
    /// Human-readable description.
    pub description: String,
}

// ---------------------------------------------------------------------------
// HappensBeforeGraph — the happens-before relation between accesses
// ---------------------------------------------------------------------------

/// A graph representing the happens-before partial order over accesses.
///
/// Constructed from sync edges, thread spawn edges, thread join edges,
/// and their transitive closure.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HappensBeforeGraph {
    /// Maps (access_before, access_after) to the ordering that establishes it.
    edges: HashMap<(AccessId, AccessId), SyncOrdering>,
}

impl HappensBeforeGraph {
    /// Construct the happens-before graph from the input.
    ///
    /// The graph is built from:
    /// 1. Explicit sync edges (direct ordering),
    /// 2. Thread spawn edges (parent's accesses happen-before child's),
    /// 3. Thread join edges (joinee's accesses happen-before joiner's post-join accesses),
    /// 4. Transitive closure (if A→B and B→C then A→C).
    pub fn from_input(input: &ConcurrentExclusivityInput) -> Self {
        let mut graph = HappensBeforeGraph::default();

        // Step 1: Add direct sync edges.
        for edge in &input.sync_edges {
            graph.edges.insert(
                (edge.access_before, edge.access_after),
                edge.ordering.clone(),
            );
        }

        // Step 2: Add happens-before from thread spawn edges.
        // When thread parent spawns child, all of parent's accesses that appear
        // before the spawn point happen-before all of child's accesses.
        // For simplicity: all parent accesses → all child accesses.
        let parent_accesses: HashMap<ThreadId, Vec<AccessId>> = {
            let mut map: HashMap<ThreadId, Vec<AccessId>> = HashMap::new();
            for ta in &input.accesses {
                map.entry(ta.thread.clone())
                    .or_default()
                    .push(ta.access.id);
            }
            map
        };

        for (parent, child, _point) in &input.thread_spawn_edges {
            if let (Some(parent_ids), Some(child_ids)) =
                (parent_accesses.get(parent), parent_accesses.get(child))
            {
                for &pid in parent_ids {
                    for &cid in child_ids {
                        graph
                            .edges
                            .entry((pid, cid))
                            .or_insert(SyncOrdering::HappensBefore);
                    }
                }
            }
        }

        // Step 3: Add happens-before from thread join edges.
        // When joiner joins joinee, all of joinee's accesses happen-before
        // all of joiner's accesses (conservative: post-join ordering).
        for (joiner, joinee, _point) in &input.thread_join_edges {
            if let (Some(joinee_ids), Some(joiner_ids)) =
                (parent_accesses.get(joinee), parent_accesses.get(joiner))
            {
                for &jid in joinee_ids {
                    for &rid in joiner_ids {
                        graph
                            .edges
                            .entry((jid, rid))
                            .or_insert(SyncOrdering::HappensBefore);
                    }
                }
            }
        }

        // Step 4: Compute transitive closure.
        // Collect all access IDs present in the graph.
        let all_access_ids: HashSet<AccessId> = graph
            .edges
            .keys()
            .flat_map(|(a, b)| [*a, *b])
            .collect();

        // Build adjacency list for BFS-based transitive closure.
        let mut adj: HashMap<AccessId, Vec<AccessId>> = HashMap::new();
        for &(from, to) in graph.edges.keys() {
            adj.entry(from).or_default().push(to);
        }

        // BFS from each node to find all reachable nodes.
        let mut new_edges: Vec<((AccessId, AccessId), SyncOrdering)> = Vec::new();
        for &start in &all_access_ids {
            let mut visited = HashSet::new();
            let mut stack = vec![start];
            while let Some(current) = stack.pop() {
                if visited.contains(&current) {
                    continue;
                }
                visited.insert(current);
                if let Some(neighbors) = adj.get(&current) {
                    for &neighbor in neighbors {
                        if !visited.contains(&neighbor) {
                            stack.push(neighbor);
                        }
                        // Record transitive edge from start to neighbor
                        if start != neighbor && !graph.edges.contains_key(&(start, neighbor)) {
                            new_edges.push((
                                (start, neighbor),
                                SyncOrdering::HappensBefore,
                            ));
                        }
                    }
                }
            }
        }

        for ((from, to), ordering) in new_edges {
            graph.edges.entry((from, to)).or_insert(ordering);
        }

        graph
    }

    /// Returns `true` if there is a happens-before ordering from `a1` to `a2`
    /// or from `a2` to `a1`.
    pub fn is_ordered(&self, a1: AccessId, a2: AccessId) -> bool {
        self.edges.contains_key(&(a1, a2)) || self.edges.contains_key(&(a2, a1))
    }

    /// Returns `true` if the two accesses are concurrent (neither A→B nor B→A).
    pub fn are_concurrent(&self, a1: AccessId, a2: AccessId) -> bool {
        !self.is_ordered(a1, a2)
    }

    /// Returns the number of ordering edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Returns the ordering between two accesses, if any.
    pub fn get_ordering(&self, a1: AccessId, a2: AccessId) -> Option<&SyncOrdering> {
        self.edges.get(&(a1, a2))
    }
}

impl fmt::Display for HappensBeforeGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HappensBeforeGraph {{ edges: {} }}", self.edge_count())
    }
}

// ---------------------------------------------------------------------------
// Data Race Detection
// ---------------------------------------------------------------------------

/// Detect data races in the given concurrent exclusivity input.
///
/// A data race exists between two accesses when:
/// 1. They target overlapping byte ranges,
/// 2. At least one is a write,
/// 3. They come from different threads,
/// 4. They are not ordered by a happens-before relationship,
/// 5. They are not both protected by the same mutex.
pub fn detect_data_races(input: &ConcurrentExclusivityInput) -> Vec<DataRace> {
    let hb_graph = HappensBeforeGraph::from_input(input);
    let mut races = Vec::new();

    // Collect all accesses from sync edges with Mutex ordering to determine
    // which accesses are protected by the same lock.
    let mut lock_groups: HashMap<u64, HashSet<AccessId>> = HashMap::new();
    for edge in &input.sync_edges {
        if let SyncOrdering::Mutex(lock_id) = &edge.ordering {
            lock_groups
                .entry(*lock_id)
                .or_default()
                .insert(edge.access_before);
            lock_groups
                .entry(*lock_id)
                .or_default()
                .insert(edge.access_after);
        }
    }

    // Check all pairs of accesses from different threads.
    let n = input.accesses.len();
    for i in 0..n {
        for j in (i + 1)..n {
            let ta1 = &input.accesses[i];
            let ta2 = &input.accesses[j];

            // Skip if same thread — same-thread accesses are inherently ordered.
            if ta1.thread == ta2.thread {
                continue;
            }

            // Skip if both are reads.
            if ta1.access.kind == AccessKind::Read && ta2.access.kind == AccessKind::Read {
                continue;
            }

            // Skip if byte ranges don't overlap.
            if !ta1.access.overlaps(&ta2.access) {
                continue;
            }

            // Check happens-before relationship.
            if hb_graph.is_ordered(ta1.access.id, ta2.access.id) {
                continue;
            }

            // Check if both are protected by the same mutex.
            let same_mutex = lock_groups.values().any(|group| {
                group.contains(&ta1.access.id) && group.contains(&ta2.access.id)
            });
            if same_mutex {
                continue;
            }

            // Determine conflict kind.
            let a1_has_write = ta1.access.kind == AccessKind::Write;
            let a2_has_write = ta2.access.kind == AccessKind::Write;

            // Also consider CapD info.
            let cap1 = input.capabilities.get(&ta1.access.id);
            let cap2 = input.capabilities.get(&ta2.access.id);

            let a1_effective_write = a1_has_write
                || cap1.map_or(false, |c| {
                    c.has_write() && c.write_requires_lock.is_none()
                });
            let a2_effective_write = a2_has_write
                || cap2.map_or(false, |c| {
                    c.has_write() && c.write_requires_lock.is_none()
                });

            let kind = if a1_effective_write && a2_effective_write {
                ConflictKind::WriteWrite
            } else if a1_effective_write || a2_effective_write {
                ConflictKind::WriteRead
            } else {
                continue;
            };

            // Compute overlapping range.
            let (s_start, s_end) = ta1.access.byte_range();
            let (o_start, o_end) = ta2.access.byte_range();
            let overlap_start = s_start.max(o_start);
            let overlap_end = s_end.min(o_end);

            races.push(DataRace {
                access1: ta1.clone(),
                access2: ta2.clone(),
                overlapping_range: (overlap_start, overlap_end),
                kind,
                hb_relation: HBRelation::Concurrent,
            });
        }
    }

    races
}

// ---------------------------------------------------------------------------
// Deadlock Detection
// ---------------------------------------------------------------------------

/// Detect potential deadlocks caused by lock order reversal.
///
/// Two threads can deadlock if:
/// - Thread1 acquires lock1 then lock2,
/// - Thread2 acquires lock2 then lock1.
///
/// This function examines the sync edges for Mutex orderings and
/// detects when two threads acquire the same pair of locks in
/// different orders.
pub fn detect_potential_deadlocks(input: &ConcurrentExclusivityInput) -> Vec<DeadlockWarning> {
    let mut warnings = Vec::new();

    // Build a map of (thread, lock) -> access IDs in order of appearance.
    // We track the order in which each thread acquires locks.
    let access_thread_map = input.access_thread_map();

    // Collect (thread, lock, access_id) tuples from Mutex sync edges,
    // preserving the access_before as the lock acquisition point.
    let mut thread_lock_acquisitions: Vec<(ThreadId, u64, AccessId)> = Vec::new();

    for edge in &input.sync_edges {
        if let SyncOrdering::Mutex(lock_id) = &edge.ordering {
            // access_before and access_after both happen under this lock.
            // The acquisition point is the earlier one.
            if let Some(thread) = access_thread_map.get(&edge.access_before) {
                thread_lock_acquisitions.push((thread.clone(), *lock_id, edge.access_before));
            }
            if let Some(thread) = access_thread_map.get(&edge.access_after) {
                thread_lock_acquisitions.push((thread.clone(), *lock_id, edge.access_after));
            }
        }
    }

    // Sort by access ID (as a proxy for program order) within each thread.
    thread_lock_acquisitions.sort_by_key(|t| t.2);

    // Build per-thread lock acquisition order.
    let mut thread_lock_order: HashMap<ThreadId, Vec<(u64, AccessId)>> = HashMap::new();
    for (thread, lock, access_id) in &thread_lock_acquisitions {
        let order = thread_lock_order.entry(thread.clone()).or_default();
        // Only add the first acquisition of a given lock by this thread.
        if !order.iter().any(|(l, _)| l == lock) {
            order.push((*lock, *access_id));
        }
    }

    // Check for lock order reversal between pairs of threads.
    let threads: Vec<&ThreadId> = thread_lock_order.keys().collect();
    for i in 0..threads.len() {
        for j in (i + 1)..threads.len() {
            let t1 = threads[i];
            let t2 = threads[j];
            let order1 = &thread_lock_order[t1];
            let order2 = &thread_lock_order[t2];

            // Build a map from lock to its position in each thread's acquisition order.
            let pos1: HashMap<u64, usize> = order1
                .iter()
                .enumerate()
                .map(|(pos, (lock, _))| (*lock, pos))
                .collect();
            let pos2: HashMap<u64, usize> = order2
                .iter()
                .enumerate()
                .map(|(pos, (lock, _))| (*lock, pos))
                .collect();

            // Find locks acquired by both threads.
            let common_locks: Vec<u64> = pos1
                .keys()
                .filter(|l| pos2.contains_key(*l))
                .copied()
                .collect();

            // Check all pairs of common locks for order reversal.
            for a in 0..common_locks.len() {
                for b in (a + 1)..common_locks.len() {
                    let lock_a = common_locks[a];
                    let lock_b = common_locks[b];

                    let t1_a = pos1[&lock_a];
                    let t1_b = pos1[&lock_b];
                    let t2_a = pos2[&lock_a];
                    let t2_b = pos2[&lock_b];

                    // If t1 acquires lock_a before lock_b, but t2 acquires lock_b before lock_a,
                    // that's a potential deadlock.
                    if (t1_a < t1_b && t2_a > t2_b) || (t1_a > t1_b && t2_a < t2_b) {
                        // Determine which lock is "first" for thread1.
                        let (first_lock, second_lock) = if t1_a < t1_b {
                            (lock_a, lock_b)
                        } else {
                            (lock_b, lock_a)
                        };

                        warnings.push(DeadlockWarning {
                            thread1: t1.clone(),
                            thread2: t2.clone(),
                            lock1: first_lock,
                            lock2: second_lock,
                            description: format!(
                                "Thread {} acquires lock {} then {}, but Thread {} acquires lock {} then {} — potential deadlock",
                                t1, first_lock, second_lock, t2, second_lock, first_lock
                            ),
                        });
                    }
                }
            }
        }
    }

    warnings
}

// ---------------------------------------------------------------------------
// ConcurrentExclusivityOutput — the output of concurrent exclusivity verification
// ---------------------------------------------------------------------------

/// The output of the concurrent exclusivity verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConcurrentExclusivityOutput {
    /// The verification result.
    pub result: VerificationResult,
    /// The happens-before graph used for the analysis.
    pub hb_graph: HappensBeforeGraph,
    /// All detected data races.
    pub data_races: Vec<DataRace>,
    /// All detected potential deadlocks.
    pub deadlock_warnings: Vec<DeadlockWarning>,
    /// The interference graph of conflicting accesses.
    pub interference_graph: InterferenceGraph,
}

impl ConcurrentExclusivityOutput {
    /// Returns `true` if no data races were detected.
    pub fn is_race_free(&self) -> bool {
        self.data_races.is_empty()
    }

    /// Returns the number of detected data races.
    pub fn race_count(&self) -> usize {
        self.data_races.len()
    }

    /// Returns the number of write-write data races.
    pub fn write_write_race_count(&self) -> usize {
        self.data_races
            .iter()
            .filter(|r| r.kind == ConflictKind::WriteWrite)
            .count()
    }

    /// Returns the number of write-read data races.
    pub fn write_read_race_count(&self) -> usize {
        self.data_races
            .iter()
            .filter(|r| r.kind == ConflictKind::WriteRead)
            .count()
    }

    /// Returns the number of deadlock warnings.
    pub fn deadlock_count(&self) -> usize {
        self.deadlock_warnings.len()
    }
}

impl fmt::Display for ConcurrentExclusivityOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ConcurrentExclusivityOutput {{ result: {}, races: {}, deadlocks: {} }}",
            self.result,
            self.race_count(),
            self.deadlock_count(),
        )
    }
}

// ---------------------------------------------------------------------------
// ConcurrentExclusivityVerifier — the main concurrent verifier
// ---------------------------------------------------------------------------

/// The concurrent exclusivity invariant verifier.
///
/// Extends the single-threaded exclusivity check with thread-aware analysis,
/// building a happens-before graph and detecting data races and potential
/// deadlocks.
pub struct ConcurrentExclusivityVerifier {
    /// Whether to emit detailed diagnostic logging.
    verbose: bool,
}

impl ConcurrentExclusivityVerifier {
    /// Create a new concurrent exclusivity verifier.
    pub fn new() -> Self {
        Self { verbose: false }
    }

    /// Enable verbose diagnostic output.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Verify the concurrent exclusivity invariant against the given input.
    ///
    /// This method:
    /// 1. Builds the happens-before graph from sync edges, spawn/join edges,
    ///    and transitive closure.
    /// 2. Identifies concurrent (unordered) accesses.
    /// 3. Detects data races among concurrent accesses.
    /// 4. Detects potential deadlocks from lock order reversals.
    /// 5. Returns a structured [`ConcurrentExclusivityOutput`].
    pub fn verify(&self, input: &ConcurrentExclusivityInput) -> ConcurrentExclusivityOutput {
        // Step 1: Build happens-before graph.
        let hb_graph = HappensBeforeGraph::from_input(input);

        if self.verbose {
            log::info!(
                "ConcurrentExclusivityVerifier: HB graph has {} edges",
                hb_graph.edge_count()
            );
        }

        // Step 2: Detect data races.
        let data_races = detect_data_races(input);

        // Step 3: Detect potential deadlocks.
        let deadlock_warnings = detect_potential_deadlocks(input);

        // Step 4: Build interference graph from data races.
        let mut interference_graph = InterferenceGraph::new();
        for race in &data_races {
            use crate::exclusivity::Conflict;
            let (overlap_start, overlap_end) = race.overlapping_range;
            let conflict = Conflict::new(
                race.access1.access.id,
                race.access2.access.id,
                race.kind.clone(),
                overlap_start,
                overlap_end,
                format!("{}", race),
            );
            interference_graph.add_conflict(conflict);
        }

        // Step 5: Determine verification status.
        let status = if data_races.is_empty() && deadlock_warnings.is_empty() {
            VerificationStatus::Proven
        } else if !data_races.is_empty() {
            // Create a counterexample from the first data race.
            let first_race = &data_races[0];
            let ce = CounterExample::new(
                vec![
                    format!(
                        "{} on {}",
                        first_race.access1.access.id, first_race.access1.thread
                    ),
                    format!(
                        "{} on {}",
                        first_race.access2.access.id, first_race.access2.thread
                    ),
                ],
                format!("{}", first_race.access1.access.id),
                format!(
                    "Data race: {} between {} on {} and {} on {} at [0x{:x}, 0x{:x})",
                    first_race.kind,
                    first_race.access1.access.id,
                    first_race.access1.thread,
                    first_race.access2.access.id,
                    first_race.access2.thread,
                    first_race.overlapping_range.0,
                    first_race.overlapping_range.1,
                ),
            );
            VerificationStatus::Violated {
                counterexample: ce,
            }
        } else {
            // Deadlock warnings but no data races.
            VerificationStatus::ProbablySafe {
                assumptions: deadlock_warnings
                    .iter()
                    .map(|w| w.description.clone())
                    .collect(),
            }
        };

        let message = format!(
            "concurrent exclusivity check: {} data race(s), {} deadlock warning(s)",
            data_races.len(),
            deadlock_warnings.len()
        );

        let result = VerificationResult::new("concurrent_exclusivity", status, message)
            .with_evidence(Evidence::ExhaustiveAnalysis);

        ConcurrentExclusivityOutput {
            result,
            hb_graph,
            data_races,
            deadlock_warnings,
            interference_graph,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exclusivity::{AccessRecord, CapDInfo, SyncEdgeRecord, SyncOrdering};
    use crate::result::ProgramPoint;

    /// Helper to create an AccessRecord quickly.
    fn make_access(id: u64, kind: AccessKind, base: u64, size: u64) -> AccessRecord {
        AccessRecord::new(
            AccessId(id),
            kind,
            base,
            size,
            ProgramPoint::from(format!("pp_{}", id)),
            id,
            1,
        )
    }

    /// Test 1: Same-thread accesses are not data races.
    #[test]
    fn same_thread_accesses_are_not_data_races() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);

        // Two writes on the same thread to overlapping ranges.
        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Write, 0x100, 8),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Write, 0x104, 8),
            t1.clone(),
        ));

        let races = detect_data_races(&input);
        assert!(
            races.is_empty(),
            "Same-thread accesses should not be reported as data races, got {} races",
            races.len()
        );
    }

    /// Test 2: Different-thread concurrent writes are data races.
    #[test]
    fn different_thread_concurrent_writes_are_data_races() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);
        let t2 = ThreadId(2);

        // Two writes on different threads to overlapping ranges, no sync.
        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Write, 0x100, 8),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Write, 0x104, 8),
            t2.clone(),
        ));

        let races = detect_data_races(&input);
        assert_eq!(races.len(), 1, "Should detect exactly one data race");
        assert_eq!(races[0].kind, ConflictKind::WriteWrite);
        assert_eq!(races[0].hb_relation, HBRelation::Concurrent);
    }

    /// Test 3: Thread spawn establishes happens-before.
    #[test]
    fn thread_spawn_establishes_happens_before() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);
        let t2 = ThreadId(2);

        // Write on parent thread, write on child thread.
        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Write, 0x100, 8),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Write, 0x104, 8),
            t2.clone(),
        ));

        // t1 spawns t2 — parent's ops happen-before child's.
        input.add_spawn_edge(t1.clone(), t2.clone(), "spawn_point".into());

        let races = detect_data_races(&input);
        assert!(
            races.is_empty(),
            "Spawn edge should establish happens-before, eliminating the race, but got {} races",
            races.len()
        );

        // Also verify the HB graph directly.
        let hb = HappensBeforeGraph::from_input(&input);
        assert!(
            hb.is_ordered(AccessId(1), AccessId(2)),
            "Spawn should create ordering from parent to child"
        );
    }

    /// Test 4: Thread join establishes happens-before.
    #[test]
    fn thread_join_establishes_happens_before() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);
        let t2 = ThreadId(2);

        // Write on child thread, write on parent (joiner) thread.
        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Write, 0x100, 8),
            t2.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Write, 0x104, 8),
            t1.clone(),
        ));

        // t1 joins t2 — joinee's ops happen-before joiner's.
        input.add_join_edge(t1.clone(), t2.clone(), "join_point".into());

        let races = detect_data_races(&input);
        assert!(
            races.is_empty(),
            "Join edge should establish happens-before, eliminating the race, but got {} races",
            races.len()
        );

        // Verify the HB graph directly.
        let hb = HappensBeforeGraph::from_input(&input);
        assert!(
            hb.is_ordered(AccessId(1), AccessId(2)),
            "Join should create ordering from joinee to joiner"
        );
    }

    /// Test 5: Lock-protected concurrent access is safe.
    #[test]
    fn lock_protected_concurrent_access_is_safe() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);
        let t2 = ThreadId(2);

        // Two writes on different threads to overlapping ranges.
        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Write, 0x100, 8),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Write, 0x104, 8),
            t2.clone(),
        ));

        // Both protected by the same mutex.
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(1),
            AccessId(2),
            SyncOrdering::Mutex(42),
        ));

        let races = detect_data_races(&input);
        assert!(
            races.is_empty(),
            "Mutex-protected accesses should not be reported as data races, got {} races",
            races.len()
        );
    }

    /// Test 6: Transitive happens-before.
    #[test]
    fn transitive_happens_before() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);
        let t2 = ThreadId(2);
        let t3 = ThreadId(3);

        // A1 on t1, A2 on t2, A3 on t3 — all writes to overlapping ranges.
        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Write, 0x100, 8),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Write, 0x104, 8),
            t2.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(3, AccessKind::Write, 0x108, 8),
            t3.clone(),
        ));

        // A1 → A2 (sync edge), A2 → A3 (sync edge), so A1 → A3 (transitive).
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(1),
            AccessId(2),
            SyncOrdering::HappensBefore,
        ));
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(2),
            AccessId(3),
            SyncOrdering::HappensBefore,
        ));

        let hb = HappensBeforeGraph::from_input(&input);

        // A1 → A2: direct
        assert!(hb.is_ordered(AccessId(1), AccessId(2)));
        // A2 → A3: direct
        assert!(hb.is_ordered(AccessId(2), AccessId(3)));
        // A1 → A3: transitive
        assert!(
            hb.is_ordered(AccessId(1), AccessId(3)),
            "Transitive HB should order A1 before A3"
        );

        // No data races because all are ordered.
        let races = detect_data_races(&input);
        assert!(
            races.is_empty(),
            "Transitive ordering should eliminate all races, got {} races",
            races.len()
        );
    }

    /// Test 7: Deadlock detection for lock order reversal.
    #[test]
    fn deadlock_detection_for_lock_order_reversal() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);
        let t2 = ThreadId(2);

        // Accesses on both threads.
        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Write, 0x100, 8),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Write, 0x200, 8),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(3, AccessKind::Write, 0x100, 8),
            t2.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(4, AccessKind::Write, 0x200, 8),
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
            AccessId(1), // dummy to associate A2 with lock 20
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
            AccessId(3), // dummy to associate A4 with lock 10
            SyncOrdering::Mutex(10),
        ));

        let deadlocks = detect_potential_deadlocks(&input);
        assert!(
            !deadlocks.is_empty(),
            "Should detect deadlock from lock order reversal"
        );
    }

    /// Test 8: No data races when all accesses are ordered.
    #[test]
    fn no_data_races_when_all_accesses_ordered() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);
        let t2 = ThreadId(2);
        let t3 = ThreadId(3);

        // Multiple writes on different threads to overlapping ranges.
        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Write, 0x100, 16),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Read, 0x104, 8),
            t2.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(3, AccessKind::Write, 0x108, 8),
            t3.clone(),
        ));

        // Chain of ordering: A1 → A2 → A3.
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(1),
            AccessId(2),
            SyncOrdering::HappensBefore,
        ));
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(2),
            AccessId(3),
            SyncOrdering::Atomic,
        ));

        let races = detect_data_races(&input);
        assert!(
            races.is_empty(),
            "All ordered accesses should produce no data races, got {} races",
            races.len()
        );
    }

    /// Additional test: Write-read race on different threads.
    #[test]
    fn write_read_race_on_different_threads() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);
        let t2 = ThreadId(2);

        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Write, 0x100, 8),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Read, 0x104, 8),
            t2.clone(),
        ));

        let races = detect_data_races(&input);
        assert_eq!(races.len(), 1);
        assert_eq!(races[0].kind, ConflictKind::WriteRead);
    }

    /// Additional test: ConcurrentExclusivityVerifier full pipeline.
    #[test]
    fn verifier_full_pipeline() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);
        let t2 = ThreadId(2);

        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Write, 0x100, 8),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Write, 0x104, 8),
            t2.clone(),
        ));

        let verifier = ConcurrentExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(!output.is_race_free());
        assert_eq!(output.race_count(), 1);
        assert!(output.result.is_violated());
    }

    /// Additional test: Verifier with no issues returns Proven.
    #[test]
    fn verifier_proven_when_no_races() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);
        let t2 = ThreadId(2);

        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Write, 0x100, 8),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Write, 0x104, 8),
            t2.clone(),
        ));
        input.add_spawn_edge(t1.clone(), t2.clone(), "spawn".into());

        let verifier = ConcurrentExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(output.is_race_free());
        assert!(output.result.is_proven());
    }

    /// Additional test: Non-overlapping accesses from different threads are not races.
    #[test]
    fn non_overlapping_accesses_not_races() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);
        let t2 = ThreadId(2);

        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Write, 0x100, 8),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Write, 0x200, 8),
            t2.clone(),
        ));

        let races = detect_data_races(&input);
        assert!(
            races.is_empty(),
            "Non-overlapping accesses should not be races"
        );
    }

    /// Additional test: Two reads from different threads are not races.
    #[test]
    fn two_reads_not_race() {
        let mut input = ConcurrentExclusivityInput::new();
        let t1 = ThreadId(1);
        let t2 = ThreadId(2);

        input.add_access(ThreadAccess::new(
            make_access(1, AccessKind::Read, 0x100, 8),
            t1.clone(),
        ));
        input.add_access(ThreadAccess::new(
            make_access(2, AccessKind::Read, 0x104, 8),
            t2.clone(),
        ));

        let races = detect_data_races(&input);
        assert!(
            races.is_empty(),
            "Two reads should not constitute a race"
        );
    }
}
