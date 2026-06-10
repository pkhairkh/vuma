//! Parse error types, recovery strategies, and diagnostic reporting for the
//! VUMA language frontend.
//!
//! This module provides:
//!
//! - **[`ParseErrorKind`]** — classification of every parse/semantic error.
//! - **[`ParseError`]** — a single error with message, span, kind, and
//!   optional "did you mean?" suggestion.
//! - **[`ErrorRecovery`]** — strategies the parser can use to keep going after
//!   an error.
//! - **[`ParseResult<T>`]** — carries a successfully-parsed value **and** any
//!   accumulated non-fatal errors, enabling partial-result error recovery.
//! - **[`ErrorCollector`]** — collects multiple diagnostics across a parse
//!   session and supports batch rendering.
//! - **[`Diagnostic`]** / **[`Severity`]** — structured diagnostics with
//!   severity levels (error, warning, note).
//! - **[`SourceLocation`]** — rich source location with file, line, column,
//!   and context rendering.
//! - **"Did you mean?" suggestions** — [`suggest`], [`suggest_keyword`],
//!   [`levenshtein`], [`format_suggestion`] for typo correction.

use std::fmt;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Span
// ---------------------------------------------------------------------------

/// A byte-offset span within a source file, used for error localisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

impl Span {
    /// Create a new span from start (inclusive) to end (exclusive) byte offsets.
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// A synthetic span representing "no location" (e.g. generated nodes).
    pub fn synthetic() -> Self {
        Self { start: 0, end: 0 }
    }

    /// Merge two spans to produce a span that covers both.
    pub fn merge(self, other: Span) -> Span {
        Span {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    /// Length in bytes of this span.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// True when the span has zero length.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

// ---------------------------------------------------------------------------
// SourceLocation
// ---------------------------------------------------------------------------

/// Rich source location: file path (optional), line, column, and a snippet of
/// the surrounding source text for context rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    /// Optional file path or module name.
    pub file: Option<String>,
    /// Line number (0-based).
    pub line: usize,
    /// Column number (0-based, measured in Unicode code points).
    pub column: usize,
    /// The text of the source line (if available).
    pub line_text: Option<String>,
}

impl SourceLocation {
    /// Create a minimal source location from line and column only.
    pub fn new(line: usize, column: usize) -> Self {
        Self {
            file: None,
            line,
            column,
            line_text: None,
        }
    }

    /// Attach a file path to this location.
    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// Attach the source line text for context rendering.
    pub fn with_line_text(mut self, text: impl Into<String>) -> Self {
        self.line_text = Some(text.into());
        self
    }

    /// Render the location in `file:line:col` format.
    pub fn format_location(&self) -> String {
        match &self.file {
            Some(f) => format!("{}:{}:{}", f, self.line + 1, self.column + 1),
            None => format!("{}:{}", self.line + 1, self.column + 1),
        }
    }

    /// Render the location with an underline pointer under the relevant
    /// column.  If `span_len` is provided the pointer uses that many `^`
    /// characters; otherwise a single `^` is drawn.
    pub fn render_with_pointer(&self, span_len: Option<usize>) -> String {
        let pointer = match span_len {
            Some(len) if len > 0 => {
                let padding = " ".repeat(self.column);
                let carets = "^".repeat(len.max(1));
                format!("{}{}", padding, carets)
            }
            _ => {
                let padding = " ".repeat(self.column);
                format!("{}^", padding)
            }
        };

        let line_text = self.line_text.as_deref().unwrap_or("<no source>");
        format!(
            "   |\n{: >3} | {}\n   | {}",
            self.line + 1,
            line_text,
            pointer
        )
    }
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_location())
    }
}

/// Convert a byte offset in `source` to a [`SourceLocation`].
///
/// If `file` is `Some`, it is attached to the resulting location.
/// The source line text is also extracted for context rendering.
pub fn offset_to_location(source: &str, offset: usize, file: Option<&str>) -> SourceLocation {
    let mut line = 0;
    let mut col = 0;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    let line_text = get_line(source, line);
    SourceLocation {
        file: file.map(|f| f.to_string()),
        line,
        column: col,
        line_text: Some(line_text),
    }
}

// ---------------------------------------------------------------------------
// Severity
// ---------------------------------------------------------------------------

/// Severity level for a diagnostic message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Severity {
    /// A hard error — the program cannot be compiled.
    Error,
    /// A warning — suspicious code that is technically valid.
    Warning,
    /// An informational note, typically attached to another diagnostic.
    Note,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Error => write!(f, "error"),
            Severity::Warning => write!(f, "warning"),
            Severity::Note => write!(f, "note"),
        }
    }
}

// ---------------------------------------------------------------------------
// ParseErrorKind
// ---------------------------------------------------------------------------

/// Classification of parse errors for programmatic handling.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParseErrorKind {
    // -- Core syntax errors --------------------------------------------------
    /// The lexer/parser encountered a token it did not expect at this position.
    UnexpectedToken,
    /// A specific token was expected but not found (e.g. missing `)`).
    ExpectedToken,
    /// A general syntax rule was violated (covers missing separators,
    /// malformed constructs, etc.).
    InvalidSyntax,

    // -- Name/scope errors ---------------------------------------------------
    /// A name was defined more than once in the same scope.
    DuplicateDefinition,
    /// A name was referenced but not defined in the current scope.
    UndefinedReference,

    // -- Type errors ---------------------------------------------------------
    /// A type annotation does not match the inferred or expected type.
    TypeMismatch,

    // -- VUMA-specific errors ------------------------------------------------
    /// An error related to a `region` declaration (e.g. missing `allocate`,
    /// invalid region expression).
    RegionError,
    /// An error in a BD (behavioral domain) annotation directive
    /// (`bd`, `repd`, `capd`, `reld`).
    BDAnnotationError,

    // -- Legacy aliases (kept for backward compat) ---------------------------
    /// Alias for [`InvalidSyntax`] — a required semicolon separator is missing.
    MissingSemicolon,
    /// Alias for [`InvalidSyntax`] — an address literal (0x...) is malformed.
    InvalidAddress,
    /// Alias for [`UndefinedReference`] — a variable name was used before
    /// definition.
    UndefinedVariable,
}

impl fmt::Display for ParseErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseErrorKind::UnexpectedToken => write!(f, "unexpected token"),
            ParseErrorKind::ExpectedToken => write!(f, "expected token"),
            ParseErrorKind::InvalidSyntax => write!(f, "invalid syntax"),
            ParseErrorKind::DuplicateDefinition => write!(f, "duplicate definition"),
            ParseErrorKind::UndefinedReference => write!(f, "undefined reference"),
            ParseErrorKind::TypeMismatch => write!(f, "type mismatch"),
            ParseErrorKind::RegionError => write!(f, "region error"),
            ParseErrorKind::BDAnnotationError => write!(f, "BD annotation error"),
            ParseErrorKind::MissingSemicolon => write!(f, "missing semicolon"),
            ParseErrorKind::InvalidAddress => write!(f, "invalid address"),
            ParseErrorKind::UndefinedVariable => write!(f, "undefined variable"),
        }
    }
}

// ---------------------------------------------------------------------------
// ParseError
// ---------------------------------------------------------------------------

/// A single parse error with full context for reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseError {
    /// Human-readable error description.
    pub message: String,
    /// Source byte-offset span where the error originates.
    pub span: Span,
    /// Classification of the error kind.
    pub kind: ParseErrorKind,
    /// An optional "did you mean?" suggestion (e.g. "fn" when the user typed
    /// "fun").
    pub suggestion: Option<String>,
}

impl ParseError {
    /// Construct a new parse error.
    pub fn new(message: impl Into<String>, span: Span, kind: ParseErrorKind) -> Self {
        Self {
            message: message.into(),
            span,
            kind,
            suggestion: None,
        }
    }

    /// Attach a "did you mean?" suggestion to this error.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    // -- Convenience constructors: core errors -------------------------------

    /// Convenience: unexpected-token error.
    pub fn unexpected(msg: impl Into<String>, span: Span) -> Self {
        Self::new(msg, span, ParseErrorKind::UnexpectedToken)
    }

    /// Convenience: expected-token error.
    pub fn expected(expected: impl Into<String>, found: impl Into<String>, span: Span) -> Self {
        Self::new(
            format!("expected {}, found {}", expected.into(), found.into()),
            span,
            ParseErrorKind::ExpectedToken,
        )
    }

    /// Convenience: invalid-syntax error.
    pub fn invalid_syntax(msg: impl Into<String>, span: Span) -> Self {
        Self::new(msg, span, ParseErrorKind::InvalidSyntax)
    }

    /// Convenience: missing-semicolon error.
    pub fn missing_semi(span: Span) -> Self {
        Self::new("expected semicolon", span, ParseErrorKind::MissingSemicolon)
    }

    /// Convenience: invalid-address-literal error.
    pub fn invalid_address(msg: impl Into<String>, span: Span) -> Self {
        Self::new(msg, span, ParseErrorKind::InvalidAddress)
    }

    /// Convenience: undefined-variable error (legacy alias for undefined
    /// reference).
    pub fn undefined_var(name: impl Into<String>, span: Span) -> Self {
        Self::new(
            format!("undefined variable `{}`", name.into()),
            span,
            ParseErrorKind::UndefinedVariable,
        )
    }

    /// Convenience: undefined-reference error.
    pub fn undefined_ref(name: impl Into<String>, span: Span) -> Self {
        Self::new(
            format!("undefined reference `{}`", name.into()),
            span,
            ParseErrorKind::UndefinedReference,
        )
    }

    /// Convenience: type-mismatch error.
    pub fn type_mismatch(msg: impl Into<String>, span: Span) -> Self {
        Self::new(msg, span, ParseErrorKind::TypeMismatch)
    }

    /// Convenience: duplicate-definition error.
    pub fn duplicate(name: impl Into<String>, span: Span) -> Self {
        Self::new(
            format!("duplicate definition of `{}`", name.into()),
            span,
            ParseErrorKind::DuplicateDefinition,
        )
    }

    /// Convenience: region-error.
    pub fn region_error(msg: impl Into<String>, span: Span) -> Self {
        Self::new(msg, span, ParseErrorKind::RegionError)
    }

    /// Convenience: BD-annotation-error.
    pub fn bd_annotation_error(msg: impl Into<String>, span: Span) -> Self {
        Self::new(msg, span, ParseErrorKind::BDAnnotationError)
    }

    /// Render the error with source context and a visual pointer.
    ///
    /// The `source` parameter should be the full source text. The function
    /// extracts the relevant line and draws a `^^^` pointer under the
    /// offending region.
    pub fn display_with_source(&self, source: &str) -> String {
        let loc = offset_to_location(source, self.span.start, None);
        let pointer_len = if self.span.is_empty() {
            None
        } else {
            Some(self.span.len().max(1))
        };

        let suggestion_text = match &self.suggestion {
            Some(s) => format!("\n   = help: did you mean `{}`?", s),
            None => String::new(),
        };

        format!(
            "error[{}]: {}\n  --> {}\n{}\n{}\n",
            self.kind,
            self.message,
            loc.format_location(),
            loc.render_with_pointer(pointer_len),
            suggestion_text,
        )
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}..{})", self.message, self.span.start, self.span.end)?;
        if let Some(ref s) = self.suggestion {
            write!(f, " (did you mean `{}`?)", s)?;
        }
        Ok(())
    }
}

impl std::error::Error for ParseError {}

// ---------------------------------------------------------------------------
// ErrorRecovery
// ---------------------------------------------------------------------------

/// Strategy for recovering from a parse error so that the parser can continue
/// and potentially find additional errors in the same file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorRecovery {
    /// Skip tokens until the next statement boundary (`;` or `}`).
    /// This is the default recovery strategy for most syntax errors.
    SkipToStatementBoundary,
    /// Skip tokens until the next block boundary (`}`).
    /// Useful when an error inside a block might cascade.
    SkipToBlockBoundary,
    /// Insert a missing token (e.g. a semicolon) and continue parsing as if
    /// it had been present.  The inserted token kind is stored for reporting.
    InsertMissingToken(String),
    /// Skip a single token and retry.  Useful when a stray token is found
    /// but the overall structure is still intact.
    SkipOneToken,
    /// No recovery possible — the parser should abort the current item.
    AbortItem,
}

impl ErrorRecovery {
    /// Return the default recovery strategy for a given error kind.
    pub fn for_kind(kind: &ParseErrorKind) -> Self {
        match kind {
            ParseErrorKind::MissingSemicolon => {
                ErrorRecovery::InsertMissingToken(";".to_string())
            }
            ParseErrorKind::ExpectedToken => ErrorRecovery::SkipOneToken,
            ParseErrorKind::UnexpectedToken => ErrorRecovery::SkipToStatementBoundary,
            ParseErrorKind::InvalidSyntax => ErrorRecovery::SkipToStatementBoundary,
            ParseErrorKind::DuplicateDefinition => ErrorRecovery::SkipToStatementBoundary,
            ParseErrorKind::UndefinedReference => ErrorRecovery::SkipOneToken,
            ParseErrorKind::TypeMismatch => ErrorRecovery::SkipOneToken,
            ParseErrorKind::RegionError => ErrorRecovery::SkipToStatementBoundary,
            ParseErrorKind::BDAnnotationError => ErrorRecovery::SkipToStatementBoundary,
            ParseErrorKind::InvalidAddress => ErrorRecovery::SkipOneToken,
            ParseErrorKind::UndefinedVariable => ErrorRecovery::SkipOneToken,
        }
    }
}

impl fmt::Display for ErrorRecovery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorRecovery::SkipToStatementBoundary => write!(f, "skip to next statement"),
            ErrorRecovery::SkipToBlockBoundary => write!(f, "skip to block end"),
            ErrorRecovery::InsertMissingToken(tok) => write!(f, "insert missing '{}'", tok),
            ErrorRecovery::SkipOneToken => write!(f, "skip one token"),
            ErrorRecovery::AbortItem => write!(f, "abort current item"),
        }
    }
}

// ---------------------------------------------------------------------------
// ParseResult<T>
// ---------------------------------------------------------------------------

/// The result of a parse operation that supports error recovery.
///
/// On success the parsed `value` is present **and** there may be accumulated
/// non-fatal errors in `errors`.  On failure `value` is `None` and `errors`
/// contains at least one fatal error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseResult<T> {
    /// The parsed value, if parsing succeeded (even with non-fatal errors).
    pub value: Option<T>,
    /// All errors accumulated during the parse (both fatal and non-fatal).
    pub errors: Vec<ParseError>,
}

impl<T> ParseResult<T> {
    /// Create a fully successful result with no errors.
    pub fn ok(value: T) -> Self {
        Self {
            value: Some(value),
            errors: Vec::new(),
        }
    }

    /// Create a result with a value and some accumulated (non-fatal) errors.
    pub fn ok_with_errors(value: T, errors: Vec<ParseError>) -> Self {
        Self {
            value: Some(value),
            errors,
        }
    }

    /// Create a failed result with no value.
    pub fn err(errors: Vec<ParseError>) -> Self {
        Self {
            value: None,
            errors,
        }
    }

    /// Create a failed result from a single error.
    pub fn from_error(error: ParseError) -> Self {
        Self {
            value: None,
            errors: vec![error],
        }
    }

    /// True when a value was successfully parsed.
    pub fn is_ok(&self) -> bool {
        self.value.is_some()
    }

    /// True when parsing failed (no value).
    pub fn is_err(&self) -> bool {
        self.value.is_none()
    }

    /// True when at least one error was accumulated.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Add another error to the accumulated list.
    pub fn push_error(&mut self, error: ParseError) {
        self.errors.push(error);
    }

    /// Merge errors from another `ParseResult` into this one.
    pub fn merge_errors(&mut self, other: &ParseResult<T>) {
        self.errors.extend(other.errors.iter().cloned());
    }

    /// Merge errors from another `ParseResult<U>` (different value type)
    /// into this one.  Only errors are transferred.
    pub fn merge_errors_from<U>(&mut self, other: &ParseResult<U>) {
        self.errors.extend(other.errors.iter().cloned());
    }

    /// Convert to a standard `Result`, discarding any non-fatal errors.
    pub fn into_result(self) -> Result<T, Vec<ParseError>> {
        match self.value {
            Some(v) => Ok(v),
            None => Err(self.errors),
        }
    }

    /// Map the value if present, preserving accumulated errors.
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> ParseResult<U> {
        ParseResult {
            value: self.value.map(f),
            errors: self.errors,
        }
    }

    /// Unwrap the value, panicking if it is `None`.
    pub fn unwrap(self) -> T {
        self.value.expect("ParseResult::unwrap on err")
    }

    /// Unwrap the value, panicking with the given message if it is `None`.
    pub fn expect(self, msg: &str) -> T {
        self.value.expect(msg)
    }
}

// ---------------------------------------------------------------------------
// Diagnostic
// ---------------------------------------------------------------------------

/// A structured diagnostic message with severity, source location, and an
/// optional error code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Severity level.
    pub severity: Severity,
    /// Error code, e.g. `"E0001"`.
    pub code: Option<String>,
    /// Human-readable message.
    pub message: String,
    /// Source location where the diagnostic originates.
    pub location: SourceLocation,
    /// Optional "did you mean?" suggestion.
    pub suggestion: Option<String>,
    /// Child diagnostics (notes, related warnings).
    pub children: Vec<Diagnostic>,
}

impl Diagnostic {
    /// Create a new error-level diagnostic.
    pub fn error(message: impl Into<String>, location: SourceLocation) -> Self {
        Self {
            severity: Severity::Error,
            code: None,
            message: message.into(),
            location,
            suggestion: None,
            children: Vec::new(),
        }
    }

    /// Create a new warning-level diagnostic.
    pub fn warning(message: impl Into<String>, location: SourceLocation) -> Self {
        Self {
            severity: Severity::Warning,
            code: None,
            message: message.into(),
            location,
            suggestion: None,
            children: Vec::new(),
        }
    }

    /// Create a new note-level diagnostic.
    pub fn note(message: impl Into<String>, location: SourceLocation) -> Self {
        Self {
            severity: Severity::Note,
            code: None,
            message: message.into(),
            location,
            suggestion: None,
            children: Vec::new(),
        }
    }

    /// Attach an error code.
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Attach a "did you mean?" suggestion.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Add a child diagnostic (e.g. a note attached to an error).
    pub fn with_child(mut self, child: Diagnostic) -> Self {
        self.children.push(child);
        self
    }

    /// Create a [`Diagnostic`] from a [`ParseError`] by extracting source
    /// location from the given source text.
    pub fn from_parse_error(err: &ParseError, source: &str, file: Option<&str>) -> Self {
        let loc = offset_to_location(source, err.span.start, file);
        let mut diag = Self {
            severity: Severity::Error,
            code: None,
            message: err.message.clone(),
            location: loc,
            suggestion: err.suggestion.clone(),
            children: Vec::new(),
        };
        // Add a child note with the error kind.
        diag.children.push(Diagnostic::note(
            format!("error kind: {}", err.kind),
            diag.location.clone(),
        ));
        diag
    }

    /// Render the diagnostic with full source context.
    pub fn display_with_source(&self) -> String {
        let code_str = match &self.code {
            Some(c) => format!("[{}]", c),
            None => String::new(),
        };
        let suggestion_text = match &self.suggestion {
            Some(s) => format!("\n   = help: did you mean `{}`?", s),
            None => String::new(),
        };
        let pointer_len = None; // we don't have span length here
        let mut result = format!(
            "{}{}: {}\n  --> {}\n{}\n{}",
            self.severity,
            code_str,
            self.message,
            self.location.format_location(),
            self.location.render_with_pointer(pointer_len),
            suggestion_text,
        );
        for child in &self.children {
            result.push_str(&format!(
                "\n{}: {}",
                child.severity, child.message
            ));
        }
        result.push('\n');
        result
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code_str = match &self.code {
            Some(c) => format!("[{}]", c),
            None => String::new(),
        };
        write!(
            f,
            "{}{}: {} at {}",
            self.severity,
            code_str,
            self.message,
            self.location
        )
    }
}

impl std::error::Error for Diagnostic {}

// ---------------------------------------------------------------------------
// ErrorCollector
// ---------------------------------------------------------------------------

/// Collects multiple [`Diagnostic`] values across a parse session.
///
/// Supports deduplication by approximate location (same line and message),
/// and batch rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorCollector {
    /// Collected diagnostics.
    diagnostics: Vec<Diagnostic>,
    /// Number of errors (for gating compilation).
    error_count: usize,
    /// Number of warnings.
    warning_count: usize,
}

impl ErrorCollector {
    /// Create an empty error collector.
    pub fn new() -> Self {
        Self {
            diagnostics: Vec::new(),
            error_count: 0,
            warning_count: 0,
        }
    }

    /// Add a diagnostic.
    pub fn add(&mut self, diag: Diagnostic) {
        match diag.severity {
            Severity::Error => self.error_count += 1,
            Severity::Warning => self.warning_count += 1,
            Severity::Note => {}
        }
        self.diagnostics.push(diag);
    }

    /// Add a parse error, converting it to a diagnostic.
    pub fn add_parse_error(&mut self, err: &ParseError, source: &str, file: Option<&str>) {
        self.add(Diagnostic::from_parse_error(err, source, file));
    }

    /// Add a diagnostic only if there isn't already one at the same line with
    /// the same message (deduplication).
    pub fn add_dedup(&mut self, diag: Diagnostic) {
        let is_dup = self.diagnostics.iter().any(|existing| {
            existing.location.line == diag.location.line
                && existing.message == diag.message
        });
        if !is_dup {
            self.add(diag);
        }
    }

    /// Number of error-level diagnostics.
    pub fn error_count(&self) -> usize {
        self.error_count
    }

    /// Number of warning-level diagnostics.
    pub fn warning_count(&self) -> usize {
        self.warning_count
    }

    /// Total number of diagnostics.
    pub fn len(&self) -> usize {
        self.diagnostics.len()
    }

    /// True when no diagnostics have been collected.
    pub fn is_empty(&self) -> bool {
        self.diagnostics.is_empty()
    }

    /// True when at least one error-level diagnostic was collected.
    pub fn has_errors(&self) -> bool {
        self.error_count > 0
    }

    /// Access all collected diagnostics.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Iterate over only error-level diagnostics.
    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.severity == Severity::Error)
    }

    /// Iterate over only warning-level diagnostics.
    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.severity == Severity::Warning)
    }

    /// Take all diagnostics, leaving the collector empty.
    pub fn take(&mut self) -> Vec<Diagnostic> {
        self.error_count = 0;
        self.warning_count = 0;
        std::mem::take(&mut self.diagnostics)
    }

    /// Merge diagnostics from another collector into this one.
    pub fn merge(&mut self, other: &ErrorCollector) {
        for diag in &other.diagnostics {
            self.add(diag.clone());
        }
    }

    /// Render all diagnostics as a single string with source context.
    pub fn render_all(&self) -> String {
        self.diagnostics
            .iter()
            .map(|d| d.display_with_source())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Render a summary line, e.g. "2 errors, 1 warning".
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.error_count > 0 {
            parts.push(format!("{} error{}", self.error_count, if self.error_count > 1 { "s" } else { "" }));
        }
        if self.warning_count > 0 {
            parts.push(format!("{} warning{}", self.warning_count, if self.warning_count > 1 { "s" } else { "" }));
        }
        if parts.is_empty() {
            "no diagnostics".to_string()
        } else {
            parts.join(", ")
        }
    }
}

impl Default for ErrorCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// "Did you mean?" suggestions
// ---------------------------------------------------------------------------

/// Compute the Levenshtein edit distance between two strings.
///
/// This is a classic dynamic-programming implementation with O(a×b) time
/// and O(min(a,b)) space.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a_len = a.chars().count();
    let b_len = b.chars().count();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    // Use the shorter string for the column dimension to save space.
    let (short, long) = if a_len <= b_len { (a, b) } else { (b, a) };
    let short_len = short.chars().count();

    let mut prev: Vec<usize> = (0..=short_len).collect();
    let mut curr = vec![0; short_len + 1];

    for lc in long.chars() {
        curr[0] = prev[0] + 1;
        for (i, sc) in (1..).zip(short.chars()) {
            let cost = if sc == lc { 0 } else { 1 };
            curr[i] = (prev[i] + 1)        // deletion
                .min(curr[i - 1] + 1)      // insertion
                .min(prev[i - 1] + cost);  // substitution
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[short_len]
}

/// Suggest the closest match from a list of candidates using Levenshtein
/// distance.  Returns `None` if no candidate is within `max_distance`.
pub fn suggest<'a>(input: &str, candidates: &'a [String], max_distance: usize) -> Option<&'a String> {
    candidates
        .iter()
        .filter(|c| {
            // Quick length-based filter: if lengths differ by more than
            // max_distance, the edit distance must be > max_distance.
            let len_diff = (c.len() as isize - input.len() as isize).unsigned_abs();
            len_diff <= max_distance
        })
        .map(|c| (c, levenshtein(input, c)))
        .filter(|(_, dist)| *dist <= max_distance && *dist > 0)
        .min_by_key(|(_, dist)| *dist)
        .map(|(c, _)| c)
}

/// VUMA language keywords, used for "did you mean?" suggestions.
pub const VUMA_KEYWORDS: &[&str] = &[
    "fn", "let", "ptr", "region", "alloc", "allocate", "free", "derive",
    "cast", "read", "write", "sync", "if", "else", "while", "for", "return",
    "struct", "enum", "match", "unsafe", "safe", "bd", "repd", "capd", "reld",
    "import", "export", "mod", "use", "self", "super", "async", "await",
    "spawn", "lock", "unlock", "channel", "send", "recv", "true", "false",
    "as", "sizeof", "alignof",
];

/// Suggest a keyword similar to `input` using Levenshtein distance.
///
/// Returns the closest keyword within distance 2, or `None`.
pub fn suggest_keyword(input: &str) -> Option<&'static str> {
    let mut best: Option<(&'static str, usize)> = None;
    for &kw in VUMA_KEYWORDS {
        if kw == input {
            continue; // exact match is not a suggestion
        }
        let dist = levenshtein(input, kw);
        if dist <= 2 {
            match best {
                Some((_, best_dist)) if dist >= best_dist => {}
                _ => best = Some((kw, dist)),
            }
        }
    }
    best.map(|(kw, _)| kw)
}

/// Format a "did you mean?" suggestion string, e.g.
/// `"did you mean 'fn'?"`.
pub fn format_suggestion(input: &str, suggestion: &str) -> String {
    format!("did you mean '{}' instead of '{}'?", suggestion, input)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract the text of a 0-based line number from `source`.
fn get_line(source: &str, line: usize) -> String {
    source
        .lines()
        .nth(line)
        .unwrap_or("")
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Span tests ----------------------------------------------------------

    #[test]
    fn span_new_and_merge() {
        let a = Span::new(10, 20);
        let b = Span::new(15, 30);
        let merged = a.merge(b);
        assert_eq!(merged.start, 10);
        assert_eq!(merged.end, 30);
        assert_eq!(merged.len(), 20);
        assert!(!merged.is_empty());

        let empty = Span::synthetic();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
    }

    // -- SourceLocation tests ------------------------------------------------

    #[test]
    fn source_location_format() {
        let loc = SourceLocation::new(2, 5)
            .with_file("test.vu")
            .with_line_text("region pool = allocate(1024);");
        assert_eq!(loc.format_location(), "test.vu:3:6");
        assert!(loc.render_with_pointer(Some(6)).contains("^^^^^^"));
    }

    #[test]
    fn offset_to_location_basic() {
        let source = "line1\nline2\nline3\n";
        let loc = offset_to_location(source, 6, Some("demo.vu"));
        // offset 6 = start of "line2"
        assert_eq!(loc.line, 1);
        assert_eq!(loc.column, 0);
        assert_eq!(loc.file.as_deref(), Some("demo.vu"));
        assert_eq!(loc.line_text.as_deref(), Some("line2"));
    }

    // -- ParseErrorKind display tests ----------------------------------------

    #[test]
    fn error_kind_display() {
        assert_eq!(ParseErrorKind::UnexpectedToken.to_string(), "unexpected token");
        assert_eq!(ParseErrorKind::ExpectedToken.to_string(), "expected token");
        assert_eq!(ParseErrorKind::InvalidSyntax.to_string(), "invalid syntax");
        assert_eq!(ParseErrorKind::DuplicateDefinition.to_string(), "duplicate definition");
        assert_eq!(ParseErrorKind::UndefinedReference.to_string(), "undefined reference");
        assert_eq!(ParseErrorKind::TypeMismatch.to_string(), "type mismatch");
        assert_eq!(ParseErrorKind::RegionError.to_string(), "region error");
        assert_eq!(ParseErrorKind::BDAnnotationError.to_string(), "BD annotation error");
    }

    // -- ParseError construction tests ---------------------------------------

    #[test]
    fn parse_error_convenience_constructors() {
        let span = Span::new(0, 5);

        let err = ParseError::unexpected("bad token", span);
        assert_eq!(err.kind, ParseErrorKind::UnexpectedToken);
        assert!(err.suggestion.is_none());

        let err = ParseError::expected("'}'", "';'", span);
        assert_eq!(err.kind, ParseErrorKind::ExpectedToken);
        assert!(err.message.contains("expected"));
        assert!(err.message.contains("found"));

        let err = ParseError::invalid_syntax("bad syntax", span);
        assert_eq!(err.kind, ParseErrorKind::InvalidSyntax);

        let err = ParseError::undefined_ref("foo", span);
        assert_eq!(err.kind, ParseErrorKind::UndefinedReference);
        assert!(err.message.contains("foo"));

        let err = ParseError::region_error("bad region", span);
        assert_eq!(err.kind, ParseErrorKind::RegionError);

        let err = ParseError::bd_annotation_error("bad bd", span);
        assert_eq!(err.kind, ParseErrorKind::BDAnnotationError);
    }

    #[test]
    fn parse_error_with_suggestion() {
        let span = Span::new(0, 3);
        let err = ParseError::unexpected("unexpected 'fun'", span)
            .with_suggestion("fn");
        assert_eq!(err.suggestion.as_deref(), Some("fn"));
        let display = err.to_string();
        assert!(display.contains("did you mean"));
        assert!(display.contains("fn"));
    }

    #[test]
    fn parse_error_display_with_source() {
        let source = "region pool = allocate(1024);";
        let span = Span::new(7, 11); // "pool"
        let err = ParseError::undefined_ref("pool", span)
            .with_suggestion("pools");
        let rendered = err.display_with_source(source);
        assert!(rendered.contains("undefined reference"));
        assert!(rendered.contains("did you mean"));
        assert!(rendered.contains("pool"));
    }

    // -- Legacy convenience constructors still work --------------------------

    #[test]
    fn legacy_constructors() {
        let span = Span::new(0, 5);
        let err = ParseError::missing_semi(span);
        assert_eq!(err.kind, ParseErrorKind::MissingSemicolon);

        let err = ParseError::invalid_address("bad hex", span);
        assert_eq!(err.kind, ParseErrorKind::InvalidAddress);

        let err = ParseError::undefined_var("x", span);
        assert_eq!(err.kind, ParseErrorKind::UndefinedVariable);
    }

    // -- ErrorRecovery tests -------------------------------------------------

    #[test]
    fn error_recovery_for_kind() {
        assert_eq!(
            ErrorRecovery::for_kind(&ParseErrorKind::MissingSemicolon),
            ErrorRecovery::InsertMissingToken(";".to_string())
        );
        assert_eq!(
            ErrorRecovery::for_kind(&ParseErrorKind::ExpectedToken),
            ErrorRecovery::SkipOneToken
        );
        assert_eq!(
            ErrorRecovery::for_kind(&ParseErrorKind::UnexpectedToken),
            ErrorRecovery::SkipToStatementBoundary
        );
        assert_eq!(
            ErrorRecovery::for_kind(&ParseErrorKind::InvalidSyntax),
            ErrorRecovery::SkipToStatementBoundary
        );
    }

    #[test]
    fn error_recovery_display() {
        assert_eq!(
            ErrorRecovery::SkipToStatementBoundary.to_string(),
            "skip to next statement"
        );
        assert_eq!(
            ErrorRecovery::InsertMissingToken(";".into()).to_string(),
            "insert missing ';'"
        );
    }

    // -- ParseResult tests ---------------------------------------------------

    #[test]
    fn parse_result_ok() {
        let result: ParseResult<i32> = ParseResult::ok(42);
        assert!(result.is_ok());
        assert!(!result.has_errors());
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn parse_result_ok_with_errors() {
        let err = ParseError::unexpected("test", Span::new(0, 1));
        let result: ParseResult<i32> = ParseResult::ok_with_errors(42, vec![err]);
        assert!(result.is_ok());
        assert!(result.has_errors());
        assert_eq!(result.value, Some(42));
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn parse_result_err() {
        let err = ParseError::unexpected("bad", Span::new(0, 3));
        let result: ParseResult<i32> = ParseResult::from_error(err);
        assert!(result.is_err());
        assert!(result.has_errors());
        assert!(result.value.is_none());
    }

    #[test]
    fn parse_result_map() {
        let result: ParseResult<i32> = ParseResult::ok(10);
        let mapped = result.map(|v| v * 2);
        assert_eq!(mapped.value, Some(20));
        assert!(!mapped.has_errors());
    }

    #[test]
    fn parse_result_merge_errors() {
        let err1 = ParseError::unexpected("e1", Span::new(0, 1));
        let err2 = ParseError::invalid_syntax("e2", Span::new(5, 6));
        let mut result: ParseResult<i32> = ParseResult::ok(1);
        result.push_error(err1);
        let other: ParseResult<String> = ParseResult::from_error(err2);
        result.merge_errors_from(&other);
        assert_eq!(result.errors.len(), 2);
        assert_eq!(result.value, Some(1));
    }

    // -- Diagnostic tests ----------------------------------------------------

    #[test]
    fn diagnostic_construction() {
        let loc = SourceLocation::new(5, 10).with_file("main.vu");
        let diag = Diagnostic::error("type mismatch", loc.clone())
            .with_code("E0001")
            .with_suggestion("u32");
        assert_eq!(diag.severity, Severity::Error);
        assert_eq!(diag.code.as_deref(), Some("E0001"));
        assert_eq!(diag.suggestion.as_deref(), Some("u32"));
    }

    #[test]
    fn diagnostic_from_parse_error() {
        let source = "fn main() { x }";
        let err = ParseError::undefined_var("x", Span::new(13, 14))
            .with_suggestion("y");
        let diag = Diagnostic::from_parse_error(&err, source, Some("main.vu"));
        assert_eq!(diag.severity, Severity::Error);
        assert!(diag.message.contains("x"));
        assert_eq!(diag.suggestion.as_deref(), Some("y"));
        // Should have a child note with error kind.
        assert!(!diag.children.is_empty());
    }

    #[test]
    fn diagnostic_display_with_source() {
        let source = "fn main() { x }";
        let loc = offset_to_location(source, 13, Some("main.vu"));
        let diag = Diagnostic::error("undefined variable `x`", loc)
            .with_suggestion("y");
        let rendered = diag.display_with_source();
        assert!(rendered.contains("error:"));
        assert!(rendered.contains("did you mean"));
    }

    // -- ErrorCollector tests ------------------------------------------------

    #[test]
    fn error_collector_basic() {
        let mut collector = ErrorCollector::new();
        assert!(collector.is_empty());
        assert!(!collector.has_errors());

        let loc = SourceLocation::new(0, 0);
        collector.add(Diagnostic::error("test error", loc));
        assert_eq!(collector.error_count(), 1);
        assert!(collector.has_errors());

        let loc2 = SourceLocation::new(1, 0);
        collector.add(Diagnostic::warning("test warning", loc2));
        assert_eq!(collector.warning_count(), 1);
        assert_eq!(collector.len(), 2);

        let summary = collector.summary();
        assert!(summary.contains("1 error"));
        assert!(summary.contains("1 warning"));
    }

    #[test]
    fn error_collector_dedup() {
        let mut collector = ErrorCollector::new();
        let loc = SourceLocation::new(5, 2);
        collector.add_dedup(Diagnostic::error("duplicate", loc.clone()));
        collector.add_dedup(Diagnostic::error("duplicate", loc.clone()));
        // Same line + message should be deduplicated.
        assert_eq!(collector.len(), 1);

        // Different message on same line is NOT deduplicated.
        collector.add_dedup(Diagnostic::error("different", loc.clone()));
        assert_eq!(collector.len(), 2);
    }

    #[test]
    fn error_collector_merge() {
        let mut a = ErrorCollector::new();
        let mut b = ErrorCollector::new();
        a.add(Diagnostic::error("a-err", SourceLocation::new(0, 0)));
        b.add(Diagnostic::warning("b-warn", SourceLocation::new(1, 0)));
        a.merge(&b);
        assert_eq!(a.len(), 2);
        assert_eq!(a.error_count(), 1);
        assert_eq!(a.warning_count(), 1);
    }

    #[test]
    fn error_collector_take() {
        let mut collector = ErrorCollector::new();
        collector.add(Diagnostic::error("err", SourceLocation::new(0, 0)));
        let diags = collector.take();
        assert_eq!(diags.len(), 1);
        assert!(collector.is_empty());
        assert_eq!(collector.error_count(), 0);
    }

    // -- Levenshtein tests ---------------------------------------------------

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("flaw", "lawn"), 2);
        assert_eq!(levenshtein("fn", "fn"), 0);
        assert_eq!(levenshtein("fun", "fn"), 1);
    }

    // -- Suggestion tests ----------------------------------------------------

    #[test]
    fn suggest_close_match() {
        let candidates: Vec<String> = vec![
            "fn".into(), "let".into(), "region".into(), "struct".into(),
        ];
        let result = suggest("fun", &candidates, 2);
        assert_eq!(result, Some(&"fn".to_string()));

        let result = suggest("legion", &candidates, 2);
        assert_eq!(result, Some(&"region".to_string()));

        let result = suggest("completely_off", &candidates, 2);
        assert!(result.is_none());
    }

    #[test]
    fn suggest_keyword_works() {
        // "fun" is close to "fn"
        let result = suggest_keyword("fun");
        assert_eq!(result, Some("fn"));

        // "regin" is close to "region"
        let result = suggest_keyword("regin");
        assert_eq!(result, Some("region"));

        // "xyz" is not close to any keyword
        let result = suggest_keyword("xyzzy");
        assert!(result.is_none());

        // Exact match returns None (distance 0 is filtered), and no other
        // keyword is within distance 2 of a long nonsense word.
        let result = suggest_keyword("zzzzzzz");
        assert!(result.is_none());
    }

    #[test]
    fn format_suggestion_works() {
        let msg = format_suggestion("fun", "fn");
        assert_eq!(msg, "did you mean 'fn' instead of 'fun'?");
    }

    // -- Full integration: ParseError → Diagnostic → ErrorCollector ----------

    #[test]
    fn full_error_pipeline() {
        let source = "region pool = allocate(1024);";
        let err = ParseError::region_error("region size must be positive", Span::new(22, 26))
            .with_suggestion("allocate(2048)");

        let mut collector = ErrorCollector::new();
        collector.add_parse_error(&err, source, Some("demo.vu"));

        assert_eq!(collector.error_count(), 1);
        let rendered = collector.render_all();
        assert!(rendered.contains("region error"));
        assert!(rendered.contains("did you mean"));
    }

    // -- ParseResult into_result round-trip ----------------------------------

    #[test]
    fn parse_result_into_result() {
        let ok: ParseResult<i32> = ParseResult::ok(42);
        let result = ok.into_result();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);

        let err = ParseError::unexpected("bad", Span::new(0, 3));
        let fail: ParseResult<i32> = ParseResult::from_error(err);
        let res = fail.into_result();
        assert!(res.is_err());
        assert_eq!(res.unwrap_err().len(), 1);
    }

    // -- Severity display ----------------------------------------------------

    #[test]
    fn severity_display() {
        assert_eq!(Severity::Error.to_string(), "error");
        assert_eq!(Severity::Warning.to_string(), "warning");
        assert_eq!(Severity::Note.to_string(), "note");
    }
}
