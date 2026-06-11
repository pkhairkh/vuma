//! # Serialization I/O for Proof Objects
//!
//! Provides JSON serialization and deserialization for all proof types through
//! a unified [`ProofEnvelope`] tagged enum. This allows proof objects of
//! different kinds to be serialized, stored, and later deserialized without
//! losing type information.

use crate::cleanup_proofs::CleanupProof;
use crate::exclusivity_proofs::ExclusivityProof;
use crate::interpretation_proofs::InterpretationProof;
use crate::liveness_proofs::LivenessProof;
use crate::origin_proofs::OriginProof;
use crate::proof::Proof;

/// Serialization error type.
#[derive(Debug, thiserror::Error)]
pub enum SerializationError {
    #[error("JSON serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A serializable proof envelope that can hold any proof type.
///
/// Uses internally-tagged serde representation (`#[serde(tag = "type", content = "data")]`)
/// so that the JSON output includes a `"type"` field identifying the proof kind
/// and a `"data"` field carrying the proof payload. This allows heterogeneous
/// collections of proofs to be serialized and deserialized correctly.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
#[allow(clippy::large_enum_variant)]
pub enum ProofEnvelope {
    Liveness(LivenessProof),
    Exclusivity(ExclusivityProof),
    Cleanup(CleanupProof),
    Origin(OriginProof),
    Interpretation(InterpretationProof),
    Generic(Proof),
}

impl ProofEnvelope {
    /// Serialize this proof envelope to a JSON string.
    pub fn to_json_string(&self) -> Result<String, SerializationError> {
        Ok(serde_json::to_string(self)?)
    }

    /// Serialize this proof envelope to a pretty JSON string.
    pub fn to_json_string_pretty(&self) -> Result<String, SerializationError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Deserialize a proof envelope from a JSON string.
    pub fn from_json_string(s: &str) -> Result<Self, SerializationError> {
        Ok(serde_json::from_str(s)?)
    }

    /// Serialize to a writer.
    pub fn to_writer<W: std::io::Write>(&self, writer: W) -> Result<(), SerializationError> {
        Ok(serde_json::to_writer(writer, self)?)
    }

    /// Deserialize from a reader.
    pub fn from_reader<R: std::io::Read>(reader: R) -> Result<Self, SerializationError> {
        Ok(serde_json::from_reader(reader)?)
    }
}

// ---------------------------------------------------------------------------
// From<XXXProof> for ProofEnvelope
// ---------------------------------------------------------------------------

impl From<LivenessProof> for ProofEnvelope {
    fn from(proof: LivenessProof) -> Self {
        ProofEnvelope::Liveness(proof)
    }
}

impl From<ExclusivityProof> for ProofEnvelope {
    fn from(proof: ExclusivityProof) -> Self {
        ProofEnvelope::Exclusivity(proof)
    }
}

impl From<CleanupProof> for ProofEnvelope {
    fn from(proof: CleanupProof) -> Self {
        ProofEnvelope::Cleanup(proof)
    }
}

impl From<OriginProof> for ProofEnvelope {
    fn from(proof: OriginProof) -> Self {
        ProofEnvelope::Origin(proof)
    }
}

impl From<InterpretationProof> for ProofEnvelope {
    fn from(proof: InterpretationProof) -> Self {
        ProofEnvelope::Interpretation(proof)
    }
}

impl From<Proof> for ProofEnvelope {
    fn from(proof: Proof) -> Self {
        ProofEnvelope::Generic(proof)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::{
        Conclusion, Fact, Goal, InvariantName, ProofContext, ProofStep, Target,
    };
    use crate::judgment::RegionId;
    use crate::liveness_proofs::LivenessProof;
    use crate::liveness_proofs::LivenessTactic;

    /// Helper: create a simple Proof for reuse in tests.
    fn make_simple_proof() -> Proof {
        let mut proof = Proof::new(Goal::new(
            InvariantName::Liveness,
            Target::Region(RegionId(1)),
            ProofContext::new("test::simple"),
        ));
        proof.add_step(ProofStep::Assume {
            fact: Fact::axiom(0, "region 1 is allocated"),
        });
        proof.add_step(ProofStep::Infer {
            from: vec![0],
            rule: crate::rules::InferenceRule::LivenessIntro,
            conclusion: Fact::derived(1, "region 1 is live"),
        });
        proof.conclude(Conclusion::Proven);
        proof
    }

    #[test]
    fn test_proof_roundtrip_json() {
        let proof = make_simple_proof();

        // Serialize
        let envelope = ProofEnvelope::Generic(proof.clone());
        let json = envelope.to_json_string().expect("serialization failed");

        // Deserialize
        let deserialized: ProofEnvelope =
            ProofEnvelope::from_json_string(&json).expect("deserialization failed");

        // Compare
        if let ProofEnvelope::Generic(roundtrip) = deserialized {
            assert_eq!(roundtrip, proof);
        } else {
            panic!("expected Generic envelope variant");
        }
    }

    #[test]
    fn test_envelope_liveness_roundtrip() {
        let proof = make_simple_proof();
        let liveness = LivenessProof {
            proof: proof.clone(),
            access_proofs: vec![(0, proof.clone())],
            freed_proofs: vec![],
            deadlock_proof: None,
            ordering: None,
            tactic: LivenessTactic::PathEnumeration,
        };

        let envelope = ProofEnvelope::from(liveness.clone());
        let json = envelope.to_json_string().expect("serialization failed");

        let deserialized: ProofEnvelope =
            ProofEnvelope::from_json_string(&json).expect("deserialization failed");

        if let ProofEnvelope::Liveness(roundtrip) = deserialized {
            assert_eq!(roundtrip, liveness);
        } else {
            panic!("expected Liveness envelope variant");
        }
    }

    #[test]
    fn test_envelope_serialization_pretty() {
        let proof = make_simple_proof();
        let envelope = ProofEnvelope::Generic(proof);

        let pretty = envelope.to_json_string_pretty().expect("pretty serialization failed");

        // Pretty output should contain newlines and the "type" tag
        assert!(pretty.contains('\n'), "pretty JSON should contain newlines");
        assert!(pretty.contains("\"type\""), "should contain type tag");
        assert!(pretty.contains("\"Generic\""), "should contain Generic variant name");
    }

    #[test]
    fn test_envelope_from_json_string() {
        let proof = make_simple_proof();
        let envelope = ProofEnvelope::Generic(proof.clone());
        let json = envelope.to_json_string().expect("serialization failed");

        // Verify the JSON string contains the expected tagged structure
        assert!(json.contains("\"type\""), "JSON should contain type tag");
        assert!(json.contains("\"Generic\""), "JSON should contain Generic tag value");

        // Parse back
        let parsed = ProofEnvelope::from_json_string(&json).expect("deserialization failed");
        if let ProofEnvelope::Generic(roundtrip) = parsed {
            assert_eq!(roundtrip, proof);
        } else {
            panic!("expected Generic envelope variant");
        }
    }
}
