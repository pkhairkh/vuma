# VUMA — Verified-Unsafe Memory Access

**Version:** 0.2.0-alpha.1  
**License:** MIT  
**Toolchain:** nightly-2026-03-01  

VUMA is a programming language compiler written in Rust (203 source files, ~282K lines). It compiles a C-like language with memory verification to 10 CPU architectures. The language has `unsafe` blocks, `allocate`/`free` for memory, `extern "C"` FFI, structs, enums, match, imports, and type annotations.

## What Actually Works

### Compiler Pipeline

```
Source → Lexer → Parser → AST → SCG → [BD Inference → MSG → IVE Verification] → IR → RegAlloc → Codegen → ELF/Wasm
```

The verification step (in brackets) is optional. The `compile_dump` binary (used by the test suite) uses `--verification none` and the canonical SCG pipeline (`bridge_scg_to_codegen`). The `vuma emit` command uses a direct AST→codegen path (`bridge_ast_to_codegen_scg`). Both produce working binaries.

**Pipeline stages** (in `src/pipeline.rs`, enum `PipelineStage`):
1. Parse → 2. AstToScg → 3. ScgValidation → 4. BdInference → 5. MsgConstruction → 6. IveVerification → 7. ScgTransforms → 8. IrLowering → 9. RegisterAlloc → 10. CodeEmission → 11. CorInit

Stages 4-6 are skipped when `--verification none` is used.

### 10 Backend Architectures

All 10 backends pass 5,738 gold-standard test programs (57,380 total runs) at 100% with `--verification none`:

| Backend | ELF | Endian | Pointer | Syscall Stubs |
|---------|-----|--------|---------|---------------|
| x86_64 | ELF64 | Little | 64-bit | 31 |
| AArch64 | ELF64 | Little | 64-bit | ~20 |
| RISC-V 64 | ELF64 | Little | 64-bit | 8 |
| ARM32 | ELF32 | Little | 32-bit | ~20 |
| MIPS64 | ELF64 | Little | 64-bit | ~20 |
| PPC64 | ELF64 | Big | 64-bit | 3+table |
| LoongArch64 | ELF64 | Little | 64-bit | ~20 |
| x86_32 | ELF32 | Little | 32-bit | ~20 |
| RISC-V 32 | ELF32 | Little | 32-bit | ~20 |
| Wasm32 | Wasm | Little | 32-bit | N/A |

**Note:** PPC64 uses ELFv2 ABI (`e_flags = 0x2`). Do not change this — `qemu-ppc64` on the Pi 5 requires ELFv2 for big-endian.

### Language Features (from `src/parser/src/ast.rs`)

**Items** (enum `Item`): FnDef, StructDef, EnumDef, RegionDef, Import, Export, Const, Static, ModuleDef, TraitDef, ConceptDecl, GestaltDecl, ManifoldDecl, ExternBlock, ImplBlock

**Statements** (enum `Stmt`): Let, Assign, CompoundAssign, Allocate, Free, Access, Cast, If, While, For, Loop, UnsafeBlock, Match, Break, Continue, Return, Expr, Block

**Expressions** (enum `Expr`): Var, Lit, BinOp, UnOp, Call, Cast, Deref, AddressOf, FieldAccess, Index, Offset, Allocate, Spawn, Async, Await, AtomicLoad, AtomicStore, AtomicCas, Range, Tuple, StructInit, FormatStr, Closure, ConceptQuery, GestaltInterpret, ContextAssert

**Types** (enum `Type`): I8/I16/I32/I64, U8/U16/U32/U64, F32/F64, Bool, Address, String, Void, Named, Array, Pointer, Tuple, Generic, Region

**Lexer token kinds**: 350 variants in `TokenKind` enum (`src/parser/src/lexer.rs`)

**Note:** `unsafe` IS a keyword in VUMA (TokenKind::Unsafe, parser has `parse_unsafe_block`). `map_device()` and `volatile` are NOT language features — they appear only in example comments.

### IR (from `src/codegen/src/ir.rs`)

**IRType**: I8, I16, I32, I64, U8, U16, U32, U64, F32, F64, Ptr, Void, Func, Struct, Array, TaggedUnion

**IRInstr variants**: Load, Store, BinOp, UnaryOp, Add, Sub, Mul, Div, Cmp, Branch, CondBranch, Call, Ret, Alloc, Free, Cast, Offset, GetAddress, Phi, Select, CtSelect, CtEq, AtomicLoad, AtomicStore, AtomicCas

**BinOpKind**: Add, Sub, Mul, SDiv, UDiv, SRem, URem, And, Or, Xor, Shl, ShrL, ShrA, Ror, Rol, SLt, SLe, SGt, SGe, ULt, ULe, UGt, UGe, Eq, Ne

**Note:** BinOp IR instructions have `ty: Option<IRType>` which is `None` for most operations. The backends use 64-bit instructions by default. Do not propagate type info to BinOp IR — it causes regressions on ppc64.

### SCG (from `src/scg/src/node.rs`, `src/scg/src/edge.rs`)

**NodeType**: Computation, Allocation, Deallocation, Access, Cast, Effect, Control, Phantom, VTable, ClosureEnv, StructDef, EnumDef, Match, ConstantTime

**NodePayload**: 14+ variants matching NodeType (Computation, Allocation, Deallocation, Access, Cast, Effect, Control, Phantom, VTable, ClosureEnv, StructDef, EnumDef, Match, ConstantTime) plus "WOMB DATA MODELS" (ConceptDecl, GestaltDecl, ManifoldDecl — parsed but not lowered to IR)

**EdgeKind**: DataFlow, ControlFlow, Derivation, Annotation, Dispatch, Call{from_node, to_node, caller_region}, Return{from_node, to_node, caller_region}, Sync

The SCG is backed by `petgraph::DiGraph` (external crate). Cycles are handled via `topological_sort_with_cycles()` using Tarjan's SCC algorithm.

### Verification (IVE)

The IVE verifies 5 invariants: Liveness, Exclusivity, Interpretation, Origin, Cleanup.

**Verification levels** (enum `VerificationLevel`): None, Quick, Normal, Exhaustive

**Current state:** `--verification normal` has false positives on valid programs (especially those using `allocate()`/`free()` with dereference). The test suite uses `--verification none`. A modular verification infrastructure exists in `src/ive/src/modular.rs` (IncrementalCache, AbstractRegionTracker, per-function verification) but is not integrated into the main pipeline.

### CLI Commands (from `src/main.rs`, enum `Commands`)

| Command | What It Does |
|---------|-------------|
| `vuma build <file>` | Canonical pipeline: parse → SCG → IR → codegen → ELF (aarch64 by default) |
| `vuma emit <isa> <file>` | Direct AST→codegen path, emit to specific ISA |
| `vuma run <file>` | Build + execute via QEMU (requires QEMU installed) |
| `vuma check <file>` | Parse + SCG + verification only (no codegen) |
| `vuma verify <file>` | Run IVE 5-invariant verification |
| `vuma compile <file>` | Compile to relocatable object file (ET_REL) |
| `vuma disasm <file>` | Disassemble a binary |
| `vuma repl` | Interactive REPL (parse and display AST) |
| `vuma lsp` | Language Server Protocol |
| `vuma pkg <cmd>` | Package manager (init, build, add) |

### FFI & Syscalls

`extern "C" { fn write(fd: i64, buf: Address, count: i64) -> i64; }` blocks work on all 10 backends. Each backend has its own syscall stubs (encoded as raw machine code in `build_runtime_syscall_stubs()` or equivalent).

**`__vuma_alloc(size)`** — mmap wrapper, provides heap memory that persists across function calls. Available on all 10 backends.

**`__vuma_free(addr, size)`** — munmap wrapper.

**`allocate(size)`** — VUMA language builtin, creates stack-local memory (freed on function return). Use `__vuma_alloc` for cross-function persistence.

**`free(ptr)`** — VUMA language builtin, no-op for stack allocations.

### Heap Allocation

`__vuma_alloc` (mmap) and `__vuma_free` (munmap) are the correct way to allocate heap memory in VUMA. They work across function calls on all 10 backends. The `allocate()` builtin creates stack-local memory that is lost when the function returns.

---

## Workspace Structure (11 crates, from `Cargo.toml`)

| Crate | Path | Lines | Tests | Role |
|-------|------|-------|-------|------|
| `vuma` (root) | `src/` | ~60K | — | Pipeline, CLI, LLM API, LSP, FFI, diagnostics |
| `vuma-scg` | `src/scg/` | ~8K | 36/36 | SCG core (petgraph-backed graph, nodes, edges, regions) |
| `vuma-parser` | `src/parser/` | ~12K | 286/286 | Lexer (350 token kinds), parser, AST, AST→SCG bridge, module resolver |
| `vuma-codegen` | `src/codegen/` | ~80K | — | IR, 10 backends, regalloc, DWARF, ELF emission |
| `vuma-ive` | `src/ive/` | ~15K | — | Inference & Verification Engine (5 invariants, BD solver) |
| `vuma-bd` | `src/bd/` | ~8K | — | Behavioral Descriptors (RepD, CapD, RelD) |
| `vuma-core` | `src/vuma/` | ~20K | 301/301 | Memory State Graph, invariants, regions, derivations |
| `vuma-cor` | `src/cor/` | ~10K | — | Continuous Optimization Runtime (partially integrated) |
| `vuma-proof` | `src/proof/` | ~8K | — | Formal proof system |
| `vuma-std` | `src/std/` | ~24K | — | Rust stdlib wrapper (NOT linked to VUMA programs) |
| `vuma-package` | `src/package/` | ~3K | — | Package manager (manifest, resolver, registry) |
| `vuma-tests` | `src/tests/` | ~30K | — | Integration tests, benchmarks |

**External dependencies** (from `Cargo.toml`): petgraph 0.6, serde 1, serde_json 1, hashbrown 0.14, indexmap 2, smallvec 1, clap 4, chrono 0.4, toml 0.8, tempfile 3, proptest 1, thiserror 1, anyhow 1, log 0.4, env_logger 0.10, colored 2, libc 0.2

---

## Womb (VUMA-Native Library)

115 `.vuma` files, ~65K lines. All compile on x86_64 with `--verification none`. Not auto-imported — programs must inline needed functions.

| Category | Files | Key Modules |
|----------|-------|-------------|
| Collections | 4 | vec.vuma (heap-backed), hashmap.vuma, btree_map.vuma, enum_map.vuma |
| Strings | 3 | string.vuma, utf8.vuma (VStr), string_builder.vuma |
| File I/O | 2 | file.vuma (raw syscalls), high_level.vuma (read_file, write_file, path ops) |
| Alloc | 1 | arena.vuma (bump allocator on mmap) |
| Graph | 2 | digraph.vuma (heap-backed, dynamic grow), algorithms.vuma (toposort, cycle detection) |
| I/O | 1 | buffered.vuma (BufReader/BufWriter) |
| Env | 1 | cli.vuma (CLI arg parsing) |
| Language | 12 | full_lexer, full_parser, ir_builder, codegen, elf, tokens, ast, ir, mini_compiler, self_host_test, lexer, parser |
| Crypto | 44 | sha256, aes128/192/256, hmac, chacha20, poly1305, rsa, ecdsa, ed25519, etc. |
| Encoding | 3 | base64, hex, url |
| Network | 10+ | tcp, udp, dns, http, websocket, mqtt, smtp |
| Codec | 1 | byte_utils.vuma (LE/BE store/load, mem_copy/set/cmp) |
| Other | ~30 | math, stdlib, stdio, time, socket, threading, sync, etc. |

**Known issues:**
- `allocate()` creates stack-local memory. Use `__vuma_alloc()` for heap memory.
- While-loop variable tracking across function calls has a compiler bug.
- The import system works but has limitations with complex module graphs.

---

## Test Suite

5,738 programs with expected exit codes (5,754 total .vuma files), across 16 categories:

| Category | Programs | Covers |
|----------|----------|--------|
| arithmetic | 377 | Add, sub, mul, div, mod |
| bitwise | 350 | AND, OR, XOR, shifts, rotates |
| memory | 377 | allocate, store, load, free |
| control_flow | 350 | if/else, while, for, break, continue |
| pointers | 350 | Pointer arithmetic, dereference |
| functions | 350 | Calls, recursion, parameters |
| structs | 349 | Struct fields, enum tagged unions |
| atomics | 350 | AtomicLoad, AtomicStore, AtomicCas |
| u32_arith | 350 | 32-bit arithmetic with overflow masking |
| edge_cases | 350 | Boundary conditions |
| multi_function | 350 | Many functions calling each other |
| complex_stores | 348 | Multi-byte stores, computed addresses |
| nested_loops | 350 | 2-3 level nested loops |
| linked_structures | 335 | Linked lists, trees, ring buffers |
| crypto_patterns | 350 | Hash and checksum patterns |
| concurrency | 350 | Lock-free structures, atomics, channels |

Run: `bash scripts/pi5_test_suite.sh --workers 4 --fresh`  
Test runner: `test_results/run_tests.py` (234 lines)  
Compile binary: `compile_dump` (uses canonical SCG pipeline, `--verification none`)

---

## Known Limitations

| Area | Status | Details |
|------|--------|---------|
| Self-hosting | ❌ Not started | VUMA cannot compile itself. Womb language modules exist individually. |
| Verification | ⚠️ False positives | `--verification normal` rejects valid programs using allocate/free. Test suite uses `--verification none`. |
| Type checking | ❌ Not implemented | Parser recognizes syntax but doesn't validate types. |
| BD inference (M2.3) | ❌ Deferred | Complex generic inference scenarios. |
| Doubly-linked list verification (M2.4) | ⚠️ Partial | Not fully verified. |
| Concurrent verification | ⚠️ Limited | Single-threaded only. |
| COR runtime | ⚠️ Partial | Not fully integrated end-to-end. |
| Standard library | ⚠️ Partial | `vuma-std` (Rust) not linked. Womb (VUMA) exists but not auto-imported. |
| While-loop variable tracking | ⚠️ Bug | Loop variables across function calls may not propagate correctly. |
| `map_device()` | ❌ Not a feature | Referenced in example comments only. |
| `volatile` | ❌ Not a feature | Not implemented. |
| BinOp type propagation | ⚠️ Do not attempt | Propagating `ty` to BinOp IR causes ppc64 regression. Keep `ty: None`. |

---

## Build

```bash
git clone https://github.com/pkhairkh/vuma.git
cd vuma
make setup    # Install nightly-2026-03-01 toolchain
make build    # cargo build --release
```

Rust nightly is required (`rust-toolchain.toml` pins `nightly-2026-03-01`).

---

## Documentation

| File | Content |
|------|---------|
| `docs/architecture.md` | System architecture (11 crates, pipeline) |
| `docs/language-reference.md` | VUMA syntax (1,257 lines) |
| `docs/ROADMAP.md` | Milestones and current status |
| `docs/CONTRIBUTING.md` | Build, test, code review process |
| `docs/GLOSSARY.md` | Term definitions |
| `docs/specs/` | 15 formal specification documents |
