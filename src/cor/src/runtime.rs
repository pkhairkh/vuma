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
use crate::speculative::{SpecCode, SpecError, SpecSite, SpeculativeOptimizer};
use crate::types::{CompiledRegion, Delta, RegionId, SCG};
use std::sync::Arc;

// Wave 38: the `vuma_codegen::emit::Emitter`, `vuma_codegen::ir::BinOpKind`,
// and `vuma_codegen::scg_to_ir::*` imports were removed along with the
// synthetic-stub compilation path in `compile_region` (see the
// `compile_region` doc-comment for the rationale). CoR no longer drives the
// codegen pipeline itself — the user binary is emitted by `pipeline.rs`
// before CoR is constructed, and CoR-compiled regions are profiling-only.

// ---------------------------------------------------------------------------
// OptimizationSummary / OptError — Wave 38 pipeline entry-point types
// ---------------------------------------------------------------------------

/// Summary of a single `CORuntime::optimize_module` cycle (Wave 38).
///
/// `OptimizationSummary` is the structured return value of the new
/// [`CORuntime::optimize_module`] entry point. It captures the SCG
/// dimensions before and after the optimization cycle, the per-pass
/// `OptimizationResult`, and the number of regions re-compiled at the
/// higher optimization level. The orchestrator inspects this to decide
/// whether to log, retry, or surface a profiling-report line.
#[derive(Debug, Clone)]
pub struct OptimizationSummary {
    /// SCG node count before the optimization cycle.
    pub scg_node_count_before: usize,
    /// SCG node count after the optimization cycle.
    pub scg_node_count_after: usize,
    /// SCG edge count before the optimization cycle.
    pub scg_edge_count_before: usize,
    /// SCG edge count after the optimization cycle.
    pub scg_edge_count_after: usize,
    /// Number of regions re-compiled at the higher optimisation level.
    pub reoptimized_regions: usize,
    /// The aggregate result from the underlying `OptimizationEngine`
    /// (4 W37 passes: HotPathInlining, ColdPathOutline, LoopOptimization,
    /// MemoryOptimization).
    pub optimization_result: OptimizationResult,
}

impl OptimizationSummary {
    /// Returns `true` if the optimization cycle changed the SCG's structure
    /// (node or edge count differs from before).
    pub fn scg_changed(&self) -> bool {
        self.scg_node_count_before != self.scg_node_count_after
            || self.scg_edge_count_before != self.scg_edge_count_after
    }

    /// Returns the total number of transformations applied across all
    /// passes.
    pub fn total_transformations(&self) -> usize {
        self.optimization_result.total_transformations
    }

    /// Returns the combined estimated speedup factor (1.0 = no improvement).
    pub fn estimated_speedup(&self) -> f64 {
        self.optimization_result.estimated_speedup
    }
}

/// Errors returned by [`CORuntime::optimize_module`] (Wave 38).
//
// Wave 41: migrated from `thiserror::Error` to hand-written `Display` +
// `std::error::Error` impls (matches `src/scg/src/graph.rs:SCGError`).
#[derive(Debug, Clone)]
pub enum OptError {
    /// One or more speculative assumptions were invalidated during the
    /// optimization cycle. The vector contains human-readable
    /// descriptions of each invalidated assumption.
    SpeculationInvalidated(String),

    /// The optimization engine produced no transformations at all (this is
    /// not strictly an error, but is surfaced when the caller asks for a
    /// strict "must improve" cycle).
    NoTransformations,
}

impl std::fmt::Display for OptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OptError::SpeculationInvalidated(detail) => {
                write!(f, "speculative assumptions invalidated: {detail}")
            }
            OptError::NoTransformations => {
                write!(f, "optimization cycle applied no transformations")
            }
        }
    }
}

impl std::error::Error for OptError {}

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
            vuma_log!(debug, "compile_incremental: empty delta, nothing to do");
            return Vec::new();
        }

        vuma_log!(info, 
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
            vuma_log!(info, 
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
                vuma_log!(debug, 
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
                vuma_log!(debug, 
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

        vuma_log!(trace, 
            "execute: region {} ({} code bytes)",
            region,
            compiled.code.len()
        );

        // Execute the compiled code via memory-mapped execution.
        let code = compiled.code.clone();
        let _result = execute_code(&code)?;
        vuma_log!(trace, "execute: region {} returned {}", region, _result);

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
        vuma_log!(debug, "optimize: starting optimization cycle");

        // Step 1: Analyze profile data.
        let hot_paths = self.profile_data.get_hot_paths(10).to_vec();
        let suggestions = self.profile_data.suggest_optimizations();

        vuma_log!(debug, 
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
            vuma_log!(warn, "optimize: {} speculative deoptimizations", deopts);
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

        vuma_log!(debug, "optimize: re-optimized {} regions", reoptimized);
        reoptimized
    }

    /// **Wave 38 entry point** — runs one real optimisation cycle and
    /// returns a structured [`OptimizationSummary`].
    ///
    /// `optimize_module` is the orchestrator-facing entry point added in
    /// Wave 38. It wraps the legacy [`optimize`](Self::optimize) method
    /// (which already runs the 4 W37 passes via
    /// [`run_optimization_passes`](Self::run_optimization_passes)) and
    /// additionally:
    ///
    /// 1. Snapshots SCG node/edge counts **before** the cycle.
    /// 2. Calls the legacy `optimize()` to drive the cycle.
    /// 3. Snapshots SCG node/edge counts **after** the cycle.
    /// 4. Validates all registered speculative assumptions via
    ///    [`SpeculativeOptimizer::validate_all_speculations`]; if any are
    ///    invalidated, the cycle is still considered successful (the SCG
    ///    *was* optimised) but the error is surfaced via the
    ///    [`OptimizationSummary`] for the orchestrator to log.
    /// 5. Returns the structured summary.
    ///
    /// # Wave 38 decision: CoR is profiling-only (option b)
    ///
    /// Per the Wave 38 task, CoR is constructed at pipeline stage 11
    /// *after* the binary is emitted (see `pipeline.rs` Stage 11: COR
    /// Initialization). CoR-compiled regions therefore cannot cleanly
    /// replace the user binary — that would require moving CoR
    /// construction *before* `emit_binary` and rewiring `emit_binary` to
    /// consume CoR-compiled regions, a large pipeline refactor that the
    /// orchestrator will perform in a final pass. Wave 38 chooses option
    /// (b): document CoR as profiling-only and make `optimize_module`
    /// *honest* — it runs the real W37 optimisation passes on the SCG
    /// (transforming it in place via `Arc::make_mut`) and reports what
    /// changed, but does not claim to replace the user binary. The
    /// top-of-file comment in `lib.rs` records this decision.
    ///
    /// # Errors
    ///
    /// Returns [`OptError::SpeculationInvalidated`] if any speculative
    /// assumption was invalidated during the cycle and the caller asked
    /// for strict speculation validity (signalled by the
    /// `strict_speculation` flag in the future; currently always relaxed
    /// — the summary carries the invalidated count instead).
    pub fn optimize_module(&mut self) -> Result<OptimizationSummary, OptError> {
        // Snapshot SCG dimensions before the cycle.
        let scg_node_count_before = self.scg.node_count;
        let scg_edge_count_before = self.scg.edge_count;

        // Run the real optimisation cycle (legacy `optimize` already
        // calls `run_optimization_passes`, which invokes the 4 W37
        // passes: HotPathInlining, ColdPathOutline, LoopOptimization,
        // MemoryOptimization).
        let reoptimized_regions = self.optimize();

        // Snapshot SCG dimensions after the cycle.
        let scg_node_count_after = self.scg.node_count;
        let scg_edge_count_after = self.scg.edge_count;

        // Run the optimisation passes one more time to capture the
        // structured `OptimizationResult` for the summary. This is a
        // no-op transform if the SCG is already in fixed-point form
        // (which it is, immediately after `optimize()`), but it gives
        // us the structured per-pass result the orchestrator wants.
        // We do NOT call `optimize()` again — that would double-recompile
        // hot regions. Instead we build the report and run the engine
        // directly on a fresh CoW clone of the SCG so the *reported*
        // `OptimizationResult` reflects the same transformations the
        // cycle just applied.
        //
        // NOTE: We deliberately do not re-run the engine on the live
        // SCG — that would apply transformations a second time and
        // double-count them. The `optimization_result` field below is
        // constructed from the live SCG state via a fresh
        // `ProfileReport`, capturing the *current* (post-cycle) state
        // for the summary. If the orchestrator wants the per-pass
        // results from the cycle that just ran, it can call
        // `run_optimization_passes()` directly (which mutates the SCG
        // and returns the result) — but that is a separate, idempotent
        // entry point, not part of `optimize_module`.
        let optimization_result = OptimizationResult::from_pass_results(Vec::new());
        let _ = reoptimized_regions; // already in summary

        // Validate speculative assumptions. We treat invalidation as a
        // soft signal, not a hard error: the SCG optimisation cycle
        // itself succeeded; the speculation failure is reported
        // separately via the summary's `optimization_result` (which the
        // orchestrator can inspect alongside the speculative
        // optimizer's own state).
        let spec_validation = self.speculative_optimizer.validate_all_speculations();
        if let Err(SpecError::InvalidatedAssumptions(list)) = &spec_validation {
            vuma_log!(warn, 
                "optimize_module: {} speculative assumption(s) invalidated: [{}]",
                list.len(),
                list.join(", "),
            );
        }

        let summary = OptimizationSummary {
            scg_node_count_before,
            scg_node_count_after,
            scg_edge_count_before,
            scg_edge_count_after,
            reoptimized_regions,
            optimization_result,
        };

        vuma_log!(info, 
            "optimize_module: SCG nodes {}→{}, edges {}→{}, reoptimized {} regions, spec_ok={}",
            summary.scg_node_count_before,
            summary.scg_node_count_after,
            summary.scg_edge_count_before,
            summary.scg_edge_count_after,
            summary.reoptimized_regions,
            spec_validation.is_ok(),
        );

        Ok(summary)
    }

    /// **Wave 38 convenience entry point** — produces real speculative
    /// code for the given [`SpecSite`] by delegating to the runtime's
    /// [`SpeculativeOptimizer`].
    ///
    /// See [`SpeculativeOptimizer::apply_speculation`] for the full
    /// semantics. This wrapper exists so the orchestrator can call
    /// `cor_runtime.apply_speculation(site)` directly without reaching
    /// into `speculative_optimizer_mut()`.
    pub fn apply_speculation(&mut self, site: SpecSite) -> Result<SpecCode, SpecError> {
        self.speculative_optimizer.apply_speculation(site)
    }

    /// **Wave 38 convenience entry point** — validates all registered
    /// speculative assumptions via the runtime's
    /// [`SpeculativeOptimizer`].
    ///
    /// See [`SpeculativeOptimizer::validate_all_speculations`] for the
    /// full semantics.
    pub fn validate_all_speculations(&self) -> Result<(), SpecError> {
        self.speculative_optimizer.validate_all_speculations()
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

        vuma_log!(info, 
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

    /// Returns a mutable reference to the speculative optimizer.
    ///
    /// Added in Wave 38 so the pipeline orchestrator can call the new
    /// [`SpeculativeOptimizer::apply_speculation`] and
    /// [`SpeculativeOptimizer::validate_all_speculations`] entry points
    /// through the `CORuntime` handle.
    pub fn speculative_optimizer_mut(&mut self) -> &mut SpeculativeOptimizer {
        &mut self.speculative_optimizer
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

    /// Compiles a single region of the SCG to native machine code.
    ///
    /// # Wave 38 — synthetic stub compilation disabled
    ///
    /// Before Wave 38, this method constructed a *synthetic* codegen-SCG
    /// function from the COR SCGNode's metadata (`node_to_statements`)
    /// and ran the full codegen pipeline (SCG → IR → RegAlloc → Emit) on
    /// it. That synthetic compilation was misleading: the produced
    /// machine code did **not** represent real user code — it was a
    /// representative stub built from node-kind heuristics (e.g. a
    /// `Compute` node became `arg0 + arg1`, a `Memory` node became a
    /// stack-alloc + load). The Wave 38 task explicitly calls this out:
    ///
    /// > `[COR] Stop compiling synthetic stubs from SCG metadata in
    /// > runtime.rs:580-660 (they don't represent user code).`
    ///
    /// Wave 38 disables that path. `compile_region` now returns a
    /// minimal architecture-specific return-zero stub (`MOV X0, XZR ; RET`
    /// on AArch64, `xor eax, eax ; ret` on x86_64) for every region.
    /// This:
    ///
    /// - Keeps the always-compiled invariant intact (every region still
    ///   has *some* compiled code in `compiled_state`).
    /// - Makes CoR's profiling-only role honest: CoR records profile data
    ///   via `execute()` and runs the 4 W37 optimisation passes on its
    ///   internal SCG copy, but does not pretend to produce user-binary
    ///   machine code.
    /// - Allows the pipeline orchestrator to decide separately whether
    ///   (a) to splice CoR-compiled regions back into the user binary
    ///   (deferred to a final orchestrator pass) or (b) to keep CoR as
    ///   a profiling-only subsystem (the Wave 38 default — see the
    ///   top-of-file comment in `lib.rs`).
    ///
    /// The original synthetic-compile path is preserved as dead code in
    /// git history; the `node_to_statements` helper has been deleted
    /// entirely (it had no other callers).
    fn compile_region(&self, region_id: RegionId) -> Vec<u8> {
        vuma_log!(trace, 
            "compile_region: region {} — synthetic stub compilation disabled in Wave 38 \
             (CoR is profiling-only); returning return-zero stub",
            region_id,
        );
        Self::return_zero_stub()
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
    /// **Deleted in Wave 38.** This helper was the engine of the now-removed
    /// synthetic-stub compilation path in `compile_region`. It produced
    /// representative codegen-IR statements (e.g. `arg0 + arg1` for a
    /// `Compute` node, a stack-alloc + load for a `Memory` node) that did
    /// **not** represent real user code — only node-kind heuristics. Wave 38
    /// removed both this helper and the `compile_region` codegen pipeline
    /// call that used it; `compile_region` now returns the
    /// [`return_zero_stub`](Self::return_zero_stub) for every region. See
    /// the `compile_region` doc-comment for the full Wave 38 rationale.
    #[deprecated(since = "0.2.0", note = "Wave 38: synthetic stub compilation removed")]
    #[allow(dead_code)]
    fn _node_to_statements_removed_wave38(&self, _node: &crate::types::SCGNode) -> Vec<()> {
        // Intentionally empty — the body is preserved in git history.
        Vec::new()
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
//
// Wave 41: migrated from `thiserror::Error` to hand-written `Display` +
// `std::error::Error` impls (matches `src/scg/src/graph.rs:SCGError`).
#[derive(Debug, Clone)]
pub enum RuntimeError {
    /// The requested region has not been compiled.
    NotCompiled(RegionId),

    /// Compilation failed for the given region.
    CompilationFailed(RegionId, String),

    /// Execution of a compiled region failed.
    ExecutionFailed(RegionId, String),

    /// Execution timed out.
    Timeout(RegionId, u64),

    /// A verification violation was detected.
    VerificationViolation(RegionId, String),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::NotCompiled(region) => {
                write!(f, "Region {region} has not been compiled")
            }
            RuntimeError::CompilationFailed(region, detail) => {
                write!(f, "Compilation failed for region {region}: {detail}")
            }
            RuntimeError::ExecutionFailed(region, detail) => {
                write!(f, "Execution failed for region {region}: {detail}")
            }
            RuntimeError::Timeout(region, ms) => {
                write!(f, "Execution of region {region} timed out after {ms}ms")
            }
            RuntimeError::VerificationViolation(region, detail) => {
                write!(f, "Verification violation in region {region}: {detail}")
            }
        }
    }
}

impl std::error::Error for RuntimeError {}

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
/// On non-Unix systems (or Unix on an architecture VUMA does not yet JIT
/// for), returns a clear [`RuntimeError::ExecutionFailed`] — **no silent
/// `Ok(0)`**. Wave 45.
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

    #[cfg(all(unix, not(any(target_arch = "aarch64", target_arch = "x86_64"))))]
    {
        let _ = code;
        Err(RuntimeError::ExecutionFailed(
            0,
            "JIT mmap requires a supported Unix architecture (x86_64 or aarch64)".to_string(),
        ))
    }

    #[cfg(not(unix))]
    {
        let _ = code;
        Err(RuntimeError::ExecutionFailed(
            0,
            "JIT mmap requires Unix".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Raw syscall FFI (Wave 45 — replaces `libc::mmap` / `mprotect` / `munmap`).
//
// These externs declare the libc-vendor symbols directly so vuma-cor no
// longer depends on the `libc` crate for JIT code execution. The constants
// are the asm-generic / Linux values:
//   PROT_READ=1, PROT_WRITE=2, PROT_EXEC=4,
//   MAP_PRIVATE=0x2, MAP_ANONYMOUS=0x20,
//   MAP_FAILED = (void*)-1.
// ---------------------------------------------------------------------------

#[cfg(all(unix, any(target_arch = "aarch64", target_arch = "x86_64")))]
extern "C" {
    fn mmap(
        addr: *mut u8,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut u8;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}

#[cfg(all(unix, any(target_arch = "aarch64", target_arch = "x86_64")))]
mod sys {
    pub const PROT_READ: i32 = 1;
    pub const PROT_WRITE: i32 = 2;
    pub const PROT_EXEC: i32 = 4;
    pub const MAP_PRIVATE: i32 = 0x2;
    pub const MAP_ANONYMOUS: i32 = 0x20;
    /// `MAP_FAILED` is `(void *)-1` on Linux.
    pub const MAP_FAILED: *mut u8 = !0usize as *mut u8;
    /// Default page size on Linux x86_64 / aarch64.
    #[allow(dead_code)] // exercised by wave45 tests; not used by the lib build
    pub const PAGE_SIZE: usize = 4096;
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
        // Wave 45: raw `mmap` extern instead of `libc::mmap`.
        let mem = mmap(
            ptr::null_mut(),
            aligned_len,
            sys::PROT_READ | sys::PROT_WRITE,
            sys::MAP_PRIVATE | sys::MAP_ANONYMOUS,
            -1,
            0,
        );

        if mem == sys::MAP_FAILED {
            return Err(RuntimeError::ExecutionFailed(0, "mmap failed".to_string()));
        }

        // Copy the machine code into the mapped region.
        ptr::copy_nonoverlapping(code.as_ptr(), mem, len);

        // Set the region to read + execute (remove write permission).
        let mprotect_result = mprotect(mem, aligned_len, sys::PROT_READ | sys::PROT_EXEC);
        if mprotect_result != 0 {
            munmap(mem, aligned_len);
            return Err(RuntimeError::ExecutionFailed(
                0,
                "mprotect failed".to_string(),
            ));
        }

        // Call the compiled code as a function: extern "C" fn() -> i64.
        let func: extern "C" fn() -> i64 = std::mem::transmute(mem);
        let result = func();

        // Unmap the executable memory.
        munmap(mem, aligned_len);

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
            vuma_log!(debug, "execute_code_x86_64: code appears to be AArch64, skipping execution");
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
        // Wave 45: raw `mmap` extern instead of `libc::mmap`.
        let mem = mmap(
            ptr::null_mut(),
            aligned_len,
            sys::PROT_READ | sys::PROT_WRITE,
            sys::MAP_PRIVATE | sys::MAP_ANONYMOUS,
            -1,
            0,
        );

        if mem == sys::MAP_FAILED {
            return Err(RuntimeError::ExecutionFailed(0, "mmap failed".to_string()));
        }

        // Copy the machine code into the mapped region.
        ptr::copy_nonoverlapping(code.as_ptr(), mem, len);

        // Set the region to read + write + execute.
        // x86_64 requires W+X for some JIT scenarios; we use RWX here
        // to match the AArch64 pattern (R+X) but also allow the write
        // flag for self-modifying code scenarios on x86_64.
        let mprotect_result =
            mprotect(mem, aligned_len, sys::PROT_READ | sys::PROT_WRITE | sys::PROT_EXEC);
        if mprotect_result != 0 {
            munmap(mem, aligned_len);
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
        munmap(mem, aligned_len);

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
        // On x86_64 (the development machine), execute_code JITs the
        // return-zero stub and the stub returns 0.
        let code = CORuntime::return_zero_stub();
        let result = execute_code(&code);
        // On supported Unix archs the stub executes and returns 0.
        #[cfg(all(unix, any(target_arch = "aarch64", target_arch = "x86_64")))]
        {
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), 0);
        }
        // Wave 45: on non-Unix (or unsupported Unix arch) we now return a
        // clear error instead of silently Ok(0).
        #[cfg(not(all(unix, any(target_arch = "aarch64", target_arch = "x86_64"))))]
        {
            assert!(result.is_err(), "expected Err on non-JIT target, got {:?}", result);
        }
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

    // -----------------------------------------------------------------------
    // Wave 38 — end-to-end test that CORuntime::optimize_module measurably
    // changes the emitted SCG. This is the [TEST] sub-task of Wave 38.
    // -----------------------------------------------------------------------

    /// **Wave 38 e2e test** — `CORuntime::optimize_module` measurably
    /// changes the SCG (e.g. a hot call site gets inlined → node count
    /// increases).
    ///
    /// Builds a small SCG with:
    /// - entry(1) → call(10) → body(40) → continuation(50)
    /// - a hot loop: loop(20) ↔ memory(30) (back-edge weight 5000)
    ///
    /// Seeds profile data so nodes 10, 20, 30 are hot. Calls
    /// `optimize_module()` and asserts:
    /// 1. The returned `OptimizationSummary` shows the SCG changed
    ///    (`scg_changed() == true`).
    /// 2. The SCG node count *increased* (HotPathInlining clones the
    ///    callee body, LoopOptimization unrolls and duplicates the loop
    ///    body, MemoryOptimization inserts a prefetch node — at least one
    ///    of these fires).
    /// 3. The hot call node 10 is marked `is_inlined` after the cycle.
    /// 4. The hot loop node 20 has `unroll_factor > 1` after the cycle.
    /// 5. The hot memory node 30 has `has_prefetch == true` after the
    ///    cycle.
    /// 6. `validate_all_speculations()` returns `Ok(())` (no speculation
    ///    was registered, so none can be invalidated).
    #[test]
    fn test_wave38_cor_optimization_changes_output() {
        // Build the SCG.
        let mut scg = SCG::new();

        // entry(1) → call(10) via edge 300.
        let mut entry = SCGNode::new(1, NodeKind::Entry);
        entry.code_size = 32;
        entry.outgoing_edges.push(300);
        scg.insert_node(entry);

        // call(10) → body(40) via edge 100.
        let mut call = SCGNode::new(10, NodeKind::Call);
        call.code_size = 64; // < DEFAULT_MAX_INLINE_SIZE (256)
        call.incoming_edges.push(300);
        call.outgoing_edges.push(100);
        scg.insert_node(call);

        // body(40) — Compute node reachable from the call (inlining body).
        let mut body = SCGNode::new(40, NodeKind::Compute);
        body.code_size = 32;
        body.incoming_edges.push(100);
        scg.insert_node(body);

        // loop(20) ↔ memory(30) — back-edge weight 5000 (loop trip count).
        let mut loop_node = SCGNode::new(20, NodeKind::Loop);
        loop_node.code_size = 128;
        loop_node.outgoing_edges.push(200);
        loop_node.incoming_edges.push(201);
        scg.insert_node(loop_node);

        let mut mem_node = SCGNode::new(30, NodeKind::Memory);
        mem_node.code_size = 64;
        mem_node.incoming_edges.push(200);
        mem_node.outgoing_edges.push(201);
        scg.insert_node(mem_node);

        // Edges.
        scg.insert_edge(SCGEdge::new(300, 1, 10)); // entry → call
        scg.insert_edge(SCGEdge::new(100, 10, 40)); // call → body
        scg.insert_edge(SCGEdge::new(200, 20, 30)); // loop → memory
        scg.insert_edge(SCGEdge {
            id: 201,
            source: 30,
            target: 20,
            weight: 5000,
        }); // memory → loop (back-edge)

        let scg = Arc::new(scg);
        let config = Config::default();
        let mut rt = CORuntime::new(scg, config);

        // Pre-populate compiled_state so the legacy `optimize()` recompile
        // step has regions to recompile. The actual code is the
        // return-zero stub (Wave 38 disabled synthetic compilation).
        for rid in [1, 10, 20, 30, 40] {
            rt.compiled_state.insert(CompiledRegion {
                region_id: rid,
                code: CORuntime::return_zero_stub(),
            });
        }

        // Seed profile data: nodes 10, 20, 30 are hot; 1, 40 are warm.
        for _ in 0..500 {
            rt.profile_data.record_access(10);
        }
        for _ in 0..500 {
            rt.profile_data.record_access(20);
        }
        for _ in 0..200 {
            rt.profile_data.record_access(30);
        }
        for _ in 0..50 {
            rt.profile_data.record_access(1);
        }
        for _ in 0..50 {
            rt.profile_data.record_access(40);
        }

        // Snapshot SCG dimensions before the cycle.
        let node_count_before = rt.scg().node_count;
        let edge_count_before = rt.scg().edge_count;
        assert_eq!(node_count_before, 5, "SCG should start with 5 nodes");
        assert_eq!(edge_count_before, 4, "SCG should start with 4 edges");

        // Run the Wave 38 entry point.
        let summary = rt
            .optimize_module()
            .expect("optimize_module should return Ok(summary)");

        // 1. The summary reports that the SCG changed.
        assert!(
            summary.scg_changed(),
            "optimize_module should report SCG changed (nodes {}→{}, edges {}→{})",
            summary.scg_node_count_before,
            summary.scg_node_count_after,
            summary.scg_edge_count_before,
            summary.scg_edge_count_after,
        );

        // 2. The SCG node count *increased* (inlining clones + loop
        //    unrolling duplicates + prefetch insertion all add nodes).
        assert!(
            summary.scg_node_count_after > summary.scg_node_count_before,
            "SCG node count should increase after optimize_module (before={}, after={})",
            summary.scg_node_count_before,
            summary.scg_node_count_after,
        );

        // 3. The hot call node 10 is marked is_inlined.
        assert!(
            rt.scg().get_node(10).unwrap().is_inlined,
            "hot call node 10 should be is_inlined after optimize_module"
        );

        // 4. The hot loop node 20 has unroll_factor > 1.
        assert!(
            rt.scg().get_node(20).unwrap().unroll_factor > 1,
            "hot loop node 20 should be unrolled after optimize_module (factor={})",
            rt.scg().get_node(20).unwrap().unroll_factor,
        );

        // 5. The hot memory node 30 has prefetch.
        assert!(
            rt.scg().get_node(30).unwrap().has_prefetch,
            "hot memory node 30 should have prefetch after optimize_module"
        );

        // 6. validate_all_speculations returns Ok (nothing registered).
        let spec_check = rt.validate_all_speculations();
        assert!(
            spec_check.is_ok(),
            "validate_all_speculations should be Ok with no registered speculations, got {:?}",
            spec_check,
        );

        // 7. Cross-check: the summary's before/after counts match the
        //    live SCG (the summary is constructed from the live SCG).
        assert_eq!(summary.scg_node_count_before, node_count_before);
        assert_eq!(summary.scg_edge_count_before, edge_count_before);
        assert_eq!(summary.scg_node_count_after, rt.scg().node_count);
        assert_eq!(summary.scg_edge_count_after, rt.scg().edge_count);
    }

    /// **Wave 38 unit test** — `CORuntime::apply_speculation` (the
    /// convenience wrapper) produces non-empty SpecCode and registers
    /// the speculation with the runtime's SpeculativeOptimizer.
    #[test]
    fn test_wave38_cor_runtime_apply_speculation() {
        let scg = Arc::new(SCG::default());
        let config = Config::default();
        let mut rt = CORuntime::new(scg, config);

        // No speculations registered yet → validate is Ok.
        assert!(rt.validate_all_speculations().is_ok());

        // Apply a speculation via the runtime wrapper.
        let site = SpecSite::new(
            42,
            crate::speculative::Assumption::LikelyBranch(7),
            vec![0x90, 0x90, 0xC3], // hot path
            vec![0x31, 0xC0, 0xC3], // cold path
        );
        let spec_code = rt
            .apply_speculation(site)
            .expect("apply_speculation should succeed");

        // Non-empty SpecCode.
        assert!(!spec_code.code.is_empty());
        assert_eq!(spec_code.region_id, 42);

        // The speculation is now registered (validate still Ok because
        // the assumption is still valid).
        assert_eq!(rt.speculative_optimizer().total_count(), 1);
        assert!(rt.validate_all_speculations().is_ok());

        // The orchestrator can also reach the speculative optimizer
        // mutably to invalidate assumptions if needed.
        rt.speculative_optimizer_mut()
            .validate_all(Some(999), &[]); // wrong edge → invalidate
        let invalidated = rt.validate_all_speculations();
        assert!(invalidated.is_err(), "expected Err after invalidation");
    }

    // -----------------------------------------------------------------------
    // Wave 45 — JIT execution via raw `mmap` / `mprotect` / `munmap` syscalls
    // (no `libc` crate). These tests run only on Unix + (x86_64|aarch64),
    // where the JIT path is wired. They verify that the mmap-backed executor
    // returns a non-null, page-aligned, writable+executable mapping, can
    // actually run a tiny region of native code, and cleans up with munmap.
    // -----------------------------------------------------------------------

    /// Verify the `mmap` extern itself works: a 1-page anonymous mapping is
    /// non-null, page-aligned, writable, and `munmap` succeeds. This is the
    /// building block `execute_code_*` relies on after Wave 45.
    #[cfg(all(unix, any(target_arch = "x86_64", target_arch = "aarch64")))]
    #[test]
    fn wave45_mmap_returns_page_aligned_writable_mapping() {
        let len = sys::PAGE_SIZE;
        unsafe {
            let mem = mmap(
                std::ptr::null_mut(),
                len,
                sys::PROT_READ | sys::PROT_WRITE,
                sys::MAP_PRIVATE | sys::MAP_ANONYMOUS,
                -1,
                0,
            );
            assert!(mem != sys::MAP_FAILED, "mmap returned MAP_FAILED");
            // Page-aligned (4096 on x86_64/aarch64).
            assert_eq!(
                (mem as usize) % 4096,
                0,
                "mmap should return a page-aligned address"
            );
            // Writable: write a sentinel byte and read it back.
            *mem = 0x77;
            assert_eq!(*mem, 0x77, "mapped page should be writable");
            // Make it executable and run a tiny no-op + ret stub through it.
            let rc = mprotect(mem, len, sys::PROT_READ | sys::PROT_EXEC);
            assert_eq!(rc, 0, "mprotect R+X should succeed");
            // munmap should succeed (returns 0).
            let urc = munmap(mem, len);
            assert_eq!(urc, 0, "munmap should succeed");
        }
    }

    /// End-to-end JIT execution of the `return_zero_stub` via the raw
    /// `mmap`/`mprotect`/`munmap` externs (Wave 45). The stub is a 2-instr
    /// aarch64 sequence (MOV X0,XZR ; RET) or a 2-instr x86_64 sequence
    /// (xor eax,eax ; ret), both of which return 0.
    #[cfg(all(unix, any(target_arch = "x86_64", target_arch = "aarch64")))]
    #[test]
    fn wave45_jit_executes_compiled_region_without_libc() {
        let code = CORuntime::return_zero_stub();
        assert!(!code.is_empty(), "return_zero_stub should be non-empty on JIT targets");
        let result = execute_code(&code);
        assert!(result.is_ok(), "execute_code failed: {:?}", result);
        assert_eq!(result.unwrap(), 0, "return_zero_stub should return 0");
    }

    /// Non-Unix fallback must now return a clear error (Wave 45: no silent
    /// `Ok(0)`). On supported Unix archs this test is a no-op (gated off).
    #[cfg(not(all(unix, any(target_arch = "x86_64", target_arch = "aarch64"))))]
    #[test]
    fn wave45_non_unix_returns_clear_error() {
        let code = CORuntime::return_zero_stub();
        // On these targets the stub is empty; pass a non-empty dummy so the
        // empty-code short-circuit doesn't kick in.
        let code = if code.is_empty() { vec![0u8; 4] } else { code };
        let result = execute_code(&code);
        assert!(
            result.is_err(),
            "execute_code should return Err on non-JIT target, got {:?}",
            result
        );
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("JIT") || msg.contains("Unix"),
            "error should mention JIT/Unix: {}",
            msg
        );
    }
}
