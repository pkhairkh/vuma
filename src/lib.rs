//! VUMA — Verified-Unsafe Memory Access
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
//! | `vuma-ive`      | Inference & Verification Engine (PMT invariant)  |
//! | `vuma-bd`       | Behavioral Descriptors (RepD, CapD, RelD)        |
//! | `vuma-core`     | Memory State Graph (MSG) and SCG → MSG           |
//! | `vuma-codegen`  | IR lowering, register allocation, 10 backends    |
//! | `vuma-tests`    | Integration tests and benchmarks                 |
//! | `vuma-package`  | Package manager                                  |
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

#[macro_use]
mod vuma_log_w44 {
    /// Logging macro for VUMA compiler diagnostics.
    ///
    /// In debug builds: always emits to stderr.
    /// In release builds: emits to stderr if `VUMA_LOG` env var is set.
    /// This ensures advisory verification warnings are visible in production.
    #[macro_export]
    macro_rules! vuma_log {
        ($level:ident, $($arg:tt)*) => {{
            let emit = cfg!(debug_assertions) || std::env::var("VUMA_LOG").is_ok();
            if emit {
                eprintln!("[{}] {}", stringify!($level), format!($($arg)*));
            }
        }};
    }
}
pub mod api;
pub mod diagnostics;
pub mod ffi;
pub mod json_value;
pub mod llm_api;
pub mod logging;
pub mod lsp;
pub mod pipeline;
pub mod telemetry;
pub mod time;

// Re-export package manager types.
pub use vuma_package::{
    add_dependency, build_package, init_package, parse_manifest, resolve_dependencies, Dependency,
    DependencyResolver, PackageError, PackageManifest, PackageRegistry, PackageResult,
    PackageTarget, ResolveResult, TargetKind,
};

// Re-export the primary pipeline API at the crate root for convenience.
pub use pipeline::{
    compile, compile_incremental, compile_modules, compile_to_wasm, compile_with_path,
    compile_with_recovery, CompilationOutput, CompileConfig, CompileResult, CompileTarget,
    DebugInfo, IncrementalCache, OptLevel, PartialCompilationOutput, PipelineStage,
    SourceFingerprint, VerificationLevel, VumaError,
};

// Re-export diagnostics types for convenience.
pub use diagnostics::{
    code_category, code_description, code_for_codegen_error, code_for_parse_error_kind,
    code_subcategory, diagnostics_to_json, diagnostics_to_json_pretty, from_codegen_error,
    from_parse_error, from_parse_errors, from_vuma_error, DiagnosticSeverity,
    DiagnosticSourceLocation, DiagnosticSummary, RelatedInfo, Suggestion, SuggestionApplicability,
    VumaDiagnostic,
};

// Re-export the primary API types for convenience.
pub use api::{
    ApiTargetInfo, CompileMetadata, CompileResult as ApiCompileResult, CounterexampleInfo,
    FunctionSummary, InvariantVerification, InvariantVerificationStatus, ParseResult, ScgSummary,
    TargetOutput, VerificationMetadata, VerificationReport, VerificationVerdict, VumaCompiler,
};

// Re-export REPL types from vuma-core for convenience.
pub use vuma_core::repl::{ReplError, ReplProfile, ReplResult, VumaRepl};

// Re-export LSP types for convenience.
pub use lsp::DiagnosticSeverity as LspDiagnosticSeverity;
pub use lsp::{
    CompletionItem, CompletionItemKind, Diagnostic, DocumentSymbol, LspServer, Position, Range,
    SemanticTokensLegend, SymbolKind, VumaDocument,
};

// Re-export telemetry types for convenience.
pub use telemetry::{StageMetrics, TelemetryCollector, TelemetryReport};

// Re-export logging types for convenience.
pub use logging::{global_logger, init_logger, LogLevel, VumaLogger};

// Re-export LLM API types for convenience.
pub use llm_api::{LLMCompileResult, LLMTargetInfo, VumaForLLM};
