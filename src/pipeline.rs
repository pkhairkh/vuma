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
//!     transform main() {
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
// Parallel register allocation across functions.
// Replaced rayon with std::thread::scope (no external dep).
use std::fmt;
use std::path::Path;
use std::time::Instant;

// ── Type aliases for the AST→SCG bridge ──────────────────────────────────
//
// These factor out the two complex types that previously tripped
// `clippy::type_complexity` in `BridgeCtx` and the `build_layout_registry` /
// `resolve_state_field_chain` helpers.

/// Program-wide shared string table. Each entry is `(label, bytes_including_NUL)`.
/// Wrapped in `Rc<RefCell<...>>` so every per-function `BridgeCtx` can append
/// during AST→SCG lowering; the table is drained once at the end and emitted
/// as a single `ScgNode::Data (ReadOnly)` section.
type StringTable = std::rc::Rc<std::cell::RefCell<Vec<(String, Vec<u8>)>>>;

/// A single field of a PMT layout: `(name, ir_type, byte_offset, byte_size,
/// type_name)`. `type_name` is the field's declared type as a string
/// (e.g. `"u32"`, `"Point"`) — used to descend into nested layout-typed
/// fields.
type LayoutField = (String, vuma_codegen::ir::IRType, u64, u64, String);

/// PMT layout registry: layout name → `(total_size, fields)`.
type LayoutRegistry = HashMap<String, (u64, Vec<LayoutField>)>;

// ── Workspace crate imports ──────────────────────────────────────────────

use vuma_bd::{repd::RepD, BD};
use vuma_codegen::{
    emit::{emit_binary, EmitConfig},
    ir::{BinOpKind as IrBinOpKind, IRFunction, IRProgram},
    regalloc::{AllocationResult, LinearScanAllocator},
    scg_to_ir::{
        AccessNode, AllocationNode, CallNode, CastNode, ChannelCloseStmt, ChannelOpenStmt,
        ChannelRecvResultStmt, ChannelRecvStmt, ChannelSendStmt, CodegenEdge, ComputationNode,
        ConstantTimeStatement, ControlNode, EnumAccessNode, ForeignConsumeStmt, GetAddressNode,
        IRBuilder, PmtOpStmt, Scg, ScgData, ScgExpr, ScgFunction, ScgNode, ScgParam, ScgStatement,
        ScgType, StructAccessNode, SwitchArm, SyscallCallNode, UnaryComputationNode,
    },
    CastKind as CodegenCastKind, CodegenError, DataSectionKind,
};
// Escape analysis + effect analysis are wired into the O2+
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
use vuma_core::{
    scg_to_msg::{scg_to_msg, ConversionError},
    MSG,
};
use vuma_ive::{
    AggregatedResult, InvariantAggregator, OverallVerdict,
    VerificationLevel as IveVerificationLevel,
};
use vuma_parser::{
    AstToScg, Item, ModuleResolver, ParseError, Parser, Program as AstProgram, ResolveError,
};
use vuma_scg::{
    AccessMode, CommonSubexpressionElimination, ComputationKind, ConstantFolding, ControlKind,
    DeadCodeElimination, DeadRegionElimination, EdgeData, EdgeKind, InliningPass,
    LoopInvariantCodeMotion, NodeData, NodeId, NodePayload, NodeType, PassManager,
    PipelineResult as ScgPipelineResult, SCGPass, StrengthReduction, TailCallOptDetection, SCG,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum VerificationLevel {
    /// Quick: only cheap syntactic checks.
    Quick,
    /// Normal: all five invariant checks.
    #[default]
    Normal,
    /// Exhaustive: all checks + formal proof attempts + interprocedural.
    Exhaustive,
    /// Modular: core 5 + modular per-function verification.
    Modular,
    /// ConstantTime: core 5 + constant-time (6th invariant).
    ConstantTime,
    /// Hardened: all 6 invariants + interprocedural + modular.
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
    /// Opt OUT of the new Inconclusive-hard-fail default.
    /// When `true`, an `OverallVerdict::Inconclusive` verdict is soft-passed
    /// (compilation continues) with an explicit SOUNDNESS WAIVER logged via
    /// `vuma_log!(warn, ...)` (visible with `VUMA_LOG=1`). Default `false`
    /// — Inconclusive hard-fails. This is the only opt-out escape hatch
    /// for the K3 default-flip migration; users who need the old soft-pass
    /// behaviour pass `--allow-inconclusive`.
    pub allow_inconclusive: bool,
    /// Promote the remaining ADVISORY IVE verifier —
    /// `bv_verify` (e-graph soundness, `pipeline.rs` Stage 7a)
    /// — to HARD-FAIL on any detected violation. The linear-channel
    /// discipline check (`vuma_ive::borrow_region::verify_linear_channels`,
    /// Stage 7c) was previously also gated by this flag, but has been
    /// **promoted to UNCONDITIONAL HARD-FAIL** — see the Stage
    /// 7c call-site comment for the promotion history.
    ///
    /// Default `false`: `bv_verify` runs but only `vuma_log!(warn, ...)`,
    /// matching the pre-existing advisory behaviour. The advisory
    /// status for `bv_verify` is retained as an explicit escape hatch
    /// for users who extend the e-graph rule set with not-yet-verified
    /// rules — the verifier is sound (exhaustive enumeration with
    /// concrete counterexamples) but the original code comment
    /// explicitly reserved promotion to a future strict mode.
    ///
    /// When `--strict-ive` is passed (plumbed through `main.rs::make_config`),
    /// the `bv_verify` gate becomes mandatory: any unsound e-graph
    /// rule returns `VumaError::Transform { pass_name: "bv_verify",
    /// ... }` and aborts compilation. The linear-channel gate is
    /// INDEPENDENT of this flag — it always hard-fails on any
    /// genuine linear-channel violation (use-after-close, double-close,
    /// use-without-open).
    pub strict_ive: bool,
    /// Maximum number of paths explored by the liveness verifier
    /// before giving up (default 64). Higher values catch more bugs at the
    /// cost of slower verification.
    pub ive_max_paths: usize,
    /// Maximum path length explored by the cleanup verifier
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
    /// Cost threshold for the IR-level inliner
    /// (`opt::inline_with_threshold`). A callee whose
    /// `function_inline_cost` (per-instr cost + 2*arg_count -
    /// 3*const_arg_count) is ≤ this threshold gets inlined at the call
    /// site. Default 40 — generous enough to inline small helpers like
    /// `fn add_one(x) { x + 1 }` while preventing runaway code growth.
    pub inline_threshold: u32,
    /// Enable runtime bounds checks for array accesses (--safe flag).
    pub runtime_bounds_checks: bool,
    /// Force section headers in the ELF output (--sections flag).
    pub section_headers: bool,
    /// Maximum expression nesting depth accepted by the
    /// recursive-descent parser before bailing with
    /// "expression nesting depth exceeds maximum (N)". Default
    /// [`vuma_parser::MAX_EXPR_DEPTH`] (1024). Override per-invocation
    /// via the `--max-expr-depth N` CLI flag (see `main.rs::Cli`).
    /// Lower values give faster failure on pathological input;
    /// higher values accommodate machine-generated code (e.g. the
    /// bignum2048 KAT tests in `scripts/womb_kat_tests/`).
    pub max_expr_depth: u32,
    /// Backend / ISA target for the emitted binary.
    ///
    /// Historically the canonical pipeline always emitted AArch64 ELF
    /// (per `EmitConfig::linux_elf()`), and `cmd_emit` / `cmd_build` used
    /// a separate direct AST→codegen path for non-AArch64 targets — which
    /// bypassed the full IVE gate suite (PMT state verifiers, memory-
    /// safety, L1→L3 collapse, session-type, information-flow, syscall
    /// allowlist). Exposing the backend on `CompileConfig` lets
    /// `cmd_emit` route through `compile_with_path` for any ISA while
    /// still running every IVE gate. Defaults to `BackendKind::AArch64`
    /// to preserve the canonical-pipeline behaviour for every existing
    /// caller.
    pub backend: vuma_codegen::backend::BackendKind,
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
                // Honor the per-config backend override so
                // `cmd_emit` (and any other caller that sets
                // `CompileConfig::backend`) gets an ELF for the requested
                // ISA rather than the historical hard-coded AArch64.
                cfg.backend = self.backend;
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
            allow_inconclusive: false,
            strict_ive: false,
            ive_max_paths: 64,
            ive_max_path_length: 256,
            entry_name: "main".to_string(),
            debug_info: false,
            stop_on_first_error: true,
            max_inline_size: 50,
            inline_threshold: vuma_codegen::opt::DEFAULT_INLINE_THRESHOLD,
            runtime_bounds_checks: false,
            section_headers: false,
            max_expr_depth: vuma_parser::MAX_EXPR_DEPTH,
            backend: vuma_codegen::backend::BackendKind::AArch64,
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
    /// Internal panic caught during compilation (crash recovery).
    PanicCaught {
        /// The pipeline stage where the panic occurred.
        stage: String,
        /// The panic message.
        message: String,
    },
    /// Memory-safety analysis failure (blocking pass).
    ///
    /// Emitted when `MemorySafetyAnalyzer` or `analyze_with_scg_liveness`
    /// detects a use-after-free, double-free, memory leak, or uninitialized
    /// read. Memory-safety analysis is MANDATORY in VUMA 2.0 — there is no
    /// opt-out. This is a hard gate: the pipeline refuses to emit code for
    /// programs with known memory-safety violations, independent of
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
            VumaError::ModuleResolution { .. } => "module-resolution",
            VumaError::Multi { .. } => "multi",
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
            VumaError::PanicCaught { stage, message } => {
                write!(f, "[panic-caught] panic in stage '{}': {}", stage, message)
            }
            VumaError::MemorySafety { report } => {
                write!(
                    f,
                    "[memory-safety] {} violation(s) found",
                    report.violations.len()
                )?;
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

/// PMT: map an access_size (in bytes) to the corresponding unsigned
/// IR type. Used by the AST→codegen SCG bridge when lowering
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
        RepD::ManifoldSpatial(_) | RepD::GestaltSuperposition(_) | RepD::ConceptRelational(_) => {
            ScgType::Ptr
        }
        // PMT: a State is a buffer view (passed by reference); a Ref is a
        // pointer-sized offset.
        RepD::State { .. } => ScgType::Ptr,
        RepD::Ref { .. } => ScgType::U64,
        // Dependent state types: a DependentArray is a dynamic
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
        // Channel<T> — opaque IPC handle.  Use "channel" as the
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
// Shared post-IR-build pipeline
// ═══════════════════════════════════════════════════════════════════════════

/// Run the shared post-IR-build O2 pipeline on an `IRProgram`.
///
/// This is the single source of truth for the sequence of passes that run
/// AFTER the SCG→IR build and BEFORE register allocation / emission.  Both
/// [`compile_with_path`] and [`compile_modules`] delegate to it, and the
/// test-suite compile path (`src/bin/compile_dump.rs`) is wired into it so
/// the test suite exercises the FULL production O2 pipeline
/// instead of `run_optimizations` alone (which uses a default latency table
/// and skips lowering / bv_verify / escape+effects).
///
/// The sequence mirrors `compile_with_path`'s historical Stage 8 exactly:
///
/// 1. **Lowering** (O1+): `monomorphize`, `lower_closures`,
///    `lower_switches`, `lower_tail_calls`, `normalize_loops`.  Each pass
///    is best-effort (soft-failures are logged, not fatal).
/// 2. **bv_verify**: verify all e-graph rewrite rules are sound.
///    Advisory only — logs a warning on unsound rules, does NOT abort.
/// 3. **Syscall allowlist**: reject syscall numbers > 600 (hard
///    error — returns `Err(VumaError::Codegen{...})` on the first
///    violation, matching `stop_on_first_error = true`).
/// 4. **Stage 8b codegen-opt** (O1+): `run_optimizations_with_target_and_inline_threshold`
///    with the REAL backend's latency table (built from `backend_kind`).
/// 5. **Escape+effects** (O2+): SROA + alloc elision +
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
/// and stays in `compile_with_path`.  A follow-up will add the codegen-SCG
/// `MemorySafetyAnalyzer` call to `compile_dump` separately if desired.
pub fn run_ir_pipeline(
    mut ir_program: IRProgram,
    config: &CompileConfig,
    backend_kind: vuma_codegen::backend::BackendKind,
    timings: &mut Vec<(String, u64)>,
) -> Result<IRProgram, VumaError> {
    // ── Lowering passes (monomorphize, closures, switch, tail-call,
    //    loop-normalize) — run after SCG→IR build, before the main opt pass.
    //    In VUMA 2.0 these run unconditionally because O3 is mandatory.
    //    Each pass is best-effort: a soft-failure is logged but does not
    //    abort compilation (these are newly-wired passes; the pipeline's
    //    correctness does not yet depend on them).
    {
        let tlower = Instant::now();
        let mut lower =
            |name: &str,
             f: fn(&mut IRProgram) -> Result<(), vuma_codegen::backend::BackendError>| {
                if let Err(e) = f(&mut ir_program) {
                    vuma_log!(warn, "wave34 lowering pass '{}' soft-failed: {}", name, e);
                }
            };
        lower("monomorphize", vuma_codegen::monomorphize::monomorphize);
        lower("lower_closures", vuma_codegen::closures::lower_closures);
        lower("lower_switches", vuma_codegen::control_flow::lower_switches);
        lower(
            "lower_tail_calls",
            vuma_codegen::control_flow::lower_tail_calls,
        );
        lower(
            "normalize_loops",
            vuma_codegen::control_flow::normalize_loops,
        );
        timings.push((
            "wave34-lowering".to_string(),
            tlower.elapsed().as_millis() as u64,
        ));
    }

    // ── IPC Builtin Lowering ──
    // Single IPC codegen path: every backend (including x86_64 and riscv64)
    // routes IPC builtins through `ipc_lowering::lower_ipc_builtins`, which
    // rewrites `Call { func: "channel_send", .. }` into real
    // `Syscall`/`Store`/`Load`/`BinOp` IR with a runtime CRC32 loop (block
    // splitting). The backend instruction selectors no longer recognise IPC
    // builtins by name — the inline Call-form arms in stack_slot_isel.rs and
    // riscv64.rs have been deleted. The `IRInstr::ChannelSend`/etc. arms in
    // each backend's isel stay (they handle the SCG-NodePayload path).
    {
        let tipc = Instant::now();
        for func in &mut ir_program.functions {
            vuma_codegen::ipc_lowering::lower_ipc_builtins(func, backend_kind);
        }
        timings.push((
            "ipc-lowering".to_string(),
            tipc.elapsed().as_millis() as u64,
        ));
    }

    // ── bv_verify gate — verify all e-graph rewrite rules are sound
    //    BEFORE the opt pass (which runs the e-graph). If any rule is unsound,
    //    log a warning so the user knows the e-graph may miscompile.
    //
    //    The gate is advisory by default
    //    (does not abort) to preserve historical behaviour for pre-
    //    existing rule sets. Passing `--strict-ive` (plumbed via
    //    `CompileConfig::strict_ive`) promotes it to a HARD failure:
    //    any unsound rule aborts compilation with `VumaError::Transform`.
    //    The verifier itself is sound — it exhaustively enumerates 256
    //    (1-var) or 65536 (2-var) input combinations per rule and only
    //    reports a rule unsound when it has a concrete counterexample —
    //    so under `--strict-ive` there are no false positives in the
    //    current rule set. The advisory default is retained as an
    //    explicit escape hatch for users who extend the rule set with
    //    not-yet-verified rules.
    {
        let tverify = Instant::now();
        let results = vuma_codegen::bv_verify::verify_all_rules();
        let unsound: Vec<_> = results.iter().filter(|r| !r.sound).collect();
        if !unsound.is_empty() {
            let names: Vec<&'static str> = unsound.iter().map(|r| r.rule_name).collect();
            vuma_log!(warn,
                "wave36 bv_verify: {} unsound e-graph rule(s) detected (compilation may miscompile): {}",
                unsound.len(),
                names.join(", ")
            );
            if config.strict_ive {
                return Err(VumaError::Transform {
                    pass_name: "bv_verify".to_string(),
                    errors: vec![format!(
                        "wave36 bv_verify: {} unsound e-graph rule(s) detected under --strict-ive: {}",
                        unsound.len(),
                        names.join(", ")
                    )],
                });
            }
        }
        timings.push((
            "wave36-bv-verify".to_string(),
            tverify.elapsed().as_millis() as u64,
        ));
    }

    // Syscall allowlist — reject obviously invalid syscall numbers
    // at compile time. Since `nr` is arch-specific, we
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

    // Note: lower_syscalls_all() was removed — real
    // IRInstr::Syscall emission was added directly to all backends, so the IR flows through
    // to codegen unchanged. The generic_syscall_name() table remains in
    // ir.rs as a utility. The lower_syscalls()/lower_syscalls_all()
    // definitions were also deleted as dead-code cleanup.

    // ── IVE formal-verification hooks ──
    // Wire linear_check and l1l3_collapse into the
    // pipeline.  These are advisory checks — they log warnings on
    // violations but do NOT abort compilation (matching the existing
    // bv_verify gate behavior).
    {
        let tive = Instant::now();
        // L1→L3 invariant collapse proof.
        // Scans the IR for runtime-checked invariants (L1: channel framing,
        // CRC, cap, protocol) and reports how many fold into compile-time
        // invariants (L3).
        // MANDATORY: hard-fail when L1 checks cannot be folded
        // to compile-time L3 invariants. The gate is real (it inspects
        // `IRInstr::Call` argument shapes and fires when any L1 check
        // has a non-`Immediate` argument), but the current gold-standard
        // suite does not exercise any L1-checked operation with a
        // non-compile-time argument, so the gate has not fired in CI.
        // Promotion to mandatory is forward-looking: it WILL fire on
        // real violations once such programs appear. See
        // docs/caveats.md §0.7.
        let collapse = vuma_ive::verification::l1l3_collapse_from_ir(&ir_program);
        if collapse.folded_checks > 0 {
            vuma_log!(
                info,
                "[l1l3-collapse]: Wave 96 L1→L3 invariant collapse: {} checks folded (collapsed={}).",
                collapse.folded_checks, collapse.collapsed
            );
        }
        if !collapse.collapsed {
            vuma_log!(
                warn,
                "VERIFICATION FAILURE [l1l3-collapse]: L1→L3 invariant collapse failed ({} runtime checks not all foldable to compile-time).",
                collapse.folded_checks
            );
            return Err(VumaError::Transform {
                pass_name: "l1l3-collapse".to_string(),
                errors: vec![format!(
                    "L1→L3 invariant collapse proof failed: {} runtime-checked invariant(s) could not be folded to compile-time (collapsed=false)",
                    collapse.folded_checks
                )],
            });
        }
        timings.push((
            "ive-verification".to_string(),
            tive.elapsed().as_millis() as u64,
        ));
    }

    // ── Session type + information-flow checks ──
    // Wire session_type_check and information_flow_check
    // into the pipeline.
    // MANDATORY: both checks now hard-fail on any violation.
    // IMPORTANT — structural, not empirical: the IR-level wrappers
    // `verify_session_types_from_ir` and `verify_information_flow_from_ir`
    // currently hardcode their inputs (session_type=`End`, `vreg=0` for
    // every channel op; security labels=`Public` for every flow). By
    // construction these wrappers cannot produce violations, so "zero
    // violations" is structural, NOT empirically validated against the
    // gold-standard suite. The underlying verifiers
    // (`verify_session_types` / `verify_information_flow`) DO real work
    // on real inputs; the gap is in the wrappers' input-shaping, which
    // awaits AST→IR label / session-type annotation propagation
    // (deferred to IVE Wave 2). See docs/caveats.md §0.7 for the
    // per-verifier RESTORE/DEFER decision table.
    {
        let tct = Instant::now();
        // Session type verification — MANDATORY.
        let session_violations = vuma_ive::session_type::verify_session_types_from_ir(&ir_program);
        if !session_violations.is_empty() {
            for v in &session_violations {
                vuma_log!(
                    warn,
                    "VERIFICATION FAILURE [session-type]: Wave 89 session type violation: {}.",
                    v.message
                );
            }
            let err_msgs: Vec<String> = session_violations
                .iter()
                .map(|v| format!("session-type: {}", v.message))
                .collect();
            return Err(VumaError::Transform {
                pass_name: "session-type".to_string(),
                errors: err_msgs,
            });
        }
        // Information-flow verification — MANDATORY.
        let flow_violations =
            vuma_ive::information_flow::verify_information_flow_from_ir(&ir_program);
        if !flow_violations.is_empty() {
            for v in &flow_violations {
                vuma_log!(
                    warn,
                    "VERIFICATION FAILURE [information-flow]: Wave 91 information-flow violation: {}.",
                    v.message
                );
            }
            let err_msgs: Vec<String> = flow_violations
                .iter()
                .map(|v| format!("information-flow: {}", v.message))
                .collect();
            return Err(VumaError::Transform {
                pass_name: "information-flow".to_string(),
                errors: err_msgs,
            });
        }
        timings.push((
            "ct-verification".to_string(),
            tct.elapsed().as_millis() as u64,
        ));
    }

    // ── Stage 8b: Codegen-Level IR Optimization (production caller) ──
    // Use the ACTUAL backend's latency table for per-ISA optimization.
    // The backend is determined from `backend_kind` (passed by the caller —
    // `config.emit_config().backend` for compile_with_path, `host_backend_kind()`
    // for compile_modules). This means the e-graph cost function and scheduler
    // make decisions based on the real target's instruction latencies, not
    // a generic default.
    //
    // In VUMA 2.0, O3 is mandatory — the codegen-opt pass always runs.
    {
        let topt = Instant::now();
        let latency_table = if let Ok(backend) = vuma_codegen::backend::create_backend(backend_kind)
        {
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

    // Escape analysis + SROA + alloc elision + interprocedural
    // effect propagation. Runs AFTER the main codegen-opt pass so the
    // analysis sees the post-optimisation IR (and so SROA's cleanup
    // happens before regalloc).  In VUMA 2.0 O3 is mandatory, so this
    // always runs.
    {
        let te = Instant::now();
        let summary = run_escape_and_effects_passes(&mut ir_program);
        vuma_log!(
            debug,
            "escape+effects: sroa_promoted={} allocs_elided={} pure_fns={}/{}",
            summary.sroa_promoted,
            summary.allocs_elided,
            summary.pure_functions,
            summary.total_functions
        );
        timings.push((
            "escape-effects".to_string(),
            te.elapsed().as_millis() as u64,
        ));
    }

    // Auto-vectorization. Runs AFTER escape/effects (so it sees the
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
        vuma_log!(
            debug,
            "vectorize: loops_vectorized={} slp_packs={}",
            loops_vectorized,
            slp_packs
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
///     transform main() { helper(); }
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
    let ast = match parse_and_resolve(source, file_path, config.max_expr_depth) {
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
    // (Path-sensitivity) Capture the set of node IDs that are the first
    // node of an `else` block BEFORE any SCG transforms run.  The parser
    // labels the Branch → else-block-first-node edge with "else", but
    // later SCG transforms (Stage 7) strip edge labels — so this must be
    // done here, on the freshly-parsed SCG.  The NodeIds are stable
    // across transforms (transforms may add/remove other nodes, but the
    // statement-level Computation/Channel nodes referenced here are
    // preserved), so the set remains valid at Stage 7c.
    use std::collections::HashSet;
    let linear_else_start_node_ids: HashSet<vuma_scg::node::NodeId> = scg
        .edges()
        .filter(|e| e.label.as_deref() == Some("else"))
        .map(|e| e.target)
        .collect();
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
    // NOTE: IVE's InferenceEngine now expects a codegen Scg (Task 6-E);
    // since we hold a semantic SCG here, call the BD engine directly
    // (it is hard-typed to &SCG). The NodeIds correspond 1:1.
    let bd_engine = vuma_bd::inference::BDInferenceEngine::new();
    let bd_results: Vec<(NodeId, BD)> = bd_engine.infer(&scg).bd_map.into_iter().collect();
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
    let verification =
        if !(msg.region_count() == 0 && config.verification_level == VerificationLevel::Quick) {
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
            let secret_vars = collect_secret_vars(&ast);
            // (Task 6-F) Bridge the AST to the codegen Scg ONCE and feed
            // it directly to IVE via `from_codegen_scg` (no semantic-SCG
            // detour). The codegen Scg's `typed_state_meta` is recovered
            // from the same bridge call, so IVE's `verify_pmt` walks the
            // codegen Scg's own `node_payload` adapter and the typed-state
            // conformance cross-check compares the codegen Scg's
            // `node_payload`-derived triples against this meta list.
            // (For pipeline callers where both sides come from the same
            // bridge, the cross-check is a tautology that always passes;
            // it remains functional for callers that manually inject
            // divergent meta — see `tests/scg_conformance.rs`.)
            let (codegen_scg, typed_state_meta) =
                bridge_ast_to_codegen_scg_with_meta(&ast);
            let input = vuma_ive::verification::VerificationInput::from_codegen_scg(codegen_scg)
                .with_pmt_layouts(pmt_layouts)
                .with_secret_vars(secret_vars)
                .with_typed_state_meta(typed_state_meta);
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
            // Inconclusive is now a HARD failure by default.
            // `--allow-inconclusive` (config.allow_inconclusive) opts back into
            // the legacy soft-pass behaviour.
            if (result.overall == OverallVerdict::Inconclusive) && !config.allow_inconclusive {
                errors.push(VumaError::Verification { result });
                return Err(errors);
            }
            // SOUNDNESS WAIVER — only logged when the user
            // has explicitly opted in via `--allow-inconclusive`. The waiver
            // makes the unverified invariants visible in the build log when
            // `VUMA_LOG=1` is set.
            if config.allow_inconclusive && result.overall == OverallVerdict::Inconclusive {
                vuma_log!(
                    warn,
                    "SOUNDNESS WAIVER: IVE verification returned Inconclusive for \
                 the program (passed={}, failed={}, total_checked={}); \
                 compilation continues because --allow-inconclusive was passed. \
                 Inconclusive is now a HARD failure by default.",
                    result.summary.passed,
                    result.summary.failed,
                    result.summary.total_checked
                );
            }
            Some(result)
        } else {
            None
        };
    timings.push((
        "ive-verification".to_string(),
        t.elapsed().as_millis() as u64,
    ));

    // ── Stage 6b: Memory Safety Analysis (blocking pass) ────
    //
    // VUMA 2.0 is PMT-only and memory-safety analysis is MANDATORY.
    // The `--no-memory-safety` CLI flag and `CompileConfig.memory_safety`
    // field have both been removed — there is no opt-out.
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
    let mem_safety_enabled = true; // VUMA 2.0: always on (no opt-out)
    if mem_safety_enabled {
        let t = Instant::now();

        // (2) SCG-liveness-based analysis on the semantic SCG.
        // This runs BEFORE codegen because it uses the semantic SCG
        // (`scg`), not the codegen SCG.
        //
        // Only UAF and uninit-read are treated as HARD errors
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
        let liveness_violations = vuma_codegen::memory_safety::analyze_with_scg_liveness(
            &liveness,
            &scg,
            &ms_config_blocking,
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
        let leak_violations = vuma_codegen::memory_safety::analyze_with_scg_liveness(
            &liveness,
            &scg,
            &ms_config_leaks,
        );
        for lv in &leak_violations {
            vuma_log!(warn, "memory-safety (non-blocking): {}", lv);
        }

        timings.push(("memory-safety".to_string(), t.elapsed().as_millis() as u64));
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
                vuma_log!(
                    warn,
                    "SCG transform soft-failures (non-fatal): {} errors: {:?}",
                    pass_errors.len(),
                    pass_errors.first()
                );
            }
        }
    }
    timings.push(("scg-transforms".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 7b: L1→L3 Invariant Collapse Proof ──────────────────────
    // Calls the real type-consistency verifier on the optimized SCG.
    // Non-fatal: logs the result as info/warn but does not block compilation.
    {
        let t_collapse = Instant::now();
        let collapse = vuma_ive::verification::l1l3_collapse(&scg);
        if collapse.collapsed {
            vuma_log!(
                info,
                "L1->L3 collapse: folded {} L1 checks and {} L2 checks",
                collapse.l1_checks_folded,
                collapse.l2_checks_folded
            );
        } else {
            vuma_log!(warn, "L1->L3 collapse FAILURE: {}", collapse.summary);
        }
        timings.push((
            "l1l3-collapse".to_string(),
            t_collapse.elapsed().as_millis() as u64,
        ));
    }

    // ── Stage 7c: Linear Channel Discipline Check ─────────────────────
    // Verifies that every channel opened is eventually closed (linear
    // discipline), and that channels are not used after close.
    //
    //
    // History: this gate was originally ADVISORY by default (only
    // `vuma_log!(warn)` on violations) because the call site used the
    // SCG node index (`i`) as the channel `vreg` identifier, producing
    // spurious "use of uninitialized channel" / "channel_close on
    // uninitialized" false positives on any program with more than one
    // channel operation (every operation had a distinct `vreg` so the
    // per-handle state map never correlated them). Under `--strict-ive`
    // (`config.strict_ive`), the gate was promoted to HARD-FAIL — but
    // the false positive meant channel-using programs would fail under
    // `--strict-ive` even when semantically valid.
    //
    // The false positive was fixed by extracting the channel
    // handle's variable name from the relevant NodePayload variants
    // (`ChannelOpenNode.dst` / `ChannelSendNode.channel` /
    // `ChannelRecvNode.channel` / `ChannelCloseNode.channel`) and
    // keying the verifier on that name (see
    // `vuma_ive::borrow_region::ChannelEvent::vreg` for the design).
    // With the FP eliminated, the verifier now has clean semantics —
    // multiple operations on the SAME channel handle correctly share
    // a state-map entry, and the verifier stays silent on legitimate
    // multi-op programs.
    //
    // The gate is now UNCONDITIONAL HARD-FAIL: any
    // linear-channel violation returns `VumaError::Transform` and
    // aborts compilation, regardless of the `--strict-ive` flag. The
    // false-positive fix made this safe — there are no known sources
    // of spurious violations in the current call site. The
    // `--strict-ive` flag is RETAINED only for `bv_verify` (Stage 7a),
    // which still has the "reserved for future strict mode" advisory
    // status.
    {
        let t_linear = Instant::now();
        let mut events: Vec<vuma_ive::borrow_region::ChannelEvent> = Vec::new();

        // (Path-sensitivity) `linear_else_start_node_ids` was captured
        // right after `ast_to_scg` (Stage 2), BEFORE SCG transforms
        // stripped the "else" edge labels.  See the comment at its
        // declaration for the rationale.
        let else_start_node_ids = &linear_else_start_node_ids;

        for (i, node) in scg.nodes().enumerate() {
            // Extract the channel handle's VARIABLE NAME
            // from the NodePayload — NOT the SCG node index `i`. Using
            // `i as u32` would make every channel operation appear as
            // a distinct handle and produce false positives on any
            // program with >1 channel operation. See
            // `vuma_ive::borrow_region::ChannelEvent::vreg` doc.

            // (Path-sensitivity) If this node is the first node of an
            // else block, emit an `ElseStart` event BEFORE the node's
            // own event, so the verifier restores the pre-branch
            // snapshot before processing the else-branch's channel ops.
            if else_start_node_ids.contains(&node.id) {
                events.push(vuma_ive::borrow_region::ChannelEvent {
                    vreg: String::new(),
                    kind: vuma_ive::borrow_region::ChannelEventKind::ElseStart,
                    at_node: i,
                });
            }

            match &node.payload {
                vuma_scg::node::NodePayload::ChannelOpen(p) => {
                    events.push(vuma_ive::borrow_region::ChannelEvent {
                        vreg: p.dst.clone(),
                        kind: vuma_ive::borrow_region::ChannelEventKind::Open,
                        at_node: i,
                    });
                }
                vuma_scg::node::NodePayload::ChannelSend(p) => {
                    events.push(vuma_ive::borrow_region::ChannelEvent {
                        vreg: p.channel.clone(),
                        kind: vuma_ive::borrow_region::ChannelEventKind::Use,
                        at_node: i,
                    });
                }
                vuma_scg::node::NodePayload::ChannelRecv(p) => {
                    events.push(vuma_ive::borrow_region::ChannelEvent {
                        vreg: p.channel.clone(),
                        kind: vuma_ive::borrow_region::ChannelEventKind::Use,
                        at_node: i,
                    });
                }
                vuma_scg::node::NodePayload::ChannelClose(p) => {
                    events.push(vuma_ive::borrow_region::ChannelEvent {
                        vreg: p.channel.clone(),
                        kind: vuma_ive::borrow_region::ChannelEventKind::Close,
                        at_node: i,
                    });
                }
                vuma_scg::node::NodePayload::Control(c) => {
                    // (Path-sensitivity) Emit structural events for
                    // control-flow nodes so the verifier can be
                    // path-sensitive across `if`/`else` boundaries and
                    // detect leaks at function-exit points.
                    use vuma_scg::node::ControlKind;
                    match c.kind {
                        ControlKind::Branch => {
                            events.push(vuma_ive::borrow_region::ChannelEvent {
                                vreg: String::new(),
                                kind: vuma_ive::borrow_region::ChannelEventKind::Branch,
                                at_node: i,
                            });
                        }
                        ControlKind::Join => {
                            events.push(vuma_ive::borrow_region::ChannelEvent {
                                vreg: String::new(),
                                kind: vuma_ive::borrow_region::ChannelEventKind::Join,
                                at_node: i,
                            });
                        }
                        ControlKind::FunctionReturn => {
                            // Only emit `FunctionExit` at REAL function-exit
                            // points — explicit `return` statements (label
                            // "return") and the function epilogue (label
                            // "fn_<name>_return(...)").  Call-site return
                            // nodes (label "return_<callee>") and
                            // monomorphisation / dispatch return nodes
                            // (labels "mono_*_return", "static_dispatch_return *")
                            // are NOT function exits — they return from a
                            // callee back to the caller, not from the
                            // current function.
                            let is_fn_exit = c.label.as_deref() == Some("return")
                                || c
                                    .label
                                    .as_ref()
                                    .map_or(false, |l| l.starts_with("fn_"));
                            if is_fn_exit {
                                events.push(vuma_ive::borrow_region::ChannelEvent {
                                    vreg: String::new(),
                                    kind: vuma_ive::borrow_region::ChannelEventKind::FunctionExit,
                                    at_node: i,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        let results = vuma_ive::borrow_region::verify_linear_channels(&events);
        if !vuma_ive::borrow_region::all_linear_valid(&results) {
            let mut violation_msgs: Vec<String> = Vec::new();
            for r in &results {
                if !r.valid {
                    let msg = r.error.as_deref().unwrap_or("unknown").to_string();
                    vuma_log!(warn, "Linear channel violation: {}", msg);
                    violation_msgs.push(msg);
                }
            }
            // UNCONDITIONAL HARD-FAIL: any linear-channel
            // violation returns `VumaError::Transform` and aborts
            // compilation, regardless of `--strict-ive`. The earlier
            // false-positive fix eliminated the only known source of
            // spurious violations, so the gate is now safe to enforce
            // by default. `--strict-ive` is retained only for
            // `bv_verify` (Stage 7a) which still has the "reserved for
            // future strict mode" advisory status.
            if !violation_msgs.is_empty() {
                timings.push((
                    "linear-check".to_string(),
                    t_linear.elapsed().as_millis() as u64,
                ));
                errors.push(VumaError::Transform {
                    pass_name: "linear-channel".to_string(),
                    errors: violation_msgs,
                });
                return Err(errors);
            }
        }
        timings.push((
            "linear-check".to_string(),
            t_linear.elapsed().as_millis() as u64,
        ));
    }

    // ── Stage 8: IR Lowering ──────────────────────────────────────────
    let t = Instant::now();
    // NOTE: The canonical pipeline now uses the DIRECT AST→codegen SCG
    // bridge (`bridge_ast_to_codegen_scg`) instead of the semantic-SCG →
    // codegen-SCG bridge. The semantic SCG (`scg`) is still built and used
    // for BD inference / MSG / IVE verification / SCG transforms above,
    // but the emitted binary is produced from the AST directly. This avoids
    // the segfaults / infinite loops that the old `bridge_scg_to_codegen*`
    // path produced.
    let (mut codegen_scg, _typed_state_meta) = bridge_ast_to_codegen_scg_with_meta(&ast);

    // Stage 2 / checkbounds: Build a `var_name → allocation_size`
    // table from the codegen SCG's `AllocationNode::Stack` statements.
    // For state-typed buffers, `AllocationNode::Stack.size` already holds
    // the layout's `total_size` (embedded at AST→SCG time by the parser's
    // `layout_total_size`), so a single pass over all function bodies
    // captures both stack arrays and PMT state buffers. Heap allocations
    // (size_expr) are not statically known and are skipped — they remain
    // `length_expr = None`.
    let mut alloc_sizes = build_alloc_sizes(&codegen_scg);

    // Arena-allocated state pointers: also collect per-state layout sizes.
    // `arena_alloc(arena, Layout)` lowers
    // to a `state_ptr = arena_ptr + offset` computation (a fresh temp not
    // present in `alloc_sizes` from `AllocationNode::Stack`), so without
    // this merge `classify_pointer` would treat every arena-state access
    // as `PointerKind::Wild` and skip the per-access `__oob_trap` check.
    //
    // `build_arena_state_sizes` pattern-matches the deterministic
    // `arena_alloc` IR sequence (anchored on `__arena_overflow` calls,
    // uniquely emitted by `Expr::ArenaAlloc` lowering at pipeline.rs:10479)
    // to recover `state_ptr_name → layout_size` pairs. Merging into
    // `alloc_sizes` makes those accesses classify as `Seq` and receive the
    // standard `__oob_trap` bounds-check pair from `inject_bounds_check_ir`.
    //
    // Bound semantics: the bound is the state's `layout_size` (not the
    // arena's capacity). The arena's capacity is already checked at
    // `arena_alloc` time via `__arena_overflow`; the per-access check
    // catches out-of-layout field accesses through `state_ptr`.
    let arena_state_sizes = vuma_codegen::memory_safety::build_arena_state_sizes(&codegen_scg);
    if !arena_state_sizes.is_empty() {
        vuma_log!(
            info,
            "Arena state sizes: recovered {} arena_alloc state_ptr → layout_size entries \
             (merging into alloc_sizes for per-access __oob_trap injection).",
            arena_state_sizes.len()
        );
        for (k, v) in &arena_state_sizes {
            // Last-writer-wins: arena state pointers are fresh temps, so
            // collisions with `AllocationNode::Stack` entries are not
            // expected in practice.
            alloc_sizes.insert(k.clone(), *v);
        }
    }

    // Run the codegen-level MemorySafetyAnalyzer on the codegen
    // SCG.  This complements the SCG-liveness analysis (Stage 6b) with
    // function-level double-free and dangling-pointer detection.  Like
    // Stage 6b, this is a HARD gate (VUMA 2.0: memory safety is mandatory;
    // there is no opt-out).
    if mem_safety_enabled {
        let ms_config = if config.runtime_bounds_checks {
            vuma_codegen::memory_safety::MemorySafetyConfig::safe_mode()
        } else {
            vuma_codegen::memory_safety::MemorySafetyConfig::compile_time_only()
        };
        let analyzer = vuma_codegen::memory_safety::MemorySafetyAnalyzer::new(ms_config);
        let ms_report = analyzer.analyze(&codegen_scg);
        if !ms_report.is_clean() {
            errors.push(VumaError::MemorySafety { report: ms_report });
            return Err(errors);
        }

        // checkbounds (Stage 2): `length_expr` is now populated
        // by `find_bounds_check_sites_with_bounds` using the `alloc_sizes`
        // table built from `AllocationNode::Stack` statements. When
        // `--safe` is set, `inject_bounds_check_ir` mutates the codegen
        // SCG to insert `__oob_trap` traps before every bounded access,
        // mirroring the proven `__arena_overflow` lowering pattern. The
        // `__oob_trap` stubs (exit 134) exist on all 19 backends.
        //
        // Honest status:
        //  * Stack allocations (incl. PMT state buffers): fully bounded
        //    by `AllocationNode::Stack.size` (= layout `total_size`).
        //  * Arena-allocated state buffers: now
        //    ALSO bounded per-access. `build_arena_state_sizes` pattern-
        //    matches the `arena_alloc` IR sequence (anchored on
        //    `__arena_overflow` calls) to recover `state_ptr → layout_size`
        //    pairs, which are merged into `alloc_sizes` above. Per-access
        //    `__oob_trap` checks are now emitted for `state_ptr + offset`.
        //    The arena's capacity is still checked at `arena_alloc` time
        //    via `__arena_overflow` (alloc-time guard, unchanged).
        //  * Raw pointer arithmetic / extern pointers: `length_expr`
        //    remains `None` — Stage 3 (SoftBound fat pointers) territory.
        if config.runtime_bounds_checks {
            let sites = vuma_codegen::memory_safety::find_bounds_check_sites_with_bounds(
                &codegen_scg,
                &alloc_sizes,
            );
            let bounded = sites.iter().filter(|s| s.length_expr.is_some()).count();
            if !sites.is_empty() {
                vuma_log!(
                    info,
                    "Bounds check sites found ({} total, {} with static bound). \
                     Emitting __oob_trap IR for bounded accesses.",
                    sites.len(),
                    bounded
                );
                for site in &sites {
                    vuma_log!(
                        debug,
                        "  BoundsCheckSite: fn={} array={} index={} length={}",
                        site.function_name,
                        site.array_name,
                        site.index_expr,
                        site.length_expr.as_deref().unwrap_or("<unknown>")
                    );
                }
            } else {
                vuma_log!(
                    debug,
                    "No array/pointer bounds-check sites found in codegen SCG \
                     (arena overflow check still applies to ArenaAlloc)."
                );
            }

            // Mutate the codegen SCG in place: prepend a
            // `ComputationNode(UGe)` + `ControlNode::If { __oob_trap }`
            // pair before every Access whose `ptr` resolves to a name in
            // `alloc_sizes`. Accesses with no static bound are skipped.
            vuma_codegen::memory_safety::inject_bounds_check_ir(&mut codegen_scg, &alloc_sizes);

            // Liveness (UAF tombstone checks): also inject runtime UAF (tombstone) checks
            // before every SEQ access through a `state_new` allocation.
            // Each such allocation is grown by +1 byte at AST→SCG bridge
            // time to hold a LIVE/DEAD flag at `[ptr + total_size]`;
            // `inject_liveness_check_ir` emits a Load + Eq + If that traps
            // via `__uaf_trap` (exit 135) when the flag is 0 (DEAD). For
            // live states the check is a no-op (flag == 1 → eq 0 → false).
            vuma_codegen::memory_safety::inject_liveness_check_ir(&mut codegen_scg, &alloc_sizes);
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
    // The lowering passes (monomorphize,
    // closures, switches, tail-calls, loop-normalize), bv_verify,
    // the syscall allowlist, Stage 8b codegen-opt (with the real
    // backend's latency table), and escape+effects passes are now
    // encapsulated in `run_ir_pipeline` — the single source of truth
    // shared with `compile_modules` and the test-suite
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
    // Each function's register allocation is independent, so we
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

// ═══════════════════════════════════════════════════════════════════════════
// Multi-module compilation
// ═══════════════════════════════════════════════════════════════════════════

/// Merge a slice of independently-parsed `AstProgram`s into a single
/// `AstProgram`, resolving cross-module `extern "C"` declarations against
/// real `fn` definitions.
///
/// Algorithm:
/// 1. **Duplicate-fn deduplication** — collect every `fn`
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
    // after the first occurrences are kept). The original policy
    // rejected every duplicate as a hard error, blocking the bootstrap
    // self-host test (`test_bootstrap_impl_self_host`).
    //
    // The current dedup-or-conflict policy replaces that hard-reject:
    // identical duplicates are silently dropped (the bootstrap
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
                        merged_items
                            .retain(|item| !matches!(item, Item::FnDef(f) if f.name == "main"));
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
/// equality.
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
/// numbers — see the site map in `test_bootstrap_impl_self_host`'s
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
    // Span-agnostic comparison via Debug-string normalization.
    //
    // Previously this function used serde_json::to_value +
    // strip_spans. That approach was removed when Serialize/Deserialize derives
    // were stripped from the parser's AST types.
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
///   pattern), the later occurrences are silently dropped;
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
/// and `debug_info` fields are `None` / empty because the
/// direct path does not run the canonical pipeline's MSG-construction,
/// IVE-verification, or COR-initialization stages.
///
/// # Example
///
/// ```rust,ignore
/// use vuma::pipeline::{compile_modules, CompileConfig};
///
/// let modules: Vec<(String, String)> = vec![
///     ("main.vuma".into(), "extern \"C\" { fn helper(); } transform main() { helper(); }".into()),
///     ("helper.vuma".into(), "transform helper() { }".into()),
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
        let mut parser = Parser::with_max_depth(source, config.max_expr_depth);
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
    // refuse to emit a binary on `Fail` (hard gate).
    // `Inconclusive` is now a HARD failure by default — only
    // `--allow-inconclusive` (config.allow_inconclusive) opts back into
    // soft-pass with a logged SOUNDNESS WAIVER. The legacy
    // `--strict-verification` flag was REMOVED in VUMA 2.0.
    let t = Instant::now();
    let pmt_scg = match ast_to_scg(&merged_ast) {
        Ok(s) => s,
        Err(e) => {
            errors.push(VumaError::AstToScg {
                message: format!("{}", e),
            });
            return Err(errors);
        }
    };
    let pmt_layouts = build_pmt_layout_specs(&merged_ast);
    let secret_vars = collect_secret_vars(&merged_ast);
    // (Task 6-F) Bridge the merged AST to the codegen Scg ONCE and feed
    // it directly to IVE via `from_codegen_scg`. The codegen Scg's
    // `typed_state_meta` is recovered from the same bridge call. IVE's
    // `verify_pmt` now walks the codegen Scg's `node_payload` adapter
    // (Task 6-A), so the typed-state conformance cross-check compares the
    // codegen Scg's adapter-derived triples against this meta list — a
    // tautology for pipeline callers (both sides from the same bridge)
    // that always passes, but remains functional for divergent-meta
    // injection (see `tests/scg_conformance.rs`).
    let (codegen_scg, typed_state_meta) =
        bridge_ast_to_codegen_scg_with_meta(&merged_ast);
    let aggregator = InvariantAggregator::new()
        .with_level(IveVerificationLevel::Pmt)
        .with_max_paths(config.ive_max_paths)
        .with_max_path_length(config.ive_max_path_length);
    let ive_input = vuma_ive::verification::VerificationInput::from_codegen_scg(codegen_scg)
        .with_pmt_layouts(pmt_layouts)
        .with_secret_vars(secret_vars)
        .with_typed_state_meta(typed_state_meta);
    let verification = aggregator.verify_all(&ive_input);
    timings.push((
        "ive-verification".to_string(),
        t.elapsed().as_millis() as u64,
    ));
    if verification.overall == OverallVerdict::Fail {
        errors.push(VumaError::Verification {
            result: verification,
        });
        return Err(errors);
    }
    // Inconclusive is now a HARD failure by default in
    // `compile_modules` too. `--allow-inconclusive` opts back into soft-pass.
    if (verification.overall == OverallVerdict::Inconclusive) && !config.allow_inconclusive {
        errors.push(VumaError::Verification {
            result: verification,
        });
        return Err(errors);
    }
    // SOUNDNESS WAIVER — only logged when the user has
    // explicitly opted in via `--allow-inconclusive`. See pipeline.rs:5186
    // for full rationale.
    if config.allow_inconclusive && verification.overall == OverallVerdict::Inconclusive {
        vuma_log!(
            warn,
            "SOUNDNESS WAIVER: IVE verification returned Inconclusive for \
             compile_modules (passed={}, failed={}, total_checked={}); \
             compilation continues because --allow-inconclusive was passed. \
             Inconclusive is now a HARD failure by default.",
            verification.summary.passed,
            verification.summary.failed,
            verification.summary.total_checked
        );
    }
    // Hold onto the verification result so it can be surfaced in the
    // final `CompilationOutput`. The semantic SCG built above is also
    // reused as the `CompilationOutput.scg` (Stage 7 below) instead of
    // rebuilding it best-effort.
    let verification = Some(verification);
    let scg = pmt_scg;

    // ── Stage 3: Bridge merged AST → codegen SCG ──────────────────────
    let t = Instant::now();
    let (codegen_scg, _typed_state_meta) = bridge_ast_to_codegen_scg_with_meta(&merged_ast);
    timings.push((
        "ast-to-codegen-scg".to_string(),
        t.elapsed().as_millis() as u64,
    ));

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
    // compile_modules now delegates to the shared
    // `run_ir_pipeline` helper — the same single source of truth used by
    // `compile_with_path` and the test-suite compile path.
    // This means compile_modules now runs the FULL O2 pipeline (lowering,
    // bv_verify, syscall allowlist, Stage 8b
    // codegen-opt, escape+effects) instead of just codegen-opt,
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
    //
    // F64 is included because the wasm32 backend stores 8-byte returns
    // (whether i64 or f64) via i64.store at mem[0]; the caller loads via
    // i64.load and bitcasts to F64 when consumed as a float.  Without F64
    // here, f64-returning functions would have their return value
    // truncated to the low 32 bits via i32.store/i32.load (e.g. newton_sqrt
    // returns f64 31.0 = 0x403F000000000000; i32.store would only save the
    // low 32 bits = 0, then floattoint would yield 0 instead of 31).
    {
        let func_64bit: HashSet<String> = ir_program
            .functions
            .iter()
            .filter(|f| {
                f.result_types.iter().any(|t| {
                    matches!(
                        t,
                        vuma_codegen::ir::IRType::I64
                            | vuma_codegen::ir::IRType::U64
                            | vuma_codegen::ir::IRType::F64
                    )
                })
            })
            .map(|f| f.name.clone())
            .collect();
        vuma_codegen::backend::set_64bit_returns(&func_64bit);
    }

    // K10G-wasm32-newton: populate the global function-name → param-IRTypes
    // map so the wasm32 backend's IRInstr::Call handler can push each call
    // argument with the correct wasm type (F64 vs I32). Without this,
    // `newton_sqrt(n_f, n_f, 8)` pushes all three args as I32, failing
    // wasmtime validation ("expected f64, found i32") because newton_sqrt's
    // wasm signature is (f64, f64, i32) -> ().
    {
        let func_params: std::collections::HashMap<String, Vec<vuma_codegen::ir::IRType>> =
            ir_program
                .functions
                .iter()
                .map(|f| (f.name.clone(), f.param_types.clone()))
                .collect();
        vuma_codegen::backend::set_func_param_types(&func_params);
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

    // Central pre-lowering float-op verification.
    //
    // Reject bitwise/shift/remainder ops (`And`/`Or`/`Xor`/`Shl`/`ShrL`/
    // `ShrA`/`Ror`/`Rol`/`SRem`/`URem`) on `F32`/`F64` operands BEFORE
    // any backend's `allocate_registers` runs, so all 19 backends
    // (including the 4 thin wrappers) benefit without per-backend
    // wiring.  The previous AArch64-only call site in
    // `AArch64Backend::allocate_registers` has been removed — this
    // central call subsumes it.  See `verify_program_float_ops` in
    // `codegen/src/backend.rs` for the full rationale.
    if let Err(errs) = vuma_codegen::backend::verify_program_float_ops(&ir_program) {
        errors.push(VumaError::Codegen {
            error: CodegenError::InvalidInstruction(format!(
                "compile_modules: pre-lowering float-op verification failed: {}",
                errs.join("; ")
            )),
        });
        return Err(errors);
    }

    let mut allocated_functions = Vec::new();
    for func in &ir_program.functions {
        match backend.allocate_registers(func) {
            Ok(allocated) => allocated_functions.push(allocated),
            Err(e) => {
                vuma_log!(
                    warn,
                    "compile_modules: register allocation failed for '{}': {}",
                    func.name,
                    e
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
        rodata_data: Vec::new(),
        function_names: std::collections::HashSet::new(),
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
/// let source = "transform main() {}";
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
        parse_and_resolve(source, file_path, config.max_expr_depth),
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
    // (Path-sensitivity) Capture else-start node IDs BEFORE SCG transforms
    // strip the "else" edge labels.  See the comment in `compile_with_path`
    // (Stage 2) for the full rationale.
    use std::collections::HashSet;
    let linear_else_start_node_ids: HashSet<vuma_scg::node::NodeId> = scg
        .edges()
        .filter(|e| e.label.as_deref() == Some("else"))
        .map(|e| e.target)
        .collect();
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
    // NOTE: IVE's InferenceEngine now expects a codegen Scg (Task 6-E);
    // since we hold a semantic SCG here, call the BD engine directly
    // (it is hard-typed to &SCG). The NodeIds correspond 1:1.
    let bd_engine = vuma_bd::inference::BDInferenceEngine::new();
    let bd_results: Vec<(NodeId, BD)> = bd_engine.infer(&scg).bd_map.into_iter().collect();
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
    let verification =
        if !(msg.region_count() == 0 && config.verification_level == VerificationLevel::Quick) {
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
            let secret_vars = collect_secret_vars(&ast);
            // (Task 6-F) Bridge the AST to the codegen Scg and feed it
            // directly to IVE via `from_codegen_scg`. No typed-state meta
            // is attached at this site (the typed-state conformance
            // cross-check is skipped when `typed_state_meta` is empty).
            let codegen_scg = bridge_ast_to_codegen_scg(&ast);
            let input = vuma_ive::verification::VerificationInput::from_codegen_scg(codegen_scg)
                .with_pmt_layouts(pmt_layouts)
                .with_secret_vars(secret_vars);
            let result = aggregator.verify_all(&input);
            // Verification is a hard safety gate: if any invariant was
            // violated, refuse to emit code for the program.  This is
            // independent of `stop_on_first_error` because emitting a binary
            // for a program with known memory-safety violations would defeat
            // the entire purpose of VUMA.
            if result.overall == OverallVerdict::Fail {
                errors.push(VumaError::Verification {
                    result: result.clone(),
                });
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
            // Inconclusive is now a HARD failure by default
            // in the partial-compile (recovery) path too. `--allow-inconclusive`
            // opts back into soft-pass.
            if (result.overall == OverallVerdict::Inconclusive) && !config.allow_inconclusive {
                errors.push(VumaError::Verification {
                    result: result.clone(),
                });
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
            // SOUNDNESS WAIVER — only logged when the user
            // has explicitly opted in via `--allow-inconclusive` in the
            // partial-compile (recovery) path — see pipeline.rs:5186 for full
            // rationale.
            if config.allow_inconclusive && result.overall == OverallVerdict::Inconclusive {
                vuma_log!(
                    warn,
                    "SOUNDNESS WAIVER: IVE verification returned Inconclusive for \
                 the program (partial-compile path; passed={}, failed={}, \
                 total_checked={}); compilation continues because \
                 --allow-inconclusive was passed. Inconclusive is now a \
                 HARD failure by default.",
                    result.summary.passed,
                    result.summary.failed,
                    result.summary.total_checked
                );
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

    // ── Stage 7b: L1→L3 Invariant Collapse Proof ──────────────────────
    // Calls the real type-consistency verifier on the optimized SCG.
    // Non-fatal: logs the result as info/warn but does not block compilation.
    {
        let t_collapse = Instant::now();
        let collapse = vuma_ive::verification::l1l3_collapse(&scg);
        if collapse.collapsed {
            vuma_log!(
                info,
                "L1->L3 collapse: folded {} L1 checks and {} L2 checks",
                collapse.l1_checks_folded,
                collapse.l2_checks_folded
            );
        } else {
            vuma_log!(warn, "L1->L3 collapse FAILURE: {}", collapse.summary);
        }
        timings.push((
            "l1l3-collapse".to_string(),
            t_collapse.elapsed().as_millis() as u64,
        ));
    }

    // ── Stage 7c: Linear Channel Discipline Check ─────────────────────
    // Verifies that every channel opened is eventually closed (linear
    // discipline), and that channels are not used after close.
    //
    //
    // History: this gate was originally ADVISORY by default (only
    // `vuma_log!(warn)` on violations) because the call site used the
    // SCG node index (`i`) as the channel `vreg` identifier, producing
    // spurious "use of uninitialized channel" / "channel_close on
    // uninitialized" false positives on any program with more than one
    // channel operation (every operation had a distinct `vreg` so the
    // per-handle state map never correlated them). Under `--strict-ive`
    // (`config.strict_ive`), the gate was promoted to HARD-FAIL — but
    // the false positive meant channel-using programs would fail under
    // `--strict-ive` even when semantically valid.
    //
    // The false positive was fixed by extracting the channel
    // handle's variable name from the relevant NodePayload variants
    // (`ChannelOpenNode.dst` / `ChannelSendNode.channel` /
    // `ChannelRecvNode.channel` / `ChannelCloseNode.channel`) and
    // keying the verifier on that name (see
    // `vuma_ive::borrow_region::ChannelEvent::vreg` for the design).
    // With the FP eliminated, the verifier now has clean semantics —
    // multiple operations on the SAME channel handle correctly share
    // a state-map entry, and the verifier stays silent on legitimate
    // multi-op programs.
    //
    // The gate is now UNCONDITIONAL HARD-FAIL: any
    // linear-channel violation returns `VumaError::Transform` and
    // aborts compilation (returning `CompileResult::Partial` on this
    // recovery path), regardless of the `--strict-ive` flag. The
    // false-positive fix made this safe — there are no known sources
    // of spurious violations in the current call site. The
    // `--strict-ive` flag is RETAINED only for `bv_verify` (Stage 7a),
    // which still has the "reserved for future strict mode" advisory
    // status.
    {
        let t_linear = Instant::now();
        let mut events: Vec<vuma_ive::borrow_region::ChannelEvent> = Vec::new();

        // (Path-sensitivity) `linear_else_start_node_ids` was captured
        // right after `ast_to_scg` (Stage 2), BEFORE SCG transforms
        // stripped the "else" edge labels.  See the comment at its
        // declaration for the rationale.
        let else_start_node_ids = &linear_else_start_node_ids;

        for (i, node) in scg.nodes().enumerate() {
            // Extract the channel handle's VARIABLE NAME
            // from the NodePayload — NOT the SCG node index `i`. Using
            // `i as u32` would make every channel operation appear as
            // a distinct handle and produce false positives on any
            // program with >1 channel operation. See
            // `vuma_ive::borrow_region::ChannelEvent::vreg` doc.

            // (Path-sensitivity) If this node is the first node of an
            // else block, emit an `ElseStart` event BEFORE the node's
            // own event, so the verifier restores the pre-branch
            // snapshot before processing the else-branch's channel ops.
            if else_start_node_ids.contains(&node.id) {
                events.push(vuma_ive::borrow_region::ChannelEvent {
                    vreg: String::new(),
                    kind: vuma_ive::borrow_region::ChannelEventKind::ElseStart,
                    at_node: i,
                });
            }

            match &node.payload {
                vuma_scg::node::NodePayload::ChannelOpen(p) => {
                    events.push(vuma_ive::borrow_region::ChannelEvent {
                        vreg: p.dst.clone(),
                        kind: vuma_ive::borrow_region::ChannelEventKind::Open,
                        at_node: i,
                    });
                }
                vuma_scg::node::NodePayload::ChannelSend(p) => {
                    events.push(vuma_ive::borrow_region::ChannelEvent {
                        vreg: p.channel.clone(),
                        kind: vuma_ive::borrow_region::ChannelEventKind::Use,
                        at_node: i,
                    });
                }
                vuma_scg::node::NodePayload::ChannelRecv(p) => {
                    events.push(vuma_ive::borrow_region::ChannelEvent {
                        vreg: p.channel.clone(),
                        kind: vuma_ive::borrow_region::ChannelEventKind::Use,
                        at_node: i,
                    });
                }
                vuma_scg::node::NodePayload::ChannelClose(p) => {
                    events.push(vuma_ive::borrow_region::ChannelEvent {
                        vreg: p.channel.clone(),
                        kind: vuma_ive::borrow_region::ChannelEventKind::Close,
                        at_node: i,
                    });
                }
                vuma_scg::node::NodePayload::Control(c) => {
                    // (Path-sensitivity) Emit structural events for
                    // control-flow nodes so the verifier can be
                    // path-sensitive across `if`/`else` boundaries and
                    // detect leaks at function-exit points.
                    use vuma_scg::node::ControlKind;
                    match c.kind {
                        ControlKind::Branch => {
                            events.push(vuma_ive::borrow_region::ChannelEvent {
                                vreg: String::new(),
                                kind: vuma_ive::borrow_region::ChannelEventKind::Branch,
                                at_node: i,
                            });
                        }
                        ControlKind::Join => {
                            events.push(vuma_ive::borrow_region::ChannelEvent {
                                vreg: String::new(),
                                kind: vuma_ive::borrow_region::ChannelEventKind::Join,
                                at_node: i,
                            });
                        }
                        ControlKind::FunctionReturn => {
                            // Only emit `FunctionExit` at REAL function-exit
                            // points — explicit `return` statements (label
                            // "return") and the function epilogue (label
                            // "fn_<name>_return(...)").  Call-site return
                            // nodes (label "return_<callee>") and
                            // monomorphisation / dispatch return nodes
                            // (labels "mono_*_return", "static_dispatch_return *")
                            // are NOT function exits — they return from a
                            // callee back to the caller, not from the
                            // current function.
                            let is_fn_exit = c.label.as_deref() == Some("return")
                                || c
                                    .label
                                    .as_ref()
                                    .map_or(false, |l| l.starts_with("fn_"));
                            if is_fn_exit {
                                events.push(vuma_ive::borrow_region::ChannelEvent {
                                    vreg: String::new(),
                                    kind: vuma_ive::borrow_region::ChannelEventKind::FunctionExit,
                                    at_node: i,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        let results = vuma_ive::borrow_region::verify_linear_channels(&events);
        if !vuma_ive::borrow_region::all_linear_valid(&results) {
            let mut violation_msgs: Vec<String> = Vec::new();
            for r in &results {
                if !r.valid {
                    let msg = r.error.as_deref().unwrap_or("unknown").to_string();
                    vuma_log!(warn, "Linear channel violation: {}", msg);
                    violation_msgs.push(msg);
                }
            }
            // UNCONDITIONAL HARD-FAIL: any linear-channel
            // violation returns `VumaError::Transform` and aborts
            // compilation (returning `CompileResult::Partial` on this
            // recovery path), regardless of `--strict-ive`. The earlier
            // false-positive fix eliminated the only known source of
            // spurious violations, so the gate is now safe to
            // enforce by default. `--strict-ive` is retained only for
            // `bv_verify` (Stage 7a) which still has the "reserved for
            // future strict mode" advisory status.
            if !violation_msgs.is_empty() {
                timings.push((
                    "linear-check".to_string(),
                    t_linear.elapsed().as_millis() as u64,
                ));
                errors.push(VumaError::Transform {
                    pass_name: "linear-channel".to_string(),
                    errors: violation_msgs,
                });
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
        }
        timings.push((
            "linear-check".to_string(),
            t_linear.elapsed().as_millis() as u64,
        ));
    }

    // ── Stage 8: IR Lowering ──────────────────────────────────────────
    let t = Instant::now();
    // NOTE: The canonical pipeline now uses the DIRECT AST→codegen SCG
    // bridge (`bridge_ast_to_codegen_scg`) instead of the semantic-SCG →
    // codegen-SCG bridge. The semantic SCG (`scg`) is still built and used
    // for BD inference / MSG / IVE verification / SCG transforms above,
    // but the emitted binary is produced from the AST directly. This avoids
    // the segfaults / infinite loops that the old `bridge_scg_to_codegen*`
    // path produced.
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

    // Syscall allowlist — reject obviously invalid syscall numbers.
    // Since `nr` is arch-specific, we use a range check.
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

    // Note: lower_syscalls_all() was removed — real
    // IRInstr::Syscall emission is added directly to all backends, so the IR flows through
    // to codegen unchanged.

    // ── Stage 8b: Codegen-Level IR Optimization (production caller) ──
    // Use the ACTUAL backend's latency table for per-ISA optimization.
    // In VUMA 2.0 O3 is mandatory, so the codegen-opt pass always runs.
    {
        let topt = Instant::now();
        let emit_config = config.emit_config();
        let latency_table =
            if let Ok(backend) = vuma_codegen::backend::create_backend(emit_config.backend) {
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

    // Escape analysis + SROA + alloc elision + interprocedural
    // effect propagation.  See `compile` for the full rationale.  In VUMA
    // 2.0 O3 is mandatory, so this always runs.
    {
        let te = Instant::now();
        let summary = run_escape_and_effects_passes(&mut ir_program);
        vuma_log!(
            debug,
            "escape+effects (recovery): sroa_promoted={} allocs_elided={} pure_fns={}/{}",
            summary.sroa_promoted,
            summary.allocs_elided,
            summary.pure_functions,
            summary.total_functions
        );
        timings.push((
            "escape-effects".to_string(),
            te.elapsed().as_millis() as u64,
        ));
    }

    let ir_function_count = ir_program.functions.len();
    let ir_instruction_count: usize = ir_program
        .functions
        .iter()
        .map(|f| f.blocks.iter().map(|b| b.instructions.len()).sum::<usize>())
        .sum();
    timings.push(("ir-lowering".to_string(), t.elapsed().as_millis() as u64));

    // ── Stage 9: Register Allocation (parallel across functions) ──────
    // Parallelize per-function register allocation with std::thread::scope.
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
/// let source = "transform main() -> i32 { return 42; }";
/// let wasm_binary = compile_to_wasm(source).expect("compilation failed");
/// // wasm_binary is a valid .wasm module that exits with code 42
/// ```
pub fn compile_to_wasm(source: &str) -> Result<Vec<u8>, Vec<VumaError>> {
    // ── Stage 1: Parse ────────────────────────────────────────────
    //
    // `compile_to_wasm` does not take a `CompileConfig`, so we use the
    // default `max_expr_depth` (= `vuma_parser::MAX_EXPR_DEPTH` = 1024).
    // Callers that need to override the limit should go through
    // `compile_with_path` instead.
    let max_depth = CompileConfig::default().max_expr_depth;
    let ast = match parse_source(source, max_depth) {
        Ok(ast) => ast,
        Err(e) => return Err(vec![e]),
    };

    // ── Stage 2: AST → SCG ───────────────────────────────────────
    let mut scg = match ast_to_scg(&ast) {
        Ok(scg) => scg,
        Err(e) => return Err(vec![e]),
    };

    // ── Stage 3: SCG Transforms ───────────────────────────────────────
    // (Pillar IX.3 — "one compilation path, IVE always full".) VUMA 2.0
    // verification is MANDATORY on EVERY compilation path. The difference
    // between `vuma build` and Wasm emission is the OUTPUT FORMAT (ELF vs
    // Wasm), NOT whether IVE runs — so the Wasm path now runs the SAME
    // PMT state verification + memory-safety gates as the canonical
    // `compile_with_path` / `compile_with_recovery` route (see Stage 6 /
    // Stage 6b there). We use `Normal` (the default, which maps to `Pmt`
    // in the pipeline) rather than the removed `None` variant so the
    // config is well-formed.
    let wasm_config = CompileConfig {
        target: CompileTarget::Wasm32,
        opt_level: OptLevel::O3,
        verification_level: VerificationLevel::Normal,
        allow_inconclusive: false,
        strict_ive: false,
        ive_max_paths: 64,
        ive_max_path_length: 256,
        entry_name: "main".to_string(),
        debug_info: false,
        stop_on_first_error: true,
        max_inline_size: 50,
        inline_threshold: vuma_codegen::opt::DEFAULT_INLINE_THRESHOLD,
        runtime_bounds_checks: false,
        section_headers: false,
        max_expr_depth: vuma_parser::MAX_EXPR_DEPTH,
        backend: vuma_codegen::backend::BackendKind::Wasm32,
    };
    let _ = run_scg_transforms(&mut scg, &wasm_config);

    // ── Stage 3b: IVE PMT Verification (VUMA 2.0 — MANDATORY) ─────────
    // (Pillar IX.3 unification.) `compile_to_wasm` previously SKIPPED IVE
    // entirely — the legacy comment read "Wasm is a sandboxed target and
    // PMT verification is the caller's responsibility." That created a
    // SECOND compilation path with a weaker safety story than the
    // canonical `compile_with_path` route. The v4 thesis requires ONE
    // compilation path with IVE always full, so the Wasm path now runs
    // the SAME PMT state verification gate (the 3 state verifiers at
    // `IveVerificationLevel::Pmt`) that the canonical pipeline runs in
    // Stage 6. Verification is a HARD gate: on `Fail` (or `Inconclusive`,
    // since `compile_to_wasm` takes no `CompileConfig` and therefore has
    // no `--allow-inconclusive` opt-out) the Wasm binary is NOT emitted.
    {
        let pmt_layouts = build_pmt_layout_specs(&ast);
        let secret_vars = collect_secret_vars(&ast);
        let (codegen_scg_for_ive, typed_state_meta) = bridge_ast_to_codegen_scg_with_meta(&ast);
        let ive_input =
            vuma_ive::verification::VerificationInput::from_codegen_scg(codegen_scg_for_ive)
                .with_pmt_layouts(pmt_layouts)
                .with_secret_vars(secret_vars)
                .with_typed_state_meta(typed_state_meta);
        let aggregator = InvariantAggregator::new()
            .with_level(IveVerificationLevel::Pmt)
            .with_max_paths(wasm_config.ive_max_paths)
            .with_max_path_length(wasm_config.ive_max_path_length);
        let ive_result = aggregator.verify_all(&ive_input);
        if ive_result.overall == OverallVerdict::Fail
            || ive_result.overall == OverallVerdict::Inconclusive
        {
            return Err(vec![VumaError::Verification { result: ive_result }]);
        }
    }

    // ── Stage 3c: Memory Safety Analysis (MANDATORY, blocking) ────────
    // Mirrors Stage 6b of `compile_with_path`: VUMA 2.0 is PMT-only and
    // memory-safety analysis is MANDATORY (no opt-out). Runs the
    // SCG-liveness-based UAF / uninit-read / double-free check on the
    // semantic SCG; leak detection is advisory-only (IVE Stage 3b handles
    // real leaks via its static-lifetime analysis). A HARD gate: any
    // violation refuses to emit the Wasm binary.
    {
        let liveness = vuma_scg::liveness::LivenessAnalysis::new(&scg);
        let ms_config_blocking = vuma_codegen::memory_safety::MemorySafetyConfig {
            check_use_after_free: true,
            check_uninitialized_reads: true,
            check_double_free: true,
            check_memory_leaks: false,
            check_dangling_pointers: false,
            runtime_bounds_checks: false,
            errors_are_fatal: true,
        };
        let liveness_violations = vuma_codegen::memory_safety::analyze_with_scg_liveness(
            &liveness,
            &scg,
            &ms_config_blocking,
        );
        if !liveness_violations.is_empty() {
            let report = vuma_codegen::memory_safety::MemorySafetyReport {
                violations: liveness_violations,
                ..vuma_codegen::memory_safety::MemorySafetyReport::empty()
            };
            return Err(vec![VumaError::MemorySafety { report }]);
        }
    }

    // ── Stage 4: IR Lowering ─────────────────────────────────────
    // NOTE: The canonical pipeline now uses the DIRECT AST→codegen SCG
    // bridge (`bridge_ast_to_codegen_scg`) instead of the semantic-SCG →
    // codegen-SCG bridge. The semantic SCG (`scg`) is still built above
    // for SCG transforms, but the emitted Wasm is produced from the AST
    // directly. This avoids the segfaults / infinite loops that the old
    // `bridge_scg_to_codegen*` path produced.
    let codegen_scg = bridge_ast_to_codegen_scg(&ast);
    let mut ir_builder = IRBuilder::new();
    let mut ir_program = match ir_builder.build(&codegen_scg) {
        Ok(ir) => ir,
        Err(e) => {
            return Err(vec![VumaError::Codegen {
                error: CodegenError::ElfError(format!("{}", e)),
            }])
        }
    };

    // Syscall allowlist — reject obviously invalid syscall numbers.
    // Since `nr` is arch-specific, we use a range check.
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

    // Note: lower_syscalls_all() was removed — real
    // IRInstr::Syscall emission is added directly to all backends, so the IR flows through
    // to codegen unchanged. (lower_syscalls()/lower_syscalls_all()
    // definitions were also deleted as dead-code cleanup.)

    // ── Codegen-Level IR Optimization (production caller) ────────
    ir_program = vuma_codegen::opt::run_optimizations(ir_program);

    // Central pre-lowering float-op verification.
    //
    // Reject bitwise/shift/remainder ops on F32/F64 operands BEFORE the
    // wasm32 backend's `allocate_registers` runs (inside
    // `vuma_codegen::compile_to_wasm`).  See `verify_program_float_ops`
    // in `codegen/src/backend.rs` for the full rationale.  Mirrors the
    // gate in `compile_modules` and `compile_to_binary_direct`.
    if let Err(errs) = vuma_codegen::backend::verify_program_float_ops(&ir_program) {
        return Err(vec![VumaError::Codegen {
            error: CodegenError::InvalidInstruction(format!(
                "compile_to_wasm: pre-lowering float-op verification failed: {}",
                errs.join("; ")
            )),
        }]);
    }

    // ── Stage 5: Compile IR → Wasm ──────────────────────────────
    let wasm_bytes = match vuma_codegen::compile_to_wasm(&ir_program.functions) {
        Ok(bytes) => bytes,
        Err(e) => {
            return Err(vec![VumaError::Codegen {
                error: CodegenError::ElfError(format!("{}", e)),
            }]);
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
///
/// `max_depth` controls the parser's expression-nesting limit (see
/// [`vuma_parser::Parser::with_max_depth`] and
/// [`vuma_parser::MAX_EXPR_DEPTH`]). Callers should pass
/// `config.max_expr_depth` so the `--max-expr-depth N` CLI flag is
/// honoured.
fn parse_source(source: &str, max_depth: u32) -> Result<AstProgram, VumaError> {
    let mut parser = Parser::with_max_depth(source, max_depth);
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
///
/// `max_depth` is forwarded to the parser (and to the [`ModuleResolver`]
/// for imported files) so the `--max-expr-depth N` CLI flag applies
/// uniformly across the root module and its imports.
fn parse_and_resolve(
    source: &str,
    file_path: Option<&Path>,
    max_depth: u32,
) -> Result<AstProgram, VumaError> {
    // Fast path: if there are no imports, just parse normally.
    let mut parser = Parser::with_max_depth(source, max_depth);
    let result = parser.parse_program();
    if result.has_errors() {
        return Err(VumaError::Parse {
            errors: result.errors,
        });
    }
    let program = result.unwrap();

    // Check if there are any import statements.
    let has_imports = program
        .items
        .iter()
        .any(|i| matches!(i, vuma_parser::ast::Item::Import(_)));
    if !has_imports {
        return Ok(program);
    }

    // Resolve imports using the ModuleResolver. Propagate `max_depth` so
    // imported files are parsed with the same expression-nesting limit.
    let mut resolver = ModuleResolver::new().with_max_depth(max_depth);
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

/// Summary of the escape+effects passes run on a program.
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

/// Drive escape-analysis-driven SROA + alloc elision +
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
    // LICM after inlining+DCE so it sees the post-inline loop
    // structure.
    pm.add_pass(LoopInvariantCodeMotion::new());
    pm.add_pass(DeadCodeElimination::new()); // cleanup after LICM
                                             // Strength reduction + tail-call detection +
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
        // The stage count is no longer hardcoded — interprocedural/modular/proof
        // stages have been added. Just verify that
        // timing data was collected for multiple stages.
        assert!(
            output.stage_timings.len() >= 8,
            "Expected at least 8 stages with timing data, got {}",
            output.stage_timings.len()
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
        assert!(
            result.is_ok(),
            "O0 compilation should succeed (O3 is mandatory — full pass set always runs)"
        );
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
        // Quick mode now runs all 5 invariants at reduced depth.
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
        let fp1 = SourceFingerprint::from_source("transform main() {}");
        let fp2 = SourceFingerprint::from_source("transform main() {} ");
        let fp3 = SourceFingerprint::from_source("transform main() {}");
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

    // ── Type-aware shift tests ────────────────────────────────────────
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
        let scg = Scg::new(vec![ScgNode::Function(ScgFunction {
                name: "test_shr_signed".into(),
                params: vec![ScgParam {
                    name: "x".into(),
                    ty: ScgType::I64,
                }],
                results: vec![],
                body: ir_body,
                var_types: Default::default(),
            })]);
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
        let scg = Scg::new(vec![ScgNode::Function(ScgFunction {
                name: "test_shr_unsigned".into(),
                params: vec![ScgParam {
                    name: "n".into(),
                    ty: ScgType::U64,
                }],
                results: vec![],
                body: ir_body,
                var_types: Default::default(),
            })]);
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

    // ── Memory-safety blocking-pass tests ───────────────────

    /// Regression test: a program with a use-after-free.
    ///
    /// This test verifies that the memory-safety blocking pass is wired
    /// into the pipeline and runs without crashing.  The SCG-liveness-
    /// based UAF detector (`find_use_after_free`) relies on precise
    /// dataflow edges that may not be present for all UAF patterns in
    /// the current SCG; when the detector DOES catch the UAF, the
    /// pipeline must reject the program with `VumaError::MemorySafety`.
    /// When it does NOT catch it (a known limitation of the current
    /// liveness analysis), the program compiles — this is documented
    /// behavior, not a bug in the wiring.
    ///
    /// The `test_wave20_memory_safety_error_variant` test below
    /// verifies the `VumaError::MemorySafety` variant itself.
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
        let config = CompileConfig::default();
        let result = compile(source, &config);
        // A clean program should compile successfully — the memory-safety
        // pass runs without crashing and finds no violations.
        assert!(
            result.is_ok(),
            "Clean program should compile (memory-safety pass is mandatory), got: {:?}",
            result.err()
        );
    }

    /// A clean program (no UAF, no leaks) must compile
    /// successfully.  This is a negative test — the analyzer must NOT
    /// produce false positives on well-behaved programs.
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
        let config = CompileConfig::default();
        let result = compile(source, &config);
        assert!(
            result.is_ok(),
            "Clean program must compile (memory-safety is mandatory), got: {:?}",
            result.err()
        );
    }

    /// The `MemorySafety` error variant's `stage()` must return
    /// `"memory-safety"` and its `Display` impl must mention "violation(s)".
    #[test]
    fn test_wave20_memory_safety_error_variant() {
        let report = vuma_codegen::memory_safety::MemorySafetyReport {
            violations: vec![
                vuma_codegen::memory_safety::MemorySafetyViolation::UseAfterFree {
                    allocation_name: "buf".to_string(),
                    dealloc_line: Some(5),
                    violation_count: 1,
                },
            ],
            ..vuma_codegen::memory_safety::MemorySafetyReport::empty()
        };
        let err = VumaError::MemorySafety { report };
        assert_eq!(err.stage(), "memory-safety");
        let msg = format!("{}", err);
        assert!(
            msg.contains("memory-safety"),
            "Display should mention memory-safety: {}",
            msg
        );
        assert!(
            msg.contains("violation"),
            "Display should mention violation: {}",
            msg
        );
    }

    // ───────────────────────────────────────────────────────────────────
    // match-stmt complex-pattern lowering + sync-block fence
    // ───────────────────────────────────────────────────────────────────
    //
    // NOTE on coverage: the lowering in `bridge_stmt_to_scg` handles
    // `MatchPattern::Range` and `MatchPattern::Or` (expanding them into
    // individual SwitchArms), but the VUMA parser (`parse_match_pattern`
    // in parser/src/parser.rs) does not yet produce these variants — the
    // AST enum has them, but the parser only emits Lit/Wildcard/Ident/
    // Struct/Enum. The Range/Or lowering is therefore forward-looking:
    // ready for when the parser is extended. The Ident-binding and
    // Enum-binding lowerings ARE exercisable today, so they have tests
    // below; Range/Or do not (the parser rejects `1..=3` and `1 | 2`).
    // The Struct-pattern warning is exercised via the CLI (it fires
    // correctly — see manual test in the docs) but is not
    // unit-tested here because it interacts with a pre-existing
    // memory-safety false-positive on function-parameter matches.
    // -----------------------------------------------------------------

    /// A match with an identifier-binding pattern (`x => ...`)
    /// must compile. The identifier is treated as a binding — the
    /// discriminant is bound to `x` and the arm becomes the default_body.
    #[test]
    fn test_wave10e_match_ident_binding_compiles() {
        let source = r#"
            fn passthrough(n: i64) -> i64 {
                match n {
                    0 => 0,
                    x => x,
                }
            }
            fn main() -> i32 {
                return 0;
            }
        "#;
        let config = CompileConfig::default();
        let result = compile(source, &config);
        assert!(
            result.is_ok(),
            "match with identifier-binding pattern should compile, got: {:?}",
            result.err()
        );
    }

    /// A match with an enum-binding pattern (`Some(v) => ...`)
    /// must compile. The enum variant is treated as a binding — the
    /// discriminant is bound to `v` and the arm becomes the default_body.
    #[test]
    fn test_wave10e_match_enum_binding_compiles() {
        let source = r#"
            fn extract(n: i64) -> i64 {
                match n {
                    0 => 0,
                    Some(v) => v,
                    _ => 99,
                }
            }
            fn main() -> i32 {
                return 0;
            }
        "#;
        let config = CompileConfig::default();
        let result = compile(source, &config);
        assert!(
            result.is_ok(),
            "match with enum-binding pattern should compile, got: {:?}",
            result.err()
        );
    }

    /// A `sync { ... }` block must compile. The body is wrapped
    /// in AtomicStore acquire/release (lowered to native atomic
    /// instructions on each backend).
    #[test]
    fn test_wave10e_sync_block_compiles() {
        let source = r#"
            fn main() -> i32 {
                x = 42;
                sync {
                    x = x + 1;
                }
                return x;
            }
        "#;
        let config = CompileConfig::default();
        let result = compile(source, &config);
        assert!(
            result.is_ok(),
            "sync block should compile, got: {:?}",
            result.err()
        );
    }

    /// Task 6-B: verify `populate_codegen_edges` populates DataFlow and
    /// ControlFlow edges from the codegen Scg's own structure after the
    /// AST → codegen bridge runs.
    #[test]
    fn test_populate_codegen_edges_basic() {
        let source = r#"
            fn add(x: i64, y: i64) -> i64 {
                let sum = x + y;
                return sum;
            }
        "#;
        let mut parser = vuma_parser::Parser::new(source);
        let ast = parser.parse_program().expect("parse");
        let scg = bridge_ast_to_codegen_scg(&ast);

        // node_index should have entries for the top-level statements.
        assert!(
            scg.node_index.len() >= 2,
            "node_index should index at least 2 statements, got {}",
            scg.node_index.len()
        );

        // edges should be non-empty.
        assert!(
            !scg.edges.is_empty(),
            "edges should be populated, got 0 edges"
        );

        // Should have at least one DataFlow edge (sum def → return sum).
        let has_dataflow = scg
            .edges
            .iter()
            .any(|e| e.kind == EdgeKind::DataFlow);
        assert!(has_dataflow, "should have at least one DataFlow edge");

        // Should have at least one ControlFlow edge (sequential fall-through).
        let has_controlflow = scg
            .edges
            .iter()
            .any(|e| e.kind == EdgeKind::ControlFlow);
        assert!(
            has_controlflow,
            "should have at least one ControlFlow edge"
        );
    }
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
// when no `fn main` exists, synthesises a `transform main() -> i64` containing
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
/// Used by ForeignState lowering to inspect #[foreign_consume],
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
/// Used to check if a layout has #[foreign(raw)].
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
            vuma_parser::ast::AttrValue::List(items) => items.first().cloned().unwrap_or_default(),
            vuma_parser::ast::AttrValue::KeyValue { value, .. } => value.clone(),
        }),
    }
}

/// Convert a slice of parser `Attribute`s to codegen `AttrInfo`s.
pub fn attrs_to_attr_infos(
    attrs: &[vuma_parser::ast::Attribute],
) -> Vec<vuma_codegen::marshal::AttrInfo> {
    attrs.iter().map(attr_to_attr_info).collect()
}

/// Build a `var_name → allocation_size_in_bytes` table from a codegen SCG.
///
/// Walks every function's body (recursing into `ControlNode::If`/`Loop`/
/// `Switch` arms) and collects `AllocationNode::Stack` entries:
///
/// * Stack arrays (`alloc(N)`): size is the explicit `N` bytes.
/// * PMT state buffers (`state_new(Layout)`): the parser embeds the
///   layout's `total_size` into `AllocationNode::Stack.size` at AST→SCG
///   time (see `to_scg::layout_total_size`), so the same field carries
///   the correct bound.
///
/// `AllocationNode::Heap` is **not** collected: its `size_expr` is a
/// runtime expression, not a static literal, so the bound cannot be
/// resolved without SCG-level constant folding (Stage 3 / SoftBound).
///
/// The returned table is consulted by
/// [`vuma_codegen::memory_safety::find_bounds_check_sites_with_bounds`]
/// to populate `length_expr`, and by
/// [`vuma_codegen::memory_safety::inject_bounds_check_ir`] to emit
/// `__oob_trap` IR when `--safe` is set.
pub fn build_alloc_sizes(scg: &Scg) -> std::collections::HashMap<String, u64> {
    let mut table = std::collections::HashMap::new();
    for node in &scg.nodes {
        if let ScgNode::Function(func) = &node {
            collect_alloc_sizes_in_stmts(&func.body, &mut table);
        }
    }
    table
}

fn collect_alloc_sizes_in_stmts(
    stmts: &[ScgStatement],
    table: &mut std::collections::HashMap<String, u64>,
) {
    use vuma_codegen::scg_to_ir::ControlNode;
    for stmt in stmts {
        match stmt {
            ScgStatement::Allocation(AllocationNode::Stack { name, size, .. }) => {
                // Last-writer-wins: a name is shadowed if redeclared in a
                // narrower scope. In practice state-typed allocations and
                // stack arrays are unique per function.
                table.insert(name.clone(), u64::from(*size));
            }
            ScgStatement::Allocation(AllocationNode::Heap { .. }) => {
                // Heap size is a runtime expression — skip (no static bound).
            }
            ScgStatement::Control(ctrl) => match ctrl {
                ControlNode::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    collect_alloc_sizes_in_stmts(then_body, table);
                    if let Some(eb) = else_body {
                        collect_alloc_sizes_in_stmts(eb, table);
                    }
                }
                ControlNode::Loop { body, .. } => {
                    collect_alloc_sizes_in_stmts(body, table);
                }
                ControlNode::Switch {
                    arms, default_body, ..
                } => {
                    for arm in arms {
                        collect_alloc_sizes_in_stmts(&arm.body, table);
                    }
                    collect_alloc_sizes_in_stmts(default_body, table);
                }
                ControlNode::Break | ControlNode::Continue => {}
            },
            _ => {}
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Task 6-B: Codegen Scg edge population (post-pass over node_index)
// ═══════════════════════════════════════════════════════════════════════════
//
// `bridge_ast_to_codegen_scg_with_meta` (Task 4-C) populated the codegen
// Scg's `node_index: HashMap<NodeId, NodeLoc>` but left `edges: Vec<CodegenEdge>`
// empty.  IVE's cache / query / inference modules need edges with `EdgeKind`
// (DataFlow, ControlFlow, ...) for graph traversal (fingerprinting,
// reachability, BD propagation).
//
// `populate_codegen_edges` is a post-pass that walks each function's
// top-level body and derives two kinds of edges from the codegen Scg's own
// structure (no semantic-SCG dependency):
//
//   1. **DataFlow**: for each statement, collect the `ScgExpr::Var(name)`
//      references in its operands.  A `HashMap<String, NodeId>` maps variable
//      name → defining `NodeId` as we walk.  When a Var reference matches a
//      previously-defined variable, a `DataFlow` edge is created from the
//      defining statement to this one.
//
//   2. **ControlFlow**: consecutive top-level statements get `ControlFlow`
//      edges.  Control nodes (`If` / `Loop` / `Switch`) also get a
//      `ControlFlow` edge to the next top-level statement after the block
//      (the fall-through / join target).
//
// # Limitations (documented gaps)
//
// - Only **top-level** statements in each `ScgNode::Function` body receive
//   NodeIds (per the 4-C node_index contract).  Statements nested inside
//   `If`/`Loop`/`Switch` bodies are NOT in `node_index`, so they receive no
//   edges and no intra-block dataflow edges are created.  This is
//   acceptable for IVE's graph-traversal use cases (fingerprinting,
//   reachability, BD propagation) which operate on the node-level graph.
//
// - Parameter references do NOT produce DataFlow edges (parameters have no
//   defining statement in the body).  This is correct: parameters are
//   function-entry inputs, not dataflow from a prior statement.
//
// - `ForeignConsume`'s `state_var` is a `String`, not an `ScgExpr`; we
//   still treat it as a Var reference so a DataFlow edge is created from
//   the statement that defined the consumed state.
//
// - Variable reassignment (`ComputationNode::reassigns`, `CallNode::reassigns`)
//   updates the `def_map` so later references point to the most recent
//   definition.  Only the latest def is tracked (SSA is not reconstructed).

/// Recursively collect `ScgExpr::Var(name)` references from an expression
/// into `out`.
fn collect_scg_expr_vars(expr: &ScgExpr, out: &mut Vec<String>) {
    match expr {
        ScgExpr::Var(n) => out.push(n.clone()),
        ScgExpr::Int(_) | ScgExpr::Float(_) | ScgExpr::Label(_) => {}
        ScgExpr::BinOp { lhs, rhs, .. } => {
            collect_scg_expr_vars(lhs, out);
            collect_scg_expr_vars(rhs, out);
        }
        ScgExpr::Load { addr } => collect_scg_expr_vars(addr, out),
    }
}

/// Collect variable names **read** (used as operands) by a statement.
/// Only top-level expressions are inspected; nested statement bodies
/// (inside `If`/`Loop`/`Switch`) are NOT walked (see limitations above).
fn collect_stmt_var_reads(stmt: &ScgStatement, out: &mut Vec<String>) {
    match stmt {
        ScgStatement::Control(ctrl) => match ctrl {
            ControlNode::If { cond, .. } => collect_scg_expr_vars(cond, out),
            ControlNode::Loop { for_range, .. } => {
                if let Some((_, start, end)) = for_range {
                    collect_scg_expr_vars(start, out);
                    collect_scg_expr_vars(end, out);
                }
            }
            ControlNode::Break | ControlNode::Continue => {}
            ControlNode::Switch { discriminant, .. } => {
                collect_scg_expr_vars(discriminant, out)
            }
        },
        ScgStatement::Allocation(AllocationNode::Heap { size_expr, .. }) => {
            collect_scg_expr_vars(size_expr, out)
        }
        ScgStatement::Allocation(AllocationNode::Stack { .. }) => {}
        ScgStatement::Access(AccessNode::Load { ptr, offset, .. }) => {
            collect_scg_expr_vars(ptr, out);
            if let Some(off) = offset {
                collect_scg_expr_vars(off, out);
            }
        }
        ScgStatement::Access(AccessNode::Store { ptr, offset, value, .. }) => {
            collect_scg_expr_vars(ptr, out);
            if let Some(off) = offset {
                collect_scg_expr_vars(off, out);
            }
            collect_scg_expr_vars(value, out);
        }
        ScgStatement::Cast(CastNode { src, .. }) => collect_scg_expr_vars(src, out),
        ScgStatement::Computation(ComputationNode { lhs, rhs, .. }) => {
            collect_scg_expr_vars(lhs, out);
            collect_scg_expr_vars(rhs, out);
        }
        ScgStatement::UnaryComputation(UnaryComputationNode { operand, .. }) => {
            collect_scg_expr_vars(operand, out)
        }
        ScgStatement::Call(CallNode { args, .. }) => {
            for a in args {
                collect_scg_expr_vars(a, out);
            }
        }
        ScgStatement::Return(exprs) => {
            for e in exprs {
                collect_scg_expr_vars(e, out);
            }
        }
        ScgStatement::ConstantTime(ct) => {
            for o in &ct.operands {
                collect_scg_expr_vars(o, out);
            }
        }
        ScgStatement::StructAccess(StructAccessNode::Load { ptr, .. }) => {
            collect_scg_expr_vars(ptr, out)
        }
        ScgStatement::StructAccess(StructAccessNode::Store { ptr, value, .. }) => {
            collect_scg_expr_vars(ptr, out);
            collect_scg_expr_vars(value, out);
        }
        ScgStatement::EnumAccess(ea) => match ea {
            EnumAccessNode::LoadTag { ptr, .. } => collect_scg_expr_vars(ptr, out),
            EnumAccessNode::StoreTag { ptr, value, .. } => {
                collect_scg_expr_vars(ptr, out);
                collect_scg_expr_vars(value, out);
            }
            EnumAccessNode::LoadPayload { ptr, .. } => collect_scg_expr_vars(ptr, out),
            EnumAccessNode::StorePayload { ptr, value, .. } => {
                collect_scg_expr_vars(ptr, out);
                collect_scg_expr_vars(value, out);
            }
        },
        ScgStatement::GetAddress(GetAddressNode { .. }) => {} // name is a symbol, not a var
        ScgStatement::Syscall(SyscallCallNode { args, .. }) => {
            for a in args {
                collect_scg_expr_vars(a, out);
            }
        }
        ScgStatement::ForeignConsume(fc) => out.push(fc.state_var.clone()),
        ScgStatement::ChannelOpen(ChannelOpenStmt { .. }) => {}
        ScgStatement::ChannelSend(ChannelSendStmt { channel, message, .. }) => {
            collect_scg_expr_vars(channel, out);
            collect_scg_expr_vars(message, out);
        }
        ScgStatement::ChannelRecv(ChannelRecvStmt { channel, .. }) => {
            collect_scg_expr_vars(channel, out)
        }
        ScgStatement::ChannelClose(ChannelCloseStmt { channel }) => {
            collect_scg_expr_vars(channel, out)
        }
        ScgStatement::ChannelRecvResult(ChannelRecvResultStmt { channel, .. }) => {
            collect_scg_expr_vars(channel, out)
        }
        ScgStatement::PmtOp(pmt) => match pmt {
            PmtOpStmt::StateInit { .. } => {}
            PmtOpStmt::StateRead { src, .. } => collect_scg_expr_vars(src, out),
            PmtOpStmt::StateWrite { ptr, val, .. } => {
                collect_scg_expr_vars(ptr, out);
                collect_scg_expr_vars(val, out);
            }
            PmtOpStmt::StateTransform { src, .. } => collect_scg_expr_vars(src, out),
            PmtOpStmt::ArenaNew { capacity, .. } => collect_scg_expr_vars(capacity, out),
            PmtOpStmt::ArenaAlloc { arena, .. } => collect_scg_expr_vars(arena, out),
            PmtOpStmt::ArenaGrow { arena, min_capacity, .. } => {
                collect_scg_expr_vars(arena, out);
                collect_scg_expr_vars(min_capacity, out);
            }
            PmtOpStmt::ArenaFree { ptr } => collect_scg_expr_vars(ptr, out),
        },
    }
}

/// Record the variable(s) **defined** (written) by a statement into the
/// `def_map`.  Reassignments update the map so later reads point to the
/// most recent def.
fn record_stmt_defs(stmt: &ScgStatement, node_id: NodeId, def_map: &mut HashMap<String, NodeId>) {
    match stmt {
        ScgStatement::Allocation(AllocationNode::Stack { name, .. })
        | ScgStatement::Allocation(AllocationNode::Heap { name, .. }) => {
            def_map.insert(name.clone(), node_id);
        }
        ScgStatement::Access(AccessNode::Load { dst, .. }) => {
            def_map.insert(dst.clone(), node_id);
        }
        ScgStatement::Cast(CastNode { dst, .. }) => {
            def_map.insert(dst.clone(), node_id);
        }
        ScgStatement::Computation(ComputationNode { dst, .. }) => {
            def_map.insert(dst.clone(), node_id);
        }
        ScgStatement::UnaryComputation(UnaryComputationNode { dst, .. }) => {
            def_map.insert(dst.clone(), node_id);
        }
        ScgStatement::Call(CallNode { dst: Some(dst), .. }) => {
            def_map.insert(dst.clone(), node_id);
        }
        ScgStatement::ConstantTime(ct) => {
            def_map.insert(ct.dst.clone(), node_id);
        }
        ScgStatement::StructAccess(StructAccessNode::Load { dst, .. }) => {
            def_map.insert(dst.clone(), node_id);
        }
        ScgStatement::EnumAccess(EnumAccessNode::LoadTag { dst, .. })
        | ScgStatement::EnumAccess(EnumAccessNode::LoadPayload { dst, .. }) => {
            def_map.insert(dst.clone(), node_id);
        }
        ScgStatement::GetAddress(GetAddressNode { dst, .. }) => {
            def_map.insert(dst.clone(), node_id);
        }
        ScgStatement::Syscall(SyscallCallNode { dst: Some(dst), .. }) => {
            def_map.insert(dst.clone(), node_id);
        }
        ScgStatement::ChannelOpen(ChannelOpenStmt { dst, .. }) => {
            def_map.insert(dst.clone(), node_id);
        }
        ScgStatement::ChannelRecv(ChannelRecvStmt { dst, .. }) => {
            def_map.insert(dst.clone(), node_id);
        }
        ScgStatement::ChannelRecvResult(ChannelRecvResultStmt { dst, err_dst, .. }) => {
            def_map.insert(dst.clone(), node_id);
            def_map.insert(err_dst.clone(), node_id);
        }
        ScgStatement::PmtOp(pmt) => match pmt {
            PmtOpStmt::StateInit { dst, .. }
            | PmtOpStmt::StateRead { dst, .. }
            | PmtOpStmt::StateTransform { dst, .. }
            | PmtOpStmt::ArenaNew { dst, .. }
            | PmtOpStmt::ArenaAlloc { dst, .. }
            | PmtOpStmt::ArenaGrow { dst, .. } => {
                def_map.insert(dst.clone(), node_id);
            }
            PmtOpStmt::StateWrite { .. } | PmtOpStmt::ArenaFree { .. } => {}
        },
        // Statements that do not define a variable:
        // Return, Control(*), Access(Store), StructAccess(Store),
        // EnumAccess(Store*), ForeignConsume, ChannelSend, ChannelClose.
        _ => {}
    }
}

/// Task 6-B: Populate the codegen Scg's `edges` vector with DataFlow and
/// ControlFlow edges derived from the Scg's own structure.
///
/// Must be called **after** `node_index` is populated (Task 4-C), since the
/// NodeIds are read from that index.  See the module-level comment above for
/// the derivation rules and documented limitations.
pub fn populate_codegen_edges(scg: &mut Scg) {
    // Invert node_index: (fn_idx, stmt_idx) → NodeId.
    let mut loc_to_node: HashMap<(usize, usize), NodeId> = HashMap::new();
    for (node_id, loc) in &scg.node_index {
        loc_to_node.insert((loc.fn_idx, loc.stmt_idx), *node_id);
    }

    let mut edges: Vec<CodegenEdge> = Vec::new();

    for (fn_idx, node) in scg.nodes.iter().enumerate() {
        let func = match node {
            ScgNode::Function(f) => f,
            ScgNode::Data(_) => continue,
        };

        // Variable name → defining NodeId, scoped to this function's body.
        let mut def_map: HashMap<String, NodeId> = HashMap::new();
        let body_len = func.body.len();

        for stmt_idx in 0..body_len {
            let stmt = &func.body[stmt_idx];
            let this_id = match loc_to_node.get(&(fn_idx, stmt_idx)) {
                Some(id) => *id,
                None => continue,
            };

            // ── DataFlow edges: Var reads → defining statement ──────────
            let mut var_reads: Vec<String> = Vec::new();
            collect_stmt_var_reads(stmt, &mut var_reads);
            for var_name in &var_reads {
                if let Some(def_id) = def_map.get(var_name) {
                    // Avoid duplicate edges (same source/target/kind/label).
                    let label = format!("var:{}", var_name);
                    let dup = edges.iter().any(|e| {
                        e.source == *def_id
                            && e.target == this_id
                            && e.kind == EdgeKind::DataFlow
                            && e.label.as_deref() == Some(label.as_str())
                    });
                    if !dup {
                        edges.push(CodegenEdge {
                            source: *def_id,
                            target: this_id,
                            kind: EdgeKind::DataFlow,
                            label: Some(label),
                        });
                    }
                }
            }

            // ── ControlFlow edge: sequential fall-through ───────────────
            if stmt_idx + 1 < body_len {
                if let Some(next_id) = loc_to_node.get(&(fn_idx, stmt_idx + 1)) {
                    edges.push(CodegenEdge {
                        source: this_id,
                        target: *next_id,
                        kind: EdgeKind::ControlFlow,
                        label: None,
                    });
                }
            }

            // ── Record this statement's definitions ────────────────────
            record_stmt_defs(stmt, this_id, &mut def_map);
        }
    }

    scg.edges = edges;
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
/// statements but no `fn main`, a synthetic `transform main() -> i64` containing
/// them (followed by `return 0;`) is emitted.
pub fn bridge_ast_to_codegen_scg_with_meta(program: &AstProgram) -> (Scg, Vec<vuma_codegen::scg_to_ir::TypedStateMeta>) {
    // Collect extern function names so we can mark calls as is_extern.
    let extern_fns = extract_extern_functions_from_ast(program);

    // Collect extern fn declarations (with attrs) and layout
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
    let void_functions: HashSet<String> = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::FnDef(fn_def) if fn_def.return_type.is_none() => Some(fn_def.name.clone()),
            Item::TransformDef(td) if td.return_type.is_none() => Some(td.name.clone()),
            _ => None,
        })
        .collect();

    // Collect ALL function names (extern + user-defined) so flatten_expr
    // can detect when a Var refers to a function address (e.g., `my_fn as u64`).
    let mut function_names: HashSet<String> = extern_fns.clone();
    for item in &program.items {
        match item {
            Item::FnDef(fn_def) => {
                function_names.insert(fn_def.name.clone());
            }
            Item::TransformDef(td) => {
                function_names.insert(td.name.clone());
            }
            _ => {}
        }
    }

    // FIX1: build a map of `fn/transform name → State<L> layout name` for
    // every function or transform whose return type is `State<LayoutName>`.
    // When a `let c = add_points(p, q);` call binds the result of such a
    // callee, `c` is registered as state-typed with that layout so
    // subsequent `c.field` accesses resolve to Loads at the right offset.
    // Without this, callers must annotate `let c: State<L> = ...` manually.
    let state_returning_fns: HashMap<String, String> = program
        .items
        .iter()
        .filter_map(|item| {
            let (name, ret_ty) = match item {
                Item::FnDef(fn_def) => (&fn_def.name, &fn_def.return_type),
                Item::TransformDef(td) => (&td.name, &td.return_type),
                _ => return None,
            };
            ret_ty
                .as_ref()
                .and_then(|ty| extract_state_layout_name_from_ast(ty).map(|ln| (name.clone(), ln)))
        })
        .collect();

    // PMT: build the layout registry from all `Item::LayoutDef`
    // items. Cloned into each function's BridgeCtx so state.field accesses
    // can resolve field offsets/types at bridge time.
    let layouts = build_layout_registry(program);

    // (Task 2-A) Build the transform registry: name -> (input_layout,
    // output_layout) for every `transform t : L_in -> L_out` whose both
    // layouts resolve. Mirrors the extractor's transform_registry setup.
    // Cloned into each function's ctx so flatten_expr's Expr::Call arm can
    // recognise transform invocations and push a StateTransform meta entry.
    let transform_registry: HashMap<String, (String, String)> = {
        let mut tr: HashMap<String, (String, String)> = HashMap::new();
        for item in &program.items {
            if let Item::TransformDef(td) = item {
                let input_layout = td
                    .params
                    .first()
                    .and_then(|p| p.ty.as_ref())
                    .and_then(extract_state_layout_name_from_ast);
                let output_layout = td
                    .return_type
                    .as_ref()
                    .and_then(extract_state_layout_name_from_ast);
                if let (Some(il), Some(ol)) = (input_layout, output_layout) {
                    tr.insert(td.name.clone(), (il, ol));
                }
            }
        }
        tr
    };

    // (Task 2-A) Program-level accumulator for TypedStateMeta entries
    // drained from each function's ctx.meta, plus a program-wide vreg
    // counter (carried across functions) so result_vreg is source-order
    // program-wide.
    let mut program_meta: Vec<vuma_codegen::scg_to_ir::TypedStateMeta> = Vec::new();
    let mut program_meta_vreg: u32;

    // ── Collect top-level statements ─────────────────────────────────
    //
    // Top-level `Item::Stmt` items (e.g. `region buf = allocate(1024);`,
    // `let x = 5;`, standalone `free(ptr);`, etc.) execute at program start.
    // The old bridge dropped them silently, which caused `main` to
    // segfault when it referenced a buffer declared at file scope.
    //
    // We use a dedicated BridgeCtx (separate from any function's ctx) so
    // temp names don't collide with those used inside `main`'s body.

    // Create a shared string table for all function contexts.
    // Each ctx gets a clone of the Rc, so string literals are deduplicated
    // program-wide. After all functions are processed, the table is drained
    // and emitted as a single ScgNode::Data (ReadOnly) section.
    let shared_string_table: StringTable = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
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
        tl_ctx.transform_registry = transform_registry.clone();
        for item in &program.items {
            if let Item::Stmt(stmt) = item {
                top_level_stmts.extend(bridge_stmt_to_scg(stmt, &mut tl_ctx));
            }
        }
        // (Task 2-A) Drain any meta produced by top-level state ops and
        // carry the program-wide vreg counter forward.
        program_meta.extend(tl_ctx.meta.drain(..));
        program_meta_vreg = tl_ctx.meta_vreg;
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
                    contract: td.contract.clone(),
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
            ctx.transform_registry = transform_registry.clone();
            // (Task 2-A) Seed this function's vreg counter with the
            // program-wide value so StateInit result_vreg stays source-order
            // program-wide (mirrors the extractor's shared counter).
            ctx.meta_vreg = program_meta_vreg;
            // PMT: register state-typed params (those with
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

            // (Task 2-A) Drain this function's typed-state meta into the
            // program-level accumulator, and carry the program-wide vreg
            // counter forward to the next function.
            program_meta.extend(ctx.meta.drain(..));
            program_meta_vreg = ctx.meta_vreg;

            // Ensure every function ends with a Return statement.
            // If the body doesn't end with a Return, add an implicit one.
            // When the function has a return type and the last statement was an
            // expression, use ctx.last_expr_result as the return value.
            let has_return = body
                .last()
                .is_some_and(|s| matches!(s, ScgStatement::Return(_)));
            if !has_return {
                let ret_val = if !results.is_empty() {
                    // First check if the last expression was tracked.
                    if let Some(ref expr) = ctx.last_expr_result {
                        Some(expr.clone())
                    } else {
                        // Otherwise, look for the last computation/call result.
                        body.iter().rev().find_map(|s| match s {
                            ScgStatement::Computation(comp) => Some(ScgExpr::Var(comp.dst.clone())),
                            ScgStatement::Call(call) => {
                                call.dst.as_ref().map(|d| ScgExpr::Var(d.clone()))
                            }
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
        let main_idx = nodes
            .iter()
            .position(|n| matches!(n, ScgNode::Function(f) if f.name == "main"));
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

    // Emit the string table as a read-only data section.
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

    // Task 3-A: populate `Scg::typed_state_meta` first-class (via
    // `new_with_meta`) AND keep returning `program_meta` as the tuple's
    // second element for backward compatibility with the existing
    // `tests/scg_conformance.rs` destructuring (Task 3-C will drop the
    // tuple and read `scg.typed_state_meta` directly). `TypedStateMeta:
    // Clone` (derive on the enum) so cloning the Vec is cheap and safe.

    // ── Task 4-C: populate the codegen Scg's graph layer ─────────────
    //
    // The canonical bridge lowers AST → codegen Scg DIRECTLY; it does not
    // retain (or build) the semantic `vuma_scg::SCG`, so semantic `NodeId`s
    // and `EdgeData` are not in scope here. (The `EdgeIndex` helper defined
    // further up in this file is dead infrastructure left over from the
    // deprecated `bridge_scg_to_codegen*` functions deleted in Tasks 1-A /
    // 1-B — it is never instantiated by this bridge, hence the task brief's
    // "builds an EdgeIndex then discards it" premise no longer holds for
    // the canonical path.)
    //
    // MINIMAL population (per the task's escape hatch): build `node_index`
    // by assigning a fresh, monotonic `NodeId` to every `ScgStatement`
    // inside every `ScgNode::Function`, mapping each to its
    // `NodeLoc { fn_idx, stmt_idx }`. This makes `Scg::get_node`,
    // `Scg::nodes`, and `Scg::node_count` fully functional for IVE and any
    // other consumer that walks the codegen Scg as a graph. `edges` is left
    // empty — `successors` / `predecessors` therefore return empty vecs
    // gracefully. Full edge population requires either threading the
    // semantic SCG (or semantic NodeIds) into this bridge, or re-deriving
    // dataflow edges from the AST; both are out of scope for the ≤1-file
    // 4-C constraint — see NEEDS_FOLLOWUP 4-C.
    let node_index: HashMap<NodeId, vuma_codegen::scg_to_ir::NodeLoc> = {
        let mut idx: HashMap<NodeId, vuma_codegen::scg_to_ir::NodeLoc> = HashMap::new();
        let mut next_id: u64 = 0;
        for (fn_idx, node) in nodes.iter().enumerate() {
            if let ScgNode::Function(f) = node {
                for stmt_idx in 0..f.body.len() {
                    idx.insert(
                        NodeId(next_id),
                        vuma_codegen::scg_to_ir::NodeLoc { fn_idx, stmt_idx },
                    );
                    next_id += 1;
                }
            }
        }
        idx
    };

    let mut codegen_scg = Scg::new_with_meta(nodes, program_meta.clone());
    codegen_scg.node_index = node_index;
    // Task 6-B: derive DataFlow + ControlFlow edges from the codegen Scg's
    // own structure (post-pass over the node_index populated above).
    populate_codegen_edges(&mut codegen_scg);

    (codegen_scg, program_meta)
}

/// (Task 2-A) Thin wrapper around [`bridge_ast_to_codegen_scg_with_meta`]
/// that discards the `TypedStateMeta` metadata, preserving the historical
/// `-> Scg` signature so existing callers (CLI bins, tests) are unaffected.
/// Callers that need the metadata should call
/// [`bridge_ast_to_codegen_scg_with_meta`] directly.
pub fn bridge_ast_to_codegen_scg(program: &AstProgram) -> Scg {
    bridge_ast_to_codegen_scg_with_meta(program).0
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
    /// (logical, unsigned) for the `>>` operator. Without
    /// this, all `>>` operations collapse to a single shift kind, breaking
    /// either `bit_abs` (i64 → needs ShrA) or `bit_log2` (u64 → needs ShrL).
    pub var_types: HashMap<String, ScgType>,
    /// Set of user-defined function names that return void (no return type).
    /// Populated by `bridge_ast_to_codegen_scg` before processing function
    /// bodies. Used by `flatten_expr` to emit `dst: None` for void function
    /// calls — critical for wasm32 which loads from mem[0] for non-void calls.
    pub void_functions: HashSet<String>,
    /// PMT: layout definitions — maps layout name → (total_size,
    /// fields). Each field is `(name, ir_type, byte_offset, byte_size,
    /// type_name)` where `type_name` is the field's declared type as a
    /// string (e.g. "u32", "Point") — used to descend into nested
    /// layout-typed fields.
    /// Built once from `Item::LayoutDef` items at the start of
    /// `bridge_ast_to_codegen_scg` and cloned into each function's ctx.
    pub layouts: LayoutRegistry,
    /// PMT: state-typed variable → layout name. Populated per
    /// function: state-typed params are registered before the body is
    /// lowered, and `let p = state_new(L)` / `let p: State<L> = ...` add
    /// entries as they're processed.
    pub state_var_layouts: HashMap<String, String>,
    /// (ForeignState): extern fn name → its declaration (incl. attrs).
    /// Used at call sites to detect #[foreign_consume], #[foreign_return],
    /// #[callback], etc. Populated by `bridge_ast_to_codegen_scg`.
    pub extern_fn_decls: HashMap<String, vuma_parser::ast::ExternFnDecl>,
    /// (ForeignState): layout name → its declaration (incl. attrs).
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
    /// program-wide string literal table. Each entry is
    /// (label, bytes_including_NUL). Shared across all function contexts
    /// via Rc<RefCell<>> so string literals are deduplicated program-wide.
    /// The table is drained after all functions are processed and emitted
    /// as a single ScgNode::Data (ReadOnly) section.
    pub string_table: StringTable,
    /// Set of all function names (extern + user-defined). Used by
    /// flatten_expr to detect when a Var refers to a function (not a variable)
    /// and produce ScgExpr::Label for function-address expressions.
    pub function_names: HashSet<String>,
    /// (Task 2-A) Recoverable typed-state metadata, threaded through the
    /// bridge so the canonical AST->codegen SCG walk produces one
    /// `TypedStateMeta` entry per typed-state op as a side product of
    /// lowering (no parallel AST walk — the standalone extractor was merged
    /// into the bridge in Task 2-B and deleted). Populated at the 5
    /// detection sites (StateInit/StateRead/StateWrite/StateTransform/
    /// ForeignConsume) and drained into a program-level accumulator after
    /// each function body is lowered.
    pub meta: Vec<vuma_codegen::scg_to_ir::TypedStateMeta>,
    /// (Task 2-A) `transform name -> (input_layout, output_layout)` for
    /// every `transform t : L_in -> L_out` whose both layouts resolve.
    /// Populated once at the start of `bridge_ast_to_codegen_scg_with_meta`
    /// and cloned into each function's ctx. Used by `flatten_expr`'s
    /// `Expr::Call` arm to recognise transform invocations and push a
    /// `StateTransform` meta entry (the bridge's own transform registry).
    pub transform_registry: HashMap<String, (String, String)>,
    /// (Task 2-A) Synthetic per-program vreg counter for `StateInit` meta
    /// entries. Carried across functions so `result_vreg` is source-order
    /// program-wide.
    pub meta_vreg: u32,
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
            meta: Vec::new(),
            transform_registry: HashMap::new(),
            meta_vreg: 0,
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
pub fn eval_const_expr(
    expr: &vuma_parser::ast::Expr,
    consts: &HashMap<String, i64>,
) -> Option<i64> {
    use vuma_parser::ast::{BinOp, Expr, Lit, UnOp};
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
                BinOp::And => {
                    if l != 0 && r != 0 {
                        1
                    } else {
                        0
                    }
                }
                BinOp::Or => {
                    if l != 0 || r != 0 {
                        1
                    } else {
                        0
                    }
                }
                BinOp::Eq => {
                    if l == r {
                        1
                    } else {
                        0
                    }
                }
                BinOp::Ne => {
                    if l != r {
                        1
                    } else {
                        0
                    }
                }
                BinOp::Lt => {
                    if l < r {
                        1
                    } else {
                        0
                    }
                }
                BinOp::Le => {
                    if l <= r {
                        1
                    } else {
                        0
                    }
                }
                BinOp::Gt => {
                    if l > r {
                        1
                    } else {
                        0
                    }
                }
                BinOp::Ge => {
                    if l >= r {
                        1
                    } else {
                        0
                    }
                }
            })
        }
        Expr::UnOp { op, expr, .. } => {
            let v = eval_const_expr(expr, consts)?;
            Some(match op {
                UnOp::Neg => v.wrapping_neg(),
                UnOp::Not => {
                    if v == 0 {
                        1
                    } else {
                        0
                    }
                }
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
        // Bridge `Channel<T>` through the pipeline.  Channel values
        // are opaque IPC handles — pointer-sized — but we preserve the inner
        // payload type via `ScgType::Channel` so downstream stages can
        // recover it when lowering send/recv operations.
        //
        // Session types: the `session_type` field on the AST
        // node is dropped here — ScgType::Channel currently carries only
        // the payload type. A future SCG extension could add a session-
        // type slot to ScgType::Channel to thread the protocol through to
        // the IVE linear-type checker.
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

/// PMT: convert a parser `Type` to a codegen `IRType` for layout
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
        // `Channel<T>` → `IRType::Channel` (pointer-sized opaque
        // capability handle; inner payload type carried for type-checking
        // only).
        // session_type field is dropped here;
        // IRType::Channel doesn't carry protocol info yet.
        Type::Channel { inner, .. } => {
            vuma_codegen::ir::IRType::Channel(Box::new(bridge_type_to_ir_type(inner)))
        }
        _ => vuma_codegen::ir::IRType::U64,
    }
}

/// PMT: compute the byte size of a parser `Type` for layout field
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
        // `Channel<T>` is pointer-sized (8 on 64-bit, 4 on 32-bit) —
        // same as Ptr.
        // session_type field doesn't affect size.
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
                    8 // fallback (forward reference not yet resolved)
                }
            }
        },
        Type::Ptr(_) | Type::RegionPtr { .. } => 8,
        // `Channel<T>` is pointer-sized (8 on 64-bit, 4 on 32-bit).
        // session_type field doesn't affect size.
        Type::Channel { .. } => 8,
        Type::Array { element, size } => {
            bridge_type_size_with_layouts(element, layout_sizes) * (*size as u64)
        }
        _ => 8,
    }
}

/// PMT: compute the byte alignment of a parser `Type` for layout
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
        // `Channel<T>` alignment is pointer-sized (8 on 64-bit, 4 on
        // 32-bit) — same as Ptr.
        // session_type field doesn't affect alignment.
        Type::Channel { .. } => 8,
        Type::Array { element, .. } => bridge_type_align(element),
        _ => 8,
    }
}

/// PMT: if `ty` is `State<LayoutName>`, return `Some(layout_name)`.
fn extract_state_layout_name_from_ast(ty: &vuma_parser::ast::Type) -> Option<String> {
    if let vuma_parser::ast::Type::State(inner) = ty {
        if let vuma_parser::ast::Type::BDBase(name) = inner.as_ref() {
            return Some(name.clone());
        }
    }
    None
}

/// PMT: build the layout registry from all `Item::LayoutDef` items
/// in the program. Returns a map: layout_name → (total_size, fields) where
/// each field is (name, ir_type, byte_offset, byte_size, type_name). Field
/// offsets are computed sequentially with alignment padding (mirroring
/// vuma-bd's `LayoutRegistry::register`).
fn build_layout_registry(program: &AstProgram) -> LayoutRegistry {
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
                if falign > 1 && !size.is_multiple_of(falign) {
                    size = (size + falign - 1) & !(falign - 1);
                }
                max_align = max_align.max(falign);
                size += fsize;
            }
            let alignment = max_align.max(1);
            if size > 0 && !size.is_multiple_of(alignment) {
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
            if falign > 1 && !offset.is_multiple_of(falign) {
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
        if offset > 0 && !offset.is_multiple_of(alignment) {
            offset = (offset + alignment - 1) & !(alignment - 1);
        }
        layouts.insert((*name).to_string(), (offset, field_list));
    }
    layouts
}

/// Build a `PmtLayoutSpec` registry from the AST's `Item::LayoutDef`
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
                if falign > 1 && !offset.is_multiple_of(falign) {
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
            if offset > 0 && !offset.is_multiple_of(alignment) {
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

/// Collect the set of secret-tainted variable names from
/// `#[secret]` attributes in the source AST.
///
/// Walks every function body (including transforms and method impls) and
/// collects:
///   - `let` bindings whose `attrs` contain `#[secret]` → the bound name
///   - `Param`s whose `attrs` contain `#[secret]` → the param name
///
/// The resulting set is attached to `VerificationInput` via
/// `with_secret_vars(...)`. When non-empty, the constant-time verifier
/// (`invariant_aggregator::verify_constant_time`) uses it *instead of* the
/// unsound substring heuristic on labels/filenames.
///
/// # Scope
///
/// Only `Item::FnDef`, `Item::TransformDef`, and `Item::ImplBlock` are
/// walked; top-level `Item::Stmt(Stmt::Let(...))` (rare in practice) is
/// also covered. Nested modules (`Item::ModuleDef`) are recursed into.
/// `Item::TraitDef` is skipped (no bodies of interest — required methods
/// have no body, provided methods are covered via the impl-block walk at
/// the use site).
///
/// See `docs/architecture/ive-fix-proposals.md` for the rationale.
pub fn collect_secret_vars(program: &AstProgram) -> HashSet<String> {
    let mut secrets = HashSet::new();
    for item in &program.items {
        collect_secret_vars_from_item(item, &mut secrets);
    }
    secrets
}

/// Recursive helper for [`collect_secret_vars`]. Walks an `Item` and any
/// nested items (modules), accumulating secret variable names into `secrets`.
fn collect_secret_vars_from_item(item: &Item, secrets: &mut HashSet<String>) {
    use vuma_parser::ast::{ImplBlock, ModuleDef, Stmt, TraitDef};
    match item {
        Item::FnDef(f) => {
            collect_secret_vars_from_block(&f.body, secrets);
            for p in &f.params {
                if param_has_secret_attr(p) {
                    secrets.insert(p.name.clone());
                }
            }
        }
        Item::TransformDef(t) => {
            // TransformDef.body is `Vec<Stmt>` (not a `Block`).
            for s in &t.body {
                collect_secret_vars_from_stmt(s, secrets);
            }
            for p in &t.params {
                if param_has_secret_attr(p) {
                    secrets.insert(p.name.clone());
                }
            }
        }
        Item::ImplBlock(ImplBlock { methods, .. }) => {
            for m in methods {
                collect_secret_vars_from_block(&m.body, secrets);
                for p in &m.params {
                    if param_has_secret_attr(p) {
                        secrets.insert(p.name.clone());
                    }
                }
            }
        }
        Item::TraitDef(TraitDef {
            provided_methods, ..
        }) => {
            for m in provided_methods {
                collect_secret_vars_from_block(&m.body, secrets);
                for p in &m.params {
                    if param_has_secret_attr(p) {
                        secrets.insert(p.name.clone());
                    }
                }
            }
        }
        Item::ModuleDef(ModuleDef { items, .. }) => {
            for inner in items {
                collect_secret_vars_from_item(inner, secrets);
            }
        }
        Item::Stmt(Stmt::Let(l)) if let_stmt_has_secret_attr(l) => {
            secrets.insert(l.name.clone());
        }
        _ => {}
    }
}

/// Walk a `Block`'s statements recursively (if/while/for/match/loop/sync/
/// unsafe all nest blocks), collecting `#[secret]`-annotated `let` names.
fn collect_secret_vars_from_block(block: &vuma_parser::ast::Block, secrets: &mut HashSet<String>) {
    for stmt in &block.statements {
        collect_secret_vars_from_stmt(stmt, secrets);
    }
}

/// Recursive statement walker for [`collect_secret_vars`]. Descends into
/// nested blocks (if/while/for/loop/match/sync/unsafe) and collects names
/// of `#[secret]`-annotated `let` bindings.
fn collect_secret_vars_from_stmt(stmt: &vuma_parser::ast::Stmt, secrets: &mut HashSet<String>) {
    use vuma_parser::ast::Stmt;
    match stmt {
        Stmt::Let(l) => {
            if let_stmt_has_secret_attr(l) {
                secrets.insert(l.name.clone());
            }
            // Also descend into the initializer expression — `let x = { let #[secret] y = ...; y };`
            collect_secret_vars_from_expr(&l.value, secrets);
        }
        Stmt::If(i) => {
            collect_secret_vars_from_expr(&i.condition, secrets);
            collect_secret_vars_from_block(&i.then_block, secrets);
            if let Some(else_b) = &i.else_block {
                collect_secret_vars_from_block(else_b, secrets);
            }
        }
        Stmt::While(w) => {
            collect_secret_vars_from_expr(&w.condition, secrets);
            collect_secret_vars_from_block(&w.body, secrets);
        }
        Stmt::For(f) => {
            collect_secret_vars_from_expr(&f.iter, secrets);
            collect_secret_vars_from_block(&f.body, secrets);
        }
        Stmt::Loop(l) => collect_secret_vars_from_block(&l.body, secrets),
        Stmt::Match(m) => {
            collect_secret_vars_from_expr(&m.subject, secrets);
            for arm in &m.arms {
                if let Some(g) = &arm.guard {
                    collect_secret_vars_from_expr(g, secrets);
                }
                collect_secret_vars_from_expr(&arm.body, secrets);
            }
        }
        Stmt::Sync(s) => collect_secret_vars_from_block(&s.body, secrets),
        Stmt::UnsafeBlock { body, .. } => collect_secret_vars_from_block(body, secrets),
        Stmt::Assign(a) => collect_secret_vars_from_expr(&a.value, secrets),
        Stmt::CompoundAssign(a) => collect_secret_vars_from_expr(&a.value, secrets),
        Stmt::Return(r) => {
            if let Some(e) = &r.value {
                collect_secret_vars_from_expr(e, secrets);
            }
        }
        Stmt::Expr(e) => collect_secret_vars_from_expr(&e.expr, secrets),
        _ => {}
    }
}

/// Walk an expression for nested `Expr::Block { statements, .. }` and
/// collect `#[secret]`-annotated `let` names within. Only the
/// `Expr::Block` variant can carry `let` statements; other expression
/// variants are recursed into where they may contain sub-expressions.
fn collect_secret_vars_from_expr(expr: &vuma_parser::ast::Expr, secrets: &mut HashSet<String>) {
    use vuma_parser::ast::Expr;
    match expr {
        Expr::Block {
            statements,
            trailing_expr,
            ..
        } => {
            for s in statements {
                collect_secret_vars_from_stmt(s, secrets);
            }
            if let Some(e) = trailing_expr {
                collect_secret_vars_from_expr(e, secrets);
            }
        }
        Expr::MatchExpr {
            scrutinee, arms, ..
        } => {
            collect_secret_vars_from_expr(scrutinee, secrets);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_secret_vars_from_expr(g, secrets);
                }
                collect_secret_vars_from_expr(&arm.body, secrets);
            }
        }
        Expr::BinOp { lhs, rhs, .. } => {
            collect_secret_vars_from_expr(lhs, secrets);
            collect_secret_vars_from_expr(rhs, secrets);
        }
        Expr::UnOp { expr: inner, .. }
        | Expr::AddressOf { expr: inner, .. }
        | Expr::Deref { expr: inner, .. }
        | Expr::Cast { expr: inner, .. }
        | Expr::Spawn { expr: inner, .. }
        | Expr::Await { expr: inner, .. } => {
            collect_secret_vars_from_expr(inner, secrets);
        }
        Expr::Call { callee, args, .. } => {
            collect_secret_vars_from_expr(callee, secrets);
            for a in args {
                collect_secret_vars_from_expr(a, secrets);
            }
        }
        Expr::FieldAccess { expr: inner, .. } => collect_secret_vars_from_expr(inner, secrets),
        Expr::Index {
            expr: inner, index, ..
        } => {
            collect_secret_vars_from_expr(inner, secrets);
            collect_secret_vars_from_expr(index, secrets);
        }
        _ => {}
    }
}

/// Returns true if `let_stmt.attrs` contains a `#[secret]` attribute
/// (the attribute may carry a value or not — both forms
/// `#[secret]` and `#[secret(...)]` are accepted; only the name is checked.
fn let_stmt_has_secret_attr(let_stmt: &vuma_parser::ast::LetStmt) -> bool {
    let_stmt.attrs.iter().any(|a| a.name == "secret")
}

/// Returns true if `param.attrs` contains a `#[secret]` attribute
/// Symmetric with [`let_stmt_has_secret_attr`].
fn param_has_secret_attr(param: &vuma_parser::ast::Param) -> bool {
    param.attrs.iter().any(|a| a.name == "secret")
}

// PMT: resolve a state-field chain `(layout_name, [field1, field2, ...])`
// against the layout registry. Returns `(cumulative_offset, size, ir_type)`
// of the leaf field, descending into nested layout-typed fields. Returns
// `None` if any field in the chain isn't found.

/// Resolve a state-typed array FieldAccess (e.g. `b.data`) into its base
/// state variable name, the field's byte offset within the state buffer,
/// the array element size, and the element's IR type.
///
/// Returns `None` when `expr` is not a `FieldAccess` chain rooted at a
/// state-typed variable whose terminal field is an inline array
/// (e.g. `[u8; N]`).
///
/// This is used by the `Expr::Index` / `AssignTarget::Index` lowering
/// to emit `AccessNode::Load/Store` with
/// `ptr: Var(base_var)` and `offset: Some(data_offset + idx*elem_size)`
/// so `inject_bounds_check_ir` classifies the access as `Seq` (the state
/// var is in `alloc_sizes`) and inserts a `__oob_trap` check.
fn resolve_state_array_access(
    expr: &vuma_parser::ast::Expr,
    ctx: &BridgeCtx,
) -> Option<(String, u64, u64, Option<vuma_codegen::ir::IRType>)> {
    use vuma_parser::ast::Expr;

    if !matches!(expr, Expr::FieldAccess { .. }) {
        return None;
    }

    let mut chain: Vec<String> = Vec::new();
    let mut cur = expr;
    while let Expr::FieldAccess {
        expr: inner, field, ..
    } = cur
    {
        chain.push(field.clone());
        cur = inner.as_ref();
    }
    let base_var = if let Expr::Var { name, .. } = cur {
        name.clone()
    } else {
        return None;
    };
    let layout_name = ctx.state_var_layouts.get(&base_var)?.clone();
    chain.reverse();
    let (offset, _size, _field_ty, type_name) =
        resolve_state_field_chain(&ctx.layouts, &layout_name, &chain)?;

    if !type_name.starts_with('[') {
        return None;
    }
    let inner = &type_name[1..];
    let semi_pos = inner.find(';')?;
    let elem_type_str = inner[..semi_pos].trim();
    let (elem_size, elem_ir_type) = match elem_type_str {
        "u8" | "i8" | "bool" => (1, Some(vuma_codegen::ir::IRType::U8)),
        "u16" | "i16" => (2, Some(vuma_codegen::ir::IRType::U16)),
        "u32" | "i32" => (4, Some(vuma_codegen::ir::IRType::U32)),
        "f32" => (4, Some(vuma_codegen::ir::IRType::F32)),
        "u64" | "i64" => (8, Some(vuma_codegen::ir::IRType::U64)),
        "f64" => (8, Some(vuma_codegen::ir::IRType::F64)),
        _ => (1, None),
    };
    Some((base_var, offset, elem_size, elem_ir_type))
}

fn detect_array_elem_type(
    expr: &vuma_parser::ast::Expr,
    ctx: &BridgeCtx,
) -> (u64, Option<vuma_codegen::ir::IRType>) {
    match resolve_state_array_access(expr, ctx) {
        Some((_bv, _off, elem_size, elem_ir_type)) => (elem_size, elem_ir_type),
        None => (1, None),
    }
}

fn resolve_state_field_chain(
    layouts: &LayoutRegistry,
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
        let (_fname, ftype, foffset, fsize, ftype_name) =
            fields.iter().find(|(n, _, _, _, _)| n == field)?;
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
pub fn bridge_block_to_scg_stmts(
    block: &vuma_parser::ast::Block,
    ctx: &mut BridgeCtx,
) -> Vec<ScgStatement> {
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
/// emit ForeignConsume marker statements for a `#[foreign_consume]`
/// extern call. For each argument that is a State variable whose layout has
/// `#[foreign(raw)]`, push a `ScgStatement::ForeignConsume` so the IVE treats
/// the State's vreg as consumed (linearity error on subsequent read/write).
fn emit_foreign_consume_markers(
    func_name: &str,
    args: &[vuma_parser::ast::Expr],
    stmts: &mut Vec<ScgStatement>,
    ctx: &mut BridgeCtx,
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
                        // (Task 2-A) Thread a ForeignConsume meta entry so
                        // the bridge produces the same recoverable metadata
                        // the extractor's emit_foreign_consume_meta does.
                        ctx.meta.push(
                            vuma_codegen::scg_to_ir::TypedStateMeta::ForeignConsume {
                                var: name.clone(),
                            },
                        );
                    }
                }
            }
        }
    }
}

/// Result of recognising `match channel_recv(ch) { Ok(v) => ..., Err(e) => ... }`.
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

/// Detect the `match channel_recv(ch) { Ok(v) => ..., Err(e) => ... }`
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
            MatchPattern::Enum {
                name,
                binding: Some(_),
                ..
            } if name == "Ok" => {
                if ok_arm.is_some() {
                    return None;
                }
                ok_arm = Some(arm);
            }
            MatchPattern::Enum {
                name,
                binding: Some(_),
                ..
            } if name == "Err" => {
                if err_arm.is_some() {
                    return None;
                }
                err_arm = Some(arm);
            }
            _ => return None, // any other pattern → not this form
        }
    }
    let ok_arm = ok_arm?;
    let err_arm = err_arm?;

    let ok_binding = match &ok_arm.pattern {
        MatchPattern::Enum {
            binding: Some(b), ..
        } => b.clone(),
        _ => unreachable!(),
    };
    let err_binding = match &err_arm.pattern {
        MatchPattern::Enum {
            binding: Some(b), ..
        } => b.clone(),
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
                // The name refers to a function (not a variable).
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
                // Lower string literals to .rodata addresses.
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
            // OPERAND TYPE, not a global default.
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
        Expr::UnOp {
            op, expr: operand, ..
        } => {
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
            // (Task 2-A) If this call invokes a transform
            // `t : L_in -> L_out`, push a StateTransform meta entry
            // (mirrors the extractor's transform-call push). This catches
            // transforms used as sub-expressions; let-position transforms
            // are also caught in bridge_stmt_to_scg's Let arm.
            if let Some((il, ol)) = ctx.transform_registry.get(&func_name).cloned() {
                ctx.meta.push(
                    vuma_codegen::scg_to_ir::TypedStateMeta::StateTransform {
                        input_layout: il,
                        output_layout: ol,
                    },
                );
            }
            let flat_args: Vec<ScgExpr> =
                args.iter().map(|a| flatten_expr(a, stmts, ctx)).collect();
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
                // if this is a #[foreign_consume] extern call, emit
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
                // emit ForeignConsume markers for #[foreign_consume] calls.
                emit_foreign_consume_markers(&func_name, args, stmts, ctx);
                ScgExpr::Var(dst)
            }
        }

        // ── Direct syscall: flatten args, emit SyscallCallNode ──
        //
        // `syscall(nr, args...)` is a first-class AST expression
        // that lowers to `ScgStatement::Syscall`. The IRBuilder then emits
        // `IRInstr::Syscall`, which each backend lowers directly to a real
        // syscall instruction (the intermediate
        // `lower_syscalls_all()` lowering pass has been removed) so backends can resolve it
        // via their existing `syscall_stubs` tables.
        //
        // Void syscalls (exit, exit_group) get `dst: None` so the IR doesn't
        // contain a dead vreg that would never be assigned. For all other
        // syscalls, we allocate a fresh temp and return it as a `Var` so the
        // result can flow into surrounding expressions.
        Expr::Syscall { nr, args, .. } => {
            let flat_args: Vec<ScgExpr> =
                args.iter().map(|a| flatten_expr(a, stmts, ctx)).collect();
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
        Expr::AtomicCas {
            addr,
            expected,
            desired,
            ..
        } => {
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
            let idx_expr = flatten_expr(index, stmts, ctx);

            // Array-lowering: when the base is a state-typed array
            // FieldAccess (e.g. `b.data[idx]`), resolve the base state var
            // and field offset directly, then emit a single
            // `AccessNode::Load { ptr: Var(base_var),
            //                     offset: Some(data_offset + scaled_idx) }`.
            // This makes the access visible to `inject_bounds_check_ir` as a
            // `Seq` access (the state var is in `alloc_sizes`), so an
            // out-of-bounds index triggers `__oob_trap` under `--safe`.
            if let Some((base_var, data_offset, elem_size, elem_ir_type)) =
                resolve_state_array_access(expr, ctx)
            {
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

                // Combine static field offset + dynamic index into a single
                // offset expression: `data_offset + scaled_idx`. When
                // `data_offset == 0`, just use `scaled_idx` directly.
                let offset_expr = if data_offset == 0 {
                    scaled_idx
                } else {
                    ScgExpr::BinOp {
                        op: BinOpKind::Add,
                        lhs: Box::new(ScgExpr::Int(data_offset as i64)),
                        rhs: Box::new(scaled_idx),
                    }
                };

                let dst = ctx.alloc_temp();
                stmts.push(ScgStatement::Access(AccessNode::Load {
                    dst: dst.clone(),
                    ptr: ScgExpr::Var(base_var),
                    offset: Some(offset_expr),
                    ty: elem_ir_type,
                }));
                return ScgExpr::Var(dst);
            }

            // Fallback (non-state-var index): original Add-based lowering.
            let base_expr = flatten_expr(expr, stmts, ctx);
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
                    ScgExpr::Int(0),    // addr = NULL
                    size_expr,          // length = size
                    ScgExpr::Int(3),    // prot = PROT_READ|PROT_WRITE
                    ScgExpr::Int(0x22), // flags = MAP_PRIVATE|MAP_ANONYMOUS
                    ScgExpr::Int(-1),   // fd = -1 (MAP_ANONYMOUS)
                    ScgExpr::Int(0),    // offset = 0
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

        // ── PMT: state.field read (and nested state.a.b read) ──
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
            while let Expr::FieldAccess {
                expr: inner, field, ..
            } = cur
            {
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
                    // (Task 2-A) Push a StateRead meta entry (mirrors the
                    // extractor's StateRead push). Use the outermost field
                    // (chain[0]) as the field_name.
                    if let Some(field_name) = chain.first().cloned() {
                        ctx.meta.push(
                            vuma_codegen::scg_to_ir::TypedStateMeta::StateRead {
                                var: bv.clone(),
                                layout_name: layout_name.clone(),
                                field_name,
                            },
                        );
                    }
                    if let Some((offset, _size, field_ty, type_name)) =
                        resolve_state_field_chain(&ctx.layouts, &layout_name, &chain)
                    {
                        // Fix (array-typed fields): when the field's
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
                        // Fix (scalar fields): compute the field
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

        // ── PMT: StateInit as an expression (rare — usually
        // handled directly in PStmt::Let). If encountered here, return 0
        // since we can't allocate without a binding name. ──
        Expr::StateInit { .. } => {
            eprintln!("[vuma] WARNING: state_new() outside let-binding in flatten_expr; using 0");
            ScgExpr::Int(0)
        }

        // ── Arena State Model: arena builtins lower to
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
            // Guard page: mmap one extra page (4096 bytes) past the
            // user-visible capacity, then mprotect it PROT_NONE so the MMU
            // traps on overflow even if the bump-pointer check is bypassed.
            // The stored arena.capacity remains the user-visible value
            // (without the +4096).
            let mmap_size = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: mmap_size.clone(),
                op: BinOpKind::Add,
                lhs: cap_expr.clone(),
                rhs: ScgExpr::Int(4096),
                tail_call: false,
                reassigns: None,
            }));
            let arena_ptr = ctx.alloc_temp();
            // FFI-5-B (Gap #1 closure): route mmap through IRInstr::Syscall
            // (asm-generic nr=222 → native x86_64 nr=9 via syscall_abi::translate).
            // Eliminates the last `is_extern: true` CallNode for mmap in this path.
            stmts.push(ScgStatement::Syscall(SyscallCallNode {
                nr: 222,
                dst: Some(arena_ptr.clone()),
                args: vec![
                    ScgExpr::Int(0),         // addr = NULL
                    ScgExpr::Var(mmap_size), // length = capacity + 4096 (guard page)
                    ScgExpr::Int(3),         // prot = PROT_READ|PROT_WRITE
                    ScgExpr::Int(0x22),      // flags = MAP_PRIVATE|MAP_ANONYMOUS
                    ScgExpr::Int(-1),        // fd = -1
                    ScgExpr::Int(0),         // offset = 0
                ],
            }));
            // mprotect(arena_ptr + capacity, 4096, PROT_NONE=0) — guard page.
            // Return value is ignored (stored to a temp, never checked) so a
            // kernel that refuses the mprotect does not fail compilation.
            let guard_addr = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: guard_addr.clone(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var(arena_ptr.clone()),
                rhs: cap_expr.clone(),
                tail_call: false,
                reassigns: None,
            }));
            let _mprot_ret = ctx.alloc_temp();
            // FFI-5-B (Gap #1 closure): route mprotect through IRInstr::Syscall
            // (asm-generic nr=226 → native x86_64 nr=10 via syscall_abi::translate).
            stmts.push(ScgStatement::Syscall(SyscallCallNode {
                nr: 226,
                dst: Some(_mprot_ret),
                args: vec![
                    ScgExpr::Var(guard_addr),
                    ScgExpr::Int(4096), // size = one page
                    ScgExpr::Int(0),    // prot = PROT_NONE
                ],
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
        Expr::ArenaAlloc {
            arena, layout_name, ..
        } => {
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
            let layout_size = ctx
                .layouts
                .get(layout_name)
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
        Expr::ArenaGrow {
            arena,
            min_capacity,
            ..
        } => {
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
            // Guard page: mremap to min_capacity + 4096 so the new
            // mapping has room for a PROT_NONE tail. The stored capacity is
            // still the user-visible min_capacity (without the +4096).
            let new_map_size = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: new_map_size.clone(),
                op: BinOpKind::Add,
                lhs: min_cap_expr.clone(),
                rhs: ScgExpr::Int(4096),
                tail_call: false,
                reassigns: None,
            }));
            // Call mremap(arena_ptr, capacity, min_capacity + 4096, MREMAP_MAYMOVE=1)
            let new_base = ctx.alloc_temp();
            // FFI-5-B (Gap #1 closure): route mremap through IRInstr::Syscall
            // (asm-generic nr=216 → native x86_64 nr=25 via syscall_abi::translate).
            stmts.push(ScgStatement::Syscall(SyscallCallNode {
                nr: 216,
                dst: Some(new_base.clone()),
                args: vec![
                    arena_ptr,
                    ScgExpr::Var(cap_val),
                    ScgExpr::Var(new_map_size),
                    ScgExpr::Int(1),
                ],
            }));
            // mprotect(new_base + min_capacity, 4096, PROT_NONE=0) — guard
            // page on the new tail. Return value ignored.
            let guard_addr = ctx.alloc_temp();
            stmts.push(ScgStatement::Computation(ComputationNode {
                dst: guard_addr.clone(),
                op: BinOpKind::Add,
                lhs: ScgExpr::Var(new_base.clone()),
                rhs: min_cap_expr.clone(),
                tail_call: false,
                reassigns: None,
            }));
            let _mprot_ret = ctx.alloc_temp();
            // FFI-5-B (Gap #1 closure): route mprotect through IRInstr::Syscall
            // (asm-generic nr=226 → native x86_64 nr=10 via syscall_abi::translate).
            stmts.push(ScgStatement::Syscall(SyscallCallNode {
                nr: 226,
                dst: Some(_mprot_ret),
                args: vec![
                    ScgExpr::Var(guard_addr),
                    ScgExpr::Int(4096), // size = one page
                    ScgExpr::Int(0),    // prot = PROT_NONE
                ],
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
            // FFI-5-B (Gap #1 closure): route munmap through IRInstr::Syscall
            // (asm-generic nr=215 → native x86_64 nr=11 via syscall_abi::translate).
            stmts.push(ScgStatement::Syscall(SyscallCallNode {
                nr: 215,
                dst: None,
                args: vec![arena_ptr, ScgExpr::Var(cap_val)],
            }));
            ScgExpr::Int(0)
        }

        // ── If-expression: `if cond { then } else { else }` ──
        // Lowers to: if cond { result = <then_value> } else { result = <else_value> }
        // then returns `result` as a ScgExpr::Var.
        // The then/else blocks' trailing expression (last Stmt::Expr) is the
        // branch value. bridge_block_to_scg_stmts stores the last ExprStmt's
        // result in ctx.last_expr_result.
        Expr::IfExpr {
            condition,
            then_block,
            else_block,
            ..
        } => {
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

        // ── Struct literal — `Point { x: 10, y: 20 }` ──
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
                    let (offset, _size, field_ty, type_name) = layout_fields
                        .iter()
                        .find_map(|(fn_, ir_ty, off, sz, tn)| {
                            if fn_ == field_name {
                                Some((*off, *sz, ir_ty.clone(), tn.clone()))
                            } else {
                                None
                            }
                        })
                        .unwrap_or((0, 0, vuma_codegen::ir::IRType::U64, String::new()));

                    // Handle nested struct literal fields inline.
                    // If the field's type_name is a known layout AND the field
                    // expression is a StructInit, write the nested struct's
                    // fields directly into the parent buffer at the cumulative
                    // offset (rather than allocating a separate temp and storing
                    // its pointer). This makes `b.a.x` resolve correctly because
                    // the nested fields are inline in the parent buffer.
                    if ctx.layouts.contains_key(&type_name) {
                        if let vuma_parser::ast::Expr::StructInit {
                            name: _nested_name,
                            fields: nested_fields,
                            ..
                        } = field_expr
                        {
                            if let Some((_nested_total, nested_layout_fields)) =
                                ctx.layouts.get(&type_name).cloned()
                            {
                                // Write each nested field directly into the parent buffer
                                // at (parent_base + field_offset + nested_field_offset).
                                for (nested_fname, nested_fexpr) in nested_fields {
                                    let (nested_off, nested_fty) = nested_layout_fields
                                        .iter()
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
                eprintln!(
                    "[vuma] WARNING: struct literal with unknown layout '{}'; using 0",
                    name
                );
                ScgExpr::Int(0)
            }
        }

        // ── Block expression (used in match arm bodies) ──
        // `{ stmt; stmt; expr }` — flatten each statement into `stmts` (via
        // `bridge_stmt_to_scg`), then flatten the optional trailing
        // expression.  The block's value is the trailing expression's value
        // (or 0 / unit if absent).  This is what makes
        // `match channel_recv(ch) { Ok(v) => { return 7; }, Err(e) => { return 99; } }`
        // lower correctly: the block's `return` statement flows into the
        // arm's ScgStatement list and becomes an `IRInstr::Ret`.
        Expr::Block {
            statements,
            trailing_expr,
            ..
        } => {
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
        // in `flatten_expr` based on the lhs operand's declared type.
        // This default applies when the operand type is unknown
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
fn infer_load_type_from_ast_expr(
    expr: &vuma_parser::ast::Expr,
) -> Option<vuma_codegen::ir::IRType> {
    use vuma_parser::ast::{BinOp, Expr, Lit};
    if let Expr::BinOp {
        op: BinOp::Add,
        lhs: _,
        rhs,
        ..
    } = expr
    {
        if let Expr::BinOp {
            op: BinOp::Mul,
            lhs: _,
            rhs: mul_rhs,
            ..
        } = rhs.as_ref()
        {
            if let Expr::Lit {
                value: Lit::Int(stride),
                ..
            } = mul_rhs.as_ref()
            {
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
    if let ScgExpr::BinOp {
        op: vuma_codegen::ir::BinOpKind::Add,
        lhs: _,
        rhs,
    } = ptr
    {
        if let ScgExpr::BinOp {
            op: vuma_codegen::ir::BinOpKind::Mul,
            lhs: _,
            rhs,
        } = rhs.as_ref()
        {
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
            // — type-aware shift regression. Without this,
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
                // PMT: register state-typed vars (`let p: State<L> = ...`)
                // so subsequent `p.field` accesses lower to Loads with the
                // layout's field offsets.
                if let Some(layout_name) = extract_state_layout_name_from_ast(ty) {
                    ctx.state_var_layouts
                        .insert(let_stmt.name.clone(), layout_name);
                }
            }

            // PMT: `let p = state_new(Layout)` → AllocationNode::Stack
            // sized to the layout's total_size. The resulting stack slot
            // holds the state's buffer pointer; subsequent `p.field` reads/
            // writes use it as the Load/Store address.
            //
            // Liveness (tombstone UAF detection): the allocation is
            // grown by +1 byte to make room for a LIVE/DEAD flag at
            // `[ptr + total_size]`. The flag is set to 1 (LIVE) here, and
            // `inject_liveness_check_ir` (memory_safety.rs) emits a check
            // before each SEQ access that traps via `__uaf_trap` (exit 135)
            // when the flag is 0 (DEAD). The +1 byte is also reflected in
            // `alloc_sizes` so the bounds check still catches true OOB
            // accesses (one byte looser than before, but never unsound:
            // the extra byte belongs to the same allocation).
            if let vuma_parser::ast::Expr::StateInit { layout_name, .. } = &let_stmt.value {
                if let Some((total_size, _)) = ctx.layouts.get(layout_name) {
                    // Register the var as state-typed BEFORE returning so
                    // subsequent `p.field` accesses can find the layout.
                    ctx.state_var_layouts
                        .insert(let_stmt.name.clone(), layout_name.clone());
                    // (Task 2-A) Push a StateInit meta entry (mirrors the
                    // extractor's StateInit push).
                    ctx.meta.push(
                        vuma_codegen::scg_to_ir::TypedStateMeta::StateInit {
                            layout_name: layout_name.clone(),
                            result_vreg: ctx.meta_vreg,
                        },
                    );
                    ctx.meta_vreg += 1;
                    let flag_off = *total_size as i64;
                    return vec![
                        ScgStatement::Allocation(AllocationNode::Stack {
                            name: let_stmt.name.clone(),
                            size: *total_size as u32, // +1 byte for liveness flag is NOT added here —
                            // the flag is stored at [ptr + total_size] which is within the stack
                            // frame's allocated region (the frame is larger than total_size).
                            // Adding +1 would make build_alloc_sizes report total_size+1, which
                            // would make the bounds check UGe(idx, total_size+1) too lenient.
                            ty: ScgType::Ptr,
                        }),
                        // Store LIVE flag (1) at [ptr + total_size].
                        ScgStatement::Access(AccessNode::Store {
                            ptr: ScgExpr::Var(let_stmt.name.clone()),
                            offset: Some(ScgExpr::Int(flag_off)),
                            value: ScgExpr::Int(1),
                            ty: Some(vuma_codegen::ir::IRType::U8),
                        }),
                    ];
                }
            }

            // `let p = Point { x: 10, y: 20 }` — register the let
            // variable as state-typed so subsequent `p.field` accesses work.
            // The actual allocation + field writes are handled by flatten_expr's
            // StructInit arm (which emits an AllocationNode::Stack for a temp,
            // writes each field, and returns the temp). Here we just pre-register
            // the let variable name as state-typed with the struct's layout.
            if let vuma_parser::ast::Expr::StructInit { name, .. } = &let_stmt.value {
                if ctx.layouts.contains_key(name) {
                    ctx.state_var_layouts
                        .insert(let_stmt.name.clone(), name.clone());
                }
            }

            // Arena State Model: `let arena = arena_new(cap)` and
            // `let w = arena_alloc(arena, Widget)` → register as state-typed
            // so field access works. The actual lowering happens in
            // flatten_expr (which emits mmap/mremap/munmap CallNodes +
            // Load/Store for the Arena struct). Here we just register the
            // variable as state-typed with the correct layout.
            match &let_stmt.value {
                vuma_parser::ast::Expr::ArenaNew { .. } => {
                    ctx.state_var_layouts
                        .insert(let_stmt.name.clone(), "Arena".to_string());
                }
                vuma_parser::ast::Expr::ArenaAlloc { layout_name, .. } => {
                    ctx.state_var_layouts
                        .insert(let_stmt.name.clone(), layout_name.clone());
                }
                vuma_parser::ast::Expr::ArenaGrow { .. } => {
                    // arena_grow returns the arena — it's already registered
                    // as state-typed from the original arena_new/arena_alloc.
                    // Just re-register with "Arena" layout to be safe.
                    ctx.state_var_layouts
                        .insert(let_stmt.name.clone(), "Arena".to_string());
                }
                _ => {}
            }

            // Check if the RHS is an allocate() call → AllocationNode::Stack
            if let vuma_parser::ast::Expr::Call { callee, args, .. } = &let_stmt.value {
                if let vuma_parser::ast::Expr::Var { name, .. } = callee.as_ref() {
                    if name == "allocate" {
                        let size: u32 = args
                            .first()
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
                    let flat_args: Vec<ScgExpr> = args
                        .iter()
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
                        ctx.state_var_layouts
                            .insert(let_stmt.name.clone(), layout_name);
                    }
                    // (Task 2-A) Push a StateTransform meta entry if this
                    // call invokes a `transform t : L_in -> L_out` (mirrors
                    // the extractor's let-position transform-call push).
                    if let Some((il, ol)) = ctx.transform_registry.get(name).cloned() {
                        ctx.meta.push(
                            vuma_codegen::scg_to_ir::TypedStateMeta::StateTransform {
                                input_layout: il,
                                output_layout: ol,
                            },
                        );
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

            // `let x = syscall(nr, args…)` → SyscallCallNode with
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
                    vuma_parser::ast::Expr::Lit {
                        value: vuma_parser::ast::Lit::Int(n),
                        ..
                    } => *n as u32,
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
                    // Arena State Model: for arena_alloc results,
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
                let idx = flatten_expr(index, &mut stmts, ctx);

                // Array-lowering: when the base is a state-typed
                // array FieldAccess (e.g. `b.data[idx] = v`), emit a single
                // `AccessNode::Store { ptr: Var(base_var),
                //                       offset: Some(data_offset + scaled_idx) }`
                // so `inject_bounds_check_ir` classifies it as `Seq` and
                // inserts a `__oob_trap` check under `--safe`.
                if let Some((base_var, data_offset, elem_size, elem_ir_type)) =
                    resolve_state_array_access(expr, ctx)
                {
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

                    let offset_expr = if data_offset == 0 {
                        scaled_idx
                    } else {
                        ScgExpr::BinOp {
                            op: BinOpKind::Add,
                            lhs: Box::new(ScgExpr::Int(data_offset as i64)),
                            rhs: Box::new(scaled_idx),
                        }
                    };

                    let value = flatten_expr(&assign_stmt.value, &mut stmts, ctx);
                    stmts.push(ScgStatement::Access(AccessNode::Store {
                        ptr: ScgExpr::Var(base_var),
                        offset: Some(offset_expr),
                        value,
                        ty: elem_ir_type,
                    }));
                    return stmts;
                }

                // Fallback (non-state-var index): original Add-based lowering.
                let base = flatten_expr(expr, &mut stmts, ctx);
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

            // PMT: state-field write — `p.field = val` (and nested
            // `l.a.x = val`). The parser represents these as
            // `AssignTarget::DerefField { expr, field }`. We walk the
            // FieldAccess chain to find the base state-typed var, resolve
            // the cumulative field offset against the layout registry, and
            // emit a single Store at that offset.
            if let vuma_parser::ast::AssignTarget::DerefField { expr, field, .. } =
                &assign_stmt.target
            {
                // Walk the chain to find (base_var, [field1, field2, ..., field]).
                let mut chain = vec![field.clone()];
                let mut cur = expr.as_ref();
                let mut base_var: Option<String> = None;
                while let vuma_parser::ast::Expr::FieldAccess {
                    expr: inner,
                    field: f,
                    ..
                } = cur
                {
                    chain.push(f.clone());
                    cur = inner.as_ref();
                }
                if let vuma_parser::ast::Expr::Var { name, .. } = cur {
                    base_var = Some(name.clone());
                }
                if let Some(bv) = &base_var {
                    if let Some(layout_name) = ctx.state_var_layouts.get(bv).cloned() {
                        chain.reverse(); // outermost-to-innermost order
                        // (Task 2-A) Push a StateWrite meta entry (mirrors
                        // the extractor's StateWrite push).
                        if let Some(field_name) = chain.first().cloned() {
                            ctx.meta.push(
                                vuma_codegen::scg_to_ir::TypedStateMeta::StateWrite {
                                    var: bv.clone(),
                                    layout_name: layout_name.clone(),
                                    field_name,
                                },
                            );
                        }
                        if let Some((offset, _size, field_ty, _type_name)) =
                            resolve_state_field_chain(&ctx.layouts, &layout_name, &chain)
                        {
                            // Fix: emit an explicit `Add(base, offset)`
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
                                flatten_expr(
                                    &vuma_parser::ast::Expr::Var {
                                        name: bv.clone(),
                                        span: vuma_parser::Span::synthetic(),
                                    },
                                    &mut stmts,
                                    ctx,
                                )
                            } else {
                                let base = flatten_expr(
                                    &vuma_parser::ast::Expr::Var {
                                        name: bv.clone(),
                                        span: vuma_parser::Span::synthetic(),
                                    },
                                    &mut stmts,
                                    ctx,
                                );
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
                        }
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
                    vuma_parser::ast::Expr::Lit {
                        value: vuma_parser::ast::Lit::Int(n),
                        ..
                    } => *n as u32,
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
                    let flat_args: Vec<ScgExpr> = args
                        .iter()
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

            // `x = syscall(nr, args…)` → SyscallCallNode with
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
            vec![ScgStatement::Control(ControlNode::Loop {
                body: loop_body,
                for_range: None,
                while_cond: None,
            })]
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
        // merge in `lower_loop` ensures the loop-header phi
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
                _other => (ScgExpr::Int(0), ScgExpr::Int(0)),
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
            vec![ScgStatement::Control(ControlNode::Loop {
                body,
                for_range: None,
                while_cond: None,
            })]
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
        // Lower to ControlNode::Switch. The lowering expanded the supported
        // pattern set beyond Lit/Wildcard to include Range, Or, Ident-
        // binding, and Enum-with-binding; see the per-arm dispatch below
        // for details. Patterns that still cannot be lowered without type
        // information (Struct, unit-variant Enum, uppercase Ident, oversized
        // Range, nested-complex Or) emit a `vuma_log!(warn, ...)` and drop
        // ONLY that arm's body — the wildcard/default arm still runs, which
        // is better than silently dropping the whole match.
        PStmt::Match(match_stmt) => {
            let mut pre_stmts = Vec::new();

            // ── `match channel_recv(ch) { Ok(v) => ..., Err(e) => ... }` ──
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
            let mut saw_guard = false;

            // Helper: extract an i64 from a Lit (Int/Bool/Address).
            // Returns None for Float/String (not valid switch values).
            let lit_as_i64 = |value: &vuma_parser::ast::Lit| -> Option<i64> {
                match value {
                    vuma_parser::ast::Lit::Int(n) => Some(*n),
                    vuma_parser::ast::Lit::Bool(b) => Some(if *b { 1 } else { 0 }),
                    vuma_parser::ast::Lit::Address(a) => Some(*a as i64),
                    _ => None,
                }
            };

            for arm in &match_stmt.arms {
                // ── Expanded complex-pattern support ──
                //
                // Previously, any non-Lit/non-Wildcard pattern caused the
                // arm body to be silently dropped. We now support:
                //
                //   • Range patterns (1..=N): expanded into individual
                //     SwitchArms (capped at 32 to avoid code bloat; larger
                //     ranges fall back to drop-with-warning).
                //   • Or-patterns (1 | 2 | 3): each Lit sub-pattern becomes
                //     a separate SwitchArm sharing the same body; Wildcard
                //     sub-patterns contribute to default_body. Nested
                //     complex sub-patterns (Range/Or/Ident/Struct/Enum)
                //     cause the whole arm to be dropped with a warning.
                //   • Ident patterns (x => ...): treated as a binding — the
                //     discriminant is bound to `x` via a copy Computation,
                //     and the arm becomes the default_body. (Rust convention:
                //     lowercase = binding; uppercase = unit-variant name.
                //     Uppercase idents are dropped with a warning since we
                //     can't determine the discriminant value without type
                //     info.)
                //   • Enum patterns with binding (Some(v) => ...): treated
                //     as a binding — `v` receives the discriminant. Unit-
                //     variant enums (None => ...) are dropped with a warning.
                //   • Struct patterns (Name { field, ... }): dropped with a
                //     warning — requires type info to determine offsets.
                //
                // Guards (pat if cond => ...) are NOT supported by the
                // Switch node (which has no guard slot). If any arm has a
                // guard, we emit a warning and silently IGNORE the guard
                // (the body is lowered as if the guard were absent — which
                // is incorrect but better than dropping the body entirely).
                match &arm.pattern {
                    vuma_parser::ast::MatchPattern::Lit { value, .. } => {
                        // Only integer-valued literals can become SwitchArm values.
                        let value_i = match lit_as_i64(value) {
                            Some(v) => v,
                            None => {
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
                    vuma_parser::ast::MatchPattern::Range { start, end, .. } => {
                        // Range pattern: `1..=10`. Expand into individual
                        // SwitchArms (capped at 32 to avoid code bloat).
                        const RANGE_EXPANSION_CAP: i64 = 32;
                        match (lit_as_i64(start), lit_as_i64(end)) {
                            (Some(lo), Some(hi)) if hi >= lo => {
                                let span = hi - lo + 1;
                                if span <= RANGE_EXPANSION_CAP {
                                    let mut arm_body: Vec<ScgStatement> = Vec::new();
                                    let _ = flatten_expr(&arm.body, &mut arm_body, ctx);
                                    for v in lo..=hi {
                                        switch_arms.push(SwitchArm {
                                            value: v,
                                            body: arm_body.clone(),
                                        });
                                    }
                                } else {
                                    vuma_log!(
                                        warn,
                                        "match arm at span {:?} uses a range pattern \
                                         {}..={} with span {} exceeding the expansion \
                                         cap {}; arm body dropped (use explicit case \
                                         arms or split the range).",
                                        arm.span,
                                        lo,
                                        hi,
                                        span,
                                        RANGE_EXPANSION_CAP
                                    );
                                    saw_complex_pattern = true;
                                }
                            }
                            _ => {
                                vuma_log!(
                                    warn,
                                    "match arm at span {:?} uses a non-integer range \
                                     pattern; arm body dropped (only integer ranges \
                                     are supported).",
                                    arm.span
                                );
                                saw_complex_pattern = true;
                            }
                        }
                    }
                    vuma_parser::ast::MatchPattern::Or { patterns, .. } => {
                        // Or-pattern: `1 | 2 | 3`. Recursively lower each
                        // sub-pattern. Lit sub-patterns become SwitchArms
                        // sharing the same body. Wildcard sub-patterns set
                        // default_body. Non-Lit/non-Wildcard sub-patterns
                        // (nested Range/Or/Ident/Struct/Enum) cause the
                        // whole arm to be dropped with a warning.
                        let mut sub_values: Vec<i64> = Vec::new();
                        let mut has_wildcard = false;
                        let mut has_nested_complex = false;
                        for sub in patterns {
                            match sub {
                                vuma_parser::ast::MatchPattern::Lit { value, .. } => {
                                    match lit_as_i64(value) {
                                        Some(v) => sub_values.push(v),
                                        None => has_nested_complex = true,
                                    }
                                }
                                vuma_parser::ast::MatchPattern::Wildcard(_) => {
                                    has_wildcard = true;
                                }
                                _ => {
                                    has_nested_complex = true;
                                }
                            }
                        }
                        if has_nested_complex {
                            vuma_log!(
                                warn,
                                "match arm at span {:?} uses an or-pattern with nested \
                                 complex sub-patterns (Range/Ident/Struct/Enum); arm \
                                 body dropped. Only Lit and Wildcard sub-patterns are \
                                 supported inside or-patterns.",
                                arm.span
                            );
                            saw_complex_pattern = true;
                        } else {
                            let mut arm_body: Vec<ScgStatement> = Vec::new();
                            let _ = flatten_expr(&arm.body, &mut arm_body, ctx);
                            if has_wildcard {
                                default_body = arm_body.clone();
                            }
                            for v in sub_values {
                                switch_arms.push(SwitchArm {
                                    value: v,
                                    body: arm_body.clone(),
                                });
                            }
                        }
                    }
                    vuma_parser::ast::MatchPattern::Ident { name, .. } => {
                        // Identifier pattern. Without type info we can't
                        // distinguish a binding (`x => ...`) from a unit-
                        // variant name (`None => ...`). We use Rust's
                        // convention: lowercase/underscore = binding;
                        // uppercase = unit-variant name.
                        let looks_like_variant = name
                            .chars()
                            .next()
                            .map(|c| c.is_uppercase())
                            .unwrap_or(false);
                        if looks_like_variant {
                            vuma_log!(
                                warn,
                                "match arm at span {:?} uses an identifier pattern \
                                 `{}` that looks like a unit-variant name; arm body \
                                 dropped (the discriminant value of a user-defined \
                                 enum variant cannot be determined without type \
                                 information).",
                                arm.span,
                                name
                            );
                            saw_complex_pattern = true;
                        } else {
                            // Treat as a binding: `x => ...` matches any
                            // value and binds the discriminant to `x`.
                            // Emit a copy Computation (`x = discriminant + 0`)
                            // so the body can reference `x` via ScgExpr::Var(x).
                            let mut arm_body: Vec<ScgStatement> = Vec::new();
                            arm_body.push(ScgStatement::Computation(ComputationNode {
                                dst: name.clone(),
                                op: BinOpKind::Add,
                                lhs: discriminant.clone(),
                                rhs: ScgExpr::Int(0),
                                tail_call: false,
                                reassigns: None,
                            }));
                            let _ = flatten_expr(&arm.body, &mut arm_body, ctx);
                            default_body = arm_body;
                            vuma_log!(
                                debug,
                                "match arm at span {:?}: identifier pattern `{}` \
                                 treated as a binding (matches any value, binds \
                                 discriminant to `{}`).",
                                arm.span,
                                name,
                                name
                            );
                        }
                    }
                    vuma_parser::ast::MatchPattern::Enum { name, binding, .. } => {
                        // Enum variant pattern: `Some(v)` or `None`.
                        // Without type info we can't determine the variant's
                        // discriminant value. If a binding is present (e.g.
                        // `Some(v)`), treat it as a binding pattern (binds
                        // the discriminant to `v`). If no binding (e.g.
                        // `None`), drop with a warning.
                        if let Some(b) = binding {
                            let mut arm_body: Vec<ScgStatement> = Vec::new();
                            arm_body.push(ScgStatement::Computation(ComputationNode {
                                dst: b.clone(),
                                op: BinOpKind::Add,
                                lhs: discriminant.clone(),
                                rhs: ScgExpr::Int(0),
                                tail_call: false,
                                reassigns: None,
                            }));
                            let _ = flatten_expr(&arm.body, &mut arm_body, ctx);
                            default_body = arm_body;
                            vuma_log!(
                                debug,
                                "match arm at span {:?}: enum variant pattern \
                                 `{}`({}) treated as a binding (matches any value, \
                                 binds discriminant to `{}`). Variant discriminant \
                                 matching is not supported without type info.",
                                arm.span,
                                name,
                                b,
                                b
                            );
                        } else {
                            vuma_log!(
                                warn,
                                "match arm at span {:?} uses a unit-variant enum \
                                 pattern `{}`; arm body dropped (the discriminant \
                                 value of a user-defined enum variant cannot be \
                                 determined without type information).",
                                arm.span,
                                name
                            );
                            saw_complex_pattern = true;
                        }
                    }
                    vuma_parser::ast::MatchPattern::Struct { name, .. } => {
                        vuma_log!(
                            warn,
                            "match arm at span {:?} uses a struct pattern `{}`; arm \
                             body dropped (struct destructuring requires type \
                             information not available in the direct AST→codegen \
                             bridge).",
                            arm.span,
                            name
                        );
                        saw_complex_pattern = true;
                    }
                }

                // Guard check: if this arm has a guard, flag it. Guards are
                // silently IGNORED — the body is lowered as if the guard
                // were absent. This is incorrect (the body should only
                // execute when both the pattern matches AND the guard is
                // true), but the Switch node has no guard slot. Proper
                // guard lowering would require rewriting the match as an
                // if/else chain.
                if arm.guard.is_some() {
                    saw_guard = true;
                }
            }

            if saw_complex_pattern {
                vuma_log!(
                    warn,
                    "match statement at span {:?} contains one or more unsupported \
                     arm patterns (see preceding warnings). Only literal-integer, \
                     wildcard, range (≤32 values), or-pattern (Lit/Wildcard \
                     sub-patterns), and identifier/enum-binding patterns were \
                     lowered; other arm bodies were dropped.",
                    match_stmt.span
                );
            }
            if saw_guard {
                vuma_log!(
                    warn,
                    "match statement at span {:?} has one or more guarded arms \
                     (`pat if cond => ...`); guards are not supported by the \
                     ControlNode::Switch lowering and are SILENTLY IGNORED. \
                     The body of a guarded arm will execute whenever the pattern \
                     matches, regardless of the guard condition — this is \
                     incorrect. Rewrite the match as an if/else chain to get \
                     correct guard semantics.",
                    match_stmt.span
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
        // Concurrency primitive. Emit a basic mutex/fence using
        // a per-sync-block stack-allocated lock byte and AtomicStore
        // acquire/release operations. This provides memory-ordering
        // semantics (release-store before and after the body — both emit
        // native atomic instructions on each backend: stlr/ldaxr on ARM,
        // lock-prefixed xchg on x86, etc.).
        //
        // NOTE: this is NOT a true mutex. The lock byte is never contended
        // (no spin-loop, no CAS), so it does not enforce mutual exclusion
        // across threads. For single-threaded code with memory-ordering
        // needs, this is sufficient. For multi-threaded code, use the
        // `AtomicCas` intrinsic directly. A `vuma_log!(warn, ...)` is
        // emitted to remind the user of this limitation.
        PStmt::Sync(sync_block) => {
            vuma_log!(
                warn,
                "sync {{ ... }} block at span {:?} lowered with a stack-allocated \
                 lock byte and AtomicStore acquire/release (no contention handling). \
                 This provides memory-ordering semantics but does NOT enforce \
                 mutual exclusion across threads.",
                sync_block.span
            );

            // Allocate a unique lock-byte stack slot. `alloc_temp` returns
            // a unique name like `__tN` — we use it directly as the lock
            // variable name (the AtomicStore args make its role clear).
            let lock_name = ctx.alloc_temp();
            let mut stmts = vec![
                // Stack-allocate 8 bytes for the lock (I64 to match
                // AtomicStore's U64 ty).
                ScgStatement::Allocation(AllocationNode::Stack {
                    name: lock_name.clone(),
                    size: 8,
                    ty: ScgType::I64,
                }),
                // Initialize lock to 0 (unlocked).
                ScgStatement::Access(AccessNode::Store {
                    ptr: ScgExpr::Var(lock_name.clone()),
                    offset: None,
                    value: ScgExpr::Int(0),
                    ty: Some(vuma_codegen::ir::IRType::U64),
                }),
                // Acquire: AtomicStore(1, &lock). On most backends this
                // emits a release-fence + atomic store (e.g., stlr on
                // ARM64, lock xchg on x86_64). The lock value (1) is
                // arbitrary — what matters is the memory barrier.
                ScgStatement::Call(CallNode {
                    dst: None,
                    func: "AtomicStore".to_string(),
                    args: vec![ScgExpr::Int(1), ScgExpr::Var(lock_name.clone())],
                    is_extern: false,
                    reassigns: None,
                }),
            ];

            // Lower the body inline between acquire and release.
            stmts.extend(bridge_block_to_scg_stmts(&sync_block.body, ctx));

            // Release: AtomicStore(0, &lock). Provides the release-fence
            // semantics on the back-edge out of the sync block.
            stmts.push(ScgStatement::Call(CallNode {
                dst: None,
                func: "AtomicStore".to_string(),
                args: vec![ScgExpr::Int(0), ScgExpr::Var(lock_name.clone())],
                is_extern: false,
                reassigns: None,
            }));

            stmts
        }

        // ── unsafe { body } ──
        // A scoping marker; lower the body inline. The unsafe contract is
        // the programmer's responsibility — no special handling needed.
        PStmt::UnsafeBlock { body, .. } => bridge_block_to_scg_stmts(body, ctx),

        // BD directives (bd/repd/capd/reld) are annotations consumed by
        // the BD inference pass — they produce no codegen statements.
        PStmt::BdDirective(_) => vec![],

        // TransformCall is parsed-but-not-emitted (the parser produces
        // Stmt::Let with a function-call RHS for transform invocations).
        // Stub: emit no statements so the build
        // does not crash; a follow-up will lower this properly.
        PStmt::TransformCall(_) => vec![],
    }
}
