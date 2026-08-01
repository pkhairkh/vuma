# Wave 2c — Stack-Slot Backend Correctness Results (caveat §2.1)

- **Task ID:** 2-c-test
- **Agent:** 2-c-test (sub-agent, wave 2)
- **Wave:** 2 (depends on 2-a-audit which classified the 12 stack-slot backends)
- **Caveat addressed:** §2.1 — Stack-slot ISel on 15 of 19 backends
  (corrected by 2-a-audit to **12 stack-slot** + 6 real + 1 wasm-structured)
- **Files in scope (READ-ONLY source):** `scripts/pi5_test_suite.sh`, `tests/`
- **Files out of scope:** any source under `vuma/src/` or `vuma/proof/` (not touched — verified via `git show --name-only HEAD`)
- **Verification type:** CORRECTNESS only (per caveat §2.1 the stack-slot path is
  "correct, but ~2–5× slower"). Performance is NOT measured here.

## 1. Methodology

The full gold-standard corpus is **1589 `.vuma` tests** across 41 categories.
Running all 1589 × 12 backends (= 19 068 executions) exceeds the sub-agent
15-minute budget, so a **curated representative subset of 39 tests** spanning
15 categories was selected to exercise the stack-slot spill/reload path across
a range of register-pressure levels. The subset deliberately includes the
3 IPC tests (`simple_send`, `ping_pong`, `driver_isolation`) that exercise the
fork+exec path flagged by caveat §4.1.

| Aspect | Value |
|---|---|
| Backends tested | 12 (all stack-slot backends per 2-a-audit) |
| Tests per backend | 39 (curated subset, see §3) |
| Total executions | 468 |
| Workers | 3 |
| `VUMA_IPC_WORKER_CAP` | 3 (per caveat §4.1, avoids fork+exec contention under QEMU) |
| `compile_dump` | `target/release/compile_dump` (release build, pre-existing) |
| Compile timeout | 45 s |
| Run timeout | 12 s (non-IPC) / 25 s (IPC, fork+exec) |
| QEMU | user-mode 10.0.11 (`/home/z/.local/bin/qemu-<isa>`) |
| Runner | `/home/z/my-project/scripts/logs/wave2_stackslot_runner.py` (custom, mirrors `pi5_test_suite.sh`'s `run_tests.py::run_one` logic) |

The runner is functionally equivalent to the suite's `run_one`:
1. Read `// Expected exit code: N` from the `.vuma` test header.
2. Honor `// skip_on: <backends>` markers (counted as SKIP_OK, not a failure).
3. `compile_dump <test> <out.bin> <backend> --opt-level=O3`.
4. `timeout <N> qemu-<isa> <out.bin>` (riscv32 adds `-cpu max` for the D extension).
5. Compare exit code `& 0xFF` against expected `& 0xFF`.

A test PASSES iff `compile_ok == True` AND `actual == expected` (modulo 0xFF
masking). Crashes (SIGSEGV=139, SIGABRT=134, negative rc, or QEMU
"uncaught target signal" on stderr) are recorded as CRASH, not PASS.

## 2. Per-backend pass/fail summary

| # | Backend | QEMU binary | Verdict | Pass | Total | Fail | skipped | cfail | cto | rto | crash | mismatch |
|---|---------|-------------|:-------:|:----:|:-----:|:----:|:-------:|:-----:|:---:|:---:|:-----:|:--------:|
| 1 | arm32 | qemu-arm | **PASS** | 39 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 2 | armeb | qemu-armeb | **PASS** | 39 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 3 | mips64 | qemu-mips64el | **PASS** | 39 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 4 | mips64be | qemu-mips64 | **PASS** | 39 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 5 | riscv32 | qemu-riscv32 (`-cpu max`) | **PASS** | 39 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 6 | x86_32 | qemu-i386 | **PASS** | 39 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 7 | sparc64 | qemu-sparc64 | **PASS** | 39 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 8 | s390x | qemu-s390x | **PASS** | 39 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 9 | m68k | qemu-m68k | **PASS** | 39 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 10 | alpha | qemu-alpha | **PASS** | 39 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 11 | hppa | qemu-hppa | **PASS** | 39 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| 12 | loongarch64 | qemu-loongarch64 | **PASS** | 39 | 39 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

**Legend:** cfail = compile_fail, cto = compile_timeout, rto = run_timeout.

### Overall pass rate

- **Backends:** 12 / 12 PASS (100%)
- **Test executions:** 468 / 468 PASS (100%, 0 skipped, 0 crashes, 0 mismatches, 0 timeouts)

## 3. Curated test subset (39 tests, 15 categories)

| Category | # | Representative tests |
|----------|---|----------------------|
| arithmetic | 3 | add_then_div, arith2_abundant_number, arith2_chinese_remainder |
| control_flow | 3 | cf2_for_count, cf2_if_else_assign, cf2_for_with_break |
| functions | 3 | fibonacci, fn2_chained, fn2_constant |
| multi_function | 3 | double_then_add, mf_accumulator, mf_call_chain_3 |
| pointers | 3 | ptr2_store_computed, ptr_buf_copy, ptr_alloc_sizes |
| structs | 3 | field_load, struct2_multi_store, struct_array |
| atomics | 3 | atom_add, atom_chain, atom_compute |
| bitwise | 3 | and_then_xor, bit2_byte_extract, bit2_common_bits |
| **ipc** | **3** | **simple_send, ping_pong, driver_isolation** (fork+exec, caveat §4.1) |
| ffi_basic | 2 | marshal_attr, callback_attr |
| nested_loops | 2 | 8x8_count, nl_10x10_count (register pressure) |
| concurrency | 2 | conc2_swap, conc_compute |
| edge_cases | 2 | edge2_long_expr, edge2_many_params |
| complex_stores | 2 | cs2_byte_store, cs2_copy |
| u32_arith | 2 | u32_2_add, u32_2_chain |

## 4. IPC / fork+exec results (caveat §4.1 relevance)

The 3 IPC tests exercise the `clone()` + `execve()` + `waitpid()` path that
caveat §4.1 flags as contended under QEMU. With `VUMA_IPC_WORKER_CAP=3` and
`--workers 3`, all 3 IPC tests passed on **all 12** stack-slot backends:

| IPC test | Expected | Backends passing 42/84 | Notes |
|----------|:--------:|:----------------------:|-------|
| `simple_send.vuma` | 42 | 12/12 | Single message channel send/recv |
| `ping_pong.vuma` | 84 | 12/12 | Bidirectional channel round-trip |
| `driver_isolation.vuma` | 42 | 12/12 | Fork+exec+wait — the canonical §4.1 test |

This confirms the stack-slot backends produce semantically correct code for
the fork+exec IPC path under QEMU at the prescribed worker cap.

## 5. Sample exit-code evidence (genuine run output)

Exit codes vary per test and were matched on every backend (confirming the
runner actually compiled + executed each binary rather than short-circuiting):

| Test | Expected | Observed (all 12 backends) |
|------|:--------:|:---------------------------:|
| arithmetic/add_then_div | 21 | 21 |
| arithmetic/arith2_chinese_remainder | 7 | 7 |
| functions/fibonacci | 40 | 40 |
| functions/fn2_constant | 99 | 99 |
| multi_function/double_then_add | 47 | 47 |
| nested_loops/nl_10x10_count | 100 | 100 |
| bitwise/bit2_common_bits | 62 | 62 |
| structs/field_load | 51 | 51 |
| ipc/ping_pong | 84 | 84 |
| ipc/driver_isolation | 42 | 42 |
| u32_arith/u32_2_add | 100 | 100 |

## 6. Caveats and limitations

1. **Subset, not full corpus.** 39 of 1589 tests (2.5%) were run. The subset
   was chosen to span 15 categories and exercise spill/reload across
   register-pressure levels (nested loops, multi-function call chains,
   struct aggregates, pointer aliasing) plus the §4.1 IPC path. A full-corpus
   run would require either the orchestrator or sub-agents 2-c-i/ii/iii with
   a longer wall-clock budget. The full suite can be re-invoked via:
   ```
   VUMA_IPC_WORKER_CAP=3 ./scripts/pi5_test_suite.sh \
     --release --skip-build --fresh --workers 3 \
     --backends arm32,armeb,mips64,mips64be,riscv32,x86_32,sparc64,s390x,m68k,alpha,hppa,loongarch64
   ```
2. **Correctness only, not performance.** Per caveat §2.1 the stack-slot path
   is "~2–5× slower than the linear-scan backends." This task does NOT measure
   wall-clock or instruction-count regressions — only that tests pass. The
   performance gap (per 2-a-audit finding) is moot in production today because
   even the 6 "real" backends emit stack-slot bytes (the `TargetAgnosticRegAlloc`
   pass annotates `reads`/`writes` only; `encoded` bytes always come from the
   stack-slot ISel baseline).
3. **`--opt-level=O3` only.** Tests were compiled at O3 (the suite default).
   O0/O1/O2 codegen paths were not exercised.
4. **No `--verify` (IVE).** IVE contract discharge was not enabled for this
   run; the suite's `--verify` flag adds Z3-based verification per test and
   would have multiplied wall-clock ~5×. IVE correctness on the stack-slot
   path is separately covered by the `verification_tests.rs` and
   `ive_loop_tests.rs` Rust unit tests in `tests/`.
5. **Pre-existing release build.** `compile_dump` was built by wave 1
   (`target/release/compile_dump`, 2025-07-30 14:36). No rebuild occurred;
   this verifies the **as-shipped** wave-1 binary, not a fresh build.

## 7. DoD check

- **DoD-1:** At least 4 of the 12 stack-slot backends tested under QEMU. ✅
  **PASS** — all **12 of 12** tested (arm32, armeb, mips64, mips64be, riscv32,
  x86_32, sparc64, s390x, m68k, alpha, hppa, loongarch64).
- **DoD-2:** Each tested backend reports PASS or FAIL with failure summary. ✅
  **PASS** — 12/12 report **PASS** (39/39 tests each, 0 failures). No failure
  summaries needed.
- **DoD-3:** Summary markdown exists at
  `scripts/audit/wave2_stackslot_results.md`. ✅ PASS (this file).
- **Per-backend logs:** `scripts/logs/wave2_stackslot_<isa>.log` (12 files) +
  `wave2_stackslot_combined.log` + `wave2_stackslot_summary.json` +
  `wave2_stackslot_run.log` (full runner transcript) + `wave2_stackslot_runner.py`
  (the runner itself, for reproducibility).

## 8. Conclusion

The 12 stack-slot ISel backends are **correct** on the curated 39-test
representative subset (468/468 executions pass, including the 36 IPC
fork+exec runs). This corroborates caveat §2.1's claim that the stack-slot
path is "correct, but slower" — the correctness half is verified here; the
slower half is acknowledged but not measured (and per 2-a-audit is currently
moot because no backend emits register-based code in production).

**Status: PASS.**
