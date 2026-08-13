# TASKLIST — Domain 2: Classic ECC (P-256, secp256k1, Ed25519, X25519)

**Branch**: `agent-classic-ecc`
**Agent**: Domain-2 agent (classic asymmetric ECC)
**Total modules in scope**: 5 (all currently PASS 20/20 on x86_64)

## File Scope (Agent MAY modify)

### Source files
- `womb/crypto/asym/ecdh_p256.vuma` — ECDH on P-256 (218 lines)
- `womb/crypto/asym/ecdsa_p256.vuma` — ECDSA sign/verify on P-256 (1286 lines)
- `womb/crypto/asym/ed25519.vuma` — Ed25519 sign/verify (782 lines)
- `womb/crypto/asym/x25519.vuma` — X25519 key agreement (669 lines)
- `womb/crypto/asym/secp256k1.vuma` — ECDSA on secp256k1 (594 lines)

### Test files
- `tests/compact_harnesses/test_{module}_b*.vuma` — only for the 5 modules above
- `test_results/standard_vectors/{module}.json` — only for the 5 modules above

## File Scope (Agent MAY NOT modify)

- `womb/crypto/asym/ecdsa_p384.vuma` — owned by Domain 3
- `womb/crypto/asym/rsa*.vuma` — owned by Domain 3
- `womb/crypto/hash/*`, `womb/crypto/symmetric/*`, `womb/crypto/mac_kdf/*`, `womb/crypto/drbg/*`, `womb/crypto/bignum/*` — owned by Domain 1
- `womb/crypto/post_quantum/*` — owned by Domain 4
- `src/codegen/*`, `src/scg/*`, `src/parser/*`, `src/bin/compile_dump.rs`, `src/pipeline.rs` — owned by Domain 5
- `scripts/validate_compact.py` — read-only

## Shared Files (Append-Only / Coordinate via PR)

- `test_results/compact_results.json` — update only your modules' entries
- `test_results/compact_results_detail.json` — same
- `worklog.md` — append your section

## Reference Implementations

- **cryptography** (50.0.0): ECDH P-256, ECDSA P-256, Ed25519, X25519, secp256k1
- Uses `cryptography.hazmat.primitives.asymmetric.ec` for ECDH/ECDSA
- Uses `cryptography.hazmat.primitives.asymmetric.ed25519` for Ed25519
- Uses `cryptography.hazmat.primitives.asymmetric.x25519` for X25519

All references are pre-installed on the remote box.

## Current State

All 5 modules in this domain PASS 20/20 on x86_64:
- `ecdh_p256`: 20/20 (fixed in prior session — pubkey byte truncation)
- `ecdsa_p256`: 20/20 (fixed in prior session — SCG inliner + aliasing + vectors)
- `ed25519`: 20/20
- `x25519`: 20/20
- `secp256k1`: 20/20 (fixed in prior session — wrong Gx/Gy/n constants)

## Waves

### Wave 1: Multi-Backend Validation — Batch 1 (Easy Backends)

**Backends**: `aarch64`, `aarch64_be`, `x86_32` (3 backends × 5 modules = 15 combinations)

For each module × backend:
1. Run `python3 scripts/validate_compact.py <module> <backend>`
2. If a harness fails, debug the VUMA implementation
3. Commit + push after each module completes

**DoD**: All 5 modules PASS 20/20 on `aarch64`, `aarch64_be`, `x86_32`.

### Wave 2: Multi-Backend Validation — Batch 2 (32-bit Backends)

**Backends**: `arm32`, `armeb`, `hppa`, `m68k`, `sparc64` (5 backends × 5 modules = 25 combinations)

These backends have known issues with u64 operations. ECC modules use u64 for field arithmetic. If a module fails:
1. Determine if it's a codegen bug (document for Domain 5) or a module bug
2. If module bug (e.g. u64 literal issue), fix it
3. Re-validate

**DoD**: All 5 modules PASS 20/20 on all 5 32-bit backends OR codegen bugs documented.

### Wave 3: Multi-Backend Validation — Batch 3 (Remaining Backends)

**Backends**: `riscv64`, `riscv32`, `s390x`, `ppc64`, `ppc64le`, `loongarch64`, `alpha`, `mips64`, `mips64be`, `wasm32` (10 backends × 5 modules = 50 combinations)

**DoD**: All 5 modules PASS 20/20 on all 10 remaining backends OR codegen bugs documented.

### Wave 4: Hardening + Final Report

1. Audit all point_add/point_double functions for aliasing bugs (same pattern as the ecdsa_p256 fix in commit `987fd6be` — local temporaries for output)
2. Verify all 5 modules still PASS 20/20 on x86_64
3. Generate `DOMAIN2_REPORT.md` with the complete 5 × 19 = 95 combination matrix
4. Open PR from `agent-classic-ecc` → `main`

**DoD**:
- All point operations audited for aliasing safety
- 5 modules × 19 backends = 95 combinations documented
- PR opened

## Commit and Push Requirements

After each task/subtask:
```bash
cd /work/vuma
git checkout agent-classic-ecc
git -c user.name="pkhairkh" -c user.email="31141379+pkhairkh@users.noreply.github.com" \
  add -f <files>
git -c user.name="pkhairkh" -c user.email="31141379+pkhairkh@users.noreply.github.com" \
  commit -m "<type>(<scope>): <description>"
git push "https://<PAT>@github.com/pkhairkh/vuma.git" agent-classic-ecc
```

## Worklog Protocol

Before starting: read `/work/vuma/worklog.md`.
After each wave: append a section starting with `---` with Task ID `DOMAIN2-WAVE<N>`.
