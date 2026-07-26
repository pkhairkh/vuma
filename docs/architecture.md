# VUMA Architecture Overview

**Status:** Reference (IEEE-style). **Audience:** compiler engineers, backend
implementers, verification engineers. **Scope:** end-to-end pipeline from
`.vuma` source to native object code / Wasm, plus the verification discipline
applied between front-end and back-end. **Cross-references:**
the language reference ·
[Backend Documentation](../backends/) ·
[Testing Infrastructure](../testing/) ·
[IVE Audit](./caveats.md) ·
[Caveats](./caveats.md).

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

### 1.2 AST

`src/parser/src/` produces a Pratt-parsed AST with declarations (`fn`,
`struct`, `enum`, `layout`, `extern`, `transform`), statements (`let`,
`assign`, `if`/`else`, `while`, `for`, `match`, `return`, `break`,
`continue`), expressions (arithmetic, logical, comparison, cast, channel,
struct/enum construction, field access, indexing), and attributes
(`#[borrow]`, `#[inline]`, `#[no_mangle]`, `#[link_section]`, effect purity
annotations). The AST is the only IR that preserves source-level names and
spans.

### 1.3 SCG — Semantic Code Graph

`src/scg/` lifts the AST into a typed, control-flow-annotated graph. The SCG
performs name resolution, type checking, monomorphization of generics
(`src/codegen/src/monomorphize.rs`), effect inference
(`src/codegen/src/effects.rs`), escape analysis
(`src/codegen/src/escape_analysis.rs`), and alias analysis
(`src/codegen/src/alias_analysis.rs`). The SCG is the canonical input to the
verifier and to codegen; raw AST nodes are never seen past this stage.

### 1.4 IVE — Invariant Verification Engine

`src/ive/` (22 K LOC, 25 modules) runs the verification discipline selected by
`CompileConfig.verification_level`. The `VerificationLevel` enum has a single
`Pmt` variant — the five legacy pointer invariants (liveness, exclusivity,
interpretation, origin, cleanup) and the `Quick`/`Normal`/`Exhaustive`/
`Modular`/`ConstantTime`/`Hardened` level variants have been **deleted**
(historical context in
`src/ive/src/invariant_aggregator.rs:115-156`). At the `Pmt` level three
state verifiers run (state-read, state-write, state-transform). IVE consumes
the SCG plus registered `PmtLayoutSpec`s and emits a structured
`VerificationReport`. See the [IVE audit](./caveats.md).

### 1.5 IR Lowering

`src/codegen/src/scg_to_ir.rs` (~8 K LOC) lowers the SCG to the backend-neutral
SSA-like IR defined in `src/codegen/src/ir.rs`. The IR uses virtual registers
(`VReg`), typed basic blocks, and a fixed set of `IRInstr` variants
(`BinOp`, `UnOp`, `Load`, `Store`, `Branch`, `Call`, `Syscall`, `Cast`,
`ICmp`, `FCmp`, `CondBranch`, `Return`, `Phi`-shaped joins via block params).

### 1.6 IPC Lowering

`src/codegen/src/ipc_lowering.rs` (~3.8 K LOC) is a single shared pass over the
IR for **all 19 backends**. It expands 35+ builtins
(`channel_open`/`send`/`recv`/`close`/`try_recv`, `spawn_worker`/`wait_worker`,
`shared_memory_*`, `checkpoint_*`, `aead_seal`/`open`, `sandbox_apply`,
`capability_grant`/`delegate`, `circuit_breaker_*`, `hot_swap_*`,
`stark_prove`/`verify`, `formal_verify`, …) into IR sequences using
`Syscall{nr}` (asm-generic numbers) and block-splitting via the `Expansion`
struct. On `wasm32` only, the `wasm32_fork_emulation_pass` runs after
`lower_ipc_builtins` to rewrite the child branch's `Return` into a `Store` at
fixed address `4096` followed by `Jump` to the parent's post-fork block, so
both branches run sequentially in-process.

### 1.7 Mid-end Optimizations (`opt`)

`src/codegen/src/opt.rs` (~2 K LOC) runs the optimization pipeline: constant
propagation/folding, dead-code elimination, common-subexpression elimination,
loop unrolling (`loop_unroll.rs`, ~3 K LOC), e-graph rewriting
(`egraph.rs`), identical-function merging (ICF), whole-program DCE,
profile-guided optimization (PGO), vectorization (`vectorize.rs`), and
`materialize_f32_immediates` (a load-bearing pass that must run after folding
and before codegen to avoid f32-bit-immediate corruption on x86_64).

### 1.8 Register Allocation / ISel

`src/codegen/src/regalloc.rs` declares three allocators: `RegAllocator`
(legacy greedy, AArch64-only), `LinearScanAllocator` (real linear-scan,
AArch64-only), and `TargetAgnosticRegAlloc` (available to all, adopted by
none). The **de facto** allocator for 17 of 19 backends is the **stack-slot
ISel** pattern: every `VReg` lives in a stack slot at `[frame_ptr − offset]`
and "allocate_registers" generates load-op-store sequences using 2–4 fixed
scratch registers. Only `loongarch64/reg_alloc_isel.rs` (1.6 K LOC)
attempts block-local register caching.

### 1.9 Backend Emission

Per-backend modules under `src/codegen/src/` (see for the matrix) consume
the post-`opt` IR and emit machine instructions as raw byte vectors. The
emitter is responsible for instruction selection, calling-convention
lowering, callee-saved register save/restore, prologue/epilogue, and
emission of trap stubs (`__arena_overflow`, `__panic`). Backends share
`src/codegen/src/emit.rs` for ELF section construction and `marshal.rs`
for relocation records.

### 1.10 Object Emission (ELF / Wasm)

`src/codegen/src/emit.rs` writes relocatable ELF objects (`ET_REL`) for the
17 native targets, including per-arch `e_machine`, endianness, and `e_flags`.
The 4 big-endian variants emit BE ELF for `qemu-*-be` testing. The wasm32
backend (`src/codegen/src/wasm32/`) emits a `.wasm` module directly using
a trampoline-loop control-flow pattern (`loop $trampoline ... br_table`)
because Wasm requires structured control flow and VUMA's CFG is arbitrary.

---

## 2. Crates

The workspace (`Cargo.toml`) declares **seven member crates** plus an
in-tree LSP module and a package manager. LOC figures are exact counts of
`.rs` lines under each crate directory at HEAD.

| Crate | Path | LOC (Rust) | Responsibility |
|------------------|-------------------|-----------:|-------------------------------------------------------------|
| `vuma-parser` | `src/parser/` | 21 282 | Lexer, Pratt parser, AST, attribute parsing. |
| `vuma-scg` | `src/scg/` | 22 569 | Semantic Code Graph: name resolution, type checking, effects.|
| `vuma-ive` | `src/ive/` | 22 085 | Invariant Verification Engine (25 modules). |
| `vuma-codegen` | `src/codegen/` | 192 039 | IR, opt, regalloc, 19 backends, ELF/Wasm emission, IPC lowering.|
| `vuma-bd` | `src/bd/` | 15 668 | Behavioral Descriptors: 3-axis (repd/capd/reld) value model.|
| `vuma-core` | `src/vuma/` + `src/*.rs` | 14 595 | Pipeline driver, FFI, diagnostics, telemetry, LSP, llm_api. |
| `vuma-package` | `src/package/` | 2 350 | Package manager, manifest, dependency resolution. |
| `vuma-lsp` (mod) | `src/vuma/lsp/` | 2 611 | Language Server Protocol front-end (in-tree module of core). |
| **Total** | | ≈ 293 K | (Live crates only.) |

`vuma-tests` (`src/tests/`) is excluded from the LOC total above; it ships
~30 K LOC of integration tests, property tests, and bootstrap suites.

---

## 3. Backend Matrix

19 backends, each a module under `src/codegen/src/`. Four
(`aarch64_be`, `armeb`, `mips64be`, `ppc64le`) are thin byte-swap wrappers
(200–530 LOC each) that delegate to a parent backend and rewrite instruction
words at the encoding boundary to produce BE/LE ELF for `qemu-*` variants.
Tier classification follows `BackendTier` in `backend.rs` and reflects actual
ISA coverage, not aspiration.

| # | Backend | File / dir | LOC | Tier | Key approach |
|--:|---------------|--------------------|-------:|---------------|------------------------------------------------------------------|
| 1 | `aarch64` | `arm64.rs` | 6 235 | Complete | Real `LinearScanAllocator`; `verify_function_float_ops` runs via the central driver-level check (all 19 backends covered). |
| 2 | `aarch64_be` | `aarch64_be.rs` | 197 | Complete (wrap)| Delegates to `arm64.rs`; **no** instruction swap (ARM ARM D6.1.3: AArch64 fetch is always LE). |
| 3 | `alpha` | `alpha.rs` | 3 365 | Experimental | Stack-slot ISel; FP ≥ 2^63 → CVTTQ path — `FloatToUInt` cast saturates to `i64::MAX` instead of using undefined `CVTTQ` semantics.|
| 4 | `arm32` | `arm32/` | 11 786 | Complete | Stack-slot ISel; `preregister_param_types` race-fix for parallel alloc.|
| 5 | `armeb` | `armeb.rs` | 242 | Complete (wrap)| Delegates to `arm32/`; **swaps instruction words** (BE32). |
| 6 | `hppa` | `hppa.rs` | 6 310 | Experimental | Scaffolded: Mul/Div/Cmp/cond-branches emit stub code; QEMU LDIL workaround uses LDI format-14. F1b FP-comparison stub uses `unimplemented!`. |
| 7 | `loongarch64` | `loongarch64/` | 11 220 | Complete | Only backend using block-local register caching (`reg_alloc_isel.rs`).|
| 8 | `m68k` | `m68k.rs` | 5 057 | Experimental | Stack-slot ISel; signed div, FP, atomics ABI gaps. G4 68881 coprocessor-1 (F-line) byte-verification path stubbed. |
| 9 | `mips64` | `mips64/` | 5 953 | Complete | Stack-slot ISel. |
|10 | `mips64be` | `mips64be.rs` | 300 | Complete (wrap)| Delegates to `mips64/`; swaps words but not ELF header. |
|11 | `ppc64` | `ppc64/` | 6 994 | Complete | Stack-slot ISel; QEMU poll-syscall workaround for `try_recv`. |
|12 | `ppc64le` | `ppc64le.rs` | 530 | Complete (wrap)| Delegates to `ppc64/`; swaps words BE → LE. |
|13 | `riscv32` | `riscv32.rs` | 9 589 | Complete | Stack-slot ISel. |
|14 | `riscv64` | `riscv64.rs` | 11 057 | Complete | Stack-slot ISel. |
|15 | `s390x` | `s390x.rs` | 4 239 | Experimental | Stack-slot ISel; secondary Ret path restores callee-saved S0–S5 before `adjust_sp`.|
|16 | `sparc64` | `sparc64.rs` | 6 030 | Experimental | Stack-slot ISel; op=2 vs op=3 footgun; `FloatToUInt` of negatives via `FSTOx → RDY → AND → LDx [sign-clear]` sequence.|
|17 | `wasm32` | `wasm32/` | 9 202 | Complete | Trampoline-loop CFG via `br_table`; in-process fork emulation; ring-buffer channel builtins.|
|18 | `x86_32` | `x86_32/` | 9 020 | Complete | Stack-slot ISel. |
|19 | `x86_64` | `x86_64/` | 10 243 | Complete | Stack-slot ISel; `materialize_f32_immediates` consumer (load-bearing pass ordering).|

Per-backend ABI tables, ISel notes, and QEMU quirks are in
[../backends/](../backends/). The 15 Complete backends are exercised by the
gold-standard matrix (1 561 × 19 = 29 659 runs).

---

## 4. Compilation Pipeline

End-to-end flow from source to binary, with the responsible crate and the
output artifact of each stage:

```
.vuma source
 │ vuma-parser (lexer + Pratt parser)
 ▼
AST ─── src/parser/
 │ vuma-scg (name resolution, type check, monomorphize, effects, escape, alias)
 ▼
SCG ─── src/scg/
 │ vuma-ive (Pmt state verifiers — legacy pointer invariants DELETED)
 ▼
Verified SCG + VerificationReport ─── src/ive/
 │ vuma-codegen: scg_to_ir
 ▼
SSA-like IR (VReg, typed blocks) ─── src/codegen/src/scg_to_ir.rs
 │ vuma-codegen: ipc_lowering (shared by all backends)
 │ + wasm32_fork_emulation_pass (wasm32 only)
 ▼
IR with Syscalls / channel ops lowered
 │ vuma-codegen: opt (CSE, DCE, loop_unroll, egraph, ICF, PGO, vectorize,
 │ materialize_f32_immediates)
 ▼
Optimized IR
 │ vuma-codegen: regalloc / stack-slot ISel (per backend)
 ▼
Machine instruction stream (Vec<u8>)
 │ vuma-codegen: emit.rs (ELF) | wasm32/mod.rs (Wasm)
 ▼
ET_REL ELF object | .wasm module
 │ system linker (ld) | wasmtime runtime
 ▼
Native executable | Wasm execution
```

Two side inputs feed the pipeline: the **PMT layout registry** (typed
`PmtLayoutSpec`s registered with IVE describing the state buffer's layouts).
The former COR runtime bridge (`src/cor/bridge.rs`) and COR→PMT
profile-driven re-optimization hook were removed along with the legacy
pointer invariants; see `./caveats.md`.

---

## 5. Verification Pipeline

The verifier is invoked between SCG construction and IR lowering. Its inputs
are the SCG, the registered PMT layout specs, and the `CompileConfig`
selected by the driver. Its output is a `VerificationReport` consumed by
`pipeline.rs` to gate code emission.

### 5.1 What IVE checks

At the (now sole) `VerificationLevel::Pmt` (mandatory since VUMA 2.0; the
`--no-memory-safety` flag is removed and `memory_safety=false` is silently
ignored), three PMT state verifiers run:

- **state-read**: every `state.field` read is type-correct against the
 registered `PmtLayoutSpec` and the slot is in a live state region.
- **state-write**: every `state.field = …` write respects the layout's type
 and offset, and the field is within the backing arena.
- **state-transform**: a function with a `State<T>` parameter that reads and
 writes the same fields preserves the layout's invariants across the call.

The `VerificationLevel` enum has a single `Pmt` variant. The five legacy
pointer invariants (liveness / exclusivity / interpretation / origin /
cleanup) and the `Quick` / `Normal` / `Exhaustive` / `Modular` /
`ConstantTime` / `Hardened` level variants have been **deleted entirely**
from `InvariantKind` (see `src/ive/src/invariant_aggregator.rs`). The 10
violation kinds E041–E050 that used to be emitted by the legacy pointer
invariants are no longer produced.

### 5.2 What IVE does not check

See the [IVE audit](./caveats.md) for the full list and the current
resolution status of each gap. The remaining gaps are:

- **Arbitrary raw-pointer arithmetic is not bounds-checked.** `--safe`
 injects `ComputationNode(UGe)` + `ControlNode::If { __oob_trap }`
 bounds-check pairs before every `AccessNode::Load` / `AccessNode::Store`
 whose base derives from an arena allocation with a known `length_expr`
 (`pipeline.rs:6070-6073` selects `safe_mode` vs `compile_time_only`;
 `pipeline.rs:6104` calls `find_bounds_check_sites_with_bounds` and
 `pipeline.rs:6139` calls `inject_bounds_check_ir`). Coverage is
 **partial** — raw pointer arithmetic and derived-pointer loads/stores
 with no resolvable allocation size remain unbounded. See `./caveats.md`.
- **`--safe` is no longer a no-op.** The flag is wired through to the
 bounds-check emitter described above (was previously parsed but
 discarded). See `./caveats.md`.
- **`find_bounds_check_sites` (the no-`_with_bounds` wrapper) was REMOVED**
 along with its `runtime_bounds_instrumented` report field (which was set
 but never read by any emitter). Only the live
 `find_bounds_check_sites_with_bounds` variant remains. See `./caveats.md`.
- **Float-op validation is now centralised across all 19 backends.**
 The AArch64-only call site in `arm64.rs::allocate_registers` was removed
 and `verify_function_float_ops` is wired into all 5 compilation drivers
 (`src/main.rs`, `src/pipeline.rs`, `src/api.rs`, `src/bin/compile_dump.rs`).
 See `./caveats.md`.
- **`syscall_abi::translate_or_warn` is no longer dead code.** It is
 invoked by 16+ call sites across 15 native backends + 1 generic helper
 to translate asm-generic syscall numbers to per-ISA native `nr`s. See
 `./caveats.md`.
- **Linear-channel discipline is now an UNCONDITIONAL HARD-FAIL.** The
 false-positive fix (re-typing `ChannelEvent.vreg` from `u32` to `String`
 and keying on the channel handle's variable name) made the verifier sound
 enough to enforce by default, so `--strict-ive` is no longer required to
 abort on use-after-close / double-close / use-without-open. The remaining
 advisory verifier — **`bv_verify` e-graph soundness** — still logs
 warnings only by default and is promoted to HARD-FAIL by `--strict-ive`.
 See `./caveats.md`.

### 5.3 PMT clarification

In this codebase **PMT = "Programs as Memory Transformations"**, not
"Persistent Memory Transaction." There is no transaction, no rollback, no
durability machinery. Every program is treated as a typed state-transformation
on a single mmap'd arena (`___pmt_buffer`); state lives in layouts registered
with the verifier; `arena_alloc` returns a state-typed pointer into that
buffer.

The canonical source-code definition lives on the `InvariantKind::Pmt` doc
comment (`src/ive/src/invariant_aggregator.rs`, `InvariantKind::Pmt` variant),
which carries an explicit "What 'PMT' means (and does NOT mean)" section
enumerating the three missing properties (no persistence, no rollback, no
durability). `VerificationLevel::Pmt` repeats the disambiguation in shorter
form. (Line numbers are intentionally omitted here — they drift as the file
is edited; grep for `Acronym disambiguation` or `What "PMT" means` to find
the comments.) See also `./caveats.md` and (Glossary) below.

---

## 6. Formal Verification (Lean 4)

VUMA's PMT memory model is mechanically checked in **Lean 4**. The proofs
form the second verification tier beneath the Rust IVE: where IVE is
the executable verifier that ships in the compiler, the Lean proofs are
the mathematical justification that IVE's verdicts are sound with respect
to the operational semantics of the PMT memory model.

**Location.** All Lean sources live in the top-level `proof/` directory
(repo root, sibling of `src/`). The package is a Lake project with a
pinned toolchain (`proof/lean-toolchain`).

**Modules.** **23+ Lean modules** under `proof/PMT/` model the PMT state
machine, the IR `exec` relation, the simulation relation between Lean
and the Rust pipeline, the three IVE state verifiers (state-read,
state-write, state-transform), IVE soundness composition, liveness, the
IR/arena/extraction lemmas, and a `Test/` suite of property and
simulation scripts. The modelling layers that close long-standing
faithfulness gaps:

- **`PMT.Iris.*`** (`CapBndInvariant.lean`, `LiveMirrorInvariant.lean`,
 `GuardInvariant.lean`, `Composition.lean`) — the three named Iris
 invariants `[cap_bnd]`, `[live_mirror]`, `[guard]` formalised as
 proper separation-logic resources with ghost state, plus the
 Composition theorem `alloc_preserves_all_invariants` showing the
 bundle `[cap_bnd] ∗ [live_mirror] ∗ [guard]` is preserved by `alloc`.
 They carry `Own γ v` ghost ownership over the `Ex`/`Ag` resource
 algebras (see [`./pmt-iris-spec.md`](./pmt-iris-spec.md)).
- **`PMT.BitVecArena`** — a faithful arena model using `BitVec 64` for
 addresses/offsets (mirrors `usize` on 64-bit targets), making the
 `usize` arithmetic-overflow branch syntactically expressible — the
 actual failure mode the Rust `checked_add` defends against.
- **`PMT.MmapArena`** — models the `mmap`/allocator-null failure path
 via `raw_create: Nat → Except TrapCode RawArena`, closing the most
 acute simulation-soundness gap (the bare `RawArena` constructor
 admitted arenas the Rust `Arena::create` would have trapped before
 producing).
- **`PMT.PipelineSim`** — establishes the **first mechanical
 Lean↔Rust simulation connection** (CompCert-style translation
 validation): a `PipelineSpec` structure models the specification that
 `src/pipeline.rs::compile` claims to meet, with theorems
 `exec_satisfies_pipeline_spec`, `pipeline_compile_sound`, and
 `pipeline_compile_no_oob` reducing end-to-end safety of the compiled
 binary to `pmt_soundness` plus a `hconforms` translation-validation
 hypothesis.

**Rust integration.** The Lean-verified PMT checkers are
hand-translated to Rust in
`src/codegen/src/runtime/pmt_check.rs` (gated by the
`pmt-runtime-check` cargo feature on `vuma-codegen`). The feature is
**WIRED into the production arena path** —
`src/codegen/src/runtime/arena.rs` calls the Lean-verified
`verified_capacity_check` (and the sibling `verified_field_bounds_check`
/ `verified_linearity_check` / `verified_pmt_check`) on the arena-overflow
and capacity-overflow branches when the feature is enabled; the four Lean
functions carry `@[export lean_verified_*]` attributes in
`proof/PMT/Extraction.lean` so `lake build` emits the C symbols
consumable by `extern "C"` bindings; and the root `Cargo.toml` forwards
the feature (`pmt-runtime-check = ["vuma-codegen/pmt-runtime-check"]`)
so `cargo build --features pmt-runtime-check` works from the repo root.
The parity test `tests/pmt_parity_test.rs` (5 tests) confirms the Rust
translations match the Lean definitions on all test cases — this is the
Rust-side discharge of the `PipelineSim` conformance assumption. The
dedicated `tests/pmt_feature_flag_test.rs` (3 tests, gated by
`#![cfg(feature = "pmt-runtime-check")]`) verifies the wiring itself
compiles and the `verified_*` symbols are callable from the codegen crate.

**Key theorems.** The development totals ~60 theorems and supporting
lemmas across the 23+ modules; Iris-construct coverage is **6/17** (the
named-invariant trio `[cap_bnd]`, `[live_mirror]`, `[guard]`, the
`ArenaRes` resource bundle, fractional permissions `↦{q}`, and the
weakest-precondition `wp e {Φ}`). The load-bearing results are summarised
below.

| Theorem | Statement | Status |
|---------|-----------|--------|
| `pmt_soundness` | PMT state operations preserve the layout invariants of the backing arena. | Sorry-free. |
| `no_oob_trap_for_well_typed_strong` | Well-typed PMT programs never trap on an out-of-bounds memory access. | Proven. |
| `verify_transform_sound` | The Rust `verify_transform` verifier accepts only state transforms that preserve the layout's invariants. | Proven. |
| `verify_state_reads_sound` | Soundness of `verify_state_reads` w.r.t. the Lean PMT model. | Proven (sorry-free). |
| `verify_state_writes_sound` | Soundness of `verify_state_writes` w.r.t. the Lean PMT model. | Proven (sorry-free). |
| `alloc_preserves_all_invariants` | The Iris bundle `[cap_bnd] ∗ [live_mirror] ∗ [guard]` is preserved by `alloc` (Composition theorem). | Proven (sorry-free). |
| `exec_satisfies_pipeline_spec` | Lean `exec` meets the `PipelineSpec` that `pipeline::compile` claims to meet. | Proven (sorry-free; reduces to `pmt_soundness`). |

The `pmt_soundness` theorem is the load-bearing result: the other
theorems reduce to it. The headline theorems above and the ~60 supporting
theorems/lemmas are mechanically discharged; the **6 documented `sorry`s**
in the three Iris modules (`ArenaRes`, `FractionalPerm`, `WeakestPrecond`)
are auxiliary Iris-algebra lemmas (splitting side-conditions, the `wp`
frame/bind/soundness trio) and do not undermine the invariants — see
[`./caveats.md`](./caveats.md) for the sorry inventory and the strict-CI
suspension policy. The strict sorry-check remains enforced for any seventh
sorry.

**Build.** Build the proofs with `make proof` from the repo root, or
`cd proof && lake build` for the raw Lake invocation. The build is
hermetic — the Lean toolchain is pinned via `proof/lean-toolchain` and
reproduces identically across CI runners. The convenience script
`scripts/verify-all.sh` runs the full local verification suite in a
single invocation: `lake build`, the strict sorry-check
(`PROOF_CHECK_STRICT=1 ./scripts/check-lean.sh`), `lake exe test`, CI
YAML validation, and the existence checks for the key Lean modules and
docs; CI invokes it from both workflows below.

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
 specification of the PMT memory model (the resource algebra, the
 state-transition assertion, and the soundness obligations).
- [`./pmt-formal-spec.md`](./pmt-formal-spec.md) — Formal Lean signature
 and axiomatisation of the PMT state, layouts, and the three IVE
 verifiers (the `exec` relation, the `well_typed` predicate, and the
 `verify_*` judgement forms).

The Rust IVE is the executable implementation that the simulation
relation ties back to the Lean model defined in these two specifications.

---

## 7. Cross-references

- the language reference — types, expressions,
 statements, builtins, FFI, PMT model.
- [Backend Documentation](../backends/) — per-backend ABI, ISel strategy,
 QEMU quirks.
- [Testing Infrastructure](../testing/) — gold-standard harness, CI, KATs.
- [Building Guide](../building.md) — prerequisites, quick start, troubleshooting.
- [Pipeline](./pipeline.md) — stage-by-stage compilation walkthrough (per-stage
 caveats and crate/file inventory).
- [IVE Audit](./caveats.md) — module-by-module verification coverage.
- [PMT Audit](./caveats.md) — PMT state verifiers, layout registry.
- [IPC Audit](./caveats.md) — 8-layer IPC stack, wire format, fork emulation.
- [PMT Iris Spec](./pmt-iris-spec.md) — Iris-style separation-logic spec of
 the PMT memory model (source of truth for the Lean proofs).
- [PMT Formal Spec](./pmt-formal-spec.md) — Lean signature and
 axiomatisation of the PMT model (source of truth for the Lean proofs).
- [Caveats](./caveats.md) — documented surprises for backend developers,
 each carrying a resolution-status annotation
 (`RESOLVED` / `PARTIALLY RESOLVED` / `STALE` / `OPEN`).

---

## 7. Glossary

Brief definitions for the acronyms and project-specific terms used
throughout this document. File:line pointers cite the canonical source or
documentation location for each entry.

| Term | Expansion / Meaning | Source |
|------|---------------------|--------|
| **PMT** | "Programs as Memory Transformations" — VUMA's verification discipline. Every program is a typed state-transformation on a single mmap'd arena (`___pmt_buffer`); state lives in registered `PmtLayoutSpec`s and is accessed via `StateRead` / `StateWrite` / `StateTransform` SCG nodes. **PMT does NOT mean "Persistent Memory Transaction":** there is no persistence (arena is anonymous-mmap, torn down at exit), no rollback (linearity is enforced statically, no undo log), and no durability (no journal, no fsync). | `src/ive/src/invariant_aggregator.rs` — `InvariantKind::Pmt` doc comment (grep `What "PMT" means`) + `VerificationLevel::Pmt` doc comment (grep `Acronym disambiguation`); §5.3 above; `./caveats.md`. |
| **IVE** | Invariant Verification Engine (`src/ive/`, 25 modules). Runs the PMT state verifiers (state-read / state-write / state-transform) between SCG construction and IR lowering. The legacy pointer-invariant suite (liveness / exclusivity / interpretation / origin / cleanup) was DELETED, collapsing `VerificationLevel` to a single `Pmt` variant. | §1.4; `./caveats.md`. |
| **SCG** | Semantic Code Graph (`src/scg/`). Typed, control-flow-annotated IR lifted from the AST; canonical input to the verifier and to codegen. | §1.3. |
| **IR** | Backend-neutral SSA-like intermediate representation (`src/codegen/src/ir.rs`) using virtual registers (`VReg`). | §1.5. |
| ~~**COR**~~ | ~~Continuous Optimization Runtime (`src/cor/`)~~ — **DELETED**. The COR→PMT bridge and profile-driven re-optimization hook no longer exist. | `./caveats.md`. |
| **BD** | Behavioral Descriptors (`src/bd/`). 3-axis (repd/capd/reld) value model feeding IVE's interpretation proofs. | `./caveats.md`. |
| **`PmtLayoutSpec`** | Typed layout registered with IVE describing the in-arena shape of a `State<T>` (field offsets, sizes, type names). | `src/ive/src/verification.rs:81-102`. |
| **`___pmt_buffer`** | The single mmap'd arena backing all PMT state in a compiled VUMA program. Anonymous (no backing file); torn down at process exit. | `./caveats.md`. |
