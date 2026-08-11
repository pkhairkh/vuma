# VUMA Womb Crypto — Faithfulness Validation Report

Generated: 2026-08-11
Repository HEAD: (see git log)

## Executive Summary

- **x86_64 modules validated**: 31
- **Fully passing (≥20/20)**: 27
- **Total vectors pass**: 589/590 (99.8%)

## Critical Codegen Fixes Applied (This Session)

### 1. Type-Aware Unsigned Comparison (commit 39494381)
VUMA's <, <=, >, >= operators on u64 values were ALWAYS lowered to SIGNED
comparisons (CmpKind::SLt). This produced wrong results when either operand
had bit 63 set (values ≥ 2^63). Fixed by adding type-aware dispatch in
pipeline.rs: signed types → S{Lt,Le,Gt,Ge}, unsigned → U{Lt,Le,Gt,Ge}.
This fixed bignum carry propagation and benefits all u64-heavy modules.

### 2. x86_64 Stack Probes (commit 96d3f3f7)
When sub rsp, frame_size skips past the kernel's 4KB stack guard page,
subsequent writes to the new frame segfault. Fixed by emitting
mov byte ptr [rsp + offset], 0 for each 4KB page (stack probe).
This unblocks argon2 and any module with > 4KB stack frames.

### 3. blake2b_update Multi-Call Fix (commit 96d3f3f7)
When ctx.len == 128 at the start of a new update call, the code wrote
to ctx.buf[128] = OOB. Fixed by checking ctx.len == 128 BEFORE writing.

### 4. x25519 fe_sub + fe_to_bytes (commit fe09e499)
fe_sub ADDED 38 when borrow occurred (should SUBTRACT). fe_to_bytes
only cleared bit 255 (should do conditional subtract of p).

## Per-Module Results (x86_64)

| Module | Score | Status |
|--------|-------|--------|
| aes128 | 20/20 | ✅ PASS |
| aes192 | 20/20 | ✅ PASS |
| aes256 | 20/20 | ✅ PASS |
| aes_cfb_ofb | 12/12 | ⚠️ PASS |
| aes_extra_modes | 20/20 | ✅ PASS |
| aes_modes | 21/21 | ✅ PASS |
| bignum | 20/20 | ✅ PASS |
| bignum2048 | 20/20 | ✅ PASS |
| blake2 | 20/20 | ✅ PASS |
| blake3 | 20/20 | ✅ PASS |
| chacha20 | 20/20 | ✅ PASS |
| chacha20_poly1305 | 5/5 | ⚠️ PASS |
| cmac_bcrypt_kdf | 20/20 | ✅ PASS |
| des | 20/20 | ✅ PASS |
| des_rc4_aria_camellia | 14/15 | ⚠️ PARTIAL |
| drbg | 20/20 | ✅ PASS |
| hkdf | 20/20 | ✅ PASS |
| hmac | 20/20 | ✅ PASS |
| key_agreement | 20/20 | ✅ PASS |
| md5 | 20/20 | ✅ PASS |
| pbkdf2 | 17/17 | ⚠️ PASS |
| poly1305 | 20/20 | ✅ PASS |
| rc4 | 20/20 | ✅ PASS |
| salsa20 | 20/20 | ✅ PASS |
| scrypt | 20/20 | ✅ PASS |
| sha1 | 20/20 | ✅ PASS |
| sha256_sha224 | 20/20 | ✅ PASS |
| sha3 | 20/20 | ✅ PASS |
| sha384 | 20/20 | ✅ PASS |
| sha512 | 20/20 | ✅ PASS |
| x25519 | 20/20 | ✅ PASS |

## Modules NOT Yet Validated

| Module | Status | Notes |
|--------|--------|-------|
| argon2 | ❌ NOT VALIDATED | |
| drbg_extra | ❌ NOT VALIDATED | |
| ed25519 | ❌ NOT VALIDATED | |
| ecdsa_p256 | ❌ NOT VALIDATED | |
| ecdsa_p384 | ❌ NOT VALIDATED | |
| ecdh_p256 | ❌ NOT VALIDATED | |
| secp256k1 | ❌ NOT VALIDATED | |
| rsa | ❌ NOT VALIDATED | |
| rsa_oaep_pss | ❌ NOT VALIDATED | |
| rsa_pkcs1_ecdsa_extra | ❌ NOT VALIDATED | |
| ml_kem | ❌ NOT VALIDATED | |
| ml_dsa | ❌ NOT VALIDATED | |
| slh_dsa | ❌ NOT VALIDATED | |
| falcon | ❌ NOT VALIDATED | |
| hqc | ❌ NOT VALIDATED | |
