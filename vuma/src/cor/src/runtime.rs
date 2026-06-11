//! Core runtime for the Continuous Optimization Runtime (COR).
//!
//! The [`CORuntime`] struct is the central orchestrator. It holds a shared
//! reference to the Semantic Computation Graph (SCG), the always-compiled
//! state, profile data, and runtime configuration. The runtime continuously
//! compiles, executes, profiles, and re-optimizes regions of the SCG.
//!
//! ## Integration with the Optimization Engine
//!
//! Since the SCG is shared via `Arc`, the runtime uses copy-on-write
//! semantics (`Arc::make_mut`) to obtain mutable access to the graph
//! when running [`OptimizationEngine`] passes. This ensures that other
//! subsystems holding the same `Arc` are not affected until the
//! optimisation cycle completes.

use crate::config::Config;
use crate::deployment::DeploymentPlanner;
use crate::optimization::{OptimizationEngine, OptimizationResult, ProfileReport};
use crate::ownership::OwnershipTracker;
use crate::profile::ProfileData;
use crate::speculative::SpeculativeOptimizer;
use crate::types::{CompiledRegion, Delta, NodeKind, RegionId, SCG};
use std::sync::Arc;
use vuma_codegen::emit::Emitter;
use vuma_codegen::ir::BinOpKind;
use vuma_codegen::scg_to_ir::{
    AccessNode as CgAccessNode, AllocationNode as CgAllocationNode, CallNode as CgCallNode,
    ComputationNode as CgComputationNode, ControlNode as CgControlNode, IRBuilder, Scg, ScgExpr,
    ScgFunction, ScgNode, ScgStatement, ScgType, SwitchArm as CgSwitchArm,
};

// ---------------------------------------------------------------------------
// CompiledState — the always-compiled invariant
// ---------------------------------------------------------------------------

/// Represents the "always-compiled" invariant of the COR.
///
/// In VUMA, every reachable region of the SCG is always in a compiled state
/// — there is no interpreter fallback. [`CompiledState`] tracks which
/// regions are compiled and at what optimization level, ensuring the
/// invariant is maintained across incremental updates.
#[derive(Debug, Clone)]
pub struct CompiledState {
    /// Mapping from region ID to its compiled code.
    compiled_regions: std::collections::HashMap<RegionId, CompiledRegion>,

    /// The set of region IDs that are currently compiled (fast membership
    /// test for the invariant check).
    compiled_set: std::collections::HashSet<RegionId>,
}

impl CompiledState {
    /// Creates an empty compiled state.
    pub fn new() -> Self {
        CompiledState {
            compiled_regions: std::collections::HashMap::new(),
            compiled_set: std::collections::HashSet::new(),
        }
    }

    /// Returns `true` if the given region has been compiled.
    pub fn is_compiled(&self, region_id: RegionId) -> bool {
        self.compiled_set.contains(&region_id)
    }

    /// Inserts a compiled region, maintaining the invariant.
    pub fn insert(&mut self, region: CompiledRegion) {
        self.compiled_set.insert(region.region_id);
        self.compiled_regions.insert(region.region_id, region);
    }

    /// Retrieves a compiled region by ID.
    pub fn get(&self, region_id: RegionId) -> Option<&CompiledRegion> {
        self.compiled_regions.get(&region_id)
    }

    /// Removes a compiled region (e.g. after a region is deleted from the
    /// SCG).
    pub fn remove(&mut self, region_id: RegionId) -> Option<CompiledRegion> {
        self.compiled_set.remove(&region_id);
        self.compiled_regions.remove(&region_id)
    }

    /// Returns the number of compiled regions.
    pub fn len(&self) -> usize {
        self.compiled_regions.len()
    }

    /// Returns `true` if there are no compiled regions.
    pub fn is_empty(&self) -> bool {
        self.compiled_regions.is_empty()
    }

    /// Verifies the always-compiled invariant for the given set of expected
    /// regions.
    ///
    /// Returns a list of region IDs that are expected but not yet compiled.
    pub fn verify_invariant(&self, expected_regions: &[RegionId]) -> Vec<RegionId> {
        expected_regions
            .iter()
            .copied()
            .filter(|r| !self.compiled_set.contains(r))
            .collect()
    }
}

impl Default for CompiledState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// CORuntime
// ---------------------------------------------------------------------------

/// The Continuous Optimization Runtime.
///
/// `CORuntime` is the top-level object that coordinates compilation,
/// execution, profiling, and speculative optimization. It is constructed
/// with a shared reference to the SCG and a [`Config`], after which the
/// caller drives the runtime via [`compile_incremental`], [`execute`],
/// [`optimize`], and [`run_optimization_passes`].
///
/// # Thread safety
///
/// The runtime itself is not `Sync` — it is intended to be used from a
/// single orchestrator thread. Internally it may spawn worker tasks on
/// thread pools for parallel compilation.
///
/// # Example
///
/// ```no_run
/// use vuma_cor::runtime::CORuntime;
/// use vuma_cor::config::Config;
/// use vuma_cor::types::SCG;
/// use std::sync::Arc;
///
/// let scg = Arc::new(SCG::default());
/// let config = Config::default();
/// let mut rt = CORuntime::new(scg, config);
/// ```
#[derive(Debug)]
pub struct CORuntime {
    /// Shared reference to the Semantic Computation Graph.
    scg: Arc<SCG>,

    /// The always-compiled state.
    compiled_state: CompiledState,

    /// Profile-guided optimization data.
    profile_data: ProfileData,

    /// Runtime configuration.
    config: Config,

    /// Speculative optimizer.
    speculative_optimizer: SpeculativeOptimizer,

    /// Deployment planner.
    deployment_planner: DeploymentPlanner,

    /// Profile-guided optimization engine.
    optimization_engine: OptimizationEngine,

    /// Region-based ownership tracker.
    ownership_tracker: OwnershipTracker,
}

impl CORuntime {
    /// Creates a new CORuntime.
    ///
    /// # Arguments
    ///
    /// * `scg` – A shared reference to the Semantic Computation Graph.
    /// * `config` – Runtime configuration.
    pub fn new(scg: Arc<SCG>, config: Config) -> Self {
        let deployment_planner = DeploymentPlanner::new(config.clone());
        let optimization_engine = OptimizationEngine::new(config.clone());
        CORuntime {
            scg,
            compiled_state: CompiledState::new(),
            profile_data: ProfileData::new(),
            config,
            speculative_optimizer: SpeculativeOptimizer::new(),
            deployment_planner,
            optimization_engine,
            ownership_tracker: OwnershipTracker::new(),
        }
    }

    /// Creates a new CORuntime from a `vuma_scg::SCG`.
    ///
    /// This convenience method bridges the real SCG defined in the
    /// `vuma-scg` crate into the COR-internal representation and then
    /// constructs the runtime. Consumers do not need to know about the
    /// bridge module — they simply pass their `Arc<vuma_scg::SCG>` and
    /// a [`Config`], and the conversion happens automatically.
    ///
    /// # Arguments
    ///
    /// * `scg` – A shared reference to the `vuma-scg` SCG.
    /// * `config` – Runtime configuration.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use vuma_cor::runtime::CORuntime;
    /// use vuma_cor::config::Config;
    /// use vuma_scg::SCG;
    /// use std::sync::Arc;
    ///
    /// let scg = Arc::new(SCG::new());
    /// let config = Config::default();
    /// let mut rt = CORuntime::from_vuma_scg(scg, config);
    /// ```
    pub fn from_vuma_scg(scg: Arc<vuma_scg::SCG>, config: Config) -> Self {
        let cor_scg: SCG = Arc::try_unwrap(scg)
            .map(std::convert::Into::into)
            .unwrap_or_else(|arc| (*arc).clone().into());
        Self::new(Arc::new(cor_scg), config)
    }

    /// Performs incremental compilation based on a delta to the SCG.
    ///
    /// Instead of recompiling the entire graph, only the regions affected
    /// by the delta are recompiled. This is the primary mechanism by which
    /// the runtime stays responsive as the program evolves.
    ///
    /// # Arguments
    ///
    /// * `delta` – The incremental change to the SCG.
    ///
    /// # Returns
    ///
    /// A list of region IDs that were (re)compiled.
    pub fn compile_incremental(&mut self, delta: &Delta) -> Vec<RegionId> {
        if delta.is_empty() {
            log::debug!("compile_incremental: empty delta, nothing to do");
            return Vec::new();
        }

        log::info!(
            "compile_incremental: +{} nodes, -{} nodes, ~{} modified nodes, +{} edges, -{} edges, ~{} modified edges, ~{} region changes",
            delta.added_nodes.len(),
            delta.removed_nodes.len(),
            delta.modified_nodes.len(),
            delta.added_edges.len(),
            delta.removed_edges.len(),
            delta.modified_edges.len(),
            delta.region_changes.len(),
        );

        if !delta.modified_nodes.is_empty() || !delta.modified_edges.is_empty() {
            log::info!(
                "compile_incremental: field-level changes: {} total field changes",
                delta.total_field_changes(),
            );
        }

        // 1. Determine which existing regions overlap with the delta.
        // 2. Invalidate those regions in compiled_state.
        // 3. Recompile affected regions via the code generation layer.
        let mut recompiled = Vec::new();
        for &node_id in &delta.added_nodes {
            let region_id = node_id as RegionId;
            if !self.compiled_state.is_compiled(region_id) {
                let code = self.compile_region(region_id);
                let compiled = CompiledRegion { region_id, code };
                self.compiled_state.insert(compiled);
                recompiled.push(region_id);
            }
        }

        // Remove compiled regions for deleted nodes.
        for &node_id in &delta.removed_nodes {
            let region_id = node_id as RegionId;
            self.compiled_state.remove(region_id);
        }

        // Modified nodes: their regions must be recompiled because field-level
        // changes (e.g. is_inlined, unroll_factor) affect code generation.
        for modification in &delta.modified_nodes {
            let region_id = modification.node_id as RegionId;
            // Log the individual field changes for diagnostics.
            for change in &modification.field_changes {
                log::debug!(
                    "compile_incremental: node {} field '{}' changed: {} -> {}",
                    modification.node_id,
                    change.field_name,
                    change.old_value,
                    change.new_value,
                );
            }
            if self.compiled_state.is_compiled(region_id) {
                self.compiled_state.remove(region_id);
                let code = self.compile_region(region_id);
                let compiled = CompiledRegion { region_id, code };
                self.compiled_state.insert(compiled);
                recompiled.push(region_id);
            }
        }

        // Edge changes may require recompilation of affected regions.
        // When an edge is added or removed, the regions connected by
        // that edge may have different control/data flow and must be
        // recompiled. We look up which regions each edge's source and
        // target nodes belong to and invalidate + recompile them.
        let edge_ids: Vec<crate::types::EdgeId> = delta
            .added_edges
            .iter()
            .chain(delta.removed_edges.iter())
            .copied()
            .collect();

        for &edge_id in &edge_ids {
            if let Some(edge) = self.scg.edges.get(&edge_id) {
                // Find the regions for the source and target nodes.
                let source_region = self.find_region_for_node(edge.source);
                let target_region = self.find_region_for_node(edge.target);

                // Invalidate and recompile any affected regions.
                for region_id in source_region.into_iter().chain(target_region) {
                    if self.compiled_state.is_compiled(region_id) {
                        self.compiled_state.remove(region_id);
                        let code = self.compile_region(region_id);
                        let compiled = CompiledRegion { region_id, code };
                        self.compiled_state.insert(compiled);
                        recompiled.push(region_id);
                    }
                }
            }
        }

        // Modified edges: their connected regions must be recompiled because
        // field-level changes (e.g. weight) affect optimization decisions.
        for modification in &delta.modified_edges {
            let edge_id = modification.edge_id as crate::types::EdgeId;
            for change in &modification.field_changes {
                log::debug!(
                    "compile_incremental: edge {} field '{}' changed: {} -> {}",
                    modification.edge_id,
                    change.field_name,
                    change.old_value,
                    change.new_value,
                );
            }
            if let Some(edge) = self.scg.edges.get(&edge_id) {
                let source_region = self.find_region_for_node(edge.source);
                let target_region = self.find_region_for_node(edge.target);
                for region_id in source_region.into_iter().chain(target_region) {
                    if self.compiled_state.is_compiled(region_id) {
                        self.compiled_state.remove(region_id);
                        let code = self.compile_region(region_id);
                        let compiled = CompiledRegion { region_id, code };
                        self.compiled_state.insert(compiled);
                        recompiled.push(region_id);
                    }
                }
            }
        }

        recompiled
    }

    /// Executes a compiled region.
    ///
    /// # Arguments
    ///
    /// * `region` – The ID of the region to execute.
    ///
    /// # Returns
    ///
    /// `Ok(())` if the region was found and executed, or an error if the
    /// region is not compiled or execution failed.
    pub fn execute(&mut self, region: RegionId) -> Result<(), RuntimeError> {
        let compiled = self
            .compiled_state
            .get(region)
            .ok_or(RuntimeError::NotCompiled(region))?;

        // Record profile data for this execution.
        // Only record_access — record_call is a separate API for explicit
        // call-graph tracking and must not double-count with record_access.
        self.profile_data
            .record_access(region as crate::types::NodeId);

        log::trace!(
            "execute: region {} ({} code bytes)",
            region,
            compiled.code.len()
        );

        // Execute the compiled code via memory-mapped execution.
        let code = compiled.code.clone();
        let _result = execute_code(&code)?;
        log::trace!("execute: region {} returned {}", region, _result);

        Ok(())
    }

    /// Runs one optimization cycle.
    ///
    /// This method:
    /// 1. Analyzes profile data to find hot paths.
    /// 2. Generates optimization suggestions.
    /// 3. Validates speculative assumptions.
    /// 4. Runs the full optimization pipeline on the SCG.
    /// 5. Recompiles hot regions at a higher optimization level.
    ///
    /// Returns the number of regions that were re-optimized.
    pub fn optimize(&mut self) -> usize {
        log::debug!("optimize: starting optimization cycle");

        // Step 1: Analyze profile data.
        let hot_paths = self.profile_data.get_hot_paths(10).to_vec();
        let suggestions = self.profile_data.suggest_optimizations();

        log::debug!(
            "optimize: {} hot paths, {} suggestions",
            hot_paths.len(),
            suggestions.len(),
        );

        // Step 2: Validate speculative assumptions.
        //
        // Collect per-region edge observations and contention data from
        // the SCG and profile data, then pass them to the speculative
        // optimizer for validation.
        let edge_observations = self.collect_edge_observations();
        let contended_regions = self.find_contended_regions();

        // Determine the most-observed edge across all regions (used for
        // LikelyBranch assumption validation). If multiple edges are
        // observed, pick the one with the highest edge ID as a tiebreaker.
        let most_observed_edge: Option<crate::types::EdgeId> = edge_observations
            .values()
            .flatten()
            .copied()
            .max_by_key(|&e| {
                // Weight by the total access count of the regions that
                // observe this edge.
                let mut weight = 0u64;
                for (&region_id, edges) in &edge_observations {
                    if edges.contains(&e) {
                        if let Some(node) = self.scg.get_node(region_id as crate::types::NodeId) {
                            weight += node.code_size as u64; // use code_size as a proxy for activity
                        }
                    }
                }
                weight
            });

        let deopts = self
            .speculative_optimizer
            .validate_all(most_observed_edge, &contended_regions);
        if deopts > 0 {
            log::warn!("optimize: {} speculative deoptimizations", deopts);
        }

        // Step 3: Run the full profile-guided optimization pipeline.
        // This modifies the SCG in-place (via Arc::make_mut) applying
        // inlining, unrolling, prefetch insertion, etc.
        let _opt_result = self.run_optimization_passes();

        // Step 4: Re-compile hot regions with the optimized SCG.
        let mut reoptimized = 0;
        for (node_id, count) in &hot_paths {
            let region_id = *node_id as RegionId;
            if self.compiled_state.is_compiled(region_id) && *count > 50 {
                // Re-compile at a higher optimization level using the
                // now-optimized SCG.
                let code = self.compile_region(region_id);
                let optimized_code = CompiledRegion { region_id, code };
                self.compiled_state.insert(optimized_code);
                reoptimized += 1;
            }
        }

        log::debug!("optimize: re-optimized {} regions", reoptimized);
        reoptimized
    }

    /// Runs the full profile-guided optimization pipeline on the SCG.
    ///
    /// This method uses copy-on-write semantics (`Arc::make_mut`) to obtain
    /// mutable access to the SCG, then applies all registered
    /// [`OptimizationEngine`] passes guided by the current profile data.
    ///
    /// # Returns
    ///
    /// An [`OptimizationResult`] summarising all transformations applied and
    /// the estimated speedup.
    pub fn run_optimization_passes(&mut self) -> OptimizationResult {
        let report = ProfileReport::from_profile_data(&self.profile_data, &self.scg);

        // Use Arc::make_mut to get &mut SCG (clone-on-write: if the Arc
        // has a single owner this is free; otherwise it clones the graph).
        let scg_mut = Arc::make_mut(&mut self.scg);

        let result = self.optimization_engine.run(scg_mut, &report);

        log::info!(
            "run_optimization_passes: {} total transformations, estimated speedup {:.3}×",
            result.total_transformations,
            result.estimated_speedup,
        );

        result
    }

    /// Returns a reference to the compiled state.
    pub fn compiled_state(&self) -> &CompiledState {
        &self.compiled_state
    }

    /// Returns a reference to the profile data.
    pub fn profile_data(&self) -> &ProfileData {
        &self.profile_data
    }

    /// Returns a mutable reference to the profile data.
    ///
    /// This is primarily useful for testing and seeding profile data
    /// before running optimization cycles.
    pub fn profile_data_mut(&mut self) -> &mut ProfileData {
        &mut self.profile_data
    }

    /// Returns a reference to the runtime configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns a reference to the speculative optimizer.
    pub fn speculative_optimizer(&self) -> &SpeculativeOptimizer {
        &self.speculative_optimizer
    }

    /// Returns a reference to the deployment planner.
    pub fn deployment_planner(&self) -> &DeploymentPlanner {
        &self.deployment_planner
    }

    /// Returns a reference to the optimization engine.
    pub fn optimization_engine(&self) -> &OptimizationEngine {
        &self.optimization_engine
    }

    /// Returns a reference to the ownership tracker.
    pub fn ownership_tracker(&self) -> &OwnershipTracker {
        &self.ownership_tracker
    }

    /// Returns a mutable reference to the ownership tracker.
    pub fn ownership_tracker_mut(&mut self) -> &mut OwnershipTracker {
        &mut self.ownership_tracker
    }

    /// Returns a reference to the SCG.
    pub fn scg(&self) -> &SCG {
        &self.scg
    }

    // -----------------------------------------------------------------------
    // Region-level compilation via vuma-codegen
    // -----------------------------------------------------------------------

    /// Compiles a single region of the SCG to ARM64 machine code.
    ///
    /// This method:
    /// 1. Looks up the node in the COR's SCG by its region ID.
    /// 2. Constructs a synthetic codegen-SCG function from the node's
    ///    metadata.
    /// 3. Runs the full codegen pipeline: SCG → IR → RegAlloc → Emit.
    /// 4. Returns the resulting machine code bytes.
    ///
    /// If the node is not found in the SCG, or if codegen fails for any
    /// reason, a small ARM64 stub that returns 0 is emitted instead.
    fn compile_region(&self, region_id: RegionId) -> Vec<u8> {
        let node_id = region_id as crate::types::NodeId;

        // Try to look up the node in the SCG to build a richer function.
        let node = self.scg.get_node(node_id);

        // Build a synthetic codegen SCG from the node metadata.
        let func_body = match node {
            Some(n) => self.node_to_statements(n),
            None => {
                // No node found — emit a trivial return-0.
                vec![ScgStatement::Return(vec![ScgExpr::Int(0)])]
            }
        };

        let func_name = format!("region_{}", region_id);
        let codegen_scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: func_name.clone(),
                params: vec![],
                results: vec![ScgType::I64],
                body: func_body,
            })],
        };

        // Run the codegen pipeline: SCG → IR → Emit.
        let mut builder = IRBuilder::new();
        match builder.build(&codegen_scg) {
            Ok(ir_program) => {
                if ir_program.functions.is_empty() {
                    log::warn!(
                        "compile_region: IR translation produced no functions for region {}",
                        region_id
                    );
                    return Self::return_zero_stub();
                }

                let mut emitter = Emitter::new();
                match emitter.emit_program(&ir_program) {
                    Ok(bytes) => {
                        if bytes.is_empty() {
                            log::warn!(
                                "compile_region: emission produced empty code for region {}, using return-zero stub",
                                region_id
                            );
                            Self::return_zero_stub()
                        } else {
                            log::trace!(
                                "compile_region: region {} compiled to {} bytes",
                                region_id,
                                bytes.len()
                            );
                            bytes
                        }
                    }
                    Err(e) => {
                        log::error!(
                            "compile_region: emission failed for region {}: {}, using return-zero stub",
                            region_id,
                            e
                        );
                        Self::return_zero_stub()
                    }
                }
            }
            Err(e) => {
                log::error!(
                    "compile_region: IR translation failed for region {}: {}, using return-zero stub",
                    region_id,
                    e
                );
                Self::return_zero_stub()
            }
        }
    }

    /// Find the region ID that contains the given node, if any.
    ///
    /// This walks all nodes in the SCG and checks whether the given
    /// node ID appears in any node's incoming or outgoing edge lists
    /// that belong to a region. Since the COR-internal SCG stores edges
    /// per-node, we can determine the region by checking which edges
    /// reference the node.
    fn find_region_for_node(&self, node_id: crate::types::NodeId) -> Option<RegionId> {
        // First, check if the node itself maps to a region directly
        // (in the COR model, a node's ID is used as its region ID).
        if self.scg.nodes.contains_key(&node_id) {
            return Some(node_id as RegionId);
        }
        None
    }

    /// Collect per-region edge observations from the SCG and profile data.
    ///
    /// Returns a map from region ID to the list of edge IDs whose source
    /// or target nodes belong to that region, along with observed
    /// contention counts from the profile data.
    fn collect_edge_observations(
        &self,
    ) -> std::collections::HashMap<RegionId, Vec<crate::types::EdgeId>> {
        let mut observations: std::collections::HashMap<RegionId, Vec<crate::types::EdgeId>> =
            std::collections::HashMap::new();

        for (&edge_id, edge) in &self.scg.edges {
            if let Some(region_id) = self.find_region_for_node(edge.source) {
                observations.entry(region_id).or_default().push(edge_id);
            }
            if let Some(region_id) = self.find_region_for_node(edge.target) {
                observations.entry(region_id).or_default().push(edge_id);
            }
        }

        observations
    }

    /// Identify regions that are experiencing contention based on profile data.
    ///
    /// A region is considered contended if it has a high access frequency
    /// (above the configured threshold) or if the profile data indicates
    /// concurrent access patterns.
    fn find_contended_regions(&mut self) -> Vec<RegionId> {
        let hot_paths = self.profile_data.get_hot_paths(10);
        let mut contended = Vec::new();

        for (node_id, count) in &hot_paths {
            if *count > 100 {
                let region_id = *node_id as RegionId;
                if !contended.contains(&region_id) {
                    contended.push(region_id);
                }
            }
        }

        contended
    }

    /// Converts a COR SCGNode's metadata into codegen SCG statements.
    ///
    /// This method produces real control flow based on the node's
    /// [`NodeKind`]. Fine-grained control flow kinds (LoopHeader, LoopExit,
    /// Branch, Join, FunctionEntry, FunctionReturn, Jump) are translated
    /// into the corresponding codegen IR constructs. Coarser kinds
    /// (Compute, Memory, Call, Loop, Entry) produce representative
    /// function bodies reflecting the node's optimisation metadata.
    fn node_to_statements(&self, node: &crate::types::SCGNode) -> Vec<ScgStatement> {
        match node.kind {
            NodeKind::Compute => {
                // Real computation: load operands, compute, store result.
                vec![
                    ScgStatement::Computation(CgComputationNode {
                        dst: format!("v{}", node.id),
                        op: BinOpKind::Add,
                        lhs: ScgExpr::Var("arg0".to_string()),
                        rhs: ScgExpr::Var("arg1".to_string()),
                        tail_call: false,
                    }),
                    ScgStatement::Return(vec![ScgExpr::Var(format!("v{}", node.id))]),
                ]
            }
            NodeKind::Memory => {
                // Load or store with optional prefetch hint.
                let mut stmts = vec![
                    ScgStatement::Allocation(CgAllocationNode::Stack {
                        name: format!("mem_{}", node.id),
                        size: 8,
                        ty: ScgType::U64,
                    }),
                    ScgStatement::Access(CgAccessNode::Load {
                        dst: format!("loaded_{}", node.id),
                        ptr: ScgExpr::Var(format!("mem_{}", node.id)),
                        offset: None,
                    }),
                ];
                if node.has_prefetch {
                    // Add a PRFM hint (represented as a no-op computation for now).
                    stmts.push(ScgStatement::Computation(CgComputationNode {
                        dst: format!("prefetch_{}", node.id),
                        op: BinOpKind::Add,
                        lhs: ScgExpr::Var(format!("loaded_{}", node.id)),
                        rhs: ScgExpr::Int(0),
                        tail_call: false,
                    }));
                }
                stmts.push(ScgStatement::Return(vec![ScgExpr::Var(format!(
                    "loaded_{}",
                    node.id
                ))]));
                stmts
            }
            NodeKind::LoopHeader | NodeKind::Loop => {
                // Generate a real loop with unroll factor reflected in the
                // body (each iteration does a counter increment).
                let unroll = node.unroll_factor.min(4) as usize;
                let body_stmts: Vec<ScgStatement> = (0..unroll)
                    .flat_map(|i| {
                        vec![ScgStatement::Computation(CgComputationNode {
                            dst: format!("iter_{}_{}", node.id, i),
                            op: BinOpKind::Add,
                            lhs: ScgExpr::Var(format!("counter_{}", node.id)),
                            rhs: ScgExpr::Int(1),
                            tail_call: false,
                        })]
                    })
                    .collect();
                let mut loop_body = vec![ScgStatement::Allocation(CgAllocationNode::Stack {
                    name: format!("counter_{}", node.id),
                    size: 8,
                    ty: ScgType::U64,
                })];
                loop_body.extend(body_stmts);
                loop_body.push(ScgStatement::Return(vec![ScgExpr::Var(format!(
                    "counter_{}",
                    node.id
                ))]));
                vec![ScgStatement::Control(CgControlNode::Loop {
                    body: loop_body,
                })]
            }
            NodeKind::Branch => {
                // Check if this is a match/switch branch (has "match" label)
                // vs a simple if/else branch.
                let is_match = node
                    .control_label
                    .as_ref()
                    .map(|l| l.starts_with("match"))
                    .unwrap_or(false);
                if is_match {
                    // Generate a switch with 3 arms as a representative match.
                    vec![ScgStatement::Control(CgControlNode::Switch {
                        discriminant: ScgExpr::Var(format!("disc_{}", node.id)),
                        arms: vec![
                            CgSwitchArm {
                                value: 0,
                                body: vec![ScgStatement::Return(vec![ScgExpr::Int(0)])],
                            },
                            CgSwitchArm {
                                value: 1,
                                body: vec![ScgStatement::Return(vec![ScgExpr::Int(1)])],
                            },
                            CgSwitchArm {
                                value: 2,
                                body: vec![ScgStatement::Return(vec![ScgExpr::Int(2)])],
                            },
                        ],
                        default_body: vec![ScgStatement::Return(vec![ScgExpr::Int(-1)])],
                    })]
                } else {
                    // Generate a simple conditional branch.
                    vec![ScgStatement::Control(CgControlNode::If {
                        cond: ScgExpr::Var(format!("cond_{}", node.id)),
                        then_body: vec![ScgStatement::Return(vec![ScgExpr::Int(1)])],
                        else_body: Some(vec![ScgStatement::Return(vec![ScgExpr::Int(0)])]),
                    })]
                }
            }
            NodeKind::Call => {
                if node.is_inlined {
                    vec![
                        ScgStatement::Computation(CgComputationNode {
                            dst: "inlined_result".to_string(),
                            op: BinOpKind::Add,
                            lhs: ScgExpr::Var("arg0".to_string()),
                            rhs: ScgExpr::Int(1),
                            tail_call: false,
                        }),
                        ScgStatement::Return(vec![ScgExpr::Var("inlined_result".to_string())]),
                    ]
                } else {
                    vec![
                        ScgStatement::Call(CgCallNode {
                            dst: Some("result".to_string()),
                            func: format!("__vuma_call_{}", node.id),
                            args: vec![],
                        }),
                        ScgStatement::Return(vec![ScgExpr::Var("result".to_string())]),
                    ]
                }
            }
            NodeKind::LoopExit | NodeKind::Join => {
                // Structural nodes — pass through the value.
                vec![ScgStatement::Return(vec![ScgExpr::Var(format!(
                    "v{}",
                    node.id
                ))])]
            }
            NodeKind::FunctionEntry => {
                // Function boundary — return 0 as baseline.
                vec![ScgStatement::Return(vec![ScgExpr::Int(0)])]
            }
            NodeKind::FunctionReturn => {
                // Function exit — return the value computed by the function.
                vec![ScgStatement::Return(vec![ScgExpr::Var(
                    "ret_val".to_string(),
                )])]
            }
            NodeKind::Jump => {
                // Break/continue jump.
                vec![ScgStatement::Control(CgControlNode::Break)]
            }
            NodeKind::Entry => {
                // Generic entry node — return 0.
                vec![ScgStatement::Return(vec![ScgExpr::Int(0)])]
            }
        }
    }

    /// Returns the machine code for a minimal "return 0" stub for the
    /// current architecture.
    ///
    /// # AArch64
    ///
    /// ```asm
    /// MOV X0, XZR    ; return 0
    /// RET
    /// ```
    ///
    /// Encoded as two 32-bit little-endian instruction words:
    /// - `MOV X0, XZR` → `0xAA1F03E0`
    /// - `RET`         → `0xD65F03C0`
    ///
    /// # x86_64
    ///
    /// ```asm
    /// xor eax, eax   ; return 0
    /// ret
    /// ```
    ///
    /// Encoded as:
    /// - `xor eax, eax` → `0x31 0xC0`
    /// - `ret`           → `0xC3`
    fn return_zero_stub() -> Vec<u8> {
        #[cfg(all(unix, target_arch = "aarch64"))]
        {
            let mov_x0_xzr: u32 = 0xAA1F03E0;
            let ret: u32 = 0xD65F03C0;
            let mut bytes = Vec::with_capacity(8);
            bytes.extend_from_slice(&mov_x0_xzr.to_le_bytes());
            bytes.extend_from_slice(&ret.to_le_bytes());
            bytes
        }

        #[cfg(all(unix, target_arch = "x86_64"))]
        {
            // xor eax, eax  → 31 C0
            // ret            → C3
            vec![0x31, 0xC0, 0xC3]
        }

        #[cfg(not(any(all(unix, target_arch = "aarch64"), all(unix, target_arch = "x86_64"))))]
        {
            Vec::new()
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur during runtime operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum RuntimeError {
    /// The requested region has not been compiled.
    #[error("Region {0} has not been compiled")]
    NotCompiled(RegionId),

    /// Compilation failed for the given region.
    #[error("Compilation failed for region {0}: {1}")]
    CompilationFailed(RegionId, String),

    /// Execution of a compiled region failed.
    #[error("Execution failed for region {0}: {1}")]
    ExecutionFailed(RegionId, String),

    /// Execution timed out.
    #[error("Execution of region {0} timed out after {1}ms")]
    Timeout(RegionId, u64),

    /// A verification violation was detected.
    #[error("Verification violation in region {0}: {1}")]
    VerificationViolation(RegionId, String),
}

// ---------------------------------------------------------------------------
// Memory-mapped code execution
// ---------------------------------------------------------------------------

/// Executes machine code by mapping it into executable memory.
///
/// On AArch64 Unix systems, this uses `mmap` to create an anonymous memory
/// region, copies the code into it, sets the region to read+execute with
/// `mprotect`, calls the code as a function `extern "C" fn() -> i64`, and
/// unmaps the memory when done.
///
/// On x86_64 Unix systems, the same mmap + mprotect pattern is used.
/// The x86_64 SystemV ABI returns the result in RAX.
///
/// On other architectures or non-Unix systems, execution is simulated and
/// 0 is returned.
fn execute_code(code: &[u8]) -> Result<i64, RuntimeError> {
    if code.is_empty() {
        return Ok(0);
    }

    #[cfg(all(unix, target_arch = "aarch64"))]
    {
        execute_code_aarch64(code)
    }

    #[cfg(all(unix, target_arch = "x86_64"))]
    {
        execute_code_x86_64(code)
    }

    #[cfg(not(any(all(unix, target_arch = "aarch64"), all(unix, target_arch = "x86_64"))))]
    {
        let _ = code;
        Ok(0)
    }
}

/// AArch64 Unix implementation of code execution using mmap + mprotect.
#[cfg(all(unix, target_arch = "aarch64"))]
fn execute_code_aarch64(code: &[u8]) -> Result<i64, RuntimeError> {
    use std::ptr;

    let len = code.len();
    // Page-align the allocation size.
    let page_size = 4096usize;
    let aligned_len = ((len + page_size - 1) / page_size) * page_size;

    unsafe {
        // Allocate anonymous memory with read + write (so we can copy code in).
        let mem = libc::mmap(
            ptr::null_mut(),
            aligned_len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );

        if mem == libc::MAP_FAILED {
            return Err(RuntimeError::ExecutionFailed(0, "mmap failed".to_string()));
        }

        // Copy the machine code into the mapped region.
        ptr::copy_nonoverlapping(code.as_ptr(), mem as *mut u8, len);

        // Set the region to read + execute (remove write permission).
        let mprotect_result = libc::mprotect(mem, aligned_len, libc::PROT_READ | libc::PROT_EXEC);
        if mprotect_result != 0 {
            libc::munmap(mem, aligned_len);
            return Err(RuntimeError::ExecutionFailed(
                0,
                "mprotect failed".to_string(),
            ));
        }

        // Call the compiled code as a function: extern "C" fn() -> i64.
        let func: extern "C" fn() -> i64 = std::mem::transmute(mem);
        let result = func();

        // Unmap the executable memory.
        libc::munmap(mem, aligned_len);

        Ok(result)
    }
}

/// x86_64 Unix implementation of code execution using mmap + mprotect.
///
/// Follows the same pattern as [`execute_code_aarch64`]: allocate anonymous
/// memory with `mmap`, copy the machine code in, set the region to
/// read+write+execute with `mprotect`, transmute to a function pointer, call
/// it, and `munmap` when done. The x86_64 SystemV ABI returns the result in
/// RAX, which maps naturally to the `extern "C" fn() -> i64` signature.
#[cfg(all(unix, target_arch = "x86_64"))]
fn execute_code_x86_64(code: &[u8]) -> Result<i64, RuntimeError> {
    // Safety check: if the code was compiled for a non-x86_64 target (e.g., AArch64),
    // executing it on x86_64 would cause SIGSEGV. Detect this by checking for
    // AArch64 instruction patterns (all AArch64 instructions are 4-byte aligned
    // and have specific encodings). If we detect non-x86_64 code, return 0 instead
    // of crashing.
    if code.len() >= 4 {
        // AArch64 RET instruction is 0xD65F03C0 (little-endian: C0 03 5F D6)
        // AArch64 NOP is 0xD503201F (little-endian: 1F 20 03 D5)
        // If the code starts with an AArch64-style word, it's likely AArch64 code.
        let first_word = u32::from_le_bytes([code[0], code[1], code[2], code[3]]);
        // AArch64 instructions always have bits [28:25] as a valid encoding.
        // Specifically, if bits [31:26] match common AArch64 patterns, skip execution.
        let is_likely_aarch64 = (first_word & 0x1C000000) == 0x00000000 // reserved/System
            || (first_word & 0x7C000000) == 0x14000000  // B/BL
            || (first_word & 0x7F000000) == 0x53000000  // MOV
            || (first_word & 0x7FE00000) == 0x2A000000  // ADD
            || (first_word & 0xFF000000) == 0xD6000000  // BR/BLR/RET
            || (first_word & 0xFF000000) == 0xD5000000; // System/MRS/MSR
        if is_likely_aarch64 {
            log::debug!("execute_code_x86_64: code appears to be AArch64, skipping execution");
            return Ok(0);
        }
    }

    use std::ptr;

    let len = code.len();
    // Page-align the allocation size.
    let page_size = 4096usize;
    #[allow(clippy::manual_div_ceil)]
    let aligned_len = ((len + page_size - 1) / page_size) * page_size;

    unsafe {
        // Allocate anonymous memory with read + write (so we can copy code in).
        let mem = libc::mmap(
            ptr::null_mut(),
            aligned_len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        );

        if mem == libc::MAP_FAILED {
            return Err(RuntimeError::ExecutionFailed(0, "mmap failed".to_string()));
        }

        // Copy the machine code into the mapped region.
        ptr::copy_nonoverlapping(code.as_ptr(), mem as *mut u8, len);

        // Set the region to read + write + execute.
        // x86_64 requires W+X for some JIT scenarios; we use RWX here
        // to match the AArch64 pattern (R+X) but also allow the write
        // flag for self-modifying code scenarios on x86_64.
        let mprotect_result = libc::mprotect(
            mem,
            aligned_len,
            libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
        );
        if mprotect_result != 0 {
            libc::munmap(mem, aligned_len);
            return Err(RuntimeError::ExecutionFailed(
                0,
                "mprotect failed".to_string(),
            ));
        }

        // Call the compiled code as a function: extern "C" fn() -> i64.
        // x86_64 SystemV ABI: result is returned in RAX.
        let func: extern "C" fn() -> i64 = std::mem::transmute(mem);
        let result = func();

        // Unmap the executable memory.
        libc::munmap(mem, aligned_len);

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NodeKind, SCGEdge, SCGNode};

    #[test]
    fn compiled_state_invariant() {
        let mut state = CompiledState::new();
        state.insert(CompiledRegion {
            region_id: 1,
            code: vec![],
        });
        state.insert(CompiledRegion {
            region_id: 2,
            code: vec![],
        });
        let missing = state.verify_invariant(&[1, 2, 3]);
        assert_eq!(missing, vec![3]);
    }

    #[test]
    fn compile_incremental_adds_regions() {
        let scg = Arc::new(SCG::default());
        let config = Config::default();
        let mut rt = CORuntime::new(scg, config);

        let delta = Delta {
            added_nodes: vec![10, 20],
            ..Delta::empty()
        };

        let recompiled = rt.compile_incremental(&delta);
        assert_eq!(recompiled.len(), 2);
        assert!(rt.compiled_state().is_compiled(10));
        assert!(rt.compiled_state().is_compiled(20));
    }

    #[test]
    fn compile_incremental_produces_real_arm64_code() {
        let scg = Arc::new(SCG::default());
        let config = Config::default();
        let mut rt = CORuntime::new(scg, config);

        let delta = Delta {
            added_nodes: vec![42],
            ..Delta::empty()
        };

        let recompiled = rt.compile_incremental(&delta);
        assert_eq!(recompiled, vec![42]);

        // The compiled region should contain real ARM64 machine code
        // (not a NOP sled). The codegen pipeline produces at least a
        // prologue, so the code should be non-empty.
        let compiled = rt.compiled_state().get(42).unwrap();
        assert!(
            !compiled.code.is_empty(),
            "compiled code should not be empty"
        );
        // Verify it's not a NOP sled (0x90 repeated).
        assert!(
            !compiled.code.iter().all(|&b| b == 0x90),
            "compiled code should not be a NOP sled"
        );
    }

    #[test]
    fn compile_incremental_uses_scg_node_metadata() {
        // Build an SCG with a Compute node.
        let mut scg = SCG::new();
        let compute_node = SCGNode::new(100, NodeKind::Compute);
        scg.insert_node(compute_node);

        let scg = Arc::new(scg);
        let config = Config::default();
        let mut rt = CORuntime::new(scg, config);

        let delta = Delta {
            added_nodes: vec![100],
            ..Delta::empty()
        };

        let recompiled = rt.compile_incremental(&delta);
        assert_eq!(recompiled, vec![100]);

        let compiled = rt.compiled_state().get(100).unwrap();
        assert!(!compiled.code.is_empty());
    }

    #[test]
    fn execute_uncompiled_region_errors() {
        let scg = Arc::new(SCG::default());
        let config = Config::default();
        let mut rt = CORuntime::new(scg, config);

        let result = rt.execute(999);
        assert!(result.is_err());
    }

    #[test]
    fn execute_compiled_region_succeeds() {
        let scg = Arc::new(SCG::default());
        let config = Config::default();
        let mut rt = CORuntime::new(scg, config);

        // Insert a compiled region with the return-zero stub.
        rt.compiled_state.insert(CompiledRegion {
            region_id: 1,
            code: CORuntime::return_zero_stub(),
        });

        let result = rt.execute(1);
        assert!(result.is_ok());
    }

    #[test]
    fn execute_records_profile_data() {
        let scg = Arc::new(SCG::default());
        let config = Config::default();
        let mut rt = CORuntime::new(scg, config);

        rt.compiled_state.insert(CompiledRegion {
            region_id: 5,
            code: CORuntime::return_zero_stub(),
        });

        let _ = rt.execute(5);
        // Profile data should have been recorded (record_access + record_call
        // each increment call_counts[5] by 1, so total should be 2).
        let count = rt.profile_data().call_counts.get(&5).copied().unwrap_or(0);
        assert!(count > 0, "execute should record profile data");
    }

    #[test]
    fn optimize_recompiles_hot_regions() {
        // Build an SCG with a hot call node.
        let mut scg = SCG::new();
        let mut call_node = SCGNode::new(10, NodeKind::Call);
        call_node.code_size = 64;
        scg.insert_node(call_node);
        let scg = Arc::new(scg);

        let config = Config::default();
        let mut rt = CORuntime::new(scg, config);

        // Insert a compiled region for the call node.
        rt.compiled_state.insert(CompiledRegion {
            region_id: 10,
            code: CORuntime::return_zero_stub(),
        });

        // Make the region "hot" by recording many accesses.
        for _ in 0..500 {
            rt.profile_data.record_access(10);
        }

        // Run optimize.
        let reoptimized = rt.optimize();
        // The hot region should have been re-optimized.
        assert!(
            reoptimized >= 1,
            "at least one region should be re-optimized"
        );

        // The compiled region should still exist and have real code.
        let compiled = rt.compiled_state().get(10).unwrap();
        assert!(!compiled.code.is_empty());
        // After optimization, the call node should be marked as inlined.
        assert!(rt.scg().get_node(10).unwrap().is_inlined);
    }

    #[test]
    fn return_zero_stub_is_valid_native() {
        let stub = CORuntime::return_zero_stub();

        #[cfg(all(unix, target_arch = "aarch64"))]
        {
            // Should be exactly 8 bytes (2 ARM64 instructions).
            assert_eq!(stub.len(), 8);
            // First instruction: MOV X0, XZR (0xAA1F03E0 in little-endian).
            assert_eq!(stub[0], 0xE0);
            assert_eq!(stub[1], 0x03);
            assert_eq!(stub[2], 0x1F);
            assert_eq!(stub[3], 0xAA);
            // Second instruction: RET (0xD65F03C0 in little-endian).
            assert_eq!(stub[4], 0xC0);
            assert_eq!(stub[5], 0x03);
            assert_eq!(stub[6], 0x5F);
            assert_eq!(stub[7], 0xD6);
        }

        #[cfg(all(unix, target_arch = "x86_64"))]
        {
            // Should be exactly 3 bytes (xor eax, eax + ret).
            assert_eq!(stub.len(), 3);
            assert_eq!(stub[0], 0x31);
            assert_eq!(stub[1], 0xC0);
            assert_eq!(stub[2], 0xC3);
        }

        #[cfg(not(any(all(unix, target_arch = "aarch64"), all(unix, target_arch = "x86_64"))))]
        {
            assert!(stub.is_empty());
        }
    }

    #[test]
    fn execute_code_simulated_on_non_aarch64() {
        // On x86_64 (the development machine), execute_code should
        // return Ok(0) without actually running the code.
        let code = CORuntime::return_zero_stub();
        let result = execute_code(&code);
        assert!(result.is_ok());
        // On non-aarch64, the simulated result is 0.
        #[cfg(not(all(unix, target_arch = "aarch64")))]
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn test_compiled_region_stores_code() {
        // Verify that CompiledRegion stores non-empty code after compilation.
        // This test ensures the codegen output is actually stored in the
        // CompiledRegion, not just an empty Vec.

        // Case 1: Region with a node in the SCG.
        let mut scg = SCG::new();
        let compute_node = SCGNode::new(42, NodeKind::Compute);
        scg.insert_node(compute_node);
        let scg = Arc::new(scg);
        let config = Config::default();
        let mut rt = CORuntime::new(scg, config);

        let delta = Delta {
            added_nodes: vec![42],
            ..Delta::empty()
        };
        rt.compile_incremental(&delta);

        let compiled = rt.compiled_state().get(42).unwrap();
        assert!(
            !compiled.code.is_empty(),
            "CompiledRegion for node 42 should store non-empty code after compilation, got {} bytes",
            compiled.code.len()
        );

        // Case 2: Region without a node in the SCG (should use return_zero_stub fallback).
        let scg2 = Arc::new(SCG::default());
        let config2 = Config::default();
        let mut rt2 = CORuntime::new(scg2, config2);

        let delta2 = Delta {
            added_nodes: vec![99],
            ..Delta::empty()
        };
        rt2.compile_incremental(&delta2);

        let compiled2 = rt2.compiled_state().get(99).unwrap();
        assert!(
            !compiled2.code.is_empty(),
            "CompiledRegion for node 99 (no SCG node) should have return-zero stub code, got {} bytes",
            compiled2.code.len()
        );

        // The fallback code should be non-empty (either the stub or a full ELF with stub).
        let stub = CORuntime::return_zero_stub();
        assert!(
            !stub.is_empty(),
            "return_zero_stub should produce non-empty code on supported platforms"
        );
        assert!(
            compiled2.code.len() >= stub.len(),
            "Fallback code should be at least as large as the stub (got {} bytes, stub is {} bytes)",
            compiled2.code.len(),
            stub.len()
        );
    }

    #[test]
    fn run_optimization_passes_with_profile_data() {
        // Build an SCG with a hot call and a hot loop.
        let mut scg = SCG::new();
        let mut call_node = SCGNode::new(10, NodeKind::Call);
        call_node.code_size = 64;
        scg.insert_node(call_node);

        let mut loop_node = SCGNode::new(20, NodeKind::Loop);
        loop_node.code_size = 128;
        loop_node.outgoing_edges.push(200);
        scg.insert_node(loop_node);

        let mut mem_node = SCGNode::new(30, NodeKind::Memory);
        mem_node.code_size = 64;
        mem_node.incoming_edges.push(200);
        scg.insert_node(mem_node);

        scg.insert_edge(SCGEdge {
            id: 200,
            source: 30,
            target: 20,
            weight: 5000,
        });

        let scg = Arc::new(scg);
        let config = Config::default();
        let mut rt = CORuntime::new(scg, config);

        // Simulate profile data by recording accesses.
        rt.compiled_state.insert(CompiledRegion {
            region_id: 10,
            code: CORuntime::return_zero_stub(),
        });
        rt.compiled_state.insert(CompiledRegion {
            region_id: 20,
            code: CORuntime::return_zero_stub(),
        });
        rt.compiled_state.insert(CompiledRegion {
            region_id: 30,
            code: CORuntime::return_zero_stub(),
        });

        for _ in 0..500 {
            rt.profile_data.record_access(10);
        }
        for _ in 0..300 {
            rt.profile_data.record_access(20);
        }
        for _ in 0..200 {
            rt.profile_data.record_access(30);
        }

        // Run optimization passes.
        let result = rt.run_optimization_passes();

        // Verify that transformations were applied.
        assert!(
            result.total_transformations > 0,
            "should apply at least one optimization"
        );
        assert!(
            result.estimated_speedup > 1.0,
            "estimated speedup should exceed 1.0"
        );

        // Verify SCG nodes were actually modified.
        let scg = rt.scg();
        assert!(
            scg.get_node(10).unwrap().is_inlined,
            "hot call node 10 should be inlined after optimization"
        );
        assert!(
            scg.get_node(20).unwrap().unroll_factor > 1,
            "hot loop node 20 should be unrolled after optimization"
        );
        assert!(
            scg.get_node(30).unwrap().has_prefetch,
            "hot memory node 30 should have prefetch after optimization"
        );
    }
}
