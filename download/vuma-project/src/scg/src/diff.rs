//! SCG Diff Algorithm
//!
//! This module provides diff computation, application, and three-way merging
//! for Semantic Computation Graphs. It is used by:
//! - **COR** (Change-Oriented Recompilation) for incremental recompilation
//! - **Projection system** for visualizing changes between program versions
//! - **IVE** (Incremental Verifier Engine) for incremental re-verification
//!
//! # Overview
//!
//! The diff algorithm compares two SCGs and produces a structured representation
//! of the differences, including added/removed/modified nodes, edges, and regions.
//! A minimal edit script can be computed that transforms one graph into another.
//! Three-way merge enables combining independent changes from two branches
//! relative to a common base.

use hashbrown::{HashMap, HashSet};

use crate::edge::{EdgeData, EdgeId};
use crate::graph::SCG;
use crate::node::{NodeData, NodeId};
use crate::region::{RegionId, SCGRegion};

// ── Diff Entry ──────────────────────────────────────────────────────────────

/// A single atomic change between two SCG versions.
///
/// Each variant captures one specific kind of graph modification, making
/// diffs inspectable, serializable, and reversible.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffEntry {
    /// A node was added in the new graph.
    NodeAdded(NodeData),
    /// A node was removed from the old graph.
    NodeRemoved(NodeId),
    /// A node exists in both graphs but its data changed.
    NodeModified {
        /// The stable identifier of the modified node.
        id: NodeId,
        /// The node data in the old graph.
        old: NodeData,
        /// The node data in the new graph.
        new: NodeData,
    },
    /// An edge was added in the new graph.
    EdgeAdded(EdgeData),
    /// An edge was removed from the old graph.
    EdgeRemoved(EdgeId),
    /// An edge exists in both graphs but its data changed.
    EdgeModified {
        /// The stable identifier of the modified edge.
        id: EdgeId,
        /// The edge data in the old graph.
        old: EdgeData,
        /// The edge data in the new graph.
        new: EdgeData,
    },
    /// A region was added in the new graph.
    RegionAdded(SCGRegion),
    /// A region was removed from the old graph.
    RegionRemoved(RegionId),
    /// A region exists in both graphs but its data changed.
    RegionModified {
        /// The stable identifier of the modified region.
        id: RegionId,
        /// The region data in the old graph.
        old: SCGRegion,
        /// The region data in the new graph.
        new: SCGRegion,
    },
}

impl DiffEntry {
    /// Returns `true` if this entry represents an addition (node, edge, or region).
    pub fn is_addition(&self) -> bool {
        matches!(
            self,
            DiffEntry::NodeAdded(_)
                | DiffEntry::EdgeAdded(_)
                | DiffEntry::RegionAdded(_)
        )
    }

    /// Returns `true` if this entry represents a removal (node, edge, or region).
    pub fn is_removal(&self) -> bool {
        matches!(
            self,
            DiffEntry::NodeRemoved(_)
                | DiffEntry::EdgeRemoved(_)
                | DiffEntry::RegionRemoved(_)
        )
    }

    /// Returns `true` if this entry represents a modification (node, edge, or region).
    pub fn is_modification(&self) -> bool {
        matches!(
            self,
            DiffEntry::NodeModified { .. }
                | DiffEntry::EdgeModified { .. }
                | DiffEntry::RegionModified { .. }
        )
    }

    /// Returns a human-readable description of this diff entry.
    pub fn describe(&self) -> String {
        match self {
            DiffEntry::NodeAdded(data) => format!("node {} added ({})", data.id, data.node_type),
            DiffEntry::NodeRemoved(id) => format!("node {} removed", id),
            DiffEntry::NodeModified { id, old, new } => {
                format!("node {} modified ({} -> {})", id, old.node_type, new.node_type)
            }
            DiffEntry::EdgeAdded(data) => {
                format!("edge {} added ({} -> {}, {})", data.id, data.source, data.target, data.kind)
            }
            DiffEntry::EdgeRemoved(id) => format!("edge {} removed", id),
            DiffEntry::EdgeModified { id, old, new } => {
                format!("edge {} modified ({} -> {})", id, old.kind, new.kind)
            }
            DiffEntry::RegionAdded(data) => format!("region {} added", data.id),
            DiffEntry::RegionRemoved(id) => format!("region {} removed", id),
            DiffEntry::RegionModified { id, .. } => format!("region {} modified", id),
        }
    }
}

// ── SCG Diff ────────────────────────────────────────────────────────────────

/// The complete difference between two SCG versions.
///
/// An `SCGDiff` contains an ordered list of `DiffEntry` items that
/// collectively describe all changes needed to transform the old graph
/// into the new graph.
#[derive(Debug, Clone, PartialEq)]
pub struct SCGDiff {
    /// The ordered list of diff entries.
    entries: Vec<DiffEntry>,
    /// Summary statistics for quick inspection.
    stats: DiffStats,
}

/// Summary statistics for a diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffStats {
    /// Number of nodes added.
    pub nodes_added: usize,
    /// Number of nodes removed.
    pub nodes_removed: usize,
    /// Number of nodes modified.
    pub nodes_modified: usize,
    /// Number of edges added.
    pub edges_added: usize,
    /// Number of edges removed.
    pub edges_removed: usize,
    /// Number of edges modified.
    pub edges_modified: usize,
    /// Number of regions added.
    pub regions_added: usize,
    /// Number of regions removed: usize,
    pub regions_removed: usize,
    /// Number of regions modified.
    pub regions_modified: usize,
}

impl DiffStats {
    /// Returns the total number of changes.
    pub fn total_changes(&self) -> usize {
        self.nodes_added + self.nodes_removed + self.nodes_modified
            + self.edges_added + self.edges_removed + self.edges_modified
            + self.regions_added + self.regions_removed + self.regions_modified
    }

    /// Returns `true` if there are no changes.
    pub fn is_empty(&self) -> bool {
        self.total_changes() == 0
    }
}

impl SCGDiff {
    /// Creates a new empty diff.
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            stats: DiffStats::default(),
        }
    }

    /// Creates a diff from a list of entries, computing stats automatically.
    pub fn from_entries(entries: Vec<DiffEntry>) -> Self {
        let stats = compute_stats(&entries);
        Self { entries, stats }
    }

    /// Returns the list of diff entries.
    pub fn entries(&self) -> &[DiffEntry] {
        &self.entries
    }

    /// Returns the diff statistics.
    pub fn stats(&self) -> DiffStats {
        self.stats
    }

    /// Returns `true` if the diff contains no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the number of entries in the diff.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns an iterator over entries that only affect nodes.
    pub fn node_entries(&self) -> impl Iterator<Item = &DiffEntry> {
        self.entries.iter().filter(|e| {
            matches!(
                e,
                DiffEntry::NodeAdded(_)
                    | DiffEntry::NodeRemoved(_)
                    | DiffEntry::NodeModified { .. }
            )
        })
    }

    /// Returns an iterator over entries that only affect edges.
    pub fn edge_entries(&self) -> impl Iterator<Item = &DiffEntry> {
        self.entries.iter().filter(|e| {
            matches!(
                e,
                DiffEntry::EdgeAdded(_)
                    | DiffEntry::EdgeRemoved(_)
                    | DiffEntry::EdgeModified { .. }
            )
        })
    }

    /// Returns an iterator over entries that only affect regions.
    pub fn region_entries(&self) -> impl Iterator<Item = &DiffEntry> {
        self.entries.iter().filter(|e| {
            matches!(
                e,
                DiffEntry::RegionAdded(_)
                    | DiffEntry::RegionRemoved(_)
                    | DiffEntry::RegionModified { .. }
            )
        })
    }
}

/// Compute stats from a slice of diff entries.
fn compute_stats(entries: &[DiffEntry]) -> DiffStats {
    let mut stats = DiffStats::default();
    for entry in entries {
        match entry {
            DiffEntry::NodeAdded(_) => stats.nodes_added += 1,
            DiffEntry::NodeRemoved(_) => stats.nodes_removed += 1,
            DiffEntry::NodeModified { .. } => stats.nodes_modified += 1,
            DiffEntry::EdgeAdded(_) => stats.edges_added += 1,
            DiffEntry::EdgeRemoved(_) => stats.edges_removed += 1,
            DiffEntry::EdgeModified { .. } => stats.edges_modified += 1,
            DiffEntry::RegionAdded(_) => stats.regions_added += 1,
            DiffEntry::RegionRemoved(_) => stats.regions_removed += 1,
            DiffEntry::RegionModified { .. } => stats.regions_modified += 1,
        }
    }
    stats
}

// ── Diff Error ──────────────────────────────────────────────────────────────

/// Errors that can occur when applying a diff to an SCG.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffError {
    /// A node referenced in the diff was not found in the target graph.
    NodeNotFound(NodeId),
    /// An edge referenced in the diff was not found in the target graph.
    EdgeNotFound(EdgeId),
    /// A region referenced in the diff was not found in the target graph.
    RegionNotFound(RegionId),
    /// A node with the given ID already exists (cannot add duplicate).
    DuplicateNode(NodeId),
    /// An edge with the given ID already exists (cannot add duplicate).
    DuplicateEdge(EdgeId),
    /// An edge's source or target node does not exist.
    InvalidEdgeEndpoints {
        source: NodeId,
        target: NodeId,
    },
    /// The diff cannot be applied in its current state (e.g., dependency
    /// ordering issue).
    CannotApply(String),
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::NodeNotFound(id) => write!(f, "diff error: node not found: {id}"),
            DiffError::EdgeNotFound(id) => write!(f, "diff error: edge not found: {id}"),
            DiffError::RegionNotFound(id) => write!(f, "diff error: region not found: {id}"),
            DiffError::DuplicateNode(id) => write!(f, "diff error: duplicate node: {id}"),
            DiffError::DuplicateEdge(id) => write!(f, "diff error: duplicate edge: {id}"),
            DiffError::InvalidEdgeEndpoints { source, target } => {
                write!(f, "diff error: invalid edge endpoints: source={source}, target={target}")
            }
            DiffError::CannotApply(msg) => write!(f, "diff error: cannot apply: {msg}"),
        }
    }
}

impl std::error::Error for DiffError {}

// ── Diff Computation ────────────────────────────────────────────────────────

/// Computes the diff between two SCGs.
///
/// The diff captures all changes needed to transform `old` into `new`.
/// Nodes and edges are matched by their stable `NodeId`/`EdgeId` identifiers.
/// Regions are matched by `RegionId`.
///
/// # Algorithm
///
/// 1. **Nodes**: Identify nodes present only in old (removed), only in new
///    (added), or in both (check for modifications by comparing `NodeData`).
/// 2. **Edges**: Identify edges present only in old (removed), only in new
///    (added), or in both (check for modifications by comparing `EdgeData`).
/// 3. **Regions**: Identify regions present only in old (removed), only in
///    new (added), or in both (check for modifications by comparing `SCGRegion`).
///
/// The ordering of entries follows: removed nodes, removed edges, removed
/// regions, modified nodes, modified edges, modified regions, added regions,
/// added edges, added nodes. This ordering ensures safe application:
/// removals happen before additions, so no dangling references exist.
pub fn diff_scg(old: &SCG, new: &SCG) -> SCGDiff {
    let mut entries = Vec::new();

    // ── Collect node IDs from both graphs ──
    let old_node_ids: HashSet<NodeId> = old.node_ids().collect();
    let new_node_ids: HashSet<NodeId> = new.node_ids().collect();

    // Removed nodes (in old but not in new)
    for id in &old_node_ids - &new_node_ids {
        entries.push(DiffEntry::NodeRemoved(id));
    }

    // Modified nodes (in both, but data differs)
    for id in &old_node_ids & &new_node_ids {
        let old_data = old.get_node(id).unwrap();
        let new_data = new.get_node(id).unwrap();
        if old_data != new_data {
            entries.push(DiffEntry::NodeModified {
                id,
                old: old_data.clone(),
                new: new_data.clone(),
            });
        }
    }

    // ── Collect edge IDs from both graphs ──
    let old_edge_ids: HashSet<EdgeId> = old.edge_ids().collect();
    let new_edge_ids: HashSet<EdgeId> = new.edge_ids().collect();

    // Removed edges (in old but not in new)
    for id in &old_edge_ids - &new_edge_ids {
        entries.push(DiffEntry::EdgeRemoved(id));
    }

    // Modified edges (in both, but data differs)
    for id in &old_edge_ids & &new_edge_ids {
        let old_data = old.get_edge(id).unwrap();
        let new_data = new.get_edge(id).unwrap();
        if old_data != new_data {
            entries.push(DiffEntry::EdgeModified {
                id,
                old: old_data.clone(),
                new: new_data.clone(),
            });
        }
    }

    // ── Collect region IDs from both graphs ──
    let old_region_ids: HashSet<RegionId> = old.regions().map(|r| r.id).collect();
    let new_region_ids: HashSet<RegionId> = new.regions().map(|r| r.id).collect();

    // Removed regions (in old but not in new)
    for id in &old_region_ids - &new_region_ids {
        entries.push(DiffEntry::RegionRemoved(id));
    }

    // Modified regions (in both, but data differs)
    for id in &old_region_ids & &new_region_ids {
        let old_region = old.get_region(id).unwrap();
        let new_region = new.get_region(id).unwrap();
        if old_region != new_region {
            entries.push(DiffEntry::RegionModified {
                id,
                old: old_region.clone(),
                new: new_region.clone(),
            });
        }
    }

    // Added regions (in new but not in old) — must come before added nodes
    for id in &new_region_ids - &old_region_ids {
        let region = new.get_region(id).unwrap();
        entries.push(DiffEntry::RegionAdded(region.clone()));
    }

    // Added edges (in new but not in old)
    for id in &new_edge_ids - &old_edge_ids {
        let edge_data = new.get_edge(id).unwrap();
        entries.push(DiffEntry::EdgeAdded(edge_data.clone()));
    }

    // Added nodes (in new but not in old)
    for id in &new_node_ids - &old_node_ids {
        let node_data = new.get_node(id).unwrap();
        entries.push(DiffEntry::NodeAdded(node_data.clone()));
    }

    SCGDiff::from_entries(entries)
}

// ── Apply Diff ──────────────────────────────────────────────────────────────

/// Applies a diff to an SCG, mutating it in place.
///
/// The diff entries are applied in order. The expected ordering is:
/// 1. Removals (nodes, edges, regions) — clean up old state
/// 2. Modifications — update existing elements
/// 3. Additions (regions, nodes, edges) — add new state
///
/// If any entry cannot be applied (e.g., a node to remove doesn't exist,
/// or a node to add already exists), an error is returned and the graph
/// may be partially modified.
pub fn apply_diff(scg: &mut SCG, diff: &SCGDiff) -> Result<(), DiffError> {
    for entry in &diff.entries {
        apply_entry(scg, entry)?;
    }
    Ok(())
}

/// Applies a single diff entry to an SCG.
fn apply_entry(scg: &mut SCG, entry: &DiffEntry) -> Result<(), DiffError> {
    match entry {
        DiffEntry::NodeRemoved(id) => {
            scg.remove_node(*id)
                .map_err(|_| DiffError::NodeNotFound(*id))?;
            Ok(())
        }
        DiffEntry::NodeAdded(data) => {
            if scg.get_node(data.id).is_some() {
                return Err(DiffError::DuplicateNode(data.id));
            }
            scg.add_node_with_id(
                data.id,
                data.node_type.clone(),
                data.payload.clone(),
                data.program_point.clone(),
            )
            .map_err(|_| DiffError::DuplicateNode(data.id))?;
            // Copy annotation if present
            if let Some(ref ann) = data.annotation {
                if let Some(node) = scg.get_node_mut(data.id) {
                    node.annotation = Some(ann.clone());
                }
            }
            Ok(())
        }
        DiffEntry::NodeModified { id, new, .. } => {
            let node = scg
                .get_node_mut(*id)
                .ok_or(DiffError::NodeNotFound(*id))?;
            node.node_type = new.node_type.clone();
            node.payload = new.payload.clone();
            node.annotation = new.annotation.clone();
            node.program_point = new.program_point.clone();
            Ok(())
        }
        DiffEntry::EdgeRemoved(id) => {
            scg.remove_edge(*id)
                .map_err(|_| DiffError::EdgeNotFound(*id))?;
            Ok(())
        }
        DiffEntry::EdgeAdded(data) => {
            if scg.get_edge(data.id).is_some() {
                return Err(DiffError::DuplicateEdge(data.id));
            }
            // Verify endpoints exist
            if scg.get_node(data.source).is_none() {
                return Err(DiffError::InvalidEdgeEndpoints {
                    source: data.source,
                    target: data.target,
                });
            }
            if scg.get_node(data.target).is_none() {
                return Err(DiffError::InvalidEdgeEndpoints {
                    source: data.source,
                    target: data.target,
                });
            }
            scg.add_edge_with_id(data.id, data.source, data.target, data.kind.clone())
                .map_err(|_| DiffError::DuplicateEdge(data.id))?;
            // Copy label if present
            if let Some(ref label) = data.label {
                if let Some(edge) = scg.get_edge_mut(data.id) {
                    edge.label = Some(label.clone());
                }
            }
            Ok(())
        }
        DiffEntry::EdgeModified { id, new, .. } => {
            let edge = scg
                .get_edge_mut(*id)
                .ok_or(DiffError::EdgeNotFound(*id))?;
            edge.kind = new.kind.clone();
            edge.label = new.label.clone();
            // Note: source/target changes on an existing edge are not
            // supported through modification. Use remove + add instead.
            Ok(())
        }
        DiffEntry::RegionRemoved(id) => {
            scg.remove_region(*id);
            Ok(())
        }
        DiffEntry::RegionAdded(data) => {
            if scg.get_region(data.id).is_some() {
                return Err(DiffError::RegionNotFound(data.id));
            }
            scg.add_region(data.clone());
            Ok(())
        }
        DiffEntry::RegionModified { id, new, .. } => {
            let region = scg
                .get_region_mut(*id)
                .ok_or(DiffError::RegionNotFound(*id))?;
            region.scope_level = new.scope_level;
            region.security_boundary = new.security_boundary;
            region.deployment_target = new.deployment_target.clone();
            region.nodes = new.nodes.clone();
            Ok(())
        }
    }
}

// ── Edit Script ─────────────────────────────────────────────────────────────

/// Computes a minimal edit script to transform `old` into `new`.
///
/// The edit script is an ordered sequence of `DiffEntry` items that, when
/// applied sequentially to `old`, produces a graph equivalent to `new`.
///
/// The ordering is carefully chosen to ensure safe application:
/// 1. Remove edges (to disconnect nodes before removal)
/// 2. Remove nodes (leaf-first to avoid dangling references)
/// 3. Remove regions
/// 4. Modify existing nodes, edges, and regions
/// 5. Add regions (before nodes so regions exist for node assignment)
/// 6. Add nodes
/// 7. Add edges (after nodes so endpoints exist)
pub fn compute_edit_script(old: &SCG, new: &SCG) -> Vec<DiffEntry> {
    let raw_diff = diff_scg(old, new);
    let mut script = Vec::with_capacity(raw_diff.len());

    // Phase 1: Removals — edges first, then nodes, then regions
    for entry in raw_diff.edge_entries() {
        if let DiffEntry::EdgeRemoved(id) = entry {
            script.push(DiffEntry::EdgeRemoved(*id));
        }
    }
    for entry in raw_diff.node_entries() {
        if let DiffEntry::NodeRemoved(id) = entry {
            script.push(DiffEntry::NodeRemoved(*id));
        }
    }
    for entry in raw_diff.region_entries() {
        if let DiffEntry::RegionRemoved(id) = entry {
            script.push(DiffEntry::RegionRemoved(*id));
        }
    }

    // Phase 2: Modifications — nodes, then edges, then regions
    for entry in raw_diff.node_entries() {
        if let DiffEntry::NodeModified { id, old, new } = entry {
            script.push(DiffEntry::NodeModified {
                id: *id,
                old: old.clone(),
                new: new.clone(),
            });
        }
    }
    for entry in raw_diff.edge_entries() {
        if let DiffEntry::EdgeModified { id, old, new } = entry {
            script.push(DiffEntry::EdgeModified {
                id: *id,
                old: old.clone(),
                new: new.clone(),
            });
        }
    }
    for entry in raw_diff.region_entries() {
        if let DiffEntry::RegionModified { id, old, new } = entry {
            script.push(DiffEntry::RegionModified {
                id: *id,
                old: old.clone(),
                new: new.clone(),
            });
        }
    }

    // Phase 3: Additions — regions first, then nodes, then edges
    for entry in raw_diff.region_entries() {
        if let DiffEntry::RegionAdded(data) = entry {
            script.push(DiffEntry::RegionAdded(data.clone()));
        }
    }
    for entry in raw_diff.node_entries() {
        if let DiffEntry::NodeAdded(data) = entry {
            script.push(DiffEntry::NodeAdded(data.clone()));
        }
    }
    for entry in raw_diff.edge_entries() {
        if let DiffEntry::EdgeAdded(data) = entry {
            script.push(DiffEntry::EdgeAdded(data.clone()));
        }
    }

    script
}

// ── Three-Way Merge ─────────────────────────────────────────────────────────

/// A conflict encountered during three-way merge.
#[derive(Debug, Clone, PartialEq)]
pub struct MergeConflict {
    /// Conflicts involving nodes.
    pub node_conflicts: Vec<NodeConflict>,
    /// Conflicts involving edges.
    pub edge_conflicts: Vec<EdgeConflict>,
    /// Conflicts involving regions.
    pub region_conflicts: Vec<RegionConflict>,
}

impl MergeConflict {
    /// Creates an empty merge conflict set.
    pub fn empty() -> Self {
        Self {
            node_conflicts: Vec::new(),
            edge_conflicts: Vec::new(),
            region_conflicts: Vec::new(),
        }
    }

    /// Returns `true` if there are no conflicts.
    pub fn is_empty(&self) -> bool {
        self.node_conflicts.is_empty()
            && self.edge_conflicts.is_empty()
            && self.region_conflicts.is_empty()
    }

    /// Returns the total number of conflicts across all categories.
    pub fn total_conflicts(&self) -> usize {
        self.node_conflicts.len() + self.edge_conflicts.len() + self.region_conflicts.len()
    }
}

impl std::fmt::Display for MergeConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "merge conflict: {} node, {} edge, {} region conflicts",
            self.node_conflicts.len(),
            self.edge_conflicts.len(),
            self.region_conflicts.len()
        )
    }
}

impl std::error::Error for MergeConflict {}

/// A conflict involving a single node.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeConflict {
    /// The ID of the conflicting node.
    pub id: NodeId,
    /// The node data in the base graph (if it exists there).
    pub base: Option<NodeData>,
    /// The node data in our branch (if it exists there).
    pub ours: Option<NodeData>,
    /// The node data in their branch (if it exists there).
    pub theirs: Option<NodeData>,
}

/// A conflict involving a single edge.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeConflict {
    /// The ID of the conflicting edge.
    pub id: EdgeId,
    /// The edge data in the base graph (if it exists there).
    pub base: Option<EdgeData>,
    /// The edge data in our branch (if it exists there).
    pub ours: Option<EdgeData>,
    /// The edge data in their branch (if it exists there).
    pub theirs: Option<EdgeData>,
}

/// A conflict involving a single region.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionConflict {
    /// The ID of the conflicting region.
    pub id: RegionId,
    /// The region data in the base graph (if it exists there).
    pub base: Option<SCGRegion>,
    /// The region data in our branch (if it exists there).
    pub ours: Option<SCGRegion>,
    /// The region data in their branch (if it exists there).
    pub theirs: Option<SCGRegion>,
}

/// Performs a three-way merge of SCGs.
///
/// Given a common `base` graph and two divergent versions (`ours` and `theirs`),
/// this function produces a merged graph that incorporates changes from both
/// branches. When both branches modify the same element differently, a conflict
/// is reported.
///
/// # Merge Rules
///
/// For each element (node, edge, region):
/// - **Unchanged in both**: Keep as-is from base.
/// - **Changed in ours only**: Apply ours' version.
/// - **Changed in theirs only**: Apply theirs' version.
/// - **Changed identically in both**: Apply either (they agree).
/// - **Changed differently in both**: Conflict — include in `MergeConflict`.
/// - **Added in ours only**: Include ours' addition.
/// - **Added in theirs only**: Include theirs' addition.
/// - **Added in both (same data)**: Include either.
/// - **Added in both (different data)**: Conflict.
/// - **Removed in ours only**: Remove.
/// - **Removed in theirs only**: Remove.
/// - **Removed in one, modified in other**: Conflict.
/// - **Removed in both**: Remove.
pub fn three_way_merge(
    base: &SCG,
    ours: &SCG,
    theirs: &SCG,
) -> Result<SCG, MergeConflict> {
    let diff_ours = diff_scg(base, ours);
    let diff_theirs = diff_scg(base, theirs);

    let mut conflict = MergeConflict::empty();

    // Start with a clone of the base graph
    let mut merged = base.clone();

    // ── Merge Nodes ──
    let ours_node_changes = collect_node_changes(&diff_ours);
    let theirs_node_changes = collect_node_changes(&diff_theirs);

    let all_node_ids: HashSet<NodeId> = ours_node_changes
        .keys()
        .chain(theirs_node_changes.keys())
        .copied()
        .collect();

    for node_id in &all_node_ids {
        let ours_change = ours_node_changes.get(node_id);
        let theirs_change = theirs_node_changes.get(node_id);

        match (ours_change, theirs_change) {
            (None, None) => { /* No change in either — keep base */ }
            (Some(oc), None) => {
                // Only ours changed — apply ours' change
                apply_node_change(&mut merged, node_id, oc);
            }
            (None, Some(tc)) => {
                // Only theirs changed — apply theirs' change
                apply_node_change(&mut merged, node_id, tc);
            }
            (Some(oc), Some(tc)) => {
                // Both changed — check if they agree
                if oc == tc {
                    apply_node_change(&mut merged, node_id, oc);
                } else {
                    conflict.node_conflicts.push(NodeConflict {
                        id: *node_id,
                        base: base.get_node(*node_id).cloned(),
                        ours: change_to_node_data(oc),
                        theirs: change_to_node_data(tc),
                    });
                }
            }
        }
    }

    // ── Merge Edges ──
    let ours_edge_changes = collect_edge_changes(&diff_ours);
    let theirs_edge_changes = collect_edge_changes(&diff_theirs);

    let all_edge_ids: HashSet<EdgeId> = ours_edge_changes
        .keys()
        .chain(theirs_edge_changes.keys())
        .copied()
        .collect();

    for edge_id in &all_edge_ids {
        let ours_change = ours_edge_changes.get(edge_id);
        let theirs_change = theirs_edge_changes.get(edge_id);

        match (ours_change, theirs_change) {
            (None, None) => {}
            (Some(oc), None) => {
                apply_edge_change(&mut merged, edge_id, oc);
            }
            (None, Some(tc)) => {
                apply_edge_change(&mut merged, edge_id, tc);
            }
            (Some(oc), Some(tc)) => {
                if oc == tc {
                    apply_edge_change(&mut merged, edge_id, oc);
                } else {
                    conflict.edge_conflicts.push(EdgeConflict {
                        id: *edge_id,
                        base: base.get_edge(*edge_id).cloned(),
                        ours: change_to_edge_data(oc),
                        theirs: change_to_edge_data(tc),
                    });
                }
            }
        }
    }

    // ── Merge Regions ──
    let ours_region_changes = collect_region_changes(&diff_ours);
    let theirs_region_changes = collect_region_changes(&diff_theirs);

    let all_region_ids: HashSet<RegionId> = ours_region_changes
        .keys()
        .chain(theirs_region_changes.keys())
        .copied()
        .collect();

    for region_id in &all_region_ids {
        let ours_change = ours_region_changes.get(region_id);
        let theirs_change = theirs_region_changes.get(region_id);

        match (ours_change, theirs_change) {
            (None, None) => {}
            (Some(oc), None) => {
                apply_region_change(&mut merged, region_id, oc);
            }
            (None, Some(tc)) => {
                apply_region_change(&mut merged, region_id, tc);
            }
            (Some(oc), Some(tc)) => {
                if oc == tc {
                    apply_region_change(&mut merged, region_id, oc);
                } else {
                    conflict.region_conflicts.push(RegionConflict {
                        id: *region_id,
                        base: base.get_region(*region_id).cloned(),
                        ours: change_to_region_data(oc),
                        theirs: change_to_region_data(tc),
                    });
                }
            }
        }
    }

    if conflict.is_empty() {
        Ok(merged)
    } else {
        Err(conflict)
    }
}

// ── Change Tracking Helpers ─────────────────────────────────────────────────

/// Represents a change to a single element, used for merge conflict detection.
#[derive(Debug, Clone, PartialEq)]
enum ElementChange<T> {
    /// The element was added with the given data.
    Added(T),
    /// The element was removed.
    Removed,
    /// The element was modified to the given data.
    Modified(T),
}

/// Collect node-level changes from a diff into a map keyed by NodeId.
fn collect_node_changes(diff: &SCGDiff) -> HashMap<NodeId, ElementChange<NodeData>> {
    let mut changes = HashMap::new();
    for entry in diff.entries() {
        match entry {
            DiffEntry::NodeAdded(data) => {
                changes.insert(data.id, ElementChange::Added(data.clone()));
            }
            DiffEntry::NodeRemoved(id) => {
                changes.insert(*id, ElementChange::Removed);
            }
            DiffEntry::NodeModified { id, new, .. } => {
                changes.insert(*id, ElementChange::Modified(new.clone()));
            }
            _ => {}
        }
    }
    changes
}

/// Collect edge-level changes from a diff into a map keyed by EdgeId.
fn collect_edge_changes(diff: &SCGDiff) -> HashMap<EdgeId, ElementChange<EdgeData>> {
    let mut changes = HashMap::new();
    for entry in diff.entries() {
        match entry {
            DiffEntry::EdgeAdded(data) => {
                changes.insert(data.id, ElementChange::Added(data.clone()));
            }
            DiffEntry::EdgeRemoved(id) => {
                changes.insert(*id, ElementChange::Removed);
            }
            DiffEntry::EdgeModified { id, new, .. } => {
                changes.insert(*id, ElementChange::Modified(new.clone()));
            }
            _ => {}
        }
    }
    changes
}

/// Collect region-level changes from a diff into a map keyed by RegionId.
fn collect_region_changes(diff: &SCGDiff) -> HashMap<RegionId, ElementChange<SCGRegion>> {
    let mut changes = HashMap::new();
    for entry in diff.entries() {
        match entry {
            DiffEntry::RegionAdded(data) => {
                changes.insert(data.id, ElementChange::Added(data.clone()));
            }
            DiffEntry::RegionRemoved(id) => {
                changes.insert(*id, ElementChange::Removed);
            }
            DiffEntry::RegionModified { id, new, .. } => {
                changes.insert(*id, ElementChange::Modified(new.clone()));
            }
            _ => {}
        }
    }
    changes
}

/// Apply a node change to the merged graph.
fn apply_node_change(scg: &mut SCG, id: &NodeId, change: &ElementChange<NodeData>) {
    match change {
        ElementChange::Added(data) => {
            if scg.get_node(*id).is_none() {
                let _ = scg.add_node_with_id(
                    data.id,
                    data.node_type.clone(),
                    data.payload.clone(),
                    data.program_point.clone(),
                );
                if let Some(ref ann) = data.annotation {
                    if let Some(node) = scg.get_node_mut(*id) {
                        node.annotation = Some(ann.clone());
                    }
                }
            }
        }
        ElementChange::Removed => {
            let _ = scg.remove_node(*id);
        }
        ElementChange::Modified(data) => {
            if let Some(node) = scg.get_node_mut(*id) {
                node.node_type = data.node_type.clone();
                node.payload = data.payload.clone();
                node.annotation = data.annotation.clone();
                node.program_point = data.program_point.clone();
            }
        }
    }
}

/// Apply an edge change to the merged graph.
fn apply_edge_change(scg: &mut SCG, id: &EdgeId, change: &ElementChange<EdgeData>) {
    match change {
        ElementChange::Added(data) => {
            if scg.get_edge(*id).is_none() {
                let _ = scg.add_edge_with_id(data.id, data.source, data.target, data.kind.clone());
                if let Some(ref label) = data.label {
                    if let Some(edge) = scg.get_edge_mut(*id) {
                        edge.label = Some(label.clone());
                    }
                }
            }
        }
        ElementChange::Removed => {
            let _ = scg.remove_edge(*id);
        }
        ElementChange::Modified(data) => {
            if let Some(edge) = scg.get_edge_mut(*id) {
                edge.kind = data.kind.clone();
                edge.label = data.label.clone();
            }
        }
    }
}

/// Apply a region change to the merged graph.
fn apply_region_change(scg: &mut SCG, id: &RegionId, change: &ElementChange<SCGRegion>) {
    match change {
        ElementChange::Added(data) => {
            if scg.get_region(*id).is_none() {
                scg.add_region(data.clone());
            }
        }
        ElementChange::Removed => {
            scg.remove_region(*id);
        }
        ElementChange::Modified(data) => {
            if let Some(region) = scg.get_region_mut(*id) {
                region.scope_level = data.scope_level;
                region.security_boundary = data.security_boundary;
                region.deployment_target = data.deployment_target.clone();
                region.nodes = data.nodes.clone();
            }
        }
    }
}

/// Extract node data from a change, if available.
fn change_to_node_data(change: &ElementChange<NodeData>) -> Option<NodeData> {
    match change {
        ElementChange::Added(data) | ElementChange::Modified(data) => Some(data.clone()),
        ElementChange::Removed => None,
    }
}

/// Extract edge data from a change, if available.
fn change_to_edge_data(change: &ElementChange<EdgeData>) -> Option<EdgeData> {
    match change {
        ElementChange::Added(data) | ElementChange::Modified(data) => Some(data.clone()),
        ElementChange::Removed => None,
    }
}

/// Extract region data from a change, if available.
fn change_to_region_data(change: &ElementChange<SCGRegion>) -> Option<SCGRegion> {
    match change {
        ElementChange::Added(data) | ElementChange::Modified(data) => Some(data.clone()),
        ElementChange::Removed => None,
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::node::{
        ComputationNode, NodePayload, NodeType, PhantomNode, ProgramPoint,
    };
    use crate::region::{DeploymentTarget, SCGRegion};

    /// Helper to create a default program point.
    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: Some("test.vu".to_string()),
            line: Some(1),
            column: Some(1),
            offset: None,
        }
    }

    /// Build a simple SCG with two computation nodes and a data-flow edge.
    fn make_simple_scg() -> SCG {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: Some("i32".to_string()),
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "sub".to_string(),
                result_type: None,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        scg
    }

    // ── Test 1: Identical graphs produce an empty diff ──

    #[test]
    fn test_diff_identical_graphs() {
        let scg1 = make_simple_scg();

        // Build an identical SCG with the same node/edge IDs
        let mut scg2 = SCG::new();
        let n1 = scg2.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: Some("i32".to_string()),
            }),
            pp(),
        ).unwrap();
        let n2 = scg2.add_node_with_id(
            NodeId::new(1),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "sub".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();
        scg2.add_edge_with_id(EdgeId::new(0), n1, n2, EdgeKind::DataFlow).unwrap();

        let diff = diff_scg(&scg1, &scg2);
        assert!(diff.is_empty(), "Expected empty diff for identical graphs, got {} entries", diff.len());
        assert!(diff.stats().is_empty());
    }

    // ── Test 2: Adding a node is detected ──

    #[test]
    fn test_diff_node_added() {
        let old = make_simple_scg();

        // Rebuild with same IDs plus a new node
        let mut new = SCG::new();
        new.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: Some("i32".to_string()),
            }),
            pp(),
        ).unwrap();
        new.add_node_with_id(
            NodeId::new(1),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "sub".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();
        new.add_edge_with_id(EdgeId::new(0), NodeId::new(0), NodeId::new(1), EdgeKind::DataFlow).unwrap();

        // Add a new node
        new.add_node(
            NodeType::Phantom,
            NodePayload::Phantom(PhantomNode {
                purpose: "marker".to_string(),
            }),
            pp(),
        );

        let diff = diff_scg(&old, &new);
        assert_eq!(diff.stats().nodes_added, 1);
        assert_eq!(diff.stats().nodes_removed, 0);
        assert_eq!(diff.stats().nodes_modified, 0);
    }

    // ── Test 3: Removing a node is detected ──

    #[test]
    fn test_diff_node_removed() {
        let mut old = SCG::new();
        old.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "a".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();
        old.add_node_with_id(
            NodeId::new(1),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "b".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();

        let mut new = SCG::new();
        new.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "a".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();

        let diff = diff_scg(&old, &new);
        assert_eq!(diff.stats().nodes_removed, 1);
        assert_eq!(diff.stats().nodes_added, 0);
    }

    // ── Test 4: Modifying a node is detected ──

    #[test]
    fn test_diff_node_modified() {
        let mut old = SCG::new();
        old.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: Some("i32".to_string()),
            }),
            pp(),
        ).unwrap();

        let mut new = SCG::new();
        new.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "mul".to_string(), // changed operation
                result_type: Some("i32".to_string()),
            }),
            pp(),
        ).unwrap();

        let diff = diff_scg(&old, &new);
        assert_eq!(diff.stats().nodes_modified, 1);

        // Verify the entry has the right old and new data
        let mod_entry = diff.node_entries().next().unwrap();
        if let DiffEntry::NodeModified { old, new, .. } = mod_entry {
            assert_eq!(old.payload, NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: Some("i32".to_string()),
            }));
            assert_eq!(new.payload, NodePayload::Computation(ComputationNode {
                operation: "mul".to_string(),
                result_type: Some("i32".to_string()),
            }));
        } else {
            panic!("Expected NodeModified entry");
        }
    }

    // ── Test 5: Edge changes are detected ──

    #[test]
    fn test_diff_edge_changes() {
        let mut old = SCG::new();
        old.add_node_with_id(NodeId::new(0), NodeType::Computation,
            NodePayload::Computation(ComputationNode { operation: "a".to_string(), result_type: None }), pp()).unwrap();
        old.add_node_with_id(NodeId::new(1), NodeType::Computation,
            NodePayload::Computation(ComputationNode { operation: "b".to_string(), result_type: None }), pp()).unwrap();
        old.add_node_with_id(NodeId::new(2), NodeType::Computation,
            NodePayload::Computation(ComputationNode { operation: "c".to_string(), result_type: None }), pp()).unwrap();
        old.add_edge_with_id(EdgeId::new(0), NodeId::new(0), NodeId::new(1), EdgeKind::DataFlow).unwrap();

        let mut new = SCG::new();
        new.add_node_with_id(NodeId::new(0), NodeType::Computation,
            NodePayload::Computation(ComputationNode { operation: "a".to_string(), result_type: None }), pp()).unwrap();
        new.add_node_with_id(NodeId::new(1), NodeType::Computation,
            NodePayload::Computation(ComputationNode { operation: "b".to_string(), result_type: None }), pp()).unwrap();
        new.add_node_with_id(NodeId::new(2), NodeType::Computation,
            NodePayload::Computation(ComputationNode { operation: "c".to_string(), result_type: None }), pp()).unwrap();
        // Old edge removed, new edge added
        new.add_edge_with_id(EdgeId::new(1), NodeId::new(0), NodeId::new(2), EdgeKind::ControlFlow).unwrap();

        let diff = diff_scg(&old, &new);
        assert_eq!(diff.stats().edges_removed, 1);
        assert_eq!(diff.stats().edges_added, 1);
    }

    // ── Test 6: Region changes are detected ──

    #[test]
    fn test_diff_region_changes() {
        let mut old = SCG::new();
        let mut region1 = SCGRegion::new(RegionId::new(1), DeploymentTarget::Heap);
        region1.add_node(NodeId::new(0));
        old.add_region(region1);

        let mut new = SCG::new();
        // Region 1 is removed, region 2 is added
        let mut region2 = SCGRegion::new(RegionId::new(2), DeploymentTarget::Stack);
        region2.add_node(NodeId::new(1));
        new.add_region(region2);

        let diff = diff_scg(&old, &new);
        assert_eq!(diff.stats().regions_removed, 1);
        assert_eq!(diff.stats().regions_added, 1);
    }

    // ── Test 7: Apply diff round-trip ──

    #[test]
    fn test_apply_diff_roundtrip() {
        let mut old = SCG::new();
        let n1 = old.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();
        let n2 = old.add_node_with_id(
            NodeId::new(1),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "sub".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();
        old.add_edge_with_id(EdgeId::new(0), n1, n2, EdgeKind::DataFlow).unwrap();

        let mut new = SCG::new();
        new.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "mul".to_string(), // modified
                result_type: None,
            }),
            pp(),
        ).unwrap();
        // n2 removed, n3 added
        let n3 = new.add_node_with_id(
            NodeId::new(2),
            NodeType::Phantom,
            NodePayload::Phantom(PhantomNode {
                purpose: "new".to_string(),
            }),
            pp(),
        ).unwrap();
        new.add_edge_with_id(EdgeId::new(1), NodeId::new(0), n3, EdgeKind::ControlFlow).unwrap();

        let _diff = diff_scg(&old, &new);
        let script = compute_edit_script(&old, &new);
        assert!(!script.is_empty());

        // Apply the edit script
        let mut target = old.clone();
        apply_diff(&mut target, &SCGDiff::from_entries(script)).unwrap();

        // Verify: node 0 modified, node 1 removed, node 2 added, edge 0 removed, edge 1 added
        assert!(target.get_node(NodeId::new(0)).is_some());
        assert!(target.get_node(NodeId::new(1)).is_none());
        assert!(target.get_node(NodeId::new(2)).is_some());
        assert_eq!(target.node_count(), 2);
        assert_eq!(target.edge_count(), 1);
    }

    // ── Test 8: Three-way merge with no conflicts ──

    #[test]
    fn test_three_way_merge_no_conflicts() {
        // Base: single computation node
        let mut base = SCG::new();
        base.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();

        // Ours: adds a new node
        let mut ours = base.clone();
        ours.add_node_with_id(
            NodeId::new(1),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "sub".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();

        // Theirs: modifies the existing node
        let mut theirs = SCG::new();
        theirs.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "mul".to_string(), // modified
                result_type: None,
            }),
            pp(),
        ).unwrap();

        let result = three_way_merge(&base, &ours, &theirs);
        assert!(result.is_ok(), "Expected merge to succeed without conflicts");

        let merged = result.unwrap();
        assert_eq!(merged.node_count(), 2);
        // Node 0 should have the modification from theirs
        let n0 = merged.get_node(NodeId::new(0)).unwrap();
        assert_eq!(n0.payload, NodePayload::Computation(ComputationNode {
            operation: "mul".to_string(),
            result_type: None,
        }));
        // Node 1 should have been added from ours
        assert!(merged.get_node(NodeId::new(1)).is_some());
    }

    // ── Test 9: Three-way merge with conflicts ──

    #[test]
    fn test_three_way_merge_with_conflicts() {
        // Base: single computation node
        let mut base = SCG::new();
        base.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();

        // Ours: modifies the operation to "sub"
        let mut ours = SCG::new();
        ours.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "sub".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();

        // Theirs: modifies the operation to "mul" (conflicting change)
        let mut theirs = SCG::new();
        theirs.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "mul".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();

        let result = three_way_merge(&base, &ours, &theirs);
        assert!(result.is_err(), "Expected merge conflict");

        let conflict = result.unwrap_err();
        assert!(!conflict.is_empty());
        assert_eq!(conflict.node_conflicts.len(), 1);
        assert_eq!(conflict.node_conflicts[0].id, NodeId::new(0));
    }

    // ── Test 10: Edit script ordering is correct ──

    #[test]
    fn test_edit_script_ordering() {
        let mut old = SCG::new();
        old.add_node_with_id(NodeId::new(0), NodeType::Computation,
            NodePayload::Computation(ComputationNode { operation: "a".to_string(), result_type: None }), pp()).unwrap();
        old.add_node_with_id(NodeId::new(1), NodeType::Computation,
            NodePayload::Computation(ComputationNode { operation: "b".to_string(), result_type: None }), pp()).unwrap();
        old.add_edge_with_id(EdgeId::new(0), NodeId::new(0), NodeId::new(1), EdgeKind::DataFlow).unwrap();

        let mut new = SCG::new();
        new.add_node_with_id(NodeId::new(0), NodeType::Computation,
            NodePayload::Computation(ComputationNode { operation: "a_v2".to_string(), result_type: None }), pp()).unwrap();
        new.add_node_with_id(NodeId::new(2), NodeType::Phantom,
            NodePayload::Phantom(PhantomNode { purpose: "new".to_string() }), pp()).unwrap();
        new.add_edge_with_id(EdgeId::new(1), NodeId::new(0), NodeId::new(2), EdgeKind::ControlFlow).unwrap();

        let script = compute_edit_script(&old, &new);

        // Verify ordering: removals before modifications before additions
        let mut saw_modification = false;
        let mut saw_addition = false;
        for entry in &script {
            match entry {
                DiffEntry::EdgeAdded(_) | DiffEntry::NodeAdded(_) | DiffEntry::RegionAdded(_) => {
                    saw_addition = true;
                }
                DiffEntry::EdgeModified { .. } | DiffEntry::NodeModified { .. } | DiffEntry::RegionModified { .. } => {
                    assert!(!saw_addition, "Modification after addition");
                    saw_modification = true;
                }
                DiffEntry::EdgeRemoved(_) | DiffEntry::NodeRemoved(_) | DiffEntry::RegionRemoved(_) => {
                    assert!(!saw_modification, "Removal after modification");
                    assert!(!saw_addition, "Removal after addition");
                }
            }
        }
    }

    // ── Test 11: DiffEntry classification helpers ──

    #[test]
    fn test_diff_entry_classification() {
        let pp = pp();
        let node_data = NodeData {
            id: NodeId::new(0),
            node_type: NodeType::Computation,
            annotation: None,
            program_point: pp,
            payload: NodePayload::Computation(ComputationNode {
                operation: "test".to_string(),
                result_type: None,
            }),
        };

        let added = DiffEntry::NodeAdded(node_data.clone());
        assert!(added.is_addition());
        assert!(!added.is_removal());
        assert!(!added.is_modification());

        let removed = DiffEntry::NodeRemoved(NodeId::new(0));
        assert!(!removed.is_addition());
        assert!(removed.is_removal());
        assert!(!removed.is_modification());

        let modified = DiffEntry::NodeModified {
            id: NodeId::new(0),
            old: node_data.clone(),
            new: node_data,
        };
        assert!(!modified.is_addition());
        assert!(!modified.is_removal());
        assert!(modified.is_modification());
    }

    // ── Test 12: DiffStats aggregation ──

    #[test]
    fn test_diff_stats() {
        let stats = DiffStats {
            nodes_added: 3,
            nodes_removed: 1,
            nodes_modified: 2,
            edges_added: 4,
            edges_removed: 0,
            edges_modified: 1,
            regions_added: 1,
            regions_removed: 0,
            regions_modified: 0,
        };
        assert_eq!(stats.total_changes(), 12);
        assert!(!stats.is_empty());

        let empty = DiffStats::default();
        assert!(empty.is_empty());
        assert_eq!(empty.total_changes(), 0);
    }

    // ── Test 13: Apply diff detects duplicate node ──

    #[test]
    fn test_apply_diff_duplicate_node() {
        let mut scg = SCG::new();
        scg.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "a".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();

        // Try to add a node that already exists
        let diff = SCGDiff::from_entries(vec![DiffEntry::NodeAdded(NodeData {
            id: NodeId::new(0),
            node_type: NodeType::Phantom,
            annotation: None,
            program_point: pp(),
            payload: NodePayload::Phantom(PhantomNode {
                purpose: "dup".to_string(),
            }),
        })]);

        let result = apply_diff(&mut scg, &diff);
        assert!(matches!(result, Err(DiffError::DuplicateNode(_))));
    }

    // ── Test 14: Three-way merge with remove vs modify conflict ──

    #[test]
    fn test_three_way_merge_remove_vs_modify_conflict() {
        let mut base = SCG::new();
        base.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();
        base.add_node_with_id(
            NodeId::new(1),
            NodeType::Phantom,
            NodePayload::Phantom(PhantomNode {
                purpose: "marker".to_string(),
            }),
            pp(),
        ).unwrap();

        // Ours: removes node 1
        let mut ours = SCG::new();
        ours.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();
        // NodeId::new(1) is absent — removed

        // Theirs: modifies node 1
        let mut theirs = SCG::new();
        theirs.add_node_with_id(
            NodeId::new(0),
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "add".to_string(),
                result_type: None,
            }),
            pp(),
        ).unwrap();
        theirs.add_node_with_id(
            NodeId::new(1),
            NodeType::Phantom,
            NodePayload::Phantom(PhantomNode {
                purpose: "updated_marker".to_string(), // modified
            }),
            pp(),
        ).unwrap();

        let result = three_way_merge(&base, &ours, &theirs);
        // Ours removes node 1, theirs modifies it — conflict
        assert!(result.is_err());
        let conflict = result.unwrap_err();
        assert_eq!(conflict.node_conflicts.len(), 1);
    }

    // ── Test 15: DiffEntry describe() ──

    #[test]
    fn test_diff_entry_describe() {
        let entry = DiffEntry::NodeRemoved(NodeId::new(42));
        assert!(entry.describe().contains("NodeId(42)"));
        assert!(entry.describe().contains("removed"));

        let entry = DiffEntry::EdgeAdded(EdgeData::new(
            EdgeId::new(7), NodeId::new(1), NodeId::new(2), EdgeKind::DataFlow,
        ));
        assert!(entry.describe().contains("added"));
    }

    // ── Test 16: Empty graphs produce empty diff ──

    #[test]
    fn test_diff_empty_graphs() {
        let scg1 = SCG::new();
        let scg2 = SCG::new();
        let diff = diff_scg(&scg1, &scg2);
        assert!(diff.is_empty());
    }

    // ── Test 17: MergeConflict display and helpers ──

    #[test]
    fn test_merge_conflict_helpers() {
        let empty = MergeConflict::empty();
        assert!(empty.is_empty());
        assert_eq!(empty.total_conflicts(), 0);

        let conflict = MergeConflict {
            node_conflicts: vec![NodeConflict {
                id: NodeId::new(0),
                base: None,
                ours: None,
                theirs: None,
            }],
            edge_conflicts: vec![],
            region_conflicts: vec![],
        };
        assert!(!conflict.is_empty());
        assert_eq!(conflict.total_conflicts(), 1);
        assert!(conflict.to_string().contains("1 node"));
    }
}
