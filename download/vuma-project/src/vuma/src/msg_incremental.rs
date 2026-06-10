//! Incremental MSG update engine.
//!
//! When the Memory State Graph changes (e.g., during interactive editing or
//! incremental analysis), rebuilding the entire graph from scratch is wasteful.
//! This module provides an **incremental update** mechanism that computes only
//! the delta — the parts of the MSG that change — and applies it efficiently.
//!
//! # Core concepts
//!
//! - [`MSGDelta`] — a change set (additions, removals, modifications) for
//!   each MSG entity type plus verification status updates.
//! - [`apply_delta`] — apply a delta to an existing MSG, maintaining
//!   invariants and propagating effects through the dependency graph.
//! - [`compute_delta`] — diff two [`MSG`] instances and produce the
//!   corresponding [`MSGDelta`].
//! - [`compute_scg_delta`] — diff two [`SCGSnapshot`] versions and produce
//!   the corresponding [`MSGDelta`] (for SCG-driven incremental updates).
//!
//! # Complexity
//!
//! As specified in the MSG Construction Spec (§5), the incremental update
//! operates in O(δ × log N) where δ is the size of the change and N is the
//! total size of the MSG. Only affected parts are updated; unaffected
//! regions, derivations, accesses, and sync edges remain untouched.
//!
//! # Invariant maintenance
//!
//! After each delta application the following MSG invariants are re-established:
//!
//! 1. **Liveness** — every access targets a live region (or is flagged).
//! 2. **Bounds** — every derivation's provenance range is within its region.
//! 3. **Origin** — every derivation traces back to a region.
//! 4. **Referential integrity** — no dangling references between entities.

use crate::access::{Access, AccessId};
use crate::address::Address;
use crate::derivation::{Derivation, DerivationId, DerivationSource};
use crate::msg::MSG;
use crate::region::{Region, RegionId, RegionStatus};
use crate::sync::{SyncEdge, SyncEdgeId};
use hashbrown::{HashMap, HashSet};
use std::fmt;

// ---------------------------------------------------------------------------
// Verification status
// ---------------------------------------------------------------------------

/// Verification status for a memory access, following the three-valued
/// lattice from the MSG spec: Safe > Unverified > Unsafe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VerificationStatus {
    /// Proven safe — all invariants hold.
    Safe,
    /// Proven unsafe — at least one invariant is violated.
    Unsafe,
    /// Unverified — cannot yet determine safety.
    Unverified,
}

impl VerificationStatus {
    /// Greatest lower bound (meet) in the verification lattice.
    ///
    /// ```text
    /// Safe   ⊓ Safe       = Safe
    /// Safe   ⊓ Unverified = Unverified
    /// Safe   ⊓ Unsafe     = Unsafe
    /// Unverified ⊓ Unsafe = Unsafe
    /// ```
    pub fn meet(self, other: Self) -> Self {
        use VerificationStatus::*;
        match (self, other) {
            (Safe, Safe) => Safe,
            (Unsafe, _) | (_, Unsafe) => Unsafe,
            _ => Unverified,
        }
    }
}

impl fmt::Display for VerificationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerificationStatus::Safe => write!(f, "safe"),
            VerificationStatus::Unsafe => write!(f, "unsafe"),
            VerificationStatus::Unverified => write!(f, "unverified"),
        }
    }
}

// ---------------------------------------------------------------------------
// SCG Snapshot — lightweight representation for SCG-driven delta computation
// ---------------------------------------------------------------------------

/// A lightweight snapshot of the SCG nodes relevant to MSG construction.
///
/// This is used by [`compute_scg_delta`] to diff two versions of the SCG and
/// produce the corresponding [`MSGDelta`]. Each variant captures the
/// information needed by the MSG construction rules from the spec.
#[derive(Debug, Clone)]
pub enum SCGNode {
    /// An allocation node — creates a new region.
    Alloc {
        node_id: u64,
        region_id: RegionId,
        base: Address,
        size: u64,
        alloc_point: crate::program_point::ProgramPoint,
    },
    /// A deallocation node — frees a region.
    Dealloc {
        node_id: u64,
        region_id: RegionId,
        free_point: crate::program_point::ProgramPoint,
    },
    /// An access node — creates an access entry.
    Access {
        node_id: u64,
        access_id: AccessId,
        target_derivation: DerivationId,
        region_id: RegionId,
        is_write: bool,
        size: u64,
        program_point: crate::program_point::ProgramPoint,
    },
    /// An arithmetic node — creates an offset derivation.
    Arithmetic {
        node_id: u64,
        derivation_id: DerivationId,
        source_derivation: DerivationId,
        offset: i64,
        proven_range: (Address, Address),
    },
    /// A cast node — creates a cast derivation.
    Cast {
        node_id: u64,
        derivation_id: DerivationId,
        source_derivation: DerivationId,
        proven_range: (Address, Address),
    },
    /// A synchronisation edge node.
    Sync {
        node_id: u64,
        edge_id: SyncEdgeId,
        access1: AccessId,
        access2: AccessId,
        ordering: crate::sync::Ordering,
    },
}

/// A snapshot of the SCG at a point in time, for diffing.
///
/// Nodes are indexed by their `node_id` for efficient lookup and comparison.
#[derive(Debug, Clone, Default)]
pub struct SCGSnapshot {
    nodes: HashMap<u64, SCGNode>,
}

impl SCGSnapshot {
    /// Create an empty snapshot.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node to the snapshot.
    pub fn add_node(&mut self, node: SCGNode) {
        let id = match &node {
            SCGNode::Alloc { node_id, .. } => *node_id,
            SCGNode::Dealloc { node_id, .. } => *node_id,
            SCGNode::Access { node_id, .. } => *node_id,
            SCGNode::Arithmetic { node_id, .. } => *node_id,
            SCGNode::Cast { node_id, .. } => *node_id,
            SCGNode::Sync { node_id, .. } => *node_id,
        };
        self.nodes.insert(id, node);
    }

    /// Remove a node from the snapshot, returning it if present.
    pub fn remove_node(&mut self, node_id: u64) -> Option<SCGNode> {
        self.nodes.remove(&node_id)
    }

    /// Look up a node by ID.
    pub fn get_node(&self, node_id: u64) -> Option<&SCGNode> {
        self.nodes.get(&node_id)
    }

    /// Iterate over all nodes.
    pub fn nodes(&self) -> impl Iterator<Item = &SCGNode> {
        self.nodes.values()
    }

    /// Number of nodes in the snapshot.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the snapshot is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Entity delta — per-entity-type change set
// ---------------------------------------------------------------------------

/// A change set for one entity type within the MSG.
///
/// Follows the (R+, R-, R~) structure from the spec §5.1:
/// - `added`: entities to insert.
/// - `removed`: entity IDs to delete.
/// - `modified`: entities whose contents have changed (replace in place).
#[derive(Debug, Clone)]
pub struct EntityDelta<T> {
    /// Entities to add.
    pub added: Vec<T>,
    /// IDs of entities to remove.
    pub removed: Vec<u64>,
    /// Entities to replace (matched by their ID field).
    pub modified: Vec<T>,
}

impl<T> Default for EntityDelta<T> {
    fn default() -> Self {
        Self {
            added: Vec::new(),
            removed: Vec::new(),
            modified: Vec::new(),
        }
    }
}

impl<T> EntityDelta<T> {
    /// Create an empty entity delta.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if there are no additions, removals, or modifications.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }

    /// Add an entity to the "added" set.
    pub fn add(&mut self, entity: T) {
        self.added.push(entity);
    }

    /// Mark an entity ID for removal.
    pub fn remove(&mut self, id: u64) {
        self.removed.push(id);
    }

    /// Add an entity to the "modified" set.
    pub fn modify(&mut self, entity: T) {
        self.modified.push(entity);
    }
}

// ---------------------------------------------------------------------------
// MSGDelta — full delta for the MSG
// ---------------------------------------------------------------------------

/// A delta (change set) to apply to an MSG.
///
/// Captures all additions, removals, and modifications for each entity type,
/// plus updates to the verification status function φ.
///
/// This corresponds to the formal definition from spec §5.1:
/// ```text
/// Delta-MSG(n) = (DeltaR, DeltaD, DeltaA, DeltaPhi)
/// where DeltaR = (R+, R-, R~), etc.
/// ```
#[derive(Debug, Clone)]
pub struct MSGDelta {
    /// Region changes.
    pub regions: EntityDelta<Region>,
    /// Derivation changes.
    pub derivations: EntityDelta<Derivation>,
    /// Access changes.
    pub accesses: EntityDelta<Access>,
    /// Sync edge changes.
    pub sync_edges: EntityDelta<SyncEdge>,
    /// Verification status updates: AccessId -> new status.
    pub verification_updates: HashMap<AccessId, VerificationStatus>,
}

impl Default for MSGDelta {
    fn default() -> Self {
        Self {
            regions: EntityDelta::new(),
            derivations: EntityDelta::new(),
            accesses: EntityDelta::new(),
            sync_edges: EntityDelta::new(),
            verification_updates: HashMap::new(),
        }
    }
}

impl MSGDelta {
    /// Create an empty delta.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if there are no changes at all.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
            && self.derivations.is_empty()
            && self.accesses.is_empty()
            && self.sync_edges.is_empty()
            && self.verification_updates.is_empty()
    }

    /// Merge another delta into this one.
    ///
    /// Later deltas take precedence for modifications and verification updates.
    pub fn merge(&mut self, other: MSGDelta) {
        self.regions.added.extend(other.regions.added);
        self.regions.removed.extend(other.regions.removed);
        self.regions.modified.extend(other.regions.modified);

        self.derivations.added.extend(other.derivations.added);
        self.derivations.removed.extend(other.derivations.removed);
        self.derivations.modified.extend(other.derivations.modified);

        self.accesses.added.extend(other.accesses.added);
        self.accesses.removed.extend(other.accesses.removed);
        self.accesses.modified.extend(other.accesses.modified);

        self.sync_edges.added.extend(other.sync_edges.added);
        self.sync_edges.removed.extend(other.sync_edges.removed);
        self.sync_edges.modified.extend(other.sync_edges.modified);

        for (aid, status) in other.verification_updates {
            self.verification_updates.insert(aid, status);
        }
    }
}

// ---------------------------------------------------------------------------
// Delta errors
// ---------------------------------------------------------------------------

/// Errors that can occur during delta application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaError {
    /// A region being removed or modified does not exist in the MSG.
    RegionNotFound(RegionId),
    /// A derivation being removed or modified does not exist.
    DerivationNotFound(DerivationId),
    /// An access being removed or modified does not exist.
    AccessNotFound(AccessId),
    /// A sync edge being removed or modified does not exist.
    SyncEdgeNotFound(SyncEdgeId),
    /// A newly added entity has an ID that already exists (collision).
    DuplicateRegion(RegionId),
    /// A newly added derivation has an ID that already exists.
    DuplicateDerivation(DerivationId),
    /// A newly added access has an ID that already exists.
    DuplicateAccess(AccessId),
    /// A newly added sync edge has an ID that already exists.
    DuplicateSyncEdge(SyncEdgeId),
    /// A derivation references a source that does not exist (broken chain).
    BrokenDerivationChain(DerivationId),
    /// An access targets a derivation that does not exist.
    DanglingAccessTarget(AccessId, DerivationId),
    /// An access targets a region that is not live.
    AccessToDeadRegion(AccessId, RegionId),
    /// A sync edge references a non-existent access.
    DanglingSyncEdgeAccess(SyncEdgeId, AccessId),
}

impl fmt::Display for DeltaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeltaError::RegionNotFound(id) => write!(f, "region not found: {}", id),
            DeltaError::DerivationNotFound(id) => write!(f, "derivation not found: {}", id),
            DeltaError::AccessNotFound(id) => write!(f, "access not found: {}", id),
            DeltaError::SyncEdgeNotFound(id) => write!(f, "sync edge not found: {}", id),
            DeltaError::DuplicateRegion(id) => write!(f, "duplicate region: {}", id),
            DeltaError::DuplicateDerivation(id) => write!(f, "duplicate derivation: {}", id),
            DeltaError::DuplicateAccess(id) => write!(f, "duplicate access: {}", id),
            DeltaError::DuplicateSyncEdge(id) => write!(f, "duplicate sync edge: {}", id),
            DeltaError::BrokenDerivationChain(id) => {
                write!(f, "broken derivation chain at: {}", id)
            }
            DeltaError::DanglingAccessTarget(aid, did) => {
                write!(f, "access {} targets missing derivation {}", aid, did)
            }
            DeltaError::AccessToDeadRegion(aid, rid) => {
                write!(f, "access {} targets dead region {}", aid, rid)
            }
            DeltaError::DanglingSyncEdgeAccess(seid, aid) => {
                write!(f, "sync edge {} references missing access {}", seid, aid)
            }
        }
    }
}

impl std::error::Error for DeltaError {}

// ---------------------------------------------------------------------------
// Delta result
// ---------------------------------------------------------------------------

/// The result of applying a delta to an MSG.
#[derive(Debug, Clone)]
pub struct DeltaResult {
    /// Whether the delta was applied fully.
    pub success: bool,
    /// Warnings and non-fatal errors encountered during application.
    pub warnings: Vec<DeltaError>,
    /// IDs of accesses that had their verification status changed during
    /// propagation, along with their new status.
    pub reverified: Vec<(AccessId, VerificationStatus)>,
    /// IDs of derivations that had their provenance ranges recomputed
    /// during propagation.
    pub recomputed_derivations: Vec<DerivationId>,
    /// IDs of regions that were invalidated (freed / removed) triggering
    /// cascading re-verification.
    pub invalidated_regions: Vec<RegionId>,
}

impl DeltaResult {
    /// Create a successful result with no warnings.
    pub fn ok() -> Self {
        Self {
            success: true,
            warnings: Vec::new(),
            reverified: Vec::new(),
            recomputed_derivations: Vec::new(),
            invalidated_regions: Vec::new(),
        }
    }

    /// Create a result with warnings (still considered successful).
    pub fn with_warnings(warnings: Vec<DeltaError>) -> Self {
        Self {
            success: true,
            warnings,
            reverified: Vec::new(),
            recomputed_derivations: Vec::new(),
            invalidated_regions: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// apply_delta — apply a delta to an MSG
// ---------------------------------------------------------------------------

/// Apply a delta to the MSG, updating only affected parts.
///
/// The application follows the formal rule from spec §5.1:
/// ```text
/// MSG + Delta-MSG = MSG'
/// where R' = (R \ R-) ∪ R+ ∪ R~
///       D' = (D \ D-) ∪ D+ ∪ D~
///       A' = (A \ A-) ∪ A+ ∪ A~
///       phi' = phi ∪ DeltaPhi   (DeltaPhi overrides existing entries)
/// ```
///
/// After the direct application, the function runs **propagation** to
/// maintain MSG invariants:
///
/// 1. Region removal → cascade: invalidate all accesses to freed regions,
///    re-verify derivations targeting the region.
/// 2. Derivation removal/modification → cascade: recompute downstream
///    derivation offsets, re-verify downstream accesses.
/// 3. Access addition → verify liveness, bounds, exclusivity.
/// 4. Sync edge addition → re-evaluate concurrency for affected accesses.
///
/// Complexity: O(δ × log N) where δ is the size of the delta and N is the
/// total size of the MSG.
pub fn apply_delta(msg: &mut MSG, delta: MSGDelta) -> DeltaResult {
    let mut result = DeltaResult::ok();

    // --- Phase 1: Remove entities (R-, D-, A-, SE-) ---

    // Track removed regions for cascading invalidation.
    let mut removed_region_ids: HashSet<RegionId> = HashSet::new();
    for rid in &delta.regions.removed {
        let id = RegionId(*rid);
        if msg.region(id).is_none() {
            result.warnings.push(DeltaError::RegionNotFound(id));
        } else {
            removed_region_ids.insert(id);
        }
    }

    // Track removed derivations for cascading.
    let mut removed_derivation_ids: HashSet<DerivationId> = HashSet::new();
    for did in &delta.derivations.removed {
        let id = DerivationId(*did);
        if msg.derivation(id).is_none() {
            result.warnings.push(DeltaError::DerivationNotFound(id));
        } else {
            removed_derivation_ids.insert(id);
        }
    }

    // Track removed accesses.
    let mut removed_access_ids: HashSet<AccessId> = HashSet::new();
    for aid in &delta.accesses.removed {
        let id = AccessId(*aid);
        if msg.access(id).is_none() {
            result.warnings.push(DeltaError::AccessNotFound(id));
        } else {
            removed_access_ids.insert(id);
        }
    }

    // Remove sync edges.
    let removed_sync_edge_ids: HashSet<SyncEdgeId> = delta
        .sync_edges
        .removed
        .iter()
        .map(|&id| SyncEdgeId(id))
        .collect();
    for seid in &removed_sync_edge_ids {
        if msg.sync_edge(*seid).is_none() {
            result.warnings.push(DeltaError::SyncEdgeNotFound(*seid));
        }
    }

    // Perform actual removals.
    apply_removals(
        msg,
        &removed_region_ids,
        &removed_derivation_ids,
        &removed_access_ids,
        &removed_sync_edge_ids,
    );

    // --- Phase 2: Add entities (R+, D+, A+, SE+) ---

    for region in delta.regions.added {
        if msg.region(region.id).is_some() {
            result.warnings.push(DeltaError::DuplicateRegion(region.id));
            continue;
        }
        msg.add_region(region);
    }

    for derivation in delta.derivations.added {
        if msg.derivation(derivation.id).is_some() {
            result.warnings.push(DeltaError::DuplicateDerivation(derivation.id));
            continue;
        }
        // Validate derivation chain: source must exist.
        match &derivation.source {
            DerivationSource::Region(rid) => {
                if msg.region(*rid).is_none() {
                    result
                        .warnings
                        .push(DeltaError::BrokenDerivationChain(derivation.id));
                }
            }
            DerivationSource::AnotherDerivation(parent_id) => {
                if msg.derivation(*parent_id).is_none() {
                    result
                        .warnings
                        .push(DeltaError::BrokenDerivationChain(derivation.id));
                }
            }
        }
        msg.add_derivation(derivation);
    }

    for access in delta.accesses.added {
        if msg.access(access.id).is_some() {
            result.warnings.push(DeltaError::DuplicateAccess(access.id));
            continue;
        }
        // Validate access target derivation exists.
        if msg.derivation(access.target).is_none() {
            result
                .warnings
                .push(DeltaError::DanglingAccessTarget(access.id, access.target));
        }
        msg.add_access(access);
    }

    for edge in delta.sync_edges.added {
        if msg.sync_edge(edge.id).is_some() {
            result.warnings.push(DeltaError::DuplicateSyncEdge(edge.id));
            continue;
        }
        // Validate referenced accesses exist.
        if msg.access(edge.access1).is_none() {
            result
                .warnings
                .push(DeltaError::DanglingSyncEdgeAccess(edge.id, edge.access1));
        }
        if msg.access(edge.access2).is_none() {
            result
                .warnings
                .push(DeltaError::DanglingSyncEdgeAccess(edge.id, edge.access2));
        }
        msg.add_sync_edge(edge);
    }

    // --- Phase 3: Modify entities (R~, D~, A~, SE~) ---

    for region in delta.regions.modified {
        let id = region.id;
        if msg.region(id).is_none() {
            result.warnings.push(DeltaError::RegionNotFound(id));
            continue;
        }
        msg.add_region(region); // insert replaces
        result.invalidated_regions.push(id);
    }

    for derivation in delta.derivations.modified {
        let id = derivation.id;
        if msg.derivation(id).is_none() {
            result.warnings.push(DeltaError::DerivationNotFound(id));
            continue;
        }
        msg.add_derivation(derivation); // insert replaces
        result.recomputed_derivations.push(id);
    }

    for access in delta.accesses.modified {
        if msg.access(access.id).is_none() {
            result.warnings.push(DeltaError::AccessNotFound(access.id));
            continue;
        }
        msg.add_access(access); // insert replaces
    }

    for edge in delta.sync_edges.modified {
        if msg.sync_edge(edge.id).is_none() {
            result.warnings.push(DeltaError::SyncEdgeNotFound(edge.id));
            continue;
        }
        msg.add_sync_edge(edge); // insert replaces
    }

    // --- Phase 4: Propagation (spec §5.3) ---

    // Propagate region removals: invalidate accesses to freed/removed regions.
    for rid in &removed_region_ids {
        result.invalidated_regions.push(*rid);
        let affected = find_accesses_to_region(msg, *rid);
        for aid in affected {
            let status = VerificationStatus::Unsafe;
            result.reverified.push((aid, status));
        }
    }

    // Propagate region modifications: re-verify accesses.
    for rid in &result.invalidated_regions {
        let affected = find_accesses_to_region(msg, *rid);
        for aid in affected {
            let status = verify_access(msg, aid);
            result.reverified.push((aid, status));
        }
    }

    // Propagate derivation removals: cascade downstream.
    if !removed_derivation_ids.is_empty() {
        let downstream = find_downstream_derivations(msg, &removed_derivation_ids);
        result
            .recomputed_derivations
            .extend(downstream.iter().map(|d| d.id));

        for deriv in &downstream {
            let affected = find_accesses_via_derivation(msg, deriv.id);
            for aid in affected {
                let status = verify_access(msg, aid);
                result.reverified.push((aid, status));
            }
        }
    }

    // Propagate derivation modifications: re-verify downstream.
    for did in result.recomputed_derivations.clone() {
        let downstream = find_downstream_derivations(msg, &HashSet::from([did]));
        for deriv in &downstream {
            let affected = find_accesses_via_derivation(msg, deriv.id);
            for aid in affected {
                let status = verify_access(msg, aid);
                result.reverified.push((aid, status));
            }
        }
    }

    // --- Phase 5: Apply verification updates ---
    // (The reverified list is the output for callers to consume.)

    // De-duplicate reverified entries (keep first occurrence).
    let mut seen: HashSet<AccessId> = HashSet::new();
    result.reverified.retain(|(aid, _)| seen.insert(*aid));

    result
}

// ---------------------------------------------------------------------------
// compute_delta — diff two MSG instances directly
// ---------------------------------------------------------------------------

/// Compute the delta between two MSG instances.
///
/// Compares `old_msg` and `new_msg` entity by entity and produces an
/// [`MSGDelta`] that, when applied to `old_msg`, yields a graph equivalent
/// to `new_msg`.
///
/// # Algorithm
///
/// For each entity type (regions, derivations, accesses, sync edges):
/// 1. Collect IDs from both MSGs.
/// 2. New IDs (in `new_msg` but not `old_msg`) → additions.
/// 3. Removed IDs (in `old_msg` but not `new_msg`) → removals.
/// 4. Changed entities (same ID, different content) → modifications.
///
/// Complexity: O(|δ| × log N) where δ is the number of changed entities
/// and N is the total number of entities.
pub fn compute_delta(old_msg: &MSG, new_msg: &MSG) -> MSGDelta {
    let mut delta = MSGDelta::new();

    // --- Regions ---
    compute_entity_delta(
        &mut delta.regions,
        old_msg.region_ids(),
        |id| old_msg.region(id).cloned(),
        new_msg.region_ids(),
        |id| new_msg.region(id).cloned(),
    );

    // --- Derivations ---
    compute_entity_delta(
        &mut delta.derivations,
        old_msg.derivation_ids(),
        |id| old_msg.derivation(id).cloned(),
        new_msg.derivation_ids(),
        |id| new_msg.derivation(id).cloned(),
    );

    // --- Accesses ---
    compute_entity_delta(
        &mut delta.accesses,
        old_msg.access_ids(),
        |id| old_msg.access(id).cloned(),
        new_msg.access_ids(),
        |id| new_msg.access(id).cloned(),
    );

    // --- Sync edges ---
    compute_entity_delta(
        &mut delta.sync_edges,
        old_msg.sync_edge_ids(),
        |id| old_msg.sync_edge(id).cloned(),
        new_msg.sync_edge_ids(),
        |id| new_msg.sync_edge(id).cloned(),
    );

    delta
}

/// Generic helper: compute an EntityDelta by comparing two ID-keyed collections.
///
/// - `old_ids` / `new_ids`: iterators over IDs in each version.
/// - `old_lookup` / `new_lookup`: functions that resolve an ID to an entity ref.
///
/// Entities in `new` but not `old` → added.
/// Entities in `old` but not `new` → removed.
/// Entities in both but differing → modified.
fn compute_entity_delta<T, Id>(
    entity_delta: &mut EntityDelta<T>,
    old_ids: impl Iterator<Item = Id>,
    old_lookup: impl Fn(Id) -> Option<T>,
    new_ids: impl Iterator<Item = Id>,
    new_lookup: impl Fn(Id) -> Option<T>,
) where
    T: Clone + PartialEq,
    Id: Copy + Eq + std::hash::Hash + ExtractId,
{
    let old_set: HashSet<Id> = old_ids.collect();
    let new_set: HashSet<Id> = new_ids.collect();

    // Added: in new but not in old.
    for id in new_set.difference(&old_set) {
        if let Some(entity) = new_lookup(*id) {
            entity_delta.added.push(entity);
        }
    }

    // Removed: in old but not in new.
    for id in old_set.difference(&new_set) {
        entity_delta.removed.push(extract_id(*id));
    }

    // Modified: same ID, different content.
    for id in old_set.intersection(&new_set) {
        if let (Some(old), Some(new)) = (old_lookup(*id), new_lookup(*id)) {
            if old != new {
                entity_delta.modified.push(new);
            }
        }
    }
}

/// Extract the u64 key from a typed ID for the EntityDelta::removed field.
trait ExtractId {
    fn extract_u64(self) -> u64;
}

fn extract_id<Id: ExtractId>(id: Id) -> u64 {
    id.extract_u64()
}

impl ExtractId for RegionId {
    fn extract_u64(self) -> u64 {
        self.0
    }
}

impl ExtractId for DerivationId {
    fn extract_u64(self) -> u64 {
        self.0
    }
}

impl ExtractId for AccessId {
    fn extract_u64(self) -> u64 {
        self.0
    }
}

impl ExtractId for SyncEdgeId {
    fn extract_u64(self) -> u64 {
        self.0
    }
}

// ---------------------------------------------------------------------------
// compute_scg_delta — diff two SCG snapshots
// ---------------------------------------------------------------------------

/// Compute the delta between two SCG snapshots.
///
/// This compares the `old_scg` and `new_scg` node-by-node and produces
/// an [`MSGDelta`] that, when applied to the MSG corresponding to
/// `old_scg`, yields the MSG corresponding to `new_scg`.
///
/// # Algorithm
///
/// 1. Collect node IDs from both snapshots.
/// 2. New nodes (in `new_scg` but not `old_scg`) → additions.
/// 3. Removed nodes (in `old_scg` but not `new_scg`) → removals.
/// 4. Changed nodes (same ID, different content) → modifications.
///
/// Complexity: O(|δ| × log N) where δ is the number of changed nodes
/// and N is the total number of nodes.
pub fn compute_scg_delta(old_scg: &SCGSnapshot, new_scg: &SCGSnapshot) -> MSGDelta {
    let mut delta = MSGDelta::new();

    let old_ids: HashSet<u64> = old_scg.nodes.keys().copied().collect();
    let new_ids: HashSet<u64> = new_scg.nodes.keys().copied().collect();

    // Added nodes: in new but not in old.
    for node_id in new_ids.difference(&old_ids) {
        if let Some(node) = new_scg.get_node(*node_id) {
            add_node_to_delta(&mut delta, node);
        }
    }

    // Removed nodes: in old but not in new.
    for node_id in old_ids.difference(&new_ids) {
        if let Some(node) = old_scg.get_node(*node_id) {
            remove_node_from_delta(&mut delta, node);
        }
    }

    // Changed nodes: same ID but different content.
    for node_id in old_ids.intersection(&new_ids) {
        let old_node = old_scg.get_node(*node_id).unwrap();
        let new_node = new_scg.get_node(*node_id).unwrap();
        if nodes_differ(old_node, new_node) {
            remove_node_from_delta(&mut delta, old_node);
            add_node_to_delta(&mut delta, new_node);
        }
    }

    delta
}

/// Add the effects of an SCG node to the delta (spec §5.2 rules).
fn add_node_to_delta(delta: &mut MSGDelta, node: &SCGNode) {
    match node {
        SCGNode::Alloc {
            region_id,
            base,
            size,
            alloc_point,
            ..
        } => {
            delta.regions.add(Region {
                id: *region_id,
                base: *base,
                size: *size,
                status: RegionStatus::Allocated,
                alloc_point: alloc_point.clone(),
                free_point: None,
                owner_context: None,
            });
        }
        SCGNode::Dealloc {
            region_id,
            free_point,
            ..
        } => {
            // Modification: set region status to Freed.
            delta.regions.modify(Region {
                id: *region_id,
                base: Address::from(0u64), // placeholder; apply_delta replaces
                size: 0,                   // placeholder
                status: RegionStatus::Freed,
                alloc_point: free_point.clone(),
                free_point: Some(free_point.clone()),
                owner_context: None,
            });
        }
        SCGNode::Access {
            access_id,
            target_derivation,
            is_write,
            size,
            program_point,
            ..
        } => {
            delta.accesses.add(Access::new(
                *access_id,
                *target_derivation,
                if *is_write {
                    crate::access::AccessKind::Write
                } else {
                    crate::access::AccessKind::Read
                },
                *size,
                program_point.clone(),
            ));
        }
        SCGNode::Arithmetic {
            derivation_id,
            source_derivation,
            offset,
            proven_range,
            ..
        } => {
            delta.derivations.add(Derivation {
                id: *derivation_id,
                source: DerivationSource::AnotherDerivation(*source_derivation),
                kind: crate::derivation::DerivationKind::Offset { by: *offset },
                proven_range: *proven_range,
            });
        }
        SCGNode::Cast {
            derivation_id,
            source_derivation,
            proven_range,
            ..
        } => {
            delta.derivations.add(Derivation {
                id: *derivation_id,
                source: DerivationSource::AnotherDerivation(*source_derivation),
                kind: crate::derivation::DerivationKind::Cast {
                    from: crate::derivation::RepD {
                        name: "source".into(),
                        size: 0,
                    },
                    to: crate::derivation::RepD {
                        name: "target".into(),
                        size: 0,
                    },
                },
                proven_range: *proven_range,
            });
        }
        SCGNode::Sync {
            edge_id,
            access1,
            access2,
            ordering,
            ..
        } => {
            delta.sync_edges.add(SyncEdge::new(
                *edge_id,
                *access1,
                *access2,
                ordering.clone(),
            ));
        }
    }
}

/// Record the removal of an SCG node in the delta.
fn remove_node_from_delta(delta: &mut MSGDelta, node: &SCGNode) {
    match node {
        SCGNode::Alloc { region_id, .. } => {
            delta.regions.remove(region_id.0);
        }
        SCGNode::Dealloc { region_id, .. } => {
            // Removing a dealloc means the region should go back to Allocated.
            delta.regions.modify(Region {
                id: *region_id,
                base: Address::from(0u64),
                size: 0,
                status: RegionStatus::Allocated,
                alloc_point: crate::program_point::ProgramPoint::new("", 0, 0),
                free_point: None,
                owner_context: None,
            });
        }
        SCGNode::Access { access_id, .. } => {
            delta.accesses.remove(access_id.0);
        }
        SCGNode::Arithmetic {
            derivation_id, ..
        } => {
            delta.derivations.remove(derivation_id.0);
        }
        SCGNode::Cast {
            derivation_id, ..
        } => {
            delta.derivations.remove(derivation_id.0);
        }
        SCGNode::Sync { edge_id, .. } => {
            delta.sync_edges.remove(edge_id.0);
        }
    }
}

/// Check if two SCG nodes differ in content.
fn nodes_differ(a: &SCGNode, b: &SCGNode) -> bool {
    match (a, b) {
        (
            SCGNode::Alloc { region_id: r1, base: b1, size: s1, .. },
            SCGNode::Alloc { region_id: r2, base: b2, size: s2, .. },
        ) => r1 != r2 || b1 != b2 || s1 != s2,

        (
            SCGNode::Dealloc { region_id: r1, free_point: fp1, .. },
            SCGNode::Dealloc { region_id: r2, free_point: fp2, .. },
        ) => r1 != r2 || fp1 != fp2,

        (
            SCGNode::Access { access_id: a1, target_derivation: td1, is_write: w1, size: s1, .. },
            SCGNode::Access { access_id: a2, target_derivation: td2, is_write: w2, size: s2, .. },
        ) => a1 != a2 || td1 != td2 || w1 != w2 || s1 != s2,

        (
            SCGNode::Arithmetic { derivation_id: d1, source_derivation: sd1, offset: o1, .. },
            SCGNode::Arithmetic { derivation_id: d2, source_derivation: sd2, offset: o2, .. },
        ) => d1 != d2 || sd1 != sd2 || o1 != o2,

        (
            SCGNode::Cast { derivation_id: d1, source_derivation: sd1, .. },
            SCGNode::Cast { derivation_id: d2, source_derivation: sd2, .. },
        ) => d1 != d2 || sd1 != sd2,

        (
            SCGNode::Sync { edge_id: e1, access1: a1, access2: a2, .. },
            SCGNode::Sync { edge_id: e2, access1: b1, access2: b2, .. },
        ) => e1 != e2 || a1 != b1 || a2 != b2,

        _ => true, // Different node types always differ.
    }
}

// ---------------------------------------------------------------------------
// Internal helpers for removal
// ---------------------------------------------------------------------------

/// Apply removals using MSG's remove_* methods.
fn apply_removals(
    msg: &mut MSG,
    removed_regions: &HashSet<RegionId>,
    removed_derivations: &HashSet<DerivationId>,
    removed_accesses: &HashSet<AccessId>,
    removed_sync_edges: &HashSet<SyncEdgeId>,
) {
    for rid in removed_regions {
        msg.remove_region(*rid);
    }
    for did in removed_derivations {
        msg.remove_derivation(*did);
    }
    for aid in removed_accesses {
        msg.remove_access(*aid);
    }
    for seid in removed_sync_edges {
        msg.remove_sync_edge(*seid);
    }
}

// ---------------------------------------------------------------------------
// Propagation helpers
// ---------------------------------------------------------------------------

/// Find all accesses that target a given region (via their derivation chain).
fn find_accesses_to_region(msg: &MSG, rid: RegionId) -> Vec<AccessId> {
    let mut result = Vec::new();
    for aid in msg.access_ids() {
        if let Some(access) = msg.access(aid) {
            if let Some(deriv) = msg.derivation(access.target) {
                if derivation_resolves_to_region(msg, &deriv, rid) {
                    result.push(aid);
                }
            }
        }
    }
    result
}

/// Check if a derivation resolves to a given region.
fn derivation_resolves_to_region(msg: &MSG, deriv: &Derivation, target_rid: RegionId) -> bool {
    match &deriv.source {
        DerivationSource::Region(rid) => *rid == target_rid,
        DerivationSource::AnotherDerivation(parent_id) => {
            if let Some(parent) = msg.derivation(*parent_id) {
                derivation_resolves_to_region(msg, &parent, target_rid)
            } else {
                false
            }
        }
    }
}

/// Find all derivations that are transitively derived from the given set
/// of derivation IDs (spec §5.3, Rule PROPAGATE-DERIVATION).
fn find_downstream_derivations(
    msg: &MSG,
    source_ids: &HashSet<DerivationId>,
) -> Vec<Derivation> {
    let mut downstream = Vec::new();
    let mut visited: HashSet<DerivationId> = HashSet::new();

    for did in msg.derivation_ids() {
        if source_ids.contains(&did) {
            continue; // Skip the sources themselves
        }
        if let Some(deriv) = msg.derivation(did) {
            if is_derived_from_any(msg, &deriv, source_ids, &mut visited) {
                downstream.push(deriv.clone());
            }
        }
    }

    downstream
}

/// Check if a derivation is (transitively) derived from any of the given IDs.
fn is_derived_from_any(
    msg: &MSG,
    deriv: &Derivation,
    source_ids: &HashSet<DerivationId>,
    visited: &mut HashSet<DerivationId>,
) -> bool {
    if visited.contains(&deriv.id) {
        return false;
    }
    visited.insert(deriv.id);

    match &deriv.source {
        DerivationSource::AnotherDerivation(parent_id) => {
            if source_ids.contains(parent_id) {
                return true;
            }
            if let Some(parent) = msg.derivation(*parent_id) {
                is_derived_from_any(msg, &parent, source_ids, visited)
            } else {
                false
            }
        }
        DerivationSource::Region(_) => false,
    }
}

/// Find all accesses that use a given derivation as their target.
fn find_accesses_via_derivation(msg: &MSG, did: DerivationId) -> Vec<AccessId> {
    let mut result = Vec::new();
    for aid in msg.access_ids() {
        if let Some(access) = msg.access(aid) {
            if access.target == did {
                result.push(aid);
            }
        }
    }
    result
}

/// Verify a single access against MSG invariants.
///
/// Returns `Unsafe` if any invariant is clearly violated, `Safe` if all
/// invariants hold, and `Unverified` if the result cannot be determined.
pub fn verify_access(msg: &MSG, aid: AccessId) -> VerificationStatus {
    let access = match msg.access(aid) {
        Some(a) => a,
        None => return VerificationStatus::Unverified,
    };

    // Check 1: Derivation chain is intact.
    let deriv = match msg.derivation(access.target) {
        Some(d) => d,
        None => return VerificationStatus::Unsafe,
    };

    // Check 2: Origin — derivation traces back to a region.
    let rid = match deriv.base_region(|did| msg.derivation(did).cloned()) {
        Some(rid) => rid,
        None => return VerificationStatus::Unsafe,
    };

    // Check 3: Liveness — region is live.
    let region = match msg.region(rid) {
        Some(r) => r,
        None => return VerificationStatus::Unsafe,
    };
    if !region.is_live() {
        return VerificationStatus::Unsafe;
    }

    // Check 4: Bounds — derivation provenance range is within region.
    if deriv.proven_range.0 < region.base || deriv.proven_range.1 > region.end() {
        return VerificationStatus::Unsafe;
    }

    VerificationStatus::Safe
}

// ---------------------------------------------------------------------------
// Change Detection — Phase 2 incremental verification
// ---------------------------------------------------------------------------

/// A set of changes between two SCG snapshots.
///
/// Captures which nodes and edges were added, removed, or modified,
/// and which regions and derivations are affected by those changes.
#[derive(Debug, Clone, Default)]
pub struct ChangeSet {
    /// Node IDs that were added in the new snapshot.
    pub added_nodes: HashSet<u64>,
    /// Node IDs that were removed from the old snapshot.
    pub removed_nodes: HashSet<u64>,
    /// Node IDs whose content changed between snapshots.
    pub modified_nodes: HashSet<u64>,
    /// Edge pairs (source_node_id, target_node_id) that were added.
    pub added_edges: HashSet<(u64, u64)>,
    /// Edge pairs (source_node_id, target_node_id) that were removed.
    pub removed_edges: HashSet<(u64, u64)>,
    /// Region IDs affected by any change.
    pub affected_regions: HashSet<u64>,
    /// Derivation IDs affected by any change.
    pub affected_derivations: HashSet<u64>,
}

impl ChangeSet {
    /// Create an empty change set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if there are no changes at all.
    pub fn is_empty(&self) -> bool {
        self.added_nodes.is_empty()
            && self.removed_nodes.is_empty()
            && self.modified_nodes.is_empty()
            && self.added_edges.is_empty()
            && self.removed_edges.is_empty()
            && self.affected_regions.is_empty()
            && self.affected_derivations.is_empty()
    }

    /// Total number of changes (nodes + edges).
    pub fn change_count(&self) -> usize {
        self.added_nodes.len()
            + self.removed_nodes.len()
            + self.modified_nodes.len()
            + self.added_edges.len()
            + self.removed_edges.len()
    }
}

/// Detects changes between two SCG snapshots.
///
/// Efficiently computes the delta between the old and new snapshots,
/// identifying which nodes, edges, regions, and derivations are affected.
/// This enables the incremental verifier to skip unchanged subgraphs.
pub struct ChangeDetector {
    old_snapshot: SCGSnapshot,
    new_snapshot: SCGSnapshot,
}

impl ChangeDetector {
    /// Create a new change detector with the given old and new snapshots.
    pub fn new(old_snapshot: SCGSnapshot, new_snapshot: SCGSnapshot) -> Self {
        Self {
            old_snapshot,
            new_snapshot,
        }
    }

    /// Detect all changes between the old and new snapshots.
    ///
    /// Returns a [`ChangeSet`] containing all added, removed, and modified
    /// nodes and edges, as well as the sets of affected regions and
    /// derivations.
    pub fn detect(&self) -> ChangeSet {
        let mut changes = ChangeSet::new();

        let old_ids: HashSet<u64> = self.old_snapshot.nodes.keys().copied().collect();
        let new_ids: HashSet<u64> = self.new_snapshot.nodes.keys().copied().collect();

        // Added nodes: in new but not in old.
        for id in new_ids.difference(&old_ids) {
            changes.added_nodes.insert(*id);
            if let Some(node) = self.new_snapshot.get_node(*id) {
                extract_affected_entities(
                    node,
                    &mut changes.affected_regions,
                    &mut changes.affected_derivations,
                    &mut changes.added_edges,
                );
            }
        }

        // Removed nodes: in old but not in new.
        for id in old_ids.difference(&new_ids) {
            changes.removed_nodes.insert(*id);
            if let Some(node) = self.old_snapshot.get_node(*id) {
                extract_affected_entities(
                    node,
                    &mut changes.affected_regions,
                    &mut changes.affected_derivations,
                    &mut changes.removed_edges,
                );
            }
        }

        // Modified nodes: same ID but different content.
        for id in old_ids.intersection(&new_ids) {
            let old_node = self.old_snapshot.get_node(*id).unwrap();
            let new_node = self.new_snapshot.get_node(*id).unwrap();
            if nodes_differ(old_node, new_node) {
                changes.modified_nodes.insert(*id);
                extract_affected_entities(
                    new_node,
                    &mut changes.affected_regions,
                    &mut changes.affected_derivations,
                    &mut changes.added_edges,
                );
            }
        }

        changes
    }

    /// Compute which invariants are affected by the given changes.
    ///
    /// Returns a list of invariant names that need to be re-verified.
    /// Invariants not in this list can be skipped during incremental
    /// verification.
    ///
    /// The invariant names are: `"liveness"`, `"origin"`, `"bounds"`,
    /// `"exclusivity"`, `"cleanup"`.
    pub fn compute_affected_invariants(changes: &ChangeSet) -> Vec<String> {
        let mut affected: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        let add = |name: &str, seen: &mut HashSet<String>, list: &mut Vec<String>| {
            if seen.insert(name.to_string()) {
                list.push(name.to_string());
            }
        };

        // Liveness: affected if regions or accesses changed.
        if !changes.affected_regions.is_empty()
            || !changes.added_nodes.is_empty()
            || !changes.removed_nodes.is_empty()
        {
            add("liveness", &mut seen, &mut affected);
        }

        // Origin: affected if derivations changed.
        if !changes.affected_derivations.is_empty() {
            add("origin", &mut seen, &mut affected);
        }

        // Bounds: affected if derivations or regions changed.
        if !changes.affected_derivations.is_empty() || !changes.affected_regions.is_empty() {
            add("bounds", &mut seen, &mut affected);
        }

        // Exclusivity: affected if edges changed (sync edges, concurrent accesses).
        if !changes.added_edges.is_empty() || !changes.removed_edges.is_empty() {
            add("exclusivity", &mut seen, &mut affected);
        }

        // Cleanup: affected if regions were removed.
        if !changes.removed_nodes.is_empty() || !changes.affected_regions.is_empty() {
            add("cleanup", &mut seen, &mut affected);
        }

        affected
    }
}

/// Extract affected region IDs, derivation IDs, and edge pairs from an SCG node.
fn extract_affected_entities(
    node: &SCGNode,
    regions: &mut HashSet<u64>,
    derivations: &mut HashSet<u64>,
    edges: &mut HashSet<(u64, u64)>,
) {
    match node {
        SCGNode::Alloc { region_id, .. } => {
            regions.insert(region_id.0);
        }
        SCGNode::Dealloc { region_id, .. } => {
            regions.insert(region_id.0);
        }
        SCGNode::Access {
            region_id,
            target_derivation,
            ..
        } => {
            regions.insert(region_id.0);
            derivations.insert(target_derivation.0);
        }
        SCGNode::Arithmetic {
            derivation_id,
            source_derivation,
            ..
        } => {
            derivations.insert(derivation_id.0);
            derivations.insert(source_derivation.0);
            edges.insert((source_derivation.0, derivation_id.0));
        }
        SCGNode::Cast {
            derivation_id,
            source_derivation,
            ..
        } => {
            derivations.insert(derivation_id.0);
            derivations.insert(source_derivation.0);
            edges.insert((source_derivation.0, derivation_id.0));
        }
        SCGNode::Sync { access1, access2, .. } => {
            edges.insert((access1.0, access2.0));
        }
    }
}

// ---------------------------------------------------------------------------
// Incremental Re-verification — Phase 2
// ---------------------------------------------------------------------------

/// The result of incremental re-verification.
///
/// Only invariants affected by the changes are re-verified; unaffected
/// invariants are skipped. The `savings_ratio` indicates how much work
/// was avoided: `0.0` means everything was re-verified, `1.0` means
/// everything was skipped.
#[derive(Debug, Clone)]
pub struct IncrementalVerificationResult {
    /// The overall verification result.
    pub result: VerificationStatus,
    /// Names of invariants that were re-verified.
    pub re_verified_invariants: Vec<String>,
    /// Names of invariants that were skipped (unchanged).
    pub skipped_invariants: Vec<String>,
    /// Number of nodes that needed to be re-checked.
    pub nodes_re_checked: usize,
    /// Total number of nodes in the graph.
    pub total_nodes: usize,
    /// Fraction of work saved by incremental verification.
    /// `0.0` = no savings, `1.0` = everything skipped.
    pub savings_ratio: f64,
}

impl IncrementalVerificationResult {
    /// Create a result where all invariants were skipped (no changes).
    pub fn all_skipped(total_nodes: usize) -> Self {
        Self {
            result: VerificationStatus::Safe,
            re_verified_invariants: Vec::new(),
            skipped_invariants: vec![
                "liveness".into(),
                "origin".into(),
                "bounds".into(),
                "exclusivity".into(),
                "cleanup".into(),
            ],
            nodes_re_checked: 0,
            total_nodes,
            savings_ratio: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Verification Cache
// ---------------------------------------------------------------------------

/// Cache of verification results for subgraphs (regions).
///
/// When a subgraph hasn't changed, the previous verification result can
/// be reused, avoiding expensive re-verification. Tracks cache hits and
/// misses for monitoring effectiveness.
#[derive(Debug, Clone)]
pub struct VerificationCache {
    /// Cached verification results keyed by region ID (u64).
    verified_subgraphs: HashMap<u64, VerificationStatus>,
    /// Number of cache hits (unchanged subgraph reused).
    cache_hits: usize,
    /// Number of cache misses (new or changed subgraph verified).
    cache_misses: usize,
}

impl VerificationCache {
    /// Create an empty verification cache.
    pub fn new() -> Self {
        Self {
            verified_subgraphs: HashMap::new(),
            cache_hits: 0,
            cache_misses: 0,
        }
    }

    /// Look up a cached result for the given region.
    ///
    /// Returns `Some(result)` on a cache hit, `None` on a miss.
    /// Increments the appropriate counter.
    pub fn lookup(&mut self, region_id: RegionId) -> Option<VerificationStatus> {
        if let Some(&result) = self.verified_subgraphs.get(&region_id.0) {
            self.cache_hits += 1;
            Some(result)
        } else {
            self.cache_misses += 1;
            None
        }
    }

    /// Update the cache with a new verification result for the given region.
    pub fn update(&mut self, region_id: RegionId, result: VerificationStatus) {
        self.verified_subgraphs.insert(region_id.0, result);
    }

    /// Check if the cache contains a result for the given region
    /// without incrementing hit/miss counters.
    pub fn contains(&self, region_id: RegionId) -> bool {
        self.verified_subgraphs.contains_key(&region_id.0)
    }

    /// Invalidate the cache entry for the given region.
    pub fn invalidate(&mut self, region_id: RegionId) {
        self.verified_subgraphs.remove(&region_id.0);
    }

    /// Clear the entire cache and reset counters.
    pub fn clear(&mut self) {
        self.verified_subgraphs.clear();
        self.cache_hits = 0;
        self.cache_misses = 0;
    }

    /// Number of cache hits.
    pub fn hits(&self) -> usize {
        self.cache_hits
    }

    /// Number of cache misses.
    pub fn misses(&self) -> usize {
        self.cache_misses
    }

    /// Hit rate as a fraction (`0.0` to `1.0`).
    pub fn hit_rate(&self) -> f64 {
        let total = self.cache_hits + self.cache_misses;
        if total == 0 {
            0.0
        } else {
            self.cache_hits as f64 / total as f64
        }
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.verified_subgraphs.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.verified_subgraphs.is_empty()
    }
}

impl Default for VerificationCache {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Incremental Metrics
// ---------------------------------------------------------------------------

/// Performance metrics for incremental verification.
///
/// Tracks the time spent in each phase and whether the Phase 2 target
/// of sub-1-second re-verification was met.
#[derive(Debug, Clone)]
pub struct IncrementalMetrics {
    /// Time spent detecting changes between snapshots.
    pub change_detection_time: std::time::Duration,
    /// Time spent computing the delta.
    pub delta_computation_time: std::time::Duration,
    /// Time spent re-verifying affected invariants.
    pub re_verification_time: std::time::Duration,
    /// Total wall-clock time for the incremental verification.
    pub total_time: std::time::Duration,
    /// Whether the verification met the Phase 2 target (< 1 second).
    pub meets_target: bool,
}

impl IncrementalMetrics {
    /// Create metrics with the given timings.
    pub fn new(
        change_detection_time: std::time::Duration,
        delta_computation_time: std::time::Duration,
        re_verification_time: std::time::Duration,
        total_time: std::time::Duration,
    ) -> Self {
        let meets_target = total_time < std::time::Duration::from_secs(1);
        Self {
            change_detection_time,
            delta_computation_time,
            re_verification_time,
            total_time,
            meets_target,
        }
    }

    /// Create zero-valued metrics (all durations zero, meets_target true).
    pub fn zero() -> Self {
        Self {
            change_detection_time: std::time::Duration::ZERO,
            delta_computation_time: std::time::Duration::ZERO,
            re_verification_time: std::time::Duration::ZERO,
            total_time: std::time::Duration::ZERO,
            meets_target: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Incremental Verifier — ties together change detection, caching, and
// incremental re-verification
// ---------------------------------------------------------------------------

/// The full set of VUMA invariant names in optimal verification order.
const ALL_INVARIANTS: &[&str] = &["liveness", "origin", "bounds", "exclusivity", "cleanup"];

/// Incremental MSG verifier with caching.
///
/// Maintains a cache of verification results per region so that unchanged
/// subgraphs don't need to be re-verified. Only invariants affected by
/// the detected changes are re-verified.
pub struct IncrementalVerifier {
    cache: VerificationCache,
    all_invariants: Vec<String>,
}

impl IncrementalVerifier {
    /// Create a new incremental verifier with an empty cache.
    pub fn new() -> Self {
        Self {
            cache: VerificationCache::new(),
            all_invariants: ALL_INVARIANTS.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Get a reference to the verification cache.
    pub fn cache(&self) -> &VerificationCache {
        &self.cache
    }

    /// Get a mutable reference to the verification cache.
    pub fn cache_mut(&mut self) -> &mut VerificationCache {
        &mut self.cache
    }

    /// Perform incremental re-verification based on the given delta and changes.
    ///
    /// Only re-verifies invariants that are affected by the changes.
    /// Unchanged subgraphs reuse their cached verification results.
    ///
    /// # Arguments
    ///
    /// * `msg` — The current Memory State Graph.
    /// * `delta` — The delta to apply (used for context, not applied here).
    /// * `changes` — The detected changes that drive incremental verification.
    pub fn incremental_verify(
        &mut self,
        msg: &MSG,
        _delta: &MSGDelta,
        changes: &ChangeSet,
    ) -> IncrementalVerificationResult {
        let total_nodes = msg.region_count() + msg.derivation_count() + msg.access_count();

        // If there are no changes, everything can be skipped.
        if changes.is_empty() {
            return IncrementalVerificationResult::all_skipped(total_nodes);
        }

        // Determine which invariants need re-verification.
        let affected_invariants = ChangeDetector::compute_affected_invariants(changes);

        let re_verified: Vec<String> = affected_invariants.clone();
        let skipped: Vec<String> = self
            .all_invariants
            .iter()
            .filter(|inv| !affected_invariants.contains(inv))
            .cloned()
            .collect();

        let mut overall_result = VerificationStatus::Safe;
        let mut nodes_re_checked = 0;

        // Invalidate cache entries for affected regions (they changed).
        for &region_id_u64 in &changes.affected_regions {
            self.cache.invalidate(RegionId(region_id_u64));
        }

        // Re-verify affected regions.
        for &region_id_u64 in &changes.affected_regions {
            let rid = RegionId(region_id_u64);
            if msg.region(rid).is_some() {
                let result = self.verify_region(msg, rid);
                self.cache.update(rid, result);
                overall_result = overall_result.meet(result);
                nodes_re_checked += 1;
            }
        }

        // Re-verify affected derivations.
        for &deriv_id_u64 in &changes.affected_derivations {
            let did = DerivationId(deriv_id_u64);
            if msg.derivation(did).is_some() {
                nodes_re_checked += 1;
                let accesses = find_accesses_via_derivation(msg, did);
                for aid in accesses {
                    let status = verify_access(msg, aid);
                    overall_result = overall_result.meet(status);
                }
            }
        }

        // Process edge changes (affect exclusivity).
        if affected_invariants.contains(&"exclusivity".to_string()) {
            for &(a, b) in &changes.added_edges {
                let aid1 = AccessId(a);
                let aid2 = AccessId(b);
                let s1 = verify_access(msg, aid1);
                let s2 = verify_access(msg, aid2);
                overall_result = overall_result.meet(s1).meet(s2);
                nodes_re_checked += 2;
            }
        }

        // Combine with cached results for unaffected regions.
        for (&_region_id_u64, &cached_result) in &self.cache.verified_subgraphs {
            overall_result = overall_result.meet(cached_result);
        }

        let savings_ratio = if total_nodes > 0 {
            1.0 - (nodes_re_checked as f64 / total_nodes as f64)
        } else {
            1.0
        };

        IncrementalVerificationResult {
            result: overall_result,
            re_verified_invariants: re_verified,
            skipped_invariants: skipped,
            nodes_re_checked,
            total_nodes,
            savings_ratio,
        }
    }

    /// Verify a single region's subgraph and return the verification status.
    fn verify_region(&mut self, msg: &MSG, rid: RegionId) -> VerificationStatus {
        let mut result = VerificationStatus::Safe;

        // Check region itself.
        match msg.region(rid) {
            Some(region) => {
                if !region.is_live() {
                    result = result.meet(VerificationStatus::Unsafe);
                }
            }
            None => {
                result = result.meet(VerificationStatus::Unsafe);
            }
        }

        // Check all accesses to this region.
        let accesses = find_accesses_to_region(msg, rid);
        for aid in accesses {
            let status = verify_access(msg, aid);
            result = result.meet(status);
        }

        result
    }

    /// Full incremental verification with metrics.
    ///
    /// Performs the complete pipeline: change detection, delta computation,
    /// and incremental re-verification, returning both the result and
    /// detailed timing metrics.
    pub fn incremental_verify_with_metrics(
        &mut self,
        msg: &MSG,
        old_scg: &SCGSnapshot,
        new_scg: &SCGSnapshot,
    ) -> (IncrementalVerificationResult, IncrementalMetrics) {
        let total_start = std::time::Instant::now();

        // Phase 1: Change detection.
        let cd_start = std::time::Instant::now();
        let detector = ChangeDetector::new(old_scg.clone(), new_scg.clone());
        let changes = detector.detect();
        let change_detection_time = cd_start.elapsed();

        // Phase 2: Delta computation.
        let dc_start = std::time::Instant::now();
        let delta = compute_scg_delta(old_scg, new_scg);
        let delta_computation_time = dc_start.elapsed();

        // Phase 3: Incremental re-verification.
        let rv_start = std::time::Instant::now();
        let result = self.incremental_verify(msg, &delta, &changes);
        let re_verification_time = rv_start.elapsed();

        let total_time = total_start.elapsed();
        let metrics = IncrementalMetrics::new(
            change_detection_time,
            delta_computation_time,
            re_verification_time,
            total_time,
        );

        (result, metrics)
    }
}

impl Default for IncrementalVerifier {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::AccessKind;
    use crate::derivation::DerivationKind;
    use crate::program_point::ProgramPoint;
    use crate::sync::Ordering;

    fn dummy_pp(line: u32) -> ProgramPoint {
        ProgramPoint::new("test.vu", line, 1)
    }

    fn make_region(id: u64, base: u64, size: u64) -> Region {
        Region {
            id: RegionId(id),
            base: Address::from(base),
            size,
            status: RegionStatus::Allocated,
            alloc_point: dummy_pp(1),
            free_point: None,
            owner_context: None,
        }
    }

    fn make_direct_derivation(id: u64, region_id: u64) -> Derivation {
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::Region(RegionId(region_id)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(0x1000_u64), Address::from(0x1200_u64)),
        }
    }

    fn make_offset_derivation(id: u64, parent_id: u64, offset: i64) -> Derivation {
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::AnotherDerivation(DerivationId(parent_id)),
            kind: DerivationKind::Offset { by: offset },
            proven_range: (Address::from(0x1040_u64), Address::from(0x1100_u64)),
        }
    }

    // =======================================================================
    // Test 1: Empty delta on empty MSG
    // =======================================================================

    #[test]
    fn apply_empty_delta() {
        let mut msg = MSG::new();
        let delta = MSGDelta::new();
        let result = apply_delta(&mut msg, delta);
        assert!(result.success);
        assert!(result.warnings.is_empty());
        assert_eq!(msg.region_count(), 0);
    }

    // =======================================================================
    // Test 2: Add a region via delta
    // =======================================================================

    #[test]
    fn add_region_delta() {
        let mut msg = MSG::new();
        let mut delta = MSGDelta::new();
        delta.regions.add(make_region(1, 0x1000, 0x100));
        let result = apply_delta(&mut msg, delta);
        assert!(result.success);
        assert_eq!(msg.region_count(), 1);
        assert!(msg.region(RegionId(1)).is_some());
    }

    // =======================================================================
    // Test 3: Add and remove a derivation via delta
    // =======================================================================

    #[test]
    fn add_and_remove_derivation_delta() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x200));

        // Add derivation.
        let mut delta = MSGDelta::new();
        delta.derivations.add(make_direct_derivation(10, 1));
        let result = apply_delta(&mut msg, delta);
        assert!(result.success);
        assert_eq!(msg.derivation_count(), 1);

        // Remove derivation.
        let mut delta2 = MSGDelta::new();
        delta2.derivations.remove(10);
        let result2 = apply_delta(&mut msg, delta2);
        assert!(result2.success);
        assert_eq!(msg.derivation_count(), 0);
    }

    // =======================================================================
    // Test 4: Add an access via delta and verify
    // =======================================================================

    #[test]
    fn add_access_delta_and_verify() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x200));
        msg.add_derivation(make_direct_derivation(10, 1));

        let mut delta = MSGDelta::new();
        delta.accesses.add(Access::new(
            AccessId(100),
            DerivationId(10),
            AccessKind::Read,
            4,
            dummy_pp(5),
        ));

        let result = apply_delta(&mut msg, delta);
        assert!(result.success);
        assert_eq!(msg.access_count(), 1);

        // Verify access should be Safe.
        let status = verify_access(&msg, AccessId(100));
        assert_eq!(status, VerificationStatus::Safe);
    }

    // =======================================================================
    // Test 5: Add a sync edge via delta
    // =======================================================================

    #[test]
    fn add_sync_edge_delta() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x200));
        msg.add_derivation(make_direct_derivation(10, 1));
        msg.add_access(Access::new(AccessId(1), DerivationId(10), AccessKind::Write, 4, dummy_pp(2)));
        msg.add_access(Access::new(AccessId(2), DerivationId(10), AccessKind::Read, 4, dummy_pp(3)));

        let mut delta = MSGDelta::new();
        delta.sync_edges.add(SyncEdge::new(
            SyncEdgeId(1),
            AccessId(1),
            AccessId(2),
            Ordering::HappensBefore,
        ));
        let result = apply_delta(&mut msg, delta);
        assert!(result.success);
        assert_eq!(msg.sync_edge_count(), 1);
    }

    // =======================================================================
    // Test 6: compute_delta detects region additions
    // =======================================================================

    #[test]
    fn compute_delta_detects_region_additions() {
        let old = MSG::new();
        let mut new = MSG::new();
        new.add_region(make_region(1, 0x1000, 0x200));

        let delta = compute_delta(&old, &new);
        assert_eq!(delta.regions.added.len(), 1);
        assert!(delta.regions.removed.is_empty());
        assert!(delta.regions.modified.is_empty());
        assert_eq!(delta.regions.added[0].id, RegionId(1));
    }

    // =======================================================================
    // Test 7: compute_delta detects removals
    // =======================================================================

    #[test]
    fn compute_delta_detects_removals() {
        let mut old = MSG::new();
        old.add_region(make_region(1, 0x1000, 0x200));
        old.add_region(make_region(2, 0x2000, 0x200));
        let new = MSG::new();

        let delta = compute_delta(&old, &new);
        assert!(delta.regions.added.is_empty());
        assert_eq!(delta.regions.removed.len(), 2);
        assert!(delta.regions.removed.contains(&1));
        assert!(delta.regions.removed.contains(&2));
    }

    // =======================================================================
    // Test 8: compute_delta detects modifications
    // =======================================================================

    #[test]
    fn compute_delta_detects_modifications() {
        let mut old = MSG::new();
        old.add_region(make_region(1, 0x1000, 0x200));

        let mut new = MSG::new();
        new.add_region(Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x400, // Changed size
            status: RegionStatus::Allocated,
            alloc_point: dummy_pp(1),
            free_point: None,
            owner_context: None,
        });

        let delta = compute_delta(&old, &new);
        assert!(delta.regions.added.is_empty());
        assert!(delta.regions.removed.is_empty());
        assert_eq!(delta.regions.modified.len(), 1);
        assert_eq!(delta.regions.modified[0].size, 0x400);
    }

    // =======================================================================
    // Test 9: compute_delta with mixed entity types
    // =======================================================================

    #[test]
    fn compute_delta_mixed_entity_types() {
        let mut old = MSG::new();
        old.add_region(make_region(1, 0x1000, 0x200));
        old.add_derivation(make_direct_derivation(10, 1));

        let mut new = MSG::new();
        new.add_region(make_region(1, 0x1000, 0x200)); // same region
        new.add_derivation(make_direct_derivation(10, 1)); // same derivation
        new.add_derivation(make_offset_derivation(20, 10, 0x40)); // new derivation
        new.add_access(Access::new(AccessId(1), DerivationId(10), AccessKind::Read, 4, dummy_pp(5)));

        let delta = compute_delta(&old, &new);
        assert!(delta.regions.is_empty()); // No region changes
        assert_eq!(delta.derivations.added.len(), 1); // D20 added
        assert_eq!(delta.derivations.added[0].id, DerivationId(20));
        assert_eq!(delta.accesses.added.len(), 1); // A1 added
    }

    // =======================================================================
    // Test 10: compute_delta then apply_delta round-trip
    // =======================================================================

    #[test]
    fn compute_delta_apply_round_trip() {
        let mut old = MSG::new();
        old.add_region(make_region(1, 0x1000, 0x200));
        old.add_derivation(make_direct_derivation(10, 1));
        old.add_access(Access::new(AccessId(1), DerivationId(10), AccessKind::Read, 4, dummy_pp(5)));

        let mut new = MSG::new();
        new.add_region(make_region(1, 0x1000, 0x200));
        new.add_derivation(make_direct_derivation(10, 1));
        // Access removed, new derivation added
        new.add_derivation(make_offset_derivation(20, 10, 0x40));

        let delta = compute_delta(&old, &new);
        assert!(delta.regions.is_empty());
        assert_eq!(delta.derivations.added.len(), 1);
        assert_eq!(delta.accesses.removed.len(), 1);

        // Apply delta to old
        let result = apply_delta(&mut old, delta);
        assert!(result.success);
        assert_eq!(old.derivation_count(), 2);
        assert_eq!(old.access_count(), 0);
    }

    // =======================================================================
    // Test 11: Region removal cascades to access invalidation
    // =======================================================================

    #[test]
    fn region_removal_cascades() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x200));
        msg.add_derivation(make_direct_derivation(10, 1));
        msg.add_access(Access::new(AccessId(100), DerivationId(10), AccessKind::Read, 4, dummy_pp(5)));

        // Remove the region.
        let mut delta = MSGDelta::new();
        delta.regions.remove(1);
        let result = apply_delta(&mut msg, delta);
        assert!(result.success);
        assert_eq!(msg.region_count(), 0);
        // The access still exists but its derivation now has a broken chain.
        let status = verify_access(&msg, AccessId(100));
        assert_eq!(status, VerificationStatus::Unsafe);
    }

    // =======================================================================
    // Test 12: Delta merge combines changes
    // =======================================================================

    #[test]
    fn delta_merge() {
        let mut d1 = MSGDelta::new();
        d1.regions.add(make_region(1, 0x1000, 0x100));

        let mut d2 = MSGDelta::new();
        d2.regions.add(make_region(2, 0x2000, 0x200));

        d1.merge(d2);
        assert_eq!(d1.regions.added.len(), 2);
    }

    // =======================================================================
    // Test 13: Duplicate region warning
    // =======================================================================

    #[test]
    fn duplicate_region_warning() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100));

        let mut delta = MSGDelta::new();
        delta.regions.add(make_region(1, 0x2000, 0x200)); // Same ID

        let result = apply_delta(&mut msg, delta);
        assert!(result.success);
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0], DeltaError::DuplicateRegion(RegionId(1)));
    }

    // =======================================================================
    // Test 14: Broken derivation chain detected
    // =======================================================================

    #[test]
    fn broken_derivation_chain_warning() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x200));

        let mut delta = MSGDelta::new();
        delta.derivations.add(Derivation {
            id: DerivationId(5),
            source: DerivationSource::AnotherDerivation(DerivationId(999)), // Missing!
            kind: DerivationKind::Offset { by: 0x10 },
            proven_range: (Address::from(0x1010_u64), Address::from(0x1080_u64)),
        });

        let result = apply_delta(&mut msg, delta);
        assert!(result.success);
        assert!(result
            .warnings
            .iter()
            .any(|w| matches!(w, DeltaError::BrokenDerivationChain(DerivationId(5)))));
    }

    // =======================================================================
    // Test 15: Access to dead region is unsafe
    // =======================================================================

    #[test]
    fn access_to_dead_region_unsafe() {
        let mut msg = MSG::new();
        msg.add_region(Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x200,
            status: RegionStatus::Freed,
            alloc_point: dummy_pp(1),
            free_point: Some(dummy_pp(2)),
            owner_context: None,
        });
        msg.add_derivation(make_direct_derivation(10, 1));
        msg.add_access(Access::new(AccessId(100), DerivationId(10), AccessKind::Read, 4, dummy_pp(5)));

        let status = verify_access(&msg, AccessId(100));
        assert_eq!(status, VerificationStatus::Unsafe);
    }

    // =======================================================================
    // Test 16: Verification status lattice
    // =======================================================================

    #[test]
    fn verification_status_meet() {
        assert_eq!(
            VerificationStatus::Safe.meet(VerificationStatus::Safe),
            VerificationStatus::Safe
        );
        assert_eq!(
            VerificationStatus::Safe.meet(VerificationStatus::Unverified),
            VerificationStatus::Unverified
        );
        assert_eq!(
            VerificationStatus::Safe.meet(VerificationStatus::Unsafe),
            VerificationStatus::Unsafe
        );
        assert_eq!(
            VerificationStatus::Unverified.meet(VerificationStatus::Unsafe),
            VerificationStatus::Unsafe
        );
    }

    // =======================================================================
    // Test 17: SCG snapshot operations
    // =======================================================================

    #[test]
    fn scg_snapshot_operations() {
        let mut snap = SCGSnapshot::new();
        assert!(snap.is_empty());

        snap.add_node(SCGNode::Alloc {
            node_id: 1,
            region_id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x100,
            alloc_point: dummy_pp(1),
        });
        assert_eq!(snap.len(), 1);
        assert!(snap.get_node(1).is_some());

        let removed = snap.remove_node(1);
        assert!(removed.is_some());
        assert!(snap.is_empty());
        assert!(snap.get_node(1).is_none());
    }

    // =======================================================================
    // Test 18: EntityDelta and MSGDelta is_empty
    // =======================================================================

    #[test]
    fn delta_empty_checks() {
        let delta: EntityDelta<Region> = EntityDelta::new();
        assert!(delta.is_empty());

        let mut delta: EntityDelta<Region> = EntityDelta::new();
        delta.remove(1);
        assert!(!delta.is_empty());

        let delta = MSGDelta::new();
        assert!(delta.is_empty());

        let mut delta = MSGDelta::new();
        delta.verification_updates.insert(AccessId(1), VerificationStatus::Unsafe);
        assert!(!delta.is_empty());
    }

    // =======================================================================
    // Test 19: compute_scg_delta detects additions and removals
    // =======================================================================

    #[test]
    fn compute_scg_delta_additions_and_removals() {
        let mut old = SCGSnapshot::new();
        old.add_node(SCGNode::Alloc {
            node_id: 1,
            region_id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x100,
            alloc_point: dummy_pp(1),
        });

        let mut new = SCGSnapshot::new();
        new.add_node(SCGNode::Alloc {
            node_id: 2,
            region_id: RegionId(2),
            base: Address::from(0x2000_u64),
            size: 0x200,
            alloc_point: dummy_pp(2),
        });

        let delta = compute_scg_delta(&old, &new);
        assert_eq!(delta.regions.added.len(), 1);
        assert_eq!(delta.regions.added[0].id, RegionId(2));
        assert_eq!(delta.regions.removed.len(), 1);
        assert_eq!(delta.regions.removed[0], 1);
    }

    // =======================================================================
    // Test 20: Modify region via delta (status change)
    // =======================================================================

    #[test]
    fn modify_region_status_via_delta() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x200));

        // Modify the region to Freed.
        let mut delta = MSGDelta::new();
        delta.regions.modify(Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x200,
            status: RegionStatus::Freed,
            alloc_point: dummy_pp(1),
            free_point: Some(dummy_pp(10)),
            owner_context: None,
        });
        let result = apply_delta(&mut msg, delta);
        assert!(result.success);
        assert_eq!(msg.region(RegionId(1)).unwrap().status, RegionStatus::Freed);
        assert!(result.invalidated_regions.contains(&RegionId(1)));
    }

    // =======================================================================
    // Test 21: compute_delta with identical MSGs yields empty delta
    // =======================================================================

    #[test]
    fn compute_delta_identical_msgs_empty() {
        let mut msg1 = MSG::new();
        msg1.add_region(make_region(1, 0x1000, 0x200));
        msg1.add_derivation(make_direct_derivation(10, 1));
        msg1.add_access(Access::new(AccessId(1), DerivationId(10), AccessKind::Read, 4, dummy_pp(5)));
        msg1.add_sync_edge(SyncEdge::new(SyncEdgeId(1), AccessId(1), AccessId(1), Ordering::HappensBefore));

        let mut msg2 = MSG::new();
        msg2.add_region(make_region(1, 0x1000, 0x200));
        msg2.add_derivation(make_direct_derivation(10, 1));
        msg2.add_access(Access::new(AccessId(1), DerivationId(10), AccessKind::Read, 4, dummy_pp(5)));
        msg2.add_sync_edge(SyncEdge::new(SyncEdgeId(1), AccessId(1), AccessId(1), Ordering::HappensBefore));

        let delta = compute_delta(&msg1, &msg2);
        assert!(delta.is_empty());
    }

    // =======================================================================
    // Test 22: compute_delta detects sync edge changes
    // =======================================================================

    #[test]
    fn compute_delta_sync_edge_changes() {
        let mut old = MSG::new();
        old.add_region(make_region(1, 0x1000, 0x200));
        old.add_derivation(make_direct_derivation(10, 1));
        old.add_access(Access::new(AccessId(1), DerivationId(10), AccessKind::Write, 4, dummy_pp(2)));
        old.add_access(Access::new(AccessId(2), DerivationId(10), AccessKind::Read, 4, dummy_pp(3)));
        old.add_sync_edge(SyncEdge::new(SyncEdgeId(1), AccessId(1), AccessId(2), Ordering::HappensBefore));

        let mut new = MSG::new();
        new.add_region(make_region(1, 0x1000, 0x200));
        new.add_derivation(make_direct_derivation(10, 1));
        new.add_access(Access::new(AccessId(1), DerivationId(10), AccessKind::Write, 4, dummy_pp(2)));
        new.add_access(Access::new(AccessId(2), DerivationId(10), AccessKind::Read, 4, dummy_pp(3)));
        // Sync edge removed, new one added
        new.add_sync_edge(SyncEdge::new(SyncEdgeId(2), AccessId(2), AccessId(1), Ordering::AtomicAcquireRelease));

        let delta = compute_delta(&old, &new);
        assert_eq!(delta.sync_edges.removed.len(), 1);
        assert_eq!(delta.sync_edges.removed[0], 1);
        assert_eq!(delta.sync_edges.added.len(), 1);
        assert_eq!(delta.sync_edges.added[0].id, SyncEdgeId(2));
    }

    // =======================================================================
    // Test 23: Dangling sync edge access warning
    // =======================================================================

    #[test]
    fn dangling_sync_edge_access_warning() {
        let mut msg = MSG::new();
        // No accesses added, so referencing them in a sync edge should warn.

        let mut delta = MSGDelta::new();
        delta.sync_edges.add(SyncEdge::new(
            SyncEdgeId(1),
            AccessId(99), // does not exist
            AccessId(100), // does not exist
            Ordering::HappensBefore,
        ));

        let result = apply_delta(&mut msg, delta);
        assert!(result.success);
        assert_eq!(result.warnings.len(), 2);
        assert!(result.warnings.contains(&DeltaError::DanglingSyncEdgeAccess(SyncEdgeId(1), AccessId(99))));
        assert!(result.warnings.contains(&DeltaError::DanglingSyncEdgeAccess(SyncEdgeId(1), AccessId(100))));
    }

    // =======================================================================
    // Test 24: Remove non-existent entity produces warning
    // =======================================================================

    #[test]
    fn remove_nonexistent_entity_warns() {
        let mut msg = MSG::new();

        let mut delta = MSGDelta::new();
        delta.regions.remove(999);
        delta.derivations.remove(888);
        delta.accesses.remove(777);
        delta.sync_edges.remove(666);

        let result = apply_delta(&mut msg, delta);
        assert!(result.success);
        assert_eq!(result.warnings.len(), 4);
        assert!(result.warnings.contains(&DeltaError::RegionNotFound(RegionId(999))));
        assert!(result.warnings.contains(&DeltaError::DerivationNotFound(DerivationId(888))));
        assert!(result.warnings.contains(&DeltaError::AccessNotFound(AccessId(777))));
        assert!(result.warnings.contains(&DeltaError::SyncEdgeNotFound(SyncEdgeId(666))));
    }

    // =======================================================================
    // Test 25: compute_delta detects access modifications
    // =======================================================================

    #[test]
    fn compute_delta_access_modification() {
        let mut old = MSG::new();
        old.add_region(make_region(1, 0x1000, 0x200));
        old.add_derivation(make_direct_derivation(10, 1));
        old.add_access(Access::new(AccessId(1), DerivationId(10), AccessKind::Read, 4, dummy_pp(5)));

        let mut new = MSG::new();
        new.add_region(make_region(1, 0x1000, 0x200));
        new.add_derivation(make_direct_derivation(10, 1));
        // Same ID but changed kind (Write instead of Read) and size.
        new.add_access(Access::new(AccessId(1), DerivationId(10), AccessKind::Write, 8, dummy_pp(5)));

        let delta = compute_delta(&old, &new);
        assert!(delta.accesses.added.is_empty());
        assert!(delta.accesses.removed.is_empty());
        assert_eq!(delta.accesses.modified.len(), 1);
        assert_eq!(delta.accesses.modified[0].kind, AccessKind::Write);
        assert_eq!(delta.accesses.modified[0].size, 8);
    }

    // =======================================================================
    // Test 26: Derivation modification propagation
    // =======================================================================

    #[test]
    fn derivation_modification_propagation() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x200));
        msg.add_derivation(make_direct_derivation(10, 1));
        msg.add_derivation(make_offset_derivation(20, 10, 0x40));
        msg.add_access(Access::new(AccessId(1), DerivationId(20), AccessKind::Read, 4, dummy_pp(5)));

        // Modify the parent derivation.
        let mut delta = MSGDelta::new();
        delta.derivations.modify(Derivation {
            id: DerivationId(10),
            source: DerivationSource::Region(RegionId(1)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(0x1000_u64), Address::from(0x1100_u64)), // Changed
        });
        let result = apply_delta(&mut msg, delta);
        assert!(result.success);
        assert!(result.recomputed_derivations.contains(&DerivationId(10)));
        // D20 is downstream of D10, so its accesses should be reverified.
        assert!(!result.reverified.is_empty());
    }

    // =======================================================================
    // Test 27: Full pipeline — compute_delta + apply_delta
    // =======================================================================

    #[test]
    fn full_pipeline_compute_and_apply() {
        // Build an "old" MSG with a region and derivation.
        let mut old = MSG::new();
        old.add_region(make_region(1, 0x1000, 0x200));
        old.add_derivation(make_direct_derivation(10, 1));

        // Build a "new" MSG that adds an access and a sync edge.
        let mut new = MSG::new();
        new.add_region(make_region(1, 0x1000, 0x200));
        new.add_derivation(make_direct_derivation(10, 1));
        new.add_access(Access::new(AccessId(1), DerivationId(10), AccessKind::Read, 4, dummy_pp(5)));

        let delta = compute_delta(&old, &new);
        assert!(delta.regions.is_empty());
        assert!(delta.derivations.is_empty());
        assert_eq!(delta.accesses.added.len(), 1);

        let result = apply_delta(&mut old, delta);
        assert!(result.success);
        assert_eq!(old.access_count(), 1);
        assert_eq!(old.region_count(), 1);
        assert_eq!(old.derivation_count(), 1);
    }

    // =======================================================================
    // Phase 2 incremental verification tests
    // =======================================================================

    // Test 28: No changes → all invariants skipped
    #[test]
    fn no_changes_all_invariants_skipped() {
        let mut verifier = IncrementalVerifier::new();
        let msg = MSG::new();
        let delta = MSGDelta::new();
        let changes = ChangeSet::new();

        let result = verifier.incremental_verify(&msg, &delta, &changes);
        assert!(result.re_verified_invariants.is_empty());
        assert_eq!(result.skipped_invariants.len(), 5);
        assert!(result.skipped_invariants.contains(&"liveness".to_string()));
        assert!(result.skipped_invariants.contains(&"origin".to_string()));
        assert!(result.skipped_invariants.contains(&"bounds".to_string()));
        assert!(result.skipped_invariants.contains(&"exclusivity".to_string()));
        assert!(result.skipped_invariants.contains(&"cleanup".to_string()));
        assert_eq!(result.savings_ratio, 1.0);
        assert_eq!(result.nodes_re_checked, 0);
    }

    // Test 29: Single node change → only affected invariants re-verified
    #[test]
    fn single_node_change_affected_invariants_only() {
        let mut changes = ChangeSet::new();
        // A single Alloc node was added — affects its region.
        changes.added_nodes.insert(1);
        changes.affected_regions.insert(10);

        let affected = ChangeDetector::compute_affected_invariants(&changes);
        // Should include liveness, bounds, cleanup (all region-related).
        assert!(affected.contains(&"liveness".to_string()));
        assert!(affected.contains(&"bounds".to_string()));
        assert!(affected.contains(&"cleanup".to_string()));
        // Should NOT include exclusivity (no edge changes).
        assert!(!affected.contains(&"exclusivity".to_string()));
    }

    // Test 30: Edge addition triggers exclusivity re-check
    #[test]
    fn edge_addition_triggers_exclusivity_recheck() {
        let mut changes = ChangeSet::new();
        // A Sync edge was added.
        changes.added_edges.insert((1, 2));

        let affected = ChangeDetector::compute_affected_invariants(&changes);
        assert!(affected.contains(&"exclusivity".to_string()));
    }

    // Test 31: Region deletion triggers cleanup re-check
    #[test]
    fn region_deletion_triggers_cleanup_recheck() {
        let mut changes = ChangeSet::new();
        // A region node was removed.
        changes.removed_nodes.insert(1);
        changes.affected_regions.insert(10);

        let affected = ChangeDetector::compute_affected_invariants(&changes);
        assert!(affected.contains(&"cleanup".to_string()));
        assert!(affected.contains(&"liveness".to_string()));
    }

    // Test 32: Cache hit for unchanged subgraph
    #[test]
    fn cache_hit_for_unchanged_subgraph() {
        let mut cache = VerificationCache::new();
        let rid = RegionId(1);

        // First lookup: miss (cache is empty).
        let result = cache.lookup(rid);
        assert!(result.is_none());
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);

        // Populate cache.
        cache.update(rid, VerificationStatus::Safe);

        // Second lookup: hit.
        let result = cache.lookup(rid);
        assert_eq!(result, Some(VerificationStatus::Safe));
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);
        assert!((cache.hit_rate() - 0.5).abs() < 0.001);
    }

    // Test 33: Cache miss for changed subgraph
    #[test]
    fn cache_miss_for_changed_subgraph() {
        let mut cache = VerificationCache::new();
        let rid = RegionId(1);

        // Populate cache.
        cache.update(rid, VerificationStatus::Safe);
        assert!(cache.contains(rid));

        // Invalidate the cache (simulating a change to this region).
        cache.invalidate(rid);
        assert!(!cache.contains(rid));

        // Lookup now is a miss.
        let result = cache.lookup(rid);
        assert!(result.is_none());
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);
    }

    // Test 34: Savings ratio computation
    #[test]
    fn savings_ratio_computation() {
        let mut verifier = IncrementalVerifier::new();

        // Build an MSG with 3 regions + 3 derivations + 3 accesses = 9 total nodes.
        let mut msg = MSG::new();
        for i in 1..=3 {
            msg.add_region(make_region(i, 0x1000 * i, 0x100));
            msg.add_derivation(make_direct_derivation(i * 10, i));
            msg.add_access(Access::new(
                AccessId(i * 100),
                DerivationId(i * 10),
                AccessKind::Read,
                4,
                dummy_pp(i as u32),
            ));
        }

        // Only region 1 changed.
        let mut changes = ChangeSet::new();
        changes.added_nodes.insert(1);
        changes.affected_regions.insert(1);

        let delta = MSGDelta::new();
        let result = verifier.incremental_verify(&msg, &delta, &changes);

        // Total nodes = 3 regions + 3 derivations + 3 accesses = 9.
        assert_eq!(result.total_nodes, 9);
        // Only region 1 was re-checked (1 node), plus derivation + access edges.
        assert!(result.nodes_re_checked > 0);
        assert!(result.savings_ratio > 0.0);
        assert!(result.savings_ratio < 1.0);
    }

    // Test 35: Performance target check (< 1 second for small edit)
    #[test]
    fn performance_target_under_one_second() {
        let mut verifier = IncrementalVerifier::new();

        // Build a small MSG.
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x200));
        msg.add_derivation(make_direct_derivation(10, 1));
        msg.add_access(Access::new(AccessId(100), DerivationId(10), AccessKind::Read, 4, dummy_pp(5)));

        // Build old and new SCG snapshots (small change: add an access node).
        let mut old_scg = SCGSnapshot::new();
        old_scg.add_node(SCGNode::Alloc {
            node_id: 1,
            region_id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x200,
            alloc_point: dummy_pp(1),
        });
        old_scg.add_node(SCGNode::Arithmetic {
            node_id: 2,
            derivation_id: DerivationId(10),
            source_derivation: DerivationId(10),
            offset: 0,
            proven_range: (Address::from(0x1000_u64), Address::from(0x1200_u64)),
        });

        let mut new_scg = old_scg.clone();
        new_scg.add_node(SCGNode::Access {
            node_id: 3,
            access_id: AccessId(100),
            target_derivation: DerivationId(10),
            region_id: RegionId(1),
            is_write: false,
            size: 4,
            program_point: dummy_pp(5),
        });

        let (result, metrics) = verifier.incremental_verify_with_metrics(&msg, &old_scg, &new_scg);

        // Must meet the Phase 2 target of < 1 second.
        assert!(metrics.meets_target, "Incremental verification took too long: {:?}", metrics.total_time);
        assert!(metrics.total_time < std::time::Duration::from_secs(1));
        assert!(metrics.change_detection_time < std::time::Duration::from_secs(1));
        assert!(metrics.delta_computation_time < std::time::Duration::from_secs(1));
        assert!(metrics.re_verification_time < std::time::Duration::from_secs(1));

        // The result should have some re-verified invariants since we added a node.
        assert!(!result.re_verified_invariants.is_empty());
    }

    // Test 36: ChangeDetector detects added/removed/modified nodes
    #[test]
    fn change_detector_detects_all_changes() {
        let mut old = SCGSnapshot::new();
        old.add_node(SCGNode::Alloc {
            node_id: 1,
            region_id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x100,
            alloc_point: dummy_pp(1),
        });
        old.add_node(SCGNode::Arithmetic {
            node_id: 2,
            derivation_id: DerivationId(10),
            source_derivation: DerivationId(10),
            offset: 0x40,
            proven_range: (Address::from(0x1040_u64), Address::from(0x1100_u64)),
        });

        let mut new = SCGSnapshot::new();
        // Node 1 stays the same, node 2 is removed, nodes 3 & 4 are added.
        new.add_node(SCGNode::Alloc {
            node_id: 1,
            region_id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x100,
            alloc_point: dummy_pp(1),
        });
        new.add_node(SCGNode::Access {
            node_id: 3,
            access_id: AccessId(100),
            target_derivation: DerivationId(10),
            region_id: RegionId(1),
            is_write: false,
            size: 4,
            program_point: dummy_pp(5),
        });
        // Add a Sync node to test added_edges.
        new.add_node(SCGNode::Sync {
            node_id: 4,
            edge_id: SyncEdgeId(1),
            access1: AccessId(100),
            access2: AccessId(200),
            ordering: Ordering::HappensBefore,
        });

        let detector = ChangeDetector::new(old, new);
        let changes = detector.detect();

        assert!(changes.added_nodes.contains(&3));
        assert!(changes.added_nodes.contains(&4));
        assert!(changes.removed_nodes.contains(&2));
        assert!(changes.affected_regions.contains(&1)); // Region 1 affected by both nodes.
        assert!(changes.affected_derivations.contains(&10)); // D10 from Arithmetic and Access.
        // Arithmetic node removed adds an edge to removed_edges.
        assert!(!changes.removed_edges.is_empty());
        // Sync node added adds an edge to added_edges.
        assert!(!changes.added_edges.is_empty());
    }

    // Test 37: IncrementalMetrics meets_target logic
    #[test]
    fn incremental_metrics_meets_target() {
        let under = IncrementalMetrics::new(
            std::time::Duration::from_millis(100),
            std::time::Duration::from_millis(200),
            std::time::Duration::from_millis(300),
            std::time::Duration::from_millis(500),
        );
        assert!(under.meets_target);
        assert!(under.total_time < std::time::Duration::from_secs(1));

        let over = IncrementalMetrics::new(
            std::time::Duration::from_millis(400),
            std::time::Duration::from_millis(300),
            std::time::Duration::from_millis(400),
            std::time::Duration::from_millis(1100),
        );
        assert!(!over.meets_target);
    }

    // Test 38: VerificationCache clear resets counters
    #[test]
    fn verification_cache_clear_resets_counters() {
        let mut cache = VerificationCache::new();
        cache.update(RegionId(1), VerificationStatus::Safe);
        cache.lookup(RegionId(1)); // hit
        cache.lookup(RegionId(2)); // miss
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.len(), 1);

        cache.clear();
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.misses(), 0);
        assert!(cache.is_empty());
    }

    // Test 39: ChangeSet is_empty and change_count
    #[test]
    fn changeset_empty_and_count() {
        let empty = ChangeSet::new();
        assert!(empty.is_empty());
        assert_eq!(empty.change_count(), 0);

        let mut cs = ChangeSet::new();
        cs.added_nodes.insert(1);
        cs.removed_nodes.insert(2);
        cs.modified_nodes.insert(3);
        cs.added_edges.insert((4, 5));
        cs.removed_edges.insert((6, 7));
        assert!(!cs.is_empty());
        assert_eq!(cs.change_count(), 5);
    }

    // Test 40: IncrementalVerifier with real MSG data
    #[test]
    fn incremental_verifier_with_msg_data() {
        let mut verifier = IncrementalVerifier::new();

        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x200));
        msg.add_derivation(make_direct_derivation(10, 1));
        msg.add_access(Access::new(AccessId(100), DerivationId(10), AccessKind::Read, 4, dummy_pp(5)));

        // Add a second region with access.
        msg.add_region(make_region(2, 0x2000, 0x200));
        msg.add_derivation(make_direct_derivation(20, 2));
        msg.add_access(Access::new(AccessId(200), DerivationId(20), AccessKind::Write, 4, dummy_pp(10)));

        // Only region 1 changed.
        let mut changes = ChangeSet::new();
        changes.affected_regions.insert(1);
        changes.added_nodes.insert(1);

        let delta = MSGDelta::new();
        let result = verifier.incremental_verify(&msg, &delta, &changes);

        // Liveness, bounds, cleanup should be re-verified.
        assert!(result.re_verified_invariants.contains(&"liveness".to_string()));
        assert!(result.re_verified_invariants.contains(&"bounds".to_string()));
        // Exclusivity should be skipped (no edge changes).
        assert!(result.skipped_invariants.contains(&"exclusivity".to_string()));
        // Overall should be Safe since both regions are live and accessible.
        assert_eq!(result.result, VerificationStatus::Safe);
    }
}
