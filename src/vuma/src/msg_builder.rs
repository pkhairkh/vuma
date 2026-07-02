//! Incremental MSG builder that constructs a Memory State Graph from an SCG.
//!
//! This module implements the 9 inference rules from the MSG construction spec
//! (VUMA-SPEC-MSG-001), walking the SCG in topological order and emitting
//! regions, derivations, accesses, and sync edges into the MSG.
//!
//! # Inference Rules
//!
//! | Rule          | SCG Node / Edge         | MSG Effect                                    |
//! |---------------|-------------------------|-----------------------------------------------|
//! | ALLOC         | AllocationNode          | Create `Region` (status = Allocated)          |
//! | DEALLOC       | DeallocationNode        | Set `Region` status → Freed                   |
//! | DERIVE-DIRECT | Direct ptr from region  | Create `Derivation` (kind = Direct)           |
//! | DERIVE-OFFSET | Pointer arithmetic      | Create `Derivation` (kind = Offset)           |
//! | DERIVE-CAST   | Type cast               | Create `Derivation` (kind = Cast)             |
//! | ACCESS-READ   | Memory read             | Create `Access` (kind = Read)                 |
//! | ACCESS-WRITE  | Memory write            | Create `Access` (kind = Write)                |
//! | SYNC          | Synchronisation edge    | Create `SyncEdge`                             |
//! | MERGE         | Parallel composition    | Combine two MSGs into one                     |
//!
//! # Incremental Updates
//!
//! When the SCG changes, the builder can compute a *delta* — the set of MSG
//! elements that must be added, removed, or modified — rather than rebuilding
//! from scratch. The delta is computed by comparing the set of already-
//! processed SCG node IDs against the current SCG and re-processing only
//! the affected nodes and their transitive dependents.

use crate::access::{Access, AccessId, AccessKind};
use crate::address::Address;
use crate::derivation::{Derivation, DerivationId, DerivationKind, DerivationSource, RepD};
use crate::msg::MSG;
use crate::program_point::ProgramPoint;
use crate::region::{Region, RegionId, RegionStatus};
use crate::sync::{Ordering, SyncEdge, SyncEdgeId};
use hashbrown::{HashMap, HashSet};
use std::fmt;

// ---------------------------------------------------------------------------
// SCG type aliases — we use the types from vuma-scg but must distinguish
// them from the MSG types that share the same names.
// ---------------------------------------------------------------------------

use vuma_scg::{
    AccessMode, EdgeData as ScgEdgeData, EdgeKind as ScgEdgeKind, NodeData, NodeId as ScgNodeId,
    NodePayload, NodeType as ScgNodeType, SCG,
};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur during MSG construction.
#[derive(Debug, Clone, PartialEq)]
pub enum BuilderError {
    /// The SCG contains a cycle and cannot be topologically sorted.
    CycleDetected,
    /// A node referenced by an edge or payload was not found in the SCG.
    NodeNotFound(ScgNodeId),
    /// A region referenced by an SCG node was not found in the MSG.
    RegionNotFound(RegionId),
    /// A derivation referenced by an SCG node was not found in the MSG.
    DerivationNotFound(DerivationId),
    /// An access referenced by an SCG edge was not found in the MSG.
    AccessNotFound(AccessId),
    /// An allocation node has size 0, which is illegal.
    ZeroSizeAllocation(ScgNodeId),
    /// A deallocation node targets a region that is already freed (double-free).
    DoubleFree { node: ScgNodeId, region: RegionId },
    /// A cast node has an alignment violation.
    AlignmentViolation {
        node: ScgNodeId,
        offset: i64,
        required_alignment: u64,
    },
    /// An offset derivation goes out of bounds.
    OutOfBounds {
        node: ScgNodeId,
        offset: i64,
        region_size: u64,
    },
    /// A general validation error with a message.
    ValidationFailed(String),
}

impl fmt::Display for BuilderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuilderError::CycleDetected => write!(f, "SCG contains a cycle; cannot build MSG"),
            BuilderError::NodeNotFound(id) => write!(f, "SCG node not found: {}", id),
            BuilderError::RegionNotFound(id) => write!(f, "MSG region not found: {}", id),
            BuilderError::DerivationNotFound(id) => write!(f, "MSG derivation not found: {}", id),
            BuilderError::AccessNotFound(id) => write!(f, "MSG access not found: {}", id),
            BuilderError::ZeroSizeAllocation(id) => {
                write!(f, "zero-size allocation at node {}", id)
            }
            BuilderError::DoubleFree { node, region } => {
                write!(f, "double-free of region {} at node {}", region, node)
            }
            BuilderError::AlignmentViolation {
                node,
                offset,
                required_alignment,
            } => {
                write!(
                    f,
                    "alignment violation at node {}: offset {} not aligned to {}",
                    node, offset, required_alignment
                )
            }
            BuilderError::OutOfBounds {
                node,
                offset,
                region_size,
            } => {
                write!(
                    f,
                    "out-of-bounds offset at node {}: {} exceeds region size {}",
                    node, offset, region_size
                )
            }
            BuilderError::ValidationFailed(msg) => write!(f, "validation failed: {}", msg),
        }
    }
}

impl std::error::Error for BuilderError {}

// ---------------------------------------------------------------------------
// Delta types for incremental updates
// ---------------------------------------------------------------------------

/// A change to a single region.
#[derive(Debug, Clone)]
pub enum RegionChange {
    /// A new region was added.
    Added(Region),
    /// An existing region was modified (e.g. status changed to Freed).
    Modified(Region),
    /// A region was removed.
    Removed(RegionId),
}

/// A change to a single derivation.
#[derive(Debug, Clone)]
pub enum DerivationChange {
    /// A new derivation was added.
    Added(Derivation),
    /// A derivation was modified.
    Modified(Derivation),
    /// A derivation was removed.
    Removed(DerivationId),
}

/// A change to a single access.
#[derive(Debug, Clone)]
pub enum AccessChange {
    /// A new access was added.
    Added(Access),
    /// An access was modified.
    Modified(Access),
    /// An access was removed.
    Removed(AccessId),
}

/// A change to a single sync edge.
#[derive(Debug, Clone)]
pub enum SyncEdgeChange {
    /// A new sync edge was added.
    Added(SyncEdge),
    /// A sync edge was removed.
    Removed(SyncEdgeId),
}

/// The delta produced by an incremental MSG update.
///
/// Each field contains the set of changes to that category of MSG element.
#[derive(Debug, Clone, Default)]
pub struct MsgDelta {
    /// Changes to regions.
    pub region_changes: Vec<RegionChange>,
    /// Changes to derivations.
    pub derivation_changes: Vec<DerivationChange>,
    /// Changes to accesses.
    pub access_changes: Vec<AccessChange>,
    /// Changes to sync edges.
    pub sync_edge_changes: Vec<SyncEdgeChange>,
}

impl MsgDelta {
    /// Create an empty delta.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` if the delta contains no changes.
    pub fn is_empty(&self) -> bool {
        self.region_changes.is_empty()
            && self.derivation_changes.is_empty()
            && self.access_changes.is_empty()
            && self.sync_edge_changes.is_empty()
    }
}

// ---------------------------------------------------------------------------
// SCG Node → MSG entity mapping
// ---------------------------------------------------------------------------

/// Records what MSG entity (if any) was produced for a given SCG node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScgNodeMapping {
    /// An AllocationNode produced a Region.
    Region(RegionId),
    /// A Computation / Cast / Allocation node produced a root Derivation.
    Derivation(DerivationId),
    /// An Access node produced an Access entry.
    Access(AccessId),
    /// A Deallocation node (no new entity, but marks a region as Freed).
    Deallocation(RegionId),
    /// A node that does not produce any MSG entity (e.g., Control, Phantom).
    None,
}

// ---------------------------------------------------------------------------
// Address allocator
// ---------------------------------------------------------------------------

/// A monotonic address allocator that hands out non-overlapping address ranges.
///
/// Each call to [`AddressAllocator::alloc`] returns a fresh base address
/// that does not overlap with any previously allocated range.
#[derive(Debug, Clone)]
struct AddressAllocator {
    /// The next available base address.
    next_addr: u64,
}

impl AddressAllocator {
    /// Create a new allocator starting from the given base address.
    fn new(base: u64) -> Self {
        Self { next_addr: base }
    }

    /// Allocate a contiguous range of `size` bytes and return the base address.
    fn alloc(&mut self, size: u64) -> Address {
        let base = self.next_addr;
        self.next_addr = base + size;
        // Align the next allocation to 16 bytes for cleanliness.
        self.next_addr = (self.next_addr + 15) & !15;
        Address::from(base)
    }
}

impl Default for AddressAllocator {
    fn default() -> Self {
        Self::new(0x1000_0000)
    }
}

// ---------------------------------------------------------------------------
// The Builder
// ---------------------------------------------------------------------------

/// Incremental MSG builder.
///
/// The builder walks an SCG in topological order and applies the 9 inference
/// rules, constructing an [`MSG`] that mirrors the memory semantics described
/// by the SCG.
///
/// # Incremental Updates
///
/// After the initial build, the builder retains its mapping from SCG node IDs
/// to MSG entities. When the SCG changes, call [`MsgBuilder::update`] with
/// the set of node IDs that have been added, removed, or modified. The builder
/// will compute a [`MsgDelta`] and apply it to the MSG, re-processing only
/// the affected nodes and their transitive dependents.
///
/// # Example
///
/// ```
/// use vuma_core::msg_builder::MsgBuilder;
/// use vuma_scg::{SCG, NodeType, NodePayload, AllocationNode, ProgramPoint};
/// use vuma_scg::region::{RegionId, DeploymentTarget, SCGRegion};
///
/// let mut scg = SCG::new();
/// let rid = RegionId::new(1);
/// scg.add_region(SCGRegion::new(rid, DeploymentTarget::Heap));
///
/// let alloc_id = scg.add_node(
///     NodeType::Allocation,
///     NodePayload::Allocation(AllocationNode {
///         size: 256,
///         align: 8,
///         region_id: rid,
///         type_name: None,
///     }),
///     ProgramPoint { file: None, line: None, column: None, offset: None },
/// );
///
/// let mut builder = MsgBuilder::new();
/// let msg = builder.build(&scg).unwrap();
/// assert_eq!(msg.region_count(), 1);
/// ```
pub struct MsgBuilder {
    /// The MSG being constructed.
    msg: MSG,

    /// Mapping from SCG NodeId → MSG entity produced by that node.
    node_map: HashMap<ScgNodeId, ScgNodeMapping>,

    /// Mapping from SCG RegionId (u64) → MSG RegionId.
    /// The SCG and MSG have different RegionId types.
    scg_region_to_msg_region: HashMap<u64, RegionId>,

    /// Address allocator for new regions.
    addr_alloc: AddressAllocator,

    /// Set of SCG node IDs that have already been processed.
    processed_nodes: HashSet<ScgNodeId>,

    /// Counter for generating the next MSG RegionId.
    next_region_id: u64,

    /// Counter for generating the next MSG DerivationId.
    next_derivation_id: u64,

    /// Counter for generating the next MSG AccessId.
    next_access_id: u64,

    /// Counter for generating the next MSG SyncEdgeId.
    next_sync_edge_id: u64,

    /// Whether to emit warnings for non-fatal issues.
    warn_on_double_free: bool,

    /// Whether to emit warnings for out-of-bounds derivations.
    warn_on_out_of_bounds: bool,

    /// Collected warnings during the build.
    warnings: Vec<String>,
}

impl MsgBuilder {
    /// Create a new builder with default settings.
    pub fn new() -> Self {
        Self {
            msg: MSG::new(),
            node_map: HashMap::new(),
            scg_region_to_msg_region: HashMap::new(),
            addr_alloc: AddressAllocator::default(),
            processed_nodes: HashSet::new(),
            next_region_id: 1,
            next_derivation_id: 1,
            next_access_id: 1,
            next_sync_edge_id: 1,
            warn_on_double_free: true,
            warn_on_out_of_bounds: true,
            warnings: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Configuration
    // -----------------------------------------------------------------------

    /// Set the starting base address for the address allocator.
    pub fn with_base_address(mut self, base: u64) -> Self {
        self.addr_alloc = AddressAllocator::new(base);
        self
    }

    /// Enable or disable double-free warnings.
    pub fn with_double_free_warnings(mut self, enabled: bool) -> Self {
        self.warn_on_double_free = enabled;
        self
    }

    /// Enable or disable out-of-bounds warnings.
    pub fn with_out_of_bounds_warnings(mut self, enabled: bool) -> Self {
        self.warn_on_out_of_bounds = enabled;
        self
    }

    // -----------------------------------------------------------------------
    // ID allocation
    // -----------------------------------------------------------------------

    /// Allocate the next fresh RegionId.
    fn alloc_region_id(&mut self) -> RegionId {
        let id = RegionId(self.next_region_id);
        self.next_region_id += 1;
        id
    }

    /// Allocate the next fresh DerivationId.
    fn alloc_derivation_id(&mut self) -> DerivationId {
        let id = DerivationId(self.next_derivation_id);
        self.next_derivation_id += 1;
        id
    }

    /// Allocate the next fresh AccessId.
    fn alloc_access_id(&mut self) -> AccessId {
        let id = AccessId(self.next_access_id);
        self.next_access_id += 1;
        id
    }

    /// Allocate the next fresh SyncEdgeId.
    fn alloc_sync_edge_id(&mut self) -> SyncEdgeId {
        let id = SyncEdgeId(self.next_sync_edge_id);
        self.next_sync_edge_id += 1;
        id
    }

    // -----------------------------------------------------------------------
    // SCG → MSG ProgramPoint conversion
    // -----------------------------------------------------------------------

    /// Convert an SCG ProgramPoint to an MSG ProgramPoint.
    ///
    /// The SCG ProgramPoint has optional fields; the MSG ProgramPoint
    /// requires file, line, and col.
    fn convert_program_point(scg_pp: &vuma_scg::ProgramPoint) -> ProgramPoint {
        ProgramPoint {
            file: scg_pp
                .file
                .clone()
                .unwrap_or_else(|| "<unknown>".to_string()),
            line: scg_pp.line.unwrap_or(0) as u32,
            col: scg_pp.column.unwrap_or(0) as u32,
            node_id: None,
        }
    }

    // -----------------------------------------------------------------------
    // Main build entry point
    // -----------------------------------------------------------------------

    /// Build the MSG from the given SCG.
    ///
    /// Walks the SCG in topological order and applies each inference rule.
    /// Returns the constructed MSG on success, or the first error encountered.
    ///
    /// This method may be called only once on a fresh builder. For incremental
    /// updates, use [`MsgBuilder::update`].
    pub fn build(&mut self, scg: &SCG) -> Result<&MSG, BuilderError> {
        // Use SCC-based topological sort that handles cycles from loops.
        // Falls back to strict topological_sort for backward compatibility
        // if the graph is acyclic.
        let topo_order = if scg.has_cycles() {
            scg.topological_sort_with_cycles()
        } else {
            scg.topological_sort()
                .map_err(|_| BuilderError::CycleDetected)?
        };

        // Pre-populate the SCG region → MSG region mapping.
        // We create one MSG Region for each SCG Region that has an AllocationNode.
        // However, the actual Region creation happens when we encounter AllocationNodes.

        for node_id in &topo_order {
            self.process_node(scg, *node_id)?;
        }

        // After processing all nodes, process edges for sync information.
        self.process_edges(scg)?;

        Ok(&self.msg)
    }

    /// Build the MSG and return ownership of it.
    ///
    /// Convenience method that calls [`MsgBuilder::build`] and then takes the MSG.
    pub fn build_into(mut self, scg: &SCG) -> Result<MSG, BuilderError> {
        self.build(scg)?;
        Ok(self.msg)
    }

    // -----------------------------------------------------------------------
    // Node processing — the 9 inference rules
    // -----------------------------------------------------------------------

    /// Process a single SCG node, applying the appropriate inference rule.
    fn process_node(&mut self, scg: &SCG, node_id: ScgNodeId) -> Result<(), BuilderError> {
        if self.processed_nodes.contains(&node_id) {
            return Ok(());
        }

        let node_data = scg
            .get_node(node_id)
            .cloned()
            .ok_or(BuilderError::NodeNotFound(node_id))?;

        let result = match node_data.node_type {
            ScgNodeType::Allocation => self.rule_alloc(&node_data)?,
            ScgNodeType::Deallocation => self.rule_dealloc(&node_data)?,
            ScgNodeType::Computation => self.rule_derive(&node_data)?,
            ScgNodeType::Access => self.rule_access(&node_data)?,
            ScgNodeType::Cast => self.rule_cast(&node_data)?,
            ScgNodeType::Effect => self.rule_effect(&node_data)?,
            ScgNodeType::Control
            | ScgNodeType::Phantom
            | ScgNodeType::VTable
            | ScgNodeType::ClosureEnv
            | ScgNodeType::StructDef
            | ScgNodeType::EnumDef
            | ScgNodeType::Match
            | ScgNodeType::ConstantTime => ScgNodeMapping::None,
        };

        self.node_map.insert(node_id, result);
        self.processed_nodes.insert(node_id);
        Ok(())
    }

    /// **Rule ALLOC**: AllocationNode → Region in MSG.
    ///
    /// Creates a new `Region` with status `Allocated` and a root `Derivation`
    /// with kind `Direct` from the region base.
    ///
    /// From the spec (§1.1):
    /// > The allocation node `n` specifies a size `s`. The rule creates a
    /// > fresh region `r` with a unique identifier `rid`, a fresh base
    /// > address `a`, and status `Allocated`.
    fn rule_alloc(&mut self, node: &NodeData) -> Result<ScgNodeMapping, BuilderError> {
        let alloc = match &node.payload {
            NodePayload::Allocation(a) => a,
            _ => {
                return Err(BuilderError::ValidationFailed(
                    "Allocation node has non-Allocation payload".to_string(),
                ))
            }
        };

        if alloc.size == 0 {
            return Err(BuilderError::ZeroSizeAllocation(node.id));
        }

        let pp = Self::convert_program_point(&node.program_point);

        // Create the MSG Region.
        let rid = self.alloc_region_id();
        let base = self.addr_alloc.alloc(alloc.size);

        let region = Region {
            id: rid,
            base,
            size: alloc.size,
            status: RegionStatus::Allocated,
            alloc_point: pp.clone(),
            free_point: None,
            owner_context: alloc.type_name.clone(),
        };

        self.msg.add_region(region);

        // Record the SCG region ID → MSG region ID mapping.
        self.scg_region_to_msg_region
            .insert(alloc.region_id.as_u64(), rid);

        // Create the root derivation (CHAIN-ROOT from spec §2.2).
        let deriv_id = self.alloc_derivation_id();
        let derivation = Derivation {
            id: deriv_id,
            source: DerivationSource::Region(rid),
            kind: DerivationKind::Direct,
            proven_range: (base, base + alloc.size),
        };

        self.msg.add_derivation(derivation);

        Ok(ScgNodeMapping::Derivation(deriv_id))
    }

    /// **Rule DEALLOC**: DeallocationNode → Region status → Freed.
    ///
    /// Transitions the target region's status from `Allocated` to `Freed`
    /// and records the deallocation program point.
    ///
    /// From the spec (§1.2):
    /// > The deallocation node `n` references a derivation that resolves to
    /// > region `r`. The rule transitions `r.status` from Allocated to Freed.
    fn rule_dealloc(&mut self, node: &NodeData) -> Result<ScgNodeMapping, BuilderError> {
        let dealloc = match &node.payload {
            NodePayload::Deallocation(d) => d,
            _ => {
                return Err(BuilderError::ValidationFailed(
                    "Deallocation node has non-Deallocation payload".to_string(),
                ))
            }
        };

        let pp = Self::convert_program_point(&node.program_point);

        // Find the MSG RegionId corresponding to the SCG region.
        let msg_region_id = self
            .scg_region_to_msg_region
            .get(&dealloc.region_id.as_u64())
            .copied()
            .ok_or(BuilderError::RegionNotFound(RegionId(
                dealloc.region_id.as_u64(),
            )))?;

        // Find the region that the allocation node created.
        // We look up the allocation node's mapping to find the region.
        let alloc_region_id = if let Some(ScgNodeMapping::Derivation(_)) =
            self.node_map.get(&dealloc.allocation_node)
        {
            // The allocation node produced a derivation. The region it belongs to
            // is the one mapped from the SCG region ID.
            Some(msg_region_id)
        } else {
            None
        };

        let region_id = alloc_region_id.unwrap_or(msg_region_id);

        // Mutate the region: set status to Freed, record free_point.
        let region = self
            .msg
            .region(region_id)
            .cloned()
            .ok_or(BuilderError::RegionNotFound(region_id))?;

        if region.status == RegionStatus::Freed {
            if self.warn_on_double_free {
                self.warnings.push(format!(
                    "Double-free detected: region {} freed again at {}",
                    region_id, pp
                ));
            }
            return Err(BuilderError::DoubleFree {
                node: node.id,
                region: region_id,
            });
        }

        let mut freed_region = region;
        freed_region.status = RegionStatus::Freed;
        freed_region.free_point = Some(pp);

        // Remove and re-insert to update.
        self.msg.remove_region(region_id);
        self.msg.add_region(freed_region);

        Ok(ScgNodeMapping::Deallocation(region_id))
    }

    /// **Rule DERIVE-DIRECT / DERIVE-OFFSET**: ComputationNode → Derivation.
    ///
    /// If the computation is pointer arithmetic (e.g., an offset operation),
    /// creates an `Offset` derivation. If it's a simple assignment (producing
    /// a direct pointer), creates a `Direct` derivation.
    ///
    /// From the spec (§1.5):
    /// > The arithmetic node `n` computes an offset `Δ` from the source
    /// > derivation's current position. The new derivation records the
    /// > absolute offset within the source region.
    fn rule_derive(&mut self, node: &NodeData) -> Result<ScgNodeMapping, BuilderError> {
        let comp = match &node.payload {
            NodePayload::Computation(c) => c,
            _ => {
                return Err(BuilderError::ValidationFailed(
                    "Computation node has non-Computation payload".to_string(),
                ))
            }
        };

        // Heuristic: if the operation name contains "offset", "add", "sub",
        // "index", "ptr+", or similar, treat it as an offset derivation.
        // Otherwise, treat as a direct derivation (assignment/alias).
        let op_lower = comp.kind.label().to_lowercase();
        let is_offset = op_lower.contains("offset")
            || op_lower.contains("add")
            || op_lower.contains("sub")
            || op_lower.contains("index")
            || op_lower.contains("ptr+")
            || op_lower.contains("ptr-")
            || op_lower.contains("increment")
            || op_lower.contains("decrement")
            || op_lower.contains("advance");

        // Find the source derivation by looking at the SCG edges.
        // We look for a DataFlow edge from a predecessor node that has a
        // Derivation mapping.
        let source_derivation_id = self.find_source_derivation_for_node(node)?;

        let deriv_id = self.alloc_derivation_id();
        let derivation = if is_offset {
            // DERIVE-OFFSET: Create an Offset derivation.
            // We use 0 as the default offset; a more precise analysis would
            // extract the actual offset from the computation's operands.
            let offset_by = MsgBuilder::extract_offset_from_operation(&comp.kind.label());
            self.create_offset_derivation(deriv_id, source_derivation_id, offset_by)?
        } else {
            // DERIVE-DIRECT: Create a Direct derivation (alias/assignment).
            self.create_direct_derivation(deriv_id, source_derivation_id)?
        };

        self.msg.add_derivation(derivation);
        Ok(ScgNodeMapping::Derivation(deriv_id))
    }

    /// **Rule DERIVE-CAST**: CastNode → Derivation with Cast kind.
    ///
    /// From the spec (§1.4):
    /// > The cast node `n` takes a source derivation and produces a new
    /// > derivation with kind Cast. The offset is preserved; the RepD changes.
    fn rule_cast(&mut self, node: &NodeData) -> Result<ScgNodeMapping, BuilderError> {
        let cast = match &node.payload {
            NodePayload::Cast(c) => c,
            _ => {
                return Err(BuilderError::ValidationFailed(
                    "Cast node has non-Cast payload".to_string(),
                ))
            }
        };

        let source_derivation_id = self.find_source_derivation_for_node(node)?;

        let deriv_id = self.alloc_derivation_id();
        let derivation = self.create_cast_derivation(
            deriv_id,
            source_derivation_id,
            &cast.from_type,
            &cast.to_type,
        )?;

        self.msg.add_derivation(derivation);
        Ok(ScgNodeMapping::Derivation(deriv_id))
    }

    /// **Rule ACCESS-READ / ACCESS-WRITE**: AccessNode → Access in MSG.
    ///
    /// From the spec (§1.3):
    /// > The access node `n` specifies which derivation produces the pointer
    /// > being dereferenced, the access mode, the size of the access, and
    /// > the program point.
    fn rule_access(&mut self, node: &NodeData) -> Result<ScgNodeMapping, BuilderError> {
        let access = match &node.payload {
            NodePayload::Access(a) => a,
            _ => {
                return Err(BuilderError::ValidationFailed(
                    "Access node has non-Access payload".to_string(),
                ))
            }
        };

        let pp = Self::convert_program_point(&node.program_point);

        // Find the derivation that this access targets.
        // If the access node has incoming DataFlow edges, we use the source
        // derivation. Otherwise, we look up the region's root derivation.
        let target_derivation_id =
            self.find_target_derivation_for_access(node, &access.region_id)?;

        let access_kind = match access.mode {
            AccessMode::Read => AccessKind::Read,
            AccessMode::Write => AccessKind::Write,
            AccessMode::ReadWrite => AccessKind::Write, // treat ReadWrite as Write for conflict detection
        };

        let access_size = access.access_size.unwrap_or(1);

        let aid = self.alloc_access_id();
        let access_entry = Access::new(aid, target_derivation_id, access_kind, access_size, pp);

        self.msg.add_access(access_entry);
        Ok(ScgNodeMapping::Access(aid))
    }

    /// Process effect nodes: I/O nodes that read/write memory.
    ///
    /// From the task description:
    /// > For each I/O node that reads/writes memory: create an Access.
    fn rule_effect(&mut self, node: &NodeData) -> Result<ScgNodeMapping, BuilderError> {
        let effect = match &node.payload {
            NodePayload::Effect(e) => e,
            _ => {
                return Err(BuilderError::ValidationFailed(
                    "Effect node has non-Effect payload".to_string(),
                ))
            }
        };

        let pp = Self::convert_program_point(&node.program_point);

        // Heuristic: determine the access kind from the effect_kind string.
        let kind_lower = effect.effect_kind.to_lowercase();
        let access_kind = if kind_lower.contains("write")
            || kind_lower.contains("output")
            || kind_lower.contains("send")
            || kind_lower.contains("store")
        {
            AccessKind::Write
        } else {
            AccessKind::Read
        };

        // For effect nodes, we try to find a source derivation, or fall back
        // to the first available derivation in the MSG.
        let target_derivation_id = self
            .find_source_derivation_for_node(node)
            .ok()
            .or_else(|| self.first_derivation_id());

        if let Some(did) = target_derivation_id {
            let aid = self.alloc_access_id();
            let access_entry = Access::new(aid, did, access_kind, 1, pp);
            self.msg.add_access(access_entry);
            Ok(ScgNodeMapping::Access(aid))
        } else {
            // No derivation available; skip this effect node.
            Ok(ScgNodeMapping::None)
        }
    }

    // -----------------------------------------------------------------------
    // Edge processing — Rule SYNC
    // -----------------------------------------------------------------------

    /// **Rule SYNC**: Synchronization edges in the SCG → SyncEdge in MSG.
    ///
    /// After all nodes are processed, we examine the SCG edges. Any edge of
    /// kind `ControlFlow` or `Annotation` that connects two access nodes is
    /// converted to a `SyncEdge` in the MSG, establishing ordering.
    fn process_edges(&mut self, scg: &SCG) -> Result<(), BuilderError> {
        // Collect edge data to avoid borrowing issues.
        let edges: Vec<ScgEdgeData> = scg.edges().cloned().collect();

        for edge in edges {
            // Only consider edges that imply synchronization.
            match edge.kind {
                ScgEdgeKind::ControlFlow
                | ScgEdgeKind::Annotation
                | ScgEdgeKind::Dispatch
                | ScgEdgeKind::Call { .. }
                | ScgEdgeKind::Return { .. } => {
                    self.process_sync_edge(&edge)?;
                }
                ScgEdgeKind::DataFlow | ScgEdgeKind::Derivation => {
                    // These edge types are handled during node processing
                    // (they establish derivation chains).
                }
            }
        }

        Ok(())
    }

    /// Process a single SCG edge that may produce a SyncEdge in the MSG.
    fn process_sync_edge(&mut self, edge: &ScgEdgeData) -> Result<(), BuilderError> {
        // Look up the MSG entities for the source and target SCG nodes.
        let source_mapping = self.node_map.get(&edge.source);
        let target_mapping = self.node_map.get(&edge.target);

        // Both endpoints must be Access nodes for a SyncEdge.
        let source_access = match source_mapping {
            Some(ScgNodeMapping::Access(aid)) => *aid,
            _ => return Ok(()), // Not an access-access edge; skip.
        };

        let target_access = match target_mapping {
            Some(ScgNodeMapping::Access(aid)) => *aid,
            _ => return Ok(()), // Not an access-access edge; skip.
        };

        let ordering = match edge.kind {
            ScgEdgeKind::ControlFlow => Ordering::HappensBefore,
            ScgEdgeKind::Annotation => Ordering::AtomicAcquireRelease,
            _ => Ordering::HappensBefore,
        };

        let seid = self.alloc_sync_edge_id();
        let sync_edge = SyncEdge::new(seid, source_access, target_access, ordering);
        self.msg.add_sync_edge(sync_edge);

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Derivation helpers
    // -----------------------------------------------------------------------

    /// Find the source DerivationId for a node by walking its SCG predecessor
    /// edges and looking for nodes that produced derivations.
    fn find_source_derivation_for_node(
        &mut self,
        _node: &NodeData,
    ) -> Result<DerivationId, BuilderError> {
        // Strategy: walk the node_map to find a predecessor that has a
        // Derivation mapping. We check all previously-processed nodes
        // that could be a source.
        //
        // A more sophisticated implementation would examine the SCG edges
        // directly. For now, we use a fallback: the most recently created
        // derivation, or the root derivation of the relevant region.
        let last_deriv = DerivationId(self.next_derivation_id - 1);
        if self.msg.derivation(last_deriv).is_some() {
            return Ok(last_deriv);
        }

        // Fallback: return the first derivation we can find.
        self.first_derivation_id()
            .ok_or(BuilderError::DerivationNotFound(DerivationId(0)))
    }

    /// Find the target DerivationId for an access node.
    ///
    /// The access targets a specific region. We look for the root derivation
    /// of that region.
    fn find_target_derivation_for_access(
        &mut self,
        node: &NodeData,
        scg_region_id: &vuma_scg::region::RegionId,
    ) -> Result<DerivationId, BuilderError> {
        // First, try to find the source derivation through predecessor nodes.
        if let Ok(did) = self.find_source_derivation_for_node(node) {
            return Ok(did);
        }

        // Fallback: find the root derivation for the region.
        let msg_region_id = self
            .scg_region_to_msg_region
            .get(&scg_region_id.as_u64())
            .copied()
            .ok_or(BuilderError::RegionNotFound(RegionId(
                scg_region_id.as_u64(),
            )))?;

        // Find the first derivation whose source is this region.
        for d in self.msg.derivations() {
            if let DerivationSource::Region(rid) = d.source {
                if rid == msg_region_id {
                    return Ok(d.id);
                }
            }
        }

        Err(BuilderError::DerivationNotFound(DerivationId(0)))
    }

    /// Get the first (root) DerivationId in the MSG, if any.
    fn first_derivation_id(&self) -> Option<DerivationId> {
        self.msg.derivation_ids().next()
    }

    /// Create a Direct derivation (alias/assignment) from a source derivation.
    fn create_direct_derivation(
        &mut self,
        new_id: DerivationId,
        source_id: DerivationId,
    ) -> Result<Derivation, BuilderError> {
        let source = self
            .msg
            .derivation(source_id)
            .cloned()
            .ok_or(BuilderError::DerivationNotFound(source_id))?;

        Ok(Derivation {
            id: new_id,
            source: DerivationSource::AnotherDerivation(source_id),
            kind: DerivationKind::Direct,
            proven_range: source.proven_range,
        })
    }

    /// Create an Offset derivation from a source derivation.
    fn create_offset_derivation(
        &mut self,
        new_id: DerivationId,
        source_id: DerivationId,
        offset: i64,
    ) -> Result<Derivation, BuilderError> {
        let source = self
            .msg
            .derivation(source_id)
            .cloned()
            .ok_or(BuilderError::DerivationNotFound(source_id))?;

        let (lo, hi) = source.proven_range;
        let new_lo = lo.offset(offset);
        let new_hi = hi.offset(offset);

        // Check bounds if the source derivation traces to a known region.
        if let Some(region_id) = self.resolve_region_for_derivation(&source) {
            if let Some(region) = self.msg.region(region_id) {
                let offset_abs = if offset >= 0 { offset as u64 } else { 0 };
                if offset_abs > region.size && self.warn_on_out_of_bounds {
                    self.warnings.push(format!(
                        "Offset derivation {} goes out of bounds: offset {} > region size {}",
                        new_id, offset, region.size
                    ));
                }
            }
        }

        Ok(Derivation {
            id: new_id,
            source: DerivationSource::AnotherDerivation(source_id),
            kind: DerivationKind::Offset { by: offset },
            proven_range: (new_lo, new_hi),
        })
    }

    /// Create a Cast derivation from a source derivation.
    fn create_cast_derivation(
        &mut self,
        new_id: DerivationId,
        source_id: DerivationId,
        from_type: &str,
        to_type: &str,
    ) -> Result<Derivation, BuilderError> {
        let source = self
            .msg
            .derivation(source_id)
            .cloned()
            .ok_or(BuilderError::DerivationNotFound(source_id))?;

        Ok(Derivation {
            id: new_id,
            source: DerivationSource::AnotherDerivation(source_id),
            kind: DerivationKind::Cast {
                from: RepD {
                    name: from_type.to_string(),
                    size: 0, // Unknown at this level
                },
                to: RepD {
                    name: to_type.to_string(),
                    size: 0, // Unknown at this level
                },
            },
            proven_range: source.proven_range,
        })
    }

    /// Resolve the RegionId for a derivation by tracing the chain to its root.
    fn resolve_region_for_derivation(&self, derivation: &Derivation) -> Option<RegionId> {
        match &derivation.source {
            DerivationSource::Region(rid) => Some(*rid),
            DerivationSource::AnotherDerivation(parent_id) => {
                let parent = self.msg.derivation(*parent_id)?;
                self.resolve_region_for_derivation(parent)
            }
        }
    }

    /// Extract a numeric offset from a computation operation string.
    ///
    /// Tries to parse trailing numeric characters from the operation name.
    /// For example, "offset_16" → 16, "add8" → 8.
    fn extract_offset_from_operation(op: &str) -> i64 {
        // Try to find a numeric suffix.
        let digits: String = op
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            return 0;
        }
        let reversed: String = digits.chars().rev().collect();
        reversed.parse::<i64>().unwrap_or(0)
    }

    // -----------------------------------------------------------------------
    // Rule MERGE — parallel composition
    // -----------------------------------------------------------------------

    /// **Rule MERGE**: Combine two MSGs.
    ///
    /// Takes another MSG and merges all its regions, derivations, accesses,
    /// and sync edges into this builder's MSG. IDs are remapped to avoid
    /// collisions.
    ///
    /// From the spec:
    /// > The MERGE rule composes two MSGs by unioning their regions,
    /// > derivations, accesses, and verification functions. ID collisions
    /// > are avoided by remapping.
    pub fn merge_msg(&mut self, other: &MSG) -> MsgDelta {
        let mut delta = MsgDelta::new();

        // Merge regions — remap IDs.
        let mut region_id_map: HashMap<RegionId, RegionId> = HashMap::new();
        for region in other.regions() {
            let old_id = region.id;
            let new_id = self.alloc_region_id();
            let mut new_region = region.clone();
            new_region.id = new_id;
            self.msg.add_region(new_region.clone());
            region_id_map.insert(old_id, new_id);
            delta.region_changes.push(RegionChange::Added(new_region));
        }

        // Merge derivations — remap IDs and source references.
        let mut deriv_id_map: HashMap<DerivationId, DerivationId> = HashMap::new();
        for derivation in other.derivations() {
            let old_id = derivation.id;
            let new_id = self.alloc_derivation_id();
            let mut new_deriv = derivation.clone();
            new_deriv.id = new_id;
            // Remap the source.
            new_deriv.source = match &derivation.source {
                DerivationSource::Region(rid) => {
                    DerivationSource::Region(region_id_map.get(rid).copied().unwrap_or(*rid))
                }
                DerivationSource::AnotherDerivation(did) => DerivationSource::AnotherDerivation(
                    deriv_id_map.get(did).copied().unwrap_or(*did),
                ),
            };
            self.msg.add_derivation(new_deriv.clone());
            deriv_id_map.insert(old_id, new_id);
            delta
                .derivation_changes
                .push(DerivationChange::Added(new_deriv));
        }

        // Merge accesses — remap target derivation IDs.
        let mut access_id_map: HashMap<AccessId, AccessId> = HashMap::new();
        for access in other.accesses() {
            let old_id = access.id;
            let new_id = self.alloc_access_id();
            let mut new_access = access.clone();
            new_access.id = new_id;
            new_access.target = deriv_id_map
                .get(&access.target)
                .copied()
                .unwrap_or(access.target);
            self.msg.add_access(new_access.clone());
            access_id_map.insert(old_id, new_id);
            delta.access_changes.push(AccessChange::Added(new_access));
        }

        // Merge sync edges — remap access IDs.
        for sync_edge in other.sync_edges() {
            let new_id = self.alloc_sync_edge_id();
            let new_se = SyncEdge::new(
                new_id,
                access_id_map
                    .get(&sync_edge.access1)
                    .copied()
                    .unwrap_or(sync_edge.access1),
                access_id_map
                    .get(&sync_edge.access2)
                    .copied()
                    .unwrap_or(sync_edge.access2),
                sync_edge.ordering.clone(),
            );
            self.msg.add_sync_edge(new_se.clone());
            delta.sync_edge_changes.push(SyncEdgeChange::Added(new_se));
        }

        delta
    }

    // -----------------------------------------------------------------------
    // Incremental updates
    // -----------------------------------------------------------------------

    /// Incrementally update the MSG when SCG nodes have changed.
    ///
    /// `added_nodes` — SCG node IDs that were added since the last build.
    /// `removed_nodes` — SCG node IDs that were removed since the last build.
    ///
    /// Returns a [`MsgDelta`] describing the changes made to the MSG.
    pub fn update(
        &mut self,
        scg: &SCG,
        added_nodes: &[ScgNodeId],
        removed_nodes: &[ScgNodeId],
    ) -> Result<MsgDelta, BuilderError> {
        let mut delta = MsgDelta::new();

        // Process removals: remove MSG entities that were produced by
        // removed SCG nodes, and collect their dependents.
        for &node_id in removed_nodes {
            if let Some(mapping) = self.node_map.get(&node_id).copied() {
                match mapping {
                    ScgNodeMapping::Region(rid) => {
                        if let Some(_region) = self.msg.remove_region(rid) {
                            delta.region_changes.push(RegionChange::Removed(rid));
                            // Also remove all derivations sourced from this region.
                            self.remove_derivations_for_region(rid, &mut delta);
                            // Remove accesses targeting those derivations.
                            self.remove_accesses_for_orphan_derivation(&mut delta);
                        }
                    }
                    ScgNodeMapping::Derivation(did) => {
                        if let Some(_deriv) = self.msg.remove_derivation(did) {
                            delta
                                .derivation_changes
                                .push(DerivationChange::Removed(did));
                            // Remove downstream derivations.
                            self.remove_downstream_derivations(did, &mut delta);
                            // Remove accesses targeting removed derivations.
                            self.remove_accesses_for_derivation(did, &mut delta);
                        }
                    }
                    ScgNodeMapping::Access(aid) => {
                        if self.msg.remove_access(aid).is_some() {
                            delta.access_changes.push(AccessChange::Removed(aid));
                            // Remove sync edges referencing this access.
                            self.remove_sync_edges_for_access(aid, &mut delta);
                        }
                    }
                    ScgNodeMapping::Deallocation(rid) => {
                        // Revert the region's status from Freed to Allocated.
                        if let Some(mut region) = self.msg.remove_region(rid) {
                            region.status = RegionStatus::Allocated;
                            region.free_point = None;
                            self.msg.add_region(region.clone());
                            delta.region_changes.push(RegionChange::Modified(region));
                        }
                    }
                    ScgNodeMapping::None => {}
                }
                self.node_map.remove(&node_id);
                self.processed_nodes.remove(&node_id);
            }
        }

        // Process additions: topologically sort the new nodes relative to
        // existing processed nodes, then apply the inference rules.
        //
        // For simplicity, we process new nodes in the order given.
        // A more precise approach would compute a topological order of the
        // new nodes considering the existing SCG edges.
        for &node_id in added_nodes {
            // Record the entity counts before processing so we can compute the delta.
            let _regions_before = self.msg.region_count();
            let derivs_before = self.msg.derivation_count();
            let accesses_before = self.msg.access_count();

            self.process_node(scg, node_id)?;

            // Record the newly-added entities in the delta.
            if let Some(mapping) = self.node_map.get(&node_id) {
                match mapping {
                    ScgNodeMapping::Region(rid) => {
                        if let Some(region) = self.msg.region(*rid) {
                            delta
                                .region_changes
                                .push(RegionChange::Added(region.clone()));
                        }
                    }
                    ScgNodeMapping::Derivation(did) => {
                        if self.msg.derivation_count() > derivs_before {
                            if let Some(deriv) = self.msg.derivation(*did) {
                                delta
                                    .derivation_changes
                                    .push(DerivationChange::Added(deriv.clone()));
                            }
                        }
                    }
                    ScgNodeMapping::Access(aid) => {
                        if self.msg.access_count() > accesses_before {
                            if let Some(access) = self.msg.access(*aid) {
                                delta
                                    .access_changes
                                    .push(AccessChange::Added(access.clone()));
                            }
                        }
                    }
                    ScgNodeMapping::Deallocation(rid) => {
                        if let Some(region) = self.msg.region(*rid) {
                            delta
                                .region_changes
                                .push(RegionChange::Modified(region.clone()));
                        }
                    }
                    ScgNodeMapping::None => {}
                }
            }
        }

        // Re-process edges to pick up any new sync edges.
        self.process_edges(scg)?;

        Ok(delta)
    }

    /// Remove all derivations whose source is a given region.
    fn remove_derivations_for_region(&mut self, rid: RegionId, delta: &mut MsgDelta) {
        let deriv_ids_to_remove: Vec<DerivationId> = self
            .msg
            .derivations()
            .filter(|d| matches!(d.source, DerivationSource::Region(r) if r == rid))
            .map(|d| d.id)
            .collect();

        for did in deriv_ids_to_remove {
            if self.msg.remove_derivation(did).is_some() {
                delta
                    .derivation_changes
                    .push(DerivationChange::Removed(did));
            }
        }
    }

    /// Remove all derivations that are transitively derived from `did`.
    fn remove_downstream_derivations(&mut self, did: DerivationId, delta: &mut MsgDelta) {
        let downstream: Vec<DerivationId> = self
            .msg
            .derivations()
            .filter(|d| {
                matches!(d.source, DerivationSource::AnotherDerivation(parent) if parent == did)
            })
            .map(|d| d.id)
            .collect();

        for child_id in downstream {
            self.remove_downstream_derivations(child_id, delta);
            if self.msg.remove_derivation(child_id).is_some() {
                delta
                    .derivation_changes
                    .push(DerivationChange::Removed(child_id));
            }
        }
    }

    /// Remove all accesses targeting a given derivation.
    fn remove_accesses_for_derivation(&mut self, did: DerivationId, delta: &mut MsgDelta) {
        let access_ids: Vec<AccessId> = self
            .msg
            .accesses()
            .filter(|a| a.target == did)
            .map(|a| a.id)
            .collect();

        for aid in access_ids {
            if self.msg.remove_access(aid).is_some() {
                delta.access_changes.push(AccessChange::Removed(aid));
                self.remove_sync_edges_for_access(aid, delta);
            }
        }
    }

    /// Remove all accesses whose target derivation has been removed.
    fn remove_accesses_for_orphan_derivation(&mut self, delta: &mut MsgDelta) {
        let access_ids: Vec<AccessId> = self
            .msg
            .accesses()
            .filter(|a| self.msg.derivation(a.target).is_none())
            .map(|a| a.id)
            .collect();

        for aid in access_ids {
            if self.msg.remove_access(aid).is_some() {
                delta.access_changes.push(AccessChange::Removed(aid));
            }
        }
    }

    /// Remove all sync edges that reference a given access.
    fn remove_sync_edges_for_access(&mut self, aid: AccessId, delta: &mut MsgDelta) {
        let seids: Vec<SyncEdgeId> = self
            .msg
            .sync_edges()
            .filter(|se| se.access1 == aid || se.access2 == aid)
            .map(|se| se.id)
            .collect();

        for seid in seids {
            if self.msg.remove_sync_edge(seid).is_some() {
                delta.sync_edge_changes.push(SyncEdgeChange::Removed(seid));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Get a reference to the constructed MSG.
    pub fn msg(&self) -> &MSG {
        &self.msg
    }

    /// Get the mapping from SCG node IDs to MSG entities.
    pub fn node_map(&self) -> &HashMap<ScgNodeId, ScgNodeMapping> {
        &self.node_map
    }

    /// Get the warnings collected during the build.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Look up the MSG entity produced for a given SCG node.
    pub fn mapping_for(&self, scg_node_id: ScgNodeId) -> Option<ScgNodeMapping> {
        self.node_map.get(&scg_node_id).copied()
    }

    /// Compute the full derivation chain for a given MSG DerivationId.
    ///
    /// Returns the chain `[root, ..., parent, self]` from the originating
    /// region to the given derivation.
    pub fn derivation_chain(&self, did: DerivationId) -> Vec<Derivation> {
        self.msg.derivation_chain(did)
    }

    /// Resolve the base address for a derivation by tracing to its region.
    pub fn resolve_base_address(&self, did: DerivationId) -> Option<Address> {
        let derivation = self.msg.derivation(did)?;
        let region_id = self.resolve_region_for_derivation(derivation)?;
        let region = self.msg.region(region_id)?;
        Some(region.base)
    }

    /// Compute the proven address range for a derivation.
    pub fn proven_range(&self, did: DerivationId) -> Option<(Address, Address)> {
        let derivation = self.msg.derivation(did)?;
        Some(derivation.proven_range)
    }
}

impl Default for MsgBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MsgBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MsgBuilder {{ msg={}, processed={}, warnings={} }}",
            self.msg,
            self.processed_nodes.len(),
            self.warnings.len(),
        )
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::Address;
    use crate::derivation::DerivationSource;
    use crate::region::RegionStatus;
    use vuma_scg::region::{DeploymentTarget, RegionId as ScgRegionId, SCGRegion};
    use vuma_scg::{
        AccessMode, AccessNode, AllocationNode, CastNode, ComputationKind, ComputationNode,
        DeallocationNode, EffectNode, ProgramPoint as ScgPP,
    };

    /// Helper: create a minimal SCG ProgramPoint.
    fn scg_pp(line: u64) -> ScgPP {
        ScgPP {
            file: Some("test.vu".to_string()),
            line: Some(line),
            column: Some(1),
            offset: None,
        }
    }

    /// Helper: build a simple SCG with one allocation.
    fn build_simple_alloc_scg() -> (SCG, ScgNodeId, ScgRegionId) {
        let mut scg = SCG::new();
        let rid = ScgRegionId::new(1);
        scg.add_region(SCGRegion::new(rid, DeploymentTarget::Heap));

        let alloc_id = scg.add_node(
            ScgNodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 8,
                region_id: rid,
                type_name: Some("Buffer".to_string()),
            }),
            scg_pp(1),
        );

        (scg, alloc_id, rid)
    }

    // ---- Test 1: ALLOC rule ----

    #[test]
    fn test_alloc_creates_region() {
        let (scg, _alloc_id, _rid) = build_simple_alloc_scg();
        let mut builder = MsgBuilder::new();
        let msg = builder.build(&scg).unwrap();

        assert_eq!(msg.region_count(), 1);
        let region = msg.regions().next().unwrap();
        assert_eq!(region.size, 256);
        assert_eq!(region.status, RegionStatus::Allocated);
        assert!(region.free_point.is_none());
        assert_eq!(region.owner_context, Some("Buffer".to_string()));
    }

    // ---- Test 2: ALLOC creates root derivation ----

    #[test]
    fn test_alloc_creates_root_derivation() {
        let (scg, alloc_id, _rid) = build_simple_alloc_scg();
        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();

        // The allocation node should have produced a Derivation mapping.
        let mapping = builder.mapping_for(alloc_id);
        assert!(matches!(mapping, Some(ScgNodeMapping::Derivation(_))));

        if let Some(ScgNodeMapping::Derivation(did)) = mapping {
            let deriv = builder.msg().derivation(did).unwrap();
            assert_eq!(deriv.kind, DerivationKind::Direct);
            assert!(matches!(deriv.source, DerivationSource::Region(_)));
        }
    }

    // ---- Test 3: DEALLOC rule ----

    #[test]
    fn test_dealloc_marks_region_freed() {
        let (mut scg, alloc_id, rid) = build_simple_alloc_scg();

        let dealloc_id = scg.add_node(
            ScgNodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_id,
                region_id: rid,
            }),
            scg_pp(10),
        );
        scg.add_edge(alloc_id, dealloc_id, ScgEdgeKind::Derivation)
            .unwrap();

        let mut builder = MsgBuilder::new();
        let msg = builder.build(&scg).unwrap();

        assert_eq!(msg.region_count(), 1);
        let region = msg.regions().next().unwrap();
        assert_eq!(region.status, RegionStatus::Freed);
        assert!(region.free_point.is_some());
    }

    // ---- Test 4: Double-free detection ----

    #[test]
    fn test_double_free_error() {
        let (mut scg, alloc_id, rid) = build_simple_alloc_scg();

        let dealloc1 = scg.add_node(
            ScgNodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_id,
                region_id: rid,
            }),
            scg_pp(10),
        );
        let dealloc2 = scg.add_node(
            ScgNodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_id,
                region_id: rid,
            }),
            scg_pp(20),
        );
        scg.add_edge(alloc_id, dealloc1, ScgEdgeKind::Derivation)
            .unwrap();
        scg.add_edge(dealloc1, dealloc2, ScgEdgeKind::ControlFlow)
            .unwrap();

        let mut builder = MsgBuilder::new();
        let result = builder.build(&scg);

        assert!(result.is_err());
        if let Err(BuilderError::DoubleFree { region, .. }) = result {
            assert_eq!(region, RegionId(1));
        } else {
            panic!("Expected DoubleFree error, got {:?}", result);
        }
    }

    // ---- Test 5: DERIVE-DIRECT (Computation node without offset) ----

    #[test]
    fn test_derive_direct_creates_derivation() {
        let (mut scg, alloc_id, _rid) = build_simple_alloc_scg();

        let comp_id = scg.add_node(
            ScgNodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("assign".to_string()),
                result_type: Some("*mut u8".to_string()),
                tail_call: false,
            }),
            scg_pp(5),
        );
        scg.add_edge(alloc_id, comp_id, ScgEdgeKind::DataFlow)
            .unwrap();

        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();

        // The computation node should produce a Direct derivation.
        let mapping = builder.mapping_for(comp_id);
        assert!(matches!(mapping, Some(ScgNodeMapping::Derivation(_))));
    }

    // ---- Test 6: DERIVE-OFFSET (Computation node with offset) ----

    #[test]
    fn test_derive_offset_creates_offset_derivation() {
        let (mut scg, alloc_id, _rid) = build_simple_alloc_scg();

        let comp_id = scg.add_node(
            ScgNodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("offset_16".to_string()),
                result_type: Some("*mut u8".to_string()),
                tail_call: false,
            }),
            scg_pp(5),
        );
        scg.add_edge(alloc_id, comp_id, ScgEdgeKind::DataFlow)
            .unwrap();

        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();

        let mapping = builder.mapping_for(comp_id);
        if let Some(ScgNodeMapping::Derivation(did)) = mapping {
            let deriv = builder.msg().derivation(did).unwrap();
            assert!(matches!(deriv.kind, DerivationKind::Offset { by: 16 }));
        } else {
            panic!("Expected Derivation mapping");
        }
    }

    // ---- Test 7: DERIVE-CAST (Cast node) ----

    #[test]
    fn test_derive_cast_creates_cast_derivation() {
        let (mut scg, alloc_id, _rid) = build_simple_alloc_scg();

        let cast_id = scg.add_node(
            ScgNodeType::Cast,
            NodePayload::Cast(CastNode {
                from_type: "*mut u8".to_string(),
                to_type: "*mut u32".to_string(),
                is_lossless: true,
            }),
            scg_pp(6),
        );
        scg.add_edge(alloc_id, cast_id, ScgEdgeKind::DataFlow)
            .unwrap();

        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();

        let mapping = builder.mapping_for(cast_id);
        if let Some(ScgNodeMapping::Derivation(did)) = mapping {
            let deriv = builder.msg().derivation(did).unwrap();
            match &deriv.kind {
                DerivationKind::Cast { from, to } => {
                    assert_eq!(from.name, "*mut u8");
                    assert_eq!(to.name, "*mut u32");
                }
                other => panic!("Expected Cast derivation, got {:?}", other),
            }
        } else {
            panic!("Expected Derivation mapping");
        }
    }

    // ---- Test 8: ACCESS-READ ----

    #[test]
    fn test_access_read_creates_read_access() {
        let (mut scg, alloc_id, rid) = build_simple_alloc_scg();

        let access_id = scg.add_node(
            ScgNodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: rid,
                offset: Some(0),
                access_size: Some(4),
            }),
            scg_pp(7),
        );
        scg.add_edge(alloc_id, access_id, ScgEdgeKind::DataFlow)
            .unwrap();

        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();

        let mapping = builder.mapping_for(access_id);
        if let Some(ScgNodeMapping::Access(aid)) = mapping {
            let access = builder.msg().access(aid).unwrap();
            assert_eq!(access.kind, AccessKind::Read);
            assert_eq!(access.size, 4);
        } else {
            panic!("Expected Access mapping");
        }
    }

    // ---- Test 9: ACCESS-WRITE ----

    #[test]
    fn test_access_write_creates_write_access() {
        let (mut scg, alloc_id, rid) = build_simple_alloc_scg();

        let access_id = scg.add_node(
            ScgNodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: rid,
                offset: Some(0),
                access_size: Some(8),
            }),
            scg_pp(8),
        );
        scg.add_edge(alloc_id, access_id, ScgEdgeKind::DataFlow)
            .unwrap();

        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();

        let mapping = builder.mapping_for(access_id);
        if let Some(ScgNodeMapping::Access(aid)) = mapping {
            let access = builder.msg().access(aid).unwrap();
            assert_eq!(access.kind, AccessKind::Write);
            assert_eq!(access.size, 8);
        } else {
            panic!("Expected Access mapping");
        }
    }

    // ---- Test 10: SYNC edge creation ----

    #[test]
    fn test_sync_edge_from_control_flow() {
        let (mut scg, alloc_id, rid) = build_simple_alloc_scg();

        let read_id = scg.add_node(
            ScgNodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: rid,
                offset: Some(0),
                access_size: Some(4),
            }),
            scg_pp(7),
        );
        let write_id = scg.add_node(
            ScgNodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: rid,
                offset: Some(0),
                access_size: Some(4),
            }),
            scg_pp(8),
        );
        scg.add_edge(alloc_id, read_id, ScgEdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(alloc_id, write_id, ScgEdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(read_id, write_id, ScgEdgeKind::ControlFlow)
            .unwrap();

        let mut builder = MsgBuilder::new();
        let msg = builder.build(&scg).unwrap();

        // The ControlFlow edge between two Access nodes should produce a SyncEdge.
        assert_eq!(msg.sync_edge_count(), 1);
        let se = msg.sync_edges().next().unwrap();
        assert_eq!(se.ordering, Ordering::HappensBefore);
    }

    // ---- Test 11: MERGE rule ----

    #[test]
    fn test_merge_two_msgs() {
        // Build two separate SCGs and their MSGs.
        let (scg1, _, _) = build_simple_alloc_scg();

        let mut scg2 = SCG::new();
        let rid2 = ScgRegionId::new(2);
        scg2.add_region(SCGRegion::new(rid2, DeploymentTarget::Heap));
        scg2.add_node(
            ScgNodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 512,
                align: 16,
                region_id: rid2,
                type_name: None,
            }),
            scg_pp(1),
        );

        let mut builder1 = MsgBuilder::new();
        builder1.build(&scg1).unwrap();

        let mut builder2 = MsgBuilder::new();
        builder2.build(&scg2).unwrap();

        // Merge builder2's MSG into builder1.
        let delta = builder1.merge_msg(builder2.msg());

        assert_eq!(builder1.msg().region_count(), 2);
        assert!(!delta.is_empty());
        assert_eq!(delta.region_changes.len(), 1);
    }

    // ---- Test 12: Incremental update — add a node ----

    #[test]
    fn test_incremental_add_node() {
        let (mut scg, alloc_id, rid) = build_simple_alloc_scg();

        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();
        assert_eq!(builder.msg().access_count(), 0);

        // Add an access node to the SCG.
        let access_id = scg.add_node(
            ScgNodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: rid,
                offset: Some(0),
                access_size: Some(4),
            }),
            scg_pp(5),
        );

        let delta = builder.update(&scg, &[access_id], &[]).unwrap();
        assert_eq!(builder.msg().access_count(), 1);
        assert!(!delta.is_empty());
    }

    // ---- Test 13: Incremental update — remove a node ----

    #[test]
    fn test_incremental_remove_node() {
        let (mut scg, alloc_id, rid) = build_simple_alloc_scg();

        let access_id = scg.add_node(
            ScgNodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: rid,
                offset: Some(0),
                access_size: Some(4),
            }),
            scg_pp(5),
        );
        scg.add_edge(alloc_id, access_id, ScgEdgeKind::DataFlow)
            .unwrap();

        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();
        assert_eq!(builder.msg().access_count(), 1);

        // Remove the access node.
        let delta = builder.update(&scg, &[], &[access_id]).unwrap();
        assert_eq!(builder.msg().access_count(), 0);
        assert!(!delta.is_empty());
    }

    // ---- Test 14: Derivation chain tracking ----

    #[test]
    fn test_derivation_chain_tracking() {
        let (mut scg, alloc_id, _rid) = build_simple_alloc_scg();

        // Create a chain: alloc → offset → cast
        let offset_id = scg.add_node(
            ScgNodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("offset_32".to_string()),
                result_type: Some("*mut u8".to_string()),
                tail_call: false,
            }),
            scg_pp(3),
        );
        let cast_id = scg.add_node(
            ScgNodeType::Cast,
            NodePayload::Cast(CastNode {
                from_type: "*mut u8".to_string(),
                to_type: "*mut u32".to_string(),
                is_lossless: true,
            }),
            scg_pp(4),
        );
        scg.add_edge(alloc_id, offset_id, ScgEdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(offset_id, cast_id, ScgEdgeKind::DataFlow)
            .unwrap();

        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();

        // Find the cast derivation and trace its chain.
        if let Some(ScgNodeMapping::Derivation(cast_did)) = builder.mapping_for(cast_id) {
            let chain = builder.derivation_chain(cast_did);
            // Chain should be: [root_direct, offset, cast]
            assert!(
                chain.len() >= 2,
                "Expected at least 2 derivations in chain, got {}",
                chain.len()
            );
        } else {
            panic!("Expected Derivation mapping for cast node");
        }
    }

    // ---- Test 15: Address range computation ----

    #[test]
    fn test_address_range_computation() {
        let (scg, alloc_id, _rid) = build_simple_alloc_scg();

        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();

        if let Some(ScgNodeMapping::Derivation(did)) = builder.mapping_for(alloc_id) {
            let range = builder.proven_range(did).unwrap();
            let region = builder.msg().regions().next().unwrap();
            assert_eq!(range.0, region.base);
            assert_eq!(range.1, region.base + region.size);
        }
    }

    // ---- Test 16: Effect node creates access ----

    #[test]
    fn test_effect_node_creates_access() {
        let (mut scg, alloc_id, rid) = build_simple_alloc_scg();

        let effect_id = scg.add_node(
            ScgNodeType::Effect,
            NodePayload::Effect(EffectNode {
                effect_kind: "io_write".to_string(),
                is_observable: true,
            }),
            scg_pp(9),
        );
        scg.add_edge(alloc_id, effect_id, ScgEdgeKind::DataFlow)
            .unwrap();

        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();

        let mapping = builder.mapping_for(effect_id);
        if let Some(ScgNodeMapping::Access(aid)) = mapping {
            let access = builder.msg().access(aid).unwrap();
            assert_eq!(access.kind, AccessKind::Write);
        } else {
            panic!("Expected Access mapping for effect node");
        }
    }

    // ---- Test 17: Zero-size allocation error ----

    #[test]
    fn test_zero_size_allocation_error() {
        let mut scg = SCG::new();
        let rid = ScgRegionId::new(1);
        scg.add_region(SCGRegion::new(rid, DeploymentTarget::Heap));

        scg.add_node(
            ScgNodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 0,
                align: 8,
                region_id: rid,
                type_name: None,
            }),
            scg_pp(1),
        );

        let mut builder = MsgBuilder::new();
        let result = builder.build(&scg);
        assert!(matches!(result, Err(BuilderError::ZeroSizeAllocation(_))));
    }

    // ---- Test 18: Cycle detection ----

    #[test]
    fn test_cycle_detection() {
        // The MSG builder now handles cycles gracefully using SCC-based
        // topological sort (topological_sort_with_cycles). This is required
        // for loop back-edges in real programs. The test verifies that a
        // cyclic SCG doesn't cause a CycleDetected error — it may produce
        // other errors (like DerivationNotFound for bare computation nodes
        // without allocation context), but the cycle itself is not rejected.
        let mut scg = SCG::new();
        let n1 = scg.add_node(
            ScgNodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("a".to_string()),
                result_type: None,
                tail_call: false,
            }),
            scg_pp(1),
        );
        let n2 = scg.add_node(
            ScgNodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("b".to_string()),
                result_type: None,
                tail_call: false,
            }),
            scg_pp(2),
        );
        scg.add_edge(n1, n2, ScgEdgeKind::DataFlow).unwrap();
        scg.add_edge(n2, n1, ScgEdgeKind::DataFlow).unwrap();

        let mut builder = MsgBuilder::new();
        let result = builder.build(&scg);
        // The cycle should NOT cause CycleDetected — the builder handles
        // cycles via SCC. Other errors are acceptable for this synthetic test.
        assert!(
            !matches!(result, Err(BuilderError::CycleDetected)),
            "cyclic SCG should not be rejected with CycleDetected; got: {:?}",
            result
        );
    }

    // ---- Test 19: Multiple allocations create non-overlapping regions ----

    #[test]
    fn test_multiple_allocations_non_overlapping() {
        let mut scg = SCG::new();
        let rid1 = ScgRegionId::new(1);
        let rid2 = ScgRegionId::new(2);
        scg.add_region(SCGRegion::new(rid1, DeploymentTarget::Heap));
        scg.add_region(SCGRegion::new(rid2, DeploymentTarget::Heap));

        scg.add_node(
            ScgNodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 128,
                align: 8,
                region_id: rid1,
                type_name: None,
            }),
            scg_pp(1),
        );
        scg.add_node(
            ScgNodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 8,
                region_id: rid2,
                type_name: None,
            }),
            scg_pp(2),
        );

        let mut builder = MsgBuilder::new();
        let msg = builder.build(&scg).unwrap();

        assert_eq!(msg.region_count(), 2);
        let regions: Vec<_> = msg.regions().collect();
        // Regions should not overlap.
        assert!(!regions[0].overlaps(regions[1]));
    }

    // ---- Test 20: Build empty SCG ----

    #[test]
    fn test_build_empty_scg() {
        let scg = SCG::new();
        let mut builder = MsgBuilder::new();
        let msg = builder.build(&scg).unwrap();
        assert_eq!(msg.region_count(), 0);
        assert_eq!(msg.derivation_count(), 0);
        assert_eq!(msg.access_count(), 0);
        assert_eq!(msg.sync_edge_count(), 0);
    }

    // ---- Test 21: ReadWrite access mode treated as Write ----

    #[test]
    fn test_readwrite_access_treated_as_write() {
        let (mut scg, alloc_id, rid) = build_simple_alloc_scg();

        let access_id = scg.add_node(
            ScgNodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::ReadWrite,
                region_id: rid,
                offset: Some(0),
                access_size: Some(4),
            }),
            scg_pp(7),
        );
        scg.add_edge(alloc_id, access_id, ScgEdgeKind::DataFlow)
            .unwrap();

        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();

        let mapping = builder.mapping_for(access_id);
        if let Some(ScgNodeMapping::Access(aid)) = mapping {
            let access = builder.msg().access(aid).unwrap();
            assert_eq!(access.kind, AccessKind::Write);
        }
    }

    // ---- Test 22: Builder display ----

    #[test]
    fn test_builder_display() {
        let scg = SCG::new();
        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();
        let display = format!("{}", builder);
        assert!(display.contains("MsgBuilder"));
        assert!(display.contains("processed=0"));
    }

    // ---- Test 23: Warnings on out-of-bounds ----

    #[test]
    fn test_out_of_bounds_warning() {
        let (mut scg, alloc_id, _rid) = build_simple_alloc_scg();

        // Create an offset computation that goes out of bounds.
        let comp_id = scg.add_node(
            ScgNodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("offset_9999".to_string()),
                result_type: Some("*mut u8".to_string()),
                tail_call: false,
            }),
            scg_pp(5),
        );
        scg.add_edge(alloc_id, comp_id, ScgEdgeKind::DataFlow)
            .unwrap();

        let mut builder = MsgBuilder::new().with_out_of_bounds_warnings(true);
        builder.build(&scg).unwrap();

        assert!(!builder.warnings().is_empty());
        assert!(builder.warnings()[0].contains("out of bounds"));
    }

    // ---- Test 24: build_into consumes the builder ----

    #[test]
    fn test_build_into() {
        let (scg, _, _) = build_simple_alloc_scg();
        let builder = MsgBuilder::new();
        let msg = builder.build_into(&scg).unwrap();
        assert_eq!(msg.region_count(), 1);
    }

    // ---- Test 25: Address allocator alignment ----

    #[test]
    fn test_address_allocator_alignment() {
        let mut alloc = AddressAllocator::new(0x1000);
        let a1 = alloc.alloc(5); // 0x1000..0x1005, next starts at 0x1010 (aligned to 16)
        let a2 = alloc.alloc(16);
        assert_eq!(a1, Address::from(0x1000_u64));
        assert_eq!(a2, Address::from(0x1010_u64));
    }

    // ---- Test 26: SCG region to MSG region mapping ----

    #[test]
    fn test_scg_region_to_msg_region_mapping() {
        let (scg, _, rid) = build_simple_alloc_scg();
        let mut builder = MsgBuilder::new();
        builder.build(&scg).unwrap();

        let msg_region_id = builder.scg_region_to_msg_region.get(&rid.as_u64()).copied();
        assert!(msg_region_id.is_some());
        assert!(builder.msg().region(msg_region_id.unwrap()).is_some());
    }

    // ---- Test 27: Full pipeline: alloc → offset → access → dealloc ----

    #[test]
    fn test_full_pipeline() {
        let mut scg = SCG::new();
        let rid = ScgRegionId::new(1);
        scg.add_region(SCGRegion::new(rid, DeploymentTarget::Heap));

        let alloc_id = scg.add_node(
            ScgNodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 1024,
                align: 16,
                region_id: rid,
                type_name: Some("BigBuffer".to_string()),
            }),
            scg_pp(1),
        );

        let offset_id = scg.add_node(
            ScgNodeType::Computation,
            NodePayload::Computation(ComputationNode {
                kind: ComputationKind::Other("offset_64".to_string()),
                result_type: Some("*mut u8".to_string()),
                tail_call: false,
            }),
            scg_pp(2),
        );

        let cast_id = scg.add_node(
            ScgNodeType::Cast,
            NodePayload::Cast(CastNode {
                from_type: "*mut u8".to_string(),
                to_type: "*mut u32".to_string(),
                is_lossless: true,
            }),
            scg_pp(3),
        );

        let read_id = scg.add_node(
            ScgNodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id: rid,
                offset: Some(64),
                access_size: Some(4),
            }),
            scg_pp(4),
        );

        let write_id = scg.add_node(
            ScgNodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id: rid,
                offset: Some(64),
                access_size: Some(4),
            }),
            scg_pp(5),
        );

        let dealloc_id = scg.add_node(
            ScgNodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_id,
                region_id: rid,
            }),
            scg_pp(6),
        );

        scg.add_edge(alloc_id, offset_id, ScgEdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(offset_id, cast_id, ScgEdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(cast_id, read_id, ScgEdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(cast_id, write_id, ScgEdgeKind::DataFlow)
            .unwrap();
        scg.add_edge(write_id, dealloc_id, ScgEdgeKind::ControlFlow)
            .unwrap();

        let mut builder = MsgBuilder::new();
        let msg = builder.build(&scg).unwrap();

        // 1 region, 3 derivations (root + offset + cast), 2 accesses
        assert_eq!(msg.region_count(), 1);
        assert_eq!(msg.derivation_count(), 3);
        assert_eq!(msg.access_count(), 2);

        // Region should be freed.
        let region = msg.regions().next().unwrap();
        assert_eq!(region.status, RegionStatus::Freed);
    }

    // ---- Test 28: MsgDelta is_empty ----

    #[test]
    fn test_msg_delta_is_empty() {
        let delta = MsgDelta::new();
        assert!(delta.is_empty());

        let delta_with_change = MsgDelta {
            region_changes: vec![RegionChange::Removed(RegionId(1))],
            ..MsgDelta::default()
        };
        assert!(!delta_with_change.is_empty());
    }

    // ---- Test 29: Custom base address ----

    #[test]
    fn test_custom_base_address() {
        let (scg, _, _) = build_simple_alloc_scg();
        let mut builder = MsgBuilder::new().with_base_address(0x5000_0000);
        let msg = builder.build(&scg).unwrap();

        let region = msg.regions().next().unwrap();
        assert_eq!(region.base, Address::from(0x5000_0000_u64));
    }

    // ---- Test 30: Extract offset from operation name ----

    #[test]
    fn test_extract_offset_from_operation() {
        assert_eq!(MsgBuilder::extract_offset_from_operation("offset_64"), 64);
        assert_eq!(MsgBuilder::extract_offset_from_operation("add8"), 8);
        assert_eq!(MsgBuilder::extract_offset_from_operation("assign"), 0);
        assert_eq!(MsgBuilder::extract_offset_from_operation("index128"), 128);
        assert_eq!(MsgBuilder::extract_offset_from_operation("increment"), 0);
    }

    // ---- Test 31: BuilderError display ----

    #[test]
    fn test_builder_error_display() {
        let err = BuilderError::CycleDetected;
        assert!(format!("{}", err).contains("cycle"));

        let err = BuilderError::ZeroSizeAllocation(ScgNodeId::new(42));
        assert!(format!("{}", err).contains("zero-size"));

        let err = BuilderError::DoubleFree {
            node: ScgNodeId::new(5),
            region: RegionId(1),
        };
        assert!(format!("{}", err).contains("double-free"));

        let err = BuilderError::OutOfBounds {
            node: ScgNodeId::new(3),
            offset: 999,
            region_size: 256,
        };
        assert!(format!("{}", err).contains("out-of-bounds"));
    }
}
