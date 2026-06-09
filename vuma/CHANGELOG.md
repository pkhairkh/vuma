# VUMA Changelog

All notable changes to the VUMA project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.1.0] — 2026-03-05

Initial release of the VUMA framework — Verified-Unsafe Memory Access AI-Native Programming Language.

---

## Wave 1: Foundation & Formal Specifications

*The first wave established the mathematical foundations, formal specifications, and initial implementations of all 12 workspace crates.*

### Added — Formal Specifications

- **SCG Formal Specification** (`docs/specs/scg-formal-spec.md`, 475 lines) — Mathematical model for the Semantic Computation Graph: directed, acyclic, attributed multigraph with nodes (allocation, access, deallocation, cast, computation, control, effect, phantom), edges (data flow, control flow, derivation, annotation), and regions (scopes, phases, security boundaries, deployment targets).

- **RepD Formal Specification** (`docs/specs/repd-formal-spec.md`, 546 lines) — Representation descriptor lattice with 7 variants (Byte, Struct, Enum, Array, Pointer, Union, Opaque), subsumption ordering, compatibility checking, and multiple simultaneous interpretations.

- **CapD Formal Specification** (`docs/specs/capd-formal-spec.md`, 492 lines) — Capability descriptor lattice with meet/join operations, context-dependent capability sets (read, write, execute, serialize, send, persist, derive-pointer), and lock conditions for concurrent access.

- **RelD Formal Specification** (`docs/specs/reld-formal-spec.md`, 600 lines) — Relational descriptor kinds (Containment, Aliasing, DataFlow, RegionBound, Ownership, SecurityLevel), composition rules, and refinement ordering.

- **VUMA Invariants Specification** (`docs/specs/vuma-invariants-spec.md`, 742 lines) — Five global memory-safety invariants: liveness (every access targets allocated memory), exclusivity (no conflicting concurrent accesses), interpretation (every access uses a valid RepD), origin (every address traces to a valid allocation), cleanup (every region is eventually freed or explicitly leaked).

- **MSG Construction Specification** (`docs/specs/msg-construction-spec.md`, 850 lines) — Memory State Graph construction algorithm from annotated SCGs: monotonic address allocation, derivation chain tracking, sync edge construction, and post-conversion verification.

- **Pi 5 Memory Model Specification** (`docs/specs/pi5-memory-model-spec.md`, 809 lines) — BCM2712 address map, MMIO register access, DMA controller, cache coherency protocol, and ARM local register space.

- **Security Model Specification** (`docs/specs/security-model-spec.md`, 606 lines) — Five security layers (memory safety, capability security, information flow, region security, platform security), six threat categories, and verification confidence/debt tracking.

- **BD Inference Algorithm** (`docs/specs/bd-inference-algorithm.md`, 1027 lines) — Iterative fixpoint algorithm for RepD, CapD, and RelD inference with interdependency resolution.

- **VUMA Verification Algorithm** (`docs/specs/vuma-verification-algorithm.md`, 1098 lines) — Five invariant verification algorithms with proof obligations, counterexample generation, and tiered confidence levels.

- **ARM64 Codegen Algorithm** (`docs/specs/arm64-codegen-algorithm.md`, 1182 lines) — Three-phase codegen pipeline (SCG→IR, register allocation, emission), instruction selection rules, and AAPCS64 compliance.

- **Benchmark Design** (`docs/specs/benchmark-design.md`, 695 lines) — Eight benchmark categories, statistical methodology (mean, median, stddev, P95, CV), and C-equivalent comparison baseline.

- **Trivial Proofs** (`docs/specs/trivial-proofs.md`, 547 lines) — Proof obligations for trivial programs: allocate/read/write/free, cast operations, and concurrent access patterns.

- **Doubly-Linked List Proof** (`docs/specs/dlist-proof.md`, 631 lines) — Formal proof of the doubly-linked list with sentinel node pattern, insertion/deletion invariants, and threading safety.

- **Decidability Analysis** (`docs/specs/decidability-analysis.md`, 416 lines) — Which VUMA invariants are decidable, which require approximation, and how verification debt handles undecidable cases.

### Added — Source Crates (Initial Implementation)

- **`vuma-scg`** (`src/scg/`, ~10,268 lines) — Semantic Computation Graph: 12 node types, 8 edge kinds, region system, query engine, dominance analysis, liveness analysis, transform passes (DCE, constant folding, inlining, CSE), diff/merge, and JSON serialization.

- **`vuma-bd`** (`src/bd/`, ~10,073 lines) — Behavioral Descriptors: RepD (7 variants), CapD (lattice operations), RelD (6 relation kinds), BD triple (composition, compatibility, refinement), inference engine, context solver, capd_lattice, reld_refine, repd_compat, and BD unification.

- **`vuma-core`** (`src/vuma/`, ~16,204 lines) — VUMA Memory Model: Address newtype, Region (contiguous span with status), Derivation (pointer provenance), Access (read/write at program point), SyncEdge (ordering), MSG (Memory State Graph), msg_builder, msg_incremental (MSGDelta), scg_to_msg conversion pipeline, five invariant checkers, access_analysis, security model, and REPL.

- **`vuma-ive`** (`src/ive/`, ~12,500 lines) — Inference & Verification Engine: InferenceEngine (BD propagation), VerificationEngine (5 invariant checks), InvariantAggregator (unified VerificationSummary), individual verifiers (liveness, exclusivity, interpretation, origin, cleanup), BD constraint solver, verification debt tracking, and result types.

- **`vuma-cor`** (`src/cor/`, ~6,244 lines) — Continuous Optimization Runtime: CORuntime orchestrator, ProfileCollector (thread-safe, Pi 5 PMU counters), SpeculativeExecutor (branch prediction, inlining, code motion, snapshot rollback), OptimizationEngine (DCE, folding, inlining, loop unrolling), DeploymentManager (hot-swap 6-phase FSM, delta deployment, version tracking), and Config.

- **`vuma-projection`** (`src/projection/`, ~8,090 lines) — Projection System: textual (SCG → code), visual (SCG → SVG/HTML), conversational (SCG → natural language), bidirectional editing (projection edits → SCG), and semantic diff.

- **`vuma-parser`** (`src/parser/`, ~9,461 lines) — Parser/Frontend: lexer (43+ keywords), recursive-descent parser, AST types, AST→SCG lowering, and error recovery with "did you mean?" suggestions.

- **`vuma-codegen`** (`src/codegen/`, ~11,879 lines) — ARM64 Code Generation: Arm64Instruction enum with binary encoding, IR types, SCG→IR translation, linear-scan register allocator, and ELF emission.

- **`vuma-pi5`** (`src/pi5/`, ~6,120 lines) — Raspberry Pi 5 Platform: bare-metal boot (exception vectors, _start, FDT parsing), linker script, build script, UART driver (PL011 + Mini UART), GPIO, timer, MMIO primitives, and SMP (multicore boot, Spinlock).

- **`vuma-proof`** (`src/proof/`, ~9,124 lines) — Formal Proof System: Proof/ProofStep/Goal/ProofStatus, checker, inference rules, automated tactics, counterexample generation, and per-invariant proof modules.

- **`vuma-std`** (`src/std/`, ~10,303 lines) — Standard Library: Ptr, RegionPtr, Slice, VumaResult, VumaOption, Range, HasBD trait, VumaAllocator, BumpAllocator, FreeListAllocator, Vec, HashMap, VumaString, LinkedList, RingBuffer, SipHash13, Mutex, RwLock, Channel, AtomicU32/64, Read/Write/BufRead traits.

- **`vuma-tests`** (`src/tests/`, ~3,962 lines) — Integration Tests & Benchmarks: test framework, trivial program tests, doubly-linked list tests, BD inference tests, concurrent verification tests, graph tests, and benchmark suite (8 categories, 40+ benchmarks).

### Added — Examples

- `examples/hello_memory.vuma` (40 lines) — Basic allocate/write/read/free
- `examples/doubly_linked_list.vuma` (89 lines) — Sentinel node pattern
- `examples/arena_allocator.vuma` (78 lines) — Arena allocation with derivation chains
- `examples/gpio_blink.vuma` (68 lines) — Pi 5 GPIO hardware access
- `examples/lock_free_queue.vuma` (99 lines) — Lock-free SPSC queue with atomics

### Added — Build & CI

- `Makefile` (233 lines) — Build/test/bench/doc/Pi 5 cross-compile/flash/debug targets
- `justfile` (226 lines) — Just command runner shortcuts
- `rust-toolchain.toml` (9 lines) — Pinned nightly toolchain
- `rustfmt.toml` (3 lines) — Formatting configuration
- `clippy.toml` (1 line) — Cognitive complexity threshold
- `.cargo/config.toml` (58 lines) — Cross-compilation and target-specific flags
- `.github/workflows/ci.yml` (217 lines) — GitHub Actions CI pipeline

### Added — Documentation

- `docs/architecture.md` (994 lines) — Full architecture document
- `docs/ROADMAP.md` (277 lines) — 5-phase project roadmap
- `docs/CONTRIBUTING.md` (840 lines) — Contributor guidelines
- `docs/CONVENTIONS.md` (796 lines) — Coding conventions
- `docs/GLOSSARY.md` (893 lines) — Project glossary

---

## Wave 2: Core Verification & Pi 5 Platform

*The second wave completed the five invariant verification passes, built the SCG→MSG conversion pipeline, and established the Pi 5 bare-metal platform with boot code, linker script, and hardware drivers.*

### Added — Verification Pipeline

- **SCG → MSG conversion** (`src/vuma/src/scg_to_msg.rs`, 1357 lines) — Topological walk of SCG nodes producing well-formed Memory State Graphs: AllocationNode→Region, AccessNode→Derivation+Access, DeallocationNode→Region Freed, CastNode→DerivationKind::Cast, ControlFlow edges→SyncEdge. 14 tests.

- **Incremental MSG** (`src/vuma/src/msg_incremental.rs`, 1907 lines) — MSGDelta computation and application for incremental re-verification: compute_delta, apply_delta, SCGSnapshot.

- **Invariant aggregator** (`src/ive/src/invariant_aggregator.rs`, 1141 lines) — Unified verification pipeline running all five invariant checks and producing VerificationSummary.

### Added — Pi 5 Bare-Metal Platform

- **Boot code** (`src/pi5/src/boot.rs`, 746 lines) — ARM64 exception vector table (16 entries), `_start` naked function (core ID check, BSS zeroing, stack setup, FDT parsing), `boot_main` high-level entry. 7 tests.

- **Linker script** (`src/pi5/link.ld`, 101 lines) — ARM64 linker script: ENTRY(_start), RAM region at 0x80000, MMIO window, section ordering, per-core stacks, symbol exports.

- **Build script** (`src/pi5/build.rs`, 47 lines) — Cargo build script for bare-metal aarch64-unknown-none: passes -Tlink.ld, -nostartfiles, -static, -no-pie.

- **UART driver** (`src/pi5/src/uart.rs`, 1700 lines) — PL011 UART0 driver, MiniUart (UART1), UartBuffer ring buffer, ISR handlers, free-standing C API. 16 tests.

- **GPIO driver** (`src/pi5/src/gpio.rs`, 1571 lines) — Memory-mapped GPIO pin function and pull control with BCM2712 register layout.

- **Timer driver** (`src/pi5/src/timer.rs`, 484 lines) — ARM generic timer (physical + virtual), virtual timer interrupt control, free-standing C API. 10 tests.

- **MMIO subsystem** (`src/pi5/src/mmio.rs`, 541 lines) — Pi 5 memory map constants, u64 address type, named 32/64-bit volatile accessors, ARM64 memory barriers (dmb, dsb, isb), MmioDevice trait. 10 tests.

- **SMP support** (`src/pi5/src/smp.rs`, 525 lines) — Multicore boot, IPI doorbell, Spinlock with RAII guard, core-start tracking. 12 tests.

### Added — Makefile Pi 5 Targets

- `make pi5` — Cross-compile for bare-metal aarch64
- `make pi5-image` — Build kernel8.img from ELF
- `make pi5-flash` — Flash to SD card boot partition
- `make pi5-debug` — Launch QEMU with GDB stub
- `make pi5-run` — Run in QEMU without debug

---

## Wave 3: Standard Library & COR Enhancement

*The third wave completed the standard library primitives, enhanced the COR with profile collection and speculative optimization, and strengthened the deployment system with hot-swap and delta deployment.*

### Added — Standard Library Primitives

- **RelD** (`src/std/src/primitives.rs`) — New Relational Descriptor type with RelKind enum (Containment, Liveness, Aliasing, DataFlow, RegionBound, Ownership), compose, refine, intersect operations, and factory functions for ptr/region_ptr/slice/result/option/numeric.

- **BD triple** — Behavioral Descriptor combining RepD × CapD × RelD with compatible() and refines() methods.

- **HasBD trait** — Unified interface for types that produce a BD.

- **Ptr\<T\>** — VUMA pointer with embedded BD annotation (addr, pointee_bd, offset, null check).

- **RegionPtr\<T\>** — Pointer bound to a memory region with in_bounds/checked_offset.

- **Slice\<T\>** — Pointer + length with BD annotation and subslice.

- **VumaResult\<T, E\>** / **VumaOption\<T\>** — Result and Option types with BD tracking.

- **Range** — Integer range type (start..end) with Contains and Iterate capabilities.

### Added — COR Enhancements

- **Pi5PmuCounters** (`src/cor/src/profile.rs`, 978 lines) — Hardware performance counter snapshot: cycle count, instruction count, cache misses, branch misses, IPC, miss rates. Thread-safe ProfileCollector. collect_profile analysis entry point with HotPath identification. 11 tests.

- **SpeculativeExecutor** (`src/cor/src/speculative.rs`, 1487 lines) — Three-phase lifecycle (identify/apply/validate-and-rollback), BranchPredictionTable, SpeculativeInlining, SpeculativeCodeMotion, Snapshot-based rollback. 19 tests.

- **DeploymentManager** (`src/cor/src/deployment.rs`, 1423 lines) — 6-phase HotSwap state machine (Idle→PreparingShadow→AwaitingSafePoint→Swapping→Completed/Failed), DeploymentDelta with block-level binary diffing, PackageVersion with CRC32 checksums, VersionLog with rollback support. 18 tests.

### Added — More Examples

- `examples/channel_demo.vuma` (237 lines) — Channel-based concurrency demo
- `examples/memory_arena.vuma` (197 lines) — Region-based allocation
- `examples/pi5_sensor.vuma` (188 lines) — Pi 5 MMIO sensor reading
- `examples/sorted_map.vuma` (192 lines) — Sorted map data structure
- `examples/thread_pool.vuma` (209 lines) — Thread pool with work stealing

---

## Wave 4: Parser, Collections, & Benchmarks

*The fourth wave enhanced the parser with comprehensive error recovery, expanded the standard library collections with VumaString and SipHash, and created the benchmark suite.*

### Added — Parser Error Recovery

- **Enhanced ParseErrorKind** (`src/parser/src/error.rs`, 1371 lines) — 8 new error kinds (UnexpectedToken, ExpectedToken, InvalidSyntax, DuplicateDefinition, UndefinedReference, TypeMismatch, RegionError, BDAnnotationError) plus 3 legacy aliases.

- **ErrorRecovery** — 5 strategies: SkipToStatementBoundary, SkipToBlockBoundary, InsertMissingToken, SkipOneToken, AbortItem. Default mapping from ParseErrorKind.

- **ParseResult\<T\>** — Partial-success result type carrying value + accumulated errors for IDE-style "parse as you type" support.

- **Diagnostic/Severity** — Structured diagnostic reporting with error, warning, note levels, source locations, and child annotations.

- **ErrorCollector** — Accumulates multiple diagnostics with deduplication and rendering.

- **"Did you mean?" suggestions** — Levenshtein distance-based keyword suggestions from 43 VUMA keywords. 29 tests.

### Added — Collections & Allocator Enhancement

- **VumaString** (`src/std/src/collections.rs`, 2293 lines) — UTF-8 string type backed by Vec\<u8\> with BD annotations, push/pop/chars iterators. CapD: {Read, Write, Iterate, Compare, Hash, Serialize, Send}.

- **SipHasher13** — SipHash 1-3 hasher (1 compression round, 3 finalization rounds) for DoS-resistant, auditable hashing in HashMap.

- **Iterator types** — VecIter, VecIterMut, VecIntoIter, VumaStringChars, HashMapIter, HashMapKeys, HashMapValues with CapD annotations.

- **BD tracking** — Per-operation BD statistics on Vec (push/pop/get/get_mut counts via Cell\<u64\>) and HashMap (insert/remove/get counts).

- **VumaAllocator enhancements** — tracker() for MSG data snapshots, active_allocations() thread-safe count, AlignedHeap\<N\> for 8-byte aligned test heaps. 39 tests total.

### Added — Benchmark Suite

- **8 benchmark categories** (`src/tests/src/benchmarks.rs`, 1162 lines) with 40+ individual benchmarks:
  1. SCG construction (99–9999 nodes)
  2. BD inference (3 sizes × 3 operations)
  3. MSG construction (60–3000 nodes)
  4. IVE verification (per-invariant + verification levels + incremental)
  5. ARM64 codegen (statement + function counts)
  6. C-equivalent comparison
  7. Memory usage (5 measurement points × 3 sizes)
  8. End-to-end pipeline

- **BenchmarkStats** — Aggregated statistics: mean, median, stddev, min, max, P95, CV. CV > 5% flagged as unreliable. 20 tests.

---

## Wave 5: Documentation & Project Packaging

*The fifth wave produced comprehensive documentation, formalized the project structure, and created the final packaging artifacts.*

### Added — Documentation

- **Architecture Document** (`docs/architecture.md`, 994 lines) — Complete rewrite with 8 major sections: System Overview, Data Flow Diagram, Crate Dependency Graph, Key Data Structures, Verification Pipeline, Code Generation Pipeline, Runtime Optimization Pipeline, Security Model Overview.

- **Language Reference** (`docs/language-reference.md`, 1101 lines) — Complete VUMA language reference with 11 sections: Lexical Structure, Types/BD, Memory Model, Pointer Operations, Control Flow, Functions, Concurrency, Memory Safety, Standard Library, Pi 5 Features, Appendix.

- **CONTRIBUTING.md** (`docs/CONTRIBUTING.md`, 840 lines) — Complete rewrite: build, test, add nodes/verifications/instructions, code review process, PR template.

- **CONVENTIONS.md** (`docs/CONVENTIONS.md`, 796 lines) — Complete rewrite: Rust style, error handling, testing, naming, documentation, git commit format.

- **GLOSSARY.md** (`docs/GLOSSARY.md`, 893 lines) — Complete rewrite: 40+ terms across core, verification, ARM64, Pi 5, and type theory domains.

- **ROADMAP.md** (`docs/ROADMAP.md`, 277 lines) — 5-phase roadmap with milestones, deliverables, success criteria, dependency graph, and risk mitigation.

### Added — Project Packaging

- **MANIFEST.md** — Complete file inventory: all 166 project files with purposes and line counts, summary statistics by category, language, and crate size.

- **README.md** — Project README: overview, architecture, quick start, Pi 5 build instructions, test instructions, project structure, key concepts, examples, documentation index, contributing link.

- **CHANGELOG.md** — This file: comprehensive changelog with entries for Waves 1–5.

### Project Statistics (Wave 5 Completion)

| Metric                 | Value     |
|------------------------|-----------|
| Total files            | ~166      |
| Total lines            | ~130,000  |
| Rust source lines      | ~100,000  |
| Documentation lines    | ~28,000   |
| VUMA example lines     | ~1,400    |
| Workspace crates       | 12        |
| Formal specifications  | 15        |
| Example programs       | 10        |
| Tests                  | 300+      |
| Benchmarks             | 40+       |

---

## Release Notes

### [0.1.0] — 2026-03-05

This is the initial public release of the VUMA framework. It contains the complete architectural foundation: all 12 workspace crates, 15 formal specifications, 10 example programs, a comprehensive benchmark suite, and full documentation. The system can construct SCGs programmatically and from parsed text, infer Behavioral Descriptors, construct Memory State Graphs, verify all five VUMA invariants, generate ARM64 machine code, and boot on Raspberry Pi 5 hardware with UART output.

**Known Limitations:**
- Concurrent verification is limited to single-threaded programs (Phase 3 target)
- ARM64 codegen does not yet support atomic instructions (Phase 3 target)
- The COR is not yet integrated end-to-end (Phase 3 target)
- The parser has known type mismatches in the AST→SCG lowering path
- Some Pi 5 bare-metal modules use inline assembly that requires nightly Rust

**Next Steps (Phase 2):**
- Complete BD inference subsumption of the Rust type system
- Verify doubly-linked list with no unsafe blocks
- Expand ARM64 codegen for complex programs
- Incremental verification targeting sub-1-second for single-function edits

---

## Worklog

- **2026-03-05 — Task 5-9:** Created comprehensive CHANGELOG.md with entries for Waves 1–5 covering all specifications, source crates, examples, build infrastructure, documentation, and packaging artifacts. Includes project statistics and release notes.
