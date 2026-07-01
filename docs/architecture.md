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
│    vuma build/emit/run/verify/check/compile/disasm/repl · LSP ·     │
│    VumaForLLM API · VumaCompiler API                                 │
├─────────────────────────────────────────────────────────────────────┤
│                    Parser / Frontend                                 │
│    Lexer (141 token kinds) → Parser → AST → AST-to-SCG Lowering ·   │
│    Module Resolution (with circular import detection) · Error        │
│    Recovery (Diagnostic, ErrorCollector, levenshtein suggesters)     │
├─────────────────────────────────────────────────────────────────────┤
│             Reasoning Core                                           │
│    IVE (Inference + Verification) · BD (Descriptors) · MSG (Memory) │
│    Invariants: Liveness · Exclusivity · Interpretation ·             │
│                Origin · Cleanup                                      │
│    Note: IVE has false positives; most programs use --verification   │
│    none to bypass. modular.rs (incremental/per-function IVE) exists  │
│    but is not wired into the pipeline.                               │
├─────────────────────────────────────────────────────────────────────┤
│                    SCG (Core Representation)                         │
│    Nodes (26 types: 14 core + 12 WOMB) · Edges (7 kinds) · Regions │
│    Backed by petgraph::DiGraph (allows cycles; Tarjan SCC for       │
│    topological ordering). Transform passes: ConstantFolding, DCE,    │
│    CSE, InliningPass, LICM, StrengthReduction, DRE, TailCallOpt.    │
├─────────────────────────────────────────────────────────────────────┤
│                    Execution                                         │
│    COR Runtime (partially integrated as Option<CORuntime>) ·        │
│    Multi-ISA Codegen (10 backends)                                  │
│    x86_64 · AArch64 · RISC-V 64/32 · ARM32 · MIPS64 · PPC64        │
│    LoongArch64 · x86_32 · Wasm32                                    │
│    57,377/57,380 gold-standard runs pass (99.99%, --verification    │
│    none). 3 failures: crc32 on riscv64+ppc64, s27_fn_two_args_mod   │
│    on ppc64.                                                         │
└─────────────────────────────────────────────────────────────────────┘
```

## Pipeline

```
Source Text → Lexer → Parser → AST → SCG Lowering → Raw SCG
    → Module Resolution (imports, circular-import detection) → Merged SCG
    → BD Inference (always runs) → MSG Construction (always runs)
    → [Optional: IVE Verification — skipped when --verification none]
    → SCG Transform Passes → IR Lowering → Register Allocation
    → Code Emission → COR Init → Machine Code / Wasm
```

The verification step (stage 6 only) is optional. When `--verification none` is used (the common case), the pipeline skips IVE verification but still runs BD inference (stage 4) and MSG construction (stage 5). The 11-stage pipeline is defined as enum `PipelineStage` in `src/pipeline.rs:567`.

Two code paths exist:
- **Canonical pipeline** (`bridge_scg_to_codegen` in `src/pipeline.rs`): used by `vuma build`, `vuma check`, `vuma verify`, `vuma run`, `vuma compile`, and the `compile_dump` test binary. Goes through the semantic SCG.
- **Direct path** (`bridge_ast_to_codegen_scg`): used by `vuma emit`. Bypasses the semantic SCG for better code quality on simple programs.

## Workspace Crates (11 members)

| Crate | Role | LOC | Tests |
|-------|------|------|-------|
| `vuma-scg` | Semantic Computation Graph (petgraph-backed) | 19,217 | 191 |
| `vuma-bd` | Behavioral Descriptors (RepD, CapD, RelD) + inference | 13,193 | 342 |
| `vuma` (core) | MSG, Region, Derivation, Access, Invariants, REPL, Security | 20,365 | 301 |
| `vuma-ive` | Inference & Verification Engine (5 invariants, modular.rs unused) | 17,824 | 235 |
| `vuma-cor` | Continuous Optimization Runtime (partially integrated) | 8,831 | 110 |
| `vuma-parser` | Lexer (141 tokens) → AST → SCG bridge, resolver, error recovery | 18,807 | 325 |
| `vuma-codegen` | 10-ISA backend, regalloc, DWARF v4, ELF | 105,070 | 1,061 |
| `vuma-proof` | Formal proof system (5 invariant proof types) | 9,132 | 102 |
| `vuma-std` | Rust stdlib wrapper (NOT linked to VUMA programs) | 24,541 | 667 |
| `vuma-package` | Package manager (manifest, resolver, registry) | 1,182 | 6 |
| `vuma-tests` | Integration tests & benchmarks (8 categories) | 25,428 | 459 |

The root `vuma` crate (`src/*.rs`, 16,037 LOC) ties everything together and provides: `main.rs` (CLI), `pipeline.rs` (canonical pipeline), `api.rs` (VumaCompiler API), `llm_api.rs` (VumaForLLM API), `ffi.rs` (19 syscalls), `diagnostics.rs`, `logging.rs`, `telemetry.rs`.

Non-workspace source: `src/bin/` (5 binaries), `src/lsp/mod.rs` (2,055 LOC, 6 LSP capabilities), `src/bootstrap/vuma_compiler.vuma` (730 LOC, lexer POC for self-hosting).

## Key Concepts

### SCG — Semantic Computation Graph
The SCG is the primary program representation. Nodes represent operations (26 types: 14 core + 12 WOMB data-model types), edges represent relationships (7 kinds: DataFlow, ControlFlow, Derivation, Annotation, Dispatch, Call, Return), and regions delineate scopes. The SCG is **not** acyclic — it explicitly supports loops and recursive calls via `has_cycles()` and `topological_sort_with_cycles()` (Tarjan's SCC). Implemented in `vuma-scg`, backed by `petgraph::DiGraph`.

### BD — Behavioral Descriptor
A Behavioral Descriptor replaces traditional nominal types with the triple (RepD, CapD, RelD):
- **RepD** (11 variants): memory layout — Byte, Struct, Array, Enum, Ptr, Union, Func, ManifoldSpatial, GestaltSuperposition, ConceptRelational, Generic
- **CapD** (17 capabilities): permitted operations — Read, Write, Execute, Iterate, Send, Persist, Serialize, Deserialize, Hash, Compare, DerivePtr, Cast, Fork, Drop, Share, Move, Pin
- **RelD** (6 relation kinds): relationships — Temporal, Containment, Dependency, Equivalence, Security, Liveness

BDs are inferred from SCG structure through iterative fixpoint computation with widening. **Note:** Complex generic inference (M2.3) is deferred — `instantiate_generic` does shallow substitution only.

### MSG — Memory State Graph
The MSG captures every allocation, pointer derivation, deallocation, and access. It is constructed from the annotated SCG and verified against the five invariants. The MSG builder handles cycles via SCC-based topological sort (`topological_sort_with_cycles()`), but may produce false positives on some valid programs. Implemented in `vuma-core` (`src/vuma/src/msg.rs`, `msg_builder.rs`, `msg_incremental.rs`).

### IVE — Inference & Verification Engine
The IVE reads the SCG, infers BDs, constructs the MSG, and verifies the five invariants. It supports interprocedural analysis, escape analysis, and verification caching. A modular verification infrastructure exists (`src/ive/src/modular.rs`, 389 LOC) with per-function analysis (`verify_function`, `verify_all_functions`), incremental caching (`IncrementalCache`), and abstract region tracking (`AbstractRegionTracker`), but is **not yet integrated** into the main pipeline — no other code references it.

Two `VerificationLevel` enums exist:
- `pipeline::VerificationLevel` (4 variants: None, Quick, Normal, Exhaustive) — controls whether IVE runs at all
- `ive::VerificationLevel` (3 variants: Quick, Normal, Exhaustive) — controls IVE depth when it does run

### COR — Continuous Optimization Runtime
Maintains an always-compiled invariant: every reachable SCG region is kept in compiled machine code. Performs incremental compilation, PGO (4 optimization passes in `optimization.rs`), and speculative optimization (`speculative.rs`). Implemented in `vuma-cor`. **Partially integrated** — the pipeline holds an `Option<CORuntime>` field and the `CorInit` stage runs at the end of the pipeline, but COR is not fully wired end-to-end.
