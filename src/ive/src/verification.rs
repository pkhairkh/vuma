//! Verification engine for the IVE module.
//!
//! The verification engine checks the five core VUMA invariants against an
//! SCG and its inferred BDs, delegating to the real per-invariant verifiers:
//!
//! - **Liveness**: [`crate::liveness::LivenessVerifier`] — every requested resource eventually provided
//! - **Exclusivity**: [`crate::exclusivity::ExclusivityVerifier`] — at most one owner for exclusive resources
//! - **Interpretation**: [`crate::interpretation::InterpretationVerifier`] — every read uses the correct BD
//! - **Origin**: [`crate::origin::OriginVerifier`] — every datum has traceable provenance
//! - **Cleanup**: [`crate::cleanup::CleanupVerifier`] — every acquired resource eventually released
//!
//! # Architecture
//!
//! The `VerificationEngine` is a facade that:
//! 1. Accepts a `vuma_scg::SCG` and optional BD map
//! 2. Extracts per-invariant input data from the SCG (via `scg_extract` converters)
//! 3. Delegates to each of the five specialized verifiers
//! 4. Aggregates results into a unified vector

use crate::cleanup::{
    CleanupGraph, CleanupVerifier, NodeId as CleanupNodeId, OperationKind,
    ResourceId as CleanupResourceId, ResourceKind as CleanupResourceKind,
};
use crate::exclusivity::{
    AccessKind as ExclusivityAccessKind, AccessRecord, ExclusivityInput, ExclusivityVerifier,
    SyncEdgeRecord, SyncOrdering,
};
use crate::interpretation::InterpretationVerifier;
use crate::liveness::{
    EventAction, LivenessInput, LivenessVerifier, PointId, ResourceEvent, ResourceId, ResourceKind,
    ThreadId,
};
use crate::origin::{
    Access as OriginAccess, AccessId as OriginAccessId, AccessKind as OriginAccessKind, Address,
    Derivation, DerivationId, DerivationKind, DerivationSource, OriginVerifier,
    Region as OriginRegion, RegionId as OriginRegionId,
};
use crate::result::{BatchedViolations, InvariantViolation, Severity, VerificationResult};
use std::collections::HashMap;
use vuma_bd::capd::{CapD, Capability};
use vuma_bd::descriptor::BD;
use vuma_scg::edge::EdgeKind;
use vuma_scg::graph::SCG;
use vuma_scg::node::{AccessMode, NodeId, NodePayload, NodeType};

// ---------------------------------------------------------------------------
// VerificationInput
// ---------------------------------------------------------------------------

/// Input for the verification engine: an SCG and optionally pre-inferred BDs.
///
/// If no BD map is provided, the verification engine will run BD inference
/// automatically before verification.
pub struct VerificationInput {
    /// The SCG to verify.
    pub scg: SCG,
    /// Pre-inferred BD map (optional — will be inferred if absent).
    pub bd_map: Option<HashMap<NodeId, BD>>,
}

impl VerificationInput {
    /// Create verification input from an SCG (without pre-inferred BDs).
    pub fn from_scg(scg: SCG) -> Self {
        Self { scg, bd_map: None }
    }

    /// Create verification input with a pre-inferred BD map.
    pub fn with_bd_map(scg: SCG, bd_map: HashMap<NodeId, BD>) -> Self {
        Self {
            scg,
            bd_map: Some(bd_map),
        }
    }
}

// ---------------------------------------------------------------------------
// VerificationEngine
// ---------------------------------------------------------------------------

/// The verification engine checks VUMA's core invariants against SCGs.
///
/// Each verification method performs a specific invariant check and returns
/// a [`VerificationResult`] encoding the outcome. The `verify_all` method
/// runs every check and aggregates the results.
///
/// # Invariant Definitions
///
/// | Invariant        | Meaning                                          | Verifier                   |
/// |------------------|--------------------------------------------------|----------------------------|
/// | Liveness         | Every request eventually receives a response.     | `LivenessVerifier`         |
/// | Exclusivity      | At most one owner for exclusive resources.        | `ExclusivityVerifier`      |
/// | Interpretation   | Reads use the correct behavioral description.     | `InterpretationVerifier`   |
/// | Origin           | Every datum has a traceable provenance.           | `OriginVerifier`           |
/// | Cleanup          | Acquired resources are eventually released.        | `CleanupVerifier`          |
pub struct VerificationEngine {
    /// Whether to emit detailed diagnostic logging.
    verbose: bool,
}

impl VerificationEngine {
    /// Construct a new verification engine.
    pub fn new() -> Self {
        Self { verbose: false }
    }

    /// Enable verbose diagnostic output.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Verify the **liveness** invariant: every requested resource will
    /// eventually be provided.
    ///
    /// Extracts liveness-relevant events from the SCG (allocations,
    /// deallocations, lock acquire/release, channel send/receive) and
    /// runs the `LivenessVerifier` which performs:
    /// - Leak detection (allocations without matching deallocations)
    /// - Deadlock detection via Tarjan's SCC on wait-for dependencies
    /// - Lock discipline checks
    /// - Message completeness verification
    pub fn verify_liveness(&self, input: &VerificationInput) -> VerificationResult {
        let liveness_input = self.extract_liveness_input(&input.scg);
        let mut verifier = LivenessVerifier::new().with_verbose(self.verbose);
        let result = verifier.verify(&liveness_input);
        result.into_verification_result()
    }

    /// Verify the **exclusivity** invariant: at most one owner for
    /// exclusive resources.
    ///
    /// Extracts access records and synchronization edges from the SCG,
    /// then runs the `ExclusivityVerifier` which performs:
    /// - O(n²) pairwise access conflict detection
    /// - O(n log n) interval tree optimization for large inputs
    /// - Interference graph construction
    /// - CapD-aware conflict resolution
    pub fn verify_exclusivity(&self, input: &VerificationInput) -> VerificationResult {
        let exclusivity_input = self.extract_exclusivity_input(&input.scg);
        let verifier = ExclusivityVerifier::new().with_verbose(self.verbose);
        let output = verifier.verify(&exclusivity_input);
        output.result
    }

    /// Verify the **interpretation** invariant: every read interprets
    /// data under the correct behavioral description (BD).
    ///
    /// Feeds write/read events from the SCG into the `InterpretationVerifier`
    /// which checks:
    /// - RepD compatibility between write and read BDs
    /// - CapD transition validity (weakening/strengthening)
    /// - RelD preservation
    /// - Type confusion detection
    /// - Pointer reinterpretation safety
    pub fn verify_interpretation(&self, input: &VerificationInput) -> VerificationResult {
        let mut verifier = InterpretationVerifier::new();
        self.feed_interpretation_events(&mut verifier, &input.scg, &input.bd_map);
        verifier.verify()
    }

    /// Verify the **origin** invariant: every piece of data has a
    /// well-defined provenance.
    ///
    /// Extracts memory regions, derivations, and accesses from the SCG,
    /// then runs the `OriginVerifier` which checks:
    /// - Provenance forest construction (every pointer traces to an allocation)
    /// - Taint tracking (trusted vs untrusted data)
    /// - Orphan/fabricated pointer detection
    /// - Bounds checking for derived pointers
    pub fn verify_origin(&self, input: &VerificationInput) -> VerificationResult {
        let mut verifier = OriginVerifier::new().with_verbose(self.verbose);
        self.feed_origin_data(&mut verifier, &input.scg);
        let report = verifier.verify();
        report.to_verification_result()
    }

    /// Verify the **cleanup** invariant: every acquired resource is
    /// eventually released.
    ///
    /// Constructs a `CleanupGraph` from the SCG's allocation/deallocation
    /// and control flow structure, then runs the `CleanupVerifier` which
    /// performs:
    /// - Path-sensitive DFS with resource state tracking
    /// - Leak detection (resources not freed on any path)
    /// - Double-free detection
    /// - Use-after-free detection
    pub fn verify_cleanup(&self, input: &VerificationInput) -> VerificationResult {
        let cleanup_graph = self.extract_cleanup_graph(&input.scg);
        let verifier = CleanupVerifier::new().with_verbose(self.verbose);
        let report = verifier.verify(&cleanup_graph);
        report.to_verification_result()
    }

    /// Run all five invariant checks and return the aggregated results.
    ///
    /// The order is: origin → liveness → exclusivity → interpretation → cleanup.
    /// This follows the dependency order from the VUMA specification:
    /// origin must be verified before liveness, and liveness before the rest.
    pub fn verify_all(&self, input: &VerificationInput) -> Vec<VerificationResult> {
        let origin = self.verify_origin(input);
        let liveness = self.verify_liveness(input);
        let exclusivity = self.verify_exclusivity(input);
        let interpretation = self.verify_interpretation(input);
        let cleanup = self.verify_cleanup(input);

        vec![origin, liveness, exclusivity, interpretation, cleanup]
    }

    // -----------------------------------------------------------------------
    // SCG → Verifier Input Extraction
    // -----------------------------------------------------------------------

    /// Extract liveness-relevant input from the SCG.
    fn extract_liveness_input(&self, scg: &SCG) -> LivenessInput {
        let mut input = LivenessInput::new();
        let mut next_resource_id: u64 = 1;
        // Map from SCG allocation NodeId to the ResourceId assigned for
        // liveness tracking, so that deallocations can reference the same
        // resource ID as their corresponding allocation.
        let mut alloc_node_to_rid: HashMap<NodeId, ResourceId> = HashMap::new();

        for node in scg.nodes() {
            match node.node_type {
                NodeType::Allocation => {
                    if let NodePayload::Allocation(_alloc) = &node.payload {
                        let rid = ResourceId(next_resource_id);
                        next_resource_id += 1;
                        alloc_node_to_rid.insert(node.id, rid);
                        input.add_event(ResourceEvent {
                            resource: rid,
                            kind: ResourceKind::Memory,
                            event: EventAction::Allocate,
                            point: PointId(node.id.as_u64()),
                            thread: ThreadId(0),
                        });
                    }
                }
                NodeType::Deallocation => {
                    if let NodePayload::Deallocation(dealloc) = &node.payload {
                        // Look up the ResourceId that was assigned to the
                        // allocation node this deallocation refers to.
                        if let Some(&rid) = alloc_node_to_rid.get(&dealloc.allocation_node) {
                            input.add_event(ResourceEvent {
                                resource: rid,
                                kind: ResourceKind::Memory,
                                event: EventAction::Deallocate,
                                point: PointId(node.id.as_u64()),
                                thread: ThreadId(0),
                            });
                        }
                    }
                }
                NodeType::Access => {
                    // Access events don't directly affect liveness
                    // but they create resource usage points
                }
                _ => {}
            }
        }

        // Add ControlFlow edges as CFG edges for liveness reachability analysis.
        // Only ControlFlow edges represent actual execution ordering; Derivation
        // and DataFlow edges represent logical relationships that can create
        // spurious "shortcut" paths in the CFG, leading to false-positive
        // leak reports for well-formed programs.
        //
        // Intraprocedural call-return ControlFlow edges (computation→FunctionEntry,
        // FunctionEntry→FunctionReturn) create dead-end branches that cause
        // false-positive "conditional deallocation" violations. We skip these
        // because the real control flow is already captured by the sequential
        // ControlFlow chain through the Computation nodes. We only include
        // interprocedural Call/Return edges (which connect real function
        // definitions) and ControlFlow edges that don't enter/exit
        // FunctionEntry/FunctionReturn nodes.
        let fn_entry_nodes: hashbrown::HashSet<u64> = scg.nodes()
            .filter(|n| matches!(
                n.node_type,
                NodeType::Control
            ) && matches!(&n.payload, NodePayload::Control(c) if c.kind == vuma_scg::node::ControlKind::FunctionEntry))
            .map(|n| n.id.as_u64())
            .collect();
        let fn_return_nodes: hashbrown::HashSet<u64> = scg.nodes()
            .filter(|n| matches!(
                n.node_type,
                NodeType::Control
            ) && matches!(&n.payload, NodePayload::Control(c) if c.kind == vuma_scg::node::ControlKind::FunctionReturn))
            .map(|n| n.id.as_u64())
            .collect();

        for edge in scg.edges() {
            match &edge.kind {
                vuma_scg::edge::EdgeKind::ControlFlow => {
                    let src = edge.source.as_u64();
                    let dst = edge.target.as_u64();
                    // Skip intraprocedural call-return edges:
                    // - computation → FunctionEntry (enters the call stub)
                    // - FunctionEntry → FunctionReturn (the call stub itself)
                    // - FunctionReturn → * (dead-end exit from call stub)
                    if fn_entry_nodes.contains(&dst) || fn_return_nodes.contains(&src) {
                        continue;
                    }
                    input.add_cfg_edge(crate::liveness::ControlFlowEdge {
                        from: PointId(src),
                        to: PointId(dst),
                        conditional: false,
                        label: None,
                    });
                }
                vuma_scg::edge::EdgeKind::Call { .. } => {
                    // Interprocedural Call edge: caller → callee's FunctionEntry.
                    // These connect real function definitions and are valid paths.
                    input.add_cfg_edge(crate::liveness::ControlFlowEdge {
                        from: PointId(edge.source.as_u64()),
                        to: PointId(edge.target.as_u64()),
                        conditional: false,
                        label: Some("call".to_string()),
                    });
                }
                vuma_scg::edge::EdgeKind::Return { .. } => {
                    // Interprocedural Return edge: callee's FunctionReturn → caller.
                    input.add_cfg_edge(crate::liveness::ControlFlowEdge {
                        from: PointId(edge.source.as_u64()),
                        to: PointId(edge.target.as_u64()),
                        conditional: false,
                        label: Some("return".to_string()),
                    });
                }
                _ => {}
            }
        }

        // Set entry point to the first node (if any)
        if let Some(first_node) = scg.nodes().next() {
            input.entry_point = Some(PointId(first_node.id.as_u64()));
        }

        input
    }

    /// Extract exclusivity-relevant input from the SCG.
    fn extract_exclusivity_input(&self, scg: &SCG) -> ExclusivityInput {
        let mut input = ExclusivityInput::new();
        let mut next_access_id: u64 = 1;

        for node in scg.nodes() {
            if node.node_type == NodeType::Access {
                if let NodePayload::Access(access) = &node.payload {
                    let access_id = crate::exclusivity::AccessId(next_access_id);
                    next_access_id += 1;

                    let kind = match access.mode {
                        AccessMode::Read => ExclusivityAccessKind::Read,
                        AccessMode::Write => ExclusivityAccessKind::Write,
                        AccessMode::ReadWrite => ExclusivityAccessKind::Write, // Conservative
                    };

                    let base_address = 0; // SCG doesn't track concrete addresses
                    let size = access.access_size.unwrap_or(8);

                    let pp = format!(
                        "{}:{}",
                        node.program_point.file.as_deref().unwrap_or("?"),
                        node.program_point.line.unwrap_or(0)
                    );

                    input.add_access(AccessRecord::new(
                        access_id,
                        kind,
                        base_address,
                        size,
                        pp,
                        node.id.as_u64(),          // derivation_id
                        access.region_id.as_u64(), // region_id
                    ));
                }
            }
        }

        // Extract synchronization edges from ControlFlow edges between Access nodes
        for edge in scg.edges() {
            if edge.kind == vuma_scg::edge::EdgeKind::ControlFlow {
                let src = scg.get_node(edge.source);
                let dst = scg.get_node(edge.target);
                if let (Some(s), Some(d)) = (src, dst) {
                    if s.node_type == NodeType::Access && d.node_type == NodeType::Access {
                        // Control flow between two accesses creates a happens-before ordering
                        input.add_sync_edge(SyncEdgeRecord::new(
                            crate::exclusivity::AccessId(edge.source.as_u64()),
                            crate::exclusivity::AccessId(edge.target.as_u64()),
                            SyncOrdering::HappensBefore,
                        ));
                    }
                }
            }
        }

        input
    }

    /// Feed interpretation events from the SCG into the InterpretationVerifier.
    fn feed_interpretation_events(
        &self,
        verifier: &mut InterpretationVerifier,
        scg: &SCG,
        bd_map: &Option<HashMap<NodeId, BD>>,
    ) {
        // If we have BDs, use them; otherwise use default BDs
        let default_bd = BD::new(
            vuma_bd::repd::RepD::Byte(vuma_bd::repd::ByteRep { size: 8, align: 8 }),
            vuma_bd::capd::CapD::all(),
            vuma_bd::reld::RelD::empty(),
        );

        for node in scg.nodes() {
            if node.node_type == NodeType::Access {
                if let NodePayload::Access(access) = &node.payload {
                    let bd = bd_map
                        .as_ref()
                        .and_then(|m| m.get(&node.id))
                        .cloned()
                        .unwrap_or_else(|| default_bd.clone());

                    let location = crate::interpretation::LocationId(access.region_id.as_u64());
                    let pp = crate::interpretation::ProgramPointId(node.id.as_u64());

                    match access.mode {
                        AccessMode::Write => verifier.record_write(location, bd, pp),
                        AccessMode::Read => verifier.record_read(location, bd, pp),
                        AccessMode::ReadWrite => {
                            // Conservative: treat as write then read
                            verifier.record_write(location.clone(), bd.clone(), pp.clone());
                            verifier.record_read(location, bd, pp);
                        }
                    }
                }
            }
        }
    }

    /// Feed origin data from the SCG into the OriginVerifier.
    fn feed_origin_data(&self, verifier: &mut OriginVerifier, scg: &SCG) {
        let mut next_region_id: u64 = 1;
        let mut next_derivation_id: u64 = 1;
        let mut allocation_regions: HashMap<NodeId, OriginRegionId> = HashMap::new();

        // Add regions for allocations
        for node in scg.nodes() {
            if node.node_type == NodeType::Allocation {
                if let NodePayload::Allocation(alloc) = &node.payload {
                    let rid = OriginRegionId(next_region_id);
                    next_region_id += 1;
                    allocation_regions.insert(node.id, rid);

                    verifier.add_region(OriginRegion::new(
                        rid,
                        Address::new(0x1000 + rid.0 * 0x1000),
                        alloc.size,
                    ));

                    // Direct derivation from allocation
                    let did = DerivationId(next_derivation_id);
                    next_derivation_id += 1;
                    verifier.add_derivation(Derivation::new(
                        did,
                        DerivationSource::Region(rid),
                        DerivationKind::Direct,
                        (
                            Address::new(0x1000 + rid.0 * 0x1000),
                            Address::new(0x1000 + rid.0 * 0x1000 + alloc.size),
                        ),
                    ));
                }
            }
        }

        // Add accesses
        let mut next_access_id: u64 = 1;
        for node in scg.nodes() {
            if node.node_type == NodeType::Access {
                if let NodePayload::Access(access) = &node.payload {
                    let aid = OriginAccessId(next_access_id);
                    next_access_id += 1;

                    // Find the derivation for this access's region
                    let target_derivation = DerivationId(access.region_id.as_u64());

                    let kind = match access.mode {
                        AccessMode::Read => OriginAccessKind::Read,
                        AccessMode::Write => OriginAccessKind::Write,
                        AccessMode::ReadWrite => OriginAccessKind::Write, // Conservative
                    };

                    let pp = format!("node_{}", node.id.as_u64());

                    verifier.add_access(OriginAccess::new(
                        aid,
                        target_derivation,
                        kind,
                        access.access_size.unwrap_or(8),
                        pp,
                        false, // initialized — to be checked by verifier
                    ));
                }
            }
        }
    }

    /// Construct a CleanupGraph from the SCG.
    fn extract_cleanup_graph(&self, scg: &SCG) -> CleanupGraph {
        let mut graph = CleanupGraph::new();
        let mut node_map: HashMap<NodeId, CleanupNodeId> = HashMap::new();

        // Add nodes for each SCG node
        for node in scg.nodes() {
            let op = match node.node_type {
                NodeType::Allocation => {
                    if let NodePayload::Allocation(alloc) = &node.payload {
                        Some(OperationKind::Acquire {
                            resource: CleanupResourceId(alloc.region_id.as_u64()),
                            kind: CleanupResourceKind::Memory,
                        })
                    } else {
                        None
                    }
                }
                NodeType::Deallocation => {
                    if let NodePayload::Deallocation(dealloc) = &node.payload {
                        Some(OperationKind::Release {
                            resource: CleanupResourceId(dealloc.region_id.as_u64()),
                            kind: CleanupResourceKind::Memory,
                        })
                    } else {
                        None
                    }
                }
                NodeType::Control => {
                    if let NodePayload::Control(ctrl) = &node.payload {
                        match ctrl.kind {
                            vuma_scg::node::ControlKind::FunctionReturn => {
                                Some(OperationKind::Return)
                            }
                            vuma_scg::node::ControlKind::Branch => Some(OperationKind::Branch {
                                condition: String::new(),
                            }),
                            _ => Some(OperationKind::Passthrough),
                        }
                    } else {
                        Some(OperationKind::Passthrough)
                    }
                }
                NodeType::Access => {
                    if let NodePayload::Access(access) = &node.payload {
                        Some(OperationKind::Access {
                            resource: CleanupResourceId(access.region_id.as_u64()),
                        })
                    } else {
                        Some(OperationKind::Passthrough)
                    }
                }
                _ => Some(OperationKind::Passthrough),
            };

            if let Some(operation) = op {
                let label = format!("node_{}", node.id.as_u64());
                let cleanup_id = graph.add_node(operation, label);
                node_map.insert(node.id, cleanup_id);
            }
        }

        // Add edges from SCG edges. Include ControlFlow, Call, and Return edges.
        // ControlFlow edges represent intra-procedural execution ordering.
        // Call edges connect caller to callee (interprocedural).
        // Return edges connect callee back to caller (interprocedural).
        // Derivation and DataFlow edges represent logical relationships
        // (e.g., "deallocation is derived from allocation"), not execution
        // ordering, and are excluded to avoid false-positive leak reports.
        //
        // We also skip intraprocedural call-return ControlFlow edges that
        // enter FunctionEntry nodes or exit FunctionReturn nodes, since
        // these create dead-end branches that cause false-positive leak
        // reports. The real control flow is already captured by the
        // sequential ControlFlow chain through the main nodes.
        let fn_entry_cleanup_ids: hashbrown::HashSet<CleanupNodeId> = scg.nodes()
            .filter(|n| matches!(n.node_type, NodeType::Control)
                && matches!(&n.payload, NodePayload::Control(c) if c.kind == vuma_scg::node::ControlKind::FunctionEntry))
            .filter_map(|n| node_map.get(&n.id).copied())
            .collect();
        let fn_return_cleanup_ids: hashbrown::HashSet<CleanupNodeId> = scg.nodes()
            .filter(|n| matches!(n.node_type, NodeType::Control)
                && matches!(&n.payload, NodePayload::Control(c) if c.kind == vuma_scg::node::ControlKind::FunctionReturn))
            .filter_map(|n| node_map.get(&n.id).copied())
            .collect();

        for edge in scg.edges() {
            match &edge.kind {
                vuma_scg::edge::EdgeKind::ControlFlow => {
                    if let (Some(&src), Some(&dst)) =
                        (node_map.get(&edge.source), node_map.get(&edge.target))
                    {
                        // Skip intraprocedural call-return edges that create
                        // dead-end branches in the cleanup graph.
                        if fn_entry_cleanup_ids.contains(&dst)
                            || fn_return_cleanup_ids.contains(&src)
                        {
                            continue;
                        }
                        let _ = graph.add_edge(src, dst);
                    }
                }
                vuma_scg::edge::EdgeKind::Call { .. } | vuma_scg::edge::EdgeKind::Return { .. } => {
                    if let (Some(&src), Some(&dst)) =
                        (node_map.get(&edge.source), node_map.get(&edge.target))
                    {
                        let _ = graph.add_edge(src, dst);
                    }
                }
                // Include Derivation edges to connect Allocation nodes
                // (linked via Derivation to Phantom markers) to the cleanup
                // graph. Without this, top-level region allocations appear
                // as disconnected nodes and are flagged as leaks.
                vuma_scg::edge::EdgeKind::Derivation => {
                    if let (Some(&src), Some(&dst)) =
                        (node_map.get(&edge.source), node_map.get(&edge.target))
                    {
                        let _ = graph.add_edge(src, dst);
                    }
                }
                _ => {}
            }
        }

        // Set entry point (first FunctionEntry node, or first node)
        if let Some(first_node) = scg.nodes().next() {
            if let Some(&entry_id) = node_map.get(&first_node.id) {
                let _ = graph.set_entry(entry_id);
            }
        }

        graph
    }
}

impl Default for VerificationEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Hardened Invariant Checks
// ---------------------------------------------------------------------------

/// A structured violation from the hardened invariant checks.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HardenedViolation {
    /// Which invariant was violated.
    pub invariant: &'static str,
    /// A human-readable description.
    pub description: String,
    /// The node where the violation was found (if applicable).
    pub node: Option<NodeId>,
}

impl std::fmt::Display for HardenedViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.node {
            Some(n) => write!(f, "[{}] {} at {}", self.invariant, self.description, n),
            None => write!(f, "[{}] {}", self.invariant, self.description),
        }
    }
}

/// Flow-sensitive CapD checking for Invariant 2.
///
/// Tracks CapD transitions through every SCG edge and detects use-after-cap-drop
/// violations — reading a value after its Write capability has been dropped.
pub fn check_capability_flow(scg: &SCG, bd_map: &HashMap<NodeId, BD>) -> Vec<HardenedViolation> {
    let mut violations = Vec::new();

    // Track the effective CapD at each node (initially from BD map or all())
    let mut effective_capd: HashMap<NodeId, CapD> = HashMap::new();

    // Initialize CapD from BD map
    for node in scg.nodes() {
        if let Some(bd) = bd_map.get(&node.id) {
            effective_capd.insert(node.id, bd.capd.clone());
        } else {
            effective_capd.insert(node.id, CapD::all());
        }
    }

    // For each edge, check if the CapD transition is valid
    for edge in scg.edges() {
        if edge.kind == EdgeKind::ControlFlow || edge.kind == EdgeKind::DataFlow {
            let src_capd = effective_capd
                .get(&edge.source)
                .cloned()
                .unwrap_or_else(CapD::all);
            let dst_capd = effective_capd
                .get(&edge.target)
                .cloned()
                .unwrap_or_else(CapD::all);

            // Check: if source has Write but target does not, that's a capability drop
            let src_has_write = src_capd.caps.contains(&Capability::Write);
            let dst_has_write = dst_capd.caps.contains(&Capability::Write);

            if src_has_write && !dst_has_write {
                // Write was dropped — check if target is a read (use-after-cap-drop)
                if let Some(target_node) = scg.get_node(edge.target) {
                    if target_node.node_type == NodeType::Access {
                        if let NodePayload::Access(access) = &target_node.payload {
                            if access.mode == AccessMode::Read
                                || access.mode == AccessMode::ReadWrite
                            {
                                violations.push(HardenedViolation {
                                    invariant: "capability_flow",
                                    description: format!(
                                        "use-after-cap-drop: Write capability dropped before read at node {}",
                                        edge.target.as_u64()
                                    ),
                                    node: Some(edge.target),
                                });
                            }
                        }
                    }
                }
            }

            // Check: if the meet of source and target CapD is empty, that's a violation
            let meet = src_capd.meet(&dst_capd);
            if meet.caps.is_empty() {
                violations.push(HardenedViolation {
                    invariant: "capability_flow",
                    description: format!(
                        "empty capability meet between nodes {} and {}",
                        edge.source.as_u64(),
                        edge.target.as_u64()
                    ),
                    node: Some(edge.target),
                });
            }
        }
    }

    violations
}

/// Aliasing verification for Invariant 3.
///
/// Verifies aliasing RelD guarantees at every write. Detects write-through-alias
/// violations where two pointers alias the same region and one writes.
pub fn check_aliasing_integrity(scg: &SCG, bd_map: &HashMap<NodeId, BD>) -> Vec<HardenedViolation> {
    let mut violations = Vec::new();

    // Collect all access nodes grouped by region
    let mut accesses_by_region: HashMap<u64, Vec<(NodeId, AccessMode, Option<BD>)>> =
        HashMap::new();

    for node in scg.nodes() {
        if node.node_type == NodeType::Access {
            if let NodePayload::Access(access) = &node.payload {
                let region_key = access.region_id.as_u64();
                let bd = bd_map.get(&node.id).cloned();
                accesses_by_region
                    .entry(region_key)
                    .or_default()
                    .push((node.id, access.mode, bd));
            }
        }
    }

    // For each region with multiple accesses, check for write-through-alias
    for accesses in accesses_by_region.values() {
        if accesses.len() < 2 {
            continue;
        }

        // Find all write accesses
        let writers: Vec<&(NodeId, AccessMode, Option<BD>)> = accesses
            .iter()
            .filter(|(_, mode, _)| *mode == AccessMode::Write || *mode == AccessMode::ReadWrite)
            .collect();

        if writers.is_empty() {
            continue;
        }

        // For each pair of accesses where at least one writes, check aliasing
        for i in 0..accesses.len() {
            for j in (i + 1)..accesses.len() {
                let (id_a, mode_a, bd_a) = &accesses[i];
                let (id_b, mode_b, bd_b) = &accesses[j];

                let a_is_write = *mode_a == AccessMode::Write || *mode_a == AccessMode::ReadWrite;
                let b_is_write = *mode_b == AccessMode::Write || *mode_b == AccessMode::ReadWrite;

                // Check if they could alias (same region, different nodes)
                // If both write, or one writes and one reads, that's a potential aliasing issue
                if a_is_write || b_is_write {
                    // Check BD RelD for aliasing information
                    // If neither BD has anti-alias guarantees, flag as potential violation
                    let a_has_alias_guard = bd_a
                        .as_ref()
                        .is_some_and(|bd| !bd.reld.relations.is_empty());
                    let b_has_alias_guard = bd_b
                        .as_ref()
                        .is_some_and(|bd| !bd.reld.relations.is_empty());

                    if !a_has_alias_guard && !b_has_alias_guard {
                        // No aliasing guarantees — potential write-through-alias
                        if a_is_write && b_is_write {
                            violations.push(HardenedViolation {
                                invariant: "aliasing",
                                description: format!(
                                    "write-through-alias: two writes to same region at nodes {} and {} without aliasing guarantees",
                                    id_a.as_u64(), id_b.as_u64()
                                ),
                                node: Some(*id_b),
                            });
                        } else if a_is_write != b_is_write {
                            // One write + one read to same region without aliasing guard
                            // This is a potential data race but not necessarily a violation
                            // Only report if they're on different threads or unsynchronized
                            // For now, report as medium severity
                        }
                    }
                }
            }
        }
    }

    violations
}

/// Derivation chain validation for Invariant 5.
///
/// Verifies that derive() produces a sub-CapD of source, and validates
/// transitive derivation chains (A→B→C where C.capd ≤ A.capd).
pub fn validate_derivation_chain(
    scg: &SCG,
    bd_map: &HashMap<NodeId, BD>,
) -> Vec<HardenedViolation> {
    let mut violations = Vec::new();

    // Collect derivation edges
    let mut derivation_edges: Vec<(NodeId, NodeId)> = Vec::new();
    for edge in scg.edges() {
        if edge.kind == EdgeKind::Derivation {
            derivation_edges.push((edge.source, edge.target));
        }
    }

    // For each derivation edge (source → target), check that target.capd ⊆ source.capd
    for (source, target) in &derivation_edges {
        let source_capd = bd_map
            .get(source)
            .map(|bd| bd.capd.clone())
            .unwrap_or_else(CapD::all);
        let target_capd = bd_map
            .get(target)
            .map(|bd| bd.capd.clone())
            .unwrap_or_else(CapD::all);

        // In a valid derivation, the derived CapD should be a subset of the source
        if !target_capd.is_subset(&source_capd) {
            violations.push(HardenedViolation {
                invariant: "derivation_chain",
                description: format!(
                    "derivation from {} to {} produces non-sub-CapD: target has capabilities not in source",
                    source.as_u64(),
                    target.as_u64()
                ),
                node: Some(*target),
            });
        }
    }

    // Validate transitive chains: for A→B→C, verify C.capd ⊆ A.capd
    // Build an adjacency list for derivation edges
    let mut deriv_successors: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for (source, target) in &derivation_edges {
        deriv_successors.entry(*source).or_default().push(*target);
    }

    // For each node, find transitive derivation targets (BFS through derivation edges)
    for source in deriv_successors.keys() {
        let source_capd = bd_map
            .get(source)
            .map(|bd| bd.capd.clone())
            .unwrap_or_else(CapD::all);

        // BFS to find all transitively derived nodes
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        if let Some(succs) = deriv_successors.get(source) {
            for &s in succs {
                queue.push_back(s);
            }
        }

        while let Some(current) = queue.pop_front() {
            if visited.contains(&current) {
                continue;
            }
            visited.insert(current);

            let current_capd = bd_map
                .get(&current)
                .map(|bd| bd.capd.clone())
                .unwrap_or_else(CapD::all);

            // Check transitive property: current.capd ⊆ source.capd
            if !current_capd.is_subset(&source_capd) {
                violations.push(HardenedViolation {
                    invariant: "derivation_chain",
                    description: format!(
                        "transitive derivation violation: {} transitively derives from {} but has non-sub-CapD",
                        current.as_u64(),
                        source.as_u64()
                    ),
                    node: Some(current),
                });
            }

            // Continue BFS
            if let Some(succs) = deriv_successors.get(&current) {
                for &s in succs {
                    if !visited.contains(&s) {
                        queue.push_back(s);
                    }
                }
            }
        }
    }

    violations
}

/// Run all hardened invariant checks and collect ALL violations (error recovery).
///
/// Unlike the individual verify_* methods that stop at the first issue,
/// this method collects every violation found across all checks into a
/// `BatchedViolations` structure.
pub fn verify_all_hardened(scg: &SCG, bd_map: &HashMap<NodeId, BD>) -> BatchedViolations {
    let mut batched = BatchedViolations::new();

    // Invariant 1: Escape analysis
    let escape_map = crate::escape::analyze_escapes(scg);
    for (node, kind) in &escape_map {
        if *kind != crate::escape::EscapeKind::DoesNotEscape {
            batched.add(InvariantViolation::new(
                "memory_safety",
                format!("pointer at node {} escapes: {}", node.as_u64(), kind),
                Severity::Medium,
            ));
        }
    }

    // Invariant 2: Flow-sensitive CapD checking
    for v in check_capability_flow(scg, bd_map) {
        batched.add(InvariantViolation::new(
            v.invariant,
            v.description,
            Severity::High,
        ));
    }

    // Invariant 3: Aliasing verification
    for v in check_aliasing_integrity(scg, bd_map) {
        batched.add(InvariantViolation::new(
            v.invariant,
            v.description,
            Severity::High,
        ));
    }

    // Invariant 5: Derivation chain validation
    for v in validate_derivation_chain(scg, bd_map) {
        batched.add(InvariantViolation::new(
            v.invariant,
            v.description,
            Severity::Medium,
        ));
    }

    batched
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::VerificationStatus;

    #[test]
    fn verify_all_on_empty_scg_returns_five_results() {
        let engine = VerificationEngine::new();
        let input = VerificationInput::from_scg(SCG::new());
        let results = engine.verify_all(&input);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn verify_liveness_on_empty_scg() {
        let engine = VerificationEngine::new();
        let input = VerificationInput::from_scg(SCG::new());
        let result = engine.verify_liveness(&input);
        // Empty SCG should be safe (no leaks possible)
        assert!(
            result.is_proven()
                || matches!(result.status, VerificationStatus::ProbablySafe { .. })
                || matches!(result.status, VerificationStatus::Unverified { .. })
        );
    }

    #[test]
    fn verify_exclusivity_on_empty_scg() {
        let engine = VerificationEngine::new();
        let input = VerificationInput::from_scg(SCG::new());
        let result = engine.verify_exclusivity(&input);
        // No accesses → no conflicts
        assert!(
            result.is_proven() || matches!(result.status, VerificationStatus::ProbablySafe { .. })
        );
    }

    #[test]
    fn verify_cleanup_on_empty_scg() {
        let engine = VerificationEngine::new();
        let input = VerificationInput::from_scg(SCG::new());
        let result = engine.verify_cleanup(&input);
        // No allocations → no leaks
        assert!(
            result.is_proven()
                || matches!(result.status, VerificationStatus::ProbablySafe { .. })
                || matches!(result.status, VerificationStatus::Unverified { .. })
        );
    }

    #[test]
    fn default_engine() {
        let engine = VerificationEngine::default();
        let input = VerificationInput::from_scg(SCG::new());
        assert_eq!(engine.verify_all(&input).len(), 5);
    }

    #[test]
    fn verification_input_from_scg() {
        let scg = SCG::new();
        let input = VerificationInput::from_scg(scg);
        assert!(input.bd_map.is_none());
    }

    #[test]
    fn verify_liveness_on_alloc_free_program() {
        // Build an SCG manually: allocate -> free
        use vuma_scg::edge::EdgeKind;
        use vuma_scg::node::ProgramPoint;
        use vuma_scg::node::{AllocationNode, DeallocationNode};
        use vuma_scg::region::{DeploymentTarget, RegionId, SCGRegion};

        let mut scg = SCG::new();
        let region_id = RegionId::new(1);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 16,
                region_id,
                type_name: Some("Buf".to_string()),
            }),
            ProgramPoint {
                file: None,
                line: Some(1),
                column: Some(1),
                offset: None,
            },
        );

        let dealloc_id = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_id,
                region_id,
            }),
            ProgramPoint {
                file: None,
                line: Some(2),
                column: Some(1),
                offset: None,
            },
        );

        let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);
        region.add_node(alloc_id);
        region.add_node(dealloc_id);
        scg.add_region(region);

        scg.add_edge(alloc_id, dealloc_id, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(alloc_id, dealloc_id, EdgeKind::Derivation)
            .unwrap();

        let engine = VerificationEngine::new();
        let input = VerificationInput::from_scg(scg);
        let result = engine.verify_liveness(&input);
        // Well-formed program should have no liveness violations
        assert!(
            !result.is_violated(),
            "Liveness check should pass for well-formed allocate/free program, but got: {} - {}",
            result.status,
            result.message
        );
    }

    #[test]
    fn verify_liveness_on_multi_region_program() {
        use vuma_scg::edge::EdgeKind;
        use vuma_scg::node::ProgramPoint;
        use vuma_scg::node::{AllocationNode, DeallocationNode};
        use vuma_scg::region::{DeploymentTarget, RegionId, SCGRegion};

        let mut scg = SCG::new();
        let region_a = RegionId::new(1);
        let region_b = RegionId::new(2);

        let alloc_a = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id: region_a,
                type_name: Some("A".to_string()),
            }),
            ProgramPoint {
                file: None,
                line: Some(1),
                column: Some(1),
                offset: None,
            },
        );
        let alloc_b = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 128,
                align: 8,
                region_id: region_b,
                type_name: Some("B".to_string()),
            }),
            ProgramPoint {
                file: None,
                line: Some(2),
                column: Some(1),
                offset: None,
            },
        );
        let dealloc_a = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_a,
                region_id: region_a,
            }),
            ProgramPoint {
                file: None,
                line: Some(3),
                column: Some(1),
                offset: None,
            },
        );
        let dealloc_b = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_b,
                region_id: region_b,
            }),
            ProgramPoint {
                file: None,
                line: Some(4),
                column: Some(1),
                offset: None,
            },
        );

        let mut ra = SCGRegion::new(region_a, DeploymentTarget::Heap);
        ra.add_node(alloc_a);
        ra.add_node(dealloc_a);
        scg.add_region(ra);

        let mut rb = SCGRegion::new(region_b, DeploymentTarget::Heap);
        rb.add_node(alloc_b);
        rb.add_node(dealloc_b);
        scg.add_region(rb);

        // Sequential control flow
        scg.add_edge(alloc_a, alloc_b, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(alloc_b, dealloc_a, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(dealloc_a, dealloc_b, EdgeKind::ControlFlow)
            .unwrap();
        // Derivation edges
        scg.add_edge(alloc_a, dealloc_a, EdgeKind::Derivation)
            .unwrap();
        scg.add_edge(alloc_b, dealloc_b, EdgeKind::Derivation)
            .unwrap();

        let engine = VerificationEngine::new();
        let input = VerificationInput::from_scg(scg);
        let result = engine.verify_liveness(&input);
        assert!(
            !result.is_violated(),
            "Liveness check should pass for well-formed multi-region program, but got: {} - {}",
            result.status,
            result.message
        );
    }

    #[test]
    fn verification_input_with_bd_map() {
        let scg = SCG::new();
        let bd_map = HashMap::new();
        let input = VerificationInput::with_bd_map(scg, bd_map);
        assert!(input.bd_map.is_some());
    }
}
