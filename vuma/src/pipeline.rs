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
use std::time::Instant;

// ── Workspace crate imports ──────────────────────────────────────────────

use vuma_bd::{repd::RepD, BD};
use vuma_codegen::{
    emit::{emit_binary, EmitConfig},
    ir::{BinOpKind as IrBinOpKind, IRProgram},
    regalloc::{AllocationResult, LinearScanAllocator},
    scg_to_ir::{
        AccessNode, AllocationNode, CallNode, CastNode, ComputationNode, ControlNode, IRBuilder,
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
    AggregatedResult, InferenceEngine, InvariantAggregator,
    VerificationLevel as IveVerificationLevel,
};
use vuma_parser::{AstToScg, ParseError, Parser, Program as AstProgram};
use vuma_scg::{
    AccessMode, CommonSubexpressionElimination, ConstantFolding, ControlKind, DeadCodeElimination,
    EdgeData, EdgeKind, InliningPass, NodeData, NodeId, NodePayload, NodeType, PassManager,
    PipelineResult as ScgPipelineResult, SCG,
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
}

impl fmt::Display for CompileTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileTarget::Linux => write!(f, "linux"),
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
}

impl CompileConfig {
    /// Fast-compilation debug configuration.
    pub fn debug() -> Self {
        Self {
            opt_level: OptLevel::O0,
            debug_info: true,
            verification_level: VerificationLevel::Quick,
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
            CompileTarget::Linux => EmitConfig::linux_elf(),
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
    /// A collection of errors accumulated across stages.
    Multi {
        /// The collected errors.
        errors: Vec<VumaError>,
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
            VumaError::Multi { .. } => "multi",
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
            VumaError::Multi { errors } => {
                write!(f, "multiple errors ({}):", errors.len())?;
                for (i, e) in errors.iter().enumerate() {
                    write!(f, "\n{}. {}", i + 1, e)?;
                }
                Ok(())
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
fn resolve_df_input(
    node_id: NodeId,
    position: usize,
    edge_idx: &EdgeIndex,
    scg: &SCG,
) -> ScgExpr {
    let df_inputs = edge_idx.incoming_df(node_id);
    if position < df_inputs.len() {
        let source = df_inputs[position].source;
        // Check if the source node is a productive node that defines a variable.
        // Control nodes (FunctionEntry, Branch, etc.) and Phantom nodes do not
        // produce named variables — skip them to avoid UnknownVariable errors.
        if let Some(src_data) = scg.get_node(source) {
            match &src_data.payload {
                // These node types do NOT produce named variables in the
                // codegen SCG.  Control/Phantom are structural; Deallocation
                // and Effect are lowered to runtime calls (no dst vreg).
                NodePayload::Control(_)
                | NodePayload::Phantom(_)
                | NodePayload::Deallocation(_)
                | NodePayload::Effect(_)
                | NodePayload::VTable(_)
                | NodePayload::ClosureEnv(_) => {
                    // Non-productive source — use 0 as placeholder
                    ScgExpr::Int(0)
                }
                _ => ScgExpr::Var(format!("v_{}", source.as_u64())),
            }
        } else {
            ScgExpr::Int(0)
        }
    } else {
        // No DataFlow edge at this position — use 0 as placeholder
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
                let is_eq = comp.operation == "eq" || comp.operation == "==";
                if is_eq {
                    // The RHS of the equality is the case value.
                    let rhs_inputs = edge_idx.incoming_df(cond_source);
                    if rhs_inputs.len() >= 2 {
                        let rhs_source = rhs_inputs[1].source;
                        if let Some(rhs_node) = scg.get_node(rhs_source) {
                            // The RHS node might be a Computation whose
                            // operation string is a literal integer.
                            if let NodePayload::Computation(rhs_comp) = &rhs_node.payload {
                                if let Ok(val) = rhs_comp.operation.parse::<i64>() {
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
                            walk_control_flow(then_tgt, scg, edge_idx, consumed, &arm_stop);

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
                                walk_control_flow(tgt, scg, edge_idx, consumed, &arm_stop);
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
                            walk_control_flow(then_tgt, scg, edge_idx, consumed, &arm_stop);

                        let else_body = else_tgt
                            .map(|tgt| walk_control_flow(tgt, scg, edge_idx, consumed, &arm_stop));

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

                    let body = walk_control_flow(body_tgt, scg, edge_idx, consumed, &loop_stop);

                    stmts.push(ScgStatement::Control(ControlNode::Loop { body }));

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
                    stmts.push(ScgStatement::Return(vec![]));
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
                    // Shouldn't appear inside a function body; pass through
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
                if let Some(stmt) = convert_node_to_statement(node_id, node_data, edge_idx, scg) {
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
            let op = parse_binop(&comp.operation).unwrap_or(IrBinOpKind::Add);
            Some(ScgStatement::Computation(ComputationNode {
                dst: node_var(node_id, "comp"),
                op,
                lhs: resolve_df_input(node_id, 0, edge_idx, scg),
                rhs: resolve_df_input(node_id, 1, edge_idx, scg),
                tail_call: false,
            }))
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
            // Lower deallocation as a proper runtime call rather than
            // a semantic no-op (`*ptr = 0`).  This ensures the memory
            // is actually freed at runtime and is more correct than
            // simply zeroing the pointer.
            Some(ScgStatement::Call(CallNode {
                dst: None,
                func: "__vuma_dealloc".to_string(),
                args: vec![resolve_df_input(node_id, 0, edge_idx, scg)],
            }))
        }

        NodePayload::Effect(eff) => Some(ScgStatement::Call(CallNode {
            dst: Some(node_var(node_id, "eff")),
            func: eff.effect_kind.clone(),
            args: vec![],
        })),

        NodePayload::Phantom(_) => None,

        NodePayload::Control(_) => {
            // Control nodes are handled by walk_control_flow
            None
        }

        NodePayload::VTable(_) | NodePayload::ClosureEnv(_) => {
            // VTable and ClosureEnv are structural nodes; no IR statement
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
                    let name = comp
                        .result_type
                        .clone()
                        .unwrap_or_else(|| format!("param_{}", i));
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
fn refine_scg_types_with_bd(scg: &mut SCG, bd_results: &[(NodeId, BD)]) {
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
fn bridge_scg_to_codegen(scg: &SCG) -> Scg {
    let edge_idx = EdgeIndex::build(scg);
    let mut consumed: HashSet<NodeId> = HashSet::new();
    let mut scg_nodes: Vec<ScgNode> = Vec::new();

    // ── Phase 1: Function boundary detection ─────────────────────
    let function_entries: Vec<(NodeId, String)> = scg
        .nodes()
        .filter_map(|n| {
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
                let mut stop_at = HashSet::new();
                if let Some(ret) = return_node {
                    stop_at.insert(ret);
                }
                walk_control_flow(first_cf.target, scg, &edge_idx, &mut consumed, &stop_at)
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
            let mut partial = walk_control_flow(*start, scg, &edge_idx, &mut consumed, &stop_at);
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
                if let Some(stmt) = convert_node_to_statement(*nid, node_data, &edge_idx, scg) {
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

    // Handle remaining nodes not consumed by any function (multi-function case)
    let remaining: Vec<NodeId> = scg.node_ids().filter(|id| !consumed.contains(id)).collect();
    if !remaining.is_empty() {
        let mut stmts = Vec::new();
        for nid in &remaining {
            if consumed.contains(nid) {
                continue;
            }
            consumed.insert(*nid);
            if let Some(node_data) = scg.get_node(*nid) {
                if let Some(stmt) = convert_node_to_statement(*nid, node_data, &edge_idx, scg) {
                    stmts.push(stmt);
                }
            }
        }
        if !stmts.is_empty() {
            if !stmts.iter().any(|s| matches!(s, ScgStatement::Return(_))) {
                stmts.push(ScgStatement::Return(vec![]));
            }
            scg_nodes.push(ScgNode::Function(ScgFunction {
                name: "__remaining".to_string(),
                params: vec![],
                results: vec![],
                body: stmts,
            }));
        }
    }

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
        "add" | "+" => Some(IrBinOpKind::Add),
        "sub" | "-" => Some(IrBinOpKind::Sub),
        "mul" | "*" => Some(IrBinOpKind::Mul),
        "sdiv" | "/" => Some(IrBinOpKind::SDiv),
        "udiv" => Some(IrBinOpKind::UDiv),
        "srem" | "%" => Some(IrBinOpKind::SRem),
        "urem" => Some(IrBinOpKind::URem),
        "and" | "&" => Some(IrBinOpKind::And),
        "or" | "|" => Some(IrBinOpKind::Or),
        "xor" | "^" => Some(IrBinOpKind::Xor),
        "shl" | "<<" => Some(IrBinOpKind::Shl),
        "shr.l" | ">>" => Some(IrBinOpKind::ShrL),
        "shr.a" => Some(IrBinOpKind::ShrA),
        "slt" | "<" => Some(IrBinOpKind::SLt),
        "sle" | "<=" => Some(IrBinOpKind::SLe),
        "sgt" | ">" => Some(IrBinOpKind::SGt),
        "sge" | ">=" => Some(IrBinOpKind::SGe),
        "ult" => Some(IrBinOpKind::ULt),
        "ule" => Some(IrBinOpKind::ULe),
        "ugt" => Some(IrBinOpKind::UGt),
        "uge" => Some(IrBinOpKind::UGe),
        "eq" | "==" => Some(IrBinOpKind::Eq),
        "ne" | "!=" => Some(IrBinOpKind::Ne),
        _ => None,
    }
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
    let mut errors: Vec<VumaError> = Vec::new();
    let mut timings: Vec<(String, u64)> = Vec::new();

    // ── Stage 1: Parse ────────────────────────────────────────────────
    let t = Instant::now();
    let ast = match parse_source(source) {
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
    let codegen_scg = bridge_scg_to_codegen(&scg);
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

/// Convert an AST to an SCG.
fn ast_to_scg(ast: &AstProgram) -> Result<SCG, VumaError> {
    let mut converter = AstToScg::new();
    converter.convert(ast).map_err(|e| VumaError::AstToScg {
        message: format!("{}", e),
    })
}

/// Run SCG transformation passes based on the optimisation level.
fn run_scg_transforms(scg: &mut SCG, config: &CompileConfig) -> Option<ScgPipelineResult> {
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
    #[test]
    fn test_compile_simple_allocation() {
        let source = r#"
            region memory_pool = allocate(1024);
            fn main() {
                node_ptr = memory_pool + 64;
                header = node_ptr as *NodeHeader;
            }
        "#;
        let config = CompileConfig::default();
        let result = compile(source, &config);
        assert!(result.is_ok(), "Expected successful compilation");
        let output = result.unwrap();
        assert!(!output.binary.is_empty(), "Should produce binary output");
        assert!(output.scg.node_count() > 0, "SCG should have nodes");
        assert!(
            output.verification.is_some(),
            "Verification should run at Normal level"
        );
        assert_eq!(
            output.stage_timings.len(),
            11,
            "All 11 stages should report timing"
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
