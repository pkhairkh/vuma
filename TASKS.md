# VUMA 2.0 — Programs as Memory Transformations (PMT)

> **One-sentence pitch:** Stop verifying that pointers are safe. Eliminate pointers from the source language. Make memory safety a type-checking property instead of a verification problem.

This document is the engineering spec for VUMA 2.0. It is organized into **waves** that can be executed by **subagents working in parallel** (where wave dependencies allow). Each wave has a **Definition of Done (DoD)** and a list of **underlying tasks**. Subagent prompts must be **surgical and comprehensive** — see §"Subagent Prompt Guidelines" at the end.

---

## 0. Guiding Principles

1. **No big-bang rewrite.** Every wave ships a compiling, tested compiler. The 19 backends (139k LOC) stay untouched as long as possible — new state-transform nodes lower to existing `IRInstr::Alloc/Store/Load`.
2. **Backward compatibility within a wave.** Old pointer-based `.vuma` programs keep compiling until the wave that removes them. Each wave adds a feature flag or parallel path, verifies, then migrates.
3. **Parallelism by design.** Waves are sequenced by *data dependencies*, not by convenience. Within a wave, tasks touch disjoint files so multiple subagents can work simultaneously without merge conflicts.
4. **Tests are the contract.** Every wave adds gold-standard tests for the new feature and keeps all existing tests passing.
5. **IVE is the oracle.** Until the IVE is rewritten (Wave 7), the existing 5-invariant verifier must stay at 100% on every test. After Wave 7, the new state-compatibility verifier replaces it.

---

## 1. Current Architecture (baseline, from explorer report)

| Crate | LOC | Role | PMT impact |
|-------|-----|------|-----------|
| `vuma-parser` | 20k | AST + lexing; `Stmt::Allocate/Free/Access`, `Expr::Deref/AddressOf`, `Type::Ptr/RegionPtr` | MASSIVE — new syntax |
| `vuma-scg` | 22k | Semantic Code Graph (data-flow); `NodePayload::Allocation/Deallocation/Access`; `region.rs` | HIGH — state-transform nodes |
| `vuma-bd` | 14k | Behavioral Descriptors: `RepD` (layout), `RelD` (temporal), `CapD` (capabilities) | HIGH — RepD becomes primary |
| `vuma-ive` | 17k | 5-invariant verifier (Liveness, Exclusivity, Interpretation, Origin, Cleanup) | HIGH — rewrite to state checks |
| `vuma-codegen` | 152k | IR (`IRInstr::Alloc/Free/Load/Store/Offset`) + 19 backends + e-graph + scheduler | CRITICAL but mechanical |
| `vuma-proof` | 11k | Proof tactics | MEDIUM — new state-compat tactics |
| `vuma-cor` | 11k | Coroutine/runtime support | LOW |
| `vuma-core` | 15k | Pipeline orchestration (`compile_with_path`, `run_ir_pipeline`) | MEDIUM |
| `vuma-package` | 2k | Package manager | NONE |
| `womb/` | 66k | Stdlib (108 `.vuma` files); arena allocator | MASSIVE — migrate to single-buffer |

**Key architectural lever:** `bridge_ast_to_codegen_scg` (~5k LOC, in `src/codegen/src/scg_to_ir.rs`) lowers AST → codegen-SCG → IR. New state-transform AST nodes can lower to **existing** `IRInstr` (Alloc/Store/Load) here, leaving all 19 backends untouched in early waves.

---

## Wave 1 — State Type System Foundation  (sequential, no parallelism)

**Goal:** Add `layout`, `State<T>`, `Ref<T, F>`, and `transform` as *new* AST + parser constructs, alongside (not replacing) the existing pointer syntax. No semantic changes yet — these parse and type-check into a parallel representation that lowers to the existing IR unchanged.

**Rationale:** This is the foundation every later wave depends on. It must be done first and correctly. No parallelism — a single subagent owns the parser/AST/RepD changes end-to-end to avoid interface churn.

### Tasks

1. **Extend AST** (`src/parser/src/ast.rs`): add `Item::Layout(LayoutDef)`, `Stmt::Transform(TransformStmt)`, `Expr::StateInit/StateRead/StateWrite`, `Type::State(Box<Type>)`, `Type::Ref(Box<Type>, Box<Type>)` (Ref<State, Field>). Add `LayoutDef { name, fields: Vec<(String, Type)> }`. Do NOT remove any existing pointer constructs.
2. **Extend parser** (`src/parser/src/parser.rs`): parse `layout Name = { ... }`, `transform name(s: State<T>) -> State<U> { ... }`, `State<T>` / `Ref<T, F>` type syntax, `state.field` access. Mirror the existing `struct`/`fn` parse paths. Add clear error messages.
3. **Extend RepD** (`src/bd/src/repd.rs`): add `RepD::State(LayoutId)` and `RepD::Ref(LayoutId, FieldId)` variants. Add a `LayoutId`/`FieldId` newtype. Add `LayoutRegistry` (maps `LayoutId → LayoutDef` with computed size/alignment). RepD's existing size/offset inference must handle these.
4. **Extend SCG node payloads** (`src/scg/src/node.rs`): add `NodePayload::StateInit/StateRead/StateWrite/StateTransform`. These are semantic-only for now (they carry layout info); the codegen bridge (Wave 2) will lower them.
5. **AST→SCG bridge** (`src/scg/src/` wherever AstToScg lives): lower new AST nodes to new SCG nodes. No codegen lowering yet.
6. **Add 10 gold-standard tests** (`tests/gold_standard/pmt_wave1/`): parsing + type-checking only (no execution). Tests assert the parse succeeds and RepD infers the right layout sizes. Example: `layout Point = { x: u32, y: u32 }` → RepD size 8.

### Definition of Done

- [ ] `layout`, `transform`, `State<T>`, `Ref<T,F>` parse without error in the test files.
- [ ] RepD computes correct sizes for all 10 new test layouts (asserted via a `dump_ir`-style tool or a new `dump_layout` tool).
- [ ] ALL existing 5,745 tests still pass at 100% on x86_64 (no regressions).
- [ ] IVE stays at 100%.
- [ ] `cargo build` clean; `cargo clippy` no new warnings.
- [ ] New constructs do NOT yet generate executable code (that's Wave 2) — tests are parse/type-check only.

---

## Wave 2 — Codegen Lowering of State Operations  (sequential)

**Goal:** Lower the new SCG state-transform nodes to existing `IRInstr` so state-based programs *compile and run*, producing correct results. The 19 backends stay untouched.

**Rationale:** Single subagent owns `scg_to_ir.rs` + a new `state_lowering.rs` pass. Must be sequential because the lowering is the contract every backend depends on.

### Tasks

1. **New `state_lowering.rs`** (`src/codegen/src/state_lowering.rs`): a pass that runs after `bridge_ast_to_codegen_scg` and before `IRBuilder::build`. Translates `StateInit`/`StateRead`/`StateWrite`/`StateTransform` SCG nodes into the existing IR:
   - `StateInit(layout)` → `Alloc { size: layout.size }` (one Alloc per state — the "single buffer" is one Alloc per state for now; Wave 8 makes it a single program-wide buffer).
   - `StateRead(ref)` → `Load { addr, offset: ref.field_offset }`.
   - `StateWrite(ref, val)` → `Store { addr, offset, value }`.
   - `StateTransform(in_state, out_layout)` → if sizes match, no-op (reinterpret); if sizes differ, `Alloc` new + `Store` copy.
2. **Wire the pass** into `compile_for_backend_with_path` (`src/bin/compile_dump.rs`) and `compile_with_path` (`src/pipeline.rs`) after SCG transforms, before IR build. Gate behind a `--pmt` flag initially (so old tests unaffected).
3. **Add 15 executable gold-standard tests** (`tests/gold_standard/pmt_wave2/`): programs using `layout`/`transform`/`State` that compute and return a value. Example: a `Point` layout, a transform that swaps x/y, main returns the new x. Expected exit codes asserted via QEMU.
4. **Verify all 19 backends** run the 15 new tests. Since lowering uses only existing `Alloc/Store/Load`, all backends should work.

### Definition of Done

- [ ] 15 new `pmt_wave2` tests PASS on all 19 backends (run via `iso_test.py` or the Pi suite).
- [ ] ALL existing 5,745 tests still pass at 100% on all backends.
- [ ] IVE stays at 100%.
- [ ] `--pmt` flag is off by default; old behavior unchanged when flag absent.
- [ ] No backend files (`src/codegen/src/arm64.rs` etc.) modified.

---

## Wave 3 — IVE State-Compatibility Verifier  (parallelizable: tasks 3a/3b/3c)

**Goal:** Add a NEW verifier (alongside the existing 5-invariant IVE) that proves state-compatibility: every `StateRead`/`StateWrite` references a field whose offset+type is valid for the current state's layout, and every `StateTransform` proves layout compatibility. This is the decidable type-checking that replaces the undecidable pointer proofs.

**Rationale:** Three independent sub-verifiers (read-check, write-check, transform-check) can be built in parallel by 3 subagents, then integrated by a 4th.

### Tasks

- **3a (subagent A):** `StateReadVerifier` — for every `StateRead(ref)`, prove `ref.field_offset + field_size ≤ state_layout.size` and `ref.field_type == read_type`. Uses `LayoutRegistry`. File: `src/ive/src/state_read.rs`.
- **3b (subagent B):** `StateWriteVerifier` — for every `StateWrite(ref, val)`, same offset/type check, PLUS linearity: the state must not be concurrently read elsewhere (states are linear — consumed on transform). File: `src/ive/src/state_write.rs`.
- **3c (subagent C):** `StateTransformVerifier` — for every `StateTransform(in, out)`, prove `in_layout` and `out_layout` are compatible (same size, or explicit copy generated). Uses RepD size inference. File: `src/ive/src/state_transform.rs`.
- **3d (subagent D, after 3a-c):** Integrate into `InvariantAggregator` (`src/ive/src/invariant_aggregator.rs`) as 3 new invariant kinds. Add a `VerificationLevel::Pmt` that runs ONLY the state verifiers (old 5 invariants skipped for `--pmt` programs). Add 20 gold-standard tests that MUST pass state verification (and 5 negative tests that MUST fail it).

### Definition of Done

- [ ] 3 new verifier modules compile and pass their own unit tests.
- [ ] `VerificationLevel::Pmt` runs the 3 state verifiers on `pmt_wave2` tests — all PASS.
- [ ] 5 negative tests (invalid offset, type mismatch, non-linear use) are correctly REJECTED with clear error messages.
- [ ] Existing 5-invariant IVE unchanged and still 100% on old tests.
- [ ] ALL existing 5,745 tests still pass.

---

## Wave 4 — BD: RepD as Primary Type System  (parallelizable: tasks 4a/4b)

**Goal:** Promote RepD from "secondary descriptor" to the primary type representation used by SCG, IVE, and codegen. CapD gains state-access capabilities; RelD gains epoch ordering for state transformations.

### Tasks

- **4a (subagent A):** `RepD` promotion — make `IRType` carry a `RepD` reference instead of duplicating layout info. Update `scg_to_ir.rs` IRBuilder to consult `LayoutRegistry` for all `State`/`Ref` types. Add `RepD::state_size()` / `RepD::field_offset()` as the canonical queries (deprecate ad-hoc size computations). File: `src/bd/src/repd.rs` + `src/codegen/src/scg_to_ir.rs`.
- **4b (subagent B, parallel with 4a):** `RelD` epochs — add `TemporalKind::EpochBefore/EpochAfter` representing state-transformation ordering. A state's epoch is the sequence number of the transform that produced it. Update `src/bd/src/reld.rs`. Add tests proving `State<A>` produced before `State<B>` implies A's epoch < B's.
- **4c (subagent C, parallel):** `CapD` state capabilities — add `Capability::StateRead/StateWrite/StateTransform/StateConsume`. States are consumed (linear), so `StateConsume` is exclusive. Update `src/bd/src/capd.rs`.

### Definition of Done

- [ ] `IRType` carries RepD refs; ad-hoc size code removed or deprecated (grep confirms no new ad-hoc `size_of` outside RepD).
- [ ] RelD epochs type-check: a `StateRead` of an epoch-2 state by an epoch-1 context is rejected.
- [ ] CapD tracks linearity: double-consume of a state is rejected.
- [ ] ALL existing tests + `pmt_wave2/3` tests still pass.
- [ ] IVE (old + new) at 100%.

---

## Wave 5 — E-graph Layout Optimization  (sequential)

**Goal:** Extend the e-graph to optimize memory *layouts*: minimize buffer size, eliminate dead state slots, merge compatible states, reorder transformations to reduce peak memory. This is the "genuinely new power" from the proposal.

**Rationale:** Single subagent — e-graph rewrites are subtle and must be bv_verify-checked. One owner avoids rewrite-rule conflicts.

### Tasks

1. **New e-graph rewrite rules** (`src/codegen/src/egraph.rs`): add ENode variants `StateInit/StateRead/StateWrite` and rewrite rules:
   - **Dead-state elimination:** if a `StateInit`'s result is never read, drop it (like DCE for states).
   - **State merge:** two `StateInit`s with compatible layouts that don't overlap in lifetime → merge into one Alloc.
   - **Transform elision:** `StateTransform(A,A)` (same layout) → no-op.
   - **Peak-memory reorder:** reorder independent transforms so peak buffer usage is minimized (use RelD epochs).
2. **bv_verify the new rules** (`src/codegen/src/bv_verify.rs`): each new rule must pass the bit-vector soundness check. Unsound rules abort compilation (the Wave 36 gate).
3. **Add 10 optimization tests** (`tests/gold_standard/pmt_wave5/`): programs where the optimizer should reduce buffer size or eliminate a state. Assert via a `--dump-layout-opt` flag that prints the post-e-graph buffer size.

### Definition of Done

- [ ] 4+ new rewrite rules added, all bv_verify-passing.
- [ ] 10 optimization tests show the optimizer reduces buffer size or eliminates a state (asserted via `--dump-layout-opt`).
- [ ] No regressions: all existing + PMT tests still pass, IVE 100%.

---

## Wave 6 — Parser: Deprecate Pointer Syntax  (parallelizable: tasks 6a/6b)

**Goal:** Add a `--pmt-only` mode that REJECTS pointer syntax (`allocate`, `free`, `*ptr`, `&x`, `Type::Ptr`). In default mode, pointer syntax emits a deprecation warning. This is the transition wave — old programs still work, new programs are encouraged to be pointer-free.

### Tasks

- **6a (subagent A):** Parser warnings — `allocate`/`free`/`*`/`&` emit `warn: pointer syntax is deprecated; use State<T> and transforms (see --pmt-only)`. File: `src/parser/src/parser.rs`. Warnings are non-fatal.
- **6b (subagent B, parallel):** `--pmt-only` flag — when set, pointer syntax is a hard compile error. Implemented in the parser's error path. Add 5 negative tests that MUST fail to compile under `--pmt-only`.

### Definition of Done

- [ ] Pointer syntax emits deprecation warnings (visible in test output).
- [ ] `--pmt-only` rejects all pointer constructs with clear errors.
- [ ] 5 negative tests pass (correctly rejected).
- [ ] All existing tests still pass (warnings don't break them).

---

## Wave 7 — IVE: Replace 5 Invariants with State Checks  (sequential)

**Goal:** For `--pmt` programs, the IVE runs ONLY the 3 state verifiers (from Wave 3), NOT the 5 pointer invariants. Memory safety is now a type-checking property, not a verification problem. The 5-invariant IVE remains for legacy pointer programs.

**Rationale:** Single subagent — this is the identity-defining change. Must be careful and well-tested.

### Tasks

1. **Pipeline branch** (`src/pipeline.rs`): if `--pmt` (or program detected as PMT — all functions use State), run `VerificationLevel::Pmt` (Wave 3's 3 verifiers) instead of `VerificationLevel::Normal` (5 invariants).
2. **Migrate 50 representative gold-standard tests** to PMT (`tests/gold_standard/pmt_migrated/`): take 50 existing pointer-based tests, rewrite them using `layout`/`State`/`transform`. They must pass under `--pmt` with ONLY state verification.
3. **Document** (`docs/pmt-verification.md`): the new verification model, why it's decidable, the collapsed-invariant table from the proposal.

### Definition of Done

- [ ] 50 migrated PMT tests pass with `VerificationLevel::Pmt` (3 state verifiers, 0 pointer invariants).
- [ ] The 5 pointer invariants are NOT run on PMT programs (confirmed via `--verify` output showing only 3 checks).
- [ ] Legacy pointer tests still pass with `VerificationLevel::Normal`.
- [ ] IVE 100% on both paths.

---

## Wave 8 — Single-Buffer Runtime + womb Migration  (parallelizable: tasks 8a/8b/8c)

**Goal:** Realize the "zero runtime overhead" promise: ONE `mmap` at program start, ZERO `malloc`/`free` during execution. All state transforms are compile-time-verified buffer slicing. Migrate the womb stdlib to PMT.

### Tasks

- **8a (subagent A):** Single-buffer lowering — in `state_lowering.rs` (Wave 2), replace per-state `Alloc` with offsets into ONE program-wide buffer allocated in `_start`. The compiler computes the max buffer size from the state pipeline (peak memory = max simultaneous live states). File: `src/codegen/src/state_lowering.rs` + `src/codegen/src/backend.rs` (runtime stub).
- **8b (subagent B, parallel):** Migrate `womb/alloc/` — rewrite the arena allocator and any `allocate`/`free` users in womb to use `State`/`transform`. File: `womb/alloc/*.vuma`.
- **8c (subagent C, parallel):** Migrate `womb/core.vuma` and 20 representative stdlib files to PMT. Each file: replace pointers with state types.

### Definition of Done

- [ ] `--pmt` programs use exactly ONE `mmap` (asserted via `strace`-style check or a `--dump-buffer` flag showing single buffer).
- [ ] womb/alloc rewritten in PMT; old arena tests still pass (or are migrated).
- [ ] 20 womb stdlib files migrated to PMT, all pass.
- [ ] All existing + PMT tests still pass; IVE 100%.

---

## Wave 9 — Dependent State Types (Dynamic Data Structures)  (sequential)

**Goal:** Support `State<List<N>>` where N is a runtime count. This is the "hard part" from the proposal — dependent type checking restricted to linear arithmetic (sizes, counts, offsets), which RepD already models.

### Tasks

1. **Dependent RepD** (`src/bd/src/repd.rs`): `RepD::DependentArray(elem, count_expr)` where `count_expr` is a linear-arithmetic expression over runtime values. Size inference solves the linear system.
2. **State-transform proofs for dependent types** (`src/ive/src/state_transform.rs`): prove `State<List<N>>` → `State<List<N+1>>` requires the buffer to have room (offset + (N+1)*elem_size ≤ buffer_size). Uses the proof system's linear-arithmetic tactic.
3. **Recursion support**: recursive functions "extend" the stack state by a frame; compiler proves room. Stack overflow becomes a compile-time error (with `#[flat]` escape hatch for unbounded recursion with runtime check).
4. **10 tests**: linked list, dynamic array, stack, queue using `State<List<N>>`.

### Definition of Done

- [ ] 10 dependent-type tests pass (linked list, dynamic array, etc.).
- [ ] State-transform proofs discharge linear-arithmetic obligations automatically.
- [ ] Recursion depth bounded at compile time (or `#[flat]` used).
- [ ] All existing + PMT tests still pass.

---

## Wave 10 — FFI Marshal Pass  (sequential)

**Goal:** Calling C functions requires raw pointers. The marshal pass flattens state references to raw pointers at the FFI boundary and proves the state is not modified by the foreign call (or marks it invalidated).

### Tasks

1. **Marshal pass** (`src/codegen/src/marshal.rs`): at `extern` call sites, convert `State<T>` args to raw pointers (`State<T>` → `*const u8` + size). After the call, the state is either "preserved" (foreign function declared `#[pure]`) or "invalidated" (must be re-initialized).
2. **IVE FFI check** (`src/ive/src/ffi.rs`): prove the foreign function doesn't invalidate a state that's read later (unless re-initialized).
3. **5 FFI tests**: call a C `write()` with a `State<Buffer>` arg; call a C function that modifies a passed buffer (must be marked invalidated).

### Definition of Done

- [ ] 5 FFI tests pass on x86_64 + aarch64.
- [ ] States are correctly invalidated after non-pure foreign calls.
- [ ] All existing + PMT tests still pass.

---

## Wave 11 — Documentation + Language Reference  (parallelizable: 11a/11b)

**Goal:** Update all docs for VUMA 2.0.

### Tasks

- **11a (subagent A):** Rewrite `docs/language-reference.md` for PMT syntax (layouts, transforms, State/Ref types). Add a migration guide from pointer syntax.
- **11b (subagent B, parallel):** Update `docs/architecture.md` — the new pipeline (state-lowering, state-IVE, e-graph layout opts), the single-buffer runtime, the collapsed-invariant table.

### Definition of Done

- [ ] Both docs updated, cross-referenced, with examples.
- [ ] Migration guide covers the 50 migrated tests from Wave 7.

---

## Wave 12 — Full Test Migration + 100% Pass Rate  (sequential)

**Goal:** Migrate ALL 5,745 gold-standard tests to PMT where feasible. Remove pointer syntax from tests that have a natural state-based equivalent. Achieve 100% pass rate on all 19 backends with the full O2 pipeline + state verifiers.

### Tasks

1. **Audit** all 5,745 tests: categorize as (a) natural-PMT (buffers, swaps, parsers), (b) PMT-with-dependent-types (lists, trees), (c) requires-FFI (syscall-heavy), (d) pointer-intrinsic (raw pointer arithmetic tests — keep as legacy).
2. **Migrate** categories (a) and (b) — expected ~3,000 tests.
3. **Run full Pi suite** — target 100% on all 19 backends with both `--pmt` (state verification) and legacy paths.

### Definition of Done

- [ ] ≥3,000 tests migrated to PMT.
- [ ] Full Pi suite: 100% on all 19 backends.
- [ ] IVE 100% (state verifiers for PMT, pointer invariants for legacy).
- [ ] `docs/migration-log.md` records what was migrated and what stayed legacy.

---

## Subagent Prompt Guidelines  (CRITICAL — read before dispatching)

Subagents have **limited context windows** and will time out if overwhelmed. Follow these rules:

1. **One wave = one subagent per task.** Do NOT give a subagent an entire wave. Give it ONE task (e.g., "Task 3a: StateReadVerifier").
2. **Surgical scope.** Tell the subagent EXACTLY which files to touch (full paths), which types to add, which functions to implement. Do NOT say "explore the codebase and decide."
3. **Provide the contract.** Give the subagent the exact struct/function signatures it must implement, the exact test it must pass, and the exact Definition of Done for its task.
4. **Provide the build + test commands.** Always include:
   ```
   source /home/z/.vuma_env; cd /home/z/my-project/vuma
   CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_CODEGEN_UNITS=1 CARGO_PROFILE_DEV_OPT_LEVEL=0 \
   CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_DEV_INCREMENTAL=true CARGO_INCREMENTAL=1 \
   RUSTFLAGS="-C debug-assertions=off -C overflow-checks=off -C strip=symbols" \
   cargo build --profile dev --bin compile_dump 2>&1 | tail -4
   ```
   And the test driver: `python3 /home/z/iso_test.py <test.vuma> <backend>`.
5. **Tell them to read the worklog first.** Every subagent prompt must include: "Read `/home/z/my-project/worklog.md` first to understand prior waves. Append your own section (Task ID, Agent, Work Log, Stage Summary) when done."
6. **Tell them to commit (not push).** The orchestrator pushes after verifying.
7. **Memory constraint.** The sandbox has a 4 GiB RAM cap. Always specify `CARGO_BUILD_JOBS=1`.
8. **No workarounds.** Every prompt must state: "No env gates, no commented-out code, no shortcuts. Fix root causes."
9. **Time-box.** If a subagent times out, the orchestrator picks up the partially-done work from the worklog + git status and re-dispatches a tighter task.
10. **Parallel dispatch.** For waves with parallel tasks (3a/3b/3c, 4a/4b/4c, 6a/6b, 8a/8b/8c, 11a/11b), dispatch ALL parallel subagents in ONE message (multiple Task tool calls) so they run simultaneously.

---

## Wave Dependency Graph

```
Wave 1 (foundation) ──► Wave 2 (lowering) ──► Wave 3 (IVE state verifiers)
                                                  │
                                                  ├──► Wave 4 (BD primary) ──► Wave 5 (e-graph layout opts)
                                                  │
                                                  └──► Wave 7 (replace 5 invariants)

Wave 6 (deprecate pointers) ── depends on Wave 2 (PMT programs must run first)

Wave 8 (single-buffer + womb) ── depends on Waves 2,3,4

Wave 9 (dependent types) ── depends on Waves 4,5

Wave 10 (FFI) ── depends on Wave 8

Wave 11 (docs) ── depends on Waves 1-8 (parallel: 11a/11b)

Wave 12 (full migration) ── depends on ALL prior waves
```

**Parallel opportunities:**
- Wave 3: 3a ∥ 3b ∥ 3c, then 3d
- Wave 4: 4a ∥ 4b ∥ 4c
- Wave 6: 6a ∥ 6b
- Wave 8: 8a ∥ 8b ∥ 8c
- Wave 11: 11a ∥ 11b

---

## Effort Estimate (from proposal, refined by explorer LOC data)

| Wave | Est. effort | Parallelism |
|------|-------------|-------------|
| 1 | 3-4 weeks | sequential |
| 2 | 2 weeks | sequential |
| 3 | 3-4 weeks | 3a∥3b∥3c, then 3d |
| 4 | 2-3 weeks | 4a∥4b∥4c |
| 5 | 2 weeks | sequential |
| 6 | 2 weeks | 6a∥6b |
| 7 | 3-4 weeks | sequential |
| 8 | 4-6 weeks | 8a∥8b∥8c |
| 9 | 3-4 weeks | sequential |
| 10 | 2 weeks | sequential |
| 11 | 2 weeks | 11a∥11b |
| 12 | 4-6 weeks | sequential |
| **Total** | **~5-6 months** | |

---

## Success Criteria (VUMA 2.0 release)

1. **100% pass rate** on all 19 backends with full O2 pipeline.
2. **Memory safety is type-checking** — PMT programs need no pointer-invariant proofs; the 3 state verifiers suffice.
3. **Zero runtime allocation overhead** — one `mmap` per program, no `malloc`/`free` for PMT programs.
4. **IVE 100%** — state verifiers for PMT, pointer invariants for legacy.
5. **All schedulers + optimizers + verifiers + vectorize ALWAYS enabled at O2.**
6. **No regressions** vs VUMA 1.x — legacy pointer programs still compile and verify.

---

*This spec is a living document. Update the DoD checkboxes as waves complete. Record deviations in `/home/z/my-project/worklog.md`.*
