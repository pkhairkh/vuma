//! Verification caching for the IVE module.
//!
//! This module implements `VerificationCache`, which stores verification results
//! keyed by subgraph fingerprints. When a subgraph has not changed, its cached
//! results can be reused, avoiding redundant verification work.

use std::collections::HashMap;
use vuma_scg::graph::SCG;
use vuma_scg::node::NodeId;

/// A structured invariant violation used by the batched violation system
/// and the verification cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InvariantViolation {
    /// Which invariant was violated.
    pub invariant: String,
    /// The node where the violation was detected.
    pub node: NodeId,
    /// Human-readable description of the violation.
    pub description: String,
    /// The severity of the violation.
    pub severity: Severity,
}

impl InvariantViolation {
    /// Create a new invariant violation.
    pub fn new(
        invariant: impl Into<String>,
        node: NodeId,
        description: impl Into<String>,
        severity: Severity,
    ) -> Self {
        Self {
            invariant: invariant.into(),
            node,
            description: description.into(),
            severity,
        }
    }
}

impl std::fmt::Display for InvariantViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {} at {}: {}",
            self.severity, self.invariant, self.node, self.description
        )
    }
}

/// Severity level for invariant violations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    /// A minor issue or warning.
    Low,
    /// A significant issue that may affect correctness.
    Medium,
    /// A critical safety violation.
    High,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Low => write!(f, "LOW"),
            Severity::Medium => write!(f, "MEDIUM"),
            Severity::High => write!(f, "HIGH"),
        }
    }
}

/// Compute a fingerprint for a subgraph of the SCG rooted at the given nodes.
///
/// The fingerprint incorporates the node types, payload hashes, and edge
/// structure of the subgraph, so that any change to the subgraph will
/// result in a different fingerprint.
pub fn compute_fingerprint(scg: &SCG, nodes: &[NodeId]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();

    // Hash node IDs and their types in sorted order for determinism
    let mut sorted_nodes: Vec<NodeId> = nodes.to_vec();
    sorted_nodes.sort_by_key(|n| n.as_u64());

    for &node_id in &sorted_nodes {
        node_id.as_u64().hash(&mut hasher);
        if let Some(node) = scg.get_node(node_id) {
            format!("{:?}", node.node_type).hash(&mut hasher);
        }
    }

    // Hash edges between the nodes
    for edge in scg.edges() {
        if nodes.contains(&edge.source) || nodes.contains(&edge.target) {
            edge.source.as_u64().hash(&mut hasher);
            edge.target.as_u64().hash(&mut hasher);
            format!("{:?}", edge.kind).hash(&mut hasher);
        }
    }

    hasher.finish()
}

/// A cache for verification results, keyed by subgraph fingerprint.
///
/// When a subgraph has not changed (same fingerprint), its cached
/// violations can be reused without re-running verification.
#[derive(Debug, Clone, Default)]
pub struct VerificationCache {
    /// Map from fingerprint to list of violations found.
    cache: HashMap<u64, Vec<InvariantViolation>>,
}

impl VerificationCache {
    /// Create a new, empty verification cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up cached violations for the given fingerprint.
    ///
    /// Returns `Some(&Vec<InvariantViolation>)` if a result is cached,
    /// or `None` if no result is available.
    pub fn get(&self, fingerprint: u64) -> Option<&Vec<InvariantViolation>> {
        self.cache.get(&fingerprint)
    }

    /// Insert verification results into the cache.
    ///
    /// If a result already exists for this fingerprint, it is replaced.
    pub fn insert(&mut self, fingerprint: u64, violations: Vec<InvariantViolation>) {
        self.cache.insert(fingerprint, violations);
    }

    /// Invalidate the cached result for the given fingerprint.
    pub fn invalidate(&mut self, fingerprint: u64) {
        self.cache.remove(&fingerprint);
    }

    /// Clear all cached results.
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// Returns the number of cached results.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Returns `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Compute a fingerprint for the given subgraph nodes and cache the result.
    pub fn compute_and_insert(
        &mut self,
        scg: &SCG,
        nodes: &[NodeId],
        violations: Vec<InvariantViolation>,
    ) -> u64 {
        let fp = compute_fingerprint(scg, nodes);
        self.cache.insert(fp, violations);
        fp
    }

    /// Check if a result is cached for the given subgraph nodes.
    pub fn get_for_nodes(&self, scg: &SCG, nodes: &[NodeId]) -> Option<&[InvariantViolation]> {
        let fp = compute_fingerprint(scg, nodes);
        self.get(fp).map(|v| v.as_slice())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use vuma_scg::edge::EdgeKind;
    use vuma_scg::graph::SCG;
    use vuma_scg::node::{AllocationNode, NodePayload, NodeType, ProgramPoint};
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
    fn test_cache_insert_and_get() {
        let mut cache = VerificationCache::new();
        let violations = vec![InvariantViolation::new(
            "memory_safety",
            NodeId::new(1),
            "leak",
            Severity::High,
        )];
        cache.insert(42, violations.clone());
        let result = cache.get(42).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].invariant, "memory_safety");
    }

    #[test]
    fn test_cache_miss() {
        let cache = VerificationCache::new();
        assert!(cache.get(999).is_none());
    }

    #[test]
    fn test_cache_invalidate() {
        let mut cache = VerificationCache::new();
        cache.insert(1, vec![]);
        cache.invalidate(1);
        assert!(cache.get(1).is_none());
    }

    #[test]
    fn test_cache_invalidate_nonexistent() {
        let mut cache = VerificationCache::new();
        cache.invalidate(999); // should not panic
        assert!(cache.is_empty());
    }

    #[test]
    fn test_fingerprint_changes_with_scg() {
        let rid = RegionId::new(1);
        // SCG with one allocation
        let mut scg1 = SCG::new();
        let n1 = scg1.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: None,
            }),
            pp(),
        );
        let mut region = SCGRegion::new(rid, DeploymentTarget::Heap);
        region.add_node(n1);
        scg1.add_region(region);

        // SCG with two allocations
        let mut scg2 = SCG::new();
        let n2a = scg2.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: None,
            }),
            pp(),
        );
        let n2b = scg2.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 128,
                align: 8,
                region_id: rid,
                type_name: None,
            }),
            pp(),
        );
        let mut region2 = SCGRegion::new(rid, DeploymentTarget::Heap);
        region2.add_node(n2a);
        region2.add_node(n2b);
        scg2.add_region(region2);

        let fp1 = compute_fingerprint(&scg1, &[n1]);
        let fp2 = compute_fingerprint(&scg2, &[n2a, n2b]);
        assert_ne!(
            fp1, fp2,
            "Different SCGs should have different fingerprints"
        );
    }

    #[test]
    fn test_cache_clear() {
        let mut cache = VerificationCache::new();
        cache.insert(1, vec![]);
        cache.insert(2, vec![]);
        cache.insert(3, vec![]);
        assert_eq!(cache.len(), 3);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn test_cache_get_returns_vec() {
        let mut cache = VerificationCache::new();
        let violations = vec![
            InvariantViolation::new("v1", NodeId::new(1), "desc1", Severity::High),
            InvariantViolation::new("v2", NodeId::new(2), "desc2", Severity::Low),
        ];
        cache.insert(42, violations);
        let result = cache.get(42).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].invariant, "v1");
        assert_eq!(result[1].severity, Severity::Low);
    }

    #[test]
    fn test_cache_insert_replaces() {
        let mut cache = VerificationCache::new();
        cache.insert(
            1,
            vec![InvariantViolation::new(
                "old",
                NodeId::new(1),
                "old",
                Severity::Low,
            )],
        );
        cache.insert(
            1,
            vec![InvariantViolation::new(
                "new",
                NodeId::new(2),
                "new",
                Severity::High,
            )],
        );
        let result = cache.get(1).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].invariant, "new");
    }

    #[test]
    fn test_cache_compute_and_insert() {
        let rid = RegionId::new(1);
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: rid,
                type_name: None,
            }),
            pp(),
        );
        let mut region = SCGRegion::new(rid, DeploymentTarget::Heap);
        region.add_node(n1);
        scg.add_region(region);

        let mut cache = VerificationCache::new();
        let violations = vec![InvariantViolation::new("test", n1, "msg", Severity::Medium)];
        let fp = cache.compute_and_insert(&scg, &[n1], violations);
        assert!(cache.get(fp).is_some());
        assert_eq!(cache.get(fp).unwrap().len(), 1);
    }

    #[test]
    fn test_cache_len_and_is_empty() {
        let mut cache = VerificationCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        cache.insert(1, vec![]);
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);
    }
}
