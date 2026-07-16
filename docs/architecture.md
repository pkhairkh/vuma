# VUMA Architecture

VUMA (**V**erified-**U**nsafe **M**emory **A**ccess) is a systems programming
language framework that makes `unsafe` memory access *verifiable*. Every
allocation, pointer derivation, access, and synchronisation event is tracked
through a chain of intermediate representations and checked against five formal
invariants before any machine code is emitted.

This document describes the compilation pipeline, the intermediate
representations, the verification system, the code generator, the VUMA-native
standard library, and the self-hosting bootstrap compiler.

---

## Table of Contents

1. [Compilation Pipeline](#1-compilation-pipeline)
2. [Intermediate Representations](#2-intermediate-representations)
3. [Verification System](#3-verification-system)
4. [Code Generation](#4-code-generation)
5. [Standard Library (`womb/`)](#5-standard-library-womb)
6. [Self-Hosting](#6-self-hosting)

---

## 1. Compilation Pipeline

VUMA is organised as a Cargo workspace of ten crates. The top-level
[`vuma`](../src/pipeline.rs) crate wires them into a single `compile` entry
point. Source text flows through the following stages, each producing a
well-defined artifact consumed by the next:

```
Source ─► Parse ─► AST ─► SCG ─► BD Inference ─► MSG Construction
        ─► IVE Verification ─► SCG Transforms ─► IR Lowering
        ─► Optimization ─► Register Allocation ─► Code Emission ─► ELF
```

| Stage              | Crate            | Artifact                | What it does                                                                                |
|--------------------|------------------|-------------------------|---------------------------------------------------------------------------------------------|
| **Lex**            | `vuma-parser`    | Token stream            | Tokenises source into `TokenKind`s (including the first-class `Syscall` token).            |
| **Parse**          | `vuma-parser`    | `AstProgram`            | Recursive-descent parser builds a typed AST (`ast` module).                                 |
| **Resolve**        | `vuma-parser`    | Resolved AST            | `ModuleResolver` resolves imports/exports and cross-module references.                     |
| **AST → SCG**      | `vuma-parser`    | `SCG`                   | `AstToScg` lowers the AST into a Semantic Computation Graph.                                |
| **BD Inference**   | `vuma-bd`        | `BDMap`                 | Derives a Behavioural Descriptor (RepD/CapD/RelD) for every value.                          |
| **MSG Construction**| `vuma-core`     | `MSG`                   | `scg_to_msg` materialises the Memory State Graph (regions, derivations, accesses, sync).    |
| **IVE Verification**| `vuma-ive`      | `AggregatedResult`      | Runs the five invariants and (optionally) attempts formal proofs.                           |
| **SCG Transforms** | `vuma-scg`       | Optimised SCG           | `PassManager` runs DCE, constant folding, CSE, inlining, LICM, strength reduction, tail-call detection. |
| **IR Lowering**    | `vuma-codegen`   | `IRProgram`             | `scg_to_ir::ScgToIr` lowers SCG nodes into the typed IR.                                    |
| **Optimization**   | `vuma-codegen`   | Optimised `IRProgram`   | `opt::run_optimizations` runs DCE, CSE, inlining, LICM, scheduling, escape analysis, e-graph rewrites. |
| **Register Alloc** | `vuma-codegen`   | `AllocatedProgram`      | `LinearScanAllocator` assigns physical registers (stack-based lowering for Wasm).           |
| **Code Emission**  | `vuma-codegen`   | ELF / Wasm bytes        | `emit::emit_binary` selects instructions per backend and writes the binary.                 |

The `CompileConfig` struct controls the run: target platform (`Linux` /
`Wasm32`), optimisation level (`O0`–`O3`), verification level (see
[§3](#3-verification-system)), inline threshold, memory-safety switches, and
ELF section options.

```rust
use vuma::pipeline::{compile, CompileConfig};

let config = CompileConfig::default();   // O2, Normal verification, Linux/AArch64
let output = compile(source, &config)?;
// output.binary        — Vec<u8> ELF bytes
// output.scg           — final SCG (post-transform)
// output.ive_result    — AggregatedResult with per-invariant verdicts
// output.msg           — Memory State Graph
```

Verification runs *before* optimisation so that the verifier sees the program
the programmer wrote, not the program the optimiser rewrote. SCG transforms
and IR optimisation then run on the already-verified graph.

---

## 2. Intermediate Representations

VUMA uses four intermediate representations, each addressing a different
concern. They are not aliases — moving between them requires an explicit,
loss-checked conversion.

### 2.1 AST — Abstract Syntax Tree

Produced by `vuma-parser::parser::Parser`. The AST is a typed tree
(`ast` module) capturing the full surface grammar: items (functions, structs,
enums, extern blocks, imports), statements, and expressions. It preserves
source ordering and span information for diagnostics but carries no semantic
facts about memory.

### 2.2 SCG — Semantic Computation Graph

The SCG (crate `vuma-scg`) is the central semantic data structure. It models
program semantics as a directed graph where:

- **Nodes** represent operations — computation, allocation, deallocation,
  memory access, type casts, side effects, control flow, syscalls, and
  phantom markers.
- **Edges** represent relationships — data flow, control flow, derivation
  (how a pointer was derived from a region), and annotation.
- **Regions** group nodes into memory scopes with security boundaries and
  deployment targets (heap, stack, mmap).

The SCG module ships with a query engine, a diff/merge engine for
incremental re-verification, dominance and liveness analyses, loop detection,
a call graph, and a transform pass manager.

### 2.3 MSG — Memory State Graph

The MSG (crate `vuma-core`) is the verification-facing projection of the SCG,
produced by `scg_to_msg`. It is organised around four concepts:

| Concept        | Module        | Description                                        |
|----------------|---------------|----------------------------------------------------|
| **Region**     | `region`      | Contiguous memory span (heap, stack, mmap, …).     |
| **Derivation** | `derivation`  | How a pointer was derived from a region.           |
| **Access**     | `access`      | A read or write at a program point.                |
| **Sync Edge**  | `sync`        | Ordering between accesses (happens-before, atomic, mutex). |

The MSG is what the IVE verifiers and the formal proof system consume.

### 2.4 IR — Code-Generation IR

The IR (crate `vuma-codegen`, module `ir`) is a typed, register-oriented
representation used during and after optimisation. An `IRProgram` is a set of
`IRFunction`s, each composed of `IRBlock`s of `IRInstr`s terminated by a
branch or return. Values are `IRValue`s (virtual registers, immediates, or
symbol references) typed by `IRType`.

The `IRInstr` enum has **27 variants**, grouped by purpose:

| Group                  | Variants                                                                                       |
|------------------------|------------------------------------------------------------------------------------------------|
| Memory                | `Load`, `Store`, `Alloc`, `Free`, `GetAddress`, `Offset`                                       |
| Arithmetic            | `BinOp`, `UnaryOp`, `Add`, `Sub`, `Mul`, `Div`                                                 |
| Comparison / select   | `Cmp`, `Select`, `Phi`                                                                         |
| Control flow          | `Branch`, `CondBranch`, `Ret`                                                                   |
| Calls                 | `Call` (with `is_extern` flag), `Syscall`                                                       |
| Casts                 | `Cast` (with `CastKind`, optional `from_ty`/`to_ty`)                                            |
| Atomics               | `AtomicLoad`, `AtomicStore`, `AtomicCas`                                                       |
| Constant-time crypto  | `CtSelect`, `CtEq` (branch-free, side-channel-safe lowering)                                   |
| SIMD                  | `VectorOp` (packed `Add`/`Sub`/`Mul` over `lanes × elem_size`)                                 |

Each variant declares its defined and used virtual registers via
`defined_regs()` / `used_regs()`, which the dataflow analyses and register
allocator rely on. The `Syscall` variant is a first-class IR node — distinct
from `Call { is_extern: true }` — so the compiler can track syscalls directly
and apply a verification-level allowlist.

### IR relationships

```
AST ──AstToScg──► SCG ──scg_to_msg──► MSG ──IVE/proof──► verdicts
                  │
                  └──ScgToIr──► IR ──opt──► IR' ──regalloc──► AllocatedProgram ──emit──► ELF
```

---

## 3. Verification System

Verification is the reason VUMA exists. It is structured as three cooperating
layers: a set of **invariants** checked by the IVE engine, a **Behavioural
Descriptor** layer that characterises every value, and a **formal proof**
system that discharges obligations the heuristic verifiers cannot close.

### 3.1 The Five Invariants

The IVE crate (`vuma-ive`) checks five core invariants against the MSG. Each
invariant has its own module, verifier, and result type:

| Invariant          | Module              | Ensures                                                                 |
|--------------------|---------------------|-------------------------------------------------------------------------|
| **Liveness**       | `liveness`          | Every accessed region was allocated and not yet freed (no use-after-free, no leaks). |
| **Exclusivity**    | `exclusivity`       | Mutating accesses do not race with concurrent reads/writes; atomicity and ordering are correct. |
| **Cleanup**        | `cleanup`           | Every allocated resource is released on all paths; temporaries do not leak. |
| **Origin**         | `origin`            | Every pointer is derived from a region it is permitted to reach; derivation chains are well-formed. |
| **Interpretation** | `interpretation`    | A value is only accessed through a capability that permits the access (read/write) and a compatible representation (no invalid reinterpretation). |

A sixth invariant — **constant-time** — is available as an opt-in level (see
below) and detects secret-dependent branches and memory accesses via taint
propagation.

The `InvariantAggregator` runs all enabled invariants and produces an
`AggregatedResult` with an `OverallVerdict` (`Verified`, `Violated`, or
`Inconclusive`). `Inconclusive` means "no violation proven, but not all
invariants discharged" — the `strict_verification` config flag controls
whether it blocks compilation.

### 3.2 Verification Levels

The IVE `VerificationLevel` enum selects which invariants run and how deeply
they explore:

| Level            | Invariants run                                         | Extra analyses                         |
|------------------|--------------------------------------------------------|----------------------------------------|
| `Quick`          | All five, with halved `max_paths` / `max_path_length`  | —                                      |
| `Normal` (default) | All five                                             | —                                      |
| `Exhaustive`     | All five                                               | Formal proof attempts, interprocedural |
| `Modular`        | All five                                               | Per-function verification summaries    |
| `ConstantTime`   | All five + constant-time (6th)                         | Taint propagation                      |
| `Hardened`       | All six                                                | Interprocedural + modular              |

The pipeline-level `VerificationLevel` adds a `None` option (skip verification
entirely) on top of the six IVE levels.

### 3.3 Behavioural Descriptors (BD)

The BD crate (`vuma-bd`) characterises every value along three orthogonal
axes. The top-level `BD` struct composes them and provides compatibility,
refinement, and composition queries.

| Layer   | Module        | Characterises                                                 |
|---------|---------------|---------------------------------------------------------------|
| **RepD** | `repd`       | Representation — memory shape, size, alignment (e.g. `ByteRep { size: 8, align: 8 }`). |
| **CapD** | `capd`       | Capability — permitted operations (`Read`, `Write`, …) organised as a lattice (`capd_lattice`). |
| **RelD** | `reld`       | Relational — temporal, dependency, and security relations (`reld`, `reld_refine`). |

Supporting modules provide a solver context (`context`, `context_solver`),
inference (`inference`), unification (`unify`), compatibility checking
(`repd_compat`), and structured error reporting. The IVE inference engine
propagates BDs across the SCG; the verifiers then check that each access is
permitted by the accessed value's BD.

### 3.4 Formal Proof System

When the heuristic verifiers cannot close an obligation, the proof crate
(`vuma-proof`) can attempt a formal proof. It provides:

- **Proof objects** (`proof`) — structured proofs with goals, facts, steps,
  and conclusions.
- **Judgments & models** (`judgment`, `models`) — the formal vocabulary:
  regions, derivations, accesses, sync edges, capabilities, origins.
- **Inference rules** (`rules`) — domain-specific rules for liveness,
  exclusivity, derivation chains, bounds preservation, cast validity, and
  temporal ordering.
- **Per-invariant provers** — `liveness_proofs`, `exclusivity_proofs`,
  `origin_proofs`, `interpretation_proofs`, `cleanup_proofs`.
- **Proof checker** (`checker`) — verifies that proof steps follow from
  previous steps using the stated rules, with circular-reasoning detection.
- **Counterexamples** (`counterexample`) — constructs minimal
  counterexamples from proof failures to aid debugging.
- **Tactics** (`tactics`) — automated strategies: simplification, induction,
  contradiction, and an auto-mode.

---

## 4. Code Generation

The codegen crate (`vuma-codegen`) is responsible for everything from IR
lowering through binary emission. It is the largest crate in the workspace and
the only one that knows about target ISAs.

### 4.1 Backends

VUMA ships **19 backends** spanning 14 instruction-set architectures and both
endiannesses:

| Backend       | ISA / variant             | Backend       | ISA / variant             |
|---------------|---------------------------|---------------|---------------------------|
| `arm64`       | AArch64 (LE)              | `aarch64_be`  | AArch64 (BE)              |
| `arm32`       | ARM 32-bit EABI           | `armeb`       | ARM 32-bit (BE)           |
| `x86_64`      | x86-64                    | `x86_32`      | x86-32 (i386)             |
| `riscv64`     | RISC-V 64-bit             | `riscv32`     | RISC-V 32-bit             |
| `loongarch64` | LoongArch 64-bit          | `mips64`      | MIPS-III / N64            |
| `mips64be`    | MIPS N64 (BE)             | `ppc64`       | PowerPC 64-bit (BE)       |
| `ppc64le`     | PowerPC 64-bit (LE)       | `s390x`       | IBM System Z              |
| `sparc64`     | SPARC V9                  | `alpha`       | DEC Alpha                 |
| `hppa`        | PA-RISC 2.0               | `m68k`        | Motorola 68000            |
| `wasm32`      | WebAssembly 32-bit (WASI) |               |                           |

Each backend implements the `Backend` trait and exposes `TargetInfo`
(pointer size, endianness, register file, calling convention). A
`TargetDescRegistry` provides machine-readable target descriptions for the
optimiser's cost model.

### 4.2 Syscall ABI Translation

The VUMA `syscall(nr, args…)` intrinsic is first-class: it lexes as
`TokenKind::Syscall`, parses as `Expr::Syscall`, and lowers to
`IRInstr::Syscall { nr, args, dst }`. To keep source programs portable across
architectures whose native syscall numbers differ, the `syscall_abi` module
defines a **VUMA-generic numbering** (Linux `asm-generic/unistd.h`) and a
per-arch translation layer.

| Class                | Architectures                                                          | Behaviour                                            |
|----------------------|------------------------------------------------------------------------|------------------------------------------------------|
| **Identity** (5)     | `aarch64`, `riscv64`, `riscv32`, `loongarch64`, `arm32` (EABI)         | `translate` is the identity function — generic number is native. |
| **Translated** (9)   | `x86_64`, `x86_32`, `mips64`, `ppc64`, `s390x`, `sparc64`, `alpha`, `hppa`, `m68k` | Per-arch `match` table covers ~106 common syscalls; returns `None` for unknown numbers. |
| **BE wrappers** (4)  | `aarch64_be`, `armeb`, `mips64be`, `ppc64le`                           | Delegate to their LE/bare counterpart.               |
| **No syscalls** (1)  | `wasm32`                                                               | Uses `vuma.*` host imports; `translate` returns `None`. |

Across the 9 translated arches the translation tables contain **949 mappings**
in total. `translate_or_warn` returns the generic number verbatim (with a
warning) for unknown numbers, preserving the kernel's `-ENOSYS` behaviour
rather than aborting compilation.

### 4.3 Optimizer

IR optimisation is driven by `opt::run_optimizations` and its profile-aware
and threshold-aware variants. The pass pipeline:

| Pass                  | Module             | Effect                                                                 |
|-----------------------|--------------------|------------------------------------------------------------------------|
| **Dead Code Elimination** | `opt`          | Removes instructions whose results are never used.                     |
| **Constant Folding**      | `opt`          | Evaluates compile-time-known expressions.                              |
| **Common Subexpression Elimination** | `opt`  | Replaces redundant computations with a single definition (`cse`).      |
| **Inliner**               | `opt`          | Inlines callees under a cost threshold (`inline_small`, `inline_with_threshold`). |
| **Loop-Invariant Code Motion** | `opt`     | Hoists loop-invariant instructions to preheaders (`licm`).             |
| **e-graph rewrites**      | `egraph`       | Equality-saturation–based rewrites; rules verified by `bv_verify` (bitvector enumeration). |
| **Scheduler**             | `scheduler`    | List-schedules instructions within blocks using a target latency table (`schedule_block`, `schedule_function`). |
| **Escape Analysis**       | `escape_analysis` | Identifies allocations that do not escape, enabling SROA / alloc elision at O2+. |
| **Effects analysis**      | `effects`      | Interprocedural effect propagation feeding the optimiser and verifier. |
| **Alias analysis**        | `alias_analysis` | Disambiguates memory accesses for the scheduler and LICM.              |
| **Loop unrolling**        | `loop_unroll`  | Correct, bounded unrolling.                                            |
| **SLP Vectorization**     | `vectorize`    | Packs independent isomorphic scalar ops into `IRInstr::VectorOp`.      |
| **Monomorphization**      | `monomorphize` | Specialises generic functions for concrete type arguments.             |
| **Closure conversion**    | `closures`     | Lifts closures into environment-carrying functions.                    |

The optimisation level (`O0`–`O3`) gates which passes run: `O0` runs none,
`O1` runs DCE + constant folding, `O2` (default) adds CSE, inlining under the
configured threshold, LICM, and escape-driven SROA, and `O3` raises the inline
threshold to admit larger callees.

### 4.4 Register Allocation

`regalloc::LinearScanAllocator` performs linear-scan register allocation over
each function's virtual-register live ranges, producing an `AllocatedProgram`
of `AllocatedFunction`s / `AllocatedBlock`s / `AllocatedInstruction`s with
physical `PhysicalReg` assignments. Allocation runs in parallel across
functions using `std::thread::scope`. The Wasm32 backend bypasses register
allocation entirely — its lowering is stack-based, with each vreg spilled to a
linear-memory slot.

### 4.5 Emission

`emit::emit_binary` takes the allocated program, runs each backend's
instruction selector and encoder, applies relocations, and writes the output
format:

- **ELF** for the 18 native ISAs (`OutputFormat::Elf`), with optional DWARF
  debug info (`dwarf` module) and optional section headers.
- **Wasm module** for `wasm32` (`OutputFormat::Wasm`), with disassembly via
  `wasm32::disasm`.

ELF emission reports unresolved relocations as fatal
`CodegenError::UnresolvedRelocation` errors naming the symbol, function,
offset, and relocation type.

---

## 5. Standard Library (`womb/`)

`womb/` is VUMA's standard library, written entirely in VUMA itself. It
contains **108 `.vuma` files** organised by domain:

| Directory        | Contents                                                                 |
|------------------|--------------------------------------------------------------------------|
| `womb/lang/`     | The bootstrap compiler (see [§6](#6-self-hosting)) and test programs.    |
| `womb/lib/`      | Core stdlib: `stdio`, `string`, `fileio`, `math`, `json`, `printf`, `time`, `http`, `http2`, `websocket`, `dns`, `tls12`, `event_loop`, `threading`, `auth`, `email`, … |
| `womb/crypto/`   | Modern and post-quantum crypto: `aes128/192/256`, `chacha20`, `poly1305`, `sha1/256/384/512`, `sha3`, `blake2/3`, `hmac`, `hkdf`, `pbkdf2`, `scrypt`, `argon2`, `rsa`, `ecdsa_p256/p384`, `ed25519`, `x25519`, `ml_kem`, `ml_dsa`, `slh_dsa`, `falcon`, `hqc`, `bignum`, `bignum2048`, `drbg`, `crc`, … |
| `womb/net/`      | Network protocols: `tcp`, `quic`, `tls13`, `ssh`.                        |
| `womb/collections/` | `vec`, `hashmap`, `btree_map`, `enum_map`.                             |
| `womb/containers/` | `containers.vuma`.                                                     |
| `womb/string/`   | `utf8`, `string`, `string_builder`.                                      |
| `womb/encoding/` | `base64`, `hex`, `url`.                                                  |
| `womb/codec/`    | `byte_utils`.                                                            |
| `womb/fs/`       | `file`, `high_level`.                                                    |
| `womb/io/`       | `buffered`.                                                              |
| `womb/graph/`    | `digraph`, `algorithms`.                                                 |
| `womb/ieee/`     | `fp`, `ieee_frames`.                                                      |
| `womb/alloc/`    | `arena`.                                                                  |
| `womb/env/`      | `cli`.                                                                    |
| `womb/core.vuma`, `womb/syscalls.vuma` | The syscall intrinsic wrappers and core prelude.       |

### The `syscall` intrinsic

`syscall(nr, args…)` is the lowest-level portable primitive. It lexes as a
dedicated token, parses as a first-class expression, lowers to
`IRInstr::Syscall`, and is translated to the target's native syscall number by
the [ABI layer](#42-syscall-abi-translation). `womb/syscalls.vuma` and
`womb/lib/stdio.vuma` build the higher-level I/O API on top of it.

---

## 6. Self-Hosting

VUMA is self-hosting. The bootstrap compiler lives in `womb/lang/` and is
written in VUMA. It is a complete — if subsetted — implementation of the
VUMA toolchain that the Rust compiler can build into an ELF, which in turn can
compile VUMA source.

### 6.1 The Bootstrap Compiler

The bootstrap compiler is **five files**:

| File                     | Lines | Role                                                        |
|--------------------------|-------|-------------------------------------------------------------|
| `womb/lang/full_lexer.vuma`  | 921   | Lexer — produces a token stream (24-byte tokens: `[type][start][len][value]`). |
| `womb/lang/full_parser.vuma` | 811   | Parser — builds a flat AST arena (`enum_map`) from the token stream. |
| `womb/lang/ir_builder.vuma`  | 902   | IR builder — lowers the AST into an IR buffer; contains stubs for SCG construction, BD inference, and IVE verification. |
| `womb/lang/codegen.vuma`     | 1380  | x86-64 code generator — emits machine bytes from IR; each vreg gets a stack slot at `[rbp - offset]`. |
| `womb/lang/elf.vuma`         | 135   | ELF64 writer — wraps the emitted `.text` into a static ELF executable. |

(`womb/lang/hello.vuma` and `womb/lang/hello2.vuma` are test programs, not
part of the compiler.)

### 6.2 Bootstrap Pipeline

`full_lexer.vuma:main()` drives the end-to-end bootstrap pipeline:

```
1. read womb/lang/hello.vuma into a heap buffer
2. full_lex()              → token stream          [full_lexer]
3. parse()                 → AST arena             [full_parser]
4. scg_construct()         → (stub, skipped)       [ir_builder]
5. irb_build_main()        → IR buffer             [ir_builder]
6. bd_infer()              → (stub, i64)           [ir_builder]
7. ive_verify()            → (stub, no-op)         [ir_builder]
8. codegen_emit()          → x86_64 bytes          [codegen]
9. write_elf64()           → a.out                 [elf]
10. exit 0 on success
```

The bootstrap test is a single command chain:

```sh
$ <rust-vuma-compiler> womb/lang/full_lexer.vuma -o vumac
$ ./vumac                         # reads womb/lang/hello.vuma
```

### 6.3 Language Coverage

The bootstrap parser implements the complete VUMA grammar:

- Struct and enum definitions
- `match` expressions with patterns
- Closures (`|args| expr`)
- Generics (`<T>`)
- Type annotations (`: Type`, `-> Type`)
- Import / export declarations
- `extern` blocks (for `__vuma_alloc`, `__vuma_free`, libc `write`, …)
- String and char literals
- All operators with correct precedence, including compound assignment
  (`+=`, `-=`, …), arrow (`->`), fat arrow (`=>`), and scope (`::`)

### 6.4 Current Limitations

The bootstrap compiler is a real compiler, not a stub, but it is deliberately
narrower than the Rust-hosted compiler:

- **Target:** x86-64 only. Code generation assigns every virtual register to
  a stack slot; there is no register allocator.
- **Verification:** SCG construction, BD inference, and IVE verification are
  present as stubs — the pipeline runs but does not enforce the five
  invariants. The Rust-hosted compiler performs full verification.
- **Optimisation:** none. The IR is lowered directly to machine code without
  DCE, CSE, inlining, or scheduling.
- **Syscalls:** linked via the existing backend's `extern "C"` stubs rather
  than the first-class `IRInstr::Syscall` path used by the Rust compiler.
- **Backend breadth:** single ISA, single output format (static ELF64).

These gaps are intentional: the bootstrap compiler exists to prove the
language can compile itself, and to anchor the standard library in VUMA
rather than Rust. Closing each gap (verification, optimisation, additional
backends) is tracked as open work on the Rust-hosted side, which remains the
production compiler.
