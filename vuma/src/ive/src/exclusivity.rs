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
//! 2. Compute the `ordered` relation (transitive closure of sync edges).
//! 3. For every pair of accesses `(a1, a2)`:
//!    a. Skip if both are reads (reads never conflict).
//!    b. Skip if their byte ranges do not overlap.
//!    c. Skip if they are ordered by a sync edge (in either direction).
//!    d. Otherwise, check CapD permissions: if both have Write → write-write
//!       data race; if one Write + one Read → read-write race.
//! 4. Build an interference graph from all detected conflicts.
//! 5. Return a structured [`VerificationResult`].
//!
//! # Interference Graph
//!
//! The interference graph is an undirected graph where each node is an
//! [`AccessId`] and each edge represents a conflict between two accesses.
//! This graph can be used for coloring-based alias analysis or for
//! reporting violation clusters to the user.

use crate::result::{CounterExample, Evidence, ProgramPoint, VerificationResult, VerificationStatus};
use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// AccessId — unique identifier for a memory access
// ---------------------------------------------------------------------------

/// Unique identifier for a memory access event within the exclusivity checker.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
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
        self.overlaps(other)
            && (self.kind == AccessKind::Write || other.kind == AccessKind::Write)
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
    pub fn new(
        access_before: AccessId,
        access_after: AccessId,
        ordering: SyncOrdering,
    ) -> Self {
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
            write_requires_lock: match (
                self.write_requires_lock,
                other.write_requires_lock,
            ) {
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
            write_requires_lock: match (
                self.write_requires_lock,
                other.write_requires_lock,
            ) {
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
            self.kind, self.access1, self.access2, self.overlap_start, self.overlap_end,
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
/// Contains all memory accesses, synchronization edges, and capability
/// descriptors needed to perform the exclusivity check.
#[derive(Debug, Clone, Default)]
pub struct ExclusivityInput {
    /// All memory access events.
    pub accesses: Vec<AccessRecord>,
    /// All synchronization edges.
    pub sync_edges: Vec<SyncEdgeRecord>,
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

    /// Add a synchronization edge.
    pub fn add_sync_edge(&mut self, edge: SyncEdgeRecord) {
        self.sync_edges.push(edge);
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
        // Step 1: Compute the `ordered` relation (transitive closure of sync edges).
        let ordered = self.compute_ordered_relation(&input.sync_edges);

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

                // Skip if they are ordered (in either direction).
                if self.are_ordered(a1.id, a2.id, &ordered) {
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
                        "{} {} at {} and {} {} at {} overlap at [0x{:x}, 0x{:x}) without synchronization",
                        a1.kind, a1.id, a1.program_point,
                        a2.kind, a2.id, a2.program_point,
                        overlap_start, overlap_end
                    )
                };

                let conflict = Conflict::new(
                    a1.id,
                    a2.id,
                    kind,
                    overlap_start,
                    overlap_end,
                    description,
                );

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

            let counterexample = first_hard.map(|c| {
                CounterExample::new(
                    vec![
                        format!("{}", c.access1),
                        format!("{}", c.access2),
                    ],
                    format!("{}", c.access1),
                    c.description.clone(),
                )
            });

            VerificationStatus::Violated {
                counterexample: counterexample.unwrap_or_else(|| {
                    CounterExample::new(
                        vec![],
                        "unknown".to_string(),
                        "exclusivity violation".to_string(),
                    )
                }),
            }
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

    /// Compute the `ordered` relation as a set of (AccessId, AccessId) pairs.
    ///
    /// Two accesses are ordered if there exists a path of sync edges from
    /// one to the other. We compute the transitive closure using a simple
    /// reachability algorithm.
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
    fn access_has_write_capability(
        &self,
        access: &AccessRecord,
        cap: Option<&CapDInfo>,
    ) -> bool {
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
            (Some(c1), Some(c2)) => {
                match (c1.write_requires_lock, c2.write_requires_lock) {
                    (Some(l1), Some(l2)) => l1 == l2,
                    _ => false,
                }
            }
            _ => false,
        }
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

        assert!(output.is_violated(), "Expected violation for two concurrent writes");
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

        assert!(output.is_proven(), "Sequential access should be proven safe");
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

        assert!(
            output.is_proven(),
            "Concurrent reads should be proven safe"
        );
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

        assert!(output.is_violated(), "Concurrent write+read should be violated");
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

        assert!(output.is_violated(), "Overlapping ranges should be violated");
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
            matches!(output.result.status, VerificationStatus::ProbablySafe { .. }),
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
            AccessId(1), AccessId(2), ConflictKind::WriteWrite,
            0x1000, 0x1004, "c1".into(),
        ));
        graph.add_conflict(Conflict::new(
            AccessId(2), AccessId(3), ConflictKind::WriteWrite,
            0x1000, 0x1004, "c2".into(),
        ));
        graph.add_conflict(Conflict::new(
            AccessId(4), AccessId(5), ConflictKind::WriteRead,
            0x2000, 0x2004, "c3".into(),
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
        let a1 = AccessRecord::new(
            AccessId(1), AccessKind::Write, 0x1000, 8, pp("x"), 1, 1,
        );
        let a2 = AccessRecord::new(
            AccessId(2), AccessKind::Read, 0x1004, 8, pp("y"), 1, 1,
        );
        let a3 = AccessRecord::new(
            AccessId(3), AccessKind::Read, 0x2000, 4, pp("z"), 2, 2,
        );

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
            AccessId(1), AccessKind::Write, 0x1000, 4, pp("a"), 1, 1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2), AccessKind::Write, 0x1000, 4, pp("b"), 1, 1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(3), AccessKind::Write, 0x1000, 4, pp("c"), 1, 1,
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
}
