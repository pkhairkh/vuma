//! SCG Variable Liveness Analysis
//!
//! This module implements variable liveness analysis on the Semantic Computation
//! Graph (SCG). It computes live-in and live-out sets for each node using
//! standard iterative backward dataflow analysis.
//!
//! # Key Concepts
//!
//! In the SCG, each node defines (produces) a value identified by its [`NodeId`].
//! A node *uses* values from its data-flow and derivation predecessors. Liveness
//! analysis determines which values are "live" (may be needed later) at each
//! program point.
//!
//! - **live_in\[n\]**: the set of values live at the entry of node `n`.
//!   A value `v` is live-in at `n` if `n` uses `v` or `v` is live-out at `n`
//!   and `v` is not defined by `n`.
//! - **live_out\[n\]**: the set of values live at the exit of node `n`.
//!   A value `v` is live-out at `n` if `v` is live-in at any successor of `n`.
//!
//! # Dataflow Equations
//!
//! ```text
//! live_out[n] = ∪ live_in[s]    for all successors s of n
//! live_in[n]  = use[n] ∪ (live_out[n] - def[n])
//! ```
//!
//! where:
//! - `def[n] = {n}` — each node defines its own value
//! - `use[n]` = NodeIds that flow into `n` via DataFlow or Derivation edges
//!
//! # IVE Integration
//!
//! The liveness information supports the Insertion/Verification of Extensions
//! (IVE) subsystem by enabling:
//! - **Use-after-free detection**: values live after deallocation
//! - **Dead allocation detection**: allocations whose results are never consumed
//! - **Dead code detection**: pure computations whose results are never used
//! - **Uninitialized read detection**: reads of values with no reaching definition

use hashbrown::{HashMap, HashSet};

use crate::edge::EdgeKind;
use crate::graph::SCG;
use crate::node::{AccessMode, NodeId, NodePayload, NodeType};

// ─── LivenessInfo ────────────────────────────────────────────────────────────

/// Liveness information for a single SCG node.
///
/// Contains the sets of values (identified by [`NodeId`]) that are live at
/// the entry and exit of this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LivenessInfo {
    /// Values live at the entry of this node.
    ///
    /// A value is live-in if it is used by this node or live-out and not
    /// defined by this node.
    pub live_in: HashSet<NodeId>,

    /// Values live at the exit of this node.
    ///
    /// A value is live-out if it is live-in at any successor of this node.
    pub live_out: HashSet<NodeId>,
}

impl LivenessInfo {
    /// Creates empty liveness info (no values live).
    pub fn empty() -> Self {
        Self {
            live_in: HashSet::new(),
            live_out: HashSet::new(),
        }
    }

    /// Returns `true` if the given value is live at the entry of this node.
    pub fn is_live_in(&self, id: &NodeId) -> bool {
        self.live_in.contains(id)
    }

    /// Returns `true` if the given value is live at the exit of this node.
    pub fn is_live_out(&self, id: &NodeId) -> bool {
        self.live_out.contains(id)
    }

    /// Returns the number of values live at entry.
    pub fn live_in_count(&self) -> usize {
        self.live_in.len()
    }

    /// Returns the number of values live at exit.
    pub fn live_out_count(&self) -> usize {
        self.live_out.len()
    }
}

impl std::fmt::Display for LivenessInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut live_in: Vec<_> = self.live_in.iter().collect();
        live_in.sort_by_key(|id| id.as_u64());
        let mut live_out: Vec<_> = self.live_out.iter().collect();
        live_out.sort_by_key(|id| id.as_u64());
        write!(
            f,
            "LivenessInfo {{ live_in: {:?}, live_out: {:?} }}",
            live_in, live_out
        )
    }
}

// ─── LivenessAnalysis ────────────────────────────────────────────────────────

/// The result of running liveness analysis on an SCG.
///
/// Contains per-node liveness information, convergence metadata, and
/// provides convenience methods for querying liveness properties.
#[derive(Debug, Clone)]
pub struct LivenessAnalysis {
    /// Per-node liveness information.
    pub liveness: HashMap<NodeId, LivenessInfo>,
    /// Number of iterations required to reach a fixed point.
    pub iterations: usize,
    /// Whether the analysis converged within the iteration limit.
    pub converged: bool,
}

impl LivenessAnalysis {
    /// Runs liveness analysis on the given SCG.
    ///
    /// This is the primary constructor for `LivenessAnalysis`. It performs
    /// standard iterative backward dataflow analysis until a fixed point
    /// is reached.
    pub fn new(scg: &SCG) -> Self {
        compute_liveness_inner(scg)
    }

    /// Retrieves liveness info for a specific node.
    pub fn get(&self, id: &NodeId) -> Option<&LivenessInfo> {
        self.liveness.get(id)
    }

    /// Returns `true` if the given value is live at the entry of the given node.
    pub fn is_live_in(&self, node: NodeId, value: NodeId) -> bool {
        self.liveness
            .get(&node)
            .is_some_and(|info| info.is_live_in(&value))
    }

    /// Returns `true` if the given value is live at the exit of the given node.
    pub fn is_live_out(&self, node: NodeId, value: NodeId) -> bool {
        self.liveness
            .get(&node)
            .is_some_and(|info| info.is_live_out(&value))
    }

    /// Returns the set of all values that are live anywhere in the graph.
    pub fn all_live_values(&self) -> HashSet<NodeId> {
        let mut values = HashSet::new();
        for info in self.liveness.values() {
            values.extend(&info.live_in);
            values.extend(&info.live_out);
        }
        values
    }
}

// ─── Core Computation ────────────────────────────────────────────────────────

/// Maximum number of iterations before declaring non-convergence.
const MAX_ITERATIONS: usize = 10_000;

/// Compute the `use` set for a node: the set of NodeIds whose values
/// this node consumes via DataFlow or Derivation edges.
///
/// DataFlow edges represent data consumption (a computation uses a value).
/// Derivation edges represent semantic dependencies (a deallocation depends
/// on an allocation) and are treated as uses for liveness purposes.
fn compute_use_set(scg: &SCG, node_id: NodeId) -> HashSet<NodeId> {
    let mut uses = HashSet::new();
    for edge in scg.edges() {
        if edge.target == node_id && matches!(edge.kind, EdgeKind::DataFlow | EdgeKind::Derivation)
        {
            uses.insert(edge.source);
        }
    }
    uses
}

/// Compute the successor set for a node for liveness propagation.
///
/// Successors are nodes reachable via ControlFlow, DataFlow, or Derivation
/// edges. Annotation edges are excluded because they do not represent
/// execution or data flow.
fn compute_successor_set(scg: &SCG, node_id: NodeId) -> Vec<NodeId> {
    let mut succs = Vec::new();
    let mut seen = HashSet::new();

    if let Some(all_succs) = scg.successors(node_id) {
        for succ in all_succs {
            if seen.insert(succ) {
                // Check if there is at least one relevant edge to this successor
                let has_relevant_edge = scg.edges().any(|e| {
                    e.source == node_id
                        && e.target == succ
                        && matches!(
                            e.kind,
                            EdgeKind::ControlFlow | EdgeKind::DataFlow | EdgeKind::Derivation
                        )
                });
                if has_relevant_edge {
                    succs.push(succ);
                }
            }
        }
    }

    succs
}

/// Compute liveness information for all nodes in the SCG.
///
/// This is the primary entry point for liveness analysis. It returns a
/// mapping from each [`NodeId`] to its [`LivenessInfo`].
///
/// Uses standard iterative backward dataflow analysis:
/// - `live_out[n] = ∪ live_in[s]` for all successors `s` of `n`
/// - `live_in[n] = use[n] ∪ (live_out[n] - def[n])`
///
/// where `def[n] = {n}` and `use[n]` is computed from incoming
/// DataFlow/Derivation edges.
///
/// # Example
///
/// ```
/// use vuma_scg::{SCG, NodeType, NodePayload, ComputationKind, ComputationNode, ProgramPoint, EdgeKind};
/// use vuma_scg::liveness::compute_liveness;
///
/// let mut scg = SCG::new();
/// let pp = ProgramPoint { file: None, line: None, column: None, offset: None };
/// let n1 = scg.add_node(
///     NodeType::Computation,
///     NodePayload::Computation(ComputationNode { kind: ComputationKind::Other("a".into()), result_type: None, tail_call: false }),
///     pp.clone(),
/// );
/// let n2 = scg.add_node(
///     NodeType::Computation,
///     NodePayload::Computation(ComputationNode { kind: ComputationKind::Other("b".into()), result_type: None, tail_call: false }),
///     pp,
/// );
/// scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
///
/// let liveness = compute_liveness(&scg);
/// assert!(liveness[&n1].is_live_out(&n1));  // n1's value is live at n1's exit (used by n2)
/// ```
pub fn compute_liveness(scg: &SCG) -> HashMap<NodeId, LivenessInfo> {
    compute_liveness_inner(scg).liveness
}

/// Internal implementation that also returns convergence metadata.
fn compute_liveness_inner(scg: &SCG) -> LivenessAnalysis {
    let all_node_ids: Vec<NodeId> = scg.node_ids().collect();

    if all_node_ids.is_empty() {
        return LivenessAnalysis {
            liveness: HashMap::new(),
            iterations: 0,
            converged: true,
        };
    }

    // Pre-compute use sets and successor sets for all nodes.
    // This avoids repeated iteration over all edges during the fixpoint loop.
    let mut use_sets: HashMap<NodeId, HashSet<NodeId>> = HashMap::with_capacity(all_node_ids.len());
    let mut successor_sets: HashMap<NodeId, Vec<NodeId>> =
        HashMap::with_capacity(all_node_ids.len());

    for &node_id in &all_node_ids {
        use_sets.insert(node_id, compute_use_set(scg, node_id));
        successor_sets.insert(node_id, compute_successor_set(scg, node_id));
    }

    // Initialize all liveness info to empty.
    let mut liveness: HashMap<NodeId, LivenessInfo> = HashMap::with_capacity(all_node_ids.len());
    for &node_id in &all_node_ids {
        liveness.insert(node_id, LivenessInfo::empty());
    }

    // Iterative backward dataflow analysis.
    let mut converged = false;
    let mut iterations = 0;

    while !converged && iterations < MAX_ITERATIONS {
        converged = true;
        iterations += 1;

        for &node_id in &all_node_ids {
            let use_set = &use_sets[&node_id];
            let succs = &successor_sets[&node_id];

            // Compute live_out: union of live_in of all successors.
            let mut new_live_out: HashSet<NodeId> = HashSet::new();
            for &succ in succs {
                if let Some(succ_info) = liveness.get(&succ) {
                    new_live_out.extend(&succ_info.live_in);
                }
            }

            // Compute live_in: use ∪ (live_out - def).
            // def[n] = {n}, so live_out - def = live_out minus the node itself.
            let mut new_live_in: HashSet<NodeId> = use_set.clone();
            for &id in &new_live_out {
                if id != node_id {
                    new_live_in.insert(id);
                }
            }

            // Check for changes and update.
            let info = liveness.get_mut(&node_id).unwrap();
            if info.live_in != new_live_in || info.live_out != new_live_out {
                converged = false;
                info.live_in = new_live_in;
                info.live_out = new_live_out;
            }
        }
    }

    LivenessAnalysis {
        liveness,
        iterations,
        converged,
    }
}

// ─── Dead Code Detection ─────────────────────────────────────────────────────

/// Returns `true` if the given node type is considered "pure" (no observable
/// side effects that must be preserved).
fn is_pure_node(node_type: &NodeType) -> bool {
    matches!(
        node_type,
        NodeType::Computation | NodeType::Cast | NodeType::Phantom
    )
}

/// Find nodes that compute values never used (dead code).
///
/// A node is considered dead if:
/// 1. It is **pure** (no observable side effects: `Computation`, `Cast`, `Phantom`)
/// 2. Its value is not needed by any side-effecting node
///
/// The analysis uses a backward reachability approach:
/// 1. Start from all "essential" (non-pure) nodes
/// 2. Mark all values they use as "needed"
/// 3. Propagate "needed" backwards through DataFlow/Derivation edges
/// 4. Any pure node whose value is not "needed" is dead code
///
/// This correctly handles transitive dead code: if A feeds B, and B feeds
/// no essential node, both A and B are dead.
pub fn find_dead_code(scg: &SCG, _liveness: &HashMap<NodeId, LivenessInfo>) -> Vec<NodeId> {
    // Step 1: Identify all "needed" values using backward reachability
    // from essential (non-pure) nodes.
    let mut needed: HashSet<NodeId> = HashSet::new();

    // All non-pure nodes are essential.
    for node_data in scg.nodes() {
        if !is_pure_node(&node_data.node_type) {
            needed.insert(node_data.id);
        }
    }

    // Propagate "needed" backwards through DataFlow and Derivation edges.
    // A value is needed if an essential node or another needed node uses it.
    let mut changed = true;
    while changed {
        changed = false;
        for edge in scg.edges() {
            if matches!(edge.kind, EdgeKind::DataFlow | EdgeKind::Derivation) {
                // If the target is needed, the source's value is also needed
                if needed.contains(&edge.target) && needed.insert(edge.source) {
                    changed = true;
                }
            }
        }
    }

    // Step 2: Any pure node not in "needed" is dead code.
    let mut dead: Vec<NodeId> = Vec::new();
    for node_data in scg.nodes() {
        if is_pure_node(&node_data.node_type) && !needed.contains(&node_data.id) {
            dead.push(node_data.id);
        }
    }

    // Sort for deterministic output.
    dead.sort_by_key(|id| id.as_u64());
    dead
}

// ─── Uninitialized Read Detection ────────────────────────────────────────────

/// Find reads of uninitialized values.
///
/// An uninitialized read is an [`NodeType::Access`] node with `Read` or
/// `ReadWrite` mode that reads from a region where no prior allocation or
/// write access exists along any path reaching this node.
///
/// The analysis checks for each read access:
/// 1. What region does it access?
/// 2. Is there any `Allocation` or `Access(Write/ReadWrite)` in the same region?
/// 3. Can any such node reach this read (is there a path in the SCG)?
///
/// If no such node can reach the read, the read is flagged as uninitialized.
pub fn find_uninitialized_reads(
    scg: &SCG,
    _liveness: &HashMap<NodeId, LivenessInfo>,
) -> Vec<NodeId> {
    let mut uninitialized: Vec<NodeId> = Vec::new();

    for node_data in scg.nodes() {
        // Only check Read and ReadWrite access nodes
        if let NodePayload::Access(ref access) = node_data.payload {
            if !matches!(access.mode, AccessMode::Read | AccessMode::ReadWrite) {
                continue;
            }

            let region = access.region_id;

            // Check if any allocation or write access in the same region can
            // reach this node.
            let has_reaching_def = scg.nodes().any(|other| {
                if other.id == node_data.id {
                    return false;
                }

                let is_relevant = match &other.payload {
                    NodePayload::Allocation(_) => true,
                    NodePayload::Access(ref a) => {
                        a.region_id == region
                            && matches!(a.mode, AccessMode::Write | AccessMode::ReadWrite)
                    }
                    _ => false,
                };

                if !is_relevant {
                    return false;
                }

                // Check if the other node can reach this read
                scg.find_path(other.id, node_data.id).unwrap_or(false)
            });

            if !has_reaching_def {
                uninitialized.push(node_data.id);
            }
        }
    }

    uninitialized.sort_by_key(|id| id.as_u64());
    uninitialized
}

// ─── IVE: Use-After-Free Detection ───────────────────────────────────────────

/// A use-after-free violation detected by liveness analysis.
///
/// Records the allocation node whose value is still live after the
/// deallocation node executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseAfterFree {
    /// The allocation node whose value is used after free.
    pub allocation: NodeId,
    /// The deallocation node that frees the value.
    pub deallocation: NodeId,
    /// The nodes that use the allocation's value after the deallocation.
    pub violating_uses: HashSet<NodeId>,
}

impl std::fmt::Display for UseAfterFree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "UseAfterFree(allocation={}, deallocation={}, violations={})",
            self.allocation,
            self.deallocation,
            self.violating_uses.len()
        )
    }
}

/// Find use-after-free violations.
///
/// A use-after-free occurs when an allocation's value is still live after
/// its corresponding deallocation node. This means some node after the
/// deallocation still uses the allocation's result, which would access
/// freed memory.
///
/// # Algorithm
///
/// For each deallocation node `D` of allocation `A`:
/// 1. Check if `A`'s NodeId is in `live_out[D]`
/// 2. If so, collect the nodes that use `A`'s value after `D`
/// 3. Report a use-after-free violation
///
/// # Returns
///
/// A vector of [`UseAfterFree`] instances describing each violation.
pub fn find_use_after_free(
    scg: &SCG,
    liveness: &HashMap<NodeId, LivenessInfo>,
) -> Vec<UseAfterFree> {
    let mut violations: Vec<UseAfterFree> = Vec::new();

    // Find all deallocation nodes
    for node_data in scg.nodes() {
        if let NodePayload::Deallocation(ref dealloc) = node_data.payload {
            let alloc_id = dealloc.allocation_node;
            let dealloc_id = node_data.id;

            // Check if the allocation's value is live after the deallocation
            if let Some(info) = liveness.get(&dealloc_id) {
                if info.is_live_out(&alloc_id) {
                    // Find which nodes use the allocation after the deallocation.
                    // These are successors of D that have alloc_id in their live_in.
                    let mut violating_uses: HashSet<NodeId> = HashSet::new();
                    collect_violating_uses(
                        scg,
                        dealloc_id,
                        alloc_id,
                        liveness,
                        &mut violating_uses,
                    );

                    violations.push(UseAfterFree {
                        allocation: alloc_id,
                        deallocation: dealloc_id,
                        violating_uses,
                    });
                }
            }
        }
    }

    violations.sort_by_key(|v| v.deallocation);
    violations
}

/// Recursively collect nodes that use `alloc_id` after `dealloc_id`.
fn collect_violating_uses(
    scg: &SCG,
    dealloc_id: NodeId,
    alloc_id: NodeId,
    liveness: &HashMap<NodeId, LivenessInfo>,
    result: &mut HashSet<NodeId>,
) {
    // Look at direct successors of the deallocation
    if let Some(succs) = scg.successors(dealloc_id) {
        for succ in succs {
            if let Some(info) = liveness.get(&succ) {
                if info.is_live_in(&alloc_id) && result.insert(succ) {
                    // Check if this successor directly uses the allocation
                    let directly_uses = scg.edges().any(|e| {
                        e.target == succ
                            && e.source == alloc_id
                            && matches!(e.kind, EdgeKind::DataFlow | EdgeKind::Derivation)
                    });

                    if directly_uses {
                        // This node directly uses the freed allocation
                    }

                    // Continue checking successors for further uses
                    collect_violating_uses_recursive(scg, succ, alloc_id, liveness, result);
                }
            }
        }
    }
}

/// Recursively check successors for further uses of the freed allocation.
fn collect_violating_uses_recursive(
    scg: &SCG,
    current: NodeId,
    alloc_id: NodeId,
    liveness: &HashMap<NodeId, LivenessInfo>,
    result: &mut HashSet<NodeId>,
) {
    if let Some(succs) = scg.successors(current) {
        for succ in succs {
            if let Some(info) = liveness.get(&succ) {
                if info.is_live_in(&alloc_id) && result.insert(succ) {
                    collect_violating_uses_recursive(scg, succ, alloc_id, liveness, result);
                }
            }
        }
    }
}

// ─── IVE: Dead Allocation Detection ──────────────────────────────────────────

/// Find dead allocations — allocations whose memory is never read.
///
/// A dead allocation is an [`NodeType::Allocation`] node where:
/// 1. No `Access(Read/ReadWrite)` node in the same region is reachable from
///    the allocation, AND
/// 2. No DataFlow edge carries the allocation's value to any consuming node
///    other than its deallocation.
///
/// Dead allocations are optimization hints: the allocation is unnecessary
/// because its result is never meaningfully consumed.
pub fn find_dead_allocations(scg: &SCG, _liveness: &HashMap<NodeId, LivenessInfo>) -> Vec<NodeId> {
    let mut dead_allocs: Vec<NodeId> = Vec::new();

    for node_data in scg.nodes() {
        if node_data.node_type != NodeType::Allocation {
            continue;
        }

        let alloc_id = node_data.id;
        let region_id = match &node_data.payload {
            NodePayload::Allocation(ref a) => a.region_id,
            _ => continue,
        };

        // Check if any read access to the same region is reachable from
        // this allocation.
        let has_read_access = scg.nodes().any(|other| {
            if other.id == alloc_id {
                return false;
            }

            let reads_region = match &other.payload {
                NodePayload::Access(ref a) => {
                    a.region_id == region_id
                        && matches!(a.mode, AccessMode::Read | AccessMode::ReadWrite)
                }
                _ => false,
            };

            if !reads_region {
                return false;
            }

            // The read access must be reachable from the allocation
            scg.find_path(alloc_id, other.id).unwrap_or(false)
        });

        if has_read_access {
            continue;
        }

        // Also check: does any computation node consume the allocation's
        // value via DataFlow (other than deallocation)?
        let has_dataflow_use = scg.edges().any(|e| {
            e.source == alloc_id
                && e.kind == EdgeKind::DataFlow
                && !matches!(
                    scg.get_node(e.target).map(|n| &n.node_type),
                    Some(NodeType::Deallocation)
                )
        });

        if !has_dataflow_use {
            dead_allocs.push(alloc_id);
        }
    }

    dead_allocs.sort_by_key(|id| id.as_u64());
    dead_allocs
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::graph::SCG;
    use crate::node::{
        AccessNode, AllocationNode, ComputationKind, ComputationNode, ControlKind, ControlNode, DeallocationNode,
        EffectNode, NodePayload, NodeType, ProgramPoint,
    };
    use crate::region::RegionId;

    /// Helper to create a default program point for tests.
    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        }
    }

    // ── Test 1: Empty SCG ──────────────────────────────────────────────

    #[test]
    fn test_empty_scg() {
        let scg = SCG::new();
        let liveness = compute_liveness(&scg);
        assert!(liveness.is_empty());

        let analysis = LivenessAnalysis::new(&scg);
        assert!(analysis.converged);
        assert_eq!(analysis.iterations, 0);
    }

    // ── Test 2: Single node with no edges ──────────────────────────────

    #[test]
    fn test_single_node_no_edges() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("const".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );

        let liveness = compute_liveness(&scg);
        assert_eq!(liveness.len(), 1);
        let info = &liveness[&n1];
        assert!(info.live_in.is_empty());
        assert!(info.live_out.is_empty());
    }

    // ── Test 3: Linear dataflow chain ──────────────────────────────────

    #[test]
    fn test_linear_dataflow_chain() {
        let mut scg = SCG::new();
        // n1 --DataFlow--> n2 --DataFlow--> n3
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("a".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("b".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Effect,
            NodePayload::Effect(EffectNode {
                effect_kind: "print".to_string(),
                is_observable: true,
            }),
            pp(),
        );

        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n3, EdgeKind::DataFlow).unwrap();

        let liveness = compute_liveness(&scg);

        // n3: uses n2, no successors → live_in = {n2}, live_out = {}
        assert!(liveness[&n3].is_live_in(&n2));
        assert!(!liveness[&n3].is_live_in(&n1));
        assert!(liveness[&n3].live_out.is_empty());

        // n2: uses n1, successor n3 with live_in={n2} → live_out={n2}, live_in={n1,n2}
        // Wait: live_out[n2] = live_in[n3] = {n2}. But def[n2] = {n2}, so
        // live_in[n2] = use[n2] ∪ (live_out[n2] - def[n2]) = {n1} ∪ ({n2} - {n2}) = {n1}
        assert!(liveness[&n2].is_live_out(&n2));
        assert!(liveness[&n2].is_live_in(&n1));
        assert!(!liveness[&n2].is_live_in(&n2)); // n2 defines itself, removed from live_in

        // n1: no uses, successor n2 with live_in={n1} → live_out={n1}, live_in={}
        assert!(liveness[&n1].is_live_out(&n1));
        assert!(!liveness[&n1].is_live_in(&n1)); // n1 defines itself
        assert!(liveness[&n1].live_in.is_empty());
    }

    // ── Test 4: Diamond (branching) ────────────────────────────────────

    #[test]
    fn test_diamond_branching() {
        let mut scg = SCG::new();
        //     n1
        //    /   \
        //   n2    n3  (both use n1)
        //    \   /
        //     n4   (uses n2, n3)
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("src".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("left".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("right".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n4 = scg.add_node(
            NodeType::Effect,
            NodePayload::Effect(EffectNode {
                effect_kind: "output".to_string(),
                is_observable: true,
            }),
            pp(),
        );

        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n1, n3, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n4, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n3, n4, EdgeKind::DataFlow).unwrap();
        scg.add_edge(n1, n3, EdgeKind::ControlFlow).unwrap();

        let liveness = compute_liveness(&scg);

        // n1's value should be live at n1's exit (used by both n2 and n3)
        assert!(liveness[&n1].is_live_out(&n1));

        // n4 uses n2 and n3
        assert!(liveness[&n4].is_live_in(&n2));
        assert!(liveness[&n4].is_live_in(&n3));
    }

    // ── Test 5: Dead code detection ────────────────────────────────────

    #[test]
    fn test_find_dead_code() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("dead".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("also_dead".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n3 = scg.add_node(
            NodeType::Effect,
            NodePayload::Effect(EffectNode {
                effect_kind: "print".to_string(),
                is_observable: true,
            }),
            pp(),
        );

        // n1 feeds n2, but nothing uses n2
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();
        // n3 is an effect (essential) with no data dependency on n1/n2

        let liveness = compute_liveness(&scg);
        let dead = find_dead_code(&scg, &liveness);

        assert!(
            dead.contains(&n1),
            "n1 should be dead (its value feeds only dead n2)"
        );
        assert!(
            dead.contains(&n2),
            "n2 should be dead (its value is never used)"
        );
        assert!(
            !dead.contains(&n3),
            "n3 should not be dead (it has side effects)"
        );
    }

    // ── Test 6: Dead code — live computation ───────────────────────────

    #[test]
    fn test_find_dead_code_live_computation() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("live_val".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Effect,
            NodePayload::Effect(EffectNode {
                effect_kind: "use".to_string(),
                is_observable: true,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();

        let liveness = compute_liveness(&scg);
        let dead = find_dead_code(&scg, &liveness);

        assert!(
            !dead.contains(&n1),
            "n1 should not be dead (it feeds an effect)"
        );
        assert!(
            dead.is_empty(),
            "no dead code when everything feeds an effect"
        );
    }

    // ── Test 7: Allocation/deallocation with Derivation ────────────────

    #[test]
    fn test_allocation_deallocation_liveness() {
        let mut scg = SCG::new();
        let region = RegionId::new(1);

        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: region,
            }),
            pp(),
        );
        scg.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();

        let liveness = compute_liveness(&scg);

        // Deallocation uses the allocation (via Derivation edge)
        assert!(liveness[&dealloc].is_live_in(&alloc));
        // Allocation's value is live at alloc's exit (used by dealloc)
        assert!(liveness[&alloc].is_live_out(&alloc));
        // After dealloc, alloc's value should not be live
        assert!(!liveness[&dealloc].is_live_out(&alloc));
    }

    // ── Test 8: Uninitialized read detection ───────────────────────────

    #[test]
    fn test_uninitialized_reads() {
        let mut scg = SCG::new();
        let region = RegionId::new(1);
        let other_region = RegionId::new(2);

        // A read with no preceding allocation or write → uninitialized
        let read1 = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );

        // A read with a preceding write → initialized
        let write = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        let read2 = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        scg.add_edge(write, read2, EdgeKind::ControlFlow).unwrap();

        // A read in a different region with no allocation → uninitialized
        let read3 = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: other_region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );

        let liveness = compute_liveness(&scg);
        let uninit = find_uninitialized_reads(&scg, &liveness);

        assert!(
            uninit.contains(&read1),
            "read1 has no reaching write or allocation"
        );
        assert!(!uninit.contains(&read2), "read2 has a reaching write");
        assert!(
            uninit.contains(&read3),
            "read3 has no reaching write or allocation"
        );
    }

    // ── Test 9: Use-after-free detection ───────────────────────────────

    #[test]
    fn test_use_after_free() {
        let mut scg = SCG::new();
        let region = RegionId::new(1);

        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: region,
            }),
            pp(),
        );
        // A use of alloc's value AFTER deallocation
        let use_after = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("use".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );

        scg.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();
        scg.add_edge(dealloc, use_after, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(alloc, use_after, EdgeKind::DataFlow).unwrap();

        let liveness = compute_liveness(&scg);
        let violations = find_use_after_free(&scg, &liveness);

        // Should detect that alloc's value is used after dealloc
        assert_eq!(
            violations.len(),
            1,
            "should find one use-after-free violation"
        );
        assert_eq!(violations[0].allocation, alloc);
        assert_eq!(violations[0].deallocation, dealloc);
    }

    // ── Test 10: No use-after-free when allocation is dead before dealloc

    #[test]
    fn test_no_use_after_free() {
        let mut scg = SCG::new();
        let region = RegionId::new(1);

        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: region,
            }),
            pp(),
        );

        scg.add_edge(alloc, dealloc, EdgeKind::Derivation).unwrap();

        let liveness = compute_liveness(&scg);
        let violations = find_use_after_free(&scg, &liveness);

        assert!(
            violations.is_empty(),
            "no use-after-free when alloc is only used by dealloc"
        );
    }

    // ── Test 11: Dead allocation detection ─────────────────────────────

    #[test]
    fn test_dead_allocations() {
        let mut scg = SCG::new();
        let region = RegionId::new(1);

        // Dead allocation: allocated but never read
        let alloc1 = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let dealloc1 = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc1,
                region_id: region,
            }),
            pp(),
        );
        scg.add_edge(alloc1, dealloc1, EdgeKind::Derivation)
            .unwrap();

        // Live allocation: allocated, read, then deallocated
        let alloc2 = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 128,
                align: 8,
                region_id: region,
                type_name: None,
            }),
            pp(),
        );
        let read_access = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        let dealloc2 = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc2,
                region_id: region,
            }),
            pp(),
        );
        scg.add_edge(alloc2, read_access, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(read_access, dealloc2, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(alloc2, dealloc2, EdgeKind::Derivation)
            .unwrap();

        let liveness = compute_liveness(&scg);
        let dead = find_dead_allocations(&scg, &liveness);

        assert!(
            dead.contains(&alloc1),
            "alloc1 is dead (never read from its region)"
        );
        assert!(
            !dead.contains(&alloc2),
            "alloc2 is not dead (region is read by read_access)"
        );
    }

    // ── Test 12: LivenessInfo display ──────────────────────────────────

    #[test]
    fn test_liveness_info_display() {
        let mut info = LivenessInfo::empty();
        info.live_in.insert(NodeId::new(1));
        info.live_in.insert(NodeId::new(2));
        info.live_out.insert(NodeId::new(3));

        let display = format!("{}", info);
        assert!(display.contains("live_in"));
        assert!(display.contains("live_out"));
    }

    // ── Test 13: LivenessAnalysis convenience methods ──────────────────

    #[test]
    fn test_liveness_analysis_methods() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("x".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Effect,
            NodePayload::Effect(EffectNode {
                effect_kind: "use".to_string(),
                is_observable: true,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();

        let analysis = LivenessAnalysis::new(&scg);

        assert!(
            analysis.is_live_out(n1, n1),
            "n1's value is live at n1's exit"
        );
        assert!(
            analysis.is_live_in(n2, n1),
            "n1's value is live at n2's entry"
        );
        assert!(
            !analysis.is_live_in(n1, n1),
            "n1's value is not live at its own entry (it defines it)"
        );

        let all_live = analysis.all_live_values();
        assert!(
            all_live.contains(&n1),
            "n1 should be in the set of all live values"
        );
    }

    // ── Test 14: Control flow edges propagate liveness ─────────────────

    #[test]
    fn test_control_flow_propagates_liveness() {
        let mut scg = SCG::new();
        // entry --CF--> n1 --CF--> n2
        // n1 --DF--> n2  (n2 uses n1's value)
        let entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: None,
            }),
            pp(),
        );
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("compute".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Effect,
            NodePayload::Effect(EffectNode {
                effect_kind: "output".to_string(),
                is_observable: true,
            }),
            pp(),
        );

        scg.add_edge(entry, n1, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(n1, n2, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();

        let liveness = compute_liveness(&scg);

        // n2 uses n1 via DataFlow
        assert!(liveness[&n2].is_live_in(&n1));
        // n1's value is live at n1's exit (it will be used by n2)
        assert!(liveness[&n1].is_live_out(&n1));
        // n1's value is NOT live at entry's exit because n1 defines itself.
        // A value produced by n1 doesn't exist before n1 executes.
        assert!(!liveness[&entry].is_live_out(&n1));
        // entry's live_out should be empty (no values flow through entry)
        assert!(liveness[&entry].live_out.is_empty());
    }

    // ── Test 15: ReadWrite access mode for uninitialized reads ─────────

    #[test]
    fn test_readwrite_access_not_uninitialized() {
        let mut scg = SCG::new();
        let region = RegionId::new(1);

        // An Access(ReadWrite) node acts as both write and read
        let rw_access = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::ReadWrite,
                region_id: region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        let read_after = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );
        scg.add_edge(rw_access, read_after, EdgeKind::ControlFlow)
            .unwrap();

        let liveness = compute_liveness(&scg);
        let uninit = find_uninitialized_reads(&scg, &liveness);

        // read_after should NOT be uninitialized because ReadWrite reaches it
        assert!(
            !uninit.contains(&read_after),
            "read_after has a reaching ReadWrite access"
        );
    }

    // ── Test 16: Write-only access is not an uninitialized read ────────

    #[test]
    fn test_write_only_not_uninitialized() {
        let mut scg = SCG::new();
        let region = RegionId::new(1);

        // A write-only access should not be flagged
        let write_access = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: region,
                offset: None,
                access_size: None,
            }),
            pp(),
        );

        let liveness = compute_liveness(&scg);
        let uninit = find_uninitialized_reads(&scg, &liveness);

        // Write-only access should never appear in uninitialized reads
        assert!(
            !uninit.contains(&write_access),
            "write-only access is not a read and should not be flagged"
        );
    }

    // ── Test 17: Convergence metadata ──────────────────────────────────

    #[test]
    fn test_convergence_metadata() {
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("a".to_string()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let n2 = scg.add_node(
            NodeType::Effect,
            NodePayload::Effect(EffectNode {
                effect_kind: "use".to_string(),
                is_observable: true,
            }),
            pp(),
        );
        scg.add_edge(n1, n2, EdgeKind::DataFlow).unwrap();

        let analysis = LivenessAnalysis::new(&scg);
        assert!(analysis.converged, "analysis should converge for a DAG");
        assert!(
            analysis.iterations >= 1,
            "should take at least one iteration"
        );
    }
}
