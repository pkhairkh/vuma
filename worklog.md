# VUMA Womb Crypto — Shared Multi-Agent Worklog

This file is the SINGLE shared worklog for all agents working on VUMA validation.
- Before starting work: read this file.
- After finishing a task: APPEND a new section starting with `---`.

## Environment

- Remote box: `root@34.147.164.40` (password auth)
- Repo: `/work/vuma` (also symlinked at `/home/z/my-project/vuma` on the box)
- HEAD as of session start: `25445c52` — fix(ml_dsa,hqc): enlarge buffers + sub-agent progress
- Helper: `/home/z/my-project/scripts/ssh_helper.py` (paramiko SSH helper)
- Local python: `/home/z/.venv/bin/python3` (has paramiko installed)
- 32 cores, 121 GiB RAM, 375 GB NVMe
- 19 backends: x86_64, x86_32, aarch64, aarch64_be, arm32, armeb, riscv64, riscv32, mips64, mips64be, ppc64, ppc64le, loongarch64, s390x, sparc64, alpha, hppa, m68k, wasm32

## State Summary (as of HEAD 25445c52)

- 408 combinations tested in `compact_results.json`:
  - 350 PASS
  - 57 PARTIAL
  - 1 FAIL
- 691 harness files in `tests/compact_harnesses/`
- 9 modules NOT yet validated on x86_64 (all have vectors):
  - ecdsa_p256, ecdsa_p384, secp256k1 — BLOCKED on Wave 0B (bignum carry bug) + Wave 0C (vector regen)
  - ml_kem — wrong shared secret output (Wave B)
  - ml_dsa — PARTIAL (wrong verify result, no crash) (Wave E)
  - falcon — SIGABRT (arena overflow) (Wave C)
  - hqc — EXC (buffer reuse incomplete) (Wave D)
  - slh_dsa — simplified bodies (Wave F)
  - rsa_pkcs1_ecdsa_extra — Ed448/P-521 stubs (Waves G, H)

## Key Discoveries (from prior session)

1. **VUMA literal parser truncates u64 values ≥ 2^63**: Use `~0 as u64`, `((1 as u64) << N)`, or byte arrays.
2. **rfc6979 nonce bug FIXED** (commit 520c3129): VUMA k now matches Python reference.
3. **bignum carry bug in bn256_mod_mul** (Wave 0B): 2*G.x differs by 0x64<<48 (limb[1] high byte) — likely carry propagation issue in bn256_mul_512.
4. **Test vectors for ecdsa_p256 are WRONG** (Wave 0C): Must regenerate from Python `cryptography` library.
5. **ml_dsa buffer fix works** (commit 25445c52): No more SIGABRT; now PARTIAL.

## CRITICAL: bn256_mul_512 carry propagation bug analysis (read before Wave 0B)

Looking at `womb/crypto/bignum/bignum.vuma` lines 251-309:

The schoolbook 4x4 multiplication has a carry-propagation bug at the highest limb. When processing (ai=3, bj=3) — the last iteration — `idx = 6`. The algorithm:
1. Adds `lo` at `r[6]` (with carry-out detected)
2. Adds `hi` at `r[7] = idx+1` (with carry from step 1)
3. If `r[7]` overflows, the "rare further carry propagation" loop tries to access `r[idx+2] = r[8]` which is OUT OF BOUNDS (Bn512Ctx has only 8 limbs, indices 0-7).

For 4x4 multiplication of large values, `r[7]` CAN overflow during intermediate steps because it receives the `hi(3,3)` contribution (up to 2^64-1) PLUS carries from `r[6]` (up to ~3 from the 3 direct contributions to r[6]). So `r[7]` can reach `2^64 + 2`, requiring a carry to `r[8]` which doesn't exist.

The "lost carry" corrupts the result. For 2*G.x, this manifests as a difference of `0x64 << 48` (100 in the high byte of limb[1] of the 256-bit result), suggesting that the lost carry propagates back through the modular reduction in a way that produces a small but non-zero error.

## Wave Plan (priority order)

1. **Wave 0B** (bignum carry bug) — CRITICAL blocker for all ECC
2. **Wave 0C** (vector regen) — independent
3. **Wave B** (ml_kem wrong output) — independent
4. **Wave C** (falcon SIGABRT) — independent
5. **Wave D** (hqc refactor) — independent
6. **Wave E** (ml_dsa verify logic) — independent
7. **Wave A** (ECC validate on x86_64) — depends on 0B + 0C
8. **Waves F, G, H** — slh_dsa, Ed448, P-521 (lower priority)
9. **Waves I, J, K** — backend fixes (after core modules pass on x86_64)
10. **Wave L** — full 46x19 expansion
11. **Wave M** — final report

## Commit / Push Protocol

After each task:
```bash
cd /work/vuma
git add -f <files>
git commit -m "<type>(<scope>): <description>"
git push "<REMOTE_URL>" main  # URL redacted
```

If push fails due to non-fast-forward: `git pull --rebase origin main` then push again.


---
Task ID: 0C
Agent: general-purpose (Wave 0C ECC vector regen)
Task: Regenerate ecdsa_p256, ecdsa_p384, secp256k1 vectors from cryptography lib

Work Log:
- Read shared worklog and confirmed context: existing ECC vectors had wrong expected_hex values; must regenerate from Python cryptography lib (RFC 6979 deterministic signatures).
- Inspected existing remote vector files to confirm JSON field schema (desc/priv_hex/pub_x_hex/pub_y_hex/msg_hex/r_hex/s_hex/expected_hex). For P-256/secp256k1, r/s/expected are 64/64/128 hex chars; for P-384 they are 96/96/192 hex chars.
- Wrote local generator at /home/z/my-project/scripts/gen_ecc_vectors.py using the cryptography lib (v44.0.3) with derive_private_key + ECDSA(SHA256/SHA384) for deterministic RFC 6979 signatures.
- Privkey scheme per spec: sha256(b"vuma-<module>-priv-" + bytes([i])) for P-256/secp256k1 (32 bytes); sha512(...)[:48] for P-384 (48 bytes). All clamped to valid range [1, n-1] via derive_priv_int (defensive increment-and-retry; never triggered in practice).
- Message scheme: msg_i = b"ecdsa test " + str(i).encode() (matches existing convention; e.g. msg 0 -> hex 656364736120746573742030).
- Generated 20 vectors per module locally; self-checked field lengths (priv 64/96, expected 128/192 hex chars) and ran round-trip verification by reconstructing public key + DER signature and calling pub.verify() — all 60 vectors PASS.
- Uploaded the 3 JSON files to /work/vuma/test_results/standard_vectors/ on the remote via paramiko sftp.
- Verified on remote via python3: each file is valid JSON with 20 vectors, expected_hex == r_hex + s_hex, and field lengths are correct.
- Staged only the 3 vector JSON files (did NOT touch compact_results_detail.json or untracked .vuma harness files left by other agents).
- Committed as c56ddbd1: "test(vectors): regenerate ECC vectors from cryptography lib (Wave 0C)".
- Pushed to main: 25445c52..c56ddbd1 (clean fast-forward, no rebase needed).

Stage Summary:
- 3 vector files regenerated and pushed: test_results/standard_vectors/{ecdsa_p256,ecdsa_p384,secp256k1}.json
- 20 deterministic RFC 6979 vectors per module, all cryptographically verified against cryptography lib.
- Generator script saved at /home/z/my-project/scripts/gen_ecc_vectors.py (local only) for reproducibility.
- HEAD is now c56ddbd1 on main. Wave 0C is complete; Wave A (x86_64 ECC validation) can proceed once Wave 0B (bignum carry bug) is fixed.
- No VUMA source (.vuma) files modified.
---
Task ID: C
Agent: general-purpose (Wave C falcon SIGABRT fix)
Task: Fix falcon_verify arena overflow (SIGABRT exit 134)

Work Log:
- Pulled latest main (HEAD c56ddbd1 from Wave 0C).
- Read falcon.vuma (600 lines) and traced falcon_verify call graph. Counted
  live state_new instances: only 6 in falcon_verify (4 FalconBuf + 2 Keccak)
  — well within any arena limit.  The "arena overflow" hypothesis was wrong.
- Compiled test_falcon_b0.vuma and ran it: exit 134 (OOB trap, NOT arena
  overflow which is exit 1 per src/codegen/src/x86_64/mod.rs:3869).
- Root-caused the OOB: falcon_shake256() called shake256_squeeze(ctx, kout,
  dst_len) with dst_len up to 4096 (FALCON_N*8 in poly_sample_gaussian) or
  512 (FALCON_N in hash_to_poly).  shake256_squeeze → keccak_squeeze writes
  dst_len bytes to out.data[out_idx] where out is State<KeccakOut> (200-byte
  data field).  Any dst_len > 200 triggers the __oob_trap (exit 134).
- Fixed falcon_shake256: rewrote to squeeze directly into the dst FalconBuf
  (32 KiB) in 136-byte (SHAKE256 rate) chunks.  Calls keccak_pad(ctx) once
  to finalize absorbing, then keccak_f1600(ctx) between chunks.  Updated
  imports to pull keccak_pad + keccak_f1600 directly from sha3.vuma (dropped
  shake256_squeeze).  Verified keccak_pad/f1600 are importable via a minimal
  test harness (prints 42, exit 0).
- After OOB fix, re-ran: exit 132 (SIGILL).  Diagnosed via debug prints:
  crash occurs in poly_norm_at which calls sqrt().  sqrt is NOT a VUMA
  builtin and was never imported from womb/lib/sys/math.vuma (which has
  sqrt_f64).  The unresolved extern resolves to __ffi_fallback_stub = ud2
  → SIGILL.  This was previously masked by the earlier OOB.
- Fixed the sqrt SIGILL: falcon_verify now uses falcon_compute_norm (returns
  ||sig||_2^2 as u32, no sqrt) and compares against FALCON_SIG_L2_BOUND_SQ
  (25000000).  poly_norm_at itself returns the squared norm (no sqrt).
- After both fixes: falcon_b0 runs with exit 0, output "1000\n999" (verify
  returned 0).  No more crash.
- Ran validate_compact.py --clear falcon x86_64, then validate_compact.py
  falcon x86_64: all 20 harnesses compile and run without crash.  Status =
  PARTIAL (0/20 pass).  Every harness outputs "00" (verify=0) but expected
  is "01" (verify=1).
- Root cause of remaining PARTIAL: the falcon_verify body is a simplified
  placeholder that treats pk/sig as raw u32 polynomial arrays (512 coefs =
  2048 bytes).  But the test vectors use the standard Falcon-512 wire format:
  pk = 897 bytes (1-byte header + 896 bytes of 14-bit packed NTT-domain h
  coefficients), sig = ~655 bytes (variable-length compressed s polynomial
  with unary+binary encoding).  The code also uses CBD-based hash_to_poly
  instead of the Falcon hash-to-point (SHAKE-256 with 14-bit rejection
  sampling in NTT domain).  Correct verification requires: (1) pk
  decompression (14-bit unpack + inverse NTT), (2) sig decompression
  (variable-length decode), (3) Falcon hash-to-point, (4) NTT-based
  multiply h*s or schoolbook mul after inverse-NTT of h.  This is a major
  implementation effort (NTT with q=12289, n=512, specific twiddle factors)
  — left as a future task (Wave C2).
- Committed as e066d91f: "fix(falcon): fix SIGABRT/OOB in keccak_squeeze +
  missing sqrt import (Wave C)".  Pushed to main (c56ddbd1..e066d91f).

Stage Summary:
- SIGABRT/OOB (exit 134) FIXED: falcon_shake256 no longer overflows KeccakOut.
- SIGILL (exit 132) FIXED: sqrt import replaced with squared-norm comparison.
- falcon_verify no longer crashes; all 20 harnesses run to completion (exit 0).
- Status: PARTIAL 0/20 — verify returns 0 instead of 1 because the simplified
  body doesn't decompress standard Falcon-512 pk/sig format or implement NTT.
- state_new count in falcon_verify: 6 live (4 FalconBuf + 2 Keccak) — no
  arena overflow; the original "arena overflow" diagnosis was incorrect.
- Definition of Done: "documented known-broken status with root cause analysis"
  (the OR branch).  Root cause: simplified verify body needs full Falcon-512
  spec (NTT + decompression + hash-to-point) — future Wave C2 task.
- Files changed: womb/crypto/post_quantum/falcon.vuma only (within scope).

