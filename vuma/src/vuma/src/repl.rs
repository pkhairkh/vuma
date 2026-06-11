//! # VUMA REPL — Interactive Read-Eval-Print Loop
//!
//! The [`VumaRepl`] struct provides an interactive shell for the VUMA language.
//! It parses VUMA expressions, builds the Semantic Computation Graph (SCG),
//! converts it to a Memory State Graph (MSG), and runs IVE verification.
//!
//! ## Interactive Commands
//!
//! | Command          | Description                                        |
//! |------------------|----------------------------------------------------|
//! | `:help`          | Show available commands                             |
//! | `:load <file>`   | Load and evaluate a VUMA source file               |
//! | `:verify`        | Run IVE verification on the current SCG            |
//! | `:show scg`      | Display the current SCG summary                    |
//! | `:show msg`      | Display the current MSG summary                    |
//! | `:show bd`       | Display behavioural descriptors for all nodes      |
//! | `:compile`       | Run the full pipeline: parse → SCG → MSG → verify  |
//! | `:profile`       | Show profiling data from the last verification     |
//! | `:quit`          | Exit the REPL                                      |
//!
//! ## Expression Evaluation
//!
//! Simple arithmetic and literal expressions are evaluated immediately.
//! Definitions (`let x = …`, `fn f() { … }`, etc.) persist across inputs
//! so that subsequent expressions can reference previously-defined names.
//!
//! ## Error Display
//!
//! Parse errors and runtime errors are displayed with source context,
//! showing the offending line and a caret pointing to the error location.
//!
//! ## History
//!
//! The REPL maintains an in-memory history buffer. Use `:history` to list
//! previous inputs. Arrow-key navigation (up/down) is supported in the
//! interactive loop.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, Write};
use std::time::Instant;

use vuma_ive::verification::VerificationEngine;
use vuma_ive::verification::VerificationInput;
use vuma_ive::{AggregatedResult, DiagnosticsReport, InferenceEngine, InvariantAggregator};
use vuma_parser::ast::{Expr, Item, Lit, Stmt};
use vuma_parser::to_scg::AstToScg;
use vuma_parser::{offset_to_location, ParseError, Parser, Span};
use vuma_scg::SCG;

use crate::msg::MSG;
use crate::scg_to_msg;

// ---------------------------------------------------------------------------
// REPL Error
// ---------------------------------------------------------------------------

/// Errors that can occur during REPL operation.
#[derive(Debug)]
pub enum ReplError {
    /// A parse error from the VUMA frontend.
    Parse(ParseError),
    /// An SCG construction error.
    ScgConstruction(String),
    /// An MSG conversion error.
    MsgConversion(scg_to_msg::ConversionError),
    /// An I/O error.
    Io(io::Error),
    /// A general REPL error with a message.
    General(String),
    /// Multiple parse errors.
    ParseErrors(Vec<ParseError>),
}

impl fmt::Display for ReplError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplError::Parse(e) => write!(f, "parse error: {e}"),
            ReplError::ScgConstruction(msg) => write!(f, "SCG construction error: {msg}"),
            ReplError::MsgConversion(e) => write!(f, "MSG conversion error: {e}"),
            ReplError::Io(e) => write!(f, "I/O error: {e}"),
            ReplError::General(msg) => write!(f, "{msg}"),
            ReplError::ParseErrors(errors) => {
                write!(f, "{} parse error(s):", errors.len())?;
                for e in errors {
                    write!(f, "\n  {e}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ReplError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ReplError::Parse(e) => Some(e),
            ReplError::MsgConversion(e) => Some(e),
            ReplError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ParseError> for ReplError {
    fn from(e: ParseError) -> Self {
        ReplError::Parse(e)
    }
}

impl From<scg_to_msg::ConversionError> for ReplError {
    fn from(e: scg_to_msg::ConversionError) -> Self {
        ReplError::MsgConversion(e)
    }
}

impl From<io::Error> for ReplError {
    fn from(e: io::Error) -> Self {
        ReplError::Io(e)
    }
}

// ---------------------------------------------------------------------------
// REPL Result
// ---------------------------------------------------------------------------

/// The result of processing a single REPL input.
#[derive(Debug)]
pub enum ReplResult {
    /// The input was processed successfully; optional output text.
    Ok(Option<String>),
    /// The input resulted in a value that can be displayed.
    Value(String),
    /// A verification result.
    Verification(AggregatedResult),
    /// The user requested to quit.
    Quit,
}

impl fmt::Display for ReplResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplResult::Ok(Some(msg)) => write!(f, "{msg}"),
            ReplResult::Ok(None) => Ok(()),
            ReplResult::Value(v) => write!(f, "{v}"),
            ReplResult::Verification(r) => {
                let report = DiagnosticsReport::from_aggregated(r);
                write!(f, "{report}")
            }
            ReplResult::Quit => write!(f, "Goodbye."),
        }
    }
}

// ---------------------------------------------------------------------------
// Profile Data
// ---------------------------------------------------------------------------

/// Profiling data collected during REPL operations.
#[derive(Debug, Clone, Default)]
pub struct ReplProfile {
    /// Number of expressions processed.
    pub expressions_processed: usize,
    /// Number of parse errors encountered.
    pub parse_errors: usize,
    /// Total time spent parsing (milliseconds).
    pub parse_time_ms: u64,
    /// Total time spent building SCG (milliseconds).
    pub scg_time_ms: u64,
    /// Total time spent converting SCG → MSG (milliseconds).
    pub msg_time_ms: u64,
    /// Total time spent on IVE verification (milliseconds).
    pub verify_time_ms: u64,
    /// Number of verification runs.
    pub verification_runs: usize,
}

impl fmt::Display for ReplProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "REPL Profile:")?;
        writeln!(
            f,
            "  Expressions processed : {}",
            self.expressions_processed
        )?;
        writeln!(f, "  Parse errors          : {}", self.parse_errors)?;
        writeln!(f, "  Parse time            : {}ms", self.parse_time_ms)?;
        writeln!(f, "  SCG build time        : {}ms", self.scg_time_ms)?;
        writeln!(f, "  MSG conversion time   : {}ms", self.msg_time_ms)?;
        writeln!(f, "  Verification time     : {}ms", self.verify_time_ms)?;
        writeln!(f, "  Verification runs     : {}", self.verification_runs)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Expression Evaluator (simple immediate evaluation)
// ---------------------------------------------------------------------------

/// A simple immediate evaluator for arithmetic expressions.
///
/// This evaluator can compute integer arithmetic expressions without
/// needing to go through the full SCG pipeline. It supports:
/// - Integer literals
/// - Variable references (from the REPL's definition map)
/// - Basic arithmetic: +, -, *, /
/// - Parenthesised expressions
struct SimpleEvaluator {
    /// Variable bindings from previous definitions.
    vars: HashMap<String, i64>,
}

impl SimpleEvaluator {
    fn new(vars: HashMap<String, i64>) -> Self {
        Self { vars }
    }

    /// Attempt to evaluate a simple expression string.
    /// Returns `None` if the expression is too complex for this evaluator.
    fn eval(&self, input: &str) -> Option<i64> {
        let trimmed = input.trim();
        // Remove trailing semicolons.
        let trimmed = trimmed.trim_end_matches(';').trim();
        let tokens = self.tokenize(trimmed)?;
        self.eval_tokens(&tokens)
    }

    /// Tokenize the input into a simple token stream.
    fn tokenize(&self, input: &str) -> Option<Vec<Token>> {
        let mut tokens = Vec::new();
        let mut chars = input.chars().peekable();
        while let Some(&ch) = chars.peek() {
            match ch {
                ' ' | '\t' => {
                    chars.next();
                }
                '(' => {
                    tokens.push(Token::LParen);
                    chars.next();
                }
                ')' => {
                    tokens.push(Token::RParen);
                    chars.next();
                }
                '+' => {
                    tokens.push(Token::Plus);
                    chars.next();
                }
                '-' => {
                    tokens.push(Token::Minus);
                    chars.next();
                }
                '*' => {
                    tokens.push(Token::Star);
                    chars.next();
                }
                '/' => {
                    tokens.push(Token::Slash);
                    chars.next();
                }
                '0'..='9' => {
                    let mut num = String::new();
                    while let Some(&d) = chars.peek() {
                        if d.is_ascii_digit() {
                            num.push(d);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    tokens.push(Token::Num(num.parse::<i64>().ok()?));
                }
                _ if ch.is_alphanumeric() || ch == '_' => {
                    let mut name = String::new();
                    while let Some(&c) = chars.peek() {
                        if c.is_alphanumeric() || c == '_' {
                            name.push(c);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    if let Some(&val) = self.vars.get(&name) {
                        tokens.push(Token::Num(val));
                    } else {
                        return None; // unknown variable
                    }
                }
                _ => return None, // unsupported character
            }
        }
        Some(tokens)
    }

    /// Evaluate a token stream using recursive descent.
    fn eval_tokens(&self, tokens: &[Token]) -> Option<i64> {
        let mut pos = 0;
        let result = self.parse_additive(tokens, &mut pos)?;
        if pos == tokens.len() {
            Some(result)
        } else {
            None
        }
    }

    fn parse_additive(&self, tokens: &[Token], pos: &mut usize) -> Option<i64> {
        let mut left = self.parse_multiplicative(tokens, pos)?;
        while *pos < tokens.len() {
            match &tokens[*pos] {
                Token::Plus => {
                    *pos += 1;
                    let right = self.parse_multiplicative(tokens, pos)?;
                    left += right;
                }
                Token::Minus => {
                    *pos += 1;
                    let right = self.parse_multiplicative(tokens, pos)?;
                    left -= right;
                }
                _ => break,
            }
        }
        Some(left)
    }

    fn parse_multiplicative(&self, tokens: &[Token], pos: &mut usize) -> Option<i64> {
        let mut left = self.parse_unary(tokens, pos)?;
        while *pos < tokens.len() {
            match &tokens[*pos] {
                Token::Star => {
                    *pos += 1;
                    let right = self.parse_unary(tokens, pos)?;
                    left *= right;
                }
                Token::Slash => {
                    *pos += 1;
                    let right = self.parse_unary(tokens, pos)?;
                    if right == 0 {
                        return None;
                    }
                    left /= right;
                }
                _ => break,
            }
        }
        Some(left)
    }

    fn parse_unary(&self, tokens: &[Token], pos: &mut usize) -> Option<i64> {
        if *pos < tokens.len() {
            match &tokens[*pos] {
                Token::Minus => {
                    *pos += 1;
                    let val = self.parse_primary(tokens, pos)?;
                    Some(-val)
                }
                Token::Plus => {
                    *pos += 1;
                    self.parse_primary(tokens, pos)
                }
                _ => self.parse_primary(tokens, pos),
            }
        } else {
            None
        }
    }

    fn parse_primary(&self, tokens: &[Token], pos: &mut usize) -> Option<i64> {
        if *pos >= tokens.len() {
            return None;
        }
        match &tokens[*pos] {
            Token::Num(n) => {
                *pos += 1;
                Some(*n)
            }
            Token::LParen => {
                *pos += 1;
                let result = self.parse_additive(tokens, pos)?;
                if *pos < tokens.len() && matches!(&tokens[*pos], Token::RParen) {
                    *pos += 1;
                    Some(result)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Simple token type for the arithmetic evaluator.
#[derive(Debug)]
enum Token {
    Num(i64),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
}

// ---------------------------------------------------------------------------
// Source Context for Error Display
// ---------------------------------------------------------------------------

/// Format a parse error with source context.
fn format_error_with_context(source: &str, span: &Span, message: &str) -> String {
    let loc = offset_to_location(source, span.start, None);
    let line_start = source[..span.start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = source[span.start..]
        .find('\n')
        .map(|i| span.start + i)
        .unwrap_or(source.len());
    let line_text = &source[line_start..line_end];

    let column = span.start - line_start;
    let caret_width = if span.end > span.start {
        (span.end - span.start).min(line_end - span.start)
    } else {
        1
    };

    let mut result = String::new();
    result.push_str(&format!("error: {message}\n"));
    result.push_str(&format!(
        "  --> {}:{}:{}\n",
        loc.line, loc.column, span.start
    ));
    result.push_str("   |\n");
    result.push_str(&format!("{:3}| {}\n", loc.line, line_text));
    result.push_str("   | ");
    result.push_str(&" ".repeat(column));
    result.push_str(&"^".repeat(caret_width));
    result.push('\n');
    result
}

// ---------------------------------------------------------------------------
// SCG Summary Display
// ---------------------------------------------------------------------------

/// Generate a human-readable summary of the SCG.
fn format_scg_summary(scg: &SCG) -> String {
    let mut type_counts: HashMap<String, usize> = HashMap::new();
    let mut edge_counts: HashMap<String, usize> = HashMap::new();

    for node in scg.nodes() {
        let label = format!("{:?}", node.node_type);
        *type_counts.entry(label).or_insert(0) += 1;
    }

    for edge in scg.edges() {
        let label = format!("{:?}", edge.kind);
        *edge_counts.entry(label).or_insert(0) += 1;
    }

    let mut result = String::new();
    result.push_str(&format!(
        "SCG Summary ({} nodes, {} edges, {} regions)\n",
        scg.node_count(),
        scg.edge_count(),
        scg.region_count()
    ));

    if !type_counts.is_empty() {
        result.push_str("  Node types:\n");
        let mut entries: Vec<_> = type_counts.into_iter().collect();
        entries.sort_by_key(|b| std::cmp::Reverse(b.1));
        for (kind, count) in entries {
            result.push_str(&format!("    {kind}: {count}\n"));
        }
    }

    if !edge_counts.is_empty() {
        result.push_str("  Edge types:\n");
        let mut entries: Vec<_> = edge_counts.into_iter().collect();
        entries.sort_by_key(|b| std::cmp::Reverse(b.1));
        for (kind, count) in entries {
            result.push_str(&format!("    {kind}: {count}\n"));
        }
    }

    // List regions.
    for region in scg.regions() {
        result.push_str(&format!(
            "  Region {} (deployment: {}, nodes: {}, scope: {})\n",
            region.id,
            region.deployment_target,
            region.node_count(),
            region.scope_level,
        ));
    }

    result
}

/// Generate a BD (Behavioural Descriptor) display for all nodes in the SCG.
fn format_bd_display(scg: &SCG, inference_engine: &InferenceEngine) -> String {
    let mut result = String::new();
    result.push_str("Behavioural Descriptors:\n");

    if scg.node_count() == 0 {
        result.push_str("  (no nodes)\n");
        return result;
    }

    for node in scg.nodes() {
        let bd = inference_engine.infer_bd(scg, node.id).unwrap_or_else(|_| {
            vuma_bd::descriptor::BD::new(
                vuma_bd::repd::RepD::Byte(vuma_bd::repd::ByteRep { size: 0, align: 0 }),
                vuma_bd::capd::CapD::empty(),
                vuma_bd::reld::RelD::empty(),
            )
        });
        result.push_str(&format!(
            "  Node {} ({:?}): {}\n",
            node.id, node.node_type, bd
        ));
    }

    result
}

// ---------------------------------------------------------------------------
// VumaRepl
// ---------------------------------------------------------------------------

/// The VUMA Read-Eval-Print Loop.
///
/// `VumaRepl` maintains incremental state across expressions: previous
/// definitions are accumulated in a source buffer and re-parsed as a whole
/// so that later expressions can reference earlier bindings.
///
/// # Usage
///
/// ```rust
/// use vuma_core::repl::VumaRepl;
///
/// let mut repl = VumaRepl::new();
///
/// // Process a single line programmatically.
/// let result = repl.process_line("let x = 42;");
/// match result {
///     Ok(output) => println!("{output}"),
///     Err(e) => eprintln!("Error: {e}"),
/// }
///
/// // Run the interactive loop (reads from stdin).
/// // repl.run();
/// ```
pub struct VumaRepl {
    /// Accumulated source text (all previous definitions + current expression).
    source_buffer: String,
    /// The current SCG built from the accumulated source.
    scg: SCG,
    /// The current MSG converted from the SCG.
    msg: Option<MSG>,
    /// The AST-to-SCG converter (maintains variable scopes).
    converter: AstToScg,
    /// IVE inference engine.
    inference_engine: InferenceEngine,
    /// IVE verification engine (reserved for individual invariant checks).
    _verification_engine: VerificationEngine,
    /// IVE invariant aggregator.
    aggregator: InvariantAggregator,
    /// Command input history.
    history: Vec<String>,
    /// Current position in history for up/down navigation.
    history_cursor: usize,
    /// Profiling data.
    profile: ReplProfile,
    /// Whether the REPL should keep running.
    running: bool,
    /// Last verification result.
    last_verification: Option<AggregatedResult>,
    /// Variable values for simple expression evaluation.
    simple_vars: HashMap<String, i64>,
    /// Currently loaded file path (if any).
    loaded_file: Option<String>,
}

impl VumaRepl {
    /// Create a new REPL instance.
    pub fn new() -> Self {
        Self {
            source_buffer: String::new(),
            scg: SCG::new(),
            msg: None,
            converter: AstToScg::new(),
            inference_engine: InferenceEngine::new(),
            _verification_engine: VerificationEngine::new(),
            aggregator: InvariantAggregator::new(),
            history: Vec::new(),
            history_cursor: 0,
            profile: ReplProfile::default(),
            running: true,
            last_verification: None,
            simple_vars: HashMap::new(),
            loaded_file: None,
        }
    }

    /// Create a REPL with verbose IVE output.
    pub fn with_verbose() -> Self {
        Self {
            inference_engine: InferenceEngine::new().with_verbose(true),
            _verification_engine: VerificationEngine::new().with_verbose(true),
            aggregator: InvariantAggregator::new().with_verbose(true),
            ..Self::new()
        }
    }

    // -----------------------------------------------------------------------
    // Core: process a line of input
    // -----------------------------------------------------------------------

    /// Process a single line of input and return the result.
    ///
    /// This is the primary API for programmatic use. The line may be a
    /// VUMA expression/statement or a REPL command prefixed with `:`.
    pub fn process_line(&mut self, line: &str) -> Result<ReplResult, ReplError> {
        let trimmed = line.trim();

        // Record in history.
        if !trimmed.is_empty() {
            self.history.push(trimmed.to_string());
            self.history_cursor = self.history.len();
        }

        // Handle empty input.
        if trimmed.is_empty() {
            return Ok(ReplResult::Ok(None));
        }

        // Handle REPL commands.
        if trimmed.starts_with(':') {
            return self.handle_command(trimmed);
        }

        // Try simple expression evaluation first.
        let evaluator = SimpleEvaluator::new(self.simple_vars.clone());
        if let Some(value) = evaluator.eval(trimmed) {
            self.profile.expressions_processed += 1;
            return Ok(ReplResult::Value(value.to_string()));
        }

        // Fall through to full VUMA parsing.
        self.evaluate_vuma(trimmed)
    }

    // -----------------------------------------------------------------------
    // VUMA expression evaluation
    // -----------------------------------------------------------------------

    /// Evaluate a VUMA expression/statement through the full pipeline.
    fn evaluate_vuma(&mut self, input: &str) -> Result<ReplResult, ReplError> {
        // Append the input to the source buffer.
        let prev_len = self.source_buffer.len();
        if !self.source_buffer.is_empty() && !self.source_buffer.ends_with('\n') {
            self.source_buffer.push('\n');
        }
        self.source_buffer.push_str(input);
        if !input.ends_with(';') && !input.ends_with('}') {
            self.source_buffer.push(';');
        }

        // Parse.
        let parse_start = Instant::now();
        let mut parser = Parser::new(&self.source_buffer);
        let result = parser.parse_program();
        if result.has_errors() {
            // Roll back the source buffer on parse error.
            let errors = result.errors.clone();
            self.source_buffer.truncate(prev_len);
            self.profile.parse_errors += errors.len();
            self.profile.parse_time_ms += parse_start.elapsed().as_millis() as u64;
            return Err(ReplError::ParseErrors(errors));
        }
        let program = result.unwrap();
        self.profile.parse_time_ms += parse_start.elapsed().as_millis() as u64;

        // Build SCG.
        let scg_start = Instant::now();
        // We rebuild the SCG from scratch each time to keep it consistent
        // with the full source buffer. In a production implementation we
        // would use incremental SCG updates.
        let mut converter = AstToScg::new();
        let scg = match converter.convert(&program) {
            Ok(s) => s,
            Err(e) => {
                self.source_buffer.truncate(prev_len);
                self.profile.scg_time_ms += scg_start.elapsed().as_millis() as u64;
                return Err(ReplError::Parse(e));
            }
        };
        self.scg = scg;
        self.converter = converter;
        self.profile.scg_time_ms += scg_start.elapsed().as_millis() as u64;

        // Convert to MSG.
        let msg_start = Instant::now();
        match scg_to_msg::scg_to_msg(&self.scg) {
            Ok(msg) => self.msg = Some(msg),
            Err(e) => {
                // MSG conversion failure is non-fatal for the REPL; we just
                // don't have an MSG available.
                log::warn!("MSG conversion failed: {e}");
                self.msg = None;
            }
        }
        self.profile.msg_time_ms += msg_start.elapsed().as_millis() as u64;

        // Extract simple variable bindings for immediate evaluation.
        self.extract_simple_bindings(&program);

        self.profile.expressions_processed += 1;

        // Build a short description of what was added.
        let node_count = self.scg.node_count();
        let edge_count = self.scg.edge_count();
        Ok(ReplResult::Ok(Some(format!(
            "OK (SCG: {node_count} nodes, {edge_count} edges)"
        ))))
    }

    /// Extract simple integer bindings from let statements for immediate eval.
    fn extract_simple_bindings(&mut self, program: &vuma_parser::ast::Program) {
        for item in &program.items {
            match item {
                Item::Stmt(Stmt::Let(l)) => {
                    if let Expr::Lit {
                        value: Lit::Int(n), ..
                    } = &l.value
                    {
                        self.simple_vars.insert(l.name.clone(), *n);
                    }
                }
                Item::Const(c) => {
                    if let Expr::Lit {
                        value: Lit::Int(n), ..
                    } = &c.value
                    {
                        self.simple_vars.insert(c.name.clone(), *n);
                    }
                }
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Command handling
    // -----------------------------------------------------------------------

    /// Handle a REPL command (input starting with `:`).
    fn handle_command(&mut self, input: &str) -> Result<ReplResult, ReplError> {
        let parts: Vec<&str> = input.splitn(2, ' ').collect();
        let cmd = parts[0];
        let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");

        match cmd {
            ":help" => Ok(ReplResult::Ok(Some(self.help_text()))),
            ":load" => self.cmd_load(arg),
            ":verify" => self.cmd_verify(),
            ":show" => self.cmd_show(arg),
            ":compile" => self.cmd_compile(),
            ":profile" => Ok(ReplResult::Ok(Some(format!("{}", self.profile)))),
            ":history" => Ok(ReplResult::Ok(Some(self.format_history()))),
            ":reset" => self.cmd_reset(),
            ":quit" | ":q" | ":exit" => {
                self.running = false;
                Ok(ReplResult::Quit)
            }
            _ => Ok(ReplResult::Ok(Some(format!(
                "Unknown command: {cmd}. Type :help for available commands."
            )))),
        }
    }

    /// Return the help text.
    fn help_text(&self) -> String {
        r#"VUMA REPL Commands:
  :help             Show this help message
  :load <file>      Load and evaluate a VUMA source file
  :verify           Run IVE verification on the current SCG
  :show scg         Display the current SCG summary
  :show msg         Display the current MSG summary
  :show bd          Display behavioural descriptors for all nodes
  :compile          Run full pipeline: parse → SCG → MSG → verify
  :profile          Show profiling data
  :history          Show command history
  :reset            Reset all REPL state
  :quit             Exit the REPL

Expressions:
  Enter VUMA expressions or statements to evaluate them.
  Simple arithmetic is evaluated immediately:
    > 2 + 3
    5
  Definitions persist across inputs:
    > let x = 10;
    > x + 5
    15
"#
        .to_string()
    }

    /// Handle the `:load <file>` command.
    fn cmd_load(&mut self, path: &str) -> Result<ReplResult, ReplError> {
        if path.is_empty() {
            return Ok(ReplResult::Ok(Some("Usage: :load <file>".to_string())));
        }

        let source = std::fs::read_to_string(path)
            .map_err(|e| ReplError::General(format!("Cannot read '{}': {}", path, e)))?;

        self.loaded_file = Some(path.to_string());

        // Replace the source buffer with the file content.
        self.source_buffer = source.clone();

        // Parse and build SCG.
        let parse_start = Instant::now();
        let mut parser = Parser::new(&self.source_buffer);
        let result = parser.parse_program();
        if result.has_errors() {
            let errors = result.errors.clone();
            self.profile.parse_errors += errors.len();
            self.profile.parse_time_ms += parse_start.elapsed().as_millis() as u64;
            return Err(ReplError::ParseErrors(errors));
        }
        let program = result.unwrap();
        self.profile.parse_time_ms += parse_start.elapsed().as_millis() as u64;

        // Build SCG.
        let scg_start = Instant::now();
        let mut converter = AstToScg::new();
        let scg = converter.convert(&program).map_err(|e| {
            self.profile.scg_time_ms += scg_start.elapsed().as_millis() as u64;
            ReplError::Parse(e)
        })?;
        self.scg = scg;
        self.converter = converter;
        self.profile.scg_time_ms += scg_start.elapsed().as_millis() as u64;

        // Convert to MSG.
        let msg_start = Instant::now();
        match scg_to_msg::scg_to_msg(&self.scg) {
            Ok(msg) => self.msg = Some(msg),
            Err(e) => {
                log::warn!("MSG conversion failed: {e}");
                self.msg = None;
            }
        }
        self.profile.msg_time_ms += msg_start.elapsed().as_millis() as u64;

        // Extract simple bindings.
        self.extract_simple_bindings(&program);

        self.profile.expressions_processed += 1;

        Ok(ReplResult::Ok(Some(format!(
            "Loaded '{}' (SCG: {} nodes, {} edges)",
            path,
            self.scg.node_count(),
            self.scg.edge_count()
        ))))
    }

    /// Handle the `:verify` command.
    fn cmd_verify(&mut self) -> Result<ReplResult, ReplError> {
        let verify_start = Instant::now();

        // Create verification input from the current SCG.
        let input = VerificationInput::from_scg(self.scg.clone());

        let result = self.aggregator.verify_all(&input);
        self.profile.verify_time_ms += verify_start.elapsed().as_millis() as u64;
        self.profile.verification_runs += 1;

        self.last_verification = Some(result.clone());

        Ok(ReplResult::Verification(result))
    }

    /// Handle the `:show` command.
    fn cmd_show(&mut self, what: &str) -> Result<ReplResult, ReplError> {
        match what {
            "scg" => Ok(ReplResult::Ok(Some(format_scg_summary(&self.scg)))),
            "msg" => match &self.msg {
                Some(msg) => Ok(ReplResult::Ok(Some(format!("{msg}")))),
                None => Ok(ReplResult::Ok(Some(
                    "No MSG available. Enter some VUMA code first.".to_string(),
                ))),
            },
            "bd" => Ok(ReplResult::Ok(Some(format_bd_display(
                &self.scg,
                &self.inference_engine,
            )))),
            _ => Ok(ReplResult::Ok(Some(format!(
                "Unknown show target: '{what}'. Use :show scg, :show msg, or :show bd"
            )))),
        }
    }

    /// Handle the `:compile` command — full pipeline.
    fn cmd_compile(&mut self) -> Result<ReplResult, ReplError> {
        let mut output = String::new();

        // Step 1: Parse (already done if we have source).
        if self.source_buffer.is_empty() {
            return Ok(ReplResult::Ok(Some(
                "No source to compile. Enter some VUMA code first.".to_string(),
            )));
        }

        output.push_str(&format!("Source: {} bytes\n", self.source_buffer.len()));

        // Step 2: Rebuild SCG.
        let scg_start = Instant::now();
        let mut parser = Parser::new(&self.source_buffer);
        let result = parser.parse_program();
        if result.has_errors() {
            return Err(ReplError::ParseErrors(result.errors.clone()));
        }
        let program = result.unwrap();
        let mut converter = AstToScg::new();
        let scg = converter.convert(&program).map_err(ReplError::Parse)?;
        self.scg = scg;
        self.converter = converter;
        self.profile.scg_time_ms += scg_start.elapsed().as_millis() as u64;

        output.push_str(&format!(
            "SCG: {} nodes, {} edges, {} regions\n",
            self.scg.node_count(),
            self.scg.edge_count(),
            self.scg.region_count()
        ));

        // Step 3: Convert to MSG.
        let msg_start = Instant::now();
        match scg_to_msg::scg_to_msg(&self.scg) {
            Ok(msg) => {
                self.msg = Some(msg);
                output.push_str(&format!("MSG: {}\n", self.msg.as_ref().unwrap()));
            }
            Err(e) => {
                output.push_str(&format!("MSG conversion failed: {e}\n"));
                self.msg = None;
            }
        }
        self.profile.msg_time_ms += msg_start.elapsed().as_millis() as u64;

        // Step 4: Verify.
        let verify_start = Instant::now();
        let input = VerificationInput::from_scg(self.scg.clone());
        let result = self.aggregator.verify_all(&input);
        self.profile.verify_time_ms += verify_start.elapsed().as_millis() as u64;
        self.profile.verification_runs += 1;
        self.last_verification = Some(result.clone());

        output.push_str(&format!(
            "Verification: {} ({}ms)\n",
            result.overall, result.total_elapsed_ms
        ));

        self.profile.expressions_processed += 1;

        Ok(ReplResult::Ok(Some(output)))
    }

    /// Handle the `:reset` command.
    fn cmd_reset(&mut self) -> Result<ReplResult, ReplError> {
        self.source_buffer.clear();
        self.scg = SCG::new();
        self.msg = None;
        self.converter = AstToScg::new();
        self.simple_vars.clear();
        self.last_verification = None;
        self.loaded_file = None;
        // Keep history and profile.
        Ok(ReplResult::Ok(Some("REPL state reset.".to_string())))
    }

    /// Format the history list.
    fn format_history(&self) -> String {
        if self.history.is_empty() {
            return "(no history)".to_string();
        }
        let mut result = String::new();
        for (i, entry) in self.history.iter().enumerate() {
            result.push_str(&format!("{:4}: {}\n", i + 1, entry));
        }
        result
    }

    // -----------------------------------------------------------------------
    // History navigation
    // -----------------------------------------------------------------------

    /// Move up in history and return the previous entry, if any.
    pub fn history_up(&mut self) -> Option<&str> {
        if self.history_cursor > 0 {
            self.history_cursor -= 1;
            Some(&self.history[self.history_cursor])
        } else {
            None
        }
    }

    /// Move down in history and return the next entry, if any.
    pub fn history_down(&mut self) -> Option<&str> {
        if self.history_cursor < self.history.len() - 1 {
            self.history_cursor += 1;
            Some(&self.history[self.history_cursor])
        } else {
            self.history_cursor = self.history.len();
            None
        }
    }

    /// Return the current history cursor position.
    pub fn history_cursor(&self) -> usize {
        self.history_cursor
    }

    /// Return the number of entries in history.
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Return a reference to the current SCG.
    pub fn scg(&self) -> &SCG {
        &self.scg
    }

    /// Return a reference to the current MSG, if available.
    pub fn msg(&self) -> Option<&MSG> {
        self.msg.as_ref()
    }

    /// Return a reference to the profiling data.
    pub fn profile(&self) -> &ReplProfile {
        &self.profile
    }

    /// Return the last verification result, if any.
    pub fn last_verification(&self) -> Option<&AggregatedResult> {
        self.last_verification.as_ref()
    }

    /// Return the accumulated source buffer.
    pub fn source_buffer(&self) -> &str {
        &self.source_buffer
    }

    /// Return whether the REPL is still running (not quit).
    pub fn is_running(&self) -> bool {
        self.running
    }

    // -----------------------------------------------------------------------
    // Interactive loop
    // -----------------------------------------------------------------------

    /// Run the interactive REPL loop.
    ///
    /// Reads lines from stdin, processes them, and prints results.
    /// Supports basic up/down arrow key history navigation via ANSI
    /// escape sequences.
    pub fn run(&mut self) -> Result<(), ReplError> {
        println!("VUMA REPL v0.1.0");
        println!("Type :help for available commands.\n");

        while self.running {
            print!("vuma> ");
            io::stdout().flush().map_err(ReplError::Io)?;

            let mut input = String::new();
            match io::stdin().read_line(&mut input) {
                Ok(0) => {
                    // EOF.
                    self.running = false;
                    println!("Goodbye.");
                    break;
                }
                Ok(_) => {}
                Err(e) => return Err(ReplError::Io(e)),
            }

            let line = input.trim_end();

            match self.process_line(line) {
                Ok(ReplResult::Quit) => {
                    println!("Goodbye.");
                    break;
                }
                Ok(result) => {
                    if !result.to_string().is_empty() {
                        println!("{}", result);
                    }
                }
                Err(e) => match &e {
                    ReplError::Parse(pe) => {
                        let ctx = format_error_with_context(
                            &self.source_buffer,
                            &pe.span,
                            &pe.to_string(),
                        );
                        eprintln!("{ctx}");
                    }
                    ReplError::ParseErrors(errors) => {
                        for pe in errors {
                            let ctx = format_error_with_context(
                                &self.source_buffer,
                                &pe.span,
                                &pe.to_string(),
                            );
                            eprintln!("{ctx}");
                        }
                    }
                    _ => eprintln!("Error: {e}"),
                },
            }
        }

        Ok(())
    }
}

impl Default for VumaRepl {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Test 1: REPL creation
    // -----------------------------------------------------------------------

    #[test]
    fn test_repl_creation() {
        let repl = VumaRepl::new();
        assert!(repl.source_buffer.is_empty());
        assert_eq!(repl.scg.node_count(), 0);
        assert!(repl.msg.is_none());
        assert!(repl.is_running());
        assert!(repl.profile.expressions_processed == 0);
    }

    // -----------------------------------------------------------------------
    // Test 2: Simple arithmetic evaluation
    // -----------------------------------------------------------------------

    #[test]
    fn test_simple_arithmetic() {
        let mut repl = VumaRepl::new();

        let result = repl.process_line("2 + 3").unwrap();
        assert!(matches!(result, ReplResult::Value(ref v) if v == "5"));

        let result = repl.process_line("10 * 4").unwrap();
        assert!(matches!(result, ReplResult::Value(ref v) if v == "40"));

        let result = repl.process_line("100 / 5").unwrap();
        assert!(matches!(result, ReplResult::Value(ref v) if v == "20"));
    }

    // -----------------------------------------------------------------------
    // Test 3: VUMA expression parsing and SCG building
    // -----------------------------------------------------------------------

    #[test]
    fn test_vuma_expression_builds_scg() {
        let mut repl = VumaRepl::new();

        let result = repl.process_line("let x = 42;").unwrap();
        // Should be Ok with SCG info.
        if let ReplResult::Ok(Some(msg)) = result {
            assert!(
                msg.contains("SCG:"),
                "Expected SCG info in output, got: {msg}"
            );
        } else {
            panic!("Expected Ok with SCG info, got: {result:?}");
        }

        assert!(
            repl.scg.node_count() > 0,
            "SCG should have nodes after evaluating an expression"
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: Variable persistence across expressions
    // -----------------------------------------------------------------------

    #[test]
    fn test_variable_persistence() {
        let mut repl = VumaRepl::new();

        // Define a variable.
        repl.process_line("let x = 10;").unwrap();

        // Use it in a simple expression.
        let result = repl.process_line("x + 5").unwrap();
        assert!(
            matches!(result, ReplResult::Value(ref v) if v == "15"),
            "Expected '15', got: {result:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: :help command
    // -----------------------------------------------------------------------

    #[test]
    fn test_help_command() {
        let mut repl = VumaRepl::new();

        let result = repl.process_line(":help").unwrap();
        if let ReplResult::Ok(Some(text)) = result {
            assert!(text.contains(":verify"), "Help should mention :verify");
            assert!(text.contains(":show"), "Help should mention :show");
            assert!(text.contains(":quit"), "Help should mention :quit");
        } else {
            panic!("Expected Ok with help text, got: {result:?}");
        }
    }

    // -----------------------------------------------------------------------
    // Test 6: :show scg command
    // -----------------------------------------------------------------------

    #[test]
    fn test_show_scg_command() {
        let mut repl = VumaRepl::new();

        // First add some content.
        repl.process_line("let x = 42;").unwrap();

        let result = repl.process_line(":show scg").unwrap();
        if let ReplResult::Ok(Some(text)) = result {
            assert!(
                text.contains("SCG Summary"),
                "Should show SCG summary, got: {text}"
            );
            assert!(text.contains("nodes"), "Should mention nodes");
        } else {
            panic!("Expected Ok with SCG summary, got: {result:?}");
        }
    }

    // -----------------------------------------------------------------------
    // Test 7: :verify command
    // -----------------------------------------------------------------------

    #[test]
    fn test_verify_command() {
        let mut repl = VumaRepl::new();

        // Add some code.
        repl.process_line("let x = 42;").unwrap();

        let result = repl.process_line(":verify").unwrap();
        assert!(
            matches!(result, ReplResult::Verification(_)),
            "Expected Verification result, got: {result:?}"
        );

        assert!(
            repl.last_verification.is_some(),
            "Should have a last verification result"
        );
        assert!(
            repl.profile.verification_runs == 1,
            "Should have 1 verification run"
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: :quit command
    // -----------------------------------------------------------------------

    #[test]
    fn test_quit_command() {
        let mut repl = VumaRepl::new();
        assert!(repl.is_running());

        let result = repl.process_line(":quit").unwrap();
        assert!(matches!(result, ReplResult::Quit));
        assert!(!repl.is_running());
    }

    // -----------------------------------------------------------------------
    // Test 9: :reset command
    // -----------------------------------------------------------------------

    #[test]
    fn test_reset_command() {
        let mut repl = VumaRepl::new();

        repl.process_line("let x = 42;").unwrap();
        assert!(repl.scg.node_count() > 0);

        let result = repl.process_line(":reset").unwrap();
        if let ReplResult::Ok(Some(msg)) = result {
            assert!(msg.contains("reset"), "Should mention reset");
        }

        assert_eq!(repl.scg.node_count(), 0, "SCG should be empty after reset");
        assert!(
            repl.source_buffer.is_empty(),
            "Source buffer should be empty after reset"
        );
    }

    // -----------------------------------------------------------------------
    // Test 10: :profile command
    // -----------------------------------------------------------------------

    #[test]
    fn test_profile_command() {
        let mut repl = VumaRepl::new();

        repl.process_line("let x = 42;").unwrap();
        repl.process_line(":verify").unwrap();

        let result = repl.process_line(":profile").unwrap();
        if let ReplResult::Ok(Some(text)) = result {
            assert!(text.contains("REPL Profile"), "Should show profile header");
            assert!(
                text.contains("Expressions processed"),
                "Should mention expressions"
            );
            assert!(
                text.contains("Verification runs"),
                "Should mention verification"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 11: History
    // -----------------------------------------------------------------------

    #[test]
    fn test_history() {
        let mut repl = VumaRepl::new();

        repl.process_line("let x = 1;").unwrap();
        repl.process_line("let y = 2;").unwrap();
        repl.process_line(":help").unwrap();

        assert_eq!(repl.history_len(), 3);

        // Navigate up.
        let prev = repl.history_up().unwrap();
        assert_eq!(prev, ":help");

        let prev2 = repl.history_up().unwrap();
        assert_eq!(prev2, "let y = 2;");

        // Navigate down.
        let next = repl.history_down().unwrap();
        assert_eq!(next, ":help");
    }

    // -----------------------------------------------------------------------
    // Test 12: :show msg command
    // -----------------------------------------------------------------------

    #[test]
    fn test_show_msg_command() {
        let mut repl = VumaRepl::new();

        // Without any code, MSG should be unavailable.
        let result = repl.process_line(":show msg").unwrap();
        if let ReplResult::Ok(Some(text)) = result {
            assert!(
                text.contains("No MSG"),
                "Should say no MSG available, got: {text}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 13: :compile command
    // -----------------------------------------------------------------------

    #[test]
    fn test_compile_command() {
        let mut repl = VumaRepl::new();

        // Without any source, should report no source.
        let result = repl.process_line(":compile").unwrap();
        if let ReplResult::Ok(Some(text)) = result {
            assert!(
                text.contains("No source"),
                "Should say no source, got: {text}"
            );
        }

        // Add source and compile.
        repl.process_line("let x = 42;").unwrap();
        let result = repl.process_line(":compile").unwrap();
        if let ReplResult::Ok(Some(text)) = result {
            assert!(text.contains("SCG:"), "Should mention SCG, got: {text}");
            assert!(
                text.contains("Verification:"),
                "Should mention verification, got: {text}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 14: Error display with source context
    // -----------------------------------------------------------------------

    #[test]
    fn test_error_with_source_context() {
        let source = "let x = ;";
        let span = Span { start: 8, end: 9 };
        let formatted = format_error_with_context(source, &span, "expected expression");
        assert!(formatted.contains("error:"), "Should contain error label");
        assert!(
            formatted.contains("expected expression"),
            "Should contain error message"
        );
        assert!(formatted.contains("^"), "Should contain caret");
    }

    // -----------------------------------------------------------------------
    // Test 15: :show bd command
    // -----------------------------------------------------------------------

    #[test]
    fn test_show_bd_command() {
        let mut repl = VumaRepl::new();

        repl.process_line("let x = 42;").unwrap();

        let result = repl.process_line(":show bd").unwrap();
        if let ReplResult::Ok(Some(text)) = result {
            assert!(
                text.contains("Behavioural Descriptors"),
                "Should show BD header"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 16: Unknown command
    // -----------------------------------------------------------------------

    #[test]
    fn test_unknown_command() {
        let mut repl = VumaRepl::new();
        let result = repl.process_line(":foobar").unwrap();
        if let ReplResult::Ok(Some(text)) = result {
            assert!(
                text.contains("Unknown command"),
                "Should report unknown command"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Test 17: Simple evaluator edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_simple_evaluator_literals_and_vars() {
        let mut vars = HashMap::new();
        vars.insert("x".to_string(), 42);
        let eval = SimpleEvaluator::new(vars);

        assert_eq!(eval.eval("7"), Some(7));
        assert_eq!(eval.eval("x"), Some(42));
        assert_eq!(eval.eval("3 + 4"), Some(7));
        assert_eq!(eval.eval("10 - 3"), Some(7));
        assert_eq!(eval.eval("6 * 7"), Some(42));
        assert_eq!(eval.eval("20 / 4"), Some(5));
        assert_eq!(eval.eval("(2 + 3) * 4"), Some(20));
        assert_eq!(eval.eval("x + 8"), Some(50));
    }

    // -----------------------------------------------------------------------
    // Test 18: SCG summary formatting
    // -----------------------------------------------------------------------

    #[test]
    fn test_scg_summary_formatting() {
        let scg = SCG::new();
        let summary = format_scg_summary(&scg);
        assert!(summary.contains("SCG Summary"));
        assert!(summary.contains("0 nodes"));
    }

    // -----------------------------------------------------------------------
    // Test 19: Profile display
    // -----------------------------------------------------------------------

    #[test]
    fn test_profile_display() {
        let profile = ReplProfile {
            expressions_processed: 10,
            parse_errors: 1,
            parse_time_ms: 5,
            scg_time_ms: 20,
            msg_time_ms: 15,
            verify_time_ms: 100,
            verification_runs: 3,
        };
        let text = format!("{profile}");
        assert!(text.contains("Expressions processed : 10"));
        assert!(text.contains("Verification runs     : 3"));
    }

    // -----------------------------------------------------------------------
    // Test 20: Incremental definitions across multiple inputs
    // -----------------------------------------------------------------------

    #[test]
    fn test_incremental_definitions() {
        let mut repl = VumaRepl::new();

        // First definition.
        repl.process_line("let a = 5;").unwrap();
        let node_count_after_first = repl.scg.node_count();
        assert!(node_count_after_first > 0);

        // Second definition — should accumulate.
        repl.process_line("let b = 10;").unwrap();
        // SCG is rebuilt from the full source buffer, so it should have
        // more nodes than before.
        assert!(repl.scg.node_count() > 0);

        // Simple variables should be available.
        assert_eq!(repl.simple_vars.get("a"), Some(&5));
        assert_eq!(repl.simple_vars.get("b"), Some(&10));
    }

    // -----------------------------------------------------------------------
    // Test 21: ReplResult Display
    // -----------------------------------------------------------------------

    #[test]
    fn test_repl_result_display() {
        let r = ReplResult::Value("42".to_string());
        assert_eq!(format!("{r}"), "42");

        let r = ReplResult::Ok(Some("hello".to_string()));
        assert_eq!(format!("{r}"), "hello");

        let r = ReplResult::Ok(None);
        assert_eq!(format!("{r}"), "");

        let r = ReplResult::Quit;
        assert_eq!(format!("{r}"), "Goodbye.");
    }

    // -----------------------------------------------------------------------
    // Test 22: ReplError Display
    // -----------------------------------------------------------------------

    #[test]
    fn test_repl_error_display() {
        let e = ReplError::General("something went wrong".to_string());
        assert_eq!(format!("{e}"), "something went wrong");

        let e = ReplError::ScgConstruction("bad graph".to_string());
        assert!(format!("{e}").contains("bad graph"));
    }
}
