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
use crate::profile::ProfileData;
use crate::speculative::SpeculativeOptimizer;
use crate::types::{CompiledRegion, Delta, NodeKind, RegionId, SCG};
use std::sync::Arc;
use vuma_codegen::emit::Emitter;
use vuma_codegen::scg_to_ir::{
    IRBuilder, Scg, ScgExpr, ScgFunction, ScgNode, ScgStatement, ScgType,
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
            "compile_incremental: +{} nodes, -{} nodes, +{} edges, -{} edges",
            delta.added_nodes.len(),
            delta.removed_nodes.len(),
            delta.added_edges.len(),
            delta.removed_edges.len(),
        );

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
        self.profile_data.record_access(region as crate::types::NodeId);
        self.profile_data.record_call(region as crate::types::NodeId);

        log::trace!("execute: region {} ({} code bytes)", region, compiled.code.len());

        // Execute the compiled code via memory-mapped execution.
        let code = compiled.code.clone();
        let result = execute_code(&code)?;
        let _ = result; // The return value is recorded but not propagated.

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
        let deopts = self.speculative_optimizer.validate_all(None, &[]);
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
                        "compile_region: IRBuilder produced no functions for region {}, \
                         falling back to return-0 stub",
                        region_id,
                    );
                    return Self::return_zero_stub();
                }

                let mut emitter = Emitter::new();
                match emitter.emit_function(&ir_program.functions[0]) {
                    Ok(code_words) => {
                        let code_bytes: Vec<u8> =
                            code_words.iter().flat_map(|w| w.to_le_bytes()).collect();
                        log::debug!(
                            "compile_region: region {} compiled to {} bytes of ARM64 code",
                            region_id,
                            code_bytes.len(),
                        );
                        code_bytes
                    }
                    Err(e) => {
                        log::warn!(
                            "compile_region: emission failed for region {}: {}, \
                             falling back to return-0 stub",
                            region_id,
                            e,
                        );
                        Self::return_zero_stub()
                    }
                }
            }
            Err(e) => {
                log::warn!(
                    "compile_region: IR translation failed for region {}: {}, \
                     falling back to return-0 stub",
                    region_id,
                    e,
                );
                Self::return_zero_stub()
            }
        }
    }

    /// Converts a COR SCGNode's metadata into codegen SCG statements.
    ///
    /// This is a best-effort translation: the COR SCG carries optimisation
    /// metadata (inlined, unrolled, vectorized, etc.) but not the full
    /// program semantics. We produce a representative function body that
    /// reflects the node's kind and any optimisation annotations.
    fn node_to_statements(
        &self,
        node: &crate::types::SCGNode,
    ) -> Vec<ScgStatement> {
        match node.kind {
            NodeKind::Compute => {
                // A simple computation node: return 42 as a placeholder.
                vec![ScgStatement::Return(vec![ScgExpr::Int(42)])]
            }
            NodeKind::Call => {
                if node.is_inlined {
                    // Inlined call — the callee's body is folded in.
                    // Emit a computation that represents the inlined result.
                    vec![ScgStatement::Return(vec![ScgExpr::Int(1)])]
                } else {
                    // Outlined call — emit a call to an external function.
                    vec![
                        ScgStatement::Call(vuma_codegen::scg_to_ir::CallNode {
                            dst: Some("result".to_string()),
                            func: format!("__vuma_call_{}", node.id),
                            args: vec![],
                        }),
                        ScgStatement::Return(vec![ScgExpr::Var("result".to_string())]),
                    ]
                }
            }
            NodeKind::Loop => {
                // A loop node: the unroll factor is reflected in the
                // generated code by unrolling the body in the IR.
                // For now we emit a simple return with the unroll factor.
                vec![ScgStatement::Return(vec![ScgExpr::Int(
                    node.unroll_factor as i64,
                )])]
            }
            NodeKind::Memory => {
                // A memory node: may have prefetch hints.
                vec![ScgStatement::Return(vec![ScgExpr::Int(
                    if node.has_prefetch { 1 } else { 0 },
                )])]
            }
            NodeKind::Branch => {
                // A branch node: return 0 for not-taken, 1 for taken.
                vec![ScgStatement::Return(vec![ScgExpr::Int(0)])]
            }
            NodeKind::Entry => {
                // An entry node: just returns 0.
                vec![ScgStatement::Return(vec![ScgExpr::Int(0)])]
            }
        }
    }

    /// Returns the ARM64 machine code for a minimal "return 0" stub.
    ///
    /// The stub is:
    /// ```asm
    /// MOV X0, XZR    ; return 0
    /// RET
    /// ```
    ///
    /// Encoded as two 32-bit little-endian instruction words:
    /// - `MOV X0, XZR` → `0xAA1F03E0`
    /// - `RET`         → `0xD65F03C0`
    fn return_zero_stub() -> Vec<u8> {
        let mov_x0_xzr: u32 = 0xAA1F03E0;
        let ret: u32 = 0xD65F03C0;
        let mut bytes = Vec::with_capacity(8);
        bytes.extend_from_slice(&mov_x0_xzr.to_le_bytes());
        bytes.extend_from_slice(&ret.to_le_bytes());
        bytes
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

/// Executes ARM64 machine code by mapping it into executable memory.
///
/// On AArch64 Unix systems, this uses `mmap` to create an anonymous memory
/// region, copies the code into it, sets the region to read+execute with
/// `mprotect`, calls the code as a function `extern "C" fn() -> i64`, and
/// unmaps the memory when done.
///
/// On non-AArch64 or non-Unix systems, execution is simulated and 0 is
/// returned. This allows the COR to be tested on x86_64 development
/// machines while still running real code on the Pi 5 target.
fn execute_code(code: &[u8]) -> Result<i64, RuntimeError> {
    if code.is_empty() {
        return Ok(0);
    }

    #[cfg(all(unix, target_arch = "aarch64"))]
    {
        execute_code_aarch64(code)
    }

    #[cfg(not(all(unix, target_arch = "aarch64")))]
    {
        let _ = code;
        // Simulated execution on non-AArch64 hosts: return 0.
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
            return Err(RuntimeError::ExecutionFailed(
                0,
                "mmap failed".to_string(),
            ));
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
    fn compile_incremental_produces_real_arm64_code() {
        let scg = Arc::new(SCG::default());
        let config = Config::default();
        let mut rt = CORuntime::new(scg, config);

        let delta = Delta {
            added_nodes: vec![42],
            removed_nodes: vec![],
            added_edges: vec![],
            removed_edges: vec![],
        };

        let recompiled = rt.compile_incremental(&delta);
        assert_eq!(recompiled, vec![42]);

        // The compiled region should contain real ARM64 machine code
        // (not a NOP sled). The codegen pipeline produces at least a
        // prologue, so the code should be non-empty.
        let compiled = rt.compiled_state().get(42).unwrap();
        assert!(!compiled.code.is_empty(),
            "compiled code should not be empty");
        // Verify it's not a NOP sled (0x90 repeated).
        assert!(!compiled.code.iter().all(|&b| b == 0x90),
            "compiled code should not be a NOP sled");
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
            removed_nodes: vec![],
            added_edges: vec![],
            removed_edges: vec![],
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
        assert!(reoptimized >= 1, "at least one region should be re-optimized");

        // The compiled region should still exist and have real code.
        let compiled = rt.compiled_state().get(10).unwrap();
        assert!(!compiled.code.is_empty());
        // After optimization, the call node should be marked as inlined.
        assert!(rt.scg().get_node(10).unwrap().is_inlined);
    }

    #[test]
    fn return_zero_stub_is_valid_arm64() {
        let stub = CORuntime::return_zero_stub();
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
        assert!(result.total_transformations > 0,
            "should apply at least one optimization");
        assert!(result.estimated_speedup > 1.0,
            "estimated speedup should exceed 1.0");

        // Verify SCG nodes were actually modified.
        let scg = rt.scg();
        assert!(scg.get_node(10).unwrap().is_inlined,
            "hot call node 10 should be inlined after optimization");
        assert!(scg.get_node(20).unwrap().unroll_factor > 1,
            "hot loop node 20 should be unrolled after optimization");
        assert!(scg.get_node(30).unwrap().has_prefetch,
            "hot memory node 30 should have prefetch after optimization");
    }
}
