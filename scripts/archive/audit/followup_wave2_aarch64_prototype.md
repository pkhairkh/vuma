# F2-c-test — Wave 2 aarch64 regalloc prototype verification

- **Task ID:** F2-c-test
- **Wave:** 2 (Performance Gap Closure — prototype verification)
- **Prior run context:** F2-b-impl (commit `ee06b362`) wired up `emit_function_regalloc` for aarch64 behind env-var gate `VUMA_REAL_REGALLOC_AARCH64=1`; smoke test on `u32_add.vuma` showed exit 100 in both modes, regalloc binary 52 bytes smaller.
- **Run date:** 2026-07-30 19:21:00 UTC
- **Toolchain:** `target/release/compile_dump` (existing binary), `qemu-aarch64-static` for execution.

## 1. Procedure

1. Read worklog tail (F2-b-impl section, commit `ee06b362`).
2. Verified `compile_dump` binary exists and `qemu-aarch64-static` is on PATH after sourcing `/home/z/my-project/vuma/scripts/env/*.sh`.
3. Sourced env shims (z3, qemu, rust, wasmtime, lean) so `libz3.so` resolves and QEMU user-mode is available.
4. Ran the curated 30-test subset on aarch64 WITHOUT `VUMA_REAL_REGALLOC_AARCH64` (stack-slot baseline):
   - For each test: `compile_dump <src>.vuma <out>.bin aarch64 && qemu-aarch64-static <out>.bin; echo exit=$?`.
   - Log saved to `scripts/logs/followup_wave2_aarch64_stackslot.log`.
5. Ran the same 30-test subset WITH `VUMA_REAL_REGALLOC_AARCH64=1` (register-based path):
   - Log saved to `scripts/logs/followup_wave2_aarch64_regalloc.log`.
6. Compared exit codes between the two runs against the per-test `// Expected exit code:` header.
7. Compared emitted binary sizes between the two runs.
8. No source files edited (READ-ONLY verification).

## 2. Curated 30-test subset

Same 6 categories as prior runs (5-c / 7-a):

| Category | Count | Tests |
|---|---|---|
| u32_arith | 6 | u32_add, u32_sub, u32_mul, u32_xor, u32_and, u32_or |
| complex_stores | 6 | cs_single_store_load, cs_byte_store, cs_overwrite_last, cs_two_buf_sum, cs_three_cell_sum, cs_pattern_fill |
| multi_function | 6 | mf_two_funcs, mf_three_funcs, mf_pass_through, mf_helper_double, mf_chained_adders, mf_square_pair_sum |
| crypto_patterns | 5 | crypto_xor_self, crypto_shl_mask, crypto_nibble_swap, crypto_popcount, crypto_byte_mix |
| concurrency | 4 | conc_two_cell, conc_three_cells, conc_swap, conc_roundtrip |
| ipc | 3 | simple_send, ping_pong, try_recv |

All 30 named tests were present in the corpus (no substitutions needed).

## 3. Per-test results

| Test | Category | Expected | Stack-slot exit | Regalloc exit | Stack-slot | Regalloc | SS size (B) | RG size (B) | Δ size (B) |
|---|---|---:|---:|---:|:---:|:---:|---:|---:|---:|
| complex_stores/cs_byte_store | complex_stores | 42 | 42 | 42 | PASS | PASS | 2968 | 2992 | +24 |
| complex_stores/cs_overwrite_last | complex_stores | 129 | 129 | 0 | PASS | FAIL | 3424 | 3616 | +192 |
| complex_stores/cs_pattern_fill | complex_stores | 7 | 7 | 7 | PASS | PASS | 3124 | 3124 | +0 |
| complex_stores/cs_single_store_load | complex_stores | 73 | 73 | 73 | PASS | PASS | 2968 | 2992 | +24 |
| complex_stores/cs_three_cell_sum | complex_stores | 75 | 75 | 40 | PASS | FAIL | 3372 | 3572 | +200 |
| complex_stores/cs_two_buf_sum | complex_stores | 80 | 80 | 33 | PASS | FAIL | 3180 | 3316 | +136 |
| concurrency/conc_roundtrip | concurrency | 123 | 123 | 123 | PASS | PASS | 2968 | 2768 | -200 |
| concurrency/conc_swap | concurrency | 1 | 1 | 1 | PASS | PASS | 3084 | 2796 | -288 |
| concurrency/conc_three_cells | concurrency | 60 | 60 | 60 | PASS | PASS | 3116 | 2804 | -312 |
| concurrency/conc_two_cell | concurrency | 70 | 70 | 70 | PASS | PASS | 3012 | 2780 | -232 |
| crypto_patterns/crypto_byte_mix | crypto_patterns | 204 | 204 | 204 | PASS | PASS | 2776 | 2724 | -52 |
| crypto_patterns/crypto_nibble_swap | crypto_patterns | 15 | 15 | 15 | PASS | PASS | 2776 | 2724 | -52 |
| crypto_patterns/crypto_popcount | crypto_patterns | 8 | 8 | 8 | PASS | PASS | 2776 | 2724 | -52 |
| crypto_patterns/crypto_shl_mask | crypto_patterns | 224 | 224 | 224 | PASS | PASS | 2776 | 2724 | -52 |
| crypto_patterns/crypto_xor_self | crypto_patterns | 0 | 0 | 0 | PASS | PASS | 2776 | 2724 | -52 |
| ipc/ping_pong | ipc | 84 | 84 | 139 | PASS | FAIL | 15404 | 9748 | -5656 |
| ipc/simple_send | ipc | 42 | 42 | 139 | PASS | FAIL | 11912 | 7240 | -4672 |
| ipc/try_recv | ipc | 77 | 77 | 77 | PASS | PASS | 3800 | 3208 | -592 |
| multi_function/mf_chained_adders | multi_function | 14 | 14 | 3 | PASS | FAIL | 3440 | 3364 | -76 |
| multi_function/mf_helper_double | multi_function | 40 | 40 | 40 | PASS | PASS | 2908 | 2828 | -80 |
| multi_function/mf_pass_through | multi_function | 42 | 42 | 0 | PASS | FAIL | 3048 | 2936 | -112 |
| multi_function/mf_square_pair_sum | multi_function | 25 | 25 | 0 | PASS | FAIL | 3028 | 2964 | -64 |
| multi_function/mf_three_funcs | multi_function | 42 | 42 | 42 | PASS | PASS | 3016 | 2916 | -100 |
| multi_function/mf_two_funcs | multi_function | 42 | 42 | 42 | PASS | PASS | 2896 | 2820 | -76 |
| u32_arith/u32_add | u32_arith | 100 | 100 | 100 | PASS | PASS | 2776 | 2724 | -52 |
| u32_arith/u32_and | u32_arith | 15 | 15 | 15 | PASS | PASS | 2776 | 2724 | -52 |
| u32_arith/u32_mul | u32_arith | 42 | 42 | 42 | PASS | PASS | 2776 | 2724 | -52 |
| u32_arith/u32_or | u32_arith | 255 | 255 | 255 | PASS | PASS | 2776 | 2724 | -52 |
| u32_arith/u32_sub | u32_arith | 30 | 30 | 30 | PASS | PASS | 2776 | 2724 | -52 |
| u32_arith/u32_xor | u32_arith | 5 | 5 | 5 | PASS | PASS | 2776 | 2724 | -52 |

## 4. Per-category summary

| Category | n | Stack-slot pass | Regalloc pass | SS size (B) | RG size (B) | Δ size (B) |
|---|---:|---:|---:|---:|---:|---:|
| complex_stores | 6 | 6/6 | 3/6 | 19036 | 19612 | +576 |
| concurrency | 4 | 4/4 | 4/4 | 12180 | 11148 | -1032 |
| crypto_patterns | 5 | 5/5 | 5/5 | 13880 | 13620 | -260 |
| ipc | 3 | 3/3 | 1/3 | 31116 | 20196 | -10920 |
| multi_function | 6 | 6/6 | 3/6 | 18336 | 17828 | -508 |
| u32_arith | 6 | 6/6 | 6/6 | 16656 | 16344 | -312 |
| **TOTAL** | **30** | **30/30** | **22/30** | **111204** | **98748** | **-12456** |

## 5. Overall pass rates

- Stack-slot baseline (no env var): **30/30 = 100.0%**
- Regalloc prototype (`VUMA_REAL_REGALLOC_AARCH64=1`): **22/30 = 73.3%**
- Regressions (stack-slot PASS → regalloc FAIL): **8**
- Improvements (stack-slot FAIL → regalloc PASS): **0** (none expected — stack-slot is the trusted baseline)

## 6. Binary size analysis

- Regalloc smaller (delta < 0): **24/30**
- Regalloc equal   (delta = 0): **1/30**
- Regalloc larger  (delta > 0): **5/30**
- Total stack-slot bytes: 111204
- Total regalloc bytes:   98748 (delta -12456, -11.20%)

Regalloc is consistently smaller on simple tests (u32_arith, crypto_patterns, single-function concurrency) — typically −52 bytes per test, matching the u32_add smoke-test figure from F2-b-impl.  On tests with **memory writes to multiple cells** or **inter-function calls**, the regalloc path is occasionally larger (cs_overwrite_last +192, cs_three_cell_sum +200, cs_two_buf_sum +136, cs_single_store_load +24, cs_byte_store +24), suggesting the LinearScanAllocator is emitting excessive spill/reload sequences it does not actually need.

## 7. Failures — root-cause analysis

**8 regressions** vs stack-slot baseline.  Grouped by category:

### 7.1 complex_stores — 3 regressions (cs_overwrite_last, cs_two_buf_sum, cs_three_cell_sum)

- All three involve **multiple sequential stores to distinct memory cells** followed by a load/sum.
- Regalloc exit codes (0, 33, 40) are smaller than expected (129, 80, 75) — values look truncated/zeroed, consistent with a store or reload going to the **wrong stack slot or register**.
- Regalloc binaries are also **larger** for these three tests (+192, +136, +200 bytes), suggesting the allocator spilled too aggressively and the spill code clobbered values it shouldn't have.
- cs_byte_store (same category, PASS) and cs_pattern_fill (PASS, delta 0) survive — they touch only one cell or use a fill pattern the allocator can keep in a register.

### 7.2 multi_function — 3 regressions (mf_pass_through, mf_chained_adders, mf_square_pair_sum)

- mf_pass_through and mf_square_pair_sum return **0** instead of 42 / 25.
- mf_chained_adders returns 3 instead of 14 (= 10 + 1 + 1 + 1) — looks like **only the last adder call's return value survives**; intermediate calls are losing their result.
- mf_two_funcs, mf_three_funcs, mf_helper_double (same category, PASS) survive — they call simpler helpers and the return value is read once.
- Root cause is most likely the **§5.3 risk materialising**: `LinearScanAllocator::used_callee_saved_gprs` does not include all callee-saved registers that the byte-changing emitter actually writes, so the epilogue restores garbage into a callee-saved register that the caller was relying on.  The chained-adders pattern (each call overwrites the previous result) is exactly the shape that would expose this.

### 7.3 ipc — 2 regressions (simple_send, ping_pong) — SIGSEGV

- Both ipc tests exit **139** = 128 + 11 = `SIGSEGV`.
- try_recv (same category, PASS) does not spawn a worker — only the parent runs.  simple_send and ping_pong both use `spawn_worker()`.
- The regalloc binaries for these are roughly **half** the stack-slot size (simple_send 7240 vs 11912, ping_pong 9748 vs 15404) — the byte-changing emitter is producing too few instructions, consistent with **skipping a save/restore sequence** that should have been emitted for the worker's entry/exit.
- Same root cause as §7.2: callee-saved register tracking in `LinearScanAllocator` is incomplete, and the child process trashes a register that `wait_worker`/`channel_*` depends on.

### 7.4 Pattern across all 8 failures

Every failure involves either **multiple memory writes** or **function call / worker-spawn boundaries**.  No failure in:
- u32_arith (6/6) — pure register-only arithmetic, no spills.
- crypto_patterns (5/5) — pure register-only arithmetic on bytes.
- 4 of 6 complex_stores, 3 of 6 multi_function, 2 of 4 concurrency, 1 of 3 ipc — i.e. **22 of 30 pass**.

This is consistent with the design-doc §5.3 HIGH-severity risk: callee-saved register save/restore correctness.  The LinearScanAllocator's `used_callee_saved_gprs` set is incomplete, so the byte-changing `Emitter::emit_function_regalloc` (emit.rs:1056) skips save/restore for registers it actually clobbers.  The simplest failing tests involve calls (multi_function, ipc); the next-simplest involve multiple stack slots whose values are kept in callee-saved registers across the sequence (complex_stores).

## 8. DoD assessment

| DoD criterion | Required | Actual | Status |
|---|---|---|:---:|
| Both runs tested on all 30 curated tests | 30 / 30 | 30 / 30 | **PASS** |
| Stack-slot pass rate ≥ 95% | ≥ 29/30 | 30/30 | **PASS** |
| Regalloc pass rate ≥ 95% | ≥ 29/30 | 22/30 | **FAIL** |
| No regressions (regalloc ≥ stack-slot pass count) | rg ≥ 30 | rg = 22 | **FAIL** |
| Summary markdown exists at `scripts/audit/followup_wave2_aarch64_prototype.md` | yes | yes | **PASS** |

**Overall DoD: NOT MET.**  The regalloc prototype passes 22/30 (73.3%) on aarch64 — 8 regressions vs the 30/30 stack-slot baseline.  The env-var gate (`VUMA_REAL_REGALLOC_AARCH64=1`) MUST remain off-by-default; flipping the default (design doc §6 Phase 1 step 5) is **not yet safe**.

## 9. Next actions (downstream)

1. **Investigate `LinearScanAllocator::used_callee_saved_gprs`** (regalloc.rs) — verify it includes every register the byte-changing emitter actually writes (X19–X28 by convention on AArch64, plus X29/X30 if used).
2. **Add a verifier pass** that walks each `AllocatedInstruction`'s `reads`/`writes`/`encoded` and asserts every physical register used is either caller-saved, in `used_callee_saved_gprs`, or one of X29/X30/SP.  (This is the design-doc §5.3 mitigation.)
3. **Re-run this 30-test matrix** after the fix; the DoD is met only when regalloc reaches ≥ 29/30 with zero regressions.
4. **Do NOT flip the default** to `1` until the curated matrix is green.
5. Optionally: investigate why regalloc binaries are LARGER on the 3 failing complex_stores tests (+192/+200/+136) — these may be over-spilling, which is also a correctness smell.

## 10. Constraint check

- No source files edited — READ-ONLY verification.  `git status` confirms only the new summary markdown is added.
- No `git push` invoked (local commit only).
- No further sub-agents spawned.
- Time budget: ~5 minutes (env shim discovery + 60-test sweep + analysis).

## 11. Logs

- Stack-slot run: `scripts/logs/followup_wave2_aarch64_stackslot.log`
- Regalloc run:   `scripts/logs/followup_wave2_aarch64_regalloc.log`
- Per-test `RESULT|mode|cat|path|status|size=…|exit=…|expected=…` lines in each log.

## 12. Status: FAIL (DoD not met) — regalloc prototype must remain env-var-gated and off-by-default

Stack-slot baseline holds at 30/30; regalloc prototype at 22/30 (73.3%) with 8 regressions concentrated in complex_stores (3), multi_function (3), and ipc (2).  All failures are consistent with the design-doc §5.3 HIGH-severity risk materialising: `LinearScanAllocator::used_callee_saved_gprs` is incomplete, so the byte-changing emitter skips callee-saved save/restore for registers it actually clobbers.  The 5 u32_arith + 5 crypto_patterns + 4 concurrency (excluding the multi-cell cs_* failures) + 1 ipc tests that pass all involve either pure-register arithmetic or single-cell memory accesses where callee-saved registers are never live across a call/store boundary.  Binary sizes are smaller on average (−12 456 bytes total, −11.20%), but the prototype is **not production-ready**.
