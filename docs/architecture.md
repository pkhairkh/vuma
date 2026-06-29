# VUMA Architecture

**Version:** 0.1.0-alpha.1

---

## Overview

VUMA (Verified-Unsafe Memory Access) is a programming language framework where unsafe memory operations are made verifiable instead of forbidden. Instead of a borrow checker rejecting programs that cannot be statically proven safe, VUMA constructs a formal model of every memory operation and verifies global invariants against that model. Programs that pass verification run without runtime overhead; programs that fail receive counterexamples showing the execution path to the violation.

The SCG (Semantic Computation Graph) is the primary program representation. Nodes represent operations, edges represent relationships, and regions delineate scopes.

---

## System Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                    LLM Integration Layer                             │
│    VumaForLLM API · LSP Server · REPL · Structured Diagnostics     │
├─────────────────────────────────────────────────────────────────────┤
│                    Parser / Frontend                                 │
│    Lexer → Parser → AST → AST-to-SCG Lowering · Module Resolution  │
├─────────────────────────────────────────────────────────────────────┤
│             Reasoning Core                                           │
│    IVE (Inference + Verification) · BD (Descriptors) · MSG (Memory) │
│    Invariants: Liveness · Exclusivity · Interpretation ·             │
│                Origin · Cleanup                                      │
├─────────────────────────────────────────────────────────────────────┤
│                    SCG (Core Representation)                         │
│    Nodes (ops, allocs, effects) · Edges (data flow, deps) · Regions │
├─────────────────────────────────────────────────────────────────────┤
│                    Execution                                         │
│    COR Runtime (always-compiled, PGO, JIT) · Multi-ISA Codegen      │
│    x86_64 · AArch64 · RISC-V 64/32 · ARM32 · MIPS64 · PPC64        │
│    LoongArch64 · x86_32 · Wasm32                                    │
└─────────────────────────────────────────────────────────────────────┘
```

### Pipeline

```
Source Text → Lexer → Parser → AST → SCG Lowering → Raw SCG
    → Module Resolution (imports) → Merged SCG
    → BD Inference (RepD + CapD + RelD fixpoint) → Annotated SCG
    → MSG Builder → VUMA Verification (5 invariants) → Verified SCG
    → Multi-Arch Codegen (IR → regalloc → emit) → Machine Code / Wasm
```

---

## Workspace Crates

The workspace has 11 member crates:

| Crate | Path | Responsibility |
|-------|------|----------------|
| `vuma-scg` | `src/scg/` | Semantic Computation Graph — nodes, edges, regions, queries, dominance, liveness, transforms |
| `vuma-ive` | `src/ive/` | Inference & Verification Engine — five invariant verifiers, counterexamples |
| `vuma-core` | `src/vuma/` | VUMA Memory Model — MSG construction, invariants, incremental verification |
| `vuma-bd` | `src/bd/` | Behavioral Descriptors — RepD, CapD, RelD, inference, unification |
| `vuma-codegen` | `src/codegen/` | Multi-ISA code generation — 10 backends, regalloc, DWARF, FFI |
| `vuma-parser` | `src/parser/` | Lexer, parser, AST, AST-to-SCG lowering, module resolution |
| `vuma-cor` | `src/cor/` | Continuous Optimization Runtime — PGO, speculative optimization, deployment |
| `vuma-proof` | `src/proof/` | Formal proof system — proofs, checker, tactics, counterexamples |
| `vuma-std` | `src/std/` | Standard library (Rust crate, not yet linked to VUMA programs) |
| `vuma-tests` | `src/tests/` | Integration tests and benchmarks |
| `vuma-package` | `src/package/` | Package manager — manifest, resolver, registry |

### Crate Dependency Graph

```
                    ┌──────────┐
                    │  tests   │  (depends on everything)
                    └────┬─────┘
           ┌─────────────┼──────────────────┐
           ▼             ▼                  ▼
    ┌────────────┐ ┌──────────┐      ┌──────────┐
    │   cor      │ │   std    │      │  codegen │
    └─────┬──────┘ └────┬─────┘      └─────┬────┘
          │              │                   │
          ▼              ▼                   ▼
    ┌──────────┐  ┌──────────┐  ┌──────────────────┐
    │  (cor)   │  │  vuma    │  │  ive · proof     │
    └────┬─────┘  └────┬─────┘  └────┬─────────────┘
         │             │              │
         └─────────────┼──────────────┘
                       ▼
                ┌──────────────┐
                │  bd · parser │
                └──────┬───────┘
                       ▼
                ┌──────────────┐
                │     scg      │  ◄── foundation (zero workspace deps)
                └──────────────┘
```

---

## Core Concepts

### SCG — Semantic Computation Graph

The SCG is the single source of truth. Nodes represent computational operations (allocations, accesses, computations, casts, effects, control flow), edges represent relationships (data flow, control flow, derivation, annotation), and regions delineate scopes.

### BD — Behavioral Descriptor

A Behavioral Descriptor replaces nominal types with the triple (RepD, CapD, RelD):

- **RepD** (Representation Descriptor): memory layout — size, alignment, field offsets
- **CapD** (Capability Descriptor): permitted operations — read, write, execute, etc.
- **RelD** (Relational Descriptor): relationships — containment, aliasing, data flow

BDs are inferred, not declared. The IVE derives them from SCG structure through iterative fixpoint computation.

### MSG — Memory State Graph

The MSG captures every allocation point, pointer derivation, deallocation point, concurrent access, and reinterpretation. It is constructed from the annotated SCG and serves as the formal model for verification.

### IVE — Inference & Verification Engine

The IVE reads the SCG, infers BDs, constructs the MSG, and verifies five global invariants through iterative fixpoint computation. It supports interprocedural analysis, escape analysis, verification caching, and incremental re-verification.

### The Five VUMA Invariants

| Invariant | Ensures |
|-----------|---------|
| **Liveness** | Every access targets allocated memory |
| **Exclusivity** | No conflicting concurrent accesses |
| **Interpretation** | Every access uses a valid representation |
| **Origin** | Every address traces to a valid allocation |
| **Cleanup** | Every region is eventually freed or explicitly leaked |

---

## Code Generation

### 10 Backend Architectures

| Backend | ELF Class | Endianness | Pointer Width | Calling Convention |
|---------|-----------|------------|---------------|-------------------|
| x86_64 | ELF64 | Little | 64-bit | System V AMD64 |
| AArch64 | ELF64 | Little | 64-bit | AAPCS64 |
| RISC-V 64 | ELF64 | Little | 64-bit | RV64G LP64D |
| ARM32 | ELF32 | Little | 32-bit | AAPCS |
| MIPS64 | ELF64 | Little | 64-bit | N64 |
| PPC64 | ELF64 | Big | 64-bit | ELFv2 |
| LoongArch64 | ELF64 | Little | 64-bit | LP64 |
| x86_32 | ELF32 | Little | 32-bit | cdecl |
| RISC-V 32 | ELF32 | Little | 32-bit | RV32G ILP32 |
| Wasm32 | Wasm | Little | 32-bit | Stack machine |

All backends share a unified `Backend` trait. The codegen pipeline:

```
SCG → IR (target-independent) → Register Allocation → Instruction Selection → Binary Emission
```

All 10 backends pass the 5,738-program gold-standard test suite at 100% (57,380/57,380 runs).

### DWARF v4 Debug Info

Per-backend DWARF debug information: `.debug_abbrev`, `.debug_info`, `.debug_line`, `.debug_frame`.

### FFI & Syscalls

19 Linux syscalls across all 10 architectures. Architecture-specific relocations. `extern "C"` blocks with `is_extern` propagation through IR and codegen.

---

## LLM Integration

### VumaForLLM API (`src/llm_api.rs`)

Stateless API for LLM agents: `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()`, `targets()`.

### VumaCompiler API (`src/api.rs`)

Full pipeline API: `compile()`, `parse()`, `analyze()`, `validate()`, `verify()`.

### LSP Server (`src/lsp/`)

Full LSP protocol: diagnostics, hover, go-to-definition, completion, document symbols, semantic tokens.

### REPL (`src/vuma/src/repl.rs`)

Commands: `:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`, `:verify`, `:help`.

### Structured Diagnostics (`src/diagnostics.rs`)

66 diagnostic codes (E000–E050, W001–W010, I001–I005) with error chaining and JSON serialization.

---

## Security Model

VUMA addresses six categories of memory safety vulnerabilities:

1. **Spatial memory errors** (buffer overflow, out-of-bounds) — addressed by liveness and interpretation invariants
2. **Temporal memory errors** (use-after-free, double-free) — addressed by liveness and cleanup invariants
3. **Type confusion** (reading integer as pointer, uninitialized reads) — addressed by interpretation invariant
4. **Resource exhaustion** (memory leaks, fd exhaustion) — addressed by cleanup invariant
5. **Concurrent access violations** (data races, deadlock) — addressed by exclusivity invariant (single-threaded currently)
6. **Supply chain attacks** — partially addressed by origin verification

### Verification Confidence

`VerificationLevel` tiers: `Full` (formal proof), `Partial` (most cases), `BestEffort` (empirical). Unverified properties are tracked as `VerificationDebt` with priorities: `Critical`, `High`, `Medium`, `Low`.
