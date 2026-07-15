//! Profile-guided optimization engine for the Continuous Optimization Runtime.
//!
//! This module defines the [`OptimizationEngine`] and the [`OptimizationPass`]
//! trait, along with four concrete passes that apply profile-guided
//! transformations to the Semantic Computation Graph (SCG):
//!
//! - [`HotPathInlining`] — inlines frequently-called function nodes.
//! - [`ColdPathOutline`] — moves rarely-executed code to separate out-of-line
//!   functions, improving instruction-cache utilisation on hot paths.
//! - [`LoopOptimization`] — unrolls hot loops and, where possible, emits
//!   SIMD/vectorized instructions.
//! - [`MemoryOptimization`] — inserts prefetch hints and aligns data to
//!   cache-line boundaries, targeting the L1/L2 cache hierarchy.
//!
//! The top-level function [`apply_optimizations`] runs all registered passes
//! over the SCG and returns an [`OptimizationResult`] summarising what was
//! done.

use crate::config::{Config, TargetArch};
use crate::profile::ProfileData;
use crate::types::{EdgeId, NodeId, NodeKind, SCG, SCGEdge, SCGNode};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Internal helpers — graph manipulation primitives
// ---------------------------------------------------------------------------
//
// The SCG model in `types.rs` exposes `nodes` / `edges` as `HashMap`s with
// public fields but provides no mutation helpers beyond `insert_node` /
// `insert_edge`. The real optimisation passes implemented in Wave 37 need
// to redirect and remove edges while keeping each endpoint's `incoming_edges`
// / `outgoing_edges` lists in sync. These free functions provide those
// primitives so every pass can perform genuine structural transforms.

/// Allocates the next available node ID in the SCG.
fn next_node_id(scg: &SCG) -> NodeId {
    scg.nodes.keys().max().copied().unwrap_or(0) + 1
}

/// Allocates the next available edge ID in the SCG.
fn next_edge_id(scg: &SCG) -> EdgeId {
    scg.edges.keys().max().copied().unwrap_or(0) + 1
}

/// Inserts a new edge between `source` and `target`, updating both endpoints'
/// edge lists. Returns the new edge ID.
fn add_edge(scg: &mut SCG, source: NodeId, target: NodeId, weight: u64) -> EdgeId {
    let eid = next_edge_id(scg);
    let edge = SCGEdge {
        id: eid,
        source,
        target,
        weight,
    };
    scg.insert_edge(edge);
    if let Some(n) = scg.get_node_mut(source) {
        n.outgoing_edges.push(eid);
    }
    if let Some(n) = scg.get_node_mut(target) {
        n.incoming_edges.push(eid);
    }
    eid
}

/// Removes an edge from the SCG, updating both endpoints' edge lists.
fn remove_edge(scg: &mut SCG, edge_id: EdgeId) {
    if let Some(edge) = scg.edges.remove(&edge_id) {
        if let Some(n) = scg.get_node_mut(edge.source) {
            n.outgoing_edges.retain(|&e| e != edge_id);
        }
        if let Some(n) = scg.get_node_mut(edge.target) {
            n.incoming_edges.retain(|&e| e != edge_id);
        }
        scg.edge_count = scg.edges.len();
    }
}

/// Returns all edge IDs whose source is `node_id`, computed from the SCG's
/// edge map (not from the node's `outgoing_edges` list, which may be
/// incomplete if edges were inserted via `insert_edge` without updating the
/// node's edge lists).
fn outgoing_edge_ids(scg: &SCG, node_id: NodeId) -> Vec<EdgeId> {
    scg.edges
        .values()
        .filter(|e| e.source == node_id)
        .map(|e| e.id)
        .collect()
}

/// Returns all edge IDs whose target is `node_id`, computed from the SCG's
/// edge map.
fn incoming_edge_ids(scg: &SCG, node_id: NodeId) -> Vec<EdgeId> {
    scg.edges
        .values()
        .filter(|e| e.target == node_id)
        .map(|e| e.id)
        .collect()
}

/// Redirects an edge's target to `new_target`, updating endpoint edge lists.
fn redirect_edge_target(scg: &mut SCG, edge_id: EdgeId, new_target: NodeId) {
    let old_target = {
        let edge = match scg.edges.get_mut(&edge_id) {
            Some(e) => e,
            None => return,
        };
        let old = edge.target;
        edge.target = new_target;
        old
    };
    if let Some(n) = scg.get_node_mut(old_target) {
        n.incoming_edges.retain(|&e| e != edge_id);
    }
    if let Some(n) = scg.get_node_mut(new_target) {
        n.incoming_edges.push(edge_id);
    }
}

/// Returns the set of nodes reachable from `start`'s outgoing edges, excluding
/// `start` itself and any nodes in `exclude`. Traversal follows outgoing edges
/// only and avoids cycles via a visited set.
///
/// NOTE: traversal uses `scg.edges` (not the node's `outgoing_edges` list)
/// because `insert_edge` does not automatically update node edge lists — some
/// SCGs in the wild have edges whose endpoints' edge lists are not populated.
fn reachable_body(scg: &SCG, start: NodeId, exclude: &HashSet<NodeId>) -> Vec<NodeId> {
    let mut visited: HashSet<NodeId> = exclude.clone();
    visited.insert(start);
    let mut result: Vec<NodeId> = Vec::new();
    let mut stack: Vec<NodeId> = Vec::new();

    // Seed: targets of edges leaving `start`.
    for edge in scg.edges.values() {
        if edge.source == start && !visited.contains(&edge.target) {
            visited.insert(edge.target);
            result.push(edge.target);
            stack.push(edge.target);
        }
    }

    // BFS via edges.
    while let Some(nid) = stack.pop() {
        for edge in scg.edges.values() {
            if edge.source == nid && !visited.contains(&edge.target) {
                visited.insert(edge.target);
                result.push(edge.target);
                stack.push(edge.target);
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// ProfileReport — digest of profile data for the optimiser
// ---------------------------------------------------------------------------

/// A summarised profile report consumed by optimisation passes.
///
/// `ProfileReport` is constructed from the raw [`ProfileData`] and provides
/// pre-classified hot/cold node lists, loop back-edge frequencies, and
/// allocation hotspots so that each pass does not have to recompute these
/// from scratch.
#[derive(Debug, Clone)]
pub struct ProfileReport {
    /// Nodes sorted by call count, descending (hottest first).
    pub hot_nodes: Vec<(NodeId, u64)>,
    /// Nodes whose call count is below the cold threshold.
    pub cold_nodes: Vec<(NodeId, u64)>,
    /// Loop back-edges and their traversal counts.
    /// Each entry is `(edge_id, source_node, target_node, weight)`.
    pub loop_back_edges: Vec<(EdgeId, NodeId, NodeId, u64)>,
    /// Allocation hotspots by region class.
    pub allocation_hotspots: Vec<(String, u64)>,
    /// Total number of profile samples recorded.
    pub total_samples: u64,
}

impl ProfileReport {
    /// Hotness threshold: nodes called more than this are "hot".
    const HOT_THRESHOLD: u64 = 100;
    /// Coldness threshold: nodes called fewer than this are "cold".
    const COLD_THRESHOLD: u64 = 5;

    /// Builds a `ProfileReport` from raw [`ProfileData`].
    ///
    /// The constructor sorts nodes into hot/cold buckets and identifies loop
    /// back-edges (edges whose `weight` significantly exceeds the average
    /// edge weight in the SCG).
    pub fn from_profile_data(profile: &ProfileData, scg: &SCG) -> Self {
        let mut all_nodes: Vec<(NodeId, u64)> =
            profile.call_counts.iter().map(|(&n, &c)| (n, c)).collect();
        all_nodes.sort_by_key(|b| std::cmp::Reverse(b.1));

        let hot_nodes: Vec<(NodeId, u64)> = all_nodes
            .iter()
            .filter(|(_, c)| *c > Self::HOT_THRESHOLD)
            .cloned()
            .collect();

        let cold_nodes: Vec<(NodeId, u64)> = all_nodes
            .iter()
            .filter(|(_, c)| *c < Self::COLD_THRESHOLD)
            .cloned()
            .collect();

        // Identify loop back-edges: edges where source > target (simple
        // heuristic) or where the weight is at least 10× the median.
        let avg_weight = if scg.edges.is_empty() {
            1
        } else {
            let total: u64 = scg.edges.values().map(|e| e.weight).sum();
            total / scg.edges.len().max(1) as u64
        };

        let loop_back_edges: Vec<(EdgeId, NodeId, NodeId, u64)> = scg
            .edges
            .values()
            .filter(|e| e.weight > avg_weight * 10 || e.target <= e.source)
            .map(|e| (e.id, e.source, e.target, e.weight))
            .collect();

        let allocation_hotspots: Vec<(String, u64)> = profile
            .allocation_stats
            .by_region_class
            .iter()
            .filter(|(_, count)| **count > 50)
            .map(|(k, &v)| (k.clone(), v))
            .collect();

        let total_samples: u64 = profile.call_counts.values().sum();

        ProfileReport {
            hot_nodes,
            cold_nodes,
            loop_back_edges,
            allocation_hotspots,
            total_samples,
        }
    }

    /// Returns `true` if the given node is classified as hot.
    pub fn is_hot(&self, node_id: NodeId) -> bool {
        self.hot_nodes.iter().any(|(n, _)| *n == node_id)
    }

    /// Returns `true` if the given node is classified as cold.
    pub fn is_cold(&self, node_id: NodeId) -> bool {
        self.cold_nodes.iter().any(|(n, _)| *n == node_id)
    }

    /// Returns the call count for the given node, or 0 if not profiled.
    pub fn call_count(&self, node_id: NodeId) -> u64 {
        self.hot_nodes
            .iter()
            .chain(self.cold_nodes.iter())
            .find(|(n, _)| *n == node_id)
            .map(|(_, c)| *c)
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Transformation record
// ---------------------------------------------------------------------------

/// The kind of optimisation transformation applied to a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransformationKind {
    /// The node was inlined at its call site(s).
    Inlined,
    /// The node was outlined to a separate cold function.
    Outlined,
    /// A loop was unrolled by the given factor.
    LoopUnrolled {
        /// Unroll factor applied.
        factor: u32,
    },
    /// A loop was vectorized using SIMD instructions.
    LoopVectorized,
    /// A prefetch instruction was inserted before a memory access.
    PrefetchInserted,
    /// Data was aligned to a cache-line boundary.
    CacheLineAligned {
        /// Alignment in bytes.
        alignment: usize,
    },
}

impl std::fmt::Display for TransformationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransformationKind::Inlined => write!(f, "Inlined"),
            TransformationKind::Outlined => write!(f, "Outlined"),
            TransformationKind::LoopUnrolled { factor } => {
                write!(f, "LoopUnrolled(×{})", factor)
            }
            TransformationKind::LoopVectorized => write!(f, "LoopVectorized"),
            TransformationKind::PrefetchInserted => write!(f, "PrefetchInserted"),
            TransformationKind::CacheLineAligned { alignment } => {
                write!(f, "CacheLineAligned({}B)", alignment)
            }
        }
    }
}

/// A record of a single optimisation applied to a specific node.
#[derive(Debug, Clone)]
pub struct Transformation {
    /// The kind of transformation.
    pub kind: TransformationKind,
    /// The node that was transformed.
    pub target_node: NodeId,
    /// Human-readable explanation of why the transformation was applied.
    pub description: String,
}

// ---------------------------------------------------------------------------
// PassResult — output of a single optimisation pass
// ---------------------------------------------------------------------------

/// The result of running one [`OptimizationPass`].
#[derive(Debug, Clone)]
pub struct PassResult {
    /// Name of the pass that produced this result.
    pub pass_name: String,
    /// Individual transformations applied.
    pub transformations: Vec<Transformation>,
    /// Estimated speedup factor (1.0 = no improvement).
    pub estimated_speedup: f64,
}

impl PassResult {
    /// Creates an empty result for the given pass name.
    pub fn empty(pass_name: &str) -> Self {
        PassResult {
            pass_name: pass_name.to_owned(),
            transformations: Vec::new(),
            estimated_speedup: 1.0,
        }
    }

    /// Returns the number of transformations applied.
    pub fn count(&self) -> usize {
        self.transformations.len()
    }
}

// ---------------------------------------------------------------------------
// OptimizationResult — aggregate output of all passes
// ---------------------------------------------------------------------------

/// The aggregate result of applying all optimisation passes.
#[derive(Debug, Clone)]
pub struct OptimizationResult {
    /// Per-pass results, in execution order.
    pub pass_results: Vec<PassResult>,
    /// Total number of transformations across all passes.
    pub total_transformations: usize,
    /// Combined estimated speedup factor (1.0 = no improvement).
    pub estimated_speedup: f64,
}

impl OptimizationResult {
    /// Creates a result from the list of per-pass results.
    pub fn from_pass_results(pass_results: Vec<PassResult>) -> Self {
        let total_transformations: usize = pass_results.iter().map(|r| r.count()).sum();
        // Combine speedups multiplicatively (a simplification).
        let estimated_speedup: f64 = pass_results
            .iter()
            .fold(1.0, |acc, r| acc * r.estimated_speedup);

        OptimizationResult {
            pass_results,
            total_transformations,
            estimated_speedup,
        }
    }

    /// Returns an empty result (no passes run, no transformations).
    pub fn empty() -> Self {
        OptimizationResult {
            pass_results: Vec::new(),
            total_transformations: 0,
            estimated_speedup: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// OptimizationPass trait
// ---------------------------------------------------------------------------

/// A single optimisation pass that can be applied to the SCG.
///
/// Implementations inspect the profile report, identify optimisation
/// opportunities, and mutate the SCG accordingly. Each pass returns a
/// [`PassResult`] describing what was done.
pub trait OptimizationPass {
    /// Human-readable name of the pass (used in logging and results).
    fn name(&self) -> &str;

    /// Apply the pass to the SCG, guided by the profile report.
    ///
    /// Returns a [`PassResult`] describing transformations applied and an
    /// estimated speedup factor.
    fn apply(&self, scg: &mut SCG, profile: &ProfileReport) -> PassResult;
}

// ---------------------------------------------------------------------------
// HotPathInlining
// ---------------------------------------------------------------------------

/// Inlines hot call sites into their callers.
///
/// When a [`Call`](NodeKind::Call) node is identified as hot by the profile
/// report and its compiled code is small enough (below `max_inline_size`),
/// this pass marks the node as inlined, eliminating the call overhead and
/// enabling further intra-procedural optimisations.
pub struct HotPathInlining {
    /// Maximum code size (in bytes) of a callee that is safe to inline.
    /// Larger functions are not inlined to avoid code bloat.
    pub max_inline_size: usize,
}

impl HotPathInlining {
    /// Default maximum inline size: 256 bytes (roughly 64 AArch64 instructions).
    pub const DEFAULT_MAX_INLINE_SIZE: usize = 256;

    /// Creates a new `HotPathInlining` pass with default settings.
    pub fn new() -> Self {
        HotPathInlining {
            max_inline_size: Self::DEFAULT_MAX_INLINE_SIZE,
        }
    }

    /// Creates a new `HotPathInlining` pass with a custom max inline size.
    pub fn with_max_inline_size(mut self, size: usize) -> Self {
        self.max_inline_size = size;
        self
    }
}

impl Default for HotPathInlining {
    fn default() -> Self {
        Self::new()
    }
}

impl OptimizationPass for HotPathInlining {
    fn name(&self) -> &str {
        "HotPathInlining"
    }

    fn apply(&self, scg: &mut SCG, profile: &ProfileReport) -> PassResult {
        let mut result = PassResult::empty(self.name());

        // Snapshot candidate hot Call nodes (avoids holding a mutable borrow
        // while we later mutate the SCG).
        let candidates: Vec<(NodeId, u64)> = profile
            .hot_nodes
            .iter()
            .filter_map(|&(id, count)| {
                let node = scg.get_node(id)?;
                if node.kind != NodeKind::Call || node.is_inlined {
                    return None;
                }
                if node.code_size > self.max_inline_size {
                    vuma_log!(debug, 
                        "HotPathInlining: skipping node {} — code_size {} exceeds limit {}",
                        id,
                        node.code_size,
                        self.max_inline_size,
                    );
                    return None;
                }
                Some((id, count))
            })
            .collect();

        for (node_id, call_count) in candidates {
            // Identify the callee body: nodes reachable from the call's
            // outgoing edges (excluding the call node itself).
            let body = reachable_body(scg, node_id, &HashSet::new());

            // Snapshot the call node's incoming/outgoing edges before mutation.
            // Use scg.edges (not node edge lists) for robustness.
            let incoming: Vec<EdgeId> = incoming_edge_ids(scg, node_id);
            let outgoing: Vec<EdgeId> = outgoing_edge_ids(scg, node_id);

            let saved_call_overhead = call_count as f64 * 5.0; // ~5 cycles per call
            result.estimated_speedup +=
                saved_call_overhead / profile.total_samples.max(1) as f64;

            if body.is_empty() {
                // No body to inline — annotation-only fallback.
                if let Some(n) = scg.get_node_mut(node_id) {
                    n.is_inlined = true;
                }
                result.transformations.push(Transformation {
                    kind: TransformationKind::Inlined,
                    target_node: node_id,
                    description: format!(
                        "Inlined hot call node (called {}×, code_size={}B, no body)",
                        call_count,
                        scg.get_node(node_id).map(|n| n.code_size).unwrap_or(0),
                    ),
                });
                continue;
            }

            // Allocate fresh IDs for the body clones.
            let base_id = next_node_id(scg);
            let mut clone_map: HashMap<NodeId, NodeId> = HashMap::new();
            for (i, &orig) in body.iter().enumerate() {
                clone_map.insert(orig, base_id + i as u64);
            }

            // Clone each body node with its new ID (edge lists rebuilt below).
            for &orig in &body {
                let mut clone = scg.get_node(orig).unwrap().clone();
                clone.id = clone_map[&orig];
                clone.incoming_edges.clear();
                clone.outgoing_edges.clear();
                scg.insert_node(clone);
            }

            // Clone internal edges (source ∈ body, target ∈ body).
            // Snapshot from scg.edges to avoid borrow conflicts during add_edge.
            let internal_edges: Vec<(NodeId, NodeId, u64)> = scg
                .edges
                .values()
                .filter(|e| body.contains(&e.source) && body.contains(&e.target))
                .map(|e| (e.source, e.target, e.weight))
                .collect();
            for (src, tgt, weight) in internal_edges {
                add_edge(scg, clone_map[&src], clone_map[&tgt], weight);
            }

            // Clone exit edges (body node → outside-the-body node) so the
            // inlined body's exits reach the original continuation.
            let exit_edges: Vec<(NodeId, NodeId, u64)> = scg
                .edges
                .values()
                .filter(|e| body.contains(&e.source) && !body.contains(&e.target))
                .map(|e| (e.source, e.target, e.weight))
                .collect();
            for (src, tgt, weight) in exit_edges {
                add_edge(scg, clone_map[&src], tgt, weight);
            }

            // Body entry clone = clone of the first body node reached from
            // the call's outgoing edges.
            let body_entry_clone: Option<NodeId> = scg
                .edges
                .values()
                .find(|e| e.source == node_id && clone_map.contains_key(&e.target))
                .map(|e| clone_map[&e.target]);

            // Redirect the caller's edges to the inlined body entry.
            if let Some(entry) = body_entry_clone {
                for &eid in &incoming {
                    redirect_edge_target(scg, eid, entry);
                }
            }

            // Remove the call edges (call node → body) — the call edge is gone.
            for &eid in &outgoing {
                remove_edge(scg, eid);
            }

            // Mark the call node as inlined (now orphaned in the graph).
            if let Some(n) = scg.get_node_mut(node_id) {
                n.is_inlined = true;
            }

            result.transformations.push(Transformation {
                kind: TransformationKind::Inlined,
                target_node: node_id,
                description: format!(
                    "Inlined hot call node (called {}×, code_size={}B, body={} nodes cloned)",
                    call_count,
                    scg.get_node(node_id).map(|n| n.code_size).unwrap_or(0),
                    body.len(),
                ),
            });
        }

        // Ensure speedup is at least 1.0 (no regression).
        result.estimated_speedup = result.estimated_speedup.max(1.0);

        vuma_log!(info, 
            "HotPathInlining: {} transformations, estimated speedup {:.3}×",
            result.count(),
            result.estimated_speedup,
        );

        result
    }
}

// ---------------------------------------------------------------------------
// ColdPathOutline
// ---------------------------------------------------------------------------

/// Moves cold code to separate out-of-line functions.
///
/// When a node is classified as cold by the profile report and resides on
/// a path that also contains hot nodes, outlining it shrinks the hot path
/// and improves instruction-cache utilisation.
pub struct ColdPathOutline {
    /// Whether to also outline nodes that share an edge with a hot node
    /// even if they are not directly cold themselves (rarely-taken branches).
    pub outline_adjacent_to_hot: bool,
}

impl ColdPathOutline {
    /// Creates a new `ColdPathOutline` pass with default settings.
    pub fn new() -> Self {
        ColdPathOutline {
            outline_adjacent_to_hot: true,
        }
    }
}

impl Default for ColdPathOutline {
    fn default() -> Self {
        Self::new()
    }
}

impl OptimizationPass for ColdPathOutline {
    fn name(&self) -> &str {
        "ColdPathOutline"
    }

    fn apply(&self, scg: &mut SCG, profile: &ProfileReport) -> PassResult {
        let mut result = PassResult::empty(self.name());

        // Collect node IDs to outline (snapshot first, mutate second to avoid
        // holding a mutable borrow while iterating over profile.cold_nodes).
        let mut to_outline: Vec<(NodeId, String)> = Vec::new();

        for &(node_id, call_count) in &profile.cold_nodes {
            let node = match scg.get_node(node_id) {
                Some(n) => n,
                None => continue,
            };

            // Only outline nodes that are on the same path as hot nodes.
            let adjacent_to_hot = if self.outline_adjacent_to_hot {
                node.incoming_edges
                    .iter()
                    .chain(node.outgoing_edges.iter())
                    .any(|&eid| {
                        scg.get_edge(eid)
                            .is_some_and(|e| profile.is_hot(e.source) || profile.is_hot(e.target))
                    })
            } else {
                false
            };

            if !adjacent_to_hot && !profile.is_hot(node_id) {
                // Cold node not adjacent to any hot node — less benefit.
                continue;
            }

            to_outline.push((
                node_id,
                format!(
                    "Outlined cold node (called {}×, adjacent to hot path)",
                    call_count,
                ),
            ));
        }

        // Also outline Branch nodes on cold paths adjacent to hot nodes.
        if self.outline_adjacent_to_hot {
            let branch_ids: Vec<NodeId> = scg
                .nodes
                .values()
                .filter(|n| n.kind == NodeKind::Branch && !n.is_outlined)
                .map(|n| n.id)
                .collect();
            for node_id in branch_ids {
                if to_outline.iter().any(|(id, _)| *id == node_id) {
                    continue;
                }
                let node = match scg.get_node(node_id) {
                    Some(n) => n,
                    None => continue,
                };
                // Check if this branch is adjacent to a hot node but is itself cold.
                let adjacent_hot = node
                    .incoming_edges
                    .iter()
                    .chain(node.outgoing_edges.iter())
                    .any(|&eid| {
                        scg.get_edge(eid)
                            .is_some_and(|e| profile.is_hot(e.source) || profile.is_hot(e.target))
                    });
                if adjacent_hot && profile.is_cold(node_id) {
                    to_outline.push((
                        node_id,
                        "Outlined cold branch adjacent to hot path".to_owned(),
                    ));
                }
            }
        }

        // For each cold node, create a synthetic FunctionEntry node that
        // represents the outlined function, redirect the caller's edge to
        // call the function entry, and add an edge from the function entry
        // to the cold node (which becomes the function's body). This is a
        // real structural transform: the caller shrinks (its edge no longer
        // targets the cold node directly) and a new function node appears.
        for (node_id, description) in to_outline {
            let incoming: Vec<EdgeId> = incoming_edge_ids(scg, node_id);
            let cold_code_size = scg.get_node(node_id).map(|n| n.code_size).unwrap_or(0);

            // Create the synthetic function-entry node.
            let func_id = next_node_id(scg);
            let mut func_node = SCGNode::new(func_id, NodeKind::FunctionEntry);
            func_node.is_outlined = true;
            func_node.code_size = cold_code_size;
            func_node.control_label = Some(format!("outlined_func_for_{}", node_id));
            scg.insert_node(func_node);

            // Redirect each incoming edge of the cold node to the function
            // entry (so callers now "call" the outlined function).
            for &eid in &incoming {
                redirect_edge_target(scg, eid, func_id);
            }

            // Add an edge from the function entry to the cold node (the body).
            add_edge(scg, func_id, node_id, 1);

            // Mark the cold node as outlined.
            if let Some(n) = scg.get_node_mut(node_id) {
                n.is_outlined = true;
            }

            result.transformations.push(Transformation {
                kind: TransformationKind::Outlined,
                target_node: node_id,
                description,
            });
        }

        // Estimated speedup: each outlined node frees icache space.
        // Rough model: 2% per outlined node, capped at 20%.
        let outline_benefit = (result.count() as f64 * 0.02).min(0.20);
        result.estimated_speedup = 1.0 + outline_benefit;

        vuma_log!(info, 
            "ColdPathOutline: {} transformations, estimated speedup {:.3}×",
            result.count(),
            result.estimated_speedup,
        );

        result
    }
}

// ---------------------------------------------------------------------------
// LoopOptimization
// ---------------------------------------------------------------------------

/// Unrolls hot loops and, where profitable, vectorises them.
///
/// Loop unrolling reduces branch overhead and exposes instruction-level
/// parallelism. Vectorization replaces scalar loop bodies with SIMD
/// operations, which is especially effective on NEON-capable
/// AArch64 targets.
pub struct LoopOptimization {
    /// Minimum trip count (from profile) to consider a loop "hot" enough
    /// for unrolling.
    pub min_trip_count: u64,
    /// Maximum unroll factor to apply.
    pub max_unroll_factor: u32,
    /// Whether to attempt vectorisation.
    pub enable_vectorization: bool,
}

impl LoopOptimization {
    /// Creates a new `LoopOptimization` pass with default settings.
    pub fn new() -> Self {
        LoopOptimization {
            min_trip_count: 100,
            max_unroll_factor: 8,
            enable_vectorization: true,
        }
    }

    /// Creates a pass with a custom minimum trip count.
    pub fn with_min_trip_count(mut self, count: u64) -> Self {
        self.min_trip_count = count;
        self
    }

    /// Determines the best unroll factor for a loop with the given trip
    /// count. Chooses the largest power-of-two ≤ `max_unroll_factor` that
    /// does not exceed the trip count.
    fn best_unroll_factor(&self, trip_count: u64) -> u32 {
        let mut factor: u32 = 1;
        let mut candidate = 2;
        while candidate <= self.max_unroll_factor && (candidate as u64) <= trip_count {
            factor = candidate;
            candidate *= 2;
        }
        factor
    }

    /// Heuristic: determines whether a loop body is vectorizable.
    ///
    /// In the full implementation this would analyse data dependencies,
    /// stride patterns, and type widths. For now we use a simple heuristic:
    /// a loop is vectorizable if it has a Memory node among its successors
    /// and is not already vectorized.
    fn is_vectorizable(&self, scg: &SCG, loop_node_id: NodeId) -> bool {
        let loop_node = match scg.get_node(loop_node_id) {
            Some(n) => n,
            None => return false,
        };
        if loop_node.is_vectorized {
            return false;
        }
        // Check if any successor reachable through outgoing edges is a
        // Memory node (indicating a load/store pattern).
        let mut has_memory = false;
        for &eid in &loop_node.outgoing_edges {
            if let Some(edge) = scg.get_edge(eid) {
                if let Some(target) = scg.get_node(edge.target) {
                    if target.kind == NodeKind::Memory {
                        has_memory = true;
                        break;
                    }
                }
            }
        }
        has_memory
    }
}

impl Default for LoopOptimization {
    fn default() -> Self {
        Self::new()
    }
}

impl OptimizationPass for LoopOptimization {
    fn name(&self) -> &str {
        "LoopOptimization"
    }

    fn apply(&self, scg: &mut SCG, profile: &ProfileReport) -> PassResult {
        let mut result = PassResult::empty(self.name());

        // Build a map from loop-header node ID to estimated trip count
        // derived from back-edge weights.
        let mut loop_trip_counts: HashMap<NodeId, u64> = HashMap::new();
        for &(_eid, _src, target, weight) in &profile.loop_back_edges {
            let entry = loop_trip_counts.entry(target).or_insert(0);
            *entry = (*entry).max(weight);
        }

        // Also consider Loop-kind nodes that are hot in the profile.
        for &(node_id, call_count) in &profile.hot_nodes {
            let node = match scg.get_node(node_id) {
                Some(n) => n,
                None => continue,
            };
            if node.kind == NodeKind::Loop || node.kind == NodeKind::LoopHeader {
                let entry = loop_trip_counts.entry(node_id).or_insert(0);
                *entry = (*entry).max(call_count);
            }
        }

        // Apply unrolling and vectorisation.
        let loop_nodes: Vec<NodeId> = loop_trip_counts.keys().cloned().collect();
        for loop_node_id in loop_nodes {
            let trip_count = loop_trip_counts[&loop_node_id];
            if trip_count < self.min_trip_count {
                continue;
            }

            // Snapshot loop-node metadata.
            let (node_kind, current_factor) = match scg.get_node(loop_node_id) {
                Some(n) => (n.kind, n.unroll_factor),
                None => continue,
            };
            if node_kind != NodeKind::Loop && node_kind != NodeKind::LoopHeader {
                continue;
            }

            let factor = self.best_unroll_factor(trip_count).max(current_factor);
            if factor <= 1 {
                continue;
            }

            // Identify the loop body: nodes reachable from the loop header's
            // outgoing edges, excluding the loop header itself.
            let mut exclude: HashSet<NodeId> = HashSet::new();
            exclude.insert(loop_node_id);
            let body = reachable_body(scg, loop_node_id, &exclude);

            // Body entry = target of the loop header's first outgoing edge
            // that points into the body. Use scg.edges for robustness.
            let body_entry: Option<NodeId> = scg
                .edges
                .values()
                .find(|e| e.source == loop_node_id && body.contains(&e.target))
                .map(|e| e.target);

            if body.is_empty() || body_entry.is_none() {
                // No body to unroll — annotation-only fallback.
                if current_factor < factor {
                    if let Some(n) = scg.get_node_mut(loop_node_id) {
                        n.unroll_factor = factor;
                    }
                    result.transformations.push(Transformation {
                        kind: TransformationKind::LoopUnrolled { factor },
                        target_node: loop_node_id,
                        description: format!(
                            "Unrolled loop (trip={}, factor {}→{}, no body)",
                            trip_count, current_factor, factor,
                        ),
                    });
                }
            } else if let Some(body_entry) = body_entry {
                // Snapshot back-edges (body node → loop header) from scg.edges.
                let back_edges: Vec<(EdgeId, NodeId, u64)> = scg
                    .edges
                    .values()
                    .filter(|e| body.contains(&e.source) && e.target == loop_node_id)
                    .map(|e| (e.id, e.source, e.weight))
                    .collect();

                // Snapshot exit edges (body node → non-body, non-header node).
                let exit_edges: Vec<(NodeId, NodeId, u64)> = scg
                    .edges
                    .values()
                    .filter(|e| {
                        body.contains(&e.source)
                            && e.target != loop_node_id
                            && !body.contains(&e.target)
                    })
                    .map(|e| (e.source, e.target, e.weight))
                    .collect();

                // Create (factor - 1) clones of the body, chained after the
                // original. Copy 0 = original body; copies 1..(F-1) are clones.
                let mut copy_clone_maps: Vec<HashMap<NodeId, NodeId>> = Vec::new();
                let mut identity: HashMap<NodeId, NodeId> = HashMap::new();
                for &orig in &body {
                    identity.insert(orig, orig);
                }
                copy_clone_maps.push(identity);

                for i in 1..factor {
                    let base_id = next_node_id(scg);
                    let mut clone_map: HashMap<NodeId, NodeId> = HashMap::new();
                    for (j, &orig) in body.iter().enumerate() {
                        clone_map.insert(orig, base_id + j as u64);
                    }
                    // Clone body nodes; mark each with an IV-iteration label
                    // so downstream passes can see which unroll copy it is.
                    for &orig in &body {
                        let mut clone = scg.get_node(orig).unwrap().clone();
                        clone.id = clone_map[&orig];
                        clone.incoming_edges.clear();
                        clone.outgoing_edges.clear();
                        clone.control_label = Some(format!("unroll_iter_{}", i));
                        scg.insert_node(clone);
                    }
                    // Clone internal edges (body → body) from scg.edges.
                    let internal_edges: Vec<(NodeId, NodeId, u64)> = scg
                        .edges
                        .values()
                        .filter(|e| body.contains(&e.source) && body.contains(&e.target))
                        .map(|e| (e.source, e.target, e.weight))
                        .collect();
                    for (src, tgt, weight) in internal_edges {
                        add_edge(scg, clone_map[&src], clone_map[&tgt], weight);
                    }
                    // Clone exit edges (body → outside).
                    for &(orig_src, ext_target, weight) in &exit_edges {
                        add_edge(scg, clone_map[&orig_src], ext_target, weight);
                    }
                    copy_clone_maps.push(clone_map);
                }

                // Entry of each copy.
                let copy_entries: Vec<NodeId> =
                    copy_clone_maps.iter().map(|m| m[&body_entry]).collect();

                // Chain back-edges:
                //   copy 0's back-edges → copy 1's entry
                //   copy 1's back-edges → copy 2's entry
                //   ...
                //   copy (F-1)'s back-edges → loop header
                if factor > 1 {
                    for &(eid, _, _) in &back_edges {
                        redirect_edge_target(scg, eid, copy_entries[1]);
                    }
                }
                for i in 1..(factor as usize) {
                    let next_target = if i + 1 < factor as usize {
                        copy_entries[i + 1]
                    } else {
                        loop_node_id
                    };
                    let clone_map = &copy_clone_maps[i];
                    for &(_, orig_src, weight) in &back_edges {
                        let clone_src = clone_map[&orig_src];
                        add_edge(scg, clone_src, next_target, weight);
                    }
                }

                // Set the unroll factor on the loop header.
                if let Some(n) = scg.get_node_mut(loop_node_id) {
                    n.unroll_factor = factor;
                }

                result.transformations.push(Transformation {
                    kind: TransformationKind::LoopUnrolled { factor },
                    target_node: loop_node_id,
                    description: format!(
                        "Unrolled loop (trip={}, factor {}→{}, body={} nodes × {} copies)",
                        trip_count,
                        current_factor,
                        factor,
                        body.len(),
                        factor,
                    ),
                });
            }

            // Vectorise.
            if self.enable_vectorization && self.is_vectorizable(scg, loop_node_id) {
                let node = scg.get_node_mut(loop_node_id).unwrap();
                if !node.is_vectorized {
                    node.is_vectorized = true;
                    result.transformations.push(Transformation {
                        kind: TransformationKind::LoopVectorized,
                        target_node: loop_node_id,
                        description: format!(
                            "Vectorized loop (trip={}, NEON/SIMD)",
                            trip_count,
                        ),
                    });
                }
            }
        }

        // Speedup model: unrolling gives ~factor/2 improvement up to 2×;
        // vectorisation gives ~4× for memory-bound loops.
        let unroll_speedup: f64 = result
            .transformations
            .iter()
            .filter(|t| matches!(t.kind, TransformationKind::LoopUnrolled { .. }))
            .map(|t| {
                if let TransformationKind::LoopUnrolled { factor } = t.kind {
                    (factor as f64 / 2.0).min(2.0)
                } else {
                    1.0
                }
            })
            .product::<f64>()
            .max(1.0);

        let vec_count = result
            .transformations
            .iter()
            .filter(|t| matches!(t.kind, TransformationKind::LoopVectorized))
            .count();

        // Each vectorised loop adds ~4× speedup for that loop body.
        let vec_speedup = if vec_count > 0 {
            1.0 + vec_count as f64 * 0.3
        } else {
            1.0
        };

        result.estimated_speedup = (unroll_speedup * vec_speedup).max(1.0);

        vuma_log!(info, 
            "LoopOptimization: {} transformations, estimated speedup {:.3}×",
            result.count(),
            result.estimated_speedup,
        );

        result
    }
}

// ---------------------------------------------------------------------------
// MemoryOptimization
// ---------------------------------------------------------------------------

/// Inserts prefetch hints and aligns data to cache-line boundaries.
///
/// This pass:
///
/// 1. Marks [`Memory`](NodeKind::Memory) nodes on hot paths with prefetch
///    instructions so data is loaded into L1 before it is needed.
/// 2. Aligns memory accesses to 64-byte boundaries to avoid cross-cache-line
///    loads that waste bandwidth.
pub struct MemoryOptimization {
    /// Cache-line size in bytes. Default: 64.
    pub cache_line_size: usize,
    /// L1 data cache size in bytes. Default: 65_536 (64 KB).
    pub l1d_size: usize,
    /// L2 cache size in bytes. Default: 524_288 (512 KB).
    pub l2_size: usize,
}

impl MemoryOptimization {
    /// Creates a new `MemoryOptimization` pass targeting the given architecture.
    pub fn new(target_arch: TargetArch) -> Self {
        let _ = target_arch; // architecture-specific tuning not yet applied
        MemoryOptimization {
            cache_line_size: 64,
            l1d_size: 65_536,
            l2_size: 524_288,
        }
    }

    /// Creates a pass with default x86-64 parameters.
    pub fn for_x86_64() -> Self {
        let mut pass = Self::new(TargetArch::X86_64);
        pass.l2_size = 1_048_576; // 1 MB typical L2
        pass
    }
}

impl OptimizationPass for MemoryOptimization {
    fn name(&self) -> &str {
        "MemoryOptimization"
    }

    fn apply(&self, scg: &mut SCG, profile: &ProfileReport) -> PassResult {
        let mut result = PassResult::empty(self.name());

        // Collect (node_id, is_hot) for Memory nodes.
        let memory_nodes: Vec<(NodeId, bool)> = scg
            .nodes
            .iter()
            .filter(|(_, n)| n.kind == NodeKind::Memory)
            .map(|(&id, _)| (id, profile.is_hot(id)))
            .collect();

        for (node_id, is_hot) in memory_nodes {
            if !is_hot {
                continue;
            }

            let already_prefetched =
                scg.get_node(node_id).map(|n| n.has_prefetch).unwrap_or(false);
            let old_alignment = scg.get_node(node_id).map(|n| n.alignment).unwrap_or(0);

            // Structural transform: insert a prefetch Memory node before the
            // hot memory node, redirect the memory node's incoming edges to
            // the prefetch, and add an edge prefetch → memory. This is a
            // real structural transform: a new node appears in the SCG and
            // the hot memory node's incoming edges are rewired through it.
            if !already_prefetched {
                let incoming: Vec<EdgeId> = incoming_edge_ids(scg, node_id);

                let prefetch_id = next_node_id(scg);
                let mut prefetch_node = SCGNode::new(prefetch_id, NodeKind::Memory);
                prefetch_node.has_prefetch = true;
                prefetch_node.alignment = self.cache_line_size;
                prefetch_node.code_size = 16; // prefetch instruction size
                prefetch_node.control_label = Some(format!("prefetch_for_{}", node_id));
                scg.insert_node(prefetch_node);

                // Redirect each incoming edge to the prefetch node.
                for &eid in &incoming {
                    redirect_edge_target(scg, eid, prefetch_id);
                }
                // Add edge prefetch → memory.
                add_edge(scg, prefetch_id, node_id, 1);

                // Mark the memory node as having a prefetch.
                if let Some(n) = scg.get_node_mut(node_id) {
                    n.has_prefetch = true;
                }

                result.transformations.push(Transformation {
                    kind: TransformationKind::PrefetchInserted,
                    target_node: node_id,
                    description: format!(
                        "Prefetch inserted for hot memory node (cache-line {}B, L1D {}KB)",
                        self.cache_line_size,
                        self.l1d_size / 1024,
                    ),
                });
            }

            // Align to cache-line boundary for hot paths.
            if old_alignment < self.cache_line_size {
                if let Some(n) = scg.get_node_mut(node_id) {
                    n.alignment = self.cache_line_size;
                }
                result.transformations.push(Transformation {
                    kind: TransformationKind::CacheLineAligned {
                        alignment: self.cache_line_size,
                    },
                    target_node: node_id,
                    description: format!(
                        "Aligned memory node {}→{}B (cache-line)",
                        old_alignment, self.cache_line_size,
                    ),
                });
            }
        }

        // Speedup model: prefetch reduces latency ~20% per access;
        // alignment avoids cross-line penalties ~10%.
        let prefetch_count = result
            .transformations
            .iter()
            .filter(|t| matches!(t.kind, TransformationKind::PrefetchInserted))
            .count();
        let align_count = result
            .transformations
            .iter()
            .filter(|t| matches!(t.kind, TransformationKind::CacheLineAligned { .. }))
            .count();

        let prefetch_benefit = (prefetch_count as f64 * 0.05).min(0.30);
        let align_benefit = (align_count as f64 * 0.02).min(0.15);
        result.estimated_speedup = 1.0 + prefetch_benefit + align_benefit;

        vuma_log!(info, 
            "MemoryOptimization: {} transformations, estimated speedup {:.3}×",
            result.count(),
            result.estimated_speedup,
        );

        result
    }
}

// ---------------------------------------------------------------------------
// OptimizationEngine
// ---------------------------------------------------------------------------

/// Applies a sequence of profile-guided optimisation passes to the SCG.
///
/// The engine holds a collection of [`OptimizationPass`] implementations and
/// a [`Config`]. Calling [`OptimizationEngine::run`] executes each pass in
/// order and returns an aggregate [`OptimizationResult`].
pub struct OptimizationEngine {
    passes: Vec<Box<dyn OptimizationPass>>,
    config: Config,
}

impl std::fmt::Debug for OptimizationEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OptimizationEngine")
            .field("pass_count", &self.passes.len())
            .field(
                "pass_names",
                &self.passes.iter().map(|p| p.name()).collect::<Vec<_>>(),
            )
            .field("config", &self.config)
            .finish()
    }
}

impl OptimizationEngine {
    /// Creates a new optimisation engine with the default set of passes
    /// appropriate for the given configuration.
    pub fn new(config: Config) -> Self {
        let mut engine = OptimizationEngine {
            passes: Vec::new(),
            config,
        };
        engine.register_default_passes();
        engine
    }

    /// Creates an engine with no passes registered (for custom setups).
    pub fn empty(config: Config) -> Self {
        OptimizationEngine {
            passes: Vec::new(),
            config,
        }
    }

    /// Registers the default set of passes based on the current config.
    fn register_default_passes(&mut self) {
        self.passes.push(Box::new(HotPathInlining::new()));

        self.passes.push(Box::new(ColdPathOutline::new()));

        self.passes.push(Box::new(LoopOptimization::new()));

        self.passes
            .push(Box::new(MemoryOptimization::new(self.config.target_arch)));
    }

    /// Adds a custom optimisation pass.
    pub fn add_pass(&mut self, pass: Box<dyn OptimizationPass>) {
        self.passes.push(pass);
    }

    /// Removes all registered passes.
    pub fn clear_passes(&mut self) {
        self.passes.clear();
    }

    /// Returns the number of registered passes.
    pub fn pass_count(&self) -> usize {
        self.passes.len()
    }

    /// Runs all registered passes over the SCG and returns the aggregate
    /// result.
    pub fn run(&self, scg: &mut SCG, profile: &ProfileReport) -> OptimizationResult {
        let mut pass_results = Vec::with_capacity(self.passes.len());

        for pass in &self.passes {
            vuma_log!(debug, "OptimizationEngine: running pass '{}'", pass.name());
            let result = pass.apply(scg, profile);
            pass_results.push(result);
        }

        OptimizationResult::from_pass_results(pass_results)
    }
}

// ---------------------------------------------------------------------------
// apply_optimizations — top-level convenience function
// ---------------------------------------------------------------------------

/// Applies all default optimisation passes to the SCG, guided by the
/// profile report.
///
/// This is the main entry point for the optimisation pipeline. It creates
/// an [`OptimizationEngine`] with the default passes for the given
/// configuration and runs them over the SCG.
///
/// # Arguments
///
/// * `scg` – The Semantic Computation Graph to optimise (mutated in place).
/// * `profile` – The profile report guiding optimisation decisions.
///
/// # Returns
///
/// An [`OptimizationResult`] summarising all transformations.
pub fn apply_optimizations(scg: &mut SCG, profile: &ProfileReport) -> OptimizationResult {
    apply_optimizations_with_config(scg, profile, &Config::default())
}

/// Applies all default optimisation passes with the given configuration.
pub fn apply_optimizations_with_config(
    scg: &mut SCG,
    profile: &ProfileReport,
    config: &Config,
) -> OptimizationResult {
    let engine = OptimizationEngine::new(config.clone());
    engine.run(scg, profile)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ProfileData;
    use crate::types::{SCGEdge, SCGNode};

    // -- Helpers ------------------------------------------------------------

    /// Builds a small SCG with a mix of node kinds and edges.
    fn build_test_scg() -> SCG {
        let mut scg = SCG::new();

        // Entry node
        let mut entry = SCGNode::new(1, NodeKind::Entry);
        entry.code_size = 32;
        entry.outgoing_edges.push(300);
        scg.insert_node(entry);

        // Hot call node
        let mut call_a = SCGNode::new(10, NodeKind::Call);
        call_a.code_size = 64;
        call_a.incoming_edges.push(300);
        call_a.outgoing_edges.push(100);
        scg.insert_node(call_a);

        // Another hot call node (too large to inline by default)
        let mut call_b = SCGNode::new(11, NodeKind::Call);
        call_b.code_size = 512; // > DEFAULT_MAX_INLINE_SIZE
        scg.insert_node(call_b);

        // Hot loop node
        let mut loop_node = SCGNode::new(20, NodeKind::Loop);
        loop_node.code_size = 128;
        loop_node.outgoing_edges.push(200); // forward edge to memory body
        loop_node.incoming_edges.push(201); // back-edge from memory
        scg.insert_node(loop_node);

        // Memory node inside the loop
        let mut mem_node = SCGNode::new(30, NodeKind::Memory);
        mem_node.code_size = 64;
        mem_node.incoming_edges.push(200);
        mem_node.outgoing_edges.push(201); // back-edge to loop header
        scg.insert_node(mem_node);

        // Cold branch node
        let mut cold_branch = SCGNode::new(40, NodeKind::Branch);
        cold_branch.code_size = 32;
        cold_branch.incoming_edges.push(100);
        scg.insert_node(cold_branch);

        // Cold compute node
        let mut cold_compute = SCGNode::new(50, NodeKind::Compute);
        cold_compute.code_size = 16;
        scg.insert_node(cold_compute);

        // Edges
        scg.insert_edge(SCGEdge::new(100, 10, 40)); // call → cold branch
        scg.insert_edge(SCGEdge::new(200, 20, 30)); // loop → memory (forward)
        scg.insert_edge(SCGEdge {
            id: 201,
            source: 30,
            target: 20,
            weight: 5000,
        }); // memory → loop (back-edge)
        scg.insert_edge(SCGEdge::new(300, 1, 10)); // entry → call

        scg
    }

    /// Builds a ProfileData that will classify nodes 10, 20, 30 as hot,
    /// nodes 40, 50 as cold.
    fn build_test_profile_data() -> ProfileData {
        let mut profile = ProfileData::new();
        // Hot nodes
        for _ in 0..500 {
            profile.record_access(10);
        }
        for _ in 0..300 {
            profile.record_access(20);
        }
        for _ in 0..200 {
            profile.record_access(30);
        }
        // Cold nodes
        profile.record_access(40);
        profile.record_access(50);
        profile
    }

    fn build_test_profile_report(scg: &SCG, profile_data: &ProfileData) -> ProfileReport {
        ProfileReport::from_profile_data(profile_data, scg)
    }

    // -- Test 1: HotPathInlining inlines hot Call nodes ---------------------

    #[test]
    fn hot_path_inlining_inlines_hot_calls() {
        let mut scg = build_test_scg();
        let profile_data = build_test_profile_data();
        let report = build_test_profile_report(&scg, &profile_data);

        let pass = HotPathInlining::new();
        let result = pass.apply(&mut scg, &report);

        // Node 10 is a hot Call with code_size=64 < 256 → should be inlined.
        assert!(
            scg.get_node(10).unwrap().is_inlined,
            "hot Call node 10 should be inlined"
        );
        // Node 11 is a hot Call with code_size=512 > 256 → should NOT be inlined.
        assert!(
            !scg.get_node(11).unwrap().is_inlined,
            "large Call node 11 should NOT be inlined"
        );
        assert!(result.count() >= 1);
        assert!(result.transformations.iter().any(|t| t.target_node == 10));
    }

    // -- Test 2: HotPathInlining respects max_inline_size -------------------

    #[test]
    fn hot_path_inlining_respects_size_limit() {
        let mut scg = SCG::new();
        let mut call_node = SCGNode::new(1, NodeKind::Call);
        call_node.code_size = 128;
        scg.insert_node(call_node);

        let mut profile_data = ProfileData::new();
        for _ in 0..200 {
            profile_data.record_access(1);
        }
        let report = ProfileReport::from_profile_data(&profile_data, &scg);

        // With max_inline_size = 64, the node should NOT be inlined.
        let pass = HotPathInlining::new().with_max_inline_size(64);
        let result = pass.apply(&mut scg, &report);
        assert!(!scg.get_node(1).unwrap().is_inlined);
        assert_eq!(result.count(), 0);
    }

    // -- Test 3: ColdPathOutline outlines cold nodes adjacent to hot -------

    #[test]
    fn cold_path_outline_outlines_cold_adjacent_to_hot() {
        let mut scg = build_test_scg();
        let profile_data = build_test_profile_data();
        let report = build_test_profile_report(&scg, &profile_data);

        let pass = ColdPathOutline::new();
        let result = pass.apply(&mut scg, &report);

        // Node 40 is a cold Branch with an incoming edge from hot node 10.
        // It should be outlined.
        assert!(
            scg.get_node(40).unwrap().is_outlined,
            "cold branch node 40 adjacent to hot path should be outlined"
        );
        assert!(result.count() >= 1);
    }

    // -- Test 4: ColdPathOutline skips isolated cold nodes ------------------

    #[test]
    fn cold_path_outline_skips_isolated_cold() {
        let mut scg = SCG::new();
        // A cold compute node with no edges to any hot node.
        let cold = SCGNode::new(99, NodeKind::Compute);
        scg.insert_node(cold);

        let mut profile_data = ProfileData::new();
        profile_data.record_access(99); // only 1 access → cold

        let report = ProfileReport::from_profile_data(&profile_data, &scg);

        let pass = ColdPathOutline::new();
        let result = pass.apply(&mut scg, &report);

        // Node 99 is cold but not adjacent to any hot node → should NOT be outlined.
        assert!(!scg.get_node(99).unwrap().is_outlined);
        assert_eq!(result.count(), 0);
    }

    // -- Test 5: LoopOptimization unrolls hot loops -------------------------

    #[test]
    fn loop_optimization_unrolls_hot_loops() {
        let mut scg = build_test_scg();
        let profile_data = build_test_profile_data();
        let report = build_test_profile_report(&scg, &profile_data);

        let pass = LoopOptimization::new().with_min_trip_count(50);
        let result = pass.apply(&mut scg, &report);

        // Node 20 is a Loop node with back-edge weight 5000 → should be unrolled.
        let loop_node = scg.get_node(20).unwrap();
        assert!(
            loop_node.unroll_factor > 1,
            "hot loop node 20 should be unrolled (factor={})",
            loop_node.unroll_factor
        );
        assert!(result
            .transformations
            .iter()
            .any(|t| { matches!(t.kind, TransformationKind::LoopUnrolled { .. }) }));
    }

    // -- Test 6: LoopOptimization vectorizes loops with Memory successors ---

    #[test]
    fn loop_optimization_vectorizes_memory_loops() {
        let mut scg = build_test_scg();
        let profile_data = build_test_profile_data();
        let report = build_test_profile_report(&scg, &profile_data);

        let pass = LoopOptimization {
            min_trip_count: 50,
            max_unroll_factor: 8,
            enable_vectorization: true,
        };
        let result = pass.apply(&mut scg, &report);

        // Node 20 (Loop) has an outgoing edge to node 30 (Memory) → should
        // be vectorized.
        let loop_node = scg.get_node(20).unwrap();
        assert!(
            loop_node.is_vectorized,
            "loop with Memory successor should be vectorized"
        );
        assert!(result
            .transformations
            .iter()
            .any(|t| { matches!(t.kind, TransformationKind::LoopVectorized) }));
    }

    // -- Test 7: MemoryOptimization aligns and prefetches -------------------

    #[test]
    fn memory_optimization_applies_prefetch_and_alignment() {
        let mut scg = build_test_scg();
        let profile_data = build_test_profile_data();
        let report = build_test_profile_report(&scg, &profile_data);

        let pass = MemoryOptimization::new(TargetArch::ArmV8A);
        let result = pass.apply(&mut scg, &report);

        // Node 30 is a hot Memory node → should get prefetch and alignment.
        let mem_node = scg.get_node(30).unwrap();
        assert!(
            mem_node.has_prefetch,
            "hot memory node 30 should have prefetch"
        );
        assert_eq!(
            mem_node.alignment, 64,
            "memory node 30 should be 64-byte aligned"
        );

        // Check transformation kinds.
        assert!(result
            .transformations
            .iter()
            .any(|t| { matches!(t.kind, TransformationKind::PrefetchInserted) }));
        assert!(result.transformations.iter().any(|t| {
            matches!(
                t.kind,
                TransformationKind::CacheLineAligned { alignment: 64 }
            )
        }));
    }

    // -- Test 8: apply_optimizations runs all passes end-to-end -------------

    #[test]
    fn apply_optimizations_end_to_end() {
        let mut scg = build_test_scg();
        let profile_data = build_test_profile_data();
        let report = build_test_profile_report(&scg, &profile_data);

        let result = apply_optimizations(&mut scg, &report);

        // At least one transformation should have been applied.
        assert!(
            result.total_transformations > 0,
            "end-to-end should produce at least one transformation"
        );
        // Speedup should be > 1.0 (we did *something*).
        assert!(
            result.estimated_speedup > 1.0,
            "estimated speedup should exceed 1.0, got {}",
            result.estimated_speedup
        );
        // We expect 4 pass results (one per default pass).
        assert_eq!(
            result.pass_results.len(),
            4,
            "should have results from 4 default passes"
        );

        // Verify individual nodes were actually modified by the structural passes.
        assert!(
            scg.get_node(10).unwrap().is_inlined,
            "node 10 should be inlined"
        );
        // Note: with real structural inlining, HotPathInlining removes the
        // call edge (10→40), which orphans node 40. ColdPathOutline only
        // outlines cold nodes adjacent to hot nodes, so node 40 may no longer
        // be outlined after the full pipeline. We verify the other passes
        // did their structural job instead.
        assert!(
            scg.get_node(20).unwrap().unroll_factor > 1,
            "node 20 should be unrolled"
        );
        assert!(
            scg.get_node(30).unwrap().has_prefetch,
            "node 30 should have prefetch"
        );
        // At least 3 of the 4 passes should have produced transformations
        // (HotPathInlining, LoopOptimization, MemoryOptimization; ColdPathOutline
        // may or may not depending on graph state after HotPathInlining).
        let passes_with_transforms = result
            .pass_results
            .iter()
            .filter(|r| r.count() > 0)
            .count();
        assert!(
            passes_with_transforms >= 3,
            "expected at least 3 passes with transformations, got {}",
            passes_with_transforms
        );
    }

    // -- Test 9: OptimizationEngine with no passes produces empty result ----

    #[test]
    fn empty_engine_produces_empty_result() {
        let mut scg = build_test_scg();
        let profile_data = build_test_profile_data();
        let report = build_test_profile_report(&scg, &profile_data);

        let engine = OptimizationEngine::empty(Config::default());
        let result = engine.run(&mut scg, &report);

        assert_eq!(result.total_transformations, 0);
        assert_eq!(result.estimated_speedup, 1.0);
        assert!(result.pass_results.is_empty());
    }

    // -- Test 10: ProfileReport correctly classifies hot and cold nodes ----

    #[test]
    fn profile_report_classifies_hot_and_cold() {
        let scg = build_test_scg();
        let profile_data = build_test_profile_data();
        let report = ProfileReport::from_profile_data(&profile_data, &scg);

        // Hot nodes: 10, 20, 30 (all > 100 calls)
        assert!(report.is_hot(10));
        assert!(report.is_hot(20));
        assert!(report.is_hot(30));

        // Cold nodes: 40, 50 (both < 5 calls)
        assert!(report.is_cold(40));
        assert!(report.is_cold(50));

        // Call count lookup
        assert_eq!(report.call_count(10), 500);
        assert_eq!(report.call_count(999), 0); // unknown node
    }

    // -- Test 11: Custom pass can be added to the engine --------------------

    #[test]
    fn custom_pass_in_engine() {
        /// A no-op pass for testing.
        struct NoopPass;
        impl OptimizationPass for NoopPass {
            fn name(&self) -> &str {
                "NoopPass"
            }
            fn apply(&self, _scg: &mut SCG, _profile: &ProfileReport) -> PassResult {
                PassResult::empty("NoopPass")
            }
        }

        let mut scg = SCG::new();
        let profile_data = ProfileData::new();
        let report = ProfileReport::from_profile_data(&profile_data, &scg);

        let mut engine = OptimizationEngine::empty(Config::default());
        engine.add_pass(Box::new(NoopPass));
        assert_eq!(engine.pass_count(), 1);

        let result = engine.run(&mut scg, &report);
        assert_eq!(result.pass_results.len(), 1);
        assert_eq!(result.pass_results[0].pass_name, "NoopPass");
    }

    // -- Test 12: LoopOptimization skips cold loops -------------------------

    #[test]
    fn loop_optimization_skips_cold_loops() {
        let mut scg = SCG::new();
        let mut loop_node = SCGNode::new(1, NodeKind::Loop);
        loop_node.code_size = 64;
        scg.insert_node(loop_node);

        let mut profile_data = ProfileData::new();
        profile_data.record_access(1); // Only 1 call — cold.

        let report = ProfileReport::from_profile_data(&profile_data, &scg);
        let pass = LoopOptimization::new();
        let result = pass.apply(&mut scg, &report);

        // Loop with 1 trip < 100 threshold → should NOT be unrolled.
        assert_eq!(scg.get_node(1).unwrap().unroll_factor, 1);
        assert_eq!(result.count(), 0);
    }

    // ======================================================================
    // Wave 37 — real structural transformation tests
    // ======================================================================
    //
    // The original tests above only verified that annotation flags
    // (`is_inlined`, `is_outlined`, `unroll_factor`, `has_prefetch`,
    // `alignment`) were set. Wave 37 makes the four CoR optimisation passes
    // perform genuine structural transforms (node/edge insertion, removal,
    // redirection). The tests below verify each pass measurably changes the
    // SCG's structure.

    // -- Test W37-A: HotPathInlining transforms structure -------------------

    #[test]
    fn wave37_hot_path_inlining_transforms_structure() {
        // SCG: entry(1) → call(10) → body_a(40) → body_b(41) → continuation(50).
        // After inlining: entry → clone(body_a) → clone(body_b) → continuation.
        // The call edge (10→40) is removed; node 10 is orphaned.
        let mut scg = SCG::new();

        let mut entry = SCGNode::new(1, NodeKind::Entry);
        entry.outgoing_edges.push(300);
        scg.insert_node(entry);

        let mut call = SCGNode::new(10, NodeKind::Call);
        call.code_size = 64;
        call.incoming_edges.push(300);
        call.outgoing_edges.push(100);
        scg.insert_node(call);

        let mut body_a = SCGNode::new(40, NodeKind::Compute);
        body_a.code_size = 32;
        body_a.incoming_edges.push(100);
        body_a.outgoing_edges.push(101);
        scg.insert_node(body_a);

        let mut body_b = SCGNode::new(41, NodeKind::Compute);
        body_b.code_size = 32;
        body_b.incoming_edges.push(101);
        body_b.outgoing_edges.push(102);
        scg.insert_node(body_b);

        let mut cont = SCGNode::new(50, NodeKind::Compute);
        cont.code_size = 16;
        cont.incoming_edges.push(102);
        scg.insert_node(cont);

        scg.insert_edge(SCGEdge::new(300, 1, 10));
        scg.insert_edge(SCGEdge::new(100, 10, 40));
        scg.insert_edge(SCGEdge::new(101, 40, 41));
        scg.insert_edge(SCGEdge::new(102, 41, 50));

        let mut profile_data = ProfileData::new();
        for _ in 0..500 {
            profile_data.record_access(10);
        }
        let report = ProfileReport::from_profile_data(&profile_data, &scg);

        let node_count_before = scg.node_count;
        let pass = HotPathInlining::new();
        let result = pass.apply(&mut scg, &report);

        // Node 10 is inlined.
        assert!(
            scg.get_node(10).unwrap().is_inlined,
            "call node 10 should be inlined"
        );

        // Node count increased (body clones added).
        assert!(
            scg.node_count > node_count_before,
            "node count should increase after inlining (before={}, after={})",
            node_count_before,
            scg.node_count
        );

        // The call edge (10→40, edge 100) is gone.
        assert!(scg.get_edge(100).is_none(), "call edge 100 should be removed");

        // Node 10's outgoing_edges should be empty (orphaned).
        assert!(
            scg.get_node(10).unwrap().outgoing_edges.is_empty(),
            "inlined call node 10 should have no outgoing edges"
        );

        // The caller's edge (300) now points to a body clone, not node 10.
        let edge_300 = scg.get_edge(300).unwrap();
        assert_ne!(
            edge_300.target, 10,
            "edge 300 should no longer target the call node"
        );

        // At least one transformation recorded.
        assert!(result.count() >= 1);
    }

    // -- Test W37-B: ColdPathOutline creates a function-entry node -----------

    #[test]
    fn wave37_cold_path_outline_creates_function_node() {
        // SCG: hot(10) → cold(40). After outlining: hot(10) → func_entry(F) → cold(40).
        let mut scg = SCG::new();

        let mut hot = SCGNode::new(10, NodeKind::Call);
        hot.code_size = 64;
        hot.outgoing_edges.push(100);
        scg.insert_node(hot);

        let mut cold = SCGNode::new(40, NodeKind::Branch);
        cold.code_size = 32;
        cold.incoming_edges.push(100);
        scg.insert_node(cold);

        scg.insert_edge(SCGEdge::new(100, 10, 40));

        let mut profile_data = ProfileData::new();
        for _ in 0..500 {
            profile_data.record_access(10);
        }
        profile_data.record_access(40); // cold
        let report = ProfileReport::from_profile_data(&profile_data, &scg);

        let node_count_before = scg.node_count;
        let pass = ColdPathOutline::new();
        let result = pass.apply(&mut scg, &report);

        // A new FunctionEntry node exists.
        let func_entries: Vec<&SCGNode> = scg
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::FunctionEntry)
            .collect();
        assert_eq!(
            func_entries.len(),
            1,
            "expected exactly 1 FunctionEntry node, got {}",
            func_entries.len()
        );

        let f_id = func_entries[0].id;

        // The cold node is outlined.
        assert!(
            scg.get_node(40).unwrap().is_outlined,
            "cold node 40 should be outlined"
        );

        // The original edge (100, 10→40) now targets F.
        let edge_100 = scg.get_edge(100).unwrap();
        assert_eq!(
            edge_100.target, f_id,
            "edge 100 should now target the function-entry node"
        );

        // A new edge F→40 exists.
        let has_f_to_40 = scg.edges.values().any(|e| e.source == f_id && e.target == 40);
        assert!(has_f_to_40, "expected an edge from function-entry to cold node");

        // Node count increased by 1 (the new function-entry).
        assert_eq!(
            scg.node_count,
            node_count_before + 1,
            "node count should increase by 1"
        );

        // Transformation recorded.
        assert!(result.count() >= 1);
    }

    // -- Test W37-C: LoopOptimization duplicates the body -------------------

    #[test]
    fn wave37_loop_optimization_duplicates_body() {
        // SCG: loop(20) → body(30) → loop(20) (back-edge).
        // After unrolling with factor F: loop → body_1 → body_2 → ... → body_F → loop.
        let mut scg = SCG::new();

        let mut loop_node = SCGNode::new(20, NodeKind::Loop);
        loop_node.code_size = 128;
        loop_node.outgoing_edges.push(200);
        loop_node.incoming_edges.push(201);
        scg.insert_node(loop_node);

        let mut body = SCGNode::new(30, NodeKind::Memory);
        body.code_size = 64;
        body.incoming_edges.push(200);
        body.outgoing_edges.push(201);
        scg.insert_node(body);

        scg.insert_edge(SCGEdge::new(200, 20, 30));
        scg.insert_edge(SCGEdge {
            id: 201,
            source: 30,
            target: 20,
            weight: 5000,
        });

        let mut profile_data = ProfileData::new();
        for _ in 0..500 {
            profile_data.record_access(20);
        }
        let report = ProfileReport::from_profile_data(&profile_data, &scg);

        let node_count_before = scg.node_count; // 2

        // Use max_unroll_factor = 4 for a deterministic test.
        let pass = LoopOptimization {
            min_trip_count: 50,
            max_unroll_factor: 4,
            enable_vectorization: false,
        };
        let result = pass.apply(&mut scg, &report);

        let factor = scg.get_node(20).unwrap().unroll_factor;
        assert!(
            factor > 1,
            "loop node 20 should be unrolled (factor={})",
            factor
        );
        assert_eq!(
            factor, 4,
            "expected factor=4 with max_unroll_factor=4 and trip=5000"
        );

        // Body node count grew: 1 original + (factor-1) clones.
        let memory_count = scg
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Memory)
            .count();
        assert_eq!(
            memory_count,
            factor as usize,
            "expected {} memory nodes (1 original + {} clones), got {}",
            factor,
            factor - 1,
            memory_count
        );

        // Total node count grew by factor-1.
        assert_eq!(
            scg.node_count,
            node_count_before + (factor - 1) as usize,
            "node count should grow by factor-1"
        );

        // Clones have IV-iteration labels.
        let labeled_clones = scg
            .nodes
            .values()
            .filter(|n| {
                n.kind == NodeKind::Memory
                    && n.control_label
                        .as_deref()
                        .map(|s| s.starts_with("unroll_iter_"))
                        .unwrap_or(false)
            })
            .count();
        assert_eq!(
            labeled_clones,
            (factor - 1) as usize,
            "expected {} clones with IV labels, got {}",
            factor - 1,
            labeled_clones
        );

        // The original back-edge (201) was redirected (no longer 30→20).
        let edge_201 = scg.get_edge(201).unwrap();
        assert_ne!(
            edge_201.target, 20,
            "back-edge 201 should no longer target the loop header directly"
        );

        // A LoopUnrolled transformation was recorded.
        assert!(result
            .transformations
            .iter()
            .any(|t| matches!(t.kind, TransformationKind::LoopUnrolled { .. })));
    }

    // -- Test W37-D: MemoryOptimization inserts prefetch nodes --------------

    #[test]
    fn wave37_memory_optimization_inserts_prefetch_nodes() {
        // SCG: hot(10) → mem(30). After: hot(10) → prefetch(P) → mem(30).
        let mut scg = SCG::new();

        let mut hot = SCGNode::new(10, NodeKind::Call);
        hot.code_size = 64;
        hot.outgoing_edges.push(100);
        scg.insert_node(hot);

        let mut mem = SCGNode::new(30, NodeKind::Memory);
        mem.code_size = 64;
        mem.incoming_edges.push(100);
        scg.insert_node(mem);

        scg.insert_edge(SCGEdge::new(100, 10, 30));

        let mut profile_data = ProfileData::new();
        for _ in 0..500 {
            profile_data.record_access(10);
        }
        for _ in 0..200 {
            profile_data.record_access(30);
        }
        let report = ProfileReport::from_profile_data(&profile_data, &scg);

        let node_count_before = scg.node_count; // 2
        let pass = MemoryOptimization::new(TargetArch::ArmV8A);
        let result = pass.apply(&mut scg, &report);

        // Node count increased (prefetch node inserted).
        assert!(
            scg.node_count > node_count_before,
            "node count should increase after prefetch insertion"
        );

        // Memory node 30 has prefetch and alignment.
        let mem_node = scg.get_node(30).unwrap();
        assert!(mem_node.has_prefetch, "memory node 30 should have prefetch");
        assert_eq!(
            mem_node.alignment, 64,
            "memory node 30 should be 64-byte aligned"
        );

        // A new prefetch Memory node exists.
        let prefetch_nodes: Vec<&SCGNode> = scg
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::Memory && n.id != 30 && n.has_prefetch)
            .collect();
        assert_eq!(
            prefetch_nodes.len(),
            1,
            "expected exactly 1 prefetch node, got {}",
            prefetch_nodes.len()
        );

        let prefetch_id = prefetch_nodes[0].id;

        // The prefetch node has an edge to the memory node.
        let has_prefetch_to_mem = scg
            .edges
            .values()
            .any(|e| e.source == prefetch_id && e.target == 30);
        assert!(
            has_prefetch_to_mem,
            "expected edge from prefetch node to memory node"
        );

        // The original edge (100, 10→30) now targets the prefetch node.
        let edge_100 = scg.get_edge(100).unwrap();
        assert_eq!(
            edge_100.target, prefetch_id,
            "edge 100 should now target the prefetch node"
        );

        // Transformations recorded.
        assert!(result
            .transformations
            .iter()
            .any(|t| matches!(t.kind, TransformationKind::PrefetchInserted)));
        assert!(result.transformations.iter().any(|t| {
            matches!(
                t.kind,
                TransformationKind::CacheLineAligned { alignment: 64 }
            )
        }));
    }

    // -- Test W37-E: All passes transform the SCG measurably ----------------

    #[test]
    fn wave37_all_passes_transform() {
        let mut scg = build_test_scg();
        let profile_data = build_test_profile_data();
        let report = build_test_profile_report(&scg, &profile_data);

        let node_count_before = scg.node_count;
        let edge_count_before = scg.edge_count;

        let result = apply_optimizations(&mut scg, &report);

        // At least one transformation.
        assert!(
            result.total_transformations > 0,
            "expected at least one transformation"
        );

        // Node count changed (structural transformation occurred).
        assert_ne!(
            scg.node_count, node_count_before,
            "node count should change after structural optimizations"
        );
        assert_ne!(
            scg.edge_count, edge_count_before,
            "edge count should change after structural optimizations"
        );

        // 4 pass results.
        assert_eq!(
            result.pass_results.len(),
            4,
            "should have results from 4 default passes"
        );
    }
}
