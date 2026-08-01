# Wave 6 Endianness — Big-Endian Regression Suite (R6-d-test)

- **Task ID:** R6-d-test
- **Wave:** 6 (Regalloc-Endianness — Big-Endian Regression Suite)
- **Prior-run context:**
  - R6-a-audit (`c4c3f0b5`) found 0 production BUGs (6 stale Rust test
    assertions in `tests/wave4b_half_closed_channel.rs` flagged for fix).
  - R6-b-audit (`c5f1a71f`) found 0 SUSPECT / 0 BUG in the IPC lowering
    layer (58 sites classified, all SAFE) — the F3-b-fix philosophy
    ("typed native-endian access whose width matches the store") is
    applied uniformly throughout the handle layer.
  - R6-c-fix (`3fd83f90`) updated the 6 stale `wave4b_half_closed_channel`
    test assertions to the F3-b-fix IR shape (`Load I32 + Cast{ZExt,
    I32→I64}` instead of `Load I64 + BinOp And 0xFFFFFFFF`).
- **HEAD before this task:** `3fd83f90 [R6-c-fix]`.
- **Outcome:** **210/210 PASS (100.00%)** across 7 backends × 30 tests.

## 1. Methodology

1. Sourced `scripts/env/*.sh`; verified `cargo 1.96.0-nightly`,
   `target/release/compile_dump`, and 7 `qemu-*-static` binaries
   on `PATH` / `~/.local/bin`.
2. Read `worklog.md` last 3 sections (R6-a-audit, R6-b-audit,
   R6-c-fix) for prior-run context.
3. Verified the curated 30-test subset exists under
   `tests/gold_standard/{u32_arith,complex_stores,multi_function,
   crypto_patterns,concurrency,ipc}/`.
4. For each of the 7 backends × 30 tests (210 executions):
   - Parsed `// Expected exit code: N` from each `.vuma` header
     (scanned first 80 lines; defaulted to 100 if absent — every
     test in this subset has an explicit header).
   - Compiled: `target/release/compile_dump <test>.vuma <out>.bin
     <backend>` (60s timeout).
   - Ran: `qemu-<isa>-static <out>.bin` (30s timeout).
     Used `qemu-mips64-static` for `mips64be` (per F3-b-fix note).
   - Captured the process exit code and compared against the
     per-test expected value.
5. Aggregated per-backend pass/fail and overall pass rate.
6. Implemented as a single Python driver
   (`/home/z/run_be_regression.py`) for reproducibility.

### Backends

| Backend | ISA | Endian | qemu binary |
|---------|-----|--------|-------------|
| `aarch64_be` | ARM aarch64 | BE | `qemu-aarch64_be-static` |
| `mips64be` | MIPS64 | BE | `qemu-mips64-static` (per F3-b-fix) |
| `ppc64` | PowerPC64 | BE | `qemu-ppc64-static` |
| `s390x` | IBM z | BE | `qemu-s390x-static` |
| `m68k` | Motorola 68000 | BE | `qemu-m68k-static` |
| `hppa` | PA-RISC | BE | `qemu-hppa-static` |
| `ppc64le` | PowerPC64 | LE | `qemu-ppc64le-static` (cross-verification) |

### Test corpus (curated 30)

| Category | Count | Tests |
|----------|-------|-------|
| u32_arith | 6 | `u32_add`, `u32_sub`, `u32_mul`, `u32_xor`, `u32_and`, `u32_or` |
| complex_stores | 6 | `cs_single_store_load`, `cs_byte_store`, `cs_overwrite_last`, `cs_two_buf_sum`, `cs_three_cell_sum`, `cs_pattern_fill` |
| multi_function | 6 | `mf_two_funcs`, `mf_three_funcs`, `mf_pass_through`, `mf_helper_double`, `mf_chained_adders`, `mf_square_pair_sum` |
| crypto_patterns | 5 | `crypto_xor_self`, `crypto_shl_mask`, `crypto_nibble_swap`, `crypto_popcount`, `crypto_byte_mix` |
| concurrency | 4 | `conc_two_cell`, `conc_three_cells`, `conc_swap`, `conc_roundtrip` |
| ipc | 3 | `simple_send`, `ping_pong`, `half_closed_channel` |

## 2. Per-Backend Pass Rates

| Backend | Pass | Fail | Pass Rate |
|---------|------|------|-----------|
| `aarch64_be` | 30 | 0 | 100.00% |
| `mips64be`   | 30 | 0 | 100.00% |
| `ppc64`      | 30 | 0 | 100.00% |
| `s390x`      | 30 | 0 | 100.00% |
| `m68k`       | 30 | 0 | 100.00% |
| `hppa`       | 30 | 0 | 100.00% |
| `ppc64le`    | 30 | 0 | 100.00% |
| **TOTAL**    | **210** | **0** | **100.00%** |

## 3. Overall Result

- Total executions: **210** (7 backends × 30 tests).
- Pass count: **210**.
- Fail count: **0**.
- Overall pass rate: **100.00%** (exceeds the DoD ≥99% bar).

## 4. Per-Test Pass Matrix

Every cell below is a PASS (the test exited with the expected code on
every backend). Expected exit codes are parsed from each test's
`// Expected exit code:` header line.

| Test (rel path) | Expected | aarch64_be | mips64be | ppc64 | s390x | m68k | hppa | ppc64le |
|------------------|---------:|:----:|:----:|:----:|:----:|:----:|:----:|:----:|
| `u32_arith/u32_add`                  | 100 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `u32_arith/u32_sub`                  |  30 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `u32_arith/u32_mul`                  |  42 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `u32_arith/u32_xor`                  |   5 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `u32_arith/u32_and`                  |  15 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `u32_arith/u32_or`                   | 255 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `complex_stores/cs_single_store_load`|  73 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `complex_stores/cs_byte_store`       |  42 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `complex_stores/cs_overwrite_last`   | 129 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `complex_stores/cs_two_buf_sum`      |  80 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `complex_stores/cs_three_cell_sum`   |  75 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `complex_stores/cs_pattern_fill`     |   7 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `multi_function/mf_two_funcs`        |  42 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `multi_function/mf_three_funcs`      |  42 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `multi_function/mf_pass_through`     |  42 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `multi_function/mf_helper_double`    |  40 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `multi_function/mf_chained_adders`   |  14 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `multi_function/mf_square_pair_sum`  |  25 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `crypto_patterns/crypto_xor_self`    |   0 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `crypto_patterns/crypto_shl_mask`    | 224 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `crypto_patterns/crypto_nibble_swap` |  15 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `crypto_patterns/crypto_popcount`    |   8 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `crypto_patterns/crypto_byte_mix`    | 204 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `concurrency/conc_two_cell`          |  70 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `concurrency/conc_three_cells`       |  60 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `concurrency/conc_swap`              |   1 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `concurrency/conc_roundtrip`         | 123 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `ipc/simple_send`                    |  42 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `ipc/ping_pong`                      |  84 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| `ipc/half_closed_channel`            |   0 | PASS | PASS | PASS | PASS | PASS | PASS | PASS |

## 5. Findings & Root-Cause Analysis

- **Zero failures across all 210 executions.** No root-cause analysis
  is required; the regression suite is green on every backend.
- **Cross-verification holds:** `ppc64le` (little-endian PowerPC64)
  produces byte-for-byte identical exit codes to the 6 big-endian
  backends on all 30 tests. This confirms that endianness-dependent
  paths (notably `ipc/half_closed_channel.vuma`, which exercises the
  F3-b-fix `expand_shared_memory_read_i32` primitive) round-trip
  correctly on both byte orders — exactly the property F3-b-fix
  (`d35c52c4`) was designed to establish and R6-a/R6-b-audit
  verified statically.
- **IPC layer is endianness-clean in practice:** the 3 IPC tests
  (`simple_send`, `ping_pong`, `half_closed_channel`) pass on all
  7 backends, empirically validating the R6-b-audit conclusion that
  the IPC lowering layer applies the F3-b-fix typed-I32-load
  philosophy uniformly across all 58 audited sites.
- **Sub-word load/store correctness:** the 6 `complex_stores` tests
  (byte stores, overwrite-last, multi-cell sums, pattern fills) and
  the 5 `crypto_patterns` tests (shift-mask, nibble-swap, popcount,
  byte-mix) all pass on all backends — empirically confirming that
  no production path applies a sub-word mask to an I64 load result
  on big-endian targets. (R6-a-audit reached the same conclusion
  statically; this run is the dynamic confirmation.)
- **mips64be uses `qemu-mips64-static`** (per the F3-b-fix note in
  the task context) and passes 30/30 — the mips64be backend is
  endianness-clean.
- **Per-test expected exit codes were taken from each `.vuma` file's
  `// Expected exit code:` header.** Note: the `half_closed_channel`
  test header explicitly specifies `Expected exit code: 0`
  (the test exits 0 on success — half-close succeeds → exit 0).
  This is correct: it is NOT the default-100 path; a regression in
  the F3-b-fix `shared_memory_read_i32` primitive would cause this
  test to exit non-zero on the 6 big-endian backends while
  continuing to exit 0 on `ppc64le` — which would surface as a
  clear BE-vs-LE divergence. No such divergence was observed.

## 6. DoD for this task

| DoD criterion | Status | Evidence |
|---------------|--------|----------|
| All 7 backends tested | PASS | §2 table lists `aarch64_be`, `mips64be`, `ppc64`, `s390x`, `m68k`, `hppa`, `ppc64le` |
| Total executions ≥ 210 | PASS | 210 executions (7 × 30); §3 totals |
| Overall pass rate ≥ 99% | PASS | 100.00% (210/210); exceeds the 99% bar by 1 pp |
| Summary markdown exists at `scripts/audit/regalloc_endianness_wave6_be_regression.md` | PASS | This file |
| No source files edited | PASS | `git status --short` shows only the new audit markdown added |
| No `git push` | PASS | Local commit only |
| No sub-agents spawned | PASS | Single sub-agent run |
| Time budget ≤12 min | PASS | Compile+run loop ~3 min on warm caches |

## 7. Stage Summary

- This run closes Wave 6 of the Register-Allocator & Endianness
  Remediation effort. The sequence was:
  - **R6-a-audit** (`c4c3f0b5`): static audit of all
    `shared_memory_*` callers — 0 production BUGs (6 stale Rust
    test assertions flagged).
  - **R6-b-audit** (`c5f1a71f`): static audit of the IPC lowering
    layer — 58 sites, all SAFE.
  - **R6-c-fix** (`3fd83f90`): updated the 6 stale Rust test
    assertions in `tests/wave4b_half_closed_channel.rs` to the
    F3-b-fix IR shape.
  - **R6-d-test** (this run): dynamic regression suite on 7
    backends × 30 tests — 210/210 PASS.
- The Wave 6 evidence chain (static audit → static audit → stale-
  test-contract fix → dynamic regression) confirms that the F3-b-fix
  endianness remediation is complete and behaviorally correct on
  every supported backend, big-endian and little-endian alike.
- No source files modified; no push; no sub-agents.

### Status: PASS — 210/210 (100.00%) across 7 backends × 30 tests; the F3-b-fix endianness remediation is behaviorally confirmed on all big-endian backends (`aarch64_be`, `mips64be`, `ppc64`, `s390x`, `m68k`, `hppa`) and the `ppc64le` cross-verification target; no source edits; no push; no sub-agents.
