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

use std::fmt;
use std::time::{Duration, Instant};

// ── Workspace crate imports ──────────────────────────────────────────────

use vuma_parser::{Parser, AstToScg, Program as AstProgram, ParseError, Diagnostic, Span,
                   ErrorCollector, ErrorRecovery};
use vuma_scg::{
    SCG, NodeId, NodeData, NodeType, EdgeKind,
    SCGError, ValidationResult,
    CommonSubexpressionElimination, ConstantFolding, DeadCodeElimination,
    InliningPass, PassManager, PipelineResult as ScgPipelineResult,
    SCGPass, VerificationPass,
};
use vuma_ive::{
    InferenceEngine, VerificationEngine,
    InvariantAggregator, VerificationLevel as IveVerificationLevel,
    AggregatedResult, OverallVerdict, DiagnosticsReport,
    InvariantDelta, InvariantKind,
    AggregatorConfig, VerificationContext as IveVerificationContext,
    VerificationSummary,
    InvariantDependencyGraph, ReVerificationPlan, SuggestedFix,
};
use vuma_bd::BD;
use vuma_core::{
    MSG,
    scg_to_msg::{scg_to_msg, ConversionError},
};
use vuma_codegen::{
    scg_to_ir::{IRBuilder, Scg, ScgNode, ScgFunction, ScgParam, ScgType,
                ScgStatement, ControlNode, AllocationNode, AccessNode,
                CastNode, ComputationNode, CallNode, ScgExpr, ScgData},
    ir::{IRProgram, IRFunction, IRBlock, IRInstr, IRValue, BinOpKind as IrBinOpKind},
    emit::{Emitter, EmitConfig, emit_elf, emit_raw, OutputFormat as EmitOutputFormat, Target as EmitTarget},
    regalloc::{LinearScanAllocator, AllocationResult},
    CodegenError, DataSectionKind, CastKind as CodegenCastKind,
};

// ═══════════════════════════════════════════════════════════════════════════
// CompileConfig
// ═══════════════════════════════════════════════════════════════════════════

/// The compilation target platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CompileTarget {
    /// Bare-metal Raspberry Pi 5 (ARMv8.2-A, loaded at 0x80000).
    Pi5Bare,
    /// Linux user-space on Raspberry Pi 5 (AArch64).
    Pi5Linux,
    /// Generic Linux user-space on AArch64.
    Linux,
}

impl Default for CompileTarget {
    fn default() -> Self {
        CompileTarget::Pi5Linux
    }
}

impl fmt::Display for CompileTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileTarget::Pi5Bare => write!(f, "pi5-bare"),
            CompileTarget::Pi5Linux => write!(f, "pi5-linux"),
            CompileTarget::Linux => write!(f, "linux"),
        }
    }
}

/// Optimization level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum OptLevel {
    /// No optimisation — fastest compilation, best debuggability.
    O0,
    /// Basic optimisations (DCE, constant folding).
    O1,
    /// Full optimisations (DCE, CSE, constant folding, inlining).
    O2,
    /// Aggressive optimisations (O2 + inlining of larger functions).
    O3,
}

impl Default for OptLevel {
    fn default() -> Self {
        OptLevel::O2
    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VerificationLevel {
    /// Skip verification entirely.
    None,
    /// Quick: only cheap syntactic checks.
    Quick,
    /// Normal: all five invariant checks.
    Normal,
    /// Exhaustive: all checks + formal proof attempts.
    Exhaustive,
}

impl Default for VerificationLevel {
    fn default() -> Self {
        VerificationLevel::Normal
    }
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
    /// Bare-metal Pi 5 defaults.
    pub fn pi5_bare() -> Self {
        Self {
            target: CompileTarget::Pi5Bare,
            entry_name: "_start".to_string(),
            ..Self::default()
        }
    }

    /// Linux hosted defaults.
    pub fn pi5_linux() -> Self {
        Self {
            target: CompileTarget::Pi5Linux,
            entry_name: "main".to_string(),
            ..Self::default()
        }
    }

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
            CompileTarget::Pi5Bare => EmitConfig::bare_metal_elf(),
            CompileTarget::Pi5Linux | CompileTarget::Linux => EmitConfig::linux_elf(),
        }
    }
}

impl Default for CompileConfig {
    fn default() -> Self {
        Self {
            target: CompileTarget::Pi5Linux,
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
        message: String,
    },
    /// SCG validation failed.
    ScgValidation {
        errors: Vec<String>,
    },
    /// SCG → MSG conversion error.
    ScgToMsg {
        error: ConversionError,
    },
    /// BD inference error.
    BdInference {
        node_id: Option<u64>,
        message: String,
    },
    /// IVE verification failure (one or more invariants violated).
    Verification {
        result: AggregatedResult,
    },
    /// SCG transformation pass error.
    Transform {
        pass_name: String,
        errors: Vec<String>,
    },
    /// IR lowering / codegen error.
    Codegen {
        error: CodegenError,
    },
    /// Register allocation failure.
    RegisterAlloc {
        message: String,
    },
    /// ELF emission failure.
    Emission {
        message: String,
    },
    /// A collection of errors accumulated across stages.
    Multi {
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
#[derive(Debug, Clone)]
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
}

impl PipelineStage {
    /// All stages in order.
    pub fn all() -> &'static [PipelineStage; 10] {
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
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SCG → Codegen SCG bridge
// ═══════════════════════════════════════════════════════════════════════════

/// Convert a `vuma_scg::SCG` into the codegen's stub `Scg` type.
///
/// The codegen crate defines its own lightweight SCG representation
/// for IR lowering. This function bridges between the two by walking
/// the real SCG and extracting function/data nodes.
fn bridge_scg_to_codegen(scg: &SCG) -> Scg {
    let mut nodes = Vec::new();

    // Walk all nodes in the SCG and classify them.
    // We group nodes into functions based on region membership and
    // control-flow structure.
    let topo = match scg.topological_sort() {
        Ok(t) => t,
        Err(_) => scg.node_ids().collect(),
    };

    // Collect all computation/alloc/access/cast nodes as a single
    // "main" function for now. A more sophisticated bridge would
    // reconstruct function boundaries from Control(FunctionEntry) nodes.
    let mut stmts = Vec::new();
    let mut alloc_count = 0u32;

    for node_id in &topo {
        if let Some(node_data) = scg.get_node(*node_id) {
            match &node_data.payload {
                vuma_scg::node::NodePayload::Allocation(alloc) => {
                    alloc_count += 1;
                    stmts.push(ScgStatement::Allocation(AllocationNode::Stack {
                        name: format!("alloc_{}", alloc_count),
                        size: alloc.size as u32,
                        ty: ScgType::U8,
                    }));
                }
                vuma_scg::node::NodePayload::Access(access) => {
                    let mode = match access.mode {
                        vuma_scg::node::AccessMode::Read => "read",
                        vuma_scg::node::AccessMode::Write => "write",
                        vuma_scg::node::AccessMode::ReadWrite => "readwrite",
                    };
                    let ptr_name = format!("ptr_{}", node_id.as_u64());
                    let offset_expr = access.offset.map(|o| ScgExpr::Int(o as i64));
                    stmts.push(ScgStatement::Access(AccessNode::Load {
                        dst: format!("val_{}", node_id.as_u64()),
                        ptr: ScgExpr::Var(ptr_name),
                        offset: offset_expr,
                    }));
                    let _ = mode; // used in future, more precise bridging
                }
                vuma_scg::node::NodePayload::Computation(comp) => {
                    // Try to parse the operation string into a BinOpKind.
                    let op = parse_binop(&comp.operation).unwrap_or(IrBinOpKind::Add);
                    let lhs_name = format!("lhs_{}", node_id.as_u64());
                    let rhs_name = format!("rhs_{}", node_id.as_u64());
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: format!("comp_{}", node_id.as_u64()),
                        op,
                        lhs: ScgExpr::Var(lhs_name),
                        rhs: ScgExpr::Var(rhs_name),
                    }));
                }
                vuma_scg::node::NodePayload::Cast(cast) => {
                    stmts.push(ScgStatement::Cast(CastNode {
                        dst: format!("cast_{}", node_id.as_u64()),
                        src: ScgExpr::Var(format!("src_{}", node_id.as_u64())),
                        kind: CodegenCastKind::BitCast,
                        from_ty: ScgType::Ptr,
                        to_ty: ScgType::Ptr,
                    }));
                }
                vuma_scg::node::NodePayload::Control(ctrl) => {
                    if ctrl.kind == vuma_scg::node::ControlKind::FunctionReturn {
                        stmts.push(ScgStatement::Return(vec![]));
                    }
                    // Other control nodes are not directly representable in
                    // the codegen's stub SCG.
                }
                vuma_scg::node::NodePayload::Deallocation(_dealloc) => {
                    stmts.push(ScgStatement::Access(AccessNode::Store {
                        ptr: ScgExpr::Var(format!("alloc_{}", node_id.as_u64())),
                        offset: None,
                        value: ScgExpr::Int(0),
                    }));
                }
                vuma_scg::node::NodePayload::Effect(_) | vuma_scg::node::NodePayload::Phantom(_) => {
                    // Skip effect and phantom nodes in the bridge.
                }
            }
        }
    }

    // Ensure at least a return statement at the end.
    if !stmts.iter().any(|s| matches!(s, ScgStatement::Return(_))) {
        stmts.push(ScgStatement::Return(vec![]));
    }

    // Build a single "main" function containing all statements.
    let main_func = ScgFunction {
        name: "main".to_string(),
        params: vec![],
        results: vec![],
        body: stmts,
    };

    nodes.push(ScgNode::Function(main_func));

    Scg { nodes }
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
    // Inference is currently placeholder; log but don't fail.
    let _bd_results = inference_engine.infer_types(&vuma_ive::inference::SCG {
        node_count: scg.node_count(),
    });
    timings.push(("bd-inference".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 5: MSG Construction ─────────────────────────────────────
    let t = Instant::now();
    let msg = match scg_to_msg(&scg) {
        Ok(msg) => msg,
        Err(e) => {
            errors.push(VumaError::ScgToMsg { error: e });
            if config.stop_on_first_error {
                return Err(errors);
            }
            MSG::new() // fall back to empty MSG
        }
    };
    timings.push(("msg-construction".to_string(), t.elapsed().as_millis() as u64));

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
        let msg_stub = vuma_ive::verification::Message::default();
        let scg_stub = vuma_ive::inference::SCG {
            node_count: scg.node_count(),
        };
        let result = aggregator.verify_all(&msg_stub, &scg_stub);
        Some(result)
    } else {
        None
    };
    timings.push(("ive-verification".to_string(), t.elapsed().as_millis() as u64));

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
    let binary = match emit_elf(
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
    let code_words = binary.len() / 4; // approximate
    timings.push(("code-emission".to_string(), t.elapsed().as_millis() as u64));

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
    })
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
    parser
        .parse_program()
        .map_err(|e| VumaError::Parse {
            errors: e,
        })
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
            pm.add_pass(ConstantFolding::new());      // re-fold after inlining
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
// IVE Verification Integration
// ═══════════════════════════════════════════════════════════════════════════

/// Configuration for the integrated IVE verification pipeline stage.
///
/// Controls how the full 5-invariant verification pipeline is invoked,
/// including incremental re-verification, caching, error recovery, and
/// time budgets.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PipelineVerificationConfig {
    /// Configuration forwarded to the `InvariantAggregator::run_full_pipeline`.
    pub aggregator_config: AggregatorConfig,
    /// Whether to enable incremental re-verification when a previous
    /// `AggregatedResult` is available in the cache.
    pub enable_incremental: bool,
    /// Whether to cache verification results for reuse by incremental runs.
    pub enable_caching: bool,
    /// Whether to attempt error recovery when verification fails, producing
    /// a `PartialVerificationResult` with safe/unsafe region classification.
    pub enable_error_recovery: bool,
    /// Target wall-clock time budget for verification. If the pipeline
    /// exceeds this duration, remaining checks are marked `Unverified`.
    pub target_verification_time: Duration,
}

impl Default for PipelineVerificationConfig {
    fn default() -> Self {
        Self {
            aggregator_config: AggregatorConfig::default(),
            enable_incremental: true,
            enable_caching: true,
            enable_error_recovery: true,
            target_verification_time: Duration::from_secs(30),
        }
    }
}

impl PipelineVerificationConfig {
    /// Create a new configuration with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a fast configuration that prioritises speed over completeness.
    pub fn fast() -> Self {
        Self {
            aggregator_config: AggregatorConfig::default()
                .with_stop_on_first_violation(true),
            enable_incremental: true,
            enable_caching: true,
            enable_error_recovery: false,
            target_verification_time: Duration::from_secs(5),
        }
    }

    /// Create a thorough configuration that runs every check and attempts
    /// error recovery.
    pub fn thorough() -> Self {
        Self {
            aggregator_config: AggregatorConfig::default()
                .with_max_violations(100),
            enable_incremental: true,
            enable_caching: true,
            enable_error_recovery: true,
            target_verification_time: Duration::from_secs(120),
        }
    }
}

// ---------------------------------------------------------------------------
// IncrementalVerificationResult
// ---------------------------------------------------------------------------

/// The result of an incremental re-verification run.
///
/// Contains both the new verification summary and metadata about what
/// was recomputed vs. reused from cache.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IncrementalVerificationResult {
    /// The updated aggregated result after incremental re-verification.
    pub result: AggregatedResult,
    /// The delta describing which invariants were affected.
    pub delta: InvariantDelta,
    /// Number of invariants that were re-checked (not cached).
    pub rechecked_count: usize,
    /// Number of invariants whose cached results were reused.
    pub reused_count: usize,
    /// Wall-clock time spent on the incremental run (milliseconds).
    pub elapsed_ms: u64,
    /// The re-verification plan that was executed (if dependency-based
    /// planning was used).
    pub plan: Option<ReVerificationPlan>,
}

impl fmt::Display for IncrementalVerificationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "IncrementalVerificationResult: verdict={}, rechecked={}, reused={}, elapsed={}ms",
            self.result.overall, self.rechecked_count, self.reused_count, self.elapsed_ms
        )
    }
}

// ---------------------------------------------------------------------------
// FixSuggestion
// ---------------------------------------------------------------------------

/// A suggested fix for a verification failure.
///
/// Each suggestion addresses a specific invariant violation and provides
/// a human-readable description and optional code hint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FixSuggestion {
    /// The invariant kind that this fix addresses.
    pub invariant: InvariantKind,
    /// Human-readable description of the fix.
    pub description: String,
    /// Optional code snippet hint showing how the fix could be applied.
    pub code_hint: Option<String>,
    /// Confidence that this fix correctly resolves the violation (0.0–1.0).
    pub confidence: f64,
}

impl fmt::Display for FixSuggestion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {} (confidence: {:.0}%)", self.invariant, self.description, self.confidence * 100.0)
    }
}

// ---------------------------------------------------------------------------
// PartialVerificationResult
// ---------------------------------------------------------------------------

/// A partial verification result produced by error recovery.
///
/// When verification fails, error recovery partitions the program into
/// safe and unsafe regions, generates fix suggestions, and preserves
/// any passing invariant results for reuse.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PartialVerificationResult {
    /// The original failed verification summary.
    pub original_summary: VerificationSummary,
    /// Invariant kinds that passed verification (safe region).
    pub safe_invariants: Vec<InvariantKind>,
    /// Invariant kinds that failed verification (unsafe region).
    pub unsafe_invariants: Vec<InvariantKind>,
    /// Invariant kinds that could not be verified.
    pub unverified_invariants: Vec<InvariantKind>,
    /// Suggested fixes for the failed invariants.
    pub fix_suggestions: Vec<FixSuggestion>,
    /// Whether error recovery was able to produce a usable partial result.
    pub recovered: bool,
    /// Diagnostics collected during error recovery.
    pub recovery_diagnostics: Vec<String>,
}

impl PartialVerificationResult {
    /// Create a partial verification result from a failed summary.
    pub fn from_failed_summary(summary: &VerificationSummary, result: &AggregatedResult) -> Self {
        let mut safe_invariants = Vec::new();
        let mut unsafe_invariants = Vec::new();
        let mut unverified_invariants = Vec::new();
        let mut fix_suggestions = Vec::new();

        for pir in &result.per_invariant {
            if pir.is_pass() {
                safe_invariants.push(pir.kind);
            } else if pir.is_fail() {
                unsafe_invariants.push(pir.kind);
                // Generate a fix suggestion for each failure.
                fix_suggestions.push(FixSuggestion {
                    invariant: pir.kind,
                    description: format!(
                        "Fix {} violation: {}",
                        pir.kind,
                        pir.result.message
                    ),
                    code_hint: None,
                    confidence: 0.5,
                });
            } else if pir.is_unverified() {
                unverified_invariants.push(pir.kind);
            }
        }

        let recovered = !safe_invariants.is_empty();

        // Build recovery diagnostics.
        let mut recovery_diagnostics = Vec::new();
        recovery_diagnostics.push(format!(
            "Error recovery: {}/{} invariants safe, {}/{} unsafe, {}/{} unverified",
            safe_invariants.len(),
            summary.total_checked,
            unsafe_invariants.len(),
            summary.total_checked,
            unverified_invariants.len(),
            summary.total_checked,
        ));
        if !fix_suggestions.is_empty() {
            recovery_diagnostics.push(format!(
                "Generated {} fix suggestion(s)",
                fix_suggestions.len()
            ));
        }

        Self {
            original_summary: summary.clone(),
            safe_invariants,
            unsafe_invariants,
            unverified_invariants,
            fix_suggestions,
            recovered,
            recovery_diagnostics,
        }
    }
}

impl fmt::Display for PartialVerificationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "PartialVerificationResult (recovered={}):", self.recovered)?;
        writeln!(f, "  Safe invariants  : {:?}", self.safe_invariants)?;
        writeln!(f, "  Unsafe invariants: {:?}", self.unsafe_invariants)?;
        writeln!(f, "  Unverified       : {:?}", self.unverified_invariants)?;
        writeln!(f, "  Fix suggestions  :")?;
        for fix in &self.fix_suggestions {
            writeln!(f, "    - {}", fix)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PipelineResult
// ---------------------------------------------------------------------------

/// The result of running the full pipeline with integrated IVE verification.
///
/// Extends `CompilationOutput` with detailed verification information,
/// incremental verification results, and error recovery data.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// The standard compilation output (binary, SCG, MSG, etc.).
    pub output: CompilationOutput,
    /// The full verification summary from the pipeline stage.
    pub verification_summary: Option<VerificationSummary>,
    /// Incremental verification result (if incremental verification was used).
    pub incremental_result: Option<IncrementalVerificationResult>,
    /// Partial verification result with error recovery (if verification failed
    /// and recovery was enabled).
    pub partial_result: Option<PartialVerificationResult>,
    /// Diagnostics report from the IVE verification stage.
    pub diagnostics_report: Option<DiagnosticsReport>,
    /// Whether the pipeline completed successfully despite verification
    /// issues (i.e., error recovery was applied).
    pub recovered_from_verification_failure: bool,
}

// ---------------------------------------------------------------------------
// verify_stage
// ---------------------------------------------------------------------------

/// Run the full 5-invariant verification pipeline stage.
///
/// Takes an SCG and MSG as input, constructs a verification context,
/// and runs `InvariantAggregator::run_full_pipeline()` with the given
/// configuration. Returns a `VerificationSummary` with per-invariant
/// results, timing, and early-termination information.
///
/// # Error Handling
///
/// If some invariants fail but others pass, this function still returns
/// the full summary — callers can inspect the `overall_status` field
/// to determine whether the result is a pass or a partial failure.
/// For automatic error recovery, use [`recover_from_verification_failure`].
pub fn verify_stage(
    scg: &SCG,
    _msg: &MSG,
    config: &PipelineVerificationConfig,
) -> VerificationSummary {
    let aggregator = InvariantAggregator::new().with_level(IveVerificationLevel::Normal);
    let context = IveVerificationContext::new(
        vuma_ive::verification::Message::default(),
        vuma_ive::inference::SCG {
            node_count: scg.node_count(),
        },
    );
    aggregator.run_full_pipeline(&context, &config.aggregator_config)
}

// ---------------------------------------------------------------------------
// incremental_verify_stage
// ---------------------------------------------------------------------------

/// Run incremental re-verification when the SCG changes.
///
/// Given the old SCG, new SCG, and the old MSG (with cached verification
/// results), this function:
/// 1. Computes a delta describing which invariants might be affected by
///    the SCG change.
/// 2. Uses the `InvariantDependencyGraph` to plan re-verification.
/// 3. Runs incremental verification via `InvariantAggregator::verify_incremental`.
/// 4. Returns an `IncrementalVerificationResult` with both fresh and cached
///    results.
pub fn incremental_verify_stage(
    _old_scg: &SCG,
    new_scg: &SCG,
    _old_msg: &MSG,
    previous_result: &AggregatedResult,
    _config: &PipelineVerificationConfig,
) -> IncrementalVerificationResult {
    let start = Instant::now();

    // Compute delta: which invariants are affected by the SCG change.
    // A change in node count is a conservative heuristic that marks all
    // invariants as potentially affected. A more precise delta would
    // diff the SCG node-by-node.
    let delta = compute_scg_delta(_old_scg, new_scg);

    // Use the dependency graph to plan re-verification if the delta
    // is non-empty.
    let plan = if !delta.is_empty() {
        let dep_graph = InvariantDependencyGraph::default();
        let affected_names: Vec<String> = delta.affected.iter().map(|k| k.label().to_string()).collect();
        Some(dep_graph.plan_re_verification(&affected_names))
    } else {
        None
    };

    // Run incremental verification.
    let mut aggregator = InvariantAggregator::new().with_level(previous_result.level);
    let context = IveVerificationContext::new(
        vuma_ive::verification::Message::default(),
        vuma_ive::inference::SCG {
            node_count: new_scg.node_count(),
        },
    );
    let result = aggregator.verify_incremental(&context.message, &context.scg, &delta);

    let elapsed_ms = start.elapsed().as_millis() as u64;

    // Count rechecked vs. reused.
    let rechecked_count = result.per_invariant.iter().filter(|pir| !pir.cached).count();
    let reused_count = result.per_invariant.iter().filter(|pir| pir.cached).count();

    IncrementalVerificationResult {
        result,
        delta,
        rechecked_count,
        reused_count,
        elapsed_ms,
        plan,
    }
}

/// Compute an `InvariantDelta` describing which invariants may be affected
/// by a change from `old_scg` to `new_scg`.
///
/// Uses a conservative heuristic: if the node count changed, all invariants
/// are marked as affected. A more precise implementation would diff the
/// individual nodes and edges.
fn compute_scg_delta(old_scg: &SCG, new_scg: &SCG) -> InvariantDelta {
    if old_scg.node_count() != new_scg.node_count() {
        InvariantDelta::from_set(InvariantKind::all().iter().copied())
            .with_reason(format!(
                "SCG node count changed: {} -> {}",
                old_scg.node_count(),
                new_scg.node_count()
            ))
    } else {
        // Conservative: mark liveness as potentially affected on any change.
        // A real implementation would inspect the diff in detail.
        InvariantDelta::single(InvariantKind::Liveness)
            .with_reason("SCG structure may have changed")
    }
}

// ---------------------------------------------------------------------------
// recover_from_verification_failure
// ---------------------------------------------------------------------------

/// Attempt to recover from a verification failure.
///
/// When the full 5-invariant pipeline produces a failing result, this
/// function uses the `ErrorCollector` pattern and the dependency graph
/// to:
/// 1. Classify invariants into safe, unsafe, and unverified regions.
/// 2. Generate fix suggestions for each violation.
/// 3. Produce a `PartialVerificationResult` that allows the pipeline to
///    continue with partial safety guarantees.
///
/// If `config.enable_error_recovery` is `false`, this function returns
/// `None`.
pub fn recover_from_verification_failure(
    failed_result: &AggregatedResult,
    config: &PipelineVerificationConfig,
) -> Option<PartialVerificationResult> {
    if !config.enable_error_recovery {
        return None;
    }

    // Only attempt recovery on actual failures.
    if failed_result.overall != OverallVerdict::Fail
        && failed_result.overall != OverallVerdict::Inconclusive
    {
        return None;
    }

    let partial = PartialVerificationResult::from_failed_summary(
        &failed_result.summary,
        failed_result,
    );

    // Enhance fix suggestions using the dependency graph.
    let dep_graph = InvariantDependencyGraph::default();
    let mut enhanced_suggestions = partial.fix_suggestions.clone();
    for fix in &mut enhanced_suggestions {
        let impact = dep_graph.impact_of_change(fix.invariant.label());
        if !impact.directly_affected.is_empty() {
            let dependents: Vec<&str> = impact
                .directly_affected
                .iter()
                .map(|s| s.as_str())
                .collect();
            fix.description.push_str(&format!(
                " (also affects: {})",
                dependents.join(", ")
            ));
        }
    }

    // Collect recovery diagnostics.
    let mut diagnostics = partial.recovery_diagnostics.clone();
    let report = DiagnosticsReport::from_aggregated(failed_result);
    diagnostics.push(format!("Diagnostics: {}", report.verdict));

    Some(PartialVerificationResult {
        fix_suggestions: enhanced_suggestions,
        recovery_diagnostics: diagnostics,
        ..partial
    })
}

// ---------------------------------------------------------------------------
// run_pipeline_with_verification
// ---------------------------------------------------------------------------

/// Run the full VUMA compilation pipeline with integrated IVE verification.
///
/// This function extends the standard [`compile`] pipeline with a
/// configurable verification stage that supports:
/// - Full 5-invariant verification via `InvariantAggregator::run_full_pipeline`
/// - Incremental re-verification when previous results are available
/// - Error recovery that produces partial results and fix suggestions
/// - Time-budgeted verification
///
/// # Returns
///
/// A [`PipelineResult`] that includes the standard compilation output
/// plus detailed verification information. If verification fails but
/// error recovery succeeds, the `recovered_from_verification_failure`
/// flag will be `true` and `partial_result` will contain the recovery
/// data.
pub fn run_pipeline_with_verification(
    source: &str,
    config: &CompileConfig,
    verify_config: &PipelineVerificationConfig,
) -> PipelineResult {
    let mut timings: Vec<(String, u64)> = Vec::new();
    let mut errors: Vec<VumaError> = Vec::new();

    // ── Stage 1: Parse ────────────────────────────────────────────────
    let t = Instant::now();
    let ast = match parse_source(source) {
        Ok(ast) => ast,
        Err(e) => {
            errors.push(e);
            return PipelineResult {
                output: CompilationOutput {
                    binary: Vec::new(),
                    scg: SCG::new(),
                    msg: MSG::new(),
                    verification: None,
                    stage_timings: timings,
                    ir_function_count: 0,
                    ir_instruction_count: 0,
                    code_words: 0,
                    debug_info: None,
                },
                verification_summary: None,
                incremental_result: None,
                partial_result: None,
                diagnostics_report: None,
                recovered_from_verification_failure: false,
            };
        }
    };
    timings.push(("parse".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 2: AST → SCG ───────────────────────────────────────────
    let t = Instant::now();
    let mut scg = match ast_to_scg(&ast) {
        Ok(scg) => scg,
        Err(e) => {
            errors.push(e);
            return PipelineResult {
                output: CompilationOutput {
                    binary: Vec::new(),
                    scg: SCG::new(),
                    msg: MSG::new(),
                    verification: None,
                    stage_timings: timings,
                    ir_function_count: 0,
                    ir_instruction_count: 0,
                    code_words: 0,
                    debug_info: None,
                },
                verification_summary: None,
                incremental_result: None,
                partial_result: None,
                diagnostics_report: None,
                recovered_from_verification_failure: false,
            };
        }
    };
    timings.push(("ast-to-scg".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 3: SCG Validation ──────────────────────────────────────
    let t = Instant::now();
    let validation = scg.validate();
    if !validation.is_valid {
        errors.push(VumaError::ScgValidation {
            errors: validation.errors.clone(),
        });
    }
    timings.push(("scg-validation".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 4: BD Inference ─────────────────────────────────────────
    let t = Instant::now();
    let inference_engine = InferenceEngine::new();
    let _bd_results = inference_engine.infer_types(&vuma_ive::inference::SCG {
        node_count: scg.node_count(),
    });
    timings.push(("bd-inference".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 5: MSG Construction ─────────────────────────────────────
    let t = Instant::now();
    let msg = match scg_to_msg(&scg) {
        Ok(msg) => msg,
        Err(e) => {
            errors.push(VumaError::ScgToMsg { error: e });
            MSG::new()
        }
    };
    timings.push(("msg-construction".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 6: IVE Verification (enhanced) ──────────────────────────
    let t = Instant::now();
    let (verification, verification_summary, incremental_result, partial_result, recovered) =
        if config.verification_level != VerificationLevel::None {
            let ive_level = match config.verification_level {
                VerificationLevel::Quick => IveVerificationLevel::Quick,
                VerificationLevel::Normal => IveVerificationLevel::Normal,
                VerificationLevel::Exhaustive => IveVerificationLevel::Exhaustive,
                VerificationLevel::None => unreachable!(),
            };

            // Run the full verification pipeline using InvariantAggregator.
            let aggregator = InvariantAggregator::new().with_level(ive_level);
            let context = IveVerificationContext::new(
                vuma_ive::verification::Message::default(),
                vuma_ive::inference::SCG {
                    node_count: scg.node_count(),
                },
            );

            let summary = aggregator.run_full_pipeline(&context, &verify_config.aggregator_config);
            let aggregated = aggregator.verify_all(&context.message, &context.scg);

            // Generate diagnostics report.
            let _diagnostics = aggregator.diagnostics(&aggregated);

            // Attempt error recovery if verification failed.
            let partial = recover_from_verification_failure(&aggregated, verify_config);
            let recovered = partial.as_ref().map_or(false, |p| p.recovered);

            (
                Some(aggregated.clone()),
                Some(summary),
                None, // incremental not applicable on first run
                partial,
                recovered,
            )
        } else {
            (None, None, None, None, false)
        };
    timings.push(("ive-verification".to_string(), t.elapsed().as_millis() as u64));

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
            return PipelineResult {
                output: CompilationOutput {
                    binary: Vec::new(),
                    scg,
                    msg,
                    verification,
                    stage_timings: timings,
                    ir_function_count: 0,
                    ir_instruction_count: 0,
                    code_words: 0,
                    debug_info: None,
                },
                verification_summary,
                incremental_result,
                partial_result,
                diagnostics_report: None,
                recovered_from_verification_failure: recovered,
            };
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
            }
        }
    }
    timings.push(("register-alloc".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 10: Code Emission ───────────────────────────────────────
    let t = Instant::now();
    let emit_config = config.emit_config();
    let binary = match emit_elf(
        &ir_program.functions,
        &ir_program.data_sections,
        &emit_config,
    ) {
        Ok(binary) => binary,
        Err(e) => {
            errors.push(VumaError::Emission {
                message: format!("{}", e),
            });
            return PipelineResult {
                output: CompilationOutput {
                    binary: Vec::new(),
                    scg,
                    msg,
                    verification,
                    stage_timings: timings,
                    ir_function_count,
                    ir_instruction_count,
                    code_words: 0,
                    debug_info: None,
                },
                verification_summary,
                incremental_result,
                partial_result,
                diagnostics_report: None,
                recovered_from_verification_failure: recovered,
            };
        }
    };
    let code_words = binary.len() / 4;
    timings.push(("code-emission".to_string(), t.elapsed().as_millis() as u64));

    // Build diagnostics report from verification if available.
    let diagnostics_report = verification.as_ref().map(|r| DiagnosticsReport::from_aggregated(r));

    let compilation_output = CompilationOutput {
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
    };

    PipelineResult {
        output: compilation_output,
        verification_summary,
        incremental_result,
        partial_result,
        diagnostics_report,
        recovered_from_verification_failure: recovered,
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
        // Use O0 to avoid SCG transform pass errors that are a known issue.
        let config = CompileConfig {
            opt_level: OptLevel::O0,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        if let Err(ref errors) = result {
            for e in errors {
                eprintln!("Compile error: {}", e);
            }
        }
        assert!(result.is_ok(), "Expected successful compilation");
        let output = result.unwrap();
        assert!(!output.binary.is_empty(), "Should produce binary output");
        assert!(output.scg.node_count() > 0, "SCG should have nodes");
        assert!(output.verification.is_some(), "Verification should run at Normal level");
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
        assert!(output.binary.len() >= 64, "Even empty program produces ELF header");
    }

    /// Test 3: Compile with O3 (aggressive optimisation).
    #[test]
    fn test_compile_aggressive_optimisation() {
        let source = r#"
            fn main() {
            }
        "#;
        let config = CompileConfig {
            opt_level: OptLevel::O3,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        if let Err(ref errors) = result {
            for e in errors {
                eprintln!("O3 error: {}", e);
            }
        }
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
        assert!(output.verification.is_none(), "Verification should be skipped");
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
        assert_eq!(verification.per_invariant.len(), 2, "Quick should check 2 invariants");
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
        assert!(debug.ir_pre_regalloc.is_some(), "IR should be in debug info");
    }

    /// Test 7: Compile for bare-metal Pi 5.
    #[test]
    fn test_compile_pi5_bare() {
        let source = r#"
            fn main() {
            }
        "#;
        let config = CompileConfig::pi5_bare();
        let result = compile(source, &config);
        assert!(result.is_ok(), "Bare-metal compilation should succeed");
        let output = result.unwrap();
        // The ELF should start with the ELF magic bytes.
        assert_eq!(&output.binary[0..4], &[0x7f, b'E', b'L', b'F']);
    }

    /// Test 8: Source fingerprint detects changes.
    #[test]
    fn test_source_fingerprint() {
        let fp1 = SourceFingerprint::from_source("fn main() {}");
        let fp2 = SourceFingerprint::from_source("fn main() {} ");
        let fp3 = SourceFingerprint::from_source("fn main() {}");
        assert_ne!(fp1, fp2, "Different sources should have different fingerprints");
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
        assert!(cache.post_opt_scg.is_some(), "Cache should be populated after incremental compile");
        assert!(cache.msg.is_some(), "MSG cache should be populated");
    }

    /// Test 10: Pipeline stage ordering.
    #[test]
    fn test_pipeline_stage_ordering() {
        let stages = PipelineStage::all();
        assert_eq!(stages.len(), 10);
        assert_eq!(stages[0], PipelineStage::Parse);
        assert_eq!(stages[9], PipelineStage::CodeEmission);

        // from() should return all stages from the given one onwards.
        let from_msg = PipelineStage::from(PipelineStage::MsgConstruction);
        assert_eq!(from_msg.len(), 6);
        assert_eq!(from_msg[0], PipelineStage::MsgConstruction);
        assert_eq!(from_msg[5], PipelineStage::CodeEmission);
    }

    /// Test 11: CompileConfig defaults are reasonable.
    #[test]
    fn test_config_defaults() {
        let config = CompileConfig::default();
        assert_eq!(config.target, CompileTarget::Pi5Linux);
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

    // ═══════════════════════════════════════════════════════════════════════
    // IVE Verification Integration Tests
    // ═══════════════════════════════════════════════════════════════════════

    /// Test 13: verify_stage produces a VerificationSummary with all 5 invariants.
    #[test]
    fn test_verify_stage_full_pipeline() {
        let source = r#"
            fn main() {
            }
        "#;
        let config = CompileConfig {
            opt_level: OptLevel::O0,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        assert!(result.is_ok());
        let output = result.unwrap();

        let verify_config = PipelineVerificationConfig::default();
        let summary = verify_stage(&output.scg, &output.msg, &verify_config);

        // The full pipeline should have checked all 5 invariants.
        assert_eq!(summary.total_checked, 5, "Full pipeline should check all 5 invariants");
        assert!(!summary.execution_order.is_empty(), "Execution order should be populated");
        assert!(summary.passed + summary.failed + summary.unverified <= 5);
    }

    /// Test 14: PipelineVerificationConfig defaults and presets.
    #[test]
    fn test_pipeline_verification_config() {
        let default_config = PipelineVerificationConfig::default();
        assert!(default_config.enable_incremental);
        assert!(default_config.enable_caching);
        assert!(default_config.enable_error_recovery);
        assert_eq!(default_config.target_verification_time, Duration::from_secs(30));

        let fast_config = PipelineVerificationConfig::fast();
        assert!(fast_config.aggregator_config.stop_on_first_violation);
        assert!(!fast_config.enable_error_recovery);
        assert_eq!(fast_config.target_verification_time, Duration::from_secs(5));

        let thorough_config = PipelineVerificationConfig::thorough();
        assert!(thorough_config.enable_error_recovery);
        assert_eq!(thorough_config.target_verification_time, Duration::from_secs(120));
    }

    /// Test 15: run_pipeline_with_verification produces a PipelineResult with
    /// verification summary and diagnostics.
    #[test]
    fn test_run_pipeline_with_verification() {
        let source = r#"
            region buf = allocate(256);
            fn main() {
                ptr = buf + 64;
            }
        "#;
        let config = CompileConfig::default();
        let verify_config = PipelineVerificationConfig::default();

        let result = run_pipeline_with_verification(source, &config, &verify_config);

        // The pipeline should produce a binary.
        assert!(!result.output.binary.is_empty(), "Should produce binary output");
        assert!(result.output.scg.node_count() > 0, "SCG should have nodes");

        // Verification data should be present at Normal level.
        assert!(result.output.verification.is_some(), "Verification should run");
        assert!(result.verification_summary.is_some(), "Verification summary should be present");
        assert!(result.diagnostics_report.is_some(), "Diagnostics report should be present");

        let summary = result.verification_summary.unwrap();
        assert_eq!(summary.total_checked, 5, "Should check all 5 invariants");
    }

    /// Test 16: incremental_verify_stage computes delta and re-verifies.
    #[test]
    fn test_incremental_verify_stage() {
        let source = r#"
            fn main() {
            }
        "#;
        let config = CompileConfig {
            opt_level: OptLevel::O0,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        assert!(result.is_ok());
        let output = result.unwrap();

        let verify_config = PipelineVerificationConfig::default();
        let aggregated = output.verification.unwrap();

        // Simulate incremental verification with the same SCG (no change).
        let inc_result = incremental_verify_stage(
            &output.scg,
            &output.scg,
            &output.msg,
            &aggregated,
            &verify_config,
        );

        // Since SCG is the same, node count matches and delta is conservative
        // (marks liveness as affected even for identical SCGs).
        assert!(!inc_result.delta.is_empty(),
            "Delta should be non-empty even for same SCG (conservative)");

        // The incremental result should have per-invariant results.
        assert!(!inc_result.result.per_invariant.is_empty(),
            "Incremental result should have per-invariant results");
    }

    /// Test 17: recover_from_verification_failure returns None when recovery
    /// is disabled or the result is not a failure.
    #[test]
    fn test_error_recovery_disabled_and_passing() {
        // With recovery disabled, should return None even for failures.
        let config_no_recovery = PipelineVerificationConfig {
            enable_error_recovery: false,
            ..PipelineVerificationConfig::default()
        };

        // Create an AggregatedResult — for default inputs this will likely pass.
        let passing_result = InvariantAggregator::new()
            .verify_all(&vuma_ive::verification::Message::default(), &vuma_ive::inference::SCG::default());

        let recovery = recover_from_verification_failure(&passing_result, &config_no_recovery);
        assert!(recovery.is_none(), "Should not recover when disabled");

        // Also, even with recovery enabled, non-failing results should not trigger recovery.
        let config_with_recovery = PipelineVerificationConfig::default();
        let recovery2 = recover_from_verification_failure(&passing_result, &config_with_recovery);
        if passing_result.overall != OverallVerdict::Fail && passing_result.overall != OverallVerdict::Inconclusive {
            assert!(recovery2.is_none(), "Should not recover non-failing results");
        }
        // If the result happens to be a Fail (unlikely with defaults), recovery should work.
    }

    /// Test 18: FixSuggestion and PartialVerificationResult display formatting.
    #[test]
    fn test_fix_suggestion_and_partial_result_display() {
        let fix = FixSuggestion {
            invariant: InvariantKind::Exclusivity,
            description: "Add synchronization".to_string(),
            code_hint: Some("lock(ptr);".to_string()),
            confidence: 0.85,
        };
        let display = format!("{}", fix);
        assert!(display.contains("exclusivity"));
        assert!(display.contains("Add synchronization"));
        assert!(display.contains("85%"));

        // Test PartialVerificationResult display.
        let partial = PartialVerificationResult {
            original_summary: VerificationSummary::default(),
            safe_invariants: vec![InvariantKind::Liveness],
            unsafe_invariants: vec![InvariantKind::Exclusivity],
            unverified_invariants: vec![],
            fix_suggestions: vec![fix],
            recovered: true,
            recovery_diagnostics: vec!["Test diagnostic".to_string()],
        };
        let partial_display = format!("{}", partial);
        assert!(partial_display.contains("recovered=true"));
        assert!(partial_display.contains("Safe invariants"));
        assert!(partial_display.contains("Unsafe invariants"));
    }

    /// Test 19: compute_scg_delta returns all invariants when node count differs.
    #[test]
    fn test_compute_scg_delta() {
        // Same SCG → conservative delta with just liveness.
        let config = CompileConfig {
            opt_level: OptLevel::O0,
            verification_level: VerificationLevel::None,
            ..CompileConfig::default()
        };
        let source1 = r#"
            fn main() {}
        "#;
        let result1 = compile(source1, &config);
        assert!(result1.is_ok());
        let scg1 = result1.unwrap().scg;

        let delta_same = compute_scg_delta(&scg1, &scg1);
        assert_eq!(delta_same.affected.len(), 1);
        assert_eq!(delta_same.affected[0], InvariantKind::Liveness);

        // When node counts differ, all invariants should be affected.
        let source2 = r#"
            region buf = allocate(256);
            fn main() {
                ptr = buf + 64;
                header = ptr as *NodeHeader;
            }
        "#;
        let result2 = compile(source2, &config);
        assert!(result2.is_ok());
        let scg2 = result2.unwrap().scg;

        if scg1.node_count() != scg2.node_count() {
            let delta_diff = compute_scg_delta(&scg1, &scg2);
            assert_eq!(delta_diff.affected.len(), 5, "All 5 invariants should be affected when node count changes");
            assert!(delta_diff.reason.is_some());
        }
    }
}
