# VUMA Compilation Pipeline

**Stage:** architecture
**Audience:** compiler engineers, security reviewers
**Cross-refs:** `./caveats.md`, `./overview.md`

This document walks the 10 stages a VUMA source file traverses from text to
ELF/Wasm binary. For each stage we name the responsible crate and file, the
transformation performed, and the most load-bearing caveat. Honesty about
limitations is prioritised over marketing language.

> **Build-time hard dependency.** The `vuma-ive` crate statically links
> against the system `libz3` (`src/ive/Cargo.toml` declares
> `z3 = "0.20"`). Without `libz3-dev` installed (`apt install libz3-dev`
> on Debian/Ubuntu), `cargo build` fails at link time. Z3 *is* the
> verifier now — it replaced the Lean FFI bridge that was deleted. See
> [`./caveats.md` §1.1](./caveats.md).

---

## 1. Lexing & Parsing

**Crate:** `vuma-parser` (`src/parser/`).
**Files:** `lexer.rs` (3 030 LOC), `parser.rs` (7 751 LOC), `error.rs`,
`to_scg.rs` (4 831 LOC), `resolver.rs`.

**Lexer.** Hand-written scanner producing `(Token, Position)` pairs
(`lexer.rs:30, 67`). Token kinds in `TokenKind` (`lexer.rs:114`). The
lexer never halts on the first error: it emits `TokenKind::Error` tokens
and continues, accumulating diagnostics (`lexer.rs:5-6`). Literals
include integer, float, address (`@hex`), string, byte-string,
raw-string, char.

**Parser.** Recursive descent for items/statements; **precedence
climbing** for expressions (`parser.rs:4, 2326-2339`). Precedence table
(`parser.rs:2328-2338`):

```
0 || 1 && 2 == != 3 < <= > >=
4 | 5 ^ 6 & 7 << >>
8 + - 9 * / %
```

Nesting is bounded by `max_depth` (`parser.rs:2341-2351`).

**Error recovery.** Three-tier strategy in `error.rs:529-544`:
(1) `SkipToStatementBoundary` — skip to next `;`/`}` (default);
(2) `SkipToBlockBoundary` — skip to next `}` (cascading-block errors);
(3) `InsertMissingToken`/`SkipOneToken` — single-token fixes.
Item-level recovery (`parser.rs:3939-3965`) skips stray tokens until an
`ITEM_STARTERS` keyword is reached, allowing multiple errors per file.

**AST → SCG bridge.** `to_scg.rs:466` `convert_item` is the lowering
entry. Layout/transform items are partially lowered (`to_scg.rs:644-700`).

---

## 2. AST → SCG Lowering

**Crate:** `vuma-parser` (`to_scg.rs`); consumed by `vuma-scg`.

The `AstToScg` converter walks `ast::Program` and emits a Semantic Compute
Graph (`SCG`) of typed nodes (`to_scg.rs:466`). Each AST item maps to one
SCG region: `FnDef` → region with entry/exit nodes (`to_scg.rs:706`);
`StructDef`/`EnumDef` → a `Computation` node carrying a descriptive label;
`TransformDef` is rewritten as a first-class `FnDef` (`to_scg.rs:683-700`).

**What gets lost in translation.** `Item::LayoutDef` is lowered to a
single `Computation` node with a stringified label and the comment
`// TODO`; the structured field types and offsets are
discarded (`to_scg.rs:644-672`). The pipeline re-walks the AST in
`build_pmt_layout_specs` (`pipeline.rs:8852`) and attaches the result
to `VerificationInput::pmt_layouts` (`pipeline.rs:8840`) — a
dual-path representation, a known parallel-development artefact.
Span fidelity is best-effort: `Span` is converted to `ProgramPoint`
via `span_to_pp`, but block IDs and source ordering are recomputed from
node IDs (`to_scg.rs:2958`), not from original AST positions.

---

## 3. SCG Validation

**Crate:** `vuma-scg` (`src/scg/src/`). Structural checks executed before
IVE: region closure (every `NodeId` referenced by an edge is in some
`SCGRegion`), dominance soundness (`dominance.rs`), loop-header
invariants (`loop_detection.rs`), and callgraph acyclicity for inline
cost estimation (`callgraph.rs`). The `SCG` is immutable after
construction; mutators go through `transform.rs`.

---

## 4. IVE Verification (Z3-discharged contracts)

**Crate:** `vuma-ive` (`src/ive/`). Z3 is a **hard build-time dependency**
(`src/ive/Cargo.toml`: `z3 = "0.20"`); without `libz3-dev` on the host,
`cargo build` fails at link time.

**Entry:** `InvariantAggregator::new.with_level(IveVerificationLevel::Pmt)`
(`pipeline.rs:5684-5685` in `compile_with_path`; `pipeline.rs:7155-7156`
in `compile_with_recovery`; `pipeline.rs:6705` in `compile_modules`; and
`main.rs:1606` `verify_pmt_on_ast` for the direct emit/build/run path).

VUMA 2.0 is **PMT-only**: the `VerificationLevel` enum has a single `Pmt`
variant (`invariant_aggregator.rs:158-173`), so the historical "silent
coercion under `cfg(not(test))`" mechanism no longer exists — there is
nothing to coerce. `with_level` is a no-op retained for API stability
(`invariant_aggregator.rs:602-606`), and `invariants_for_level` always
returns `vec![InvariantKind::Pmt]` (`invariant_aggregator.rs:747-749`).
The PMT level runs **exactly three state verifiers**:

| # | Verifier | What it checks |
|---|----------|----------------|
| 1 | `state_read` (`state_read.rs:44`) | Field exists, `offset+size ≤ total_size`, read type matches declared type. |
| 2 | `state_write` (`state_write.rs:56`) | Same as state_read plus **linearity**: no write to a state already consumed by a `StateTransform` or `ForeignConsume`. |
| 3 | `state_transform` (`state_transform.rs:57`) | Both layouts registered; same-size reinterpret valid; different-size copy valid (compiler emits Alloc+Store). |

The five legacy pointer invariants (liveness, exclusivity, interpretation,
origin, cleanup) were **DELETED** from `InvariantKind` — they are not
"skipped", they no longer exist. Pointer syntax is a hard parse error in
VUMA 2.0, so there is nothing for them to verify.

**Z3-based contract discharge.** Every memory-safety obligation the
verifiers identify is emitted as a `contract_assert(…)` whose body is a
first-order formula over the program's SSA state (vreg offsets, layout
sizes, linear-token status, information-flow labels, session-type
states). **Z3 discharges the contract** at compile time. The current
discharge rate is 100 % on the gold-standard suite: all curated test matrix / curated test matrix
tests pass on all 19 backends with zero outstanding `contract_assert`
failures. When Z3 cannot discharge a contract (genuine
memory-safety violation), the pipeline hard-fails with
`VumaError::Verification`. There are no `WARNING + TODO` stubs for
deferred contract discharge — Z3 either proves the contract or the
build fails.

**Session-type verifier uses real vregs.** The session-type verifier
(`src/ive/src/session_type.rs`) maintains a per-vreg session-state map
keyed on the *actual SSA vreg* of each channel-typed binding — not a
hardcoded `vreg=0` placeholder. Each `ChannelOpen` / `ChannelSend` /
`ChannelRecv` / `ChannelClose` event emits a `contract_assert(…)`
asserting the channel's session type is in the expected state; Z3
discharges the assertion.

**Information-flow verifier uses real labels.** The information-flow
verifier (`src/ive/src/information_flow.rs`) tracks a real
`SecurityLabel` lattice (`Public` / `Internal` / `Secret` / `TopSecret`)
per vreg — not a hardcoded `Public` placeholder. Every `FlowKind` event
(Assign, BinOp, ChannelSend, Branch-implicit-flow) emits a
`contract_assert(src_label.can_flow_to(dst_label))` (or
`can_flow_to(join(lhs, rhs))` for BinOp, or per-branch-var
`can_flow_to(cond_label)` for implicit flows) that Z3 discharges.

**Hard-fail policy.** A failed PMT verifier
sets `OverallVerdict::Fail` and the pipeline returns
`VumaError::Verification` immediately (`pipeline.rs:5706-5708`). An
unverified invariant sets `OverallVerdict::Inconclusive`
(`invariant_aggregator.rs:320-328`) — and **Inconclusive now HARD-FAILS
by default** (`pipeline.rs:5713-5716`, `pipeline.rs:6724`,
`pipeline.rs:7203`). The `--allow-inconclusive` CLI flag
(`CompileConfig.allow_inconclusive`, plumbed at `main.rs:1567`) is the
sole opt-out: when set, the pipeline logs a `SOUNDNESS WAIVER` and
soft-passes (`pipeline.rs:5721-5732`). The same gate is mirrored in the
direct path's `verify_pmt_on_ast` (`main.rs:1643-1660`).

**Advisory verifier and `--strict-ive`.** Only **one** verifier remains
advisory by default — `bv_verify`. The linear-channel gate was promoted to
UNCONDITIONAL HARD-FAIL (see the next subsection). Three other verifiers —
`l1l3_collapse_from_ir`, session-type, information-flow — are
**unconditional HARD-FAIL**.

| Verifier | Call site | Default | Under `--strict-ive` |
|----------|-----------|---------|---------------------|
| `bv_verify` (e-graph soundness) | `pipeline.rs:5317` (in `run_ir_pipeline`) | `vuma_log!(warn)` only | `VumaError::Transform { pass_name: "bv_verify", ... }` HARD-FAIL |

The `--strict-ive` flag (`CompileConfig.strict_ive`, parsed at
`main.rs:708-733`, plumbed at `main.rs:1568`) promotes `bv_verify` to a
hard failure. See `pipeline.rs:5299-5337` for the bv_verify gate
comment and `pipeline.rs:5328` for the `VumaError::Transform` raise
under `--strict-ive`.

**Linear-channel gate — UNCONDITIONAL HARD-FAIL.**
The `verify_linear_channels` gate is no longer advisory. Any non-empty
result from `vuma_ive::borrow_region::verify_linear_channels` aborts
compilation with `VumaError::Transform { pass_name: "linear-channel",
... }`, regardless of `--strict-ive`. The two call sites are
`pipeline.rs:5973` (`compile_with_path`) and `pipeline.rs:7399`
(`compile_with_recovery`); both share the same UNCONDITIONAL raise
(comments at `pipeline.rs:5983-5990` and `pipeline.rs:7409-7416`).
`--strict-ive` is retained only for `bv_verify` (Stage 7a) which still
has the "reserved for future strict mode" advisory status; the help
string at `main.rs:549` documents this split.

**Linear-channel call-site false positive — FIXED.** The linear-channel
verifier previously emitted spurious "use of uninitialized channel" /
"channel_close on uninitialized" warnings on any program with more than
one channel operation, because the call site used the SCG node index as
the channel `vreg` identifier instead of the handle's variable name.
`ChannelEvent.vreg` was changed from `u32` to `String` in
`ive/src/borrow_region.rs` and the verifier's state map was re-keyed on
the handle name extracted from `ChannelOpenNode.dst` /
`ChannelSendNode.channel` / `ChannelRecvNode.channel` /
`ChannelCloseNode.channel` (`scg/src/node.rs:948-997`). With the FP
gone, the unconditional promotion was safe: the gate aborts only on
genuine linear-channel violations (use-after-close, double-close,
use-without-open). Regression test:
`tests/linear_channel_hard_fail.rs::linear_channel_use_after_close_fails_by_default`.

**See `./caveats.md`** for the full trustworthiness assessment.

### 4.1 Formal specification (Lean 4 — standalone)

The PMT memory model is also mechanised in **Lean 4** under `proof/`.
The Lean development is the **formal specification** of the PMT model:
it defines the arena, layout, linear-token, and information-flow
predicates and proves the corresponding soundness theorems. The Lean
proofs are machine-checked (`lake build` passes; sorry-audit by
`scripts/check_lean.sh`) but they are **not linked into the compiler
binary**. Build-time verification goes through Z3 and the hand-written
Rust verifiers in `src/ive/`; the Lean proofs document *what* is being
checked, not *how the binary checks it*.

The current proof status of the three PMT state verifiers is:

| # | Rust verifier | Lean theorem | Status |
|---|---------------|--------------|--------|
| 1 | `verify_state_reads` (`state_read.rs:44`) | `verify_state_reads_sound` | **Stated** — statement in `proof/PMT/`; proof pending. |
| 2 | `verify_state_writes` (`state_write.rs:56`) | `verify_state_writes_sound` | **Stated** — statement in `proof/PMT/`; proof pending. |
| 3 | `verify_transform` (`state_transform.rs:57`) | `verify_transform_sound` | **Proven** — sorry-free; reduces to `pmt_soundness`. |

The proven `verify_transform_sound` theorem states that whenever the
Rust `verify_transform` verifier accepts a `StateTransform` node, the
Lean `exec` relation for that transform preserves the layout's
invariants — i.e. the verifier is sound w.r.t. the operational
semantics. The two `state_read` / `state_write` soundness theorems are
stated with the same shape but their proofs are not yet
closed; until they are, the *acceptance* of those two verifiers rests on
the Rust-side audit in `./caveats.md` (and on the Z3-discharged
contracts that the executable verifier emits), not on a Lean proof.

The broader OOB-safety result `no_oob_trap_for_well_typed_strong`
(proven, sorry-free) composes with `verify_transform_sound` to
give the end-to-end guarantee that a well-typed program accepted by the
PMT IVE never traps on an out-of-bounds memory access.

**Lean↔Rust simulation (CompCert-style).** `proof/PMT/PipelineSim.lean`
provides the **first mechanical simulation theorem connecting Lean
`exec` to the Rust `pipeline::compile` specification**. Following the
CompCert translation validation approach (Leroy, JAR 2009), the module
introduces a `PipelineSpec` structure that captures what
`src/pipeline.rs::compile` promises to produce (a binary whose observable
behavior matches Lean `exec` and which is safe — canonical trap codes 1,
134, 135 only, no undefined behavior), and proves three load-bearing
results:

- `exec_satisfies_pipeline_spec` — Lean's own `exec` already meets the
  `PipelineSpec` (sorry-free, reduces to `pmt_soundness`).
- `pipeline_compile_sound` — conditional on the translation-validation
  hypothesis `hconforms: PipelineSpec prog s`, the compiled binary's
  behavior is safe.
- `pipeline_compile_no_oob` — under the same hypothesis, the compiled
  binary never traps on an out-of-bounds access for well-typed
  programs.

The `hconforms` hypothesis is the Rust-side translation-validation
obligation: it is discharged empirically by the parity test
`tests/pmt_parity_test.rs` (5 tests), which verifies that the
hand-translated Rust PMT checkers in
`src/codegen/src/runtime/pmt_check.rs` (gated by the
`pmt-runtime-check` cargo feature) match the Lean definitions on all
test cases. The two `state_read` / `state_write` soundness theorems
are still stated-but-pending as in the table above; `PipelineSim` does
not close them, but it does provide the *contract* against which their
eventual discharge will be measured.

> The Lean FFI bridge that previously linked Lean-verified checkers
> into the binary has been **deleted**. There is no `lean_stub.c`,
> no `lean_ffi_linked` cfg, no `lean_verify_*` extern surface. Z3 +
> the hand-written Rust verifiers do the executable verification; the
> Lean proofs remain as the formal specification only.

**Cross-references for the Lean development of this stage:**

- [`./pmt-formal-spec.md`](./pmt-formal-spec.md) and
  [`./pmt-iris-spec.md`](./pmt-iris-spec.md) — the formal specifications
  the Lean proofs are checked against.
- [`./caveats.md` §3](./caveats.md) — the explicit statement that Lean
  proofs are a standalone artifact, not linked into the binary.

---

## 5. Memory Safety Analysis

**Crate:** `vuma-codegen` (`src/codegen/src/memory_safety.rs`).
**Entry:** `MemorySafetyAnalyzer::new(ms_config)` where `ms_config` is
selected by `config.runtime_bounds_checks` (`pipeline.rs:6070-6074`).

Ten violation kinds E041–E050 (`memory_safety.rs:9-18`): eight compile-time
(UAF, double-free, leak, null-deref, dangling, uninit-read, use-after-scope,
invalid-free) and **two runtime** (bounds-check E044, buffer-overflow E048).
The analyzer is a **HARD gate** in VUMA 2.0: `--no-memory-safety` was
removed (passing it is a hard parse error, `main.rs:759-761`), and any
non-clean report returns `VumaError::MemorySafety` (`pipeline.rs:6077-6080`).

**Runtime bounds checks are ALWAYS ON.** `safe: true` is hard-coded in the
default `Cli` (`main.rs:610`); the `--safe` flag has been **removed** from
the CLI surface (see `./caveats.md` §5.1). `cli.safe` always resolves to
`true`, which is plumbed into
`CompileConfig.runtime_bounds_checks` (`main.rs:1570`). The pipeline
conditionally selects `MemorySafetyConfig::safe_mode`
(`memory_safety.rs:277`) vs `compile_time_only`
(`memory_safety.rs:290`) at `pipeline.rs:6070-6074` — in production the
`safe_mode` branch is always taken. There is no way to disable
runtime bounds-check injection; the previous `--safe` flag is no longer
accepted.

**Runtime bounds-check IR injection.** When
`config.runtime_bounds_checks` is set (which is always), the pipeline calls
`find_bounds_check_sites_with_bounds` (`memory_safety.rs:823`) on the
codegen SCG, then `inject_bounds_check_ir` (`memory_safety.rs:1014`,
invoked at `pipeline.rs:6139`) mutates the SCG to prepend a
`ComputationNode(UGe)` + `ControlNode::If { __oob_trap }` pair before
every bounded `Seq` access. The `__oob_trap` extern stub (exit 134)
exists on all 19 backends (`memory_safety.rs:1000`). Stack allocations
and PMT state buffers are fully bounded by their `AllocationNode::Stack.size`.

**Arena state-pointer bounding.**
`build_arena_state_sizes` (`memory_safety.rs:1239`) pattern-matches the
`arena_alloc` IR sequence (anchored on `__arena_overflow` calls) to
recover `state_ptr → layout_size` pairs, which are merged into
`alloc_sizes` before `inject_bounds_check_ir` runs. This causes
per-access `__oob_trap` checks to be emitted for `state_ptr + offset`
through arena-allocated state buffers — previously these classified as
`Wild` and were skipped. Raw pointer arithmetic / extern pointers still
have `length_expr == None` and are skipped (future SoftBound work).

**Liveness (UAF) trap injection.** `inject_liveness_check_ir`
(`pipeline.rs:6148`) emits a Load + Eq + If that traps via `__uaf_trap`
(exit 135) before every `Seq` access through a `state_new` allocation;
each such allocation is grown by +1 byte at AST→SCG bridge time to hold
a LIVE/DEAD flag at `[ptr + total_size]`.

**Arena overflow trap.** The Rust-level arena runtime
(`src/codegen/src/runtime/arena.rs`) terminates the process via
`arena_overflow_trap` (`arena.rs:107`), which calls
`std::process::exit(1)` — mirroring the codegen-emitted `__arena_overflow`
stub that every backend lowers to `exit(1)` (e.g. x86_64 `sys_exit`
code=1, aarch64 `svc #0` with X8=93/X0=1). This module previously used
`std::process::abort` (SIGABRT, exit 134), which diverged from the codegen
trap contract. Aligning both paths keeps the Iris spec
(`wp (call __arena_overflow) { _, False }`) faithful. See `arena.rs:23-34`
for the full trap-semantics note. Integration tests live in
`tests/arena_overflow_trap_tests.rs`.

**See `./caveats.md`** for the "no buffer overflow" faithfulness audit.

---

## 6. IR Construction

**Crate:** `vuma-codegen` (`scg_to_ir.rs` 8 191 LOC).
**Entry:** `IrLowerer::convert(&mut self, scg: &Scg) -> Result<IRProgram>`
(`scg_to_ir.rs:977`). The lowerer maintains `vreg_types: HashMap<u32,
IRType>` (`scg_to_ir.rs:893`) populated at every `Alloc`, `Load`, `Call`,
`Cast`, and `BinOp` site (`scg_to_ir.rs:3265, 3297, 3466, 3685-3739,
4133-4310`). Type propagation is forward-only; type queries fall back to
a per-function `fn_var_types` map (`scg_to_ir.rs:4133`). When no type can
be inferred, the lowerer defaults to `IRType::Ptr` (`scg_to_ir.rs:3268,
3303, 3739, 4301`) — a sound but imprecise fallback. SCG `StateInit`/
`StateRead`/`StateWrite`/`StateTransform` are lowered to IR `Alloc`+
`Load`+`Store` against the backing arena `___pmt_buffer` (see Stage 7
for the arena-overflow instrumentation).

---

## 7. IPC Lowering

**Crate:** `vuma-codegen` (`ipc_lowering.rs` 3 822 LOC + `ipc.rs`
8 700 LOC). **Entry:** `lower_ipc_builtins(func, backend)` invoked from the pipeline after IR construction. 35+ builtins are expanded into IR instructions: `channel_open` →
`pipe2` (nr 59, `ipc_lowering.rs:902`) returning an 8-byte
heap-allocated `(read_fd, write_fd)` pair **as a pointer**
(`ipc_lowering.rs:908-914`; the older I64-packing lost high bits on
32-bit backends). `spawn_worker` → `clone` (nr 220, SIGCHLD=17)
(`ipc_lowering.rs:1001-1011`). On `wasm32`, `clone` is unavailable; the
backend's `Syscall{220}` handler returns 0 (child branch), and
`wasm32_fork_emulation_pass` (`ipc_lowering.rs:232`) rewrites the
child's `Return` to `Store(exit_val, 4096); Jump(parent_post)`. Both
branches run sequentially in-process — **not real isolation**.

**Two-pipe channel architecture (the nanosleep hack is gone).** Each
channel end is a 16-byte handle holding **4 file descriptors**: the
parent→child pipe (read+write ends) and the child→parent pipe (read+write
ends). Send and recv touch *different* pipes, so the previous
single-pipe design — and its `nanosleep`-based send/recv race
workaround — has been **removed entirely**. There is no
`nanosleep` call anywhere in the channel runtime; the two-pipe design
eliminates the race by construction (a sender writes to the
parent→child pipe; a reader reads from the same pipe; the kernel's
pipe buffer handles synchronization). See `./caveats.md` §2.3 for the
full two-pipe handle layout and the half-closed-channel semantics.

L1 wire frame (`ipc.rs:106-136`):

```
[0..4] magic [0x56,0x55,0x4D,0x41] = "VUMA"
[4..6] version u16 LE (=2)
[6..8] flags u16 LE
[8..16] channel_id u64 LE
[16..24] sequence u64 LE
[24..32] type_hash u64 LE (FNV-1a 64)
[32..40] payload_len u64 LE
[40..44] cap_count u32 LE
[44..44+payload_len] payload
... capabilities (cap_count × token)
[last 4] crc32 u32 LE (poly 0xEDB88320, matches zlib)
```

`MAX_PAYLOAD_SIZE = 16 MiB` (`ipc.rs:23`). **See `./caveats.md`** for
the 8-layer stack, syscall ABI, and QEMU workarounds.

---

## 8. Optimization

**Crate:** `vuma-codegen` (`opt.rs` 4 817 LOC).
**Entry:** `run_optimizations_inner(program, latency_table, profile,
inline_threshold)` (`opt.rs:1984`).

Per-function passes (`opt.rs:2007-2044`), in order:

1. `constant_fold` (`opt.rs:849, 2011`) → 2. `cse` (`opt.rs:1170, 2012`)
 → 3. `equality_saturation_with_cost` (e-graph; `opt.rs:2013`) →
 4. `mark_ive_proven_nonaliasing` (`opt.rs:2014`) →
 5. `dead_store_eliminate` (`opt.rs:2015`) →
 6. `dead_code_eliminate` (`opt.rs:1078, 2016`) →
 7. `inline_with_threshold` (`opt.rs:1321, 2020`) →
 8. `constant_fold`+`dead_code_eliminate` (`opt.rs:2021-2022`) →
 9. `licm` (`opt.rs:1632, 2029`) →
 10. `constant_fold`+`dead_code_eliminate` (`opt.rs:2030-2031`) →
 11. `scheduler::schedule_function` (`opt.rs:2039`) →
 12. `dead_code_eliminate` (`opt.rs:2042`)

Whole-program passes (`opt.rs:2046-2077`):

13. `cross_function_constant_prop` (`opt.rs:2051`) → 14. `constant_fold`
 +`dead_code_eliminate` (per fn; `opt.rs:2054-2055`) →
 15. `identical_function_merge` (ICF; `opt.rs:2063`) — wired,
 defined (`opt.rs:2059-2062`) → 16. `whole_program_dce`
 (`opt.rs:2065`) → 17. `loop_unroll::unroll_loops` (`opt.rs:2067`) →
 18. `materialize_f32_immediates` (`opt.rs:630, 2076`) — load-bearing
 hack, must run after folding and before codegen (`opt.rs:2070-2077`).

`detect_deadlock` (`opt.rs:2082`) emits warnings only. All passes are
unconditional (`opt.rs:2037`); profile-guided cost via
`run_optimizations_with_profile` (`opt.rs:1910`).

---

## 9. Register Allocation

**Crate:** `vuma-codegen` (`regalloc.rs`, `regalloc_emit.rs`, per-backend
`*/stack_slot_isel.rs`, `*/reg_alloc_isel.rs`). Four register-allocation
strategies are in use:

| Allocator | Backends | Notes |
|-----------|----------|-------|
| **`LinearScanAllocator`** (`regalloc.rs`) | `aarch64` | Real linear-scan; production. |
| **`TargetAgnosticRegAlloc`** (`regalloc.rs`) | `x86_64`, `riscv64`, `ppc64` | Real `TargetDesc`-driven linear-scan. |
| **Stack-slot ISel** (per-backend `*/stack_slot_isel.rs`) | 14 backends: `x86_32`, `loongarch64`, `arm32`, `mips64`, `ppc64le`, `wasm32`, `riscv32`, `armeb`, `aarch64_be`, `sparc64`, `s390x`, `m68k`, `alpha`, `hppa`, `mips64be` | Every vreg materialised to a stack slot; each IR op emits `load lhs → load rhs → op → store`. Correct but ~2–5× slower than the linear-scan backends under register pressure. |
| **LoongArch64 register cache** (`loongarch64/reg_alloc_isel.rs`) | `loongarch64` | "Register cache" that keeps values in physical registers within a basic block, flushing only at block boundaries and before calls. Under QEMU TCG every load/store is 10–100× slower than a register op (`loongarch64/reg_alloc_isel.rs:7-15`), motivating the optimization. A deliberate single-backend performance choice. |

Stack-slot ISel is the *fallback* path on the 14 backends that have not
yet had a complete `TargetDesc` populated. It is no longer the case that
"stack-slot ISel is the only option" — four backends (`aarch64`,
`x86_64`, `riscv64`, `ppc64`) have real linear-scan allocators, and
`loongarch64` has its own register-cache ISel. Wiring the remaining 14
backends up to `TargetAgnosticRegAlloc` requires populating a complete,
validated `TargetDesc` (register classes, caller/callee-saved sets, ABI
register roles, frame layout) per backend; that work is tracked in
`src/codegen/src/target_desc.rs`. See `./caveats.md` §2.1 for the full
per-backend allocator matrix.

---

## 10. Backend Emission

**Crate:** `vuma-codegen` (`emit.rs`, `backend.rs`). Each backend's
`emit_*` pass walks the `AllocatedFunction` stream and produces machine
code + relocations. ELF emission writes a minimal ET_EXEC ELF64/ELF32
with a single PT_LOAD segment, no dynamic linker, and a fixed entry at
the first function emitted. Wasm32 emission (`wasm32/mod.rs`) produces a
wasm32 module with custom `vuma.*` imports (`pipe`, `fork`, `execve`,
`dup2`, `waitpid`, `strcmp`) supplied by `scripts/wasm32_runner.py`.
Big-endian backends (`armeb.rs`, `aarch64_be.rs`, `mips64be.rs`,
`ppc64le.rs`) are thin byte-swap shims — they do not re-implement
instruction encoding.

**Emit path uses `compile_to_binary_direct`, not the canonical pipeline.**
`cmd_emit` (`main.rs:2259`) calls `compile_to_binary_direct`
(`main.rs:1690`) — the same direct AST→codegen bridge path that
`cmd_build_direct` (`main.rs:1977`) and `cmd_run` (`main.rs:2053`) use —
instead of `compile_with_path`. The direct path calls
`backend.encode_program(ir_program)`, which for every backend emits
ISA-specific machine code with a real `_start` stub and exit syscall
wrapper (AArch64: `backend.rs:2838-2896`; x86_64: `x86_64/mod.rs:3908`).
PMT state verification still runs as a mandatory gate inside the direct
path via `verify_pmt_on_ast` (`main.rs:1606`, invoked at `main.rs:1720`).
The full canonical-pipeline IVE suite (Stage 6b memory-safety, Stage 7c
linear-channel, Stage 8b advisory verifiers) is **NOT** run on the emit
path — users who want the full IVE gate should use `vuma build`, which
routes through `compile_with_path` / `compile_with_recovery`.

The previous canonical-pipeline emit path produced broken ELFs (no
`_start` stub, no exit syscall on AArch64; cross-ISA machine-code
mismatch on x86_64) that SIGSEGV'd under QEMU.

**All 19 backends pass the gold-standard suite at 100 %.** The current
matrix is **curated test matrix / curated test matrix = 100.00 %** across all 19 backends
(`x86_64`, `x86_32`, `aarch64`, `aarch64_be`, `arm32`, `armeb`,
`riscv64`, `riscv32`, `mips64`, `mips64be`, `ppc64`, `ppc64le`,
`loongarch64`, `s390x`, `sparc64`, `alpha`, `hppa`, `m68k`, `wasm32`).
The runner is `scripts/pi5_test_suite.sh`; the manifest is
`tests/gold_standard/manifest.json`.

---

## Caveats

The following caveats apply pipeline-wide; per-stage caveats are inlined
above. See `./caveats.md` for the consolidated list.

1. **Runtime bounds checks are ALWAYS ON** (Stage 5; `main.rs:610`
   hard-codes `safe: true`). The `--safe` flag has been removed from the
   CLI surface; it is no longer accepted. `--no-memory-safety` is
   rejected as a hard parse error (`main.rs:759-761`).
2. **PMT ≠ "Persistent Memory Transaction"** — it means "Programs as Memory
   Transformations" (Stage 4; `invariant_aggregator.rs:166-170`).
3. **The five legacy pointer invariants were DELETED**, not "skipped" —
   `VerificationLevel` is a single-variant enum
   (`invariant_aggregator.rs:158-173`); `invariants_for_level` always
   returns `vec![InvariantKind::Pmt]` (`invariant_aggregator.rs:747-749`).
4. **`materialize_f32_immediates` is load-bearing** — reordering it
   corrupts f32 constants on x86_64 (Stage 8; `opt.rs:2070-2077`).
5. **ICF and `whole_program_dce` are wired** (`opt.rs:2059-2063`).
6. **`vuma_log!` is no-op in release** (`lib.rs:36-43` of every core crate)
   — backend advisory output is invisible in `vuma build --release`.
7. **14 of 19 backends use stack-slot ISel**; 4 backends (`aarch64`,
   `x86_64`, `riscv64`, `ppc64`) have real linear-scan; `loongarch64`
   uses its own register-cache ISel (Stage 9).
8. **`syscall_abi::translate`** is wrapped by `translate_or_warn`
   (`syscall_abi.rs:281`), which is the real production caller invoked by
   sparc64, s390x, hppa, riscv64, x86_64, m68k, ppc64. The bare `translate`
   is reachable only through the wrapper.
9. **wasm32 fork is not real isolation** — both branches run sequentially
   in-process (Stage 7; `ipc_lowering.rs:232`).
10. **`find_bounds_check_sites_with_bounds` is WIRED** (Stage 5). Called
    from `compile_with_path` at `pipeline.rs:6104`; `inject_bounds_check_ir`
    (`pipeline.rs:6139`) emits `__oob_trap` IR.
11. **`Inconclusive` HARD-FAILS by default** (Stage 4). `--allow-inconclusive`
    (`main.rs:692`) is the sole opt-out.
12. **`--strict-ive` promotes the remaining advisory verifier to HARD-FAIL**
    (Stage 4). The only advisory gate left is `bv_verify`
    (`pipeline.rs:5317`); the linear-channel gate is UNCONDITIONAL
    HARD-FAIL (`pipeline.rs:5973` and `pipeline.rs:7399`) and no longer
    requires `--strict-ive`.
13. **`cmd_emit` uses the direct path, not the canonical pipeline**
    (Stage 10). Routes through `compile_to_binary_direct`
    (`main.rs:1690`); full IVE suite is NOT run on the emit path.
14. **Z3 is a hard build dependency** (`src/ive/Cargo.toml`:
    `z3 = "0.20"`). The Lean FFI bridge (with `lean_stub.c`,
    `lean_ffi_linked`, `lean_verify_*` externs) has been **deleted**;
    the Lean proofs under `proof/` are the formal specification only
    and are not linked into the binary.
15. **Two-pipe channel architecture** (Stage 7). The previous
    `nanosleep`-based send/recv race workaround is gone — each channel
    end is a 16-byte handle with 4 fds (parent→child pipe + child→parent
    pipe); send and recv touch different pipes, eliminating the race by
    construction.

---

*Document length: updated to reflect the Z3-based IVE contract
discharge, two-pipe channel architecture, four-backend real-regalloc
matrix, and curated test matrix / curated test matrix = 100.00 % gold-standard pass rate; the
Lean FFI bridge is gone, Z3 is the executable verifier; file:line
citations refreshed to HEAD.*
