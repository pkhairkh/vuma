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
//! - [`exclusivity`]         — Single-threaded exclusivity invariant verifier.
//! - [`exclusivity_concurrent`] — Thread-aware exclusivity and data-race detection.
//! - [`interpretation`]      — Interpretation invariant verifier.
//! - [`liveness`]            — Liveness invariant verifier.
//! - [`origin`]              — Origin invariant verifier.
//! - [`cleanup`]             — Cleanup invariant verifier.
//! - [`dependency`]          — Cross-invariant dependency analysis.
//! - [`error_recovery`]      — Error recovery suggestions and partial verification.
//! - [`bd_solver`]           — BD constraint solver (iterative + fixpoint).
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
pub mod dependency;
pub mod error_recovery;
pub mod exclusivity;
pub mod exclusivity_concurrent;
pub mod inference;
pub mod interpretation;
pub mod invariant_aggregator;
pub mod liveness;
pub mod origin;
pub mod result;
pub mod verification;

// ---------------------------------------------------------------------------
// Re-exports: constraint
// ---------------------------------------------------------------------------
pub use constraint::{
    AccessPattern, ComplexityConstraint, Constraint, ConstraintCombinator, ConstraintId,
    ConstraintSolution, ConstraintSolver, LivenessConstraint, RegionConstraintKind,
    ResourceFlowConstraint, SecurityConstraint, SolutionStatus, TemporalConstraint,
    TemporalRelation,
};

// ---------------------------------------------------------------------------
// Re-exports: debt
// ---------------------------------------------------------------------------
pub use debt::{
    AgedDebt, AutoResolution, DebtContext, DebtItem, DebtReport, DebtScore, DebtStatus, DebtTrend,
    Priority, VerificationDebt, VerificationDebtTracker,
};

// ---------------------------------------------------------------------------
// Re-exports: inference
// ---------------------------------------------------------------------------
pub use inference::{BD, InferenceEngine, InferenceError, NodeId, SCG};

// ---------------------------------------------------------------------------
// Re-exports: result
// ---------------------------------------------------------------------------
pub use result::{
    Assumption, ConfidenceLevel, CounterExample, Evidence, EvidenceCombinator, InvariantName,
    ProgramPoint, ProofStep, VerificationResult, VerificationStatus, WitnessState,
};

// ---------------------------------------------------------------------------
// Re-exports: invariant_aggregator
// ---------------------------------------------------------------------------
pub use invariant_aggregator::{
    AggregatedResult, AggregatorConfig, DiagnosticEntry, DiagnosticsReport, InvariantAggregator,
    InvariantDelta, InvariantKind, OverallVerdict, PerInvariantResult, VerificationContext,
    VerificationLevel, VerificationSummary, OPTIMAL_INVARIANT_ORDER,
};

// ---------------------------------------------------------------------------
// Re-exports: verification
// ---------------------------------------------------------------------------
pub use verification::{Message, VerificationEngine};

// ---------------------------------------------------------------------------
// Re-exports: interpretation
// ---------------------------------------------------------------------------
pub use interpretation::{
    AccessEvent, BitCastRisk, CapDTransitionResult, CastKind, CastProofObligation, CastRecord,
    CastValidationResult, DeepConfusionKind, EnumVariantTracker, InterpretationVerifier,
    InterpretationViolation, LocationId, ProgramPointId, ProofDifficulty, SafetyProof,
    UnionDiscriminator, WriteReadPair,
};

// ---------------------------------------------------------------------------
// Re-exports: liveness
// ---------------------------------------------------------------------------
pub use liveness::{
    DeadAllocation, DeadReason, EventAction, InitializationMap, LivenessInput, LivenessPath,
    LivenessVerificationResult, LivenessVerifier, LivenessViolation, ObligationKind,
    PartialInitViolation, PointId, ProofObligation, ResourceEvent, ResourceId, ResourceKind,
    ThreadId, VerificationContext as LivenessVerificationContext, WaitForDependency, verify_liveness,
};

// ---------------------------------------------------------------------------
// Re-exports: dependency
// ---------------------------------------------------------------------------
pub use dependency::{
    CyclicDependency, DependencyEdge, DependencyStrength, DependencyViolation, ImpactSet,
    InvariantDependencyGraph, ReVerificationPlan, ReVerificationStep,
};

// ---------------------------------------------------------------------------
// Re-exports: cleanup
// ---------------------------------------------------------------------------
pub use cleanup::{
    AnnotatedCleanupGraph, AnnotationIssue, AnnotationIssueKind, CleanupGraph, CleanupReport,
    CleanupVerifier, CleanupViolation, LeakAnnotation, LeakReason, NodeId as CleanupNodeId,
    OperationKind, ResourceId as CleanupResourceId, ResourceKind as CleanupResourceKind,
    ViolationKind as CleanupViolationKind,
};

// ---------------------------------------------------------------------------
// Re-exports: exclusivity
// ---------------------------------------------------------------------------
pub use exclusivity::{
    AccessId as ExclusivityAccessId, AccessIntervalTree, AccessKind as ExclusivityAccessKind,
    AccessRecord, CapDInfo, Conflict, ConflictKind, DerivationAliasInfo, ExclusivityInput,
    ExclusivityOutput, ExclusivityProofObligation, ExclusivityVerifier, InterferenceGraph,
    ProofDifficulty as ExclusivityProofDifficulty, ResolutionKind, SuggestedFix, SyncEdgeRecord,
    SyncOrdering,
};

// ---------------------------------------------------------------------------
// Re-exports: exclusivity_concurrent
// ---------------------------------------------------------------------------
pub use exclusivity_concurrent::{
    ConcurrentExclusivityInput, ConcurrentExclusivityOutput, ConcurrentExclusivityVerifier,
    DataRace, DeadlockWarning, HappensBeforeGraph, HBRelation, ThreadAccess,
    ThreadId as ConcurrentThreadId, detect_data_races, detect_potential_deadlocks,
};

// ---------------------------------------------------------------------------
// Re-exports: origin
// ---------------------------------------------------------------------------
pub use origin::{
    Address as OriginAddress, CastRecord as OriginCastRecord, CastClassification,
    DerivationId, DerivationSource, DerivationStep, DerivationViolation, ForgedPointerDetector,
    OriginReport, OriginRoot, OriginVerificationResult, OriginVerifier, ProvenanceEdge,
    ProvenanceGraph, ProvenanceGraphNode, ProvenanceNodeKind, Region as OriginRegion,
    RegionId as OriginRegionId, TaintLevel, ViolationKind as OriginViolationKind,
};

// ---------------------------------------------------------------------------
// Re-exports: bd_solver
// ---------------------------------------------------------------------------
pub use bd_solver::{
    BDConstraint, BDConstraintSolver, BDFixpointSolver, BDProofObligation, BDObligationKind,
    FlowKind, SolverError, SolverResult,
};

// ---------------------------------------------------------------------------
// Re-exports: error_recovery
// ---------------------------------------------------------------------------
pub use error_recovery::{
    ErrorCollector, ErrorSeverity, ErrorSummary, PartialVerificationResult, SafeRegion,
    SuggestedFix as RecoverySuggestedFix, UnsafeRegion, VerificationError,
};
