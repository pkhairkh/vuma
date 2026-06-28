# VUMA — Verified-Unsafe Memory Access

**A programming language framework with behavioral verification instead of a borrow checker.**

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Version: 0.1.0-alpha.1](https://img.shields.io/badge/version-0.1.0--alpha.1-orange.svg)](CHANGELOG.md)
[![Rust: nightly](https://img.shields.io/badge/rust-nightly-93450a.svg)](rust-toolchain.toml)

---

## Table of Contents

1. [Overview](#overview)
2. [Features](#features)
3. [10 Backend Architectures](#10-backend-architectures)
4. [Architecture](#architecture)
5. [VUMA for LLMs](#vuma-for-llms)
6. [Installation](#installation)
7. [Quick Start](#quick-start)
8. [Running Tests](#running-tests)
9. [Project Structure](#project-structure)
10. [Key Concepts](#key-concepts)
11. [Example Programs](#example-programs)
12. [API Examples](#api-examples)
13. [Known Limitations](#known-limitations)
14. [Documentation](#documentation)
15. [Contributing](#contributing)
16. [License](#license)

---

## Overview

VUMA is a programming language framework where unsafe memory operations are made verifiable instead of forbidden. Instead of a borrow checker rejecting programs that cannot be statically proven safe, VUMA constructs a formal model of every memory operation and verifies global invariants against that model. Programs that pass verification run without runtime overhead; programs that fail receive counterexamples showing the execution path to the violation.

### Core Ideas

1. **Semantic Computation Graphs (SCGs)** — the primary program representation. The SCG is a directed, acyclic, attributed multigraph where nodes are operations, edges are relationships, and regions delineate scopes.

2. **Behavioral Descriptors (BDs)** replace nominal types. A BD is the triple (RepD, CapD, RelD) — representation, capabilities, and relationships — inferred from program structure.

3. **Verification over restriction** — VUMA verifies that programs are safe and provides diagnostics when they are not, rather than rejecting expression of unsafe patterns.

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

### Multi-Architecture Codegen

VUMA compiles to **10 CPU/platform targets** with a unified `Backend` trait. All 10 backends pass the full 5,738-program gold-standard test suite at 100% pass rate (57,380/57,380 runs).

### AI-Native Design

- **VumaForLLM API** — `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()`
- **LSP Server** — diagnostics, hover, go-to-definition, completion, semantic tokens
- **REPL** — `:wasm`, `:backends`, `:check`, `:diagnostics`, `:exports`
- **Wasm32 Backend** — compile to sandboxed WebAssembly modules
- **66 Diagnostic Codes** — E000–E050, W001–W010, I001–I005 with error chaining

### Memory Safety Verification

- **10 violation types** detected at compile time: UseAfterFree, DoubleFree, MemoryLeak, BoundsCheckFailure, NullDereference, DanglingPointer, UninitializedRead, BufferOverflow, UseAfterScope, InvalidFree
- **Runtime bounds checking** behind `--safe` flag
- **Constant-time crypto**: Branchless `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte`

### Atomics & Concurrency

All 10 backends support atomic operations:

| Backend | Atomic Load | Atomic Store | Atomic CAS | Mechanism |
|---------|-------------|-------------|------------|-----------|
| x86_64 | ✅ | ✅ | ✅ | Aligned MOV + LOCK CMPXCHG |
| AArch64 | ✅ | ✅ | ✅ | LDAXR / STLXR loop |
| RISC-V 64 | ✅ | ✅ | ✅ | LR.D / SC.D loop |
| ARM32 | ✅ | ✅ | ✅ | LDREX / STREX loop |
| MIPS64 | ✅ | ✅ | ✅ | LL / SC loop |
| PPC64 | ✅ | ✅ | ✅ | lwsync + aligned load/store + sync |
| LoongArch64 | ✅ | ✅ | ✅ | LL.W / SC.W loop |
| x86_32 | ✅ | ✅ | ✅ | LOCK CMPXCHG |
| RISC-V 32 | ✅ | ✅ | ✅ | LR.W / SC.W loop |
| Wasm32 | ✅ | ✅ | ✅ | Wasm atomic instructions |

### Floating-Point Conversions

All 10 backends support: `IntToFloat`, `UIntToFloat`, `FloatToInt`, `FloatToUInt`, `FloatToFloat`

### DWARF v4 Debug Info

Per-backend DWARF debug information: `.debug_abbrev`, `.debug_info`, `.debug_line`, `.debug_frame`

### FFI & System Calls

- **19 Linux syscalls** across all 10 architectures (read, write, open, close, exit, mmap, munmap, brk, ioctl, fcntl, getpid, clone, futex, etc.)
- **Architecture-specific relocations**: `Arm32Call`, `Mips26`, `Ppc64Rel24`, `LoongArchB26`, `X86_32Plt32`, etc.
- **`extern "C" { fn ...; }`** FFI blocks

### Standard Library (host-side, not yet compiled to target)

| Module | Key Functions |
|--------|--------------|
| **math** | `sin`, `cos`, `tan`, `sqrt`, `exp`, `ln`, `pow`, `floor`, `ceil`, `is_nan`, `copysign` + f32 variants |
| **fmt** | `format_int`, `format_uint`, `format_float`, `format_hex`, `format_binary`, `pad_left`, `join`, `write_str` |
| **crypto** | SHA-256 constants, constant-time `ct_select`, `ct_eq`, `ct_ne`, `ct_lt`, `ct_gte` |
| **string** | `strlen`, `strcmp`, `memcpy`, `memset` |
| **alloc** | `heap_alloc`, `heap_free`, `heap_realloc` + 5 allocator types |
| **io** | `read_bytes`, `write_bytes`, `read_u32_le`, `write_u32_le` |
| **sync** | `Mutex`, `RwLock`, `Channel`, `Barrier` |
| **collections** | `Vec`, `HashMap`, `DoublyLinkedList`, `RingBuffer`, `VumaString` |

### Module System & Package Manager

- **Multi-file compilation**: `import "crypto.vuma"::{sha256, sha256d};` with circular import detection
- **Package manager**: `vuma pkg init`, `vuma pkg build`, `vuma pkg add` with dependency resolution

---

## 10 Backend Architectures

VUMA supports multi-ISA code generation across 10 backend architectures:

| Backend | ELF Class | Endianness | Pointer Width | Calling Convention |
|---------|-----------|------------|---------------|-------------------|
| **x86_64** | ELF64 | Little | 64-bit | System V AMD64 (RDI, RSI, RDX, RCX, R8, R9) |
| **AArch64** | ELF64 | Little | 64-bit | AAPCS64 (X0-X7) |
| **RISC-V 64** | ELF64 | Little | 64-bit | RV64G LP64D (A0-A7) |
| **ARM32** | ELF32 | Little | 32-bit | AAPCS (R0-R3) |
| **MIPS64** | ELF64 | Little | 64-bit | N64 ($a0-$a7) |
| **PPC64** | ELF64 | Big | 64-bit | ELFv2 (R3-R10, TOC) |
| **LoongArch64** | ELF64 | Little | 64-bit | LP64 ($a0-$a7) |
| **x86_32** | ELF32 | Little | 32-bit | cdecl (stack + EAX/EDX/ECX) |
| **RISC-V 32** | ELF32 | Little | 32-bit | RV32G ILP32 (A0-A7) |
| **Wasm32** | Wasm | Little | 32-bit | Stack machine |

All backends share a unified `Backend` trait and produce either ELF executables (9 native targets) or Wasm modules (Wasm32). The codegen pipeline is:

```
SCG → IR (target-independent) → Register Allocation → Instruction Selection → Binary Emission
```

### Backend Test Coverage

| Test Category | Count | What It Covers |
|---------------|-------|----------------|
| Gold-standard suite | 57,380 | 5,738 programs × 10 backends (100% pass rate) |
| Cross-backend consistency | 9 | 4 IR programs × 10 backends |
| ABI conformance | 27 | Calling conventions for all 10 backends |
| ELF validation | 9 × 4 | ELF32/64, endianness, machine types |
| Wasm validation | 12 | Magic, sections, globals, exports, code bodies |

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                    LLM Integration Layer                             │
│    VumaForLLM API · LSP Server · REPL · Structured Diagnostics     │
├─────────────────────────────────────────────────────────────────────┤
│                    Parser / Frontend                                 │
│    Lexer → Parser → AST → AST-to-SCG Lowering · Module Resolution  │
├─────────────────────────────────────────────────────────────────────┤
│             Reasoning Core                                           │
│    IVE (Inference + Verification) · BD (Descriptors) · MSG (Memory) │
│    Invariants: Liveness · Exclusivity · Interpretation ·             │
│                Origin · Cleanup                                      │
├─────────────────────────────────────────────────────────────────────┤
│                    SCG (Core Representation)                         │
│    Nodes (ops, allocs, effects) · Edges (data flow, deps) · Regions │
├─────────────────────────────────────────────────────────────────────┤
│                    Execution                                         │
│    COR Runtime (always-compiled, PGO, JIT) · Multi-ISA Codegen      │
│    x86_64 · AArch64 · RISC-V 64/32 · ARM32 · MIPS64 · PPC64        │
│    LoongArch64 · x86_32 · Wasm32                                    │
└─────────────────────────────────────────────────────────────────────┘
```

### Pipeline

```
Source Text → Lexer → Parser → AST → SCG Lowering → Raw SCG
    → Module Resolution (imports) → Merged SCG
    → BD Inference (RepD + CapD + RelD fixpoint) → Annotated SCG
    → MSG Builder → VUMA Verification (5 invariants) → Verified SCG
    → Multi-Arch Codegen (IR → regalloc → emit) → Machine Code / Wasm
```

### Workspace Crates

```
                    ┌──────────┐
                    │  tests   │  (depends on everything)
                    └────┬─────┘
           ┌─────────────┼──────────────────┐
           ▼             ▼                  ▼
    ┌────────────┐ ┌──────────┐      ┌──────────┐
    │   cor      │ │   std    │      │  codegen │
    └─────┬──────┘ └────┬─────┘      └─────┬────┘
          │              │                   │
          ▼              ▼                   ▼
    ┌──────────┐  ┌──────────┐  ┌──────────────────┐
    │  (cor)   │  │  vuma    │  │  ive · proof     │
    └────┬─────┘  └────┬─────┘  └────┬─────────────┘
         │             │              │
         └─────────────┼──────────────┘
                       ▼
                ┌──────────────┐
                │  bd · parser │
                └──────┬───────┘
                       ▼
                ┌──────────────┐
                │     scg      │  ◄── foundation (zero workspace deps)
                └──────────────┘
```

The workspace has 11 member crates: `scg`, `bd`, `vuma` (core), `ive`, `cor`, `parser`, `codegen`, `proof`, `std`, `tests`, `package`.

---

## VUMA for LLMs

VUMA is designed for programmatic consumption by AI agents: parse source, analyze structure, run verification, compile to any of 10 backends, and inspect results — all through clean API boundaries.

### LLM-Facing Interfaces

| Interface | Description | Use Case |
|-----------|-------------|----------|
| `VumaForLLM` API | `compile()`, `check()`, `analyze()`, `to_wasm()`, `explain_error()`, `suggest_fixes()` | LLM compiles and verifies code in a sandbox |
| `VumaCompiler` API | `compile()`, `parse()`, `analyze()`, `validate()`, `verify()` | Full pipeline with verification reports |
| LSP Server | Full LSP protocol: diagnostics, hover, go-to-definition, completion | IDE integration and LLM agent interaction |
| REPL `:wasm` | Compile to Wasm and show binary size | Quick Wasm compilation check |
| REPL `:backends` | List 10 backends with status | Discover compilation capabilities |
| REPL `:check` | Run IVE verification | Instant verification feedback |
| REPL `:diagnostics` | Show all diagnostics as JSON | Structured error analysis |
| REPL `:exports` | List all functions and signatures | Program structure inspection |

### Wasm32 Sandbox for LLM Agents

The Wasm32 backend enables LLM agents to compile VUMA programs into sandboxed WebAssembly modules. This is the recommended execution path for LLM-generated code: the Wasm module runs in a sandboxed environment with no access to host memory or peripherals.

```rust
use vuma::VumaForLLM;

let result = VumaForLLM::compile(source);
if let Some(wasm) = &result.wasm_binary {
    println!("Wasm module: {} bytes", wasm.len());
}
```

### Parser Hardening for LLM-Generated Code

- **LLM type aliases**: `int` → `i32`, `float` → `f32`, `double` → `f64`, `String` → `string`
- **C/Rust construct detection**: `println!`, `vec!`, `format!`, `panic!` produce helpful errors
- **C-style for loop detection**: `for (i=0; i<n; i++)` → specific error with VUMA suggestion
- **Reference type conversion**: `&T` / `&mut T` auto-convert to `*T` (pointer type)

---

## Installation

### Building from Source

**Prerequisites:**

- **Rust** — nightly toolchain (pinned in `rust-toolchain.toml`)
- **Make** or **Just** — build orchestration
- **QEMU** (optional) — emulation for testing cross-architecture backends
- **Wasmtime** (optional) — for running Wasm32 backend tests

```bash
# Clone the repository
git clone https://github.com/pkhairkh/vuma.git
cd vuma

# Install the pinned toolchain, components, and targets
make setup

# Build the entire workspace
make build

# Verify the build
cargo run --release -- --version

# Run all tests
make test
```

### Quick Build Check

```bash
make lint && make test
```

---

## Quick Start

### Your First VUMA Program

Create `hello.vuma`:

```vuma
// hello.vuma — Basic memory allocation, write, read, and deallocation
// Expected exit code: 42

fn main() -> i32 {
    // Allocate a region for one byte
    buf = allocate(8);
    // Write 42 to the buffer
    *(buf + 0) = 42;
    // Read it back
    val: u32 = *(buf + 0);
    // Free the buffer
    free(buf);
    // Return the value
    return val;
}
```

Compile and verify:

```bash
# Verify against all 5 VUMA invariants
vuma verify hello.vuma

# Compile to a specific backend (use `vuma emit <ISA> <file>`)
vuma emit x86_64 hello.vuma -o hello.x86_64
vuma emit aarch64 hello.vuma -o hello.aarch64
vuma emit wasm32 hello.vuma -o hello.wasm

# Quick syntax and semantic check
vuma check hello.vuma

# Run with memory safety analysis
vuma run --safe hello.vuma

# Compile with DWARF debug info
vuma emit --debug x86_64 hello.vuma -o hello.debug
```

The verifier checks all five invariants against the SCG derived from your program. If verification passes, you get zero-overhead machine code for any of the 10 backends. If it fails, you get a counterexample showing the path to the violation.

### FFI Example

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

### Importing Modules

```vuma
import "crypto.vuma"::{sha256, sha256d};

fn main() {
    let hash: Address = sha256(data, len);
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
# Cross-backend consistency (10 backends)
cargo test -p vuma-tests -- cross_backend

# ABI conformance (27 tests)
cargo test -p vuma-tests -- abi_conformance

# ELF validation (9 native backends)
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

### Gold-Standard Suite (5,738 programs × 10 backends)

```bash
bash scripts/pi5_test_suite.sh --workers 4 --fresh
```

This runs the full gold-standard test suite across all 10 backends using QEMU and Wasmtime. Current result: **57,380/57,380 = 100.00%**.

### Verification Tests (Thread-Sensitive)

```bash
cargo test -p vuma-ive -- --test-threads=1
```

### Benchmarks

```bash
make bench
# or: cargo bench --workspace
cargo run --release -- --bench
```

### Code Quality

```bash
make fmt-check    # Format check
make clippy       # Lint
make lint         # Full lint (fmt-check + clippy)
```

---

## Project Structure

```
vuma/
├── Cargo.toml              # Workspace root (11 crate members)
├── CHANGELOG.md            # Changelog
├── RELEASES.md             # Release notes
├── Makefile / justfile     # Build/test orchestration
├── rust-toolchain.toml     # Pinned nightly toolchain
├── src/
│   ├── lib.rs / pipeline.rs / main.rs   # Crate root, pipeline, CLI
│   ├── llm_api.rs           # VumaForLLM API for LLM agents
│   ├── api.rs               # VumaCompiler API
│   ├── diagnostics.rs       # 66 diagnostic codes, error chaining
│   ├── ffi.rs               # FFI, 19 syscalls, relocations (10 archs)
│   ├── lsp/                 # Language Server Protocol
│   ├── scg/                 # Semantic Computation Graph
│   ├── bd/                  # Behavioral Descriptors (repd, capd, reld, inference)
│   ├── vuma/                # VUMA Memory Model (msg, invariants)
│   ├── ive/                 # Inference & Verification Engine
│   ├── cor/                 # Continuous Optimization Runtime
│   ├── parser/              # Parser / Frontend / Module Resolution
│   ├── codegen/             # Multi-ISA Code Generation (10 backends)
│   │   ├── backend.rs / ir.rs / scg_to_ir.rs / regalloc.rs / opt.rs
│   │   ├── emit.rs / dwarf.rs / control_flow.rs / memory_safety.rs
│   │   ├── x86_64/          # x86_64 backend (SysV AMD64)
│   │   ├── arm64.rs         # AArch64 backend (AAPCS64)
│   │   ├── riscv64.rs       # RISC-V 64 backend (LP64D)
│   │   ├── riscv32.rs       # RISC-V 32 backend (ILP32)
│   │   ├── arm32/           # ARM32 backend (AAPCS)
│   │   ├── mips64/          # MIPS64 backend (N64)
│   │   ├── ppc64/           # PowerPC64 backend (ELFv2)
│   │   ├── loongarch64/     # LoongArch64 backend (LP64)
│   │   ├── x86_32/          # x86_32 backend (cdecl)
│   │   └── wasm32/          # WebAssembly backend
│   ├── proof/               # Formal Proof System
│   ├── std/                 # Standard Library (18 modules, host-side)
│   ├── package/             # Package Manager (manifest, resolver, registry)
│   └── tests/               # Integration Tests & Benchmarks
├── examples/                # 48 VUMA example programs (*.vuma)
├── tests/gold_standard/     # 5,738 gold-standard test programs
├── docs/                    # Documentation + 15 formal specs
└── .github/workflows/       # CI (test + cross-compile for all 10 targets)
```

---

## Key Concepts

### SCG — Semantic Computation Graph

The SCG is the single source of truth in VUMA. Nodes represent computational operations (allocations, accesses, computations, casts, effects, control flow), edges represent relationships (data flow, control flow, derivation, annotation), and regions delineate scopes, phases, and deployment targets.

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
| `lock_free_queue.vuma`    | 91    | Lock-free SPSC queue with byte-level access       |
| `channel_demo.vuma`       | 237   | Channel-based concurrency                         |
| `memory_arena.vuma`       | 197   | Region-based allocation                           |
| `sorted_map.vuma`         | 192   | Sorted map with BD-verified operations            |
| `thread_pool.vuma`        | 209   | Thread pool with work stealing                    |
| `spinlock.vuma`           | 88    | Spinlock using atomic CAS                         |
| `sha256d.vuma`            | 375   | SHA256d hash computation                          |
| `ffi_demo.vuma`           | 32    | FFI and syscall usage                             |
| `fibonacci.vuma`          | 74    | Fibonacci computation                             |
| `quicksort.vuma`          | 111   | Quicksort implementation                          |
| `crc32.vuma`              | 119   | CRC32 checksum                                    |
| `base64_encode.vuma`      | 177   | Base64 encoding                                   |
| `enum_demo.vuma`          | 114   | Enum (tagged union) types                         |
| `struct_demo.vuma`        | 72    | Struct types with field access                    |
| `syscall_32bit.vuma`      | 35    | FFI write/exit litmus test for 32-bit backends    |

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
vuma repl

# REPL commands:
:wasm            # Compile current session to Wasm32
:backends        # List all 10 available backends
:check           # Run IVE verification on current session
:diagnostics     # Show all diagnostics as JSON
:exports         # List all function signatures
:verify          # Full verification pipeline
:help            # Show available commands
```

---

## Known Limitations

This is an alpha release.

| Area | Status | Details |
|------|--------|---------|
| **Self-hosting** | ❌ Not started | VUMA cannot compile itself yet. The compiler is written in Rust. |
| **Structs / Enums** | ✅ Working | User-defined struct and enum types compile to tagged unions in memory. Struct field access, enum dispatch, and pattern matching work on all 10 backends. |
| **Womb subsystem** | ✅ Working | The `concept`, `gestalt`, `manifold`, and `aura` data models are integrated. SCG nodes, BD inference, and IVE verification support them. |
| **Stdlib is host-side only** | ⚠️ Partial | Math, fmt, string, and crypto functions execute on the host side (Rust). They are not yet compiled to target machine code. |
| **BD inference completeness** | ⚠️ Partial | Some complex BD inference scenarios (M2.3) are deferred to a future release. |
| **Doubly-linked list verification** | ⚠️ Partial | Full verification of doubly-linked list patterns (M2.4) is not yet complete. |
| **Concurrent verification** | ⚠️ Limited | Verification is limited to single-threaded programs. Full concurrent verification is planned. |
| **COR end-to-end** | ⚠️ Partial | The Continuous Optimization Runtime is not yet integrated end-to-end. |
| **Backend tiers** | ✅ All Complete | All 10 backends (aarch64, x86_64, riscv64, riscv32, arm32, mips64, ppc64, loongarch64, x86_32, wasm32) pass 100% of the 5,738 gold-standard tests (57,380/57,380 runs). |
| **Verification gate** | ✅ Strict by default | `--verification normal` (default) halts codegen on invariant violations. Use `--verification none` to bypass. |
| **Error recovery** | ⚠️ Partial | Parser has known type mismatches in AST→SCG lowering for some edge cases. |

---

## Documentation

| Document | Description |
|----------|-------------|
| [Architecture](docs/architecture.md) | Full architecture document with LLM integration section |
| [Language Reference](docs/language-reference.md) | Complete VUMA syntax and semantics |
| [LLM Language Reference](docs/llm-language-reference.md) | LLM-oriented guide with pitfalls and patterns |
| [Roadmap](docs/ROADMAP.md) | Development roadmap with milestones |
| [Contributing](docs/CONTRIBUTING.md) | Build, test, add features, code review process |
| [Conventions](docs/CONVENTIONS.md) | Rust style, error handling, testing, naming, docs |
| [Glossary](docs/GLOSSARY.md) | Defined terms across all domains |
| [Changelog](CHANGELOG.md) | Detailed changelog |
| [Release Notes](RELEASES.md) | Release summaries with known limitations |
| [Formal Specs](docs/specs/) | 15 formal specification documents |

### Formal Specifications

| Specification | Topic |
|---------------|-------|
| `scg-formal-spec.md` | SCG mathematical model |
| `repd-formal-spec.md` | Representation descriptor lattice |
| `capd-formal-spec.md` | Capability descriptor lattice |
| `reld-formal-spec.md` | Relational descriptor kinds |
| `vuma-invariants-spec.md` | Five VUMA invariants |
| `msg-construction-spec.md` | MSG construction algorithm |
| `security-model-spec.md` | Security model and threat categories |
| `bd-inference-algorithm.md` | BD inference fixpoint algorithm |
| `vuma-verification-algorithm.md` | VUMA verification algorithm |
| `arm64-codegen-algorithm.md` | ARM64 codegen algorithm |
| `multi-arch-isa-research.md` | Multi-architecture ISA research |
| `benchmark-design.md` | Benchmark methodology and categories |
| `trivial-proofs.md` | Trivial program proofs |
| `dlist-proof.md` | Doubly-linked list proof |
| `decidability-analysis.md` | Decidability analysis |

---

## Contributing

See [CONTRIBUTING.md](docs/CONTRIBUTING.md) for the complete contributor guide.

Quick start for contributors:

```bash
make setup                     # Install toolchain and targets
make lint && make test         # Full CI check
cargo test -p vuma-scg         # Fast per-crate iteration
```

---

## License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.
