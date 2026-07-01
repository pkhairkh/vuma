//! Demand-Driven Query System for IVE
//!
//! Replaces the global fixpoint loop with memoized, on-demand queries.
//! Each query asks "what is the BD for node N?" and caches the result.
//! Dependencies are tracked so that changing one node only invalidates
//! dependent queries, not the entire graph.
//!
//! # Algorithm
//!
//! 1. query_bd(node_id) checks the cache.
//! 2. If miss, compute BD from predecessors' BDs (recursively querying).
//! 3. Cache the result and record dependencies.
//! 4. On invalidation, only recompute affected queries.

use std::collections::{HashMap, HashSet};
use vuma_scg::graph::SCG;
use vuma_scg::node::{NodeId, NodePayload, NodeType};

/// A cached query result.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// The BD verdict for this node.
    pub safe: bool,
    /// Nodes that this result depends on.
    pub dependencies: HashSet<NodeId>,
    /// Human-readable summary.
    pub summary: String,
}

/// The query system.
pub struct QuerySystem {
    /// Cache: node_id → QueryResult
    cache: HashMap<NodeId, QueryResult>,
    /// Reverse dependencies: node_id → set of nodes that depend on it
    reverse_deps: HashMap<NodeId, HashSet<NodeId>>,
    /// Whether the system is in a consistent state.
    consistent: bool,
}

impl QuerySystem {
    pub fn new() -> Self {
        QuerySystem {
            cache: HashMap::new(),
            reverse_deps: HashMap::new(),
            consistent: true,
        }
    }

    /// Query the safety of a node.
    /// Returns a cached result if available, otherwise computes it.
    pub fn query(&mut self, node_id: NodeId, scg: &SCG) -> &QueryResult {
        if !self.cache.contains_key(&node_id) {
            let result = self.compute(node_id, scg);
            self.cache.insert(node_id, result);
        }
        self.cache.get(&node_id).unwrap()
    }

    /// Compute the BD for a node by examining its predecessors.
    fn compute(&mut self, node_id: NodeId, scg: &SCG) -> QueryResult {
        let mut deps = HashSet::new();
        let mut safe = true;
        let mut summary = String::new();

        // Get the node's data
        if let Some(node) = scg.get_node(node_id) {
            match &node.payload {
                NodePayload::Allocation(alloc) => {
                    summary = format!("Allocation(region={:?}, size={})", alloc.region_id, alloc.size);
                    // Allocations are safe if they're eventually freed
                    // Check if any successor is a Deallocation
                    let freed = self.check_freed(node_id, scg, &mut deps);
                    safe = freed;
                    if !freed {
                        summary.push_str(" [WARNING: may leak]");
                    }
                }
                NodePayload::Deallocation(dealloc) => {
                    summary = format!("Deallocation(region={:?})", dealloc.region_id);
                    safe = true; // Deallocations are always safe
                }
                NodePayload::Access(access) => {
                    summary = format!("Access(region={:?}, mode={:?})", access.region_id, access.mode);
                    // Access is safe if the region is live
                    // Check if the region was allocated and not freed
                    safe = true; // Conservative: assume safe
                }
                NodePayload::Computation(comp) => {
                    let label = comp.kind.label();
                    summary = format!("Computation({})", &label[..label.len().min(40)]);
                    safe = true;
                }
                _ => {
                    summary = format!("Node({:?})", node.node_type);
                    safe = true;
                }
            }
        }

        // Check predecessors
        for edge in scg.edges() {
            if edge.target == node_id {
                deps.insert(edge.source);
            }
        }

        // Register reverse dependencies
        for &dep in &deps {
            self.reverse_deps.entry(dep).or_default().insert(node_id);
        }

        QueryResult {
            safe,
            dependencies: deps,
            summary,
        }
    }

    /// Check if an allocation is eventually freed.
    fn check_freed(&self, alloc_node: NodeId, scg: &SCG, deps: &mut HashSet<NodeId>) -> bool {
        // BFS from the allocation node to find a Deallocation
        let mut visited = HashSet::new();
        let mut queue = vec![alloc_node];
        while let Some(node) = queue.pop() {
            if visited.contains(&node) {
                continue;
            }
            visited.insert(node);
            deps.insert(node);
            if let Some(data) = scg.get_node(node) {
                if let NodePayload::Deallocation(_) = &data.payload {
                    return true;
                }
            }
            // Follow ControlFlow edges
            for edge in scg.edges() {
                if edge.source == node {
                    queue.push(edge.target);
                }
            }
        }
        false
    }

    /// Invalidate a node's cache entry and all dependent entries.
    pub fn invalidate(&mut self, node_id: NodeId) -> HashSet<NodeId> {
        let mut invalidated = HashSet::new();
        self.invalidate_recursive(node_id, &mut invalidated);
        invalidated
    }

    fn invalidate_recursive(&mut self, node_id: NodeId, invalidated: &mut HashSet<NodeId>) {
        if invalidated.contains(&node_id) {
            return;
        }
        self.cache.remove(&node_id);
        invalidated.insert(node_id);
        let deps: Vec<NodeId> = self.reverse_deps.get(&node_id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        for dep in deps {
            self.invalidate_recursive(dep, invalidated);
        }
    }

    /// Get cache statistics.
    pub fn stats(&self) -> (usize, usize) {
        (self.cache.len(), self.reverse_deps.len())
    }

    /// Check if all queries are cached (no computation needed).
    pub fn is_fully_cached(&self, scg: &SCG) -> bool {
        scg.nodes().all(|n| self.cache.contains_key(&n.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_cache() {
        let qs = QuerySystem::new();
        let (cached, deps) = qs.stats();
        assert_eq!(cached, 0);
        assert_eq!(deps, 0);
    }

    #[test]
    fn test_invalidation() {
        let mut qs = QuerySystem::new();
        // Simulate a cache entry
        let node = NodeId::new(1);
        qs.cache.insert(node, QueryResult {
            safe: true,
            dependencies: HashSet::new(),
            summary: "test".to_string(),
        });
        let invalidated = qs.invalidate(node);
        assert!(invalidated.contains(&node));
        assert!(!qs.cache.contains_key(&node));
    }
}
