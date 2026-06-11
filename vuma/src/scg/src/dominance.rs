//! Dominance and Post-Dominance Analysis on SCG
//!
//! This module implements dominance analysis for the Semantic Computation Graph,
//! providing the core data structures and algorithms needed for control-flow
//! reasoning in the IVE (Invariant Verification Engine).
//!
//! # Key Concepts
//!
//! - **Dominator**: Node `a` dominates node `b` if every path from the entry to `b`
//!   passes through `a`. By convention, every node dominates itself.
//! - **Immediate Dominator (idom)**: The unique dominator of `b` that is closest
//!   to `b` in the dominator tree (all other dominators of `b` dominate `idom(b)`).
//! - **Post-Dominator**: Same concept, but on the reverse CFG with an exit node.
//!   Node `a` post-dominates `b` if every path from `b` to exit passes through `a`.
//! - **Dominance Frontier**: The set of nodes where a node's dominance "just ends" —
//!   `DF(a) = { b | a dominates a predecessor of b, but a does not strictly dominate b }`.
//! - **Nearest Common Dominator (NCD)**: The lowest common ancestor in the dominator tree.
//!
//! # IVE Applications
//!
//! - Determining if cleanup code always executes (post-dominance of the cleanup block)
//! - Checking if a write always precedes a read (dominance of write over read)
//! - Placing phi-functions in SSA conversion (dominance frontiers)
//! - Identifying loop headers and back-edges (dominance relationships)
//!
//! # Algorithm
//!
//! Dominators are computed using the Lengauer-Tarjan algorithm, which runs in
//! near-linear time O(E α(V, E)), where α is the inverse Ackermann function.

use hashbrown::{HashMap, HashSet};

use crate::graph::SCG;
use crate::node::NodeId;

// ─── Dominator Tree ──────────────────────────────────────────────────────────

/// The dominator tree resulting from a dominance analysis.
///
/// Each node (except the entry) has exactly one immediate dominator (idom),
/// forming a tree rooted at the entry node. From this tree, all dominance
/// relationships can be derived: `a` dominates `b` iff `a` is an ancestor
/// of `b` in the dominator tree.
#[derive(Debug, Clone)]
pub struct DominatorTree {
    /// The entry (root) node of the dominator tree.
    entry: NodeId,
    /// Map from each node to its immediate dominator.
    /// The entry node has no immediate dominator and is absent from this map.
    idom: HashMap<NodeId, NodeId>,
    /// Pre-computed depth of each node in the dominator tree (entry has depth 0).
    /// Used for efficient NCD and dominance queries.
    depth: HashMap<NodeId, u32>,
    /// All nodes that participated in the analysis (reachable from entry).
    nodes: HashSet<NodeId>,
}

impl DominatorTree {
    /// Returns the entry (root) node of this dominator tree.
    pub fn entry(&self) -> NodeId {
        self.entry
    }

    /// Returns the immediate dominator of `node`, or `None` if `node` is the entry
    /// or is not in the dominator tree.
    pub fn idom(&self, node: NodeId) -> Option<NodeId> {
        self.idom.get(&node).copied()
    }

    /// Returns an iterator over all nodes in this dominator tree.
    pub fn nodes(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.iter().copied()
    }

    /// Returns the number of nodes in this dominator tree.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` if the dominator tree contains no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Returns the depth of `node` in the dominator tree.
    /// The entry node has depth 0; its children have depth 1, etc.
    /// Returns `None` if `node` is not in the tree.
    pub fn depth(&self, node: NodeId) -> Option<u32> {
        self.depth.get(&node).copied()
    }

    /// Returns the children of `node` in the dominator tree.
    pub fn children(&self, node: NodeId) -> Vec<NodeId> {
        self.idom
            .iter()
            .filter_map(|(&child, &parent)| if parent == node { Some(child) } else { None })
            .collect()
    }
}

// ─── Dominance Queries ───────────────────────────────────────────────────────

/// Checks whether node `a` dominates node `b` in the given dominator tree.
///
/// `a` dominates `b` iff `a` is an ancestor of `b` (or equal to `b`) in the
/// dominator tree. This is computed by walking up the idom chain from `b`.
///
/// # Complexity
///
/// O(depth of dominator tree) in the worst case. For repeated queries,
/// consider pre-computing ancestor ranges with DFS timestamps.
///
/// # Examples
///
/// ```
/// use vuma_scg::{SCG, NodeType, NodePayload, ComputationNode, ProgramPoint, EdgeKind};
/// use vuma_scg::dominance::{compute_dominators, dominates};
///
/// let mut scg = SCG::new();
/// let pp = ProgramPoint { file: None, line: None, column: None, offset: None };
/// let n0 = scg.add_node(NodeType::Control, NodePayload::Phantom(
///     vuma_scg::PhantomNode { purpose: "entry".into() }), pp.clone());
/// let n1 = scg.add_node(NodeType::Computation, NodePayload::Computation(
///     ComputationNode { operation: "a".into(), result_type: None, tail_call: false }), pp.clone());
/// let n2 = scg.add_node(NodeType::Computation, NodePayload::Computation(
///     ComputationNode { operation: "b".into(), result_type: None, tail_call: false }), pp);
/// scg.add_edge(n0, n1, EdgeKind::ControlFlow).unwrap();
/// scg.add_edge(n1, n2, EdgeKind::ControlFlow).unwrap();
///
/// let dom_tree = compute_dominators(&scg, n0);
/// assert!(dominates(&dom_tree, n0, n2));
/// assert!(!dominates(&dom_tree, n2, n0));
/// ```
pub fn dominates(dom_tree: &DominatorTree, a: NodeId, b: NodeId) -> bool {
    if a == b {
        return true;
    }
    if !dom_tree.nodes.contains(&a) || !dom_tree.nodes.contains(&b) {
        return false;
    }
    // Walk up from b to see if we hit a
    let mut current = b;
    while let Some(parent) = dom_tree.idom(current) {
        if parent == a {
            return true;
        }
        current = parent;
    }
    false
}

/// Checks whether node `a` **strictly** dominates node `b`.
///
/// Strict dominance means `a` dominates `b` and `a != b`.
pub fn strictly_dominates(dom_tree: &DominatorTree, a: NodeId, b: NodeId) -> bool {
    a != b && dominates(dom_tree, a, b)
}

// ─── Lengauer-Tarjan Algorithm ───────────────────────────────────────────────

/// Internal state for the Lengauer-Tarjan algorithm.
///
/// The algorithm assigns DFS numbers to all reachable nodes, then processes
/// them in reverse DFS order to compute semi-dominators and immediate dominators.
struct LengauerTarjan {
    /// DFS number assigned to each node (1-based).
    dfs_num: HashMap<NodeId, u32>,
    /// Node at each DFS position (1-based indexing).
    vertex: Vec<NodeId>,
    /// Parent in the DFS tree.
    parent: HashMap<NodeId, NodeId>,
    /// Semi-dominator: the DFS number of the semi-dominator.
    semi: HashMap<NodeId, u32>,
    /// The label (representative) of each node in the union-find forest.
    label: HashMap<NodeId, NodeId>,
    /// Ancestor in the union-find forest (for path compression).
    ancestor: HashMap<NodeId, Option<NodeId>>,
    /// Buckets: nodes whose semi-dominator is the given DFS number.
    bucket: HashMap<u32, Vec<NodeId>>,
    /// Immediate dominator result.
    idom: HashMap<NodeId, NodeId>,
    /// Number of reachable nodes.
    n: u32,
}

impl LengauerTarjan {
    fn new() -> Self {
        Self {
            dfs_num: HashMap::new(),
            vertex: vec![NodeId::new(0)], // index 0 unused; 1-based
            parent: HashMap::new(),
            semi: HashMap::new(),
            label: HashMap::new(),
            ancestor: HashMap::new(),
            bucket: HashMap::new(),
            idom: HashMap::new(),
            n: 0,
        }
    }

    /// Step 1: Perform DFS from `entry`, numbering all reachable nodes.
    fn dfs(&mut self, scg: &SCG, entry: NodeId) {
        // Iterative DFS to avoid stack overflow on deep graphs
        let mut stack: Vec<(NodeId, Option<NodeId>)> = vec![(entry, None)];
        let mut visited: HashSet<NodeId> = HashSet::new();

        while let Some((node, parent)) = stack.pop() {
            if visited.contains(&node) {
                continue;
            }
            visited.insert(node);

            self.n += 1;
            let dfs_number = self.n;
            self.dfs_num.insert(node, dfs_number);
            self.vertex.push(node);
            self.semi.insert(node, dfs_number);
            self.label.insert(node, node);
            self.ancestor.insert(node, None);

            if let Some(p) = parent {
                self.parent.insert(node, p);
            }

            // Push successors in reverse order so they're processed in forward order
            if let Some(succs) = scg.successors(node) {
                for &succ in succs.iter().rev() {
                    if !visited.contains(&succ) {
                        stack.push((succ, Some(node)));
                    }
                }
            }
        }
    }

    /// Compress the path from `v` to the root in the union-find forest,
    /// updating `label` to the node with the smallest semi along the path.
    fn compress(&mut self, v: NodeId) {
        let ancestor_v = match self.ancestor.get(&v).copied().flatten() {
            Some(a) => a,
            None => return,
        };

        // Recursively compress
        let ancestor_of_ancestor = self.ancestor.get(&ancestor_v).copied().flatten();
        if ancestor_of_ancestor.is_some() {
            self.compress(ancestor_v);

            let label_ancestor = self.label[&ancestor_v];
            let label_v = self.label[&v];
            let semi_label_ancestor = self.semi[&label_ancestor];
            let semi_label_v = self.semi[&label_v];

            if semi_label_ancestor < semi_label_v {
                self.label.insert(v, label_ancestor);
            }

            self.ancestor.insert(v, ancestor_of_ancestor);
        }
    }

    /// Evaluate `v`: find the node with smallest semi on the path from `v`
    /// to the root in the union-find forest.
    fn eval(&mut self, v: NodeId) -> NodeId {
        match self.ancestor.get(&v).copied().flatten() {
            None => self.label[&v],
            Some(_) => {
                self.compress(v);
                self.label[&v]
            }
        }
    }

    /// Link `v` as a child of `w` in the union-find forest.
    fn link(&mut self, v: NodeId, w: NodeId) {
        self.ancestor.insert(v, Some(w));
    }

    /// Run the full Lengauer-Tarjan algorithm.
    fn run(&mut self, scg: &SCG, entry: NodeId) -> DominatorTree {
        // Step 1: DFS
        self.dfs(scg, entry);

        // Steps 2-3: Process nodes in reverse DFS order
        for i in (2..=self.n).rev() {
            let w = self.vertex[i as usize];

            // Step 2: Compute semi-dominator of w
            if let Some(preds) = scg.predecessors(w) {
                for v in preds {
                    if !self.dfs_num.contains_key(&v) {
                        // v is not reachable from entry; skip
                        continue;
                    }
                    let u = self.eval(v);
                    let semi_u = self.semi[&u];
                    let semi_w = self.semi[&w];
                    if semi_u < semi_w {
                        self.semi.insert(w, semi_u);
                    }
                }
            }

            // Add w to bucket of its semi-dominator
            let semi_w = self.semi[&w];
            self.bucket.entry(semi_w).or_default().push(w);

            // Link w's parent into the forest
            if let Some(&parent) = self.parent.get(&w) {
                self.link(w, parent);
            }

            // Step 3: Process bucket of w's parent
            if let Some(&parent) = self.parent.get(&w) {
                let dfs_parent = self.dfs_num[&parent];
                // Take the bucket out to avoid borrow conflicts with self.eval()
                let bucket_items = self.bucket.remove(&dfs_parent).unwrap_or_default();
                for v in bucket_items {
                    let u = self.eval(v);
                    let semi_u = self.semi[&u];
                    let semi_v = self.semi[&v];
                    if semi_u < semi_v {
                        self.idom.insert(v, u);
                    } else {
                        self.idom.insert(v, parent);
                    }
                }
            }
        }

        // Step 4: Finalize idom — process in forward DFS order
        for i in 2..=self.n {
            let w = self.vertex[i as usize];
            if let Some(&idom_w) = self.idom.get(&w) {
                let semi_w = self.semi[&w];
                let dfs_idom = self.dfs_num.get(&idom_w).copied();
                if dfs_idom != Some(semi_w) {
                    if let Some(&final_idom) = self.idom.get(&idom_w) {
                        self.idom.insert(w, final_idom);
                    }
                }
            }
        }

        // Build the DominatorTree
        let mut nodes: HashSet<NodeId> = HashSet::new();
        nodes.insert(entry);
        for i in 1..=self.n {
            nodes.insert(self.vertex[i as usize]);
        }

        // Compute depths for each node
        let mut depth: HashMap<NodeId, u32> = HashMap::new();
        depth.insert(entry, 0);
        for i in 2..=self.n {
            let w = self.vertex[i as usize];
            if let Some(&idom_w) = self.idom.get(&w) {
                let d = depth.get(&idom_w).copied().unwrap_or(0) + 1;
                depth.insert(w, d);
            }
        }

        DominatorTree {
            entry,
            idom: self.idom.clone(),
            depth,
            nodes,
        }
    }
}

// ─── Public API ──────────────────────────────────────────────────────────────

/// Computes the dominator tree for the SCG starting from the given `entry` node.
///
/// Uses the Lengauer-Tarjan algorithm, which runs in near-linear time.
/// Only nodes reachable from `entry` are included in the dominator tree.
///
/// # Panics
///
/// Does not panic; if `entry` is not in the graph, the result is a dominator
/// tree containing only the entry node with no idom edges.
///
/// # Examples
///
/// ```
/// use vuma_scg::{SCG, NodeType, NodePayload, ComputationNode, ProgramPoint, EdgeKind};
/// use vuma_scg::dominance::compute_dominators;
///
/// let mut scg = SCG::new();
/// let pp = ProgramPoint { file: None, line: None, column: None, offset: None };
/// let entry = scg.add_node(NodeType::Control, NodePayload::Phantom(
///     vuma_scg::PhantomNode { purpose: "entry".into() }), pp.clone());
/// let a = scg.add_node(NodeType::Computation, NodePayload::Computation(
///     ComputationNode { operation: "a".into(), result_type: None, tail_call: false }), pp);
///
/// scg.add_edge(entry, a, EdgeKind::ControlFlow).unwrap();
///
/// let dom_tree = compute_dominators(&scg, entry);
/// assert_eq!(dom_tree.idom(a), Some(entry));
/// assert_eq!(dom_tree.entry(), entry);
/// ```
pub fn compute_dominators(scg: &SCG, entry: NodeId) -> DominatorTree {
    if scg.get_node(entry).is_none() {
        // Entry node doesn't exist in the graph — return empty tree
        return DominatorTree {
            entry,
            idom: HashMap::new(),
            depth: HashMap::new(),
            nodes: HashSet::new(),
        };
    }

    let mut lt = LengauerTarjan::new();
    lt.run(scg, entry)
}

/// Computes the post-dominator tree for the SCG with the given `exit` node.
///
/// Post-dominance is equivalent to dominance on the reverse CFG. We reverse
/// all edges and compute dominance from `exit` as the entry of the reversed graph.
///
/// # How It Works
///
/// Instead of actually building a reversed graph (which would be expensive),
/// we swap the roles of successors and predecessors in the Lengauer-Tarjan
/// traversal. The DFS follows predecessors (reverse edges), and the semi-
/// dominator computation looks at successors (reverse-predecessors).
///
/// # IVE Use Cases
///
/// - Determining if cleanup code always executes (does it post-dominate all
///   paths from a given point?)
/// - Checking if a write always precedes a read along every execution path
pub fn compute_post_dominators(scg: &SCG, exit: NodeId) -> DominatorTree {
    if scg.get_node(exit).is_none() {
        return DominatorTree {
            entry: exit,
            idom: HashMap::new(),
            depth: HashMap::new(),
            nodes: HashSet::new(),
        };
    }

    // Run a modified Lengauer-Tarjan on the reversed CFG.
    // DFS follows predecessors (instead of successors).
    // Semi-dominator computation examines successors (instead of predecessors).
    let mut lt = PostLengauerTarjan::new();
    lt.run(scg, exit)
}

/// Internal state for post-dominance (reversed Lengauer-Tarjan).
///
/// This is structurally identical to `LengauerTarjan` but swaps the
/// roles of successors and predecessors to operate on the reverse CFG.
struct PostLengauerTarjan {
    dfs_num: HashMap<NodeId, u32>,
    vertex: Vec<NodeId>,
    parent: HashMap<NodeId, NodeId>,
    semi: HashMap<NodeId, u32>,
    label: HashMap<NodeId, NodeId>,
    ancestor: HashMap<NodeId, Option<NodeId>>,
    bucket: HashMap<u32, Vec<NodeId>>,
    idom: HashMap<NodeId, NodeId>,
    n: u32,
}

impl PostLengauerTarjan {
    fn new() -> Self {
        Self {
            dfs_num: HashMap::new(),
            vertex: vec![NodeId::new(0)],
            parent: HashMap::new(),
            semi: HashMap::new(),
            label: HashMap::new(),
            ancestor: HashMap::new(),
            bucket: HashMap::new(),
            idom: HashMap::new(),
            n: 0,
        }
    }

    /// DFS on the reversed graph: follow predecessors instead of successors.
    fn dfs(&mut self, scg: &SCG, exit: NodeId) {
        let mut stack: Vec<(NodeId, Option<NodeId>)> = vec![(exit, None)];
        let mut visited: HashSet<NodeId> = HashSet::new();

        while let Some((node, parent)) = stack.pop() {
            if visited.contains(&node) {
                continue;
            }
            visited.insert(node);

            self.n += 1;
            let dfs_number = self.n;
            self.dfs_num.insert(node, dfs_number);
            self.vertex.push(node);
            self.semi.insert(node, dfs_number);
            self.label.insert(node, node);
            self.ancestor.insert(node, None);

            if let Some(p) = parent {
                self.parent.insert(node, p);
            }

            // Follow PREDECESSORS (reverse edges) instead of successors
            if let Some(preds) = scg.predecessors(node) {
                for &pred in preds.iter().rev() {
                    if !visited.contains(&pred) {
                        stack.push((pred, Some(node)));
                    }
                }
            }
        }
    }

    fn compress(&mut self, v: NodeId) {
        let ancestor_v = match self.ancestor.get(&v).copied().flatten() {
            Some(a) => a,
            None => return,
        };

        let ancestor_of_ancestor = self.ancestor.get(&ancestor_v).copied().flatten();
        if ancestor_of_ancestor.is_some() {
            self.compress(ancestor_v);

            let label_ancestor = self.label[&ancestor_v];
            let label_v = self.label[&v];
            let semi_label_ancestor = self.semi[&label_ancestor];
            let semi_label_v = self.semi[&label_v];

            if semi_label_ancestor < semi_label_v {
                self.label.insert(v, label_ancestor);
            }

            self.ancestor.insert(v, ancestor_of_ancestor);
        }
    }

    fn eval(&mut self, v: NodeId) -> NodeId {
        match self.ancestor.get(&v).copied().flatten() {
            None => self.label[&v],
            Some(_) => {
                self.compress(v);
                self.label[&v]
            }
        }
    }

    fn link(&mut self, v: NodeId, w: NodeId) {
        self.ancestor.insert(v, Some(w));
    }

    fn run(&mut self, scg: &SCG, exit: NodeId) -> DominatorTree {
        self.dfs(scg, exit);

        for i in (2..=self.n).rev() {
            let w = self.vertex[i as usize];

            // In the reversed graph, the "predecessors" of w in the original
            // graph are the "successors" in the reversed graph. For computing
            // semi-dominators in the reverse CFG, we look at successors of w
            // in the original graph (which are predecessors in the reversed CFG).
            if let Some(succs) = scg.successors(w) {
                for v in succs {
                    if !self.dfs_num.contains_key(&v) {
                        continue;
                    }
                    let u = self.eval(v);
                    let semi_u = self.semi[&u];
                    let semi_w = self.semi[&w];
                    if semi_u < semi_w {
                        self.semi.insert(w, semi_u);
                    }
                }
            }

            let semi_w = self.semi[&w];
            self.bucket.entry(semi_w).or_default().push(w);

            if let Some(&parent) = self.parent.get(&w) {
                self.link(w, parent);
            }

            if let Some(&parent) = self.parent.get(&w) {
                let dfs_parent = self.dfs_num[&parent];
                // Take the bucket out to avoid borrow conflicts with self.eval()
                let bucket_items = self.bucket.remove(&dfs_parent).unwrap_or_default();
                for v in bucket_items {
                    let u = self.eval(v);
                    let semi_u = self.semi[&u];
                    let semi_v = self.semi[&v];
                    if semi_u < semi_v {
                        self.idom.insert(v, u);
                    } else {
                        self.idom.insert(v, parent);
                    }
                }
            }
        }

        for i in 2..=self.n {
            let w = self.vertex[i as usize];
            if let Some(&idom_w) = self.idom.get(&w) {
                let semi_w = self.semi[&w];
                let dfs_idom = self.dfs_num.get(&idom_w).copied();
                if dfs_idom != Some(semi_w) {
                    if let Some(&final_idom) = self.idom.get(&idom_w) {
                        self.idom.insert(w, final_idom);
                    }
                }
            }
        }

        let mut nodes: HashSet<NodeId> = HashSet::new();
        nodes.insert(exit);
        for i in 1..=self.n {
            nodes.insert(self.vertex[i as usize]);
        }

        let mut depth: HashMap<NodeId, u32> = HashMap::new();
        depth.insert(exit, 0);
        for i in 2..=self.n {
            let w = self.vertex[i as usize];
            if let Some(&idom_w) = self.idom.get(&w) {
                let d = depth.get(&idom_w).copied().unwrap_or(0) + 1;
                depth.insert(w, d);
            }
        }

        DominatorTree {
            entry: exit,
            idom: self.idom.clone(),
            depth,
            nodes,
        }
    }
}

// ─── Dominance Frontier ─────────────────────────────────────────────────────

/// Computes the dominance frontier for each node in the dominator tree.
///
/// The dominance frontier of node `a` is the set of nodes `b` such that:
/// - `a` dominates a predecessor of `b`, but
/// - `a` does not strictly dominate `b`.
///
/// This is the standard definition used in SSA construction and in the
/// IVE for determining where invariants must be checked.
///
/// # Algorithm
///
/// We use the standard iterative algorithm based on the dominator tree:
/// - For each node `b` that has multiple predecessors, walk up the dominator
///   tree from each predecessor until reaching `idom(b)`, adding `b` to the
///   dominance frontier of each visited node.
///
/// # Complexity
///
/// O(N * D) where N is the number of nodes and D is the depth of the dominator
/// tree. In practice, this is very efficient for typical SCG structures.
pub fn find_dominance_frontier(
    scg: &SCG,
    dom_tree: &DominatorTree,
) -> HashMap<NodeId, HashSet<NodeId>> {
    let mut df: HashMap<NodeId, HashSet<NodeId>> = HashMap::new();

    for node in dom_tree.nodes() {
        // A node's dominance frontier can only include join points
        // (nodes with multiple predecessors)
        let preds = match scg.predecessors(node) {
            Some(p) if p.len() >= 2 => p,
            _ => continue,
        };

        for runner in preds {
            // Only consider predecessors that are in the dominator tree
            if !dom_tree.nodes.contains(&runner) {
                continue;
            }

            let mut current = runner;
            loop {
                // Stop if current is the immediate dominator of node
                if dom_tree.idom(node) == Some(current) {
                    break;
                }

                // Add node to the dominance frontier of current
                df.entry(current).or_default().insert(node);

                // Walk up the dominator tree
                match dom_tree.idom(current) {
                    Some(parent) => current = parent,
                    None => break, // Reached the root
                }
            }
        }
    }

    df
}

// ─── Nearest Common Dominator ────────────────────────────────────────────────

/// Finds the nearest common dominator (NCD) of two nodes.
///
/// The NCD is the lowest common ancestor of `a` and `b` in the dominator tree.
/// It is the unique node that dominates both `a` and `b`, and is dominated by
/// every other node that dominates both.
///
/// # Returns
///
/// - `Some(ncd)` if both nodes are in the dominator tree.
/// - `None` if either node is not in the dominator tree.
///
/// # IVE Application
///
/// Used to find the earliest point where two control flow paths merge,
/// which is important for determining the scope of invariants.
pub fn nearest_common_dominator(dom_tree: &DominatorTree, a: NodeId, b: NodeId) -> Option<NodeId> {
    if !dom_tree.nodes.contains(&a) || !dom_tree.nodes.contains(&b) {
        return None;
    }

    if a == b {
        return Some(a);
    }

    // Use depth-based algorithm: bring both nodes to the same depth,
    // then walk up together.
    let mut a_curr = a;
    let mut b_curr = b;

    let a_depth = dom_tree.depth(a)?;
    let b_depth = dom_tree.depth(b)?;

    // Bring the deeper node up to the same depth
    if a_depth > b_depth {
        for _ in 0..(a_depth - b_depth) {
            a_curr = dom_tree.idom(a_curr)?;
        }
    } else if b_depth > a_depth {
        for _ in 0..(b_depth - a_depth) {
            b_curr = dom_tree.idom(b_curr)?;
        }
    }

    // Now both are at the same depth; walk up together
    while a_curr != b_curr {
        a_curr = dom_tree.idom(a_curr)?;
        b_curr = dom_tree.idom(b_curr)?;
    }

    Some(a_curr)
}

// ─── Utility: Dominator Tree Iteration ───────────────────────────────────────

/// Returns a topological ordering of the nodes in the dominator tree
/// (children before parents). Useful for bottom-up traversals.
pub fn dom_tree_postorder(dom_tree: &DominatorTree) -> Vec<NodeId> {
    let mut result = Vec::new();
    let mut stack = vec![(dom_tree.entry(), false)];

    while let Some((node, visited)) = stack.pop() {
        if visited {
            result.push(node);
        } else {
            stack.push((node, true));
            for child in dom_tree.children(node) {
                stack.push((child, false));
            }
        }
    }

    result
}

/// Returns all nodes dominated by `node` (including `node` itself).
///
/// This is the subtree of the dominator tree rooted at `node`.
pub fn dominated_by(dom_tree: &DominatorTree, node: NodeId) -> HashSet<NodeId> {
    if !dom_tree.nodes.contains(&node) {
        return HashSet::new();
    }

    let mut result = HashSet::new();
    let mut stack = vec![node];

    while let Some(n) = stack.pop() {
        if result.insert(n) {
            for child in dom_tree.children(n) {
                stack.push(child);
            }
        }
    }

    result
}

/// Returns the set of all dominators of `node` (including `node` itself).
///
/// This is the set of all ancestors of `node` in the dominator tree,
/// plus `node` itself.
pub fn dominators_of(dom_tree: &DominatorTree, node: NodeId) -> HashSet<NodeId> {
    if !dom_tree.nodes.contains(&node) {
        return HashSet::new();
    }

    let mut result = HashSet::new();
    let mut current = node;
    loop {
        result.insert(current);
        match dom_tree.idom(current) {
            Some(parent) => current = parent,
            None => break,
        }
    }

    result
}

// ─── IVE-Specific Helpers ───────────────────────────────────────────────────

/// Determines if `cleanup_node` always executes after `start_node`.
///
/// This is true iff `cleanup_node` post-dominates `start_node` —
/// every execution path from `start_node` to exit passes through `cleanup_node`.
///
/// This is the primary IVE use case for dominance analysis: verifying
/// that cleanup/deallocation code is guaranteed to execute.
pub fn always_executes_after(
    post_dom_tree: &DominatorTree,
    start_node: NodeId,
    cleanup_node: NodeId,
) -> bool {
    dominates(post_dom_tree, cleanup_node, start_node)
}

/// Determines if `write_node` always precedes `read_node` on every
/// execution path from entry.
///
/// This is true iff `write_node` dominates `read_node` — every path from
/// entry to `read_node` passes through `write_node`.
pub fn write_precedes_read(
    dom_tree: &DominatorTree,
    write_node: NodeId,
    read_node: NodeId,
) -> bool {
    strictly_dominates(dom_tree, write_node, read_node)
}

/// Finds all nodes that are guaranteed to execute on any path from `entry`
/// to `target`. These are exactly the dominators of `target`.
pub fn guaranteed_execution_path(dom_tree: &DominatorTree, target: NodeId) -> Vec<NodeId> {
    if !dom_tree.nodes.contains(&target) {
        return Vec::new();
    }

    let mut path = Vec::new();
    let mut current = target;
    loop {
        path.push(current);
        match dom_tree.idom(current) {
            Some(parent) => current = parent,
            None => break,
        }
    }

    // Reverse so that entry comes first
    path.reverse();
    path
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::EdgeKind;
    use crate::node::{
        ComputationNode, ControlKind, ControlNode, NodePayload, NodeType, PhantomNode, ProgramPoint,
    };

    /// Helper to create a default program point for tests.
    fn pp() -> ProgramPoint {
        ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        }
    }

    /// Helper to add a control node.
    fn add_ctrl(scg: &mut SCG, label: &str, kind: ControlKind) -> NodeId {
        scg.add_node(
            NodeType::Control,
            NodePayload::Control(ControlNode {
                kind,
                label: Some(label.to_string()),
            }),
            pp(),
        )
    }

    /// Helper to add a phantom node.
    fn add_phantom(scg: &mut SCG, purpose: &str) -> NodeId {
        scg.add_node(
            NodeType::Phantom,
            NodePayload::Phantom(PhantomNode {
                purpose: purpose.to_string(),
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

    // ── Test 1: Linear chain ─────────────────────────────────────────────

    #[test]
    fn test_linear_chain() {
        let mut scg = SCG::new();
        let n0 = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let n1 = add_comp(&mut scg, "a");
        let n2 = add_comp(&mut scg, "b");
        let n3 = add_comp(&mut scg, "c");

        scg.add_edge(n0, n1, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(n1, n2, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(n2, n3, EdgeKind::ControlFlow).unwrap();

        let dom_tree = compute_dominators(&scg, n0);

        // n0 dominates everything
        assert!(dominates(&dom_tree, n0, n0));
        assert!(dominates(&dom_tree, n0, n1));
        assert!(dominates(&dom_tree, n0, n2));
        assert!(dominates(&dom_tree, n0, n3));

        // n1 dominates n1, n2, n3 but not n0
        assert!(dominates(&dom_tree, n1, n1));
        assert!(dominates(&dom_tree, n1, n2));
        assert!(dominates(&dom_tree, n1, n3));
        assert!(!dominates(&dom_tree, n1, n0));

        // Immediate dominators form a chain
        assert_eq!(dom_tree.idom(n1), Some(n0));
        assert_eq!(dom_tree.idom(n2), Some(n1));
        assert_eq!(dom_tree.idom(n3), Some(n2));
        assert_eq!(dom_tree.idom(n0), None);
    }

    // ── Test 2: Diamond (if-then-else) ───────────────────────────────────

    #[test]
    fn test_diamond_shape() {
        let mut scg = SCG::new();
        //     entry
        //     /    \
        //   then   else
        //     \    /
        //      join
        let entry = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let then = add_comp(&mut scg, "then");
        let else_ = add_comp(&mut scg, "else");
        let join = add_ctrl(&mut scg, "join", ControlKind::Join);

        scg.add_edge(entry, then, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(entry, else_, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(then, join, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(else_, join, EdgeKind::ControlFlow).unwrap();

        let dom_tree = compute_dominators(&scg, entry);

        // entry dominates everything
        assert!(dominates(&dom_tree, entry, then));
        assert!(dominates(&dom_tree, entry, else_));
        assert!(dominates(&dom_tree, entry, join));

        // then does not dominate else (and vice versa)
        assert!(!dominates(&dom_tree, then, else_));
        assert!(!dominates(&dom_tree, else_, then));

        // then does not dominate join (there's a path through else)
        assert!(!dominates(&dom_tree, then, join));
        assert!(!dominates(&dom_tree, else_, join));

        // entry is the immediate dominator of join
        assert_eq!(dom_tree.idom(join), Some(entry));

        // Strict dominance
        assert!(strictly_dominates(&dom_tree, entry, join));
        assert!(!strictly_dominates(&dom_tree, join, join));
    }

    // ── Test 3: Dominance frontier ───────────────────────────────────────

    #[test]
    fn test_dominance_frontier_diamond() {
        let mut scg = SCG::new();
        let entry = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let then = add_comp(&mut scg, "then");
        let else_ = add_comp(&mut scg, "else");
        let join = add_ctrl(&mut scg, "join", ControlKind::Join);

        scg.add_edge(entry, then, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(entry, else_, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(then, join, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(else_, join, EdgeKind::ControlFlow).unwrap();

        let dom_tree = compute_dominators(&scg, entry);
        let df = find_dominance_frontier(&scg, &dom_tree);

        // entry strictly dominates join, so join is NOT in entry's DF.
        // DF(entry) should be empty (entry dominates all predecessors of every node,
        // and strictly dominates all of them too).
        assert!(df.get(&entry).is_none() || df.get(&entry).map_or(true, |s| s.is_empty()));

        // then's dominance frontier is {join} (then dominates a predecessor of join
        // -- namely itself -- but does not strictly dominate join)
        assert!(df.get(&then).map_or(false, |s| s.contains(&join)));

        // else's dominance frontier is {join}
        assert!(df.get(&else_).map_or(false, |s| s.contains(&join)));

        // join has no dominance frontier (it's a leaf)
        assert!(df.get(&join).is_none() || df.get(&join).map_or(true, |s| s.is_empty()));
    }

    // ── Test 4: Post-dominance ───────────────────────────────────────────

    #[test]
    fn test_post_dominators() {
        let mut scg = SCG::new();
        // entry -> a -> b -> exit
        let entry = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let a = add_comp(&mut scg, "a");
        let b = add_comp(&mut scg, "b");
        let exit = add_ctrl(&mut scg, "exit", ControlKind::FunctionReturn);

        scg.add_edge(entry, a, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(a, b, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(b, exit, EdgeKind::ControlFlow).unwrap();

        let pdom_tree = compute_post_dominators(&scg, exit);

        // exit post-dominates everything
        assert!(dominates(&pdom_tree, exit, entry));
        assert!(dominates(&pdom_tree, exit, a));
        assert!(dominates(&pdom_tree, exit, b));
        assert!(dominates(&pdom_tree, exit, exit));

        // b post-dominates entry and a (every path from them to exit goes through b)
        assert!(dominates(&pdom_tree, b, entry));
        assert!(dominates(&pdom_tree, b, a));

        // entry does NOT post-dominate a or b
        assert!(!dominates(&pdom_tree, entry, a));
        assert!(!dominates(&pdom_tree, entry, b));

        // Immediate post-dominators
        assert_eq!(pdom_tree.idom(b), Some(exit));
        assert_eq!(pdom_tree.idom(a), Some(b));
        assert_eq!(pdom_tree.idom(entry), Some(a));
    }

    // ── Test 5: Post-dominance with diamond ───────────────────────────────

    #[test]
    fn test_post_dominators_diamond() {
        let mut scg = SCG::new();
        //     entry
        //     /    \
        //   then   else
        //     \    /
        //      join
        //       |
        //      exit
        let entry = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let then = add_comp(&mut scg, "then");
        let else_ = add_comp(&mut scg, "else");
        let join = add_ctrl(&mut scg, "join", ControlKind::Join);
        let exit = add_ctrl(&mut scg, "exit", ControlKind::FunctionReturn);

        scg.add_edge(entry, then, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(entry, else_, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(then, join, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(else_, join, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(join, exit, EdgeKind::ControlFlow).unwrap();

        let pdom_tree = compute_post_dominators(&scg, exit);

        // exit post-dominates everything
        assert!(dominates(&pdom_tree, exit, entry));

        // join post-dominates then, else, and entry
        assert!(dominates(&pdom_tree, join, then));
        assert!(dominates(&pdom_tree, join, else_));
        assert!(dominates(&pdom_tree, join, entry));

        // then does NOT post-dominate entry (path through else avoids then)
        assert!(!dominates(&pdom_tree, then, entry));
        assert!(!dominates(&pdom_tree, else_, entry));
    }

    // ── Test 6: Nearest common dominator ──────────────────────────────────

    #[test]
    fn test_nearest_common_dominator() {
        let mut scg = SCG::new();
        //     entry
        //     /    \
        //   then   else
        //     \    /
        //      join
        let entry = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let then = add_comp(&mut scg, "then");
        let else_ = add_comp(&mut scg, "else");
        let join = add_ctrl(&mut scg, "join", ControlKind::Join);

        scg.add_edge(entry, then, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(entry, else_, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(then, join, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(else_, join, EdgeKind::ControlFlow).unwrap();

        let dom_tree = compute_dominators(&scg, entry);

        // NCD(then, else) = entry
        assert_eq!(
            nearest_common_dominator(&dom_tree, then, else_),
            Some(entry)
        );

        // NCD(then, join) = entry
        assert_eq!(nearest_common_dominator(&dom_tree, then, join), Some(entry));

        // NCD(node, node) = node
        assert_eq!(nearest_common_dominator(&dom_tree, then, then), Some(then));

        // NCD with nonexistent node
        assert_eq!(
            nearest_common_dominator(&dom_tree, then, NodeId::new(999)),
            None
        );
    }

    // ── Test 7: IVE helpers ───────────────────────────────────────────────

    #[test]
    fn test_ive_helpers() {
        let mut scg = SCG::new();
        // entry -> write -> read -> exit
        let entry = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let write = add_comp(&mut scg, "write");
        let read = add_comp(&mut scg, "read");
        let exit = add_ctrl(&mut scg, "exit", ControlKind::FunctionReturn);

        scg.add_edge(entry, write, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(write, read, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(read, exit, EdgeKind::ControlFlow).unwrap();

        let dom_tree = compute_dominators(&scg, entry);
        let pdom_tree = compute_post_dominators(&scg, exit);

        // Write always precedes read
        assert!(write_precedes_read(&dom_tree, write, read));
        assert!(!write_precedes_read(&dom_tree, read, write));

        // Exit always executes after entry
        assert!(always_executes_after(&pdom_tree, entry, exit));
        assert!(always_executes_after(&pdom_tree, write, exit));

        // Guaranteed execution path from entry to read
        let path = guaranteed_execution_path(&dom_tree, read);
        assert!(path.contains(&entry));
        assert!(path.contains(&write));
        assert!(path.contains(&read));
        // entry should come before write before read
        let entry_pos = path.iter().position(|&n| n == entry).unwrap();
        let write_pos = path.iter().position(|&n| n == write).unwrap();
        let read_pos = path.iter().position(|&n| n == read).unwrap();
        assert!(entry_pos < write_pos);
        assert!(write_pos < read_pos);
    }

    // ── Test 8: Loop with back-edge ───────────────────────────────────────

    #[test]
    fn test_loop_with_back_edge() {
        let mut scg = SCG::new();
        // entry -> header -> body -> latch --+
        //          ^                        |
        //          +------------------------+
        // header -> exit
        let entry = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let header = add_ctrl(&mut scg, "header", ControlKind::LoopHeader);
        let body = add_comp(&mut scg, "body");
        let latch = add_ctrl(&mut scg, "latch", ControlKind::Jump);
        let exit = add_ctrl(&mut scg, "exit", ControlKind::LoopExit);

        scg.add_edge(entry, header, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(header, body, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(body, latch, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(latch, header, EdgeKind::ControlFlow).unwrap(); // back-edge
        scg.add_edge(header, exit, EdgeKind::ControlFlow).unwrap();

        let dom_tree = compute_dominators(&scg, entry);

        // entry dominates everything
        assert!(dominates(&dom_tree, entry, header));
        assert!(dominates(&dom_tree, entry, body));
        assert!(dominates(&dom_tree, entry, latch));
        assert!(dominates(&dom_tree, entry, exit));

        // header dominates body, latch, and exit
        assert!(dominates(&dom_tree, header, body));
        assert!(dominates(&dom_tree, header, latch));
        assert!(dominates(&dom_tree, header, exit));

        // latch does not dominate header (there's a path from entry to header
        // that doesn't go through latch)
        assert!(!dominates(&dom_tree, latch, header));

        // Immediate dominators
        assert_eq!(dom_tree.idom(header), Some(entry));
        assert_eq!(dom_tree.idom(body), Some(header));
        assert_eq!(dom_tree.idom(latch), Some(body));
        assert_eq!(dom_tree.idom(exit), Some(header));

        // Dominance frontier
        let df = find_dominance_frontier(&scg, &dom_tree);
        // latch's dominance frontier should contain header
        // (header has predecessors: entry and latch; latch is dominated by header
        //  but header's idom is not latch, so header is in latch's DF)
        assert!(df.get(&latch).map_or(false, |s| s.contains(&header)));

        // Dominated-by subtree
        let header_subtree = dominated_by(&dom_tree, header);
        assert!(header_subtree.contains(&header));
        assert!(header_subtree.contains(&body));
        assert!(header_subtree.contains(&latch));
        assert!(header_subtree.contains(&exit));
        assert!(!header_subtree.contains(&entry));
    }

    // ── Test 9: Single node ──────────────────────────────────────────────

    #[test]
    fn test_single_node() {
        let mut scg = SCG::new();
        let entry = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);

        let dom_tree = compute_dominators(&scg, entry);

        assert_eq!(dom_tree.len(), 1);
        assert!(dominates(&dom_tree, entry, entry));
        assert_eq!(dom_tree.idom(entry), None);
        assert_eq!(dom_tree.depth(entry), Some(0));
    }

    // ── Test 10: Nonexistent entry ───────────────────────────────────────

    #[test]
    fn test_nonexistent_entry() {
        let scg = SCG::new();
        let fake = NodeId::new(999);
        let dom_tree = compute_dominators(&scg, fake);
        assert!(dom_tree.is_empty());
    }

    // ── Test 11: Dominated-by and dominators-of ──────────────────────────

    #[test]
    fn test_dominated_by_and_dominators_of() {
        let mut scg = SCG::new();
        let n0 = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let n1 = add_comp(&mut scg, "a");
        let n2 = add_comp(&mut scg, "b");

        scg.add_edge(n0, n1, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(n1, n2, EdgeKind::ControlFlow).unwrap();

        let dom_tree = compute_dominators(&scg, n0);

        // dominators_of(n2) = {n2, n1, n0}
        let doms = dominators_of(&dom_tree, n2);
        assert_eq!(doms.len(), 3);
        assert!(doms.contains(&n0));
        assert!(doms.contains(&n1));
        assert!(doms.contains(&n2));

        // dominated_by(n0) = {n0, n1, n2}
        let sub = dominated_by(&dom_tree, n0);
        assert_eq!(sub.len(), 3);
        assert!(sub.contains(&n0));
        assert!(sub.contains(&n1));
        assert!(sub.contains(&n2));

        // dominated_by(n1) = {n1, n2}
        let sub1 = dominated_by(&dom_tree, n1);
        assert_eq!(sub1.len(), 2);
        assert!(sub1.contains(&n1));
        assert!(sub1.contains(&n2));

        // dominators_of for nonexistent
        let doms_fake = dominators_of(&dom_tree, NodeId::new(999));
        assert!(doms_fake.is_empty());
    }

    // ── Test 12: Post-order traversal of dominator tree ──────────────────

    #[test]
    fn test_dom_tree_postorder() {
        let mut scg = SCG::new();
        let n0 = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let n1 = add_comp(&mut scg, "a");
        let n2 = add_comp(&mut scg, "b");
        let n3 = add_comp(&mut scg, "c");

        scg.add_edge(n0, n1, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(n0, n2, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(n1, n3, EdgeKind::ControlFlow).unwrap();

        let dom_tree = compute_dominators(&scg, n0);
        let po = dom_tree_postorder(&dom_tree);

        // In postorder, entry (root) should be last
        assert_eq!(po.last(), Some(&n0));
        // All nodes should be present
        assert_eq!(po.len(), 4);
    }

    // ── Test 13: Multiple paths with shared prefix ───────────────────────

    #[test]
    fn test_shared_prefix() {
        let mut scg = SCG::new();
        // entry -> a -> b -> c
        //              \-> d
        let entry = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let a = add_comp(&mut scg, "a");
        let b = add_comp(&mut scg, "b");
        let c = add_comp(&mut scg, "c");
        let d = add_comp(&mut scg, "d");

        scg.add_edge(entry, a, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(a, b, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(a, d, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(b, c, EdgeKind::ControlFlow).unwrap();

        let dom_tree = compute_dominators(&scg, entry);

        // a dominates b, c, d
        assert!(dominates(&dom_tree, a, b));
        assert!(dominates(&dom_tree, a, c));
        assert!(dominates(&dom_tree, a, d));

        // b dominates c but not d
        assert!(dominates(&dom_tree, b, c));
        assert!(!dominates(&dom_tree, b, d));

        // NCD(c, d) = a
        assert_eq!(nearest_common_dominator(&dom_tree, c, d), Some(a));

        // NCD(b, d) = a
        assert_eq!(nearest_common_dominator(&dom_tree, b, d), Some(a));
    }

    // ── Test 14: Dominance frontier for linear chain is empty ────────────

    #[test]
    fn test_dominance_frontier_linear() {
        let mut scg = SCG::new();
        let n0 = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let n1 = add_comp(&mut scg, "a");
        let n2 = add_comp(&mut scg, "b");

        scg.add_edge(n0, n1, EdgeKind::ControlFlow).unwrap();
        scg.add_edge(n1, n2, EdgeKind::ControlFlow).unwrap();

        let dom_tree = compute_dominators(&scg, n0);
        let df = find_dominance_frontier(&scg, &dom_tree);

        // Linear chain has no join points => empty dominance frontiers
        for (_, frontier) in &df {
            assert!(frontier.is_empty());
        }
    }

    // ── Test 15: Unreachable nodes are excluded ──────────────────────────

    #[test]
    fn test_unreachable_nodes_excluded() {
        let mut scg = SCG::new();
        let entry = add_ctrl(&mut scg, "entry", ControlKind::FunctionEntry);
        let reachable = add_comp(&mut scg, "reachable");
        let unreachable = add_comp(&mut scg, "unreachable");

        scg.add_edge(entry, reachable, EdgeKind::ControlFlow)
            .unwrap();
        // unreachable has no edges from entry

        let dom_tree = compute_dominators(&scg, entry);

        // Only entry and reachable should be in the dominator tree
        assert!(dom_tree.nodes().any(|n| n == entry));
        assert!(dom_tree.nodes().any(|n| n == reachable));
        assert!(!dom_tree.nodes().any(|n| n == unreachable));

        assert_eq!(dom_tree.len(), 2);
    }
}
