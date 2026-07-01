# VUMA Gold-Standard Test Suite

A categorized collection of `.vuma` programs that serve as the gold standard for testing the VUMA compiler across all 10 supported backends.

## Quick Facts

- **Total programs:** 5,738 (with expected exit codes) across 16 categories
- **Total .vuma files:** 5,754 (some lack expected exit codes)
- **Test runs:** 5,738 programs × 10 backends = 57,380 runs
- **Pass rate:** 100% (57,380/57,380) with `--verification none`
- **Test runner:** `scripts/pi5_test_suite.sh` (uses QEMU for cross-architecture, Wasmtime for Wasm32)
- **Compilation:** Uses `compile_dump` binary (canonical SCG pipeline, `--verification none`)

## Categories

| # | Category | Programs | Title |
|---|----------|---------:|-------|
| 1 | `arithmetic/` | ~377 | Basic Integer Arithmetic |
| 2 | `bitwise/` | ~350 | Bitwise Operations |
| 3 | `memory/` | ~377 | Memory Allocation, Load, and Store |
| 4 | `control_flow/` | ~350 | Control Flow: if/else, while, for, break, continue |
| 5 | `pointers/` | ~350 | Pointer Arithmetic and Dereference |
| 6 | `functions/` | ~350 | Function Calls, Recursion, and Parameters |
| 7 | `structs/` | ~349 | Struct and Enum (Tagged Union) Types |
| 8 | `atomics/` | ~350 | Atomic Operations and Memory Ordering |
| 9 | `u32_arith/` | ~350 | 32-bit Arithmetic with Overflow Masking |
| 10 | `edge_cases/` | ~350 | Boundary Conditions and Unusual Features |
| 11 | `multi_function/` | ~350 | Many Functions Calling Each Other |
| 12 | `complex_stores/` | ~348 | Multi-byte Stores and Computed Addresses |
| 13 | `nested_loops/` | ~350 | Two- and Three-Level Nested Loops |
| 14 | `linked_structures/` | ~335 | Linked Lists, Trees, and Ring Buffers |
| 15 | `crypto_patterns/` | ~350 | Cryptographic Hash and Checksum Patterns |
| 16 | `concurrency/` | ~350 | Lock-Free Structures, Atomics, and Channels |

Each program has a header comment with `// Expected exit code: N` so the test runner can verify both "did it crash?" and "did it return the right value?".

## Running Tests

```bash
# Full suite (requires QEMU + Wasmtime, runs on Pi 5)
bash scripts/pi5_test_suite.sh --workers 4 --fresh

# The test runner:
# 1. Compiles each .vuma file with compile_dump to each backend
# 2. Runs the resulting binary under QEMU (or Wasmtime for wasm32)
# 3. Compares the exit code with the expected value
# 4. Reports pass/fail per backend
```

## Important Notes

- All tests use `--verification none` (the IVE has false positives on some valid programs)
- The `compile_dump` binary uses the canonical SCG pipeline (`bridge_scg_to_codegen`)
- Wasm32 tests use `wasmtime run --invoke _vuma_main` (or `wasmtime run` for print tests)
- The test runner does NOT modify any source files or the compiler
