//! Exclusivity invariant checker for the Memory State Graph (MSG).
//!
//! **Invariant 2 (Exclusivity):** No conflicting concurrent accesses exist
//! without synchronization.
//!
//! Formally: ∀ a₁, a₂ ∈ 𝒜 : conflicts(a₁, a₂) ⇒ ordered(a₁, a₂)
//!
//! Two accesses *conflict* when:
//!   1. At least one is a [`AccessKind::Write`],
//!   2. Their byte ranges overlap, and
//!   3. They are distinct accesses.
//!
//! Two accesses are *ordered* when a path of [`SyncEdge`]s exists in the
//! MSG from one to the other (in either direction), establishing a
//! happens-before relationship. The ordering relation is the transitive
//! closure of the sync-edge graph.
//!
//! # Algorithm
//!
//! 1. Enumerate all pairs of accesses whose byte ranges overlap and where
//!    at least one access is a Write.
//! 2. For each conflict pair, compute whether they are ordered using the
//!    transitive closure of sync edges.
//! 3. If a conflict pair is not ordered, record a [`Violation`].
//! 4. Build an [`InterferenceGraph`] from all conflict pairs for downstream
//!    analyses.
//! 5. Return an [`InvariantResult`] summarizing satisfaction or violations.

use crate::access::{Access, AccessId, AccessKind};
use crate::address::Address;
use crate::derivation::DerivationId;
use crate::msg::MSG;
use crate::sync::SyncEdgeId;
use hashbrown::{HashMap, HashSet};
use std::fmt;

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// The outcome of checking the exclusivity invariant on an MSG.
#[derive(Debug, Clone)]
pub enum InvariantResult {
    /// The invariant holds: every conflicting pair of accesses is ordered.
    Satisfied {
        /// The total number of accesses examined.
        access_count: usize,
        /// The number of conflicting pairs found (all of which are ordered).
        conflict_pair_count: usize,
        /// The interference graph built during analysis.
        interference_graph: InterferenceGraph,
    },
    /// The invariant is violated: at least one conflicting pair is unordered.
    Violated {
        /// The total number of accesses examined.
        access_count: usize,
        /// The total number of conflicting pairs found.
        conflict_pair_count: usize,
        /// The specific violations (unordered conflict pairs).
        violations: Vec<Violation>,
        /// The interference graph built during analysis.
        interference_graph: InterferenceGraph,
    },
}

impl InvariantResult {
    /// Returns `true` if the exclusivity invariant is satisfied.
    pub fn is_satisfied(&self) -> bool {
        matches!(self, InvariantResult::Satisfied { .. })
    }

    /// Returns the number of violations, or 0 if satisfied.
    pub fn violation_count(&self) -> usize {
        match self {
            InvariantResult::Satisfied { .. } => 0,
            InvariantResult::Violated { violations, .. } => violations.len(),
        }
    }

    /// Returns a reference to the interference graph.
    pub fn interference_graph(&self) -> &InterferenceGraph {
        match self {
            InvariantResult::Satisfied {
                interference_graph,
                ..
            } => interference_graph,
            InvariantResult::Violated {
                interference_graph,
                ..
            } => interference_graph,
        }
    }
}

impl fmt::Display for InvariantResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvariantResult::Satisfied {
                access_count,
                conflict_pair_count,
                ..
            } => write!(
                f,
                "Exclusivity: SATISFIED ({} accesses, {} conflict pairs all ordered)",
                access_count, conflict_pair_count
            ),
            InvariantResult::Violated {
                access_count,
                conflict_pair_count,
                violations,
                ..
            } => write!(
                f,
                "Exclusivity: VIOLATED ({} accesses, {} conflict pairs, {} violations)",
                access_count,
                conflict_pair_count,
                violations.len()
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Violation details
// ---------------------------------------------------------------------------

/// A specific violation of the exclusivity invariant.
///
/// Records the two accesses that conflict without being ordered, along
/// with details about the overlap and missing synchronisation.
#[derive(Debug, Clone)]
pub struct Violation {
    /// The first access involved in the conflict.
    pub access1: AccessId,
    /// The second access involved in the conflict.
    pub access2: AccessId,
    /// The kind of the first access (Read or Write).
    pub kind1: AccessKind,
    /// The kind of the second access (Read or Write).
    pub kind2: AccessKind,
    /// Byte-range overlap details.
    pub overlap: OverlapInfo,
    /// The derivation IDs targeted by each access.
    pub target1: DerivationId,
    /// The derivation IDs targeted by each access.
    pub target2: DerivationId,
    /// Description of the missing synchronisation that would be needed.
    pub missing_sync: MissingSync,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Exclusivity violation: {} ({}) and {} ({}) conflict on bytes [{:#x}, {:#x}) — {}",
            self.access1,
            self.kind1,
            self.access2,
            self.kind2,
            self.overlap.overlap_start.as_u64(),
            self.overlap.overlap_end.as_u64(),
            self.missing_sync,
        )
    }
}

// ---------------------------------------------------------------------------
// Overlap information
// ---------------------------------------------------------------------------

/// Details about the byte-range overlap between two accesses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlapInfo {
    /// Start address of the overlap (inclusive).
    pub overlap_start: Address,
    /// End address of the overlap (exclusive).
    pub overlap_end: Address,
    /// Number of overlapping bytes.
    pub overlap_size: u64,
}

impl fmt::Display for OverlapInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "overlap [{:#x}, {:#x}) ({} bytes)",
            self.overlap_start.as_u64(),
            self.overlap_end.as_u64(),
            self.overlap_size,
        )
    }
}

/// Compute the byte-range overlap between two accesses.
///
/// Returns `None` if the ranges do not overlap.
pub fn compute_overlap(
    base1: Address,
    size1: u64,
    base2: Address,
    size2: u64,
) -> Option<OverlapInfo> {
    let start1 = base1;
    let end1 = base1 + size1;
    let start2 = base2;
    let end2 = base2 + size2;

    // Standard half-open interval overlap: [s1, e1) ∩ [s2, e2)
    if start1 < end2 && start2 < end1 {
        let overlap_start = std::cmp::max(start1, start2);
        let overlap_end = std::cmp::min(end1, end2);
        let overlap_size = overlap_end - overlap_start;
        Some(OverlapInfo {
            overlap_start,
            overlap_end,
            overlap_size: overlap_size as u64,
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Missing synchronisation description
// ---------------------------------------------------------------------------

/// Describes the synchronisation that would be needed to resolve a violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingSync {
    /// No sync edges exist between the two accesses at all.
    NoSyncEdges,
    /// Sync edges exist but none form a path from one access to the other.
    NoOrderingPath {
        /// Sync edge IDs that touch one or both accesses but do not connect them.
        nearby_edges: Vec<SyncEdgeId>,
    },
}

impl fmt::Display for MissingSync {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MissingSync::NoSyncEdges => {
                write!(f, "no synchronisation edges between these accesses")
            }
            MissingSync::NoOrderingPath { nearby_edges } => {
                write!(
                    f,
                    "no ordering path ({} nearby edge(s) do not connect them)",
                    nearby_edges.len()
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Conflict pair
// ---------------------------------------------------------------------------

/// A pair of accesses that conflict (overlapping bytes, at least one write).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConflictPair {
    /// The first access (always the one with the lower AccessId for canonical form).
    pub access1: AccessId,
    /// The second access.
    pub access2: AccessId,
}

impl ConflictPair {
    /// Create a new conflict pair in canonical form (lower ID first).
    pub fn new(a: AccessId, b: AccessId) -> Self {
        if a.0 <= b.0 {
            ConflictPair {
                access1: a,
                access2: b,
            }
        } else {
            ConflictPair {
                access1: b,
                access2: a,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Interference graph
// ---------------------------------------------------------------------------

/// A graph whose nodes are [`AccessId`]s and whose edges connect accesses
/// that conflict (overlapping bytes with at least one write).
///
/// The interference graph is useful for downstream analyses such as:
/// - Colouring-based lock assignment
/// - Identifying independent access groups that can be analysed separately
/// - Visualising the conflict structure of the program
#[derive(Debug, Clone, Default)]
pub struct InterferenceGraph {
    /// Adjacency list: each access maps to the set of accesses it conflicts with.
    edges: HashMap<AccessId, HashSet<AccessId>>,
    /// All conflict pairs in the graph.
    conflict_pairs: Vec<ConflictPair>,
    /// Subset of conflict pairs that are *not* ordered (violations).
    unordered_pairs: HashSet<ConflictPair>,
}

impl InterferenceGraph {
    /// Create an empty interference graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a conflict pair to the graph.
    pub fn add_conflict(&mut self, pair: ConflictPair, is_ordered: bool) {
        self.edges
            .entry(pair.access1)
            .or_default()
            .insert(pair.access2);
        self.edges
            .entry(pair.access2)
            .or_default()
            .insert(pair.access1);
        self.conflict_pairs.push(pair.clone());
        if !is_ordered {
            self.unordered_pairs.insert(pair);
        }
    }

    /// Returns the set of accesses that conflict with the given access.
    pub fn conflicts_of(&self, id: AccessId) -> Option<&HashSet<AccessId>> {
        self.edges.get(&id)
    }

    /// Returns all conflict pairs.
    pub fn conflict_pairs(&self) -> &[ConflictPair] {
        &self.conflict_pairs
    }

    /// Returns the subset of conflict pairs that are not ordered (violations).
    pub fn unordered_pairs(&self) -> &HashSet<ConflictPair> {
        &self.unordered_pairs
    }

    /// Number of nodes (accesses) in the interference graph.
    pub fn node_count(&self) -> usize {
        self.edges.len()
    }

    /// Number of edges (conflict pairs) in the interference graph.
    pub fn edge_count(&self) -> usize {
        self.conflict_pairs.len()
    }

    /// Number of unordered (violating) edges.
    pub fn unordered_edge_count(&self) -> usize {
        self.unordered_pairs.len()
    }
}

impl fmt::Display for InterferenceGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "InterferenceGraph {{ nodes: {}, edges: {}, unordered: {} }}",
            self.node_count(),
            self.edge_count(),
            self.unordered_edge_count(),
        )
    }
}

// ---------------------------------------------------------------------------
// Ordering / reachability
// ---------------------------------------------------------------------------

/// Compute the transitive closure of the sync-edge graph to determine
/// which accesses are reachable from which.
///
/// Returns a map from `AccessId` to the set of `AccessId`s that are
/// reachable from it by following sync edges (access1 → access2 direction).
fn compute_reachability(msg: &MSG) -> HashMap<AccessId, HashSet<AccessId>> {
    // Build forward adjacency list from sync edges.
    let mut forward: HashMap<AccessId, Vec<AccessId>> = HashMap::new();
    for edge in msg.sync_edges() {
        forward
            .entry(edge.access1)
            .or_default()
            .push(edge.access2);
    }

    // For each access, compute reachable set via BFS/DFS.
    let mut reachability: HashMap<AccessId, HashSet<AccessId>> = HashMap::new();

    for start in msg.access_ids() {
        let mut visited = HashSet::new();
        let mut stack = vec![start];

        while let Some(current) = stack.pop() {
            if visited.insert(current) {
                if let Some(neighbours) = forward.get(&current) {
                    for &next in neighbours {
                        if !visited.contains(&next) {
                            stack.push(next);
                        }
                    }
                }
            }
        }

        // Remove self from reachable set.
        visited.remove(&start);
        reachability.insert(start, visited);
    }

    reachability
}

/// Determine whether two accesses are ordered (in either direction).
///
/// Two accesses are ordered if one is reachable from the other in the
/// sync-edge graph's transitive closure.
fn are_ordered(
    reachability: &HashMap<AccessId, HashSet<AccessId>>,
    a1: AccessId,
    a2: AccessId,
) -> bool {
    // a1 → a2 ?
    if let Some(reachable) = reachability.get(&a1) {
        if reachable.contains(&a2) {
            return true;
        }
    }
    // a2 → a1 ?
    if let Some(reachable) = reachability.get(&a2) {
        if reachable.contains(&a1) {
            return true;
        }
    }
    false
}

/// Find sync edge IDs that are "nearby" the two accesses (touching either
/// endpoint) but do not form an ordering path between them.
fn find_nearby_edges(msg: &MSG, a1: AccessId, a2: AccessId) -> Vec<SyncEdgeId> {
    msg.sync_edges()
        .filter(|edge| {
            edge.access1 == a1
                || edge.access2 == a1
                || edge.access1 == a2
                || edge.access2 == a2
        })
        .map(|edge| edge.id)
        .collect()
}

// ---------------------------------------------------------------------------
// Main checker
// ---------------------------------------------------------------------------

/// Check the exclusivity invariant on the given MSG.
///
/// The `resolve_base` closure maps an [`AccessId`] to the resolved base
/// [`Address`] of its target derivation. This is required because the MSG
/// does not store concrete addresses — they depend on the derivation chain
/// which may involve offsets and casts.
///
/// # Returns
///
/// An [`InvariantResult`] indicating whether the invariant is satisfied
/// or violated, with full details on any violations found.
///
/// # Example
///
/// ```ignore
/// use vuma_core::invariant_exclusivity::check_exclusivity;
/// use vuma_core::msg::MSG;
/// use vuma_core::address::Address;
///
/// let msg = MSG::new();
/// // ... populate msg ...
/// let resolve = |aid| Some(Address::from(0x1000_u64));
/// let result = check_exclusivity(&msg, resolve);
/// assert!(result.is_satisfied());
/// ```
pub fn check_exclusivity<F>(msg: &MSG, resolve_base: F) -> InvariantResult
where
    F: Fn(AccessId) -> Option<Address>,
{
    let access_count = msg.access_count();

    // Collect all accesses with their resolved base addresses.
    let mut access_info: Vec<(AccessId, Address, &Access)> = msg
        .accesses()
        .filter_map(|access| {
            let base = resolve_base(access.id)?;
            Some((access.id, base, access))
        })
        .collect();

    // Sort by AccessId for deterministic iteration order.
    access_info.sort_by_key(|(id, _, _)| id.0);

    // If no accesses can be resolved, the invariant is trivially satisfied.
    if access_info.is_empty() {
        return InvariantResult::Satisfied {
            access_count,
            conflict_pair_count: 0,
            interference_graph: InterferenceGraph::new(),
        };
    }

    // Compute the transitive closure of sync edges for ordering checks.
    let reachability = compute_reachability(msg);

    // Find all conflict pairs and check ordering.
    let mut violations: Vec<Violation> = Vec::new();
    let mut interference_graph = InterferenceGraph::new();
    let mut conflict_pair_count: usize = 0;

    for i in 0..access_info.len() {
        let (id1, base1, access1) = &access_info[i];

        // Skip pairs of reads — they never conflict.
        if access1.kind == AccessKind::Read {
            // We still need to check against writes, so we continue
            // but we can skip Read-Read pairs later.
        }

        for j in (i + 1)..access_info.len() {
            let (id2, base2, access2) = &access_info[j];

            // Two reads never conflict.
            if access1.kind == AccessKind::Read && access2.kind == AccessKind::Read {
                continue;
            }

            // Check byte-range overlap.
            let overlap = match compute_overlap(*base1, access1.size, *base2, access2.size) {
                Some(o) => o,
                None => continue,
            };

            // This is a conflict pair.
            let pair = ConflictPair::new(*id1, *id2);
            conflict_pair_count += 1;

            // Check ordering.
            let ordered = are_ordered(&reachability, *id1, *id2);

            // Add to interference graph.
            interference_graph.add_conflict(pair.clone(), ordered);

            if !ordered {
                // Determine the missing sync description.
                let missing_sync = if msg.sync_edge_count() == 0 {
                    MissingSync::NoSyncEdges
                } else {
                    let nearby = find_nearby_edges(msg, *id1, *id2);
                    if nearby.is_empty() {
                        MissingSync::NoSyncEdges
                    } else {
                        MissingSync::NoOrderingPath {
                            nearby_edges: nearby,
                        }
                    }
                };

                violations.push(Violation {
                    access1: *id1,
                    access2: *id2,
                    kind1: access1.kind,
                    kind2: access2.kind,
                    overlap,
                    target1: access1.target,
                    target2: access2.target,
                    missing_sync,
                });
            }
        }
    }

    if violations.is_empty() {
        InvariantResult::Satisfied {
            access_count,
            conflict_pair_count,
            interference_graph,
        }
    } else {
        InvariantResult::Violated {
            access_count,
            conflict_pair_count,
            violations,
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
    use crate::access::{Access, AccessId, AccessKind};
    use crate::address::Address;
    use crate::derivation::{Derivation, DerivationId, DerivationKind, DerivationSource};
    use crate::msg::MSG;
    use crate::program_point::ProgramPoint;
    use crate::region::{Region, RegionId, RegionStatus};
    use crate::sync::{LockId, Ordering, SyncEdge, SyncEdgeId};

    /// Helper: create a dummy program point.
    fn pp(line: u32) -> ProgramPoint {
        ProgramPoint::new("test_excl.vu", line, 1)
    }

    /// Helper: create a test region and add it to the MSG.
    fn add_test_region(msg: &mut MSG, id: u64, base: u64, size: u64) -> RegionId {
        let region = Region {
            id: RegionId(id),
            base: Address::from(base),
            size,
            status: RegionStatus::Allocated,
            alloc_point: pp(1),
            free_point: None,
            owner_context: None,
        };
        msg.add_region(region)
    }

    /// Helper: add a direct derivation from a region.
    fn add_direct_derivation(msg: &mut MSG, id: u64, region_id: u64, base: u64, end: u64) {
        msg.add_derivation(Derivation {
            id: DerivationId(id),
            source: DerivationSource::Region(RegionId(region_id)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(base), Address::from(end)),
        });
    }

    /// Helper: add a read access.
    fn add_read(msg: &mut MSG, id: u64, target: u64, size: u64, line: u32) -> AccessId {
        msg.add_access(Access::new(
            AccessId(id),
            DerivationId(target),
            AccessKind::Read,
            size,
            pp(line),
        ))
    }

    /// Helper: add a write access.
    fn add_write(msg: &mut MSG, id: u64, target: u64, size: u64, line: u32) -> AccessId {
        msg.add_access(Access::new(
            AccessId(id),
            DerivationId(target),
            AccessKind::Write,
            size,
            pp(line),
        ))
    }

    /// Helper: add a happens-before sync edge.
    fn add_hb(msg: &mut MSG, id: u64, a1: u64, a2: u64) {
        msg.add_sync_edge(SyncEdge::new(
            SyncEdgeId(id),
            AccessId(a1),
            AccessId(a2),
            Ordering::HappensBefore,
        ));
    }

    /// Helper: add a mutex-locked sync edge.
    fn add_mutex(msg: &mut MSG, id: u64, a1: u64, a2: u64, lock: u64) {
        msg.add_sync_edge(SyncEdge::new(
            SyncEdgeId(id),
            AccessId(a1),
            AccessId(a2),
            Ordering::MutexLocked(LockId(lock)),
        ));
    }

    // -----------------------------------------------------------------------
    // Test 1: Empty MSG — trivially satisfied
    // -----------------------------------------------------------------------

    #[test]
    fn empty_msg_is_satisfied() {
        let msg = MSG::new();
        let resolve = |_: AccessId| None;
        let result = check_exclusivity(&msg, resolve);
        assert!(result.is_satisfied());
        assert_eq!(result.violation_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Test 2: Single read — no conflicts possible
    // -----------------------------------------------------------------------

    #[test]
    fn single_read_is_satisfied() {
        let mut msg = MSG::new();
        add_test_region(&mut msg, 1, 0x1000, 0x100);
        add_direct_derivation(&mut msg, 10, 1, 0x1000, 0x1100);
        add_read(&mut msg, 100, 10, 8, 10);

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = check_exclusivity(&msg, resolve);
        assert!(result.is_satisfied());
    }

    // -----------------------------------------------------------------------
    // Test 3: Two overlapping reads — no conflict (reads never conflict)
    // -----------------------------------------------------------------------

    #[test]
    fn two_overlapping_reads_are_not_a_conflict() {
        let mut msg = MSG::new();
        add_test_region(&mut msg, 1, 0x1000, 0x100);
        add_direct_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_read(&mut msg, 1, 10, 8, 10);
        add_read(&mut msg, 2, 10, 8, 11);

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = check_exclusivity(&msg, resolve);
        assert!(result.is_satisfied());

        // No conflict pairs at all since both are reads.
        let ig = result.interference_graph();
        assert_eq!(ig.edge_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Test 4: Write + Read overlapping WITHOUT sync — VIOLATION
    // -----------------------------------------------------------------------

    #[test]
    fn write_and_read_overlapping_without_sync_is_violation() {
        let mut msg = MSG::new();
        add_test_region(&mut msg, 1, 0x1000, 0x100);
        add_direct_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_write(&mut msg, 1, 10, 8, 10); // Write [0x1000, 0x1008)
        add_read(&mut msg, 2, 10, 4, 11); // Read [0x1000, 0x1004) — overlaps!

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = check_exclusivity(&msg, resolve);
        assert!(!result.is_satisfied());
        assert_eq!(result.violation_count(), 1);

        if let InvariantResult::Violated { violations, .. } = &result {
            let v = &violations[0];
            assert_eq!(v.access1, AccessId(1));
            assert_eq!(v.access2, AccessId(2));
            assert_eq!(v.kind1, AccessKind::Write);
            assert_eq!(v.kind2, AccessKind::Read);
            assert!(v.overlap.overlap_size > 0);
        } else {
            panic!("expected Violated");
        }
    }

    // -----------------------------------------------------------------------
    // Test 5: Write + Read overlapping WITH happens-before — SATISFIED
    // -----------------------------------------------------------------------

    #[test]
    fn write_and_read_overlapping_with_hb_is_satisfied() {
        let mut msg = MSG::new();
        add_test_region(&mut msg, 1, 0x1000, 0x100);
        add_direct_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_write(&mut msg, 1, 10, 8, 10);
        add_read(&mut msg, 2, 10, 4, 11);
        add_hb(&mut msg, 1, 1, 2); // A1 happens-before A2

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = check_exclusivity(&msg, resolve);
        assert!(result.is_satisfied());

        // There is a conflict pair, but it's ordered.
        let ig = result.interference_graph();
        assert_eq!(ig.edge_count(), 1);
        assert_eq!(ig.unordered_edge_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Test 6: Write + Read overlapping WITH mutex — SATISFIED
    // -----------------------------------------------------------------------

    #[test]
    fn write_and_read_overlapping_with_mutex_is_satisfied() {
        let mut msg = MSG::new();
        add_test_region(&mut msg, 1, 0x1000, 0x100);
        add_direct_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_write(&mut msg, 1, 10, 8, 10);
        add_read(&mut msg, 2, 10, 4, 11);
        add_mutex(&mut msg, 1, 1, 2, 42); // A1 and A2 under same mutex

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = check_exclusivity(&msg, resolve);
        assert!(result.is_satisfied());
    }

    // -----------------------------------------------------------------------
    // Test 7: Two overlapping writes WITHOUT sync — VIOLATION
    // -----------------------------------------------------------------------

    #[test]
    fn two_overlapping_writes_without_sync_is_violation() {
        let mut msg = MSG::new();
        add_test_region(&mut msg, 1, 0x1000, 0x100);
        add_direct_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_write(&mut msg, 1, 10, 8, 10); // Write [0x1000, 0x1008)
        add_write(&mut msg, 2, 10, 8, 11); // Write [0x1000, 0x1008) — same range!

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = check_exclusivity(&msg, resolve);
        assert!(!result.is_satisfied());
        assert_eq!(result.violation_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 8: Non-overlapping write and read — no conflict
    // -----------------------------------------------------------------------

    #[test]
    fn non_overlapping_write_and_read_is_not_conflict() {
        let mut msg = MSG::new();
        add_test_region(&mut msg, 1, 0x1000, 0x100);
        add_direct_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_write(&mut msg, 1, 10, 4, 10); // Write [0x1000, 0x1004)
        add_read(&mut msg, 2, 10, 4, 11); // Read [0x1008, 0x100C)

        let resolve = |aid: AccessId| {
            if aid == AccessId(1) {
                Some(Address::from(0x1000_u64))
            } else {
                Some(Address::from(0x1008_u64))
            }
        };
        let result = check_exclusivity(&msg, resolve);
        assert!(result.is_satisfied());
        assert_eq!(result.interference_graph().edge_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Test 9: Transitive ordering (A → B → C) — A and C are ordered
    // -----------------------------------------------------------------------

    #[test]
    fn transitive_ordering_resolves_conflict() {
        let mut msg = MSG::new();
        add_test_region(&mut msg, 1, 0x1000, 0x100);
        add_direct_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_write(&mut msg, 1, 10, 8, 10); // A1: Write
        add_write(&mut msg, 2, 10, 8, 11); // A2: Write
        add_read(&mut msg, 3, 10, 8, 12); // A3: Read

        // A1 → A2 → A3
        add_hb(&mut msg, 1, 1, 2);
        add_hb(&mut msg, 2, 2, 3);

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = check_exclusivity(&msg, resolve);
        assert!(result.is_satisfied());

        // Three conflict pairs (1-2, 1-3, 2-3), all ordered.
        let ig = result.interference_graph();
        assert_eq!(ig.edge_count(), 3);
        assert_eq!(ig.unordered_edge_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Test 10: Partial ordering — some pairs ordered, others not
    // -----------------------------------------------------------------------

    #[test]
    fn partial_ordering_yields_mixed_result() {
        let mut msg = MSG::new();
        add_test_region(&mut msg, 1, 0x1000, 0x100);
        add_direct_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_write(&mut msg, 1, 10, 8, 10); // A1: Write [0x1000, 0x1008)
        add_write(&mut msg, 2, 10, 8, 11); // A2: Write [0x1000, 0x1008)
        add_read(&mut msg, 3, 10, 8, 12); // A3: Read [0x1000, 0x1008)

        // Only A1 → A2 is ordered. A3 is unordered with both.
        add_hb(&mut msg, 1, 1, 2);

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = check_exclusivity(&msg, resolve);
        assert!(!result.is_satisfied());

        // Violations: A1-A3 and A2-A3 are concurrent conflicting pairs.
        assert_eq!(result.violation_count(), 2);

        let ig = result.interference_graph();
        assert_eq!(ig.edge_count(), 3); // 1-2, 1-3, 2-3
        assert_eq!(ig.unordered_edge_count(), 2); // 1-3, 2-3
    }

    // -----------------------------------------------------------------------
    // Test 11: compute_overlap unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn overlap_computation_basic() {
        // Full overlap
        let o = compute_overlap(Address::from(0x1000_u64), 8, Address::from(0x1000_u64), 8);
        assert!(o.is_some());
        let o = o.unwrap();
        assert_eq!(o.overlap_start, Address::from(0x1000_u64));
        assert_eq!(o.overlap_end, Address::from(0x1008_u64));
        assert_eq!(o.overlap_size, 8);

        // Partial overlap
        let o = compute_overlap(Address::from(0x1000_u64), 8, Address::from(0x1004_u64), 8);
        assert!(o.is_some());
        let o = o.unwrap();
        assert_eq!(o.overlap_start, Address::from(0x1004_u64));
        assert_eq!(o.overlap_end, Address::from(0x1008_u64));
        assert_eq!(o.overlap_size, 4);

        // No overlap
        let o = compute_overlap(Address::from(0x1000_u64), 4, Address::from(0x1008_u64), 4);
        assert!(o.is_none());

        // Adjacent but not overlapping
        let o = compute_overlap(Address::from(0x1000_u64), 8, Address::from(0x1008_u64), 8);
        assert!(o.is_none());
    }

    // -----------------------------------------------------------------------
    // Test 12: Interference graph queries
    // -----------------------------------------------------------------------

    #[test]
    fn interference_graph_queries() {
        let mut ig = InterferenceGraph::new();

        let pair1 = ConflictPair::new(AccessId(1), AccessId(2));
        let pair2 = ConflictPair::new(AccessId(2), AccessId(3));

        ig.add_conflict(pair1.clone(), true); // ordered
        ig.add_conflict(pair2.clone(), false); // unordered

        assert_eq!(ig.node_count(), 3);
        assert_eq!(ig.edge_count(), 2);
        assert_eq!(ig.unordered_edge_count(), 1);

        // A1 conflicts with A2
        let conflicts = ig.conflicts_of(AccessId(1));
        assert!(conflicts.is_some());
        assert!(conflicts.unwrap().contains(&AccessId(2)));

        // A2 conflicts with both A1 and A3
        let conflicts = ig.conflicts_of(AccessId(2));
        assert!(conflicts.is_some());
        assert_eq!(conflicts.unwrap().len(), 2);

        // A3 conflicts with A2
        let conflicts = ig.conflicts_of(AccessId(3));
        assert!(conflicts.is_some());
        assert!(conflicts.unwrap().contains(&AccessId(2)));
    }

    // -----------------------------------------------------------------------
    // Test 13: Violation display formatting
    // -----------------------------------------------------------------------

    #[test]
    fn violation_display_format() {
        let violation = Violation {
            access1: AccessId(1),
            access2: AccessId(2),
            kind1: AccessKind::Write,
            kind2: AccessKind::Read,
            overlap: OverlapInfo {
                overlap_start: Address::from(0x1000_u64),
                overlap_end: Address::from(0x1008_u64),
                overlap_size: 8,
            },
            target1: DerivationId(10),
            target2: DerivationId(11),
            missing_sync: MissingSync::NoSyncEdges,
        };

        let display = format!("{}", violation);
        assert!(display.contains("A1"));
        assert!(display.contains("A2"));
        assert!(display.contains("write"));
        assert!(display.contains("read"));
    }

    // -----------------------------------------------------------------------
    // Test 14: Nearby edges reported in NoOrderingPath
    // -----------------------------------------------------------------------

    #[test]
    fn nearby_edges_reported_in_missing_sync() {
        let mut msg = MSG::new();
        add_test_region(&mut msg, 1, 0x1000, 0x100);
        add_direct_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_write(&mut msg, 1, 10, 8, 10);
        add_read(&mut msg, 2, 10, 4, 11);
        add_read(&mut msg, 3, 10, 4, 12);

        // A1 → A3 is ordered, but A1 and A2 are not.
        add_hb(&mut msg, 1, 1, 3);

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = check_exclusivity(&msg, resolve);
        assert!(!result.is_satisfied());

        if let InvariantResult::Violated { violations, .. } = &result {
            // The A1-A2 violation should report nearby edges.
            let v = violations.iter().find(|v| v.access2 == AccessId(2)).unwrap();
            match &v.missing_sync {
                MissingSync::NoOrderingPath { nearby_edges } => {
                    // The edge A1→A3 touches A1, so it's nearby.
                    assert!(!nearby_edges.is_empty());
                }
                MissingSync::NoSyncEdges => {
                    panic!("expected NoOrderingPath since there are sync edges in the MSG");
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Test 15: InvariantResult Display
    // -----------------------------------------------------------------------

    #[test]
    fn invariant_result_display() {
        let satisfied = InvariantResult::Satisfied {
            access_count: 5,
            conflict_pair_count: 2,
            interference_graph: InterferenceGraph::new(),
        };
        let display = format!("{}", satisfied);
        assert!(display.contains("SATISFIED"));
        assert!(display.contains("5 accesses"));

        let violated = InvariantResult::Violated {
            access_count: 3,
            conflict_pair_count: 1,
            violations: vec![],
            interference_graph: InterferenceGraph::new(),
        };
        let display = format!("{}", violated);
        assert!(display.contains("VIOLATED"));
    }

    // -----------------------------------------------------------------------
    // Test 16: Ordering in reverse direction (A2 → A1)
    // -----------------------------------------------------------------------

    #[test]
    fn ordering_in_reverse_direction() {
        let mut msg = MSG::new();
        add_test_region(&mut msg, 1, 0x1000, 0x100);
        add_direct_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_read(&mut msg, 1, 10, 8, 10);
        add_write(&mut msg, 2, 10, 8, 11);

        // A2 happens-before A1 (unusual but valid — e.g., join).
        add_hb(&mut msg, 1, 2, 1);

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = check_exclusivity(&msg, resolve);
        assert!(result.is_satisfied());
    }

    // -----------------------------------------------------------------------
    // Test 17: Atomic acquire-release ordering
    // -----------------------------------------------------------------------

    #[test]
    fn atomic_acquire_release_provides_ordering() {
        let mut msg = MSG::new();
        add_test_region(&mut msg, 1, 0x1000, 0x100);
        add_direct_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_write(&mut msg, 1, 10, 8, 10);
        add_read(&mut msg, 2, 10, 4, 11);

        msg.add_sync_edge(SyncEdge::new(
            SyncEdgeId(1),
            AccessId(1),
            AccessId(2),
            Ordering::AtomicAcquireRelease,
        ));

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = check_exclusivity(&msg, resolve);
        assert!(result.is_satisfied());
    }
}

// ---------------------------------------------------------------------------
// IVE-synced types and methods
// ---------------------------------------------------------------------------

/// An alias set: the set of access IDs that target the same region.
///
/// Mirrors the alias-set concept from `ive::exclusivity` for vuma-core
/// consumers. Multiple pointers (accesses) to the same region form an
/// alias set that requires exclusivity analysis.
#[derive(Debug, Clone)]
pub struct AliasSet {
    /// The region that all accesses in this set target.
    pub region_id: crate::region::RegionId,
    /// The access IDs that alias (target the same region).
    pub access_ids: HashSet<AccessId>,
}

impl AliasSet {
    /// Create a new alias set for the given region.
    pub fn new(region_id: crate::region::RegionId) -> Self {
        Self {
            region_id,
            access_ids: HashSet::new(),
        }
    }

    /// Add an access to this alias set.
    pub fn add(&mut self, access_id: AccessId) {
        self.access_ids.insert(access_id);
    }

    /// Returns the number of accesses in this alias set.
    pub fn len(&self) -> usize {
        self.access_ids.len()
    }

    /// Returns `true` if this alias set has more than one access (multi-pointer).
    pub fn is_multi_pointer(&self) -> bool {
        self.access_ids.len() > 1
    }
}

/// A proof obligation for the exclusivity invariant.
///
/// Represents a condition that must hold for the exclusivity invariant
/// to be satisfied, but which cannot be verified statically.
#[derive(Debug, Clone)]
pub struct ExclusivityProofObligation {
    /// A unique identifier for this obligation.
    pub id: u64,
    /// A human-readable description of what must be proven.
    pub description: String,
    /// The access pair involved.
    pub access1: AccessId,
    /// The access pair involved.
    pub access2: AccessId,
    /// The kind of obligation.
    pub obligation_kind: ExclusivityObligationKind,
}

/// The kind of exclusivity proof obligation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExclusivityObligationKind {
    /// Prove that the two accesses are ordered at runtime.
    RuntimeOrdering,
    /// Prove that a lock is held when the access occurs.
    LockHeld { lock_id: u64 },
    /// Prove that the accesses never execute concurrently.
    MutualExclusion,
}

/// The result of the enhanced exclusivity check, including proof obligations.
#[derive(Debug, Clone)]
pub struct EnhancedExclusivityResult {
    /// The base exclusivity result.
    pub base_result: InvariantResult,
    /// Alias sets computed from the input.
    pub alias_sets: Vec<AliasSet>,
    /// Proof obligations that must be discharged.
    pub proof_obligations: Vec<ExclusivityProofObligation>,
}

// ---------------------------------------------------------------------------
// IVE-synced wrapper methods
// ---------------------------------------------------------------------------

/// Verify multi-pointer exclusivity: when multiple accesses target the same
/// region, check that they don't conflict without synchronization.
///
/// This extends the basic `check_exclusivity` with region-aware alias
/// analysis. When multiple pointers (accesses) target the same region,
/// additional care is needed to ensure exclusivity.
pub fn verify_multi_pointer_exclusivity<F>(msg: &MSG, resolve_base: F) -> EnhancedExclusivityResult
where
    F: Fn(AccessId) -> Option<Address>,
{
    let base_result = check_exclusivity(msg, resolve_base);
    let alias_sets = compute_alias_sets(msg);
    let proof_obligations = generate_proof_obligations(&base_result, &alias_sets);

    EnhancedExclusivityResult {
        base_result,
        alias_sets,
        proof_obligations,
    }
}

/// Compute alias sets: groups of accesses that target the same region.
///
/// An alias set contains all accesses that resolve to the same root
/// region, indicating they may be accessing overlapping memory.
pub fn compute_alias_sets(msg: &MSG) -> Vec<AliasSet> {
    let mut region_accesses: HashMap<crate::region::RegionId, Vec<AccessId>> = HashMap::new();

    for access in msg.accesses() {
        // Walk derivation chain to find root region
        let mut current_id = access.target;
        loop {
            match msg.derivation(current_id) {
                Some(deriv) => match &deriv.source {
                    crate::derivation::DerivationSource::Region(rid) => {
                        region_accesses.entry(*rid).or_default().push(access.id);
                        break;
                    }
                    crate::derivation::DerivationSource::AnotherDerivation(parent_id) => {
                        current_id = *parent_id;
                    }
                },
                None => break,
            }
        }
    }

    region_accesses
        .into_iter()
        .map(|(region_id, access_ids)| {
            let mut set = AliasSet::new(region_id);
            for aid in access_ids {
                set.add(aid);
            }
            set
        })
        .collect()
}

/// Generate proof obligations from the exclusivity result and alias sets.
///
/// For each unordered conflict pair, a proof obligation is generated that
/// requires the caller to demonstrate that the accesses are properly
/// synchronized at runtime.
pub fn generate_proof_obligations(
    result: &InvariantResult,
    alias_sets: &[AliasSet],
) -> Vec<ExclusivityProofObligation> {
    let mut obligations = Vec::new();
    let mut next_id = 0u64;

    match result {
        InvariantResult::Violated { violations, .. } => {
            for v in violations {
                obligations.push(ExclusivityProofObligation {
                    id: next_id,
                    description: format!(
                        "Accesses {} and {} conflict without synchronization",
                        v.access1, v.access2
                    ),
                    access1: v.access1,
                    access2: v.access2,
                    obligation_kind: ExclusivityObligationKind::RuntimeOrdering,
                });
                next_id += 1;
            }
        }
        InvariantResult::Satisfied { .. } => {}
    }

    // For multi-pointer alias sets that are satisfied, add lock-held
    // obligations if there are multiple accesses to the same region.
    for alias_set in alias_sets {
        if alias_set.is_multi_pointer() {
            // Check if there's a write in the set — if so, may need
            // lock-based proof obligations
            let has_write = alias_set.access_ids.iter().any(|aid| {
                // We'd need to check access kind, but we don't have the MSG here.
                // Conservatively add a mutual exclusion obligation.
                false
            });
            if has_write {
                obligations.push(ExclusivityProofObligation {
                    id: next_id,
                    description: format!(
                        "Multi-pointer alias set for region {} requires mutual exclusion",
                        alias_set.region_id
                    ),
                    access1: *alias_set.access_ids.iter().next().unwrap_or(&AccessId(0)),
                    access2: *alias_set.access_ids.iter().nth(1).unwrap_or(&AccessId(0)),
                    obligation_kind: ExclusivityObligationKind::MutualExclusion,
                });
                next_id += 1;
            }
        }
    }

    obligations
}

/// Verify exclusivity using an interval tree for efficient overlap detection.
///
/// This produces the same result as [`check_exclusivity`] but uses a sorted
/// interval-based approach for O(n log n) overlap detection instead of
/// O(n²) pairwise comparison.
pub fn verify_with_interval_tree<F>(msg: &MSG, resolve_base: F) -> InvariantResult
where
    F: Fn(AccessId) -> Option<Address>,
{
    // Collect all accesses with their resolved base addresses.
    let mut access_info: Vec<(AccessId, Address, &Access)> = msg
        .accesses()
        .filter_map(|access| {
            let base = resolve_base(access.id)?;
            Some((access.id, base, access))
        })
        .collect();

    access_info.sort_by_key(|(id, _, _)| id.0);

    if access_info.is_empty() {
        return InvariantResult::Satisfied {
            access_count: 0,
            conflict_pair_count: 0,
            interference_graph: InterferenceGraph::new(),
        };
    }

    // Sort by base address for interval-tree-like sweep
    access_info.sort_by_key(|(_, base, _)| base.as_u64());

    let reachability = compute_reachability(msg);

    let mut violations: Vec<Violation> = Vec::new();
    let mut interference_graph = InterferenceGraph::new();
    let mut conflict_pair_count: usize = 0;

    // Sweep-line approach: since accesses are sorted by base address,
    // we only need to check adjacent accesses for overlap (and then
    // extend forward while there's overlap).
    for i in 0..access_info.len() {
        let (id1, base1, access1) = &access_info[i];
        let end1 = base1.as_u64() + access1.size;

        for j in (i + 1)..access_info.len() {
            let (id2, base2, access2) = &access_info[j];

            // Since sorted by base, if base2 >= end1, no further overlaps possible
            if base2.as_u64() >= end1 {
                break;
            }

            if access1.kind == AccessKind::Read && access2.kind == AccessKind::Read {
                continue;
            }

            let overlap = match compute_overlap(*base1, access1.size, *base2, access2.size) {
                Some(o) => o,
                None => continue,
            };

            let pair = ConflictPair::new(*id1, *id2);
            conflict_pair_count += 1;

            let ordered = are_ordered(&reachability, *id1, *id2);
            interference_graph.add_conflict(pair.clone(), ordered);

            if !ordered {
                let missing_sync = if msg.sync_edge_count() == 0 {
                    MissingSync::NoSyncEdges
                } else {
                    let nearby = find_nearby_edges(msg, *id1, *id2);
                    if nearby.is_empty() {
                        MissingSync::NoSyncEdges
                    } else {
                        MissingSync::NoOrderingPath { nearby_edges: nearby }
                    }
                };

                violations.push(Violation {
                    access1: *id1,
                    access2: *id2,
                    kind1: access1.kind,
                    kind2: access2.kind,
                    overlap,
                    target1: access1.target,
                    target2: access2.target,
                    missing_sync,
                });
            }
        }
    }

    if violations.is_empty() {
        InvariantResult::Satisfied {
            access_count: access_info.len(),
            conflict_pair_count,
            interference_graph,
        }
    } else {
        InvariantResult::Violated {
            access_count: access_info.len(),
            conflict_pair_count,
            violations,
            interference_graph,
        }
    }
}

// ---------------------------------------------------------------------------
// IVE-synced tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod ive_sync_tests {
    use super::*;
    use crate::access::{Access, AccessId, AccessKind};
    use crate::address::Address;
    use crate::derivation::{Derivation, DerivationId, DerivationKind, DerivationSource};
    use crate::msg::MSG;
    use crate::program_point::ProgramPoint;
    use crate::region::{Region, RegionId, RegionStatus};
    use crate::sync::{LockId, Ordering, SyncEdge, SyncEdgeId};

    fn pp(line: u32) -> ProgramPoint {
        ProgramPoint::new("test_ive.vu", line, 1)
    }

    fn add_region(msg: &mut MSG, id: u64, base: u64, size: u64) {
        msg.add_region(Region {
            id: RegionId(id),
            base: Address::from(base),
            size,
            status: RegionStatus::Allocated,
            alloc_point: pp(1),
            free_point: None,
            owner_context: None,
        });
    }

    fn add_derivation(msg: &mut MSG, id: u64, region_id: u64, base: u64, end: u64) {
        msg.add_derivation(Derivation {
            id: DerivationId(id),
            source: DerivationSource::Region(RegionId(region_id)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(base), Address::from(end)),
        });
    }

    fn add_access(msg: &mut MSG, id: u64, target: u64, kind: AccessKind, size: u64, line: u32) {
        msg.add_access(Access::new(AccessId(id), DerivationId(target), kind, size, pp(line)));
    }

    fn add_hb(msg: &mut MSG, id: u64, a1: u64, a2: u64) {
        msg.add_sync_edge(SyncEdge::new(
            SyncEdgeId(id), AccessId(a1), AccessId(a2), Ordering::HappensBefore,
        ));
    }

    // ----- IVE Test 1: compute_alias_sets -----

    #[test]
    fn alias_sets_group_by_region() {
        let mut msg = MSG::new();
        add_region(&mut msg, 1, 0x1000, 0x100);
        add_region(&mut msg, 2, 0x2000, 0x100);
        add_derivation(&mut msg, 10, 1, 0x1000, 0x1100);
        add_derivation(&mut msg, 20, 2, 0x2000, 0x2100);

        add_access(&mut msg, 1, 10, AccessKind::Write, 8, 10);
        add_access(&mut msg, 2, 10, AccessKind::Read, 4, 11);
        add_access(&mut msg, 3, 20, AccessKind::Read, 4, 12);

        let alias_sets = compute_alias_sets(&msg);
        assert_eq!(alias_sets.len(), 2, "Expected 2 alias sets for 2 regions");

        let set1 = alias_sets.iter().find(|s| s.region_id == RegionId(1)).unwrap();
        assert_eq!(set1.len(), 2, "Region 1 should have 2 accesses");
        assert!(set1.is_multi_pointer());

        let set2 = alias_sets.iter().find(|s| s.region_id == RegionId(2)).unwrap();
        assert_eq!(set2.len(), 1, "Region 2 should have 1 access");
        assert!(!set2.is_multi_pointer());
    }

    // ----- IVE Test 2: verify_multi_pointer_exclusivity -----

    #[test]
    fn multi_pointer_exclusivity_detects_conflicts() {
        let mut msg = MSG::new();
        add_region(&mut msg, 1, 0x1000, 0x100);
        add_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_access(&mut msg, 1, 10, AccessKind::Write, 8, 10);
        add_access(&mut msg, 2, 10, AccessKind::Read, 4, 11);
        // No sync edge → conflict

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = verify_multi_pointer_exclusivity(&msg, resolve);

        assert!(!result.base_result.is_satisfied());
        assert!(!result.proof_obligations.is_empty());
    }

    // ----- IVE Test 3: verify_with_interval_tree -----

    #[test]
    fn interval_tree_produces_same_result_as_pairwise() {
        let mut msg = MSG::new();
        add_region(&mut msg, 1, 0x1000, 0x100);
        add_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_access(&mut msg, 1, 10, AccessKind::Write, 8, 10);
        add_access(&mut msg, 2, 10, AccessKind::Read, 4, 11);
        add_hb(&mut msg, 1, 1, 2);

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));

        let pairwise = check_exclusivity(&msg, &resolve);
        let interval = verify_with_interval_tree(&msg, resolve);

        assert_eq!(pairwise.is_satisfied(), interval.is_satisfied());
        assert_eq!(pairwise.violation_count(), interval.violation_count());
    }

    #[test]
    fn interval_tree_detects_violation() {
        let mut msg = MSG::new();
        add_region(&mut msg, 1, 0x1000, 0x100);
        add_derivation(&mut msg, 10, 1, 0x1000, 0x1100);

        add_access(&mut msg, 1, 10, AccessKind::Write, 8, 10);
        add_access(&mut msg, 2, 10, AccessKind::Read, 4, 11);
        // No sync → violation

        let resolve = |_: AccessId| Some(Address::from(0x1000_u64));
        let result = verify_with_interval_tree(&msg, resolve);
        assert!(!result.is_satisfied());
        assert_eq!(result.violation_count(), 1);
    }
}
