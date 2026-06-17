//! Exclusivity Invariant Verifier for the IVE module.
//!
//! This module implements the **exclusivity** invariant of the VUMA safety model:
//!
//! > **"At most one owner for exclusive resources."**
//!
//! Concretely, this means:
//!
//! - No two concurrent write accesses to overlapping memory ranges.
//! - No concurrent read + write to overlapping ranges without synchronization.
//! - Exclusive (mutable) pointers must not alias.
//! - In the CapD lattice, if two accesses both have Write capability and
//!   overlap, they must be ordered by a sync edge.
//!
//! # Algorithm
//!
//! 1. Collect all access records from the input.
//! 2. Compute two ordering relations (each a transitive closure):
//!    - `sync_ordered`: pairs ordered by **synchronization** edges
//!      (mutex lock/unlock, atomic acquire/release, channel send/recv).
//!      These establish cross-thread happens-before.
//!    - `program_ordered`: pairs ordered by **sequential control flow**
//!      (single-threaded program order). These order accesses within
//!      one thread but do **not** establish happens-before across threads.
//! 3. Collect the `mutually_exclusive` set — pairs of accesses that can
//!    never both execute on any single run (e.g., accesses in different
//!    arms of an `if`).
//! 4. For every pair of accesses `(a1, a2)`:
//!    a. Skip if both are reads (reads never conflict).
//!    b. Skip if their byte ranges do not overlap.
//!    c. Skip if they are ordered by a sync edge (in either direction).
//!    d. Skip if they are ordered by program-order (in either direction).
//!       Sequential execution provides ordering for single-threaded code.
//!    e. Skip if they are mutually exclusive (cannot both execute).
//!    f. Otherwise, check CapD permissions: if both have Write → write-write
//!    data race; if one Write + one Read → read-write race.
//! 5. Build an interference graph from all detected conflicts.
//! 6. Return a structured [`VerificationResult`].
//!
//! # Why program-order is separate from sync edges
//!
//! Treating ordinary ControlFlow as a *synchronization* edge is wrong: a
//! well-formed single-threaded CFG transitively orders all accesses, which
//! would make Exclusivity vacuously `Proven` and hide real data races.
//! Sync edges must come from *actual* synchronization operations
//! (mutex lock/unlock, atomic RMWs, channel send/recv) — only these
//! establish happens-before across threads. Program-order edges capture
//! sequential ordering, which is sufficient to rule out conflicts in
//! single-threaded code but does *not* synchronize concurrent threads.
//!
//! # Interference Graph
//!
//! The interference graph is an undirected graph where each node is an
//! [`AccessId`] and each edge represents a conflict between two accesses.
//! This graph can be used for coloring-based alias analysis or for
//! reporting violation clusters to the user.

use crate::result::{
    CounterExample, Evidence, ProgramPoint, VerificationResult, VerificationStatus,
};
use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// AccessId — unique identifier for a memory access
// ---------------------------------------------------------------------------

/// Unique identifier for a memory access event within the exclusivity checker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AccessId(pub u64);

impl fmt::Display for AccessId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "A{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// AccessKind — read or write
// ---------------------------------------------------------------------------

/// The kind of memory access: read or write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccessKind {
    /// A read from memory.
    Read,
    /// A write to memory.
    Write,
}

impl fmt::Display for AccessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AccessKind::Read => write!(f, "read"),
            AccessKind::Write => write!(f, "write"),
        }
    }
}

// ---------------------------------------------------------------------------
// SyncOrdering — the kind of synchronization between two accesses
// ---------------------------------------------------------------------------

/// The kind of ordering constraint established by a sync edge.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SyncOrdering {
    /// `access_before` happens-before `access_after` (sequential order).
    HappensBefore,
    /// An atomic acquire-release pair.
    Atomic,
    /// Both accesses occur while the same mutex (identified by lock ID) is held.
    Mutex(u64),
}

impl fmt::Display for SyncOrdering {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncOrdering::HappensBefore => write!(f, "happens-before"),
            SyncOrdering::Atomic => write!(f, "atomic"),
            SyncOrdering::Mutex(id) => write!(f, "mutex({})", id),
        }
    }
}

// ---------------------------------------------------------------------------
// AccessRecord — a single memory access event
// ---------------------------------------------------------------------------

/// A record of a single memory access event for exclusivity analysis.
///
/// Each access targets a derived pointer, has a kind (read/write), a base
/// address and size determining the byte range, and a program point where
/// it occurs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessRecord {
    /// Unique identifier for this access.
    pub id: AccessId,
    /// Read or write.
    pub kind: AccessKind,
    /// The resolved base address of the target derivation.
    pub base_address: u64,
    /// Number of bytes accessed.
    pub size: u64,
    /// Source location where this access occurs.
    pub program_point: ProgramPoint,
    /// The derivation ID that this access targets (for provenance tracing).
    pub derivation_id: u64,
    /// The region ID that this access ultimately traces to.
    pub region_id: u64,
}

impl AccessRecord {
    /// Create a new access record.
    pub fn new(
        id: AccessId,
        kind: AccessKind,
        base_address: u64,
        size: u64,
        program_point: ProgramPoint,
        derivation_id: u64,
        region_id: u64,
    ) -> Self {
        Self {
            id,
            kind,
            base_address,
            size,
            program_point,
            derivation_id,
            region_id,
        }
    }

    /// Returns the byte range `[base_address, base_address + size)`.
    pub fn byte_range(&self) -> (u64, u64) {
        (self.base_address, self.base_address + self.size)
    }

    /// Returns `true` if the byte range of this access overlaps with `other`.
    pub fn overlaps(&self, other: &AccessRecord) -> bool {
        let (s_start, s_end) = self.byte_range();
        let (o_start, o_end) = other.byte_range();
        s_start < o_end && o_start < s_end
    }

    /// Returns `true` if this access and `other` conflict.
    ///
    /// Two accesses conflict when:
    /// 1. Their byte ranges overlap, **and**
    /// 2. At least one of them is a write.
    pub fn conflicts_with(&self, other: &AccessRecord) -> bool {
        self.overlaps(other) && (self.kind == AccessKind::Write || other.kind == AccessKind::Write)
    }
}

// ---------------------------------------------------------------------------
// SyncEdge — a synchronization relationship between two accesses
// ---------------------------------------------------------------------------

/// A synchronization edge between two accesses.
///
/// Records that `access_before` is ordered before `access_after` according
/// to the given [`SyncOrdering`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEdgeRecord {
    /// The access that happens first.
    pub access_before: AccessId,
    /// The access that happens second.
    pub access_after: AccessId,
    /// The kind of ordering constraint.
    pub ordering: SyncOrdering,
}

impl SyncEdgeRecord {
    /// Create a new sync edge record.
    pub fn new(access_before: AccessId, access_after: AccessId, ordering: SyncOrdering) -> Self {
        Self {
            access_before,
            access_after,
            ordering,
        }
    }
}

// ---------------------------------------------------------------------------
// CapDInfo — simplified capability descriptor for exclusivity checking
// ---------------------------------------------------------------------------

/// A simplified capability descriptor for exclusivity analysis.
///
/// This mirrors the CapD lattice from the `bd` crate but focuses on the
/// capabilities relevant to exclusivity: Read, Write, and any lock
/// conditions that gate them.
///
/// In the full CapD lattice:
/// - `⊥` (bottom) = no capabilities
/// - `⊤` (top) = all capabilities
/// - **meet** = intersection of capabilities, union of conditions
/// - **join** = union of capabilities, intersection of conditions
///
/// For exclusivity, we care about whether a capability includes Write and
/// whether it is conditioned on holding a specific lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapDInfo {
    /// Whether the Read capability is granted.
    pub can_read: bool,
    /// Whether the Write capability is granted.
    pub can_write: bool,
    /// If the Write capability requires a specific lock, this is the lock ID.
    pub write_requires_lock: Option<u64>,
    /// If the Read capability requires a specific lock, this is the lock ID.
    pub read_requires_lock: Option<u64>,
}

impl CapDInfo {
    /// A CapD with only Read capability, no conditions.
    pub fn read_only() -> Self {
        Self {
            can_read: true,
            can_write: false,
            write_requires_lock: None,
            read_requires_lock: None,
        }
    }

    /// A CapD with only Write capability, no conditions.
    pub fn write_only() -> Self {
        Self {
            can_read: false,
            can_write: true,
            write_requires_lock: None,
            read_requires_lock: None,
        }
    }

    /// A CapD with both Read and Write capabilities, no conditions.
    pub fn read_write() -> Self {
        Self {
            can_read: true,
            can_write: true,
            write_requires_lock: None,
            read_requires_lock: None,
        }
    }

    /// A CapD with Write capability conditioned on holding the given lock.
    pub fn write_locked(lock_id: u64) -> Self {
        Self {
            can_read: true,
            can_write: true,
            write_requires_lock: Some(lock_id),
            read_requires_lock: None,
        }
    }

    /// An empty CapD (no capabilities).
    pub fn empty() -> Self {
        Self {
            can_read: false,
            can_write: false,
            write_requires_lock: None,
            read_requires_lock: None,
        }
    }

    /// Returns `true` if this CapD has Write capability (possibly conditional).
    pub fn has_write(&self) -> bool {
        self.can_write
    }

    /// Returns `true` if this CapD has Read capability (possibly conditional).
    pub fn has_read(&self) -> bool {
        self.can_read
    }

    /// Returns `true` if the Write capability is effectively active given
    /// that the specified set of locks are currently held.
    pub fn is_write_active(&self, held_locks: &HashSet<u64>) -> bool {
        if !self.can_write {
            return false;
        }
        match self.write_requires_lock {
            Some(lock_id) => held_locks.contains(&lock_id),
            None => true,
        }
    }

    /// Returns `true` if the Read capability is effectively active given
    /// that the specified set of locks are currently held.
    pub fn is_read_active(&self, held_locks: &HashSet<u64>) -> bool {
        if !self.can_read {
            return false;
        }
        match self.read_requires_lock {
            Some(lock_id) => held_locks.contains(&lock_id),
            None => true,
        }
    }

    /// **Meet** (greatest lower bound) in the capability lattice.
    ///
    /// Capabilities: intersection. Conditions: union (more restrictive).
    pub fn meet(&self, other: &CapDInfo) -> CapDInfo {
        CapDInfo {
            can_read: self.can_read && other.can_read,
            can_write: self.can_write && other.can_write,
            write_requires_lock: match (self.write_requires_lock, other.write_requires_lock) {
                (Some(a), Some(b)) if a == b => Some(a),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
                (Some(a), Some(_)) => Some(a), // different locks → union: pick one (conservative)
            },
            read_requires_lock: match (self.read_requires_lock, other.read_requires_lock) {
                (Some(a), Some(b)) if a == b => Some(a),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
                (Some(a), Some(_)) => Some(a),
            },
        }
    }

    /// **Join** (least upper bound) in the capability lattice.
    ///
    /// Capabilities: union. Conditions: intersection (less restrictive).
    pub fn join(&self, other: &CapDInfo) -> CapDInfo {
        CapDInfo {
            can_read: self.can_read || other.can_read,
            can_write: self.can_write || other.can_write,
            write_requires_lock: match (self.write_requires_lock, other.write_requires_lock) {
                (Some(a), Some(b)) if a == b => Some(a),
                _ => None, // different or missing → intersection is empty → no condition
            },
            read_requires_lock: match (self.read_requires_lock, other.read_requires_lock) {
                (Some(a), Some(b)) if a == b => Some(a),
                _ => None,
            },
        }
    }
}

impl fmt::Display for CapDInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CapD{{read={}, write={}", self.can_read, self.can_write)?;
        if let Some(lock) = self.write_requires_lock {
            write!(f, ", write_lock={}", lock)?;
        }
        if let Some(lock) = self.read_requires_lock {
            write!(f, ", read_lock={}", lock)?;
        }
        write!(f, "}}")
    }
}

// ---------------------------------------------------------------------------
// ConflictKind — the type of conflict detected
// ---------------------------------------------------------------------------

/// The kind of exclusivity conflict between two accesses.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConflictKind {
    /// Two concurrent writes to overlapping memory (data race).
    WriteWrite,
    /// A concurrent write and read to overlapping memory without synchronization.
    WriteRead,
}

impl fmt::Display for ConflictKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConflictKind::WriteWrite => write!(f, "write-write conflict"),
            ConflictKind::WriteRead => write!(f, "write-read conflict"),
        }
    }
}

// ---------------------------------------------------------------------------
// Conflict — a detected exclusivity violation
// ---------------------------------------------------------------------------

/// A detected conflict between two concurrent accesses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    /// The first access involved in the conflict.
    pub access1: AccessId,
    /// The second access involved in the conflict.
    pub access2: AccessId,
    /// The kind of conflict.
    pub kind: ConflictKind,
    /// Start address of the overlapping byte range.
    pub overlap_start: u64,
    /// End address (exclusive) of the overlapping byte range.
    pub overlap_end: u64,
    /// Human-readable description of the conflict.
    pub description: String,
}

impl Conflict {
    /// Create a new conflict record.
    pub fn new(
        access1: AccessId,
        access2: AccessId,
        kind: ConflictKind,
        overlap_start: u64,
        overlap_end: u64,
        description: String,
    ) -> Self {
        Self {
            access1,
            access2,
            kind,
            overlap_start,
            overlap_end,
            description,
        }
    }
}

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} between {} and {} at [0x{:x}, 0x{:x}): {}",
            self.kind,
            self.access1,
            self.access2,
            self.overlap_start,
            self.overlap_end,
            self.description
        )
    }
}

// ---------------------------------------------------------------------------
// InterferenceGraph — graph of conflicting accesses
// ---------------------------------------------------------------------------

/// An undirected interference graph where nodes are accesses and edges
/// represent conflicts.
///
/// This graph can be used for:
/// - Coloring-based alias analysis
/// - Reporting violation clusters
/// - Determining which accesses need synchronization
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InterferenceGraph {
    /// Adjacency list: maps each access to the set of accesses it conflicts with.
    edges: HashMap<AccessId, HashSet<AccessId>>,
    /// All conflicts, indexed by the (sorted) pair of access IDs.
    conflicts: HashMap<(AccessId, AccessId), Conflict>,
}

impl InterferenceGraph {
    /// Create an empty interference graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a conflict to the graph.
    pub fn add_conflict(&mut self, conflict: Conflict) {
        let a1 = conflict.access1;
        let a2 = conflict.access2;
        let key = if a1 < a2 { (a1, a2) } else { (a2, a1) };

        self.edges.entry(a1).or_default().insert(a2);
        self.edges.entry(a2).or_default().insert(a1);
        self.conflicts.entry(key).or_insert(conflict);
    }

    /// Returns `true` if two accesses conflict in the graph.
    pub fn are_conflicting(&self, a1: AccessId, a2: AccessId) -> bool {
        let key = if a1 < a2 { (a1, a2) } else { (a2, a1) };
        self.conflicts.contains_key(&key)
    }

    /// Returns the set of accesses that conflict with the given access.
    pub fn neighbors(&self, access: AccessId) -> Option<&HashSet<AccessId>> {
        self.edges.get(&access)
    }

    /// Returns the number of conflicts (edges) in the graph.
    pub fn conflict_count(&self) -> usize {
        self.conflicts.len()
    }

    /// Returns the number of nodes (accesses) in the graph.
    pub fn node_count(&self) -> usize {
        self.edges.len()
    }

    /// Returns an iterator over all conflicts.
    pub fn conflicts(&self) -> impl Iterator<Item = &Conflict> {
        self.conflicts.values()
    }

    /// Returns an iterator over all conflicting access pairs.
    pub fn conflict_pairs(&self) -> impl Iterator<Item = (AccessId, AccessId)> + '_ {
        self.conflicts.keys().copied()
    }

    /// Returns `true` if the graph has no conflicts.
    pub fn is_empty(&self) -> bool {
        self.conflicts.is_empty()
    }

    /// Returns the set of all access IDs that participate in at least one conflict.
    pub fn conflicting_accesses(&self) -> HashSet<AccessId> {
        self.edges.keys().copied().collect()
    }

    /// Compute the connected components of the interference graph.
    ///
    /// Each component is a set of access IDs that are transitively connected
    /// through conflicts. This is useful for grouping related violations.
    pub fn connected_components(&self) -> Vec<HashSet<AccessId>> {
        let mut visited: HashSet<AccessId> = HashSet::new();
        let mut components = Vec::new();

        for &node in self.edges.keys() {
            if visited.contains(&node) {
                continue;
            }
            let mut component = HashSet::new();
            let mut stack = vec![node];
            while let Some(current) = stack.pop() {
                if visited.contains(&current) {
                    continue;
                }
                visited.insert(current);
                component.insert(current);
                if let Some(neighbors) = self.edges.get(&current) {
                    for &neighbor in neighbors {
                        if !visited.contains(&neighbor) {
                            stack.push(neighbor);
                        }
                    }
                }
            }
            components.push(component);
        }

        components
    }
}

impl fmt::Display for InterferenceGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "InterferenceGraph {{ nodes: {}, edges: {} }}",
            self.node_count(),
            self.conflict_count()
        )
    }
}

// ---------------------------------------------------------------------------
// ExclusivityInput — the input to the exclusivity verifier
// ---------------------------------------------------------------------------

/// The input to the exclusivity verifier.
///
/// Contains all memory accesses, synchronization edges, capability
/// descriptors needed to perform the exclusivity check.
#[derive(Debug, Clone, Default)]
pub struct ExclusivityInput {
    /// All memory access events.
    pub accesses: Vec<AccessRecord>,
    /// All **synchronization** edges (mutex lock/unlock, atomic
    /// acquire/release, channel send/recv). These establish cross-thread
    /// happens-before ordering between two accesses.
    ///
    /// Ordinary sequential ControlFlow between two accesses must **not**
    /// be modeled as a sync edge — that would make Exclusivity vacuously
    /// `Proven` for any well-formed single-threaded CFG. Use
    /// [`program_order`](Self::program_order) for that.
    pub sync_edges: Vec<SyncEdgeRecord>,
    /// **Program-order** (sequential ControlFlow) edges between accesses.
    ///
    /// These order accesses within a single thread of execution. For
    /// single-threaded programs, two accesses ordered by program-order do
    /// not conflict (sequential execution provides ordering). However,
    /// program-order does **not** synchronize concurrent threads — so
    /// for multi-threaded code, only `sync_edges` can rule out a
    /// conflict between accesses on different threads.
    pub program_order: Vec<(AccessId, AccessId)>,
    /// Pairs of accesses that **cannot both execute** on any single run
    /// of the program (e.g., accesses in different arms of an `if`, or
    /// in different match arms). Such pairs never conflict regardless of
    /// overlap, because the program can execute at most one of them.
    pub mutually_exclusive: Vec<(AccessId, AccessId)>,
    /// Capability descriptors indexed by access ID.
    pub capabilities: HashMap<AccessId, CapDInfo>,
    /// The set of locks currently held (for CapD condition resolution).
    pub held_locks: HashSet<u64>,
}

impl ExclusivityInput {
    /// Create an empty input.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an access record.
    pub fn add_access(&mut self, access: AccessRecord) {
        self.accesses.push(access);
    }

    /// Add a **synchronization** edge (cross-thread happens-before,
    /// mutex, atomic acquire/release, or channel send/recv) between two
    /// accesses.
    ///
    /// Do **not** use this for ordinary sequential ControlFlow — use
    /// [`add_program_order_edge`](Self::add_program_order_edge) instead.
    pub fn add_sync_edge(&mut self, edge: SyncEdgeRecord) {
        self.sync_edges.push(edge);
    }

    /// Add a **program-order** (sequential ControlFlow) edge between two
    /// accesses. This orders the accesses within a single thread but
    /// does *not* establish happens-before across threads.
    pub fn add_program_order_edge(&mut self, before: AccessId, after: AccessId) {
        self.program_order.push((before, after));
    }

    /// Mark two accesses as **mutually exclusive** — they cannot both
    /// execute on any single run of the program (e.g., they live in
    /// different arms of an `if`). Such pairs never conflict.
    pub fn add_mutually_exclusive_pair(&mut self, a1: AccessId, a2: AccessId) {
        self.mutually_exclusive.push((a1, a2));
    }

    /// Set the CapD for an access.
    pub fn set_capability(&mut self, access_id: AccessId, cap: CapDInfo) {
        self.capabilities.insert(access_id, cap);
    }

    /// Mark a lock as currently held.
    pub fn hold_lock(&mut self, lock_id: u64) {
        self.held_locks.insert(lock_id);
    }
}

// ---------------------------------------------------------------------------
// ExclusivityVerifier — the main verifier
// ---------------------------------------------------------------------------

/// The exclusivity invariant verifier.
///
/// Checks that no two concurrent accesses conflict without synchronization.
/// Builds an interference graph of all detected conflicts and returns a
/// structured [`VerificationResult`].
pub struct ExclusivityVerifier {
    /// Whether to emit detailed diagnostic logging.
    verbose: bool,
}

impl ExclusivityVerifier {
    /// Create a new exclusivity verifier.
    pub fn new() -> Self {
        Self { verbose: false }
    }

    /// Enable verbose diagnostic output.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Verify the exclusivity invariant against the given input.
    ///
    /// Returns a [`VerificationResult`] with:
    /// - `Proven` if no conflicts were detected.
    /// - `Violated` with a counterexample if conflicts exist.
    /// - `ProbablySafe` if conflicts exist but are protected by lock conditions.
    pub fn verify(&self, input: &ExclusivityInput) -> ExclusivityOutput {
        // Step 1: Compute the two ordering relations (transitive closures):
        //   - `sync_ordered`: from synchronization edges (cross-thread HB).
        //   - `program_ordered`: from sequential ControlFlow (intra-thread).
        // And the `mutually_exclusive` set (pairs that can't both execute).
        let sync_ordered = self.compute_ordered_relation(&input.sync_edges);
        let program_ordered = self.compute_pair_closure(&input.program_order);
        let mutex_excl: HashSet<(AccessId, AccessId)> = input
            .mutually_exclusive
            .iter()
            .map(|&(a, b)| if a < b { (a, b) } else { (b, a) })
            .collect();

        // Step 2: Check every pair of accesses for conflicts.
        let mut graph = InterferenceGraph::new();
        let access_count = input.accesses.len();

        for i in 0..access_count {
            for j in (i + 1)..access_count {
                let a1 = &input.accesses[i];
                let a2 = &input.accesses[j];

                // Skip if both are reads — reads never conflict.
                if a1.kind == AccessKind::Read && a2.kind == AccessKind::Read {
                    continue;
                }

                // Skip if byte ranges don't overlap.
                if !a1.overlaps(a2) {
                    continue;
                }

                // Skip if they are mutually exclusive (cannot both execute).
                let pair_key = if a1.id < a2.id {
                    (a1.id, a2.id)
                } else {
                    (a2.id, a1.id)
                };
                if mutex_excl.contains(&pair_key) {
                    continue;
                }

                // Skip if they are ordered by a sync edge (in either direction).
                // Sync edges establish cross-thread happens-before.
                if self.are_ordered(a1.id, a2.id, &sync_ordered) {
                    continue;
                }

                // Skip if they are ordered by program-order (in either direction).
                // Program-order is the sequential ControlFlow within a single
                // thread — for single-threaded programs, this provides
                // sufficient ordering to rule out a data race.
                if self.are_ordered(a1.id, a2.id, &program_ordered) {
                    continue;
                }

                // Determine the conflict kind using CapD lattice.
                //
                // For exclusivity, we check whether an access *has write
                // capability* in the CapD lattice. If the CapD is present,
                // `can_write` indicates the access *may* write (possibly
                // conditioned on a lock). If no CapD is provided, the
                // access kind (Read/Write) determines the capability.
                //
                // Separately, we check whether both writes are guarded by
                // the same lock via CapD conditions. If so, the conflict
                // is recorded but treated as "probably safe" because mutual
                // exclusion is guaranteed at runtime.
                let cap1 = input.capabilities.get(&a1.id);
                let cap2 = input.capabilities.get(&a2.id);

                let a1_has_write = self.access_has_write_capability(a1, cap1);
                let a2_has_write = self.access_has_write_capability(a2, cap2);

                // Determine conflict kind.
                let kind = if a1_has_write && a2_has_write {
                    ConflictKind::WriteWrite
                } else if a1_has_write || a2_has_write {
                    ConflictKind::WriteRead
                } else {
                    // Both effectively read-only after CapD resolution — no conflict.
                    continue;
                };

                // Compute the overlap range.
                let (s_start, s_end) = a1.byte_range();
                let (o_start, o_end) = a2.byte_range();
                let overlap_start = s_start.max(o_start);
                let overlap_end = s_end.min(o_end);

                // Check if both accesses are protected by the same mutex lock.
                // If the CapD for both writes requires the same lock, mutual
                // exclusion guarantees they cannot execute concurrently.
                let both_locked = self.both_protected_by_same_lock(cap1, cap2);

                let description = if both_locked {
                    format!(
                        "{} {} at {} and {} {} at {} overlap at [0x{:x}, 0x{:x}) but protected by same mutex",
                        a1.kind, a1.id, a1.program_point,
                        a2.kind, a2.id, a2.program_point,
                        overlap_start, overlap_end
                    )
                } else {
                    format!(
                        "{} {} at {} and {} {} at {} overlap at [0x{:x}, 0x{:x}) \
                         without synchronization or program-order",
                        a1.kind, a1.id, a1.program_point,
                        a2.kind, a2.id, a2.program_point,
                        overlap_start, overlap_end
                    )
                };

                let conflict =
                    Conflict::new(a1.id, a2.id, kind, overlap_start, overlap_end, description);

                if self.verbose {
                    log::info!("ExclusivityVerifier: detected conflict: {}", conflict);
                }

                // Add to the interference graph regardless; the
                // both_locked flag is checked later when computing
                // hard vs. lock-protected violations.
                graph.add_conflict(conflict);
            }
        }

        // Step 3: Build the output.
        let conflicts: Vec<Conflict> = graph.conflicts().cloned().collect();
        let lock_protected_count = conflicts
            .iter()
            .filter(|c| {
                let cap1 = input.capabilities.get(&c.access1);
                let cap2 = input.capabilities.get(&c.access2);
                self.both_protected_by_same_lock(cap1, cap2)
            })
            .count();

        let hard_violations = conflicts.len() - lock_protected_count;

        let status = if hard_violations > 0 {
            // Create a counterexample from the first hard violation.
            let first_hard = conflicts.iter().find(|c| {
                let cap1 = input.capabilities.get(&c.access1);
                let cap2 = input.capabilities.get(&c.access2);
                !self.both_protected_by_same_lock(cap1, cap2)
            });

            // Build a REAL counterexample whose execution_path references the
            // actual access node IDs, program points, regions, byte offsets,
            // and the lack of a synchronization/program-order edge between
            // them. Previously this was just `vec![A1, A2]` — two opaque IDs
            // with no execution context.
            let counterexample = first_hard
                .map(|c| self.build_real_counterexample(c, input, conflicts.len()))
                .unwrap_or_else(|| {
                    CounterExample::new(
                        Vec::new(),
                        "unknown".to_string(),
                        "exclusivity violation (no conflict record found)".to_string(),
                    )
                });

            VerificationStatus::Violated { counterexample }
        } else if lock_protected_count > 0 {
            VerificationStatus::ProbablySafe {
                assumptions: vec![format!(
                    "{} conflict(s) protected by mutex locks",
                    lock_protected_count
                )],
            }
        } else {
            VerificationStatus::Proven
        };

        let message = format!(
            "exclusivity check: {} conflict(s) detected ({} hard violations, {} lock-protected)",
            conflicts.len(),
            hard_violations,
            lock_protected_count
        );

        let result = VerificationResult::new("exclusivity", status, message)
            .with_evidence(Evidence::ExhaustiveAnalysis);

        ExclusivityOutput {
            result,
            interference_graph: graph,
            conflicts,
        }
    }

    /// Compute the `sync_ordered` relation as a set of (AccessId, AccessId)
    /// pairs.
    ///
    /// Two accesses are sync-ordered if there exists a path of sync edges
    /// from one to the other. We compute the transitive closure using a
    /// simple reachability algorithm.
    fn compute_ordered_relation(
        &self,
        sync_edges: &[SyncEdgeRecord],
    ) -> HashSet<(AccessId, AccessId)> {
        // Build adjacency list.
        let mut adj: HashMap<AccessId, Vec<AccessId>> = HashMap::new();
        for edge in sync_edges {
            adj.entry(edge.access_before)
                .or_default()
                .push(edge.access_after);
        }

        // Compute transitive closure using BFS from each node.
        let mut ordered: HashSet<(AccessId, AccessId)> = HashSet::new();
        let all_nodes: Vec<AccessId> = adj.keys().copied().collect();

        for &start in &all_nodes {
            let mut visited = HashSet::new();
            let mut queue = std::collections::VecDeque::new();
            queue.push_back(start);

            while let Some(current) = queue.pop_front() {
                if visited.contains(&current) {
                    continue;
                }
                visited.insert(current);

                if current != start {
                    ordered.insert((start, current));
                }

                if let Some(neighbors) = adj.get(&current) {
                    for &next in neighbors {
                        if !visited.contains(&next) {
                            queue.push_back(next);
                        }
                    }
                }
            }
        }

        // Also add direct sync edge pairs (for non-BFS-reachable ones).
        for edge in sync_edges {
            ordered.insert((edge.access_before, edge.access_after));
        }

        ordered
    }

    /// Compute the transitive closure of a set of `(AccessId, AccessId)`
    /// pairs (used for `program_order`). Same algorithm as
    /// [`compute_ordered_relation`](Self::compute_ordered_relation), but
    /// over plain pairs instead of `SyncEdgeRecord`s.
    fn compute_pair_closure(
        &self,
        pairs: &[(AccessId, AccessId)],
    ) -> HashSet<(AccessId, AccessId)> {
        let mut adj: HashMap<AccessId, Vec<AccessId>> = HashMap::new();
        for &(before, after) in pairs {
            adj.entry(before).or_default().push(after);
        }

        let mut closure: HashSet<(AccessId, AccessId)> = HashSet::new();
        let all_nodes: Vec<AccessId> = adj.keys().copied().collect();

        for &start in &all_nodes {
            let mut visited = HashSet::new();
            let mut queue = std::collections::VecDeque::new();
            queue.push_back(start);

            while let Some(current) = queue.pop_front() {
                if !visited.insert(current) {
                    continue;
                }
                if current != start {
                    closure.insert((start, current));
                }
                if let Some(neighbors) = adj.get(&current) {
                    for &next in neighbors {
                        if !visited.contains(&next) {
                            queue.push_back(next);
                        }
                    }
                }
            }
        }

        for &(before, after) in pairs {
            closure.insert((before, after));
        }

        closure
    }

    /// Check if two accesses are ordered (in either direction).
    fn are_ordered(
        &self,
        a1: AccessId,
        a2: AccessId,
        ordered: &HashSet<(AccessId, AccessId)>,
    ) -> bool {
        ordered.contains(&(a1, a2)) || ordered.contains(&(a2, a1))
    }

    /// Determine if an access has Write capability in the CapD lattice.
    ///
    /// If a CapD is provided, `can_write` determines whether the access
    /// *may* write (regardless of lock conditions — the lock condition
    /// is checked separately to determine if the conflict is protected).
    /// If no CapD info is available, the access kind determines capability.
    fn access_has_write_capability(&self, access: &AccessRecord, cap: Option<&CapDInfo>) -> bool {
        // If the access is a Read kind, it never has write capability.
        if access.kind == AccessKind::Read {
            return false;
        }

        // If no CapD info, the access kind determines capability.
        match cap {
            Some(capd) => capd.has_write(),
            None => true, // Write access with no CapD → assume write capability
        }
    }

    /// Check if two accesses are both protected by the same lock via
    /// their CapD conditions.
    fn both_protected_by_same_lock(
        &self,
        cap1: Option<&CapDInfo>,
        cap2: Option<&CapDInfo>,
    ) -> bool {
        match (cap1, cap2) {
            (Some(c1), Some(c2)) => match (c1.write_requires_lock, c2.write_requires_lock) {
                (Some(l1), Some(l2)) => l1 == l2,
                _ => false,
            },
            _ => false,
        }
    }

    /// Build a REAL [`CounterExample`] for a detected exclusivity conflict.
    ///
    /// The counterexample's `execution_path` is a `Vec<ProgramPoint>` of
    /// structured proof-step strings that reference:
    ///
    /// 1. Access 1: its node ID (`A{id}`), access mode (read/write), the
    ///    program point where it occurs, the region ID, the derivation ID,
    ///    and the exact byte range it touches.
    /// 2. Access 2: the same fields.
    /// 3. The lack of a synchronization edge between the two accesses
    ///    (no happens-before, atomic acquire/release, or mutex ordering).
    /// 4. The lack of a program-order edge (no single-threaded sequential
    ///    ordering rules out the race).
    /// 5. The fact that the pair is not marked mutually exclusive (both
    ///    accesses may execute on the same run).
    /// 6. The overlap range and the kind of data race (`write-write` or
    ///    `write-read`).
    ///
    /// The `violation_point` is set to the program point of the first
    /// access (or the second, if the first record is missing), so callers
    /// can navigate directly to the source location.
    ///
    /// The previous implementation built the path as `vec![format!("{}", c.access1),
    /// format!("{}", c.access2)]` — just two opaque IDs (`"A1"` and `"A2"`) with
    /// no execution context. That made the counterexample useless for
    /// debugging or for downstream consumers expecting a real program
    /// path.
    fn build_real_counterexample(
        &self,
        conflict: &Conflict,
        input: &ExclusivityInput,
        total_conflicts: usize,
    ) -> CounterExample {
        // Look up the access records by ID. The conflict stores AccessIds;
        // we need the full AccessRecord to extract program points, region,
        // byte range, etc.
        let a1 = input.accesses.iter().find(|a| a.id == conflict.access1);
        let a2 = input.accesses.iter().find(|a| a.id == conflict.access2);

        let mut steps: Vec<ProgramPoint> = Vec::with_capacity(6);

        // Step 1: Access 1 — node ID, mode, program point, region, byte range.
        if let Some(a1) = a1 {
            steps.push(format!(
                "Access {}: {} at program point `{}` (region={}, derivation_id={}, \
                 byte range=[0x{:x}, 0x{:x}), base=0x{:x}, size={})",
                a1.id,
                a1.kind,
                a1.program_point,
                a1.region_id,
                a1.derivation_id,
                a1.base_address,
                a1.base_address + a1.size,
                a1.base_address,
                a1.size,
            ));
        } else {
            steps.push(format!(
                "Access {}: (access record not found in input — id only)",
                conflict.access1
            ));
        }

        // Step 2: Access 2 — same fields.
        if let Some(a2) = a2 {
            steps.push(format!(
                "Access {}: {} at program point `{}` (region={}, derivation_id={}, \
                 byte range=[0x{:x}, 0x{:x}), base=0x{:x}, size={})",
                a2.id,
                a2.kind,
                a2.program_point,
                a2.region_id,
                a2.derivation_id,
                a2.base_address,
                a2.base_address + a2.size,
                a2.base_address,
                a2.size,
            ));
        } else {
            steps.push(format!(
                "Access {}: (access record not found in input — id only)",
                conflict.access2
            ));
        }

        // Step 3: No synchronization edge.
        steps.push(format!(
            "No synchronization edge between {} and {} \
             (no happens-before, atomic acquire/release, or mutex ordering)",
            conflict.access1, conflict.access2
        ));

        // Step 4: No program-order edge.
        steps.push(format!(
            "No program-order edge between {} and {} \
             (not sequentialized within a single thread of execution)",
            conflict.access1, conflict.access2
        ));

        // Step 5: Not mutually exclusive.
        steps.push(format!(
            "{} and {} are not marked mutually exclusive \
             (both may execute on the same program run)",
            conflict.access1, conflict.access2
        ));

        // Step 6: The overlap and conflict conclusion.
        steps.push(format!(
            "Byte ranges overlap at [0x{:x}, 0x{:x}) — {} on the shared region \
             (these accesses are potentially concurrent)",
            conflict.overlap_start, conflict.overlap_end, conflict.kind
        ));

        // The violation point: the program point of access 1 if available,
        // otherwise access 2. This is the source location a developer
        // would navigate to first to investigate the race.
        let violation_point = a1
            .map(|a| a.program_point.clone())
            .or_else(|| a2.map(|a| a.program_point.clone()))
            .unwrap_or_else(|| format!("{}", conflict.access1));

        let description = format!(
            "{}: {} and {} access overlapping bytes [0x{:x}, 0x{:x}) without \
             synchronization or program-order (1 of {} conflict(s))",
            conflict.kind,
            conflict.access1,
            conflict.access2,
            conflict.overlap_start,
            conflict.overlap_end,
            total_conflicts,
        );

        CounterExample::new(steps, violation_point, description)
    }
}

impl Default for ExclusivityVerifier {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ExclusivityOutput — the result of exclusivity verification
// ---------------------------------------------------------------------------

/// The output of exclusivity verification.
///
/// Contains the verification result, the interference graph, and the
/// list of all detected conflicts.
#[derive(Debug, Clone)]
pub struct ExclusivityOutput {
    /// The verification result (proven, probably safe, or violated).
    pub result: VerificationResult,
    /// The interference graph of conflicting accesses.
    pub interference_graph: InterferenceGraph,
    /// All detected conflicts.
    pub conflicts: Vec<Conflict>,
}

impl ExclusivityOutput {
    /// Returns `true` if the exclusivity invariant was proven.
    pub fn is_proven(&self) -> bool {
        self.result.is_proven()
    }

    /// Returns `true` if the exclusivity invariant was violated.
    pub fn is_violated(&self) -> bool {
        self.result.is_violated()
    }

    /// Returns the number of detected conflicts.
    pub fn conflict_count(&self) -> usize {
        self.conflicts.len()
    }

    /// Returns the number of write-write conflicts.
    pub fn write_write_count(&self) -> usize {
        self.conflicts
            .iter()
            .filter(|c| c.kind == ConflictKind::WriteWrite)
            .count()
    }

    /// Returns the number of write-read conflicts.
    pub fn write_read_count(&self) -> usize {
        self.conflicts
            .iter()
            .filter(|c| c.kind == ConflictKind::WriteRead)
            .count()
    }
}

impl fmt::Display for ExclusivityOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ExclusivityOutput {{ result: {}, conflicts: {}, graph: {} }}",
            self.result,
            self.conflict_count(),
            self.interference_graph
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a program point from a string.
    fn pp(s: &str) -> ProgramPoint {
        s.to_string()
    }

    // -----------------------------------------------------------------------
    // Test 1: Simple aliasing violation (two concurrent writes to same memory)
    // -----------------------------------------------------------------------
    #[test]
    fn test_aliasing_violation_two_concurrent_writes() {
        let mut input = ExclusivityInput::new();

        // Two writes to the same address, no sync edge.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("test.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1000,
            4,
            pp("test.vu:2"),
            1,
            1,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_violated(),
            "Expected violation for two concurrent writes"
        );
        assert_eq!(output.write_write_count(), 1);
        assert_eq!(output.interference_graph.conflict_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 2: Safe sequential access (ordered by sync edge)
    // -----------------------------------------------------------------------
    #[test]
    fn test_safe_sequential_access() {
        let mut input = ExclusivityInput::new();

        // Write then Read, ordered by happens-before.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("test.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Read,
            0x1000,
            4,
            pp("test.vu:2"),
            1,
            1,
        ));
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(1),
            AccessId(2),
            SyncOrdering::HappensBefore,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_proven(),
            "Sequential access should be proven safe"
        );
        assert_eq!(output.conflict_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Test 3: Concurrent reads are safe
    // -----------------------------------------------------------------------
    #[test]
    fn test_concurrent_reads_safe() {
        let mut input = ExclusivityInput::new();

        // Two reads to the same address, no sync edge.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Read,
            0x1000,
            4,
            pp("test.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Read,
            0x1000,
            4,
            pp("test.vu:2"),
            1,
            1,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(output.is_proven(), "Concurrent reads should be proven safe");
        assert_eq!(output.conflict_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Test 4: Data race detection (concurrent write + read)
    // -----------------------------------------------------------------------
    #[test]
    fn test_data_race_write_read() {
        let mut input = ExclusivityInput::new();

        // Write and Read to the same address, no sync.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            8,
            pp("test.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Read,
            0x1004,
            4,
            pp("test.vu:2"),
            1,
            1,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_violated(),
            "Concurrent write+read should be violated"
        );
        assert_eq!(output.write_read_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 5: Mutex-protected access
    // -----------------------------------------------------------------------
    #[test]
    fn test_mutex_protected_access() {
        let mut input = ExclusivityInput::new();

        // Write and Read to the same address, both protected by same mutex.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("test.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Read,
            0x1000,
            4,
            pp("test.vu:2"),
            1,
            1,
        ));
        // Sync edge with mutex ordering.
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(1),
            AccessId(2),
            SyncOrdering::Mutex(42),
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_proven(),
            "Mutex-ordered access should be proven safe"
        );
        assert_eq!(output.conflict_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Test 6: Overlapping byte ranges (partial overlap)
    // -----------------------------------------------------------------------
    #[test]
    fn test_overlapping_byte_ranges() {
        let mut input = ExclusivityInput::new();

        // Write [0x1000, 0x1010) and Read [0x1008, 0x1018) — partial overlap.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            16,
            pp("test.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Read,
            0x1008,
            16,
            pp("test.vu:2"),
            1,
            1,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_violated(),
            "Overlapping ranges should be violated"
        );
        assert_eq!(output.write_read_count(), 1);

        // Check overlap range.
        let conflict = &output.conflicts[0];
        assert_eq!(conflict.overlap_start, 0x1008);
        assert_eq!(conflict.overlap_end, 0x1010);
    }

    // -----------------------------------------------------------------------
    // Test 7: Capability-based exclusivity (CapD with lock conditions)
    // -----------------------------------------------------------------------
    #[test]
    fn test_capability_based_exclusivity() {
        let mut input = ExclusivityInput::new();

        // Two writes with CapD: both require the same lock.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("test.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1000,
            4,
            pp("test.vu:2"),
            1,
            1,
        ));

        // Both have write capability conditioned on lock 42.
        input.set_capability(AccessId(1), CapDInfo::write_locked(42));
        input.set_capability(AccessId(2), CapDInfo::write_locked(42));

        // No sync edge, but both locked — should be "probably safe"
        // because mutual exclusion is guaranteed by the lock.
        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // The conflict should be detected (it goes into the interference graph)
        // but it should be classified as "probably safe" not "violated."
        assert!(
            !output.is_violated(),
            "Lock-protected concurrent writes should not be a hard violation"
        );
        assert!(
            output.conflict_count() > 0,
            "Should have detected the conflict in the interference graph"
        );
        // The result should be ProbablySafe, not Proven (since there IS a conflict,
        // but it's protected by a lock assumption).
        assert!(
            matches!(
                output.result.status,
                VerificationStatus::ProbablySafe { .. }
            ),
            "Lock-protected conflict should be ProbablySafe"
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: Clean program with no violations
    // -----------------------------------------------------------------------
    #[test]
    fn test_clean_program() {
        let mut input = ExclusivityInput::new();

        // Multiple non-overlapping accesses, some reads, some writes.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("test.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Read,
            0x2000,
            4,
            pp("test.vu:2"),
            2,
            2,
        ));
        input.add_access(AccessRecord::new(
            AccessId(3),
            AccessKind::Write,
            0x3000,
            8,
            pp("test.vu:3"),
            3,
            3,
        ));
        input.add_access(AccessRecord::new(
            AccessId(4),
            AccessKind::Read,
            0x1000,
            4,
            pp("test.vu:4"),
            1,
            1,
        ));

        // A4 reads from 0x1000 which was written by A1, but they're ordered.
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(1),
            AccessId(4),
            SyncOrdering::HappensBefore,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(output.is_proven(), "Clean program should be proven safe");
        assert_eq!(output.conflict_count(), 0);
        assert!(output.interference_graph.is_empty());
    }

    // -----------------------------------------------------------------------
    // Additional: CapD lattice operations
    // -----------------------------------------------------------------------
    #[test]
    fn test_capd_lattice_operations() {
        let read_only = CapDInfo::read_only();
        let write_only = CapDInfo::write_only();
        let _read_write = CapDInfo::read_write();

        // Meet: read_only ∩ write_only = empty
        let meet = read_only.meet(&write_only);
        assert!(!meet.can_read);
        assert!(!meet.can_write);

        // Join: read_only ∪ write_only = read_write
        let join = read_only.join(&write_only);
        assert!(join.can_read);
        assert!(join.can_write);

        // Empty
        let empty = CapDInfo::empty();
        assert!(!empty.has_read());
        assert!(!empty.has_write());
    }

    // -----------------------------------------------------------------------
    // Additional: Interference graph connected components
    // -----------------------------------------------------------------------
    #[test]
    fn test_interference_graph_components() {
        let mut graph = InterferenceGraph::new();

        // Two separate conflict clusters:
        // Cluster 1: A1-A2, A2-A3
        // Cluster 2: A4-A5
        graph.add_conflict(Conflict::new(
            AccessId(1),
            AccessId(2),
            ConflictKind::WriteWrite,
            0x1000,
            0x1004,
            "c1".into(),
        ));
        graph.add_conflict(Conflict::new(
            AccessId(2),
            AccessId(3),
            ConflictKind::WriteWrite,
            0x1000,
            0x1004,
            "c2".into(),
        ));
        graph.add_conflict(Conflict::new(
            AccessId(4),
            AccessId(5),
            ConflictKind::WriteRead,
            0x2000,
            0x2004,
            "c3".into(),
        ));

        assert_eq!(graph.conflict_count(), 3);
        assert_eq!(graph.node_count(), 5);

        let components = graph.connected_components();
        assert_eq!(components.len(), 2, "Should have 2 connected components");

        // Find the component with 3 nodes and the one with 2 nodes.
        let sizes: Vec<usize> = components.iter().map(|c| c.len()).collect();
        assert!(sizes.contains(&3));
        assert!(sizes.contains(&2));
    }

    // -----------------------------------------------------------------------
    // Additional: Transitive ordering via sync edges
    // -----------------------------------------------------------------------
    #[test]
    fn test_transitive_ordering() {
        let mut input = ExclusivityInput::new();

        // A1 (write) → A2 → A3 (read), with A1 ordered before A2 and
        // A2 ordered before A3. So A1 is transitively ordered before A3.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("test.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x2000, // different address — won't conflict with A1 or A3
            4,
            pp("test.vu:2"),
            2,
            2,
        ));
        input.add_access(AccessRecord::new(
            AccessId(3),
            AccessKind::Read,
            0x1000,
            4,
            pp("test.vu:3"),
            1,
            1,
        ));

        // A1 ─[hb]─▶ A2 ─[hb]─▶ A3
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

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // A1 and A3 are transitively ordered, so no conflict.
        assert!(
            output.is_proven(),
            "Transitively ordered accesses should be safe"
        );
    }

    // -----------------------------------------------------------------------
    // Additional: AccessRecord overlap and conflict methods
    // -----------------------------------------------------------------------
    #[test]
    fn test_access_record_overlap_and_conflict() {
        let a1 = AccessRecord::new(AccessId(1), AccessKind::Write, 0x1000, 8, pp("x"), 1, 1);
        let a2 = AccessRecord::new(AccessId(2), AccessKind::Read, 0x1004, 8, pp("y"), 1, 1);
        let a3 = AccessRecord::new(AccessId(3), AccessKind::Read, 0x2000, 4, pp("z"), 2, 2);

        // a1 and a2 overlap partially.
        assert!(a1.overlaps(&a2));
        assert!(a1.conflicts_with(&a2)); // write + read + overlap

        // a1 and a3 don't overlap.
        assert!(!a1.overlaps(&a3));
        assert!(!a1.conflicts_with(&a3));
    }

    // -----------------------------------------------------------------------
    // Additional: Empty input (no accesses) should be proven
    // -----------------------------------------------------------------------
    #[test]
    fn test_empty_input_proven() {
        let input = ExclusivityInput::new();
        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(output.is_proven(), "Empty input should be proven safe");
        assert_eq!(output.conflict_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Additional: CapD lock condition resolution
    // -----------------------------------------------------------------------
    #[test]
    fn test_capd_lock_condition_resolution() {
        let locked_write = CapDInfo::write_locked(42);
        let held_locks: HashSet<u64> = [42u64].into_iter().collect();
        let no_locks: HashSet<u64> = HashSet::new();

        // With the lock held, write should be active.
        assert!(locked_write.is_write_active(&held_locks));

        // Without the lock, write should be inactive.
        assert!(!locked_write.is_write_active(&no_locks));

        // Read should always be active for write_locked.
        assert!(locked_write.is_read_active(&held_locks));
        assert!(locked_write.is_read_active(&no_locks));
    }

    // -----------------------------------------------------------------------
    // Additional: Multiple conflicts produce correct interference graph
    // -----------------------------------------------------------------------
    #[test]
    fn test_multiple_conflicts_interference_graph() {
        let mut input = ExclusivityInput::new();

        // Three writes to the same address — 3 pairwise conflicts.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("a"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1000,
            4,
            pp("b"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(3),
            AccessKind::Write,
            0x1000,
            4,
            pp("c"),
            1,
            1,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(output.is_violated());
        // 3 choose 2 = 3 conflicts
        assert_eq!(output.conflict_count(), 3);
        assert_eq!(output.interference_graph.node_count(), 3);
        assert_eq!(output.write_write_count(), 3);

        // All should be in one connected component.
        let components = output.interference_graph.connected_components();
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].len(), 3);
    }

    // -----------------------------------------------------------------------
    // W5: Program-order vs. sync-edge semantics.
    //
    // These tests pin down the new behavior introduced by W5: ordinary
    // sequential ControlFlow between two accesses does NOT establish a
    // sync edge (cross-thread happens-before), but it DOES establish
    // program-order, which is sufficient to rule out conflicts in
    // single-threaded code. Genuine conflicts arise only when two
    // overlapping accesses have neither sync-ordering, nor
    // program-ordering, nor mutual exclusivity between them.
    // -----------------------------------------------------------------------

    // Test W5-A: Conflicting writes to the same region with no ordering
    // at all should be flagged as a real data race.
    #[test]
    fn test_w5_conflicting_writes_to_same_region_flagged() {
        let mut input = ExclusivityInput::new();

        // Two writes to the same byte range, no sync edge, no program-order
        // edge, and not marked mutually exclusive. These could execute in
        // either order, so this is a genuine write-write data race.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("race.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1000,
            4,
            pp("race.vu:2"),
            1,
            1,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_violated(),
            "Two unordered writes to the same region should be a data race"
        );
        assert_eq!(output.write_write_count(), 1);
        assert_eq!(output.conflict_count(), 1);
    }

    // Test W5-B: Read-then-write ordered by program-order (sequential
    // ControlFlow) is safe in a single-threaded program — no flag.
    #[test]
    fn test_w5_ordered_read_then_write_no_flag() {
        let mut input = ExclusivityInput::new();

        // Read then Write to the same address. They overlap and one is a
        // write, so naively they conflict — but they are ordered by
        // program-order (sequential ControlFlow), which is sufficient
        // ordering for single-threaded code.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Read,
            0x1000,
            4,
            pp("seq.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1000,
            4,
            pp("seq.vu:2"),
            1,
            1,
        ));
        input.add_program_order_edge(AccessId(1), AccessId(2));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_proven(),
            "Sequential read-then-write ordered by program-order should not conflict"
        );
        assert_eq!(output.conflict_count(), 0);
    }

    // Test W5-C: Two writes to the same address in mutually exclusive
    // branches (e.g., different arms of an `if`) should NOT conflict.
    #[test]
    fn test_w5_writes_in_different_branches_no_flag() {
        let mut input = ExclusivityInput::new();

        // Two writes to the same byte range. They have no sync or
        // program-order edge between them, but they are marked mutually
        // exclusive — only one can execute on any single run, so there
        // is no data race.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("branch.vu:then"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1000,
            4,
            pp("branch.vu:else"),
            1,
            1,
        ));
        input.add_mutually_exclusive_pair(AccessId(1), AccessId(2));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_proven(),
            "Mutually exclusive writes (different branches) should not conflict"
        );
        assert_eq!(output.conflict_count(), 0);
        assert!(output.interference_graph.is_empty());
    }

    // Test W5-D: Sync edges (e.g., Mutex) still rule out conflicts
    // independently of program-order — this guards against regressions
    // where we might accidentally drop sync-edge handling.
    #[test]
    fn test_w5_sync_edge_still_orders_pair() {
        let mut input = ExclusivityInput::new();

        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("sync.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1000,
            4,
            pp("sync.vu:2"),
            1,
            1,
        ));
        // No program-order edge — only a sync edge.
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(1),
            AccessId(2),
            SyncOrdering::HappensBefore,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_proven(),
            "Sync edge alone (without program-order) should still rule out the conflict"
        );
        assert_eq!(output.conflict_count(), 0);
    }

    // Test W5-E: Program-order transitive closure. If A → B → C in
    // program-order, then A and C are ordered (transitively) and do not
    // conflict, even though there is no direct program-order edge A → C.
    #[test]
    fn test_w5_program_order_transitive_closure() {
        let mut input = ExclusivityInput::new();

        // A1 (write) → A2 (write, different addr) → A3 (write, same addr as A1).
        // A1 and A3 are transitively program-ordered, so no conflict.
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("t.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x2000,
            4,
            pp("t.vu:2"),
            2,
            2,
        ));
        input.add_access(AccessRecord::new(
            AccessId(3),
            AccessKind::Write,
            0x1000,
            4,
            pp("t.vu:3"),
            1,
            1,
        ));
        input.add_program_order_edge(AccessId(1), AccessId(2));
        input.add_program_order_edge(AccessId(2), AccessId(3));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_proven(),
            "Transitively program-ordered accesses should not conflict"
        );
        assert_eq!(output.conflict_count(), 0);
    }

    // Test W5-F: A program-order edge in one direction does NOT rule
    // out a conflict if the underlying pair could still race across
    // threads — but for single-threaded semantics, even one-directional
    // program-order is enough. This test confirms the verifier does not
    // spuriously flag a pair that is ordered A → B just because we
    // happened to query the pair as (B, A) in the iteration.
    #[test]
    fn test_w5_program_order_either_direction_rules_out_conflict() {
        let mut input = ExclusivityInput::new();

        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("d.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Read,
            0x1000,
            4,
            pp("d.vu:2"),
            1,
            1,
        ));
        // Program-order edge 2 → 1 (i.e., A2 happens before A1 in
        // sequential order). Either direction should rule out the
        // conflict.
        input.add_program_order_edge(AccessId(2), AccessId(1));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_proven(),
            "Program-order in either direction should rule out the conflict"
        );
    }

    // -----------------------------------------------------------------------
    // W8: Real counterexamples for Exclusivity.
    //
    // These tests pin down the new behavior introduced by W8: when a
    // conflict (data race) is detected between two accesses, the
    // counterexample's `execution_path` is a real `Vec<ProgramPoint>`
    // referencing both access nodes (their IDs, program points, regions,
    // byte offsets), the lack of a synchronization edge, the lack of a
    // program-order edge, the absence of mutual exclusion, and the
    // overlap that triggers the race.
    // -----------------------------------------------------------------------

    // Test W8-A: A real data race (two unordered writes to the same
    // region) must produce a counterexample whose execution_path:
    //   - references BOTH access IDs (A1 and A2),
    //   - references BOTH program points,
    //   - explicitly mentions the lack of a synchronization edge,
    //   - explicitly mentions the lack of program-order,
    //   - explicitly mentions the overlap.
    #[test]
    fn test_w8_counterexample_references_both_access_nodes() {
        let mut input = ExclusivityInput::new();

        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("race.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1000,
            4,
            pp("race.vu:2"),
            1,
            1,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(output.is_violated(), "expected a violation: {}", output);

        let counterexample = match &output.result.status {
            VerificationStatus::Violated { counterexample } => counterexample,
            other => panic!("expected Violated, got {:?}", other),
        };

        // Execution path must be non-empty.
        assert!(
            !counterexample.execution_path.is_empty(),
            "execution_path should not be empty"
        );

        let path_joined = counterexample.execution_path.join("\n");

        // Must reference BOTH access node IDs.
        assert!(
            path_joined.contains("A1"),
            "execution_path should reference Access A1: {}",
            path_joined
        );
        assert!(
            path_joined.contains("A2"),
            "execution_path should reference Access A2: {}",
            path_joined
        );

        // Must reference BOTH program points.
        assert!(
            path_joined.contains("race.vu:1"),
            "execution_path should reference program point `race.vu:1`: {}",
            path_joined
        );
        assert!(
            path_joined.contains("race.vu:2"),
            "execution_path should reference program point `race.vu:2`: {}",
            path_joined
        );

        // Must mention the lack of synchronization.
        assert!(
            path_joined
                .to_lowercase()
                .contains("no synchronization edge"),
            "execution_path should mention the lack of synchronization: {}",
            path_joined
        );

        // Must mention the lack of program-order.
        assert!(
            path_joined.to_lowercase().contains("no program-order edge"),
            "execution_path should mention the lack of program-order: {}",
            path_joined
        );

        // Must mention mutual exclusion (or its absence).
        assert!(
            path_joined.to_lowercase().contains("mutually exclusive"),
            "execution_path should mention mutual exclusion: {}",
            path_joined
        );

        // Must mention the byte-range overlap.
        assert!(
            path_joined.contains("overlap"),
            "execution_path should mention the byte-range overlap: {}",
            path_joined
        );

        // Must reference the region id (region=1).
        assert!(
            path_joined.contains("region=1"),
            "execution_path should mention the region id: {}",
            path_joined
        );

        // The violation_point should be one of the access program points.
        assert!(
            counterexample.violation_point == "race.vu:1"
                || counterexample.violation_point == "race.vu:2",
            "violation_point should be one of the access program points, got: {}",
            counterexample.violation_point
        );

        // The description should mention both access IDs.
        assert!(
            counterexample.description.contains("A1"),
            "description should reference A1: {}",
            counterexample.description
        );
        assert!(
            counterexample.description.contains("A2"),
            "description should reference A2: {}",
            counterexample.description
        );
    }

    // Test W8-B: A write-read race must produce a counterexample
    // referencing the read access and its program point too — not just
    // the write. This guards against a regression where only one of the
    // two accesses is mentioned.
    #[test]
    fn test_w8_write_read_counterexample_references_read_access() {
        let mut input = ExclusivityInput::new();

        input.add_access(AccessRecord::new(
            AccessId(7),
            AccessKind::Write,
            0x2000,
            8,
            pp("wr.vu:write"),
            5,
            9,
        ));
        input.add_access(AccessRecord::new(
            AccessId(8),
            AccessKind::Read,
            0x2004,
            4,
            pp("wr.vu:read"),
            5,
            9,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(output.is_violated(), "expected a write-read violation");

        let counterexample = match &output.result.status {
            VerificationStatus::Violated { counterexample } => counterexample,
            other => panic!("expected Violated, got {:?}", other),
        };

        let path_joined = counterexample.execution_path.join("\n");

        // Both access IDs.
        assert!(
            path_joined.contains("A7") && path_joined.contains("A8"),
            "execution_path should reference both A7 and A8: {}",
            path_joined
        );

        // Both access KINDS (write and read).
        assert!(
            path_joined.contains("write"),
            "execution_path should mention the write access kind: {}",
            path_joined
        );
        assert!(
            path_joined.contains("read"),
            "execution_path should mention the read access kind: {}",
            path_joined
        );

        // Both program points.
        assert!(
            path_joined.contains("wr.vu:write") && path_joined.contains("wr.vu:read"),
            "execution_path should reference both program points: {}",
            path_joined
        );

        // The conflict kind in the description should be write-read.
        assert!(
            counterexample
                .description
                .to_lowercase()
                .contains("write-read"),
            "description should mention write-read conflict: {}",
            counterexample.description
        );
    }

    // Test W8-C: A clean program (no violations) must NOT produce a
    // counterexample — the status should be Proven, not Violated. This
    // guards against a regression where the real-counterexample builder
    // might be called on an empty conflict list.
    #[test]
    fn test_w8_clean_program_has_no_counterexample() {
        let mut input = ExclusivityInput::new();

        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("ok.vu:1"),
            1,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Read,
            0x1000,
            4,
            pp("ok.vu:2"),
            1,
            1,
        ));
        // Sync edge rules out the race.
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(1),
            AccessId(2),
            SyncOrdering::HappensBefore,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_proven(),
            "clean program should be Proven, got: {}",
            output
        );
        assert!(
            !output.is_violated(),
            "clean program should not be Violated"
        );
    }
}
