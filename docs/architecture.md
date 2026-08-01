# VUMA Architecture Overview

**Version:** 0.2.0-alpha.10.

**Status:** Reference (IEEE-style). **Audience:** compiler engineers, backend
implementers, verification engineers. **Scope:** end-to-end pipeline from
`.vuma` source to native object code / Wasm, plus the verification discipline
applied between front-end and back-end. **Cross-references:**
[Language Reference](./language-reference.md) ·
[Backend Documentation](./backends.md) ·
[Testing Infrastructure](./testing.md) ·
[Pipeline](./pipeline.md) ·
[Building Guide](./building.md) ·
[Caveats](./caveats.md) ·
[PMT Iris Spec](./pmt-iris-spec.md) ·
[PMT Formal Spec](./pmt-formal-spec.md).

---

## 1. System Architecture

VUMA compiles a statically-typed source language to native object code or
WebAssembly through a 10-stage pipeline. Each stage is described below.

### 1.1 Parse (lexing)

`src/parser/src/lexer.rs` tokenizes UTF-8 source into a token stream covering
keywords, identifiers, integer/float literals, string literals, operators,
delimiters, and `//`/`/* */` comments. Tokens carry source spans for
diagnostics. The lexer is hand-written (no `nom` / `logos` dependency) and
serves both the compiler and the LSP.

The sole function-declaration keyword is **`transform`**. The
legacy `fn` keyword was removed from the keyword table and now lexes as a
plain `Ident`; `TokenKind::Fn` is retained only for backward-compatibility
references in `parser.rs` / `is_name_keyword` / `Display` and is never
produced by the lexer.

### 1.2 AST

`src/parser/src/` produces a Pratt-parsed AST with declarations
(`transform`, `struct`, `enum`, `layout`, `extern`), statements (`let`,
`assign`, `if`/`else`, `while`, `for`, `match`, `return`, `break`,
`continue`), expressions (arithmetic, logical, comparison, cast, channel,
struct/enum construction, field access, indexing), and attributes
(`#[borrow]`, `#[secret]`, `#[inline]`, `#[no_mangle]`, `#[link_section]`,
effect purity annotations). The AST is the only IR that preserves source-
level names and spans.

Source-level contracts (`requires` / `ensures`) and `prove { require …; <body> }`
proof-obligation blocks are parsed on `transform` declarations and lifted by
the pipeline into the IVE-consumable `FnContract` / `ProveBlockObligation`
structs (see §5).

### 1.3 SCG — Semantic Code Graph

`src/scg/` lifts the AST into a typed, control-flow-annotated graph. The SCG
performs name resolution, type checking, monomorphization of generics
(`src/codegen/src/monomorphize.rs`), effect inference
(`src/codegen/src/effects.rs`), escape analysis
(`src/codegen/src/escape_analysis.rs`), and alias analysis
(`src/codegen/src/alias_analysis.rs`). The SCG is the canonical input to the
verifier and to codegen; raw AST nodes are never seen past this stage.

### 1.4 IVE — Invariant Verification Engine

`src/ive/` (25 modules) runs the verification discipline. IVE consumes the
SCG plus registered `PmtLayoutSpec`s, source-level `FnContract`s, and
`ProveBlockObligation`s, and emits a structured `VerificationReport` whose
summary line includes the contract **discharge rate**:

```
passed=<N> failed=<N> unverified=<N> total=<N> discharge_rate=<M>%
```

The `discharge_rate` is the fraction of proof obligations that the IVE
discharged (via Z3 or trivial-true elision) over the total obligations
collected from the program. The exact wire format is in
`src/bin/compile_dump.rs:228` and `src/ive/src/result.rs`.

The IVE has three subsystems, all hand-written in Rust (no Lean FFI — see
§6.1):

- **PMT state verifiers** — `state_read`, `state_write`, `state_transform`
  (`src/ive/src/state_read.rs`, `src/ive/src/state_write.rs`,
  `src/ive/src/state_transform.rs`). These check that every
  `state.field` read/write is type-correct against the registered
  `PmtLayoutSpec` and that `State<T>`-parameterized transforms preserve the
  layout's invariants.
- **Session-type verifier** (`src/ive/src/session_type.rs`) — checks that
  every `channel_open` / `channel_send` / `channel_recv` / `channel_close`
  on a given channel vreg follows the declared session type. Each
  `SessionEvent` carries the **real vreg** of the channel handle (not a
  hardcoded `0`), so multi-channel programs are checked independently per
  channel. Use-after-close, double-close, and send/recv-on-closed-channel
  are unconditional HARD-FAILs.
- **Information-flow verifier** (`src/ive/src/information_flow.rs`) — checks
  the Denning lattice `Public ⊑ Internal ⊑ Secret ⊑ TopSecret`. Each vreg's
  label is computed from the source-level `#[secret]` annotation set
  collected by `pipeline.rs::collect_secret_vars` (a `#[secret] let k = …`
  produces a vreg named `"k"` whose `Store`/`ChannelSend` events are labeled
  `Secret`). The legacy "hardcode `Public` for every vreg" behaviour was
  removed — see the historical note in `information_flow.rs:468-496`.
- **Contract discharge** — `requires` / `ensures` / `require` clauses are
  translated to SMT-LIB2 and discharged by **Z3** (see §5.2). Trivially-
  `true` clauses are elided without Z3. Non-dischargeable clauses produce a
  WARNING (not a hard `Violated`); the `discharge_rate` reflects the
  fraction discharged.

The IVE also produces a **`LinearityReport`**
(`src/ive/src/verification.rs:1602-1655`) enumerating the bare vregs
linearly consumed by `StateTransform` / `ForeignConsume` nodes in the
verified SCG. The report is threaded through `run_optimizations` and
consumed by the optimizer to perform *provenance-directed* (non-aliasing)
dead-code elimination — channel-close, arena-free, and capability-revoke
vregs become eligible for DCE once they have been observed by the IVE.

### 1.5 IR Lowering

`src/codegen/src/scg_to_ir.rs` lowers the SCG to the backend-neutral SSA-like
IR defined in `src/codegen/src/ir.rs`. The IR uses virtual registers
(`VReg` / `IRValue::Register(n)`), typed basic blocks, and a fixed set of
`IRInstr` variants (`BinOp`, `UnOp`, `Load`, `Store`, `Branch`, `Call`,
`Syscall`, `Cast`, `ICmp`, `FCmp`, `CondBranch`, `Return`, `Phi`-shaped
joins via block params). Each `IRFunction` populates a `vregs` map from
vreg ID to the source-level `let`/parameter name; this map is the lookup
the information-flow verifier uses to map `#[secret]` annotations to vregs.

### 1.6 IPC Lowering

`src/codegen/src/ipc_lowering.rs` is a single shared pass over the IR for
**all 19 backends**. It expands the IPC builtins (`channel_open` / `send` /
`recv` / `close` / `try_recv`, `spawn_worker` / `wait_worker`,
`shared_memory_*`, `checkpoint_*`, `aead_seal` / `open`, `sandbox_apply`,
`capability_grant` / `delegate`, `circuit_breaker_*`, `hot_swap_*`,
`stark_prove` / `verify`, `formal_verify`, …) into IR sequences using
`Syscall{nr}` (asm-generic numbers) and block-splitting via the `Expansion`
struct. On `wasm32` only, the `wasm32_fork_emulation_pass` runs after
`lower_ipc_builtins` to rewrite the child branch's `Return` into a `Store`
at fixed address `4096` followed by `Jump` to the parent's post-fork block,
so both branches run sequentially in-process.

The IPC lowering implements VUMA's **two-pipe channel architecture** (see
§6) — every `channel_open` allocates a 16-byte handle `{read_fd1, write_fd1,
read_fd2, write_fd2}` and registers it in a per-function channel-handle
registry. `expand_spawn_worker` walks the registry after `clone()` and swaps
the read/write ends of both pipes on every registered handle so the child
reads from the parent→child pipe and writes to the child→parent pipe.

### 1.7 Mid-end Optimizations (`opt`)

`src/codegen/src/opt.rs` runs the optimization pipeline: constant
propagation/folding, dead-code elimination, common-subexpression
elimination, loop unrolling (`loop_unroll.rs`), e-graph rewriting
(`egraph.rs`), identical-function merging (ICF), whole-program DCE,
profile-guided optimization (PGO), vectorization (`vectorize.rs`), and
`materialize_f32_immediates` (a load-bearing pass that must run after
folding and before codegen to avoid f32-bit-immediate corruption on
x86_64). The LinearityReport from IVE (§1.4) is consumed here to drive
provenance-directed DCE.

### 1.8 Register Allocation / ISel

As of v0.2.0-alpha.10, **18 of 19 backends use full register-based
emission** — there are no stack-slot fallbacks on the default code path.
The full register-allocation pipeline (live-range computation → linear
scan → post-allocation conflict resolution → per-backend emission) is
documented in §7 below and pictured in the [register allocation
pipeline diagram](../README.md#register-allocation-pipeline) in
`README.md`. The two allocators in `src/codegen/src/regalloc.rs` are:

- **`TargetAgnosticRegAlloc`** (`regalloc.rs:2966`, `new` at `:2981`) —
  the production allocator on every register-based backend except
  `aarch64`. A `TargetDesc`-driven linear-scan allocator: the per-ISA
  register file is supplied at construction time by
  `target_desc::TargetDescRegistry::get(<isa>)`. Implements
  live-interval computation (`LiveRangeComputer`), boundary-safe overlap
  detection, spill-weighted eviction, copy coalescing, and — critically —
  the post-allocation conflict-resolution pass
  `resolve_register_reuse_conflicts` (`regalloc.rs:2836`, see §7.3) that
  eliminates the register-reuse hazard that previously forced
  stack-slot fallbacks on syscall-heavy functions.
- **`LinearScanAllocator`** (`regalloc.rs:1284`, `new` at `:1323`) — the
  older AArch64-specific linear-scan allocator with hardcoded caller /
callee-saved GPR+SIMD lists. Used only by the `aarch64` backend (which
  predates the directory-style `reg_isel.rs` pattern and has not yet
  been ported to `TargetAgnosticRegAlloc`). `aarch64_be` inherits this
  via one-line delegation.

Both allocators produce a `RegAllocResult` consumed by the per-backend
emitter (either `reg_isel::emit_function_regalloc_full` for the 14
directory-style backends, or `Emitter::emit_function_regalloc` at
`emit.rs:1056` for `aarch64`). The emitter writes real vreg→preg machine
code, performs prologue/epilogue with callee-saved save/restore, inserts
spill/reload code at the positions in `RegAllocResult::spill_code`, and
resolves branch fixups.

The 4 byte-swap wrapper backends (`aarch64_be`, `armeb`, `mips64be`,
`ppc64le`) inherit their parent's allocation result verbatim via
one-line `allocate_registers` delegation.

`wasm32` has no register allocator at all — WebAssembly is a stack
machine, so vregs map to Wasm `local`s and the IR is lowered directly
to structured stack-machine bytecode via `wasm32/mod.rs::lower_function`
(this is the correct architecture for the target, not a fallback).

**Stack-slot ISel is NOT the production path.** The legacy stack-slot
emitters (`<isa>/stack_slot_isel.rs` where present, or inlined paths
inside `alpha/mod.rs`, `hppa/mod.rs`, `m68k/mod.rs`) survive only as the
**`contains_fork` opt-out**: functions whose IR contains a `clone`/
`fork` syscall (Linux generic nrs 220/221) fall back to the stack-slot
path because the child process's divergent register state is
incompatible with the register-based prologue/epilogue. This is a
**correctness requirement**, not a fallback for register pressure or
unimplemented IR ops. See §7.4 and [caveats.md §2.1](./caveats.md#21-contains_fork-opt-out-clonefork-detection).

The `loongarch64/reg_alloc_isel.rs` file (1.6 K LOC) on disk is **dead
code** — the module declaration is commented out at
`loongarch64/mod.rs` and the production `allocate_registers` calls
`reg_isel::emit_function_regalloc_full` via `try_real_regalloc`. The
file is retained for historical reference; it is not compiled.

### 1.9 Backend Emission

Per-backend modules under `src/codegen/src/` (see [backends.md](./backends.md)
for the matrix) consume the post-`opt`, post-`regalloc` IR and emit
machine instructions as raw byte vectors. Each register-based backend
provides an `emit_function_regalloc_full(func, &alloc)` entry point in its
`reg_isel.rs` module that walks the IR, substitutes the
allocator-assigned physical registers into the per-ISA `Instruction::encode()`,
and emits prologue / argument shuffle / body / spill-reload / epilogue /
branch-fixup / relocation records. `aarch64` is the one exception: it uses
the shared `Emitter::emit_function_regalloc` in `src/codegen/src/emit.rs`
instead of a per-backend `reg_isel.rs` (historical — `aarch64` was the
first register-based backend; the directory pattern was extracted from
it later).

Backends share `src/codegen/src/emit.rs` for ELF section construction and
`marshal.rs` for relocation records. Every backend emits the three
runtime trap stubs (`__arena_overflow`, `__oob_trap`, `__uaf_trap`) with
exit codes `1` / `134` / `135` respectively, matching the Lean
`TrapCode.to_exit` mapping.

All ISA encodings have been **verified against the official ISA manuals** —
see [backends.md](./backends.md) §9 for the per-ISA audit (LoongArch FP
condition codes, Power ISA XO field corrections, RISC-V OPC_NMADD, Alpha
CMPULE→CMPULT emulation under QEMU). Each verified encoding carries a
citation to the manual section in its inline comment.

### 1.10 Object Emission (ELF / Wasm)

`src/codegen/src/emit.rs` and the per-backend `<isa>/mod.rs::encode_program`
implementations write relocatable ELF objects (`ET_REL`) for the 18 native
targets (everything except `wasm32`), including per-arch `e_machine`,
endianness, and `e_flags`. The 4 big-endian variants
(`aarch64_be`, `armeb`, `mips64be`, plus natively-BE `ppc64`/`s390x`/
`sparc64`/`hppa`/`m68k`) emit BE ELF for `qemu-*-be` testing. The wasm32
backend (`src/codegen/src/wasm32/`) emits a `.wasm` module directly using a
trampoline-loop control-flow pattern (`loop $trampoline ... br_table`)
because Wasm requires structured control flow and VUMA's CFG is arbitrary.

---

## 2. Crates

The workspace (`Cargo.toml`) declares **seven member crates** plus an
in-tree LSP module and a package manager. LOC figures are exact counts of
`.rs` lines under each crate directory at HEAD.

| Crate | Path | Responsibility |
|------------------|-------------------|-------------------------------------------------------------|
| `vuma-parser` | `src/parser/` | Lexer, Pratt parser, AST, attribute parsing. |
| `vuma-scg` | `src/scg/` | Semantic Code Graph: name resolution, type checking, effects. |
| `vuma-ive` | `src/ive/` | Invariant Verification Engine (25 modules). **Hard-depends on Z3** (`z3 = "0.20"`) for contract discharge. |
| `vuma-codegen` | `src/codegen/` | IR, opt, regalloc, 19 backends, ELF/Wasm emission, IPC lowering, runtime stubs. |
| `vuma-bd` | `src/bd/` | Behavioral Descriptors: 3-axis (repd/capd/reld) value model. |
| `vuma-core` | `src/vuma/` + `src/*.rs` | Pipeline driver, FFI, diagnostics, telemetry, LSP, llm_api. |
| `vuma-package` | `src/package/` | Package manager, manifest, dependency resolution. |
| `vuma-lsp` (mod) | `src/vuma/lsp/` | Language Server Protocol front-end (in-tree module of core). |

`vuma-tests` (`src/tests/`) ships ~30 K LOC of integration tests, property
tests, and bootstrap suites; it is excluded from the production LOC total.

The Z3 dependency is **hard** — `vuma-ive/Cargo.toml` declares `z3 = "0.20"`
with the comment *"The 'V' in VUMA depends on Z3."* Hosts must install
`libz3-dev` (Debian/Ubuntu), `z3` (Homebrew), or `z3` (Arch) before
building VUMA. There is no `--no-z3` fallback: the contract-discharge pass
requires a working Z3 linkage at build time.

---

## 3. Backend Matrix

19 backends, each a module under `src/codegen/src/`. As of v0.2.0-alpha.10
**18 of 19 backends have full register-based emission** — 14 native backends
with a per-backend `<isa>/reg_isel.rs` module (including `aarch64`, which
gained its own `reg_isel.rs` in W7-impl) plus 4 byte-swap wrappers
(`aarch64_be`, `armeb`, `mips64be`, `ppc64le`) that are thin (200–530 LOC
each) wrappers delegating to a parent backend's `allocate_registers` via
one-line delegation and rewriting instruction words at the encoding
boundary to produce BE/LE ELF for `qemu-*` variants. `wasm32` uses
structured stack-machine emission (the correct architecture for a stack
machine, not a fallback).

**Test status:** all 19 backends pass the curated 30-test matrix
(`scripts/vuma_test_matrix_19backends.sh`) — every register-based
backend emits register-to-register machine code for every IR instruction
in every curated test, with no stack-slot fallback. The 4 byte-swap
wrappers inherit their parent's emission byte-for-byte and only differ in
ELF endianness, so they pass whenever the parent passes. See
[testing.md](./testing.md) for the full gold-standard harness.

| # | Backend | File / dir | Emission | Regalloc | Key approach |
|--:|---------|------------|----------|----------|--------------|
|  1 | `aarch64`     | `aarch64/{mod,reg_isel}.rs` + `backend.rs` + `emit.rs` | register-based | `LinearScanAllocator` (`regalloc.rs:1284`) | Reference backend; since W7-impl, uses `aarch64::reg_isel::emit_function_regalloc_full` as its default emission path, falling back to `LinearScanAllocator` + `Emitter::emit_function_regalloc` (`emit.rs:1056`) only on encoding failure. |
|  2 | `aarch64_be`  | `aarch64_be.rs`                                  | register-based (wrap) | inherits AArch64 | Forwards LE instr. bytes (ARM ARM D6.1.3); only ELF header swapped. |
|  3 | `x86_64`      | `x86_64/{mod,reg_isel,disasm,stack_slot_isel}.rs` | register-based | `TargetAgnosticRegAlloc` | `materialize_f32_immediates` consumer (load-bearing pass ordering). R11 not allocatable. |
|  4 | `x86_32`      | `x86_32/{mod,reg_isel,disasm,stack_slot_isel}.rs` | register-based | `TargetAgnosticRegAlloc` | 32-bit x86; args on stack; `int 0x80` syscall. |
|  5 | `riscv64`     | `riscv64/{mod,reg_isel}.rs`                      | register-based | `TargetAgnosticRegAlloc` | T5/T6 not allocatable (scratch). |
|  6 | `riscv32`     | `riscv32/{mod,reg_isel}.rs`                      | register-based | `TargetAgnosticRegAlloc` | QEMU run requires `-cpu max` (D extension). |
|  7 | `loongarch64` | `loongarch64/{mod,reg_isel,disasm,stack_slot_isel,reg_alloc_isel†}.rs` | register-based | `TargetAgnosticRegAlloc` | FP compare condition codes fixed per LoongArch Vol 1 §3.2.2.1. T7/T8 scratch; `maskeqz`/`masknez` conditional select. |
|  8 | `arm32`       | `arm32/{mod,reg_isel,disasm}.rs`                 | register-based | `TargetAgnosticRegAlloc` | Conditional execution (`MOVcc`); no hardware divide; R12 scratch. |
|  9 | `armeb`       | `armeb.rs`                                       | register-based (wrap) | inherits Arm32 | BE32 word-swap on every 4-byte instr. |
| 10 | `mips64`      | `mips64/{mod,reg_isel,disasm}.rs`                | register-based | `TargetAgnosticRegAlloc` | Emits LE ELF; run via `qemu-mips64el-static`. Branch delay slots. |
| 11 | `mips64be`    | `mips64be.rs`                                    | register-based (wrap) | inherits MIPS64 | Word-swaps each 4-byte instr. word LE→BE. |
| 12 | `ppc64`       | `ppc64/{mod,reg_isel,disasm}.rs`                 | register-based | `TargetAgnosticRegAlloc` | Native BE (ELFv2); 6 Power ISA XO bugs fixed. R11 scratch; `isel` conditional move. |
| 13 | `ppc64le`     | `ppc64le.rs`                                     | register-based (wrap) | inherits PPC64 | Reuses ppc64 encoders; only ELF endianness flipped. |
| 14 | `sparc64`     | `sparc64/{mod,reg_isel}.rs`                      | register-based | `TargetAgnosticRegAlloc` | Register windows (`SAVE`/`RESTORE`); branch delay slots; `SETHI` for upper immediates. |
| 15 | `s390x`       | `s390x/{mod,reg_isel}.rs`                        | register-based | `TargetAgnosticRegAlloc` | R0 scratch; `SVC 0` syscall; 5 arg regs (R2–R6). |
| 16 | `m68k`        | `m68k/{mod,reg_isel}.rs`                         | register-based | `TargetAgnosticRegAlloc` | D/A register separation (only D0–D7 allocatable); 2-operand; variable-length encoding. |
| 17 | `alpha`       | `alpha/{mod,reg_isel}.rs`                        | register-based | `TargetAgnosticRegAlloc` | R27 scratch; 3-operand; `callsys`; branch PC+4 bias. QEMU 10.0-alpha rejects CMPULE (function 0x3D); workaround via CMPULT. |
| 18 | `hppa`        | `hppa/{mod,reg_isel}.rs`                         | register-based | `TargetAgnosticRegAlloc` | `GATE` for syscalls (NOP after); 4 arg regs (R26–R23 reversed); `BV` return. |
| 19 | `wasm32`      | `wasm32/{mod,disasm}.rs`                         | stack-machine  | (none — vregs → Wasm locals) | Trampoline loop + ring-buffer channels + in-process fork emulation. |

† `loongarch64/reg_alloc_isel.rs` is **dead code** — the module declaration
is commented out at `loongarch64/mod.rs` and the production
`allocate_registers` calls `reg_isel::emit_function_regalloc_full`. The
file is retained for historical reference; it is not compiled.

Per-backend ABI tables, ISel notes, encoding-audit details, and QEMU
quirks are in [backends.md](./backends.md). The register-allocation
pipeline diagram lives in [README.md §3](../README.md#register-allocation-pipeline).

---

## 4. Compilation Pipeline

End-to-end flow from source to binary, with the responsible crate and the
output artifact of each stage:

```
.vuma source
 │ vuma-parser (lexer + Pratt parser; `transform` is the only fn keyword)
 ▼
AST ─── src/parser/
 │ vuma-scg (name resolution, type check, monomorphize, effects, escape, alias)
 ▼
SCG ─── src/scg/
 │ vuma-ive (PMT state verifiers + session-type + info-flow + Z3 contract
 │          discharge; produces VerificationReport + LinearityReport)
 ▼
Verified SCG + VerificationReport + LinearityReport ─── src/ive/
 │ vuma-codegen: scg_to_ir
 ▼
SSA-like IR (VReg, typed blocks) ─── src/codegen/src/scg_to_ir.rs
 │ vuma-codegen: ipc_lowering (shared by all backends)
 │   ├─ channel_open → 16-byte two-pipe handle + registry insertion
 │   ├─ spawn_worker → clone() + handle-registry swap loop
 │   └─ wasm32_fork_emulation_pass (wasm32 only)
 ▼
IR with Syscalls / channel ops lowered
 │ vuma-codegen: opt (CSE, DCE, loop_unroll, egraph, ICF, PGO, vectorize,
 │   materialize_f32_immediates, linearity-driven DCE)
 ▼
Optimized IR
 │ vuma-codegen: regalloc (TargetAgnosticRegAlloc on 14 backends,
 │   LinearScanAllocator on aarch64; wasm32 has no registers)
 ▼
Machine instruction stream (Vec<u8>)
 │ vuma-codegen: emit.rs (ELF) | wasm32/mod.rs (Wasm)
 ▼
ET_REL ELF object | .wasm module
 │ system linker (ld) | wasmtime runtime
 ▼
Native executable | Wasm execution
```

Three side inputs feed the pipeline:

- The **PMT layout registry** (typed `PmtLayoutSpec`s registered with IVE
  describing the state buffer's layouts).
- The **`#[secret]` annotation set** (`pipeline.rs::collect_secret_vars`),
  consumed by the information-flow verifier to label vregs `Secret`.
- The **source-level contracts** (`requires` / `ensures` on `transform`
  declarations and `prove { require …; <body> }` blocks), translated to
  `FnContract` / `ProveBlockObligation` and discharged by Z3.

---

## 5. Verification Pipeline

The verifier is invoked between SCG construction and IR lowering. Its
inputs are the SCG, the registered PMT layout specs, the `#[secret]`
annotation set, and the source-level contracts. Its output is a
`VerificationReport` (consumed by `pipeline.rs` to gate code emission) and
a `LinearityReport` (consumed by `run_optimizations`).

### 5.1 What IVE checks

- **PMT state-read / state-write / state-transform** — every `state.field`
  read/write is type-correct against the registered `PmtLayoutSpec` and the
  slot is in a live state region; transforms that read and write the same
  fields preserve the layout's invariants.
- **Session-type discipline** — every `channel_open` / `channel_send` /
  `channel_recv` / `channel_close` on a given channel vreg follows the
  declared session type. Each `SessionEvent` carries the **real vreg** of
  the channel handle (not a hardcoded `0`), so multi-channel programs are
  checked independently per channel. Use-after-close, double-close, and
  send/recv-on-closed-channel are unconditional HARD-FAILs.
- **Information flow** — the Denning lattice `Public ⊑ Internal ⊑ Secret ⊑
  TopSecret`. Vreg labels are computed from `#[secret]` annotations
  (collected by `pipeline.rs::collect_secret_vars`); the legacy "hardcode
  `Public` for every vreg" behaviour was removed.
- **Linear-channel discipline** — `LinearityReport` enumerates vregs
  linearly consumed by `StateTransform` / `ForeignConsume` nodes; this
  drives provenance-directed DCE in `opt`.
- **Contract discharge** — `requires` / `ensures` / `require` clauses are
  translated to SMT-LIB2 (`ContractClause::smt_lib2`) and discharged by
  Z3. The `discharge_rate` in the verification summary reflects the
  fraction of obligations discharged (via Z3 or trivial-true elision).

### 5.2 Z3 contract discharge

Contract clauses (`requires`, `ensures`, `require`) are translated to
SMT-LIB2 strings by the pipeline (`collect_contracts` /
`collect_prove_blocks` helpers) and stored in
`ContractClause::smt_lib2`. The IVE's `discharge_contracts_and_prove_blocks`
pass (in `VerificationEngine::verify_pmt`) feeds each non-empty SMT-LIB2
string to Z3 via the `z3` Rust crate (v0.20):

- A clause that Z3 reports `unsat` (i.e. the negation of the clause is
  unsatisfiable) is **discharged** — the obligation is proven.
- A clause that Z3 reports `sat` or `unknown` is recorded as
  `unverified` and the `discharge_rate` is reduced accordingly.
- A clause that is a literal `true` at the AST level is elided without
  invoking Z3 (`ContractClause::trivially_true`).
- A clause whose AST → SMT-LIB2 translation failed (empty `smt_lib2`
  field) is recorded as `unverified`.

Non-dischargeable clauses currently produce a **WARNING**, not a hard
`Violated`, so that consumption is wired without rejecting every program
whose contracts the IVE cannot yet prove. The WARNING + TODO marker is
the explicit signal that hard-gate discharge is pending. The
`discharge_rate=N%` summary line in `compile_dump` output makes the
fraction discharged visible per compilation.

### 5.3 PMT clarification

In this codebase **PMT = "Programs as Memory Transformations"**, not
"Persistent Memory Transaction." There is no transaction, no rollback, no
durability machinery. Every program is treated as a typed
state-transformation on a single mmap'd arena (`___pmt_buffer`); state
lives in layouts registered with the verifier; `arena_alloc` returns a
state-typed pointer into that buffer.

The canonical source-code definition lives on the `InvariantKind::Pmt` doc
comment (`src/ive/src/invariant_aggregator.rs`), which carries an explicit
"What 'PMT' means (and does NOT mean)" section enumerating the three
missing properties (no persistence, no rollback, no durability).
`VerificationLevel::Pmt` repeats the disambiguation in shorter form. See
also [caveats.md](./caveats.md) and the Glossary below.

### 5.4 Removed CLI flags

VUMA 2.0 removed three CLI flags that previously gated memory-safety
analysis. Each is now a hard error with a migration message:

- **`--safe`** — runtime bounds checks are **always on** (memory-safety
  analysis is mandatory). The bounds-check injector
  (`codegen/src/memory_safety.rs::inject_bounds_check_ir`) runs
  unconditionally on every compilation; the `safe_mode` selector that
  used to gate it is gone. See `src/main.rs:738-746`.
- **`--repl`** — use the `vuma repl` subcommand instead. See
  `src/main.rs:731-737`.
- **`--no-memory-safety`** — memory-safety analysis is mandatory. See
  `src/main.rs:747-753`.

The `--strict-ive` flag is retained but is now a no-op for the
linear-channel discipline (which is an unconditional HARD-FAIL); it still
promotes `bv_verify` e-graph soundness warnings to HARD-FAIL.

---

## 6. Two-Pipe IPC Architecture

VUMA's channel abstraction is built on a **two-pipe** pattern: every
`channel_open` creates *two* OS pipes and stores all four file descriptors
in a single 16-byte handle buffer. This is implemented in
`src/codegen/src/ipc_lowering.rs` (`expand_channel_open`,
`expand_spawn_worker`).

### 6.1 Handle layout

```
offset  field        pipe          direction
──────  ───────────  ────────────  ─────────────────────────
  0     read_fd1     pipe1         parent → child  (child reads)
  4     write_fd1    pipe1         parent → child  (parent writes)
  8     read_fd2     pipe2         child → parent  (parent reads)
 12     write_fd2    pipe2         child → parent  (child writes)
```

`channel_send(ch, msg)` on the parent writes to `write_fd1`; the child
reads from `read_fd1`. `channel_recv(ch)` on the parent reads from
`read_fd2`; the child writes to `write_fd2`. Both pipes are bidirectional
at the OS level, but the discipline above is what the session-type
verifier (§1.4) enforces.

### 6.2 Per-function channel-handle registry

Each function that contains a `channel_open` allocates a per-function
channel-handle registry at function entry:

```
[  0.. 80]  10 channel-handle pointers (8 bytes each)
[ 80.. 84]  I32 channel_count (current number of registered handles)
```

`expand_channel_open` stores each newly-created 16-byte handle pointer at
index `channel_count`, then increments `channel_count`.

### 6.3 Fork-time handle swap

`expand_spawn_worker` (non-wasm32) emits the `clone()` syscall, then —
**in the child branch only** — iterates 0..10 and, for each valid index
`< channel_count`, swaps the handle's `[0↔8]` and `[4↔12]` fd pairs. After
the swap the child's `read_fd1` is what the parent called `write_fd1`,
and the child's `write_fd2` is what the parent called `read_fd2`. The
unrolled loop is branchless (uses `Select` guarded by an `i < count`
predicate) because `expand_spawn_worker` returns `Vec<IRInstr>` (flat),
not `Expansion` (block-supporting).

The single-pipe swap (swap `[0↔4]` only) was the original design and was
sufficient for single-channel ping-pong. It broke multi-channel tests
(e.g. `ping_pong`, `session_types`) because every channel's child-side
handle ended up reading from the same pipe. The two-pipe-plus-registry
design fixed this; see the inline comment at
`ipc_lowering.rs:656-664` for the full history.

### 6.4 wasm32 fork emulation

`vuma_fork` cannot `os.fork` on wasm32 because wasmtime runs background
threads that break the child's state. Instead, `wasm32_fork_emulation_pass`
(`ipc_lowering.rs:232`) rewrites the child branch's `Return` into
`Store(exit_val, 4096); Jump(parent_post_block)` and rewrites
`wait_worker` to `Load(4096)`. The wasm32 child-branch code is dead in
the emitted binary — the parent and child run sequentially in-process
with **no isolation**. The runner (`scripts/wasm32_runner.py`) provides
host-side `fdio` functions backed by a ring buffer in host memory because
wasm32 has no `pipe2` syscall.

---

## 7. Register Allocation

VUMA ships two real linear-scan register allocators in
`src/codegen/src/regalloc.rs`. As of v0.2.0-alpha.10 every register-based
backend (18 of 19) uses one of them as its **production code-emission
path** — there is no stack-slot fallback on the default code path. The
pipeline (live-range computation → linear scan → post-allocation conflict
resolution → per-backend emission) is pictured in [README.md §3 —
Register allocation
pipeline](../README.md#register-allocation-pipeline).

```
IRFunction
    │
    ▼
LiveRangeComputer::compute()     ← global position numbering (pos += 2 per instr + terminator)
    │
    ├── intervals: Vec<LiveInterval>   (start, end, use_positions, def_positions, crosses_call)
    ├── call_positions: BTreeSet<u32>  (Call + Syscall positions)
    └── coalesced_map                 (copy-related vreg merging)
    │
    ▼
TargetAgnosticRegAlloc::allocate_intervals()   ← linear-scan
    │
    ├── Sort intervals by start position (longer first at same start)
    ├── For each interval:
    │   ├── Expire old intervals (return registers to caller/callee pools)
    │   ├── Try alloc: caller-saved first, callee-saved if crosses_call
    │   └── If no free reg: spill_or_evict (lowest weight per length)
    │
    ▼
resolve_register_reuse_conflicts()   ← post-allocation verification (see §7.3)
    │
    ▼
RegAllocResult
    ├── vreg_to_preg: HashMap<vreg, PhysicalReg>
    ├── spill_slots: HashMap<vreg, GenericSpillSlot>
    ├── spill_code: BTreeMap<pos, Vec<SpillCode>>
    ├── coalesced_map
    └── total_spill_slots
    │
    ▼
reg_isel::emit_function_regalloc_full(func, &alloc)   ← per-backend emission
    │
    ├── Prologue (SAVE/push/stmg/link — ISA-specific)
    ├── Argument shuffle (ABI arg regs → allocator-assigned regs)
    ├── Body: per-IR-instruction emission using Instruction::encode()
    ├── Spill/reload insertion at positions from alloc.spill_code
    ├── Epilogue at EVERY Return path (restore SP from FP, pop callee-saved, ret)
    ├── Branch fixup resolution (patch rel32/rel21/rel16 displacements)
    └── Re-slice AllocatedInstruction.encoded from patched all_code
```

### 7.1 `LinearScanAllocator` (AArch64 only)

A real linear-scan allocator (`regalloc.rs:1284`, `new` at `:1323`) using
the full AArch64 register set (caller-saved GPRs X9–X15, X16–X18, X8;
SIMD V0–V31). Live-interval computation, boundary-safe overlap
detection (`liveness_interference_from`), spill-weighted eviction
(`spill_weight_with_pressure`), and copy coalescing
(`coalesce_copies_post_alloc`). Produces a `RegAllocResult` consumed by
`Emitter::emit_function_regalloc` (`emit.rs:1056`), which writes real
vreg→preg machine code with callee-saved prologue/epilogue and spill /
reload insertion. `aarch64` is the only backend that uses this
allocator — every other register-based backend uses
`TargetAgnosticRegAlloc`. The `aarch64_be` wrapper inherits the
allocation result verbatim via one-line `allocate_registers` delegation.
The `VUMA_REAL_REGALLOC_AARCH64` env var (default **ON**) gates the
register-based path; setting it to `0` falls back to the stack-slot
emitter for debugging.

### 7.2 `TargetAgnosticRegAlloc` (the other 14 register-based backends)

A `TargetDesc`-driven linear-scan allocator (`regalloc.rs:2966`, `new`
at `:2919`) that takes the per-ISA register file from
`target_desc::TargetDescRegistry::get(<isa>)`. The allocator contains
no ISA-specific constants — adding a new backend requires only a new
`TargetDesc` entry, no allocator changes. Used by:

- `x86_64`, `x86_32`, `arm32`, `riscv64`, `riscv32`, `mips64`,
  `ppc64`, `loongarch64`, `sparc64`, `s390x`, `m68k`, `alpha`,
  `hppa` — each via its own `try_real_regalloc` helper in
  `<isa>/mod.rs` and the per-backend `reg_isel::emit_function_regalloc_full`.
- `aarch64_be` inherits `aarch64`'s allocation result (which uses
  `LinearScanAllocator`, not `TargetAgnosticRegAlloc`).
- `armeb` inherits `arm32`'s allocation result.
- `mips64be` inherits `mips64`'s allocation result.
- `ppc64le` inherits `ppc64`'s allocation result.

The four wrapper backends inherit the parent's `RegAllocResult`
byte-for-byte and only differ in ELF endianness (see
[backends.md §7](./backends.md#7-big-endian-backends)).

Each backend's `allocate_registers` follows the same dispatch pattern:

```rust
let real_regalloc = env::var("VUMA_REAL_REGALLOC_<ISA>")
    .map(|v| v != "0").unwrap_or(true);              // default ON
let contains_fork = /* see §7.4 */;
if real_regalloc && !contains_fork {
    if let Some(ar) = try_real_regalloc(func) {
        if let Ok(full) = reg_isel::emit_function_regalloc_full(func, &ar) {
            return Ok(full);
        }
    }
    // fall through to stack-slot only on allocator/emitter error
    // (never on the happy path)
}
stack_slot_isel::allocate_registers(func)
```

### 7.3 `resolve_register_reuse_conflicts` (post-allocation conflict resolution)

`resolve_register_reuse_conflicts` (`regalloc.rs:2836`) is a
**post-allocation verification pass** that runs after
`TargetAgnosticRegAlloc::allocate_intervals` and patches the
`RegAllocResult` in place before it is handed to the per-backend
emitter. It is the load-bearing fix that eliminated the need for
stack-slot fallbacks on syscall-heavy functions.

**The hazard.** A single IR instruction may both *use* a vreg (as an
argument) and *define* a vreg (as a destination). When the allocator
assigns both vregs to the **same physical register** AND the used vreg
is **live after** that instruction, the def clobbers the use — the
instruction's output overwrites its own input before any subsequent
reader can see it. The classic case is `IRInstr::Syscall`, where the
syscall's argument register and its return-value register can be
coalesced to the same physical register by the copy-coalescing pass.
Without resolution, the syscall clobbers the argument value.

**The fix.** For each instruction, the pass walks every
`(use_vreg, def_vreg)` pair. If the two share a physical register and
the use vreg's interval extends past the current position, the pass
reassigns the def vreg to a different register drawn from the
`caller_saved_gprs` + `callee_saved_gprs` lists (not from arbitrary
physical-register indices — the candidate set is the same set the
allocator itself uses, so the reassignment respects `is_allocatable` /
`is_callee_saved`). If every allocatable register is taken, the def
vreg is spilled to a stack slot for that one instruction; the emitter
inserts the corresponding spill/reload code at the position recorded
in `RegAllocResult::spill_code`.

**Why this matters.** Before this pass existed, the register-reuse
hazard forced a broad "syscall-hazard fallback" that pushed
syscall-heavy functions onto the stack-slot path — which in turn kept
those functions off the register-based emitter entirely. With
`resolve_register_reuse_conflicts` in place, the only remaining
fallback is the `contains_fork` opt-out (§7.4), which exists for a
correctness reason unrelated to allocator pressure.

The pass is invoked from both `allocate_function` and
`allocate_function_with_classes` (`regalloc.rs:3031` and `:3051`).
The `aarch64`-specific `LinearScanAllocator` does not call this pass —
its `coalesce_copies_post_alloc` step is the AArch64-local equivalent.

### 7.4 `contains_fork` opt-out (clone/fork detection)

Every register-based backend's `allocate_registers` computes a
`contains_fork: bool` over the IR function before deciding which
emitter to call. The detection catches both the IPC-level
`Call{func: "spawn_worker"}` and `Call{func: "fork"}` and the lowered
`Syscall{nr: 220, ...}` (Linux generic `clone`) / `Syscall{nr: 221,
...}` (`vfork`):

```rust
let contains_fork = func.blocks.iter().any(|block| {
    block.instructions.iter().any(|inst| match inst {
        IRInstr::Call { func: f, .. } => f == "spawn_worker" || f == "fork",
        IRInstr::Syscall { nr, .. } => *nr == 220 || *nr == 221,
        _ => false,
    })
});
```

When `contains_fork` is true, the backend takes the stack-slot path —
**not** because the allocator failed, but because `clone(2)` creates a
child process whose register state diverges from the parent's at the
syscall return, and the register-based prologue/epilogue assumes a
single linear function invocation. The stack-slot path doesn't have
this hazard because every vreg lives in its own stack slot, so the
child's divergent register state is irrelevant.

This is a **targeted correctness opt-out**, not a generic fallback for
register pressure, unimplemented IR ops, or allocator failure. For the
overwhelming majority of compiled functions (no fork), the
register-based `reg_isel.rs` is the production emission path. See
[caveats.md §2.1](./caveats.md#21-contains_fork-opt-out-clonefork-detection)
for the full discussion and
[backends.md §5](./backends.md#5-contains_fork-opt-out-clonefork-detection)
for the per-backend dispatch table.

`wasm32` computes `contains_fork` for parity with the other backends,
but the boolean is purely observational — `wasm32` is a stack machine
with no register-based emitter to fall back from. The actual fork
emulation on `wasm32` is handled separately by
`wasm32_fork_emulation_pass` (§6.4), which rewrites the child branch
to run in-process.

### 7.5 `LinearityReport` consumption

The IVE-produced `LinearityReport` (§1.4) feeds the optimizer's
provenance-directed DCE pass. A vreg that is linearly consumed by a
`StateTransform` or `ForeignConsume` node (and is therefore guaranteed by
the verifier not to be aliased) becomes eligible for DCE once the
consuming node has executed — channel-close, arena-free, and
capability-revoke vregs are the primary beneficiaries. This is a pure
refinement: with an empty `LinearityReport::empty()` the optimizer
behaves exactly as before.

---

## 8. Formal Verification (Lean 4)

VUMA's PMT memory model is mechanically checked in **Lean 4**. The proofs
form the second verification tier beneath the Rust IVE: where IVE is the
executable verifier that ships in the compiler, the Lean proofs are the
mathematical justification that IVE's verdicts are sound with respect to
the operational semantics of the PMT memory model.

**Location.** All Lean sources live in the top-level `proof/` directory
(repo root, sibling of `src/`). The package is a Lake project with a
pinned toolchain (`proof/lean-toolchain`).

### 8.1 What the Lean proofs cover (and what they do not)

The Lean development models the PMT state machine, the IR `exec` relation,
the simulation relation between Lean and the Rust pipeline, the three IVE
state verifiers (state-read, state-write, state-transform), IVE soundness
composition, liveness, the IR/arena/extraction lemmas, and a `Test/` suite
of property and simulation scripts. The three load-bearing results:

| Theorem | Statement | Status |
|---------|-----------|--------|
| `pmt_pillar_sound` | PMT pillar theorem. For a VUMA program `P` with `NoExterns P`, `P.well_typed env`, `DataflowOk`, and `CapacityInvariant`, the Lean `exec` is memory-safe: (1) produces a result, (2) on success `final_used ≤ capacity`, (3) never traps with the OOB code (134). | Proven, sorry-free. |
| `ive_pillar_sound` | IVE pillar theorem. If all 12 IVE rules accept a program (`IveAccepted`), then `FullyVerified` holds and all 9 PMT memory-safety conjuncts follow. | Proven, sorry-free. |
| `ffi_pillar_sound` + `no_ffi_program_sound` | FFI pillar theorem. For `NoFFI P`, every call targets a built-in or an allowlisted syscall; no other externs. | Proven, sorry-free. |

The residual TCB (parser, SCG→IR lowering, optimizer, regalloc, backend
ISel, ELF/Wasm emission, OS interface, hardware) is **not** established by
these theorems. The `PipelineSim` scaffolding (`proof/PMT/PipelineSim.lean`)
is the translation-validation bridge between Lean `exec` and Rust
`pipeline::compile`; closing it is the subject of the deferred
`pmt_pillar_sound_full` work.

### 8.2 Lean FFI bridge — REMOVED

The Lean proofs are **mathematical artefacts only**. There is no Lean→Rust
FFI bridge in the production compiler. The history is documented in
`src/ive/src/verification.rs:948-984`: a previous design routed the 3 PMT
state verifiers (`verify_state_reads`, `verify_state_writes`,
`verify_all_transforms`) through Lean-extracted `lean_verify_*_prim`
externs via a `lean_ffi_linked` build cfg. The default (stub) path linked
a now-deleted `proof/extracted/lean_stub.c`, which hardcoded success
(return 1) for every `lean_verify_*` symbol — every program's PMT state
verification "passed" regardless of actual safety. The real (linked)
path was effectively dead code (`build.rs` never emitted `lean_ffi_linked`
in the default build). **The entire bridge — including the
`proof/extracted/` directory — was deleted in v0.2.0-alpha.10.**

The production verifiers are now **hand-written Rust**
(`src/ive/src/state_read.rs`, `src/ive/src/state_write.rs`,
`src/ive/src/state_transform.rs`, `session_type.rs`,
`information_flow.rs`) and contract discharge is handled by **Z3**
(§5.2). The former `proof/extracted/lean_stub.c` and
`proof/extracted/pmt_check.rs` were removed with the rest of the
`proof/extracted/` directory; the Lean proofs themselves remain in
`proof/` as standalone specification artefacts that are no longer linked
into the compiler binary (see [caveats.md §3.2](./caveats.md)). The
`pmt-runtime-check` Cargo feature is retained as a no-op for `vuma-ive`
(so existing CI commands do not break) but still activates the
independent pure-Rust `pmt_check` module in `vuma-codegen` (a
hand-translation of the Lean checkers, parity-tested, and NOT dependent
on the deleted stub).

**Build.** Build the proofs with `make proof` from the repo root, or
`cd proof && lake build` for the raw Lake invocation. The build is
hermetic — the Lean toolchain is pinned via `proof/lean-toolchain` and
reproduces identically across CI runners. The convenience script
`scripts/verify-all.sh` runs the full local verification suite in a
single invocation: `lake build`, the strict sorry-check
(`PROOF_CHECK_STRICT=1 ./scripts/check-lean.sh`), `lake exe test`, CI
YAML validation, and the existence checks for the key Lean modules and
docs.

**CI integration.** Two GitHub Actions workflows gate the proofs on
every pull request, both running in **strict mode** (any `sorry` or
build warning fails the job):

- `lean-proofs` job inside `.github/workflows/ci.yml` — invokes `lake build`
  on `ubuntu-latest` with a cached Lean toolchain; the job is a required
  check for merge to `main`.
- `.github/workflows/proof-verify.yml` — extended proof-verification
  workflow that runs `lake build` with `--warning-as-error`, plus the
  Rust-side `proof_log` / `bv_verify` cross-check.

**Specifications.** The PMT model is specified in two companion documents
that are the source of truth for the Lean proofs:

- [`./pmt-iris-spec.md`](./pmt-iris-spec.md) — Iris-style separation-logic
  specification of the PMT memory model.
- [`./pmt-formal-spec.md`](./pmt-formal-spec.md) — Formal Lean signature
  and axiomatisation of the PMT state, layouts, and the three IVE
  verifiers.

---

## 9. Cross-references

- [Language Reference](./language-reference.md) — types, expressions,
  statements, builtins, FFI, PMT model.
- [Backend Documentation](./backends.md) — per-backend ABI, ISel strategy,
  ISA encoding audit, QEMU quirks.
- [Testing Infrastructure](./testing.md) — gold-standard harness, CI, KATs.
- [Building Guide](./building.md) — prerequisites (including `libz3-dev`),
  quick start, troubleshooting.
- [Pipeline](./pipeline.md) — stage-by-stage compilation walkthrough.
- [Caveats](./caveats.md) — documented surprises for backend developers.
- [PMT Iris Spec](./pmt-iris-spec.md) — Iris-style separation-logic spec of
  the PMT memory model (source of truth for the Lean proofs).
- [PMT Formal Spec](./pmt-formal-spec.md) — Lean signature and
  axiomatisation of the PMT model (source of truth for the Lean proofs).

---

## 10. Glossary

Brief definitions for the acronyms and project-specific terms used
throughout this document.

| Term | Expansion / Meaning |
|------|---------------------|
| **PMT** | "Programs as Memory Transformations" — VUMA's verification discipline. Every program is a typed state-transformation on a single mmap'd arena (`___pmt_buffer`); state lives in registered `PmtLayoutSpec`s and is accessed via `StateRead` / `StateWrite` / `StateTransform` SCG nodes. **PMT does NOT mean "Persistent Memory Transaction":** there is no persistence (arena is anonymous-mmap, torn down at exit), no rollback (linearity is enforced statically, no undo log), and no durability (no journal, no fsync). |
| **IVE** | Invariant Verification Engine (`src/ive/`, 25 modules). Runs the PMT state verifiers, session-type verifier, information-flow verifier, and Z3-based contract discharge between SCG construction and IR lowering. The legacy pointer-invariant suite (liveness / exclusivity / interpretation / origin / cleanup) was DELETED; `VerificationLevel` has a single `Pmt` variant. |
| **Z3** | The SMT solver (Microsoft Research) used by IVE for contract discharge. **HARD dependency** — `vuma-ive/Cargo.toml` declares `z3 = "0.20"`; the "V" in VUMA depends on it. Install `libz3-dev` (Debian/Ubuntu), `z3` (Homebrew), or `z3` (Arch) before building. |
| **SCG** | Semantic Code Graph (`src/scg/`). Typed, control-flow-annotated IR lifted from the AST; canonical input to the verifier and to codegen. |
| **IR** | Backend-neutral SSA-like intermediate representation (`src/codegen/src/ir.rs`) using virtual registers (`VReg` / `IRValue::Register(n)`). |
| **BD** | Behavioral Descriptors (`src/bd/`). 3-axis (repd/capd/reld) value model feeding IVE's interpretation proofs. |
| **`LinearityReport`** | IVE-produced report enumerating bare vregs linearly consumed by `StateTransform` / `ForeignConsume` nodes. Consumed by `run_optimizations` for provenance-directed DCE. |
| **`FnContract`** | Source-level `requires` / `ensures` contract on a `transform` declaration, translated to IVE-consumable form. Discharged by Z3. |
| **`ProveBlockObligation`** | A `prove { require …; <body> }` block, translated to IVE-consumable form. Each `require` clause is a Z3-dischargeable proof obligation. |
| **`PmtLayoutSpec`** | Typed layout registered with IVE describing the in-arena shape of a `State<T>` (field offsets, sizes, type names). |
| **`___pmt_buffer`** | The single mmap'd arena backing all PMT state in a compiled VUMA program. Anonymous (no backing file); torn down at process exit. |
| **Two-pipe channel** | VUMA's IPC architecture: every `channel_open` creates two OS pipes packed into a 16-byte handle `{read_fd1, write_fd1, read_fd2, write_fd2}` (pipe1: parent→child, pipe2: child→parent). A per-function handle registry allows `spawn_worker` to swap all handles in the child after `clone()`. |
| **`transform`** | The sole function-declaration keyword. The legacy `fn` keyword was removed from the keyword table and now lexes as a plain `Ident`. |
| **`TargetAgnosticRegAlloc`** | The production register allocator on all 14 native register-based backends, including `aarch64` (since W7-impl) (`regalloc.rs:2966`). `TargetDesc`-driven linear-scan: the per-ISA register file is supplied at construction time by `TargetDescRegistry::get(<isa>)`. The 4 byte-swap wrapper backends inherit their parent's allocation result via one-line delegation. |
| **`LinearScanAllocator`** | The older AArch64-specific linear-scan allocator (`regalloc.rs:1284`). Now a **fallback only** — `aarch64`'s default path uses `TargetAgnosticRegAlloc` via `try_real_regalloc` + `reg_isel::emit_function_regalloc_full` (since W7-impl); `LinearScanAllocator` + `Emitter::emit_function_regalloc` (`emit.rs:1056`) is invoked only when the full register-based emitter returns an error. Predates the directory-style `reg_isel.rs` pattern; functionally equivalent to `TargetAgnosticRegAlloc` but with hardcoded caller/callee-saved GPR+SIMD lists. |
| **`resolve_register_reuse_conflicts`** | Post-allocation verification pass (`regalloc.rs:2836`) that detects and fixes cases where a single instruction's `use_vreg` and `def_vreg` would land in the same physical register while the `use_vreg` is still live afterwards. Reassigns the `def_vreg` to a different allocatable register, or spills it if every register is taken. Eliminated the broad "syscall-hazard fallback" that previously forced stack-slot emission on syscall-heavy functions. See §7.3. |
| **`contains_fork` opt-out** | The *one and only* situation in which the register-based emission path is bypassed: a function whose IR contains a `clone`/`fork` syscall (Linux generic nrs 220/221) takes the stack-slot path because the child process's divergent register state is incompatible with the register-based prologue/epilogue. This is a **correctness requirement**, not a fallback for register pressure or unimplemented IR ops. See §7.4. |
| **`reg_isel.rs`** | Per-backend module exposing `emit_function_regalloc_full(func, &alloc)` — the register-to-register machine-code emitter that consumes a `RegAllocResult` and produces an `AllocatedFunction`. Present in all 14 native register-based backends (including `aarch64`, which gained its own `reg_isel.rs` in W7-impl). The 4 byte-swap wrapper backends re-export their parent's `emit_function_regalloc_full` via `pub use`. |
