//! Parse error types for the VUMA language frontend.
//!
//! Provides structured error reporting with source spans and contextual
//! messages suitable for both human review and machine consumption.

use std::fmt;

/// A byte-offset span within a source file, used for error localisation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Classification of parse errors for programmatic handling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParseErrorKind {
    /// The lexer/parser encountered a token it did not expect at this position.
    UnexpectedToken,
    /// A required semicolon separator is missing.
    MissingSemicolon,
    /// An address literal (0x...) is malformed.
    InvalidAddress,
    /// A variable name was used before it was defined in the current scope.
    UndefinedVariable,
    /// A type annotation does not match the inferred or expected type.
    TypeMismatch,
    /// A name was defined more than once in the same scope.
    DuplicateDefinition,
}

/// A single parse error with full context for reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseError {
    /// Human-readable error description.
    pub message: String,
    /// Source byte-offset span where the error originates.
    pub span: Span,
    /// Classification of the error kind.
    pub kind: ParseErrorKind,
}

impl ParseError {
    /// Construct a new parse error.
    pub fn new(message: impl Into<String>, span: Span, kind: ParseErrorKind) -> Self {
        Self {
            message: message.into(),
            span,
            kind,
        }
    }

    /// Convenience: unexpected-token error.
    pub fn unexpected(msg: impl Into<String>, span: Span) -> Self {
        Self::new(msg, span, ParseErrorKind::UnexpectedToken)
    }

    /// Convenience: missing-semicolon error.
    pub fn missing_semi(span: Span) -> Self {
        Self::new("expected semicolon", span, ParseErrorKind::MissingSemicolon)
    }

    /// Convenience: invalid-address-literal error.
    pub fn invalid_address(msg: impl Into<String>, span: Span) -> Self {
        Self::new(msg, span, ParseErrorKind::InvalidAddress)
    }

    /// Convenience: undefined-variable error.
    pub fn undefined_var(name: impl Into<String>, span: Span) -> Self {
        Self::new(
            format!("undefined variable `{}`", name.into()),
            span,
            ParseErrorKind::UndefinedVariable,
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

    /// Render the error with source context and a visual pointer.
    ///
    /// The `source` parameter should be the full source text. The function
    /// extracts the relevant line and draws a `^^^` pointer under the
    /// offending region.
    pub fn display_with_source(&self, source: &str) -> String {
        let line_col = span_to_line_col(source, self.span.start);
        let line_text = get_line(source, line_col.line);

        let pointer = if self.span.is_empty() {
            " ".repeat(line_col.column) + "^"
        } else {
            let padding = " ".repeat(line_col.column);
            let carets = "^".repeat(self.span.len().max(1));
            format!("{}{}", padding, carets)
        };

        format!(
            "error: {}\n  --> line {}:{}\n   |\n{: >3} | {}\n   | {}\n",
            self.message,
            line_col.line + 1,
            line_col.column + 1,
            line_col.line + 1,
            line_text,
            pointer
        )
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({}..{})", self.message, self.span.start, self.span.end)
    }
}

impl std::error::Error for ParseError {}

// ---------------------------------------------------------------------------
// Helper: zero-width source location types (not exported as public API)
// ---------------------------------------------------------------------------

/// Line (0-based) and column (0-based) within source text.
struct LineCol {
    line: usize,
    column: usize,
}

/// Convert a byte offset to (line, column), both 0-based.
fn span_to_line_col(source: &str, offset: usize) -> LineCol {
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
    LineCol { line: line, column: col }
}

/// Extract the text of a 0-based line number from `source`.
fn get_line(source: &str, line: usize) -> String {
    source
        .lines()
        .nth(line)
        .unwrap_or("")
        .to_string()
}

// ---------------------------------------------------------------------------
// Serde / trait imports used by derive macros
// ---------------------------------------------------------------------------
use serde::{Deserialize, Serialize};
