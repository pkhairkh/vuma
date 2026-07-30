# Wave 5-d-test — wasmtime wasm32 row of the 19-backend matrix (caveat §4.3)

- **Task ID:** 5-d-test
- **Agent:** 5-d-test (sub-agent, wave 5)
- **Wave:** 5 (depends on waves 0 / 1 / 2 / 3 / 4 / 5-a-test / 5-b-test / 5-c-audit)
- **Caveat addressed:** §4.3 — wasm32 row of the 19-backend matrix runs under `wasmtime` v29+
- **Files in scope (READ-ONLY audit + test execution; NO source edits):** `scripts/pi5_test_suite.sh` (ro), `scripts/vuma_test_matrix_19backends.sh` (ro, invocation reference), `tests/gold_standard/` (corpus). New: `scripts/audit/wave5_wasmtime_row.md` (this file).
- **DoD:** wasmtime version ≥ v29; ≥ 20 of 30 curated tests pass on wasm32 under wasmtime; this markdown exists.

## Environment

| Item | Value |
|---|---|
| `wasmtime --version` | `wasmtime 29.0.0 (545407736 2025-01-20)` — satisfies ≥ v29 ✓ |
| `which wasmtime` | `/home/z/.wasmtime/bin/wasmtime` |
| `compile_dump` | `target/release/compile_dump` (uses canonical `compile_with_path` pipeline, which DOES call `lower_ipc_builtins`) |
| Runner | x86_64 Linux, user `z` |

## Invocation contract

Same as 5-c-audit for the 18 QEMU rows, but for the 19th row:

```
compile_dump <test.vuma> <out.wasm> wasm32
wasmtime     <out.wasm>
```

The reference matrix script `scripts/vuma_test_matrix_19backends.sh` line 82–85 only prints `wasm` and **skips** the actual wasmtime execution — confirming that 5-d-test is the first task to actually exercise the wasm32 row under wasmtime. The .wasm emitted by `compile_dump` is a valid WebAssembly MVP module (verified via `file`: `WebAssembly (wasm) binary module version 0x1 (MVP)`).

## Curated 30-test subset

Same 6 categories as 5-c-audit (so the wasm32 row is directly comparable to the 18 QEMU rows):

| Category | Count | Tests |
|---|---|---|
| u32_arith | 6 | u32_add, u32_2_add, u32_2_mul, u32_2_and, u32_2_or, u32_2_mask |
| complex_stores | 6 | cs2_byte_store, cs2_copy, cs2_chain, cs2_multi_byte, cs2_double_buf, cs2_independent |
| multi_function | 6 | mf_accumulator, mf_calculator, mf_call_chain_3, mf_call_in_expr, mf_call_two_then_sum, mf_chain_3 |
| crypto_patterns | 5 | and_then_xor, crypto2_and, crypto2_bit_reverse, crypto2_byte_swap, crypto2_gray |
| concurrency | 4 | conc2_chain, conc2_copy, conc2_roundtrip, conc2_swap |
| ipc | 3 | simple_send, ping_pong, try_recv (incl. exit-77 try_recv path from wave 4-d) |
| **Total** | **30** | |

## Methodology

Harness at `/home/z/wave5d_harness.py` (ephemeral; outside repo, not committed — Write tool is sandboxed to `/home/z`). For each `(category, test)`:

1. Parse expected exit code from `// Expected exit code:` header in the .vuma.
2. `compile_dump <vuma> <wasm> wasm32` with a 30-s timeout → emits `.wasm`.
3. `wasmtime <wasm>` with a 5-s timeout → observed exit code.
4. `PASS` if observed == expected; else `FAIL` with wasmtime stderr captured.

Per-test log at `/home/z/my-project/scripts/logs/wave5_wasmtime.log` (outside repo, matching wave-3/4/5 convention). JSON at `wave5_wasmtime.json` in the same dir.

## Results

**27 / 30 PASS (90.0%).** Elapsed: 0.28 s. DoD threshold (≥ 20 / 30) exceeded by 7.

| # | Category | Test | Expected | Got | Verdict |
|---|---|---|---|---|---|
| 1 | u32_arith | u32_add | 100 | 100 | PASS |
| 2 | u32_arith | u32_2_add | 100 | 100 | PASS |
| 3 | u32_arith | u32_2_mul | 42 | 42 | PASS |
| 4 | u32_arith | u32_2_and | 15 | 15 | PASS |
| 5 | u32_arith | u32_2_or | 255 | 1 | FAIL |
| 6 | u32_arith | u32_2_mask | 255 | 1 | FAIL |
| 7 | complex_stores | cs2_byte_store | 42 | 42 | PASS |
| 8 | complex_stores | cs2_copy | 42 | 42 | PASS |
| 9 | complex_stores | cs2_chain | 55 | 55 | PASS |
| 10 | complex_stores | cs2_multi_byte | 6 | 6 | PASS |
| 11 | complex_stores | cs2_double_buf | 84 | 84 | PASS |
| 12 | complex_stores | cs2_independent | 42 | 42 | PASS |
| 13 | multi_function | mf_accumulator | 6 | 6 | PASS |
| 14 | multi_function | mf_calculator | 12 | 12 | PASS |
| 15 | multi_function | mf_call_chain_3 | 32 | 32 | PASS |
| 16 | multi_function | mf_call_in_expr | 10 | 10 | PASS |
| 17 | multi_function | mf_call_two_then_sum | 30 | 30 | PASS |
| 18 | multi_function | mf_chain_3 | 11 | 11 | PASS |
| 19 | crypto_patterns | and_then_xor | 19 | 19 | PASS |
| 20 | crypto_patterns | crypto2_and | 15 | 15 | PASS |
| 21 | crypto_patterns | crypto2_bit_reverse | 240 | 1 | FAIL |
| 22 | crypto_patterns | crypto2_byte_swap | 120 | 120 | PASS |
| 23 | crypto_patterns | crypto2_gray | 4 | 4 | PASS |
| 24 | concurrency | conc2_chain | 3 | 3 | PASS |
| 25 | concurrency | conc2_copy | 42 | 42 | PASS |
| 26 | concurrency | conc2_roundtrip | 123 | 123 | PASS |
| 27 | concurrency | conc2_swap | 1 | 1 | PASS |
| 28 | ipc | simple_send | 42 | 42 | PASS |
| 29 | ipc | ping_pong | 84 | 84 | PASS |
| 30 | ipc | try_recv | 77 | 77 | PASS |

### Per-category pass rate

| Category | Pass / Total | Rate |
|---|---|---|
| u32_arith | 4 / 6 | 66.7% |
| complex_stores | 6 / 6 | 100.0% |
| multi_function | 6 / 6 | 100.0% |
| crypto_patterns | 4 / 5 | 80.0% |
| concurrency | 4 / 4 | 100.0% |
| ipc | 3 / 3 | 100.0% |
| **Total** | **27 / 30** | **90.0%** |

### 3 FAILures — root cause analysis

All 3 failures share an identical symptom and root cause:

```
Error: failed to run main module `…wasm`
Caused by:
    0: failed to instantiate "…wasm"
    1: error while executing at wasm backtrace:
           0:  0x41c - <unknown>!<wasm function 18>
    2: exit with invalid exit status outside of [0..126)
```

The expected exit codes of the 3 failing tests are **255, 255, 240** — all **≥ 126**, which falls outside the `[0..126)` window that wasmtime v29 enforces for `wasi_snapshot_preview1`'s `proc_exit`. (POSIX reserves exit codes ≥ 128 for signal-terminated processes, and wasmtime's strictness here is a known deliberate enforcement.)

So the failure is **not** a VUMA codegen bug — the wasm32 backend correctly emits `proc_exit(255)` (or `proc_exit(240)`); wasmtime rejects it because the value is out of the valid Unix exit-code range. The 27 passing tests all exit with codes in `[0..126)` and pass cleanly.

**Implication for caveat §4.3:** the wasm32 row is functionally sound under wasmtime v29. Tests authored with exit codes ≥ 126 will need to either (a) use a side-channel (e.g. shared memory, fd write) to communicate the result, or (b) be re-pinned to exit codes in `[0..126)`. This is a **test-authoring convention** for the wasm32 row, not a runtime defect — and is documented here as a follow-up for the corpus curators. Out of scope for this read-only test task.

### IPC under wasm32

The 3 IPC tests (`simple_send`, `ping_pong`, `try_recv` incl. the exit-77 try_recv path from wave 4-d) **all pass under wasmtime** — same result as the 18 QEMU rows. This is because `compile_dump` uses the canonical `compile_with_path` pipeline that DOES call `lower_ipc_builtins`, NOT the `vuma build`/`vuma run`/`vuma emit` CLI path that waves 4-b/4-c flagged as skipping `lower_ipc_builtins`. That CLI gap is out of scope here and does not affect this matrix row.

## DoD Assessment

| DoD criterion | Status | Evidence |
|---|---|---|
| wasmtime version ≥ v29 | **PASS** | `wasmtime 29.0.0 (545407736 2025-01-20)` |
| ≥ 20 of 30 curated tests pass on wasm32 under wasmtime | **PASS** | 27 / 30 (90.0%) pass |
| Summary markdown at `vuma/scripts/audit/wave5_wasmtime_row.md` | **PASS** | this file |

## Constraint check

- No source files edited under `vuma/src/`. `git status` shows only the new audit markdown (+ worklog append).
- `scripts/pi5_test_suite.sh` and `scripts/vuma_test_matrix_19backends.sh` treated as READ-ONLY (invocation-contract reference only; not invoked).
- `compile_dump` invoked directly — same approach as 5-c-audit for the 18 QEMU rows.
- Per-test logs at `/home/z/my-project/scripts/logs/wave5_wasmtime.log` and `wave5_wasmtime.json` (outside the repo, matching wave-3/4/5 convention).
- Harness `/home/z/wave5d_harness.py` is ephemeral, outside the repo, not committed.
- No push (local commit only).
- No further sub-agents spawned.
- Time budget: ~6 minutes (well under 10-minute cap).

## Note for orchestrator

Caveat §4.3 satisfied: **wasm32 row of the 19-backend matrix passes 27/30 (90.0%) under wasmtime v29.0.0** with the same curated 30-test subset used by 5-c-audit for the 18 QEMU rows. The 3 failures (u32_2_or, u32_2_mask, crypto2_bit_reverse) are NOT VUMA defects — they are tests whose expected exit codes (255, 255, 240) fall outside wasmtime's enforced `[0..126)` range for `proc_exit`. Recommended follow-up (out of scope, source-edit task): either re-author the 3 affected tests to use exit codes in `[0..126)`, or document an exit-code-window convention for the wasm32 row in the corpus README. The 19th row of the matrix is now exercised end-to-end, completing caveat §4.3 alongside §4.2 (QEMU 18 rows).

### Status: PASS (27 / 30 ≥ 20 threshold; wasmtime v29.0.0 ≥ v29; 3 FAILs attributable to wasmtime exit-code window, not VUMA defects)
