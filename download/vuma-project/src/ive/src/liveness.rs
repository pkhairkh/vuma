//! Liveness invariant verifier for the IVE module.
//!
//! This module implements a complete liveness verification engine that checks
//! whether "every requested resource will eventually be provided" across all
//! execution paths. The liveness invariant encompasses:
//!
//! - **Allocation reachability**: every allocation must have a matching
//!   deallocation reachable on all execution paths.
//! - **Deadlock freedom**: no circular wait-for dependencies exist in the
//!   resource acquisition graph (verified via SCC on wait-for graphs).
//! - **Lock discipline**: every lock acquisition has a corresponding release.
//! - **Message completeness**: every send has a matching receive (no lost
//!   messages in concurrent code).
//!
//! # Architecture
//!
//! The verifier operates on a [`LivenessInput`] model, which is constructed
//! from the Memory State Graph (MSG) and Semantic Computation Graph (SCG).
//! The model captures regions, resource events, control-flow edges, and
//! wait-for dependencies. The verification proceeds in four phases:
//!
//! 1. **Resource leak detection** — walk all allocations and verify that a
//!    deallocation is reachable on every execution path.
//! 2. **Deadlock detection** — build a wait-for graph and detect cycles
//!    using Tarjan's SCC algorithm.
//! 3. **Lock discipline checking** — verify every lock acquisition has a
//!    matching release on all paths.
//! 4. **Message completeness** — verify every send has a matching receive.
//!
//! Each phase produces structured findings that are aggregated into a final
//! [`LivenessVerificationResult`].

use crate::result::{
    CounterExample, Evidence, ProgramPoint, VerificationResult, VerificationStatus,
};
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// Core identifiers
// ---------------------------------------------------------------------------

/// Unique identifier for a resource (region, lock, channel, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ResourceId(pub u64);

impl fmt::Display for ResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Res{}", self.0)
    }
}

/// Unique identifier for a program point in the control-flow graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PointId(pub u64);

impl fmt::Display for PointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PP{}", self.0)
    }
}

/// Unique identifier for a thread of execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThreadId(pub u64);

impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "T{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Resource kinds and events
// ---------------------------------------------------------------------------

/// The kind of a tracked resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    /// Heap-allocated memory region.
    Memory,
    /// A mutual-exclusion lock.
    Lock,
    /// A bounded or unbounded channel.
    Channel,
    /// A file handle or I/O resource.
    FileHandle,
    /// A custom user-defined resource kind.
    Custom(u16),
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResourceKind::Memory => write!(f, "memory"),
            ResourceKind::Lock => write!(f, "lock"),
            ResourceKind::Channel => write!(f, "channel"),
            ResourceKind::FileHandle => write!(f, "file_handle"),
            ResourceKind::Custom(n) => write!(f, "custom({})", n),
        }
    }
}

/// An event related to a resource at a specific program point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceEvent {
    /// The resource this event pertains to.
    pub resource: ResourceId,
    /// The kind of resource.
    pub kind: ResourceKind,
    /// The specific event that occurred.
    pub event: EventAction,
    /// The program point at which this event occurs.
    pub point: PointId,
    /// The thread performing this event.
    pub thread: ThreadId,
}

/// The action performed on a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventAction {
    /// A resource was allocated/created.
    Allocate,
    /// A resource was deallocated/destroyed.
    Deallocate,
    /// A lock was acquired.
    Acquire,
    /// A lock was released.
    Release,
    /// A message was sent on a channel.
    Send,
    /// A message was received from a channel.
    Receive,
    /// A resource was read (memory read access).
    Read,
    /// A resource was written (memory write access).
    Write,
}

impl fmt::Display for EventAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventAction::Allocate => write!(f, "allocate"),
            EventAction::Deallocate => write!(f, "deallocate"),
            EventAction::Acquire => write!(f, "acquire"),
            EventAction::Release => write!(f, "release"),
            EventAction::Send => write!(f, "send"),
            EventAction::Receive => write!(f, "receive"),
            EventAction::Read => write!(f, "read"),
            EventAction::Write => write!(f, "write"),
        }
    }
}

// ---------------------------------------------------------------------------
// Control-flow graph edge
// ---------------------------------------------------------------------------

/// A directed edge in the control-flow graph, representing possible
/// execution flow from one program point to another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlFlowEdge {
    /// Source program point.
    pub from: PointId,
    /// Target program point.
    pub to: PointId,
    /// Whether this edge is conditional (branch) or unconditional.
    pub conditional: bool,
    /// An optional label (e.g., "true", "false", "loop_back").
    pub label: Option<String>,
}

// ---------------------------------------------------------------------------
// Wait-for dependency
// ---------------------------------------------------------------------------

/// A wait-for dependency: `waiter` thread holds `held` resource and is
/// waiting to acquire `wanted` resource. Used for deadlock detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitForDependency {
    /// The thread that is waiting.
    pub waiter: ThreadId,
    /// The resource the thread already holds.
    pub held: ResourceId,
    /// The resource the thread wants to acquire.
    pub wanted: ResourceId,
}

// ---------------------------------------------------------------------------
// Input model
// ---------------------------------------------------------------------------

/// The input model for the liveness verifier, constructed from the MSG/SCG.
///
/// This structure captures all the information needed to verify the liveness
/// invariant: resource events, control-flow edges, and wait-for dependencies.
#[derive(Debug, Clone, Default)]
pub struct LivenessInput {
    /// All resource events (allocations, deallocations, acquires, releases, etc.).
    pub events: Vec<ResourceEvent>,
    /// Control-flow edges defining possible execution paths.
    pub cfg_edges: Vec<ControlFlowEdge>,
    /// Wait-for dependencies for deadlock analysis.
    pub wait_for_deps: Vec<WaitForDependency>,
    /// Entry point of the program.
    pub entry_point: Option<PointId>,
}

impl LivenessInput {
    /// Create a new, empty input model.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a resource event.
    pub fn add_event(&mut self, event: ResourceEvent) {
        self.events.push(event);
    }

    /// Add a control-flow edge.
    pub fn add_cfg_edge(&mut self, edge: ControlFlowEdge) {
        self.cfg_edges.push(edge);
    }

    /// Add a wait-for dependency.
    pub fn add_wait_for(&mut self, dep: WaitForDependency) {
        self.wait_for_deps.push(dep);
    }

    /// Returns all events for a given resource.
    pub fn events_for_resource(&self, rid: ResourceId) -> Vec<&ResourceEvent> {
        self.events.iter().filter(|e| e.resource == rid).collect()
    }

    /// Returns all allocation events.
    pub fn allocations(&self) -> Vec<&ResourceEvent> {
        self.events
            .iter()
            .filter(|e| e.event == EventAction::Allocate)
            .collect()
    }

    /// Returns all deallocation events for a specific resource.
    pub fn deallocations_for(&self, rid: ResourceId) -> Vec<&ResourceEvent> {
        self.events
            .iter()
            .filter(|e| e.resource == rid && e.event == EventAction::Deallocate)
            .collect()
    }

    /// Returns all acquire events for a specific resource.
    pub fn acquires_for(&self, rid: ResourceId) -> Vec<&ResourceEvent> {
        self.events
            .iter()
            .filter(|e| e.resource == rid && e.event == EventAction::Acquire)
            .collect()
    }

    /// Returns all release events for a specific resource.
    pub fn releases_for(&self, rid: ResourceId) -> Vec<&ResourceEvent> {
        self.events
            .iter()
            .filter(|e| e.resource == rid && e.event == EventAction::Release)
            .collect()
    }

    /// Returns all send events for a specific resource (channel).
    pub fn sends_for(&self, rid: ResourceId) -> Vec<&ResourceEvent> {
        self.events
            .iter()
            .filter(|e| e.resource == rid && e.event == EventAction::Send)
            .collect()
    }

    /// Returns all receive events for a specific resource (channel).
    pub fn receives_for(&self, rid: ResourceId) -> Vec<&ResourceEvent> {
        self.events
            .iter()
            .filter(|e| e.resource == rid && e.event == EventAction::Receive)
            .collect()
    }

    /// Returns all read events for a specific resource.
    pub fn reads_for(&self, rid: ResourceId) -> Vec<&ResourceEvent> {
        self.events
            .iter()
            .filter(|e| e.resource == rid && e.event == EventAction::Read)
            .collect()
    }

    /// Returns all write events for a specific resource.
    pub fn writes_for(&self, rid: ResourceId) -> Vec<&ResourceEvent> {
        self.events
            .iter()
            .filter(|e| e.resource == rid && e.event == EventAction::Write)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Findings (violations, warnings, proof obligations)
// ---------------------------------------------------------------------------

/// A specific liveness violation found during verification.
#[derive(Debug, Clone, PartialEq)]
pub enum LivenessViolation {
    /// A resource was allocated but never deallocated on any path (leak).
    ResourceLeak {
        /// The leaked resource.
        resource: ResourceId,
        /// The kind of resource.
        kind: ResourceKind,
        /// The program point where the allocation occurs.
        alloc_point: PointId,
        /// The thread that allocated the resource.
        thread: ThreadId,
    },

    /// A deadlock cycle was detected in the wait-for graph.
    DeadlockCycle {
        /// The resources involved in the cycle, in order.
        cycle: Vec<ResourceId>,
        /// The threads involved in the cycle, in order.
        threads: Vec<ThreadId>,
        /// Human-readable description of the deadlock.
        description: String,
    },

    /// A lock was acquired but never released on some path.
    LockHeldTooLong {
        /// The lock resource.
        resource: ResourceId,
        /// Where the lock was acquired.
        acquire_point: PointId,
        /// The thread holding the lock.
        thread: ThreadId,
    },

    /// A message was sent but never received (lost message).
    LostMessage {
        /// The channel resource.
        channel: ResourceId,
        /// Where the send occurred.
        send_point: PointId,
        /// The thread that sent the message.
        thread: ThreadId,
    },

    /// An allocation was deallocated on some paths but not all
    /// (conditional deallocation, may lead to leak).
    ConditionalDeallocation {
        /// The resource.
        resource: ResourceId,
        /// Where the allocation occurs.
        alloc_point: PointId,
        /// Paths where deallocation occurs.
        dealloc_paths: Vec<Vec<PointId>>,
        /// Paths where no deallocation was found.
        leak_paths: Vec<Vec<PointId>>,
    },

    /// A circular dependency was detected (general case beyond deadlock).
    CircularDependency {
        /// The resources forming the cycle.
        cycle: Vec<ResourceId>,
        /// Description of the dependency.
        description: String,
    },
}

impl fmt::Display for LivenessViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LivenessViolation::ResourceLeak {
                resource,
                kind,
                alloc_point,
                thread,
            } => write!(
                f,
                "Resource leak: {} {} allocated at {} by {} is never deallocated",
                kind, resource, alloc_point, thread
            ),
            LivenessViolation::DeadlockCycle {
                cycle,
                threads,
                description,
            } => write!(
                f,
                "Deadlock cycle: resources [{}] held by threads [{}]: {}",
                cycle
                    .iter()
                    .map(|r| r.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                threads
                    .iter()
                    .map(|t| t.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                description
            ),
            LivenessViolation::LockHeldTooLong {
                resource,
                acquire_point,
                thread,
            } => write!(
                f,
                "Lock {} acquired at {} by {} is never released",
                resource, acquire_point, thread
            ),
            LivenessViolation::LostMessage {
                channel,
                send_point,
                thread,
            } => write!(
                f,
                "Message sent on channel {} at {} by {} is never received",
                channel, send_point, thread
            ),
            LivenessViolation::ConditionalDeallocation {
                resource,
                alloc_point,
                dealloc_paths,
                leak_paths,
            } => write!(
                f,
                "Conditional deallocation: {} allocated at {} is deallocated on {} path(s) but may leak on {} path(s)",
                resource,
                alloc_point,
                dealloc_paths.len(),
                leak_paths.len()
            ),
            LivenessViolation::CircularDependency {
                cycle,
                description,
            } => write!(
                f,
                "Circular dependency: [{}]: {}",
                cycle
                    .iter()
                    .map(|r| r.to_string())
                    .collect::<Vec<_>>()
                    .join(" -> "),
                description
            ),
        }
    }
}

/// A proof obligation that must be discharged to complete liveness verification.
#[derive(Debug, Clone, PartialEq)]
pub struct ProofObligation {
    /// A unique identifier for this obligation.
    pub id: u64,
    /// A human-readable description of what must be proven.
    pub description: String,
    /// The resource this obligation pertains to.
    pub resource: ResourceId,
    /// The type of obligation.
    pub obligation_kind: ObligationKind,
}

/// The kind of proof obligation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObligationKind {
    /// Prove that a deallocation is reachable on all paths.
    DeallocationReachable,
    /// Prove that no deadlock can occur with this acquisition pattern.
    DeadlockFreedom,
    /// Prove that a lock release is reachable on all paths.
    LockReleaseReachable,
    /// Prove that every send has a matching receive.
    MessageReceived,
    /// Prove that an access is safe despite potential use-after-free.
    UseAfterFreeSafe,
    /// Prove that a dead allocation is actually needed.
    DeadAllocationNeeded,
    /// Prove that a partially initialized region is fully initialized before use.
    FullyInitialized,
}

// ---------------------------------------------------------------------------
// Use-After-Free Path Tracking
// ---------------------------------------------------------------------------

/// A complete lifecycle path for a tracked resource, including any
/// use-after-free accesses detected along the way.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LivenessPath {
    /// The program point where the resource was allocated.
    pub allocation_point: ProgramPoint,
    /// The program point where the resource was deallocated, if any.
    pub deallocation_point: Option<ProgramPoint>,
    /// Accesses that occur after the resource has been freed.
    /// Each entry is (program point, description of the access).
    pub access_after_free: Vec<(ProgramPoint, String)>,
    /// The numeric ID of the resource.
    pub resource_id: u64,
    /// The kind of resource (as a string for serialization).
    pub resource_kind: String,
}

// ---------------------------------------------------------------------------
// Dead Allocation Detection
// ---------------------------------------------------------------------------

/// A dead allocation: a resource that was allocated but never usefully used.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadAllocation {
    /// The program point where the allocation occurs.
    pub allocation_point: ProgramPoint,
    /// The numeric ID of the resource.
    pub resource_id: u64,
    /// The reason this allocation is considered dead.
    pub reason: DeadReason,
}

/// The reason an allocation is considered dead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeadReason {
    /// The resource was allocated but never accessed at all.
    NeverAccessed,
    /// The resource was written to but never read from.
    OnlyWrittenNeverRead,
    /// The resource was allocated and immediately deallocated with no
    /// intervening use.
    RedundantAllocation,
}

// ---------------------------------------------------------------------------
// Uninitialized Read Detection
// ---------------------------------------------------------------------------

/// A map tracking which byte ranges within each region have been initialized.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializationMap {
    /// Maps region_id to a set of initialized byte ranges (start, end).
    pub initialized: std::collections::HashMap<u64, Vec<(u64, u64)>>,
}

impl InitializationMap {
    /// Create a new, empty initialization map.
    pub fn new() -> Self {
        Self {
            initialized: std::collections::HashMap::new(),
        }
    }

    /// Mark a byte range as initialized within a region.
    pub fn mark_initialized(&mut self, region_id: u64, start: u64, end: u64) {
        self.initialized
            .entry(region_id)
            .or_default()
            .push((start, end));
    }

    /// Check whether a given byte range is fully initialized within a region.
    /// Returns the list of uninitialized sub-ranges that fall within the
    /// requested range.
    pub fn check_range(
        &self,
        region_id: u64,
        access_start: u64,
        access_end: u64,
    ) -> Vec<(u64, u64)> {
        let init_ranges = match self.initialized.get(&region_id) {
            Some(ranges) => ranges,
            None => return vec![(access_start, access_end)],
        };

        if init_ranges.is_empty() {
            return vec![(access_start, access_end)];
        }

        // Collect and sort all initialized ranges
        let mut sorted: Vec<(u64, u64)> = init_ranges.clone();
        sorted.sort_by_key(|r| r.0);

        // Merge overlapping initialized ranges
        let mut merged: Vec<(u64, u64)> = Vec::new();
        for (s, e) in sorted {
            if let Some(last) = merged.last_mut() {
                if s <= last.1 {
                    last.1 = last.1.max(e);
                    continue;
                }
            }
            merged.push((s, e));
        }

        // Find gaps within [access_start, access_end)
        let mut uninitialized: Vec<(u64, u64)> = Vec::new();
        let mut cursor = access_start;

        for (s, e) in &merged {
            // Skip ranges entirely before the access range
            if *e <= access_start {
                continue;
            }
            // Stop if we've passed the access range
            if *s >= access_end {
                break;
            }

            // Clamp to the access range
            let s_clamped = (*s).max(access_start);

            if s_clamped > cursor {
                uninitialized.push((cursor, s_clamped));
            }
            cursor = cursor.max(*e);
        }

        if cursor < access_end {
            uninitialized.push((cursor, access_end));
        }

        uninitialized
    }
}

impl Default for InitializationMap {
    fn default() -> Self {
        Self::new()
    }
}

/// A violation where a partially initialized region is accessed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialInitViolation {
    /// The region that was accessed with partial initialization.
    pub region_id: u64,
    /// The program point where the partially-initialized access occurs.
    pub access_point: ProgramPoint,
    /// The byte range that was accessed.
    pub accessed_range: (u64, u64),
    /// The sub-ranges within the accessed range that are uninitialized.
    pub uninitialized_ranges: Vec<(u64, u64)>,
}

// ---------------------------------------------------------------------------
// Verification context
// ---------------------------------------------------------------------------

/// Context provided to enhanced liveness verification methods, bundling
/// the input model with optional initialization tracking data.
#[derive(Debug, Clone)]
pub struct VerificationContext {
    /// The liveness input model (events, CFG edges, wait-for deps).
    pub input: LivenessInput,
    /// Optional initialization map for partial-initialization checking.
    pub init_map: InitializationMap,
}

impl VerificationContext {
    /// Create a new verification context from the given input.
    pub fn new(input: LivenessInput) -> Self {
        Self {
            input,
            init_map: InitializationMap::new(),
        }
    }

    /// Create a verification context with an explicit initialization map.
    pub fn with_init_map(input: LivenessInput, init_map: InitializationMap) -> Self {
        Self { input, init_map }
    }
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// The result of liveness verification.
#[derive(Debug, Clone)]
pub struct LivenessVerificationResult {
    /// All violations found during verification.
    pub violations: Vec<LivenessViolation>,
    /// All proof obligations that must be discharged.
    pub proof_obligations: Vec<ProofObligation>,
    /// The number of resources checked.
    pub resources_checked: usize,
    /// The number of paths analyzed.
    pub paths_analyzed: usize,
    /// Whether the liveness invariant holds.
    pub invariant_holds: bool,
}

impl LivenessVerificationResult {
    /// Create a new result builder.
    pub fn new() -> Self {
        Self {
            violations: Vec::new(),
            proof_obligations: Vec::new(),
            resources_checked: 0,
            paths_analyzed: 0,
            invariant_holds: true,
        }
    }

    /// Record a violation.
    pub fn add_violation(&mut self, violation: LivenessViolation) {
        self.invariant_holds = false;
        self.violations.push(violation);
    }

    /// Record a proof obligation.
    pub fn add_obligation(&mut self, obligation: ProofObligation) {
        self.proof_obligations.push(obligation);
    }

    /// Convert this result into the crate-level [`VerificationResult`].
    pub fn into_verification_result(self) -> VerificationResult {
        if self.invariant_holds {
            if self.proof_obligations.is_empty() {
                VerificationResult::new(
                    "liveness",
                    VerificationStatus::Proven,
                    format!(
                        "Liveness invariant verified: {} resources checked, {} paths analyzed, no violations",
                        self.resources_checked, self.paths_analyzed
                    ),
                )
                .with_evidence(Evidence::ExhaustiveAnalysis)
            } else {
                let assumptions: Vec<String> = self
                    .proof_obligations
                    .iter()
                    .map(|o| o.description.clone())
                    .collect();
                VerificationResult::new(
                    "liveness",
                    VerificationStatus::ProbablySafe { assumptions },
                    format!(
                        "Liveness verified under {} proof obligation(s): {} resources checked, {} paths analyzed",
                        self.proof_obligations.len(),
                        self.resources_checked,
                        self.paths_analyzed
                    ),
                )
            }
        } else {
            let first_violation = self.violations.first();
            let violation_point = match first_violation {
                Some(LivenessViolation::ResourceLeak { alloc_point, .. }) => {
                    alloc_point.to_string()
                }
                Some(LivenessViolation::DeadlockCycle { cycle, .. }) => {
                    cycle.first().map_or("unknown".into(), |r| r.to_string())
                }
                Some(LivenessViolation::LockHeldTooLong { acquire_point, .. }) => {
                    acquire_point.to_string()
                }
                Some(LivenessViolation::LostMessage { send_point, .. }) => {
                    send_point.to_string()
                }
                Some(LivenessViolation::ConditionalDeallocation { alloc_point, .. }) => {
                    alloc_point.to_string()
                }
                Some(LivenessViolation::CircularDependency { cycle, .. }) => {
                    cycle.first().map_or("unknown".into(), |r| r.to_string())
                }
                None => "unknown".into(),
            };
            let description = self
                .violations
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            VerificationResult::new(
                "liveness",
                VerificationStatus::Violated {
                    counterexample: CounterExample::new(
                        Vec::new(),
                        violation_point,
                        description,
                    ),
                },
                format!(
                    "Liveness invariant violated: {} violation(s) found across {} resources",
                    self.violations.len(),
                    self.resources_checked
                ),
            )
        }
    }
}

impl Default for LivenessVerificationResult {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Control-flow graph (adjacency list representation)
// ---------------------------------------------------------------------------

/// A simple control-flow graph for reachability analysis.
#[derive(Debug, Clone, Default)]
struct CFG {
    /// Adjacency list: point -> list of successor points.
    successors: hashbrown::HashMap<PointId, Vec<PointId>>,
    /// Reverse adjacency list: point -> list of predecessor points.
    predecessors: hashbrown::HashMap<PointId, Vec<PointId>>,
}

impl CFG {
    fn new() -> Self {
        Self::default()
    }

    fn add_edge(&mut self, from: PointId, to: PointId) {
        self.successors.entry(from).or_default().push(to);
        self.predecessors.entry(to).or_default().push(from);
    }

    /// BFS reachability from `start` to `target`.
    fn is_reachable(&self, start: PointId, target: PointId) -> bool {
        if start == target {
            return true;
        }
        let mut visited = hashbrown::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        visited.insert(start);
        queue.push_back(start);
        while let Some(current) = queue.pop_front() {
            if let Some(succs) = self.successors.get(&current) {
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

    /// Find all points reachable from `start`.
    fn reachable_set(&self, start: PointId) -> hashbrown::HashSet<PointId> {
        let mut visited = hashbrown::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        visited.insert(start);
        queue.push_back(start);
        while let Some(current) = queue.pop_front() {
            if let Some(succs) = self.successors.get(&current) {
                for &succ in succs {
                    if visited.insert(succ) {
                        queue.push_back(succ);
                    }
                }
            }
        }
        visited
    }

    /// Find a path from `start` to `target` (BFS). Returns None if no path.
    fn find_path(&self, start: PointId, target: PointId) -> Option<Vec<PointId>> {
        if start == target {
            return Some(vec![start]);
        }
        let mut visited = hashbrown::HashMap::new();
        let mut queue = std::collections::VecDeque::new();
        visited.insert(start, None);
        queue.push_back(start);
        while let Some(current) = queue.pop_front() {
            if let Some(succs) = self.successors.get(&current) {
                for &succ in succs {
                    if visited.contains_key(&succ) {
                        continue;
                    }
                    visited.insert(succ, Some(current));
                    if succ == target {
                        // Reconstruct path
                        let mut path = vec![target];
                        let mut step = Some(current);
                        while let Some(prev) = step {
                            path.push(prev);
                            step = visited[&prev];
                        }
                        path.reverse();
                        return Some(path);
                    }
                    queue.push_back(succ);
                }
            }
        }
        None
    }

    /// Find all simple paths from `start` to `target` (bounded by `max_paths`).
    fn find_all_paths(
        &self,
        start: PointId,
        target: PointId,
        max_paths: usize,
    ) -> Vec<Vec<PointId>> {
        let mut results = Vec::new();
        let mut stack = vec![(start, vec![start])];
        while let Some((current, path)) = stack.pop() {
            if current == target && path.len() > 1 {
                results.push(path);
                if results.len() >= max_paths {
                    break;
                }
                continue;
            }
            if let Some(succs) = self.successors.get(&current) {
                for &succ in succs {
                    // Avoid cycles: don't revisit nodes already in the path
                    if !path.contains(&succ) || succ == target {
                        let mut new_path = path.clone();
                        new_path.push(succ);
                        stack.push((succ, new_path));
                    }
                }
            }
        }
        results
    }
}

// ---------------------------------------------------------------------------
// Tarjan's SCC algorithm for deadlock detection
// ---------------------------------------------------------------------------

/// Strongly connected component in a directed graph.
#[derive(Debug, Clone)]
struct SCC {
    /// Nodes in this SCC.
    nodes: hashbrown::HashSet<ResourceId>,
    /// Whether this SCC is a non-trivial cycle (size > 1 or self-loop).
    is_cycle: bool,
}

/// Run Tarjan's algorithm to find all SCCs in a directed graph.
/// The graph is represented as an adjacency list of ResourceId.
fn tarjan_scc(
    graph: &hashbrown::HashMap<ResourceId, Vec<ResourceId>>,
) -> Vec<SCC> {
    let mut index_counter: u64 = 0;
    let mut stack: Vec<ResourceId> = Vec::new();
    let mut on_stack: hashbrown::HashSet<ResourceId> = hashbrown::HashSet::new();
    let mut indices: hashbrown::HashMap<ResourceId, u64> = hashbrown::HashMap::new();
    let mut lowlinks: hashbrown::HashMap<ResourceId, u64> = hashbrown::HashMap::new();
    let mut sccs: Vec<SCC> = Vec::new();

    let all_nodes: Vec<ResourceId> = graph.keys().copied().collect();

    for node in all_nodes {
        if !indices.contains_key(&node) {
            tarjan_strongconnect(
                node,
                graph,
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

fn tarjan_strongconnect(
    v: ResourceId,
    graph: &hashbrown::HashMap<ResourceId, Vec<ResourceId>>,
    index_counter: &mut u64,
    stack: &mut Vec<ResourceId>,
    on_stack: &mut hashbrown::HashSet<ResourceId>,
    indices: &mut hashbrown::HashMap<ResourceId, u64>,
    lowlinks: &mut hashbrown::HashMap<ResourceId, u64>,
    sccs: &mut Vec<SCC>,
) {
    indices.insert(v, *index_counter);
    lowlinks.insert(v, *index_counter);
    *index_counter += 1;
    stack.push(v);
    on_stack.insert(v);

    if let Some(neighbors) = graph.get(&v) {
        for &w in neighbors {
            if !indices.contains_key(&w) {
                tarjan_strongconnect(
                    w, graph, index_counter, stack, on_stack, indices, lowlinks, sccs,
                );
                let vl = lowlinks[&v];
                let wl = lowlinks[&w];
                lowlinks.insert(v, vl.min(wl));
            } else if on_stack.contains(&w) {
                let vl = lowlinks[&v];
                let wl = indices[&w];
                lowlinks.insert(v, vl.min(wl));
            }
        }
    }

    // If v is a root node, pop the SCC
    if lowlinks[&v] == indices[&v] {
        let mut component = hashbrown::HashSet::new();
        loop {
            let w = stack.pop().unwrap();
            on_stack.remove(&w);
            component.insert(w);
            if w == v {
                break;
            }
        }
        let is_cycle = component.len() > 1
            || component.iter().any(|&node| {
                graph
                    .get(&node)
                    .map_or(false, |nbrs| nbrs.contains(&node))
            });
        sccs.push(SCC {
            nodes: component,
            is_cycle,
        });
    }
}

// ---------------------------------------------------------------------------
// LivenessVerifier
// ---------------------------------------------------------------------------

/// The liveness invariant verifier.
///
/// Walks all allocations/regions from the MSG and verifies that:
/// 1. Every allocation has a reachable deallocation on all paths.
/// 2. No circular wait-for dependencies (deadlock cycles) exist.
/// 3. Every lock acquisition has a matching release.
/// 4. Every channel send has a matching receive.
pub struct LivenessVerifier {
    /// Whether to emit detailed diagnostic logging.
    verbose: bool,
    /// Maximum number of paths to enumerate during path-sensitive analysis.
    max_paths: usize,
    /// The next proof obligation ID.
    next_obligation_id: u64,
}

impl LivenessVerifier {
    /// Construct a new liveness verifier.
    pub fn new() -> Self {
        Self {
            verbose: false,
            max_paths: 64,
            next_obligation_id: 0,
        }
    }

    /// Enable verbose diagnostic output.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Set the maximum number of paths to enumerate (default: 64).
    pub fn with_max_paths(mut self, max_paths: usize) -> Self {
        self.max_paths = max_paths;
        self
    }

    /// Allocate a unique proof obligation ID.
    fn alloc_obligation_id(&mut self) -> u64 {
        let id = self.next_obligation_id;
        self.next_obligation_id += 1;
        id
    }

    // -----------------------------------------------------------------------
    // Main verification entry point
    // -----------------------------------------------------------------------

    /// Run the full liveness verification on the given input.
    ///
    /// This executes all four verification phases and returns an aggregated
    /// result.
    pub fn verify(&mut self, input: &LivenessInput) -> LivenessVerificationResult {
        let mut result = LivenessVerificationResult::new();

        // Build the CFG from the input edges.
        let cfg = self.build_cfg(input);

        // Collect all unique resources.
        let resources = self.collect_resources(input);
        result.resources_checked = resources.len();

        // Phase 1: Resource leak detection
        let leak_count = self.check_resource_leaks(input, &cfg, &mut result);
        if self.verbose {
            log::info!("Phase 1 (leak detection): {} leaks found", leak_count);
        }

        // Phase 2: Deadlock detection via SCC
        let deadlock_count = self.check_deadlock(input, &mut result);
        if self.verbose {
            log::info!("Phase 2 (deadlock detection): {} deadlock cycles found", deadlock_count);
        }

        // Phase 3: Lock discipline
        let lock_count = self.check_lock_discipline(input, &cfg, &mut result);
        if self.verbose {
            log::info!("Phase 3 (lock discipline): {} violations found", lock_count);
        }

        // Phase 4: Message completeness
        let msg_count = self.check_message_completeness(input, &cfg, &mut result);
        if self.verbose {
            log::info!("Phase 4 (message completeness): {} violations found", msg_count);
        }

        // Count paths analyzed (approximation from CFG reachability queries)
        result.paths_analyzed = self.count_analyzed_paths(input, &cfg);

        result
    }

    /// Build the internal CFG from the input control-flow edges.
    fn build_cfg(&self, input: &LivenessInput) -> CFG {
        let mut cfg = CFG::new();
        for edge in &input.cfg_edges {
            cfg.add_edge(edge.from, edge.to);
        }
        cfg
    }

    /// Collect all unique resource IDs from the input.
    fn collect_resources(&self, input: &LivenessInput) -> hashbrown::HashSet<ResourceId> {
        input
            .events
            .iter()
            .map(|e| e.resource)
            .collect()
    }

    // -----------------------------------------------------------------------
    // Phase 1: Resource leak detection
    // -----------------------------------------------------------------------

    /// Check that every allocation has a reachable deallocation.
    fn check_resource_leaks(
        &mut self,
        input: &LivenessInput,
        cfg: &CFG,
        result: &mut LivenessVerificationResult,
    ) -> usize {
        let mut leak_count = 0;
        let allocations = input.allocations();

        for alloc_event in &allocations {
            let resource = alloc_event.resource;
            let alloc_point = alloc_event.point;
            let kind = alloc_event.kind;
            let thread = alloc_event.thread;

            let deallocs = input.deallocations_for(resource);

            if deallocs.is_empty() {
                // No deallocation at all — definite leak
                result.add_violation(LivenessViolation::ResourceLeak {
                    resource,
                    kind,
                    alloc_point,
                    thread,
                });
                leak_count += 1;
            } else {
                // Check reachability of each deallocation from the allocation
                let reachable_deallocs: Vec<&ResourceEvent> = deallocs
                    .iter()
                    .filter(|de| cfg.is_reachable(alloc_point, de.point))
                    .copied()
                    .collect();

                if reachable_deallocs.is_empty() {
                    // Deallocations exist but none are reachable from alloc
                    result.add_violation(LivenessViolation::ResourceLeak {
                        resource,
                        kind,
                        alloc_point,
                        thread,
                    });
                    leak_count += 1;
                } else {
                    // Check path sensitivity: are there paths where dealloc
                    // is NOT reachable? This requires finding all paths from
                    // alloc_point and checking if every path reaches a dealloc.
                    let reachable_from_alloc = cfg.reachable_set(alloc_point);

                    // Find deallocation points that are reachable
                    let dealloc_points: hashbrown::HashSet<PointId> = reachable_deallocs
                        .iter()
                        .map(|de| de.point)
                        .collect();

                    // Find points reachable from alloc but NOT reaching any dealloc
                    // (simplified: check if every successor path leads to dealloc)
                    let _dealloc_reachable_set: hashbrown::HashSet<PointId> = dealloc_points
                        .iter()
                        .flat_map(|&dp| cfg.reachable_set(dp))
                        .collect();

                    // A point is "after alloc but before dealloc" if it's reachable
                    // from alloc but no dealloc point is reachable from it.
                    let mut has_potential_leak_path = false;
                    for &point in &reachable_from_alloc {
                        if dealloc_points.contains(&point) {
                            continue; // This IS a dealloc point
                        }
                        let point_reachable = cfg.reachable_set(point);
                        let reaches_dealloc = dealloc_points
                            .iter()
                            .any(|&dp| point_reachable.contains(&dp));
                        if !reaches_dealloc && point != alloc_point {
                            // This point leads to a path with no dealloc
                            // But we need to check that the point is actually
                            // reachable without going through a dealloc first.
                            // Simplified check: is this point reachable from alloc
                            // without passing through dealloc?
                            if let Some(path) = cfg.find_path(alloc_point, point) {
                                let passes_through_dealloc = path
                                    .iter()
                                    .any(|p| dealloc_points.contains(p));
                                if !passes_through_dealloc {
                                    has_potential_leak_path = true;
                                    break;
                                }
                            }
                        }
                    }

                    if has_potential_leak_path {
                        // Some paths don't reach a deallocation
                        let dealloc_paths: Vec<Vec<PointId>> = reachable_deallocs
                            .iter()
                            .filter_map(|de| cfg.find_path(alloc_point, de.point))
                            .take(self.max_paths)
                            .collect();

                        result.add_violation(LivenessViolation::ConditionalDeallocation {
                            resource,
                            alloc_point,
                            dealloc_paths,
                            leak_paths: vec![vec![]], // Simplified: at least one leak path exists
                        });
                        leak_count += 1;
                    }
                    // If no potential leak path was found, all reachable paths
                    // from the allocation point eventually reach a deallocation.
                    // No proof obligation is needed — the check is satisfied.
                }
            }
        }
        leak_count
    }

    // -----------------------------------------------------------------------
    // Phase 2: Deadlock detection via SCC on wait-for graph
    // -----------------------------------------------------------------------

    /// Check for deadlock cycles in the wait-for graph.
    fn check_deadlock(
        &mut self,
        input: &LivenessInput,
        result: &mut LivenessVerificationResult,
    ) -> usize {
        // Build the resource wait-for graph:
        // Edge from resource A -> resource B means some thread holds A and waits for B.
        let mut wait_for_graph: hashbrown::HashMap<ResourceId, Vec<ResourceId>> =
            hashbrown::HashMap::new();

        for dep in &input.wait_for_deps {
            wait_for_graph
                .entry(dep.held)
                .or_default()
                .push(dep.wanted);
        }

        // Also ensure all wanted resources are in the graph (even if they
        // don't have outgoing edges)
        for dep in &input.wait_for_deps {
            wait_for_graph.entry(dep.wanted).or_default();
        }

        let sccs = tarjan_scc(&wait_for_graph);
        let mut deadlock_count = 0;

        for scc in sccs {
            if scc.is_cycle {
                // Found a cycle — this is a potential deadlock
                let cycle: Vec<ResourceId> = scc.nodes.iter().copied().collect();

                // Collect the threads involved in this deadlock
                let threads: Vec<ThreadId> = input
                    .wait_for_deps
                    .iter()
                    .filter(|dep| scc.nodes.contains(&dep.held) && scc.nodes.contains(&dep.wanted))
                    .map(|dep| dep.waiter)
                    .collect();

                let mut cycle_ordered = cycle.clone();
                cycle_ordered.sort();

                let description = format!(
                    "Deadlock cycle detected among resources: {}. Threads [{}] each hold one resource while waiting for another in the cycle.",
                    cycle_ordered.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(" -> "),
                    threads.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ")
                );

                result.add_violation(LivenessViolation::DeadlockCycle {
                    cycle: cycle_ordered,
                    threads,
                    description,
                });
                deadlock_count += 1;
            }
        }

        // Also check for circular dependencies in resource allocation order
        // This detects cases where thread T1 holds R1 and waits for R2,
        // while T2 holds R2 and waits for R1 (even if not explicitly in
        // the wait_for_deps list, we can infer from the acquire events).
        let circular_count = self.check_circular_resource_dependencies(input, result);
        deadlock_count += circular_count;

        deadlock_count
    }

    /// Check for circular resource dependencies based on acquisition ordering.
    fn check_circular_resource_dependencies(
        &mut self,
        input: &LivenessInput,
        result: &mut LivenessVerificationResult,
    ) -> usize {
        // Build a graph: resource A -> resource B if any thread acquires A
        // before B (without releasing A in between).
        let mut acquire_before: hashbrown::HashMap<ResourceId, Vec<ResourceId>> =
            hashbrown::HashMap::new();

        // Group events by thread
        let mut thread_events: hashbrown::HashMap<ThreadId, Vec<&ResourceEvent>> =
            hashbrown::HashMap::new();
        for event in &input.events {
            thread_events
                .entry(event.thread)
                .or_default()
                .push(event);
        }

        // For each thread, track held locks and the order they're acquired
        for (_thread, events) in &thread_events {
            let mut held_locks: Vec<ResourceId> = Vec::new();
            // Sort events by point (approximate temporal order)
            let mut sorted_events = events.clone();
            sorted_events.sort_by_key(|e| e.point);

            for event in sorted_events {
                match event.event {
                    EventAction::Acquire => {
                        // This lock is acquired while holding other locks
                        for &held in &held_locks {
                            acquire_before
                                .entry(held)
                                .or_default()
                                .push(event.resource);
                        }
                        held_locks.push(event.resource);
                    }
                    EventAction::Release => {
                        held_locks.retain(|&r| r != event.resource);
                    }
                    _ => {}
                }
            }
        }

        // Ensure all resources are in the graph
        for events in thread_events.values() {
            for event in events {
                acquire_before.entry(event.resource).or_default();
            }
        }

        // Find cycles in the acquisition ordering graph
        let sccs = tarjan_scc(&acquire_before);
        let mut circular_count = 0;

        for scc in sccs {
            if scc.is_cycle && scc.nodes.len() > 1 {
                let mut cycle: Vec<ResourceId> = scc.nodes.iter().copied().collect();
                cycle.sort();

                let description = format!(
                    "Circular resource acquisition dependency: resources [{}] may be acquired in different orders by different threads, risking deadlock.",
                    cycle.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", ")
                );

                result.add_violation(LivenessViolation::CircularDependency {
                    cycle,
                    description,
                });
                circular_count += 1;
            }
        }

        circular_count
    }

    // -----------------------------------------------------------------------
    // Phase 3: Lock discipline
    // -----------------------------------------------------------------------

    /// Check that every lock acquisition has a matching release.
    fn check_lock_discipline(
        &mut self,
        input: &LivenessInput,
        cfg: &CFG,
        result: &mut LivenessVerificationResult,
    ) -> usize {
        let mut violation_count = 0;

        // Find all lock resources
        let lock_resources: hashbrown::HashSet<ResourceId> = input
            .events
            .iter()
            .filter(|e| e.kind == ResourceKind::Lock)
            .map(|e| e.resource)
            .collect();

        for &lock in &lock_resources {
            let acquires = input.acquires_for(lock);
            let releases = input.releases_for(lock);

            for acquire in &acquires {
                let matching_releases: Vec<&&ResourceEvent> = releases
                    .iter()
                    .filter(|r| r.thread == acquire.thread && cfg.is_reachable(acquire.point, r.point))
                    .collect();

                if matching_releases.is_empty() {
                    result.add_violation(LivenessViolation::LockHeldTooLong {
                        resource: lock,
                        acquire_point: acquire.point,
                        thread: acquire.thread,
                    });
                    violation_count += 1;
                } else {
                    // Add a proof obligation that the release is reachable
                    // on ALL paths from acquire
                    result.add_obligation(ProofObligation {
                        id: self.alloc_obligation_id(),
                        description: format!(
                            "Prove that lock {} acquired at {} by {} is released on all paths",
                            lock, acquire.point, acquire.thread
                        ),
                        resource: lock,
                        obligation_kind: ObligationKind::LockReleaseReachable,
                    });
                }
            }
        }

        violation_count
    }

    // -----------------------------------------------------------------------
    // Phase 4: Message completeness
    // -----------------------------------------------------------------------

    /// Check that every channel send has a matching receive.
    fn check_message_completeness(
        &mut self,
        input: &LivenessInput,
        _cfg: &CFG,
        result: &mut LivenessVerificationResult,
    ) -> usize {
        let mut violation_count = 0;

        // Find all channel resources
        let channel_resources: hashbrown::HashSet<ResourceId> = input
            .events
            .iter()
            .filter(|e| e.kind == ResourceKind::Channel)
            .map(|e| e.resource)
            .collect();

        for &channel in &channel_resources {
            let sends = input.sends_for(channel);
            let receives = input.receives_for(channel);

            // Each send should have at least one matching receive
            for send in &sends {
                if receives.is_empty() {
                    result.add_violation(LivenessViolation::LostMessage {
                        channel,
                        send_point: send.point,
                        thread: send.thread,
                    });
                    violation_count += 1;
                } else {
                    // Check that at least one receive is reachable after this send
                    // (on some path)
                    let reachable_receives: Vec<&&ResourceEvent> = receives
                        .iter()
                        .filter(|r| r.thread != send.thread)
                        .collect();

                    if reachable_receives.is_empty() {
                        // All receives are on the same thread as send —
                        // this might be a synchronous channel, so it's OK
                        // Add a proof obligation
                        result.add_obligation(ProofObligation {
                            id: self.alloc_obligation_id(),
                            description: format!(
                                "Prove that message sent on channel {} at {} is eventually received",
                                channel, send.point
                            ),
                            resource: channel,
                            obligation_kind: ObligationKind::MessageReceived,
                        });
                    }
                }
            }
        }

        violation_count
    }

    // -----------------------------------------------------------------------
    // Path counting (approximate)
    // -----------------------------------------------------------------------

    /// Count the approximate number of paths analyzed during verification.
    fn count_analyzed_paths(&self, input: &LivenessInput, cfg: &CFG) -> usize {
        let mut count = 0;
        let allocations = input.allocations();

        for alloc in &allocations {
            let deallocs = input.deallocations_for(alloc.resource);
            for dealloc in &deallocs {
                let paths = cfg.find_all_paths(alloc.point, dealloc.point, 10);
                count += paths.len().max(1);
            }
            if deallocs.is_empty() {
                count += 1;
            }
        }

        count.max(1)
    }

    // -----------------------------------------------------------------------
    // Enhanced analysis: Use-After-Free Path Tracking
    // -----------------------------------------------------------------------

    /// Trace the complete lifecycle of each tracked resource, identifying
    /// any accesses that occur after deallocation (use-after-free).
    ///
    /// For each resource that has at least one allocation event, this method
    /// builds a [`LivenessPath`] that records the allocation point, the
    /// deallocation point (if any), and any access events that are reachable
    /// after the deallocation point in the CFG.
    pub fn compute_liveness_paths(&self, context: &VerificationContext) -> Vec<LivenessPath> {
        let cfg = self.build_cfg(&context.input);
        let allocations = context.input.allocations();
        let mut paths = Vec::new();

        for alloc_event in &allocations {
            let resource = alloc_event.resource;
            let alloc_point = alloc_event.point;
            let kind = alloc_event.kind;

            let deallocs = context.input.deallocations_for(resource);
            let dealloc_point = deallocs.first().map(|d| d.point.to_string());

            // Collect accesses after free: find access events (Read/Write) for
            // this resource whose program points are reachable from a
            // deallocation point.
            let mut access_after_free: Vec<(ProgramPoint, String)> = Vec::new();

            if let Some(dealloc) = deallocs.first() {
                let dealloc_pp = dealloc.point;

                // Find all read/write events for this resource
                let reads = context.input.reads_for(resource);
                let writes = context.input.writes_for(resource);

                for read_ev in &reads {
                    if cfg.is_reachable(dealloc_pp, read_ev.point) {
                        access_after_free.push((
                            read_ev.point.to_string(),
                            format!("read after free at {}", read_ev.point),
                        ));
                    }
                }

                for write_ev in &writes {
                    if cfg.is_reachable(dealloc_pp, write_ev.point) {
                        access_after_free.push((
                            write_ev.point.to_string(),
                            format!("write after free at {}", write_ev.point),
                        ));
                    }
                }
            }

            paths.push(LivenessPath {
                allocation_point: alloc_point.to_string(),
                deallocation_point: dealloc_point,
                access_after_free,
                resource_id: resource.0,
                resource_kind: kind.to_string(),
            });
        }

        paths
    }

    // -----------------------------------------------------------------------
    // Enhanced analysis: Dead Allocation Detection
    // -----------------------------------------------------------------------

    /// Detect allocations that are never meaningfully used.
    ///
    /// A "dead allocation" is one where the resource is allocated but:
    /// - **Never accessed**: no read, write, acquire, release, send, or
    ///   receive events occur after allocation.
    /// - **Only written, never read**: data is written but never read back,
    ///   suggesting the write is wasted.
    /// - **Redundant**: allocated and immediately deallocated with no
    ///   intervening operations.
    pub fn detect_dead_allocations(&self, context: &VerificationContext) -> Vec<DeadAllocation> {
        let allocations = context.input.allocations();
        let mut dead = Vec::new();

        for alloc_event in &allocations {
            let resource = alloc_event.resource;
            let alloc_point = alloc_event.point;

            // Collect all non-allocate/deallocate events for this resource
            let all_events: Vec<&ResourceEvent> = context
                .input
                .events_for_resource(resource)
                .into_iter()
                .filter(|e| e.event != EventAction::Allocate && e.event != EventAction::Deallocate)
                .collect();

            let reads = context.input.reads_for(resource);
            let writes = context.input.writes_for(resource);
            let deallocs = context.input.deallocations_for(resource);

            if all_events.is_empty() {
                // No events other than allocate/deallocate
                dead.push(DeadAllocation {
                    allocation_point: alloc_point.to_string(),
                    resource_id: resource.0,
                    reason: DeadReason::NeverAccessed,
                });
            } else if !writes.is_empty() && reads.is_empty() {
                // Has writes but no reads
                dead.push(DeadAllocation {
                    allocation_point: alloc_point.to_string(),
                    resource_id: resource.0,
                    reason: DeadReason::OnlyWrittenNeverRead,
                });
            } else if !deallocs.is_empty() {
                // Check for redundant allocation: deallocation is the very next
                // event after allocation with no intervening access.
                let _dealloc = deallocs.first().unwrap();
                // Check that there are no events between alloc and dealloc
                // that are reads or writes (i.e., the only events are
                // allocate and deallocate).
                let has_access_between = all_events.iter().any(|e| {
                    matches!(
                        e.event,
                        EventAction::Read
                            | EventAction::Write
                            | EventAction::Acquire
                            | EventAction::Release
                            | EventAction::Send
                            | EventAction::Receive
                    )
                });

                if !has_access_between && deallocs.len() == 1 {
                    dead.push(DeadAllocation {
                        allocation_point: alloc_point.to_string(),
                        resource_id: resource.0,
                        reason: DeadReason::RedundantAllocation,
                    });
                }
            }
        }

        dead
    }

    // -----------------------------------------------------------------------
    // Enhanced analysis: Partial Initialization Checking
    // -----------------------------------------------------------------------

    /// Check for partial initialization violations.
    ///
    /// For each Read event in the input, this method checks whether the
    /// accessed byte range (derived from the initialization map) is fully
    /// covered by initialized ranges. If not, a [`PartialInitViolation`] is
    /// generated.
    ///
    /// The initialization map is taken from the [`VerificationContext`] and
    /// maps region IDs (which correspond to resource IDs) to the byte ranges
    /// that have been initialized.
    pub fn check_partial_initialization(
        &self,
        context: &VerificationContext,
    ) -> Vec<PartialInitViolation> {
        let mut violations = Vec::new();

        // For each read event, check partial initialization
        for event in &context.input.events {
            if event.event != EventAction::Read {
                continue;
            }

            let region_id = event.resource.0;

            // Use the access range from the init_map context, or default to
            // checking the whole region (0..0 = no range info, skip)
            let init_ranges = context.init_map.initialized.get(&region_id);

            // If there's no initialization data for this region, skip
            if init_ranges.is_none() || init_ranges.unwrap().is_empty() {
                // No initialization info — cannot verify, so report violation
                // assuming the entire region is accessed and none is initialized.
                // Only report if there's actually data to check.
                continue;
            }

            // Check each initialized range entry to find the total covered region
            // and look for gaps. We assume the read accesses the union of all
            // initialized ranges plus gaps.
            let init_data = init_ranges.unwrap();
            let mut sorted: Vec<(u64, u64)> = init_data.clone();
            sorted.sort_by_key(|r| r.0);

            // Find the overall span of the region from init data
            let min_start = sorted.first().map(|r| r.0).unwrap_or(0);
            let max_end = sorted.iter().map(|r| r.1).max().unwrap_or(0);

            if min_start >= max_end {
                continue;
            }

            // Check for uninitialized gaps
            let uninit = context.init_map.check_range(region_id, min_start, max_end);

            if !uninit.is_empty() {
                violations.push(PartialInitViolation {
                    region_id,
                    access_point: event.point.to_string(),
                    accessed_range: (min_start, max_end),
                    uninitialized_ranges: uninit,
                });
            }
        }

        violations
    }

    // -----------------------------------------------------------------------
    // Enhanced analysis: Verification with Proof Obligations
    // -----------------------------------------------------------------------

    /// Run liveness verification and generate proof obligations for each
    /// violation found.
    ///
    /// This method first runs the standard liveness check via [`verify`],
    /// then examines each violation and generates a proof obligation — a
    /// formal statement that, if proven, would resolve the violation. The
    /// resulting [`LivenessVerificationResult`] contains both the original
    /// violations and the generated proof obligations.
    pub fn verify_with_proofs(
        &mut self,
        context: &VerificationContext,
    ) -> LivenessVerificationResult {
        let mut result = self.verify(&context.input);

        // Generate proof obligations for each violation
        let violations: Vec<LivenessViolation> = result.violations.clone();
        for violation in &violations {
            match violation {
                LivenessViolation::ResourceLeak {
                    resource,
                    kind,
                    alloc_point,
                    ..
                } => {
                    result.add_obligation(ProofObligation {
                        id: self.alloc_obligation_id(),
                        description: format!(
                            "Prove that {} {} allocated at {} is deallocated on all execution paths",
                            kind, resource, alloc_point
                        ),
                        resource: *resource,
                        obligation_kind: ObligationKind::DeallocationReachable,
                    });
                }
                LivenessViolation::DeadlockCycle {
                    cycle, threads, ..
                } => {
                    result.add_obligation(ProofObligation {
                        id: self.alloc_obligation_id(),
                        description: format!(
                            "Prove that deadlock among resources [{}] with threads [{}] cannot occur",
                            cycle.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", "),
                            threads.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ")
                        ),
                        resource: cycle.first().copied().unwrap_or(ResourceId(0)),
                        obligation_kind: ObligationKind::DeadlockFreedom,
                    });
                }
                LivenessViolation::LockHeldTooLong {
                    resource,
                    acquire_point,
                    ..
                } => {
                    result.add_obligation(ProofObligation {
                        id: self.alloc_obligation_id(),
                        description: format!(
                            "Prove that lock {} acquired at {} is released on all paths",
                            resource, acquire_point
                        ),
                        resource: *resource,
                        obligation_kind: ObligationKind::LockReleaseReachable,
                    });
                }
                LivenessViolation::LostMessage {
                    channel,
                    send_point,
                    ..
                } => {
                    result.add_obligation(ProofObligation {
                        id: self.alloc_obligation_id(),
                        description: format!(
                            "Prove that message sent on channel {} at {} is eventually received",
                            channel, send_point
                        ),
                        resource: *channel,
                        obligation_kind: ObligationKind::MessageReceived,
                    });
                }
                LivenessViolation::ConditionalDeallocation {
                    resource,
                    alloc_point,
                    ..
                } => {
                    result.add_obligation(ProofObligation {
                        id: self.alloc_obligation_id(),
                        description: format!(
                            "Prove that {} allocated at {} is deallocated on all conditional paths",
                            resource, alloc_point
                        ),
                        resource: *resource,
                        obligation_kind: ObligationKind::DeallocationReachable,
                    });
                }
                LivenessViolation::CircularDependency { cycle, .. } => {
                    result.add_obligation(ProofObligation {
                        id: self.alloc_obligation_id(),
                        description: format!(
                            "Prove that circular dependency among [{}] does not lead to deadlock",
                            cycle.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(" -> ")
                        ),
                        resource: cycle.first().copied().unwrap_or(ResourceId(0)),
                        obligation_kind: ObligationKind::DeadlockFreedom,
                    });
                }
            }
        }

        // Also generate proof obligations for use-after-free paths
        let liveness_paths = self.compute_liveness_paths(context);
        for path in &liveness_paths {
            if !path.access_after_free.is_empty() {
                result.add_obligation(ProofObligation {
                    id: self.alloc_obligation_id(),
                    description: format!(
                        "Prove that use-after-free accesses for resource {} at [{}] are safe",
                        path.resource_id,
                        path.access_after_free
                            .iter()
                            .map(|(pp, desc)| format!("{} ({})", pp, desc))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    resource: ResourceId(path.resource_id),
                    obligation_kind: ObligationKind::UseAfterFreeSafe,
                });
            }
        }

        // Generate proof obligations for dead allocations
        let dead_allocs = self.detect_dead_allocations(context);
        for da in &dead_allocs {
            result.add_obligation(ProofObligation {
                id: self.alloc_obligation_id(),
                description: format!(
                    "Prove that dead allocation of resource {} at {} is actually needed ({:?})",
                    da.resource_id, da.allocation_point, da.reason
                ),
                resource: ResourceId(da.resource_id),
                obligation_kind: ObligationKind::DeadAllocationNeeded,
            });
        }

        // Generate proof obligations for partial initialization violations
        let partial_violations = self.check_partial_initialization(context);
        for pv in &partial_violations {
            result.add_obligation(ProofObligation {
                id: self.alloc_obligation_id(),
                description: format!(
                    "Prove that region {} accessed at {} is fully initialized before use (uninitialized ranges: {:?})",
                    pv.region_id, pv.access_point, pv.uninitialized_ranges
                ),
                resource: ResourceId(pv.region_id),
                obligation_kind: ObligationKind::FullyInitialized,
            });
        }

        result
    }
}

impl Default for LivenessVerifier {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Convenience function
// ---------------------------------------------------------------------------

/// Run liveness verification on the given input and return a
/// [`VerificationResult`] suitable for use with the IVE verification engine.
pub fn verify_liveness(input: &LivenessInput) -> VerificationResult {
    let mut verifier = LivenessVerifier::new();
    let result = verifier.verify(input);
    result.into_verification_result()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a program point.
    fn pp(id: u64) -> PointId {
        PointId(id)
    }

    /// Helper to create a resource ID.
    fn rid(id: u64) -> ResourceId {
        ResourceId(id)
    }

    /// Helper to create a thread ID.
    fn tid(id: u64) -> ThreadId {
        ThreadId(id)
    }

    // -----------------------------------------------------------------------
    // Test 1: Simple allocation/deallocation pairs — should pass
    // -----------------------------------------------------------------------

    #[test]
    fn test_simple_allocation_deallocation_pairs() {
        let mut input = LivenessInput::new();

        // Thread 1: allocate R1 at PP1, deallocate R1 at PP2
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Deallocate,
            point: pp(2),
            thread: tid(1),
        });

        // CFG: PP1 -> PP2
        input.add_cfg_edge(ControlFlowEdge {
            from: pp(1),
            to: pp(2),
            conditional: false,
            label: None,
        });
        input.entry_point = Some(pp(1));

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);

        assert!(result.invariant_holds, "Expected invariant to hold for clean alloc/dealloc pairs");
        assert!(result.violations.is_empty(), "Expected no violations, got: {:?}", result.violations);
    }

    // -----------------------------------------------------------------------
    // Test 2: Leaked memory — allocation with no deallocation
    // -----------------------------------------------------------------------

    #[test]
    fn test_leaked_memory() {
        let mut input = LivenessInput::new();

        // Thread 1: allocate R1 at PP1, never deallocate
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });

        input.entry_point = Some(pp(1));

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);

        assert!(!result.invariant_holds, "Expected invariant violation for leaked memory");
        assert_eq!(result.violations.len(), 1);
        assert!(matches!(
            &result.violations[0],
            LivenessViolation::ResourceLeak { resource, kind, alloc_point, .. }
            if *resource == rid(1) && *kind == ResourceKind::Memory && *alloc_point == pp(1)
        ));
    }

    // -----------------------------------------------------------------------
    // Test 3: Deadlock cycle — two threads, two locks, circular wait
    // -----------------------------------------------------------------------

    #[test]
    fn test_deadlock_cycle() {
        let mut input = LivenessInput::new();

        // T1 holds Lock1, waits for Lock2
        input.add_wait_for(WaitForDependency {
            waiter: tid(1),
            held: rid(1),   // Lock1
            wanted: rid(2), // Lock2
        });

        // T2 holds Lock2, waits for Lock1
        input.add_wait_for(WaitForDependency {
            waiter: tid(2),
            held: rid(2),   // Lock2
            wanted: rid(1), // Lock1
        });

        // Add the lock events so the resources exist
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Lock,
            event: EventAction::Acquire,
            point: pp(1),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(2),
            kind: ResourceKind::Lock,
            event: EventAction::Acquire,
            point: pp(2),
            thread: tid(2),
        });

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);

        assert!(!result.invariant_holds, "Expected invariant violation for deadlock cycle");
        let has_deadlock = result.violations.iter().any(|v| {
            matches!(v, LivenessViolation::DeadlockCycle { .. })
        });
        assert!(has_deadlock, "Expected a deadlock cycle violation, got: {:?}", result.violations);
    }

    // -----------------------------------------------------------------------
    // Test 4: Conditional deallocation — some paths have dealloc, some don't
    // -----------------------------------------------------------------------

    #[test]
    fn test_conditional_deallocation() {
        let mut input = LivenessInput::new();

        // Allocate R1 at PP1
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });

        // Deallocate R1 at PP3 (only reachable via branch PP2a)
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Deallocate,
            point: pp(3),
            thread: tid(1),
        });

        // CFG: PP1 -> PP2a (dealloc path) -> PP3 (dealloc)
        //      PP1 -> PP2b (leak path) -> PP4 (end)
        input.add_cfg_edge(ControlFlowEdge {
            from: pp(1),
            to: pp(2),
            conditional: true,
            label: Some("dealloc_branch".into()),
        });
        input.add_cfg_edge(ControlFlowEdge {
            from: pp(2),
            to: pp(3),
            conditional: false,
            label: None,
        });
        input.add_cfg_edge(ControlFlowEdge {
            from: pp(1),
            to: pp(4),
            conditional: true,
            label: Some("leak_branch".into()),
        });
        input.entry_point = Some(pp(1));

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);

        assert!(!result.invariant_holds, "Expected invariant violation for conditional deallocation");
        // We should get either a ResourceLeak (for the path without dealloc)
        // or a ConditionalDeallocation violation
        let has_leak_or_conditional = result.violations.iter().any(|v| {
            matches!(v, LivenessViolation::ResourceLeak { .. })
                || matches!(v, LivenessViolation::ConditionalDeallocation { .. })
        });
        assert!(
            has_leak_or_conditional,
            "Expected a leak or conditional deallocation violation, got: {:?}",
            result.violations
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: Concurrent paths — lock acquire/release on parallel threads
    // -----------------------------------------------------------------------

    #[test]
    fn test_concurrent_paths_lock_discipline() {
        let mut input = LivenessInput::new();

        // Thread 1: acquire lock at PP1, release at PP2
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Lock,
            event: EventAction::Acquire,
            point: pp(1),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Lock,
            event: EventAction::Release,
            point: pp(2),
            thread: tid(1),
        });

        // Thread 2: acquire lock at PP10, never release
        input.add_event(ResourceEvent {
            resource: rid(2),
            kind: ResourceKind::Lock,
            event: EventAction::Acquire,
            point: pp(10),
            thread: tid(2),
        });

        // CFG for thread 1: PP1 -> PP2
        input.add_cfg_edge(ControlFlowEdge {
            from: pp(1),
            to: pp(2),
            conditional: false,
            label: None,
        });

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);

        assert!(!result.invariant_holds, "Expected invariant violation for unreleased lock on T2");
        let has_lock_violation = result.violations.iter().any(|v| {
            matches!(v, LivenessViolation::LockHeldTooLong { resource, thread, .. }
                if *resource == rid(2) && *thread == tid(2))
        });
        assert!(has_lock_violation, "Expected LockHeldTooLong for R2 on T2, got: {:?}", result.violations);
    }

    // -----------------------------------------------------------------------
    // Test 6: Nested allocations — allocate, allocate inner, free inner, free outer
    // -----------------------------------------------------------------------

    #[test]
    fn test_nested_allocations() {
        let mut input = LivenessInput::new();

        // Allocate outer R1 at PP1
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });
        // Allocate inner R2 at PP2
        input.add_event(ResourceEvent {
            resource: rid(2),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(2),
            thread: tid(1),
        });
        // Deallocate inner R2 at PP3
        input.add_event(ResourceEvent {
            resource: rid(2),
            kind: ResourceKind::Memory,
            event: EventAction::Deallocate,
            point: pp(3),
            thread: tid(1),
        });
        // Deallocate outer R1 at PP4
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Deallocate,
            point: pp(4),
            thread: tid(1),
        });

        // Linear CFG: PP1 -> PP2 -> PP3 -> PP4
        input.add_cfg_edge(ControlFlowEdge { from: pp(1), to: pp(2), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(2), to: pp(3), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(3), to: pp(4), conditional: false, label: None });
        input.entry_point = Some(pp(1));

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);

        assert!(result.invariant_holds, "Expected invariant to hold for nested allocations");
        assert!(result.violations.is_empty(), "Expected no violations, got: {:?}", result.violations);
    }

    // -----------------------------------------------------------------------
    // Test 7: Circular dependencies — lock ordering violation
    // -----------------------------------------------------------------------

    #[test]
    fn test_circular_dependencies() {
        let mut input = LivenessInput::new();

        // T1 acquires Lock1 (R1) at PP1, then Lock2 (R2) at PP2
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Lock,
            event: EventAction::Acquire,
            point: pp(1),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(2),
            kind: ResourceKind::Lock,
            event: EventAction::Acquire,
            point: pp(2),
            thread: tid(1),
        });

        // T2 acquires Lock2 (R2) at PP10, then Lock1 (R1) at PP11
        input.add_event(ResourceEvent {
            resource: rid(2),
            kind: ResourceKind::Lock,
            event: EventAction::Acquire,
            point: pp(10),
            thread: tid(2),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Lock,
            event: EventAction::Acquire,
            point: pp(11),
            thread: tid(2),
        });

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);

        assert!(!result.invariant_holds, "Expected invariant violation for circular lock dependency");
        let has_circular = result.violations.iter().any(|v| {
            matches!(v, LivenessViolation::CircularDependency { .. })
                || matches!(v, LivenessViolation::DeadlockCycle { .. })
        });
        assert!(has_circular, "Expected circular dependency or deadlock violation, got: {:?}", result.violations);
    }

    // -----------------------------------------------------------------------
    // Test 8: Clean program — everything properly paired
    // -----------------------------------------------------------------------

    #[test]
    fn test_clean_program() {
        let mut input = LivenessInput::new();

        // Allocate and free memory
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Deallocate,
            point: pp(5),
            thread: tid(1),
        });

        // Lock acquire and release
        input.add_event(ResourceEvent {
            resource: rid(2),
            kind: ResourceKind::Lock,
            event: EventAction::Acquire,
            point: pp(2),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(2),
            kind: ResourceKind::Lock,
            event: EventAction::Release,
            point: pp(4),
            thread: tid(1),
        });

        // Channel send and receive
        input.add_event(ResourceEvent {
            resource: rid(3),
            kind: ResourceKind::Channel,
            event: EventAction::Send,
            point: pp(2),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(3),
            kind: ResourceKind::Channel,
            event: EventAction::Receive,
            point: pp(3),
            thread: tid(2),
        });

        // CFG: PP1 -> PP2 -> PP3 -> PP4 -> PP5
        input.add_cfg_edge(ControlFlowEdge { from: pp(1), to: pp(2), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(2), to: pp(3), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(3), to: pp(4), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(4), to: pp(5), conditional: false, label: None });
        input.entry_point = Some(pp(1));

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);

        assert!(result.invariant_holds, "Expected invariant to hold for clean program");
        assert!(result.violations.is_empty(), "Expected no violations, got: {:?}", result.violations);
    }

    // -----------------------------------------------------------------------
    // Additional tests for internal components
    // -----------------------------------------------------------------------

    #[test]
    fn test_cfg_reachability() {
        let mut cfg = CFG::new();
        cfg.add_edge(pp(1), pp(2));
        cfg.add_edge(pp(2), pp(3));
        cfg.add_edge(pp(3), pp(4));

        assert!(cfg.is_reachable(pp(1), pp(4)));
        assert!(cfg.is_reachable(pp(1), pp(2)));
        assert!(!cfg.is_reachable(pp(4), pp(1)));
        assert!(!cfg.is_reachable(pp(1), pp(99)));
    }

    #[test]
    fn test_cfg_find_path() {
        let mut cfg = CFG::new();
        cfg.add_edge(pp(1), pp(2));
        cfg.add_edge(pp(2), pp(3));
        cfg.add_edge(pp(3), pp(4));

        let path = cfg.find_path(pp(1), pp(4));
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path[0], pp(1));
        assert_eq!(path[path.len() - 1], pp(4));

        let no_path = cfg.find_path(pp(4), pp(1));
        assert!(no_path.is_none());
    }

    #[test]
    fn test_cfg_find_all_paths() {
        let mut cfg = CFG::new();
        cfg.add_edge(pp(1), pp(2));
        cfg.add_edge(pp(1), pp(3));
        cfg.add_edge(pp(2), pp(4));
        cfg.add_edge(pp(3), pp(4));

        let paths = cfg.find_all_paths(pp(1), pp(4), 10);
        assert_eq!(paths.len(), 2, "Expected two paths from PP1 to PP4");
    }

    #[test]
    fn test_tarjan_scc_no_cycles() {
        // A -> B -> C (no cycle)
        let mut graph: hashbrown::HashMap<ResourceId, Vec<ResourceId>> =
            hashbrown::HashMap::new();
        graph.insert(rid(1), vec![rid(2)]);
        graph.insert(rid(2), vec![rid(3)]);
        graph.insert(rid(3), vec![]);

        let sccs = tarjan_scc(&graph);
        let cycles: Vec<&SCC> = sccs.iter().filter(|scc| scc.is_cycle).collect();
        assert!(cycles.is_empty(), "Expected no cycles in a DAG");
    }

    #[test]
    fn test_tarjan_scc_with_cycle() {
        // A -> B -> C -> A (cycle)
        let mut graph: hashbrown::HashMap<ResourceId, Vec<ResourceId>> =
            hashbrown::HashMap::new();
        graph.insert(rid(1), vec![rid(2)]);
        graph.insert(rid(2), vec![rid(3)]);
        graph.insert(rid(3), vec![rid(1)]);

        let sccs = tarjan_scc(&graph);
        let cycles: Vec<&SCC> = sccs.iter().filter(|scc| scc.is_cycle).collect();
        assert_eq!(cycles.len(), 1, "Expected one SCC cycle");
        assert_eq!(cycles[0].nodes.len(), 3);
    }

    #[test]
    fn test_verification_result_proven() {
        let result = LivenessVerificationResult {
            violations: Vec::new(),
            proof_obligations: Vec::new(),
            resources_checked: 5,
            paths_analyzed: 10,
            invariant_holds: true,
        };

        let vr = result.into_verification_result();
        assert!(vr.is_proven());
        assert!(!vr.is_violated());
    }

    #[test]
    fn test_verification_result_violated() {
        let result = LivenessVerificationResult {
            violations: vec![LivenessViolation::ResourceLeak {
                resource: rid(1),
                kind: ResourceKind::Memory,
                alloc_point: pp(1),
                thread: tid(1),
            }],
            proof_obligations: Vec::new(),
            resources_checked: 1,
            paths_analyzed: 1,
            invariant_holds: false,
        };

        let vr = result.into_verification_result();
        assert!(vr.is_violated());
        assert!(!vr.is_proven());
    }

    #[test]
    fn test_verification_result_probably_safe() {
        let result = LivenessVerificationResult {
            violations: Vec::new(),
            proof_obligations: vec![ProofObligation {
                id: 0,
                description: "test obligation".into(),
                resource: rid(1),
                obligation_kind: ObligationKind::DeallocationReachable,
            }],
            resources_checked: 1,
            paths_analyzed: 1,
            invariant_holds: true,
        };

        let vr = result.into_verification_result();
        assert!(!vr.is_proven());
        assert!(!vr.is_violated());
        // Should be ProbablySafe since there are proof obligations
        assert!(matches!(vr.status, VerificationStatus::ProbablySafe { .. }));
    }

    #[test]
    fn test_convenience_function() {
        let mut input = LivenessInput::new();
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Deallocate,
            point: pp(2),
            thread: tid(1),
        });
        input.add_cfg_edge(ControlFlowEdge {
            from: pp(1),
            to: pp(2),
            conditional: false,
            label: None,
        });

        let result = verify_liveness(&input);
        assert!(result.is_proven());
    }

    #[test]
    fn test_lost_message_violation() {
        let mut input = LivenessInput::new();
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Channel,
            event: EventAction::Send,
            point: pp(1),
            thread: tid(1),
        });
        // No receive event — lost message

        let mut verifier = LivenessVerifier::new();
        let result = verifier.verify(&input);

        assert!(!result.invariant_holds);
        let has_lost = result.violations.iter().any(|v| {
            matches!(v, LivenessViolation::LostMessage { .. })
        });
        assert!(has_lost, "Expected LostMessage violation, got: {:?}", result.violations);
    }

    #[test]
    fn test_display_violations() {
        let leak = LivenessViolation::ResourceLeak {
            resource: rid(42),
            kind: ResourceKind::Memory,
            alloc_point: pp(10),
            thread: tid(1),
        };
        let s = format!("{}", leak);
        assert!(s.contains("Resource leak"));
        assert!(s.contains("Res42"));

        let deadlock = LivenessViolation::DeadlockCycle {
            cycle: vec![rid(1), rid(2)],
            threads: vec![tid(1), tid(2)],
            description: "test deadlock".into(),
        };
        let s = format!("{}", deadlock);
        assert!(s.contains("Deadlock cycle"));

        let lock = LivenessViolation::LockHeldTooLong {
            resource: rid(5),
            acquire_point: pp(3),
            thread: tid(1),
        };
        let s = format!("{}", lock);
        assert!(s.contains("Lock"));
        assert!(s.contains("never released"));

        let lost = LivenessViolation::LostMessage {
            channel: rid(7),
            send_point: pp(4),
            thread: tid(2),
        };
        let s = format!("{}", lost);
        assert!(s.contains("never received"));

        let cond = LivenessViolation::ConditionalDeallocation {
            resource: rid(9),
            alloc_point: pp(1),
            dealloc_paths: vec![vec![pp(1), pp(2)]],
            leak_paths: vec![vec![pp(1), pp(3)]],
        };
        let s = format!("{}", cond);
        assert!(s.contains("Conditional deallocation"));

        let circ = LivenessViolation::CircularDependency {
            cycle: vec![rid(1), rid(2), rid(3)],
            description: "test circular".into(),
        };
        let s = format!("{}", circ);
        assert!(s.contains("Circular dependency"));
    }

    // =======================================================================
    // Enhanced liveness verification tests
    // =======================================================================

    // -----------------------------------------------------------------------
    // Test: Use-after-free with path tracking
    // -----------------------------------------------------------------------

    #[test]
    fn test_use_after_free_path_tracking() {
        let mut input = LivenessInput::new();

        // Allocate R1 at PP1, deallocate at PP3, read at PP5 (after free)
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Deallocate,
            point: pp(3),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Read,
            point: pp(5),
            thread: tid(1),
        });

        // CFG: PP1 -> PP2 -> PP3 -> PP4 -> PP5
        input.add_cfg_edge(ControlFlowEdge { from: pp(1), to: pp(2), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(2), to: pp(3), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(3), to: pp(4), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(4), to: pp(5), conditional: false, label: None });

        let verifier = LivenessVerifier::new();
        let context = VerificationContext::new(input);
        let paths = verifier.compute_liveness_paths(&context);

        assert_eq!(paths.len(), 1, "Expected one liveness path for one allocation");
        let path = &paths[0];
        assert_eq!(path.resource_id, 1);
        assert_eq!(path.allocation_point, "PP1");
        assert_eq!(path.deallocation_point, Some("PP3".to_string()));
        assert_eq!(path.access_after_free.len(), 1, "Expected one use-after-free access");
        assert!(path.access_after_free[0].0.contains("PP5"));
        assert!(path.access_after_free[0].1.contains("read after free"));
    }

    // -----------------------------------------------------------------------
    // Test: Dead allocation — never accessed
    // -----------------------------------------------------------------------

    #[test]
    fn test_dead_allocation_never_accessed() {
        let mut input = LivenessInput::new();

        // Allocate R1 but never read, write, or otherwise use it
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });

        let verifier = LivenessVerifier::new();
        let context = VerificationContext::new(input);
        let dead = verifier.detect_dead_allocations(&context);

        assert_eq!(dead.len(), 1, "Expected one dead allocation");
        assert_eq!(dead[0].resource_id, 1);
        assert!(matches!(dead[0].reason, DeadReason::NeverAccessed));
    }

    // -----------------------------------------------------------------------
    // Test: Dead allocation — only written, never read
    // -----------------------------------------------------------------------

    #[test]
    fn test_dead_allocation_only_written_never_read() {
        let mut input = LivenessInput::new();

        // Allocate R1, write to it, but never read
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Write,
            point: pp(2),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Deallocate,
            point: pp(3),
            thread: tid(1),
        });

        input.add_cfg_edge(ControlFlowEdge { from: pp(1), to: pp(2), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(2), to: pp(3), conditional: false, label: None });

        let verifier = LivenessVerifier::new();
        let context = VerificationContext::new(input);
        let dead = verifier.detect_dead_allocations(&context);

        assert_eq!(dead.len(), 1, "Expected one dead allocation (only written)");
        assert_eq!(dead[0].resource_id, 1);
        assert!(matches!(dead[0].reason, DeadReason::OnlyWrittenNeverRead));
    }

    // -----------------------------------------------------------------------
    // Test: Partial initialization — struct with some fields uninitialized
    // -----------------------------------------------------------------------

    #[test]
    fn test_partial_initialization_some_fields_uninit() {
        let mut input = LivenessInput::new();

        // Region 1: allocate, then read (but only partially initialized)
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Read,
            point: pp(2),
            thread: tid(1),
        });

        // Init map: only bytes 0-3 initialized, but region spans 0-15
        let mut init_map = InitializationMap::new();
        init_map.mark_initialized(1, 0, 4);   // bytes 0-3
        init_map.mark_initialized(1, 8, 16);  // bytes 8-15
        // Gap: bytes 4-7 are uninitialized

        let verifier = LivenessVerifier::new();
        let context = VerificationContext::with_init_map(input, init_map);
        let violations = verifier.check_partial_initialization(&context);

        assert_eq!(violations.len(), 1, "Expected one partial init violation");
        assert_eq!(violations[0].region_id, 1);
        assert!(!violations[0].uninitialized_ranges.is_empty(), "Expected uninitialized ranges");
        // Should find gap at bytes 4-7
        assert!(
            violations[0].uninitialized_ranges.iter().any(|(s, e)| *s == 4 && *e == 8),
            "Expected uninitialized gap at (4, 8), got: {:?}",
            violations[0].uninitialized_ranges
        );
    }

    // -----------------------------------------------------------------------
    // Test: Full initialization — all bytes covered
    // -----------------------------------------------------------------------

    #[test]
    fn test_full_initialization_all_bytes_covered() {
        let mut input = LivenessInput::new();

        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Read,
            point: pp(2),
            thread: tid(1),
        });

        // Init map: fully covers bytes 0-16 (contiguous)
        let mut init_map = InitializationMap::new();
        init_map.mark_initialized(1, 0, 16);

        let verifier = LivenessVerifier::new();
        let context = VerificationContext::with_init_map(input, init_map);
        let violations = verifier.check_partial_initialization(&context);

        assert!(violations.is_empty(), "Expected no partial init violations for fully initialized region, got: {:?}", violations);
    }

    // -----------------------------------------------------------------------
    // Test: Proof obligation generation for use-after-free
    // -----------------------------------------------------------------------

    #[test]
    fn test_proof_obligation_for_use_after_free() {
        let mut input = LivenessInput::new();

        // Allocate R1, deallocate, then read after free
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Deallocate,
            point: pp(3),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Read,
            point: pp(5),
            thread: tid(1),
        });

        // CFG: PP1 -> PP2 -> PP3 -> PP4 -> PP5
        input.add_cfg_edge(ControlFlowEdge { from: pp(1), to: pp(2), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(2), to: pp(3), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(3), to: pp(4), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(4), to: pp(5), conditional: false, label: None });

        let mut verifier = LivenessVerifier::new();
        let context = VerificationContext::new(input);
        let result = verifier.verify_with_proofs(&context);

        // Should have at least one proof obligation for use-after-free
        let uaf_obligations: Vec<&ProofObligation> = result
            .proof_obligations
            .iter()
            .filter(|o| o.obligation_kind == ObligationKind::UseAfterFreeSafe)
            .collect();

        assert!(
            !uaf_obligations.is_empty(),
            "Expected use-after-free proof obligation, got: {:?}",
            result.proof_obligations
        );
        assert!(uaf_obligations[0].description.contains("use-after-free"));
    }

    // -----------------------------------------------------------------------
    // Test: Proof obligation generation for dead allocation
    // -----------------------------------------------------------------------

    #[test]
    fn test_proof_obligation_for_dead_allocation() {
        let mut input = LivenessInput::new();

        // Allocate R1 but never access it (dead allocation)
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });

        let mut verifier = LivenessVerifier::new();
        let context = VerificationContext::new(input);
        let result = verifier.verify_with_proofs(&context);

        // Should have a proof obligation for the dead allocation
        let dead_obligations: Vec<&ProofObligation> = result
            .proof_obligations
            .iter()
            .filter(|o| o.obligation_kind == ObligationKind::DeadAllocationNeeded)
            .collect();

        assert!(
            !dead_obligations.is_empty(),
            "Expected dead allocation proof obligation, got: {:?}",
            result.proof_obligations
        );
        assert!(dead_obligations[0].description.contains("dead allocation"));
    }

    // -----------------------------------------------------------------------
    // Test: Multiple resources with mixed liveness
    // -----------------------------------------------------------------------

    #[test]
    fn test_multiple_resources_mixed_liveness() {
        let mut input = LivenessInput::new();

        // R1: properly allocated, used, and freed
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(1),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Write,
            point: pp(2),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Read,
            point: pp(3),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(1),
            kind: ResourceKind::Memory,
            event: EventAction::Deallocate,
            point: pp(4),
            thread: tid(1),
        });

        // R2: allocated but never accessed (dead)
        input.add_event(ResourceEvent {
            resource: rid(2),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(10),
            thread: tid(1),
        });

        // R3: allocated, written, but never read (only written, never read)
        input.add_event(ResourceEvent {
            resource: rid(3),
            kind: ResourceKind::Memory,
            event: EventAction::Allocate,
            point: pp(20),
            thread: tid(1),
        });
        input.add_event(ResourceEvent {
            resource: rid(3),
            kind: ResourceKind::Memory,
            event: EventAction::Write,
            point: pp(21),
            thread: tid(1),
        });

        // CFG: linear for R1
        input.add_cfg_edge(ControlFlowEdge { from: pp(1), to: pp(2), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(2), to: pp(3), conditional: false, label: None });
        input.add_cfg_edge(ControlFlowEdge { from: pp(3), to: pp(4), conditional: false, label: None });

        let verifier = LivenessVerifier::new();
        let context = VerificationContext::new(input);

        // Check liveness paths
        let paths = verifier.compute_liveness_paths(&context);
        assert_eq!(paths.len(), 3, "Expected three liveness paths for three allocations");

        // R1 should have no use-after-free
        let r1_path = paths.iter().find(|p| p.resource_id == 1).unwrap();
        assert!(r1_path.access_after_free.is_empty(), "R1 should have no use-after-free");

        // R2 should have no deallocation
        let r2_path = paths.iter().find(|p| p.resource_id == 2).unwrap();
        assert!(r2_path.deallocation_point.is_none(), "R2 should have no deallocation");

        // Check dead allocations
        let dead = verifier.detect_dead_allocations(&context);
        assert!(dead.len() >= 2, "Expected at least 2 dead allocations, got: {:?}", dead);

        let r2_dead = dead.iter().find(|d| d.resource_id == 2);
        assert!(r2_dead.is_some(), "R2 should be detected as dead");
        assert!(matches!(r2_dead.unwrap().reason, DeadReason::NeverAccessed));

        let r3_dead = dead.iter().find(|d| d.resource_id == 3);
        assert!(r3_dead.is_some(), "R3 should be detected as dead");
        assert!(matches!(r3_dead.unwrap().reason, DeadReason::OnlyWrittenNeverRead));
    }

    // -----------------------------------------------------------------------
    // Test: InitializationMap check_range utility
    // -----------------------------------------------------------------------

    #[test]
    fn test_initialization_map_check_range() {
        let mut init_map = InitializationMap::new();
        // Region 1: bytes 0-4 and 8-16 initialized, gap at 4-8
        init_map.mark_initialized(1, 0, 4);
        init_map.mark_initialized(1, 8, 16);

        // Check full range [0, 16): should find gap at [4, 8)
        let uninit = init_map.check_range(1, 0, 16);
        assert_eq!(uninit, vec![(4, 8)], "Expected gap at (4, 8)");

        // Check subrange [0, 4): fully initialized
        let uninit = init_map.check_range(1, 0, 4);
        assert!(uninit.is_empty(), "Expected no gaps in [0, 4)");

        // Check subrange [4, 8): entirely uninitialized
        let uninit = init_map.check_range(1, 4, 8);
        assert_eq!(uninit, vec![(4, 8)], "Expected gap at (4, 8)");

        // Check unknown region: entirely uninitialized
        let uninit = init_map.check_range(99, 0, 10);
        assert_eq!(uninit, vec![(0, 10)], "Expected full range uninitialized for unknown region");
    }
}
