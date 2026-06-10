//! Cleanup Invariant Verifier for the IVE module.
//!
//! This module implements VUMA's **cleanup invariant**: "Every acquired resource
//! is eventually released." It operates on a simplified control-flow/resource
//! graph derived from the SCG and verifies:
//!
//! 1. **No resource leaks** — every allocation/acquisition reaches a matching
//!    deallocation/release on ALL execution paths.
//! 2. **No double-free** — the same resource is never freed more than once
//!    on any execution path.
//! 3. **No use-after-free** — no access occurs after a deallocation on any
//!    execution path.
//!
//! # Architecture
//!
//! The verifier works on a [`CleanupGraph`] — a directed graph whose nodes
//! represent resource operations (acquire, release, access) and control-flow
//! points (branch, join, return, error). Edges represent possible execution
//! transfer. The verifier performs path-sensitive analysis by enumerating
//! paths through the graph and checking the cleanup invariant on each.
//!
//! # Resource Model
//!
//! Resources include heap allocations, locks, file handles, and any other
//! acquire/release pair. Each resource is identified by a [`ResourceId`]
//! and tracked across all paths through the graph.

use crate::result::{CounterExample, VerificationResult, VerificationStatus};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;

// ---------------------------------------------------------------------------
// Resource identifiers
// ---------------------------------------------------------------------------

/// Unique identifier for a tracked resource (allocation, lock, file handle, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ResourceId(pub u64);

impl fmt::Display for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "res{}", self.0)
    }
}

/// The kind of resource being tracked.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    /// Heap memory allocation.
    Memory,
    /// A mutual-exclusion lock.
    Lock,
    /// An open file handle.
    FileHandle,
    /// A network socket.
    Socket,
    /// Any other resource with acquire/release semantics.
    Custom(String),
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourceKind::Memory => write!(f, "memory"),
            ResourceKind::Lock => write!(f, "lock"),
            ResourceKind::FileHandle => write!(f, "file_handle"),
            ResourceKind::Socket => write!(f, "socket"),
            ResourceKind::Custom(name) => write!(f, "custom({name})"),
        }
    }
}

// ---------------------------------------------------------------------------
// Graph node identifiers and operations
// ---------------------------------------------------------------------------

/// Unique identifier for a node in the [`CleanupGraph`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u64);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "N{}", self.0)
    }
}

/// The kind of operation represented by a node in the cleanup graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OperationKind {
    /// Acquire a resource (allocate, lock, open, …).
    Acquire {
        /// The resource being acquired.
        resource: ResourceId,
        /// The kind of resource.
        kind: ResourceKind,
    },
    /// Release a resource (free, unlock, close, …).
    Release {
        /// The resource being released.
        resource: ResourceId,
        /// The kind of resource.
        kind: ResourceKind,
    },
    /// Access a resource (read/write after acquisition).
    Access {
        /// The resource being accessed.
        resource: ResourceId,
    },
    /// A conditional branch point (e.g. if-else).
    Branch {
        /// A label for the branch condition.
        condition: String,
    },
    /// A join point where branches merge.
    Join,
    /// A normal function return / exit.
    Return,
    /// An early return due to error.
    ErrorReturn {
        /// Optional description of the error.
        description: String,
    },
    /// A no-op / passthrough node (useful for graph construction).
    Passthrough,
}

impl OperationKind {
    /// Returns the resource referenced by this operation, if any.
    pub fn resource(&self) -> Option<ResourceId> {
        match self {
            OperationKind::Acquire { resource, .. } => Some(*resource),
            OperationKind::Release { resource, .. } => Some(*resource),
            OperationKind::Access { resource } => Some(*resource),
            _ => None,
        }
    }

    /// Returns `true` if this is an acquire operation.
    pub fn is_acquire(&self) -> bool {
        matches!(self, OperationKind::Acquire { .. })
    }

    /// Returns `true` if this is a release operation.
    pub fn is_release(&self) -> bool {
        matches!(self, OperationKind::Release { .. })
    }

    /// Returns `true` if this is an access operation.
    pub fn is_access(&self) -> bool {
        matches!(self, OperationKind::Access { .. })
    }
}

/// A node in the cleanup graph.
#[derive(Debug, Clone)]
pub struct CleanupNode {
    /// Unique identifier.
    pub id: NodeId,
    /// The operation this node represents.
    pub operation: OperationKind,
    /// A human-readable label (e.g., source location).
    pub label: String,
}

// ---------------------------------------------------------------------------
// Cleanup Graph
// ---------------------------------------------------------------------------

/// A directed graph representing resource operations and control flow.
///
/// Nodes are [`CleanupNode`]s (acquire, release, access, branch, join, etc.)
/// and edges represent possible execution transfer. The graph is built
/// incrementally and then verified by [`CleanupVerifier`].
#[derive(Debug, Clone, Default)]
pub struct CleanupGraph {
    /// Nodes indexed by [`NodeId`].
    nodes: BTreeMap<NodeId, CleanupNode>,
    /// Adjacency list: node → set of successor node IDs.
    successors: BTreeMap<NodeId, BTreeSet<NodeId>>,
    /// Reverse adjacency list: node → set of predecessor node IDs.
    predecessors: BTreeMap<NodeId, BTreeSet<NodeId>>,
    /// Counter for generating the next `NodeId`.
    next_node_id: u64,
    /// Entry node of the graph (set when verification begins).
    entry: Option<NodeId>,
}

impl CleanupGraph {
    /// Create a new, empty cleanup graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a node to the graph, returning its `NodeId`.
    pub fn add_node(&mut self, operation: OperationKind, label: impl Into<String>) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;
        self.nodes.insert(
            id,
            CleanupNode {
                id,
                operation,
                label: label.into(),
            },
        );
        self.successors.insert(id, BTreeSet::new());
        self.predecessors.insert(id, BTreeSet::new());
        id
    }

    /// Add a directed edge from `source` to `target`.
    ///
    /// Returns `Ok(())` if both nodes exist, or `Err` with a description
    /// if either node is missing.
    pub fn add_edge(&mut self, source: NodeId, target: NodeId) -> Result<(), String> {
        if !self.nodes.contains_key(&source) {
            return Err(format!("source node {source} does not exist"));
        }
        if !self.nodes.contains_key(&target) {
            return Err(format!("target node {target} does not exist"));
        }
        self.successors.get_mut(&source).unwrap().insert(target);
        self.predecessors.get_mut(&target).unwrap().insert(source);
        Ok(())
    }

    /// Set the entry node for verification.
    pub fn set_entry(&mut self, id: NodeId) -> Result<(), String> {
        if !self.nodes.contains_key(&id) {
            return Err(format!("entry node {id} does not exist"));
        }
        self.entry = Some(id);
        Ok(())
    }

    /// Get a reference to a node by ID.
    pub fn get_node(&self, id: NodeId) -> Option<&CleanupNode> {
        self.nodes.get(&id)
    }

    /// Get the successor set of a node.
    pub fn successors_of(&self, id: NodeId) -> Option<&BTreeSet<NodeId>> {
        self.successors.get(&id)
    }

    /// Get the predecessor set of a node.
    pub fn predecessors_of(&self, id: NodeId) -> Option<&BTreeSet<NodeId>> {
        self.predecessors.get(&id)
    }

    /// Return the number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Return an iterator over all node IDs.
    pub fn node_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.nodes.keys().copied()
    }

    /// Return all acquire nodes in the graph.
    pub fn acquire_nodes(&self) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, n)| n.operation.is_acquire())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Return all release nodes in the graph.
    pub fn release_nodes(&self) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, n)| n.operation.is_release())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Return all access nodes in the graph.
    pub fn access_nodes(&self) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, n)| n.operation.is_access())
            .map(|(id, _)| *id)
            .collect()
    }

    /// Find all nodes where a specific resource is acquired.
    pub fn acquire_nodes_for(&self, resource: ResourceId) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, n)| match &n.operation {
                OperationKind::Acquire { resource: r, .. } => *r == resource,
                _ => false,
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Find all nodes where a specific resource is released.
    pub fn release_nodes_for(&self, resource: ResourceId) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, n)| match &n.operation {
                OperationKind::Release { resource: r, .. } => *r == resource,
                _ => false,
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Find all nodes that access a specific resource.
    pub fn access_nodes_for(&self, resource: ResourceId) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, n)| match &n.operation {
                OperationKind::Access { resource: r } => *r == resource,
                _ => false,
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Check whether a path exists from `source` to `target` using BFS.
    pub fn has_path(&self, source: NodeId, target: NodeId) -> bool {
        if source == target {
            return true;
        }
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        visited.insert(source);
        queue.push_back(source);
        while let Some(current) = queue.pop_front() {
            if let Some(succs) = self.successors_of(current) {
                for &succ in succs {
                    if succ == target {
                        return true;
                    }
                    if visited.insert(succ) {
                        queue.push_back(succ);
                    }
                }
            }
        }
        false
    }

    /// Return all terminal nodes (nodes with no successors) — these are
    /// the exit points of the graph (Return, ErrorReturn, or dead ends).
    pub fn terminal_nodes(&self) -> Vec<NodeId> {
        self.nodes
            .keys()
            .filter(|&&id| self.successors.get(&id).map_or(true, |s| s.is_empty()))
            .copied()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Leak annotations
// ---------------------------------------------------------------------------

/// The reason a resource leak is considered intentional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeakReason {
    /// Arena allocator — freed all at once.
    Arena,
    /// Global cache — lives for program duration.
    GlobalCache,
    /// Singleton pattern — one instance, never freed.
    Singleton,
    /// Static storage duration.
    StaticStorage,
    /// Explicitly marked as intentional leak.
    Intentional,
    /// Custom reason.
    Custom(String),
}

impl fmt::Display for LeakReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LeakReason::Arena => write!(f, "arena"),
            LeakReason::GlobalCache => write!(f, "global_cache"),
            LeakReason::Singleton => write!(f, "singleton"),
            LeakReason::StaticStorage => write!(f, "static_storage"),
            LeakReason::Intentional => write!(f, "intentional"),
            LeakReason::Custom(reason) => write!(f, "custom({reason})"),
        }
    }
}

/// An annotation marking a resource leak as intentional.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeakAnnotation {
    /// The resource that is intentionally leaked.
    pub resource: ResourceId,
    /// The reason the leak is intentional.
    pub reason: LeakReason,
    /// Source location or description of where the annotation was placed.
    pub annotation_point: String,
    /// Optional reviewer who approved the annotation.
    pub reviewer: Option<String>,
}

impl fmt::Display for LeakAnnotation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.reviewer {
            Some(reviewer) => write!(
                f,
                "{}: {} (reason: {}, reviewer: {}) at {}",
                self.resource, self.reason, self.reason, reviewer, self.annotation_point
            ),
            None => write!(
                f,
                "{}: {} (reason: {}) at {}",
                self.resource, self.reason, self.reason, self.annotation_point
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Annotated Cleanup Graph
// ---------------------------------------------------------------------------

/// A [`CleanupGraph`] extended with intentional leak annotations.
///
/// Some resources are intentionally never freed (arenas, global caches,
/// singletons, etc.). This wrapper allows such resources to be annotated
/// so the verifier can distinguish intentional leaks from genuine bugs.
#[derive(Debug, Clone)]
pub struct AnnotatedCleanupGraph {
    /// The underlying cleanup graph.
    pub graph: CleanupGraph,
    /// Leak annotations indexed by resource ID.
    leak_annotations: HashMap<ResourceId, LeakAnnotation>,
}

impl AnnotatedCleanupGraph {
    /// Create a new annotated graph wrapping the given cleanup graph.
    pub fn new(graph: CleanupGraph) -> Self {
        Self {
            graph,
            leak_annotations: HashMap::new(),
        }
    }

    /// Add a leak annotation for a resource.
    ///
    /// Returns `Err` if the resource already has an annotation.
    pub fn add_leak_annotation(&mut self, annotation: LeakAnnotation) -> Result<(), String> {
        if self.leak_annotations.contains_key(&annotation.resource) {
            return Err(format!(
                "resource {} already has a leak annotation",
                annotation.resource
            ));
        }
        self.leak_annotations.insert(annotation.resource, annotation);
        Ok(())
    }

    /// Check whether a resource has an intentional leak annotation.
    pub fn is_annotated_leak(&self, resource: ResourceId) -> bool {
        self.leak_annotations.contains_key(&resource)
    }

    /// Get the leak annotation for a resource, if any.
    pub fn get_leak_annotation(&self, resource: ResourceId) -> Option<&LeakAnnotation> {
        self.leak_annotations.get(&resource)
    }

    /// Iterate over all leak annotations.
    pub fn leak_annotations(&self) -> impl Iterator<Item = &LeakAnnotation> {
        self.leak_annotations.values()
    }

    /// Return the number of leak annotations.
    pub fn annotation_count(&self) -> usize {
        self.leak_annotations.len()
    }
}

// ---------------------------------------------------------------------------
// Violation types
// ---------------------------------------------------------------------------

/// The kind of cleanup invariant violation detected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ViolationKind {
    /// A resource was acquired but never released on some path (leak).
    Leak,
    /// A resource was released more than once on some path (double-free).
    DoubleFree,
    /// A resource was accessed after it was released on some path
    /// (use-after-free).
    UseAfterFree,
}

impl fmt::Display for ViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ViolationKind::Leak => write!(f, "resource leak"),
            ViolationKind::DoubleFree => write!(f, "double free"),
            ViolationKind::UseAfterFree => write!(f, "use after free"),
        }
    }
}

/// A single cleanup invariant violation, with trace information.
#[derive(Debug, Clone, PartialEq)]
pub struct CleanupViolation {
    /// What kind of violation.
    pub kind: ViolationKind,
    /// The resource involved.
    pub resource: ResourceId,
    /// The kind of the resource involved.
    pub resource_kind: ResourceKind,
    /// The execution path leading to the violation (sequence of node labels).
    pub path: Vec<String>,
    /// The node at which the violation is detected.
    pub violation_node: NodeId,
    /// Human-readable description.
    pub description: String,
}

impl fmt::Display for CleanupViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let path_str = self.path.join(" → ");
        write!(
            f,
            "{}: {} ({} at {}): {}",
            self.kind, self.resource, self.resource_kind, path_str, self.description
        )
    }
}

// ---------------------------------------------------------------------------
// Path state for DFS traversal
// ---------------------------------------------------------------------------

/// Tracks the state of resources along a single execution path during
/// graph traversal.
#[derive(Debug, Clone, Default)]
struct PathState {
    /// Resources currently acquired (not yet released) on this path.
    /// Maps ResourceId → (AcquireNodeId, ResourceKind).
    live_resources: BTreeMap<ResourceId, (NodeId, ResourceKind)>,
    /// Resources that have been released on this path.
    /// Maps ResourceId → (ReleaseNodeId, ResourceKind).
    released_resources: BTreeMap<ResourceId, (NodeId, ResourceKind)>,
    /// Number of times each resource has been released on this path.
    release_count: BTreeMap<ResourceId, usize>,
    /// The path so far (node labels for diagnostics).
    path_labels: Vec<String>,
    /// The path so far (node IDs for structural reasoning).
    path_nodes: Vec<NodeId>,
}

impl PathState {
    /// Process a node, updating the path state. Returns a list of any
    /// violations detected at this node.
    fn process_node(&mut self, node: &CleanupNode) -> Vec<CleanupViolation> {
        let mut violations = Vec::new();

        match &node.operation {
            OperationKind::Acquire { resource, kind } => {
                // If this resource was already acquired and not yet released,
                // that's not a violation per se (re-acquisition), but we
                // record the new acquisition point.
                self.live_resources
                    .insert(*resource, (node.id, kind.clone()));
                // If it was previously released, re-acquisition is fine
                // (removes it from released set logically).
            }
            OperationKind::Release { resource, kind } => {
                // Check for double-free
                let count = self.release_count.entry(*resource).or_insert(0);
                *count += 1;
                if *count > 1 {
                    violations.push(CleanupViolation {
                        kind: ViolationKind::DoubleFree,
                        resource: *resource,
                        resource_kind: kind.clone(),
                        path: self.path_labels.clone(),
                        violation_node: node.id,
                        description: format!(
                            "{} released {} time(s) on this path",
                            resource, count
                        ),
                    });
                }
                // Move from live to released
                if let Some((_, rk)) = self.live_resources.remove(resource) {
                    self.released_resources
                        .insert(*resource, (node.id, rk));
                }
            }
            OperationKind::Access { resource } => {
                // Check for use-after-free: resource has been released
                if self.released_resources.contains_key(resource) {
                    let kind = self
                        .released_resources
                        .get(resource)
                        .map(|(_, k)| k.clone())
                        .unwrap_or(ResourceKind::Memory);
                    violations.push(CleanupViolation {
                        kind: ViolationKind::UseAfterFree,
                        resource: *resource,
                        resource_kind: kind,
                        path: self.path_labels.clone(),
                        violation_node: node.id,
                        description: format!(
                            "{} accessed after being released on this path",
                            resource
                        ),
                    });
                }
            }
            _ => {}
        }

        self.path_labels.push(node.label.clone());
        self.path_nodes.push(node.id);

        violations
    }

    /// Check for leaks at a terminal node: any live resource that hasn't
    /// been released is a leak.
    fn check_leaks(&self, terminal_node: NodeId) -> Vec<CleanupViolation> {
        let mut violations = Vec::new();
        for (&resource, (_, kind)) in &self.live_resources {
            violations.push(CleanupViolation {
                kind: ViolationKind::Leak,
                resource,
                resource_kind: kind.clone(),
                path: self.path_labels.clone(),
                violation_node: terminal_node,
                description: format!(
                    "{} ({}) acquired but never released on this path",
                    resource, kind
                ),
            });
        }
        violations
    }
}

// ---------------------------------------------------------------------------
// Cleanup Verifier
// ---------------------------------------------------------------------------

/// The cleanup invariant verifier.
///
/// Performs path-sensitive analysis on a [`CleanupGraph`] to detect:
/// - Resource leaks (acquire without matching release on all paths)
/// - Double-free (same resource released more than once on any path)
/// - Use-after-free (access after release on any path)
pub struct CleanupVerifier {
    /// Maximum path length to explore (prevents infinite traversal on cycles).
    max_path_length: usize,
    /// Whether to emit detailed diagnostic logging.
    verbose: bool,
}

/// The result of cleanup verification.
#[derive(Debug, Clone)]
pub struct CleanupReport {
    /// All violations found (excluding annotated intentional leaks).
    pub violations: Vec<CleanupViolation>,
    /// Whether the cleanup invariant holds (no unannotated violations).
    pub clean: bool,
    /// Number of paths explored.
    pub paths_explored: usize,
    /// Number of acquire nodes checked.
    pub acquires_checked: usize,
    /// Intentional leaks that were annotated and thus suppressed from violations.
    pub intentional_leaks: Vec<LeakAnnotation>,
    /// Leaks without annotations (genuine violations).
    pub unannotated_leaks: Vec<CleanupViolation>,
    /// Total number of leak annotations considered.
    pub annotation_count: usize,
}

impl CleanupReport {
    /// Create a report from a list of violations.
    pub fn from_violations(violations: Vec<CleanupViolation>, paths_explored: usize, acquires_checked: usize) -> Self {
        let clean = violations.is_empty();
        let unannotated_leaks = violations.iter()
            .filter(|v| v.kind == ViolationKind::Leak)
            .cloned()
            .collect();
        Self {
            violations,
            clean,
            paths_explored,
            acquires_checked,
            intentional_leaks: Vec::new(),
            unannotated_leaks,
            annotation_count: 0,
        }
    }

    /// Create a report with annotation-aware results.
    pub fn from_annotated(
        violations: Vec<CleanupViolation>,
        intentional_leaks: Vec<LeakAnnotation>,
        unannotated_leaks: Vec<CleanupViolation>,
        annotation_count: usize,
        paths_explored: usize,
        acquires_checked: usize,
    ) -> Self {
        // clean = no violations remain (all leaks are annotated, no other violations)
        let clean = violations.is_empty();
        Self {
            violations,
            clean,
            paths_explored,
            acquires_checked,
            intentional_leaks,
            unannotated_leaks,
            annotation_count,
        }
    }

    /// Convert this report into a [`VerificationResult`] for integration
    /// with the IVE verification engine.
    ///
    /// The result considers annotation awareness:
    /// - If only intentional (annotated) leaks exist → ProbablySafe
    /// - If any unannotated leaks or other violations → Violated
    /// - If completely clean → Proven
    pub fn to_verification_result(&self) -> VerificationResult {
        if self.clean {
            // No violations at all — check if we had intentional leaks
            if self.intentional_leaks.is_empty() {
                VerificationResult::new(
                    "cleanup",
                    VerificationStatus::Proven,
                    format!(
                        "cleanup invariant verified: {} acquire(s) checked across {} path(s)",
                        self.acquires_checked, self.paths_explored
                    ),
                )
            } else {
                // Only intentional leaks — probably safe under the assumption
                // that annotations are correct
                VerificationResult::new(
                    "cleanup",
                    VerificationStatus::ProbablySafe {
                        assumptions: self.intentional_leaks.iter().map(|a| {
                            format!(
                                "resource {} is intentionally leaked (reason: {})",
                                a.resource, a.reason
                            )
                        }).collect(),
                    },
                    format!(
                        "cleanup invariant: {} intentional leak(s) annotated, {} acquire(s) checked across {} path(s)",
                        self.intentional_leaks.len(), self.acquires_checked, self.paths_explored
                    ),
                )
            }
        } else {
            let first = &self.violations[0];
            let path: Vec<String> = first.path.clone();
            VerificationResult::new(
                "cleanup",
                VerificationStatus::Violated {
                    counterexample: CounterExample::new(
                        path,
                        first.violation_node.to_string(),
                        format!("{}", first),
                    ),
                },
                format!(
                    "cleanup invariant violated: {} violation(s) found",
                    self.violations.len()
                ),
            )
        }
    }
}

impl CleanupVerifier {
    /// Create a new verifier with default settings.
    pub fn new() -> Self {
        Self {
            max_path_length: 256,
            verbose: false,
        }
    }

    /// Set the maximum path length for traversal.
    pub fn with_max_path_length(mut self, len: usize) -> Self {
        self.max_path_length = len;
        self
    }

    /// Enable verbose diagnostic output.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Verify the cleanup invariant on the given graph.
    ///
    /// This performs a depth-first traversal of all execution paths from the
    /// entry node (or from all nodes if no entry is set), checking for
    /// leaks, double-frees, and use-after-free violations.
    pub fn verify(&self, graph: &CleanupGraph) -> CleanupReport {
        let mut all_violations: Vec<CleanupViolation> = Vec::new();
        let mut paths_explored = 0usize;

        // Determine starting nodes
        let start_nodes: Vec<NodeId> = if let Some(entry) = graph.entry {
            vec![entry]
        } else {
            // Start from all nodes that have no predecessors (entry points)
            let entries: Vec<NodeId> = graph
                .node_ids()
                .filter(|&id| {
                    graph.predecessors_of(id).map_or(true, |p| p.is_empty())
                })
                .collect();
            if entries.is_empty() && graph.node_count() > 0 {
                // Fallback: start from all nodes
                graph.node_ids().collect()
            } else {
                entries
            }
        };

        if start_nodes.is_empty() {
            return CleanupReport::from_violations(vec![], 0, 0);
        }

        let acquires_checked = graph.acquire_nodes().len();

        // DFS with path state tracking
        for start in &start_nodes {
            let initial_state = PathState::default();
            self.dfs_verify(
                graph,
                *start,
                initial_state,
                &mut all_violations,
                &mut paths_explored,
                &mut BTreeSet::new(),
            );
        }

        // Deduplicate violations (same kind + resource + violation_node)
        let mut seen = BTreeSet::new();
        let violations: Vec<CleanupViolation> = all_violations
            .into_iter()
            .filter(|v| {
                let key = (v.kind.clone(), v.resource, v.violation_node);
                seen.insert(key)
            })
            .collect();

        CleanupReport::from_violations(violations, paths_explored, acquires_checked)
    }

    /// Recursive DFS that explores all paths from `current`, accumulating
    /// violations and tracking resource state.
    fn dfs_verify(
        &self,
        graph: &CleanupGraph,
        current: NodeId,
        mut state: PathState,
        violations: &mut Vec<CleanupViolation>,
        paths_explored: &mut usize,
        visited_on_path: &mut BTreeSet<NodeId>,
    ) {
        // Cycle / path length guard
        if state.path_nodes.len() >= self.max_path_length {
            if self.verbose {
                // Path length limit reached — log if verbose
                let _ = current; // suppress unused warning
            }
            return;
        }

        // Simple cycle detection: if we've visited this node on the
        // current path, skip (prevents infinite loops).
        if visited_on_path.contains(&current) {
            return;
        }
        visited_on_path.insert(current);

        // Process the current node
        if let Some(node) = graph.get_node(current) {
            let node_violations = state.process_node(node);
            violations.extend(node_violations);
        } else {
            visited_on_path.remove(&current);
            return;
        }

        // Get successors
        let succs: Vec<NodeId> = graph
            .successors_of(current)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();

        if succs.is_empty() {
            // Terminal node — check for leaks
            *paths_explored += 1;
            let leak_violations = state.check_leaks(current);
            violations.extend(leak_violations);
        } else {
            // Explore each successor
            for succ in succs {
                self.dfs_verify(
                    graph,
                    succ,
                    state.clone(),
                    violations,
                    paths_explored,
                    visited_on_path,
                );
            }
        }

        visited_on_path.remove(&current);
    }

    /// Quick check: for each acquire node, does a matching release node
    /// exist that is reachable from it? This is a fast O(V+E) per acquire
    /// reachability check, but doesn't account for conditional paths.
    ///
    /// Returns a list of (acquire_node, resource) pairs where no release
    /// is reachable.
    pub fn quick_check_reachability(&self, graph: &CleanupGraph) -> Vec<(NodeId, ResourceId)> {
        let mut unreachable: Vec<(NodeId, ResourceId)> = Vec::new();

        for acquire_id in graph.acquire_nodes() {
            if let Some(node) = graph.get_node(acquire_id) {
                if let OperationKind::Acquire { resource, .. } = &node.operation {
                    // Find all release nodes for this resource
                    let release_ids = graph.release_nodes_for(*resource);
                    let any_reachable = release_ids
                        .iter()
                        .any(|&rid| graph.has_path(acquire_id, rid));

                    if !any_reachable {
                        unreachable.push((acquire_id, *resource));
                    }
                }
            }
        }

        unreachable
    }

    /// Verify the cleanup invariant on the given annotated graph.
    ///
    /// This runs the standard verification, then:
    /// - Filters out Leak violations where the resource has a `LeakAnnotation`
    /// - Double-free and use-after-free violations are NEVER filtered
    /// - Populates `intentional_leaks` and `unannotated_leaks` in the report
    pub fn verify_annotated(&self, annotated: &AnnotatedCleanupGraph) -> CleanupReport {
        // Run standard verification on the inner graph
        let base_report = self.verify(&annotated.graph);

        let mut intentional_leaks = Vec::new();
        let mut unannotated_leaks = Vec::new();
        let mut remaining_violations = Vec::new();

        for violation in base_report.violations {
            match violation.kind {
                ViolationKind::Leak => {
                    if annotated.is_annotated_leak(violation.resource) {
                        // This leak is annotated — it's intentional
                        if let Some(annotation) = annotated.get_leak_annotation(violation.resource) {
                            intentional_leaks.push(annotation.clone());
                        }
                        // Do NOT add to remaining violations
                    } else {
                        // Unannotated leak — genuine violation
                        unannotated_leaks.push(violation.clone());
                        remaining_violations.push(violation);
                    }
                }
                // Double-free and use-after-free are NEVER filtered
                ViolationKind::DoubleFree | ViolationKind::UseAfterFree => {
                    remaining_violations.push(violation);
                }
            }
        }

        CleanupReport::from_annotated(
            remaining_violations,
            intentional_leaks,
            unannotated_leaks,
            annotated.annotation_count(),
            base_report.paths_explored,
            base_report.acquires_checked,
        )
    }

    /// Validate leak annotations for consistency with the graph.
    ///
    /// Checks for:
    /// - `AnnotatedButFreed`: A resource marked as leaked but actually freed
    /// - `AnnotatedButAccessedAfter`: A resource marked as leaked but accessed
    ///   after its annotation point
    /// - `MissingJustification`: No reviewer and no custom reason provided
    pub fn validate_annotations(
        &self,
        annotated: &AnnotatedCleanupGraph,
    ) -> Vec<AnnotationIssue> {
        let mut issues = Vec::new();

        for annotation in annotated.leak_annotations() {
            let resource = annotation.resource;

            // Check: AnnotatedButFreed — marked as leak but actually freed
            let release_nodes = annotated.graph.release_nodes_for(resource);
            if !release_nodes.is_empty() {
                issues.push(AnnotationIssue {
                    resource,
                    issue: AnnotationIssueKind::AnnotatedButFreed,
                });
                continue; // No point checking further for this annotation
            }

            // Check: MissingJustification — no reviewer and no custom reason
            if annotation.reviewer.is_none()
                && !matches!(annotation.reason, LeakReason::Custom(_))
            {
                issues.push(AnnotationIssue {
                    resource,
                    issue: AnnotationIssueKind::MissingJustification,
                });
            }

            // Check: AnnotatedButAccessedAfter — resource accessed after annotation point
            // We check if there are access nodes for this resource that appear
            // after the annotation point in the graph. For simplicity, we check
            // if any access node is reachable from the acquire node. Since the
            // annotation point is typically at the acquire site, any access
            // after that is considered "accessed after annotation".
            // A more precise check would require tracking node ordering.
            // Here we use a simplified heuristic: if there are access nodes
            // for the resource AND they are reachable from any acquire node
            // for that resource, we flag it.
            let access_nodes = annotated.graph.access_nodes_for(resource);
            let acquire_nodes = annotated.graph.acquire_nodes_for(resource);

            for &access_id in &access_nodes {
                for &acquire_id in &acquire_nodes {
                    if annotated.graph.has_path(acquire_id, access_id) {
                        // The resource is accessed after being acquired.
                        // Since it's annotated as a leak, this access is fine
                        // (the resource is still live). But if the annotation
                        // point is set *before* the access and the resource is
                        // "leaked" (never freed), accessing it is expected.
                        // We only flag if the annotation_point string matches
                        // a node label that is before the access in the graph.
                        // For simplicity, if the annotation_point exactly matches
                        // an access node label, we flag AnnotatedButAccessedAfter.
                        if let Some(node) = annotated.graph.get_node(access_id) {
                            if node.label == annotation.annotation_point {
                                issues.push(AnnotationIssue {
                                    resource,
                                    issue: AnnotationIssueKind::AnnotatedButAccessedAfter,
                                });
                            }
                        }
                    }
                }
            }
        }

        issues
    }
}

// ---------------------------------------------------------------------------
// Annotation validation types
// ---------------------------------------------------------------------------

/// A kind of issue detected when validating leak annotations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationIssueKind {
    /// Resource marked as leaked but actually freed in the graph.
    AnnotatedButFreed,
    /// Resource marked as leaked but accessed after the "leak point".
    AnnotatedButAccessedAfter,
    /// No reviewer and no custom reason provided for the annotation.
    MissingJustification,
}

impl fmt::Display for AnnotationIssueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnnotationIssueKind::AnnotatedButFreed => {
                write!(f, "annotated as leak but actually freed")
            }
            AnnotationIssueKind::AnnotatedButAccessedAfter => {
                write!(f, "annotated as leak but accessed after leak point")
            }
            AnnotationIssueKind::MissingJustification => {
                write!(f, "missing justification (no reviewer or custom reason)")
            }
        }
    }
}

/// An issue detected when validating a leak annotation.
#[derive(Debug, Clone)]
pub struct AnnotationIssue {
    /// The resource with the problematic annotation.
    pub resource: ResourceId,
    /// The kind of issue detected.
    pub issue: AnnotationIssueKind,
}

impl fmt::Display for AnnotationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.resource, self.issue)
    }
}

impl Default for CleanupVerifier {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a program point label.
    fn pp(file: &str, line: u32) -> String {
        format!("{file}:{line}")
    }

    // -----------------------------------------------------------------------
    // Test 1: Simple alloc/dealloc — clean program
    // -----------------------------------------------------------------------
    #[test]
    fn test_simple_alloc_dealloc_clean() {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let access = graph.add_node(
            OperationKind::Access { resource: res },
            pp("test.vu", 3),
        );
        let dealloc = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 4),
        );
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 5));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, access).unwrap();
        graph.add_edge(access, dealloc).unwrap();
        graph.add_edge(dealloc, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(report.clean, "Expected clean, got: {:?}", report.violations);
        assert_eq!(report.paths_explored, 1);
    }

    // -----------------------------------------------------------------------
    // Test 2: Leaked memory — allocation with no deallocation
    // -----------------------------------------------------------------------
    #[test]
    fn test_leaked_memory() {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let access = graph.add_node(
            OperationKind::Access { resource: res },
            pp("test.vu", 3),
        );
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 4));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, access).unwrap();
        graph.add_edge(access, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(!report.clean, "Expected leak violation");
        assert!(report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::Leak && v.resource == res));
    }

    // -----------------------------------------------------------------------
    // Test 3: Double-free — same resource freed twice
    // -----------------------------------------------------------------------
    #[test]
    fn test_double_free() {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let dealloc1 = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 3),
        );
        let dealloc2 = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 4),
        );
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 5));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, dealloc1).unwrap();
        graph.add_edge(dealloc1, dealloc2).unwrap();
        graph.add_edge(dealloc2, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(!report.clean, "Expected double-free violation");
        assert!(report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DoubleFree && v.resource == res));
    }

    // -----------------------------------------------------------------------
    // Test 4: Use-after-free — access after deallocation
    // -----------------------------------------------------------------------
    #[test]
    fn test_use_after_free() {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let dealloc = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 3),
        );
        let access = graph.add_node(
            OperationKind::Access { resource: res },
            pp("test.vu", 4),
        );
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 5));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, dealloc).unwrap();
        graph.add_edge(dealloc, access).unwrap();
        graph.add_edge(access, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(!report.clean, "Expected use-after-free violation");
        assert!(report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UseAfterFree && v.resource == res));
    }

    // -----------------------------------------------------------------------
    // Test 5: Conditional cleanup — both branches free the resource
    // -----------------------------------------------------------------------
    #[test]
    fn test_conditional_cleanup_both_branches_free() {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let branch = graph.add_node(
            OperationKind::Branch {
                condition: "cond".into(),
            },
            pp("test.vu", 3),
        );
        // Then branch
        let access_then = graph.add_node(
            OperationKind::Access { resource: res },
            pp("test.vu", 4),
        );
        let free_then = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 5),
        );
        // Else branch
        let free_else = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 6),
        );
        let join = graph.add_node(OperationKind::Join, pp("test.vu", 7));
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 8));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, branch).unwrap();
        graph.add_edge(branch, access_then).unwrap();
        graph.add_edge(access_then, free_then).unwrap();
        graph.add_edge(branch, free_else).unwrap();
        graph.add_edge(free_then, join).unwrap();
        graph.add_edge(free_else, join).unwrap();
        graph.add_edge(join, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(report.clean, "Expected clean, got: {:?}", report.violations);
        assert_eq!(report.paths_explored, 2);
    }

    // -----------------------------------------------------------------------
    // Test 6: Conditional cleanup — one branch leaks
    // -----------------------------------------------------------------------
    #[test]
    fn test_conditional_cleanup_one_branch_leaks() {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let branch = graph.add_node(
            OperationKind::Branch {
                condition: "cond".into(),
            },
            pp("test.vu", 3),
        );
        // Then branch — frees the resource
        let free_then = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 4),
        );
        // Else branch — does NOT free (leak!)
        let passthrough_else = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 5),
        );
        let join = graph.add_node(OperationKind::Join, pp("test.vu", 6));
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 7));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, branch).unwrap();
        graph.add_edge(branch, free_then).unwrap();
        graph.add_edge(branch, passthrough_else).unwrap();
        graph.add_edge(free_then, join).unwrap();
        graph.add_edge(passthrough_else, join).unwrap();
        graph.add_edge(join, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(!report.clean, "Expected leak on one branch");
        assert!(report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::Leak && v.resource == res));
    }

    // -----------------------------------------------------------------------
    // Test 7: Error path cleanup — resource freed on error path
    // -----------------------------------------------------------------------
    #[test]
    fn test_error_path_cleanup() {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let access = graph.add_node(
            OperationKind::Access { resource: res },
            pp("test.vu", 3),
        );
        let branch = graph.add_node(
            OperationKind::Branch {
                condition: "error?".into(),
            },
            pp("test.vu", 4),
        );
        // Happy path
        let free_happy = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 5),
        );
        let ret_happy = graph.add_node(OperationKind::Return, pp("test.vu", 6));
        // Error path — must also free!
        let free_err = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 7),
        );
        let err_ret = graph.add_node(
            OperationKind::ErrorReturn {
                description: "oops".into(),
            },
            pp("test.vu", 8),
        );

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, access).unwrap();
        graph.add_edge(access, branch).unwrap();
        graph.add_edge(branch, free_happy).unwrap();
        graph.add_edge(free_happy, ret_happy).unwrap();
        graph.add_edge(branch, free_err).unwrap();
        graph.add_edge(free_err, err_ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(report.clean, "Expected clean (both paths free), got: {:?}", report.violations);
    }

    // -----------------------------------------------------------------------
    // Test 8: Error path with leak — resource NOT freed on error path
    // -----------------------------------------------------------------------
    #[test]
    fn test_error_path_leak() {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let branch = graph.add_node(
            OperationKind::Branch {
                condition: "error?".into(),
            },
            pp("test.vu", 3),
        );
        // Happy path — frees
        let free_happy = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 4),
        );
        let ret_happy = graph.add_node(OperationKind::Return, pp("test.vu", 5));
        // Error path — does NOT free (leak on error path!)
        let err_ret = graph.add_node(
            OperationKind::ErrorReturn {
                description: "early exit".into(),
            },
            pp("test.vu", 6),
        );

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, branch).unwrap();
        graph.add_edge(branch, free_happy).unwrap();
        graph.add_edge(free_happy, ret_happy).unwrap();
        graph.add_edge(branch, err_ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(!report.clean, "Expected leak on error path");
        assert!(report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::Leak && v.resource == res));
    }

    // -----------------------------------------------------------------------
    // Test 9: Nested resources — two allocations, both freed
    // -----------------------------------------------------------------------
    #[test]
    fn test_nested_resources_clean() {
        let mut graph = CleanupGraph::new();
        let res1 = ResourceId(1);
        let res2 = ResourceId(2);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let alloc1 = graph.add_node(
            OperationKind::Acquire {
                resource: res1,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let alloc2 = graph.add_node(
            OperationKind::Acquire {
                resource: res2,
                kind: ResourceKind::Lock,
            },
            pp("test.vu", 3),
        );
        let access = graph.add_node(
            OperationKind::Access { resource: res1 },
            pp("test.vu", 4),
        );
        let free2 = graph.add_node(
            OperationKind::Release {
                resource: res2,
                kind: ResourceKind::Lock,
            },
            pp("test.vu", 5),
        );
        let free1 = graph.add_node(
            OperationKind::Release {
                resource: res1,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 6),
        );
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 7));

        graph.add_edge(entry, alloc1).unwrap();
        graph.add_edge(alloc1, alloc2).unwrap();
        graph.add_edge(alloc2, access).unwrap();
        graph.add_edge(access, free2).unwrap();
        graph.add_edge(free2, free1).unwrap();
        graph.add_edge(free1, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(report.clean, "Expected clean, got: {:?}", report.violations);
    }

    // -----------------------------------------------------------------------
    // Test 10: Nested resources — inner resource leaks
    // -----------------------------------------------------------------------
    #[test]
    fn test_nested_resources_inner_leak() {
        let mut graph = CleanupGraph::new();
        let res1 = ResourceId(1);
        let res2 = ResourceId(2);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let alloc1 = graph.add_node(
            OperationKind::Acquire {
                resource: res1,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let alloc2 = graph.add_node(
            OperationKind::Acquire {
                resource: res2,
                kind: ResourceKind::Lock,
            },
            pp("test.vu", 3),
        );
        let free1 = graph.add_node(
            OperationKind::Release {
                resource: res1,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 4),
        );
        // res2 is never released
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 5));

        graph.add_edge(entry, alloc1).unwrap();
        graph.add_edge(alloc1, alloc2).unwrap();
        graph.add_edge(alloc2, free1).unwrap();
        graph.add_edge(free1, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(!report.clean, "Expected leak for res2");
        assert!(report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::Leak && v.resource == res2));
        // res1 should NOT be leaked
        assert!(!report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::Leak && v.resource == res1));
    }

    // -----------------------------------------------------------------------
    // Test 11: Quick reachability check
    // -----------------------------------------------------------------------
    #[test]
    fn test_quick_reachability_check() {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 1),
        );
        let dealloc = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        graph.add_edge(alloc, dealloc).unwrap();

        let verifier = CleanupVerifier::new();
        let unreachable = verifier.quick_check_reachability(&graph);
        assert!(unreachable.is_empty(), "Expected no unreachable releases");

        // Now test with a leak (no dealloc node at all)
        let mut graph2 = CleanupGraph::new();
        let res2 = ResourceId(2);
        graph2.add_node(
            OperationKind::Acquire {
                resource: res2,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 3),
        );
        let unreachable2 = verifier.quick_check_reachability(&graph2);
        assert_eq!(unreachable2.len(), 1);
        assert_eq!(unreachable2[0].1, res2);
    }

    // -----------------------------------------------------------------------
    // Test 12: Conversion to VerificationResult
    // -----------------------------------------------------------------------
    #[test]
    fn test_to_verification_result_clean() {
        let report = CleanupReport::from_violations(vec![], 3, 1);
        let result = report.to_verification_result();
        assert!(result.is_proven());
        assert_eq!(result.invariant, "cleanup");
    }

    #[test]
    fn test_to_verification_result_violated() {
        let violation = CleanupViolation {
            kind: ViolationKind::Leak,
            resource: ResourceId(1),
            resource_kind: ResourceKind::Memory,
            path: vec!["entry".into(), "alloc".into(), "return".into()],
            violation_node: NodeId(99),
            description: "leaked".into(),
        };
        let report = CleanupReport::from_violations(vec![violation], 1, 1);
        let result = report.to_verification_result();
        assert!(result.is_violated());
    }

    // -----------------------------------------------------------------------
    // Test 13: File handle resource — acquire/release
    // -----------------------------------------------------------------------
    #[test]
    fn test_file_handle_cleanup() {
        let mut graph = CleanupGraph::new();
        let fh = ResourceId(10);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let open = graph.add_node(
            OperationKind::Acquire {
                resource: fh,
                kind: ResourceKind::FileHandle,
            },
            pp("test.vu", 2),
        );
        let access = graph.add_node(
            OperationKind::Access { resource: fh },
            pp("test.vu", 3),
        );
        let close = graph.add_node(
            OperationKind::Release {
                resource: fh,
                kind: ResourceKind::FileHandle,
            },
            pp("test.vu", 4),
        );
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 5));

        graph.add_edge(entry, open).unwrap();
        graph.add_edge(open, access).unwrap();
        graph.add_edge(access, close).unwrap();
        graph.add_edge(close, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(report.clean, "Expected clean, got: {:?}", report.violations);
    }

    // -----------------------------------------------------------------------
    // Test 14: Lock resource — double unlock
    // -----------------------------------------------------------------------
    #[test]
    fn test_lock_double_unlock() {
        let mut graph = CleanupGraph::new();
        let lock = ResourceId(20);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let lock_acquire = graph.add_node(
            OperationKind::Acquire {
                resource: lock,
                kind: ResourceKind::Lock,
            },
            pp("test.vu", 2),
        );
        let unlock1 = graph.add_node(
            OperationKind::Release {
                resource: lock,
                kind: ResourceKind::Lock,
            },
            pp("test.vu", 3),
        );
        let unlock2 = graph.add_node(
            OperationKind::Release {
                resource: lock,
                kind: ResourceKind::Lock,
            },
            pp("test.vu", 4),
        );
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 5));

        graph.add_edge(entry, lock_acquire).unwrap();
        graph.add_edge(lock_acquire, unlock1).unwrap();
        graph.add_edge(unlock1, unlock2).unwrap();
        graph.add_edge(unlock2, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(!report.clean, "Expected double-unlock violation");
        assert!(report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DoubleFree && v.resource == lock));
    }

    // -----------------------------------------------------------------------
    // Test 15: Use-after-free on conditional path
    // -----------------------------------------------------------------------
    #[test]
    fn test_conditional_use_after_free() {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(
            OperationKind::Passthrough,
            pp("test.vu", 1),
        );
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let branch1 = graph.add_node(
            OperationKind::Branch {
                condition: "c1".into(),
            },
            pp("test.vu", 3),
        );
        // Branch A: free then use-after-free
        let free_a = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 4),
        );
        let access_after_free = graph.add_node(
            OperationKind::Access { resource: res },
            pp("test.vu", 5),
        );
        let ret_a = graph.add_node(OperationKind::Return, pp("test.vu", 6));
        // Branch B: normal use then free
        let access_b = graph.add_node(
            OperationKind::Access { resource: res },
            pp("test.vu", 7),
        );
        let free_b = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 8),
        );
        let ret_b = graph.add_node(OperationKind::Return, pp("test.vu", 9));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, branch1).unwrap();
        graph.add_edge(branch1, free_a).unwrap();
        graph.add_edge(free_a, access_after_free).unwrap();
        graph.add_edge(access_after_free, ret_a).unwrap();
        graph.add_edge(branch1, access_b).unwrap();
        graph.add_edge(access_b, free_b).unwrap();
        graph.add_edge(free_b, ret_b).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(!report.clean, "Expected use-after-free on branch A");
        assert!(report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UseAfterFree && v.resource == res));
    }

    // -----------------------------------------------------------------------
    // Test 16: Empty graph
    // -----------------------------------------------------------------------
    #[test]
    fn test_empty_graph() {
        let graph = CleanupGraph::new();
        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(report.clean);
        assert_eq!(report.paths_explored, 0);
    }

    // -----------------------------------------------------------------------
    // Test 17: Display formatting for violations
    // -----------------------------------------------------------------------
    #[test]
    fn test_violation_display() {
        let v = CleanupViolation {
            kind: ViolationKind::Leak,
            resource: ResourceId(42),
            resource_kind: ResourceKind::Memory,
            path: vec!["main:1".into(), "alloc:2".into(), "return:3".into()],
            violation_node: NodeId(2),
            description: "res42 (memory) acquired but never released on this path".into(),
        };
        let s = format!("{v}");
        assert!(s.contains("resource leak"));
        assert!(s.contains("res42"));
    }

    // =======================================================================
    // Leak Annotation Tests
    // =======================================================================

    /// Helper: build a simple graph with a leaked resource (no release).
    fn build_leaky_graph() -> CleanupGraph {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let access = graph.add_node(
            OperationKind::Access { resource: res },
            pp("test.vu", 3),
        );
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 4));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, access).unwrap();
        graph.add_edge(access, ret).unwrap();
        graph.set_entry(entry).unwrap();

        graph
    }

    /// Helper: build a double-free graph.
    fn build_double_free_graph() -> CleanupGraph {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let dealloc1 = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 3),
        );
        let dealloc2 = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 4),
        );
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 5));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, dealloc1).unwrap();
        graph.add_edge(dealloc1, dealloc2).unwrap();
        graph.add_edge(dealloc2, ret).unwrap();
        graph.set_entry(entry).unwrap();

        graph
    }

    /// Helper: build a use-after-free graph.
    fn build_use_after_free_graph() -> CleanupGraph {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let dealloc = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 3),
        );
        let access = graph.add_node(
            OperationKind::Access { resource: res },
            pp("test.vu", 4),
        );
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 5));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, dealloc).unwrap();
        graph.add_edge(dealloc, access).unwrap();
        graph.add_edge(access, ret).unwrap();
        graph.set_entry(entry).unwrap();

        graph
    }

    // -----------------------------------------------------------------------
    // Annotation Test 1: Arena annotation suppresses leak warning
    // -----------------------------------------------------------------------
    #[test]
    fn test_arena_annotation_suppresses_leak() {
        let graph = build_leaky_graph();
        let mut annotated = AnnotatedCleanupGraph::new(graph);
        annotated
            .add_leak_annotation(LeakAnnotation {
                resource: ResourceId(1),
                reason: LeakReason::Arena,
                annotation_point: pp("test.vu", 2),
                reviewer: Some("alice".into()),
            })
            .unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify_annotated(&annotated);

        // The report should be clean (leak suppressed by annotation)
        assert!(report.clean, "Expected clean with arena annotation, got: {:?}", report.violations);
        // There should be one intentional leak
        assert_eq!(report.intentional_leaks.len(), 1);
        assert_eq!(report.intentional_leaks[0].reason, LeakReason::Arena);
        // No unannotated leaks
        assert!(report.unannotated_leaks.is_empty());
        // Result should be ProbablySafe
        let result = report.to_verification_result();
        assert!(matches!(result.status, VerificationStatus::ProbablySafe { .. }));
    }

    // -----------------------------------------------------------------------
    // Annotation Test 2: Global cache annotation
    // -----------------------------------------------------------------------
    #[test]
    fn test_global_cache_annotation_suppresses_leak() {
        let graph = build_leaky_graph();
        let mut annotated = AnnotatedCleanupGraph::new(graph);
        annotated
            .add_leak_annotation(LeakAnnotation {
                resource: ResourceId(1),
                reason: LeakReason::GlobalCache,
                annotation_point: pp("test.vu", 2),
                reviewer: Some("bob".into()),
            })
            .unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify_annotated(&annotated);

        assert!(report.clean, "Expected clean with global cache annotation");
        assert_eq!(report.intentional_leaks.len(), 1);
        assert_eq!(report.intentional_leaks[0].reason, LeakReason::GlobalCache);
    }

    // -----------------------------------------------------------------------
    // Annotation Test 3: Singleton annotation
    // -----------------------------------------------------------------------
    #[test]
    fn test_singleton_annotation_suppresses_leak() {
        let graph = build_leaky_graph();
        let mut annotated = AnnotatedCleanupGraph::new(graph);
        annotated
            .add_leak_annotation(LeakAnnotation {
                resource: ResourceId(1),
                reason: LeakReason::Singleton,
                annotation_point: pp("test.vu", 2),
                reviewer: Some("carol".into()),
            })
            .unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify_annotated(&annotated);

        assert!(report.clean, "Expected clean with singleton annotation");
        assert_eq!(report.intentional_leaks.len(), 1);
        assert_eq!(report.intentional_leaks[0].reason, LeakReason::Singleton);
    }

    // -----------------------------------------------------------------------
    // Annotation Test 4: Annotation doesn't suppress double-free
    // -----------------------------------------------------------------------
    #[test]
    fn test_annotation_does_not_suppress_double_free() {
        let graph = build_double_free_graph();
        let mut annotated = AnnotatedCleanupGraph::new(graph);
        annotated
            .add_leak_annotation(LeakAnnotation {
                resource: ResourceId(1),
                reason: LeakReason::Arena,
                annotation_point: pp("test.vu", 2),
                reviewer: Some("alice".into()),
            })
            .unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify_annotated(&annotated);

        // Double-free should NOT be suppressed
        assert!(!report.clean, "Expected violation: double-free must not be suppressed by annotation");
        assert!(report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::DoubleFree && v.resource == ResourceId(1)));
    }

    // -----------------------------------------------------------------------
    // Annotation Test 5: Annotation doesn't suppress use-after-free
    // -----------------------------------------------------------------------
    #[test]
    fn test_annotation_does_not_suppress_use_after_free() {
        let graph = build_use_after_free_graph();
        let mut annotated = AnnotatedCleanupGraph::new(graph);
        annotated
            .add_leak_annotation(LeakAnnotation {
                resource: ResourceId(1),
                reason: LeakReason::Arena,
                annotation_point: pp("test.vu", 2),
                reviewer: Some("alice".into()),
            })
            .unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify_annotated(&annotated);

        // Use-after-free should NOT be suppressed
        assert!(!report.clean, "Expected violation: use-after-free must not be suppressed by annotation");
        assert!(report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::UseAfterFree && v.resource == ResourceId(1)));
    }

    // -----------------------------------------------------------------------
    // Annotation Test 6: Missing annotation still reports leak
    // -----------------------------------------------------------------------
    #[test]
    fn test_missing_annotation_still_reports_leak() {
        let graph = build_leaky_graph();
        let annotated = AnnotatedCleanupGraph::new(graph);

        let verifier = CleanupVerifier::new();
        let report = verifier.verify_annotated(&annotated);

        // Without any annotation, the leak should be reported
        assert!(!report.clean, "Expected leak violation without annotation");
        assert!(report.unannotated_leaks.iter().any(|v| v.kind == ViolationKind::Leak));
        assert!(report.intentional_leaks.is_empty());
        // Result should be Violated
        let result = report.to_verification_result();
        assert!(result.is_violated());
    }

    // -----------------------------------------------------------------------
    // Annotation Test 7: AnnotatedButFreed issue detected
    // -----------------------------------------------------------------------
    #[test]
    fn test_annotated_but_freed_issue() {
        // Build a clean graph (resource IS freed)
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let dealloc = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 3),
        );
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 4));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, dealloc).unwrap();
        graph.add_edge(dealloc, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let mut annotated = AnnotatedCleanupGraph::new(graph);
        annotated
            .add_leak_annotation(LeakAnnotation {
                resource: res,
                reason: LeakReason::Arena,
                annotation_point: pp("test.vu", 2),
                reviewer: Some("alice".into()),
            })
            .unwrap();

        let verifier = CleanupVerifier::new();
        let issues = verifier.validate_annotations(&annotated);

        assert_eq!(issues.len(), 1, "Expected exactly one issue");
        assert!(matches!(issues[0].issue, AnnotationIssueKind::AnnotatedButFreed));
        assert_eq!(issues[0].resource, res);
    }

    // -----------------------------------------------------------------------
    // Annotation Test 8: Custom leak reason
    // -----------------------------------------------------------------------
    #[test]
    fn test_custom_leak_reason() {
        let graph = build_leaky_graph();
        let mut annotated = AnnotatedCleanupGraph::new(graph);
        annotated
            .add_leak_annotation(LeakAnnotation {
                resource: ResourceId(1),
                reason: LeakReason::Custom("performance-critical hot path".into()),
                annotation_point: pp("test.vu", 2),
                reviewer: None, // Custom reason doesn't require reviewer
            })
            .unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify_annotated(&annotated);

        assert!(report.clean, "Expected clean with custom reason annotation");
        assert_eq!(report.intentional_leaks.len(), 1);
        assert!(matches!(
            &report.intentional_leaks[0].reason,
            LeakReason::Custom(s) if s == "performance-critical hot path"
        ));

        // Custom reason should NOT trigger MissingJustification
        let issues = verifier.validate_annotations(&annotated);
        let has_missing = issues.iter().any(|i| matches!(i.issue, AnnotationIssueKind::MissingJustification));
        assert!(!has_missing, "Custom reason should not trigger MissingJustification");
    }

    // -----------------------------------------------------------------------
    // Annotation Test 9: Duplicate annotation rejected
    // -----------------------------------------------------------------------
    #[test]
    fn test_duplicate_annotation_rejected() {
        let graph = build_leaky_graph();
        let mut annotated = AnnotatedCleanupGraph::new(graph);

        annotated
            .add_leak_annotation(LeakAnnotation {
                resource: ResourceId(1),
                reason: LeakReason::Arena,
                annotation_point: pp("test.vu", 2),
                reviewer: Some("alice".into()),
            })
            .unwrap();

        // Second annotation for the same resource should fail
        let result = annotated.add_leak_annotation(LeakAnnotation {
            resource: ResourceId(1),
            reason: LeakReason::GlobalCache,
            annotation_point: pp("test.vu", 2),
            reviewer: Some("bob".into()),
        });
        assert!(result.is_err(), "Expected error for duplicate annotation");
    }

    // -----------------------------------------------------------------------
    // Annotation Test 10: MissingJustification detected
    // -----------------------------------------------------------------------
    #[test]
    fn test_missing_justification_detected() {
        let graph = build_leaky_graph();
        let mut annotated = AnnotatedCleanupGraph::new(graph);
        // No reviewer and no Custom reason → MissingJustification
        annotated
            .add_leak_annotation(LeakAnnotation {
                resource: ResourceId(1),
                reason: LeakReason::Arena,
                annotation_point: pp("test.vu", 2),
                reviewer: None,
            })
            .unwrap();

        let verifier = CleanupVerifier::new();
        let issues = verifier.validate_annotations(&annotated);

        let has_missing = issues.iter().any(|i| matches!(i.issue, AnnotationIssueKind::MissingJustification));
        assert!(has_missing, "Expected MissingJustification issue for no reviewer and no custom reason");
    }

    // -----------------------------------------------------------------------
    // Annotation Test 11: is_annotated_leak and annotation_count
    // -----------------------------------------------------------------------
    #[test]
    fn test_annotated_graph_queries() {
        let graph = build_leaky_graph();
        let mut annotated = AnnotatedCleanupGraph::new(graph);

        assert!(!annotated.is_annotated_leak(ResourceId(1)));
        assert_eq!(annotated.annotation_count(), 0);

        annotated
            .add_leak_annotation(LeakAnnotation {
                resource: ResourceId(1),
                reason: LeakReason::StaticStorage,
                annotation_point: pp("test.vu", 2),
                reviewer: Some("alice".into()),
            })
            .unwrap();

        assert!(annotated.is_annotated_leak(ResourceId(1)));
        assert!(!annotated.is_annotated_leak(ResourceId(99)));
        assert_eq!(annotated.annotation_count(), 1);

        // Verify we can iterate over annotations
        let annotations: Vec<&LeakAnnotation> = annotated.leak_annotations().collect();
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].reason, LeakReason::StaticStorage);
    }

    // -----------------------------------------------------------------------
    // Annotation Test 12: ProbablySafe result for annotated-only leaks
    // -----------------------------------------------------------------------
    #[test]
    fn test_probably_safe_result_for_annotated_leaks() {
        let graph = build_leaky_graph();
        let mut annotated = AnnotatedCleanupGraph::new(graph);
        annotated
            .add_leak_annotation(LeakAnnotation {
                resource: ResourceId(1),
                reason: LeakReason::Intentional,
                annotation_point: pp("test.vu", 2),
                reviewer: Some("reviewer".into()),
            })
            .unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify_annotated(&annotated);
        let result = report.to_verification_result();

        // Should be ProbablySafe, not Proven or Violated
        assert!(matches!(result.status, VerificationStatus::ProbablySafe { .. }));
        // Should have assumptions
        if let VerificationStatus::ProbablySafe { assumptions } = &result.status {
            assert_eq!(assumptions.len(), 1);
            assert!(assumptions[0].contains("intentionally leaked"));
        } else {
            panic!("Expected ProbablySafe status");
        }
    }
}
