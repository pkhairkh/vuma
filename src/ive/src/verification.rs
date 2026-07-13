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
use crate::result::VerificationResult;
use std::collections::HashMap;
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
    /// (Wave 19) Maximum number of paths explored by the liveness verifier
    /// before giving up (default 64). Configurable via `CompileConfig`.
    max_paths: usize,
    /// (Wave 19) Maximum path length explored by the cleanup verifier
    /// before giving up (default 256). Configurable via `CompileConfig`.
    max_path_length: usize,
}

impl Clone for VerificationEngine {
    fn clone(&self) -> Self {
        Self {
            verbose: self.verbose,
            max_paths: self.max_paths,
            max_path_length: self.max_path_length,
        }
    }
}

impl VerificationEngine {
    /// Construct a new verification engine.
    pub fn new() -> Self {
        Self {
            verbose: false,
            max_paths: 64,
            max_path_length: 256,
        }
    }

    /// Enable verbose diagnostic output.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// (Wave 19) Set the maximum number of paths for the liveness verifier.
    pub fn with_max_paths(mut self, max_paths: usize) -> Self {
        self.max_paths = max_paths;
        self
    }

    /// (Wave 19) Set the maximum path length for the cleanup verifier.
    pub fn with_max_path_length(mut self, max_path_length: usize) -> Self {
        self.max_path_length = max_path_length;
        self
    }

    /// (Wave 19) Accessor for the liveness path limit.
    pub fn max_paths(&self) -> usize {
        self.max_paths
    }

    /// (Wave 19) Accessor for the cleanup path-length limit.
    pub fn max_path_length(&self) -> usize {
        self.max_path_length
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
        let mut verifier = LivenessVerifier::new()
            .with_verbose(self.verbose)
            .with_max_paths(self.max_paths);
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
        let verifier = CleanupVerifier::new()
            .with_verbose(self.verbose)
            .with_max_path_length(self.max_path_length);
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
                        // (Wave 19) Mark top-level `region` allocations as
                        // static-lifetime so the liveness leak detector skips
                        // them (spec §5.4). An allocation is static-lifetime
                        // if it has no incoming ControlFlow edge (not
                        // reachable from any function's entry point).
                        let has_ctrlflow_pred = scg.edges().any(|e| {
                            e.target == node.id && matches!(e.kind, EdgeKind::ControlFlow)
                        });
                        if !has_ctrlflow_pred {
                            input.static_lifetime_resources.insert(rid);
                        }
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
                // Include Derivation edges to bridge Allocation and
                // Deallocation nodes into the ControlFlow CFG.
                //
                // The parser emits Allocation/Deallocation nodes OFF the
                // main ControlFlow chain: a Computation node (e.g. the
                // assignment "region = allocate(8)") has a Derivation edge
                // TO the Allocation node, and the Allocation node has a
                // Derivation edge TO the Deallocation node. Without
                // including these Derivation edges in the CFG, the
                // Allocation node is a disconnected dead-end, and
                // `is_reachable(alloc_point, dealloc_point)` returns
                // false — producing a false-positive "Resource leak"
                // report even when the program correctly calls `free()`.
                //
                // This mirrors the same fix already applied to
                // `extract_cleanup_graph` (see lines 617-627 above),
                // which is why the Cleanup invariant passes but the
                // Liveness invariant does not.
                //
                // Derivation edges only connect logically-related nodes
                // (Computation↔Allocation, Allocation↔Deallocation), so
                // they cannot create "spurious shortcut" paths between
                // unrelated resources.
                vuma_scg::edge::EdgeKind::Derivation => {
                    input.add_cfg_edge(crate::liveness::ControlFlowEdge {
                        from: PointId(edge.source.as_u64()),
                        to: PointId(edge.target.as_u64()),
                        conditional: false,
                        label: Some("derivation".to_string()),
                    });
                }
                _ => {}
            }
        }

        // Set entry point to the first node (if any)
        if let Some(first_node) = scg.nodes().next() {
            input.entry_point = Some(PointId(first_node.id.as_u64()));
        }

        // Collect all FunctionReturn node IDs — these are legitimate
        // path exits that should not be flagged as leak endpoints.
        for node in scg.nodes() {
            if node.node_type == NodeType::Control {
                if let NodePayload::Control(ctrl) = &node.payload {
                    if ctrl.kind == vuma_scg::node::ControlKind::FunctionReturn {
                        input.function_returns.insert(PointId(node.id.as_u64()));
                    }
                }
            }
        }

        input
    }

    /// Extract exclusivity-relevant input from the SCG.
    fn extract_exclusivity_input(&self, scg: &SCG) -> ExclusivityInput {
        let mut input = ExclusivityInput::new();

        // Build a map from Access NodeId → allocation NodeId.
        // An Access node is connected to its Allocation via Derivation
        // edges (through the parent Computation node). We BFS backward
        // from each Access node through Derivation edges to find the
        // nearest Allocation node. This gives us a per-allocation ID
        // (the Allocation's NodeId) instead of the coarse region_id,
        // so writes to different allocations in the same region are
        // correctly recognized as non-conflicting.
        use std::collections::{HashSet, VecDeque};
        let mut access_to_alloc: HashMap<u64, u64> = HashMap::new();
        for node in scg.nodes() {
            if node.node_type != NodeType::Access {
                continue;
            }
            // BFS backward through Derivation edges to find Allocation
            let start = node.id.as_u64();
            let mut visited: HashSet<u64> = HashSet::new();
            let mut queue: VecDeque<u64> = VecDeque::new();
            queue.push_back(start);
            visited.insert(start);
            let mut found_alloc: Option<u64> = None;
            while let Some(curr) = queue.pop_front() {
                if let Some(n) = scg.get_node(vuma_scg::node::NodeId::new(curr)) {
                    if n.node_type == NodeType::Allocation && curr != start {
                        found_alloc = Some(curr);
                        break;
                    }
                }
                // Check predecessors via Derivation edges
                for edge in scg.edges() {
                    if edge.target.as_u64() == curr && edge.kind == EdgeKind::Derivation {
                        if visited.insert(edge.source.as_u64()) {
                            queue.push_back(edge.source.as_u64());
                        }
                    }
                }
                // Also check successors via Derivation (Access → Allocation
                // can be forward too)
                for edge in scg.edges() {
                    if edge.source.as_u64() == curr && edge.kind == EdgeKind::Derivation {
                        if visited.insert(edge.target.as_u64()) {
                            queue.push_back(edge.target.as_u64());
                        }
                    }
                }
            }
            if let Some(alloc_id) = found_alloc {
                access_to_alloc.insert(start, alloc_id);
            }
        }

        // First pass: create AccessRecords for all Access nodes.
        // Use the SCG NodeId as the AccessId so that sync edges (which
        // reference nodes by NodeId) correctly match the access records.
        // Use the access's `offset` field (if present) as the base_address
        // so that writes to different offsets within the same buffer
        // (e.g., `*(buf+0)` and `*(buf+1)`) are correctly recognized as
        // non-overlapping.  Previously, base_address was hard-coded to 0,
        // causing all accesses to the same region to appear overlapping.
        //
        // For region_id, use the allocation NodeId (found via Derivation
        // BFS above) instead of the coarse region_id. This ensures that
        // accesses to different allocations in the same region are
        // correctly recognized as non-conflicting.
        let mut access_node_ids: Vec<vuma_scg::node::NodeId> = Vec::new();
        for node in scg.nodes() {
            if node.node_type == NodeType::Access {
                if let NodePayload::Access(access) = &node.payload {
                    let access_id = crate::exclusivity::AccessId(node.id.as_u64());

                    let kind = match access.mode {
                        AccessMode::Read => ExclusivityAccessKind::Read,
                        AccessMode::Write => ExclusivityAccessKind::Write,
                        AccessMode::ReadWrite => ExclusivityAccessKind::Write, // Conservative
                    };

                    let base_address = access.offset.unwrap_or(0);
                    let size = access.access_size.unwrap_or(8);

                    let pp = format!(
                        "{}:{}",
                        node.program_point.file.as_deref().unwrap_or("?"),
                        node.program_point.line.unwrap_or(0)
                    );

                    // Use the allocation NodeId as the region_id for
                    // conflict detection. Fall back to region_id if no
                    // allocation was found (conservative).
                    let alloc_id = access_to_alloc
                        .get(&node.id.as_u64())
                        .copied()
                        .unwrap_or(access.region_id.as_u64());

                    input.add_access(AccessRecord::new(
                        access_id,
                        kind,
                        base_address,
                        size,
                        pp,
                        node.id.as_u64(), // derivation_id
                        alloc_id,         // region_id (per-allocation)
                    ));
                    access_node_ids.push(node.id);
                }
            }
        }

        // Build a reachability map between Access nodes using BOTH
        // ControlFlow and Derivation edges.  Access nodes are connected
        // to the ControlFlow chain only via Derivation edges (from their
        // parent Computation nodes), so a BFS that only follows
        // ControlFlow would never leave an Access node.  We traverse:
        //   - ControlFlow edges (forward, for execution order)
        //   - Derivation edges (bidirectional, to bridge Access nodes
        //     to/from their parent Computation nodes on the ControlFlow
        //     chain)
        // DataFlow edges are excluded — they represent data dependencies,
        // not execution order, and could create spurious orderings.
        // (HashSet and VecDeque already imported above)
        let mut fwd_cf: HashMap<u64, Vec<u64>> = HashMap::new();
        let mut fwd_deriv: HashMap<u64, Vec<u64>> = HashMap::new();
        let mut bwd_deriv: HashMap<u64, Vec<u64>> = HashMap::new();
        for edge in scg.edges() {
            match edge.kind {
                vuma_scg::edge::EdgeKind::ControlFlow => {
                    fwd_cf.entry(edge.source.as_u64()).or_default().push(edge.target.as_u64());
                }
                vuma_scg::edge::EdgeKind::Derivation => {
                    fwd_deriv.entry(edge.source.as_u64()).or_default().push(edge.target.as_u64());
                    bwd_deriv.entry(edge.target.as_u64()).or_default().push(edge.source.as_u64());
                }
                _ => {}
            }
        }

        // For each Access node, BFS through ControlFlow (forward) and
        // Derivation (bidirectional) to find all reachable Access nodes.
        let access_id_set: HashSet<u64> = access_node_ids.iter().map(|n| n.as_u64()).collect();
        for &src_node in &access_node_ids {
            let src_u64 = src_node.as_u64();
            let mut visited: HashSet<u64> = HashSet::new();
            let mut queue: VecDeque<u64> = VecDeque::new();
            // Start by going backward through Derivation to reach the
            // parent Computation node on the ControlFlow chain.
            if let Some(preds) = bwd_deriv.get(&src_u64) {
                for &p in preds {
                    queue.push_back(p);
                }
            }
            while let Some(curr) = queue.pop_front() {
                if !visited.insert(curr) {
                    continue; // Already visited
                }
                if access_id_set.contains(&curr) && curr != src_u64 {
                    // Found a reachable Access node — add sync edge.
                    input.add_sync_edge(SyncEdgeRecord::new(
                        crate::exclusivity::AccessId(src_u64),
                        crate::exclusivity::AccessId(curr),
                        SyncOrdering::HappensBefore,
                    ));
                    // Continue BFS past this Access node (it may bridge
                    // to further Access nodes via its own Derivation
                    // edges).
                }
                // Forward ControlFlow (execution order)
                if let Some(succs) = fwd_cf.get(&curr) {
                    for &s in succs {
                        queue.push_back(s);
                    }
                }
                // Forward Derivation (Computation → Access)
                if let Some(succs) = fwd_deriv.get(&curr) {
                    for &s in succs {
                        queue.push_back(s);
                    }
                }
                // Backward Derivation (Access ← Computation)
                if let Some(preds) = bwd_deriv.get(&curr) {
                    for &p in preds {
                        queue.push_back(p);
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
        // IMPORTANT: Use the allocation's NodeId as the CleanupResourceId,
        // NOT the region_id. Multiple allocations in the same region
        // (which is the common case — all allocations in main() share
        // RegionId(1)) must have distinct resource IDs, otherwise freeing
        // allocation B after freeing allocation A looks like a double-free.
        // Deallocation references its allocation via `allocation_node`,
        // so it uses that NodeId to match the allocation's resource ID.
        for node in scg.nodes() {
            let op = match node.node_type {
                NodeType::Allocation => {
                    if let NodePayload::Allocation(_alloc) = &node.payload {
                        Some(OperationKind::Acquire {
                            resource: CleanupResourceId(node.id.as_u64()),
                            kind: CleanupResourceKind::Memory,
                        })
                    } else {
                        None
                    }
                }
                NodeType::Deallocation => {
                    if let NodePayload::Deallocation(dealloc) = &node.payload {
                        Some(OperationKind::Release {
                            resource: CleanupResourceId(dealloc.allocation_node.as_u64()),
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

        // Spec §5.4 "Global scope / Static lifetime": allocations that
        // live at the top level of a program (not inside any function's
        // control flow) have **static lifetime** and are intentionally
        // leaked — they are released only at program shutdown and MUST
        // NOT be reported as leak violations.
        //
        // Detection heuristic: an SCG `Allocation` node with NO incoming
        // `ControlFlow` edge is not reachable from any function's entry,
        // so it is a top-level / program-init allocation. We mark its
        // `CleanupResourceId` as static-lifetime on the cleanup graph;
        // `CleanupVerifier::dfs_verify` then filters out leak reports
        // for these resources at terminal nodes.
        //
        // This does NOT change behavior for allocations inside function
        // control flow — those still have an incoming `ControlFlow` edge
        // (from the function's entry / preceding computation) and remain
        // subject to the leak invariant.
        for node in scg.nodes() {
            if node.node_type != NodeType::Allocation {
                continue;
            }
            // (Wave 19) An allocation is static-lifetime if it has no
            // incoming ControlFlow edge — i.e., it is not reachable from
            // any function's entry point. This covers top-level `region`
            // declarations (spec §5.4 "Global scope / Static lifetime").
            // Allocations inside function control flow still have an
            // incoming ControlFlow edge and remain subject to the leak
            // invariant.
            let has_ctrlflow_pred = scg.edges().any(|e| {
                e.target == node.id && matches!(e.kind, EdgeKind::ControlFlow)
            });
            if !has_ctrlflow_pred {
                graph.mark_static_lifetime(CleanupResourceId(
                    node.id.as_u64(),
                ));
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

    // Regression test for the IVE false-positive on top-level `region`
    // declarations (see spec §5.4 "Global scope / Static lifetime").
    //
    // A top-level `region` allocation has NO incoming `ControlFlow`
    // edge in the SCG (it is not reachable from any function's entry;
    // only a Derivation edge links it to its Phantom marker). Before
    // the fix, `extract_cleanup_graph` placed it as a disconnected
    // node in the cleanup graph, where `CleanupVerifier` treated it
    // as both a start and a terminal node — and thus reported the
    // program-lifetime allocation as a leak. After the fix, the
    // resource is added to `CleanupGraph::static_lifetime_resources`
    // and leak reports for it are filtered out in `dfs_verify`.
    #[test]
    fn verify_cleanup_top_level_region_not_leaked() {
        use vuma_scg::node::ProgramPoint;
        use vuma_scg::node::AllocationNode;
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

        let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);
        region.add_node(alloc_id);
        scg.add_region(region);

        // Intentionally NO ControlFlow edges: this allocation lives
        // at the top level of the program (not inside any function's
        // control flow), so it has program-lifetime / static lifetime
        // per spec §5.4 and must NOT be flagged as a leak.

        let engine = VerificationEngine::new();
        let input = VerificationInput::from_scg(scg);
        let result = engine.verify_cleanup(&input);
        assert!(
            !result.is_violated(),
            "Top-level `region` declaration must NOT be flagged as a leak \
             (spec §5.4 static lifetime), but got: {} - {}",
            result.status,
            result.message,
        );
    }

    // Companion regression test: an allocation that DOES have an
    // incoming `ControlFlow` edge (i.e., it is inside some function's
    // control flow) is NOT exempt from the leak invariant. If it is
    // never freed, it must still be flagged. This guards against the
    // fix over-applying the static-lifetime exemption.
    #[test]
    fn verify_cleanup_function_local_allocation_still_leaked() {
        use vuma_scg::edge::EdgeKind;
        use vuma_scg::node::ProgramPoint;
        use vuma_scg::node::AllocationNode;
        use vuma_scg::region::{DeploymentTarget, RegionId, SCGRegion};

        let mut scg = SCG::new();
        let region_id = RegionId::new(2);

        // First node: a top-level allocation (no ControlFlow pred).
        // It would normally be static-lifetime-exempt, but we use it
        // here only to provide a ControlFlow predecessor for the
        // second allocation, demonstrating that the second allocation
        // is "inside" control flow and must still be checked.
        let alloc_a_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id,
                type_name: Some("A".to_string()),
            }),
            ProgramPoint {
                file: None,
                line: Some(1),
                column: Some(1),
                offset: None,
            },
        );

        // Second allocation — same region_id would normally be a
        // re-acquire, so use a distinct region to keep resources
        // separate. This allocation HAS a ControlFlow predecessor
        // (alloc_a), so it is NOT static-lifetime and must be
        // flagged as a leak (no dealloc exists for it).
        let region_id_b = RegionId::new(3);
        let alloc_b_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 128,
                align: 8,
                region_id: region_id_b,
                type_name: Some("B".to_string()),
            }),
            ProgramPoint {
                file: None,
                line: Some(2),
                column: Some(1),
                offset: None,
            },
        );

        let mut region_a = SCGRegion::new(region_id, DeploymentTarget::Heap);
        region_a.add_node(alloc_a_id);
        scg.add_region(region_a);
        let mut region_b = SCGRegion::new(region_id_b, DeploymentTarget::Heap);
        region_b.add_node(alloc_b_id);
        scg.add_region(region_b);

        // ControlFlow from A to B: B now has a ControlFlow predecessor
        // and is therefore NOT static-lifetime.
        scg.add_edge(alloc_a_id, alloc_b_id, EdgeKind::ControlFlow)
            .unwrap();

        let engine = VerificationEngine::new();
        let input = VerificationInput::from_scg(scg);
        let result = engine.verify_cleanup(&input);
        assert!(
            result.is_violated(),
            "Function-local allocation (with a ControlFlow predecessor) \
             that is never freed MUST be flagged as a leak, but got: \
             {} - {}",
            result.status,
            result.message,
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
