//! VUMA — Verified-Unsafe Memory Access: AI-Native Programming Language Framework
//!
//! This is the root crate that aggregates all workspace members and provides
//! the full compilation pipeline.
//!
//! # Architecture
//!
//! The VUMA framework is organised as a workspace of specialised crates:
//!
//! | Crate           | Purpose                                          |
//! |-----------------|--------------------------------------------------|
//! | `vuma-parser`   | Lexer, parser, AST, and AST → SCG bridge        |
//! | `vuma-scg`      | Semantic Computation Graph (SCG) core            |
//! | `vuma-ive`      | Inference & Verification Engine (5 invariants)   |
//! | `vuma-bd`       | Behavioral Descriptors (RepD, CapD, RelD)        |
//! | `vuma-core`     | Memory State Graph (MSG) and SCG → MSG           |
//! | `vuma-codegen`  | IR lowering, register allocation, ARM64 codegen   |
//! | `vuma-projection` | Textual/visual/conversational projections       |
//! | `vuma-proof`    | Proof generation and checking                    |
//! | `vuma-cor`      | Coordination runtime                             |
//! | `vuma-std`      | Standard library                                 |
//!
//! # Quick Start
//!
//! ```rust
//! use vuma::pipeline::{compile, CompileConfig};
//!
//! let source = "fn main() {}";
//! let config = CompileConfig::default();
//! match compile(source, &config) {
//!     Ok(output) => println!("Compiled {} bytes", output.binary.len()),
//!     Err(errors) => {
//!         for err in &errors {
//!             eprintln!("{}", err);
//!         }
//!     }
//! }
//! ```

#![warn(missing_docs)]

pub mod api;
pub mod diagnostics;
pub mod ffi;
pub mod llm_api;
pub mod lsp;
pub mod pipeline;

// Re-export package manager types.
pub use vuma_package::{
    PackageManifest, PackageTarget, Dependency, TargetKind,
    PackageRegistry, DependencyResolver, ResolveResult,
    PackageError, PackageResult,
    init_package, add_dependency, build_package,
    parse_manifest, resolve_dependencies,
};

// Re-export the primary pipeline API at the crate root for convenience.
pub use pipeline::{
    compile, compile_incremental, compile_to_wasm, compile_with_path, CompilationOutput, CompileConfig,
    CompileTarget, DebugInfo, IncrementalCache, OptLevel, PipelineStage, SourceFingerprint,
    VerificationLevel, VumaError,
};

// Re-export diagnostics types for convenience.
pub use diagnostics::{
    diagnostics_to_json, diagnostics_to_json_pretty, from_codegen_error, from_parse_error,
    from_parse_errors, from_vuma_error, code_for_parse_error_kind, code_for_codegen_error,
    code_category, code_subcategory, code_description,
    DiagnosticSeverity, DiagnosticSourceLocation, DiagnosticSummary,
    RelatedInfo, Suggestion, SuggestionApplicability, VumaDiagnostic,
};

// Re-export the primary API types for convenience.
pub use api::{
    ApiTargetInfo, CompileMetadata, CompileResult, CounterexampleInfo, FunctionSummary,
    InvariantVerification, InvariantVerificationStatus, ParseResult, ScgSummary, TargetOutput,
    VerificationMetadata, VerificationReport, VerificationVerdict, VumaCompiler,
};

// Re-export REPL types from vuma-core for convenience.
pub use vuma_core::repl::{ReplError, ReplProfile, ReplResult, VumaRepl};

// Re-export LSP types for convenience.
pub use lsp::{
    CompletionItem, CompletionItemKind, Diagnostic, DocumentSymbol,
    LspServer, Position, Range, SemanticTokensLegend, SymbolKind, VumaDocument,
};
pub use lsp::DiagnosticSeverity as LspDiagnosticSeverity;

// Re-export LLM API types for convenience.
pub use llm_api::{LLMCompileResult, LLMTargetInfo, VumaForLLM};
