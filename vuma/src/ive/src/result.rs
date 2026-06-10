//! Verification result types for the IVE module.
//!
//! This module defines the core result and status types used by the
//! verification engine to report outcomes of invariant checks.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// The name of an invariant being verified.
pub type InvariantName = String;

/// A point in the program, used for counterexample traces.
pub type ProgramPoint = String;

/// An assumption made during verification that has not been formally proven.
pub type Assumption = String;

/// A single step in a formal proof.
pub type ProofStep = String;

// ---------------------------------------------------------------------------
// VerificationStatus
// ---------------------------------------------------------------------------

/// The status of a verification attempt.
///
/// Encodes the spectrum from full formal proof to detected violations,
/// following VUMA's graduated assurance model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VerificationStatus {
    /// The invariant has been formally proven to hold.
    Proven,

    /// The invariant is probably safe, but relies on unproven assumptions.
    ProbablySafe {
        /// List of assumptions under which the property holds.
        assumptions: Vec<Assumption>,
    },

    /// The invariant could not be verified (insufficient information).
    Unverified {
        /// Human-readable reason why verification was inconclusive.
        reason: String,
    },

    /// The invariant was violated; a counterexample exists.
    Violated {
        /// A concrete counterexample demonstrating the violation.
        counterexample: CounterExample,
    },
}

impl fmt::Display for VerificationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Proven => write!(f, "PROVEN"),
            Self::ProbablySafe { assumptions } => {
                write!(f, "PROBABLY_SAFE ({} assumption(s))", assumptions.len())
            }
            Self::Unverified { reason } => write!(f, "UNVERIFIED: {reason}"),
            Self::Violated { counterexample } => {
                write!(f, "VIOLATED at {}", counterexample.violation_point)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CounterExample
// ---------------------------------------------------------------------------

/// A concrete counterexample demonstrating a violation of an invariant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CounterExample {
    /// The execution path leading to the violation.
    pub execution_path: Vec<ProgramPoint>,
    /// The specific program point where the violation occurs.
    pub violation_point: ProgramPoint,
    /// Human-readable description of the violation.
    pub description: String,
}

impl CounterExample {
    /// Construct a new counterexample.
    pub fn new(
        execution_path: Vec<ProgramPoint>,
        violation_point: ProgramPoint,
        description: String,
    ) -> Self {
        Self {
            execution_path,
            violation_point,
            description,
        }
    }
}

// ---------------------------------------------------------------------------
// Evidence
// ---------------------------------------------------------------------------

/// Evidence supporting a verification result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Evidence {
    /// A formal proof consisting of discrete steps.
    FormalProof {
        /// The steps that constitute the proof.
        steps: Vec<ProofStep>,
    },
    /// Evidence from exhaustive (e.g., model-checking) analysis.
    ExhaustiveAnalysis,
    /// Evidence from statistical or sampling-based analysis.
    StatisticalAnalysis,
}

impl fmt::Display for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FormalProof { steps } => {
                write!(f, "FormalProof ({} step(s))", steps.len())
            }
            Self::ExhaustiveAnalysis => write!(f, "ExhaustiveAnalysis"),
            Self::StatisticalAnalysis => write!(f, "StatisticalAnalysis"),
        }
    }
}

/// The result of verifying a single invariant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerificationResult {
    /// The name of the invariant that was checked.
    pub invariant: InvariantName,
    /// The verification status.
    pub status: VerificationStatus,
    /// Human-readable message describing the outcome.
    pub message: String,
    /// Optional evidence supporting the result.
    pub evidence: Option<Evidence>,
}

impl VerificationResult {
    /// Construct a new verification result.
    pub fn new(
        invariant: impl Into<InvariantName>,
        status: VerificationStatus,
        message: impl Into<String>,
    ) -> Self {
        Self {
            invariant: invariant.into(),
            status,
            message: message.into(),
            evidence: None,
        }
    }

    /// Attach evidence to this result.
    pub fn with_evidence(mut self, evidence: Evidence) -> Self {
        self.evidence = Some(evidence);
        self
    }

    /// Returns `true` if the invariant was proven.
    pub fn is_proven(&self) -> bool {
        matches!(self.status, VerificationStatus::Proven)
    }

    /// Returns `true` if the invariant was violated.
    pub fn is_violated(&self) -> bool {
        matches!(self.status, VerificationStatus::Violated { .. })
    }

    /// Returns the confidence level for this result.
    pub fn confidence(&self) -> ConfidenceLevel {
        match &self.status {
            VerificationStatus::Proven => ConfidenceLevel::High,
            VerificationStatus::ProbablySafe { .. } => ConfidenceLevel::Medium,
            VerificationStatus::Unverified { .. } => ConfidenceLevel::Low,
            VerificationStatus::Violated { .. } => ConfidenceLevel::Low,
        }
    }
}

impl fmt::Display for VerificationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {} — {}", self.status, self.invariant, self.message)
    }
}

// ---------------------------------------------------------------------------
// ConfidenceLevel
// ---------------------------------------------------------------------------

/// Graduated confidence level for verification results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ConfidenceLevel {
    /// Low confidence — unverified or violated.
    Low,
    /// Medium confidence — probably safe under assumptions.
    Medium,
    /// High confidence — formally proven.
    High,
}

impl fmt::Display for ConfidenceLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::High => write!(f, "HIGH"),
            Self::Medium => write!(f, "MEDIUM"),
            Self::Low => write!(f, "LOW"),
        }
    }
}

// ---------------------------------------------------------------------------
// Severity
// ---------------------------------------------------------------------------

/// Severity level for invariant violations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    /// A minor issue or warning.
    Low,
    /// A significant issue that may affect correctness.
    Medium,
    /// A critical safety violation.
    High,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Low => write!(f, "LOW"),
            Severity::Medium => write!(f, "MEDIUM"),
            Severity::High => write!(f, "HIGH"),
        }
    }
}

// ---------------------------------------------------------------------------
// InvariantViolation
// ---------------------------------------------------------------------------

/// A structured invariant violation for use in batched error recovery.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InvariantViolation {
    /// Which invariant was violated.
    pub invariant: InvariantName,
    /// A human-readable description of the violation.
    pub description: String,
    /// The severity of the violation.
    pub severity: Severity,
}

impl InvariantViolation {
    /// Create a new invariant violation.
    pub fn new(
        invariant: impl Into<InvariantName>,
        description: impl Into<String>,
        severity: Severity,
    ) -> Self {
        Self {
            invariant: invariant.into(),
            description: description.into(),
            severity,
        }
    }
}

impl fmt::Display for InvariantViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.severity, self.invariant, self.description)
    }
}

// ---------------------------------------------------------------------------
// BatchedViolations
// ---------------------------------------------------------------------------

/// A collection of all violations found during verification, organized by
/// severity for error recovery. Unlike stopping at the first violation,
/// `BatchedViolations` collects ALL violations so the user can see every
/// issue in a single pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchedViolations {
    /// All violations found, in order of discovery.
    pub violations: Vec<InvariantViolation>,
    /// Violations indexed by severity for efficient lookup.
    severity_index: HashMap<Severity, Vec<InvariantViolation>>,
}

impl BatchedViolations {
    /// Create a new, empty batched violations collector.
    pub fn new() -> Self {
        Self {
            violations: Vec::new(),
            severity_index: HashMap::new(),
        }
    }

    /// Add a violation to the batch.
    pub fn add(&mut self, v: InvariantViolation) {
        let severity = v.severity;
        self.severity_index.entry(severity).or_default().push(v.clone());
        self.violations.push(v);
    }

    /// Group all violations by severity level.
    ///
    /// Returns a HashMap mapping each severity level to the list of
    /// violations at that level.
    pub fn by_severity(&self) -> HashMap<Severity, Vec<&InvariantViolation>> {
        let mut result = HashMap::new();
        for (sev, violations) in &self.severity_index {
            result.insert(*sev, violations.iter().collect());
        }
        result
    }

    /// Generate a human-readable report of all violations.
    pub fn report(&self) -> String {
        if self.violations.is_empty() {
            return "No violations found.".to_string();
        }

        let mut report = format!("Batched Violations Report ({} total):\n", self.total());

        for severity in &[Severity::High, Severity::Medium, Severity::Low] {
            if let Some(violations) = self.severity_index.get(severity) {
                if !violations.is_empty() {
                    report.push_str(&format!("\n{} severity ({}):\n", severity, violations.len()));
                    for v in violations {
                        report.push_str(&format!("  - {}\n", v));
                    }
                }
            }
        }

        report
    }

    /// Get all violations at a specific severity level.
    pub fn by_severity_level(&self, severity: Severity) -> &[InvariantViolation] {
        self.severity_index.get(&severity).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get the total number of violations.
    pub fn total(&self) -> usize {
        self.violations.len()
    }

    /// Returns `true` if there are no violations.
    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }

    /// Returns `true` if there are any high-severity violations.
    pub fn has_critical(&self) -> bool {
        self.severity_index.contains_key(&Severity::High)
            && !self.severity_index[&Severity::High].is_empty()
    }

    /// Get all violations in order of discovery.
    pub fn all(&self) -> &[InvariantViolation] {
        &self.violations
    }
}

impl Default for BatchedViolations {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for BatchedViolations {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.report())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proven_result_is_proven() {
        let r = VerificationResult::new("liveness", VerificationStatus::Proven, "ok");
        assert!(r.is_proven());
        assert!(!r.is_violated());
        assert_eq!(r.confidence(), ConfidenceLevel::High);
    }

    #[test]
    fn violated_result_is_violated() {
        let ce = CounterExample::new(
            vec!["entry".into(), "loop".into()],
            "loop".into(),
            "infinite loop".into(),
        );
        let r = VerificationResult::new(
            "liveness",
            VerificationStatus::Violated {
                counterexample: ce,
            },
            "loop never terminates",
        );
        assert!(r.is_violated());
        assert!(!r.is_proven());
        assert_eq!(r.confidence(), ConfidenceLevel::Low);
    }

    #[test]
    fn display_formats() {
        let r = VerificationResult::new("test", VerificationStatus::Proven, "all good");
        assert_eq!(format!("{r}"), "[PROVEN] test — all good");
    }

    #[test]
    fn evidence_display() {
        let e = Evidence::FormalProof {
            steps: vec!["step1".into(), "step2".into()],
        };
        let s = format!("{}", e);
        assert!(s.contains("2 step(s)"));

        let s2 = format!("{}", Evidence::ExhaustiveAnalysis);
        assert_eq!(s2, "ExhaustiveAnalysis");
    }

    // ----- BatchedViolations tests -----

    #[test]
    fn batched_violations_empty() {
        let bv = BatchedViolations::new();
        assert!(bv.is_empty());
        assert_eq!(bv.total(), 0);
        assert!(!bv.has_critical());
    }

    #[test]
    fn batched_violations_add_and_query() {
        let mut bv = BatchedViolations::new();
        bv.add(InvariantViolation::new("memory_safety", "leak detected", Severity::High));
        bv.add(InvariantViolation::new("capability", "use-after-cap-drop", Severity::Medium));
        bv.add(InvariantViolation::new("aliasing", "minor alias concern", Severity::Low));

        assert_eq!(bv.total(), 3);
        assert!(bv.has_critical());
        assert_eq!(bv.by_severity_level(Severity::High).len(), 1);
        assert_eq!(bv.by_severity_level(Severity::Medium).len(), 1);
        assert_eq!(bv.by_severity_level(Severity::Low).len(), 1);
    }

    #[test]
    fn batched_violations_report() {
        let mut bv = BatchedViolations::new();
        bv.add(InvariantViolation::new("memory_safety", "leak", Severity::High));
        let report = bv.report();
        assert!(report.contains("1 total"));
        assert!(report.contains("HIGH severity"));
        assert!(report.contains("leak"));
    }

    #[test]
    fn batched_violations_display() {
        let mut bv = BatchedViolations::new();
        bv.add(InvariantViolation::new("test", "msg", Severity::Low));
        let s = format!("{}", bv);
        assert!(s.contains("1 total"));
    }

    #[test]
    fn batched_violations_severity_ordering() {
        let mut bv = BatchedViolations::new();
        bv.add(InvariantViolation::new("low_inv", "low", Severity::Low));
        bv.add(InvariantViolation::new("high_inv", "high", Severity::High));
        bv.add(InvariantViolation::new("med_inv", "med", Severity::Medium));

        let all = bv.all();
        assert_eq!(all[0].invariant, "low_inv");
        assert_eq!(all[1].invariant, "high_inv");
        assert_eq!(all[2].invariant, "med_inv");

        // by_severity_level returns only matching
        assert_eq!(bv.by_severity_level(Severity::High).len(), 1);
        assert_eq!(bv.by_severity_level(Severity::High)[0].invariant, "high_inv");
    }

    // ----- Additional BatchedViolations tests -----

    #[test]
    fn batched_violations_by_severity_grouped() {
        let mut bv = BatchedViolations::new();
        bv.add(InvariantViolation::new("h1", "high1", Severity::High));
        bv.add(InvariantViolation::new("m1", "med1", Severity::Medium));
        bv.add(InvariantViolation::new("h2", "high2", Severity::High));

        let grouped = bv.by_severity();
        assert_eq!(grouped.get(&Severity::High).unwrap().len(), 2);
        assert_eq!(grouped.get(&Severity::Medium).unwrap().len(), 1);
        assert!(grouped.get(&Severity::Low).is_none());
    }

    #[test]
    fn batched_violations_public_violations_field() {
        let mut bv = BatchedViolations::new();
        bv.add(InvariantViolation::new("test", "msg", Severity::Low));
        // violations is a public field
        assert_eq!(bv.violations.len(), 1);
        assert_eq!(bv.violations[0].invariant, "test");
    }

    #[test]
    fn batched_violations_by_severity_empty() {
        let bv = BatchedViolations::new();
        let grouped = bv.by_severity();
        assert!(grouped.is_empty());
    }

    #[test]
    fn batched_violations_total_matches() {
        let mut bv = BatchedViolations::new();
        bv.add(InvariantViolation::new("a", "a", Severity::High));
        bv.add(InvariantViolation::new("b", "b", Severity::High));
        bv.add(InvariantViolation::new("c", "c", Severity::Medium));
        bv.add(InvariantViolation::new("d", "d", Severity::Low));
        assert_eq!(bv.total(), 4);
        assert_eq!(bv.violations.len(), bv.total());
    }

    #[test]
    fn batched_violations_add_parameter_name() {
        let mut bv = BatchedViolations::new();
        let v = InvariantViolation::new("inv", "desc", Severity::Medium);
        bv.add(v);
        assert_eq!(bv.total(), 1);
    }
}
