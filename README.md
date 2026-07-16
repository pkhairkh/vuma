# VUMA

**Verified-Unsafe Memory Access** — a systems programming language with built-in memory-safety verification, multi-architecture code generation, and a self-hosting bootstrap compiler.

VUMA is designed for environments where memory safety is non-negotiable but runtime checks are too expensive. The compiler performs static verification of five memory invariants (liveness, exclusivity, cleanup, origin, interpretation) at compile time, emitting bare-metal code with zero runtime overhead when verification succeeds.

## Key Features

- **Zero external dependencies** — the entire workspace (10 crates) uses only Rust's `std`. No `serde`, no `libc`, no `clap`, no `rayon`. Every external crate was replaced with hand-written implementations.
- **19-architecture code generation** — x86_64, AArch64, RISC-V 64/32, ARM32, x86_32, LoongArch64, MIPS64, PPC64, s390x, SPARC64, Alpha, PA-RISC, m68k, wasm32, plus 4 endianness wrappers. Each backend emits real machine code.
- **Compile-time memory safety** — five invariants verified statically: use-after-free, double-free, uninitialized reads, aliasing conflicts, and representation mismatches. Hard errors by default; `--no-memory-safety` escape hatch available.
- **VUMA-native standard library** — `womb/` is a 108-file standard library written entirely in VUMA. Syscalls invoke the kernel directly via the `syscall()` intrinsic. No C or Rust wrappers.
- **Formal proof system** — generates and checks formal proofs for each invariant violation (or non-violation). Supports counterexample generation, circular-reasoning detection, and 6 verification levels from quick to hardened.
- **Self-hosting bootstrap** — a 5-file VUMA compiler (`womb/lang/`) that can lex, parse, lower to IR, and emit x86_64 ELF executables. The bootstrap supports `syscall()`, `allocate()`/`free()`, `if`/`else`, `while`, byte-level memory access, and `print_int`.

## Quick Start

```bash
# Build the compiler (requires Rust nightly)
cargo build --release

# Compile and run a VUMA program
./target/release/vuma build hello.vuma -o hello
./hello

# Verify without compiling
./target/release/vuma check hello.vuma

# Compile for a specific architecture
./target/release/vuma emit aarch64 hello.vuma -o hello.aarch64
qemu-aarch64 hello.aarch64

# Link multiple modules
./target/release/vuma link module1.vuma module2.vuma -o combined
```

## Language Overview

```vuma
fn main() -> i32 {
    // Heap allocation via mmap syscall
    buf: Address = allocate(16);

    // Byte-level memory access
    *(buf + 0) = 72;   // 'H'
    *(buf + 1) = 105;  // 'i'
    *(buf + 2) = 10;   // '\n'

    // Direct syscall (VUMA-generic = Linux asm-generic numbering)
    // write(stdout=1, buf, count=3) — translated to native per-arch
    syscall(64, 1, buf, 3);

    free(buf);
    return 0;
}
```

### Syscall Intrinsic

The `syscall(nr, args...)` intrinsic uses VUMA-generic numbering (Linux `asm-generic/unistd.h`). The compiler translates to native per-arch automatically:

| VUMA-generic | Name | x86_64 native | AArch64 native |
|---|---|---|---|
| 64 | write | 1 | 64 (identity) |
| 63 | read | 0 | 63 (identity) |
| 56 | openat | 257 | 56 (identity) |
| 57 | close | 3 | 57 (identity) |
| 222 | mmap | 9 | 222 (identity) |
| 172 | getpid | 39 | 172 (identity) |

Identity arches (native == generic): AArch64, RISC-V 64/32, LoongArch64, ARM32.
Translated arches: x86_64, x86_32, MIPS64, PPC64, s390x, SPARC64, Alpha, PA-RISC, m68k.

See `womb/syscalls.vuma` for the full reference table (111 syscalls across 15 categories).

### Module System

```vuma
// Import symbols from another module
import "../crypto/hqc.vuma" { sha256_oneshot };
import "../lib/socket.vuma" { tcp_connect, tcp_send_str };
```

Cross-module references resolve at link time via `vuma link` or `compile_modules`.

## Project Structure

```
vuma/
├── Cargo.toml                  # Workspace root (zero external deps)
├── src/
│   ├── main.rs                 # CLI entry point (build, run, check, emit, link, ...)
│   ├── pipeline.rs             # Full compilation pipeline (parse → SCG → IVE → codegen → ELF)
│   ├── api.rs                  # Public compiler API (VumaCompiler, CompileConfig)
│   ├── scg/                    # Semantic Computation Graph (formal graph IR)
│   ├── ive/                    # Invariant Verification Engine (6 verification levels)
│   ├── bd/                     # Behavioral Descriptors (RepD, CapD, RelD)
│   ├── vuma/                   # Core: MSG, memory model, security analysis
│   ├── codegen/                # 19-architecture codegen + optimizer + regalloc
│   │   ├── ir.rs              # IR instruction set (27 variants incl. Syscall, VectorOp)
│   │   ├── syscall_abi.rs     # Per-arch syscall number translation (949 mappings)
│   │   ├── opt.rs             # DCE, CSE, constant folding, inlining, LICM, e-graph
│   │   ├── scheduler.rs       # Instruction scheduler (alias analysis + pressure heuristic)
│   │   ├── escape_analysis.rs # Escape analysis + SROA + alloc elision
│   │   ├── egraph.rs          # Equality saturation (35 algebraic rules)
│   │   ├── regalloc.rs        # Register allocation (coalescing, spill weight)
│   │   └── x86_64/            # x86_64 backend (mod, disasm, stack_slot_isel)
│   ├── parser/                 # Lexer, recursive-descent parser, AST, AST→SCG bridge
│   ├── cor/                    # Continuous Optimization Runtime (JIT, profiling)
│   ├── proof/                  # Formal proof system (tactics, checker, counterexamples)
│   ├── package/                # Package manager (TOML manifest, dependency resolver)
│   └── tests/                  # Integration test suite
├── womb/                       # VUMA-native standard library (108 .vuma files)
│   ├── lang/                   # 5-file self-hosting bootstrap compiler
│   ├── lib/                    # stdio, printf, socket, http, dns, json, deflate, ...
│   ├── crypto/                 # 45 files: AES, ChaCha20, SHA, HMAC, RSA, ECDSA, Ed25519, ML-KEM, ...
│   ├── net/                    # TCP, TLS 1.2/1.3, QUIC, SSH
│   ├── collections/            # Vec, HashMap, BTreeMap, EnumMap
│   ├── syscalls.vuma           # VUMA-generic syscall number reference (111 entries)
│   └── ...
├── examples/                   # Example .vuma programs
├── scripts/                    # Test harnesses
└── tests/                      # Gold-standard test suite (structs, multi_function, ...)
```

## Workspace Crates

| Crate | Purpose |
|---|---|
| `vuma-scg` | Semantic Computation Graph — formal graph IR with nodes, edges, regions |
| `vuma-ive` | Invariant Verification Engine — 6 levels, 5 invariants, interprocedural analysis |
| `vuma-bd` | Behavioral Descriptors — RepD (layout), CapD (capabilities lattice), RelD (relations) |
| `vuma-core` | Core — Memory State Graph, memory model, security analysis |
| `vuma-codegen` | 19-architecture codegen, optimizer, register allocator, instruction scheduler |
| `vuma-parser` | Lexer, recursive-descent parser, typed AST, AST→SCG bridge |
| `vuma-cor` | Continuous Optimization Runtime — JIT, profiling, speculative optimization |
| `vuma-proof` | Formal proof system — tactics, checker, counterexample generation |
| `vuma-package` | Package manager — TOML manifest parser, dependency resolver, local registry |
| `vuma-tests` | Integration test suite — cross-backend, property-based, ABI conformance |

## Verification Levels

| Level | Checks | Use case |
|---|---|---|
| `quick` | Exclusivity, origin | Fast iteration during development |
| `normal` (default) | All 5 core invariants | Production builds |
| `exhaustive` | All 5 + formal proof generation + interprocedural | Release verification |
| `modular` | All 5 + per-function modular verification | Large codebases |
| `constant-time` | All 5 + constant-time (taint) analysis | Cryptographic code |
| `hardened` | All 6 + interprocedural + modular | Maximum assurance |

## Optimization

| Level | Passes | Notes |
|---|---|---|
| O0 | None | Fastest compilation |
| O1 | Constant fold, CSE, e-graph, DCE, DSE | Basic optimizations |
| O2 (default) | O1 + inliner, LICM, scheduler, cross-function const prop, escape analysis, SROA, alloc elision, identical-function merge | Production |
| O3 | O2 + aggressive inline threshold | Maximum performance |

The instruction scheduler models memory dependencies via cast-aware type-based alias analysis (TBAA) with IVE-proven non-aliasing overrides. `VUMA_NO_SCHED=1` disables the scheduler (debugging escape hatch).

## Building

```bash
# Requires Rust nightly (pinned: nightly-2026-03-01)
rustup toolchain install nightly-2026-03-01
rustup target add aarch64-unknown-linux-gnu

# Debug build
cargo build

# Release build (LTO, codegen-units=1)
cargo build --release

# Fast release (no LTO, for iterative development)
cargo build --profile release-fast
```

## Testing

```bash
# Run all tests
cargo test --workspace

# Run specific test categories
cargo test -p vuma-codegen --lib emit          # Codegen emission (104 tests)
cargo test -p vuma-codegen --lib syscall_abi    # Syscall translation (15 tests)
cargo test -p vuma-codegen --lib escape         # Escape analysis (12 tests)
cargo test -p vuma-codegen --lib scheduler      # Instruction scheduler (6 tests)
cargo test -p vuma-codegen --lib opt            # Optimizer (15 tests)
cargo test -p vuma-parser --lib                 # Parser (289 tests)
cargo test -p vuma-proof --lib                  # Proof system (132 tests)
cargo test -p vuma-tests --lib wave48           # Bootstrap self-host (9 tests)
```

## License

MIT
