# tests/ — VUMA Test Suite

The `tests/` directory contains VUMA's test infrastructure: a
**manifest-driven gold-standard suite of 5,832+ `.vuma` programs** that
compiles + executes on every VUMA backend and checks the exit code against
an `// Expected exit code: N` header, plus 13 Rust integration test files
that exercise the compiler internals (parser, scheduler, e-graph, register
allocator, loop unroller, etc.).

This README is the entry point for the test suite. For the runner-script
reference see [`docs/building.md` §4 Cross-Backend Testing](../docs/building.md#4-cross-backend-testing).
For the contributor workflow (adding a new test) see
[`docs/contributing.md` §5](../docs/contributing.md#5-adding-a-new-test).

---

## What's here

```
tests/
├── gold_standard/        # 5,832+ manifest-driven PMT test programs
│   ├── arithmetic/       # 377 files — integer arithmetic
│   ├── atomics/          # 338 files — atomic RMW patterns
│   ├── bitwise/          # 359 files — AND/OR/XOR/shifts
│   ├── complex_stores/   # 348 files — multi-cell stores
│   ├── concurrency/      # 338 files — multi-state interaction
│   ├── control_flow/     # 445 files — if/else/while/for/break/continue
│   ├── crypto_patterns/  # 345 files — AES/SHA/HMAC/ChaCha rounds
│   ├── edge_cases/       # 345 files — overflow, div-by-zero, sign edges
│   ├── functions/        # 351 files — 0-to-4 arg calls, recursion
│   ├── linked_structures/# 335 files — PMT-linked lists, trees
│   ├── memory/           # 377 files — buffer allocation, reuse
│   ├── multi_function/   # 338 files — cross-function data flow
│   ├── nested_loops/     # 414 files — 2-/3-deep loop nests
│   ├── pointers/         # 350 files — PMT-migrated pointer programs
│   ├── structs/          # 349 files — layout field access
│   ├── u32_arith/        # 345 files — 32-bit unsigned arithmetic
│   ├── pmt_wave1/        # 5 files — basic layout/state_new
│   ├── pmt_wave2/        # 16 files — multi-state, multi-field
│   ├── pmt_wave3_negative/ # 5 files — type checker rejects
│   ├── pmt_wave5/        # 5 files — state lifetimes, buffer reuse
│   ├── pmt_wave7/        # 5 files — field swaps, copies, transforms
│   ├── pmt_wave8/        # 3 files — buffer sizing
│   ├── pmt_wave9/        # 5 files — advanced PMT patterns
│   ├── pmt_wave10/       # 3 files — final PMT conformance
│   ├── arena_wave0/      # 3 files — arena builtin
│   ├── arena_wave1/      # 4 files — arena overflow regression
│   ├── arena_wave2/      # 1 file  — arena multiple + grow
│   ├── ffi_wave0/        # 9 files — FFI marshal borrow
│   ├── ffi_wave1/        # 3 files — FFI marshal modes
│   ├── ffi_wave2/        # 4 files — marshal scratch
│   ├── ffi_wave3/        # 4 files — foreign state
│   ├── ffi_wave4/        # 2 files — callbacks
│   ├── kernel_boot/      # 1 fixture (expected output)
│   └── kernel_crypto/    # 1 file — SHA-256 KAT
├── backend_latency_tests.rs
├── egraph_extraction_tests.rs
├── ive_loop_tests.rs
├── latency_table_tests.rs
├── loop_depth_tests.rs
├── loop_unroll_tests.rs
├── lto_tests.rs
├── parallel_codegen_tests.rs
├── pgo_tests.rs
├── property_tests.rs
├── provenance_tests.rs
├── scheduler_tests.rs
└── verification_tests.rs
```

Total: **5,832+ `.vuma` test programs** across 34 subdirectories, plus
**13 Rust integration test files** (~3,400 LOC).

---

## Gold-standard tests

The gold-standard suite is the canonical regression gate for the compiler.
Every `.vuma` file:

1. Carries an `// Expected exit code: N` header that the runner parses.
2. Is compiled by `compile_dump` for every backend in the sweep.
3. Is executed under QEMU user-mode (or natively on x86_64, or under
   `wasmtime` for `wasm32`).
4. Has its actual exit code compared against the expected value.

### Feature categories (16 directories)

The feature categories exercise general VUMA language features. They are
NOT PMT-specific — many were originally written in the legacy pointer
dialect and have been migrated to PMT.

| Directory | Files | Focus |
|-----------|-------|-------|
| `arithmetic/` | 377 | Integer arithmetic (add/sub/mul/div, overflow, mixed widths) |
| `atomics/` | 338 | Atomic read-modify-write patterns |
| `bitwise/` | 359 | AND/OR/XOR/shifts, bit extraction |
| `complex_stores/` | 348 | Multi-cell stores, overwrites, scatter/gather |
| `concurrency/` | 338 | Multi-state interaction patterns |
| `control_flow/` | 445 | Branches, loops, early returns |
| `crypto_patterns/` | 345 | Reduced-step crypto primitives (AES, SHA, ChaCha rounds) |
| `edge_cases/` | 345 | Boundary values (0, MAX, MIN), empty functions, alloc edges |
| `functions/` | 351 | Single-function call/return semantics |
| `linked_structures/` | 335 | State-based linked lists, trees |
| `memory/` | 377 | Buffer allocation, reuse, lifetime |
| `multi_function/` | 338 | Multi-function programs, cross-function data flow |
| `nested_loops/` | 414 | Loop nesting, induction-variable correctness |
| `pointers/` | 350 | PMT-translated pointer programs (state-as-pointer) |
| `structs/` | 349 | Multi-field layouts, field access patterns |
| `u32_arith/` | 345 | 32-bit unsigned arithmetic stress |

### PMT wave directories (8 directories)

These exercise PMT-specific features: `layout`, `State<T>`, `state_new`,
`state.field`, transforms, `#[borrow]` externs, FFI marshal modes.

| Directory | Files | Focus |
|-----------|-------|-------|
| `pmt_wave1/` | 5 | Basic `layout` / `state_new` / single-field access |
| `pmt_wave2/` | 16 | Multiple states, multi-field layouts, u32/i64 fields |
| `pmt_wave3_negative/` | 5 | Negative tests — programs the type checker must reject |
| `pmt_wave5/` | 5 | State lifetimes, buffer reuse |
| `pmt_wave7/` | 5 | Field swaps, copies, linked states, accumulators |
| `pmt_wave8/` | 3 | Buffer sizing (single, large, reuse) |
| `pmt_wave9/` | 5 | Advanced PMT patterns |
| `pmt_wave10/` | 3 | Final PMT conformance wave |

### Arena + FFI + kernel wave directories

| Directory | Files | Focus |
|-----------|-------|-------|
| `arena_wave0/` | 3 | K0 arena-builtin tests (arena_alloc parse) |
| `arena_wave1/` | 4 | K0 arena-overflow regression (arena_basic, arena_grow, arena_multiple, arena_overflow) |
| `arena_wave2/` | 1 | K0 arena-multiple + grow tests |
| `ffi_wave0/` | 9 | FFI marshal: borrow modes |
| `ffi_wave1/` | 3 | FFI marshal modes |
| `ffi_wave2/` | 4 | FFI marshal scratch |
| `ffi_wave3/` | 4 | FFI foreign state |
| `ffi_wave4/` | 2 | FFI callbacks |
| `kernel_boot/` | 1 | The `kernel.vuma` smoke test (expected-output fixture, no `.vuma` file) |
| `kernel_crypto/` | 1 | SHA-256 KAT test for the kernel crypto subsystem |

---

## How tests work

Every `.vuma` test file begins with the standard header:

```
// <name> — <one-line description>
// Expected exit code: <N>
//
// <longer description / what this tests>
//
// VUMA Key Concepts:
//   - <bullet list of PMT features exercised>
```

The runner parses the `Expected exit code:` line, compiles the file with
`compile_dump`, executes the resulting binary, and compares the actual
process exit code against the expected value. A `skip_on: wasm32, ppc64`
header marks tests that exercise architecturally-unavailable functionality
(e.g. `fork` on wasm32) — those tests are skipped on the listed backends
rather than failing.

The runner is the test harness — there is no `#[test]` attribute or
`cargo test` invocation for gold-standard tests. The runner is invoked via
shell script (see [Test runner scripts](#test-runner-scripts) below).

---

## Test runner scripts

### `scripts/pi5_test_suite.sh` — the canonical gold-standard sweep

The end-to-end cross-backend runner. Builds `compile_dump`, walks
`tests/gold_standard/`, compiles every `.vuma` file on every backend,
executes each under QEMU / wasmtime, and checks the exit code against the
`// Expected exit code: N` header.

```bash
scripts/pi5_test_suite.sh --workers 8 --fresh --verify
```

Flags: `--workers N` (parallel compile+run workers, default 4), `--fresh`
(force a from-scratch cargo build), `--verify` (run IVE on each program),
`--skip-build`, `--backends LIST`, `--release`, `--profile dev`,
`--no-push`.

### `scripts/kernel_smoke.sh` — kernel boot smoke test (single-arch)

Compiles `womb/kernel/kernel.vuma` for `x86_64` with `--verify`, runs the
ELF as a regular Linux process, greps stdout for `vuma kernel: hello`, and
checks exit code 0. The minimum bar every commit must clear.

```bash
bash scripts/kernel_smoke.sh
# Expected: "PASS: kernel boots, prints banner, exits 0"
```

### `scripts/kernel_parity.sh` — 19-backend kernel parity sweep

Compiles + runs `kernel.vuma` and 10 gold-standard tests across **all 19
backends** (190 compile+execute checks), and compile-verifies 19 kernel
modules on 4 backends (76 module compiles). Total: 266 backend
compilations per invocation. Exits 0 only if every backend passes.

```bash
bash scripts/kernel_parity.sh            # full sweep (~10 minutes)
bash scripts/kernel_parity.sh --quick    # arena_basic + kernel smoke only
```

### Other runner scripts

| Script | Scope |
|--------|-------|
| [`scripts/run_all_gold.sh`](../scripts/run_all_gold.sh) | Run all gold-standard tests on x86_64 (fast inner loop) |
| [`scripts/cross_backend_test.sh`](../scripts/cross_backend_test.sh) | Cross-backend agreement sweep (compares exit codes across backends) |
| [`scripts/run_gold_sweep.py`](../scripts/run_gold_sweep.py) | Python gold-sweep with parallel workers |
| [`scripts/run_backend_resilient.py`](../scripts/run_backend_resilient.py) | Resilient runner that retries failed backends |
| [`scripts/supervisor.py`](../scripts/supervisor.py) | Long-running test supervisor |
| [`scripts/ci_run_tests.sh`](../scripts/ci_run_tests.sh) | CI entry point |
| [`scripts/run_fuzz.sh`](../scripts/run_fuzz.sh) | Fuzz driver harness |
| [`scripts/supervisor_3par.sh`](../scripts/supervisor_3par.sh) | 3-parallel supervisor |
| [`scripts/run_8backends.py`](../scripts/run_8backends.py) | 8-backend parallel runner |
| [`scripts/run_one_batch.py`](../scripts/run_one_batch.py) | Single-batch runner |
| [`scripts/add_expected_codes.py`](../scripts/add_expected_codes.py) | Auto-fill `// Expected exit code:` headers by running tests |

See [`docs/building.md` §4](../docs/building.md#4-cross-backend-testing)
for the full runner-script reference.

---

## Integration tests (13 `.rs` files)

The 13 Rust integration test files in `tests/` (3,400 LOC total) exercise
the compiler internals. Run them with `cargo test --workspace` or
`cargo test -p vuma-tests`.

| File | Focus |
|------|-------|
| `backend_latency_tests.rs` | Backend latency table consistency |
| `egraph_extraction_tests.rs` | E-graph equality saturation extraction |
| `ive_loop_tests.rs` | IVE state verifiers on loop-heavy programs |
| `latency_table_tests.rs` | Latency table correctness |
| `loop_depth_tests.rs` | Loop-nest depth analysis |
| `loop_unroll_tests.rs` | Loop unroller correctness |
| `lto_tests.rs` | LTO (fat vs thin) codegen soundness |
| `parallel_codegen_tests.rs` | Parallel codegen (multi-thread) soundness |
| `pgo_tests.rs` | Profile-guided optimization |
| `property_tests.rs` | Property-based testing of parser + codegen |
| `provenance_tests.rs` | Pointer provenance tracking |
| `scheduler_tests.rs` | Instruction scheduler correctness |
| `verification_tests.rs` | IVE verifier end-to-end |

The `src/tests/` crate (`vuma-tests`) contains the integration test
framework (`framework.rs`, `full_pipeline.rs`, `cross_backend.rs`,
`codegen.rs`, `abi_conformance.rs`, `elf_validation.rs`,
`execution_validation.rs`, `ffi_types.rs`, `property_tests.rs`,
`regression.rs`, `sha256d.rs`, `sha256d_backends.rs`,
`parser_roundtrip.rs`, `bd_inference.rs`, `dwarf_ffi_integration.rs`,
`wasm_validation.rs`, `concurrent.rs`, `dlist.rs`, `graph.rs`,
`e2e_cor.rs`, `final_integration.rs`, `diagnostics_integration.rs`,
`wave47_bootstrap.rs`, `wave48_bootstrap.rs`, `wave48_self_host.rs`,
`wave50.rs`, `trivial.rs`).

---

## KAT tests

Known-answer tests for crypto algorithms live in two directories:

| Directory | Files | Scope |
|-----------|-------|-------|
| [`scripts/womb_kat_tests/`](../scripts/womb_kat_tests/) | 86 | Womb-library KAT tests — every algorithm in `womb/crypto/` + `womb/lib/` has at least one |
| [`scripts/real_kat_tests/`](../scripts/real_kat_tests/) | 127 | Real cross-architecture KAT tests — known-answer vectors verified across multiple backends |

Each KAT test is a `.vuma` program that computes a hash, ciphertext, or
signature and checks it against a known value. Algorithm coverage:
SHA-256, AES-128/192/256, Ed25519, ECDSA P-256/P-384, ML-DSA, ML-KEM,
SLH-DSA, Falcon, HQC, X25519, ChaCha20, Poly1305, Argon2, scrypt, HKDF,
HMAC, RSA-OAEP-PSS, BLAKE2/3, SHA-3/SHAKE, and more.

Run them with:

```bash
bash scripts/run_all_kat.sh        # womb KAT tests
bash scripts/run_real_kat.sh       # real cross-arch KAT suite
```

The test generators
[`scripts/gen_real_kat.py`](../scripts/gen_real_kat.py) and
[`scripts/generate_all_kat_tests.py`](../scripts/generate_all_kat_tests.py)
auto-generate the KAT test files from NIST CAVP / RFC test vectors.

---

## How to add a new test

The full guide is in [`docs/contributing.md` §5](../docs/contributing.md#5-adding-a-new-test).
The short version:

1. **Pick a category** under `tests/gold_standard/` — pick the most
   specific one (arithmetic, atomics, bitwise, control_flow, functions,
   structs, …). If the test exercises a PMT-specific feature, use the
   matching `pmt_wave*` directory. If it exercises the arena runtime, use
   `arena_wave*`. If it exercises the FFI marshal matrix, use `ffi_wave*`.
2. **Write the `.vuma` file** with the standard header (`// <name> —
   <description>`, `// Expected exit code: <N>`, longer description, key
   concepts). The body is PMT-only — see
   [`docs/contributing.md` §3 PMT-Only Test Policy](../docs/contributing.md#3-pmt-only-test-policy).
3. **Verify locally** — build `compile_dump` and run the test under
   `x86_64` (native, fastest) plus at least one cross-backend (e.g.
   `aarch64` via QEMU). The exit code must match the header on **every**
   backend.
4. **Run the full category** with `scripts/pi5_test_suite.sh --workers 8
   --backends x86_64,aarch64,riscv64 --verify`.

---

## See also

- [`docs/building.md`](../docs/building.md) — build reference, runner
  flags, profile selection, constrained-memory workaround.
- [`docs/contributing.md`](../docs/contributing.md) — adding a new test,
  PMT-only test policy, code patterns.
- [`womb/kernel/README.md`](../womb/kernel/README.md) — kernel-side test
  layers (per-module self-tests, boot smoke, parity sweep).
- [`womb/crypto/README.md`](../womb/crypto/README.md) — KAT coverage
  matrix for the crypto library.
- [`src/README.md`](../src/README.md) — the `vuma-tests` crate source.
