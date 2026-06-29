//! # VUMA LLM API — High-Level Interface for LLM Consumption
//!
//! This module provides a stateless, zero-configuration API designed for
//! Large Language Models (LLMs) that need to compile, analyze, and reason
//! about VUMA source code programmatically.
//!
//! ## Design Principles
//!
//! - **Stateless**: No compiler instance required; all methods are associated
//!   functions on [`VumaForLLM`].
//! - **Always returns data**: Errors are captured as diagnostics rather than
//!   panics or `Err` variants.
//! - **JSON-friendly**: All result types derive `Serialize`/`Deserialize`.
//! - **Natural language**: Explanations and suggestions are in plain English,
//!   suitable for LLM reasoning chains.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use vuma::llm_api::{VumaForLLM, LLMCompileResult};
//!
//! let source = "fn main() { x = 1 + 2; }";
//!
//! // Full compilation with structured result
//! let result = VumaForLLM::compile(source);
//! if result.success {
//!     println!("Compiled! Sizes: {:?}", result.binary_sizes);
//! }
//!
//! // Quick syntax check
//! let diags = VumaForLLM::check(source);
//! println!("{} diagnostics", diags.len());
//!
//! // SCG analysis for LLM reasoning
//! let scg = VumaForLLM::analyze(source).unwrap();
//! println!("SCG: {}", serde_json::to_string_pretty(&scg).unwrap());
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::diagnostics::{code_description, VumaDiagnostic};
use crate::pipeline::{self, CompileConfig};

// ═══════════════════════════════════════════════════════════════════════════
// VumaForLLM — Stateless LLM API
// ═══════════════════════════════════════════════════════════════════════════

/// High-level, stateless API for LLM consumption of the VUMA compiler.
///
/// All methods are associated functions (no `self`), making this trivially
/// callable from any context.  The design prioritises:
///
/// 1. **Structured results** over panics or `Err` returns.
/// 2. **Natural language explanations** that an LLM can incorporate into
///    its reasoning chain.
/// 3. **Multiple output formats** (native binary, Wasm, SCG JSON).
pub struct VumaForLLM;

impl VumaForLLM {
    /// Compile and return a structured result with everything an LLM needs.
    ///
    /// This runs the full compilation pipeline (parse → SCG → IR → codegen)
    /// and returns an [`LLMCompileResult`] containing:
    /// - Success/failure status
    /// - Diagnostics (errors, warnings)
    /// - Natural language explanation of the result
    /// - SCG as JSON (if parsing succeeded)
    /// - Wasm binary (if requested and compilation succeeded)
    /// - Binary sizes per target
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let result = VumaForLLM::compile("fn main() { x = 42; }");
    /// println!("Success: {}", result.success);
    /// println!("Explanation: {}", result.explanation);
    /// ```
    pub fn compile(source: &str) -> LLMCompileResult {
        let config = CompileConfig::default();

        // Compile for the default Linux target.
        let compile_result = pipeline::compile(source, &config);

        match compile_result {
            Ok(output) => {
                let mut binary_sizes = HashMap::new();
                binary_sizes.insert("aarch64-linux".to_string(), output.binary.len());

                // Get SCG JSON if available (using the SCG's built-in LLM
                // structured output rather than raw Serialize).
                let scg_json = serde_json::from_str(&output.scg.to_json()).ok();

                // Try Wasm compilation as well.
                let wasm_binary = match pipeline::compile_to_wasm(source) {
                    Ok(wasm) => {
                        binary_sizes.insert("wasm32".to_string(), wasm.len());
                        Some(wasm)
                    }
                    Err(_) => None,
                };

                let explanation = format!(
                    "Compilation succeeded. Produced {} bytes of AArch64 ELF binary \
                     with {} SCG nodes, {} SCG regions, and {} IR instructions. \
                     {} IR function(s) generated.",
                    output.binary.len(),
                    output.scg.node_count(),
                    output.scg.region_count(),
                    output.ir_instruction_count,
                    output.ir_function_count,
                );

                LLMCompileResult {
                    success: true,
                    diagnostics: Vec::new(),
                    explanation,
                    scg_json,
                    wasm_binary,
                    binary_sizes,
                }
            }
            Err(errors) => {
                let diagnostics: Vec<VumaDiagnostic> = errors
                    .iter()
                    .flat_map(crate::diagnostics::from_vuma_error)
                    .collect();

                let error_descriptions: Vec<String> = diagnostics
                    .iter()
                    .filter(|d| d.severity == crate::diagnostics::DiagnosticSeverity::Error)
                    .map(|d| {
                        let desc = code_description(&d.code);
                        format!("[{}] {} — {}", d.code, desc, d.message)
                    })
                    .collect();

                let explanation = if error_descriptions.is_empty() {
                    "Compilation failed with unknown errors.".to_string()
                } else {
                    format!(
                        "Compilation failed with {} error(s):\n{}",
                        error_descriptions.len(),
                        error_descriptions.join("\n")
                    )
                };

                LLMCompileResult {
                    success: false,
                    diagnostics,
                    explanation,
                    scg_json: None,
                    wasm_binary: None,
                    binary_sizes: HashMap::new(),
                }
            }
        }
    }

    /// Quick syntax check — returns just diagnostics.
    ///
    /// This runs the parser and SCG validation only, without codegen.
    /// It is the fastest way to check if a program is syntactically
    /// and semantically well-formed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let diags = VumaForLLM::check("fn main() { x = 1 + ; }");
    /// for d in &diags {
    ///     println!("[{}] {}", d.code, d.message);
    /// }
    /// ```
    pub fn check(source: &str) -> Vec<VumaDiagnostic> {
        use vuma_parser::{AstToScg, Parser};

        let mut all_diagnostics = Vec::new();

        // Parse
        let mut parser = Parser::new(source);
        let parse_output = parser.parse_program();

        if parse_output.has_errors() {
            all_diagnostics.extend(crate::diagnostics::from_parse_errors(
                &parse_output.errors,
                source,
                None,
            ));
            return all_diagnostics;
        }

        let ast = match parse_output.value {
            Some(a) => a,
            None => return all_diagnostics,
        };

        // AST → SCG
        let mut converter = AstToScg::new();
        match converter.convert(&ast) {
            Ok(scg) => {
                // Validate SCG
                let validation = scg.validate();
                if !validation.is_valid {
                    for err in &validation.errors {
                        all_diagnostics.push(VumaDiagnostic::new(
                            "E022",
                            crate::diagnostics::DiagnosticSeverity::Error,
                            err.clone(),
                            "scg-validation",
                            crate::diagnostics::DiagnosticSourceLocation::unknown(),
                        ));
                    }
                }
            }
            Err(e) => {
                all_diagnostics.push(VumaDiagnostic::new(
                    "E019",
                    crate::diagnostics::DiagnosticSeverity::Error,
                    format!("{}", e),
                    "ast-to-scg",
                    crate::diagnostics::DiagnosticSourceLocation::unknown(),
                ));
            }
        }

        all_diagnostics
    }

    /// Get the SCG as JSON for LLM reasoning.
    ///
    /// Returns the Semantic Computation Graph serialised as a structured
    /// JSON value that an LLM can inspect to understand the program's
    /// data flow, control flow, and memory operations.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let scg_json = VumaForLLM::analyze("fn main() { x = 1 + 2; }").unwrap();
    /// println!("{}", serde_json::to_string_pretty(&scg_json).unwrap());
    /// ```
    pub fn analyze(source: &str) -> Result<serde_json::Value, String> {
        use vuma_parser::{AstToScg, Parser};

        // Parse
        let mut parser = Parser::new(source);
        let parse_output = parser.parse_program();

        if parse_output.has_errors() {
            let errors: Vec<String> = parse_output
                .errors
                .iter()
                .map(|e| format!("{:?}", e))
                .collect();
            return Err(format!("Parse errors: {}", errors.join("; ")));
        }

        let ast = parse_output.unwrap();

        // AST → SCG
        let mut converter = AstToScg::new();
        let scg = converter
            .convert(&ast)
            .map_err(|e| format!("AST → SCG conversion failed: {}", e))?;

        // Use the SCG's built-in structured output for LLMs.
        let json_str = scg.to_json();
        serde_json::from_str(&json_str)
            .map_err(|e| format!("SCG JSON serialization failed: {}", e))
    }

    /// Compile to Wasm for sandboxed execution.
    ///
    /// Produces a `.wasm` binary module that can be executed in any
    /// WebAssembly runtime (wasmer, wasmtime, Node.js).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// match VumaForLLM::to_wasm("fn main() -> i32 { return 42; }") {
    ///     Ok(wasm_bytes) => println!("Wasm binary: {} bytes", wasm_bytes.len()),
    ///     Err(diags) => println!("Errors: {:?}", diags),
    /// }
    /// ```
    pub fn to_wasm(source: &str) -> Result<Vec<u8>, Vec<VumaDiagnostic>> {
        match pipeline::compile_to_wasm(source) {
            Ok(wasm_bytes) => Ok(wasm_bytes),
            Err(errors) => {
                let diagnostics: Vec<VumaDiagnostic> = errors
                    .iter()
                    .flat_map(crate::diagnostics::from_vuma_error)
                    .collect();
                Err(diagnostics)
            }
        }
    }

    /// Explain a compilation error in natural language.
    ///
    /// Given a [`VumaDiagnostic`], returns a human-readable explanation
    /// of what went wrong and why, suitable for an LLM's reasoning chain
    /// or for displaying to a user.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let diags = VumaForLLM::check("fn 123bad() {}");
    /// for d in &diags {
    ///     let explanation = VumaForLLM::explain_error(d);
    ///     println!("{}", explanation);
    /// }
    /// ```
    pub fn explain_error(diagnostic: &VumaDiagnostic) -> String {
        let code_desc = code_description(&diagnostic.code);
        let severity_str = match diagnostic.severity {
            crate::diagnostics::DiagnosticSeverity::Error => "error",
            crate::diagnostics::DiagnosticSeverity::Warning => "warning",
            crate::diagnostics::DiagnosticSeverity::Info => "information",
            crate::diagnostics::DiagnosticSeverity::Hint => "hint",
        };

        let mut explanation = format!(
            "{} [{}]: {} — {}",
            severity_str, diagnostic.code, code_desc, diagnostic.message
        );

        // Add source location if available.
        if !diagnostic.location.file.is_empty() {
            explanation.push_str(&format!(
                " (at {}:{}:{}-{}:{})",
                diagnostic.location.file,
                diagnostic.location.start_line,
                diagnostic.location.start_col,
                diagnostic.location.end_line,
                diagnostic.location.end_col
            ));
        } else if diagnostic.location.start_line > 0 {
            explanation.push_str(&format!(
                " (at line {}:{}-{}:{})",
                diagnostic.location.start_line,
                diagnostic.location.start_col,
                diagnostic.location.end_line,
                diagnostic.location.end_col
            ));
        }

        // Add context about the compiler stage.
        explanation.push_str(&format!(
            " [stage: {}]",
            diagnostic.source
        ));

        // Add chain information.
        if !diagnostic.chain.is_empty() {
            explanation.push_str("\nCaused by:");
            for (i, cause) in diagnostic.chain.iter().enumerate() {
                explanation.push_str(&format!(
                    "\n  {}: [{}] {}",
                    i + 1,
                    cause.code,
                    cause.message
                ));
            }
        }

        explanation
    }

    /// Suggest fixes for a compilation error.
    ///
    /// Returns a list of suggested fixes, each as a natural language
    /// string that describes the fix.  If the diagnostic includes
    /// structured suggestions (with edit ranges), those are included.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let diags = VumaForLLM::check("fn main() { x = ; }");
    /// for d in &diags {
    ///     let fixes = VumaForLLM::suggest_fixes(d);
    ///     for fix in &fixes {
    ///         println!("Try: {}", fix);
    ///     }
    /// }
    /// ```
    pub fn suggest_fixes(diagnostic: &VumaDiagnostic) -> Vec<String> {
        let mut suggestions = Vec::new();

        // Include structured suggestions.
        for s in &diagnostic.suggestions {
            suggestions.push(s.message.clone());
        }

        // Include legacy suggestions.
        for s in &diagnostic.legacy_suggestions {
            suggestions.push(s.clone());
        }

        // If no explicit suggestions, generate one based on the error code.
        if suggestions.is_empty() {
            let code_desc = code_description(&diagnostic.code);
            let hint = match diagnostic.code.as_str() {
                "E001" | "E009" | "E010" => {
                    format!("{}: Check the syntax near the reported location. \
                             Common fixes: missing semicolons, mismatched braces, \
                             or incorrect operator usage.", code_desc)
                }
                "E002" => {
                    format!("{}: The variable '{}' is not defined in the current scope. \
                             Make sure it is declared before use, or check for typos.",
                             code_desc, diagnostic.message)
                }
                "E003" => {
                    format!("{}: The types don't match. Check that the left-hand side \
                             and right-hand side of the expression have compatible types.",
                             code_desc)
                }
                "E004" => {
                    format!("{}: A symbol with this name already exists. Choose a \
                             different name or remove the duplicate.", code_desc)
                }
                "E021" => {
                    format!("{}: The code appears to use C/Rust syntax instead of VUMA \
                             syntax. Replace `int` with `i32` or `i64`, remove type \
                             annotations after variable names, and use VUMA's assignment \
                             syntax.", code_desc)
                }
                "E022" => {
                    format!("{}: VUMA uses range-based for loops, not C-style. \
                             Replace `for (i=0; i<n; i++)` with `for i in 0..n`.",
                             code_desc)
                }
                "E023" => {
                    format!("{}: VUMA uses sized integer types like `i8`, `i16`, \
                             `i32`, `i64`, `u8`, `u32`, `u64`. Replace the unknown \
                             type with a VUMA-sized type.", code_desc)
                }
                _ => {
                    format!("{}: Review the error message and check the relevant \
                             code near the reported location.", code_desc)
                }
            };
            suggestions.push(hint);
        }

        suggestions
    }

    /// Get available compilation targets.
    ///
    /// Returns a list of [`LLMTargetInfo`] describing each supported
    /// backend (ISA name, pointer width, endianness, output format).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// for target in VumaForLLM::targets() {
    ///     println!("{}: {}-bit, {}", target.name, target.pointer_width, target.endianness);
    /// }
    /// ```
    pub fn targets() -> Vec<LLMTargetInfo> {
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
            BackendKind::X86_32,
            BackendKind::RiscV32,
        ];

        all_kinds
            .iter()
            .filter_map(|&kind| {
                create_backend(kind).ok().map(|backend| {
                    let info = backend.target_info();
                    LLMTargetInfo {
                        name: kind.isa_name().to_string(),
                        triple: info.target_triple().to_string(),
                        pointer_width: info.pointer_width() * 8,
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
}

// ═══════════════════════════════════════════════════════════════════════════
// LLM Result Types
// ═══════════════════════════════════════════════════════════════════════════

/// Structured compilation result designed for LLM consumption.
///
/// Contains everything an LLM needs to reason about the compilation
/// outcome: success status, diagnostics, natural language explanation,
/// SCG analysis, Wasm binary, and binary sizes per target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMCompileResult {
    /// Whether compilation succeeded.
    pub success: bool,
    /// Any diagnostics (errors, warnings, notes) produced.
    pub diagnostics: Vec<VumaDiagnostic>,
    /// Natural language summary of the compilation result.
    pub explanation: String,
    /// SCG serialised as JSON for LLM reasoning (present if parsing succeeded).
    pub scg_json: Option<serde_json::Value>,
    /// Wasm binary for sandboxed execution (present if Wasm compilation succeeded).
    pub wasm_binary: Option<Vec<u8>>,
    /// Binary sizes per target (e.g., "aarch64-linux" → 1234).
    pub binary_sizes: HashMap<String, usize>,
}

/// Information about a supported compilation target.
///
/// Simplified version of [`ApiTargetInfo`](crate::api::ApiTargetInfo)
/// for the LLM API surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LLMTargetInfo {
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
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_compile_simple() {
        let result = VumaForLLM::compile("fn main() {}");
        assert!(result.success, "Simple compilation should succeed");
        assert!(!result.explanation.is_empty(), "Should have explanation");
    }

    #[test]
    fn test_llm_compile_invalid() {
        let result = VumaForLLM::compile("fn 123bad() {}");
        assert!(!result.success, "Invalid source should fail");
        assert!(!result.diagnostics.is_empty(), "Should have diagnostics");
        assert!(
            result.explanation.contains("failed"),
            "Explanation should mention failure"
        );
    }

    #[test]
    fn test_llm_check_valid() {
        let diags = VumaForLLM::check("fn main() {}");
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == crate::diagnostics::DiagnosticSeverity::Error)
            .collect();
        assert!(errors.is_empty(), "Valid source should have no errors");
    }

    #[test]
    fn test_llm_check_invalid() {
        let diags = VumaForLLM::check("fn 123bad() {}");
        assert!(!diags.is_empty(), "Invalid source should have diagnostics");
    }

    #[test]
    fn test_llm_analyze() {
        let result = VumaForLLM::analyze("fn main() { x = 1 + 2; }");
        assert!(result.is_ok(), "Analysis should succeed");
        let json = result.unwrap();
        assert!(json.is_object(), "Result should be a JSON object");
    }

    #[test]
    fn test_llm_explain_error() {
        let diags = VumaForLLM::check("fn 123bad() {}");
        if let Some(diag) = diags.first() {
            let explanation = VumaForLLM::explain_error(diag);
            assert!(!explanation.is_empty(), "Should have explanation");
            assert!(
                explanation.contains("[") && explanation.contains("]"),
                "Should include error code"
            );
        }
    }

    #[test]
    fn test_llm_suggest_fixes() {
        let diags = VumaForLLM::check("fn 123bad() {}");
        if let Some(diag) = diags.first() {
            let fixes = VumaForLLM::suggest_fixes(diag);
            assert!(!fixes.is_empty(), "Should have at least one suggestion");
        }
    }

    #[test]
    fn test_llm_targets() {
        let targets = VumaForLLM::targets();
        assert!(!targets.is_empty(), "Should have available targets");
        let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"aarch64"), "AArch64 should be available");
        assert!(names.contains(&"x86_64"), "x86_64 should be available");
    }

    #[test]
    fn test_llm_compile_result_has_explanation() {
        let result = VumaForLLM::compile("fn main() {}");
        assert!(
            result.explanation.contains("succeeded") || result.explanation.contains("failed"),
            "Explanation should describe the outcome"
        );
    }

    #[test]
    fn test_llm_binary_sizes_on_success() {
        let result = VumaForLLM::compile("fn main() {}");
        if result.success {
            assert!(
                result.binary_sizes.contains_key("aarch64-linux"),
                "Should have aarch64-linux binary size"
            );
            assert!(
                *result.binary_sizes.get("aarch64-linux").unwrap() > 0,
                "Binary should have non-zero size"
            );
        }
    }
}
