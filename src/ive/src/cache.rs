//! Verification caching for the IVE module.
//!
//! This module implements `VerificationCache`, which stores verification results
//! keyed by subgraph fingerprints. When a subgraph has not changed, its cached
//! results can be reused, avoiding redundant verification work.

use std::collections::HashMap;
use vuma_codegen::scg_to_ir::Scg;
use vuma_scg::edge::EdgeKind;
use vuma_scg::node::{NodeId, NodeType};

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
pub fn compute_fingerprint(scg: &Scg, nodes: &[NodeId]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();

    // Hash node IDs and their types in sorted order for determinism.
    //
    // Task 6-C: this now reads the codegen `Scg`'s `node_type` adapter
    // (Task 6-A) rather than destructuring the semantic `NodeData` struct.
    // The codegen `Scg::edges()` (Task 6-B) yields `CodegenEdge`s whose
    // `source` / `target` / `kind` fields mirror the semantic `EdgeData`,
    // so the edge-hashing loop is unchanged.
    let mut sorted_nodes: Vec<NodeId> = nodes.to_vec();
    sorted_nodes.sort_by_key(|n| n.as_u64());

    for &node_id in &sorted_nodes {
        node_id.as_u64().hash(&mut hasher);
        // `node_type` is a semantic `NodeType` derived from the codegen
        // ScgStatement discriminant by the 6-A adapter.
        let node_type: Option<NodeType> = scg.node_type(node_id);
        if let Some(nt) = node_type {
            format!("{:?}", nt).hash(&mut hasher);
        }
    }

    // Hash edges between the nodes. Each `CodegenEdge.kind` is a semantic
    // `EdgeKind` mirrored by the 6-B edge layer.
    for edge in scg.edges() {
        let kind: &EdgeKind = &edge.kind;
        if nodes.contains(&edge.source) || nodes.contains(&edge.target) {
            edge.source.as_u64().hash(&mut hasher);
            edge.target.as_u64().hash(&mut hasher);
            format!("{:?}", kind).hash(&mut hasher);
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
        scg: &Scg,
        nodes: &[NodeId],
        violations: Vec<InvariantViolation>,
    ) -> u64 {
        let fp = compute_fingerprint(scg, nodes);
        self.cache.insert(fp, violations);
        fp
    }

    /// Check if a result is cached for the given subgraph nodes.
    pub fn get_for_nodes(&self, scg: &Scg, nodes: &[NodeId]) -> Option<&[InvariantViolation]> {
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
    // `Scg`, `NodeId`, `NodeType`, and `EdgeKind` come from `super::*`
    // (top-level imports); only the codegen statement-list builders need
    // their own imports.
    use vuma_codegen::scg_to_ir::{
        AllocationNode, CodegenEdge, NodeLoc, ScgFunction, ScgNode, ScgStatement,
        ScgType,
    };

    /// Build a minimal codegen `Scg` containing `num_allocs` stack
    /// allocations (each a distinct size) followed by a `Return` statement,
    /// with the `node_index` map populated so the codegen `Scg`'s
    /// `node_type` / `get_node` adapters (Task 6-A) resolve correctly.
    ///
    /// Returns the built `Scg` and the `NodeId`s of the allocation
    /// statements (in body order). This is the codegen-Scg analogue of the
    /// old semantic `SCG::add_node(NodeType::Allocation, ...)` fixtures.
    fn build_alloc_scg(num_allocs: usize) -> (Scg, Vec<NodeId>) {
        let mut body: Vec<ScgStatement> = Vec::new();
        for i in 0..num_allocs {
            body.push(ScgStatement::Allocation(AllocationNode::Stack {
                name: format!("buf{}", i),
                size: (64 * (i as u32 + 1)) as u32,
                ty: ScgType::U8,
            }));
        }
        body.push(ScgStatement::Return(vec![]));
        let body_len = body.len();

        let func = ScgFunction {
            name: "test_fn".to_string(),
            params: vec![],
            results: vec![],
            body,
            var_types: std::collections::HashMap::new(),
        };
        let mut scg = Scg::new(vec![ScgNode::Function(func)]);

        // Populate node_index with fresh monotonic NodeIds (mirroring the
        // 4-C contract used by the AST->codegen bridge). Without this,
        // `Scg::get_node` / `Scg::node_type` return `None`.
        let mut alloc_ids: Vec<NodeId> = Vec::new();
        let mut all_ids: Vec<NodeId> = Vec::new();
        for stmt_idx in 0..body_len {
            let id = NodeId::new(stmt_idx as u64);
            if stmt_idx < num_allocs {
                alloc_ids.push(id);
            }
            scg.node_index
                .insert(id, NodeLoc { fn_idx: 0, stmt_idx });
            all_ids.push(id);
        }

        // Populate ControlFlow fall-through edges between consecutive
        // statements (mirrors the codegen bridge's `populate_codegen_edges`
        // post-pass from Task 6-B). This exercises the edge-hashing branch
        // of `compute_fingerprint` against the codegen `CodegenEdge` shape.
        for window in all_ids.windows(2) {
            scg.edges.push(CodegenEdge {
                source: window[0],
                target: window[1],
                kind: EdgeKind::ControlFlow,
                label: None,
            });
        }

        (scg, alloc_ids)
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
        // codegen Scg with one allocation
        let (scg1, ids1) = build_alloc_scg(1);
        // codegen Scg with two allocations
        let (scg2, ids2) = build_alloc_scg(2);

        // Sanity-check the migrated read path: the codegen Scg's 6-A
        // `node_type` adapter resolves allocations to the semantic
        // `NodeType::Allocation`, and the 6-B edge layer is populated.
        assert_eq!(
            scg1.node_type(ids1[0]),
            Some(NodeType::Allocation),
            "6-A node_type adapter should classify allocations"
        );
        assert!(
            scg1.edges().any(|e| e.kind == EdgeKind::ControlFlow),
            "6-B edge layer should contain ControlFlow fall-throughs"
        );

        let fp1 = compute_fingerprint(&scg1, &ids1);
        let fp2 = compute_fingerprint(&scg2, &ids2);
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
        let (scg, ids) = build_alloc_scg(1);
        let n1 = ids[0];

        let mut cache = VerificationCache::new();
        let violations = vec![InvariantViolation::new("test", n1, "msg", Severity::Medium)];
        let fp = cache.compute_and_insert(&scg, &ids, violations);
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
