//! IVE — Inference and Verification Engine for VUMA.
//!
//! The IVE module is responsible for:
//!
//! 1. **Inference**: Deriving behavioral descriptions (BDs), constraints,
//!    and type information from the Semantic Compute Graph (SCG).
//! 2. **Verification**: Checking VUMA's five core invariants (liveness,
//!    exclusivity, interpretation, origin, cleanup) against program
//!    fragments and returning structured verification results.
//! 3. **Debt tracking**: Recording verification obligations that have not
//!    yet been discharged, ordered by priority.
//!
//! # Module Layout
//!
//! - [`inference`]           — Inference engine (BD propagation, constraint derivation).
//! - [`verification`]        — Verification engine (5 invariant checks).
//! - [`invariant_aggregator`] — Aggregator that runs all checks and produces unified results.
//! - [`result`]              — Verification result and status types.
//! - [`debt`]                — Verification debt tracking.
//! - [`constraint`]          — Constraint types (temporal, resource flow, security, …).
//!
//! # Example
//!
//! ```rust
//! use vuma_ive::{InferenceEngine, VerificationEngine, VerificationDebt, InvariantAggregator};
//!
//! let inference = InferenceEngine::new();
//! let verification = VerificationEngine::new();
//! let debt = VerificationDebt::new();
//! let aggregator = InvariantAggregator::new();
//!
//! // Placeholder — these will operate on real SCG / message types
//! // once the vuma-scg and vuma-bd crates are integrated.
//! ```

pub mod bd_solver;
pub mod cleanup;
pub mod constraint;
pub mod debt;
pub mod exclusivity;
pub mod inference;
pub mod interpretation;
pub mod invariant_aggregator;
pub mod liveness;
pub mod origin;
pub mod result;
pub mod verification;

// Re-export the primary public API.
pub use constraint::{Constraint, ConstraintId};
pub use debt::{DebtItem, DebtStatus, Priority, VerificationDebt};
pub use inference::{BD, InferenceEngine, InferenceError, NodeId, SCG};
pub use result::{
    Assumption, ConfidenceLevel, CounterExample, Evidence, InvariantName, ProgramPoint, ProofStep,
    VerificationResult, VerificationStatus,
};
pub use invariant_aggregator::{
    AggregatedResult, DiagnosticsReport, InvariantAggregator, InvariantDelta, InvariantKind,
    OverallVerdict, VerificationLevel, VerificationSummary,
};
pub use verification::VerificationEngine;
pub use interpretation::{
    AccessEvent, CapDTransitionResult, InterpretationVerifier, InterpretationViolation,
    LocationId, ProgramPointId, SafetyProof, WriteReadPair,
};
pub use liveness::{
    EventAction, LivenessInput, LivenessVerificationResult, LivenessVerifier, LivenessViolation,
    ObligationKind, PointId, ProofObligation, ResourceEvent, ResourceId, ResourceKind, ThreadId,
    WaitForDependency, verify_liveness,
};
pub use cleanup::{
    CleanupGraph, CleanupReport, CleanupVerifier, CleanupViolation, NodeId as CleanupNodeId,
    OperationKind, ResourceId as CleanupResourceId, ResourceKind as CleanupResourceKind,
    ViolationKind,
};
pub use exclusivity::{
    AccessId as ExclusivityAccessId, AccessKind as ExclusivityAccessKind, AccessRecord,
    CapDInfo, Conflict, ConflictKind, ExclusivityInput, ExclusivityOutput,
    ExclusivityVerifier, InterferenceGraph, SyncEdgeRecord, SyncOrdering,
};
