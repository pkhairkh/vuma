//! Hand-written directed graph (`DiGraph<N, E>`) with linked-list adjacency.
//!
//! This module replaces `petgraph::graph::DiGraph` as the backing store for the
//! SCG (see wave 39 of the VUMA remediation plan). The design mirrors the
//! VUMA-native `womb/graph/digraph.vuma` in spirit: each node owns a pair of
//! adjacency lists (outgoing / incoming edge indices), and edges are stored in
//! a side table so that `EdgeIndex` is a stable, opaque handle.
//!
//! Storage layout
//! --------------
//! * `nodes: Vec<Option<NodeEntry<N>>>` — node weights plus per-node adjacency.
//!   `Option` is used so that removing a node leaves a tombstone slot; this
//!   keeps every existing `NodeIndex` valid for the lifetime of the graph
//!   (the SCG layer caches `NodeIndex` ↔ `NodeId` mappings, so index stability
//!   means we don't have to rebuild those maps on every mutation).
//! * `edges: Vec<Option<EdgeEntry<E>>>` — edge weights plus endpoints. Same
//!   tombstone strategy for `EdgeIndex` stability.
//! * `num_nodes` / `num_edges` — live counts (excluding tombstones).
//!
//! The three graph algorithms (`toposort`, `tarjan_scc`, `has_path_connecting`)
//! are also implemented here so that `scg::graph` no longer reaches into
//! `petgraph::algo`.

use std::collections::VecDeque;

// ── Index types ─────────────────────────────────────────────────────────────

/// Opaque handle for a node in a [`DiGraph`].
///
/// Wraps a `usize` slot index. Stable across removals: a `NodeIndex` returned
/// by [`DiGraph::add_node`] stays valid (and keeps pointing at the same node)
/// until that node is removed via [`DiGraph::remove_node`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeIndex(pub usize);

impl NodeIndex {
    /// Return the raw slot index.
    #[inline]
    pub fn index(self) -> usize {
        self.0
    }
}

impl From<usize> for NodeIndex {
    fn from(i: usize) -> Self {
        NodeIndex(i)
    }
}

/// Opaque handle for an edge in a [`DiGraph`].
///
/// Wraps a `usize` slot index. Stable across removals (see [`NodeIndex`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EdgeIndex(pub usize);

impl EdgeIndex {
    /// Return the raw slot index.
    #[inline]
    pub fn index(self) -> usize {
        self.0
    }
}

impl From<usize> for EdgeIndex {
    fn from(i: usize) -> Self {
        EdgeIndex(i)
    }
}

// ── Direction ───────────────────────────────────────────────────────────────

/// Direction of edge traversal.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Edges leaving the node (`source → target`).
    Outgoing,
    /// Edges entering the node (`target ← source`).
    Incoming,
}

// ── Internal entries ────────────────────────────────────────────────────────

/// A node entry: weight + per-node adjacency lists (edge slot indices).
#[derive(Debug, Clone)]
struct NodeEntry<N> {
    weight: N,
    /// Outgoing edge indices: `self → other`.
    adj_out: Vec<usize>,
    /// Incoming edge indices: `other → self`.
    adj_in: Vec<usize>,
}

/// An edge entry: weight + endpoints.
#[derive(Debug, Clone)]
struct EdgeEntry<E> {
    weight: E,
    source: usize,
    target: usize,
}

// ── EdgeReference ───────────────────────────────────────────────────────────

/// A borrowed view of an edge, returned by [`DiGraph::edges_directed`] and
/// [`DiGraph::edges`]. Mirrors the surface of `petgraph::EdgeRef` so callers
/// can use `.id()`, `.source()`, `.target()`, `.weight()`.
#[derive(Copy, Clone, Debug)]
pub struct EdgeReference<'a, E> {
    id: EdgeIndex,
    source: NodeIndex,
    target: NodeIndex,
    weight: &'a E,
}

impl<'a, E> EdgeReference<'a, E> {
    /// The edge's index.
    #[inline]
    pub fn id(&self) -> EdgeIndex {
        self.id
    }
    /// The source node index.
    #[inline]
    pub fn source(&self) -> NodeIndex {
        self.source
    }
    /// The target node index.
    #[inline]
    pub fn target(&self) -> NodeIndex {
        self.target
    }
    /// The edge weight.
    #[inline]
    pub fn weight(&self) -> &'a E {
        self.weight
    }
}

// ── DiGraph ─────────────────────────────────────────────────────────────────

/// A directed graph with linked-list adjacency, generic over node weight `N`
/// and edge weight `E`.
///
/// Designed as a drop-in replacement for `petgraph::graph::DiGraph` for the
/// SCG's storage needs. Indices are stable across removals (tombstone slots),
/// which lets the SCG layer keep its `NodeIndex`/`EdgeIndex` ↔ `NodeId`/`EdgeId`
/// hash maps consistent without rebuilding them on every mutation.
#[derive(Debug, Clone)]
pub struct DiGraph<N, E> {
    nodes: Vec<Option<NodeEntry<N>>>,
    edges: Vec<Option<EdgeEntry<E>>>,
    num_nodes: usize,
    num_edges: usize,
}

impl<N, E> DiGraph<N, E> {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            num_nodes: 0,
            num_edges: 0,
        }
    }

    /// Create an empty graph with reserved capacity.
    pub fn with_capacity(nodes: usize, edges: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(nodes),
            edges: Vec::with_capacity(edges),
            num_nodes: 0,
            num_edges: 0,
        }
    }

    // ── Node operations ──────────────────────────────────────────────────

    /// Add a node with the given weight. Returns its index.
    pub fn add_node(&mut self, weight: N) -> NodeIndex {
        let idx = self.nodes.len();
        self.nodes.push(Some(NodeEntry {
            weight,
            adj_out: Vec::new(),
            adj_in: Vec::new(),
        }));
        self.num_nodes += 1;
        NodeIndex(idx)
    }

    /// Remove a node and return its weight.
    ///
    /// All edges connected to the node (incoming and outgoing) are also
    /// removed. Returns `None` if the index is out of bounds or already
    /// vacated. Other `NodeIndex`es remain valid (slot becomes a tombstone).
    pub fn remove_node(&mut self, n: NodeIndex) -> Option<N> {
        let entry = self.nodes.get_mut(n.0)?.take()?;
        // Collect connected edges (clone indices to avoid borrow issues).
        let connected: Vec<usize> = entry
            .adj_out
            .iter()
            .chain(entry.adj_in.iter())
            .copied()
            .collect();
        for eidx in connected {
            self.remove_edge_inner(eidx);
        }
        self.num_nodes -= 1;
        Some(entry.weight)
    }

    /// Borrow the weight at the given node index.
    pub fn node_weight(&self, n: NodeIndex) -> Option<&N> {
        self.nodes.get(n.0)?.as_ref().map(|e| &e.weight)
    }

    /// Mutably borrow the weight at the given node index.
    pub fn node_weight_mut(&mut self, n: NodeIndex) -> Option<&mut N> {
        self.nodes.get_mut(n.0)?.as_mut().map(|e| &mut e.weight)
    }

    /// Number of live nodes.
    pub fn node_count(&self) -> usize {
        self.num_nodes
    }

    /// Iterate over all live node indices (skipping tombstones), in ascending
    /// slot order.
    pub fn node_indices(&self) -> impl Iterator<Item = NodeIndex> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|_| NodeIndex(i)))
    }

    /// Iterate over all live node weights.
    pub fn node_weights(&self) -> impl Iterator<Item = &N> {
        self.nodes.iter().filter_map(|slot| slot.as_ref().map(|e| &e.weight))
    }

    /// Iterate over all live node weights, mutably.
    pub fn node_weights_mut(&mut self) -> impl Iterator<Item = &mut N> {
        self.nodes
            .iter_mut()
            .filter_map(|slot| slot.as_mut().map(|e| &mut e.weight))
    }

    // ── Edge operations ──────────────────────────────────────────────────

    /// Add an edge `a → b` with the given weight. Returns its index.
    ///
    /// Both endpoints must already exist in the graph (otherwise this panics;
    /// the SCG layer validates endpoints before calling).
    pub fn add_edge(&mut self, a: NodeIndex, b: NodeIndex, weight: E) -> EdgeIndex {
        debug_assert!(
            self.nodes.get(a.0).map_or(false, |s| s.is_some()),
            "add_edge: source node {a:?} does not exist"
        );
        debug_assert!(
            self.nodes.get(b.0).map_or(false, |s| s.is_some()),
            "add_edge: target node {b:?} does not exist"
        );
        let eidx = self.edges.len();
        self.edges.push(Some(EdgeEntry {
            weight,
            source: a.0,
            target: b.0,
        }));
        if let Some(slot) = self.nodes.get_mut(a.0) {
            if let Some(entry) = slot.as_mut() {
                entry.adj_out.push(eidx);
            }
        }
        if let Some(slot) = self.nodes.get_mut(b.0) {
            if let Some(entry) = slot.as_mut() {
                entry.adj_in.push(eidx);
            }
        }
        self.num_edges += 1;
        EdgeIndex(eidx)
    }

    /// Remove an edge by index and return its weight.
    ///
    /// Returns `None` if the index is out of bounds or already vacated. Other
    /// `EdgeIndex`es remain valid (slot becomes a tombstone).
    pub fn remove_edge(&mut self, e: EdgeIndex) -> Option<E> {
        self.remove_edge_inner(e.0)
    }

    fn remove_edge_inner(&mut self, eidx: usize) -> Option<E> {
        let entry = self.edges.get_mut(eidx)?.take()?;
        // Detach from source's out-list.
        if let Some(Some(src)) = self.nodes.get_mut(entry.source) {
            src.adj_out.retain(|&x| x != eidx);
        }
        // Detach from target's in-list.
        if let Some(Some(dst)) = self.nodes.get_mut(entry.target) {
            dst.adj_in.retain(|&x| x != eidx);
        }
        self.num_edges -= 1;
        Some(entry.weight)
    }

    /// Borrow the weight at the given edge index.
    pub fn edge_weight(&self, e: EdgeIndex) -> Option<&E> {
        self.edges.get(e.0)?.as_ref().map(|en| &en.weight)
    }

    /// Mutably borrow the weight at the given edge index.
    pub fn edge_weight_mut(&mut self, e: EdgeIndex) -> Option<&mut E> {
        self.edges.get_mut(e.0)?.as_mut().map(|en| &mut en.weight)
    }

    /// Number of live edges.
    pub fn edge_count(&self) -> usize {
        self.num_edges
    }

    /// Iterate over all live edge indices (skipping tombstones), in ascending
    /// slot order.
    pub fn edge_indices(&self) -> impl Iterator<Item = EdgeIndex> + '_ {
        self.edges
            .iter()
            .enumerate()
            .filter_map(|(i, slot)| slot.as_ref().map(|_| EdgeIndex(i)))
    }

    /// Iterate over all live edge weights.
    pub fn edge_weights(&self) -> impl Iterator<Item = &E> {
        self.edges.iter().filter_map(|slot| slot.as_ref().map(|e| &e.weight))
    }

    /// Iterate over all live edge weights, mutably.
    pub fn edge_weights_mut(&mut self) -> impl Iterator<Item = &mut E> {
        self.edges
            .iter_mut()
            .filter_map(|slot| slot.as_mut().map(|e| &mut e.weight))
    }

    // ── Adjacency / traversal ────────────────────────────────────────────

    /// Iterate over the neighbours of `n` along outgoing edges.
    pub fn neighbors(&self, n: NodeIndex) -> impl Iterator<Item = NodeIndex> + '_ {
        self.neighbors_directed(n, Direction::Outgoing)
    }

    /// Iterate over the neighbours of `n` along edges in the given direction.
    ///
    /// * `Outgoing` → successors (nodes that `n` points to).
    /// * `Incoming` → predecessors (nodes that point to `n`).
    pub fn neighbors_directed(
        &self,
        n: NodeIndex,
        dir: Direction,
    ) -> impl Iterator<Item = NodeIndex> + '_ {
        let slot = self.nodes.get(n.0);
        slot.into_iter()
            .flatten()
            .flat_map(move |entry| {
                let list: &[usize] = match dir {
                    Direction::Outgoing => &entry.adj_out,
                    Direction::Incoming => &entry.adj_in,
                };
                list.iter().copied().collect::<Vec<_>>().into_iter()
            })
            .filter_map(move |eidx| {
                self.edges
                    .get(eidx)
                    .and_then(|s| s.as_ref())
                    .map(|en| match dir {
                        Direction::Outgoing => NodeIndex(en.target),
                        Direction::Incoming => NodeIndex(en.source),
                    })
            })
    }

    /// Iterate over edge references for the outgoing edges of `n`.
    pub fn edges(&self, n: NodeIndex) -> impl Iterator<Item = EdgeReference<'_, E>> + '_ {
        self.edges_directed(n, Direction::Outgoing)
    }

    /// Iterate over edge references incident to `n` in the given direction.
    pub fn edges_directed(
        &self,
        n: NodeIndex,
        dir: Direction,
    ) -> impl Iterator<Item = EdgeReference<'_, E>> + '_ {
        let slot = self.nodes.get(n.0);
        slot.into_iter()
            .flatten()
            .flat_map(move |entry| {
                let list: &[usize] = match dir {
                    Direction::Outgoing => &entry.adj_out,
                    Direction::Incoming => &entry.adj_in,
                };
                list.iter().copied().collect::<Vec<_>>().into_iter()
            })
            .filter_map(move |eidx| {
                self.edges.get(eidx).and_then(|s| s.as_ref()).map(|en| {
                    EdgeReference {
                        id: EdgeIndex(eidx),
                        source: NodeIndex(en.source),
                        target: NodeIndex(en.target),
                        weight: &en.weight,
                    }
                })
            })
    }

    /// Returns `true` if there is at least one edge `a → b`.
    pub fn contains_edge(&self, a: NodeIndex, b: NodeIndex) -> bool {
        self.find_edge(a, b).is_some()
    }

    /// Returns the index of any edge `a → b`, if one exists.
    ///
    /// For multi-graphs (multiple edges between the same pair), this returns
    /// the first one encountered.
    pub fn find_edge(&self, a: NodeIndex, b: NodeIndex) -> Option<EdgeIndex> {
        let entry = self.nodes.get(a.0)?.as_ref()?;
        for &eidx in &entry.adj_out {
            if let Some(en) = self.edges.get(eidx).and_then(|s| s.as_ref()) {
                if en.target == b.0 {
                    return Some(EdgeIndex(eidx));
                }
            }
        }
        None
    }

    // ── Bulk ─────────────────────────────────────────────────────────────

    /// Remove all nodes and edges, leaving the graph empty.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.edges.clear();
        self.num_nodes = 0;
        self.num_edges = 0;
    }

    /// Reserve capacity for at least `additional` more nodes.
    pub fn reserve_nodes(&mut self, additional: usize) {
        self.nodes.reserve(additional);
    }

    /// Reserve capacity for at least `additional` more edges.
    pub fn reserve_edges(&mut self, additional: usize) {
        self.edges.reserve(additional);
    }
}

impl<N, E> Default for DiGraph<N, E> {
    fn default() -> Self {
        Self::new()
    }
}

// ── Algorithms ──────────────────────────────────────────────────────────────

/// Topologically sort the nodes of a directed graph using Kahn's algorithm.
///
/// Returns `Ok(ordering)` if the graph is acyclic, where `ordering` is a
/// `Vec<NodeIndex>` such that every edge `a → b` has `a` appearing before `b`.
/// Returns `Err(cycle_node)` if the graph contains a cycle; `cycle_node` is
/// one of the nodes still in a cycle when the algorithm stalls.
///
/// Matches the surface of `petgraph::algo::toposort` (minus the optional
/// visited-set argument, which the SCG never uses).
///
/// Scheduling note: zero-in-degree nodes are kept in a **LIFO** stack (rather
/// than a FIFO queue) and seeded in ascending slot order. This makes the
/// algorithm behave like a DFS postorder traversal — once a chain is started,
/// it is followed to completion before another zero-in-degree node is
/// explored. This matches the order `petgraph::algo::toposort` happens to
/// produce (DFS-based reverse postorder), which downstream callers
/// (e.g. `region::lifetimes_overlap`) implicitly depend on for deterministic
/// "fully-process-one-chain-before-starting-the-next" ordering.
pub fn toposort<N, E>(g: &DiGraph<N, E>) -> Result<Vec<NodeIndex>, NodeIndex> {
    let n = g.nodes.len();
    // Live in-degree per slot (0 for tombstones; they're never visited).
    let in_degree: Vec<usize> = (0..n)
        .map(|i| {
            g.nodes
                .get(i)
                .and_then(|s| s.as_ref())
                .map_or(0, |e| e.adj_in.len())
        })
        .collect();

    // Seed the stack with all live zero-in-degree nodes. We push them in
    // ascending slot order and then `pop()` from the end, so the smallest
    // slot is explored first.
    let mut stack: Vec<NodeIndex> = (0..n)
        .filter(|i| {
            g.nodes.get(*i).map_or(false, |s| s.is_some()) && in_degree[*i] == 0
        })
        .map(NodeIndex)
        .collect();

    let mut in_degree = in_degree;
    let mut result: Vec<NodeIndex> = Vec::with_capacity(g.num_nodes);

    while let Some(u) = stack.pop() {
        result.push(u);
        if let Some(Some(entry)) = g.nodes.get(u.0) {
            // Snapshot the out-list to avoid borrow conflicts.
            let out: Vec<usize> = entry.adj_out.iter().copied().collect();
            for eidx in out {
                if let Some(Some(en)) = g.edges.get(eidx) {
                    let v = en.target;
                    if in_degree[v] > 0 {
                        in_degree[v] -= 1;
                        if in_degree[v] == 0 {
                            // LIFO: newly-ready nodes are explored next
                            // (DFS-like), preserving chain locality.
                            stack.push(NodeIndex(v));
                        }
                    }
                }
            }
        }
    }

    if result.len() == g.num_nodes {
        Ok(result)
    } else {
        // Return any node still in a cycle (in_degree > 0 and live).
        let leftover = (0..n)
            .find(|i| {
                g.nodes.get(*i).map_or(false, |s| s.is_some()) && in_degree[*i] > 0
            })
            .map(NodeIndex)
            .unwrap_or(NodeIndex(0));
        Err(leftover)
    }
}

/// Compute strongly connected components using Tarjan's algorithm
/// (recursive formulation, mirroring the pattern in
/// `src/ive/src/liveness.rs:723-817`).
///
/// Returns `Vec<Vec<NodeIndex>>` where each inner vector is one SCC. SCCs are
/// returned in reverse topological order (children before parents), matching
/// `petgraph::algo::tarjan_scc`.
pub fn tarjan_scc<N, E>(g: &DiGraph<N, E>) -> Vec<Vec<NodeIndex>> {
    let n = g.nodes.len();
    let mut index_counter: u32 = 0;
    let mut stack: Vec<NodeIndex> = Vec::new();
    let mut on_stack: Vec<bool> = vec![false; n];
    let mut indices: Vec<Option<u32>> = vec![None; n];
    let mut lowlinks: Vec<u32> = vec![0; n];
    let mut sccs: Vec<Vec<NodeIndex>> = Vec::new();

    for start in g.node_indices() {
        if indices[start.0].is_none() {
            tarjan_strongconnect(
                start,
                g,
                &mut index_counter,
                &mut stack,
                &mut on_stack,
                &mut indices,
                &mut lowlinks,
                &mut sccs,
            );
        }
    }

    sccs
}

#[allow(clippy::too_many_arguments)]
fn tarjan_strongconnect<N, E>(
    v: NodeIndex,
    g: &DiGraph<N, E>,
    index_counter: &mut u32,
    stack: &mut Vec<NodeIndex>,
    on_stack: &mut Vec<bool>,
    indices: &mut Vec<Option<u32>>,
    lowlinks: &mut Vec<u32>,
    sccs: &mut Vec<Vec<NodeIndex>>,
) {
    indices[v.0] = Some(*index_counter);
    lowlinks[v.0] = *index_counter;
    *index_counter += 1;
    stack.push(v);
    on_stack[v.0] = true;

    if let Some(Some(entry)) = g.nodes.get(v.0) {
        let out: Vec<usize> = entry.adj_out.iter().copied().collect();
        for eidx in out {
            let w = match g.edges.get(eidx).and_then(|s| s.as_ref()) {
                Some(en) => NodeIndex(en.target),
                None => continue,
            };
            if indices[w.0].is_none() {
                tarjan_strongconnect(
                    w, g, index_counter, stack, on_stack, indices, lowlinks, sccs,
                );
                lowlinks[v.0] = lowlinks[v.0].min(lowlinks[w.0]);
            } else if on_stack[w.0] {
                lowlinks[v.0] = lowlinks[v.0].min(indices[w.0].unwrap());
            }
        }
    }

    if lowlinks[v.0] == indices[v.0].unwrap() {
        let mut component: Vec<NodeIndex> = Vec::new();
        loop {
            let w = stack.pop().expect("tarjan: stack non-empty");
            on_stack[w.0] = false;
            component.push(w);
            if w == v {
                break;
            }
        }
        sccs.push(component);
    }
}

/// Returns `true` if there is a directed path from `from` to `to` in `g`.
///
/// Uses BFS. `from == to` is treated as a trivial path (returns `true`) as
/// long as the node exists.
pub fn has_path_connecting<N, E>(
    g: &DiGraph<N, E>,
    from: NodeIndex,
    to: NodeIndex,
) -> bool {
    if g.nodes.get(from.0).map_or(false, |s| s.is_none())
        || g.nodes.get(to.0).map_or(false, |s| s.is_none())
    {
        return false;
    }
    if from == to {
        return true;
    }
    let n = g.nodes.len();
    let mut visited: Vec<bool> = vec![false; n];
    let mut queue: VecDeque<NodeIndex> = VecDeque::new();
    visited[from.0] = true;
    queue.push_back(from);

    while let Some(u) = queue.pop_front() {
        if let Some(Some(entry)) = g.nodes.get(u.0) {
            let out: Vec<usize> = entry.adj_out.iter().copied().collect();
            for eidx in out {
                let w = match g.edges.get(eidx).and_then(|s| s.as_ref()) {
                    Some(en) => NodeIndex(en.target),
                    None => continue,
                };
                if w == to {
                    return true;
                }
                if !visited[w.0] {
                    visited[w.0] = true;
                    queue.push_back(w);
                }
            }
        }
    }

    false
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn build_chain() -> DiGraph<&'static str, &'static str> {
        // 0 → 1 → 2
        let mut g = DiGraph::new();
        let a = g.add_node("a");
        let b = g.add_node("b");
        let c = g.add_node("c");
        g.add_edge(a, b, "ab");
        g.add_edge(b, c, "bc");
        g
    }

    #[test]
    fn add_and_get_node() {
        let g = build_chain();
        assert_eq!(g.node_count(), 3);
        assert_eq!(g.edge_count(), 2);
        assert_eq!(g.node_weight(NodeIndex(0)), Some(&"a"));
        assert_eq!(g.node_weight(NodeIndex(1)), Some(&"b"));
        assert_eq!(g.node_weight(NodeIndex(2)), Some(&"c"));
        assert_eq!(g.node_weight(NodeIndex(99)), None);
    }

    #[test]
    fn add_and_get_edge() {
        let g = build_chain();
        assert_eq!(g.edge_weight(EdgeIndex(0)), Some(&"ab"));
        assert_eq!(g.edge_weight(EdgeIndex(1)), Some(&"bc"));
        assert_eq!(g.edge_weight(EdgeIndex(99)), None);
    }

    #[test]
    fn remove_node_cascades_edges() {
        // Removing the middle node should remove both incident edges.
        let mut g = build_chain();
        let removed = g.remove_node(NodeIndex(1));
        assert_eq!(removed, Some("b"));
        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 0);
        // Indices stay stable: nodes 0 and 2 are still accessible.
        assert_eq!(g.node_weight(NodeIndex(0)), Some(&"a"));
        assert_eq!(g.node_weight(NodeIndex(2)), Some(&"c"));
        assert_eq!(g.node_weight(NodeIndex(1)), None);
        // Tombstone is skipped by node_indices().
        let live: Vec<usize> = g.node_indices().map(|n| n.0).collect();
        assert_eq!(live, vec![0, 2]);
    }

    #[test]
    fn remove_edge_keeps_indices_stable() {
        let mut g = build_chain();
        let removed = g.remove_edge(EdgeIndex(0));
        assert_eq!(removed, Some("ab"));
        assert_eq!(g.edge_count(), 1);
        // The other edge keeps its index.
        assert_eq!(g.edge_weight(EdgeIndex(1)), Some(&"bc"));
        assert_eq!(g.edge_weight(EdgeIndex(0)), None);
        // Tombstone is skipped by edge_indices().
        let live: Vec<usize> = g.edge_indices().map(|e| e.0).collect();
        assert_eq!(live, vec![1]);
        // Adjacency was updated.
        let succs: Vec<usize> = g.neighbors(NodeIndex(0)).map(|n| n.0).collect();
        assert!(succs.is_empty());
        let preds: Vec<usize> = g
            .neighbors_directed(NodeIndex(1), Direction::Incoming)
            .map(|n| n.0)
            .collect();
        assert!(preds.is_empty());
    }

    #[test]
    fn neighbors_directed_both_directions() {
        let g = build_chain();
        let succs: Vec<usize> = g.neighbors(NodeIndex(1)).map(|n| n.0).collect();
        assert_eq!(succs, vec![2]);
        let preds: Vec<usize> = g
            .neighbors_directed(NodeIndex(1), Direction::Incoming)
            .map(|n| n.0)
            .collect();
        assert_eq!(preds, vec![0]);
    }

    #[test]
    fn edges_directed_yields_references() {
        let g = build_chain();
        let out: Vec<(usize, usize, &str)> = g
            .edges_directed(NodeIndex(0), Direction::Outgoing)
            .map(|e| (e.id().0, e.target().0, *e.weight()))
            .collect();
        assert_eq!(out, vec![(0, 1, "ab")]);

        let in_refs: Vec<(usize, usize, &str)> = g
            .edges_directed(NodeIndex(2), Direction::Incoming)
            .map(|e| (e.id().0, e.source().0, *e.weight()))
            .collect();
        assert_eq!(in_refs, vec![(1, 1, "bc")]);
    }

    #[test]
    fn contains_edge_and_find_edge() {
        let g = build_chain();
        assert!(g.contains_edge(NodeIndex(0), NodeIndex(1)));
        assert!(!g.contains_edge(NodeIndex(0), NodeIndex(2)));
        assert_eq!(
            g.find_edge(NodeIndex(1), NodeIndex(2)),
            Some(EdgeIndex(1))
        );
        assert_eq!(g.find_edge(NodeIndex(2), NodeIndex(0)), None);
    }

    #[test]
    fn clear_empties_graph() {
        let mut g = build_chain();
        g.clear();
        assert_eq!(g.node_count(), 0);
        assert_eq!(g.edge_count(), 0);
        assert_eq!(g.node_indices().count(), 0);
        assert_eq!(g.edge_indices().count(), 0);
    }

    #[test]
    fn node_and_edge_weights_iterate_live_only() {
        let mut g = build_chain();
        // Tombstone one node (cascades its edge) and one edge.
        g.remove_node(NodeIndex(0)); // removes edge 0 (a→b)
        g.remove_edge(EdgeIndex(1)); // removes b→c
        let nodes: Vec<&str> = g.node_weights().copied().collect();
        assert_eq!(nodes, vec!["b", "c"]);
        let edges: Vec<&str> = g.edge_weights().copied().collect();
        assert!(edges.is_empty());
    }

    // ── toposort ─────────────────────────────────────────────────────────

    #[test]
    fn toposort_acyclic() {
        // 0 → 1 → 2, plus 0 → 2.
        let mut g = DiGraph::new();
        let a = g.add_node("a");
        let b = g.add_node("b");
        let c = g.add_node("c");
        g.add_edge(a, b, ());
        g.add_edge(b, c, ());
        g.add_edge(a, c, ());

        let order = toposort(&g).expect("acyclic");
        assert_eq!(order.len(), 3);
        let pos = |n: NodeIndex| order.iter().position(|&x| x == n).unwrap();
        assert!(pos(a) < pos(b));
        assert!(pos(b) < pos(c));
        assert!(pos(a) < pos(c));
    }

    #[test]
    fn toposort_cycle_returns_err() {
        let mut g = DiGraph::new();
        let a = g.add_node("a");
        let b = g.add_node("b");
        g.add_edge(a, b, ());
        g.add_edge(b, a, ());
        let res = toposort(&g);
        assert!(res.is_err());
    }

    #[test]
    fn toposort_handles_tombstones() {
        // Build a chain, then remove the middle node. The remaining graph
        // (two disconnected nodes) should still toposort cleanly.
        let mut g = build_chain();
        g.remove_node(NodeIndex(1));
        let order = toposort(&g).expect("acyclic after removal");
        let live: Vec<usize> = order.iter().map(|n| n.0).collect();
        assert_eq!(live.len(), 2);
        assert!(live.contains(&0) && live.contains(&2));
    }

    // ── tarjan_scc ───────────────────────────────────────────────────────

    #[test]
    fn tarjan_scc_known_partition() {
        // Graph:
        //   0 → 1 → 2 → 0   (cycle: {0,1,2})
        //   2 → 3            (3 is its own SCC)
        //   3 → 4            (4 is its own SCC)
        let mut g = DiGraph::new();
        let n0 = g.add_node(0);
        let n1 = g.add_node(1);
        let n2 = g.add_node(2);
        let n3 = g.add_node(3);
        let n4 = g.add_node(4);
        g.add_edge(n0, n1, ());
        g.add_edge(n1, n2, ());
        g.add_edge(n2, n0, ());
        g.add_edge(n2, n3, ());
        g.add_edge(n3, n4, ());

        let mut sccs = tarjan_scc(&g);
        // Each SCC is a Vec<NodeIndex>; canonicalise by sorting members.
        for scc in sccs.iter_mut() {
            scc.sort();
        }
        sccs.sort();

        // Find the 3-node cycle SCC.
        let big: Vec<usize> = sccs
            .iter()
            .find(|s| s.len() == 3)
            .expect("one 3-node SCC")
            .iter()
            .map(|n| n.0)
            .collect();
        assert_eq!(big, vec![0, 1, 2]);

        // Singleton SCCs for 3 and 4.
        let singletons: Vec<usize> = sccs
            .iter()
            .filter(|s| s.len() == 1)
            .map(|s| s[0].0)
            .collect();
        assert_eq!(singletons, vec![3, 4]);

        // Total SCC count.
        assert_eq!(sccs.len(), 3);
    }

    #[test]
    fn tarjan_scc_acyclic_returns_singletons() {
        let g = build_chain();
        let sccs = tarjan_scc(&g);
        assert_eq!(sccs.len(), 3);
        for scc in &sccs {
            assert_eq!(scc.len(), 1);
        }
    }

    // ── has_path_connecting ─────────────────────────────────────────────

    #[test]
    fn has_path_connecting_true_and_false() {
        let g = build_chain();
        assert!(has_path_connecting(&g, NodeIndex(0), NodeIndex(2)));
        assert!(has_path_connecting(&g, NodeIndex(0), NodeIndex(1)));
        assert!(!has_path_connecting(&g, NodeIndex(2), NodeIndex(0)));
        assert!(!has_path_connecting(&g, NodeIndex(1), NodeIndex(0)));
        // Trivial path.
        assert!(has_path_connecting(&g, NodeIndex(1), NodeIndex(1)));
    }

    #[test]
    fn has_path_connecting_after_removal() {
        // Remove the bridging edge 1→2; now 0 cannot reach 2.
        let mut g = build_chain();
        g.remove_edge(EdgeIndex(1));
        assert!(!has_path_connecting(&g, NodeIndex(0), NodeIndex(2)));
        assert!(has_path_connecting(&g, NodeIndex(0), NodeIndex(1)));
    }

    // ── Stress: larger acyclic toposort ─────────────────────────────────

    #[test]
    fn toposort_diamond() {
        // 0 → 1, 0 → 2, 1 → 3, 2 → 3
        let mut g: DiGraph<(), ()> = DiGraph::new();
        let n = (0..4).map(|_| g.add_node(())).collect::<Vec<_>>();
        g.add_edge(n[0], n[1], ());
        g.add_edge(n[0], n[2], ());
        g.add_edge(n[1], n[3], ());
        g.add_edge(n[2], n[3], ());
        let order = toposort(&g).expect("diamond is acyclic");
        let pos = |i| order.iter().position(|&x| x == n[i]).unwrap();
        assert!(pos(0) < pos(1));
        assert!(pos(0) < pos(2));
        assert!(pos(1) < pos(3));
        assert!(pos(2) < pos(3));
    }
}
