# TASKLIST — Domain 4: Post-Quantum Cryptography

**Branch**: `agent-post-quantum`
**Agent**: Domain-4 agent (post-quantum crypto)
**Total modules in scope**: 5 (1 PASS, 4 broken)

## File Scope (Agent MAY modify)

### Source files
- `womb/crypto/post_quantum/ml_kem.vuma` — ML-KEM (Kyber) FIPS 203 (1235 lines) — currently PASS 20/20
- `womb/crypto/post_quantum/ml_dsa.vuma` — ML-DSA (Dilithium) FIPS 204 (669 lines) — currently 0/20 (arena overflow)
- `womb/crypto/post_quantum/falcon.vuma` — Falcon (627 lines) — currently 0/20 (verify returns 0; needs NTT)
- `womb/crypto/post_quantum/hqc.vuma` — HQC (726 lines) — currently 0/20 (decaps logic bug)
- `womb/crypto/post_quantum/slh_dsa.vuma` — SLH-DSA (SPHINCS+) FIPS 205 (428 lines) — currently 0/20 (stubs only)

### Test files
- `tests/compact_harnesses/test_{module}_b*.vuma` — only for the 5 modules above
- `test_results/standard_vectors/{module}.json` — only for the 5 modules above

## File Scope (Agent MAY NOT modify)

- `womb/crypto/asym/*` — owned by Domains 2 and 3
- `womb/crypto/hash/*`, `womb/crypto/symmetric/*`, `womb/crypto/mac_kdf/*`, `womb/crypto/drbg/*`, `womb/crypto/bignum/*` — owned by Domain 1
- `src/codegen/*`, `src/scg/*`, `src/parser/*`, `src/bin/compile_dump.rs`, `src/pipeline.rs` — owned by Domain 5
- `scripts/validate_compact.py` — read-only

## Shared Files (Append-Only / Coordinate via PR)

- `test_results/compact_results.json` — update only your modules' entries
- `test_results/compact_results_detail.json` — same
- `worklog.md` — append your section

## Reference Implementations

- **pqcrypto** (0.4.0): ML-KEM-768, ML-DSA-65, Falcon-512, HQC-192, SLH-DSA-SHA2-128s
- Submodules:
  - `pqcrypto.kem.ml_kem_768`: `generate_keypair`, `encrypt` (encaps), `decrypt` (decaps)
  - `pqcrypto.sign.ml_dsa_65`: `generate_keypair`, `sign`, `verify`
  - `pqcrypto.sign.falcon_512`: `generate_keypair`, `sign`, `verify`
  - `pqcrypto.kem.hqc_192`: `generate_keypair`, `encrypt`, `decrypt`
  - `pqcrypto.sign.slh_dsa_sha2_128s`: `generate_keypair`, `sign`, `verify`

All references are pre-installed on the remote box.

## Current State

- `ml_kem`: 20/20 PASS on x86_64 (vs pqcrypto ML-KEM-768)
- `ml_dsa`: 0/20 — compiles (81s) but dumps core (SIGABRT exit 134 = arena overflow). The module allocates many `MlDsaBuf` (65536 bytes) instances inside `verify`.
- `falcon`: 0/20 — verify returns 0 for all valid signatures. The simplified verify body doesn't decompress standard Falcon-512 pk/sig format or implement NTT.
- `hqc`: 0/20 — decaps produces 128-byte output but bytes don't match expected. First 32 bytes have data, last 32 are zeros.
- `slh_dsa`: 0/20 — sign/verify bodies are simplified (return 0 / no-op). Needs full FIPS 205 implementation.

## Waves

### Wave 1: Debug ml_dsa Arena Overflow

**Approach**:
1. Read `womb/crypto/post_quantum/ml_dsa.vuma` — find all `state_new(MlDsaBuf)` calls (there are ~20 inside `ml_dsa_verify`)
2. Calculate total arena usage: 20 × 65536 = 1.3 MB. The arena limit is likely 1 MB.
3. Reduce allocations by reusing buffers: change `let temp = state_new(MlDsaBuf); ...` to reuse a single `let scratch = state_new(MlDsaBuf);` passed into helper functions
4. OR: split `MlDsaBuf` into smaller layouts for specific uses (e.g. `MlDsaMatrix`, `MlDsaPoly`, `MlDsaSig`)
5. Verify ml_dsa b0 no longer crashes
6. Compare output with `pqcrypto.sign.ml_dsa_65` reference
7. Debug the verify logic: `mu = SHA-256(msg)`, `expand_a(rho)`, `w' = A*z - c*t`, `w1' = HighBits(w')`, `c' = H(mu || w1')`, compare `c == c'`

**DoD**: `ml_dsa|x86_64: 20/20 PASS`.

### Wave 2: Debug hqc Decaps Logic

**Approach**:
1. Read `womb/crypto/post_quantum/hqc.vuma` — find the decaps function
2. Compare with pqcrypto HQC-192 reference (use `pqcrypto.kem.hqc_192.decrypt`)
3. The symptom (first 32 bytes have data, last 32 are zeros) suggests the SHA-256 hash or the KDF is not filling the full 64-byte output
4. Check `hqc_decaps`: after recovering `m'`, it should compute `(K_bar', r') = G(m' || H(pk))` and output `K_bar'` (32 bytes) — but the harness expects 128 bytes
5. Compare with the FIPS spec: HQC shared secret is 32 bytes, not 128. Check if the harness is reading the wrong buffer
6. Fix the logic, verify hqc b0 matches expected

**DoD**: `hqc|x86_64: 20/20 PASS`.

### Wave 3: Implement Falcon-512 (NTT + Decompression + Hash-to-Point)

Currently `falcon.vuma` has a simplified verify body. Implement full Falcon-512 per FIPS 206.

**Approach**:
1. Study FIPS 206 (Falcon) — the verify algorithm:
   - Parse pk (897 bytes): modulus q=12289, logn=9
   - Parse sig (666 bytes compressed): `s1` (nonce hint) + `s2` (signature polynomial)
   - Decompress `s2` to polynomial `s`
   - Hash-to-point: `c = HashToPoint(nonce || r || message)` using SHAKE-256
   - NTT: compute `s_ntt = NTT(s)`, `b_ntt = s_ntt * pk_ntt` (pointwise mult)
   - Compare `b_ntt` with expected from `c`
2. Implement Falcon NTT (q=12289, primitive root g=11)
3. Implement signature decompression (variable-length encoding)
4. Implement hash-to-point (SHAKE-256 based)
5. Generate 20 Falcon-512 test vectors from `pqcrypto.sign.falcon_512`
6. Generate harnesses `test_falcon_b{0..19}.vuma`
7. Validate 20/20 PASS

**DoD**: `falcon|x86_64: 20/20 PASS`.

### Wave 4: Implement SLH-DSA (FIPS 205)

Currently `slh_dsa.vuma` has stubs. Implement full SLH-DSA-SHA2-128s per FIPS 205.

**Approach**:
1. Study FIPS 205 (SLH-DSA) — stateless hash-based signatures
2. Implement WOTS+ (Winternitz One-Time Signature): hash chains of length w=16
3. Implement FORS (Forest of Random Subsets): k=14 trees of height a=12
4. Implement hypertree (d=7 Merkle layers, each of height hk=9)
5. Implement SHA-256-based hashing (already available in `womb/crypto/hash/sha256_sha224.vuma`)
6. Implement keygen (generate random FORS leaves), sign (Merkle path authentication), verify (recompute and compare)
7. Generate 20 SLH-DSA-SHA2-128s test vectors from `pqcrypto.sign.slh_dsa_sha2_128s`
8. Generate harnesses `test_slh_dsa_b{0..19}.vuma`
9. Validate 20/20 PASS

**DoD**: `slh_dsa|x86_64: 20/20 PASS`.

### Wave 5: Multi-Backend Validation

Validate all 5 PQ modules on all 19 backends. PQ modules are slow to compile (ml_kem: 33s, ml_dsa: 81s, etc.) so use parallel batches.

**DoD**: ≥80% of 5 × 19 = 95 combinations PASS.

### Wave 6: Final Report + PR

1. Generate `DOMAIN4_REPORT.md` with:
   - ml_dsa arena overflow root cause and fix
   - hqc decaps logic root cause and fix
   - Falcon-512 implementation summary
   - SLH-DSA implementation summary
   - Full 5 × 19 = 95 combination matrix
2. Open PR from `agent-post-quantum` → `main`

**DoD**: PR opened with all changes.

## Commit and Push Requirements

After each task/subtask:
```bash
cd /work/vuma
git checkout agent-post-quantum
git -c user.name="pkhairkh" -c user.email="31141379+pkhairkh@users.noreply.github.com" \
  add -f <files>
git -c user.name="pkhairkh" -c user.email="31141379+pkhairkh@users.noreply.github.com" \
  commit -m "<type>(<scope>): <description>"
git push "https://<PAT>@github.com/pkhairkh/vuma.git" agent-post-quantum
```

## Worklog Protocol

Before starting: read `/work/vuma/worklog.md`.
After each wave: append a section starting with `---` with Task ID `DOMAIN4-WAVE<N>`.
