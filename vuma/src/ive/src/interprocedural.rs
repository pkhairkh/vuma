//! Summary-based interprocedural analysis for the IVE.
//!
//! This module computes function summaries that capture the effects of each
//! function on regions and resources. Summaries are computed bottom-up in the
//! call graph and used at call sites to verify cross-function invariants
//! without requiring full inlining.
//!
//! # Summary Model
//!
//! Each function summary captures:
//! - **Allocated regions**: regions allocated within the function (not freed).
//! - **Freed regions**: regions deallocated within the function.
//! - **Written regions**: regions written to within the function.
//! - **Read regions**: regions read from within the function.
//! - **Acquired locks**: locks acquired within the function (not released).
//! - **Released locks**: locks released within the function.
//! - **May leak**: whether the function may leak resources on some path.

use std::collections::{BTreeSet, HashMap, HashSet};
use vuma_scg::callgraph::{CallGraph, FunctionId};
use vuma_scg::graph::SCG;
use vuma_scg::node::{AccessMode, ControlKind, NodeId, NodePayload, NodeType};
use vuma_scg::region::RegionId;

// ---------------------------------------------------------------------------
// Function Summary
// ---------------------------------------------------------------------------

/// A summary of a function's effects on regions and resources.
///
/// Summaries are computed bottom-up through the call graph and used
/// at call sites to verify cross-function invariants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionSummary {
    /// The function this summary describes.
    pub function: Option<FunctionId>,
    /// Regions allocated within this function (not freed within it).
    pub allocated_regions: BTreeSet<RegionId>,
    /// Regions deallocated within this function.
    pub freed_regions: BTreeSet<RegionId>,
    /// Regions written to within this function.
    pub written_regions: BTreeSet<RegionId>,
    /// Regions read from within this function.
    pub read_regions: BTreeSet<RegionId>,
    /// Locks acquired within this function (not released within it).
    pub acquired_locks: BTreeSet<RegionId>,
    /// Locks released within this function.
    pub released_locks: BTreeSet<RegionId>,
    /// Whether the function may leak a resource on some execution path.
    pub may_leak: bool,
    /// Functions called by this function (transitively, including summaries).
    pub calls: BTreeSet<FunctionId>,
}

impl FunctionSummary {
    /// Create an empty summary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a summary for a specific function.
    pub fn for_function(fid: FunctionId) -> Self {
        Self {
            function: Some(fid),
            ..Self::default()
        }
    }

    /// Merge another summary into this one (union of effects).
    ///
    /// Used when inlining a callee's summary into the caller.
    pub fn merge(&mut self, other: &FunctionSummary) {
        self.allocated_regions
            .extend(other.allocated_regions.iter());
        self.freed_regions.extend(other.freed_regions.iter());
        self.written_regions.extend(other.written_regions.iter());
        self.read_regions.extend(other.read_regions.iter());
        self.acquired_locks.extend(other.acquired_locks.iter());
        self.released_locks.extend(other.released_locks.iter());
        self.may_leak = self.may_leak || other.may_leak;
        self.calls.extend(other.calls.iter().copied());
    }

    /// Returns regions that are allocated but not freed (potential leaks).
    pub fn leaked_regions(&self) -> BTreeSet<RegionId> {
        self.allocated_regions
            .difference(&self.freed_regions)
            .copied()
            .collect()
    }

    /// Returns regions that are both written and read (potential data race
    /// candidates if not synchronized).
    pub fn written_and_read_regions(&self) -> BTreeSet<RegionId> {
        self.written_regions
            .intersection(&self.read_regions)
            .copied()
            .collect()
    }

    /// Returns true if the function writes to any region that it did not
    /// allocate (cross-function write).
    pub fn has_cross_function_writes(&self) -> bool {
        !self
            .written_regions
            .difference(&self.allocated_regions)
            .collect::<BTreeSet<_>>()
            .is_empty()
    }
}

// ---------------------------------------------------------------------------
// Summary Computation
// ---------------------------------------------------------------------------

/// Computes function summaries for all functions in the SCG.
///
/// Uses the call graph to determine bottom-up processing order.
/// For each function, computes a summary of its direct effects and
/// then merges summaries from callees.
pub fn compute_summaries(
    scg: &SCG,
    call_graph: &CallGraph,
) -> HashMap<FunctionId, FunctionSummary> {
    let mut summaries: HashMap<FunctionId, FunctionSummary> = HashMap::new();

    // Get bottom-up order (callees before callers)
    let order = call_graph.bottom_up_order();

    // Compute direct effects for each function
    for &fid in &order {
        let summary = compute_direct_effects(scg, fid);
        summaries.insert(fid, summary);
    }

    // Merge callee summaries into callers (bottom-up)
    for &fid in &order {
        let callees = call_graph.callees(&fid).to_vec();
        for cge in &callees {
            if let Some(callee_summary) = summaries.get(&cge.callee).cloned() {
                if let Some(summary) = summaries.get_mut(&fid) {
                    summary.merge(&callee_summary);
                    summary.calls.insert(cge.callee);
                }
            }
        }
    }

    summaries
}

/// Compute the direct effects of a single function (no callee merging).
fn compute_direct_effects(scg: &SCG, fid: FunctionId) -> FunctionSummary {
    let entry_node = fid.0;
    let mut summary = FunctionSummary::for_function(fid);

    // Find all nodes belonging to this function.
    // A node belongs to a function if it's between the FunctionEntry and
    // FunctionReturn (reachable via ControlFlow edges from the entry
    // before hitting the return).
    let function_nodes = find_function_nodes(scg, entry_node);

    for &node_id in &function_nodes {
        if let Some(node) = scg.get_node(node_id) {
            match node.node_type {
                NodeType::Allocation => {
                    if let NodePayload::Allocation(alloc) = &node.payload {
                        summary.allocated_regions.insert(alloc.region_id);
                    }
                }
                NodeType::Deallocation => {
                    if let NodePayload::Deallocation(dealloc) = &node.payload {
                        summary.freed_regions.insert(dealloc.region_id);
                    }
                }
                NodeType::Access => {
                    if let NodePayload::Access(access) = &node.payload {
                        match access.mode {
                            AccessMode::Write | AccessMode::ReadWrite => {
                                summary.written_regions.insert(access.region_id);
                            }
                            AccessMode::Read => {
                                summary.read_regions.insert(access.region_id);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Check for potential leaks: allocated but not freed
    summary.may_leak = !summary.leaked_regions().is_empty();

    summary
}

/// Find all nodes that belong to a function, i.e., are reachable from the
/// FunctionEntry via ControlFlow edges before reaching the FunctionReturn.
fn find_function_nodes(scg: &SCG, entry: NodeId) -> HashSet<NodeId> {
    let mut nodes = HashSet::new();
    let mut visited = HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    queue.push_back(entry);
    visited.insert(entry);

    while let Some(current) = queue.pop_front() {
        nodes.insert(current);

        if let Some(succs) = scg.successors(current) {
            for succ in succs {
                if visited.insert(succ) {
                    // Check if the edge to this successor is a ControlFlow or
                    // Call/Return edge (we follow ControlFlow within the function,
                    // and also include nodes reachable via intra-function edges)
                    // We stop at FunctionReturn but include it
                    if let Some(node) = scg.get_node(succ) {
                        if node.node_type == NodeType::Control {
                            if let NodePayload::Control(ctrl) = &node.payload {
                                if ctrl.kind == ControlKind::FunctionReturn {
                                    nodes.insert(succ);
                                    continue; // Don't traverse past return
                                }
                            }
                        }
                    }
                    queue.push_back(succ);
                }
            }
        }
    }

    nodes
}

// ---------------------------------------------------------------------------
// Cross-function Invariant Verification
// ---------------------------------------------------------------------------

/// A cross-function invariant violation detected by summary-based analysis.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InterproceduralViolation {
    /// A function allocates memory that is not freed on any path (cross-function leak).
    CrossFunctionLeak {
        /// The function that leaks.
        function: FunctionId,
        /// The region that is leaked.
        region: RegionId,
    },
    /// A caller and callee both write to the same region without synchronization
    /// (cross-function data race).
    CrossFunctionDataRace {
        /// The caller function.
        caller: FunctionId,
        /// The callee function.
        callee: FunctionId,
        /// The region with conflicting writes.
        region: RegionId,
    },
    /// A function acquires a lock that is not released (cross-function lock leak).
    CrossFunctionLockLeak {
        /// The function that leaks the lock.
        function: FunctionId,
        /// The lock region that is leaked.
        region: RegionId,
    },
    /// A recursive call chain that may leak resources on each recursion.
    RecursiveLeak {
        /// The recursive function.
        function: FunctionId,
        /// The region that may be leaked per recursion.
        region: RegionId,
    },
}

impl std::fmt::Display for InterproceduralViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InterproceduralViolation::CrossFunctionLeak { function, region } => {
                write!(
                    f,
                    "cross-function leak: function {} leaks region {}",
                    function, region
                )
            }
            InterproceduralViolation::CrossFunctionDataRace {
                caller,
                callee,
                region,
            } => {
                write!(
                    f,
                    "cross-function data race: caller {} and callee {} both write region {}",
                    caller, callee, region
                )
            }
            InterproceduralViolation::CrossFunctionLockLeak { function, region } => {
                write!(
                    f,
                    "cross-function lock leak: function {} does not release lock for region {}",
                    function, region
                )
            }
            InterproceduralViolation::RecursiveLeak { function, region } => {
                write!(
                    f,
                    "recursive leak: recursive function {} may leak region {} per recursion",
                    function, region
                )
            }
        }
    }
}

/// Verify interprocedural invariants using summary-based analysis.
///
/// This performs the following checks:
/// 1. **Cross-function leaks**: Functions that allocate but don't free.
/// 2. **Cross-function data races**: Caller and callee write to the same region.
/// 3. **Recursive leaks**: Recursive functions that may leak per recursion.
/// 4. **Lock discipline**: Functions that acquire locks without releasing them.
pub fn verify_interprocedural_invariants(
    _scg: &SCG,
    call_graph: &CallGraph,
    summaries: &HashMap<FunctionId, FunctionSummary>,
) -> Vec<InterproceduralViolation> {
    let mut violations = Vec::new();

    for (&fid, summary) in summaries {
        // Check for cross-function leaks
        for region in summary.leaked_regions() {
            violations.push(InterproceduralViolation::CrossFunctionLeak {
                function: fid,
                region,
            });
        }

        // Check for cross-function data races:
        // If the caller writes to a region and a callee also writes to the same region,
        // that's a potential data race.
        for cge in call_graph.callees(&fid) {
            if let Some(callee_summary) = summaries.get(&cge.callee) {
                let conflicting: BTreeSet<RegionId> = summary
                    .written_regions
                    .intersection(&callee_summary.written_regions)
                    .copied()
                    .collect();
                for region in conflicting {
                    violations.push(InterproceduralViolation::CrossFunctionDataRace {
                        caller: fid,
                        callee: cge.callee,
                        region,
                    });
                }
            }
        }

        // Check for recursive leaks
        if call_graph.is_recursive(&fid) {
            for region in summary.leaked_regions() {
                violations.push(InterproceduralViolation::RecursiveLeak {
                    function: fid,
                    region,
                });
            }
        }

        // Check for lock discipline
        let leaked_locks: BTreeSet<RegionId> = summary
            .acquired_locks
            .difference(&summary.released_locks)
            .copied()
            .collect();
        for region in leaked_locks {
            violations.push(InterproceduralViolation::CrossFunctionLockLeak {
                function: fid,
                region,
            });
        }
    }

    violations
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use vuma_scg::edge::EdgeKind;
    use vuma_scg::graph::SCG;
    use vuma_scg::node::{
        AccessNode, AllocationNode, ComputationKind, ComputationNode, ControlKind, ControlNode,
        DeallocationNode, NodePayload, NodeType, ProgramPoint,
    };
    use vuma_scg::region::{DeploymentTarget, RegionId, SCGRegion};

    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: Some("test.vu".into()),
            line: Some(1),
            column: Some(1),
            offset: None,
        }
    }

    /// Helper: Build a simple two-function SCG with a call from main to foo.
    /// foo allocates region 1 and deallocates it (clean).
    fn build_clean_call_scg() -> (SCG, NodeId, NodeId) {
        let mut scg = SCG::new();
        let rid = RegionId::new(1);

        // main function
        let main_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("main".into()),
            }),
            pp(),
        );
        let call_site = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("call foo".into()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let main_ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: Some("main".into()),
            }),
            pp(),
        );
        scg.add_edge(main_entry, call_site, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(call_site, main_ret, EdgeKind::ControlFlow)
            .unwrap();

        // foo function: alloc and dealloc region 1
        let foo_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("foo".into()),
            }),
            pp(),
        );
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
        let dealloc = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc,
                region_id: rid,
            }),
            pp(),
        );
        let foo_ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: Some("foo".into()),
            }),
            pp(),
        );
        scg.add_edge(foo_entry, alloc, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(alloc, dealloc, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(dealloc, foo_ret, EdgeKind::ControlFlow)
            .unwrap();

        // Call and return edges
        scg.add_call_edge(call_site, foo_entry, rid).unwrap();
        scg.add_return_edge(foo_ret, call_site, vec![]).unwrap();

        (scg, main_entry, foo_entry)
    }

    #[test]
    fn test_clean_call_no_leaks() {
        let (scg, _main_entry, _foo_entry) = build_clean_call_scg();
        let cg = CallGraph::build(&scg);
        let summaries = compute_summaries(&scg, &cg);

        let violations = verify_interprocedural_invariants(&scg, &cg, &summaries);
        let leak_violations: Vec<_> = violations
            .iter()
            .filter(|v| matches!(v, InterproceduralViolation::CrossFunctionLeak { .. }))
            .collect();
        assert!(
            leak_violations.is_empty(),
            "Expected no cross-function leaks, got: {:?}",
            leak_violations
        );
    }

    #[test]
    fn test_cross_function_leak_detected() {
        let mut scg = SCG::new();
        let rid = RegionId::new(1);

        // main function calls foo, which allocates but does NOT free
        let main_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("main".into()),
            }),
            pp(),
        );
        let call_site = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("call foo".into()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let main_ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(main_entry, call_site, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(call_site, main_ret, EdgeKind::ControlFlow)
            .unwrap();

        let foo_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("foo".into()),
            }),
            pp(),
        );
        let _alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: None,
            }),
            pp(),
        );
        // No deallocation!
        let foo_ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(foo_entry, _alloc, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(_alloc, foo_ret, EdgeKind::ControlFlow)
            .unwrap();

        scg.add_call_edge(call_site, foo_entry, rid).unwrap();
        scg.add_return_edge(foo_ret, call_site, vec![]).unwrap();

        let cg = CallGraph::build(&scg);
        let summaries = compute_summaries(&scg, &cg);
        let violations = verify_interprocedural_invariants(&scg, &cg, &summaries);

        let leak_violations: Vec<_> = violations
            .iter()
            .filter(|v| matches!(v, InterproceduralViolation::CrossFunctionLeak { .. }))
            .collect();
        assert!(!leak_violations.is_empty(), "Expected cross-function leak");
    }

    #[test]
    fn test_cross_function_data_race_detected() {
        let mut scg = SCG::new();
        let rid = RegionId::new(1);

        // main writes to region 1, calls foo which also writes to region 1
        let main_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("main".into()),
            }),
            pp(),
        );
        let main_write = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: rid,
                offset: None,
                access_size: Some(8),
            }),
            pp(),
        );
        let call_site = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("call foo".into()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let main_ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(main_entry, main_write, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(main_write, call_site, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(call_site, main_ret, EdgeKind::ControlFlow)
            .unwrap();

        let foo_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("foo".into()),
            }),
            pp(),
        );
        let foo_write = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: rid,
                offset: None,
                access_size: Some(8),
            }),
            pp(),
        );
        let foo_ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(foo_entry, foo_write, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(foo_write, foo_ret, EdgeKind::ControlFlow)
            .unwrap();

        scg.add_call_edge(call_site, foo_entry, rid).unwrap();
        scg.add_return_edge(foo_ret, call_site, vec![]).unwrap();

        let cg = CallGraph::build(&scg);
        let summaries = compute_summaries(&scg, &cg);
        let violations = verify_interprocedural_invariants(&scg, &cg, &summaries);

        let race_violations: Vec<_> = violations
            .iter()
            .filter(|v| matches!(v, InterproceduralViolation::CrossFunctionDataRace { .. }))
            .collect();
        assert!(
            !race_violations.is_empty(),
            "Expected cross-function data race"
        );
    }

    #[test]
    fn test_recursive_function_leak() {
        let mut scg = SCG::new();
        let rid = RegionId::new(1);

        // Recursive function that allocates but doesn't free
        let f_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("recursive".into()),
            }),
            pp(),
        );
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
        let call_self = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("call recursive".into()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let f_ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(f_entry, alloc, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(alloc, call_self, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(call_self, f_ret, EdgeKind::ControlFlow)
            .unwrap();

        scg.add_call_edge(call_self, f_entry, rid).unwrap();
        scg.add_return_edge(f_ret, call_self, vec![]).unwrap();

        let cg = CallGraph::build(&scg);
        let summaries = compute_summaries(&scg, &cg);
        let violations = verify_interprocedural_invariants(&scg, &cg, &summaries);

        let recursive_leaks: Vec<_> = violations
            .iter()
            .filter(|v| matches!(v, InterproceduralViolation::RecursiveLeak { .. }))
            .collect();
        assert!(!recursive_leaks.is_empty(), "Expected recursive leak");
    }

    #[test]
    fn test_summary_merge() {
        let rid1 = RegionId::new(1);
        let rid2 = RegionId::new(2);
        let fid = FunctionId(vuma_scg::node::NodeId::new(1));

        let mut s1 = FunctionSummary::for_function(fid);
        s1.allocated_regions.insert(rid1);
        s1.written_regions.insert(rid1);

        let mut s2 = FunctionSummary::new();
        s2.allocated_regions.insert(rid2);
        s2.read_regions.insert(rid1);

        s1.merge(&s2);

        assert!(s1.allocated_regions.contains(&rid1));
        assert!(s1.allocated_regions.contains(&rid2));
        assert!(s1.written_regions.contains(&rid1));
        assert!(s1.read_regions.contains(&rid1));
    }

    #[test]
    fn test_no_violations_in_well_formed_program() {
        let (scg, _main_entry, _foo_entry) = build_clean_call_scg();
        let cg = CallGraph::build(&scg);
        let summaries = compute_summaries(&scg, &cg);
        let violations = verify_interprocedural_invariants(&scg, &cg, &summaries);
        assert!(
            violations.is_empty(),
            "Expected no violations, got: {:?}",
            violations
        );
    }

    #[test]
    fn test_function_call_edges_in_scg() {
        let (scg, _main_entry, _foo_entry) = build_clean_call_scg();

        // Verify Call and Return edges exist
        let call_edges = scg.call_edges();
        let return_edges = scg.return_edges();

        assert_eq!(call_edges.len(), 1, "Expected 1 call edge");
        assert_eq!(return_edges.len(), 1, "Expected 1 return edge");

        // Verify edge kinds
        assert!(matches!(call_edges[0].kind, EdgeKind::Call { .. }));
        assert!(matches!(return_edges[0].kind, EdgeKind::Return { .. }));
    }

    #[test]
    fn test_cross_function_no_data_race_when_read_only() {
        let mut scg = SCG::new();
        let rid = RegionId::new(1);

        // main writes to region 1, calls foo which only reads region 1
        let main_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("main".into()),
            }),
            pp(),
        );
        let main_write = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: rid,
                offset: None,
                access_size: Some(8),
            }),
            pp(),
        );
        let call_site = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("call foo".into()),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        );
        let main_ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(main_entry, main_write, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(main_write, call_site, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(call_site, main_ret, EdgeKind::ControlFlow)
            .unwrap();

        let foo_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("foo".into()),
            }),
            pp(),
        );
        let foo_read = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: rid,
                offset: None,
                access_size: Some(8),
            }),
            pp(),
        );
        let foo_ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(foo_entry, foo_read, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(foo_read, foo_ret, EdgeKind::ControlFlow)
            .unwrap();

        scg.add_call_edge(call_site, foo_entry, rid).unwrap();
        scg.add_return_edge(foo_ret, call_site, vec![]).unwrap();

        let cg = CallGraph::build(&scg);
        let summaries = compute_summaries(&scg, &cg);
        let violations = verify_interprocedural_invariants(&scg, &cg, &summaries);

        // Main writes, foo only reads — no data race (both need to write)
        let race_violations: Vec<_> = violations
            .iter()
            .filter(|v| matches!(v, InterproceduralViolation::CrossFunctionDataRace { .. }))
            .collect();
        assert!(
            race_violations.is_empty(),
            "Expected no data race when callee only reads, got: {:?}",
            race_violations
        );
    }
}
