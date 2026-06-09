//! Invariant aggregator for the IVE module.
//!
//! The [`InvariantAggregator`] runs all five VUMA invariant checks against a
//! message/SCG pair and produces a unified [`AggregatedResult`] that captures
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

use crate::inference::SCG;
use crate::result::{
    ConfidenceLevel, CounterExample, Evidence, ProofStep, VerificationResult, VerificationStatus,
};
use crate::verification::{Message, VerificationEngine};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Instant;

// ---------------------------------------------------------------------------
// InvariantKind
// ---------------------------------------------------------------------------

/// The five VUMA invariant kinds.
///
/// Each variant corresponds to one of the core safety invariants that
/// every VUMA program must satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
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
}

impl InvariantKind {
    /// Return all five invariant kinds in canonical order.
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
/// Controls which invariant checks are run and whether proofs are
/// attempted for properties that can be formally established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum VerificationLevel {
    /// Only run cheap, syntactic checks (exclusivity, origin).
    Quick,
    /// Run all five invariant checks (default).
    #[default]
    Normal,
    /// Run all checks and attempt formal proof generation.
    Exhaustive,
}

impl fmt::Display for VerificationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerificationLevel::Quick => write!(f, "QUICK"),
            VerificationLevel::Normal => write!(f, "NORMAL"),
            VerificationLevel::Exhaustive => write!(f, "EXHAUSTIVE"),
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
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
        let mut summary = Self::default();
        summary.total_checked = results.len();

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
        writeln!(
            f,
            "  Pass rate     : {:.0}%",
            self.pass_rate() * 100.0
        )?;
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticEntry {
    /// Which invariant this entry is about.
    pub kind: InvariantKind,
    /// Status icon ("✓", "✗", "~", "?").
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
                VerificationStatus::Proven => ("✓".to_string(), "PASS".into()),
                VerificationStatus::ProbablySafe { .. } => ("~".to_string(), "PROBABLY_SAFE".into()),
                VerificationStatus::Unverified { .. } => ("?".to_string(), "UNVERIFIED".into()),
                VerificationStatus::Violated { .. } => ("✗".to_string(), "FAIL".into()),
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
/// use vuma_ive::verification::Message;
/// use vuma_ive::inference::SCG;
///
/// let aggregator = InvariantAggregator::new();
/// let msg = Message::default();
/// let scg = SCG::default();
/// let result = aggregator.verify_all(&msg, &scg);
/// ```
pub struct InvariantAggregator {
    /// The underlying verification engine.
    engine: VerificationEngine,
    /// The verification level (default: Normal).
    level: VerificationLevel,
    /// Cached results from a previous run (for incremental verification).
    cache: Vec<Option<PerInvariantResult>>,
    /// Whether to emit verbose diagnostic output.
    verbose: bool,
}

impl InvariantAggregator {
    /// Construct a new invariant aggregator with default settings.
    pub fn new() -> Self {
        Self {
            engine: VerificationEngine::new(),
            level: VerificationLevel::Normal,
            cache: InvariantKind::all().iter().map(|_| None).collect(),
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
        self
    }

    /// Run all invariant checks (at the configured verification level)
    /// and return the aggregated result.
    pub fn verify_all(&self, msg: &Message, _scg: &SCG) -> AggregatedResult {
        let run_start = Instant::now();

        let invariants_to_run = self.invariants_for_level();
        let mut per_invariant = Vec::with_capacity(invariants_to_run.len());

        for &kind in &invariants_to_run {
            let check_start = Instant::now();
            let result = self.run_single_check(kind, msg);
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
        msg: &Message,
        _scg: &SCG,
        delta: &InvariantDelta,
    ) -> AggregatedResult {
        let run_start = Instant::now();
        let invariants_to_run = self.invariants_for_level();
        let mut per_invariant = Vec::with_capacity(invariants_to_run.len());

        for &kind in &invariants_to_run {
            if delta.affects(kind) {
                // Re-check this invariant.
                let check_start = Instant::now();
                let result = self.run_single_check(kind, msg);
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
                let result = self.run_single_check(kind, msg);
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
        self.cache = InvariantKind::all().iter().map(|_| None).collect();
    }

    /// Returns the current verification level.
    pub fn level(&self) -> VerificationLevel {
        self.level
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Return the set of invariants to check for the current level.
    fn invariants_for_level(&self) -> Vec<InvariantKind> {
        match self.level {
            VerificationLevel::Quick => InvariantKind::quick_set().to_vec(),
            VerificationLevel::Normal => InvariantKind::all().to_vec(),
            VerificationLevel::Exhaustive => InvariantKind::all().to_vec(),
        }
    }

    /// Run a single invariant check by kind.
    fn run_single_check(&self, kind: InvariantKind, msg: &Message) -> VerificationResult {
        if self.verbose {
            log::info!("InvariantAggregator: checking {kind}");
        }

        let mut result = match kind {
            InvariantKind::Liveness => self.engine.verify_liveness(msg),
            InvariantKind::Exclusivity => self.engine.verify_exclusivity(msg),
            InvariantKind::Interpretation => self.engine.verify_interpretation(msg),
            InvariantKind::Origin => self.engine.verify_origin(msg),
            InvariantKind::Cleanup => self.engine.verify_cleanup(msg),
        };

        // In exhaustive mode, attempt to attach proof evidence for
        // proven properties.
        if self.level == VerificationLevel::Exhaustive && result.is_proven() {
            result = result.with_evidence(Evidence::FormalProof {
                steps: vec![ProofStep::from(format!(
                    "proof of {} (placeholder)",
                    kind.label()
                ))],
            });
        }

        result
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

/// Convenience function: run all five invariant checks at the Normal
/// verification level and return the aggregated result.
pub fn verify_all(msg: &Message, scg: &SCG) -> AggregatedResult {
    InvariantAggregator::new().verify_all(msg, scg)
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

/// Map an invariant kind to a cache index (0..5).
fn invariant_index(kind: InvariantKind) -> Option<usize> {
    match kind {
        InvariantKind::Liveness => Some(0),
        InvariantKind::Exclusivity => Some(1),
        InvariantKind::Interpretation => Some(2),
        InvariantKind::Origin => Some(3),
        InvariantKind::Cleanup => Some(4),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::VerificationStatus;

    // -- InvariantKind tests --

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

    // -- VerificationLevel tests --

    #[test]
    fn verification_level_default_is_normal() {
        assert_eq!(VerificationLevel::default(), VerificationLevel::Normal);
    }

    #[test]
    fn verification_level_display() {
        assert_eq!(format!("{}", VerificationLevel::Quick), "QUICK");
        assert_eq!(format!("{}", VerificationLevel::Normal), "NORMAL");
        assert_eq!(format!("{}", VerificationLevel::Exhaustive), "EXHAUSTIVE");
    }

    // -- InvariantDelta tests --

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
        let delta =
            InvariantDelta::from_set([InvariantKind::Liveness, InvariantKind::Cleanup])
                .with_reason("resource change");
        assert!(delta.affects(InvariantKind::Liveness));
        assert!(delta.affects(InvariantKind::Cleanup));
        assert!(!delta.affects(InvariantKind::Origin));
        assert_eq!(delta.reason.as_deref(), Some("resource change"));
    }

    // -- Aggregator: full run --

    #[test]
    fn verify_all_normal_returns_five_results() {
        let aggregator = InvariantAggregator::new();
        let msg = Message::default();
        let scg = SCG::default();
        let result = aggregator.verify_all(&msg, &scg);
        assert_eq!(result.per_invariant.len(), 5);
        assert_eq!(result.level, VerificationLevel::Normal);
    }

    #[test]
    fn verify_all_normal_overall_is_inconclusive() {
        // All checks return Unverified, so overall should be Inconclusive.
        let aggregator = InvariantAggregator::new();
        let msg = Message::default();
        let scg = SCG::default();
        let result = aggregator.verify_all(&msg, &scg);
        assert_eq!(result.overall, OverallVerdict::Inconclusive);
    }

    #[test]
    fn verify_all_quick_returns_two_results() {
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Quick);
        let msg = Message::default();
        let scg = SCG::default();
        let result = aggregator.verify_all(&msg, &scg);
        assert_eq!(result.per_invariant.len(), 2);
        assert_eq!(result.level, VerificationLevel::Quick);
    }

    #[test]
    fn verify_all_exhaustive_returns_five_results() {
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Exhaustive);
        let msg = Message::default();
        let scg = SCG::default();
        let result = aggregator.verify_all(&msg, &scg);
        assert_eq!(result.per_invariant.len(), 5);
        assert_eq!(result.level, VerificationLevel::Exhaustive);
    }

    // -- Free function --

    #[test]
    fn free_function_verify_all() {
        let msg = Message::default();
        let scg = SCG::default();
        let result = verify_all(&msg, &scg);
        assert_eq!(result.per_invariant.len(), 5);
        assert_eq!(result.level, VerificationLevel::Normal);
    }

    // -- Summary --

    #[test]
    fn summary_from_all_unverified() {
        let aggregator = InvariantAggregator::new();
        let msg = Message::default();
        let scg = SCG::default();
        let result = aggregator.verify_all(&msg, &scg);

        assert_eq!(result.summary.total_checked, 5);
        assert_eq!(result.summary.unverified, 5);
        assert_eq!(result.summary.passed, 0);
        assert_eq!(result.summary.failed, 0);
        assert!(!result.summary.is_all_pass());
        assert_eq!(result.summary.min_confidence, Some(ConfidenceLevel::Low));
    }

    #[test]
    fn summary_pass_rate_zero_when_all_unverified() {
        let aggregator = InvariantAggregator::new();
        let msg = Message::default();
        let scg = SCG::default();
        let result = aggregator.verify_all(&msg, &scg);
        assert!((result.summary.pass_rate() - 0.0).abs() < f64::EPSILON);
    }

    // -- Incremental verification --

    #[test]
    fn incremental_reuses_cache_for_unaffected() {
        let mut aggregator = InvariantAggregator::new();
        let msg = Message::default();
        let scg = SCG::default();

        // First run to populate cache.
        let first = aggregator.verify_all(&msg, &scg);
        // Manually populate cache from first run.
        for pir in &first.per_invariant {
            if let Some(idx) = invariant_index(pir.kind) {
                aggregator.cache[idx] = Some(pir.clone());
            }
        }

        // Incremental run — only liveness affected.
        let delta = InvariantDelta::single(InvariantKind::Liveness);
        let second = aggregator.verify_incremental(&msg, &scg, &delta);

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
        let mut aggregator = InvariantAggregator::new();
        let msg = Message::default();
        let scg = SCG::default();

        let first = aggregator.verify_all(&msg, &scg);
        for pir in &first.per_invariant {
            if let Some(idx) = invariant_index(pir.kind) {
                aggregator.cache[idx] = Some(pir.clone());
            }
        }

        let delta = InvariantDelta::new();
        let second = aggregator.verify_incremental(&msg, &scg, &delta);

        // All results should be cached.
        assert_eq!(second.summary.cached_count, 5);
        assert_eq!(second.summary.fresh_count, 0);
    }

    // -- Diagnostics report --

    #[test]
    fn diagnostics_report_renders() {
        let aggregator = InvariantAggregator::new();
        let msg = Message::default();
        let scg = SCG::default();
        let result = aggregator.verify_all(&msg, &scg);
        let report = aggregator.diagnostics(&result);

        let rendered = report.render();
        assert!(rendered.contains("IVE Verification Report"));
        assert!(rendered.contains("INCONCLUSIVE"));
        assert!(rendered.contains("liveness"));
        assert!(rendered.contains("exclusivity"));
        assert!(rendered.contains("interpretation"));
        assert!(rendered.contains("origin"));
        assert!(rendered.contains("cleanup"));
    }

    #[test]
    fn diagnostics_report_display_delegates_to_render() {
        let aggregator = InvariantAggregator::new();
        let msg = Message::default();
        let scg = SCG::default();
        let result = aggregator.verify_all(&msg, &scg);
        let report = aggregator.diagnostics(&result);

        let via_render = report.render();
        let via_display = format!("{report}");
        assert_eq!(via_render, via_display);
    }

    // -- Overall verdict computation --

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

    // -- Clear cache --

    #[test]
    fn clear_cache_resets() {
        let mut aggregator = InvariantAggregator::new();
        let msg = Message::default();
        let scg = SCG::default();

        let first = aggregator.verify_all(&msg, &scg);
        for pir in &first.per_invariant {
            if let Some(idx) = invariant_index(pir.kind) {
                aggregator.cache[idx] = Some(pir.clone());
            }
        }

        aggregator.clear_cache();

        // All cache slots should be None.
        for slot in &aggregator.cache {
            assert!(slot.is_none());
        }
    }

    // -- PerInvariantResult helpers --

    #[test]
    fn per_invariant_result_pass_and_fail() {
        let pass = PerInvariantResult::new(
            InvariantKind::Liveness,
            VerificationResult::new("liveness", VerificationStatus::Proven, "ok"),
            5,
        );
        assert!(pass.is_pass());
        assert!(!pass.is_fail());
        assert!(!pass.is_unverified());

        let ce = CounterExample::new(vec![], "x".into(), "bad".into());
        let fail = PerInvariantResult::new(
            InvariantKind::Cleanup,
            VerificationResult::new(
                "cleanup",
                VerificationStatus::Violated { counterexample: ce },
                "leak",
            ),
            10,
        );
        assert!(fail.is_fail());
        assert!(!fail.is_pass());

        let unv = PerInvariantResult::new(
            InvariantKind::Origin,
            VerificationResult::new(
                "origin",
                VerificationStatus::Unverified {
                    reason: "todo".into(),
                },
                "pending",
            ),
            1,
        );
        assert!(unv.is_unverified());
    }

    // -- Default aggregator --

    #[test]
    fn default_aggregator() {
        let aggregator = InvariantAggregator::default();
        assert_eq!(aggregator.level(), VerificationLevel::Normal);
    }

    // -- Summary display --

    #[test]
    fn summary_display() {
        let aggregator = InvariantAggregator::new();
        let msg = Message::default();
        let scg = SCG::default();
        let result = aggregator.verify_all(&msg, &scg);
        let text = format!("{}", result.summary);
        assert!(text.contains("Verification Summary"));
        assert!(text.contains("Total checked : 5"));
        assert!(text.contains("Unverified    : 5"));
    }

    // -- OverallVerdict display --

    #[test]
    fn overall_verdict_display() {
        assert_eq!(format!("{}", OverallVerdict::Pass), "PASS");
        assert_eq!(format!("{}", OverallVerdict::Fail), "FAIL");
        assert_eq!(format!("{}", OverallVerdict::Inconclusive), "INCONCLUSIVE");
        assert_eq!(format!("{}", OverallVerdict::NoChecks), "NO_CHECKS");
    }
}
