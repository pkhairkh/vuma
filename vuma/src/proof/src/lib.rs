//! # VUMA Proof Module
//!
//! Formal proof objects and verification for the VUMA language framework.
//!
//! This crate provides the core data structures and algorithms for constructing,
//! checking, and manipulating formal proofs about memory safety invariants in
//! VUMA programs. It supports:
//!
//! - **Proof objects**: Structured representations of formal proofs with goals,
//!   steps, and conclusions.
//! - **Inference rules**: Domain-specific rules for reasoning about liveness,
//!   exclusivity, derivation chains, bounds preservation, cast validity, and
//!   temporal ordering.
//! - **Proof checking**: Automated verification that proof steps follow from
//!   previous steps using the stated rules, with circular-reasoning detection.
//! - **Counterexample generation**: Construction of minimal counterexamples
//!   from proof failures to aid debugging.
//! - **Proof tactics**: Automated proof strategies including simplification,
//!   induction, contradiction, and auto-mode.

pub mod checker;
pub mod cleanup_proofs;
pub mod composition;
pub mod counterexample;
pub mod exclusivity_proofs;
pub mod interpretation_proofs;
pub mod judgment;
pub mod liveness_proofs;
pub mod models;
pub mod origin_proofs;
pub mod proof;
pub mod rules;
pub mod serialization;
pub mod tactics;

// Re-export the primary types for convenience
pub use checker::{CheckResult, ProofChecker};
pub use counterexample::{CounterExample, Step, ViolationPoint};
pub use judgment::{CapDKind, EventId, Judgment, PointerId, RegionId, ResourceId, VariableId};
pub use models::{
    Addr, BDKind, Compatibility, DerivationId, LockId, OriginInfo, OriginInfoBuilder,
    ProofAccess, ProofAccessKind, ProofDerivation, ProofMemOp, ProofMemOpKind, ProofMSG,
    ProofRegion, ProofRegionStatus, ProofRepD, ProofSCG, ProofSCGEdge, ProofSyncEdge,
    RepDId, SinkSensitivity, SourceTrust, SyncEdgeId, SyncOrdering, TaintLabelId,
    valid_reinterpretation,
};
pub use proof::{Conclusion, Fact, FactKind, Goal, InvariantName, Proof, ProofContext, ProofStep, Target};
pub use origin_proofs::{
    DerivationChainProof, OriginProof, OriginTactic, ProofFailure as OriginProofFailure,
    TaintProof, prove_origin,
};
pub use rules::InferenceRule;
pub use tactics::Tactic;
