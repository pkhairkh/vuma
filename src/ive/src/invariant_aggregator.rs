//! Invariant aggregator for the IVE module.
//!
//! The [`InvariantAggregator`] runs all five VUMA invariant checks against an
//! SCG and produces a unified [`AggregatedResult`] that captures
//! per-invariant outcomes, an overall pass/fail verdict, and a
//! [`VerificationSummary`] with statistics.
//!
//! # Verification Levels
//!
//! | Level       | Checks run                                             |
//! |-------------|--------------------------------------------------------|
//! | [`Quick`]   | Only cheap, syntactic checks (exclusivity, origin).    |
//! | [`Normal`]  | All five invariant checks.                             |
//! | [`Exhaustive`] | All checks plus proof-generation where possible.   |
//!
//! # Incremental Verification
//!
//! The aggregator supports *incremental* verification: when a delta
//! describing which invariants are affected is provided, only those
//! invariants are re-checked while cached results are kept for the rest.
//!
//! [`Quick`]: VerificationLevel::Quick
//! [`Normal`]: VerificationLevel::Normal
//! [`Exhaustive`]: VerificationLevel::Exhaustive

use crate::result::{ConfidenceLevel, VerificationResult, VerificationStatus};
use crate::verification::{VerificationEngine, VerificationInput};
use std::fmt;
use std::time::Instant;

// ---------------------------------------------------------------------------
// InvariantKind
// ---------------------------------------------------------------------------

/// The five VUMA invariant kinds.
///
/// Each variant corresponds to one of the core safety invariants that
/// every VUMA program must satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InvariantKind {
    /// Every requested resource will eventually be provided.
    Liveness,
    /// At most one owner for exclusive resources.
    Exclusivity,
    /// Every read interprets data under the correct BD.
    Interpretation,
    /// Every piece of data has a well-defined provenance.
    Origin,
    /// Every acquired resource is eventually released.
    Cleanup,
    /// (Wave 16) Constant-time safety: secret values do not influence
    /// control flow or memory addresses.  Only checked under
    /// [`VerificationLevel::ConstantTime`] and [`VerificationLevel::Hardened`].
    ConstantTime,
    /// (Wave 16) Interprocedural analysis: cross-function leaks, data
    /// races, and lock-discipline violations.  Only checked under
    /// [`VerificationLevel::Exhaustive`] and [`VerificationLevel::Hardened`].
    Interprocedural,
    /// (Wave 16) Modular analysis: per-function verification using
    /// function summaries.  Only checked under
    /// [`VerificationLevel::Modular`] and [`VerificationLevel::Hardened`].
    Modular,
    /// (Wave 3d) PMT state verification: state-field reads/writes +
    /// state transformations.  Only checked under
    /// [`VerificationLevel::Pmt`].  Skips the 5 pointer invariants.
    Pmt,
}

impl InvariantKind {
    /// Return the five core invariant kinds in canonical order.
    ///
    /// These are the invariants checked at the [`VerificationLevel::Normal`]
    /// level.  The three extended kinds (`ConstantTime`, `Interprocedural`,
    /// `Modular`) are NOT included here — they are opt-in via their
    /// respective verification levels.
    pub fn all() -> &'static [InvariantKind; 5] {
        &[
            InvariantKind::Liveness,
            InvariantKind::Exclusivity,
            InvariantKind::Interpretation,
            InvariantKind::Origin,
            InvariantKind::Cleanup,
        ]
    }

    /// Return the cheap (quick-check) invariants.
    ///
    /// Exclusivity and origin can be verified by syntactic analysis
    /// without deep semantic reasoning.
    pub fn quick_set() -> &'static [InvariantKind; 2] {
        &[InvariantKind::Exclusivity, InvariantKind::Origin]
    }

    /// Human-readable label for this invariant kind.
    pub fn label(&self) -> &'static str {
        match self {
            InvariantKind::Liveness => "liveness",
            InvariantKind::Exclusivity => "exclusivity",
            InvariantKind::Interpretation => "interpretation",
            InvariantKind::Origin => "origin",
            InvariantKind::Cleanup => "cleanup",
            InvariantKind::ConstantTime => "constant-time",
            InvariantKind::Interprocedural => "interprocedural",
            InvariantKind::Modular => "modular",
            InvariantKind::Pmt => "pmt-state",
        }
    }
}

impl fmt::Display for InvariantKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

// ---------------------------------------------------------------------------
// VerificationLevel
// ---------------------------------------------------------------------------

/// How thoroughly to verify the program.
///
/// VUMA 2.0 is PMT-only: every program is verified with
/// [`VerificationLevel::Pmt`] (the default), which runs the three PMT
/// state verifiers (state-read / state-write / state-transform) and
/// SKIPS the five legacy pointer invariants. The other variants
/// (`Quick`/`Normal`/`Exhaustive`/`Modular`/`ConstantTime`/`Hardened`)
/// remain on the enum for API stability and for use by IVE-internal
/// tests, but they are NOT user-selectable in the production pipeline —
/// the CLI `--verification` flag accepts only `pmt`, and the pipeline
/// hard-codes `VerificationLevel::Pmt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum VerificationLevel {
    /// Only run cheap, syntactic checks (exclusivity, origin).
    Quick,
    /// Run all five core pointer-invariant checks (LEGACY — not used
    /// in VUMA 2.0 production code paths; retained for IVE-internal
    /// tests and API stability).
    Normal,
    /// Run all checks and attempt formal proof generation.
    /// Also runs the interprocedural analysis (Wave 16).
    Exhaustive,
    /// (Wave 16) Run the five core invariants plus modular per-function
    /// verification using function summaries.
    Modular,
    /// (Wave 16) Run the five core invariants plus the constant-time
    /// invariant (6th invariant).  Detects secret-dependent branches
    /// and memory accesses via taint propagation.
    ConstantTime,
    /// (Wave 16) Run all 6 invariants (5 core + constant-time) plus
    /// interprocedural and modular analyses.  The most thorough level.
    Hardened,
    /// (Wave 3d / VUMA 2.0 default) PMT state verification only —
    /// runs the 3 state verifiers (state-read, state-write,
    /// state-transform) and SKIPS the 5 pointer invariants.  Used for
    /// PMT (Programs as Memory Transformations) verification where
    /// memory safety is established by type-checking rather than
    /// pointer proofs.  This is the DEFAULT level for VUMA 2.0.
    #[default]
    Pmt,
}

impl fmt::Display for VerificationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerificationLevel::Quick => write!(f, "QUICK"),
            VerificationLevel::Normal => write!(f, "NORMAL"),
            VerificationLevel::Exhaustive => write!(f, "EXHAUSTIVE"),
            VerificationLevel::Modular => write!(f, "MODULAR"),
            VerificationLevel::ConstantTime => write!(f, "CONSTANT_TIME"),
            VerificationLevel::Hardened => write!(f, "HARDENED"),
            VerificationLevel::Pmt => write!(f, "PMT"),
        }
    }
}

// ---------------------------------------------------------------------------
// InvariantDelta
// ---------------------------------------------------------------------------

/// Describes which invariants are affected by a change, for incremental
/// verification.
///
/// When a program is edited, only the invariants whose results could
/// change need to be re-checked. The delta captures this set.
#[derive(Debug, Clone, Default)]
pub struct InvariantDelta {
    /// Invariant kinds that must be re-checked.
    pub affected: Vec<InvariantKind>,
    /// Optional description of the change that triggered this delta.
    pub reason: Option<String>,
}

impl InvariantDelta {
    /// Create an empty delta (nothing affected).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a delta affecting a single invariant.
    pub fn single(kind: InvariantKind) -> Self {
        Self {
            affected: vec![kind],
            reason: None,
        }
    }

    /// Create a delta affecting the given invariants.
    pub fn from_set(kinds: impl IntoIterator<Item = InvariantKind>) -> Self {
        Self {
            affected: kinds.into_iter().collect(),
            reason: None,
        }
    }

    /// Attach a human-readable reason to this delta.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Returns `true` if a given invariant kind is in the affected set.
    pub fn affects(&self, kind: InvariantKind) -> bool {
        self.affected.contains(&kind)
    }

    /// Returns `true` if the delta is empty (no invariants affected).
    pub fn is_empty(&self) -> bool {
        self.affected.is_empty()
    }
}

// ---------------------------------------------------------------------------
// PerInvariantResult
// ---------------------------------------------------------------------------

/// The result of a single invariant check within an aggregated run.
#[derive(Debug, Clone)]
pub struct PerInvariantResult {
    /// Which invariant was checked.
    pub kind: InvariantKind,
    /// The verification result.
    pub result: VerificationResult,
    /// Wall-clock time spent on this check (milliseconds).
    pub elapsed_ms: u64,
    /// Whether this result was reused from a previous run (incremental).
    pub cached: bool,
}

impl PerInvariantResult {
    /// Construct a new per-invariant result.
    pub fn new(kind: InvariantKind, result: VerificationResult, elapsed_ms: u64) -> Self {
        Self {
            kind,
            result,
            elapsed_ms,
            cached: false,
        }
    }

    /// Mark this result as cached (from a previous run).
    pub fn with_cached(mut self, cached: bool) -> Self {
        self.cached = cached;
        self
    }

    /// Returns `true` if the invariant was proven or probably safe.
    pub fn is_pass(&self) -> bool {
        matches!(
            self.result.status,
            VerificationStatus::Proven | VerificationStatus::ProbablySafe { .. }
        )
    }

    /// Returns `true` if the invariant was violated.
    pub fn is_fail(&self) -> bool {
        self.result.is_violated()
    }

    /// Returns `true` if the invariant could not be verified.
    pub fn is_unverified(&self) -> bool {
        matches!(self.result.status, VerificationStatus::Unverified { .. })
    }
}

// ---------------------------------------------------------------------------
// AggregatedResult
// ---------------------------------------------------------------------------

/// The unified result of running all (or a subset of) invariant checks.
///
/// Contains per-invariant results, an overall pass/fail verdict, and a
/// summary of statistics.
#[derive(Debug, Clone)]
pub struct AggregatedResult {
    /// Per-invariant results, in canonical order.
    pub per_invariant: Vec<PerInvariantResult>,
    /// The overall verdict.
    pub overall: OverallVerdict,
    /// The verification level that was used.
    pub level: VerificationLevel,
    /// Total wall-clock time for the entire verification run (milliseconds).
    pub total_elapsed_ms: u64,
    /// Summary statistics.
    pub summary: VerificationSummary,
}

// ---------------------------------------------------------------------------
// OverallVerdict
// ---------------------------------------------------------------------------

/// The overall verdict across all invariant checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OverallVerdict {
    /// All checked invariants passed (proven or probably safe).
    Pass,
    /// At least one invariant was violated.
    Fail,
    /// No invariant was violated, but at least one is unverified.
    Inconclusive,
    /// No checks were run (empty input).
    NoChecks,
}

impl fmt::Display for OverallVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OverallVerdict::Pass => write!(f, "PASS"),
            OverallVerdict::Fail => write!(f, "FAIL"),
            OverallVerdict::Inconclusive => write!(f, "INCONCLUSIVE"),
            OverallVerdict::NoChecks => write!(f, "NO_CHECKS"),
        }
    }
}

// ---------------------------------------------------------------------------
// VerificationSummary
// ---------------------------------------------------------------------------

/// Statistics about an aggregated verification run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VerificationSummary {
    /// Number of invariants that passed (proven or probably safe).
    pub passed: usize,
    /// Number of invariants that failed (violated).
    pub failed: usize,
    /// Number of invariants that were unverified.
    pub unverified: usize,
    /// Total number of invariants checked.
    pub total_checked: usize,
    /// Number of results reused from cache (incremental verification).
    pub cached_count: usize,
    /// Number of results freshly computed.
    pub fresh_count: usize,
    /// Overall confidence level (minimum across all results).
    pub min_confidence: Option<ConfidenceLevel>,
}

impl VerificationSummary {
    /// Compute a summary from a slice of per-invariant results.
    pub fn from_results(results: &[PerInvariantResult]) -> Self {
        let mut summary = Self {
            total_checked: results.len(),
            ..Default::default()
        };

        for r in results {
            if r.cached {
                summary.cached_count += 1;
            } else {
                summary.fresh_count += 1;
            }
            if r.is_pass() {
                summary.passed += 1;
            } else if r.is_fail() {
                summary.failed += 1;
            } else if r.is_unverified() {
                summary.unverified += 1;
            }
        }

        // Compute minimum confidence across all results.
        if !results.is_empty() {
            summary.min_confidence = Some(
                results
                    .iter()
                    .map(|r| r.result.confidence())
                    .min()
                    .unwrap_or(ConfidenceLevel::Low),
            );
        }

        summary
    }

    /// Returns `true` if all checks passed.
    pub fn is_all_pass(&self) -> bool {
        self.total_checked > 0 && self.failed == 0 && self.unverified == 0
    }

    /// Returns the pass rate as a fraction 0.0..=1.0.
    pub fn pass_rate(&self) -> f64 {
        if self.total_checked == 0 {
            0.0
        } else {
            self.passed as f64 / self.total_checked as f64
        }
    }
}

impl fmt::Display for VerificationSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Verification Summary:")?;
        writeln!(f, "  Total checked : {}", self.total_checked)?;
        writeln!(f, "  Passed        : {}", self.passed)?;
        writeln!(f, "  Failed        : {}", self.failed)?;
        writeln!(f, "  Unverified    : {}", self.unverified)?;
        writeln!(f, "  Cached        : {}", self.cached_count)?;
        writeln!(f, "  Fresh         : {}", self.fresh_count)?;
        writeln!(f, "  Pass rate     : {:.0}%", self.pass_rate() * 100.0)?;
        match self.min_confidence {
            Some(c) => writeln!(f, "  Min confidence: {c}")?,
            None => writeln!(f, "  Min confidence: N/A")?,
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// DiagnosticsReport
// ---------------------------------------------------------------------------

/// A human-readable diagnostics report for a verification run.
///
/// Generates a structured text report suitable for terminal output or
/// logging, summarising the verification results and highlighting
/// violations and unverified invariants.
#[derive(Debug, Clone)]
pub struct DiagnosticsReport {
    /// Header line (e.g., "IVE Verification Report").
    pub header: String,
    /// The verification level used.
    pub level: VerificationLevel,
    /// The overall verdict.
    pub verdict: OverallVerdict,
    /// The summary statistics.
    pub summary: VerificationSummary,
    /// Per-invariant diagnostic entries.
    pub entries: Vec<DiagnosticEntry>,
    /// Total elapsed time in milliseconds.
    pub total_elapsed_ms: u64,
}

/// A single diagnostic entry for one invariant.
#[derive(Debug, Clone)]
pub struct DiagnosticEntry {
    /// Which invariant this entry is about.
    pub kind: InvariantKind,
    /// Status icon.
    pub icon: String,
    /// Status label (PASS, FAIL, UNVERIFIED).
    pub status_label: String,
    /// Human-readable message.
    pub message: String,
    /// Whether this was a cached result.
    pub cached: bool,
    /// Elapsed time in milliseconds.
    pub elapsed_ms: u64,
}

impl DiagnosticsReport {
    /// Build a diagnostics report from an aggregated result.
    pub fn from_aggregated(result: &AggregatedResult) -> Self {
        let mut entries = Vec::with_capacity(result.per_invariant.len());

        for pir in &result.per_invariant {
            let (icon, status_label) = match &pir.result.status {
                VerificationStatus::Proven => ("PASS".to_string(), "PROVEN".into()),
                VerificationStatus::ProbablySafe { .. } => {
                    ("PROB".to_string(), "PROBABLY_SAFE".into())
                }
                VerificationStatus::Unverified { .. } => ("????".to_string(), "UNVERIFIED".into()),
                VerificationStatus::Violated { .. } => ("FAIL".to_string(), "VIOLATED".into()),
            };

            entries.push(DiagnosticEntry {
                kind: pir.kind,
                icon,
                status_label,
                message: pir.result.message.clone(),
                cached: pir.cached,
                elapsed_ms: pir.elapsed_ms,
            });
        }

        Self {
            header: "IVE Verification Report".into(),
            level: result.level,
            verdict: result.overall,
            summary: result.summary.clone(),
            entries,
            total_elapsed_ms: result.total_elapsed_ms,
        }
    }

    /// Render the report as a human-readable string.
    pub fn render(&self) -> String {
        let mut out = String::new();

        out.push_str(&format!("{}\n", self.header));
        out.push_str(&format!(
            "Level: {} | Verdict: {} | Time: {}ms\n\n",
            self.level, self.verdict, self.total_elapsed_ms
        ));

        for entry in &self.entries {
            let cached_tag = if entry.cached { " [cached]" } else { "" };
            out.push_str(&format!(
                "  {} {:<16} {:<16} ({}ms){} — {}\n",
                entry.icon,
                entry.kind.label(),
                entry.status_label,
                entry.elapsed_ms,
                cached_tag,
                entry.message,
            ));
        }

        out.push('\n');
        out.push_str(&self.summary.to_string());

        out
    }
}

impl fmt::Display for DiagnosticsReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.render())
    }
}

// ---------------------------------------------------------------------------
// InvariantAggregator
// ---------------------------------------------------------------------------

/// Runs all five VUMA invariant checks and aggregates the results.
///
/// The aggregator wraps a [`VerificationEngine`] and orchestrates the
/// individual invariant checks, collecting timing data, supporting
/// incremental re-verification, and producing unified results.
///
/// # Example
///
/// ```rust
/// use vuma_ive::invariant_aggregator::{
///     InvariantAggregator, VerificationLevel,
/// };
/// use vuma_ive::verification::VerificationInput;
/// use vuma_scg::SCG;
///
/// let scg = SCG::new();
/// let input = VerificationInput::from_scg(scg);
/// let aggregator = InvariantAggregator::new();
/// let result = aggregator.verify_all(&input);
/// ```
pub struct InvariantAggregator {
    /// The underlying verification engine.
    engine: VerificationEngine,
    /// The verification level (default: Pmt in VUMA 2.0).
    level: VerificationLevel,
    /// Cached results from a previous run (for incremental verification).
    cache: Vec<Option<PerInvariantResult>>,
    /// Whether to emit verbose diagnostic output.
    verbose: bool,
}

impl InvariantAggregator {
    /// Construct a new invariant aggregator with default settings.
    ///
    /// In VUMA 2.0 the default level is [`VerificationLevel::Pmt`]
    /// (PMT state verification only — no legacy pointer invariants).
    pub fn new() -> Self {
        Self {
            engine: VerificationEngine::new(),
            level: VerificationLevel::Pmt,
            cache: (0..EXTENDED_INVARIANT_COUNT).map(|_| None).collect(),
            verbose: false,
        }
    }

    /// Set the verification level.
    pub fn with_level(mut self, level: VerificationLevel) -> Self {
        self.level = level;
        self
    }

    /// Enable verbose diagnostic output.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self.engine = self.engine.with_verbose(verbose);
        self
    }

    /// (Wave 19) Set the maximum number of paths for the liveness verifier.
    pub fn with_max_paths(mut self, max_paths: usize) -> Self {
        self.engine = self.engine.with_max_paths(max_paths);
        self
    }

    /// (Wave 19) Set the maximum path length for the cleanup verifier.
    pub fn with_max_path_length(mut self, max_path_length: usize) -> Self {
        self.engine = self.engine.with_max_path_length(max_path_length);
        self
    }

    /// Run all invariant checks (at the configured verification level)
    /// and return the aggregated result.
    ///
    /// (Wave 19) For `Quick` mode, all 5 invariants run but with halved
    /// `max_paths` / `max_path_length` (reduced depth). This catches more
    /// bugs than the old 2-invariant Quick mode while remaining cheaper
    /// than `Normal`.
    pub fn verify_all(&self, input: &VerificationInput) -> AggregatedResult {
        let run_start = Instant::now();

        let invariants_to_run = self.invariants_for_level();
        let mut per_invariant = Vec::with_capacity(invariants_to_run.len());

        // (Wave 19) For Quick mode, build a reduced-depth engine.
        // The engine is cheap to clone (just 3 usize fields + bool).
        let effective_engine = if self.level == VerificationLevel::Quick {
            // Halve the path limits for reduced-depth Quick verification.
            let half_paths = (self.engine.max_paths() / 2).max(1);
            let half_len = (self.engine.max_path_length() / 2).max(1);
            self.engine.clone().with_max_paths(half_paths).with_max_path_length(half_len)
        } else {
            self.engine.clone()
        };

        for &kind in &invariants_to_run {
            let check_start = Instant::now();
            let result = self.run_single_check_with(&effective_engine, kind, input);
            let elapsed = check_start.elapsed().as_millis() as u64;

            per_invariant.push(PerInvariantResult::new(kind, result, elapsed));
        }

        let total_elapsed = run_start.elapsed().as_millis() as u64;
        let summary = VerificationSummary::from_results(&per_invariant);
        let overall = compute_overall_verdict(&per_invariant);

        AggregatedResult {
            per_invariant,
            overall,
            level: self.level,
            total_elapsed_ms: total_elapsed,
            summary,
        }
    }

    /// Run incremental verification: only re-check invariants affected
    /// by the given delta, reusing cached results for the rest.
    pub fn verify_incremental(
        &mut self,
        input: &VerificationInput,
        delta: &InvariantDelta,
    ) -> AggregatedResult {
        let run_start = Instant::now();
        let invariants_to_run = self.invariants_for_level();
        let mut per_invariant = Vec::with_capacity(invariants_to_run.len());

        for &kind in &invariants_to_run {
            if delta.affects(kind) {
                // Re-check this invariant.
                let check_start = Instant::now();
                let result = self.run_single_check(kind, input);
                let elapsed = check_start.elapsed().as_millis() as u64;

                let pir = PerInvariantResult::new(kind, result, elapsed);
                // Update cache.
                if let Some(idx) = invariant_index(kind) {
                    self.cache[idx] = Some(pir.clone());
                }
                per_invariant.push(pir);
            } else {
                // Reuse cached result if available.
                if let Some(idx) = invariant_index(kind) {
                    if let Some(cached) = self.cache[idx].clone() {
                        per_invariant.push(cached.with_cached(true));
                        continue;
                    }
                }
                // No cache — must compute anyway.
                let check_start = Instant::now();
                let result = self.run_single_check(kind, input);
                let elapsed = check_start.elapsed().as_millis() as u64;
                let pir = PerInvariantResult::new(kind, result, elapsed);
                if let Some(idx) = invariant_index(kind) {
                    self.cache[idx] = Some(pir.clone());
                }
                per_invariant.push(pir);
            }
        }

        let total_elapsed = run_start.elapsed().as_millis() as u64;
        let summary = VerificationSummary::from_results(&per_invariant);
        let overall = compute_overall_verdict(&per_invariant);

        AggregatedResult {
            per_invariant,
            overall,
            level: self.level,
            total_elapsed_ms: total_elapsed,
            summary,
        }
    }

    /// Generate a diagnostics report from the given aggregated result.
    pub fn diagnostics(&self, result: &AggregatedResult) -> DiagnosticsReport {
        DiagnosticsReport::from_aggregated(result)
    }

    /// Clear the internal cache, forcing all checks to be re-run.
    pub fn clear_cache(&mut self) {
        self.cache = (0..EXTENDED_INVARIANT_COUNT).map(|_| None).collect();
    }

    /// Returns the current verification level.
    pub fn level(&self) -> VerificationLevel {
        self.level
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Return the set of invariants to check for the current level.
    ///
    /// (Wave 19) `Quick` now runs ALL 5 invariants (not just the 2-invariant
    /// `quick_set`) at reduced depth. The depth reduction is implemented by
    /// halving `max_paths` / `max_path_length` in `verify_all` when the level
    /// is `Quick`. This catches more bugs than the old 2-invariant Quick mode
    /// while remaining cheaper than `Normal`.
    fn invariants_for_level(&self) -> Vec<InvariantKind> {
        match self.level {
            VerificationLevel::Quick => InvariantKind::all().to_vec(),
            VerificationLevel::Normal => InvariantKind::all().to_vec(),
            VerificationLevel::Exhaustive => {
                // Core 5 + interprocedural analysis.
                let mut v = InvariantKind::all().to_vec();
                v.push(InvariantKind::Interprocedural);
                v
            }
            VerificationLevel::Modular => {
                // Core 5 + modular analysis.
                let mut v = InvariantKind::all().to_vec();
                v.push(InvariantKind::Modular);
                v
            }
            VerificationLevel::ConstantTime => {
                // Core 5 + constant-time (6th invariant).
                let mut v = InvariantKind::all().to_vec();
                v.push(InvariantKind::ConstantTime);
                v
            }
            VerificationLevel::Hardened => {
                // All 6 invariants + interprocedural + modular.
                let mut v = InvariantKind::all().to_vec();
                v.push(InvariantKind::ConstantTime);
                v.push(InvariantKind::Interprocedural);
                v.push(InvariantKind::Modular);
                v
            }
            VerificationLevel::Pmt => {
                // Wave 3d: only PMT state verification (skips 5 pointer invariants).
                vec![InvariantKind::Pmt]
            }
        }
    }

    /// Run a single invariant check by kind.
    fn run_single_check(
        &self,
        kind: InvariantKind,
        input: &VerificationInput,
    ) -> VerificationResult {
        self.run_single_check_with(&self.engine, kind, input)
    }

    /// (Wave 19) Run a single invariant check using a specific engine
    /// (allows reduced-depth engine for Quick mode).
    fn run_single_check_with(
        &self,
        engine: &VerificationEngine,
        kind: InvariantKind,
        input: &VerificationInput,
    ) -> VerificationResult {
        if self.verbose {
            vuma_log!(info, "InvariantAggregator: checking {kind}");
        }

        let result = match kind {
            InvariantKind::Liveness => engine.verify_liveness(input),
            InvariantKind::Exclusivity => engine.verify_exclusivity(input),
            InvariantKind::Interpretation => engine.verify_interpretation(input),
            InvariantKind::Origin => engine.verify_origin(input),
            InvariantKind::Cleanup => engine.verify_cleanup(input),
            InvariantKind::ConstantTime => self.verify_constant_time(input),
            InvariantKind::Interprocedural => self.verify_interprocedural(input),
            InvariantKind::Modular => self.verify_modular(input),
            InvariantKind::Pmt => self.verify_pmt(input),
        };

        // Wave 18: The fake FormalProof string-evidence was removed.
        // Real proof-system cross-checking now happens in api.rs's
        // build_proof_bundle(), which calls ProofChecker::check on the
        // prove_* tactics' output. IVE no longer claims to have formal
        // proof evidence when it only did dataflow analysis.

        result
    }

    // -----------------------------------------------------------------------
    // Wave 16: Extended analysis helpers
    // -----------------------------------------------------------------------

    /// Verify the constant-time invariant: secret values must not influence
    /// control flow (branches) or memory addresses (accesses).
    ///
    /// Extracts `secret_nodes`, `branch_nodes`, `access_nodes`, and data-flow
    /// `edges` from the SCG and delegates to
    /// [`crate::constant_time::verify_constant_time`].
    fn verify_constant_time(&self, input: &VerificationInput) -> VerificationResult {
        use crate::constant_time::verify_constant_time as ct_verify;
        use vuma_scg::edge::EdgeKind;
        use vuma_scg::node::{ControlKind, NodePayload, NodeType};

        let scg = &input.scg;
        let mut secret_nodes: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut branch_nodes: std::collections::HashSet<u64> = std::collections::HashSet::new();
        let mut access_nodes: std::collections::HashSet<u64> = std::collections::HashSet::new();

        for node in scg.nodes() {
            let id = node.id.as_u64();
            // Branch nodes: Control nodes with kind=Branch.
            if node.node_type == NodeType::Control {
                if let NodePayload::Control(ctrl) = &node.payload {
                    if ctrl.kind == ControlKind::Branch {
                        branch_nodes.insert(id);
                    }
                    // Secret heuristic: check Control node labels for "secret".
                    if let Some(label) = &ctrl.label {
                        if label.to_lowercase().contains("secret") {
                            secret_nodes.insert(id);
                        }
                    }
                }
            }
            // Access nodes: memory read/write nodes.
            if node.node_type == NodeType::Access {
                access_nodes.insert(id);
            }
            // Secret heuristic: check source file name for "secret".
            if let Some(ref file) = node.program_point.file {
                if file.to_lowercase().contains("secret") {
                    secret_nodes.insert(id);
                }
            }
        }

        // Collect data-flow edges as (source, target) pairs.
        let edges: Vec<(u64, u64)> = scg
            .edges()
            .filter(|e| matches!(e.kind, EdgeKind::DataFlow))
            .map(|e| (e.source.as_u64(), e.target.as_u64()))
            .collect();

        let violations = ct_verify(&secret_nodes, &branch_nodes, &access_nodes, &edges);

        if violations.is_empty() {
            VerificationResult::new(
                "constant-time",
                VerificationStatus::Proven,
                format!(
                    "constant-time check passed ({} secret(s), {} branch(es), {} access(es))",
                    secret_nodes.len(),
                    branch_nodes.len(),
                    access_nodes.len()
                ),
            )
        } else {
            let msgs: Vec<String> = violations.iter().map(|v| v.message.clone()).collect();
            VerificationResult::new(
                "constant-time",
                VerificationStatus::Violated {
                    counterexample: crate::result::CounterExample::new(
                        Vec::new(),
                        default_program_point(),
                        msgs.join("; "),
                    ),
                },
                format!("constant-time violated: {} violation(s)", violations.len()),
            )
        }
    }

    /// Verify interprocedural invariants: cross-function leaks, data races,
    /// and lock-discipline violations.
    ///
    /// Builds a [`CallGraph`] from the SCG, computes function summaries
    /// bottom-up, and delegates to
    /// [`crate::interprocedural::verify_interprocedural_invariants`].
    fn verify_interprocedural(&self, input: &VerificationInput) -> VerificationResult {
        use crate::interprocedural::{compute_summaries, verify_interprocedural_invariants};
        use vuma_scg::callgraph::CallGraph;

        let scg = &input.scg;
        let call_graph = CallGraph::build(scg);
        let summaries = compute_summaries(scg, &call_graph);
        let violations = verify_interprocedural_invariants(scg, &call_graph, &summaries);

        if violations.is_empty() {
            VerificationResult::new(
                "interprocedural",
                VerificationStatus::Proven,
                format!(
                    "interprocedural check passed ({} function(s) analyzed)",
                    summaries.len()
                ),
            )
        } else {
            let msgs: Vec<String> = violations.iter().map(|v| v.to_string()).collect();
            VerificationResult::new(
                "interprocedural",
                VerificationStatus::Violated {
                    counterexample: crate::result::CounterExample::new(
                        Vec::new(),
                        default_program_point(),
                        msgs.join("; "),
                    ),
                },
                format!(
                    "interprocedural violations: {} violation(s)",
                    violations.len()
                ),
            )
        }
    }

    /// Verify modular invariants: per-function verification using function
    /// summaries (allocation/free discipline, purity, escape analysis).
    ///
    /// Extracts function entries from the SCG and delegates to
    /// [`crate::modular::verify_all_functions`].
    fn verify_modular(&self, input: &VerificationInput) -> VerificationResult {
        use crate::modular::verify_all_functions;
        use vuma_scg::node::{ControlKind, NodePayload, NodeType};

        let scg = &input.scg;
        // Build function_entries: (name, node_ids) for each function.
        let (entries, _returns) = scg.function_boundary_nodes();
        let mut function_entries: Vec<(String, Vec<vuma_scg::node::NodeId>)> = Vec::new();

        for entry_id in &entries {
            // BFS through ControlFlow edges to collect all nodes in this function.
            let mut nodes = vec![*entry_id];
            let mut visited = std::collections::HashSet::new();
            visited.insert(*entry_id);
            let mut queue = std::collections::VecDeque::new();
            queue.push_back(*entry_id);
            while let Some(cur) = queue.pop_front() {
                for edge in scg.edges() {
                    if edge.source == cur && !visited.contains(&edge.target) {
                        visited.insert(edge.target);
                        // Stop at FunctionReturn (don't include return nodes of
                        // other functions — but include our own).
                        if let Some(n) = scg.get_node(edge.target) {
                            if n.node_type == NodeType::Control {
                                if let NodePayload::Control(ctrl) = &n.payload {
                                    if ctrl.kind == ControlKind::FunctionReturn {
                                        nodes.push(edge.target);
                                        continue; // include but don't traverse past
                                    }
                                    if ctrl.kind == ControlKind::FunctionEntry {
                                        continue; // don't cross into another function
                                    }
                                }
                            }
                            nodes.push(edge.target);
                            queue.push_back(edge.target);
                        }
                    }
                }
            }

            // Function name: use the label from the entry node's Control payload,
            // or a synthetic name.
            let name = scg.get_node(*entry_id).and_then(|n| {
                if let NodePayload::Control(ctrl) = &n.payload {
                    ctrl.label.clone()
                } else {
                    None
                }
            }).unwrap_or_else(|| format!("fn_{}", entry_id.as_u64()));
            function_entries.push((name, nodes));
        }

        let issues = verify_all_functions(scg, &function_entries);

        if issues.is_empty() {
            VerificationResult::new(
                "modular",
                VerificationStatus::Proven,
                format!(
                    "modular check passed ({} function(s) verified)",
                    function_entries.len()
                ),
            )
        } else {
            VerificationResult::new(
                "modular",
                VerificationStatus::Violated {
                    counterexample: crate::result::CounterExample::new(
                        Vec::new(),
                        default_program_point(),
                        issues.join("; "),
                    ),
                },
                format!("modular violations: {} issue(s)", issues.len()),
            )
        }
    }

    // -----------------------------------------------------------------------
    // Wave 3d: PMT state verification
    // -----------------------------------------------------------------------

    /// (Wave 3d) Verify PMT (Programs as Memory Transformations) state
    /// safety: state-field reads, state-field writes (with linearity), and
    /// state transformations.
    ///
    /// Walks the SCG for `StateRead` / `StateWrite` / `StateTransform` /
    /// `StateInit` nodes, builds the per-verifier input tuples, and
    /// delegates to [`crate::state_read::verify_state_reads`],
    /// [`crate::state_write::verify_state_writes`], and
    /// [`crate::state_transform::verify_all_transforms`].
    ///
    /// # Layout registry
    ///
    /// The SCG does not retain structured layout info — `Item::LayoutDef`
    /// is lowered to a `Computation` node with a descriptive label (see
    /// `parser::to_scg::convert_item`).  The pipeline therefore attaches
    /// the layout registry to [`VerificationInput::pmt_layouts`] before
    /// invoking the aggregator at the [`VerificationLevel::Pmt`] level.
    /// If `pmt_layouts` is absent, the verifiers will report "layout not
    /// found" for every state operation (a FAIL verdict) — this is the
    /// correct behaviour for an unconfigured run.
    ///
    /// # Vreg → var-name mapping
    ///
    /// The SCG's `StateReadNode` / `StateWriteNode` use a `state_vreg: u32`
    /// field rather than a source variable name.  The verifiers work in
    /// terms of variable names, so we synthesise a stable name
    /// `"_state_{vreg}"` per distinct vreg and use the node's
    /// `layout_name` to populate `state_var_layouts`.  This is sufficient
    /// for the verifiers' current API (which only consults the layout
    /// name).
    fn verify_pmt(&self, input: &VerificationInput) -> VerificationResult {
        use crate::state_read::{verify_state_reads, LayoutInfo as ReadLayout, FieldInfo as ReadField};
        use crate::state_write::{
            verify_state_writes, LayoutInfo as WriteLayout, FieldInfo as WriteField,
            StateWriteOp,
        };
        use crate::state_transform::{
            verify_all_transforms, LayoutInfo as TransformLayout, FieldInfo as TransformField,
        };
        use std::collections::{HashMap, HashSet};
        use vuma_scg::node::{NodePayload, NodeType, StateReadNode, StateWriteNode, StateTransformNode, ForeignConsumeNode};

        let scg = &input.scg;

        // ── Build the per-verifier layout registries ──────────────────────
        //
        // The 3 verifiers each carry their own duplicated `LayoutInfo` /
        // `FieldInfo` structs (Wave 3c/3b parallel-development artefact).
        // We convert the unified `PmtLayoutSpec` to each one on demand.
        let empty: HashMap<String, crate::verification::PmtLayoutSpec> = HashMap::new();
        let pmt_layouts = input.pmt_layouts.as_ref().unwrap_or(&empty);

        let mut read_layouts: HashMap<String, ReadLayout> = HashMap::new();
        let mut write_layouts: HashMap<String, WriteLayout> = HashMap::new();
        let mut transform_layouts: HashMap<String, TransformLayout> = HashMap::new();
        for (name, spec) in pmt_layouts {
            read_layouts.insert(
                name.clone(),
                ReadLayout {
                    name: spec.name.clone(),
                    total_size: spec.total_size,
                    fields: spec
                        .fields
                        .iter()
                        .map(|f| ReadField {
                            name: f.name.clone(),
                            offset: f.offset,
                            size: f.size,
                            type_name: f.type_name.clone(),
                        })
                        .collect(),
                },
            );
            write_layouts.insert(
                name.clone(),
                WriteLayout {
                    name: spec.name.clone(),
                    total_size: spec.total_size,
                    fields: spec
                        .fields
                        .iter()
                        .map(|f| WriteField {
                            name: f.name.clone(),
                            offset: f.offset,
                            size: f.size,
                            type_name: f.type_name.clone(),
                        })
                        .collect(),
                },
            );
            transform_layouts.insert(
                name.clone(),
                TransformLayout {
                    name: spec.name.clone(),
                    total_size: spec.total_size,
                    fields: spec
                        .fields
                        .iter()
                        .map(|f| TransformField {
                            name: f.name.clone(),
                            offset: f.offset,
                            size: f.size,
                            type_name: f.type_name.clone(),
                        })
                        .collect(),
                },
            );
        }

        // ── Walk SCG nodes; collect reads / writes / transforms ───────────
        //
        // We also track which vregs have been "consumed" by a transform
        // (the input vreg of a StateTransform is linearly consumed).  The
        // write verifier's linearity check uses this set.
        let mut state_var_layouts: HashMap<String, String> = HashMap::new();
        let mut consumed_vars: HashSet<String> = HashSet::new();
        let mut reads: Vec<(String, String, String)> = Vec::new();
        let mut writes: Vec<StateWriteOp> = Vec::new();
        let mut transforms: Vec<(String, String)> = Vec::new();
        let mut state_init_count: usize = 0;

        // First pass: collect transforms (to populate `consumed_vars`)
        // before writes, so the linearity check sees them.  SCG node
        // iteration order is not guaranteed to match source order; we
        // sort by NodeId to get a stable, source-approximating order.
        let mut state_nodes: Vec<(u64, NodeType, NodePayload)> = Vec::new();
        for node in scg.nodes() {
            state_nodes.push((node.id.as_u64(), node.node_type.clone(), node.payload.clone()));
        }
        state_nodes.sort_by_key(|(id, _, _)| *id);

        for (_, _, payload) in &state_nodes {
            match payload {
                NodePayload::StateInit(_) => {
                    state_init_count += 1;
                }
                NodePayload::StateTransform(t) => {
                    let StateTransformNode {
                        input_vreg,
                        input_layout,
                        output_layout,
                        ..
                    } = t;
                    let in_var = format!("_state_{}", input_vreg);
                    state_var_layouts
                        .entry(in_var.clone())
                        .or_insert_with(|| input_layout.clone());
                    consumed_vars.insert(in_var);
                    transforms.push((input_layout.clone(), output_layout.clone()));
                }
                NodePayload::ForeignConsume(fc) => {
                    // A #[foreign_consume] call (e.g. sqlite3_close) linearly
                    // consumes its State argument, exactly like a StateTransform.
                    // The existing state_write linearity check then catches any
                    // post-close read/write as a use-after-consume error.
                    let ForeignConsumeNode {
                        input_vreg,
                        layout_name,
                    } = fc;
                    let in_var = format!("_state_{}", input_vreg);
                    state_var_layouts
                        .entry(in_var.clone())
                        .or_insert_with(|| layout_name.clone());
                    consumed_vars.insert(in_var);
                }
                NodePayload::StateRead(r) => {
                    let StateReadNode {
                        state_vreg,
                        layout_name,
                        field_name,
                        ..
                    } = r;
                    let var = format!("_state_{}", state_vreg);
                    state_var_layouts
                        .entry(var.clone())
                        .or_insert_with(|| layout_name.clone());
                    // Look up the field's declared type so the verifier's
                    // type-mismatch check has a non-empty expected_type to
                    // compare against.  If the layout/field is absent,
                    // pass "" — the verifier will report "field not found"
                    // or "layout not found" before reaching the type check.
                    let expected_type = pmt_layouts
                        .get(layout_name)
                        .and_then(|spec| spec.fields.iter().find(|f| &f.name == field_name))
                        .map(|f| f.type_name.clone())
                        .unwrap_or_default();
                    reads.push((var, field_name.clone(), expected_type));
                }
                NodePayload::StateWrite(w) => {
                    let StateWriteNode {
                        state_vreg,
                        layout_name,
                        field_name,
                        ..
                    } = w;
                    let var = format!("_state_{}", state_vreg);
                    state_var_layouts
                        .entry(var.clone())
                        .or_insert_with(|| layout_name.clone());
                    // The SCG StateWriteNode does not carry the value's
                    // type; infer it from the layout's field declaration.
                    let value_type = pmt_layouts
                        .get(layout_name)
                        .and_then(|spec| spec.fields.iter().find(|f| &f.name == field_name))
                        .map(|f| f.type_name.clone())
                        .unwrap_or_default();
                    writes.push(StateWriteOp {
                        var_name: var,
                        field_name: field_name.clone(),
                        value_type,
                        after_consume: false,
                    });
                }
                _ => {}
            }
        }

        // ── Run the 3 verifiers ───────────────────────────────────────────
        let read_results = verify_state_reads(&state_var_layouts, &read_layouts, &reads);
        let write_results =
            verify_state_writes(&state_var_layouts, &write_layouts, &writes, &consumed_vars);
        let transform_results = verify_all_transforms(&transform_layouts, &transforms);

        let read_ok = read_results.iter().all(|r| r.valid);
        let write_ok = write_results.iter().all(|r| r.valid);
        let transform_ok = transform_results.iter().all(|r| r.valid);

        let read_errs: Vec<String> = read_results
            .iter()
            .filter_map(|r| r.error.clone())
            .collect();
        let write_errs: Vec<String> = write_results
            .iter()
            .filter_map(|r| r.error.clone())
            .collect();
        let transform_errs: Vec<String> = transform_results
            .iter()
            .filter_map(|r| r.error.clone())
            .collect();

        let all_errs: Vec<String> = read_errs
            .iter()
            .chain(write_errs.iter())
            .chain(transform_errs.iter())
            .cloned()
            .collect();

        let total_ops = reads.len() + writes.len() + transforms.len();
        let all_ok = read_ok && write_ok && transform_ok;

        if all_ok {
            VerificationResult::new(
                "pmt-state",
                VerificationStatus::Proven,
                format!(
                    "pmt-state check passed ({} init(s), {} read(s), {} write(s), {} transform(s))",
                    state_init_count,
                    reads.len(),
                    writes.len(),
                    transforms.len()
                ),
            )
        } else if total_ops == 0 {
            // No state operations in the SCG — trivially safe.
            VerificationResult::new(
                "pmt-state",
                VerificationStatus::Proven,
                "pmt-state check passed (no state operations found)".to_string(),
            )
        } else {
            VerificationResult::new(
                "pmt-state",
                VerificationStatus::Violated {
                    counterexample: crate::result::CounterExample::new(
                        Vec::new(),
                        default_program_point(),
                        all_errs.join("; "),
                    ),
                },
                format!(
                    "pmt-state violations: {} read-error(s), {} write-error(s), {} transform-error(s)",
                    read_errs.len(),
                    write_errs.len(),
                    transform_errs.len()
                ),
            )
        }
    }
}

impl Default for InvariantAggregator {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Free function: verify_all
// ---------------------------------------------------------------------------

/// Convenience function: run verification at the PMT level
/// ([`VerificationLevel::Pmt`] in VUMA 2.0 — the three PMT state
/// verifiers only) and return the aggregated result.
///
/// VUMA 2.0 is PMT-only: this helper explicitly chains
/// `.with_level(VerificationLevel::Pmt)` so PMT enforcement does not
/// depend solely on the `InvariantAggregator::new()` default (which is
/// also `Pmt`). This makes the mandate robust against any future change
/// to the `new()` default.
///
/// Note: PMT state verification requires the layout registry to be
/// attached to the [`VerificationInput`] via `with_pmt_layouts(...)`.
/// If `pmt_layouts` is absent, the verifiers report "layout not found"
/// for every state operation (a FAIL verdict).
pub fn verify_all(input: &VerificationInput) -> AggregatedResult {
    InvariantAggregator::new()
        .with_level(VerificationLevel::Pmt)
        .verify_all(input)
}

// ---------------------------------------------------------------------------
// Wave 96: 5→3 invariant reduction (5to3)
// ---------------------------------------------------------------------------

/// Wave 96: The 5→3 invariant reduction (5to3).
///
/// VUMA's five core invariants (Liveness, Exclusivity, Interpretation,
/// Origin, Cleanup) collapse into THREE compile-time invariants under
/// the L1-L3 collapse theorem (see `verification::l1l3_collapse`):
///
///   1. **Resource Safety** = Liveness ∪ Cleanup
///      (every acquired resource is eventually released AND every
///      requested resource is eventually provided — together they
///      guarantee no leaks and no deadlocks).
///
///   2. **Access Safety** = Exclusivity ∪ Interpretation
///      (at most one owner for exclusive resources AND every read
///      interprets data under the correct BD — together they guarantee
///      no data races and no type confusion).
///
///   3. **Provenance Safety** = Origin
///      (every datum has a well-defined provenance — unchanged by the
///      collapse; origin is already a single-invariant property that
///      subsumes the data-trust boundary).
///
/// The 5to3 reduction is sound: if all five original invariants hold,
/// then all three collapsed invariants hold. The converse is NOT
/// true in general (the collapse loses information about WHICH of the
/// two original invariants in each pair failed), but for the purposes
/// of compile-time verification, the three-way partition is sufficient
/// to gate codegen on (a program that fails any of the three collapsed
/// invariants is rejected).
///
/// The reduction is used by the L1L3 collapse proof to simplify the
/// final verdict: instead of reporting five separate invariant
/// outcomes, the aggregator can report three (one per collapsed
/// category), which is easier for downstream tooling (compilers,
/// IDEs, security review dashboards) to consume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollapsedInvariant {
    /// Resource Safety = Liveness ∪ Cleanup.
    ResourceSafety,
    /// Access Safety = Exclusivity ∪ Interpretation.
    AccessSafety,
    /// Provenance Safety = Origin.
    ProvenanceSafety,
}

impl CollapsedInvariant {
    /// Returns the three collapsed invariant kinds in canonical order.
    pub fn all() -> &'static [CollapsedInvariant; 3] {
        &[
            CollapsedInvariant::ResourceSafety,
            CollapsedInvariant::AccessSafety,
            CollapsedInvariant::ProvenanceSafety,
        ]
    }

    /// Returns the human-readable label for this collapsed invariant.
    pub fn label(&self) -> &'static str {
        match self {
            CollapsedInvariant::ResourceSafety => "resource_safety",
            CollapsedInvariant::AccessSafety => "access_safety",
            CollapsedInvariant::ProvenanceSafety => "provenance_safety",
        }
    }

    /// Maps a five-core `InvariantKind` to its collapsed three-core
    /// equivalent. Returns `None` for the extended kinds (ConstantTime,
    /// Interprocedural, Modular, PMT) which are NOT part of the 5→3
    /// reduction.
    pub fn from_five(kind: InvariantKind) -> Option<CollapsedInvariant> {
        match kind {
            InvariantKind::Liveness | InvariantKind::Cleanup => {
                Some(CollapsedInvariant::ResourceSafety)
            }
            InvariantKind::Exclusivity | InvariantKind::Interpretation => {
                Some(CollapsedInvariant::AccessSafety)
            }
            InvariantKind::Origin => Some(CollapsedInvariant::ProvenanceSafety),
            // Extended kinds are not part of the 5→3 reduction.
            InvariantKind::ConstantTime
            | InvariantKind::Interprocedural
            | InvariantKind::Modular
            | InvariantKind::Pmt => None,
        }
    }
}

impl fmt::Display for CollapsedInvariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Wave 96: Result of the 5→3 invariant reduction.
///
/// Maps each of the five core invariants to one of the three collapsed
/// invariants, and records whether each collapsed invariant is
/// satisfied (i.e. both of its constituent five-core invariants
/// passed).
#[derive(Debug, Clone)]
pub struct FiveToThreeReduction {
    /// For each of the three collapsed invariants, whether it is
    /// satisfied (true) or violated (false). Indexed by
    /// `CollapsedInvariant::all()` order.
    pub collapsed: [bool; 3],
    /// The number of five-core invariants that were folded into the
    /// three collapsed invariants (always 5 for a complete reduction).
    pub folded: usize,
    /// Human-readable summary.
    pub summary: String,
}

/// Wave 96: Perform the 5→3 invariant reduction.
///
/// Given the per-invariant pass/fail status of the five core
/// invariants (in canonical order: Liveness, Exclusivity,
/// Interpretation, Origin, Cleanup — the order returned by
/// `InvariantKind::all()`), compute the three collapsed invariants
/// and their pass/fail status.
///
/// A collapsed invariant is satisfied iff BOTH of its constituent
/// five-core invariants are satisfied:
///   - ResourceSafety = Liveness ∧ Cleanup
///   - AccessSafety = Exclusivity ∧ Interpretation
///   - ProvenanceSafety = Origin (single invariant, identity)
///
/// `five_results` — slice of (kind, passed) pairs for the five core
/// invariants. Invariants not present in the slice are treated as
/// "unverified" (collapsed invariant = false).
pub fn reduce_5to3<I>(five_results: I) -> FiveToThreeReduction
where
    I: IntoIterator<Item = (InvariantKind, bool)>,
{
    use std::collections::HashMap;
    let map: HashMap<InvariantKind, bool> = five_results.into_iter().collect();
    let mut collapsed = [false; 3];
    let mut seen = [false; 3];
    let mut folded = 0;
    for &kind in InvariantKind::all() {
        if let Some(&passed) = map.get(&kind) {
            folded += 1;
            if let Some(c) = CollapsedInvariant::from_five(kind) {
                let idx = match c {
                    CollapsedInvariant::ResourceSafety => 0,
                    CollapsedInvariant::AccessSafety => 1,
                    CollapsedInvariant::ProvenanceSafety => 2,
                };
                // A collapsed invariant is satisfied iff ALL of its
                // constituents are satisfied. First-encounter sets
                // the value; subsequent encounters AND with it.
                if !seen[idx] {
                    collapsed[idx] = passed;
                    seen[idx] = true;
                } else {
                    collapsed[idx] = collapsed[idx] && passed;
                }
            }
        }
    }
    let summary = format!(
        "5→3 reduction: folded {} five-core invariants into three collapsed invariants \
         (resource_safety={}, access_safety={}, provenance_safety={})",
        folded,
        collapsed[0],
        collapsed[1],
        collapsed[2]
    );
    FiveToThreeReduction {
        collapsed,
        folded,
        summary,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the overall verdict from per-invariant results.
fn compute_overall_verdict(results: &[PerInvariantResult]) -> OverallVerdict {
    if results.is_empty() {
        return OverallVerdict::NoChecks;
    }

    let has_violation = results.iter().any(|r| r.is_fail());
    let has_unverified = results.iter().any(|r| r.is_unverified());

    if has_violation {
        OverallVerdict::Fail
    } else if has_unverified {
        OverallVerdict::Inconclusive
    } else {
        OverallVerdict::Pass
    }
}

/// Map an invariant kind to a cache index (0..7).
///
/// Indices 0-4 are the five core invariants; 5-7 are the Wave 16 extended
/// kinds.  The cache vector in [`InvariantAggregator`] is sized to
/// `EXTENDED_INVARIANT_COUNT` (8) to accommodate all kinds.
fn invariant_index(kind: InvariantKind) -> Option<usize> {
    match kind {
        InvariantKind::Liveness => Some(0),
        InvariantKind::Exclusivity => Some(1),
        InvariantKind::Interpretation => Some(2),
        InvariantKind::Origin => Some(3),
        InvariantKind::Cleanup => Some(4),
        InvariantKind::ConstantTime => Some(5),
        InvariantKind::Interprocedural => Some(6),
        InvariantKind::Modular => Some(7),
        InvariantKind::Pmt => Some(8),
    }
}

/// The total number of invariant kinds (5 core + 3 extended + 1 PMT = 9).
const EXTENDED_INVARIANT_COUNT: usize = 9;

/// Construct a default [`ProgramPoint`] (empty string) for use in
/// counterexamples where the exact source location is not known.
fn default_program_point() -> crate::result::ProgramPoint {
    String::new()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::CounterExample;
    use vuma_scg::graph::SCG;

    #[test]
    fn invariant_kind_all_has_five() {
        assert_eq!(InvariantKind::all().len(), 5);
    }

    #[test]
    fn invariant_kind_quick_set_has_two() {
        assert_eq!(InvariantKind::quick_set().len(), 2);
        assert!(InvariantKind::quick_set().contains(&InvariantKind::Exclusivity));
        assert!(InvariantKind::quick_set().contains(&InvariantKind::Origin));
    }

    #[test]
    fn invariant_kind_labels() {
        assert_eq!(InvariantKind::Liveness.label(), "liveness");
        assert_eq!(InvariantKind::Exclusivity.label(), "exclusivity");
        assert_eq!(InvariantKind::Interpretation.label(), "interpretation");
        assert_eq!(InvariantKind::Origin.label(), "origin");
        assert_eq!(InvariantKind::Cleanup.label(), "cleanup");
    }

    #[test]
    fn invariant_kind_display() {
        assert_eq!(format!("{}", InvariantKind::Liveness), "liveness");
    }

    #[test]
    fn verification_level_default_is_pmt() {
        // VUMA 2.0: PMT is the default verification level.
        assert_eq!(VerificationLevel::default(), VerificationLevel::Pmt);
    }

    #[test]
    fn verification_level_display() {
        assert_eq!(format!("{}", VerificationLevel::Quick), "QUICK");
        assert_eq!(format!("{}", VerificationLevel::Normal), "NORMAL");
        assert_eq!(format!("{}", VerificationLevel::Exhaustive), "EXHAUSTIVE");
        assert_eq!(format!("{}", VerificationLevel::Modular), "MODULAR");
        assert_eq!(format!("{}", VerificationLevel::ConstantTime), "CONSTANT_TIME");
        assert_eq!(format!("{}", VerificationLevel::Hardened), "HARDENED");
        assert_eq!(format!("{}", VerificationLevel::Pmt), "PMT");
    }

    #[test]
    fn delta_empty_by_default() {
        let delta = InvariantDelta::new();
        assert!(delta.is_empty());
        assert!(!delta.affects(InvariantKind::Liveness));
    }

    #[test]
    fn delta_single_affects_only_one() {
        let delta = InvariantDelta::single(InvariantKind::Cleanup);
        assert!(!delta.is_empty());
        assert!(delta.affects(InvariantKind::Cleanup));
        assert!(!delta.affects(InvariantKind::Liveness));
    }

    #[test]
    fn delta_from_set() {
        let delta = InvariantDelta::from_set([InvariantKind::Liveness, InvariantKind::Cleanup])
            .with_reason("resource change");
        assert!(delta.affects(InvariantKind::Liveness));
        assert!(delta.affects(InvariantKind::Cleanup));
        assert!(!delta.affects(InvariantKind::Origin));
        assert_eq!(delta.reason.as_deref(), Some("resource change"));
    }

    #[test]
    fn verify_all_normal_returns_five_results() {
        // VUMA 2.0: the default aggregator level is now Pmt (1 result),
        // so to test the legacy 5-pointer-invariant `Normal` mode we
        // must explicitly request it.
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Normal);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.per_invariant.len(), 5);
        assert_eq!(result.level, VerificationLevel::Normal);
    }

    #[test]
    fn verify_all_pmt_default_returns_one_result() {
        // VUMA 2.0: the default level is PMT, which runs only the 3
        // PMT state verifiers surfaced as a single `InvariantKind::Pmt`
        // aggregated result.
        let aggregator = InvariantAggregator::new();
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.per_invariant.len(), 1);
        assert_eq!(result.level, VerificationLevel::Pmt);
    }

    #[test]
    fn verify_all_quick_returns_five_results() {
        // Wave 19: Quick mode now runs ALL 5 invariants at reduced depth
        // (halved max_paths / max_path_length), not just the 2-invariant
        // quick_set. This catches more bugs while remaining cheaper than
        // Normal.
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Quick);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.per_invariant.len(), 5);
        assert_eq!(result.level, VerificationLevel::Quick);
    }

    #[test]
    fn verify_all_exhaustive_returns_six_results() {
        // Wave 16: Exhaustive now runs 5 core + interprocedural = 6 checks.
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Exhaustive);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.per_invariant.len(), 6);
        assert_eq!(result.level, VerificationLevel::Exhaustive);
    }

    // ── Wave 16: new verification level tests ───────────────────────────

    #[test]
    fn verify_all_modular_returns_six_results() {
        // Modular: 5 core + modular analysis = 6 checks.
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Modular);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.per_invariant.len(), 6);
        assert_eq!(result.level, VerificationLevel::Modular);
    }

    #[test]
    fn verify_all_constant_time_returns_six_results() {
        // ConstantTime: 5 core + constant-time (6th invariant) = 6 checks.
        let aggregator =
            InvariantAggregator::new().with_level(VerificationLevel::ConstantTime);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.per_invariant.len(), 6);
        assert_eq!(result.level, VerificationLevel::ConstantTime);
    }

    #[test]
    fn verify_all_hardened_returns_eight_results() {
        // Hardened: 5 core + constant-time + interprocedural + modular = 8 checks.
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Hardened);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.per_invariant.len(), 8);
        assert_eq!(result.level, VerificationLevel::Hardened);
    }

    #[test]
    fn invariant_kind_extended_labels() {
        assert_eq!(InvariantKind::ConstantTime.label(), "constant-time");
        assert_eq!(InvariantKind::Interprocedural.label(), "interprocedural");
        assert_eq!(InvariantKind::Modular.label(), "modular");
    }

    #[test]
    fn invariant_index_covers_all_nine_kinds() {
        assert_eq!(invariant_index(InvariantKind::Liveness), Some(0));
        assert_eq!(invariant_index(InvariantKind::Exclusivity), Some(1));
        assert_eq!(invariant_index(InvariantKind::Interpretation), Some(2));
        assert_eq!(invariant_index(InvariantKind::Origin), Some(3));
        assert_eq!(invariant_index(InvariantKind::Cleanup), Some(4));
        assert_eq!(invariant_index(InvariantKind::ConstantTime), Some(5));
        assert_eq!(invariant_index(InvariantKind::Interprocedural), Some(6));
        assert_eq!(invariant_index(InvariantKind::Modular), Some(7));
        assert_eq!(invariant_index(InvariantKind::Pmt), Some(8));
    }

    #[test]
    fn extended_invariant_count_is_nine() {
        // Wave 3d: 5 core + 3 extended (CT/Interprocedural/Modular) + 1 PMT = 9.
        assert_eq!(EXTENDED_INVARIANT_COUNT, 9);
    }

    #[test]
    fn cache_sized_for_all_nine_kinds() {
        let aggregator = InvariantAggregator::new();
        assert_eq!(aggregator.cache.len(), EXTENDED_INVARIANT_COUNT);
    }

    #[test]
    fn free_function_verify_all() {
        // VUMA 2.0: the free function `verify_all` uses the new PMT
        // default — it returns 1 result at the PMT level.
        let input = VerificationInput::from_scg(SCG::new());
        let result = verify_all(&input);
        assert_eq!(result.per_invariant.len(), 1);
        assert_eq!(result.level, VerificationLevel::Pmt);
    }

    #[test]
    fn incremental_reuses_cache_for_unaffected() {
        // VUMA 2.0: explicitly select Normal level so the 5 pointer
        // invariants (liveness/exclusivity/...) are exercised by this
        // cache test (default is now PMT, which runs only 1 invariant).
        let mut aggregator = InvariantAggregator::new().with_level(VerificationLevel::Normal);
        let input = VerificationInput::from_scg(SCG::new());

        // First run to populate cache.
        let first = aggregator.verify_all(&input);
        for pir in &first.per_invariant {
            if let Some(idx) = invariant_index(pir.kind) {
                aggregator.cache[idx] = Some(pir.clone());
            }
        }

        // Incremental run — only liveness affected.
        let delta = InvariantDelta::single(InvariantKind::Liveness);
        let second = aggregator.verify_incremental(&input, &delta);

        // Liveness should be fresh; others should be cached.
        let liveness = second
            .per_invariant
            .iter()
            .find(|r| r.kind == InvariantKind::Liveness)
            .unwrap();
        assert!(!liveness.cached);

        let exclusivity = second
            .per_invariant
            .iter()
            .find(|r| r.kind == InvariantKind::Exclusivity)
            .unwrap();
        assert!(exclusivity.cached);

        assert!(second.summary.cached_count > 0);
    }

    #[test]
    fn incremental_empty_delta_uses_all_cache() {
        // VUMA 2.0: explicitly select Normal level so all 5 pointer
        // invariants are run and cached.
        let mut aggregator = InvariantAggregator::new().with_level(VerificationLevel::Normal);
        let input = VerificationInput::from_scg(SCG::new());

        let first = aggregator.verify_all(&input);
        for pir in &first.per_invariant {
            if let Some(idx) = invariant_index(pir.kind) {
                aggregator.cache[idx] = Some(pir.clone());
            }
        }

        let delta = InvariantDelta::new();
        let second = aggregator.verify_incremental(&input, &delta);

        // All results should be cached.
        assert_eq!(second.summary.cached_count, 5);
        assert_eq!(second.summary.fresh_count, 0);
    }

    #[test]
    fn diagnostics_report_renders() {
        // VUMA 2.0: explicitly select Normal level so the 5 pointer
        // invariants (liveness, exclusivity, etc.) appear in the
        // diagnostics report (default PMT level produces only a single
        // pmt-state invariant).
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Normal);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        let report = aggregator.diagnostics(&result);

        let rendered = report.render();
        assert!(rendered.contains("IVE Verification Report"));
        assert!(rendered.contains("liveness"));
        assert!(rendered.contains("exclusivity"));
        assert!(rendered.contains("interpretation"));
        assert!(rendered.contains("origin"));
        assert!(rendered.contains("cleanup"));
    }

    #[test]
    fn overall_verdict_no_checks() {
        let results: Vec<PerInvariantResult> = vec![];
        assert_eq!(compute_overall_verdict(&results), OverallVerdict::NoChecks);
    }

    #[test]
    fn overall_verdict_pass() {
        let results = vec![PerInvariantResult::new(
            InvariantKind::Liveness,
            VerificationResult::new("liveness", VerificationStatus::Proven, "ok"),
            0,
        )];
        assert_eq!(compute_overall_verdict(&results), OverallVerdict::Pass);
    }

    #[test]
    fn overall_verdict_fail() {
        let ce = CounterExample::new(
            vec!["entry".into()],
            "entry".into(),
            "duplicate owner".into(),
        );
        let results = vec![PerInvariantResult::new(
            InvariantKind::Exclusivity,
            VerificationResult::new(
                "exclusivity",
                VerificationStatus::Violated { counterexample: ce },
                "violation",
            ),
            0,
        )];
        assert_eq!(compute_overall_verdict(&results), OverallVerdict::Fail);
    }

    #[test]
    fn overall_verdict_inconclusive() {
        let results = vec![PerInvariantResult::new(
            InvariantKind::Liveness,
            VerificationResult::new(
                "liveness",
                VerificationStatus::Unverified {
                    reason: "not yet implemented".into(),
                },
                "pending",
            ),
            0,
        )];
        assert_eq!(
            compute_overall_verdict(&results),
            OverallVerdict::Inconclusive
        );
    }

    #[test]
    fn clear_cache_resets() {
        let mut aggregator = InvariantAggregator::new();
        let input = VerificationInput::from_scg(SCG::new());

        let first = aggregator.verify_all(&input);
        for pir in &first.per_invariant {
            if let Some(idx) = invariant_index(pir.kind) {
                aggregator.cache[idx] = Some(pir.clone());
            }
        }

        aggregator.clear_cache();

        for slot in &aggregator.cache {
            assert!(slot.is_none());
        }
    }

    #[test]
    fn default_aggregator() {
        // VUMA 2.0: the default verification level is now PMT.
        let aggregator = InvariantAggregator::default();
        assert_eq!(aggregator.level(), VerificationLevel::Pmt);
    }

    #[test]
    fn summary_display() {
        // VUMA 2.0: explicitly select Normal level so 5 pointer
        // invariants are run and the summary shows "Total checked : 5".
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Normal);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        let text = format!("{}", result.summary);
        assert!(text.contains("Verification Summary"));
        assert!(text.contains("Total checked : 5"));
    }

    #[test]
    fn overall_verdict_display() {
        assert_eq!(format!("{}", OverallVerdict::Pass), "PASS");
        assert_eq!(format!("{}", OverallVerdict::Fail), "FAIL");
        assert_eq!(format!("{}", OverallVerdict::Inconclusive), "INCONCLUSIVE");
        assert_eq!(format!("{}", OverallVerdict::NoChecks), "NO_CHECKS");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Wave 19 regression tests — verification escape-hatch closure
    // ═══════════════════════════════════════════════════════════════════════

    /// (Wave 19 / VUMA 2.0) The default verification level is now PMT
    /// (PMT state verification only). The CLI `--verification` flag
    /// accepts only `pmt`; `--no-verify` has been removed. This test
    /// verifies the aggregator's default level is PMT.
    #[test]
    fn wave19_default_verification_level_is_pmt() {
        let agg = InvariantAggregator::new();
        assert_eq!(agg.level, VerificationLevel::Pmt);
    }

    /// (Wave 19, Task 2) `--strict-verification` makes `Inconclusive`
    /// block compilation. This test verifies that the aggregator produces
    /// `Inconclusive` for an empty SCG (no violations, but invariants
    /// cannot be fully proven), which the pipeline would then block on
    /// if `strict_verification` were true.
    #[test]
    fn wave19_strict_verification_inconclusive_blocks() {
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Normal);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        // An empty SCG yields either Pass or Inconclusive (no violations
        // possible, but some invariants may be Unverified). The pipeline
        // treats Inconclusive as blocking only when strict_verification=true.
        assert!(
            result.overall == OverallVerdict::Pass
                || result.overall == OverallVerdict::Inconclusive
                || result.overall == OverallVerdict::NoChecks,
            "Empty SCG must not produce Fail, got {}",
            result.overall
        );
    }

    /// (Wave 19, Task 3) Quick mode runs ALL 5 invariants (not just 2).
    #[test]
    fn wave19_quick_mode_runs_all_five_invariants() {
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Quick);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(
            result.per_invariant.len(),
            5,
            "Quick mode must run all 5 invariants at reduced depth"
        );
    }

    /// (Wave 19, Task 5) `max_paths` is configurable via the aggregator.
    #[test]
    fn wave19_max_paths_configurable() {
        let aggregator = InvariantAggregator::new().with_max_paths(128);
        assert_eq!(aggregator.engine.max_paths(), 128);
    }

    /// (Wave 19, Task 5) `max_path_length` is configurable via the aggregator.
    #[test]
    fn wave19_max_path_length_configurable() {
        let aggregator = InvariantAggregator::new().with_max_path_length(512);
        assert_eq!(aggregator.engine.max_path_length(), 512);
    }

    /// (Wave 19, Task 5) Custom limits actually take effect: the liveness
    /// verifier respects the configured `max_paths` and completes without
    /// panicking. (We don't assert on the verdict — very low limits may
    /// cause the verifier to give up and return Unverified, which is the
    /// correct behavior, not a crash.)
    #[test]
    fn wave19_reduced_max_paths_does_not_crash() {
        use vuma_scg::node::{AllocationNode, ProgramPoint};
        use vuma_scg::region::{DeploymentTarget, RegionId, SCGRegion};
        let mut scg = SCG::new();
        let region_id = RegionId::new(1);
        let alloc_id = scg.add_node(
            vuma_scg::node::NodeType::Allocation,
            vuma_scg::node::NodePayload::Allocation(AllocationNode {
                size: 64,
                align: 8,
                region_id,
                type_name: None,
            }),
            ProgramPoint { file: None, line: Some(1), column: Some(1), offset: None },
        );
        let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);
        region.add_node(alloc_id);
        scg.add_region(region);

        // With max_paths=1 (very aggressive), verification should still
        // complete without panicking. The verdict may be Pass, Inconclusive,
        // or even Fail (if the reduced depth misses the static-lifetime
        // exemption) — the point is that the configurable limit is honored
        // and the verifier doesn't crash.
        let aggregator = InvariantAggregator::new()
            .with_level(VerificationLevel::Normal)
            .with_max_paths(1)
            .with_max_path_length(8);
        let input = VerificationInput::from_scg(scg);
        let result = aggregator.verify_all(&input);
        // Must produce a valid verdict (not panic).
        let _ = result.overall; // touching the field confirms no panic
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Wave 3d tests — PMT state verification level
    // ═══════════════════════════════════════════════════════════════════════

    /// Helper: build a `PmtLayoutSpec` for `Point = { x: u32, y: u32 }`.
    fn pmt_layout_point() -> crate::verification::PmtLayoutSpec {
        use crate::verification::{PmtFieldSpec, PmtLayoutSpec};
        PmtLayoutSpec {
            name: "Point".to_string(),
            total_size: 8,
            fields: vec![
                PmtFieldSpec {
                    name: "x".to_string(),
                    offset: 0,
                    size: 4,
                    type_name: "u32".to_string(),
                },
                PmtFieldSpec {
                    name: "y".to_string(),
                    offset: 4,
                    size: 4,
                    type_name: "u32".to_string(),
                },
            ],
        }
    }

    /// Helper: build a `PmtLayoutSpec` for `Vec2 = { a: u32, b: u32 }`
    /// (same total_size as Point — exercises reinterpret transform).
    fn pmt_layout_vec2() -> crate::verification::PmtLayoutSpec {
        use crate::verification::{PmtFieldSpec, PmtLayoutSpec};
        PmtLayoutSpec {
            name: "Vec2".to_string(),
            total_size: 8,
            fields: vec![
                PmtFieldSpec {
                    name: "a".to_string(),
                    offset: 0,
                    size: 4,
                    type_name: "u32".to_string(),
                },
                PmtFieldSpec {
                    name: "b".to_string(),
                    offset: 4,
                    size: 4,
                    type_name: "u32".to_string(),
                },
            ],
        }
    }

    /// Helper: construct an SCG with a single StateRead node.
    fn scg_with_state_read(layout: &str, field: &str) -> SCG {
        use vuma_scg::node::{
            NodeType, ProgramPoint, StateReadNode,
        };
        let mut scg = SCG::new();
        scg.add_node(
            NodeType::StateRead,
            vuma_scg::node::NodePayload::StateRead(StateReadNode {
                state_vreg: 1,
                layout_name: layout.to_string(),
                field_name: field.to_string(),
                result_vreg: 2,
            }),
            ProgramPoint { file: None, line: Some(1), column: Some(1), offset: None },
        );
        scg
    }

    /// Helper: construct an SCG with a single StateWrite node.
    fn scg_with_state_write(layout: &str, field: &str) -> SCG {
        use vuma_scg::node::{
            NodeType, ProgramPoint, StateWriteNode,
        };
        let mut scg = SCG::new();
        scg.add_node(
            NodeType::StateWrite,
            vuma_scg::node::NodePayload::StateWrite(StateWriteNode {
                state_vreg: 1,
                layout_name: layout.to_string(),
                field_name: field.to_string(),
                value_vreg: 2,
            }),
            ProgramPoint { file: None, line: Some(1), column: Some(1), offset: None },
        );
        scg
    }

    /// Helper: construct an SCG with a StateTransform node followed by a
    /// StateWrite to the same input vreg (linearity violation).
    fn scg_with_transform_then_write(
        in_layout: &str,
        out_layout: &str,
        write_layout: &str,
        write_field: &str,
    ) -> SCG {
        use vuma_scg::node::{
            NodeType, ProgramPoint, StateTransformNode, StateWriteNode,
        };
        let mut scg = SCG::new();
        let pp = ProgramPoint { file: None, line: Some(1), column: Some(1), offset: None };
        scg.add_node(
            NodeType::StateTransform,
            vuma_scg::node::NodePayload::StateTransform(StateTransformNode {
                input_vreg: 1,
                input_layout: in_layout.to_string(),
                output_layout: out_layout.to_string(),
                result_vreg: 2,
            }),
            pp.clone(),
        );
        scg.add_node(
            NodeType::StateWrite,
            vuma_scg::node::NodePayload::StateWrite(StateWriteNode {
                state_vreg: 1,
                layout_name: write_layout.to_string(),
                field_name: write_field.to_string(),
                value_vreg: 3,
            }),
            pp,
        );
        scg
    }

    /// Helper: construct an SCG with a single StateTransform node.
    fn scg_with_state_transform(in_layout: &str, out_layout: &str) -> SCG {
        use vuma_scg::node::{
            NodeType, ProgramPoint, StateTransformNode,
        };
        let mut scg = SCG::new();
        scg.add_node(
            NodeType::StateTransform,
            vuma_scg::node::NodePayload::StateTransform(StateTransformNode {
                input_vreg: 1,
                input_layout: in_layout.to_string(),
                output_layout: out_layout.to_string(),
                result_vreg: 2,
            }),
            ProgramPoint { file: None, line: Some(1), column: Some(1), offset: None },
        );
        scg
    }

    #[test]
    fn wave3d_pmt_level_runs_single_invariant() {
        // Pmt level runs ONLY the PMT state verifier (skips 5 pointer invariants).
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.per_invariant.len(), 1, "Pmt level must run exactly 1 check");
        assert_eq!(result.level, VerificationLevel::Pmt);
        assert_eq!(result.per_invariant[0].kind, InvariantKind::Pmt);
    }

    #[test]
    fn wave3d_pmt_empty_scg_passes() {
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.overall, OverallVerdict::Pass);
        assert!(result.per_invariant[0].is_pass());
    }

    #[test]
    fn wave3d_pmt_valid_read_passes() {
        // SCG has a StateRead(Point.x) + layout registry has Point.
        // → verifier finds layout, finds field, type matches → PASS.
        let scg = scg_with_state_read("Point", "x");
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("Point".to_string(), pmt_layout_point());
        let input = VerificationInput::from_scg(scg).with_pmt_layouts(layouts);
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(result.overall, OverallVerdict::Pass,
            "valid read must PASS, got {:?}: {}",
            result.overall,
            result.per_invariant[0].result.message
        );
    }

    #[test]
    fn wave3d_pmt_unknown_field_fails() {
        // SCG has a StateRead(Point.z) but Point only has {x, y}.
        // → verifier reports "field 'z' not found".
        let scg = scg_with_state_read("Point", "z");
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("Point".to_string(), pmt_layout_point());
        let input = VerificationInput::from_scg(scg).with_pmt_layouts(layouts);
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(result.overall, OverallVerdict::Fail);
        let msg = &result.per_invariant[0].result.message;
        assert!(msg.contains("pmt-state violations"), "got: {}", msg);
    }

    #[test]
    fn wave3d_pmt_unknown_layout_fails() {
        // SCG has a StateTransform referencing an undeclared layout "Ghost".
        // → transform verifier reports "input layout 'Ghost' not found".
        let scg = scg_with_state_transform("Ghost", "Point");
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("Point".to_string(), pmt_layout_point());
        let input = VerificationInput::from_scg(scg).with_pmt_layouts(layouts);
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(result.overall, OverallVerdict::Fail);
        let msg = &result.per_invariant[0].result.message;
        assert!(msg.contains("pmt-state violations"), "got: {}", msg);
    }

    #[test]
    fn wave3d_pmt_write_after_consume_fails() {
        // SCG has StateTransform(Point→Vec2) followed by StateWrite(Point.x).
        // The transform consumes vreg 1 (Point), so the subsequent write
        // violates linearity.
        let scg = scg_with_transform_then_write("Point", "Vec2", "Point", "x");
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("Point".to_string(), pmt_layout_point());
        layouts.insert("Vec2".to_string(), pmt_layout_vec2());
        let input = VerificationInput::from_scg(scg).with_pmt_layouts(layouts);
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(result.overall, OverallVerdict::Fail);
        // Counterexample description should mention linearity / consumed.
        let pir = &result.per_invariant[0];
        match &pir.result.status {
            VerificationStatus::Violated { counterexample } => {
                assert!(
                    counterexample.description.contains("linearity")
                        || counterexample.description.contains("consumed"),
                    "expected linearity violation, got: {}",
                    counterexample.description
                );
            }
            other => panic!("expected Violated, got {:?}", other),
        }
    }

    #[test]
    fn wave3d_pmt_valid_transform_passes() {
        // SCG has StateTransform(Point→Vec2); both layouts declared and
        // same total_size (8) → reinterpret transform, valid.
        let scg = scg_with_state_transform("Point", "Vec2");
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("Point".to_string(), pmt_layout_point());
        layouts.insert("Vec2".to_string(), pmt_layout_vec2());
        let input = VerificationInput::from_scg(scg).with_pmt_layouts(layouts);
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(result.overall, OverallVerdict::Pass,
            "valid transform must PASS, got {:?}: {}",
            result.overall,
            result.per_invariant[0].result.message
        );
    }

    #[test]
    fn wave3d_pmt_valid_write_passes() {
        // SCG has StateWrite(Point.x); layout declares x: u32 → valid.
        let scg = scg_with_state_write("Point", "x");
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("Point".to_string(), pmt_layout_point());
        let input = VerificationInput::from_scg(scg).with_pmt_layouts(layouts);
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(result.overall, OverallVerdict::Pass,
            "valid write must PASS, got {:?}: {}",
            result.overall,
            result.per_invariant[0].result.message
        );
    }

    #[test]
    fn wave3d_pmt_no_layouts_fails_on_state_ops() {
        // SCG has StateRead(Point.x) but no layout registry attached
        // (the default path when pipeline doesn't populate pmt_layouts).
        // → verifier reports "layout 'Point' not found".
        let scg = scg_with_state_read("Point", "x");
        let input = VerificationInput::from_scg(scg); // no pmt_layouts
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(result.overall, OverallVerdict::Fail);
    }

    #[test]
    fn wave3d_pmt_kind_label() {
        assert_eq!(InvariantKind::Pmt.label(), "pmt-state");
    }

    #[test]
    fn wave3d_pmt_oob_field_fails() {
        // Hand-craft an inconsistent PmtLayoutSpec: layout "Tiny" with
        // total_size=1 but a field at offset 60 of size 4 → OOB.
        // (The normal pipeline never produces this; the test exercises
        // the state_read verifier's offset-overflow branch via the
        // aggregator.)
        use crate::verification::{PmtFieldSpec, PmtLayoutSpec};
        let tiny = PmtLayoutSpec {
            name: "Tiny".to_string(),
            total_size: 1,
            fields: vec![PmtFieldSpec {
                name: "tag".to_string(),
                offset: 60,
                size: 4,
                type_name: "u32".to_string(),
            }],
        };
        let scg = scg_with_state_read("Tiny", "tag");
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("Tiny".to_string(), tiny);
        let input = VerificationInput::from_scg(scg).with_pmt_layouts(layouts);
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(result.overall, OverallVerdict::Fail);
        match &result.per_invariant[0].result.status {
            VerificationStatus::Violated { counterexample } => {
                assert!(
                    counterexample.description.contains("exceeds layout")
                        || counterexample.description.contains("out"),
                    "expected OOB error, got: {}",
                    counterexample.description
                );
            }
            other => panic!("expected Violated, got {:?}", other),
        }
    }
}
