//! # VUMA SCG — Semantic Computation Graph
//!
//! This crate provides the core data structures and algorithms for the
//! **Semantic Computation Graph (SCG)**, a central component of the VUMA
//! framework for verified-unsafe memory access.
//!
//! ## Overview
//!
//! The SCG models program semantics as a directed graph where:
//! - **Nodes** represent operations (computation, allocation, deallocation,
//!   memory access, type casts, side effects, control flow, and phantom markers).
//! - **Edges** represent relationships between operations (data flow, control
//!   flow, derivation, and annotation).
//! - **Regions** group nodes into memory scopes with security boundaries.
//!
//! ## Module Structure
//!
//! - [`node`] — Node types (`NodeId`, `NodeType`, `NodeData`, and per-variant payloads).
//! - [`edge`] — Edge types (`EdgeId`, `EdgeKind`, `EdgeData`).
//! - [`graph`] — The `SCG` graph structure with construction, traversal, and
//!   validation operations.
//! - [`region`] — Memory region types (`RegionId`, `SCGRegion`, `DeploymentTarget`).
//! - [`query`] — Query engine for structured graph inspection.
//!
//! ## Quick Start
//!
//! ```
//! use vuma_scg::{
//!     SCG, NodeId, NodeData, NodeType, NodePayload,
//!     ComputationNode, EdgeKind, ProgramPoint,
//!     SCGQuery, execute,
//! };
//!
//! let mut scg = SCG::new();
//!
//! let n1 = scg.add_node(
//!     NodeType::Computation,
//!     NodePayload::Computation(ComputationNode {
//!         operation: "add".to_string(),
//!         result_type: Some("i32".to_string()),
//!     }),
//!     ProgramPoint { file: None, line: None, column: None, offset: None },
//! );
//!
//! let result = execute(&scg, SCGQuery::NodesByType(NodeType::Computation));
//! assert_eq!(result.node_ids.len(), 1);
//! ```

pub mod dominance;
pub mod diff;
pub mod edge;
pub mod graph;
pub mod liveness;
pub mod node;
pub mod query;
pub mod region;
pub mod serialize;
pub mod transform;

// Re-export the primary public API at the crate root for convenience.

// -- Node types --
pub use node::{
    AccessMode, AccessNode, AllocationNode, BDReference, CastNode, ComputationNode, ControlKind,
    ControlNode, DeallocationNode, EffectNode, NodeData, NodeId, NodePayload, NodeType,
    PhantomNode, ProgramPoint,
};

// -- Edge types --
pub use edge::{EdgeData, EdgeId, EdgeKind};

// -- Graph --
pub use graph::{SCG, SCGError, ValidationResult};

// -- Region types --
pub use region::{DeploymentTarget, RegionId, SCGRegion};

// -- Query engine --
pub use query::{execute, DerivationChain, QueryResult, SCGQuery, find_access_nodes_to_region, find_derivation_chains};

// -- Diff engine --
pub use diff::{
    apply_diff, compute_edit_script, diff_scg, three_way_merge,
    DiffEntry, DiffError, DiffStats, MergeConflict,
    EdgeConflict, NodeConflict, RegionConflict, SCGDiff,
};

// -- Dominance analysis --
pub use dominance::{
    DominatorTree, compute_dominators, compute_post_dominators, dominates,
    strictly_dominates, find_dominance_frontier, nearest_common_dominator,
    dom_tree_postorder, dominated_by, dominators_of,
    always_executes_after, write_precedes_read, guaranteed_execution_path,
};

// -- Liveness analysis --
pub use liveness::{
    LivenessAnalysis, LivenessInfo, UseAfterFree, compute_liveness, find_dead_allocations,
    find_dead_code, find_uninitialized_reads, find_use_after_free,
};

// -- Transform passes --
pub use transform::{
    CommonSubexpressionElimination, ConstantFolding, DeadCodeElimination, InliningPass,
    PassManager, PassResult, PipelineResult, SCGPass, VerificationPass,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Integration test: build a small SCG, validate it, and query it.
    #[test]
    fn integration_test_build_validate_query() {
        let region_id = RegionId::new(1);

        // Build the SCG
        let mut scg = SCG::new();

        // Add a region
        let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);

        // Add an allocation node
        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 16,
                region_id,
                type_name: Some("MyBuffer".to_string()),
            }),
            ProgramPoint {
                file: Some("main.vu".to_string()),
                line: Some(10),
                column: Some(5),
                offset: None,
            },
        );
        region.add_node(alloc_id);

        // Add a computation node
        let comp_id = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: "write_buffer".to_string(),
                result_type: None,
            }),
            ProgramPoint {
                file: Some("main.vu".to_string()),
                line: Some(11),
                column: Some(3),
                offset: None,
            },
        );

        // Add a deallocation node
        let dealloc_id = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_id,
                region_id,
            }),
            ProgramPoint {
                file: Some("main.vu".to_string()),
                line: Some(20),
                column: Some(1),
                offset: None,
            },
        );
        region.add_node(dealloc_id);

        scg.add_region(region);

        // Add edges
        scg.add_edge(alloc_id, comp_id, EdgeKind::DataFlow).unwrap();
        scg.add_edge(alloc_id, dealloc_id, EdgeKind::Derivation).unwrap();
        scg.add_edge(comp_id, dealloc_id, EdgeKind::ControlFlow).unwrap();

        // Validate
        let validation = scg.validate();
        assert!(validation.is_valid, "Validation errors: {:?}", validation.errors);

        // Query: nodes by type
        let comp_nodes = execute(&scg, SCGQuery::NodesByType(NodeType::Computation));
        assert_eq!(comp_nodes.node_ids.len(), 1);
        assert_eq!(comp_nodes.node_ids[0], comp_id);

        // Query: derivation chains from allocation
        let chains = find_derivation_chains(&scg, alloc_id, 5);
        assert_eq!(chains.len(), 1);
        assert_eq!(chains[0].root(), Some(&alloc_id));
        assert_eq!(chains[0].leaf(), Some(&dealloc_id));

        // Query: no leaked allocations
        let leaked = execute(&scg, SCGQuery::LeakedAllocations);
        assert!(leaked.node_ids.is_empty());

        // Topological sort
        let topo = scg.topological_sort().unwrap();
        assert_eq!(topo.len(), 3);
        // alloc must come before dealloc
        let alloc_pos = topo.iter().position(|&id| id == alloc_id).unwrap();
        let dealloc_pos = topo.iter().position(|&id| id == dealloc_id).unwrap();
        assert!(alloc_pos < dealloc_pos);
    }
}
