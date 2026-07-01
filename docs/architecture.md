# VUMA Architecture

**Version:** 0.2.0-alpha.1

---

## Overview

VUMA (Verified-Unsafe Memory Access) is a programming language framework where unsafe memory operations are made verifiable instead of forbidden. The compiler constructs a formal model (MSG) of every memory operation and verifies global invariants. Programs that pass verification run without runtime overhead; programs that fail receive counterexamples.

**Current state:** The verification engine has false positives on some valid programs. Most compilation uses `--verification none`.

## System Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                    CLI / API Layer                                    │
│    vuma build/emit/run/verify · VumaForLLM API · LSP · REPL        │
├─────────────────────────────────────────────────────────────────────┤
│                    Parser / Frontend                                 │
│    Lexer → Parser → AST → AST-to-SCG Lowering · Module Resolution  │
├─────────────────────────────────────────────────────────────────────┤
│             Reasoning Core                                           │
│    IVE (Inference + Verification) · BD (Descriptors) · MSG (Memory) │
│    Invariants: Liveness · Exclusivity · Interpretation ·             │
│                Origin · Cleanup                                      │
│    Note: IVE has false positives; most programs use --verification   │
│    none to bypass.                                                   │
├─────────────────────────────────────────────────────────────────────┤
│                    SCG (Core Representation)                         │
│    Nodes (ops, allocs, effects) · Edges (data flow, deps) · Regions │
├─────────────────────────────────────────────────────────────────────┤
│                    Execution                                         │
│    COR Runtime (partially integrated) · Multi-ISA Codegen            │
│    x86_64 · AArch64 · RISC-V 64/32 · ARM32 · MIPS64 · PPC64        │
│    LoongArch64 · x86_32 · Wasm32                                    │
│    All 10 backends at 100% gold-standard pass rate                   │
│    (with --verification none)                                        │
└─────────────────────────────────────────────────────────────────────┘
```

## Pipeline

```
Source Text → Lexer → Parser → AST → SCG Lowering → Raw SCG
    → Module Resolution (imports) → Merged SCG
    → [Optional: BD Inference → MSG Builder → VUMA Verification]
    → Multi-Arch Codegen (IR → regalloc → emit) → Machine Code / Wasm
```

The verification step is optional. When `--verification none` is used (the common case), the pipeline skips BD inference and MSG verification, going directly from SCG to codegen.

## Workspace Crates (11 members)

| Crate | Role | Test Status |
|-------|------|-------------|
| `vuma-scg` | Semantic Computation Graph (petgraph-backed) | 36/36 pass |
| `vuma-bd` | Behavioral Descriptors (RepD, CapD, RelD) | Tests pass |
| `vuma` (core) | MSG, Region, Derivation, Access, Invariants | 301/301 pass |
| `vuma-ive` | Inference & Verification Engine | Tests pass (false positives on valid programs) |
| `vuma-cor` | Continuous Optimization Runtime | Partially integrated |
| `vuma-parser` | Lexer → AST → SCG bridge | 286/286 pass |
| `vuma-codegen` | 10-ISA backend | 57,380/57,380 gold-standard pass |
| `vuma-proof` | Formal proof system | Tests pass |
| `vuma-std` | Rust stdlib wrapper (NOT linked to VUMA programs) | N/A |
| `vuma-package` | Package manager | Basic functionality |
| `vuma-tests` | Integration tests & benchmarks | Tests pass |

## Key Concepts

### SCG — Semantic Computation Graph
The SCG is the primary program representation. Nodes represent operations (allocations, accesses, computations, casts, effects, control flow), edges represent relationships (data flow, control flow, derivation), and regions delineate scopes.

### BD — Behavioral Descriptor
A Behavioral Descriptor replaces traditional nominal types with the triple (RepD, CapD, RelD):
- **RepD**: memory layout (size, alignment, field offsets)
- **CapD**: permitted operations (read, write, execute, etc.)
- **RelD**: relationships (temporal, structural, dependency)

BDs are inferred from SCG structure through iterative fixpoint computation. **Note:** Complex generic inference (M2.3) is deferred.

### MSG — Memory State Graph
The MSG captures every allocation, pointer derivation, deallocation, and access. It is constructed from the annotated SCG and verified against the five invariants. **Note:** The MSG builder handles cycles via SCC-based topological sort, but may produce false positives on some valid programs.

### IVE — Inference & Verification Engine
The IVE reads the SCG, infers BDs, constructs the MSG, and verifies the five invariants. It supports interprocedural analysis, escape analysis, and verification caching. A modular verification infrastructure exists (`src/ive/src/modular.rs`) with per-function analysis, incremental caching, and abstract region tracking, but is not yet integrated into the main pipeline.
