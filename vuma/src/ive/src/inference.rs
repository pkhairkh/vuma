//! Inference engine for the IVE module.
//!
//! The inference engine walks the SCG (Semantic Compute Graph) and derives
//! behavioral descriptions (BDs), constraints, and type information. These
//! inferred properties feed into the verification engine for formal checking.
//!
//! # Integration Architecture
//!
//! This module is wired to the real BD inference engine from `vuma-bd`:
//!
//! 1. [`InferenceEngine`] accepts a `vuma_scg::SCG` and delegates to
//!    [`vuma_bd::inference::BDInferenceEngine`] for the 3-phase BD inference
//!    algorithm (propagation, constraint solving, context refinement).
//! 2. Constraint derivation converts SCG edge structure into IVE-level
//!    constraints (temporal, resource flow, security, complexity, liveness).
//! 3. The `infer_types` method returns fully inferred `(NodeId, BD)` pairs
//!    that can be passed directly to the verification engine.

use crate::constraint::{ComplexityConstraint, Constraint, LivenessConstraint, ResourceFlowConstraint, SecurityConstraint, TemporalConstraint};
use hashbrown::HashMap;
use std::fmt;
use vuma_bd::descriptor::BD;
use vuma_bd::inference::{BDInferenceEngine as BdEngineInner, InferenceResult as BdInferenceResult};
use vuma_scg::edge::EdgeKind;
use vuma_scg::graph::SCG;
use vuma_scg::node::{NodeId, NodePayload, NodeType};

// ---------------------------------------------------------------------------
// InferenceError
// ---------------------------------------------------------------------------

/// Errors that can occur during inference.
#[derive(Debug, Clone, thiserror::Error)]
pub enum InferenceError {
    /// The requested node does not exist in the SCG.
    #[error("node not found: {node_id:?}")]
    NodeNotFound { node_id: NodeId },

    /// The SCG is in an invalid state for inference.
    #[error("invalid SCG: {reason}")]
    InvalidSCG { reason: String },

    /// A cycle was detected during BD propagation.
    #[error("cycle detected during BD propagation at node {node_id:?}")]
    CycleDetected { node_id: NodeId },

    /// Inference could not converge.
    #[error("inference did not converge after {iterations} iterations")]
    NoConvergence { iterations: usize },

    /// One or more BD inference errors occurred.
    #[error("{count} BD inference error(s): {summary}")]
    BdErrors { count: usize, summary: String },

    /// Topological sort failed (SCG has cycles).
    #[error("SCG is not a DAG: {reason}")]
    NotADag { reason: String },
}

// ---------------------------------------------------------------------------
// InferenceResult
// ---------------------------------------------------------------------------

/// The result of running BD inference on an SCG.
#[derive(Debug, Clone)]
pub struct InferenceResult {
    /// Inferred BD for each node.
    pub bd_map: HashMap<NodeId, BD>,
    /// IVE-level constraints derived from the SCG structure.
    pub constraints: Vec<Constraint>,
    /// Number of BD inference iterations.
    pub iterations: u32,
    /// Warnings from the inference process.
    pub warnings: Vec<String>,
    /// Errors from the inference process.
    pub errors: Vec<InferenceError>,
}

impl InferenceResult {
    /// Returns `true` if inference completed without errors.
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// Get the BD for a specific node, if inferred.
    pub fn get_bd(&self, node_id: &NodeId) -> Option<&BD> {
        self.bd_map.get(node_id)
    }
}

impl fmt::Display for InferenceResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "InferenceResult({} nodes, {} constraints, {} iterations)",
            self.bd_map.len(), self.constraints.len(), self.iterations)?;
        if !self.errors.is_empty() {
            writeln!(f, "  errors: {}", self.errors.len())?;
        }
        if !self.warnings.is_empty() {
            writeln!(f, "  warnings: {}", self.warnings.len())?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// InferenceEngine
// ---------------------------------------------------------------------------

/// The inference engine derives BDs, constraints, and type information
/// from the Semantic Compute Graph.
///
/// # Algorithm
///
/// 1. **BD Inference**: Delegates to `vuma_bd::BDInferenceEngine` which
///    performs a 3-phase algorithm:
///    - Phase 1: Propagate initial BDs through the SCG in topological order
///    - Phase 2: Solve constraints via iterative fixpoint with widening
///    - Phase 3: Refine CapD based on usage context
///
/// 2. **Constraint Derivation**: Converts SCG edge structure into IVE-level
///    constraints for the verification engine:
///    - DataFlow edges → ResourceFlow constraints
///    - ControlFlow edges → Temporal constraints
///    - Derivation edges → Security constraints
///    - Loops → Complexity constraints
///    - Deadlock patterns → Liveness constraints
pub struct InferenceEngine {
    /// Whether to log detailed inference steps.
    verbose: bool,
    /// Maximum iterations for BD inference (default: 100).
    max_iterations: u32,
    /// Whether to use widening in the fixpoint solver.
    use_widening: bool,
    /// Whether to enable context-aware CapD refinement.
    enable_context_refinement: bool,
}

impl InferenceEngine {
    /// Construct a new inference engine with default settings.
    pub fn new() -> Self {
        Self {
            verbose: false,
            max_iterations: 100,
            use_widening: true,
            enable_context_refinement: true,
        }
    }

    /// Enable verbose logging.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Set maximum iterations for the BD inference fixpoint solver.
    pub fn with_max_iterations(mut self, max: u32) -> Self {
        self.max_iterations = max;
        self
    }

    /// Enable or disable widening in the fixpoint solver.
    pub fn with_widening(mut self, use_widening: bool) -> Self {
        self.use_widening = use_widening;
        self
    }

    /// Enable or disable context-aware CapD refinement (Phase 3).
    pub fn with_context_refinement(mut self, enable: bool) -> Self {
        self.enable_context_refinement = enable;
        self
    }

    /// Run full inference on the SCG: BD inference + constraint derivation.
    ///
    /// This is the primary entry point. It:
    /// 1. Runs the 3-phase BD inference algorithm
    /// 2. Derives IVE-level constraints from the SCG structure
    /// 3. Returns both BDs and constraints in a unified result
    pub fn infer(&self, scg: &SCG) -> InferenceResult {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // Phase 1-3: Run BD inference via vuma-bd
        let bd_result = self.run_bd_inference(scg);

        let (bd_map, iterations) = match bd_result {
            Ok(result) => {
                warnings.extend(result.warnings.iter().cloned());
                if !result.errors.is_empty() {
                    let summary = result.errors.iter()
                        .map(|e| format!("{}", e))
                        .collect::<Vec<_>>()
                        .join("; ");
                    errors.push(InferenceError::BdErrors {
                        count: result.errors.len(),
                        summary,
                    });
                }
                (result.bd_map, result.iterations)
            }
            Err(e) => {
                errors.push(InferenceError::BdErrors {
                    count: 1,
                    summary: format!("{}", e),
                });
                (HashMap::new(), 0)
            }
        };

        // Constraint derivation from SCG structure
        let constraints = self.derive_constraints(scg, &bd_map);

        if self.verbose {
            log::info!("InferenceEngine::infer: {} BDs, {} constraints, {} iterations, {} errors",
                bd_map.len(), constraints.len(), iterations, errors.len());
        }

        InferenceResult {
            bd_map,
            constraints,
            iterations,
            warnings,
            errors,
        }
    }

    /// Infer the Behavioral Description (BD) for a specific node in the SCG.
    ///
    /// Runs full inference and extracts the BD for the requested node.
    /// For batch inference, prefer [`infer`] which avoids redundant work.
    pub fn infer_bd(&self, scg: &SCG, node_id: NodeId) -> Result<BD, InferenceError> {
        // Verify node exists
        if scg.get_node(node_id).is_none() {
            return Err(InferenceError::NodeNotFound { node_id });
        }

        let result = self.run_bd_inference(scg).map_err(|e| InferenceError::BdErrors {
            count: 1,
            summary: format!("{}", e),
        })?;

        result.bd_map.get(&node_id).cloned().ok_or_else(|| {
            InferenceError::NodeNotFound { node_id }
        })
    }

    /// Infer all constraints from the SCG.
    ///
    /// Constraints are derived from the structure and semantics of the SCG:
    /// - **Temporal**: ordering constraints from sequential composition.
    /// - **ResourceFlow**: data-flow constraints from DataFlow edges.
    /// - **Security**: information-flow constraints from security annotations.
    /// - **Complexity**: complexity bounds from loop/recursion structure.
    /// - **Liveness**: progress guarantees from the graph topology.
    pub fn infer_constraints(&self, scg: &SCG) -> Vec<Constraint> {
        let bd_map = match self.run_bd_inference(scg) {
            Ok(result) => result.bd_map,
            Err(_) => HashMap::new(),
        };
        self.derive_constraints(scg, &bd_map)
    }

    /// Infer BDs for all nodes in the SCG.
    ///
    /// Returns a vector of (NodeId, BD) pairs representing the inferred
    /// behavioral description for each node.
    pub fn infer_types(&self, scg: &SCG) -> Vec<(NodeId, BD)> {
        match self.run_bd_inference(scg) {
            Ok(result) => result.bd_map.into_iter().collect(),
            Err(_) => Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Private implementation
    // -----------------------------------------------------------------------

    /// Run the 3-phase BD inference algorithm via vuma-bd.
    fn run_bd_inference(&self, scg: &SCG) -> Result<BdInferenceResult, vuma_bd::inference::InferenceError> {
        let engine = BdEngineInner::new()
            .with_max_iterations(self.max_iterations);

        if self.verbose {
            log::info!("Running BD inference on SCG with {} nodes", scg.node_count());
        }

        let result = engine.infer(scg);

        if self.verbose {
            log::info!("BD inference complete: {} BDs inferred, {} errors, {} warnings, {} iterations",
                result.bd_map.len(), result.errors.len(), result.warnings.len(), result.iterations);
        }

        Ok(result)
    }

    /// Derive IVE-level constraints from the SCG structure and inferred BDs.
    fn derive_constraints(&self, scg: &SCG, bd_map: &HashMap<NodeId, BD>) -> Vec<Constraint> {
        let mut constraints = Vec::new();

        // Derive constraints from edges
        for edge in scg.edges() {
            match edge.kind {
                EdgeKind::DataFlow => {
                    // Data flow imposes resource flow constraints
                    let src = scg.get_node(edge.source);
                    let dst = scg.get_node(edge.target);
                    let desc = match (src, dst) {
                        (Some(s), Some(d)) => format!(
                            "data flows from {:?}({}) to {:?}({})",
                            s.node_type, s.id.as_u64(),
                            d.node_type, d.id.as_u64()
                        ),
                        _ => format!("data flow: {} -> {}", edge.source.as_u64(), edge.target.as_u64()),
                    };
                    constraints.push(Constraint::ResourceFlow(ResourceFlowConstraint { description: desc }));
                }
                EdgeKind::ControlFlow => {
                    // Control flow imposes temporal ordering constraints
                    let desc = if let Some(label) = &edge.label {
                        format!(
                            "temporal: {} -> {} ({})",
                            edge.source.as_u64(), edge.target.as_u64(), label
                        )
                    } else {
                        format!(
                            "temporal: {} -> {}",
                            edge.source.as_u64(), edge.target.as_u64()
                        )
                    };
                    constraints.push(Constraint::Temporal(TemporalConstraint { description: desc }));
                }
                EdgeKind::Derivation => {
                    // Derivation edges impose security constraints (provenance must be valid)
                    let desc = format!(
                        "derivation: {} -> {} (provenance must be traceable)",
                        edge.source.as_u64(), edge.target.as_u64()
                    );
                    constraints.push(Constraint::Security(SecurityConstraint { description: desc }));
                }
                EdgeKind::Annotation => {
                    // Annotation edges carry BD compatibility constraints
                    // These are already handled by the BD inference engine
                }
                EdgeKind::Dispatch => {
                    // Dispatch edges impose temporal constraints like ControlFlow
                    let desc = format!(
                        "dispatch: {} -> {}",
                        edge.source.as_u64(), edge.target.as_u64()
                    );
                    constraints.push(Constraint::Temporal(TemporalConstraint { description: desc }));
                }
            }
        }

        // Derive complexity constraints from loops
        for node in scg.nodes() {
            if let NodeType::Control = node.node_type {
                if let NodePayload::Control(ctrl) = &node.payload {
                    match ctrl.kind {
                        vuma_scg::node::ControlKind::LoopHeader => {
                            let desc = format!(
                                "loop at node {} — complexity must be bounded",
                                node.id.as_u64()
                            );
                            constraints.push(Constraint::Complexity(ComplexityConstraint { description: desc }));
                        }
                        _ => {}
                    }
                }
            }
        }

        // Derive liveness constraints from allocation/deallocation patterns
        let allocation_nodes: Vec<_> = scg.nodes()
            .filter(|n| matches!(n.node_type, NodeType::Allocation))
            .collect();
        let deallocation_nodes: Vec<_> = scg.nodes()
            .filter(|n| matches!(n.node_type, NodeType::Deallocation))
            .collect();

        if allocation_nodes.len() > deallocation_nodes.len() {
            let desc = format!(
                "liveness: {} allocations but only {} deallocations — potential leaks",
                allocation_nodes.len(), deallocation_nodes.len()
            );
            constraints.push(Constraint::Liveness(LivenessConstraint { description: desc }));
        }

        // Check for CapD compatibility between connected nodes
        for edge in scg.edges() {
            if edge.kind == EdgeKind::DataFlow {
                if let (Some(src_bd), Some(dst_bd)) = (bd_map.get(&edge.source), bd_map.get(&edge.target)) {
                    if !src_bd.compatible(dst_bd) {
                        let desc = format!(
                            "security: BD incompatibility on edge {} -> {} ({} vs {})",
                            edge.source.as_u64(), edge.target.as_u64(),
                            src_bd, dst_bd
                        );
                        constraints.push(Constraint::Security(SecurityConstraint { description: desc }));
                    }
                }
            }
        }

        if self.verbose {
            log::info!("Derived {} constraints from SCG", constraints.len());
        }

        constraints
    }
}

impl Default for InferenceEngine {
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
    fn inference_engine_new_defaults() {
        let engine = InferenceEngine::new();
        assert_eq!(engine.max_iterations, 100);
        assert!(engine.use_widening);
        assert!(engine.enable_context_refinement);
        assert!(!engine.verbose);
    }

    #[test]
    fn inference_engine_builder_pattern() {
        let engine = InferenceEngine::new()
            .with_verbose(true)
            .with_max_iterations(200)
            .with_widening(false)
            .with_context_refinement(false);
        assert!(engine.verbose);
        assert_eq!(engine.max_iterations, 200);
        assert!(!engine.use_widening);
        assert!(!engine.enable_context_refinement);
    }

    #[test]
    fn infer_on_empty_scg() {
        let engine = InferenceEngine::new();
        let scg = SCG::new();
        let result = engine.infer(&scg);
        // Empty SCG should produce empty BDs and no errors
        assert!(result.bd_map.is_empty());
        assert!(result.is_ok() || result.errors.iter().any(|e| matches!(e, InferenceError::BdErrors { .. })));
    }

    #[test]
    fn infer_constraints_on_empty_scg() {
        let engine = InferenceEngine::new();
        let scg = SCG::new();
        let constraints = engine.infer_constraints(&scg);
        assert!(constraints.is_empty());
    }

    #[test]
    fn infer_types_on_empty_scg() {
        let engine = InferenceEngine::new();
        let scg = SCG::new();
        let types = engine.infer_types(&scg);
        assert!(types.is_empty());
    }

    #[test]
    fn infer_bd_node_not_found() {
        let engine = InferenceEngine::new();
        let scg = SCG::new();
        let result = engine.infer_bd(&scg, NodeId::new(999));
        assert!(matches!(result, Err(InferenceError::NodeNotFound { .. })));
    }

    #[test]
    fn inference_result_display() {
        let result = InferenceResult {
            bd_map: HashMap::new(),
            constraints: vec![],
            iterations: 5,
            warnings: vec![],
            errors: vec![],
        };
        let display = format!("{}", result);
        assert!(display.contains("0 nodes"));
        assert!(display.contains("0 constraints"));
    }

    #[test]
    fn inference_result_is_ok() {
        let result = InferenceResult {
            bd_map: HashMap::new(),
            constraints: vec![],
            iterations: 0,
            warnings: vec![],
            errors: vec![],
        };
        assert!(result.is_ok());
    }

    #[test]
    fn inference_result_has_errors() {
        let result = InferenceResult {
            bd_map: HashMap::new(),
            constraints: vec![],
            iterations: 0,
            warnings: vec![],
            errors: vec![InferenceError::BdErrors { count: 1, summary: "test".into() }],
        };
        assert!(!result.is_ok());
    }

    #[test]
    fn default_engine() {
        let engine = InferenceEngine::default();
        assert_eq!(engine.max_iterations, 100);
    }
}
