//! # Womb Data Models — LLM-Native Type System
//!
//! This module implements the four revolutionary data models that replace
//! legacy C-style structs, unions, and arrays:
//!
//! - **Concept**: Relational data with lazy layout inference (replaces struct)
//! - **Gestalt**: Tagless, context-dependent memory superposition (replaces union)
//! - **Manifold**: Multi-dimensional spatial data with space-filling curves (replaces array)
//! - **Aura**: Self-describing metadata for runtime introspection
//!
//! ## Architecture
//!
//! Each model has three layers:
//! 1. **SCG Nodes** (in `vuma-scg`): Graph-level representation
//! 2. **BD Extensions** (in this module): Behavioral descriptor inference
//! 3. **IVE Verification** (in this module): Invariant checking
//!
//! ## The 5 VUMA Invariants
//!
//! All models enforce: Liveness, Exclusivity, Interpretation, Origin, Cleanup.

pub mod concept;
pub mod gestalt;
pub mod manifold;
pub mod aura;

pub use concept::{LayoutResolutionPass, ConceptLayout};
pub use gestalt::{GestaltInterpreter, GestaltProof};
pub use manifold::{ZOrderCurve, HilbertCurve, SpaceFillingCurveLayout};
pub use aura::{AuraHeader, AuraCleanupVerifier};

/// The size of the AuraHeader in bytes.
/// Stored before the base pointer when Aura is attached.
pub const AURA_HEADER_SIZE: u64 = 32;

/// The alignment of the AuraHeader.
pub const AURA_HEADER_ALIGN: u64 = 8;
