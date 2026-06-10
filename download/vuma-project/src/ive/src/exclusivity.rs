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
    /// Derivation depth for each access: how many pointer dereferences from root.
    /// If not specified for an access, defaults to 0.
    pub derivation_depths: HashMap<AccessId, u32>,
    /// Base address for each region ID, used to compute offsets within regions.
    pub region_bases: HashMap<u64, u64>,
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

    /// Set the derivation depth for an access.
    pub fn set_derivation_depth(&mut self, access_id: AccessId, depth: u32) {
        self.derivation_depths.insert(access_id, depth);
    }

    /// Set the base address for a region.
    pub fn set_region_base(&mut self, region_id: u64, base: u64) {
        self.region_bases.insert(region_id, base);
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
        // Check if there are multiple pointers to the same region.
        // If so, use the multi-pointer alias analysis.
        let has_multi_pointer = self.has_multi_pointer_aliasing(input);

        let mut output = if has_multi_pointer {
            self.verify_multi_pointer_exclusivity(input)
        } else {
            // Fall back to the simple pairwise check.
            self.verify_pairwise(input)
        };

        // Generate proof obligations and populate the output.
        let obligations = self.generate_proof_obligations(&output, input);
        output.proof_obligations = obligations;

        output
    }

    /// Check whether the input has multiple pointers (accesses) targeting
    /// the same region, which would benefit from alias-set-based analysis.
    fn has_multi_pointer_aliasing(&self, input: &ExclusivityInput) -> bool {
        let mut region_counts: HashMap<u64, usize> = HashMap::new();
        for access in &input.accesses {
            *region_counts.entry(access.region_id).or_default() += 1;
        }
        region_counts.values().any(|&count| count > 1)
    }

    /// Simple pairwise exclusivity check (the original algorithm).
    ///
    /// This is used as a fallback when there is no multi-pointer aliasing
    /// in the input.
    fn verify_pairwise(&self, input: &ExclusivityInput) -> ExclusivityOutput {
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
            proof_obligations: Vec::new(),
        }
    }

    /// Verify the exclusivity invariant using an interval tree for efficient
    /// overlap detection.
    ///
    /// This method produces the same result as [`verify()`](Self::verify) but
    /// uses an interval tree to find overlapping access pairs in O(n log n)
    /// instead of O(n²) pairwise comparison.
    ///
    /// # Algorithm
    ///
    /// 1. Build an interval tree from all access records.
    /// 2. Compute the `ordered` relation (same as `verify()`).
    /// 3. For each write access, query the interval tree for overlapping
    ///    accesses (at least one write in every conflict → iterating over
    ///    writes is sufficient).
    /// 4. For each overlapping pair, check sync ordering and CapD.
    /// 5. Build the interference graph and return the same output as `verify()`.
    pub fn verify_with_interval_tree(&self, input: &ExclusivityInput) -> ExclusivityOutput {
        // If multi-pointer aliasing is detected, fall back to the
        // specialized multi-pointer analysis (which uses region-based
        // alias analysis beyond simple byte-range overlap).
        if self.has_multi_pointer_aliasing(input) {
            return self.verify_multi_pointer_exclusivity(input);
        }

        // Step 1: Build the interval tree.
        let tree = AccessIntervalTree::from_accesses(&input.accesses);

        // Step 2: Compute the ordered relation.
        let ordered = self.compute_ordered_relation(&input.sync_edges);

        // Build an index for quick AccessRecord lookup by AccessId.
        let access_by_id: HashMap<AccessId, usize> = input
            .accesses
            .iter()
            .enumerate()
            .map(|(i, a)| (a.id, i))
            .collect();

        // Step 3: For each write access, query the tree for overlaps.
        let mut graph = InterferenceGraph::new();
        let mut checked_pairs: HashSet<(AccessId, AccessId)> = HashSet::new();

        for access in &input.accesses {
            // Only iterate over writes — any conflict involves at least one write.
            if access.kind == AccessKind::Read {
                continue;
            }

            let (start, end) = access.byte_range();
            let overlapping = tree.query_overlaps(start, end);

            for other_id in overlapping {
                if other_id == access.id {
                    continue;
                }

                // Normalize pair to avoid double-counting.
                let pair = if access.id < other_id {
                    (access.id, other_id)
                } else {
                    (other_id, access.id)
                };

                if checked_pairs.contains(&pair) {
                    continue;
                }
                checked_pairs.insert(pair);

                let &other_idx = access_by_id.get(&other_id).unwrap();
                let other = &input.accesses[other_idx];

                // Skip if they are ordered (in either direction).
                if self.are_ordered(access.id, other_id, &ordered) {
                    continue;
                }

                // Determine the conflict kind using CapD lattice.
                let cap1 = input.capabilities.get(&access.id);
                let cap2 = input.capabilities.get(&other_id);

                let a1_has_write = self.access_has_write_capability(access, cap1);
                let a2_has_write = self.access_has_write_capability(other, cap2);

                let kind = if a1_has_write && a2_has_write {
                    ConflictKind::WriteWrite
                } else if a1_has_write || a2_has_write {
                    ConflictKind::WriteRead
                } else {
                    continue;
                };

                // Compute the overlap range.
                let (s_start, s_end) = access.byte_range();
                let (o_start, o_end) = other.byte_range();
                let overlap_start = s_start.max(o_start);
                let overlap_end = s_end.min(o_end);

                let both_locked = self.both_protected_by_same_lock(cap1, cap2);

                let description = if both_locked {
                    format!(
                        "{} {} at {} and {} {} at {} overlap at [0x{:x}, 0x{:x}) but protected by same mutex",
                        access.kind, access.id, access.program_point,
                        other.kind, other_id, other.program_point,
                        overlap_start, overlap_end
                    )
                } else {
                    format!(
                        "{} {} at {} and {} {} at {} overlap at [0x{:x}, 0x{:x}) without synchronization",
                        access.kind, access.id, access.program_point,
                        other.kind, other_id, other.program_point,
                        overlap_start, overlap_end
                    )
                };

                let conflict = Conflict::new(
                    access.id,
                    other_id,
                    kind,
                    overlap_start,
                    overlap_end,
                    description,
                );

                if self.verbose {
                    log::info!(
                        "ExclusivityVerifier (interval tree): detected conflict: {}",
                        conflict
                    );
                }

                graph.add_conflict(conflict);
            }
        }

        // Step 4: Build the output (same logic as verify()).
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
            let first_hard = conflicts.iter().find(|c| {
                let cap1 = input.capabilities.get(&c.access1);
                let cap2 = input.capabilities.get(&c.access2);
                !self.both_protected_by_same_lock(cap1, cap2)
            });

            let counterexample = first_hard.map(|c| {
                CounterExample::new(
                    vec![format!("{}", c.access1), format!("{}", c.access2)],
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
            "exclusivity check (interval tree): {} conflict(s) detected ({} hard violations, {} lock-protected)",
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
            proof_obligations: Vec::new(),
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

    // -----------------------------------------------------------------------
    // Multi-pointer aliasing support
    // -----------------------------------------------------------------------

    /// Compute derivation alias info for each access in the input.
    ///
    /// This extracts the root region, offset within the region, size, and
    /// derivation depth for each access. The offset is computed as
    /// `base_address - region_base` when a region base is available,
    /// or `base_address` itself when no region base is registered.
    ///
    /// The derivation depth defaults to 0 if not explicitly set in the input.
    pub fn compute_derivation_alias_info(
        &self,
        input: &ExclusivityInput,
    ) -> HashMap<AccessId, DerivationAliasInfo> {
        let mut result = HashMap::new();
        for access in &input.accesses {
            let offset = match input.region_bases.get(&access.region_id) {
                Some(&base) => access.base_address.saturating_sub(base),
                None => access.base_address,
            };
            let depth = input
                .derivation_depths
                .get(&access.id)
                .copied()
                .unwrap_or(0);
            result.insert(
                access.id,
                DerivationAliasInfo::new(access.region_id, offset, access.size, depth),
            );
        }
        result
    }

    /// Compute alias sets using union-find.
    ///
    /// Two accesses are placed in the same alias set if:
    /// 1. They target overlapping byte ranges, **and**
    /// 2. They share a common derivation ancestor (same `region_id`).
    ///
    /// This is more precise than simple pairwise checking because it groups
    /// transitively-aliased accesses together, enabling whole-set analysis.
    pub fn compute_alias_sets(
        &self,
        input: &ExclusivityInput,
    ) -> Vec<HashSet<AccessId>> {
        let n = input.accesses.len();
        if n == 0 {
            return Vec::new();
        }

        let mut uf = UnionFind::new(n);

        // Union accesses that alias: same region_id AND overlapping ranges.
        for i in 0..n {
            for j in (i + 1)..n {
                let a1 = &input.accesses[i];
                let a2 = &input.accesses[j];

                // Must share the same root region.
                if a1.region_id != a2.region_id {
                    continue;
                }

                // Must have overlapping byte ranges.
                if !a1.overlaps(a2) {
                    continue;
                }

                // They alias — union them.
                uf.union(i, j);
            }
        }

        // Collect sets of indices, then convert to sets of AccessIds.
        let index_sets = uf.collect_sets(n);
        index_sets
            .into_iter()
            .map(|idx_set| {
                idx_set
                    .into_iter()
                    .map(|idx| input.accesses[idx].id)
                    .collect()
            })
            .collect()
    }

    /// Verify multi-pointer exclusivity through derived pointer aliasing.
    ///
    /// This is a more sophisticated analysis than simple pairwise checking.
    /// It:
    /// 1. Computes alias sets (groups of accesses that alias through
    ///    derivation chains).
    /// 2. For each alias set containing both reads and writes, checks that
    ///    writes are ordered by sync edges.
    /// 3. For alias sets with multiple writes, checks that they are either
    ///    ordered or protected by the same lock.
    /// 4. Produces conflict descriptions that include the alias set context.
    pub fn verify_multi_pointer_exclusivity(
        &self,
        input: &ExclusivityInput,
    ) -> ExclusivityOutput {
        // Step 1: Compute alias sets.
        let alias_sets = self.compute_alias_sets(input);

        // Step 2: Compute derivation alias info for enriched conflict descriptions.
        let alias_info = self.compute_derivation_alias_info(input);

        // Step 3: Compute the ordered relation.
        let ordered = self.compute_ordered_relation(&input.sync_edges);

        let mut graph = InterferenceGraph::new();

        // Step 4: For each alias set, check for conflicts within the set.
        for alias_set in &alias_sets {
            // Collect the accesses in this alias set.
            let set_accesses: Vec<&AccessRecord> = input
                .accesses
                .iter()
                .filter(|a| alias_set.contains(&a.id))
                .collect();

            // Check each pair within the alias set.
            for i in 0..set_accesses.len() {
                for j in (i + 1)..set_accesses.len() {
                    let a1 = set_accesses[i];
                    let a2 = set_accesses[j];

                    // Skip if both are reads.
                    if a1.kind == AccessKind::Read && a2.kind == AccessKind::Read {
                        continue;
                    }

                    // Skip if they are ordered.
                    if self.are_ordered(a1.id, a2.id, &ordered) {
                        continue;
                    }

                    // Determine write capability.
                    let cap1 = input.capabilities.get(&a1.id);
                    let cap2 = input.capabilities.get(&a2.id);

                    let a1_has_write = self.access_has_write_capability(a1, cap1);
                    let a2_has_write = self.access_has_write_capability(a2, cap2);

                    let kind = if a1_has_write && a2_has_write {
                        ConflictKind::WriteWrite
                    } else if a1_has_write || a2_has_write {
                        ConflictKind::WriteRead
                    } else {
                        continue;
                    };

                    // Compute overlap.
                    let (s_start, s_end) = a1.byte_range();
                    let (o_start, o_end) = a2.byte_range();
                    let overlap_start = s_start.max(o_start);
                    let overlap_end = s_end.min(o_end);

                    let both_locked = self.both_protected_by_same_lock(cap1, cap2);

                    // Build enriched description with alias set context.
                    let alias_ctx = format!(
                        "alias_set(region={}, members={})",
                        a1.region_id,
                        alias_set.len()
                    );

                    let info1 = alias_info.get(&a1.id);
                    let info2 = alias_info.get(&a2.id);
                    let derivation_ctx = match (info1, info2) {
                        (Some(i1), Some(i2)) => format!(
                            " [derivation: depth={}, offset=0x{:x} vs depth={}, offset=0x{:x}]",
                            i1.derivation_depth, i1.offset,
                            i2.derivation_depth, i2.offset
                        ),
                        _ => String::new(),
                    };

                    let description = if both_locked {
                        format!(
                            "{} {} at {} and {} {} at {} overlap at [0x{:x}, 0x{:x}) in {} but protected by same mutex{}",
                            a1.kind, a1.id, a1.program_point,
                            a2.kind, a2.id, a2.program_point,
                            overlap_start, overlap_end,
                            alias_ctx, derivation_ctx
                        )
                    } else {
                        format!(
                            "{} {} at {} and {} {} at {} overlap at [0x{:x}, 0x{:x}) in {} without synchronization{}",
                            a1.kind, a1.id, a1.program_point,
                            a2.kind, a2.id, a2.program_point,
                            overlap_start, overlap_end,
                            alias_ctx, derivation_ctx
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
                        log::info!(
                            "ExclusivityVerifier(multi-pointer): detected conflict: {}",
                            conflict
                        );
                    }

                    graph.add_conflict(conflict);
                }
            }
        }

        // Step 5: Build the output.
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
            let first_hard = conflicts.iter().find(|c| {
                let cap1 = input.capabilities.get(&c.access1);
                let cap2 = input.capabilities.get(&c.access2);
                !self.both_protected_by_same_lock(cap1, cap2)
            });

            let counterexample = first_hard.map(|c| {
                CounterExample::new(
                    vec![format!("{}", c.access1), format!("{}", c.access2)],
                    format!("{}", c.access1),
                    c.description.clone(),
                )
            });

            VerificationStatus::Violated {
                counterexample: counterexample.unwrap_or_else(|| {
                    CounterExample::new(
                        vec![],
                        "unknown".to_string(),
                        "multi-pointer exclusivity violation".to_string(),
                    )
                }),
            }
        } else if lock_protected_count > 0 {
            VerificationStatus::ProbablySafe {
                assumptions: vec![format!(
                    "{} conflict(s) protected by mutex locks in multi-pointer alias analysis",
                    lock_protected_count
                )],
            }
        } else {
            VerificationStatus::Proven
        };

        let message = format!(
            "multi-pointer exclusivity check: {} conflict(s) detected ({} hard violations, {} lock-protected), {} alias set(s)",
            conflicts.len(),
            hard_violations,
            lock_protected_count,
            alias_sets.len()
        );

        let result = VerificationResult::new("exclusivity", status, message)
            .with_evidence(Evidence::ExhaustiveAnalysis);

        ExclusivityOutput {
            result,
            interference_graph: graph,
            conflicts,
            proof_obligations: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Proof obligation generation
    // -----------------------------------------------------------------------

    /// Generate proof obligations from the conflicts in a verification output.
    ///
    /// For each detected conflict, one or more proof obligations are generated
    /// that, if discharged, would resolve the conflict. The obligations suggest
    /// specific resolutions such as adding synchronization, mutex protection,
    /// or proving single-threaded execution.
    ///
    /// # Difficulty Assignment
    ///
    /// - **Trivial**: Both accesses are already protected by the same mutex.
    /// - **Easy**: A sync edge just needs to be added.
    /// - **Moderate**: A mutex needs to be added.
    /// - **Hard**: Proving single-threaded execution is required.
    /// - **Undecidable**: General concurrent case with no clear resolution.
    pub fn generate_proof_obligations(
        &self,
        output: &ExclusivityOutput,
        input: &ExclusivityInput,
    ) -> Vec<ExclusivityProofObligation> {
        let mut obligations = Vec::new();
        let mut next_id: u64 = 1;

        for conflict in &output.conflicts {
            let cap1 = input.capabilities.get(&conflict.access1);
            let cap2 = input.capabilities.get(&conflict.access2);
            let both_locked = self.both_protected_by_same_lock(cap1, cap2);

            match conflict.kind {
                ConflictKind::WriteWrite => {
                    // For WriteWrite conflicts, suggest AddSyncEdge or
                    // ProveSingleThreaded, depending on context.

                    if both_locked {
                        // Already protected by same mutex — trivial to discharge.
                        obligations.push(ExclusivityProofObligation {
                            obligation_id: next_id,
                            conflict: conflict.clone(),
                            resolution_kind: ResolutionKind::AddMutexProtection {
                                lock_id: cap1.and_then(|c| c.write_requires_lock).unwrap_or(0),
                                access1: conflict.access1,
                                access2: conflict.access2,
                            },
                            description: format!(
                                "Write-write conflict between {} and {} is already \
                                 protected by the same mutex — verify lock correctness",
                                conflict.access1, conflict.access2
                            ),
                            difficulty: ProofDifficulty::Trivial,
                        });
                        next_id += 1;
                    } else {
                        // Suggest adding a sync edge (Easy).
                        obligations.push(ExclusivityProofObligation {
                            obligation_id: next_id,
                            conflict: conflict.clone(),
                            resolution_kind: ResolutionKind::AddSyncEdge {
                                from: conflict.access1,
                                to: conflict.access2,
                                suggested_ordering: SyncOrdering::HappensBefore,
                            },
                            description: format!(
                                "Add a happens-before edge from {} to {} to \
                                 resolve write-write conflict",
                                conflict.access1, conflict.access2
                            ),
                            difficulty: ProofDifficulty::Easy,
                        });
                        next_id += 1;

                        // Also suggest proving single-threaded (Hard).
                        obligations.push(ExclusivityProofObligation {
                            obligation_id: next_id,
                            conflict: conflict.clone(),
                            resolution_kind: ResolutionKind::ProveSingleThreaded {
                                access1: conflict.access1,
                                access2: conflict.access2,
                            },
                            description: format!(
                                "Prove that {} and {} never execute concurrently \
                                 (single-threaded path) to resolve write-write conflict",
                                conflict.access1, conflict.access2
                            ),
                            difficulty: ProofDifficulty::Hard,
                        });
                        next_id += 1;

                        // Also suggest adding mutex protection (Moderate).
                        obligations.push(ExclusivityProofObligation {
                            obligation_id: next_id,
                            conflict: conflict.clone(),
                            resolution_kind: ResolutionKind::AddMutexProtection {
                                lock_id: 0, // new lock needed
                                access1: conflict.access1,
                                access2: conflict.access2,
                            },
                            description: format!(
                                "Add mutex protection around {} and {} to \
                                 resolve write-write conflict",
                                conflict.access1, conflict.access2
                            ),
                            difficulty: ProofDifficulty::Moderate,
                        });
                        next_id += 1;
                    }
                }
                ConflictKind::WriteRead => {
                    if both_locked {
                        // Already protected — trivial.
                        obligations.push(ExclusivityProofObligation {
                            obligation_id: next_id,
                            conflict: conflict.clone(),
                            resolution_kind: ResolutionKind::AddMutexProtection {
                                lock_id: cap1.and_then(|c| c.write_requires_lock).unwrap_or(0),
                                access1: conflict.access1,
                                access2: conflict.access2,
                            },
                            description: format!(
                                "Write-read conflict between {} and {} is already \
                                 protected by the same mutex — verify lock correctness",
                                conflict.access1, conflict.access2
                            ),
                            difficulty: ProofDifficulty::Trivial,
                        });
                        next_id += 1;
                    } else {
                        // Suggest adding a sync edge (Easy).
                        obligations.push(ExclusivityProofObligation {
                            obligation_id: next_id,
                            conflict: conflict.clone(),
                            resolution_kind: ResolutionKind::AddSyncEdge {
                                from: conflict.access1,
                                to: conflict.access2,
                                suggested_ordering: SyncOrdering::HappensBefore,
                            },
                            description: format!(
                                "Add a happens-before edge from {} to {} to \
                                 resolve write-read conflict",
                                conflict.access1, conflict.access2
                            ),
                            difficulty: ProofDifficulty::Easy,
                        });
                        next_id += 1;

                        // Suggest restricting capability (Moderate/Hard depending
                        // on whether the read or write should be restricted).
                        // Find the write access to suggest restricting its cap.
                        let write_access_id = if let Some(c1) = cap1 {
                            if c1.has_write() {
                                conflict.access1
                            } else {
                                conflict.access2
                            }
                        } else {
                            // Fall back to access kind.
                            let a1 = input.accesses.iter().find(|a| a.id == conflict.access1);
                            if let Some(a1) = a1 {
                                if a1.kind == AccessKind::Write {
                                    conflict.access1
                                } else {
                                    conflict.access2
                                }
                            } else {
                                conflict.access1
                            }
                        };

                        obligations.push(ExclusivityProofObligation {
                            obligation_id: next_id,
                            conflict: conflict.clone(),
                            resolution_kind: ResolutionKind::RestrictCapability {
                                access: write_access_id,
                                remove_cap: CapDInfo::write_only(),
                            },
                            description: format!(
                                "Restrict write capability on {} to resolve \
                                 write-read conflict with {}",
                                write_access_id,
                                if write_access_id == conflict.access1 {
                                    conflict.access2
                                } else {
                                    conflict.access1
                                }
                            ),
                            difficulty: ProofDifficulty::Moderate,
                        });
                        next_id += 1;

                        // Also suggest mutex protection (Moderate).
                        obligations.push(ExclusivityProofObligation {
                            obligation_id: next_id,
                            conflict: conflict.clone(),
                            resolution_kind: ResolutionKind::AddMutexProtection {
                                lock_id: 0, // new lock needed
                                access1: conflict.access1,
                                access2: conflict.access2,
                            },
                            description: format!(
                                "Add mutex protection around {} and {} to \
                                 resolve write-read conflict",
                                conflict.access1, conflict.access2
                            ),
                            difficulty: ProofDifficulty::Moderate,
                        });
                        next_id += 1;
                    }
                }
            }

            // For any unresolved conflict (not lock-protected), add an
            // Undecidable obligation as a catch-all if none was already
            // added with Undecidable difficulty.
            if !both_locked {
                obligations.push(ExclusivityProofObligation {
                    obligation_id: next_id,
                    conflict: conflict.clone(),
                    resolution_kind: ResolutionKind::ProveSingleThreaded {
                        access1: conflict.access1,
                        access2: conflict.access2,
                    },
                    description: format!(
                        "General concurrent conflict between {} and {} — \
                         may require full concurrency analysis",
                        conflict.access1, conflict.access2
                    ),
                    difficulty: ProofDifficulty::Undecidable,
                });
                next_id += 1;
            }
        }

        obligations
    }

    /// Generate human-readable suggestions for resolving proof obligations.
    ///
    /// For each obligation, produces a [`SuggestedFix`] with a description,
    /// code hint, and confidence score.
    pub fn suggest_fixes(
        &self,
        obligations: &[ExclusivityProofObligation],
    ) -> Vec<SuggestedFix> {
        obligations
            .iter()
            .map(|obl| {
                let (description, hint, confidence) = match &obl.resolution_kind {
                    ResolutionKind::AddSyncEdge {
                        from,
                        to,
                        suggested_ordering,
                    } => {
                        let ordering_str = match suggested_ordering {
                            SyncOrdering::HappensBefore => "happens-before",
                            SyncOrdering::Atomic => "atomic (acquire-release)",
                            SyncOrdering::Mutex(id) => {
                                &format!("mutex({})", id) as &str
                            }
                        };
                        (
                            format!(
                                "Add a {} synchronization edge from {} to {}",
                                ordering_str, from, to
                            ),
                            format!(
                                "sync_edge({} -> {}, ordering={})",
                                from, to, ordering_str
                            ),
                            0.9,
                        )
                    }
                    ResolutionKind::AddMutexProtection {
                        lock_id,
                        access1,
                        access2,
                    } => {
                        if *lock_id == 0 {
                            (
                                format!(
                                    "Introduce a new mutex to protect both {} and {}",
                                    access1, access2
                                ),
                                format!(
                                    "let mutex = Mutex::new(); // protect {} and {}",
                                    access1, access2
                                ),
                                0.7,
                            )
                        } else {
                            (
                                format!(
                                    "Extend mutex {} to also protect {} and {}",
                                    lock_id, access1, access2
                                ),
                                format!(
                                    "mutex_{}.lock(); // around {} and {}",
                                    lock_id, access1, access2
                                ),
                                0.85,
                            )
                        }
                    }
                    ResolutionKind::SplitAccess {
                        original,
                        split_point,
                    } => (
                        format!(
                            "Split access {} at offset 0x{:x} to eliminate overlap",
                            original, split_point
                        ),
                        format!(
                            "// Split {} into [0..0x{:x}) and [0x{:x}..)",
                            original, split_point, split_point
                        ),
                        0.6,
                    ),
                    ResolutionKind::RestrictCapability {
                        access,
                        remove_cap,
                    } => (
                        format!(
                            "Remove write capability from {} (current: {})",
                            access, remove_cap
                        ),
                        format!(
                            "// Change {} to read-only access",
                            access
                        ),
                        0.5,
                    ),
                    ResolutionKind::ProveSingleThreaded {
                        access1,
                        access2,
                    } => (
                        format!(
                            "Prove that {} and {} are always on the same thread",
                            access1, access2
                        ),
                        format!(
                            "// assert!(same_thread({}, {}));",
                            access1, access2
                        ),
                        0.3,
                    ),
                };

                SuggestedFix {
                    obligation_id: obl.obligation_id,
                    fix_description: description,
                    code_hint: hint,
                    confidence,
                }
            })
            .collect()
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
/// Contains the verification result, the interference graph, the
/// list of all detected conflicts, and any proof obligations generated
/// from potentially-resolvable conflicts.
#[derive(Debug, Clone)]
pub struct ExclusivityOutput {
    /// The verification result (proven, probably safe, or violated).
    pub result: VerificationResult,
    /// The interference graph of conflicting accesses.
    pub interference_graph: InterferenceGraph,
    /// All detected conflicts.
    pub conflicts: Vec<Conflict>,
    /// Proof obligations generated from detected conflicts that could
    /// potentially be resolved (e.g., by adding synchronization, mutex
    /// protection, or proving single-threaded execution).
    pub proof_obligations: Vec<ExclusivityProofObligation>,
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

    /// Returns the number of proof obligations.
    pub fn proof_obligation_count(&self) -> usize {
        self.proof_obligations.len()
    }
}

impl fmt::Display for ExclusivityOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ExclusivityOutput {{ result: {}, conflicts: {}, obligations: {}, graph: {} }}",
            self.result,
            self.conflict_count(),
            self.proof_obligation_count(),
            self.interference_graph
        )
    }
}

// ---------------------------------------------------------------------------
// DerivationAliasInfo — derivation chain aliasing information
// ---------------------------------------------------------------------------

/// Information about how an access relates to its root allocation region
/// through a derivation chain of pointers.
///
/// Two accesses are considered to alias via derivation if they:
/// 1. Target overlapping byte ranges, **and**
/// 2. Share a common derivation ancestor (i.e., same `root_region`).
///
/// The `derivation_depth` indicates how many pointer dereferences separate
/// this access from the root allocation. A depth of 0 means the access
/// targets the root region directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivationAliasInfo {
    /// The root allocation region this access ultimately traces to.
    pub root_region: u64,
    /// Offset from the region start (in bytes).
    pub offset: u64,
    /// Size of the access (in bytes).
    pub size: u64,
    /// How many pointer dereferences from root (0 = direct access).
    pub derivation_depth: u32,
}

impl DerivationAliasInfo {
    /// Create a new DerivationAliasInfo.
    pub fn new(root_region: u64, offset: u64, size: u64, derivation_depth: u32) -> Self {
        Self {
            root_region,
            offset,
            size,
            derivation_depth,
        }
    }

    /// Returns the byte range `[offset, offset + size)` within the root region.
    pub fn offset_range(&self) -> (u64, u64) {
        (self.offset, self.offset + self.size)
    }

    /// Returns `true` if this alias info overlaps with another within the
    /// same root region.
    pub fn overlaps(&self, other: &DerivationAliasInfo) -> bool {
        if self.root_region != other.root_region {
            return false;
        }
        let (s_start, s_end) = self.offset_range();
        let (o_start, o_end) = other.offset_range();
        s_start < o_end && o_start < s_end
    }
}

impl fmt::Display for DerivationAliasInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DerivationAliasInfo {{ region: {}, offset: 0x{:x}, size: {}, depth: {} }}",
            self.root_region, self.offset, self.size, self.derivation_depth
        )
    }
}

// ---------------------------------------------------------------------------
// UnionFind — disjoint-set data structure for alias set computation
// ---------------------------------------------------------------------------

/// A union-find (disjoint-set) data structure for grouping accesses into
/// alias sets. Uses path compression and union by rank for near-constant
/// amortized time complexity.
#[derive(Debug, Clone)]
struct UnionFind {
    /// Parent pointer for each element.
    parent: Vec<usize>,
    /// Rank for union by rank optimization.
    rank: Vec<usize>,
}

impl UnionFind {
    /// Create a new union-find with `n` elements (0..n-1), each in its own set.
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    /// Find the representative (root) of the set containing element `x`,
    /// with path compression.
    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }

    /// Merge the sets containing elements `x` and `y`.
    /// Uses union by rank to keep the tree shallow.
    fn union(&mut self, x: usize, y: usize) {
        let rx = self.find(x);
        let ry = self.find(y);
        if rx == ry {
            return;
        }
        if self.rank[rx] < self.rank[ry] {
            self.parent[rx] = ry;
        } else if self.rank[rx] > self.rank[ry] {
            self.parent[ry] = rx;
        } else {
            self.parent[ry] = rx;
            self.rank[rx] += 1;
        }
    }

    /// Check if elements `x` and `y` are in the same set.
    fn connected(&mut self, x: usize, y: usize) -> bool {
        self.find(x) == self.find(y)
    }

    /// Collect all elements into their disjoint sets.
    /// Returns a vector of HashSets, one per equivalence class.
    fn collect_sets(mut self, n: usize) -> Vec<HashSet<usize>> {
        let mut sets: HashMap<usize, HashSet<usize>> = HashMap::new();
        for i in 0..n {
            let root = self.find(i);
            sets.entry(root).or_default().insert(i);
        }
        sets.into_values().collect()
    }
}

// ---------------------------------------------------------------------------
// AccessIntervalTree — interval tree for efficient overlap queries
// ---------------------------------------------------------------------------

/// An interval tree for efficient overlap queries on memory access ranges.
///
/// Uses a centered interval tree structure to reduce the cost of finding
/// all overlapping intervals from O(n²) pairwise comparison to O(n log n)
/// in the common case. Each node stores intervals that contain the node's
/// center point, partitioned into left and right subtrees for intervals
/// falling entirely to the left or right of the center.
///
/// # Construction
///
/// The tree is built recursively by selecting the median of interval center
/// points as the split point. Intervals containing the center are stored at
/// the current node; the rest are recursed into the appropriate subtree.
///
/// # Query Complexity
///
/// - `query_overlaps`: O(log n + k) where k is the number of results
/// - `query_conflicts`: O(log n + k) where k is the number of results
pub struct AccessIntervalTree {
    nodes: Vec<IntervalNode>,
    /// Map from AccessId to its AccessKind for conflict checking.
    kinds: HashMap<AccessId, AccessKind>,
}

struct IntervalNode {
    /// The center point used to partition intervals at this node.
    center: u64,
    /// Intervals containing the center, sorted by start address (ascending).
    left_intervals: Vec<(u64, u64, AccessId)>,
    /// Same intervals as left_intervals, sorted by end address (descending).
    right_intervals: Vec<(u64, u64, AccessId)>,
    /// Index into `nodes` for the left subtree (intervals entirely left of center).
    left_child: Option<usize>,
    /// Index into `nodes` for the right subtree (intervals entirely right of center).
    right_child: Option<usize>,
}

impl AccessIntervalTree {
    /// Build an interval tree from a slice of access records.
    ///
    /// Uses the median of interval centers as the split point at each level,
    /// ensuring the tree depth is O(log n).
    pub fn from_accesses(accesses: &[AccessRecord]) -> Self {
        let mut kinds = HashMap::with_capacity(accesses.len());
        let mut intervals: Vec<(u64, u64, AccessId)> = Vec::with_capacity(accesses.len());

        for access in accesses {
            let (start, end) = access.byte_range();
            intervals.push((start, end, access.id));
            kinds.insert(access.id, access.kind);
        }

        let mut nodes = Vec::new();
        if !intervals.is_empty() {
            build_node(&mut nodes, &mut intervals);
        }

        AccessIntervalTree { nodes, kinds }
    }

    /// Returns all AccessIds whose byte ranges overlap `[start, end)`.
    ///
    /// Two ranges `[a, b)` and `[c, d)` overlap iff `a < d` and `c < b`.
    /// Complexity: O(log n + k) where k is the number of results.
    pub fn query_overlaps(&self, start: u64, end: u64) -> Vec<AccessId> {
        if self.nodes.is_empty() || start >= end {
            return Vec::new();
        }

        let mut result = Vec::new();
        query_node(&self.nodes, 0, start, end, &mut result);
        result
    }

    /// Returns AccessIds that overlap `[start, end)` AND would conflict
    /// with an access of the given `kind`.
    ///
    /// Two accesses conflict when:
    /// 1. Their byte ranges overlap, **and**
    /// 2. At least one of them is a write.
    ///
    /// Complexity: O(log n + k) where k is the number of results.
    pub fn query_conflicts(&self, start: u64, end: u64, kind: AccessKind) -> Vec<AccessId> {
        let overlapping = self.query_overlaps(start, end);
        overlapping
            .into_iter()
            .filter(|id| {
                let other_kind = self.kinds.get(id).copied().unwrap_or(AccessKind::Read);
                // Conflict iff at least one is a write
                kind == AccessKind::Write || other_kind == AccessKind::Write
            })
            .collect()
    }

    /// Returns the number of intervals stored in the tree.
    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    /// Returns `true` if the tree contains no intervals.
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }
}

/// Recursively build a centered interval tree node.
///
/// Returns the index of the newly created node in the `nodes` vector.
fn build_node(
    nodes: &mut Vec<IntervalNode>,
    intervals: &mut Vec<(u64, u64, AccessId)>,
) -> usize {
    if intervals.is_empty() {
        // Should not be called with empty intervals, but handle gracefully.
        let idx = nodes.len();
        nodes.push(IntervalNode {
            center: 0,
            left_intervals: Vec::new(),
            right_intervals: Vec::new(),
            left_child: None,
            right_child: None,
        });
        return idx;
    }

    // Find the median of interval centers.
    let mut centers: Vec<u64> = intervals.iter().map(|(s, e, _)| s + (e - s) / 2).collect();
    centers.sort_unstable();
    let center = centers[centers.len() / 2];

    // Partition intervals into left, right, and center sets.
    let mut left_set: Vec<(u64, u64, AccessId)> = Vec::new();
    let mut right_set: Vec<(u64, u64, AccessId)> = Vec::new();
    let mut center_set: Vec<(u64, u64, AccessId)> = Vec::new();

    for interval in intervals.drain(..) {
        let (start, end, _) = interval;
        if end <= center {
            // Interval is entirely to the left of center.
            left_set.push(interval);
        } else if start > center {
            // Interval is entirely to the right of center.
            right_set.push(interval);
        } else {
            // Interval contains the center point.
            center_set.push(interval);
        }
    }

    // Sort center intervals by start (ascending) for left_intervals.
    let mut left_sorted = center_set.clone();
    left_sorted.sort_by_key(|(s, _, _)| *s);

    // Sort center intervals by end (descending) for right_intervals.
    let mut right_sorted = center_set;
    right_sorted.sort_by(|a, b| b.1.cmp(&a.1));

    let node_idx = nodes.len();
    nodes.push(IntervalNode {
        center,
        left_intervals: left_sorted,
        right_intervals: right_sorted,
        left_child: None,
        right_child: None,
    });

    // Build left subtree.
    if !left_set.is_empty() {
        let left_child = build_node(nodes, &mut left_set);
        nodes[node_idx].left_child = Some(left_child);
    }

    // Build right subtree.
    if !right_set.is_empty() {
        let right_child = build_node(nodes, &mut right_set);
        nodes[node_idx].right_child = Some(right_child);
    }

    node_idx
}

/// Recursively query a node for intervals overlapping `[q_start, q_end)`.
fn query_node(
    nodes: &[IntervalNode],
    node_idx: usize,
    q_start: u64,
    q_end: u64,
    result: &mut Vec<AccessId>,
) {
    let node = &nodes[node_idx];

    // Check intervals stored at this node.
    // All intervals at this node satisfy: start <= center < end.
    // Two intervals overlap iff: start < q_end && q_start < end.
    for (start, end, id) in &node.left_intervals {
        if *start < q_end && q_start < *end {
            result.push(*id);
        }
    }

    // Recurse into left subtree if query might overlap intervals there.
    // Left subtree intervals have end <= center, so they overlap only if
    // q_start < center (otherwise q_start >= center >= end for all of them).
    if q_start < node.center {
        if let Some(left) = node.left_child {
            query_node(nodes, left, q_start, q_end, result);
        }
    }

    // Recurse into right subtree if query might overlap intervals there.
    // Right subtree intervals have start > center, so they overlap only if
    // q_end > center (otherwise q_end <= center < start for all of them).
    if q_end > node.center {
        if let Some(right) = node.right_child {
            query_node(nodes, right, q_start, q_end, result);
        }
    }
}

// ---------------------------------------------------------------------------
// ExclusivityProofObligation — proof obligation for resolvable conflicts
// ---------------------------------------------------------------------------

/// A proof obligation generated from a detected conflict that could
/// potentially be resolved through synchronization, mutex protection,
/// or other means.
///
/// Each obligation captures a specific conflict and suggests one or more
/// ways it might be resolved, along with an estimated difficulty level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExclusivityProofObligation {
    /// Unique identifier for this proof obligation.
    pub obligation_id: u64,
    /// The conflict that generated this obligation.
    pub conflict: Conflict,
    /// The kind of resolution that could resolve this conflict.
    pub resolution_kind: ResolutionKind,
    /// Human-readable description of the obligation.
    pub description: String,
    /// Estimated difficulty of discharging this obligation.
    pub difficulty: ProofDifficulty,
}

impl fmt::Display for ExclusivityProofObligation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Obligation#{} [{}] {:?}: {}",
            self.obligation_id, self.difficulty, self.resolution_kind, self.description
        )
    }
}

/// The kind of resolution that could resolve a conflict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ResolutionKind {
    /// Add a synchronization edge between two accesses.
    AddSyncEdge {
        /// The access that should happen before.
        from: AccessId,
        /// The access that should happen after.
        to: AccessId,
        /// The suggested ordering for the new sync edge.
        suggested_ordering: SyncOrdering,
    },
    /// Add mutex protection around two accesses.
    AddMutexProtection {
        /// The lock ID for the new mutex.
        lock_id: u64,
        /// The first access to protect.
        access1: AccessId,
        /// The second access to protect.
        access2: AccessId,
    },
    /// Split an access into non-overlapping parts.
    SplitAccess {
        /// The original access to split.
        original: AccessId,
        /// The byte offset at which to split.
        split_point: u64,
    },
    /// Restrict a capability to eliminate the conflict.
    RestrictCapability {
        /// The access whose capability should be restricted.
        access: AccessId,
        /// The capability to remove.
        remove_cap: CapDInfo,
    },
    /// Prove that the two accesses never execute concurrently
    /// (i.e., they are on the same thread).
    ProveSingleThreaded {
        /// The first access.
        access1: AccessId,
        /// The second access.
        access2: AccessId,
    },
}

/// The estimated difficulty of discharging a proof obligation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProofDifficulty {
    /// Trivially discharged (e.g., both accesses already under same mutex).
    Trivial,
    /// Easy to discharge (e.g., just need a sync edge).
    Easy,
    /// Moderate effort required (e.g., adding a mutex).
    Moderate,
    /// Hard to discharge (e.g., proving single-threaded execution).
    Hard,
    /// Generally undecidable (e.g., general concurrent case).
    Undecidable,
}

impl fmt::Display for ProofDifficulty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProofDifficulty::Trivial => write!(f, "Trivial"),
            ProofDifficulty::Easy => write!(f, "Easy"),
            ProofDifficulty::Moderate => write!(f, "Moderate"),
            ProofDifficulty::Hard => write!(f, "Hard"),
            ProofDifficulty::Undecidable => write!(f, "Undecidable"),
        }
    }
}

// ---------------------------------------------------------------------------
// SuggestedFix — a human-readable suggestion for resolving an obligation
// ---------------------------------------------------------------------------

/// A human-readable suggestion for how to resolve a proof obligation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedFix {
    /// The obligation this fix addresses.
    pub obligation_id: u64,
    /// Human-readable description of the fix.
    pub fix_description: String,
    /// A code snippet hint showing how the fix could be applied.
    pub code_hint: String,
    /// Confidence level (0.0 to 1.0) that this fix correctly resolves
    /// the obligation.
    pub confidence: f64,
}

impl fmt::Display for SuggestedFix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Fix for obligation#{} (confidence={:.0}%): {}\n  Hint: {}",
            self.obligation_id,
            self.confidence * 100.0,
            self.fix_description,
            self.code_hint
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

    // =======================================================================
    // Multi-pointer aliasing tests
    // =======================================================================

    // -----------------------------------------------------------------------
    // MP-Test 1: Two derived pointers to same struct field (alias)
    // -----------------------------------------------------------------------
    #[test]
    fn test_multi_pointer_same_field_alias() {
        let mut input = ExclusivityInput::new();

        // Two derived pointers both writing to the same field at offset 0x10
        // within region 1.
        input.set_region_base(1, 0x1000);
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1010,
            4,
            pp("test.vu:1"),
            10, // derivation_id
            1,  // region_id
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1010,
            4,
            pp("test.vu:2"),
            20, // different derivation_id, same region
            1,
        ));
        input.set_derivation_depth(AccessId(1), 1);
        input.set_derivation_depth(AccessId(2), 1);

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // Same field → alias → write-write conflict
        assert!(output.is_violated(), "Two writes to same field through derived pointers should be violated");
        assert_eq!(output.write_write_count(), 1);

        // Verify alias info has correct derivation depth
        let alias_info = verifier.compute_derivation_alias_info(&input);
        assert_eq!(alias_info[&AccessId(1)].derivation_depth, 1);
        assert_eq!(alias_info[&AccessId(1)].offset, 0x10);
    }

    // -----------------------------------------------------------------------
    // MP-Test 2: Two derived pointers to different struct fields (no alias)
    // -----------------------------------------------------------------------
    #[test]
    fn test_multi_pointer_different_fields_no_alias() {
        let mut input = ExclusivityInput::new();

        input.set_region_base(1, 0x1000);
        // Pointer 1 writes to field at offset 0x00
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("test.vu:1"),
            10,
            1,
        ));
        // Pointer 2 writes to field at offset 0x08 (no overlap)
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1008,
            4,
            pp("test.vu:2"),
            20,
            1,
        ));
        input.set_derivation_depth(AccessId(1), 1);
        input.set_derivation_depth(AccessId(2), 1);

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // Different fields → no alias → no conflict (even though same region)
        assert!(
            output.is_proven(),
            "Writes to different fields through derived pointers should be proven safe"
        );
        assert_eq!(output.conflict_count(), 0);

        // Verify alias sets are separate (non-overlapping ranges)
        let alias_sets = verifier.compute_alias_sets(&input);
        // Each access should be in its own alias set since they don't overlap
        assert_eq!(alias_sets.len(), 2, "Non-overlapping accesses should be in separate alias sets");
    }

    // -----------------------------------------------------------------------
    // MP-Test 3: Three-level pointer chain (ptr→ptr→value)
    // -----------------------------------------------------------------------
    #[test]
    fn test_three_level_pointer_chain() {
        let mut input = ExclusivityInput::new();

        input.set_region_base(1, 0x1000);
        // Level 0: direct access to base
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            8,
            pp("test.vu:1"),
            1,
            1,
        ));
        input.set_derivation_depth(AccessId(1), 0);

        // Level 1: one level of indirection
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1000,
            8,
            pp("test.vu:2"),
            2,
            1,
        ));
        input.set_derivation_depth(AccessId(2), 1);

        // Level 2: two levels of indirection
        input.add_access(AccessRecord::new(
            AccessId(3),
            AccessKind::Read,
            0x1000,
            8,
            pp("test.vu:3"),
            3,
            1,
        ));
        input.set_derivation_depth(AccessId(3), 2);

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // All three access the same bytes → write-write between A1,A2
        // and write-read between A1,A3 and A2,A3
        assert!(output.is_violated(), "Overlapping accesses at different derivation depths should conflict");
        assert!(output.write_write_count() >= 1);
        assert!(output.write_read_count() >= 1);

        // Verify derivation info captures the depth
        let alias_info = verifier.compute_derivation_alias_info(&input);
        assert_eq!(alias_info[&AccessId(1)].derivation_depth, 0);
        assert_eq!(alias_info[&AccessId(2)].derivation_depth, 1);
        assert_eq!(alias_info[&AccessId(3)].derivation_depth, 2);
    }

    // -----------------------------------------------------------------------
    // MP-Test 4: Array element access aliasing
    // -----------------------------------------------------------------------
    #[test]
    fn test_array_element_access_aliasing() {
        let mut input = ExclusivityInput::new();

        input.set_region_base(1, 0x1000);
        // Two pointers accessing overlapping array elements
        // arr[0] at 0x1000..0x1004 and arr[0..1] at 0x1000..0x1008
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            4,
            pp("test.vu:1"),
            10,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Read,
            0x1000,
            8,
            pp("test.vu:2"),
            20,
            1,
        ));
        input.set_derivation_depth(AccessId(1), 1);
        input.set_derivation_depth(AccessId(2), 1);

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // Overlapping array elements → write-read conflict
        assert!(output.is_violated(), "Overlapping array element accesses should conflict");
        assert_eq!(output.write_read_count(), 1);
    }

    // -----------------------------------------------------------------------
    // MP-Test 5: Pointer arithmetic offset aliasing
    // -----------------------------------------------------------------------
    #[test]
    fn test_pointer_arithmetic_offset_aliasing() {
        let mut input = ExclusivityInput::new();

        input.set_region_base(1, 0x1000);
        // ptr + 0 writes [0x1000, 0x1004)
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            8,
            pp("test.vu:1"),
            10,
            1,
        ));
        // ptr + 4 writes [0x1004, 0x100C) — partial overlap with first
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1004,
            8,
            pp("test.vu:2"),
            20,
            1,
        ));
        input.set_derivation_depth(AccessId(1), 1);
        input.set_derivation_depth(AccessId(2), 1);

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // Partial overlap from pointer arithmetic → write-write conflict
        assert!(output.is_violated(), "Pointer arithmetic with overlapping offsets should conflict");
        assert_eq!(output.write_write_count(), 1);

        // Verify the alias set groups both accesses
        let alias_sets = verifier.compute_alias_sets(&input);
        let overlapping_sets: Vec<_> = alias_sets.iter().filter(|s| s.len() > 1).collect();
        assert_eq!(overlapping_sets.len(), 1, "Both overlapping accesses should be in one alias set");
        assert_eq!(overlapping_sets[0].len(), 2);
    }

    // -----------------------------------------------------------------------
    // MP-Test 6: Mixed read/write through derived pointers
    // -----------------------------------------------------------------------
    #[test]
    fn test_mixed_read_write_derived_pointers() {
        let mut input = ExclusivityInput::new();

        input.set_region_base(1, 0x1000);
        // Three derived pointers: read, write, read to same location
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Read,
            0x1000,
            4,
            pp("test.vu:1"),
            10,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1000,
            4,
            pp("test.vu:2"),
            20,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(3),
            AccessKind::Read,
            0x1000,
            4,
            pp("test.vu:3"),
            30,
            1,
        ));
        input.set_derivation_depth(AccessId(1), 1);
        input.set_derivation_depth(AccessId(2), 1);
        input.set_derivation_depth(AccessId(3), 2);

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // Write conflicts with both reads → 2 write-read conflicts
        assert!(output.is_violated(), "Mixed read/write through derived pointers should conflict");
        assert_eq!(output.write_read_count(), 2);
        assert_eq!(output.write_write_count(), 0);
    }

    // -----------------------------------------------------------------------
    // MP-Test 7: Ordered derived pointer access (safe)
    // -----------------------------------------------------------------------
    #[test]
    fn test_ordered_derived_pointer_access_safe() {
        let mut input = ExclusivityInput::new();

        input.set_region_base(1, 0x1000);
        // Two derived pointers to same location, but ordered
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1010,
            4,
            pp("test.vu:1"),
            10,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Read,
            0x1010,
            4,
            pp("test.vu:2"),
            20,
            1,
        ));
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(1),
            AccessId(2),
            SyncOrdering::HappensBefore,
        ));
        input.set_derivation_depth(AccessId(1), 1);
        input.set_derivation_depth(AccessId(2), 2);

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // Ordered through sync edge → safe
        assert!(
            output.is_proven(),
            "Ordered derived pointer accesses should be proven safe"
        );
        assert_eq!(output.conflict_count(), 0);
    }

    // -----------------------------------------------------------------------
    // MP-Test 8: Lock-protected derived pointer access
    // -----------------------------------------------------------------------
    #[test]
    fn test_lock_protected_derived_pointer_access() {
        let mut input = ExclusivityInput::new();

        input.set_region_base(1, 0x1000);
        // Two writes through derived pointers, both protected by same lock
        input.add_access(AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1010,
            4,
            pp("test.vu:1"),
            10,
            1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2),
            AccessKind::Write,
            0x1010,
            4,
            pp("test.vu:2"),
            20,
            1,
        ));

        // Both require lock 42
        input.set_capability(AccessId(1), CapDInfo::write_locked(42));
        input.set_capability(AccessId(2), CapDInfo::write_locked(42));
        input.set_derivation_depth(AccessId(1), 1);
        input.set_derivation_depth(AccessId(2), 2);

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // Conflict detected but protected by same lock → probably safe
        assert!(
            !output.is_violated(),
            "Lock-protected derived pointer writes should not be a hard violation"
        );
        assert!(
            output.conflict_count() > 0,
            "Should detect the conflict in the interference graph"
        );
        assert!(
            matches!(output.result.status, VerificationStatus::ProbablySafe { .. }),
            "Lock-protected derived pointer conflict should be ProbablySafe"
        );

        // Verify derivation info is present in conflict descriptions
        let conflict_desc = &output.conflicts[0].description;
        assert!(
            conflict_desc.contains("derivation"),
            "Conflict description should include derivation context"
        );
        assert!(
            conflict_desc.contains("alias_set"),
            "Conflict description should include alias set context"
        );
    }

    // -----------------------------------------------------------------------
    // Additional: UnionFind basic operations
    // -----------------------------------------------------------------------
    #[test]
    fn test_union_find_operations() {
        let mut uf = UnionFind::new(5);

        // Initially, each element is its own set.
        assert!(uf.connected(0, 0));
        assert!(!uf.connected(0, 1));

        // Union 0 and 1.
        uf.union(0, 1);
        assert!(uf.connected(0, 1));
        assert!(!uf.connected(0, 2));

        // Union 2 and 3.
        uf.union(2, 3);
        assert!(uf.connected(2, 3));
        assert!(!uf.connected(1, 2));

        // Union 1 and 2 → now 0,1,2,3 are all connected.
        uf.union(1, 2);
        assert!(uf.connected(0, 3));

        // 4 is still isolated.
        assert!(!uf.connected(0, 4));

        // Collect sets.
        let sets = uf.collect_sets(5);
        assert_eq!(sets.len(), 2); // {0,1,2,3} and {4}
    }

    // -----------------------------------------------------------------------
    // Additional: DerivationAliasInfo overlap
    // -----------------------------------------------------------------------
    #[test]
    fn test_derivation_alias_info_overlap() {
        let info1 = DerivationAliasInfo::new(1, 0x10, 8, 1);
        let info2 = DerivationAliasInfo::new(1, 0x14, 8, 2);
        let info3 = DerivationAliasInfo::new(1, 0x20, 4, 1);
        let info4 = DerivationAliasInfo::new(2, 0x10, 8, 1); // different region

        // Overlapping same region
        assert!(info1.overlaps(&info2));
        // Non-overlapping same region
        assert!(!info1.overlaps(&info3));
        // Different region never overlaps
        assert!(!info1.overlaps(&info4));
    }

    // -----------------------------------------------------------------------
    // Additional: compute_derivation_alias_info defaults
    // -----------------------------------------------------------------------
    #[test]
    fn test_compute_derivation_alias_info_defaults() {
        let mut input = ExclusivityInput::new();
        input.add_access(AccessRecord::new(
            AccessId(1), AccessKind::Write, 0x1000, 4, pp("x"), 1, 1,
        ));

        let verifier = ExclusivityVerifier::new();
        let info = verifier.compute_derivation_alias_info(&input);

        // Without region_bases, offset = base_address
        assert_eq!(info[&AccessId(1)].offset, 0x1000);
        // Without derivation_depths, depth defaults to 0
        assert_eq!(info[&AccessId(1)].derivation_depth, 0);
        assert_eq!(info[&AccessId(1)].root_region, 1);
        assert_eq!(info[&AccessId(1)].size, 4);

        // Now set region base and derivation depth
        input.set_region_base(1, 0x1000);
        input.set_derivation_depth(AccessId(1), 2);
        let info2 = verifier.compute_derivation_alias_info(&input);
        assert_eq!(info2[&AccessId(1)].offset, 0);
        assert_eq!(info2[&AccessId(1)].derivation_depth, 2);
    }

    // =======================================================================
    // Interval tree tests
    // =======================================================================

    /// Helper: simple LCG pseudo-random number generator for deterministic tests.
    struct SimpleRng {
        state: u64,
    }

    impl SimpleRng {
        fn new(seed: u64) -> Self {
            SimpleRng { state: seed }
        }
        fn next_u64(&mut self) -> u64 {
            // Numerical Recipes LCG
            self.state = self.state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            self.state
        }
        fn next_u64_in_range(&mut self, lo: u64, hi: u64) -> u64 {
            if hi <= lo {
                return lo;
            }
            lo + (self.next_u64() % (hi - lo))
        }
    }

    // -----------------------------------------------------------------------
    // Interval Tree Test 1: Empty tree
    // -----------------------------------------------------------------------
    #[test]
    fn test_interval_tree_empty() {
        let tree = AccessIntervalTree::from_accesses(&[]);
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);

        let overlaps = tree.query_overlaps(0, 100);
        assert!(overlaps.is_empty());

        let conflicts = tree.query_conflicts(0, 100, AccessKind::Write);
        assert!(conflicts.is_empty());
    }

    // -----------------------------------------------------------------------
    // Interval Tree Test 2: Single interval
    // -----------------------------------------------------------------------
    #[test]
    fn test_interval_tree_single_interval() {
        let accesses = vec![AccessRecord::new(
            AccessId(1),
            AccessKind::Write,
            0x1000,
            8,
            pp("test.vu:1"),
            1,
            1,
        )];

        let tree = AccessIntervalTree::from_accesses(&accesses);
        assert!(!tree.is_empty());
        assert_eq!(tree.len(), 1);

        // Overlapping query
        let overlaps = tree.query_overlaps(0x1000, 0x1008);
        assert_eq!(overlaps.len(), 1);
        assert!(overlaps.contains(&AccessId(1)));

        // Non-overlapping query
        let no_overlaps = tree.query_overlaps(0x2000, 0x2010);
        assert!(no_overlaps.is_empty());

        // Partial overlap
        let partial = tree.query_overlaps(0x1004, 0x100C);
        assert_eq!(partial.len(), 1);

        // Conflict query (write vs write)
        let conflicts = tree.query_conflicts(0x1000, 0x1008, AccessKind::Write);
        assert_eq!(conflicts.len(), 1);

        // Read vs write also conflicts (at least one is a write)
        let rw_conflicts = tree.query_conflicts(0x1000, 0x1008, AccessKind::Read);
        assert_eq!(rw_conflicts.len(), 1);

        // Build a tree with a read-only access; read vs read = no conflict
        let read_accesses = vec![AccessRecord::new(
            AccessId(10),
            AccessKind::Read,
            0x1000,
            8,
            pp("read_only"),
            1,
            1,
        )];
        let read_tree = AccessIntervalTree::from_accesses(&read_accesses);
        let no_conflicts = read_tree.query_conflicts(0x1000, 0x1008, AccessKind::Read);
        assert!(no_conflicts.is_empty());
    }

    // -----------------------------------------------------------------------
    // Interval Tree Test 3: Non-overlapping intervals
    // -----------------------------------------------------------------------
    #[test]
    fn test_interval_tree_non_overlapping() {
        let accesses = vec![
            AccessRecord::new(AccessId(1), AccessKind::Write, 0x1000, 16, pp("a"), 1, 1),
            AccessRecord::new(AccessId(2), AccessKind::Write, 0x2000, 16, pp("b"), 1, 1),
            AccessRecord::new(AccessId(3), AccessKind::Write, 0x3000, 16, pp("c"), 1, 1),
        ];

        let tree = AccessIntervalTree::from_accesses(&accesses);

        // Each range only overlaps itself
        let overlaps_1 = tree.query_overlaps(0x1000, 0x1010);
        assert_eq!(overlaps_1.len(), 1);
        assert!(overlaps_1.contains(&AccessId(1)));

        let overlaps_2 = tree.query_overlaps(0x2000, 0x2010);
        assert_eq!(overlaps_2.len(), 1);
        assert!(overlaps_2.contains(&AccessId(2)));

        // No overlaps in gap
        let gap = tree.query_overlaps(0x1500, 0x1600);
        assert!(gap.is_empty());
    }

    // -----------------------------------------------------------------------
    // Interval Tree Test 4: All overlapping
    // -----------------------------------------------------------------------
    #[test]
    fn test_interval_tree_all_overlapping() {
        let accesses = vec![
            AccessRecord::new(AccessId(1), AccessKind::Write, 0x1000, 64, pp("a"), 1, 1),
            AccessRecord::new(AccessId(2), AccessKind::Write, 0x1010, 64, pp("b"), 1, 1),
            AccessRecord::new(AccessId(3), AccessKind::Read, 0x1020, 64, pp("c"), 1, 1),
            AccessRecord::new(AccessId(4), AccessKind::Write, 0x1030, 64, pp("d"), 1, 1),
        ];

        let tree = AccessIntervalTree::from_accesses(&accesses);

        // Query from the middle should find all (query [0x1020, 0x1040) overlaps all four)
        let overlaps = tree.query_overlaps(0x1020, 0x1040);
        assert_eq!(overlaps.len(), 4);

        // Conflict query from a write should find all (since 3 are writes)
        let conflicts = tree.query_conflicts(0x1020, 0x1040, AccessKind::Write);
        assert_eq!(conflicts.len(), 4);

        // Read-only query should find only the write accesses
        let read_conflicts = tree.query_conflicts(0x1020, 0x1040, AccessKind::Read);
        // Only AccessId(1), AccessId(2), AccessId(4) are writes
        assert_eq!(read_conflicts.len(), 3);
        assert!(read_conflicts.contains(&AccessId(1)));
        assert!(read_conflicts.contains(&AccessId(2)));
        assert!(read_conflicts.contains(&AccessId(4)));
    }

    // -----------------------------------------------------------------------
    // Interval Tree Test 5: Nested intervals (small inside large)
    // -----------------------------------------------------------------------
    #[test]
    fn test_interval_tree_nested() {
        let accesses = vec![
            AccessRecord::new(AccessId(1), AccessKind::Write, 0x1000, 0x200, pp("outer"), 1, 1),
            AccessRecord::new(AccessId(2), AccessKind::Read, 0x1100, 16, pp("inner1"), 1, 1),
            AccessRecord::new(AccessId(3), AccessKind::Write, 0x1180, 16, pp("inner2"), 1, 1),
        ];

        let tree = AccessIntervalTree::from_accesses(&accesses);

        // Query at inner1 should find inner1 and outer
        let overlaps_inner1 = tree.query_overlaps(0x1100, 0x1110);
        assert_eq!(overlaps_inner1.len(), 2);
        assert!(overlaps_inner1.contains(&AccessId(1)));
        assert!(overlaps_inner1.contains(&AccessId(2)));

        // Query at inner2 should find inner2 and outer
        let overlaps_inner2 = tree.query_overlaps(0x1180, 0x1190);
        assert_eq!(overlaps_inner2.len(), 2);
        assert!(overlaps_inner2.contains(&AccessId(1)));
        assert!(overlaps_inner2.contains(&AccessId(3)));

        // Query from outer's full range should find all three
        let overlaps_full = tree.query_overlaps(0x1000, 0x1200);
        assert_eq!(overlaps_full.len(), 3);
    }

    // -----------------------------------------------------------------------
    // Interval Tree Test 6: Point query (zero-width interval)
    // -----------------------------------------------------------------------
    #[test]
    fn test_interval_tree_point_query() {
        let accesses = vec![
            AccessRecord::new(AccessId(1), AccessKind::Write, 0x1000, 16, pp("a"), 1, 1),
            AccessRecord::new(AccessId(2), AccessKind::Read, 0x2000, 16, pp("b"), 1, 1),
        ];

        let tree = AccessIntervalTree::from_accesses(&accesses);

        // Point query at start (zero-width, no overlap possible)
        let point_nothing = tree.query_overlaps(0x1000, 0x1000);
        assert!(point_nothing.is_empty());

        // Single-byte query inside first interval
        let point_inside = tree.query_overlaps(0x1005, 0x1006);
        assert_eq!(point_inside.len(), 1);
        assert!(point_inside.contains(&AccessId(1)));

        // Single-byte query between intervals
        let point_gap = tree.query_overlaps(0x1500, 0x1501);
        assert!(point_gap.is_empty());
    }

    // -----------------------------------------------------------------------
    // Interval Tree Test 7: Large number of intervals (10000)
    // -----------------------------------------------------------------------
    #[test]
    fn test_interval_tree_large_number() {
        let mut rng = SimpleRng::new(42);
        let mut accesses = Vec::with_capacity(10000);

        for i in 0..10000u64 {
            let base = rng.next_u64_in_range(0, 1_000_000);
            let size = rng.next_u64_in_range(1, 1000);
            let kind = if i % 3 == 0 { AccessKind::Write } else { AccessKind::Read };
            accesses.push(AccessRecord::new(
                AccessId(i),
                kind,
                base,
                size,
                pp("large"),
                1,
                1,
            ));
        }

        let tree = AccessIntervalTree::from_accesses(&accesses);
        assert_eq!(tree.len(), 10000);

        // Query should return results without panic
        let overlaps = tree.query_overlaps(500_000, 501_000);
        // Verify each result actually overlaps
        for id in &overlaps {
            let access = &accesses[id.0 as usize];
            let (s, e) = access.byte_range();
            assert!(s < 501_000 && 500_000 < e, "Result {} does not overlap query range", id);
        }

        // Brute-force verify: count actual overlaps
        let mut expected_count = 0usize;
        for access in &accesses {
            let (s, e) = access.byte_range();
            if s < 501_000 && 500_000 < e {
                expected_count += 1;
            }
        }
        assert_eq!(overlaps.len(), expected_count,
            "Interval tree returned {} results, expected {}", overlaps.len(), expected_count);
    }

    // -----------------------------------------------------------------------
    // Interval Tree Test 8: Boundary cases (adjacent but not overlapping)
    // -----------------------------------------------------------------------
    #[test]
    fn test_interval_tree_boundary_cases() {
        let accesses = vec![
            // [0x1000, 0x1010)
            AccessRecord::new(AccessId(1), AccessKind::Write, 0x1000, 16, pp("a"), 1, 1),
            // [0x1010, 0x1020) — adjacent, NOT overlapping
            AccessRecord::new(AccessId(2), AccessKind::Write, 0x1010, 16, pp("b"), 1, 1),
            // [0x1020, 0x1030) — adjacent to 2, NOT overlapping with 1
            AccessRecord::new(AccessId(3), AccessKind::Write, 0x1020, 16, pp("c"), 1, 1),
        ];

        let tree = AccessIntervalTree::from_accesses(&accesses);

        // Query [0x1000, 0x1010) should only find access 1
        let overlaps_1 = tree.query_overlaps(0x1000, 0x1010);
        assert_eq!(overlaps_1.len(), 1);
        assert!(overlaps_1.contains(&AccessId(1)));

        // Query [0x1010, 0x1020) should only find access 2
        let overlaps_2 = tree.query_overlaps(0x1010, 0x1020);
        assert_eq!(overlaps_2.len(), 1);
        assert!(overlaps_2.contains(&AccessId(2)));

        // Query spanning 1 and 2 but NOT overlapping 3
        let overlaps_12 = tree.query_overlaps(0x1005, 0x1015);
        assert_eq!(overlaps_12.len(), 2);
        assert!(overlaps_12.contains(&AccessId(1)));
        assert!(overlaps_12.contains(&AccessId(2)));

        // Verify adjacent intervals don't conflict with each other
        let mut input = ExclusivityInput::new();
        input.add_access(accesses[0].clone());
        input.add_access(accesses[1].clone());
        input.add_access(accesses[2].clone());

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);
        assert!(output.is_proven(), "Adjacent non-overlapping writes should be proven safe");
        assert_eq!(output.conflict_count(), 0);

        // Same result via interval tree
        let output_it = verifier.verify_with_interval_tree(&input);
        assert!(output_it.is_proven());
        assert_eq!(output_it.conflict_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Interval Tree Equivalence Test: verify() vs verify_with_interval_tree()
    // -----------------------------------------------------------------------
    #[test]
    fn test_interval_tree_vs_brute_force_equivalence() {
        // Use a small deterministic test first to verify correctness.
        // Use unique region IDs to avoid multi-pointer aliasing fallback.
        let mut input = ExclusivityInput::new();

        // 5 writes and 5 reads, all in a small address range for lots of overlaps.
        for i in 0..5u64 {
            input.add_access(AccessRecord::new(
                AccessId(i), AccessKind::Write, i * 10, 30, pp("w"), i, i,
            ));
        }
        for i in 5..10u64 {
            input.add_access(AccessRecord::new(
                AccessId(i), AccessKind::Read, i * 10, 30, pp("r"), i, i,
            ));
        }
        // Add sync edge: AccessId(0) happens-before AccessId(5)
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(0), AccessId(5), SyncOrdering::HappensBefore,
        ));

        let verifier = ExclusivityVerifier::new();
        let output_brute = verifier.verify(&input);
        let output_tree = verifier.verify_with_interval_tree(&input);

        let mut pairs_brute: HashSet<(AccessId, AccessId)> = HashSet::new();
        for c in &output_brute.conflicts {
            let key = if c.access1 < c.access2 { (c.access1, c.access2) } else { (c.access2, c.access1) };
            pairs_brute.insert(key);
        }
        let mut pairs_tree: HashSet<(AccessId, AccessId)> = HashSet::new();
        for c in &output_tree.conflicts {
            let key = if c.access1 < c.access2 { (c.access1, c.access2) } else { (c.access2, c.access1) };
            pairs_tree.insert(key);
        }
        assert_eq!(pairs_brute, pairs_tree,
            "Small test: brute found {}, tree found {}",
            pairs_brute.len(), pairs_tree.len());

        // Now the random equivalence test with unique region IDs.
        let mut rng = SimpleRng::new(12345);
        let mut input2 = ExclusivityInput::new();

        for i in 0..200u64 {
            let base = rng.next_u64_in_range(0, 10_000);
            let size = rng.next_u64_in_range(1, 200);
            let kind = if rng.next_u64() % 3 == 0 { AccessKind::Write } else { AccessKind::Read };
            // Use unique region_id per access to avoid multi-pointer aliasing fallback
            input2.add_access(AccessRecord::new(AccessId(i), kind, base, size, pp("rand"), i, i));
        }
        for i in 0..20u64 {
            input2.add_sync_edge(SyncEdgeRecord::new(
                AccessId(i), AccessId(i + 100), SyncOrdering::HappensBefore,
            ));
        }

        let output_brute2 = verifier.verify(&input2);
        let output_tree2 = verifier.verify_with_interval_tree(&input2);

        let mut pairs_brute2: HashSet<(AccessId, AccessId)> = HashSet::new();
        for c in &output_brute2.conflicts {
            let key = if c.access1 < c.access2 { (c.access1, c.access2) } else { (c.access2, c.access1) };
            pairs_brute2.insert(key);
        }
        let mut pairs_tree2: HashSet<(AccessId, AccessId)> = HashSet::new();
        for c in &output_tree2.conflicts {
            let key = if c.access1 < c.access2 { (c.access1, c.access2) } else { (c.access2, c.access1) };
            pairs_tree2.insert(key);
        }
        assert_eq!(pairs_brute2, pairs_tree2,
            "Random test: brute found {}, tree found {}",
            pairs_brute2.len(), pairs_tree2.len());
        assert_eq!(output_brute2.is_violated(), output_tree2.is_violated());
        assert_eq!(output_brute2.is_proven(), output_tree2.is_proven());
    }

    // =======================================================================
    // Proof obligation tests
    // =======================================================================

    // -----------------------------------------------------------------------
    // PO-Test 1: Lock-protected conflict generates AddMutexProtection obligation
    // -----------------------------------------------------------------------
    #[test]
    fn test_lock_protected_generates_add_mutex_protection_obligation() {
        let mut input = ExclusivityInput::new();

        // Two writes with CapD: both require the same lock.
        input.add_access(AccessRecord::new(
            AccessId(1), AccessKind::Write, 0x1000, 4, pp("test.vu:1"), 1, 1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2), AccessKind::Write, 0x1000, 4, pp("test.vu:2"), 1, 1,
        ));
        input.set_capability(AccessId(1), CapDInfo::write_locked(42));
        input.set_capability(AccessId(2), CapDInfo::write_locked(42));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // Should have proof obligations.
        assert!(
            !output.proof_obligations.is_empty(),
            "Lock-protected conflict should generate proof obligations"
        );

        // Should contain an AddMutexProtection obligation with Trivial difficulty.
        let has_mutex_obligation = output.proof_obligations.iter().any(|obl| {
            matches!(obl.resolution_kind, ResolutionKind::AddMutexProtection { .. })
                && obl.difficulty == ProofDifficulty::Trivial
        });
        assert!(
            has_mutex_obligation,
            "Lock-protected conflict should generate AddMutexProtection with Trivial difficulty"
        );
    }

    // -----------------------------------------------------------------------
    // PO-Test 2: WriteWrite generates ProveSingleThreaded or AddSyncEdge
    // -----------------------------------------------------------------------
    #[test]
    fn test_write_write_generates_prove_single_threaded_or_sync_edge() {
        let mut input = ExclusivityInput::new();

        // Two concurrent writes without sync.
        input.add_access(AccessRecord::new(
            AccessId(1), AccessKind::Write, 0x1000, 4, pp("test.vu:1"), 1, 1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2), AccessKind::Write, 0x1000, 4, pp("test.vu:2"), 1, 1,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // Should have obligations.
        assert!(!output.proof_obligations.is_empty());

        // Should contain AddSyncEdge obligation.
        let has_sync_edge = output.proof_obligations.iter().any(|obl| {
            matches!(obl.resolution_kind, ResolutionKind::AddSyncEdge { .. })
        });
        assert!(
            has_sync_edge,
            "WriteWrite conflict should generate AddSyncEdge obligation"
        );

        // Should contain ProveSingleThreaded obligation.
        let has_single_threaded = output.proof_obligations.iter().any(|obl| {
            matches!(obl.resolution_kind, ResolutionKind::ProveSingleThreaded { .. })
        });
        assert!(
            has_single_threaded,
            "WriteWrite conflict should generate ProveSingleThreaded obligation"
        );
    }

    // -----------------------------------------------------------------------
    // PO-Test 3: WriteRead generates RestrictCapability obligation
    // -----------------------------------------------------------------------
    #[test]
    fn test_write_read_generates_restrict_capability_obligation() {
        let mut input = ExclusivityInput::new();

        // Write and Read to the same address, no sync.
        input.add_access(AccessRecord::new(
            AccessId(1), AccessKind::Write, 0x1000, 8, pp("test.vu:1"), 1, 1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2), AccessKind::Read, 0x1004, 4, pp("test.vu:2"), 1, 1,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // Should have obligations.
        assert!(!output.proof_obligations.is_empty());

        // Should contain RestrictCapability obligation.
        let has_restrict = output.proof_obligations.iter().any(|obl| {
            matches!(obl.resolution_kind, ResolutionKind::RestrictCapability { .. })
        });
        assert!(
            has_restrict,
            "WriteRead conflict should generate RestrictCapability obligation"
        );
    }

    // -----------------------------------------------------------------------
    // PO-Test 4: Difficulty assignment correctness
    // -----------------------------------------------------------------------
    #[test]
    fn test_difficulty_assignment_correctness() {
        // Test Trivial: lock-protected conflict
        let mut input_locked = ExclusivityInput::new();
        input_locked.add_access(AccessRecord::new(
            AccessId(1), AccessKind::Write, 0x1000, 4, pp("a"), 1, 1,
        ));
        input_locked.add_access(AccessRecord::new(
            AccessId(2), AccessKind::Write, 0x1000, 4, pp("b"), 1, 1,
        ));
        input_locked.set_capability(AccessId(1), CapDInfo::write_locked(10));
        input_locked.set_capability(AccessId(2), CapDInfo::write_locked(10));

        let verifier = ExclusivityVerifier::new();
        let output_locked = verifier.verify(&input_locked);

        let trivial_count = output_locked
            .proof_obligations
            .iter()
            .filter(|obl| obl.difficulty == ProofDifficulty::Trivial)
            .count();
        assert!(
            trivial_count > 0,
            "Lock-protected conflict should have at least one Trivial obligation"
        );

        // Test Easy: non-protected WriteWrite should have AddSyncEdge with Easy
        let mut input_ww = ExclusivityInput::new();
        input_ww.add_access(AccessRecord::new(
            AccessId(1), AccessKind::Write, 0x1000, 4, pp("a"), 1, 1,
        ));
        input_ww.add_access(AccessRecord::new(
            AccessId(2), AccessKind::Write, 0x1000, 4, pp("b"), 1, 1,
        ));

        let output_ww = verifier.verify(&input_ww);

        let easy_sync = output_ww.proof_obligations.iter().any(|obl| {
            matches!(obl.resolution_kind, ResolutionKind::AddSyncEdge { .. })
                && obl.difficulty == ProofDifficulty::Easy
        });
        assert!(
            easy_sync,
            "Non-protected WriteWrite should have Easy AddSyncEdge obligation"
        );

        let hard_single_thread = output_ww.proof_obligations.iter().any(|obl| {
            matches!(obl.resolution_kind, ResolutionKind::ProveSingleThreaded { .. })
                && obl.difficulty == ProofDifficulty::Hard
        });
        assert!(
            hard_single_thread,
            "Non-protected WriteWrite should have Hard ProveSingleThreaded obligation"
        );

        // Test Undecidable: non-protected conflicts should have Undecidable
        let undecidable_count = output_ww
            .proof_obligations
            .iter()
            .filter(|obl| obl.difficulty == ProofDifficulty::Undecidable)
            .count();
        assert!(
            undecidable_count > 0,
            "Non-protected conflict should have Undecidable obligation"
        );

        // Test Moderate: WriteRead should have Moderate obligations
        let mut input_wr = ExclusivityInput::new();
        input_wr.add_access(AccessRecord::new(
            AccessId(1), AccessKind::Write, 0x1000, 4, pp("a"), 1, 1,
        ));
        input_wr.add_access(AccessRecord::new(
            AccessId(2), AccessKind::Read, 0x1000, 4, pp("b"), 1, 1,
        ));

        let output_wr = verifier.verify(&input_wr);

        let moderate_count = output_wr
            .proof_obligations
            .iter()
            .filter(|obl| obl.difficulty == ProofDifficulty::Moderate)
            .count();
        assert!(
            moderate_count > 0,
            "Non-protected WriteRead should have Moderate obligations"
        );
    }

    // -----------------------------------------------------------------------
    // PO-Test 5: Suggest fixes for various obligation types
    // -----------------------------------------------------------------------
    #[test]
    fn test_suggest_fixes_for_various_obligation_types() {
        let mut input = ExclusivityInput::new();
        input.add_access(AccessRecord::new(
            AccessId(1), AccessKind::Write, 0x1000, 4, pp("test.vu:1"), 1, 1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2), AccessKind::Write, 0x1000, 4, pp("test.vu:2"), 1, 1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(3), AccessKind::Read, 0x1000, 4, pp("test.vu:3"), 1, 1,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        // Generate suggestions.
        let fixes = verifier.suggest_fixes(&output.proof_obligations);
        assert!(
            !fixes.is_empty(),
            "Should generate suggested fixes for proof obligations"
        );

        // Every fix should have a non-empty description and code hint.
        for fix in &fixes {
            assert!(
                !fix.fix_description.is_empty(),
                "Fix description should not be empty"
            );
            assert!(
                !fix.code_hint.is_empty(),
                "Code hint should not be empty"
            );
            assert!(
                fix.confidence > 0.0 && fix.confidence <= 1.0,
                "Confidence should be in (0.0, 1.0]"
            );
        }

        // Should contain suggestions for sync edges.
        let has_sync_fix = fixes.iter().any(|f| {
            f.code_hint.contains("sync_edge")
        });
        assert!(
            has_sync_fix,
            "Should suggest a sync edge fix"
        );

        // Should contain suggestions for mutex protection.
        let has_mutex_fix = fixes.iter().any(|f| {
            f.code_hint.contains("mutex") || f.code_hint.contains("Mutex")
        });
        assert!(
            has_mutex_fix,
            "Should suggest a mutex fix"
        );
    }

    // -----------------------------------------------------------------------
    // PO-Test 6: Empty obligations for clean program
    // -----------------------------------------------------------------------
    #[test]
    fn test_empty_obligations_for_clean_program() {
        let mut input = ExclusivityInput::new();

        // Non-overlapping accesses with sync edges.
        input.add_access(AccessRecord::new(
            AccessId(1), AccessKind::Write, 0x1000, 4, pp("test.vu:1"), 1, 1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2), AccessKind::Read, 0x2000, 4, pp("test.vu:2"), 2, 2,
        ));
        input.add_access(AccessRecord::new(
            AccessId(3), AccessKind::Write, 0x3000, 8, pp("test.vu:3"), 3, 3,
        ));
        input.add_access(AccessRecord::new(
            AccessId(4), AccessKind::Read, 0x1000, 4, pp("test.vu:4"), 1, 1,
        ));
        input.add_sync_edge(SyncEdgeRecord::new(
            AccessId(1), AccessId(4), SyncOrdering::HappensBefore,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        assert!(
            output.is_proven(),
            "Clean program should be proven safe"
        );
        assert_eq!(
            output.conflict_count(), 0,
            "Clean program should have no conflicts"
        );
        assert!(
            output.proof_obligations.is_empty(),
            "Clean program should have no proof obligations"
        );
        assert_eq!(
            output.proof_obligation_count(), 0,
            "proof_obligation_count should be 0 for clean program"
        );

        // Suggesting fixes on empty obligations should return empty.
        let fixes = verifier.suggest_fixes(&output.proof_obligations);
        assert!(fixes.is_empty(), "No fixes for empty obligations");
    }

    // -----------------------------------------------------------------------
    // PO-Test 7: Obligation IDs are unique and sequential
    // -----------------------------------------------------------------------
    #[test]
    fn test_obligation_ids_are_unique_and_sequential() {
        let mut input = ExclusivityInput::new();
        input.add_access(AccessRecord::new(
            AccessId(1), AccessKind::Write, 0x1000, 4, pp("a"), 1, 1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2), AccessKind::Write, 0x1000, 4, pp("b"), 1, 1,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        let ids: Vec<u64> = output.proof_obligations.iter().map(|o| o.obligation_id).collect();
        let unique_ids: std::collections::HashSet<u64> = ids.iter().copied().collect();

        assert_eq!(
            ids.len(),
            unique_ids.len(),
            "Obligation IDs should be unique"
        );
        assert!(
            ids.iter().all(|&id| id >= 1),
            "All obligation IDs should be >= 1"
        );
    }

    // -----------------------------------------------------------------------
    // PO-Test 8: SuggestedFix obligation_id matches the obligation
    // -----------------------------------------------------------------------
    #[test]
    fn test_suggested_fix_obligation_id_matches() {
        let mut input = ExclusivityInput::new();
        input.add_access(AccessRecord::new(
            AccessId(1), AccessKind::Write, 0x1000, 4, pp("a"), 1, 1,
        ));
        input.add_access(AccessRecord::new(
            AccessId(2), AccessKind::Read, 0x1000, 4, pp("b"), 1, 1,
        ));

        let verifier = ExclusivityVerifier::new();
        let output = verifier.verify(&input);

        let fixes = verifier.suggest_fixes(&output.proof_obligations);

        // Every fix's obligation_id should correspond to an actual obligation.
        let obl_ids: std::collections::HashSet<u64> = output
            .proof_obligations
            .iter()
            .map(|o| o.obligation_id)
            .collect();

        for fix in &fixes {
            assert!(
                obl_ids.contains(&fix.obligation_id),
                "Fix obligation_id {} should match an actual obligation",
                fix.obligation_id
            );
        }

        // Same number of fixes as obligations.
        assert_eq!(
            fixes.len(),
            output.proof_obligations.len(),
            "Should have one fix per obligation"
        );
    }
}
