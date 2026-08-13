# TASKLIST — Domain 3: RSA + P-384 + Ed448/P-521

**Branch**: `agent-rsa-p384-ed448`
**Agent**: Domain-3 agent (RSA + ECC P-384 + Ed448/P-521)
**Total modules in scope**: 4 (2 PASS, 2 broken)

## File Scope (Agent MAY modify)

### Source files
- `womb/crypto/asym/rsa.vuma` — RSA sign/verify/encrypt/decrypt (837 lines) — currently PASS 20/20
- `womb/crypto/asym/rsa_oaep_pss.vuma` — RSA-OAEP/PSS (932 lines) — currently PASS 20/20
- `womb/crypto/asym/rsa_pkcs1_ecdsa_extra.vuma` — Ed448/P-521 stubs (573 lines) — currently N/A (no harnesses)
- `womb/crypto/asym/ecdsa_p384.vuma` — ECDSA on P-384 (892 lines) — currently 0/20 (logic bug)

### Test files
- `tests/compact_harnesses/test_{module}_b*.vuma` — only for the 4 modules above
- `test_results/standard_vectors/{module}.json` — only for the 4 modules above

## File Scope (Agent MAY NOT modify)

- `womb/crypto/asym/{ecdh_p256,ecdsa_p256,ed25519,x25519,secp256k1}.vuma` — owned by Domain 2
- `womb/crypto/hash/*`, `womb/crypto/symmetric/*`, `womb/crypto/mac_kdf/*`, `womb/crypto/drbg/*`, `womb/crypto/bignum/*` — owned by Domain 1
- `womb/crypto/post_quantum/*` — owned by Domain 4
- `src/codegen/*`, `src/scg/*`, `src/parser/*`, `src/bin/compile_dump.rs`, `src/pipeline.rs` — owned by Domain 5
- `scripts/validate_compact.py` — read-only

## Shared Files (Append-Only / Coordinate via PR)

- `test_results/compact_results.json` — update only your modules' entries
- `test_results/compact_results_detail.json` — same
- `worklog.md` — append your section

## Reference Implementations

- **pycryptodome** (3.23.0): RSA (PKCS#1 v1.5, OAEP, PSS)
- **cryptography** (50.0.0): ECDSA P-384, Ed448 (RFC 8032), P-521 ECDSA
- References for Ed448: RFC 8032 test vectors
- References for P-521: NIST CAVP test vectors

All references are pre-installed on the remote box.

## Current State

- `rsa`: 20/20 PASS (vs pycryptodome)
- `rsa_oaep_pss`: 20/20 PASS (vs pycryptodome)
- `rsa_pkcs1_ecdsa_extra`: N/A — no harnesses exist; module contains stubs (Ed448/P-521 return 0)
- `ecdsa_p384`: 0/20 — runtime now fast (23.7s, was timing out) but output is WRONG. Aliasing fix applied (commit `47dddde5`) but deeper arithmetic bug remains in bn384_mod_mul or bn384_mod_inv_fermat.

## Waves

### Wave 1: Debug ecdsa_p384 Arithmetic Bug

The aliasing fix (commit `47dddde5`) was applied but the output is still wrong. The bug is in the bn384 arithmetic.

**Approach**:
1. Read `womb/crypto/asym/ecdsa_p384.vuma` lines 162-260 (bn384_mod_add, bn384_mod_sub, bn384_mod_mul, bn384_mod_exp, bn384_mod_inv_fermat)
2. Create a minimal test harness that exercises bn384_mod_mul with known inputs and compare with Python `pow(a*b, 1, p384_p)`
3. Create a test for bn384_mod_inv_fermat: `bn384_mod_inv_fermat(3, p384_p)` should equal `pow(3, p384_p-2, p384_p)`
4. If bn384_mod_mul is wrong, audit the schoolbook multiplication loop (lines 176-189) for carry propagation bugs
5. If bn384_mod_inv_fermat is wrong, audit the Montgomery setup (bn384_mont_mu, bn384_mont_r2, bn384_mont_mul)
6. Compare with the working bn256 implementation in `womb/crypto/bignum/bignum.vuma` (lines 437-549) — bn256 works, bn384 doesn't, so the bug is in the bn384-specific code
7. Fix the bug, verify ecdsa_p384 b0 produces correct r||s

**DoD**: `ecdsa_p384|x86_64: 20/20 PASS` (all 20 vectors match expected r||s).

### Wave 2: Implement Ed448 (RFC 8032)

Currently `rsa_pkcs1_ecdsa_extra.vuma` has Ed448 stubs (return 0). Implement full Ed448.

**Approach**:
1. Study RFC 8032 (EdDSA) — Ed448 uses the golden ratio prime p = 2^448 - 2^224 - 1
2. Implement field arithmetic: fe448_add, fe448_sub, fe448_mul, fe448_sq, fe448_inv
3. Implement point operations: point_add, point_double, scalar_mul (using Edwards curve coordinates)
4. Implement hash-to-point: SHAKE256(msg, 114) → point
5. Implement keygen, sign, verify
6. Generate 10 Ed448 test vectors from RFC 8032 Appendix A
7. Generate harnesses `test_rsa_pkcs1_ecdsa_extra_ed448_b{0..9}.vuma`
8. Validate 10/10 PASS

**DoD**: 10 Ed448 vectors PASS on x86_64.

### Wave 3: Implement P-521 ECDSA

Currently `rsa_pkcs1_ecdsa_extra.vuma` has P-521 stubs (return 0). Implement full P-521 ECDSA.

**Approach**:
1. Implement bn521 field arithmetic (p = 2^521 - 1, a Mersenne prime — simplifies reduction)
2. Implement P-521 curve operations (a = -3, b = 0x0051953eb9618e867a09f550fb875b9d6eedf5b7d3f8e3c3a4a5a5a5a5a5a5a5a)
3. Implement ECDSA sign/verify with SHA-512 hashing
4. Generate 10 P-521 test vectors from NIST CAVP or Python cryptography
5. Generate harnesses `test_rsa_pkcs1_ecdsa_extra_p521_b{0..9}.vuma`
6. Validate 10/10 PASS

**DoD**: 10 P-521 vectors PASS on x86_64.

### Wave 4: Multi-Backend Validation

Validate all 4 modules on all 19 backends:
- `rsa`: 20 vectors × 19 backends = 380 combinations
- `rsa_oaep_pss`: 20 × 19 = 380
- `rsa_pkcs1_ecdsa_extra` (Ed448 + P-521): 20 × 19 = 380
- `ecdsa_p384`: 20 × 19 = 380

Total: 1520 combinations.

**DoD**: ≥95% (≥1444/1520) combinations PASS.

### Wave 5: Final Report + PR

1. Generate `DOMAIN3_REPORT.md` with:
   - ecdsa_p384 root cause analysis and fix
   - Ed448 implementation summary
   - P-521 implementation summary
   - Full 4 × 19 = 76 module × backend matrix
2. Open PR from `agent-rsa-p384-ed448` → `main`

**DoD**: PR opened with all changes.

## Commit and Push Requirements

After each task/subtask:
```bash
cd /work/vuma
git checkout agent-rsa-p384-ed448
git -c user.name="pkhairkh" -c user.email="31141379+pkhairkh@users.noreply.github.com" \
  add -f <files>
git -c user.name="pkhairkh" -c user.email="31141379+pkhairkh@users.noreply.github.com" \
  commit -m "<type>(<scope>): <description>"
git push "https://<PAT>@github.com/pkhairkh/vuma.git" agent-rsa-p384-ed448
```

## Worklog Protocol

Before starting: read `/work/vuma/worklog.md`.
After each wave: append a section starting with `---` with Task ID `DOMAIN3-WAVE<N>`.
