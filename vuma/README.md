# VUMA — Verified-Unsafe Memory Access

**An AI-native programming language framework that replaces traditional type systems with behavioral verification.**

[![CI](https://github.com/vuma-lang/vuma/actions/workflows/ci.yml/badge.svg)](https://github.com/vuma-lang/vuma/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Version: 0.2.0](https://img.shields.io/badge/version-0.2.0-green.svg)](CHANGELOG.md)

---

## Table of Contents

1. [Overview](#overview)
2. [Features](#features)
3. [Architecture](#architecture)
4. [VUMA for LLMs](#vuma-for-llms)
5. [Quick Start](#quick-start)
6. [8 Backend Architectures](#8-backend-architectures)
7. [Running Tests](#running-tests)
8. [Project Structure](#project-structure)
9. [Key Concepts](#key-concepts)
10. [Example Programs](#example-programs)
11. [API Examples](#api-examples)
12. [Documentation](#documentation)
13. [Contributing](#contributing)
14. [License](#license)

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

## Features

### Multi-Architecture Codegen (8 Backends)

VUMA compiles to **8 CPU/platform targets** with a unified backend trait architecture:

| Backend | Architecture | Status | ABI |
|---------|-------------|--------|-----|
| x86_64 | x86-64 (SysV) | ✅ Stable | System V AMD64 |
| AArch64 | ARM64 (AAPCS64) | ✅ Stable | AAPCS64 |
| RISC-V 64 | RV64G | ✅ Stable | LP64D |
| ARM32 | ARMv7-A (AAPCS) | ✅ Stable | AAPCS |
| MIPS64 | MIPS III (N64) | ✅ Stable | N64 |
| PPC64 | PowerPC v2 (ELFv2) | ✅ Stable | ELFv2 |
| LoongArch64 | LA64 | 🔄 Experimental | LP64 |
| Wasm32 | WebAssembly | 🔄 Experimental | Stack machine |

All 6 native backends pass the full SHA256d execution test. Wasm32 generates valid modules. LoongArch64 passes individual operation tests.

### AI-Native Design

VUMA is designed from the ground up for programmatic consumption by AI agents:

- **VumaForLLM API** — stateless `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()`
- **LSP Server** — full protocol: diagnostics, hover, go-to-definition, completion, semantic tokens
- **Enhanced REPL** — `:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`, tab completion, color output
- **Wasm32 Sandbox** — LLM agents compile to safe, sandboxed WebAssembly modules
- **65 Diagnostic Codes** — E001-E050, W001-W010, I001-I005 with error chaining and structured suggestions

### Memory Safety Verification

- **10 violation types** detected at compile time: UseAfterFree, DoubleFree, MemoryLeak, BoundsCheckFailure, NullDereference, DanglingPointer, UninitializedRead, BufferOverflow, UseAfterScope, InvalidFree
- **Runtime bounds checking** behind `--safe` flag
- **Constant-time crypto**: Branchless `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte` across all 8 backends

### Module System & Package Manager

- **Multi-file compilation**: `import "crypto.vuma"::{sha256, sha256d};` with circular import detection
- **Package manager**: `vuma pkg init`, `vuma pkg build`, `vuma pkg add` with dependency resolution

### FFI & System Calls

- **19 Linux syscalls** across all 8 architectures (read, write, open, close, exit, mmap, munmap, brk, ioctl, fcntl, getpid, clone, futex, etc.)
- **Architecture-specific relocations** for all backends
- **`extern "C" { fn ...; }`** FFI blocks with is_extern flag propagation

---

## Architecture

VUMA implements a six-layer architecture where data flows from human intent through graph construction, inference, verification, code generation, and into execution on bare metal. Feedback flows upward at every stage.

```
┌─────────────────────────────────────────────────────────────────────┐
│                    LLM Integration Layer                             │
│    VumaForLLM API · LSP Server · REPL · Structured Diagnostics     │
├─────────────────────────────────────────────────────────────────────┤
│                    Layer 3 — Projection System                       │
│    Textual · Visual · Conversational · Bidirectional · Diff         │
├─────────────────────────────────────────────────────────────────────┤
│                    Parser / Frontend (auxiliary)                     │
│    Lexer → Parser → AST → AST-to-SCG Lowering · Module Resolution  │
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
│    x86_64 · AArch64 · RISC-V 64 · ARM32 · MIPS64 · PPC64           │
│    LoongArch64 · Wasm32                                             │
└─────────────────────────────────────────────────────────────────────┘
```

### Pipeline

```
Source Text → Lexer → Parser → AST → SCG Lowering → Raw SCG
    → Module Resolution (imports) → Merged SCG
    → BD Inference (RepD + CapD + RelD fixpoint) → Annotated SCG
    → MSG Builder → VUMA Verification (5 invariants) → Verified SCG
    → Multi-Arch Codegen (IR → regalloc → emit) → Machine Code / Wasm
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

## VUMA for LLMs

VUMA is designed as an **AI-native** language framework. Every interface is designed for programmatic consumption by AI agents. This LLM-first design allows AI coding agents to use VUMA as a verified compilation sandbox: parse source, analyze structure, run verification, compile to any of 8 backends, and inspect results — all through clean API boundaries.

### LLM-Facing Interfaces

| Interface | Description | Use Case |
|-----------|-------------|----------|
| `VumaForLLM` API | `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()` | LLM compiles and verifies code in a sandbox |
| `VumaCompiler` API | `compile()`, `parse()`, `analyze()`, `validate()`, `verify()` | Full pipeline with verification reports |
| LSP Server | Full LSP protocol: diagnostics, hover, go-to-definition, completion | IDE integration and LLM agent interaction |
| REPL `:wasm` | Compile to Wasm and show binary size | Quick Wasm compilation check |
| REPL `:backends` | List 8 backends with status | Discover compilation capabilities |
| REPL `:check` | Run IVE verification | Instant verification feedback |
| REPL `:diagnostics` | Show all diagnostics as JSON | Structured error analysis |
| REPL `:exports` | List all functions and signatures | Program structure inspection |

### Wasm32 Sandbox for LLM Agents

The Wasm32 backend enables LLM agents to compile VUMA programs into safe, sandboxed WebAssembly modules. This is the recommended execution path for LLM-generated code: the Wasm module runs in a sandboxed environment with no access to host memory or peripherals, ensuring that LLM-generated code cannot cause harm.

```rust
// LLM agent compiles VUMA to Wasm
let result = VumaForLLM::compile(source);
if let Some(wasm) = &result.wasm_binary {
    println!("Wasm module: {} bytes", wasm.len());
}
```

### Parser Hardening for LLM-Generated Code

The VUMA parser is specifically hardened for code generated by LLMs:

- **LLM type aliases**: `int` → `i32`, `float` → `f32`, `double` → `f64`, `String` → `string`
- **C/Rust construct detection**: `println!`, `vec!`, `format!`, `panic!` produce helpful errors
- **C-style for loop detection**: `for (i=0; i<n; i++)` → specific error with VUMA suggestion
- **Reference type conversion**: `&T` / `&mut T` auto-convert to `*T` (pointer type)

---

## Quick Start

### Prerequisites

- **Rust** — nightly toolchain (pinned in `rust-toolchain.toml`)
- **Make** or **Just** — build orchestration
- **QEMU** (optional) — emulation for testing cross-architecture backends

### Installation

```bash
# Clone the repository
git clone https://github.com/vuma-lang/vuma.git
cd vuma

# Install the pinned toolchain, components, and targets
make setup

# Build the entire workspace
make build

# Run all tests
make test
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
# Verify against all 5 VUMA invariants
cargo run -- verify hello.vuma

# Compile to a specific backend
cargo run -- build --target x86_64 hello.vuma
cargo run -- build --target aarch64 hello.vuma
cargo run -- build --target wasm32 hello.vuma

# Quick syntax and semantic check
cargo run -- check hello.vuma

# Run with memory safety analysis
cargo run -- run --safe hello.vuma

# Compile with debug info
cargo run -- build --debug hello.vuma
```

The verifier will check all five invariants against the SCG derived from your program. If verification passes, you get zero-overhead machine code for any of the 8 backends. If it fails, you get a precise counterexample showing the exact path to the violation.

---

## 8 Backend Architectures

VUMA supports multi-ISA code generation across 8 backend architectures:

| Backend | ELF Class | Endianness | Pointer Width | Section Alignment | Calling Convention |
|---------|-----------|------------|---------------|-------------------|-------------------|
| x86_64 | ELF64 | Little | 64-bit | 16 | System V AMD64 (RDI, RSI, RDX, RCX, R8, R9) |
| AArch64 | ELF64 | Little | 64-bit | 16 | AAPCS64 (X0-X7, V0-V7) |
| RISC-V 64 | ELF64 | Little | 64-bit | 4 | RV64G LP64D (A0-A7) |
| ARM32 | ELF32 | Little | 32-bit | 4 | AAPCS (R0-R3) |
| MIPS64 | ELF64 | Big | 64-bit | 8 | N64 ($a0-$a7) |
| PPC64 | ELF64 | Big | 64-bit | 16 | ELFv2 (R3-R10, TOC) |
| LoongArch64 | ELF64 | Little | 64-bit | 8 | LP64 ($a0-$a7) |
| Wasm32 | Wasm | Little | 32-bit | — | Stack machine |

All backends share a unified `Backend` trait and produce either ELF executables (7 native targets) or Wasm modules (Wasm32). The codegen pipeline is:

```
SCG → IR (target-independent) → Register Allocation → Instruction Selection → Binary Emission
```

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

# Multi-architecture code generation
cargo test -p vuma-codegen

# Standard library
cargo test -p vuma-std

# Proof system
cargo test -p vuma-proof

# Parser
cargo test -p vuma-parser
```

### Specific Test Categories

```bash
# Cross-backend consistency (8 backends)
cargo test -p vuma-tests -- cross_backend

# ABI conformance (27 tests)
cargo test -p vuma-tests -- abi_conformance

# ELF validation (7 native backends)
cargo test -p vuma-tests -- elf_validation

# Wasm validation (12 tests)
cargo test -p vuma-tests -- wasm_validation

# Property-based testing
cargo test -p vuma-tests -- property

# Parser roundtrip
cargo test -p vuma-tests -- parser_roundtrip
```

### Verification Tests (Thread-Sensitive)

```bash
cargo test -p vuma-ive -- --test-threads=1
```

### Benchmarks

```bash
make bench
# or: cargo bench --workspace

# Run the benchmark suite via CLI
cargo run -- --bench
```

The benchmark suite covers:
- SHA256d across all 8 backends (timing, binary size, instruction count)
- Compilation speed at varying program sizes
- Backend comparison (binary sizes)
- Codegen quality (redundant load/store analysis)

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
├── CHANGELOG.md             # Changelog (Waves 1-32)
├── RELEASES.md              # Release notes
├── Makefile                 # Build/test targets
├── justfile                 # Just command runner shortcuts
├── rust-toolchain.toml      # Pinned nightly toolchain
├── src/
│   ├── lib.rs               # Workspace crate root
│   ├── pipeline.rs          # Top-level compilation pipeline
│   ├── main.rs              # CLI entry point
│   ├── llm_api.rs           # VumaForLLM API for LLM agents
│   ├── api.rs               # VumaCompiler API
│   ├── diagnostics.rs       # 65 diagnostic codes, error chaining
│   ├── ffi.rs               # FFI, syscalls, relocations
│   ├── lsp/                 # Language Server Protocol
│   ├── scg/                 # Layer 1 — Semantic Computation Graph
│   ├── bd/                  # Layer 5 — Behavioral Descriptors
│   ├── vuma/                # Layer 6 — VUMA Memory Model
│   ├── ive/                 # Layer 2 — Inference & Verification Engine
│   ├── cor/                 # Layer 4 — Continuous Optimization Runtime
│   ├── projection/          # Layer 3 — Projection System
│   ├── parser/              # Parser / Frontend / Module Resolution
│   ├── codegen/             # Multi-ISA Code Generation (8 backends)
│   ├── proof/               # Formal Proof System
│   ├── std/                 # Standard Library
│   ├── package/             # Package Manager
│   └── tests/               # Integration Tests & Benchmarks
├── examples/                # VUMA example programs (*.vuma)
├── docs/                    # Documentation
│   ├── architecture.md      # Full architecture document
│   ├── language-reference.md # VUMA language reference
│   ├── llm-language-reference.md # LLM-oriented language reference
│   ├── ROADMAP.md           # 5-phase project roadmap
│   ├── CONTRIBUTING.md      # Contributor guidelines
│   ├── CONVENTIONS.md       # Coding conventions
│   ├── GLOSSARY.md          # Project glossary (40+ terms)
│   └── specs/               # 15 formal specification documents
├── .cargo/config.toml       # Cargo cross-compilation config
└── .github/workflows/       # CI (test + cross-compile for all 8 targets)
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

The IVE is the reasoning core. It reads the SCG, infers Behavioral Descriptors, constructs the MSG, and verifies the five global invariants. It operates through iterative fixpoint computation, resolving interdependencies between RepD, CapD, and RelD inference. It supports interprocedural analysis, escape analysis, verification caching, and incremental re-verification.

### COR — Continuous Optimization Runtime

The COR maintains an always-compiled invariant: every reachable SCG region is kept in compiled machine code at all times. It performs incremental compilation, profile-guided optimization (using PMU counters), speculative optimization with transparent deoptimization, and adaptive deployment across heterogeneous targets.

---

## Example Programs

| Example                   | Lines | Demonstrates                                      |
|---------------------------|-------|---------------------------------------------------|
| `hello_memory.vuma`       | 40    | Basic allocate/write/read/free                    |
| `doubly_linked_list.vuma` | 89    | Sentinel node pattern, derivation chains          |
| `arena_allocator.vuma`    | 78    | Arena allocation with region semantics            |
| `gpio_blink.vuma`         | 68    | GPIO hardware access                              |
| `lock_free_queue.vuma`    | 99    | Lock-free SPSC queue with atomics                 |
| `channel_demo.vuma`       | 237   | Channel-based concurrency                         |
| `memory_arena.vuma`       | 197   | Region-based allocation                           |
| `aarch64_sensor.vuma`     | 188   | AArch64 MMIO sensor reading                       |
| `sorted_map.vuma`         | 192   | Sorted map with BD-verified operations            |
| `thread_pool.vuma`        | 209   | Thread pool with work stealing                    |

---

## API Examples

### LLM API (VumaForLLM)

```rust
use vuma::VumaForLLM;

// Compile VUMA source code
let result = VumaForLLM::compile(source);
if result.success {
    println!("Compilation succeeded!");
    println!("Explanation: {}", result.explanation);
    if let Some(scg) = &result.scg_json {
        println!("SCG: {}", scg);
    }
    for (target, size) in &result.binary_sizes {
        println!("{}: {} bytes", target, size);
    }
}

// Quick syntax/semantic check
let diagnostics = VumaForLLM::check(source);
for diag in &diagnostics {
    println!("[{}] {}", diag.severity, diag.message);
}

// Analyze SCG structure
let analysis = VumaForLLM::analyze(source)?;

// Compile to Wasm sandbox
let wasm = VumaForLLM::to_wasm(source)?;

// Explain an error in natural language
let explanation = VumaForLLM::explain_error(&diagnostic);

// Get fix suggestions
let fixes = VumaForLLM::suggest_fixes(&diagnostic);

// List available targets
let targets = VumaForLLM::targets();
for t in &targets {
    println!("{} ({}, {}-bit, {})", t.name, t.triple, t.pointer_width, t.endianness);
}
```

### LSP Server

```bash
# Start the LSP server (for IDE integration)
vuma lsp
```

The LSP server provides:
- **Diagnostics**: real-time verification feedback with counterexamples
- **Hover**: BD details, verification status, type information
- **Go-to-definition**: navigate to function/variable definitions
- **Completion**: VUMA keywords, functions, and types
- **Document symbols**: function list with signatures
- **Semantic tokens**: syntax highlighting information

### REPL

```bash
# Start the interactive REPL
vuma repl

# REPL commands:
:wasm            # Compile current session to Wasm32
:backends        # List all 8 available backends
:check           # Run IVE verification on current session
:diagnostics     # Show all diagnostics as JSON
:exports         # List all function signatures
:verify          # Full verification pipeline
:help            # Show available commands
```

---

## Documentation

| Document                    | Description                                           |
|-----------------------------|-------------------------------------------------------|
| [Architecture](docs/architecture.md) | Full architecture document with LLM integration section |
| [Language Reference](docs/language-reference.md) | Complete VUMA syntax and semantics |
| [LLM Language Reference](docs/llm-language-reference.md) | LLM-oriented guide with pitfalls and patterns |
| [Roadmap](docs/ROADMAP.md) | 5-phase development roadmap with milestones           |
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
| `aarch64-memory-model-spec.md`      | 809   | AArch64 memory model                     |
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
- **How to Add Backend Instructions** — 7-step process with encoding verification
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

- **2026-03-05 — Waves 1-5:** Created comprehensive README.md with overview, architecture, quick start, AArch64 build instructions, test instructions, project structure, key concepts, examples, documentation index, and contributing link.
- **2026-03-05 — Waves 6-32:** Expanded README for v0.2.0 release: 8 backends, LLM integration, Wasm sandbox, API examples, REPL commands, LSP, diagnostics, module system, package manager.
