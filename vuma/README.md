# VUMA — Verified-Unsafe Memory Access

**An AI-native programming language framework that replaces traditional type systems with behavioral verification.**

[![CI](https://github.com/vuma-lang/vuma/actions/workflows/ci.yml/badge.svg)](https://github.com/vuma-lang/vuma/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Version: 0.1.0-alpha.1](https://img.shields.io/badge/version-0.1.0--alpha.1-orange.svg)](CHANGELOG.md)
[![Crates.io](https://img.shields.io/badge/crates.io-vuma-blue.svg)](https://crates.io/crates/vuma)
[![Rust: nightly](https://img.shields.io/badge/rust-nightly-93450a.svg)](rust-toolchain.toml)

---

## Table of Contents

1. [What's New in v0.1-alpha](#whats-new-in-v01-alpha)
2. [Overview](#overview)
3. [Features](#features)
4. [8 Backend Architectures](#8-backend-architectures)
5. [Architecture](#architecture)
6. [VUMA for LLMs](#vuma-for-llms)
7. [Installation](#installation)
8. [Quick Start](#quick-start)
9. [Running Tests](#running-tests)
10. [Project Structure](#project-structure)
11. [Key Concepts](#key-concepts)
12. [Example Programs](#example-programs)
13. [API Examples](#api-examples)
14. [Known Limitations](#known-limitations)
15. [Documentation](#documentation)
16. [Contributing](#contributing)
17. [License](#license)

---

## What's New in v0.1-alpha

This is the first public alpha release of the VUMA framework. It ships five waves of engineering work covering the full compilation pipeline from source text to machine code across 8 architectures, with formal verification, LLM integration, and a comprehensive standard library.

### Wave 1 — Foundation & Formal Specifications

- **12 workspace crates**: `scg`, `bd`, `vuma`, `ive`, `cor`, `projection`, `parser`, `codegen`, `proof`, `std`, `tests`, `package`
- **15 formal specifications** (~9,800 lines): SCG model, RepD/CapD/RelD lattices, five VUMA invariants, MSG construction, BD inference algorithm, verification algorithm, ARM64 codegen algorithm, security model, benchmark design, proof systems, decidability analysis
- **10 example programs**: hello_memory, doubly_linked_list, arena_allocator, gpio_blink, lock_free_queue, channel_demo, memory_arena, sorted_map, thread_pool, and more
- **Full verification pipeline**: SCG → MSG → IVE verification with counterexample generation
- **Proof system**: Formal proofs, checker, tactics, and counterexample generation

### Wave 2 — Core Verification & Memory Model

- **SCG → MSG conversion** (1,357 lines): Topological walk producing well-formed Memory State Graphs
- **Incremental MSG** (1,907 lines): Delta computation and application for fast re-verification
- **Invariant aggregator** (1,141 lines): Unified verification pipeline across all five invariants

### Wave 3 — Standard Library & COR Enhancement

- **Std primitives**: RelD, BD triple, `HasBD` trait, `Ptr<T>`, `RegionPtr<T>`, `Slice<T>`, `VumaResult`, `VumaOption`, `Range`
- **COR enhancements**: `PmuCounters`, `SpeculativeExecutor`, `DeploymentManager` with hot-swap FSM
- **5 allocators**: Global, Arena, Pool, Bump, FreeList with VUMA-compatible BD annotations

### Wave 4 — Parser, Collections, & Benchmarks

- **Parser error recovery**: 8 error kinds, 5 recovery strategies, "Did you mean?" suggestions
- **Collections**: `VumaString`, `SipHasher13`, `DoublyLinkedList`, `Vec`, `HashMap`, `RingBuffer`
- **Benchmark suite**: 8 categories with 40+ benchmarks

### Wave 5 — Multi-Arch Codegen & LLM Integration

- **8 backend architectures** all passing SHA256d or individual operation tests
- **73 math functions** (f64 + f32 variants): trig, exp/log, rounding, classification, constants
- **14 formatting functions**: `format_int`, `format_uint`, `format_float`, `format_hex`, `format_binary`, `format_octal`, `format_pointer`, `pad_left`, `pad_right`, `join`, `write_str`, `write_int`, `write_float`
- **DWARF v4 debug info**: Per-backend address size and instruction length, `.debug_abbrev`, `.debug_info`, `.debug_line`, `.debug_frame`
- **FFI & syscalls**: 19 Linux syscalls across 8 architectures, `extern "C"` blocks, architecture-specific relocations
- **Atomics**: `AtomicLoad`, `AtomicStore`, `AtomicCas` across all 8 backends (LL/SC on AArch64/RISC-V, LOCK CMPXCHG on x86_64, acquire/release on PPC64)
- **FP conversions**: `IntToFloat`, `UIntToFloat`, `FloatToInt`, `FloatToUInt`, `FloatToFloat` across all 8 backends
- **Constant-time crypto**: Branchless `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte` across all 8 backends
- **65 diagnostic codes**: E001–E050, W001–W010, I001–I005 with error chaining and structured suggestions
- **LLM API**: `VumaForLLM` with `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()`
- **Package manager**: `vuma pkg init`, `vuma pkg build`, `vuma pkg add` with dependency resolution
- **Module system**: `import "crypto.vuma"::{sha256, sha256d};` with circular import detection

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

### 8-Backend Multi-Architecture Codegen

VUMA compiles to **8 CPU/platform targets** with a unified `Backend` trait architecture. All 6 native backends pass the full SHA256d execution test. Wasm32 generates valid modules. LoongArch64 passes individual operation tests.

### AI-Native Design

VUMA is designed from the ground up for programmatic consumption by AI agents:

- **VumaForLLM API** — stateless `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()`
- **LSP Server** — full protocol: diagnostics, hover, go-to-definition, completion, semantic tokens
- **Enhanced REPL** — `:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`, tab completion, color output
- **Wasm32 Sandbox** — LLM agents compile to safe, sandboxed WebAssembly modules
- **65 Diagnostic Codes** — E001–E050, W001–W010, I001–I005 with error chaining and structured suggestions

### Memory Safety Verification

- **10 violation types** detected at compile time: UseAfterFree, DoubleFree, MemoryLeak, BoundsCheckFailure, NullDereference, DanglingPointer, UninitializedRead, BufferOverflow, UseAfterScope, InvalidFree
- **Runtime bounds checking** behind `--safe` flag
- **Constant-time crypto**: Branchless `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte` across all 8 backends

### Atomics & Concurrency

All 8 backends support atomic operations with architecture-appropriate lowering:

| Backend | Atomic Load | Atomic Store | Atomic CAS | Mechanism |
|---------|-------------|-------------|------------|-----------|
| x86_64 | ✅ | ✅ | ✅ | Aligned MOV + LOCK CMPXCHG |
| AArch64 | ✅ | ✅ | ✅ | LDAXR / STLXR loop |
| RISC-V 64 | ✅ | ✅ | ✅ | LR.D / SC.D loop |
| ARM32 | ✅ | ✅ | ✅ | LDREX / STREX loop |
| MIPS64 | ✅ | ✅ | ✅ | LL / SC loop |
| PPC64 | ✅ | ✅ | ✅ | lwsync + aligned load/store + sync |
| LoongArch64 | ✅ | ✅ | ✅ | LL.W / SC.W loop |
| Wasm32 | ✅ | ✅ | ✅ | Wasm atomic instructions |

### Floating-Point Conversions

All 8 backends support complete FP conversion casts:

| Cast | Description | Example |
|------|-------------|---------|
| `IntToFloat` | Signed integer → float | `SCVTF` (AArch64), `CVTSI2SD` (x86_64) |
| `UIntToFloat` | Unsigned integer → float | `UCVTF` (AArch64), zero-extend + CVT (x86_64) |
| `FloatToInt` | Float → signed integer | `FCVTZS` (AArch64), `CVTTSD2SI` (x86_64) |
| `FloatToUInt` | Float → unsigned integer | `FCVTZU` (AArch64), via saturating path (x86_64) |
| `FloatToFloat` | f32 ↔ f64 conversion | `FCVT` (AArch64), `CVTSD2SS`/`CVTSS2SD` (x86_64) |

### DWARF v4 Debug Info

Per-backend DWARF debug information with four sections:

| Section | Contents |
|---------|----------|
| `.debug_abbrev` | Abbreviation tables (tag + attribute encodings) |
| `.debug_info` | Compilation unit DIEs (subprograms, variables) |
| `.debug_line` | Line-number program (DWARF standard opcodes) |
| `.debug_frame` | Call frame information (CIE + FDE entries) |

### FFI & System Calls

- **19 Linux syscalls** across all 8 architectures (read, write, open, close, exit, mmap, munmap, brk, ioctl, fcntl, getpid, clone, futex, etc.)
- **Architecture-specific relocations**: `Arm32Call`, `Mips26`, `Ppc64Rel24`, `LoongArchB26`, etc.
- **`extern "C" { fn ...; }`** FFI blocks with `is_extern` flag propagation through IR and codegen

### Standard Library

| Module | Key Functions | Count |
|--------|--------------|-------|
| **math** | `sin`, `cos`, `tan`, `sqrt`, `exp`, `ln`, `pow`, `floor`, `ceil`, `is_nan`, `copysign` + f32 variants + 9 constants | 73 |
| **fmt** | `format_int`, `format_uint`, `format_float`, `format_hex`, `format_binary`, `pad_left`, `join`, `write_str` | 14 |
| **crypto** | SHA-256 constants, constant-time `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte` | 5 |
| **string** | `strlen`, `strcmp`, `memcpy`, `memset` | 4 |
| **alloc** | `heap_alloc`, `heap_free`, `heap_realloc` + 5 allocator types | 8 |
| **io** | `read_bytes`, `write_bytes`, `read_u32_le`, `write_u32_le` | 4 |
| **sync** | `Mutex`, `RwLock`, `Channel`, `Barrier` | 4 |
| **collections** | `Vec`, `HashMap`, `DoublyLinkedList`, `RingBuffer`, `VumaString` | 5 |

### Module System & Package Manager

- **Multi-file compilation**: `import "crypto.vuma"::{sha256, sha256d};` with circular import detection
- **Package manager**: `vuma pkg init`, `vuma pkg build`, `vuma pkg add` with dependency resolution

---

## 8 Backend Architectures

VUMA supports multi-ISA code generation across 8 backend architectures — a unified compilation pipeline producing native machine code or WebAssembly from a single source program:

| Backend | ELF Class | Endianness | Pointer Width | Calling Convention | Atomics | FP Casts | DWARF |
|---------|-----------|------------|---------------|-------------------|---------|----------|-------|
| **x86_64** | ELF64 | Little | 64-bit | System V AMD64 (RDI, RSI, RDX, RCX, R8, R9) | ✅ | ✅ | ✅ |
| **AArch64** | ELF64 | Little | 64-bit | AAPCS64 (X0-X7, V0-V7) | ✅ | ✅ | ✅ |
| **RISC-V 64** | ELF64 | Little | 64-bit | RV64G LP64D (A0-A7) | ✅ | ✅ | ✅ |
| **ARM32** | ELF32 | Little | 32-bit | AAPCS (R0-R3) | ✅ | ✅ | ✅ |
| **MIPS64** | ELF64 | Big | 64-bit | N64 ($a0-$a7) | ✅ | ✅ | ✅ |
| **PPC64** | ELF64 | Big | 64-bit | ELFv2 (R3-R10, TOC) | ✅ | ✅ | ✅ |
| **LoongArch64** | ELF64 | Little | 64-bit | LP64 ($a0-$a7) | ✅ | ✅ | ✅ |
| **Wasm32** | Wasm | Little | 32-bit | Stack machine | ✅ | ✅ | ✅ |

All backends share a unified `Backend` trait and produce either ELF executables (7 native targets) or Wasm modules (Wasm32). The codegen pipeline is:

```
SCG → IR (target-independent) → Register Allocation → Instruction Selection → Binary Emission
```

### Backend Test Coverage

| Test Category | Count | What It Covers |
|---------------|-------|----------------|
| Cross-backend consistency | 9 | 4 IR programs × 8 backends |
| ABI conformance | 27 | Calling conventions for all 8 backends |
| ELF validation | 7 × 4 | ELF32/64, endianness, machine types |
| Wasm validation | 12 | Magic, sections, globals, exports, code bodies |
| SHA256d execution | 6 | Full SHA256d on all 6 stable native backends |

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

## Installation

### Via Cargo

```bash
cargo install vuma
```

### Building from Source

**Prerequisites:**

- **Rust** — nightly toolchain (pinned in `rust-toolchain.toml`)
- **Make** or **Just** — build orchestration
- **QEMU** (optional) — emulation for testing cross-architecture backends

```bash
# Clone the repository
git clone https://github.com/vuma-lang/vuma.git
cd vuma

# Install the pinned toolchain, components, and targets
make setup

# Build the entire workspace
make build

# Verify the build
cargo run -- --version

# Run all tests
make test
```

### Quick Build Check

```bash
# Build + lint + test in one command
make lint && make test
```

---

## Quick Start

### Your First VUMA Program

Create `hello.vuma`:

```vuma
// hello.vuma — Basic memory allocation, write, read, and deallocation

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
vuma verify hello.vuma

# Compile to a specific backend
vuma build --target x86_64 hello.vuma
vuma build --target aarch64 hello.vuma
vuma build --target wasm32 hello.vuma

# Quick syntax and semantic check
vuma check hello.vuma

# Run with memory safety analysis
vuma run --safe hello.vuma

# Compile with DWARF debug info
vuma build --debug-info hello.vuma
```

The verifier will check all five invariants against the SCG derived from your program. If verification passes, you get zero-overhead machine code for any of the 8 backends. If it fails, you get a precise counterexample showing the exact path to the violation.

### FFI Example

Create `ffi_demo.vuma`:

```vuma
// ffi_demo.vuma — Call into C library functions via FFI

extern "C" {
    fn write(fd: i64, buf: Address, count: i64) -> i64;
    fn exit(code: i64);
}

fn main() {
    write(1, 0x400000, 13);
    exit(0);
}
```

### Using the Math Library

```vuma
// math_demo.vuma — Use standard library math functions

fn compute() -> f64 {
    let angle: f64 = 1.57079632679;  // π/2
    let result: f64 = sin(angle);     // ≈ 1.0
    return result;
}
```

### Importing Modules

```vuma
// main.vuma — Import from another VUMA file

import "crypto.vuma"::{sha256, sha256d};

fn main() {
    let hash: ptr<u8> = sha256(data, len);
}
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
cargo test -p vuma-scg          # SCG core
cargo test -p vuma-ive          # Inference and verification engine
cargo test -p vuma-bd           # Behavioral descriptors
cargo test -p vuma-core         # VUMA memory model
cargo test -p vuma-codegen      # Multi-architecture code generation
cargo test -p vuma-std          # Standard library
cargo test -p vuma-proof        # Proof system
cargo test -p vuma-parser       # Parser
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

# DWARF + FFI integration
cargo test -p vuma-tests -- dwarf_ffi

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
make fmt-check    # Format check
make clippy       # Lint
make lint         # Full lint
make lint && make test  # Full CI check
```

---

## Project Structure

```
vuma/
├── Cargo.toml              # Workspace root (12 crate members)
├── CHANGELOG.md             # Changelog (Waves 1–32)
├── RELEASES.md              # Release notes
├── Makefile / justfile      # Build/test orchestration
├── rust-toolchain.toml      # Pinned nightly toolchain
├── src/
│   ├── lib.rs / pipeline.rs / main.rs   # Crate root, pipeline, CLI
│   ├── llm_api.rs           # VumaForLLM API for LLM agents
│   ├── api.rs               # VumaCompiler API
│   ├── diagnostics.rs       # 65 diagnostic codes, error chaining
│   ├── ffi.rs               # FFI, 19 syscalls, relocations
│   ├── lsp/                 # Language Server Protocol
│   ├── scg/                 # Layer 1 — Semantic Computation Graph
│   │   └── node, edge, region, graph, callgraph, liveness, dominance, loop_detection
│   ├── bd/                  # Layer 5 — Behavioral Descriptors
│   │   └── repd, capd, reld, inference, context_solver
│   ├── vuma/                # Layer 6 — VUMA Memory Model
│   │   └── msg, msg_builder, msg_incremental, invariant_*
│   ├── ive/                 # Layer 2 — Inference & Verification Engine
│   │   └── verification, inference, escape, interprocedural, cache
│   ├── cor/                 # Layer 4 — Continuous Optimization Runtime
│   │   └── runtime, speculative, deployment, optimization
│   ├── projection/          # Layer 3 — Projection System
│   │   └── textual, visual, conversational, bidirectional, diff
│   ├── parser/              # Parser / Frontend / Module Resolution
│   │   └── lexer, parser, ast, resolver, to_scg
│   ├── codegen/             # Multi-ISA Code Generation (8 backends)
│   │   ├── backend.rs / ir.rs / scg_to_ir.rs / regalloc.rs / opt.rs
│   │   ├── emit.rs / dwarf.rs / control_flow.rs / memory_safety.rs
│   │   ├── x86_64/          # x86_64 backend (SysV AMD64)
│   │   ├── arm64.rs         # AArch64 backend (AAPCS64)
│   │   ├── riscv64.rs       # RISC-V 64 backend (LP64D)
│   │   ├── arm32/           # ARM32 backend (AAPCS)
│   │   ├── mips64/          # MIPS64 backend (N64)
│   │   ├── ppc64/           # PowerPC64 backend (ELFv2)
│   │   ├── loongarch64/     # LoongArch64 backend (LP64)
│   │   └── wasm32/          # WebAssembly backend
│   ├── proof/               # Formal Proof System
│   │   └── proof, checker, counterexample, tactics, composition
│   ├── std/                 # Standard Library (18 modules)
│   │   └── primitives, alloc, collections, math (73 fns), fmt (14 fns),
│   │       crypto, string, io, sync, thread, env, fs, net, path,
│   │       process, time, error
│   ├── package/             # Package Manager (manifest, resolver, registry)
│   └── tests/               # Integration Tests & Benchmarks
│       └── cross_backend, abi_conformance, elf_validation, wasm_validation,
│           dwarf_ffi_integration, full_pipeline, sha256d_backends,
│           property_tests, benchmarks
├── examples/                # 20+ VUMA example programs (*.vuma)
├── docs/                    # Documentation + 15 formal specs
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
| `sorted_map.vuma`         | 192   | Sorted map with BD-verified operations            |
| `thread_pool.vuma`        | 209   | Thread pool with work stealing                    |
| `spinlock.vuma`           | 45    | Spinlock using atomic CAS                         |
| `sha256d.vuma`            | 150+  | SHA256d hash computation                          |
| `ffi_demo.vuma`           | 30    | FFI and syscall usage                             |
| `fibonacci.vuma`          | 25    | Fibonacci computation                             |
| `quicksort.vuma`          | 60    | Quicksort implementation                          |
| `crc32.vuma`              | 50    | CRC32 checksum                                    |
| `base64_encode.vuma`      | 55    | Base64 encoding                                   |

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

## Known Limitations

This is an alpha release. We are transparent about what is not yet complete:

| Area | Status | Details |
|------|--------|---------|
| **Self-hosting** | ❌ Not started | VUMA cannot compile itself yet. The compiler is written in Rust. |
| **Structs / Enums** | ❌ Not started | No user-defined struct or enum types. Programs operate on primitives, pointers, and regions. |
| **Stdlib is host-side only** | ⚠️ Partial | Math, fmt, string, and crypto functions execute on the host side (Rust). They are not yet compiled to target machine code. |
| **BD inference completeness** | ⚠️ Partial | Some complex BD inference scenarios (M2.3) are deferred to a future release. |
| **Doubly-linked list verification** | ⚠️ Partial | Full verification of doubly-linked list patterns (M2.4) is not yet complete. |
| **Concurrent verification** | ⚠️ Limited | Verification is limited to single-threaded programs. Full concurrent verification is planned. |
| **COR end-to-end** | ⚠️ Partial | The Continuous Optimization Runtime is not yet integrated end-to-end. |
| **LoongArch64 performance** | ⚠️ Slow | Full SHA256d on QEMU is too slow; should work on native hardware. |
| **Error recovery** | ⚠️ Partial | Parser has known type mismatches in AST→SCG lowering for some edge cases. |

We believe in honest roadmapping. These limitations represent active development areas, not permanent constraints.

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
| [Changelog](CHANGELOG.md) | Detailed changelog (Waves 1–32)                      |
| [Release Notes](RELEASES.md) | Release summaries with known limitations          |
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
make setup                     # Install toolchain and targets
make lint && make test         # Full CI check
cargo test -p vuma-scg         # Fast per-crate iteration
```

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.

---

## Worklog

- **2026-03-05 — Waves 1–5:** Created comprehensive README.md with overview, architecture, quick start, AArch64 build instructions, test instructions, project structure, key concepts, examples, documentation index, and contributing link.
- **2026-03-05 — Waves 6–32:** Expanded README for v0.2.0 release: 8 backends, LLM integration, Wasm sandbox, API examples, REPL commands, LSP, diagnostics, module system, package manager.
- **2026-03-05 — Task 6-b:** Polished README for public v0.1-alpha release: added "What's New in v0.1-alpha" section, updated features list with DWARF/FFI/atomics/FP casts/92 math functions/fmt module, added comprehensive backend status table with atomics/FP/DWARF columns, added installation instructions (cargo install + from source), added working quick-start examples (hello/FFI/math/modules), added badges, added "Known Limitations" section, updated project structure with all 8 backend modules and std module details.
- **2026-03-05 — Task 6-a:** Comprehensive documentation consistency pass: updated language-reference.md (added FFI/extern section, expanded stdlib with fmt/math/crypto/string/additional modules, added multi-backend table, version bump to 0.2.0), updated architecture.md (version 0.2.0, status Phase 3, expanded codegen layout with all 8 backends + DWARF + backend trait + memory_safety, expanded std layout with all 16 modules), updated ROADMAP.md (version 0.2.0, status Phase 3, wave 19-20 DWARF v4 clarity, phase 2 status note), updated CONTRIBUTING.md (new Section 3: Test Infrastructure with 11 test categories, renamed "ARM64 Instructions" to "Backend Instructions" covering all 8 architectures, added FFI review rule, renumbered sections), updated llm-language-reference.md (expanded stdlib intro to mention all modules, added crypto module docs, string module docs, additional modules list), verified llm-system-prompt.md already covers fmt/math/FFI/atomics/DWARF.
