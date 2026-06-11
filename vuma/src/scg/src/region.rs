//! SCG Region Types
//!
//! This module defines memory regions within the Semantic Computation Graph.
//! Regions group nodes into logical memory scopes, enabling reasoning about
//! allocation lifetimes, access boundaries, and security isolation.

#![allow(clippy::explicit_counter_loop, clippy::needless_range_loop)]
//!
//! # Region Inference
//!
//! The [`infer_regions`] function automatically discovers memory regions from
//! the SCG by pairing allocation and deallocation nodes, collecting the nodes
//! within each lifetime scope, and building a nesting hierarchy.
//!
//! # Alias Analysis
//!
//! The [`RegionAliasAnalysis`] struct provides `may_alias` and `can_merge`
//! queries over inferred regions, supporting optimization decisions about
//! whether two regions can safely overlap or be merged.

use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

use crate::edge::EdgeKind;
use crate::graph::SCG;
use crate::node::{NodeId, NodePayload, NodeType};

/// Unique identifier for a region within the SCG.
///
/// `RegionId` is a newtype wrapper around `u64`, providing type safety
/// to distinguish region identifiers from node and edge identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RegionId(pub u64);

impl RegionId {
    /// Creates a new `RegionId` from a `u64` value.
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Returns the underlying `u64` value.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for RegionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RegionId({})", self.0)
    }
}

/// Deployment target for a region.
///
/// Specifies where the memory region is physically or logically
/// allocated, which affects access semantics and security properties.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeploymentTarget {
    /// Main program heap memory.
    Heap,
    /// Stack-allocated memory.
    Stack,
    /// GPU device memory.
    Gpu,
    /// Shared memory accessible across processes.
    Shared,
    /// Persisted storage (e.g., memory-mapped file).
    Persisted,
    /// A custom or vendor-specific target identified by name.
    Custom(String),
}

impl std::fmt::Display for DeploymentTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeploymentTarget::Heap => write!(f, "Heap"),
            DeploymentTarget::Stack => write!(f, "Stack"),
            DeploymentTarget::Gpu => write!(f, "Gpu"),
            DeploymentTarget::Shared => write!(f, "Shared"),
            DeploymentTarget::Persisted => write!(f, "Persisted"),
            DeploymentTarget::Custom(name) => write!(f, "Custom({name})"),
        }
    }
}

/// A memory region within the SCG.
///
/// Regions group related allocation, access, and deallocation nodes,
/// providing a scope for memory lifetime analysis and security boundary
/// enforcement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SCGRegion {
    /// The unique identifier of this region.
    pub id: RegionId,
    /// The set of nodes belonging to this region.
    pub nodes: hashbrown::HashSet<NodeId>,
    /// The nesting scope level (0 = top-level, higher = more deeply nested).
    pub scope_level: u32,
    /// Whether this region constitutes a security boundary.
    ///
    /// Security boundaries enforce access restrictions: nodes outside
    /// the boundary cannot directly access memory within it.
    pub security_boundary: bool,
    /// The deployment target specifying where this region's memory resides.
    pub deployment_target: DeploymentTarget,
}

impl SCGRegion {
    /// Creates a new region with the given ID and deployment target.
    ///
    /// The region starts with no nodes, scope level 0, and no security boundary.
    pub fn new(id: RegionId, deployment_target: DeploymentTarget) -> Self {
        Self {
            id,
            nodes: hashbrown::HashSet::new(),
            scope_level: 0,
            security_boundary: false,
            deployment_target,
        }
    }

    /// Creates a new region with a specified scope level.
    pub fn with_scope_level(
        id: RegionId,
        deployment_target: DeploymentTarget,
        scope_level: u32,
    ) -> Self {
        Self {
            id,
            nodes: hashbrown::HashSet::new(),
            scope_level,
            security_boundary: false,
            deployment_target,
        }
    }

    /// Creates a new security-boundary region.
    pub fn with_security_boundary(
        id: RegionId,
        deployment_target: DeploymentTarget,
        security_boundary: bool,
    ) -> Self {
        Self {
            id,
            nodes: hashbrown::HashSet::new(),
            scope_level: 0,
            security_boundary,
            deployment_target,
        }
    }

    /// Adds a node to this region.
    pub fn add_node(&mut self, node_id: NodeId) {
        self.nodes.insert(node_id);
    }

    /// Removes a node from this region.
    ///
    /// Returns `true` if the node was present and removed.
    pub fn remove_node(&mut self, node_id: &NodeId) -> bool {
        self.nodes.remove(node_id)
    }

    /// Returns `true` if this region contains the specified node.
    pub fn contains_node(&self, node_id: &NodeId) -> bool {
        self.nodes.contains(node_id)
    }

    /// Returns the number of nodes in this region.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns an iterator over the node IDs in this region.
    pub fn iter_nodes(&self) -> impl Iterator<Item = &NodeId> {
        self.nodes.iter()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Inferred Region and Region Lifetime
// ─────────────────────────────────────────────────────────────────────────────

/// Describes the lifetime semantics of an inferred region.
///
/// A region's lifetime determines when its memory is valid and how
/// deallocation is triggered.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RegionLifetime {
    /// The region lives for the entire duration of the program.
    Static,
    /// The region has a well-defined scope from allocation to deallocation.
    Scoped {
        /// The node that performs the allocation.
        alloc: NodeId,
        /// The node that performs the deallocation.
        dealloc: NodeId,
    },
    /// The region is reference-counted: deallocation occurs when the last
    /// reference is dropped. Each ref node represents a point where a
    /// reference count is incremented or decremented.
    ReferenceCounted {
        /// Nodes that hold or manipulate references into this region.
        ref_nodes: Vec<NodeId>,
    },
    /// The lifetime cannot be determined statically.
    Unknown,
}

impl std::fmt::Display for RegionLifetime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegionLifetime::Static => write!(f, "Static"),
            RegionLifetime::Scoped { alloc, dealloc } => {
                write!(f, "Scoped({}..{})", alloc, dealloc)
            }
            RegionLifetime::ReferenceCounted { ref_nodes } => {
                write!(f, "ReferenceCounted({} refs)", ref_nodes.len())
            }
            RegionLifetime::Unknown => write!(f, "Unknown"),
        }
    }
}

/// A memory region inferred from the SCG by pairing allocations
/// with deallocations.
///
/// Unlike [`SCGRegion`] which is manually constructed, `InferredRegion`
/// is automatically discovered by [`infer_regions`] from the graph
/// structure. Each inferred region corresponds to a single
/// allocation/deallocation pair and contains all nodes whose lifetimes
/// are bounded by that pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferredRegion {
    /// The unique identifier assigned to this inferred region.
    pub id: RegionId,
    /// All nodes that belong to this region (including alloc and dealloc).
    pub nodes: Vec<NodeId>,
    /// The entry (allocation) node of this region.
    pub entry_node: NodeId,
    /// The exit (deallocation) nodes of this region.
    ///
    /// Typically one node, but may be multiple if the allocation can be
    /// freed along different control-flow paths.
    pub exit_nodes: Vec<NodeId>,
    /// The lifetime semantics of this region.
    pub lifetime: RegionLifetime,
    /// The parent region, if this region is nested inside another.
    pub parent: Option<RegionId>,
    /// Child regions that are nested within this region.
    pub children: Vec<RegionId>,
}

impl InferredRegion {
    /// Returns `true` if this region contains the given node.
    pub fn contains_node(&self, node_id: NodeId) -> bool {
        self.nodes.contains(&node_id)
    }

    /// Returns the number of nodes in this region.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` if this region is a top-level region (no parent).
    pub fn is_top_level(&self) -> bool {
        self.parent.is_none()
    }

    /// Returns the scope depth of this region (0 = top-level).
    pub fn depth(&self, regions: &[InferredRegion]) -> u32 {
        let mut depth = 0u32;
        let mut current_parent = self.parent;
        let region_map: HashMap<RegionId, &InferredRegion> =
            regions.iter().map(|r| (r.id, r)).collect();
        while let Some(parent_id) = current_parent {
            depth += 1;
            if let Some(parent) = region_map.get(&parent_id) {
                current_parent = parent.parent;
            } else {
                break;
            }
        }
        depth
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Region Inference
// ─────────────────────────────────────────────────────────────────────────────

/// An alloc/dealloc pair discovered in the SCG.
struct AllocPair {
    alloc_node: NodeId,
    dealloc_node: NodeId,
}

/// Infers memory regions from the SCG by pairing allocation and
/// deallocation nodes.
///
/// # Algorithm
///
/// 1. Find all `Allocation` nodes and their matching `Deallocation` nodes
///    (matched via `DeallocationNode::allocation_node`).
/// 2. For each alloc/dealloc pair, collect all nodes on paths from the
///    allocation to the deallocation (excluding nodes that belong to
///    other alloc/dealloc pairs).
/// 3. Determine the lifetime kind (`Scoped` for paired, `Unknown` for
///    unpaired allocations).
/// 4. Build a nesting hierarchy: a region whose alloc and dealloc are
///    both contained within another region is a child of that region.
/// 5. Assign sequential `RegionId`s.
///
/// # Returns
///
/// A vector of `InferredRegion`s sorted by `RegionId`.
pub fn infer_regions(scg: &SCG) -> Vec<InferredRegion> {
    // Step 1: Collect alloc/dealloc pairs.
    let mut pairs: Vec<AllocPair> = Vec::new();
    let mut unpaired_allocs: Vec<NodeId> = Vec::new();
    let alloc_set: HashSet<NodeId> = scg
        .nodes()
        .filter(|n| n.node_type == NodeType::Allocation)
        .map(|n| n.id)
        .collect();

    // Track which allocs have been matched
    let mut matched_allocs: HashSet<NodeId> = HashSet::new();

    // Collect pairs from deallocation nodes
    for node in scg.nodes() {
        if node.node_type == NodeType::Deallocation {
            if let NodePayload::Deallocation(dealloc) = &node.payload {
                if alloc_set.contains(&dealloc.allocation_node) {
                    pairs.push(AllocPair {
                        alloc_node: dealloc.allocation_node,
                        dealloc_node: node.id,
                    });
                    matched_allocs.insert(dealloc.allocation_node);
                }
            }
        }
    }

    // Collect unpaired allocations
    for alloc_id in &alloc_set {
        if !matched_allocs.contains(alloc_id) {
            unpaired_allocs.push(*alloc_id);
        }
    }

    // Step 2: For each pair, collect nodes in the region scope.
    // A node is in the region if it is reachable from alloc AND can reach dealloc,
    // using DataFlow, ControlFlow, and Derivation edges.
    let mut region_node_sets: Vec<(AllocPair, Vec<NodeId>)> = Vec::new();

    for pair in &pairs {
        let reachable_from_alloc = bfs_forward(scg, pair.alloc_node, Some(pair.dealloc_node));
        let reaching_dealloc = bfs_backward(scg, pair.dealloc_node, Some(pair.alloc_node));

        // Nodes in the region: reachable from alloc AND can reach dealloc
        let reachable_set: HashSet<NodeId> = reachable_from_alloc.into_iter().collect();
        let reaching_set: HashSet<NodeId> = reaching_dealloc.into_iter().collect();
        let region_nodes: Vec<NodeId> =
            reachable_set.intersection(&reaching_set).copied().collect();

        region_node_sets.push((
            AllocPair {
                alloc_node: pair.alloc_node,
                dealloc_node: pair.dealloc_node,
            },
            region_nodes,
        ));
    }

    // For unpaired allocs, create regions with Unknown lifetime containing
    // just the alloc node and nodes reachable from it (limited scope).
    for alloc_id in unpaired_allocs {
        let reachable = bfs_forward(scg, alloc_id, None);
        region_node_sets.push((
            AllocPair {
                alloc_node: alloc_id,
                dealloc_node: alloc_id, // placeholder; won't be used as Scoped
            },
            reachable,
        ));
    }

    // Step 3: Build InferredRegion objects and determine lifetimes.
    let mut regions: Vec<InferredRegion> = Vec::new();
    let mut next_id: u64 = 0;

    for (pair, nodes) in &region_node_sets {
        let id = RegionId::new(next_id);
        next_id += 1;

        let is_paired =
            alloc_set.contains(&pair.alloc_node) && pair.dealloc_node != pair.alloc_node;

        let lifetime = if is_paired {
            RegionLifetime::Scoped {
                alloc: pair.alloc_node,
                dealloc: pair.dealloc_node,
            }
        } else {
            RegionLifetime::Unknown
        };

        // Exit nodes: for paired regions, the dealloc node; for unpaired, empty.
        let exit_nodes = if is_paired {
            vec![pair.dealloc_node]
        } else {
            Vec::new()
        };

        regions.push(InferredRegion {
            id,
            nodes: nodes.clone(),
            entry_node: pair.alloc_node,
            exit_nodes,
            lifetime,
            parent: None,
            children: Vec::new(),
        });
    }

    // Step 4: Build nesting hierarchy.
    // Region A is a child of region B if both A's alloc and dealloc are
    // contained in B's node set. Pick the smallest (most deeply nested)
    // such parent.
    let region_count = regions.len();
    for i in 0..region_count {
        let mut best_parent: Option<usize> = None;
        let mut best_parent_size = usize::MAX;

        {
            let child = &regions[i];
            let child_alloc = child.entry_node;
            // For unpaired regions, skip nesting check
            let child_dealloc = if let Some(exit) = child.exit_nodes.first() {
                *exit
            } else {
                continue;
            };

            for j in 0..region_count {
                if i == j {
                    continue;
                }
                let candidate = &regions[j];
                let candidate_nodes: HashSet<NodeId> = candidate.nodes.iter().copied().collect();

                if candidate_nodes.contains(&child_alloc)
                    && candidate_nodes.contains(&child_dealloc)
                    && candidate.nodes.len() < best_parent_size
                {
                    best_parent_size = candidate.nodes.len();
                    best_parent = Some(j);
                }
            }
        }

        if let Some(parent_idx) = best_parent {
            let parent_id = regions[parent_idx].id;
            regions[i].parent = Some(parent_id);
            // Note: exit_nodes should not contain parent_id; children tracked on parent
            // regions[i].exit_nodes.push(parent_id); // intentionally removed
        }
    }

    // Fix: remove the incorrect exit_nodes addition from above, and add children
    // First, reset exit_nodes that were incorrectly extended
    for i in 0..region_count {
        let is_paired = matches!(regions[i].lifetime, RegionLifetime::Scoped { .. });
        if !is_paired {
            // unpaired — exit_nodes should be empty
            regions[i].exit_nodes.clear();
        } else {
            // paired — exit_nodes should be just the dealloc node
            regions[i].exit_nodes.truncate(1);
        }
    }

    // Now add children references to parents
    for i in 0..region_count {
        if let Some(parent_id) = regions[i].parent {
            let child_id = regions[i].id;
            if let Some(parent) = regions.iter_mut().find(|r| r.id == parent_id) {
                parent.children.push(child_id);
            }
        }
    }

    // Step 5: Sort by RegionId for deterministic output
    regions.sort_by_key(|r| r.id);
    regions
}

/// BFS forward from `start`, collecting all reachable nodes via
/// DataFlow, ControlFlow, and Derivation edges. If `stop` is provided,
/// the search includes `stop` but does not traverse beyond it.
fn bfs_forward(scg: &SCG, start: NodeId, stop: Option<NodeId>) -> Vec<NodeId> {
    let mut visited = HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(start);
    visited.insert(start);

    while let Some(current) = queue.pop_front() {
        if stop == Some(current) {
            // Include the stop node but don't expand past it
            continue;
        }
        if let Some(successors) = scg.successors(current) {
            for succ in successors {
                if visited.insert(succ) {
                    queue.push_back(succ);
                }
            }
        }
    }

    visited.into_iter().collect()
}

/// BFS backward from `start`, collecting all nodes that can reach `start`
/// via DataFlow, ControlFlow, and Derivation edges. If `stop` is provided,
/// the search includes `stop` but does not traverse beyond it.
fn bfs_backward(scg: &SCG, start: NodeId, stop: Option<NodeId>) -> Vec<NodeId> {
    let mut visited = HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(start);
    visited.insert(start);

    while let Some(current) = queue.pop_front() {
        if stop == Some(current) {
            continue;
        }
        if let Some(predecessors) = scg.predecessors(current) {
            for pred in predecessors {
                if visited.insert(pred) {
                    queue.push_back(pred);
                }
            }
        }
    }

    visited.into_iter().collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Region Alias Analysis
// ─────────────────────────────────────────────────────────────────────────────

/// Alias analysis over inferred regions.
///
/// Provides queries about whether two regions may alias (access the same
/// memory) and whether they can be safely merged.
#[derive(Debug, Clone)]
pub struct RegionAliasAnalysis {
    /// The inferred regions this analysis was built from.
    regions: Vec<InferredRegion>,
    /// Map from RegionId to index in the regions vector.
    region_index: HashMap<RegionId, usize>,
    /// Set of region pairs that may alias, populated during construction.
    alias_pairs: HashSet<(RegionId, RegionId)>,
}

impl RegionAliasAnalysis {
    /// Constructs a new alias analysis from the given SCG.
    ///
    /// First infers regions, then computes alias relationships based on:
    /// - Shared access nodes targeting the same memory region_id
    /// - Data flow edges crossing between regions
    /// - Overlapping lifetimes
    /// - Ancestor/descendant relationships
    pub fn new(scg: &SCG) -> Self {
        let regions = infer_regions(scg);
        let region_index: HashMap<RegionId, usize> =
            regions.iter().enumerate().map(|(i, r)| (r.id, i)).collect();

        let mut analysis = Self {
            regions,
            region_index,
            alias_pairs: HashSet::new(),
        };

        analysis.compute_alias_pairs(scg);
        analysis
    }

    /// Constructs an alias analysis from pre-inferred regions.
    pub fn from_regions(scg: &SCG, regions: Vec<InferredRegion>) -> Self {
        let region_index: HashMap<RegionId, usize> =
            regions.iter().enumerate().map(|(i, r)| (r.id, i)).collect();

        let mut analysis = Self {
            regions,
            region_index,
            alias_pairs: HashSet::new(),
        };

        analysis.compute_alias_pairs(scg);
        analysis
    }

    /// Returns a reference to the inferred regions.
    pub fn regions(&self) -> &[InferredRegion] {
        &self.regions
    }

    /// Returns a reference to the inferred region with the given ID.
    pub fn get_region(&self, id: RegionId) -> Option<&InferredRegion> {
        self.region_index.get(&id).map(|&idx| &self.regions[idx])
    }

    /// Returns `true` if two regions may alias (access the same memory).
    ///
    /// Two regions may alias if any of the following conditions hold:
    /// - They share nodes in common
    /// - There is a DataFlow edge from a node in one region to a node in the other
    /// - Their lifetimes overlap (both are live at some program point)
    /// - One is an ancestor of the other (nested regions share scope)
    /// - They contain Access nodes targeting the same `region_id`
    pub fn may_alias(&self, _scg: &SCG, region_a: RegionId, region_b: RegionId) -> bool {
        if region_a == region_b {
            return true;
        }

        let (lo, hi) = if region_a < region_b {
            (region_a, region_b)
        } else {
            (region_b, region_a)
        };

        self.alias_pairs.contains(&(lo, hi))
    }

    /// Returns `true` if two regions can be safely merged.
    ///
    /// Regions can be merged if:
    /// - They do NOT may-alias (non-overlapping memory)
    /// - Neither region has a security boundary in the original SCG
    /// - Neither region is an ancestor of the other
    /// - Their lifetimes are compatible (both Scoped and non-overlapping,
    ///   or one is Static and the other is Scoped)
    pub fn can_merge(&self, scg: &SCG, region_a: RegionId, region_b: RegionId) -> bool {
        if region_a == region_b {
            return false; // already the same region
        }

        // Cannot merge if they may alias
        if self.may_alias(scg, region_a, region_b) {
            return false;
        }

        let reg_a = match self.get_region(region_a) {
            Some(r) => r,
            None => return false,
        };
        let reg_b = match self.get_region(region_b) {
            Some(r) => r,
            None => return false,
        };

        // Cannot merge if one is ancestor of the other
        if self.is_ancestor(region_a, region_b) || self.is_ancestor(region_b, region_a) {
            return false;
        }

        // Cannot merge if either region has a security boundary in the SCG
        if self.has_security_boundary(scg, region_a) || self.has_security_boundary(scg, region_b) {
            return false;
        }

        // Check lifetime compatibility
        match (&reg_a.lifetime, &reg_b.lifetime) {
            // Two scoped regions with non-overlapping lifetimes can merge
            (
                RegionLifetime::Scoped {
                    alloc: a1,
                    dealloc: d1,
                },
                RegionLifetime::Scoped {
                    alloc: a2,
                    dealloc: d2,
                },
            ) => {
                // Non-overlapping: one ends before the other starts
                let a_before_b = node_order(scg, *d1) < node_order(scg, *a2);
                let b_before_a = node_order(scg, *d2) < node_order(scg, *a1);
                a_before_b || b_before_a
            }
            // Static and Scoped can merge if the scoped region doesn't conflict
            (RegionLifetime::Static, RegionLifetime::Scoped { .. })
            | (RegionLifetime::Scoped { .. }, RegionLifetime::Static) => true,
            // Two static regions can merge
            (RegionLifetime::Static, RegionLifetime::Static) => true,
            // Unknown lifetimes are conservative: cannot merge
            (RegionLifetime::Unknown, _) | (_, RegionLifetime::Unknown) => false,
            // Reference-counted regions can merge if they don't share ref nodes
            (
                RegionLifetime::ReferenceCounted { ref_nodes: a_refs },
                RegionLifetime::ReferenceCounted { ref_nodes: b_refs },
            ) => {
                let a_set: HashSet<NodeId> = a_refs.iter().copied().collect();
                let b_set: HashSet<NodeId> = b_refs.iter().copied().collect();
                a_set.is_disjoint(&b_set)
            }
            (RegionLifetime::ReferenceCounted { .. }, _)
            | (_, RegionLifetime::ReferenceCounted { .. }) => {
                false // conservative: don't merge RC with non-RC
            }
        }
    }

    /// Returns `true` if `ancestor` is an ancestor of `descendant`.
    fn is_ancestor(&self, ancestor: RegionId, descendant: RegionId) -> bool {
        let mut current = self.get_region(descendant).and_then(|r| r.parent);
        while let Some(parent_id) = current {
            if parent_id == ancestor {
                return true;
            }
            current = self.get_region(parent_id).and_then(|r| r.parent);
        }
        false
    }

    /// Checks whether the SCG region corresponding to this inferred region
    /// has a security boundary.
    fn has_security_boundary(&self, scg: &SCG, region_id: RegionId) -> bool {
        // Check if the inferred region's entry node belongs to an SCG region
        // that has a security boundary
        if let Some(inferred) = self.get_region(region_id) {
            for node_id in &inferred.nodes {
                if let Some(node) = scg.get_node(*node_id) {
                    if let NodePayload::Allocation(alloc) = &node.payload {
                        if let Some(scg_region) = scg.get_region(alloc.region_id) {
                            if scg_region.security_boundary {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Computes the set of alias pairs during construction.
    fn compute_alias_pairs(&mut self, scg: &SCG) {
        let n = self.regions.len();

        // Build node-to-region mapping
        let mut node_to_regions: HashMap<NodeId, Vec<RegionId>> = HashMap::new();
        for region in &self.regions {
            for &node_id in &region.nodes {
                node_to_regions.entry(node_id).or_default().push(region.id);
            }
        }

        // Shared nodes => may alias
        for regions in node_to_regions.values() {
            for i in 0..regions.len() {
                for j in (i + 1)..regions.len() {
                    let (lo, hi) = if regions[i] < regions[j] {
                        (regions[i], regions[j])
                    } else {
                        (regions[j], regions[i])
                    };
                    self.alias_pairs.insert((lo, hi));
                }
            }
        }

        // DataFlow edges crossing regions => may alias
        for edge in scg.edges() {
            if matches!(edge.kind, EdgeKind::DataFlow) {
                if let (Some(src_regions), Some(tgt_regions)) = (
                    node_to_regions.get(&edge.source),
                    node_to_regions.get(&edge.target),
                ) {
                    for &src_rid in src_regions {
                        for &tgt_rid in tgt_regions {
                            if src_rid != tgt_rid {
                                let (lo, hi) = if src_rid < tgt_rid {
                                    (src_rid, tgt_rid)
                                } else {
                                    (tgt_rid, src_rid)
                                };
                                self.alias_pairs.insert((lo, hi));
                            }
                        }
                    }
                }
            }
        }

        // Ancestor/descendant => may alias (nested regions share scope)
        for region in &self.regions {
            if let Some(parent_id) = region.parent {
                let (lo, hi) = if region.id < parent_id {
                    (region.id, parent_id)
                } else {
                    (parent_id, region.id)
                };
                self.alias_pairs.insert((lo, hi));
            }
        }

        // Overlapping lifetimes for Scoped regions
        for i in 0..n {
            for j in (i + 1)..n {
                let (lo, hi) = if self.regions[i].id < self.regions[j].id {
                    (self.regions[i].id, self.regions[j].id)
                } else {
                    (self.regions[j].id, self.regions[i].id)
                };

                if self.alias_pairs.contains(&(lo, hi)) {
                    continue; // already known to alias
                }

                if let (
                    RegionLifetime::Scoped {
                        alloc: a1,
                        dealloc: d1,
                    },
                    RegionLifetime::Scoped {
                        alloc: a2,
                        dealloc: d2,
                    },
                ) = (&self.regions[i].lifetime, &self.regions[j].lifetime)
                {
                    // Check if lifetimes overlap
                    if lifetimes_overlap(scg, *a1, *d1, *a2, *d2) {
                        self.alias_pairs.insert((lo, hi));
                    }
                }
            }
        }

        // Access nodes targeting the same region_id
        let mut access_region_groups: HashMap<crate::region::RegionId, Vec<RegionId>> =
            HashMap::new();
        for inferred in &self.regions {
            for &node_id in &inferred.nodes {
                if let Some(node) = scg.get_node(node_id) {
                    if let NodePayload::Access(access) = &node.payload {
                        access_region_groups
                            .entry(access.region_id)
                            .or_default()
                            .push(inferred.id);
                    }
                }
            }
        }
        for group in access_region_groups.values() {
            for i in 0..group.len() {
                for j in (i + 1)..group.len() {
                    let (lo, hi) = if group[i] < group[j] {
                        (group[i], group[j])
                    } else {
                        (group[j], group[i])
                    };
                    self.alias_pairs.insert((lo, hi));
                }
            }
        }
    }
}

/// Determines if two scoped lifetimes overlap by checking topological
/// ordering. Lifetimes overlap if neither [a1..d1] ends before [a2..d2]
/// starts, nor vice versa.
fn lifetimes_overlap(scg: &SCG, a1: NodeId, d1: NodeId, a2: NodeId, d2: NodeId) -> bool {
    // Use topological sort to determine ordering
    let topo = match scg.topological_sort() {
        Ok(t) => t,
        Err(_) => return true, // cycle => conservative: assume overlap
    };

    let pos = |id: NodeId| -> Option<usize> { topo.iter().position(|&x| x == id) };

    let (p_a1, p_d1, p_a2, p_d2) = match (pos(a1), pos(d1), pos(a2), pos(d2)) {
        (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
        _ => return true, // node not found => conservative
    };

    // Overlap if neither interval is completely before the other
    !(p_d1 < p_a2 || p_d2 < p_a1)
}

/// Returns the topological order position of a node, or `usize::MAX`
/// if the node is not in the topological sort (e.g., in a cycle).
fn node_order(scg: &SCG, node: NodeId) -> usize {
    match scg.topological_sort() {
        Ok(topo) => topo.iter().position(|&id| id == node).unwrap_or(usize::MAX),
        Err(_) => usize::MAX,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::graph::SCG;
    use crate::node::{
        AccessMode, AccessNode, AllocationNode, ComputationNode, DeallocationNode, NodePayload,
        NodeType, ProgramPoint,
    };

    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: Some("test.vu".to_string()),
            line: Some(1),
            column: Some(1),
            offset: None,
        }
    }

    // ── Original tests (preserved) ───────────────────────────────────

    #[test]
    fn test_region_id_creation_and_display() {
        let id = RegionId::new(3);
        assert_eq!(id.as_u64(), 3);
        assert_eq!(format!("{id}"), "RegionId(3)");
    }

    #[test]
    fn test_deployment_target_display() {
        assert_eq!(format!("{}", DeploymentTarget::Heap), "Heap");
        assert_eq!(format!("{}", DeploymentTarget::Gpu), "Gpu");
        assert_eq!(
            format!("{}", DeploymentTarget::Custom("TPU".to_string())),
            "Custom(TPU)"
        );
    }

    #[test]
    fn test_region_new() {
        let region = SCGRegion::new(RegionId::new(1), DeploymentTarget::Heap);
        assert_eq!(region.id, RegionId::new(1));
        assert!(region.nodes.is_empty());
        assert_eq!(region.scope_level, 0);
        assert!(!region.security_boundary);
    }

    #[test]
    fn test_region_add_remove_nodes() {
        let mut region = SCGRegion::new(RegionId::new(1), DeploymentTarget::Heap);
        let n1 = NodeId::new(10);
        let n2 = NodeId::new(20);

        region.add_node(n1);
        region.add_node(n2);
        assert_eq!(region.node_count(), 2);
        assert!(region.contains_node(&n1));
        assert!(region.contains_node(&n2));

        assert!(region.remove_node(&n1));
        assert_eq!(region.node_count(), 1);
        assert!(!region.contains_node(&n1));

        // Removing again returns false
        assert!(!region.remove_node(&n1));
    }

    #[test]
    fn test_region_security_boundary() {
        let region =
            SCGRegion::with_security_boundary(RegionId::new(2), DeploymentTarget::Gpu, true);
        assert!(region.security_boundary);
    }

    // ── RegionLifetime tests ─────────────────────────────────────────

    #[test]
    fn test_region_lifetime_display() {
        let scoped = RegionLifetime::Scoped {
            alloc: NodeId::new(1),
            dealloc: NodeId::new(5),
        };
        assert_eq!(format!("{scoped}"), "Scoped(NodeId(1)..NodeId(5))");

        let rc = RegionLifetime::ReferenceCounted {
            ref_nodes: vec![NodeId::new(2), NodeId::new(3)],
        };
        assert_eq!(format!("{rc}"), "ReferenceCounted(2 refs)");

        assert_eq!(format!("{}", RegionLifetime::Static), "Static");
        assert_eq!(format!("{}", RegionLifetime::Unknown), "Unknown");
    }

    #[test]
    fn test_region_lifetime_equality() {
        let a = RegionLifetime::Scoped {
            alloc: NodeId::new(1),
            dealloc: NodeId::new(2),
        };
        let b = RegionLifetime::Scoped {
            alloc: NodeId::new(1),
            dealloc: NodeId::new(2),
        };
        assert_eq!(a, b);

        let c = RegionLifetime::Scoped {
            alloc: NodeId::new(1),
            dealloc: NodeId::new(3),
        };
        assert_ne!(a, c);
    }

    // ── InferredRegion tests ─────────────────────────────────────────

    #[test]
    fn test_inferred_region_contains_node() {
        let region = InferredRegion {
            id: RegionId::new(0),
            nodes: vec![NodeId::new(1), NodeId::new(2), NodeId::new(3)],
            entry_node: NodeId::new(1),
            exit_nodes: vec![NodeId::new(3)],
            lifetime: RegionLifetime::Scoped {
                alloc: NodeId::new(1),
                dealloc: NodeId::new(3),
            },
            parent: None,
            children: Vec::new(),
        };
        assert!(region.contains_node(NodeId::new(2)));
        assert!(!region.contains_node(NodeId::new(99)));
        assert_eq!(region.node_count(), 3);
        assert!(region.is_top_level());
    }

    #[test]
    fn test_inferred_region_depth() {
        let regions = vec![
            InferredRegion {
                id: RegionId::new(0),
                nodes: vec![NodeId::new(1)],
                entry_node: NodeId::new(1),
                exit_nodes: vec![NodeId::new(1)],
                lifetime: RegionLifetime::Unknown,
                parent: None,
                children: vec![RegionId::new(1)],
            },
            InferredRegion {
                id: RegionId::new(1),
                nodes: vec![NodeId::new(2)],
                entry_node: NodeId::new(2),
                exit_nodes: vec![NodeId::new(2)],
                lifetime: RegionLifetime::Unknown,
                parent: Some(RegionId::new(0)),
                children: Vec::new(),
            },
        ];
        assert_eq!(regions[0].depth(&regions), 0);
        assert_eq!(regions[1].depth(&regions), 1);
    }

    // ── infer_regions: simple region inference ────────────────────────

    #[test]
    fn test_infer_regions_simple_pair() {
        let mut scg = SCG::new();
        let rid = RegionId::new(1);

        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: None,
            }),
            pp(),
        );
        let comp = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "process".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: rid,
            }),
            pp(),
        );

        scg.add_edge(alloc, comp, EdgeKind::DataFlow).unwrap();
        scg.add_edge(comp, dealloc, EdgeKind::DataFlow).unwrap();

        let regions = infer_regions(&scg);
        assert_eq!(regions.len(), 1);

        let r = &regions[0];
        assert_eq!(r.entry_node, alloc);
        assert_eq!(r.exit_nodes, vec![dealloc]);
        assert!(r.contains_node(alloc));
        assert!(r.contains_node(comp));
        assert!(r.contains_node(dealloc));
        assert!(r.parent.is_none());
        assert!(r.children.is_empty());

        // Verify lifetime
        match &r.lifetime {
            RegionLifetime::Scoped {
                alloc: a,
                dealloc: d,
            } => {
                assert_eq!(*a, alloc);
                assert_eq!(*d, dealloc);
            }
            other => panic!("expected Scoped lifetime, got {:?}", other),
        }
    }

    // ── infer_regions: multiple independent regions ──────────────────

    #[test]
    fn test_infer_regions_multiple_independent() {
        let mut scg = SCG::new();

        // Region A
        let alloc_a = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 32,
                align: 8,
                region_id: RegionId::new(1),
                type_name: None,
            }),
            pp(),
        );
        let dealloc_a = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_a,
                region_id: RegionId::new(1),
            }),
            pp(),
        );
        scg.add_edge(alloc_a, dealloc_a, EdgeKind::DataFlow)
            .unwrap();

        // Region B
        let alloc_b = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: RegionId::new(2),
                type_name: None,
            }),
            pp(),
        );
        let dealloc_b = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_b,
                region_id: RegionId::new(2),
            }),
            pp(),
        );
        scg.add_edge(alloc_b, dealloc_b, EdgeKind::DataFlow)
            .unwrap();

        let regions = infer_regions(&scg);
        assert_eq!(regions.len(), 2);

        // Both should be top-level with Scoped lifetime
        for r in &regions {
            assert!(r.is_top_level());
            assert!(matches!(r.lifetime, RegionLifetime::Scoped { .. }));
        }
    }

    // ── infer_regions: nested regions ────────────────────────────────

    #[test]
    fn test_infer_regions_nested() {
        let mut scg = SCG::new();

        // Outer region
        let alloc_outer = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 16,
                region_id: RegionId::new(1),
                type_name: None,
            }),
            pp(),
        );
        let comp_outer = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "outer_work".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        // Inner region
        let alloc_inner = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: RegionId::new(2),
                type_name: None,
            }),
            pp(),
        );
        let comp_inner = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "inner_work".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let dealloc_inner = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_inner,
                region_id: RegionId::new(2),
            }),
            pp(),
        );
        let dealloc_outer = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_outer,
                region_id: RegionId::new(1),
            }),
            pp(),
        );

        // Build edges: outer_alloc -> comp_outer -> inner_alloc -> comp_inner -> inner_dealloc -> outer_dealloc
        scg.add_edge(alloc_outer, comp_outer, EdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(comp_outer, alloc_inner, EdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(alloc_inner, comp_inner, EdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(comp_inner, dealloc_inner, EdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(dealloc_inner, dealloc_outer, EdgeKind::DataFlow)
            .unwrap();

        let regions = infer_regions(&scg);
        assert_eq!(regions.len(), 2);

        // Find the outer and inner regions
        let outer = regions
            .iter()
            .find(|r| r.entry_node == alloc_outer)
            .expect("outer region should exist");
        let inner = regions
            .iter()
            .find(|r| r.entry_node == alloc_inner)
            .expect("inner region should exist");

        // Outer should be top-level, inner should have parent
        assert!(outer.parent.is_none());
        assert!(inner.parent.is_some());
        assert_eq!(inner.parent.unwrap(), outer.id);

        // Outer should list inner as child
        assert!(outer.children.contains(&inner.id));
    }

    // ── infer_regions: unpaired allocation ───────────────────────────

    #[test]
    fn test_infer_regions_unpaired_allocation() {
        let mut scg = SCG::new();

        let _alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 128,
                align: 8,
                region_id: RegionId::new(1),
                type_name: None,
            }),
            pp(),
        );
        // No deallocation node added

        let regions = infer_regions(&scg);
        assert_eq!(regions.len(), 1);

        let r = &regions[0];
        assert!(matches!(r.lifetime, RegionLifetime::Unknown));
        assert!(r.exit_nodes.is_empty());
    }

    // ── RegionAliasAnalysis: may_alias with shared nodes ─────────────

    #[test]
    fn test_may_alias_shared_access_nodes() {
        let mut scg = SCG::new();
        let rid = RegionId::new(1);

        // Region A: alloc -> access -> dealloc
        let alloc_a = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: None,
            }),
            pp(),
        );
        let access_a = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::ReadWrite,
                region_id: rid,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        let dealloc_a = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_a,
                region_id: rid,
            }),
            pp(),
        );
        scg.add_edge(alloc_a, access_a, EdgeKind::DataFlow).unwrap();
        scg.add_edge(access_a, dealloc_a, EdgeKind::DataFlow)
            .unwrap();

        // Region B: alloc -> access -> dealloc (same region_id!)
        let alloc_b = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: None,
            }),
            pp(),
        );
        let access_b = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::ReadWrite,
                region_id: rid,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        let dealloc_b = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_b,
                region_id: rid,
            }),
            pp(),
        );
        scg.add_edge(alloc_b, access_b, EdgeKind::DataFlow).unwrap();
        scg.add_edge(access_b, dealloc_b, EdgeKind::DataFlow)
            .unwrap();

        let analysis = RegionAliasAnalysis::new(&scg);
        let regions = analysis.regions();

        assert_eq!(regions.len(), 2);

        // The two regions access the same region_id, so they should may_alias
        let rid_a = regions[0].id;
        let rid_b = regions[1].id;
        assert!(
            analysis.may_alias(&scg, rid_a, rid_b),
            "regions accessing the same region_id should may-alias"
        );
    }

    // ── RegionAliasAnalysis: no alias for disjoint regions ───────────

    #[test]
    fn test_may_alias_disjoint_regions() {
        let mut scg = SCG::new();

        // Region A: uses region_id 1
        let alloc_a = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 32,
                align: 8,
                region_id: RegionId::new(1),
                type_name: None,
            }),
            pp(),
        );
        let comp_a = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "work_a".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let dealloc_a = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_a,
                region_id: RegionId::new(1),
            }),
            pp(),
        );
        scg.add_edge(alloc_a, comp_a, EdgeKind::DataFlow).unwrap();
        scg.add_edge(comp_a, dealloc_a, EdgeKind::DataFlow).unwrap();

        // Region B: uses region_id 2, fully independent
        let alloc_b = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: RegionId::new(2),
                type_name: None,
            }),
            pp(),
        );
        let comp_b = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "work_b".to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let dealloc_b = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_b,
                region_id: RegionId::new(2),
            }),
            pp(),
        );
        scg.add_edge(alloc_b, comp_b, EdgeKind::DataFlow).unwrap();
        scg.add_edge(comp_b, dealloc_b, EdgeKind::DataFlow).unwrap();

        let analysis = RegionAliasAnalysis::new(&scg);
        let regions = analysis.regions();
        assert_eq!(regions.len(), 2);

        let rid_a = regions[0].id;
        let rid_b = regions[1].id;

        // Disjoint regions with non-overlapping lifetimes and different region_ids
        // should NOT alias
        assert!(
            !analysis.may_alias(&scg, rid_a, rid_b),
            "disjoint regions with non-overlapping lifetimes should not alias"
        );
    }

    // ── RegionAliasAnalysis: may_alias with DataFlow crossing ────────

    #[test]
    fn test_may_alias_dataflow_crossing() {
        let mut scg = SCG::new();

        // Region A
        let alloc_a = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: RegionId::new(1),
                type_name: None,
            }),
            pp(),
        );
        let dealloc_a = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_a,
                region_id: RegionId::new(1),
            }),
            pp(),
        );
        scg.add_edge(alloc_a, dealloc_a, EdgeKind::DataFlow)
            .unwrap();

        // Region B
        let alloc_b = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: RegionId::new(2),
                type_name: None,
            }),
            pp(),
        );
        let dealloc_b = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_b,
                region_id: RegionId::new(2),
            }),
            pp(),
        );
        scg.add_edge(alloc_b, dealloc_b, EdgeKind::DataFlow)
            .unwrap();

        // Cross-region DataFlow edge: dealloc_a -> alloc_b
        // This makes their lifetimes overlap (they are connected by data flow)
        scg.add_edge(dealloc_a, alloc_b, EdgeKind::DataFlow)
            .unwrap();

        let analysis = RegionAliasAnalysis::new(&scg);
        let regions = analysis.regions();

        let rid_a = regions.iter().find(|r| r.entry_node == alloc_a).unwrap().id;
        let rid_b = regions.iter().find(|r| r.entry_node == alloc_b).unwrap().id;

        assert!(
            analysis.may_alias(&scg, rid_a, rid_b),
            "data flow crossing between regions should cause may-alias"
        );
    }

    // ── RegionAliasAnalysis: can_merge for non-overlapping ───────────

    #[test]
    fn test_can_merge_non_overlapping() {
        let mut scg = SCG::new();

        // Region A: sequential, then ends
        let alloc_a = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 32,
                align: 8,
                region_id: RegionId::new(1),
                type_name: None,
            }),
            pp(),
        );
        let dealloc_a = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_a,
                region_id: RegionId::new(1),
            }),
            pp(),
        );
        scg.add_edge(alloc_a, dealloc_a, EdgeKind::Derivation)
            .unwrap();

        // Region B: starts after A ends (no shared nodes, no crossing edges)
        let alloc_b = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: RegionId::new(2),
                type_name: None,
            }),
            pp(),
        );
        let dealloc_b = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_b,
                region_id: RegionId::new(2),
            }),
            pp(),
        );
        scg.add_edge(alloc_b, dealloc_b, EdgeKind::Derivation)
            .unwrap();

        let analysis = RegionAliasAnalysis::new(&scg);
        let regions = analysis.regions();
        assert_eq!(regions.len(), 2);

        let rid_a = regions[0].id;
        let rid_b = regions[1].id;

        // Two non-overlapping scoped regions with no alias and no
        // security boundary should be mergeable
        assert!(
            analysis.can_merge(&scg, rid_a, rid_b),
            "non-overlapping scoped regions without security boundaries should be mergeable"
        );
    }

    // ── RegionAliasAnalysis: cannot merge with security boundary ─────

    #[test]
    fn test_cannot_merge_security_boundary() {
        let mut scg = SCG::new();
        let rid = RegionId::new(1);

        // Region A with a security boundary
        let alloc_a = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 32,
                align: 8,
                region_id: rid,
                type_name: None,
            }),
            pp(),
        );
        let dealloc_a = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_a,
                region_id: rid,
            }),
            pp(),
        );
        scg.add_edge(alloc_a, dealloc_a, EdgeKind::Derivation)
            .unwrap();

        // Add a security boundary region in the SCG
        let mut scg_region = SCGRegion::with_security_boundary(rid, DeploymentTarget::Heap, true);
        scg_region.add_node(alloc_a);
        scg.add_region(scg_region);

        // Region B (independent, no security boundary)
        let rid_b = RegionId::new(2);
        let alloc_b = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid_b,
                type_name: None,
            }),
            pp(),
        );
        let dealloc_b = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_b,
                region_id: rid_b,
            }),
            pp(),
        );
        scg.add_edge(alloc_b, dealloc_b, EdgeKind::Derivation)
            .unwrap();

        let analysis = RegionAliasAnalysis::new(&scg);
        let regions = analysis.regions();

        let region_a = regions.iter().find(|r| r.entry_node == alloc_a).unwrap();
        let region_b = regions.iter().find(|r| r.entry_node == alloc_b).unwrap();

        // Cannot merge: region A has a security boundary
        assert!(
            !analysis.can_merge(&scg, region_a.id, region_b.id),
            "should not merge when one region has a security boundary"
        );
    }

    // ── RegionAliasAnalysis: cannot merge ancestor/descendant ────────

    #[test]
    fn test_cannot_merge_ancestor_descendant() {
        let mut scg = SCG::new();

        // Outer region
        let alloc_outer = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 16,
                region_id: RegionId::new(1),
                type_name: None,
            }),
            pp(),
        );
        // Inner region
        let alloc_inner = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: RegionId::new(2),
                type_name: None,
            }),
            pp(),
        );
        let dealloc_inner = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_inner,
                region_id: RegionId::new(2),
            }),
            pp(),
        );
        let dealloc_outer = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_outer,
                region_id: RegionId::new(1),
            }),
            pp(),
        );

        scg.add_edge(alloc_outer, alloc_inner, EdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(alloc_inner, dealloc_inner, EdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(dealloc_inner, dealloc_outer, EdgeKind::DataFlow)
            .unwrap();

        let analysis = RegionAliasAnalysis::new(&scg);
        let regions = analysis.regions();

        let outer = regions
            .iter()
            .find(|r| r.entry_node == alloc_outer)
            .unwrap();
        let inner = regions
            .iter()
            .find(|r| r.entry_node == alloc_inner)
            .unwrap();

        // Cannot merge ancestor and descendant
        assert!(
            !analysis.can_merge(&scg, outer.id, inner.id),
            "should not merge ancestor and descendant regions"
        );
    }

    // ── RegionAliasAnalysis: may_alias same region ───────────────────

    #[test]
    fn test_may_alias_same_region() {
        let mut scg = SCG::new();

        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 32,
                align: 8,
                region_id: RegionId::new(1),
                type_name: None,
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: RegionId::new(1),
            }),
            pp(),
        );
        scg.add_edge(alloc, dealloc, EdgeKind::DataFlow).unwrap();

        let analysis = RegionAliasAnalysis::new(&scg);
        let region_id = analysis.regions()[0].id;

        // A region always aliases with itself
        assert!(analysis.may_alias(&scg, region_id, region_id));
    }

    // ── RegionAliasAnalysis: cannot merge overlapping lifetimes ──────

    #[test]
    fn test_cannot_merge_overlapping_lifetimes() {
        let mut scg = SCG::new();

        // Region A: alloc_a -> comp -> dealloc_a
        let alloc_a = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 32,
                align: 8,
                region_id: RegionId::new(1),
                type_name: None,
            }),
            pp(),
        );
        let dealloc_a = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_a,
                region_id: RegionId::new(1),
            }),
            pp(),
        );
        scg.add_edge(alloc_a, dealloc_a, EdgeKind::Derivation)
            .unwrap();

        // Region B: alloc_b -> comp -> dealloc_b
        // B is entirely inside A's lifetime (overlapping)
        let alloc_b = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 32,
                align: 8,
                region_id: RegionId::new(2),
                type_name: None,
            }),
            pp(),
        );
        let dealloc_b = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_b,
                region_id: RegionId::new(2),
            }),
            pp(),
        );

        // Connect: alloc_a -> alloc_b -> dealloc_b -> dealloc_a
        // This makes A's lifetime overlap with B's, and they're not nested
        // (because we're not creating a nested alloc/dealloc pattern)
        scg.add_edge(alloc_a, alloc_b, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(alloc_b, dealloc_b, EdgeKind::Derivation)
            .unwrap();
        scg.add_edge(dealloc_b, dealloc_a, EdgeKind::ControlFlow)
            .unwrap();

        let analysis = RegionAliasAnalysis::new(&scg);
        let regions = analysis.regions();

        // The regions will have overlapping lifetimes and share DataFlow/ControlFlow edges
        // They may alias, so cannot merge
        let rid_a = regions.iter().find(|r| r.entry_node == alloc_a).unwrap().id;
        let rid_b = regions.iter().find(|r| r.entry_node == alloc_b).unwrap().id;

        // These regions should may-alias (lifetimes overlap, DataFlow crosses)
        assert!(
            analysis.may_alias(&scg, rid_a, rid_b),
            "overlapping lifetimes should cause may-alias"
        );

        // And therefore cannot merge
        assert!(
            !analysis.can_merge(&scg, rid_a, rid_b),
            "overlapping lifetime regions that may-alias should not be mergeable"
        );
    }
}
