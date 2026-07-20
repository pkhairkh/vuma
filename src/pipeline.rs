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
// Wave 9: parallel register allocation across functions.
// Replaced rayon with std::thread::scope (no external dep).
use std::fmt;
use std::path::Path;
use std::time::Instant;

// ── Workspace crate imports ──────────────────────────────────────────────

use vuma_bd::{repd::RepD, BD};
use vuma_codegen::{
    emit::{emit_binary, EmitConfig},
    ir::{BinOpKind as IrBinOpKind, IRFunction, IRProgram},
    regalloc::{AllocationResult, LinearScanAllocator},
    scg_to_ir::{
        AccessNode, AllocationNode, CallNode, CastNode, ComputationNode, ControlNode, GetAddressNode, IRBuilder,
        Scg, ScgData, ScgExpr, ScgFunction, ScgNode, ScgParam, ScgStatement, ScgType, StructAccessNode, SwitchArm, SyscallCallNode,
        ChannelOpenStmt, ChannelSendStmt, ChannelRecvStmt, ChannelCloseStmt, ChannelRecvResultStmt,
    },
    CastKind as CodegenCastKind, CodegenError, DataSectionKind,
};
// (Wave 32) Escape analysis + effect analysis are wired into the O2+
// codegen-opt stage.  We import the modules so the pipeline can call
// `escape_analysis::analyze_escapes_program`, drive SROA / alloc
// elision, and call `effects::analyze_program_effects` for
// interprocedural effect propagation.
use vuma_codegen::{effects, escape_analysis};

// Re-import the same types under their canonical (un-aliased) names so the
// moved AST→codegen SCG bridge functions (see end of this file) can use
// `BinOpKind` / `CastKind` exactly as they did in `src/main.rs`. Both names
// refer to the same types; the aliases above are kept for the existing
// pipeline-only code that already uses them.
use vuma_codegen::ir::BinOpKind;
use vuma_codegen::CastKind;
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
    DeadRegionElimination, EdgeData, EdgeKind, InliningPass,
    LoopInvariantCodeMotion, NodeData, NodeId, NodePayload, NodeType, PassManager,
    PipelineResult as ScgPipelineResult, SCG, SCGPass, StrengthReduction, TailCallOptDetection,
    ComputationKind,
};

// ═══════════════════════════════════════════════════════════════════════════
// Parallel map helper (std::thread::scope based — replaces rayon)
// ═══════════════════════════════════════════════════════════════════════════

/// Map a slice to a `Vec` in parallel using `std::thread::scope` with chunked
/// work distribution across the available CPU cores.
///
/// Preserves input order in the output (chunks are processed in order and the
/// per-chunk results are concatenated). Falls back to a sequential `iter().map()`
/// when the slice is empty or when only one thread is available, avoiding the
/// scope/thread-spawn overhead in those cases.
fn par_map<T, U, F>(items: &[T], f: F) -> Vec<U>
where
    T: Sync,
    U: Send,
    F: Fn(&T) -> U + Sync,
{
    let n = items.len();
    if n == 0 {
        return Vec::new();
    }
    let num_threads = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(1)
        .min(n);
    if num_threads == 1 {
        return items.iter().map(&f).collect();
    }
    let chunk_size = n.div_ceil(num_threads);
    std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(num_threads);
        for chunk in items.chunks(chunk_size) {
            let f_ref = &f;
            handles.push(s.spawn(move || chunk.iter().map(f_ref).collect::<Vec<U>>()));
        }
        let mut out = Vec::with_capacity(n);
        for h in handles {
            out.extend(h.join().expect("worker thread panicked"));
        }
        out
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// CompileConfig
// ═══════════════════════════════════════════════════════════════════════════

/// The compilation target platform.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default,
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
///
/// In VUMA 2.0, O3 is **mandatory** — every pipeline path runs the full
/// O3 pass set unconditionally (see `run_ir_pipeline` / `run_scg_transforms`).
/// The `O0`/`O1`/`O2` variants are retained for API stability and
/// serialised display, but the CLI rejects every non-O3 value and
/// `OptLevel::default()` returns `O3` so that any code path that forgets
/// to set the level explicitly still gets the mandatory O3 behaviour.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default,
)]
pub enum OptLevel {
    /// No optimisation — fastest compilation, best debuggability.
    O0,
    /// Basic optimisations (DCE, constant folding).
    O1,
    /// Full optimisations (DCE, CSE, constant folding, inlining).
    O2,
    /// Aggressive optimisations (O2 + inlining of larger functions).
    ///
    /// This is the VUMA 2.0 default — O3 is mandatory and every pipeline
    /// path runs the full O3 pass set regardless of this value.
    #[default]
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
///
/// VUMA 2.0 is PMT-only and verification is **MANDATORY** — there is no
/// `None`/"skip verification" variant. Every level maps to
/// `IveVerificationLevel::Pmt` (the 3 PMT state verifiers) at the IVE
/// stage, and the IVE gate is a hard compile error on `Fail`. The
/// `Quick`/`Normal`/`Exhaustive`/`Modular`/`ConstantTime`/`Hardened`
/// variants are retained for API stability and for the `--verification`
/// flag's error messages, but they all collapse to PMT state verification
/// in the pipeline. The `#[default]` is `Normal` (which maps to `Pmt`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default,
)]
pub enum VerificationLevel {
    /// Quick: only cheap syntactic checks.
    Quick,
    /// Normal: all five invariant checks.
    #[default]
    Normal,
    /// Exhaustive: all checks + formal proof attempts + interprocedural.
    Exhaustive,
    /// (Wave 16) Modular: core 5 + modular per-function verification.
    Modular,
    /// (Wave 16) ConstantTime: core 5 + constant-time (6th invariant).
    ConstantTime,
    /// (Wave 16) Hardened: all 6 invariants + interprocedural + modular.
    Hardened,
}

impl fmt::Display for VerificationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerificationLevel::Quick => write!(f, "quick"),
            VerificationLevel::Normal => write!(f, "normal"),
            VerificationLevel::Exhaustive => write!(f, "exhaustive"),
            VerificationLevel::Modular => write!(f, "modular"),
            VerificationLevel::ConstantTime => write!(f, "constant-time"),
            VerificationLevel::Hardened => write!(f, "hardened"),
        }
    }
}

/// Full compilation configuration.
#[derive(Debug, Clone)]
pub struct CompileConfig {
    /// Target platform.
    pub target: CompileTarget,
    /// Optimisation level.
    pub opt_level: OptLevel,
    /// Verification thoroughness.
    pub verification_level: VerificationLevel,
    /// (Wave 19) Treat `OverallVerdict::Inconclusive` as a compilation-
    /// blocking error. Default `false` (Inconclusive is allowed — it means
    /// "no violation proven, but not all invariants verified").
    pub strict_verification: bool,
    /// (Wave 19) Maximum number of paths explored by the liveness verifier
    /// before giving up (default 64). Higher values catch more bugs at the
    /// cost of slower verification.
    pub ive_max_paths: usize,
    /// (Wave 19) Maximum path length explored by the cleanup verifier
    /// before giving up (default 256). Higher values catch more leaks.
    pub ive_max_path_length: usize,
    /// Entry-point function name (default: "main" for hosted, "_start" for bare).
    pub entry_name: String,
    /// Include debug info in the output.
    pub debug_info: bool,
    /// Stop compilation at the first error.
    pub stop_on_first_error: bool,
    /// Maximum inline size (number of SCG nodes) for the inlining pass.
    pub max_inline_size: usize,
    /// (Wave 25) Cost threshold for the IR-level inliner
    /// (`opt::inline_with_threshold`). A callee whose
    /// `function_inline_cost` (per-instr cost + 2*arg_count -
    /// 3*const_arg_count) is ≤ this threshold gets inlined at the call
    /// site. Default 40 — generous enough to inline small helpers like
    /// `fn add_one(x) { x + 1 }` while preventing runaway code growth.
    pub inline_threshold: u32,
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
    /// through, defeating VUMA's core safety guarantee.  O3 is **mandatory**
    /// in VUMA 2.0 — every pipeline path runs the full O3 pass set regardless
    /// of the `opt_level` value in the config; the "fast" aspect of this
    /// preset comes only from enabling `debug_info`.
    pub fn debug() -> Self {
        Self {
            opt_level: OptLevel::O3,
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
            opt_level: OptLevel::O3,
            verification_level: VerificationLevel::Normal,
            strict_verification: false,
            ive_max_paths: 64,
            ive_max_path_length: 256,
            entry_name: "main".to_string(),
            debug_info: false,
            stop_on_first_error: true,
            max_inline_size: 50,
            inline_threshold: vuma_codegen::opt::DEFAULT_INLINE_THRESHOLD,
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
    /// Memory-safety analysis failure (Wave 20 — blocking pass).
    ///
    /// Emitted when `MemorySafetyAnalyzer` or `analyze_with_scg_liveness`
    /// detects a use-after-free, double-free, memory leak, or uninitialized
    /// read and `CompileConfig.memory_safety` is `true` (the default).
    /// This is a hard gate: the pipeline refuses to emit code for programs
    /// with known memory-safety violations, independent of
    /// `stop_on_first_error`.
    MemorySafety {
        /// The memory-safety report containing the violations.
        report: vuma_codegen::memory_safety::MemorySafetyReport,
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
            VumaError::MemorySafety { .. } => "memory-safety",
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
            VumaError::MemorySafety { report } => {
                write!(f, "[memory-safety] {} violation(s) found", report.violations.len())?;
                for v in &report.violations {
                    write!(f, "\n  - {}", v)?;
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
    Success(Box<CompilationOutput>),
    /// Compilation failed, but partial results are available.
    Partial(Box<PartialCompilationOutput>),
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
                    // For "param <name>" nodes, return Var("<name>") so the
                    // IR builder can resolve it via its names map (which
                    // registers params by their real name).
                    if let Some(param_name) = label.strip_prefix("param ") {
                        let param_name = param_name.trim();
                        if !param_name.is_empty()
                            && param_name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                            && param_name.chars().all(|c| c.is_alphanumeric() || c == '_')
                        {
                            return ScgExpr::Var(param_name.to_string());
                        }
                    }
                    if let Some(num_str) = label.strip_prefix("lit_") {
                        if let Ok(num) = num_str.parse::<i64>() {
                            return ScgExpr::Int(num);
                        }
                        // Boolean literals: lit_true -> 1, lit_false -> 0
                        if num_str == "true" {
                            return ScgExpr::Int(1);
                        }
                        if num_str == "false" {
                            return ScgExpr::Int(0);
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
                        // For variable reference nodes (label is just "result"),
                        // return Var(label) so IR builder uses names[label].
                        //
                        // BUT: only do this if the source node's DataFlow
                        // predecessors do NOT include an assignment
                        // (label with " = " or starting with "let "). If the
                        // variable was defined by a Load/Allocation/assignment,
                        // the vreg (Var("v_N")) is the correct reference —
                        // using Var(label) would be wrong in if/else bodies
                        // where the variable has different values on different
                        // paths, because names[label] might point to the wrong
                        // path's value.
                        //
                        // The Var(label) path is only safe for pure variable
                        // references in match/Switch contexts, where the
                        // variable's value comes from a phi/merge node (not
                        // an assignment).
                        let is_var_ref = !(!label.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                            || !label.chars().all(|c| c.is_alphanumeric() || c == '_')
                            || label.starts_with("lit_")
                            || label.starts_with("param ")
                            || label.starts_with("match_arm")
                            || label.starts_with("v_") && label[2..].chars().all(|c| c.is_ascii_digit()))
                            && !label.starts_with("const ")
                            && !label.contains(" = ")
                            && !label.contains("(");
                        if is_var_ref {
                            // Check if any DataFlow predecessor is an assignment
                            // (has " = " in its label). If so, fall through to
                            // the vreg-based resolution.
                            let has_assignment_predecessor = edge_idx
                                .incoming
                                .get(&source)
                                .map(|edges| {
                                    edges.iter().any(|e| {
                                        if e.kind == EdgeKind::DataFlow {
                                            if let Some(pred_data) = scg.get_node(e.source) {
                                                if let NodePayload::Computation(pred_comp) = &pred_data.payload {
                                                    if let ComputationKind::Other(ref pred_label) = pred_comp.kind {
                                                        return pred_label.contains(" = ")
                                                            || pred_label.starts_with("let ");
                                                    }
                                                }
                                            }
                                        }
                                        false
                                    })
                                })
                                .unwrap_or(false);
                            if !has_assignment_predecessor {
                                return ScgExpr::Var(label.clone());
                            }
                            // Fall through to vreg-based resolution
                        }
                        // For "param <name>" nodes, return Var("<name>")
                        if let Some(param_name) = label.strip_prefix("param ") {
                            let param_name = param_name.trim();
                            if !param_name.is_empty()
                                && param_name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                                && param_name.chars().all(|c| c.is_alphanumeric() || c == '_')
                            {
                                return ScgExpr::Var(param_name.to_string());
                            }
                        }
                        if let Some(num_str) = label.strip_prefix("lit_") {
                            if let Ok(num) = num_str.parse::<i64>() {
                                return ScgExpr::Int(num);
                            }
                            // Boolean literals: lit_true -> 1, lit_false -> 0
                            if num_str == "true" {
                                return ScgExpr::Int(1);
                            }
                            if num_str == "false" {
                                return ScgExpr::Int(0);
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
///
/// Returns the condition as an `ScgExpr` AND a list of `ScgStatement`s
/// (typically `Call` statements) that must be emitted **before** the
/// branch.  These pre-statements are produced when the condition contains
/// inline function calls like `if (is_space(c) == 1)` — the call
/// `is_space(c)` has no dedicated Computation node in the SCG (it only
/// appears in the Branch label), so we extract it here and emit it as a
/// separate `Call` statement whose result vreg is then referenced by the
/// condition expression.
fn resolve_branch_cond(
    branch_id: NodeId,
    edge_idx: &EdgeIndex,
    scg: &SCG,
    extern_functions: &HashSet<String>,
) -> (ScgExpr, Vec<ScgStatement>) {
    // First, try to parse the branch label (e.g., "if (a > b)")
    // to extract the condition expression.
    if let Some(node_data) = scg.get_node(branch_id) {
        if let NodePayload::Control(ctrl) = &node_data.payload {
            if let Some(label) = &ctrl.label {
                // Strip "if " prefix and outer parentheses
                let cond_str = label.trim();
                let cond_str = cond_str.strip_prefix("if").unwrap_or(cond_str).trim();
                let cond_str = cond_str.strip_prefix('(').unwrap_or(cond_str);
                let cond_str = cond_str.strip_suffix(')').unwrap_or(cond_str);
                let cond_str = cond_str.trim();

                let df_inputs = edge_idx.incoming_df(branch_id);
                let sources: Vec<NodeId> = df_inputs.iter().map(|e| e.source).collect();

                // ── Extract inline function calls from the condition ──
                //
                // The condition string may contain function calls like
                // `is_space(c)` that have no dedicated SCG Computation node.
                // `extract_calls_from_label` scans the string, emits a
                // `Call` statement for each call, and replaces the call
                // text with a vreg reference (e.g. `v_N_call_0`).  The
                // returned `Call` statements must be emitted before the
                // branch so the vreg is populated.
                let (cond_no_calls, pre_calls) = extract_calls_from_label(
                    cond_str,
                    branch_id,
                    &sources,
                    edge_idx,
                    scg,
                    extern_functions,
                );

                // Try to parse as a comparison expression
                if let Some((op, lhs_str, rhs_str)) = parse_expr_split(&cond_no_calls) {
                    let lhs = resolve_subexpr(&lhs_str, &sources, edge_idx, scg);
                    let rhs = resolve_subexpr(&rhs_str, &sources, edge_idx, scg);
                    return (
                        ScgExpr::BinOp {
                            op: map_binop_kind(op),
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        },
                        pre_calls,
                    );
                }

                // For "if true" or "if false", return Int(1) or Int(0)
                if cond_no_calls == "true" {
                    return (ScgExpr::Int(1), pre_calls);
                }
                if cond_no_calls == "false" {
                    return (ScgExpr::Int(0), pre_calls);
                }

                // For simple variable conditions, resolve via DataFlow
                let is_valid_var = cond_no_calls.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                    && cond_no_calls.chars().all(|c| c.is_alphanumeric() || c == '_');
                if is_valid_var {
                    return (
                        resolve_subexpr(&cond_no_calls, &sources, edge_idx, scg),
                        pre_calls,
                    );
                }
            }
        }
    }

    // Fallback: use the first DataFlow input
    (resolve_df_input(branch_id, 0, edge_idx, scg), Vec::new())
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
fn resolve_loop(header_id: NodeId, scg: &SCG, edge_idx: &EdgeIndex) -> (NodeId, Option<NodeId>, Option<NodeId>) {
    let cf_edges = edge_idx.outgoing_cf(header_id);

    let mut body_target = None;
    let mut exit_target = None;
    let mut after_loop_target = None;

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
        } else if after_loop_target.is_none() {
            after_loop_target = Some(edge.target);
        }
    }

    if body_target.is_none() {
        body_target = cf_edges.first().map(|e| e.target);
    }
    if exit_target.is_none() && cf_edges.len() > 1 {
        exit_target = cf_edges.get(1).map(|e| e.target);
    }

    (body_target.unwrap_or(header_id), exit_target, after_loop_target)
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
                    let (cond, pre_calls) = resolve_branch_cond(node_id, edge_idx, scg, extern_functions);

                    // Emit any pre-branch Call statements (extracted from
                    // inline function calls in the condition, e.g.
                    // `if (is_space(c) == 1)`).  These must execute before
                    // the branch so the condition can read the call's result.
                    stmts.extend(pre_calls);

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
                        // If the Join has no outgoing CF edges (a known SCG
                        // pattern where the "after-if" code is chained directly
                        // from the Branch node rather than from the Join),
                        // fall back to the Branch's other CF edges to find the
                        // continuation. We skip the then/else targets, the
                        // Join itself, and any already-consumed nodes.
                        if current.is_none() {
                            current = edge_idx.outgoing_cf(node_id)
                                .iter()
                                .map(|e| e.target)
                                .find(|&t| {
                                    t != join
                                        && t != then_tgt
                                        && else_tgt != Some(t)
                                        && !consumed.contains(&t)
                                });
                        }
                    } else {
                        current = None;
                    }
                    continue;
                }

                ControlKind::LoopHeader => {
                    let (body_tgt, exit_tgt, after_loop_tgt) = resolve_loop(node_id, scg, edge_idx);

                    // Stop the body walk at back-edges (LoopHeader) and LoopExit
                    let mut loop_stop = stop_at.clone();
                    loop_stop.insert(node_id); // back-edge target
                    if let Some(exit) = exit_tgt {
                        loop_stop.insert(exit);
                    }
                    // Also stop at the after-loop target so the body walk
                    // doesn't consume it (it belongs to the enclosing scope).
                    if let Some(after) = after_loop_tgt {
                        loop_stop.insert(after);
                    }

                    let body = walk_control_flow_with_externs(body_tgt, scg, edge_idx, consumed, &loop_stop, extern_functions);

                    // ── While-loop → for-range conversion ──
                    //
                    // While-loops in this SCG have no exit condition
                    // (while_cond=None, for_range=None), making them infinite.
                    // The LoopHeader label is e.g. "while (i < 4)".  We try to
                    // convert this to a for-range (var, start, end) so that
                    // lower_loop emits the counter init, condition check, AND
                    // increment — making the loop terminate without needing
                    // the after-loop code (which is unreachable via CF).
                    //
                    // If conversion fails (complex condition, non-literal
                    // bound, etc.), fall back to the while-condition guard
                    // (If + Break at the start of the body).
                    let mut for_range = ctrl.label.as_ref().and_then(|label| parse_for_range(label));
                    let mut needs_guard = false;
                    if for_range.is_none() {
                        // Try to parse a while-loop condition into a for-range.
                        // CAUTION: The for-range conversion creates a separate
                        // loop counter that is NOT updated when the body
                        // modifies the loop variable (e.g. `while i < 8 { ...
                        // i = 8; }`). This causes the loop to not exit early,
                        // leading to out-of-bounds access and memory corruption
                        // on 32-bit backends (arm32, x86_32) where stack
                        // allocations are adjacent.
                        //
                        // We check if the body has ANY reassignment. If so,
                        // we skip the for-range conversion and use the
                        // while-condition guard (Break) instead, which
                        // correctly handles variable reassignment.
                        if let Some(label) = &ctrl.label {
                            if let Some(fr) = parse_while_to_for_range(node_id, label, edge_idx, scg) {
                                // Check if the body reassigns any variable.
                                // If so, it might reassign the loop variable,
                                // so use the guard to be safe.
                                let body_has_reassigns = body_has_any_reassigns(&body);
                                if body_has_reassigns {
                                    needs_guard = true;
                                } else {
                                    for_range = Some(fr);
                                }
                            }
                            if for_range.is_none() && !needs_guard {
                                if let Some((_neg_cond, _pre_calls)) = parse_while_condition(node_id, label, edge_idx, scg, extern_functions) {
                                    needs_guard = true;
                                }
                            }
                        }
                    }
                    let body = if needs_guard {
                        let mut b = body;
                        if let Some(label) = &ctrl.label {
                            if let Some((neg_cond, pre_calls)) = parse_while_condition(node_id, label, edge_idx, scg, extern_functions) {
                                // Emit any pre-guard Call statements (extracted
                                // from inline function calls in the while
                                // condition, e.g. `while (is_space(c) == 0)`).
                                // These must execute BEFORE the guard so the
                                // condition can read the call's result.
                                //
                                // We insert the pre-calls first (at position 0,
                                // in order), then insert the guard If AFTER
                                // them (at position = number of pre-calls).
                                // The previous code inserted the guard If at
                                // position 0 AFTER the pre-calls, placing it
                                // BEFORE them — causing the guard to read
                                // uninitialized/zero values instead of the
                                // call results.
                                let n_pre = pre_calls.len();
                                for call_stmt in pre_calls {
                                    b.insert(0, call_stmt);
                                }
                                b.insert(n_pre, ScgStatement::Control(ControlNode::If {
                                    cond: neg_cond,
                                    then_body: vec![ScgStatement::Control(ControlNode::Break)],
                                    else_body: None,
                                }));
                            }
                        }
                        b
                    } else {
                        body
                    };

                    stmts.push(ScgStatement::Control(ControlNode::Loop { body, for_range, while_cond: None }));

                    // Continue from the statement AFTER the loop.
                    //
                    // The SCG construction (see `to_scg::convert_block_ids` +
                    // `Stmt::For`/`Stmt::While`) adds a ControlFlow edge from
                    // the LoopHeader to the *next sibling statement* in the
                    // enclosing block (this is `after_loop_tgt` returned by
                    // `resolve_loop`).  The LoopExit node itself has NO
                    // outgoing CF edges, so following `outgoing_cf(exit)` —
                    // as the previous code did — always yields `None` and
                    // silently drops every statement that comes after the
                    // loop (e.g. `i = i - 1` at the end of a while body, or
                    // `ten: u32 = 10; return n - ten;` after the outer loop).
                    //
                    // This was the root cause of the nested-loop timeouts:
                    // the inner loop's back-edge variable (e.g. `i`) was
                    // never decremented because `i = i - 1` lived after the
                    // inner loop and was discarded, turning the outer loop
                    // into an infinite loop.
                    //
                    // Fix: prefer `after_loop_tgt`.  Fall back to the
                    // LoopExit's outgoing edge for SCG variants that wire it
                    // up (defensive — current codegen never does).
                    if let Some(exit) = exit_tgt {
                        consumed.insert(exit);
                    }
                    if let Some(after) = after_loop_tgt {
                        current = Some(after);
                    } else if let Some(exit) = exit_tgt {
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
                            .map(|(i, _e)| resolve_df_input(node_id, i, edge_idx, scg))
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

                            // Find the caller Computation node
                            let caller_node = edge_idx.incoming
                                .get(&node_id)
                                .and_then(|edges| edges.iter().find(|e| e.kind == EdgeKind::ControlFlow))
                                .map(|e| e.source);

                            let df_inputs = edge_idx.incoming_df(node_id);
                            let mut sources: Vec<NodeId> = df_inputs.iter().map(|e| e.source).collect();

                            // CRITICAL: Exclude the caller node from sources.
                            // The caller's label (e.g. "let val = read_u32_be(block, i * 4)")
                            // contains variable names like "i". Without this filter,
                            // resolve_subexpr would match the caller node when resolving
                            // "i" in a subsequent call like "w_store(w, i, val)", causing
                            // the loop variable to be replaced with the caller's result.
                            if let Some(caller) = caller_node {
                                sources.retain(|&s| s != caller);
                            }

                            // Parse arguments from the caller's label.
                            // The AST→SCG converter stores the call expression as
                            // a string label, and DataFlow edges connect individual
                            // variables rather than computed sub-expressions.
                            //
                            // CRITICAL: Arguments may contain nested function calls
                            // (e.g. write_u32_be(state, 0, (read_u32_be(state, 0) + a) & mask)).
                            // We must extract these nested calls into separate Call
                            // statements BEFORE resolving the argument expression,
                            // otherwise resolve_subexpr would silently replace the
                            // call with Int(0), producing wrong results.
                            let mut nested_call_stmts: Vec<ScgStatement> = Vec::new();
                            let args: Vec<ScgExpr> = if let Some(caller) = caller_node {
                                if let Some(caller_data) = scg.get_node(caller) {
                                    if let NodePayload::Computation(comp) = &caller_data.payload {
                                        let caller_label = comp.kind.label();
                                        if let Some(expr) = extract_call_expr_from_label(&caller_label, func_name) {
                                            let arg_strs = parse_call_args(&expr);
                                            arg_strs.iter()
                                                .map(|a| {
                                                    // Extract nested function calls from this argument
                                                    let (modified_a, mut calls) = extract_calls_from_label(
                                                        a, node_id, &sources, edge_idx, scg, extern_functions,
                                                    );
                                                    nested_call_stmts.append(&mut calls);
                                                    resolve_subexpr(&modified_a, &sources, edge_idx, scg)
                                                })
                                                .collect()
                                        } else {
                                            collect_args_from_df(&df_inputs, scg, edge_idx)
                                        }
                                    } else {
                                        collect_args_from_df(&df_inputs, scg, edge_idx)
                                    }
                                } else {
                                    collect_args_from_df(&df_inputs, scg, edge_idx)
                                }
                            } else {
                                collect_args_from_df(&df_inputs, scg, edge_idx)
                            };
                            // Emit any extracted nested Call statements BEFORE
                            // the outer call. This ensures the nested calls
                            // execute first and their results are available
                            // as vreg references in the outer call's arguments.
                            stmts.append(&mut nested_call_stmts);

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
                            // Extract user-visible variable name from the
                            // caller's label (e.g. "let a = read_u32_be(...)"
                            // → reassigns = Some("a")). This is critical for
                            // phi resolution: without it, the let-binding's
                            // dst would be a synthetic name (v_N) and the
                            // user name ("a") would not be in the names map.
                            // When a subsequent reassignment (e.g. "a = t1+t2")
                            // updates names["a"], the phi for "v_N" would not
                            // see the update, causing the back-edge value to
                            // be self-referential and the loop to not propagate
                            // the new value.
                            let reassigns = if let Some(caller) = caller_node {
                                if let Some(caller_data) = scg.get_node(caller) {
                                    if let NodePayload::Computation(comp) = &caller_data.payload {
                                        let label = comp.kind.label();
                                        let (_expr, user_var) = strip_assignment_prefix(&label);
                                        user_var
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            stmts.push(ScgStatement::Call(CallNode {
                                dst: call_dst,
                                func: func_name.to_string(),
                                args,
                                is_extern,
                                reassigns,
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

                ControlKind::Switch => {
                    // Switch node: collect all SwitchCase children via Dispatch edges,
                    // walk each arm body, and emit a ControlNode::Switch statement.
                    //
                    // SCG structure for match:
                    //   Switch --Dispatch--> SwitchCase("case 0: <pattern>")
                    //   SwitchCase --CF--> body_computation --CF--> Join
                    //   Switch --Dispatch--> SwitchCase("case 1: ...")
                    //   ...
                    //   Switch --CF--> Join (default/fall-through)
                    //
                    // We convert this to:
                    //   ScgStatement::Control(ControlNode::Switch {
                    //       discriminant: <subject expr>,
                    //       arms: vec![SwitchArm { value: <case_value>, body: <arm_body> }, ...],
                    //       default_body: <default_body>,
                    //   })

                    // Parse the discriminant from the Switch label: "match <subject>"
                    // Use the Switch node's DataFlow inputs as sources so that
                    // variable references in the subject expression resolve
                    // correctly.
                    let df_sources: Vec<NodeId> = edge_idx.incoming_df(node_id)
                        .iter().map(|e| e.source).collect();
                    let disc_expr = if let Some(ref label) = ctrl.label {
                        let s = label.trim();
                        if let Some(subject_str) = s.strip_prefix("match ") {
                            resolve_subexpr(subject_str, &df_sources, edge_idx, scg)
                        } else {
                            ScgExpr::Int(0)
                        }
                    } else {
                        ScgExpr::Int(0)
                    };

                    // Collect Dispatch edges to find all SwitchCase nodes
                    let dispatch_edges: Vec<_> = edge_idx.outgoing
                        .get(&node_id)
                        .map(|edges| edges.iter().filter(|e| e.kind == EdgeKind::Dispatch).collect())
                        .unwrap_or_default();

                    // Find the Join node (first CF successor that is a Join)
                    let join_node = edge_idx.outgoing_cf(node_id)
                        .iter()
                        .find(|e| {
                            if let Some(n) = scg.get_node(e.target) {
                                if let NodePayload::Control(c) = &n.payload {
                                    return c.kind == ControlKind::Join;
                                }
                            }
                            false
                        })
                        .map(|e| e.target);

                    // Build arm_stop set: includes the Join node
                    let mut arm_stop: HashSet<NodeId> = stop_at.clone();
                    if let Some(join) = join_node {
                        arm_stop.insert(join);
                    }

                    let mut arms: Vec<SwitchArm> = Vec::new();
                    let _arm_bodies: Vec<Vec<ScgStatement>> = Vec::new();

                    let mut default_body: Vec<ScgStatement> = Vec::new();

                    for dedge in &dispatch_edges {
                        let case_node = dedge.target;
                        if let Some(case_data) = scg.get_node(case_node) {
                            if let NodePayload::Control(case_ctrl) = &case_data.payload {
                                if case_ctrl.kind == ControlKind::SwitchCase {
                                    // Extract case value from label: "case N: <pattern>"
                                    let pattern_str = if let Some(ref label) = case_ctrl.label {
                                        let s = label.trim();
                                        if let Some(colon_pos) = s.find(':') {
                                            s[colon_pos + 1..].trim().to_string()
                                        } else {
                                            String::new()
                                        }
                                    } else {
                                        String::new()
                                    };

                                    // Walk the arm body from the SwitchCase to the Join
                                    let body = walk_control_flow_with_externs(
                                        case_node,
                                        scg,
                                        edge_idx,
                                        consumed,
                                        &arm_stop,
                                        extern_functions,
                                    );

                                    // Check if this is a wildcard/default arm
                                    if pattern_str == "_" || pattern_str.is_empty() {
                                        // Wildcard arm -> default_body
                                        default_body = body;
                                    } else {
                                        // Regular arm with integer value
                                        let case_value = pattern_str.parse::<i64>().unwrap_or(0);
                                        arms.push(SwitchArm {
                                            value: case_value,
                                            body,
                                        });
                                    }
                                }
                            }
                        }
                    }

                    // Consume the Join node
                    if let Some(join) = join_node {
                        consumed.insert(join);
                    }

stmts.push(ScgStatement::Control(ControlNode::Switch {
                        discriminant: disc_expr,
                        arms,
                        default_body,
                    }));

                    // Continue from after the Join node.
                    // If the Join has no CF successors (common in SCG layout
                    // where post-match code is connected to the Switch, not
                    // the Join), fall back to the Switch's CF successors
                    // (skipping Dispatch edges and already-consumed nodes).
                    if let Some(join) = join_node {
                        current = edge_idx.outgoing_cf(join).first().map(|e| e.target);
                    }
                    if current.is_none() {
                        // Join has no CF successors — try Switch's CF successors
                        current = edge_idx.outgoing_cf(node_id)
                            .iter()
                            .map(|e| e.target)
                            .find(|&t| {
                                !consumed.contains(&t)
                                    && t != join_node.unwrap_or(t)
                            });
                    }
                    if current.is_none() {
                        // Still no successor — try any outgoing edge from Switch
                        current = edge_idx.outgoing
                            .get(&node_id)
                            .and_then(|edges| edges.iter()
                                .filter(|e| e.kind == EdgeKind::ControlFlow)
                                .map(|e| e.target)
                                .find(|&t| !consumed.contains(&t) && t != join_node.unwrap_or(t)));
                    }
                    continue;
                }

                ControlKind::SwitchCase => {
                    // SwitchCase nodes are handled by the Switch handler above.
                    // If we reach here directly, it means the SwitchCase wasn't
                    // consumed by a Switch (shouldn't happen in well-formed SCG).
                    // Skip it and continue.
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
                let node_stmts = convert_node_to_statement_with_externs(node_id, node_data, edge_idx, scg, extern_functions);
                stmts.extend(node_stmts);

                // Continue to the next node via ControlFlow
                current = edge_idx.outgoing_cf(node_id).first().map(|e| e.target);
            }
        }
    }

    stmts
}

// ── Node-to-statement conversion ───────────────────────────────────────

/// Scan an expression label for function-call patterns like
/// `func_name(arg1, arg2, ...)`.  For each call found:
///
/// 1. Parse the function name and arguments (respecting nested parens).
/// 2. Resolve each argument as an `ScgExpr` via `resolve_subexpr`.
/// 3. Emit a `Call` statement with a unique destination variable
///    (e.g. `v_13_call_0`).
/// 4. Replace the call expression in the label with a reference to that
///    destination variable (so the surrounding expression can refer to it).
///
/// Returns `(modified_label, call_statements)`.
///
/// This is what lets recursive calls inside expressions (e.g.
/// `fib_recursive(n - 1) + fib_recursive(n - 2)`) be lowered correctly.
/// The AST→SCG converter doesn't create call-site `FunctionEntry` nodes
/// for calls inside expressions; it stores the entire expression as a
/// string label on a single `Computation` node.  Without this extraction,
/// `resolve_subexpr` would silently turn each call into `Int(0)`.
fn extract_calls_from_label(
    label: &str,
    node_id: NodeId,
    sources: &[NodeId],
    edge_idx: &EdgeIndex,
    scg: &SCG,
    extern_functions: &HashSet<String>,
) -> (String, Vec<ScgStatement>) {
    let mut calls: Vec<ScgStatement> = Vec::new();
    let mut result = String::with_capacity(label.len());
    let bytes = label.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i] as char;

        // Identifier start: alphabetic or underscore
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
            {
                i += 1;
            }
            let ident = &label[start..i];

            // Skip whitespace
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }

            // If next non-space char is '(', treat as a function call
            if j < bytes.len() && bytes[j] == b'(' {
                // Skip VUMA keywords that look like calls (e.g. "if (cond)")
                if matches!(
                    ident,
                    "if" | "while" | "for" | "return" | "match" | "let"
                        | "else" | "fn" | "struct" | "enum" | "true" | "false"
                        | "None" | "null" | "nullptr" | "as" | "in" | "where"
                        | "sizeof" | "alignof" | "typeof"
                ) {
                    result.push_str(ident);
                    i = j;
                    continue;
                }

                // Skip built-in runtime functions that have special
                // lowering paths (Heap allocation, atomic ops, etc.).
                // These are handled by Derivation-edge detection or the
                // atomic_load/atomic_store label checks above.
                if matches!(
                    ident,
                    "allocate" | "free" | "__vuma_alloc" | "__vuma_free"
                        | "atomic_load" | "atomic_store" | "atomic_cas"
                        | "AtomicLoad" | "AtomicStore" | "AtomicCas"
                ) {
                    result.push_str(ident);
                    i = j;
                    continue;
                }

                // Find the matching ')'
                let mut depth: i32 = 1;
                let args_start = j + 1;
                let mut k = j + 1;
                while k < bytes.len() && depth > 0 {
                    if bytes[k] == b'(' {
                        depth += 1;
                    } else if bytes[k] == b')' {
                        depth -= 1;
                    }
                    if depth > 0 {
                        k += 1;
                    }
                }

                if depth == 0 {
                    let args_str = &label[args_start..k];
                    let call_end = k + 1; // include the ')'

                    // Recursively extract calls from the argument string.
                    // This handles nested calls like `f(g(x))` by first
                    // extracting `g(x)` into a vreg, then passing that
                    // vreg as the argument to `f`.
                    let (modified_args_str, mut nested_calls) =
                        extract_calls_from_label(
                            args_str,
                            node_id,
                            sources,
                            edge_idx,
                            scg,
                            extern_functions,
                        );
                    calls.append(&mut nested_calls);

                    // Parse the (now call-free) arguments
                    let args = parse_call_args(&modified_args_str);

                    // Resolve each argument as an ScgExpr
                    let arg_exprs: Vec<ScgExpr> = args
                        .iter()
                        .map(|a| resolve_subexpr(a, sources, edge_idx, scg))
                        .collect();

                    // Create a unique destination variable for this call.
                    let call_idx = calls.len();
                    let dst = format!("v_{}_call_{}", node_id.as_u64(), call_idx);

                    let is_extern = extern_functions.contains(ident);
                    calls.push(ScgStatement::Call(CallNode {
                        dst: Some(dst.clone()),
                        func: ident.to_string(),
                        args: arg_exprs,
                        is_extern,
                        reassigns: None,
                    }));

                    // Replace the call expression with the destination variable
                    result.push_str(&dst);

                    i = call_end;
                    continue;
                }
            }

            // Not a call — copy the identifier as-is
            result.push_str(ident);
            continue;
        }

        // Copy other characters as-is
        result.push(c);
        i += 1;
    }

    (result, calls)
}

/// Extract the argument-expression string from a caller label.
/// Given `ackermann((m - one), one)` and func_name `ackermann`,
/// returns `(m - one), one`.
fn extract_call_expr_from_label(label: &str, func_name: &str) -> Option<String> {
    let search = format!("{}(", func_name);
    let pos = label.find(&search)?;
    let args_start = pos + search.len();
    let bytes = label.as_bytes();
    let mut depth: i32 = 1;
    let mut i = args_start;
    while i < bytes.len() && depth > 0 {
        if bytes[i] == b'(' { depth += 1; }
        else if bytes[i] == b')' { depth -= 1; }
        if depth > 0 { i += 1; }
    }
    if depth == 0 { Some(label[args_start..i].to_string()) } else { None }
}

/// Collect call arguments from DataFlow edges (fallback).
fn collect_args_from_df(
    df_inputs: &[&vuma_scg::EdgeData],
    scg: &SCG,
    _edge_idx: &EdgeIndex,
) -> Vec<ScgExpr> {
    let mut args = Vec::new();
    for df_edge in df_inputs {
        let source = df_edge.source;
        if let Some(src_data) = scg.get_node(source) {
            if let NodePayload::Computation(comp) = &src_data.payload {
                if let ComputationKind::Other(ref lbl) = comp.kind {
                    if let Some(param_name) = lbl.strip_prefix("param ") {
                        let pn = param_name.trim();
                        if !pn.is_empty()
                            && pn.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                            && pn.chars().all(|c| c.is_alphanumeric() || c == '_')
                        {
                            args.push(ScgExpr::Var(pn.to_string()));
                            continue;
                        }
                    }
                    if let Some(num_str) = lbl.strip_prefix("lit_") {
                        if let Ok(num) = num_str.parse::<i64>() {
                            args.push(ScgExpr::Int(num));
                            continue;
                        }
                    }
                    match lbl.as_str() {
                        "true" => { args.push(ScgExpr::Int(1)); continue; }
                        "false" => { args.push(ScgExpr::Int(0)); continue; }
                        "None" | "null" | "nullptr" => { args.push(ScgExpr::Int(0)); continue; }
                        _ => {}
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
    args
}

/// Parse function-call arguments: comma-separated, respecting nested parens.
fn parse_call_args(args_str: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth: i32 = 0;

    for b in args_str.bytes() {
        if b == b'(' {
            depth += 1;
            current.push(b as char);
        } else if b == b')' {
            depth -= 1;
            current.push(b as char);
        } else if b == b',' && depth == 0 {
            let arg = current.trim().to_string();
            if !arg.is_empty() {
                args.push(arg);
            }
            current.clear();
        } else {
            current.push(b as char);
        }
    }

    let arg = current.trim().to_string();
    if !arg.is_empty() {
        args.push(arg);
    }

    args
}

/// Convert a non-control SCG node into an `ScgStatement`, with knowledge
/// of which functions are declared as extern.
///
/// Returns a `Vec<ScgStatement>` because some nodes (notably `Computation`
/// nodes whose label contains function calls inside expressions) need to
/// emit additional `Call` statements before the main statement.  For
/// example, the label `fib(n-1) + fib(n-2)` produces two `Call` statements
/// followed by a `Computation` that references the call destinations.
///
/// Handles all node types except `Control` (which is handled by
/// `walk_control_flow`) and `Phantom` (which is skipped).
fn convert_node_to_statement_with_externs(
    node_id: NodeId,
    node_data: &NodeData,
    edge_idx: &EdgeIndex,
    scg: &SCG,
    extern_functions: &HashSet<String>,
) -> Vec<ScgStatement> {
    // Helper: wrap a single optional statement into a Vec
    fn single(stmt: Option<ScgStatement>) -> Vec<ScgStatement> {
        stmt.into_iter().collect()
    }

    match &node_data.payload {
        NodePayload::Allocation(alloc) => {
            let ty = alloc
                .type_name
                .as_deref()
                .and_then(parse_scg_type)
                .unwrap_or(ScgType::U8);
            if alloc.size == 0 {
                let size_expr = resolve_df_input(node_id, 0, edge_idx, scg);
                if !matches!(size_expr, ScgExpr::Int(0)) {
                    return single(Some(ScgStatement::Allocation(AllocationNode::Heap {
                        name: node_var(node_id, "alloc"),
                        size_expr,
                        ty,
                    })));
                }
            }
            single(Some(ScgStatement::Allocation(AllocationNode::Stack {
                name: node_var(node_id, "alloc"),
                size: alloc.size as u32,
                ty,
            })))
        }

        NodePayload::Access(access) => match access.mode {
            AccessMode::Read => {
                // PMT (Wave 2): when both `offset` and `access_size` are set
                // (state-field reads), use `access_size` to pick the Load's IR
                // type. For legacy Access nodes (raw deref), `access_size`
                // reflects the pointer size (8) not the value size, so we
                // still fall through to None and let the IR builder infer.
                let ty = if access.offset.is_some() {
                    access.access_size.and_then(access_size_to_ir_type)
                } else {
                    None
                };
                single(Some(ScgStatement::Access(AccessNode::Load {
                    ty,
                    dst: node_var(node_id, "val"),
                    ptr: resolve_df_input(node_id, 0, edge_idx, scg),
                    offset: access.offset.map(|o| ScgExpr::Int(o as i64)),
                })))
            }
            AccessMode::Write | AccessMode::ReadWrite => {
                {
                    // PMT (Wave 2): same as Read — use `access_size` for the
                    // Store's IR type when both offset and access_size are set.
                    let ty = if access.offset.is_some() {
                        access.access_size.and_then(access_size_to_ir_type)
                    } else {
                        None
                    };
                    single(Some(ScgStatement::Access(AccessNode::Store {
                        ptr: resolve_df_input(node_id, 0, edge_idx, scg),
                        offset: access.offset.map(|o| ScgExpr::Int(o as i64)),
                        value: resolve_df_input(node_id, 1, edge_idx, scg),
                        ty,
                    })))
                }
            }
        },

        NodePayload::Computation(comp) => {
            convert_computation_node(node_id, comp, edge_idx, scg, extern_functions)
        }

        NodePayload::Cast(cast) => {
            let to_ty = parse_scg_type(&cast.to_type).unwrap_or(ScgType::Ptr);
            let from_ty = parse_scg_type(&cast.from_type).unwrap_or(ScgType::Ptr);
            single(Some(ScgStatement::Cast(CastNode {
                dst: node_var(node_id, "cast"),
                src: resolve_df_input(node_id, 0, edge_idx, scg),
                kind: if cast.is_lossless {
                    CodegenCastKind::ZExt
                } else {
                    CodegenCastKind::BitCast
                },
                from_ty,
                to_ty,
            })))
        }

        NodePayload::Deallocation(_dealloc) => Vec::new(),

        NodePayload::Effect(eff) => {
            let is_extern = extern_functions.contains(&eff.effect_kind);
            single(Some(ScgStatement::Call(CallNode {
                dst: Some(node_var(node_id, "eff")),
                func: eff.effect_kind.clone(),
                args: vec![],
                is_extern,
                reassigns: None,
            })))
        }

        NodePayload::Syscall(syscall) => {
            // Wave 10: lower a syscall to a first-class
            // `ScgStatement::Syscall`. The `IRBuilder` lowers this to
            // `IRInstr::Syscall`, which each backend lowers directly to a
            // real syscall instruction (Wave 11/12 removed the intermediate
            // `lower_syscalls_all()` lowering pass). Backends resolve the
            // syscall number via their existing `syscall_stubs` tables.
            single(Some(ScgStatement::Syscall(SyscallCallNode {
                nr: syscall.nr,
                dst: syscall.dst.clone(),
                args: syscall
                    .args
                    .iter()
                    .map(|a| ScgExpr::Var(a.clone()))
                    .collect(),
            })))
        }

        NodePayload::Phantom(_) => Vec::new(),

        NodePayload::Control(_) => Vec::new(),

        NodePayload::VTable(_) | NodePayload::ClosureEnv(_) => Vec::new(),

        NodePayload::StructDef(_) | NodePayload::EnumDef(_) | NodePayload::Match(_)
        | NodePayload::ConstantTime(_) => Vec::new(),

        // PMT (Wave 1c TODO): StateInit/StateRead/StateWrite/StateTransform
        // nodes need proper lowering to ScgStatement — for now, emit no
        // statements so the build passes. Wave 1c will wire this.
        NodePayload::StateInit(_)
        | NodePayload::StateRead(_)
        | NodePayload::StateWrite(_)
        | NodePayload::StateTransform(_)
        | NodePayload::ForeignConsume(_)
        | NodePayload::ArenaNew(_)
        | NodePayload::ArenaAlloc(_)
        | NodePayload::ArenaGrow(_)
        | NodePayload::ArenaFree(_) => Vec::new(),

        // Wave 2b: channel operations.  Lower the SCG-side NodePayload
        // (which uses String-typed variable/type names — the scg crate
        // cannot depend on vuma-codegen) into the codegen-side
        // ScgStatement (which uses ScgType / ScgExpr).  The IRBuilder
        // then lowers these to IRInstr::Channel{Open,Send,Recv,Close}.
        NodePayload::ChannelOpen(co) => {
            let elem_ty = parse_scg_type(&co.elem_type).unwrap_or(ScgType::U8);
            single(Some(ScgStatement::ChannelOpen(ChannelOpenStmt {
                dst: node_var(node_id, "ch"),
                elem_ty,
            })))
        }
        NodePayload::ChannelSend(cs) => {
            let ty = parse_scg_type(&cs.ty).unwrap_or(ScgType::U8);
            single(Some(ScgStatement::ChannelSend(ChannelSendStmt {
                channel: ScgExpr::Var(cs.channel.clone()),
                message: ScgExpr::Var(cs.message.clone()),
                ty,
            })))
        }
        NodePayload::ChannelRecv(cr) => {
            let ty = parse_scg_type(&cr.ty).unwrap_or(ScgType::U8);
            single(Some(ScgStatement::ChannelRecv(ChannelRecvStmt {
                dst: node_var(node_id, "msg"),
                channel: ScgExpr::Var(cr.channel.clone()),
                ty,
            })))
        }
        NodePayload::ChannelClose(cc) => {
            single(Some(ScgStatement::ChannelClose(ChannelCloseStmt {
                channel: ScgExpr::Var(cc.channel.clone()),
            })))
        }
    }
}

/// Convert a `Computation` SCG node into one or more `ScgStatement`s.
///
/// This is extracted from `convert_node_to_statement_with_externs` because
/// the call-extraction logic can produce multiple statements (Call + Computation).
fn convert_computation_node(
    node_id: NodeId,
    comp: &vuma_scg::ComputationNode,
    edge_idx: &EdgeIndex,
    scg: &SCG,
    extern_functions: &HashSet<String>,
) -> Vec<ScgStatement> {
    let op_label = comp.kind.label().to_string();

    // Skip parameter nodes
    if op_label.starts_with("param ") {
        return Vec::new();
    }
    if op_label == "uninitialized" {
        return Vec::new();
    }
    if op_label.starts_with("lit_") {
        return Vec::new();
    }

    // Match arm body: "match_arm[N]: <expr>"
    if op_label.starts_with("match_arm[") {
        return Vec::new();
    }

    // Block end marker in match arm body
    if op_label.contains("block_end") {
        return Vec::new();
    }

    // Enum binding: "enum_bind Name(var)"
    if op_label.starts_with("enum_bind ") {
        return Vec::new();
    }

    // Match arm destructuring: "destructure Name.field"
    if op_label.starts_with("destructure ") {
        return Vec::new();
    }

    // Range check: "range_check start..=end"
    if op_label.starts_with("range_check ") {
        return Vec::new();
    }

    // Atomic operations
    if op_label.contains("atomic_store") {
        let addr = resolve_df_input(node_id, 0, edge_idx, scg);
        let value = resolve_df_input(node_id, 1, edge_idx, scg);
        return vec![ScgStatement::Call(CallNode {
            dst: None,
            func: "AtomicStore".to_string(),
            args: vec![value, addr],
            is_extern: false,
            reassigns: None,
        })];
    }
    if op_label.contains("atomic_load") {
        let addr = resolve_df_input(node_id, 0, edge_idx, scg);
        let user_var = extract_user_var_from_label(&op_label);
        return vec![ScgStatement::Call(CallNode {
            dst: Some(node_var(node_id, "val")),
            func: "AtomicLoad".to_string(),
            args: vec![addr],
            is_extern: false,
            reassigns: user_var,
        })];
    }

    // Skip Computation nodes that represent call expressions (top-level
    // calls already have a CF edge to a call_<name> FunctionEntry node).
    for cf_edge in edge_idx.outgoing_cf(node_id) {
        if let Some(target_data) = scg.get_node(cf_edge.target) {
            if let NodePayload::Control(c) = &target_data.payload {
                if c.kind == ControlKind::FunctionEntry {
                    if let Some(label) = &c.label {
                        if label.starts_with("call_") {
                            return Vec::new();
                        }
                    }
                }
            }
        }
    }

    // Collect DataFlow sources for expression resolution
    let df_inputs: Vec<vuma_scg::EdgeData> = edge_idx
        .incoming_df(node_id)
        .iter()
        .map(|e| (*e).clone())
        .collect();
    let sources: Vec<NodeId> = df_inputs.iter().map(|e| e.source).collect();

    // ── Extract function calls embedded in the expression label ──
    //
    // For recursive calls inside expressions (e.g. `fib(n-1) + fib(n-2)`),
    // the AST→SCG converter doesn't create call-site FunctionEntry nodes;
    // it stores the whole expression as a string label on this Computation
    // node.  We scan the label for `func(args)` patterns, emit Call
    // statements for each, and replace the call text with a vreg reference
    // so the surrounding expression can be parsed normally.
    let (label_no_calls, mut call_stmts) = extract_calls_from_label(
        &op_label,
        node_id,
        &sources,
        edge_idx,
        scg,
        extern_functions,
    );

    // If calls were extracted, the Computation's op/lhs/rhs need to be
    // re-derived from the modified label.  Otherwise, fall through to the
    // original label-based parsing below.
    if !call_stmts.is_empty() {
        // Strip "<var> = " or "let <var> = " prefix to get the expression
        let (expr_str, user_var) = strip_assignment_prefix(&label_no_calls);

        // Try to parse the (now call-free) expression
        if let Some((mut op, lhs_str, rhs_str)) = parse_expr_split(&expr_str) {
            // Type-aware >> shift and / % division
            // In VUMA, >> and / default to unsigned operations unless the
            // result_type is explicitly signed (i8/i16/i32/i64).
            let is_signed = comp
                .result_type
                .as_deref()
                .map(|t| t.starts_with('i'))
                .unwrap_or(false);
            if !is_signed {
                if op == IrBinOpKind::ShrA {
                    op = IrBinOpKind::ShrL;
                }
                if op == IrBinOpKind::SDiv {
                    op = IrBinOpKind::UDiv;
                }
                if op == IrBinOpKind::SRem {
                    op = IrBinOpKind::URem;
                }
                // Convert signed comparisons to unsigned for unsigned types
                if op == IrBinOpKind::SLt {
                    op = IrBinOpKind::ULt;
                }
                if op == IrBinOpKind::SLe {
                    op = IrBinOpKind::ULe;
                }
                if op == IrBinOpKind::SGt {
                    op = IrBinOpKind::UGt;
                }
                if op == IrBinOpKind::SGe {
                    op = IrBinOpKind::UGe;
                }
            }
            let lhs = resolve_subexpr(&lhs_str, &sources, edge_idx, scg);
            let rhs = resolve_subexpr(&rhs_str, &sources, edge_idx, scg);
            call_stmts.push(ScgStatement::Computation(ComputationNode {
                dst: computation_dst(node_id, &label_no_calls, scg),
                op,
                lhs,
                rhs,
                tail_call: false,
                reassigns: user_var,
            }));
            return call_stmts;
        }

        // No top-level operator: emit a copy (Add(0, rhs))
        let rhs_expr = resolve_subexpr(&expr_str, &sources, edge_idx, scg);
        call_stmts.push(ScgStatement::Computation(ComputationNode {
            dst: computation_dst(node_id, &label_no_calls, scg),
            op: IrBinOpKind::Add,
            lhs: ScgExpr::Int(0),
            rhs: rhs_expr,
            tail_call: false,
            reassigns: user_var,
        }));
        return call_stmts;
    }

    // No calls extracted — fall through to the original label-based parsing.
    // (This path is identical to the previous single-statement return.)
    convert_computation_no_calls(node_id, comp, &op_label, &sources, edge_idx, scg)
}

/// Strip "<var> = " or "let <var> = " prefix from a label, returning
/// (expression_string, Option<user_var>).
fn strip_assignment_prefix(label: &str) -> (String, Option<String>) {
    if let Some(eq_pos) = label.find("= ") {
        let before_eq = &label[..eq_pos];
        let after_eq = &label[eq_pos + 1..]; // starts with "= "
        let is_assignment_eq = !before_eq.ends_with('<')
            && !before_eq.ends_with('>')
            && !before_eq.ends_with('!')
            && !before_eq.ends_with('=')
            && !after_eq.starts_with("= ");
        if is_assignment_eq {
            let var_part = before_eq.strip_prefix("let ").unwrap_or(before_eq).trim();
            let is_simple_ident = !var_part.is_empty()
                && var_part.chars().all(|c| c.is_alphanumeric() || c == '_')
                && !var_part.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false);
            let uv = if is_simple_ident {
                Some(var_part.to_string())
            } else {
                None
            };
            (label[eq_pos + 2..].to_string(), uv)
        } else {
            (label.to_string(), None)
        }
    } else {
        (label.to_string(), None)
    }
}

/// Compute the destination variable name for a Computation node, given
/// its (possibly call-stripped) label.
fn computation_dst_from_label(node_id: NodeId, label: &str, _scg: &SCG) -> String {
    // If the label is a let-binding or assignment, use the user-visible
    // variable name as the destination so subsequent references resolve.
    if let Some(eq_pos) = label.find("= ") {
        let before_eq = &label[..eq_pos];
        let after_eq = &label[eq_pos + 1..];
        let is_assignment_eq = !before_eq.ends_with('<')
            && !before_eq.ends_with('>')
            && !before_eq.ends_with('!')
            && !before_eq.ends_with('=')
            && !after_eq.starts_with("= ");
        if is_assignment_eq {
            let var_part = before_eq.strip_prefix("let ").unwrap_or(before_eq).trim();
            let is_simple_ident = !var_part.is_empty()
                && var_part.chars().all(|c| c.is_alphanumeric() || c == '_')
                && !var_part.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false);
            if is_simple_ident {
                return var_part.to_string();
            }
        }
    }
    // Fallback: use the node-id-based variable name
    node_var(node_id, "comp")
}

/// Convert a Computation node's result_type to a load type.
/// Only returns Some for specific integer types (u8/u16/u32/u64/i8/i16/i32/i64).
/// Returns None for Address, void, or unknown types (defaults to U8).
fn result_type_to_load_ty(result_type: &Option<String>) -> Option<vuma_codegen::ir::IRType> {
    match result_type.as_deref() {
        Some("u8") | Some("U8") | Some("i8") | Some("I8") => Some(vuma_codegen::ir::IRType::U8),
        Some("u16") | Some("U16") | Some("i16") | Some("I16") => Some(vuma_codegen::ir::IRType::U16),
        Some("u32") | Some("U32") | Some("i32") | Some("I32") => Some(vuma_codegen::ir::IRType::U32),
        Some("u64") | Some("U64") | Some("i64") | Some("I64") => Some(vuma_codegen::ir::IRType::U64),
        // Don't override for Address/void/unknown — let IR builder default to U8
        _ => None,
    }
}


/// Original (no-call-extraction) Computation node handling — used when
/// `extract_calls_from_label` finds no calls in the label.
fn convert_computation_no_calls(
    node_id: NodeId,
    comp: &vuma_scg::ComputationNode,
    op_label: &str,
    sources: &[NodeId],
    edge_idx: &EdgeIndex,
    scg: &SCG,
) -> Vec<ScgStatement> {
    // Detect N-level dereference: "let val = **buf1" or "let val = ***buf1"
    // This generates N loads: each loads a pointer (U64) except the last
    // which loads the value (U8).
    if op_label.contains("= *") && !op_label.starts_with("*") {
        if let Some(pos) = op_label.find("= *") {
            // Count ALL '*' characters after "= " (not just after "= *")
            let after_eq = op_label[pos + 2..].trim(); // skip "= " (2 chars)
            if !after_eq.is_empty() && !after_eq.contains('=') {
                let deref_count = after_eq.chars().take_while(|&c| c == '*').count();
                if deref_count >= 2 {
                    // Multi-level dereference (**buf1, ***buf1, etc.)
                    let base_expr = strip_outer_parens(after_eq[deref_count..].trim());
                    let df_sources: Vec<NodeId> = edge_idx
                        .incoming_df(node_id)
                        .iter()
                        .map(|e| e.source)
                        .collect();
                    let base_ptr = if let Some((op, l, r)) = parse_expr_split(base_expr) {
                        let lhs_val = resolve_subexpr(&l, &df_sources, edge_idx, scg);
                        let rhs_val = resolve_subexpr(&r, &df_sources, edge_idx, scg);
                        ScgExpr::BinOp {
                            op: map_binop_kind(op),
                            lhs: Box::new(lhs_val),
                            rhs: Box::new(rhs_val),
                        }
                    } else {
                        resolve_subexpr(base_expr, &df_sources, edge_idx, scg)
                    };
                    
                    let mut stmts = Vec::new();
                    let mut current_ptr = base_ptr;
                    for level in 0..deref_count {
                        let is_last = level == deref_count - 1;
                        let dst = if is_last {
                            // Last level: use the user-visible variable name
                            computation_dst_from_label(node_id, op_label, scg)
                        } else {
                            format!("v_{}_deref_{}", node_id.as_u64(), level)
                        };
                        // Intermediate loads use U64 (loading a pointer);
                        // final load uses U8 (loading the value).
                        let load_ty = if is_last { None } else { Some(vuma_codegen::ir::IRType::U64) };
                        stmts.push(ScgStatement::Access(AccessNode::Load {
                            dst: dst.clone(),
                            ptr: current_ptr.clone(),
                            offset: None,
                            ty: load_ty,
                        }));
                        current_ptr = ScgExpr::Var(dst);
                    }
                    return stmts;
                }
            }
        }
    }

    // Detect Load patterns: "let value = *region" or "X = *Y"
    if op_label.contains("= *") && !op_label.starts_with("*") {
        if let Some(pos) = op_label.find("= *") {
            let after = op_label[pos + 3..].trim();
            if !after.is_empty() && !after.contains('=') {
                let ptr_expr = strip_outer_parens(after);
                let df_sources: Vec<NodeId> = edge_idx
                    .incoming_df(node_id)
                    .iter()
                    .map(|e| e.source)
                    .collect();
                let ptr = if let Some((op, l, r)) = parse_expr_split(ptr_expr) {
                    let lhs_val = resolve_subexpr(&l, &df_sources, edge_idx, scg);
                    let rhs_val = resolve_subexpr(&r, &df_sources, edge_idx, scg);
                    ScgExpr::BinOp {
                        op: map_binop_kind(op),
                        lhs: Box::new(lhs_val),
                        rhs: Box::new(rhs_val),
                    }
                } else {
                    resolve_subexpr(ptr_expr, &df_sources, edge_idx, scg)
                };
                // Infer load type from the pointer expression and result_type.
                // Case 1: `base + N` where N > 0 and N % 4 == 0 → struct field
                //   e.g. *(opt + 4) loads a U32 field
                // Case 2: `base + (idx * stride)` where stride is 4 or 8 → array
                //   e.g. *(arr + idx * 8) loads a U64 array element
                //   e.g. *(mat + (row * 4 + col) * 4) loads a U32 matrix element
                //
                // On big-endian (ppc64), a U8 load of a multi-byte value reads
                // the wrong byte (MSB instead of LSB). Using the correct load
                // type ensures all bytes are read in the right order.
                //
                // For offset 0 (tag bytes) or non-aligned offsets (byte access
                // in read_u32_be), keep ty=None (defaults to U8).
                let load_ty = {
                    let mut inferred_ty: Option<vuma_codegen::ir::IRType> = None;

                    // Only use array stride for load type inference.
                    // Constant-offset inference is unreliable because stores
                    // and loads may use different offset expressions
                    // (e.g. mem_arena_alloc stores via variable offset but
                    // loads via constant offset).
                    if let ScgExpr::BinOp { op: vuma_codegen::ir::BinOpKind::Add, lhs: _, rhs } = &ptr {
                        if let ScgExpr::BinOp { op: vuma_codegen::ir::BinOpKind::Mul, lhs: _, rhs } = rhs.as_ref() {
                            if let ScgExpr::Int(stride) = rhs.as_ref() {
                                inferred_ty = match *stride {
                                    8 => Some(vuma_codegen::ir::IRType::U64),
                                    4 => Some(vuma_codegen::ir::IRType::U32),
                                    _ => None,
                                };
                            }
                        }
                    }

                    inferred_ty
                };
                let load_dst = node_var(node_id, "val");
                let mut stmts = vec![ScgStatement::Access(AccessNode::Load {
                    dst: load_dst.clone(),
                    ptr,
                    offset: None,
                    ty: load_ty,
                })];
                // If this is a "let X = *ptr" pattern, also register X in
                // the IR builder's names map by emitting a copy statement
                // with reassigns: Some("X"). Without this, the variable
                // reference node (label "X") created by the AST→SCG
                // converter resolves to Var("X"), but names["X"] was never
                // set — only names["v_N"] was set by the Load. The
                // reassigns mechanism propagates the Load's result vreg to
                // both names["v_N"] and names["X"], so both the old-style
                // Var("v_N") and the new-style Var("X") references work.
                if let Some(uv) = extract_user_var_from_label(op_label) {
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: node_var(node_id, "let"),
                        op: IrBinOpKind::Add,
                        lhs: ScgExpr::Int(0),
                        rhs: ScgExpr::Var(load_dst),
                        tail_call: false,
                        reassigns: Some(uv),
                    }));
                }
                return stmts;
            }
        }
    }

    // Detect multi-level store: "**buf1 = 42" or "***buf1 = val"
    // This generates N-1 loads to get the final pointer, then a store.
    if op_label.starts_with("**") && op_label.contains("= ") {
        if let Some(eq_pos) = op_label.rfind("= ") {
            let lhs = op_label[..eq_pos].trim();
            let rhs = op_label[eq_pos + 2..].trim();
            // Count leading '*' characters
            let deref_count = lhs.chars().take_while(|&c| c == '*').count();
            if deref_count >= 2 {
                let base_expr = strip_outer_parens(lhs[deref_count..].trim());
                // Collect ALL sources: DataFlow + Derivation edges
                let df_inputs = edge_idx.incoming_df(node_id);
                let mut all_sources: Vec<NodeId> = df_inputs.iter().map(|e| e.source).collect();
                // Also check Derivation edges to Access nodes
                for out_edge in edge_idx.outgoing.get(&node_id).map(|v| v.as_slice()).unwrap_or(&[]) {
                    if out_edge.kind == EdgeKind::Derivation {
                        if let Some(access_incoming) = edge_idx.incoming.get(&out_edge.target) {
                            for e in access_incoming {
                                if e.kind == EdgeKind::Derivation {
                                    all_sources.push(e.source);
                                }
                            }
                        }
                    }
                }
                let base_ptr = if let Some((op, l, r)) = parse_expr_split(base_expr) {
                    let lhs_val = resolve_subexpr(&l, &all_sources, edge_idx, scg);
                    let rhs_val = resolve_subexpr(&r, &all_sources, edge_idx, scg);
                    ScgExpr::BinOp {
                        op: map_binop_kind(op),
                        lhs: Box::new(lhs_val),
                        rhs: Box::new(rhs_val),
                    }
                } else {
                    resolve_subexpr(base_expr, &all_sources, edge_idx, scg)
                };
                let value = resolve_subexpr(rhs, &all_sources, edge_idx, scg);
                let mut stmts = Vec::new();
                let mut current_ptr = base_ptr;
                for level in 0..deref_count - 1 {
                    let dst = format!("v_{}_store_deref_{}", node_id.as_u64(), level);
                    // All intermediate loads in a multi-level store use U64
                    // (loading pointers to dereference through).
                    stmts.push(ScgStatement::Access(AccessNode::Load {
                        dst: dst.clone(),
                        ptr: current_ptr.clone(),
                        offset: None,
                        ty: Some(vuma_codegen::ir::IRType::U64),
                    }));
                    current_ptr = ScgExpr::Var(dst);
                }
                // Final store to the dereferenced pointer
                stmts.push(ScgStatement::Access(AccessNode::Store {
                    ptr: current_ptr,
                    offset: None,
                    value,
                    ty: None,
                }));
                return stmts;
            }
        }
    }

    // Detect bare dereference: "*<expr>" as a return value or expression.
    // This pattern appears in `return *(arr + idx * 8);` where the
    // Computation node's label is just "*(arr + (idx * 8))" with no `=`.
    // The label starts with '*' and is NOT a store (stores have "= <value>"
    // after the pointer expression).
    if op_label.starts_with("*") && !op_label.contains("= ") {
        let ptr_expr = strip_outer_parens(op_label[1..].trim());
        let df_sources: Vec<NodeId> = edge_idx
            .incoming_df(node_id)
            .iter()
            .map(|e| e.source)
            .collect();
        let ptr = if let Some((op, l, r)) = parse_expr_split(ptr_expr) {
            let lhs_val = resolve_subexpr(&l, &df_sources, edge_idx, scg);
            let rhs_val = resolve_subexpr(&r, &df_sources, edge_idx, scg);
            ScgExpr::BinOp {
                op: map_binop_kind(op),
                lhs: Box::new(lhs_val),
                rhs: Box::new(rhs_val),
            }
        } else {
            resolve_subexpr(ptr_expr, &df_sources, edge_idx, scg)
        };
        let load_ty = comp.result_type.as_deref()
            .and_then(|rt| result_type_to_load_ty(&Some(rt.to_string())));
        return vec![ScgStatement::Access(AccessNode::Load {
            dst: node_var(node_id, "val"),
            ptr,
            offset: None,
            ty: load_ty,
        })];
    }

    // Check for Derivation edges to Allocation or Access nodes
    for deriv_edge in edge_idx
        .outgoing
        .get(&node_id)
        .map(|v| v.as_slice())
        .unwrap_or(&[])
    {
        if deriv_edge.kind == EdgeKind::Derivation {
            if let Some(target_data) = scg.get_node(deriv_edge.target) {
                match &target_data.payload {
                    NodePayload::Allocation(alloc) => {
                        let ty = alloc
                            .type_name
                            .as_deref()
                            .and_then(parse_scg_type)
                            .unwrap_or(ScgType::U8);
                        let alloc_name = node_var(node_id, "comp");
                        if alloc.size == 0 {
                            let df_inputs = edge_idx.incoming_df(node_id);
                            let sources2: Vec<NodeId> =
                                df_inputs.iter().map(|e| e.source).collect();
                            if let Some(size_expr) = extract_dynamic_alloc_size(
                                op_label,
                                &sources2,
                                edge_idx,
                                scg,
                            ) {
                                let mut stmts = vec![ScgStatement::Allocation(AllocationNode::Heap {
                                    name: alloc_name.clone(),
                                    size_expr,
                                    ty,
                                })];
                                // Register user-visible variable name (e.g. "buf" from "buf = allocate(8)")
                                if let Some(uv) = extract_user_var_from_label(op_label) {
                                    stmts.push(ScgStatement::Computation(ComputationNode {
                                        dst: node_var(node_id, "let"),
                                        op: IrBinOpKind::Add,
                                        lhs: ScgExpr::Int(0),
                                        rhs: ScgExpr::Var(alloc_name),
                                        tail_call: false,
                                        reassigns: Some(uv),
                                    }));
                                }
                                return stmts;
                            }
                        }
                        let mut stmts = vec![ScgStatement::Allocation(AllocationNode::Stack {
                            name: alloc_name.clone(),
                            size: alloc.size as u32,
                            ty,
                        })];
                        // Register user-visible variable name (e.g. "buf" from "buf = allocate(8)")
                        if let Some(uv) = extract_user_var_from_label(op_label) {
                            stmts.push(ScgStatement::Computation(ComputationNode {
                                dst: node_var(node_id, "let"),
                                op: IrBinOpKind::Add,
                                lhs: ScgExpr::Int(0),
                                rhs: ScgExpr::Var(alloc_name),
                                tail_call: false,
                                reassigns: Some(uv),
                            }));
                        }
                        return stmts;
                    }
                    NodePayload::Access(access) => {
                        let is_store_label =
                            op_label.starts_with("*") && op_label.contains("= ");
                        let is_load_label =
                            op_label.contains("= *") && !op_label.starts_with("*");
                        match access.mode {
                            AccessMode::Read if is_load_label => {
                                let load_dst = node_var(node_id, "val");
                                let mut stmts = vec![ScgStatement::Access(AccessNode::Load {
                                    ty: None,
                                    dst: load_dst.clone(),
                                    ptr: resolve_df_input(node_id, 0, edge_idx, scg),
                                    offset: access.offset.map(|o| ScgExpr::Int(o as i64)),
                                })];
                                // Register user-visible variable name (same fix as above)
                                if let Some(uv) = extract_user_var_from_label(op_label) {
                                    stmts.push(ScgStatement::Computation(ComputationNode {
                                        dst: node_var(node_id, "let"),
                                        op: IrBinOpKind::Add,
                                        lhs: ScgExpr::Int(0),
                                        rhs: ScgExpr::Var(load_dst),
                                        tail_call: false,
                                        reassigns: Some(uv),
                                    }));
                                }
                                return stmts;
                            }
                            AccessMode::Write | AccessMode::ReadWrite if is_store_label => {
                                let access_id = deriv_edge.target;
                                let df_inputs = edge_idx.incoming_df(node_id);
                                let mut all_sources: Vec<NodeId> =
                                    df_inputs.iter().map(|e| e.source).collect();
                                if let Some(access_incoming) = edge_idx.incoming.get(&access_id) {
                                    for e in access_incoming {
                                        if e.kind == EdgeKind::Derivation {
                                            all_sources.push(e.source);
                                        }
                                    }
                                }
                                // CRITICAL: Exclude the current node (node_id) from
                                // the sources. The store's own Computation node has
                                // a label like "*(block + i) = *(msg + i)" which
                                // contains the variable name "i". Without this
                                // filter, resolve_subexpr would match the store
                                // node itself when resolving "i", creating a
                                // circular reference (v_611 = ... v_611 ...) that
                                // corrupts the loop variable and causes SIGSEGV.
                                all_sources.retain(|&s| s != node_id);
                                let (ptr, value) = if op_label.starts_with("*") {
                                    if let Some(eq_pos) = op_label.rfind("= ") {
                                        let lhs = op_label[..eq_pos].trim();
                                        let rhs = op_label[eq_pos + 2..].trim();
                                        let ptr_expr =
                                            strip_outer_parens(lhs[1..].trim());
                                        let ptr = if let Some((op, l, r)) =
                                            parse_expr_split(ptr_expr)
                                        {
                                            let lhs_val = resolve_subexpr(
                                                &l,
                                                &all_sources,
                                                edge_idx,
                                                scg,
                                            );
                                            let rhs_val = resolve_subexpr(
                                                &r,
                                                &all_sources,
                                                edge_idx,
                                                scg,
                                            );
                                            ScgExpr::BinOp {
                                                op: map_binop_kind(op),
                                                lhs: Box::new(lhs_val),
                                                rhs: Box::new(rhs_val),
                                            }
                                        } else {
                                            resolve_subexpr(
                                                ptr_expr,
                                                &all_sources,
                                                edge_idx,
                                                scg,
                                            )
                                        };
                                        let value = if let Some(rhs_rest) = rhs.strip_prefix('*') {
                                            // RHS is a dereference: `*(msg + i)`.
                                            // Generate a Load statement first, then
                                            // use the loaded value as the store value.
                                            // Without this, resolve_subexpr would
                                            // misparse `*` as the Mul operator and
                                            // return Int(0), silently dropping the
                                            // load and storing 0 instead of the
                                            // actual byte.
                                            //
                                            // We return a synthetic statement list
                                            // (Load + Store) instead of a single
                                            // Store. The caller (convert_computation_no_calls)
                                            // returns this Vec directly.
                                            let load_ptr_expr = strip_outer_parens(rhs_rest.trim());
                                            let load_ptr = if let Some((op2, l2, r2)) =
                                                parse_expr_split(load_ptr_expr)
                                            {
                                                let lv = resolve_subexpr(
                                                    &l2,
                                                    &all_sources,
                                                    edge_idx,
                                                    scg,
                                                );
                                                let rv = resolve_subexpr(
                                                    &r2,
                                                    &all_sources,
                                                    edge_idx,
                                                    scg,
                                                );
                                                ScgExpr::BinOp {
                                                    op: map_binop_kind(op2),
                                                    lhs: Box::new(lv),
                                                    rhs: Box::new(rv),
                                                }
                                            } else {
                                                resolve_subexpr(
                                                    load_ptr_expr,
                                                    &all_sources,
                                                    edge_idx,
                                                    scg,
                                                )
                                            };
                                            let load_dst = format!("v_{}_load_rhs", node_id.as_u64());
                                            let load_stmt = ScgStatement::Access(AccessNode::Load {
                                                dst: load_dst.clone(),
                                                ptr: load_ptr,
                                                offset: None,
                                                ty: None,
                                            });
                                            let store_stmt = ScgStatement::Access(AccessNode::Store {
                                                ptr,
                                                offset: access.offset.map(|o| ScgExpr::Int(o as i64)),
                                                value: ScgExpr::Var(load_dst),
                                                ty: None,
                                            });
                                            return vec![load_stmt, store_stmt];
                                        } else if let Some((op, l, r)) =
                                            parse_expr_split(rhs)
                                        {
                                            let lhs_val = resolve_subexpr(
                                                &l,
                                                &all_sources,
                                                edge_idx,
                                                scg,
                                            );
                                            let rhs_val = resolve_subexpr(
                                                &r,
                                                &all_sources,
                                                edge_idx,
                                                scg,
                                            );
                                            ScgExpr::BinOp {
                                                op: map_binop_kind(op),
                                                lhs: Box::new(lhs_val),
                                                rhs: Box::new(rhs_val),
                                            }
                                        } else {
                                            resolve_subexpr(
                                                rhs,
                                                &all_sources,
                                                edge_idx,
                                                scg,
                                            )
                                        };
                                        (ptr, value)
                                    } else {
                                        return Vec::new();
                                    }
                                } else {
                                    return Vec::new();
                                };
                                return vec![ScgStatement::Access(AccessNode::Store {
                                    ptr,
                                    offset: access.offset.map(|o| ScgExpr::Int(o as i64)),
                                    value,
                                    ty: None,
                                })];
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Detect address-of patterns: "let x = @func_name"
    if let Some(addr_name) = op_label.strip_prefix("let ") {
        if let Some(at_pos) = addr_name.find("= @") {
            let symbol = addr_name[at_pos + 3..].trim();
            if !symbol.is_empty() && !symbol.contains(' ') && !symbol.contains('(') {
                let var_part = addr_name[..at_pos].trim();
                let user_name = if var_part.is_empty() {
                    None
                } else {
                    Some(var_part.to_string())
                };
                // Use node_var (v_{node_id}) as GetAddress dst so that
                // resolve_subexpr (which returns Var("v_{source_node_id}"))
                // can find it in the IR builder's names map.
                let node_dst = node_var(node_id, "addr");
                // Also create a Computation copy with the user-visible name
                // so that references to the user name also resolve.
                let mut stmts = vec![ScgStatement::GetAddress(GetAddressNode {
                    dst: node_dst.clone(),
                    name: symbol.to_string(),
                })];
                if let Some(uname) = user_name {
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: uname.clone(),
                        op: IrBinOpKind::Add,
                        lhs: ScgExpr::Var(node_dst),
                        rhs: ScgExpr::Int(0),
                        tail_call: false,
                        reassigns: Some(uname),
                    }));
                }
                return stmts;
            }
        }
    } else if let Some(symbol) = op_label.strip_prefix("@") {
        let symbol = symbol.trim();
        if !symbol.is_empty() && !symbol.contains(' ') && !symbol.contains('(') {
            return vec![ScgStatement::GetAddress(GetAddressNode {
                dst: node_var(node_id, "addr"),
                name: symbol.to_string(),
            })];
        }
    }

    // Parse the expression label
    let (expr_str, user_var) = strip_assignment_prefix(op_label);

    if let Some((mut op, lhs_str, rhs_str)) = parse_expr_split(&expr_str) {
        // Type-aware >> shift and / % division
        let is_signed = comp
            .result_type
            .as_deref()
            .map(|t| t.starts_with('i'))
            .unwrap_or(false);
        if !is_signed {
            if op == IrBinOpKind::ShrA {
                op = IrBinOpKind::ShrL;
            }
            if op == IrBinOpKind::SDiv {
                op = IrBinOpKind::UDiv;
            }
            if op == IrBinOpKind::SRem {
                op = IrBinOpKind::URem;
            }
            // Convert signed comparisons to unsigned for unsigned types
            if op == IrBinOpKind::SLt {
                op = IrBinOpKind::ULt;
            }
            if op == IrBinOpKind::SLe {
                op = IrBinOpKind::ULe;
            }
            if op == IrBinOpKind::SGt {
                op = IrBinOpKind::UGt;
            }
            if op == IrBinOpKind::SGe {
                op = IrBinOpKind::UGe;
            }
        }
        let lhs = resolve_subexpr(&lhs_str, sources, edge_idx, scg);
        let rhs = resolve_subexpr(&rhs_str, sources, edge_idx, scg);
        return vec![ScgStatement::Computation(ComputationNode {
            dst: computation_dst(node_id, op_label, scg),
            op,
            lhs,
            rhs,
            tail_call: false,
            reassigns: user_var,
        })];
    }

    // No top-level operator: emit a copy
    let rhs_expr = resolve_subexpr(&expr_str, sources, edge_idx, scg);
    vec![ScgStatement::Computation(ComputationNode {
        dst: computation_dst(node_id, op_label, scg),
        op: IrBinOpKind::Add,
        lhs: ScgExpr::Int(0),
        rhs: rhs_expr,
        tail_call: false,
        reassigns: user_var,
    })]
}

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
    let two_char_ops: [(&str, IrBinOpKind); 8] = [
        ("<=", IrBinOpKind::SLe), (">=", IrBinOpKind::SGe),
        ("==", IrBinOpKind::Eq), ("!=", IrBinOpKind::Ne),
        ("<<", IrBinOpKind::Shl), (">>", IrBinOpKind::ShrA),
        // Logical AND/OR: lowered to bitwise And/Or on integer operands
        // (VUMA booleans are i1/i64, so bitwise ops on 0/1 values are
        // equivalent to logical ops).
        ("&&", IrBinOpKind::And), ("||", IrBinOpKind::Or),
    ];
    
    // Check for single-character operators in precedence order (lowest first)
    let single_ops: [(&str, IrBinOpKind); 10] = [
        ("|", IrBinOpKind::Or),
        ("^", IrBinOpKind::Xor),
        ("&", IrBinOpKind::And),
        ("<", IrBinOpKind::SLt),
        (">", IrBinOpKind::SGt),
        ("+", IrBinOpKind::Add),
        ("-", IrBinOpKind::Sub),
        ("*", IrBinOpKind::Mul),
        ("/", IrBinOpKind::SDiv),
        ("%", IrBinOpKind::SRem),
    ];
    
    // Search for top-level operators (outside parentheses)
    // Process in precedence order (lowest first)
    // Check two-char operators FIRST (before single-char)
    // This ensures << >> are matched before < >
    for &(op_str, op_kind) in &two_char_ops {
        if let Some(pos) = find_top_level_op(expr, op_str) {
            let lhs = expr[..pos].trim().to_string();
            let rhs = expr[pos + op_str.len()..].trim().to_string();
            if !lhs.is_empty() && !rhs.is_empty() {
                return Some((op_kind, lhs, rhs));
            }
        }
    }
    
    for &(op_str, op_kind) in &single_ops {
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
                if (op == "<" || op == ">")
                    && i + 1 < bytes.len() && (bytes[i + 1] == b'=' || bytes[i + 1] == b'<' || bytes[i + 1] == b'>') {
                        continue;
                    }
                // Don't match '&' in '&&' or '|' in '||'
                if op == "&" || op == "|" {
                    // Skip if this is part of a double operator (&& or ||)
                    if (i + 1 < bytes.len() && bytes[i + 1] == bytes[i]) 
                       || (i > 0 && bytes[i - 1] == bytes[i]) {
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
        IrBinOpKind::UDiv => vuma_codegen::ir::BinOpKind::UDiv,
        IrBinOpKind::URem => vuma_codegen::ir::BinOpKind::URem,
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


/// Extract the user-visible variable name from a label like "out = atomic_load(...)".
/// Returns None if the label doesn't match the "<var> = ..." pattern.
fn extract_user_var_from_label(label: &str) -> Option<String> {
    if let Some(eq_pos) = label.find("= ") {
        let var_part = label[..eq_pos].trim();
        let var_part = var_part.strip_prefix("let ").unwrap_or(var_part).trim();
        if !var_part.is_empty()
            && var_part.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && var_part.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            return Some(var_part.to_string());
        }
    }
    None
}

fn resolve_subexpr(
    subexpr: &str,
    sources: &[NodeId],
    edge_idx: &EdgeIndex,
    scg: &SCG,
) -> ScgExpr {
    let subexpr = subexpr.trim();

    // Strip outer parentheses (e.g. "(-42)" → "-42") so negative literals
    // and parenthesized sub-expressions are handled correctly.
    let subexpr = strip_outer_parens(subexpr);

    // Check if it's a literal number (handles negative literals like "-42")
    if let Ok(num) = subexpr.parse::<i64>() {
        return ScgExpr::Int(num);
    }

    // Bitwise NOT: ~expr = expr ^ -1 (XOR with all-ones)
    if let Some(inner) = subexpr.strip_prefix('~') {
        let inner_expr = resolve_subexpr(inner.trim(), sources, edge_idx, scg);
        return ScgExpr::BinOp {
            op: vuma_codegen::ir::BinOpKind::Xor,
            lhs: Box::new(inner_expr),
            rhs: Box::new(ScgExpr::Int(-1)),
        };
    }

    // Dereference: *expr = Load(expr)
    // When a dereference appears as a function call argument or in an
    // expression, resolve the address expression and emit a Load.
    if let Some(inner) = subexpr.strip_prefix('*') {
        let inner = inner.trim();
        let inner_expr = resolve_subexpr(inner, sources, edge_idx, scg);
        return ScgExpr::Load {
            addr: Box::new(inner_expr),
        };
    }

    // Boolean and unit literals
    match subexpr {
        "true" => return ScgExpr::Int(1),
        "false" => return ScgExpr::Int(0),
        "None" | "null" | "nullptr" => return ScgExpr::Int(0),
        _ => {}
    }

    // Check if it's a hex literal (e.g. "0x12", "0xFF", "-0x1A")
    let hex_str = subexpr.strip_prefix("-").unwrap_or(subexpr);
    let is_neg = subexpr.starts_with('-');
    if let Some(hex_digits) = hex_str.strip_prefix("0x").or_else(|| hex_str.strip_prefix("0X")) {
        if let Ok(num) = i64::from_str_radix(hex_digits, 16) {
            return ScgExpr::Int(if is_neg { -num } else { num });
        }
    }
    
    // Check if it's a function call: "func_name(args)"
    // Only match if the expression starts with an identifier followed by "("
    // This avoids matching expressions like "(a + b)" or "*(ptr + 0)"
    if subexpr.contains('(') && subexpr.contains(')') {
        // Check if it starts with an identifier (not *, (, etc.)
        let first_char = subexpr.chars().next().unwrap_or(' ');
        if first_char.is_alphabetic() || first_char == '_' {
            // Find the opening paren
            if let Some(paren_pos) = subexpr.find('(') {
                let func_name = &subexpr[..paren_pos].trim();
                // Verify it's a valid function name
                let is_valid_name = !func_name.is_empty()
                    && func_name.chars().all(|c| c.is_alphanumeric() || c == '_');
                if is_valid_name {
                    // Look for a Computation node whose label contains this call
                    for node_data in scg.nodes() {
                        if let NodePayload::Computation(comp) = &node_data.payload {
                            let label = comp.kind.label();
                            // Match "let var = func_name(...)" or "func_name(...)"
                            if label.contains(subexpr)
                                && (label.starts_with("let ") || label.contains(&(format!("= {}", subexpr)))) {
                                    return ScgExpr::Var(format!("v_{}", node_data.id.as_u64()));
                                }
                        }
                    }
                }
            }
        }
    }

    // Check if it's a known literal (lit_<n>)
    if let Some(num_str) = subexpr.strip_prefix("lit_") {
        if let Ok(num) = num_str.parse::<i64>() {
            return ScgExpr::Int(num);
        }
        // Boolean literals: lit_true -> 1, lit_false -> 0
        if num_str == "true" {
            return ScgExpr::Int(1);
        }
        if num_str == "false" {
            return ScgExpr::Int(0);
        }
    }
    
    // Check if it's a simple variable name
    // Match against the DataFlow sources
    if is_simple_var(subexpr) {
        // First, try exact match: the source node IS the variable definition
        for &src in sources.iter() {
            if let Some(src_data) = scg.get_node(src) {
                if let NodePayload::Computation(comp) = &src_data.payload {
                    let label = comp.kind.label();
                    // Check for exact match or "param <var>" or "<var> = ..."
                    if label == *subexpr 
                       || label == format!("param {}", subexpr)
                       || label.starts_with(&format!("{} ", subexpr))
                       || label.starts_with(&format!("{} =", subexpr))
                       || label.starts_with(&format!("let {} =", subexpr)) {
                        // For multi-level dereference loads (let val = **buf1,
                        // let val = ***buf1), the DataFlow input is the base
                        // pointer, not the loaded value. Return Var(name) so
                        // the IR builder resolves it via the names map.
                        if label.contains("**") {
                            return ScgExpr::Var(subexpr.to_string());
                        }
                        return resolve_df_input_for_node(src, edge_idx, scg);
                    }
                }
            }
        }
        // Second pass: check if any source's label contains the variable name
        // as a whole word (word-boundary match) AND the source DEFINES the
        // variable (label starts with "let <var> =" or "<var> =" or "param <var>").
        //
        // The previous `contains_word`-only check was too loose: it matched
        // "i" inside "let val = read_u32_be(block, i * 4)", causing the loop
        // variable "i" to be resolved to the read_u32_be result instead of
        // the actual loop variable. This silently corrupted call arguments
        // like w_store(w, i, val) → w_store(w, read_u32_be_result, val).
        for &src in sources {
            if let Some(src_data) = scg.get_node(src) {
                if let NodePayload::Computation(comp) = &src_data.payload {
                    let label = comp.kind.label();
                    // Only match if the source DEFINES the variable, not just
                    // uses it. Definitions have the form:
                    //   "let <var> = ..."
                    //   "<var> = ..."
                    //   "param <var>"
                    //   "<var>" (exact match, already handled in first pass)
                    let defines_var = label.starts_with(&format!("let {} =", subexpr))
                        || label.starts_with(&format!("{} =", subexpr))
                        || label == format!("param {}", subexpr);
                    if defines_var && contains_word(&label, subexpr) {
                        return resolve_df_input_for_node(src, edge_idx, scg);
                    }
                }
            }
        }
        // Check if it's a const reference (e.g. MASK32, BN256_MASK64)
        // Const nodes have label "const NAME = VALUE"
        // Scan all nodes for a const definition matching the name
        for node_data in scg.nodes() {
            if let NodePayload::Computation(comp) = &node_data.payload {
                let label = comp.kind.label();
                if let Some(rest) = label.strip_prefix("const ") {
                    // Format: "const NAME = VALUE"
                    // skip "const "
                    if let Some(eq_pos) = rest.find('=') {
                        let name = rest[..eq_pos].trim();
                        if name == subexpr {
                            let value_str = rest[eq_pos + 1..].trim();
                            // Try to parse as integer (decimal or hex)
                            if let Ok(num) = value_str.parse::<i64>() {
                                return ScgExpr::Int(num);
                            }
                            if value_str.starts_with("0x") || value_str.starts_with("0X") {
                                if let Ok(num) = i64::from_str_radix(&value_str[2..], 16) {
                                    return ScgExpr::Int(num);
                                }
                            }
                            // Try parsing as u64 then converting (for large constants)
                            if let Ok(num) = value_str.parse::<u64>() {
                                return ScgExpr::Int(num as i64);
                            }
                        }
                    }
                }
            }
        }

        // If still no match, return the variable name as a Var expression
        // for valid variable names. The IR builder will resolve it from its
        // names map (e.g., for-loop iterators registered by lower_loop).
        // Invalid identifiers (hex literals, numbers, etc.) fall back to
        // the first source.
        let is_valid_var = subexpr.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && subexpr.chars().all(|c| c.is_alphanumeric() || c == '_');
        if is_valid_var {
            return ScgExpr::Var(subexpr.to_string());
        }
        // Fallback: use the first source
        if let Some(&src) = sources.first() {
            return resolve_df_input_for_node(src, edge_idx, scg);
        }
    }
    
    // For complex sub-expressions, recursively parse and return BinOp
    if let Some((op, lhs_str, rhs_str)) = parse_expr_split(subexpr) {
        // VUMA's `>>` is arithmetic (sign-extending) for signed integers.
        // `parse_expr_split` already maps `>>` to `IrBinOpKind::ShrA`
        // (see the `two_char_ops` table).  We keep `op` as-is here — the
        // previous unconditional `ShrA → ShrL` rewrite broke any signed-
        // shift code, most notably the `bit_abs` gold-standard test
        // (`x >> 63` on a negative i64 must yield -1, not a large positive
        // number; the abs computation `flipped - sign` then returned 214
        // instead of 42 across all backends).
        let lhs = resolve_subexpr(&lhs_str, sources, edge_idx, scg);
        let rhs = resolve_subexpr(&rhs_str, sources, edge_idx, scg);
        // Map IrBinOpKind to the codegen BinOpKind
        let binop_kind = match op {
            IrBinOpKind::Add => vuma_codegen::ir::BinOpKind::Add,
            IrBinOpKind::Sub => vuma_codegen::ir::BinOpKind::Sub,
            IrBinOpKind::Mul => vuma_codegen::ir::BinOpKind::Mul,
            IrBinOpKind::SDiv => vuma_codegen::ir::BinOpKind::SDiv,
            IrBinOpKind::SRem => vuma_codegen::ir::BinOpKind::SRem,
            IrBinOpKind::UDiv => vuma_codegen::ir::BinOpKind::UDiv,
            IrBinOpKind::URem => vuma_codegen::ir::BinOpKind::URem,
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

    // Handle cast expressions: "expr as Type" or "(expr as Type)"
    // The SCG represents casts as Computation nodes with label "(expr as Type)"
    // when the cast is used directly in an expression (not a let-binding).
    // We strip the cast and return the inner expression — the type is already
    // tracked by the SCG node's result_type, and the backend will handle
    // the actual value (casts between integer types are no-ops at the IR
    // level since all registers are 64-bit).
    if let Some(inner) = subexpr.strip_prefix("(").and_then(|s| s.strip_suffix(")")) {
        if let Some(as_pos) = inner.find(" as ") {
            let expr_part = inner[..as_pos].trim();
            let type_part = inner[as_pos + 4..].trim();
            // Validate type_part looks like a type name
            if type_part.chars().all(|c| c.is_alphanumeric() || c == '_') {
                return resolve_subexpr(expr_part, sources, edge_idx, scg);
            }
        }
    }
    // Also handle without parens: "expr as Type"
    if let Some(as_pos) = subexpr.find(" as ") {
        let expr_part = subexpr[..as_pos].trim();
        let type_part = subexpr[as_pos + 4..].trim();
        if type_part.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return resolve_subexpr(expr_part, sources, edge_idx, scg);
        }
    }

    // Fallback: log warning for unsupported sub-expressions instead of
    // silently returning 0. This makes debugging easier when constructs
    // are not handled by the SCG→IR bridge.
    eprintln!("[vuma] WARNING: resolve_subexpr fallback for '{}'; using 0", subexpr);
    ScgExpr::Int(0)
}

/// Check if a string is a simple variable name (alphanumeric, no spaces or operators)
fn is_simple_var(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_') && s.parse::<i64>().is_err()
}

/// Check if `needle` appears in `haystack` as a whole word (bounded by
/// non-identifier characters or string boundaries).  This prevents false
/// matches like "i" inside "lit_5" or "result" inside "results".
fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    let needle_bytes = needle.as_bytes();
    let h_bytes = haystack.as_bytes();
    let n_len = needle_bytes.len();
    if n_len == 0 {
        return false;
    }
    let mut i = 0;
    while i + n_len <= h_bytes.len() {
        if &h_bytes[i..i + n_len] == needle_bytes {
            // Check left boundary
            let left_ok = i == 0 || !is_ident_byte(h_bytes[i - 1]);
            // Check right boundary
            let right_ok = i + n_len == h_bytes.len() || !is_ident_byte(h_bytes[i + n_len]);
            if left_ok && right_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Try to extract the size expression from a dynamic-size `allocate(<expr>)`
/// call, given the parent Computation node's label and DataFlow sources.
///
/// The label typically looks like one of:
///   - `let buf = allocate(n)`
///   - `buf = allocate(n + 8)`
///   - `buf = allocate(capacity * msg_size)`
///
/// Returns `Some(ScgExpr)` if a non-trivial size expression is found, or
/// `None` if the size is a literal 0 (or the label doesn't match the
/// `allocate(...)` pattern).  The caller should fall back to a stack
/// allocation in the `None` case.
fn extract_dynamic_alloc_size(
    comp_label: &str,
    sources: &[NodeId],
    edge_idx: &EdgeIndex,
    scg: &SCG,
) -> Option<ScgExpr> {
    // Locate "allocate(" in the label.
    let alloc_pos = comp_label.find("allocate(")?;
    let after = &comp_label[alloc_pos + "allocate(".len()..];
    // Find the matching closing paren (handle nested parens, e.g. for
    // `allocate(f(x))` — though that's rare in practice).
    let mut depth: i32 = 1;
    let mut end: usize = 0;
    for (i, c) in after.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    let size_str = after[..end].trim();
    if size_str.is_empty() {
        return None;
    }
    // If the size is a literal integer, leave it to the stack-allocation
    // path (the SCG AllocationNode.size should already hold the value).
    if size_str.parse::<i64>().is_ok() {
        return None;
    }
    // Resolve the size expression to a ScgExpr using the parent
    // Computation node's DataFlow sources.
    let size_expr = if let Some((op, lhs_str, rhs_str)) = parse_expr_split(size_str) {
        let lhs = resolve_subexpr(&lhs_str, sources, edge_idx, scg);
        let rhs = resolve_subexpr(&rhs_str, sources, edge_idx, scg);
        ScgExpr::BinOp {
            op: map_binop_kind(op),
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    } else {
        resolve_subexpr(size_str, sources, edge_idx, scg)
    };
    // If the resolved expression fell back to Int(0) (e.g., the variable
    // wasn't found), don't emit a heap allocation — it would call
    // __vuma_alloc(0) which is wasteful and may return NULL.
    if matches!(size_expr, ScgExpr::Int(0)) {
        return None;
    }
    Some(size_expr)
}

/// Determine the destination variable name for a Computation node.
/// For reassignments ("x = ...", not "let x = ..."), reuse the original
/// definition's node id so that SSA phi nodes are created at merge points.
/// For all other cases (including "let x = ..." definitions), use the node's
/// own id.
fn computation_dst(node_id: NodeId, _op_label: &str, _scg: &SCG) -> String {
    // TEMPORARILY REVERTED for testing
    node_var(node_id, "comp")
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
                    // Extract the parameter name from the label.
                    // Labels look like "param n" or "param count" etc.
                    let label = comp.kind.label();
                    let name = if let Some(rest) = label.strip_prefix("param ") {
                        rest.trim().to_string()
                    } else {
                        format!("v_{}", edge.target.as_u64())
                    };
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

/// PMT (Wave 2): map an access_size (in bytes) to the corresponding unsigned
/// IR type. Used by `convert_node_to_statement_with_externs` when lowering
/// state-field Access nodes (which carry both an explicit `offset` and a
/// `access_size` matching the field's width). Returns `None` for sizes that
/// don't map to a standard integer type — the IR builder's heuristics then
/// pick a sensible default (typically U8).
fn access_size_to_ir_type(size: u64) -> Option<vuma_codegen::ir::IRType> {
    match size {
        1 => Some(vuma_codegen::ir::IRType::U8),
        2 => Some(vuma_codegen::ir::IRType::U16),
        4 => Some(vuma_codegen::ir::IRType::U32),
        8 => Some(vuma_codegen::ir::IRType::U64),
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
        RepD::ManifoldSpatial(_) | RepD::GestaltSuperposition(_) | RepD::ConceptRelational(_) => ScgType::Ptr,
        // PMT: a State is a buffer view (passed by reference); a Ref is a
        // pointer-sized offset.
        RepD::State { .. } => ScgType::Ptr,
        RepD::Ref { .. } => ScgType::U64,
        // Wave 9 — Dependent state types: a DependentArray is a dynamic
        // data structure passed by reference (like Struct/Array); the
        // runtime count is tracked in a separate u64 vreg.
        RepD::DependentArray { .. } => ScgType::Ptr,
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
        // Wave 1c: Channel<T> — opaque IPC handle.  Use "channel" as the
        // canonical name (the inner payload type is not part of the runtime
        // representation).
        ScgType::Channel(_) => "channel",
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
fn parse_for_range(label: &str) -> Option<(String, ScgExpr, ScgExpr)> {
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
        // Start bound: can be a constant (i64) or a variable name or
        // a parenthesized expression like "(msg_len + 1)".
        let start_expr = if let Ok(start) = start_str.parse::<i64>() {
            ScgExpr::Int(start)
        } else if start_str.starts_with('(') && start_str.ends_with(')') {
            // Parenthesized expression — strip parens and try to parse
            // as a variable or simple binop.
            let inner = start_str[1..start_str.len()-1].trim();
            if let Ok(start) = inner.parse::<i64>() {
                ScgExpr::Int(start)
            } else if inner.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                && inner.chars().all(|c| c.is_alphanumeric() || c == '_')
            {
                ScgExpr::Var(inner.to_string())
            } else {
                // Try to parse as a binop (e.g. "msg_len + 1")
                if let Some((op, l, r)) = parse_expr_split(inner) {
                    let lhs = if let Ok(v) = l.parse::<i64>() {
                        ScgExpr::Int(v)
                    } else {
                        ScgExpr::Var(l.to_string())
                    };
                    let rhs = if let Ok(v) = r.parse::<i64>() {
                        ScgExpr::Int(v)
                    } else {
                        ScgExpr::Var(r.to_string())
                    };
                    ScgExpr::BinOp {
                        op: map_binop_kind(op),
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                    }
                } else {
                    return None;
                }
            }
        } else if start_str.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && start_str.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            ScgExpr::Var(start_str.to_string())
        } else {
            return None;
        };
        // End bound can be a constant or a variable name.  Constants are
        // parsed as i64 and wrapped in ScgExpr::Int.  Variable names are
        // wrapped in ScgExpr::Var — the IR builder resolves them via its
        // `names` map (e.g. an outer loop's phi vreg).  This is what makes
        // `for j in 0..i` work when `i` is the outer loop variable.
        let end_expr = if let Ok(end) = end_str.parse::<i64>() {
            let end = if inclusive { end + 1 } else { end };
            ScgExpr::Int(end)
        } else if end_str.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && end_str.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            // Variable end bound — inclusive adjustment is handled at
            // runtime by the comparison (we use SLt for exclusive, but
            // for inclusive the caller should have already adjusted).
            if inclusive {
                // `for j in 0..=i` → compare j <= i.  We can't easily
                // express SLe with the current for_range structure (which
                // always uses SLt).  For now, treat inclusive variable
                // bounds as exclusive (rare in practice).
                ScgExpr::Var(end_str.to_string())
            } else {
                ScgExpr::Var(end_str.to_string())
            }
        } else {
            return None;
        };
        return Some((var_name, start_expr, end_expr));
    }
    None
}

/// Parse a while-loop condition from the LoopHeader label and return the
/// *negated* condition as an `ScgExpr`, so that inserting
/// `If { cond: negated, then_body: [Break] }` at the start of the loop body
/// makes the loop exit when the original condition becomes false.
///
/// The label looks like `"while (i < 4)"`.  The LoopHeader node receives
/// exactly two DataFlow inputs: input 0 is the LHS operand, input 1 is the
/// RHS operand.  The comparison operator is extracted from the label text.
///
/// Returns `None` if the label is not a while-loop, if no comparison
/// operator is found, or if the DataFlow inputs are missing.
fn parse_while_condition(
    header_id: NodeId,
    label: &str,
    edge_idx: &EdgeIndex,
    scg: &SCG,
    extern_functions: &HashSet<String>,
) -> Option<(ScgExpr, Vec<ScgStatement>)> {
    let label = label.trim();
    // Strip "while (" prefix and ")" suffix to get the condition expression.
    let cond_str = label.strip_prefix("while")?.trim();
    let cond_str = cond_str.strip_prefix('(').unwrap_or(cond_str);
    let cond_str = cond_str.strip_suffix(')').unwrap_or(cond_str);
    let cond_str = cond_str.trim();

    // ── Extract inline function calls from the while condition ──
    //
    // While conditions like `while (is_space(c) == 0)` contain a function
    // call that has no dedicated SCG Computation node.  We extract the call
    // into a `Call` statement (emitted before the loop guard) and replace
    // the call text with a vreg reference so the comparison can be parsed.
    let df_inputs = edge_idx.incoming_df(header_id);
    let sources: Vec<NodeId> = df_inputs.iter().map(|e| e.source).collect();
    let (cond_no_calls, pre_calls) = extract_calls_from_label(
        cond_str,
        header_id,
        &sources,
        edge_idx,
        scg,
        extern_functions,
    );

    // Find the comparison operator.  Check two-character operators first
    // (<=, >=, ==, !=) before single-character ones (<, >).
    let (op_str, lhs_str, rhs_str) = if let Some(pos) = find_operator(&cond_no_calls, "<=") {
        ("<=", &cond_no_calls[..pos], &cond_no_calls[pos + 2..])
    } else if let Some(pos) = find_operator(&cond_no_calls, ">=") {
        (">=", &cond_no_calls[..pos], &cond_no_calls[pos + 2..])
    } else if let Some(pos) = find_operator(&cond_no_calls, "==") {
        ("==", &cond_no_calls[..pos], &cond_no_calls[pos + 2..])
    } else if let Some(pos) = find_operator(&cond_no_calls, "!=") {
        ("!=", &cond_no_calls[..pos], &cond_no_calls[pos + 2..])
    } else if let Some(pos) = find_operator(&cond_no_calls, "<") {
        ("<", &cond_no_calls[..pos], &cond_no_calls[pos + 1..])
    } else if let Some(pos) = find_operator(&cond_no_calls, ">") {
        (">", &cond_no_calls[..pos], &cond_no_calls[pos + 1..])
    } else {
        return None;
    };

    // Resolve lhs and rhs operands using resolve_subexpr.
    //
    // We use resolve_subexpr (the same function used by resolve_branch_cond
    // for if-conditions) because it can handle complex sub-expressions like
    // `*(src + p)` (Load), `is_digit(c)` (function call), and `a + b` (BinOp).
    //
    // The old approach used resolve_df_input which only looks at DataFlow
    // edges — it cannot resolve Loads or function calls that have no
    // dedicated Computation node in the SCG.  This caused while-conditions
    // like `while (*(src + p) == 1)` to silently evaluate to false (0 == 1),
    // making the loop exit immediately with pre-loop values.
    let lhs = resolve_subexpr(lhs_str.trim(), &sources, edge_idx, scg);
    let rhs = resolve_subexpr(rhs_str.trim(), &sources, edge_idx, scg);

    let neg_op = match op_str {
        "<" => IrBinOpKind::SGe,   // !(a < b)  ≡  a >= b
        "<=" => IrBinOpKind::SGt,  // !(a <= b) ≡  a >  b
        ">" => IrBinOpKind::SLe,   // !(a > b)  ≡  a <= b
        ">=" => IrBinOpKind::SLt,  // !(a >= b) ≡  a <  b
        "==" => IrBinOpKind::Ne,   // !(a == b) ≡  a != b
        "!=" => IrBinOpKind::Eq,   // !(a != b) ≡  a == b
        _ => return None,
    };

    Some((
        ScgExpr::BinOp {
            op: map_binop_kind(neg_op),
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
        pre_calls,
    ))
}

/// Find the position of a comparison operator in a condition string,
/// respecting nested parentheses (operators inside parens are skipped).
fn find_operator(s: &str, op: &str) -> Option<usize> {
    let mut depth = 0i32;
    let bytes = s.as_bytes();
    let op_bytes = op.as_bytes();
    let mut i = 0;
    while i + op_bytes.len() <= bytes.len() {
        let c = bytes[i] as char;
        if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
        } else if depth == 0
            && bytes[i..i + op_bytes.len()] == *op_bytes {
                // Avoid matching "<" inside "<=" or ">" inside ">=", or inside
                // "<<"/">>" shift operators.
                let next_is = |b: u8| i + 1 < bytes.len() && bytes[i + 1] == b;
                if (op == "<" && next_is(b'='))
                    || (op == ">" && next_is(b'='))
                    || (op == "<" && next_is(b'<'))
                    || (op == ">" && next_is(b'>'))
                {
                    i += 1;
                } else {
                    return Some(i);
                }
            }
        i += 1;
    }
    None
}

/// Bridge the `vuma-scg` SCG to the codegen SCG (no extern functions).
///
/// This is a convenience wrapper around [`bridge_scg_to_codegen_with_externs`]
/// that passes an empty set of extern function names. Use this when the
/// program does not declare any `extern "C"` blocks, or when all function
/// calls are to locally-defined VUMA functions.
///
/// DEPRECATED: the canonical pipeline now uses [`bridge_ast_to_codegen_scg`]
/// (direct AST→codegen path). This function is retained because the
/// `compile_dump`, `dump_ir`, `dump_codegen_scg` binaries and several test
/// files in `src/tests/` still import it; new code should call
/// `bridge_ast_to_codegen_scg` instead.
pub fn bridge_scg_to_codegen(scg: &SCG) -> Scg {
    bridge_scg_to_codegen_with_externs(scg, &HashSet::new())
}

/// Bridge the `vuma-scg` SCG to the codegen SCG, with knowledge of which
/// functions are declared as extern (foreign) in the source program.
///
/// When a function call targets a name in `extern_functions`, the resulting
/// `CallNode` gets `is_extern: true`, which causes the backend to emit
/// a relocation entry instead of a local `BL` instruction.
///
/// DEPRECATED: the canonical pipeline now uses [`bridge_ast_to_codegen_scg`]
/// (direct AST→codegen path) because the semantic-SCG → codegen-SCG path
/// produced broken code (segfaults, infinite loops — see Task 4-A in the
/// worklog). This function is retained for the binaries / tests that still
/// import it; new code should call `bridge_ast_to_codegen_scg` instead.
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

            // Note: we do NOT consume the FunctionReturn node here.
            // If we do, the walk's if-body handler cannot reach it when
            // a Return statement is inside an if-then block, causing
            // the Return to be dropped (the then-body walk breaks at
            // the already-consumed FunctionReturn node).
            // The FunctionReturn will be consumed by the walk itself
            // when it reaches it.
            if !body.iter().any(|s| matches!(s, ScgStatement::Return(_))) {
                // The walk didn't reach the FunctionReturn (e.g., loop
                // exit with no outgoing CF). Try to process the
                // FunctionReturn directly to resolve the return value.
                // Only use the reassignment search for functions with loops
                // (where the walk is known to stop at LoopExit).
                let has_loop = body.iter().any(|s| {
                    matches!(s, ScgStatement::Control(ControlNode::Loop { .. }))
                });
                if let Some(ret) = return_node {
                    let df_inputs = edge_idx.incoming_df(ret);
                    let ret_vals: Vec<ScgExpr> = if df_inputs.is_empty() || !has_loop {
                        // No DataFlow inputs or no loops — use simple resolution
                        df_inputs.iter()
                            .enumerate()
                            .map(|(i, _)| resolve_df_input(ret, i, &edge_idx, scg))
                            .collect()
                    } else {
                        df_inputs.iter()
                            .enumerate()
                            .map(|(i, _)| {
                                let source = df_inputs[i].source;
                                if let Some(source_data) = scg.get_node(source) {
                                    if let NodePayload::Computation(comp) = &source_data.payload {
                                        if let ComputationKind::Other(ref label) = comp.kind {
                                            let var_name = label.trim();
                                            let reassign_prefix = format!("{} =", var_name);
                                            let let_prefix = format!("let {}", var_name);
                                            let mut latest_reassign: Option<NodeId> = None;
                                            for node in scg.nodes() {
                                                if !consumed.contains(&node.id) {
                                                    continue;
                                                }
                                                if let NodePayload::Computation(c) = &node.payload {
                                                    if let ComputationKind::Other(ref l) = c.kind {
                                                        if l.starts_with(&reassign_prefix) && !l.starts_with(&let_prefix) {
                                                            latest_reassign = Some(node.id);
                                                        }
                                                    }
                                                }
                                            }
                                            if let Some(reassign_id) = latest_reassign {
                                                return ScgExpr::Var(format!("v_{}", reassign_id.as_u64()));
                                            }
                                        }
                                    }
                                }
                                resolve_df_input(ret, i, &edge_idx, scg)
                            })
                            .collect()
                    };
                    body.push(ScgStatement::Return(ret_vals));
                } else {
                    body.push(ScgStatement::Return(vec![]));
                }
            }

            // Keep results=[] for all functions. The wasm32 backend uses
            // memory to pass the return value (not the wasm return type)
            // because multi-block functions have stack imbalance issues
            // with structured wasm control flow.
            //
            // The return type is still available to the IR builder via
            // the function name (e.g. "fn_main_entry(u64)" → return type
            // u64), which lower_function parses directly.
            scg_nodes.push(ScgNode::Function(ScgFunction {
                name: func_name.clone(),
                params,
                results: vec![],
                body,
                var_types: Default::default(),
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
                let node_stmts = convert_node_to_statement_with_externs(*nid, node_data, &edge_idx, scg, extern_functions);
                body.extend(node_stmts);
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
            var_types: Default::default(),
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
            var_types: Default::default(),
        }));
    }

    Scg { nodes: scg_nodes }
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

// ═══════════════════════════════════════════════════════════════════════════
// Shared post-IR-build pipeline (Wave 1 — Task ID 5)
// ═══════════════════════════════════════════════════════════════════════════

/// Run the shared post-IR-build O2 pipeline on an `IRProgram`.
///
/// This is the single source of truth for the sequence of passes that run
/// AFTER the SCG→IR build and BEFORE register allocation / emission.  Both
/// [`compile_with_path`] and [`compile_modules`] delegate to it, and the
/// test-suite compile path (`src/bin/compile_dump.rs`) is wired into it by
/// Wave 2 so the test suite exercises the FULL production O2 pipeline
/// instead of `run_optimizations` alone (which uses a default latency table
/// and skips lowering / bv_verify / escape+effects).
///
/// The sequence mirrors `compile_with_path`'s historical Stage 8 exactly:
///
/// 1. **Wave 34 lowering** (O1+): `monomorphize`, `lower_closures`,
///    `lower_switches`, `lower_tail_calls`, `normalize_loops`.  Each pass
///    is best-effort (soft-failures are logged, not fatal).
/// 2. **Wave 36 bv_verify**: verify all e-graph rewrite rules are sound.
///    Advisory only — logs a warning on unsound rules, does NOT abort.
/// 3. **Wave 10 syscall allowlist**: reject syscall numbers > 600 (hard
///    error — returns `Err(VumaError::Codegen{...})` on the first
///    violation, matching `stop_on_first_error = true`).
/// 4. **Stage 8b codegen-opt** (O1+): `run_optimizations_with_target_and_inline_threshold`
///    with the REAL backend's latency table (built from `backend_kind`).
/// 5. **Wave 32 escape+effects** (O2+): SROA + alloc elision +
///    interprocedural effect propagation via `run_escape_and_effects_passes`.
///
/// # Parameters
/// - `ir_program`: the freshly-built IR (from `IRBuilder::build` or
///   `ScgToIr::convert`).  Taken by value and returned by value.
/// - `config`: the compile config — used for `opt_level` gating and
///   `inline_threshold`.
/// - `backend_kind`: the backend whose latency table should drive the
///   e-graph cost function and scheduler.  Callers pass the backend they
///   will eventually emit for (e.g. `config.emit_config().backend` for
///   `compile_with_path`, `host_backend_kind()` for `compile_modules`).
/// - `timings`: per-pass timings are pushed onto this Vec (preserving the
///   historical `wave34-lowering` / `wave36-bv-verify` / `codegen-opt` /
///   `escape-effects` keys so `CompilationOutput.stage_timings` is
///   unchanged).
///
/// # Returns
/// - `Ok(IRProgram)` on success.
/// - `Err(VumaError)` if the syscall allowlist rejects a syscall number.
///   The caller is responsible for pushing the error into its `errors`
///   Vec and returning `Err(errors)` (matching the historical behavior
///   where the syscall gate is a hard error that aborts compilation).
///
/// # Memory-safety note
/// The `MemorySafetyAnalyzer` pass (compile_with_path Stage 8, lines ~4906)
/// runs on the codegen SCG BEFORE `IRBuilder::build`, so it CANNOT be
/// encapsulated here (this helper takes `IRProgram`, not the codegen SCG).
/// It stays inline in `compile_with_path`.  Likewise the SCG-liveness
/// `analyze_with_scg_liveness` pass (Stage 6b) runs on the semantic SCG
/// and stays in `compile_with_path`.  Wave 2 will add the codegen-SCG
/// `MemorySafetyAnalyzer` call to `compile_dump` separately if desired.
pub fn run_ir_pipeline(
    mut ir_program: IRProgram,
    config: &CompileConfig,
    backend_kind: vuma_codegen::backend::BackendKind,
    timings: &mut Vec<(String, u64)>,
) -> Result<IRProgram, VumaError> {
    // ── Wave 34: Lowering passes (monomorphize, closures, switch, tail-call,
    //    loop-normalize) — run after SCG→IR build, before the main opt pass.
    //    In VUMA 2.0 these run unconditionally because O3 is mandatory.
    //    Each pass is best-effort: a soft-failure is logged but does not
    //    abort compilation (these are newly-wired passes; the pipeline's
    //    correctness does not yet depend on them).
    {
        let tlower = Instant::now();
        let mut lower = |name: &str, f: fn(&mut IRProgram) -> Result<(), vuma_codegen::backend::BackendError>| {
            if let Err(e) = f(&mut ir_program) {
                vuma_log!(warn, "wave34 lowering pass '{}' soft-failed: {}", name, e);
            }
        };
        lower("monomorphize", vuma_codegen::monomorphize::monomorphize);
        lower("lower_closures", vuma_codegen::closures::lower_closures);
        lower("lower_switches", vuma_codegen::control_flow::lower_switches);
        lower("lower_tail_calls", vuma_codegen::control_flow::lower_tail_calls);
        lower("normalize_loops", vuma_codegen::control_flow::normalize_loops);
        timings.push(("wave34-lowering".to_string(), tlower.elapsed().as_millis() as u64));
    }

    // ── IPC Builtin Lowering ──
    // Expand IPC builtin Calls (channel_open, channel_send, etc.) into
    // IR instruction sequences. This gives ALL backends IPC L0-L8 support
    // without backend-specific inline code. On x86_64, the inline builtins
    // in stack_slot_isel.rs handle IPC directly (they intercept the Call
    // in the instruction selector); this pass is skipped for x86_64 to
    // preserve the well-tested inline implementation. On all other
    // backends (aarch64, riscv64, arm32, etc.), this pass is the ONLY
    // IPC path — it lowers builtins to IR that those backends can emit.
    {
        let tipc = Instant::now();
        // Skip for x86_64 — it has its own inline IPC implementation
        // that is more complete (real CRC32, type_hash, cap sigs, etc.)
        if backend_kind != vuma_codegen::backend::BackendKind::X86_64 {
            for func in &mut ir_program.functions {
                vuma_codegen::ipc_lowering::lower_ipc_builtins(func);
            }
        }
        timings.push(("ipc-lowering".to_string(), tipc.elapsed().as_millis() as u64));
    }

    // ── Wave 36: bv_verify gate — verify all e-graph rewrite rules are sound
    //    BEFORE the opt pass (which runs the e-graph). If any rule is unsound,
    //    log a warning so the user knows the e-graph may miscompile. The gate
    //    is advisory (does not abort) to avoid breaking compilation for
    //    pre-existing rule sets; a future strict mode could promote this to an
    //    error.
    {
        let tverify = Instant::now();
        let results = vuma_codegen::bv_verify::verify_all_rules();
        let unsound: Vec<_> = results.iter().filter(|r| !r.sound).collect();
        if !unsound.is_empty() {
            vuma_log!(warn,
                "wave36 bv_verify: {} unsound e-graph rule(s) detected (compilation may miscompile): {}",
                unsound.len(),
                unsound.iter().map(|r| r.rule_name).collect::<Vec<_>>().join(", ")
            );
        }
        timings.push(("wave36-bv-verify".to_string(), tverify.elapsed().as_millis() as u64));
    }

    // Wave 10: Syscall allowlist — reject obviously invalid syscall numbers
    // at compile time. Since `nr` is arch-specific (Wave 11/12 design), we
    // use a range check rather than a name lookup. Valid Linux syscall
    // numbers are in the range 0..=600 across all supported architectures.
    //
    // This is a HARD error: the helper returns the first violation as a
    // single `VumaError::Codegen`. This matches `compile_with_path`'s
    // historical behavior under the default `stop_on_first_error = true`.
    // (Under `stop_on_first_error = false` the historical path collected
    // all violations into its `errors` Vec; the helper reports only the
    // first, which the caller pushes into its `errors` Vec — a minor
    // behavioral narrowing that does not affect any current test.)
    for func in &ir_program.functions {
        for block in &func.blocks {
            for instr in &block.instructions {
                if let vuma_codegen::ir::IRInstr::Syscall { nr, .. } = instr {
                    if *nr > 600 {
                        return Err(VumaError::Codegen {
                            error: CodegenError::InvalidInstruction(format!(
                                "Invalid syscall number {}: exceeds maximum (600)",
                                nr
                            )),
                        });
                    }
                }
            }
        }
    }

    // Note: lower_syscalls_all() was removed — Wave 11/12 added real
    // IRInstr::Syscall emission to all backends, so the IR flows through
    // to codegen unchanged. The generic_syscall_name() table remains in
    // ir.rs as a utility. The lower_syscalls()/lower_syscalls_all()
    // definitions were also deleted in Wave 10 dead-code cleanup
    // (see TASKS.md).

    // ── Stage 8b: Codegen-Level IR Optimization (production caller) ──
    // Wave 10: Use the ACTUAL backend's latency table for per-ISA optimization.
    // The backend is determined from `backend_kind` (passed by the caller —
    // `config.emit_config().backend` for compile_with_path, `host_backend_kind()`
    // for compile_modules). This means the e-graph cost function and scheduler
    // make decisions based on the real target's instruction latencies, not
    // a generic default.
    //
    // In VUMA 2.0, O3 is mandatory — the codegen-opt pass always runs.
    {
        let topt = Instant::now();
        let latency_table = if let Ok(backend) = vuma_codegen::backend::create_backend(backend_kind) {
            backend.target_info().latency_table()
        } else {
            vuma_codegen::target_desc::LatencyTable::default_ooo()
        };
        ir_program = vuma_codegen::opt::run_optimizations_with_target_and_inline_threshold(
            ir_program,
            &latency_table,
            config.inline_threshold,
        );
        timings.push(("codegen-opt".to_string(), topt.elapsed().as_millis() as u64));
    }

    // (Wave 32) Escape analysis + SROA + alloc elision + interprocedural
    // effect propagation. Runs AFTER the main codegen-opt pass so the
    // analysis sees the post-optimisation IR (and so SROA's cleanup
    // happens before regalloc).  In VUMA 2.0 O3 is mandatory, so this
    // always runs.
    {
        let te = Instant::now();
        let summary = run_escape_and_effects_passes(&mut ir_program);
        vuma_log!(debug,
            "escape+effects: sroa_promoted={} allocs_elided={} pure_fns={}/{}",
            summary.sroa_promoted,
            summary.allocs_elided,
            summary.pure_functions,
            summary.total_functions
        );
        timings.push(("escape-effects".to_string(), te.elapsed().as_millis() as u64));
    }

    // (Wave 4) Auto-vectorization. Runs AFTER escape/effects (so it sees the
    // post-optimization, post-SROA IR) and BEFORE regalloc. The vectorizer
    // performs per-function loop vectorization (counted self-loops with safe
    // bodies — vector loop + scalar remainder) and SLP planning (plan-only;
    // IR is not mutated by SLP — see vectorize.rs docs). Target-agnostic IR
    // rewriting — no latency table needed.  In VUMA 2.0 O3 is mandatory, so
    // this always runs.
    {
        let tv = Instant::now();
        let mut loops_vectorized = 0usize;
        let mut slp_packs = 0usize;
        for func in &mut ir_program.functions {
            let f = std::mem::replace(func, IRFunction::new("__tmp__"));
            let (new_f, plan) = vuma_codegen::vectorize::vectorize_function_with_plan(f);
            if plan.vf > 0 {
                loops_vectorized += 1;
            }
            slp_packs += plan.packed_ops.len();
            *func = new_f;
        }
        vuma_log!(debug,
            "vectorize: loops_vectorized={} slp_packs={}",
            loops_vectorized, slp_packs
        );
        timings.push(("vectorize".to_string(), tv.elapsed().as_millis() as u64));
    }

    Ok(ir_program)
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

    // ── Stage 3b: Interprocedural Allocation Flow ────────────────────
    // Connect call sites to allocation nodes inside callee functions
    // so that IVE can trace `free(caller_var)` to `allocate()` inside
    // the callee.  This runs BEFORE IVE verification (Stage 6) to
    // eliminate false-positive "Resource leak" reports for programs
    // that use factory functions (e.g., `counter = counter_new()`).
    let t = Instant::now();
    let ipaf_pass = vuma_scg::transform::InterproceduralAllocFlow::new();
    let _ = ipaf_pass.run(&mut scg);
    timings.push(("ipaf".to_string(), t.elapsed().as_millis() as u64));

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
        Err(_e) => {
            vuma_log!(warn, "MSG construction soft-failure (non-fatal): {:?}", _e);
            MSG::new()
        }
    };
    timings.push((
        "msg-construction".to_string(),
        t.elapsed().as_millis() as u64,
    ));

    // ── Stage 6: IVE Verification (VUMA 2.0 — MANDATORY) ─────────────
    // There is no `VerificationLevel::None` escape hatch: PMT state
    // verification ALWAYS runs. The only short-circuit is `Quick` mode
    // on a region-less program (no allocations to verify), which mirrors
    // the original intent of `Quick` (cheap syntactic checks).
    let t = Instant::now();
    let verification = if !(msg.region_count() == 0 && config.verification_level == VerificationLevel::Quick) {
        // VUMA 2.0 is PMT-only: every pipeline verification level maps
        // to `IveVerificationLevel::Pmt` (the 3 PMT state verifiers only
        // — the 5 legacy pointer invariants are skipped because pointer
        // syntax is a hard parse error in VUMA 2.0).
        let ive_level = match config.verification_level {
            VerificationLevel::Quick
            | VerificationLevel::Normal
            | VerificationLevel::Exhaustive
            | VerificationLevel::Modular
            | VerificationLevel::ConstantTime
            | VerificationLevel::Hardened => IveVerificationLevel::Pmt,
        };
        let aggregator = InvariantAggregator::new()
            .with_level(ive_level)
            .with_max_paths(config.ive_max_paths)
            .with_max_path_length(config.ive_max_path_length);
        // (VUMA 2.0 PMT-only) Build the PMT layout registry from the
        // AST's `Item::LayoutDef` items so the IVE's `Pmt` level can
        // run the 3 state verifiers (state_read / state_write /
        // state_transform) with full field offset/size info. Without
        // this, every state op would FAIL verification ("layout not
        // found") and the production pipeline would refuse to emit
        // any PMT program that uses state ops.
        let pmt_layouts = build_pmt_layout_specs(&ast);
        let input = vuma_ive::verification::VerificationInput::from_scg(scg.clone())
            .with_pmt_layouts(pmt_layouts);
        let result = aggregator.verify_all(&input);
        // Verification is a hard safety gate: if any invariant was
        // violated, refuse to emit code for the program.  This is
        // independent of `stop_on_first_error` because emitting a binary
        // for a program with known memory-safety violations would defeat
        // the entire purpose of VUMA.
        if result.overall == OverallVerdict::Fail {
            errors.push(VumaError::Verification { result });
            return Err(errors);
        }
        // (Wave 19) `--strict-verification`: treat `Inconclusive` (no
        // violation proven, but some invariants unverified) as a
        // compilation-blocking error. By default Inconclusive is allowed.
        if config.strict_verification
            && result.overall == OverallVerdict::Inconclusive
        {
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

    // ── Stage 6b: Memory Safety Analysis (Wave 20 — blocking pass) ────
    //
    // VUMA 2.0 is PMT-only and memory-safety analysis is MANDATORY —
    // the `CompileConfig.memory_safety` field is retained for API
    // stability but its value is IGNORED here (the analyzer always
    // runs). The `--no-memory-safety` CLI flag has been removed.
    //
    // When enabled, the pipeline runs BOTH:
    //   1. `MemorySafetyAnalyzer::analyze` on the codegen SCG (after
    //      Stage 8 builds it) — detects double-free, dangling pointers,
    //      and simple leaks at the function level.
    //   2. `analyze_with_scg_liveness` on the semantic SCG — uses the
    //      full SCG liveness analysis for precise use-after-free and
    //      uninitialized-read detection.
    //
    // This is a HARD gate: if any violation is found, the pipeline
    // refuses to emit code, independent of `stop_on_first_error`.
    // Emitting a binary for a program with known memory-safety
    // violations would defeat the entire purpose of VUMA.
    let mem_safety_enabled = true; // VUMA 2.0: always on (escape hatch removed)
    if mem_safety_enabled {
        let t = Instant::now();

        // (2) SCG-liveness-based analysis on the semantic SCG.
        // This runs BEFORE codegen because it uses the semantic SCG
        // (`scg`), not the codegen SCG.
        //
        // Wave 20: Only UAF and uninit-read are treated as HARD errors
        // (high confidence).  Leak detection via `find_dead_allocations`
        // is imprecise (it flags write-only allocations that are freed
        // but never read as "dead"), so we run it but only LOG a warning
        // rather than blocking compilation.  The IVE cleanup invariant
        // (Stage 6) already handles real leaks with its static-lifetime
        // analysis.
        let liveness = vuma_scg::liveness::LivenessAnalysis::new(&scg);
        let ms_config_blocking = vuma_codegen::memory_safety::MemorySafetyConfig {
            check_use_after_free: true,
            check_uninitialized_reads: true,
            check_double_free: true,
            check_memory_leaks: false, // IVE Stage 6 handles leaks.
            check_dangling_pointers: false,
            runtime_bounds_checks: false,
            errors_are_fatal: true,
        };
        let liveness_violations =
            vuma_codegen::memory_safety::analyze_with_scg_liveness(
                &liveness, &scg, &ms_config_blocking,
            );

        if !liveness_violations.is_empty() {
            let report = vuma_codegen::memory_safety::MemorySafetyReport {
                violations: liveness_violations,
                ..vuma_codegen::memory_safety::MemorySafetyReport::empty()
            };
            errors.push(VumaError::MemorySafety { report });
            return Err(errors);
        }

        // Leak detection (warning only, non-blocking).  IVE Stage 6
        // already handles real leaks with its static-lifetime analysis.
        let ms_config_leaks = vuma_codegen::memory_safety::MemorySafetyConfig {
            check_use_after_free: false,
            check_uninitialized_reads: false,
            check_double_free: false,
            check_memory_leaks: true,
            check_dangling_pointers: false,
            runtime_bounds_checks: false,
            errors_are_fatal: false,
        };
        let leak_violations =
            vuma_codegen::memory_safety::analyze_with_scg_liveness(
                &liveness, &scg, &ms_config_leaks,
            );
        for lv in &leak_violations {
            vuma_log!(warn, "memory-safety (non-blocking): {}", lv);
        }

        timings.push((
            "memory-safety".to_string(),
            t.elapsed().as_millis() as u64,
        ));
    }
    // (VUMA 2.0: the `else` branch that previously skipped memory-safety
    // analysis with a `--no-memory-safety` warning is now unreachable —
    // `mem_safety_enabled` is always `true`.)

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
            // SCG transform errors (cycles, duplicate edges) are
            // **optimisation soft-failures**, not safety violations.
            // The canonical codegen path (Stage 8) uses the DIRECT
            // AST→codegen SCG bridge, which does not depend on the
            // semantic SCG being acyclic.  IVE verification (Stage 6)
            // is the real safety gate and has already run by this point.
            //
            // Treating these as hard errors would block compilation of
            // any program with loops (which create back-edges in the
            // SCG, triggering "graph contains a cycle" from DCE/CSE).
            // Instead, log them as warnings and continue.
            if !pass_errors.is_empty() {
                vuma_log!(warn, 
                    "SCG transform soft-failures (non-fatal): {} errors: {:?}",
                    pass_errors.len(),
                    pass_errors.first()
                );
            }
        }
    }
    timings.push(("scg-transforms".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 8: IR Lowering ──────────────────────────────────────────
    let t = Instant::now();
    // NOTE: The canonical pipeline now uses the DIRECT AST→codegen SCG
    // bridge (`bridge_ast_to_codegen_scg`) instead of the semantic-SCG →
    // codegen-SCG bridge. The semantic SCG (`scg`) is still built and used
    // for BD inference / MSG / IVE verification / SCG transforms above,
    // but the emitted binary is produced from the AST directly. This avoids
    // the segfaults / infinite loops that the old `bridge_scg_to_codegen*`
    // path produced (Task 4-A).
    let codegen_scg = bridge_ast_to_codegen_scg(&ast);

    // Wave 20: Run the codegen-level MemorySafetyAnalyzer on the codegen
    // SCG.  This complements the SCG-liveness analysis (Stage 6b) with
    // function-level double-free and dangling-pointer detection.  Like
    // Stage 6b, this is a HARD gate (VUMA 2.0: memory safety is mandatory;
    // `config.memory_safety` is ignored).
    if mem_safety_enabled {
        let ms_config = vuma_codegen::memory_safety::MemorySafetyConfig::compile_time_only();
        let analyzer = vuma_codegen::memory_safety::MemorySafetyAnalyzer::new(ms_config);
        let ms_report = analyzer.analyze(&codegen_scg);
        if !ms_report.is_clean() {
            errors.push(VumaError::MemorySafety { report: ms_report });
            return Err(errors);
        }
    }
    let mut ir_builder = IRBuilder::new();
    let mut ir_program = match ir_builder.build(&codegen_scg) {
        Ok(ir) => ir,
        Err(e) => {
            errors.push(VumaError::Codegen { error: e });
            if config.stop_on_first_error {
                return Err(errors);
            }
            return Err(errors); // Cannot continue without IR.
        }
    };

    // ── Stage 8b: Shared post-IR-build O2 pipeline ────────────────────
    // Wave 1 (Task ID 5): the Wave 34 lowering passes (monomorphize,
    // closures, switches, tail-calls, loop-normalize), Wave 36 bv_verify,
    // Wave 10 syscall allowlist, Stage 8b codegen-opt (with the real
    // backend's latency table), and Wave 32 escape+effects passes are now
    // encapsulated in `run_ir_pipeline` — the single source of truth
    // shared with `compile_modules` and (in Wave 2) the test-suite
    // compile path. The backend kind for the latency table is derived
    // from `config.emit_config().backend` (always AArch64 for the
    // canonical Linux/ELF path — preserved exactly). Behavior is
    // identical to the previous inline sequence.
    let backend_kind = config.emit_config().backend;
    ir_program = match run_ir_pipeline(ir_program, config, backend_kind, &mut timings) {
        Ok(ir) => ir,
        Err(e) => {
            errors.push(e);
            return Err(errors);
        }
    };

    let ir_function_count = ir_program.functions.len();
    let ir_instruction_count: usize = ir_program
        .functions
        .iter()
        .map(|f| f.blocks.iter().map(|b| b.instructions.len()).sum::<usize>())
        .sum();
    timings.push(("ir-lowering".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 9: Register Allocation (parallel across functions) ──────
    // Wave 9: Each function's register allocation is independent, so we
    // parallelize across CPU cores using std::thread::scope. This gives a
    // speedup proportional to core count for programs with multiple functions.
    let t = Instant::now();
    let allocator = LinearScanAllocator::new();
    // Parallel: map each function to its allocation result, then collect.
    // Errors are collected alongside successes so we can report all of them.
    let par_results: Vec<(String, Result<AllocationResult, String>)> =
        par_map(&ir_program.functions, |func| {
            let r = allocator.allocate_function(func);
            let r = r.map_err(|e| format!("{}: {}", func.name, e));
            (func.name.clone(), r)
        });
    let mut regalloc_results = Vec::new();
    for (_name, result) in par_results {
        match result {
            Ok(r) => regalloc_results.push(r),
            Err(e) => {
                errors.push(VumaError::RegisterAlloc { message: e });
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
        &regalloc_results,
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
    let mut cor_runtime = {
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
        vuma_log!(info, 
            "cor-init: compiled {} regions incrementally from SCG ({} nodes)",
            recompiled.len(),
            scg.node_count(),
        );
        Some(rt)
    };
    timings.push(("cor-init".to_string(), t.elapsed().as_millis() as u64));

    // ── Wave 38: CORuntime::optimize_module — run the real CoR optimization
    //    passes (HotPathInlining/ColdPathOutline/LoopOptimization/MemoryOptimization,
    //    per Wave 37) on the constructed runtime. Per Wave 38 decision (b),
    //    CoR is profiling-only: this call optimizes the CoR-internal SCG
    //    representation and logs the OptimizationSummary, but does NOT splice
    //    the result back into the emitted user binary (that would require
    //    moving CoR construction before emit_binary — deferred). Gated behind
    //    a runtime check so default compilation is unaffected.
    if let Some(ref mut rt) = cor_runtime {
        let topt = Instant::now();
        match rt.optimize_module() {
            Ok(summary) => {
                vuma_log!(info,
                    "wave38 cor-optimize: nodes {}->{}, edges {}->{}, reoptimized {} regions",
                    summary.scg_node_count_before,
                    summary.scg_node_count_after,
                    summary.scg_edge_count_before,
                    summary.scg_edge_count_after,
                    summary.reoptimized_regions,
                );
                timings.push(("wave38-cor-optimize".to_string(), topt.elapsed().as_millis() as u64));
            }
            Err(e) => {
                vuma_log!(warn, "wave38 cor-optimize soft-failed: {}", e);
            }
        }
    }

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

// ═══════════════════════════════════════════════════════════════════════════
// Multi-module compilation (Wave 48 — Task 7-a)
// ═══════════════════════════════════════════════════════════════════════════

/// Merge a slice of independently-parsed `AstProgram`s into a single
/// `AstProgram`, resolving cross-module `extern "C"` declarations against
/// real `fn` definitions.
///
/// Algorithm:
/// 1. **Duplicate-fn deduplication** (Task 7-c) — collect every `fn`
///    definition across all modules into a `HashMap<String, FnDef>` keyed
///    by name. When a name collides, compare the new definition to the
///    existing one via [`fn_defs_equivalent`] (span-agnostic structural
///    equality — see the helper's doc-comment for the rationale).
///    - **Identical** (modulo spans) → silently drop the duplicate
///      (with a `vuma_log!(debug, ...)` trace). This supports the
///      "each-module-is-self-contained" bootstrap pattern, where each
///      `.vuma` file copy-pastes a small helper preamble
///      (`store_u64`/`load_u64`/`store_u32`/`load_u32`) at the top of
///      the file so the file can be compiled standalone with `vuma run`.
///      Without this dedup, linking the 5 bootstrap files together would
///      produce 14 duplicate-fn errors (4 helper names × 5 files, minus
///      the first occurrences) and `compile_modules` would reject the
///      whole bundle.
///    - **Conflicting** (same name, different signature or body) →
///      return a `VumaError::AstToScg` with a clear message naming the
///      conflicting fn. This is the real "duplicate symbol" case the
///      codegen cannot link.
/// 2. **Extern-block filtering** — iterate every `extern "C" { ... }`
///    block in every module. For each `fn foo(...)` declaration inside
///    the block, check whether `foo` is defined as a real `fn` in any
///    module. If yes, drop that declaration from the block (the real
///    `fn` definition wins — no duplicate symbol). If no, keep the
///    declaration (it's a genuine external like `__vuma_alloc` or
///    `open` that resolves at link time).
/// 3. **Empty-block elision** — if filtering empties an extern block,
///    drop the block entirely (avoids a stray `extern "C" { }` in the
///    merged AST).
/// 4. **Concatenation** — all surviving items (fn defs, struct/enum/
///    region/const/static/import/export/module/trait/impl declarations,
///    filtered extern blocks, and top-level statements) are concatenated
///    into the merged `AstProgram.items` in module-then-declaration
///    order. For fn defs, only the FIRST occurrence of each name survives
///    (subsequent occurrences were either deduplicated in Pass 1 or would
///    have caused Pass 1 to return an error before reaching Pass 2).
///
/// The merged AST is what `compile_modules` feeds into the codegen
/// pipeline. Because cross-module calls are no longer in the merged
/// AST's `extern_fns` set (their extern declarations were stripped),
/// `bridge_ast_to_codegen_scg` treats them as local calls and the
/// backend emits proper local-call relocations that resolve to the
/// sibling module's `fn` body — this is what makes the bootstrap's
/// `parse(...)` / `irb_build_main(...)` / `codegen_emit(...)` /
/// `write_elf64(...)` calls resolve correctly when the five `.vuma`
/// files are linked together.
fn merge_module_asts(module_asts: &[AstProgram]) -> Result<AstProgram, Vec<VumaError>> {
    use vuma_parser::ast::{ExternBlockDef, ExternFnDecl, FnDef};

    let mut errors: Vec<VumaError> = Vec::new();

    // ── Pass 1: deduplicate fn definitions across modules. ──────────
    //
    // Why dedup? The 5 bootstrap `.vuma` files in `womb/lang/` each
    // copy-paste the same 4 helper fns (`store_u64`, `load_u64`,
    // `store_u32`, `load_u32`) at the top of the file as a self-
    // contained preamble, so each file can be compiled standalone with
    // `vuma run <file>` (no inter-file `import` resolution required).
    // When `compile_modules` links all 5 files together, those 4 helper
    // names appear 4-5 times each (18 occurrences total → 14 duplicates
    // after the first occurrences are kept). The pre-Task-7-c policy
    // rejected every duplicate as a hard error, blocking the bootstrap
    // self-host test (`test_wave48_bootstrap_self_host`).
    //
    // Task 7-c replaces that hard-reject policy with a dedup-or-conflict
    // policy: identical duplicates are silently dropped (the bootstrap
    // pattern is legitimate), conflicting duplicates (same name,
    // different signature or body) still produce a hard error (the
    // codegen cannot link two different bodies for the same symbol).
    //
    // Structural equality is span-agnostic: two fns at different source
    // positions (which necessarily have different `Span` byte offsets)
    // compare as equal iff their non-span AST fields are identical. See
    // [`fn_defs_equivalent`] for the comparison mechanism.
    let mut fn_def_map: HashMap<String, FnDef> = HashMap::new();
    for ast in module_asts {
        for item in &ast.items {
            if let Item::FnDef(fn_def) = item {
                match fn_def_map.get(&fn_def.name) {
                    None => {
                        // First occurrence of this name — keep it.
                        fn_def_map.insert(fn_def.name.clone(), fn_def.clone());
                    }
                    Some(existing) => {
                        // Name collision — decide dedup vs. conflict.
                        // Special case: `main` is allowed to differ across
                        // modules — the importing file's `main` (the LAST
                        // one in the module list, since the main file is
                        // appended after all imports) always wins. This
                        // enables the kernel's import pattern where every
                        // module has its own `fn main()` self-test, but
                        // the top-level kernel.vuma's `main` is the real
                        // entry point.
                        if fn_def.name == "main" {
                            // Replace with the later definition.
                            fn_def_map.insert("main".to_string(), fn_def.clone());
                        } else if fn_defs_equivalent(existing, fn_def) {
                            vuma_log!(
                                debug,
                                "merge_module_asts: dedup identical fn '{}' \
                                 (later occurrence silently dropped)",
                                fn_def.name
                            );
                        } else {
                            errors.push(VumaError::AstToScg {
                                message: format!(
                                    "compile_modules: conflicting fn definition '{}' — a later \
                                     occurrence's signature or body differs from the earlier \
                                     definition with the same name. Each fn name may be defined \
                                     in multiple modules only if the definitions are byte-identical \
                                     modulo source spans (the bootstrap's self-contained-preamble \
                                     pattern is allowed); conflicting overloads are not. Compare \
                                     the two definitions' parameter types, return type, and body \
                                     statements to find the divergence.",
                                    fn_def.name
                                ),
                            });
                        }
                    }
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    // The set of fn names defined across all modules (post-dedup). Used
    // by Pass 2's extern-block filtering to decide which `extern "C" {
    // fn foo(...); }` declarations should be stripped (because a real
    // `fn foo` definition exists in some module) vs. kept (because `foo`
    // is a genuine external like `__vuma_alloc` that resolves at link
    // time).
    let fn_def_names: HashSet<String> = fn_def_map.keys().cloned().collect();

    // ── Pass 2: concatenate items, filtering extern blocks and skipping
    //    duplicate fn occurrences (which were verified identical to the
    //    first occurrence in Pass 1; conflicts would have returned an
    //    error before reaching this point). ──────────────────────────
    let mut merged_items: Vec<Item> = Vec::new();
    let mut emitted_fns: HashSet<String> = HashSet::new();
    for ast in module_asts {
        for item in &ast.items {
            match item {
                Item::FnDef(fn_def) => {
                    // First occurrence wins for non-main fns.
                    // For `main`, the LAST occurrence wins (the importing
                    // file's main overrides imported modules' self-test mains).
                    if fn_def.name == "main" {
                        // Remove any previously emitted main, then emit this one.
                        merged_items.retain(|item| {
                            !matches!(item, Item::FnDef(f) if f.name == "main")
                        });
                        merged_items.push(item.clone());
                    } else if emitted_fns.insert(fn_def.name.clone()) {
                        merged_items.push(item.clone());
                    }
                }
                // FIX1: transforms are lowered to codegen exactly like fns
                // (see `bridge_ast_to_codegen_scg` and `to_scg.rs`). For
                // multi-module dedup, treat a `TransformDef` as a first-class
                // function name: first occurrence wins, subsequent
                // occurrences (under the same name) are silently dropped.
                // The item is pushed as-is (`Item::TransformDef`) so the
                // downstream lowering arms in `bridge_ast_to_codegen_scg`
                // and `to_scg.rs` still fire.
                Item::TransformDef(td) => {
                    if emitted_fns.insert(td.name.clone()) {
                        merged_items.push(item.clone());
                    }
                }
                Item::ExternBlock(eb) => {
                    // Keep only the extern fn declarations for which NO
                    // real fn definition exists in any module. The rest
                    // (the ones matched by a real `fn foo(...) { ... }`
                    // in some module) are stripped — the merged AST's
                    // extern_fns set won't contain their names, so the
                    // codegen bridge treats calls to them as local calls.
                    let kept_fns: Vec<ExternFnDecl> = eb
                        .functions
                        .iter()
                        .filter(|fd| !fn_def_names.contains(&fd.name))
                        .cloned()
                        .collect();
                    if !kept_fns.is_empty() {
                        let filtered_eb = ExternBlockDef {
                            convention: eb.convention.clone(),
                            functions: kept_fns,
                            span: eb.span,
                        };
                        merged_items.push(Item::ExternBlock(filtered_eb));
                    }
                    // else: every declaration in this block resolved to a
                    // local fn definition — drop the block entirely.
                }
                _ => merged_items.push(item.clone()),
            }
        }
    }

    Ok(AstProgram {
        items: merged_items,
        span: vuma_parser::Span::synthetic(),
    })
}

/// Compare two [`vuma_parser::ast::FnDef`]s for span-agnostic structural
/// equality (Task 7-c).
///
/// Two fn definitions are "equivalent" iff their ASTs are identical
/// **modulo source spans**. We achieve this by formatting both `FnDef`s
/// with the `Debug` trait (`format!("{:?}", …)`), normalising every
/// `Span { start: N, end: M }` sub-string in each Debug string to the
/// fixed placeholder `Span { start: 0, end: 0 }` via
/// [`normalize_spans_in_debug`], and then comparing the two normalised
/// strings for equality.
///
/// # Why span-agnostic?
///
/// Every AST node carries a `Span` (a byte-offset range into its source
/// file). Two textually-identical fn definitions copy-pasted into two
/// different `.vuma` files have *different* spans (the files have
/// different lengths and the helper preamble appears at different line
/// numbers — see the site map in `test_wave48_bootstrap_self_host`'s
/// doc-comment). A naive `PartialEq` on `FnDef` (which `FnDef` does NOT
/// derive, but if it did) would compare the spans and report the two
/// fns as different even when their source text is byte-identical. That
/// would defeat the dedup policy in [`merge_module_asts`]: every
/// duplicate would look like a conflict and produce a hard error.
///
/// Span-agnostic equality is the correct relation: two fns are "the same
/// preamble helper" iff their source text (modulo whitespace and span
/// positions) parses to the same AST. The Debug-string + span-normalisation
/// approach gives us exactly this relation without requiring us to add
/// `#[derive(PartialEq)]` to ~30 AST types and write a span-erasing
/// visitor pass.
///
/// # What is compared?
///
/// Every non-`span` field of `FnDef` and its transitive sub-types:
/// - `visibility`, `attrs`, `name`, `type_params`, `params`,
///   `return_type`, `body`, `is_async`, `where_clause`.
/// - For `Block`: `statements`.
/// - For `Stmt` variants: every field except `span` (e.g. `LetStmt.name`,
///   `LetStmt.ty`, `LetStmt.value`; `AssignStmt.target`,
///   `AssignStmt.value`; `ReturnStmt.value`; `BdDirectiveStmt.kind`,
///   `BdDirectiveStmt.name`, `BdDirectiveStmt.expr`; etc.).
/// - For `Expr` variants: every field except `span` (e.g. `Var.name`,
///   `Lit.value`, `BinOp.op` / `lhs` / `rhs`, `Call.callee` / `args`,
///   `Syscall.nr` / `args`, etc.).
/// - For `Type` variants: every field (Type has no `span` field — it's
///   pure structural: `BDBase(String)`, `Ptr(Box<Type>)`, etc.).
/// - For `Lit`, `BinOp`, `UnOp`, `BdDirectiveKind`, `CompoundOp`,
///   `CaptureKind`, `Visibility`, `AttrValue`, `Attribute`, `TypeParam`,
///   `WhereClause`, `WherePredicate`, `MatchArm`, `MatchPattern`,
///   `FormatStrPart`, `ClosureBody`, `Param`, `AssignTarget`, etc.: all
///   their non-`span` fields.
///
/// # Failure mode
///
/// `format!("{:?}", …)` is infallible for `FnDef` (every AST type
/// derives `Debug` and contains only `Debug`-able fields), so this
/// function has no failure path: it always returns `true` (the two
/// normalised Debug strings are byte-identical) or `false` (they
/// differ in some non-`span` field). There is no "comparison crashed"
/// case to fall back from.
fn fn_defs_equivalent(a: &vuma_parser::ast::FnDef, b: &vuma_parser::ast::FnDef) -> bool {
    // Span-agnostic comparison via Debug-string normalization (Task 8-a).
    //
    // Previously (Task 7-c) this function used serde_json::to_value +
    // strip_spans. That approach was removed when Wave 43 stripped
    // Serialize/Deserialize derives from the parser's AST types.
    //
    // The current approach: format both FnDefs via Debug, then regex-
    // replace every "Span { start: N, end: M }" pattern with a fixed
    // placeholder "Span { start: 0, end: 0 }" before string comparison.
    // This works because Debug output includes all fields transitively,
    // and Span is the only field that differs between copy-pasted fns.
    let a_dbg = format!("{:?}", a);
    let b_dbg = format!("{:?}", b);
    normalize_spans_in_debug(&a_dbg) == normalize_spans_in_debug(&b_dbg)
}

/// Replace every `Span { start: N, end: M }` pattern in `s` with
/// `Span { start: 0, end: 0 }` so two ASTs at different source positions
/// compare equal. Used by [`fn_defs_equivalent`] for span-agnostic
/// structural equality.
fn normalize_spans_in_debug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"Span { start: ") {
            // Find the closing " }" for this Span struct.
            // Scan forward: skip "Span { start: ", digits, ", end: ", digits, " }".
            let mut j = i + "Span { start: ".len();
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if bytes[j..].starts_with(b", end: ") {
                j += ", end: ".len();
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if bytes[j..].starts_with(b" }") {
                    j += " }".len();
                    out.push_str("Span { start: 0, end: 0 }");
                    i = j;
                    continue;
                }
            }
            // If the pattern didn't match, fall through to copy the byte.
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Detect the host architecture at compile time / runtime and return the
/// matching VUMA backend kind. Used by `compile_modules` so the emitted
/// ELF can be executed natively on the developer's machine (mirrors the
/// `host_isa()` helper in `src/main.rs`).
///
/// Returns `BackendKind::AArch64` (the canonical pipeline's historical
/// default) on unsupported hosts.
fn host_backend_kind() -> vuma_codegen::backend::BackendKind {
    match std::env::consts::ARCH {
        "x86_64" => vuma_codegen::backend::BackendKind::X86_64,
        "aarch64" => vuma_codegen::backend::BackendKind::AArch64,
        "riscv64" => vuma_codegen::backend::BackendKind::RiscV64,
        "arm" => vuma_codegen::backend::BackendKind::Arm32,
        "powerpc64" => vuma_codegen::backend::BackendKind::PowerPC64,
        "powerpc64le" => vuma_codegen::backend::BackendKind::PowerPC64LE,
        "loongarch64" => vuma_codegen::backend::BackendKind::LoongArch64,
        _ => vuma_codegen::backend::BackendKind::AArch64,
    }
}

/// Compile multiple VUMA source modules into a single ELF binary.
///
/// Each `(name, source)` pair is parsed independently into an `AstProgram`.
/// The ASTs are then merged via [`merge_module_asts`]:
/// - `fn` definitions from all modules are concatenated into a single
///   `AstProgram.items` vector. Duplicate `fn` definitions across modules
///   (same name) are **deduplicated**: if the duplicates are byte-identical
///   modulo source spans (the bootstrap's self-contained-preamble
///   pattern), the later occurrences are silently dropped (Task 7-c);
///   if they conflict (same name, different signature or body), a
///   `VumaError::AstToScg` is returned.
/// - `extern "C" { fn foo(...); }` declarations are filtered: if `foo` is
///   defined as a real `fn` in ANY module, the extern declaration is
///   removed (the actual function definition wins — no duplicate symbol).
///   Extern declarations for which no definition exists anywhere are kept
///   (these resolve to real external symbols like `__vuma_alloc`, `open`,
///   `read`, `write` at link time).
///
/// The merged AST is then compiled through the direct
/// AST → codegen SCG → IR → register allocation → backend.encode_program
/// path. This is the same path used by `vuma run --isa <host>` (see
/// `main.rs::compile_to_binary_direct`), so the emitted ELF targets the
/// host architecture and can be executed natively. (The canonical
/// [`compile_with_path`] always emits AArch64; using it here would
/// silently produce a non-runnable binary on x86_64 / riscv64 / etc.
/// hosts, defeating the purpose of multi-module linking for the bootstrap
/// self-host test.)
///
/// Returns a [`CompilationOutput`] whose `binary` field contains the merged
/// ELF (a single executable containing all functions from all modules).
/// The `scg` field is the semantic SCG (best-effort — empty if
/// `ast_to_scg` fails on the merged AST). The `msg`, `verification`,
/// `cor_runtime`, and `debug_info` fields are `None` / empty because the
/// direct path does not run the canonical pipeline's MSG-construction,
/// IVE-verification, or COR-initialization stages.
///
/// # Example
///
/// ```rust,ignore
/// use vuma::pipeline::{compile_modules, CompileConfig};
///
/// let modules: Vec<(String, String)> = vec![
///     ("main.vuma".into(), "extern \"C\" { fn helper(); } fn main() { helper(); }".into()),
///     ("helper.vuma".into(), "fn helper() { }".into()),
/// ];
/// let config = CompileConfig::default();
/// match compile_modules(&modules, &config) {
///     Ok(output) => println!("Linked ELF: {} bytes", output.binary.len()),
///     Err(errors) => for e in &errors { eprintln!("{}", e); },
/// }
/// ```
pub fn compile_modules(
    modules: &[(String, String)],
    config: &CompileConfig,
) -> Result<CompilationOutput, Vec<VumaError>> {
    let mut errors: Vec<VumaError> = Vec::new();
    let mut timings: Vec<(String, u64)> = Vec::new();

    // ── Stage 1: Parse each module independently ───────────────────────
    let t = Instant::now();
    let mut module_asts: Vec<AstProgram> = Vec::with_capacity(modules.len());
    for (_name, source) in modules {
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        if result.has_errors() {
            errors.push(VumaError::Parse {
                errors: result.errors.clone(),
            });
            // Continue parsing the remaining modules so we can report
            // all parse errors at once (rather than aborting on the
            // first module that fails).
            continue;
        }
        module_asts.push(result.unwrap());
    }
    timings.push(("parse-modules".to_string(), t.elapsed().as_millis() as u64));
    if !errors.is_empty() {
        return Err(errors);
    }

    // ── Stage 2: Merge ASTs (dedup externs vs fn defs) ────────────────
    let t = Instant::now();
    let merged_ast = match merge_module_asts(&module_asts) {
        Ok(ast) => ast,
        Err(merge_errors) => {
            errors.extend(merge_errors);
            return Err(errors);
        }
    };
    timings.push(("merge-asts".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 2b: IVE PMT Verification (VUMA 2.0 — MANDATORY) ─────────
    // `vuma link` MUST run PMT state verification — there is no
    // `VerificationLevel::None` escape hatch in VUMA 2.0. This mirrors
    // the Stage 6 gate in `compile_with_path` / `compile_with_recovery`:
    // build the semantic SCG from the merged AST, attach the PMT layout
    // registry, run the 3 PMT state verifiers (state_read / state_write
    // / state_transform) via `InvariantAggregator` at `Pmt` level, and
    // refuse to emit a binary on `Fail` (hard gate). `Inconclusive` is
    // only blocking under `--strict-verification`.
    let t = Instant::now();
    let pmt_scg = match ast_to_scg(&merged_ast) {
        Ok(s) => s,
        Err(e) => {
            errors.push(VumaError::AstToScg { message: format!("{}", e) });
            return Err(errors);
        }
    };
    let pmt_layouts = build_pmt_layout_specs(&merged_ast);
    let aggregator = InvariantAggregator::new()
        .with_level(IveVerificationLevel::Pmt)
        .with_max_paths(config.ive_max_paths)
        .with_max_path_length(config.ive_max_path_length);
    let ive_input = vuma_ive::verification::VerificationInput::from_scg(pmt_scg.clone())
        .with_pmt_layouts(pmt_layouts);
    let verification = aggregator.verify_all(&ive_input);
    timings.push(("ive-verification".to_string(), t.elapsed().as_millis() as u64));
    if verification.overall == OverallVerdict::Fail {
        errors.push(VumaError::Verification { result: verification });
        return Err(errors);
    }
    if config.strict_verification && verification.overall == OverallVerdict::Inconclusive {
        errors.push(VumaError::Verification { result: verification });
        return Err(errors);
    }
    // Hold onto the verification result so it can be surfaced in the
    // final `CompilationOutput`. The semantic SCG built above is also
    // reused as the `CompilationOutput.scg` (Stage 7 below) instead of
    // rebuilding it best-effort.
    let verification = Some(verification);
    let scg = pmt_scg;

    // ── Stage 3: Bridge merged AST → codegen SCG ──────────────────────
    let t = Instant::now();
    let codegen_scg = bridge_ast_to_codegen_scg(&merged_ast);
    timings.push(("ast-to-codegen-scg".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 4: Lower codegen SCG → IR ───────────────────────────────
    let t = Instant::now();
    let mut ir_builder = vuma_codegen::ScgToIr::new();
    let mut ir_program = match ir_builder.convert(&codegen_scg) {
        Ok(ir) => ir,
        Err(e) => {
            errors.push(VumaError::Codegen {
                error: CodegenError::TranslationError(format!(
                    "compile_modules: SCG → IR conversion failed: {}",
                    e
                )),
            });
            return Err(errors);
        }
    };
    timings.push(("ir-lowering".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 4b: Shared post-IR-build O2 pipeline ────────────────────
    // Wave 1 (Task ID 5): compile_modules now delegates to the shared
    // `run_ir_pipeline` helper — the same single source of truth used by
    // `compile_with_path` and (in Wave 2) the test-suite compile path.
    // This means compile_modules now runs the FULL O2 pipeline (Wave 34
    // lowering, Wave 36 bv_verify, Wave 10 syscall allowlist, Stage 8b
    // codegen-opt, Wave 32 escape+effects) instead of just codegen-opt,
    // bringing it into alignment with the production path. The backend
    // kind for the latency table is `host_backend_kind()` (preserved
    // exactly from the previous inline Stage 4b — `backend_kind` is also
    // reused below for the actual register-allocator backend).
    let backend_kind = host_backend_kind();
    ir_program = match run_ir_pipeline(ir_program, config, backend_kind, &mut timings) {
        Ok(ir) => ir,
        Err(e) => {
            errors.push(e);
            return Err(errors);
        }
    };

    let ir_function_count = ir_program.functions.len();
    let ir_instruction_count: usize = ir_program
        .functions
        .iter()
        .map(|f| f.blocks.iter().map(|b| b.instructions.len()).sum::<usize>())
        .sum();

    // Populate the thread-local set of 64-bit-returning function names
    // (mirrors main.rs::compile_to_binary_direct — needed by arm32 and
    // other 32-bit backends for call-return lowering; included here for
    // parity with the production path).
    {
        let func_64bit: HashSet<String> = ir_program
            .functions
            .iter()
            .filter(|f| {
                f.result_types
                    .iter()
                    .any(|t| matches!(t, vuma_codegen::ir::IRType::I64 | vuma_codegen::ir::IRType::U64))
            })
            .map(|f| f.name.clone())
            .collect();
        vuma_codegen::backend::set_64bit_returns(&func_64bit);
    }

    // ── Stage 5: Register allocation (per-function) ───────────────────
    let t = Instant::now();
    let backend = match vuma_codegen::backend::create_backend(backend_kind) {
        Ok(b) => b,
        Err(e) => {
            errors.push(VumaError::Emission {
                message: format!(
                    "compile_modules: cannot create {} backend: {}",
                    backend_kind.isa_name(),
                    e
                ),
            });
            return Err(errors);
        }
    };
    let mut allocated_functions = Vec::new();
    for func in &ir_program.functions {
        match backend.allocate_registers(func) {
            Ok(allocated) => allocated_functions.push(allocated),
            Err(e) => {
                vuma_log!(warn,
                    "compile_modules: register allocation failed for '{}': {}",
                    func.name, e
                );
            }
        }
    }
    if allocated_functions.is_empty() {
        errors.push(VumaError::Emission {
            message: "compile_modules: no functions were successfully allocated \
                      (register allocation failed for all functions in the merged program)"
                .to_string(),
        });
        return Err(errors);
    }
    timings.push(("register-alloc".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 6: Encode program → ELF binary ──────────────────────────
    let t = Instant::now();
    let allocated_program = vuma_codegen::backend::AllocatedProgram {
        functions: allocated_functions,
        total_code_size: 0,
        total_data_size: 0,
        rodata_data: Vec::new(), function_names: std::collections::HashSet::new(),
    };
    let binary = match backend.encode_program(&allocated_program) {
        Ok(bytes) => bytes,
        Err(e) => {
            errors.push(VumaError::Emission {
                message: format!(
                    "compile_modules: {} encode_program failed: {}",
                    backend.name(),
                    e
                ),
            });
            return Err(errors);
        }
    };
    let code_words = count_text_section_instructions(&binary);
    timings.push(("code-emission".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 7: Semantic SCG ─────────────────────────────────────────
    // The semantic SCG was already built in Stage 2b for IVE PMT
    // verification and is reused here as `CompilationOutput.scg`
    // (previously this was a best-effort rebuild — now it is the same
    // graph the verifiers saw, so introspection and verification agree).

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(CompilationOutput {
        binary,
        scg,
        msg: MSG::new(),
        verification,
        stage_timings: timings,
        ir_function_count,
        ir_instruction_count,
        code_words,
        debug_info: None,
        cor_runtime: None,
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
                    return CompileResult::Partial(Box::new($partial_builder));
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
            last_completed_stage: None,
            diagnostics: errors,
        }
    ) {
        Ok(ast) => ast,
        Err(e) => {
            errors.push(e);
            timings.push(("parse".to_string(), t.elapsed().as_millis() as u64));
            return CompileResult::Partial(Box::new(PartialCompilationOutput {
                ast: None,
                scg: None,
                msg: None,
                verification: None,
                stage_timings: timings,
                ir_function_count: None,
                ir_instruction_count: None,
                last_completed_stage: None,
                diagnostics: errors,
            }));
        }
    };
    timings.push(("parse".to_string(), t.elapsed().as_millis() as u64));

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
            last_completed_stage: Some(PipelineStage::Parse),
            diagnostics: errors,
        }
    ) {
        Ok(scg) => scg,
        Err(e) => {
            errors.push(e);
            timings.push(("ast-to-scg".to_string(), t.elapsed().as_millis() as u64));
            return CompileResult::Partial(Box::new(PartialCompilationOutput {
                ast: Some(ast),
                scg: None,
                msg: None,
                verification: None,
                stage_timings: timings,
                ir_function_count: None,
                ir_instruction_count: None,
                last_completed_stage: Some(PipelineStage::Parse),
                diagnostics: errors,
            }));
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
        // Non-fatal: continue with warnings
    }
    timings.push(("scg-validation".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 3b: Interprocedural Allocation Flow ────────────────────
    let t = Instant::now();
    let ipaf_pass = vuma_scg::transform::InterproceduralAllocFlow::new();
    let _ = ipaf_pass.run(&mut scg);
    timings.push(("ipaf".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 4: BD Inference ─────────────────────────────────────────
    let t = Instant::now();
    let inference_engine = InferenceEngine::new();
    let bd_results = inference_engine.infer_types(&scg);
    refine_scg_types_with_bd(&mut scg, &bd_results);
    timings.push(("bd-inference".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 5: MSG Construction (soft failure) ─────────────────────
    let t = Instant::now();
    let msg = match scg_to_msg(&scg) {
        Ok(msg) => msg,
        Err(_e) => {
            // MSG construction is a soft-failure analysis IR, NOT used
            // by the canonical codegen path. Log and continue.
            vuma_log!(warn, "MSG construction soft-failure (non-fatal): {:?}", _e);
            MSG::new()
        }
    };
    timings.push(("msg-construction".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 6: IVE Verification (VUMA 2.0 — MANDATORY) ─────────────
    // There is no `VerificationLevel::None` escape hatch: PMT state
    // verification ALWAYS runs. The only short-circuit is `Quick` mode
    // on a region-less program (no allocations to verify), which mirrors
    // the original intent of `Quick` (cheap syntactic checks).
    let t = Instant::now();
    let verification = if !(msg.region_count() == 0 && config.verification_level == VerificationLevel::Quick) {
        // VUMA 2.0 is PMT-only: every pipeline verification level maps
        // to `IveVerificationLevel::Pmt` (the 3 PMT state verifiers only
        // — the 5 legacy pointer invariants are skipped because pointer
        // syntax is a hard parse error in VUMA 2.0).
        let ive_level = match config.verification_level {
            VerificationLevel::Quick
            | VerificationLevel::Normal
            | VerificationLevel::Exhaustive
            | VerificationLevel::Modular
            | VerificationLevel::ConstantTime
            | VerificationLevel::Hardened => IveVerificationLevel::Pmt,
        };
        let aggregator = InvariantAggregator::new()
            .with_level(ive_level)
            .with_max_paths(config.ive_max_paths)
            .with_max_path_length(config.ive_max_path_length);
        // (VUMA 2.0 PMT-only) Build the PMT layout registry from the
        // AST's `Item::LayoutDef` items so the IVE's `Pmt` level can
        // run the 3 state verifiers (state_read / state_write /
        // state_transform) with full field offset/size info. Without
        // this, every state op would FAIL verification ("layout not
        // found") and the production pipeline would refuse to emit
        // any PMT program that uses state ops.
        let pmt_layouts = build_pmt_layout_specs(&ast);
        let input = vuma_ive::verification::VerificationInput::from_scg(scg.clone())
            .with_pmt_layouts(pmt_layouts);
        let result = aggregator.verify_all(&input);
        // Verification is a hard safety gate: if any invariant was
        // violated, refuse to emit code for the program.  This is
        // independent of `stop_on_first_error` because emitting a binary
        // for a program with known memory-safety violations would defeat
        // the entire purpose of VUMA.
        if result.overall == OverallVerdict::Fail {
            errors.push(VumaError::Verification { result: result.clone() });
            timings.push((
                "ive-verification".to_string(),
                t.elapsed().as_millis() as u64,
            ));
            return CompileResult::Partial(Box::new(PartialCompilationOutput {
                ast: Some(ast),
                scg: Some(scg),
                msg: Some(msg),
                verification: Some(result),
                stage_timings: timings,
                ir_function_count: None,
                ir_instruction_count: None,
                last_completed_stage: Some(PipelineStage::MsgConstruction),
                diagnostics: errors,
            }));
        }
        // (Wave 19) `--strict-verification`: treat `Inconclusive` as blocking.
        if config.strict_verification
            && result.overall == OverallVerdict::Inconclusive
        {
            errors.push(VumaError::Verification { result: result.clone() });
            timings.push((
                "ive-verification".to_string(),
                t.elapsed().as_millis() as u64,
            ));
            return CompileResult::Partial(Box::new(PartialCompilationOutput {
                ast: Some(ast),
                scg: Some(scg),
                msg: Some(msg),
                verification: Some(result),
                stage_timings: timings,
                ir_function_count: None,
                ir_instruction_count: None,
                last_completed_stage: Some(PipelineStage::MsgConstruction),
                diagnostics: errors,
            }));
        }
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

    // ── Stage 8: IR Lowering ──────────────────────────────────────────
    let t = Instant::now();
    // NOTE: The canonical pipeline now uses the DIRECT AST→codegen SCG
    // bridge (`bridge_ast_to_codegen_scg`) instead of the semantic-SCG →
    // codegen-SCG bridge. The semantic SCG (`scg`) is still built and used
    // for BD inference / MSG / IVE verification / SCG transforms above,
    // but the emitted binary is produced from the AST directly. This avoids
    // the segfaults / infinite loops that the old `bridge_scg_to_codegen*`
    // path produced (Task 4-A).
    let codegen_scg = bridge_ast_to_codegen_scg(&ast);
    let mut ir_builder = IRBuilder::new();
    let mut ir_program = match ir_builder.build(&codegen_scg) {
        Ok(ir) => ir,
        Err(e) => {
            errors.push(VumaError::Codegen { error: e });
            timings.push(("ir-lowering".to_string(), t.elapsed().as_millis() as u64));
            return CompileResult::Partial(Box::new(PartialCompilationOutput {
                ast: Some(ast),
                scg: Some(scg),
                msg: Some(msg),
                verification,
                stage_timings: timings,
                ir_function_count: None,
                ir_instruction_count: None,
                last_completed_stage: Some(PipelineStage::ScgTransforms),
                diagnostics: errors,
            }));
        }
    };

    // Wave 10: Syscall allowlist — reject obviously invalid syscall numbers.
    // Since `nr` is arch-specific (Wave 11/12 design), we use a range check.
    let mut had_syscall_error = false;
    for func in &ir_program.functions {
        for block in &func.blocks {
            for instr in &block.instructions {
                if let vuma_codegen::ir::IRInstr::Syscall { nr, .. } = instr {
                    if *nr > 600 {
                        errors.push(VumaError::Codegen {
                            error: CodegenError::InvalidInstruction(format!(
                                "Invalid syscall number {}: exceeds maximum (600)",
                                nr
                            )),
                        });
                        had_syscall_error = true;
                    }
                }
            }
        }
    }
    if had_syscall_error {
        timings.push(("ir-lowering".to_string(), t.elapsed().as_millis() as u64));
        return CompileResult::Partial(Box::new(PartialCompilationOutput {
            ast: Some(ast),
            scg: Some(scg),
            msg: Some(msg),
            verification,
            stage_timings: timings,
            ir_function_count: None,
            ir_instruction_count: None,
            last_completed_stage: Some(PipelineStage::ScgTransforms),
            diagnostics: errors,
        }));
    }

    // Note: lower_syscalls_all() was removed — Wave 11/12 added real
    // IRInstr::Syscall emission to all backends, so the IR flows through
    // to codegen unchanged.

    // ── Stage 8b: Codegen-Level IR Optimization (production caller) ──
    // Wave 10: Use the ACTUAL backend's latency table for per-ISA optimization.
    // In VUMA 2.0 O3 is mandatory, so the codegen-opt pass always runs.
    {
        let topt = Instant::now();
        let emit_config = config.emit_config();
        let latency_table = if let Ok(backend) = vuma_codegen::backend::create_backend(emit_config.backend) {
            backend.target_info().latency_table()
        } else {
            vuma_codegen::target_desc::LatencyTable::default_ooo()
        };
        ir_program = vuma_codegen::opt::run_optimizations_with_target_and_inline_threshold(
            ir_program,
            &latency_table,
            config.inline_threshold,
        );
        timings.push(("codegen-opt".to_string(), topt.elapsed().as_millis() as u64));
    }

    // (Wave 32) Escape analysis + SROA + alloc elision + interprocedural
    // effect propagation.  See `compile` for the full rationale.  In VUMA
    // 2.0 O3 is mandatory, so this always runs.
    {
        let te = Instant::now();
        let summary = run_escape_and_effects_passes(&mut ir_program);
        vuma_log!(debug, 
            "escape+effects (recovery): sroa_promoted={} allocs_elided={} pure_fns={}/{}",
            summary.sroa_promoted,
            summary.allocs_elided,
            summary.pure_functions,
            summary.total_functions
        );
        timings.push(("escape-effects".to_string(), te.elapsed().as_millis() as u64));
    }

    let ir_function_count = ir_program.functions.len();
    let ir_instruction_count: usize = ir_program
        .functions
        .iter()
        .map(|f| f.blocks.iter().map(|b| b.instructions.len()).sum::<usize>())
        .sum();
    timings.push(("ir-lowering".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 9: Register Allocation (parallel across functions) ──────
    // Wave 9: parallelize per-function register allocation with std::thread::scope.
    let t = Instant::now();
    let allocator = LinearScanAllocator::new();
    let par_results: Vec<(String, Result<AllocationResult, String>)> =
        par_map(&ir_program.functions, |func| {
            let r = allocator.allocate_function(func);
            let r = r.map_err(|e| format!("{}: {}", func.name, e));
            (func.name.clone(), r)
        });
    let mut regalloc_results = Vec::new();
    let mut regalloc_failed = false;
    for (_name, result) in par_results {
        match result {
            Ok(r) => regalloc_results.push(r),
            Err(e) => {
                errors.push(VumaError::RegisterAlloc { message: e });
                regalloc_failed = true;
            }
        }
    }
    if regalloc_failed && regalloc_results.is_empty() {
        timings.push(("register-alloc".to_string(), t.elapsed().as_millis() as u64));
        return CompileResult::Partial(Box::new(PartialCompilationOutput {
            ast: Some(ast),
            scg: Some(scg),
            msg: Some(msg),
            verification,
            stage_timings: timings,
            ir_function_count: Some(ir_function_count),
            ir_instruction_count: Some(ir_instruction_count),
            last_completed_stage: Some(PipelineStage::IrLowering),
            diagnostics: errors,
        }));
    }
    timings.push(("register-alloc".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 10: Code Emission (with backend fallback) ───────────────
    let t = Instant::now();
    let emit_config = config.emit_config();
    let binary = match emit_binary(
        &ir_program.functions,
        &ir_program.data_sections,
        &emit_config,
        &regalloc_results,
    ) {
        Ok(binary) => binary,
        Err(e) => {
            let emission_err = format!("{}", e);
            errors.push(VumaError::Emission {
                message: emission_err.clone(),
            });
            timings.push(("code-emission".to_string(), t.elapsed().as_millis() as u64));
            // Return partial — no binary but we have everything else
            return CompileResult::Partial(Box::new(PartialCompilationOutput {
                ast: Some(ast),
                scg: Some(scg),
                msg: Some(msg),
                verification,
                stage_timings: timings,
                ir_function_count: Some(ir_function_count),
                ir_instruction_count: Some(ir_instruction_count),
                last_completed_stage: Some(PipelineStage::RegisterAlloc),
                diagnostics: errors,
            }));
        }
    };
    let code_words = count_text_section_instructions(&binary);
    timings.push(("code-emission".to_string(), t.elapsed().as_millis() as u64));

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
        CompileResult::Success(Box::new(CompilationOutput {
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
        }))
    } else {
        // We have a binary but also some non-fatal errors — still return
        // Success since the binary is valid. Errors can be logged.
        // If the caller needs partial+diagnostics, they should use
        // compile_with_recovery.
        CompileResult::Success(Box::new(CompilationOutput {
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
        }))
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

    // ── Stage 3: SCG Transforms ───────────────────────────────────────
    // VUMA 2.0: verification is mandatory, but `run_scg_transforms`
    // itself does not run IVE — the `verification_level` field is only
    // consulted by the IVE stage (which `compile_to_wasm` does not
    // invoke; Wasm is a sandboxed target and PMT verification is the
    // caller's responsibility). We use `Normal` (the default, which
    // maps to `Pmt` in the pipeline) rather than the removed `None`
    // variant so the config is well-formed.
    let _ = run_scg_transforms(&mut scg, &CompileConfig {
        target: CompileTarget::Wasm32,
        opt_level: OptLevel::O3,
        verification_level: VerificationLevel::Normal,
        strict_verification: false,
        ive_max_paths: 64,
        ive_max_path_length: 256,
        entry_name: "main".to_string(),
        debug_info: false,
        stop_on_first_error: true,
        max_inline_size: 50,
        inline_threshold: vuma_codegen::opt::DEFAULT_INLINE_THRESHOLD,
        memory_safety: true,
        runtime_bounds_checks: false,
        section_headers: false,
    });

    // ── Stage 4: IR Lowering ─────────────────────────────────────
    // NOTE: The canonical pipeline now uses the DIRECT AST→codegen SCG
    // bridge (`bridge_ast_to_codegen_scg`) instead of the semantic-SCG →
    // codegen-SCG bridge. The semantic SCG (`scg`) is still built above
    // for SCG transforms, but the emitted Wasm is produced from the AST
    // directly. This avoids the segfaults / infinite loops that the old
    // `bridge_scg_to_codegen*` path produced (Task 4-A).
    let codegen_scg = bridge_ast_to_codegen_scg(&ast);
    let mut ir_builder = IRBuilder::new();
    let mut ir_program = match ir_builder.build(&codegen_scg) {
        Ok(ir) => ir,
        Err(e) => return Err(vec![VumaError::Codegen { error: CodegenError::ElfError(format!("{}", e)) }]),
    };

    // Wave 10: Syscall allowlist — reject obviously invalid syscall numbers.
    // Since `nr` is arch-specific (Wave 11/12 design), we use a range check.
    let mut allowlist_errors: Vec<VumaError> = Vec::new();
    for func in &ir_program.functions {
        for block in &func.blocks {
            for instr in &block.instructions {
                if let vuma_codegen::ir::IRInstr::Syscall { nr, .. } = instr {
                    if *nr > 600 {
                        allowlist_errors.push(VumaError::Codegen {
                            error: CodegenError::InvalidInstruction(format!(
                                "Invalid syscall number {}: exceeds maximum (600)",
                                nr
                            )),
                        });
                    }
                }
            }
        }
    }
    if !allowlist_errors.is_empty() {
        return Err(allowlist_errors);
    }

    // Note: lower_syscalls_all() was removed — Wave 11/12 added real
    // IRInstr::Syscall emission to all backends, so the IR flows through
    // to codegen unchanged. (lower_syscalls()/lower_syscalls_all()
    // definitions were also deleted in Wave 10 dead-code cleanup.)

    // ── Codegen-Level IR Optimization (production caller) ────────
    ir_program = vuma_codegen::opt::run_optimizations(ir_program);

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
///
/// DEPRECATED: the canonical pipeline now uses
/// [`extract_extern_functions_from_ast`] (which is identical but `pub` so the
/// direct AST→codegen bridge can call it). This private copy is retained
/// because the older `bridge_scg_to_codegen_with_externs` path (also
/// deprecated) used it; the canonical pipeline no longer calls either.
#[allow(dead_code)]
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

/// (Wave 32) Summary of the escape+effects passes run on a program.
///
/// Returned by [`run_escape_and_effects_passes`] and pushed into the
/// pipeline's `stage_timings` as a single `escape-effects` entry so
/// callers can introspect how much work the passes did without
/// re-running them.
#[derive(Debug, Default, Clone, Copy)]
pub struct EscapeAndEffectsSummary {
    /// Number of allocations promoted by SROA (across all functions).
    pub sroa_promoted: usize,
    /// Number of non-escaping alloc/free pairs elided.
    pub allocs_elided: usize,
    /// Number of functions whose final interprocedural effect set is `Pure`.
    pub pure_functions: usize,
    /// Total number of functions analysed.
    pub total_functions: usize,
}

/// (Wave 32) Drive escape-analysis-driven SROA + alloc elision +
/// interprocedural effect analysis on `program`.
///
/// Should be called at O2+ **after** the main codegen-opt pass so the
/// analysis sees the post-optimisation IR (and so SROA's cleanup
/// happens before regalloc).  The effect map is computed via
/// [`effects::analyze_program_effects`] (which now does fixpoint
/// propagation across call edges); this function exposes only the
/// `Pure`-count summary, but a later pass that wants the full map can
/// re-call `analyze_program_effects` directly.
pub fn run_escape_and_effects_passes(program: &mut IRProgram) -> EscapeAndEffectsSummary {
    let mut summary = EscapeAndEffectsSummary {
        total_functions: program.functions.len(),
        ..Default::default()
    };

    // Phase 1: per-function escape analysis + transforms.
    // We recompute `analyze_escapes` per function (rather than calling
    // `analyze_escapes_program` once) so we can mutate each function
    // in-place between analysis and transform.
    for func in &mut program.functions {
        let escape_info = escape_analysis::analyze_escapes(func);
        let sroa = escape_analysis::scalar_replace_aggregates(func, &escape_info);
        let elided = escape_analysis::elide_non_escaping_allocs(func, &escape_info);
        summary.sroa_promoted += sroa;
        summary.allocs_elided += elided;
    }

    // Phase 2: interprocedural effect analysis on the post-transform IR.
    let effects_map = effects::analyze_program_effects(&program.functions);
    summary.pure_functions = effects_map.values().filter(|e| e.is_pure()).count();

    summary
}

/// Run SCG transformation passes.
///
/// In VUMA 2.0, O3 is **mandatory** — every pipeline path runs the full
/// O3 SCG pass set (DCE, constant folding, CSE, inlining, LICM, strength
/// reduction, tail-call detection, dead-region elimination) regardless of
/// the `opt_level` value in the config. The `opt_level` field is kept for
/// API backwards compatibility with downstream code that still
/// constructs a `CompileConfig`, but it has no effect on which SCG passes
/// run.
pub fn run_scg_transforms(scg: &mut SCG, config: &CompileConfig) -> Option<ScgPipelineResult> {
    let mut pm = PassManager::new().verify_between(true).stop_on_error(false);

    // O3 is mandatory — always run the full O3 SCG pass set.
    let _ = config.opt_level; // acknowledged: opt_level is intentionally ignored (O3 mandatory)
    pm.add_pass(DeadCodeElimination::new());
    pm.add_pass(ConstantFolding::new());
    pm.add_pass(CommonSubexpressionElimination::new());
    pm.add_pass(InliningPass::with_max_size(config.max_inline_size));
    pm.add_pass(DeadCodeElimination::new()); // cleanup after inlining
    pm.add_pass(ConstantFolding::new()); // re-fold after inlining
    pm.add_pass(CommonSubexpressionElimination::new());
    // (Wave 26) LICM after inlining+DCE so it sees the post-inline loop
    // structure.
    pm.add_pass(LoopInvariantCodeMotion::new());
    pm.add_pass(DeadCodeElimination::new()); // cleanup after LICM
    // (Wave 33) Strength reduction + tail-call detection +
    // dead-region elimination on the post-inline IR.
    pm.add_pass(StrengthReduction::new());
    pm.add_pass(TailCallOptDetection::new());
    pm.add_pass(DeadRegionElimination::new());
    pm.add_pass(DeadCodeElimination::new()); // final cleanup

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
    /// IVE verification is enabled at `Normal` level. The Liveness
    /// invariant's CFG now includes Derivation edges (mirroring the
    /// cleanup graph fix), so Allocation nodes connected to the
    /// ControlFlow chain only via Derivation edges are correctly
    /// recognized as reachable to their Deallocation nodes.
    #[test]
    fn test_compile_simple_allocation() {
        let source = r#"
            layout Point = { x: u32, y: u32 }
            fn main() -> i32 {
                return 0;
            }
        "#;
        let config = CompileConfig {
            verification_level: VerificationLevel::Normal,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        assert!(result.is_ok(), "Expected successful compilation");
        let output = result.unwrap();
        assert!(!output.binary.is_empty(), "Should produce binary output");
        assert!(output.scg.node_count() > 0, "SCG should have nodes");
        assert!(
            output.verification.is_some(),
            "Verification should run at Normal level"
        );
        // (Wave 19) The stage count is no longer hardcoded — waves 16/18
        // added interprocedural/modular/proof stages. Just verify that
        // timing data was collected for multiple stages.
        assert!(
            output.stage_timings.len() >= 8,
            "Expected at least 8 stages with timing data, got {}",
            output.stage_timings.len()
        );
        assert!(
            output.cor_runtime.is_some(),
            "COR runtime should be initialized"
        );
    }

    /// Test 2: Compile with the legacy `OptLevel::O0` value.
    ///
    /// In VUMA 2.0, O3 is **mandatory** — the `opt_level` field is kept
    /// for API stability but has no effect. Constructing a `CompileConfig`
    /// with `OptLevel::O0` still compiles successfully because every
    /// pipeline path runs the full O3 pass set regardless. This test
    /// pins that backwards-compatibility contract.
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
        assert!(result.is_ok(), "O0 compilation should succeed (O3 is mandatory — full pass set always runs)");
        let output = result.unwrap();
        assert!(
            output.binary.len() >= 64,
            "Even empty program produces ELF header"
        );
    }

    /// Test 3: Compile with O3 (aggressive optimisation).
    ///
    /// IVE verification is enabled at `Normal` level (same as Test 1).
    #[test]
    fn test_compile_aggressive_optimisation() {
        let source = r#"
            layout Point = { x: u32, y: u32 }
            fn process() -> i32 {
                return 42;
            }
            fn main() -> i32 {
                return process();
            }
        "#;
        let config = CompileConfig {
            opt_level: OptLevel::O3,
            verification_level: VerificationLevel::Normal,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        assert!(result.is_ok(), "O3 compilation should succeed");
    }

    /// Test 4: Verification is MANDATORY — there is no `None` bypass.
    /// VUMA 2.0 removed `VerificationLevel::None`; every compile runs
    /// the 3 PMT state verifiers. This test confirms that even an empty
    /// program (no state ops) produces a non-`None` verification result
    /// (it may be `NoChecks` for a region-less program, but it is never
    /// silently skipped).
    #[test]
    fn test_compile_verification_always_runs() {
        let source = r#"
            fn main() {
            }
        "#;
        let config = CompileConfig::default(); // verification_level: Normal
        let result = compile(source, &config);
        assert!(result.is_ok());
        let output = result.unwrap();
        assert!(
            output.verification.is_some(),
            "VUMA 2.0: verification must always run (no None bypass)"
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
        // (Wave 19) Quick mode now runs all 5 invariants at reduced depth.
        // An empty program (no allocations) may skip verification entirely
        // (msg.region_count() == 0), so verification may be None.
        if let Some(verification) = output.verification {
            assert_eq!(
                verification.per_invariant.len(),
                5,
                "Quick should check all 5 invariants at reduced depth (Wave 19)"
            );
        }
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
        assert_eq!(config.opt_level, OptLevel::O3);
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

    // ── Type-aware shift tests (Task 7-A) ────────────────────────────────
    //
    // Verifies that `flatten_expr` chooses `BinOpKind::ShrA` (arithmetic,
    // sign-extending) when the lhs operand is a variable declared with a
    // SIGNED integer type, and `BinOpKind::ShrL` (logical, zero-filling)
    // when the lhs is a variable declared with an UNSIGNED integer type.
    //
    // Background: 6-A changed `map_ast_binop` to map `>>` → `ShrA`
    // unconditionally to fix `bit_abs.vuma` (i64, -42 >> 63). This broke
    // the 3 unsigned-shift gold-standard tests on ALL 10 backends (30
    // failures) — `bit_log2` (u64, expected 3, got 253), `bit_priority_encoder`
    // (u64, expected 7, got 249), and `bit2_priority_encode` (u64, expected 7,
    // got 249). The fix is type-aware: signed → ShrA, unsigned → ShrL.

    /// Test 13: Type-aware right shift — SIGNED operand uses ShrA.
    ///
    /// Source pattern: `x: i64 = -42; y = x >> 63;`
    /// Expected: the `>>` lowers to `BinOpKind::ShrA` (arithmetic shift),
    ///           and the resulting IR BinOp instruction uses `ShrA`.
    #[test]
    fn test_shr_signed_uses_shra() {
        use vuma_parser::ast::{BinOp as AstBinOp, Expr as AstExpr, Lit as AstLit};
        use vuma_parser::Span;

        let mut ctx = BridgeCtx::new();
        // Mimic what `bridge_stmt_to_scg`'s PStmt::Let arm does when it
        // sees `let x: i64 = -42` — record x's declared type as I64.
        ctx.var_types.insert("x".to_string(), ScgType::I64);

        // AST for `x >> 63`
        let shr_expr = AstExpr::BinOp {
            op: AstBinOp::Shr,
            lhs: Box::new(AstExpr::Var {
                name: "x".into(),
                span: Span::synthetic(),
            }),
            rhs: Box::new(AstExpr::Lit {
                value: AstLit::Int(63),
                span: Span::synthetic(),
            }),
            span: Span::synthetic(),
        };

        let mut stmts = Vec::new();
        let result = flatten_expr(&shr_expr, &mut stmts, &mut ctx);

        // The flatten should produce exactly one Computation with op: ShrA.
        let computations: Vec<&ComputationNode> = stmts
            .iter()
            .filter_map(|s| {
                if let ScgStatement::Computation(c) = s {
                    Some(c)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            computations.len(),
            1,
            "Expected exactly 1 Computation node for `x >> 63`"
        );
        assert_eq!(
            computations[0].op,
            BinOpKind::ShrA,
            "Signed `i64 >> 63` must lower to ShrA (arithmetic shift). \
             Got {:?}. If this is ShrL, `bit_abs.vuma` (i64, -42 >> 63) \
             would return 214 (-42 as u8) instead of 42 — this is the \
             regression 6-A introduced and Task 7-A fixes via the \
             type-aware path (signed → ShrA).",
            computations[0].op,
        );
        // Result is a Var referencing the computation's dst.
        assert!(
            matches!(result, ScgExpr::Var(_)),
            "flatten_expr should return a Var referencing the Computation's dst"
        );

        // Also verify the IR: build the SCG, lower to IR, and assert the
        // IR contains a BinOp instruction with op: ShrA. This catches any
        // future regression in lower_computation's shift handling.
        // Append a Return so the function has a proper terminator (the IR
        // builder's CFG rebuild expects one). Capture the dst before moving
        // `stmts` into `ir_body` (the `computations` vec borrows `stmts`).
        let shr_dst = computations[0].dst.clone();
        let mut ir_body = stmts;
        ir_body.push(ScgStatement::Return(vec![ScgExpr::Var(shr_dst)]));
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_shr_signed".into(),
                params: vec![ScgParam {
                    name: "x".into(),
                    ty: ScgType::I64,
                }],
                results: vec![],
                body: ir_body,
                var_types: Default::default(),
            })],
        };
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).expect("IR build should succeed");
        let func = &program.functions[0];
        let has_shra = func.blocks.iter().any(|b| {
            b.instructions.iter().any(|i| {
                matches!(
                    i,
                    vuma_codegen::ir::IRInstr::BinOp {
                        op: BinOpKind::ShrA,
                        ..
                    }
                )
            })
        });
        assert!(
            has_shra,
            "IR should contain a BinOp {{ op: ShrA, .. }} instruction for \
             `i64 >> 63`. If absent, lower_computation's shift handling is \
             dropping the ShrA op kind."
        );
    }

    /// Test 14: Type-aware right shift — UNSIGNED operand uses ShrL.
    ///
    /// Source pattern: `n: u64 = 8; y = n >> 1;`
    /// Expected: the `>>` lowers to `BinOpKind::ShrL` (logical shift),
    ///           and the resulting IR BinOp instruction uses `ShrL`.
    #[test]
    fn test_shr_unsigned_uses_shrl() {
        use vuma_parser::ast::{BinOp as AstBinOp, Expr as AstExpr, Lit as AstLit};
        use vuma_parser::Span;

        let mut ctx = BridgeCtx::new();
        // Mimic `let n: u64 = 8` — record n's declared type as U64.
        ctx.var_types.insert("n".to_string(), ScgType::U64);

        // AST for `n >> 1`
        let shr_expr = AstExpr::BinOp {
            op: AstBinOp::Shr,
            lhs: Box::new(AstExpr::Var {
                name: "n".into(),
                span: Span::synthetic(),
            }),
            rhs: Box::new(AstExpr::Lit {
                value: AstLit::Int(1),
                span: Span::synthetic(),
            }),
            span: Span::synthetic(),
        };

        let mut stmts = Vec::new();
        let result = flatten_expr(&shr_expr, &mut stmts, &mut ctx);

        let computations: Vec<&ComputationNode> = stmts
            .iter()
            .filter_map(|s| {
                if let ScgStatement::Computation(c) = s {
                    Some(c)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            computations.len(),
            1,
            "Expected exactly 1 Computation node for `n >> 1`"
        );
        assert_eq!(
            computations[0].op,
            BinOpKind::ShrL,
            "Unsigned `u64 >> 1` must lower to ShrL (logical shift). \
             Got {:?}. If this is ShrA, `bit_log2.vuma` (u64, 8 >> 1) \
             returns 253 (=-3) instead of 3, and `bit_priority_encoder` \
             / `bit2_priority_encode` (u64) return 249 (=-7) instead of 7 \
             — these are the 30 failures across all 10 backends that \
             Task 7-A fixes via the type-aware path (unsigned → ShrL).",
            computations[0].op,
        );
        assert!(
            matches!(result, ScgExpr::Var(_)),
            "flatten_expr should return a Var referencing the Computation's dst"
        );

        // Also verify the IR contains a BinOp with op: ShrL. Capture the
        // dst before moving `stmts` (the `computations` vec borrows `stmts`),
        // and append a Return so the function has a proper terminator.
        let shr_dst = computations[0].dst.clone();
        let mut ir_body = stmts;
        ir_body.push(ScgStatement::Return(vec![ScgExpr::Var(shr_dst)]));
        let scg = Scg {
            nodes: vec![ScgNode::Function(ScgFunction {
                name: "test_shr_unsigned".into(),
                params: vec![ScgParam {
                    name: "n".into(),
                    ty: ScgType::U64,
                }],
                results: vec![],
                body: ir_body,
                var_types: Default::default(),
            })],
        };
        let mut builder = IRBuilder::new();
        let program = builder.build(&scg).expect("IR build should succeed");
        let func = &program.functions[0];
        let has_shrl = func.blocks.iter().any(|b| {
            b.instructions.iter().any(|i| {
                matches!(
                    i,
                    vuma_codegen::ir::IRInstr::BinOp {
                        op: BinOpKind::ShrL,
                        ..
                    }
                )
            })
        });
        assert!(
            has_shrl,
            "IR should contain a BinOp {{ op: ShrL, .. }} instruction for \
             `u64 >> 1`. If absent, lower_computation's shift handling is \
             dropping the ShrL op kind."
        );
    }

    /// Test 15: Type-aware right shift — UNKNOWN operand defaults to ShrL.
    ///
    /// When the lhs variable has no recorded type (untyped let-binding,
    /// untyped parameter), `flatten_expr` should default to `ShrL`. This
    /// is the safer default for the bit-twiddling idioms in the
    /// gold-standard tests, and it matches the behavior of the previous
    /// (pre-6-A) bridge. The `bit_abs` test (i64) explicitly declares
    /// its type, so it still gets ShrA via the type-aware path.
    #[test]
    fn test_shr_untyped_defaults_to_shrl() {
        use vuma_parser::ast::{BinOp as AstBinOp, Expr as AstExpr, Lit as AstLit};
        use vuma_parser::Span;

        let mut ctx = BridgeCtx::new();
        // NOTE: deliberately do NOT register "x" in ctx.var_types —
        // simulates an untyped `let x = ...` or untyped parameter.

        let shr_expr = AstExpr::BinOp {
            op: AstBinOp::Shr,
            lhs: Box::new(AstExpr::Var {
                name: "x".into(),
                span: Span::synthetic(),
            }),
            rhs: Box::new(AstExpr::Lit {
                value: AstLit::Int(1),
                span: Span::synthetic(),
            }),
            span: Span::synthetic(),
        };

        let mut stmts = Vec::new();
        let _ = flatten_expr(&shr_expr, &mut stmts, &mut ctx);

        let computations: Vec<&ComputationNode> = stmts
            .iter()
            .filter_map(|s| {
                if let ScgStatement::Computation(c) = s {
                    Some(c)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(computations.len(), 1);
        assert_eq!(
            computations[0].op,
            BinOpKind::ShrL,
            "Untyped `x >> 1` should default to ShrL (logical shift). \
             Got {:?}. This default matches the pre-6-A bridge behavior \
             and is safer for the bit-twiddling gold-standard tests.",
            computations[0].op,
        );
    }

    // ── Wave 20: Memory-safety blocking-pass tests ───────────────────

    /// Wave 20 regression test: a program with a use-after-free.
    ///
    /// This test verifies that the memory-safety blocking pass is wired
    /// into the pipeline and runs without crashing.  The SCG-liveness-
    /// based UAF detector (`find_use_after_free`) relies on precise
    /// dataflow edges that may not be present for all UAF patterns in
    /// the current SCG; when the detector DOES catch the UAF, the
    /// pipeline must reject the program with `VumaError::MemorySafety`.
    /// When it does NOT catch it (a known limitation of the current
    /// liveness analysis), the program compiles — this is documented
    /// behavior, not a bug in Wave 20's wiring.
    ///
    /// The `test_wave20_memory_safety_error_variant` test below
    /// verifies the `VumaError::MemorySafety` variant itself, and the
    /// `test_wave20_no_memory_safety_escape_hatch` test verifies the
    /// `--no-memory-safety` flag works.
    #[test]
    fn test_wave20_uaf_rejected_at_compile_time() {
        // VUMA 2.0 is PMT-only: pointer syntax (allocate/free/*ptr) is a
        // hard parse error. This test now verifies that a clean PMT program
        // compiles successfully through the memory-safety pass without
        // crashing. The old UAF scenario (use-after-free with allocate/free)
        // is structurally impossible in PMT — states are linear, and the
        // IVE linearity checker enforces use-after-consume at compile time.
        let source = r#"
            fn main() -> i32 {
                x = 42;
                return x;
            }
        "#;
        let config = CompileConfig::default(); // memory_safety: true
        let result = compile(source, &config);
        // A clean program should compile successfully — the memory-safety
        // pass runs without crashing and finds no violations.
        assert!(
            result.is_ok(),
            "Clean program should compile with memory_safety enabled, got: {:?}",
            result.err()
        );
    }

    /// Wave 20 / VUMA 2.0: the `--no-memory-safety` escape hatch has
    /// been REMOVED — `CompileConfig.memory_safety` is retained for API
    /// stability but its value is IGNORED (the memory-safety analyzer
    /// always runs). This test confirms that a clean PMT program
    /// compiles successfully even when the legacy `memory_safety:
    /// false` field is set (because the field is now a no-op).
    #[test]
    fn test_wave20_no_memory_safety_escape_hatch() {
        // VUMA 2.0 PMT-only: pointer syntax is a hard parse error.
        // Use a clean program to verify that setting `memory_safety:
        // false` in the config is now IGNORED — the memory-safety pass
        // still runs, and a clean PMT program compiles successfully.
        let source = r#"
            fn main() -> i32 {
                x = 42;
                return x;
            }
        "#;
        let config = CompileConfig {
            memory_safety: false, // VUMA 2.0: ignored — pass always runs.
            verification_level: VerificationLevel::Normal,
            ..CompileConfig::default()
        };
        let result = compile(source, &config);
        assert!(
            result.is_ok(),
            "PMT program must compile (memory_safety=false is now ignored), got: {:?}",
            result.err()
        );
    }

    /// Wave 20: a clean program (no UAF, no leaks) must compile
    /// successfully with `memory_safety: true`.  This is a negative test
    /// — the analyzer must NOT produce false positives on well-behaved
    /// programs.
    #[test]
    fn test_wave20_clean_program_compiles_with_memory_safety() {
        // VUMA 2.0 PMT-only: use a simple clean program (no allocation)
        // to verify the memory-safety pass doesn't false-positive on
        // well-behaved code.
        let source = r#"
            fn main() -> i32 {
                x = 42;
                return x;
            }
        "#;
        let config = CompileConfig::default(); // memory_safety: true
        let result = compile(source, &config);
        assert!(
            result.is_ok(),
            "Clean program must compile with memory_safety enabled, got: {:?}",
            result.err()
        );
    }

    /// Wave 20: the `MemorySafety` error variant's `stage()` must return
    /// `"memory-safety"` and its `Display` impl must mention "violation(s)".
    #[test]
    fn test_wave20_memory_safety_error_variant() {
        let report = vuma_codegen::memory_safety::MemorySafetyReport {
            violations: vec![vuma_codegen::memory_safety::MemorySafetyViolation::UseAfterFree {
                allocation_name: "buf".to_string(),
                dealloc_line: Some(5),
                violation_count: 1,
            }],
            ..vuma_codegen::memory_safety::MemorySafetyReport::empty()
        };
        let err = VumaError::MemorySafety { report };
        assert_eq!(err.stage(), "memory-safety");
        let msg = format!("{}", err);
        assert!(msg.contains("memory-safety"), "Display should mention memory-safety: {}", msg);
        assert!(msg.contains("violation"), "Display should mention violation: {}", msg);
    }
}/// Try to convert a while-loop condition into a for-range tuple.
///
/// Recognises patterns like "while (i < 4)" and converts them to
/// (var_name, start, end).
fn parse_while_to_for_range(
    header_id: NodeId,
    label: &str,
    edge_idx: &EdgeIndex,
    scg: &SCG,
) -> Option<(String, ScgExpr, ScgExpr)> {
    let label = label.trim();
    let cond_str = label.strip_prefix("while")?.trim();
    let cond_str = cond_str.strip_prefix('(').unwrap_or(cond_str);
    let cond_str = cond_str.strip_suffix(')').unwrap_or(cond_str);
    let cond_str = cond_str.trim();

    let pos = find_operator(cond_str, "<")?;
    if pos + 1 < cond_str.len() && cond_str.as_bytes()[pos + 1] == b'=' {
        return None;
    }
    let rhs_str = cond_str[pos + 1..].trim();
    // End bound can be a constant or a variable name.
    let end_expr = if let Ok(end) = rhs_str.parse::<i64>() {
        ScgExpr::Int(end)
    } else if rhs_str.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && rhs_str.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        ScgExpr::Var(rhs_str.to_string())
    } else {
        return None;
    };

    let lhs_expr = resolve_df_input(header_id, 0, edge_idx, scg);
    let var_name = match &lhs_expr {
        ScgExpr::Var(name) => name.clone(),
        _ => return None,
    };

    let df_inputs = edge_idx.incoming_df(header_id);
    if df_inputs.is_empty() {
        return None;
    }
    let source = df_inputs[0].source;
    let start = if let Some(src_data) = scg.get_node(source) {
        if let NodePayload::Computation(comp) = &src_data.payload {
            if let ComputationKind::Other(lbl) = &comp.kind {
                if let Some(eq_pos) = lbl.find("= ") {
                    let start_str = lbl[eq_pos + 2..].trim();
                    if let Ok(v) = start_str.parse::<i64>() {
                        ScgExpr::Int(v)
                    } else {
                        ScgExpr::Var(start_str.to_string())
                    }
                } else { ScgExpr::Int(0) }
            } else { ScgExpr::Int(0) }
        } else { ScgExpr::Int(0) }
    } else { ScgExpr::Int(0) };

    Some((var_name, start, end_expr))
}

/// Check if a loop body has any variable reassignment.
///
/// This is used to decide whether the while-to-for-range conversion is safe.
/// If the body reassigns any variable, it might reassign the loop variable,
/// which would cause the for-range counter to diverge. In that case, we
/// use the while-condition guard (Break) instead.
///
/// Recursively scans if/else and nested loop bodies.
fn body_has_any_reassigns(body: &[ScgStatement]) -> bool {
    for stmt in body {
        match stmt {
            ScgStatement::Computation(comp)
                if comp.reassigns.is_some() => {
                    return true;
                }
            ScgStatement::Control(ControlNode::If { then_body, else_body, .. }) => {
                if body_has_any_reassigns(then_body) {
                    return true;
                }
                if let Some(else_b) = else_body {
                    if body_has_any_reassigns(else_b) {
                        return true;
                    }
                }
            }
            ScgStatement::Control(ControlNode::Loop { body, .. })
                if body_has_any_reassigns(body) => {
                    return true;
                }
            ScgStatement::Control(ControlNode::Switch { arms, default_body, .. }) => {
                for arm in arms {
                    if body_has_any_reassigns(&arm.body) {
                        return true;
                    }
                }
                if body_has_any_reassigns(default_body) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

// ═══════════════════════════════════════════════════════════════════════════
// Direct AST → codegen SCG bridge (moved here from `src/main.rs`).
//
// The canonical pipeline (compile_with_path / compile_with_recovery /
// compile_to_wasm) now lowers the binary via this direct bridge instead of
// the older `bridge_scg_to_codegen*` path that converted the semantic SCG →
// codegen SCG.  The semantic SCG is still built and used for BD inference,
// MSG construction, IVE verification, and SCG transforms — but the emitted
// binary is produced from the AST directly, which is the path that the
// `vuma emit` / `vuma compile` / `vuma run` subcommands have always used
// and that (after the #3 fix from task 3-B) lowers statements correctly.
//
// Bug #6-segfault: top-level `Item::Stmt` statements (e.g. a file-scope
// `region buf = allocate(1024);`) used to be silently dropped, so `main`
// segfaulted on the first reference to `buf`.  The bridge now collects
// every top-level statement and either prepends it to `main`'s body or,
// when no `fn main` exists, synthesises a `fn main() -> i64` containing
// them followed by `return 0;`.
// ═══════════════════════════════════════════════════════════════════════════

/// Extract the set of extern function names declared in `extern "C" { ... }`
/// blocks in the AST. These are functions that should be linked against
/// external libraries (e.g. libc) and must be emitted as relocations rather
/// than local branch instructions.
pub fn extract_extern_functions_from_ast(program: &AstProgram) -> HashSet<String> {
    let mut extern_fns = HashSet::new();
    for item in &program.items {
        if let Item::ExternBlock(eb) = item {
            for fn_decl in &eb.functions {
                extern_fns.insert(fn_decl.name.clone());
            }
        }
    }
    extern_fns
}

/// Extract extern function declarations (name + attrs + params) from the AST.
/// Used by Wave 5 (ForeignState lowering) to inspect #[foreign_consume],
/// #[foreign_return], #[callback], etc. at call sites.
pub fn extract_extern_fn_decls_from_ast(
    program: &AstProgram,
) -> HashMap<String, vuma_parser::ast::ExternFnDecl> {
    let mut map = HashMap::new();
    for item in &program.items {
        if let Item::ExternBlock(eb) = item {
            for fn_decl in &eb.functions {
                map.insert(fn_decl.name.clone(), fn_decl.clone());
            }
        }
    }
    map
}

/// Extract layout declarations (name → LayoutDef incl. attrs) from the AST.
/// Used by Wave 5 to check if a layout has #[foreign(raw)].
pub fn extract_layout_decls_from_ast(
    program: &AstProgram,
) -> HashMap<String, vuma_parser::ast::LayoutDef> {
    let mut map = HashMap::new();
    for item in &program.items {
        if let Item::LayoutDef(ld) = item {
            map.insert(ld.name.clone(), ld.clone());
        }
    }
    map
}

/// Convert a parser `Attribute` to a codegen `AttrInfo` (the lightweight
/// attribute view used by the marshal module).
pub fn attr_to_attr_info(attr: &vuma_parser::ast::Attribute) -> vuma_codegen::marshal::AttrInfo {
    vuma_codegen::marshal::AttrInfo {
        name: attr.name.clone(),
        value: attr.value.as_ref().map(|v| match v {
            vuma_parser::ast::AttrValue::Single(s) => s.clone(),
            vuma_parser::ast::AttrValue::List(items) => {
                items.first().cloned().unwrap_or_default()
            }
            vuma_parser::ast::AttrValue::KeyValue { value, .. } => value.clone(),
        }),
    }
}

/// Convert a slice of parser `Attribute`s to codegen `AttrInfo`s.
pub fn attrs_to_attr_infos(attrs: &[vuma_parser::ast::Attribute]) -> Vec<vuma_codegen::marshal::AttrInfo> {
    attrs.iter().map(attr_to_attr_info).collect()
}

/// Bridge a parsed VUMA AST into the codegen crate's SCG representation.
///
/// This is the direct path: AST → codegen SCG. It bypasses the semantic
/// `vuma_scg::SCG` entirely. The semantic SCG is still constructed by the
/// canonical pipeline for BD inference / MSG / IVE verification, but the
/// binary is emitted from the SCG returned by this function.
///
/// Top-level statements (`Item::Stmt`) are injected at the START of `main`'s
/// body so that file-scope `region buf = allocate(N);` declarations execute
/// before `main` references the buffer. If the program has top-level
/// statements but no `fn main`, a synthetic `fn main() -> i64` containing
/// them (followed by `return 0;`) is emitted.
pub fn bridge_ast_to_codegen_scg(program: &AstProgram) -> Scg {
    // Collect extern function names so we can mark calls as is_extern.
    let extern_fns = extract_extern_functions_from_ast(program);

    // Wave 5: collect extern fn declarations (with attrs) and layout
    // declarations (with attrs) so call sites can detect #[foreign_consume],
    // #[foreign_return], #[foreign(raw)], etc.
    let extern_fn_decls = extract_extern_fn_decls_from_ast(program);
    let layout_decls = extract_layout_decls_from_ast(program);

    // Collect global constant definitions so they can be inlined
    // as literal values when referenced in function bodies.
    let global_constants = collect_global_constants(program);

    // Collect void function names (functions with no return type annotation).
    // Used by flatten_expr to emit dst=None for void function calls — critical
    // for wasm32 which loads from mem[0] for non-void calls.
    //
    // FIX1: transforms are lowered exactly like fns, so a transform with no
    // `-> Type` annotation is also "void" for this purpose.
    let void_functions: HashSet<String> = program.items.iter()
        .filter_map(|item| {
            match item {
                Item::FnDef(fn_def) if fn_def.return_type.is_none() => {
                    Some(fn_def.name.clone())
                }
                Item::TransformDef(td) if td.return_type.is_none() => {
                    Some(td.name.clone())
                }
                _ => None,
            }
        })
        .collect();

    // Wave 5: Collect ALL function names (extern + user-defined) so flatten_expr
    // can detect when a Var refers to a function address (e.g., `my_fn as u64`).
    let mut function_names: HashSet<String> = extern_fns.clone();
    for item in &program.items {
        match item {
            Item::FnDef(fn_def) => { function_names.insert(fn_def.name.clone()); }
            Item::TransformDef(td) => { function_names.insert(td.name.clone()); }
            _ => {}
        }
    }

    // FIX1: build a map of `fn/transform name → State<L> layout name` for
    // every function or transform whose return type is `State<LayoutName>`.
    // When a `let c = add_points(p, q);` call binds the result of such a
    // callee, `c` is registered as state-typed with that layout so
    // subsequent `c.field` accesses resolve to Loads at the right offset.
    // Without this, callers must annotate `let c: State<L> = ...` manually.
    let state_returning_fns: HashMap<String, String> = program.items.iter()
        .filter_map(|item| {
            let (name, ret_ty) = match item {
                Item::FnDef(fn_def) => (&fn_def.name, &fn_def.return_type),
                Item::TransformDef(td) => (&td.name, &td.return_type),
                _ => return None,
            };
            ret_ty.as_ref().and_then(|ty| {
                extract_state_layout_name_from_ast(ty).map(|ln| (name.clone(), ln))
            })
        })
        .collect();

    // PMT (Wave 2): build the layout registry from all `Item::LayoutDef`
    // items. Cloned into each function's BridgeCtx so state.field accesses
    // can resolve field offsets/types at bridge time.
    let layouts = build_layout_registry(program);

    // ── Collect top-level statements ─────────────────────────────────
    //
    // Top-level `Item::Stmt` items (e.g. `region buf = allocate(1024);`,
    // `let x = 5;`, standalone `free(ptr);`, etc.) execute at program start.
    // The old bridge dropped them silently, which caused `main` to
    // segfault when it referenced a buffer declared at file scope.
    //
    // We use a dedicated BridgeCtx (separate from any function's ctx) so
    // temp names don't collide with those used inside `main`'s body.

    // Wave 1: Create a shared string table for all function contexts.
    // Each ctx gets a clone of the Rc, so string literals are deduplicated
    // program-wide. After all functions are processed, the table is drained
    // and emitted as a single ScgNode::Data (ReadOnly) section.
    let shared_string_table: std::rc::Rc<std::cell::RefCell<Vec<(String, Vec<u8>)>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut top_level_stmts: Vec<ScgStatement> = Vec::new();
    {
        let mut tl_ctx = BridgeCtx::new();
        tl_ctx.extern_fns = extern_fns.clone();
        tl_ctx.global_constants = global_constants.clone();
        tl_ctx.layouts = layouts.clone();
        tl_ctx.extern_fn_decls = extern_fn_decls.clone();
        tl_ctx.layout_decls = layout_decls.clone();
        tl_ctx.state_returning_fns = state_returning_fns.clone();
        tl_ctx.function_names = function_names.clone();
        tl_ctx.string_table = shared_string_table.clone();
        for item in &program.items {
            if let Item::Stmt(stmt) = item {
                top_level_stmts.extend(bridge_stmt_to_scg(stmt, &mut tl_ctx));
            }
        }
    }

    let mut nodes: Vec<ScgNode> = Vec::new();

    // FIX1: `Item::TransformDef` is lowered to codegen exactly like
    // `Item::FnDef` — synthesize an `FnDef` with the transform's name,
    // params, return type, and body, then process it through the same
    // codegen path. The transform's `Vec<Stmt>` body is wrapped in a
    // `Block` (the structural type `FnDef` expects).
    for item in &program.items {
        // Produce a borrowed `FnDef` to process. For a real `Item::FnDef`
        // we borrow it directly; for `Item::TransformDef` we synthesize an
        // owned `FnDef` and borrow that. Both arms below share the same
        // processing code via a unified `&FnDef` reference.
        let synthesized: vuma_parser::ast::FnDef;
        let fn_def: &vuma_parser::ast::FnDef = match item {
            Item::FnDef(fd) => fd,
            Item::TransformDef(td) => {
                synthesized = vuma_parser::ast::FnDef {
                    visibility: vuma_parser::ast::Visibility::Private,
                    attrs: Vec::new(),
                    name: td.name.clone(),
                    type_params: Vec::new(),
                    params: td.params.clone(),
                    return_type: td.return_type.clone(),
                    body: vuma_parser::ast::Block {
                        statements: td.body.clone(),
                        span: td.span,
                    },
                    is_async: false,
                    where_clause: None,
                    span: td.span,
                };
                &synthesized
            }
            _ => continue,
        };
        {
            let params: Vec<ScgParam> = fn_def
                .params
                .iter()
                .map(|p| ScgParam {
                    name: p.name.clone(),
                    ty: bridge_type_to_codegen_scg(&p.ty),
                })
                .collect();

            let results = if let Some(ref ret_ty) = fn_def.return_type {
                vec![bridge_type_to_codegen_scg(&Some(ret_ty.clone()))]
            } else {
                vec![]
            };

            let mut ctx = BridgeCtx::new();
            ctx.extern_fns = extern_fns.clone();
            ctx.global_constants = global_constants.clone();
            ctx.void_functions = void_functions.clone();
            ctx.layouts = layouts.clone();
            ctx.extern_fn_decls = extern_fn_decls.clone();
            ctx.layout_decls = layout_decls.clone();
            ctx.state_returning_fns = state_returning_fns.clone();
            ctx.function_names = function_names.clone();
            ctx.string_table = shared_string_table.clone();
            // PMT (Wave 2): register state-typed params (those with
            // `State<L>` type annotation) so `param.field` accesses inside
            // the body lower to Loads with the layout's field offsets.
            for p in &fn_def.params {
                if let Some(ref ty) = p.ty {
                    if let Some(layout_name) = extract_state_layout_name_from_ast(ty) {
                        ctx.state_var_layouts.insert(p.name.clone(), layout_name);
                    }
                }
            }
            let mut body = bridge_block_to_scg_stmts(&fn_def.body, &mut ctx);

            // Ensure every function ends with a Return statement.
            // If the body doesn't end with a Return, add an implicit one.
            // When the function has a return type and the last statement was an
            // expression, use ctx.last_expr_result as the return value.
            let has_return = body.last().is_some_and(|s| matches!(s, ScgStatement::Return(_)));
            if !has_return {
                let ret_val = if !results.is_empty() {
                    // First check if the last expression was tracked.
                    if let Some(ref expr) = ctx.last_expr_result {
                        Some(expr.clone())
                    } else {
                        // Otherwise, look for the last computation/call result.
                        body.iter().rev().find_map(|s| match s {
                            ScgStatement::Computation(comp) => Some(ScgExpr::Var(comp.dst.clone())),
                            ScgStatement::Call(call) => call.dst.as_ref().map(|d| ScgExpr::Var(d.clone())),
                            _ => None,
                        })
                    }
                } else {
                    None
                };
                body.push(ScgStatement::Return(ret_val.into_iter().collect()));
            }

            nodes.push(ScgNode::Function(ScgFunction {
                name: fn_def.name.clone(),
                params,
                results,
                body,
                var_types: ctx.var_types.clone(),
            }));
        }
    }

    // ── Inject top-level statements ──────────────────────────────────
    //
    // If there are top-level statements, prepend them to `main`'s body so
    // they run before `main`'s own statements. If no `fn main` exists,
    // synthesise one containing them followed by `return 0;`.
    if !top_level_stmts.is_empty() {
        let main_idx = nodes.iter().position(|n| {
            matches!(n, ScgNode::Function(f) if f.name == "main")
        });
        match main_idx {
            Some(i) => {
                if let ScgNode::Function(f) = &mut nodes[i] {
                    let mut original_body = std::mem::take(&mut f.body);
                    top_level_stmts.append(&mut original_body);
                    f.body = top_level_stmts;
                }
            }
            None => {
                // No `fn main` in the program. Synthesise one so the
                // top-level statements actually execute at runtime.
                top_level_stmts.push(ScgStatement::Return(vec![ScgExpr::Int(0)]));
                nodes.push(ScgNode::Function(ScgFunction {
                    name: "main".to_string(),
                    params: vec![],
                    results: vec![ScgType::I64],
                    body: top_level_stmts,
                    var_types: Default::default(),
                }));
            }
        }
    }

    // Wave 1: Emit the string table as a read-only data section.
    // All string literals collected by flatten_expr are concatenated
    // (NUL-terminated) and placed in .rodata. The addresses were already
    // computed as compile-time constants (rodata_vaddr + offset).
    {
        let table = shared_string_table.borrow();
        if !table.is_empty() {
            let mut string_data: Vec<u8> = Vec::new();
            for (_label, bytes) in table.iter() {
                string_data.extend_from_slice(bytes);
                string_data.push(0); // NUL terminator
            }
            nodes.push(ScgNode::Data(ScgData {
                name: "vuma_strings".to_string(),
                kind: DataSectionKind::ReadOnly,
                align: 1,
                data: string_data,
            }));
        }
    }

    Scg { nodes }
}

/// Context for the AST → codegen SCG bridge, tracking a monotonic temp counter
/// and the last expression result for implicit returns.
pub struct BridgeCtx {
    temp_counter: u32,
    /// The result of the last expression statement, if any.
    /// Used for implicit return when a function body ends with an expression.
    pub last_expr_result: Option<ScgExpr>,
    /// Set of extern function names (from `extern "C" { ... }` blocks).
    /// Used to mark CallNodes as is_extern when the target is declared extern.
    pub extern_fns: HashSet<String>,
    /// Global constant definitions: maps name → integer value.
    /// Populated by scanning top-level `const`, `static`, and type-ascription
    /// declarations before processing any function bodies.
    pub global_constants: HashMap<String, i64>,
    /// Variable name → declared type (from `let x: T = ...` annotations).
    /// Populated by the `PStmt::Let` arm of `bridge_stmt_to_scg`. Used by
    /// `flatten_expr` to choose `ShrA` (arithmetic, signed) vs `ShrL`
    /// (logical, unsigned) for the `>>` operator — see Task 7-A. Without
    /// this, all `>>` operations collapse to a single shift kind, breaking
    /// either `bit_abs` (i64 → needs ShrA) or `bit_log2` (u64 → needs ShrL).
    pub var_types: HashMap<String, ScgType>,
    /// Set of user-defined function names that return void (no return type).
    /// Populated by `bridge_ast_to_codegen_scg` before processing function
    /// bodies. Used by `flatten_expr` to emit `dst: None` for void function
    /// calls — critical for wasm32 which loads from mem[0] for non-void calls.
    pub void_functions: HashSet<String>,
    /// PMT (Wave 2): layout definitions — maps layout name → (total_size,
    /// fields). Each field is `(name, ir_type, byte_offset, byte_size,
    /// type_name)` where `type_name` is the field's declared type as a
    /// string (e.g. "u32", "Point") — used to descend into nested
    /// layout-typed fields.
    /// Built once from `Item::LayoutDef` items at the start of
    /// `bridge_ast_to_codegen_scg` and cloned into each function's ctx.
    pub layouts: HashMap<String, (u64, Vec<(String, vuma_codegen::ir::IRType, u64, u64, String)>)>,
    /// PMT (Wave 2): state-typed variable → layout name. Populated per
    /// function: state-typed params are registered before the body is
    /// lowered, and `let p = state_new(L)` / `let p: State<L> = ...` add
    /// entries as they're processed.
    pub state_var_layouts: HashMap<String, String>,
    /// Wave 5 (ForeignState): extern fn name → its declaration (incl. attrs).
    /// Used at call sites to detect #[foreign_consume], #[foreign_return],
    /// #[callback], etc. Populated by `bridge_ast_to_codegen_scg`.
    pub extern_fn_decls: HashMap<String, vuma_parser::ast::ExternFnDecl>,
    /// Wave 5 (ForeignState): layout name → its declaration (incl. attrs).
    /// Used to check if a layout has #[foreign(raw)]. Populated by
    /// `bridge_ast_to_codegen_scg`.
    pub layout_decls: HashMap<String, vuma_parser::ast::LayoutDef>,
    /// FIX1: fn/transform name → `State<L>` layout name, for every function
    /// or transform whose return type is `State<LayoutName>`. When a
    /// `let c = callee(...)` call binds such a callee's result, `c` is
    /// registered in `state_var_layouts` with this layout so subsequent
    /// `c.field` accesses resolve to Loads at the right offset — without
    /// requiring the caller to annotate `let c: State<L> = ...`.
    pub state_returning_fns: HashMap<String, String>,
    /// Wave 1: program-wide string literal table. Each entry is
    /// (label, bytes_including_NUL). Shared across all function contexts
    /// via Rc<RefCell<>> so string literals are deduplicated program-wide.
    /// The table is drained after all functions are processed and emitted
    /// as a single ScgNode::Data (ReadOnly) section.
    pub string_table: std::rc::Rc<std::cell::RefCell<Vec<(String, Vec<u8>)>>>,
    /// Wave 5: Set of all function names (extern + user-defined). Used by
    /// flatten_expr to detect when a Var refers to a function (not a variable)
    /// and produce ScgExpr::Label for function-address expressions.
    pub function_names: HashSet<String>,
}

impl Default for BridgeCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl BridgeCtx {
    /// Construct a fresh, empty bridge context.
    pub fn new() -> Self {
        Self {
            temp_counter: 0,
            last_expr_result: None,
            extern_fns: HashSet::new(),
            global_constants: HashMap::new(),
            var_types: HashMap::new(),
            void_functions: HashSet::new(),
            layouts: HashMap::new(),
            state_var_layouts: HashMap::new(),
            extern_fn_decls: HashMap::new(),
            layout_decls: HashMap::new(),
            state_returning_fns: HashMap::new(),
            string_table: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            function_names: HashSet::new(),
        }
    }

    /// Allocate a unique temporary variable name.
    pub fn alloc_temp(&mut self) -> String {
        let name = format!("__t{}", self.temp_counter);
        self.temp_counter += 1;
        name
    }
}

/// Try to evaluate a constant expression to an integer value.
///
/// Handles integer literals, boolean literals, and simple binary operations
/// on constant sub-expressions. Returns `None` for non-constant expressions
/// (variable references, function calls, etc.).
pub fn eval_const_expr(expr: &vuma_parser::ast::Expr, consts: &HashMap<String, i64>) -> Option<i64> {
    use vuma_parser::ast::{Expr, Lit, BinOp, UnOp};
    match expr {
        Expr::Lit { value, .. } => match value {
            Lit::Int(n) => Some(*n),
            Lit::Bool(b) => Some(if *b { 1 } else { 0 }),
            _ => None,
        },
        Expr::Var { name, .. } => consts.get(name).copied(),
        Expr::BinOp { op, lhs, rhs, .. } => {
            let l = eval_const_expr(lhs, consts)?;
            let r = eval_const_expr(rhs, consts)?;
            Some(match op {
                BinOp::Add => l.wrapping_add(r),
                BinOp::Sub => l.wrapping_sub(r),
                BinOp::Mul => l.wrapping_mul(r),
                BinOp::Div => l.checked_div(r)?,
                BinOp::Mod => l.checked_rem(r)?,
                BinOp::BitAnd => l & r,
                BinOp::BitOr => l | r,
                BinOp::BitXor => l ^ r,
                BinOp::Shl => l.wrapping_shl(r as u32),
                BinOp::Shr => l.wrapping_shr(r as u32),
                BinOp::And => if l != 0 && r != 0 { 1 } else { 0 },
                BinOp::Or => if l != 0 || r != 0 { 1 } else { 0 },
                BinOp::Eq => if l == r { 1 } else { 0 },
                BinOp::Ne => if l != r { 1 } else { 0 },
                BinOp::Lt => if l < r { 1 } else { 0 },
                BinOp::Le => if l <= r { 1 } else { 0 },
                BinOp::Gt => if l > r { 1 } else { 0 },
                BinOp::Ge => if l >= r { 1 } else { 0 },
            })
        }
        Expr::UnOp { op, expr, .. } => {
            let v = eval_const_expr(expr, consts)?;
            Some(match op {
                UnOp::Neg => v.wrapping_neg(),
                UnOp::Not => if v == 0 { 1 } else { 0 },
                UnOp::BitNot => !v,
                UnOp::Deref => return None, // can't const-eval deref
            })
        }
        Expr::Cast { expr, .. } | Expr::TypeAscription { expr, .. } => {
            eval_const_expr(expr, consts)
        }
        _ => None,
    }
}

/// Collect global constant definitions from the top-level program items.
///
/// Scans for `Item::Const`, `Item::Static`, and `Item::Stmt(Stmt::Let)` that
/// have constant-evaluable initializers and builds a name → value map.
pub fn collect_global_constants(program: &AstProgram) -> HashMap<String, i64> {
    let mut consts: HashMap<String, i64> = HashMap::new();

    for item in &program.items {
        match item {
            Item::Const(c) => {
                if let Some(val) = eval_const_expr(&c.value, &consts) {
                    consts.insert(c.name.clone(), val);
                }
            }
            Item::Static(s) => {
                if let Some(val) = eval_const_expr(&s.value, &consts) {
                    consts.insert(s.name.clone(), val);
                }
            }
            Item::Stmt(vuma_parser::ast::Stmt::Let(let_stmt)) => {
                if let Some(val) = eval_const_expr(&let_stmt.value, &consts) {
                    consts.insert(let_stmt.name.clone(), val);
                }
            }
            _ => {}
        }
    }

    consts
}

/// Convert a parser type annotation to a codegen SCG type.
pub fn bridge_type_to_codegen_scg(ty: &Option<vuma_parser::ast::Type>) -> ScgType {
    match ty {
        Some(vuma_parser::ast::Type::BDBase(name)) => match name.as_str() {
            "i8" => ScgType::I8,
            "i16" => ScgType::I16,
            "i32" => ScgType::I32,
            "i64" => ScgType::I64,
            "u8" => ScgType::U8,
            "u16" => ScgType::U16,
            "u32" => ScgType::U32,
            "u64" => ScgType::U64,
            "f32" => ScgType::F32,
            "f64" => ScgType::F64,
            _ => ScgType::I64,
        },
        Some(vuma_parser::ast::Type::Ptr(_)) => ScgType::Ptr,
        Some(vuma_parser::ast::Type::RegionPtr { .. }) => ScgType::Ptr,
        // Wave 1c: Bridge `Channel<T>` through the pipeline.  Channel values
        // are opaque IPC handles — pointer-sized — but we preserve the inner
        // payload type via `ScgType::Channel` so downstream stages can
        // recover it when lowering send/recv operations.
        //
        // Wave 89-90 (Session Types): the `session_type` field on the AST
        // node is dropped here — ScgType::Channel currently carries only
        // the payload type. A future SCG extension could add a session-
        // type slot to ScgType::Channel to thread the protocol through to
        // the IVE linear-type checker (Wave 95).
        Some(vuma_parser::ast::Type::Channel { inner, .. }) => {
            // `inner: &Box<Type>` (match ergonomics on `&Option<Type>`).
            // Recursively bridge the payload type so ScgType::Channel
            // carries the inner type for downstream type-checking.
            ScgType::Channel(Box::new(bridge_type_to_codegen_scg(&Some(
                inner.as_ref().clone(),
            ))))
        }
        // FIX1: a `State<L>` value is represented at runtime as a pointer
        // to a stack-allocated buffer (see `state_new` lowering →
        // `AllocationNode::Stack` with `ty: ScgType::Ptr`). Mapping
        // `State<_>` to `Ptr` here lets transform/fn signatures with
        // `State<L>` return types lower correctly (the function returns
        // the buffer pointer) and lets `let p: State<L> = ...` bindings
        // register a pointer var type for shift-inference.
        Some(vuma_parser::ast::Type::State(_)) => ScgType::Ptr,
        _ => ScgType::Void,
    }
}

/// PMT (Wave 2): convert a parser `Type` to a codegen `IRType` for layout
/// field computation. Returns `U64` for unknown/aggregate types (safe
/// default — the field's bytes are still read/written correctly).
fn bridge_type_to_ir_type(ty: &vuma_parser::ast::Type) -> vuma_codegen::ir::IRType {
    use vuma_parser::ast::Type;
    match ty {
        Type::BDBase(name) => match name.as_str() {
            "i8" => vuma_codegen::ir::IRType::I8,
            "i16" => vuma_codegen::ir::IRType::I16,
            "i32" => vuma_codegen::ir::IRType::I32,
            "i64" => vuma_codegen::ir::IRType::I64,
            "u8" => vuma_codegen::ir::IRType::U8,
            "u16" => vuma_codegen::ir::IRType::U16,
            "u32" => vuma_codegen::ir::IRType::U32,
            "u64" => vuma_codegen::ir::IRType::U64,
            _ => vuma_codegen::ir::IRType::U64,
        },
        Type::Ptr(_) | Type::RegionPtr { .. } => vuma_codegen::ir::IRType::U64,
        // Wave 1c: `Channel<T>` → `IRType::Channel` (pointer-sized opaque
        // capability handle; inner payload type carried for type-checking
        // only).
        // Wave 89-90 (Session Types): session_type field is dropped here;
        // IRType::Channel doesn't carry protocol info yet.
        Type::Channel { inner, .. } => {
            vuma_codegen::ir::IRType::Channel(Box::new(bridge_type_to_ir_type(inner)))
        }
        _ => vuma_codegen::ir::IRType::U64,
    }
}

/// PMT (Wave 2): compute the byte size of a parser `Type` for layout field
/// offset computation.
fn bridge_type_size(ty: &vuma_parser::ast::Type) -> u64 {
    use vuma_parser::ast::Type;
    match ty {
        Type::BDBase(name) => match name.as_str() {
            "i8" | "u8" | "bool" => 1,
            "i16" | "u16" => 2,
            "i32" | "u32" | "f32" => 4,
            "i64" | "u64" | "f64" => 8,
            _ => 8,
        },
        Type::Ptr(_) | Type::RegionPtr { .. } => 8,
        // Wave 1c: `Channel<T>` is pointer-sized (8 on 64-bit, 4 on 32-bit) —
        // same as Ptr.
        // Wave 89-90: session_type field doesn't affect size.
        Type::Channel { .. } => 8,
        Type::Array { element, size } => bridge_type_size(element) * (*size as u64),
        _ => 8,
    }
}

/// PMT: compute the byte size of a parser `Type`, looking up user-defined
/// layout names in the provided `layout_sizes` map. This fixes the nested-
/// layout bug where `bridge_type_size` returned 8 for any user-defined
/// layout name (e.g. `WaitQueue` inside a `Pipe` layout), causing field
/// offset corruption.
fn bridge_type_size_with_layouts(
    ty: &vuma_parser::ast::Type,
    layout_sizes: &HashMap<String, u64>,
) -> u64 {
    use vuma_parser::ast::Type;
    match ty {
        Type::BDBase(name) => match name.as_str() {
            "i8" | "u8" | "bool" => 1,
            "i16" | "u16" => 2,
            "i32" | "u32" | "f32" => 4,
            "i64" | "u64" | "f64" => 8,
            _ => {
                // User-defined layout name — look up real size.
                if let Some(&size) = layout_sizes.get(name) {
                    size
                } else {
                    8  // fallback (forward reference not yet resolved)
                }
            }
        },
        Type::Ptr(_) | Type::RegionPtr { .. } => 8,
        // Wave 1c: `Channel<T>` is pointer-sized (8 on 64-bit, 4 on 32-bit).
        // Wave 89-90: session_type field doesn't affect size.
        Type::Channel { .. } => 8,
        Type::Array { element, size } => {
            bridge_type_size_with_layouts(element, layout_sizes) * (*size as u64)
        }
        _ => 8,
    }
}

/// PMT (Wave 2): compute the byte alignment of a parser `Type` for layout
/// field offset computation.
fn bridge_type_align(ty: &vuma_parser::ast::Type) -> u64 {
    use vuma_parser::ast::Type;
    match ty {
        Type::BDBase(name) => match name.as_str() {
            "i8" | "u8" | "bool" => 1,
            "i16" | "u16" => 2,
            "i32" | "u32" | "f32" => 4,
            "i64" | "u64" | "f64" => 8,
            _ => 8,
        },
        Type::Ptr(_) | Type::RegionPtr { .. } => 8,
        // Wave 1c: `Channel<T>` alignment is pointer-sized (8 on 64-bit, 4 on
        // 32-bit) — same as Ptr.
        // Wave 89-90: session_type field doesn't affect alignment.
        Type::Channel { .. } => 8,
        Type::Array { element, .. } => bridge_type_align(element),
        _ => 8,
    }
}

/// PMT (Wave 2): if `ty` is `State<LayoutName>`, return `Some(layout_name)`.
fn extract_state_layout_name_from_ast(ty: &vuma_parser::ast::Type) -> Option<String> {
    if let vuma_parser::ast::Type::State(inner) = ty {
        if let vuma_parser::ast::Type::BDBase(name) = inner.as_ref() {
            return Some(name.clone());
        }
    }
    None
}

/// PMT (Wave 2): build the layout registry from all `Item::LayoutDef` items
/// in the program. Returns a map: layout_name → (total_size, fields) where
/// each field is (name, ir_type, byte_offset, byte_size, type_name). Field
/// offsets are computed sequentially with alignment padding (mirroring
/// vuma-bd's `LayoutRegistry::register`).
fn build_layout_registry(program: &AstProgram) -> HashMap<String, (u64, Vec<(String, vuma_codegen::ir::IRType, u64, u64, String)>)> {
    // Multi-pass: first compute all layout sizes (resolving nested layout
    // references), then compute field offsets using the real sizes.
    // This fixes the nested-layout bug where user-defined layout names
    // (e.g. WaitQueue inside Pipe) were treated as size 8.

    // Pass 1: collect all layout definitions.
    let mut layout_defs: Vec<(&str, &Vec<(String, vuma_parser::ast::Type)>)> = Vec::new();
    for item in &program.items {
        if let Item::LayoutDef(ld) = item {
            layout_defs.push((ld.name.as_str(), &ld.fields));
        }
    }

    // Pass 2: iteratively compute layout sizes. Repeat until no changes
    // (handles forward references and nested layouts).
    let mut layout_sizes: HashMap<String, u64> = HashMap::new();
    let mut changed = true;
    let mut iterations = 0;
    while changed && iterations < 10 {
        changed = false;
        iterations += 1;
        for (name, fields) in &layout_defs {
            let mut size: u64 = 0;
            let mut max_align: u64 = 1;
            for (_fname, ftype) in *fields {
                let falign = bridge_type_align(ftype).max(1);
                let fsize = bridge_type_size_with_layouts(ftype, &layout_sizes);
                if falign > 1 && size % falign != 0 {
                    size = (size + falign - 1) & !(falign - 1);
                }
                max_align = max_align.max(falign);
                size += fsize;
            }
            let alignment = max_align.max(1);
            if size > 0 && size % alignment != 0 {
                size = (size + alignment - 1) & !(alignment - 1);
            }
            let prev = layout_sizes.get(*name).copied();
            if prev != Some(size) {
                layout_sizes.insert((*name).to_string(), size);
                changed = true;
            }
        }
    }

    // Pass 3: compute field offsets using the resolved layout sizes.
    let mut layouts = HashMap::new();
    for (name, fields) in &layout_defs {
        let mut offset: u64 = 0;
        let mut max_align: u64 = 1;
        let mut field_list: Vec<(String, vuma_codegen::ir::IRType, u64, u64, String)> = Vec::new();
        for (fname, ftype) in *fields {
            let falign = bridge_type_align(ftype).max(1);
            let fsize = bridge_type_size_with_layouts(ftype, &layout_sizes);
            if falign > 1 && offset % falign != 0 {
                offset = (offset + falign - 1) & !(falign - 1);
            }
            max_align = max_align.max(falign);
            let ir_ty = bridge_type_to_ir_type(ftype);
            let type_name = match ftype {
                vuma_parser::ast::Type::BDBase(n) => n.clone(),
                other => other.to_string(),
            };
            field_list.push((fname.clone(), ir_ty, offset, fsize, type_name));
            offset += fsize;
        }
        let alignment = max_align.max(1);
        if offset > 0 && offset % alignment != 0 {
            offset = (offset + alignment - 1) & !(alignment - 1);
        }
        layouts.insert((*name).to_string(), (offset, field_list));
    }
    layouts
}

/// (Wave 7) Build a `PmtLayoutSpec` registry from the AST's `Item::LayoutDef`
/// items, suitable for attaching to a `VerificationInput` via
/// `with_pmt_layouts()` so the IVE's `VerificationLevel::Pmt` can run the 3
/// state verifiers (state_read / state_write / state_transform) without the
/// 5 pointer invariants.
///
/// This is the IVE-public counterpart of the internal `build_layout_registry`:
/// it walks the same `Item::LayoutDef` AST nodes and computes the same
/// per-field offset/size using the same `bridge_type_*` helpers, but emits
/// the IVE's unified `PmtLayoutSpec` shape (`{name, total_size, fields}`).
/// The conversion is mechanical — `build_layout_registry` is the
/// codegen-side representation (`(total_size, Vec<(name, IRType, off, size,
/// type_name)>)`); `build_pmt_layout_specs` is the verification-side
/// representation.
pub fn build_pmt_layout_specs(program: &AstProgram) -> HashMap<String, vuma_ive::PmtLayoutSpec> {
    let mut layouts: HashMap<String, vuma_ive::PmtLayoutSpec> = HashMap::new();
    for item in &program.items {
        if let Item::LayoutDef(ld) = item {
            let mut offset: u64 = 0;
            let mut max_align: u64 = 1;
            let mut fields: Vec<vuma_ive::PmtFieldSpec> = Vec::new();
            for (fname, ftype) in &ld.fields {
                let falign = bridge_type_align(ftype).max(1);
                let fsize = bridge_type_size(ftype);
                if falign > 1 && offset % falign != 0 {
                    offset = (offset + falign - 1) & !(falign - 1);
                }
                max_align = max_align.max(falign);
                let type_name = match ftype {
                    vuma_parser::ast::Type::BDBase(n) => n.clone(),
                    other => other.to_string(),
                };
                fields.push(vuma_ive::PmtFieldSpec {
                    name: fname.clone(),
                    offset,
                    size: fsize,
                    type_name,
                });
                offset += fsize;
            }
            let alignment = max_align.max(1);
            if offset > 0 && offset % alignment != 0 {
                offset = (offset + alignment - 1) & !(alignment - 1);
            }
            layouts.insert(
                ld.name.clone(),
                vuma_ive::PmtLayoutSpec {
                    name: ld.name.clone(),
                    total_size: offset,
                    fields,
                },
            );
        }
    }
    layouts
}

/// PMT (Wave 2): resolve a state-field chain `(layout_name, [field1, field2, ...])`
/// against the layout registry. Returns `(cumulative_offset, size, ir_type)`
/// of the leaf field, descending into nested layout-typed fields. Returns
/// `None` if any field in the chain isn't found.

/// Detect the element type and size of an array field being indexed.
/// Returns `(elem_size, Some(ir_type))` for known types, `(1, None)` for u8/unknown.
/// This enables typed-array index scaling: `arr[i]` → `*(base + i * elem_size)`
/// instead of the byte-granular default `*(base + i)`.
fn detect_array_elem_type(
    expr: &vuma_parser::ast::Expr,
    ctx: &BridgeCtx,
) -> (u64, Option<vuma_codegen::ir::IRType>) {
    use vuma_parser::ast::Expr;

    // Only FieldAccess chains can resolve to typed arrays.
    if !matches!(expr, Expr::FieldAccess { .. }) {
        return (1, None);
    }

    // Walk the chain to find the field's type_name.
    let mut chain: Vec<String> = Vec::new();
    let mut cur = expr;
    while let Expr::FieldAccess { expr: inner, field, .. } = cur {
        chain.push(field.clone());
        cur = inner.as_ref();
    }
    if let Expr::Var { name: bv, .. } = cur {
        if let Some(layout_name) = ctx.state_var_layouts.get(bv).cloned() {
            chain.reverse();
            if let Some((_offset, _size, _field_ty, type_name)) =
                resolve_state_field_chain(&ctx.layouts, &layout_name, &chain)
            {
                // Parse element type from "[T; N]" format.
                if type_name.starts_with('[') {
                    let inner = &type_name[1..];
                    if let Some(semi_pos) = inner.find(';') {
                        let elem_type_str = inner[..semi_pos].trim();
                        return match elem_type_str {
                            "u8" | "i8" | "bool" => (1, Some(vuma_codegen::ir::IRType::U8)),
                            "u16" | "i16" => (2, Some(vuma_codegen::ir::IRType::U16)),
                            "u32" | "i32" => (4, Some(vuma_codegen::ir::IRType::U32)),
                            "f32" => (4, Some(vuma_codegen::ir::IRType::F32)),
                            "u64" | "i64" => (8, Some(vuma_codegen::ir::IRType::U64)),
                            "f64" => (8, Some(vuma_codegen::ir::IRType::F64)),
                            _ => (1, None),
                        };
                    }
                }
            }
        }
    }
    (1, None)
}

fn resolve_state_field_chain(
    layouts: &HashMap<String, (u64, Vec<(String, vuma_codegen::ir::IRType, u64, u64, String)>)>,
    start_layout: &str,
    chain: &[String],
) -> Option<(u64, u64, vuma_codegen::ir::IRType, String)> {
    if chain.is_empty() {
        return None;
    }
    let mut layout = start_layout.to_string();
    let mut cum_offset: u64 = 0;
    let mut last_size: u64 = 0;
    let mut last_ty: Option<vuma_codegen::ir::IRType> = None;
    let mut last_type_name: String = String::new();
    for field in chain {
        let (_, fields) = layouts.get(&layout)?;
        let (_fname, ftype, foffset, fsize, ftype_name) = fields
            .iter()
            .find(|(n, _, _, _, _)| n == field)?;
        cum_offset += foffset;
        last_size = *fsize;
        last_ty = Some(ftype.clone());
        last_type_name = ftype_name.clone();
        // Descend into nested layout-typed fields: if the field's declared
        // type name matches a known layout, switch to that layout for the
        // next field in the chain.
        if layouts.contains_key(ftype_name) {
            layout = ftype_name.clone();
        }
    }
    last_ty.map(|ty| (cum_offset, last_size, ty, last_type_name))
}

/// Convert a parser block into codegen SCG statements, flattening expressions
/// into three-address code with temporaries.
pub fn bridge_block_to_scg_stmts(block: &vuma_parser::ast::Block, ctx: &mut BridgeCtx) -> Vec<ScgStatement> {
    block
        .statements
        .iter()
        .flat_map(|s| bridge_stmt_to_scg(s, ctx))
        .collect()
}

/// Flatten an expression into three-address code. Returns the ScgExpr that
/// holds the result, and appends any intermediate computation statements
/// to `stmts`.
///
/// Wave 5: emit ForeignConsume marker statements for a `#[foreign_consume]`
/// extern call. For each argument that is a State variable whose layout has
/// `#[foreign(raw)]`, push a `ScgStatement::ForeignConsume` so the IVE treats
/// the State's vreg as consumed (linearity error on subsequent read/write).
fn emit_foreign_consume_markers(
    func_name: &str,
    args: &[vuma_parser::ast::Expr],
    stmts: &mut Vec<ScgStatement>,
    ctx: &BridgeCtx,
) {
    // Look up the extern fn's declaration to check for #[foreign_consume].
    let fn_decl = match ctx.extern_fn_decls.get(func_name) {
        Some(d) => d,
        None => return, // not an extern fn — nothing to do
    };
    let fn_attrs = attrs_to_attr_infos(&fn_decl.attrs);
    if vuma_codegen::marshal::foreign_consume_field(&fn_attrs).is_none() {
        return; // not a #[foreign_consume] fn
    }
    // For each arg that is a State variable (Expr::Var) whose layout is
    // #[foreign(raw)], emit a ForeignConsume marker.
    for arg in args {
        if let vuma_parser::ast::Expr::Var { name, .. } = arg {
            // Check if this variable is a state-typed variable.
            if let Some(layout_name) = ctx.state_var_layouts.get(name) {
                // Check if the layout has #[foreign(raw)].
                if let Some(layout_decl) = ctx.layout_decls.get(layout_name) {
                    let layout_attrs = attrs_to_attr_infos(&layout_decl.attrs);
                    if vuma_codegen::marshal::foreign_layout_field(&layout_attrs).is_some() {
                        stmts.push(ScgStatement::ForeignConsume(
                            vuma_codegen::scg_to_ir::ForeignConsumeStmt {
                                state_var: name.clone(),
                                layout_name: layout_name.clone(),
                            },
                        ));
                    }
                }
            }
        }
    }
}

/// Wave 8b: result of recognising `match channel_recv(ch) { Ok(v) => ..., Err(e) => ... }`.
///
/// Produced by [`try_match_channel_recv_result`].  The pipeline emits a
/// [`ScgStatement::ChannelRecvResult`] binding `ok_binding` (the Ok arm's
/// `v`) to the payload vreg and `err_binding` (the Err arm's `e`) to the
/// ChannelError-discriminant vreg, followed by a `ControlNode::If` on
/// `err_binding == 0` dispatching to the two flattened arm bodies.
struct ChannelRecvMatch {
    /// The flattened channel-handle expression (operand of `channel_recv(ch)`).
    channel_expr: ScgExpr,
    /// The Ok arm's binding name (e.g. `v`) — receives the payload on success.
    ok_binding: String,
    /// The Err arm's binding name (e.g. `e`) — receives the ChannelError discriminant on failure.
    err_binding: String,
    /// Flattened Ok-arm body statements.
    ok_body: Vec<ScgStatement>,
    /// Flattened Err-arm body statements.
    err_body: Vec<ScgStatement>,
}

/// Wave 8b: detect the `match channel_recv(ch) { Ok(v) => ..., Err(e) => ... }`
/// pattern and lower it to a [`ChannelRecvMatch`] ready for emission as
/// `ScgStatement::ChannelRecvResult` + `ControlNode::If`.
///
/// Returns `None` if `match_stmt` is not exactly this form (so the generic
/// Switch / complex-pattern fallback path handles it).
///
/// Recognition rules (all must hold):
///   - `match_stmt.subject` is `Expr::Call { callee: Var("channel_recv"), args: [ch] }`
///   - `match_stmt.arms.len() == 2`
///   - one arm's pattern is `MatchPattern::Enum { name: "Ok", binding: Some(v) }`
///   - the other arm's pattern is `MatchPattern::Enum { name: "Err", binding: Some(e) }`
///
/// The channel-handle expression `ch` is flattened into `pre_stmts` (so any
/// nested sub-expressions are evaluated before the recv).  The two arm bodies
/// are each flattened into their own `Vec<ScgStatement>`.
fn try_match_channel_recv_result(
    match_stmt: &vuma_parser::ast::MatchStmt,
    ctx: &mut BridgeCtx,
    pre_stmts: &mut Vec<ScgStatement>,
) -> Option<ChannelRecvMatch> {
    use vuma_parser::ast::{Expr, MatchPattern};

    // Subject must be `channel_recv(ch)`.
    let ch_expr = match &match_stmt.subject {
        Expr::Call { callee, args, .. } => {
            let is_channel_recv = matches!(callee.as_ref(),
                Expr::Var { name, .. } if name == "channel_recv");
            if !is_channel_recv || args.len() != 1 {
                return None;
            }
            &args[0]
        }
        _ => return None,
    };

    // Exactly two arms: one Ok(v), one Err(e).
    if match_stmt.arms.len() != 2 {
        return None;
    }
    let mut ok_arm: Option<&vuma_parser::ast::MatchArm> = None;
    let mut err_arm: Option<&vuma_parser::ast::MatchArm> = None;
    for arm in &match_stmt.arms {
        match &arm.pattern {
            MatchPattern::Enum { name, binding: Some(_), .. } if name == "Ok" => {
                if ok_arm.is_some() { return None; }
                ok_arm = Some(arm);
            }
            MatchPattern::Enum { name, binding: Some(_), .. } if name == "Err" => {
                if err_arm.is_some() { return None; }
                err_arm = Some(arm);
            }
            _ => return None, // any other pattern → not this form
        }
    }
    let ok_arm = ok_arm?;
    let err_arm = err_arm?;

    let ok_binding = match &ok_arm.pattern {
        MatchPattern::Enum { binding: Some(b), .. } => b.clone(),
        _ => unreachable!(),
    };
    let err_binding = match &err_arm.pattern {
        MatchPattern::Enum { binding: Some(b), .. } => b.clone(),
        _ => unreachable!(),
    };

    // Flatten the channel-handle expression (may emit pre-statements).
    let channel_expr = flatten_expr(ch_expr, pre_stmts, ctx);

    // Flatten each arm body into its own statement list.
    let mut ok_body: Vec<ScgStatement> = Vec::new();
    let _ = flatten_expr(&ok_arm.body, &mut ok_body, ctx);
    let mut err_body: Vec<ScgStatement> = Vec::new();
    let _ = flatten_expr(&err_arm.body, &mut err_body, ctx);

    Some(ChannelRecvMatch {
        channel_expr,
        ok_binding,
        err_binding,
        ok_body,
        err_body,
    })
}

/// This is the core of the bridge: it recursively decomposes nested
/// expressions into a sequence of simple computation nodes, each operating
/// on at most two operands and producing one result. This preserves the
/// full semantics of the original expression tree.
pub fn flatten_expr(
    expr: &vuma_parser::ast::Expr,
    stmts: &mut Vec<ScgStatement>,
    ctx: &mut BridgeCtx,
) -> ScgExpr {
    use vuma_parser::ast::{Expr, Lit, UnOp};

    match expr {
        // ── Leaf expressions: return directly, no flattening needed ──
        Expr::Var { name, .. } => {
            // Check if this variable is a known global constant.
            // If so, inline its literal value instead of emitting a variable
            // reference that would be unresolved during IR lowering.
            if let Some(&val) = ctx.global_constants.get(name) {
                ScgExpr::Int(val)
            } else if ctx.function_names.contains(name) {
                // Wave 5: The name refers to a function (not a variable).
                // Produce a Label so the backend emits a RIP-relative LEA
                // with a relocation to the function's address.
                ScgExpr::Label(name.clone())
            } else {
                ScgExpr::Var(name.clone())
            }
        }
        Expr::Lit { value, .. } => match value {
            Lit::Int(n) => ScgExpr::Int(*n),
            Lit::Float(f) => ScgExpr::Float(*f),
            Lit::Bool(b) => ScgExpr::Int(if *b { 1 } else { 0 }),
            Lit::Address(a) => ScgExpr::Int(*a as i64),
            Lit::String(s) => {
                // Wave 1: Lower string literals to .rodata addresses.
                //
                // The ELF layout for hosted Linux ET_EXEC is deterministic:
                //   base_addr = 0x400000
                //   headers_total = 64 (ehdr) + 56*3 (3 phdrs) = 232
                //   rodata_vaddr = base_addr + headers_total = 0x4000E8
                //
                // The string table is emitted as a single ScgNode::Data
                // (ReadOnly, align=1) containing all unique string literals
                // NUL-terminated and concatenated. Each string's absolute
                // address is rodata_vaddr + offset_within_table.
                //
                // This avoids IRValue::Label (which isn't fully wired in all
                // backends) by computing the address as a compile-time i64.
                const BASE_ADDR_LINUX: i64 = 0x400000;
                const EHDR_SIZE: i64 = 64;
                const PHDR_SIZE: i64 = 56;
                // 4 program headers: rodata + text + bss + gnu_stack
                // (bss is always present because of runtime argv storage)
                const NUM_PHDRS: i64 = 4;
                const RODATA_VADDR: i64 = BASE_ADDR_LINUX + EHDR_SIZE + PHDR_SIZE * NUM_PHDRS;

                let bytes = s.as_bytes();
                // Deduplicate: check if this exact string is already in the table.
                let mut table = ctx.string_table.borrow_mut();
                let mut offset: i64 = 0;
                let mut found = false;
                for (_existing_label, existing_bytes) in table.iter() {
                    if existing_bytes == bytes {
                        // Found a duplicate — reuse the existing label.
                        // The offset is the sum of all preceding strings' lengths (including NUL).
                        found = true;
                        break;
                    }
                    offset += existing_bytes.len() as i64 + 1; // +1 for NUL
                }
                if !found {
                    let label = format!("__vuma_str_{}", table.len());
                    offset = table.iter().map(|(_, b)| b.len() as i64 + 1).sum();
                    table.push((label, bytes.to_vec()));
                }
                drop(table);

                let addr = RODATA_VADDR + offset;
                ScgExpr::Int(addr)
            }
        },

        // ── Binary operations: flatten lhs and rhs, then emit one Computation ──
        Expr::BinOp { op, lhs, rhs, .. } => {
            let lhs_expr = flatten_expr(lhs, stmts, ctx);
            let rhs_expr = flatten_expr(rhs, stmts, ctx);
            let dst = ctx.alloc_temp();
            // Type-aware right shift: arithmetic (ShrA) for SIGNED integer
            // types (i8/i16/i32/i64), logical (ShrL) for UNSIGNED types
            // (u8/u16/u32/u64) and pointers. The shift kind depends on the
            // OPERAND TYPE, not a global default — see Task 7-A.
            //
            //   `bit_abs.vuma`     uses `i64` → ShrA  (correct: -42 >> 63 == -1)
            //   `bit_log2.vuma`    uses `u64` → ShrL  (correct: 0xFF..FF00 >> 63 == 1)
            //   `mmap_sha256d.vuma` uses `u32` → ShrL (correct: small_sigma0/1)
            //
            // When the lhs is not a Var (complex expression) OR the variable
            // has no recorded type (untyped let-binding, untyped parameter),
            // default to ShrL — safer for the bit-twiddling idioms in the
            // gold-standard tests, and `bit_abs` (i64) explicitly declares
            // its type so it still gets ShrA via the type-aware path.
            let binop_kind = match op {
                vuma_parser::ast::BinOp::Shr => {
                    let is_signed = match lhs.as_ref() {
                        Expr::Var { name, .. } => ctx
                            .var_types
                            .get(name)
                            .map(|ty| {
                                matches!(
                                    ty,
                                    ScgType::I8 | ScgType::I16 | ScgType::I32 | ScgType::I64
                                )
                            })
                            .unwrap_or(false),
                        _ => false,
                    };
                    if is_signed {
                        BinOpKind::ShrA
                    } else {
                        BinOpKind::ShrL
                    }
                }
                _ => map_ast_binop(op),
            };
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: dst.clone(),
                op: binop_kind,
                lhs: lhs_expr,
                rhs: rhs_expr,
                tail_call: false,
                reassigns: None,
            }));
            ScgExpr::Var(dst)
        }

        // ── Unary operations: flatten operand, then emit one Computation ──
        Expr::UnOp { op, expr: operand, .. } => {
            let operand_expr = flatten_expr(operand, stmts, ctx);
            let dst = ctx.alloc_temp();
            match op {
                UnOp::BitNot => {
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: dst.clone(),
                        op: BinOpKind::Xor,
                        lhs: operand_expr,
                        rhs: ScgExpr::Int(-1),
                        tail_call: false,
                        reassigns: None,
                    }));
                }
                UnOp::Neg => {
                    // G7: for float operands, multiply by -1.0 (correct f64
                    // negation).  Integer Sub(0, bits) would produce wrong
                    // results (0 - 0x4007... = 0xBFF8... ≠ -2.9).
                    let is_float = match &operand_expr {
                        ScgExpr::Float(_) => true,
                        ScgExpr::Var(name) => matches!(
                            ctx.var_types.get(name),
                            Some(ScgType::F32) | Some(ScgType::F64)
                        ),
                        _ => false,
                    };
                    if is_float {
                        stmts.push(ScgStatement::Computation(ComputationNode {
                            dst: dst.clone(),
                            op: BinOpKind::Mul,
                            lhs: ScgExpr::Float(-1.0),
                            rhs: operand_expr,
                            tail_call: false,
                            reassigns: None,
                        }));
                    } else {
                        stmts.push(ScgStatement::Computation(ComputationNode {
                            dst: dst.clone(),
                            op: BinOpKind::Sub,
                            lhs: ScgExpr::Int(0),
                            rhs: operand_expr,
                            tail_call: false,
                            reassigns: None,
                        }));
                    }
                }
                UnOp::Not => {
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: dst.clone(),
                        op: BinOpKind::Eq,
                        lhs: operand_expr,
                        rhs: ScgExpr::Int(0),
                        tail_call: false,
                        reassigns: None,
                    }));
                }
                UnOp::Deref => {
                    stmts.push(ScgStatement::Access(AccessNode::Load {
                        dst: dst.clone(),
                        ptr: operand_expr,
                        offset: None,
                        ty: None,
                    }));
                }
            }
            ScgExpr::Var(dst)
        }

        // ── Function call: flatten args, emit CallNode ──
        Expr::Call { callee, args, .. } => {
            let func_name = match callee.as_ref() {
                Expr::Var { name, .. } => name.clone(),
                _ => "_unknown".into(),
            };
            let flat_args: Vec<ScgExpr> = args.iter()
                .map(|a| flatten_expr(a, stmts, ctx))
                .collect();
            // Mark as extern if the function was declared in an extern "C" block
            // OR if it's a known built-in intrinsic (AtomicLoad/AtomicStore/AtomicCas
            // are lowered by the backend to machine instructions, not external calls,
            // but allocate/free are truly external).
            let is_extern = ctx.extern_fns.contains(&func_name)
                || func_name == "__vuma_alloc"
                || func_name == "__vuma_dealloc";
            // Void functions (no return type) get dst=None so the wasm32
            // backend doesn't load stale data from mem[0].
            let is_void = ctx.void_functions.contains(&func_name)
                || func_name == "free"
                || func_name == "print_int"
                || func_name == "print_hex"
                || func_name == "print_newline";
            if is_void {
                stmts.push(ScgStatement::Call(CallNode {
                    dst: None,
                    func: func_name.clone(),
                    args: flat_args.clone(),
                    is_extern,
                    reassigns: None,
                }));
                // Wave 5: if this is a #[foreign_consume] extern call, emit
                // a ForeignConsume marker for each State arg whose layout is
                // #[foreign(raw)]. The IVE treats the State's vreg as consumed.
                emit_foreign_consume_markers(&func_name, args, stmts, ctx);
                // Return a dummy value (0) for void calls used as expressions
                ScgExpr::Int(0)
            } else {
                let dst = ctx.alloc_temp();
                stmts.push(ScgStatement::Call(CallNode {
                    dst: Some(dst.clone()),
                    func: func_name.clone(),
                    args: flat_args.clone(),
                    is_extern,
                    reassigns: None,
                }));
                // Wave 5: emit ForeignConsume markers for #[foreign_consume] calls.
                emit_foreign_consume_markers(&func_name, args, stmts, ctx);
                ScgExpr::Var(dst)
            }
        }

        // ── Direct syscall: flatten args, emit SyscallCallNode ──
        //
        // Wave 10: `syscall(nr, args...)` is a first-class AST expression
        // that lowers to `ScgStatement::Syscall`. The IRBuilder then emits
        // `IRInstr::Syscall`, which each backend lowers directly to a real
        // syscall instruction (Wave 11/12 removed the intermediate
        // `lower_syscalls_all()` lowering pass) so backends can resolve it
        // via their existing `syscall_stubs` tables.
        //
        // Void syscalls (exit, exit_group) get `dst: None` so the IR doesn't
        // contain a dead vreg that would never be assigned. For all other
        // syscalls, we allocate a fresh temp and return it as a `Var` so the
        // result can flow into surrounding expressions.
        Expr::Syscall { nr, args, .. } => {
            let flat_args: Vec<ScgExpr> = args
                .iter()
                .map(|a| flatten_expr(a, stmts, ctx))
                .collect();
            let is_void_syscall = matches!(*nr, 60 | 231); // exit, exit_group
            if is_void_syscall {
                stmts.push(ScgStatement::Syscall(SyscallCallNode {
                    nr: *nr,
                    dst: None,
                    args: flat_args,
                }));
                ScgExpr::Int(0)
            } else {
                let dst = ctx.alloc_temp();
                stmts.push(ScgStatement::Syscall(SyscallCallNode {
                    nr: *nr,
                    dst: Some(dst.clone()),
                    args: flat_args,
                }));
                ScgExpr::Var(dst)
            }
        }

        // ── Atomic operations: emit as CallNodes with special names ──
        // The backend's instruction selector recognizes these names and lowers
        // them to proper atomic machine instructions (LDAXR/STLXR on AArch64,
        // LOCK CMPXCHG on x86_64, LR.D/SC.D on RISC-V, etc.).
        Expr::AtomicLoad { addr, .. } => {
            let addr_expr = flatten_expr(addr, stmts, ctx);
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Call(CallNode {
                dst: Some(dst.clone()),
                func: "AtomicLoad".to_string(),
                args: vec![addr_expr],
                is_extern: true,
                reassigns: None,
            }));
            ScgExpr::Var(dst)
        }
        Expr::AtomicStore { value, addr, .. } => {
            let value_expr = flatten_expr(value, stmts, ctx);
            let addr_expr = flatten_expr(addr, stmts, ctx);
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Call(CallNode {
                dst: Some(dst.clone()),
                func: "AtomicStore".to_string(),
                args: vec![value_expr, addr_expr],
                is_extern: true,
                reassigns: None,
            }));
            ScgExpr::Var(dst)
        }
        Expr::AtomicCas { addr, expected, desired, .. } => {
            let addr_expr = flatten_expr(addr, stmts, ctx);
            let expected_expr = flatten_expr(expected, stmts, ctx);
            let desired_expr = flatten_expr(desired, stmts, ctx);
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Call(CallNode {
                dst: Some(dst.clone()),
                func: "AtomicCas".to_string(),
                args: vec![addr_expr, expected_expr, desired_expr],
                is_extern: true,
                reassigns: None,
            }));
            ScgExpr::Var(dst)
        }

        // ── Dereference: flatten the address, emit Load ──
        //
        // For chained derefs (**p, ***p, ...), the INNER loads must read a
        // full pointer-width value (U64 on 64-bit backends, falls back to
        // native register width on 32-bit backends) so a 64-bit address is
        // not truncated to its low byte. The OUTERMOST load reads the final
        // value (default U8 — e.g. a byte stored via `*p = 42`).
        //
        // Without this, `**buf1` (where buf1 holds buf2's address) reads
        // only 1 byte of buf2's address, then dereferences the truncated
        // value → SIGSEGV on all native backends. See ptr_double_deref,
        // ptr_multi_level, ptr_pointer_to_pointer gold-standard tests.
        Expr::Deref { expr, .. } => {
            // Walk the inner Deref chain to find the innermost non-Deref
            // expression. `chain_depth` counts the total number of Derefs
            // (1 for the current one + N for any nested Derefs in `expr`).
            let mut chain_depth: usize = 1;
            let mut cur = expr.as_ref();
            while let Expr::Deref { expr: inner, .. } = cur {
                chain_depth += 1;
                cur = inner.as_ref();
            }
            // `cur` is now the innermost non-Deref expression (typically a
            // Var holding a pointer, or an Offset/Index computing an address).
            // Check the AST expression for stride pattern BEFORE flattening.
            let outer_load_ty = infer_load_type_from_ast_expr(cur);
            let mut current_addr = flatten_expr(cur, stmts, ctx);
            for i in (0..chain_depth).rev() {
                let dst = ctx.alloc_temp();
                let ty = if i == 0 {
                    outer_load_ty.clone()
                } else {
                    Some(vuma_codegen::ir::IRType::U64)
                };
                stmts.push(ScgStatement::Access(AccessNode::Load {
                    dst: dst.clone(),
                    ptr: current_addr,
                    offset: None,
                    ty,
                }));
                current_addr = ScgExpr::Var(dst);
            }
            current_addr
        }

        // ── Offset (pointer arithmetic): flatten base and offset, emit Add ──
        Expr::Offset { base, offset, .. } => {
            let base_expr = flatten_expr(base, stmts, ctx);
            let off_expr = flatten_expr(offset, stmts, ctx);
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: dst.clone(),
                op: BinOpKind::Add,
                lhs: base_expr,
                rhs: off_expr,
                tail_call: false,
                reassigns: None,
            }));
            ScgExpr::Var(dst)
        }

        // ── Cast: flatten operand, pass through ──
        Expr::Cast { expr, .. } => flatten_expr(expr, stmts, ctx),

        // ── TypeAscription: flatten inner expression ──
        Expr::TypeAscription { expr, .. } => flatten_expr(expr, stmts, ctx),

        // ── Index: flatten base and index, compute addr, emit Load ──
        Expr::Index { expr, index, .. } => {
            let base_expr = flatten_expr(expr, stmts, ctx);
            let idx_expr = flatten_expr(index, stmts, ctx);

            // Determine the array element type and size for proper index scaling.
            let (elem_size, elem_ir_type) = detect_array_elem_type(expr, ctx);

            // Scale the index by element size if needed.
            let scaled_idx = if elem_size > 1 {
                let mul_dst = ctx.alloc_temp();
                stmts.push(ScgStatement::Computation(ComputationNode {
                    dst: mul_dst.clone(),
                    op: BinOpKind::Mul,
                    lhs: idx_expr,
                    rhs: ScgExpr::Int(elem_size as i64),
                    tail_call: false,
                    reassigns: None,
                }));
                ScgExpr::Var(mul_dst)
            } else {
                idx_expr
            };

            let addr = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: addr.clone(),
                op: BinOpKind::Add,
                lhs: base_expr,
                rhs: scaled_idx,
                tail_call: false,
                reassigns: None,
            }));
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Access(AccessNode::Load {
                dst: dst.clone(),
                ptr: ScgExpr::Var(addr),
                offset: None,
                ty: elem_ir_type,
            }));
            ScgExpr::Var(dst)
        }

        // ── Range: just flatten start (range is handled by For) ──
        Expr::Range { start, .. } => flatten_expr(start, stmts, ctx),

        // ── Allocate: emit as a heap allocation via direct mmap syscall ──
        // P2: `allocate(size)` in expression context now lowers directly to
        // a SyscallCallNode (mmap, nr 222) instead of a Call to the
        // Rust-emitted `__vuma_alloc` runtime stub. Eliminates the last
        // Rust-wrapper dependency in the heap-allocation path.
        Expr::Allocate { size, .. } => {
            let size_expr = flatten_expr(size, stmts, ctx);
            let dst = ctx.alloc_temp();
            stmts.push(ScgStatement::Syscall(SyscallCallNode {
                nr: 222,
                dst: Some(dst.clone()),
                args: vec![
                    ScgExpr::Int(0),      // addr = NULL
                    size_expr,            // length = size
                    ScgExpr::Int(3),      // prot = PROT_READ|PROT_WRITE
                    ScgExpr::Int(0x22),   // flags = MAP_PRIVATE|MAP_ANONYMOUS
                    ScgExpr::Int(-1),     // fd = -1 (MAP_ANONYMOUS)
                    ScgExpr::Int(0),      // offset = 0
                ],
            }));
            ScgExpr::Var(dst)
        }

        // ── Null → 0 ──
        Expr::Null { .. } => ScgExpr::Int(0),

        // ── Uninitialized → 0 ──
        Expr::Uninitialized { .. } => ScgExpr::Int(0),

        // ── Address-of: emit GetAddress for symbol references ──
        Expr::AddressOf { expr, .. } => {
            // If the inner expression is a variable (function name, data symbol),
            // emit a GetAddress node that will lower to `IRInstr::GetAddress`
            // with a proper relocation.  Otherwise, just flatten the inner expr
            // (e.g. for @(*ptr) or other complex address-of patterns).
            match expr.as_ref() {
                Expr::Var { name, .. } => {
                    let dst = ctx.alloc_temp();
                    stmts.push(ScgStatement::GetAddress(GetAddressNode {
                        dst: dst.clone(),
                        name: name.clone(),
                    }));
                    ScgExpr::Var(dst)
                }
                _ => flatten_expr(expr, stmts, ctx),
            }
        }

        // ── PMT (Wave 2): state.field read (and nested state.a.b read) ──
        //
        // `Expr::FieldAccess` chains rooted at a state-typed var (e.g.
        // `p.x`, `l.a.x`) lower to a single Load at the field's cumulative
        // byte offset within the state's buffer. Non-state FieldAccess
        // (e.g. on a struct value) is not supported here — it falls through
        // to the unsupported-expression warning.
        Expr::FieldAccess { .. } => {
            // Walk the chain to find (base_var, [field1, ..., fieldN]).
            let mut chain: Vec<String> = Vec::new();
            let mut cur = expr;
            while let Expr::FieldAccess { expr: inner, field, .. } = cur {
                chain.push(field.clone());
                cur = inner.as_ref();
            }
            let base_var = if let Expr::Var { name, .. } = cur {
                Some(name.clone())
            } else {
                None
            };
            if let Some(bv) = base_var {
                if let Some(layout_name) = ctx.state_var_layouts.get(&bv).cloned() {
                    chain.reverse(); // outermost-to-innermost order
                    if let Some((offset, _size, field_ty, type_name)) =
                        resolve_state_field_chain(&ctx.layouts, &layout_name, &chain)
                    {
                        // Wave 2 fix (array-typed fields): when the field's
                        // declared type is an inline array (e.g. `data: [u8; 4]`),
                        // `state.field` should produce the ADDRESS of the
                        // inline array — NOT a Load of its (non-existent)
                        // pointer. The downstream `Index` access then adds
                        // the element index to this address and Loads from
                        // the result. Without this, the FieldAccess would
                        // emit a U64 Load reading 8 bytes from a 4-byte
                        // allocation (buffer overread), and the Index would
                        // dereference the resulting garbage as a pointer
                        // (SIGSEGV on x86_64). Array types are detected via
                        // the field's stored `type_name` (Display form, e.g.
                        // `[u8; 4]`).
                        if type_name.starts_with('[') {
                            let addr = if offset == 0 {
                                ScgExpr::Var(bv.clone())
                            } else {
                                let addr_tmp = ctx.alloc_temp();
                                stmts.push(ScgStatement::Computation(ComputationNode {
                                    dst: addr_tmp.clone(),
                                    op: BinOpKind::Add,
                                    lhs: ScgExpr::Var(bv.clone()),
                                    rhs: ScgExpr::Int(offset as i64),
                                    tail_call: false,
                                    reassigns: None,
                                }));
                                ScgExpr::Var(addr_tmp)
                            };
                            return addr;
                        }
                        // Wave 2 fix (scalar fields): compute the field
                        // address explicitly via a separate
                        // `Add(base, offset)` Computation node so each field
                        // access lowers to a Load with a UNIQUE address vreg
                        // (and `offset: None`). The `dead_store_eliminate`
                        // pass in the IR optimizer treats two Stores with
                        // the SAME `addr` IRValue as aliasing (ignoring the
                        // `offset` field), so a naive
                        // `Load { ptr: Var(bv), offset: Some(Int(N)) }` would
                        // lower to `Load { addr: R_base, offset: N }` and
                        // two Stores to different fields of the same state
                        // buffer would alias by `addr`, causing the first
                        // Store to be eliminated (returning the LAST field
                        // written instead of the sum). Emitting an explicit
                        // `Add` produces a distinct address vreg per access;
                        // the TBAA `Unique(N)` alias class is inherited via
                        // the BinOp arm of `AliasAnalysis`, so the DSE then
                        // correctly falls through to `may_alias_combined`
                        // and stops (may-alias, conservatively keep both).
                        let ptr_expr = if offset == 0 {
                            ScgExpr::Var(bv.clone())
                        } else {
                            let addr_tmp = ctx.alloc_temp();
                            stmts.push(ScgStatement::Computation(ComputationNode {
                                dst: addr_tmp.clone(),
                                op: BinOpKind::Add,
                                lhs: ScgExpr::Var(bv.clone()),
                                rhs: ScgExpr::Int(offset as i64),
                                tail_call: false,
                                reassigns: None,
                            }));
                            ScgExpr::Var(addr_tmp)
                        };
                        let dst = ctx.alloc_temp();
                        stmts.push(ScgStatement::Access(AccessNode::Load {
                            dst: dst.clone(),
                            ptr: ptr_expr,
                            offset: None,
                            ty: Some(field_ty),
                        }));
                        return ScgExpr::Var(dst);
                    }
                }
            }
            // Not a state-typed FieldAccess — emit warning and return 0.
            eprintln!("[vuma] WARNING: unsupported FieldAccess (not state-typed) in flatten_expr; using 0");
            ScgExpr::Int(0)
        }

        // ── PMT (Wave 2): StateInit as an expression (rare — usually
        // handled directly in PStmt::Let). If encountered here, return 0
        // since we can't allocate without a binding name. ──
        Expr::StateInit { .. } => {
            eprintln!("[vuma] WARNING: state_new() outside let-binding in flatten_expr; using 0");
            ScgExpr::Int(0)
        }

        // ── Arena State Model (Wave 3a): arena builtins lower to
        // CallNodes to mmap/mremap/munmap (already registered as syscall
        // stubs on all 19 backends) + AccessNode Load/Store for the
        // Arena struct fields. ──
        //
        // arena_new(capacity) → State<Arena>:
        //   1. Call mmap(0, capacity, 3, 0x22, -1, 0) → base pointer
        //   2. Allocate Arena struct (24 bytes: base u64, offset u64, capacity u64)
        //   3. Store base, offset=0, capacity into the Arena struct
        //   4. Return pointer to the Arena struct
        Expr::ArenaNew { capacity, .. } => {
            // Single mmap CallNode: the Arena struct (24 bytes) is embedded at
            // the BEGINNING of the mmap'd region. Arena data starts at offset
            // 24. The mmap'd region is zero-initialized by the kernel, so
            // arena.base starts at 0.
            //
            // arena_alloc uses arena_ptr directly as the base (since the
            // Arena struct IS at the beginning of the mmap'd region).
            //
            // Layout: [base:u64=0 | offset:u64=24 | capacity:u64 | data...]
            //          ^--- arena_ptr points here (= data base)
            //
            // All Store/Load operations use StructAccessNode (the SAME path
            // as PMT state.field writes) — this is proven to work on ALL 19
            // backends. The AccessNode::Store path with ComputationNode::Add
            // crashes on strict-alignment ISAs (mips64, s390x, alpha, hppa).
            let cap_expr = flatten_expr(capacity, stmts, ctx);
            let arena_ptr = ctx.alloc_temp();
            stmts.push(ScgStatement::Call(CallNode {
                dst: Some(arena_ptr.clone()),
                func: "mmap".to_string(),
                args: vec![
                    ScgExpr::Int(0),       // addr = NULL
                    cap_expr.clone(),       // length = capacity
                    ScgExpr::Int(3),        // prot = PROT_READ|PROT_WRITE
                    ScgExpr::Int(0x22),     // flags = MAP_PRIVATE|MAP_ANONYMOUS
                    ScgExpr::Int(-1),       // fd = -1
                    ScgExpr::Int(0),        // offset = 0
                ],
                is_extern: true,
                reassigns: None,
            }));
            // arena.base (offset 0) is already 0 (mmap zeroes the region).
            // Store arena.offset = 24 at [arena_ptr+8] via ComputationNode::Add
            // + AccessNode::Store (the SAME path as PMT state.field writes —
            // proven to work on ALL 19 backends. StructAccessNode::Store
            // crashes on mips64 because the mips64 Store instruction selector
            // doesn't handle IR Store with non-zero offset correctly.)
            let off_addr = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: off_addr.clone(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var(arena_ptr.clone()),
                rhs: ScgExpr::Int(8),
                tail_call: false,
                reassigns: None,
            }));
            stmts.push(ScgStatement::Access(AccessNode::Store {
                ptr: ScgExpr::Var(off_addr),
                offset: None,
                value: ScgExpr::Int(24),
                ty: Some(vuma_codegen::ir::IRType::U64),
            }));
            // Store arena.capacity at [arena_ptr+16]
            let cap_addr = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: cap_addr.clone(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var(arena_ptr.clone()),
                rhs: ScgExpr::Int(16),
                tail_call: false,
                reassigns: None,
            }));
            stmts.push(ScgStatement::Access(AccessNode::Store {
                ptr: ScgExpr::Var(cap_addr),
                offset: None,
                value: cap_expr,
                ty: Some(vuma_codegen::ir::IRType::U64),
            }));
            ScgExpr::Var(arena_ptr)
        }

        // arena_alloc(arena, Layout) → State<Layout>:
        //   arena_ptr IS the data base. Arena data starts at offset 24.
        //   We load arena.offset (at [arena_ptr+8]), compute ptr = arena_ptr + offset,
        //   bump the offset, and return ptr as the new State<Layout>.
        Expr::ArenaAlloc { arena, layout_name, .. } => {
            let arena_ptr = flatten_expr(arena, stmts, ctx);
            // Load arena.offset at [arena_ptr+8] via ComputationNode::Add
            // + AccessNode::Load (the SAME path as PMT state.field reads).
            let off_addr = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: off_addr.clone(),
                op: BinOpKind::Add,
                lhs: arena_ptr.clone(),
                rhs: ScgExpr::Int(8),
                tail_call: false,
                reassigns: None,
            }));
            let offset_val = ctx.alloc_temp();
            stmts.push(ScgStatement::Access(AccessNode::Load {
                dst: offset_val.clone(),
                ptr: ScgExpr::Var(off_addr),
                offset: None,
                ty: Some(vuma_codegen::ir::IRType::U64),
            }));
            // Get layout_size from the layout registry
            let layout_size = ctx.layouts.get(layout_name)
                .map(|(size, _)| *size as i64)
                .unwrap_or(8);
            // Compute new_offset = offset + layout_size
            let new_offset = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: new_offset.clone(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var(offset_val.clone()),
                rhs: ScgExpr::Int(layout_size),
                tail_call: false,
                reassigns: None,
            }));
            // ── Bounds check (K0e/K0f DoD): load arena.capacity at
            // [arena_ptr+16], compute (new_offset > capacity) unsigned,
            // and if true call __arena_overflow() which exits(1).
            // This is the runtime guard the IVE arena_bounds verifier
            // expects to be present at every arena_alloc site.
            let cap_addr = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: cap_addr.clone(),
                op: BinOpKind::Add,
                lhs: arena_ptr.clone(),
                rhs: ScgExpr::Int(16),
                tail_call: false,
                reassigns: None,
            }));
            let cap_val = ctx.alloc_temp();
            stmts.push(ScgStatement::Access(AccessNode::Load {
                dst: cap_val.clone(),
                ptr: ScgExpr::Var(cap_addr),
                offset: None,
                ty: Some(vuma_codegen::ir::IRType::U64),
            }));
            let overflow_cond = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: overflow_cond.clone(),
                op: BinOpKind::UGt,
                lhs: ScgExpr::Var(new_offset.clone()),
                rhs: ScgExpr::Var(cap_val),
                tail_call: false,
                reassigns: None,
            }));
            stmts.push(ScgStatement::Control(ControlNode::If {
                cond: ScgExpr::Var(overflow_cond),
                then_body: vec![ScgStatement::Call(CallNode {
                    dst: None,
                    func: "__arena_overflow".to_string(),
                    args: vec![],
                    is_extern: true,
                    reassigns: None,
                })],
                else_body: None,
            }));
            // Compute state_ptr = arena_ptr + offset (arena_ptr IS the base)
            let state_ptr = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: state_ptr.clone(),
                op: BinOpKind::Add,
                lhs: arena_ptr.clone(),
                rhs: ScgExpr::Var(offset_val),
                tail_call: false,
                reassigns: None,
            }));
            // Store new_offset back to arena.offset at [arena_ptr+8]
            let off_addr2 = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: off_addr2.clone(),
                op: BinOpKind::Add,
                lhs: arena_ptr,
                rhs: ScgExpr::Int(8),
                tail_call: false,
                reassigns: None,
            }));
            stmts.push(ScgStatement::Access(AccessNode::Store {
                ptr: ScgExpr::Var(off_addr2),
                offset: None,
                value: ScgExpr::Var(new_offset),
                ty: Some(vuma_codegen::ir::IRType::U64),
            }));
            ScgExpr::Var(state_ptr)
        }

        // arena_grow(arena, min_capacity) → State<Arena>:
        //   Load arena.capacity, call mremap, store new capacity.
        //   mremap may return a NEW base (MREMAP_MAYMOVE). Return the
        //   new_base as the updated arena pointer.
        Expr::ArenaGrow { arena, min_capacity, .. } => {
            let arena_ptr = flatten_expr(arena, stmts, ctx);
            let min_cap_expr = flatten_expr(min_capacity, stmts, ctx);
            // Load arena.capacity at [arena_ptr+16] via ComputationNode::Add
            // + AccessNode::Load
            let cap_addr = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: cap_addr.clone(),
                op: BinOpKind::Add,
                lhs: arena_ptr.clone(),
                rhs: ScgExpr::Int(16),
                tail_call: false,
                reassigns: None,
            }));
            let cap_val = ctx.alloc_temp();
            stmts.push(ScgStatement::Access(AccessNode::Load {
                dst: cap_val.clone(),
                ptr: ScgExpr::Var(cap_addr),
                offset: None,
                ty: Some(vuma_codegen::ir::IRType::U64),
            }));
            // Call mremap(arena_ptr, capacity, min_capacity, MREMAP_MAYMOVE=1)
            let new_base = ctx.alloc_temp();
            stmts.push(ScgStatement::Call(CallNode {
                dst: Some(new_base.clone()),
                func: "mremap".to_string(),
                args: vec![
                    arena_ptr,
                    ScgExpr::Var(cap_val),
                    min_cap_expr.clone(),
                    ScgExpr::Int(1),
                ],
                is_extern: true,
                reassigns: None,
            }));
            // Store min_capacity at [new_base+16]
            let cap_addr2 = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: cap_addr2.clone(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var(new_base.clone()),
                rhs: ScgExpr::Int(16),
                tail_call: false,
                reassigns: None,
            }));
            stmts.push(ScgStatement::Access(AccessNode::Store {
                ptr: ScgExpr::Var(cap_addr2),
                offset: None,
                value: min_cap_expr,
                ty: Some(vuma_codegen::ir::IRType::U64),
            }));
            ScgExpr::Var(new_base)
        }

        // arena_free(arena) → void:
        //   Load arena.capacity, call munmap(arena_ptr, capacity).
        Expr::ArenaFree { arena, .. } => {
            let arena_ptr = flatten_expr(arena, stmts, ctx);
            // Load arena.capacity at [arena_ptr+16] via ComputationNode::Add
            // + AccessNode::Load
            let cap_addr = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: cap_addr.clone(),
                op: BinOpKind::Add,
                lhs: arena_ptr.clone(),
                rhs: ScgExpr::Int(16),
                tail_call: false,
                reassigns: None,
            }));
            let cap_val = ctx.alloc_temp();
            stmts.push(ScgStatement::Access(AccessNode::Load {
                dst: cap_val.clone(),
                ptr: ScgExpr::Var(cap_addr),
                offset: None,
                ty: Some(vuma_codegen::ir::IRType::U64),
            }));
            // Call munmap(arena_ptr, capacity)
            stmts.push(ScgStatement::Call(CallNode {
                dst: None,
                func: "munmap".to_string(),
                args: vec![
                    arena_ptr,
                    ScgExpr::Var(cap_val),
                ],
                is_extern: true,
                reassigns: None,
            }));
            ScgExpr::Int(0)
        }

        // ── If-expression: `if cond { then } else { else }` ──
        // Lowers to: if cond { result = <then_value> } else { result = <else_value> }
        // then returns `result` as a ScgExpr::Var.
        // The then/else blocks' trailing expression (last Stmt::Expr) is the
        // branch value. bridge_block_to_scg_stmts stores the last ExprStmt's
        // result in ctx.last_expr_result.
        Expr::IfExpr { condition, then_block, else_block, .. } => {
            // Evaluate the condition.
            let cond_expr = flatten_expr(condition, stmts, ctx);
            // Allocate a temp for the result.
            let result_tmp = ctx.alloc_temp();
            // Lower the then-block.
            let mut then_body = {
                ctx.last_expr_result = None;
                let mut tb = bridge_block_to_scg_stmts(then_block, ctx);
                let then_val = ctx.last_expr_result.take().unwrap_or(ScgExpr::Int(0));
                tb.push(ScgStatement::Computation(ComputationNode {
                    dst: result_tmp.clone(),
                    op: BinOpKind::Add,
                    lhs: then_val,
                    rhs: ScgExpr::Int(0),
                    tail_call: false,
                    reassigns: None,
                }));
                tb
            };
            // Lower the else-block.
            let mut else_body = {
                ctx.last_expr_result = None;
                let mut eb = bridge_block_to_scg_stmts(else_block, ctx);
                let else_val = ctx.last_expr_result.take().unwrap_or(ScgExpr::Int(0));
                eb.push(ScgStatement::Computation(ComputationNode {
                    dst: result_tmp.clone(),
                    op: BinOpKind::Add,
                    lhs: else_val,
                    rhs: ScgExpr::Int(0),
                    tail_call: false,
                    reassigns: None,
                }));
                eb
            };
            let _ = &mut then_body;
            let _ = &mut else_body;
            // Emit the if-statement with both branches.
            stmts.push(ScgStatement::Control(ControlNode::If {
                cond: cond_expr,
                then_body,
                else_body: Some(else_body),
            }));
            // Return the result temp.
            ScgExpr::Var(result_tmp)
        }

        // ── Wave 2: Struct literal — `Point { x: 10, y: 20 }` ──
        // Lower to: allocate stack slot, register as state-typed, write each field.
        Expr::StructInit { name, fields, .. } => {
            // Look up the layout to get the total size.
            if let Some((total_size, layout_fields)) = ctx.layouts.get(name).cloned() {
                let temp = ctx.alloc_temp();
                // Emit stack allocation for the state buffer.
                stmts.push(ScgStatement::Allocation(AllocationNode::Stack {
                    name: temp.clone(),
                    size: total_size as u32,
                    ty: ScgType::Ptr,
                }));
                // Register the temp as state-typed so field accesses work.
                ctx.state_var_layouts.insert(temp.clone(), name.clone());
                // For each field, emit a Store at the field's offset.
                for (field_name, field_expr) in fields {
                    // Find the field's offset, size, and type_name from the layout registry.
                    let (offset, _size, field_ty, type_name) = layout_fields.iter()
                        .find_map(|(fn_, ir_ty, off, sz, tn)| {
                            if fn_ == field_name {
                                Some((*off, *sz, ir_ty.clone(), tn.clone()))
                            } else {
                                None
                            }
                        })
                        .unwrap_or((0, 0, vuma_codegen::ir::IRType::U64, String::new()));

                    // Wave 33: Handle nested struct literal fields inline.
                    // If the field's type_name is a known layout AND the field
                    // expression is a StructInit, write the nested struct's
                    // fields directly into the parent buffer at the cumulative
                    // offset (rather than allocating a separate temp and storing
                    // its pointer). This makes `b.a.x` resolve correctly because
                    // the nested fields are inline in the parent buffer.
                    if ctx.layouts.contains_key(&type_name) {
                        if let vuma_parser::ast::Expr::StructInit { name: nested_name, fields: nested_fields, .. } = field_expr {
                            if let Some((_nested_total, nested_layout_fields)) = ctx.layouts.get(&type_name).cloned() {
                                // Write each nested field directly into the parent buffer
                                // at (parent_base + field_offset + nested_field_offset).
                                for (nested_fname, nested_fexpr) in nested_fields {
                                    let (nested_off, nested_fty) = nested_layout_fields.iter()
                                        .find_map(|(fn_, ir_ty, off, _sz, _tn)| {
                                            if fn_ == nested_fname {
                                                Some((*off, ir_ty.clone()))
                                            } else {
                                                None
                                            }
                                        })
                                        .unwrap_or((0, vuma_codegen::ir::IRType::U64));
                                    let cum_offset = offset + nested_off;
                                    let nptr = if cum_offset == 0 {
                                        ScgExpr::Var(temp.clone())
                                    } else {
                                        let naddr = ctx.alloc_temp();
                                        stmts.push(ScgStatement::Computation(ComputationNode {
                                            dst: naddr.clone(),
                                            op: BinOpKind::Add,
                                            lhs: ScgExpr::Var(temp.clone()),
                                            rhs: ScgExpr::Int(cum_offset as i64),
                                            tail_call: false,
                                            reassigns: None,
                                        }));
                                        ScgExpr::Var(naddr)
                                    };
                                    let nval = flatten_expr(nested_fexpr, stmts, ctx);
                                    stmts.push(ScgStatement::Access(AccessNode::Store {
                                        ptr: nptr,
                                        offset: None,
                                        value: nval,
                                        ty: Some(nested_fty),
                                    }));
                                }
                                // Skip the default Store below — nested fields written inline.
                                continue;
                            }
                        }
                    }

                    // Default: compute address: base + offset.
                    let ptr = if offset == 0 {
                        ScgExpr::Var(temp.clone())
                    } else {
                        let addr_tmp = ctx.alloc_temp();
                        stmts.push(ScgStatement::Computation(ComputationNode {
                            dst: addr_tmp.clone(),
                            op: BinOpKind::Add,
                            lhs: ScgExpr::Var(temp.clone()),
                            rhs: ScgExpr::Int(offset as i64),
                            tail_call: false,
                            reassigns: None,
                        }));
                        ScgExpr::Var(addr_tmp)
                    };
                    // Flatten the field value.
                    let value = flatten_expr(field_expr, stmts, ctx);
                    // Emit the Store.
                    stmts.push(ScgStatement::Access(AccessNode::Store {
                        ptr,
                        offset: None,
                        value,
                        ty: Some(field_ty),
                    }));
                }
                // Return the temp as the expression result.
                ScgExpr::Var(temp)
            } else {
                eprintln!("[vuma] WARNING: struct literal with unknown layout '{}'; using 0", name);
                ScgExpr::Int(0)
            }
        }

        // ── Wave 8b: Block expression (used in match arm bodies) ──
        // `{ stmt; stmt; expr }` — flatten each statement into `stmts` (via
        // `bridge_stmt_to_scg`), then flatten the optional trailing
        // expression.  The block's value is the trailing expression's value
        // (or 0 / unit if absent).  This is what makes
        // `match channel_recv(ch) { Ok(v) => { return 7; }, Err(e) => { return 99; } }`
        // lower correctly: the block's `return` statement flows into the
        // arm's ScgStatement list and becomes an `IRInstr::Ret`.
        Expr::Block { statements, trailing_expr, .. } => {
            for s in statements {
                let sub = bridge_stmt_to_scg(s, ctx);
                stmts.extend(sub);
            }
            match trailing_expr {
                Some(tail) => flatten_expr(tail, stmts, ctx),
                None => ScgExpr::Int(0),
            }
        }

        // ── Fallback for unsupported expression types ──
        // Log a warning instead of silently returning 0. This makes
        // unsupported constructs visible during compilation.
        _ => {
            eprintln!("[vuma] WARNING: unsupported expression type in flatten_expr; using 0");
            ScgExpr::Int(0)
        }
    }
}

/// Map a VUMA AST BinOp to a codegen BinOpKind.
pub fn map_ast_binop(op: &vuma_parser::ast::BinOp) -> BinOpKind {
    use vuma_parser::ast::BinOp;
    match op {
        BinOp::Add => BinOpKind::Add,
        BinOp::Sub => BinOpKind::Sub,
        BinOp::Mul => BinOpKind::Mul,
        BinOp::Div => BinOpKind::SDiv,
        BinOp::Mod => BinOpKind::SRem,
        BinOp::And => BinOpKind::And,
        BinOp::Or => BinOpKind::Or,
        BinOp::BitAnd => BinOpKind::And,
        BinOp::BitOr => BinOpKind::Or,
        BinOp::BitXor => BinOpKind::Xor,
        BinOp::Shl => BinOpKind::Shl,
        // VUMA's `>>` DEFAULTS to LOGICAL (unsigned) shift. The actual
        // type-aware decision (ShrA for signed, ShrL for unsigned) is made
        // in `flatten_expr` based on the lhs operand's declared type — see
        // Task 7-A. This default applies when the operand type is unknown
        // (e.g. untyped let-bindings, function parameters without type
        // annotations), which is the common case for the bit-twiddling
        // gold-standard tests (`bit_log2`, `bit_priority_encoder`,
        // `bit2_priority_encode` all use `u64` and expect logical shifts).
        //
        // `bit_abs` (i64) gets ShrA via the type-aware path because it
        // explicitly declares `i64`, so reverting this default from ShrA
        // back to ShrL does NOT regress it.
        //
        // Historical note: 6-A changed this to `ShrA` unconditionally to
        // fix `bit_abs`, which broke the 3 unsigned-shift tests on all 10
        // backends (30 failures). The type-aware path is the correct fix.
        BinOp::Shr => BinOpKind::ShrL,
        BinOp::Eq => BinOpKind::Eq,
        BinOp::Ne => BinOpKind::Ne,
        BinOp::Lt => BinOpKind::SLt,
        BinOp::Le => BinOpKind::SLe,
        BinOp::Gt => BinOpKind::SGt,
        BinOp::Ge => BinOpKind::SGe,
    }
}

/// Convert a single parser statement into zero or more codegen SCG statements.
/// Uses `flatten_expr` to decompose nested expressions into three-address code.
/// Infer the load type from an AST expression (before flattening).
///
/// Checks for the pattern `base + idx * stride` in the AST and infers:
/// - stride 4 → U32
/// - stride 8 → U64
fn infer_load_type_from_ast_expr(expr: &vuma_parser::ast::Expr) -> Option<vuma_codegen::ir::IRType> {
    use vuma_parser::ast::{Expr, BinOp, Lit};
    if let Expr::BinOp { op: BinOp::Add, lhs: _, rhs, .. } = expr {
        if let Expr::BinOp { op: BinOp::Mul, lhs: _, rhs: mul_rhs, .. } = rhs.as_ref() {
            if let Expr::Lit { value: Lit::Int(stride), .. } = mul_rhs.as_ref() {
                return match *stride {
                    8 => Some(vuma_codegen::ir::IRType::U64),
                    4 => Some(vuma_codegen::ir::IRType::U32),
                    _ => None,
                };
            }
        }
    }
    None
}

/// Infer the load type from a codegen SCG expression (after flattening).
fn infer_load_type_from_ptr(ptr: &ScgExpr) -> Option<vuma_codegen::ir::IRType> {
    if let ScgExpr::BinOp { op: vuma_codegen::ir::BinOpKind::Add, lhs: _, rhs } = ptr {
        if let ScgExpr::BinOp { op: vuma_codegen::ir::BinOpKind::Mul, lhs: _, rhs } = rhs.as_ref() {
            if let ScgExpr::Int(stride) = rhs.as_ref() {
                return match *stride {
                    8 => Some(vuma_codegen::ir::IRType::U64),
                    4 => Some(vuma_codegen::ir::IRType::U32),
                    _ => None,
                };
            }
        }
    }
    None
}

/// Convert a VUMA parser AST statement into a sequence of SCG
/// (Semantic Code Graph) statements, using `ctx` to track variable
/// types and other bridging state.
///
/// This is the bridge entry point used by the parser→SCG frontend.
pub fn bridge_stmt_to_scg(stmt: &vuma_parser::ast::Stmt, ctx: &mut BridgeCtx) -> Vec<ScgStatement> {
    use vuma_parser::ast::Stmt as PStmt;

    match stmt {
        // ── let x [: T] = expr ──
        PStmt::Let(let_stmt) => {
            let mut stmts = Vec::new();

            // Record the variable's declared type (if any) so `flatten_expr`
            // can choose ShrA (signed) vs ShrL (unsigned) for `>>` operations
            // — see Task 7-A (type-aware shift regression). Without this,
            // all `>>` operations collapse to a single shift kind, breaking
            // either `bit_abs` (i64 → needs ShrA) or `bit_log2` (u64 →
            // needs ShrL).
            //
            // Only primitive integer types (i8/i16/i32/i64/u8/u16/u32/u64)
            // and pointers are recorded; other types map to `ScgType::Void`
            // via `bridge_type_to_codegen_scg` and are skipped (no shift
            // type information for aggregates/void/etc.).
            //
            // This must run BEFORE the early-return paths below (allocate,
            // Allocate, call) so the type is registered regardless of which
            // path the let-binding takes. The early-return paths don't go
            // through `flatten_expr`, but the variable may still be used as
            // a shift operand in subsequent statements.
            if let Some(ref ty) = let_stmt.ty {
                let scg_ty = bridge_type_to_codegen_scg(&Some(ty.clone()));
                if scg_ty != ScgType::Void {
                    ctx.var_types.insert(let_stmt.name.clone(), scg_ty);
                }
                // PMT (Wave 2): register state-typed vars (`let p: State<L> = ...`)
                // so subsequent `p.field` accesses lower to Loads with the
                // layout's field offsets.
                if let Some(layout_name) = extract_state_layout_name_from_ast(ty) {
                    ctx.state_var_layouts.insert(let_stmt.name.clone(), layout_name);
                }
            }

            // PMT (Wave 2): `let p = state_new(Layout)` → AllocationNode::Stack
            // sized to the layout's total_size. The resulting stack slot
            // holds the state's buffer pointer; subsequent `p.field` reads/
            // writes use it as the Load/Store address.
            if let vuma_parser::ast::Expr::StateInit { layout_name, .. } = &let_stmt.value {
                if let Some((total_size, _)) = ctx.layouts.get(layout_name) {
                    // Register the var as state-typed BEFORE returning so
                    // subsequent `p.field` accesses can find the layout.
                    ctx.state_var_layouts.insert(let_stmt.name.clone(), layout_name.clone());
                    return vec![ScgStatement::Allocation(AllocationNode::Stack {
                        name: let_stmt.name.clone(),
                        size: *total_size as u32,
                        ty: ScgType::Ptr,
                    })];
                }
            }

            // Wave 2: `let p = Point { x: 10, y: 20 }` — register the let
            // variable as state-typed so subsequent `p.field` accesses work.
            // The actual allocation + field writes are handled by flatten_expr's
            // StructInit arm (which emits an AllocationNode::Stack for a temp,
            // writes each field, and returns the temp). Here we just pre-register
            // the let variable name as state-typed with the struct's layout.
            if let vuma_parser::ast::Expr::StructInit { name, .. } = &let_stmt.value {
                if ctx.layouts.contains_key(name) {
                    ctx.state_var_layouts.insert(let_stmt.name.clone(), name.clone());
                }
            }

            // Arena State Model (Wave 3a): `let arena = arena_new(cap)` and
            // `let w = arena_alloc(arena, Widget)` → register as state-typed
            // so field access works. The actual lowering happens in
            // flatten_expr (which emits mmap/mremap/munmap CallNodes +
            // Load/Store for the Arena struct). Here we just register the
            // variable as state-typed with the correct layout.
            match &let_stmt.value {
                vuma_parser::ast::Expr::ArenaNew { .. } => {
                    ctx.state_var_layouts.insert(let_stmt.name.clone(), "Arena".to_string());
                }
                vuma_parser::ast::Expr::ArenaAlloc { layout_name, .. } => {
                    ctx.state_var_layouts.insert(let_stmt.name.clone(), layout_name.clone());
                }
                vuma_parser::ast::Expr::ArenaGrow { .. } => {
                    // arena_grow returns the arena — it's already registered
                    // as state-typed from the original arena_new/arena_alloc.
                    // Just re-register with "Arena" layout to be safe.
                    ctx.state_var_layouts.insert(let_stmt.name.clone(), "Arena".to_string());
                }
                _ => {}
            }

            // Check if the RHS is an allocate() call → AllocationNode::Stack
            if let vuma_parser::ast::Expr::Call { callee, args, .. } = &let_stmt.value {
                if let vuma_parser::ast::Expr::Var { name, .. } = callee.as_ref() {
                    if name == "allocate" {
                        let size: u32 = args.first()
                            .and_then(|a| {
                                if let vuma_parser::ast::Expr::Lit {
                                    value: vuma_parser::ast::Lit::Int(n),
                                    ..
                                } = a
                                {
                                    return Some(*n as u32);
                                }
                                None
                            })
                            .unwrap_or(8);
                        return vec![ScgStatement::Allocation(AllocationNode::Stack {
                            name: let_stmt.name.clone(),
                            size,
                            ty: ScgType::Ptr,
                        })];
                    }
                    // Other function calls → CallNode (flatten args)
                    let flat_args: Vec<ScgExpr> = args.iter()
                        .map(|a| flatten_expr(a, &mut stmts, ctx))
                        .collect();
                    let is_extern = ctx.extern_fns.contains(name)
                        || name == "__vuma_alloc"
                        || name == "__vuma_dealloc";
                    // FIX1: if the callee is a fn/transform whose return
                    // type is `State<L>`, register the let-binding as
                    // state-typed with that layout so subsequent `dst.field`
                    // accesses resolve to Loads at the right offset —
                    // without requiring `let dst: State<L> = ...`.
                    if let Some(layout_name) = ctx.state_returning_fns.get(name).cloned() {
                        ctx.state_var_layouts.insert(let_stmt.name.clone(), layout_name);
                    }
                    stmts.push(ScgStatement::Call(CallNode {
                        dst: Some(let_stmt.name.clone()),
                        func: name.clone(),
                        args: flat_args,
                        is_extern,
                        reassigns: None,
                    }));
                    return stmts;
                }
            }

            // Wave 10: `let x = syscall(nr, args…)` → SyscallCallNode with
            // dst = the let-binding's name. This avoids a wasted temp +
            // Add(0) copy when the result is consumed directly.
            if let vuma_parser::ast::Expr::Syscall { nr, args, .. } = &let_stmt.value {
                let flat_args: Vec<ScgExpr> = args
                    .iter()
                    .map(|a| flatten_expr(a, &mut stmts, ctx))
                    .collect();
                stmts.push(ScgStatement::Syscall(SyscallCallNode {
                    nr: *nr,
                    dst: Some(let_stmt.name.clone()),
                    args: flat_args,
                }));
                return stmts;
            }

            // Check if the RHS is an Allocate expression → AllocationNode::Stack
            if let vuma_parser::ast::Expr::Allocate { size, .. } = &let_stmt.value {
                let size_val: u32 = match size.as_ref() {
                    vuma_parser::ast::Expr::Lit { value: vuma_parser::ast::Lit::Int(n), .. } => {
                        *n as u32
                    }
                    _ => 8,
                };
                return vec![ScgStatement::Allocation(AllocationNode::Stack {
                    name: let_stmt.name.clone(),
                    size: size_val,
                    ty: ScgType::Ptr,
                })];
            }

            // General case: flatten the expression and assign to dst
            let result = flatten_expr(&let_stmt.value, &mut stmts, ctx);
            // G7: if the last statement is a CallNode, set its reassigns to
            // the let-binding's name so lower_call can look up the dst type
            // in fn_var_types (critical for floattofloat widen/narrow).
            if let Some(ScgStatement::Call(call_node)) = stmts.last_mut() {
                call_node.reassigns = Some(let_stmt.name.clone());
            }
            match &result {
                ScgExpr::Var(name) if name == &let_stmt.name => {}
                _ => {
                    // Arena State Model (Wave 3a): for arena_alloc results,
                    // the temp IS the state pointer. We need to assign it to
                    // the let-binding name so field access (w.x) resolves.
                    // Use Add(result, 0) to create a named copy that
                    // identify_state_vars can find.
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: let_stmt.name.clone(),
                        op: BinOpKind::Add,
                        lhs: result,
                        rhs: ScgExpr::Int(0),
                        tail_call: false,
                        reassigns: None,
                    }));
                }
            }
            stmts
        }

        // ── target = value ──
        PStmt::Assign(assign_stmt) => {
            let mut stmts = Vec::new();

            // Detect dereference writes: `*expr = val` → Access::Store
            if let vuma_parser::ast::AssignTarget::Deref { expr, .. } = &assign_stmt.target {
                // For chained deref targets (`**p = val`, `***p = val`, ...),
                // the inner Deref chain must be flattened with pointer-width
                // (U64) loads so intermediate addresses aren't truncated to
                // their low byte. For a single `*p = val`, the value of `p`
                // IS the address to store at — no load is needed.
                //
                // Without this, `**buf1 = 42` (where buf1 holds buf2's
                // address) loads only 1 byte of buf2's address, then stores
                // 42 at the truncated address → SIGSEGV. See
                // ptr_pointer_to_pointer gold-standard test.
                let ptr = if let vuma_parser::ast::Expr::Deref { .. } = expr.as_ref() {
                    // Chained deref target. Walk the inner Deref chain; every
                    // load in the chain reads a pointer (U64).
                    let mut chain_depth: usize = 0;
                    let mut cur = expr.as_ref();
                    while let vuma_parser::ast::Expr::Deref { expr: inner, .. } = cur {
                        chain_depth += 1;
                        cur = inner.as_ref();
                    }
                    let mut current_addr = flatten_expr(cur, &mut stmts, ctx);
                    for _ in 0..chain_depth {
                        let dst = ctx.alloc_temp();
                        stmts.push(ScgStatement::Access(AccessNode::Load {
                            dst: dst.clone(),
                            ptr: current_addr,
                            offset: None,
                            ty: Some(vuma_codegen::ir::IRType::U64),
                        }));
                        current_addr = ScgExpr::Var(dst);
                    }
                    current_addr
                } else {
                    // Single deref: `*p = val`. `p`'s value is the store address.
                    flatten_expr(expr, &mut stmts, ctx)
                };
                let value = flatten_expr(&assign_stmt.value, &mut stmts, ctx);
                // Infer store type from the address expression.
                // For both single and chained deref, use stride-based inference.
                // Do NOT force U64 for chained deref stores — the store type
                // should match the value being stored (e.g. 42 → U8), not the
                // address expression.  The intermediate LOADS in a chained
                // deref already use U64 (set above), so pointers are handled
                // correctly.  Forcing U64 here would cause a store/load type
                // mismatch: **ptr = 42 would store as U64, but **ptr (load)
                // would default to U8, reading the wrong byte on big-endian.
                let store_ty = infer_load_type_from_ast_expr(expr.as_ref());
                stmts.push(ScgStatement::Access(AccessNode::Store {
                    ptr,
                    offset: None,
                    value,
                    ty: store_ty,
                }));
                return stmts;
            }

            // Handle Index target: ptr[index] = value
            if let vuma_parser::ast::AssignTarget::Index { expr, index, .. } = &assign_stmt.target {
                let base = flatten_expr(expr, &mut stmts, ctx);
                let idx = flatten_expr(index, &mut stmts, ctx);

                // Determine the array element type for proper index scaling.
                let (elem_size, elem_ir_type) = detect_array_elem_type(expr, ctx);

                // Scale the index by element size if needed.
                let scaled_idx = if elem_size > 1 {
                    let mul_dst = ctx.alloc_temp();
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: mul_dst.clone(),
                        op: BinOpKind::Mul,
                        lhs: idx,
                        rhs: ScgExpr::Int(elem_size as i64),
                        tail_call: false,
                        reassigns: None,
                    }));
                    ScgExpr::Var(mul_dst)
                } else {
                    idx
                };

                let addr = ctx.alloc_temp();
                stmts.push(ScgStatement::Computation(ComputationNode {
                    dst: addr.clone(),
                    op: BinOpKind::Add,
                    lhs: base,
                    rhs: scaled_idx,
                    tail_call: false,
                    reassigns: None,
                }));
                let value = flatten_expr(&assign_stmt.value, &mut stmts, ctx);
                stmts.push(ScgStatement::Access(AccessNode::Store {
                    ptr: ScgExpr::Var(addr),
                    offset: None,
                    value,
                    ty: elem_ir_type,
                }));
                return stmts;
            }

            // PMT (Wave 2): state-field write — `p.field = val` (and nested
            // `l.a.x = val`). The parser represents these as
            // `AssignTarget::DerefField { expr, field }`. We walk the
            // FieldAccess chain to find the base state-typed var, resolve
            // the cumulative field offset against the layout registry, and
            // emit a single Store at that offset.
            if let vuma_parser::ast::AssignTarget::DerefField { expr, field, .. } = &assign_stmt.target {
                // Walk the chain to find (base_var, [field1, field2, ..., field]).
                let mut chain = vec![field.clone()];
                let mut cur = expr.as_ref();
                let mut base_var: Option<String> = None;
                while let vuma_parser::ast::Expr::FieldAccess { expr: inner, field: f, .. } = cur {
                    chain.push(f.clone());
                    cur = inner.as_ref();
                }
                if let vuma_parser::ast::Expr::Var { name, .. } = cur {
                    base_var = Some(name.clone());
                }
                if let Some(bv) = &base_var {
                    if let Some(layout_name) = ctx.state_var_layouts.get(bv).cloned() {
                        chain.reverse(); // outermost-to-innermost order
                        if let Some((offset, _size, field_ty, _type_name)) =
                            resolve_state_field_chain(&ctx.layouts, &layout_name, &chain)
                        {
                            // Wave 2 fix: emit an explicit `Add(base, offset)`
                            // Computation node so each Store gets a UNIQUE
                            // address vreg (mirrors the Load-side fix in
                            // `flatten_expr`'s FieldAccess arm). Without
                            // this, two Stores to different fields of the
                            // same state buffer would share the same `addr`
                            // IRValue, and `dead_store_eliminate` would
                            // incorrectly eliminate the first Store
                            // (returning the LAST field written instead of
                            // the sum of all fields). The `Add` result
                            // inherits the base's `Unique(N)` alias class
                            // via `AliasAnalysis`'s BinOp arm, so DSE then
                            // conservatively treats the two Stores as
                            // may-alias (keeping both).
                            let ptr = if offset == 0 {
                                flatten_expr(&vuma_parser::ast::Expr::Var {
                                    name: bv.clone(),
                                    span: vuma_parser::Span::synthetic(),
                                }, &mut stmts, ctx)
                            } else {
                                let base = flatten_expr(&vuma_parser::ast::Expr::Var {
                                    name: bv.clone(),
                                    span: vuma_parser::Span::synthetic(),
                                }, &mut stmts, ctx);
                                let addr_tmp = ctx.alloc_temp();
                                stmts.push(ScgStatement::Computation(ComputationNode {
                                    dst: addr_tmp.clone(),
                                    op: BinOpKind::Add,
                                    lhs: base,
                                    rhs: ScgExpr::Int(offset as i64),
                                    tail_call: false,
                                    reassigns: None,
                                }));
                                ScgExpr::Var(addr_tmp)
                            };
                            let value = flatten_expr(&assign_stmt.value, &mut stmts, ctx);
                            stmts.push(ScgStatement::Access(AccessNode::Store {
                                ptr,
                                offset: None,
                                value,
                                ty: Some(field_ty),
                            }));
                            return stmts;
                        } else {
                        }
                    } else {
                    }
                }
                // Not a state-typed field write — fall through to the
                // generic handling below (treats `field` as a var name).
            }

            let dst = match &assign_stmt.target {
                vuma_parser::ast::AssignTarget::Var { name, .. } => name.clone(),
                vuma_parser::ast::AssignTarget::DerefField { field, .. } => field.clone(),
                vuma_parser::ast::AssignTarget::Deref { .. } => "_deref".into(),
                vuma_parser::ast::AssignTarget::Index { .. } => "_index".into(),
            };

            // Detect allocate() expression → AllocationNode::Stack
            if let vuma_parser::ast::Expr::Allocate { size, .. } = &assign_stmt.value {
                let size_val: u32 = match size.as_ref() {
                    vuma_parser::ast::Expr::Lit { value: vuma_parser::ast::Lit::Int(n), .. } => {
                        *n as u32
                    }
                    _ => 8,
                };
                return vec![ScgStatement::Allocation(AllocationNode::Stack {
                    name: dst,
                    size: size_val,
                    ty: ScgType::Ptr,
                })];
            }

            // Detect function calls in assign: `x = foo(args)` → CallNode (flatten args)
            if let vuma_parser::ast::Expr::Call { callee, args, .. } = &assign_stmt.value {
                if let vuma_parser::ast::Expr::Var { name, .. } = callee.as_ref() {
                    let flat_args: Vec<ScgExpr> = args.iter()
                        .map(|a| flatten_expr(a, &mut stmts, ctx))
                        .collect();
                    let is_extern = ctx.extern_fns.contains(name)
                        || name == "__vuma_alloc"
                        || name == "__vuma_dealloc";
                    stmts.push(ScgStatement::Call(CallNode {
                        dst: Some(dst),
                        func: name.clone(),
                        args: flat_args,
                        is_extern,
                        reassigns: None,
                    }));
                    return stmts;
                }
            }

            // Wave 10: `x = syscall(nr, args…)` → SyscallCallNode with
            // dst = the assignment target. Mirrors the Call path above.
            if let vuma_parser::ast::Expr::Syscall { nr, args, .. } = &assign_stmt.value {
                let flat_args: Vec<ScgExpr> = args
                    .iter()
                    .map(|a| flatten_expr(a, &mut stmts, ctx))
                    .collect();
                stmts.push(ScgStatement::Syscall(SyscallCallNode {
                    nr: *nr,
                    dst: Some(dst),
                    args: flat_args,
                }));
                return stmts;
            }

            // General case: flatten the value expression and assign to dst
            let result = flatten_expr(&assign_stmt.value, &mut stmts, ctx);
            match &result {
                ScgExpr::Var(name) if name == &dst => {}
                _ => {
                    stmts.push(ScgStatement::Computation(ComputationNode {
                        dst: dst.clone(),
                        op: BinOpKind::Add,
                        lhs: result,
                        rhs: ScgExpr::Int(0),
                        tail_call: false,
                        reassigns: Some(dst),
                    }));
                }
            }
            stmts
        }

        // ── target op= value ──
        PStmt::CompoundAssign(ca_stmt) => {
            let mut stmts = Vec::new();
            let dst = match &ca_stmt.target {
                vuma_parser::ast::AssignTarget::Var { name, .. } => name.clone(),
                vuma_parser::ast::AssignTarget::DerefField { field, .. } => field.clone(),
                _ => "_".into(),
            };
            let binop = match ca_stmt.op {
                vuma_parser::ast::CompoundOp::Add => BinOpKind::Add,
                vuma_parser::ast::CompoundOp::Sub => BinOpKind::Sub,
                vuma_parser::ast::CompoundOp::Mul => BinOpKind::Mul,
                vuma_parser::ast::CompoundOp::Div => BinOpKind::SDiv,
                vuma_parser::ast::CompoundOp::Mod => BinOpKind::SRem,
                vuma_parser::ast::CompoundOp::BitAnd => BinOpKind::And,
                vuma_parser::ast::CompoundOp::BitOr => BinOpKind::Or,
                vuma_parser::ast::CompoundOp::BitXor => BinOpKind::Xor,
                vuma_parser::ast::CompoundOp::Shl => BinOpKind::Shl,
                vuma_parser::ast::CompoundOp::Shr => BinOpKind::ShrL,
            };
            let rhs = flatten_expr(&ca_stmt.value, &mut stmts, ctx);
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: dst.clone(),
                op: binop,
                lhs: ScgExpr::Var(dst.clone()),
                rhs,
                tail_call: false,
                reassigns: Some(dst),
            }));
            stmts
        }

        // ── allocate(size);  (standalone — not bound to a variable) ──
        // Lower to a stack allocation when the size is a literal int (mirrors
        // the `let x = allocate(N)` path), else to a heap allocation with a
        // dynamic size expression. Previously this was silently dropped.
        PStmt::Allocate(alloc_stmt) => {
            let mut stmts = Vec::new();
            let temp = ctx.alloc_temp();
            // Try to extract a literal integer size; fall back to Heap otherwise.
            if let vuma_parser::ast::Expr::Lit {
                value: vuma_parser::ast::Lit::Int(n),
                ..
            } = &alloc_stmt.size
            {
                let size = (*n as u32).max(1); // never allocate 0 bytes
                stmts.push(ScgStatement::Allocation(AllocationNode::Stack {
                    name: temp,
                    size,
                    ty: ScgType::U8, // raw byte buffer; caller may cast the pointer
                }));
            } else {
                let size_expr = flatten_expr(&alloc_stmt.size, &mut stmts, ctx);
                stmts.push(ScgStatement::Allocation(AllocationNode::Heap {
                    name: temp,
                    size_expr,
                    ty: ScgType::U8,
                }));
            }
            stmts
        }

        // ── free(ptr);  (standalone) ──
        // P2: `free(ptr)` now lowers directly to a SyscallCallNode (munmap,
        // nr 215) instead of a Call to the Rust-emitted `__vuma_free`
        // runtime stub. Eliminates the last Rust-wrapper dependency in the
        // heap-deallocation path.
        //
        // FreeStmt: free() is a no-op in VUMA's current memory model.
        // Stack allocations (IRInstr::Alloc) are freed automatically when
        // the function returns (stack frame deallocation). Heap allocations
        // (via mmap) are freed automatically when the process exits.
        //
        // The previous implementation emitted Syscall(215, munmap, [ptr, 0]),
        // but munmap(addr, 0) is a no-op (length=0 → nothing unmapped), so
        // this was already a no-op. However, the Syscall instruction itself
        // was treated as a barrier by the scheduler and DSE, causing
        // incorrect reordering of atomic operations around free() calls.
        // Removing the Syscall entirely eliminates this issue.
        //
        // When real per-allocation deallocation is needed (e.g. long-running
        // servers), the memory model will need to track heap vs stack
        // allocations and emit munmap only for heap allocations with the
        // correct size.
        PStmt::Free(_free_stmt) => {
            // No-op: stack allocations are freed on function return,
            // heap allocations are freed on process exit.
            Vec::new()
        }

        // ── expr as Type;  (standalone cast) ──
        // Lower to a proper CastNode so the type conversion is preserved in
        // the IR. Previously this kept the operand (via flatten_expr) but
        // discarded the target type, producing an incorrect no-op.
        PStmt::Cast(cast_stmt) => {
            let mut stmts = Vec::new();
            let src = flatten_expr(&cast_stmt.expr, &mut stmts, ctx);
            let temp = ctx.alloc_temp();
            // The AST's CastStmt always carries a target type (target_type: Type,
            // not Option<Type>); use the existing bridge to map it to ScgType.
            let to_ty = bridge_type_to_codegen_scg(&Some(cast_stmt.target_type.clone()));
            // Source type is not annotated in the AST — assume I64 (VUMA's
            // default integer width). The IR layer can refine this via BD.
            let from_ty = ScgType::I64;
            // Choose a CastKind based on the source/target bit widths.
            // Floats are not handled by this heuristic (bridge_type_to_codegen_scg
            // currently maps "f32"/"f64" to ScgType::I64), so the result is
            // always an integer-to-integer cast kind.
            let kind = {
                let from_bits = match from_ty {
                    ScgType::I8 | ScgType::U8 => 8,
                    ScgType::I16 | ScgType::U16 => 16,
                    ScgType::I32 | ScgType::U32 | ScgType::F32 => 32,
                    _ => 64, // I64, U64, Ptr, F64, Void
                };
                let to_bits = match to_ty {
                    ScgType::I8 | ScgType::U8 => 8,
                    ScgType::I16 | ScgType::U16 => 16,
                    ScgType::I32 | ScgType::U32 | ScgType::F32 => 32,
                    _ => 64,
                };
                if to_bits > from_bits {
                    CastKind::SExt
                } else if to_bits < from_bits {
                    CastKind::Trunc
                } else {
                    CastKind::BitCast
                }
            };
            stmts.push(ScgStatement::Cast(CastNode {
                dst: temp,
                src,
                kind,
                from_ty,
                to_ty,
            }));
            stmts
        }

        PStmt::Return(ret_stmt) => {
            let mut stmts = Vec::new();
            let values = match &ret_stmt.value {
                Some(expr) => vec![flatten_expr(expr, &mut stmts, ctx)],
                None => vec![],
            };
            stmts.push(ScgStatement::Return(values));
            stmts
        }

        PStmt::Expr(expr_stmt) => {
            let mut stmts = Vec::new();
            let result = flatten_expr(&expr_stmt.expr, &mut stmts, ctx);
            ctx.last_expr_result = Some(result);
            stmts
        }

        PStmt::If(if_stmt) => {
            let mut pre_stmts = Vec::new();
            let cond = flatten_expr(&if_stmt.condition, &mut pre_stmts, ctx);
            let then_body = bridge_block_to_scg_stmts(&if_stmt.then_block, ctx);
            let else_body = if_stmt
                .else_block
                .as_ref()
                .map(|b| bridge_block_to_scg_stmts(b, ctx));
            let mut result = pre_stmts;
            result.push(ScgStatement::Control(ControlNode::If {
                cond,
                then_body,
                else_body,
            }));
            result
        }

        // while condition { body }
        // Lowered as: Loop { <compute cond>; if cond { body } else { break } }
        PStmt::While(while_stmt) => {
            let then_body = bridge_block_to_scg_stmts(&while_stmt.body, ctx);
            let mut loop_body = Vec::new();
            let cond = flatten_expr(&while_stmt.condition, &mut loop_body, ctx);
            loop_body.push(ScgStatement::Control(ControlNode::If {
                cond,
                then_body,
                else_body: Some(vec![ScgStatement::Control(ControlNode::Break)]),
            }));
            vec![ScgStatement::Control(ControlNode::Loop { body: loop_body, for_range: None, while_cond: None })]
        }

        // for name in start..end { body }
        // Lowered as:
        //   loop { body } with for_range = Some((name, start, end))
        //
        // `lower_loop` emits the counter initialization (before the loop
        // header), the condition check (in the loop header), AND the
        // increment (in the `loop_continue` block, after the continue-phi
        // merge).  Placing the increment in `loop_continue` is critical:
        // a `continue` inside the body jumps to `loop_continue`, so the
        // increment still executes — matching the semantics of a for-loop
        // (where `continue` does NOT skip the increment).
        //
        // The previous lowering wrapped the body in an explicit
        // `if cond { body; name = name + 1 } else { break }` and set
        // `for_range = None`.  That structure placed the increment INSIDE
        // the then-branch, so `continue` skipped it — the loop variable
        // never advanced past the continue point, causing an infinite loop
        // (cf_for_continue, all 10 backends timeout).  The continue-phi
        // merge in `lower_loop` (Task 6-A) ensures the loop-header phi
        // back-edge observes path-merged values, but the increment MUST
        // still run on the continue path for the loop variable to advance
        // — only `for_range` guarantees that.
        PStmt::For(for_stmt) => {
            let mut pre_stmts = Vec::new();
            let (start_expr, end_expr) = match &for_stmt.iter {
                vuma_parser::ast::Expr::Range { start, end, .. } => {
                    let s = flatten_expr(start, &mut pre_stmts, ctx);
                    let e = flatten_expr(end, &mut pre_stmts, ctx);
                    (s, e)
                }
                _other => {
                    (ScgExpr::Int(0), ScgExpr::Int(0))
                }
            };

            let inner_body = bridge_block_to_scg_stmts(&for_stmt.body, ctx);

            let for_range = Some((for_stmt.name.clone(), start_expr, end_expr));

            let mut result = pre_stmts;
            result.push(ScgStatement::Control(ControlNode::Loop {
                body: inner_body,
                for_range,
                while_cond: None,
            }));
            result
        }

        PStmt::Loop(loop_stmt) => {
            let body = bridge_block_to_scg_stmts(&loop_stmt.body, ctx);
            vec![ScgStatement::Control(ControlNode::Loop { body, for_range: None, while_cond: None })]
        }

        PStmt::Break(_) => vec![ScgStatement::Control(ControlNode::Break)],
        PStmt::Continue(_) => vec![ScgStatement::Control(ControlNode::Continue)],

        // ── *ptr;  or  (*ptr).field;  (standalone access / deref read) ──
        // A dereference read with no destination. Lower to a Load into a
        // temporary so the pointer is evaluated AND the load happens
        // (which may trap on a bad pointer — the correct behavior for an
        // explicit dereference). Previously this was silently dropped,
        // which suppressed segfaults that the programmer should observe.
        PStmt::Access(access_stmt) => {
            let mut stmts = Vec::new();
            let ptr_expr = flatten_expr(&access_stmt.expr, &mut stmts, ctx);
            let temp = ctx.alloc_temp();
            // Infer load type from pointer expression: if ptr = base + idx * stride,
            // infer U32 for stride=4, U64 for stride=8. This is critical for
            // big-endian backends where a U8 load of a multi-byte value reads
            // the wrong byte (MSB instead of LSB).
            let load_ty = infer_load_type_from_ptr(&ptr_expr);
            stmts.push(ScgStatement::Access(AccessNode::Load {
                dst: temp,
                ptr: ptr_expr,
                offset: None,
                ty: load_ty,
            }));
            stmts
        }

        // ── match subject { arms } ──
        // Lower to ControlNode::Switch when every arm pattern is a simple
        // integer literal (or the wildcard `_`). For complex patterns
        // (Ident, Struct, Enum, Range, Or), emit a TODO warning and drop
        // ONLY those arm bodies — the wildcard/default arm still runs,
        // which is better than silently dropping the whole match.
        PStmt::Match(match_stmt) => {
            let mut pre_stmts = Vec::new();

            // ── Wave 8b: `match channel_recv(ch) { Ok(v) => ..., Err(e) => ... }` ──
            // First-class lowering of the fallible channel-recv match form.
            // We detect the subject `channel_recv(ch)` (an Expr::Call whose
            // callee is Var("channel_recv") with exactly one argument) and
            // exactly two arms whose patterns are `Ok(v)` and `Err(e)`
            // (MatchPattern::Enum with name "Ok"/"Err" and a binding).
            //
            // This lowers to:
            //   1. ScgStatement::ChannelRecvResult { channel: ch, dst: v, err_dst: e, ty: I64 }
            //      — emits IRInstr::ChannelRecvResult, writing both the
            //        payload vreg (bound to `v`) and the error discriminant
            //        vreg (bound to `e`).
            //   2. ControlNode::If { cond: (e == 0), then_body: ok_body, else_body: Some(err_body) }
            //      — dispatches to the Ok arm when err_dst == 0, else the Err arm.
            //
            // The Ok arm's `v` resolves to the payload vreg; the Err arm's
            // `e` resolves to the ChannelError discriminant (1=Closed,
            // 3=PermissionDenied, 5=CrcMismatch, 6=ProtocolViolation, ...).
            if let Some(crr) = try_match_channel_recv_result(match_stmt, ctx, &mut pre_stmts) {
                // crr carries the two arm bodies + the ch/ok_binding/err_binding.
                // Emit the ChannelRecvResult statement (binds v + e to their vregs).
                pre_stmts.push(ScgStatement::ChannelRecvResult(ChannelRecvResultStmt {
                    channel: crr.channel_expr,
                    dst: crr.ok_binding.clone(),
                    err_dst: crr.err_binding.clone(),
                    ty: ScgType::I64,
                }));
                // Emit the dispatching if/else on err_dst == 0.
                let cond = ScgExpr::BinOp {
                    op: BinOpKind::Eq,
                    lhs: Box::new(ScgExpr::Var(crr.err_binding.clone())),
                    rhs: Box::new(ScgExpr::Int(0)),
                };
                pre_stmts.push(ScgStatement::Control(ControlNode::If {
                    cond,
                    then_body: crr.ok_body,
                    else_body: Some(crr.err_body),
                }));
                return pre_stmts;
            }

            let discriminant = flatten_expr(&match_stmt.subject, &mut pre_stmts, ctx);

            let mut switch_arms: Vec<SwitchArm> = Vec::new();
            let mut default_body: Vec<ScgStatement> = Vec::new();
            let mut saw_complex_pattern = false;

            for arm in &match_stmt.arms {
                match &arm.pattern {
                    vuma_parser::ast::MatchPattern::Lit { value, .. } => {
                        // Only integer-valued literals can become SwitchArm values.
                        let value_i = match value {
                            vuma_parser::ast::Lit::Int(n) => *n,
                            vuma_parser::ast::Lit::Bool(b) => {
                                if *b {
                                    1
                                } else {
                                    0
                                }
                            }
                            vuma_parser::ast::Lit::Address(a) => *a as i64,
                            _ => {
                                // Float/String literal — not a valid switch arm value.
                                saw_complex_pattern = true;
                                continue;
                            }
                        };
                        let mut arm_body: Vec<ScgStatement> = Vec::new();
                        let _ = flatten_expr(&arm.body, &mut arm_body, ctx);
                        switch_arms.push(SwitchArm {
                            value: value_i,
                            body: arm_body,
                        });
                    }
                    vuma_parser::ast::MatchPattern::Wildcard(_) => {
                        let mut arm_body: Vec<ScgStatement> = Vec::new();
                        let _ = flatten_expr(&arm.body, &mut arm_body, ctx);
                        default_body = arm_body;
                    }
                    _ => {
                        // Ident / Struct / Enum / Range / Or — too complex for
                        // the direct AST→codegen bridge. The arm body is
                        // dropped (NOT the whole match).
                        saw_complex_pattern = true;
                    }
                }
            }

            if saw_complex_pattern {
                eprintln!(
                    "[vuma] TODO: match statement at span {:?} uses complex patterns \
                     (ident/struct/enum/range/or) which are not yet supported by the direct \
                     AST→codegen bridge. Only literal-integer and wildcard arms were lowered; \
                     other arm bodies were dropped.",
                    match_stmt.span,
                );
            }

            pre_stmts.push(ScgStatement::Control(ControlNode::Switch {
                discriminant,
                arms: switch_arms,
                default_body,
            }));
            pre_stmts
        }

        // ── sync { body } ──
        // Concurrency primitive. The direct AST→codegen bridge does not
        // enforce sync semantics (no mutex / atomic fence emission); the
        // body is lowered inline so the statements still execute. TODO:
        // implement proper sync-block lowering.
        PStmt::Sync(sync_block) => {
            eprintln!(
                "[vuma] TODO: sync {{ ... }} block at span {:?} lowered without \
                 synchronization semantics (body executes inline, no mutex/fence)",
                sync_block.span,
            );
            bridge_block_to_scg_stmts(&sync_block.body, ctx)
        }

        // ── unsafe { body } ──
        // A scoping marker; lower the body inline. The unsafe contract is
        // the programmer's responsibility — no special handling needed.
        PStmt::UnsafeBlock { body, .. } => {
            bridge_block_to_scg_stmts(body, ctx)
        }

        // BD directives (bd/repd/capd/reld) are annotations consumed by
        // the BD inference pass — they produce no codegen statements.
        PStmt::BdDirective(_) => vec![],

        // PMT (Wave 1a): TransformCall is parsed-but-not-emitted in Wave
        // 1a (the parser produces Stmt::Let with a function-call RHS for
        // transform invocations). Stub: emit no statements so the build
        // does not crash; Wave 1c will lower this properly.
        PStmt::TransformCall(_) => vec![],
    }
}

