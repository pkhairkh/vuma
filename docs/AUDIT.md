# VUMA Project Audit Report

**Generated:** 2026-06-28

---

## 1. Source Code Metrics

| Metric | Value |
|---|---|
| Workspace crates | 11 |
| Backend architectures | 10 |
| Gold-standard test programs | 5,738 |
| Total test runs (programs × backends) | 57,380 |
| Pass rate | 100% |

## 2. Workspace Crates

| Crate | Path | Responsibility |
|---|---|---|
| `vuma` (root) | `src/` | Crate root, pipeline, CLI, FFI, diagnostics, LLM API |
| `vuma-scg` | `src/scg/` | Semantic Computation Graph |
| `vuma-ive` | `src/ive/` | Inference & Verification Engine |
| `vuma-core` | `src/vuma/` | VUMA Memory Model (MSG, invariants) |
| `vuma-bd` | `src/bd/` | Behavioral Descriptors |
| `vuma-codegen` | `src/codegen/` | Multi-ISA code generation (10 backends) |
| `vuma-parser` | `src/parser/` | Lexer, parser, AST, AST-to-SCG |
| `vuma-cor` | `src/cor/` | Continuous Optimization Runtime |
| `vuma-proof` | `src/proof/` | Formal proof system |
| `vuma-std` | `src/std/` | Standard library (host-side) |
| `vuma-tests` | `src/tests/` | Integration tests and benchmarks |
| `vuma-package` | `src/package/` | Package manager |

## 3. Backend Architectures

All 10 backends pass the 5,738-program gold-standard test suite at 100% (57,380/57,380 runs):

| Backend | ELF Class | Endianness | Pointer Width |
|---|---|---|---|
| x86_64 | ELF64 | Little | 64-bit |
| AArch64 | ELF64 | Little | 64-bit |
| RISC-V 64 | ELF64 | Little | 64-bit |
| ARM32 | ELF32 | Little | 32-bit |
| MIPS64 | ELF64 | Little | 64-bit |
| PPC64 | ELF64 | Big | 64-bit |
| LoongArch64 | ELF64 | Little | 64-bit |
| x86_32 | ELF32 | Little | 32-bit |
| RISC-V 32 | ELF32 | Little | 32-bit |
| Wasm32 | Wasm | Little | 32-bit |

## 4. Test Coverage

| Category | Count |
|---|---|
| Gold-standard programs | 5,738 |
| Total runs (× 10 backends) | 57,380 |
| Pass rate | 100% |
| Example programs | 48 |
| Formal specifications | 15 |
