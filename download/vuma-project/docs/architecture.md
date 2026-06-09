# VUMA Architecture Document

**Project:** VUMA — Verified-Unsafe Memory Access Framework  
**Version:** 0.1.0  
**Status:** Phase 2 — Core Implementation  
**Date:** March 5, 2026  
**Authors:** VUMA Project Team

---

## Table of Contents

1. [System Overview](#1-system-overview)  
2. [Data Flow Diagram](#2-data-flow-diagram)  
3. [Crate Dependency Graph](#3-crate-dependency-graph)  
4. [Key Data Structures and Their Relationships](#4-key-data-structures-and-their-relationships)  
5. [Verification Pipeline](#5-verification-pipeline)  
6. [Code Generation Pipeline for Pi 5](#6-code-generation-pipeline-for-pi-5)  
7. [Runtime Optimization Pipeline](#7-runtime-optimization-pipeline)  
8. [Security Model Overview](#8-security-model-overview)  

---

## 1. System Overview

The VUMA framework implements a six-layer architecture for an AI-native programming language that replaces the traditional text-first, type-constrained, restriction-based language paradigm with a semantics-first, behaviorally-described, verification-based model. The name VUMA — Verified-Unsafe Memory Access — captures the central thesis: unsafe memory operations should not be forbidden but instead made verifiable. Rather than relying on a borrow checker to reject programs that cannot be statically proven safe, VUMA constructs a formal model of every memory operation and verifies global invariants against that model. Programs that pass verification run without runtime overhead; programs that fail receive precise counterexamples showing the exact execution path to the violation.

The six layers form a stack of cooperating subsystems, each with a well-defined responsibility and clean interfaces to adjacent layers. Data flows downward from human intent through graph construction, inference, verification, code generation, and finally into execution on bare metal. Feedback flows upward: runtime profiling data informs optimization, verification results inform the projection system, and hardware constraints inform graph layout decisions. The SCG is the single source of truth — there is no "source code" that the compiler translates; the SCG *is* the program, and every other representation (textual, visual, conversational, or machine code) is a projection of the SCG.

### The Six Layers

**Layer 1 — SCG (Semantic Computation Graph).** The foundational representation. The SCG is a directed, acyclic, attributed multigraph where nodes represent computational operations (allocations, deallocations, memory accesses, computations, casts, effects, control flow), edges represent relationships (data flow, control flow, derivation, annotation), and regions delineate scopes, phases, security boundaries, and deployment targets. The SCG has zero internal workspace dependencies; every other crate depends on it directly or transitively.

**Layer 2 — IVE (Inference and Verification Engine).** The reasoning core. The IVE reads the SCG, infers Behavioral Descriptors for every node and edge, and verifies VUMA's five global invariants (liveness, exclusivity, interpretation, origin, cleanup) against the Memory State Graph derived from the annotated SCG. The IVE operates through iterative fixpoint computation, resolving interdependencies between RepD inference, CapD inference, and RelD inference. It produces annotated SCGs with verification results, confidence levels, and counterexamples for violations.

**Layer 3 — Projections.** The human interface. The Projection System renders the SCG into multiple views: textual (code-like syntax with role-specific formatting), visual (dataflow diagrams, call graphs, memory layout views as SVG/HTML), and conversational (natural-language descriptions for AI agents). Bidirectional editing allows modifications in any projection to write back through validation, producing SCG modifications that are verified before application.

**Layer 4 — COR (Continuous Optimization Runtime).** The always-on execution engine. COR maintains an always-compiled invariant: every reachable SCG region is kept in compiled ARM64 machine code at all times. It performs incremental compilation, profile-guided optimization, speculative optimization with transparent deoptimization, and adaptive deployment across heterogeneous targets (local, remote, Pi 5 cores). Runtime profile data feeds back to the IVE for continuous re-optimization.

**Layer 5 — BD (Behavioral Descriptors).** The type replacement. A BD is the triple (RepD, CapD, RelD) that replaces traditional nominal types. RepD describes memory layout (size, alignment, field offsets, multiple simultaneous interpretations). CapD describes permitted operations (read, write, execute, serialize, send, persist, derive-pointer) with context-dependent capability sets. RelD describes relationships (temporal co-occurrence, structural containment, dependency ordering, semantic equivalence, security-level flow). BDs are inferred, not declared; the IVE derives them from SCG structure.

**Layer 6 — VUMA (Verified-Unsafe Memory Access).** The memory safety guarantee. VUMA operates through the Memory State Graph (MSG), which captures every allocation point, every pointer derivation, every deallocation point, every concurrent access, and every reinterpretation. The VUMA Verification Engine checks five global invariants against the MSG, producing formal proofs for verified invariants and counterexamples for violations. Programs that pass all five invariants are guaranteed memory-safe without any runtime overhead.

### Layer Interaction Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                     Projection System (Layer 3)                         │
│   ┌──────────┐   ┌──────────┐   ┌────────────────┐   ┌─────────────┐ │
│   │ Textual  │   │ Visual   │   │ Conversational │   │ Diff        │ │
│   │ View     │   │ View     │   │ View           │   │ View        │ │
│   └────┬─────┘   └────┬─────┘   └───────┬────────┘   └──────┬──────┘ │
│        └───────────────┼─────────────────┼───────────────────┘        │
│                        │  bidirectional edits via validation          │
├────────────────────────┼───────────────────────────────────────────────┤
│              Parser / Frontend (auxiliary)                             │
│   ┌─────────┐   ┌──────────┐   ┌───────────────────────┐             │
│   │  Lexer  │──▶│  Parser  │──▶│  AST → SCG Lowering   │             │
│   └─────────┘   └──────────┘   └───────────────────────┘             │
├────────────────────────────────────────────────────────────────────────┤
│                   Reasoning Core (Layers 2, 5, 6)                      │
│                                                                        │
│   ┌───────────────┐  ┌───────────────┐  ┌───────────────────────────┐│
│   │ Inference     │  │ Verification  │  │ MSG Builder               ││
│   │ Engine (IVE)  │  │ Engine (IVE)  │  │ (Memory State Graph)      ││
│   └───────┬───────┘  └───────┬───────┘  └───────────┬───────────────┘│
│           │                  │                       │                 │
│   ┌───────┴───────┐  ┌──────┴────────┐  ┌──────────┴───────────────┐│
│   │ RepD Inference│  │ CapD Inference│  │ RelD Inference           ││
│   └───────────────┘  └───────────────┘  └──────────────────────────┘│
│                                                                        │
│   Invariants: Liveness · Exclusivity · Interpretation ·               │
│               Origin · Cleanup                                         │
├────────────────────────────────────────────────────────────────────────┤
│                  SCG (Layer 1) — Core Representation                    │
│   ┌───────────┐   ┌───────────┐   ┌────────────────────────────────┐ │
│   │   Nodes   │   │   Edges   │   │   Regions                     │ │
│   │ (ops,     │   │ (data     │   │ (scopes, phases, security,    │ │
│   │  allocs,  │   │  flow,    │   │  deployment targets)          │ │
│   │  effects) │   │  deps)    │   │                               │ │
│   └───────────┘   └───────────┘   └────────────────────────────────┘ │
├────────────────────────────────────────────────────────────────────────┤
│           Execution Layer (Layer 4 + Platform)                         │
│   ┌───────────────┐  ┌───────────────┐  ┌──────────────────────────┐│
│   │ COR Runtime   │  │ ARM64         │  │ Pi 5 Platform            ││
│   │ (always-      │  │ Codegen       │  │ (GPIO, UART, I2C, SPI,  ││
│   │  compiled,   │  │ (register     │  │  DMA, multicore          ││
│   │  PGO, JIT)   │  │  alloc,       │  │  Cortex-A76)             ││
│   │              │  │  insn sel)    │  │                          ││
│   └───────────────┘  └───────────────┘  └──────────────────────────┘│
└────────────────────────────────────────────────────────────────────────┘
```

### Architectural Principles

The architecture is governed by five principles. **SCG primacy**: the SCG is the single source of truth; all other representations are projections. **Verification over restriction**: instead of rejecting programs that cannot be statically proven safe, VUMA verifies them and provides precise diagnostics when violations are found. **Inference over annotation**: Behavioral Descriptors are derived from program structure, not manually declared; the programmer specifies intent, the IVE infers types. **Continuous optimization**: the COR treats execution as a continuous cycle of compile-profile-optimize-recompile, not a one-shot compilation. **Bare-metal first**: the Pi 5 is not an afterthought or a porting target; it is the primary platform, and every design decision accounts for its constraints (4× Cortex-A76, BCM2712 peripherals, 4–8 GB LPDDR4X, no MMU in bare-metal mode).

---

## 2. Data Flow Diagram

A VUMA program travels through a multi-stage pipeline from human intent to hardware execution. Unlike traditional compilers where data flows linearly (lex → parse → typecheck → optimize → emit), the VUMA pipeline contains feedback loops at every stage: verification results flow back to the projection system for display, runtime profiles flow back to the IVE for re-inference, and the MSG constrains code generation decisions. These feedback loops are not optional — they are integral to the system's ability to continuously improve code quality and catch violations that only emerge at runtime.

### Primary Data Flow: Source to Execution

```
 Source          Lexer         Parser          AST           AST-to-SCG         SCG
  Text     ──▶   Tokens  ──▶   AST     ──▶   (validated) ──▶  Lowering    ──▶  (raw)
                                                                            │
                                                                            ▼
                                                               ┌─────────────────────┐
                                                               │    Raw SCG           │
                                                               │  • No BDs attached   │
                                                               │  • No verification   │
                                                               │  • Names resolved    │
                                                               │  • Dependencies set  │
                                                               └─────────┬───────────┘
                                                                         │
                                                           ┌─────────────┼─────────────┐
                                                           ▼             ▼             ▼
                                                     ┌──────────┐ ┌──────────┐ ┌──────────┐
                                                     │ RepD     │ │ CapD     │ │ RelD     │
                                                     │ Inference│ │ Inference│ │ Inference│
                                                     └────┬─────┘ └────┬─────┘ └────┬─────┘
                                                          │            │            │
                                                          └────────────┼────────────┘
                                                                       ▼  fixpoint iteration
                                                          ┌──────────────────────────┐
                                                          │   Annotated SCG          │
                                                          │  • BDs on every node     │
                                                          │  • BDs on every edge     │
                                                          │  • Constraint set        │
                                                          └────────────┬─────────────┘
                                                                       │
                                                           ┌───────────┼───────────┐
                                                           ▼           ▼           ▼
                                                     ┌──────────┐ ┌────────┐ ┌────────────┐
                                                     │ MSG      │ │ VUMA   │ │ Proof      │
                                                     │ Builder  │ │ Verify │ │ Engine     │
                                                     └────┬─────┘ └───┬────┘ └─────┬──────┘
                                                          │           │             │
                                                          └───────────┼─────────────┘
                                                                      ▼
                                                          ┌──────────────────────────┐
                                                          │   Verified SCG           │
                                                          │  • All invariants proven │
                                                          │  • Counterexamples for   │
                                                          │    any violations        │
                                                          │  • Confidence levels     │
                                                          └────────────┬─────────────┘
                                                                       │
                                                           ┌───────────┼───────────┐
                                                           ▼           ▼           ▼
                                                     ┌──────────┐ ┌────────┐ ┌────────────┐
                                                     │ ARM64    │ │ COR    │ │ PGO        │
                                                     │ Codegen  │ │ Setup  │ │ Guided Opts│
                                                     └────┬─────┘ └───┬────┘ └─────┬──────┘
                                                          │           │             │
                                                          └───────────┼─────────────┘
                                                                      ▼
                                                          ┌──────────────────────────┐
                                                          │  ARM64 Machine Code      │
                                                          │  + Runtime Metadata      │
                                                          │  + Profile Instrument.   │
                                                          └────────────┬─────────────┘
                                                                       │
                                                                      ▼
                                                          ┌──────────────────────────┐
                                                          │  COR Runtime on Pi 5     │◀──┐
                                                          │  (execute + profile)     │   │
                                                          └──────────────────────────┘   │
                                                                                         │
                                                          ┌──────────────────────────┐   │
                                                          │  Profile Data Feedback   │───┘
                                                          │  (hot paths, allocs,     │
                                                          │   cache misses, PMU)     │
                                                          └──────────────────────────┘
```

### Feedback Loop Details

The data flow is not purely top-down. Three critical feedback loops ensure continuous improvement:

**Verification Feedback Loop.** When the VUMA verification engine detects a violation (e.g., a use-after-free or a data race), it produces a `Counterexample` containing the exact execution path to the violation. This counterexample flows back to the Projection System, which renders it as a human-readable diagnostic with source locations, affected nodes, and suggested fixes. The programmer interacts with the projection to modify the SCG, which re-enters the pipeline. This loop is synchronous — the programmer sees the violation immediately and can fix it before proceeding.

**Profile Feedback Loop.** During execution, the COR collects profile data (edge traversal frequencies, node execution times, Pi 5 PMU counters including cache misses and branch mispredictions) and feeds it back to the IVE. The IVE uses this data to refine BDs (e.g., discovering that a value is only read in practice, allowing CapD narrowing), re-prioritize verification (e.g., verifying hot paths more aggressively), and drive profile-guided optimization (e.g., inlining, code layout, branch prediction hints). This loop is asynchronous — it operates continuously in the background without interrupting execution.

**Deployment Feedback Loop.** The COR Deployment Manager monitors execution across heterogeneous targets (local CPU, Pi 5 cores, remote endpoints) and migrates SCG regions at runtime to rebalance load. When a region becomes hot on one target, the deployment manager may migrate it to a more suitable target (e.g., moving a DMA-heavy region to a Pi 5 core with dedicated DMA channels). This loop integrates with the profile feedback loop — migration decisions are informed by profile data.

### Stage-by-Stage Description

**Stage 1 — Lexing and Parsing.** Source text (which is itself a projection, but serves as the common input format) enters the lexer, which produces a token stream covering keywords, identifiers, operators, literals, and BD annotations. The parser consumes tokens and produces an Abstract Syntax Tree. The parser is deliberately simple — it performs no type checking, name resolution, or semantic analysis. Its sole job is to recognize syntactic structure and reject malformed input with helpful error messages and suggested corrections. The grammar is designed to be unambiguous; the "most vexing parse" problem does not exist because the syntax is derived from the SCG schema, not the other way around. Incremental parsing re-parses only changed portions of text, maintaining the SCG-to-text mapping for sub-100ms edit responsiveness.

**Stage 2 — AST-to-SCG Lowering.** The AST is lowered into a raw Semantic Computation Graph. This is not a simple syntactic transformation — the lowering resolves names, identifies data flow dependencies, constructs the initial region hierarchy, and creates the graph structure that the IVE will reason over. The resulting SCG has nodes and edges but no Behavioral Descriptors and no verification annotations; it is the "blank canvas" that the reasoning engine will fill in. The lowering produces `AllocationNode`, `AccessNode`, `DeallocationNode`, `CastNode`, `ComputationNode`, `ControlNode`, `EffectNode`, and `PhantomNode` types with their respective payloads.

**Stage 3 — BD Inference.** The IVE infers Behavioral Descriptors for every node in the SCG through iterative fixpoint computation. This proceeds in three interdependent tracks: RepD inference determines the memory layout of every value (size, alignment, field offsets, bit-level structure, multiple simultaneous interpretations); CapD inference determines what operations are valid on every value in every context (read, write, execute, serialize, send, persist, derive-pointer) with context-dependent capability sets; RelD inference determines the relationships between values (temporal co-occurrence, structural containment, dependency ordering, semantic equivalence, security-level flow). These three tracks are interdependent — CapD inference may refine RepD by discovering that a value is only read in a particular context, and RelD inference may constrain CapD by establishing security boundaries. The IVE resolves these interdependencies through iterative fixpoint computation until a stable state is reached.

**Stage 4 — VUMA Verification.** Once BDs are attached, the MSG Builder constructs a Memory State Graph from the annotated SCG via the `scg_to_msg` conversion pipeline. This conversion performs a topological walk of SCG nodes, mapping `AllocationNode → Region` with monotonic address allocation, `AccessNode → Derivation + Access` with proper kind and size, `DeallocationNode → Region status Freed`, and `CastNode → DerivationKind::Cast` derivation chains. Control flow edges between Access nodes produce `SyncEdge` with HappensBefore ordering. The VUMA Verification Engine then checks the five global invariants against the MSG: liveness (every access targets allocated memory), exclusivity (no conflicting concurrent accesses), interpretation (every access uses a valid RepD), origin (every address traces to a valid allocation), and cleanup (every region is eventually freed or explicitly leaked). The Proof Engine generates formal proofs for verified invariants and counterexamples for violations.

**Stage 5 — Code Generation.** The verified SCG is handed to the ARM64 code generator through a three-phase pipeline: SCG → IR lowering, register allocation, and machine code emission. Because VUMA has already proven memory safety, the codegen can emit raw pointer operations without any runtime bounds checks, borrow checks, or GC barriers. The COR sets up the runtime environment: allocating stack space, configuring the memory allocator, setting up the profile-guided optimization feedback loop with Pi 5 PMU instrumentation, and preparing the hardware (GPIO, UART, I2C, SPI, DMA, interrupt controllers) for execution.

**Stage 6 — Execution and Feedback.** The ARM64 machine code runs on the Pi 5 under the COR. The COR collects profile data through `ProfileCollector` — a thread-safe collector that records node execution times, edge traversal frequencies, allocation statistics, and Pi 5 PMU counter snapshots (cycle count, instruction count, cache misses, branch misses). The `collect_profile` analysis entry point computes hot spots, cold spots, hot paths, and PMU aggregates, producing recommendations for optimization. This feedback loop is continuous — the system is always learning from its own execution and improving accordingly through speculative optimization, profile-guided inlining, and adaptive code layout.

---

## 3. Crate Dependency Graph

The VUMA project is organized as a Cargo workspace with twelve crates. The dependency graph reflects the layered architecture: lower layers (SCG, BD) have no dependencies on higher layers, while higher layers (IVE, VUMA, COR) depend on the lower layers. Cross-layer dependencies are minimized to ensure that the core reasoning components remain independent of the presentation and execution components. This section describes each crate, its role, its dependencies, and the rules governing the dependency structure.

### Workspace Layout

```
vuma/
├── Cargo.toml                    (workspace root — shared dependencies)
├── Makefile                      (build targets: dev, release, pi5, pi5-image, pi5-flash, pi5-debug)
├── src/
│   ├── scg/                      (Layer 1 — Semantic Computation Graph)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            (crate root, re-exports)
│   │       ├── node.rs           (NodeId, NodeType, NodeData, NodePayload, per-variant payloads)
│   │       ├── edge.rs           (EdgeId, EdgeKind, EdgeData)
│   │       ├── graph.rs          (SCG struct, construction, validation, traversal)
│   │       ├── region.rs         (RegionId, SCGRegion, DeploymentTarget)
│   │       ├── query.rs          (SCGQuery, execute, find_derivation_chains, find_access_nodes_to_region)
│   │       ├── dominance.rs      (DominatorTree, compute_dominators, dominance frontier)
│   │       ├── liveness.rs       (LivenessAnalysis, find_use_after_free, find_dead_allocations)
│   │       ├── transform.rs      (PassManager, DCE, constant folding, inlining, CSE, verification pass)
│   │       ├── diff.rs           (SCGDiff, compute_edit_script, three_way_merge, apply_diff)
│   │       └── serialize.rs      (JSON serialization/deserialization via serde)
│   │
│   ├── bd/                       (Layer 5 — Behavioral Descriptors)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            (crate root, re-exports)
│   │       ├── repd.rs           (RepD: Byte, Struct, Enum, Array, Pointer, Union, Opaque)
│   │       ├── capd.rs           (CapD: capability set with BitSet, context-dependent)
│   │       ├── reld.rs           (RelD: Relation kinds — Containment, Aliasing, DataFlow, etc.)
│   │       ├── descriptor.rs     (BD triple: RepD × CapD × RelD, BDId, compatibility, refinement)
│   │       ├── inference.rs      (BD inference from SCG structure)
│   │       ├── context.rs        (Context: evaluation context for CapD transitions)
│   │       ├── context_solver.rs (Context-dependent capability resolution)
│   │       ├── capd_lattice.rs   (CapD lattice operations: meet, join, subcap)
│   │       ├── reld_refine.rs    (RelD refinement ordering and composition)
│   │       ├── repd_compat.rs    (RepD compatibility checking and subtyping)
│   │       └── unify.rs          (BD unification algorithm for inference)
│   │
│   ├── vuma/                     (Layer 6 — VUMA Memory Model)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            (crate root, re-exports)
│   │       ├── address.rs        (Address newtype with hex display and arithmetic)
│   │       ├── region.rs         (Region: contiguous memory span, RegionId, RegionStatus)
│   │       ├── derivation.rs     (Derivation: pointer provenance tracking, DerivationKind)
│   │       ├── access.rs         (Access: read/write at a program point, AccessKind)
│   │       ├── sync.rs           (SyncEdge: ordering between accesses, SyncOrdering)
│   │       ├── msg.rs            (MSG: the Memory State Graph tying regions, derivations, accesses)
│   │       ├── msg_builder.rs    (MSG construction from raw data)
│   │       ├── msg_incremental.rs (MSGDelta, compute_delta, apply_delta, SCGSnapshot)
│   │       ├── scg_to_msg.rs     (SCG → MSG conversion pipeline)
│   │       ├── invariant_liveness.rs      (Liveness invariant checker)
│   │       ├── invariant_exclusivity.rs   (Exclusivity invariant checker)
│   │       ├── invariant_interpretation.rs (Interpretation invariant checker)
│   │       ├── invariant_origin.rs        (Origin invariant checker)
│   │       ├── invariant_cleanup.rs       (Cleanup invariant checker)
│   │       ├── access_analysis.rs         (Access pattern analysis)
│   │       └── program_point.rs           (Source location tracking)
│   │
│   ├── ive/                      (Layer 2 — Inference & Verification Engine)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            (crate root, re-exports)
│   │       ├── inference.rs      (InferenceEngine: BD propagation, constraint derivation)
│   │       ├── bd_solver.rs      (BD constraint solver for inference)
│   │       ├── constraint.rs     (Constraint types: temporal, resource flow, security)
│   │       ├── verification.rs   (VerificationEngine: 5 invariant checks)
│   │       ├── liveness.rs       (Liveness verifier with proof obligations)
│   │       ├── exclusivity.rs    (Exclusivity verifier with interference graph)
│   │       ├── interpretation.rs (Interpretation verifier with WriteReadPair tracking)
│   │       ├── origin.rs         (Origin verifier)
│   │       ├── cleanup.rs        (Cleanup verifier with resource lifecycle graph)
│   │       ├── invariant_aggregator.rs (Runs all checks, produces unified VerificationSummary)
│   │       ├── result.rs         (VerificationResult, VerificationStatus, ConfidenceLevel, CounterExample)
│   │       └── debt.rs           (VerificationDebt: tracking unverified obligations by priority)
│   │
│   ├── cor/                      (Layer 4 — Continuous Optimization Runtime)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            (crate root, re-exports)
│   │       ├── runtime.rs        (CORuntime: central orchestrator)
│   │       ├── profile.rs        (ProfileData, ProfileCollector, Pi5PmuCounters, HotPath analysis)
│   │       ├── speculative.rs    (SpeculativeExecutor, BranchPredictionTable, SpeculativeInlining)
│   │       ├── optimization.rs   (OptimizationEngine, DCE, constant folding, inlining, loop unrolling)
│   │       ├── deployment.rs     (DeploymentManager, HotSwap, Delta deployment, version tracking)
│   │       ├── config.rs         (Config: optimization level, time budgets, target architecture)
│   │       └── types.rs          (COR-internal types: SCG wrapper, Delta, RegionId mapping)
│   │
│   ├── projection/               (Layer 3 — Projection System)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            (crate root)
│   │       ├── textual.rs        (Textual projection: SCG → human-readable code)
│   │       ├── visual.rs         (Visual projection: SCG → SVG/HTML dataflow diagrams)
│   │       ├── conversational.rs (Conversational projection: SCG → natural language)
│   │       ├── bidirectional.rs  (Bidirectional editing: projection edits → SCG modifications)
│   │       └── diff.rs           (Semantic diff: compute and render differences between SCG versions)
│   │
│   ├── parser/                   (Auxiliary — Frontend)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            (crate root)
│   │       ├── lexer.rs          (Tokenizer for VUMA textual syntax)
│   │       ├── parser.rs         (Parser: token stream → AST)
│   │       ├── ast.rs            (AST types)
│   │       ├── to_scg.rs         (AST → SCG lowering)
│   │       └── error.rs          (Parse errors with recovery and suggestions)
│   │
│   ├── codegen/                  (Auxiliary — ARM64 Code Generation)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            (crate root, CodegenError, pipeline entry)
│   │       ├── arm64.rs          (Arm64Instruction, register/condition enums, binary encoding)
│   │       ├── ir.rs             (IR types: functions, blocks, instructions, terminators, values)
│   │       ├── scg_to_ir.rs      (SCG → IR translation via ScgToIr converter)
│   │       ├── regalloc.rs       (Linear-scan register allocator for aarch64)
│   │       └── emit.rs           (ARM64 code emitter and ELF generation)
│   │
│   ├── pi5/                      (Platform — Raspberry Pi 5)
│   │   ├── Cargo.toml            (build = "build.rs", no_std compatible)
│   │   ├── build.rs              (Cargo build script for bare-metal aarch64-unknown-none)
│   │   ├── link.ld               (ARM64 linker script: entry, sections, per-core stacks)
│   │   └── src/
│   │       ├── lib.rs            (crate root)
│   │       ├── boot.rs           (exception vectors, _start, boot_main, FDT parsing)
│   │       ├── platform.rs       (BCM2712 memory map, board identification)
│   │       ├── uart.rs           (PL011 UART driver)
│   │       ├── gpio.rs           (Memory-mapped GPIO)
│   │       ├── timer.rs          (ARM generic timer, BCM2712 system timer)
│   │       ├── mmio.rs           (MMIO register access primitives)
│   │       └── smp.rs            (Multicore boot and inter-core communication)
│   │
│   ├── proof/                    (Formal Proofs)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            (crate root)
│   │       ├── proof.rs          (Proof, ProofStep, Goal, ProofStatus, Counterexample)
│   │       ├── checker.rs        (Proof checker: verify each step independently)
│   │       ├── rules.rs          (Inference rules for proof construction)
│   │       ├── tactics.rs        (Automated proof tactics)
│   │       ├── counterexample.rs (Counterexample generation and minimization)
│   │       ├── liveness_proofs.rs     (Liveness-specific proof rules)
│   │       ├── exclusivity_proofs.rs  (Exclusivity-specific proof rules)
│   │       ├── interpretation_proofs.rs (Interpretation-specific proof rules)
│   │       ├── origin_proofs.rs       (Origin-specific proof rules)
│   │       └── cleanup_proofs.rs      (Cleanup-specific proof rules)
│   │
│   ├── std/                      (Standard Library)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs            (crate root, re-exports: BD, RelD, Ptr, RegionPtr, Slice, etc.)
│   │       ├── primitives.rs     (Ptr, RegionPtr, Slice, VumaResult, VumaOption, Range, HasBD trait)
│   │       ├── alloc.rs          (Allocation, deallocation, copy, fill, zero — VUMA-VERIFIED)
│   │       ├── collections.rs    (Vec, LinkedList, HashMap, BTreeMap — VUMA-VERIFIED)
│   │       ├── sync.rs           (Mutex, RwLock, Channel, AtomicU32, AtomicU64 — VUMA-VERIFIED)
│   │       └── io.rs             (Read, Write, BufRead traits with UART and Pi 5 backends)
│   │
│   └── tests/                    (Integration Tests)
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs            (test framework)
│           ├── trivial.rs        (Trivial program proofs)
│           ├── dlist.rs          (Doubly-linked list verification)
│           ├── bd_inference.rs   (BD inference integration tests)
│           ├── concurrent.rs     (Concurrent verification tests)
│           ├── graph.rs          (SCG graph construction and query tests)
│           └── framework.rs      (Test infrastructure and helpers)
│
└── docs/
    ├── architecture.md           (this document)
    ├── ROADMAP.md                (project roadmap)
    ├── WORKLOG.md                (agent work log)
    ├── GLOSSARY.md               (project glossary)
    ├── CONVENTIONS.md            (coding conventions)
    ├── CONTRIBUTING.md           (contributor guidelines)
    └── specs/                    (formal specifications)
        ├── scg-formal-spec.md
        ├── repd-formal-spec.md
        ├── capd-formal-spec.md
        ├── reld-formal-spec.md
        ├── vuma-invariants-spec.md
        ├── msg-construction-spec.md
        ├── pi5-memory-model-spec.md
        ├── security-model-spec.md
        ├── bd-inference-algorithm.md
        ├── vuma-verification-algorithm.md
        ├── arm64-codegen-algorithm.md
        ├── benchmark-design.md
        ├── decidability-analysis.md
        ├── trivial-proofs.md
        └── dlist-proof.md
```

### Dependency Graph (ASCII)

```
                        ┌──────────────┐
                        │    tests      │
                        └──────┬───────┘
                               │ depends on everything
          ┌────────────────────┼────────────────────────────┐
          │                    │                            │
          ▼                    ▼                            ▼
   ┌────────────┐      ┌───────────┐               ┌───────────┐
   │ projection │      │   pi5     │               │  codegen   │
   └─────┬──────┘      └─────┬─────┘               └──────┬────┘
         │                   │                            │
         │                   │           ┌────────────────┤
         │                   ▼           │                │
         │            ┌───────────┐     ▼                │
         │            │   cor     │ ┌───────────┐       │
         │            └─────┬─────┘ │  vuma      │       │
         │                  │       └─────┬──────┘       │
         │                  │             │              │
         │         ┌────────┼─────────────┤              │
         │         │        │             │              │
         ▼         ▼        ▼             ▼              ▼
   ┌─────────────────────────────────────────────────────────────┐
   │                        ┌────────┐                           │
   │              ┌─────────┤  ive   ├──────────┐               │
   │              │         └───┬────┘           │               │
   │              │             │                │               │
   │              ▼             ▼                ▼               │
   │        ┌──────────┐ ┌──────────┐ ┌──────────────┐          │
   │        │   bd     │ │  proof   │ │    std       │          │
   │        └────┬─────┘ └────┬─────┘ └──────┬───────┘          │
   │             │            │               │                   │
   │             └────────────┼───────────────┘                   │
   │                          ▼                                   │
   │                   ┌───────────┐                               │
   │                   │    scg    │  ◄── foundation (zero deps)   │
   │                   └───────────┘                               │
   │                                                               │
   │             ┌───────────┐                                     │
   │             │  parser   │ ──▶ produces scg                    │
   │             └───────────┘                                     │
   └───────────────────────────────────────────────────────────────┘
```

### Key Dependency Rules

1. **`scg` is the foundation.** It has zero workspace-internal dependencies. It defines the core data structures (`Node`, `Edge`, `Region`, `Annotation`) and the graph operations (construction, composition, transformation, querying, dominance analysis, liveness analysis, diff/merge). Every other crate depends on `scg` either directly or transitively. This guarantees that the SCG remains the single source of truth and that no higher-level concern leaks into the core representation.

2. **`bd` and `proof` are orthogonal extensions of `scg`.** They depend on `scg` but not on each other. `bd` adds Behavioral Descriptor types (`RepD`, `CapD`, `RelD`, `BD`) and the inference algorithm with context solving, capability lattices, RelD refinement, RepD compatibility, and BD unification. `proof` adds the proof representation (`Proof`, `ProofStep`, `Goal`, `Counterexample`) and the proof-checking algorithm with per-invariant proof rules, automated tactics, and counterexample minimization. This separation ensures that BD inference can be developed and tested independently of the proof system.

3. **`ive` is the central orchestrator.** It depends on `scg`, `bd`, `proof`, and `std`. It ties together type inference (via `bd`), constraint inference, verification (via `proof`), and standard library knowledge (via `std`). The IVE is the "brain" of the system, and its dependency footprint reflects this — it is the only crate that touches all four foundational crates.

4. **`vuma` extends `ive` with memory verification.** It depends on `scg`, `ive`, and `bd`. It constructs the MSG (via `scg_to_msg`), verifies the five VUMA invariants, and supports incremental verification (via `msg_incremental`). It is not part of `ive` itself because the memory model is a separate concern from general inference and verification.

5. **`codegen` and `cor` are the execution layer.** They depend on `scg` and on the verification crates (`vuma`, `ive`) because they need the verified, annotated SCG to generate correct code. They do not depend on `projection` or `parser` — execution is independent of input format and presentation. `codegen` provides the three-phase pipeline (SCG → IR → register allocation → emission), while `cor` adds continuous optimization (profiling, speculative optimization, deployment management).

6. **`pi5` is the platform layer.** It depends on `cor` and `codegen` to integrate Pi 5–specific runtime services (GPIO, UART, multicore boot, interrupt handling, DMA) with the code generation pipeline. It is the only crate that contains target-specific code, bare-metal boot sequences, and the ARM64 linker script. The `build.rs` script activates only when targeting `aarch64-unknown-none`, and the crate uses `no_std`-compatible dependencies.

7. **`projection` and `parser` are the human interface.** They depend on `scg` and `ive` but not on `vuma`, `cor`, `codegen`, or `pi5`. This ensures that the projection system can render verification results without depending on the execution layer, and that the parser can produce SCGs without depending on verification.

---

## 4. Key Data Structures and Their Relationships

The VUMA framework is built around four core data structures that correspond to the four main layers of reasoning: the SCG (representation), the BD (data characterization), the MSG (memory verification), and the Proof (formal guarantee). These structures are deeply interconnected: the SCG is the substrate from which BDs are inferred, the MSG is derived from the annotated SCG, and Proofs are generated from the verification of invariants against the MSG. Understanding these structures and their relationships is essential for anyone contributing to the codebase.

### 4.1 Semantic Computation Graph (SCG)

The SCG is the primary representation of a program. It is a directed, acyclic, attributed multigraph. Every node represents a computational operation; every edge represents a data flow or dependency; every region delineates a scope, phase, or security boundary. The SCG uses `IndexMap` for nodes and edges (preserving insertion order and providing stable indices) and `HashMap` for annotations (fast lookup by key). The graph is stored as an adjacency list; each node maintains its incoming and outgoing edge lists for efficient traversal. Regions form a tree (each region has at most one parent), enabling efficient containment queries.

```
┌──────────────────────────────────────────────────────────────────────┐
│                         SCG Core Types                                │
│                                                                      │
│  struct SCG {                                                        │
│      nodes: IndexMap<NodeId, NodeData>,     // O(1) lookup           │
│      edges: IndexMap<EdgeId, EdgeData>,     // O(1) lookup           │
│      regions: IndexMap<RegionId, SCGRegion>,// O(1) lookup           │
│  }                                                                   │
│                                                                      │
│  struct NodeData {                                                   │
│      id: NodeId,                 // newtyped u32                     │
│      node_type: NodeType,        // Allocation, Access, Deallocation,│
│                                   // Cast, Computation, Control,     │
│                                   // Effect, Phantom                 │
│      payload: NodePayload,       // per-variant payload              │
│      program_point: ProgramPoint,// source location                  │
│  }                                                                   │
│                                                                      │
│  struct EdgeData {                                                   │
│      id: EdgeId,                 // newtyped u32                     │
│      src: NodeId,                // data producer                    │
│      dst: NodeId,                // data consumer                    │
│      kind: EdgeKind,             // DataFlow, ControlFlow,           │
│                                   // Derivation, Annotation          │
│  }                                                                   │
│                                                                      │
│  struct SCGRegion {                                                  │
│      id: RegionId,               // newtyped u32                     │
│      deployment: DeploymentTarget,// Heap, Stack, Gpu, Tls,         │
│                                   // Peripheral, Custom              │
│      nodes: Vec<NodeId>,         // nodes in this region            │
│  }                                                                   │
│                                                                      │
│  enum NodeType {                                                     │
│      Allocation,    // AllocationNode { size, align, region_id }     │
│      Access,        // AccessNode { mode, size, region_id }         │
│      Deallocation,  // DeallocationNode { allocation_node }         │
│      Cast,          // CastNode { source_type, target_type }        │
│      Computation,   // ComputationNode { operation, result_type }   │
│      Control,       // ControlNode { kind: Branch|Loop|Return }     │
│      Effect,        // EffectNode { effect_kind }                   │
│      Phantom,       // PhantomNode { purpose }                      │
│  }                                                                   │
└──────────────────────────────────────────────────────────────────────┘
```

**Node Payloads.** Each `NodeType` has a corresponding payload struct that carries type-specific data. `AllocationNode` carries `size`, `align`, `region_id`, and optional `type_name`. `AccessNode` carries `mode` (Read/Write), `size`, and `region_id`. `DeallocationNode` carries a reference to the `allocation_node` it frees. `CastNode` carries source and target type information. `ComputationNode` carries the operation name and result type. `ControlNode` carries the control kind (Branch, Loop, Return) with condition and targets. `EffectNode` carries side-effect metadata. `PhantomNode` carries a purpose string for nodes that exist only for verification scaffolding.

**Edge Kinds.** `DataFlow` edges connect data producers to consumers. `ControlFlow` edges represent control dependencies (branch targets, loop back-edges). `Derivation` edges trace pointer provenance from allocation through offset/cast operations to access. `Annotation` edges attach metadata without implying data or control dependence.

**SCG Operations.** The SCG supports construction (`add_node`, `add_edge`, `add_region`), validation (structural integrity checks), traversal (topological sort, BFS, DFS), querying (`SCGQuery` enum with `NodesByType`, `LeakedAllocations`, etc.), dominance analysis (dominator tree, dominance frontier, post-dominators), liveness analysis (use-after-free detection, dead allocation identification), transformation (DCE, constant folding, inlining, CSE via `PassManager`), and diff/merge (`compute_edit_script`, `three_way_merge`, `apply_diff`).

### 4.2 Behavioral Descriptor (BD)

A BD is the triple `(RepD, CapD, RelD)` that replaces traditional nominal types. Each component captures an orthogonal dimension of data behavior. BDs are inferred, not declared — the IVE derives them from SCG structure through iterative fixpoint computation.

```
┌──────────────────────────────────────────────────────────────────────┐
│                      Behavioral Descriptor                            │
│                                                                      │
│  struct BD {                                                         │
│      id: BDId,             // unique identifier                      │
│      repd: RepD,           // how data is laid out in memory         │
│      capd: CapD,           // what operations are allowed            │
│      reld: RelD,           // how data relates to other values       │
│  }                                                                   │
│                                                                      │
│  // RepD — Representation Descriptor                                 │
│  enum RepD {                                                         │
│      Byte(ByteRep),       // { size, align }                        │
│      Struct(StructRep),   // { fields: Vec<(name, RepD, offset)> }  │
│      Enum(EnumRep),       // { variants: Vec<(name, RepD)> }        │
│      Array(ArrayRep),     // { elem: Box<RepD>, count }             │
│      Pointer(PointerRep), // { pointee: Box<RepD> }                 │
│      Union(UnionRep),     // { variants: Vec<RepD> }                │
│      Opaque(OpaqueRep),   // { size, align, name }                  │
│  }                                                                   │
│  // POINTER_SIZE = 8 (64-bit target)                                │
│                                                                      │
│  // CapD — Capability Descriptor                                    │
│  struct CapD {                                                       │
│      capabilities: HashSet<Capability>,                              │
│      // Read, Write, Execute, Iterate, Send, Persist,               │
│      // Serialize, Hash, Compare, DerivePtr, Share,                 │
│      // Drop, Freeze, Thaw, ...                                     │
│  }                                                                   │
│  // Lattice: meet (intersection), join (union), subcap (subset)     │
│  // Context-dependent: same value can have different CapD in        │
│  // different SCG regions (via Context + ContextSolver)             │
│                                                                      │
│  // RelD — Relational Descriptor                                    │
│  struct RelD {                                                       │
│      relations: Vec<Relation>,                                       │
│  }                                                                   │
│  struct Relation {                                                   │
│      kind: RelKind,       // Containment, Liveness, Aliasing,       │
│                           // DataFlow, RegionBound, Ownership       │
│      target: Option<BDId>, // related value (if known)              │
│      properties: Vec<RelProperty>,                                   │
│  }                                                                   │
│  // Refinement ordering: reld1.refines(reld2) iff reld1 has        │
│  // a superset of reld2's relations                                 │
└──────────────────────────────────────────────────────────────────────┘
```

**RepD Design.** `RepD` supports multiple simultaneous interpretations of the same memory, which is essential for zero-copy interop and type punning. A `StructRep` can have overlapping fields at the same offset, representing the same bytes viewed in different ways. This is the VUMA equivalent of C's `union` or Rust's transmute, but with formal verification that each interpretation is valid in its usage context. The `PointerRep` carries the pointee's `RepD`, enabling the IVE to track pointer depth and derive correct `RepD`s for pointer arithmetic.

**CapD Design.** `CapD` uses a `HashSet<Capability>` for efficient set operations (union via `strengthen`, intersection via `weaken`, subset via `subcap`). The `capd_lattice` module provides formal lattice operations with `meet` (greatest lower bound, intersection) and `join` (least upper bound, union). The `Context` and `ContextSolver` modules enable context-dependent capabilities — the same value can have different capability sets in different SCG regions. For example, a buffer may have `Read | Write` before sealing and only `Read` after, with the transition verified by the IVE.

**RelD Design.** `RelD` stores relationships as a flat list, because relationship queries are typically "what are all relationships of this value?" rather than "what values are related in this specific way?" This makes the common case O(n) in the number of relationships per value (typically small) and avoids the overhead of maintaining relationship indexes. The `reld_refine` module provides the refinement ordering: `RelD1.refines(RelD2)` iff `RelD1` has a superset of `RelD2`'s relations, meaning more constrained values refine less constrained ones.

**BD Composition and Compatibility.** Two BDs are compatible if their RepDs are compatible (same size and alignment, compatible interpretations), their CapDs are compatible (no conflicting capabilities), and their RelDs are compatible (no contradictory relationships). The `descriptor` module provides `BD::compatible()` and `BD::refines()` methods. The `unify` module implements the BD unification algorithm used by the IVE for inference.

### 4.3 Memory State Graph (MSG)

The MSG is the IVE's formal model of the program's entire memory behavior. It is derived from the SCG via the `scg_to_msg` conversion pipeline and used exclusively by the VUMA verification layer. The MSG captures every allocation point, every pointer derivation, every deallocation point, every concurrent access, and every reinterpretation.

```
┌──────────────────────────────────────────────────────────────────────┐
│                     Memory State Graph (MSG)                          │
│                                                                      │
│  struct MSG {                                                        │
│      regions: Vec<Region>,          // memory regions                │
│      derivations: Vec<Derivation>,  // pointer provenance chains    │
│      accesses: Vec<Access>,         // read/write events            │
│      sync_edges: Vec<SyncEdge>,     // happens-before ordering      │
│  }                                                                   │
│                                                                      │
│  struct Region {                                                     │
│      id: RegionId,                                                   │
│      base: Address,              // start address (u64 newtype)      │
│      size: u64,                  // region size in bytes             │
│      status: RegionStatus,       // Allocated, Freed, Stack,        │
│                                   // Mapped, Device                  │
│      alloc_point: ProgramPoint,  // where allocation occurs         │
│      free_point: Option<ProgramPoint>,                               │
│      owner_context: Option<RegionId>,                                │
│  }                                                                   │
│                                                                      │
│  struct Derivation {                                                 │
│      id: DerivationId,                                               │
│      source: DerivationSource,  // Direct, Offset, Cast, Index      │
│      kind: DerivationKind,                                           │
│      parent: Option<DerivationId>,                                   │
│      offset: i64,                // byte offset from parent          │
│      provenance_range: Option<(Address, Address)>,                   │
│  }                                                                   │
│                                                                      │
│  struct Access {                                                     │
│      id: AccessId,                                                   │
│      target: Address,            // address being accessed           │
│      kind: AccessKind,           // Read, Write                      │
│      size: u64,                  // bytes accessed                   │
│      program_point: ProgramPoint,                                    │
│      derivation: Option<DerivationId>,                               │
│  }                                                                   │
│                                                                      │
│  struct SyncEdge {                                                   │
│      id: SyncEdgeId,                                                 │
│      access_a: AccessId,                                             │
│      access_b: AccessId,                                             │
│      ordering: Ordering,         // HappensBefore, Atomic,           │
│                                   // LockAcquire, LockRelease        │
│  }                                                                   │
└──────────────────────────────────────────────────────────────────────┘
```

**SCG-to-MSG Conversion.** The `scg_to_msg` module implements the conversion pipeline. It performs a topological walk of SCG nodes ensuring all predecessors are converted first. `AllocationNode → Region` with monotonic address allocation (base `0x1_0000`). `AccessNode → Derivation + Access` with proper kind (Read/Write) and size. `DeallocationNode → Region status Freed` with free_point set. `CastNode → DerivationKind::Cast` derivation from parent chain. `ComputationNode → passthrough Derivation` (Direct, forwarding provenance). Pointer operations produce `Derivation` with `DerivationKind::Offset` and provenance range. ControlFlow edges between Access nodes produce `SyncEdge` with `HappensBefore` ordering. Post-conversion verification checks all derivation chains are well-formed.

**Incremental MSG.** The `msg_incremental` module supports incremental verification via `MSGDelta`, `compute_delta`, `apply_delta`, and `SCGSnapshot`. When the SCG changes, only the affected subgraph is re-converted, and the delta is applied to the existing MSG. This enables sub-second verification for single-function edits, which is critical for interactive development.

### 4.4 Proof

The Proof structure represents a formal verification result — either a proof that an invariant holds or a counterexample demonstrating a violation. Proofs are generated per-invariant by specialized proof modules (`liveness_proofs`, `exclusivity_proofs`, `interpretation_proofs`, `origin_proofs`, `cleanup_proofs`) and checked by the independent `checker` module.

```
┌──────────────────────────────────────────────────────────────────────┐
│                       Proof Structure                                 │
│                                                                      │
│  struct Proof {                                                      │
│      goal: Goal,                  // what we're proving              │
│      status: ProofStatus,         // Proven, Refuted, Unknown       │
│      steps: Vec<ProofStep>,       // chain of reasoning             │
│  }                                                                   │
│                                                                      │
│  struct Goal {                                                       │
│      invariant: InvariantName,    // Liveness, Exclusivity, etc.    │
│      target: ProgramPoint,        // where the invariant applies    │
│      context: Vec<Assumption>,    // assumptions in scope           │
│  }                                                                   │
│                                                                      │
│  struct ProofStep {                                                  │
│      rule: InferenceRule,         // which rule was applied         │
│      premises: Vec<Goal>,         // sub-goals                      │
│      conclusion: Goal,            // derived fact                   │
│  }                                                                   │
│                                                                      │
│  enum ProofStatus {                                                  │
│      Proven(ConfidenceLevel),      // High, Medium, Low             │
│      Refuted(CounterExample),      // violation found               │
│      Unknown,                       // cannot prove or refute        │
│  }                                                                   │
│                                                                      │
│  struct CounterExample {                                             │
│      execution_path: Vec<ProgramPoint>,  // path to violation       │
│      violated_invariant: InvariantName,                            │
│      evidence: Vec<Evidence>,           // supporting observations  │
│      description: String,               // human-readable           │
│  }                                                                   │
└──────────────────────────────────────────────────────────────────────┘
```

**Design Decisions.** The Proof structure is a derivation tree — each step depends on premises that are themselves goals, forming a tree of reasoning. This enables independent verification: the `checker` module can verify each step independently and compose the results. The `ProofStatus` uses a tiered confidence model: `Proven` with a confidence level (High = full formal proof, Medium = proof with some assumptions, Low = empirical evidence only), `Refuted` with a concrete counterexample, or `Unknown` when the proof engine cannot reach a conclusion. This avoids the binary accept/reject decision that would recreate the restriction problem VUMA was designed to solve. The `tactics` module provides automated proof strategies for common patterns.

### 4.5 Data Structure Relationships

```
                           ┌─────────┐
                           │   SCG   │ ◄── single source of truth
                           └────┬────┘
                                │
                    ┌───────────┼───────────┐
                    │           │           │
                    ▼           ▼           ▼
              ┌──────────┐ ┌────────┐ ┌──────────┐
              │   BD     │ │ Parser │ │ Proof    │
              │(inferred)│ │(input) │ │(output)  │
              └────┬─────┘ └────────┘ └────┬─────┘
                   │                       │
                   ▼                       │
             ┌───────────┐                │
             │ Annotated │                │
             │    SCG    │                │
             └─────┬─────┘                │
                   │                       │
                   ▼                       │
             ┌───────────┐                │
             │    MSG    │────────────────┘
             │ (derived) │  verification produces
             └─────┬─────┘  Proof + CounterExample
                   │
                   ▼
             ┌───────────┐
             │ Verified  │
             │    SCG    │ ──▶ Codegen ──▶ ARM64 ──▶ COR ──▶ Pi 5
             └───────────┘
```

The key insight is that data flows through these structures in a strict order: SCG → BD (inference) → Annotated SCG → MSG (construction) → Verification → Proof → Verified SCG → Codegen. Each transformation is one-way and produces a richer artifact. Feedback loops operate at the SCG level — profile data and counterexamples modify the SCG, which re-enters the pipeline.

---

## 5. Verification Pipeline

The verification pipeline is the heart of the VUMA system. It transforms a raw SCG (produced by the parser) into a verified, annotated SCG (consumed by the code generator) through a sequence of inference, construction, and verification steps. Each step is designed to be composable, incremental, and auditable — you can re-run any step independently, and the system produces structured results that can be inspected, cached, and incrementally updated. The pipeline is orchestrated by the `InvariantAggregator`, which runs all five invariant checks and produces a unified `VerificationSummary`.

### Pipeline Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        Verification Pipeline                             │
│                                                                         │
│  ┌──────────┐    ┌──────────────┐    ┌───────────────┐                 │
│  │ Raw SCG  │───▶│ BD Inference │───▶│ Annotated SCG │                 │
│  └──────────┘    │ (IVE)        │    │ (BDs attached)│                 │
│                  └──────────────┘    └───────┬───────┘                 │
│                                              │                          │
│                                              ▼                          │
│                                      ┌───────────────┐                 │
│                                      │ MSG Builder   │                 │
│                                      │ (scg_to_msg)  │                 │
│                                      └───────┬───────┘                 │
│                                              │                          │
│                          ┌───────────────────┼───────────────────┐     │
│                          ▼                   ▼                   ▼     │
│                  ┌──────────────┐   ┌──────────────┐   ┌────────────┐ │
│                  │ Liveness     │   │ Exclusivity  │   │Interpret.  │ │
│                  │ Verifier     │   │ Verifier     │   │ Verifier   │ │
│                  └──────┬───────┘   └──────┬───────┘   └─────┬──────┘ │
│                         │                  │                  │        │
│                         └──────────────────┼──────────────────┘        │
│                                            │                           │
│                          ┌─────────────────┼──────────────────┐        │
│                          ▼                 ▼                            │
│                  ┌──────────────┐   ┌──────────────┐                  │
│                  │ Origin       │   │ Cleanup      │                  │
│                  │ Verifier     │   │ Verifier     │                  │
│                  └──────┬───────┘   └──────┬───────┘                  │
│                         │                  │                           │
│                         └──────────────────┘                           │
│                                    │                                    │
│                                    ▼                                    │
│                         ┌──────────────────────┐                       │
│                         │ Invariant Aggregator │                       │
│                         │ (VerificationSummary)│                       │
│                         └──────────┬───────────┘                       │
│                                    │                                    │
│                          ┌─────────┼─────────┐                        │
│                          ▼         ▼         ▼                        │
│                    ┌──────────┐ ┌────────┐ ┌──────────┐               │
│                    │ Proof    │ │Counter │ │ Debt     │               │
│                    │ Engine   │ │Example │ │ Tracker  │               │
│                    └──────────┘ └────────┘ └──────────┘               │
└─────────────────────────────────────────────────────────────────────────┘
```

### The Five VUMA Invariants

**1. Liveness** — Every access targets allocated memory. The liveness verifier builds a `LivenessInput` from the MSG, tracking resource events (allocate, free, access) across program points and threads. It generates `ProofObligation`s for each access and verifies that the accessed region is in `Allocated` status at the access point. Violations produce `LivenessViolation` records identifying use-after-free and use-before-allocate scenarios. The verifier handles concurrent programs by tracking per-thread resource states and synchronization edges.

**2. Exclusivity** — No overlapping simultaneous write access. The exclusivity verifier builds an `InterferenceGraph` from concurrent accesses to overlapping memory regions. Two accesses conflict if they target overlapping addresses, at least one is a write, and they are not ordered by a happens-before relationship (via `SyncEdge`). The verifier checks `CapDInfo` to determine whether concurrent reads are allowed (shared-read regions). Violations produce `Conflict` records with `ConflictKind` (WriteWrite, WriteRead, ReadWrite) and the specific access pairs involved.

**3. Interpretation** — Every access interprets bytes according to a valid RepD. The interpretation verifier tracks `WriteReadPair`s: every read must be preceded by a write that established the data with a compatible RepD. The `CapDTransitionResult` tracks capability transitions that may change how a value can be interpreted (e.g., sealing a buffer removes the write capability and fixes the interpretation). Violations produce `InterpretationViolation` records identifying type confusion, reading integer bytes as pointers, and reading uninitialized memory.

**4. Origin** — Every address traces to a valid allocation. The origin verifier traverses the derivation chain from every access back to its root allocation. Each `Derivation` must have a valid parent chain ending at a `DerivationSource::Direct` from a `Region`. The verifier rejects phantom pointers (hardcoded addresses, untrusted inputs) that have no valid derivation chain. Violations produce origin violation records with the broken chain and the address that cannot be traced to an allocation.

**5. Cleanup** — Every allocated region is eventually freed or intentionally leaked. The cleanup verifier builds a `CleanupGraph` from the MSG, tracking the lifecycle of each resource (allocation → use → deallocation). It verifies that every region reaches a terminal state (`Freed` or intentionally leaked via annotation). The verifier distinguishes between leaks (unreachable regions with no deallocation) and intentional leaks (regions marked with explicit leak annotations, such as arenas and global buffers). Violations produce `CleanupViolation` records with `ViolationKind` (Leak, DoubleFree, UseAfterFree).

### Verification Result Aggregation

The `InvariantAggregator` runs all five verifiers and produces a `VerificationSummary` containing:

- **`AggregatedResult`** per invariant: `OverallVerdict` (Pass, Fail, Inconclusive) with `VerificationLevel` (Full, Partial, BestEffort)
- **`DiagnosticsReport`**: human-readable diagnostics with per-invariant details
- **`InvariantDelta`**: when re-verifying, the delta between previous and current results (for incremental reporting)
- **`VerificationDebt`**: unverified obligations ordered by `Priority` (Critical, High, Medium, Low)

The tiered verification model (`VerificationLevel`) is essential: some invariants can be fully verified (e.g., liveness in single-threaded programs), some can be partially verified (e.g., exclusivity requires happens-before analysis that may be incomplete), and some can only be verified on a best-effort basis (e.g., cleanup in the presence of arbitrary interop). The system does not reject programs that cannot be fully verified; instead, it records the verification debt and makes it visible to the programmer.

### Incremental Verification

The `msg_incremental` module supports incremental verification via `MSGDelta`. When the SCG changes (e.g., a single function is edited), the system computes the delta between the old and new SCGs using `compute_scg_delta`, converts only the affected subgraph to MSG via `compute_delta`, and applies the delta to the existing MSG via `apply_delta`. The `VerificationStatus` tracks which invariants need re-verification based on the delta. This enables sub-second verification for typical edits, which is critical for interactive development and the projection system's bidirectional editing feature.

---

## 6. Code Generation Pipeline for Pi 5

The code generation pipeline translates a verified SCG into ARM64 machine code that runs on the Raspberry Pi 5. The pipeline consists of three phases: SCG-to-IR lowering, register allocation, and machine code emission. Each phase produces a well-defined intermediate artifact that can be inspected, cached, and incrementally updated. The pipeline is designed to produce zero-overhead code — because VUMA has already proven memory safety, the codegen can emit raw pointer operations without any runtime bounds checks, borrow checks, or GC barriers.

### Three-Phase Pipeline

```
┌──────────────────────────────────────────────────────────────────────────┐
│                     Code Generation Pipeline                              │
│                                                                          │
│  ┌───────────┐     ┌──────────────────┐     ┌──────────────────────┐    │
│  │ Verified  │     │   IR (Intermedi- │     │   ARM64 Machine Code │    │
│  │ SCG       │────▶│   ate Represen-  │────▶│   + ELF Binary       │    │
│  │           │     │   tation)        │     │   + Debug Info        │    │
│  └───────────┘     └──────────────────┘     └──────────────────────┘    │
│       Phase 1           Phase 2                  Phase 3                │
│    (scg_to_ir)        (regalloc)                (emit)                  │
└──────────────────────────────────────────────────────────────────────────┘
```

### Phase 1: SCG → IR Lowering

The `ScgToIr` converter translates SCG nodes into an intermediate representation that is closer to the target architecture while remaining target-independent enough to support future backends. The IR consists of functions, basic blocks, instructions, terminators, and values.

```
┌──────────────────────────────────────────────────────────────────────┐
│                         IR Types                                     │
│                                                                      │
│  struct IrFunction {                                                 │
│      name: String,                                                   │
│      params: Vec<IrValue>,     // function parameters               │
│      blocks: Vec<IrBasicBlock>,// basic blocks                      │
│      return_type: Option<IrType>,                                    │
│  }                                                                   │
│                                                                      │
│  struct IrBasicBlock {                                               │
│      label: String,                                                  │
│      instructions: Vec<IrInstruction>,                               │
│      terminator: IrTerminator,                                       │
│  }                                                                   │
│                                                                      │
│  enum IrInstruction {                                                │
│      BinOp { op, dst, lhs, rhs },      // Add, Sub, Mul, Div, ...  │
│      UnaryOp { op, dst, src },         // Neg, Not, ...             │
│      Load { dst, addr, size },         // memory read               │
│      Store { addr, value, size },      // memory write              │
│      Call { dst, func, args },         // function call             │
│      Alloca { dst, size, align },      // stack allocation          │
│      Cast { dst, src, from_type, to_type },                         │
│      Gep { dst, base, offset },        // pointer arithmetic        │
│      Cmp { op, dst, lhs, rhs },        // comparison               │
│      Phi { dst, inputs },              // SSA phi node             │
│  }                                                                   │
│                                                                      │
│  enum IrTerminator {                                                 │
│      Return(Option<IrValue>),                                        │
│      Branch { condition, true_block, false_block },                  │
│      Jump { target },                                                │
│      Unreachable,                                                    │
│  }                                                                   │
│                                                                      │
│  enum IrValue {                                                      │
│      Register(IrReg),         // virtual register                   │
│      Constant(IrConst),       // immediate value                    │
│      FunctionRef(String),     // function reference                 │
│  }                                                                   │
└──────────────────────────────────────────────────────────────────────┘
```

**Node-to-IR Mapping.** `AllocationNode` → `Alloca` instruction. `AccessNode` with Read mode → `Load` instruction. `AccessNode` with Write mode → `Store` instruction. `ComputationNode` → `BinOp` or `UnaryOp` depending on the operation. `CastNode` → `Cast` instruction. `ControlNode` with Branch → `Branch` terminator. `ControlNode` with Return → `Return` terminator. `PhiNode` → `Phi` instruction. `EffectNode` → target-specific intrinsic call.

### Phase 2: Register Allocation

The register allocator assigns physical ARM64 registers to virtual registers in the IR. The current implementation uses a linear-scan algorithm, which provides good allocation quality in O(n) time where n is the number of virtual registers. The allocator targets the full AArch64 register set: `x0–x30` (general-purpose), `sp` (stack pointer), and `xzr` (zero register). It respects calling conventions (AAPCS64): `x0–x7` are argument/result registers, `x19–x28` are callee-saved, and `x9–x15` are caller-saved temporaries.

The allocator handles spilling when the number of simultaneously live virtual registers exceeds the number of available physical registers. Spill code generates `Load`/`Store` pairs that save and restore values to/from the stack. Because the VUMA verification has already proven memory safety, spilled values can be stored directly to the stack without any additional bounds checking.

### Phase 3: Machine Code Emission

The `arm64` module defines `Arm64Instruction` with all core integer instructions (MOV, ADD, SUB, MUL, DIV, AND, ORR, EOR, LSL, LSR, ASR), memory instructions (LDR, STR, LDP, STP with all addressing modes), and branch instructions (B, BL, BR, BLR, B.cond, CBZ, CBNZ, TBZ, TBNZ). Each instruction has a binary encoding verified against the ARM Architecture Reference Manual.

The `emit` module generates ARM64 machine code from the register-allocated IR and produces an ELF binary with proper section layout (`.text.boot`, `.text`, `.rodata`, `.data`, `.bss`) and debug information mapping SCG nodes to code offsets. The emitted binary includes `DebugInfo` with source-to-offset mapping, symbol table, and compiler notes for COR integration.

### Pi 5 Bare-Metal Boot Sequence

The Pi 5 platform module provides a complete bare-metal boot sequence. The `_start` entry point (in `.text.boot`) saves the DTB pointer, reads the core ID from `MPIDR_EL1.Aff0`, parks secondary cores in a `WFE` loop, sets up the stack for core 0 above `__bss_end`, zeros the BSS section, installs the exception vector table via `VBAR_EL1`, and jumps to `boot_main`. The `boot_main` function initializes UART at 115200 baud, parses the FDT header, constructs `BootInfo`, and calls the user's `main()` function. The linker script (`link.ld`) defines the memory layout: kernel loaded at `0x80000`, per-core 64 KiB stacks, MMIO window at `0x100000`, and exports `__bss_start`, `__bss_end`, `__stack_core0..3` symbols for boot code consumption.

---

## 7. Runtime Optimization Pipeline

The Continuous Optimization Runtime (COR) is the always-on execution engine that treats compilation not as a one-shot process but as a continuous cycle of compile-profile-optimize-recompile. The COR maintains an always-compiled invariant: every reachable SCG region is kept in compiled ARM64 machine code at all times. This enables the system to respond to runtime observations by re-optimizing hot paths, deoptimizing when speculation fails, and migrating regions across heterogeneous targets.

### COR Architecture

```
┌──────────────────────────────────────────────────────────────────────────┐
│                    Continuous Optimization Runtime                        │
│                                                                          │
│  ┌──────────────────────────────────────────────────────────────────┐    │
│  │                       CORuntime (orchestrator)                    │    │
│  │  ┌─────────────┐ ┌──────────────┐ ┌─────────────────────────┐   │    │
│  │  │ Incremental │ │ Profile      │ │ Deployment              │   │    │
│  │  │ Compiler    │ │ Collector    │ │ Manager                 │   │    │
│  │  │             │ │ (thread-safe)│ │ (hot-swap, delta,       │   │    │
│  │  │             │ │              │ │  version tracking)      │   │    │
│  │  └──────┬──────┘ └──────┬───────┘ └───────────┬─────────────┘   │    │
│  │         │               │                      │                  │    │
│  │         └───────────────┼──────────────────────┘                  │    │
│  │                         │                                         │    │
│  │                         ▼                                         │    │
│  │  ┌──────────────────────────────────────────────────────────┐    │    │
│  │  │              Optimization Engine                          │    │    │
│  │  │  ┌─────────────┐ ┌──────────────┐ ┌──────────────────┐  │    │    │
│  │  │  │ PGO         │ │ Speculative  │ │ SCG Transform    │  │    │    │
│  │  │  │ (hot paths, │ │ Executor     │ │ Passes           │  │    │    │
│  │  │  │  inlining,  │ │ (branch pred,│ │ (DCE, folding,   │  │    │    │
│  │  │  │  code       │ │  inlining,   │ │  inlining, CSE,  │  │    │    │
│  │  │  │  layout)    │ │  code motion)│ │  loop unroll)    │  │    │    │
│  │  │  └─────────────┘ └──────────────┘ └──────────────────┘  │    │    │
│  │  └──────────────────────────────────────────────────────────┘    │    │
│  └──────────────────────────────────────────────────────────────────┘    │
└──────────────────────────────────────────────────────────────────────────┘
```

### Profile-Guided Optimization

The `ProfileCollector` is a thread-safe runtime collector (backed by `Mutex<ProfileData>` + `AtomicU64` sample counter) that records node execution times, edge traversal frequencies, allocation statistics, and Pi 5 PMU counter snapshots. The `Pi5PmuCounters` struct captures cycle count, instruction count, cache misses, and branch misses, with computed metrics: `ipc()` (instructions per cycle), `cache_miss_rate()`, and `branch_miss_rate()`. The `collect_profile` analysis entry point ingests `ProfileSample` records, computes hot spots (`NodeHotSpot` with per-node call count, total time, time fraction), cold spots, hot paths (`HotPath` with cumulative time fraction and dominance threshold), and PMU aggregates. It produces a `ProfileReport` with recommendations including `CacheOptimize` and `BranchLayout` suggestions derived from PMU data.

Profile-guided optimization improves benchmark performance by at least 15% over unoptimized codegen through: (1) aggressive inlining of hot call sites, (2) code layout optimization for hot paths (sequential instruction cache utilization), (3) branch prediction hints for frequently taken branches, and (4) cold path outlining for rarely executed code.

### Speculative Optimization

The `SpeculativeExecutor` manages the full speculative-optimization lifecycle through three phases. **Phase 1 — Identify:** `identify_candidates()` scans profile data for `SpeculationCandidate`s with `CandidateKind` (LikelyBranch, HotPath, MonomorphicCall, UncontendedRegion) and confidence scores. `identify_inline_candidates()` analyzes call frequency data for speculative inlining. `identify_code_motion_candidates()` identifies hot/cold code for hoisting/sinking. **Phase 2 — Apply:** `apply_speculation()`, `apply_inline()`, and `apply_code_motion()` each save a `Snapshot` (compiled regions + SCG dimensions) before transforming the code. The `BranchPredictionTable` stores per-edge predictions derived from `ProfileData` with confidence thresholds. **Phase 3 — Validate and Rollback:** `validate_and_rollback()` checks all active assumptions against runtime observations. When an assumption is invalidated, the executor rolls back to the pre-speculation `Snapshot`, records a `SpeculationResult::Failure`, and removes the speculation from the active set. This transparent deoptimization ensures correctness while enabling aggressive optimization.

### SCG Transformation Passes

The `PassManager` in the SCG module orchestrates verification-aware optimization passes that operate directly on the SCG before code generation. Available passes include: `DeadCodeElimination` (removes unreachable nodes), `ConstantFolding` (evaluates compile-time-known expressions), `InliningPass` (inline calls below a size threshold), `CommonSubexpressionElimination` (deduplicates redundant computations), and `VerificationPass` (re-runs the verification pipeline after transformation to ensure semantics are preserved). Each pass implements the `SCGPass` trait and returns a `PassResult` indicating whether the pass made changes. The `PassManager` runs passes in dependency order and stops if any pass introduces a verification violation.

### Deployment and Hot-Swap

The `DeploymentManager` handles adaptive deployment across heterogeneous targets. `DeploymentTarget` variants include `Local`, `Pi5Bare { board_id, core_id }`, `Pi5Linux { host, core_affinity }`, and `Remote { endpoint }`. The manager supports: **Hot-swap deployment** via a 6-phase state machine (Idle → PreparingShadow → AwaitingSafePoint → Swapping → Completed → Failed) that replaces running code without stopping execution; **Delta deployment** via `DeploymentDelta` with block-level binary diffing (`compute()`, `apply()`, `estimated_size()`); **Version tracking** via `VersionLog` with per-region version history and rollback support; and **Package management** via `DeploymentPackage` with CRC32 checksums, debug info, and monotonic version numbers. Deployment results include timing, bytes transferred, and whether the deployment was a hot-swap or delta.

---

## 8. Security Model Overview

VUMA's security model is built on the principle that security properties should be derived from verified invariants, not from access control restrictions. Traditional security models restrict what programs can do; VUMA verifies what programs actually do and raises an alarm when behavior deviates from verified expectations. This inversion eliminates the fundamental tension between security and expressiveness: programs are not prevented from performing "dangerous" operations; instead, every dangerous operation is verified against the security model, and violations are caught before deployment.

### Security Layers

**Layer 1 — Memory Safety (VUMA Invariants).** The five VUMA invariants form the foundation of the security model. Liveness prevents use-after-free attacks. Exclusivity prevents data races that could lead to information disclosure or corruption. Interpretation prevents type confusion attacks (e.g., interpreting integer bytes as pointers). Origin prevents phantom pointer injection (e.g., using hardcoded addresses or untrusted input as pointers). Cleanup prevents resource exhaustion attacks via memory leaks. These invariants are verified at compile time and do not impose any runtime overhead.

**Layer 2 — Capability Security (CapD).** The `CapD` component of Behavioral Descriptors provides fine-grained capability-based security. Every value has a set of capabilities that determine what operations are permitted. Capabilities are not just read/write — they include `Send` (can the value be sent across a security boundary?), `Persist` (can the value be written to persistent storage?), `Serialize` (can the value be converted to a byte stream?), `DerivePtr` (can a pointer be derived from this value?), and `Execute` (can these bytes be executed as code?). The `ContextSolver` ensures that capabilities are context-dependent — a value may have `Send` in one security region but not in another. Capability transitions are verified by the IVE: a capability can only be removed (never added) as a value crosses a security boundary, ensuring that security levels only increase.

**Layer 3 — Information Flow (RelD).** The `RelD` component of Behavioral Descriptors tracks information flow between values. The `SecurityLevel` relationship kind assigns security classifications to values (e.g., Public, Confidential, Secret). The IVE verifies that information does not flow from higher-security values to lower-security channels: a `Secret` value cannot be written to a `Public` output, a `Confidential` value cannot be sent across an unencrypted channel. This is verified at the SCG level — the IVE traces data flow edges and checks that every edge from a higher-security node to a lower-security node is either explicitly authorized or blocked. The RelD also tracks `MustNotOutlive` relationships that prevent dangling references to freed resources, and `DerivedFrom` relationships that maintain taint tracking through pointer arithmetic and casts.

**Layer 4 — Region Security (SCG Regions).** SCG regions provide coarse-grained security boundaries. A `Security` region encloses a set of nodes that operate at a particular security level. Crossing a region boundary requires capability downgrade: values entering a lower-security region have their CapD intersected with the region's allowed capabilities. This is enforced by the IVE during verification — no code generation proceeds if a region boundary violation is detected. Regions can be nested (a Secret region inside a Confidential region inside a Public region), and the containment hierarchy is verified to be a proper lattice (no cycles, consistent ordering).

**Layer 5 — Platform Security (Pi 5).** The Pi 5 platform module provides hardware-level security through the BCM2712's memory protection features. The `DeploymentTarget::Pi5Bare` variant includes `board_id` and `core_id` fields that bind compiled code to specific hardware. The bare-metal boot sequence installs exception vectors that trap unauthorized access attempts. The MMIO module provides memory-mapped register access with capability descriptors that prevent unauthorized peripheral access. The DMA controller driver includes cache coherency management that prevents DMA-based attacks on main memory.

### Threat Model

VUMA's security model addresses the following threat categories:

1. **Memory corruption attacks** (buffer overflows, use-after-free, double-free, type confusion) — addressed by VUMA invariants verified at compile time with zero runtime overhead.

2. **Information disclosure** (reading sensitive data through pointer arithmetic, type punning, or side channels) — addressed by RelD information flow tracking and CapD capability restrictions.

3. **Privilege escalation** (gaining unauthorized capabilities by crossing security boundaries) — addressed by CapD context transitions verified by the IVE and region boundary enforcement.

4. **Resource exhaustion** (memory leaks, file descriptor exhaustion) — addressed by the cleanup invariant that verifies every resource is eventually freed or explicitly leaked.

5. **Concurrent access violations** (data races, deadlock, livelock) — addressed by the exclusivity invariant with happens-before analysis and deadlock detection.

6. **Supply chain attacks** (malicious code injected through dependencies) — partially addressed by origin verification (every pointer traces to a valid allocation), but full supply chain security requires additional mechanisms beyond VUMA's current scope.

### Verification Confidence and Debt

Not all security properties can be verified with full confidence. The `VerificationLevel` tier (Full, Partial, BestEffort) reflects the verification engine's ability to prove each invariant. `Full` verification produces a formal proof. `Partial` verification covers most cases but may have unresolved assumptions. `BestEffort` verification provides empirical evidence but no formal guarantee. Unverified properties are tracked as `VerificationDebt` items with priorities: `Critical` debt (safety violations) must be resolved before deployment, `High` debt (security concerns) should be resolved, and `Medium`/`Low` debt (quality issues) can be deferred. The `VerificationDebt` tracker maintains an ordered list of unresolved obligations and supports incremental resolution as the IVE gains more information (through additional annotations, profile data, or manual proof assistance).

---

*This document is maintained by the VUMA project team and updated as the architecture evolves. For implementation details, refer to the source crates in `src/`. For formal specifications, refer to `docs/specs/`. For the project roadmap, refer to `docs/ROADMAP.md`.*
