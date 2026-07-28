//! IVE — Inference and Verification Engine for VUMA.
//!
//! The IVE module is responsible for:
//!
//! 1. **Inference**: Deriving behavioral descriptions (BDs), constraints,
//!    and type information from the Semantic Compute Graph (SCG).
//! 2. **Verification**: Checking the PMT state invariant (state-field
//!    reads/writes + state transformations) against program fragments
//!    and returning structured verification results.  (Legacy cleanup:
//!    the five pointer-invariant verifiers — liveness / exclusivity /
//!    interpretation / origin / cleanup — have been removed.)
//! 3. **Debt tracking**: Recording verification obligations that have not
//!    yet been discharged, ordered by priority.
//!
//! # Module Layout
//!
//! - [`inference`]           — Inference engine (BD propagation, constraint derivation).
//! - [`verification`]        — Verification engine (PMT state check).
//! - [`invariant_aggregator`] — Aggregator that runs the PMT check and produces unified results.
//! - [`result`]              — Verification result and status types.
//! - [`debt`]                — Verification debt tracking.
//! - [`constraint`]          — Constraint types (temporal, resource flow, security, …).
//!
//! # Example
//!
//! ```rust,no_run
//! use vuma_ive::{InferenceEngine, VerificationEngine, VerificationInput};
//! use vuma_codegen::scg_to_ir::Scg;
//!
//! let scg = Scg::new(vec![]);
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

#[macro_use]
mod vuma_log_w44 {
    /// Logging macro for VUMA compiler diagnostics.
    ///
    /// In debug builds: always emits to stderr.
    /// In release builds: emits to stderr if `VUMA_LOG` env var is set.
    /// This ensures advisory verification warnings are visible in production.
    #[macro_export]
    macro_rules! vuma_log {
        ($level:ident, $($arg:tt)*) => {{
            let emit = cfg!(debug_assertions) || std::env::var("VUMA_LOG").is_ok();
            if emit {
                eprintln!("[{}] {}", stringify!($level), format!($($arg)*));
            }
        }};
    }
}
pub mod arena_bounds;
pub mod borrow_region;
pub mod cache;
pub mod constraint;
pub mod debt;
pub mod inference;
/// CT2: Information-flow type checker (security-label lattice).
pub mod information_flow;
pub mod invariant_aggregator;
pub mod query;
pub mod result;
/// CT1: Session type checker (compile-time protocol verification).
pub mod session_type;
pub mod state_read;
pub mod state_transform;
pub mod state_write;
pub mod verification;

// Re-export the primary public API.
pub use cache::{
    compute_fingerprint, InvariantViolation as CacheInvariantViolation, Severity as CacheSeverity,
    VerificationCache,
};
pub use constraint::{Constraint, ConstraintId};
pub use debt::{DebtItem, DebtStatus, Priority, VerificationDebt};
pub use inference::{InferenceEngine, InferenceError, InferenceResult};
pub use invariant_aggregator::{
    AggregatedResult, DiagnosticsReport, InvariantAggregator, InvariantDelta, InvariantKind,
    OverallVerdict, VerificationLevel, VerificationSummary,
};
pub use result::{
    Assumption, BatchedViolations, ConfidenceLevel, CounterExample, Evidence, InvariantName,
    InvariantViolation, ProgramPoint, ProofStep, Severity, VerificationResult, VerificationStatus,
};
pub use verification::{PmtFieldSpec, PmtLayoutSpec, VerificationEngine, VerificationInput};
