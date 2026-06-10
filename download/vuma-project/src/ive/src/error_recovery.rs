//! Error recovery module for the IVE.
//!
//! When verification fails, this module provides structured error recovery
//! suggestions and partial verification support. It enables the system to:
//!
//! 1. **Collect and categorise** verification errors by severity and invariant.
//! 2. **Suggest fixes** with code hints and confidence scores.
//! 3. **Summarise** the error landscape with estimated fix times and priorities.
//! 4. **Extract partial results** — identify which invariants and regions
//!    *are* verified even when full verification fails.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

// ---------------------------------------------------------------------------
// ErrorSeverity
// ---------------------------------------------------------------------------

/// The severity of a verification error.
///
/// Ordered from `Critical` (program is definitely unsafe) to `Info`
/// (informational, no safety impact). The derived `Ord` implementation
/// orders from most to least severe so that sorting produces the most
/// urgent errors first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorSeverity {
    /// Program is definitely unsafe.
    Critical,
    /// Likely unsafe without additional proof.
    High,
    /// Possibly unsafe, needs investigation.
    Medium,
    /// Minor issue, unlikely to cause problems.
    Low,
    /// Informational, no safety impact.
    Info,
}

impl ErrorSeverity {
    /// Return a numeric weight for sorting (higher = more severe).
    pub fn weight(self) -> u8 {
        match self {
            Self::Critical => 5,
            Self::High => 4,
            Self::Medium => 3,
            Self::Low => 2,
            Self::Info => 1,
        }
    }

    /// Return the estimated fix time for a single error of this severity.
    pub fn estimated_fix_time(self) -> Duration {
        match self {
            Self::Critical => Duration::from_secs(3600), // ~1 hour
            Self::High => Duration::from_secs(1800),     // ~30 minutes
            Self::Medium => Duration::from_secs(600),    // ~10 minutes
            Self::Low => Duration::from_secs(120),       // ~2 minutes
            Self::Info => Duration::from_secs(30),       // ~30 seconds
        }
    }
}

impl Ord for ErrorSeverity {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher weight = more severe = greater in ordering
        self.weight().cmp(&other.weight())
    }
}

impl PartialOrd for ErrorSeverity {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for ErrorSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Critical => write!(f, "CRITICAL"),
            Self::High => write!(f, "HIGH"),
            Self::Medium => write!(f, "MEDIUM"),
            Self::Low => write!(f, "LOW"),
            Self::Info => write!(f, "INFO"),
        }
    }
}

// ---------------------------------------------------------------------------
// SuggestedFix
// ---------------------------------------------------------------------------

/// A suggested fix for a verification error.
///
/// Each fix carries a human-readable description, an optional code hint
/// showing how the fix could be applied, and a confidence score indicating
/// how likely the fix is to resolve the underlying error.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SuggestedFix {
    /// Human-readable description of the fix.
    pub description: String,
    /// An optional code snippet hint showing how to apply the fix.
    pub code_hint: Option<String>,
    /// Confidence (0.0 – 1.0) that this fix resolves the error.
    pub confidence: f64,
    /// Whether the fix can be applied automatically (e.g. by a linter).
    pub auto_applicable: bool,
}

impl SuggestedFix {
    /// Construct a new suggested fix.
    pub fn new(description: impl Into<String>, confidence: f64) -> Self {
        Self {
            description: description.into(),
            code_hint: None,
            confidence: confidence.clamp(0.0, 1.0),
            auto_applicable: false,
        }
    }

    /// Attach a code hint to this fix.
    pub fn with_code_hint(mut self, hint: impl Into<String>) -> Self {
        self.code_hint = Some(hint.into());
        self
    }

    /// Mark this fix as auto-applicable.
    pub fn with_auto_applicable(mut self, val: bool) -> Self {
        self.auto_applicable = val;
        self
    }
}

impl fmt::Display for SuggestedFix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Fix (confidence={:.0}%): {}",
            self.confidence * 100.0,
            self.description
        )?;
        if let Some(hint) = &self.code_hint {
            write!(f, "\n  Hint: {hint}")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// VerificationError
// ---------------------------------------------------------------------------

/// A structured verification error produced when an invariant check fails.
///
/// Each error identifies the invariant that was violated, the severity of
/// the violation, a human-readable description, an optional source location,
/// a list of suggested fixes, and indices of related errors in the same
/// collection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerificationError {
    /// The invariant that was violated.
    pub invariant: String,
    /// How severe this violation is.
    pub severity: ErrorSeverity,
    /// Human-readable description of the violation.
    pub violation: String,
    /// Optional source location (e.g. "src/main.rs:42:10").
    pub location: Option<String>,
    /// Suggested fixes for this error.
    pub suggested_fixes: Vec<SuggestedFix>,
    /// Indices of related errors within the same `ErrorCollector`.
    pub related_errors: Vec<usize>,
}

impl VerificationError {
    /// Construct a new verification error with no suggested fixes or
    /// related errors.
    pub fn new(
        invariant: impl Into<String>,
        severity: ErrorSeverity,
        violation: impl Into<String>,
    ) -> Self {
        Self {
            invariant: invariant.into(),
            severity,
            violation: violation.into(),
            location: None,
            suggested_fixes: Vec::new(),
            related_errors: Vec::new(),
        }
    }

    /// Attach a source location to this error.
    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    /// Add a suggested fix.
    pub fn with_suggested_fix(mut self, fix: SuggestedFix) -> Self {
        self.suggested_fixes.push(fix);
        self
    }

    /// Add a related error index.
    pub fn with_related_error(mut self, index: usize) -> Self {
        self.related_errors.push(index);
        self
    }
}

impl fmt::Display for VerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} — {}",
            self.severity, self.invariant, self.violation
        )?;
        if let Some(loc) = &self.location {
            write!(f, " (at {loc})")?;
        }
        if !self.suggested_fixes.is_empty() {
            write!(f, "\n  Suggested fixes:")?;
            for fix in &self.suggested_fixes {
                write!(f, "\n    - {fix}")?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ErrorSummary
// ---------------------------------------------------------------------------

/// A summary of all verification errors collected during a verification run.
///
/// Aggregates counts by severity and invariant, estimates total fix time,
/// and produces a priority-ordered list of error indices.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorSummary {
    /// Total number of errors.
    pub total_errors: usize,
    /// Number of errors at each severity level.
    pub by_severity: HashMap<ErrorSeverity, usize>,
    /// Number of errors for each invariant.
    pub by_invariant: HashMap<String, usize>,
    /// Estimated total time to fix all errors.
    pub estimated_fix_time: Duration,
    /// Error indices ordered by fix priority (most urgent first).
    pub fix_priority_order: Vec<usize>,
}

impl fmt::Display for ErrorSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== Error Summary ===")?;
        writeln!(f, "Total errors: {}", self.total_errors)?;
        writeln!(f, "Estimated fix time: {:.1}s", self.estimated_fix_time.as_secs_f64())?;
        writeln!(f, "By severity:")?;
        for sev in &[
            ErrorSeverity::Critical,
            ErrorSeverity::High,
            ErrorSeverity::Medium,
            ErrorSeverity::Low,
            ErrorSeverity::Info,
        ] {
            let count = self.by_severity.get(sev).copied().unwrap_or(0);
            writeln!(f, "  {}: {}", sev, count)?;
        }
        if !self.by_invariant.is_empty() {
            writeln!(f, "By invariant:")?;
            let mut invs: Vec<_> = self.by_invariant.iter().collect();
            invs.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
            for (inv, count) in invs {
                writeln!(f, "  {}: {}", inv, count)?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// ErrorCollector
// ---------------------------------------------------------------------------

/// Collects verification errors from multiple invariant checks.
///
/// Provides methods to add errors, query by severity or invariant,
/// check for critical errors, and generate a summary with prioritised
/// fix order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ErrorCollector {
    /// The collected errors.
    errors: Vec<VerificationError>,
}

impl ErrorCollector {
    /// Construct a new, empty error collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a verification error to the collector.
    pub fn add_error(&mut self, error: VerificationError) {
        self.errors.push(error);
    }

    /// Return all errors sorted by severity (most severe first).
    pub fn errors_by_severity(&self) -> Vec<&VerificationError> {
        let mut refs: Vec<&VerificationError> = self.errors.iter().collect();
        refs.sort_by(|a, b| b.severity.cmp(&a.severity));
        refs
    }

    /// Return all errors for a specific invariant.
    pub fn errors_by_invariant(&self, invariant: &str) -> Vec<&VerificationError> {
        self.errors
            .iter()
            .filter(|e| e.invariant == invariant)
            .collect()
    }

    /// Return the number of critical errors.
    pub fn critical_count(&self) -> usize {
        self.errors
            .iter()
            .filter(|e| e.severity == ErrorSeverity::Critical)
            .count()
    }

    /// Return `true` if there is at least one critical error.
    pub fn has_critical(&self) -> bool {
        self.errors
            .iter()
            .any(|e| e.severity == ErrorSeverity::Critical)
    }

    /// Return the total number of errors.
    pub fn len(&self) -> usize {
        self.errors.len()
    }

    /// Return `true` if there are no errors.
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    /// Return an iterator over all errors.
    pub fn iter(&self) -> impl Iterator<Item = &VerificationError> {
        self.errors.iter()
    }

    /// Generate a summary of the collected errors.
    ///
    /// The summary includes counts by severity and invariant, an estimated
    /// total fix time (sum of per-severity estimates), and a prioritised
    /// fix order (indices sorted by severity, then by number of suggested
    /// fixes — fewer suggestions means harder to fix, so higher priority).
    pub fn summary(&self) -> ErrorSummary {
        let mut by_severity: HashMap<ErrorSeverity, usize> = HashMap::new();
        let mut by_invariant: HashMap<String, usize> = HashMap::new();
        let mut estimated_fix_time = Duration::ZERO;

        for error in &self.errors {
            *by_severity.entry(error.severity).or_insert(0) += 1;
            *by_invariant.entry(error.invariant.clone()).or_insert(0) += 1;
            estimated_fix_time += error.severity.estimated_fix_time();
        }

        // Build priority order: sort error indices by severity (most severe
        // first), breaking ties by fewer suggested fixes (harder to fix =
        // higher priority).
        let mut indexed: Vec<(usize, &VerificationError)> =
            self.errors.iter().enumerate().collect();
        indexed.sort_by(|(i_a, a), (i_b, b)| {
            // Most severe first, then fewer suggested fixes = higher priority,
            // then stable by original index.
            b.severity
                .cmp(&a.severity)
                .then_with(|| {
                    // Fewer suggested fixes → higher priority.
                    a.suggested_fixes
                        .len()
                        .cmp(&b.suggested_fixes.len())
                })
                .then_with(|| i_a.cmp(i_b))
        });

        let fix_priority_order: Vec<usize> = indexed.into_iter().map(|(i, _)| i).collect();

        ErrorSummary {
            total_errors: self.errors.len(),
            by_severity,
            by_invariant,
            estimated_fix_time,
            fix_priority_order,
        }
    }
}

impl fmt::Display for ErrorCollector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "ErrorCollector ({} errors):", self.errors.len())?;
        for error in &self.errors {
            writeln!(f, "  {error}")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SafeRegion
// ---------------------------------------------------------------------------

/// A region of the program that has been verified safe for at least one
/// invariant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SafeRegion {
    /// Unique identifier for this region.
    pub region_id: u64,
    /// The invariants that have been verified within this region.
    pub verified_invariants: Vec<String>,
    /// Confidence that this region is truly safe (0.0 – 1.0).
    pub confidence: f64,
}

impl SafeRegion {
    /// Construct a new safe region.
    pub fn new(region_id: u64, verified_invariants: Vec<String>, confidence: f64) -> Self {
        Self {
            region_id,
            verified_invariants,
            confidence: confidence.clamp(0.0, 1.0),
        }
    }
}

impl fmt::Display for SafeRegion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SafeRegion#{} (confidence={:.0}%): [{}]",
            self.region_id,
            self.confidence * 100.0,
            self.verified_invariants.join(", ")
        )
    }
}

// ---------------------------------------------------------------------------
// UnsafeRegion
// ---------------------------------------------------------------------------

/// A region of the program that contains one or more violations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnsafeRegion {
    /// Unique identifier for this region.
    pub region_id: u64,
    /// The violations found within this region.
    pub violations: Vec<String>,
}

impl UnsafeRegion {
    /// Construct a new unsafe region.
    pub fn new(region_id: u64, violations: Vec<String>) -> Self {
        Self {
            region_id,
            violations,
        }
    }
}

impl fmt::Display for UnsafeRegion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "UnsafeRegion#{}: [{}]",
            self.region_id,
            self.violations.join(", ")
        )
    }
}

// ---------------------------------------------------------------------------
// PartialVerificationResult
// ---------------------------------------------------------------------------

/// The result of a partial verification — when full verification fails,
/// this captures what *is* verified and what is not.
///
/// This enables incremental verification: developers can focus on fixing
/// the failed invariants and unsafe regions while knowing which parts of
/// the program are already safe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PartialVerificationResult {
    /// Invariants that passed verification.
    pub verified_invariants: Vec<String>,
    /// Invariants that failed verification.
    pub failed_invariants: Vec<String>,
    /// Regions that were verified safe.
    pub safe_regions: Vec<SafeRegion>,
    /// Regions that contain violations.
    pub unsafe_regions: Vec<UnsafeRegion>,
}

impl PartialVerificationResult {
    /// Construct a new, empty partial verification result.
    pub fn new() -> Self {
        Self {
            verified_invariants: Vec::new(),
            failed_invariants: Vec::new(),
            safe_regions: Vec::new(),
            unsafe_regions: Vec::new(),
        }
    }

    /// Add a verified invariant.
    pub fn with_verified_invariant(mut self, invariant: impl Into<String>) -> Self {
        self.verified_invariants.push(invariant.into());
        self
    }

    /// Add a failed invariant.
    pub fn with_failed_invariant(mut self, invariant: impl Into<String>) -> Self {
        self.failed_invariants.push(invariant.into());
        self
    }

    /// Add a safe region.
    pub fn with_safe_region(mut self, region: SafeRegion) -> Self {
        self.safe_regions.push(region);
        self
    }

    /// Add an unsafe region.
    pub fn with_unsafe_region(mut self, region: UnsafeRegion) -> Self {
        self.unsafe_regions.push(region);
        self
    }

    /// Build a partial verification result from an error collector.
    ///
    /// The verified invariants are the complement of the invariants that
    /// appear in the error collector. The safe and unsafe regions must be
    /// supplied by the caller (they depend on program structure).
    pub fn from_collector(
        collector: &ErrorCollector,
        all_invariants: &[String],
        safe_regions: Vec<SafeRegion>,
        unsafe_regions: Vec<UnsafeRegion>,
    ) -> Self {
        let failed_set: std::collections::HashSet<&str> = collector
            .iter()
            .map(|e| e.invariant.as_str())
            .collect();

        let verified_invariants: Vec<String> = all_invariants
            .iter()
            .filter(|inv| !failed_set.contains(inv.as_str()))
            .cloned()
            .collect();

        let failed_invariants: Vec<String> = all_invariants
            .iter()
            .filter(|inv| failed_set.contains(inv.as_str()))
            .cloned()
            .collect();

        Self {
            verified_invariants,
            failed_invariants,
            safe_regions,
            unsafe_regions,
        }
    }

    /// Returns `true` if all invariants passed (no failures).
    pub fn is_fully_verified(&self) -> bool {
        self.failed_invariants.is_empty() && self.unsafe_regions.is_empty()
    }

    /// Returns the ratio of verified invariants to total invariants.
    pub fn verification_ratio(&self) -> f64 {
        let total = self.verified_invariants.len() + self.failed_invariants.len();
        if total == 0 {
            1.0
        } else {
            self.verified_invariants.len() as f64 / total as f64
        }
    }
}

impl Default for PartialVerificationResult {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PartialVerificationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== Partial Verification Result ===")?;
        writeln!(
            f,
            "Verified invariants ({}): [{}]",
            self.verified_invariants.len(),
            self.verified_invariants.join(", ")
        )?;
        writeln!(
            f,
            "Failed invariants ({}): [{}]",
            self.failed_invariants.len(),
            self.failed_invariants.join(", ")
        )?;
        writeln!(f, "Safe regions ({}):", self.safe_regions.len())?;
        for region in &self.safe_regions {
            writeln!(f, "  {region}")?;
        }
        writeln!(f, "Unsafe regions ({}):", self.unsafe_regions.len())?;
        for region in &self.unsafe_regions {
            writeln!(f, "  {region}")?;
        }
        writeln!(
            f,
            "Verification ratio: {:.0}%",
            self.verification_ratio() * 100.0
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Test 1: ErrorSeverity ordering and display ----

    #[test]
    fn error_severity_ordering_and_display() {
        assert!(ErrorSeverity::Critical > ErrorSeverity::High);
        assert!(ErrorSeverity::High > ErrorSeverity::Medium);
        assert!(ErrorSeverity::Medium > ErrorSeverity::Low);
        assert!(ErrorSeverity::Low > ErrorSeverity::Info);

        assert_eq!(format!("{}", ErrorSeverity::Critical), "CRITICAL");
        assert_eq!(format!("{}", ErrorSeverity::High), "HIGH");
        assert_eq!(format!("{}", ErrorSeverity::Medium), "MEDIUM");
        assert_eq!(format!("{}", ErrorSeverity::Low), "LOW");
        assert_eq!(format!("{}", ErrorSeverity::Info), "INFO");
    }

    // ---- Test 2: ErrorCollector add and query by severity ----

    #[test]
    fn collector_add_and_query_by_severity() {
        let mut collector = ErrorCollector::new();

        collector.add_error(VerificationError::new(
            "liveness",
            ErrorSeverity::Critical,
            "Resource never freed",
        ));
        collector.add_error(VerificationError::new(
            "exclusivity",
            ErrorSeverity::High,
            "Data race detected",
        ));
        collector.add_error(VerificationError::new(
            "cleanup",
            ErrorSeverity::Low,
            "Redundant drop",
        ));
        collector.add_error(VerificationError::new(
            "liveness",
            ErrorSeverity::Medium,
            "Possible deadlock",
        ));

        assert_eq!(collector.len(), 4);
        assert!(collector.has_critical());
        assert_eq!(collector.critical_count(), 1);

        let sorted = collector.errors_by_severity();
        assert_eq!(sorted[0].severity, ErrorSeverity::Critical);
        assert_eq!(sorted[1].severity, ErrorSeverity::High);
        assert_eq!(sorted[2].severity, ErrorSeverity::Medium);
        assert_eq!(sorted[3].severity, ErrorSeverity::Low);
    }

    // ---- Test 3: ErrorCollector query by invariant ----

    #[test]
    fn collector_query_by_invariant() {
        let mut collector = ErrorCollector::new();

        collector.add_error(VerificationError::new(
            "liveness",
            ErrorSeverity::Critical,
            "Resource never freed",
        ));
        collector.add_error(VerificationError::new(
            "exclusivity",
            ErrorSeverity::High,
            "Data race",
        ));
        collector.add_error(VerificationError::new(
            "liveness",
            ErrorSeverity::Low,
            "Minor delay",
        ));

        let liveness_errors = collector.errors_by_invariant("liveness");
        assert_eq!(liveness_errors.len(), 2);

        let exclusivity_errors = collector.errors_by_invariant("exclusivity");
        assert_eq!(exclusivity_errors.len(), 1);

        let origin_errors = collector.errors_by_invariant("origin");
        assert!(origin_errors.is_empty());
    }

    // ---- Test 4: ErrorSummary with prioritised fix order ----

    #[test]
    fn summary_with_prioritised_fix_order() {
        let mut collector = ErrorCollector::new();

        // Add errors in mixed order.
        collector.add_error(
            VerificationError::new("cleanup", ErrorSeverity::Low, "Redundant drop")
                .with_suggested_fix(SuggestedFix::new("Remove redundant drop", 0.9)),
        );
        collector.add_error(VerificationError::new(
            "liveness",
            ErrorSeverity::Critical,
            "Resource never freed",
        ));
        collector.add_error(
            VerificationError::new("exclusivity", ErrorSeverity::High, "Data race")
                .with_suggested_fix(SuggestedFix::new("Add mutex", 0.7)),
        );

        let summary = collector.summary();

        assert_eq!(summary.total_errors, 3);
        assert_eq!(summary.by_severity.get(&ErrorSeverity::Critical), Some(&1));
        assert_eq!(summary.by_severity.get(&ErrorSeverity::High), Some(&1));
        assert_eq!(summary.by_severity.get(&ErrorSeverity::Low), Some(&1));
        assert!(summary.estimated_fix_time > Duration::ZERO);

        // Fix priority: Critical (index 1) first, then High (index 2), then
        // Low (index 0). Among same severity, fewer suggestions = higher
        // priority (but here severities differ).
        assert_eq!(summary.fix_priority_order[0], 1); // Critical
        assert_eq!(summary.fix_priority_order[1], 2); // High
        assert_eq!(summary.fix_priority_order[2], 0); // Low
    }

    // ---- Test 5: VerificationError builder pattern ----

    #[test]
    fn verification_error_builder() {
        let error = VerificationError::new(
            "interpretation",
            ErrorSeverity::High,
            "Invalid cast from *u8 to *u64",
        )
        .with_location("src/main.rs:42:10")
        .with_suggested_fix(
            SuggestedFix::new("Use checked cast", 0.85)
                .with_code_hint("let ptr = (addr as usize).checked_add(8)? as *const u64;")
                .with_auto_applicable(true),
        )
        .with_related_error(2)
        .with_related_error(5);

        assert_eq!(error.invariant, "interpretation");
        assert_eq!(error.severity, ErrorSeverity::High);
        assert_eq!(error.location, Some("src/main.rs:42:10".to_string()));
        assert_eq!(error.suggested_fixes.len(), 1);
        assert_eq!(error.suggested_fixes[0].confidence, 0.85);
        assert!(error.suggested_fixes[0].auto_applicable);
        assert_eq!(error.related_errors, vec![2, 5]);
    }

    // ---- Test 6: SuggestedFix confidence clamping and display ----

    #[test]
    fn suggested_fix_confidence_clamping_and_display() {
        let fix = SuggestedFix::new("Apply mutex", 1.5); // Over 1.0, should be clamped
        assert!((fix.confidence - 1.0).abs() < 1e-9);

        let fix = SuggestedFix::new("Refactor", -0.3); // Below 0.0, should be clamped
        assert!((fix.confidence - 0.0).abs() < 1e-9);

        let fix = SuggestedFix::new("Add bounds check", 0.7).with_code_hint("if i < len { ... }");
        let displayed = format!("{fix}");
        assert!(displayed.contains("70%"));
        assert!(displayed.contains("Add bounds check"));
        assert!(displayed.contains("if i < len { ... }"));
    }

    // ---- Test 7: PartialVerificationResult from collector ----

    #[test]
    fn partial_verification_from_collector() {
        let mut collector = ErrorCollector::new();
        collector.add_error(VerificationError::new(
            "liveness",
            ErrorSeverity::Critical,
            "Resource leak",
        ));
        collector.add_error(VerificationError::new(
            "exclusivity",
            ErrorSeverity::High,
            "Data race",
        ));

        let all_invariants = vec![
            "liveness".to_string(),
            "exclusivity".to_string(),
            "origin".to_string(),
            "cleanup".to_string(),
            "interpretation".to_string(),
        ];

        let safe_regions = vec![SafeRegion::new(1, vec!["origin".to_string()], 0.95)];
        let unsafe_regions = vec![UnsafeRegion::new(2, vec!["Resource leak".to_string()])];

        let result = PartialVerificationResult::from_collector(
            &collector,
            &all_invariants,
            safe_regions,
            unsafe_regions,
        );

        // Failed: liveness, exclusivity
        assert_eq!(result.failed_invariants.len(), 2);
        assert!(result.failed_invariants.contains(&"liveness".to_string()));
        assert!(result.failed_invariants.contains(&"exclusivity".to_string()));

        // Verified: origin, cleanup, interpretation
        assert_eq!(result.verified_invariants.len(), 3);
        assert!(result.verified_invariants.contains(&"origin".to_string()));
        assert!(result.verified_invariants.contains(&"cleanup".to_string()));
        assert!(result
            .verified_invariants
            .contains(&"interpretation".to_string()));

        assert!(!result.is_fully_verified());
        assert!((result.verification_ratio() - 0.6).abs() < 1e-9); // 3/5 = 0.6
    }

    // ---- Test 8: PartialVerificationResult fully verified ----

    #[test]
    fn partial_verification_fully_verified() {
        let result = PartialVerificationResult::new()
            .with_verified_invariant("liveness")
            .with_verified_invariant("exclusivity")
            .with_verified_invariant("origin")
            .with_verified_invariant("cleanup")
            .with_verified_invariant("interpretation")
            .with_safe_region(SafeRegion::new(1, vec!["liveness".to_string()], 0.99));

        assert!(result.is_fully_verified());
        assert!((result.verification_ratio() - 1.0).abs() < 1e-9);
    }

    // ---- Test 9: Empty ErrorCollector ----

    #[test]
    fn empty_collector() {
        let collector = ErrorCollector::new();
        assert!(collector.is_empty());
        assert_eq!(collector.len(), 0);
        assert!(!collector.has_critical());
        assert_eq!(collector.critical_count(), 0);

        let summary = collector.summary();
        assert_eq!(summary.total_errors, 0);
        assert!(summary.by_severity.is_empty());
        assert!(summary.by_invariant.is_empty());
        assert_eq!(summary.estimated_fix_time, Duration::ZERO);
        assert!(summary.fix_priority_order.is_empty());
    }

    // ---- Test 10: ErrorSeverity estimated fix time ----

    #[test]
    fn severity_estimated_fix_time() {
        assert!(ErrorSeverity::Critical.estimated_fix_time() > ErrorSeverity::High.estimated_fix_time());
        assert!(ErrorSeverity::High.estimated_fix_time() > ErrorSeverity::Medium.estimated_fix_time());
        assert!(ErrorSeverity::Medium.estimated_fix_time() > ErrorSeverity::Low.estimated_fix_time());
        assert!(ErrorSeverity::Low.estimated_fix_time() > ErrorSeverity::Info.estimated_fix_time());
    }

    // ---- Test 11: VerificationError display with location and fixes ----

    #[test]
    fn verification_error_display() {
        let error = VerificationError::new(
            "origin",
            ErrorSeverity::Medium,
            "Pointer may be forged",
        )
        .with_location("src/ptr.rs:10:5")
        .with_suggested_fix(SuggestedFix::new("Validate provenance", 0.8));

        let displayed = format!("{error}");
        assert!(displayed.contains("[MEDIUM]"));
        assert!(displayed.contains("origin"));
        assert!(displayed.contains("Pointer may be forged"));
        assert!(displayed.contains("src/ptr.rs:10:5"));
        assert!(displayed.contains("Suggested fixes"));
        assert!(displayed.contains("Validate provenance"));
    }

    // ---- Test 12: PartialVerificationResult builder and ratio ----

    #[test]
    fn partial_result_builder_and_ratio() {
        let result = PartialVerificationResult::new()
            .with_verified_invariant("liveness")
            .with_verified_invariant("exclusivity")
            .with_failed_invariant("origin")
            .with_safe_region(SafeRegion::new(10, vec!["liveness".to_string()], 0.9))
            .with_unsafe_region(UnsafeRegion::new(20, vec!["Forged pointer".to_string()]));

        assert!(!result.is_fully_verified());
        assert!((result.verification_ratio() - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(result.safe_regions.len(), 1);
        assert_eq!(result.unsafe_regions.len(), 1);
        assert_eq!(result.safe_regions[0].region_id, 10);
        assert_eq!(result.unsafe_regions[0].region_id, 20);
    }

    // ---- Test 13: Summary estimated fix time accumulates ----

    #[test]
    fn summary_estimated_fix_time_accumulates() {
        let mut collector = ErrorCollector::new();
        collector.add_error(VerificationError::new("a", ErrorSeverity::Critical, "err1"));
        collector.add_error(VerificationError::new("b", ErrorSeverity::Low, "err2"));

        let summary = collector.summary();
        let expected = ErrorSeverity::Critical.estimated_fix_time()
            + ErrorSeverity::Low.estimated_fix_time();
        assert_eq!(summary.estimated_fix_time, expected);
    }

    // ---- Test 14: SafeRegion and UnsafeRegion display ----

    #[test]
    fn region_display() {
        let safe = SafeRegion::new(42, vec!["liveness".to_string(), "origin".to_string()], 0.87);
        let displayed = format!("{safe}");
        assert!(displayed.contains("SafeRegion#42"));
        assert!(displayed.contains("87%"));
        assert!(displayed.contains("liveness"));
        assert!(displayed.contains("origin"));

        let unsafe_r = UnsafeRegion::new(99, vec!["Data race".to_string()]);
        let displayed = format!("{unsafe_r}");
        assert!(displayed.contains("UnsafeRegion#99"));
        assert!(displayed.contains("Data race"));
    }

    // ---- Test 15: Fix priority order with same severity ----

    #[test]
    fn fix_priority_order_same_severity() {
        let mut collector = ErrorCollector::new();

        // Two High-severity errors: the one with fewer fixes should come first.
        collector.add_error(
            VerificationError::new("a", ErrorSeverity::High, "err_a")
                .with_suggested_fix(SuggestedFix::new("fix1", 0.9))
                .with_suggested_fix(SuggestedFix::new("fix2", 0.8)),
        );
        collector.add_error(
            VerificationError::new("b", ErrorSeverity::High, "err_b")
                // No suggested fixes — harder to fix, higher priority
        );

        let summary = collector.summary();
        // Index 1 (no fixes) should come before index 0 (2 fixes)
        assert_eq!(summary.fix_priority_order[0], 1);
        assert_eq!(summary.fix_priority_order[1], 0);
    }
}
