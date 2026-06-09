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
//! - [`inference`]   — Inference engine (BD propagation, constraint derivation).
//! - [`verification`] — Verification engine (5 invariant checks).
//! - [`result`]      — Verification result and status types.
//! - [`debt`]        — Verification debt tracking.
//! - [`constraint`]  — Constraint types (temporal, resource flow, security, …).
//!
//! # Example
//!
//! ```rust
//! use vuma_ive::{InferenceEngine, VerificationEngine, VerificationDebt};
//!
//! let inference = InferenceEngine::new();
//! let verification = VerificationEngine::new();
//! let debt = VerificationDebt::new();
//!
//! // Placeholder — these will operate on real SCG / message types
//! // once the vuma-scg and vuma-bd crates are integrated.
//! ```

pub mod constraint;
pub mod debt;
pub mod inference;
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
pub use verification::{Message, VerificationEngine};
