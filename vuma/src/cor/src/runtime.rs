//! Core runtime for the Continuous Optimization Runtime (COR).
//!
//! The [`CORuntime`] struct is the central orchestrator. It holds a shared
//! reference to the Semantic Computation Graph (SCG), the always-compiled
//! state, profile data, and runtime configuration. The runtime continuously
//! compiles, executes, profiles, and re-optimizes regions of the SCG.

use crate::config::Config;
use crate::deployment::DeploymentPlanner;
use crate::profile::ProfileData;
use crate::speculative::SpeculativeOptimizer;
use crate::types::{CompiledRegion, Delta, RegionId, SCG};
use std::sync::Arc;

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
/// caller drives the runtime via [`compile_incremental`], [`execute`], and
/// [`optimize`].
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
        CORuntime {
            scg,
            compiled_state: CompiledState::new(),
            profile_data: ProfileData::new(),
            config,
            speculative_optimizer: SpeculativeOptimizer::new(),
            deployment_planner,
        }
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
            "compile_incremental: +{} nodes, -{} nodes, +{} edges, -{} edges",
            delta.added_nodes.len(),
            delta.removed_nodes.len(),
            delta.added_edges.len(),
            delta.removed_edges.len(),
        );

        // In a full implementation we would:
        // 1. Determine which existing regions overlap with the delta.
        // 2. Invalidate those regions in compiled_state.
        // 3. Recompile affected regions via the code generation layer.
        //
        // For now we create stub compiled regions for any added nodes.
        let mut recompiled = Vec::new();
        for &node_id in &delta.added_nodes {
            let region_id = node_id as RegionId; // simple mapping for now
            if !self.compiled_state.is_compiled(region_id) {
                let compiled = CompiledRegion {
                    region_id,
                    code: vec![0x90; 8], // NOP sled placeholder
                };
                self.compiled_state.insert(compiled);
                recompiled.push(region_id);
            }
        }

        // Remove compiled regions for deleted nodes.
        for &node_id in &delta.removed_nodes {
            let region_id = node_id as RegionId;
            self.compiled_state.remove(region_id);
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
    /// region is not compiled.
    pub fn execute(&mut self, region: RegionId) -> Result<(), RuntimeError> {
        let compiled = self
            .compiled_state
            .get(region)
            .ok_or(RuntimeError::NotCompiled(region))?;

        // Record profile data for this execution.
        self.profile_data.record_access(region as crate::types::NodeId);
        self.profile_data.record_call(region as crate::types::NodeId);

        log::trace!("execute: region {} ({} code bytes)", region, compiled.code.len());

        // In a full implementation we would jump to the compiled code.
        // For now this is a no-op that validates the compiled state.
        Ok(())
    }

    /// Runs one optimization cycle.
    ///
    /// This method:
    /// 1. Analyzes profile data to find hot paths.
    /// 2. Generates optimization suggestions.
    /// 3. Validates speculative assumptions.
    /// 4. Recompiles hot regions at a higher optimization level.
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
        let deopts = self.speculative_optimizer.validate_all(None, &[]);
        if deopts > 0 {
            log::warn!("optimize: {} speculative deoptimizations", deopts);
        }

        // Step 3: Re-optimize hot regions.
        let mut reoptimized = 0;
        for (node_id, count) in &hot_paths {
            let region_id = *node_id as RegionId;
            if self.compiled_state.is_compiled(region_id) && *count > 50 {
                // Re-compile at a higher optimization level.
                let optimized_code = CompiledRegion {
                    region_id,
                    code: vec![0x90; 4], // shorter = more optimized (placeholder)
                };
                self.compiled_state.insert(optimized_code);
                reoptimized += 1;
            }
        }

        log::debug!("optimize: re-optimized {} regions", reoptimized);
        reoptimized
    }

    /// Returns a reference to the compiled state.
    pub fn compiled_state(&self) -> &CompiledState {
        &self.compiled_state
    }

    /// Returns a reference to the profile data.
    pub fn profile_data(&self) -> &ProfileData {
        &self.profile_data
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

    /// Returns a reference to the SCG.
    pub fn scg(&self) -> &SCG {
        &self.scg
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

    /// Execution timed out.
    #[error("Execution of region {0} timed out after {1}ms")]
    Timeout(RegionId, u64),

    /// A verification violation was detected.
    #[error("Verification violation in region {0}: {1}")]
    VerificationViolation(RegionId, String),
}

#[cfg(test)]
mod tests {
    use super::*;

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
            removed_nodes: vec![],
            added_edges: vec![],
            removed_edges: vec![],
        };

        let recompiled = rt.compile_incremental(&delta);
        assert_eq!(recompiled.len(), 2);
        assert!(rt.compiled_state().is_compiled(10));
        assert!(rt.compiled_state().is_compiled(20));
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

        // Insert a compiled region manually.
        rt.compiled_state.insert(CompiledRegion {
            region_id: 1,
            code: vec![0x90],
        });

        let result = rt.execute(1);
        assert!(result.is_ok());
    }
}
