# VUMA 2.0 FFI — Wave-Based Engineering Spec

> **One-sentence pitch:** Build a real FFI for VUMA 2.0 behind a 4-mode matrix (Borrow / Invalidate / Marshal / Foreign-Pass × Scalar / Unmarshal / Foreign-Wrap × optional Callback) that preserves the single-buffer invariant, enables zero-copy I/O, and isolates C's memory model behind a typed boundary.

This document is the engineering spec for the VUMA 2.0 FFI system described in the proposal *"VUMA 2.0 FFI — A Buildable Design (v2)"*. It is organized into **8 waves** designed for **parallel subagent execution** (up to 4 agents at once). Each wave has **surgical tasks** (exact files, exact contracts), a **Definition of Done** with checkable checkboxes, and a **Dispatch Box** containing a ready-to-paste subagent prompt.

**This is greenfield work.** Today's FFI is declarative-only: externs emit `SHN_UNDEF` relocations, but the 3 wave-10 tests never actually call a foreign function, `marshal.rs` is admitted-redundant (64 lines), and `#[pure]` is literally unparsable because `ExternFnDecl` has no `attrs` field.

---

## 0. Guiding Principles

1. **Single-buffer purity is sacred.** `___pmt_buffer` is sized once from the union of all `State<T>`. FFI malloc (scratchpad, callback context) is *foreign memory* — never aliased by `___pmt_buffer`, never tracked by `StateRead`/`StateWrite`/`StateTransform`.
2. **No pointer syntax in source.** `*T`, `&x`, `allocate`, `free` remain hard parse errors. FFI uses `State<T>`, `Address`, layouts, and attributes only.
3. **3 PMT verifiers remain canonical.** FFI safety is *additive* (`ffi` + new `borrow_region` pass), not a replacement.
4. **Reuse existing machinery.** `#[foreign_consume]` linearity reuses `state_write` via synthetic `StateTransformNode`. The `Attribute` parser already exists — Wave 1 just wires it into `ExternFnDecl`/`LayoutDef`.
5. **Tests are the contract.** Every wave adds real gold-standard tests. The DoD for the whole effort requires **real end-to-end libc calls** (not declarative-only tests).
6. **Parallelism by disjoint files.** Within a wave, parallel tasks touch disjoint files so subagents never merge-conflict. The file-ownership map in §3 is the source of truth for what an agent may touch.

---

## 1. Current Architecture (FFI baseline)

| Crate / File | Role | FFI impact |
|---|---|---|
| `src/parser/src/ast.rs` | AST: `ExternFnDecl` (no `attrs`!), `ExternBlockDef`, `LayoutDef` (no `attrs`!), `Param` (no `attrs`), `Attribute` (exists, unused on externs) | **Wave 1** — add `attrs` fields, parse FFI attributes |
| `src/parser/src/parser.rs` | `parse_extern_block` (L1115), `parse_extern_fn_decl` (L1160), `parse_outer_attributes` (L3749) | **Wave 1** — call `parse_outer_attributes` in extern/layout parsing |
| `src/parser/src/to_scg.rs` | AST→SCG bridge | **Wave 2b** — emit `ForeignConsume` SCG node |
| `src/scg/src/node.rs` | SCG node types: `StateTransformNode` (L745), `NodeType::StateTransform` (L82) | **Wave 2b** — add `ForeignConsume` node type |
| `src/ive/src/ffi.rs` | FFI safety verifier (83 lines, `HashSet` intersection) | **Wave 4** — extend to consume `borrow_region` output |
| `src/ive/src/borrow_region.rs` | *(does not exist)* | **Wave 2c** (scaffold) → **Wave 4** (implement) |
| `src/codegen/src/marshal.rs` | Marshal pass (64 lines, admitted-redundant) | **Wave 3a** — rewrite for real marshalling |
| `src/codegen/src/scg_to_ir.rs` | SCG→IR bridge; `___pmt_buffer` sizing (L944, L1101); `IRInstr::Call { is_extern }` (L1290 in ir.rs) | **Wave 3b** (transform hooks) → **Wave 5** (foreign arg lowering) |
| `src/codegen/src/emit.rs` | ELF emission: `SHN_UNDEF` (L180), extern symbol table (L4914) | *(unchanged — already works)* |
| `src/codegen/src/runtime/` | *(does not exist)* | **Wave 2a** (scratchpad) → **Wave 7** (vuma_context, callback) |
| `scripts/wasm32_runner.py` | wasm32 host shim (`make_host_functions`: read_mem/write_mem/read_cstr) | **Precedent for Wave 7** — generalize to C header |
| `tests/gold_standard/pmt_wave10/` | 3 FFI tests (none actually call a foreign fn) | **Wave 6** — real libc tests in new `ffi_wave*/` dirs |
| `docs/language-reference.md` §14, `docs/architecture.md` §9 | FFI docs (binary `#[pure]`/invalidate model) | **Wave 8b** — rewrite for mode matrix |

**Key lever:** `ExternFnDecl` and `LayoutDef` both lack an `attrs` field. The `Attribute` parser (`parse_attribute`, L3781) already exists and is used on `fn`/`enum`/`struct`. Wave 1 is a ~30-line change that unlocks everything.

---

## 2. The FFI Mode Matrix (design reference)

Every foreign call is classified into **exactly one** argument mode per argument, **exactly one** return mode, plus optionally a callback mode. Real C calls use ≥2 modes simultaneously (e.g. `sqlite3_prepare_v2` uses Foreign-Pass + Marshal + Unmarshal + Invalidate).

**Argument modes:** `#[borrow]` (preserved, read-only) · *(default)* invalidate · `#[marshal]` (scratchpad) · `#[foreign(raw)]` on the arg's layout (pass the `raw` field as a C pointer).

**Return modes:** *(default)* scalar · `#[unmarshal(Layout)]` (copy into `___pmt_buffer`) · `#[foreign_return(raw)]` (wrap C pointer into `State<ForeignLayout>`).

**Callback mode:** `#[callback]` + a `vuma_context_t*` param — C may invoke VUMA functions during the call.

Full details in the proposal document. This spec implements the matrix across 8 waves.

---

## 3. File-Ownership Map (parallelism contract)

Agents may **only** touch files in their assigned cell. Cross-cell edits cause merge conflicts.

| Domain | Files | Waves that touch them |
|---|---|---|
| **Parser** | `src/parser/src/ast.rs`, `src/parser/src/parser.rs` | W1 only |
| **SCG** | `src/scg/src/node.rs`, `src/parser/src/to_scg.rs` | W2b only |
| **IVE-borrow** | `src/ive/src/borrow_region.rs` (new), `src/ive/src/lib.rs` | W2c → W4 |
| **IVE-ffi** | `src/ive/src/ffi.rs` | W4 only |
| **Runtime-scratch** | `src/codegen/src/runtime/mod.rs` (new), `src/codegen/src/runtime/ffi_scratch.rs` (new), `src/codegen/src/lib.rs` | W2a only |
| **Marshal** | `src/codegen/src/marshal.rs` | W3a only |
| **SCG→IR** | `src/codegen/src/scg_to_ir.rs` | W3b → W5 (sequential) |
| **Runtime-context** | `src/codegen/src/runtime/vuma_context.rs` (new), `vuma_vm.h` (new) | W7a only |
| **Runtime-callback** | `src/codegen/src/runtime/callback.rs` (new) | W7b only |
| **Tests W1** | `tests/gold_standard/ffi_wave1/*.vuma` | W6a, W6b, W6c |
| **Tests W2** | `tests/gold_standard/ffi_wave2/*.vuma` | W6d |
| **Tests W4** | `tests/gold_standard/ffi_wave4/*.vuma` | W8a |
| **Docs** | `docs/language-reference.md`, `docs/architecture.md` | W8b |
| **Makefile** | `Makefile`, `justfile` | W6a (test runner target) |

**Conflict rule:** within a wave, parallel tasks must have disjoint rows. Across waves, sequencing (§6 dependency graph) ensures a file is never touched by two waves simultaneously.

---

## Wave 1 — Parser Attrs Foundation  (sequential, 1 agent — MUST be first)

**Goal:** Make FFI attributes parseable on extern fn declarations and layout declarations. Today `ExternFnDecl` and `LayoutDef` have no `attrs` field, so `#[pure]`/`#[borrow]`/`#[foreign(raw)]` etc. are literally unparsable.

**Rationale:** Every later wave depends on attributes being in the AST. This is the foundation — no parallelism, single agent owns parser/AST end-to-end to avoid interface churn. ~30-60 lines of real change.

### Tasks

1. **Add `attrs` to `ExternFnDecl`** (`src/parser/src/ast.rs:395`): add `pub attrs: Vec<Attribute>`. The `Attribute` struct already exists (L54: `is_inner`, `name`, `value`, `items`/`list`).
2. **Add `attrs` to `LayoutDef`** (`src/parser/src/ast.rs:430`): add `pub attrs: Vec<Attribute>`.
3. **Call `parse_outer_attributes` in `parse_extern_fn_decl`** (`src/parser/src/parser.rs:1160`): insert `let attrs = self.parse_outer_attributes()?;` at the top, before `expect(TokenKind::Fn)`. Store in the returned `ExternFnDecl`.
4. **Call `parse_outer_attributes` in layout parsing** (`src/parser/src/parser.rs`, wherever `layout Name = { ... }` is parsed): same pattern, store in `LayoutDef`.
5. **Recognize the 8 FFI attributes** (validation only — just store them; semantic handling is later waves). The attributes are:
   - On extern fn params: `#[borrow]`, `#[marshal]`, `#[may_retain]`
   - On extern fn return: `#[unmarshal(Layout)]`, `#[foreign_return(raw)]`
   - On extern fn itself: `#[callback]`, `#[foreign_consume(raw)]`
   - On layouts: `#[foreign(raw)]`
   Validation: if an attribute is on the wrong target (e.g. `#[foreign(raw)]` on an extern fn), emit a parse error. Otherwise store and pass through.
6. **Add 8 parse-only tests** (`tests/gold_standard/ffi_wave0/`): one per attribute, asserting the parse succeeds and the attribute is on the right AST node. These are parse-only (no execution).

### Definition of Done

- [ ] `ExternFnDecl` and `LayoutDef` both have `pub attrs: Vec<Attribute>`.
- [ ] `parse_extern_fn_decl` and layout parsing call `parse_outer_attributes`.
- [ ] All 8 FFI attributes parse on their correct targets without error.
- [ ] Misplaced attributes (e.g. `#[foreign(raw)]` on an extern fn) produce a parse error.
- [ ] All 8 new parse-only tests in `tests/gold_standard/ffi_wave0/` pass.
- [ ] ALL existing 704 gold-standard tests still pass at 100% on x86_64.
- [ ] `cargo build` clean; `cargo clippy` no new warnings.
- [ ] Committed (not pushed).

### Dispatch Box — Wave 1

```
You are Wave 1 of the VUMA 2.0 FFI effort. Task ID: W1.

READ FIRST: /home/z/my-project/worklog.md (understand prior work). When done, APPEND your section (Task ID: W1, Agent, Work Log, Stage Summary).

GOAL: Make FFI attributes parseable on extern fn decls and layout decls. Today ExternFnDecl (src/parser/src/ast.rs:395) and LayoutDef (src/parser/src/ast.rs:430) have NO `attrs` field — so #[pure]/#[borrow]/#[foreign(raw)] are literally unparsable.

YOU MAY ONLY TOUCH:
  - src/parser/src/ast.rs
  - src/parser/src/parser.rs
  - tests/gold_standard/ffi_wave0/  (new dir, parse-only tests)

CONTRACT:
  1. Add `pub attrs: Vec<Attribute>` to ExternFnDecl (ast.rs:395) and LayoutDef (ast.rs:430).
  2. The Attribute struct already exists (ast.rs:54). Do NOT modify it.
  3. In parse_extern_fn_decl (parser.rs:1160): call self.parse_outer_attributes()? at the top, before expect(Fn). Store in the returned ExternFnDecl.
  4. In layout parsing (find it via: rg -n "parse_layout\|LayoutDef" src/parser/src/parser.rs): same pattern.
  5. Recognize these 8 attributes and validate placement:
       #[borrow]            — extern fn param only
       #[marshal]           — extern fn param only
       #[may_retain]        — extern fn param only
       #[unmarshal(Layout)] — extern fn return only
       #[foreign_return(raw)] — extern fn return only
       #[callback]          — extern fn itself only
       #[foreign_consume(raw)] — extern fn itself only
       #[foreign(raw)]      — layout decl only
     Misplaced → ParseError. Correct placement → store in attrs, pass through.
  6. Add 8 parse-only tests in tests/gold_standard/ffi_wave0/:
       borrow_attr.vuma, marshal_attr.vuma, may_retain_attr.vuma,
       unmarshal_attr.vuma, foreign_return_attr.vuma, callback_attr.vuma,
       foreign_consume_attr.vuma, foreign_layout_attr.vuma
     Each: declares the extern/layout with the attribute, asserts parse OK.
     Add 1 negative test: misplaced_attr.vuma (#[foreign(raw)] on extern fn → parse error).

BUILD + TEST:
  cd /home/z/vuma-analysis
  CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_OPT_LEVEL=0 CARGO_INCREMENTAL=1 \
    cargo build --profile dev --bin compile_dump 2>&1 | tail -4
  # Run existing tests to confirm no regression:
  CARGO_BUILD_JOBS=1 cargo test --workspace 2>&1 | tail -20
  # Run your new parse tests:
  for t in tests/gold_standard/ffi_wave0/*.vuma; do
    ./target/dev/compile_dump "$t" /tmp/w1.bin x86_64 --verify && echo "OK: $t" || echo "FAIL: $t"
  done

RULES: No env gates. No commented-out code. No shortcuts. Fix root causes. CARGO_BUILD_JOBS=1 (4 GiB RAM cap). Commit, do NOT push.

DoD: see TASKS.md Wave 1 Definition of Done. All 8 checkboxes must pass.
```

---

## Wave 2 — Three Disjoint Foundations  (3 agents in parallel)

**Goal:** Build the three independent foundations that later waves compose: (a) the scratchpad runtime, (b) the SCG consume node for `#[foreign_consume]`, (c) the borrow-region verifier scaffold. All three touch disjoint files → no merge conflicts.

**Rationale:** These are the load-bearing primitives. They have no inter-dependencies (each is self-contained), so they can be built simultaneously. Wave 1 must be complete first (2b reads attributes from the AST that Wave 1 makes parseable).

### Task 2a — Runtime Scratchpad

**Files:** `src/codegen/src/runtime/mod.rs` (new), `src/codegen/src/runtime/ffi_scratch.rs` (new), `src/codegen/src/lib.rs` (register module).

**Contract:** A thread-local, `malloc`-backed stack scratchpad, separate from `___pmt_buffer`:

```rust
// src/codegen/src/runtime/ffi_scratch.rs
// Thread-local scratchpad for FFI marshalling. Stack-shaped: push on
// transform entry, pop on transform exit. NEVER aliased by ___pmt_buffer.
// Exports the symbol `___ffi_scratch` (current frame base) for codegen.

pub struct ScratchFrame { base: *mut u8, len: usize, capacity: usize }
thread_local! { static SCRATCH_STACK: RefCell<Vec<ScratchFrame>> = ... }
pub fn push_frame();        // alloc a new frame (malloc-backed)
pub fn pop_frame();         // free the top frame's malloc'd block
pub fn alloc(bytes: usize) -> u64;  // bump-alloc within the top frame, return Address
pub fn current_base() -> u64;       // for codegen to load into a vreg
```

**DoD:**
- [ ] `src/codegen/src/runtime/` exists with `mod.rs` + `ffi_scratch.rs`.
- [ ] `src/codegen/src/lib.rs` declares `pub mod runtime; pub mod runtime::ffi_scratch;`.
- [ ] `push_frame`/`pop_frame`/`alloc`/`current_base` implemented and unit-tested.
- [ ] `cargo build` clean; `cargo test -p vuma-codegen ffi_scratch` passes.
- [ ] Committed.

### Task 2b — SCG ForeignConsume Node

**Files:** `src/scg/src/node.rs`, `src/parser/src/to_scg.rs`.

**Contract:** Add a SCG node that marks a state as consumed by a foreign close-call, so the existing `state_write` verifier catches post-close use (no new verifier needed):

```rust
// src/scg/src/node.rs
pub enum NodeType { ..., ForeignConsume }   // new variant
pub enum NodePayload { ..., ForeignConsume(ForeignConsumeNode) }
pub struct ForeignConsumeNode {
    pub input_vreg: u32,       // the State<ForeignLayout> being closed
    pub layout_name: String,   // for diagnostics
}
// In src/parser/src/to_scg.rs: when lowering an Expr::Call where the callee
// is an extern fn with #[foreign_consume(raw)], emit a ForeignConsume node
// for each State-typed arg whose layout has #[foreign(raw)].
```

The `state_write` verifier already tracks consumed vregs via `StateTransformNode`. Wire `ForeignConsume` into the same consumed-set so post-close reads/writes are caught. (This may require a 1-line addition to `state_write`'s consumed-set builder to also include `ForeignConsume` inputs — check `src/ive/src/` for the consumed-set logic.)

**DoD:**
- [ ] `NodeType::ForeignConsume` + `NodePayload::ForeignConsume` added.
- [ ] `to_scg.rs` emits `ForeignConsume` for `#[foreign_consume]` extern calls.
- [ ] `state_write` verifier treats `ForeignConsume` inputs as consumed (post-close use → error).
- [ ] Unit test: a `.vuma` program that reads a foreign handle after close → `state_write` error.
- [ ] All existing tests pass.
- [ ] Committed.

### Task 2c — IVE borrow_region Scaffold

**Files:** `src/ive/src/borrow_region.rs` (new), `src/ive/src/lib.rs` (register).

**Contract:** Scaffold the borrow-region verifier (struct + stub). Wave 4 fills in the logic:

```rust
// src/ive/src/borrow_region.rs
/// Borrow-region verifier (scaffold — implemented in Wave 4).
/// Tracks #[borrow] regions per extern call site. Rule: any StateWrite to a
/// borrowed region DURING the call window is a violation. Auto-released on return.
#[derive(Debug, Clone)]
pub struct BorrowRegion { pub vreg: u32, pub byte_range: (u64, u64), pub call_site: usize }
pub struct BorrowVerification { pub valid: bool, pub error: Option<String> }
/// STUB: returns Vec::new() (always valid). Wave 4 implements the real check.
pub fn verify_borrow_regions(regions: &[BorrowRegion], writes: &[(u32, u64, u64)]) -> Vec<BorrowVerification> {
    let _ = (regions, writes);
    Vec::new()
}
pub fn all_valid(results: &[BorrowVerification]) -> bool { results.iter().all(|r| r.valid) }
```

**DoD:**
- [ ] `src/ive/src/borrow_region.rs` exists with the structs + stub `verify_borrow_regions`.
- [ ] `src/ive/src/lib.rs` (or `mod.rs`) declares `pub mod borrow_region;`.
- [ ] `cargo build` clean; `cargo test -p vuma-ive` passes.
- [ ] Committed.

### Dispatch Box — Wave 2 (dispatch all 3 in ONE message)

> **Orchestrator:** dispatch 2a, 2b, 2c as three separate `Task` tool calls in a **single message** so they run simultaneously. Each is self-contained (disjoint files). Wait for all 3 to complete before starting Wave 3.

```
=== COMMON PREAMBLE (prepend to each of 2a/2b/2c) ===
You are Wave {ID} of the VUMA 2.0 FFI effort.

READ FIRST: /home/z/my-project/worklog.md (Wave 1 must be complete). When done, APPEND your section (Task ID: W{ID}, Agent, Work Log, Stage Summary).

RULES: No env gates. No commented-out code. No shortcuts. Fix root causes. CARGO_BUILD_JOBS=1 (4 GiB RAM). Commit, do NOT push. Only touch the files listed below — do NOT touch any other file.

BUILD:
  cd /home/z/vuma-analysis
  CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_OPT_LEVEL=0 CARGO_INCREMENTAL=1 \
    cargo build --profile dev --bin compile_dump 2>&1 | tail -4

=== 2a: Runtime Scratchpad ===
TASK ID: W2a. FILES: src/codegen/src/runtime/mod.rs (new), src/codegen/src/runtime/ffi_scratch.rs (new), src/codegen/src/lib.rs (add `pub mod runtime;`).
CONTRACT: [paste Task 2a contract above]. Implement thread-local malloc-backed stack: push_frame/pop_frame/alloc/current_base. Unit-test all 4 fns.
DoD: cargo test -p vuma-codegen ffi_scratch passes; all existing tests pass.

=== 2b: SCG ForeignConsume Node ===
TASK ID: W2b. FILES: src/scg/src/node.rs, src/parser/src/to_scg.rs. (You MAY also add ≤3 lines to the state_write consumed-set builder in src/ive/src/ if needed to treat ForeignConsume inputs as consumed — find it via: rg -n "consumed\|StateTransform" src/ive/src/.)
CONTRACT: [paste Task 2b contract above]. Add ForeignConsume node; emit it for #[foreign_consume] calls; wire into state_write consumed-set.
DoD: a .vuma program reading a foreign handle after close → state_write error; all existing tests pass.

=== 2c: IVE borrow_region Scaffold ===
TASK ID: W2c. FILES: src/ive/src/borrow_region.rs (new), src/ive/src/lib.rs.
CONTRACT: [paste Task 2c contract above]. Scaffold only — structs + stub verify_borrow_regions returning Vec::new(). Wave 4 implements logic.
DoD: cargo test -p vuma-ive passes; all existing tests pass.
```

---

## Wave 3 — Marshal Codegen  (2 agents in parallel)

**Goal:** Implement the scratchpad marshalling path: `marshal_cstr`/`unmarshal` builtins (3a) and transform-entry/exit scratchpad push/pop hooks (3b). Both depend on Wave 2a (runtime exists). Disjoint files → parallel.

### Task 3a — Marshal Builtins

**Files:** `src/codegen/src/marshal.rs` (rewrite the 64-line stub).

**Contract:** Replace the current `marshal_state_for_ffi` stub with real marshalling:

```rust
// src/codegen/src/marshal.rs
// Recognize and lower:
//   marshal_cstr(s: State<String>) -> Address
//     → copy s.bytes[0..s.len] into ___ffi_scratch, append '\0', return Address
//   unmarshal<T>(src: Address, len: u64) -> State<T>
//     → alloc fresh offset in ___pmt_buffer, memcpy from src, return State<T>
// Route #[marshal] and #[may_retain] args through the scratchpad instead of
// handing C a pointer into ___pmt_buffer.
// #[borrow] args → pass ___pmt_buffer_base + offset directly (zero-copy, as today).
// #[foreign(raw)] args → extract the `raw` field value (Wave 5 handles this; 3a just must not break it).
```

**DoD:**
- [ ] `marshal_cstr` builtin recognized and lowered (scratchpad alloc + copy + NUL).
- [ ] `unmarshal` builtin recognized and lowered (fresh `___pmt_buffer` offset + memcpy).
- [ ] `#[marshal]`/`#[may_retain]` args route through scratchpad.
- [ ] `#[borrow]` args remain zero-copy (no regression).
- [ ] Unit tests for each builtin/mode.
- [ ] All existing tests pass.
- [ ] Committed.

### Task 3b — Transform Scratchpad Hooks

**Files:** `src/codegen/src/scg_to_ir.rs`.

**Contract:** Emit scratchpad push/pop at transform boundaries:

```rust
// In scg_to_ir.rs, in the transform-lowering path (find via:
//   rg -n "StateTransform\|transform" src/codegen/src/scg_to_ir.rs):
//   - On transform ENTRY: emit IRInstr::Call to `ffi_scratch_push_frame` (runtime symbol from W2a)
//   - On transform EXIT (all return paths): emit IRInstr::Call to `ffi_scratch_pop_frame`
//   - This ensures the scratchpad is wiped when the transform ends.
//   - Nested transforms get nested frames (push/pop is a stack).
```

**DoD:**
- [ ] Every transform entry emits a `push_frame` call.
- [ ] Every transform exit (including early returns) emits a `pop_frame` call.
- [ ] Nested transforms produce nested frames (verified via IR dump).
- [ ] All existing tests pass.
- [ ] Committed.

### Dispatch Box — Wave 3 (dispatch 3a + 3b in ONE message)

```
=== COMMON PREAMBLE (prepend to each) ===
You are Wave {ID} of the VUMA 2.0 FFI effort.
READ FIRST: /home/z/my-project/worklog.md (Waves 1 + 2a must be complete). APPEND your section when done.
RULES: No env gates. No commented-out code. No shortcuts. CARGO_BUILD_JOBS=1. Commit, do NOT push. Only touch listed files.
BUILD: cd /home/z/vuma-analysis && CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_OPT_LEVEL=0 CARGO_INCREMENTAL=1 cargo build --profile dev --bin compile_dump 2>&1 | tail -4

=== 3a: Marshal Builtins ===
TASK ID: W3a. FILES: src/codegen/src/marshal.rs ONLY.
CONTRACT: [paste Task 3a contract]. Rewrite the 64-line stub. Recognize marshal_cstr/unmarshal builtins; route #[marshal]/#[may_retain] through scratchpad; keep #[borrow] zero-copy.
DoD: [paste Task 3a DoD].

=== 3b: Transform Scratchpad Hooks ===
TASK ID: W3b. FILES: src/codegen/src/scg_to_ir.rs ONLY.
CONTRACT: [paste Task 3b contract]. Emit push_frame on transform entry, pop_frame on all exit paths. Wire the `ffi_scratch_push_frame`/`ffi_scratch_pop_frame` runtime symbols from W2a.
DoD: [paste Task 3b DoD].
```

---

## Wave 4 — Borrow Region Verifier  (1 agent, sequential)

**Goal:** Implement the `borrow_region` pass (scaffolded in 2c) and wire it into the existing `ffi` verifier. Tracks `#[borrow]` regions per call site; flags `StateWrite` during the borrow window; auto-releases on call return.

**Files:** `src/ive/src/borrow_region.rs` (implement), `src/ive/src/ffi.rs` (extend).

### Tasks

1. **Implement `verify_borrow_regions`** in `borrow_region.rs`: replace the stub. Input: list of `(vreg, byte_range, call_site)` borrow-regions + list of `(vreg, offset, size)` writes. Output: violation if a write hits a borrowed region whose call_site is "in flight" (i.e. the call hasn't returned yet).
2. **Track call-site liveness:** the borrow_region pass receives the ordered list of SCG nodes; a borrow-region is "in flight" from its call node until the next node after the call's return.
3. **Wire into `ffi.rs`:** on a `#[borrow]` call return, mark the region as *preserved* (not invalidated) in the existing `invalidated_vars` set. On a non-`#[borrow]` call, mark as *invalidated* (existing behavior, unchanged).
4. **Add 3 tests** in `tests/gold_standard/ffi_wave2/`:
   - `borrow_preserved.vuma` — `#[borrow]` call, state readable after → compiles OK.
   - `write_during_borrow_reject.vuma` — negative: `StateWrite` to a `#[borrow]` region mid-call → `borrow_region` error.
   - `invalidate_still_works.vuma` — default (no attr) call still invalidates → read-after = error.

### DoD

- [ ] `verify_borrow_regions` implemented (not a stub).
- [ ] `#[borrow]` calls preserve state (verified by `borrow_region` pass, not just trusted).
- [ ] `StateWrite` to a borrowed region during the call window → compile error.
- [ ] Default (no attr) calls still invalidate (existing `ffi` behavior unchanged).
- [ ] 3 new tests in `ffi_wave2/` pass.
- [ ] All existing tests pass.
- [ ] Committed.

### Dispatch Box — Wave 4

```
You are Wave 4 of the VUMA 2.0 FFI effort. Task ID: W4.
READ FIRST: /home/z/my-project/worklog.md (Waves 1, 2c, 3 must be complete). APPEND your section when done.
FILES (only these): src/ive/src/borrow_region.rs, src/ive/src/ffi.rs, tests/gold_standard/ffi_wave2/ (new).
GOAL: Implement the borrow_region pass (scaffolded in W2c) and wire into ffi verifier.
CONTRACT: [paste Wave 4 Tasks 1-4 above].
BUILD: cd /home/z/vuma-analysis && CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_OPT_LEVEL=0 CARGO_INCREMENTAL=1 cargo build --profile dev --bin compile_dump 2>&1 | tail -4
TEST: CARGO_BUILD_JOBS=1 cargo test --workspace 2>&1 | tail -20
RULES: No env gates. No commented-out code. No shortcuts. CARGO_BUILD_JOBS=1. Commit, do NOT push.
DoD: [paste Wave 4 DoD].
```

---

## Wave 5 — ForeignState Lowering  (1 agent, sequential)

**Goal:** Lower `#[foreign(raw)]` arguments (pass the `raw` field value, not the buffer pointer) and `#[foreign_return(raw)]` returns (wrap the C pointer into a `State<ForeignLayout>`).

**Files:** `src/codegen/src/scg_to_ir.rs` (foreign arg extraction + return wrapping).

**Depends on:** Wave 1 (attrs parseable), Wave 2b (ForeignConsume SCG node), Wave 3b (scg_to_ir transform hooks done — no file conflict since W3b is committed).

### Tasks

1. **Foreign arg extraction:** when an extern call arg has a `State<T>` whose layout `T` has `#[foreign(raw)]`, extract the `raw` field's value (a `u64` = the C pointer) and pass *that* as the call argument — NOT `___pmt_buffer_base + offset`.
2. **Foreign return wrapping:** when an extern fn has `#[foreign_return(raw)]`, allocate a fresh `State<ForeignLayout>` offset in `___pmt_buffer` and store the returned `u64` into its `raw` field. (The C pointer is just a `u64` payload living in `___pmt_buffer` — single-buffer invariant preserved.)
3. **`as Address` cast:** allow `state_var as Address` to explicitly pass a buffer pointer (for non-foreign states handed to C as raw buffers, e.g. out-params like `&stmt`). Lower to `___pmt_buffer_base + offset`.
4. **Add 2 tests** in `tests/gold_standard/ffi_wave3/`:
   - `foreign_arg_pass.vuma` — pass a `State<DbHandle>` to an extern fn; verify the `raw` field is passed (not the buffer pointer) via IR inspection.
   - `foreign_return_wrap.vuma` — extern fn with `#[foreign_return(raw)]`; verify the return is wrapped into a `State<ForeignLayout>`.

### DoD

- [ ] `#[foreign(raw)]` args: the `raw` field value is passed to C (not the buffer pointer).
- [ ] `#[foreign_return(raw)]` returns: wrapped into a fresh `State<ForeignLayout>` in `___pmt_buffer`.
- [ ] `state_var as Address` lowers to `___pmt_buffer_base + offset`.
- [ ] 2 new tests in `ffi_wave3/` pass.
- [ ] All existing tests pass.
- [ ] Committed.

### Dispatch Box — Wave 5

```
You are Wave 5 of the VUMA 2.0 FFI effort. Task ID: W5.
READ FIRST: /home/z/my-project/worklog.md (Waves 1, 2b, 3b must be complete). APPEND your section when done.
FILES (only these): src/codegen/src/scg_to_ir.rs, tests/gold_standard/ffi_wave3/ (new).
GOAL: Lower #[foreign(raw)] args (pass raw field, not buffer ptr) + #[foreign_return(raw)] returns (wrap into State<ForeignLayout>).
CONTRACT: [paste Wave 5 Tasks 1-3 above].
BUILD: cd /home/z/vuma-analysis && CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_OPT_LEVEL=0 CARGO_INCREMENTAL=1 cargo build --profile dev --bin compile_dump 2>&1 | tail -4
TEST: CARGO_BUILD_JOBS=1 cargo test --workspace 2>&1 | tail -20
RULES: No env gates. No commented-out code. No shortcuts. CARGO_BUILD_JOBS=1. Commit, do NOT push.
DoD: [paste Wave 5 DoD].
```

---

## Wave 6 — Real libc End-to-End Tests  (4 agents in parallel — MAX PARALLELISM)

**Goal:** The defining wave. Replace today's declarative-only FFI tests with **real end-to-end foreign calls** linking libc on x86_64 + aarch64. Four agents, four disjoint test files → demonstrates the max-4-parallel workflow.

**Depends on:** Waves 1–5 all complete.

### Task 6a — Real `write(1, buf, 16)`

**Files:** `tests/gold_standard/ffi_wave1/write_real.vuma`, `Makefile` (add `ffi-test` target).

**Contract:** A `.vuma` program that actually calls `write(1, buf, 16)` via the extern, links against libc (`ld -o out.o -lc`), runs, and exits 0. This is the test today's `ffi_write.vuma` *claims* to be but comments out the call.

```vuma
// tests/gold_standard/ffi_wave1/write_real.vuma
// Expected exit code: 0
// Actually calls write(1, buf, 16) — links libc, runs for real.
layout IoBuf = { len: u64, data: [u8; 16] }
extern "C" {
    #[borrow]
    fn write(fd: i64, buf: Address, count: i64) -> i64;
}
transform emit(b: State<IoBuf>) -> State<IoBuf> {
    let n = write(1, b as Address, 16);
    b.len = n as u64;
    return b;
}
fn main() -> i32 {
    let b = state_new(IoBuf);
    b.data[0] = 'H'; /* ...fill "Hello, VUMA!\n\0..." */
    b.len = 16;
    emit(b);
    return 0;
}
```

**DoD:**
- [ ] `write_real.vuma` compiles to a relocatable object on x86_64 + aarch64.
- [ ] Links with `ld -o write_real write_real.o -lc` successfully.
- [ ] Runs and exits 0 on x86_64 (and aarch64 under QEMU).
- [ ] Actually writes 16 bytes to stdout (verified by capturing output).
- [ ] `Makefile` has a `ffi-test` target that builds + runs all Wave 6 tests.
- [ ] Committed.

### Task 6b — Real `open(marshal_cstr(path))`

**Files:** `tests/gold_standard/ffi_wave1/open_file.vuma`.

**Contract:** Opens `/dev/null` via `open(marshal_cstr(path), O_RDONLY)`, checks `fd >= 0`, closes it. Proves NUL-terminated string marshalling works (today's FFI cannot do this at all).

**DoD:**
- [ ] Compiles + links + runs on x86_64 + aarch64.
- [ ] `marshal_cstr` produces a NUL-terminated string in the scratchpad.
- [ ] `open` receives a valid `const char*` and returns `fd >= 0`.
- [ ] Exits 0.
- [ ] Committed.

### Task 6c — `strdup` + unmarshal + `free` round-trip

**Files:** `tests/gold_standard/ffi_wave1/strdup.vuma`.

**Contract:** Calls `strdup("hello")`, unmarshals the result into a `State<String>`, verifies the bytes, calls `free`. Proves C-ownership memory round-trips safely through the scratchpad boundary.

**DoD:**
- [ ] `strdup` returns a C-owned pointer.
- [ ] `unmarshal` copies it into `___pmt_buffer` (not a borrow — a copy).
- [ ] `free` is called on the original C pointer (not the `___pmt_buffer` copy).
- [ ] Exits 0 on x86_64 + aarch64.
- [ ] Committed.

### Task 6d — Borrow-mode tests

**Files:** `tests/gold_standard/ffi_wave2/borrow_preserved.vuma`, `invalidate_reject.vuma`, `may_retain_forces_marshal.vuma`.

**Contract:** Three tests exercising the borrow-mode taxonomy from Wave 4:
- `borrow_preserved.vuma` — `#[borrow]` call, state readable after → compiles + runs OK.
- `invalidate_reject.vuma` — negative: read-after-invalidate → compile error (assert the error message mentions "invalidation").
- `may_retain_forces_marshal.vuma` — `#[may_retain]` on a borrowable-looking arg routes through scratchpad (verify via IR inspection that the arg is NOT `___pmt_buffer_base + offset`).

**DoD:**
- [ ] `borrow_preserved.vuma` compiles + runs OK.
- [ ] `invalidate_reject.vuma` is a compile error with the right message.
- [ ] `may_retain_forces_marshal.vuma` routes through scratchpad (IR-verified).
- [ ] Committed.

### Dispatch Box — Wave 6 (dispatch all 4 in ONE message)

```
=== COMMON PREAMBLE (prepend to each) ===
You are Wave {ID} of the VUMA 2.0 FFI effort.
READ FIRST: /home/z/my-project/worklog.md (Waves 1-5 must be complete). APPEND your section when done.
RULES: No env gates. No commented-out code. No shortcuts. CARGO_BUILD_JOBS=1. Commit, do NOT push.
BUILD: cd /home/z/vuma-analysis && CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_OPT_LEVEL=0 CARGO_INCREMENTAL=1 cargo build --profile dev --bin compile_dump 2>&1 | tail -4
COMPILE A TEST: ./target/dev/compile_dump <test.vuma> /tmp/out.o x86_64 --format obj
LINK: ld -o /tmp/out /tmp/out.o -lc
RUN: /tmp/out; echo "exit=$?"
CROSS (aarch64): ./target/dev/compile_dump <test.vuma> /tmp/out.o aarch64 --format obj && aarch64-linux-gnu-gcc -o /tmp/out /tmp/out.o && qemu-aarch64 /tmp/out; echo "exit=$?"

=== 6a: Real write() ===
TASK ID: W6a. FILES: tests/gold_standard/ffi_wave1/write_real.vuma, Makefile.
CONTRACT: [paste Task 6a contract + example]. Must actually call write(1, buf, 16) and link libc.
DoD: [paste Task 6a DoD].

=== 6b: Real open() ===
TASK ID: W6b. FILES: tests/gold_standard/ffi_wave1/open_file.vuma.
CONTRACT: [paste Task 6b contract]. open(marshal_cstr("/dev/null"), 0). Proves NUL-term marshalling.
DoD: [paste Task 6b DoD].

=== 6c: strdup round-trip ===
TASK ID: W6c. FILES: tests/gold_standard/ffi_wave1/strdup.vuma.
CONTRACT: [paste Task 6c contract]. strdup + unmarshal + free. Proves C-ownership round-trip.
DoD: [paste Task 6c DoD].

=== 6d: Borrow-mode tests ===
TASK ID: W6d. FILES: tests/gold_standard/ffi_wave2/{borrow_preserved,invalidate_reject,may_retain_forces_marshal}.vuma.
CONTRACT: [paste Task 6d contract]. 3 tests: positive borrow, negative invalidate, IR-verified may_retain.
DoD: [paste Task 6d DoD].
```

---

## Wave 7 — vuma_context_t C-API for Callbacks  (2 agents in parallel)

**Goal:** Build the C-API that lets C call *back* into VUMA during a foreign function (e.g. `sqlite3_exec`'s row callback). Generalizes the existing `scripts/wasm32_runner.py:make_host_functions` precedent into a C header shipped for all backends. Two agents, two new disjoint files.

**Re-entrancy rule (decided):** callbacks run on an isolated callback stack with their own scratchpad frame, and are forbidden from touching any `State` in the caller's live set (enforced by a runtime `callback_live_set` guard — trap on violation).

**Depends on:** Wave 5 (foreign handles for callback examples).

### Task 7a — vuma_vm.h + Accessor Implementation

**Files:** `vuma_vm.h` (new, repo root), `src/codegen/src/runtime/vuma_context.rs` (new).

**Contract:**

```c
// vuma_vm.h — the VUMA C-API (generalizes scripts/wasm32_runner.py:make_host_functions)
typedef struct vuma_context vuma_context_t;
uint32_t vuma_read_u32 (vuma_context_t* ctx, uint64_t offset);
uint64_t vuma_read_u64 (vuma_context_t* ctx, uint64_t offset);
void     vuma_write_u32(vuma_context_t* ctx, uint64_t offset, uint32_t val);
void     vuma_write_u64(vuma_context_t* ctx, uint64_t offset, uint64_t val);
uint64_t vuma_state_new(vuma_context_t* ctx, const char* layout_name);
void     vuma_push_i32  (vuma_context_t* ctx, int32_t val);
void     vuma_push_i64  (vuma_context_t* ctx, int64_t val);
```

Implement for x86_64 + aarch64 first (the two test backends). Big-endian backends (`mips64be`, `ppc64`, `s390x`, `sparc64`, `armeb`) get byte-order-correct `vuma_read_u32` — mechanical, deferred to a follow-up if time-boxed.

**DoD:**
- [ ] `vuma_vm.h` exists at repo root with the 8 accessor declarations.
- [ ] `src/codegen/src/runtime/vuma_context.rs` implements all 8 for x86_64 + aarch64.
- [ ] `cargo build` clean; accessor unit tests pass.
- [ ] Committed.

### Task 7b — Callback Runtime (re-entrancy guard)

**Files:** `src/codegen/src/runtime/callback.rs` (new).

**Contract:**

```rust
// src/codegen/src/runtime/callback.rs
// Callback stack + live-set guard. Enforces the re-entrancy rule:
// callbacks run on an isolated stack, may only state_new their own states,
// and trap if they touch a caller-live State.
pub struct CallbackContext { scratch_frame: ScratchFrame, live_set: LiveSet }
pub struct LiveSet { /* bitmask of caller-live ___pmt_buffer offsets */ }
pub fn enter_callback(caller_live: LiveSet) -> CallbackContext;  // push isolated frame
pub fn exit_callback(ctx: CallbackContext);                       // pop + free
pub fn check_access(ctx: &CallbackContext, offset: u64) -> bool;  // false → trap
```

**DoD:**
- [ ] `callback.rs` exists with `CallbackContext`/`LiveSet`/`enter_callback`/`exit_callback`/`check_access`.
- [ ] `check_access` returns `false` (trap) for any offset in the caller's `LiveSet`.
- [ ] Unit test: a callback touching a caller-live offset → trap.
- [ ] `cargo build` clean.
- [ ] Committed.

### Dispatch Box — Wave 7 (dispatch 7a + 7b in ONE message)

```
=== COMMON PREAMBLE ===
You are Wave {ID} of the VUMA 2.0 FFI effort.
READ FIRST: /home/z/my-project/worklog.md (Waves 1-5, esp. W5, must be complete). APPEND your section when done.
RULES: No env gates. No commented-out code. No shortcuts. CARGO_BUILD_JOBS=1. Commit, do NOT push.
BUILD: cd /home/z/vuma-analysis && CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_OPT_LEVEL=0 CARGO_INCREMENTAL=1 cargo build --profile dev --bin compile_dump 2>&1 | tail -4
REFERENCE: scripts/wasm32_runner.py:make_host_functions is the precedent — this wave generalizes it to C.

=== 7a: vuma_vm.h + accessors ===
TASK ID: W7a. FILES: vuma_vm.h (new, repo root), src/codegen/src/runtime/vuma_context.rs (new), src/codegen/src/runtime/mod.rs (register).
CONTRACT: [paste Task 7a contract]. 8 accessors, x86_64 + aarch64 impl. Big-endian deferred.
DoD: [paste Task 7a DoD].

=== 7b: Callback runtime + re-entrancy guard ===
TASK ID: W7b. FILES: src/codegen/src/runtime/callback.rs (new), src/codegen/src/runtime/mod.rs (register).
CONTRACT: [paste Task 7b contract]. CallbackContext + LiveSet + enter/exit/check_access. Trap on caller-live access.
DoD: [paste Task 7b DoD].
```

---

## Wave 8 — Callback Tests + Docs  (2 agents in parallel)

**Goal:** Close the loop with a real `sqlite3_exec` callback test + a negative trap test, and rewrite the docs for the mode matrix. Two agents, disjoint (tests vs docs).

**Depends on:** Wave 7.

### Task 8a — Callback Tests

**Files:** `tests/gold_standard/ffi_wave4/sqlite_exec_callback.vuma`, `callback_touches_caller_state_traps.vuma`.

**Contract:**
- `sqlite_exec_callback.vuma` — real `sqlite3_open` + `sqlite3_exec` with a VUMA `#[callback]` row handler that counts rows. Exits with the row count. Links `-lsqlite3`.
- `callback_touches_caller_state_traps.vuma` — a callback that writes to a caller-live state → runtime trap (non-zero exit with diagnostic).

**DoD:**
- [ ] `sqlite_exec_callback.vuma` compiles, links `-lsqlite3`, runs, exits with the correct row count on x86_64 + aarch64.
- [ ] `callback_touches_caller_state_traps.vuma` traps at runtime with the re-entrancy diagnostic.
- [ ] Committed.

### Task 8b — Docs Rewrite

**Files:** `docs/language-reference.md` (§14 FFI), `docs/architecture.md` (§9 FFI Marshal Pass).

**Contract:** Replace the binary `#[pure]`/invalidate model with the 4-mode matrix. Cover:
- The 4 argument modes (`#[borrow]`, default-invalidate, `#[marshal]`, `#[foreign(raw)]`)
- The 3 return modes (scalar, `#[unmarshal(Layout)]`, `#[foreign_return(raw)]`)
- The callback mode (`#[callback]` + `vuma_context_t`)
- The scratchpad model (stack-per-frame, unmarshal=always-copy)
- The re-entrancy rule (isolated callback stack, `callback_live_set` guard)
- Worked examples: `write`, `open`, `sqlite3_prepare_v2` (the all-modes-composing example from the proposal)

**DoD:**
- [ ] `docs/language-reference.md` §14 rewritten with the matrix + 3 worked examples.
- [ ] `docs/architecture.md` §9 rewritten: scratchpad runtime, borrow_region pass, ForeignConsume SCG node, vuma_context C-API.
- [ ] Cross-referenced with the proposal document.
- [ ] Committed.

### Dispatch Box — Wave 8 (dispatch 8a + 8b in ONE message)

```
=== COMMON PREAMBLE ===
You are Wave {ID} of the VUMA 2.0 FFI effort.
READ FIRST: /home/z/my-project/worklog.md (Waves 1-7 must be complete). APPEND your section when done.
RULES: No env gates. No commented-out code. No shortcuts. CARGO_BUILD_JOBS=1. Commit, do NOT push.
BUILD: cd /home/z/vuma-analysis && CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_OPT_LEVEL=0 CARGO_INCREMENTAL=1 cargo build --profile dev --bin compile_dump 2>&1 | tail -4

=== 8a: Callback tests ===
TASK ID: W8a. FILES: tests/gold_standard/ffi_wave4/sqlite_exec_callback.vuma, callback_touches_caller_state_traps.vuma.
CONTRACT: [paste Task 8a contract]. Real sqlite3_exec callback + negative trap test.
LINK: gcc -o out out.o -lsqlite3
DoD: [paste Task 8a DoD].

=== 8b: Docs rewrite ===
TASK ID: W8b. FILES: docs/language-reference.md, docs/architecture.md.
CONTRACT: [paste Task 8b contract]. Replace #[pure]/invalidate binary model with the 4-mode matrix. 3 worked examples.
DoD: [paste Task 8b DoD].
```

---

## 4. Subagent Dispatch Workflow (for the orchestrator)

This is the protocol an orchestrator follows to execute the waves with up to 4 parallel subagents without overwhelming their context.

### 4.1 Rules

1. **One task = one subagent.** Never give a subagent an entire wave. Give it ONE task (e.g. "W2a: Runtime Scratchpad"). The Dispatch Boxes above are sized to fit in a subagent's context.
2. **Surgical scope.** Each Dispatch Box names the EXACT files the agent may touch. Agents must not touch other files — this is the parallelism contract (§3 file-ownership map).
3. **Parallel dispatch = single message, multiple Task calls.** For waves with parallel tasks (W2: 3 agents, W3: 2, W6: 4, W7: 2, W8: 2), dispatch ALL parallel subagents in ONE message (multiple `Task` tool calls) so they run simultaneously.
4. **Max 4 at once.** Never exceed 4 parallel subagents (context + RAM). Wave 6 is the only 4-way wave; others are ≤3.
5. **Sequential waves wait.** W1 must complete+commit before W2 starts. W3 waits for W2a. W4 waits for W2c+W1. W5 waits for W2b+W3b. W6 waits for W1-5. W7 waits for W5. W8 waits for W7. (See §6 graph.)
6. **Worklog protocol.** Every subagent reads `/home/z/my-project/worklog.md` first, and appends its section (Task ID, Agent, Work Log, Stage Summary) when done. The orchestrator reads the worklog before dispatching the next wave to confirm prerequisites are met.
7. **Commit, don't push.** Subagents commit their own work. The orchestrator pushes after verifying a wave's DoD.
8. **Memory constraint.** 4 GiB RAM cap. Always `CARGO_BUILD_JOBS=1`. The build command is pinned in every Dispatch Box.
9. **No workarounds.** Every prompt states: "No env gates, no commented-out code, no shortcuts. Fix root causes."
10. **Time-box.** If a subagent times out, the orchestrator picks up the partial work from worklog + git status and re-dispatches a tighter task.

### 4.2 Dispatch Sequence

```
Round 1:  W1                    (1 agent — sequential foundation)
          ↓ (commit + verify DoD)
Round 2:  W2a ∥ W2b ∥ W2c       (3 agents — disjoint files)
          ↓ (all 3 commit + verify)
Round 3:  W3a ∥ W3b             (2 agents — disjoint files, both depend on W2a)
          ↓ (commit + verify)
Round 4:  W4                    (1 agent — IVE domain, depends on W2c+W1)
          ↓ (commit + verify)
Round 5:  W5                    (1 agent — scg_to_ir, depends on W2b+W3b)
          ↓ (commit + verify)
Round 6:  W6a ∥ W6b ∥ W6c ∥ W6d (4 agents — MAX parallel, disjoint test files)
          ↓ (commit + verify)
Round 7:  W7a ∥ W7b             (2 agents — disjoint new files, depend on W5)
          ↓ (commit + verify)
Round 8:  W8a ∥ W8b             (2 agents — tests vs docs)
          ↓ (commit + verify)
          PUSH to origin/main
```

### 4.3 Verification Between Waves

After each round, the orchestrator:
1. Reads `/home/z/my-project/worklog.md` — confirms each agent's Stage Summary claims DoD met.
2. Runs `cd /home/z/vuma-analysis && git log --oneline -<n>` — confirms the expected commits landed.
3. Runs a smoke build: `CARGO_BUILD_JOBS=1 cargo build --profile dev --bin compile_dump 2>&1 | tail -4` — confirms the tree compiles.
4. Runs `CARGO_BUILD_JOBS=1 cargo test --workspace 2>&1 | tail -20` — confirms no regressions.
5. Only then dispatches the next round.

If any check fails, the orchestrator dispatches a **fix-up agent** for the specific failing task before proceeding.

---

## 5. Definition of Done (whole effort)

The FFI effort is complete when ALL of the following are checked:

### Phase 1 (Scratchpad) — Waves 1, 2a, 3
- [ ] `marshal_cstr` / `unmarshal` builtins work (W3a).
- [ ] Scratchpad push/pop on transform entry/exit (W3b).
- [ ] `ffi_write_real.vuma` actually calls `write(1, buf, 16)` and exits 0 on x86_64 + aarch64 (W6a).
- [ ] `open_file.vuma` calls `open(marshal_cstr(path))` and exits 0 (W6b).
- [ ] `strdup.vuma` round-trips C-owned memory and exits 0 (W6c).

### Phase 2 (Borrow modes) — Waves 2c, 4
- [ ] `borrow_region` verifier implemented (W4).
- [ ] `#[borrow]` preserves state (verified, not trusted).
- [ ] `#[may_retain]` forces scratchpad routing.
- [ ] `borrow_preserved.vuma` / `invalidate_reject.vuma` / `may_retain_forces_marshal.vuma` pass (W6d).

### Phase 3 (ForeignState) — Waves 2b, 5
- [ ] `#[foreign(raw)]` args pass the raw field (not the buffer pointer) (W5).
- [ ] `#[foreign_return(raw)]` wraps returns into `State<ForeignLayout>` (W5).
- [ ] `#[foreign_consume]` close-calls mark the state consumed (reuses `state_write`) (W2b).
- [ ] Post-close use → `state_write` linearity error.

### Phase 4 (vuma_context_t) — Waves 7, 8a
- [ ] `vuma_vm.h` ships with 8 accessors, x86_64 + aarch64 impl (W7a).
- [ ] Callback runtime with `callback_live_set` re-entrancy guard (W7b).
- [ ] `sqlite_exec_callback.vuma` runs a real `sqlite3_exec` callback and exits with the row count (W8a).
- [ ] `callback_touches_caller_state_traps.vuma` traps with the diagnostic (W8a).

### Cross-cutting
- [ ] All 4 sacred invariants preserved (§0): single-buffer purity, no pointer syntax, 3 PMT verifiers canonical, zero per-state malloc/free.
- [ ] ALL existing 704 gold-standard tests pass at 100% on x86_64 (no regressions).
- [ ] `docs/language-reference.md` §14 + `docs/architecture.md` §9 rewritten for the mode matrix (W8b).
- [ ] `--verify` (IVE) on for every new test.
- [ ] `cargo build` clean; `cargo clippy` no new warnings.
- [ ] Pushed to `origin/main`.

---

## 6. Wave Dependency Graph

```
W1 (parser attrs) ──┬──► W2a (scratchpad runtime) ──► W3a (marshal builtins) ──┐
                    │                                       ┌──► W3b (transform hooks) ──┤
                    ├──► W2b (SCG consume node) ────────────┘                              ├──► W6 (4 real libc tests) ──► W8 (callback tests + docs)
                    │                                                                    │
                    └──► W2c (borrow_region scaffold) ──► W4 (borrow verifier impl) ──────┤
                                                                                         │
                    W5 (foreign lowering, needs W2b + W3b) ─────────────────────────────► W7 (vuma_context + callback) ──► W8
```

**Parallel opportunities (max 4 at once):**
- **W2:** 2a ∥ 2b ∥ 2c (3 agents)
- **W3:** 3a ∥ 3b (2 agents)
- **W6:** 6a ∥ 6b ∥ 6c ∥ 6d (4 agents — MAX)
- **W7:** 7a ∥ 7b (2 agents)
- **W8:** 8a ∥ 8b (2 agents)

---

## 7. Effort Estimate

| Wave | Est. effort | Parallelism | Depends on |
|------|-------------|-------------|------------|
| 1 | 1-2 days | sequential (1) | — |
| 2 | 3-5 days | 2a ∥ 2b ∥ 2c (3) | W1 |
| 3 | 2-3 days | 3a ∥ 3b (2) | W2a |
| 4 | 2-3 days | sequential (1) | W2c, W1 |
| 5 | 2-3 days | sequential (1) | W2b, W3b |
| 6 | 3-4 days | 6a ∥ 6b ∥ 6c ∥ 6d (4) | W1-5 |
| 7 | 4-5 days | 7a ∥ 7b (2) | W5 |
| 8 | 2-3 days | 8a ∥ 8b (2) | W7 |
| **Total** | **~3-4 weeks** (with parallelism) | | |

Sequential total would be ~6-7 weeks; parallelism per the graph cuts it to ~3-4.

---

## 8. Success Criteria (VUMA 2.0 FFI release)

1. **Real foreign calls work** — `write`, `open`, `strdup`, `sqlite3_open`, `sqlite3_close`, `sqlite3_exec` all run end-to-end on x86_64 + aarch64, linking libc (-lsqlite3 for the sqlite tests).
2. **Single-buffer invariant preserved** — `___pmt_buffer` is never aliased by FFI malloc; `StateRead`/`StateWrite`/`StateTransform` never see scratchpad/callback memory.
3. **Zero-copy read-only FFI** — `#[borrow]` args pass `___pmt_buffer_base + offset` directly to C (no copy), verified by IR inspection.
4. **Linear handle safety** — `#[foreign_consume]` close-calls mark the state consumed; post-close use is a compile error (via existing `state_write`, no new verifier).
5. **Callback re-entrancy safe** — callbacks run on an isolated stack with a `callback_live_set` guard; touching caller-live state traps at runtime.
6. **No regressions** — all 704 existing gold-standard tests pass at 100% on x86_64; IVE stays at 100%.
7. **Docs accurate** — `language-reference.md` §14 and `architecture.md` §9 describe the mode matrix, not the binary `#[pure]`/invalidate model.

---

*This spec is a living document. Update the DoD checkboxes as waves complete. Record deviations in `/home/z/my-project/worklog.md`. The proposal document ("VUMA 2.0 FFI — A Buildable Design (v2)") is the design reference; this TASKS.md is the execution plan.*
