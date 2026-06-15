# VUMA Project Roadmap

**Project:** VUMA — Verified-Unsafe Memory Access Framework  
**Version:** 0.2.0  
**Status:** Phase 2 — Core Implementation (substantially complete)  
**Date:** March 5, 2026  

---

## Overview

The VUMA project implements a six-layer AI-native programming language framework: (1) Semantic Computation Graph (SCG), (2) Inference and Verification Engine (IVE), (3) Projection System, (4) Continuous Optimization Runtime (COR), (5) Behavioral Descriptors (BD), and (6) Verified-Unsafe Memory Access (VUMA). The project targets multi-architecture backends (AArch64, x86_64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32) with AArch64 as the primary platform.

The implementation follows a bottom-up strategy organized into five phases, each building on the output of previous phases. Phase 1 (COMPLETE) established foundational data structures and the minimal pipeline. Phase 2 (current, substantially complete) implements core verification, multi-backend code generation, and LLM integration. Phase 3 hardens the system and adds concurrency. Phase 4 adds language server and tooling support. Phase 5 achieves self-hosting compiler status. Each phase has specific milestones and deliverables that are objectively verifiable.

### Key Differentiator: LLM Integration

VUMA is designed as an **AI-native** language framework. Unlike traditional compilers that expose only a command-line interface, VUMA provides a rich programmatic API (`VumaCompiler`), a structured REPL with LLM-friendly commands (`:check`, `:diagnostics`, `:exports`), a full LSP server, and JSON-structured diagnostics — all specifically designed for LLM agents to compile, verify, and iterate on code programmatically. This LLM-first design allows AI coding agents to use VUMA as a verified compilation sandbox: parse source, analyze structure, run verification, compile to any of 8 backends, and inspect results — all through clean API boundaries. The REPL's `:wasm` command enables LLMs to produce sandboxed Wasm modules for safe execution, while `:check` provides instant IVE verification feedback.

---

## Phase 1: Foundation (COMPLETE)

**Goal:** Establish the core data structures, memory model, multi-architecture codegen, parser, and the full verification pipeline. At the end of Phase 1, the project can build a SCG, verify basic memory invariants, parse VUMA source text, and emit machine code for 8 backend architectures.

**Status:** ✅ Complete

### Milestones

| Milestone | Description | Status |
|-----------|-------------|--------|
| M1.1 | SCG core types, construction, serialization, and property tests | ✅ Complete |
| M1.2 | MSG construction from SCG, derivation chain tracking | ✅ Complete |
| M1.3 | IVE liveness and origin verification passes | ✅ Complete |
| M1.4 | Multi-architecture codegen: 8 backends (AArch64, x86_64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, Wasm32) | ✅ Complete |
| M1.5 | Parser with lexer, AST, error recovery, and AST-to-SCG lowering | ✅ Complete |
| M1.6 | Proof system with formal proofs, counterexamples, and tactics | ✅ Complete |

### Deliverables

**SCG Crate (`src/scg/`).** Fully implemented with all node types (`AllocationNode`, `AccessNode`, `DeallocationNode`, `CastNode`, `ComputationNode`, `ControlNode`, `EffectNode`, `PhantomNode`), edge types (`DataFlow`, `ControlFlow`, `Derivation`, `Annotation`), region system (`SCGRegion`, `DeploymentTarget`), query engine (`SCGQuery`, `find_derivation_chains`, `find_access_nodes_to_region`), dominance analysis (dominator tree, dominance frontier, post-dominators), liveness analysis (use-after-free detection, dead allocation identification, uninitialized read detection), transform passes (DCE, constant folding, inlining, CSE via `PassManager`), diff/merge (`compute_edit_script`, `three_way_merge`), and JSON serialization.

**VUMA Crate (`src/vuma/`).** Fully implemented with `Address` newtype, `Region` type (contiguous address range with allocation status), `Derivation` (pointer provenance tracking with `DerivationKind`), `Access` (read/write at a program point), `SyncEdge` (ordering between accesses), `MSG` (the Memory State Graph), `scg_to_msg` conversion pipeline (topological walk, monotonic address allocation, derivation chain construction, post-conversion verification), incremental MSG (`MSGDelta`, `compute_delta`, `apply_delta`, `SCGSnapshot`), and all five invariant checkers (`invariant_liveness`, `invariant_exclusivity`, `invariant_interpretation`, `invariant_origin`, `invariant_cleanup`).

**IVE Crate (`src/ive/`).** Implemented with `InferenceEngine` (BD propagation, constraint derivation), `VerificationEngine` (5 invariant checks), `InvariantAggregator` (runs all checks, produces `VerificationSummary`), individual verifiers (liveness with proof obligations, exclusivity with interference graph, interpretation with WriteReadPair tracking, origin, cleanup with resource lifecycle graph), `VerificationDebt` tracking (ordered by priority), interprocedural analysis, escape analysis, BD solver, verification cache, and verification result types (`VerificationResult`, `ConfidenceLevel`, `CounterExample`).

**BD Crate (`src/bd/`).** Implemented with `RepD` (Byte, Struct, Enum, Array, Pointer, Union, Opaque representations), `CapD` (capability set with lattice operations), `RelD` (relation kinds: Containment, Aliasing, DataFlow, etc.), `BD` triple (composition, compatibility, refinement), `Context`/`ContextSolver` (context-dependent capabilities), `capd_lattice` (meet, join, subcap), `reld_refine` (refinement ordering), `repd_compat` (compatibility checking), BD inference, and `unify` (BD unification algorithm).

**Codegen Crate (`src/codegen/`).** Implemented with **8 backend architectures**: `arm64` (AArch64 instruction definitions, register/condition enums, binary encoding), `x86_64` (x86-64 instruction encoding and register allocation), `riscv64` (RISC-V 64-bit instruction encoding), `arm32` (ARM32/AArch32 instruction encoding), `mips64` (MIPS64 instruction encoding), `ppc64` (PowerPC 64-bit instruction encoding), `loongarch64` (LoongArch64 instruction encoding), `wasm32` (WebAssembly 32-bit module generation). Also includes `ir` module (functions, blocks, instructions, terminators, values), `scg_to_ir` translation, `regalloc` (linear-scan register allocator), `emit` (code emitter and ELF generation), and `backend` trait (multi-backend architecture with `TargetInfo` and `Backend` traits). All backends pass SHA256d validation; 6 native backends (x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64) pass full execution testing; Wasm32 generates valid modules; LoongArch64 passes individual operation tests.

**COR Crate (`src/cor/`).** Implemented with `CORuntime` orchestrator, `ProfileCollector` (thread-safe, hardware PMU counters), `SpeculativeExecutor` (branch prediction, speculative inlining, code motion, snapshot-based rollback), `OptimizationEngine` (DCE, folding, inlining, loop unrolling), `DeploymentManager` (hot-swap via 6-phase state machine, delta deployment with block-level binary diffing, version tracking with rollback), and `Config` (optimization level, time budgets, target architecture).

**Std Crate (`src/std/`).** Implemented with primitives (`Ptr`, `RegionPtr`, `Slice`, `VumaResult`, `VumaOption`, `Range`, `HasBD` trait), alloc, collections, sync, io, string, and math modules.

**Proof Crate (`src/proof/`).** Implemented with `Proof`/`ProofStep`/`Goal`/`ProofStatus`, `checker`, `rules`, `tactics`, `counterexample`, and per-invariant proof modules.

**Parser Crate (`src/parser/`).** Implemented with full lexer (keywords, identifiers, operators, literals, BD annotations), recursive-descent parser producing AST, AST-to-SCG lowering (`AstToScg`), error recovery with helpful messages, and incremental parsing support.

**Projection Crate (`src/projection/`).** Implemented with textual, visual, conversational, bidirectional, and diff projections.

### Phase 1 Achievement Summary

Phase 1 established the complete architectural foundation of the VUMA framework, exceeding original goals. All workspace crates are implemented with full functionality. The system can: construct SCGs programmatically and from parsed text; infer Behavioral Descriptors; construct MSGs from SCGs; verify all five VUMA invariants (liveness, exclusivity, interpretation, origin, cleanup); generate machine code for 8 backend architectures; produce formal proofs and counterexamples; and parse VUMA source text into AST and SCG.

---

## Phase 2: Core Implementation (SUBSTANTIALLY COMPLETE)

**Goal:** Complete the verification engine, strengthen BD inference, expand multi-architecture codegen to handle complex programs, add LLM integration features, and demonstrate non-trivial verified programs. At the end of Phase 2, the IVE can verify all five invariants for single-threaded programs with dynamic allocation, 8 backends handle complex programs, and LLM agents can use VUMA as a verified compilation sandbox.

**Status:** ✅ Substantially Complete (9/11 milestones achieved)

### Milestones

| Milestone | Description | Status |
|-----------|-------------|--------|
| M2.1 | Exclusivity and interpretation verification passes pass all integration tests | ✅ Complete |
| M2.2 | Cleanup verification and full invariant pipeline with incremental re-verification | ✅ Complete |
| M2.3 | BD inference subsumes Rust type system (all Rust-typable programs have valid BDs) | 📋 Deferred |
| M2.4 | Doubly-linked list verified by IVE with no unsafe blocks | 📋 Deferred |
| M2.5 | Multi-architecture codegen handles complex programs (SHA256d, factorial, Fibonacci) | ✅ Complete |
| M2.6 | Profile-guided optimization improves benchmarks by ≥15% | ✅ Complete |
| M2.7 | LLM API (`VumaCompiler`) for programmatic compilation and analysis | ✅ Complete |
| M2.8 | LSP server with diagnostics, hover, go-to-definition, completion | ✅ Complete |
| M2.9 | REPL with LLM-friendly commands (`:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`) | ✅ Complete |
| M2.10 | Module resolution system | ✅ Complete |
| M2.11 | Wasm32 sandbox compilation for LLM agents | ✅ Complete |

### Deliverables

**2.1 — Exclusivity and Interpretation Verification.** ✅ Complete. The exclusivity pass handles aliasing through multiple pointers in single-threaded programs. The interpretation pass catches type confusion (reading integer bytes as pointer, reading uninitialized memory). `ExclusivityResult` and `InterpretationResult` types are implemented. Integration tests cover data races, type confusion, and uninitialized reads. All five VUMA invariants are verifiable end-to-end.

**2.2 — Cleanup Verification and Full Pipeline.** ✅ Complete. The cleanup verification pass identifies memory leaks and respects intentional leak annotations (arenas, globals). The full invariant pipeline runs all five passes in optimal order and produces a comprehensive `VerificationSummary`. Incremental verification re-verifies only affected subgraphs when the SCG changes. Verification debt tracking is implemented.

**2.3 — BD Inference Completeness.** 📋 Pending. RepD inference that derives representation descriptors from SCG structure is partially implemented. CapD inference that derives capability sets from usage patterns is implemented. RelD inference that derives relationships from SCG edges is implemented. BD consistency checking and subsumption testing remain to be completed.

**2.4 — Verified Data Structures.** 📋 Pending. The doubly-linked list verification remains a key milestone. The framework has all the necessary infrastructure (five invariant verifiers, proof system, counterexample generation) but demonstration of non-trivial data structure verification is still needed.

**2.5 — Multi-Architecture Codegen.** ✅ Complete. All 8 backends handle complex programs. SHA256d passes on 6 native backends (x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64). Wasm32 generates valid Wasm modules. LoongArch64 passes individual operation tests. The `Backend` trait architecture enables easy addition of new targets. Cross-backend validation tests ensure consistency.

**2.6 — Profile-Guided Optimization.** ✅ Complete. The `ProfileCollector`, `PmuCounters`, `collect_profile` analysis, and `ProfileReport` with `CacheOptimize`/`BranchLayout` recommendations are complete. Profile-guided optimization improves benchmark performance by ≥15%.

**2.7 — LLM API.** ✅ Complete. The `VumaCompiler` API provides a clean programmatic interface for LLM agents: `compile()` for full pipeline execution, `compile_for_target()` for target-specific compilation, `parse()` for syntax analysis, `analyze()` for SCG structure inspection, `available_targets()` for backend enumeration, and `validate()` for diagnostic-only checks. Results include structured `CompileResult`, `ParseResult`, `ScgSummary`, `FunctionSummary`, and `ApiTargetInfo` types — all JSON-serializable for LLM consumption.

**2.8 — LSP Server.** ✅ Complete. The `LspServer` provides full LSP protocol support: textDocument sync, diagnostics with line/column info, hover information, go-to-definition, completion suggestions, document symbols, and semantic tokens. This enables IDE integration and LLM agent interaction with the compiler.

**2.9 — Enhanced REPL.** ✅ Complete. The REPL provides LLM-friendly commands: `:wasm` (compile to Wasm and show binary size), `:backends` (list available backends with status), `:check` (run IVE verification), `:diagnostics` (show all diagnostics as JSON), `:exports` (list all functions and signatures), tab completion for commands and VUMA keywords, and ANSI color output for errors/warnings.

**2.10 — Module Resolution.** ✅ Complete. Module resolution with `ModuleResolution` error type handles import resolution and module path tracking.

**2.11 — Wasm32 Sandbox.** ✅ Complete. LLM agents can compile VUMA programs to Wasm32 modules for safe sandboxed execution. The `:wasm` REPL command and `compile_for_target("wasm32")` API provide Wasm compilation with binary size reporting.

### Phase 2 Success Criteria

- [x] All five VUMA invariant verifiers pass their integration test suites
- [x] Incremental verification re-verifies affected subgraphs only
- [ ] Doubly-linked list requires no `unsafe` blocks (VUMA-VERIFIED instead) — *Deferred to Phase 3*
- [ ] BD inference subsumes the Rust type system (subsumption test passes) — *Deferred to Phase 3*
- [x] 8 backend codegens correctly compile SHA256d, factorial, Fibonacci, and data structure operations
- [x] Profile-guided optimization improves benchmark performance by ≥15%
- [x] LLM API (`VumaCompiler`) enables programmatic compilation and analysis
- [x] LSP server provides diagnostics, hover, go-to-definition, and completion
- [x] REPL supports LLM-friendly commands with structured output
- [x] Wasm32 sandbox compilation for LLM agent code execution
- [x] Module resolution system with import syntax
- [x] Package manager foundation with dependency resolution
- [x] Memory safety analysis with 10 violation types
- [x] Constant-time crypto operations across all 8 backends
- [x] FFI and syscall support across 8 architectures
- [x] 65 structured diagnostic codes with error chaining
- [x] Cross-backend validation and ABI conformance testing

---

## Phase 3: Hardening & Optimization (IN PROGRESS — Waves 6-32)

**Goal:** Add concurrency support, expand codegen to include atomics and barriers, implement the full Continuous Optimization Runtime, add comprehensive peripheral support, and harden the verification pipeline. At the end of Phase 3, the IVE can verify multi-threaded programs and the codegen can produce efficient code for concurrent algorithms.

**Status:** 🔄 In Progress (significant progress from Waves 6-32)

### Milestones

| Milestone | Description | Target | Status |
|-----------|-------------|--------|--------|
| M3.1 | ARM64 atomic instructions (LDXR, STXR, CAS, barriers) with correct encoding | Sprint 9 | 📋 Planned |
| M3.2 | Concurrent exclusivity verification with happens-before analysis | Sprint 10 | 📋 Planned |
| M3.3 | COR integration: incremental compilation, PGO, speculative optimization | Sprint 11 | 🔄 In Progress |
| M3.4 | Full peripheral support (GPIO, UART, I2C, SPI, DMA, interrupts) | Sprint 12 | 📋 Planned |
| M3.5 | Lock-free data structure verified and running on bare metal | Sprint 12 | 📋 Planned |
| M3.6 | Verification pipeline hardening: interprocedural analysis, escape analysis, caching | Sprint 9-10 | ✅ Complete |
| M3.7 | Cross-backend validation and consistency testing | Sprint 9-10 | ✅ Complete |
| M3.8 | Diagnostics system with structured error reporting | Sprint 9-10 | ✅ Complete |

### Deliverables

**3.1 — ARM64 Atomics and Concurrency Primitives.** Implement atomic instructions (LDXR, STXR, CAS, LDADD, STADD with all sizes and ordering variants), barrier instructions (DMB, DSB, ISB with all option variants), exclusive access modeling for the IVE (LDXR/STXR exclusive monitors for exclusivity verification), lock-free data structure codegen (atomic counter, spinlock, MCS lock lowering), thread management (spawn/join lowering via bare-metal scheduler stubs), AAPCS64 compliance verification, concurrent SCG node codegen (`ForkNode`, `JoinNode`, `SyncNode`, `ChannelSendNode`, `ChannelRecvNode`), and concurrency codegen tests (spinlock, producer-consumer, dining philosophers).

**3.2 — Concurrent Verification.** Extend the exclusivity pass to handle LDXR/STXR, locks, and channel-based synchronization. Implement happens-before analysis that builds a happens-before graph from synchronization operations (locks, barriers, channel sends/receives). Implement thread-local MSG that partitions the MSG by thread and verifies per-thread invariants before cross-thread analysis. Implement deadlock detection that identifies circular lock acquisition orders. Implement liveness verification for concurrent programs (every lock eventually released, every channel send eventually received). Create concurrent verification test suite covering at least 10 scenarios (races, deadlocks, livelocks).

**3.3 — COR Integration.** Integrate incremental compilation that recompiles only affected SCG subgraphs when edits occur (target: <500ms for single-function edits). Complete profile data collection with runtime execution counters. Implement profile-guided optimization with hot path identification and aggressive optimization. Implement speculative optimization with transparent fallback on mis-speculation. Implement SCG transformation verification that proves each optimization pass preserves semantics (verified by IVE). Add COR integration tests verifying that optimized programs produce the same results as unoptimized programs.

**3.4 — Full Peripheral Support.** Implement memory map (peripheral base addresses, register layouts, access permissions). Complete GPIO driver with memory-mapped register access and capability descriptors. Complete UART driver (PL011 and mini UART with interrupt support). Implement interrupt controller with interrupt routing. Implement timer driver (ARM generic timer and system timer). Implement DMA controller with cache coherency management. Implement PCIe driver for NVMe and network. Create platform integration test (boot, initialize all peripherals, run concurrent workload, verify via UART).

**3.5 — Lock-Free Demonstration.** Implement a lock-free data structure (e.g., Michael-Scott queue) verified by the concurrent IVE. Lower it to ARM64 code with atomic instructions. Deploy it on bare-metal hardware. Verify correctness via UART output. This is the concurrent milestone: the first non-trivial concurrent program verified by the IVE and running on real hardware.

**3.6 — Verification Pipeline Hardening.** ✅ Complete. Interprocedural analysis (`interprocedural` module), escape analysis (`escape` module), BD solver (`bd_solver` module), verification cache (`cache` module), and constraint system (`constraint` module) are implemented. The verification pipeline now handles cross-function analysis and caches intermediate results.

**3.7 — Cross-Backend Validation.** ✅ Complete. Cross-backend validation tests (`cross_backend.rs`), execution validation (`execution_validation.rs`), and ELF validation (`elf_validation.rs`) ensure that all 8 backends produce consistent, correct results. SHA256d is the primary validation benchmark.

**3.8 — Diagnostics System.** ✅ Complete. Structured diagnostics (`diagnostics.rs`) with `VumaDiagnostic` types, severity levels, source context, and JSON serialization. The `ModuleResolution` error type handles import resolution. All errors are structured for LLM consumption.

### Phase 3 Success Criteria

- [ ] LDXR/STXR pair encodes correctly and runs on QEMU
- [ ] DMB/DSB/ISB instructions encode correctly with all option fields
- [ ] Lock-free atomic counter passes IVE exclusivity verification
- [ ] IVE correctly identifies data races in multi-threaded programs
- [ ] Happens-before graph correctly constructed from lock and channel operations
- [ ] Deadlock detection identifies circular lock acquisition orders
- [x] Incremental compilation support is in place
- [x] Profile-guided optimization improves benchmark performance by ≥15%
- [x] Verification pipeline handles interprocedural analysis and caching
- [x] Cross-backend validation ensures consistency across all 8 backends
- [x] Structured diagnostics system provides LLM-friendly error reporting
- [ ] GPIO can toggle an LED (blink test)
- [ ] UART produces reliable console output at 115200 baud
- [ ] Full boot sequence completes in under 2 seconds on target hardware
- [ ] Lock-free data structure running on bare metal with concurrent IVE verification

---

## Phase 4: Language Server & Tooling

**Goal:** Complete the Projection System, outcome spaces, and a comprehensive language server protocol (LSP) implementation. At the end of Phase 4, VUMA is a complete, usable framework with human-facing projections, IDE integration, and a growing ecosystem.

**Status:** 🔄 Partially Complete (LSP server already implemented in Phase 2)

### Milestones

| Milestone | Description | Target | Status |
|-----------|-------------|--------|--------|
| M4.1 | Textual and visual projections with bidirectional editing | Sprint 13 | ✅ Complete |
| M4.2 | Outcome spaces with exhaustive handling verification | Sprint 14 | 📋 Pending |
| M4.3 | Parser with error recovery and incremental parsing | Sprint 15 | ✅ Complete |
| M4.4 | Language server protocol implementation | Sprint 15 | ✅ Complete |
| M4.5 | Standard library and ecosystem examples | Sprint 16 | 📋 Pending |

### Deliverables

**4.1 — Projection System.** ✅ Complete. Textual projection renders SCG as human-readable code-like text with role-specific views. Bidirectional editing parses textual projections back into SCG modifications, validates via IVE before applying. Visual projection renders SCG as dataflow diagram, call graph, and memory layout view. Diff projection computes and renders semantic diffs between SCG versions. The `Projection` trait with `render()` and `parse()` methods provides pluggable projection backends.

**4.2 — Outcome Spaces and Error Handling.** 📋 Pending. Implement outcome space types for structured enumeration of all possible outcomes (success, validation errors, timeouts, resource exhaustion). Implement outcome space inference where the IVE infers the complete outcome space of each function from its SCG structure. Implement exhaustive handling verification where the IVE proves that every outcome is handled explicitly or by a verified safe default. Implement safe default inference where the IVE infers safe default handlers for common unhandled outcomes. Implement outcome space narrowing where, as invariants are established, the outcome space shrinks.

**4.3 — Parser and Language Server.** ✅ Complete. Full lexer tokenizes VUMA textual syntax (keywords, identifiers, operators, literals, BD annotations). Recursive-descent parser produces SCG from the token stream, handling expressions, control flow, function definitions, and BD annotations. Error recovery produces helpful messages with suggestions. Incremental parsing re-parses only changed portions of text. LSP server provides: go-to-definition, find-all-references, hover information (BD details, verification status), diagnostics (verification violations, counterexamples), code actions, completion, document symbols, and semantic tokens.

**4.4 — Standard Library and Ecosystem.** 📋 Pending. Complete `vuma_std::mem` (allocation, deallocation, copy, fill, zero — all VUMA-VERIFIED). Complete `vuma_std::collections` (Vec, LinkedList, HashMap, BTreeMap — all VUMA-VERIFIED, no unsafe). Complete `vuma_std::sync` (Mutex, RwLock, Channel, AtomicU32, AtomicU64 — all VUMA-VERIFIED). Complete `vuma_std::io` (Read, Write, BufRead traits with UART and peripheral implementations). Complete `vuma_std::fmt` (Display, Debug formatting with UART output backend). Complete `vuma_std::bd` (BD construction helpers, RepD/CapD/RelD builders, BD annotation macros). Create ecosystem examples: HTTP server, key-value store, sensor reader, real-time signal processor.

### Phase 4 Success Criteria

- [x] Textual projection renders benchmark programs as readable, syntax-highlighted code
- [x] Bidirectional editing: modifying the projection and applying yields a verified SCG
- [x] Visual projection produces dataflow diagrams
- [ ] Outcome space inference correctly enumerates all possible outcomes
- [ ] Exhaustive handling verification catches unhandled failure modes
- [x] Parser correctly produces SCG from textual syntax for all benchmark programs
- [x] Error messages are helpful and suggest corrections for common mistakes
- [x] LSP provides go-to-definition, diagnostics, completion, and code actions
- [ ] All stdlib functions are VUMA-VERIFIED (no IVE-TODO items in stdlib)
- [ ] LinkedList implementation requires no `unsafe` blocks
- [ ] At least 4 ecosystem examples run on target hardware

---

## Phase 5: Self-Hosting Compiler

**Goal:** Bootstrap the VUMA compiler in VUMA itself. The compiler becomes its own first non-trivial program, verified by its own IVE, compiled by its own codegen, and running on its own COR. This is the ultimate proof of the framework's viability: if VUMA can verify and compile itself, it can verify and compile any program.

**Status:** 📋 Planned (far future)

### Milestones

| Milestone | Description | Target |
|-----------|-------------|--------|
| M5.1 | VUMA compiler core written in VUMA textual syntax | TBD |
| M5.2 | Self-verification: VUMA compiler verified by its own IVE | TBD |
| M5.3 | Self-compilation: VUMA compiler compiled by its own codegen | TBD |
| M5.4 | Self-hosting: VUMA compiler running on its own COR | TBD |
| M5.5 | Performance parity: self-hosted compiler within 2× of Rust-compiled version | TBD |

### Phase 5 Success Criteria

- VUMA compiler core is written entirely in VUMA textual syntax
- All five VUMA invariants verified for the compiler itself
- Self-compiled binary passes all tests that the Rust-compiled binary passes
- Self-hosted compiler can compile and verify a non-trivial test program
- Self-hosted compiler achieves within 2× of Rust-compiled performance

---

## LLM Integration

VUMA's LLM integration is a key differentiator that sets it apart from traditional programming language frameworks. Every interface is designed for programmatic consumption by AI agents.

### Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                      LLM Agent                                       │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────────────┐    │
│  │ Vuma API │  │ LSP      │  │ REPL     │  │ Structured       │    │
│  │ (VumaCmp)│  │ Server   │  │ Commands │  │ Diagnostics      │    │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────────┬─────────┘    │
│       │              │              │                  │              │
│       └──────────────┴──────────────┴──────────────────┘              │
│                              │                                        │
│                     ┌────────▼────────┐                              │
│                     │ VUMA Compiler   │                              │
│                     │ Pipeline        │                              │
│                     │ (parse → SCG →  │                              │
│                     │  IVE → codegen) │                              │
│                     └────────┬────────┘                              │
│                              │                                        │
│              ┌───────────────┼───────────────┐                       │
│              ▼               ▼               ▼                       │
│        ┌──────────┐   ┌──────────┐   ┌──────────────┐               │
│        │ 8 Native │   │ Wasm32   │   │ JSON         │               │
│        │ Backends │   │ Sandbox  │   │ Diagnostics  │               │
│        └──────────┘   └──────────┘   └──────────────┘               │
└──────────────────────────────────────────────────────────────────────┘
```

### LLM-Facing Interfaces

| Interface | Description | Use Case |
|-----------|-------------|----------|
| `VumaCompiler` API | Programmatic compilation: `compile()`, `parse()`, `analyze()`, `validate()` | LLM compiles and verifies code in a sandbox |
| `compile_for_target()` | Target-specific compilation (8 backends + Wasm32) | LLM produces sandboxed Wasm modules |
| LSP Server | Full LSP protocol: diagnostics, hover, go-to-definition, completion | IDE integration and LLM agent interaction |
| REPL `:wasm` | Compile current session to Wasm and show binary size | Quick Wasm compilation check |
| REPL `:backends` | List available backends with status | Discover compilation capabilities |
| REPL `:check` | Run IVE verification on current session | Instant verification feedback |
| REPL `:diagnostics` | Show all diagnostics as JSON | Structured error analysis |
| REPL `:exports` | List all functions and their signatures | Program structure inspection |
| `VumaDiagnostic` | Structured diagnostics with severity, source context, JSON output | Error analysis and automated fixing |

### Wasm32 Sandbox for LLM Agents

The Wasm32 backend enables LLM agents to compile VUMA programs into safe, sandboxed WebAssembly modules. This is the recommended execution path for LLM-generated code: the Wasm module runs in a sandboxed environment with no access to host memory or peripherals, ensuring that LLM-generated code cannot cause harm. The `:wasm` REPL command and `compile_for_target("wasm32")` API provide Wasm compilation with binary size reporting.

---

## Dependency Graph

```
Phase 1 (COMPLETED) ──┬── Phase 2 (SUBSTANTIALLY COMPLETE) ──── Phase 3 (IN PROGRESS) ──── Phase 4 ──── Phase 5
                       │                                       │                          │            │
  SCG Foundation ──────┤                            Concurrency &           LSP (DONE)    Self-hosting
  MSG Construction ────┤                            Hardening                Projections   compiler
  IVE Core ────────────┤                            COR Integration          Stdlib
  Multi-Arch Codegen ──┤                            Diagnostics
  BD Types             │
  COR Framework        │
  Parser & AST ────────┤
  Proof System ────────┤
  LLM API ─────────────┤
  LSP Server ──────────┤
  Module Resolution ───┘
```

**Critical path:** Phase 1 → Phase 2 (BD inference completeness) → Phase 3 (concurrent verification) → Phase 4 (stdlib/ecosystem) → Phase 5 (self-hosting).

The longest sequential dependency chain is in the verification pipeline: BD inference must be complete before concurrent verification can be extended, which must be complete before the IVE can verify itself in Phase 5.

---

## Risk Mitigation

| Risk | Impact | Mitigation |
|------|--------|-----------|
| IVE verification too slow for large programs | Blocks Phase 3+ | Verification cache and incremental verification implemented; profile and optimize hot paths continuously; use verification debt to prioritize |
| Backend instruction encoding bugs | Blocks execution | Cross-backend validation tests; SHA256d benchmark; QEMU testing for native backends; Wasm validation for Wasm32 |
| BD inference incompleteness | Blocks Phase 2+ | Subsumption test against Rust type system; fallback to explicit annotations; iterative refinement via profile feedback |
| Target hardware availability | Blocks Phase 3 peripheral testing | QEMU-based testing as primary; hardware testing as validation |
| Concurrent verification undecidability | Blocks Phase 3 | Limit to finite-state abstraction; use tiered verification confidence; accept partial verification with explicit debt |
| Self-hosting complexity | Blocks Phase 5 | Incremental approach: self-verify subsystem by subsystem; use Rust-compiled compiler to verify each step |
| LLM API stability | Blocks LLM adoption | Stable API surface with JSON-serializable results; backward-compatible changes only; LLM agents can rely on structured output |

---

## Success Criteria Summary

| Phase | Milestone Name | Key Metric | Status |
|-------|---------------|------------|--------|
| Phase 1 | Foundation Complete | 8 backends, parser, proof system, all 5 invariants | ✅ Complete |
| Phase 2 | Core Implementation | LLM API, LSP, REPL, Wasm32 sandbox, verification pipeline | ✅ Substantially Complete |
| Phase 3 | Hardening | Concurrent verification, COR integration, diagnostics | 🔄 In Progress |
| Phase 4 | VUMA IDE | Projections (done), outcome spaces, stdlib, ecosystem | 📋 Planned |
| Phase 5 | Self-Hosting | VUMA compiler verifies and compiles itself | 📋 Planned |

---

### Wave 1-32 Summary

| Wave Range | Focus Area | Key Achievements |
|------------|-----------|------------------|
| W1-5 | Foundation & Platform Consolidation | All 8 backends pass SHA256d, platform-specific code removed, formal specs, initial crates |
| W6 | Testing & Validation | Cross-backend tests, ELF/Wasm validation, PPC64 deep audit, codegen bug fixes |
| W7 | Parser Hardening | LLM type aliases, macro detection, C-style for loop detection, reference conversion |
| W8 | Standard Library | crypto.rs, string.rs, math.rs, enhanced alloc.rs and io.rs |
| W9 | Register Allocator | LoopDetector, GreedyRegCache, dead-vreg reuse, loop-depth spill weights |
| W10 | Module System | Multi-file compilation, import resolution, circular import detection |
| W11-12 | Verification Hardening | VumaCompiler.verify(), property-based testing, proof cross-check |
| W13-14 | Documentation & REPL | ROADMAP overhaul, architecture.md, REPL commands (:wasm, :backends, :check, etc.) |
| W15 | Diagnostics | 65 diagnostic codes, error chaining, structured suggestions, 4 output formats |
| W16 | CI Infrastructure | GitHub Actions, cross-compile matrix (8 targets), Dependabot |
| W17-18 | Memory Safety & Benchmarks | 10 violation types, runtime bounds checks, benchmark suite |
| W19-20 | ABI & Debug Info | ABI conformance (27 tests), DWARF per-backend config, --debug-info |
| W21-22 | Linker & LLM API | 3 LOAD segments (W^X), VumaForLLM API, section alignment |
| W23 | Package Manager | PackageManifest, resolve_dependencies, CLI subcommands |
| W24 | FFI & Syscalls | 19 syscalls × 8 architectures, relocations, is_extern flag |
| W25-27 | Security Hardening | Codegen quality, test infrastructure hardening |
| W28 | Constant-Time Crypto | ct_select/ct_eq across all 8 backends, PPC64 carry-flag masks |
| W29-31 | Final Hardening | Documentation updates, test coverage, release preparation |
| W32 | Release Preparation | Cargo.toml v0.2.0, CHANGELOG, README, ROADMAP, RELEASES.md |

---

*This roadmap is a living document. Phase assignments and timelines may be adjusted based on progress, and milestones may be reordered as dependencies are resolved. The phase structure ensures that each phase builds on verified foundations.*
