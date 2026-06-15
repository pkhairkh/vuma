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
//! information, and quick-fix suggestions.
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
//!     "suggestions": ["did you mean `for`?"]
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
// Diagnostic codes
// ═══════════════════════════════════════════════════════════════════════════

/// Returns the diagnostic code string for a given [`ParseErrorKind`].
///
/// Mapping:
///
/// | Code  | Kind                    | Description                |
/// |-------|-------------------------|----------------------------|
/// | E001  | InvalidSyntax           | Syntax error               |
/// | E002  | UndefinedReference / UndefinedVariable | Undefined variable |
/// | E003  | TypeMismatch            | Type mismatch              |
/// | E004  | DuplicateDefinition     | Duplicate definition       |
/// | E005  | —                       | Invalid argument count     |
/// | E006  | —                       | Invalid type               |
/// | E007  | —                       | Missing return             |
/// | E008  | —                       | Unreachable code           |
/// | W001  | —                       | Unused variable            |
/// | W002  | —                       | Implicit type conversion   |
/// | W003  | —                       | Large constant (perf hint) |
/// | E009  | UnexpectedToken         | Unexpected token           |
/// | E010  | ExpectedToken           | Expected token             |
/// | E011  | RegionError             | Region error               |
/// | E012  | BDAnnotationError       | BD annotation error        |
/// | E013  | InvalidCompoundOp       | Invalid compound operator  |
/// | E014  | MissingSemicolon        | Missing semicolon          |
/// | E015  | InvalidAddress          | Invalid address literal    |
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
        CodegenError::InvalidInstruction(_) => "E016",
        CodegenError::RegisterAllocFailed(_) => "E017",
        CodegenError::EncodingError(_) => "E018",
        CodegenError::TranslationError(_) => "E019",
        CodegenError::ElfError(_) => "E020",
        CodegenError::UnknownVariable { .. } => "E002",
        CodegenError::WasmSectionNotFound { .. } => "E016",
    }
}

/// Human-readable description of a diagnostic code.
pub fn code_description(code: &str) -> &'static str {
    match code {
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
        "E016" => "Invalid instruction",
        "E017" => "Register allocation failed",
        "E018" => "Encoding error",
        "E019" => "IR translation error",
        "E021" => "LLM code mismatch (Rust/C syntax in VUMA)",
        "E022" => "C-style for loop (use range-based for instead)",
        "E023" => "Unknown type (use VUMA sized integer types)",
        "E020" => "ELF emission error",
        "W001" => "Unused variable",
        "W002" => "Implicit type conversion",
        "W003" => "Large constant (performance hint)",
        _ => "Unknown diagnostic code",
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
    /// Quick-fix suggestions (e.g. `"did you mean 'fn'?"`,
    /// `"add semicolon"`).
    pub suggestions: Vec<String>,
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
        }
    }

    /// Add a related-info entry.
    pub fn with_related(mut self, info: RelatedInfo) -> Self {
        self.related.push(info);
        self
    }

    /// Add a quick-fix suggestion.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestions.push(suggestion.into());
        self
    }

    /// Serialize this diagnostic as a JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"code":"E000","severity":"error","message":"JSON serialization failed","source":"diagnostics","location":{"file":"","start_line":0,"start_col":0,"end_line":0,"end_col":0},"related":[],"suggestions":[]}"#.to_string()
        })
    }

    /// Serialize this diagnostic as a pretty-printed JSON string.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| self.to_json())
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
            write!(f, "\n  help: {}", suggestion)?;
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
                "E001",
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
                    "E001",
                    DiagnosticSeverity::Error,
                    msg,
                    "scg",
                    DiagnosticSourceLocation::unknown(),
                )
            })
            .collect(),
        VumaError::ScgToMsg { error } => {
            vec![VumaDiagnostic::new(
                "E001",
                DiagnosticSeverity::Error,
                error.to_string(),
                "msg",
                DiagnosticSourceLocation::unknown(),
            )]
        }
        VumaError::BdInference { node_id, message } => {
            let mut diag = VumaDiagnostic::new(
                "E001",
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
                "E001",
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
                    "E001",
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
                "E017",
                DiagnosticSeverity::Error,
                message,
                "register-alloc",
                DiagnosticSourceLocation::unknown(),
            )]
        }
        VumaError::Emission { message } => {
            vec![VumaDiagnostic::new(
                "E020",
                DiagnosticSeverity::Error,
                message,
                "elf-emission",
                DiagnosticSourceLocation::unknown(),
            )]
        }
        VumaError::CorInit { message } => {
            vec![VumaDiagnostic::new(
                "E001",
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
                    "E001",
                    DiagnosticSeverity::Error,
                    e.to_string(),
                    "module-resolution",
                    DiagnosticSourceLocation::unknown(),
                )]
            })
            .collect(),
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
            Some(format!("declare '{}' before use", name)),
        ),
        CodegenError::RegisterAllocFailed(_) => (
            "register-alloc",
            Some("reduce register pressure by splitting live ranges".to_string()),
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
            Some(format!("ensure '{}' section is generated", section)),
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
        diag = diag.with_suggestion(s);
    }

    diag
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

/// Create a W001 (unused variable) diagnostic.
pub fn unused_variable(name: &str, location: DiagnosticSourceLocation) -> VumaDiagnostic {
    VumaDiagnostic::new(
        "W001",
        DiagnosticSeverity::Warning,
        format!("unused variable `{}`", name),
        "parser",
        location,
    ).with_suggestion(format!("prefix with `_{}' to suppress", name))
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
    }

    #[test]
    fn location_json_roundtrip() {
        let loc = DiagnosticSourceLocation::range("test.vu", 3, 1, 8);
        let json = serde_json::to_string(&loc).unwrap();
        let back: DiagnosticSourceLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(loc, back);
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
    }

    #[test]
    fn diagnostic_with_suggestion() {
        let loc = DiagnosticSourceLocation::point("main.vu", 5, 10);
        let diag = VumaDiagnostic::new("E002", DiagnosticSeverity::Error, "undefined variable `x`", "parser", loc)
            .with_suggestion("did you mean 'y'?");
        assert_eq!(diag.suggestions.len(), 1);
        assert_eq!(diag.suggestions[0], "did you mean 'y'?");
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
        assert_eq!(back.suggestions.len(), 1);
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
    fn code_descriptions() {
        assert_eq!(code_description("E001"), "Syntax error");
        assert_eq!(code_description("E002"), "Undefined variable");
        assert_eq!(code_description("E003"), "Type mismatch");
        assert_eq!(code_description("E004"), "Duplicate definition");
        assert_eq!(code_description("E005"), "Invalid argument count");
        assert_eq!(code_description("E006"), "Invalid type");
        assert_eq!(code_description("E007"), "Missing return");
        assert_eq!(code_description("E008"), "Unreachable code");
        assert_eq!(code_description("W001"), "Unused variable");
        assert_eq!(code_description("W002"), "Implicit type conversion");
        assert_eq!(code_description("W003"), "Large constant (performance hint)");
    }

    // -- Convenience constructors --------------------------------------------

    #[test]
    fn convenience_constructors() {
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

        let d = unused_variable("y", loc.clone());
        assert_eq!(d.code, "W001");
        assert_eq!(d.severity, DiagnosticSeverity::Warning);
        assert!(!d.suggestions.is_empty());

        let d = implicit_conversion("u8", "u32", loc.clone());
        assert_eq!(d.code, "W002");

        let d = large_constant("0xDEADBEEF", loc);
        assert_eq!(d.code, "W003");
        assert_eq!(d.severity, DiagnosticSeverity::Hint);
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
        assert!(!diag.suggestions.is_empty());
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

    // -- Integration: CodegenError → VumaDiagnostic --------------------------

    #[test]
    fn from_codegen_error_integration() {
        let err = CodegenError::RegisterAllocFailed("spill".to_string());
        let diag = from_codegen_error(&err);
        assert_eq!(diag.code, "E017");
        assert_eq!(diag.source, "register-alloc");

        let err = CodegenError::UnknownVariable {
            name: "bar".to_string(),
        };
        let diag = from_codegen_error(&err);
        assert_eq!(diag.code, "E002");
        assert!(diag.suggestions.iter().any(|s| s.contains("declare")));
    }
}
