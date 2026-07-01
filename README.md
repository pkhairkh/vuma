# VUMA — Verified-Unsafe Memory Access

**A programming language framework with behavioral verification instead of a borrow checker.**

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Version: 0.2.0-alpha.1](https://img.shields.io/badge/version-0.2.0--alpha.1-orange.svg)](CHANGELOG.md)
[![Rust: nightly](https://img.shields.io/badge/rust-nightly-93450a.svg)](rust-toolchain.toml)

---

## Overview

VUMA is a programming language framework where unsafe memory operations are made verifiable instead of forbidden. Instead of a borrow checker rejecting programs that cannot be statically proven safe, VUMA constructs a formal model of every memory operation and verifies global invariants against that model. Programs that pass verification run without runtime overhead; programs that fail receive counterexamples showing the execution path to the violation.

### Core Ideas

1. **Semantic Computation Graphs (SCGs)** — the primary program representation. The SCG is a directed, attributed multigraph where nodes are operations, edges are relationships, and regions delineate scopes.

2. **Behavioral Descriptors (BDs)** replace nominal types. A BD is the triple (RepD, CapD, RelD) — representation, capabilities, and relationships — inferred from program structure.

3. **Verification over restriction** — VUMA verifies that programs are safe and provides diagnostics when they are not, rather than rejecting expression of unsafe patterns.

### The Five VUMA Invariants

Every VUMA program can be verified against five global memory-safety invariants:

| Invariant       | Ensures                                            |
|-----------------|----------------------------------------------------|
| **Liveness**    | Every access targets allocated memory              |
| **Exclusivity** | No conflicting concurrent accesses                 |
| **Interpretation** | Every access uses a valid representation         |
| **Origin**      | Every address traces to a valid allocation         |
| **Cleanup**     | Every region is eventually freed or explicitly leaked |

**Note:** The verification engine currently has false positives on programs that use `allocate()`/`free()` with pointer dereference. Most programs are compiled with `--verification none` to bypass this. The `--verification normal` flag runs the IVE but may reject valid programs.

---

## Features

### Multi-Architecture Codegen

VUMA compiles to **10 CPU/platform targets** with a unified `Backend` trait. All 10 backends pass the full gold-standard test suite (5,738 programs × 10 backends = 57,380 runs) at 100% pass rate when compiled with `--verification none`.

### Language Features

- **Functions**: `fn`, parameters, return types, recursion
- **Control flow**: `if`/`else`, `while`, `for` ranges, `break`, `continue`
- **Match expressions**: pattern matching with block arms
- **Structs**: user-defined struct types with field access
- **Enums**: tagged unions with variant payloads
- **Imports**: `import "module.vuma"::{func1, func2};`
- **Extern FFI**: `extern "C" { fn write(fd: i64, buf: Address, count: i64) -> i64; }`
- **Pointer arithmetic**: `*(buf + offset)`, `*ptr`
- **Type annotations**: `let x: u32 = 42;`
- **`unsafe` blocks**: explicitly mark unverifiable code (keyword exists in parser)

### Atomics & Concurrency

All 10 backends support atomic operations (`AtomicLoad`, `AtomicStore`, `AtomicCas`).

### FFI & System Calls

- **19+ Linux syscalls** across all 10 architectures (read, write, open, close, exit, mmap, munmap, etc.)
- **`extern "C" { fn ...; }`** FFI blocks
- **`__vuma_alloc`** (mmap wrapper) and **`__vuma_free`** (munmap wrapper) provide heap allocation that persists across function calls

### Standard Library (Womb)

The `womb/` directory contains **115 `.vuma` files** (~65K lines) of VUMA-native library code. All 115 files compile on x86_64. Key modules:

| Category | Modules |
|----------|---------|
| **Collections** | `vec.vuma` (heap-backed DynamicVec), `hashmap.vuma`, `btree_map.vuma`, `enum_map.vuma` |
| **Strings** | `string.vuma`, `utf8.vuma` (dynamic VStr with grow), `string_builder.vuma` |
| **File I/O** | `file.vuma` (raw syscalls), `high_level.vuma` (read_file, write_file, path ops) |
| **Alloc** | `arena.vuma` (bump allocator on mmap) |
| **Graph** | `digraph.vuma` (heap-backed, dynamic grow), `algorithms.vuma` (toposort, cycle detection) |
| **Lang** | `full_lexer.vuma`, `full_parser.vuma`, `ir_builder.vuma`, `codegen.vuma`, `elf.vuma` |
| **Crypto** | SHA-256, AES-128/192/256, HMAC, ChaCha20, Poly1305, RSA, ECDSA, Ed25519, and more |
| **Encoding** | Base64, hex, URL encoding |
| **Network** | TCP/UDP sockets, DNS, HTTP, WebSocket |

**Note:** The `vuma-std` Rust crate (24K lines) wraps Rust's `std` but is **not linked** to VUMA programs. The Womb is the intended replacement, written in VUMA itself.

### Other Tools

- **LSP Server** — diagnostics, hover, go-to-definition, completion (`vuma lsp`)
- **REPL** — parse and display AST (`vuma repl`)
- **Package manager** — `vuma pkg init/build/add` (basic functionality)
- **DWARF v4 debug info** — `.debug_abbrev`, `.debug_info`, `.debug_line`, `.debug_frame`

---

## 10 Backend Architectures

| Backend | ELF Class | Endianness | Pointer Width |
|---------|-----------|------------|---------------|
| **x86_64** | ELF64 | Little | 64-bit |
| **AArch64** | ELF64 | Little | 64-bit |
| **RISC-V 64** | ELF64 | Little | 64-bit |
| **ARM32** | ELF32 | Little | 32-bit |
| **MIPS64** | ELF64 | Little | 64-bit |
| **PPC64** | ELF64 | Big | 64-bit |
| **LoongArch64** | ELF64 | Little | 64-bit |
| **x86_32** | ELF32 | Little | 32-bit |
| **RISC-V 32** | ELF32 | Little | 32-bit |
| **Wasm32** | Wasm | Little | 32-bit |

The codegen pipeline: `SCG → IR (target-independent) → Register Allocation → Instruction Selection → Binary Emission`

---

## Installation

```bash
git clone https://github.com/pkhairkh/vuma.git
cd vuma
make setup    # Install pinned nightly toolchain
make build    # Build the workspace
```

---

## Quick Start

Create `hello.vuma`:

```vuma
fn main() -> i32 {
    buf = allocate(8);
    *(buf + 0) = 42;
    val: u32 = *(buf + 0);
    free(buf);
    return val;
}
```

Compile (bypasses verification — most programs use this):

```bash
vuma emit x86_64 hello.vuma -o hello.bin
chmod +x hello.bin
./hello.bin   # exits with code 42
```

Verify (may produce false positives on some programs):

```bash
vuma verify hello.vuma
```

---

## Running Tests

### Gold-Standard Suite (5,738 programs × 10 backends)

```bash
bash scripts/pi5_test_suite.sh --workers 4 --fresh
```

### Per-Crate Tests

```bash
cargo test -p vuma-parser    # 286 tests
cargo test -p vuma-core      # 301 tests
cargo test -p vuma-scg       # 36 tests
```

---

## Project Structure

```
vuma/
├── Cargo.toml              # Workspace root (11 crate members)
├── src/
│   ├── lib.rs / pipeline.rs / main.rs   # Crate root, pipeline, CLI
│   ├── llm_api.rs           # VumaForLLM API
│   ├── api.rs               # VumaCompiler API
│   ├── diagnostics.rs       # 66 diagnostic codes
│   ├── ffi.rs               # FFI, syscalls, relocations
│   ├── lsp/                 # Language Server Protocol
│   ├── scg/                 # Semantic Computation Graph
│   ├── bd/                  # Behavioral Descriptors
│   ├── vuma/                # VUMA Memory Model (MSG, invariants)
│   ├── ive/                 # Inference & Verification Engine
│   ├── cor/                 # Continuous Optimization Runtime
│   ├── parser/              # Parser / Frontend / Module Resolution
│   ├── codegen/             # Multi-ISA Code Generation (10 backends)
│   ├── proof/               # Formal Proof System
│   ├── std/                 # Standard Library (Rust, not linked to VUMA)
│   ├── package/             # Package Manager
│   └── tests/               # Integration Tests & Benchmarks
├── examples/                # 48 VUMA example programs
├── tests/gold_standard/     # 5,738 gold-standard test programs
├── womb/                    # 115 VUMA-native library modules (~65K lines)
└── docs/                    # Documentation + formal specs
```

---

## Known Limitations

This is an alpha release. Key limitations:

| Area | Status | Details |
|------|--------|---------|
| **Self-hosting** | ❌ Not started | VUMA cannot compile itself. The compiler is written in Rust. Individual womb modules (lexer, parser, IR, codegen, ELF) exist but haven't been tested as a complete end-to-end pipeline. |
| **Verification** | ⚠️ False positives | `--verification normal` rejects some valid programs (especially those using `allocate()`/`free()` with dereference). Most compilation uses `--verification none`. |
| **Standard library** | ⚠️ Partial | The `vuma-std` Rust crate is not linked. The Womb (115 .vuma files) provides library code but is not integrated into the compilation pipeline. |
| **BD inference** | ⚠️ Partial | Some complex BD inference scenarios (M2.3) are deferred. |
| **Doubly-linked list verification** | ⚠️ Partial | Full verification of doubly-linked list patterns (M2.4) is not complete. |
| **Concurrent verification** | ⚠️ Limited | Verification is limited to single-threaded programs. |
| **COR runtime** | ⚠️ Partial | The Continuous Optimization Runtime is not fully integrated end-to-end. |
| **Type checking** | ❌ Not implemented | The parser recognizes syntax but does not perform semantic type checking or borrow checking. |
| **`map_device()`** | ❌ Not a language feature | Referenced in example comments but not implemented as a language keyword. |
| **`volatile`** | ❌ Not a language feature | No volatile keyword or semantics. |
| **Backends** | ✅ All pass | All 10 backends pass 100% of the 5,738 gold-standard tests (57,380/57,380 runs) with `--verification none`. |

---

## Documentation

| Document | Description |
|----------|-------------|
| [Architecture](docs/architecture.md) | System architecture overview |
| [Language Reference](docs/language-reference.md) | VUMA syntax and semantics |
| [Roadmap](docs/ROADMAP.md) | Development roadmap |
| [Contributing](docs/CONTRIBUTING.md) | Contributor guide |
| [Glossary](docs/GLOSSARY.md) | Defined terms |
| [Formal Specs](docs/specs/) | 15 formal specification documents |

---

## License

MIT License. See [LICENSE](LICENSE) for details.
