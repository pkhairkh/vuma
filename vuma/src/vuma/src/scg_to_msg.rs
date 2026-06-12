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
//! 1. **Topological walk** — SCG nodes are processed in topological order so
//!    that all predecessors of a node are converted before the node itself.
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
/// - The SCG contains a cycle (topological sort fails).
/// - A node references a region or allocation that doesn't exist.
/// - Post-conversion verification finds broken derivation chains.
pub fn scg_to_msg(scg: &SCG) -> Result<MSG, ConversionError> {
    let mut ctx = ConversionContext::new();

    // Step 1: Topological sort the SCG.
    let sorted_nodes = scg
        .topological_sort()
        .map_err(|_| ConversionError::CycleDetected)?;

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
        | NodeType::ClosureEnv => {
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

    // Find the parent derivation via incoming Derivation edges.
    let parent_deriv_id = find_parent_derivation(scg, ctx, node.id)
        .ok_or(ConversionError::CastWithoutParent(node.id))?;

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

/// Compute the derivation source, kind, and provenance range for an Access
/// node based on its incoming edges and the access payload.
///
/// If the access has a parent derivation (via Derivation/DataFlow edges),
/// the new derivation is chained from it with an optional offset.
/// Otherwise, the derivation originates directly from the region.
fn compute_derivation_for_node(
    scg: &SCG,
    ctx: &ConversionContext,
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
        let msg_region_id = ctx
            .scg_region_to_msg_region
            .get(&access_payload.region_id.as_u64())
            .copied()
            .ok_or(ConversionError::AccessRegionNotFound(
                access_payload.region_id,
            ))?;

        let &(base, size) =
            ctx.region_bounds
                .get(&msg_region_id)
                .ok_or(ConversionError::AccessRegionNotFound(
                    access_payload.region_id,
                ))?;

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
            if !derivation.is_within_bounds() {
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
        AccessMode, AccessNode, AllocationNode, CastNode, ComputationNode, DeallocationNode,
        NodePayload, NodeType, ProgramPoint as ScgPP,
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
                operation: "offset_compute".to_string(),
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
}
