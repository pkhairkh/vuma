# VUMA Project Manifest

**Version:** 0.1.0  
**Updated:** 2026-03-05  
**Status:** Phase 2 — Core Implementation

This document lists every file in the VUMA project, its purpose, and line count.
Files in `target/` are excluded.

---

## Table of Contents

1. [Root Configuration](#1-root-configuration)
2. [Documentation (docs/)](#2-documentation-docs)
3. [Source Crates (src/)](#3-source-crates-src)
4. [Examples (examples/)](#4-examples-examples)
5. [Build & CI](#5-build--ci)
6. [Summary Statistics](#6-summary-statistics)

---

## 1. Root Configuration

| File                     | Lines | Purpose                                                      |
|--------------------------|-------|--------------------------------------------------------------|
| `Cargo.toml`             | 49    | Workspace root: 12 crate members, shared deps, release profile |
| `Cargo.lock`             | —     | Lockfile for reproducible builds (auto-generated)            |
| `Makefile`               | 233   | Build/test/bench/doc/Pi 5 cross-compile/flash/debug targets  |
| `justfile`               | 226   | Just command runner shortcuts (mirrors Makefile)             |
| `rust-toolchain.toml`    | 9     | Pinned nightly toolchain, components, targets                |
| `rustfmt.toml`           | 3     | Formatting: max_width=100, tab_spaces=4                      |
| `clippy.toml`            | 1     | Lint: cognitive-complexity-threshold=50                      |
| `.gitignore`             | 8     | Ignore target/, *.swp, *.orig, .env                          |
| `WORKLOG.md`             | 1186  | Detailed agent worklog for all tasks                          |
| `MANIFEST.md`            | —     | This file — complete project file inventory                  |
| `README.md`              | —     | Project README with overview, architecture, quick start       |
| `CHANGELOG.md`           | —     | Changelog for Waves 1–5                                      |

**Root subtotal:** ~1,718 lines

---

## 2. Documentation (docs/)

### 2.1 Top-Level Documentation

| File                     | Lines | Purpose                                                      |
|--------------------------|-------|--------------------------------------------------------------|
| `docs/architecture.md`   | 994   | Full architecture: 6-layer system, data flow, crate deps, data structures, verification/codegen/COR pipelines, security model |
| `docs/language-reference.md` | 1101 | VUMA language reference: lexical structure, types/BD, memory model, pointers, control flow, concurrency, safety, Pi 5 features |
| `docs/ROADMAP.md`        | 277   | 5-phase roadmap with milestones, deliverables, success criteria, risk mitigation |
| `docs/CONTRIBUTING.md`   | 840   | Contributor guide: build, test, add nodes/verifications/instructions, code review, PR template |
| `docs/CONVENTIONS.md`    | 796   | Coding conventions: style, error handling, testing, naming, docs, git commits |
| `docs/GLOSSARY.md`       | 893   | Project glossary: 40+ terms across core, verification, ARM64, Pi 5, type theory |
| `docs/WORKLOG.md`        | 405   | Detailed worklog for documentation tasks                      |

### 2.2 Formal Specifications (docs/specs/)

| File                                     | Lines | Purpose                                              |
|------------------------------------------|-------|------------------------------------------------------|
| `docs/specs/scg-formal-spec.md`          | 475   | SCG mathematical model: graph definition, node/edge/region types, well-formedness |
| `docs/specs/repd-formal-spec.md`         | 546   | RepD formal spec: representation descriptor lattice, subsumption, compatibility |
| `docs/specs/capd-formal-spec.md`         | 492   | CapD formal spec: capability descriptor lattice, meet/join, context transitions |
| `docs/specs/reld-formal-spec.md`         | 600   | RelD formal spec: relational descriptor kinds, composition, refinement ordering |
| `docs/specs/vuma-invariants-spec.md`     | 742   | VUMA 5 invariants: liveness, exclusivity, interpretation, origin, cleanup |
| `docs/specs/msg-construction-spec.md`    | 850   | MSG construction algorithm: SCG-to-MSG mapping, derivation chains, sync edges |
| `docs/specs/pi5-memory-model-spec.md`    | 809   | Pi 5 memory model: BCM2712 address map, MMIO, DMA, cache coherency |
| `docs/specs/security-model-spec.md`      | 606   | Security model: 5 layers, threat model, confidence/debt tracking |
| `docs/specs/bd-inference-algorithm.md`   | 1027  | BD inference algorithm: RepD/CapD/RelD inference, fixpoint iteration |
| `docs/specs/vuma-verification-algorithm.md` | 1098 | VUMA verification algorithm: 5 invariant checkers, proof obligations, counterexamples |
| `docs/specs/arm64-codegen-algorithm.md`  | 1182  | ARM64 codegen algorithm: SCG→IR, regalloc, insn selection, encoding |
| `docs/specs/benchmark-design.md`         | 695   | Benchmark design: 8 categories, methodology, statistical analysis |
| `docs/specs/trivial-proofs.md`           | 547   | Trivial program proofs: allocate/read/write/free, cast, concurrent |
| `docs/specs/dlist-proof.md`              | 631   | Doubly-linked list proof: sentinel node, insertion, deletion, threading |
| `docs/specs/decidability-analysis.md`    | 416   | Decidability analysis: which invariants are decidable, approximation |

**Docs subtotal:** ~11,682 lines

---

## 3. Source Crates (src/)

### 3.1 Workspace Root

| File                     | Lines | Purpose                                                      |
|--------------------------|-------|--------------------------------------------------------------|
| `src/lib.rs`             | 50    | Workspace crate root: re-exports, feature flags              |
| `src/pipeline.rs`        | 1210  | Top-level compilation pipeline: parse → SCG → IVE → codegen → COR |

**Workspace root subtotal:** ~1,260 lines

### 3.2 `src/scg/` — Semantic Computation Graph (Layer 1)

| File                     | Lines | Purpose                                                      |
|--------------------------|-------|--------------------------------------------------------------|
| `src/scg/Cargo.toml`     | 12    | Crate manifest: depends on serde, petgraph, indexmap, smallvec |
| `src/scg/src/lib.rs`     | 206   | Crate root: re-exports, module overview, node/edge type table |
| `src/scg/src/node.rs`    | 329   | NodeId, NodeType (12 variants), NodeData, NodePayload, per-variant structs |
| `src/scg/src/edge.rs`    | 177   | EdgeId, EdgeKind (8 variants), EdgeData                      |
| `src/scg/src/graph.rs`   | 1028  | SCG struct: construction, validation, traversal, queries      |
| `src/scg/src/region.rs`  | 218   | RegionId, SCGRegion, DeploymentTarget (6 variants)            |
| `src/scg/src/query.rs`   | 661   | SCGQuery: find_derivation_chains, find_access_nodes_to_region |
| `src/scg/src/dominance.rs` | 1437 | DominatorTree, compute_dominators, dominance frontier         |
| `src/scg/src/liveness.rs` | 1358 | LivenessAnalysis, find_use_after_free, find_dead_allocations  |
| `src/scg/src/transform.rs` | 1453 | PassManager, DCE, constant folding, inlining, CSE, VerificationPass |
| `src/scg/src/diff.rs`    | 1709  | SCGDiff, compute_edit_script, three_way_merge, apply_diff     |
| `src/scg/src/serialize.rs` | 1680 | JSON serialization/deserialization via serde                  |

**SCG subtotal:** ~10,268 lines

### 3.3 `src/bd/` — Behavioral Descriptors (Layer 5)

| File                          | Lines | Purpose                                              |
|-------------------------------|-------|------------------------------------------------------|
| `src/bd/Cargo.toml`           | 10    | Crate manifest: depends on scg, serde, smallvec      |
| `src/bd/src/lib.rs`           | 44    | Crate root: re-exports, BD triple overview           |
| `src/bd/src/repd.rs`          | 442   | RepD: Byte, Struct, Enum, Array, Pointer, Union, Opaque |
| `src/bd/src/capd.rs`          | 398   | CapD: capability set with BitSet, context-dependent  |
| `src/bd/src/reld.rs`          | 330   | RelD: relation kinds (Containment, Aliasing, DataFlow, etc.) |
| `src/bd/src/descriptor.rs`    | 229   | BD triple: RepD × CapD × RelD, compatibility, refinement |
| `src/bd/src/inference.rs`     | 1706  | BD inference from SCG structure, constraint propagation |
| `src/bd/src/context.rs`       | 160   | Context: evaluation context for CapD transitions     |
| `src/bd/src/context_solver.rs`| 1186  | Context-dependent capability resolution solver       |
| `src/bd/src/capd_lattice.rs`  | 1217  | CapD lattice operations: meet, join, subcap         |
| `src/bd/src/reld_refine.rs`   | 1317  | RelD refinement ordering and composition             |
| `src/bd/src/repd_compat.rs`   | 1570  | RepD compatibility checking and subtyping            |
| `src/bd/src/unify.rs`         | 1464  | BD unification algorithm for inference               |

**BD subtotal:** ~10,073 lines

### 3.4 `src/vuma/` — VUMA Core / Memory Model (Layer 6)

| File                              | Lines | Purpose                                          |
|-----------------------------------|-------|--------------------------------------------------|
| `src/vuma/Cargo.toml`             | 13    | Crate manifest: depends on scg, bd, serde, thiserror |
| `src/vuma/src/lib.rs`             | 90    | Crate root: re-exports, module overview          |
| `src/vuma/src/address.rs`         | 190   | Address newtype with hex display and arithmetic  |
| `src/vuma/src/region.rs`          | 234   | Region: contiguous memory span, RegionId, RegionStatus |
| `src/vuma/src/derivation.rs`      | 272   | Derivation: pointer provenance tracking, DerivationKind |
| `src/vuma/src/access.rs`          | 217   | Access: read/write at program point, AccessKind  |
| `src/vuma/src/sync.rs`            | 142   | SyncEdge: ordering between accesses, SyncOrdering |
| `src/vuma/src/msg.rs`             | 422   | MSG: Memory State Graph tying regions, derivations, accesses |
| `src/vuma/src/msg_builder.rs`     | 2238  | MSG construction from raw data, builder pattern  |
| `src/vuma/src/msg_incremental.rs` | 1907  | MSGDelta, compute_delta, apply_delta, SCGSnapshot |
| `src/vuma/src/scg_to_msg.rs`      | 1357  | SCG → MSG conversion pipeline (topological walk) |
| `src/vuma/src/access_analysis.rs` | 1476  | Access pattern analysis: hot/cold spots, conflicts |
| `src/vuma/src/program_point.rs`   | 140   | Source location tracking for verification diagnostics |
| `src/vuma/src/invariant_liveness.rs`      | 1022 | Liveness invariant checker: every access targets allocated memory |
| `src/vuma/src/invariant_exclusivity.rs`   | 1108 | Exclusivity invariant checker: no conflicting concurrent accesses |
| `src/vuma/src/invariant_interpretation.rs`| 1469 | Interpretation invariant checker: every access uses a valid RepD |
| `src/vuma/src/invariant_origin.rs`        | 903  | Origin invariant checker: every address traces to valid allocation |
| `src/vuma/src/invariant_cleanup.rs`       | 1118 | Cleanup invariant checker: every region eventually freed or leaked |
| `src/vuma/src/security.rs`        | 2094  | Security model: 5 layers, threat categories, confidence/debt |
| `src/vuma/src/repl.rs`            | 1459  | VUMA REPL: interactive verification and exploration |

**VUMA core subtotal:** ~16,204 lines

### 3.5 `src/ive/` — Inference & Verification Engine (Layer 2)

| File                              | Lines | Purpose                                          |
|-----------------------------------|-------|--------------------------------------------------|
| `src/ive/Cargo.toml`             | 13    | Crate manifest: depends on scg, bd, vuma, proof  |
| `src/ive/src/lib.rs`             | 79    | Crate root: re-exports, IVE architecture overview |
| `src/ive/src/inference.rs`       | 224   | InferenceEngine: BD propagation, constraint derivation |
| `src/ive/src/bd_solver.rs`       | 1482  | BD constraint solver for inference               |
| `src/ive/src/constraint.rs`      | 244   | Constraint types: temporal, resource flow, security |
| `src/ive/src/verification.rs`    | 262   | VerificationEngine: 5 invariant checks entry point |
| `src/ive/src/liveness.rs`        | 2032  | Liveness verifier with proof obligations         |
| `src/ive/src/exclusivity.rs`     | 1571  | Exclusivity verifier with interference graph     |
| `src/ive/src/interpretation.rs`  | 1619  | Interpretation verifier with WriteReadPair tracking |
| `src/ive/src/origin.rs`          | 1726  | Origin verifier with derivation chain validation |
| `src/ive/src/cleanup.rs`         | 1600  | Cleanup verifier with resource lifecycle graph   |
| `src/ive/src/invariant_aggregator.rs` | 1141 | Runs all 5 checks, produces unified VerificationSummary |
| `src/ive/src/result.rs`          | 246   | VerificationResult, VerificationStatus, ConfidenceLevel |
| `src/ive/src/debt.rs`            | 261   | VerificationDebt: tracking unverified obligations by priority |

**IVE subtotal:** ~12,500 lines

### 3.6 `src/cor/` — Continuous Optimization Runtime (Layer 4)

| File                              | Lines | Purpose                                          |
|-----------------------------------|-------|--------------------------------------------------|
| `src/cor/Cargo.toml`             | 9     | Crate manifest: depends on scg, vuma, serde      |
| `src/cor/src/lib.rs`             | 63    | Crate root: COR architecture overview, re-exports |
| `src/cor/src/runtime.rs`         | 533   | CORuntime: central orchestrator for continuous opt |
| `src/cor/src/profile.rs`         | 978   | ProfileCollector, Pi5PmuCounters, HotPath, collect_profile |
| `src/cor/src/speculative.rs`     | 1487  | SpeculativeExecutor, BranchPredictionTable, Snapshots |
| `src/cor/src/optimization.rs`    | 1328  | OptimizationEngine, DCE, constant folding, inlining, loop unrolling |
| `src/cor/src/deployment.rs`      | 1423  | DeploymentManager, HotSwap (6-phase FSM), delta deploy, version tracking |
| `src/cor/src/config.rs`          | 160   | Config: optimization level, time budgets, target architecture |
| `src/cor/src/types.rs`           | 263   | COR-internal types: SCG wrapper, Delta, RegionId mapping |

**COR subtotal:** ~6,244 lines

### 3.7 `src/projection/` — Projection System (Layer 3)

| File                              | Lines | Purpose                                          |
|-----------------------------------|-------|--------------------------------------------------|
| `src/projection/Cargo.toml`      | 9     | Crate manifest: depends on scg, bd, serde        |
| `src/projection/src/lib.rs`      | 237   | Crate root: projection architecture, re-exports  |
| `src/projection/src/textual.rs`  | 1390  | Textual projection: SCG → human-readable code    |
| `src/projection/src/visual.rs`   | 1345  | Visual projection: SCG → SVG/HTML dataflow diagrams |
| `src/projection/src/conversational.rs` | 1939 | Conversational projection: SCG → natural language |
| `src/projection/src/bidirectional.rs` | 1550 | Bidirectional editing: projection edits → SCG mods |
| `src/projection/src/diff.rs`     | 1620  | Semantic diff: compute and render differences between SCG versions |

**Projection subtotal:** ~8,090 lines

### 3.8 `src/parser/` — Parser / Frontend (Auxiliary)

| File                              | Lines | Purpose                                          |
|-----------------------------------|-------|--------------------------------------------------|
| `src/parser/Cargo.toml`          | 10    | Crate manifest: depends on scg, thiserror, log   |
| `src/parser/src/lib.rs`          | 98    | Crate root: re-exports                            |
| `src/parser/src/lexer.rs`        | 2334  | Tokenizer for VUMA textual syntax (43+ keywords)  |
| `src/parser/src/parser.rs`       | 2424  | Recursive-descent parser: token stream → AST     |
| `src/parser/src/ast.rs`          | 631   | AST types: Program, Item, Stmt, Expr, Type, etc. |
| `src/parser/src/to_scg.rs`       | 2593  | AST → SCG lowering: name resolution, graph construction |
| `src/parser/src/error.rs`        | 1371  | Parse errors with recovery, diagnostics, "did you mean?" |

**Parser subtotal:** ~9,461 lines

### 3.9 `src/codegen/` — ARM64 Code Generation (Auxiliary)

| File                              | Lines | Purpose                                          |
|-----------------------------------|-------|--------------------------------------------------|
| `src/codegen/Cargo.toml`         | 9     | Crate manifest: depends on scg, thiserror        |
| `src/codegen/src/lib.rs`         | 59    | Crate root: CodegenError, pipeline entry         |
| `src/codegen/src/arm64.rs`       | 3349  | Arm64Instruction enum, register/condition enums, binary encoding |
| `src/codegen/src/ir.rs`          | 2019  | IR types: functions, blocks, instructions, terminators, values |
| `src/codegen/src/scg_to_ir.rs`   | 2470  | SCG → IR translation via ScgToIr converter      |
| `src/codegen/src/regalloc.rs`    | 2506  | Linear-scan register allocator for aarch64       |
| `src/codegen/src/emit.rs`        | 1467  | ARM64 code emitter and ELF generation            |

**Codegen subtotal:** ~11,879 lines

### 3.10 `src/pi5/` — Raspberry Pi 5 Platform

| File                              | Lines | Purpose                                          |
|-----------------------------------|-------|--------------------------------------------------|
| `src/pi5/Cargo.toml`             | 9     | Crate manifest: build=build.rs, no_std compatible |
| `src/pi5/build.rs`               | 47    | Cargo build script for bare-metal aarch64-unknown-none |
| `src/pi5/link.ld`                | 101   | ARM64 linker script: entry, sections, per-core stacks |
| `src/pi5/src/lib.rs`             | 61    | Crate root: platform overview, re-exports        |
| `src/pi5/src/boot.rs`            | 746   | Exception vectors, _start, boot_main, FDT parsing |
| `src/pi5/src/platform.rs`        | 335   | BCM2712 memory map, Pi5Platform, board identification |
| `src/pi5/src/uart.rs`            | 1700  | PL011 UART driver, MiniUart, ring buffer, ISR handlers |
| `src/pi5/src/gpio.rs`            | 1571  | Memory-mapped GPIO: set_function, set_pull, pin mux |
| `src/pi5/src/timer.rs`           | 484   | ARM generic timer, virtual timer, C-style API    |
| `src/pi5/src/mmio.rs`            | 541   | MMIO register access, ARM64 barriers, MmioDevice trait |
| `src/pi5/src/smp.rs`             | 525   | Multicore boot, IPI, Spinlock with RAII guard    |

**Pi 5 subtotal:** ~6,120 lines

### 3.11 `src/proof/` — Formal Proof System

| File                              | Lines | Purpose                                          |
|-----------------------------------|-------|--------------------------------------------------|
| `src/proof/Cargo.toml`           | 9     | Crate manifest: depends on scg, bd               |
| `src/proof/src/lib.rs`           | 41    | Crate root: proof system overview, re-exports    |
| `src/proof/src/proof.rs`         | 402   | Proof, ProofStep, Goal, ProofStatus, Counterexample |
| `src/proof/src/checker.rs`       | 476   | Proof checker: verify each step independently    |
| `src/proof/src/rules.rs`         | 478   | Inference rules for proof construction            |
| `src/proof/src/tactics.rs`       | 363   | Automated proof tactics                           |
| `src/proof/src/counterexample.rs`| 264   | Counterexample generation and minimization       |
| `src/proof/src/liveness_proofs.rs`     | 1201 | Liveness-specific proof rules and obligations   |
| `src/proof/src/exclusivity_proofs.rs`  | 1837 | Exclusivity-specific proof rules                |
| `src/proof/src/interpretation_proofs.rs`| 1582 | Interpretation-specific proof rules            |
| `src/proof/src/origin_proofs.rs`       | 1142 | Origin-specific proof rules                     |
| `src/proof/src/cleanup_proofs.rs`      | 1329 | Cleanup-specific proof rules                    |

**Proof subtotal:** ~9,124 lines

### 3.12 `src/std/` — Standard Library

| File                              | Lines | Purpose                                          |
|-----------------------------------|-------|--------------------------------------------------|
| `src/std/Cargo.toml`             | 7     | Crate manifest: minimal dependencies             |
| `src/std/src/lib.rs`             | 102   | Crate root: re-exports (BD, RelD, Ptr, RegionPtr, etc.) |
| `src/std/src/primitives.rs`      | 1700  | Ptr, RegionPtr, Slice, VumaResult, VumaOption, Range, HasBD |
| `src/std/src/alloc.rs`           | 2472  | VumaAllocator, BumpAllocator, FreeListAllocator, MemoryStats |
| `src/std/src/collections.rs`     | 2293  | Vec, HashMap, VumaString, LinkedList, RingBuffer, SipHash13 |
| `src/std/src/sync.rs`            | 1722  | Mutex, RwLock, Channel, AtomicU32/64 — VUMA-VERIFIED |
| `src/std/src/io.rs`              | 2007  | Read, Write, BufRead traits, UART and Pi 5 backends |

**Std subtotal:** ~10,303 lines

### 3.13 `src/tests/` — Integration Tests & Benchmarks

| File                              | Lines | Purpose                                          |
|-----------------------------------|-------|--------------------------------------------------|
| `src/tests/Cargo.toml`           | 15    | Crate manifest: depends on all workspace crates   |
| `src/tests/src/lib.rs`           | 50    | Test framework root, module declarations          |
| `src/tests/src/framework.rs`     | 2060  | Test infrastructure: SCG builders, helper macros  |
| `src/tests/src/trivial.rs`       | 126   | Trivial program verification tests               |
| `src/tests/src/dlist.rs`         | 135   | Doubly-linked list verification tests             |
| `src/tests/src/bd_inference.rs`  | 190   | BD inference integration tests                    |
| `src/tests/src/concurrent.rs`    | 106   | Concurrent verification tests                     |
| `src/tests/src/graph.rs`         | 118   | SCG graph construction and query tests            |
| `src/tests/src/benchmarks.rs`    | 1162  | Benchmark suite: 8 categories, 40+ benchmarks    |

**Tests subtotal:** ~3,962 lines

---

## 4. Examples (examples/)

| File                              | Lines | Purpose                                          |
|-----------------------------------|-------|--------------------------------------------------|
| `examples/README.md`              | 154   | Example descriptions and running instructions    |
| `examples/hello_memory.vuma`      | 40    | Basic allocate/write/read/free pattern           |
| `examples/doubly_linked_list.vuma`| 89    | Doubly-linked list with sentinel node pattern    |
| `examples/arena_allocator.vuma`   | 78    | Arena allocator with derivation chains           |
| `examples/gpio_blink.vuma`        | 68    | Pi 5 GPIO hardware blink                         |
| `examples/lock_free_queue.vuma`   | 99    | Lock-free SPSC queue with atomics               |
| `examples/channel_demo.vuma`      | 237   | Channel-based concurrency demo                   |
| `examples/memory_arena.vuma`      | 197   | Memory arena with region-based allocation        |
| `examples/pi5_sensor.vuma`        | 188   | Pi 5 sensor reading with MMIO                    |
| `examples/sorted_map.vuma`        | 192   | Sorted map data structure                         |
| `examples/thread_pool.vuma`       | 209   | Thread pool with work stealing                   |

**Examples subtotal:** ~1,551 lines

---

## 5. Build & CI

| File                              | Lines | Purpose                                          |
|-----------------------------------|-------|--------------------------------------------------|
| `.cargo/config.toml`              | 58    | Cargo build config: cross-compilation, target flags |
| `.github/workflows/ci.yml`       | 217   | GitHub Actions CI: fmt, clippy, test, doc, Pi 5 cross-compile |

**Build/CI subtotal:** ~275 lines

---

## 6. Summary Statistics

### By Category

| Category               | Files | Lines     |
|------------------------|-------|-----------|
| Root Configuration     | 10    | ~1,718    |
| Documentation          | 22    | ~11,682   |
| Source: SCG            | 12    | ~10,268   |
| Source: BD             | 13    | ~10,073   |
| Source: VUMA Core      | 20    | ~16,204   |
| Source: IVE            | 14    | ~12,500   |
| Source: COR            | 9     | ~6,244    |
| Source: Projection     | 7     | ~8,090    |
| Source: Parser         | 7     | ~9,461    |
| Source: Codegen        | 7     | ~11,879   |
| Source: Pi 5           | 11    | ~6,120    |
| Source: Proof          | 13    | ~9,124    |
| Source: Std            | 7     | ~10,303   |
| Source: Tests          | 9     | ~3,962    |
| Source: Pipeline       | 2     | ~1,260    |
| Examples               | 11    | ~1,551    |
| Build & CI             | 2     | ~275      |
| **Total**              | **~166** | **~129,714** |

### By Language

| Language               | Approximate Lines |
|------------------------|-------------------|
| Rust (`.rs`)           | ~100,000          |
| Markdown (`.md`)       | ~28,000           |
| VUMA (`.vuma`)         | ~1,400            |
| TOML (`.toml`)         | ~200              |
| Linker Script (`.ld`)  | ~100              |
| YAML (`.yml`)          | ~217              |
| Makefile / Justfile    | ~459              |

### Crates by Size (descending)

| Crate                  | Lines   |
|------------------------|---------|
| vuma-core              | ~16,204 |
| vuma-ive               | ~12,500 |
| vuma-codegen           | ~11,879 |
| vuma-scg               | ~10,268 |
| vuma-std               | ~10,303 |
| vuma-bd                | ~10,073 |
| vuma-proof             | ~9,124  |
| vuma-parser            | ~9,461  |
| vuma-projection        | ~8,090  |
| vuma-pi5               | ~6,120  |
| vuma-cor               | ~6,244  |
| vuma-tests             | ~3,962  |

---

## Worklog

- **2026-03-05 — Task 5-9:** Created comprehensive MANIFEST.md with all 166 project files, purposes, and line counts across 6 sections. Includes summary statistics by category, language, and crate size.
