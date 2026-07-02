# VUMA — Verified-Unsafe Memory Access

**Version:** 0.2.0-alpha.1
**License:** MIT
**Toolchain:** nightly-2026-03-01 (pinned in `rust-toolchain.toml`)
**Repository:** https://github.com/pkhairkh/vuma
**Author:** Parham Khairkhah

VUMA is a programming language compiler framework written in Rust. It compiles a C-like language with behavioral verification to multiple CPU architectures. The language has `unsafe` blocks, `allocate`/`free` for memory, `extern "C"` FFI, structs, enums, match, imports, traits/impls, closures, atomics, and type annotations.

> ## ⚠️ Critical Known Issues (read before trusting any claim below)
>
> Empirical testing (building the workspace, running the compiler against examples, the bootstrap file, and `womb/lang/*`) reveals that several "works" claims in this README and the docs are aspirational, not real. The static source-code audit that produced the rest of this doc trusted comments and test-suite metadata too much. The following are **blocking issues** that mean VUMA is currently a research prototype, not a usable compiler:
>
> 1. **IVE verification is broken on the canonical idiom.** Every one of the 48 `examples/*.vuma` files fails default (`--verification normal`) verification. Even `examples/hello_memory.vuma` — whose own header claims all 5 invariants pass — fails with `error[ive-verification]: verdict: FAIL`. Root cause is documented in `src/pipeline.rs:5783-5797`: top-level `region` declarations have no ControlFlow edges, so `check_leaks` flags every program-lifetime arena as a leak. Spec §5.4 "Global scope / Static lifetime" inference is also unimplemented. **The language's flagship feature is unusable.** The only way to compile anything is `--verification none`.
> 2. **The bootstrap file does not compile.** `src/bootstrap/vuma_compiler.vuma` (730 lines, "Phase 5 goal") fails at SCG→MSG construction: `error[scg-to-msg]: access references unknown region RegionId(8)`. It is also only a lexer — no parser, IR builder, or codegen written in VUMA. It additionally has a live bug: `lex_identifier` references an undeclared global `src_len_global` (declared 200 lines later, never initialized by `main`), so even if it compiled it would loop forever. Self-hosting is at <5%.
> 3. **6 of 16 `womb/lang/*.vuma` files do not parse.** The parser supports `else if` (no braces) and `else { block }`, but not `else { if … } else { … }` chains — the inner `if` closes the outer one, leaving subsequent `else` orphaned. The entire `womb/lang` self-hosting effort is written in a style the parser rejects.
> 4. **`concept`/`gestalt`/`manifold`/`aura` are tokenized but never parsed.** The lexer produces `TokenKind::Concept/Gestalt/Manifold/Aura`, the AST has `Item::ConceptDecl` etc., the SCG has `NodeType::ConceptDecl` etc., but `parser.rs` has no `parse_concept_decl` — these tokens are never matched. The entire "Womb" data-model layer is a frontend gap, not just unfinished.
> 5. **Two parallel SCG→IR bridges with divergent semantics.** `vuma build` routes through the canonical semantic SCG (which verifies but produces broken code); `vuma emit` uses `bridge_ast_to_codegen_scg` (`src/main.rs:909`) which bypasses verification entirely. The verification IR (MSG) and the codegen IR are not connected — verification never sees what gets emitted. `compile_with_path` explicitly notes this.
> 6. **Memory ops are silently dropped in the AST→codegen bridge.** In `src/main.rs:1656-1657` and `1781-1784`: standalone `Allocate`, `Free`, `Match`, `Sync`, `UnsafeBlock`, `Access` statements generate zero instructions. `Cast` keeps the operand but loses the target type. The `region = allocate(N)` assignment form works (special-cased to `AllocationNode::Stack`), but `free(region)` after it is a no-op — every VUMA program leaks every allocation.
> 7. **Codegen produces binaries that crash or infinite-loop.** `region buf = allocate(1024);` at top level (the canonical pattern) → emitted x86_64 binary segfaults (SIGSEGV, exit 139). `womb/lang/minicompiler.vuma` (a 100-line toy parser, compiles cleanly) → emitted binary infinite-loops at runtime. Even with `--verification none` and the working emit path, the output is not trustworthy.
> 8. **`vuma run` is broken on non-aarch64 hosts.** `cmd_run` (`src/main.rs:488`) tries native exec first (returns ENOEXEC on x86_64 since `vuma build` defaults to AArch64 ELF), then falls back to `qemu-aarch64` — which isn't installed by default. There is no host-arch detection, no `--target` flag on `run`, no graceful error. Developers on x86_64 cannot run any VUMA program without manually installing qemu-aarch64 or remembering to use `vuma emit x86_64`.
> 9. **SCG→MSG errors are swallowed silently under Quick mode.** `src/pipeline.rs:4794`: when `verification_level == Quick`, SCG→MSG errors (including `AccessRegionNotFound`, the bootstrap's failure) are dropped and an empty MSG is substituted. This makes debugging the bootstrap essentially impossible without first knowing to use Normal mode.
>
> **Bottom line:** until items 1–3 above are fixed, writing a self-hosting compiler in VUMA is not achievable. The "99.99% gold-standard pass rate" below is real but only measures that 5,738 tiny test programs exit with the expected code under `--verification none` — it does **not** mean the verifier works, that emitted binaries are correct in general, or that the language is usable for programs larger than a few dozen lines.

## What Compiles (with caveats)

### Compiler Pipeline

```
Source → Lexer → Parser → AST → SCG → [BD Inference → MSG Construction → IVE Verification] → IR → RegAlloc → Codegen → ELF/Wasm
```

The verification step (in brackets) is optional. With `--verification none`, only the IVE verification stage (stage 6) is skipped — BD inference (stage 4) and MSG construction (stage 5) still run. The `compile_dump` binary (used by the test suite) uses `--verification none` and the canonical SCG pipeline (`bridge_scg_to_codegen` in `src/pipeline.rs`). The `vuma emit` command uses a direct AST→codegen path (`bridge_ast_to_codegen_scg`) that bypasses verification entirely. **Neither path produces trustworthy binaries in general** — see Critical Known Issues #5, #6, #7 above.


**Pipeline stages** (enum `PipelineStage` in `src/pipeline.rs:567`, 11 variants):
1. `Parse` — Lexing + parsing
2. `AstToScg` — AST → SCG conversion
3. `ScgValidation` — SCG validation
4. `BdInference` — BD inference (always runs)
5. `MsgConstruction` — SCG → MSG construction (always runs)
6. `IveVerification` — IVE verification (skipped when `--verification none`)
7. `ScgTransforms` — SCG transformation passes
8. `IrLowering` — IR lowering (SCG → IR)
9. `RegisterAlloc` — Register allocation
10. `CodeEmission` — Code emission
11. `CorInit` — COR (Continuous Optimization Runtime) initialization

### Backend Architectures

The codegen crate (`src/codegen/`) implements 10 backends (enum `BackendKind`, 10 variants). Test results from the latest full-suite run (`test_results/summary.json`, 2026-07-01 22:05:13 UTC, host `pi-pkhairkh-dev`):

| Backend | ELF | Endian | Pointer | Syscall Stubs | Pass Rate |
|---------|-----|--------|---------|---------------|-----------|
| x86_64 | ELF64 | Little | 64-bit | 26 syscalls + 5 runtime helpers | 5738/5738 |
| AArch64 | ELF64 | Little | 64-bit | 21 | 5738/5738 |
| RISC-V 64 | ELF64 | Little | 64-bit | 22 | 5737/5738 |
| ARM32 | ELF32 | Little | 32-bit | 22 | 5738/5738 |
| MIPS64 | ELF64 | **Big** | 64-bit | 21 | 5738/5738 |
| PPC64 | ELF64 | Big (ELFv2) | 64-bit | 21 | 5736/5738 |
| LoongArch64 | ELF64 | Little | 64-bit | 22 | 5738/5738 |
| x86_32 | ELF32 | Little | 32-bit | 21 syscalls + 4 helpers | 5738/5738 |
| RISC-V 32 | ELF32 | Little | 32-bit | 22 | 5738/5738 |
| Wasm32 | Wasm | Little | 32-bit | 0 (bump allocator) | 5738/5738 |

**Overall: 57,377 / 57,380 runs pass = 99.99%** (not 100%). Three failures:
- `crypto_patterns/crc32.vuma` — riscv64 and ppc64 return 170 (expected 38) — CRC32 polynomial mismatch on Big-endian/lower-width ISAs
- `functions/s27_fn_two_args_mod.vuma` — ppc64 returns -4 (expected 4) — signed modulo sign issue

**Notes:**
- PPC64 uses ELFv2 ABI (`e_flags = 0x2`). Do not change this — `qemu-ppc64` requires ELFv2 for big-endian.
- The CLI `vuma emit` and `vuma compile` commands accept only 8 ISA targets (enum `IsaArg` in `src/main.rs:137`): aarch64, x86_64, riscv64, wasm32, loongarch64, arm32, mips64, ppc64. RISC-V 32 and x86_32 exist in the codegen crate but are not exposed via the CLI `emit`/`compile` subcommands.
- `src/codegen/src/lib.rs.tmp` is a stale 1-line leftover and should be deleted.

### Language Features (from `src/parser/src/ast.rs`)

**Items** (enum `Item` at `ast.rs:102`, 17 variants): `FnDef`, `StructDef`, `EnumDef`, `RegionDef`, `Import`, `Export`, `Const`, `Static`, `ModuleDef`, `TraitDef`, `ImplBlock`, `ExternBlock`, `ConceptDecl`, `GestaltDecl`, `ManifoldDecl`, `AuraDecl`, `Stmt`

**Statements** (enum `Stmt` at `ast.rs:519`, 19 variants): `Let`, `Assign`, `CompoundAssign`, `Allocate`, `Free`, `Access`, `Cast`, `If`, `While`, `For`, `Loop`, `UnsafeBlock`, `Match`, `Sync`, `Return`, `Break`, `Continue`, `BdDirective`, `Expr`

> Note: `Block` is a struct (`ast.rs:506`), not a `Stmt` variant. A block of statements appears as `Expr(ExprStmt)` containing an `Expr::Block`.

**Expressions** (enum `Expr` at `ast.rs:934`, 33 variants): `Var`, `Lit`, `BinOp`, `UnOp`, `Call`, `AddressOf`, `Deref`, `Offset`, `Cast`, `Index`, `StructInit`, `FieldAccess`, `NamespaceAccess`, `Derive`, `Sizeof`, `Alignof`, `TypeAscription`, `Async`, `Spawn`, `Allocate`, `Null`, `CtSelect`, `CtEq`, `Range`, `FormatStr`, `Closure`, `Await`, `Uninitialized`, `AtomicLoad`, `AtomicStore`, `AtomicCas`, `Block`, `MatchExpr`

**Types** (enum `Type` at `ast.rs:1342`, 8 variants): `BDBase(String)`, `Ptr(Box<Type>)`, `RegionPtr { inner, region }`, `Array { element, size }`, `Struct { name, fields }`, `Generic { name, args }`, `Func { params, return_type }`, `BdAnnot { name }`

> Primitive types (`u8`, `u32`, `i64`, `bool`, `void`, `address`, etc.) are represented as `Type::BDBase(String)` — the string carries the primitive name. There are no per-primitive enum variants.

**Binary operators** (enum `BinOp` at `ast.rs:1260`, 19 variants): `Add`, `Sub`, `Mul`, `Div`, `Mod`, `Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge`, `And`, `Or`, `BitAnd`, `BitOr`, `BitXor`, `Shl`, `Shr`

**Unary operators** (enum `UnOp` at `ast.rs:1301`, 4 variants): `Neg`, `Not`, `Deref`, `BitNot`

**Lexer token kinds**: 141 variants in the `TokenKind` enum (`src/parser/src/lexer.rs:115`).

**`unsafe` IS a keyword** in VUMA (`TokenKind::Unsafe`, parser has `parse_unsafe_block`). `map_device()` and `volatile` are NOT language features — they appear only in example comments.

### IR (from `src/codegen/src/ir.rs`)

**IRType** (`ir.rs:43`, 16 variants): I8, I16, I32, I64, U8, U16, U32, U64, F32, F64, Ptr, Void, Func, Struct, Array, TaggedUnion

**IRInstr** (`ir.rs:1211`, 25 variants): Load, Store, BinOp, UnaryOp, Add, Sub, Mul, Div, Cmp, Branch, CondBranch, Call, Ret, Alloc, Free, Cast, Offset, GetAddress, Phi, Select, CtSelect, CtEq, AtomicLoad, AtomicStore, AtomicCas

**BinOpKind** (`ir.rs:951`, 25 variants): Add, Sub, Mul, SDiv, UDiv, SRem, URem, And, Or, Xor, Shl, ShrL, ShrA, Ror, Rol, SLt, SLe, SGt, SGe, ULt, ULe, UGt, UGe, Eq, Ne

> Note: BinOp IR instructions have `ty: Option<IRType>` which is `None` for most operations. The backends use 64-bit instructions by default. Do not propagate type info to BinOp IR — it causes regressions on ppc64.

### SCG (from `src/scg/src/node.rs`, `src/scg/src/edge.rs`)

**NodeType** (`node.rs:41`, 26 variants):
- Core (14): Computation, Allocation, Deallocation, Access, Cast, Effect, Control, Phantom, VTable, ClosureEnv, StructDef, EnumDef, Match, ConstantTime
- WOMB Data Models (12): ConceptDecl, ConceptField, ConceptAccess, GestaltDecl, GestaltInterpret, ContextAssert, ManifoldDecl, ManifoldQuery, ManifoldSlice, AuraAttach, AuraQuery, AuraUpdate

**NodePayload** (`node.rs:187`, 26 variants): 1:1 with NodeType.

**EdgeKind** (`edge.rs:41`, 7 variants): `DataFlow`, `ControlFlow`, `Derivation`, `Annotation`, `Dispatch`, `Call { from_node, to_node, caller_region }`, `Return { from_node, to_node, return_values: Vec<NodeId> }`

> The SCG is backed by `petgraph::DiGraph` (external crate). **The SCG is NOT acyclic** — it allows cycles (e.g., loops, recursive calls) and handles them via `topological_sort_with_cycles()` using Tarjan's SCC algorithm (`src/scg/src/graph.rs`).

### Verification (IVE)

The IVE verifies 5 invariants: Liveness, Exclusivity, Interpretation, Origin, Cleanup.

**Two `VerificationLevel` enums exist:**
- `pipeline::VerificationLevel` (`src/pipeline.rs:127`, 4 variants): `None`, `Quick`, `Normal` (default), `Exhaustive`
- `ive::VerificationLevel` (`src/ive/src/invariant_aggregator.rs:101`, 3 variants): `Quick`, `Normal` (default), `Exhaustive` — no `None` (the pipeline level `None` short-circuits before IVE is called)

**Current state:** `--verification normal` has false positives on valid programs (especially those using `allocate()`/`free()` with dereference). The test suite uses `--verification none`. A modular verification infrastructure exists in `src/ive/src/modular.rs` (389 LOC, with `IncrementalCache`, `AbstractRegionTracker`, `RegionSummary`, `FunctionSummary`, `verify_function`, `verify_all_functions`) but is not integrated into the main pipeline — no other code calls it.

### Behavioral Descriptors (BD)

BD replaces nominal types with the triple (RepD, CapD, RelD). Implemented in `src/bd/`.

**RepD** (`src/bd/src/repd.rs:191`, 11 variants): `Byte`, `Struct`, `Array`, `Enum`, `Ptr`, `Union`, `Func`, `ManifoldSpatial`, `GestaltSuperposition`, `ConceptRelational`, `Generic`

**Capability** (CapD, `src/bd/src/capd.rs:50`, 17 variants): `Read`, `Write`, `Execute`, `Iterate`, `Send`, `Persist`, `Serialize`, `Deserialize`, `Hash`, `Compare`, `DerivePtr`, `Cast`, `Fork`, `Drop`, `Share`, `Move`, `Pin`

**Relation** (RelD, `src/bd/src/reld.rs:112`, 6 variants): `Temporal(TemporalKind)`, `Containment`, `Dependency(DepKind)`, `Equivalence`, `Security(FlowPolicy)`, `Liveness`

BDs are inferred from SCG structure through iterative fixpoint computation with widening (`src/bd/src/inference.rs`). **Note:** Complex generic inference (M2.3) is deferred — `instantiate_generic` in `src/bd/src/unify.rs` does only shallow substitution.

### CLI Commands (from `src/main.rs`, enum `Commands` at line 172, 10 variants)

| Command | What It Does |
|---------|-------------|
| `vuma build <file>` | Canonical pipeline: parse → SCG → IR → codegen → ELF (aarch64 by default) |
| `vuma emit <isa> <file>` | Direct AST→codegen path, emit to specific ISA (8 ISAs: aarch64, x86_64, riscv64, wasm32, loongarch64, arm32, mips64, ppc64) |
| `vuma run <file>` | Build + execute (tries native first, falls back to QEMU aarch64) |
| `vuma check <file>` | Runs the full compile pipeline (including codegen) with verification forced to `Normal`; discards the binary output |
| `vuma verify <file>` | Run IVE 5-invariant verification |
| `vuma compile <file>` | Compile to relocatable object file (byte-patches `e_type` from ET_EXEC to ET_REL on the direct path) |
| `vuma disasm <file>` | Disassemble a binary |
| `vuma repl` | Interactive REPL (full pipeline: parse → SCG → MSG → IVE → Wasm → multi-ISA; `src/vuma/src/repl.rs`, 2,693 LOC) |
| `vuma lsp` | Language Server Protocol |
| `vuma pkg <cmd>` | Package manager (init, build, add) |

### FFI & Syscalls

`extern "C" { fn write(fd: i64, buf: Address, count: i64) -> i64; }` blocks work on all 10 backends. Each backend has its own syscall stubs (encoded as raw machine code in `build_runtime_syscall_stubs()` on x86_64/x86_32, or inline equivalents elsewhere).

**`SyscallName`** enum (`src/ffi.rs:478`, 19 variants): `Read`, `Write`, `Open`, `Close`, `Exit`, `ExitGroup`, `Mmap`, `Munmap`, `Brk`, `Ioctl`, `Fcntl`, `Getpid`, `Kill`, `Mprotect`, `ClockGettime`, `SchedYield`, `Clone`, `Futex`, `SetTidAddress`

**`__vuma_alloc(size)`** — mmap wrapper on 9/10 backends; on Wasm32 it uses a bump allocator instead (no mmap in Wasm).

**`__vuma_free(addr, size)`** — munmap wrapper on 9/10 backends; no-op on Wasm32.

**`allocate(size)`** — VUMA language builtin. In the canonical pipeline (`bridge_scg_to_codegen`), allocations ≤ 4096 bytes are stack-local (freed on function return); larger allocations use `__vuma_alloc` (heap). In the direct path (`bridge_ast_to_codegen_scg`), all `allocate()` calls are stack-local. Use `__vuma_alloc` for guaranteed cross-function persistence.

**`free(ptr)`** — VUMA language builtin. In the direct path it is a no-op for stack allocations. In the canonical pipeline, `free` always lowers to `__vuma_free` (the pipeline does not track which allocations were stack-local).

### LSP

The LSP server (`src/lsp/mod.rs`, 2,055 LOC) implements 6 capabilities: `textDocumentSync`, `completion`, `hover`, `definition`, `documentSymbol`, `semanticTokens`.

---

## Workspace Structure (11 crates, from `Cargo.toml`)

| Crate | Path | LOC | Tests | Role |
|-------|------|------|-------|------|
| `vuma` (root) | `src/*.rs` | 16,037 | — | Pipeline, CLI, LLM API, LSP, FFI, diagnostics, logging, telemetry |
| `vuma-scg` | `src/scg/` | 19,217 | 191 | SCG core (petgraph-backed graph, nodes, edges, regions, transforms, dominance, liveness) |
| `vuma-parser` | `src/parser/` | 18,807 | 325 | Lexer (141 token kinds), parser, AST, AST→SCG bridge, module resolver, error recovery |
| `vuma-codegen` | `src/codegen/` | 105,070 | 1,061 | IR, 10 backends, regalloc, DWARF v4, ELF emission |
| `vuma-ive` | `src/ive/` | 17,824 | 235 | Inference & Verification Engine (5 invariants, BD solver, modular.rs not integrated) |
| `vuma-bd` | `src/bd/` | 13,193 | 342 | Behavioral Descriptors (RepD, CapD, RelD) + inference + unify + context solver |
| `vuma-core` | `src/vuma/` | 20,365 | 301 | MSG, invariants, regions, derivations, access analysis, security, REPL |
| `vuma-cor` | `src/cor/` | 8,831 | 110 | Continuous Optimization Runtime (partially integrated as `Option<CORuntime>`) |
| `vuma-proof` | `src/proof/` | 9,132 | 102 | Formal proof system (liveness, exclusivity, interpretation, origin, cleanup proofs) |
| `vuma-std` | `src/std/` | 24,541 | 667 | Rust stdlib wrapper (NOT linked to VUMA programs; provides BD-annotated primitives, alloc, collections, io, fmt) |
| `vuma-package` | `src/package/` | 1,182 | 6 | Package manager (manifest, resolver, registry) |
| `vuma-tests` | `src/tests/` | 25,428 | 459 | Integration tests, benchmarks (8 categories) |

**Non-workspace source directories:**
- `src/bin/` — 5 binaries: `compile_dump` (173 LOC, used by test suite), `dump_codegen_scg` (55 LOC), `dump_ir` (35 LOC), `parse_test` (10 LOC), `scg_dump` (22 LOC)
- `src/lsp/` — `mod.rs` (2,055 LOC), LSP server
- `src/bootstrap/` — `vuma_compiler.vuma` (730 LOC, VUMA-in-VUMA lexer proof-of-concept)

**External dependencies** (from `Cargo.toml`): petgraph 0.6, serde 1, serde_json 1, hashbrown 0.14, indexmap 2, smallvec 1, clap 4, chrono 0.4, toml 0.8, tempfile 3, proptest 1, thiserror 1, anyhow 1, log 0.4, env_logger 0.10, colored 2, libc 0.2

**Total:** 205 Rust source files, ~283K lines.

---

## Womb (VUMA-Native Library)

115 `.vuma` files, 64,759 lines. Not auto-imported — programs must inline needed functions.

> **Caveat:** `womb/core.vuma` (197 LOC) is explicitly a design spec, not compilable — its own header states "DESIGN SPEC, NOT COMPILABLE — VUMA compiler CANNOT compile this file." So 114 of 115 files compile on x86_64 with `--verification none`; `core.vuma` does not.

| Directory | Files | LOC | Key Modules |
|-----------|-------|------|-------------|
| `alloc/` | 1 | 55 | arena.vuma (bump allocator on mmap) |
| `codec/` | 1 | 28 | byte_utils.vuma (LE/BE store/load, mem_copy/set/cmp) |
| `collections/` | 4 | 714 | vec.vuma (heap-backed), hashmap.vuma, btree_map.vuma, enum_map.vuma |
| `containers/` | 1 | 627 | containers.vuma (generic container abstractions) |
| `crypto/` | 45 | 26,223 | sha1/sha3/sha384/sha512/sha_variants, aes128/192/256 + modes, hmac, chacha20, poly1305, rsa + oaep/pss, ecdsa (p256/p384), ed25519, x25519, secp256k1, ml_dsa, ml_kem, slh_dsa, falcon, hqc, bignum/bignum2048, blake2/blake3, md5, crc, hkdf, pbkdf2, scrypt, argon2, drbg, salsa20, kdf_cmac_bcrypt, key_agreement, signatures_extra, legacy_ciphers |
| `encoding/` | 3 | 204 | base64, hex, url |
| `env/` | 1 | 175 | cli.vuma (CLI arg parsing) |
| `fs/` | 2 | 304 | file.vuma (raw syscalls), high_level.vuma (read_file, write_file, path ops) |
| `graph/` | 2 | 416 | digraph.vuma (heap-backed, dynamic grow), algorithms.vuma (toposort, cycle detection) |
| `ieee/` | 2 | 1,227 | fp.vuma (floating-point), ieee_frames.vuma |
| `io/` | 1 | 151 | buffered.vuma (BufReader/BufWriter) |
| `lang/` | 15 | 3,553 | full_lexer, full_parser, ir_builder, codegen, elf, tokens, ast, ir, mini_compiler, minicompiler, self_host_test, lexer, parser, **string**, **vuma_compiler** (506 LOC, full VUMA-in-VUMA self-hosting compiler pipeline) |
| `lib/` | 28 | 25,027 | stdlib, stdio, math, time, string, printf, unicode, json, fileio, socket, dns, dns_extra, http, http2, websocket, email (SMTP), app_protocols (MQTT), net_protocols, asn1, x509, pki, auth, jwt, hpack, deflate, compression_extra, event_loop, threading |
| `net/` | 5 | 5,390 | tcp, ssh, quic, tls12, tls13 |
| `string/` | 3 | 468 | string.vuma (minimal (data,len) helpers), utf8.vuma (VStr), string_builder.vuma |
| `core.vuma` (root) | 1 | 197 | Design spec only — NOT compilable |

> Note: There are three `string.vuma` files: `womb/string/string.vuma` (minimal), `womb/lib/string.vuma` (POSIX string.h), `womb/lang/string.vuma` (language-level string utilities). They serve different purposes.

**Known issues:**
- `allocate()` creates stack-local memory in the direct path; in the canonical pipeline, allocations > 4096 bytes use the heap. Use `__vuma_alloc()` for guaranteed heap memory.
- While-loop variable tracking across function calls has a compiler bug.
- The import system works but has limitations with complex module graphs.

---

## Test Suite

5,738 programs with expected exit codes (5,754 total `.vuma` files in `tests/gold_standard/`), across 16 categories:

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
Test runner: `test_results/run_tests.py`
Compile binary: `compile_dump` (uses canonical SCG pipeline, `--verification none`)

---

## Known Limitations

> The first rows are **blocking** — see "Critical Known Issues" at the top of this README for full detail.

| Area | Status | Details |
|------|--------|---------|
| IVE verification | ❌ Broken on canonical idiom | Every `examples/*.vuma` fails `--verification normal`. Top-level `region` declarations are flagged as leaks (`pipeline.rs:5783-5797`). Spec §5.4 static-lifetime inference unimplemented. Test suite uses `--verification none`. |
| Self-hosting | ❌ Bootstrap does not compile | `src/bootstrap/vuma_compiler.vuma` fails SCG→MSG (`access references unknown region RegionId(8)`). It is lexer-only and has a live `src_len_global` bug. `womb/lang/vuma_compiler.vuma` (506 LOC) exists but is in the 6/16 set that doesn't parse. |
| Parser — `else { if … } else { … }` chains | ❌ Unsupported | 6 of 16 `womb/lang/*.vuma` files fail to parse. Parser only supports `else if` (no braces) or `else { block }`. |
| Parser — Womb keywords | ❌ Tokenized, never parsed | `concept`/`gestalt`/`manifold`/`aura` have `TokenKind`, `Item`, and `NodeType` variants but no `parse_*_decl` in `parser.rs`. The Womb data-model layer is a frontend gap. |
| Codegen bridges | ❌ Divergent | Canonical path verifies but emits broken code; `bridge_ast_to_codegen_scg` emits but skips verification. MSG and codegen IR are not connected. |
| Silent statement dropping | ❌ Bug | `Allocate`/`Free`/`Match`/`Sync`/`UnsafeBlock`/`Access` as standalone statements emit zero instructions (`src/main.rs:1656-1657, 1781-1784`). `Cast` loses its target type. Every program leaks. |
| Emitted binary correctness | ❌ Crashes / infinite loops | Top-level `region buf = allocate(1024)` → SIGSEGV on x86_64. `womb/lang/minicompiler.vuma` → infinite loop. Output is not trustworthy. |
| `vuma run` on non-aarch64 | ❌ Broken | Native exec fails (ENOEXEC), qemu-aarch64 fallback not installed by default. No host-arch detection, no `--target` flag. |
| SCG→MSG error swallowing | ⚠️ Quick mode hides errors | `pipeline.rs:4794`: under `Quick`, SCG→MSG errors are dropped and an empty MSG substituted. Use `Normal` to see them. |
| Type checking | ❌ Not implemented | Parser recognizes syntax but doesn't validate types. |
| BD inference (M2.3) | ❌ Deferred | Complex generic inference scenarios. `instantiate_generic` does shallow substitution only. |
| Doubly-linked list verification (M2.4) | ⚠️ Partial | `src/tests/src/dlist.rs` (1,010 LOC) has hand-built tests; not fully verified end-to-end. |
| Concurrent verification | ⚠️ Limited | Single-threaded only. |
| COR runtime | ⚠️ Partial | Not fully integrated end-to-end (`Option<CORuntime>` field in pipeline). |
| Standard library | ⚠️ Partial | `vuma-std` (Rust) not linked to VUMA programs. Womb (VUMA) exists but not auto-imported. |
| While-loop variable tracking | ⚠️ Bug | Loop variables across function calls may not propagate correctly. |
| `map_device()` | ❌ Not a feature | Referenced in example comments only. |
| `volatile` | ❌ Not a feature | Not implemented. |
| BinOp type propagation | ⚠️ Do not attempt | Propagating `ty` to BinOp IR causes ppc64 regression. Keep `ty: None`. |
| `womb/core.vuma` | ❌ Not compilable | Explicitly a design spec; the other 114 womb files compile. |
| CLI ISA coverage | ⚠️ 8 of 10 | `vuma emit`/`vuma compile` accept 8 ISAs (missing RISC-V 32, x86_32). All 10 exist in codegen and are tested via `compile_dump`. |
| Housekeeping | ⚠️ Cleanup needed | `src/codegen/src/lib.rs.tmp` is a stale leftover. `cargo check --workspace` produces 200+ warnings. `src/pipeline.rs` is ~5,800 lines with ~12 unused helper functions from abandoned refactors. |

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
| `docs/language-reference.md` | VUMA syntax |
| `docs/ROADMAP.md` | Milestones and current status |
| `docs/CONTRIBUTING.md` | Build, test, code review process |
| `docs/GLOSSARY.md` | Term definitions |
| `docs/specs/` | Formal specification documents |
