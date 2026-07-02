//! # Interpretation Proofs
//!
//! Generic proof objects for type-interpretation (cast / view change)
//! safety. These are used by the proof composition and serialization
//! subsystems.
//!
//! The Womb-specific Gestalt verifier that previously lived here has
//! been removed along with the rest of the Womb data-model vaporware
//! (concept / gestalt / manifold / aura).

/// A proof that an interpretation (type cast / view change) is valid.
///
/// This struct is used by the composition system and serialization.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InterpretationProof {
    /// Proofs that BD representations are compatible across the interpretation.
    pub bd_compatibility_proofs: Vec<BDCompatibilityProof>,
    /// Proofs that reinterpretation is safe (no aliasing violations).
    pub reinterpretation_safety_proofs: Vec<ReinterpretationSafetyProof>,
    /// The underlying formal proof.
    pub proof: crate::proof::Proof,
}

/// A proof that two BD representations are compatible.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BDCompatibilityProof {
    /// The formal proof.
    pub proof: crate::proof::Proof,
}

/// A proof that reinterpretation is safe (no aliasing violations).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReinterpretationSafetyProof {
    /// The formal proof.
    pub proof: crate::proof::Proof,
}
