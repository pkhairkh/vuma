//! Cleanup Invariant Verifier for the IVE module.
//!
//! This module implements VUMA's **cleanup invariant** (Invariant 5):
//! "Every allocation is eventually freed or explicitly leaked; no region is
//! freed twice." (See `docs/specs/vuma-invariants-spec.md`, §7.) It operates
//! on a simplified control-flow/resource graph derived from the SCG and
//! verifies:
//!
//! 1. **No resource leaks (primary job)** — every allocation/acquisition
//!    reaches a matching deallocation/release OR an explicit leak annotation
//!    ([`OperationKind::Leak`]), on ALL execution paths. A resource marked as
//!    explicitly leaked is exempt from the leak violation, per the spec's
//!    "freed or explicitly leaked" clause (Invariant 5, Part A).
//! 2. **No double-free** — the same resource is never freed more than once
//!    on any execution path.
//! 3. **No use-after-free (defense-in-depth)** — no access occurs after a
//!    deallocation on any execution path. This is the spec's Part C, which
//!    overlaps with the Liveness invariant; it is retained here as
//!    defense-in-depth and will be removed once the Liveness verifier
//!    (W2-a) owns UAF detection.
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
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

// ---------------------------------------------------------------------------
// Resource identifiers
// ---------------------------------------------------------------------------

/// Unique identifier for a tracked resource (allocation, lock, file handle, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    /// Explicitly leak a resource (intentional non-deallocation).
    ///
    /// This is the cleanup verifier's representation of the spec's
    /// "explicitly leaked" exception (Invariant 5, Part A): the programmer
    /// or the IVE annotates the resource as intentionally never freed
    /// (e.g. a long-lived arena, global state, or process-lifetime mapping).
    ///
    /// When a `Leak` node is encountered on a path, the resource is moved
    /// from the live set to the leaked set and is NOT flagged as
    /// [`ViolationKind::Leak`] at the terminal node. This mirrors the
    /// behaviour of `RegionStatus::Leaked` in `vuma_core::region`.
    Leak {
        /// The resource being explicitly leaked.
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
            OperationKind::Leak { resource, .. } => Some(*resource),
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

    /// Returns `true` if this is an explicit-leak annotation.
    ///
    /// See [`OperationKind::Leak`] for the spec's "explicitly leaked"
    /// exception (Invariant 5, Part A).
    pub fn is_leak(&self) -> bool {
        matches!(self, OperationKind::Leak { .. })
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

    /// Return all explicit-leak nodes in the graph.
    ///
    /// A leak node ([`OperationKind::Leak`]) annotates a resource as
    /// intentionally never freed — the spec's "explicitly leaked"
    /// exception (Invariant 5, Part A). Such a resource is not flagged as
    /// [`ViolationKind::Leak`] at the terminal node.
    pub fn leak_nodes(&self) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, n)| n.operation.is_leak())
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

    /// Find all nodes where a specific resource is explicitly leaked.
    pub fn leak_nodes_for(&self, resource: ResourceId) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, n)| match &n.operation {
                OperationKind::Leak { resource: r, .. } => *r == resource,
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
            .filter(|&&id| self.successors.get(&id).is_none_or(|s| s.is_empty()))
            .copied()
            .collect()
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
#[derive(Debug, Clone)]
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
    /// Resources currently acquired (not yet released, not yet leaked) on
    /// this path. Maps ResourceId → (AcquireNodeId, ResourceKind).
    ///
    /// Only resources still in this set at a terminal node are flagged as
    /// [`ViolationKind::Leak`] (see [`PathState::check_leaks`]).
    live_resources: BTreeMap<ResourceId, (NodeId, ResourceKind)>,
    /// Resources that have been released on this path.
    /// Maps ResourceId → (ReleaseNodeId, ResourceKind).
    released_resources: BTreeMap<ResourceId, (NodeId, ResourceKind)>,
    /// Resources that have been explicitly marked as Leaked on this path
    /// (see [`OperationKind::Leak`]). Maps ResourceId → (LeakNodeId,
    /// ResourceKind). These are EXEMPT from the leak violation per the
    /// spec's "freed or explicitly leaked" clause (Invariant 5, Part A).
    leaked_resources: BTreeMap<ResourceId, (NodeId, ResourceKind)>,
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
                // If it was previously released, re-acquisition is fine:
                // clear the release count and remove from released set so
                // a subsequent release is not a false double-free.
                self.released_resources.remove(resource);
                self.release_count.remove(resource);
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
            OperationKind::Leak { resource, kind } => {
                // Explicit leak annotation (spec Invariant 5, Part A: "freed
                // or explicitly leaked"). Move the resource out of the live
                // set into the leaked set so it is NOT flagged as a
                // `ViolationKind::Leak` at the terminal node.
                //
                // A `Leak` on an already-released resource, or a second
                // `Leak` on an already-leaked resource, is treated as
                // idempotent and silently absorbed (no violation). This
                // matches the vuma_core `invariant_cleanup.rs` behaviour
                // for `RegionStatus::Leaked`.
                if let Some((_, rk)) = self.live_resources.remove(resource) {
                    self.leaked_resources.insert(*resource, (node.id, rk));
                } else {
                    // Resource is not currently live (already released or
                    // already leaked). Record the annotation idempotently,
                    // preserving the first-seen kind if available.
                    self.leaked_resources
                        .entry(*resource)
                        .or_insert((node.id, kind.clone()));
                }
            }
            OperationKind::Access { resource }
                // Check for use-after-free: resource has been released
                if self.released_resources.contains_key(resource) =>
                {
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
            _ => {}
        }

        self.path_labels.push(node.label.clone());
        self.path_nodes.push(node.id);

        violations
    }

    /// Check for leaks at a terminal node: any live resource that hasn't
    /// been released AND hasn't been explicitly leaked is a leak.
    ///
    /// This is the spec's primary cleanup check (Invariant 5, Part A):
    /// "∀ r ∈ R : r.free_point ≠ null ∨ r.status = Leaked". Resources in
    /// [`PathState::leaked_resources`] satisfy the second disjunct and are
    /// therefore NOT reported here.
    fn check_leaks(&self, terminal_node: NodeId) -> Vec<CleanupViolation> {
        let mut violations = Vec::new();
        for (&resource, (_, kind)) in &self.live_resources {
            // Only report a leak if the resource is neither released nor
            // explicitly leaked. (Resources in `live_resources` are by
            // construction not in `released_resources` or
            // `leaked_resources`, but we double-check `leaked_resources`
            // here for defense-in-depth.)
            if self.leaked_resources.contains_key(&resource) {
                continue;
            }
            violations.push(CleanupViolation {
                kind: ViolationKind::Leak,
                resource,
                resource_kind: kind.clone(),
                path: self.path_labels.clone(),
                violation_node: terminal_node,
                description: format!(
                    "{} ({}) acquired but never released and not marked Leaked on this path",
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
/// - **Resource leaks (primary)** — acquire without matching release OR
///   explicit-leak annotation, on any path. See [`OperationKind::Leak`] for
///   the spec's "explicitly leaked" exception (Invariant 5, Part A).
/// - **Double-free** — same resource released more than once on any path.
/// - **Use-after-free (defense-in-depth)** — access after release on any
///   path. This overlaps with the Liveness invariant (spec §7 Part C) and
///   is retained here until the Liveness verifier (W2-a) owns UAF.
pub struct CleanupVerifier {
    /// Maximum path length to explore (prevents infinite traversal on cycles).
    max_path_length: usize,
    /// Whether to emit detailed diagnostic logging.
    verbose: bool,
}

/// The result of cleanup verification.
#[derive(Debug, Clone)]
pub struct CleanupReport {
    /// All violations found.
    pub violations: Vec<CleanupViolation>,
    /// Whether the cleanup invariant holds (no violations found).
    ///
    /// Note: when [`incomplete`](Self::incomplete) is `true`, `clean` being
    /// `true` does **not** mean the invariant is proven — it means only that
    /// no violation was found within the explored prefix. Callers should
    /// consult [`to_verification_result`](Self::to_verification_result),
    /// which returns [`VerificationStatus::Unverified`] in that case.
    pub clean: bool,
    /// Number of terminal paths explored.
    pub paths_explored: usize,
    /// Number of acquire nodes checked.
    pub acquires_checked: usize,
    /// Whether verification was incomplete (hit a path-length or
    /// state-exploration limit). When `true` and `clean` is `true`, the
    /// invariant is **not** proven — it is merely "no violation found
    /// within the explored prefix". The aggregator should treat this as
    /// inconclusive ([`VerificationStatus::Unverified`]).
    ///
    /// This is the fix for false-negative (b): the old verifier silently
    /// returned `Proven` when `max_path_length` was reached, accepting
    /// leaks/UAF beyond the cutoff. Now it surfaces the incompleteness.
    pub incomplete: bool,
    /// Human-readable reason when [`incomplete`](Self::incomplete) is `true`.
    pub incomplete_reason: Option<String>,
}

impl CleanupReport {
    /// Create a report from a list of violations.
    pub fn from_violations(
        violations: Vec<CleanupViolation>,
        paths_explored: usize,
        acquires_checked: usize,
    ) -> Self {
        let clean = violations.is_empty();
        Self {
            violations,
            clean,
            paths_explored,
            acquires_checked,
            incomplete: false,
            incomplete_reason: None,
        }
    }

    /// Convert this report into a [`VerificationResult`] for integration
    /// with the IVE verification engine.
    ///
    /// Precedence:
    /// 1. If any violation was found → [`VerificationStatus::Violated`]
    ///    (a real counterexample trumps incompleteness).
    /// 2. Else if verification was incomplete →
    ///    [`VerificationStatus::Unverified`] (inconclusive: not pass, not
    ///    fail).
    /// 3. Else (fully verified, no violations) →
    ///    [`VerificationStatus::Proven`].
    pub fn to_verification_result(&self) -> VerificationResult {
        if !self.clean {
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
        } else if self.incomplete {
            let reason_str = self
                .incomplete_reason
                .clone()
                .unwrap_or_else(|| "verification incomplete".to_string());
            VerificationResult::new(
                "cleanup",
                VerificationStatus::Unverified { reason: reason_str.clone() },
                format!(
                    "cleanup invariant inconclusive: {} path(s) explored, {} acquire(s) checked; {}",
                    self.paths_explored, self.acquires_checked, reason_str
                ),
            )
        } else {
            VerificationResult::new(
                "cleanup",
                VerificationStatus::Proven,
                format!(
                    "cleanup invariant verified: {} acquire(s) checked across {} path(s)",
                    self.acquires_checked, self.paths_explored
                ),
            )
        }
    }
}

/// Maximum number of distinct `(node, live, released)` states to explore
/// before declaring verification incomplete. This is the termination
/// safeguard for the state-ful DFS: the state space is finite but can be
/// large in pathological graphs, so we cap total explorations. See
/// [`CleanupVerifier::verify`] and [`StateKey`].
const MAX_STATES: usize = 10_000;

/// Memoization key for the state-ful DFS in [`CleanupVerifier::dfs_verify`].
///
/// Two arrivals at the same `node` with the same `live` and `released`
/// resource sets are equivalent for the purpose of future exploration, so
/// we only explore one of them. This is the key fix for false-negative
/// (a): the old simple-path DFS (one visit per node per path) missed
/// cross-iteration UAF, where a resource is freed in one loop iteration
/// and accessed in the next. By keying on the resource state, we allow
/// revisiting a node when the state has changed (e.g. a resource was
/// freed), which is necessary to catch the UAF on the second iteration.
///
/// `live` and `released` are sufficient: they determine leak detection
/// (live set at terminals), UAF detection (released set on access), and
/// double-free is detected at the `Release` node itself on first visit.
type StateKey = (NodeId, BTreeSet<ResourceId>, BTreeSet<ResourceId>);

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
    /// entry node (or from all nodes if no entry is set), checking that every
    /// acquire is matched by a release OR an explicit-leak annotation on every
    /// path (primary job, spec Invariant 5 Part A), and also reporting
    /// double-free and use-after-free violations (the latter as
    /// defense-in-depth — see [`CleanupVerifier`] doc).
    pub fn verify(&self, graph: &CleanupGraph) -> CleanupReport {
        let mut all_violations: Vec<CleanupViolation> = Vec::new();
        let mut paths_explored = 0usize;
        let mut states_explored = 0usize;
        let mut incomplete = false;
        let mut incomplete_reason: Option<String> = None;

        // Determine starting nodes
        let start_nodes: Vec<NodeId> = if let Some(entry) = graph.entry {
            vec![entry]
        } else {
            // Start from all nodes that have no predecessors (entry points)
            let entries: Vec<NodeId> = graph
                .node_ids()
                .filter(|&id| graph.predecessors_of(id).is_none_or(|p| p.is_empty()))
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

        // State-ful DFS: memoize on (node, live_resource_set, released_resource_set).
        //
        // This is the fix for false-negative (a): the old simple-path DFS
        // (one visit per node per path) missed cross-iteration UAF, where a
        // resource is freed in one loop iteration and accessed in the next.
        // By keying on the resource state, we allow revisiting a node when
        // the state has changed (e.g. a resource was freed), which is
        // necessary to catch the UAF on the second iteration. The state
        // space is bounded by `MAX_STATES` to ensure termination.
        let mut visited_states: BTreeSet<StateKey> = BTreeSet::new();

        for start in &start_nodes {
            let initial_state = PathState::default();
            self.dfs_verify(
                graph,
                *start,
                initial_state,
                &mut all_violations,
                &mut paths_explored,
                &mut states_explored,
                &mut visited_states,
                &mut incomplete,
                &mut incomplete_reason,
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

        let mut report =
            CleanupReport::from_violations(violations, paths_explored, acquires_checked);
        report.incomplete = incomplete;
        report.incomplete_reason = incomplete_reason;
        report
    }

    /// Recursive state-ful DFS that explores all paths from `current`,
    /// accumulating violations and tracking resource state.
    ///
    /// Memoization key: [`StateKey`] = `(node, live_resource_set,
    /// released_resource_set)`. If we reach the same node with the same
    /// resource state, we skip (already explored). If we reach it with a
    /// DIFFERENT state (e.g. a resource was freed in a loop iteration), we
    /// explore again — this is what catches cross-iteration UAF (fix for
    /// false-negative (a)).
    ///
    /// Two termination safeguards:
    /// - `max_path_length`: caps the length of a single exploration path
    ///   (relevant when the state keeps changing on cycles).
    /// - `MAX_STATES`: caps the total number of distinct states explored.
    ///
    /// When either limit is hit, `incomplete` is set to `true` and the
    /// caller ([`verify`](Self::verify)) will report
    /// [`VerificationStatus::Unverified`] (fix for false-negative (b):
    /// the old verifier silently returned `Proven`).
    fn dfs_verify(
        &self,
        graph: &CleanupGraph,
        current: NodeId,
        mut state: PathState,
        violations: &mut Vec<CleanupViolation>,
        paths_explored: &mut usize,
        states_explored: &mut usize,
        visited_states: &mut BTreeSet<StateKey>,
        incomplete: &mut bool,
        incomplete_reason: &mut Option<String>,
    ) {
        // Path length guard — prevents unbounded path growth on cycles
        // that keep changing state. Fix for false-negative (b): mark
        // verification incomplete instead of silently returning `Proven`.
        if state.path_nodes.len() >= self.max_path_length {
            if !*incomplete {
                *incomplete = true;
                *incomplete_reason = Some(format!(
                    "path length limit ({} nodes) reached at {}; verification incomplete",
                    self.max_path_length, current
                ));
            }
            return;
        }

        // State-space cap — ensures termination even if the state space
        // is large. Also marks verification incomplete.
        if *states_explored >= MAX_STATES {
            if !*incomplete {
                *incomplete = true;
                *incomplete_reason = Some(format!(
                    "state exploration cap ({}) reached; verification incomplete",
                    MAX_STATES
                ));
            }
            return;
        }

        // State-ful memoization: compute the state-on-arrival key.
        // Fix for false-negative (a): a node CAN be revisited if the
        // resource state has changed (e.g. a resource was freed in a
        // loop iteration). Only skip if we've seen this exact
        // (node, live, released) triple before.
        let live_set: BTreeSet<ResourceId> = state.live_resources.keys().copied().collect();
        let released_set: BTreeSet<ResourceId> = state.released_resources.keys().copied().collect();
        let state_key: StateKey = (current, live_set, released_set);
        if !visited_states.insert(state_key) {
            // Already explored this (node, state) — skip.
            return;
        }
        *states_explored += 1;

        // Process the current node.
        let node = match graph.get_node(current) {
            Some(n) => n,
            None => return,
        };
        let node_violations = state.process_node(node);
        violations.extend(node_violations);

        // Get successors.
        let succs: Vec<NodeId> = graph
            .successors_of(current)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();

        if succs.is_empty() {
            // Terminal node — check for leaks.
            *paths_explored += 1;
            let leak_violations = state.check_leaks(current);
            violations.extend(leak_violations);
        } else {
            // Explore each successor.
            for succ in succs {
                self.dfs_verify(
                    graph,
                    succ,
                    state.clone(),
                    violations,
                    paths_explored,
                    states_explored,
                    visited_states,
                    incomplete,
                    incomplete_reason,
                );
            }
        }
    }

    /// Quick check: for each acquire node, does a matching release node OR
    /// an explicit-leak node exist that is reachable from it? This is a fast
    /// O(V+E) per acquire reachability check, but doesn't account for
    /// conditional paths.
    ///
    /// A reachable [`OperationKind::Leak`] node satisfies the cleanup
    /// invariant per the spec's "freed or explicitly leaked" clause
    /// (Invariant 5, Part A), so it is treated as a valid cleanup endpoint
    /// here — symmetric with [`PathState::check_leaks`] exempting
    /// leaked resources from the leak violation.
    ///
    /// Returns a list of (acquire_node, resource) pairs where neither a
    /// release nor a leak annotation is reachable.
    pub fn quick_check_reachability(&self, graph: &CleanupGraph) -> Vec<(NodeId, ResourceId)> {
        let mut unreachable: Vec<(NodeId, ResourceId)> = Vec::new();

        for acquire_id in graph.acquire_nodes() {
            if let Some(node) = graph.get_node(acquire_id) {
                if let OperationKind::Acquire { resource, .. } = &node.operation {
                    // A resource is satisfactorily cleaned up if EITHER a
                    // release OR an explicit-leak annotation is reachable
                    // from the acquire point (spec Invariant 5, Part A:
                    // "freed or explicitly leaked").
                    let release_ids = graph.release_nodes_for(*resource);
                    let leak_ids = graph.leak_nodes_for(*resource);
                    let any_cleanup_reachable = release_ids
                        .iter()
                        .chain(leak_ids.iter())
                        .any(|&id| graph.has_path(acquire_id, id));

                    if !any_cleanup_reachable {
                        unreachable.push((acquire_id, *resource));
                    }
                }
            }
        }

        unreachable
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

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let access = graph.add_node(OperationKind::Access { resource: res }, pp("test.vu", 3));
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

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let access = graph.add_node(OperationKind::Access { resource: res }, pp("test.vu", 3));
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
        let access = graph.add_node(OperationKind::Access { resource: res }, pp("test.vu", 4));
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

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
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
        let access_then = graph.add_node(OperationKind::Access { resource: res }, pp("test.vu", 4));
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
        // With state-ful DFS (W4), both branches converge to the same
        // (live=∅, released={res}) state at the `join` node, so the
        // second branch's terminal is deduplicated against the first.
        // `paths_explored` counts distinct terminal states reached, not
        // distinct simple paths — the verification is still complete.
        assert_eq!(report.paths_explored, 1);
    }

    // -----------------------------------------------------------------------
    // Test 6: Conditional cleanup — one branch leaks
    // -----------------------------------------------------------------------
    #[test]
    fn test_conditional_cleanup_one_branch_leaks() {
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
        let passthrough_else = graph.add_node(OperationKind::Passthrough, pp("test.vu", 5));
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

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let access = graph.add_node(OperationKind::Access { resource: res }, pp("test.vu", 3));
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
        assert!(
            report.clean,
            "Expected clean (both paths free), got: {:?}",
            report.violations
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: Error path with leak — resource NOT freed on error path
    // -----------------------------------------------------------------------
    #[test]
    fn test_error_path_leak() {
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

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
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
        let access = graph.add_node(OperationKind::Access { resource: res1 }, pp("test.vu", 4));
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

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
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

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
        let open = graph.add_node(
            OperationKind::Acquire {
                resource: fh,
                kind: ResourceKind::FileHandle,
            },
            pp("test.vu", 2),
        );
        let access = graph.add_node(OperationKind::Access { resource: fh }, pp("test.vu", 3));
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

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
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

        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
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
        let access_after_free =
            graph.add_node(OperationKind::Access { resource: res }, pp("test.vu", 5));
        let ret_a = graph.add_node(OperationKind::Return, pp("test.vu", 6));
        // Branch B: normal use then free
        let access_b = graph.add_node(OperationKind::Access { resource: res }, pp("test.vu", 7));
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

    // -----------------------------------------------------------------------
    // Test 18: Explicit leak annotation — allocated but marked Leaked.
    // Spec Invariant 5, Part A: "freed or explicitly leaked". A region
    // marked `RegionStatus::Leaked` (modeled here by `OperationKind::Leak`)
    // must NOT be flagged as `ViolationKind::Leak`.
    // -----------------------------------------------------------------------
    #[test]
    fn test_explicit_leak_is_clean() {
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
        let access = graph.add_node(OperationKind::Access { resource: res }, pp("test.vu", 3));
        // Explicit leak annotation — programmer says "intentionally not freed"
        // (e.g. global arena, process-lifetime mapping).
        let leak = graph.add_node(
            OperationKind::Leak {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 4),
        );
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 5));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, access).unwrap();
        graph.add_edge(access, leak).unwrap();
        graph.add_edge(leak, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            report.clean,
            "Explicit leak annotation should exempt the resource from the leak violation, got: {:?}",
            report.violations
        );
        assert_eq!(report.paths_explored, 1);
        // No Leak violation for `res` specifically.
        assert!(!report
            .violations
            .iter()
            .any(|v| v.kind == ViolationKind::Leak && v.resource == res));
    }

    // -----------------------------------------------------------------------
    // Test 19: Conditional — one branch explicitly leaks (clean), the other
    // neither frees nor leaks (real leak). Only the real-leak branch should
    // be flagged. Confirms that the explicit-leak exemption is path-local
    // and does NOT silence genuine leaks on other paths.
    // -----------------------------------------------------------------------
    #[test]
    fn test_conditional_one_branch_explicit_leak_other_real_leak() {
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
        let branch = graph.add_node(
            OperationKind::Branch {
                condition: "cond".into(),
            },
            pp("test.vu", 3),
        );
        // Then branch — explicitly leaks (intentional, no violation)
        let leak_then = graph.add_node(
            OperationKind::Leak {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 4),
        );
        // Else branch — neither freed nor leaked (real leak!)
        let passthrough_else = graph.add_node(OperationKind::Passthrough, pp("test.vu", 5));
        let join = graph.add_node(OperationKind::Join, pp("test.vu", 6));
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 7));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, branch).unwrap();
        graph.add_edge(branch, leak_then).unwrap();
        graph.add_edge(branch, passthrough_else).unwrap();
        graph.add_edge(leak_then, join).unwrap();
        graph.add_edge(passthrough_else, join).unwrap();
        graph.add_edge(join, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            !report.clean,
            "Real leak on the else branch must still be flagged"
        );
        // Exactly one Leak violation for `res` (from the else branch only).
        let leak_violations: Vec<_> = report
            .violations
            .iter()
            .filter(|v| v.kind == ViolationKind::Leak && v.resource == res)
            .collect();
        assert_eq!(
            leak_violations.len(),
            1,
            "Expected exactly one Leak violation (from the real-leak branch), got {:?}",
            leak_violations
        );
    }

    // -----------------------------------------------------------------------
    // Test 20: Quick reachability accepts explicit-leak annotation as a
    // valid cleanup endpoint (symmetric with the path-sensitive verifier).
    // -----------------------------------------------------------------------
    #[test]
    fn test_quick_reachability_accepts_explicit_leak() {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 1),
        );
        let leak = graph.add_node(
            OperationKind::Leak {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        graph.add_edge(alloc, leak).unwrap();

        let verifier = CleanupVerifier::new();
        let unreachable = verifier.quick_check_reachability(&graph);
        assert!(
            unreachable.is_empty(),
            "Explicit-leak annotation should count as a reachable cleanup endpoint, got: {:?}",
            unreachable
        );

        // Sanity: a graph with NO release and NO leak for the resource
        // is still flagged as unreachable by the quick check.
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
    // Test 21: Leak node helpers on `CleanupGraph` (`leak_nodes`,
    // `leak_nodes_for`) and `OperationKind::is_leak` / `resource`.
    // -----------------------------------------------------------------------
    #[test]
    fn test_leak_node_helpers() {
        let mut graph = CleanupGraph::new();
        let res1 = ResourceId(1);
        let res2 = ResourceId(2);

        let alloc1 = graph.add_node(
            OperationKind::Acquire {
                resource: res1,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 1),
        );
        let leak1 = graph.add_node(
            OperationKind::Leak {
                resource: res1,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        let alloc2 = graph.add_node(
            OperationKind::Acquire {
                resource: res2,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 3),
        );
        // res2 is freed, not leaked.
        let free2 = graph.add_node(
            OperationKind::Release {
                resource: res2,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 4),
        );
        graph.add_edge(alloc1, leak1).unwrap();
        graph.add_edge(alloc2, free2).unwrap();

        // `leak_nodes` returns exactly the one Leak node.
        let leaks = graph.leak_nodes();
        assert_eq!(leaks.len(), 1);
        assert_eq!(leaks[0], leak1);

        // `leak_nodes_for(res1)` returns the leak; for res2 returns empty.
        assert_eq!(graph.leak_nodes_for(res1), vec![leak1]);
        assert!(graph.leak_nodes_for(res2).is_empty());

        // `OperationKind::is_leak` and `resource()` on the Leak variant.
        let leak_node = graph.get_node(leak1).unwrap();
        assert!(leak_node.operation.is_leak());
        assert!(!leak_node.operation.is_release());
        assert!(!leak_node.operation.is_acquire());
        assert_eq!(leak_node.operation.resource(), Some(res1));
    }

    // -----------------------------------------------------------------------
    // Test 22: Cross-iteration use-after-free (W4 fix for false-negative (a)).
    //
    // A loop where one branch frees the resource and then loops back
    // (a `continue`-style back-edge), and the other branch accesses the
    // resource unconditionally. The old simple-path DFS missed this
    // because it never explored the second iteration: the loop header
    // was already "visited" on the first iteration, so the back-edge was
    // skipped, and the `access` node was only ever seen with the
    // pre-free state (live={res}, released=∅) — never with the post-free
    // state (live=∅, released={res}) that would trigger the UAF.
    //
    // The state-ful DFS (W4) catches it: after the `free` branch, the
    // loop header is revisited with a DIFFERENT (live=∅, released={res})
    // state, so it is re-explored, and the `access` on the next iteration
    // is then flagged as use-after-free.
    //
    //     acquire(r)
    //     loop:
    //       if cond:
    //         free(r)     // cond=true: free, back-edge to loop
    //       else:
    //         access(r)   // cond=false: access, back-edge to loop
    //
    // Iteration 1 cond=true frees r; iteration 2 cond=false accesses r → UAF.
    // -----------------------------------------------------------------------
    #[test]
    fn test_cross_iteration_uaf_detected() {
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
        let loop_header = graph.add_node(
            OperationKind::Branch {
                condition: "cond".into(),
            },
            pp("test.vu", 3),
        );
        // cond=true branch: free, then back-edge to loop header.
        let free = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 4),
        );
        // cond=false branch: access, then back-edge to loop header.
        let access = graph.add_node(OperationKind::Access { resource: res }, pp("test.vu", 5));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, loop_header).unwrap();
        // cond=true: loop_header → free → loop_header (back-edge)
        graph.add_edge(loop_header, free).unwrap();
        graph.add_edge(free, loop_header).unwrap();
        // cond=false: loop_header → access → loop_header (back-edge)
        graph.add_edge(loop_header, access).unwrap();
        graph.add_edge(access, loop_header).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        assert!(
            !report.clean,
            "Expected cross-iteration UAF to be detected, got: {:?}",
            report.violations
        );
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UseAfterFree && v.resource == res),
            "Expected UseAfterFree for {}, got: {:?}",
            res,
            report.violations
        );
    }

    // -----------------------------------------------------------------------
    // Test 23: Path length limit returns Unverified (W4 fix for false-negative (b)).
    //
    // A linear chain of >256 nodes exceeds the default `max_path_length`.
    // The old verifier silently bailed at the limit and returned `Proven`
    // (no violation recorded, no incompleteness surfaced), accepting any
    // leaks/UAF beyond node 256 as "verified safe". The W4 fix marks the
    // report `incomplete` and `to_verification_result()` returns
    // `VerificationStatus::Unverified` instead of `Proven`, so the
    // aggregator treats the result as inconclusive (not pass, not fail).
    // -----------------------------------------------------------------------
    #[test]
    fn test_path_length_limit_returns_unverified() {
        let mut graph = CleanupGraph::new();
        let res = ResourceId(1);

        // Build a chain: entry → alloc → 300 passthroughs → free → ret.
        // Total chain length (305) exceeds the default max_path_length (256),
        // so the verifier cannot reach the free/return at the end.
        let entry = graph.add_node(OperationKind::Passthrough, pp("test.vu", 1));
        let alloc = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 2),
        );
        graph.add_edge(entry, alloc).unwrap();

        let mut prev = alloc;
        for i in 0u32..300 {
            let node = graph.add_node(OperationKind::Passthrough, pp("test.vu", 3 + i));
            graph.add_edge(prev, node).unwrap();
            prev = node;
        }
        let free = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 303),
        );
        graph.add_edge(prev, free).unwrap();
        let ret = graph.add_node(OperationKind::Return, pp("test.vu", 304));
        graph.add_edge(free, ret).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new(); // default max_path_length = 256
        let report = verifier.verify(&graph);

        // The verifier should have hit the path length limit.
        assert!(
            report.incomplete,
            "Expected incomplete=true due to path length limit, got: {:?}",
            report
        );
        // No violations found (exploration stopped before the free node),
        // but the invariant is NOT proven.
        assert!(
            report.clean,
            "Expected no violations (exploration stopped before the free), got: {:?}",
            report.violations
        );
        assert!(
            report.incomplete_reason.is_some(),
            "Expected an incomplete_reason, got: {:?}",
            report.incomplete_reason
        );
        // The verification result should be Unverified, not Proven.
        let result = report.to_verification_result();
        assert!(
            matches!(result.status, VerificationStatus::Unverified { .. }),
            "Expected Unverified status, got: {:?}",
            result.status
        );
        assert!(
            !result.is_proven(),
            "Should not be Proven when verification is incomplete"
        );
        assert!(
            !result.is_violated(),
            "Should not be Violated when no violations were found"
        );
    }

    // -----------------------------------------------------------------------
    // Test 24: Cross-iteration UAF is NOT a false positive.
    //
    // The mirror of test 22: the same loop structure, but the `access`
    // branch re-acquires the resource before accessing it. This must NOT
    // be flagged as UAF, confirming that the state-ful DFS doesn't
    // over-report when the resource state legitimately resets.
    //
    // (Note: this graph may also produce `DoubleFree` violations because
    // the `free` branch is inside a loop and the verifier's
    // `release_count` is global — a pre-existing limitation unrelated to
    // the W4 fix. This test only asserts the absence of UAF.)
    // -----------------------------------------------------------------------
    #[test]
    fn test_cross_iteration_reacquire_no_false_positive() {
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
        let loop_header = graph.add_node(
            OperationKind::Branch {
                condition: "cond".into(),
            },
            pp("test.vu", 3),
        );
        // cond=true branch: free, then back-edge.
        let free = graph.add_node(
            OperationKind::Release {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 4),
        );
        // cond=false branch: re-acquire, access, then back-edge.
        let reacquire = graph.add_node(
            OperationKind::Acquire {
                resource: res,
                kind: ResourceKind::Memory,
            },
            pp("test.vu", 5),
        );
        let access = graph.add_node(OperationKind::Access { resource: res }, pp("test.vu", 6));

        graph.add_edge(entry, alloc).unwrap();
        graph.add_edge(alloc, loop_header).unwrap();
        graph.add_edge(loop_header, free).unwrap();
        graph.add_edge(free, loop_header).unwrap();
        graph.add_edge(loop_header, reacquire).unwrap();
        graph.add_edge(reacquire, access).unwrap();
        graph.add_edge(access, loop_header).unwrap();
        graph.set_entry(entry).unwrap();

        let verifier = CleanupVerifier::new();
        let report = verifier.verify(&graph);
        // No UAF: the `access` after `reacquire` always has res live.
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::UseAfterFree && v.resource == res),
            "Expected NO UseAfterFree (resource is re-acquired before access), got: {:?}",
            report.violations
        );
    }
}
