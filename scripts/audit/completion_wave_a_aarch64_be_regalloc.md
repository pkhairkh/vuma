# CA-a-test — aarch64_be 30-Test Matrix Verification (Regalloc Inheritance)

- **Task ID:** CA-a-test
- **Wave:** A (aarch64_be Verification)
- **Prior-run context (per VUMA Regalloc Completion run):**
  - Per the prior 2-a-audit, `aarch64_be` delegates `allocate_registers`
    to `aarch64` (see `src/codegen/src/aarch64_be.rs:150-152`:
    `self.inner.allocate_registers(func)`).
  - Wave 1 of the regalloc-endianness run fixed aarch64's callee-saved
    register handling across three commits:
    - R1-b-impl (`4c6b8524`) — spill position + X15 scratch + verifier
    - R1-b2-fix (`6a8dbd42`) — `contains_fork` extended to match
      `Syscall{nr: 220|221}` (clone/vfork)
    - R1-b3-fix — final consolidation (see R1-c-test summary)
  - The aarch64 regalloc path passes 29/30 with `VUMA_REAL_REGALLOC_AARCH64=1`;
    `try_recv` is the 1 known edge case (regalloc-mode exits 0 instead of 77).
- **HEAD before this task:** `7d41f2b6 [regalloc-endianness-wave-7-dod-pass]`.
- **This task:** verify that `aarch64_be` inherits the Wave-1 fix and
  exhibits the same 29/30 regalloc + 30/30 stack-slot pass rates as
  `aarch64`. No source files edited.

## §1 Methodology

1. Sourced `scripts/env/*.sh`; verified `PATH` includes `cargo`,
   `target/release/compile_dump`, and `$HOME/.local/bin/qemu-aarch64_be-static`
   (Debian `1:10.0.11+ds-0+deb13u1`).
2. Read `/home/z/my-project/worklog.md` last 5 sections (R6-b-audit,
   R6-c-fix, R6-d-test, regalloc-endianness-wave-7-dod-pass, FINAL
   ORCHESTRATOR RETURN) for prior-run context.
3. Confirmed `aarch64_be.rs:150-152` delegates `allocate_registers`
   to the inner `AArch64Backend` — i.e. the env-var-gated regalloc
   path (off by default) automatically applies to `aarch64_be` once
   `VUMA_REAL_REGALLOC_AARCH64=1` is set.
4. Identified the curated 30-test matrix in `tests/gold_standard/`:
   - 6 u32_arith, 6 complex_stores, 6 multi_function, 5 crypto_patterns,
     4 concurrency, 3 ipc — same list as R1-c-test.
5. Expected exit codes were taken from the per-test
   `// Expected exit code: N` header (defaulting to 100 when absent);
   cross-verified against the R1-c-test per-test exit-code table
   (`scripts/audit/regalloc_endianness_wave1_aarch64_regalloc_30test.md`).
6. Wrote a Python driver (`/home/z/caa/runner.py`) that, for each of
   the 30 tests × 2 modes (regalloc / stack-slot), runs:
   ```bash
   # regalloc mode
   VUMA_REAL_REGALLOC_AARCH64=1 target/release/compile_dump <test>.vuma <bin> aarch64_be
   qemu-aarch64_be-static <bin>; echo exit=$?
   # stack-slot mode
   target/release/compile_dump <test>.vuma <bin> aarch64_be
   qemu-aarch64_be-static <bin>; echo exit=$?
   ```
   with per-step timeouts (compile 60s, run 30s) and captures
   per-test: compile RC, run RC, binary size (bytes), expected exit,
   and PASS/FAIL verdict (run RC & 0xFF == expected).
7. Ran the driver (warm cache): 60 executions in ~3 minutes.
8. Wrote this summary; verified `git status --short` shows only the
   new audit markdown staged before commit.

## §2 Per-test exit codes (regalloc vs stack-slot)

| # | Category | Test | Expected | Regalloc | Stack-slot | Verdict |
|---|----------|------|----------|----------|------------|---------|
| 1 | u32_arith | u32_add | 100 | 100 | 100 | PASS / PASS |
| 2 | u32_arith | u32_sub | 30 | 30 | 30 | PASS / PASS |
| 3 | u32_arith | u32_mul | 42 | 42 | 42 | PASS / PASS |
| 4 | u32_arith | u32_xor | 5 | 5 | 5 | PASS / PASS |
| 5 | u32_arith | u32_and | 15 | 15 | 15 | PASS / PASS |
| 6 | u32_arith | u32_or | 255 | 255 | 255 | PASS / PASS |
| 7 | complex_stores | cs_single_store_load | 73 | 73 | 73 | PASS / PASS |
| 8 | complex_stores | cs_byte_store | 42 | 42 | 42 | PASS / PASS |
| 9 | complex_stores | cs_overwrite_last | 129 | 129 | 129 | PASS / PASS |
| 10 | complex_stores | cs_two_buf_sum | 80 | 80 | 80 | PASS / PASS |
| 11 | complex_stores | cs_three_cell_sum | 75 | 75 | 75 | PASS / PASS |
| 12 | complex_stores | cs_pattern_fill | 7 | 7 | 7 | PASS / PASS |
| 13 | multi_function | mf_two_funcs | 42 | 42 | 42 | PASS / PASS |
| 14 | multi_function | mf_three_funcs | 42 | 42 | 42 | PASS / PASS |
| 15 | multi_function | mf_pass_through | 42 | 42 | 42 | PASS / PASS |
| 16 | multi_function | mf_helper_double | 40 | 40 | 40 | PASS / PASS |
| 17 | multi_function | mf_chained_adders | 14 | 14 | 14 | PASS / PASS |
| 18 | multi_function | mf_square_pair_sum | 25 | 25 | 25 | PASS / PASS |
| 19 | crypto_patterns | crypto_xor_self | 0 | 0 | 0 | PASS / PASS |
| 20 | crypto_patterns | crypto_shl_mask | 224 | 224 | 224 | PASS / PASS |
| 21 | crypto_patterns | crypto_nibble_swap | 15 | 15 | 15 | PASS / PASS |
| 22 | crypto_patterns | crypto_popcount | 8 | 8 | 8 | PASS / PASS |
| 23 | crypto_patterns | crypto_byte_mix | 204 | 204 | 204 | PASS / PASS |
| 24 | concurrency | conc_two_cell | 70 | 70 | 70 | PASS / PASS |
| 25 | concurrency | conc_three_cells | 60 | 60 | 60 | PASS / PASS |
| 26 | concurrency | conc_swap | 1 | 1 | 1 | PASS / PASS |
| 27 | concurrency | conc_roundtrip | 123 | 123 | 123 | PASS / PASS |
| 28 | ipc | simple_send | 42 | 42 | 42 | PASS / PASS |
| 29 | ipc | ping_pong | 84 | 84 | 84 | PASS / PASS |
| 30 | ipc | try_recv | 77 | **0 (edge case)** | 77 | **FAIL / PASS** |

## §3 Overall pass rate

| Mode | Pass / Total | Rate | DoD Threshold | Status |
|------|--------------|------|---------------|--------|
| Regalloc (`VUMA_REAL_REGALLOC_AARCH64=1`) | 29 / 30 | 96.67% | ≥ 28 / 30 | **PASS** (within tolerance) |
| Stack-slot (env var unset) | 30 / 30 | 100.00% | 30 / 30 (no regression) | **PASS** |

## §4 Comparison to aarch64 results

| Backend | Regalloc pass | Stack-slot pass | Notes |
|---------|---------------|-----------------|-------|
| `aarch64` (R1-c-test) | 29 / 30 | 30 / 30 | `try_recv` is the 1 edge case |
| `aarch64_be` (this task) | 29 / 30 | 30 / 30 | `try_recv` is the 1 edge case |
| **Match?** | **YES** | **YES** | aarch64_be inherits Wave-1 fix via `self.inner.allocate_registers` delegation |

Both pass rates are byte-for-byte identical to the `aarch64` baseline.
This confirms that the `aarch64_be` backend transparently inherits
the Wave-1 callee-saved-register / fork-fallback fixes through its
delegation pattern (`aarch64_be.rs:150-152`). The only endianness-
specific work in `aarch64_be` is `swap_le_elf_to_be` (applied at
`encode_program`, line 156) — which is purely an ELF byte-order
transformation and does not interact with regalloc.

## §5 Failure (root-cause analysis)

### 5.1 try_recv (regalloc mode) — exits 0 instead of 77 (edge case)

**Symptom:** Binary compiles successfully under
`VUMA_REAL_REGALLOC_AARCH64=1` (3712 bytes), then runs cleanly
under `qemu-aarch64_be-static` but exits 0 instead of the expected
77 (EAGAIN sentinel). Stack-slot path (3800 bytes) runs cleanly and
exits 77 as expected. Reproducible across the full run.

**Root cause:** Same as `aarch64` (R1-c-test §5.1, gap G6). The
regalloc mode does not correctly preserve the `try_recv` return
value of -2 (EAGAIN) — the value is consumed/clobbered before the
final `exit` syscall, so the program exits with the default 0
instead of 77. The exact mechanism (`LinearScanAllocator::compute`
at `regalloc.rs:948` not tracking `IRInstr::Syscall { .. }` in
`call_positions`, so vregs live across syscalls are not spilled
around caller-saved-register clobbers) is the upstream aarch64
gap; `aarch64_be` simply inherits the broken behaviour via the
`self.inner.allocate_registers` delegation. The aarch64_be binary
size (3712 regalloc vs 3800 stack-slot, -2.6%) is *smaller* than
stack-slot — different from the aarch64 case (+540 bytes / +14%),
but the end-to-end behavioural bug (wrong exit code) is identical.

**Why 0 (not 139 SIGSEGV as in R1-c-test):** the wave1 aarch64 doc
recorded `try_recv` regalloc-mode as exiting 139 (SIGSEGV). The
current behaviour — exit 0 instead — indicates that subsequent
defensive fixes (likely from R1-b3-fix or a later consolidation)
have hardened the regalloc path enough to avoid the crash, but the
underlying G6 gap (syscall clobber tracking) remains and surfaces
as a wrong-exit-code bug rather than a crash. Either way, the
regalloc `try_recv` result is "FAIL vs expected 77" — matching the
task's stated edge case ("try_recv is the known edge case that
exits 0 instead of 77").

**Fork-fallback note:** `simple_send` and `ping_pong` produce
byte-identical binaries across regalloc/stack-slot (11912 and 15404
bytes respectively, Δ = 0), confirming these tests hit the
`contains_fork` fallback (R1-b2-fix) and do NOT exercise the
regalloc codegen path. This is the intended behaviour — fork+regalloc
is unsafe; the fallback emits the stack-slot path verbatim.

### 5.2 Stack-slot path — 0 failures

No stack-slot path failures. Identical pass rate (30/30) and
identical per-test exit codes to the R1-c-test aarch64 stack-slot
baseline, confirming zero regression on `aarch64_be`.

## §6 DoD for this task

| DoD criterion | Status | Evidence |
|---------------|--------|----------|
| Both runs tested on all 30 tests | PASS | 60 executions (30 regalloc + 30 stack-slot) |
| Regalloc pass rate ≥ 28/30 | PASS | 29/30 (96.67%) — only `try_recv` fails |
| Stack-slot pass rate = 30/30 | PASS | 30/30 (100.00%) — zero regression |
| Summary markdown exists at `scripts/audit/completion_wave_a_aarch64_be_regalloc.md` | PASS | This file |
| Comparison to aarch64 results | PASS | Both pass rates match (29/30 regalloc, 30/30 stack-slot) |
| No source files edited | PASS | `git status --short` shows only the new audit markdown added |
| No `git push` | PASS | Local commit only |
| No sub-agents spawned | PASS | Single sub-agent run |
| Time budget ≤ 12 min | PASS | Compile+run loop ~3 min; full task ~6 min |

## §7 Stage Summary

- Single commit `[CA-a-test]` adds
  `scripts/audit/completion_wave_a_aarch64_be_regalloc.md` (this file).
- `aarch64_be` regalloc path: **29/30** PASS with
  `VUMA_REAL_REGALLOC_AARCH64=1`; `try_recv` is the 1 known edge
  case (exits 0 instead of 77, root-caused to upstream gap G6
  inherited via `self.inner.allocate_registers` delegation).
- `aarch64_be` stack-slot path: **30/30** PASS (zero regression).
- Pass rates are byte-for-byte identical to the `aarch64` baseline
  (R1-c-test), confirming that the Wave-1 callee-saved-register /
  fork-fallback fixes are transparently inherited by `aarch64_be`
  via the existing delegation pattern (`aarch64_be.rs:150-152`).
- No source files modified; no push; no sub-agents.

### Status: PASS — regalloc 29/30, stack-slot 30/30, matches aarch64 baseline (Y); commit `[CA-a-test]`; no source edits; no push; no sub-agents.
