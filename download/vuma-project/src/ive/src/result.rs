//! Verification result types for the IVE module.
//!
//! This module defines the core result and status types used by the
//! verification engine to report outcomes of invariant checks.
//!
//! # Graduated Assurance Model
//!
//! Results carry a [`ConfidenceLevel`] ranging from `Exhaustive` (100, formal
//! proof with all paths checked) down to `Unverified` (0, no evidence at all).
//! The [`VerificationResult::composite_confidence`] method factors in
//! dependencies on other invariants, producing a conservative overall score.
//!
//! # Machine-Readable Output
//!
//! Every [`VerificationResult`] can be serialised to JSON via
//! [`VerificationResult::to_json`], enabling integration with CI pipelines
//! and downstream tooling.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

/// The name of an invariant being verified.
pub type InvariantName = String;

/// A point in the program, used for counterexample traces.
pub type ProgramPoint = String;

/// An assumption made during verification that has not been formally proven.
pub type Assumption = String;

/// A single step in a formal proof.
pub type ProofStep = String;

// ---------------------------------------------------------------------------
// ConfidenceLevel
// ---------------------------------------------------------------------------

/// Graduated confidence level for verification results.
///
/// Each variant carries an explicit numerical value accessible via
/// [`ConfidenceLevel::numerical`]. The derived `Ord` implementation orders
/// levels from `Unverified` (lowest) to `Exhaustive` (highest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ConfidenceLevel {
    /// All paths checked, formal proof — the strongest assurance.
    Exhaustive = 100,
    /// Nearly exhaustive, small assumptions.
    VeryHigh = 90,
    /// Strong evidence, few assumptions.
    High = 75,
    /// Moderate evidence, some assumptions.
    Medium = 50,
    /// Weak evidence, many assumptions.
    Low = 25,
    /// Minimal evidence.
    VeryLow = 10,
    /// No evidence at all.
    Unverified = 0,
}

impl ConfidenceLevel {
    /// Return the numerical score associated with this confidence level.
    pub fn numerical(&self) -> u8 {
        match self {
            Self::Exhaustive => 100,
            Self::VeryHigh => 90,
            Self::High => 75,
            Self::Medium => 50,
            Self::Low => 25,
            Self::VeryLow => 10,
            Self::Unverified => 0,
        }
    }

    /// Returns `true` if this confidence level meets or exceeds `min`.
    pub fn meets_threshold(&self, min: ConfidenceLevel) -> bool {
        self >= &min
    }

    /// Return the next-lower confidence level, or `None` if already at
    /// `Unverified`.
    fn decrement(self) -> Option<ConfidenceLevel> {
        match self {
            Self::Exhaustive => Some(Self::VeryHigh),
            Self::VeryHigh => Some(Self::High),
            Self::High => Some(Self::Medium),
            Self::Medium => Some(Self::Low),
            Self::Low => Some(Self::VeryLow),
            Self::VeryLow => Some(Self::Unverified),
            Self::Unverified => None,
        }
    }
}

impl fmt::Display for ConfidenceLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exhaustive => write!(f, "EXHAUSTIVE({})", self.numerical()),
            Self::VeryHigh => write!(f, "VERY_HIGH({})", self.numerical()),
            Self::High => write!(f, "HIGH({})", self.numerical()),
            Self::Medium => write!(f, "MEDIUM({})", self.numerical()),
            Self::Low => write!(f, "LOW({})", self.numerical()),
            Self::VeryLow => write!(f, "VERY_LOW({})", self.numerical()),
            Self::Unverified => write!(f, "UNVERIFIED({})", self.numerical()),
        }
    }
}

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
// EvidenceCombinator
// ---------------------------------------------------------------------------

/// Describes how two pieces of [`Evidence`] are combined.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EvidenceCombinator {
    /// Both pieces of evidence must hold (conjunction).
    Conjunction,
    /// Either piece of evidence suffices (disjunction).
    Disjunction,
    /// Primary evidence with secondary as fallback (weakening).
    Weakening,
}

// ---------------------------------------------------------------------------
// WitnessState
// ---------------------------------------------------------------------------

/// A snapshot of the program state at the point of a violation, useful for
/// reproducing and debugging counterexamples.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WitnessState {
    /// Memory snapshot as (address, raw bytes) pairs.
    pub memory_snapshot: Vec<(u64, Vec<u8>)>,
    /// Resource IDs that were active at the violation point.
    pub active_resources: Vec<u64>,
    /// Lock IDs that were held at the violation point.
    pub held_locks: Vec<u64>,
    /// High-level descriptions of each thread's state.
    pub thread_states: Vec<String>,
}

impl WitnessState {
    /// Construct an empty witness state.
    pub fn empty() -> Self {
        Self {
            memory_snapshot: Vec::new(),
            active_resources: Vec::new(),
            held_locks: Vec::new(),
            thread_states: Vec::new(),
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
    /// Optional snapshot of program state at the violation point.
    #[serde(default)]
    pub witness_state: Option<WitnessState>,
    /// Step-by-step instructions to reproduce the violation.
    #[serde(default)]
    pub reproduction_steps: Vec<String>,
}

impl CounterExample {
    /// Construct a new counterexample (backward-compatible signature).
    pub fn new(
        execution_path: Vec<ProgramPoint>,
        violation_point: ProgramPoint,
        description: String,
    ) -> Self {
        Self {
            execution_path,
            violation_point,
            description,
            witness_state: None,
            reproduction_steps: Vec::new(),
        }
    }

    /// Attach a witness state to this counterexample.
    pub fn with_witness_state(mut self, ws: WitnessState) -> Self {
        self.witness_state = Some(ws);
        self
    }

    /// Attach reproduction steps to this counterexample.
    pub fn with_reproduction_steps(mut self, steps: Vec<String>) -> Self {
        self.reproduction_steps = steps;
        self
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
    /// Evidence from statistical or sampling-based analysis (legacy).
    StatisticalAnalysis,
    /// Evidence from sampling a subset of the state space.
    SamplingAnalysis {
        /// Number of samples examined.
        sample_size: usize,
        /// Total population size.
        total: usize,
    },
    /// Evidence from explicit-state model checking.
    ModelChecking {
        /// Number of states explored.
        states_explored: u64,
        /// Total number of reachable states, if known.
        states_total: Option<u64>,
    },
    /// Evidence from statistical inference with quantified uncertainty.
    StatisticalInference {
        /// Confidence level (0.0 – 1.0).
        confidence: f64,
        /// p-value of the test.
        p_value: f64,
    },
    /// Evidence from heuristic-based analysis.
    HeuristicAnalysis {
        /// Names of heuristics that were applied.
        heuristics_applied: Vec<String>,
    },
    /// Evidence composed from two sub-evidences.
    Composed {
        /// Primary evidence.
        primary: Box<Evidence>,
        /// Secondary (supporting or fallback) evidence.
        secondary: Box<Evidence>,
        /// How the two pieces of evidence are combined.
        combinator: EvidenceCombinator,
    },
}

// ---------------------------------------------------------------------------
// Serde helpers for std::time::Duration
// ---------------------------------------------------------------------------

mod duration_ms {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match duration {
            Some(d) => serializer.serialize_some(&d.as_millis()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<u64> = Option::deserialize(deserializer)?;
        Ok(opt.map(Duration::from_millis))
    }
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
    /// Optional single piece of evidence supporting the result (legacy).
    pub evidence: Option<Evidence>,

    // -- Enhanced fields --
    /// Confidence level for this result.
    #[serde(default = "default_confidence")]
    pub confidence: ConfidenceLevel,
    /// Ordered chain of evidence supporting this result.
    #[serde(default)]
    pub evidence_chain: Vec<Evidence>,
    /// Wall-clock time spent on this verification.
    #[serde(default, with = "duration_ms")]
    pub verification_time: Option<Duration>,
    /// Names of other invariants that this result depends on.
    #[serde(default)]
    pub invariant_dependencies: Vec<String>,
}

fn default_confidence() -> ConfidenceLevel {
    ConfidenceLevel::Unverified
}

/// Derive the default confidence level from a verification status.
fn confidence_from_status(status: &VerificationStatus) -> ConfidenceLevel {
    match status {
        VerificationStatus::Proven => ConfidenceLevel::High,
        VerificationStatus::ProbablySafe { .. } => ConfidenceLevel::Medium,
        VerificationStatus::Unverified { .. } => ConfidenceLevel::Low,
        VerificationStatus::Violated { .. } => ConfidenceLevel::Low,
    }
}

impl VerificationResult {
    /// Construct a new verification result.
    pub fn new(
        invariant: impl Into<InvariantName>,
        status: VerificationStatus,
        message: impl Into<String>,
    ) -> Self {
        let conf = confidence_from_status(&status);
        Self {
            invariant: invariant.into(),
            status,
            message: message.into(),
            evidence: None,
            confidence: conf,
            evidence_chain: Vec::new(),
            verification_time: None,
            invariant_dependencies: Vec::new(),
        }
    }

    /// Attach a single piece of evidence to this result (legacy builder).
    pub fn with_evidence(mut self, evidence: Evidence) -> Self {
        self.evidence = Some(evidence);
        self
    }

    /// Override the confidence level for this result.
    pub fn with_confidence(mut self, confidence: ConfidenceLevel) -> Self {
        self.confidence = confidence;
        self
    }

    /// Set the evidence chain for this result.
    pub fn with_evidence_chain(mut self, chain: Vec<Evidence>) -> Self {
        self.evidence_chain = chain;
        self
    }

    /// Record the wall-clock time spent on verification.
    pub fn with_verification_time(mut self, duration: Duration) -> Self {
        self.verification_time = Some(duration);
        self
    }

    /// Declare dependencies on other invariants.
    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.invariant_dependencies = deps;
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
        self.confidence
    }

    /// Compute a composite confidence that factors in dependencies on other
    /// invariants.
    ///
    /// Each dependency reduces the confidence by one level (conservative
    /// assumption that the dependency may not hold at the same level). The
    /// result is floored at [`ConfidenceLevel::Unverified`].
    pub fn composite_confidence(&self) -> ConfidenceLevel {
        let mut level = self.confidence;
        for _ in &self.invariant_dependencies {
            level = level.decrement().unwrap_or(ConfidenceLevel::Unverified);
        }
        level
    }

    /// Serialise this result to a JSON string (machine-readable output).
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|e| {
            format!(
                r#"{{"error":"serialization failed","details":"{}"}}"#,
                e.to_string().replace('"', "\\\"")
            )
        })
    }
}

impl fmt::Display for VerificationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} — {} (confidence: {})",
            self.status, self.invariant, self.message, self.confidence
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ConfidenceLevel numerical values ----

    #[test]
    fn confidence_level_numerical_values() {
        assert_eq!(ConfidenceLevel::Exhaustive.numerical(), 100);
        assert_eq!(ConfidenceLevel::VeryHigh.numerical(), 90);
        assert_eq!(ConfidenceLevel::High.numerical(), 75);
        assert_eq!(ConfidenceLevel::Medium.numerical(), 50);
        assert_eq!(ConfidenceLevel::Low.numerical(), 25);
        assert_eq!(ConfidenceLevel::VeryLow.numerical(), 10);
        assert_eq!(ConfidenceLevel::Unverified.numerical(), 0);
    }

    // ---- ConfidenceLevel meets_threshold ----

    #[test]
    fn confidence_meets_threshold() {
        assert!(ConfidenceLevel::High.meets_threshold(ConfidenceLevel::High));
        assert!(ConfidenceLevel::Exhaustive.meets_threshold(ConfidenceLevel::High));
        assert!(!ConfidenceLevel::Medium.meets_threshold(ConfidenceLevel::High));
        assert!(ConfidenceLevel::Medium.meets_threshold(ConfidenceLevel::Medium));
        assert!(!ConfidenceLevel::Low.meets_threshold(ConfidenceLevel::Medium));
        assert!(ConfidenceLevel::Low.meets_threshold(ConfidenceLevel::Low));
        assert!(!ConfidenceLevel::Unverified.meets_threshold(ConfidenceLevel::VeryLow));
    }

    // ---- Evidence composition (conjunction) ----

    #[test]
    fn evidence_composition_conjunction() {
        let primary = Evidence::ExhaustiveAnalysis;
        let secondary = Evidence::FormalProof {
            steps: vec!["step1".into(), "step2".into()],
        };
        let composed = Evidence::Composed {
            primary: Box::new(primary),
            secondary: Box::new(secondary),
            combinator: EvidenceCombinator::Conjunction,
        };
        if let Evidence::Composed {
            combinator: EvidenceCombinator::Conjunction,
            ..
        } = composed
        {
            // ok
        } else {
            panic!("expected Conjunction combinator");
        }
    }

    // ---- Witness state construction ----

    #[test]
    fn witness_state_construction() {
        let ws = WitnessState {
            memory_snapshot: vec![(0x1000u64, vec![0xDE, 0xAD])],
            active_resources: vec![42u64],
            held_locks: vec![7u64],
            thread_states: vec!["running".into(), "blocked".into()],
        };
        assert_eq!(ws.memory_snapshot.len(), 1);
        assert_eq!(ws.active_resources, vec![42]);
        assert_eq!(ws.held_locks, vec![7]);
        assert_eq!(ws.thread_states.len(), 2);

        let empty = WitnessState::empty();
        assert!(empty.memory_snapshot.is_empty());
        assert!(empty.active_resources.is_empty());
    }

    // ---- JSON export produces valid JSON ----

    #[test]
    fn json_export_produces_valid_json() {
        let r = VerificationResult::new("test_inv", VerificationStatus::Proven, "all good")
            .with_confidence(ConfidenceLevel::Exhaustive);
        let json = r.to_json();
        // Must be valid JSON — round-trip through serde_json.
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("to_json must produce valid JSON");
        assert_eq!(parsed["invariant"], "test_inv");
        assert_eq!(parsed["status"]["Proven"], null_check());
        assert_eq!(parsed["confidence"], "Exhaustive");
    }

    /// Helper: in serde_json, a unit variant serialises as `null` inside an
    /// object with one key (tagged). We just need a sentinel to assert
    /// against; the real check is that parsing succeeds.
    fn null_check() -> serde_json::Value {
        serde_json::Value::Null
    }

    // ---- Composite confidence with dependencies ----

    #[test]
    fn composite_confidence_with_dependencies() {
        let r = VerificationResult::new("inv_a", VerificationStatus::Proven, "proven")
            .with_dependencies(vec!["inv_b".into(), "inv_c".into()]);
        // High(75) decremented twice: High → Medium → Low
        assert_eq!(r.composite_confidence(), ConfidenceLevel::Low);
    }

    #[test]
    fn composite_confidence_no_dependencies() {
        let r = VerificationResult::new("inv_a", VerificationStatus::Proven, "proven");
        assert_eq!(r.composite_confidence(), ConfidenceLevel::High);
    }

    #[test]
    fn composite_confidence_floored_at_unverified() {
        let r = VerificationResult::new("inv_a", VerificationStatus::Proven, "proven")
            .with_confidence(ConfidenceLevel::VeryLow)
            .with_dependencies(vec![
                "d1".into(),
                "d2".into(),
                "d3".into(),
            ]);
        // VeryLow → Unverified after first dep; stays Unverified
        assert_eq!(r.composite_confidence(), ConfidenceLevel::Unverified);
    }

    // ---- Verification result with timing ----

    #[test]
    fn verification_result_with_timing() {
        let r = VerificationResult::new("timed_inv", VerificationStatus::Proven, "checked")
            .with_verification_time(Duration::from_millis(42));
        assert_eq!(r.verification_time, Some(Duration::from_millis(42)));
    }

    // ---- CounterExample with reproduction steps ----

    #[test]
    fn counterexample_with_reproduction_steps() {
        let ce = CounterExample::new(
            vec!["entry".into(), "loop".into()],
            "loop".into(),
            "infinite loop".into(),
        )
        .with_reproduction_steps(vec![
            "1. Allocate resource R".into(),
            "2. Enter loop without releasing R".into(),
            "3. Observe infinite loop".into(),
        ]);
        assert_eq!(ce.reproduction_steps.len(), 3);
        assert_eq!(ce.reproduction_steps[0], "1. Allocate resource R");
    }

    #[test]
    fn counterexample_with_witness_state() {
        let ws = WitnessState {
            memory_snapshot: vec![(0x2000, vec![0xBE, 0xEF])],
            active_resources: vec![1],
            held_locks: vec![],
            thread_states: vec!["idle".into()],
        };
        let ce = CounterExample::new(
            vec!["start".into()],
            "crash".into(),
            "segfault".into(),
        )
        .with_witness_state(ws.clone());
        assert!(ce.witness_state.is_some());
        assert_eq!(ce.witness_state.unwrap(), ws);
    }

    // ---- Legacy tests (preserved) ----

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
        let displayed = format!("{r}");
        assert!(displayed.contains("[PROVEN]"));
        assert!(displayed.contains("test"));
        assert!(displayed.contains("all good"));
        assert!(displayed.contains("confidence"));
    }

    // ---- JSON round-trip test ----

    #[test]
    fn json_roundtrip() {
        let ce = CounterExample::new(
            vec!["a".into(), "b".into()],
            "b".into(),
            "bad access".into(),
        )
        .with_reproduction_steps(vec!["step1".into()])
        .with_witness_state(WitnessState {
            memory_snapshot: vec![(0x100, vec![0x00])],
            active_resources: vec![99],
            held_locks: vec![],
            thread_states: vec![],
        });

        let r = VerificationResult::new(
            "inv",
            VerificationStatus::Violated { counterexample: ce },
            "violation found",
        )
        .with_confidence(ConfidenceLevel::Low)
        .with_evidence_chain(vec![Evidence::ModelChecking {
            states_explored: 1024,
            states_total: Some(2048),
        }])
        .with_verification_time(Duration::from_millis(123))
        .with_dependencies(vec!["other_inv".into()]);

        let json = r.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["invariant"], "inv");
        assert_eq!(parsed["confidence"], "Low");
        assert!(parsed["evidence_chain"].is_array());
    }
}
