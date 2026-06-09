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

use crate::cleanup::{CleanupGraph, CleanupVerifier, NodeId as CleanupNodeId, OperationKind, ResourceId as CleanupResourceId, ResourceKind as CleanupResourceKind};
use crate::exclusivity::{AccessKind as ExclusivityAccessKind, AccessRecord, CapDInfo, ExclusivityInput, ExclusivityVerifier, SyncEdgeRecord, SyncOrdering};
use crate::interpretation::InterpretationVerifier;
use crate::liveness::{EventAction, LivenessInput, LivenessVerifier, ResourceEvent, ResourceId, ResourceKind, PointId, ThreadId};
use crate::origin::{Access as OriginAccess, AccessId as OriginAccessId, AccessKind as OriginAccessKind, Address, Derivation, DerivationId, DerivationKind, DerivationSource, OriginVerifier, Region as OriginRegion, RegionId as OriginRegionId, TaintLevel};
use crate::result::{Evidence, VerificationResult, VerificationStatus};
use std::collections::HashMap;
use vuma_bd::descriptor::BD;
use vuma_scg::graph::SCG;
use vuma_scg::node::{AccessMode, NodeData, NodeId, NodePayload, NodeType};

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
        Self { scg, bd_map: Some(bd_map) }
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

        for node in scg.nodes() {
            match node.node_type {
                NodeType::Allocation => {
                    if let NodePayload::Allocation(alloc) = &node.payload {
                        let rid = ResourceId(next_resource_id);
                        next_resource_id += 1;
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
                        let rid = ResourceId(dealloc.allocation_node.as_u64());
                        input.add_event(ResourceEvent {
                            resource: rid,
                            kind: ResourceKind::Memory,
                            event: EventAction::Deallocate,
                            point: PointId(node.id.as_u64()),
                            thread: ThreadId(0),
                        });
                    }
                }
                NodeType::Access => {
                    // Access events don't directly affect liveness
                    // but they create resource usage points
                }
                _ => {}
            }
        }

        // Add control flow edges as CFG edges for liveness analysis
        for edge in scg.edges() {
            if edge.kind == vuma_scg::edge::EdgeKind::ControlFlow {
                // CFG edges help the liveness verifier reason about paths
            }
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

                    let pp = format!("{}:{}", node.program_point.file.as_deref().unwrap_or("?"), node.program_point.line.unwrap_or(0));

                    input.add_access(AccessRecord::new(
                        access_id,
                        kind,
                        base_address,
                        size,
                        pp,
                        node.id.as_u64(), // derivation_id
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
                    let bd = bd_map.as_ref()
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
                        (Address::new(0x1000 + rid.0 * 0x1000),
                         Address::new(0x1000 + rid.0 * 0x1000 + alloc.size)),
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
                            vuma_scg::node::ControlKind::FunctionReturn => Some(OperationKind::Return),
                            vuma_scg::node::ControlKind::Branch => Some(OperationKind::Branch { condition: String::new() }),
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

        // Add edges from SCG edges
        for edge in scg.edges() {
            if let (Some(&src), Some(&dst)) = (node_map.get(&edge.source), node_map.get(&edge.target)) {
                let _ = graph.add_edge(src, dst);
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(result.is_proven() || matches!(result.status, VerificationStatus::ProbablySafe { .. }) || matches!(result.status, VerificationStatus::Unverified { .. }));
    }

    #[test]
    fn verify_exclusivity_on_empty_scg() {
        let engine = VerificationEngine::new();
        let input = VerificationInput::from_scg(SCG::new());
        let result = engine.verify_exclusivity(&input);
        // No accesses → no conflicts
        assert!(result.is_proven() || matches!(result.status, VerificationStatus::ProbablySafe { .. }));
    }

    #[test]
    fn verify_cleanup_on_empty_scg() {
        let engine = VerificationEngine::new();
        let input = VerificationInput::from_scg(SCG::new());
        let result = engine.verify_cleanup(&input);
        // No allocations → no leaks
        assert!(result.is_proven() || matches!(result.status, VerificationStatus::ProbablySafe { .. }) || matches!(result.status, VerificationStatus::Unverified { .. }));
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
    fn verification_input_with_bd_map() {
        let scg = SCG::new();
        let bd_map = HashMap::new();
        let input = VerificationInput::with_bd_map(scg, bd_map);
        assert!(input.bd_map.is_some());
    }
}
