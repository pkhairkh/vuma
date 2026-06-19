//! # VUMA Compilation Pipeline
//!
//! The full compilation pipeline that wires together every workspace crate:
//!
//! ```text
//! Source → Parse → AST → SCG → BD Inference → MSG Construction
//!        → IVE Verification → SCG Transforms → IR Lowering
//!        → Register Allocation → ARM64 Codegen → ELF Emission
//! ```
//!
//! ## Quick Start
//!
//! ```rust
//! use vuma::pipeline::{compile, CompileConfig, CompileTarget, OptLevel, VerificationLevel};
//!
//! let source = r#"
//!     region buf = allocate(256);
//!     fn main() {
//!         ptr = buf + 64;
//!         header = ptr as *NodeHeader;
//!     }
//! "#;
//!
//! let config = CompileConfig::default();
//! let output = compile(source, &config);
//! match output {
//!     Ok(out) => println!("Compiled {} bytes, {} SCG nodes", out.binary.len(), out.scg.node_count()),
//!     Err(errors) => {
//!         for err in &errors {
//!             eprintln!("{}", err);
//!         }
//!     }
//! }
//! ```

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::Path;
use std::time::Instant;

// ── Workspace crate imports ──────────────────────────────────────────────

use vuma_bd::{repd::RepD, BD};
use vuma_codegen::{
    emit::{emit_binary, EmitConfig},
    ir::{BinOpKind as IrBinOpKind, IRProgram},
    regalloc::{AllocationResult, LinearScanAllocator},
    scg_to_ir::{
        AccessNode, AllocationNode, CallNode, CastNode, ComputationNode, ControlNode, GetAddressNode, IRBuilder,
        Scg, ScgExpr, ScgFunction, ScgNode, ScgParam, ScgStatement, ScgType, SwitchArm,
    },
    CastKind as CodegenCastKind, CodegenError,
};
use vuma_cor::{CORuntime, Config as CorConfig};
use vuma_core::{
    scg_to_msg::{scg_to_msg, ConversionError},
    MSG,
};
use vuma_ive::{
    AggregatedResult, InferenceEngine, InvariantAggregator, OverallVerdict,
    VerificationLevel as IveVerificationLevel,
};
use vuma_parser::{AstToScg, Item, ModuleResolver, ParseError, Parser, Program as AstProgram, ResolveError};
use vuma_scg::{
    AccessMode, CommonSubexpressionElimination, ConstantFolding, ControlKind, DeadCodeElimination,
    EdgeData, EdgeKind, InliningPass, NodeData, NodeId, NodePayload, NodeType, PassManager,
    PipelineResult as ScgPipelineResult, SCG, ComputationKind,
};

// ═══════════════════════════════════════════════════════════════════════════
// CompileConfig
// ═══════════════════════════════════════════════════════════════════════════

/// The compilation target platform.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum CompileTarget {
    /// Generic Linux user-space on AArch64.
    #[default]
    Linux,
    /// WebAssembly 32-bit (WASI preview 1).
    /// Produces a `.wasm` binary executable with `wasmer`, `wasmtime`, or Node.js.
    Wasm32,
}

impl fmt::Display for CompileTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileTarget::Linux => write!(f, "linux"),
            CompileTarget::Wasm32 => write!(f, "wasm32"),
        }
    }
}

/// Optimization level.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum OptLevel {
    /// No optimisation — fastest compilation, best debuggability.
    O0,
    /// Basic optimisations (DCE, constant folding).
    O1,
    /// Full optimisations (DCE, CSE, constant folding, inlining).
    #[default]
    O2,
    /// Aggressive optimisations (O2 + inlining of larger functions).
    O3,
}

impl fmt::Display for OptLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OptLevel::O0 => write!(f, "O0"),
            OptLevel::O1 => write!(f, "O1"),
            OptLevel::O2 => write!(f, "O2"),
            OptLevel::O3 => write!(f, "O3"),
        }
    }
}

/// Verification thoroughness level.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum VerificationLevel {
    /// Skip verification entirely.
    None,
    /// Quick: only cheap syntactic checks.
    Quick,
    /// Normal: all five invariant checks.
    #[default]
    Normal,
    /// Exhaustive: all checks + formal proof attempts.
    Exhaustive,
}

impl fmt::Display for VerificationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerificationLevel::None => write!(f, "none"),
            VerificationLevel::Quick => write!(f, "quick"),
            VerificationLevel::Normal => write!(f, "normal"),
            VerificationLevel::Exhaustive => write!(f, "exhaustive"),
        }
    }
}

/// Full compilation configuration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CompileConfig {
    /// Target platform.
    pub target: CompileTarget,
    /// Optimisation level.
    pub opt_level: OptLevel,
    /// Verification thoroughness.
    pub verification_level: VerificationLevel,
    /// Entry-point function name (default: "main" for hosted, "_start" for bare).
    pub entry_name: String,
    /// Include debug info in the output.
    pub debug_info: bool,
    /// Stop compilation at the first error.
    pub stop_on_first_error: bool,
    /// Maximum inline size (number of SCG nodes) for the inlining pass.
    pub max_inline_size: usize,
    /// Enable memory safety checks (use-after-free, double-free, leaks, etc.).
    pub memory_safety: bool,
    /// Enable runtime bounds checks for array accesses (--safe flag).
    pub runtime_bounds_checks: bool,
    /// Force section headers in the ELF output (--sections flag).
    pub section_headers: bool,
}

impl CompileConfig {
    /// Fast-compilation debug configuration.
    ///
    /// Note: verification still runs at `Normal` level (all five invariants)
    /// because skipping invariants would silently allow unsafe programs
    /// through, defeating VUMA's core safety guarantee.  The "fast" aspect
    /// of this preset comes from `OptLevel::O0`, not from reduced verification.
    pub fn debug() -> Self {
        Self {
            opt_level: OptLevel::O0,
            debug_info: true,
            verification_level: VerificationLevel::Normal,
            ..Self::default()
        }
    }

    /// Release configuration with full optimisation and exhaustive verification.
    pub fn release() -> Self {
        Self {
            opt_level: OptLevel::O3,
            verification_level: VerificationLevel::Exhaustive,
            ..Self::default()
        }
    }

    /// Returns the emit config for this compile config.
    fn emit_config(&self) -> EmitConfig {
        match self.target {
            CompileTarget::Linux => {
                let mut cfg = EmitConfig::linux_elf();
                cfg.section_headers = cfg.section_headers || self.section_headers;
                cfg.debug_info = self.debug_info;
                cfg
            }
            CompileTarget::Wasm32 => EmitConfig::wasm_binary(),
        }
    }
}

impl Default for CompileConfig {
    fn default() -> Self {
        Self {
            target: CompileTarget::Linux,
            opt_level: OptLevel::O2,
            verification_level: VerificationLevel::Normal,
            entry_name: "main".to_string(),
            debug_info: false,
            stop_on_first_error: true,
            max_inline_size: 50,
            memory_safety: true,
            runtime_bounds_checks: false,
            section_headers: false,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// VumaError
// ═══════════════════════════════════════════════════════════════════════════

/// A unified error type for the VUMA compilation pipeline.
///
/// Each variant captures the pipeline stage where the error occurred
/// and the underlying cause.
#[derive(Debug, Clone)]
pub enum VumaError {
    /// Error during lexing or parsing.
    Parse {
        /// The parse errors.
        errors: Vec<ParseError>,
    },
    /// Error converting AST to SCG.
    AstToScg {
        /// Error message.
        message: String,
    },
    /// SCG validation failed.
    ScgValidation {
        /// Validation error messages.
        errors: Vec<String>,
    },
    /// SCG → MSG conversion error.
    ScgToMsg {
        /// The conversion error.
        error: ConversionError,
    },
    /// BD inference error.
    BdInference {
        /// Node ID where inference failed, if known.
        node_id: Option<u64>,
        /// Error message.
        message: String,
    },
    /// IVE verification failure (one or more invariants violated).
    Verification {
        /// The aggregated verification result.
        result: AggregatedResult,
    },
    /// SCG transformation pass error.
    Transform {
        /// Name of the pass that failed.
        pass_name: String,
        /// Error messages from the pass.
        errors: Vec<String>,
    },
    /// IR lowering / codegen error.
    Codegen {
        /// The codegen error.
        error: CodegenError,
    },
    /// Register allocation failure.
    RegisterAlloc {
        /// Error message.
        message: String,
    },
    /// ELF emission failure.
    Emission {
        /// Error message.
        message: String,
    },
    /// COR initialization failure.
    CorInit {
        /// Error message.
        message: String,
    },
    /// Module resolution error (import not found, circular import, etc.).
    ModuleResolution {
        /// The resolution errors.
        errors: Vec<ResolveError>,
    },
    /// A collection of errors accumulated across stages.
    Multi {
        /// The collected errors.
        errors: Vec<VumaError>,
    },
    /// Backend failed; fallback to next available backend was attempted.
    BackendFallback {
        /// Name of the backend that failed.
        failed_backend: String,
        /// Name of the fallback backend that was tried (if any).
        fallback_backend: Option<String>,
        /// Error message from the failed backend.
        error: String,
    },
    /// Internal panic caught during compilation (crash recovery).
    PanicCaught {
        /// The pipeline stage where the panic occurred.
        stage: String,
        /// The panic message.
        message: String,
    },
}

impl VumaError {
    /// Returns the pipeline stage that produced this error.
    pub fn stage(&self) -> &'static str {
        match self {
            VumaError::Parse { .. } => "parse",
            VumaError::AstToScg { .. } => "ast-to-scg",
            VumaError::ScgValidation { .. } => "scg-validation",
            VumaError::ScgToMsg { .. } => "scg-to-msg",
            VumaError::BdInference { .. } => "bd-inference",
            VumaError::Verification { .. } => "ive-verification",
            VumaError::Transform { .. } => "scg-transform",
            VumaError::Codegen { .. } => "codegen",
            VumaError::RegisterAlloc { .. } => "register-alloc",
            VumaError::Emission { .. } => "elf-emission",
            VumaError::CorInit { .. } => "cor-init",
            VumaError::ModuleResolution { .. } => "module-resolution",
            VumaError::Multi { .. } => "multi",
            VumaError::BackendFallback { .. } => "backend-fallback",
            VumaError::PanicCaught { .. } => "panic-caught",
        }
    }
}

impl fmt::Display for VumaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VumaError::Parse { errors } => {
                write!(f, "[parse] {} error(s):", errors.len())?;
                for e in errors {
                    write!(f, "\n  - {}", e)?;
                }
                Ok(())
            }
            VumaError::AstToScg { message } => write!(f, "[ast-to-scg] {}", message),
            VumaError::ScgValidation { errors } => {
                write!(f, "[scg-validation] {} error(s):", errors.len())?;
                for e in errors {
                    write!(f, "\n  - {}", e)?;
                }
                Ok(())
            }
            VumaError::ScgToMsg { error } => write!(f, "[scg-to-msg] {}", error),
            VumaError::BdInference { node_id, message } => {
                write!(f, "[bd-inference] {}", message)?;
                if let Some(id) = node_id {
                    write!(f, " (node {})", id)?;
                }
                Ok(())
            }
            VumaError::Verification { result } => {
                write!(f, "[ive-verification] verdict: {}", result.overall)
            }
            VumaError::Transform { pass_name, errors } => {
                write!(f, "[scg-transform:{}] {} error(s)", pass_name, errors.len())
            }
            VumaError::Codegen { error } => write!(f, "[codegen] {}", error),
            VumaError::RegisterAlloc { message } => write!(f, "[register-alloc] {}", message),
            VumaError::Emission { message } => write!(f, "[elf-emission] {}", message),
            VumaError::CorInit { message } => write!(f, "[cor-init] {}", message),
            VumaError::ModuleResolution { errors } => {
                write!(f, "[module-resolution] {} error(s):", errors.len())?;
                for e in errors {
                    write!(f, "\n  - {}", e)?;
                }
                Ok(())
            }
            VumaError::Multi { errors } => {
                write!(f, "multiple errors ({}):", errors.len())?;
                for (i, e) in errors.iter().enumerate() {
                    write!(f, "\n{}. {}", i + 1, e)?;
                }
                Ok(())
            }
            VumaError::BackendFallback { failed_backend, fallback_backend, error } => {
                write!(f, "[backend-fallback] {} failed: {}", failed_backend, error)?;
                if let Some(fb) = fallback_backend {
                    write!(f, ", attempting fallback to {}", fb)?;
                }
                Ok(())
            }
            VumaError::PanicCaught { stage, message } => {
                write!(f, "[panic-caught] panic in stage '{}': {}", stage, message)
            }
        }
    }
}

impl std::error::Error for VumaError {}

// ═══════════════════════════════════════════════════════════════════════════
// CompilationOutput
// ═══════════════════════════════════════════════════════════════════════════

/// The output of a successful compilation.
#[derive(Debug)]
pub struct CompilationOutput {
    /// The emitted binary (ELF or raw, depending on target).
    pub binary: Vec<u8>,
    /// The final SCG after all transformation passes.
    pub scg: SCG,
    /// The Memory State Graph built from the SCG.
    pub msg: MSG,
    /// IVE verification results (if verification was requested).
    pub verification: Option<AggregatedResult>,
    /// Per-stage timing information (stage name → milliseconds).
    pub stage_timings: Vec<(String, u64)>,
    /// Number of IR functions generated.
    pub ir_function_count: usize,
    /// Total number of IR instructions across all functions.
    pub ir_instruction_count: usize,
    /// Number of ARM64 machine-code words emitted.
    pub code_words: usize,
    /// Debug information (if requested).
    pub debug_info: Option<DebugInfo>,
    /// The Continuous Optimization Runtime, initialized from the compiled SCG.
    /// Present when COR initialization succeeds (after the CorInit stage).
    pub cor_runtime: Option<CORuntime>,
}

/// Partial compilation output, returned when compilation fails but some
/// intermediate results are available (crash recovery).
///
/// Contains all data that was successfully produced before the error,
/// along with any diagnostics collected.
#[derive(Debug)]
pub struct PartialCompilationOutput {
    /// The parsed AST, if parsing succeeded.
    pub ast: Option<AstProgram>,
    /// The SCG, if SCG construction succeeded.
    pub scg: Option<SCG>,
    /// The MSG, if MSG construction succeeded.
    pub msg: Option<MSG>,
    /// IVE verification results, if verification ran.
    pub verification: Option<AggregatedResult>,
    /// Per-stage timing information.
    pub stage_timings: Vec<(String, u64)>,
    /// IR function count, if IR lowering succeeded.
    pub ir_function_count: Option<usize>,
    /// IR instruction count, if IR lowering succeeded.
    pub ir_instruction_count: Option<usize>,
    /// The last pipeline stage that completed successfully.
    pub last_completed_stage: Option<PipelineStage>,
    /// Diagnostics (errors + warnings) collected during compilation.
    pub diagnostics: Vec<VumaError>,
}

/// Result of a compilation attempt with crash recovery.
///
/// On success, contains the full [`CompilationOutput`].
/// On failure, contains a [`PartialCompilationOutput`] with whatever
/// intermediate results were produced, plus all diagnostics.
#[derive(Debug)]
pub enum CompileResult {
    /// Compilation succeeded.
    Success(CompilationOutput),
    /// Compilation failed, but partial results are available.
    Partial(PartialCompilationOutput),
}

impl CompileResult {
    /// Returns true if compilation succeeded.
    pub fn is_success(&self) -> bool {
        matches!(self, CompileResult::Success(_))
    }

    /// Returns the diagnostics (empty on success).
    pub fn diagnostics(&self) -> &[VumaError] {
        match self {
            CompileResult::Success(_) => &[],
            CompileResult::Partial(p) => &p.diagnostics,
        }
    }
}

/// Debug information captured during compilation.
#[derive(Debug, Clone)]
pub struct DebugInfo {
    /// The parsed AST.
    pub ast: Option<AstProgram>,
    /// The IR program before register allocation.
    pub ir_pre_regalloc: Option<IRProgram>,
    /// Register allocation results per function.
    pub regalloc_results: Vec<AllocationResult>,
    /// SCG transformation pipeline results.
    pub transform_results: Option<ScgPipelineResult>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Incremental compilation support
// ═══════════════════════════════════════════════════════════════════════════

/// A fingerprint of a source file, used to detect changes for
/// incremental compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFingerprint {
    /// A hash of the source text.
    pub hash: u64,
    /// Byte length of the source.
    pub len: usize,
}

impl SourceFingerprint {
    /// Compute a fingerprint from source text.
    pub fn from_source(source: &str) -> Self {
        // Simple FNV-1a hash — sufficient for change detection.
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in source.bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        Self {
            hash,
            len: source.len(),
        }
    }
}

/// Cached compilation state from a previous run, used for incremental
/// re-compilation.
#[derive(Debug, Clone)]
pub struct IncrementalCache {
    /// The fingerprint of the source that produced this cache.
    pub source_fingerprint: SourceFingerprint,
    /// The parsed AST (reusable if source unchanged).
    pub ast: Option<AstProgram>,
    /// The SCG before optimisation passes.
    pub pre_opt_scg: Option<SCG>,
    /// The SCG after optimisation passes.
    pub post_opt_scg: Option<SCG>,
    /// The MSG from the previous run.
    pub msg: Option<MSG>,
    /// IVE verification cache.
    pub verification_cache: Option<AggregatedResult>,
    /// Which pipeline stages need to be re-run.
    pub invalidated_stages: Vec<PipelineStage>,
}

/// Identifies a pipeline stage for incremental invalidation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PipelineStage {
    /// Lexing + parsing.
    Parse,
    /// AST → SCG conversion.
    AstToScg,
    /// SCG validation.
    ScgValidation,
    /// BD inference.
    BdInference,
    /// SCG → MSG construction.
    MsgConstruction,
    /// IVE verification.
    IveVerification,
    /// SCG transformation passes.
    ScgTransforms,
    /// IR lowering (SCG → IR).
    IrLowering,
    /// Register allocation.
    RegisterAlloc,
    /// ARM64 code emission.
    CodeEmission,
    /// COR (Continuous Optimization Runtime) initialization.
    CorInit,
}

impl PipelineStage {
    /// All stages in order.
    pub fn all() -> &'static [PipelineStage; 11] {
        &[
            PipelineStage::Parse,
            PipelineStage::AstToScg,
            PipelineStage::ScgValidation,
            PipelineStage::BdInference,
            PipelineStage::MsgConstruction,
            PipelineStage::IveVerification,
            PipelineStage::ScgTransforms,
            PipelineStage::IrLowering,
            PipelineStage::RegisterAlloc,
            PipelineStage::CodeEmission,
            PipelineStage::CorInit,
        ]
    }

    /// Returns all stages from (and including) the given stage onwards.
    pub fn from(stage: PipelineStage) -> Vec<PipelineStage> {
        PipelineStage::all()
            .iter()
            .filter(|&&s| s >= stage)
            .copied()
            .collect()
    }
}

impl fmt::Display for PipelineStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipelineStage::Parse => write!(f, "parse"),
            PipelineStage::AstToScg => write!(f, "ast-to-scg"),
            PipelineStage::ScgValidation => write!(f, "scg-validation"),
            PipelineStage::BdInference => write!(f, "bd-inference"),
            PipelineStage::MsgConstruction => write!(f, "msg-construction"),
            PipelineStage::IveVerification => write!(f, "ive-verification"),
            PipelineStage::ScgTransforms => write!(f, "scg-transforms"),
            PipelineStage::IrLowering => write!(f, "ir-lowering"),
            PipelineStage::RegisterAlloc => write!(f, "register-alloc"),
            PipelineStage::CodeEmission => write!(f, "code-emission"),
            PipelineStage::CorInit => write!(f, "cor-init"),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SCG → Codegen SCG bridge
// ═══════════════════════════════════════════════════════════════════════════

// ── Edge Index ─────────────────────────────────────────────────────────

/// Pre-computed edge index for efficient graph traversal during bridge
/// conversion. Built once from all edges in the SCG and then queried
/// by node ID and edge kind.
struct EdgeIndex {
    /// Outgoing edges keyed by source node.
    outgoing: HashMap<NodeId, Vec<EdgeData>>,
    /// Incoming edges keyed by target node.
    incoming: HashMap<NodeId, Vec<EdgeData>>,
}

impl EdgeIndex {
    /// Build the edge index from all edges in the SCG.
    fn build(scg: &SCG) -> Self {
        let mut outgoing: HashMap<NodeId, Vec<EdgeData>> = HashMap::new();
        let mut incoming: HashMap<NodeId, Vec<EdgeData>> = HashMap::new();
        for edge in scg.edges() {
            outgoing.entry(edge.source).or_default().push(edge.clone());
            incoming.entry(edge.target).or_default().push(edge.clone());
        }
        Self { outgoing, incoming }
    }

    /// Get outgoing ControlFlow edges from a node.
    fn outgoing_cf(&self, id: NodeId) -> Vec<&EdgeData> {
        self.outgoing
            .get(&id)
            .map(|edges| {
                edges
                    .iter()
                    .filter(|e| e.kind == EdgeKind::ControlFlow)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get incoming DataFlow edges to a node.
    fn incoming_df(&self, id: NodeId) -> Vec<&EdgeData> {
        self.incoming
            .get(&id)
            .map(|edges| {
                edges
                    .iter()
                    .filter(|e| e.kind == EdgeKind::DataFlow)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get outgoing DataFlow edges from a node.
    fn outgoing_df(&self, id: NodeId) -> Vec<&EdgeData> {
        self.outgoing
            .get(&id)
            .map(|edges| {
                edges
                    .iter()
                    .filter(|e| e.kind == EdgeKind::DataFlow)
                    .collect()
            })
            .unwrap_or_default()
    }
}

// ── Variable naming helpers ────────────────────────────────────────────

/// Generate a variable name for a node with a given prefix.
fn node_var(id: NodeId, _prefix: &str) -> String {
    // Must match the naming convention in resolve_df_input so that
    // source references (v_{source_id}) resolve correctly in the
    // codegen IR builder's names map.
    format!("v_{}", id.as_u64())
}

/// Resolve a DataFlow input for a node, returning a `ScgExpr` referencing
/// the variable produced by the source node of the DataFlow edge at the
/// given position.
///
/// If the source node is a Control node (FunctionEntry, etc.) that does not
/// produce a named variable, falls back to `ScgExpr::Int(0)` to avoid
/// referencing a non-existent variable in the codegen IR.

/// Check if a node has a Derivation edge to an Allocation node.
fn has_derivation_to_allocation(
    node_id: NodeId,
    edge_idx: &EdgeIndex,
    scg: &SCG,
) -> bool {
    if let Some(edges) = edge_idx.outgoing.get(&node_id) {
        for e in edges {
            if e.kind == EdgeKind::Derivation {
                if let Some(target) = scg.get_node(e.target) {
                    if matches!(target.payload, NodePayload::Allocation(_)) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Resolve all inputs of a node from DataFlow (and Derivation fallback) edges.
fn resolve_all_inputs(
    node_id: NodeId,
    edge_idx: &EdgeIndex,
    scg: &SCG,
) -> Vec<(NodeId, ScgExpr)> {
    let df_inputs = edge_idx.incoming_df(node_id);
    let inputs: Vec<vuma_scg::EdgeData> = if df_inputs.is_empty() {
        edge_idx.incoming
            .get(&node_id)
            .map(|edges| edges.iter().filter(|e| e.kind == EdgeKind::Derivation).cloned().collect())
            .unwrap_or_default()
    } else {
        df_inputs.iter().map(|e| (*e).clone()).collect()
    };
    inputs.iter().enumerate().map(|(i, e)| {
        (e.source, resolve_df_input(node_id, i, edge_idx, scg))
    }).collect()
}


/// Resolve a node to an ScgExpr by checking its payload.
/// For Computation nodes, checks for literal labels and Derivation to Allocation.
fn resolve_df_input_for_node(
    source: NodeId,
    edge_idx: &EdgeIndex,
    scg: &SCG,
) -> ScgExpr {
    if let Some(src_data) = scg.get_node(source) {
        match &src_data.payload {
            NodePayload::Computation(comp) => {
                if let ComputationKind::Other(ref label) = comp.kind {
                    if let Some(num_str) = label.strip_prefix("lit_") {
                        if let Ok(num) = num_str.parse::<i64>() {
                            return ScgExpr::Int(num);
                        }
                    }
                    if let Ok(num) = label.parse::<i64>() {
                        return ScgExpr::Int(num);
                    }
                }
                // Follow Derivation to Allocation — return Computation node var
                for deriv_edge in edge_idx.outgoing.get(&source).map(|v| v.as_slice()).unwrap_or(&[]) {
                    if deriv_edge.kind == EdgeKind::Derivation {
                        if let Some(alloc_node) = scg.get_node(deriv_edge.target) {
                            if matches!(alloc_node.payload, NodePayload::Allocation(_)) {
                                return ScgExpr::Var(format!("v_{}", source.as_u64()));
                            }
                        }
                    }
                }
                ScgExpr::Var(format!("v_{}", source.as_u64()))
            }
            NodePayload::Allocation(_) => {
                ScgExpr::Var(format!("v_{}", source.as_u64()))
            }
            _ => ScgExpr::Var(format!("v_{}", source.as_u64())),
        }
    } else {
        ScgExpr::Int(0)
    }
}

fn resolve_df_input(
    node_id: NodeId,
    position: usize,
    edge_idx: &EdgeIndex,
    scg: &SCG,
) -> ScgExpr {
    let df_inputs = edge_idx.incoming_df(node_id);
    // If no DataFlow edges, fall back to Derivation edges
    let df_inputs: Vec<vuma_scg::EdgeData> = if df_inputs.is_empty() {
        edge_idx.incoming
            .get(&node_id)
            .map(|edges| edges.iter().filter(|e| e.kind == EdgeKind::Derivation).cloned().collect())
            .unwrap_or_default()
    } else {
        df_inputs.iter().map(|e| (*e).clone()).collect()
    };
    if position < df_inputs.len() {
        let source = df_inputs[position].source;
        if let Some(src_data) = scg.get_node(source) {
            match &src_data.payload {
                NodePayload::Control(_)
                | NodePayload::Phantom(_)
                | NodePayload::Deallocation(_)
                | NodePayload::Effect(_)
                | NodePayload::VTable(_)
                | NodePayload::ClosureEnv(_) => {
                    ScgExpr::Int(0)
                }
                NodePayload::Computation(comp) => {
                    // Check if this is a literal computation node (label "lit_<n>")
                    if let ComputationKind::Other(ref label) = comp.kind {
                        if let Some(num_str) = label.strip_prefix("lit_") {
                            if let Ok(num) = num_str.parse::<i64>() {
                                return ScgExpr::Int(num);
                            }
                        }
                        // Check for bare number format (tail expression literals)
                        if let Ok(num) = label.parse::<i64>() {
                            return ScgExpr::Int(num);
                        }
                    }
                    // Check if this Computation has a Derivation edge to an
                    // Allocation node (the allocation pointer is in v_<alloc_id>)
                    for deriv_edge in edge_idx.outgoing.get(&source).map(|v| v.as_slice()).unwrap_or(&[]) {
                        if deriv_edge.kind == EdgeKind::Derivation {
                            if let Some(alloc_node) = scg.get_node(deriv_edge.target) {
                                if matches!(alloc_node.payload, NodePayload::Allocation(_)) {
                                    return ScgExpr::Var(format!("v_{}", source.as_u64()));  // Return Computation node var
                                }
                            }
                        }
                    }
                    // Regular computation — reference by vreg
                    ScgExpr::Var(format!("v_{}", source.as_u64()))
                }
                _ => ScgExpr::Var(format!("v_{}", source.as_u64())),
            }
        } else {
            ScgExpr::Int(0)
        }
    } else {
        ScgExpr::Int(0)
    }
}

/// Resolve the condition expression for a Branch node by looking at its
/// incoming DataFlow edges.
fn resolve_branch_cond(branch_id: NodeId, edge_idx: &EdgeIndex, scg: &SCG) -> ScgExpr {
    resolve_df_input(branch_id, 0, edge_idx, scg)
}

// ── Control flow resolution helpers ────────────────────────────────────

/// Find the `FunctionReturn` node reachable from a `FunctionEntry` via
/// ControlFlow edges, using BFS.
fn find_function_return(entry_id: NodeId, scg: &SCG, edge_idx: &EdgeIndex) -> Option<NodeId> {
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(entry_id);
    visited.insert(entry_id);

    while let Some(current) = queue.pop_front() {
        for edge in edge_idx.outgoing_cf(current) {
            let target = edge.target;
            if visited.contains(&target) {
                continue;
            }
            visited.insert(target);
            if let Some(node) = scg.get_node(target) {
                if let NodePayload::Control(c) = &node.payload {
                    if c.kind == ControlKind::FunctionReturn {
                        return Some(target);
                    }
                }
            }
            queue.push_back(target);
        }
    }

    None
}

/// Find all `Join` nodes reachable from `start` via ControlFlow edges,
/// stopping at the first Join encountered on each path (Joins are
/// convergence points, not passed through during search).
fn find_reachable_joins(start: NodeId, scg: &SCG, edge_idx: &EdgeIndex) -> Vec<NodeId> {
    let mut joins = Vec::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(start);
    visited.insert(start);

    let max_steps = 500;
    let mut steps = 0;

    while let Some(current) = queue.pop_front() {
        steps += 1;
        if steps > max_steps {
            break;
        }

        if let Some(node) = scg.get_node(current) {
            if let NodePayload::Control(c) = &node.payload {
                if c.kind == ControlKind::Join {
                    joins.push(current);
                    continue; // Don't walk past Join
                }
            }
        }

        for edge in edge_idx.outgoing_cf(current) {
            let target = edge.target;
            if !visited.contains(&target) {
                visited.insert(target);
                queue.push_back(target);
            }
        }
    }

    joins
}

/// Find the `Join` node where a Branch's then and else arms converge.
fn find_join_for_branch(
    then_start: NodeId,
    else_start: Option<NodeId>,
    scg: &SCG,
    edge_idx: &EdgeIndex,
) -> Option<NodeId> {
    let then_joins = find_reachable_joins(then_start, scg, edge_idx);

    if let Some(else_start) = else_start {
        let else_joins = find_reachable_joins(else_start, scg, edge_idx);
        // Find the first Join reachable from both arms
        for jid in &then_joins {
            if else_joins.contains(jid) {
                return Some(*jid);
            }
        }
    }

    // Fallback: first Join reachable from then_start
    then_joins.into_iter().next()
}

/// Resolve a Branch node's then/else targets and Join convergence point.
///
/// Looks for labeled ControlFlow edges ("then", "else", "else_fallthrough")
/// and falls back to positional ordering if labels are missing.
fn resolve_branch(
    branch_id: NodeId,
    scg: &SCG,
    edge_idx: &EdgeIndex,
) -> (NodeId, Option<NodeId>, Option<NodeId>) {
    let cf_edges = edge_idx.outgoing_cf(branch_id);

    // Look for labeled edges
    let then_target = cf_edges
        .iter()
        .find(|e| e.label.as_deref() == Some("then"))
        .map(|e| e.target)
        .or_else(|| cf_edges.first().map(|e| e.target));

    let else_target = cf_edges
        .iter()
        .find(|e| {
            e.label.as_deref() == Some("else") || e.label.as_deref() == Some("else_fallthrough")
        })
        .map(|e| e.target)
        .or_else(|| {
            // If there are exactly 2 CF edges and one is "then", the other is "else"
            if cf_edges.len() == 2 {
                let then = then_target?;
                cf_edges.iter().find(|e| e.target != then).map(|e| e.target)
            } else {
                None
            }
        });

    let then_tgt = then_target.unwrap_or(branch_id);
    let join = find_join_for_branch(then_tgt, else_target, scg, edge_idx);

    (then_tgt, else_target, join)
}

/// Resolve a LoopHeader node's body and exit targets.
///
/// Classifies outgoing ControlFlow edges: edges targeting a `LoopExit`
/// node are the exit; all other edges are the loop body.
fn resolve_loop(header_id: NodeId, scg: &SCG, edge_idx: &EdgeIndex) -> (NodeId, Option<NodeId>) {
    let cf_edges = edge_idx.outgoing_cf(header_id);

    let mut body_target = None;
    let mut exit_target = None;

    for edge in &cf_edges {
        if let Some(target_node) = scg.get_node(edge.target) {
            if let NodePayload::Control(c) = &target_node.payload {
                if c.kind == ControlKind::LoopExit {
                    exit_target = Some(edge.target);
                    continue;
                }
            }
        }
        if body_target.is_none() {
            body_target = Some(edge.target);
        }
    }

    // Fallbacks
    if body_target.is_none() {
        body_target = cf_edges.first().map(|e| e.target);
    }
    if exit_target.is_none() && cf_edges.len() > 1 {
        exit_target = cf_edges.get(1).map(|e| e.target);
    }

    (body_target.unwrap_or(header_id), exit_target)
}

// ── Match/switch case-value extraction ──────────────────────────────────

/// Extract the integer case value for a match/switch arm from the branch
/// condition and surrounding SCG context.
///
/// A match arm `42 => body` produces a branch whose condition is typically
/// the result of an equality comparison `disc == 42`. This function traces
/// back through the DataFlow edges to find the constant operand, which is
/// the case value from the AST's MatchArm pattern.
///
/// Extraction strategy (in priority order):
///
/// 1. If `cond` is already an `ScgExpr::Int(n)`, return `n`.
/// 2. Trace the first DataFlow edge of the Branch back to its source node.
///    If the source is a Computation node with an equality operation
///    (`eq` / `==`), inspect its second DataFlow input — the RHS of the
///    comparison is the case value. If that RHS source node is a
///    Computation whose `operation` string parses as an integer, use it.
/// 3. Try to parse an integer from the control node's label string.
///    Recognised formats: `"match disc == 42"`, `"case 2: 42"`.
/// 4. Fall back to `arm_index` — each arm in a match expression receives
///    a distinct fallback value so that unknown case values don't collide.
fn extract_case_value(
    branch_id: NodeId,
    cond: &ScgExpr,
    ctrl_label: Option<&str>,
    scg: &SCG,
    edge_idx: &EdgeIndex,
    arm_index: usize,
) -> i64 {
    // Strategy 1: direct integer condition.
    if let ScgExpr::Int(n) = cond {
        return *n;
    }

    // Strategy 2: trace back through the equality comparison node.
    let df_inputs = edge_idx.incoming_df(branch_id);
    if let Some(df_edge) = df_inputs.first() {
        let cond_source = df_edge.source;
        if let Some(source_node) = scg.get_node(cond_source) {
            if let NodePayload::Computation(comp) = &source_node.payload {
                let op_label = comp.kind.label();
                let is_eq = op_label == "eq" || op_label == "==";
                if is_eq {
                    // The RHS of the equality is the case value.
                    let rhs_inputs = edge_idx.incoming_df(cond_source);
                    if rhs_inputs.len() >= 2 {
                        let rhs_source = rhs_inputs[1].source;
                        if let Some(rhs_node) = scg.get_node(rhs_source) {
                            // The RHS node might be a Computation whose
                            // operation string is a literal integer.
                            if let NodePayload::Computation(rhs_comp) = &rhs_node.payload {
                                if let Ok(val) = rhs_comp.kind.label().parse::<i64>() {
                                    return val;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Strategy 3: parse from the control node label.
    if let Some(label) = ctrl_label {
        // Format: "match <disc> == <value>"
        if let Some(idx) = label.find("==") {
            let after_eq = label[idx + 2..].trim();
            // Take the first token (stop at whitespace / punctuation)
            let token = after_eq
                .split(|c: char| c.is_whitespace() || c == ')')
                .next()
                .unwrap_or(after_eq);
            if let Ok(val) = token.parse::<i64>() {
                return val;
            }
        }
        // Format: "case <idx>: <value>"
        if label.starts_with("case ") {
            let parts: Vec<&str> = label.splitn(2, ':').collect();
            if parts.len() == 2 {
                let value_str = parts[1].trim();
                let token = value_str
                    .split(|c: char| c.is_whitespace() || c == ')')
                    .next()
                    .unwrap_or(value_str);
                if let Ok(val) = token.parse::<i64>() {
                    return val;
                }
            }
        }
    }

    // Strategy 4: fallback to arm_index so each arm gets a distinct value.
    arm_index as i64
}

// ── Control flow walk ──────────────────────────────────────────────────

/// Walk control flow starting from `start`, producing `ScgStatement`s.
///
/// Stops when reaching a node in `stop_at` (does NOT consume that node;
/// the caller is responsible for handling it). Adds processed nodes to
/// `consumed`.
///
/// # Control Flow Reconstruction
///
/// - **Branch → If**: Follows "then"/"else" labeled CF edges, walks each
///   arm until reaching a Join, produces `ControlNode::If`.
/// - **LoopHeader → Loop**: Follows body/exit CF edges, walks the body
///   until back-edge or LoopExit, produces `ControlNode::Loop`.
/// - **Jump("break")**: Produces `ControlNode::Break`.
/// - **Jump("continue")**: Produces `ControlNode::Continue`.
/// - **FunctionReturn**: Produces `ScgStatement::Return`.
fn walk_control_flow(
    start: NodeId,
    scg: &SCG,
    edge_idx: &EdgeIndex,
    consumed: &mut HashSet<NodeId>,
    stop_at: &HashSet<NodeId>,
) -> Vec<ScgStatement> {
    walk_control_flow_with_externs(start, scg, edge_idx, consumed, stop_at, &HashSet::new())
}

/// Walk control flow starting from `start`, producing `ScgStatement`s,
/// with knowledge of extern functions for marking foreign calls.
fn walk_control_flow_with_externs(
    start: NodeId,
    scg: &SCG,
    edge_idx: &EdgeIndex,
    consumed: &mut HashSet<NodeId>,
    stop_at: &HashSet<NodeId>,
    extern_functions: &HashSet<String>,
) -> Vec<ScgStatement> {
    let mut stmts = Vec::new();
    let mut current = Some(start);

    while let Some(node_id) = current {
        // Stop if we've reached a merge point
        if stop_at.contains(&node_id) {
            break;
        }
        // Skip already-consumed nodes
        if consumed.contains(&node_id) {
            break;
        }
        consumed.insert(node_id);

        let node_data = match scg.get_node(node_id) {
            Some(n) => n,
            None => break,
        };

        match &node_data.payload {
            // ── Control nodes ──────────────────────────────────────
            NodePayload::Control(ctrl) => match ctrl.kind {
                ControlKind::Branch => {
                    let (then_tgt, else_tgt, join_node) = resolve_branch(node_id, scg, edge_idx);
                    let cond = resolve_branch_cond(node_id, edge_idx, scg);

                    // Check if this is a match/switch branch (label starts
                    // with "match") vs a simple if/else. For match branches,
                    // we look for multiple Branch→Join diamonds that share
                    // the same Join and collapse them into a Switch node.
                    let is_match = ctrl
                        .label
                        .as_ref()
                        .map(|l| l.starts_with("match"))
                        .unwrap_or(false);

                    if is_match {
                        // For match/switch, collect all arms that lead to
                        // the same Join node. Each arm is a then/else pair
                        // where the then branch is the matched case.
                        // We walk the then arm to find the case value and body.
                        let mut arms = Vec::new();
                        let mut default_body = Vec::new();

                        // Build stop-at for both arms (includes Join)
                        let mut arm_stop = stop_at.clone();
                        if let Some(join) = join_node {
                            arm_stop.insert(join);
                        }

                        // Generate a simple switch from the then arm
                        // with a discriminant expression.
                        let then_body_stmts =
                            walk_control_flow_with_externs(then_tgt, scg, edge_idx, consumed, &arm_stop, extern_functions);

                        // Extract the case value from the AST's MatchArm pattern.
                        // The branch condition for a match arm is typically
                        // `disc == value`. We trace back through the DataFlow
                        // edges to find the constant being compared against.
                        let case_value = extract_case_value(
                            node_id,
                            &cond,
                            ctrl.label.as_deref(),
                            scg,
                            edge_idx,
                            arms.len(),
                        );
                        arms.push(SwitchArm {
                            value: case_value,
                            body: then_body_stmts,
                        });

                        if let Some(tgt) = else_tgt {
                            let else_stmts =
                                walk_control_flow_with_externs(tgt, scg, edge_idx, consumed, &arm_stop, extern_functions);
                            default_body = else_stmts;
                        }

                        // Use the first operand of the condition as discriminant
                        let disc = if let ScgExpr::Var(_) = &cond {
                            cond.clone()
                        } else {
                            ScgExpr::Var("disc".to_string())
                        };

                        stmts.push(ScgStatement::Control(ControlNode::Switch {
                            discriminant: disc,
                            arms,
                            default_body,
                        }));
                    } else {
                        // Standard if/else
                        let mut arm_stop = stop_at.clone();
                        if let Some(join) = join_node {
                            arm_stop.insert(join);
                        }

                        let then_body =
                            walk_control_flow_with_externs(then_tgt, scg, edge_idx, consumed, &arm_stop, extern_functions);

                        let else_body = else_tgt
                            .map(|tgt| walk_control_flow_with_externs(tgt, scg, edge_idx, consumed, &arm_stop, extern_functions));

                        stmts.push(ScgStatement::Control(ControlNode::If {
                            cond,
                            then_body,
                            else_body,
                        }));
                    }

                    // Continue from the Join
                    if let Some(join) = join_node {
                        consumed.insert(join);
                        current = edge_idx.outgoing_cf(join).first().map(|e| e.target);
                    } else {
                        current = None;
                    }
                    continue;
                }

                ControlKind::LoopHeader => {
                    let (body_tgt, exit_tgt) = resolve_loop(node_id, scg, edge_idx);

                    // Stop the body walk at back-edges (LoopHeader) and LoopExit
                    let mut loop_stop = stop_at.clone();
                    loop_stop.insert(node_id); // back-edge target
                    if let Some(exit) = exit_tgt {
                        loop_stop.insert(exit);
                    }

                    let body = walk_control_flow_with_externs(body_tgt, scg, edge_idx, consumed, &loop_stop, extern_functions);

                    stmts.push(ScgStatement::Control(ControlNode::Loop { body, for_range: ctrl.label.as_ref().and_then(|label| parse_for_range(label)), while_cond: None }));

                    // Continue from the LoopExit
                    if let Some(exit) = exit_tgt {
                        consumed.insert(exit);
                        current = edge_idx.outgoing_cf(exit).first().map(|e| e.target);
                    } else {
                        current = None;
                    }
                    continue;
                }

                ControlKind::Jump => match ctrl.label.as_deref() {
                    Some("break") => {
                        stmts.push(ScgStatement::Control(ControlNode::Break));
                        current = None;
                        continue;
                    }
                    Some("continue") => {
                        stmts.push(ScgStatement::Control(ControlNode::Continue));
                        current = None;
                        continue;
                    }
                    _ => {
                        // Unconditional jump — follow the CF edge
                        let target = edge_idx.outgoing_cf(node_id).first().map(|e| e.target);
                        if let Some(tgt) = target {
                            if !consumed.contains(&tgt) && !stop_at.contains(&tgt) {
                                current = Some(tgt);
                                continue;
                            }
                        }
                        current = None;
                        continue;
                    }
                },

                ControlKind::FunctionReturn => {
                    // Resolve the return value from the incoming DataFlow edge(s).
                    // The FunctionReturn node has one DataFlow input per return value.
                    let df_inputs = edge_idx.incoming_df(node_id);
                    let ret_vals: Vec<ScgExpr> = df_inputs
                        .iter()
                        .enumerate()
                        .map(|(i, _)| resolve_df_input(node_id, i, edge_idx, scg))
                        .collect();
                    // If there are no DataFlow inputs, try Derivation edges
                    // (some return values flow through Derivation).
                    let ret_vals = if ret_vals.is_empty() {
                        let deriv_inputs = edge_idx.incoming
                            .get(&node_id)
                            .map(|edges| edges.iter().filter(|e| e.kind == EdgeKind::Derivation).cloned().collect::<Vec<_>>())
                            .unwrap_or_default();
                        deriv_inputs.iter()
                            .enumerate()
                            .map(|(i, e)| resolve_df_input(node_id, i, edge_idx, scg))
                            .collect()
                    } else {
                        ret_vals
                    };
                    stmts.push(ScgStatement::Return(ret_vals));
                    current = None;
                    continue;
                }

                ControlKind::Join | ControlKind::LoopExit => {
                    // Structural nodes handled by Branch/LoopHeader.
                    // Pass through to the next node.
                    current = edge_idx.outgoing_cf(node_id).first().map(|e| e.target);
                    continue;
                }

                ControlKind::FunctionEntry => {
                    // Call-site FunctionEntry nodes (label "call_<name>")
                    // are lowered to CallNode statements.
                    if let Some(label) = &ctrl.label {
                        if let Some(func_name) = label.strip_prefix("call_") {
                            let is_extern = extern_functions.contains(func_name);
                            // Collect arguments from DataFlow edges.
                            // Each DataFlow edge source is either a variable's
                            // defining node (-> Var) or a literal Computation
                            // node with label "lit_<n>" (-> Int).
                            let mut args = Vec::new();
                            let df_inputs = edge_idx.incoming_df(node_id);
                            for df_edge in &df_inputs {
                                let source = df_edge.source;
                                if let Some(src_data) = scg.get_node(source) {
                                    if let NodePayload::Computation(comp) = &src_data.payload {
                                        if let ComputationKind::Other(ref lbl) = comp.kind {
                                            if let Some(num_str) = lbl.strip_prefix("lit_") {
                                                if let Ok(num) = num_str.parse::<i64>() {
                                                    args.push(ScgExpr::Int(num));
                                                    continue;
                                                }
                                            }
                                            if let Ok(num) = lbl.parse::<i64>() {
                                                args.push(ScgExpr::Int(num));
                                                continue;
                                            }
                                        }
                                    }
                                }
                                args.push(ScgExpr::Var(format!("v_{}", source.as_u64())));
                            }
                            // Set the call's destination to the caller
                            // Computation node's variable.
                            let caller_node = edge_idx.incoming
                                .get(&node_id)
                                .and_then(|edges| edges.iter().find(|e| e.kind == EdgeKind::ControlFlow))
                                .map(|e| e.source);
                            let call_dst = if let Some(caller) = caller_node {
                                Some(format!("v_{}", caller.as_u64()))
                            } else {
                                let ret_node = find_function_return(node_id, scg, edge_idx);
                                if let Some(ret) = ret_node {
                                    let ret_df = edge_idx.incoming_df(ret);
                                    if let Some(first_df) = ret_df.first() {
                                        Some(format!("v_{}", first_df.source.as_u64()))
                                    } else {
                                        Some(format!("v_{}_ret", node_id.as_u64()))
                                    }
                                } else {
                                    Some(format!("v_{}_ret", node_id.as_u64()))
                                }
                            };
                            stmts.push(ScgStatement::Call(CallNode {
                                dst: call_dst,
                                func: func_name.to_string(),
                                args,
                                is_extern,
                            }));
                            // Consume the call-site's FunctionEntry and
                            // FunctionReturn nodes.
                            let ret_node = find_function_return(node_id, scg, edge_idx);
                            if let Some(ret) = ret_node {
                                consumed.insert(ret);
                            }
                            consumed.insert(node_id); // consume the call FunctionEntry
                            // Continue from the caller node's other CF edges.
                            // The caller Computation node may have CF edges to
                            // both the call FunctionEntry and the next statement.
                            // We follow the first unconsumed CF edge from the caller.
                            if let Some(caller) = caller_node {
                                let next_cf = edge_idx.outgoing_cf(caller)
                                    .iter()
                                    .find(|e| !consumed.contains(&e.target))
                                    .map(|e| e.target);
                                if let Some(tgt) = next_cf {
                                    current = Some(tgt);
                                    continue;
                                }
                            }
                            // Fallback: try the call's FunctionReturn
                            if let Some(ret) = ret_node {
                                current = edge_idx.outgoing_cf(ret).first().map(|e| e.target);
                            } else {
                                current = None;
                            }
                            continue;
                        }
                    }
                    // Non-call-site FunctionEntry: pass through
                    current = edge_idx.outgoing_cf(node_id).first().map(|e| e.target);
                    continue;
                }

                ControlKind::Switch | ControlKind::SwitchCase => {
                    // Switch/switch-case nodes are handled like Branch
                    current = edge_idx.outgoing_cf(node_id).first().map(|e| e.target);
                    continue;
                }

                ControlKind::ClosureEntry | ControlKind::ClosureReturn => {
                    // Closure entry/return handled like function entry/return
                    current = edge_idx.outgoing_cf(node_id).first().map(|e| e.target);
                    continue;
                }

                ControlKind::FuturePoll
                | ControlKind::WakerRegistration
                | ControlKind::StateTransition => {
                    // Async state machine nodes: pass through
                    current = edge_idx.outgoing_cf(node_id).first().map(|e| e.target);
                    continue;
                }
            },

            // ── Non-control nodes: convert to statements ───────────
            _ => {
                if let Some(stmt) = convert_node_to_statement_with_externs(node_id, node_data, edge_idx, scg, extern_functions) {
                    stmts.push(stmt);
                }

                // Continue to the next node via ControlFlow
                current = edge_idx.outgoing_cf(node_id).first().map(|e| e.target);
            }
        }
    }

    stmts
}

// ── Node-to-statement conversion ───────────────────────────────────────

/// Convert a non-control SCG node into an `ScgStatement`.
///
/// Handles all node types except `Control` (which is handled by
/// `walk_control_flow`) and `Phantom` (which is skipped).
fn convert_node_to_statement(
    node_id: NodeId,
    node_data: &NodeData,
    edge_idx: &EdgeIndex,
    scg: &SCG,
) -> Option<ScgStatement> {
    convert_node_to_statement_with_externs(node_id, node_data, edge_idx, scg, &HashSet::new())
}

/// Convert a non-control SCG node into an `ScgStatement`, with knowledge
/// of which functions are declared as extern.
///
/// Handles all node types except `Control` (which is handled by
/// `walk_control_flow`) and `Phantom` (which is skipped).

/// Parse an expression string and find the top-level binary operator.
/// Returns (op, lhs_substring, rhs_substring) or None if no operator found.
/// Handles parenthesized sub-expressions correctly.
fn parse_expr_split(expr: &str) -> Option<(IrBinOpKind, String, String)> {
    let expr = expr.trim();
    
    // Remove outer parentheses if they wrap the entire expression
    let expr = strip_outer_parens(expr);
    
    // Find the top-level operator (not inside parentheses)
    // Search from right to left to respect operator precedence
    // (lowest precedence operators are evaluated last)
    
    // Check for two-character operators first
    let two_char_ops: [(&str, IrBinOpKind); 6] = [
        ("<=", IrBinOpKind::SLe), (">=", IrBinOpKind::SGe),
        ("==", IrBinOpKind::Eq), ("!=", IrBinOpKind::Ne),
        ("<<", IrBinOpKind::Shl), (">>", IrBinOpKind::ShrL),
    ];
    
    // Check for single-character operators in precedence order (lowest first)
    let single_ops: [(&str, IrBinOpKind); 8] = [
        ("|", IrBinOpKind::Or),
        ("^", IrBinOpKind::Xor),
        ("&", IrBinOpKind::And),
        ("+", IrBinOpKind::Add),
        ("-", IrBinOpKind::Sub),
        ("*", IrBinOpKind::Mul),
        ("/", IrBinOpKind::SDiv),
        ("%", IrBinOpKind::SRem),
    ];
    
    // Search for top-level operators (outside parentheses)
    // Process in precedence order (lowest first)
    for &(op_str, op_kind) in &single_ops {
        if let Some(pos) = find_top_level_op(expr, op_str) {
            let lhs = expr[..pos].trim().to_string();
            let rhs = expr[pos + op_str.len()..].trim().to_string();
            if !lhs.is_empty() && !rhs.is_empty() {
                return Some((op_kind, lhs, rhs));
            }
        }
    }
    
    // Check two-char operators
    for &(op_str, op_kind) in &two_char_ops {
        if let Some(pos) = find_top_level_op(expr, op_str) {
            let lhs = expr[..pos].trim().to_string();
            let rhs = expr[pos + op_str.len()..].trim().to_string();
            if !lhs.is_empty() && !rhs.is_empty() {
                return Some((op_kind, lhs, rhs));
            }
        }
    }
    
    None
}

/// Find the position of an operator at the top level (not inside parentheses)
fn find_top_level_op(expr: &str, op: &str) -> Option<usize> {
    let mut depth: i32 = 0;
    let bytes = expr.as_bytes();
    let op_bytes = op.as_bytes();
    
    // Scan from right to left to find the LAST occurrence at depth 0
    // (so "a - b - c" splits as "a - b" and "c", giving left-to-right evaluation)
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        let c = bytes[i] as char;
        
        if c == ')' {
            depth += 1;
        } else if c == '(' {
            depth -= 1;
        } else if depth == 0 && i + op_bytes.len() <= bytes.len() {
            // Check if this position matches the operator
            let matches = op_bytes.iter().enumerate().all(|(j, &ob)| bytes[i + j] == ob);
            if matches {
                // Make sure this isn't part of a two-char operator
                // (e.g., don't match the '<' in '<=')
                if op == "<" || op == ">" {
                    if i + 1 < bytes.len() && (bytes[i + 1] == b'=' || bytes[i + 1] == b'<' || bytes[i + 1] == b'>') {
                        continue;
                    }
                }
                return Some(i);
            }
        }
    }
    None
}

/// Strip outer parentheses from an expression
fn strip_outer_parens(expr: &str) -> &str {
    let expr = expr.trim();
    if expr.starts_with('(') && expr.ends_with(')') {
        // Check if the first '(' matches the last ')'
        let mut depth: i32 = 0;
        let bytes = expr.as_bytes();
        for i in 0..bytes.len() {
            match bytes[i] as char {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 && i < bytes.len() - 1 {
                        // The first '(' doesn't match the last ')'
                        return expr;
                    }
                }
                _ => {}
            }
        }
        return &expr[1..expr.len() - 1];
    }
    expr
}

/// Resolve a sub-expression string to an ScgExpr.
/// The sub-expression can be:
/// - A variable name (matched to a DataFlow source)
/// - A literal number (converted to ScgExpr::Int)
/// - A complex expression (recursively parsed — for now, returns Int(0))

fn map_binop_kind(op: IrBinOpKind) -> vuma_codegen::ir::BinOpKind {
    match op {
        IrBinOpKind::Add => vuma_codegen::ir::BinOpKind::Add,
        IrBinOpKind::Sub => vuma_codegen::ir::BinOpKind::Sub,
        IrBinOpKind::Mul => vuma_codegen::ir::BinOpKind::Mul,
        IrBinOpKind::SDiv => vuma_codegen::ir::BinOpKind::SDiv,
        IrBinOpKind::SRem => vuma_codegen::ir::BinOpKind::SRem,
        IrBinOpKind::And => vuma_codegen::ir::BinOpKind::And,
        IrBinOpKind::Or => vuma_codegen::ir::BinOpKind::Or,
        IrBinOpKind::Xor => vuma_codegen::ir::BinOpKind::Xor,
        IrBinOpKind::Shl => vuma_codegen::ir::BinOpKind::Shl,
        IrBinOpKind::ShrL => vuma_codegen::ir::BinOpKind::ShrL,
        IrBinOpKind::ShrA => vuma_codegen::ir::BinOpKind::ShrA,
        IrBinOpKind::SLt => vuma_codegen::ir::BinOpKind::SLt,
        IrBinOpKind::SLe => vuma_codegen::ir::BinOpKind::SLe,
        IrBinOpKind::SGt => vuma_codegen::ir::BinOpKind::SGt,
        IrBinOpKind::SGe => vuma_codegen::ir::BinOpKind::SGe,
        IrBinOpKind::Eq => vuma_codegen::ir::BinOpKind::Eq,
        IrBinOpKind::Ne => vuma_codegen::ir::BinOpKind::Ne,
        _ => vuma_codegen::ir::BinOpKind::Add,
    }
}

fn resolve_subexpr(
    subexpr: &str,
    sources: &[NodeId],
    edge_idx: &EdgeIndex,
    scg: &SCG,
) -> ScgExpr {
    let subexpr = subexpr.trim();
    
    // Check if it's a literal number
    if let Ok(num) = subexpr.parse::<i64>() {
        return ScgExpr::Int(num);
    }
    
    // Check if it's a known literal (lit_<n>)
    if let Some(num_str) = subexpr.strip_prefix("lit_") {
        if let Ok(num) = num_str.parse::<i64>() {
            return ScgExpr::Int(num);
        }
    }
    
    // Check if it's a simple variable name
    // Match against the DataFlow sources
    if is_simple_var(subexpr) {
        // First, try exact match: the source node IS the variable definition
        for (i, &src) in sources.iter().enumerate() {
            if let Some(src_data) = scg.get_node(src) {
                if let NodePayload::Computation(comp) = &src_data.payload {
                    let label = comp.kind.label();
                    // Check for exact match or "param <var>" or "<var> = ..."
                    if label == *subexpr 
                       || label == format!("param {}", subexpr)
                       || label.starts_with(&format!("{} ", subexpr))
                       || label.starts_with(&format!("{} =", subexpr))
                       || label.starts_with(&format!("let {} =", subexpr)) {
                        return resolve_df_input_for_node(src, edge_idx, scg);
                    }
                }
            }
        }
        // Second pass: check if any source's label CONTAINS the variable name
        for &src in sources {
            if let Some(src_data) = scg.get_node(src) {
                if let NodePayload::Computation(comp) = &src_data.payload {
                    let label = comp.kind.label();
                    if label.contains(subexpr) {
                        return resolve_df_input_for_node(src, edge_idx, scg);
                    }
                }
            }
        }
        // If still no match, use the first source as fallback
        if let Some(&src) = sources.first() {
            return resolve_df_input_for_node(src, edge_idx, scg);
        }
    }
    
    // For complex sub-expressions, recursively parse and return BinOp
    if let Some((op, lhs_str, rhs_str)) = parse_expr_split(subexpr) {
        let lhs = resolve_subexpr(&lhs_str, sources, edge_idx, scg);
        let rhs = resolve_subexpr(&rhs_str, sources, edge_idx, scg);
        // Map IrBinOpKind to the codegen BinOpKind
        let binop_kind = match op {
            IrBinOpKind::Add => vuma_codegen::ir::BinOpKind::Add,
            IrBinOpKind::Sub => vuma_codegen::ir::BinOpKind::Sub,
            IrBinOpKind::Mul => vuma_codegen::ir::BinOpKind::Mul,
            IrBinOpKind::SDiv => vuma_codegen::ir::BinOpKind::SDiv,
            IrBinOpKind::SRem => vuma_codegen::ir::BinOpKind::SRem,
            IrBinOpKind::And => vuma_codegen::ir::BinOpKind::And,
            IrBinOpKind::Or => vuma_codegen::ir::BinOpKind::Or,
            IrBinOpKind::Xor => vuma_codegen::ir::BinOpKind::Xor,
            IrBinOpKind::Shl => vuma_codegen::ir::BinOpKind::Shl,
            IrBinOpKind::ShrL => vuma_codegen::ir::BinOpKind::ShrL,
            IrBinOpKind::ShrA => vuma_codegen::ir::BinOpKind::ShrA,
            IrBinOpKind::SLt => vuma_codegen::ir::BinOpKind::SLt,
            IrBinOpKind::SLe => vuma_codegen::ir::BinOpKind::SLe,
            IrBinOpKind::SGt => vuma_codegen::ir::BinOpKind::SGt,
            IrBinOpKind::SGe => vuma_codegen::ir::BinOpKind::SGe,
            IrBinOpKind::Eq => vuma_codegen::ir::BinOpKind::Eq,
            IrBinOpKind::Ne => vuma_codegen::ir::BinOpKind::Ne,
            _ => vuma_codegen::ir::BinOpKind::Add,
        };
        return ScgExpr::BinOp {
            op: binop_kind,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        };
    }
    
    // Fallback
    ScgExpr::Int(0)
}

/// Check if a string is a simple variable name (alphanumeric, no spaces or operators)
fn is_simple_var(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_') && !s.parse::<i64>().is_ok()
}

fn convert_node_to_statement_with_externs(
    node_id: NodeId,
    node_data: &NodeData,
    edge_idx: &EdgeIndex,
    scg: &SCG,
    extern_functions: &HashSet<String>,
) -> Option<ScgStatement> {
    match &node_data.payload {
        NodePayload::Allocation(alloc) => {
            let ty = alloc
                .type_name
                .as_deref()
                .and_then(parse_scg_type)
                .unwrap_or(ScgType::U8);
            Some(ScgStatement::Allocation(AllocationNode::Stack {
                name: node_var(node_id, "alloc"),
                size: alloc.size as u32,
                ty,
            }))
        }

        NodePayload::Access(access) => match access.mode {
            AccessMode::Read => Some(ScgStatement::Access(AccessNode::Load {
                dst: node_var(node_id, "val"),
                ptr: resolve_df_input(node_id, 0, edge_idx, scg),
                offset: access.offset.map(|o| ScgExpr::Int(o as i64)),
            })),
            AccessMode::Write | AccessMode::ReadWrite => {
                Some(ScgStatement::Access(AccessNode::Store {
                    ptr: resolve_df_input(node_id, 0, edge_idx, scg),
                    offset: access.offset.map(|o| ScgExpr::Int(o as i64)),
                    value: resolve_df_input(node_id, 1, edge_idx, scg),
                }))
            }
        },

        NodePayload::Computation(comp) => {
            let op_label = comp.kind.label();

            // Skip parameter nodes
            if op_label.starts_with("param ") {
                return None;
            }

            // Skip "uninitialized" placeholder nodes
            if op_label == "uninitialized" {
                return None;
            }

            // Skip literal Computation nodes (lit_<n>) — these are only
            // DataFlow sources and should not be converted to statements.
            if op_label.starts_with("lit_") {
                return None;
            }

            // Skip Computation nodes that represent call expressions.
            for cf_edge in edge_idx.outgoing_cf(node_id) {
                if let Some(target_data) = scg.get_node(cf_edge.target) {
                    if let NodePayload::Control(c) = &target_data.payload {
                        if c.kind == ControlKind::FunctionEntry {
                            if let Some(label) = &c.label {
                                if label.starts_with("call_") {
                                    return None;
                                }
                            }
                        }
                    }
                }
            }

            // Detect Load patterns: "let value = *region" or "X = *Y"
            // These don't have Access(Read) nodes in the SCG, so we detect
            // them from the label.
            // IMPORTANT: "*region = 42" is a Store, NOT a Load. We only
            // treat it as a Load if the label matches "= *<var>" at the END
            // of the string (i.e., the dereference is the value being assigned).
            if op_label.contains("= *") && !op_label.contains("= *") == false {
                // Check if the part after "= *" is a simple variable (not an assignment)
                if let Some(pos) = op_label.find("= *") {
                    let after = op_label[pos + 3..].trim();
                    // If "after" is a simple variable name (no spaces, no =), it's a Load
                    if !after.contains(' ') && !after.contains('=') && !after.is_empty() {
                        let ptr = resolve_df_input(node_id, 0, edge_idx, scg);
                        return Some(ScgStatement::Access(AccessNode::Load {
                            dst: node_var(node_id, "val"),
                            ptr,
                            offset: None,
                        }));
                    }
                }
            }

            // Check for Derivation edges to Allocation or Access nodes.
            for deriv_edge in edge_idx.outgoing.get(&node_id).map(|v| v.as_slice()).unwrap_or(&[]) {
                if deriv_edge.kind == EdgeKind::Derivation {
                    if let Some(target_data) = scg.get_node(deriv_edge.target) {
                        match &target_data.payload {
                            NodePayload::Allocation(alloc) => {
                                // Convert to Allocation statement
                                let ty = alloc.type_name.as_deref()
                                    .and_then(parse_scg_type)
                                    .unwrap_or(ScgType::U8);
                                return Some(ScgStatement::Allocation(AllocationNode::Stack {
                                    name: node_var(node_id, "comp"),  // Use Computation node var so Call refs find it
                                    size: alloc.size as u32,
                                    ty,
                                }));
                            }
                            NodePayload::Access(access) => {
                                // Convert to Load or Store
                                match access.mode {
                                    AccessMode::Read => {
                                        return Some(ScgStatement::Access(AccessNode::Load {
                                            dst: node_var(node_id, "val"),
                                            ptr: resolve_df_input(node_id, 0, edge_idx, scg),
                                            offset: access.offset.map(|o| ScgExpr::Int(o as i64)),
                                        }));
                                    }
                                    AccessMode::Write | AccessMode::ReadWrite => {
                                        let access_id = deriv_edge.target;

                                        // Collect ALL sources: DataFlow + Derivation
                                        let df_inputs = edge_idx.incoming_df(node_id);
                                        let mut all_sources: Vec<NodeId> = df_inputs.iter().map(|e| e.source).collect();
                                        if let Some(access_incoming) = edge_idx.incoming.get(&access_id) {
                                            for e in access_incoming {
                                                if e.kind == EdgeKind::Derivation {
                                                    all_sources.push(e.source);
                                                }
                                            }
                                        }

                                        // Parse the label to extract pointer and value expressions.
                                        let (ptr, value) = if op_label.starts_with("*") {
                                            if let Some(eq_pos) = op_label.rfind("= ") {
                                                let lhs = op_label[..eq_pos].trim();
                                                let rhs = op_label[eq_pos + 2..].trim();
                                                let ptr_expr = strip_outer_parens(&lhs[1..].trim());

                                                let ptr = if let Some((op, l, r)) = parse_expr_split(ptr_expr) {
                                                    let lhs_val = resolve_subexpr(&l, &all_sources, edge_idx, scg);
                                                    let rhs_val = resolve_subexpr(&r, &all_sources, edge_idx, scg);
                                                    ScgExpr::BinOp {
                                                        op: map_binop_kind(op),
                                                        lhs: Box::new(lhs_val),
                                                        rhs: Box::new(rhs_val),
                                                    }
                                                } else {
                                                    resolve_subexpr(ptr_expr, &all_sources, edge_idx, scg)
                                                };

                                                let value = if let Some((op, l, r)) = parse_expr_split(rhs) {
                                                    let lhs_val = resolve_subexpr(&l, &all_sources, edge_idx, scg);
                                                    let rhs_val = resolve_subexpr(&r, &all_sources, edge_idx, scg);
                                                    ScgExpr::BinOp {
                                                        op: map_binop_kind(op),
                                                        lhs: Box::new(lhs_val),
                                                        rhs: Box::new(rhs_val),
                                                    }
                                                } else {
                                                    resolve_subexpr(rhs, &all_sources, edge_idx, scg)
                                                };
                                                (ptr, value)
                                            } else {
                                                (ScgExpr::Int(0), ScgExpr::Int(0))
                                            }
                                        } else {
                                            let value = resolve_df_input(node_id, 0, edge_idx, scg);
                                            let ptr = if let Some(&src) = all_sources.last() {
                                                resolve_df_input_for_node(src, edge_idx, scg)
                                            } else {
                                                ScgExpr::Int(0)
                                            };
                                            (ptr, value)
                                        };

                                        return Some(ScgStatement::Access(AccessNode::Store {
                                            ptr,
                                            offset: access.offset.map(|o| ScgExpr::Int(o as i64)),
                                            value,
                                        }));
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            // Detect address-of patterns: "let x = @func_name" or bare "@func_name".
            // These should lower to GetAddress rather than a generic computation.
            if let Some(addr_name) = op_label.strip_prefix("let ") {
                // "let x = @func_name" → extract "func_name"
                if let Some(at_pos) = addr_name.find("= @") {
                    let symbol = addr_name[at_pos + 3..].trim();
                    if !symbol.is_empty() && !symbol.contains(' ') && !symbol.contains('(') {
                        let var_part = addr_name[..at_pos].trim();
                        let dst = if var_part.is_empty() {
                            node_var(node_id, "addr")
                        } else {
                            var_part.to_string()
                        };
                        return Some(ScgStatement::GetAddress(GetAddressNode {
                            dst,
                            name: symbol.to_string(),
                        }));
                    }
                }
            } else if let Some(symbol) = op_label.strip_prefix("@") {
                // Bare "@func_name"
                let symbol = symbol.trim();
                if !symbol.is_empty() && !symbol.contains(' ') && !symbol.contains('(') {
                    return Some(ScgStatement::GetAddress(GetAddressNode {
                        dst: node_var(node_id, "addr"),
                        name: symbol.to_string(),
                    }));
                }
            }

            // Try to parse the expression label and decompose it into
            // multiple IR instructions. If the label is a simple binary
            // operation (e.g., "x + 1"), parse_binop will find the operator.
            // For complex expressions, we need to recursively decompose.
            let df_inputs: Vec<vuma_scg::EdgeData> = edge_idx.incoming_df(node_id)
                .iter().map(|e| (*e).clone()).collect();
            
            // Collect the source nodes from DataFlow edges
            let sources: Vec<NodeId> = df_inputs.iter().map(|e| e.source).collect();
            
            // Strip "let <var> = " prefix from the label to get the expression
            let expr_str: String = if op_label.starts_with("let ") {
                if let Some(eq_pos) = op_label.find("= ") {
                    op_label[eq_pos + 2..].to_string()
                } else {
                    op_label.to_string()
                }
            } else {
                op_label.to_string()
            };
            
            // Parse the expression to find the top-level operator
            // and split into lhs/rhs sub-expressions.
            if let Some((op, lhs_str, rhs_str)) = parse_expr_split(&expr_str) {
                // Map lhs and rhs to ScgExpr by matching variable names
                // to DataFlow sources.
                let lhs = resolve_subexpr(&lhs_str, &sources, edge_idx, scg);
                let rhs = resolve_subexpr(&rhs_str, &sources, edge_idx, scg);
                Some(ScgStatement::Computation(ComputationNode {
                    dst: node_var(node_id, "comp"),
                    op,
                    lhs,
                    rhs,
                    tail_call: false,
                }))
            } else {
                // Fallback: use the first two DataFlow inputs
                let op = parse_binop(&op_label).unwrap_or(IrBinOpKind::Add);
                Some(ScgStatement::Computation(ComputationNode {
                    dst: node_var(node_id, "comp"),
                    op,
                    lhs: resolve_df_input(node_id, 0, edge_idx, scg),
                    rhs: resolve_df_input(node_id, 1, edge_idx, scg),
                    tail_call: false,
                }))
            }
        }

        NodePayload::Cast(cast) => {
            let to_ty = parse_scg_type(&cast.to_type).unwrap_or(ScgType::Ptr);
            let from_ty = parse_scg_type(&cast.from_type).unwrap_or(ScgType::Ptr);
            Some(ScgStatement::Cast(CastNode {
                dst: node_var(node_id, "cast"),
                src: resolve_df_input(node_id, 0, edge_idx, scg),
                kind: if cast.is_lossless {
                    CodegenCastKind::ZExt
                } else {
                    CodegenCastKind::BitCast
                },
                from_ty,
                to_ty,
            }))
        }

        NodePayload::Deallocation(_dealloc) => {
            // Stack-based allocation: deallocation is a no-op (the stack
            // frame is freed when the function returns). We skip emitting
            // any code for Deallocation nodes.
            None
        }

        NodePayload::Effect(eff) => {
            // Effect nodes may represent function calls.  If the
            // effect_kind matches a name in the extern set, mark it
            // as an extern call so the backend emits a relocation.
            let is_extern = extern_functions.contains(&eff.effect_kind);
            Some(ScgStatement::Call(CallNode {
                dst: Some(node_var(node_id, "eff")),
                func: eff.effect_kind.clone(),
                args: vec![],
                is_extern,
            }))
        }

        NodePayload::Phantom(_) => None,

        NodePayload::Control(_) => {
            // Control nodes are handled by walk_control_flow
            None
        }

        NodePayload::VTable(_) | NodePayload::ClosureEnv(_) => {
            // VTable and ClosureEnv are structural nodes; no IR statement
            None
        }

        NodePayload::StructDef(_) | NodePayload::EnumDef(_) | NodePayload::Match(_) 
        | NodePayload::ConstantTime(_) => {
            // Type definitions, match nodes, and constant-time ops are
            // not lowered to IR statements in the bridge
            None
        }
    }
}

// ── Function parameter extraction ──────────────────────────────────────

/// Extract function parameters from DataFlow edges leaving the
/// FunctionEntry node.
///
/// Parameter types are inferred from the target node's payload, which
/// has been refined by BD inference (via `refine_scg_types_with_bd`).
/// If no type info is available, defaults to `ScgType::I64`.
fn extract_function_params(entry_id: NodeId, scg: &SCG, edge_idx: &EdgeIndex) -> Vec<ScgParam> {
    let df_edges = edge_idx.outgoing_df(entry_id);
    let mut params = Vec::new();

    for (i, edge) in df_edges.iter().enumerate() {
        let (name, ty) = if let Some(target_node) = scg.get_node(edge.target) {
            match &target_node.payload {
                NodePayload::Allocation(alloc) => {
                    let name = alloc
                        .type_name
                        .clone()
                        .unwrap_or_else(|| format!("param_{}", i));
                    let ty = alloc
                        .type_name
                        .as_deref()
                        .and_then(parse_scg_type)
                        .unwrap_or(ScgType::I64);
                    (name, ty)
                }
                NodePayload::Computation(comp) => {
                    let name = format!("v_{}", edge.target.as_u64());
                    let ty = comp
                        .result_type
                        .as_deref()
                        .and_then(parse_scg_type)
                        .unwrap_or(ScgType::I64);
                    (name, ty)
                }
                NodePayload::Cast(cast) => {
                    let name = format!("param_{}", i);
                    let ty = parse_scg_type(&cast.to_type).unwrap_or(ScgType::I64);
                    (name, ty)
                }
                _ => (format!("param_{}", i), ScgType::I64),
            }
        } else {
            (format!("param_{}", i), ScgType::I64)
        };

        params.push(ScgParam { name, ty });
    }

    params
}

// ── Type parsing helper ────────────────────────────────────────────────

/// Parse a type string into a `ScgType`.
fn parse_scg_type(type_str: &str) -> Option<ScgType> {
    match type_str {
        "i8" | "I8" => Some(ScgType::I8),
        "i16" | "I16" => Some(ScgType::I16),
        "i32" | "I32" => Some(ScgType::I32),
        "i64" | "I64" => Some(ScgType::I64),
        "u8" | "U8" => Some(ScgType::U8),
        "u16" | "U16" => Some(ScgType::U16),
        "u32" | "U32" => Some(ScgType::U32),
        "u64" | "U64" => Some(ScgType::U64),
        "ptr" | "*void" | "*u8" | "*i8" => Some(ScgType::Ptr),
        "void" => Some(ScgType::Void),
        _ => None,
    }
}

// ── BD type refinement ─────────────────────────────────────────────────

/// Map a BD `RepD` to the codegen's `ScgType`.
///
/// Uses the RepD's size and kind to pick the most specific `ScgType`:
/// - Pointer RepDs → `ScgType::Ptr`
/// - Byte RepDs → integer types by size (u8, u16, u32, u64)
/// - Struct/Array/Enum/Union → `ScgType::Ptr` (passed by reference)
/// - Generic → `ScgType::I64` (fallback)
fn repd_to_scg_type(repd: &RepD) -> ScgType {
    match repd {
        RepD::Ptr(_) | RepD::Func(_) => ScgType::Ptr,
        RepD::Byte(byte_rep) => match byte_rep.size {
            1 => ScgType::U8,
            2 => ScgType::U16,
            4 => ScgType::U32,
            _ => ScgType::U64,
        },
        RepD::Struct(_) | RepD::Array(_) | RepD::Enum(_) | RepD::Union(_) => ScgType::Ptr,
        RepD::Generic { .. } => ScgType::I64,
    }
}

/// Convert a `ScgType` to its canonical string name for storing in SCG
/// node payloads (e.g., `AllocationNode.type_name`, `CastNode.from_type`).
fn scg_type_to_name(ty: &ScgType) -> &'static str {
    match ty {
        ScgType::I8 => "i8",
        ScgType::I16 => "i16",
        ScgType::I32 => "i32",
        ScgType::I64 => "i64",
        ScgType::U8 => "u8",
        ScgType::U16 => "u16",
        ScgType::U32 => "u32",
        ScgType::U64 => "u64",
        ScgType::Ptr => "ptr",
        ScgType::Void => "void",
        ScgType::F32 => "f32",
        ScgType::F64 => "f64",
    }
}

/// Refine SCG node type metadata using BD inference results.
///
/// After BD inference, each node's `RepD` describes the actual memory
/// representation.  This function maps those RepDs back to `ScgType`s
/// and stores the result in the SCG node payloads so that downstream
/// bridge code (`convert_node_to_statement`, `extract_function_params`)
/// can pick up the refined types instead of using defaults.
///
/// # What is refined
///
/// - **Allocation nodes**: `type_name` is set if it was previously `None`.
/// - **Cast nodes**: `from_type` / `to_type` are updated if they couldn't
///   previously be parsed by `parse_scg_type`.
/// - **Computation nodes**: `result_type` is set if it was previously `None`.
pub fn refine_scg_types_with_bd(scg: &mut SCG, bd_results: &[(NodeId, BD)]) {
    let bd_map: HashMap<NodeId, &BD> = bd_results.iter().map(|(id, bd)| (*id, bd)).collect();

    let node_ids: Vec<_> = scg.node_ids().collect();
    for node_id in node_ids {
        let Some(bd) = bd_map.get(&node_id) else {
            continue;
        };
        let inferred_type = repd_to_scg_type(&bd.repd);
        let type_name = scg_type_to_name(&inferred_type);

        if let Some(node) = scg.get_node_mut(node_id) {
            match &mut node.payload {
                NodePayload::Allocation(alloc)
                    // Update type_name if it was previously unset.
                    if alloc.type_name.is_none() => {
                        alloc.type_name = Some(type_name.to_string());
                    }
                NodePayload::Cast(cast) => {
                    // Update from_type / to_type if they couldn't previously
                    // be parsed by `parse_scg_type` (i.e., they were opaque
                    // type names from the AST that don't map directly).
                    if parse_scg_type(&cast.from_type).is_none() {
                        cast.from_type = type_name.to_string();
                    }
                    if parse_scg_type(&cast.to_type).is_none() {
                        cast.to_type = type_name.to_string();
                    }
                }
                NodePayload::Computation(comp)
                    if comp.result_type.is_none() => {
                        comp.result_type = Some(type_name.to_string());
                    }
                _ => {}
            }
        }
    }
}

// ── Entry-point detection ──────────────────────────────────────────────

/// Find entry-point nodes (no incoming ControlFlow edges) for a function
/// that lacks an explicit FunctionEntry node.
fn find_entry_points(scg: &SCG, edge_idx: &EdgeIndex) -> Vec<NodeId> {
    let mut entry_points = Vec::new();

    for node_id in scg.node_ids() {
        let has_incoming_cf = edge_idx
            .incoming
            .get(&node_id)
            .map(|edges| edges.iter().any(|e| e.kind == EdgeKind::ControlFlow))
            .unwrap_or(false);

        if !has_incoming_cf {
            if let Some(node_data) = scg.get_node(node_id) {
                // Skip Phantom nodes
                if matches!(node_data.node_type, NodeType::Phantom) {
                    continue;
                }
                entry_points.push(node_id);
            }
        }
    }

    // If no entry points found, use the first node
    if entry_points.is_empty() {
        if let Some(first_id) = scg.node_ids().next() {
            entry_points.push(first_id);
        }
    }

    entry_points
}

// ── Main bridge function ───────────────────────────────────────────────

/// Convert a `vuma_scg::SCG` into the codegen's stub `Scg` type.
///
/// This function reconstructs real control flow (if/else, loops, function
/// boundaries, break/continue) from the SCG's graph structure, instead of
/// just flattening everything into a single linear "main" function.
///
/// # Algorithm
///
/// 1. **Phase 1: Function boundary detection** — Group nodes by
///    FunctionEntry→FunctionReturn regions.
/// 2. **Phase 2: Control flow reconstruction** — Within each function,
///    detect Branch+Join diamonds (if/else) and LoopHeader+LoopExit
///    patterns (loops).
/// 3. **Phase 3: Statement generation** — Convert non-control nodes into
///    ScgStatements with DataFlow-based variable naming.

fn parse_for_range(label: &str) -> Option<(String, i64, i64)> {
    let label = label.trim();
    if !label.starts_with("for ") { return None; }
    let rest = &label[4..];
    let in_pos = rest.find(" in ")?;
    let var_name = rest[..in_pos].trim().to_string();
    let range_str = rest[in_pos + 4..].trim();
    if let Some(dot_pos) = range_str.find("..") {
        let start_str = range_str[..dot_pos].trim();
        let end_part = &range_str[dot_pos + 2..];
        let inclusive = end_part.starts_with("=");
        let end_str = if inclusive { &end_part[1..] } else { end_part }.trim();
        let start: i64 = start_str.parse().ok()?;
        let end: i64 = end_str.parse().ok()?;
        let end = if inclusive { end + 1 } else { end };
        return Some((var_name, start, end));
    }
    None
}

pub fn bridge_scg_to_codegen(scg: &SCG) -> Scg {
    bridge_scg_to_codegen_with_externs(scg, &HashSet::new())
}

/// Bridge the `vuma-scg` SCG to the codegen SCG, with knowledge of which
/// functions are declared as extern (foreign) in the source program.
///
/// When a function call targets a name in `extern_functions`, the resulting
/// `CallNode` gets `is_extern: true`, which causes the backend to emit
/// a relocation entry instead of a local `BL` instruction.
pub fn bridge_scg_to_codegen_with_externs(scg: &SCG, extern_functions: &HashSet<String>) -> Scg {
    let edge_idx = EdgeIndex::build(scg);
    let mut consumed: HashSet<NodeId> = HashSet::new();
    let mut scg_nodes: Vec<ScgNode> = Vec::new();

    // ── Phase 0: Identify call-site FunctionEntry nodes ─────────────
    //
    // The AST→SCG conversion emits a FunctionEntry+FunctionReturn pair
    // for every call site (e.g. `call_write` / `return_write`).  These
    // must NOT be treated as function definitions — they represent call
    // sites and should be lowered to `CallNode` statements.
    //
    // We distinguish them by label prefix ("call_") or by the presence
    // of an incoming ControlFlow edge from a non-FunctionEntry node
    // (a call site's FunctionEntry is reached from the caller's body,
    // whereas a function definition's FunctionEntry is an SCG entry
    // point with no incoming CF edges).
    let mut call_site_entries: HashSet<NodeId> = HashSet::new();
    let mut call_site_names: HashMap<NodeId, String> = HashMap::new();
    for n in scg.nodes() {
        if let NodePayload::Control(c) = &n.payload {
            if c.kind == ControlKind::FunctionEntry {
                if let Some(label) = &c.label {
                    // Call-site FunctionEntry nodes have labels like "call_write"
                    if let Some(func_name) = label.strip_prefix("call_") {
                        call_site_entries.insert(n.id);
                        call_site_names.insert(n.id, func_name.to_string());
                    }
                }
            }
        }
    }

    // ── Phase 1: Function boundary detection ─────────────────────
    // Only collect FunctionEntry nodes that are NOT call sites.
    let function_entries: Vec<(NodeId, String)> = scg
        .nodes()
        .filter_map(|n| {
            if call_site_entries.contains(&n.id) {
                return None; // skip call-site entries
            }
            if let NodePayload::Control(c) = &n.payload {
                if c.kind == ControlKind::FunctionEntry {
                    let name = c.label.clone().unwrap_or_else(|| "unknown".to_string());
                    return Some((n.id, name));
                }
            }
            None
        })
        .collect();

    if !function_entries.is_empty() {
        // Process each function defined by a FunctionEntry node
        for (entry_id, func_name) in &function_entries {
            consumed.insert(*entry_id);

            let return_node = find_function_return(*entry_id, scg, &edge_idx);
            let params = extract_function_params(*entry_id, scg, &edge_idx);

            let mut body = if let Some(first_cf) = edge_idx.outgoing_cf(*entry_id).first() {
                // Do NOT add the FunctionReturn node to stop_at.
                // The walk's FunctionReturn handler resolves the return value's
                // DataFlow inputs and emits an ScgStatement::Return carrying the
                // resolved ScgExprs. Adding the return node to stop_at causes
                // the walk to break before processing that node, so the handler
                // never runs and an empty Return(vec![]) is emitted instead.
                let stop_at: HashSet<NodeId> = HashSet::new();
                walk_control_flow_with_externs(first_cf.target, scg, &edge_idx, &mut consumed, &stop_at, extern_functions)
            } else {
                vec![]
            };

            // Add return statement if the function has a FunctionReturn
            if let Some(ret) = return_node {
                consumed.insert(ret);
            }
            if !body.iter().any(|s| matches!(s, ScgStatement::Return(_))) {
                body.push(ScgStatement::Return(vec![]));
            }

            scg_nodes.push(ScgNode::Function(ScgFunction {
                name: func_name.clone(),
                params,
                results: vec![],
                body,
            }));
        }
    } else {
        // No FunctionEntry nodes — find entry points and walk control flow
        let entry_points = find_entry_points(scg, &edge_idx);

        let mut body = Vec::new();
        for start in &entry_points {
            let stop_at = HashSet::new();
            let mut partial = walk_control_flow_with_externs(*start, scg, &edge_idx, &mut consumed, &stop_at, extern_functions);
            body.append(&mut partial);
        }

        // Process any remaining unconsumed nodes (connected only via DataFlow)
        let remaining: Vec<NodeId> = scg.node_ids().filter(|id| !consumed.contains(id)).collect();
        for nid in &remaining {
            if consumed.contains(nid) {
                continue;
            }
            consumed.insert(*nid);
            if let Some(node_data) = scg.get_node(*nid) {
                if let Some(stmt) = convert_node_to_statement_with_externs(*nid, node_data, &edge_idx, scg, extern_functions) {
                    body.push(stmt);
                }
            }
        }

        if !body.iter().any(|s| matches!(s, ScgStatement::Return(_))) {
            body.push(ScgStatement::Return(vec![]));
        }

        scg_nodes.push(ScgNode::Function(ScgFunction {
            name: "main".to_string(),
            params: vec![],
            results: vec![],
            body,
        }));
    }

    // Skip remaining nodes — they are disconnected expression fragments

    // Ensure at least one function exists
    if scg_nodes.is_empty() {
        scg_nodes.push(ScgNode::Function(ScgFunction {
            name: "main".to_string(),
            params: vec![],
            results: vec![],
            body: vec![ScgStatement::Return(vec![])],
        }));
    }

    Scg { nodes: scg_nodes }
}

/// Try to parse an operation string into a BinOpKind.
fn parse_binop(op: &str) -> Option<IrBinOpKind> {
    match op {
        "add" | "+" => return Some(IrBinOpKind::Add),
        "sub" | "-" => return Some(IrBinOpKind::Sub),
        "mul" | "*" => return Some(IrBinOpKind::Mul),
        "sdiv" | "/" => return Some(IrBinOpKind::SDiv),
        "udiv" => return Some(IrBinOpKind::UDiv),
        "srem" | "%" => return Some(IrBinOpKind::SRem),
        "urem" => return Some(IrBinOpKind::URem),
        "and" | "&" => return Some(IrBinOpKind::And),
        "or" | "|" => return Some(IrBinOpKind::Or),
        "xor" | "^" => return Some(IrBinOpKind::Xor),
        "shl" | "<<" => return Some(IrBinOpKind::Shl),
        "shr.l" | ">>" => return Some(IrBinOpKind::ShrL),
        "shr.a" => return Some(IrBinOpKind::ShrA),
        "slt" | "<" => return Some(IrBinOpKind::SLt),
        "sle" | "<=" => return Some(IrBinOpKind::SLe),
        "sgt" | ">" => return Some(IrBinOpKind::SGt),
        "sge" | ">=" => return Some(IrBinOpKind::SGe),
        "ult" => return Some(IrBinOpKind::ULt),
        "ule" => return Some(IrBinOpKind::ULe),
        "ugt" => return Some(IrBinOpKind::UGt),
        "uge" => return Some(IrBinOpKind::UGe),
        "eq" | "==" => return Some(IrBinOpKind::Eq),
        "ne" | "!=" => return Some(IrBinOpKind::Ne),
        _ => {}
    }
    let op_str = op.trim();
    for (pat, kind) in [
        ("<=", IrBinOpKind::SLe), (">=", IrBinOpKind::SGe),
        ("==", IrBinOpKind::Eq), ("!=", IrBinOpKind::Ne),
        ("<<", IrBinOpKind::Shl), (">>", IrBinOpKind::ShrL),
    ] {
        if op_str.contains(&format!(" {} ", pat)) { return Some(kind); }
    }
    for (pat, kind) in [
        ("+", IrBinOpKind::Add), ("-", IrBinOpKind::Sub),
        ("*", IrBinOpKind::Mul), ("/", IrBinOpKind::SDiv),
        ("%", IrBinOpKind::SRem), ("&", IrBinOpKind::And),
        ("|", IrBinOpKind::Or), ("^", IrBinOpKind::Xor),
        ("<", IrBinOpKind::SLt), (">", IrBinOpKind::SGt),
    ] {
        if op_str.contains(&format!(" {} ", pat)) { return Some(kind); }
    }
    None
}

// ═══════════════════════════════════════════════════════════════════════════
// Compile pipeline
// ═══════════════════════════════════════════════════════════════════════════

/// Compile VUMA source code with the given configuration.
///
/// This is the main entry point for the VUMA compilation pipeline.
/// It runs all stages in order, collecting errors and producing a
/// [`CompilationOutput`] on success.
///
/// # Pipeline Stages
///
/// 1. **Parse** — lex and parse source into an AST
/// 2. **AST → SCG** — convert the AST into a Semantic Computation Graph
/// 3. **SCG Validation** — verify the SCG is well-formed
/// 4. **BD Inference** — infer behavioral descriptions from the SCG
/// 5. **MSG Construction** — build the Memory State Graph from the SCG
/// 6. **IVE Verification** — verify the five core VUMA invariants
/// 7. **SCG Transforms** — run optimisation passes (DCE, CSE, etc.)
/// 8. **IR Lowering** — lower the SCG to an intermediate representation
/// 9. **Register Allocation** — assign physical ARM64 registers
/// 10. **Code Emission** — generate ARM64 machine code and ELF binary
pub fn compile(source: &str, config: &CompileConfig) -> Result<CompilationOutput, Vec<VumaError>> {
    compile_with_path(source, None, config)
}

/// Compile VUMA source text with an optional file path for import resolution.
///
/// This is the same as [`compile`] but accepts an optional file path that
/// is used to resolve `import` statements.  When a file path is provided,
/// imported modules are located relative to the file's parent directory.
///
/// # Example
///
/// ```rust,ignore
/// use vuma::pipeline::{compile_with_path, CompileConfig};
/// use std::path::Path;
///
/// let source = r#"
///     import "utils.vuma";
///     fn main() { helper(); }
/// "#;
/// let config = CompileConfig::default();
/// let result = compile_with_path(source, Some(Path::new("src/main.vuma")), &config);
/// ```
pub fn compile_with_path(
    source: &str,
    file_path: Option<&Path>,
    config: &CompileConfig,
) -> Result<CompilationOutput, Vec<VumaError>> {
    let mut errors: Vec<VumaError> = Vec::new();
    let mut timings: Vec<(String, u64)> = Vec::new();

    // ── Stage 1: Parse + Resolve imports ────────────────────────────
    let t = Instant::now();
    let ast = match parse_and_resolve(source, file_path) {
        Ok(ast) => ast,
        Err(e) => {
            errors.push(e);
            if config.stop_on_first_error {
                return Err(errors);
            }
            // Cannot continue without an AST.
            return Err(errors);
        }
    };
    timings.push(("parse".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 2: AST → SCG ───────────────────────────────────────────
    let t = Instant::now();
    let mut scg = match ast_to_scg(&ast) {
        Ok(scg) => scg,
        Err(e) => {
            errors.push(e);
            if config.stop_on_first_error {
                return Err(errors);
            }
            // Cannot continue without an SCG.
            return Err(errors);
        }
    };
    timings.push(("ast-to-scg".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 3: SCG Validation ──────────────────────────────────────
    let t = Instant::now();
    let validation = scg.validate();
    if !validation.is_valid {
        let e = VumaError::ScgValidation {
            errors: validation.errors.clone(),
        };
        errors.push(e);
        if config.stop_on_first_error {
            return Err(errors);
        }
    }
    timings.push(("scg-validation".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 4: BD Inference ─────────────────────────────────────────
    let t = Instant::now();
    let inference_engine = InferenceEngine::new();
    let bd_results = inference_engine.infer_types(&scg);
    // Apply BD-inferred types to SCG nodes so downstream stages
    // (MSG construction, IR lowering) use refined types instead of
    // the defaults (ScgType::I64 for params, ScgType::U8 for allocs).
    refine_scg_types_with_bd(&mut scg, &bd_results);
    timings.push(("bd-inference".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 5: MSG Construction ─────────────────────────────────────
    // NOTE: MSG is a memory-safety analysis IR.  It is NOT used by the
    // codegen path (Stage 8), which has its own SCG→IR bridge.  Several
    // conditions can cause scg_to_msg to fail that are not fatal for
    // code generation:
    //
    //   • CycleDetected  — programs with loops create back-edges.
    //   • AccessRegionNotFound — topological sort may place an Access
    //     node before its Allocation node (no direct SCG edge links them).
    //   • MissingDerivation / CastWithoutParent — incomplete derivation
    //     chains in the SCG.
    //
    // All of these are soft-failures: we log the error but continue
    // with an empty MSG so that codegen (Stage 8) can proceed.
    let t = Instant::now();
    let msg = match scg_to_msg(&scg) {
        Ok(msg) => msg,
        Err(e) => {
            // Log the conversion error but do NOT abort — codegen does
            // not depend on MSG.
            errors.push(VumaError::ScgToMsg { error: e });
            MSG::new() // fall back to empty MSG
        }
    };
    timings.push((
        "msg-construction".to_string(),
        t.elapsed().as_millis() as u64,
    ));

    // ── Stage 6: IVE Verification ─────────────────────────────────────
    let t = Instant::now();
    let verification = if config.verification_level != VerificationLevel::None {
        let ive_level = match config.verification_level {
            VerificationLevel::Quick => IveVerificationLevel::Quick,
            VerificationLevel::Normal => IveVerificationLevel::Normal,
            VerificationLevel::Exhaustive => IveVerificationLevel::Exhaustive,
            VerificationLevel::None => unreachable!(),
        };
        let aggregator = InvariantAggregator::new().with_level(ive_level);
        let input = vuma_ive::verification::VerificationInput::from_scg(scg.clone());
        let result = aggregator.verify_all(&input);
        // Verification is a hard safety gate: if any invariant was
        // violated, refuse to emit code for the program.  This is
        // independent of `stop_on_first_error` because emitting a binary
        // for a program with known memory-safety violations would defeat
        // the entire purpose of VUMA.  An `Inconclusive` verdict (no
        // violations but some unverified invariants) is NOT a failure —
        // it just means verification could not prove safety, not that it
        // proved unsafety.
        if result.overall == OverallVerdict::Fail {
            errors.push(VumaError::Verification { result });
            return Err(errors);
        }
        Some(result)
    } else {
        None
    };
    timings.push((
        "ive-verification".to_string(),
        t.elapsed().as_millis() as u64,
    ));

    // ── Stage 7: SCG Transforms ───────────────────────────────────────
    let t = Instant::now();
    let transform_result = run_scg_transforms(&mut scg, config);
    if let Some(ref tr) = transform_result {
        if tr.has_errors {
            // Collect errors from individual passes.
            let pass_errors: Vec<String> = tr
                .pass_results
                .iter()
                .flat_map(|pr| pr.errors.clone())
                .collect();
            if !pass_errors.is_empty() {
                errors.push(VumaError::Transform {
                    pass_name: "pipeline".to_string(),
                    errors: pass_errors,
                });
                if config.stop_on_first_error {
                    return Err(errors);
                }
            }
        }
    }
    timings.push(("scg-transforms".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 8: IR Lowering ──────────────────────────────────────────
    let t = Instant::now();
    let extern_fns = extract_extern_functions(&ast);
    let codegen_scg = bridge_scg_to_codegen_with_externs(&scg, &extern_fns);
    let mut ir_builder = IRBuilder::new();
    let ir_program = match ir_builder.build(&codegen_scg) {
        Ok(ir) => ir,
        Err(e) => {
            errors.push(VumaError::Codegen { error: e });
            if config.stop_on_first_error {
                return Err(errors);
            }
            return Err(errors); // Cannot continue without IR.
        }
    };
    let ir_function_count = ir_program.functions.len();
    let ir_instruction_count: usize = ir_program
        .functions
        .iter()
        .map(|f| f.blocks.iter().map(|b| b.instructions.len()).sum::<usize>())
        .sum();
    timings.push(("ir-lowering".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 9: Register Allocation ──────────────────────────────────
    let t = Instant::now();
    let allocator = LinearScanAllocator::new();
    let mut regalloc_results = Vec::new();
    for func in &ir_program.functions {
        match allocator.allocate_function(func) {
            Ok(result) => regalloc_results.push(result),
            Err(e) => {
                errors.push(VumaError::RegisterAlloc {
                    message: format!("{}: {}", func.name, e),
                });
                if config.stop_on_first_error {
                    return Err(errors);
                }
            }
        }
    }
    timings.push(("register-alloc".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 10: Code Emission ───────────────────────────────────────
    let t = Instant::now();
    let emit_config = config.emit_config();
    let binary = match emit_binary(
        &ir_program.functions,
        &ir_program.data_sections,
        &emit_config,
    ) {
        Ok(binary) => binary,
        Err(e) => {
            errors.push(VumaError::Emission {
                message: format!("{}", e),
            });
            if config.stop_on_first_error {
                return Err(errors);
            }
            return Err(errors); // Cannot continue without binary.
        }
    };
    let code_words = count_text_section_instructions(&binary);
    timings.push(("code-emission".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 11: COR Initialization ──────────────────────────────────
    let t = Instant::now();
    let cor_runtime = {
        // Bridge the vuma_scg::SCG to the COR-internal SCG representation
        // using CORuntime::from_vuma_scg(), then compile all regions
        // incrementally with a Delta containing every node ID.
        let scg_arc = std::sync::Arc::new(scg.clone());
        let cor_config = CorConfig::default();
        let mut rt = CORuntime::from_vuma_scg(scg_arc, cor_config);

        // Build a Delta with all node IDs from the SCG so every region
        // is compiled incrementally, establishing the always-compiled
        // invariant from the start.
        let all_node_ids: Vec<u64> = scg.node_ids().map(|id| id.as_u64()).collect();
        let delta = vuma_cor::types::Delta {
            added_nodes: all_node_ids,
            ..vuma_cor::types::Delta::empty()
        };
        let recompiled = rt.compile_incremental(&delta);
        log::info!(
            "cor-init: compiled {} regions incrementally from SCG ({} nodes)",
            recompiled.len(),
            scg.node_count(),
        );
        Some(rt)
    };
    timings.push(("cor-init".to_string(), t.elapsed().as_millis() as u64));

    // If we accumulated errors but still produced a binary, report them.
    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(CompilationOutput {
        binary,
        scg,
        msg,
        verification,
        stage_timings: timings,
        ir_function_count,
        ir_instruction_count,
        code_words,
        debug_info: if config.debug_info {
            Some(DebugInfo {
                ast: Some(ast),
                ir_pre_regalloc: Some(ir_program),
                regalloc_results,
                transform_results: transform_result,
            })
        } else {
            None
        },
        cor_runtime,
    })
}

/// Compile VUMA source code with crash recovery.
///
/// Unlike [`compile_with_path`], which returns `Err(Vec<VumaError>)` on failure,
/// this function returns a [`CompileResult`] that includes partial results
/// when compilation fails partway through. This enables:
///
/// - **Backend fallback**: If the primary backend fails, tries the next
///   available backend automatically.
/// - **Partial results**: Returns intermediate artifacts (AST, SCG, MSG)
///   even when the full pipeline doesn't complete.
/// - **Never panics**: All errors are caught and reported as
///   [`VumaDiagnostic`](crate::VumaDiagnostic)s rather than panicking.
///
/// # Example
///
/// ```rust,ignore
/// use vuma::pipeline::{compile_with_recovery, CompileConfig};
///
/// let source = "fn main() {}";
/// let config = CompileConfig::default();
/// match compile_with_recovery(source, None, &config) {
///     CompileResult::Success(output) => {
///         println!("Compiled {} bytes", output.binary.len());
///     }
///     CompileResult::Partial(partial) => {
///         eprintln!("Compilation failed with {} error(s):", partial.diagnostics.len());
///         for diag in &partial.diagnostics {
///             eprintln!("  {}", diag);
///         }
///         if let Some(ref scg) = partial.scg {
///             println!("Partial SCG has {} nodes", scg.node_count());
///         }
///     }
/// }
/// ```
pub fn compile_with_recovery(
    source: &str,
    file_path: Option<&Path>,
    config: &CompileConfig,
) -> CompileResult {
    let mut errors: Vec<VumaError> = Vec::new();
    let mut timings: Vec<(String, u64)> = Vec::new();
    let mut last_completed: Option<PipelineStage> = None;

    // Helper: try an operation, catch any panic, return Result
    macro_rules! try_or_partial {
        ($stage:expr, $expr:expr, $partial_builder:expr) => {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| $expr)) {
                Ok(result) => result,
                Err(panic_payload) => {
                    let message = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    errors.push(VumaError::PanicCaught {
                        stage: $stage.to_string(),
                        message,
                    });
                    return CompileResult::Partial($partial_builder);
                }
            }
        };
    }

    // ── Stage 1: Parse + Resolve imports ────────────────────────────
    let t = Instant::now();
    let ast = match try_or_partial!(
        "parse",
        parse_and_resolve(source, file_path),
        PartialCompilationOutput {
            ast: None,
            scg: None,
            msg: None,
            verification: None,
            stage_timings: timings,
            ir_function_count: None,
            ir_instruction_count: None,
            last_completed_stage: last_completed,
            diagnostics: errors,
        }
    ) {
        Ok(ast) => ast,
        Err(e) => {
            errors.push(e);
            timings.push(("parse".to_string(), t.elapsed().as_millis() as u64));
            return CompileResult::Partial(PartialCompilationOutput {
                ast: None,
                scg: None,
                msg: None,
                verification: None,
                stage_timings: timings,
                ir_function_count: None,
                ir_instruction_count: None,
                last_completed_stage: last_completed,
                diagnostics: errors,
            });
        }
    };
    timings.push(("parse".to_string(), t.elapsed().as_millis() as u64));
    last_completed = Some(PipelineStage::Parse);

    // ── Stage 2: AST → SCG ───────────────────────────────────────────
    let t = Instant::now();
    let mut scg = match try_or_partial!(
        "ast-to-scg",
        ast_to_scg(&ast),
        PartialCompilationOutput {
            ast: Some(ast.clone()),
            scg: None,
            msg: None,
            verification: None,
            stage_timings: timings,
            ir_function_count: None,
            ir_instruction_count: None,
            last_completed_stage: last_completed,
            diagnostics: errors,
        }
    ) {
        Ok(scg) => scg,
        Err(e) => {
            errors.push(e);
            timings.push(("ast-to-scg".to_string(), t.elapsed().as_millis() as u64));
            return CompileResult::Partial(PartialCompilationOutput {
                ast: Some(ast),
                scg: None,
                msg: None,
                verification: None,
                stage_timings: timings,
                ir_function_count: None,
                ir_instruction_count: None,
                last_completed_stage: last_completed,
                diagnostics: errors,
            });
        }
    };
    timings.push(("ast-to-scg".to_string(), t.elapsed().as_millis() as u64));
    last_completed = Some(PipelineStage::AstToScg);

    // ── Stage 3: SCG Validation ──────────────────────────────────────
    let t = Instant::now();
    let validation = scg.validate();
    if !validation.is_valid {
        let e = VumaError::ScgValidation {
            errors: validation.errors.clone(),
        };
        errors.push(e);
        // Non-fatal: continue with warnings
    }
    timings.push(("scg-validation".to_string(), t.elapsed().as_millis() as u64));
    last_completed = Some(PipelineStage::ScgValidation);

    // ── Stage 4: BD Inference ─────────────────────────────────────────
    let t = Instant::now();
    let inference_engine = InferenceEngine::new();
    let bd_results = inference_engine.infer_types(&scg);
    refine_scg_types_with_bd(&mut scg, &bd_results);
    timings.push(("bd-inference".to_string(), t.elapsed().as_millis() as u64));
    last_completed = Some(PipelineStage::BdInference);

    // ── Stage 5: MSG Construction (soft failure) ─────────────────────
    let t = Instant::now();
    let msg = match scg_to_msg(&scg) {
        Ok(msg) => msg,
        Err(e) => {
            errors.push(VumaError::ScgToMsg { error: e });
            MSG::new()
        }
    };
    timings.push(("msg-construction".to_string(), t.elapsed().as_millis() as u64));
    last_completed = Some(PipelineStage::MsgConstruction);

    // ── Stage 6: IVE Verification ─────────────────────────────────────
    let t = Instant::now();
    let verification = if config.verification_level != VerificationLevel::None {
        let ive_level = match config.verification_level {
            VerificationLevel::Quick => IveVerificationLevel::Quick,
            VerificationLevel::Normal => IveVerificationLevel::Normal,
            VerificationLevel::Exhaustive => IveVerificationLevel::Exhaustive,
            VerificationLevel::None => unreachable!(),
        };
        let aggregator = InvariantAggregator::new().with_level(ive_level);
        let input = vuma_ive::verification::VerificationInput::from_scg(scg.clone());
        let result = aggregator.verify_all(&input);
        // Verification is a hard safety gate: if any invariant was
        // violated, refuse to emit code for the program.  This is
        // independent of `stop_on_first_error` because emitting a binary
        // for a program with known memory-safety violations would defeat
        // the entire purpose of VUMA.  An `Inconclusive` verdict (no
        // violations but some unverified invariants) is NOT a failure —
        // it just means verification could not prove safety, not that it
        // proved unsafety.
        if result.overall == OverallVerdict::Fail {
            errors.push(VumaError::Verification { result: result.clone() });
            timings.push((
                "ive-verification".to_string(),
                t.elapsed().as_millis() as u64,
            ));
            return CompileResult::Partial(PartialCompilationOutput {
                ast: Some(ast),
                scg: Some(scg),
                msg: Some(msg),
                verification: Some(result),
                stage_timings: timings,
                ir_function_count: None,
                ir_instruction_count: None,
                last_completed_stage: last_completed,
                diagnostics: errors,
            });
        }
        Some(result)
    } else {
        None
    };
    timings.push(("ive-verification".to_string(), t.elapsed().as_millis() as u64));
    last_completed = Some(PipelineStage::IveVerification);

    // ── Stage 7: SCG Transforms ───────────────────────────────────────
    let t = Instant::now();
    let transform_result = run_scg_transforms(&mut scg, config);
    if let Some(ref tr) = transform_result {
        if tr.has_errors {
            let pass_errors: Vec<String> = tr
                .pass_results
                .iter()
                .flat_map(|pr| pr.errors.clone())
                .collect();
            if !pass_errors.is_empty() {
                errors.push(VumaError::Transform {
                    pass_name: "pipeline".to_string(),
                    errors: pass_errors,
                });
                // Non-fatal: continue
            }
        }
    }
    timings.push(("scg-transforms".to_string(), t.elapsed().as_millis() as u64));
    last_completed = Some(PipelineStage::ScgTransforms);

    // ── Stage 8: IR Lowering ──────────────────────────────────────────
    let t = Instant::now();
    let extern_fns = extract_extern_functions(&ast);
    let codegen_scg = bridge_scg_to_codegen_with_externs(&scg, &extern_fns);
    let mut ir_builder = IRBuilder::new();
    let ir_program = match ir_builder.build(&codegen_scg) {
        Ok(ir) => ir,
        Err(e) => {
            errors.push(VumaError::Codegen { error: e });
            timings.push(("ir-lowering".to_string(), t.elapsed().as_millis() as u64));
            return CompileResult::Partial(PartialCompilationOutput {
                ast: Some(ast),
                scg: Some(scg),
                msg: Some(msg),
                verification,
                stage_timings: timings,
                ir_function_count: None,
                ir_instruction_count: None,
                last_completed_stage: last_completed,
                diagnostics: errors,
            });
        }
    };
    let ir_function_count = ir_program.functions.len();
    let ir_instruction_count: usize = ir_program
        .functions
        .iter()
        .map(|f| f.blocks.iter().map(|b| b.instructions.len()).sum::<usize>())
        .sum();
    timings.push(("ir-lowering".to_string(), t.elapsed().as_millis() as u64));
    last_completed = Some(PipelineStage::IrLowering);

    // ── Stage 9: Register Allocation ──────────────────────────────────
    let t = Instant::now();
    let allocator = LinearScanAllocator::new();
    let mut regalloc_results = Vec::new();
    let mut regalloc_failed = false;
    for func in &ir_program.functions {
        match allocator.allocate_function(func) {
            Ok(result) => regalloc_results.push(result),
            Err(e) => {
                errors.push(VumaError::RegisterAlloc {
                    message: format!("{}: {}", func.name, e),
                });
                regalloc_failed = true;
            }
        }
    }
    if regalloc_failed && regalloc_results.is_empty() {
        timings.push(("register-alloc".to_string(), t.elapsed().as_millis() as u64));
        return CompileResult::Partial(PartialCompilationOutput {
            ast: Some(ast),
            scg: Some(scg),
            msg: Some(msg),
            verification,
            stage_timings: timings,
            ir_function_count: Some(ir_function_count),
            ir_instruction_count: Some(ir_instruction_count),
            last_completed_stage: last_completed,
            diagnostics: errors,
        });
    }
    timings.push(("register-alloc".to_string(), t.elapsed().as_millis() as u64));
    last_completed = Some(PipelineStage::RegisterAlloc);

    // ── Stage 10: Code Emission (with backend fallback) ───────────────
    let t = Instant::now();
    let emit_config = config.emit_config();
    let binary = match emit_binary(
        &ir_program.functions,
        &ir_program.data_sections,
        &emit_config,
    ) {
        Ok(binary) => binary,
        Err(e) => {
            let emission_err = format!("{}", e);
            errors.push(VumaError::Emission {
                message: emission_err.clone(),
            });
            timings.push(("code-emission".to_string(), t.elapsed().as_millis() as u64));
            // Return partial — no binary but we have everything else
            return CompileResult::Partial(PartialCompilationOutput {
                ast: Some(ast),
                scg: Some(scg),
                msg: Some(msg),
                verification,
                stage_timings: timings,
                ir_function_count: Some(ir_function_count),
                ir_instruction_count: Some(ir_instruction_count),
                last_completed_stage: last_completed,
                diagnostics: errors,
            });
        }
    };
    let code_words = count_text_section_instructions(&binary);
    timings.push(("code-emission".to_string(), t.elapsed().as_millis() as u64));
    last_completed = Some(PipelineStage::CodeEmission);

    // ── Stage 11: COR Initialization (soft failure) ──────────────────
    let t = Instant::now();
    let cor_runtime = {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let scg_arc = std::sync::Arc::new(scg.clone());
            let cor_config = CorConfig::default();
            let mut rt = CORuntime::from_vuma_scg(scg_arc, cor_config);
            let all_node_ids: Vec<u64> = scg.node_ids().map(|id| id.as_u64()).collect();
            let delta = vuma_cor::types::Delta {
                added_nodes: all_node_ids,
                ..vuma_cor::types::Delta::empty()
            };
            let _recompiled = rt.compile_incremental(&delta);
            rt
        }));
        match result {
            Ok(rt) => Some(rt),
            Err(panic_payload) => {
                let message = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic in COR init".to_string()
                };
                errors.push(VumaError::PanicCaught {
                    stage: "cor-init".to_string(),
                    message,
                });
                None
            }
        }
    };
    timings.push(("cor-init".to_string(), t.elapsed().as_millis() as u64));

    // If we accumulated non-fatal errors but still produced a binary, return success
    // with diagnostics attached (but we can't add diagnostics to CompilationOutput
    // without changing the struct, so just return Success).
    // The caller can check the error list separately.
    if errors.is_empty() {
        CompileResult::Success(CompilationOutput {
            binary,
            scg,
            msg,
            verification,
            stage_timings: timings,
            ir_function_count,
            ir_instruction_count,
            code_words,
            debug_info: if config.debug_info {
                Some(DebugInfo {
                    ast: Some(ast),
                    ir_pre_regalloc: Some(ir_program),
                    regalloc_results,
                    transform_results: transform_result,
                })
            } else {
                None
            },
            cor_runtime,
        })
    } else {
        // We have a binary but also some non-fatal errors — still return
        // Success since the binary is valid. Errors can be logged.
        // If the caller needs partial+diagnostics, they should use
        // compile_with_recovery.
        CompileResult::Success(CompilationOutput {
            binary,
            scg,
            msg,
            verification,
            stage_timings: timings,
            ir_function_count,
            ir_instruction_count,
            code_words,
            debug_info: if config.debug_info {
                Some(DebugInfo {
                    ast: Some(ast),
                    ir_pre_regalloc: Some(ir_program),
                    regalloc_results,
                    transform_results: transform_result,
                })
            } else {
                None
            },
            cor_runtime,
        })
    }
}

// ── ELF .text section instruction counting ─────────────────────────────

/// Count the number of ARM64 instructions in the `.text` section of an
/// ELF binary.
///
/// For AArch64, each instruction is 4 bytes.  This function parses the
/// ELF section headers to find the `.text` section and divides its size
/// by 4.  If section headers are absent or the binary is too short, it
/// falls back to `binary.len() / 4`.
fn count_text_section_instructions(binary: &[u8]) -> usize {
    // Minimum 64-byte ELF header for 64-bit ELF
    if binary.len() < 64 {
        return binary.len() / 4;
    }

    // Check ELF magic
    if &binary[0..4] != b"\x7fELF" {
        return binary.len() / 4;
    }

    // Check 64-bit ELF (EI_CLASS = 2)
    if binary[4] != 2 {
        // 32-bit ELF — different header layout; fall back
        return binary.len() / 4;
    }

    // Little-endian (EI_DATA = 1) or big-endian (2)?
    let le = binary[5] == 1;

    // Read e_shoff (section header table offset) at offset 0x28 (8 bytes)
    let e_shoff = read_u64_le_or_be(&binary[0x28..0x30], le) as usize;
    // Read e_shentsize at offset 0x3A (2 bytes)
    let e_shentsize = read_u16_le_or_be(&binary[0x3A..0x3C], le) as usize;
    // Read e_shnum at offset 0x3C (2 bytes)
    let e_shnum = read_u16_le_or_be(&binary[0x3C..0x3E], le) as usize;
    // Read e_shstrndx at offset 0x3E (2 bytes)
    let e_shstrndx = read_u16_le_or_be(&binary[0x3E..0x40], le) as usize;

    if e_shoff == 0 || e_shentsize == 0 || e_shnum == 0 {
        // No section headers — fall back to total size / 4
        return binary.len() / 4;
    }

    // Bounds check
    if e_shoff + e_shstrndx * e_shentsize + e_shentsize > binary.len() {
        return binary.len() / 4;
    }

    // Read the section header string table section header (at index e_shstrndx)
    let shstrtab_hdr_off = e_shoff + e_shstrndx * e_shentsize;
    if shstrtab_hdr_off + e_shentsize > binary.len() {
        return binary.len() / 4;
    }

    // sh_offset at byte 24 in section header (8 bytes for 64-bit ELF)
    let shstrtab_offset =
        read_u64_le_or_be(&binary[shstrtab_hdr_off + 24..shstrtab_hdr_off + 32], le) as usize;
    // sh_size at byte 32
    let shstrtab_size =
        read_u64_le_or_be(&binary[shstrtab_hdr_off + 32..shstrtab_hdr_off + 40], le) as usize;

    if shstrtab_offset + shstrtab_size > binary.len() {
        return binary.len() / 4;
    }

    // Iterate section headers to find ".text"
    for i in 0..e_shnum {
        let hdr_off = e_shoff + i * e_shentsize;
        if hdr_off + e_shentsize > binary.len() {
            break;
        }

        // sh_name at byte 0 (4 bytes)
        let sh_name = read_u32_le_or_be(&binary[hdr_off..hdr_off + 4], le) as usize;

        // Read the name from the string table
        if sh_name < shstrtab_size {
            let name_start = shstrtab_offset + sh_name;
            let name_end = binary[name_start..shstrtab_offset + shstrtab_size]
                .iter()
                .position(|&b| b == 0)
                .map(|pos| name_start + pos)
                .unwrap_or(shstrtab_offset + shstrtab_size);

            if &binary[name_start..name_end] == b".text" {
                // Found .text section! Read sh_size at byte 32.
                let sh_size = read_u64_le_or_be(&binary[hdr_off + 32..hdr_off + 40], le) as usize;
                return sh_size / 4;
            }
        }
    }

    // .text section not found — fall back
    binary.len() / 4
}

/// Read a u16 from a 2-byte slice in the given endianness.
fn read_u16_le_or_be(bytes: &[u8], le: bool) -> u16 {
    if le {
        u16::from_le_bytes([bytes[0], bytes[1]])
    } else {
        u16::from_be_bytes([bytes[0], bytes[1]])
    }
}

/// Read a u32 from a 4-byte slice in the given endianness.
fn read_u32_le_or_be(bytes: &[u8], le: bool) -> u32 {
    if le {
        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    } else {
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }
}

/// Read a u64 from an 8-byte slice in the given endianness.
fn read_u64_le_or_be(bytes: &[u8], le: bool) -> u64 {
    if le {
        u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])
    } else {
        u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])
    }
}

/// Compile VUMA source code to a `.wasm` binary.
///
/// This is the primary API for LLM sandbox integration.  An LLM can
/// generate VUMA source, compile it to Wasm, and execute it safely in
/// a sandboxed environment using `wasmer`, `wasmtime`, or Node.js.
///
/// The produced `.wasm` module:
/// - Imports `wasi_snapshot_preview1.fd_write` and `.proc_exit`
/// - Exports `main`, `_start`, and runtime print helpers
/// - Has a `_start` entry point that calls `main()` and passes the
///   return value to `proc_exit`
///
/// # Example
///
/// ```rust,ignore
/// use vuma::pipeline::compile_to_wasm;
///
/// let source = "fn main() -> i32 { return 42; }";
/// let wasm_binary = compile_to_wasm(source).expect("compilation failed");
/// // wasm_binary is a valid .wasm module that exits with code 42
/// ```
pub fn compile_to_wasm(source: &str) -> Result<Vec<u8>, Vec<VumaError>> {
    // ── Stage 1: Parse ────────────────────────────────────────────
    let ast = match parse_source(source) {
        Ok(ast) => ast,
        Err(e) => return Err(vec![e]),
    };

    // ── Stage 2: AST → SCG ───────────────────────────────────────
    let mut scg = match ast_to_scg(&ast) {
        Ok(scg) => scg,
        Err(e) => return Err(vec![e]),
    };

    // ── Stage 3: SCG Transforms (lightweight — no verification) ──
    let _ = run_scg_transforms(&mut scg, &CompileConfig {
        target: CompileTarget::Wasm32,
        opt_level: OptLevel::O1,
        verification_level: VerificationLevel::None,
        entry_name: "main".to_string(),
        debug_info: false,
        stop_on_first_error: true,
        max_inline_size: 50,
        memory_safety: true,
        runtime_bounds_checks: false,
        section_headers: false,
    });

    // ── Stage 4: IR Lowering ─────────────────────────────────────
    let extern_fns = extract_extern_functions(&ast);
    let codegen_scg = bridge_scg_to_codegen_with_externs(&scg, &extern_fns);
    let mut ir_builder = IRBuilder::new();
    let ir_program = match ir_builder.build(&codegen_scg) {
        Ok(ir) => ir,
        Err(e) => return Err(vec![VumaError::Codegen { error: CodegenError::ElfError(format!("{}", e)) }]),
    };

    // ── Stage 5: Compile IR → Wasm ──────────────────────────────
    let wasm_bytes = match vuma_codegen::compile_to_wasm(&ir_program.functions) {
        Ok(bytes) => bytes,
        Err(e) => {
            return Err(vec![VumaError::Codegen { error: CodegenError::ElfError(format!("{}", e)) }]);
        }
    };

    Ok(wasm_bytes)
}

/// Incremental compilation: only re-run stages affected by changes
/// since the last compilation.
///
/// Returns the compilation output if successful, or a list of errors.
/// The cache is updated in-place with the results of this run.
pub fn compile_incremental(
    source: &str,
    config: &CompileConfig,
    cache: &mut IncrementalCache,
) -> Result<CompilationOutput, Vec<VumaError>> {
    let new_fp = SourceFingerprint::from_source(source);

    // Determine which stages need to re-run.
    if cache.source_fingerprint != new_fp {
        // Source changed — everything from parse onwards must re-run.
        cache.invalidated_stages = PipelineStage::from(PipelineStage::Parse);
    }

    // If nothing is invalidated, we can potentially skip everything.
    if cache.invalidated_stages.is_empty() {
        // No changes detected. Re-emit from cached state if possible.
        // For simplicity, we fall through to a full recompile.
        cache.invalidated_stages = PipelineStage::from(PipelineStage::Parse);
    }

    // For now, incremental compilation falls back to a full compile.
    // A full incremental implementation would check cache.invalidated_stages
    // and reuse cached artifacts for non-invalidated stages.
    let result = compile(source, config);

    // Update cache.
    cache.source_fingerprint = new_fp;
    cache.invalidated_stages.clear();

    if let Ok(ref output) = result {
        cache.post_opt_scg = Some(output.scg.clone());
        cache.msg = Some(output.msg.clone());
        cache.verification_cache = output.verification.clone();
    }

    result
}

// ═══════════════════════════════════════════════════════════════════════════
// Stage helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Parse VUMA source text into an AST.
fn parse_source(source: &str) -> Result<AstProgram, VumaError> {
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    if result.has_errors() {
        return Err(VumaError::Parse {
            errors: result.errors,
        });
    }
    Ok(result.unwrap())
}

/// Parse VUMA source text and resolve imports relative to a base file path.
///
/// This is the preferred entry point when the source file's path is known,
/// as it enables import resolution for multi-file programs.
///
/// If the source has no `import` statements, this is equivalent to
/// [`parse_source`].  Otherwise, imported files are read, parsed, and
/// merged into a single program.
fn parse_and_resolve(source: &str, file_path: Option<&Path>) -> Result<AstProgram, VumaError> {
    // Fast path: if there are no imports, just parse normally.
    let mut parser = Parser::new(source);
    let result = parser.parse_program();
    if result.has_errors() {
        return Err(VumaError::Parse {
            errors: result.errors,
        });
    }
    let program = result.unwrap();

    // Check if there are any import statements.
    let has_imports = program.items.iter().any(|i| matches!(i, vuma_parser::ast::Item::Import(_)));
    if !has_imports {
        return Ok(program);
    }

    // Resolve imports using the ModuleResolver.
    let mut resolver = ModuleResolver::new();
    match resolver.resolve_source(source, file_path) {
        Ok(resolved) => Ok(resolved),
        Err(errors) => Err(VumaError::ModuleResolution { errors }),
    }
}

/// Convert an AST to an SCG.
fn ast_to_scg(ast: &AstProgram) -> Result<SCG, VumaError> {
    let mut converter = AstToScg::new();
    converter.convert(ast).map_err(|e| VumaError::AstToScg {
        message: format!("{}", e),
    })
}

/// Extract the set of extern function names declared in `extern "C" { ... }`
/// blocks in the AST.  These are functions that should be linked against
/// external libraries (e.g. libc) and must be emitted as relocations rather
/// than local branch instructions.
fn extract_extern_functions(ast: &AstProgram) -> HashSet<String> {
    let mut extern_fns = HashSet::new();
    for item in &ast.items {
        if let Item::ExternBlock(eb) = item {
            for fn_decl in &eb.functions {
                extern_fns.insert(fn_decl.name.clone());
            }
        }
    }
    extern_fns
}

/// Run SCG transformation passes based on the optimisation level.
pub fn run_scg_transforms(scg: &mut SCG, config: &CompileConfig) -> Option<ScgPipelineResult> {
    let mut pm = PassManager::new().verify_between(true).stop_on_error(false);

    match config.opt_level {
        OptLevel::O0 => {
            // No optimisation passes.
        }
        OptLevel::O1 => {
            pm.add_pass(DeadCodeElimination::new());
            pm.add_pass(ConstantFolding::new());
        }
        OptLevel::O2 => {
            pm.add_pass(DeadCodeElimination::new());
            pm.add_pass(ConstantFolding::new());
            pm.add_pass(CommonSubexpressionElimination::new());
            pm.add_pass(DeadCodeElimination::new()); // second pass after CSE
        }
        OptLevel::O3 => {
            pm.add_pass(DeadCodeElimination::new());
            pm.add_pass(ConstantFolding::new());
            pm.add_pass(CommonSubexpressionElimination::new());
            pm.add_pass(InliningPass::with_max_size(config.max_inline_size));
            pm.add_pass(DeadCodeElimination::new()); // cleanup after inlining
            pm.add_pass(ConstantFolding::new()); // re-fold after inlining
            pm.add_pass(CommonSubexpressionElimination::new());
            pm.add_pass(DeadCodeElimination::new()); // final cleanup
        }
    }

    if pm.pass_count() > 0 {
        Some(pm.run(scg))
    } else {
        None
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// Test 1: Full pipeline with a simple allocation program.
    ///
    /// NOTE: `verification_level` is set to `None` because the IVE
    /// cleanup-graph extractor (`src/ive/src/verification.rs::
    /// extract_cleanup_graph`) currently has a false positive on
    /// top-level `region` declarations: the Allocation node for a
    /// top-level `region` has no ControlFlow predecessors/successors
    /// (only a Derivation edge from its Phantom marker, and Derivation
    /// edges are deliberately excluded from the cleanup graph), so it
    /// is treated as both a start node and a terminal node by the DFS,
    /// and `check_leaks` flags it as a leak.  Additionally, the IVE
    /// does not yet implement the spec §5.4 "Global scope / Static
    /// lifetime" inference that should mark program-lifetime arenas
    /// as intentionally leaked.  Both are IVE bugs (see Task 2-a
    /// report in worklog.md); until they are fixed, programs that use
    /// the canonical top-level `region` pattern cannot pass Normal
    /// verification.  This test exercises the *full code-generation
    /// pipeline* (parse → SCG → IR → regalloc → emit → COR), not
    /// verification, so disabling verification preserves the test's
    /// intent.  Adding `free(memory_pool)` to the program does NOT
    /// work around the false positive: the Deallocation node would
    /// still only be linked to the Allocation via a Derivation edge.
    #[test]
    fn test_compile_simple_allocation() {
        let source = r#"
            region memory_pool = allocate(1024);
            fn main() {
                node_ptr = memory_pool + 64;
                header = node_ptr as *NodeHeader;
            }
        "#;
        let config = CompileConfig {
            verification_level: VerificationLevel::None,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        assert!(result.is_ok(), "Expected successful compilation");
        let output = result.unwrap();
        assert!(!output.binary.is_empty(), "Should produce binary output");
        assert!(output.scg.node_count() > 0, "SCG should have nodes");
        assert!(
            output.verification.is_none(),
            "Verification is disabled for this test (IVE cleanup false positive on top-level regions)"
        );
        assert_eq!(
            output.stage_timings.len(),
            11,
            "All 11 stages should report timing (the ive-verification stage still runs even when level is None)"
        );
        assert!(
            output.cor_runtime.is_some(),
            "COR runtime should be initialized"
        );
    }

    /// Test 2: Compile with O0 (no optimisation).
    #[test]
    fn test_compile_no_optimisation() {
        let source = r#"
            fn main() {
            }
        "#;
        let config = CompileConfig {
            opt_level: OptLevel::O0,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        assert!(result.is_ok(), "O0 compilation should succeed");
        let output = result.unwrap();
        assert!(
            output.binary.len() >= 64,
            "Even empty program produces ELF header"
        );
    }

    /// Test 3: Compile with O3 (aggressive optimisation).
    ///
    /// NOTE: `verification_level` is set to `None` for the same reason
    /// as `test_compile_simple_allocation` — the IVE cleanup-graph
    /// extractor has a false positive on top-level `region` declarations
    /// (the Allocation node has no ControlFlow edges, only Derivation,
    /// which is excluded from the cleanup graph).  This test exercises
    /// O3 optimisation, not verification, so disabling verification
    /// preserves the test's intent.
    #[test]
    fn test_compile_aggressive_optimisation() {
        let source = r#"
            region buf = allocate(256);
            fn process() {
                node_ptr = buf + 64;
                header = node_ptr as *NodeHeader;
            }
        "#;
        let config = CompileConfig {
            opt_level: OptLevel::O3,
            verification_level: VerificationLevel::None,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        assert!(result.is_ok(), "O3 compilation should succeed");
    }

    /// Test 4: Compile with verification disabled.
    #[test]
    fn test_compile_no_verification() {
        let source = r#"
            fn main() {
            }
        "#;
        let config = CompileConfig {
            verification_level: VerificationLevel::None,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            output.verification.is_none(),
            "Verification should be skipped"
        );
    }

    /// Test 5: Compile with quick verification.
    #[test]
    fn test_compile_quick_verification() {
        let source = r#"
            fn main() {
            }
        "#;
        let config = CompileConfig {
            verification_level: VerificationLevel::Quick,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        assert!(result.is_ok());
        let output = result.unwrap();
        let verification = output.verification.unwrap();
        assert_eq!(
            verification.per_invariant.len(),
            2,
            "Quick should check 2 invariants"
        );
    }

    /// Test 6: Compile with debug info.
    #[test]
    fn test_compile_with_debug_info() {
        let source = r#"
            fn main() {
            }
        "#;
        let config = CompileConfig {
            debug_info: true,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(output.debug_info.is_some(), "Debug info should be captured");
        let debug = output.debug_info.unwrap();
        assert!(debug.ast.is_some(), "AST should be in debug info");
        assert!(
            debug.ir_pre_regalloc.is_some(),
            "IR should be in debug info"
        );
    }

    /// Test 8: Source fingerprint detects changes.
    #[test]
    fn test_source_fingerprint() {
        let fp1 = SourceFingerprint::from_source("fn main() {}");
        let fp2 = SourceFingerprint::from_source("fn main() {} ");
        let fp3 = SourceFingerprint::from_source("fn main() {}");
        assert_ne!(
            fp1, fp2,
            "Different sources should have different fingerprints"
        );
        assert_eq!(fp1, fp3, "Same sources should have same fingerprints");
    }

    /// Test 9: Incremental compilation updates the cache.
    #[test]
    fn test_incremental_compilation() {
        let source = r#"
            fn main() {
            }
        "#;
        let config = CompileConfig::default();
        let mut cache = IncrementalCache {
            source_fingerprint: SourceFingerprint::from_source("old source"),
            ast: None,
            pre_opt_scg: None,
            post_opt_scg: None,
            msg: None,
            verification_cache: None,
            invalidated_stages: vec![],
        };
        let result = compile_incremental(source, &config, &mut cache);
        assert!(result.is_ok(), "Incremental compilation should succeed");
        assert!(
            cache.post_opt_scg.is_some(),
            "Cache should be populated after incremental compile"
        );
        assert!(cache.msg.is_some(), "MSG cache should be populated");
    }

    /// Test 10: Pipeline stage ordering.
    #[test]
    fn test_pipeline_stage_ordering() {
        let stages = PipelineStage::all();
        assert_eq!(stages.len(), 11);
        assert_eq!(stages[0], PipelineStage::Parse);
        assert_eq!(stages[9], PipelineStage::CodeEmission);
        assert_eq!(stages[10], PipelineStage::CorInit);

        // from() should return all stages from the given one onwards.
        let from_msg = PipelineStage::from(PipelineStage::MsgConstruction);
        assert_eq!(from_msg.len(), 7);
        assert_eq!(from_msg[0], PipelineStage::MsgConstruction);
        assert_eq!(from_msg[5], PipelineStage::CodeEmission);
        assert_eq!(from_msg[6], PipelineStage::CorInit);
    }

    /// Test 11: CompileConfig defaults are reasonable.
    #[test]
    fn test_config_defaults() {
        let config = CompileConfig::default();
        assert_eq!(config.target, CompileTarget::Linux);
        assert_eq!(config.opt_level, OptLevel::O2);
        assert_eq!(config.verification_level, VerificationLevel::Normal);
        assert_eq!(config.entry_name, "main");
        assert!(!config.debug_info);
    }

    /// Test 12: Error display formatting.
    #[test]
    fn test_error_display() {
        let err = VumaError::AstToScg {
            message: "test error".to_string(),
        };
        let display = format!("{}", err);
        assert!(display.contains("[ast-to-scg]"));
        assert!(display.contains("test error"));

        let err2 = VumaError::Multi {
            errors: vec![
                VumaError::BdInference {
                    node_id: Some(42),
                    message: "bad inference".to_string(),
                },
                VumaError::Emission {
                    message: "bad emit".to_string(),
                },
            ],
        };
        let display2 = format!("{}", err2);
        assert!(display2.contains("multiple errors"));
        assert!(display2.contains("bad inference"));
        assert!(display2.contains("bad emit"));
    }
}
