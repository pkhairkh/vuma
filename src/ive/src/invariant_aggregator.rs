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

use crate::result::{ConfidenceLevel, Evidence, ProofStep, VerificationResult, VerificationStatus};
use crate::verification::{VerificationEngine, VerificationInput};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Instant;

// ---------------------------------------------------------------------------
// InvariantKind
// ---------------------------------------------------------------------------

/// The five VUMA invariant kinds.
///
/// Each variant corresponds to one of the core safety invariants that
/// every VUMA program must satisfy, as defined in
/// `docs/specs/vuma-invariants-spec.md`. The canonical order below is the
/// order in which the spec lists the invariants (and the recommended
/// verification order is topological: Origin → Liveness →
/// (Exclusivity, Interpretation) → Cleanup).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum InvariantKind {
    /// **Liveness** — every access targets allocated memory.
    ///
    /// Guarantees the absence of use-after-free and out-of-bounds access:
    /// for each access `a`, the region of `a.target` must be in an
    /// allocated state at `a.program_point`, and the accessed byte range
    /// must be fully contained within that region.
    Liveness,
    /// **Exclusivity** — no conflicting concurrent accesses exist without
    /// synchronization.
    ///
    /// Any two accesses that conflict (at least one is a write, target the
    /// same region, and have overlapping byte ranges) must be ordered by a
    /// `SyncEdge` (HappensBefore / Atomic / Locked), preventing data races.
    Exclusivity,
    /// **Interpretation** — every access respects the Representation
    /// Descriptor (RepD) of its target.
    ///
    /// The effective RepD of the access's target derivation must be
    /// compatible with the RepD expected by the operation. Reading
    /// uninitialized memory as a pointer type is forbidden.
    Interpretation,
    /// **Origin** — every address traces to a valid allocation, and
    /// arithmetic derivations stay within bounds.
    ///
    /// Every derivation chain must terminate at a `Region` (no fabricated
    /// addresses), and offset arithmetic must remain within the source
    /// region's bounds.
    Origin,
    /// **Cleanup** — every allocation is eventually freed or explicitly
    /// leaked; no region is freed twice.
    ///
    /// Catches memory leaks (regions with no `free_point` and not marked
    /// `Leaked`) and double-frees (more than one free operation on the
    /// same region on any execution path).
    Cleanup,
    /// **Hardened invariants** — advanced flow-sensitive analyses that
    /// supplement the five basic invariants. Runs escape analysis,
    /// flow-sensitive CapD checking, aliasing integrity verification,
    /// and derivation-chain validation. Invoked at `Normal` and
    /// `Exhaustive` verification levels.
    Hardened,
    /// **Interprocedural invariants** — summary-based cross-function
    /// analysis. Builds a call graph, computes per-function summaries
    /// bottom-up, and detects cross-function leaks, data races,
    /// recursive leaks, and lock-discipline violations. Invoked at
    /// `Normal` and `Exhaustive` verification levels.
    Interprocedural,
    /// **Path-sensitive liveness** — refinement of the basic liveness
    /// check using meet-at-join dataflow. Computes per-point
    /// "definitely live on all paths" resource sets and flags
    /// use-after-free where a resource is accessed at a point at
    /// which it is not provably live. Invoked at `Normal` and
    /// `Exhaustive` verification levels.
    PathSensitiveLiveness,
}

impl InvariantKind {
    /// Return all five **basic** invariant kinds in canonical (spec)
    /// order: Liveness, Exclusivity, Interpretation, Origin, Cleanup.
    ///
    /// This is the set used by both the `Normal` and `Exhaustive`
    /// verification levels for the core checks. Advanced analyses
    /// (hardened, interprocedural, path-sensitive) are returned
    /// separately by [`InvariantKind::advanced`].
    pub fn all() -> &'static [InvariantKind; 5] {
        &[
            InvariantKind::Liveness,
            InvariantKind::Exclusivity,
            InvariantKind::Interpretation,
            InvariantKind::Origin,
            InvariantKind::Cleanup,
        ]
    }

    /// Return the three **advanced** invariant kinds (hardened,
    /// interprocedural, path-sensitive liveness).
    ///
    /// These are run as supplements to the basic invariants at
    /// `Normal` and `Exhaustive` verification levels. They are not
    /// part of [`InvariantKind::all`] so that the basic five-check
    /// loop remains stable, and they are never cached for incremental
    /// re-verification (they are always recomputed).
    pub fn advanced() -> &'static [InvariantKind; 3] {
        &[
            InvariantKind::Hardened,
            InvariantKind::Interprocedural,
            InvariantKind::PathSensitiveLiveness,
        ]
    }

    /// Return the cheap (quick-check) invariants.
    ///
    /// [`Exclusivity`] and [`Origin`](InvariantKind::Origin) can be
    /// verified by syntactic / structural analysis of the SCG and MSG
    /// (derivation forest well-formedness for Origin; conflict-pair /
    /// sync-edge reachability for Exclusivity) without the deep
    /// path-sensitive reasoning required for Liveness, Interpretation, and
    /// Cleanup. The `Quick` verification level runs only these two.
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
            InvariantKind::Hardened => "hardened_invariants",
            InvariantKind::Interprocedural => "interprocedural",
            InvariantKind::PathSensitiveLiveness => "path_sensitive_liveness",
        }
    }

    /// Returns `true` if this is one of the advanced (non-basic)
    /// invariant kinds returned by [`InvariantKind::advanced`].
    pub fn is_advanced(&self) -> bool {
        matches!(
            self,
            InvariantKind::Hardened
                | InvariantKind::Interprocedural
                | InvariantKind::PathSensitiveLiveness
        )
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
    /// Per-invariant results for the five basic invariants, in canonical
    /// order. This is always exactly 5 entries at `Normal`/`Exhaustive`
    /// levels (or 2 at `Quick` level).
    pub per_invariant: Vec<PerInvariantResult>,
    /// Results from the advanced supplementary analyses (hardened,
    /// interprocedural, path-sensitive liveness). Populated at
    /// `Normal` and `Exhaustive` verification levels; empty at
    /// `Quick` level.
    ///
    /// These are merged into the `overall` verdict and `summary`
    /// statistics: if any advanced analysis finds a violation, the
    /// overall verdict is `Fail`; if any is `Unverified`, the overall
    /// verdict is `Inconclusive` (unless a violation was also found).
    #[serde(default)]
    pub advanced_results: Vec<PerInvariantResult>,
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
    ///
    /// The report includes both the basic per-invariant results and the
    /// advanced supplementary analyses (hardened, interprocedural,
    /// path-sensitive liveness), so that violations found by the
    /// advanced analyses are visible in the rendered report.
    pub fn from_aggregated(result: &AggregatedResult) -> Self {
        let mut entries =
            Vec::with_capacity(result.per_invariant.len() + result.advanced_results.len());

        for pir in &result.per_invariant {
            entries.push(build_diagnostic_entry(pir));
        }

        // Append the advanced supplementary analyses so their results
        // appear in the rendered diagnostics report.
        for pir in &result.advanced_results {
            entries.push(build_diagnostic_entry(pir));
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
    ///
    /// At `Normal` and `Exhaustive` levels, this also runs the three
    /// advanced supplementary analyses (hardened, interprocedural,
    /// path-sensitive liveness) and merges their results into the
    /// `advanced_results` field, the `overall` verdict, and the
    /// `summary` statistics. At `Quick` level only the cheap syntactic
    /// checks (exclusivity, origin) are run and `advanced_results` is
    /// empty.
    pub fn verify_all(&self, input: &VerificationInput) -> AggregatedResult {
        let run_start = Instant::now();

        let invariants_to_run = self.invariants_for_level();
        let mut per_invariant = Vec::with_capacity(invariants_to_run.len());

        for &kind in &invariants_to_run {
            let check_start = Instant::now();
            let result = self.run_single_check(kind, input);
            let elapsed = check_start.elapsed().as_millis() as u64;

            per_invariant.push(PerInvariantResult::new(kind, result, elapsed));
        }

        // Run the advanced supplementary analyses at Normal+ levels.
        let advanced_results = self.run_advanced_checks(input);

        let total_elapsed = run_start.elapsed().as_millis() as u64;
        // Combine basic + advanced results for the summary and overall
        // verdict so that advanced violations are reflected in the
        // aggregated output.
        let combined: Vec<PerInvariantResult> = per_invariant
            .iter()
            .chain(advanced_results.iter())
            .cloned()
            .collect();
        let summary = VerificationSummary::from_results(&combined);
        let overall = compute_overall_verdict(&combined);

        AggregatedResult {
            per_invariant,
            advanced_results,
            overall,
            level: self.level,
            total_elapsed_ms: total_elapsed,
            summary,
        }
    }

    /// Run incremental verification: only re-check invariants affected
    /// by the given delta, reusing cached results for the rest.
    ///
    /// At `Normal` and `Exhaustive` levels the three advanced analyses
    /// are also run (they are never cached, so they are always
    /// recomputed) and merged into the result, just as in
    /// [`Self::verify_all`].
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

        // Run the advanced supplementary analyses at Normal+ levels.
        // These are never cached — they are always recomputed.
        let advanced_results = self.run_advanced_checks(input);

        let total_elapsed = run_start.elapsed().as_millis() as u64;
        // Combine basic + advanced results for the summary and overall
        // verdict so that advanced violations are reflected in the
        // aggregated output.
        let combined: Vec<PerInvariantResult> = per_invariant
            .iter()
            .chain(advanced_results.iter())
            .cloned()
            .collect();
        let summary = VerificationSummary::from_results(&combined);
        let overall = compute_overall_verdict(&combined);

        AggregatedResult {
            per_invariant,
            advanced_results,
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

    /// Return the set of **basic** invariants to check for the current
    /// level.
    ///
    /// At `Quick` level, only the cheap syntactic checks (exclusivity,
    /// origin) are run. At `Normal` and `Exhaustive` levels, all five
    /// basic invariants are run. The three advanced analyses (hardened,
    /// interprocedural, path-sensitive liveness) are run separately as
    /// a supplement at `Normal`+ levels — see
    /// [`InvariantAggregator::run_advanced_checks`].
    fn invariants_for_level(&self) -> Vec<InvariantKind> {
        match self.level {
            VerificationLevel::Quick => InvariantKind::quick_set().to_vec(),
            VerificationLevel::Normal | VerificationLevel::Exhaustive => {
                InvariantKind::all().to_vec()
            }
        }
    }

    /// Run the advanced supplementary analyses (hardened, interprocedural,
    /// path-sensitive liveness) at `Normal` and `Exhaustive` verification
    /// levels. Returns an empty vec at `Quick` level.
    ///
    /// Each advanced analysis is panic-safe: a panic in the underlying
    /// analysis is caught inside the `VerificationEngine` wrapper and
    /// returned as an `Unverified` result, so one failing analysis does
    /// not prevent the others from running.
    fn run_advanced_checks(&self, input: &VerificationInput) -> Vec<PerInvariantResult> {
        if self.level == VerificationLevel::Quick {
            return Vec::new();
        }

        let mut advanced = Vec::with_capacity(InvariantKind::advanced().len());
        for &kind in InvariantKind::advanced() {
            let check_start = Instant::now();
            let result = self.run_single_check(kind, input);
            let elapsed = check_start.elapsed().as_millis() as u64;
            advanced.push(PerInvariantResult::new(kind, result, elapsed));
        }
        advanced
    }

    /// Run a single invariant check by kind.
    ///
    /// For the five basic invariant kinds this dispatches to the
    /// corresponding `VerificationEngine::verify_*` method. For the
    /// three advanced kinds (Hardened, Interprocedural,
    /// PathSensitiveLiveness) this dispatches to the engine's advanced
    /// analysis methods, which are panic-safe (a panic in the underlying
    /// analysis is caught and returned as an `Unverified` result).
    fn run_single_check(
        &self,
        kind: InvariantKind,
        input: &VerificationInput,
    ) -> VerificationResult {
        if self.verbose {
            log::info!("InvariantAggregator: checking {kind}");
        }

        let mut result = match kind {
            InvariantKind::Liveness => self.engine.verify_liveness(input),
            InvariantKind::Exclusivity => self.engine.verify_exclusivity(input),
            InvariantKind::Interpretation => self.engine.verify_interpretation(input),
            InvariantKind::Origin => self.engine.verify_origin(input),
            InvariantKind::Cleanup => self.engine.verify_cleanup(input),
            InvariantKind::Hardened => self.engine.verify_hardened(input),
            InvariantKind::Interprocedural => self.engine.verify_interprocedural(input),
            InvariantKind::PathSensitiveLiveness => {
                self.engine.verify_liveness_path_sensitive(input)
            }
        };

        // In exhaustive mode, attach a formal-proof evidence record that
        // summarises the IVE's verification finding. The full proof
        // object (with goal, steps, and conclusion) is constructed
        // downstream in `vuma::api::build_proof_bundle`, which wraps
        // this evidence into a typed `LivenessProof` / `ExclusivityProof`
        // / ... struct. The steps below capture the key facts that the
        // downstream proof will reference — previously this was a
        // single-line placeholder string with no substantive content.
        if self.level == VerificationLevel::Exhaustive && result.is_proven() {
            let inv_label = kind.label();
            let level_str = format!("{}", self.level);
            let status_str = format!("{}", result.status);
            let confidence_str = format!("{}", result.confidence());
            let message = result.message.clone();
            result = result.with_evidence(Evidence::FormalProof {
                steps: vec![
                    format!(
                        "goal: prove {inv_label} for the full program (target = FullProgram)"
                    ),
                    format!("method: IVE verification engine at {level_str} level"),
                    format!("status: {status_str}"),
                    format!("confidence: {confidence_str}"),
                    format!("finding: {message}"),
                    format!(
                        "downstream: see vuma::api::build_proof_bundle for the constructed proof object"
                    ),
                ],
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
pub fn verify_all(input: &VerificationInput) -> AggregatedResult {
    InvariantAggregator::new().verify_all(input)
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

/// Map an invariant kind to a cache index (0..5) for the basic
/// invariants, or `None` for the advanced analyses.
///
/// Advanced analyses (Hardened, Interprocedural, PathSensitiveLiveness)
/// return `None` because they are not cached between runs — they are
/// always recomputed. This is by design: the advanced analyses are
/// relatively expensive and may depend on global state that the
/// incremental delta does not track.
fn invariant_index(kind: InvariantKind) -> Option<usize> {
    match kind {
        InvariantKind::Liveness => Some(0),
        InvariantKind::Exclusivity => Some(1),
        InvariantKind::Interpretation => Some(2),
        InvariantKind::Origin => Some(3),
        InvariantKind::Cleanup => Some(4),
        InvariantKind::Hardened
        | InvariantKind::Interprocedural
        | InvariantKind::PathSensitiveLiveness => None,
    }
}

/// Build a [`DiagnosticEntry`] from a [`PerInvariantResult`].
///
/// Used by [`DiagnosticsReport::from_aggregated`] for both the basic
/// per-invariant results and the advanced supplementary analyses, so
/// that violations found by either set of checks appear in the
/// rendered report.
fn build_diagnostic_entry(pir: &PerInvariantResult) -> DiagnosticEntry {
    let (icon, status_label) = match &pir.result.status {
        VerificationStatus::Proven => ("PASS".to_string(), "PROVEN".into()),
        VerificationStatus::ProbablySafe { .. } => ("PROB".to_string(), "PROBABLY_SAFE".into()),
        VerificationStatus::Unverified { .. } => ("????".to_string(), "UNVERIFIED".into()),
        VerificationStatus::Violated { .. } => ("FAIL".to_string(), "VIOLATED".into()),
    };

    DiagnosticEntry {
        kind: pir.kind,
        icon,
        status_label,
        message: pir.result.message.clone(),
        cached: pir.cached,
        elapsed_ms: pir.elapsed_ms,
    }
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
    fn invariant_kind_advanced_has_three() {
        // The advanced set is the three analyses wired in by task W3
        // (hardened, interprocedural, path-sensitive liveness).
        assert_eq!(InvariantKind::advanced().len(), 3);
        assert!(InvariantKind::advanced().contains(&InvariantKind::Hardened));
        assert!(InvariantKind::advanced().contains(&InvariantKind::Interprocedural));
        assert!(
            InvariantKind::advanced().contains(&InvariantKind::PathSensitiveLiveness)
        );
    }

    #[test]
    fn invariant_kind_is_advanced_flag() {
        // Basic invariants are not advanced.
        for kind in InvariantKind::all() {
            assert!(!kind.is_advanced(), "{:?} should not be advanced", kind);
        }
        // Advanced invariants are advanced.
        for kind in InvariantKind::advanced() {
            assert!(kind.is_advanced(), "{:?} should be advanced", kind);
        }
    }

    #[test]
    fn invariant_kind_advanced_labels() {
        assert_eq!(InvariantKind::Hardened.label(), "hardened_invariants");
        assert_eq!(InvariantKind::Interprocedural.label(), "interprocedural");
        assert_eq!(
            InvariantKind::PathSensitiveLiveness.label(),
            "path_sensitive_liveness"
        );
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
    fn verification_level_default_is_normal() {
        assert_eq!(VerificationLevel::default(), VerificationLevel::Normal);
    }

    #[test]
    fn verification_level_display() {
        assert_eq!(format!("{}", VerificationLevel::Quick), "QUICK");
        assert_eq!(format!("{}", VerificationLevel::Normal), "NORMAL");
        assert_eq!(format!("{}", VerificationLevel::Exhaustive), "EXHAUSTIVE");
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
    fn verify_all_normal_returns_five_results_plus_advanced() {
        // At Normal level, the aggregator runs the 5 basic invariants
        // (in `per_invariant`) plus the 3 advanced analyses (in
        // `advanced_results`). The `per_invariant` field stays at 5
        // for backward compatibility with external consumers.
        let aggregator = InvariantAggregator::new();
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.per_invariant.len(), 5);
        assert_eq!(result.advanced_results.len(), 3);
        assert_eq!(result.level, VerificationLevel::Normal);

        // The advanced results should be present and proven on an
        // empty SCG.
        for kind in InvariantKind::advanced() {
            let pir = result
                .advanced_results
                .iter()
                .find(|r| r.kind == *kind)
                .unwrap_or_else(|| panic!("advanced result {:?} missing", kind));
            assert!(
                pir.result.is_proven(),
                "advanced result {:?} should be proven on empty SCG, got: {}",
                kind,
                pir.result.status
            );
        }
    }

    #[test]
    fn verify_all_quick_returns_two_results_no_advanced() {
        // At Quick level, only the 2 cheap syntactic checks run,
        // and the advanced analyses are NOT run (advanced_results is
        // empty).
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Quick);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.per_invariant.len(), 2);
        assert_eq!(result.advanced_results.len(), 0);
        assert_eq!(result.level, VerificationLevel::Quick);
    }

    #[test]
    fn verify_all_exhaustive_returns_five_results_plus_advanced() {
        // At Exhaustive level, the aggregator runs the 5 basic + 3
        // advanced invariants. per_invariant stays at 5.
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Exhaustive);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.per_invariant.len(), 5);
        assert_eq!(result.advanced_results.len(), 3);
        assert_eq!(result.level, VerificationLevel::Exhaustive);
    }

    #[test]
    fn free_function_verify_all() {
        let input = VerificationInput::from_scg(SCG::new());
        let result = verify_all(&input);
        // 5 basic in per_invariant + 3 advanced in advanced_results.
        assert_eq!(result.per_invariant.len(), 5);
        assert_eq!(result.advanced_results.len(), 3);
        assert_eq!(result.level, VerificationLevel::Normal);
    }

    #[test]
    fn incremental_reuses_cache_for_unaffected() {
        let mut aggregator = InvariantAggregator::new();
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
        let mut aggregator = InvariantAggregator::new();
        let input = VerificationInput::from_scg(SCG::new());

        let first = aggregator.verify_all(&input);
        for pir in &first.per_invariant {
            if let Some(idx) = invariant_index(pir.kind) {
                aggregator.cache[idx] = Some(pir.clone());
            }
        }

        let delta = InvariantDelta::new();
        let second = aggregator.verify_incremental(&input, &delta);

        // The 5 basic invariants should be cached. The 3 advanced
        // analyses are never cached (invariant_index returns None),
        // so they are recomputed on every incremental run.
        assert_eq!(second.summary.cached_count, 5);
        assert_eq!(second.summary.fresh_count, 3);
    }

    #[test]
    fn diagnostics_report_renders() {
        let aggregator = InvariantAggregator::new();
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
        let aggregator = InvariantAggregator::default();
        assert_eq!(aggregator.level(), VerificationLevel::Normal);
    }

    #[test]
    fn summary_display() {
        let aggregator = InvariantAggregator::new();
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        let text = format!("{}", result.summary);
        assert!(text.contains("Verification Summary"));
        // At Normal level we run the 5 basic + 3 advanced invariants.
        assert!(text.contains("Total checked : 8"));
    }

    #[test]
    fn overall_verdict_display() {
        assert_eq!(format!("{}", OverallVerdict::Pass), "PASS");
        assert_eq!(format!("{}", OverallVerdict::Fail), "FAIL");
        assert_eq!(format!("{}", OverallVerdict::Inconclusive), "INCONCLUSIVE");
        assert_eq!(format!("{}", OverallVerdict::NoChecks), "NO_CHECKS");
    }
}
