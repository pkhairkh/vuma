//! Loop Detection on SCG
//!
//! This module provides algorithms for detecting natural loops, computing
//! loop nesting hierarchies, and identifying infinite loops in the SCG.
//!
//! # Key Concepts
//!
//! - **Natural Loop**: A loop with a single entry point (header) that dominates
//!   all nodes in the loop body. Detected via back-edges in the CFG.
//! - **Back-edge**: An edge from a node to one of its dominators. Every
//!   back-edge defines a natural loop.
//! - **Loop Nesting Tree**: A hierarchy showing how loops are nested within
//!   each other, with depth information for optimization passes.
//! - **Infinite Loop**: A loop with no exit edges, meaning execution cannot
//!   escape once the loop is entered.
//!
//! # Algorithm
//!
//! Natural loops are detected using a CFG-only dominator tree:
//! 1. Build a control-flow subgraph (only `EdgeKind::ControlFlow` edges).
//! 2. Compute the dominator tree on this subgraph using the iterative
//!    Cooper-Harvey-Kennedy algorithm.
//! 3. Find all back-edges (edges where the target dominates the source).
//! 4. For each back-edge, compute the natural loop body by walking predecessors
//!    from the back-edge source until reaching the header.
//! 5. Compute loop exits (nodes in the body with successors outside the body).
//! 6. Build the nesting tree by analyzing header/body containment.
//!
//! # Why a Separate Dominator Computation?
//!
//! The general-purpose [`crate::dominance::compute_dominators`] in the `dominance` module
//! operates on **all** edges (including `DataFlow`, `Derivation`, etc.).
//! Loop detection must reason about control-flow reachability only: a
//! DataFlow edge from outside a loop into the loop body does **not** mean
//! there is a control path that bypasses the loop header. Computing
//! dominators on the CFG subgraph ensures correct back-edge identification.

use hashbrown::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

use crate::edge::EdgeKind;
use crate::graph::SCG;
use crate::node::{ControlKind, NodeId, NodePayload, NodeType};

// ─── Natural Loop ─────────────────────────────────────────────────────────

/// A natural loop in the SCG.
///
/// A natural loop is defined by a back-edge from `backedge_source` to `header`,
/// where `header` dominates `backedge_source`. The loop body includes all nodes
/// on paths from `header` to `backedge_source` that do not leave the loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NaturalLoop {
    /// The loop header — the unique entry point that dominates all body nodes.
    pub header: NodeId,
    /// The source of the back-edge that defines this loop.
    pub backedge_source: NodeId,
    /// The set of nodes in the loop body (including header and backedge_source).
    pub body: HashSet<NodeId>,
    /// The set of exit nodes — body nodes that have successors outside the loop.
    pub exits: HashSet<NodeId>,
    /// The nesting depth (0 = outermost, higher = more deeply nested).
    pub depth: u32,
}

impl NaturalLoop {
    /// Creates a new natural loop with the given header and back-edge source.
    pub fn new(header: NodeId, backedge_source: NodeId) -> Self {
        Self {
            header,
            backedge_source,
            body: HashSet::new(),
            exits: HashSet::new(),
            depth: 0,
        }
    }

    /// Returns `true` if the given node is inside this loop's body.
    pub fn contains(&self, node: &NodeId) -> bool {
        self.body.contains(node)
    }

    /// Returns the number of nodes in the loop body.
    pub fn body_size(&self) -> usize {
        self.body.len()
    }

    /// Returns `true` if this loop has at least one exit.
    pub fn has_exit(&self) -> bool {
        !self.exits.is_empty()
    }
}

// ─── Loop Nesting Tree ────────────────────────────────────────────────────

/// A tree representing the nesting hierarchy of natural loops.
///
/// Each loop has a parent (the immediately enclosing loop) and zero or more
/// children (loops directly nested within it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoopNestingTree {
    /// All natural loops in the tree.
    pub loops: Vec<NaturalLoop>,
    /// Map from loop index to parent loop index (None = root loop).
    pub parent: HashMap<usize, Option<usize>>,
}

impl LoopNestingTree {
    /// Creates a new empty nesting tree.
    pub fn new() -> Self {
        Self {
            loops: Vec::new(),
            parent: HashMap::new(),
        }
    }

    /// Returns the number of loops in the tree.
    pub fn len(&self) -> usize {
        self.loops.len()
    }

    /// Returns `true` if the tree contains no loops.
    pub fn is_empty(&self) -> bool {
        self.loops.is_empty()
    }

    /// Returns the root loops (loops with no parent).
    pub fn root_loops(&self) -> Vec<&NaturalLoop> {
        self.loops
            .iter()
            .enumerate()
            .filter(|(i, _)| self.parent.get(i) == Some(&None))
            .map(|(_, l)| l)
            .collect()
    }

    /// Returns the children of the loop at the given index.
    pub fn children(&self, loop_idx: usize) -> Vec<usize> {
        self.parent
            .iter()
            .filter_map(|(&child, &parent)| {
                if parent == Some(loop_idx) {
                    Some(child)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Returns the depth of the loop at the given index.
    pub fn depth(&self, loop_idx: usize) -> u32 {
        self.loops.get(loop_idx).map(|l| l.depth).unwrap_or(0)
    }
}

impl Default for LoopNestingTree {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Loop Detector ────────────────────────────────────────────────────────

/// Detector for natural loops, loop nesting, and infinite loops in the SCG.
///
/// Uses a CFG-only dominator tree to identify back-edges and compute natural
/// loop bodies. This ensures that DataFlow and other non-control edges do not
/// interfere with loop detection.
pub struct LoopDetector;

impl LoopDetector {
    /// Detects all natural loops in the given SCG.
    ///
    /// A natural loop is identified by each back-edge (an edge where the target
    /// dominates the source in the CFG). The loop body is computed by walking
    /// predecessors from the back-edge source until reaching the header.
    ///
    /// # Algorithm
    ///
    /// 1. Build the CFG adjacency lists (only `ControlFlow` edges).
    /// 2. Compute the dominator tree on the CFG subgraph.
    /// 3. Find all back-edges by checking if each CF edge's target dominates
    ///    its source.
    /// 4. For each back-edge, compute the loop body using the standard natural
    ///    loop algorithm (walk CF predecessors from backedge_source to header).
    /// 5. Compute exit nodes (body nodes with CF successors outside the body).
    pub fn detect_natural_loops(scg: &SCG) -> Vec<NaturalLoop> {
        let mut loops = Vec::new();

        // We need an entry node. Find a reasonable one:
        // Use FunctionEntry nodes, or fall back to nodes with no CF predecessors.
        let entry_nodes = Self::find_entry_nodes(scg);
        if entry_nodes.is_empty() {
            return loops;
        }

        // Build CFG-only adjacency lists.
        let (cf_succs, cf_preds) = Self::build_cf_adjacency(scg);

        // Compute dominator tree on the CFG subgraph from the first entry.
        let dom_tree = Self::compute_cfg_dominators(&cf_succs, &cf_preds, entry_nodes[0]);

        // Find all back-edges: CF edges where the target dominates the source.
        let back_edges = Self::find_back_edges(&cf_succs, &dom_tree);

        for (source, header) in back_edges {
            // Skip if we already have a loop with the same header and source.
            if loops
                .iter()
                .any(|l: &NaturalLoop| l.header == header && l.backedge_source == source)
            {
                continue;
            }

            let mut loop_ = NaturalLoop::new(header, source);
            loop_.body.insert(header);
            loop_.body.insert(source);

            // Compute loop body: walk CF predecessors from source until we reach header.
            let mut stack = vec![source];
            while let Some(node) = stack.pop() {
                if let Some(preds) = cf_preds.get(&node) {
                    for &pred in preds {
                        if !loop_.body.contains(&pred) {
                            loop_.body.insert(pred);
                            stack.push(pred);
                        }
                    }
                }
            }

            // Compute exit nodes: body nodes with CF successors outside the loop.
            for &node in &loop_.body {
                if let Some(succs) = cf_succs.get(&node) {
                    for &succ in succs {
                        if !loop_.body.contains(&succ) {
                            loop_.exits.insert(node);
                            break;
                        }
                    }
                }
            }

            loops.push(loop_);
        }

        loops
    }

    /// Detects the loop nesting tree for all natural loops in the SCG.
    ///
    /// The nesting tree shows which loops are contained within other loops.
    /// A loop B is nested within loop A if B's header is in A's body.
    pub fn detect_loop_nesting(scg: &SCG) -> LoopNestingTree {
        let mut natural_loops = Self::detect_natural_loops(scg);
        if natural_loops.is_empty() {
            return LoopNestingTree::new();
        }

        // Assign initial depth of 0 to all loops.
        for loop_ in &mut natural_loops {
            loop_.depth = 0;
        }

        let mut parent_map: HashMap<usize, Option<usize>> = HashMap::new();

        // Determine nesting: loop i is nested in loop j if header of i
        // is in the body of j, and j's body is the smallest containing body.
        for i in 0..natural_loops.len() {
            let mut best_parent: Option<usize> = None;
            let mut best_parent_size = usize::MAX;

            for j in 0..natural_loops.len() {
                if i == j {
                    continue;
                }
                // If loop i's header is in loop j's body, j is a potential parent.
                if natural_loops[j].body.contains(&natural_loops[i].header) {
                    // Prefer the smallest body (most immediate enclosing loop).
                    if natural_loops[j].body_size() < best_parent_size {
                        best_parent_size = natural_loops[j].body_size();
                        best_parent = Some(j);
                    }
                }
            }

            parent_map.insert(i, best_parent);

            // Set depth: parent's depth + 1.
            if let Some(parent_idx) = best_parent {
                natural_loops[i].depth = natural_loops[parent_idx].depth + 1;
            }
        }

        LoopNestingTree {
            loops: natural_loops,
            parent: parent_map,
        }
    }

    /// Detects infinite loops in the SCG.
    ///
    /// An infinite loop is a natural loop with no exit edges. Once entered,
    /// execution cannot escape.
    pub fn detect_infinite_loops(scg: &SCG) -> Vec<NodeId> {
        let loops = Self::detect_natural_loops(scg);
        loops
            .iter()
            .filter(|l| !l.has_exit())
            .map(|l| l.header)
            .collect()
    }

    /// Finds nodes that are loop-invariant within the given loop.
    ///
    /// A node is loop-invariant if:
    /// - It is a computation (no side effects).
    /// - All of its data-flow predecessors are defined outside the loop.
    ///
    /// This is the key analysis for Loop Invariant Code Motion (LICM).
    pub fn loop_invariant_nodes(loop_: &NaturalLoop, scg: &SCG) -> Vec<NodeId> {
        let body_set = &loop_.body;
        let mut invariant = Vec::new();

        for &node_id in &loop_.body {
            if let Some(node) = scg.get_node(node_id) {
                // Skip nodes with side effects.
                if matches!(
                    node.node_type,
                    NodeType::Effect
                        | NodeType::Allocation
                        | NodeType::Deallocation
                        | NodeType::Access
                        | NodeType::Control
                        | NodeType::Phantom
                        | NodeType::VTable
                        | NodeType::ClosureEnv
                ) {
                    continue;
                }

                // Check all data-flow predecessors are outside the loop.
                let mut all_outside = true;
                if let Some(preds) = scg.predecessors(node_id) {
                    for pred in preds {
                        let is_df = scg.edges().any(|e| {
                            e.source == pred
                                && e.target == node_id
                                && matches!(e.kind, EdgeKind::DataFlow)
                        });
                        if is_df && body_set.contains(&pred) {
                            all_outside = false;
                            break;
                        }
                    }
                }

                if all_outside {
                    invariant.push(node_id);
                }
            }
        }

        invariant
    }

    // ─── Internal Helpers ───────────────────────────────────────────────

    /// Finds entry nodes for the SCG (FunctionEntry or nodes with no CF predecessors).
    fn find_entry_nodes(scg: &SCG) -> Vec<NodeId> {
        let mut entries: Vec<NodeId> = scg
            .nodes()
            .filter(|n| {
                if let NodePayload::Control(ctrl) = &n.payload {
                    ctrl.kind == ControlKind::FunctionEntry
                } else {
                    false
                }
            })
            .map(|n| n.id)
            .collect();

        if entries.is_empty() {
            // Fallback: use nodes with no CF predecessors.
            let (_, cf_preds) = Self::build_cf_adjacency(scg);
            for node in scg.nodes() {
                let has_preds = cf_preds.get(&node.id).is_some_and(|p| !p.is_empty());
                if !has_preds {
                    entries.push(node.id);
                }
            }
        }

        entries
    }

    /// Builds adjacency lists for the CFG subgraph (ControlFlow edges only).
    ///
    /// Returns `(succs, preds)` where `succs[n]` is the list of CF successors
    /// of node `n`, and `preds[n]` is the list of CF predecessors.
    fn build_cf_adjacency(
        scg: &SCG,
    ) -> (HashMap<NodeId, Vec<NodeId>>, HashMap<NodeId, Vec<NodeId>>) {
        let mut succs: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut preds: HashMap<NodeId, Vec<NodeId>> = HashMap::new();

        for edge in scg.edges() {
            if matches!(edge.kind, EdgeKind::ControlFlow) {
                succs.entry(edge.source).or_default().push(edge.target);
                preds.entry(edge.target).or_default().push(edge.source);
            }
        }

        (succs, preds)
    }

    /// Computes the dominator tree on the CFG subgraph using the iterative
    /// Cooper-Harvey-Kennedy algorithm.
    ///
    /// This is a simple, correct algorithm that computes immediate dominators
    /// by iterating to a fixpoint. It operates on the CFG adjacency lists
    /// directly, ensuring that only ControlFlow edges are considered.
    ///
    /// # Complexity
    ///
    /// O(N * D * iter) where N is the number of reachable nodes, D is the
    /// average number of predecessors, and iter is the number of iterations
    /// to convergence (typically very few, 2-5 in practice).
    fn compute_cfg_dominators(
        cf_succs: &HashMap<NodeId, Vec<NodeId>>,
        cf_preds: &HashMap<NodeId, Vec<NodeId>>,
        entry: NodeId,
    ) -> CfgDomTree {
        // Step 1: Find all nodes reachable from entry via CF edges.
        let reachable = {
            let mut visited = HashSet::new();
            let mut stack = vec![entry];
            while let Some(node) = stack.pop() {
                if visited.insert(node) {
                    if let Some(succ_list) = cf_succs.get(&node) {
                        for &succ in succ_list {
                            if !visited.contains(&succ) {
                                stack.push(succ);
                            }
                        }
                    }
                }
            }
            visited
        };

        if reachable.is_empty() {
            return CfgDomTree {
                entry,
                idom: HashMap::new(),
            };
        }

        // Step 2: Compute a reverse postorder (RPO) of the CFG.
        let rpo = Self::reverse_postorder(cf_succs, entry, &reachable);

        // Build RPO index map for the intersect function.
        let rpo_idx: HashMap<NodeId, usize> =
            rpo.iter().enumerate().map(|(i, &node)| (node, i)).collect();

        // Step 3: Iterative dominator computation (Cooper-Harvey-Kennedy).
        let mut idom: HashMap<NodeId, NodeId> = HashMap::new();
        idom.insert(entry, entry);

        let mut changed = true;
        while changed {
            changed = false;
            for &node in &rpo {
                if node == entry {
                    continue;
                }

                // Find the set of processed predecessors.
                let pred_list: Vec<NodeId> = cf_preds
                    .get(&node)
                    .map(|p| {
                        p.iter()
                            .filter(|p| idom.contains_key(*p))
                            .copied()
                            .collect()
                    })
                    .unwrap_or_default();

                let new_idom = match pred_list.first() {
                    None => continue, // No processed predecessor; skip.
                    Some(&first) => first,
                };

                // Intersect all processed predecessors.
                let mut new_idom = new_idom;
                for &pred in &pred_list[1..] {
                    new_idom = Self::intersect(&idom, new_idom, pred, &rpo_idx);
                }

                if idom.get(&node) != Some(&new_idom) {
                    idom.insert(node, new_idom);
                    changed = true;
                }
            }
        }

        // Remove self-dominance of entry.
        idom.remove(&entry);

        CfgDomTree { entry, idom }
    }

    /// Computes the reverse postorder of the CFG from `entry`.
    fn reverse_postorder(
        cf_succs: &HashMap<NodeId, Vec<NodeId>>,
        entry: NodeId,
        reachable: &HashSet<NodeId>,
    ) -> Vec<NodeId> {
        let mut visited = HashSet::new();
        let mut post_order = Vec::new();
        let mut stack = vec![(entry, false)];

        while let Some((node, processed)) = stack.pop() {
            if processed {
                post_order.push(node);
                continue;
            }
            if !reachable.contains(&node) || visited.contains(&node) {
                continue;
            }
            visited.insert(node);
            stack.push((node, true));
            if let Some(succ_list) = cf_succs.get(&node) {
                for &succ in succ_list.iter().rev() {
                    if !visited.contains(&succ) && reachable.contains(&succ) {
                        stack.push((succ, false));
                    }
                }
            }
        }

        // Reverse postorder = reverse of postorder.
        post_order.reverse();
        post_order
    }

    /// Intersects two nodes in the dominator tree using the RPO-based
    /// algorithm from Cooper-Harvey-Kennedy.
    ///
    /// Walks up the idom chains of both fingers, always advancing the one
    /// with the higher RPO number, until they meet at their common dominator.
    fn intersect(
        idom: &HashMap<NodeId, NodeId>,
        mut finger1: NodeId,
        mut finger2: NodeId,
        rpo_idx: &HashMap<NodeId, usize>,
    ) -> NodeId {
        loop {
            // Walk the finger with the higher RPO number (deeper in the tree).
            let idx1 = rpo_idx.get(&finger1).copied().unwrap_or(0);
            let idx2 = rpo_idx.get(&finger2).copied().unwrap_or(0);

            if idx1 > idx2 {
                finger1 = idom[&finger1];
            } else if idx2 > idx1 {
                finger2 = idom[&finger2];
            } else {
                // Same RPO index => same node.
                return finger1;
            }
        }
    }

    /// Finds all back-edges in the CFG: CF edges where the target dominates the source.
    fn find_back_edges(
        cf_succs: &HashMap<NodeId, Vec<NodeId>>,
        dom_tree: &CfgDomTree,
    ) -> Vec<(NodeId, NodeId)> {
        let mut back_edges = Vec::new();

        for (&source, succs) in cf_succs {
            for &target in succs {
                // A back-edge: target dominates source.
                if dom_tree.dominates(target, source) {
                    back_edges.push((source, target));
                }
            }
        }

        back_edges
    }
}

// ─── CFG-only Dominator Tree ──────────────────────────────────────────────

/// A simple dominator tree computed on the CFG subgraph only.
///
/// Unlike the general [`DominatorTree`] in `dominance`, this only considers
/// `ControlFlow` edges, which is essential for correct loop detection.
struct CfgDomTree {
    #[allow(dead_code)]
    entry: NodeId,
    idom: HashMap<NodeId, NodeId>,
}

impl CfgDomTree {
    /// Returns `true` if `a` dominates `b` in this dominator tree.
    ///
    /// `a` dominates `b` iff `a` is an ancestor of `b` (or equal to `b`)
    /// in the dominator tree.
    fn dominates(&self, a: NodeId, b: NodeId) -> bool {
        if a == b {
            return true;
        }
        // Walk up from b to see if we hit a.
        let mut current = b;
        while let Some(&parent) = self.idom.get(&current) {
            if parent == a {
                return true;
            }
            current = parent;
        }
        false
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::graph::SCG;
    use crate::node::{
        ComputationNode, ControlKind, ControlNode, NodeId, NodePayload, NodeType, ProgramPoint,
    };

    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        }
    }

    /// Helper to add a control node.
    fn add_ctrl(scg: &mut SCG, kind: ControlKind, label: &str) -> NodeId {
        scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind,
                label: Some(label.to_string()),
            }),
            pp(),
        )
    }

    /// Helper to add a computation node.
    fn add_comp(scg: &mut SCG, op: &str) -> NodeId {
        scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: op.to_string(),
                result_type: None,
                tail_call: false,
            }),
            pp(),
        )
    }

    /// Builds a simple while-loop CFG:
    ///   entry → header → body → latch → header (back-edge)
    ///                  ↘ exit
    fn build_simple_loop() -> (SCG, NodeId, NodeId, NodeId, NodeId) {
        let mut scg = SCG::new();
        let entry = add_ctrl(&mut scg, ControlKind::FunctionEntry, "entry");
        let header = add_ctrl(&mut scg, ControlKind::LoopHeader, "while_cond");
        let body = add_comp(&mut scg, "loop_body");
        let latch = add_comp(&mut scg, "latch");
        let exit = add_ctrl(&mut scg, ControlKind::LoopExit, "while_exit");

        scg.add_edge(entry, header, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(header, body, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(header, exit, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(body, latch, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(latch, header, EdgeKind::ControlFlow).unwrap(); // back-edge

        (scg, header, body, latch, exit)
    }

    // ── Test 1: Simple loop detection ──────────────────────────────────

    #[test]
    fn test_detect_simple_loop() {
        let (scg, header, body, latch, _exit) = build_simple_loop();
        let loops = LoopDetector::detect_natural_loops(&scg);
        assert_eq!(loops.len(), 1, "should detect exactly one natural loop");

        let loop_ = &loops[0];
        assert_eq!(loop_.header, header, "header should be the LoopHeader node");
        assert!(loop_.body.contains(&header), "header in body");
        assert!(loop_.body.contains(&body), "body node in body");
        assert!(loop_.body.contains(&latch), "latch in body");
        assert!(
            loop_.exits.contains(&header),
            "header should be an exit (has edge to exit node)"
        );
    }

    // ── Test 2: No loops in linear CFG ─────────────────────────────────

    #[test]
    fn test_detect_no_loops() {
        let mut scg = SCG::new();
        let n1 = add_comp(&mut scg, "a");
        let n2 = add_comp(&mut scg, "b");
        scg.add_edge(n1, n2, EdgeKind::ControlFlow).unwrap();

        let loops = LoopDetector::detect_natural_loops(&scg);
        assert!(loops.is_empty(), "linear CFG should have no loops");
    }

    // ── Test 3: Nested loops ───────────────────────────────────────────

    #[test]
    fn test_detect_nested_loops() {
        let mut scg = SCG::new();

        let entry = add_ctrl(&mut scg, ControlKind::FunctionEntry, "entry");
        let header1 = add_ctrl(&mut scg, ControlKind::LoopHeader, "outer_header");
        let inner_header = add_ctrl(&mut scg, ControlKind::LoopHeader, "inner_header");
        let inner_body = add_comp(&mut scg, "inner_body");
        let inner_latch = add_comp(&mut scg, "inner_latch");
        let inner_exit = add_ctrl(&mut scg, ControlKind::LoopExit, "inner_exit");
        let outer_body = add_comp(&mut scg, "outer_body");
        let outer_latch = add_comp(&mut scg, "outer_latch");
        let outer_exit = add_ctrl(&mut scg, ControlKind::LoopExit, "outer_exit");

        scg.add_edge(entry, header1, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(header1, inner_header, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(header1, outer_exit, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(inner_header, inner_body, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(inner_header, inner_exit, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(inner_body, inner_latch, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(inner_latch, inner_header, EdgeKind::ControlFlow)
            .unwrap(); // inner back-edge
        scg.add_edge(inner_exit, outer_body, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(outer_body, outer_latch, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(outer_latch, header1, EdgeKind::ControlFlow)
            .unwrap(); // outer back-edge

        let loops = LoopDetector::detect_natural_loops(&scg);
        assert_eq!(loops.len(), 2, "should detect two natural loops");

        let headers: Vec<NodeId> = loops.iter().map(|l| l.header).collect();
        assert!(
            headers.contains(&header1),
            "outer loop header should be detected"
        );
        assert!(
            headers.contains(&inner_header),
            "inner loop header should be detected"
        );
    }

    // ── Test 4: Loop nesting tree (single loop) ────────────────────────

    #[test]
    fn test_loop_nesting_tree() {
        let (scg, _header, _body, _latch, _exit) = build_simple_loop();
        let tree = LoopDetector::detect_loop_nesting(&scg);
        assert_eq!(tree.len(), 1);
        assert!(tree.root_loops().len() >= 1);
        assert_eq!(tree.root_loops()[0].depth, 0);
    }

    // ── Test 5: Loop nesting tree (nested loops) ───────────────────────

    #[test]
    fn test_loop_nesting_tree_nested() {
        let mut scg = SCG::new();

        let entry = add_ctrl(&mut scg, ControlKind::FunctionEntry, "entry");
        let outer_h = add_ctrl(&mut scg, ControlKind::LoopHeader, "outer");
        let inner_h = add_ctrl(&mut scg, ControlKind::LoopHeader, "inner");
        let inner_body = add_comp(&mut scg, "ib");
        let outer_body = add_comp(&mut scg, "ob");
        let exit = add_ctrl(&mut scg, ControlKind::LoopExit, "exit");

        scg.add_edge(entry, outer_h, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(outer_h, inner_h, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(outer_h, exit, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(inner_h, inner_body, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(inner_h, outer_body, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(inner_body, inner_h, EdgeKind::ControlFlow)
            .unwrap(); // inner back-edge
        scg.add_edge(outer_body, outer_h, EdgeKind::ControlFlow)
            .unwrap(); // outer back-edge

        let tree = LoopDetector::detect_loop_nesting(&scg);
        assert_eq!(tree.len(), 2);

        let outer_idx = tree
            .loops
            .iter()
            .position(|l| l.header == outer_h)
            .expect("outer loop should exist");
        let inner_idx = tree
            .loops
            .iter()
            .position(|l| l.header == inner_h)
            .expect("inner loop should exist");

        // Inner loop should be nested inside outer loop.
        assert_eq!(tree.parent[&inner_idx], Some(outer_idx));
        assert_eq!(tree.loops[inner_idx].depth, tree.loops[outer_idx].depth + 1);
    }

    // ── Test 6: Infinite loop detection ────────────────────────────────

    #[test]
    fn test_infinite_loop_detection() {
        let mut scg = SCG::new();

        let entry = add_ctrl(&mut scg, ControlKind::FunctionEntry, "entry");
        let header = add_ctrl(&mut scg, ControlKind::LoopHeader, "infinite");
        let body = add_comp(&mut scg, "spin");

        scg.add_edge(entry, header, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(header, body, EdgeKind::ControlFlow).unwrap();
        // No exit edge from header; only back-edge.
        scg.add_edge(body, header, EdgeKind::ControlFlow).unwrap(); // back-edge

        let infinite = LoopDetector::detect_infinite_loops(&scg);
        assert_eq!(infinite.len(), 1, "should detect one infinite loop");
        assert_eq!(infinite[0], header);
    }

    // ── Test 7: Loop with exit is not infinite ─────────────────────────

    #[test]
    fn test_infinite_loop_empty() {
        let (scg, ..) = build_simple_loop();
        let infinite = LoopDetector::detect_infinite_loops(&scg);
        // The simple loop has an exit, so it should not be infinite.
        assert!(infinite.is_empty(), "loop with exit is not infinite");
    }

    // ── Test 8: Loop-invariant nodes ───────────────────────────────────

    #[test]
    fn test_loop_invariant_nodes() {
        let mut scg = SCG::new();

        // Build a loop with an invariant computation defined outside.
        let entry = add_ctrl(&mut scg, ControlKind::FunctionEntry, "entry");
        // Constant defined outside the loop.
        let constant = add_comp(&mut scg, "const_42");
        let header = add_ctrl(&mut scg, ControlKind::LoopHeader, "header");
        let loop_body = add_comp(&mut scg, "add");
        let exit = add_ctrl(&mut scg, ControlKind::LoopExit, "exit");

        scg.add_edge(entry, constant, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(constant, header, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(header, loop_body, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(header, exit, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(loop_body, header, EdgeKind::ControlFlow)
            .unwrap(); // back-edge
        scg.add_edge(constant, loop_body, EdgeKind::DataFlow)
            .unwrap(); // invariant input from outside

        let loops = LoopDetector::detect_natural_loops(&scg);
        assert!(!loops.is_empty(), "should detect at least one loop");

        // loop_body depends on constant (outside the loop via DataFlow), so
        // it is loop-invariant.
        let invariant = LoopDetector::loop_invariant_nodes(&loops[0], &scg);
        assert!(
            invariant.contains(&loop_body),
            "loop_body should be loop-invariant since all DF preds are outside"
        );
    }

    // ── Test 9: Non-invariant nodes ────────────────────────────────────

    #[test]
    fn test_loop_invariant_none() {
        let mut scg = SCG::new();

        let entry = add_ctrl(&mut scg, ControlKind::FunctionEntry, "entry");
        let header = add_ctrl(&mut scg, ControlKind::LoopHeader, "header");
        let body1 = add_comp(&mut scg, "b1");
        let body2 = add_comp(&mut scg, "b2");
        let exit = add_ctrl(&mut scg, ControlKind::LoopExit, "exit");

        scg.add_edge(entry, header, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(header, body1, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(header, exit, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(body1, body2, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(body2, header, EdgeKind::ControlFlow).unwrap(); // back-edge
                                                                     // body2 depends on body1 (inside the loop) via DataFlow.
        scg.add_edge(body1, body2, EdgeKind::DataFlow).unwrap();

        let loops = LoopDetector::detect_natural_loops(&scg);
        assert!(!loops.is_empty());

        let invariant = LoopDetector::loop_invariant_nodes(&loops[0], &scg);
        // body1 has no data-flow predecessors inside the loop, so it IS invariant.
        assert!(invariant.contains(&body1), "body1 should be loop-invariant");
        // body2 depends on body1 (inside loop), so not invariant.
        assert!(
            !invariant.contains(&body2),
            "body2 should NOT be loop-invariant (depends on body1 via DataFlow)"
        );
    }

    // ── Test 10: NaturalLoop struct helpers ─────────────────────────────

    #[test]
    fn test_natural_loop_contains() {
        let mut loop_ = NaturalLoop::new(NodeId::new(1), NodeId::new(5));
        loop_.body.insert(NodeId::new(1));
        loop_.body.insert(NodeId::new(2));
        loop_.body.insert(NodeId::new(5));
        assert!(loop_.contains(&NodeId::new(1)));
        assert!(loop_.contains(&NodeId::new(5)));
        assert!(!loop_.contains(&NodeId::new(99)));
        assert_eq!(loop_.body_size(), 3);
        assert!(!loop_.has_exit());
        loop_.exits.insert(NodeId::new(1));
        assert!(loop_.has_exit());
    }

    // ── Test 11: LoopNestingTree defaults ──────────────────────────────

    #[test]
    fn test_loop_nesting_tree_default() {
        let tree = LoopNestingTree::default();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);
    }

    // ── Test 12: DataFlow edges do not affect loop detection ───────────

    #[test]
    fn test_dataflow_edges_dont_create_false_loops() {
        let mut scg = SCG::new();

        // Build a DAG with DataFlow edges but no back-edges.
        let a = add_comp(&mut scg, "a");
        let b = add_comp(&mut scg, "b");
        let c = add_comp(&mut scg, "c");

        scg.add_edge(a, b, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(b, c, EdgeKind::ControlFlow).unwrap();
        // DataFlow from c back to a — this should NOT create a loop
        // because it's not a ControlFlow edge.
        scg.add_edge(c, a, EdgeKind::DataFlow).unwrap();

        let loops = LoopDetector::detect_natural_loops(&scg);
        assert!(
            loops.is_empty(),
            "DataFlow back-edge should not create a natural loop"
        );
    }

    // ── Test 13: LoopNestingTree children helper ───────────────────────

    #[test]
    fn test_nesting_tree_children() {
        let mut scg = SCG::new();

        let entry = add_ctrl(&mut scg, ControlKind::FunctionEntry, "entry");
        let outer_h = add_ctrl(&mut scg, ControlKind::LoopHeader, "outer");
        let inner_h = add_ctrl(&mut scg, ControlKind::LoopHeader, "inner");
        let inner_body = add_comp(&mut scg, "ib");
        let outer_body = add_comp(&mut scg, "ob");
        let exit = add_ctrl(&mut scg, ControlKind::LoopExit, "exit");

        scg.add_edge(entry, outer_h, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(outer_h, inner_h, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(outer_h, exit, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(inner_h, inner_body, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(inner_h, outer_body, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(inner_body, inner_h, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(outer_body, outer_h, EdgeKind::ControlFlow)
            .unwrap();

        let tree = LoopDetector::detect_loop_nesting(&scg);
        let outer_idx = tree.loops.iter().position(|l| l.header == outer_h).unwrap();

        let children = tree.children(outer_idx);
        assert_eq!(children.len(), 1, "outer loop should have one child");
    }
}
