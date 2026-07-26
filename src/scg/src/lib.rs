//! # VUMA SCG — Semantic Computation Graph
//!
//! **This crate is the canonical SCG IR for VUMA.** It is consumed by
//! both the Invariant Verification Engine (IVE) — for liveness, dominance,
//! region, and derivation analyses — and (planned, see Task 4 of the SCG
//! unification track) the codegen backend. Other crates that currently
//! hold local SCG-shaped stubs (notably `vuma_codegen::scg_to_ir`, which
//! uses a statement-list-shaped SCG) are slated to be replaced by
//! re-exports of the types defined here. See the "SCG Unification Status
//! (Task 4-f)" section in `src/codegen/src/scg_to_ir.rs` for the current
//! migration state — as of Wave 4-e, the *structure* of `CastNode`,
//! `ControlNode`, and `SwitchArm` has been migrated here as generic types
//! (`CodegenCastNode`, `CodegenControlNode`, `CodegenSwitchArm`); as of
//! Wave 4-b, the *structure* of `AllocationNode` and `AccessNode` (both
//! enums, unlike their graph-shaped struct counterparts) has been
//! migrated here as `CodegenAllocationNode` and `CodegenAccessNode`; as
//! of Wave 12-a, the *structure* of the binary `ComputationNode` has
//! been migrated here as `CodegenComputationNode<E, K>`; as of Wave 13-c,
//! the *structure* of the unary `UnaryComputationNode` and the function
//! call `CallNode` (both codegen-only types with no semantic-SCG
//! counterpart) have been migrated here as `CodegenUnaryComputationNode<E, K>`
//! and `CodegenCallNode<E>`. The concrete operand/kind types (`ScgExpr`,
//! `ScgType`, `CastKind`, `BinOpKind`, `UnaryOpKind`, `IRType`) remain
//! codegen-local pending their own migration.
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
//!     ComputationKind, ComputationNode, EdgeKind, ProgramPoint,
//!     SCGQuery, execute,
//! };
//!
//! let mut scg = SCG::new();
//!
//! let n1 = scg.add_node(
//!     NodeType::Computation,
//!     NodePayload::Computation(ComputationNode {
//!         kind: ComputationKind::Other("add".to_string()),
//!         result_type: Some("i32".to_string()),
//!         tail_call: false }),
//!     ProgramPoint { file: None, line: None, column: None, offset: None },
//! );
//!
//! let result = execute(&scg, SCGQuery::NodesByType(NodeType::Computation));
//! assert_eq!(result.node_ids.len(), 1);
//! ```

pub mod callgraph;
pub mod diff;
pub mod digraph;
pub mod dominance;
pub mod edge;
pub mod graph;
pub mod hash;
pub mod liveness;
pub mod llm_json;
pub mod loop_detection;
pub mod node;
pub mod query;
pub mod region;
pub mod serialize;
pub mod structured_output;
pub mod transform;

// Re-export the primary public API at the crate root for convenience.

// -- Node types --
//
// Re-exports the full set of node payloads from `node.rs` at the crate root
// so downstream crates (notably `vuma-codegen`'s `scg_to_ir.rs`, which is
// unifying on the semantic SCG per Task 4) can write `vuma_scg::ComputationNode`
// rather than `vuma_scg::node::ComputationNode`. Keep this list in sync with
// the `pub struct`/`pub enum` declarations in `node.rs` (see Task 4-b in
// worklog.md).
pub use node::{
    AccessMode,
    AccessNode,
    AllocationNode,
    ArenaAllocNode,
    ArenaFreeNode,
    ArenaGrowNode,
    ArenaNewNode,
    BDReference,
    CastNode,
    ChannelCloseNode,
    ChannelOpenNode,
    ChannelRecvNode,
    ChannelSendNode,
    ClosureEnvNode,
    CodegenAccessNode,
    // (Wave 4-b) Generic statement-list-shaped allocation/access node
    // types — the structure of the codegen-side AllocationNode /
    // AccessNode (both enums, unlike their graph-shaped struct
    // counterparts above) now lives in the canonical SCG crate.
    // Codegen creates concrete aliases:
    //   type AllocationNode = vuma_scg::CodegenAllocationNode<ScgExpr, ScgType>;
    //   type AccessNode     = vuma_scg::CodegenAccessNode<ScgExpr, crate::ir::IRType>;
    CodegenAllocationNode,
    CodegenCallNode,
    // (Wave 4-e) Generic statement-list-shaped node types — the structure
    // of the codegen-side CastNode / ControlNode / SwitchArm now lives in
    // the canonical SCG crate.  Codegen creates concrete aliases:
    //   type CastNode    = vuma_scg::CodegenCastNode<ScgExpr, ScgType, CastKind>;
    //   type ControlNode = vuma_scg::CodegenControlNode<ScgExpr, ScgStatement>;
    //   type SwitchArm   = vuma_scg::CodegenSwitchArm<ScgStatement>;
    CodegenCastNode,
    // (Wave 12-a) Generic statement-list-shaped binary-computation node
    // type — the structure of the codegen-side ComputationNode (struct
    // with dst/op/lhs/rhs/tail_call/reassigns) now lives in the canonical
    // SCG crate.  Codegen creates a concrete alias:
    //   type ComputationNode = vuma_scg::CodegenComputationNode<ScgExpr, BinOpKind>;
    CodegenComputationNode,
    CodegenControlNode,
    CodegenSwitchArm,
    // (Wave 13-c) Generic statement-list-shaped unary-computation and
    // function-call node types — both are codegen-only (no semantic-SCG
    // counterpart).  Codegen creates concrete aliases:
    //   type UnaryComputationNode = vuma_scg::CodegenUnaryComputationNode<ScgExpr, UnaryOpKind>;
    //   type CallNode             = vuma_scg::CodegenCallNode<ScgExpr>;
    CodegenUnaryComputationNode,
    ComputationKind,
    ComputationNode,
    ConstantTimeNode,
    ConstantTimeOp,
    ControlKind,
    ControlNode,
    DeallocationNode,
    EffectNode,
    EnumDefNode,
    EnumVariantInfo,
    ForeignConsumeNode,
    IntrinsicKind,
    MatchArmInfo,
    MatchNode,
    MatchPatternInfo,
    NodeData,
    NodeId,
    NodePayload,
    NodeType,
    PhantomNode,
    ProgramPoint,
    StateInitNode,
    StateReadNode,
    StateTransformNode,
    StateWriteNode,
    StructDefNode,
    StructFieldInfo,
    SyscallNode,
    VTableNode,
};

/// Central visitor trait for dispatching on NodePayload.
/// Eliminates the "11 duplicated match statements" DRY violation.
pub use node::NodeVisitor;

// -- Hashing utilities --
//
// Re-export the FNV-1a 64-bit single-source-of-truth hash functions so
// downstream crates can write `vuma_scg::type_hash` rather than
// `vuma_scg::hash::type_hash`. Per Task 1-b / Gap 6 fix, this is the canonical
// type-hash used across IVE and codegen (previously duplicated in 7 places).
pub use hash::{fnv1a_64, type_hash};

// -- Edge types --
pub use edge::{EdgeData, EdgeId, EdgeKind};

// -- Graph --
pub use graph::{SCGError, ValidationResult, SCG};

// -- Call Graph --
pub use callgraph::{CallGraph, CallGraphEdge, FunctionId};

// -- Region types --
pub use region::{
    infer_regions, DeploymentTarget, InferredRegion, RegionAliasAnalysis, RegionId, RegionLifetime,
    SCGRegion,
};

// -- Query engine --
pub use query::{
    execute, find_access_nodes_to_region, find_derivation_chains, DerivationChain, FunctionInfo,
    QueryResult, SCGQuery,
};

// -- Diff engine --
pub use diff::{
    apply_diff, compute_edit_script, diff_scg, scg_diff, three_way_merge, AffectedFunctions,
    DiffEntry, DiffError, DiffStats, EdgeConflict, LlmDiff, LlmDiffChange, MergeConflict,
    NodeConflict, RegionConflict, SCGDiff,
};

// -- Dominance analysis --
pub use dominance::{
    always_executes_after, compute_dominators, compute_post_dominators, dom_tree_postorder,
    dominated_by, dominates, dominators_of, find_dominance_frontier, guaranteed_execution_path,
    nearest_common_dominator, strictly_dominates, write_precedes_read, DominatorTree,
};

// -- Liveness analysis --
pub use liveness::{
    compute_liveness, find_dead_allocations, find_dead_code, find_uninitialized_reads,
    find_use_after_free, LivenessAnalysis, LivenessInfo, UseAfterFree,
};

// -- Loop detection --
pub use loop_detection::{ControlFlowGraph, LoopDetector, LoopNestingTree, NaturalLoop};

// -- Transform passes --
pub use transform::{
    dead_region_elim, detect_tail_calls, licm, strength_reduce, CommonSubexpressionElimination,
    ConstantFolding, DeadCodeElimination, DeadRegionElimination, InliningPass,
    InterproceduralAllocFlow, LoopInvariantCodeMotion, PassManager, PassResult, PipelineResult,
    SCGPass, StrengthReduction, TailCallOptDetection, VerificationPass,
};

// -- Structured output for LLMs --
pub use structured_output::{
    LlmEdge, LlmFunction, LlmNode, LlmRegion, LlmScgJson, LlmSourceLocation, LlmSummary,
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
                kind: ComputationKind::Other("write_buffer".to_string()),
                result_type: None,
                tail_call: false,
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
        scg.add_edge(alloc_id, dealloc_id, EdgeKind::Derivation)
            .unwrap();
        scg.add_edge(comp_id, dealloc_id, EdgeKind::ControlFlow)
            .unwrap();

        // Validate
        let validation = scg.validate();
        assert!(
            validation.is_valid,
            "Validation errors: {:?}",
            validation.errors
        );

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
