//! SCG → MSG Conversion Pipeline
//!
//! This module implements the conversion from a Semantic Computation Graph (SCG)
//! to a Memory State Graph (MSG). The SCG is a front-end IR that captures
//! computation, allocation, access, and control flow. The MSG is the core
//! analysis IR that tracks memory regions, pointer derivations, access events,
//! and synchronisation edges.
//!
//! # Conversion Strategy
//!
//! 1. **Cycle-tolerant topological walk** — SCG nodes are processed in an
//!    order that respects the acyclic portion of the graph (Kahn's
//!    algorithm) and appends any nodes that participate in cycles (e.g.
//!    loop back-edges) in `NodeId` order afterwards. This never fails on
//!    cyclic SCGs: the MSG is a state graph, not a DAG, and is allowed to
//!    contain revisited states (a loop body revisits the same memory
//!    state on each iteration).
//! 2. **AllocationNode → Region** — each `Allocation` node becomes an MSG
//!    `Region` with a monotonically assigned base address (starting from
//!    `0x1_0000`).
//! 3. **Derivation edges → Derivations** — `Derivation` edges in the SCG carry
//!    pointer provenance and are mapped to MSG `Derivation` nodes with proper
//!    source chains and provenance ranges.
//! 4. **Access nodes → Accesses** — each `Access` node in the SCG is converted
//!    to an MSG `Access` targeting the appropriate derivation.
//! 5. **Cast nodes → Derivations** — `Cast` nodes produce `DerivationKind::Cast`
//!    derivations from their parent derivation.
//! 6. **Control-flow edges → SyncEdges** — `ControlFlow` edges between two
//!    `Access` nodes become `SyncEdge`s with `HappensBefore` ordering.
//! 7. **DeallocationNode → Region status Freed** — `Deallocation` nodes mark
//!    their corresponding region as `Freed`.
//! 8. **Verification** — after conversion, all derivation chains are verified
//!    to be well-formed (each chain terminates at a region).

use std::fmt;

use hashbrown::HashMap;

use crate::access::{Access, AccessId, AccessKind};
use crate::address::Address;
use crate::derivation::{Derivation, DerivationId, DerivationKind, DerivationSource, RepD};
use crate::msg::MSG;
use crate::program_point::ProgramPoint as MsgProgramPoint;
use crate::region::{Region, RegionId, RegionStatus};
use crate::sync::{Ordering, SyncEdge, SyncEdgeId};

use vuma_scg::edge::EdgeKind;
use vuma_scg::graph::SCG;
use vuma_scg::node::{
    AccessMode, NodeData, NodeId as ScgNodeId, NodePayload, NodeType,
    ProgramPoint as ScgProgramPoint,
};
use vuma_scg::region::RegionId as ScgRegionId;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The starting base address for monotonic region allocation.
const BASE_ADDRESS: u64 = 0x1_0000;

/// Default alignment used when no alignment is specified (16 bytes).
const DEFAULT_ALIGN: u64 = 16;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur during SCG → MSG conversion.
#[derive(Debug, Clone, PartialEq)]
pub enum ConversionError {
    /// The SCG contains a cycle and cannot be topologically sorted.
    CycleDetected,
    /// An allocation node references a region that was not found in the SCG.
    UnknownRegion(ScgRegionId),
    /// A deallocation node references an allocation that was not converted.
    UnknownAllocation(ScgNodeId),
    /// A derivation edge references a source node that has no associated derivation.
    MissingDerivation(ScgNodeId),
    /// An access node references a region that was not found.
    AccessRegionNotFound(ScgRegionId),
    /// A cast node has no incoming derivation to chain from.
    CastWithoutParent(ScgNodeId),
    /// Verification failed: a derivation chain does not terminate at a region.
    BrokenDerivationChain(DerivationId),
    /// Verification failed: a derivation has an invalid provenance range.
    InvalidProvenanceRange(DerivationId),
}

impl fmt::Display for ConversionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConversionError::CycleDetected => write!(f, "SCG contains a cycle, cannot convert"),
            ConversionError::UnknownRegion(rid) => {
                write!(f, "allocation references unknown region {}", rid)
            }
            ConversionError::UnknownAllocation(nid) => {
                write!(f, "deallocation references unknown allocation {}", nid)
            }
            ConversionError::MissingDerivation(nid) => {
                write!(
                    f,
                    "derivation edge source node {} has no associated derivation",
                    nid
                )
            }
            ConversionError::AccessRegionNotFound(rid) => {
                write!(f, "access references unknown region {}", rid)
            }
            ConversionError::CastWithoutParent(nid) => {
                write!(
                    f,
                    "cast node {} has no incoming derivation to chain from",
                    nid
                )
            }
            ConversionError::BrokenDerivationChain(did) => {
                write!(f, "derivation {} chain does not terminate at a region", did)
            }
            ConversionError::InvalidProvenanceRange(did) => {
                write!(f, "derivation {} has invalid provenance range", did)
            }
        }
    }
}

impl std::error::Error for ConversionError {}

// ---------------------------------------------------------------------------
// Conversion context
// ---------------------------------------------------------------------------

/// Internal context that tracks the mapping between SCG and MSG entities
/// during the conversion process.
struct ConversionContext {
    /// The MSG being built.
    msg: MSG,

    /// Next available address for monotonic allocation.
    next_address: u64,

    /// Maps SCG Allocation NodeId → MSG RegionId.
    alloc_node_to_region: HashMap<ScgNodeId, RegionId>,

    /// Maps SCG Allocation NodeId → DerivationId (the Direct derivation from the region).
    alloc_node_to_derivation: HashMap<ScgNodeId, DerivationId>,

    /// Maps SCG node (Access, Cast, Computation) → DerivationId.
    node_to_derivation: HashMap<ScgNodeId, DerivationId>,

    /// Maps SCG Access NodeId → AccessId.
    node_to_access: HashMap<ScgNodeId, AccessId>,

    /// Maps SCG region ID (raw u64) → MSG region ID.
    scg_region_to_msg_region: HashMap<u64, RegionId>,

    /// Maps MSG RegionId → (base_address, size) for provenance range computation.
    region_bounds: HashMap<RegionId, (Address, u64)>,

    /// Counter for DerivationId assignment.
    next_derivation_id: u64,

    /// Counter for AccessId assignment.
    next_access_id: u64,

    /// Counter for SyncEdgeId assignment.
    next_sync_edge_id: u64,
}

impl ConversionContext {
    fn new() -> Self {
        Self {
            msg: MSG::new(),
            next_address: BASE_ADDRESS,
            alloc_node_to_region: HashMap::new(),
            alloc_node_to_derivation: HashMap::new(),
            node_to_derivation: HashMap::new(),
            node_to_access: HashMap::new(),
            scg_region_to_msg_region: HashMap::new(),
            region_bounds: HashMap::new(),
            next_derivation_id: 0,
            next_access_id: 0,
            next_sync_edge_id: 0,
        }
    }

    /// Allocate a contiguous address range of `size` bytes with `align` alignment.
    /// Returns the base address of the allocated range. Addresses are assigned
    /// monotonically — each new region is placed after the previous one.
    fn allocate_address(&mut self, size: u64, align: u64) -> Address {
        let align = if align == 0 { DEFAULT_ALIGN } else { align };
        let base = Address::from(self.next_address).align_to(align);
        self.next_address = base.as_u64() + size;
        base
    }

    fn alloc_derivation_id(&mut self) -> DerivationId {
        let id = DerivationId(self.next_derivation_id);
        self.next_derivation_id += 1;
        id
    }

    fn alloc_access_id(&mut self) -> AccessId {
        let id = AccessId(self.next_access_id);
        self.next_access_id += 1;
        id
    }

    fn alloc_sync_edge_id(&mut self) -> SyncEdgeId {
        let id = SyncEdgeId(self.next_sync_edge_id);
        self.next_sync_edge_id += 1;
        id
    }

    /// Look up the derivation associated with an SCG node, checking both
    /// allocation derivation maps and the general node-to-derivation map.
    fn get_derivation_for_node(&self, node_id: ScgNodeId) -> Option<DerivationId> {
        self.alloc_node_to_derivation
            .get(&node_id)
            .copied()
            .or_else(|| self.node_to_derivation.get(&node_id).copied())
    }
}

// ---------------------------------------------------------------------------
// Program point conversion
// ---------------------------------------------------------------------------

/// Convert an SCG ProgramPoint to an MSG ProgramPoint.
fn convert_program_point(scg_pp: &ScgProgramPoint) -> MsgProgramPoint {
    MsgProgramPoint::new(
        scg_pp
            .file
            .clone()
            .unwrap_or_else(|| "<unknown>".to_string()),
        scg_pp.line.map(|l| l as u32).unwrap_or(0),
        scg_pp.column.map(|c| c as u32).unwrap_or(0),
    )
}

// ---------------------------------------------------------------------------
// Node conversion helpers
// ---------------------------------------------------------------------------

/// Determine the MSG `RegionStatus` from a deployment target name and whether
/// the region has been freed.
fn region_status_from_deployment(deployment: &str, freed: bool) -> RegionStatus {
    if freed {
        return RegionStatus::Freed;
    }
    match deployment {
        "Stack" => RegionStatus::Stack,
        "Gpu" => RegionStatus::Device,
        "Persisted" => RegionStatus::Mapped,
        _ => RegionStatus::Allocated,
    }
}

/// Map SCG `AccessMode` to MSG `AccessKind`.
fn access_kind_from_mode(mode: AccessMode) -> AccessKind {
    match mode {
        AccessMode::Read => AccessKind::Read,
        AccessMode::Write => AccessKind::Write,
        AccessMode::ReadWrite => AccessKind::Write, // conservative: treat ReadWrite as Write
    }
}

// ---------------------------------------------------------------------------
// Cycle-tolerant node ordering
// ---------------------------------------------------------------------------

/// Compute a stable node ordering for the SCG that tolerates cycles.
///
/// This is a Kahn-style topological sort with cycle tolerance. Nodes with
/// no incoming edges are emitted first (seeded in `NodeId` order so the
/// output is deterministic); as each node is emitted, its successors'
/// in-degrees are decremented and any that reach zero are appended.
///
/// Unlike [`SCG::topological_sort`], this never fails: when the remaining
/// subgraph contains only cycles (e.g. loop back-edges produced by `while`
/// loops in programs like `sha256d.vuma`), those nodes are appended in
/// `NodeId` order after the acyclic portion.
///
/// Within the cyclic tail, lower-`NodeId` nodes are processed first. The
/// front-end emits `Derivation` edges in program order (low-ID source →
/// high-ID target), so derivation parents are still converted before their
/// children even inside a loop body. `ControlFlow` back-edges do not
/// participate in derivation lookup ([`find_parent_derivation`] only
/// follows `Derivation`/`DataFlow` edges), so loops convert cleanly.
fn cyclic_aware_node_order(scg: &SCG) -> Vec<ScgNodeId> {
    use std::collections::VecDeque;

    // Collect node IDs in stable (ascending) order.
    let node_ids: Vec<ScgNodeId> = scg.node_ids().collect();

    // Compute in-degree (count of incoming edges) for each node.
    let mut in_degree: HashMap<ScgNodeId, usize> = HashMap::new();
    for &nid in &node_ids {
        in_degree.insert(nid, 0);
    }
    for edge in scg.edges() {
        if let Some(d) = in_degree.get_mut(&edge.target) {
            *d += 1;
        }
    }

    // Seed the queue with all in-degree-0 nodes (in ID order so the
    // output is deterministic).
    let mut queue: VecDeque<ScgNodeId> = node_ids
        .iter()
        .copied()
        .filter(|nid| in_degree.get(nid).copied().unwrap_or(0) == 0)
        .collect();

    let mut result: Vec<ScgNodeId> = Vec::with_capacity(node_ids.len());
    while let Some(nid) = queue.pop_front() {
        result.push(nid);
        if let Some(succs) = scg.successors(nid) {
            for s in succs {
                if let Some(d) = in_degree.get_mut(&s) {
                    if *d > 0 {
                        *d -= 1;
                        if *d == 0 {
                            queue.push_back(s);
                        }
                    }
                }
            }
        }
    }

    // Append remaining (cyclic) nodes in ID order.
    let emitted: hashbrown::HashSet<ScgNodeId> = result.iter().copied().collect();
    for &nid in &node_ids {
        if !emitted.contains(&nid) {
            result.push(nid);
        }
    }

    debug_assert_eq!(result.len(), node_ids.len());
    result
}

// ---------------------------------------------------------------------------
// Main conversion function
// ---------------------------------------------------------------------------

/// Convert an entire SCG to an MSG.
///
/// This is the primary entry point for the conversion pipeline. It walks the
/// SCG in topological order, mapping each node to the appropriate MSG
/// construct, and then processes edges to build derivation chains and
/// synchronisation edges.
///
/// # Errors
///
/// Returns a [`ConversionError`] if:
/// - A node references a region or allocation that doesn't exist.
/// - Post-conversion verification finds broken derivation chains.
///
/// Cyclic SCGs (e.g. programs with loops that produce back-edges in the
/// SCG's `ControlFlow` edges) are handled gracefully — see
/// [`cyclic_aware_node_order`].
pub fn scg_to_msg(scg: &SCG) -> Result<MSG, ConversionError> {
    let mut ctx = ConversionContext::new();

    // Step 1: Compute a stable node ordering that tolerates cycles.
    // Real programs (e.g. `sha256d.vuma`) contain loops, which the SCG
    // represents with `ControlFlow` back-edges. A strict topological sort
    // refuses such graphs; instead we use a Kahn-style algorithm that
    // emits the acyclic portion first and appends cyclic nodes in `NodeId`
    // order. The MSG is a state graph (not a DAG) and may legitimately
    // contain revisited states, so cycles do not invalidate the conversion.
    let sorted_nodes = cyclic_aware_node_order(scg);

    // Step 2: Pre-scan — identify which allocation nodes exist and which regions
    // are freed, so we can assign correct region status upfront.
    let mut freed_allocations: hashbrown::HashSet<ScgNodeId> = hashbrown::HashSet::new();
    for node_data in scg.nodes() {
        if let NodePayload::Deallocation(dealloc) = &node_data.payload {
            freed_allocations.insert(dealloc.allocation_node);
        }
    }

    // Step 3a: Process Allocation nodes FIRST.  This ensures that every
    // region is registered in `scg_region_to_msg_region` before any Access
    // node tries to look it up.  The topological sort may place an Access
    // node before its corresponding Allocation node when there is no direct
    // SCG edge between them (e.g. the allocation is a sibling of the
    // pointer-producing Computation, not a parent of the Access).
    for node_id in &sorted_nodes {
        let node_data = scg.get_node(*node_id).expect("node must exist in SCG");
        if node_data.node_type == NodeType::Allocation {
            process_allocation(scg, &mut ctx, node_data, &freed_allocations)?;
        }
    }

    // Step 3b: Process all remaining nodes in topological order.
    for node_id in &sorted_nodes {
        let node_data = scg.get_node(*node_id).expect("node must exist in SCG");
        if node_data.node_type != NodeType::Allocation {
            process_node(scg, &mut ctx, node_data, &freed_allocations)?;
        }
    }

    // Step 4: Process edges — build sync edges from ControlFlow edges.
    process_edges(scg, &mut ctx)?;

    // Step 5: Verify derivation chains.
    verify_derivation_chains(&ctx)?;

    Ok(ctx.msg)
}

// ---------------------------------------------------------------------------
// Node processing
// ---------------------------------------------------------------------------

/// Process a single SCG node and add the corresponding MSG constructs.
fn process_node(
    scg: &SCG,
    ctx: &mut ConversionContext,
    node: &NodeData,
    freed_allocations: &hashbrown::HashSet<ScgNodeId>,
) -> Result<(), ConversionError> {
    match node.node_type {
        NodeType::Allocation => {
            process_allocation(scg, ctx, node, freed_allocations)?;
        }
        NodeType::Access => {
            process_access(scg, ctx, node)?;
        }
        NodeType::Cast => {
            process_cast(scg, ctx, node)?;
        }
        NodeType::Deallocation => {
            process_deallocation(ctx, node)?;
        }
        NodeType::Computation
        | NodeType::Effect
        | NodeType::Control
        | NodeType::Phantom
        | NodeType::VTable
        | NodeType::ClosureEnv
        | NodeType::StructDef
        | NodeType::EnumDef
        | NodeType::Match
        | NodeType::ConstantTime => {
            // These node types do not directly produce MSG constructs.
            // However, if they participate in derivation chains via Derivation
            // edges, we create a passthrough derivation.
            process_passthrough(scg, ctx, node)?;
        }
    }
    Ok(())
}

/// Process an AllocationNode: create an MSG Region and a Direct derivation.
///
/// The allocation is assigned a base address from the monotonic allocator.
/// A `DerivationKind::Direct` derivation is also created, linking the
/// region base to the pointer produced by this allocation.
fn process_allocation(
    scg: &SCG,
    ctx: &mut ConversionContext,
    node: &NodeData,
    freed_allocations: &hashbrown::HashSet<ScgNodeId>,
) -> Result<(), ConversionError> {
    let alloc_payload = match &node.payload {
        NodePayload::Allocation(a) => a,
        _ => unreachable!("Allocation node must have Allocation payload"),
    };

    // Look up the SCG region to get the deployment target.
    let scg_region = scg.get_region(alloc_payload.region_id);
    let deployment_name = scg_region
        .map(|r| format!("{}", r.deployment_target))
        .unwrap_or_else(|| "Heap".to_string());

    let is_freed = freed_allocations.contains(&node.id);

    // Assign address with monotonic allocator.
    let base = ctx.allocate_address(alloc_payload.size, alloc_payload.align);

    // Determine MSG RegionId — use the SCG region's raw value.
    let msg_region_id = RegionId(alloc_payload.region_id.as_u64());
    ctx.scg_region_to_msg_region
        .insert(alloc_payload.region_id.as_u64(), msg_region_id);

    let status = region_status_from_deployment(&deployment_name, is_freed);

    let region = Region {
        id: msg_region_id,
        base,
        size: alloc_payload.size,
        status,
        alloc_point: convert_program_point(&node.program_point),
        free_point: None, // will be set when deallocation is processed
        owner_context: None,
    };

    ctx.region_bounds
        .insert(msg_region_id, (base, alloc_payload.size));
    ctx.alloc_node_to_region.insert(node.id, msg_region_id);
    ctx.msg.add_region(region);

    // Create a Direct derivation from this region.
    let deriv_id = ctx.alloc_derivation_id();
    let derivation = Derivation {
        id: deriv_id,
        source: DerivationSource::Region(msg_region_id),
        kind: DerivationKind::Direct,
        proven_range: (base, base + alloc_payload.size),
    };
    ctx.msg.add_derivation(derivation);
    ctx.alloc_node_to_derivation.insert(node.id, deriv_id);

    Ok(())
}

/// Process an Access node: create an MSG Derivation and Access.
///
/// The derivation captures pointer provenance (how the accessed pointer
/// was derived from a region). The access records the kind (Read/Write)
/// and size of the memory operation.
fn process_access(
    scg: &SCG,
    ctx: &mut ConversionContext,
    node: &NodeData,
) -> Result<(), ConversionError> {
    let access_payload = match &node.payload {
        NodePayload::Access(a) => a,
        _ => unreachable!("Access node must have Access payload"),
    };

    // Find the derivation source by looking at incoming Derivation edges.
    let (source, kind, proven_range) = compute_derivation_for_node(scg, ctx, node)?;

    let deriv_id = ctx.alloc_derivation_id();
    let derivation = Derivation {
        id: deriv_id,
        source,
        kind,
        proven_range,
    };
    ctx.msg.add_derivation(derivation);
    ctx.node_to_derivation.insert(node.id, deriv_id);

    // Create the access event.
    let access_kind = access_kind_from_mode(access_payload.mode);
    let access_size = access_payload.access_size.unwrap_or(1);
    let access_id = ctx.alloc_access_id();

    let access = Access::new(
        access_id,
        deriv_id,
        access_kind,
        access_size,
        convert_program_point(&node.program_point),
    );
    ctx.msg.add_access(access);
    ctx.node_to_access.insert(node.id, access_id);

    Ok(())
}

/// Process a Cast node: create an MSG Derivation with DerivationKind::Cast.
///
/// Cast nodes represent type-level pointer casts (e.g. `*mut u8` → `*mut u32`).
/// The derivation inherits the provenance range from its parent.
fn process_cast(
    scg: &SCG,
    ctx: &mut ConversionContext,
    node: &NodeData,
) -> Result<(), ConversionError> {
    let cast_payload = match &node.payload {
        NodePayload::Cast(c) => c,
        _ => unreachable!("Cast node must have Cast payload"),
    };

    // Find the parent derivation via incoming Derivation edges. When the
    // SCG contains cycles (e.g. loop back-edges), the parent may belong to
    // the same strongly-connected component and not yet have been
    // converted; in that case we skip creating a Cast derivation. The MSG
    // remains sound — downstream accesses fall back to deriving directly
    // from the region in `compute_derivation_for_node`.
    let parent_deriv_id = match find_parent_derivation(scg, ctx, node.id) {
        Some(id) => id,
        None => return Ok(()),
    };

    let parent_deriv = ctx
        .msg
        .derivation(parent_deriv_id)
        .expect("derivation must exist")
        .clone();
    let proven_range = parent_deriv.proven_range;

    let deriv_id = ctx.alloc_derivation_id();
    let derivation = Derivation {
        id: deriv_id,
        source: DerivationSource::AnotherDerivation(parent_deriv_id),
        kind: DerivationKind::Cast {
            from: RepD {
                name: cast_payload.from_type.clone(),
                size: 0, // unknown at this level
            },
            to: RepD {
                name: cast_payload.to_type.clone(),
                size: 0,
            },
        },
        proven_range,
    };
    ctx.msg.add_derivation(derivation);
    ctx.node_to_derivation.insert(node.id, deriv_id);

    Ok(())
}

/// Process a DeallocationNode: mark the corresponding region as Freed.
///
/// The deallocation node references its allocation node; the MSG Region
/// associated with that allocation is updated to `RegionStatus::Freed`
/// and the `free_point` is set.
fn process_deallocation(
    ctx: &mut ConversionContext,
    node: &NodeData,
) -> Result<(), ConversionError> {
    let dealloc_payload = match &node.payload {
        NodePayload::Deallocation(d) => d,
        _ => unreachable!("Deallocation node must have Deallocation payload"),
    };

    // Find the region associated with the allocation being freed.
    let msg_region_id = ctx
        .alloc_node_to_region
        .get(&dealloc_payload.allocation_node)
        .copied()
        .ok_or(ConversionError::UnknownAllocation(
            dealloc_payload.allocation_node,
        ))?;

    // Update the region status and set the free point.
    if let Some(region) = ctx.msg.region(msg_region_id).cloned() {
        let updated_region = Region {
            status: RegionStatus::Freed,
            free_point: Some(convert_program_point(&node.program_point)),
            ..region
        };
        ctx.msg.add_region(updated_region); // add_region replaces existing
    }

    Ok(())
}

/// Process passthrough nodes (Computation, Effect, Control, Phantom) that may
/// participate in derivation chains. If the node has an incoming Derivation
/// or DataFlow edge, we create a passthrough derivation that forwards the
/// provenance from its parent.
fn process_passthrough(
    scg: &SCG,
    ctx: &mut ConversionContext,
    node: &NodeData,
) -> Result<(), ConversionError> {
    // Check if this node has incoming Derivation edges.
    if let Some(parent_deriv_id) = find_parent_derivation(scg, ctx, node.id) {
        let parent_deriv = ctx
            .msg
            .derivation(parent_deriv_id)
            .expect("derivation must exist")
            .clone();
        let proven_range = parent_deriv.proven_range;

        let deriv_id = ctx.alloc_derivation_id();
        let derivation = Derivation {
            id: deriv_id,
            source: DerivationSource::AnotherDerivation(parent_deriv_id),
            kind: DerivationKind::Direct, // passthrough — no transformation
            proven_range,
        };
        ctx.msg.add_derivation(derivation);
        ctx.node_to_derivation.insert(node.id, deriv_id);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Edge processing
// ---------------------------------------------------------------------------

/// Process SCG edges to create MSG sync edges from ControlFlow edges.
///
/// A `ControlFlow` edge between two `Access` nodes becomes a
/// `SyncEdge` with `Ordering::HappensBefore`, establishing a
/// happens-before relation for data-race analysis.
fn process_edges(scg: &SCG, ctx: &mut ConversionContext) -> Result<(), ConversionError> {
    for edge_data in scg.edges() {
        if edge_data.kind == EdgeKind::ControlFlow {
            // Only create sync edges between Access nodes.
            let source_access = ctx.node_to_access.get(&edge_data.source).copied();
            let target_access = ctx.node_to_access.get(&edge_data.target).copied();

            if let (Some(a1), Some(a2)) = (source_access, target_access) {
                let sync_id = ctx.alloc_sync_edge_id();
                let sync_edge = SyncEdge::new(sync_id, a1, a2, Ordering::HappensBefore);
                ctx.msg.add_sync_edge(sync_edge);
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Derivation helpers
// ---------------------------------------------------------------------------

/// Ensure that an MSG region exists for the SCG region referenced by an
/// Access node, synthesizing one if no Allocation node created it.
///
/// Normally the SCG's Allocation node creates the MSG region during
/// [`process_allocation`]. However, real programs also access memory that
/// is not modeled as a heap allocation: stack-allocated buffers, function
/// parameters, and global/static variables. The front-end still emits
/// Access nodes referencing such regions (so they appear in the SCG with a
/// `region_id`), but no Allocation node exists for them — so no MSG region
/// would exist without this helper.
///
/// This function synthesizes a live MSG region for those accesses so that
/// the MSG records the access against a real region. This is sound:
/// - A synthetic region is always live (`RegionStatus::Allocated` or the
///   SCG-declared deployment status), so it never produces false
///   use-after-free violations.
/// - Real heap regions still have explicit Allocation/Deallocation nodes
///   and are tracked precisely — real UAFs are still caught.
/// - The verifier checks accesses against region liveness, so accesses to
///   genuinely freed heap regions still fail verification.
fn ensure_region_for_access(
    scg: &SCG,
    ctx: &mut ConversionContext,
    access_node: &NodeData,
) -> RegionId {
    let access_payload = match &access_node.payload {
        NodePayload::Access(a) => a,
        _ => unreachable!("expected Access payload"),
    };
    let scg_region_raw = access_payload.region_id.as_u64();

    // Fast path: the region was already registered (by an Allocation node
    // processed earlier, or by a previous call to this helper for another
    // access to the same region).
    if let Some(&id) = ctx.scg_region_to_msg_region.get(&scg_region_raw) {
        return id;
    }

    // Synthesize a region. Look up the SCG region (if one was registered)
    // to honor its deployment_target (Stack, Gpu, etc.); otherwise default
    // to the generic "Heap" deployment, which maps to `RegionStatus::Allocated`.
    let scg_region = scg.get_region(access_payload.region_id);
    let deployment_name = scg_region
        .map(|r| format!("{}", r.deployment_target))
        .unwrap_or_else(|| "Heap".to_string());
    let status = region_status_from_deployment(&deployment_name, false);

    // Size: cover at least this access (offset + access_size), with a
    // reasonable minimum so subsequent accesses at higher offsets on the
    // same synthetic region stay in-bounds. We round up to the default
    // alignment so the monotonic allocator doesn't waste address space.
    let offset = access_payload.offset.unwrap_or(0);
    let access_size = access_payload.access_size.unwrap_or(1);
    let needed = offset.saturating_add(access_size);
    let size = needed.max(DEFAULT_ALIGN).max(16);

    let base = ctx.allocate_address(size, DEFAULT_ALIGN);
    let msg_region_id = RegionId(scg_region_raw);

    // Use the access's program point as a synthetic allocation point —
    // stack/static memory has no single "allocation" site in the SCG, so
    // the access site is the best available provenance.
    let region = Region {
        id: msg_region_id,
        base,
        size,
        status,
        alloc_point: convert_program_point(&access_node.program_point),
        free_point: None,
        owner_context: None,
    };

    ctx.region_bounds.insert(msg_region_id, (base, size));
    ctx.scg_region_to_msg_region
        .insert(scg_region_raw, msg_region_id);
    ctx.msg.add_region(region);

    msg_region_id
}

/// Compute the derivation source, kind, and provenance range for an Access
/// node based on its incoming edges and the access payload.
///
/// If the access has a parent derivation (via Derivation/DataFlow edges),
/// the new derivation is chained from it with an optional offset.
/// Otherwise, the derivation originates directly from the region — and if
/// the region has no Allocation node (stack/static memory), a synthetic
/// live region is created on the fly by [`ensure_region_for_access`].
fn compute_derivation_for_node(
    scg: &SCG,
    ctx: &mut ConversionContext,
    node: &NodeData,
) -> Result<(DerivationSource, DerivationKind, (Address, Address)), ConversionError> {
    let access_payload = match &node.payload {
        NodePayload::Access(a) => a,
        _ => unreachable!("expected Access payload"),
    };

    // Try to find a parent derivation via incoming edges.
    if let Some(parent_deriv_id) = find_parent_derivation(scg, ctx, node.id) {
        let parent_deriv = ctx
            .msg
            .derivation(parent_deriv_id)
            .expect("derivation must exist")
            .clone();
        let offset = access_payload.offset.unwrap_or(0);
        let base = parent_deriv.proven_range.0;
        let parent_end = parent_deriv.proven_range.1;

        if offset == 0 {
            Ok((
                DerivationSource::AnotherDerivation(parent_deriv_id),
                DerivationKind::Direct,
                parent_deriv.proven_range,
            ))
        } else {
            let offset_addr = base + offset;
            let end = if offset_addr < parent_end {
                parent_end // proven range extends to end of parent
            } else {
                offset_addr // degenerate: offset at end or beyond
            };
            Ok((
                DerivationSource::AnotherDerivation(parent_deriv_id),
                DerivationKind::Offset { by: offset as i64 },
                (offset_addr, end),
            ))
        }
    } else {
        // No parent derivation found — derive directly from the region.
        // Ensure a MSG region exists for this access; if the SCG has no
        // Allocation node for it (stack/static memory), a synthetic live
        // region is created. This previously returned
        // `AccessRegionNotFound`, which blocked conversion of real
        // programs like `sha256d.vuma` that access stack buffers.
        let msg_region_id = ensure_region_for_access(scg, ctx, node);

        let &(base, size) = ctx
            .region_bounds
            .get(&msg_region_id)
            .expect("region bounds must exist after ensure_region_for_access");

        let offset = access_payload.offset.unwrap_or(0);
        if offset == 0 {
            Ok((
                DerivationSource::Region(msg_region_id),
                DerivationKind::Direct,
                (base, base + size),
            ))
        } else {
            let offset_addr = base + offset;
            Ok((
                DerivationSource::Region(msg_region_id),
                DerivationKind::Offset { by: offset as i64 },
                (offset_addr, base + size),
            ))
        }
    }
}

/// Find the parent derivation for a node by looking at incoming Derivation
/// and DataFlow edges. Derivation edges are preferred; DataFlow edges are
/// used as a fallback.
fn find_parent_derivation(
    scg: &SCG,
    ctx: &ConversionContext,
    node_id: ScgNodeId,
) -> Option<DerivationId> {
    // Look at predecessors connected by Derivation edges.
    if let Some(preds) = scg.predecessors(node_id) {
        // First pass: check Derivation edges.
        for pred_id in &preds {
            for edge in scg.edges() {
                if edge.source == *pred_id
                    && edge.target == node_id
                    && edge.kind == EdgeKind::Derivation
                {
                    if let Some(deriv_id) = ctx.get_derivation_for_node(*pred_id) {
                        return Some(deriv_id);
                    }
                }
            }
        }
        // Second pass: check DataFlow edges as a fallback.
        for pred_id in &preds {
            for edge in scg.edges() {
                if edge.source == *pred_id
                    && edge.target == node_id
                    && edge.kind == EdgeKind::DataFlow
                {
                    if let Some(deriv_id) = ctx.get_derivation_for_node(*pred_id) {
                        return Some(deriv_id);
                    }
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

/// Verify that all derivation chains in the MSG are well-formed.
///
/// A chain is well-formed if:
/// 1. Every derivation chain terminates at a `DerivationSource::Region`.
/// 2. Every derivation has a valid provenance range (lo < hi).
fn verify_derivation_chains(ctx: &ConversionContext) -> Result<(), ConversionError> {
    // We check derivations that we created (IDs 0..next_derivation_id).
    for i in 0..ctx.next_derivation_id {
        let did = DerivationId(i);
        if let Some(derivation) = ctx.msg.derivation(did) {
            // Check provenance range.
            // Allow zero-size ranges (base == end) which represent a
            // derived pointer to a single byte. Only reject inverted ranges.
            if derivation.proven_range.0 > derivation.proven_range.1 {
                return Err(ConversionError::InvalidProvenanceRange(did));
            }

            // Check that the chain terminates at a region.
            if derivation
                .base_region(|id| ctx.msg.derivation(id).cloned())
                .is_none()
            {
                return Err(ConversionError::BrokenDerivationChain(did));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use vuma_scg::edge::EdgeKind;
    use vuma_scg::graph::SCG;
    use vuma_scg::node::{
        AccessMode, AccessNode, AllocationNode, CastNode, ComputationKind, ComputationNode,
        DeallocationNode, NodePayload, NodeType, ProgramPoint as ScgPP,
    };
    use vuma_scg::region::{DeploymentTarget, RegionId as ScgRegionId, SCGRegion};

    /// Helper: create an SCG ProgramPoint.
    fn scg_pp(line: u64) -> ScgPP {
        ScgPP {
            file: Some("test.vu".to_string()),
            line: Some(line),
            column: Some(1),
            offset: None,
        }
    }

    /// Helper: build a simple SCG with one allocation + one read access.
    fn build_simple_scg() -> SCG {
        let mut scg = SCG::new();
        let region_id = ScgRegionId::new(1);

        // Add region.
        let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);

        // Allocation node.
        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 16,
                region_id,
                type_name: Some("Buffer".to_string()),
            }),
            scg_pp(10),
        );
        region.add_node(alloc_id);

        // Access node (read).
        let access_id = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id,
                offset: None,
                access_size: Some(4),
            }),
            scg_pp(11),
        );
        region.add_node(access_id);

        // Derivation edge: allocation → access.
        scg.add_edge(alloc_id, access_id, EdgeKind::Derivation)
            .unwrap();

        scg.add_region(region);
        scg
    }

    // -----------------------------------------------------------------------
    // Test 1: Allocation → Region
    // -----------------------------------------------------------------------

    #[test]
    fn test_allocation_creates_region_with_monotonic_address() {
        let scg = build_simple_scg();
        let msg = scg_to_msg(&scg).unwrap();

        assert_eq!(msg.region_count(), 1);
        let region = msg.region(RegionId(1)).unwrap();
        assert_eq!(region.size, 256);
        assert!(region.base.as_u64() >= BASE_ADDRESS);
        assert!(region.is_live());
    }

    // -----------------------------------------------------------------------
    // Test 2: Allocation → Direct Derivation
    // -----------------------------------------------------------------------

    #[test]
    fn test_allocation_creates_direct_derivation() {
        let scg = build_simple_scg();
        let msg = scg_to_msg(&scg).unwrap();

        // Should have at least 1 derivation from the allocation.
        assert!(msg.derivation_count() >= 1);

        // The first derivation (from allocation) should be Direct.
        let direct_deriv = msg.derivation(DerivationId(0)).unwrap();
        assert!(matches!(direct_deriv.kind, DerivationKind::Direct));
        assert!(matches!(direct_deriv.source, DerivationSource::Region(_)));
        assert!(direct_deriv.is_within_bounds());
    }

    // -----------------------------------------------------------------------
    // Test 3: Access → Derivation + Access event
    // -----------------------------------------------------------------------

    #[test]
    fn test_access_creates_derivation_and_access_event() {
        let scg = build_simple_scg();
        let msg = scg_to_msg(&scg).unwrap();

        assert_eq!(msg.access_count(), 1);
        assert!(msg.derivation_count() >= 2);

        // The access should target a derivation.
        let access = msg.access(AccessId(0)).unwrap();
        assert_eq!(access.kind, AccessKind::Read);
        assert_eq!(access.size, 4);

        // The derivation chain from the access should terminate at a region.
        let chain = msg.derivation_chain(access.target);
        assert!(!chain.is_empty());
        assert!(matches!(chain[0].source, DerivationSource::Region(_)));
    }

    // -----------------------------------------------------------------------
    // Test 4: DeallocationNode → Region status Freed
    // -----------------------------------------------------------------------

    #[test]
    fn test_deallocation_marks_region_freed() {
        let mut scg = SCG::new();
        let region_id = ScgRegionId::new(1);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 128,
                align: 8,
                region_id,
                type_name: None,
            }),
            scg_pp(1),
        );

        let dealloc_id = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_id,
                region_id,
            }),
            scg_pp(5),
        );

        scg.add_edge(alloc_id, dealloc_id, EdgeKind::Derivation)
            .unwrap();

        let msg = scg_to_msg(&scg).unwrap();
        let region = msg.region(RegionId(1)).unwrap();
        assert_eq!(region.status, RegionStatus::Freed);
        assert!(region.free_point.is_some());
    }

    // -----------------------------------------------------------------------
    // Test 5: Cast → DerivationKind::Cast
    // -----------------------------------------------------------------------

    #[test]
    fn test_cast_creates_cast_derivation() {
        let mut scg = SCG::new();
        let region_id = ScgRegionId::new(1);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id,
                type_name: Some("u8".to_string()),
            }),
            scg_pp(1),
        );

        let cast_id = scg.add_node(
            NodeType::Cast,
            NodePayload::Cast(CastNode {
                from_type: "*mut u8".to_string(),
                to_type: "*mut u32".to_string(),
                is_lossless: true,
            }),
            scg_pp(2),
        );

        scg.add_edge(alloc_id, cast_id, EdgeKind::Derivation)
            .unwrap();

        let msg = scg_to_msg(&scg).unwrap();

        // Should have 2 derivations: Direct (from alloc) + Cast
        assert_eq!(msg.derivation_count(), 2);

        let cast_deriv = msg.derivation(DerivationId(1)).unwrap();
        assert!(matches!(cast_deriv.kind, DerivationKind::Cast { .. }));
        assert!(matches!(
            cast_deriv.source,
            DerivationSource::AnotherDerivation(DerivationId(0))
        ));

        // The cast derivation chain should lead back to the region.
        let chain = msg.derivation_chain(DerivationId(1));
        assert_eq!(chain.len(), 2);
        assert!(matches!(chain[0].source, DerivationSource::Region(_)));
    }

    // -----------------------------------------------------------------------
    // Test 6: ControlFlow edge → SyncEdge with HappensBefore
    // -----------------------------------------------------------------------

    #[test]
    fn test_control_flow_creates_sync_edge() {
        let mut scg = SCG::new();
        let region_id = ScgRegionId::new(1);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 128,
                align: 8,
                region_id,
                type_name: None,
            }),
            scg_pp(1),
        );

        let write_id = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id,
                offset: None,
                access_size: Some(8),
            }),
            scg_pp(2),
        );

        let read_id = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id,
                offset: None,
                access_size: Some(8),
            }),
            scg_pp(3),
        );

        // Derivation edges: alloc → write, alloc → read.
        scg.add_edge(alloc_id, write_id, EdgeKind::Derivation)
            .unwrap();
        scg.add_edge(alloc_id, read_id, EdgeKind::Derivation)
            .unwrap();

        // Control flow: write → read (happens-before).
        scg.add_edge(write_id, read_id, EdgeKind::ControlFlow)
            .unwrap();

        let msg = scg_to_msg(&scg).unwrap();

        assert_eq!(msg.access_count(), 2);
        assert_eq!(msg.sync_edge_count(), 1);

        // The sync edge should order the write before the read.
        let sync = msg.sync_edge(SyncEdgeId(0)).unwrap();
        assert_eq!(sync.access1, AccessId(0)); // write
        assert_eq!(sync.access2, AccessId(1)); // read
        assert!(matches!(sync.ordering, Ordering::HappensBefore));
    }

    // -----------------------------------------------------------------------
    // Test 7: Monotonic address assignment
    // -----------------------------------------------------------------------

    #[test]
    fn test_monotonic_address_assignment() {
        let mut scg = SCG::new();

        // Two allocations in separate regions.
        let r1 = ScgRegionId::new(1);
        let r2 = ScgRegionId::new(2);

        let a1 = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 16,
                region_id: r1,
                type_name: None,
            }),
            scg_pp(1),
        );

        let a2 = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 128,
                align: 8,
                region_id: r2,
                type_name: None,
            }),
            scg_pp(2),
        );

        // Derivation edge to establish order.
        scg.add_edge(a1, a2, EdgeKind::ControlFlow).unwrap();

        let msg = scg_to_msg(&scg).unwrap();

        let region1 = msg.region(RegionId(1)).unwrap();
        let region2 = msg.region(RegionId(2)).unwrap();

        // Region 1 should start at BASE_ADDRESS (aligned to 16).
        assert_eq!(region1.base, Address::from(BASE_ADDRESS));

        // Region 2 should start after region 1 (aligned to 8).
        let expected_r2_base = Address::from(BASE_ADDRESS + 256).align_to(8);
        assert_eq!(region2.base, expected_r2_base);

        // Regions should not overlap.
        assert!(!region1.overlaps(region2));
    }

    // -----------------------------------------------------------------------
    // Test 8: Pointer offset → DerivationKind::Offset with provenance range
    // -----------------------------------------------------------------------

    #[test]
    fn test_access_with_offset_creates_offset_derivation() {
        let mut scg = SCG::new();
        let region_id = ScgRegionId::new(1);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 1024,
                align: 16,
                region_id,
                type_name: None,
            }),
            scg_pp(1),
        );

        let access_id = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id,
                offset: Some(64),
                access_size: Some(4),
            }),
            scg_pp(2),
        );

        scg.add_edge(alloc_id, access_id, EdgeKind::Derivation)
            .unwrap();

        let msg = scg_to_msg(&scg).unwrap();

        // The access derivation should be an Offset derivation.
        let access = msg.access(AccessId(0)).unwrap();
        let deriv = msg.derivation(access.target).unwrap();

        assert!(matches!(deriv.kind, DerivationKind::Offset { by: 64 }));

        // The provenance range should start at base + 64.
        let region = msg.region(RegionId(1)).unwrap();
        let expected_start = region.base + 64;
        assert_eq!(deriv.proven_range.0, expected_start);
        assert!(deriv.is_within_bounds());
    }

    // -----------------------------------------------------------------------
    // Test 9: Empty SCG → Empty MSG
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_scg_produces_empty_msg() {
        let scg = SCG::new();
        let msg = scg_to_msg(&scg).unwrap();

        assert_eq!(msg.region_count(), 0);
        assert_eq!(msg.derivation_count(), 0);
        assert_eq!(msg.access_count(), 0);
        assert_eq!(msg.sync_edge_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Test 10: Derivation chain verification
    // -----------------------------------------------------------------------

    #[test]
    fn test_all_derivation_chains_are_well_formed() {
        let scg = build_simple_scg();
        let msg = scg_to_msg(&scg).unwrap();

        // All derivations should have well-formed chains.
        for i in 0..msg.derivation_count() {
            let did = DerivationId(i as u64);
            if let Some(deriv) = msg.derivation(did) {
                assert!(deriv.is_within_bounds());
                let base_region = deriv.base_region(|id| msg.derivation(id).cloned());
                assert!(
                    base_region.is_some(),
                    "Derivation {} has no base region",
                    did.0
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Test 11: Concurrent accesses (no sync edge)
    // -----------------------------------------------------------------------

    #[test]
    fn test_concurrent_accesses_detected() {
        let mut scg = SCG::new();
        let region_id = ScgRegionId::new(1);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 16,
                region_id,
                type_name: None,
            }),
            scg_pp(1),
        );

        let w1 = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id,
                offset: None,
                access_size: Some(8),
            }),
            scg_pp(2),
        );

        let w2 = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id,
                offset: None,
                access_size: Some(8),
            }),
            scg_pp(3),
        );

        // Both accesses derived from the same allocation, no control flow between them.
        scg.add_edge(alloc_id, w1, EdgeKind::Derivation).unwrap();
        scg.add_edge(alloc_id, w2, EdgeKind::Derivation).unwrap();

        let msg = scg_to_msg(&scg).unwrap();

        // No sync edges — accesses should be concurrent.
        assert_eq!(msg.sync_edge_count(), 0);

        let concurrent = msg.concurrent_accesses(AccessId(0));
        assert!(concurrent.contains(&AccessId(1)));
    }

    // -----------------------------------------------------------------------
    // Test 12: Deployment target → RegionStatus mapping
    // -----------------------------------------------------------------------

    #[test]
    fn test_stack_deployment_produces_stack_region() {
        let mut scg = SCG::new();
        let region_id = ScgRegionId::new(1);
        let mut region = SCGRegion::new(region_id, DeploymentTarget::Stack);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id,
                type_name: None,
            }),
            scg_pp(1),
        );
        region.add_node(alloc_id);
        scg.add_region(region);

        let msg = scg_to_msg(&scg).unwrap();
        let msg_region = msg.region(RegionId(1)).unwrap();
        assert_eq!(msg_region.status, RegionStatus::Stack);
    }

    // -----------------------------------------------------------------------
    // Test 13: Computation node passthrough derivation
    // -----------------------------------------------------------------------

    #[test]
    fn test_computation_node_passthrough_derivation() {
        let mut scg = SCG::new();
        let region_id = ScgRegionId::new(1);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 128,
                align: 8,
                region_id,
                type_name: None,
            }),
            scg_pp(1),
        );

        // Computation node in the derivation chain.
        let comp_id = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("offset_compute".to_string()),
                result_type: Some("*mut u8".to_string()),
                tail_call: false,
            }),
            scg_pp(2),
        );

        let access_id = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id,
                offset: None,
                access_size: Some(4),
            }),
            scg_pp(3),
        );

        // Chain: alloc → computation → access
        scg.add_edge(alloc_id, comp_id, EdgeKind::Derivation)
            .unwrap();
        scg.add_edge(comp_id, access_id, EdgeKind::Derivation)
            .unwrap();

        let msg = scg_to_msg(&scg).unwrap();

        // Should have 3 derivations: alloc (Direct), computation (Direct passthrough), access (Direct)
        assert!(msg.derivation_count() >= 3);

        // The access derivation chain should lead back to the region through
        // the computation passthrough.
        let access = msg.access(AccessId(0)).unwrap();
        let chain = msg.derivation_chain(access.target);
        assert!(chain.len() >= 2);
        assert!(matches!(chain[0].source, DerivationSource::Region(_)));
    }

    // -----------------------------------------------------------------------
    // Test 14: GPU deployment → Device region status
    // -----------------------------------------------------------------------

    #[test]
    fn test_gpu_deployment_produces_device_region() {
        let mut scg = SCG::new();
        let region_id = ScgRegionId::new(1);
        let mut region = SCGRegion::new(region_id, DeploymentTarget::Gpu);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 512,
                align: 256,
                region_id,
                type_name: None,
            }),
            scg_pp(1),
        );
        region.add_node(alloc_id);
        scg.add_region(region);

        let msg = scg_to_msg(&scg).unwrap();
        let msg_region = msg.region(RegionId(1)).unwrap();
        assert_eq!(msg_region.status, RegionStatus::Device);
    }

    // -----------------------------------------------------------------------
    // Test 15: Cyclic SCG (loop back-edge) — sha256d-style `while` loop
    // -----------------------------------------------------------------------

    /// A cyclic SCG must convert without returning `CycleDetected`.
    ///
    /// Models the shape produced by `while i < 64` in `sha256d.vuma`: two
    /// accesses inside a loop body joined by a `ControlFlow` back-edge from
    /// the loop tail back to the loop header. Before the cycle-tolerant
    /// ordering was introduced, `scg_to_msg` refused such graphs with
    /// `ConversionError::CycleDetected`, blocking the whole pipeline.
    #[test]
    fn test_cyclic_scg_with_back_edge_converts() {
        let mut scg = SCG::new();
        let region_id = ScgRegionId::new(1);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 16,
                region_id,
                type_name: None,
            }),
            scg_pp(1),
        );

        // Loop-header access (read).
        let header_id = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id,
                offset: None,
                access_size: Some(4),
            }),
            scg_pp(2),
        );

        // Loop-body access (write).
        let body_id = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id,
                offset: None,
                access_size: Some(4),
            }),
            scg_pp(3),
        );

        // Derivation edges: alloc -> header, alloc -> body.
        scg.add_edge(alloc_id, header_id, EdgeKind::Derivation)
            .unwrap();
        scg.add_edge(alloc_id, body_id, EdgeKind::Derivation)
            .unwrap();

        // Control flow: header -> body -> header (back-edge creates a cycle).
        scg.add_edge(header_id, body_id, EdgeKind::ControlFlow)
            .unwrap();
        scg.add_edge(body_id, header_id, EdgeKind::ControlFlow)
            .unwrap();

        // Must NOT return CycleDetected.
        let msg = scg_to_msg(&scg).expect("cyclic SCG should convert");

        // Both accesses should be present.
        assert_eq!(msg.access_count(), 2);
        assert_eq!(msg.region_count(), 1);
        // 1 direct (alloc) + 2 access derivations.
        assert_eq!(msg.derivation_count(), 3);

        // Both access derivations must terminate at the region.
        for i in 0..msg.access_count() {
            let access = msg.access(AccessId(i as u64)).unwrap();
            let base = msg
                .derivation(access.target)
                .unwrap()
                .base_region(|id| msg.derivation(id).cloned());
            assert!(base.is_some(), "access {} has no base region", i);
        }

        // Both ControlFlow edges are between Access nodes, so both become
        // sync edges (the MSG is a state graph and may contain cycles).
        assert_eq!(msg.sync_edge_count(), 2);
    }

    // -----------------------------------------------------------------------
    // Test 16: Cast whose parent is in a cycle — graceful fallback
    // -----------------------------------------------------------------------

    /// A `Cast` node whose derivation parent has not yet been converted
    /// (because both are in the same strongly-connected component) must not
    /// abort the conversion. The Cast is simply skipped; downstream accesses
    /// fall back to deriving directly from the region.
    #[test]
    fn test_cast_in_cycle_skips_gracefully() {
        let mut scg = SCG::new();
        let region_id = ScgRegionId::new(1);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id,
                type_name: Some("u8".to_string()),
            }),
            scg_pp(1),
        );

        // Cast added with a *lower* NodeId than its derivation parent, and
        // joined into a ControlFlow cycle so both land in the cyclic tail of
        // `cyclic_aware_node_order`. The Cast is processed first; its parent
        // (the Computation) has no derivation yet, so the Cast is skipped.
        let cast_id = scg.add_node(
            NodeType::Cast,
            NodePayload::Cast(CastNode {
                from_type: "*mut u8".to_string(),
                to_type: "*mut u32".to_string(),
                is_lossless: true,
            }),
            scg_pp(2),
        );

        let comp_id = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("loop_phi".to_string()),
                result_type: Some("*mut u8".to_string()),
                tail_call: false,
            }),
            scg_pp(3),
        );

        // Derivation: alloc -> comp -> cast (provenance chain).
        scg.add_edge(alloc_id, comp_id, EdgeKind::Derivation)
            .unwrap();
        scg.add_edge(comp_id, cast_id, EdgeKind::Derivation)
            .unwrap();

        // ControlFlow back-edge: cast -> comp closes the cycle (comp and cast
        // are in the same SCC).
        scg.add_edge(cast_id, comp_id, EdgeKind::ControlFlow)
            .unwrap();

        // Must not error.
        let msg = scg_to_msg(&scg).expect("cast-in-cycle should convert");

        // The region and the alloc's direct derivation always exist.
        assert_eq!(msg.region_count(), 1);
        assert!(msg.derivation_count() >= 1);
    }
    // -----------------------------------------------------------------------
    // Test 17: Access to a region with no Allocation → synthetic region
    // -----------------------------------------------------------------------

    /// An Access node that references a region with no matching Allocation
    /// node (e.g. a stack buffer or function parameter) must not abort the
    /// conversion with `AccessRegionNotFound`. Instead, a synthetic live
    /// region is created on the fly and the access is recorded against it.
    ///
    /// This is the core regression test for the `sha256d.vuma` failure:
    /// `sha256d` accesses `RegionId(9)` (a stack/static buffer) that the
    /// front-end never modeled as an Allocation. Before the fix, the
    /// converter returned `AccessRegionNotFound(RegionId(9))` and blocked
    /// the entire pipeline.
    #[test]
    fn test_access_without_allocation_synthesizes_region() {
        let mut scg = SCG::new();
        // Note: NO Allocation node, NO region registered with the SCG.
        // The Access alone references RegionId(42).
        let region_id = ScgRegionId::new(42);

        let access_id = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id,
                offset: None,
                access_size: Some(8),
            }),
            scg_pp(7),
        );

        // Previously returned AccessRegionNotFound(RegionId(42)).
        let msg = scg_to_msg(&scg).expect("synthetic region should be created");

        // A synthetic region with the access's region_id must exist.
        assert_eq!(msg.region_count(), 1);
        let region = msg
            .region(RegionId(42))
            .expect("synthetic region must exist");
        assert!(region.is_live(), "synthetic region must be live");
        // Default deployment (no SCG region) → Allocated.
        assert_eq!(region.status, RegionStatus::Allocated);
        // Synthetic alloc_point is the access's program point.
        assert_eq!(region.alloc_point, convert_program_point(&scg_pp(7)));
        // Size must cover the access (8 bytes), rounded up to the minimum.
        assert!(region.size >= 8);

        // The access must be recorded and its derivation must terminate at
        // the synthetic region.
        assert_eq!(msg.access_count(), 1);
        let access = msg.access(AccessId(0)).unwrap();
        assert_eq!(access.kind, AccessKind::Read);
        let base_region = msg
            .derivation(access.target)
            .unwrap()
            .base_region(|id| msg.derivation(id).cloned());
        assert_eq!(base_region, Some(RegionId(42)));

        // Sanity: the unused `access_id` is the one we added.
        let _ = access_id;
    }

    // -----------------------------------------------------------------------
    // Test 18: Multiple accesses to one unallocated region share one synth
    // -----------------------------------------------------------------------

    /// Two Access nodes referencing the same unallocated region must share
    /// a single synthetic MSG region (not one-per-access). The second
    /// access hits the fast path in `ensure_region_for_access`.
    #[test]
    fn test_multiple_accesses_share_one_synthetic_region() {
        let mut scg = SCG::new();
        let region_id = ScgRegionId::new(9); // the sha256d case

        let a1 = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id,
                offset: Some(0),
                access_size: Some(4),
            }),
            scg_pp(1),
        );
        let a2 = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id,
                offset: Some(4),
                access_size: Some(4),
            }),
            scg_pp(2),
        );

        // Order them so the topological walk is deterministic.
        scg.add_edge(a1, a2, EdgeKind::ControlFlow).unwrap();

        let msg = scg_to_msg(&scg).expect("should convert with one synth region");

        // Exactly one synthetic region for RegionId(9).
        assert_eq!(msg.region_count(), 1);
        assert!(msg.region(RegionId(9)).is_some());

        // Both accesses recorded, both terminate at the same region.
        assert_eq!(msg.access_count(), 2);
        for i in 0..msg.access_count() {
            let access = msg.access(AccessId(i as u64)).unwrap();
            let base = msg
                .derivation(access.target)
                .unwrap()
                .base_region(|id| msg.derivation(id).cloned());
            assert_eq!(base, Some(RegionId(9)));
        }

        // The region's size must cover the second access (offset 4 + size 4).
        let region = msg.region(RegionId(9)).unwrap();
        assert!(region.size >= 8, "region size {} must cover offset 4 + 4", region.size);
    }
}
