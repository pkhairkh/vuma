# Task: Add serde_json dependency and serialization I/O methods to the proof crate

## Summary of Changes

### 1. `src/proof/Cargo.toml` — Added `serde_json = "1"` dependency

### 2. `src/proof/src/serialization.rs` — New file with:
- **`SerializationError`** enum with `Serialization(serde_json::Error)` and `Io(std::io::Error)` variants
- **`ProofEnvelope`** tagged enum (`#[serde(tag = "type", content = "data")]`) with variants:
  - `Liveness(LivenessProof)`
  - `Exclusivity(ExclusivityProof)`
  - `Cleanup(CleanupProof)`
  - `Origin(OriginProof)`
  - `Interpretation(InterpretationProof)`
  - `Generic(Proof)`
- **Methods on `ProofEnvelope`:**
  - `to_json_string()` → `Result<String, SerializationError>`
  - `to_json_string_pretty()` → `Result<String, SerializationError>`
  - `from_json_string(&str)` → `Result<Self, SerializationError>`
  - `to_writer<W: Write>(&self, W)` → `Result<(), SerializationError>`
  - `from_reader<R: Read>(R)` → `Result<Self, SerializationError>`
- **`From<XXXProof> for ProofEnvelope`** impls for all 6 proof types
- **4 tests:**
  - `test_proof_roundtrip_json` — creates a Proof, serializes/deserializes, compares
  - `test_envelope_liveness_roundtrip` — LivenessProof roundtrip
  - `test_envelope_serialization_pretty` — verifies pretty JSON format
  - `test_envelope_from_json_string` — verifies tagged JSON structure and roundtrip

### 3. `src/proof/src/lib.rs` — Added `pub mod serialization;`

### 4. Serialize/Deserialize derives — All proof types already had them; no changes needed

## Test Results
```
running 4 tests
test serialization::tests::test_envelope_serialization_pretty ... ok
test serialization::tests::test_envelope_from_json_string ... ok
test serialization::tests::test_envelope_liveness_roundtrip ... ok
test serialization::tests::test_proof_roundtrip_json ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured
```
