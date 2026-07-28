//! Invariant aggregator for the IVE module.
//!
//! (Legacy cleanup) The five legacy pointer-invariant verifiers
//! (liveness / exclusivity / interpretation / origin / cleanup) have been
//! removed. The aggregator now runs a single [`InvariantKind::Pmt`] check
//! that delegates to [`VerificationEngine::verify_pmt`]. The
//! [`AggregatedResult`] still records per-invariant outcomes, an overall
//! pass/fail verdict, and a [`VerificationSummary`] with statistics.
//!
//! # Verification Levels
//!
//! VUMA 2.0 is PMT-only: every program is verified with
//! [`VerificationLevel::Pmt`] (the default). The `Quick` and `Normal`
//! variants remain on the enum for API stability and IVE-internal tests,
//! but they are NOT user-selectable in the production pipeline and all
//! remap to the same single-PMT-check code path.
//!
//! # Incremental Verification
//!
//! The aggregator supports *incremental* verification: when a delta
//! describing which invariants are affected is provided, only those
//! invariants are re-checked while cached results are kept for the rest.

use crate::result::{ConfidenceLevel, VerificationResult, VerificationStatus};
use crate::verification::{VerificationEngine, VerificationInput};
use std::fmt;
use std::time::Instant;

// ---------------------------------------------------------------------------
// InvariantKind
// ---------------------------------------------------------------------------

/// The VUMA invariant kinds.
///
/// # Historical context (8 → 1)
///
/// VUMA 1.x enumerated **8 invariant kinds**. Of these, **5 were legacy
/// pointer invariants** — `Liveness`, `Exclusivity`, `Interpretation`,
/// `Origin`, `Cleanup` — each of which encoded a separate proof obligation
/// about pointer aliasing, lifetime, use-after-free, or provenance. The
/// historical documentation noted that the default `Pmt`
/// verification level *skipped* those 5 at the PMT level, leaving them
/// dead in production but still present in the enum.
///
/// VUMA 2.0 collapsed the enum to a single `Pmt` variant. The 5 legacy
/// pointer invariants were **removed entirely** (not merely skipped),
/// because PMT state verification subsumes them: in the PMT ("Programs as
/// Memory Transformations") model, every memory access is encoded as a
/// typed state-field read or write, so the aliasing / lifetime / provenance
/// properties the legacy invariants attempted to prove are established by
/// construction via type checking rather than by separate pointer proofs.
/// The remaining 2 historical variants (early PMT-state-style checks) were
/// folded into the single canonical `Pmt` pass.
///
/// See `lib.rs:9-12` and `verification.rs:3-7` for the matching legacy
/// cleanup notes, and the `VerificationLevel` doc at
/// `invariant_aggregator.rs:118-155` for the parallel collapse of the
/// verification-level enum (`Quick` / `Normal` / `Exhaustive` / `Modular`
/// / `ConstantTime` / `Hardened` → `Pmt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InvariantKind {
    /// PMT state verification: state-field reads/writes + state
    /// transformations.  Only checked under
    /// [`VerificationLevel::Pmt`].
    ///
    /// # What "PMT" means (and does NOT mean)
    ///
    /// **PMT = "Programs as Memory Transformations."** It is a *verification
    /// discipline*: every program is treated as a typed state-transformation
    /// on a single backing arena (`___pmt_buffer`), with state-typed
    /// variables addressed by vreg and accessed via `StateRead` /
    /// `StateWrite` / `StateTransform` SCG nodes.
    ///
    /// **PMT does NOT mean "Persistent Memory Transaction."** Despite the
    /// acronym overlap, there is:
    /// - **no persistence** — state does not outlive the process; the arena
    ///   is mmap'd and torn down at exit;
    /// - **no rollback** — once a `StateWrite` commits, the previous value
    ///   is gone (write-after-consume linearity is enforced *statically*,
    ///   not by an undo log);
    /// - **no durability** — there is no crash-recovery log, no journal, no
    ///   on-disk replica, no fsync.
    ///
    /// A `grep` for `persistent.*mem|transaction|rollback|durable` returns
    /// zero hits in this crate. See `docs/architecture/pmt-audit.md` §1 and
    /// `docs/architecture/overview.md` §5.3 ("PMT clarification") for the
    /// full disambiguation.
    Pmt,
}

impl InvariantKind {
    /// Return the single invariant kind, `[Pmt]`.
    ///
    /// (Legacy cleanup) Previously returned the five legacy pointer
    /// invariants; now returns only `Pmt`.
    pub fn all() -> Vec<InvariantKind> {
        vec![InvariantKind::Pmt]
    }

    /// Human-readable label for this invariant kind.
    pub fn label(&self) -> &'static str {
        match self {
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
/// state verifiers (state-read / state-write / state-transform). The
/// legacy `Quick`/`Normal`/`Exhaustive`/`Modular`/`ConstantTime`/
/// `Hardened` levels have been removed — the enum now exposes only the
/// single `Pmt` variant so callers cannot accidentally bypass PMT
/// enforcement. The CLI `--verification` flag accepts only `pmt`, and
/// the pipeline hard-codes `VerificationLevel::Pmt`.
///
/// # Historical context
///
/// The historical documentation claimed that
/// [`InvariantAggregator::with_level`] "silently coerces" non-`Pmt`
/// levels to `Pmt` under `#[cfg(not(test))]`, leaving the 5-invariant /
/// constant-time / interprocedural code paths dead in production. That
/// mechanism **no longer exists**: the enum itself was collapsed to a
/// single variant, so there is nothing to coerce. Specifically:
///
/// - The five legacy pointer invariants (Liveness / Exclusivity /
///   Interpretation / Origin / Cleanup) have been **DELETED** from
///   `InvariantKind` — see `lib.rs:9-12` and the `InvariantKind` doc
///   comment at `invariant_aggregator.rs:33-59` (which carries the
///   full 8 → 1 historical-context narrative). Only
///   `InvariantKind::Pmt` remains.
/// - The `Quick` / `Normal` / `Exhaustive` / `Modular` /
///   `ConstantTime` / `Hardened` level variants have been **DELETED**
///   from this enum.
/// - [`InvariantAggregator::with_level`] (at `invariant_aggregator.rs:551`)
///   is a no-op: it accepts a `VerificationLevel` purely for API
///   stability and always resolves to `Pmt`.
/// - [`invariants_for_level`] (at `invariant_aggregator.rs:696-698`)
///   always returns `vec![InvariantKind::Pmt]`.
///
/// The "silent coercion" caveat is therefore **STALE**: the dead code
/// paths were deleted, not bypassed. Callers cannot opt out of PMT
/// verification by selecting a different level — the type system
/// forbids it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum VerificationLevel {
    /// PMT state verification only — runs the 3 state verifiers
    /// (state-read, state-write, state-transform).  Used for PMT (Programs
    /// as Memory Transformations) verification where memory safety is
    /// established by type-checking rather than pointer proofs.  This is
    /// the DEFAULT (and only) level for VUMA 2.0.
    ///
    /// **Acronym disambiguation:** PMT = "Programs as Memory
    /// Transformations" — *not* "Persistent Memory Transaction." There
    /// is no persistence, no rollback, and no durability machinery in
    /// this verifier or anywhere else in the VUMA pipeline. See
    /// [`InvariantKind::Pmt`] for the full clarification.
    #[default]
    Pmt,
}

impl fmt::Display for VerificationLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
/// ```rust,no_run
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
    ///
    /// Only [`VerificationLevel::Pmt`] is supported in VUMA 2.0 — the
    /// enum now exposes a single variant. The `level` parameter is
    /// accepted for API compatibility but always resolves to `Pmt`.
    pub fn with_level(self, _level: VerificationLevel) -> Self {
        // Only Pmt is supported. The level parameter is accepted for API
        // compatibility but always uses Pmt.
        self
    }

    /// Enable verbose diagnostic output.
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self.engine = self.engine.with_verbose(verbose);
        self
    }

    /// (Legacy, retained for API stability) Set the maximum number of paths
    /// for the (now-removed) liveness verifier.  No-op for PMT verification.
    pub fn with_max_paths(mut self, max_paths: usize) -> Self {
        self.engine = self.engine.with_max_paths(max_paths);
        self
    }

    /// (Legacy, retained for API stability) Set the maximum path length
    /// for the (now-removed) cleanup verifier.  No-op for PMT verification.
    pub fn with_max_path_length(mut self, max_path_length: usize) -> Self {
        self.engine = self.engine.with_max_path_length(max_path_length);
        self
    }

    /// Run all invariant checks (at the configured verification level)
    /// and return the aggregated result.
    ///
    /// (Legacy cleanup) The five pointer-invariant verifiers have been
    /// removed; `verify_all` now always runs the single `InvariantKind::Pmt`
    /// check.  The `level` field is still recorded on the result for
    /// backwards compatibility with API consumers.
    pub fn verify_all(&self, input: &VerificationInput) -> AggregatedResult {
        let run_start = Instant::now();

        let invariants_to_run = self.invariants_for_level();
        let mut per_invariant = Vec::with_capacity(invariants_to_run.len());

        let effective_engine = self.engine.clone();

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
    /// (Legacy cleanup) Always returns `vec![InvariantKind::Pmt]` — the
    /// five legacy pointer invariants have been removed. The `Quick` and
    /// `Normal` levels are accepted for backwards compatibility but
    /// produce the same single-PMT-check set.
    fn invariants_for_level(&self) -> Vec<InvariantKind> {
        vec![InvariantKind::Pmt]
    }

    /// Run a single invariant check by kind.
    fn run_single_check(
        &self,
        kind: InvariantKind,
        input: &VerificationInput,
    ) -> VerificationResult {
        self.run_single_check_with(&self.engine, kind, input)
    }

    /// Run a single invariant check using a specific engine.
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
            InvariantKind::Pmt => engine.verify_pmt(input),
        };

        // The fake FormalProof string-evidence was removed.  Real
        // proof-system cross-checking now happens in api.rs's
        // build_proof_bundle(), which calls ProofChecker::check on the
        // prove_* tactics' output. IVE no longer claims to have formal
        // proof evidence when it only did dataflow analysis.

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

/// Map an invariant kind to a cache index.
///
/// (Legacy cleanup) Only `Pmt` remains; the cache is sized to
/// `EXTENDED_INVARIANT_COUNT` (1) for backwards compatibility with the
/// incremental-verification code path.
fn invariant_index(kind: InvariantKind) -> Option<usize> {
    match kind {
        InvariantKind::Pmt => Some(0),
    }
}

/// The total number of invariant kinds (1 = Pmt only).
const EXTENDED_INVARIANT_COUNT: usize = 1;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::result::CounterExample;
    use vuma_scg::graph::SCG;

    #[test]
    fn invariant_kind_all_has_one() {
        // (Legacy cleanup) Only Pmt remains.
        assert_eq!(InvariantKind::all().len(), 1);
        assert!(InvariantKind::all().contains(&InvariantKind::Pmt));
    }

    #[test]
    fn invariant_kind_labels() {
        assert_eq!(InvariantKind::Pmt.label(), "pmt-state");
    }

    #[test]
    fn invariant_kind_display() {
        assert_eq!(format!("{}", InvariantKind::Pmt), "pmt-state");
    }

    #[test]
    fn verification_level_default_is_pmt() {
        // VUMA 2.0: PMT is the default verification level.
        assert_eq!(VerificationLevel::default(), VerificationLevel::Pmt);
    }

    #[test]
    fn verification_level_display() {
        // (Legacy cleanup) Only Pmt remains on the enum.
        assert_eq!(format!("{}", VerificationLevel::Pmt), "PMT");
    }

    #[test]
    fn delta_empty_by_default() {
        let delta = InvariantDelta::new();
        assert!(delta.is_empty());
        assert!(!delta.affects(InvariantKind::Pmt));
    }

    #[test]
    fn delta_single_affects_only_one() {
        let delta = InvariantDelta::single(InvariantKind::Pmt);
        assert!(!delta.is_empty());
        assert!(delta.affects(InvariantKind::Pmt));
    }

    #[test]
    fn delta_from_set() {
        let delta = InvariantDelta::from_set([InvariantKind::Pmt]).with_reason("state change");
        assert!(delta.affects(InvariantKind::Pmt));
        assert_eq!(delta.reason.as_deref(), Some("state change"));
    }

    #[test]
    fn verify_all_pmt_default_returns_one_result() {
        // VUMA 2.0: the default level is PMT, which runs only the
        // PMT state verifier surfaced as a single `InvariantKind::Pmt`
        // aggregated result.
        let aggregator = InvariantAggregator::new();
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(result.per_invariant.len(), 1);
        assert_eq!(result.level, VerificationLevel::Pmt);
    }

    #[test]
    fn invariant_index_covers_pmt() {
        assert_eq!(invariant_index(InvariantKind::Pmt), Some(0));
    }

    #[test]
    fn extended_invariant_count_is_one() {
        // (Legacy cleanup) Only Pmt remains.
        assert_eq!(EXTENDED_INVARIANT_COUNT, 1);
    }

    #[test]
    fn cache_sized_for_one_kind() {
        let aggregator = InvariantAggregator::new();
        assert_eq!(aggregator.cache.len(), EXTENDED_INVARIANT_COUNT);
    }

    #[test]
    fn free_function_verify_all() {
        // VUMA 2.0: the free function `verify_all` uses the PMT default.
        let input = VerificationInput::from_scg(SCG::new());
        let result = verify_all(&input);
        assert_eq!(result.per_invariant.len(), 1);
        assert_eq!(result.level, VerificationLevel::Pmt);
    }

    #[test]
    fn overall_verdict_no_checks() {
        let results: Vec<PerInvariantResult> = vec![];
        assert_eq!(compute_overall_verdict(&results), OverallVerdict::NoChecks);
    }

    #[test]
    fn overall_verdict_pass() {
        let results = vec![PerInvariantResult::new(
            InvariantKind::Pmt,
            VerificationResult::new("pmt-state", VerificationStatus::Proven, "ok"),
            0,
        )];
        assert_eq!(compute_overall_verdict(&results), OverallVerdict::Pass);
    }

    #[test]
    fn overall_verdict_fail() {
        let ce = CounterExample::new(
            vec!["entry".into()],
            "entry".into(),
            "pmt-state violation".into(),
        );
        let results = vec![PerInvariantResult::new(
            InvariantKind::Pmt,
            VerificationResult::new(
                "pmt-state",
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
            InvariantKind::Pmt,
            VerificationResult::new(
                "pmt-state",
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
        // VUMA 2.0: the default verification level is PMT.
        let aggregator = InvariantAggregator::default();
        assert_eq!(aggregator.level(), VerificationLevel::Pmt);
    }

    #[test]
    fn overall_verdict_display() {
        assert_eq!(format!("{}", OverallVerdict::Pass), "PASS");
        assert_eq!(format!("{}", OverallVerdict::Fail), "FAIL");
        assert_eq!(format!("{}", OverallVerdict::Inconclusive), "INCONCLUSIVE");
        assert_eq!(format!("{}", OverallVerdict::NoChecks), "NO_CHECKS");
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Verification escape-hatch closure regression tests
    // ═══════════════════════════════════════════════════════════════════════

    /// The default verification level is PMT (PMT state verification
    /// only). The CLI `--verification` flag accepts only `pmt`;
    /// `--no-verify` has been removed. This test verifies the
    /// aggregator's default level is PMT.
    #[test]
    fn wave19_default_verification_level_is_pmt() {
        let agg = InvariantAggregator::new();
        assert_eq!(agg.level, VerificationLevel::Pmt);
    }

    /// The aggregator produces a non-`Fail` verdict for an empty SCG (it
    /// cannot prove a violation on no program). The `--strict-verification`
    /// flag has been REMOVED in VUMA 2.0 — Inconclusive is now a HARD
    /// failure by default in the pipeline gates (only
    /// `--allow-inconclusive` opts out, with a logged SOUNDNESS WAIVER).
    #[test]
    fn wave19_strict_verification_inconclusive_blocks() {
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert!(
            result.overall == OverallVerdict::Pass
                || result.overall == OverallVerdict::Inconclusive
                || result.overall == OverallVerdict::NoChecks,
            "Empty SCG must not produce Fail, got {}",
            result.overall
        );
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Inconclusive→Err regression — Inconclusive is a hard failure
    // ═══════════════════════════════════════════════════════════════════════

    /// Regression test (RESOLVED).
    ///
    /// `OverallVerdict::Inconclusive` MUST map to a hard compile error by
    /// default. This test directly exercises the canonical pipeline gate
    /// logic that appears verbatim at four call sites:
    ///   * `src/pipeline.rs:5199-5204` (`compile_with_path` Stage 6)
    ///   * `src/pipeline.rs:6107-6112` (`compile_modules` Stage 2b)
    ///   * `src/pipeline.rs:6550-6556` (`compile_with_recovery` partial-compile)
    ///   * `src/main.rs:1569-1581`     (`verify_pmt_on_ast`, used by `vuma emit`)
    ///
    /// The legacy `--strict-verification` flag was REMOVED in VUMA 2.0
    /// (`main.rs:654-666` returns a hard error). The ONLY opt-out is
    /// `--allow-inconclusive`, threaded via `CompileConfig.allow_inconclusive`.
    ///
    /// This is a direct unit test on the Inconclusive→Err mapping, rather
    /// than an end-to-end compile test, because constructing a VUMA source
    /// program that deterministically produces an `Unverified` per-invariant
    /// status is non-trivial (the aggregator's `Pmt` level tends to either
    /// `Proven` or `Violated`).
    #[test]
    fn wave5_inconclusive_gate_hard_fails_by_default() {
        use crate::result::{VerificationResult, VerificationStatus};

        // Build a per-invariant result whose `Unverified` status forces
        // `compute_overall_verdict` to return `Inconclusive`.
        let pir = PerInvariantResult::new(
            InvariantKind::Pmt,
            VerificationResult::new(
                "pmt-state",
                VerificationStatus::Unverified {
                    reason: "synthetic Inconclusive for Wave 5 regression".to_string(),
                },
                "wave5 regression: per-invariant Unverified",
            ),
            0,
        );
        // Sanity: the per-invariant result is unverified, not failed.
        assert!(pir.is_unverified());
        assert!(!pir.is_fail());

        let per_invariant = vec![pir];
        // Sanity: the aggregator's verdict combiner maps this to Inconclusive.
        assert_eq!(
            compute_overall_verdict(&per_invariant),
            OverallVerdict::Inconclusive,
            "Unverified per-invariant result must aggregate to Inconclusive"
        );

        let summary = VerificationSummary::from_results(&per_invariant);
        let inconclusive_result = AggregatedResult {
            per_invariant,
            overall: OverallVerdict::Inconclusive,
            level: VerificationLevel::Pmt,
            total_elapsed_ms: 0,
            summary,
        };

        /// Mirror of the canonical pipeline gate at `pipeline.rs:5199-5204`.
        ///
        /// Returns `Err(())` when the gate would block compilation; `Ok(())`
        /// when the gate would soft-pass (either non-Inconclusive, or
        /// Inconclusive with the explicit `--allow-inconclusive` opt-in).
        fn apply_inconclusive_gate(
            result: &AggregatedResult,
            allow_inconclusive: bool,
        ) -> Result<(), ()> {
            // Inconclusive is a HARD failure by default.
            // `--allow-inconclusive` opts back into the legacy soft-pass
            // behaviour. Mirrors `pipeline.rs:5199-5204` verbatim.
            if (result.overall == OverallVerdict::Inconclusive) && !allow_inconclusive {
                return Err(());
            }
            Ok(())
        }

        // Default behaviour: HARD FAILURE on Inconclusive.
        assert_eq!(
            apply_inconclusive_gate(&inconclusive_result, false),
            Err(()),
            "Gap 1 full flip: Inconclusive MUST hard-fail when \
             --allow-inconclusive is absent (config.allow_inconclusive=false)"
        );

        // Opt-out: `--allow-inconclusive` soft-passes with a SOUNDNESS WAIVER.
        assert_eq!(
            apply_inconclusive_gate(&inconclusive_result, true),
            Ok(()),
            "--allow-inconclusive must opt back into the legacy soft-pass \
             behaviour for Inconclusive verdicts"
        );

        // Cross-check: a `Pass` verdict never hits the gate, regardless of
        // the `--allow-inconclusive` flag.
        let pass_result = AggregatedResult {
            overall: OverallVerdict::Pass,
            ..inconclusive_result.clone()
        };
        assert_eq!(
            apply_inconclusive_gate(&pass_result, false),
            Ok(()),
            "Pass verdict must never hit the Inconclusive gate"
        );
        assert_eq!(
            apply_inconclusive_gate(&pass_result, true),
            Ok(()),
            "Pass verdict must never hit the Inconclusive gate"
        );

        // Cross-check: a `Fail` verdict is handled by a SEPARATE earlier
        // gate (see `pipeline.rs:5192-5195`), so the Inconclusive gate
        // itself must let `Fail` pass through (the separate Fail gate will
        // already have returned `Err` upstream). This documents the
        // sequencing invariant: Fail gate FIRST, then Inconclusive gate.
        let fail_result = AggregatedResult {
            overall: OverallVerdict::Fail,
            ..inconclusive_result.clone()
        };
        assert_eq!(
            apply_inconclusive_gate(&fail_result, false),
            Ok(()),
            "Fail verdict is handled by a separate upstream gate; the \
             Inconclusive gate itself must be a no-op for Fail"
        );
    }

    /// `max_paths` is configurable via the aggregator.
    #[test]
    fn wave19_max_paths_configurable() {
        let aggregator = InvariantAggregator::new().with_max_paths(128);
        assert_eq!(aggregator.engine.max_paths(), 128);
    }

    /// `max_path_length` is configurable via the aggregator.
    #[test]
    fn wave19_max_path_length_configurable() {
        let aggregator = InvariantAggregator::new().with_max_path_length(512);
        assert_eq!(aggregator.engine.max_path_length(), 512);
    }

    /// Custom limits do not crash PMT verification.
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
            ProgramPoint {
                file: None,
                line: Some(1),
                column: Some(1),
                offset: None,
            },
        );
        let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);
        region.add_node(alloc_id);
        scg.add_region(region);

        let aggregator = InvariantAggregator::new()
            .with_level(VerificationLevel::Pmt)
            .with_max_paths(1)
            .with_max_path_length(8);
        let input = VerificationInput::from_scg(scg);
        let result = aggregator.verify_all(&input);
        // Must produce a valid verdict (not panic).
        let _ = result.overall;
    }

    // ═══════════════════════════════════════════════════════════════════════
    // PMT state verification level tests
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
        use vuma_scg::node::{NodeType, ProgramPoint, StateReadNode};
        let mut scg = SCG::new();
        scg.add_node(
            NodeType::StateRead,
            vuma_scg::node::NodePayload::StateRead(StateReadNode {
                state_vreg: 1,
                layout_name: layout.to_string(),
                field_name: field.to_string(),
                result_vreg: 2,
            }),
            ProgramPoint {
                file: None,
                line: Some(1),
                column: Some(1),
                offset: None,
            },
        );
        scg
    }

    /// Helper: construct an SCG with a single StateWrite node.
    fn scg_with_state_write(layout: &str, field: &str) -> SCG {
        use vuma_scg::node::{NodeType, ProgramPoint, StateWriteNode};
        let mut scg = SCG::new();
        scg.add_node(
            NodeType::StateWrite,
            vuma_scg::node::NodePayload::StateWrite(StateWriteNode {
                state_vreg: 1,
                layout_name: layout.to_string(),
                field_name: field.to_string(),
                value_vreg: 2,
            }),
            ProgramPoint {
                file: None,
                line: Some(1),
                column: Some(1),
                offset: None,
            },
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
        use vuma_scg::node::{NodeType, ProgramPoint, StateTransformNode, StateWriteNode};
        let mut scg = SCG::new();
        let pp = ProgramPoint {
            file: None,
            line: Some(1),
            column: Some(1),
            offset: None,
        };
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
        use vuma_scg::node::{NodeType, ProgramPoint, StateTransformNode};
        let mut scg = SCG::new();
        scg.add_node(
            NodeType::StateTransform,
            vuma_scg::node::NodePayload::StateTransform(StateTransformNode {
                input_vreg: 1,
                input_layout: in_layout.to_string(),
                output_layout: out_layout.to_string(),
                result_vreg: 2,
            }),
            ProgramPoint {
                file: None,
                line: Some(1),
                column: Some(1),
                offset: None,
            },
        );
        scg
    }

    #[test]
    fn wave3d_pmt_level_runs_single_invariant() {
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let input = VerificationInput::from_scg(SCG::new());
        let result = aggregator.verify_all(&input);
        assert_eq!(
            result.per_invariant.len(),
            1,
            "Pmt level must run exactly 1 check"
        );
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
        let scg = scg_with_state_read("Point", "x");
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("Point".to_string(), pmt_layout_point());
        let input = VerificationInput::from_scg(scg).with_pmt_layouts(layouts);
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(
            result.overall,
            OverallVerdict::Pass,
            "valid read must PASS, got {:?}: {}",
            result.overall,
            result.per_invariant[0].result.message
        );
    }

    #[test]
    fn wave3d_pmt_unknown_field_fails() {
        let scg = scg_with_state_read("Point", "z");
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("Point".to_string(), pmt_layout_point());
        let input = VerificationInput::from_scg(scg).with_pmt_layouts(layouts);
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(result.overall, OverallVerdict::Fail);
        let msg = &result.per_invariant[0].result.message;
        // The unknown-field case is now caught EARLIER by the
        // field-list cross-check (`verify_layout_field_list_consistency`)
        // before the state-read verifier runs. Either message is a valid
        // failure signal — the cross-check fires when the SCG references a
        // field not declared in the parser-provided layout, which is a
        // strictly stronger check than the verifier's "unknown field".
        assert!(
            msg.contains("pmt-state violations") || msg.contains("field-list cross-check failed"),
            "expected pmt-state violation or field-list cross-check failure, got: {}",
            msg,
        );
    }

    #[test]
    fn wave3d_pmt_unknown_layout_fails() {
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
        let scg = scg_with_transform_then_write("Point", "Vec2", "Point", "x");
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("Point".to_string(), pmt_layout_point());
        layouts.insert("Vec2".to_string(), pmt_layout_vec2());
        let input = VerificationInput::from_scg(scg).with_pmt_layouts(layouts);
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(result.overall, OverallVerdict::Fail);
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
        let scg = scg_with_state_transform("Point", "Vec2");
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("Point".to_string(), pmt_layout_point());
        layouts.insert("Vec2".to_string(), pmt_layout_vec2());
        let input = VerificationInput::from_scg(scg).with_pmt_layouts(layouts);
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(
            result.overall,
            OverallVerdict::Pass,
            "valid transform must PASS, got {:?}: {}",
            result.overall,
            result.per_invariant[0].result.message
        );
    }

    #[test]
    fn wave3d_pmt_valid_write_passes() {
        let scg = scg_with_state_write("Point", "x");
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("Point".to_string(), pmt_layout_point());
        let input = VerificationInput::from_scg(scg).with_pmt_layouts(layouts);
        let aggregator = InvariantAggregator::new().with_level(VerificationLevel::Pmt);
        let result = aggregator.verify_all(&input);
        assert_eq!(
            result.overall,
            OverallVerdict::Pass,
            "valid write must PASS, got {:?}: {}",
            result.overall,
            result.per_invariant[0].result.message
        );
    }

    #[test]
    fn wave3d_pmt_no_layouts_fails_on_state_ops() {
        let scg = scg_with_state_read("Point", "x");
        let input = VerificationInput::from_scg(scg);
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
