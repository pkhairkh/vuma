# VUMA Architecture Overview

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
`src/bin/compile_dump.rs:226` and `src/ive/src/result.rs`.

The IVE has three subsystems, all hand-written in Rust (no Lean FFI — see
§6.1):

- **PMT state verifiers** — `state_read`, `state_write`, `state_transform`
  (`src/ive/src/state_{read,write,transform}.rs`). These check that every
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

VUMA ships two allocation strategies, both driven from
`src/codegen/src/regalloc.rs`:

- **Real linear-scan register allocation** on the four tier-1 backends
  (`aarch64`, `x86_64`, `riscv64`, `ppc64`). The `LinearScanAllocator`
  (`regalloc.rs:1208`) covers AArch64; `TargetAgnosticRegAlloc`
  (`regalloc.rs:2562`) is a `TargetDesc`-driven linear-scan allocator used
  by `x86_64`, `riscv64`, and `ppc64`. Both
  implement live-interval computation, boundary-safe overlap detection,
  spill-weighted eviction, and copy coalescing; both write a
  `RegAllocResult` that `regalloc_emit.rs::annotate_with_regalloc` merges
  into the stack-slot ISel output to annotate `reads` / `writes` with the
  assigned physical registers. Spilled vregs fall back to their stack slot.
- **Stack-slot ISel** on the remaining 15 backends (`arm32`, `armeb`,
  `aarch64_be`, `alpha`, `hppa`, `loongarch64`, `m68k`, `mips64`,
  `mips64be`, `ppc64le`, `riscv32`, `s390x`, `sparc64`, `x86_32`, and the
  non-tier-1 paths of the four tier-1 backends). Every `VReg` lives in a
  stack slot at `[frame_ptr − offset]` and `allocate_registers` generates
  load-op-store sequences using 2–4 fixed scratch registers. The
  per-backend implementation lives in
  `{arm32,x86_64,x86_32,mips64,ppc64,riscv64,riscv32,s390x,sparc64,loongarch64,...}/stack_slot_isel.rs`
  or is inlined into `{m68k,alpha,hppa}.rs`.

The `loongarch64/reg_alloc_isel.rs` file (1.6 K LOC) on disk is **dead
code** — the module declaration is commented out at
`loongarch64/mod.rs:6943` and the production `allocate_registers` calls
`stack_slot_isel::allocate_registers`. The file is retained for
historical reference; it is not compiled.

### 1.9 Backend Emission

Per-backend modules under `src/codegen/src/` (see [backends.md](./backends.md)
for the matrix) consume the post-`opt` IR and emit machine instructions as
raw byte vectors. The emitter is responsible for instruction selection,
calling-convention lowering, callee-saved register save/restore,
prologue/epilogue, and emission of trap stubs (`__arena_overflow`,
`__oob_trap`, `__uaf_trap`). Backends share `src/codegen/src/emit.rs` for
ELF section construction and `marshal.rs` for relocation records.

All ISA encodings have been **verified against the official ISA manuals** —
see [backends.md](./backends.md) §6 for the per-ISA audit (LoongArch FP
condition codes, Power ISA XO field corrections, RISC-V OPC_NMADD, Alpha
CMPULE comment). Each verified encoding carries a citation to the manual
section in its inline comment.

### 1.10 Object Emission (ELF / Wasm)

`src/codegen/src/emit.rs` writes relocatable ELF objects (`ET_REL`) for the
17 native targets, including per-arch `e_machine`, endianness, and `e_flags`.
The 4 big-endian variants emit BE ELF for `qemu-*-be` testing. The wasm32
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

19 backends, each a module under `src/codegen/src/`. Four
(`aarch64_be`, `armeb`, `mips64be`, `ppc64le`) are thin byte-swap wrappers
(200–530 LOC each) that delegate to a parent backend and rewrite
instruction words at the encoding boundary to produce BE/LE ELF for
`qemu-*` variants. Tier classification follows `BackendTier` in `backend.rs`
and reflects actual ISA coverage.

**Test status:** all 19 backends pass the gold-standard matrix at **100 %
(29 944 / 29 944 test runs)** as of HEAD. The matrix is driven by
`scripts/vuma_test_matrix_19backends.sh` and `scripts/pi5_test_suite.sh`.

| # | Backend | File / dir | Tier | Regalloc | Key approach |
|--:|---------|------------|------|----------|--------------|
|  1 | `aarch64`     | `arm64.rs`              | Complete | LinearScan | Reference backend; `LinearScanAllocator` (`regalloc.rs:1208`). |
|  2 | `aarch64_be`  | `aarch64_be.rs`         | Complete (wrap) | inherits AArch64 | Forwards LE instr. bytes (ARM ARM D6.1.3); only ELF header swapped. |
|  3 | `x86_64`      | `x86_64/{mod,stack_slot_isel,disasm}.rs` | Complete | TargetAgnostic | `materialize_f32_immediates` consumer (load-bearing pass ordering). |
|  4 | `x86_32`      | `x86_32/{mod,stack_slot_isel,disasm}.rs` | Complete | Stack-slot | I64 channel handle stored in 4-byte slot. |
|  5 | `riscv64`     | `riscv64.rs`            | Complete | TargetAgnostic | Largest single-file backend; `try_real_regalloc` helper. |
|  6 | `riscv32`     | `riscv32.rs`            | Complete | Stack-slot | QEMU run requires `-cpu max` (D extension). |
|  7 | `loongarch64` | `loongarch64/{mod,stack_slot_isel,disasm}.rs` | Complete | Stack-slot | FP compare condition codes fixed per LoongArch Vol 1 §3.2.2.1. |
|  8 | `arm32`       | `arm32/{mod,disasm}.rs` | Complete | Stack-slot | `preregister_param_types` race-fix for parallel alloc. |
|  9 | `armeb`       | `armeb.rs`              | Complete (wrap) | inherits Arm32 | BE32 word-swap on every 4-byte instr. |
| 10 | `mips64`      | `mips64/{mod,disasm}.rs`| Complete | Stack-slot | Emits BE ELF header natively. |
| 11 | `mips64be`    | `mips64be.rs`           | Complete (wrap) | inherits MIPS64 | Word-swaps each 4-byte instr. word LE→BE. |
| 12 | `ppc64`       | `ppc64/{mod,disasm}.rs` | Complete | TargetAgnostic | Native BE (ELFv2); 6 Power ISA XO bugs fixed. |
| 13 | `ppc64le`     | `ppc64le.rs`            | Complete (wrap) | inherits PPC64 | Reuses ppc64 encoders; only ELF endianness flipped. |
| 14 | `wasm32`      | `wasm32/{mod,disasm}.rs`| Complete | Wasm-structured | Trampoline loop + ring-buffer channels + in-process fork emulation. |
| 15 | `sparc64`     | `sparc64.rs`            | Experimental | Stack-slot | `FloatToUInt` of negatives via `FSTOx → RDY → AND → LDx` sign-clear sequence. |
| 16 | `s390x`       | `s390x.rs`              | Experimental | Stack-slot | Secondary Ret path restores callee-saved S0–S5 before `adjust_sp`. |
| 17 | `m68k`        | `m68k.rs`               | Experimental | Stack-slot | Two QEMU 7.2.0-m68k translator bugs worked around (MOVEM, ADDI.B/CMPI.B). |
| 18 | `alpha`       | `alpha.rs`              | Experimental | Stack-slot | f64→u64 truncation for f≥2⁶³ saturates to `i64::MAX`. QEMU 10.0-alpha rejects CMPULE (function 0x3D); workaround via CMPULT. |
| 19 | `hppa`        | `hppa.rs`               | Experimental | Stack-slot | QEMU 7.2.0-hppa LDIL decoder bug worked around via format-14 LDO. |

Per-backend ABI tables, ISel notes, encoding-audit details, and QEMU
quirks are in [backends.md](./backends.md).

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
 │ vuma-codegen: regalloc (LinearScan / TargetAgnostic on 4 tier-1 backends,
 │   stack-slot ISel on the other 15)
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
  `src/main.rs:747-750`.

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

Two real register allocators live in `src/codegen/src/regalloc.rs`:

### 7.1 `LinearScanAllocator` (AArch64)

A real linear-scan allocator (`regalloc.rs:1208`) using the full AArch64
register set (caller-saved GPRs X9–X15, X16–X18, X8; SIMD V0–V31).
Live-interval computation, boundary-safe overlap detection
(`liveness_interference_from`), spill-weighted eviction
(`spill_weight_with_pressure`), and copy coalescing
(`coalesce_copies_post_alloc`). Produces a `RegAllocResult` consumed by
`AArch64Backend::emit_function_regalloc` (`backend.rs:2212`). The
`aarch64_be` wrapper inherits this verbatim via one-line
`allocate_registers` delegation.

### 7.2 `TargetAgnosticRegAlloc` (x86_64, riscv64, ppc64)

A `TargetDesc`-driven linear-scan allocator (`regalloc.rs:2562`) that
takes the per-ISA register file from
`target_desc::TargetDescRegistry::get(<isa>)`. The same algorithm runs
on three tier-1 backends:

- **`x86_64`** — wired at `x86_64/mod.rs:4081`
  (`TargetAgnosticRegAlloc::new(target)`).
- **`riscv64`** — wired via `try_real_regalloc` at
  `riscv64.rs:6542`, looking up `"riscv64"` in the registry.
- **`ppc64`** — wired via `try_real_regalloc` at
  `ppc64/mod.rs:3011`, looking up `"ppc64"`. `ppc64le` inherits via
  one-line delegation.

Each backend's `try_real_regalloc` returns `None` (and the backend falls
back to the unannotated stack-slot ISel output) if the target description
is missing or the allocator errored. The `RegAllocResult` is merged into
the stack-slot output by
`regalloc_emit::annotate_with_regalloc`, which overwrites the
`reads` / `writes` physical-register metadata on each
`AllocatedInstruction` with the assigned physical registers. Spilled
vregs keep their stack slot.

### 7.3 Stack-slot ISel (other 15 backends)

Every backend not listed in §7.1 / §7.2 uses stack-slot ISel: every
`VReg` lives in a stack slot at `[frame_ptr − offset]` and
`allocate_registers` generates load-op-store sequences using 2–4 fixed
scratch registers. An IR op such as `BinOp{Add, dst: v5, lhs: v3, rhs:
v4}` is lowered as `load scratch0,[fp+v3_off]; load scratch1,[fp+v4_off];
add scratch0,scratch0,scratch1; store [fp+v5_off],scratch0` — three
memory operations per IR op. Under QEMU TCG user-mode emulation each
load/store is ~10–100× slower than a register op; this is acceptable for
correctness testing but is the primary reason emitted VUMA binaries are
not benchmark-grade.

### 7.4 `LinearityReport` consumption

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
`proof/extracted/lean_stub.c`, which hardcoded success (return 1) for
every `lean_verify_*` symbol — every program's PMT state verification
"passed" regardless of actual safety. The real (linked) path was
effectively dead code (`build.rs` never emitted `lean_ffi_linked` in the
default build). **The entire bridge has been deleted.**

The production verifiers are now **hand-written Rust**
(`src/ive/src/state_{read,write,transform}.rs`,
`session_type.rs`, `information_flow.rs`) and contract discharge is
handled by **Z3** (§5.2). `proof/extracted/lean_stub.c` and
`proof/extracted/pmt_check.rs` are kept on disk for reference but are no
longer compiled or linked. The `pmt-runtime-check` Cargo feature is
retained as a no-op for `vuma-ive` (so existing CI commands do not break)
but still activates the independent pure-Rust `pmt_check` module in
`vuma-codegen` (a hand-translation of the Lean checkers, parity-tested,
and NOT dependent on the stub).

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
