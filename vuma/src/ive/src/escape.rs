//! Escape analysis for Invariant 1 (Memory Safety).
//!
//! This module implements `analyze_escapes`, which determines whether pointers
//! derived from a region escape their defining scope. A pointer that does not
//! escape can be considered safe for region-based memory management.
//!
//! # Escape Kinds
//!
//! - **DoesNotEscape**: The pointer stays within its region and scope. Safe.
//! - **EscapesToHeap**: The pointer is stored to a heap-allocated structure,
//!   extending its lifetime beyond the current stack frame.
//! - **EscapesToCaller**: The pointer is returned or passed to a caller,
//!   making its lifetime depend on the caller's behavior.

use std::collections::HashMap;
use vuma_scg::edge::EdgeKind;
use vuma_scg::graph::SCG;
use vuma_scg::node::{AccessMode, NodeId, NodePayload, NodeType};

/// Classification of how a pointer derived from a region may escape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EscapeKind {
    /// The pointer does not escape its defining scope — considered safe.
    DoesNotEscape,
    /// The pointer escapes to the heap (e.g., stored in a heap-allocated struct).
    EscapesToHeap,
    /// The pointer escapes to the caller (e.g., returned from a function).
    EscapesToCaller,
}

impl std::fmt::Display for EscapeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EscapeKind::DoesNotEscape => write!(f, "DoesNotEscape"),
            EscapeKind::EscapesToHeap => write!(f, "EscapesToHeap"),
            EscapeKind::EscapesToCaller => write!(f, "EscapesToCaller"),
        }
    }
}

/// Analyze escape behavior of all pointer-deriving nodes in the SCG.
///
/// For each node that derives a pointer from a region (allocations and
/// accesses), determine whether the derived pointer escapes. The analysis
/// examines edges to determine:
///
/// - **DataFlow edges** from a pointer to a write access → may escape to heap
/// - **ControlFlow edges** to FunctionReturn → escapes to caller
/// - **Derivation edges** → propagate escape kind through the chain
///
/// Nodes that have no escaping edges are classified as `DoesNotEscape`.
pub fn analyze_escapes(scg: &SCG) -> HashMap<NodeId, EscapeKind> {
    let mut result: HashMap<NodeId, EscapeKind> = HashMap::new();

    // Collect all pointer-deriving nodes (allocations and accesses)
    let mut pointer_nodes: Vec<NodeId> = Vec::new();
    for node in scg.nodes() {
        match node.node_type {
            NodeType::Allocation | NodeType::Access => {
                pointer_nodes.push(node.id);
            }
            _ => {}
        }
    }

    // Build adjacency for escape propagation
    // Map: source node → list of (target node, edge kind)
    let mut outgoing: HashMap<NodeId, Vec<(NodeId, EdgeKind)>> = HashMap::new();
    for edge in scg.edges() {
        outgoing
            .entry(edge.source)
            .or_default()
            .push((edge.target, edge.kind.clone()));
    }

    // Identify FunctionReturn control nodes (pointers flowing to these escape to caller)
    let mut return_nodes: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
    for node in scg.nodes() {
        if node.node_type == NodeType::Control {
            if let NodePayload::Control(ctrl) = &node.payload {
                if ctrl.kind == vuma_scg::node::ControlKind::FunctionReturn {
                    return_nodes.insert(node.id);
                }
            }
        }
    }

    // Identify heap-storing write accesses
    let mut write_access_nodes: std::collections::HashSet<NodeId> =
        std::collections::HashSet::new();
    for node in scg.nodes() {
        if node.node_type == NodeType::Access {
            if let NodePayload::Access(access) = &node.payload {
                if access.mode == AccessMode::Write || access.mode == AccessMode::ReadWrite {
                    write_access_nodes.insert(node.id);
                }
            }
        }
    }

    // For each pointer node, check if it escapes
    for &ptr_node in &pointer_nodes {
        let escape =
            compute_escape_kind(ptr_node, &outgoing, &return_nodes, &write_access_nodes, scg);
        result.insert(ptr_node, escape);
    }

    // Propagate escape kinds through derivation chains
    // If A derives from B and B escapes, then A also escapes (at least as much)
    let changed = propagate_escapes(&mut result, &outgoing);
    let _ = changed; // propagation complete

    // Assign DoesNotEscape to any remaining pointer nodes
    for &ptr_node in &pointer_nodes {
        result.entry(ptr_node).or_insert(EscapeKind::DoesNotEscape);
    }

    result
}

/// Compute the initial escape kind for a single node based on its outgoing edges.
fn compute_escape_kind(
    node: NodeId,
    outgoing: &HashMap<NodeId, Vec<(NodeId, EdgeKind)>>,
    return_nodes: &std::collections::HashSet<NodeId>,
    write_access_nodes: &std::collections::HashSet<NodeId>,
    scg: &SCG,
) -> EscapeKind {
    let mut max_escape = EscapeKind::DoesNotEscape;

    if let Some(edges) = outgoing.get(&node) {
        for &(target, ref kind) in edges {
            match kind {
                EdgeKind::DataFlow => {
                    // Data flow to a write access → escapes to heap
                    if write_access_nodes.contains(&target) {
                        max_escape = worse_escape(max_escape, EscapeKind::EscapesToHeap);
                    }
                    // Data flow to a return → escapes to caller
                    if return_nodes.contains(&target) {
                        max_escape = worse_escape(max_escape, EscapeKind::EscapesToCaller);
                    }
                    // Check if target is a control node (function return)
                    if let Some(target_node) = scg.get_node(target) {
                        if target_node.node_type == NodeType::Control {
                            if let NodePayload::Control(ctrl) = &target_node.payload {
                                if ctrl.kind == vuma_scg::node::ControlKind::FunctionReturn {
                                    max_escape =
                                        worse_escape(max_escape, EscapeKind::EscapesToCaller);
                                }
                            }
                        }
                    }
                }
                EdgeKind::ControlFlow => {
                    // Control flow to a return node → escapes to caller
                    if return_nodes.contains(&target) {
                        max_escape = worse_escape(max_escape, EscapeKind::EscapesToCaller);
                    }
                }
                EdgeKind::Derivation => {
                    // Derivation edges propagate escape — handled in propagation pass
                }
                EdgeKind::Annotation | EdgeKind::Dispatch => {
                    // These don't directly cause escape
                }
                EdgeKind::Call { .. } | EdgeKind::Return { .. } => {
                    // Interprocedural edges: a call transfers control to a callee;
                    // if the pointer is passed as an argument, it escapes to callee.
                    // A return brings the pointer back. For now, we conservatively
                    // treat Call edges from a pointer as escaping to the callee.
                    if let Some(target_node) = scg.get_node(target) {
                        if target_node.node_type == NodeType::Control {
                            if let NodePayload::Control(ctrl) = &target_node.payload {
                                if ctrl.kind == vuma_scg::node::ControlKind::FunctionEntry {
                                    max_escape =
                                        worse_escape(max_escape, EscapeKind::EscapesToCaller);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    max_escape
}

/// Return the "worse" (more escaping) of two escape kinds.
fn worse_escape(a: EscapeKind, b: EscapeKind) -> EscapeKind {
    use EscapeKind::*;
    match (a, b) {
        (EscapesToCaller, _) | (_, EscapesToCaller) => EscapesToCaller,
        (EscapesToHeap, _) | (_, EscapesToHeap) => EscapesToHeap,
        (DoesNotEscape, DoesNotEscape) => DoesNotEscape,
    }
}

/// Propagate escape kinds through derivation edges.
///
/// If node A has a derivation edge to node B, and B escapes, then A
/// should escape at least as much. We iterate until fixed point.
fn propagate_escapes(
    result: &mut HashMap<NodeId, EscapeKind>,
    outgoing: &HashMap<NodeId, Vec<(NodeId, EdgeKind)>>,
) -> bool {
    let mut changed = true;
    let mut any_changed = false;

    while changed {
        changed = false;
        // Collect updates to avoid borrow issues
        let mut updates: Vec<(NodeId, EscapeKind)> = Vec::new();

        for (&node, edges) in outgoing.iter() {
            for &(target, ref kind) in edges {
                if *kind == EdgeKind::Derivation {
                    // If target escapes, source must escape at least as much
                    if let Some(&target_escape) = result.get(&target) {
                        let current = result
                            .get(&node)
                            .copied()
                            .unwrap_or(EscapeKind::DoesNotEscape);
                        let new_escape = worse_escape(current, target_escape);
                        if new_escape != current {
                            updates.push((node, new_escape));
                        }
                    }
                    // Also: if source escapes, target inherits
                    if let Some(&source_escape) = result.get(&node) {
                        let current = result
                            .get(&target)
                            .copied()
                            .unwrap_or(EscapeKind::DoesNotEscape);
                        let new_escape = worse_escape(current, source_escape);
                        if new_escape != current {
                            updates.push((target, new_escape));
                        }
                    }
                }
            }
        }

        for (node, new_escape) in updates {
            let current = result
                .get(&node)
                .copied()
                .unwrap_or(EscapeKind::DoesNotEscape);
            if new_escape != current {
                result.insert(node, new_escape);
                changed = true;
                any_changed = true;
            }
        }
    }

    any_changed
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
        AccessNode, AllocationNode, ComputationNode, ControlKind, ControlNode, NodePayload,
        NodeType, ProgramPoint,
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

    #[test]
    fn test_no_escape_simple_alloc() {
        // An allocation with no outgoing edges should not escape
        let mut scg = SCG::new();
        let rid = RegionId::new(1);
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: Some("Buf".into()),
            }),
            pp(),
        );
        let mut region = SCGRegion::new(rid, DeploymentTarget::Heap);
        region.add_node(alloc);
        scg.add_region(region);

        let result = analyze_escapes(&scg);
        assert_eq!(result.get(&alloc), Some(&EscapeKind::DoesNotEscape));
    }

    #[test]
    fn test_escape_to_heap_via_write() {
        // Pointer from alloc flows via DataFlow to a write access → escapes to heap
        let mut scg = SCG::new();
        let rid = RegionId::new(1);
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: Some("Buf".into()),
            }),
            pp(),
        );
        let write_access = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: rid,
                offset: None,
                access_size: Some(8),
            }),
            pp(),
        );
        scg.add_edge(alloc, write_access, EdgeKind::DataFlow)
            .unwrap();

        let result = analyze_escapes(&scg);
        assert_eq!(result.get(&alloc), Some(&EscapeKind::EscapesToHeap));
    }

    #[test]
    fn test_escape_to_caller_via_return() {
        // Pointer flows to a FunctionReturn → escapes to caller
        let mut scg = SCG::new();
        let rid = RegionId::new(1);
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: Some("Buf".into()),
            }),
            pp(),
        );
        let ret = scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind: ControlKind::FunctionReturn,
                label: None,
            }),
            pp(),
        );
        scg.add_edge(alloc, ret, EdgeKind::DataFlow).unwrap();

        let result = analyze_escapes(&scg);
        assert_eq!(result.get(&alloc), Some(&EscapeKind::EscapesToCaller));
    }

    #[test]
    fn test_escape_propagation_through_derivation() {
        // A →(Derivation)→ B, where B escapes to heap → A also escapes
        let mut scg = SCG::new();
        let rid = RegionId::new(1);
        let alloc_a = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: Some("A".into()),
            }),
            pp(),
        );
        let alloc_b = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: Some("B".into()),
            }),
            pp(),
        );
        let write_access = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: rid,
                offset: None,
                access_size: Some(8),
            }),
            pp(),
        );
        // B flows to write (escapes to heap)
        scg.add_edge(alloc_b, write_access, EdgeKind::DataFlow)
            .unwrap();
        // A derives from B
        scg.add_edge(alloc_a, alloc_b, EdgeKind::Derivation)
            .unwrap();

        let result = analyze_escapes(&scg);
        // B escapes to heap; A should also escape to heap via derivation
        assert_eq!(result.get(&alloc_b), Some(&EscapeKind::EscapesToHeap));
        assert_eq!(result.get(&alloc_a), Some(&EscapeKind::EscapesToHeap));
    }

    #[test]
    fn test_escape_kind_ordering() {
        assert_eq!(
            worse_escape(EscapeKind::DoesNotEscape, EscapeKind::EscapesToHeap),
            EscapeKind::EscapesToHeap
        );
        assert_eq!(
            worse_escape(EscapeKind::EscapesToHeap, EscapeKind::EscapesToCaller),
            EscapeKind::EscapesToCaller
        );
        assert_eq!(
            worse_escape(EscapeKind::DoesNotEscape, EscapeKind::DoesNotEscape),
            EscapeKind::DoesNotEscape
        );
    }

    #[test]
    fn test_escape_kind_display() {
        assert_eq!(format!("{}", EscapeKind::DoesNotEscape), "DoesNotEscape");
        assert_eq!(format!("{}", EscapeKind::EscapesToHeap), "EscapesToHeap");
        assert_eq!(
            format!("{}", EscapeKind::EscapesToCaller),
            "EscapesToCaller"
        );
    }

    #[test]
    fn test_escape_empty_scg() {
        let scg = SCG::new();
        let result = analyze_escapes(&scg);
        assert!(result.is_empty());
    }

    #[test]
    fn test_escape_read_access_does_not_escape() {
        // A pointer that only flows to a read access should not escape
        let mut scg = SCG::new();
        let rid = RegionId::new(1);
        let alloc = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: Some("Buf".into()),
            }),
            pp(),
        );
        let read_access = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: rid,
                offset: None,
                access_size: Some(8),
            }),
            pp(),
        );
        scg.add_edge(alloc, read_access, EdgeKind::DataFlow)
            .unwrap();

        let result = analyze_escapes(&scg);
        assert_eq!(result.get(&alloc), Some(&EscapeKind::DoesNotEscape));
    }

    #[test]
    fn test_escape_worse_escape_symmetry() {
        assert_eq!(
            worse_escape(EscapeKind::EscapesToHeap, EscapeKind::DoesNotEscape),
            EscapeKind::EscapesToHeap
        );
        assert_eq!(
            worse_escape(EscapeKind::EscapesToCaller, EscapeKind::EscapesToHeap),
            EscapeKind::EscapesToCaller
        );
    }
}
