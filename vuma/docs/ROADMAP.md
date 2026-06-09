# VUMA Project Roadmap

This document describes the detailed wave-by-wave implementation plan for the VUMA project. The project is organized into 16 waves, each deploying up to 32 agents working in parallel. Each wave produces specific deliverables, and dependencies between waves are explicitly tracked.

---

## Overview

The VUMA project implements a six-layer AI-native programming language framework: (1) Semantic Computation Graph (SCG), (2) Inference and Verification Engine (IVE), (3) Projection System, (4) Continuous Optimization Runtime (COR), (5) Behavioral Descriptors (BD), and (6) Verified-Unsafe Memory Access (VUMA). The project targets the Raspberry Pi 5 (BCM2712, quad Cortex-A76) as its primary hardware platform.

The implementation follows a bottom-up strategy: foundational data structures and memory models are built first, then the verification engine, then the higher-level language features. Each wave builds on the output of previous waves, and no wave begins until its dependencies are satisfied.

---

## Phase 1: Foundation (Waves 1–4)

**Goal**: Establish the core data structures, memory model, and minimal ARM64 codegen. At the end of Phase 1, we can build a simple SCG, verify basic memory invariants, and emit ARM64 assembly that runs on QEMU or Pi 5 hardware.

---

### Wave 1: Core Data Structures and SCG Foundation

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: None (first wave)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W1-01 to W1-08 | 8 | SCG node types: `AllocationNode`, `FunctionCallNode`, `EffectNode`, `LiteralNode`, `ParameterNode`, `ReturnNode`, `BranchNode`, `PhiNode` |
| W1-09 to W1-12 | 4 | SCG edge types: `DataEdge`, `ControlEdge`, `AnnotationEdge`, `RegionEdge` |
| W1-13 to W1-16 | 4 | SCG graph structure: `SemanticComputationGraph` struct with add/remove/query operations, using `petgraph` |
| W1-17 to W1-20 | 4 | Annotation system: `Annotation` enum with type, constraint, invariant, and metadata variants |
| W1-21 to W1-24 | 4 | Region system: SCG region delineation, nesting, composition operators |
| W1-25 to W1-28 | 4 | SCG serialization: JSON serialization/deserialization via `serde`, round-trip property tests |
| W1-29 to W1-31 | 3 | Project infrastructure: Cargo.toml workspace, CI pipeline, documentation (glossary, conventions, contributing guide) |
| W1-32 | 1 | Integration: SCG construction API that ties all components together |

**Success Criteria**:
- All SCG node and edge types are defined with full doc comments and examples
- SCG can be constructed programmatically, serialized to JSON, and deserialized back
- Property tests verify that SCG operations are consistent (add-then-remove yields original graph)
- CI pipeline passes for all workspace members

---

### Wave 2: Memory Model and VUMA Primitives

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Wave 1 (SCG structure)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W2-01 to W2-06 | 6 | `Address` type: 64-bit address with derivation tracking, arithmetic operations, and display formatting |
| W2-07 to W2-12 | 6 | `Region` type: contiguous address range with allocation status, ownership context, access history, capability set |
| W2-13 to W2-18 | 6 | `Access` type: read/write operation targeting an address, with RepD interpretation and verification metadata |
| W2-19 to W2-24 | 6 | `MemoryStateGraph` (MSG): construction from SCG, tracking allocations, deallocations, pointer derivations, and accesses |
| W2-25 to W2-28 | 4 | Derivation chain tracking: every address traces back to an allocation via a chain of operations |
| W2-29 to W2-32 | 4 | MSG serialization and visualization: JSON export, DOT graph export for debugging |

**Success Criteria**:
- `Address`, `Region`, and `Access` types are fully implemented with doc comments
- MSG can be constructed from a simple SCG (single-function, no control flow)
- Derivation chains are correctly tracked from allocation to every derived address
- All types implement `Serialize`/`Deserialize` for persistence

---

### Wave 3: IVE Core — Liveness and Origin Verification

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Waves 1, 2 (SCG structure and MSG)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W3-01 to W3-08 | 8 | Liveness verification pass: check every access targets an allocated region |
| W3-09 to W3-16 | 8 | Origin verification pass: check every address traces back to a valid allocation |
| W3-17 to W3-20 | 4 | Verification result types: `LivenessResult`, `OriginResult`, `ViolationReport` |
| W3-21 to W3-24 | 4 | Counterexample generation: when verification fails, produce the execution path leading to the violation |
| W3-25 to W3-28 | 4 | Verification pipeline: composable pass architecture, pass ordering, result aggregation |
| W3-29 to W3-32 | 4 | IVE test suite: property tests for liveness and origin verification, including known-bad programs |

**Success Criteria**:
- Liveness pass correctly identifies use-after-free scenarios
- Origin pass correctly identifies phantom-pointer scenarios (hardcoded addresses, untrusted inputs)
- Counterexamples include the full execution path and are human-readable
- IVE pipeline runs liveness and origin passes in sequence and aggregates results
- Test suite covers at least 20 distinct failure scenarios for each invariant

---

### Wave 4: ARM64 Codegen — Core Instructions

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Wave 1 (SCG node types)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W4-01 to W4-04 | 4 | Instruction type definitions: `Arm64Instruction` enum with all core integer instructions (MOV, ADD, SUB, MUL, DIV, AND, ORR, EOR, LSL, LSR, ASR) |
| W4-05 to W4-08 | 4 | Memory instructions: LDR, STR, LDP, STP with all addressing modes (immediate, register, pre/post-index) |
| W4-09 to W4-12 | 4 | Branch instructions: B, BL, BR, BLR, B.cond, CBZ, CBNZ, TBZ, TBNZ |
| W4-13 to W4-16 | 4 | Instruction encoding: binary encoding for all implemented instructions, verified against ARM Architecture Reference Manual |
| W4-17 to W4-20 | 4 | Register allocator: basic linear-scan register allocator for aarch64 (x0–x30, sp, xzr) |
| W4-21 to W4-24 | 4 | SCG-to-ARM64 lowering: map SCG node types to ARM64 instruction sequences |
| W4-25 to W4-28 | 4 | Pi 5 boot code: minimal bare-metal startup (exception vectors, stack setup, UART init) |
| W4-29 to W4-32 | 4 | Codegen tests: snapshot tests for instruction encoding, integration tests for full lowering pipeline |

**Success Criteria**:
- All core A64 instructions have correct binary encodings (verified against ARM ARM)
- Register allocator handles at least 16 virtual registers without spilling
- A simple SCG program (factorial, Fibonacci) can be lowered to ARM64 assembly and executed under QEMU
- Pi 5 boot code initializes UART and prints "VUMA OK"

**Milestone 1: Phase 1 Complete** — At this point, the project can build an SCG, construct an MSG, verify liveness and origin invariants, and emit ARM64 code that runs on Pi 5 hardware. This is the "hello world" milestone: the VUMA framework produces its first verified, running program.

---

## Phase 2: Verification Engine (Waves 5–8)

**Goal**: Complete the VUMA invariant set, add Behavioral Descriptors, and implement the IVE's inference capabilities. At the end of Phase 2, the IVE can verify all five invariants for single-threaded programs with dynamic allocation.

---

### Wave 5: IVE — Exclusivity and Interpretation Verification

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Waves 2, 3 (MSG and IVE core)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W5-01 to W5-08 | 8 | Exclusivity verification pass: check no overlapping simultaneous write access |
| W5-09 to W5-16 | 8 | Interpretation verification pass: check every access interprets bytes according to a valid RepD |
| W5-17 to W5-20 | 4 | RepD validation: check that representation descriptors are consistent with region metadata |
| W5-21 to W5-24 | 4 | Uninitialized memory detection: flag reads of regions that have not been written to |
| W5-25 to W5-28 | 4 | Verification result types: `ExclusivityResult`, `InterpretationResult` |
| W5-29 to W5-32 | 4 | Integration tests: programs with data races, type confusion, and uninitialized reads |

**Success Criteria**:
- Exclusivity pass catches all simple data races in single-threaded programs (aliasing through multiple pointers)
- Interpretation pass catches type confusion (reading integer bytes as pointer, reading uninitialized memory)
- Uninitialized memory detection covers stack and heap allocations
- All five VUMA invariants are now verifiable (liveness, exclusivity, interpretation, origin, cleanup)

---

### Wave 6: IVE — Cleanup Verification and Full Invariant Pipeline

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Waves 3, 5 (IVE core and exclusivity/interpretation)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W6-01 to W6-08 | 8 | Cleanup verification pass: check every allocated region is eventually freed or intentionally leaked |
| W6-09 to W6-12 | 4 | Intentional leak annotation: mechanism to mark regions as intentionally leaked (arenas, globals) |
| W6-13 to W6-16 | 4 | Full invariant pipeline: run all five passes (liveness, exclusivity, interpretation, origin, cleanup) in optimal order |
| W6-17 to W6-20 | 4 | Verification summary: `VerificationSummary` with pass/fail per invariant, total verification debt |
| W6-21 to W6-24 | 4 | Incremental verification: re-verify only affected subgraphs when the SCG changes |
| W6-25 to W6-28 | 4 | Verification debt tracking: maintain and report the set of unverified properties |
| W6-29 to W6-32 | 4 | Benchmark programs: doubly-linked list, tree, hash map, arena allocator — all verified by IVE |

**Success Criteria**:
- Cleanup pass correctly identifies memory leaks and respects intentional leak annotations
- Full pipeline runs all five passes and produces a comprehensive `VerificationSummary`
- Incremental verification re-verifies in under 1 second for single-function edits
- Doubly-linked list implementation requires no `unsafe` blocks (VUMA-VERIFIED instead)
- Benchmark suite covers at least 5 data structures

---

### Wave 7: Behavioral Descriptors — RepD, CapD, RelD

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Waves 1, 2 (SCG and memory model)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W7-01 to W7-08 | 8 | `RepresentationDescriptor` (RepD): size, alignment, field offsets, bit-level structure, multiple simultaneous interpretations |
| W7-09 to W7-16 | 8 | `CapabilityDescriptor` (CapD): permission set (read, write, execute, serialize, send, persist, derive-pointer), context-dependent capability sets |
| W7-17 to W7-24 | 8 | `RelationalDescriptor` (RelD): temporal co-occurrence, structural containment, dependency ordering, semantic equivalence, security-level flow |
| W7-25 to W7-28 | 4 | `BehavioralDescriptor` triple: composition of (RepD, CapD, RelD), equality and subtyping rules |
| W7-29 to W1-32 | 4 | BD serialization and property tests: JSON round-trip, BD equivalence under structural equality |

**Success Criteria**:
- All three BD components are fully implemented with doc comments and examples
- A value can have multiple simultaneous RepD interpretations (e.g., same bytes viewed as `uint32` and `float32`)
- CapD context-switching works: the same value has different capability sets in different SCG regions
- RelD can express "must not outlive" and "security level derived from" relationships
- BD equality is structural: two BDs with the same (RepD, CapD, RelD) are equal regardless of provenance

---

### Wave 8: IVE — BD Inference

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Waves 3, 7 (IVE core and BD types)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W8-01 to W8-08 | 8 | RepD inference: derive representation descriptors from SCG structure (allocation sizes, field accesses, cast operations) |
| W8-09 to W8-16 | 8 | CapD inference: derive capability sets from how a value is used (read, written, sent, persisted) |
| W8-17 to W8-24 | 8 | RelD inference: derive relationships from SCG edges (data flow, ownership, security boundaries) |
| W8-25 to W8-28 | 4 | BD consistency checking: verify that inferred BDs are internally consistent and compatible with explicit annotations |
| W8-29 to W8-32 | 4 | BD subsumption testing: verify that BD inference subsumes traditional type inference (every Rust-typable program has a valid BD assignment) |

**Success Criteria**:
- IVE infers correct RepDs for all benchmark programs without manual annotation
- CapD inference correctly identifies context-dependent capability changes (e.g., buffer becomes read-only after sealing)
- RelD inference captures "must not outlive" and "derived from" relationships
- BD consistency checker rejects invalid BDs (e.g., write capability on a read-only region)
- Subsumption test: all Rust type-check programs produce valid BDs

**Milestone 2: Phase 2 Complete** — The IVE can verify all five VUMA invariants for single-threaded programs, infer complete Behavioral Descriptors, and produce verification summaries. This is the "verified singly-linked list" milestone: the first non-trivial data structure verified entirely by the IVE with no manual annotations.

---

## Phase 3: Concurrency and Codegen Expansion (Waves 9–12)

**Goal**: Add concurrency support, expand ARM64 codegen to include atomics and barriers, and implement the Continuous Optimization Runtime. At the end of Phase 3, the IVE can verify multi-threaded programs and the codegen can produce efficient ARM64 code for concurrent algorithms.

---

### Wave 9: ARM64 Codegen — Atomics, Barriers, and Concurrency Primitives

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Wave 4 (core codegen)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W9-01 to W9-04 | 4 | Atomic instructions: LDXR, STXR, CAS, LDADD, STADD with all sizes and ordering variants |
| W9-05 to W9-08 | 4 | Barrier instructions: DMB, DSB, ISB with all option variants |
| W9-09 to W9-12 | 4 | Exclusive access modeling: IVE model of LDXR/STXR exclusive monitors for exclusivity verification |
| W9-13 to W9-16 | 4 | Lock-free data structure codegen: atomic counter, spinlock, MCS lock lowering |
| W9-17 to W9-20 | 4 | Thread management: thread spawn/join lowering via Pi 5 bare-metal scheduler stubs |
| W9-21 to W9-24 | 4 | AAPCS64 compliance: verify all calling conventions, stack alignment, callee-saved register preservation |
| W9-25 to W9-28 | 4 | Codegen for concurrent SCG nodes: `ForkNode`, `JoinNode`, `SyncNode`, `ChannelSendNode`, `ChannelRecvNode` |
| W9-29 to W9-32 | 4 | Concurrency codegen tests: spinlock, producer-consumer, dining philosophers |

**Success Criteria**:
- LDXR/STXR pair encodes correctly and runs on QEMU
- DMB/DSB/ISB instructions encode correctly with all option fields
- Lock-free atomic counter passes IVE exclusivity verification
- Spinlock implementation runs correctly under concurrent QEMU execution
- All calling conventions comply with AAPCS64

---

### Wave 10: IVE — Concurrent Verification

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Waves 5, 9 (exclusivity verification and concurrency codegen)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W10-01 to W10-08 | 8 | Concurrent exclusivity verification: extend exclusivity pass to handle LDXR/STXR, locks, and channel-based synchronization |
| W10-09 to W10-16 | 8 | Happens-before analysis: build a happens-before graph from synchronization operations (locks, barriers, channel sends/receives) |
| W10-17 to W10-20 | 4 | Thread-local MSG: partition the MSG by thread and verify per-thread invariants before cross-thread analysis |
| W10-21 to W10-24 | 4 | Deadlock detection: detect potential deadlocks in lock acquisition order |
| W10-25 to W10-28 | 4 | Liveness verification for concurrent programs: verify that every lock is eventually released, every channel send is eventually received |
| W10-29 to W10-32 | 4 | Concurrent verification tests: data race detection, deadlock detection, lock-free algorithm verification |

**Success Criteria**:
- IVE correctly identifies data races in multi-threaded programs
- Happens-before graph is correctly constructed from lock and channel operations
- Deadlock detection identifies circular lock acquisition orders
- Lock-free atomic counter passes concurrent exclusivity verification
- Test suite covers at least 10 concurrent scenarios (races, deadlocks, livelocks)

---

### Wave 11: Continuous Optimization Runtime (COR)

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Waves 1, 4 (SCG and codegen)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W11-01 to W11-06 | 6 | Incremental compilation: recompile only affected SCG subgraphs when edits occur |
| W11-07 to W11-12 | 6 | Profile data collection: runtime execution counters for edge traversal, node execution, cache misses |
| W11-13 to W11-18 | 6 | Profile-guided optimization: hot path identification, aggressive optimization for hot paths, size optimization for cold paths |
| W11-19 to W11-22 | 4 | Speculative optimization: pre-optimize likely execution paths with transparent fallback on mis-speculation |
| W11-23 to W11-26 | 4 | Optimization passes: dead code elimination, constant folding, inlining, loop unrolling (operating on SCG) |
| W11-27 to W11-30 | 4 | SCG transformation verification: prove that each optimization pass preserves semantics (verified by IVE) |
| W11-31 to W11-32 | 2 | COR integration tests: verify that optimized programs produce the same results as unoptimized programs |

**Success Criteria**:
- Incremental compilation recompiles in under 500ms for single-function edits
- Profile-guided optimization improves benchmark performance by at least 15% over unoptimized codegen
- All optimization passes are verified to preserve semantics by the IVE
- Speculative optimization correctly falls back when speculation is wrong

---

### Wave 12: Pi 5 Platform — Full Peripheral Support

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Waves 4, 9 (codegen and concurrency primitives)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W12-01 to W12-04 | 4 | BCM2712 memory map: define all peripheral base addresses, register layouts, and access permissions |
| W12-05 to W12-08 | 4 | GPIO driver: memory-mapped GPIO register access with read/write capability descriptors |
| W12-09 to W12-12 | 4 | UART driver: PL011 UART and mini UART with interrupt support |
| W12-13 to W12-16 | 4 | Interrupt controller: GIC-400 (or BCM2712 equivalent) driver with interrupt routing |
| W12-17 to W12-20 | 4 | Timer driver: ARM generic timer and BCM2712 system timer |
| W12-21 to W12-24 | 4 | DMA controller: BCM2712 DMA engine driver with cache coherency management |
| W12-25 to W12-28 | 4 | PCIe driver: BCM2712 PCIe 2.0 root complex for NVMe and network |
| W12-29 to W12-32 | 4 | Platform integration test: boot, initialize all peripherals, run a concurrent workload, verify via UART output |

**Success Criteria**:
- All Pi 5 peripheral drivers compile and run on QEMU (where applicable) and real hardware
- GPIO can toggle an LED (blink test)
- UART produces reliable console output at 115200 baud
- Interrupt controller handles timer and UART interrupts without drops
- Full boot sequence completes in under 2 seconds on Pi 5 hardware

**Milestone 3: Phase 3 Complete** — The VUMA framework can verify concurrent programs, produce optimized ARM64 code, and run on real Pi 5 hardware with full peripheral support. This is the "concurrent Pi 5" milestone: a lock-free data structure running on bare-metal Pi 5, verified by the IVE, with UART output.

---

## Phase 4: Language Features and Projections (Waves 13–16)

**Goal**: Implement the Projection System, outcome spaces, parser, and standard library. At the end of Phase 4, VUMA is a complete, usable framework with human-facing projections and a growing ecosystem.

---

### Wave 13: Projection System — Textual and Visual

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Waves 1, 7 (SCG and BD)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W13-01 to W13-08 | 8 | Textual projection: render SCG as human-readable code-like text, with role-specific views (systems programmer, domain expert, security auditor) |
| W13-09 to W13-16 | 8 | Bidirectional editing: parse textual projections back into SCG modifications, validate via IVE before applying |
| W13-17 to W13-22 | 6 | Visual projection: render SCG as dataflow diagram, call graph, and memory layout view (SVG/HTML output) |
| W13-23 to W13-26 | 4 | Diff projection: compute and render semantic diffs between SCG versions in human terms |
| W13-27 to W13-30 | 4 | Projection framework: abstract `Projection` trait with `render()` and `parse()` methods, pluggable projection backends |
| W13-31 to W13-32 | 2 | Projection integration tests: round-trip property (SCG → projection → SCG preserves semantics) |

**Success Criteria**:
- Textual projection renders benchmark programs as readable, syntax-highlighted code
- Bidirectional editing: modifying the projection and applying it yields a semantically equivalent or explicitly different SCG
- Visual projection produces interactive SVG/HTML dataflow diagrams
- Diff projection describes changes in human terms, not line-level diffs
- Round-trip property holds for all benchmark programs

---

### Wave 14: Outcome Spaces and Error Handling

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Waves 7, 8 (BD and BD inference)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W14-01 to W14-08 | 8 | Outcome space types: structured enumeration of all possible outcomes (success, validation errors, timeouts, resource exhaustion) |
| W14-09 to W14-16 | 8 | Outcome space inference: IVE infers the complete outcome space of each function from its SCG structure |
| W14-17 to W14-22 | 6 | Exhaustive handling verification: IVE proves that every outcome is handled explicitly or by a verified safe default |
| W14-23 to W14-26 | 4 | Safe default inference: IVE infers safe default handlers for common unhandled outcomes (retry for timeout, backoff for resource exhaustion) |
| W14-27 to W14-30 | 4 | Outcome space narrowing: as invariants are established, the outcome space shrinks (e.g., post-authentication removes "unauthorized") |
| W14-31 to W14-32 | 2 | Outcome space integration tests: web service simulation with all failure modes |

**Success Criteria**:
- Outcome space inference correctly enumerates all possible outcomes for benchmark programs
- Exhaustive handling verification catches unhandled failure modes
- Safe default inference produces correct retry/backoff behavior for timeout and resource exhaustion
- Outcome space narrowing works: the IVE removes impossible outcomes after invariant establishment
- Test suite covers at least 5 programs with multiple failure modes

---

### Wave 15: Parser and SCG Construction from Text

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: Waves 1, 13 (SCG and textual projection)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W15-01 to W15-08 | 8 | Lexer: tokenize VUMA textual syntax (keywords, identifiers, operators, literals, BD annotations) |
| W15-09 to W15-16 | 8 | Parser: parse token stream into SCG, handling expressions, control flow, function definitions, BD annotations |
| W15-17 to W15-20 | 4 | Error recovery: parse errors produce helpful messages with suggestions, not just "unexpected token" |
| W15-21 to W15-24 | 4 | Incremental parsing: re-parse only changed portions of the text, maintaining the SCG-to-text mapping |
| W15-25 to W15-28 | 4 | Parser tests: comprehensive test suite with valid and invalid programs, error message quality checks |
| W15-29 to W15-32 | 4 | Parser benchmarks: parse speed for programs of varying size (10 nodes, 100 nodes, 1000 nodes, 10000 nodes) |

**Success Criteria**:
- Parser correctly produces SCG from textual syntax for all benchmark programs
- Error messages are helpful and suggest corrections for common mistakes
- Incremental parsing re-parses in under 100ms for single-statement edits
- Parser handles at least 100 distinct syntax forms
- Parse speed is under 10ms for 1000-node programs

---

### Wave 16: Standard Library and Ecosystem

**Duration**: 1 sprint  
**Agents**: 32  
**Dependencies**: All previous waves (complete framework)

| Agent Group | Count | Deliverable |
|-------------|-------|-------------|
| W16-01 to W16-04 | 4 | `vuma_std::mem`: allocation, deallocation, copy, fill, zero — all VUMA-VERIFIED |
| W16-05 to W16-08 | 4 | `vuma_std::collections`: Vec, LinkedList, HashMap, BTreeMap — all VUMA-VERIFIED, no unsafe |
| W16-09 to W16-12 | 4 | `vuma_std::sync`: Mutex, RwLock, Channel, AtomicU32, AtomicU64 — all VUMA-VERIFIED |
| W16-13 to W16-16 | 4 | `vuma_std::io`: Read, Write, BufRead traits with UART and Pi 5 peripheral implementations |
| W16-17 to W16-20 | 4 | `vuma_std::fmt`: Display, Debug formatting with UART output backend |
| W16-21 to W16-24 | 4 | `vuma_std::pi5`: Pi 5 platform abstractions (GPIO, UART, Timer, Interrupt, DMA) |
| W16-25 to W16-28 | 4 | `vuma_std::bd`: BD construction helpers, RepD/CapD/RelD builders, BD annotation macros |
| W16-29 to W16-32 | 4 | Ecosystem examples: HTTP server, key-value store, sensor reader, real-time signal processor — all running on Pi 5 |

**Success Criteria**:
- All stdlib functions are VUMA-VERIFIED (no IVE-TODO items remaining in stdlib)
- LinkedList implementation requires no `unsafe` blocks (the VUMA equivalent of Rust's "unsafe doubly-linked list")
- HashMap passes all IVE invariant checks including concurrent access via Mutex
- At least 4 ecosystem examples run on Pi 5 hardware with UART output
- Full stdlib documentation with examples for every public function

**Milestone 4: Phase 4 Complete** — VUMA is a complete, usable framework with a standard library, parser, projection system, and ecosystem examples. This is the "VUMA 1.0" milestone: a self-hosting framework that can verify its own standard library, parse programs from text, render them as human-readable projections, and run verified ARM64 code on Pi 5 hardware.

---

## Dependency Graph

```
Wave 1 ──┬── Wave 2 ──┬── Wave 3 ──┬── Wave 5 ── Wave 10
         │             │             │
         │             └── Wave 6 ──┘
         │
         ├── Wave 4 ──┬── Wave 9  ── Wave 10
         │             └── Wave 12
         │
         └── Wave 7 ── Wave 8 ── Wave 14

Wave 1 ── Wave 4 ── Wave 11
Wave 1 ── Wave 7 ── Wave 13 ── Wave 15
All waves ── Wave 16
```

**Critical path**: Wave 1 → Wave 2 → Wave 3 → Wave 5 → Wave 10 (concurrent verification is the longest sequential dependency chain).

---

## Success Criteria Summary

| Phase | Milestone | Key Metric |
|-------|-----------|------------|
| Phase 1 | Hello VUMA | Pi 5 boots and prints "VUMA OK" with IVE-verified ARM64 code |
| Phase 2 | Verified Data Structures | Doubly-linked list verified by IVE with no `unsafe` blocks |
| Phase 3 | Concurrent Pi 5 | Lock-free data structure running on bare-metal Pi 5 with concurrent IVE verification |
| Phase 4 | VUMA 1.0 | Self-hosting framework with stdlib, parser, projections, and ecosystem examples |

---

## Risk Mitigation

| Risk | Impact | Mitigation |
|------|--------|-----------|
| IVE verification too slow for large programs | Blocks Phase 3+ | Invest early in incremental verification (Wave 6); profile and optimize hot paths continuously |
| ARM64 instruction encoding bugs | Blocks all Pi 5 execution | Snapshot tests verified against ARM ARM; QEMU testing before hardware |
| BD inference incompleteness | Blocks Phase 2+ | Subsumption test against Rust type system; fallback to explicit annotations |
| Pi 5 hardware availability | Blocks Wave 12+ | QEMU-based testing as primary; hardware testing as validation |
| Concurrent verification undecidability | Blocks Phase 3 | Limit to finite-state abstraction; use tiered verification confidence |

---

*This roadmap is a living document. Wave assignments may be adjusted based on progress, and agents may be reassigned between waves as needed. The dependency structure ensures that no wave begins before its prerequisites are satisfied.*
