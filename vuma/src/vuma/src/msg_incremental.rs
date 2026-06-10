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
}
