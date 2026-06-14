# VUMA — Verified-Unsafe Memory Access

**An AI-native programming language framework that replaces traditional type systems with behavioral verification.**

[![CI](https://github.com/vuma-lang/vuma/actions/workflows/ci.yml/badge.svg)](https://github.com/vuma-lang/vuma/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

---

## Table of Contents

1. [Overview](#overview)
2. [Architecture](#architecture)
3. [Quick Start](#quick-start)
4. [Building for AArch64](#building-for-aarch64)
5. [Running Tests](#running-tests)
6. [Project Structure](#project-structure)
7. [Key Concepts](#key-concepts)
8. [Example Programs](#example-programs)
9. [Documentation](#documentation)
10. [Contributing](#contributing)
11. [License](#license)

---

## Overview

VUMA is a programming language framework built on a radical premise: **unsafe memory operations should not be forbidden — they should be made verifiable.** Instead of relying on a borrow checker to reject programs that cannot be statically proven safe, VUMA constructs a formal model of every memory operation and verifies global invariants against that model. Programs that pass verification run without runtime overhead; programs that fail receive precise counterexamples showing the exact execution path to the violation.

### Why VUMA?

Traditional languages face a fundamental trade-off: either restrict what programmers can express (Rust's borrow checker, Java's GC) or accept memory-unsafe programs (C, C++). VUMA breaks this trade-off through three innovations:

1. **Semantic Computation Graphs (SCGs)** replace source code as the primary program representation. The SCG is a directed, acyclic, attributed multigraph where nodes are operations, edges are relationships, and regions delineate scopes and deployment targets.

2. **Behavioral Descriptors (BDs)** replace nominal types. A BD is the triple (RepD, CapD, RelD) — representation, capabilities, and relationships — inferred from program structure rather than declared by the programmer.

3. **Verification over restriction** — instead of rejecting programs that might be unsafe, VUMA verifies that they are safe and provides precise diagnostics when they are not.

### The Five VUMA Invariants

Every VUMA program is verified against five global memory-safety invariants:

| Invariant       | Ensures                                            |
|-----------------|----------------------------------------------------|
| **Liveness**    | Every access targets allocated memory              |
| **Exclusivity** | No conflicting concurrent accesses                 |
| **Interpretation** | Every access uses a valid representation         |
| **Origin**      | Every address traces to a valid allocation         |
| **Cleanup**     | Every region is eventually freed or explicitly leaked |

---

## Architecture

VUMA implements a six-layer architecture where data flows from human intent through graph construction, inference, verification, code generation, and into execution on bare metal. Feedback flows upward at every stage.

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Layer 3 — Projection System                       │
│    Textual · Visual · Conversational · Bidirectional · Diff         │
├─────────────────────────────────────────────────────────────────────┤
│                    Parser / Frontend (auxiliary)                     │
│    Lexer → Parser → AST → AST-to-SCG Lowering                       │
├─────────────────────────────────────────────────────────────────────┤
│             Layers 2, 5, 6 — Reasoning Core                         │
│    IVE (Inference + Verification) · BD (Descriptors) · MSG (Memory) │
│    Invariants: Liveness · Exclusivity · Interpretation ·             │
│                Origin · Cleanup                                      │
├─────────────────────────────────────────────────────────────────────┤
│                    Layer 1 — SCG (Core Representation)               │
│    Nodes (ops, allocs, effects) · Edges (data flow, deps) · Regions │
├─────────────────────────────────────────────────────────────────────┤
│                    Layer 4 — Execution                               │
│    COR Runtime (always-compiled, PGO, JIT) · Multi-ISA Codegen      │
└─────────────────────────────────────────────────────────────────────┘
```

### Pipeline

```
Source Text → Lexer → Parser → AST → SCG Lowering → Raw SCG
    → BD Inference (RepD + CapD + RelD fixpoint) → Annotated SCG
    → MSG Builder → VUMA Verification (5 invariants) → Verified SCG
    → ARM64 Codegen (IR → regalloc → emit) → Machine Code
    → COR Runtime → Execution → Profile Feedback → re-optimize
```

### Crate Dependency Graph

```
                    ┌──────────┐
                    │  tests   │  (depends on everything)
                    └────┬─────┘
           ┌─────────────┼──────────────────┐
           ▼             ▼                  ▼
    ┌────────────┐ ┌──────────┐      ┌──────────┐
    │ projection │ │   std    │      │  codegen  │
    └─────┬──────┘ └────┬─────┘      └─────┬────┘
          │              │                   │
          ▼              ▼                   ▼
    ┌──────────┐  ┌──────────┐  ┌──────────────────┐
    │   cor    │  │  vuma    │  │    ive · proof    │
    └────┬─────┘  └────┬─────┘  └────┬─────────────┘
         │             │              │
         └─────────────┼──────────────┘
                       ▼
                ┌──────────────┐
                │  bd · std    │
                └──────┬───────┘
                       ▼
                ┌──────────────┐
                │     scg      │  ◄── foundation (zero workspace deps)
                └──────────────┘
```

---

## Quick Start

### Prerequisites

- **Rust** — nightly toolchain (pinned in `rust-toolchain.toml`)
- **Make** or **Just** — build orchestration
- **QEMU** (optional) — emulation for testing
- **aarch64 cross-toolchain** (optional) — AArch64 bare-metal builds

### Setup

```bash
# Clone the repository
git clone https://github.com/vuma-lang/vuma.git
cd vuma

# Install the pinned toolchain, components, and targets
make setup
# or: just setup

# Build the entire workspace
make build
# or: cargo build --workspace

# Run all tests
make test
# or: cargo test --workspace
```

### First Program

Create `hello.vuma`:

```vuma
region main_region {
    let x: ptr<u8> = allocate(1) in main_region;
    write x, 42;
    let val: u8 = read x;
    deallocate x from main_region;
}
```

Compile and verify:

```bash
cargo run -- verify hello.vuma
```

The verifier will check all five invariants against the SCG derived from your program. If verification passes, you get zero-overhead ARM64 machine code. If it fails, you get a precise counterexample showing the exact path to the violation.

---

## Building for AArch64

VUMA supports multi-ISA code generation including **AArch64** (ARM64). The codegen crate can produce ARM64 machine code for deployment on AArch64 targets.

---

## Running Tests

### All Tests

```bash
make test
# or: cargo test --workspace
```

### Per-Crate Tests

```bash
# SCG core
cargo test -p vuma-scg

# Inference and verification engine
cargo test -p vuma-ive

# Behavioral descriptors
cargo test -p vuma-bd

# VUMA memory model
cargo test -p vuma-core

# ARM64 code generation
cargo test -p vuma-codegen

# Standard library
cargo test -p vuma-std

# Proof system
cargo test -p vuma-proof

# Parser
cargo test -p vuma-parser
```

### Specific Test

```bash
cargo test -p vuma-ive -- liveness
cargo test -p vuma-core -- invariant_exclusivity
```

### Verification Tests (Thread-Sensitive)

IVE verification tests use `--test-threads=1` because they depend on deterministic ordering:

```bash
cargo test -p vuma-ive -- --test-threads=1
```

### Benchmarks

```bash
make bench
# or: cargo bench --workspace

# Single crate
cargo bench -p vuma-tests
```

The benchmark suite covers 8 categories with 40+ individual benchmarks:
- SCG construction (99–9999 nodes)
- BD inference (3 sizes × 3 operations)
- MSG construction (60–3000 nodes)
- IVE verification (per-invariant + incremental)
- ARM64 codegen (statement + function counts)
- C-equivalent comparison
- Memory usage (5 measurement points × 3 sizes)
- End-to-end pipeline

### Code Quality

```bash
# Format check
make fmt-check

# Lint
make clippy

# Full CI check
make lint && make test
```

---

## Project Structure

```
vuma/
├── Cargo.toml              # Workspace root (12 crate members)
├── Makefile                # Build/test targets
├── justfile                # Just command runner shortcuts
├── rust-toolchain.toml     # Pinned nightly toolchain
├── src/
│   ├── lib.rs              # Workspace crate root
│   ├── pipeline.rs         # Top-level compilation pipeline
│   ├── scg/                # Layer 1 — Semantic Computation Graph
│   │   └── src/            # node, edge, graph, region, query, dominance,
│   │                       # liveness, transform, diff, serialize
│   ├── bd/                 # Layer 5 — Behavioral Descriptors
│   │   └── src/            # repd, capd, reld, descriptor, inference,
│   │                       # context, context_solver, capd_lattice,
│   │                       # reld_refine, repd_compat, unify
│   ├── vuma/               # Layer 6 — VUMA Memory Model
│   │   └── src/            # msg, msg_builder, msg_incremental,
│   │                       # scg_to_msg, 5 invariant checkers,
│   │                       # access_analysis, security, repl
│   ├── ive/                # Layer 2 — Inference & Verification Engine
│   │   └── src/            # inference, bd_solver, constraint,
│   │                       # 5 verifiers, invariant_aggregator,
│   │                       # result, debt
│   ├── cor/                # Layer 4 — Continuous Optimization Runtime
│   │   └── src/            # runtime, profile, speculative, optimization,
│   │                       # deployment, config, types
│   ├── projection/         # Layer 3 — Projection System
│   │   └── src/            # textual, visual, conversational,
│   │                       # bidirectional, diff
│   ├── parser/             # Parser / Frontend
│   │   └── src/            # lexer, parser, ast, to_scg, error
│   ├── codegen/            # Multi-ISA Code Generation
│   │   └── src/            # arm64, ir, scg_to_ir, regalloc, emit
│   ├── proof/              # Formal Proof System
│   │   └── src/            # proof, checker, rules, tactics,
│   │                       # counterexample, 5 invariant proof modules
│   ├── std/                # Standard Library
│   │   └── src/            # primitives, alloc, collections, sync, io
│   └── tests/              # Integration Tests & Benchmarks
│       └── src/            # framework, trivial, dlist, bd_inference,
│                           # concurrent, graph, benchmarks
├── examples/               # VUMA example programs (*.vuma)
├── docs/                   # Documentation
│   ├── architecture.md     # Full architecture document
│   ├── language-reference.md # VUMA language reference
│   ├── ROADMAP.md          # 5-phase project roadmap
│   ├── CONTRIBUTING.md     # Contributor guidelines
│   ├── CONVENTIONS.md      # Coding conventions
│   ├── GLOSSARY.md         # Project glossary (40+ terms)
│   ├── WORKLOG.md          # Detailed work log
│   └── specs/              # 15 formal specification documents
├── .cargo/config.toml      # Cargo cross-compilation config
└── .github/workflows/ci.yml # GitHub Actions CI pipeline
```

---

## Key Concepts

### SCG — Semantic Computation Graph

The SCG is the single source of truth in VUMA. There is no "source code" that the compiler translates — the SCG *is* the program, and every other representation (textual, visual, conversational, machine code) is a **projection** of the SCG. Nodes represent computational operations (allocations, accesses, computations, casts, effects, control flow), edges represent relationships (data flow, control flow, derivation, annotation), and regions delineate scopes, phases, and deployment targets.

### BD — Behavioral Descriptor

A Behavioral Descriptor replaces traditional nominal types with the triple (RepD, CapD, RelD):

- **RepD** (Representation Descriptor): memory layout — size, alignment, field offsets, multiple simultaneous interpretations
- **CapD** (Capability Descriptor): permitted operations — read, write, execute, serialize, send, persist, derive-pointer (context-dependent)
- **RelD** (Relational Descriptor): relationships — temporal co-occurrence, structural containment, dependency ordering, semantic equivalence, security-level flow

BDs are **inferred**, not declared. The IVE derives them from SCG structure through iterative fixpoint computation.

### MSG — Memory State Graph

The Memory State Graph captures every allocation point, every pointer derivation, every deallocation point, every concurrent access, and every reinterpretation. It is constructed from the annotated SCG and serves as the formal model against which the five invariants are verified.

### IVE — Inference & Verification Engine

The IVE is the reasoning core. It reads the SCG, infers Behavioral Descriptors, constructs the MSG, and verifies the five global invariants. It operates through iterative fixpoint computation, resolving interdependencies between RepD, CapD, and RelD inference.

### COR — Continuous Optimization Runtime

The COR maintains an always-compiled invariant: every reachable SCG region is kept in compiled ARM64 machine code at all times. It performs incremental compilation, profile-guided optimization (using PMU counters), speculative optimization with transparent deoptimization, and adaptive deployment across heterogeneous targets.

---

## Example Programs

| Example                   | Lines | Demonstrates                                      |
|---------------------------|-------|---------------------------------------------------|
| `hello_memory.vuma`       | 40    | Basic allocate/write/read/free                    |
| `doubly_linked_list.vuma` | 89    | Sentinel node pattern, derivation chains          |
| `arena_allocator.vuma`    | 78    | Arena allocation with region semantics            |
| `gpio_blink.vuma`         | 68    | GPIO hardware access                           |
| `lock_free_queue.vuma`    | 99    | Lock-free SPSC queue with atomics                |
| `channel_demo.vuma`       | 237   | Channel-based concurrency                        |
| `memory_arena.vuma`       | 197   | Region-based allocation                          |
| `aarch64_sensor.vuma`     | 188   | AArch64 MMIO sensor reading                   |
| `sorted_map.vuma`         | 192   | Sorted map with BD-verified operations           |
| `thread_pool.vuma`        | 209   | Thread pool with work stealing                   |

---

## Documentation

| Document                    | Description                                           |
|-----------------------------|-------------------------------------------------------|
| [Architecture](docs/architecture.md) | Full 8-section architecture document (994 lines) |
| [Language Reference](docs/language-reference.md) | Complete VUMA syntax and semantics (1101 lines) |
| [Roadmap](docs/ROADMAP.md) | 5-phase development roadmap with milestones          |
| [Contributing](docs/CONTRIBUTING.md) | Build, test, add features, code review process   |
| [Conventions](docs/CONVENTIONS.md) | Rust style, error handling, testing, naming, docs |
| [Glossary](docs/GLOSSARY.md) | 40+ defined terms across all domains               |
| [Formal Specs](docs/specs/) | 15 formal specification documents                   |

### Formal Specifications

| Specification                        | Lines | Topic                                    |
|--------------------------------------|-------|------------------------------------------|
| `scg-formal-spec.md`                 | 475   | SCG mathematical model                   |
| `repd-formal-spec.md`                | 546   | Representation descriptor lattice        |
| `capd-formal-spec.md`                | 492   | Capability descriptor lattice            |
| `reld-formal-spec.md`                | 600   | Relational descriptor kinds              |
| `vuma-invariants-spec.md`            | 742   | Five VUMA invariants                     |
| `msg-construction-spec.md`           | 850   | MSG construction algorithm               |
| `aarch64-memory-model-spec.md`      | 809   | AArch64 memory model                   |
| `security-model-spec.md`             | 606   | Security model and threat categories     |
| `bd-inference-algorithm.md`          | 1027  | BD inference fixpoint algorithm          |
| `vuma-verification-algorithm.md`     | 1098  | VUMA verification algorithm              |
| `arm64-codegen-algorithm.md`         | 1182  | ARM64 codegen algorithm                  |
| `benchmark-design.md`                | 695   | Benchmark methodology and categories     |
| `trivial-proofs.md`                  | 547   | Trivial program proofs                   |
| `dlist-proof.md`                     | 631   | Doubly-linked list proof                 |
| `decidability-analysis.md`           | 416   | Decidability analysis                    |

---

## Contributing

See [CONTRIBUTING.md](docs/CONTRIBUTING.md) for the complete contributor guide, including:

- **How to Build** — prerequisites, workspace layout, Make/just commands
- **How to Run Tests** — unit, integration, verification, and benchmark tests
- **How to Add SCG Node Types** — 8-step process with examples
- **How to Add Verification Passes** — 9-step process with formal spec requirements
- **How to Add ARM64 Instructions** — 7-step process with encoding verification
- **Code Review Process** — 7 review criteria, timeline, special rules
- **PR Template** — summary, verification impact, test plan, checklist

Quick start for contributors:

```bash
# Setup
make setup

# Format + lint + test
make lint && make test

# Run per-crate tests during development
cargo test -p vuma-scg    # fast iteration
```

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.

---

## Worklog

- **2026-03-05 — Task 5-9:** Created comprehensive README.md with overview, architecture, quick start, AArch64 build instructions, test instructions, project structure, key concepts, examples, documentation index, and contributing link.
