//! Call Graph Construction for Interprocedural Analysis.
//!
//! This module builds a call graph from the SCG's Call and Return edges.
//! The call graph represents the caller-callee relationships between functions
//! and supports top-down and bottom-up traversal for summary-based analysis.

use crate::edge::EdgeKind;
use crate::graph::SCG;
use crate::node::{ControlKind, NodeId, NodePayload, NodeType};
use crate::region::RegionId;
use hashbrown::{HashMap, HashSet};

/// Identifier for a function in the call graph.
///
/// Each function is identified by its FunctionEntry node's `NodeId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FunctionId(pub NodeId);

impl std::fmt::Display for FunctionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Func({})", self.0)
    }
}

/// An edge in the call graph, representing a caller→callee relationship.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallGraphEdge {
    /// The function that makes the call.
    pub caller: FunctionId,
    /// The function being called.
    pub callee: FunctionId,
    /// The SCG Call edge that this call-graph edge corresponds to.
    pub call_edge_source: NodeId,
    /// The region of the caller at the call site.
    pub caller_region: RegionId,
}

/// The call graph built from the SCG.
///
/// Nodes are functions (identified by their FunctionEntry `NodeId`),
/// and edges represent caller→callee relationships derived from
/// the SCG's `EdgeKind::Call` edges.
#[derive(Debug, Clone)]
pub struct CallGraph {
    /// All functions in the call graph.
    functions: HashSet<FunctionId>,
    /// Edges: caller → list of (callee, call_edge_source, caller_region).
    call_edges: HashMap<FunctionId, Vec<CallGraphEdge>>,
    /// Reverse edges: callee → list of callers.
    reverse_edges: HashMap<FunctionId, Vec<FunctionId>>,
    /// Map from FunctionId to the FunctionReturn node.
    function_returns: HashMap<FunctionId, NodeId>,
    /// Map from FunctionId to the function label (from the ControlNode).
    function_labels: HashMap<FunctionId, String>,
}

impl CallGraph {
    /// Build a call graph from the given SCG.
    pub fn build(scg: &SCG) -> Self {
        let mut cg = CallGraph {
            functions: HashSet::new(),
            call_edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            function_returns: HashMap::new(),
            function_labels: HashMap::new(),
        };

        // Find all FunctionEntry nodes → these are our functions
        for node in scg.nodes() {
            if node.node_type == NodeType::Control {
                if let NodePayload::Control(ctrl) = &node.payload {
                    if ctrl.kind == ControlKind::FunctionEntry {
                        let fid = FunctionId(node.id);
                        cg.functions.insert(fid);
                        if let Some(label) = &ctrl.label {
                            cg.function_labels.insert(fid, label.clone());
                        }
                    }
                }
            }
        }

        // Find FunctionReturn nodes and map them back to their function
        // Strategy: for each FunctionReturn, find the FunctionEntry that
        // can reach it via ControlFlow edges (going backwards)
        for node in scg.nodes() {
            if node.node_type == NodeType::Control {
                if let NodePayload::Control(ctrl) = &node.payload {
                    if ctrl.kind == ControlKind::FunctionReturn {
                        // Find which function this return belongs to
                        // by looking for a FunctionEntry that reaches this node
                        if let Some(entry_id) = cg.find_enclosing_function(scg, node.id) {
                            let fid = FunctionId(entry_id);
                            cg.function_returns.insert(fid, node.id);
                        }
                    }
                }
            }
        }

        // Walk Call edges to build caller→callee relationships
        for edge in scg.edges() {
            if let EdgeKind::Call {
                from_node,
                to_node,
                caller_region,
            } = &edge.kind
            {
                // from_node is in the caller, to_node is the callee's FunctionEntry
                let callee_fid = FunctionId(*to_node);

                // Find the function containing from_node
                let caller_fid = if let Some(entry_id) = cg.find_enclosing_function(scg, *from_node)
                {
                    FunctionId(entry_id)
                } else {
                    // from_node is not inside a known function — it might be the
                    // top-level entry. Create an implicit "root" function.
                    continue;
                };

                let cge = CallGraphEdge {
                    caller: caller_fid,
                    callee: callee_fid,
                    call_edge_source: *from_node,
                    caller_region: *caller_region,
                };

                cg.call_edges.entry(caller_fid).or_default().push(cge);
                cg.reverse_edges
                    .entry(callee_fid)
                    .or_default()
                    .push(caller_fid);
                cg.functions.insert(caller_fid);
                cg.functions.insert(callee_fid);
            }
        }

        cg
    }

    /// Find the FunctionEntry node that encloses the given node by walking
    /// backwards through ControlFlow edges.
    pub fn find_enclosing_function(&self, scg: &SCG, node_id: NodeId) -> Option<NodeId> {
        // BFS backwards from node_id, looking for a FunctionEntry
        let mut visited = HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(node_id);
        visited.insert(node_id);

        while let Some(current) = queue.pop_front() {
            if let Some(node) = scg.get_node(current) {
                if node.node_type == NodeType::Control {
                    if let NodePayload::Control(ctrl) = &node.payload {
                        if ctrl.kind == ControlKind::FunctionEntry {
                            return Some(current);
                        }
                    }
                }
            }
            if let Some(preds) = scg.predecessors(current) {
                for pred in preds {
                    if visited.insert(pred) {
                        queue.push_back(pred);
                    }
                }
            }
        }
        None
    }

    /// Returns all functions in the call graph.
    pub fn functions(&self) -> impl Iterator<Item = &FunctionId> {
        self.functions.iter()
    }

    /// Returns the number of functions in the call graph.
    pub fn function_count(&self) -> usize {
        self.functions.len()
    }

    /// Returns the callees of a given function.
    pub fn callees(&self, fid: &FunctionId) -> &[CallGraphEdge] {
        self.call_edges
            .get(fid)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Returns the callers of a given function.
    pub fn callers(&self, fid: &FunctionId) -> &[FunctionId] {
        self.reverse_edges
            .get(fid)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Returns the FunctionReturn node for a given function, if known.
    pub fn function_return(&self, fid: &FunctionId) -> Option<NodeId> {
        self.function_returns.get(fid).copied()
    }

    /// Returns the label of a function, if available.
    pub fn function_label(&self, fid: &FunctionId) -> Option<&str> {
        self.function_labels.get(fid).map(|s| s.as_str())
    }

    /// Returns true if the function is recursive (appears in its own call
    /// chain).
    pub fn is_recursive(&self, fid: &FunctionId) -> bool {
        let mut visited = HashSet::new();
        let mut stack = vec![*fid];
        while let Some(current) = stack.pop() {
            if current == *fid && !visited.is_empty() {
                return true;
            }
            if visited.insert(current) {
                if let Some(edges) = self.call_edges.get(&current) {
                    for edge in edges {
                        stack.push(edge.callee);
                    }
                }
            }
        }
        false
    }

    /// Returns functions in a bottom-up order (callees before callers).
    ///
    /// This is useful for summary-based analysis where we compute summaries
    /// bottom-up: first for leaf functions, then for their callers.
    pub fn bottom_up_order(&self) -> Vec<FunctionId> {
        // Topological sort of the call graph (ignoring back edges from recursion)
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut in_stack = HashSet::new();

        for &fid in &self.functions {
            self.dfs_postorder(fid, &mut visited, &mut in_stack, &mut result);
        }

        result
    }

    fn dfs_postorder(
        &self,
        fid: FunctionId,
        visited: &mut HashSet<FunctionId>,
        in_stack: &mut HashSet<FunctionId>,
        result: &mut Vec<FunctionId>,
    ) {
        if visited.contains(&fid) {
            return;
        }
        if in_stack.contains(&fid) {
            // Back edge (recursion) — skip to avoid infinite loop
            return;
        }
        in_stack.insert(fid);

        if let Some(edges) = self.call_edges.get(&fid) {
            for edge in edges {
                self.dfs_postorder(edge.callee, visited, in_stack, result);
            }
        }

        in_stack.remove(&fid);
        visited.insert(fid);
        result.push(fid);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::graph::SCG;
    use crate::node::{ControlKind, ControlNode, NodePayload, NodeType, ProgramPoint};
    use crate::region::RegionId;

    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: Some("test.vu".into()),
            line: Some(1),
            column: Some(1),
            offset: None,
        }
    }

    #[test]
    fn test_empty_call_graph() {
        let scg = SCG::new();
        let cg = CallGraph::build(&scg);
        assert_eq!(cg.function_count(), 0);
    }

    #[test]
    fn test_single_function_no_calls() {
        let mut scg = SCG::new();
        let entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("main".into()),
            }),
            pp(),
        );
        let ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: Some("main".into()),
            }),
            pp(),
        );
        scg.add_edge(entry, ret, EdgeKind::ControlFlow).unwrap();

        let cg = CallGraph::build(&scg);
        assert_eq!(cg.function_count(), 1);
        let main_fid = FunctionId(entry);
        assert!(cg.callees(&main_fid).is_empty());
        assert_eq!(cg.function_return(&main_fid), Some(ret));
        assert_eq!(cg.function_label(&main_fid), Some("main"));
    }

    #[test]
    fn test_caller_callee_relationship() {
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
            NodePayload::Computation(crate::node::ComputationNode {
                operation: "call foo".into(),
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

        // foo function
        let foo_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("foo".into()),
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
        scg.add_edge(foo_entry, foo_ret, EdgeKind::ControlFlow)
            .unwrap();

        // Call edge: main → foo
        scg.add_call_edge(call_site, foo_entry, rid).unwrap();
        // Return edge: foo → main
        scg.add_return_edge(foo_ret, call_site, vec![]).unwrap();

        let cg = CallGraph::build(&scg);
        let main_fid = FunctionId(main_entry);
        let foo_fid = FunctionId(foo_entry);

        assert_eq!(cg.callees(&main_fid).len(), 1);
        assert_eq!(cg.callees(&main_fid)[0].callee, foo_fid);
        assert_eq!(cg.callers(&foo_fid).len(), 1);
        assert_eq!(cg.callers(&foo_fid)[0], main_fid);
    }

    #[test]
    fn test_bottom_up_order() {
        let mut scg = SCG::new();
        let rid = RegionId::new(1);

        // main → foo → bar
        let main_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("main".into()),
            }),
            pp(),
        );
        let main_call = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(crate::node::ComputationNode {
                operation: "call foo".into(),
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
        scg.add_edge(main_entry, main_call, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(main_call, main_ret, EdgeKind::ControlFlow)
            .unwrap();

        let foo_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("foo".into()),
            }),
            pp(),
        );
        let foo_call = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(crate::node::ComputationNode {
                operation: "call bar".into(),
                result_type: None,
                tail_call: false,
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
        scg.add_edge(foo_entry, foo_call, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(foo_call, foo_ret, EdgeKind::ControlFlow)
            .unwrap();

        let bar_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("bar".into()),
            }),
            pp(),
        );
        let bar_ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(bar_entry, bar_ret, EdgeKind::ControlFlow)
            .unwrap();

        scg.add_call_edge(main_call, foo_entry, rid).unwrap();
        scg.add_return_edge(foo_ret, main_call, vec![]).unwrap();
        scg.add_call_edge(foo_call, bar_entry, rid).unwrap();
        scg.add_return_edge(bar_ret, foo_call, vec![]).unwrap();

        let cg = CallGraph::build(&scg);
        let order = cg.bottom_up_order();

        // bar must come before foo, foo must come before main
        let bar_pos = order
            .iter()
            .position(|f| *f == FunctionId(bar_entry))
            .unwrap();
        let foo_pos = order
            .iter()
            .position(|f| *f == FunctionId(foo_entry))
            .unwrap();
        let main_pos = order
            .iter()
            .position(|f| *f == FunctionId(main_entry))
            .unwrap();
        assert!(bar_pos < foo_pos);
        assert!(foo_pos < main_pos);
    }

    #[test]
    fn test_recursive_function_detection() {
        let mut scg = SCG::new();
        let rid = RegionId::new(1);

        // Recursive function: f calls itself
        let f_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("recursive_f".into()),
            }),
            pp(),
        );
        let f_call = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(crate::node::ComputationNode {
                operation: "call recursive_f".into(),
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
        scg.add_edge(f_entry, f_call, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(f_call, f_ret, EdgeKind::ControlFlow).unwrap();

        scg.add_call_edge(f_call, f_entry, rid).unwrap();
        scg.add_return_edge(f_ret, f_call, vec![]).unwrap();

        let cg = CallGraph::build(&scg);
        let f_fid = FunctionId(f_entry);
        assert!(cg.is_recursive(&f_fid));
    }

    #[test]
    fn test_non_recursive_function() {
        let mut scg = SCG::new();

        let f_entry = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionEntry,
                label: Some("leaf".into()),
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
        scg.add_edge(f_entry, f_ret, EdgeKind::ControlFlow).unwrap();

        let cg = CallGraph::build(&scg);
        let f_fid = FunctionId(f_entry);
        assert!(!cg.is_recursive(&f_fid));
    }
}
