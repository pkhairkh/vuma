//! Structured JSON diagnostics output for LLM integration.
//!
//! This module provides machine-readable diagnostic messages that can be
//! serialized to JSON, enabling LLM-based tools and IDE integrations to
//! programmatically consume compiler errors, warnings, and hints.
//!
//! # Architecture
//!
//! Each diagnostic carries a structured error code (`"E001"`, `"W001"`, etc.),
//! a severity level, a human-readable message, a source identifier (which
//! compiler stage produced it), a precise source location, optional related
//! information, quick-fix suggestions with edit ranges, and an optional
//! causal chain linking to parent diagnostics.
//!
//! # Error Catalog
//!
//! The diagnostic codes are organized into the following ranges:
//!
//! | Range    | Category       | Description                              |
//! |----------|----------------|------------------------------------------|
//! | E001–E030| Compilation    | Syntax, type, name resolution errors     |
//! | E031–E040| Codegen        | Register allocation, encoding, relocation|
//! | E041–E050| Verification   | Invariant violations, proof failures     |
//! | W001–W010| Warnings       | Unused vars, performance hints            |
//! | I001–I005| Informational  | General compiler information messages     |
//!
//! # Example JSON output
//!
//! ```json
//! [
//!   {
//!     "code": "E002",
//!     "severity": "error",
//!     "message": "undefined variable `foo`",
//!     "source": "parser",
//!     "location": {
//!       "file": "main.vu",
//!       "start_line": 10,
//!       "start_col": 4,
//!       "end_line": 10,
//!       "end_col": 7
//!     },
//!     "related": [],
//!     "suggestions": ["did you mean `for`?"],
//!     "chain": []
//!   }
//! ]
//! ```
//!
//! # Integration
//!
//! Use [`from_parse_error`] to convert parser errors, [`from_vuma_error`] for
//! pipeline-level errors, and [`from_codegen_error`] for code-generation
//! failures.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

use vuma_codegen::CodegenError;
use vuma_parser::{offset_to_location, ParseError, ParseErrorKind, Severity as ParserSeverity};

use crate::pipeline::VumaError;

// ═══════════════════════════════════════════════════════════════════════════
// DiagnosticSeverity
// ═══════════════════════════════════════════════════════════════════════════

/// Severity level for a machine-readable diagnostic.
///
/// Unlike the parser's `Severity` (which has `Note`), this uses `Hint` for
/// LLM-consumable suggestions and `Info` for informational notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    /// A hard error — the program cannot be compiled.
    Error,
    /// A warning — suspicious code that is technically valid.
    Warning,
    /// An informational note, typically providing context.
    Info,
    /// A hint or quick-fix suggestion.
    Hint,
}

impl fmt::Display for DiagnosticSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiagnosticSeverity::Error => write!(f, "error"),
            DiagnosticSeverity::Warning => write!(f, "warning"),
            DiagnosticSeverity::Info => write!(f, "info"),
            DiagnosticSeverity::Hint => write!(f, "hint"),
        }
    }
}

impl From<ParserSeverity> for DiagnosticSeverity {
    fn from(s: ParserSeverity) -> Self {
        match s {
            ParserSeverity::Error => DiagnosticSeverity::Error,
            ParserSeverity::Warning => DiagnosticSeverity::Warning,
            ParserSeverity::Note => DiagnosticSeverity::Info,
        }
    }
}

impl DiagnosticSeverity {
    /// Returns the LSP numeric severity value.
    pub fn to_lsp_severity(&self) -> u32 {
        match self {
            DiagnosticSeverity::Error => 1,
            DiagnosticSeverity::Warning => 2,
            DiagnosticSeverity::Info => 3,
            DiagnosticSeverity::Hint => 4,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// DiagnosticSourceLocation
// ═══════════════════════════════════════════════════════════════════════════

/// Machine-readable source location with line/column ranges.
///
/// All positions are 1-based (as is conventional for compiler diagnostics)
/// rather than the 0-based positions used internally by the parser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticSourceLocation {
    /// File path or module name.
    pub file: String,
    /// Start line (1-based).
    pub start_line: u32,
    /// Start column (1-based).
    pub start_col: u32,
    /// End line (1-based).
    pub end_line: u32,
    /// End column (1-based, exclusive).
    pub end_col: u32,
}

impl DiagnosticSourceLocation {
    /// Create a location spanning a single point (zero-width).
    pub fn point(file: impl Into<String>, line: u32, col: u32) -> Self {
        Self {
            file: file.into(),
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
        }
    }

    /// Create a location spanning a range within a single line.
    pub fn range(
        file: impl Into<String>,
        line: u32,
        start_col: u32,
        end_col: u32,
    ) -> Self {
        Self {
            file: file.into(),
            start_line: line,
            start_col,
            end_line: line,
            end_col,
        }
    }

    /// Create a multi-line location.
    pub fn multi_line(
        file: impl Into<String>,
        start_line: u32,
        start_col: u32,
        end_line: u32,
        end_col: u32,
    ) -> Self {
        Self {
            file: file.into(),
            start_line,
            start_col,
            end_line,
            end_col,
        }
    }

    /// Create a synthetic "unknown" location for diagnostics that cannot be
    /// tied to a specific source position.
    pub fn unknown() -> Self {
        Self {
            file: String::new(),
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
        }
    }

    /// Returns `true` if this location represents an unknown / synthetic
    /// position (all fields zero, no file).
    pub fn is_unknown(&self) -> bool {
        self.file.is_empty() && self.start_line == 0 && self.start_col == 0
    }
}

impl fmt::Display for DiagnosticSourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_unknown() {
            write!(f, "<unknown>")
        } else if self.start_line == self.end_line && self.start_col == self.end_col {
            write!(f, "{}:{}:{}", self.file, self.start_line, self.start_col)
        } else {
            write!(
                f,
                "{}:{}:{}-{}:{}",
                self.file, self.start_line, self.start_col, self.end_line, self.end_col
            )
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// RelatedInfo
// ═══════════════════════════════════════════════════════════════════════════

/// Additional context for a diagnostic: a related source location with a
/// message explaining its relevance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedInfo {
    /// The related source location.
    pub location: DiagnosticSourceLocation,
    /// Why this location is relevant to the primary diagnostic.
    pub message: String,
}

impl RelatedInfo {
    /// Create a new related-info entry.
    pub fn new(location: DiagnosticSourceLocation, message: impl Into<String>) -> Self {
        Self {
            location,
            message: message.into(),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Suggestion (structured)
// ═══════════════════════════════════════════════════════════════════════════

/// A structured suggestion with an optional edit range for automatic fixing.
///
/// Suggestions can be simple text hints (no edit range) or precise code
/// edits that an IDE or tool can apply automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    /// Human-readable description of the suggestion.
    pub message: String,
    /// Optional source location that this suggestion replaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_range: Option<DiagnosticSourceLocation>,
    /// Optional replacement text for the edit range.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    /// The applicability of this suggestion.
    #[serde(default = "default_applicability")]
    pub applicability: SuggestionApplicability,
}

/// How likely a suggestion is to be the right fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionApplicability {
    /// The suggestion is definitely what the user intended (machine-applicable).
    MachineApplicable,
    /// The suggestion has a very high likelihood of being correct.
    MaybeIncorrect,
    /// The suggestion may or may not be correct — requires human judgment.
    HasPlaceholders,
    /// The suggestion is a rough hint, not a precise fix.
    Unspecified,
}

fn default_applicability() -> SuggestionApplicability {
    SuggestionApplicability::Unspecified
}

impl Suggestion {
    /// Create a simple text-only suggestion (no automatic edit).
    pub fn text(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            edit_range: None,
            replacement: None,
            applicability: SuggestionApplicability::Unspecified,
        }
    }

    /// Create a suggestion with an exact edit range and replacement text.
    pub fn edit(
        message: impl Into<String>,
        range: DiagnosticSourceLocation,
        replacement: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            edit_range: Some(range),
            replacement: Some(replacement.into()),
            applicability: SuggestionApplicability::MachineApplicable,
        }
    }

    /// Create a machine-applicable suggestion with an edit range.
    pub fn machine_applicable(
        message: impl Into<String>,
        range: DiagnosticSourceLocation,
        replacement: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            edit_range: Some(range),
            replacement: Some(replacement.into()),
            applicability: SuggestionApplicability::MachineApplicable,
        }
    }

    /// Create a suggestion with placeholders (needs human review).
    pub fn with_placeholders(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            edit_range: None,
            replacement: None,
            applicability: SuggestionApplicability::HasPlaceholders,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Diagnostic codes — expanded catalog
// ═══════════════════════════════════════════════════════════════════════════

/// Returns the diagnostic code string for a given [`ParseErrorKind`].
///
/// Mapping (E001–E030: Compilation errors):
///
/// | Code  | Kind                    | Description                     |
/// |-------|-------------------------|---------------------------------|
/// | E001  | InvalidSyntax           | Syntax error                    |
/// | E002  | UndefinedReference / UndefinedVariable | Undefined variable  |
/// | E003  | TypeMismatch            | Type mismatch                   |
/// | E004  | DuplicateDefinition     | Duplicate definition            |
/// | E005  | —                       | Invalid argument count          |
/// | E006  | —                       | Invalid type                    |
/// | E007  | —                       | Missing return                  |
/// | E008  | —                       | Unreachable code                |
/// | E009  | UnexpectedToken         | Unexpected token                |
/// | E010  | ExpectedToken           | Expected token                  |
/// | E011  | RegionError             | Region error                    |
/// | E012  | BDAnnotationError       | BD annotation error             |
/// | E013  | InvalidCompoundOp       | Invalid compound operator       |
/// | E014  | MissingSemicolon        | Missing semicolon               |
/// | E015  | InvalidAddress          | Invalid address literal         |
/// | E016  | —                       | Invalid instruction             |
/// | E017  | —                       | Register allocation failed      |
/// | E018  | —                       | Encoding error                  |
/// | E019  | —                       | IR translation error            |
/// | E020  | —                       | ELF emission error              |
/// | E021  | LlmMistake              | LLM code mismatch               |
/// | E022  | CStyleForLoop           | C-style for loop                |
/// | E023  | UnknownType             | Unknown type                    |
/// | E024  | —                       | Name resolution error           |
/// | E025  | —                       | Circular dependency             |
/// | E026  | —                       | Invalid assignment target       |
/// | E027  | —                       | Break/continue outside loop     |
/// | E028  | —                       | Invalid cast                    |
/// | E029  | —                       | Missing function body           |
/// | E030  | —                       | Invalid visibility modifier     |
pub fn code_for_parse_error_kind(kind: &ParseErrorKind) -> &'static str {
    match kind {
        ParseErrorKind::InvalidSyntax => "E001",
        ParseErrorKind::UndefinedReference | ParseErrorKind::UndefinedVariable => "E002",
        ParseErrorKind::TypeMismatch => "E003",
        ParseErrorKind::DuplicateDefinition => "E004",
        ParseErrorKind::UnexpectedToken => "E009",
        ParseErrorKind::ExpectedToken => "E010",
        ParseErrorKind::RegionError => "E011",
        ParseErrorKind::BDAnnotationError => "E012",
        ParseErrorKind::InvalidCompoundOp => "E013",
        ParseErrorKind::MissingSemicolon => "E014",
        ParseErrorKind::InvalidAddress => "E015",
        ParseErrorKind::LlmMistake => "E021",
        ParseErrorKind::CStyleForLoop => "E022",
        ParseErrorKind::UnknownType => "E023",
    }
}

/// Returns the diagnostic code string for a given [`CodegenError`].
pub fn code_for_codegen_error(err: &CodegenError) -> &'static str {
    match err {
        CodegenError::InvalidInstruction(_) => "E031",
        CodegenError::RegisterAllocFailed(_) => "E032",
        CodegenError::EncodingError(_) => "E033",
        CodegenError::TranslationError(_) => "E034",
        CodegenError::ElfError(_) => "E035",
        CodegenError::UnknownVariable { .. } => "E002",
        CodegenError::WasmSectionNotFound { .. } => "E036",
        CodegenError::UnresolvedRelocation { .. } => "E037",
    }
}

/// Human-readable description of a diagnostic code.
///
/// Covers the full error catalog: E001–E050, W001–W010, I001–I005.
pub fn code_description(code: &str) -> &'static str {
    match code {
        // ── Compilation errors (E001–E030) ──
        "E001" => "Syntax error",
        "E002" => "Undefined variable",
        "E003" => "Type mismatch",
        "E004" => "Duplicate definition",
        "E005" => "Invalid argument count",
        "E006" => "Invalid type",
        "E007" => "Missing return",
        "E008" => "Unreachable code",
        "E009" => "Unexpected token",
        "E010" => "Expected token",
        "E011" => "Region error",
        "E012" => "BD annotation error",
        "E013" => "Invalid compound assignment operator",
        "E014" => "Missing semicolon",
        "E015" => "Invalid address literal",
        "E016" => "Invalid instruction (legacy)",
        "E017" => "Register allocation failed (legacy)",
        "E018" => "Encoding error (legacy)",
        "E019" => "IR translation error (legacy)",
        "E020" => "ELF emission error (legacy)",
        "E021" => "LLM code mismatch (Rust/C syntax in VUMA)",
        "E022" => "C-style for loop (use range-based for instead)",
        "E023" => "Unknown type (use VUMA sized integer types)",
        "E024" => "Name resolution error",
        "E025" => "Circular dependency",
        "E026" => "Invalid assignment target",
        "E027" => "Break/continue outside loop",
        "E028" => "Invalid cast",
        "E029" => "Missing function body",
        "E030" => "Invalid visibility modifier",

        // ── Codegen errors (E031–E040) ──
        "E031" => "Invalid instruction",
        "E032" => "Register allocation failed",
        "E033" => "Encoding error",
        "E034" => "IR translation error",
        "E035" => "ELF emission error",
        "E036" => "Wasm section not found",
        "E037" => "Relocation error",
        "E038" => "Stack layout error",
        "E039" => "Linker error",
        "E040" => "Target unsupported feature",

        // ── Verification errors (E041–E050) ──
        "E041" => "Invariant violation",
        "E042" => "Proof failure",
        "E043" => "Liveness invariant violated",
        "E044" => "Origin invariant violated",
        "E045" => "Exclusivity invariant violated",
        "E046" => "Interpretation invariant violated",
        "E047" => "Cleanup invariant violated",
        "E048" => "BD inference error",
        "E049" => "Constraint unsatisfiable",
        "E050" => "Verification timeout",

        // ── Warnings (W001–W010) ──
        "W001" => "Unused variable",
        "W002" => "Implicit type conversion",
        "W003" => "Large constant (performance hint)",
        "W004" => "Dead code",
        "W005" => "Redundant cast",
        "W006" => "Shadowed variable",
        "W007" => "Unnecessary mut keyword",
        "W008" => "Deprecated feature",
        "W009" => "Unused import",
        "W010" => "Reachable panic",

        // ── Informational (I001–I005) ──
        "I001" => "Compilation started",
        "I002" => "Compilation stage completed",
        "I003" => "Optimization applied",
        "I004" => "Verification passed",
        "I005" => "Build artifact produced",

        _ => "Unknown diagnostic code",
    }
}

/// Returns the category prefix for a diagnostic code.
///
/// - `"E"` for errors, `"W"` for warnings, `"I"` for informational.
pub fn code_category(code: &str) -> &'static str {
    if code.starts_with('E') {
        "error"
    } else if code.starts_with('W') {
        "warning"
    } else if code.starts_with('I') {
        "info"
    } else {
        "unknown"
    }
}

/// Returns the sub-category range for a diagnostic code.
///
/// - `"compilation"` for E001–E030
/// - `"codegen"` for E031–E040
/// - `"verification"` for E041–E050
/// - `"warning"` for W001–W010
/// - `"informational"` for I001–I005
pub fn code_subcategory(code: &str) -> &'static str {
    if let Ok(num) = code[1..].parse::<u32>() {
        match code.chars().next() {
            Some('E') => {
                if num <= 30 {
                    "compilation"
                } else if num <= 40 {
                    "codegen"
                } else if num <= 50 {
                    "verification"
                } else {
                    "unknown"
                }
            }
            Some('W') => "warning",
            Some('I') => "informational",
            _ => "unknown",
        }
    } else {
        "unknown"
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// VumaDiagnostic
// ═══════════════════════════════════════════════════════════════════════════

/// A machine-readable diagnostic with structured fields for LLM consumption.
///
/// This is the primary output type for JSON diagnostics. Every field is
/// serializable and carries enough information for an LLM or IDE to
/// understand, categorize, and potentially auto-fix the issue.
///
/// # Error Chaining
///
/// Diagnostics support causal chains via the [`chain`](VumaDiagnostic::chain)
/// method. A chain represents the cause of a diagnostic: for example,
/// "E003: Type mismatch" may be caused by "E023: Unknown type 'int'".
///
/// # Suggestions
///
/// Each diagnostic can carry zero or more [`Suggestion`] values, which may
/// include precise edit ranges for automatic code fixes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VumaDiagnostic {
    /// Structured diagnostic code, e.g. `"E001"`, `"W001"`.
    pub code: String,
    /// Severity level.
    pub severity: DiagnosticSeverity,
    /// Human-readable message (suitable for display).
    pub message: String,
    /// Compiler stage that produced this diagnostic
    /// (`"parser"`, `"scg"`, `"ir"`, `"codegen"`, etc.).
    pub source: String,
    /// Precise source location.
    pub location: DiagnosticSourceLocation,
    /// Related source locations with explanatory messages.
    pub related: Vec<RelatedInfo>,
    /// Quick-fix suggestions (structured with optional edit ranges).
    #[serde(default)]
    pub suggestions: Vec<Suggestion>,
    /// Causal chain: list of diagnostics that caused this one.
    /// The first element is the immediate cause, the last is the root cause.
    #[serde(default)]
    pub chain: Vec<Box<VumaDiagnostic>>,
    /// Backward-compatible plain-text suggestions (stored alongside
    /// structured suggestions for JSON serialization compatibility).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub legacy_suggestions: Vec<String>,
}

impl VumaDiagnostic {
    /// Create a new diagnostic with all fields.
    pub fn new(
        code: impl Into<String>,
        severity: DiagnosticSeverity,
        message: impl Into<String>,
        source: impl Into<String>,
        location: DiagnosticSourceLocation,
    ) -> Self {
        Self {
            code: code.into(),
            severity,
            message: message.into(),
            source: source.into(),
            location,
            related: Vec::new(),
            suggestions: Vec::new(),
            chain: Vec::new(),
            legacy_suggestions: Vec::new(),
        }
    }

    /// Add a related-info entry.
    pub fn with_related(mut self, info: RelatedInfo) -> Self {
        self.related.push(info);
        self
    }

    /// Add a quick-fix suggestion (plain text, backward-compatible).
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.legacy_suggestions.push(suggestion.into());
        self
    }

    /// Add a structured suggestion with optional edit range.
    pub fn with_structured_suggestion(mut self, suggestion: Suggestion) -> Self {
        self.suggestions.push(suggestion);
        self
    }

    /// Chain another diagnostic as the cause of this one.
    ///
    /// The chained diagnostic becomes the immediate cause. Subsequent
    /// calls to `chain()` push further causes, building a causal chain
    /// from immediate cause to root cause.
    ///
    /// # Example
    ///
    /// ```
    /// use vuma::diagnostics::*;
    ///
    /// let root = VumaDiagnostic::new(
    ///     "E023", DiagnosticSeverity::Error,
    ///     "Unknown type 'int' — did you mean 'i32'?",
    ///     "parser", DiagnosticSourceLocation::unknown()
    /// );
    /// let diag = VumaDiagnostic::new(
    ///     "E003", DiagnosticSeverity::Error,
    ///     "Type mismatch", "parser", DiagnosticSourceLocation::unknown()
    /// ).chain(root);
    ///
    /// assert_eq!(diag.chain.len(), 1);
    /// assert_eq!(diag.chain[0].code, "E023");
    /// ```
    pub fn chain(mut self, cause: VumaDiagnostic) -> Self {
        self.chain.push(Box::new(cause));
        self
    }

    /// Returns the root cause of this diagnostic (the last element in the
    /// causal chain), or `None` if there is no chain.
    pub fn root_cause(&self) -> Option<&VumaDiagnostic> {
        self.chain.last().map(|b| b.as_ref())
    }

    /// Returns the immediate cause of this diagnostic (the first element
    /// in the causal chain), or `None` if there is no chain.
    pub fn immediate_cause(&self) -> Option<&VumaDiagnostic> {
        self.chain.first().map(|b| b.as_ref())
    }

    /// Returns the full causal chain as a slice of diagnostics.
    pub fn causal_chain(&self) -> &[Box<VumaDiagnostic>] {
        &self.chain
    }

    /// Returns `true` if this diagnostic has a causal chain.
    pub fn has_chain(&self) -> bool {
        !self.chain.is_empty()
    }

    /// Returns all suggestions, combining both structured and legacy
    /// plain-text suggestions.
    pub fn all_suggestions(&self) -> Vec<&Suggestion> {
        self.suggestions.iter().collect()
    }

    /// Returns `true` if this diagnostic has any machine-applicable
    /// suggestions (i.e. suggestions with edit ranges).
    pub fn has_machine_applicable_fixes(&self) -> bool {
        self.suggestions.iter().any(|s| {
            s.edit_range.is_some()
                && s.replacement.is_some()
                && s.applicability == SuggestionApplicability::MachineApplicable
        })
    }

    // ── Output formats ────────────────────────────────────────────────

    /// Serialize this diagnostic as a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"code":"E000","severity":"error","message":"JSON serialization failed","source":"diagnostics","location":{"file":"","start_line":0,"start_col":0,"end_line":0,"end_col":0},"related":[],"suggestions":[],"chain":[]}"#.to_string()
        })
    }

    /// Serialize this diagnostic as a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| self.to_json())
    }

    /// Format this diagnostic as plain text (for logs, no ANSI colors).
    ///
    /// Format: `severity[code]: message (source at location)`
    pub fn to_plain_text(&self) -> String {
        let mut out = format!(
            "{}[{}]: {} ({} at {})",
            self.severity, self.code, self.message, self.source, self.location
        );
        for related in &self.related {
            out.push_str(&format!("\n  note: {} at {}", related.message, related.location));
        }
        for suggestion in &self.suggestions {
            out.push_str(&format!("\n  help: {}", suggestion.message));
            if let Some(ref repl) = suggestion.replacement {
                out.push_str(&format!(" → `{}`", repl));
            }
        }
        for suggestion in &self.legacy_suggestions {
            out.push_str(&format!("\n  help: {}", suggestion));
        }
        for cause in &self.chain {
            out.push_str(&format!(
                "\n  caused by: {}[{}]: {}",
                cause.severity, cause.code, cause.message
            ));
        }
        out
    }

    /// Format this diagnostic as ANSI-colored terminal output (rich text).
    ///
    /// Uses standard compiler diagnostic coloring:
    /// - Errors in red, warnings in yellow, info in cyan, hints in blue.
    /// - Code and source in bold.
    /// - Suggestions in green.
    /// - Causal chain indented with "caused by" prefix.
    pub fn to_rich_text(&self) -> String {
        let mut out = String::new();

        // Severity prefix with color
        let (sev_label, sev_code) = match self.severity {
            DiagnosticSeverity::Error => ("\x1b[31merror\x1b[0m", "\x1b[31;1m"),
            DiagnosticSeverity::Warning => ("\x1b[33mwarning\x1b[0m", "\x1b[33;1m"),
            DiagnosticSeverity::Info => ("\x1b[36minfo\x1b[0m", "\x1b[36;1m"),
            DiagnosticSeverity::Hint => ("\x1b[34mhint\x1b[0m", "\x1b[34;1m"),
        };

        // Main line: severity[code]: message
        out.push_str(&format!(
            "{}[\x1b[1m{}{}\x1b[0m]: {}\n",
            sev_label, sev_code, self.code, self.message
        ));

        // Location line
        if !self.location.is_unknown() {
            out.push_str(&format!(
                "  \x1b[2m--> {}:{}:{}\x1b[0m\n",
                self.location.file, self.location.start_line, self.location.start_col
            ));
        }

        // Source indicator
        out.push_str(&format!("  \x1b[2m[{}]\x1b[0m\n", self.source));

        // Related info
        for related in &self.related {
            out.push_str(&format!(
                "  \x1b[36mnote\x1b[0m: {} \x1b[2m(at {})\x1b[0m\n",
                related.message, related.location
            ));
        }

        // Structured suggestions
        for suggestion in &self.suggestions {
            out.push_str(&format!(
                "  \x1b[32mhelp\x1b[0m: {}",
                suggestion.message
            ));
            if let Some(ref repl) = suggestion.replacement {
                out.push_str(&format!(" → \x1b[32m`{}`\x1b[0m", repl));
            }
            if suggestion.applicability == SuggestionApplicability::MachineApplicable {
                out.push_str(" \x1b[2m[machine-applicable]\x1b[0m");
            }
            out.push('\n');
        }

        // Legacy suggestions
        for suggestion in &self.legacy_suggestions {
            out.push_str(&format!("  \x1b[32mhelp\x1b[0m: {}\n", suggestion));
        }

        // Causal chain (recursively format)
        for (i, cause) in self.chain.iter().enumerate() {
            let indent = "  ".repeat(i + 1);
            let (cause_sev, cause_code) = match cause.severity {
                DiagnosticSeverity::Error => ("\x1b[31merror\x1b[0m", "\x1b[31;1m"),
                DiagnosticSeverity::Warning => ("\x1b[33mwarning\x1b[0m", "\x1b[33;1m"),
                DiagnosticSeverity::Info => ("\x1b[36minfo\x1b[0m", "\x1b[36;1m"),
                DiagnosticSeverity::Hint => ("\x1b[34mhint\x1b[0m", "\x1b[34;1m"),
            };
            out.push_str(&format!(
                "{}\x1b[2mcaused by:\x1b[0m {}[\x1b[1m{}{}\x1b[0m]: {}\n",
                indent, cause_sev, cause_code, cause.code, cause.message
            ));
            if !cause.location.is_unknown() {
                out.push_str(&format!(
                    "{}  \x1b[2m--> {}:{}:{}\x1b[0m\n",
                    indent, cause.location.file, cause.location.start_line, cause.location.start_col
                ));
            }
        }

        out
    }

    /// Format this diagnostic as an LSP Diagnostic.
    ///
    /// Returns a JSON `Value` conforming to the LSP Diagnostic structure:
    /// - `range`: 0-based Position range
    /// - `severity`: LSP numeric severity (1=Error, 2=Warning, 3=Info, 4=Hint)
    /// - `code`: the diagnostic code string
    /// - `source`: the compiler stage
    /// - `message`: the human-readable message
    /// - `relatedInformation`: optional related locations
    pub fn to_lsp(&self) -> serde_json::Value {
        use serde_json::json;

        let start_line = if self.location.start_line > 0 {
            self.location.start_line - 1
        } else {
            0
        };
        let start_col = if self.location.start_col > 0 {
            self.location.start_col - 1
        } else {
            0
        };
        let end_line = if self.location.end_line > 0 {
            self.location.end_line - 1
        } else {
            0
        };
        let end_col = if self.location.end_col > 0 {
            self.location.end_col - 1
        } else {
            0
        };

        let mut diagnostic = json!({
            "range": {
                "start": { "line": start_line, "character": start_col },
                "end": { "line": end_line, "character": end_col }
            },
            "severity": self.severity.to_lsp_severity(),
            "code": self.code,
            "source": self.source,
            "message": self.message,
        });

        // Add related information if present
        if !self.related.is_empty() {
            let related: Vec<serde_json::Value> = self.related.iter().map(|r| {
                let r_start_line = if r.location.start_line > 0 { r.location.start_line - 1 } else { 0 };
                let r_start_col = if r.location.start_col > 0 { r.location.start_col - 1 } else { 0 };
                let r_end_line = if r.location.end_line > 0 { r.location.end_line - 1 } else { 0 };
                let r_end_col = if r.location.end_col > 0 { r.location.end_col - 1 } else { 0 };
                json!({
                    "location": {
                        "uri": format!("file://{}", r.location.file),
                        "range": {
                            "start": { "line": r_start_line, "character": r_start_col },
                            "end": { "line": r_end_line, "character": r_end_col }
                        }
                    },
                    "message": r.message
                })
            }).collect();
            diagnostic["relatedInformation"] = json!(related);
        }

        // Add LSP CodeDescription with href if it's a known code
        let desc = code_description(&self.code);
        if desc != "Unknown diagnostic code" {
            diagnostic["codeDescription"] = json!({
                "href": format!("https://vuma.dev/docs/diagnostics/{}", self.code)
            });
        }

        // Add tags for warnings (Unnecessary = 1, Deprecated = 2)
        let mut tags: Vec<u32> = Vec::new();
        match self.code.as_str() {
            "W001" | "W004" | "W009" => tags.push(1), // Unnecessary
            "W008" => tags.push(2),                     // Deprecated
            _ => {}
        }
        if !tags.is_empty() {
            diagnostic["tags"] = json!(tags);
        }

        diagnostic
    }
}

impl fmt::Display for VumaDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}[{}]: {} ({} at {})",
            self.severity, self.code, self.message, self.source, self.location
        )?;
        for related in &self.related {
            write!(f, "\n  note: {} at {}", related.message, related.location)?;
        }
        for suggestion in &self.suggestions {
            write!(f, "\n  help: {}", suggestion.message)?;
            if let Some(ref repl) = suggestion.replacement {
                write!(f, " → `{}`", repl)?;
            }
        }
        for suggestion in &self.legacy_suggestions {
            write!(f, "\n  help: {}", suggestion)?;
        }
        for cause in &self.chain {
            write!(
                f,
                "\n  caused by: {}[{}]: {}",
                cause.severity, cause.code, cause.message
            )?;
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// JSON array serialization
// ═══════════════════════════════════════════════════════════════════════════

/// Serialize a slice of diagnostics as a JSON array.
pub fn diagnostics_to_json(diagnostics: &[VumaDiagnostic]) -> String {
    serde_json::to_string(diagnostics).unwrap_or_else(|_| "[]".to_string())
}

/// Serialize a slice of diagnostics as a pretty-printed JSON array.
pub fn diagnostics_to_json_pretty(diagnostics: &[VumaDiagnostic]) -> String {
    serde_json::to_string_pretty(diagnostics).unwrap_or_else(|_| "[]".to_string())
}

// ═══════════════════════════════════════════════════════════════════════════
// DiagnosticSummary — error statistics
// ═══════════════════════════════════════════════════════════════════════════

/// Summary statistics for a collection of diagnostics.
///
/// Counts diagnostics by severity and by individual code, providing
/// a quick overview of the health of a compilation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagnosticSummary {
    /// Total number of diagnostics.
    pub total: usize,
    /// Number of errors.
    pub errors: usize,
    /// Number of warnings.
    pub warnings: usize,
    /// Number of informational messages.
    pub infos: usize,
    /// Number of hints.
    pub hints: usize,
    /// Count per diagnostic code (e.g. `"E001"` → 3).
    pub by_code: HashMap<String, usize>,
    /// Count per source (e.g. `"parser"` → 5).
    pub by_source: HashMap<String, usize>,
    /// Count per subcategory (e.g. `"compilation"` → 3).
    pub by_subcategory: HashMap<String, usize>,
}

impl DiagnosticSummary {
    /// Create a new empty summary.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a summary from a slice of diagnostics.
    pub fn from_diagnostics(diagnostics: &[VumaDiagnostic]) -> Self {
        let mut summary = Self::new();
        for diag in diagnostics {
            summary.add(diag);
        }
        summary
    }

    /// Add a single diagnostic to the summary.
    pub fn add(&mut self, diag: &VumaDiagnostic) {
        self.total += 1;
        match diag.severity {
            DiagnosticSeverity::Error => self.errors += 1,
            DiagnosticSeverity::Warning => self.warnings += 1,
            DiagnosticSeverity::Info => self.infos += 1,
            DiagnosticSeverity::Hint => self.hints += 1,
        }
        *self.by_code.entry(diag.code.clone()).or_insert(0) += 1;
        *self.by_source.entry(diag.source.clone()).or_insert(0) += 1;
        let subcat = code_subcategory(&diag.code).to_string();
        *self.by_subcategory.entry(subcat).or_insert(0) += 1;
    }

    /// Returns `true` if there are any errors.
    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }

    /// Returns `true` if there are any warnings.
    pub fn has_warnings(&self) -> bool {
        self.warnings > 0
    }

    /// Returns the count for a specific diagnostic code.
    pub fn count_for_code(&self, code: &str) -> usize {
        self.by_code.get(code).copied().unwrap_or(0)
    }

    /// Returns the count for a specific source.
    pub fn count_for_source(&self, source: &str) -> usize {
        self.by_source.get(source).copied().unwrap_or(0)
    }

    /// Serialize this summary as JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Serialize this summary as pretty-printed JSON.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

impl fmt::Display for DiagnosticSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "diagnostic summary: {} total ({} errors, {} warnings, {} info, {} hints)",
            self.total, self.errors, self.warnings, self.infos, self.hints
        )?;
        if !self.by_code.is_empty() {
            write!(f, "\n  by code:")?;
            let mut codes: Vec<_> = self.by_code.iter().collect();
            codes.sort_by_key(|(k, _)| k.as_str());
            for (code, count) in codes {
                write!(f, "\n    {} × {}", code, count)?;
            }
        }
        if !self.by_source.is_empty() {
            write!(f, "\n  by source:")?;
            let mut sources: Vec<_> = self.by_source.iter().collect();
            sources.sort_by_key(|(k, _)| k.as_str());
            for (source, count) in sources {
                write!(f, "\n    {} × {}", source, count)?;
            }
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration: ParseError → VumaDiagnostic
// ═══════════════════════════════════════════════════════════════════════════

/// Convert a [`ParseError`] to a [`VumaDiagnostic`].
///
/// `source` is the full source text (used to compute line/column from byte
/// offsets). `file` is the optional file path.
pub fn from_parse_error(
    err: &ParseError,
    source: &str,
    file: Option<&str>,
) -> VumaDiagnostic {
    let code = code_for_parse_error_kind(&err.kind).to_string();
    let loc = offset_to_location(source, err.span.start, file);

    // Compute end position from span end.
    let end_loc = if err.span.end > err.span.start && err.span.end <= source.len() {
        offset_to_location(source, err.span.end, file)
    } else {
        loc.clone()
    };

    let diag_loc = DiagnosticSourceLocation {
        file: loc.file.clone().unwrap_or_default(),
        start_line: loc.line as u32 + 1, // convert 0-based to 1-based
        start_col: loc.column as u32 + 1,
        end_line: end_loc.line as u32 + 1,
        end_col: end_loc.column as u32 + 1,
    };

    let mut diag = VumaDiagnostic::new(
        code,
        DiagnosticSeverity::Error,
        &err.message,
        "parser",
        diag_loc,
    );

    if let Some(ref suggestion) = err.suggestion {
        diag = diag.with_suggestion(format!("did you mean '{}'?", suggestion));
    }

    // Store the line text as a hint if available.
    if let Some(ref line_text) = loc.line_text {
        diag = diag.with_suggestion(format!("source: {}", line_text.trim()));
    }

    diag
}

/// Convert multiple [`ParseError`]s into [`VumaDiagnostic`]s.
pub fn from_parse_errors(
    errors: &[ParseError],
    source: &str,
    file: Option<&str>,
) -> Vec<VumaDiagnostic> {
    errors.iter().map(|e| from_parse_error(e, source, file)).collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration: VumaError → Vec<VumaDiagnostic>
// ═══════════════════════════════════════════════════════════════════════════

/// Convert a [`VumaError`] to one or more [`VumaDiagnostic`]s.
///
/// Since `VumaError` does not carry the original source text, the resulting
/// diagnostics will have [`DiagnosticSourceLocation::unknown()`] for variants
/// that don't carry location information. For parse errors, prefer
/// [`from_parse_error`] which can compute precise locations.
pub fn from_vuma_error(err: &VumaError) -> Vec<VumaDiagnostic> {
    match err {
        VumaError::Parse { errors } => {
            // Parse errors need source text for location computation;
            // without it, we produce diagnostics with unknown locations.
            errors
                .iter()
                .map(|pe| {
                    let code = code_for_parse_error_kind(&pe.kind).to_string();
                    let mut diag = VumaDiagnostic::new(
                        code,
                        DiagnosticSeverity::Error,
                        &pe.message,
                        "parser",
                        DiagnosticSourceLocation::unknown(),
                    );
                    if let Some(ref suggestion) = pe.suggestion {
                        diag = diag.with_suggestion(format!("did you mean '{}'?", suggestion));
                    }
                    diag
                })
                .collect()
        }
        VumaError::AstToScg { message } => {
            vec![VumaDiagnostic::new(
                "E024",
                DiagnosticSeverity::Error,
                message,
                "scg",
                DiagnosticSourceLocation::unknown(),
            )]
        }
        VumaError::ScgValidation { errors } => errors
            .iter()
            .map(|msg| {
                VumaDiagnostic::new(
                    "E041",
                    DiagnosticSeverity::Error,
                    msg,
                    "scg",
                    DiagnosticSourceLocation::unknown(),
                )
            })
            .collect(),
        VumaError::ScgToMsg { error } => {
            vec![VumaDiagnostic::new(
                "E034",
                DiagnosticSeverity::Error,
                error.to_string(),
                "msg",
                DiagnosticSourceLocation::unknown(),
            )]
        }
        VumaError::BdInference { node_id, message } => {
            let mut diag = VumaDiagnostic::new(
                "E048",
                DiagnosticSeverity::Error,
                message,
                "bd",
                DiagnosticSourceLocation::unknown(),
            );
            if let Some(id) = node_id {
                diag = diag.with_suggestion(format!("at SCG node {}", id));
            }
            vec![diag]
        }
        VumaError::Verification { result } => {
            vec![VumaDiagnostic::new(
                "E042",
                DiagnosticSeverity::Error,
                format!("verification failed: {}", result.overall),
                "ive",
                DiagnosticSourceLocation::unknown(),
            )]
        }
        VumaError::Transform { pass_name, errors } => errors
            .iter()
            .map(|msg| {
                VumaDiagnostic::new(
                    "E041",
                    DiagnosticSeverity::Error,
                    msg,
                    format!("scg-transform:{}", pass_name),
                    DiagnosticSourceLocation::unknown(),
                )
            })
            .collect(),
        VumaError::Codegen { error } => {
            vec![from_codegen_error(error)]
        }
        VumaError::RegisterAlloc { message } => {
            vec![VumaDiagnostic::new(
                "E032",
                DiagnosticSeverity::Error,
                message,
                "register-alloc",
                DiagnosticSourceLocation::unknown(),
            )]
        }
        VumaError::Emission { message } => {
            vec![VumaDiagnostic::new(
                "E035",
                DiagnosticSeverity::Error,
                message,
                "elf-emission",
                DiagnosticSourceLocation::unknown(),
            )]
        }
        VumaError::CorInit { message } => {
            vec![VumaDiagnostic::new(
                "E024",
                DiagnosticSeverity::Error,
                message,
                "cor-init",
                DiagnosticSourceLocation::unknown(),
            )]
        }
        VumaError::Multi { errors } => errors.iter().flat_map(from_vuma_error).collect(),
        VumaError::ModuleResolution { errors } => errors
            .iter()
            .flat_map(|e| {
                vec![VumaDiagnostic::new(
                    "E024",
                    DiagnosticSeverity::Error,
                    e.to_string(),
                    "module-resolution",
                    DiagnosticSourceLocation::unknown(),
                )]
            })
            .collect(),
        VumaError::BackendFallback { failed_backend, fallback_backend, error } => {
            let mut msg = format!("backend '{}' failed: {}", failed_backend, error);
            if let Some(fb) = fallback_backend {
                msg.push_str(&format!(", attempting fallback to '{}'", fb));
            }
            vec![VumaDiagnostic::new(
                "E031",
                DiagnosticSeverity::Warning,
                &msg,
                "backend-fallback",
                DiagnosticSourceLocation::unknown(),
            )]
        }
        VumaError::PanicCaught { stage, message } => {
            vec![VumaDiagnostic::new(
                "E050",
                DiagnosticSeverity::Error,
                &format!("internal panic in stage '{}': {}", stage, message),
                stage,
                DiagnosticSourceLocation::unknown(),
            )]
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Integration: CodegenError → VumaDiagnostic
// ═══════════════════════════════════════════════════════════════════════════

/// Convert a [`CodegenError`] to a [`VumaDiagnostic`].
pub fn from_codegen_error(err: &CodegenError) -> VumaDiagnostic {
    let code = code_for_codegen_error(err).to_string();
    let message = err.to_string();

    let (source, suggestion) = match err {
        CodegenError::UnknownVariable { name } => (
            "codegen",
            Some(Suggestion::edit(
                format!("declare '{}' before use", name),
                DiagnosticSourceLocation::unknown(),
                format!("let {} = …;", name),
            )),
        ),
        CodegenError::RegisterAllocFailed(_) => (
            "register-alloc",
            Some(Suggestion::text(
                "reduce register pressure by splitting live ranges".to_string(),
            )),
        ),
        CodegenError::TranslationError(_) => (
            "ir",
            None,
        ),
        CodegenError::EncodingError(_) => (
            "codegen",
            None,
        ),
        CodegenError::ElfError(_) => (
            "emit",
            None,
        ),
        CodegenError::InvalidInstruction(_) => (
            "codegen",
            None,
        ),
        CodegenError::WasmSectionNotFound { section } => (
            "codegen",
            Some(Suggestion::text(
                format!("ensure '{}' section is generated", section),
            )),
        ),
        CodegenError::UnresolvedRelocation { symbol, function, .. } => (
            "codegen",
            Some(Suggestion::text(
                format!("define function '{}' or provide an external definition for '{}' referenced in '{}'", symbol, symbol, function),
            )),
        ),
    };

    let mut diag = VumaDiagnostic::new(
        code,
        DiagnosticSeverity::Error,
        message,
        source,
        DiagnosticSourceLocation::unknown(),
    );

    if let Some(s) = suggestion {
        diag = diag.with_structured_suggestion(s);
    }

    diag
}

// ═══════════════════════════════════════════════════════════════════════════
// Memory safety diagnostics (E041–E050)
// ═══════════════════════════════════════════════════════════════════════════

/// Convert a [`MemorySafetyViolation`] into a [`VumaDiagnostic`].
///
/// Maps each violation to its E041–E050 code and creates a structured
/// diagnostic with source location and quick-fix suggestions where
/// applicable.
pub fn from_memory_safety_violation(violation: &vuma_codegen::MemorySafetyViolation) -> VumaDiagnostic {
    use vuma_codegen::MemorySafetyViolation;

    let code = violation.code().to_string();
    let message = violation.description();
    let severity = DiagnosticSeverity::Error;

    let (source, suggestion): (&str, Option<Suggestion>) = match violation {
        MemorySafetyViolation::UseAfterFree { allocation_name, .. } => (
            "memory-safety",
            Some(Suggestion::text(format!(
                "ensure '{}' is not accessed after it is freed", allocation_name
            ))),
        ),
        MemorySafetyViolation::DoubleFree { allocation_name, .. } => (
            "memory-safety",
            Some(Suggestion::text(format!(
                "'{}' is freed more than once — remove the duplicate free", allocation_name
            ))),
        ),
        MemorySafetyViolation::MemoryLeak { allocation_name, .. } => (
            "memory-safety",
            Some(Suggestion::text(format!(
                "add a 'free({})' before the function returns", allocation_name
            ))),
        ),
        MemorySafetyViolation::BoundsCheckFailure { array_name, .. } => (
            "runtime-safety",
            Some(Suggestion::text(format!(
                "check the index before accessing '{}'", array_name
            ))),
        ),
        MemorySafetyViolation::NullDereference { pointer_name } => (
            "memory-safety",
            Some(Suggestion::text(format!(
                "add a null check before dereferencing '{}'", pointer_name
            ))),
        ),
        MemorySafetyViolation::DanglingPointer { pointer_name, .. } => (
            "memory-safety",
            Some(Suggestion::text(format!(
                "'{}' references a stack allocation that escapes its scope — allocate on the heap instead",
                pointer_name
            ))),
        ),
        MemorySafetyViolation::UninitializedRead { variable_name } => (
            "memory-safety",
            Some(Suggestion::text(format!(
                "initialize '{}' before reading it", variable_name
            ))),
        ),
        MemorySafetyViolation::BufferOverflow { buffer_name, .. } => (
            "runtime-safety",
            Some(Suggestion::text(format!(
                "check the offset before writing to '{}'", buffer_name
            ))),
        ),
        MemorySafetyViolation::UseAfterScope { variable_name, .. } => (
            "memory-safety",
            Some(Suggestion::text(format!(
                "'{}' is used after its scope ends — extend the scope or copy the value",
                variable_name
            ))),
        ),
        MemorySafetyViolation::InvalidFree { pointer_name, reason } => (
            "memory-safety",
            Some(Suggestion::text(format!(
                "'{}' cannot be freed: {} — check the pointer origin", pointer_name, reason
            ))),
        ),
    };

    let mut diag = VumaDiagnostic::new(
        code,
        severity,
        message,
        source,
        DiagnosticSourceLocation::unknown(),
    );

    if let Some(s) = suggestion {
        diag = diag.with_structured_suggestion(s);
    }

    diag
}

/// Convert all violations from a [`MemorySafetyReport`] into diagnostics.
pub fn from_memory_safety_report(report: &vuma_codegen::MemorySafetyReport) -> Vec<VumaDiagnostic> {
    report
        .violations
        .iter()
        .map(from_memory_safety_violation)
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// Convenience constructors for common diagnostics
// ═══════════════════════════════════════════════════════════════════════════

/// Create an E001 (syntax error) diagnostic.
pub fn syntax_error(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E001", DiagnosticSeverity::Error, message, "parser", location)
}

/// Create an E002 (undefined variable) diagnostic.
pub fn undefined_variable(
    name: &str,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "E002",
        DiagnosticSeverity::Error,
        format!("undefined variable `{}`", name),
        "parser",
        location,
    )
}

/// Create an E003 (type mismatch) diagnostic.
pub fn type_mismatch(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E003", DiagnosticSeverity::Error, message, "parser", location)
}

/// Create an E004 (duplicate definition) diagnostic.
pub fn duplicate_definition(
    name: &str,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "E004",
        DiagnosticSeverity::Error,
        format!("duplicate definition of `{}`", name),
        "parser",
        location,
    )
}

/// Create an E005 (invalid argument count) diagnostic.
pub fn invalid_arg_count(
    expected: usize,
    found: usize,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "E005",
        DiagnosticSeverity::Error,
        format!("expected {} argument(s), found {}", expected, found),
        "parser",
        location,
    )
}

/// Create an E006 (invalid type) diagnostic.
pub fn invalid_type(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E006", DiagnosticSeverity::Error, message, "parser", location)
}

/// Create an E007 (missing return) diagnostic.
pub fn missing_return(location: DiagnosticSourceLocation) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "E007",
        DiagnosticSeverity::Error,
        "function may not return a value",
        "parser",
        location,
    )
}

/// Create an E008 (unreachable code) diagnostic.
pub fn unreachable_code(location: DiagnosticSourceLocation) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "E008",
        DiagnosticSeverity::Warning,
        "unreachable code detected",
        "parser",
        location,
    )
}

/// Create an E024 (name resolution error) diagnostic.
pub fn name_resolution_error(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E024", DiagnosticSeverity::Error, message, "name-resolution", location)
}

/// Create an E025 (circular dependency) diagnostic.
pub fn circular_dependency(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E025", DiagnosticSeverity::Error, message, "parser", location)
}

/// Create an E026 (invalid assignment target) diagnostic.
pub fn invalid_assignment_target(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E026", DiagnosticSeverity::Error, message, "parser", location)
}

/// Create an E027 (break/continue outside loop) diagnostic.
pub fn break_outside_loop(location: DiagnosticSourceLocation) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "E027",
        DiagnosticSeverity::Error,
        "break statement outside of loop",
        "parser",
        location,
    )
}

/// Create an E028 (invalid cast) diagnostic.
pub fn invalid_cast(
    from: &str,
    to: &str,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "E028",
        DiagnosticSeverity::Error,
        format!("cannot cast from `{}` to `{}`", from, to),
        "parser",
        location,
    )
}

/// Create an E029 (missing function body) diagnostic.
pub fn missing_function_body(
    name: &str,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "E029",
        DiagnosticSeverity::Error,
        format!("function `{}` has no body", name),
        "parser",
        location,
    )
}

/// Create an E030 (invalid visibility modifier) diagnostic.
pub fn invalid_visibility(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E030", DiagnosticSeverity::Error, message, "parser", location)
}

/// Create an E031 (invalid instruction) diagnostic.
pub fn invalid_instruction(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E031", DiagnosticSeverity::Error, message, "codegen", location)
}

/// Create an E032 (register allocation failed) diagnostic.
pub fn register_alloc_failed(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "E032",
        DiagnosticSeverity::Error,
        message,
        "register-alloc",
        location,
    ).with_structured_suggestion(Suggestion::text(
        "reduce register pressure by splitting live ranges",
    ))
}

/// Create an E033 (encoding error) diagnostic.
pub fn encoding_error(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E033", DiagnosticSeverity::Error, message, "codegen", location)
}

/// Create an E037 (relocation error) diagnostic.
pub fn relocation_error(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E037", DiagnosticSeverity::Error, message, "linker", location)
}

/// Create an E038 (stack layout error) diagnostic.
pub fn stack_layout_error(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E038", DiagnosticSeverity::Error, message, "codegen", location)
}

/// Create an E039 (linker error) diagnostic.
pub fn linker_error(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E039", DiagnosticSeverity::Error, message, "linker", location)
}

/// Create an E040 (target unsupported feature) diagnostic.
pub fn unsupported_feature(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "E040",
        DiagnosticSeverity::Error,
        message,
        "codegen",
        location,
    )
}

/// Create an E041 (invariant violation) diagnostic.
pub fn invariant_violation(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E041", DiagnosticSeverity::Error, message, "ive", location)
}

/// Create an E042 (proof failure) diagnostic.
pub fn proof_failure(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E042", DiagnosticSeverity::Error, message, "proof", location)
}

/// Create an E043 (liveness invariant violated) diagnostic.
pub fn liveness_violation(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E043", DiagnosticSeverity::Error, message, "ive", location)
}

/// Create an E044 (origin invariant violated) diagnostic.
pub fn origin_violation(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E044", DiagnosticSeverity::Error, message, "ive", location)
}

/// Create an E045 (exclusivity invariant violated) diagnostic.
pub fn exclusivity_violation(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E045", DiagnosticSeverity::Error, message, "ive", location)
}

/// Create an E046 (interpretation invariant violated) diagnostic.
pub fn interpretation_violation(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E046", DiagnosticSeverity::Error, message, "ive", location)
}

/// Create an E047 (cleanup invariant violated) diagnostic.
pub fn cleanup_violation(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E047", DiagnosticSeverity::Error, message, "ive", location)
}

/// Create an E048 (BD inference error) diagnostic.
pub fn bd_inference_error(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E048", DiagnosticSeverity::Error, message, "bd", location)
}

/// Create an E049 (constraint unsatisfiable) diagnostic.
pub fn constraint_unsatisfiable(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new("E049", DiagnosticSeverity::Error, message, "ive", location)
}

/// Create an E050 (verification timeout) diagnostic.
pub fn verification_timeout(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "E050",
        DiagnosticSeverity::Error,
        message,
        "ive",
        location,
    ).with_structured_suggestion(Suggestion::with_placeholders(
        "consider simplifying the code or increasing the verification timeout",
    ))
}

/// Create a W001 (unused variable) diagnostic.
pub fn unused_variable(name: &str, location: DiagnosticSourceLocation) -> VumaDiagnostic {
    let edit_loc = location.clone();
    VumaDiagnostic::new(
        "W001",
        DiagnosticSeverity::Warning,
        format!("unused variable `{}`", name),
        "parser",
        location,
    )
    .with_structured_suggestion(Suggestion::edit(
        format!("prefix with `_{}' to suppress", name),
        edit_loc,
        format!("_{}", name),
    ))
}

/// Create a W002 (implicit type conversion) diagnostic.
pub fn implicit_conversion(
    from: &str,
    to: &str,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "W002",
        DiagnosticSeverity::Warning,
        format!("implicit conversion from `{}` to `{}`", from, to),
        "parser",
        location,
    ).with_suggestion(format!("add explicit `as {}' cast", to))
}

/// Create a W003 (large constant performance hint) diagnostic.
pub fn large_constant(value: &str, location: DiagnosticSourceLocation) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "W003",
        DiagnosticSeverity::Hint,
        format!("large constant `{}` may require multiple instructions", value),
        "codegen",
        location,
    )
}

/// Create a W004 (dead code) diagnostic.
pub fn dead_code(message: impl Into<String>, location: DiagnosticSourceLocation) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "W004",
        DiagnosticSeverity::Warning,
        message,
        "parser",
        location,
    )
}

/// Create a W005 (redundant cast) diagnostic.
pub fn redundant_cast(
    from: &str,
    to: &str,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    let edit_loc = location.clone();
    VumaDiagnostic::new(
        "W005",
        DiagnosticSeverity::Warning,
        format!("redundant cast from `{}` to `{}`", from, to),
        "parser",
        location,
    ).with_structured_suggestion(Suggestion::edit(
        "remove the redundant cast",
        edit_loc,
        "", // remove the cast expression
    ))
}

/// Create a W006 (shadowed variable) diagnostic.
pub fn shadowed_variable(
    name: &str,
    location: DiagnosticSourceLocation,
    original_location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "W006",
        DiagnosticSeverity::Warning,
        format!("variable `{}` shadows a previous declaration", name),
        "parser",
        location,
    ).with_related(RelatedInfo::new(original_location, "previous declaration here"))
}

/// Create a W007 (unnecessary mut keyword) diagnostic.
pub fn unnecessary_mut(
    name: &str,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    let edit_loc = location.clone();
    VumaDiagnostic::new(
        "W007",
        DiagnosticSeverity::Warning,
        format!("variable `{}` is declared mut but never modified", name),
        "parser",
        location,
    ).with_structured_suggestion(Suggestion::machine_applicable(
        "remove 'mut' keyword — variables are mutable by default",
        edit_loc,
        "", // remove 'mut'
    ))
}

/// Create a W008 (deprecated feature) diagnostic.
pub fn deprecated_feature(
    feature: &str,
    replacement: Option<&str>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    let mut diag = VumaDiagnostic::new(
        "W008",
        DiagnosticSeverity::Warning,
        format!("use of deprecated feature '{}'", feature),
        "parser",
        location,
    );
    if let Some(repl) = replacement {
        diag = diag.with_suggestion(format!("use '{}' instead", repl));
    }
    diag
}

/// Create a W009 (unused import) diagnostic.
pub fn unused_import(name: &str, location: DiagnosticSourceLocation) -> VumaDiagnostic {
    let edit_loc = location.clone();
    VumaDiagnostic::new(
        "W009",
        DiagnosticSeverity::Warning,
        format!("unused import `{}`", name),
        "parser",
        location,
    ).with_structured_suggestion(Suggestion::machine_applicable(
        format!("remove unused import `{}`", name),
        edit_loc,
        "",
    ))
}

/// Create a W010 (reachable panic) diagnostic.
pub fn reachable_panic(
    message: impl Into<String>,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "W010",
        DiagnosticSeverity::Warning,
        message,
        "parser",
        location,
    )
}

/// Create an I001 (compilation started) diagnostic.
pub fn compilation_started(file: &str) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "I001",
        DiagnosticSeverity::Info,
        format!("compilation started: {}", file),
        "pipeline",
        DiagnosticSourceLocation::unknown(),
    )
}

/// Create an I002 (compilation stage completed) diagnostic.
pub fn stage_completed(stage: &str) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "I002",
        DiagnosticSeverity::Info,
        format!("stage '{}' completed", stage),
        "pipeline",
        DiagnosticSourceLocation::unknown(),
    )
}

/// Create an I003 (optimization applied) diagnostic.
pub fn optimization_applied(
    pass: &str,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "I003",
        DiagnosticSeverity::Info,
        format!("optimization '{}' applied", pass),
        "optimizer",
        location,
    )
}

/// Create an I004 (verification passed) diagnostic.
pub fn verification_passed(
    invariant: &str,
    location: DiagnosticSourceLocation,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "I004",
        DiagnosticSeverity::Info,
        format!("verification passed: {}", invariant),
        "ive",
        location,
    )
}

/// Create an I005 (build artifact produced) diagnostic.
pub fn artifact_provided(
    artifact: &str,
    size_bytes: usize,
) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "I005",
        DiagnosticSeverity::Info,
        format!("build artifact '{}' produced ({} bytes)", artifact, size_bytes),
        "pipeline",
        DiagnosticSourceLocation::unknown(),
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // -- DiagnosticSeverity tests --------------------------------------------

    #[test]
    fn severity_display() {
        assert_eq!(DiagnosticSeverity::Error.to_string(), "error");
        assert_eq!(DiagnosticSeverity::Warning.to_string(), "warning");
        assert_eq!(DiagnosticSeverity::Info.to_string(), "info");
        assert_eq!(DiagnosticSeverity::Hint.to_string(), "hint");
    }

    #[test]
    fn severity_from_parser_severity() {
        assert_eq!(
            DiagnosticSeverity::from(ParserSeverity::Error),
            DiagnosticSeverity::Error
        );
        assert_eq!(
            DiagnosticSeverity::from(ParserSeverity::Warning),
            DiagnosticSeverity::Warning
        );
        assert_eq!(
            DiagnosticSeverity::from(ParserSeverity::Note),
            DiagnosticSeverity::Info
        );
    }

    #[test]
    fn severity_json_roundtrip() {
        let json = serde_json::to_string(&DiagnosticSeverity::Error).unwrap();
        assert_eq!(json, "\"error\"");
        let back: DiagnosticSeverity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, DiagnosticSeverity::Error);
    }

    #[test]
    fn severity_to_lsp() {
        assert_eq!(DiagnosticSeverity::Error.to_lsp_severity(), 1);
        assert_eq!(DiagnosticSeverity::Warning.to_lsp_severity(), 2);
        assert_eq!(DiagnosticSeverity::Info.to_lsp_severity(), 3);
        assert_eq!(DiagnosticSeverity::Hint.to_lsp_severity(), 4);
    }

    // -- DiagnosticSourceLocation tests --------------------------------------

    #[test]
    fn location_point() {
        let loc = DiagnosticSourceLocation::point("main.vu", 10, 5);
        assert_eq!(loc.file, "main.vu");
        assert_eq!(loc.start_line, 10);
        assert_eq!(loc.start_col, 5);
        assert_eq!(loc.end_line, 10);
        assert_eq!(loc.end_col, 5);
        assert!(!loc.is_unknown());
    }

    #[test]
    fn location_range() {
        let loc = DiagnosticSourceLocation::range("main.vu", 10, 5, 9);
        assert_eq!(loc.start_line, 10);
        assert_eq!(loc.end_line, 10);
        assert_eq!(loc.start_col, 5);
        assert_eq!(loc.end_col, 9);
    }

    #[test]
    fn location_multi_line() {
        let loc = DiagnosticSourceLocation::multi_line("main.vu", 5, 10, 8, 15);
        assert_eq!(loc.start_line, 5);
        assert_eq!(loc.start_col, 10);
        assert_eq!(loc.end_line, 8);
        assert_eq!(loc.end_col, 15);
    }

    #[test]
    fn location_unknown() {
        let loc = DiagnosticSourceLocation::unknown();
        assert!(loc.is_unknown());
        assert_eq!(loc.to_string(), "<unknown>");
    }

    #[test]
    fn location_display() {
        let loc = DiagnosticSourceLocation::point("foo.vu", 5, 10);
        assert_eq!(loc.to_string(), "foo.vu:5:10");

        let loc = DiagnosticSourceLocation::range("foo.vu", 5, 10, 15);
        assert_eq!(loc.to_string(), "foo.vu:5:10-5:15");

        let loc = DiagnosticSourceLocation::multi_line("foo.vu", 5, 10, 8, 15);
        assert_eq!(loc.to_string(), "foo.vu:5:10-8:15");
    }

    #[test]
    fn location_json_roundtrip() {
        let loc = DiagnosticSourceLocation::range("test.vu", 3, 1, 8);
        let json = serde_json::to_string(&loc).unwrap();
        let back: DiagnosticSourceLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(loc, back);
    }

    // -- Suggestion tests ----------------------------------------------------

    #[test]
    fn suggestion_text_only() {
        let s = Suggestion::text("did you mean 'fn'?");
        assert_eq!(s.message, "did you mean 'fn'?");
        assert!(s.edit_range.is_none());
        assert!(s.replacement.is_none());
        assert_eq!(s.applicability, SuggestionApplicability::Unspecified);
    }

    #[test]
    fn suggestion_edit() {
        let loc = DiagnosticSourceLocation::range("main.vu", 5, 10, 13);
        let s = Suggestion::edit("Replace 'int' with 'i32'", loc.clone(), "i32");
        assert_eq!(s.message, "Replace 'int' with 'i32'");
        assert_eq!(s.edit_range, Some(loc));
        assert_eq!(s.replacement, Some("i32".to_string()));
        assert_eq!(s.applicability, SuggestionApplicability::MachineApplicable);
    }

    #[test]
    fn suggestion_machine_applicable() {
        let loc = DiagnosticSourceLocation::range("main.vu", 5, 10, 13);
        let s = Suggestion::machine_applicable("Add mask", loc, "& 4294967295");
        assert_eq!(s.applicability, SuggestionApplicability::MachineApplicable);
    }

    #[test]
    fn suggestion_with_placeholders() {
        let s = Suggestion::with_placeholders("replace with appropriate type");
        assert_eq!(s.applicability, SuggestionApplicability::HasPlaceholders);
    }

    #[test]
    fn suggestion_json_roundtrip() {
        let loc = DiagnosticSourceLocation::range("main.vu", 5, 10, 13);
        let s = Suggestion::edit("Replace 'int' with 'i32'", loc, "i32");
        let json = serde_json::to_string(&s).unwrap();
        let back: Suggestion = serde_json::from_str(&json).unwrap();
        assert_eq!(s.message, back.message);
        assert_eq!(s.replacement, back.replacement);
    }

    // -- VumaDiagnostic tests -----------------------------------------------

    #[test]
    fn diagnostic_construction() {
        let loc = DiagnosticSourceLocation::point("main.vu", 5, 10);
        let diag = VumaDiagnostic::new("E002", DiagnosticSeverity::Error, "undefined variable `x`", "parser", loc);
        assert_eq!(diag.code, "E002");
        assert_eq!(diag.severity, DiagnosticSeverity::Error);
        assert_eq!(diag.message, "undefined variable `x`");
        assert_eq!(diag.source, "parser");
        assert!(diag.related.is_empty());
        assert!(diag.suggestions.is_empty());
        assert!(diag.chain.is_empty());
        assert!(diag.legacy_suggestions.is_empty());
    }

    #[test]
    fn diagnostic_with_suggestion() {
        let loc = DiagnosticSourceLocation::point("main.vu", 5, 10);
        let diag = VumaDiagnostic::new("E002", DiagnosticSeverity::Error, "undefined variable `x`", "parser", loc)
            .with_suggestion("did you mean 'y'?");
        assert_eq!(diag.legacy_suggestions.len(), 1);
        assert_eq!(diag.legacy_suggestions[0], "did you mean 'y'?");
    }

    #[test]
    fn diagnostic_with_structured_suggestion() {
        let loc = DiagnosticSourceLocation::point("main.vu", 5, 10);
        let edit_loc = DiagnosticSourceLocation::range("main.vu", 5, 10, 13);
        let diag = VumaDiagnostic::new("E003", DiagnosticSeverity::Error, "type mismatch", "parser", loc)
            .with_structured_suggestion(Suggestion::edit(
                "Replace 'int' with 'i32'",
                edit_loc,
                "i32",
            ));
        assert_eq!(diag.suggestions.len(), 1);
        assert_eq!(diag.suggestions[0].message, "Replace 'int' with 'i32'");
        assert_eq!(diag.suggestions[0].replacement, Some("i32".to_string()));
    }

    #[test]
    fn diagnostic_with_related() {
        let loc = DiagnosticSourceLocation::point("main.vu", 5, 10);
        let related_loc = DiagnosticSourceLocation::point("main.vu", 2, 1);
        let diag = VumaDiagnostic::new("E004", DiagnosticSeverity::Error, "duplicate definition", "parser", loc)
            .with_related(RelatedInfo::new(related_loc, "previous definition here"));
        assert_eq!(diag.related.len(), 1);
        assert_eq!(diag.related[0].message, "previous definition here");
    }

    #[test]
    fn diagnostic_display() {
        let loc = DiagnosticSourceLocation::point("main.vu", 5, 10);
        let diag = VumaDiagnostic::new("E002", DiagnosticSeverity::Error, "undefined variable `x`", "parser", loc)
            .with_suggestion("did you mean 'y'?");
        let display = diag.to_string();
        assert!(display.contains("error[E002]"));
        assert!(display.contains("undefined variable `x`"));
        assert!(display.contains("did you mean 'y'?"));
    }

    #[test]
    fn diagnostic_json_output() {
        let loc = DiagnosticSourceLocation::range("main.vu", 10, 4, 7);
        let diag = VumaDiagnostic::new("E002", DiagnosticSeverity::Error, "undefined variable `foo`", "parser", loc)
            .with_suggestion("did you mean `for`?");

        let json = diag.to_json();
        assert!(json.contains("\"code\":\"E002\""));
        assert!(json.contains("\"severity\":\"error\""));
        assert!(json.contains("\"message\":\"undefined variable `foo`\""));
        assert!(json.contains("\"source\":\"parser\""));
        assert!(json.contains("\"start_line\":10"));
        assert!(json.contains("\"start_col\":4"));
        assert!(json.contains("\"end_line\":10"));
        assert!(json.contains("\"end_col\":7"));
        assert!(json.contains("did you mean"));

        // Verify round-trip.
        let back: VumaDiagnostic = serde_json::from_str(&json).unwrap();
        assert_eq!(back.code, "E002");
        assert_eq!(back.severity, DiagnosticSeverity::Error);
        assert_eq!(back.message, "undefined variable `foo`");
        assert_eq!(back.source, "parser");
        assert_eq!(back.legacy_suggestions.len(), 1);
    }

    #[test]
    fn diagnostics_array_json() {
        let loc1 = DiagnosticSourceLocation::point("a.vu", 1, 1);
        let loc2 = DiagnosticSourceLocation::point("b.vu", 2, 3);
        let diags = vec![
            VumaDiagnostic::new("E001", DiagnosticSeverity::Error, "syntax error", "parser", loc1),
            VumaDiagnostic::new("W001", DiagnosticSeverity::Warning, "unused variable", "parser", loc2),
        ];
        let json = diagnostics_to_json(&diags);
        assert!(json.starts_with('['));
        assert!(json.ends_with(']'));
        assert!(json.contains("\"E001\""));
        assert!(json.contains("\"W001\""));
    }

    // -- Error chaining tests ------------------------------------------------

    #[test]
    fn diagnostic_chain() {
        let root = VumaDiagnostic::new(
            "E023",
            DiagnosticSeverity::Error,
            "Unknown type 'int' — did you mean 'i32'?",
            "parser",
            DiagnosticSourceLocation::unknown(),
        );
        let diag = VumaDiagnostic::new(
            "E003",
            DiagnosticSeverity::Error,
            "Type mismatch",
            "parser",
            DiagnosticSourceLocation::unknown(),
        ).chain(root);

        assert!(diag.has_chain());
        assert_eq!(diag.chain.len(), 1);
        assert_eq!(diag.immediate_cause().unwrap().code, "E023");
        assert_eq!(diag.root_cause().unwrap().code, "E023");
    }

    #[test]
    fn diagnostic_chain_multi_level() {
        let root_cause = VumaDiagnostic::new(
            "E024",
            DiagnosticSeverity::Error,
            "Name resolution error for 'int'",
            "name-resolution",
            DiagnosticSourceLocation::unknown(),
        );
        let intermediate = VumaDiagnostic::new(
            "E023",
            DiagnosticSeverity::Error,
            "Unknown type 'int' — did you mean 'i32'?",
            "parser",
            DiagnosticSourceLocation::unknown(),
        ).chain(root_cause);
        let top = VumaDiagnostic::new(
            "E003",
            DiagnosticSeverity::Error,
            "Type mismatch",
            "parser",
            DiagnosticSourceLocation::unknown(),
        ).chain(intermediate);

        assert!(top.has_chain());
        assert_eq!(top.immediate_cause().unwrap().code, "E023");
        // The root cause is the intermediate's chain's root
        assert_eq!(top.root_cause().unwrap().code, "E023");
        // But the intermediate has its own chain
        assert_eq!(
            top.immediate_cause().unwrap().root_cause().unwrap().code,
            "E024"
        );
    }

    #[test]
    fn diagnostic_no_chain() {
        let diag = VumaDiagnostic::new(
            "E001",
            DiagnosticSeverity::Error,
            "Syntax error",
            "parser",
            DiagnosticSourceLocation::unknown(),
        );
        assert!(!diag.has_chain());
        assert!(diag.immediate_cause().is_none());
        assert!(diag.root_cause().is_none());
        assert!(diag.causal_chain().is_empty());
    }

    #[test]
    fn diagnostic_chain_display() {
        let cause = VumaDiagnostic::new(
            "E023",
            DiagnosticSeverity::Error,
            "Unknown type 'int'",
            "parser",
            DiagnosticSourceLocation::unknown(),
        );
        let diag = VumaDiagnostic::new(
            "E003",
            DiagnosticSeverity::Error,
            "Type mismatch",
            "parser",
            DiagnosticSourceLocation::unknown(),
        ).chain(cause);

        let display = diag.to_string();
        assert!(display.contains("caused by: error[E023]: Unknown type 'int'"));
    }

    #[test]
    fn diagnostic_chain_json() {
        let cause = VumaDiagnostic::new(
            "E023",
            DiagnosticSeverity::Error,
            "Unknown type 'int'",
            "parser",
            DiagnosticSourceLocation::unknown(),
        );
        let diag = VumaDiagnostic::new(
            "E003",
            DiagnosticSeverity::Error,
            "Type mismatch",
            "parser",
            DiagnosticSourceLocation::unknown(),
        ).chain(cause);

        let json = diag.to_json();
        assert!(json.contains("\"chain\""));
        assert!(json.contains("\"E023\""));

        // Verify round-trip
        let back: VumaDiagnostic = serde_json::from_str(&json).unwrap();
        assert!(back.has_chain());
        assert_eq!(back.chain[0].code, "E023");
    }

    // -- Output format tests -------------------------------------------------

    #[test]
    fn to_plain_text() {
        let loc = DiagnosticSourceLocation::range("main.vu", 5, 10, 15);
        let diag = VumaDiagnostic::new("E003", DiagnosticSeverity::Error, "Type mismatch", "parser", loc)
            .with_structured_suggestion(Suggestion::edit(
                "Replace 'int' with 'i32'",
                DiagnosticSourceLocation::range("main.vu", 5, 10, 13),
                "i32",
            ))
            .chain(VumaDiagnostic::new(
                "E023",
                DiagnosticSeverity::Error,
                "Unknown type 'int'",
                "parser",
                DiagnosticSourceLocation::unknown(),
            ));

        let plain = diag.to_plain_text();
        assert!(plain.contains("error[E003]: Type mismatch"));
        assert!(plain.contains("parser at main.vu:5:10-5:15"));
        assert!(plain.contains("help: Replace 'int' with 'i32'"));
        assert!(plain.contains("→ `i32`"));
        assert!(plain.contains("caused by: error[E023]: Unknown type 'int'"));
    }

    #[test]
    fn to_rich_text_contains_ansi() {
        let loc = DiagnosticSourceLocation::range("main.vu", 5, 10, 15);
        let diag = VumaDiagnostic::new("E003", DiagnosticSeverity::Error, "Type mismatch", "parser", loc)
            .with_structured_suggestion(Suggestion::edit(
                "Replace 'int' with 'i32'",
                DiagnosticSourceLocation::range("main.vu", 5, 10, 13),
                "i32",
            ));

        let rich = diag.to_rich_text();
        assert!(rich.contains("\x1b[")); // contains ANSI escape codes
        assert!(rich.contains("error"));
        assert!(rich.contains("E003"));
        assert!(rich.contains("Type mismatch"));
        assert!(rich.contains("main.vu"));
        assert!(rich.contains("help"));
    }

    #[test]
    fn to_rich_text_warning_color() {
        let diag = VumaDiagnostic::new(
            "W001",
            DiagnosticSeverity::Warning,
            "unused variable",
            "parser",
            DiagnosticSourceLocation::unknown(),
        );
        let rich = diag.to_rich_text();
        assert!(rich.contains("\x1b[33m")); // yellow for warning
    }

    #[test]
    fn to_rich_text_chain() {
        let diag = VumaDiagnostic::new(
            "E003",
            DiagnosticSeverity::Error,
            "Type mismatch",
            "parser",
            DiagnosticSourceLocation::unknown(),
        ).chain(VumaDiagnostic::new(
            "E023",
            DiagnosticSeverity::Error,
            "Unknown type 'int'",
            "parser",
            DiagnosticSourceLocation::unknown(),
        ));

        let rich = diag.to_rich_text();
        assert!(rich.contains("caused by:"));
        assert!(rich.contains("E023"));
    }

    #[test]
    fn to_lsp_format() {
        let loc = DiagnosticSourceLocation::range("main.vu", 5, 10, 15);
        let related_loc = DiagnosticSourceLocation::point("main.vu", 2, 1);
        let diag = VumaDiagnostic::new("E003", DiagnosticSeverity::Error, "Type mismatch", "parser", loc)
            .with_related(RelatedInfo::new(related_loc, "previous definition here"));

        let lsp = diag.to_lsp();
        assert_eq!(lsp["severity"], 1); // Error
        assert_eq!(lsp["code"], "E003");
        assert_eq!(lsp["source"], "parser");
        assert_eq!(lsp["message"], "Type mismatch");

        // LSP uses 0-based positions
        let range = &lsp["range"];
        assert_eq!(range["start"]["line"], 4); // 5 - 1
        assert_eq!(range["start"]["character"], 9); // 10 - 1
        assert_eq!(range["end"]["line"], 4);
        assert_eq!(range["end"]["character"], 14); // 15 - 1

        // Related information
        assert!(lsp["relatedInformation"].is_array());
        let related = lsp["relatedInformation"].as_array().unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0]["message"], "previous definition here");
    }

    #[test]
    fn to_lsp_warning_tags() {
        let diag = unused_variable("x", DiagnosticSourceLocation::point("main.vu", 5, 10));
        let lsp = diag.to_lsp();
        assert_eq!(lsp["severity"], 2); // Warning
        assert!(lsp["tags"].is_array());
        let tags = lsp["tags"].as_array().unwrap();
        assert!(tags.contains(&serde_json::json!(1))); // Unnecessary
    }

    #[test]
    fn to_lsp_deprecated_tag() {
        let diag = deprecated_feature("old_fn", Some("new_fn"), DiagnosticSourceLocation::point("main.vu", 5, 10));
        let lsp = diag.to_lsp();
        assert_eq!(lsp["severity"], 2); // Warning
        let tags = lsp["tags"].as_array().unwrap();
        assert!(tags.contains(&serde_json::json!(2))); // Deprecated
    }

    #[test]
    fn to_lsp_code_description() {
        let diag = syntax_error("bad syntax", DiagnosticSourceLocation::point("main.vu", 5, 10));
        let lsp = diag.to_lsp();
        assert!(lsp["codeDescription"]["href"].is_string());
        let href = lsp["codeDescription"]["href"].as_str().unwrap();
        assert!(href.contains("E001"));
    }

    // -- DiagnosticSummary tests ---------------------------------------------

    #[test]
    fn summary_empty() {
        let summary = DiagnosticSummary::new();
        assert_eq!(summary.total, 0);
        assert_eq!(summary.errors, 0);
        assert_eq!(summary.warnings, 0);
        assert!(!summary.has_errors());
        assert!(!summary.has_warnings());
    }

    #[test]
    fn summary_from_diagnostics() {
        let loc = DiagnosticSourceLocation::point("main.vu", 1, 1);
        let diags = vec![
            VumaDiagnostic::new("E001", DiagnosticSeverity::Error, "err1", "parser", loc.clone()),
            VumaDiagnostic::new("E003", DiagnosticSeverity::Error, "err2", "parser", loc.clone()),
            VumaDiagnostic::new("W001", DiagnosticSeverity::Warning, "warn1", "parser", loc.clone()),
            VumaDiagnostic::new("I001", DiagnosticSeverity::Info, "info1", "pipeline", loc.clone()),
            VumaDiagnostic::new("W003", DiagnosticSeverity::Hint, "hint1", "codegen", loc),
        ];
        let summary = DiagnosticSummary::from_diagnostics(&diags);
        assert_eq!(summary.total, 5);
        assert_eq!(summary.errors, 2);
        assert_eq!(summary.warnings, 1);
        assert_eq!(summary.infos, 1);
        assert_eq!(summary.hints, 1);
        assert!(summary.has_errors());
        assert!(summary.has_warnings());
        assert_eq!(summary.count_for_code("E001"), 1);
        assert_eq!(summary.count_for_code("E003"), 1);
        assert_eq!(summary.count_for_code("W001"), 1);
        assert_eq!(summary.count_for_code("E999"), 0);
        assert_eq!(summary.count_for_source("parser"), 3);
        assert_eq!(summary.count_for_source("pipeline"), 1);
        assert_eq!(summary.count_for_source("codegen"), 1);
    }

    #[test]
    fn summary_by_subcategory() {
        let loc = DiagnosticSourceLocation::point("main.vu", 1, 1);
        let diags = vec![
            VumaDiagnostic::new("E003", DiagnosticSeverity::Error, "err", "parser", loc.clone()),
            VumaDiagnostic::new("E032", DiagnosticSeverity::Error, "err", "codegen", loc.clone()),
            VumaDiagnostic::new("E042", DiagnosticSeverity::Error, "err", "ive", loc.clone()),
            VumaDiagnostic::new("W001", DiagnosticSeverity::Warning, "warn", "parser", loc.clone()),
            VumaDiagnostic::new("I001", DiagnosticSeverity::Info, "info", "pipeline", loc),
        ];
        let summary = DiagnosticSummary::from_diagnostics(&diags);
        assert_eq!(summary.by_subcategory.get("compilation"), Some(&1));
        assert_eq!(summary.by_subcategory.get("codegen"), Some(&1));
        assert_eq!(summary.by_subcategory.get("verification"), Some(&1));
        assert_eq!(summary.by_subcategory.get("warning"), Some(&1));
        assert_eq!(summary.by_subcategory.get("informational"), Some(&1));
    }

    #[test]
    fn summary_display() {
        let loc = DiagnosticSourceLocation::point("main.vu", 1, 1);
        let diags = vec![
            VumaDiagnostic::new("E001", DiagnosticSeverity::Error, "err1", "parser", loc.clone()),
            VumaDiagnostic::new("E003", DiagnosticSeverity::Error, "err2", "parser", loc),
        ];
        let summary = DiagnosticSummary::from_diagnostics(&diags);
        let display = summary.to_string();
        assert!(display.contains("2 total"));
        assert!(display.contains("2 errors"));
        assert!(display.contains("by code:"));
        assert!(display.contains("E001"));
    }

    #[test]
    fn summary_json() {
        let loc = DiagnosticSourceLocation::point("main.vu", 1, 1);
        let diags = vec![
            VumaDiagnostic::new("E001", DiagnosticSeverity::Error, "err", "parser", loc),
        ];
        let summary = DiagnosticSummary::from_diagnostics(&diags);
        let json = summary.to_json();
        assert!(json.contains("\"total\":1"));
        assert!(json.contains("\"errors\":1"));

        // Verify round-trip
        let back: DiagnosticSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total, 1);
        assert_eq!(back.errors, 1);
    }

    // -- Diagnostic code tests -----------------------------------------------

    #[test]
    fn codes_for_parse_error_kinds() {
        assert_eq!(code_for_parse_error_kind(&ParseErrorKind::InvalidSyntax), "E001");
        assert_eq!(code_for_parse_error_kind(&ParseErrorKind::UndefinedVariable), "E002");
        assert_eq!(code_for_parse_error_kind(&ParseErrorKind::UndefinedReference), "E002");
        assert_eq!(code_for_parse_error_kind(&ParseErrorKind::TypeMismatch), "E003");
        assert_eq!(code_for_parse_error_kind(&ParseErrorKind::DuplicateDefinition), "E004");
        assert_eq!(code_for_parse_error_kind(&ParseErrorKind::UnexpectedToken), "E009");
        assert_eq!(code_for_parse_error_kind(&ParseErrorKind::ExpectedToken), "E010");
        assert_eq!(code_for_parse_error_kind(&ParseErrorKind::MissingSemicolon), "E014");
    }

    #[test]
    fn code_descriptions_compilation() {
        // E001–E030: Compilation errors
        assert_eq!(code_description("E001"), "Syntax error");
        assert_eq!(code_description("E002"), "Undefined variable");
        assert_eq!(code_description("E003"), "Type mismatch");
        assert_eq!(code_description("E004"), "Duplicate definition");
        assert_eq!(code_description("E005"), "Invalid argument count");
        assert_eq!(code_description("E006"), "Invalid type");
        assert_eq!(code_description("E007"), "Missing return");
        assert_eq!(code_description("E008"), "Unreachable code");
        assert_eq!(code_description("E009"), "Unexpected token");
        assert_eq!(code_description("E010"), "Expected token");
        assert_eq!(code_description("E011"), "Region error");
        assert_eq!(code_description("E012"), "BD annotation error");
        assert_eq!(code_description("E013"), "Invalid compound assignment operator");
        assert_eq!(code_description("E014"), "Missing semicolon");
        assert_eq!(code_description("E015"), "Invalid address literal");
        assert_eq!(code_description("E024"), "Name resolution error");
        assert_eq!(code_description("E025"), "Circular dependency");
        assert_eq!(code_description("E026"), "Invalid assignment target");
        assert_eq!(code_description("E027"), "Break/continue outside loop");
        assert_eq!(code_description("E028"), "Invalid cast");
        assert_eq!(code_description("E029"), "Missing function body");
        assert_eq!(code_description("E030"), "Invalid visibility modifier");
    }

    #[test]
    fn code_descriptions_codegen() {
        // E031–E040: Codegen errors
        assert_eq!(code_description("E031"), "Invalid instruction");
        assert_eq!(code_description("E032"), "Register allocation failed");
        assert_eq!(code_description("E033"), "Encoding error");
        assert_eq!(code_description("E034"), "IR translation error");
        assert_eq!(code_description("E035"), "ELF emission error");
        assert_eq!(code_description("E036"), "Wasm section not found");
        assert_eq!(code_description("E037"), "Relocation error");
        assert_eq!(code_description("E038"), "Stack layout error");
        assert_eq!(code_description("E039"), "Linker error");
        assert_eq!(code_description("E040"), "Target unsupported feature");
    }

    #[test]
    fn code_descriptions_verification() {
        // E041–E050: Verification errors
        assert_eq!(code_description("E041"), "Invariant violation");
        assert_eq!(code_description("E042"), "Proof failure");
        assert_eq!(code_description("E043"), "Liveness invariant violated");
        assert_eq!(code_description("E044"), "Origin invariant violated");
        assert_eq!(code_description("E045"), "Exclusivity invariant violated");
        assert_eq!(code_description("E046"), "Interpretation invariant violated");
        assert_eq!(code_description("E047"), "Cleanup invariant violated");
        assert_eq!(code_description("E048"), "BD inference error");
        assert_eq!(code_description("E049"), "Constraint unsatisfiable");
        assert_eq!(code_description("E050"), "Verification timeout");
    }

    #[test]
    fn code_descriptions_warnings() {
        // W001–W010
        assert_eq!(code_description("W001"), "Unused variable");
        assert_eq!(code_description("W002"), "Implicit type conversion");
        assert_eq!(code_description("W003"), "Large constant (performance hint)");
        assert_eq!(code_description("W004"), "Dead code");
        assert_eq!(code_description("W005"), "Redundant cast");
        assert_eq!(code_description("W006"), "Shadowed variable");
        assert_eq!(code_description("W007"), "Unnecessary mut keyword");
        assert_eq!(code_description("W008"), "Deprecated feature");
        assert_eq!(code_description("W009"), "Unused import");
        assert_eq!(code_description("W010"), "Reachable panic");
    }

    #[test]
    fn code_descriptions_informational() {
        // I001–I005
        assert_eq!(code_description("I001"), "Compilation started");
        assert_eq!(code_description("I002"), "Compilation stage completed");
        assert_eq!(code_description("I003"), "Optimization applied");
        assert_eq!(code_description("I004"), "Verification passed");
        assert_eq!(code_description("I005"), "Build artifact produced");
    }

    #[test]
    fn test_code_category() {
        assert_eq!(super::code_category("E001"), "error");
        assert_eq!(super::code_category("W001"), "warning");
        assert_eq!(super::code_category("I001"), "info");
        assert_eq!(super::code_category("X001"), "unknown");
    }

    #[test]
    fn test_code_subcategory() {
        assert_eq!(super::code_subcategory("E003"), "compilation");
        assert_eq!(super::code_subcategory("E030"), "compilation");
        assert_eq!(super::code_subcategory("E031"), "codegen");
        assert_eq!(super::code_subcategory("E040"), "codegen");
        assert_eq!(super::code_subcategory("E041"), "verification");
        assert_eq!(super::code_subcategory("E050"), "verification");
        assert_eq!(super::code_subcategory("W001"), "warning");
        assert_eq!(super::code_subcategory("I003"), "informational");
    }

    #[test]
    fn total_diagnostic_codes() {
        // Verify we have 50+ codes with descriptions
        let all_codes: Vec<String> = (1..=30)
            .map(|n| format!("E{:03}", n))
            .chain((31..=40).map(|n| format!("E{:03}", n)))
            .chain((41..=50).map(|n| format!("E{:03}", n)))
            .chain((1..=10).map(|n| format!("W{:03}", n)))
            .chain((1..=5).map(|n| format!("I{:03}", n)))
            .collect();

        let described = all_codes.iter()
            .filter(|code| code_description(code) != "Unknown diagnostic code")
            .count();

        assert!(described >= 50, "Expected at least 50 diagnostic codes, got {}", described);
    }

    // -- Convenience constructors --------------------------------------------

    #[test]
    fn convenience_constructors_compilation() {
        let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);

        let d = syntax_error("bad syntax", loc.clone());
        assert_eq!(d.code, "E001");

        let d = undefined_variable("x", loc.clone());
        assert_eq!(d.code, "E002");
        assert!(d.message.contains("x"));

        let d = type_mismatch("expected u32", loc.clone());
        assert_eq!(d.code, "E003");

        let d = duplicate_definition("foo", loc.clone());
        assert_eq!(d.code, "E004");

        let d = invalid_arg_count(2, 3, loc.clone());
        assert_eq!(d.code, "E005");
        assert!(d.message.contains("2"));
        assert!(d.message.contains("3"));

        let d = invalid_type("void not allowed", loc.clone());
        assert_eq!(d.code, "E006");

        let d = missing_return(loc.clone());
        assert_eq!(d.code, "E007");

        let d = unreachable_code(loc.clone());
        assert_eq!(d.code, "E008");
        assert_eq!(d.severity, DiagnosticSeverity::Warning);

        let d = name_resolution_error("unknown name", loc.clone());
        assert_eq!(d.code, "E024");

        let d = circular_dependency("A -> B -> A", loc.clone());
        assert_eq!(d.code, "E025");

        let d = invalid_assignment_target("literal", loc.clone());
        assert_eq!(d.code, "E026");

        let d = break_outside_loop(loc.clone());
        assert_eq!(d.code, "E027");

        let d = invalid_cast("void", "u32", loc.clone());
        assert_eq!(d.code, "E028");

        let d = missing_function_body("main", loc.clone());
        assert_eq!(d.code, "E029");

        let d = invalid_visibility("private top-level", loc.clone());
        assert_eq!(d.code, "E030");
    }

    #[test]
    fn convenience_constructors_codegen() {
        let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);

        let d = invalid_instruction("bad op", loc.clone());
        assert_eq!(d.code, "E031");

        let d = register_alloc_failed("spill", loc.clone());
        assert_eq!(d.code, "E032");
        assert!(!d.suggestions.is_empty());

        let d = encoding_error("invalid opcode", loc.clone());
        assert_eq!(d.code, "E033");

        let d = relocation_error("overflow", loc.clone());
        assert_eq!(d.code, "E037");

        let d = stack_layout_error("misaligned", loc.clone());
        assert_eq!(d.code, "E038");

        let d = linker_error("undefined symbol", loc.clone());
        assert_eq!(d.code, "E039");

        let d = unsupported_feature("SIMD", loc);
        assert_eq!(d.code, "E040");
    }

    #[test]
    fn convenience_constructors_verification() {
        let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);

        let d = invariant_violation("violation", loc.clone());
        assert_eq!(d.code, "E041");

        let d = proof_failure("proof failed", loc.clone());
        assert_eq!(d.code, "E042");

        let d = liveness_violation("use after free", loc.clone());
        assert_eq!(d.code, "E043");

        let d = origin_violation("bad origin", loc.clone());
        assert_eq!(d.code, "E044");

        let d = exclusivity_violation("shared+exclusive", loc.clone());
        assert_eq!(d.code, "E045");

        let d = interpretation_violation("bad interp", loc.clone());
        assert_eq!(d.code, "E046");

        let d = cleanup_violation("no cleanup", loc.clone());
        assert_eq!(d.code, "E047");

        let d = bd_inference_error("inference failed", loc.clone());
        assert_eq!(d.code, "E048");

        let d = constraint_unsatisfiable("unsat", loc.clone());
        assert_eq!(d.code, "E049");

        let d = verification_timeout("timed out", loc);
        assert_eq!(d.code, "E050");
        assert!(!d.suggestions.is_empty());
    }

    #[test]
    fn convenience_constructors_warnings() {
        let loc = DiagnosticSourceLocation::point("test.vu", 1, 1);

        let d = unused_variable("y", loc.clone());
        assert_eq!(d.code, "W001");
        assert_eq!(d.severity, DiagnosticSeverity::Warning);
        assert!(!d.suggestions.is_empty());

        let d = implicit_conversion("u8", "u32", loc.clone());
        assert_eq!(d.code, "W002");

        let d = large_constant("0xDEADBEEF", loc.clone());
        assert_eq!(d.code, "W003");
        assert_eq!(d.severity, DiagnosticSeverity::Hint);

        let d = dead_code("unreachable", loc.clone());
        assert_eq!(d.code, "W004");

        let d = redundant_cast("u32", "u32", loc.clone());
        assert_eq!(d.code, "W005");
        assert!(!d.suggestions.is_empty());

        let d = shadowed_variable("x", loc.clone(), DiagnosticSourceLocation::point("test.vu", 1, 1));
        assert_eq!(d.code, "W006");
        assert!(!d.related.is_empty());

        let d = unnecessary_mut("x", loc.clone());
        assert_eq!(d.code, "W007");
        assert!(!d.suggestions.is_empty());

        let d = deprecated_feature("old_fn", Some("new_fn"), loc.clone());
        assert_eq!(d.code, "W008");

        let d = unused_import("std::foo", loc.clone());
        assert_eq!(d.code, "W009");
        assert!(!d.suggestions.is_empty());

        let d = reachable_panic("panic possible", loc);
        assert_eq!(d.code, "W010");
    }

    #[test]
    fn convenience_constructors_informational() {
        let d = compilation_started("main.vu");
        assert_eq!(d.code, "I001");
        assert_eq!(d.severity, DiagnosticSeverity::Info);

        let d = stage_completed("parser");
        assert_eq!(d.code, "I002");

        let d = optimization_applied("constant_fold", DiagnosticSourceLocation::point("test.vu", 1, 1));
        assert_eq!(d.code, "I003");

        let d = verification_passed("liveness", DiagnosticSourceLocation::point("test.vu", 1, 1));
        assert_eq!(d.code, "I004");

        let d = artifact_provided("main.o", 4096);
        assert_eq!(d.code, "I005");
        assert!(d.message.contains("4096"));
    }

    // -- Integration: ParseError → VumaDiagnostic ----------------------------

    #[test]
    fn from_parse_error_integration() {
        use vuma_parser::Span;

        let source = "fn main() { x }";
        let err = ParseError::undefined_var("x", Span::new(13, 14)).with_suggestion("y");
        let diag = from_parse_error(&err, source, Some("main.vu"));

        assert_eq!(diag.code, "E002");
        assert_eq!(diag.severity, DiagnosticSeverity::Error);
        assert_eq!(diag.source, "parser");
        assert!(diag.message.contains("x"));
        assert!(!diag.location.is_unknown());
        assert!(!diag.legacy_suggestions.is_empty());
    }

    // -- Integration: VumaError → Vec<VumaDiagnostic> ------------------------

    #[test]
    fn from_vuma_error_ast_to_scg() {
        let err = VumaError::AstToScg {
            message: "cannot convert node".to_string(),
        };
        let diags = from_vuma_error(&err);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].source, "scg");
        assert_eq!(diags[0].code, "E024");
    }

    #[test]
    fn from_vuma_error_codegen() {
        let err = VumaError::Codegen {
            error: CodegenError::UnknownVariable {
                name: "foo".to_string(),
            },
        };
        let diags = from_vuma_error(&err);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "E002");
        assert_eq!(diags[0].source, "codegen");
        assert!(!diags[0].suggestions.is_empty());
    }

    #[test]
    fn from_vuma_error_multi() {
        let err = VumaError::Multi {
            errors: vec![
                VumaError::AstToScg {
                    message: "err1".to_string(),
                },
                VumaError::Emission {
                    message: "err2".to_string(),
                },
            ],
        };
        let diags = from_vuma_error(&err);
        assert_eq!(diags.len(), 2);
    }

    #[test]
    fn from_vuma_error_verification() {
        // Verification errors should map to E042
        let err = VumaError::BdInference {
            node_id: Some(42),
            message: "inference failed".to_string(),
        };
        let diags = from_vuma_error(&err);
        assert_eq!(diags[0].code, "E048");
    }

    // -- Integration: CodegenError → VumaDiagnostic --------------------------

    #[test]
    fn from_codegen_error_integration() {
        let err = CodegenError::RegisterAllocFailed("spill".to_string());
        let diag = from_codegen_error(&err);
        assert_eq!(diag.code, "E032");
        assert_eq!(diag.source, "register-alloc");

        let err = CodegenError::UnknownVariable {
            name: "bar".to_string(),
        };
        let diag = from_codegen_error(&err);
        assert_eq!(diag.code, "E002");
        assert!(diag.suggestions.iter().any(|s| s.message.contains("declare")));
    }

    // -- has_machine_applicable_fixes test ------------------------------------

    #[test]
    fn has_machine_applicable_fixes() {
        let loc = DiagnosticSourceLocation::point("main.vu", 5, 10);
        let edit_loc = DiagnosticSourceLocation::range("main.vu", 5, 10, 13);

        let diag = VumaDiagnostic::new("E003", DiagnosticSeverity::Error, "type mismatch", "parser", loc.clone())
            .with_structured_suggestion(Suggestion::edit("fix it", edit_loc, "i32"));
        assert!(diag.has_machine_applicable_fixes());

        let diag2 = VumaDiagnostic::new("E001", DiagnosticSeverity::Error, "syntax error", "parser", loc)
            .with_structured_suggestion(Suggestion::text("try something else"));
        assert!(!diag2.has_machine_applicable_fixes());
    }

    // -- Chaining with structured suggestions --------------------------------

    #[test]
    fn chain_with_suggestions() {
        let loc = DiagnosticSourceLocation::range("main.vu", 5, 10, 13);
        let cause = VumaDiagnostic::new(
            "E023",
            DiagnosticSeverity::Error,
            "Unknown type 'int' — did you mean 'i32'?",
            "parser",
            loc.clone(),
        ).with_structured_suggestion(Suggestion::machine_applicable(
            "Replace 'int' with 'i32'",
            loc,
            "i32",
        ));

        let diag = VumaDiagnostic::new(
            "E003",
            DiagnosticSeverity::Error,
            "Type mismatch: expected i32, found int",
            "parser",
            DiagnosticSourceLocation::range("main.vu", 5, 20, 25),
        ).chain(cause);

        assert!(diag.has_chain());
        let immediate = diag.immediate_cause().unwrap();
        assert_eq!(immediate.code, "E023");
        assert!(immediate.has_machine_applicable_fixes());
    }

    #[test]
    fn all_suggestions_method() {
        let loc = DiagnosticSourceLocation::point("main.vu", 5, 10);
        let diag = VumaDiagnostic::new("E003", DiagnosticSeverity::Error, "type error", "parser", loc)
            .with_structured_suggestion(Suggestion::text("fix A"))
            .with_structured_suggestion(Suggestion::text("fix B"));
        assert_eq!(diag.all_suggestions().len(), 2);
    }
}
