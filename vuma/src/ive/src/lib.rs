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
//! - [`liveness`]            — Liveness invariant verifier.
//! - [`exclusivity`]         — Exclusivity invariant verifier.
//! - [`interpretation`]      — Interpretation invariant verifier.
//! - [`origin`]              — Origin invariant verifier.
//! - [`cleanup`]             — Cleanup invariant verifier.
//! - [`bd_solver`]           — BD fixpoint constraint solver.
//!
//! # Example
//!
//! ```rust
//! use vuma_ive::{InferenceEngine, VerificationEngine, VerificationInput};
//! use vuma_scg::SCG;
//!
//! let scg = SCG::new();
//!
//! // Run BD inference
//! let inference = InferenceEngine::new();
//! let inference_result = inference.infer(&scg);
//! assert!(inference_result.bd_map.is_empty()); // empty SCG has no nodes
//!
//! // Run verification — BD inference happens internally if not provided
//! let verification = VerificationEngine::new();
//! let input = VerificationInput::from_scg(scg);
//! let results = verification.verify_all(&input);
//! // results is a Vec<VerificationResult> — one per invariant
//! ```

pub mod bd_solver;
pub mod cache;
pub mod cleanup;
pub mod constraint;
pub mod debt;
pub mod escape;
pub mod exclusivity;
pub mod inference;
pub mod interpretation;
pub mod interprocedural;
pub mod invariant_aggregator;
pub mod liveness;
pub mod origin;
pub mod result;
pub mod verification;

// Re-export the primary public API.
pub use cache::{
    compute_fingerprint, InvariantViolation as CacheInvariantViolation, Severity as CacheSeverity,
    VerificationCache,
};
pub use cleanup::{
    CleanupGraph, CleanupReport, CleanupVerifier, CleanupViolation, NodeId as CleanupNodeId,
    OperationKind, ResourceId as CleanupResourceId, ResourceKind as CleanupResourceKind,
    ViolationKind,
};
pub use constraint::{Constraint, ConstraintId};
pub use debt::{DebtItem, DebtStatus, Priority, VerificationDebt};
pub use escape::{analyze_escapes, EscapeKind};
pub use exclusivity::{
    AccessId as ExclusivityAccessId, AccessKind as ExclusivityAccessKind, AccessRecord, CapDInfo,
    Conflict, ConflictKind, ExclusivityInput, ExclusivityOutput, ExclusivityVerifier,
    InterferenceGraph, SyncEdgeRecord, SyncOrdering,
};
pub use inference::{InferenceEngine, InferenceError, InferenceResult};
pub use interpretation::{
    AccessEvent, CapDTransitionResult, InterpretationStrictness, InterpretationVerificationDetail,
    InterpretationVerifier, InterpretationViolation, LocationId, ProgramPointId, SafetyProof,
    UnverifiedPair, VerificationWarning, WriteReadPair,
};
pub use interprocedural::{
    compute_summaries, verify_interprocedural_invariants, FunctionSummary, InterproceduralViolation,
};
pub use invariant_aggregator::{
    AggregatedResult, DiagnosticsReport, InvariantAggregator, InvariantDelta, InvariantKind,
    OverallVerdict, VerificationLevel, VerificationSummary,
};
pub use liveness::{
    verify_liveness, EventAction, LivenessInput, LivenessVerificationResult, LivenessVerifier,
    LivenessViolation, ObligationKind, PointId, ProofObligation, ResourceEvent, ResourceId,
    ResourceKind, ThreadId, WaitForDependency,
};
pub use result::{
    Assumption, BatchedViolations, ConfidenceLevel, CounterExample, Evidence, InvariantName,
    InvariantViolation, ProgramPoint, ProofStep, Severity, VerificationResult, VerificationStatus,
};
pub use verification::{VerificationEngine, VerificationInput};
