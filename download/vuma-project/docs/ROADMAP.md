# VUMA Project Roadmap

**Project:** VUMA — Verified-Unsafe Memory Access Framework  
**Version:** 0.1.0  
**Status:** Phase 2 — Core Implementation  
**Date:** March 5, 2026  

---

## Overview

The VUMA project implements a six-layer AI-native programming language framework: (1) Semantic Computation Graph (SCG), (2) Inference and Verification Engine (IVE), (3) Projection System, (4) Continuous Optimization Runtime (COR), (5) Behavioral Descriptors (BD), and (6) Verified-Unsafe Memory Access (VUMA). The project targets the Raspberry Pi 5 (BCM2712, quad Cortex-A76) as its primary hardware platform.

The implementation follows a bottom-up strategy organized into five phases, each building on the output of previous phases. Phase 1 establishes foundational data structures and the minimal pipeline. Phase 2 (current) implements core verification and inference. Phase 3 hardens the system and optimizes performance. Phase 4 adds language server and tooling support. Phase 5 achieves self-hosting compiler status. Each phase has specific milestones and deliverables that are objectively verifiable.

---

## Phase 1: Foundation (COMPLETED)

**Goal:** Establish the core data structures, memory model, minimal ARM64 codegen, and the basic verification pipeline. At the end of Phase 1, the project can build a simple SCG, verify basic memory invariants, and emit ARM64 assembly that runs on QEMU or Pi 5 hardware.

**Status:** ✅ Complete

### Milestones

| Milestone | Description | Status |
|-----------|-------------|--------|
| M1.1 | SCG core types, construction, serialization, and property tests | ✅ Complete |
| M1.2 | MSG construction from SCG, derivation chain tracking | ✅ Complete |
| M1.3 | IVE liveness and origin verification passes | ✅ Complete |
| M1.4 | ARM64 core instruction encoding and Pi 5 boot | ✅ Complete |

### Deliverables

**SCG Crate (`src/scg/`).** Fully implemented with all node types (`AllocationNode`, `AccessNode`, `DeallocationNode`, `CastNode`, `ComputationNode`, `ControlNode`, `EffectNode`, `PhantomNode`), edge types (`DataFlow`, `ControlFlow`, `Derivation`, `Annotation`), region system (`SCGRegion`, `DeploymentTarget`), query engine (`SCGQuery`, `find_derivation_chains`, `find_access_nodes_to_region`), dominance analysis (dominator tree, dominance frontier, post-dominators), liveness analysis (use-after-free detection, dead allocation identification, uninitialized read detection), transform passes (DCE, constant folding, inlining, CSE via `PassManager`), diff/merge (`compute_edit_script`, `three_way_merge`), and JSON serialization.

**VUMA Crate (`src/vuma/`).** Fully implemented with `Address` newtype, `Region` type (contiguous address range with allocation status), `Derivation` (pointer provenance tracking with `DerivationKind`), `Access` (read/write at a program point), `SyncEdge` (ordering between accesses), `MSG` (the Memory State Graph), `scg_to_msg` conversion pipeline (topological walk, monotonic address allocation, derivation chain construction, post-conversion verification), incremental MSG (`MSGDelta`, `compute_delta`, `apply_delta`, `SCGSnapshot`), and all five invariant checkers (`invariant_liveness`, `invariant_exclusivity`, `invariant_interpretation`, `invariant_origin`, `invariant_cleanup`).

**IVE Crate (`src/ive/`).** Implemented with `InferenceEngine` (BD propagation, constraint derivation), `VerificationEngine` (5 invariant checks), `InvariantAggregator` (runs all checks, produces `VerificationSummary`), individual verifiers (liveness with proof obligations, exclusivity with interference graph, interpretation with WriteReadPair tracking, origin, cleanup with resource lifecycle graph), `VerificationDebt` tracking (ordered by priority), and verification result types (`VerificationResult`, `ConfidenceLevel`, `CounterExample`).

**BD Crate (`src/bd/`).** Implemented with `RepD` (Byte, Struct, Enum, Array, Pointer, Union, Opaque representations), `CapD` (capability set with lattice operations), `RelD` (relation kinds: Containment, Aliasing, DataFlow, etc.), `BD` triple (composition, compatibility, refinement), `Context`/`ContextSolver` (context-dependent capabilities), `capd_lattice` (meet, join, subcap), `reld_refine` (refinement ordering), `repd_compat` (compatibility checking), and `unify` (BD unification algorithm).

**Codegen Crate (`src/codegen/`).** Implemented with `arm64` module (instruction definitions, register/condition enums, binary encoding), `ir` module (functions, blocks, instructions, terminators, values), `scg_to_ir` translation, `regalloc` (linear-scan register allocator), and `emit` (ARM64 code emitter and ELF generation).

**COR Crate (`src/cor/`).** Implemented with `CORuntime` orchestrator, `ProfileCollector` (thread-safe, Pi 5 PMU counters), `SpeculativeExecutor` (branch prediction, speculative inlining, code motion, snapshot-based rollback), `OptimizationEngine` (DCE, folding, inlining, loop unrolling), `DeploymentManager` (hot-swap via 6-phase state machine, delta deployment with block-level binary diffing, version tracking with rollback), and `Config` (optimization level, time budgets, target architecture).

**Pi 5 Crate (`src/pi5/`).** Implemented with bare-metal boot code (exception vectors, `_start` entry, FDT parsing, `boot_main`), linker script (`link.ld` with per-core stacks, MMIO window), build script (`build.rs` for `aarch64-unknown-none`), UART driver, GPIO driver, timer driver, MMIO primitives, and SMP (multicore boot).

**Std Crate (`src/std/`).** Implemented with primitives (`Ptr`, `RegionPtr`, `Slice`, `VumaResult`, `VumaOption`, `Range`, `HasBD` trait), alloc, collections, sync, and io modules.

**Proof Crate (`src/proof/`).** Implemented with `Proof`/`ProofStep`/`Goal`/`ProofStatus`, `checker`, `rules`, `tactics`, `counterexample`, and per-invariant proof modules.

**Parser and Projection Crates.** Initial implementations with lexer, parser, AST, AST-to-SCG lowering, error recovery; textual, visual, conversational, bidirectional, and diff projections.

### Phase 1 Achievement Summary

Phase 1 established the complete architectural foundation of the VUMA framework. All 12 workspace crates are implemented with core functionality. The system can: construct SCGs programmatically and from parsed text; infer Behavioral Descriptors; construct MSGs from SCGs; verify all five VUMA invariants (liveness, exclusivity, interpretation, origin, cleanup); generate ARM64 machine code; and boot on Pi 5 hardware with UART output. The Makefile provides `make pi5`, `make pi5-image`, `make pi5-flash`, and `make pi5-debug` targets for the full development cycle.

---

## Phase 2: Core Implementation (CURRENT)

**Goal:** Complete the verification engine, strengthen BD inference, expand the ARM64 codegen to handle complex programs, and demonstrate non-trivial verified programs running on Pi 5 hardware. At the end of Phase 2, the IVE can verify all five invariants for single-threaded programs with dynamic allocation, and the framework can verify a doubly-linked list with no `unsafe` blocks.

**Status:** 🔄 In Progress

### Milestones

| Milestone | Description | Status |
|-----------|-------------|--------|
| M2.1 | Exclusivity and interpretation verification passes pass all integration tests | 🔄 In Progress |
| M2.2 | Cleanup verification and full invariant pipeline with incremental re-verification | 🔄 In Progress |
| M2.3 | BD inference subsumes Rust type system (all Rust-typable programs have valid BDs) | 📋 Pending |
| M2.4 | Doubly-linked list verified by IVE with no unsafe blocks | 📋 Pending |
| M2.5 | ARM64 codegen handles complex programs (factorial, Fibonacci, data structures) | 🔄 In Progress |
| M2.6 | Profile-guided optimization improves benchmarks by ≥15% | ✅ Complete |

### Deliverables

**2.1 — Exclusivity and Interpretation Verification.** Extend the exclusivity pass to handle aliasing through multiple pointers in single-threaded programs. Complete the interpretation pass to catch type confusion (reading integer bytes as pointer, reading uninitialized memory). Implement `ExclusivityResult` and `InterpretationResult` types. Add integration tests for data races, type confusion, and uninitialized reads. Ensure all five VUMA invariants are verifiable end-to-end.

**2.2 — Cleanup Verification and Full Pipeline.** Complete the cleanup verification pass to identify memory leaks and respect intentional leak annotations (arenas, globals). Implement the full invariant pipeline that runs all five passes in optimal order and produces a comprehensive `VerificationSummary`. Implement incremental verification that re-verifies only affected subgraphs when the SCG changes, targeting sub-1-second verification for single-function edits. Implement verification debt tracking. Create benchmark programs: doubly-linked list, tree, hash map, arena allocator — all verified by IVE.

**2.3 — BD Inference Completeness.** Implement RepD inference that derives representation descriptors from SCG structure (allocation sizes, field accesses, cast operations). Implement CapD inference that derives capability sets from usage patterns (read, written, sent, persisted). Implement RelD inference that derives relationships from SCG edges (data flow, ownership, security boundaries). Implement BD consistency checking to verify that inferred BDs are internally consistent and compatible with explicit annotations. Implement BD subsumption testing to verify that BD inference subsumes traditional type inference — every Rust-typable program must produce a valid BD assignment.

**2.4 — Verified Data Structures.** Demonstrate that the VUMA framework can verify non-trivial data structures without `unsafe` blocks. The primary target is a doubly-linked list, which in Rust requires `unsafe` for pointer manipulation. In VUMA, the IVE should verify all five invariants for the doubly-linked list implementation, producing formal proofs for verified operations and counterexamples for any violations. Additional targets: binary tree, hash map with open addressing, arena allocator.

**2.5 — ARM64 Codegen Expansion.** Expand the ARM64 codegen to handle all SCG node types and complex control flow (nested loops, recursive functions, multi-way branches). Improve the register allocator to handle at least 32 virtual registers with intelligent spilling. Implement function calling conventions (AAPCS64) with proper stack alignment and callee-saved register preservation. Add snapshot tests for instruction encoding and integration tests for the full lowering pipeline.

**2.6 — Profile-Guided Optimization.** Complete the profile collection and analysis pipeline with Pi 5 PMU counter integration. Implement hot path identification, aggressive optimization for hot paths, and size optimization for cold paths. Target ≥15% improvement over unoptimized codegen on benchmark programs. ✅ Already implemented — the `ProfileCollector`, `Pi5PmuCounters`, `collect_profile` analysis, and `ProfileReport` with `CacheOptimize`/`BranchLayout` recommendations are complete.

### Phase 2 Success Criteria

- All five VUMA invariant verifiers pass their integration test suites
- Incremental verification re-verifies in under 1 second for single-function edits
- Doubly-linked list requires no `unsafe` blocks (VUMA-VERIFIED instead)
- BD inference subsumes the Rust type system (subsumption test passes)
- ARM64 codegen correctly compiles factorial, Fibonacci, and data structure operations
- Benchmark suite covers at least 5 data structures
- Profile-guided optimization improves benchmark performance by ≥15%

---

## Phase 3: Hardening & Optimization (NEXT)

**Goal:** Add concurrency support, expand ARM64 codegen to include atomics and barriers, implement the full Continuous Optimization Runtime, and achieve comprehensive Pi 5 peripheral support. At the end of Phase 3, the IVE can verify multi-threaded programs and the codegen can produce efficient ARM64 code for concurrent algorithms running on bare-metal Pi 5.

**Status:** 📋 Planned

### Milestones

| Milestone | Description | Target |
|-----------|-------------|--------|
| M3.1 | ARM64 atomic instructions (LDXR, STXR, CAS, barriers) with correct encoding | Sprint 9 |
| M3.2 | Concurrent exclusivity verification with happens-before analysis | Sprint 10 |
| M3.3 | COR integration: incremental compilation, PGO, speculative optimization | Sprint 11 |
| M3.4 | Pi 5 full peripheral support (GPIO, UART, I2C, SPI, DMA, interrupts) | Sprint 12 |
| M3.5 | Lock-free data structure verified and running on bare-metal Pi 5 | Sprint 12 |

### Deliverables

**3.1 — ARM64 Atomics and Concurrency Primitives.** Implement atomic instructions (LDXR, STXR, CAS, LDADD, STADD with all sizes and ordering variants), barrier instructions (DMB, DSB, ISB with all option variants), exclusive access modeling for the IVE (LDXR/STXR exclusive monitors for exclusivity verification), lock-free data structure codegen (atomic counter, spinlock, MCS lock lowering), thread management (spawn/join lowering via Pi 5 bare-metal scheduler stubs), AAPCS64 compliance verification, concurrent SCG node codegen (`ForkNode`, `JoinNode`, `SyncNode`, `ChannelSendNode`, `ChannelRecvNode`), and concurrency codegen tests (spinlock, producer-consumer, dining philosophers).

**3.2 — Concurrent Verification.** Extend the exclusivity pass to handle LDXR/STXR, locks, and channel-based synchronization. Implement happens-before analysis that builds a happens-before graph from synchronization operations (locks, barriers, channel sends/receives). Implement thread-local MSG that partitions the MSG by thread and verifies per-thread invariants before cross-thread analysis. Implement deadlock detection that identifies circular lock acquisition orders. Implement liveness verification for concurrent programs (every lock eventually released, every channel send eventually received). Create concurrent verification test suite covering at least 10 scenarios (races, deadlocks, livelocks).

**3.3 — COR Integration.** Integrate incremental compilation that recompiles only affected SCG subgraphs when edits occur (target: <500ms for single-function edits). Complete profile data collection with runtime execution counters. Implement profile-guided optimization with hot path identification and aggressive optimization. Implement speculative optimization with transparent fallback on mis-speculation. Implement SCG transformation verification that proves each optimization pass preserves semantics (verified by IVE). Add COR integration tests verifying that optimized programs produce the same results as unoptimized programs.

**3.4 — Pi 5 Full Peripheral Support.** Implement BCM2712 memory map (peripheral base addresses, register layouts, access permissions). Complete GPIO driver with memory-mapped register access and capability descriptors. Complete UART driver (PL011 and mini UART with interrupt support). Implement interrupt controller (GIC-400 or BCM2712 equivalent) with interrupt routing. Implement timer driver (ARM generic timer and BCM2712 system timer). Implement DMA controller with cache coherency management. Implement PCIe driver (BCM2712 PCIe 2.0 root complex for NVMe and network). Create platform integration test (boot, initialize all peripherals, run concurrent workload, verify via UART).

**3.5 — Lock-Free Pi 5 Demonstration.** Implement a lock-free data structure (e.g., Michael-Scott queue) verified by the concurrent IVE. Lower it to ARM64 code with atomic instructions. Deploy it on bare-metal Pi 5 hardware. Verify correctness via UART output. This is the "concurrent Pi 5" milestone: the first non-trivial concurrent program verified by the IVE and running on real hardware.

### Phase 3 Success Criteria

- LDXR/STXR pair encodes correctly and runs on QEMU
- DMB/DSB/ISB instructions encode correctly with all option fields
- Lock-free atomic counter passes IVE exclusivity verification
- IVE correctly identifies data races in multi-threaded programs
- Happens-before graph correctly constructed from lock and channel operations
- Deadlock detection identifies circular lock acquisition orders
- Incremental compilation recompiles in under 500ms for single-function edits
- Profile-guided optimization improves benchmark performance by ≥15%
- All optimization passes verified to preserve semantics by the IVE
- GPIO can toggle an LED (blink test)
- UART produces reliable console output at 115200 baud
- Full boot sequence completes in under 2 seconds on Pi 5 hardware
- Lock-free data structure running on bare-metal Pi 5 with concurrent IVE verification

---

## Phase 4: Language Server & Tooling

**Goal:** Implement the Projection System, outcome spaces, parser with error recovery, and a language server protocol (LSP) implementation. At the end of Phase 4, VUMA is a complete, usable framework with human-facing projections, IDE integration, and a growing ecosystem.

**Status:** 📋 Planned

### Milestones

| Milestone | Description | Target |
|-----------|-------------|--------|
| M4.1 | Textual and visual projections with bidirectional editing | Sprint 13 |
| M4.2 | Outcome spaces with exhaustive handling verification | Sprint 14 |
| M4.3 | Parser with error recovery and incremental parsing | Sprint 15 |
| M4.4 | Language server protocol implementation | Sprint 15 |
| M4.5 | Standard library and ecosystem examples | Sprint 16 |

### Deliverables

**4.1 — Projection System.** Implement textual projection that renders SCG as human-readable code-like text with role-specific views (systems programmer, domain expert, security auditor). Implement bidirectional editing that parses textual projections back into SCG modifications, validates via IVE before applying. Implement visual projection that renders SCG as dataflow diagram, call graph, and memory layout view (SVG/HTML output). Implement diff projection that computes and renders semantic diffs between SCG versions in human terms. Implement the projection framework as an abstract `Projection` trait with `render()` and `parse()` methods, pluggable projection backends. Verify round-trip property (SCG → projection → SCG preserves semantics).

**4.2 — Outcome Spaces and Error Handling.** Implement outcome space types for structured enumeration of all possible outcomes (success, validation errors, timeouts, resource exhaustion). Implement outcome space inference where the IVE infers the complete outcome space of each function from its SCG structure. Implement exhaustive handling verification where the IVE proves that every outcome is handled explicitly or by a verified safe default. Implement safe default inference where the IVE infers safe default handlers for common unhandled outcomes (retry for timeout, backoff for resource exhaustion). Implement outcome space narrowing where, as invariants are established, the outcome space shrinks (e.g., post-authentication removes "unauthorized").

**4.3 — Parser and Language Server.** Implement a full lexer that tokenizes VUMA textual syntax (keywords, identifiers, operators, literals, BD annotations). Implement a recursive-descent parser that produces SCG from the token stream, handling expressions, control flow, function definitions, and BD annotations. Implement error recovery that produces helpful messages with suggestions, not just "unexpected token". Implement incremental parsing that re-parses only changed portions of text, maintaining the SCG-to-text mapping. Implement a Language Server Protocol (LSP) server that provides: go-to-definition (jump from usage to SCG node), find-all-references, hover information (BD details, verification status), diagnostics (verification violations, counterexamples), code actions (suggested fixes for violations), and document symbols. Target parse speed under 10ms for 1000-node programs and incremental re-parse under 100ms for single-statement edits.

**4.4 — Standard Library and Ecosystem.** Complete `vuma_std::mem` (allocation, deallocation, copy, fill, zero — all VUMA-VERIFIED). Complete `vuma_std::collections` (Vec, LinkedList, HashMap, BTreeMap — all VUMA-VERIFIED, no unsafe). Complete `vuma_std::sync` (Mutex, RwLock, Channel, AtomicU32, AtomicU64 — all VUMA-VERIFIED). Complete `vuma_std::io` (Read, Write, BufRead traits with UART and Pi 5 peripheral implementations). Complete `vuma_std::fmt` (Display, Debug formatting with UART output backend). Complete `vuma_std::pi5` (Pi 5 platform abstractions). Complete `vuma_std::bd` (BD construction helpers, RepD/CapD/RelD builders, BD annotation macros). Create ecosystem examples: HTTP server, key-value store, sensor reader, real-time signal processor — all running on Pi 5.

### Phase 4 Success Criteria

- Textual projection renders benchmark programs as readable, syntax-highlighted code
- Bidirectional editing: modifying the projection and applying yields a verified SCG
- Visual projection produces interactive SVG/HTML dataflow diagrams
- Outcome space inference correctly enumerates all possible outcomes
- Exhaustive handling verification catches unhandled failure modes
- Parser correctly produces SCG from textual syntax for all benchmark programs
- Error messages are helpful and suggest corrections for common mistakes
- Incremental parsing re-parses in under 100ms for single-statement edits
- LSP provides go-to-definition, diagnostics, and code actions
- All stdlib functions are VUMA-VERIFIED (no IVE-TODO items in stdlib)
- LinkedList implementation requires no `unsafe` blocks
- At least 4 ecosystem examples run on Pi 5 hardware

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
| M5.4 | Self-hosting: VUMA compiler running on its own COR on Pi 5 | TBD |
| M5.5 | Performance parity: self-hosted compiler within 2× of Rust-compiled version | TBD |

### Deliverables

**5.1 — Compiler Core in VUMA.** Rewrite the SCG construction, BD inference, verification, and code generation logic in VUMA's own textual syntax. The parser, IVE, and codegen are the three primary subsystems to self-host. Start with the simplest: the SCG construction library, which has no dependencies on verification. Then tackle BD inference, which depends on the SCG. Then verification, which depends on BD inference. Finally codegen, which depends on verification. This incremental approach ensures that each subsystem is verified by the existing Rust-based compiler before being used to verify subsequent subsystems.

**5.2 — Self-Verification.** Run the VUMA compiler's own IVE on the VUMA compiler's own SCG. This is the most technically challenging milestone: the IVE must be able to verify programs as complex as itself. This requires: (1) the IVE's own code must be well-structured enough that the IVE can reason about it, (2) the verification pipeline must handle recursive data structures (the SCG is defined in terms of itself), and (3) the proof system must be powerful enough to prove invariants about the IVE's own algorithms. Success means: all five VUMA invariants are verified for the compiler itself, with no critical or high-priority verification debt.

**5.3 — Self-Compilation.** Use the VUMA codegen to compile the VUMA compiler itself. The resulting ARM64 binary should produce the same output as the Rust-compiled version. This requires: (1) the codegen must handle all SCG node types used by the compiler, (2) the register allocator must handle the compiler's large functions, and (3) the linker script must accommodate the compiler's larger binary size. Success means: the self-compiled binary passes all the same tests as the Rust-compiled binary.

**5.4 — Self-Hosting.** Run the self-compiled VUMA compiler on the COR on Pi 5 hardware. The compiler reads VUMA source files, constructs SCGs, infers BDs, verifies invariants, generates ARM64 code, and writes the output — all running on the COR on Pi 5. This is the "VUMA runs VUMA" milestone: the system is fully self-hosting. Success means: the self-hosted compiler can compile a non-trivial test program (e.g., a doubly-linked list) from VUMA source to ARM64 binary, verify it, and run it on Pi 5.

**5.5 — Performance Parity.** Optimize the self-hosted compiler to achieve within 2× of the Rust-compiled version's performance. This requires: (1) profile-guided optimization of the compiler's hot paths (parsing, inference, verification), (2) speculative optimization of the IVE's fixpoint iteration, and (3) careful memory management to avoid the overhead that a garbage collector would impose. Success means: compilation of a 10,000-node program completes in under 10 seconds on Pi 5.

### Phase 5 Success Criteria

- VUMA compiler core is written entirely in VUMA textual syntax
- All five VUMA invariants verified for the compiler itself
- Self-compiled binary passes all tests that the Rust-compiled binary passes
- Self-hosted compiler can compile and verify a non-trivial test program on Pi 5
- Self-hosted compiler achieves within 2× of Rust-compiled performance

---

## Dependency Graph

```
Phase 1 (COMPLETED) ──┬── Phase 2 (CURRENT) ──── Phase 3 (NEXT) ──── Phase 4 ──── Phase 5
                       │                          │                     │            │
                       │                          │                     │            │
  SCG Foundation ──────┤                 Concurrency &     LSP & Projections   Self-hosting
  MSG Construction ────┤                 Optimization      Parser & Stdlib       compiler
  IVE Core ────────────┤                 Pi 5 Peripherals  Ecosystem
  ARM64 Codegen ───────┘
  BD Types
  COR Framework
```

**Critical path:** Phase 1 → Phase 2 (BD inference) → Phase 3 (concurrent verification) → Phase 4 (parser/LSP) → Phase 5 (self-hosting).

The longest sequential dependency chain is in the verification pipeline: BD inference must be complete before concurrent verification can be extended, which must be complete before the IVE can verify itself in Phase 5.

---

## Risk Mitigation

| Risk | Impact | Mitigation |
|------|--------|-----------|
| IVE verification too slow for large programs | Blocks Phase 3+ | Invest early in incremental verification (Phase 2); profile and optimize hot paths continuously; use verification debt to prioritize |
| ARM64 instruction encoding bugs | Blocks all Pi 5 execution | Snapshot tests verified against ARM ARM; QEMU testing before hardware; per-instruction encoding verification |
| BD inference incompleteness | Blocks Phase 2+ | Subsumption test against Rust type system; fallback to explicit annotations; iterative refinement via profile feedback |
| Pi 5 hardware availability | Blocks Phase 3 peripheral testing | QEMU-based testing as primary; hardware testing as validation; raspi3b model as closest QEMU approximation |
| Concurrent verification undecidability | Blocks Phase 3 | Limit to finite-state abstraction; use tiered verification confidence; accept partial verification with explicit debt |
| Self-hosting complexity | Blocks Phase 5 | Incremental approach: self-verify subsystem by subsystem; use Rust-compiled compiler to verify each step |
| LSP performance | Blocks Phase 4 adoption | Incremental parsing and verification; cache verification results; use worker threads for IVE analysis |

---

## Success Criteria Summary

| Phase | Milestone Name | Key Metric | Status |
|-------|---------------|------------|--------|
| Phase 1 | Hello VUMA | Pi 5 boots and prints "VUMA OK" with IVE-verified ARM64 code | ✅ Complete |
| Phase 2 | Verified Data Structures | Doubly-linked list verified by IVE with no `unsafe` blocks | 🔄 In Progress |
| Phase 3 | Concurrent Pi 5 | Lock-free data structure on bare-metal Pi 5 with concurrent IVE verification | 📋 Planned |
| Phase 4 | VUMA IDE | LSP with diagnostics, go-to-definition, code actions on VUMA programs | 📋 Planned |
| Phase 5 | Self-Hosting | VUMA compiler verifies and compiles itself on Pi 5 | 📋 Planned |

---

*This roadmap is a living document. Phase assignments and timelines may be adjusted based on progress, and milestones may be reordered as dependencies are resolved. The phase structure ensures that each phase builds on verified foundations.*
