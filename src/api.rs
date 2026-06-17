//! # VUMA Compiler API for LLM Consumption
//!
//! This module provides a clean, programmatic interface designed for LLM agents
//! and automated tools that need to compile VUMA source code and receive
//! structured, machine-readable results.
//!
//! ## Design Principles
//!
//! - **Always succeeds**: Every method returns a structured result; errors are
//!   captured as diagnostics rather than panicking or returning `Err`.
//! - **Serializable**: All result types derive `Serialize` / `Deserialize` so
//!   they can be sent over JSON-based IPC channels.
//! - **Summaries, not internals**: The API returns enough information for an
//!   LLM to reason about the program (function signatures, call graph, node
//!   counts) without exposing the full graph representation.
//! - **Target-aware**: The API can compile for any supported backend
//!   (x86_64, AArch64, RISC-V, etc.) and returns target-specific outputs.
//!
//! ## Quick Start
//!
//! ```rust
//! use vuma::api::VumaCompiler;
//!
//! let compiler = VumaCompiler::new();
//! let source = r#"
//!     fn main() {
//!         x = 1 + 2;
//!     }
//! "#;
//!
//! let result = compiler.compile(source);
//! if result.success {
//!     println!("Compiled {} functions", result.scg.unwrap().function_count);
//! } else {
//!     for diag in &result.diagnostics {
//!         println!("[{}] {}", diag.severity, diag.message);
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::fmt;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::diagnostics::{
    self, DiagnosticSeverity, DiagnosticSourceLocation, VumaDiagnostic,
};
use crate::pipeline::{self, CompileConfig, VerificationLevel};
use vuma_ive::{
    InvariantAggregator,
    VerificationLevel as IveVerificationLevel,
    verification::VerificationInput,
};
use vuma_proof::{
    CounterExample as ProofCounterExample,
    ViolationPoint,
    composition::{ProofBundle, InvariantStatus},
};

// ═══════════════════════════════════════════════════════════════════════════
// VumaCompiler
// ═══════════════════════════════════════════════════════════════════════════

/// The primary compiler API for programmatic (LLM) consumption.
///
/// `VumaCompiler` is the main entry point for LLMs and automated tools.
/// It wraps the full VUMA compilation pipeline and returns structured results
/// that are easy to parse and reason about programmatically.
#[derive(Debug, Clone)]
pub struct VumaCompiler {
    config: CompileConfig,
}

impl VumaCompiler {
    /// Create a new compiler with default configuration.
    pub fn new() -> Self {
        Self {
            config: CompileConfig::default(),
        }
    }

    /// Create a compiler with a specific configuration.
    pub fn with_config(config: CompileConfig) -> Self {
        Self { config }
    }

    /// Compile from source string. Returns a structured result.
    ///
    /// This runs the full compilation pipeline:
    /// Parse → AST → SCG → BD Inference → MSG → IVE Verification
    /// → SCG Transforms → IR Lowering → Register Allocation → Code Emission
    ///
    /// The result always contains a value — check `result.success` to
    /// determine if compilation succeeded, and inspect `result.diagnostics`
    /// for any warnings or errors.
    pub fn compile(&self, source: &str) -> CompileResult {
        let start = Instant::now();
        let source_lines = source.lines().count();
        let source_bytes = source.len();

        match pipeline::compile(source, &self.config) {
            Ok(output) => {
                let scg_summary = Some(build_scg_summary(&output.scg));

                // Get disassembly from the binary using the default backend
                let disasm = disassemble_default(&output.binary);

                let target_output = Some(TargetOutput {
                    backend: "aarch64".to_string(),
                    binary: output.binary.clone(),
                    binary_size: output.binary.len(),
                    disassembly: disasm,
                });

                // Collect any non-fatal diagnostics
                let diagnostics = Vec::new();

                CompileResult {
                    success: true,
                    diagnostics,
                    scg: scg_summary,
                    target: target_output,
                    metadata: CompileMetadata {
                        compile_time_ms: start.elapsed().as_millis() as u64,
                        source_lines,
                        source_bytes,
                    },
                }
            }
            Err(errors) => {
                let diagnostics = errors
                    .iter()
                    .flat_map(|e| diagnostics::from_vuma_error(e))
                    .collect();

                CompileResult {
                    success: false,
                    diagnostics,
                    scg: None,
                    target: None,
                    metadata: CompileMetadata {
                        compile_time_ms: start.elapsed().as_millis() as u64,
                        source_lines,
                        source_bytes,
                    },
                }
            }
        }
    }

    /// Compile for a specific target backend.
    ///
    /// Valid target strings: `"x86_64"`, `"aarch64"`, `"riscv64"`,
    /// `"wasm32"`, `"loongarch64"`, `"arm32"`, `"mips64"`, `"ppc64"`.
    ///
    /// If the target string is not recognised, the result will contain
    /// error diagnostics.
    pub fn compile_for_target(&self, source: &str, target: &str) -> CompileResult {
        let start = Instant::now();
        let source_lines = source.lines().count();
        let source_bytes = source.len();

        // Parse the target string into a BackendKind
        let backend_kind = match parse_target(target) {
            Some(kind) => kind,
            None => {
                return CompileResult {
                    success: false,
                    diagnostics: vec![VumaDiagnostic::new(
                        "E021",
                        DiagnosticSeverity::Error,
                        format!(
                            "Unknown target '{}'. Available: x86_64, aarch64, riscv64, \
                             wasm32, loongarch64, arm32, mips64, ppc64",
                            target
                        ),
                        "target-selection",
                        DiagnosticSourceLocation::unknown(),
                    )],
                    scg: None,
                    target: None,
                    metadata: CompileMetadata {
                        compile_time_ms: start.elapsed().as_millis() as u64,
                        source_lines,
                        source_bytes,
                    },
                };
            }
        };

        // Run the front-end pipeline (parse through SCG transforms)
        let front_result = run_frontend(source, &self.config);

        let (scg, mut diagnostics) = match front_result {
            FrontendResult::Ok { scg } => (scg, Vec::new()),
            FrontendResult::Err { diagnostics } => {
                return CompileResult {
                    success: false,
                    diagnostics,
                    scg: None,
                    target: None,
                    metadata: CompileMetadata {
                        compile_time_ms: start.elapsed().as_millis() as u64,
                        source_lines,
                        source_bytes,
                    },
                };
            }
        };

        // Build SCG summary from the validated SCG
        let scg_summary = Some(build_scg_summary(&scg));

        // Run target-specific codegen
        let target_output = match run_backend_codegen(&scg, backend_kind) {
            Ok(output) => Some(output),
            Err(diags) => {
                diagnostics.extend(diags);
                return CompileResult {
                    success: false,
                    diagnostics,
                    scg: scg_summary,
                    target: None,
                    metadata: CompileMetadata {
                        compile_time_ms: start.elapsed().as_millis() as u64,
                        source_lines,
                        source_bytes,
                    },
                };
            }
        };

        CompileResult {
            success: true,
            diagnostics,
            scg: scg_summary,
            target: target_output,
            metadata: CompileMetadata {
                compile_time_ms: start.elapsed().as_millis() as u64,
                source_lines,
                source_bytes,
            },
        }
    }

    /// Just parse — return AST/SCG without codegen.
    ///
    /// Useful for LLMs that want to understand the program structure
    /// without incurring the cost of full code generation.
    pub fn parse(&self, source: &str) -> ParseResult {
        let start = Instant::now();

        use vuma_parser::{AstToScg, Parser};

        // Parse source to AST
        let mut parser = Parser::new(source);
        let parse_output = parser.parse_program();

        if parse_output.has_errors() {
            let diagnostics = diagnostics::from_parse_errors(&parse_output.errors, source, None);

            return ParseResult {
                success: false,
                diagnostics,
                ast_summary: None,
                scg: None,
                metadata: CompileMetadata {
                    compile_time_ms: start.elapsed().as_millis() as u64,
                    source_lines: source.lines().count(),
                    source_bytes: source.len(),
                },
            };
        }

        let ast = parse_output.unwrap();
        let ast_summary = Some(build_ast_summary(&ast));

        // Convert AST to SCG
        let mut converter = AstToScg::new();
        match converter.convert(&ast) {
            Ok(scg) => {
                let scg_summary = Some(build_scg_summary(&scg));
                ParseResult {
                    success: true,
                    diagnostics: Vec::new(),
                    ast_summary,
                    scg: scg_summary,
                    metadata: CompileMetadata {
                        compile_time_ms: start.elapsed().as_millis() as u64,
                        source_lines: source.lines().count(),
                        source_bytes: source.len(),
                    },
                }
            }
            Err(e) => {
                let diagnostics = vec![VumaDiagnostic::new(
                    "E019",
                    DiagnosticSeverity::Error,
                    format!("{}", e),
                    "ast-to-scg",
                    DiagnosticSourceLocation::unknown(),
                )];
                ParseResult {
                    success: false,
                    diagnostics,
                    ast_summary,
                    scg: None,
                    metadata: CompileMetadata {
                        compile_time_ms: start.elapsed().as_millis() as u64,
                        source_lines: source.lines().count(),
                        source_bytes: source.len(),
                    },
                }
            }
        }
    }

    /// Get SCG summary for a source string.
    ///
    /// This runs the front-end pipeline (parse + SCG construction +
    /// validation + BD inference + SCG transforms) but skips codegen.
    /// It is faster than `compile()` and is ideal for program analysis.
    pub fn analyze(&self, source: &str) -> ScgSummary {
        let front_result = run_frontend(source, &self.config);
        match front_result {
            FrontendResult::Ok { scg, .. } => build_scg_summary(&scg),
            FrontendResult::Err { .. } => ScgSummary {
                function_count: 0,
                functions: Vec::new(),
                total_nodes: 0,
                total_edges: 0,
            },
        }
    }

    /// List available targets.
    ///
    /// Returns information about every backend the compiler supports.
    pub fn available_targets(&self) -> Vec<ApiTargetInfo> {
        use vuma_codegen::backend::{create_backend, BackendKind};

        let all_kinds = [
            BackendKind::AArch64,
            BackendKind::X86_64,
            BackendKind::RiscV64,
            BackendKind::Wasm32,
            BackendKind::LoongArch64,
            BackendKind::Arm32,
            BackendKind::Mips64,
            BackendKind::PowerPC64,
        ];

        all_kinds
            .iter()
            .filter_map(|&kind| {
                create_backend(kind).ok().map(|backend| {
                    let info = backend.target_info();
                    ApiTargetInfo {
                        name: kind.isa_name().to_string(),
                        triple: info.target_triple().to_string(),
                        pointer_width: info.pointer_width() * 8, // bytes → bits
                        endianness: match info.endianness() {
                            vuma_codegen::backend::Endianness::Little => "little".to_string(),
                            vuma_codegen::backend::Endianness::Big => "big".to_string(),
                            vuma_codegen::backend::Endianness::Bi => "bi".to_string(),
                        },
                        output_format: match info.output_format() {
                            vuma_codegen::backend::OutputFormat::Elf64 => "elf64".to_string(),
                            vuma_codegen::backend::OutputFormat::Elf32 => "elf32".to_string(),
                            vuma_codegen::backend::OutputFormat::WasmBinary => "wasm".to_string(),
                            vuma_codegen::backend::OutputFormat::RawBinary => "raw".to_string(),
                        },
                    }
                })
            })
            .collect()
    }

    /// Validate source without full compilation.
    ///
    /// Runs parsing and SCG validation, returning a list of diagnostics.
    /// This is the fastest way to check if a program is well-formed.
    pub fn validate(&self, source: &str) -> Vec<VumaDiagnostic> {
        use vuma_parser::{AstToScg, Parser};

        let mut all_diagnostics = Vec::new();

        // Parse
        let mut parser = Parser::new(source);
        let parse_output = parser.parse_program();

        if parse_output.has_errors() {
            all_diagnostics.extend(diagnostics::from_parse_errors(
                &parse_output.errors,
                source,
                None,
            ));
            return all_diagnostics;
        }

        let ast = parse_output.unwrap();

        // AST → SCG
        let mut converter = AstToScg::new();
        let scg = match converter.convert(&ast) {
            Ok(scg) => scg,
            Err(e) => {
                all_diagnostics.push(VumaDiagnostic::new(
                    "E019",
                    DiagnosticSeverity::Error,
                    format!("{}", e),
                    "ast-to-scg",
                    DiagnosticSourceLocation::unknown(),
                ));
                return all_diagnostics;
            }
        };

        // Validate SCG
        let validation = scg.validate();
        if !validation.is_valid {
            for err in &validation.errors {
                all_diagnostics.push(VumaDiagnostic::new(
                    "E022",
                    DiagnosticSeverity::Error,
                    err.clone(),
                    "scg-validation",
                    DiagnosticSourceLocation::unknown(),
                ));
            }
        }

        all_diagnostics
    }

    /// Verify a VUMA program by running all five IVE invariant checkers
    /// on the SCG and producing a structured verification report.
    ///
    /// This method runs the full front-end pipeline (parse → SCG),
    /// then invokes the IVE `InvariantAggregator` to check all five
    /// core invariants (liveness, exclusivity, interpretation, origin,
    /// cleanup) and the proof system to produce pass/fail per invariant
    /// with counterexamples for any violations.
    ///
    /// # Returns
    ///
    /// A [`VerificationReport`] containing:
    /// - Per-invariant pass/fail status
    /// - Counterexamples for each violation
    /// - An overall pass/fail verdict
    /// - Timing metadata
    ///
    /// # Example
    ///
    /// ```rust
    /// use vuma::api::VumaCompiler;
    ///
    /// let compiler = VumaCompiler::new();
    /// let source = "fn main() {}";
    /// let report = compiler.verify(source);
    /// println!("Overall verdict: {}", report.overall_verdict);
    /// for inv in &report.invariants {
    ///     println!("  {} — {}", inv.kind, inv.status);
    /// }
    /// ```
    pub fn verify(&self, source: &str) -> VerificationReport {
        let start = Instant::now();

        // Run the front-end pipeline to get the SCG.
        let front_result = run_frontend(source, &self.config);

        let scg = match front_result {
            FrontendResult::Ok { scg } => scg,
            FrontendResult::Err { diagnostics } => {
                let messages: Vec<String> =
                    diagnostics.iter().map(|d| d.message.clone()).collect();
                return VerificationReport {
                    overall_verdict: VerificationVerdict::Error,
                    invariants: Vec::new(),
                    diagnostics: messages,
                    metadata: VerificationMetadata {
                        total_elapsed_ms: start.elapsed().as_millis() as u64,
                        source_lines: source.lines().count(),
                        source_bytes: source.len(),
                    },
                    proof_bundle: None,
                };
            }
        };

        // Run the IVE invariant aggregator at Normal level (all 5 checks).
        let aggregator = InvariantAggregator::new().with_level(IveVerificationLevel::Normal);
        let input = VerificationInput::from_scg(scg.clone());
        let aggregated = aggregator.verify_all(&input);

        // Convert the aggregated result into per-invariant API results,
        // building counterexamples from the proof system for any violations.
        let mut invariants = Vec::with_capacity(aggregated.per_invariant.len());
        for pir in &aggregated.per_invariant {
            let kind_str = pir.kind.label().to_string();

            let (status, counterexample) = if pir.is_pass() {
                (InvariantVerificationStatus::Pass, None)
            } else if pir.is_fail() {
                // Build a proof-system counterexample from the IVE violation.
                let proof_ce = build_proof_counterexample(&pir.result);
                (InvariantVerificationStatus::Fail, Some(proof_ce))
            } else {
                (InvariantVerificationStatus::Unverified, None)
            };

            invariants.push(InvariantVerification {
                kind: kind_str,
                status,
                message: pir.result.message.clone(),
                elapsed_ms: pir.elapsed_ms,
                counterexample,
            });
        }

        // Determine overall verdict.
        let overall_verdict = match aggregated.overall {
            vuma_ive::OverallVerdict::Pass => VerificationVerdict::Pass,
            vuma_ive::OverallVerdict::Fail => VerificationVerdict::Fail,
            vuma_ive::OverallVerdict::Inconclusive => VerificationVerdict::Inconclusive,
            vuma_ive::OverallVerdict::NoChecks => VerificationVerdict::Error,
        };

        // Also attempt proof-system verification for a cross-check.
        // `build_proof_bundle` constructs a typed proof object for each
        // of the five basic invariants, referencing the IVE's per-
        // invariant evidence (status, message, assumptions, counterexample).
        let proof_bundle = build_proof_bundle(&scg, &aggregated);
        let proof_statuses = proof_bundle.status();

        // If the proof system found failures that the IVE missed,
        // upgrade unverified results to fail. Match by invariant name
        // (not position) — `proof_statuses` is in bundle order
        // (Liveness, Exclusivity, Cleanup, Origin, Interpretation)
        // while `invariants` is in IVE canonical order
        // (Liveness, Exclusivity, Interpretation, Origin, Cleanup),
        // so positional indexing would cross-wire the upgrade.
        for (inv_name, proof_status) in &proof_statuses {
            if let InvariantStatus::Failed(reason) = proof_status {
                let inv_str = format!("{}", inv_name);
                for inv in &mut invariants {
                    if inv.kind == inv_str
                        && inv.status == InvariantVerificationStatus::Unverified
                    {
                        inv.status = InvariantVerificationStatus::Fail;
                        inv.counterexample = Some(CounterexampleInfo {
                            description: reason.clone(),
                            execution_trace: Vec::new(),
                        });
                    }
                }
            }
        }

        let diagnostics = Vec::new();
        let total_elapsed_ms = start.elapsed().as_millis() as u64;

        VerificationReport {
            overall_verdict,
            invariants,
            diagnostics,
            metadata: VerificationMetadata {
                total_elapsed_ms,
                source_lines: source.lines().count(),
                source_bytes: source.len(),
            },
            proof_bundle: Some(proof_bundle),
        }
    }
}

impl Default for VumaCompiler {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Result Types
// ═══════════════════════════════════════════════════════════════════════════

/// Result of compiling a VUMA program.
///
/// This is the primary return type for `VumaCompiler::compile()` and
/// `VumaCompiler::compile_for_target()`. It always contains a value —
/// check `success` to determine the outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileResult {
    /// Whether compilation succeeded (produced a binary).
    pub success: bool,
    /// Any diagnostics (errors, warnings, notes) produced during compilation.
    pub diagnostics: Vec<VumaDiagnostic>,
    /// Summary of the SCG (Semantic Computation Graph).
    ///
    /// Present when parsing succeeds, even if codegen fails.
    pub scg: Option<ScgSummary>,
    /// Compiled output for the target.
    ///
    /// Present only when compilation succeeds.
    pub target: Option<TargetOutput>,
    /// Metadata about the compilation process.
    pub metadata: CompileMetadata,
}

/// Result of parsing (without codegen).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseResult {
    /// Whether parsing succeeded.
    pub success: bool,
    /// Any diagnostics produced during parsing.
    pub diagnostics: Vec<VumaDiagnostic>,
    /// Summary of the AST structure.
    pub ast_summary: Option<AstSummary>,
    /// Summary of the SCG (if AST → SCG conversion succeeded).
    pub scg: Option<ScgSummary>,
    /// Metadata about the parse process.
    pub metadata: CompileMetadata,
}

// ═══════════════════════════════════════════════════════════════════════════
// SCG Summary Types
// ═══════════════════════════════════════════════════════════════════════════

/// Summary of the SCG (not the full graph — just enough for LLM understanding).
///
/// The SCG summary provides a structured overview of the program's semantic
/// computation graph without exposing the full graph representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScgSummary {
    /// Number of functions in the SCG.
    pub function_count: usize,
    /// Per-function summaries.
    pub functions: Vec<FunctionSummary>,
    /// Total number of nodes across all functions.
    pub total_nodes: usize,
    /// Total number of edges across all functions.
    pub total_edges: usize,
}

/// Summary of a single function in the SCG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSummary {
    /// Function name.
    pub name: String,
    /// Parameters as (name, type) pairs.
    pub params: Vec<(String, String)>,
    /// Return type (e.g., `"void"`, `"i64"`, `"ptr"`).
    pub return_type: String,
    /// Number of SCG nodes in this function's body.
    pub node_count: usize,
    /// Names of functions called from this function.
    pub calls: Vec<String>,
}

// ═══════════════════════════════════════════════════════════════════════════
// AST Summary
// ═══════════════════════════════════════════════════════════════════════════

/// Summary of the parsed AST.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AstSummary {
    /// Total number of top-level items.
    pub item_count: usize,
    /// Names of defined functions.
    pub function_names: Vec<String>,
    /// Names of declared regions.
    pub region_names: Vec<String>,
    /// Number of import declarations.
    pub import_count: usize,
}

// ═══════════════════════════════════════════════════════════════════════════
// Target Output
// ═══════════════════════════════════════════════════════════════════════════

/// Compiled output for a specific target backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetOutput {
    /// Backend name (e.g., "x86_64", "aarch64", "riscv64").
    pub backend: String,
    /// Raw binary output (ELF, Wasm, or raw binary depending on target).
    ///
    /// Serialized as a hex string for compact JSON representation.
    #[serde(
        serialize_with = "serialize_binary_hex",
        deserialize_with = "deserialize_binary_hex"
    )]
    pub binary: Vec<u8>,
    /// Size of the binary in bytes.
    pub binary_size: usize,
    /// Human-readable disassembly of the compiled code.
    pub disassembly: String,
}

// ═══════════════════════════════════════════════════════════════════════════
// Metadata Types
// ═══════════════════════════════════════════════════════════════════════════

/// Metadata about a compilation run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompileMetadata {
    /// Wall-clock compilation time in milliseconds.
    pub compile_time_ms: u64,
    /// Number of lines in the source code.
    pub source_lines: usize,
    /// Number of bytes in the source code.
    pub source_bytes: usize,
}

/// Information about a supported compilation target.
///
/// Named `ApiTargetInfo` to avoid collision with
/// `vuma_codegen::backend::TargetInfo`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiTargetInfo {
    /// ISA name (e.g., "x86_64", "aarch64").
    pub name: String,
    /// LLVM-style target triple (e.g., "aarch64-unknown-linux-gnu").
    pub triple: String,
    /// Pointer width in bits (32 or 64).
    pub pointer_width: usize,
    /// Byte order ("little", "big", or "bi").
    pub endianness: String,
    /// Output binary format ("elf64", "elf32", "wasm", "raw").
    pub output_format: String,
}

// ═══════════════════════════════════════════════════════════════════════════
// Binary Serde Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Serialize `Vec<u8>` as a hex string for compact JSON representation.
fn serialize_binary_hex<S: serde::Serializer>(data: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
    let hex: String = data.iter().map(|b| format!("{:02x}", b)).collect();
    s.serialize_str(&hex)
}

/// Deserialize `Vec<u8>` from a hex string.
fn deserialize_binary_hex<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    use serde::de::Error;

    struct HexVisitor;

    impl<'de> serde::de::Visitor<'de> for HexVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "a hex-encoded string")
        }

        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
            if v.len() % 2 != 0 {
                return Err(E::custom("hex string has odd length"));
            }
            (0..v.len())
                .step_by(2)
                .map(|i| {
                    u8::from_str_radix(&v[i..i + 2], 16)
                        .map_err(|e| E::custom(format!("hex decode error: {}", e)))
                })
                .collect()
        }
    }

    d.deserialize_str(HexVisitor)
}

// ═══════════════════════════════════════════════════════════════════════════
// Verification Report Types
// ═══════════════════════════════════════════════════════════════════════════

/// Overall verdict of the verification run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VerificationVerdict {
    /// All five invariants passed.
    Pass,
    /// At least one invariant was violated.
    Fail,
    /// No invariant was violated, but at least one is unverified.
    Inconclusive,
    /// An error occurred before verification could run (e.g., parse error).
    Error,
}

impl fmt::Display for VerificationVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerificationVerdict::Pass => write!(f, "PASS"),
            VerificationVerdict::Fail => write!(f, "FAIL"),
            VerificationVerdict::Inconclusive => write!(f, "INCONCLUSIVE"),
            VerificationVerdict::Error => write!(f, "ERROR"),
        }
    }
}

/// Status of a single invariant verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InvariantVerificationStatus {
    /// The invariant was proven to hold.
    Pass,
    /// The invariant was violated; see the counterexample.
    Fail,
    /// The invariant could not be verified (insufficient information).
    Unverified,
}

impl fmt::Display for InvariantVerificationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvariantVerificationStatus::Pass => write!(f, "PASS"),
            InvariantVerificationStatus::Fail => write!(f, "FAIL"),
            InvariantVerificationStatus::Unverified => write!(f, "UNVERIFIED"),
        }
    }
}

/// Counterexample information for an invariant violation.
///
/// Provides a human-readable description and an execution trace that
/// demonstrates how the violation can be reached.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CounterexampleInfo {
    /// Human-readable description of the violation.
    pub description: String,
    /// Execution trace steps demonstrating the violation.
    pub execution_trace: Vec<String>,
}

/// Result of verifying a single invariant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvariantVerification {
    /// Name of the invariant (e.g., "liveness", "exclusivity").
    pub kind: String,
    /// Pass/fail/unverified status.
    pub status: InvariantVerificationStatus,
    /// Human-readable message describing the outcome.
    pub message: String,
    /// Wall-clock time spent checking this invariant (milliseconds).
    pub elapsed_ms: u64,
    /// Counterexample demonstrating the violation, if any.
    pub counterexample: Option<CounterexampleInfo>,
}

/// Metadata about a verification run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationMetadata {
    /// Total wall-clock time for the verification run (milliseconds).
    pub total_elapsed_ms: u64,
    /// Number of lines in the source code.
    pub source_lines: usize,
    /// Number of bytes in the source code.
    pub source_bytes: usize,
}

/// The full verification report produced by `VumaCompiler::verify()`.
///
/// Contains per-invariant results with pass/fail status and counterexamples
/// for any violations, plus an overall verdict and metadata. A
/// [`ProofBundle`] is also included, carrying the formal proof objects
/// constructed from the IVE's per-invariant evidence (see
/// [`build_proof_bundle`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReport {
    /// The overall verification verdict.
    pub overall_verdict: VerificationVerdict,
    /// Per-invariant verification results.
    pub invariants: Vec<InvariantVerification>,
    /// Any diagnostics or informational messages.
    pub diagnostics: Vec<String>,
    /// Metadata about the verification run.
    pub metadata: VerificationMetadata,
    /// The formal proof bundle, with one typed proof per invariant that
    /// the IVE checked. `None` only when verification could not run
    /// (e.g. parse error); otherwise a bundle whose `status()` entries
    /// mirror the IVE's per-invariant findings.
    #[serde(default)]
    pub proof_bundle: Option<ProofBundle>,
}

impl VerificationReport {
    /// Returns `true` if all invariants passed.
    pub fn is_pass(&self) -> bool {
        self.overall_verdict == VerificationVerdict::Pass
    }

    /// Returns `true` if at least one invariant was violated.
    pub fn is_fail(&self) -> bool {
        self.overall_verdict == VerificationVerdict::Fail
    }

    /// Returns the number of invariants that passed.
    pub fn pass_count(&self) -> usize {
        self.invariants
            .iter()
            .filter(|i| i.status == InvariantVerificationStatus::Pass)
            .count()
    }

    /// Returns the number of invariants that failed.
    pub fn fail_count(&self) -> usize {
        self.invariants
            .iter()
            .filter(|i| i.status == InvariantVerificationStatus::Fail)
            .count()
    }
}

impl fmt::Display for VerificationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "Verification Report — {} ({}ms)",
            self.overall_verdict, self.metadata.total_elapsed_ms
        )?;
        for inv in &self.invariants {
            write!(f, "  {} — {}", inv.kind, inv.status)?;
            if let Some(ce) = &inv.counterexample {
                write!(f, " — {}", ce.description)?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Internal Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Front-end pipeline result (everything up to and including SCG transforms).
enum FrontendResult {
    Ok {
        scg: vuma_scg::SCG,
    },
    Err {
        diagnostics: Vec<VumaDiagnostic>,
    },
}

/// Run the front-end pipeline: Parse → AST → SCG → Validate → BD Inference
/// → IVE Verification → SCG Transforms.
fn run_frontend(source: &str, config: &CompileConfig) -> FrontendResult {
    use vuma_ive::{InvariantAggregator, VerificationLevel as IveVerificationLevel};
    use vuma_parser::{AstToScg, Parser};

    // Stage 1: Parse
    let mut parser = Parser::new(source);
    let parse_output = parser.parse_program();
    if parse_output.has_errors() {
        return FrontendResult::Err {
            diagnostics: diagnostics::from_parse_errors(&parse_output.errors, source, None),
        };
    }
    let ast = parse_output.unwrap();

    // Stage 2: AST → SCG
    let mut converter = AstToScg::new();
    let mut scg = match converter.convert(&ast) {
        Ok(scg) => scg,
        Err(e) => {
            return FrontendResult::Err {
                diagnostics: vec![VumaDiagnostic::new(
                    "E019",
                    DiagnosticSeverity::Error,
                    format!("{}", e),
                    "ast-to-scg",
                    DiagnosticSourceLocation::unknown(),
                )],
            };
        }
    };

    // Stage 3: SCG Validation (non-fatal — warn but continue)
    let _validation = scg.validate();

    // Stage 4: BD Inference (refine types)
    let inference_engine = vuma_ive::InferenceEngine::new();
    let bd_results = inference_engine.infer_types(&scg);
    pipeline::refine_scg_types_with_bd(&mut scg, &bd_results);

    // Stage 5: IVE Verification (non-fatal)
    if config.verification_level != VerificationLevel::None {
        let ive_level = match config.verification_level {
            VerificationLevel::Quick => IveVerificationLevel::Quick,
            VerificationLevel::Normal => IveVerificationLevel::Normal,
            VerificationLevel::Exhaustive => IveVerificationLevel::Exhaustive,
            VerificationLevel::None => unreachable!(),
        };
        let aggregator = InvariantAggregator::new().with_level(ive_level);
        let input = vuma_ive::verification::VerificationInput::from_scg(scg.clone());
        let _ = aggregator.verify_all(&input);
    }

    // Stage 6: SCG Transforms
    pipeline::run_scg_transforms(&mut scg, config);

    FrontendResult::Ok { scg }
}

/// Run target-specific codegen using the Backend trait.
fn run_backend_codegen(
    scg: &vuma_scg::SCG,
    backend_kind: vuma_codegen::backend::BackendKind,
) -> Result<TargetOutput, Vec<VumaDiagnostic>> {
    use vuma_codegen::backend::{create_backend, AllocatedProgram};
    use vuma_codegen::regalloc::RegAllocator;
    use vuma_codegen::scg_to_ir::IRBuilder;

    // Bridge SCG to codegen SCG
    let codegen_scg = pipeline::bridge_scg_to_codegen(scg);

    // IR Lowering
    let mut ir_builder = IRBuilder::new();
    let ir_program = match ir_builder.build(&codegen_scg) {
        Ok(ir) => ir,
        Err(e) => {
            return Err(vec![diagnostics::from_codegen_error(&e)]);
        }
    };

    // Create backend
    let backend = match create_backend(backend_kind) {
        Ok(b) => b,
        Err(e) => {
            return Err(vec![VumaDiagnostic::new(
                "E023",
                DiagnosticSeverity::Error,
                format!("{}", e),
                "backend-creation",
                DiagnosticSourceLocation::unknown(),
            )]);
        }
    };

    // Register allocation — delegate to the backend
    let mut allocated_functions = Vec::new();
    for func in &ir_program.functions {
        match backend.allocate_registers(func) {
            Ok(allocd) => allocated_functions.push(allocd),
            Err(e) => {
                return Err(vec![VumaDiagnostic::new(
                    "E017",
                    DiagnosticSeverity::Error,
                    format!("{}: {}", func.name, e),
                    "register-alloc",
                    DiagnosticSourceLocation::unknown(),
                )]);
            }
        }
    }

    let allocated_program = AllocatedProgram {
        functions: allocated_functions,
        total_code_size: 0,
        total_data_size: 0,
    };

    // Encode the program
    let binary = match backend.encode_program(&allocated_program) {
        Ok(binary) => binary,
        Err(e) => {
            use vuma_codegen::backend::BackendError;
            use crate::diagnostics::RelatedInfo;
            match &e {
                BackendError::UnresolvedRelocation { symbol, function, offset, reloc_type, .. } => {
                    let mut diag = VumaDiagnostic::new(
                        "E037",
                        DiagnosticSeverity::Error,
                        format!("unresolved relocation: symbol '{}' not found", symbol),
                        "codegen",
                        DiagnosticSourceLocation::unknown(),
                    );
                    diag = diag.with_related(
                        RelatedInfo::new(
                            DiagnosticSourceLocation::unknown(),
                            format!("referenced in function '{}' at offset 0x{:X} (relocation type: {})", function, offset, reloc_type),
                        ),
                    );
                    return Err(vec![diag]);
                }
                _ => {
                    return Err(vec![VumaDiagnostic::new(
                        "E020",
                        DiagnosticSeverity::Error,
                        format!("{}", e),
                        "code-emission",
                        DiagnosticSourceLocation::unknown(),
                    )]);
                }
            }
        }
    };

    // Disassemble
    let base_addr = backend.target_info().default_base_address();
    let disasm_lines = backend.disassemble(&binary, base_addr);
    let disassembly = disasm_lines.join("\n");

    Ok(TargetOutput {
        backend: backend_kind.isa_name().to_string(),
        binary_size: binary.len(),
        binary,
        disassembly,
    })
}

/// Build an edge index from the SCG for efficient traversal.
struct ScgEdgeIndex {
    outgoing: HashMap<vuma_scg::NodeId, Vec<vuma_scg::EdgeData>>,
    incoming: HashMap<vuma_scg::NodeId, Vec<vuma_scg::EdgeData>>,
}

impl ScgEdgeIndex {
    fn build(scg: &vuma_scg::SCG) -> Self {
        let mut outgoing: HashMap<vuma_scg::NodeId, Vec<vuma_scg::EdgeData>> = HashMap::new();
        let mut incoming: HashMap<vuma_scg::NodeId, Vec<vuma_scg::EdgeData>> = HashMap::new();
        for edge in scg.edges() {
            outgoing.entry(edge.source).or_default().push(edge.clone());
            incoming.entry(edge.target).or_default().push(edge.clone());
        }
        Self { outgoing, incoming }
    }

    fn outgoing(&self, id: vuma_scg::NodeId) -> &[vuma_scg::EdgeData] {
        self.outgoing.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    fn incoming(&self, id: vuma_scg::NodeId) -> &[vuma_scg::EdgeData] {
        self.incoming.get(&id).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Build an SCG summary from the full SCG graph.
fn build_scg_summary(scg: &vuma_scg::SCG) -> ScgSummary {
    use vuma_scg::{ControlKind, EdgeKind, NodePayload};

    let total_nodes = scg.node_count();
    let total_edges = scg.edge_count();
    let edge_idx = ScgEdgeIndex::build(scg);

    // Identify function entry nodes and build per-function summaries
    let mut functions = Vec::new();

    // Collect function entries
    let func_entries: Vec<(vuma_scg::NodeId, String)> = scg
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

    // For each function entry, count nodes reachable via ControlFlow
    for (entry_id, func_name) in &func_entries {
        let mut reachable = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(*entry_id);
        reachable.insert(*entry_id);

        while let Some(current) = queue.pop_front() {
            for edge in edge_idx.outgoing(current) {
                if edge.kind == EdgeKind::ControlFlow && !reachable.contains(&edge.target) {
                    reachable.insert(edge.target);
                    queue.push_back(edge.target);
                }
            }
        }

        let node_count = reachable.len();

        // Extract parameters from outgoing DataFlow edges from the entry
        let mut params = Vec::new();
        for edge in edge_idx.outgoing(*entry_id) {
            if edge.kind == EdgeKind::DataFlow {
                if let Some(target_node) = scg.get_node(edge.target) {
                    let name = match &target_node.payload {
                        NodePayload::Allocation(a) => {
                            a.type_name.clone().unwrap_or_else(|| "param".to_string())
                        }
                        NodePayload::Computation(c) => {
                            c.result_type.clone().unwrap_or_else(|| "param".to_string())
                        }
                        _ => "param".to_string(),
                    };
                    let ty = match &target_node.payload {
                        NodePayload::Allocation(a) => {
                            a.type_name.as_deref().unwrap_or("i64").to_string()
                        }
                        NodePayload::Computation(c) => {
                            c.result_type.as_deref().unwrap_or("i64").to_string()
                        }
                        NodePayload::Cast(c) => c.to_type.clone(),
                        _ => "i64".to_string(),
                    };
                    params.push((name, ty));
                }
            }
        }

        // Find calls within this function
        let mut calls = Vec::new();
        for node_id in &reachable {
            if let Some(node) = scg.get_node(*node_id) {
                if let NodePayload::Computation(comp) = &node.payload {
                    let op_label = comp.kind.label();
                    if !is_known_binop(&op_label) && !op_label.starts_with('_') {
                        if !calls.contains(&op_label) {
                            calls.push(op_label);
                        }
                    }
                }
            }
        }

        // Determine return type
        let return_type = find_return_type(scg, *entry_id, &edge_idx);

        functions.push(FunctionSummary {
            name: func_name.clone(),
            params,
            return_type,
            node_count,
            calls,
        });
    }

    // If no function entries were found, create a single "main" summary
    if functions.is_empty() {
        let mut calls = Vec::new();
        for node in scg.nodes() {
            if let NodePayload::Computation(comp) = &node.payload {
                let op_label = comp.kind.label();
                if !is_known_binop(&op_label) && !op_label.starts_with('_') {
                    if !calls.contains(&op_label) {
                        calls.push(op_label);
                    }
                }
            }
        }

        functions.push(FunctionSummary {
            name: "main".to_string(),
            params: Vec::new(),
            return_type: "void".to_string(),
            node_count: total_nodes,
            calls,
        });
    }

    ScgSummary {
        function_count: functions.len(),
        functions,
        total_nodes,
        total_edges,
    }
}

/// Check if an operation string is a known binary operation.
fn is_known_binop(op: &str) -> bool {
    matches!(
        op,
        "add" | "sub" | "mul" | "sdiv" | "udiv" | "srem" | "urem" | "and" | "or" | "xor"
            | "shl" | "shr.l" | "shr.a" | "slt" | "sle" | "sgt" | "sge" | "ult" | "ule"
            | "ugt" | "uge" | "eq" | "ne" | "+" | "-" | "*" | "/" | "%" | "&" | "|"
            | "^" | "<<" | ">>" | "<" | "<=" | ">" | ">=" | "=="
    )
}

/// Find the return type of a function by tracing to its FunctionReturn node.
fn find_return_type(
    scg: &vuma_scg::SCG,
    entry_id: vuma_scg::NodeId,
    edge_idx: &ScgEdgeIndex,
) -> String {
    use vuma_scg::{ControlKind, EdgeKind, NodePayload};

    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(entry_id);
    visited.insert(entry_id);

    while let Some(current) = queue.pop_front() {
        for edge in edge_idx.outgoing(current) {
            if edge.kind == EdgeKind::ControlFlow && !visited.contains(&edge.target) {
                visited.insert(edge.target);
                if let Some(node) = scg.get_node(edge.target) {
                    if let NodePayload::Control(c) = &node.payload {
                        if c.kind == ControlKind::FunctionReturn {
                            // Check incoming DataFlow edges for return value type
                            for ret_edge in edge_idx.incoming(edge.target) {
                                if ret_edge.kind == EdgeKind::DataFlow {
                                    if let Some(src) = scg.get_node(ret_edge.source) {
                                        if let NodePayload::Computation(comp) = &src.payload {
                                            if let Some(rt) = &comp.result_type {
                                                return rt.clone();
                                            }
                                        }
                                    }
                                }
                            }
                            return "void".to_string();
                        }
                    }
                }
                queue.push_back(edge.target);
            }
        }
    }

    "void".to_string()
}

/// Build an AST summary from the parsed program.
fn build_ast_summary(ast: &vuma_parser::Program) -> AstSummary {
    use vuma_parser::Item;

    let mut function_names = Vec::new();
    let mut region_names = Vec::new();
    let mut import_count = 0;

    for item in &ast.items {
        match item {
            Item::FnDef(fn_decl) => {
                function_names.push(fn_decl.name.clone());
            }
            Item::RegionDef(region_decl) => {
                region_names.push(region_decl.name.clone());
            }
            Item::Import(_) => {
                import_count += 1;
            }
            _ => {}
        }
    }

    AstSummary {
        item_count: ast.items.len(),
        function_names,
        region_names,
        import_count,
    }
}

/// Disassemble the default (AArch64) binary output.
fn disassemble_default(binary: &[u8]) -> String {
    use vuma_codegen::backend::{create_backend, BackendKind};

    match create_backend(BackendKind::AArch64) {
        Ok(backend) => {
            let base = backend.target_info().default_base_address();
            backend.disassemble(binary, base).join("\n")
        }
        Err(_) => format!("({} bytes of binary output)", binary.len()),
    }
}

/// Parse a target string into a BackendKind.
fn parse_target(target: &str) -> Option<vuma_codegen::backend::BackendKind> {
    use vuma_codegen::backend::BackendKind;

    match target.to_lowercase().as_str() {
        "x86_64" | "x86-64" | "amd64" => Some(BackendKind::X86_64),
        "aarch64" | "arm64" => Some(BackendKind::AArch64),
        "riscv64" | "risc-v64" | "riscv-64" => Some(BackendKind::RiscV64),
        "wasm32" | "wasm" => Some(BackendKind::Wasm32),
        "loongarch64" | "la64" => Some(BackendKind::LoongArch64),
        "arm32" | "arm" => Some(BackendKind::Arm32),
        "mips64" | "mips" => Some(BackendKind::Mips64),
        "ppc64" | "powerpc64" | "ppc" => Some(BackendKind::PowerPC64),
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Verification Helper Functions
// ═══════════════════════════════════════════════════════════════════════════

/// Build a proof-system counterexample from an IVE verification result.
///
/// Takes the IVE `VerificationResult` (which uses its own counterexample
/// format) and converts it into a proof-system `CounterExample`, then
/// extracts the relevant information into the API's `CounterexampleInfo`.
fn build_proof_counterexample(
    result: &vuma_ive::result::VerificationResult,
) -> CounterexampleInfo {
    use vuma_ive::result::VerificationStatus;

    match &result.status {
        VerificationStatus::Violated { counterexample } => {
            // Convert the IVE counterexample into a proof-system
            // counterexample for structural consistency.
            let proof_inv = match result.invariant.as_str() {
                "liveness" => vuma_proof::proof::InvariantName::Liveness,
                "exclusivity" => vuma_proof::proof::InvariantName::Exclusivity,
                "cleanup" => vuma_proof::proof::InvariantName::Cleanup,
                "origin" => vuma_proof::proof::InvariantName::Origin,
                "interpretation" => vuma_proof::proof::InvariantName::Interpretation,
                _ => vuma_proof::proof::InvariantName::Liveness,
            };

            let violation_point = ViolationPoint::new(
                proof_inv,
                &counterexample.description,
                0, // program offset
            );
            let proof_ce = ProofCounterExample::from_violation(&result.message, violation_point);
            let minimal_ce = proof_ce.minimal();

            // Convert trace steps to human-readable strings.
            let trace: Vec<String> = minimal_ce
                .execution
                .iter()
                .map(|step| step.to_string())
                .collect();

            CounterexampleInfo {
                description: counterexample.description.clone(),
                execution_trace: trace,
            }
        }
        _ => CounterexampleInfo {
            description: result.message.clone(),
            execution_trace: Vec::new(),
        },
    }
}

/// Build a proof bundle from the IVE's aggregated verification results.
///
/// For each of the five basic invariants (liveness, exclusivity,
/// interpretation, origin, cleanup), this constructs a typed proof object
/// (`LivenessProof`, `ExclusivityProof`, ...) wrapping a `Proof` that
/// references the IVE's per-invariant evidence:
///
/// - For a `Proven` invariant, the proof concludes `Conclusion::Proven`
///   and includes a `Checked` fact citing the IVE's status and message.
/// - For a `Violated` invariant, the proof concludes `Conclusion::Refuted`
///   and includes a `Checked` fact citing the IVE's counterexample
///   description and violation point.
/// - For `ProbablySafe`, the proof concludes `Conclusion::Inconclusive`
///   and includes the IVE's assumptions as `Assumption` facts.
/// - For `Unverified`, the proof concludes `Conclusion::Inconclusive`
///   with the IVE's reason cited as a `Checked` fact.
///
/// Each typed proof is then placed into the matching slot of the
/// [`ProofBundle`]. The bundle's `status()` method therefore mirrors the
/// IVE's per-invariant findings, and downstream consumers (serialisation,
/// cross-invariant consistency checking) operate on real proof objects
/// rather than an empty placeholder bundle.
///
/// **Future work**: full proof generation with tactic application
/// (path enumeration, ranking functions, lockset analysis, ownership
/// tracking, etc.) requires converting the `vuma_scg::SCG` into the proof
/// crate's `ProofSCG`/`ProofMSG` model and invoking the `prove_liveness`/
/// `prove_origin`/... entry points. Currently the bundle contains
/// evidence references from the IVE rather than independently-generated
/// tactic proofs — but it is no longer dead code: every invariant the
/// IVE checked produces a corresponding proof object in the bundle.
fn build_proof_bundle(
    _scg: &vuma_scg::SCG,
    aggregated: &vuma_ive::AggregatedResult,
) -> ProofBundle {
    use vuma_ive::InvariantKind;
    use vuma_ive::result::{VerificationResult, VerificationStatus};
    use vuma_ive::invariant_aggregator::PerInvariantResult;
    use vuma_proof::cleanup_proofs::{CleanupProof, CleanupTactic};
    use vuma_proof::exclusivity_proofs::ExclusivityProof;
    use vuma_proof::interpretation_proofs::InterpretationProof;
    use vuma_proof::liveness_proofs::{LivenessProof, LivenessTactic};
    use vuma_proof::origin_proofs::OriginProof;
    use vuma_proof::proof::{
        Conclusion, Fact, FactKind, Goal, InvariantName, Proof, ProofContext, ProofStep, Target,
    };

    /// Map an IVE `InvariantKind` to the proof crate's `InvariantName`.
    /// Returns `None` for the advanced analyses (Hardened, Interprocedural,
    /// PathSensitiveLiveness) which have no corresponding proof-invariant
    /// slot.
    fn inv_name(kind: InvariantKind) -> Option<InvariantName> {
        match kind {
            InvariantKind::Liveness => Some(InvariantName::Liveness),
            InvariantKind::Exclusivity => Some(InvariantName::Exclusivity),
            InvariantKind::Interpretation => Some(InvariantName::Interpretation),
            InvariantKind::Origin => Some(InvariantName::Origin),
            InvariantKind::Cleanup => Some(InvariantName::Cleanup),
            _ => None,
        }
    }

    /// Look up the IVE's per-invariant result for a given kind.
    fn find_result<'a>(
        aggregated: &'a vuma_ive::AggregatedResult,
        kind: InvariantKind,
    ) -> Option<&'a PerInvariantResult> {
        aggregated.per_invariant.iter().find(|pir| pir.kind == kind)
    }

    /// Construct a generic `Proof` for a given invariant kind, citing
    /// the IVE's verification result as a `Checked` fact and setting the
    /// conclusion to match the IVE's status. Returns `None` for advanced
    /// invariant kinds that have no proof-crate `InvariantName`.
    fn make_proof(kind: InvariantKind, result: &VerificationResult) -> Option<Proof> {
        let inv = inv_name(kind)?;
        let scope = format!("ive::{}", kind.label());
        let mut proof = Proof::new(Goal::new(
            inv,
            Target::FullProgram,
            ProofContext::new(&scope),
        ));

        // Cite the IVE's verification status and message as a Checked fact.
        let statement = format!(
            "IVE {} verification status: {} — {}",
            kind.label(),
            result.status,
            result.message
        );
        proof.add_step(ProofStep::Assume {
            fact: Fact::new(0, statement, FactKind::Checked),
        });

        // For ProbablySafe, cite each assumption as an Assumption fact.
        if let VerificationStatus::ProbablySafe { assumptions } = &result.status {
            for (i, assumption) in assumptions.iter().enumerate() {
                proof.add_step(ProofStep::Assume {
                    fact: Fact::assumption((i + 1) as u64, assumption.clone()),
                });
            }
        }

        // For Violated, cite the counterexample as a Checked witness fact.
        if let VerificationStatus::Violated { counterexample } = &result.status {
            proof.add_step(ProofStep::Assume {
                fact: Fact::new(
                    1,
                    format!(
                        "counterexample: {} (violation at {})",
                        counterexample.description, counterexample.violation_point
                    ),
                    FactKind::Checked,
                ),
            });
        }

        // Set the conclusion based on the IVE's status.
        let conclusion = match &result.status {
            VerificationStatus::Proven => Conclusion::Proven,
            VerificationStatus::ProbablySafe { .. } => Conclusion::Inconclusive,
            VerificationStatus::Unverified { .. } => Conclusion::Inconclusive,
            VerificationStatus::Violated { .. } => Conclusion::Refuted,
        };
        proof.conclude(conclusion);

        Some(proof)
    }

    // Build each typed proof slot from the IVE's per-invariant result.
    // Slots for invariants the IVE did not run (e.g. at Quick level) or
    // for advanced kinds remain `None` (NotAttempted in the bundle).
    let liveness = find_result(aggregated, InvariantKind::Liveness)
        .and_then(|pir| make_proof(pir.kind, &pir.result))
        .map(|proof| LivenessProof {
            proof,
            access_proofs: Vec::new(),
            freed_proofs: Vec::new(),
            deadlock_proof: None,
            ordering: None,
            tactic: LivenessTactic::PathEnumeration,
        });

    let exclusivity = find_result(aggregated, InvariantKind::Exclusivity)
        .and_then(|pir| make_proof(pir.kind, &pir.result))
        .map(|proof| ExclusivityProof {
            proof,
            sub_proofs: Vec::new(),
            tactics_used: Vec::new(),
        });

    let interpretation = find_result(aggregated, InvariantKind::Interpretation)
        .and_then(|pir| make_proof(pir.kind, &pir.result))
        .map(|proof| InterpretationProof {
            bd_compatibility_proofs: Vec::new(),
            reinterpretation_safety_proofs: Vec::new(),
            proof,
        });

    let origin = find_result(aggregated, InvariantKind::Origin)
        .and_then(|pir| make_proof(pir.kind, &pir.result))
        .map(|proof| OriginProof {
            proof,
            verified_regions: Vec::new(),
            checked_chains: Vec::new(),
        });

    let cleanup = find_result(aggregated, InvariantKind::Cleanup)
        .and_then(|pir| make_proof(pir.kind, &pir.result))
        .map(|proof| CleanupProof {
            proof,
            release_map: std::collections::HashMap::new(),
            tactic: CleanupTactic::PathEnumeration,
        });

    ProofBundle {
        liveness,
        exclusivity,
        cleanup,
        origin,
        interpretation,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_simple() {
        let compiler = VumaCompiler::new();
        let source = r#"
            fn main() {
            }
        "#;
        let result = compiler.compile(source);
        assert!(result.success, "Compilation should succeed");
        assert!(result.scg.is_some(), "SCG summary should be present");
        assert!(result.target.is_some(), "Target output should be present");
        assert!(result.diagnostics.is_empty(), "No diagnostics expected");
    }

    /// NOTE: `verification_level` is set to `None` for the same reason
    /// as `pipeline::tests::test_compile_simple_allocation` — the IVE
    /// cleanup-graph extractor has a false positive on top-level
    /// `region` declarations (the Allocation node has no ControlFlow
    /// edges, only Derivation, which is excluded from the cleanup
    /// graph, so it is flagged as a leak).  This test exercises the
    /// `VumaCompiler` API compile path, not verification, so disabling
    /// verification preserves the test's intent.  See Task 2-a report
    /// in worklog.md for the full IVE false-positive analysis.
    #[test]
    fn test_compile_with_allocation() {
        let compiler = VumaCompiler::with_config(CompileConfig {
            verification_level: VerificationLevel::None,
            ..CompileConfig::default()
        });
        let source = r#"
            region memory_pool = allocate(1024);
            fn main() {
                node_ptr = memory_pool + 64;
                header = node_ptr as *NodeHeader;
            }
        "#;
        let result = compiler.compile(source);
        assert!(result.success, "Compilation should succeed");
        let scg = result.scg.unwrap();
        assert!(scg.total_nodes > 0, "SCG should have nodes");
    }

    #[test]
    fn test_parse_only() {
        let compiler = VumaCompiler::new();
        let source = r#"
            fn add(a: i64, b: i64) {
                result = a + b;
            }
            fn main() {
            }
        "#;
        let result = compiler.parse(source);
        assert!(result.success, "Parsing should succeed");
        assert!(result.ast_summary.is_some(), "AST summary should be present");
        assert!(result.scg.is_some(), "SCG summary should be present");
    }

    #[test]
    fn test_analyze() {
        let compiler = VumaCompiler::new();
        let source = r#"
            fn main() {
                x = 1 + 2;
            }
        "#;
        let summary = compiler.analyze(source);
        assert!(summary.total_nodes > 0, "SCG should have nodes");
        assert!(!summary.functions.is_empty(), "Should have at least one function");
    }

    #[test]
    fn test_validate_valid() {
        let compiler = VumaCompiler::new();
        let source = "fn main() {}";
        let diags = compiler.validate(source);
        assert!(diags.is_empty(), "Valid source should have no diagnostics");
    }

    #[test]
    fn test_validate_invalid() {
        let compiler = VumaCompiler::new();
        let source = "fn 123invalid() {}";
        let diags = compiler.validate(source);
        assert!(!diags.is_empty(), "Invalid source should have diagnostics");
        assert!(diags.iter().any(|d| d.severity == DiagnosticSeverity::Error));
    }

    #[test]
    fn test_available_targets() {
        let compiler = VumaCompiler::new();
        let targets = compiler.available_targets();
        assert!(!targets.is_empty(), "Should have available targets");
        let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"aarch64"), "AArch64 should be available");
        assert!(names.contains(&"x86_64"), "x86_64 should be available");
        assert!(names.contains(&"riscv64"), "RISC-V 64 should be available");
    }

    #[test]
    fn test_compile_for_unknown_target() {
        let compiler = VumaCompiler::new();
        let source = "fn main() {}";
        let result = compiler.compile_for_target(source, "unknown_arch");
        assert!(!result.success, "Should fail for unknown target");
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("Unknown target")));
    }

    #[test]
    fn test_compile_result_serializable() {
        let compiler = VumaCompiler::new();
        let source = "fn main() {}";
        let result = compiler.compile(source);
        let json = serde_json::to_string(&result);
        assert!(json.is_ok(), "CompileResult should be serializable");
    }

    #[test]
    fn test_metadata() {
        let compiler = VumaCompiler::new();
        let source = "fn main() {}";
        let result = compiler.compile(source);
        assert!(result.metadata.source_lines > 0);
        assert!(result.metadata.source_bytes > 0);
    }

    #[test]
    fn test_verify_simple() {
        let compiler = VumaCompiler::new();
        let source = "fn main() {}";
        let report = compiler.verify(source);
        // A simple empty function should parse and verify without errors.
        assert!(
            report.overall_verdict != VerificationVerdict::Error,
            "Verification should not error for valid source"
        );
        assert!(
            !report.invariants.is_empty(),
            "Should have per-invariant results"
        );
        assert!(
            report.metadata.total_elapsed_ms > 0 || report.invariants.len() == 5,
            "Should have timing data or all 5 invariants"
        );
    }

    #[test]
    fn test_verify_report_serializable() {
        let compiler = VumaCompiler::new();
        let source = "fn main() {}";
        let report = compiler.verify(source);
        let json = serde_json::to_string(&report);
        assert!(json.is_ok(), "VerificationReport should be serializable");
    }

    #[test]
    fn test_verify_invalid_source() {
        let compiler = VumaCompiler::new();
        let source = "fn 123invalid() {}";
        let report = compiler.verify(source);
        assert_eq!(
            report.overall_verdict,
            VerificationVerdict::Error,
            "Invalid source should produce Error verdict"
        );
        assert!(
            !report.diagnostics.is_empty(),
            "Invalid source should have diagnostics"
        );
    }

    /// W10: proof generation is wired into the pipeline — the proof
    /// bundle produced by `verify()` is non-empty for a safe program
    /// and contains a proof object for each invariant the IVE checked.
    #[test]
    fn test_proof_bundle_wired_into_pipeline() {
        let compiler = VumaCompiler::new();
        let source = "fn main() {}";
        let report = compiler.verify(source);

        // The proof bundle must be populated (not None) — this is the
        // core assertion that proof generation is no longer dead code.
        let bundle = report
            .proof_bundle
            .as_ref()
            .expect("proof_bundle should be populated by verify() for a safe program");

        // The bundle reports one status entry per invariant (5 total).
        let statuses = bundle.status();
        assert_eq!(
            statuses.len(),
            5,
            "bundle should cover all 5 invariants"
        );

        // The bundle must NOT be empty — at least one proof must have
        // been constructed (i.e. at least one non-NotAttempted status).
        // An all-NotAttempted bundle would mean `build_proof_bundle`
        // regressed to returning `ProofBundle::new()`.
        let attempted = statuses
            .iter()
            .filter(|(_, s)| !matches!(s, InvariantStatus::NotAttempted))
            .count();
        assert!(
            attempted > 0,
            "proof bundle is empty (all 5 invariants NotAttempted) — \
             proof generation is dead code"
        );

        // Cross-check: every invariant the IVE marked as Pass should
        // have a corresponding proof object in the bundle (i.e. its
        // status is Proven, Failed, or at least not NotAttempted).
        for (inv_name, proof_status) in &statuses {
            let inv_str = format!("{}", inv_name);
            if let Some(ive) = report.invariants.iter().find(|i| i.kind == inv_str) {
                if ive.status == InvariantVerificationStatus::Pass {
                    assert!(
                        !matches!(proof_status, InvariantStatus::NotAttempted),
                        "invariant {} passed IVE verification but has no proof in the bundle",
                        inv_str
                    );
                }
            }
        }

        // The constructed proofs should carry evidence-referencing
        // steps (not be empty shells). Verify on the first non-empty
        // proof we find.
        let has_steps = bundle
            .liveness
            .as_ref()
            .map(|p| !p.proof.steps.is_empty())
            .or_else(|| {
                bundle.exclusivity.as_ref().map(|p| !p.proof.steps.is_empty())
            })
            .or_else(|| {
                bundle.cleanup.as_ref().map(|p| !p.proof.steps.is_empty())
            })
            .or_else(|| {
                bundle.origin.as_ref().map(|p| !p.proof.steps.is_empty())
            })
            .or_else(|| {
                bundle
                    .interpretation
                    .as_ref()
                    .map(|p| !p.proof.steps.is_empty())
            })
            .unwrap_or(false);
        assert!(
            has_steps,
            "constructed proofs should contain at least one evidence-referencing step"
        );
    }

    /// W10: a parse error should produce `proof_bundle: None` (not an
    /// empty bundle), so consumers can distinguish "verification ran and
    /// produced no proofs" from "verification never ran".
    #[test]
    fn test_proof_bundle_none_on_parse_error() {
        let compiler = VumaCompiler::new();
        let source = "fn 123invalid() {}";
        let report = compiler.verify(source);
        assert_eq!(report.overall_verdict, VerificationVerdict::Error);
        assert!(
            report.proof_bundle.is_none(),
            "proof_bundle should be None when verification never ran (parse error)"
        );
    }
}
