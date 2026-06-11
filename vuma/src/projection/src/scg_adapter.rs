//! Adapter layer for converting between projection placeholder types and real vuma-scg types.
//!
//! The projection crate defines lightweight placeholder SCG types so that it compiles
//! independently. This module provides bidirectional conversion functions so that
//! projection code can interoperate with the real `vuma_scg` types.
//!
//! # Lossy conversions
//!
//! Some information is lost during conversion because the two type systems have
//! different shapes and levels of detail:
//!
//! - **Projection → Real**: `SCGNode.bds` (a `Vec<BehaviouralDescriptor>`) is collapsed
//!   to a single `Option<BDReference>` in `NodeData.annotation`. Only the first BD is
//!   preserved. The `SCGNode.regions` field is not stored in `NodeData` — region
//!   membership is tracked in `SCGRegion` instead.
//!
//! - **Real → Projection**: `NodeData.program_point` and the full `NodePayload` are
//!   not representable in the projection types. Only the `NodeType` (mapped to
//!   `NodeKind`) and a label extracted from the payload are preserved. The
//!   `SCGRegion.security_boundary`, `scope_level`, and `deployment_target` fields
//!   have no projection equivalent.

use crate::{BdKind, BehaviouralDescriptor, EdgeKind, NodeKind, SCGEdge, SCGNode, SCGRegion, SCG};

// ── NodeKind ↔ NodeType ──────────────────────────────────────────────────────

/// Convert a projection [`NodeKind`] to a real [`vuma_scg::NodeType`].
///
/// The mapping follows the semantic correspondence between the two type systems.
/// `NodeKind::Function`, `Merge`, and `Module` all map to `NodeType::Control` but
/// with different [`vuma_scg::ControlKind`] values carried in the node payload.
pub fn node_kind_to_node_type(kind: &NodeKind) -> vuma_scg::NodeType {
    match kind {
        NodeKind::Function => vuma_scg::NodeType::Control,
        NodeKind::Value => vuma_scg::NodeType::Computation,
        NodeKind::MessageSend => vuma_scg::NodeType::Effect,
        NodeKind::MessageReceive => vuma_scg::NodeType::Effect,
        NodeKind::Merge => vuma_scg::NodeType::Control,
        NodeKind::Effect => vuma_scg::NodeType::Effect,
        NodeKind::Module => vuma_scg::NodeType::Control,
        NodeKind::Allocation => vuma_scg::NodeType::Allocation,
        NodeKind::Deallocation => vuma_scg::NodeType::Deallocation,
        NodeKind::Access => vuma_scg::NodeType::Access,
        NodeKind::Computation => vuma_scg::NodeType::Computation,
    }
}

/// Determine the [`vuma_scg::ControlKind`] for a projection [`NodeKind`] that maps
/// to [`vuma_scg::NodeType::Control`].
pub fn node_kind_to_control_kind(kind: &NodeKind) -> vuma_scg::ControlKind {
    match kind {
        NodeKind::Function => vuma_scg::ControlKind::FunctionEntry,
        NodeKind::Module => vuma_scg::ControlKind::FunctionEntry,
        NodeKind::Merge => vuma_scg::ControlKind::Join,
        _ => vuma_scg::ControlKind::Jump,
    }
}

/// Convert a real [`vuma_scg::NodeType`] (and optional [`vuma_scg::ControlKind`])
/// back to a projection [`NodeKind`].
///
/// For `NodeType::Control`, the `control_kind` parameter disambiguates between
/// `NodeKind::Function`, `NodeKind::Merge`, and `NodeKind::Module`. If `control_kind`
/// is `None`, `NodeKind::Function` is used as the default.
pub fn node_type_to_node_kind(
    node_type: &vuma_scg::NodeType,
    control_kind: Option<&vuma_scg::ControlKind>,
) -> NodeKind {
    match node_type {
        vuma_scg::NodeType::Computation => NodeKind::Computation,
        vuma_scg::NodeType::Allocation => NodeKind::Allocation,
        vuma_scg::NodeType::Deallocation => NodeKind::Deallocation,
        vuma_scg::NodeType::Access => NodeKind::Access,
        vuma_scg::NodeType::Cast => NodeKind::Computation,
        vuma_scg::NodeType::Effect => NodeKind::Effect,
        vuma_scg::NodeType::Control => match control_kind {
            Some(vuma_scg::ControlKind::Join) => NodeKind::Merge,
            Some(
                vuma_scg::ControlKind::FunctionEntry
                | vuma_scg::ControlKind::FunctionReturn
                | vuma_scg::ControlKind::ClosureEntry
                | vuma_scg::ControlKind::ClosureReturn,
            ) => NodeKind::Function,
            Some(vuma_scg::ControlKind::FuturePoll)
            | Some(vuma_scg::ControlKind::WakerRegistration)
            | Some(vuma_scg::ControlKind::StateTransition) => NodeKind::Effect,
            _ => NodeKind::Function,
        },
        vuma_scg::NodeType::Phantom => NodeKind::Effect,
        vuma_scg::NodeType::VTable => NodeKind::Effect,
        vuma_scg::NodeType::ClosureEnv => NodeKind::Value,
    }
}

// ── EdgeKind ↔ vuma_scg::EdgeKind ────────────────────────────────────────────

/// Convert a projection [`EdgeKind`] to a real [`vuma_scg::EdgeKind`].
///
/// # Mapping
///
/// | Projection        | Real SCG                                    |
/// |-------------------|---------------------------------------------|
/// | `DataFlow`        | `DataFlow`                                  |
/// | `ControlFlow`     | `ControlFlow`                               |
/// | `Message`         | `Dispatch` (closest semantic match)         |
/// | `Borrow`          | `DataFlow` (with label `"borrow"`)          |
/// | `Call`            | `Call { from_node, to_node, caller_region }`|
/// | `Derivation`      | `Derivation`                                |
/// | `Annotation`      | `Annotation`                                |
pub fn edge_kind_to_scg_edge_kind(
    kind: &EdgeKind,
    source: crate::NodeId,
    target: crate::NodeId,
) -> vuma_scg::EdgeKind {
    match kind {
        EdgeKind::DataFlow => vuma_scg::EdgeKind::DataFlow,
        EdgeKind::ControlFlow => vuma_scg::EdgeKind::ControlFlow,
        EdgeKind::Message => vuma_scg::EdgeKind::Dispatch,
        EdgeKind::Borrow => vuma_scg::EdgeKind::DataFlow,
        EdgeKind::Call => vuma_scg::EdgeKind::Call {
            from_node: vuma_scg::NodeId::new(source),
            to_node: vuma_scg::NodeId::new(target),
            caller_region: vuma_scg::RegionId::new(0),
        },
        EdgeKind::Derivation => vuma_scg::EdgeKind::Derivation,
        EdgeKind::Annotation => vuma_scg::EdgeKind::Annotation,
    }
}

/// Convert a real [`vuma_scg::EdgeKind`] to a projection [`EdgeKind`].
///
/// # Mapping
///
/// | Real SCG                   | Projection        | Notes                          |
/// |----------------------------|-------------------|--------------------------------|
/// | `DataFlow`                 | `DataFlow`        | Unless label is `"borrow"` → `Borrow` |
/// | `ControlFlow`              | `ControlFlow`     |                                |
/// | `Dispatch`                 | `Message`         |                                |
/// | `Call { .. }`              | `Call`            | Call metadata is lost          |
/// | `Return { .. }`            | `ControlFlow`     | Return edges are control flow  |
/// | `Derivation`               | `Derivation`      |                                |
/// | `Annotation`               | `Annotation`      |                                |
pub fn scg_edge_kind_to_edge_kind(kind: &vuma_scg::EdgeKind, label: Option<&str>) -> EdgeKind {
    match kind {
        vuma_scg::EdgeKind::DataFlow => {
            // If the edge was created from a Borrow, it carries the label "borrow"
            if label == Some("borrow") {
                EdgeKind::Borrow
            } else {
                EdgeKind::DataFlow
            }
        }
        vuma_scg::EdgeKind::ControlFlow => EdgeKind::ControlFlow,
        vuma_scg::EdgeKind::Dispatch => EdgeKind::Message,
        vuma_scg::EdgeKind::Call { .. } => EdgeKind::Call,
        vuma_scg::EdgeKind::Return { .. } => EdgeKind::ControlFlow,
        vuma_scg::EdgeKind::Derivation => EdgeKind::Derivation,
        vuma_scg::EdgeKind::Annotation => EdgeKind::Annotation,
    }
}

// ── Label extraction helpers ─────────────────────────────────────────────────

/// Extract a human-readable label from a real [`vuma_scg::NodePayload`].
///
/// The projection `SCGNode` has a single `label: String` field, while the real
/// SCG stores type-specific data in the `NodePayload` enum. This function picks
/// the most natural string from each payload variant.
pub fn extract_label(payload: &vuma_scg::NodePayload) -> String {
    match payload {
        vuma_scg::NodePayload::Computation(p) => p.operation.clone(),
        vuma_scg::NodePayload::Allocation(p) => p
            .type_name
            .clone()
            .unwrap_or_else(|| format!("alloc_{}", p.size)),
        vuma_scg::NodePayload::Deallocation(p) => {
            format!("dealloc_{}", p.allocation_node.as_u64())
        }
        vuma_scg::NodePayload::Access(p) => format!(
            "access_{:?}{}_r{}",
            p.mode,
            p.offset.map(|o| format!("_off{}", o)).unwrap_or_default(),
            p.region_id.as_u64()
        ),
        vuma_scg::NodePayload::Cast(p) => format!("{}_to_{}", p.from_type, p.to_type),
        vuma_scg::NodePayload::Effect(p) => p.effect_kind.clone(),
        vuma_scg::NodePayload::Control(p) => {
            p.label.clone().unwrap_or_else(|| format!("{:?}", p.kind))
        }
        vuma_scg::NodePayload::Phantom(p) => p.purpose.clone(),
        vuma_scg::NodePayload::VTable(p) => {
            format!("vtable_{}_for_{}", p.trait_name, p.concrete_type)
        }
        vuma_scg::NodePayload::ClosureEnv(p) => {
            format!("closure_env_{}captures", p.captured_vars.len())
        }
    }
}

/// Extract the [`vuma_scg::ControlKind`] from a [`vuma_scg::NodePayload`] if it
/// is a `Control` variant.
pub fn extract_control_kind(payload: &vuma_scg::NodePayload) -> Option<vuma_scg::ControlKind> {
    match payload {
        vuma_scg::NodePayload::Control(p) => Some(p.kind),
        _ => None,
    }
}

// ── NodeData ↔ SCGNode ───────────────────────────────────────────────────────

/// Convert a real [`vuma_scg::NodeData`] to a projection [`SCGNode`].
///
/// # Note
///
/// The `regions` field is set to an empty vector because region membership is
/// tracked in [`vuma_scg::SCGRegion`], not in the node itself. Use [`from_scg`]
/// to populate region memberships from the full graph.
pub fn from_scg_node(data: &vuma_scg::NodeData) -> SCGNode {
    let control_kind = extract_control_kind(&data.payload);
    SCGNode {
        id: data.id.as_u64(),
        label: extract_label(&data.payload),
        kind: node_type_to_node_kind(&data.node_type, control_kind.as_ref()),
        bds: data
            .annotation
            .as_ref()
            .map(|ann| {
                vec![BehaviouralDescriptor {
                    id: ann.bd_id,
                    name: format!("bd_{}", ann.bd_id),
                    kind: BdKind::Custom,
                    parameter: ann.version.map(|v| format!("v{}", v)),
                }]
            })
            .unwrap_or_default(),
        regions: Vec::new(), // populated by from_scg
    }
}

/// Convert a projection [`SCGNode`] to a real [`vuma_scg::NodeData`].
///
/// # Lossy conversions
///
/// - Only the first behavioural descriptor is preserved (as `annotation`).
/// - `program_point` is set to a default (no source info in projection).
/// - For `Allocation`, `Deallocation`, and `Access` nodes, the payload uses
///   placeholder values for fields not present in the projection type.
///   The first region ID from `node.regions` is used as `region_id` if available.
pub fn to_scg_node(node: &SCGNode) -> vuma_scg::NodeData {
    let node_type = node_kind_to_node_type(&node.kind);
    let default_region = node
        .regions
        .first()
        .map(|&r| vuma_scg::RegionId::new(r))
        .unwrap_or(vuma_scg::RegionId::new(0));

    let payload = match &node.kind {
        NodeKind::Computation | NodeKind::Value => {
            vuma_scg::NodePayload::Computation(vuma_scg::ComputationNode {
                operation: node.label.clone(),
                result_type: None,
                tail_call: false,
            })
        }
        NodeKind::Allocation => vuma_scg::NodePayload::Allocation(vuma_scg::AllocationNode {
            size: 0,
            align: 1,
            region_id: default_region,
            type_name: Some(node.label.clone()),
        }),
        NodeKind::Deallocation => {
            vuma_scg::NodePayload::Deallocation(vuma_scg::DeallocationNode {
                allocation_node: vuma_scg::NodeId::new(0), // placeholder
                region_id: default_region,
            })
        }
        NodeKind::Access => vuma_scg::NodePayload::Access(vuma_scg::AccessNode {
            mode: vuma_scg::AccessMode::ReadWrite,
            region_id: default_region,
            offset: None,
            access_size: None,
        }),
        NodeKind::MessageSend | NodeKind::MessageReceive => {
            vuma_scg::NodePayload::Effect(vuma_scg::EffectNode {
                effect_kind: node.label.clone(),
                is_observable: matches!(node.kind, NodeKind::MessageSend),
            })
        }
        NodeKind::Effect => vuma_scg::NodePayload::Effect(vuma_scg::EffectNode {
            effect_kind: node.label.clone(),
            is_observable: true,
        }),
        NodeKind::Function | NodeKind::Merge | NodeKind::Module => {
            vuma_scg::NodePayload::Control(vuma_scg::ControlNode {
                kind: node_kind_to_control_kind(&node.kind),
                label: Some(node.label.clone()),
            })
        }
    };

    let annotation = node.bds.first().map(|bd| vuma_scg::BDReference {
        bd_id: bd.id,
        version: bd.parameter.as_ref().and_then(|p| {
            // Try to parse "vN" back to a version number
            p.strip_prefix('v').and_then(|n| n.parse::<u64>().ok())
        }),
    });

    vuma_scg::NodeData {
        id: vuma_scg::NodeId::new(node.id),
        node_type,
        annotation,
        program_point: vuma_scg::ProgramPoint {
            file: None,
            line: None,
            column: None,
            offset: None,
        },
        payload,
    }
}

// ── EdgeData ↔ SCGEdge ───────────────────────────────────────────────────────

/// Convert a real [`vuma_scg::EdgeData`] to a projection [`SCGEdge`].
pub fn from_scg_edge(data: &vuma_scg::EdgeData) -> SCGEdge {
    SCGEdge {
        id: data.id.as_u64(),
        source: data.source.as_u64(),
        target: data.target.as_u64(),
        kind: scg_edge_kind_to_edge_kind(&data.kind, data.label.as_deref()),
    }
}

/// Convert a projection [`SCGEdge`] to a real [`vuma_scg::EdgeData`].
///
/// # Note
///
/// - `EdgeKind::Borrow` is converted to `vuma_scg::EdgeKind::DataFlow` with the
///   label set to `"borrow"` so it can be recovered in the reverse direction.
/// - `EdgeKind::Call` is converted to `vuma_scg::EdgeKind::Call` with a default
///   `caller_region` of `RegionId::new(0)`.
pub fn to_scg_edge(edge: &SCGEdge) -> vuma_scg::EdgeData {
    let kind = edge_kind_to_scg_edge_kind(&edge.kind, edge.source, edge.target);

    let label = match &edge.kind {
        EdgeKind::Borrow => Some("borrow".to_string()),
        _ => None,
    };

    vuma_scg::EdgeData {
        id: vuma_scg::EdgeId::new(edge.id),
        source: vuma_scg::NodeId::new(edge.source),
        target: vuma_scg::NodeId::new(edge.target),
        kind,
        label,
    }
}

// ── SCGRegion ↔ SCGRegion ────────────────────────────────────────────────────

/// Convert a real [`vuma_scg::SCGRegion`] to a projection [`SCGRegion`].
///
/// # Note
///
/// The real `SCGRegion` has no `name` field, so a synthetic name is generated.
/// The `security_boundary`, `scope_level`, and `deployment_target` fields are
/// not representable in the projection type.
pub fn from_scg_region(region: &vuma_scg::SCGRegion) -> SCGRegion {
    SCGRegion {
        id: region.id.as_u64(),
        name: format!("region_{}", region.id.as_u64()),
        nodes: region.iter_nodes().map(|n| n.as_u64()).collect(),
    }
}

/// Convert a projection [`SCGRegion`] to a real [`vuma_scg::SCGRegion`].
///
/// # Note
///
/// - The `name` field is not representable in the real type and is dropped.
/// - `deployment_target` defaults to [`vuma_scg::DeploymentTarget::Heap`].
/// - `scope_level` defaults to `0`.
/// - `security_boundary` defaults to `false`.
pub fn to_scg_region(region: &SCGRegion) -> vuma_scg::SCGRegion {
    let mut scg_region = vuma_scg::SCGRegion::new(
        vuma_scg::RegionId::new(region.id),
        vuma_scg::DeploymentTarget::Heap,
    );
    for &node_id in &region.nodes {
        scg_region.add_node(vuma_scg::NodeId::new(node_id));
    }
    scg_region
}

// ── SCG ↔ SCG ────────────────────────────────────────────────────────────────

/// Convert an entire real [`vuma_scg::SCG`] to a projection [`SCG`].
///
/// This function iterates over all nodes, edges, and regions in the real SCG
/// and converts each one. Node region memberships are populated by scanning
/// all regions for their contained nodes.
pub fn from_scg(scg: &vuma_scg::SCG) -> SCG {
    // Convert nodes, collecting into a map so we can add region info
    let mut proj_nodes: Vec<SCGNode> = scg.nodes().map(from_scg_node).collect();

    // Build a mapping from NodeId to the region IDs it belongs to
    let mut node_regions: std::collections::HashMap<u64, Vec<u64>> =
        std::collections::HashMap::new();
    for region in scg.regions() {
        let rid = region.id.as_u64();
        for node_id in region.iter_nodes() {
            node_regions.entry(node_id.as_u64()).or_default().push(rid);
        }
    }

    // Populate region memberships on nodes
    for node in &mut proj_nodes {
        if let Some(regions) = node_regions.get(&node.id) {
            node.regions = regions.clone();
        }
    }

    // Convert edges
    let proj_edges: Vec<SCGEdge> = scg.edges().map(from_scg_edge).collect();

    // Convert regions
    let proj_regions: Vec<SCGRegion> = scg.regions().map(from_scg_region).collect();

    SCG {
        nodes: proj_nodes,
        edges: proj_edges,
        regions: proj_regions,
    }
}

/// Convert an entire projection [`SCG`] to a real [`vuma_scg::SCG`].
///
/// Nodes are added first (so edges can reference them), then edges, then regions.
/// IDs are preserved where possible using `add_node_with_id` and `add_edge_with_id`.
pub fn to_scg(scg: &SCG) -> vuma_scg::SCG {
    let mut real = vuma_scg::SCG::new();

    // Add all nodes first (edges require both endpoints to exist)
    for node in &scg.nodes {
        let data = to_scg_node(node);
        let _ = real.add_node_with_id(data.id, data.node_type, data.payload, data.program_point);
        // Copy annotation if present
        if let Some(ref ann) = data.annotation {
            if let Some(real_node) = real.get_node_mut(data.id) {
                real_node.annotation = Some(ann.clone());
            }
        }
    }

    // Add all edges
    for edge in &scg.edges {
        let data = to_scg_edge(edge);
        let _ = real.add_edge_with_id(data.id, data.source, data.target, data.kind);
        // Copy label if present
        if let Some(ref label) = data.label {
            if let Some(real_edge) = real.get_edge_mut(data.id) {
                real_edge.label = Some(label.clone());
            }
        }
    }

    // Add all regions
    for region in &scg.regions {
        let real_region = to_scg_region(region);
        real.add_region(real_region);
    }

    real
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that a projection SCGNode round-trips through the real SCG types
    /// and the essential fields (id, kind, label) are preserved.
    #[test]
    fn test_node_roundtrip() {
        // Use Computation — the cleanest roundtrip (label → operation → label)
        let original = SCGNode {
            id: 42,
            label: "add_i32".to_string(),
            kind: NodeKind::Computation,
            bds: vec![BehaviouralDescriptor {
                id: 1,
                name: "Send".to_string(),
                kind: BdKind::Capability,
                parameter: None,
            }],
            regions: vec![10],
        };

        // projection → real
        let real_node = to_scg_node(&original);
        assert_eq!(real_node.id.as_u64(), 42);
        assert_eq!(real_node.node_type, vuma_scg::NodeType::Computation);

        // real → projection
        let roundtripped = from_scg_node(&real_node);

        // Essential fields should match
        assert_eq!(roundtripped.id, original.id);
        assert_eq!(roundtripped.kind, original.kind);
        assert_eq!(roundtripped.label, original.label);

        // First BD should be preserved (via annotation)
        assert_eq!(roundtripped.bds.len(), 1);
        assert_eq!(roundtripped.bds[0].id, 1);

        // regions field is empty (populated only by from_scg)
        assert!(roundtripped.regions.is_empty());

        // Also test Function (Control) roundtrip
        let func_node = SCGNode {
            id: 7,
            label: "auth_handler".to_string(),
            kind: NodeKind::Function,
            bds: vec![],
            regions: vec![],
        };
        let real_func = to_scg_node(&func_node);
        let rt_func = from_scg_node(&real_func);
        assert_eq!(rt_func.id, 7);
        assert_eq!(rt_func.kind, NodeKind::Function);
        assert_eq!(rt_func.label, "auth_handler");

        // Also test Merge (ControlKind::Join) roundtrip
        let merge_node = SCGNode {
            id: 15,
            label: "merge_point".to_string(),
            kind: NodeKind::Merge,
            bds: vec![],
            regions: vec![],
        };
        let real_merge = to_scg_node(&merge_node);
        let rt_merge = from_scg_node(&real_merge);
        assert_eq!(rt_merge.id, 15);
        assert_eq!(rt_merge.kind, NodeKind::Merge);
        assert_eq!(rt_merge.label, "merge_point");
    }

    /// Test that a projection SCGEdge round-trips through the real SCG types.
    #[test]
    fn test_edge_roundtrip() {
        // Test DataFlow edge
        let original = SCGEdge {
            id: 100,
            source: 1,
            target: 2,
            kind: EdgeKind::DataFlow,
        };
        let real_edge = to_scg_edge(&original);
        let rt_edge = from_scg_edge(&real_edge);
        assert_eq!(rt_edge.id, original.id);
        assert_eq!(rt_edge.source, original.source);
        assert_eq!(rt_edge.target, original.target);
        assert_eq!(rt_edge.kind, original.kind);

        // Test ControlFlow edge
        let cf_edge = SCGEdge {
            id: 101,
            source: 2,
            target: 3,
            kind: EdgeKind::ControlFlow,
        };
        let real_cf = to_scg_edge(&cf_edge);
        let rt_cf = from_scg_edge(&real_cf);
        assert_eq!(rt_cf.kind, EdgeKind::ControlFlow);

        // Test Borrow edge (DataFlow + "borrow" label roundtrip)
        let borrow_edge = SCGEdge {
            id: 102,
            source: 3,
            target: 4,
            kind: EdgeKind::Borrow,
        };
        let real_borrow = to_scg_edge(&borrow_edge);
        // Should be stored as DataFlow with "borrow" label
        assert_eq!(real_borrow.kind, vuma_scg::EdgeKind::DataFlow);
        assert_eq!(real_borrow.label.as_deref(), Some("borrow"));
        let rt_borrow = from_scg_edge(&real_borrow);
        assert_eq!(rt_borrow.kind, EdgeKind::Borrow);

        // Test Message edge (→ Dispatch roundtrip)
        let msg_edge = SCGEdge {
            id: 103,
            source: 4,
            target: 5,
            kind: EdgeKind::Message,
        };
        let real_msg = to_scg_edge(&msg_edge);
        assert_eq!(real_msg.kind, vuma_scg::EdgeKind::Dispatch);
        let rt_msg = from_scg_edge(&real_msg);
        assert_eq!(rt_msg.kind, EdgeKind::Message);

        // Test Call edge (→ Call { .. } roundtrip)
        let call_edge = SCGEdge {
            id: 104,
            source: 5,
            target: 6,
            kind: EdgeKind::Call,
        };
        let real_call = to_scg_edge(&call_edge);
        assert!(matches!(real_call.kind, vuma_scg::EdgeKind::Call { .. }));
        let rt_call = from_scg_edge(&real_call);
        assert_eq!(rt_call.kind, EdgeKind::Call);

        // Test Derivation edge
        let deriv_edge = SCGEdge {
            id: 105,
            source: 6,
            target: 7,
            kind: EdgeKind::Derivation,
        };
        let real_deriv = to_scg_edge(&deriv_edge);
        let rt_deriv = from_scg_edge(&real_deriv);
        assert_eq!(rt_deriv.kind, EdgeKind::Derivation);

        // Test Annotation edge
        let ann_edge = SCGEdge {
            id: 106,
            source: 7,
            target: 8,
            kind: EdgeKind::Annotation,
        };
        let real_ann = to_scg_edge(&ann_edge);
        let rt_ann = from_scg_edge(&real_ann);
        assert_eq!(rt_ann.kind, EdgeKind::Annotation);
    }

    /// Test that a small projection SCG with 3 nodes and 2 edges round-trips
    /// through the real SCG type and the graph structure is preserved.
    #[test]
    fn test_scg_roundtrip() {
        // Build a projection SCG with 3 nodes and 2 edges
        let original = SCG {
            nodes: vec![
                SCGNode {
                    id: 0,
                    label: "alloc_buffer".to_string(),
                    kind: NodeKind::Allocation,
                    bds: vec![],
                    regions: vec![1],
                },
                SCGNode {
                    id: 1,
                    label: "compute".to_string(),
                    kind: NodeKind::Computation,
                    bds: vec![BehaviouralDescriptor {
                        id: 99,
                        name: "Send".to_string(),
                        kind: BdKind::Capability,
                        parameter: None,
                    }],
                    regions: vec![1],
                },
                SCGNode {
                    id: 2,
                    label: "dealloc_buffer".to_string(),
                    kind: NodeKind::Deallocation,
                    bds: vec![],
                    regions: vec![1],
                },
            ],
            edges: vec![
                SCGEdge {
                    id: 0,
                    source: 0,
                    target: 1,
                    kind: EdgeKind::DataFlow,
                },
                SCGEdge {
                    id: 1,
                    source: 1,
                    target: 2,
                    kind: EdgeKind::ControlFlow,
                },
            ],
            regions: vec![SCGRegion {
                id: 1,
                name: "heap_region".to_string(),
                nodes: vec![0, 1, 2],
            }],
        };

        // projection → real
        let real_scg = to_scg(&original);

        // Verify the real SCG has the right structure
        assert_eq!(real_scg.node_count(), 3);
        assert_eq!(real_scg.edge_count(), 2);
        assert_eq!(real_scg.region_count(), 1);

        // Verify nodes
        let n0 = real_scg.get_node(vuma_scg::NodeId::new(0)).unwrap();
        assert_eq!(n0.node_type, vuma_scg::NodeType::Allocation);

        let n1 = real_scg.get_node(vuma_scg::NodeId::new(1)).unwrap();
        assert_eq!(n1.node_type, vuma_scg::NodeType::Computation);
        assert!(n1.annotation.is_some());

        let n2 = real_scg.get_node(vuma_scg::NodeId::new(2)).unwrap();
        assert_eq!(n2.node_type, vuma_scg::NodeType::Deallocation);

        // Verify edges
        let e0 = real_scg.get_edge(vuma_scg::EdgeId::new(0)).unwrap();
        assert_eq!(e0.kind, vuma_scg::EdgeKind::DataFlow);

        let e1 = real_scg.get_edge(vuma_scg::EdgeId::new(1)).unwrap();
        assert_eq!(e1.kind, vuma_scg::EdgeKind::ControlFlow);

        // Verify region
        let region = real_scg.get_region(vuma_scg::RegionId::new(1)).unwrap();
        assert_eq!(region.node_count(), 3);

        // real → projection
        let roundtripped = from_scg(&real_scg);

        // Verify node count and edge count
        assert_eq!(roundtripped.nodes.len(), 3);
        assert_eq!(roundtripped.edges.len(), 2);
        assert_eq!(roundtripped.regions.len(), 1);

        // Verify nodes roundtripped correctly
        assert_eq!(roundtripped.nodes[0].id, 0);
        assert_eq!(roundtripped.nodes[0].kind, NodeKind::Allocation);
        assert_eq!(roundtripped.nodes[1].id, 1);
        assert_eq!(roundtripped.nodes[1].kind, NodeKind::Computation);
        assert_eq!(roundtripped.nodes[1].bds.len(), 1);
        assert_eq!(roundtripped.nodes[1].bds[0].id, 99);
        assert_eq!(roundtripped.nodes[2].id, 2);
        assert_eq!(roundtripped.nodes[2].kind, NodeKind::Deallocation);

        // Verify edges roundtripped correctly
        assert_eq!(roundtripped.edges[0].kind, EdgeKind::DataFlow);
        assert_eq!(roundtripped.edges[0].source, 0);
        assert_eq!(roundtripped.edges[0].target, 1);
        assert_eq!(roundtripped.edges[1].kind, EdgeKind::ControlFlow);
        assert_eq!(roundtripped.edges[1].source, 1);
        assert_eq!(roundtripped.edges[1].target, 2);

        // Verify region membership was populated (from_scg fills it in)
        assert_eq!(roundtripped.nodes[0].regions.len(), 1);
        assert_eq!(roundtripped.nodes[0].regions[0], 1);
        assert_eq!(roundtripped.nodes[1].regions.len(), 1);
        assert_eq!(roundtripped.nodes[2].regions.len(), 1);

        // Verify region
        assert_eq!(roundtripped.regions[0].id, 1);
        assert_eq!(roundtripped.regions[0].nodes.len(), 3);
    }
}
