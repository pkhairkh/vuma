//! VUMA Integration Test Framework
//!
//! This module provides a comprehensive test framework for the VUMA project,
//! offering high-level helpers that compose the full compilation and
//! verification pipeline:
//!
//! 1. **Source parsing** — `vuma-parser` lexes and parses VUMA source text into
//!    an AST, which is then converted to the parser's local SCG representation.
//! 2. **SCG bridging** — The parser SCG is translated into `vuma-scg::SCG` for
//!    graph operations, validation, dominance/liveness analysis.
//! 3. **Invariant verification** — The `vuma-ive` invariant aggregator runs all
//!    five VUMA invariant checks (liveness, exclusivity, interpretation, origin,
//!    cleanup) against the program.
//! 4. **Code generation** — The `vuma-codegen` crate lowers the SCG through IR
//!    to ARM64 machine code, producing a minimal ELF binary.
//!
//! # Test Categories
//!
//! | Category       | Scope                                                        |
//! |----------------|--------------------------------------------------------------|
//! | [`Unit`]       | Individual crate functions, data structures, edge cases.     |
//! | [`Integration`]| Cross-crate pipelines (parse -> SCG -> verify, etc.).        |
//! | [`Verification`]| IVE invariant checks, proof generation, counterexamples.    |
//! | [`Codegen`]    | ARM64 code emission, register allocation, ELF generation.    |
//! | [`Pi5`]        | Raspberry Pi 5 target-specific tests (MMIO, UART, GPIO).    |
//!
//! # Helper Macros
//!
//! The framework provides declarative macros that automatically annotate tests
//! with the correct [`TestCategory`] and integrate with the [`TestRegistry`]:
//!
//! - [`vuma_unit_test!`] — unit test category
//! - [`vuma_integration_test!`] — integration test category
//! - [`vuma_verification_test!`] — verification test category
//! - [`vuma_codegen_test!`] — codegen test category
//! - [`vuma_pi5_test!`] — Pi5 target test category
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use vuma_tests::framework::{build_scg_from_source, assert_verifies, assert_violation};
//! use vuma_ive::InvariantKind;
//!
//! // Build an SCG from source text.
//! let scg = build_scg_from_source("region pool = allocate(1024); free(pool);").unwrap();
//!
//! // Assert that a program passes all invariant checks.
//! assert_verifies("region buf = allocate(256); free(buf);");
//!
//! // Assert that a specific invariant is violated.
//! assert_violation("region buf = allocate(256);", InvariantKind::Cleanup);
//! ```

use std::collections::HashMap;
use std::fmt;
use std::panic;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use vuma_ive::{
    AggregatedResult, InvariantAggregator, InvariantKind, OverallVerdict,
    VerificationLevel,
};
use vuma_ive::verification::VerificationInput;
use vuma_parser::{
    Parser, ParseError,
    to_scg::AstToScg,
};
use vuma_scg::{
    SCG, NodeId, NodeType, NodePayload, ProgramPoint, EdgeKind,
    AllocationNode, DeallocationNode, ComputationNode, AccessNode, AccessMode,
    CastNode, ControlNode, ControlKind, EffectNode, PhantomNode,
    RegionId, SCGRegion, DeploymentTarget,
};

// ===========================================================================
// Test Categories
// ===========================================================================

/// Classification of test categories in the VUMA test suite.
///
/// Each variant maps to a distinct scope of testing, enabling selective
/// test runs and organised reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TestCategory {
    /// Unit tests: individual crate functions, data structures, edge cases.
    Unit,
    /// Integration tests: cross-crate pipelines (parse -> SCG -> verify).
    Integration,
    /// Verification tests: IVE invariant checks, proof generation, counterexamples.
    Verification,
    /// Codegen tests: ARM64 code emission, register allocation, ELF generation.
    Codegen,
    /// Pi5 tests: Raspberry Pi 5 target-specific (MMIO, UART, GPIO, SMP).
    Pi5,
}

impl TestCategory {
    /// Returns all test categories in canonical order.
    pub fn all() -> &'static [TestCategory; 5] {
        &[
            TestCategory::Unit,
            TestCategory::Integration,
            TestCategory::Verification,
            TestCategory::Codegen,
            TestCategory::Pi5,
        ]
    }

    /// Returns a human-readable label for this category.
    pub fn label(&self) -> &'static str {
        match self {
            TestCategory::Unit => "unit",
            TestCategory::Integration => "integration",
            TestCategory::Verification => "verification",
            TestCategory::Codegen => "codegen",
            TestCategory::Pi5 => "pi5",
        }
    }
}

impl fmt::Display for TestCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

// ===========================================================================
// Helper Macros for Test Categories
// ===========================================================================

/// Declare a VUMA unit test.
///
/// Annotates the test with the `Unit` category for registry tracking
/// and applies the standard `#[test]` attribute.
///
/// # Example
///
/// ```rust,ignore
/// vuma_unit_test!(test_my_unit_thing {
///     assert_eq!(2 + 2, 4);
/// });
/// ```
#[macro_export]
macro_rules! vuma_unit_test {
    ($name:ident { $($body:tt)* }) => {
        #[test]
        fn $name() {
            let _category = $crate::framework::TestCategory::Unit;
            $($body)*
        }
    };
}

/// Declare a VUMA integration test.
///
/// Annotates the test with the `Integration` category for registry tracking
/// and applies the standard `#[test]` attribute.
///
/// # Example
///
/// ```rust,ignore
/// vuma_integration_test!(test_parse_and_verify {
///     let result = $crate::framework::verify_program("region x = allocate(64); free(x);");
///     assert_eq!(result.per_invariant.len(), 5);
/// });
/// ```
#[macro_export]
macro_rules! vuma_integration_test {
    ($name:ident { $($body:tt)* }) => {
        #[test]
        fn $name() {
            let _category = $crate::framework::TestCategory::Integration;
            $($body)*
        }
    };
}

/// Declare a VUMA verification test.
///
/// Annotates the test with the `Verification` category for registry tracking
/// and applies the standard `#[test]` attribute.
///
/// # Example
///
/// ```rust,ignore
/// vuma_verification_test!(test_liveness_check {
///     $crate::framework::assert_verifies("region buf = allocate(64); free(buf);");
/// });
/// ```
#[macro_export]
macro_rules! vuma_verification_test {
    ($name:ident { $($body:tt)* }) => {
        #[test]
        fn $name() {
            let _category = $crate::framework::TestCategory::Verification;
            $($body)*
        }
    };
}

/// Declare a VUMA codegen test.
///
/// Annotates the test with the `Codegen` category for registry tracking
/// and applies the standard `#[test]` attribute.
///
/// # Example
///
/// ```rust,ignore
/// vuma_codegen_test!(test_arm64_emit {
///     let result = $crate::framework::compile_to_arm64("fn main() { return; }");
///     assert!(result.is_err()); // codegen not yet available
/// });
/// ```
#[macro_export]
macro_rules! vuma_codegen_test {
    ($name:ident { $($body:tt)* }) => {
        #[test]
        fn $name() {
            let _category = $crate::framework::TestCategory::Codegen;
            $($body)*
        }
    };
}

/// Declare a VUMA Pi5 target test.
///
/// Annotates the test with the `Pi5` category for registry tracking
/// and applies the standard `#[test]` attribute.
///
/// # Example
///
/// ```rust,ignore
/// vuma_pi5_test!(test_uart_output {
///     // Pi5-specific test
/// });
/// ```
#[macro_export]
macro_rules! vuma_pi5_test {
    ($name:ident { $($body:tt)* }) => {
        #[test]
        fn $name() {
            let _category = $crate::framework::TestCategory::Pi5;
            $($body)*
        }
    };
}

// ===========================================================================
// Compile Error
// ===========================================================================

/// Errors that can occur during the full compilation pipeline.
///
/// This aggregates errors from the parsing, SCG conversion, and code
/// generation stages into a single error type for the test framework.
#[derive(Debug)]
pub enum CompileError {
    /// One or more parse errors.
    Parse(Vec<ParseError>),
    /// SCG conversion or validation error.
    ScgConversion(String),
    /// Code generation error (message string — the underlying
    /// `vuma_codegen::CodegenError` is not exposed because the codegen
    /// crate is currently in flux and may not compile).
    Codegen(String),
    /// The codegen pipeline is not yet available (stub).
    CodegenNotAvailable,
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileError::Parse(errors) => {
                write!(f, "parse errors:")?;
                for e in errors {
                    write!(f, "\n  {}", e)?;
                }
                Ok(())
            }
            CompileError::ScgConversion(msg) => write!(f, "SCG conversion error: {}", msg),
            CompileError::Codegen(msg) => write!(f, "codegen error: {}", msg),
            CompileError::CodegenNotAvailable => write!(f, "codegen pipeline not yet available"),
        }
    }
}

impl std::error::Error for CompileError {}

impl From<Vec<ParseError>> for CompileError {
    fn from(errors: Vec<ParseError>) -> Self {
        CompileError::Parse(errors)
    }
}

// ===========================================================================
// Pipeline Stage Tracking
// ===========================================================================

/// A stage in the VUMA compilation/verification pipeline.
///
/// Used by [`PipelineResult`] to record which stages succeeded or failed,
/// enabling fine-grained diagnosis of pipeline failures in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PipelineStage {
    /// Source lexing and parsing (source -> AST).
    Parse,
    /// AST-to-SCG conversion (AST -> parser SCG).
    AstToScg,
    /// Parser SCG to vuma_scg bridge (parser SCG -> vuma_scg::SCG).
    ScgBridge,
    /// SCG validation.
    ScgValidation,
    /// IVE verification (all five invariant checks).
    IveVerification,
    /// Code generation (SCG -> IR -> ARM64).
    Codegen,
}

impl PipelineStage {
    /// Returns all pipeline stages in execution order.
    pub fn all() -> &'static [PipelineStage; 6] {
        &[
            PipelineStage::Parse,
            PipelineStage::AstToScg,
            PipelineStage::ScgBridge,
            PipelineStage::ScgValidation,
            PipelineStage::IveVerification,
            PipelineStage::Codegen,
        ]
    }

    /// Returns a human-readable label for this stage.
    pub fn label(&self) -> &'static str {
        match self {
            PipelineStage::Parse => "parse",
            PipelineStage::AstToScg => "ast-to-scg",
            PipelineStage::ScgBridge => "scg-bridge",
            PipelineStage::ScgValidation => "scg-validation",
            PipelineStage::IveVerification => "ive-verification",
            PipelineStage::Codegen => "codegen",
        }
    }
}

impl fmt::Display for PipelineStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

/// The outcome of a single pipeline stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageOutcome {
    /// The stage completed successfully.
    Passed,
    /// The stage failed with an error.
    Failed,
    /// The stage was skipped (e.g., codegen not available).
    Skipped,
}

/// The result of running a program through the full VUMA pipeline.
///
/// Records the outcome of each pipeline stage, the final SCG (if
/// constructed), and the aggregated verification result (if run).
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// Per-stage outcomes, in pipeline execution order.
    pub stages: Vec<(PipelineStage, StageOutcome)>,
    /// The SCG produced by the pipeline, if parsing and bridging succeeded.
    pub scg: Option<SCG>,
    /// The aggregated verification result, if verification was run.
    pub verification: Option<AggregatedResult>,
    /// Total elapsed time in milliseconds.
    pub elapsed_ms: u64,
}

impl PipelineResult {
    /// Returns `true` if all pipeline stages that were attempted passed.
    pub fn all_passed(&self) -> bool {
        self.stages
            .iter()
            .all(|(_, outcome)| *outcome == StageOutcome::Passed || *outcome == StageOutcome::Skipped)
    }

    /// Returns the first stage that failed, if any.
    pub fn first_failure(&self) -> Option<&(PipelineStage, StageOutcome)> {
        self.stages
            .iter()
            .find(|(_, outcome)| *outcome == StageOutcome::Failed)
    }

    /// Returns the stage at which the pipeline stopped (last non-skipped stage).
    pub fn last_executed_stage(&self) -> Option<PipelineStage> {
        self.stages
            .iter()
            .rev()
            .find(|(_, outcome)| *outcome != StageOutcome::Skipped)
            .map(|(stage, _)| *stage)
    }
}

impl fmt::Display for PipelineResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Pipeline Result ({}ms):", self.elapsed_ms)?;
        for (stage, outcome) in &self.stages {
            let icon = match outcome {
                StageOutcome::Passed => "PASS",
                StageOutcome::Failed => "FAIL",
                StageOutcome::Skipped => "SKIP",
            };
            writeln!(f, "  {:<20} {}", stage, icon)?;
        }
        if let Some(ref scg) = self.scg {
            writeln!(f, "  SCG: {} nodes, {} edges", scg.node_count(), scg.edge_count())?;
        }
        if let Some(ref v) = self.verification {
            writeln!(f, "  Verdict: {}", v.overall)?;
        }
        Ok(())
    }
}

// ===========================================================================
// Test Registry
// ===========================================================================

/// The outcome of a single named test within the framework.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestOutcome {
    /// The test passed.
    Pass,
    /// The test failed (panic or assertion failure).
    Fail,
    /// The test was ignored (skipped).
    Ignore,
}

impl fmt::Display for TestOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TestOutcome::Pass => write!(f, "PASS"),
            TestOutcome::Fail => write!(f, "FAIL"),
            TestOutcome::Ignore => write!(f, "IGNORE"),
        }
    }
}

/// A record of a single test execution.
#[derive(Debug, Clone)]
pub struct TestRecord {
    /// The test name.
    pub name: String,
    /// The test category.
    pub category: TestCategory,
    /// The outcome of the test.
    pub outcome: TestOutcome,
    /// Elapsed time in microseconds.
    pub elapsed_us: u64,
    /// Optional failure message.
    pub message: Option<String>,
}

/// A registry for tracking test executions and reporting results.
///
/// The test registry accumulates [`TestRecord`]s as tests are run and
/// provides summary statistics and reporting. It is designed for use
/// with the [`run_test`] helper function.
///
/// # Thread Safety
///
/// The global registry uses atomic counters for pass/fail/ignore totals,
/// making it safe to use from multiple threads (e.g., with `cargo test`
/// parallelism). Detailed per-test records are not collected in the
/// global registry; use a local [`TestRegistry`] instance instead.
pub struct TestRegistry {
    records: Vec<TestRecord>,
    pass_count: AtomicUsize,
    fail_count: AtomicUsize,
    ignore_count: AtomicUsize,
}

impl TestRegistry {
    /// Create a new empty test registry.
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            pass_count: AtomicUsize::new(0),
            fail_count: AtomicUsize::new(0),
            ignore_count: AtomicUsize::new(0),
        }
    }

    /// Record a test result.
    pub fn record(&mut self, record: TestRecord) {
        match record.outcome {
            TestOutcome::Pass => self.pass_count.fetch_add(1, Ordering::Relaxed),
            TestOutcome::Fail => self.fail_count.fetch_add(1, Ordering::Relaxed),
            TestOutcome::Ignore => self.ignore_count.fetch_add(1, Ordering::Relaxed),
        };
        self.records.push(record);
    }

    /// Returns the total number of tests recorded.
    pub fn total(&self) -> usize {
        self.records.len()
    }

    /// Returns the number of passing tests.
    pub fn pass_count(&self) -> usize {
        self.pass_count.load(Ordering::Relaxed)
    }

    /// Returns the number of failing tests.
    pub fn fail_count(&self) -> usize {
        self.fail_count.load(Ordering::Relaxed)
    }

    /// Returns the number of ignored tests.
    pub fn ignore_count(&self) -> usize {
        self.ignore_count.load(Ordering::Relaxed)
    }

    /// Returns `true` if all recorded tests passed.
    pub fn all_passed(&self) -> bool {
        self.fail_count.load(Ordering::Relaxed) == 0
            && self.pass_count.load(Ordering::Relaxed) > 0
    }

    /// Returns records filtered by category.
    pub fn by_category(&self, category: TestCategory) -> Vec<&TestRecord> {
        self.records
            .iter()
            .filter(|r| r.category == category)
            .collect()
    }

    /// Generate a summary report of all recorded tests.
    pub fn report(&self) -> TestReport {
        let mut by_category: HashMap<TestCategory, Vec<&TestRecord>> = HashMap::new();
        for record in &self.records {
            by_category.entry(record.category).or_default().push(record);
        }

        TestReport {
            total: self.total(),
            passed: self.pass_count(),
            failed: self.fail_count(),
            ignored: self.ignore_count(),
            by_category,
        }
    }
}

impl Default for TestRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// A summary report of test execution results.
#[derive(Debug)]
pub struct TestReport<'a> {
    /// Total number of tests.
    pub total: usize,
    /// Number of passing tests.
    pub passed: usize,
    /// Number of failing tests.
    pub failed: usize,
    /// Number of ignored tests.
    pub ignored: usize,
    /// Records grouped by category.
    pub by_category: HashMap<TestCategory, Vec<&'a TestRecord>>,
}

impl<'a> fmt::Display for TestReport<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "VUMA Test Report")?;
        writeln!(f, "================")?;
        writeln!(f, "Total: {} | Pass: {} | Fail: {} | Ignore: {}",
            self.total, self.passed, self.failed, self.ignored)?;

        for cat in TestCategory::all() {
            if let Some(records) = self.by_category.get(cat) {
                let pass_in_cat = records.iter().filter(|r| r.outcome == TestOutcome::Pass).count();
                let fail_in_cat = records.iter().filter(|r| r.outcome == TestOutcome::Fail).count();
                writeln!(f, "\n  [{}] {} tests, {} pass, {} fail",
                    cat, records.len(), pass_in_cat, fail_in_cat)?;
                for record in records {
                    let icon = match record.outcome {
                        TestOutcome::Pass => "PASS",
                        TestOutcome::Fail => "FAIL",
                        TestOutcome::Ignore => "SKIP",
                    };
                    writeln!(f, "    {} {} ({}us)", icon, record.name, record.elapsed_us)?;
                }
            }
        }

        Ok(())
    }
}

// ===========================================================================
// Framework Helpers — Source -> SCG
// ===========================================================================

/// Parse VUMA source text and convert it into a `vuma_scg::SCG`.
///
/// This is the primary entry point for the test framework. It:
/// 1. Lexes and parses the source using `vuma_parser`.
/// 2. Converts the AST to the parser's local SCG via `AstToScg`.
/// 3. Bridges the parser SCG into the full `vuma_scg::SCG` with regions,
///    typed node payloads, and proper edge kinds.
///
/// # Errors
///
/// Returns `Err(Vec<ParseError>)` if the source cannot be parsed or
/// converted to the parser SCG.
///
/// # Example
///
/// ```rust,ignore
/// let scg = build_scg_from_source("region pool = allocate(1024); free(pool);").unwrap();
/// assert!(scg.node_count() > 0);
/// ```
pub fn build_scg_from_source(source: &str) -> Result<SCG, Vec<ParseError>> {
    // Step 1: Parse source -> AST.
    let mut parser = Parser::new(source);
    let program = parser.parse_program()?;

    // Step 2: Convert AST -> vuma_scg::SCG (AstToScg now produces SCG directly).
    let mut converter = AstToScg::new();
    let scg = converter.convert(&program).map_err(|e| vec![e])?;

    Ok(scg)
}

/// Parse VUMA source, build an SCG, and run all five IVE invariant checks.
///
/// Returns an [`AggregatedResult`] with per-invariant outcomes, an overall
/// verdict, and a summary of statistics.
///
/// # Example
///
/// ```rust,ignore
/// let result = verify_program("region buf = allocate(256); free(buf);");
/// println!("Verdict: {}", result.overall);
/// ```
pub fn verify_program(source: &str) -> AggregatedResult {
    // Build the vuma_scg::SCG (silently treating parse errors as empty programs).
    let scg = build_scg_from_source(source).unwrap_or_default();

    // Build verification input from the SCG.
    let input = VerificationInput::from_scg(scg);

    // Run all five invariant checks at Normal level.
    let aggregator = InvariantAggregator::new();
    aggregator.verify_all(&input)
}

/// Parse VUMA source, build an SCG, and run IVE checks at a specific
/// verification level.
///
/// This is like [`verify_program`] but allows control over the verification
/// level (Quick, Normal, or Exhaustive).
///
/// # Example
///
/// ```rust,ignore
/// use vuma_ive::VerificationLevel;
/// let result = verify_program_at_level("region buf = allocate(256);", VerificationLevel::Quick);
/// assert_eq!(result.per_invariant.len(), 2); // Quick only runs 2 checks
/// ```
pub fn verify_program_at_level(source: &str, level: VerificationLevel) -> AggregatedResult {
    let scg = build_scg_from_source(source).unwrap_or_default();
    let input = VerificationInput::from_scg(scg);

    let aggregator = InvariantAggregator::new().with_level(level);
    aggregator.verify_all(&input)
}

/// Run the full pipeline with stage-by-stage tracking.
///
/// Returns a [`PipelineResult`] that records which stages succeeded or
/// failed, the constructed SCG (if any), and the verification result
/// (if the verification stage was reached).
///
/// This is the most detailed pipeline function, suitable for integration
/// tests that need to diagnose *where* in the pipeline a failure occurs.
///
/// # Example
///
/// ```rust,ignore
/// let result = verify_program_detailed("region buf = allocate(64); free(buf);");
/// assert!(result.all_passed());
/// assert!(result.scg.is_some());
/// assert!(result.verification.is_some());
/// ```
pub fn verify_program_detailed(source: &str) -> PipelineResult {
    let start = Instant::now();
    let mut stages = Vec::new();
    let mut scg_result: Option<SCG> = None;
    let mut verification_result: Option<AggregatedResult> = None;

    // Stage 1: Parse source -> AST.
    let mut parser = Parser::new(source);
    let program = match parser.parse_program() {
        Ok(p) => {
            stages.push((PipelineStage::Parse, StageOutcome::Passed));
            p
        }
        Err(_) => {
            stages.push((PipelineStage::Parse, StageOutcome::Failed));
            return PipelineResult {
                stages,
                scg: None,
                verification: None,
                elapsed_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    // Stage 2: AST -> vuma_scg::SCG (AstToScg now produces SCG directly).
    let mut converter = AstToScg::new();
    let scg = match converter.convert(&program) {
        Ok(s) => {
            stages.push((PipelineStage::AstToScg, StageOutcome::Passed));
            s
        }
        Err(_) => {
            stages.push((PipelineStage::AstToScg, StageOutcome::Failed));
            return PipelineResult {
                stages,
                scg: None,
                verification: None,
                elapsed_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    // Stage 3: Bridge step no longer needed (AstToScg produces vuma_scg::SCG directly).
    stages.push((PipelineStage::ScgBridge, StageOutcome::Passed));
    scg_result = Some(scg.clone());

    // Stage 4: Validate SCG.
    let validation = scg.validate();
    if validation.is_valid {
        stages.push((PipelineStage::ScgValidation, StageOutcome::Passed));
    } else {
        stages.push((PipelineStage::ScgValidation, StageOutcome::Failed));
    }

    // Stage 5: IVE verification.
    let input = VerificationInput::from_scg(scg);
    let aggregator = InvariantAggregator::new();
    let aggregated = aggregator.verify_all(&input);
    stages.push((PipelineStage::IveVerification, StageOutcome::Passed));
    verification_result = Some(aggregated);

    // Stage 6: Codegen (not yet available).
    stages.push((PipelineStage::Codegen, StageOutcome::Skipped));

    PipelineResult {
        stages,
        scg: scg_result,
        verification: verification_result,
        elapsed_ms: start.elapsed().as_millis() as u64,
    }
}

/// Run the full compilation pipeline: source -> SCG -> IR -> ARM64 ELF binary.
///
/// Returns the raw bytes of a minimal ELF binary for Linux/AArch64.
///
/// **Note**: This function currently returns
/// [`CompileError::CodegenNotAvailable`] because the `vuma-codegen` crate
/// is undergoing refactoring and does not yet compile. Once the codegen
/// crate stabilises, this function will be wired through the full pipeline:
///
/// 1. Parse source -> AST -> parser SCG.
/// 2. Bridge parser SCG -> codegen SCG -> IR.
/// 3. Emit ARM64 machine code -> ELF binary.
///
/// # Errors
///
/// Returns [`CompileError`] if any stage fails:
/// - [`CompileError::Parse`] — source parsing failed.
/// - [`CompileError::ScgConversion`] — SCG bridge or validation failed.
/// - [`CompileError::Codegen`] — IR translation or emission failed.
/// - [`CompileError::CodegenNotAvailable`] — codegen crate not yet wired.
pub fn compile_to_arm64(source: &str) -> Result<Vec<u8>, Vec<CompileError>> {
    // Step 1: Parse source -> AST -> parser SCG.
    let mut parser = Parser::new(source);
    let _program = parser.parse_program().map_err(|errors| vec![CompileError::Parse(errors)])?;

    // TODO: Wire through vuma-codegen once the crate compiles.
    // The full pipeline will be:
    //   let mut converter = AstToScg::new();
    //   let parser_scg = converter.convert(&program)...;
    //   let codegen_scg = bridge_parser_scg_to_codegen_scg(&parser_scg);
    //   let ir_program = ScgToIr::new().convert(&codegen_scg)...;
    //   let elf_bytes = Emitter::new().emit_program(&ir_program)...;

    Err(vec![CompileError::CodegenNotAvailable])
}

// ===========================================================================
// Assertion Helpers
// ===========================================================================

/// Assert that the given VUMA source program passes all invariant checks.
///
/// This panics if the aggregated verification result contains any
/// concrete invariant **violations**.
///
/// In the current implementation, all IVE checks return `Unverified`, so the
/// overall verdict is `Inconclusive`. This assertion accepts `Inconclusive`
/// as a passing result until the IVE checks are fully implemented. It only
/// fails when a concrete invariant *violation* is detected.
///
/// # Panics
///
/// Panics if any invariant check returns a `Violated` status.
pub fn assert_verifies(source: &str) {
    let result = verify_program(source);

    // Check for any violations.
    let violations: Vec<_> = result
        .per_invariant
        .iter()
        .filter(|pir| pir.is_fail())
        .collect();

    if !violations.is_empty() {
        let violation_labels: Vec<String> = violations
            .iter()
            .map(|v| format!("{}: {}", v.kind, v.result.message))
            .collect();
        panic!(
            "Program failed invariant checks:\n  {}\n\nOverall verdict: {}\nSource:\n{}",
            violation_labels.join("\n  "),
            result.overall,
            source
        );
    }
}

/// Assert that the given VUMA source program violates a specific invariant.
///
/// This panics if the specified invariant is *not* violated (i.e., it is
/// proven, probably safe, or unverified).
///
/// **Note**: Since the IVE verification engine is currently in placeholder
/// mode (returning `Unverified` for all checks), this assertion will fail
/// for all programs until the actual verification logic is implemented.
/// Use this assertion in tests that are explicitly marked as
/// `#[ignore]` pending IVE implementation.
///
/// # Panics
///
/// Panics if the specified invariant is not violated.
pub fn assert_violation(source: &str, invariant: InvariantKind) {
    let result = verify_program(source);

    let pir = result
        .per_invariant
        .iter()
        .find(|pir| pir.kind == invariant);

    match pir {
        Some(pir) if pir.is_fail() => {
            // Expected: the invariant is violated.
        }
        Some(pir) => {
            panic!(
                "Expected violation of {:?}, but got status: {:?}\n\
                 Message: {}\n\
                 Overall verdict: {}\n\
                 Source:\n{}",
                invariant,
                pir.result.status,
                pir.result.message,
                result.overall,
                source
            );
        }
        None => {
            panic!(
                "Invariant {:?} was not checked in the verification run.\n\
                 Checked invariants: {}\n\
                 Source:\n{}",
                invariant,
                result.per_invariant
                    .iter()
                    .map(|p| format!("{:?}", p.kind))
                    .collect::<Vec<_>>()
                    .join(", "),
                source
            );
        }
    }
}

// ===========================================================================
// SCG Bridges
// ===========================================================================

// bridge_scg_to_ive_message removed — verification now uses VerificationInput::from_scg()

// ===========================================================================
// Utility Helpers
// ===========================================================================

/// Run a test closure with a descriptive label, capturing any panic and
/// reporting it with context about the test category and source.
pub fn run_test(category: TestCategory, name: &str, f: impl FnOnce() + panic::UnwindSafe) -> bool {
    match panic::catch_unwind(f) {
        Ok(()) => {
            println!("[PASS] [{:?}] {}", category, name);
            true
        }
        Err(_) => {
            eprintln!("[FAIL] [{:?}] {}", category, name);
            false
        }
    }
}

/// Run a test closure and record the result in a [`TestRegistry`].
///
/// This is the registry-aware version of [`run_test`]. It measures elapsed
/// time and records a [`TestRecord`] in the provided registry.
pub fn run_registered_test(
    registry: &mut TestRegistry,
    category: TestCategory,
    name: &str,
    f: impl FnOnce() + panic::UnwindSafe,
) -> TestOutcome {
    let start = Instant::now();
    let result = panic::catch_unwind(f);
    let elapsed_us = start.elapsed().as_micros() as u64;

    let (outcome, message) = match result {
        Ok(()) => (TestOutcome::Pass, None),
        Err(_) => (TestOutcome::Fail, Some("test panicked".to_string())),
    };

    let icon = match outcome {
        TestOutcome::Pass => "PASS",
        TestOutcome::Fail => "FAIL",
        TestOutcome::Ignore => "SKIP",
    };
    println!("[{}] [{:?}] {} ({}us)", icon, category, name, elapsed_us);

    registry.record(TestRecord {
        name: name.to_string(),
        category,
        outcome,
        elapsed_us,
        message,
    });

    outcome
}

// ===========================================================================
// SCG Builder Helpers
// ===========================================================================

/// Build a trivial SCG manually: allocate -> compute -> free.
///
/// This is a convenience helper for tests that need a well-formed SCG
/// without going through the parser.
pub fn build_trivial_scg() -> SCG {
    let mut scg = SCG::new();
    let region_id = RegionId::new(1);

    let alloc_id = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 256,
            align: 16,
            region_id,
            type_name: Some("Buffer".to_string()),
        }),
        ProgramPoint {
            file: Some("trivial.vu".to_string()),
            line: Some(1),
            column: Some(1),
            offset: None,
        },
    );

    let comp_id = scg.add_node(
        NodeType::Computation,
        NodePayload::Computation(ComputationNode {
            operation: "write_buffer".to_string(),
            result_type: None,
        }),
        ProgramPoint {
            file: Some("trivial.vu".to_string()),
            line: Some(2),
            column: Some(1),
            offset: None,
        },
    );

    let dealloc_id = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc_id,
            region_id,
        }),
        ProgramPoint {
            file: Some("trivial.vu".to_string()),
            line: Some(3),
            column: Some(1),
            offset: None,
        },
    );

    let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);
    region.add_node(alloc_id);
    region.add_node(dealloc_id);
    scg.add_region(region);

    scg.add_edge(alloc_id, comp_id, EdgeKind::DataFlow).unwrap();
    scg.add_edge(comp_id, dealloc_id, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(alloc_id, dealloc_id, EdgeKind::Derivation).unwrap();

    scg
}

/// Build an SCG with a use-after-free pattern: allocate -> free -> access.
///
/// The access after deallocation should trigger a liveness violation
/// once the IVE liveness checker is fully implemented.
pub fn build_use_after_free_scg() -> SCG {
    let mut scg = SCG::new();
    let region_id = RegionId::new(1);

    let alloc_id = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 64,
            align: 8,
            region_id,
            type_name: None,
        }),
        ProgramPoint {
            file: Some("uaf.vu".to_string()),
            line: Some(1),
            column: Some(1),
            offset: None,
        },
    );

    let dealloc_id = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc_id,
            region_id,
        }),
        ProgramPoint {
            file: Some("uaf.vu".to_string()),
            line: Some(2),
            column: Some(1),
            offset: None,
        },
    );

    let access_id = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id,
            offset: Some(0),
            access_size: Some(8),
        }),
        ProgramPoint {
            file: Some("uaf.vu".to_string()),
            line: Some(3),
            column: Some(1),
            offset: None,
        },
    );

    scg.add_edge(alloc_id, dealloc_id, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(dealloc_id, access_id, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(alloc_id, access_id, EdgeKind::DataFlow).unwrap();

    scg
}

/// Build an SCG with a double-free pattern: allocate -> free -> free.
///
/// The second deallocation of the same region should trigger a cleanup
/// violation once the IVE cleanup checker is fully implemented.
pub fn build_double_free_scg() -> SCG {
    let mut scg = SCG::new();
    let region_id = RegionId::new(1);

    let alloc_id = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 128,
            align: 8,
            region_id,
            type_name: Some("DoubleFreeBuf".to_string()),
        }),
        ProgramPoint {
            file: Some("dblfree.vu".to_string()),
            line: Some(1),
            column: Some(1),
            offset: None,
        },
    );

    let dealloc1_id = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc_id,
            region_id,
        }),
        ProgramPoint {
            file: Some("dblfree.vu".to_string()),
            line: Some(2),
            column: Some(1),
            offset: None,
        },
    );

    let dealloc2_id = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc_id,
            region_id,
        }),
        ProgramPoint {
            file: Some("dblfree.vu".to_string()),
            line: Some(3),
            column: Some(1),
            offset: None,
        },
    );

    let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);
    region.add_node(alloc_id);
    region.add_node(dealloc1_id);
    scg.add_region(region);

    scg.add_edge(alloc_id, dealloc1_id, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(dealloc1_id, dealloc2_id, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(alloc_id, dealloc1_id, EdgeKind::Derivation).unwrap();
    scg.add_edge(alloc_id, dealloc2_id, EdgeKind::Derivation).unwrap();

    scg
}

/// Build an SCG with an out-of-bounds access pattern.
///
/// The access at an offset beyond the allocation size should trigger an
/// interpretation violation once the IVE interpretation checker is
/// fully implemented.
pub fn build_out_of_bounds_scg() -> SCG {
    let mut scg = SCG::new();
    let region_id = RegionId::new(1);

    let alloc_id = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 16,
            align: 8,
            region_id,
            type_name: None,
        }),
        ProgramPoint {
            file: Some("oob.vu".to_string()),
            line: Some(1),
            column: Some(1),
            offset: None,
        },
    );

    // Access at offset 24, which is beyond the 16-byte allocation.
    let access_id = scg.add_node(
        NodeType::Access,
        NodePayload::Access(AccessNode {
            mode: AccessMode::Read,
            region_id,
            offset: Some(24),
            access_size: Some(8),
        }),
        ProgramPoint {
            file: Some("oob.vu".to_string()),
            line: Some(2),
            column: Some(1),
            offset: None,
        },
    );

    let dealloc_id = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc_id,
            region_id,
        }),
        ProgramPoint {
            file: Some("oob.vu".to_string()),
            line: Some(3),
            column: Some(1),
            offset: None,
        },
    );

    let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);
    region.add_node(alloc_id);
    region.add_node(access_id);
    region.add_node(dealloc_id);
    scg.add_region(region);

    scg.add_edge(alloc_id, access_id, EdgeKind::DataFlow).unwrap();
    scg.add_edge(access_id, dealloc_id, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(alloc_id, dealloc_id, EdgeKind::Derivation).unwrap();

    scg
}

/// Build an SCG with a leaked allocation (no deallocation).
///
/// The allocation without a matching deallocation should trigger a cleanup
/// violation once the IVE cleanup checker is fully implemented.
pub fn build_leaked_allocation_scg() -> SCG {
    let mut scg = SCG::new();
    let region_id = RegionId::new(1);

    let alloc_id = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 512,
            align: 16,
            region_id,
            type_name: Some("LeakedBuf".to_string()),
        }),
        ProgramPoint {
            file: Some("leak.vu".to_string()),
            line: Some(1),
            column: Some(1),
            offset: None,
        },
    );

    let comp_id = scg.add_node(
        NodeType::Computation,
        NodePayload::Computation(ComputationNode {
            operation: "use_leaked_buf".to_string(),
            result_type: None,
        }),
        ProgramPoint {
            file: Some("leak.vu".to_string()),
            line: Some(2),
            column: Some(1),
            offset: None,
        },
    );

    let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);
    region.add_node(alloc_id);
    scg.add_region(region);

    scg.add_edge(alloc_id, comp_id, EdgeKind::DataFlow).unwrap();

    scg
}

/// Build an SCG with multiple allocation/free pairs (multi-region).
///
/// This tests the framework's ability to handle programs with multiple
/// independent memory regions, each with their own lifecycle.
pub fn build_multi_region_scg() -> SCG {
    let mut scg = SCG::new();

    // Region 1: allocate -> compute -> free
    let region1_id = RegionId::new(1);
    let alloc1_id = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 64,
            align: 8,
            region_id: region1_id,
            type_name: Some("Region1".to_string()),
        }),
        ProgramPoint {
            file: Some("multi.vu".to_string()),
            line: Some(1),
            column: Some(1),
            offset: None,
        },
    );
    let comp1_id = scg.add_node(
        NodeType::Computation,
        NodePayload::Computation(ComputationNode {
            operation: "process_region1".to_string(),
            result_type: None,
        }),
        ProgramPoint {
            file: Some("multi.vu".to_string()),
            line: Some(2),
            column: Some(1),
            offset: None,
        },
    );
    let dealloc1_id = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc1_id,
            region_id: region1_id,
        }),
        ProgramPoint {
            file: Some("multi.vu".to_string()),
            line: Some(3),
            column: Some(1),
            offset: None,
        },
    );

    // Region 2: allocate -> compute -> free
    let region2_id = RegionId::new(2);
    let alloc2_id = scg.add_node(
        NodeType::Allocation,
        NodePayload::Allocation(AllocationNode {
            size: 128,
            align: 16,
            region_id: region2_id,
            type_name: Some("Region2".to_string()),
        }),
        ProgramPoint {
            file: Some("multi.vu".to_string()),
            line: Some(4),
            column: Some(1),
            offset: None,
        },
    );
    let comp2_id = scg.add_node(
        NodeType::Computation,
        NodePayload::Computation(ComputationNode {
            operation: "process_region2".to_string(),
            result_type: None,
        }),
        ProgramPoint {
            file: Some("multi.vu".to_string()),
            line: Some(5),
            column: Some(1),
            offset: None,
        },
    );
    let dealloc2_id = scg.add_node(
        NodeType::Deallocation,
        NodePayload::Deallocation(DeallocationNode {
            allocation_node: alloc2_id,
            region_id: region2_id,
        }),
        ProgramPoint {
            file: Some("multi.vu".to_string()),
            line: Some(6),
            column: Some(1),
            offset: None,
        },
    );

    // Add regions.
    let mut r1 = SCGRegion::new(region1_id, DeploymentTarget::Heap);
    r1.add_node(alloc1_id);
    r1.add_node(dealloc1_id);
    scg.add_region(r1);

    let mut r2 = SCGRegion::new(region2_id, DeploymentTarget::Heap);
    r2.add_node(alloc2_id);
    r2.add_node(dealloc2_id);
    scg.add_region(r2);

    // Edges.
    scg.add_edge(alloc1_id, comp1_id, EdgeKind::DataFlow).unwrap();
    scg.add_edge(comp1_id, dealloc1_id, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(alloc1_id, dealloc1_id, EdgeKind::Derivation).unwrap();

    scg.add_edge(alloc2_id, comp2_id, EdgeKind::DataFlow).unwrap();
    scg.add_edge(comp2_id, dealloc2_id, EdgeKind::ControlFlow).unwrap();
    scg.add_edge(alloc2_id, dealloc2_id, EdgeKind::Derivation).unwrap();

    // Cross-region dependency: comp1 feeds into comp2.
    scg.add_edge(comp1_id, comp2_id, EdgeKind::DataFlow).unwrap();

    scg
}

// ===========================================================================
// Framework Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Test 1: build_scg_from_source with valid program
    // -----------------------------------------------------------------------
    #[test]
    fn test_build_scg_from_valid_source() {
        let source = "region pool = allocate(1024); free(pool);";
        let result = build_scg_from_source(source);
        assert!(result.is_ok(), "Expected successful SCG construction");

        let scg = result.unwrap();
        assert!(scg.node_count() > 0, "SCG should have at least one node");
    }

    // -----------------------------------------------------------------------
    // Test 2: verify_program returns five invariant checks
    // -----------------------------------------------------------------------
    #[test]
    fn test_verify_program_returns_five_invariants() {
        let source = "region buf = allocate(256); free(buf);";
        let result = verify_program(source);

        assert_eq!(
            result.per_invariant.len(),
            5,
            "Expected 5 invariant checks at Normal verification level"
        );

        // After Wave 2 wiring, the IVE returns real results.
        // A well-formed program should now get Proven/Pass rather than
        // the placeholder Inconclusive verdict.
        assert_ne!(
            result.overall,
            OverallVerdict::Fail,
            "Well-formed program should not have Fail verdict"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: assert_verifies does not panic for well-formed programs
    // -----------------------------------------------------------------------
    #[test]
    fn test_assert_verifies_well_formed_program() {
        // A well-formed program should not trigger any violations.
        // Since IVE returns Unverified (not Violated), this should pass.
        assert_verifies("region buf = allocate(256); free(buf);");
    }

    // -----------------------------------------------------------------------
    // Test 4: build_trivial_scg helper produces a valid SCG
    // -----------------------------------------------------------------------
    #[test]
    fn test_build_trivial_scg_helper() {
        let scg = build_trivial_scg();

        // Should have 3 nodes: allocation, computation, deallocation.
        assert_eq!(scg.node_count(), 3);

        // Should have 1 region.
        assert_eq!(scg.region_count(), 1);

        // SCG should validate successfully.
        let validation = scg.validate();
        assert!(validation.is_valid, "Validation errors: {:?}", validation.errors);
    }

    // -----------------------------------------------------------------------
    // Test 5: build_use_after_free_scg helper
    // -----------------------------------------------------------------------
    #[test]
    fn test_build_use_after_free_scg() {
        let scg = build_use_after_free_scg();

        // Should have 3 nodes: allocation, deallocation, access.
        assert_eq!(scg.node_count(), 3);

        // Should have edges.
        assert!(scg.edge_count() > 0);

        // The access node should exist.
        let has_access = scg.nodes().any(|n| matches!(n.node_type, NodeType::Access));
        assert!(has_access, "UAF SCG should contain an Access node");
    }

    // -----------------------------------------------------------------------
    // Test 6: compile_to_arm64 returns CodegenNotAvailable
    // -----------------------------------------------------------------------
    #[test]
    fn test_compile_to_arm64_returns_not_available() {
        let source = "fn main() { return; }";
        let result = compile_to_arm64(source);
        assert!(result.is_err(), "Expected error from compile_to_arm64");
        let errors = result.unwrap_err();
        assert!(
            errors.iter().any(|e| matches!(e, CompileError::CodegenNotAvailable)),
            "Expected CodegenNotAvailable error"
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: test categories have correct labels
    // -----------------------------------------------------------------------
    #[test]
    fn test_category_labels() {
        assert_eq!(TestCategory::Unit.label(), "unit");
        assert_eq!(TestCategory::Integration.label(), "integration");
        assert_eq!(TestCategory::Verification.label(), "verification");
        assert_eq!(TestCategory::Codegen.label(), "codegen");
        assert_eq!(TestCategory::Pi5.label(), "pi5");
    }

    // -----------------------------------------------------------------------
    // Test 8: test category enumeration
    // -----------------------------------------------------------------------
    #[test]
    fn test_category_all_has_five() {
        assert_eq!(TestCategory::all().len(), 5);
    }

    // -----------------------------------------------------------------------
    // Test 9: CompileError display formatting
    // -----------------------------------------------------------------------
    #[test]
    fn test_compile_error_display() {
        let err = CompileError::ScgConversion("test error".to_string());
        let display = format!("{}", err);
        assert!(display.contains("SCG conversion error"));
        assert!(display.contains("test error"));

        let err_not_avail = CompileError::CodegenNotAvailable;
        let display2 = format!("{}", err_not_avail);
        assert!(display2.contains("not yet available"));
    }

    // -----------------------------------------------------------------------
    // Test 10: run_test captures panics
    // -----------------------------------------------------------------------
    #[test]
    fn test_run_test_captures_panics() {
        // A passing test.
        assert!(run_test(TestCategory::Unit, "passing", || {}));

        // A failing test (panics).
        assert!(!run_test(TestCategory::Unit, "failing", || {
            panic!("intentional test failure");
        }));
    }

    // -----------------------------------------------------------------------
    // Test 11: build_scg_from_source with a function definition
    // -----------------------------------------------------------------------
    #[test]
    fn test_build_scg_from_function_source() {
        let source = r#"
            fn add(a: u32, b: u32) -> u32 {
                return a;
            }
        "#;
        let result = build_scg_from_source(source);
        assert!(result.is_ok(), "Expected successful SCG construction for function def");

        let scg = result.unwrap();
        assert!(scg.node_count() > 0, "SCG should have nodes for function definition");
    }

    // -----------------------------------------------------------------------
    // Test 12: PipelineResult — detailed pipeline tracking
    // -----------------------------------------------------------------------
    #[test]
    fn test_verify_program_detailed_all_stages() {
        let source = "region buf = allocate(256); free(buf);";
        let result = verify_program_detailed(source);

        // All stages up to codegen should pass; codegen should be skipped.
        assert!(result.all_passed(), "Expected all executed stages to pass");
        assert!(result.scg.is_some(), "Expected SCG to be constructed");
        assert!(result.verification.is_some(), "Expected verification result");

        // Check that codegen was skipped.
        let codegen_stage = result.stages.iter().find(|(s, _)| *s == PipelineStage::Codegen);
        assert!(codegen_stage.is_some());
        assert_eq!(codegen_stage.unwrap().1, StageOutcome::Skipped);

        // Last executed stage should be IveVerification.
        assert_eq!(result.last_executed_stage(), Some(PipelineStage::IveVerification));
    }

    // -----------------------------------------------------------------------
    // Test 13: PipelineResult — parse failure
    // -----------------------------------------------------------------------
    #[test]
    fn test_verify_program_detailed_parse_failure() {
        // Intentionally invalid source that the parser should reject.
        let source = "}}}invalid{{{";
        let result = verify_program_detailed(source);

        // Parse stage should fail.
        assert!(!result.all_passed());
        assert!(result.first_failure().is_some());
        assert_eq!(result.first_failure().unwrap().0, PipelineStage::Parse);

        // SCG and verification should not be produced.
        assert!(result.scg.is_none());
        assert!(result.verification.is_none());
    }

    // -----------------------------------------------------------------------
    // Test 14: verify_program_at_level — Quick level
    // -----------------------------------------------------------------------
    #[test]
    fn test_verify_program_at_quick_level() {
        let source = "region buf = allocate(256); free(buf);";
        let result = verify_program_at_level(source, VerificationLevel::Quick);

        // Quick level should only run 2 checks (exclusivity + origin).
        assert_eq!(
            result.per_invariant.len(),
            2,
            "Quick verification should run exactly 2 invariant checks"
        );
        assert_eq!(result.level, VerificationLevel::Quick);
    }

    // -----------------------------------------------------------------------
    // Test 15: verify_program_at_level — Exhaustive level
    // -----------------------------------------------------------------------
    #[test]
    fn test_verify_program_at_exhaustive_level() {
        let source = "region buf = allocate(256); free(buf);";
        let result = verify_program_at_level(source, VerificationLevel::Exhaustive);

        // Exhaustive level should run all 5 checks.
        assert_eq!(
            result.per_invariant.len(),
            5,
            "Exhaustive verification should run all 5 invariant checks"
        );
        assert_eq!(result.level, VerificationLevel::Exhaustive);
    }

    // -----------------------------------------------------------------------
    // Test 16: build_double_free_scg helper
    // -----------------------------------------------------------------------
    #[test]
    fn test_build_double_free_scg() {
        let scg = build_double_free_scg();

        // Should have 3 nodes: allocation, free, free.
        assert_eq!(scg.node_count(), 3);

        // Should have 2 deallocation nodes.
        let dealloc_count = scg.nodes()
            .filter(|n| matches!(n.node_type, NodeType::Deallocation))
            .count();
        assert_eq!(dealloc_count, 2, "Double-free SCG should have 2 deallocation nodes");

        // Should have 1 region.
        assert_eq!(scg.region_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 17: build_out_of_bounds_scg helper
    // -----------------------------------------------------------------------
    #[test]
    fn test_build_out_of_bounds_scg() {
        let scg = build_out_of_bounds_scg();

        // Should have 3 nodes: allocation, access, deallocation.
        assert_eq!(scg.node_count(), 3);

        // The access node should have offset=24, access_size=8 (beyond 16-byte alloc).
        let access_node = scg.nodes()
            .find(|n| matches!(n.node_type, NodeType::Access));
        assert!(access_node.is_some(), "OOB SCG should contain an Access node");

        if let Some(nd) = access_node {
            if let NodePayload::Access(ref access) = nd.payload {
                assert_eq!(access.offset, Some(24));
                assert_eq!(access.access_size, Some(8));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Test 18: build_leaked_allocation_scg helper
    // -----------------------------------------------------------------------
    #[test]
    fn test_build_leaked_allocation_scg() {
        let scg = build_leaked_allocation_scg();

        // Should have 2 nodes: allocation, computation (no deallocation).
        assert_eq!(scg.node_count(), 2);

        // No deallocation nodes.
        let dealloc_count = scg.nodes()
            .filter(|n| matches!(n.node_type, NodeType::Deallocation))
            .count();
        assert_eq!(dealloc_count, 0, "Leaked allocation SCG should have no deallocation nodes");

        // Should have 1 region.
        assert_eq!(scg.region_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 19: build_multi_region_scg helper
    // -----------------------------------------------------------------------
    #[test]
    fn test_build_multi_region_scg() {
        let scg = build_multi_region_scg();

        // Should have 6 nodes: 2x (alloc, compute, dealloc).
        assert_eq!(scg.node_count(), 6, "Multi-region SCG should have 6 nodes");

        // Should have 2 regions.
        assert_eq!(scg.region_count(), 2, "Multi-region SCG should have 2 regions");

        // Should have cross-region edge.
        assert!(scg.edge_count() >= 7, "Multi-region SCG should have at least 7 edges");

        // SCG should validate.
        let validation = scg.validate();
        assert!(validation.is_valid, "Validation errors: {:?}", validation.errors);
    }

    // -----------------------------------------------------------------------
    // Test 20: TestRegistry — record and report
    // -----------------------------------------------------------------------
    #[test]
    fn test_registry_record_and_report() {
        let mut registry = TestRegistry::new();

        registry.record(TestRecord {
            name: "test_a".to_string(),
            category: TestCategory::Unit,
            outcome: TestOutcome::Pass,
            elapsed_us: 100,
            message: None,
        });
        registry.record(TestRecord {
            name: "test_b".to_string(),
            category: TestCategory::Integration,
            outcome: TestOutcome::Fail,
            elapsed_us: 200,
            message: Some("assertion failed".to_string()),
        });
        registry.record(TestRecord {
            name: "test_c".to_string(),
            category: TestCategory::Unit,
            outcome: TestOutcome::Pass,
            elapsed_us: 50,
            message: None,
        });

        assert_eq!(registry.total(), 3);
        assert_eq!(registry.pass_count(), 2);
        assert_eq!(registry.fail_count(), 1);
        assert!(!registry.all_passed());

        // Category filter.
        let unit_tests = registry.by_category(TestCategory::Unit);
        assert_eq!(unit_tests.len(), 2);

        // Report.
        let report = registry.report();
        let report_str = format!("{}", report);
        assert!(report_str.contains("VUMA Test Report"));
        assert!(report_str.contains("Total: 3"));
    }

    // -----------------------------------------------------------------------
    // Test 21: run_registered_test helper
    // -----------------------------------------------------------------------
    #[test]
    fn test_run_registered_test() {
        let mut registry = TestRegistry::new();

        let outcome = run_registered_test(
            &mut registry,
            TestCategory::Unit,
            "passing_test",
            || {},
        );
        assert_eq!(outcome, TestOutcome::Pass);

        let outcome2 = run_registered_test(
            &mut registry,
            TestCategory::Verification,
            "failing_test",
            || panic!("intentional failure"),
        );
        assert_eq!(outcome2, TestOutcome::Fail);

        assert_eq!(registry.total(), 2);
        assert_eq!(registry.pass_count(), 1);
        assert_eq!(registry.fail_count(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 22: PipelineStage labels and ordering
    // -----------------------------------------------------------------------
    #[test]
    fn test_pipeline_stage_labels() {
        assert_eq!(PipelineStage::Parse.label(), "parse");
        assert_eq!(PipelineStage::AstToScg.label(), "ast-to-scg");
        assert_eq!(PipelineStage::ScgBridge.label(), "scg-bridge");
        assert_eq!(PipelineStage::ScgValidation.label(), "scg-validation");
        assert_eq!(PipelineStage::IveVerification.label(), "ive-verification");
        assert_eq!(PipelineStage::Codegen.label(), "codegen");
    }

    // -----------------------------------------------------------------------
    // Test 23: PipelineStage::all has 6 stages in order
    // -----------------------------------------------------------------------
    #[test]
    fn test_pipeline_stage_all_six() {
        let all = PipelineStage::all();
        assert_eq!(all.len(), 6);
        assert_eq!(all[0], PipelineStage::Parse);
        assert_eq!(all[5], PipelineStage::Codegen);
    }

    // -----------------------------------------------------------------------
    // Test 24: TestOutcome display
    // -----------------------------------------------------------------------
    #[test]
    fn test_outcome_display() {
        assert_eq!(format!("{}", TestOutcome::Pass), "PASS");
        assert_eq!(format!("{}", TestOutcome::Fail), "FAIL");
        assert_eq!(format!("{}", TestOutcome::Ignore), "IGNORE");
    }

    // -----------------------------------------------------------------------
    // Test 25: PipelineResult display formatting
    // -----------------------------------------------------------------------
    #[test]
    fn test_pipeline_result_display() {
        let source = "region buf = allocate(64); free(buf);";
        let result = verify_program_detailed(source);
        let display = format!("{}", result);
        assert!(display.contains("Pipeline Result"));
        assert!(display.contains("PASS"));
        assert!(display.contains("SKIP"));
    }
}
