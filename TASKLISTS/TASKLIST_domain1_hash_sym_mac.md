# TASKLIST — Domain 1: Hash + Symmetric + MAC/KDF + DRBG + Bignum

**Branch**: `agent-hash-sym-mac`
**Agent**: Domain-1 agent (hash + symmetric + MAC/KDF + DRBG + bignum)
**Total modules in scope**: 30 (all currently PASS 20/20 on x86_64)

## File Scope (Agent MAY modify)

### Source files
- `womb/crypto/hash/*.vuma` — sha1, sha256_sha224, sha384, sha512, md5, sha3, blake2, blake3 (8 modules)
- `womb/crypto/symmetric/*.vuma` — aes128, aes192, aes256, rc4, chacha20, salsa20, des, poly1305, aes_modes, aes_cfb_ofb, aes_extra_modes, chacha20_poly1305, des_rc4_aria_camellia (13 modules)
- `womb/crypto/mac_kdf/*.vuma` — hmac, hkdf, pbkdf2, scrypt, cmac_bcrypt_kdf, key_agreement, argon2 (7 modules)
- `womb/crypto/drbg/*.vuma` — drbg, drbg_extra (2 modules)
- `womb/crypto/bignum/*.vuma` — bignum, bignum2048 (2 modules)

### Test files
- `tests/compact_harnesses/test_{module}_b*.vuma` — only for modules listed above
- `test_results/standard_vectors/{module}.json` — only for modules listed above

## File Scope (Agent MAY NOT modify)

- `womb/crypto/asym/*` — owned by Domains 2 and 3
- `womb/crypto/post_quantum/*` — owned by Domain 4
- `src/codegen/*`, `src/scg/*`, `src/parser/*`, `src/bin/compile_dump.rs`, `src/pipeline.rs` — owned by Domain 5
- `scripts/validate_compact.py` — read-only (use as-is)

## Shared Files (Append-Only / Coordinate via PR)

- `test_results/compact_results.json` — update only your modules' entries; do not delete others
- `test_results/compact_results_detail.json` — same
- `worklog.md` — append your section with `---` separator

## Reference Implementations

- **hashlib** (built-in): sha1, sha256, sha224, sha384, sha512, md5, sha3_256, sha3_512, blake2b, blake2s, hmac, pbkdf2_hmac, scrypt
- **blake3** (pip 1.0.9): blake3
- **pycryptodome** (3.23.0): AES, RC4, ChaCha20, Salsa20, DES, Poly1305, CMAC
- **argon2-cffi** (25.1.0): Argon2id

All references are pre-installed on the remote box.

## Current State

All 30 modules in this domain PASS 20/20 on x86_64. See `/home/z/my-project/download/FINAL_REPORT.md` for the full validation matrix.

## Waves

### Wave 1: Multi-Backend Validation — Batch 1 (Easy Backends)

**Backends**: `aarch64`, `aarch64_be`, `x86_32` (3 backends × 30 modules = 90 combinations)

For each module × backend:
1. Run `python3 scripts/validate_compact.py <module> <backend>` (resumable)
2. Record pass/fail per harness in `test_results/compact_results_detail.json`
3. If a harness fails, debug the VUMA implementation (NOT the test vector — references are confirmed correct)
4. Commit + push after each module completes

**DoD**: All 30 modules PASS 20/20 on `aarch64`, `aarch64_be`, `x86_32`. Any failures are fixed in the VUMA source.

### Wave 2: Multi-Backend Validation — Batch 2 (32-bit Backends)

**Backends**: `arm32`, `armeb`, `hppa`, `m68k`, `sparc64` (5 backends × 30 modules = 150 combinations)

These backends have known issues with u64 operations (SHA-384/512, MD5 use u64). If a module fails:
1. Identify whether the failure is a codegen bug (Domain 5's responsibility — file an issue in worklog) or a module bug (your responsibility)
2. If it's a module bug (e.g. u64 literal ≥ 2^63 — though the parser was fixed, module code may still use `~0` workarounds that need cleanup), fix it
3. If it's a codegen bug, document in worklog and skip — Domain 5 will fix the codegen, then you re-validate

**DoD**: All 30 modules PASS 20/20 on `arm32`, `armeb`, `hppa`, `m68k`, `sparc64` OR codegen bugs are documented for Domain 5.

### Wave 3: Multi-Backend Validation — Batch 3 (Remaining Backends)

**Backends**: `riscv64`, `riscv32`, `s390x`, `ppc64`, `ppc64le`, `loongarch64`, `alpha`, `mips64`, `mips64be`, `wasm32` (10 backends × 30 modules = 300 combinations)

Same workflow as Wave 2. `riscv32` (segfault) and `wasm32` (print_int issue) have known problems — document for Domain 5.

**DoD**: All 30 modules PASS 20/20 on all 10 remaining backends OR known codegen bugs are documented for Domain 5.

### Wave 4: Hardening + Final Report

1. Remove any remaining `~0 as u64` or `((1 as u64) << N)` workarounds in your modules (the parser was fixed — direct u64 literals now work)
2. Verify all 30 modules still PASS 20/20 on x86_64 after cleanup
3. Generate `DOMAIN1_REPORT.md` with the complete 30 × 19 = 570 combination matrix
4. Open PR from `agent-hash-sym-mac` → `main`

**DoD**: 
- All `~0`/`<<N` workarounds removed from hash/symmetric/mac_kdf/drbg/bignum modules
- 30 modules × 19 backends = 570 combinations documented
- PR opened with all changes

## Commit and Push Requirements

After each task/subtask:
```bash
cd /work/vuma
git checkout agent-hash-sym-mac
git -c user.name="pkhairkh" -c user.email="31141379+pkhairkh@users.noreply.github.com" \
  add -f <files>
git -c user.name="pkhairkh" -c user.email="31141379+pkhairkh@users.noreply.github.com" \
  commit -m "<type>(<scope>): <description>"
git push "https://<PAT>@github.com/pkhairkh/vuma.git" agent-hash-sym-mac
```

Commit types: `fix`, `test`, `perf`, `refactor`, `docs`.

## Worklog Protocol

Before starting: read `/work/vuma/worklog.md`.
After each wave: append a section starting with `---`:
```
---
Task ID: DOMAIN1-WAVE<N>
Agent: agent-hash-sym-mac
Task: <wave description>
Work Log:
- <steps>
Stage Summary:
- <results>
```
