//! Verification result types for the IVE module.
//!
//! This module defines the core result and status types used by the
//! verification engine to report outcomes of invariant checks.

use serde::{Deserialize, Serialize};
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

// ---------------------------------------------------------------------------
// VerificationResult
// ---------------------------------------------------------------------------

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
}
