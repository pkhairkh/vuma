# VUMA Architecture Overview

**Project:** VUMA — Verified-Unsafe Memory Access Framework
**Version:** 0.1.0 (Phase 1 — Foundation)
**Date:** March 4, 2026
**Author:** Agent W1-21

---

## 1. System Overview

The VUMA framework is a six-layer architecture for AI-native programming language design. It replaces the traditional text-first, type-constrained, restriction-based language paradigm with a semantics-first, behaviorally-described, verification-based model. Each layer addresses a fundamental aspect of computation: representation (SCG), reasoning (IVE + BD), human interaction (Projection), execution (COR + Codegen), memory safety (VUMA), and hardware targeting (Pi 5 platform).

The architecture is organized as a stack of cooperating subsystems. Data flows downward from source text through parsing and graph construction, through inference and verification, through code generation, and finally into execution on bare metal. Feedback flows upward: runtime profiling data informs optimization, verification results inform the projection system, and hardware constraints inform graph layout decisions.

```
┌─────────────────────────────────────────────────────────────────┐
│                  Projection System (Layer 3)                     │
│  ┌──────────┐  ┌──────────┐  ┌────────────────┐                │
│  │ Textual  │  │ Visual   │  │ Conversational │                │
│  │ Projection│  │ Projection│ │ Projection     │                │
│  └────┬─────┘  └────┬─────┘  └───────┬────────┘                │
│       │              │                │                          │
│       └──────────────┼────────────────┘                         │
│                      │ bidirectional edits                      │
├──────────────────────┼──────────────────────────────────────────┤
│            Parser / Frontend (Layer 9 — auxiliary)              │
│  ┌────────┐  ┌─────────┐  ┌──────────────────┐                 │
│  │ Lexer  │  │ Parser  │  │ AST → SCG        │                 │
│  │        │→│         │→│ Lowering         │                 │
│  └────────┘  └─────────┘  └──────────────────┘                 │
├─────────────────────────────────────────────────────────────────┤
│         IVE (L2) + BD (L5) + VUMA (L6) — Reasoning Core        │
│                                                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
│  │ Inference    │  │ Verification │  │ MSG Builder          │  │
│  │ Engine       │  │ Engine       │  │ (Memory State Graph) │  │
│  └──────┬───────┘  └──────┬───────┘  └──────────┬───────────┘  │
│         │                 │                      │              │
│  ┌──────┴───────┐  ┌──────┴───────┐  ┌──────────┴───────────┐  │
│  │ RepD         │  │ CapD         │  │ RelD                 │  │
│  │ Inference    │  │ Inference    │  │ Inference            │  │
│  └──────────────┘  └──────────────┘  └──────────────────────┘  │
│                                                                 │
│  Invariants: Liveness · Exclusivity · Interpretation ·          │
│              Origin · Cleanup                                    │
├─────────────────────────────────────────────────────────────────┤
│            SCG (Layer 1) — Core Representation                   │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────────────────┐  │
│  │ Nodes    │  │ Edges    │  │ Regions                      │  │
│  │ (ops,    │  │ (data    │  │ (scopes, phases, security,   │  │
│  │  allocs, │  │  flow,   │  │  deployment targets)         │  │
│  │  effects)│  │  deps)   │  │                              │  │
│  └──────────┘  └──────────┘  └──────────────────────────────┘  │
├─────────────────────────────────────────────────────────────────┤
│         COR (L4) + Codegen + Pi5 (Layer 12 — Platform)          │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────┐  │
│  │ COR Runtime  │  │ ARM64        │  │ Pi 5 Platform        │  │
│  │ (always-     │  │ Codegen      │  │ (GPIO, UART, I2C,   │  │
│  │  compiled,   │  │ (register    │  │  SPI, DMA, multicore│  │
│  │  PGO, JIT)   │  │  alloc,      │  │  Cortex-A76)         │  │
│  │              │  │  insn sel)   │  │                      │  │
│  └──────────────┘  └──────────────┘  └──────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

### Layer Interaction Model

The six layers interact through well-defined interfaces. The SCG (Layer 1) is the central data structure; all other layers either consume it, produce it, or transform it. The IVE (Layer 2) reads the SCG and produces annotated SCG with inferred types, constraints, and verification results. Behavioral Descriptors (Layer 5) are computed by the IVE and attached to SCG nodes as annotations. VUMA (Layer 6) operates through the MSG, which is itself derived from the SCG. The Projection System (Layer 3) reads the SCG and renders views; bidirectional edits write back through validation. COR (Layer 4) consumes the annotated SCG and produces executable code, then feeds runtime profile data back into the IVE.

```
               ┌──────────┐
               │  Human   │
               └────┬─────┘
                    │ reads / edits
                    ▼
            ┌──────────────┐
            │  Projection  │◄──────────────────────────────┐
            │  System (L3) │                               │
            └──────┬───────┘                               │
                   │ bidirectional                         │
                   ▼                                       │
  ┌────────────────────────────────────────┐               │
  │              SCG (L1)                  │               │
  │  ┌─────────┐  ┌─────────┐  ┌───────┐  │               │
  │  │ Nodes   │  │ Edges   │  │Regions│  │               │
  │  └─────────┘  └─────────┘  └───────┘  │               │
  └───────┬────────────┬───────────────────┘               │
          │            │                                    │
          ▼            ▼                                    │
  ┌────────────┐ ┌───────────┐                              │
  │  IVE (L2)  │ │  BD (L5)  │  ──► annotated SCG          │
  └─────┬──────┘ └─────┬─────┘                              │
        │              │                                     │
        ▼              ▼                                     │
  ┌──────────────────────────┐                              │
  │  VUMA (L6) / MSG        │                              │
  │  (memory verification)   │                              │
  └───────────┬──────────────┘                              │
              │                                              │
              ▼                                              │
  ┌──────────────────────────┐    profile data              │
  │  COR (L4) + Codegen     │──────────────────────────────┘
  │  (runtime + ARM64 emit) │     (feedback loop)
  └───────────┬──────────────┘
              │
              ▼
  ┌──────────────────────────┐
  │  Pi 5 Hardware (L12)    │
  │  (Cortex-A76 × 4,      │
  │   GPIO, UART, I2C, ...) │
  └──────────────────────────┘
```

The key architectural insight is that the SCG is the single source of truth. There is no "source code" that the compiler translates; the SCG *is* the program. Every other representation — textual, visual, conversational, or machine code — is a projection of the SCG. This inversion of the traditional hierarchy (where text is primary and the AST is derived) is the foundation of the entire framework and enables all subsequent innovations: bidirectional editing, formal refactoring, BD-based type inference, and VUMA's global verification.

---

## 2. Data Flow

A VUMA program travels through a multi-stage pipeline from human intent to hardware execution. Unlike traditional compilers where data flows linearly (lex → parse → typecheck → optimize → emit), the VUMA pipeline contains feedback loops: verification results flow back to the projection system, runtime profiles flow back to the IVE, and the MSG constrains code generation decisions. Understanding this flow is essential for anyone working on the system.

### Primary Pipeline: Source to Execution

```
 Source        Lexer       Parser        AST         AST-to-SCG       SCG
  Text    ──►  Tokens ──►  AST    ──►  (validated) ──►  Lowering  ──►  (raw)
                                                                    │
                                                                    ▼
                                                         ┌──────────────────┐
                                                         │   SCG (raw)      │
                                                         │   no BDs, no     │
                                                         │   verification   │
                                                         └────────┬─────────┘
                                                                  │
                                                    ┌─────────────┼─────────────┐
                                                    ▼             ▼             ▼
                                              ┌──────────┐ ┌──────────┐ ┌──────────┐
                                              │ RepD     │ │ CapD     │ │ RelD     │
                                              │ Inference│ │ Inference│ │ Inference│
                                              └────┬─────┘ └────┬─────┘ └────┬─────┘
                                                   │            │            │
                                                   └────────────┼────────────┘
                                                                ▼
                                                    ┌──────────────────────┐
                                                    │  Annotated SCG       │
                                                    │  (BDs attached to    │
                                                    │   every node/edge)   │
                                                    └──────────┬───────────┘
                                                               │
                                                    ┌──────────┼──────────┐
                                                    ▼          ▼          ▼
                                              ┌──────────┐ ┌────────┐ ┌────────┐
                                              │ MSG      │ │ VUMA   │ │ Proof  │
                                              │ Builder  │ │ Verify │ │ Engine │
                                              └────┬─────┘ └───┬────┘ └───┬────┘
                                                   │           │          │
                                                   └───────────┼──────────┘
                                                               ▼
                                                    ┌──────────────────────┐
                                                    │  Verified SCG        │
                                                    │  (all invariants     │
                                                    │   proven or flagged) │
                                                    └──────────┬───────────┘
                                                               │
                                                    ┌──────────┼──────────┐
                                                    ▼          ▼          ▼
                                              ┌──────────┐ ┌────────┐ ┌────────┐
                                              │ ARM64    │ │ COR    │ │Profile │
                                              │ Codegen  │ │ Setup  │ │Guided  │
                                              │          │ │        │ │Opts    │
                                              └────┬─────┘ └───┬────┘ └───┬────┘
                                                   │           │          │
                                                   └───────────┼──────────┘
                                                               ▼
                                                    ┌──────────────────────┐
                                                    │  ARM64 Machine Code  │
                                                    │  + Runtime Metadata  │
                                                    └──────────┬───────────┘
                                                               │
                                                               ▼
                                                    ┌──────────────────────┐
                                                    │  COR Runtime on Pi 5│
                                                    │  (execute + profile)│
                                                    └──────────────────────┘
```

### Stage-by-Stage Description

**Stage 1: Lexing and Parsing.** Source text (which is itself a projection, but serves as the common input format) enters the lexer, which produces a token stream. The parser consumes tokens and produces an Abstract Syntax Tree. The parser is deliberately simple — it does not perform type checking, name resolution, or semantic analysis. Its sole job is to recognize syntactic structure and reject malformed input. The grammar is designed to be unambiguous; the "most vexing parse" problem does not exist because the syntax is derived from the SCG schema, not the other way around.

**Stage 2: AST-to-SCG Lowering.** The AST is lowered into a raw Semantic Computation Graph. This is not a simple syntactic transformation — the lowering resolves names, identifies data flow dependencies, constructs the initial region hierarchy, and creates the graph structure that the IVE will reason over. The resulting SCG has nodes and edges but no Behavioral Descriptors and no verification annotations; it is the "blank canvas" that the reasoning engine will fill in.

**Stage 3: BD Inference.** The IVE infers Behavioral Descriptors for every node in the SCG. This proceeds in three parallel tracks: RepD inference determines the memory layout of every value (size, alignment, field offsets, bit-level structure); CapD inference determines what operations are valid on every value in every context (read, write, execute, serialize, send, etc.); RelD inference determines the relationships between values (temporal co-occurrence, structural containment, security-level flow). These three tracks are interdependent — CapD inference may refine RepD by discovering that a value is only read in a particular context, and RelD inference may constrain CapD by establishing security boundaries. The IVE resolves these interdependencies through iterative fixpoint computation.

**Stage 4: VUMA Verification.** Once BDs are attached, the MSG Builder constructs a Memory State Graph from the annotated SCG. The MSG captures every allocation point, every pointer derivation, every deallocation point, every concurrent access, and every reinterpretation. The VUMA Verification Engine then checks the five global invariants against the MSG: liveness (every access targets allocated memory), exclusivity (no conflicting concurrent accesses), interpretation (every access uses a valid RepD), origin (every address traces to a valid allocation), and cleanup (every region is eventually freed or explicitly leaked). The Proof Engine generates formal proofs for verified invariants and counterexamples for violations.

**Stage 5: Code Generation.** The verified SCG is handed to the ARM64 code generator, which performs register allocation, instruction selection, and machine code emission. Because VUMA has already proven memory safety, the codegen can emit raw pointer operations without any runtime bounds checks, borrow checks, or GC barriers. The COR sets up the runtime environment: allocating stack space, configuring the memory allocator, setting up the profile-guided optimization feedback loop, and preparing the Pi 5 hardware (GPIO, UART, etc.) for execution.

**Stage 6: Execution and Feedback.** The ARM64 machine code runs on the Pi 5 under the COR. The COR collects profile data (hot paths, allocation patterns, cache miss rates) and feeds it back to the IVE, which uses it to drive re-optimization. This feedback loop is continuous — the system is always learning from its own execution and improving accordingly.

---

## 3. Module Dependencies

The VUMA project is organized as a Cargo workspace with twelve crates. The dependency graph reflects the layered architecture: lower layers (SCG, BD) have no dependencies on higher layers, while higher layers (IVE, VUMA, COR) depend on the lower layers. Cross-layer dependencies are minimized to ensure that the core reasoning components remain independent of the presentation and execution components.

### Workspace Layout

```
vuma/
├── Cargo.toml              (workspace root)
├── src/
│   ├── scg/                (Layer 1 — Core Representation)
│   ├── ive/                (Layer 2 — Inference & Verification)
│   ├── projection/         (Layer 3 — Projection System)
│   ├── cor/                (Layer 4 — Continuous Optimization Runtime)
│   ├── bd/                 (Layer 5 — Behavioral Descriptors)
│   ├── vuma/               (Layer 6 — VUMA Memory Model)
│   ├── parser/             (Auxiliary — Lexer + Parser + AST)
│   ├── codegen/            (Auxiliary — ARM64 Code Generation)
│   ├── pi5/                (Platform — Raspberry Pi 5 Support)
│   ├── std/                (Standard Library — Core primitives)
│   ├── proof/              (Formal Proofs — Proof generation & checking)
│   └── tests/              (Integration Tests)
└── docs/
    └── architecture.md     (this document)
```

### Dependency Graph (ASCII)

```
                    ┌─────────┐
                    │  tests   │
                    └────┬─────┘
                         │ depends on everything
          ┌──────────────┼──────────────────────────────┐
          │              │                              │
          ▼              ▼                              ▼
   ┌────────────┐ ┌───────────┐                 ┌───────────┐
   │ projection │ │  pi5      │                 │  codegen   │
   └─────┬──────┘ └─────┬─────┘                 └──────┬────┘
         │              │                              │
         │              │              ┌───────────────┤
         │              ▼              │               │
         │       ┌───────────┐        ▼               │
         │       │   cor     │  ┌───────────┐         │
         │       └─────┬─────┘  │  vuma     │         │
         │             │        └─────┬─────┘         │
         │             │              │               │
         │      ┌──────┼──────────────┤               │
         │      │      │              │               │
         ▼      ▼      ▼              ▼               ▼
   ┌──────────────────────────────────────────────────────┐
   │                    ┌────────┐                         │
   │              ┌─────┤  ive   ├─────┐                   │
   │              │     └───┬────┘     │                   │
   │              │         │          │                   │
   │              ▼         ▼          ▼                   │
   │        ┌─────────┐ ┌──────┐ ┌─────────┐              │
   │        │   bd    │ │proof │ │  std    │              │
   │        └────┬────┘ └──┬───┘ └────┬────┘              │
   │             │         │          │                    │
   │             └─────────┼──────────┘                    │
   │                       ▼                               │
   │                ┌────────────┐                          │
   │                │    scg     │  ◄── foundation          │
   │                └────────────┘                          │
   │                                                        │
   │              ┌────────────┐                            │
   │              │  parser    │ ──► produces scg           │
   │              └────────────┘                            │
   └──────────────────────────────────────────────────────────┘

   Key:
     scg     = Semantic Computation Graph (no internal deps)
     bd      = Behavioral Descriptors (depends on: scg)
     proof   = Proof Engine (depends on: scg)
     std     = Standard Library (depends on: scg)
     ive     = Inference & Verification Engine (depends on: scg, bd, proof, std)
     vuma    = VUMA Memory Model (depends on: scg, ive, bd)
     cor     = Continuous Optimization Runtime (depends on: scg, ive)
     codegen = ARM64 Code Generation (depends on: scg, vuma, cor)
     projection = Projection System (depends on: scg, ive)
     pi5     = Pi 5 Platform (depends on: cor, codegen)
     parser  = Lexer + Parser (depends on: scg)
     tests   = Integration Tests (depends on: everything)
```

### Key Dependency Rules

1. **`scg` is the foundation.** It has zero workspace-internal dependencies. It defines the core data structures (`Node`, `Edge`, `Region`, `Annotation`) and the graph operations (construction, composition, transformation, querying). Every other crate depends on `scg` either directly or transitively.

2. **`bd` and `proof` are orthogonal extensions of `scg`.** They depend on `scg` but not on each other. `bd` adds Behavioral Descriptor types (`RepD`, `CapD`, `RelD`, `BD`) and the inference algorithm. `proof` adds the proof representation (`Proof`, `ProofStep`, `Goal`, `Counterexample`) and the proof-checking algorithm. This separation ensures that BD inference can be developed and tested independently of the proof system.

3. **`ive` is the central orchestrator.** It depends on `scg`, `bd`, `proof`, and `std`. It ties together type inference (via `bd`), constraint inference, verification (via `proof`), and standard library knowledge (via `std`). The IVE is the "brain" of the system, and its dependency footprint reflects this.

4. **`vuma` extends `ive` with memory verification.** It depends on `scg`, `ive`, and `bd`. It constructs the MSG and verifies the five VUMA invariants. It is not part of `ive` itself because the memory model is a separate concern from general inference and verification.

5. **`codegen` and `cor` are the execution layer.** They depend on `scg` and on the verification crates (`vuma`, `ive`) because they need the verified, annotated SCG to generate correct code. They do not depend on `projection` or `parser` — execution is independent of input format and presentation.

6. **`pi5` is the platform layer.** It depends on `cor` and `codegen` to integrate Pi 5–specific runtime services (GPIO, UART, multicore boot) with the code generation pipeline. It is the only crate that contains target-specific code.

---

## 4. Key Data Structures

The VUMA framework is built around four core data structures that correspond to the four main layers of reasoning: the SCG (representation), the BD (data characterization), the MSG (memory verification), and the Proof (formal guarantee). Understanding these structures and their relationships is essential for anyone contributing to the codebase.

### 4.1 Semantic Computation Graph (SCG)

The SCG is the primary representation of a program. It is a directed, acyclic, attributed multigraph. Every node represents a computational operation; every edge represents a data flow or dependency; every region delineates a scope, phase, or security boundary.

```
┌──────────────────────────────────────────────────────────────┐
│                    SCG Core Types                             │
│                                                              │
│  struct SCG {                                                │
│      nodes: IndexMap<NodeId, Node>,       // O(1) lookup     │
│      edges: IndexMap<EdgeId, Edge>,       // O(1) lookup     │
│      regions: IndexMap<RegionId, Region>, // O(1) lookup     │
│      annotations: HashMap<AnnotationKey, Annotation>,        │
│  }                                                           │
│                                                              │
│  struct Node {                                               │
│      id: NodeId,                                             │
│      kind: NodeKind,        // Op, Alloc, Dealloc, Effect,  │
│      region: RegionId,      // enclosing region              │
│      bd: Option<BD>,        // inferred behavioral desc.     │
│      metadata: NodeMeta,    // source location, etc.         │
│  }                                                           │
│                                                              │
│  struct Edge {                                               │
│      id: EdgeId,                                             │
│      src: NodeId,           // data producer                 │
│      dst: NodeId,           // data consumer                 │
│      kind: EdgeKind,        // DataFlow, Control, Sync       │
│      bd: Option<BD>,        // edge-level behavioral desc.   │
│  }                                                           │
│                                                              │
│  struct Region {                                             │
│      id: RegionId,                                           │
│      kind: RegionKind,      // Scope, Phase, Security,       │
│                              // Deployment                   │
│      parent: Option<RegionId>,                               │
│      children: Vec<RegionId>,                                │
│      constraints: Vec<Constraint>,  // region-level rules    │
│  }                                                           │
│                                                              │
│  enum NodeKind {                                             │
│      Op(Opcode),            // arithmetic, logic, etc.       │
│      Alloc(AllocInfo),      // memory allocation             │
│      Dealloc(DeallocInfo),  // memory deallocation           │
│      Effect(EffectInfo),    // I/O, network, etc.            │
│      Call(CallInfo),        // function application          │
│      Construct(BD),        // value construction             │
│      Phi,                   // SSA phi node                  │
│      Branch(BranchInfo),    // conditional control flow      │
│  }                                                           │
└──────────────────────────────────────────────────────────────┘
```

**Design decisions:** The SCG uses `IndexMap` for nodes and edges (preserving insertion order and providing stable indices) and `HashMap` for annotations (fast lookup by key). The graph is stored as an adjacency list; each node maintains its incoming and outgoing edge lists for efficient traversal. Regions form a tree (each region has at most one parent), enabling efficient containment queries. The `NodeId` and `EdgeId` are newtyped `u32` values, keeping the graph compact and cache-friendly.

### 4.2 Behavioral Descriptor (BD)

A BD is the triple `(RepD, CapD, RelD)` that replaces traditional nominal types. Each component captures an orthogonal dimension of data behavior.

```
┌──────────────────────────────────────────────────────────────┐
│                    Behavioral Descriptor                      │
│                                                              │
│  struct BD {                                                 │
│      repd: RepD,            // how data is laid out          │
│      capd: CapD,            // what operations are allowed   │
│      reld: RelD,            // how data relates to others    │
│  }                                                           │
│                                                              │
│  struct RepD {                                               │
│      size: u64,              // total size in bytes          │
│      align: u64,             // alignment requirement        │
│      interpretations: Vec<Interpretation>,                   │
│      // Multiple valid views of the same bytes:              │
│      //   e.g., bytes[128], float32[32], struct {...}        │
│  }                                                           │
│                                                              │
│  struct Interpretation {                                     │
│      name: InternedStr,                                      │
│      offset: u64,            // byte offset within RepD      │
│      inner: RepD,            // nested representation        │
│  }                                                           │
│                                                              │
│  struct CapD {                                               │
│      capabilities: BitSet<Capability>,                       │
│      // Read, Write, Execute, Iterate, Send, Persist,        │
│      // Serialize, Hash, Compare, DerivePtr, ...             │
│      context_constraints: Vec<ContextConstraint>,            │
│      // e.g., "Send only if encrypted"                       │
│  }                                                           │
│                                                              │
│  struct RelD {                                               │
│      relationships: Vec<Relationship>,                       │
│  }                                                           │
│                                                              │
│  struct Relationship {                                       │
│      kind: RelKind,         // DerivedFrom, ContainedIn,     │
│                              // MustNotOutlive, EqTo,         │
│                              // SecurityLevel, ...            │
│      target: NodeId,        // the other value               │
│      properties: Vec<RelProperty>,                           │
│  }                                                           │
└──────────────────────────────────────────────────────────────┘
```

**Design decisions:** `RepD` supports multiple simultaneous interpretations of the same memory, which is essential for zero-copy interop and type punning. `CapD` uses a `BitSet` for efficient set operations (union, intersection, subset checks). `RelD` stores relationships as a flat list rather than a graph, because relationship queries are typically "what are all relationships of this value?" rather than "what values are related in this specific way?" This makes the common case O(1) and the rare case O(n).

### 4.3 Memory State Graph (MSG)

The MSG is the IVE's formal model of the program's entire memory behavior. It is derived from the SCG and used exclusively by the VUMA verification layer.

```
┌──────────────────────────────────────────────────────────────┐
│                    Memory State Graph (MSG)                   │
│                                                              │
│  struct MSG {                                                │
│      regions: Vec<MemoryRegion>,                             │
│      derivations: Vec<Derivation>,                           │
│      accesses: Vec<Access>,                                  │
│      sync_edges: Vec<SyncEdge>,                              │
│  }                                                           │
│                                                              │
│  struct MemoryRegion {                                       │
│      id: RegionId,                                           │
│      alloc_node: NodeId,      // SCG node that allocates     │
│      dealloc_node: Option<NodeId>,                           │
│      base: Address,           // start address               │
│      size: u64,               // region size                 │
│      status: RegionStatus,    // Allocated, Freed, Stack,    │
│                                 // Mapped, Device             │
│      ownership_context: RegionId,  // SCG region that owns   │
│  }                                                           │
│                                                              │
│  struct Derivation {                                         │
│      source: Address,         // parent pointer              │
│      derived: Address,        // child pointer               │
│      kind: DerivationKind,    // Offset, Cast, Index, ...    │
│      offset: i64,             // byte offset from source     │
│  }                                                           │
│                                                              │
│  struct Access {                                             │
│      node: NodeId,            // SCG node performing access  │
│      target: Address,         // address being accessed      │
│      kind: AccessKind,        // Read, Write, Execute        │
│      size: u64,               // bytes accessed              │
│      repd: RepD,              // interpretation used         │
│  }                                                           │
│                                                              │
│  struct SyncEdge {                                           │
│      access_a: AccessId,                                     │
│      access_b: AccessId,                                     │
│      ordering: SyncOrdering,  // HappensBefore, Atomic, ...  │
│  }                                                           │
└──────────────────────────────────────────────────────────────┘
```

**Design decisions:** The MSG is a flat, indexed structure rather than a graph, because the primary access patterns are (1) iterating over all accesses to a region, (2) tracing the derivation chain of an address, and (3) checking synchronization between concurrent accesses. Each of these is efficiently supported by auxiliary indexes built on top of the flat arrays. The `SyncEdge` type explicitly models the happens-before relationship between concurrent accesses, enabling the IVE to reason about thread safety without requiring a lock-based model.

### 4.4 Proof

The Proof structure represents a formal verification result — either a proof that an invariant holds or a counterexample demonstrating a violation.

```
┌──────────────────────────────────────────────────────────────┐
│                    Proof Structure                            │
│                                                              │
│  struct Proof {                                              │
│      goal: Goal,              // what we're proving          │
│      status: ProofStatus,     // Proven, Refuted, Unknown   │
│      steps: Vec<ProofStep>,   // chain of reasoning         │
│      conclusion: Conclusion,                                 │
│  }                                                           │
│                                                              │
│  struct Goal {                                               │
│      invariant: Invariant,    // liveness, exclusivity, etc. │
│      target: Target,          // node, edge, or region       │
│      context: ProofContext,   // assumptions in scope        │
│  }                                                           │
│                                                              │
│  struct ProofStep {                                          │
│      rule: InferenceRule,     // which rule was applied      │
│      premises: Vec<Goal>,     // sub-goals                   │
│      conclusion: Goal,        // derived fact                │
│  }                                                           │
│                                                              │
│  enum ProofStatus {                                          │
│      Proven(Confidence),      // confidence level (1.0=max)  │
│      Refuted(Counterexample), // violation found             │
│      Unknown(UnresolvedGoal), // cannot prove or refute      │
│  }                                                           │
│                                                              │
│  struct Counterexample {                                     │
│      execution_path: Vec<NodeId>,  // path to violation      │
│      violated_invariant: Invariant,                          │
│      description: String,     // human-readable explanation  │
│  }                                                           │
└──────────────────────────────────────────────────────────────┘
```

**Design decisions:** The Proof structure is a derivation tree — each step depends on premises that are themselves goals, forming a tree of reasoning. This enables independent verification: a proof checker can verify each step independently and compose the results. The `ProofStatus` uses a tiered confidence model (as discussed in the VUMA Verification Gap open question): `Proven` with a confidence level, `Refuted` with a concrete counterexample, or `Unknown` with the unresolved goal. This avoids the binary accept/reject decision that would recreate the restriction problem VUMA was designed to solve.

---

## 5. Verification Pipeline

The verification pipeline is the heart of the VUMA system. It transforms a raw SCG (produced by the parser) into a verified, annotated SCG (consumed by the code generator) through a sequence of inference, construction, and verification steps. Each step is designed to be composable, incremental, and auditable — you can re-run any step independently, and the system can verify its own verification results.

### Step-by-Step Pipeline

```
┌─────────────────────────────────────────────────────────────────┐
│                    VERIFICATION PIPELINE                         │
│                                                                 │
│  1. PARSE                                                       │
│     source text ──► tokens ──► AST ──► raw SCG                  │
│                                                                 │
│  2. INFER BDs                                                   │
│     raw SCG ──► RepD inference ──┐                               │
│                              CapD inference ──┤                 │
│                              RelD inference ──┘                 │
│                          ──► fixpoint iteration ──►             │
│                          ──► annotated SCG (BDs attached)       │
│                                                                 │
│  3. BUILD MSG                                                   │
│     annotated SCG ──► extract allocations ──┐                   │
│                    extract derivations ────┤                     │
│                    extract accesses ───────┤                     │
│                    extract sync edges ─────┘                     │
│                ──► MSG (Memory State Graph)                      │
│                                                                 │
│  4. VERIFY VUMA INVARIANTS                                      │
│     MSG ──► liveness check ──────────────┐                      │
│          ──► exclusivity check ──────────┤                      │
│          ──► interpretation check ───────┤                      │
│          ──► origin check ───────────────┤                      │
│          ──► cleanup check ──────────────┘                      │
│          ──► invariant results (per-invariant)                   │
│                                                                 │
│  5. GENERATE PROOFS                                              │
│     invariant results ──► proof construction ──┐                │
│     (for verified invariants)                   │                │
│                        ──► proof tree ─────────┘                │
│                                                                 │
│  6. GENERATE COUNTEREXAMPLES                                     │
│     invariant results ──► path exploration ──┐                  │
│     (for violated invariants)                 │                  │
│                        ──► counterexample ───┘                  │
│                                                                 │
│  7. REPORT VIA PROJECTION                                        │
│     proofs + counterexamples ──► projection system ──►          │
│         textual report / visual diagram / conversational         │
│         explanation                                              │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Detailed Step Descriptions

**Step 1: Parse Source to SCG.** The parser frontend converts source text into a raw SCG. This step is deliberately kept simple — it performs no verification, no inference, and no optimization. The output is a valid graph structure (well-formed nodes and edges with correct connectivity) but contains no behavioral descriptors, no verification annotations, and no proof information. The parser's only responsibility is syntactic correctness; everything else is handled downstream.

**Step 2: Infer BDs for All Nodes.** The IVE performs Behavioral Descriptor inference in three parallel tracks. RepD inference walks the SCG bottom-up, determining the memory layout of each value from its construction. CapD inference walks the SCG top-down, determining what operations are valid on each value from its usage context. RelD inference walks the SCG across (laterally), determining relationships between values from their data flow connections. These three tracks run iteratively until a fixpoint is reached — that is, until no further BD annotations can be added or refined. The fixpoint is guaranteed to converge because each track is monotonic (it only adds information, never removes it) and bounded (the BD space is finite for any given program).

**Step 3: Build MSG from SCG.** The MSG Builder extracts memory-relevant information from the annotated SCG. It identifies every allocation node and creates a `MemoryRegion`. It traces every pointer derivation (offset arithmetic, casts, index operations) and creates a `Derivation`. It identifies every read/write/execute operation and creates an `Access`. It identifies synchronization primitives (mutexes, atomics, barriers) and creates `SyncEdges`. The resulting MSG is a complete formal model of the program's memory behavior, suitable for verification.

**Step 4: Verify Each VUMA Invariant.** The VUMA Verification Engine checks the five global invariants against the MSG:

| Invariant       | Check                                                                 |
|----------------|-----------------------------------------------------------------------|
| Liveness       | For each Access, verify the target Address falls within a MemoryRegion with status Allocated at the program point of the access. |
| Exclusivity    | For each pair of concurrent Accesses targeting overlapping addresses, verify at least one is Read, or they are ordered by a SyncEdge. |
| Interpretation | For each Access, verify the RepD used to interpret the target bytes is a valid interpretation of the MemoryRegion's RepD. |
| Origin         | For each Address used in an Access, verify there exists a Derivation chain from an allocation point to that Address. |
| Cleanup        | For each MemoryRegion with status Allocated, verify there exists an SCG path from the allocation node to a deallocation node. |

Each invariant check produces a per-target result: either `Proven`, `Refuted` (with evidence), or `Unknown` (with unresolved sub-goals).

**Step 5: Generate Proofs for Verified Invariants.** For each invariant that was proven, the Proof Engine constructs a formal proof tree. The proof tree is a derivation tree whose leaves are axioms (e.g., "this allocation creates a live region") and whose internal nodes are inference rules (e.g., "if region R is live before node N, and node N does not free R, then R is live after node N"). The proof tree can be independently verified by the proof checker, providing a second line of defense against IVE bugs.

**Step 6: Generate Counterexamples for Violations.** For each invariant that was refuted, the Verification Engine constructs a concrete counterexample — an execution path through the SCG that leads to the violation. The counterexample includes the exact sequence of nodes traversed, the memory state at each point, and the specific access that violates the invariant. This is the information that the projection system presents to the human (or AI agent) to guide fix construction.

**Step 7: Report Results via Projection System.** The verification results — proofs, counterexamples, and unresolved goals — are handed to the Projection System, which renders them in the appropriate modality. A textual projection might show a traditional error report with source locations and explanations. A visual projection might highlight the violating path in a dataflow diagram. A conversational projection might explain the violation in natural language: "The pointer `p` may be used after the region it points to is freed. This can happen when the function returns early on line 42 without freeing the buffer. Consider adding a cleanup handler for the early return path."

---

## 6. Pi 5 Deployment

The VUMA framework targets the Raspberry Pi 5 as its reference hardware platform. The Pi 5 features a Broadcom BCM2712 SoC with four ARM Cortex-A76 cores clocked at 2.4 GHz, 4 GB or 8 GB of LPDDR4X-4267 RAM, and a rich set of peripherals (GPIO, UART, I2C, SPI, DMA, PCIe). This makes it an ideal target for VUMA: it is real hardware with real memory-mapped I/O (exercising VUMA's pointer verification on device registers), real multicore concurrency (exercising VUMA's exclusivity invariant), and real performance constraints (exercising COR's profile-guided optimization).

### Execution Modes

```
┌─────────────────────────────────────────────────────────────┐
│                 Pi 5 Execution Modes                         │
│                                                             │
│  Mode A: Linux Userspace           Mode B: Bare Metal       │
│  ┌─────────────────────────┐      ┌─────────────────────┐   │
│  │    VUMA Application     │      │   VUMA Application  │   │
│  ├─────────────────────────┤      ├─────────────────────┤   │
│  │    COR Runtime          │      │   COR Runtime       │   │
│  │    (userspace services) │      │   (bare metal)      │   │
│  ├─────────────────────────┤      ├─────────────────────┤   │
│  │    Linux Kernel         │      │   Boot Stub         │   │
│  │    (standard syscalls)  │      │   (UART, mbox)      │   │
│  ├─────────────────────────┤      ├─────────────────────┤   │
│  │    BCM2712 Hardware     │      │   BCM2712 Hardware  │   │
│  │    (via /dev/gpiomem,   │      │   (direct MMIO)     │   │
│  │     /dev/mem, sysfs)    │      │                     │   │
│  └─────────────────────────┘      └─────────────────────┘   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

**Mode A: Linux Userspace.** In this mode, the VUMA application runs as a standard Linux process on the Pi 5's Raspberry Pi OS. The COR provides userspace services: a memory allocator that respects VUMA's verified-unsafe model (direct `mmap` for large allocations, custom allocator for small ones), peripheral access through `/dev/gpiomem` and `/dev/mem` (with IVE-verified pointer arithmetic for register access), and multicore threading via standard POSIX threads. This mode is used for development, testing, and applications that need Linux services (networking, filesystem, etc.).

**Mode B: Bare Metal.** In this mode, the VUMA application runs directly on the BCM2712 hardware with no OS. The COR provides bare-metal services: a boot stub that initializes the UART and mailbox interface, a memory allocator that manages physical RAM directly, peripheral access through memory-mapped I/O (with the IVE verifying every device register access), and multicore execution by parking secondary cores in a WFE loop and dispatching work via shared memory. This mode is used for real-time applications, embedded control systems, and situations where Linux overhead is unacceptable.

### Memory Mapping for GPIO/UART

```
┌──────────────────────────────────────────────────────────────────┐
│               Pi 5 Memory Map (VUMA Perspective)                  │
│                                                                  │
│  Address Range          Size     Description                     │
│  ─────────────────────────────────────────────────────────────── │
│  0x0000_0000 – 0x3FFF_FFFF  1 GB   RAM (LPDDR4X)               │
│  0x4_0000_0000 – ...        var    RAM (high, if 8 GB model)    │
│  0x7C00_0000 – 0x7C00_FFFF  64 KB  BCM2712 GPIO Registers      │
│  0x7C21_0000 – 0x7C21_0FFF  4 KB   BCM2712 UART Registers      │
│  0x7C80_4000 – 0x7C80_4FFF  4 KB   BCM2712 I2C Registers       │
│  0x7C80_5000 – 0x7C80_5FFF  4 KB   BCM2712 SPI Registers       │
│  0x7C90_0000 – 0x7C90_0FFF  4 KB   BCM2712 DMA Registers       │
│                                                                  │
│  VUMA pointer verification for MMIO:                             │
│                                                                  │
│  gpio_ptr = 0x7C00_0000;           // address literal           │
│  // IVE: Origin check — is this a valid allocation?              │
│  //   YES — device-mapped region (special allocation kind)       │
│  // IVE: Interpretation check — is the RepD valid?              │
│  //   YES — GPIO register layout matches the access pattern     │
│  // IVE: Exclusivity check — is concurrent access safe?         │
│  //   YES — device registers are atomic by hardware design      │
│                                                                  │
│  *gpio_ptr = 0x01;  // write to GPIO register                   │
│  // IVE: PROVEN SAFE — all three checks pass                    │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

VUMA treats memory-mapped I/O regions as special allocation kinds. The IVE's origin invariant recognizes device-mapped addresses as valid "allocations" (they are always live, never freed). The interpretation invariant verifies that the access pattern matches the hardware register layout (defined in the `pi5` crate as `RepD` values). The exclusivity invariant recognizes that device registers are inherently atomic and concurrent access is safe (or requires specific synchronization, as documented in the BCM2712 datasheet). This means VUMA can verify hardware access code with the same rigor as software memory access — a capability that no existing language provides.

### Multi-Core Execution via COR

```
┌──────────────────────────────────────────────────────────────────┐
│              COR Multi-Core Execution Model                       │
│                                                                  │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────┐│
│  │  Core 0     │  │  Core 1     │  │  Core 2     │  │ Core 3  ││
│  │  (primary)  │  │  (worker)   │  │  (worker)   │  │(worker) ││
│  │             │  │             │  │             │  │         ││
│  │ SCG region: │  │ SCG region: │  │ SCG region: │  │SCG reg: ││
│  │ main()      │  │ dataflow_1  │  │ dataflow_2  │  │dataflow ││
│  │             │  │             │  │             │  │   _3    ││
│  │ Runs:       │  │ Runs:       │  │ Runs:       │  │Runs:    ││
│  │ - COR       │  │ - dataflow  │  │ - dataflow  │  │- dataflw││
│  │ - IVE       │  │   nodes     │  │   nodes     │  │  nodes  ││
│  │ - profile   │  │ - no GC     │  │ - no GC     │  │- no GC  ││
│  │ - dispatch  │  │ - no locks* │  │ - no locks* │  │- no lks*││
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘  └────┬────┘│
│         │                │                │              │      │
│         └────────────────┴────────────────┴──────────────┘      │
│                          │                                       │
│                   ┌──────┴──────┐                                │
│                   │ Shared      │  (IVE-verified concurrent     │
│                   │ Memory      │   access — no data races)     │
│                   └─────────────┘                                │
│                                                                  │
│  * No locks when IVE proves non-overlapping access patterns.     │
│    Locks inserted only for accesses the IVE cannot prove safe.   │
└──────────────────────────────────────────────────────────────────┘
```

The COR dispatches SCG regions to Pi 5 cores based on data flow dependencies. The primary core (Core 0) runs the COR main loop, which coordinates dispatch, collects profile data, and runs incremental IVE verification. Worker cores (Cores 1–3) execute dataflow regions — independent subgraphs of the SCG that can run concurrently. The key innovation is that VUMA's exclusivity invariant determines whether synchronization is needed: if the IVE can prove that two concurrent accesses target non-overlapping memory regions, no lock is needed. If the IVE cannot prove this, it inserts the minimal necessary synchronization (a lock, an atomic, or a barrier) and verifies that the inserted synchronization is correct.

### Cache-Optimized Allocation Strategies

The COR's memory allocator is aware of the Pi 5's cache architecture (L1: 64 KB per core, L2: 1 MB shared) and uses VUMA's RepD information to optimize allocation:

- **Cache-line alignment:** Allocations are aligned to 64-byte cache lines by default. The allocator uses the RepD's alignment field to determine the correct alignment for each allocation.
- **Structure-of-arrays transformation:** When the IVE detects that a loop accesses only a subset of fields from a struct, it can transform the data layout from array-of-structures (AoS) to structure-of-arrays (SoA) to improve cache utilization. This transformation is a verified graph rewrite — the IVE proves that the transformed SCG is semantically equivalent to the original.
- **NUMA-aware placement:** Although the Pi 5 is UMA (uniform memory access), the allocator can still benefit from placing frequently co-accessed data in the same L2 cache line set, reducing L2 miss rates.
- **Prefetch hints:** The codegen emits ARM `PRFM` (prefetch memory) instructions based on profile data, prefetching data into L1 or L2 cache before it is accessed.

---

## 7. Build System

The VUMA project uses Cargo as its build system, with workspace-level configuration for multi-crate management. The build system supports two target configurations: Linux userspace (for development and testing) and bare metal (for deployment on Pi 5 hardware). Building and testing the project requires the `aarch64-unknown-linux-gnu` and `aarch64-unknown-none` toolchain targets.

### Build Commands

```bash
# ──────────────────────────────────────────────────────────────
# VUMA Build System — Quick Reference
# ──────────────────────────────────────────────────────────────

# 1. Install required targets (one-time setup)
rustup target add aarch64-unknown-linux-gnu
rustup target add aarch64-unknown-none

# 2. Build for Linux userspace (development + testing)
cargo build --target aarch64-unknown-linux-gnu

# 3. Build for bare metal (Pi 5 deployment)
cargo build --target aarch64-unknown-none \
    --no-default-features \
    --features "bare-metal,pi5"

# 4. Build in release mode (optimized, LTO enabled)
cargo build --release --target aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-none \
    --no-default-features \
    --features "bare-metal,pi5"

# 5. Run all tests (Linux userspace only)
cargo test

# 6. Run tests for a specific crate
cargo test -p vuma-scg
cargo test -p vuma-ive
cargo test -p vuma-bd

# 7. Run VUMA verification tests (end-to-end pipeline)
cargo test -p vuma-tests --test verification_pipeline

# 8. Generate documentation
cargo doc --no-deps --open

# 9. Run the VUMA CLI (compile + verify a source file)
cargo run -- compile --input examples/doubly_linked_list.vuma \
    --target aarch64-unknown-linux-gnu \
    --verify --report textual

# 10. Cross-compile from x86_64 host to Pi 5
#     (requires aarch64 cross-linker)
cargo build --target aarch64-unknown-linux-gnu \
    --config target.aarch64-unknown-linux-gnu.linker="aarch64-linux-gnu-gcc"
```

### Build Configuration

```
┌──────────────────────────────────────────────────────────────────┐
│                    Cargo.toml Configuration                       │
│                                                                  │
│  [workspace]                                                     │
│  members = [                                                     │
│      "src/scg",        # Semantic Computation Graph              │
│      "src/ive",        # Inference & Verification Engine         │
│      "src/vuma",       # VUMA Memory Model                      │
│      "src/bd",         # Behavioral Descriptors                  │
│      "src/cor",        # Continuous Optimization Runtime         │
│      "src/projection", # Projection System                       │
│      "src/parser",     # Lexer + Parser + AST                    │
│      "src/codegen",    # ARM64 Code Generation                   │
│      "src/pi5",        # Pi 5 Platform Support                   │
│      "src/std",        # Standard Library                        │
│      "src/proof",      # Formal Proof Engine                     │
│      "src/tests",      # Integration Tests                       │
│  ]                                                               │
│                                                                  │
│  [profile.release]                                               │
│  opt-level = 3          # maximum optimization                   │
│  lto = true             # link-time optimization                 │
│  codegen-units = 1      # single codegen unit (better opts)      │
│  target-cpu = "native"  # use all available ARM64 instructions   │
│                                                                  │
│  [profile.dev]                                                   │
│  opt-level = 0          # no optimization (fast compile)         │
│  debug = true           # full debug info                        │
│  overflow-checks = true # catch integer overflow in dev          │
│                                                                  │
│  Feature flags (in workspace crates):                            │
│                                                                  │
│  [features]                                                      │
│  default = ["linux-userspace"]                                   │
│  linux-userspace = []   # Linux syscalls, /dev/gpiomem           │
│  bare-metal = []        # no OS, direct MMIO, custom boot        │
│  pi5 = []               # BCM2712-specific peripherals           │
│  proof-check = []       # enable proof verification at runtime   │
│  profile-guided = []    # enable COR profile feedback loop       │
│  verbose-verify = []    # detailed verification logging          │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

### Build Pipeline (CI/CD)

```
┌──────────────────────────────────────────────────────────────────┐
│                    CI/CD Pipeline                                  │
│                                                                  │
│  Commit ──► Format Check ──► Clippy ──► Build (2 targets) ──►   │
│                                                                  │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │  Stage 1: Format & Lint                                     │ │
│  │    cargo fmt --check                                        │ │
│  │    cargo clippy --all-targets -- -D warnings                │ │
│  └───────────────────────────┬─────────────────────────────────┘ │
│                              │                                    │
│  ┌───────────────────────────▼─────────────────────────────────┐ │
│  │  Stage 2: Build                                             │ │
│  │    cargo build --target aarch64-unknown-linux-gnu           │ │
│  │    cargo build --target aarch64-unknown-none                │ │
│  │      --no-default-features --features "bare-metal,pi5"      │ │
│  └───────────────────────────┬─────────────────────────────────┘ │
│                              │                                    │
│  ┌───────────────────────────▼─────────────────────────────────┐ │
│  │  Stage 3: Test                                              │ │
│  │    cargo test --target aarch64-unknown-linux-gnu            │ │
│  │    cargo test -p vuma-tests --test verification_pipeline    │ │
│  │    cargo test -p vuma-scg    # unit tests for SCG           │ │
│  │    cargo test -p vuma-ive    # unit tests for IVE           │ │
│  │    cargo test -p vuma-bd     # unit tests for BD            │ │
│  │    cargo test -p vuma-vuma   # unit tests for VUMA          │ │
│  └───────────────────────────┬─────────────────────────────────┘ │
│                              │                                    │
│  ┌───────────────────────────▼─────────────────────────────────┐ │
│  │  Stage 4: Verification Benchmark                            │ │
│  │    cargo run --release -- benchmark \                       │ │
│  │      --suite standard \                                     │ │
│  │      --output results.json                                  │ │
│  └───────────────────────────┬─────────────────────────────────┘ │
│                              │                                    │
│  ┌───────────────────────────▼─────────────────────────────────┐ │
│  │  Stage 5: Deploy to Pi 5 (on tag)                           │ │
│  │    scp target/aarch64-unknown-none/release/vuma pi5:~/      │ │
│  │    ssh pi5 "./vuma --self-test"                             │ │
│  └─────────────────────────────────────────────────────────────┘ │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

### Directory Structure for Build Artifacts

```
target/
├── aarch64-unknown-linux-gnu/
│   ├── debug/
│   │   ├── vuma                  # CLI binary (debug)
│   │   ├── libvuma_scg.rlib      # SCG library
│   │   ├── libvuma_ive.rlib      # IVE library
│   │   └── ...
│   └── release/
│       ├── vuma                  # CLI binary (release, LTO)
│       └── ...
└── aarch64-unknown-none/
    ├── debug/
    │   ├── vuma                  # bare-metal binary (debug)
    │   └── ...
    └── release/
        ├── vuma                  # bare-metal binary (release)
        └── ...
```

The bare-metal target produces a single statically-linked binary that can be loaded directly onto the Pi 5's SD card or transferred via UART. The Linux userspace target produces a standard ELF binary that runs on Raspberry Pi OS. Both targets share the same SCG, IVE, BD, VUMA, and Proof code — only the COR and Pi5 crates have target-specific implementations, selected via Cargo feature flags.

---

*End of Architecture Overview. For questions or contributions, refer to the VUMA project repository and the proposal document at `/home/z/my-project/download/proposal-ai-designed-programming-languages.md`.*
