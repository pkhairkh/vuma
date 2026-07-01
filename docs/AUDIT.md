# VUMA Project Audit Report

**Generated:** 2026-06-28
**Last updated:** 2026-07-02 (corrected against source code)

---

## 1. Source Code Metrics

| Metric | Value |
|---|---|
| Workspace crates | 11 |
| Non-workspace source dirs | 3 (`src/bin/`, `src/lsp/`, `src/bootstrap/`) |
| Rust source files | 205 |
| Rust LOC | ~283,000 |
| Backend architectures | 10 (8 exposed via CLI `emit`/`compile`; all 10 via `compile_dump`) |
| Gold-standard test programs | 5,738 (5,754 total `.vuma` files) |
| Total test runs (programs × backends) | 57,380 |
| Pass rate | 99.99% (57,377/57,380) |
| Failures | 3: `crc32.vuma` (riscv64, ppc64), `s27_fn_two_args_mod.vuma` (ppc64) |

## 2. Workspace Crates (11)

| Crate | Path | LOC | Tests | Responsibility |
|---|---|---|---|---|
| `vuma` (root) | `src/*.rs` | 16,037 | — | Crate root, pipeline, CLI, FFI, diagnostics, LLM API, LSP, logging, telemetry |
| `vuma-scg` | `src/scg/` | 19,217 | 191 | Semantic Computation Graph (petgraph-backed, 26 node types, 7 edge kinds) |
| `vuma-ive` | `src/ive/` | 17,824 | 235 | Inference & Verification Engine (5 invariants; modular.rs not integrated) |
| `vuma-core` | `src/vuma/` | 20,365 | 301 | VUMA Memory Model (MSG, invariants, regions, derivations, access, security, REPL) |
| `vuma-bd` | `src/bd/` | 13,193 | 342 | Behavioral Descriptors (RepD 11, CapD 17, RelD 6) + inference + unify |
| `vuma-codegen` | `src/codegen/` | 105,070 | 1,061 | Multi-ISA code generation (10 backends), regalloc, DWARF v4, ELF |
| `vuma-parser` | `src/parser/` | 18,807 | 325 | Lexer (141 token kinds), parser, AST, AST-to-SCG, resolver, error recovery |
| `vuma-cor` | `src/cor/` | 8,831 | 110 | Continuous Optimization Runtime (partially integrated) |
| `vuma-proof` | `src/proof/` | 9,132 | 102 | Formal proof system (5 invariant proof types) |
| `vuma-std` | `src/std/` | 24,541 | 667 | Standard library (host-side, NOT linked to VUMA programs) |
| `vuma-tests` | `src/tests/` | 25,428 | 459 | Integration tests and benchmarks (8 categories) |
| `vuma-package` | `src/package/` | 1,182 | 6 | Package manager (manifest, resolver, registry) |

## 3. Backend Architectures

10 backends (enum `BackendKind`). Latest full-suite results (`test_results/summary.json`, 2026-07-01 22:05:13 UTC, host `pi-pkhairkh-dev`):

| Backend | ELF Class | Endianness | Pointer Width | Syscall Stubs | Pass Rate |
|---|---|---|---|---|---|
| x86_64 | ELF64 | Little | 64-bit | 26 + 5 helpers | 5738/5738 |
| AArch64 | ELF64 | Little | 64-bit | 21 | 5738/5738 |
| RISC-V 64 | ELF64 | Little | 64-bit | 22 | 5737/5738 |
| ARM32 | ELF32 | Little | 32-bit | 22 | 5738/5738 |
| MIPS64 | ELF64 | **Big** | 64-bit | 21 | 5738/5738 |
| PPC64 | ELF64 | Big (ELFv2) | 64-bit | 21 | 5736/5738 |
| LoongArch64 | ELF64 | Little | 64-bit | 22 | 5738/5738 |
| x86_32 | ELF32 | Little | 32-bit | 21 + 4 helpers | 5738/5738 |
| RISC-V 32 | ELF32 | Little | 32-bit | 22 | 5738/5738 |
| Wasm32 | Wasm | Little | 32-bit | 0 (bump allocator) | 5738/5738 |

**Overall: 57,377/57,380 = 99.99%** (not 100%).

## 4. Test Coverage

| Category | Count |
|---|---|
| Gold-standard programs | 5,738 (5,754 total `.vuma` files in `tests/gold_standard/`) |
| Total runs (× 10 backends) | 57,380 |
| Pass rate | 99.99% (57,377/57,380) |
| Failures | 3 across 2 tests |
| Example programs | 48 (`examples/`) |
| Womb stdlib files | 115 (`womb/`, 64,759 LOC; 114 compilable) |
| Formal specifications | 15 (`docs/specs/`) |

### Failure Details

| Test | Category | Expected | Failing Backends | Actual | Notes |
|---|---|---|---|---|---|
| `crc32.vuma` | crypto_patterns | 38 | riscv64, ppc64 | 170 | CRC32 polynomial mismatch on certain ISAs |
| `s27_fn_two_args_mod.vuma` | functions | 4 | ppc64 | -4 | Signed modulo sign issue on PPC64 |
