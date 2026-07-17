# VUMA Arena State Model — Wave-Based Engineering Spec

> **One-sentence pitch:** Add a Tiered State Model to VUMA: static states (`state_new`) stay in the compile-time-sized `___pmt_buffer` for deterministic/embedded code; arena states (`arena_alloc`) use runtime-growaable mmap'd regions for dynamic applications (browsers, GUIs, servers) — both use `State<T>`, neither uses pointers.

This document implements the **Arena State Model** from the design proposal. It is organized into **7 waves** designed for **parallel subagent execution** (up to 4 agents at once). Each wave has **surgical tasks** (exact files, exact contracts), a **Definition of Done** with checkable checkboxes, and a **Dispatch Box** with a ready-to-paste subagent prompt.

**No shortcuts. No stubs. No simplified solutions.** Every backend gets real `mmap`/`mremap`/`munmap` syscall stubs. The arena allocator is real bump-allocation with real bounds checking.

---

## 0. Sacred Invariants (non-negotiable)

1. **No pointer syntax in source.** `*T`, `&x`, `allocate`, `free` remain hard parse errors. Arena states use `State<T>`, not `*T`.
2. **`___pmt_buffer` purity for static states.** The compile-time-sized buffer is unchanged. `state_new` states still live there. Arena memory is separate (mmap'd).
3. **3 PMT verifiers canonical.** StateRead/StateWrite/StateTransform work on both static and arena states. Arena bounds are checked at runtime (like FFI scratchpad).
4. **Zero per-state malloc/free.** Arena uses bump-allocation (no per-object malloc) and arena-free (one `munmap` for the whole arena).
5. **All 19 backends.** Every backend gets real arena syscall stubs. No backend is left behind.

---

## 1. Architecture Overview

### The Arena State Model

```
___pmt_buffer (compile-time sized, all STATIC states live here)
  [state_new(Point) @ offset 0]
  [state_new(Buffer) @ offset 16]
  ...

arena_1 (mmap'd, runtime sized)     arena_2 (mmap'd, runtime sized)
  [Widget @ arena offset 0]           [Node @ arena offset 0]
  [Widget @ arena offset 72]          [Node @ arena offset 48]
  ...                                 ...
```

### The 4 arena builtins (all PMT-pure)

```vuma
layout Arena = { base: u64, offset: u64, capacity: u64 }
layout Widget = { x: u32, y: u32 }

transform main() -> i32 {
    let arena = arena_new(4096);                    // mmap → State<Arena>
    let (arena, w) = arena_alloc(arena, Widget);     // bump-alloc → (State<Arena>, State<Widget>)
    w.x = 100;                                        // field access (same as static)
    let arena = arena_grow(arena, 65536);             // mremap → State<Arena>
    arena_free(arena);                                // munmap → void
    return 0;
}
```

### How it works in the codegen

The codegen already represents `State<T>` as a pointer (`ScgType::Ptr`). Field access `w.x` lowers to `Load [w + field_offset]` — this is **identical** for static and arena states. The only difference is WHERE the pointer comes from:

- Static: `state_new(Layout)` → stack allocation in `___pmt_buffer`
- Arena: `arena_alloc(arena, Layout)` → pointer to `arena.base + arena.offset`

So the arena builtins lower to **sequences of existing IR instructions** (Load/Store/BinOp/Call). No new IR instructions needed.

---

## 2. File-Ownership Map (parallelism contract)

| Domain | Files | Waves that touch them |
|---|---|---|
| **Parser/AST** | `src/parser/src/ast.rs`, `src/parser/src/parser.rs` | W1 only |
| **SCG** | `src/scg/src/node.rs`, `src/scg/src/serialize.rs`, `src/scg/src/structured_output.rs`, `src/parser/src/to_scg.rs` | W2 only |
| **Codegen bridge** | `src/pipeline.rs` | W3a only |
| **IR lowering** | `src/codegen/src/scg_to_ir.rs` | W3b only |
| **IVE** | `src/ive/src/arena_bounds.rs` (new), `src/ive/src/lib.rs` | W4 only |
| **x86_64 stubs** | `src/codegen/src/x86_64/mod.rs` | W5a only |
| **x86_32 stubs** | `src/codegen/src/x86_32/mod.rs` | W5b only |
| **aarch64 stubs** | `src/codegen/src/backend.rs` | W5c only |
| **riscv64/32 stubs** | `src/codegen/src/riscv64.rs`, `src/codegen/src/riscv32.rs` | W5d only |
| **arm32 stubs** | `src/codegen/src/arm32/mod.rs` | W5e only |
| **mips64 stubs** | `src/codegen/src/mips64/mod.rs` | W5f only |
| **ppc64 stubs** | `src/codegen/src/ppc64/mod.rs` | W5g only |
| **loongarch64 stubs** | `src/codegen/src/loongarch64/mod.rs` | W5h only |
| **s390x stubs** | `src/codegen/src/s390x.rs` | W5i only |
| **sparc64 stubs** | `src/codegen/src/sparc64.rs` | W5j only |
| **alpha stubs** | `src/codegen/src/alpha.rs` | W5k only |
| **hppa stubs** | `src/codegen/src/hppa.rs` | W5l only |
| **m68k stubs** | `src/codegen/src/m68k.rs` | W5m only |
| **wasm32 stubs** | `src/codegen/src/wasm32/mod.rs` | W5n only |
| **Runtime** | `src/codegen/src/runtime/arena.rs` (new), `src/codegen/src/runtime/mod.rs` | W3c only |
| **Tests** | `tests/gold_standard/arena_wave*/` | W6a, W6b |
| **Docs** | `docs/language-reference.md`, `docs/architecture.md` | W6c |

---

## Wave 1 — Parser/AST: Arena Builtin Recognition (sequential, 1 agent)

**Goal:** Make `arena_new`, `arena_alloc`, `arena_grow`, `arena_free` recognized as builtin functions in the parser, producing new AST nodes. This mirrors the `state_new` interception pattern.

**Rationale:** Foundation for all later waves. Single agent to avoid AST interface churn.

### Tasks

1. **Add 4 AST nodes** (`src/parser/src/ast.rs`): `Expr::ArenaNew { capacity: Box<Expr>, span: Span }`, `Expr::ArenaAlloc { arena: Box<Expr>, layout_name: String, span: Span }`, `Expr::ArenaGrow { arena: Box<Expr>, min_capacity: Box<Expr>, span: Span }`, `Expr::ArenaFree { arena: Box<Expr>, span: Span }`. Add them to the `Expr` enum, the `span()` method, the `Display` impl, and the `infer_type` method (return `State<Arena>` for arena_new/arena_grow/arena_free, `(State<Arena>, State<T>)` for arena_alloc).

2. **Intercept arena builtins** (`src/parser/src/parser.rs`): In `parse_postfix` (where `state_new` is intercepted at line ~2476), add interception for `arena_new(capacity)`, `arena_alloc(arena, LayoutName)`, `arena_grow(arena, min_capacity)`, `arena_free(arena)`. Each produces the corresponding `Expr::Arena*` node. Follow the exact pattern of the `state_new` interception: check `name == "arena_new" && args.len() == 1`, etc.

3. **Add 3 parse-only tests** (`tests/gold_standard/arena_wave0/`): `arena_new_parse.vuma`, `arena_alloc_parse.vuma`, `arena_grow_free_parse.vuma`. Each declares the builtins and asserts parse succeeds. No execution.

### DoD

- [ ] 4 `Expr::Arena*` variants in ast.rs with Display + span + infer_type
- [ ] Parser intercepts `arena_new`/`arena_alloc`/`arena_grow`/`arena_free` (same pattern as `state_new`)
- [ ] 3 parse-only tests pass
- [ ] ALL existing tests pass (no regressions)
- [ ] `cargo build` clean
- [ ] Committed

### Dispatch Box — Wave 1

```
You are Wave 1 of the VUMA Arena State Model. Task ID: W1.

READ FIRST: /home/z/my-project/worklog.md. APPEND your section when done.

GOAL: Add arena_new, arena_alloc, arena_grow, arena_free as recognized parser
builtins, producing new AST nodes. Mirror the state_new interception pattern.

YOU MAY ONLY TOUCH:
  - src/parser/src/ast.rs (add 4 Expr variants)
  - src/parser/src/parser.rs (intercept 4 builtins in parse_postfix)
  - tests/gold_standard/arena_wave0/ (new dir, 3 parse-only tests)

CONTRACT:
1. In ast.rs, add to the Expr enum (near StateInit at line ~1255):
   ArenaNew { capacity: Box<Expr>, span: Span }
   ArenaAlloc { arena: Box<Expr>, layout_name: String, span: Span }
   ArenaGrow { arena: Box<Expr>, min_capacity: Box<Expr>, span: Span }
   ArenaFree { arena: Box<Expr>, span: Span }
   Add each to: the span() method, the Display impl, the infer_type method
   (return Type::State(Box::new(Type::BDBase("Arena".to_string()))) for
   ArenaNew/Grow/Free; for ArenaAlloc return a tuple type — but since VUMA's
   Type enum doesn't have tuples, return Type::State(Box::new(Type::BDBase(layout_name.clone())))).
   Add to any exhaustive match on Expr in ast.rs.

2. In parser.rs, in parse_postfix (find "state_new" interception at line ~2476),
   add interception for the 4 arena builtins AFTER the state_new block:
   - "arena_new" with 1 arg → Expr::ArenaNew { capacity: args[0], span }
   - "arena_alloc" with 2 args (2nd must be Expr::Var) → Expr::ArenaAlloc
   - "arena_grow" with 2 args → Expr::ArenaGrow
   - "arena_free" with 1 arg → Expr::ArenaFree
   Follow the EXACT pattern of state_new (check name + args.len, transform expr).

3. Add 3 parse-only tests in tests/gold_standard/arena_wave0/:
   arena_new_parse.vuma: layout Arena = { base: u64, offset: u64, capacity: u64 }
     fn main() -> i32 { let a = arena_new(4096); arena_free(a); return 0; }
   arena_alloc_parse.vuma: layout Widget = { x: u32, y: u32 }
     fn main() -> i32 { let a = arena_new(4096); let (a, w) = arena_alloc(a, Widget); w.x = 1; arena_free(a); return 0; }
   arena_grow_free_parse.vuma: fn main() -> i32 { let a = arena_new(4096); let a = arena_grow(a, 65536); arena_free(a); return 0; }
   Each: // Expected exit code: 0 (parse-only, no execution yet)

4. Also add the 4 arena builtins to EVERY exhaustive match on Expr in:
   src/parser/src/parser.rs, src/parser/src/to_scg.rs, src/pipeline.rs,
   src/codegen/src/scg_to_ir.rs, src/codegen/src/memory_safety.rs,
   src/vuma/src/repl.rs, src/bd/src/inference.rs
   For now, add them as no-ops (return ScgExpr::Int(0) or equivalent) —
   the real lowering is Wave 3. This prevents "non-exhaustive match" errors.

BUILD + TEST:
  cd /home/z/vuma-analysis && source /home/z/.vuma_env
  cargo build --profile dev --bin compile_dump 2>&1 | tail -5
  BIN=./target/debug/compile_dump
  for t in tests/gold_standard/arena_wave0/*.vuma; do $BIN "$t" /tmp/w1.bin x86_64 2>/dev/null && echo "OK: $(basename $t)"; done
  $BIN /tmp/counter.vuma /tmp/c.bin x86_64 --verify 2>/dev/null && /tmp/c.bin; echo "counter: $?"

RULES: No stubs. No shortcuts. Fix root causes. CARGO_BUILD_JOBS=1. Commit, do NOT push.
DoD: see TASKS.md Wave 1 Definition of Done.
```

---

## Wave 2 — SCG Nodes: Arena Operations (depends on W1, 1 agent)

**Goal:** Add SCG node types for the 4 arena operations and wire the AST→SCG bridge to emit them.

### Tasks

1. **Add 4 SCG node types** (`src/scg/src/node.rs`): `NodeType::ArenaNew`, `NodeType::ArenaAlloc`, `NodeType::ArenaGrow`, `NodeType::ArenaFree`. Add `NodePayload::Arena*` variants with payloads carrying the capacity/arena/layout info. Mirror the `StateInit`/`StateTransform` pattern.

2. **Add serialization** (`src/scg/src/serialize.rs`): Add `NODE_TYPE_ARENA_NEW`/`ALLOC`/`GROW`/`FREE` constants (tags 20-23). Add serialize/deserialize for each payload. Mirror `StateTransform`.

3. **Add structured output** (`src/codegen/src/structured_output.rs`): Add display for each arena node.

4. **Wire to_scg.rs** (`src/parser/src/to_scg.rs`): When lowering `Expr::ArenaNew`/`ArenaAlloc`/`ArenaGrow`/`ArenaFree`, emit the corresponding SCG nodes.

5. **Wire all other exhaustive matches**: `src/bd/src/inference.rs`, `src/cor/src/bridge.rs`, `src/vuma/src/{msg_builder,scg_to_msg,repl}.rs`, `src/pipeline.rs` — add arena node arms (as passthrough/no-op for now).

### DoD

- [ ] 4 `NodeType::Arena*` + `NodePayload::Arena*` in node.rs
- [ ] Full serialization (tags 20-23) in serialize.rs
- [ ] Structured output in structured_output.rs
- [ ] to_scg.rs emits arena nodes
- [ ] All exhaustive matches updated (bd, cor, vuma, pipeline)
- [ ] ALL existing tests pass
- [ ] `cargo build` clean
- [ ] Committed

### Dispatch Box — Wave 2

```
You are Wave 2 of the VUMA Arena State Model. Task ID: W2.

READ FIRST: /home/z/my-project/worklog.md (Wave 1 must be complete). APPEND your section when done.

GOAL: Add SCG node types for arena_new/arena_alloc/arena_grow/arena_free and
wire the AST→SCG bridge to emit them.

YOU MAY ONLY TOUCH:
  - src/scg/src/node.rs (add 4 NodeType + NodePayload + structs)
  - src/scg/src/serialize.rs (add tags 20-23 + serialize/deserialize)
  - src/scg/src/structured_output.rs (add display)
  - src/parser/src/to_scg.rs (emit arena nodes for Expr::Arena*)
  - src/bd/src/inference.rs (add arena arms — phantom BD + None usage)
  - src/cor/src/bridge.rs (add arena arms — NodeKind::Memory)
  - src/vuma/src/msg_builder.rs, scg_to_msg.rs, repl.rs (add arena arms)
  - src/pipeline.rs (add arena arms in convert_node_to_statement)

CONTRACT:
1. In node.rs, add to NodeType enum (after ForeignConsume):
   ArenaNew, ArenaAlloc, ArenaGrow, ArenaFree
   Add to NodePayload:
   ArenaNew(ArenaNewNode) — { capacity_vreg: u32, result_vreg: u32 }
   ArenaAlloc(ArenaAllocNode) — { arena_vreg: u32, layout_name: String, result_arena_vreg: u32, result_state_vreg: u32 }
   ArenaGrow(ArenaGrowNode) — { arena_vreg: u32, min_capacity_vreg: u32, result_vreg: u32 }
   ArenaFree(ArenaFreeNode) — { arena_vreg: u32 }
   Add Display impls. Add #[derive(Debug, Clone, PartialEq, Eq, Hash)] to each struct.

2. In serialize.rs, add constants NODE_TYPE_ARENA_NEW=20, ARENA_ALLOC=21, ARENA_GROW=22, ARENA_FREE=23.
   Add to node_type_to_tag and tag_to_node_type. Add serialize/deserialize for
   each NodePayload::Arena* variant (mirror StateTransform pattern). Add to
   the node_label function.

3. In structured_output.rs, add display for each Arena* payload.

4. In to_scg.rs, find where Expr::StateInit is handled and add arms for
   Expr::ArenaNew/ArenaAlloc/ArenaGrow/ArenaFree — emit the corresponding
   SCG nodes.

5. In bd/inference.rs, cor/bridge.rs, vuma/{msg_builder,scg_to_msg,repl}.rs,
   pipeline.rs — add Arena* arms to every exhaustive match on
   NodeType/NodePayload. Use the same pattern as StateInit/StateTransform
   (phantom BD, NodeKind::Memory, ScgNodeMapping::None, etc.).

BUILD + TEST:
  cd /home/z/vuma-analysis && source /home/z/.vuma_env
  cargo build --profile dev --bin compile_dump 2>&1 | tail -5
  BIN=./target/debug/compile_dump
  $BIN tests/gold_standard/arena_wave0/arena_new_parse.vuma /tmp/w2.bin x86_64 2>/dev/null && echo "OK"
  $BIN /tmp/counter.vuma /tmp/c.bin x86_64 --verify 2>/dev/null && /tmp/c.bin; echo "counter: $?"

RULES: No stubs. No shortcuts. CARGO_BUILD_JOBS=1. Commit, do NOT push.
```

---

## Wave 3 — Codegen Bridge: Arena Builtin Lowering (depends on W1+W2, 3 parallel agents)

**Goal:** Lower the 4 arena builtins to sequences of existing IR instructions in the AST→codegen bridge. This is the core implementation wave.

### Task 3a — Arena lowering in pipeline.rs

**Files:** `src/pipeline.rs` ONLY.

**Contract:** In `flatten_expr` and `bridge_stmt_to_scg`, handle `Expr::ArenaNew`/`ArenaAlloc`/`ArenaGrow`/`ArenaFree`:

- `arena_new(capacity)`: Emit a `CallNode` to `mmap` with args (0, capacity, 3, 0x22, -1, 0). The result is the arena base pointer. Emit a `Stack` allocation for the Arena struct (24 bytes: base + offset + capacity). Store base=mmap_result, offset=0, capacity=capacity into the arena struct. Return the arena variable.

- `arena_alloc(arena, Layout)`: Load arena.base, arena.offset, arena.capacity. Emit a runtime bounds check: if offset + layout_size > capacity → call `arena_grow` (or trap). Compute ptr = base + offset. Store arena.offset = offset + layout_size (bump). Return (arena, ptr) as a tuple — the ptr becomes the State<T> variable.

- `arena_grow(arena, min_capacity)`: Load arena.base, arena.capacity. Call `mremap(base, capacity, min_capacity, 1)` (MREMAP_MAYMOVE=1). Store arena.base = mremap_result, arena.capacity = min_capacity. Return arena.

- `arena_free(arena)`: Load arena.base, arena.capacity. Call `munmap(base, capacity)`. Return void.

### DoD

- [ ] `arena_new` lowers to mmap call + Arena struct init
- [ ] `arena_alloc` lowers to bounds check + bump + ptr computation
- [ ] `arena_grow` lowers to mremap call
- [ ] `arena_free` lowers to munmap call
- [ ] Arena states support field access (w.x works — same as static states)
- [ ] ALL existing tests pass
- [ ] Committed

### Task 3b — IR lowering in scg_to_ir.rs

**Files:** `src/codegen/src/scg_to_ir.rs` ONLY.

**Contract:** Handle `ScgStatement::ArenaNew`/`ArenaAlloc`/`ArenaGrow`/`ArenaFree` (if 3a emits custom ScgStatement variants) OR ensure the CallNodes emitted by 3a lower correctly (if 3a uses existing CallNode). If 3a uses CallNodes, this task is a no-op (verify and document). If custom ScgStatement variants are needed, add them and lower them.

### DoD

- [ ] Arena CallNodes lower correctly to IR
- [ ] (If needed) Custom Arena ScgStatement variants lower to IR
- [ ] ALL existing tests pass
- [ ] Committed

### Task 3c — Runtime arena module

**Files:** `src/codegen/src/runtime/arena.rs` (new), `src/codegen/src/runtime/mod.rs` ONLY.

**Contract:** Create a Rust-level arena allocator module (for testing and for the vuma_context callback path). This is NOT the codegen stub — it's the Rust runtime that the arena builtins conceptually wrap. It provides:

```rust
pub struct Arena { base: *mut u8, offset: usize, capacity: usize }
pub fn arena_create(capacity: usize) -> Arena;  // mmap
pub fn arena_alloc<T>(arena: &mut Arena) -> *mut T;  // bump + bounds check
pub fn arena_grow(arena: &mut Arena, min_capacity: usize);  // mremap
pub fn arena_destroy(arena: Arena);  // munmap
```

With unit tests. This module is linked into the VUMA binary for the callback path (Wave 7 vuma_context) and for testing.

### DoD

- [ ] `runtime/arena.rs` exists with Arena struct + 4 functions
- [ ] `runtime/mod.rs` declares `pub mod arena;`
- [ ] 5+ unit tests (create, alloc, grow, destroy, bounds_check_overflow)
- [ ] `cargo test -p vuma-codegen arena` passes
- [ ] Committed

### Dispatch Box — Wave 3 (dispatch 3a + 3b + 3c in ONE message)

```
=== COMMON PREAMBLE ===
You are Wave {ID} of the VUMA Arena State Model.
READ FIRST: /home/z/my-project/worklog.md (Waves 1+2 must be complete). APPEND your section when done.
RULES: No stubs. No shortcuts. CARGO_BUILD_JOBS=1. Commit, do NOT push.
BUILD: cd /home/z/vuma-analysis && source /home/z/.vuma_env && cargo build --profile dev --bin compile_dump 2>&1 | tail -5

=== 3a: Arena lowering in pipeline.rs ===
TASK ID: W3a. FILES: src/pipeline.rs ONLY.
GOAL: Lower Expr::ArenaNew/ArenaAlloc/ArenaGrow/ArenaFree to IR instruction
sequences using existing CallNode (mmap/mremap/munmap) + Load/Store/BinOp.
CONTRACT:
- arena_new(capacity): Call mmap(0, capacity, 3, 0x22, -1, 0) → base ptr.
  Alloc Arena struct (24 bytes: base u64 + offset u64 + capacity u64).
  Store base, offset=0, capacity. Return arena var.
- arena_alloc(arena, Layout): Load arena.base/offset/capacity.
  Bounds check: if offset + layout_size > capacity → trap (emit a CallNode
  to a "__arena_overflow" stub that aborts, or call arena_grow inline).
  ptr = base + offset. Store arena.offset = offset + layout_size.
  Return (arena, ptr) — ptr becomes the State<T> var.
- arena_grow(arena, min_cap): Load arena.base/capacity.
  Call mremap(base, capacity, min_cap, 1). Store arena.base/capacity.
- arena_free(arena): Load arena.base/capacity. Call munmap(base, capacity).
Use the existing CallNode pattern (is_extern: true, func: "mmap"/"mremap"/"munmap").
These are ALREADY registered as syscall stubs on all 19 backends.
DoD: arena builtins compile and run (arena_new returns a valid Arena state,
arena_alloc returns a writable State<T>, field access works).

=== 3b: IR lowering in scg_to_ir.rs ===
TASK ID: W3b. FILES: src/codegen/src/scg_to_ir.rs ONLY.
GOAL: Ensure the CallNodes emitted by W3a lower correctly to IR. If W3a uses
only existing CallNode/Load/Store/BinOp, this is a verification task (confirm
+ document). If custom ScgStatement::Arena* variants are needed, add them
and lower them to IR instruction sequences.
CONTRACT: Check that mmap/mremap/munmap CallNodes resolve to the existing
syscall stubs (already registered in func_offsets on all backends).
DoD: arena programs compile to valid IR; no "unresolved symbol" errors.

=== 3c: Runtime arena module ===
TASK ID: W3c. FILES: src/codegen/src/runtime/arena.rs (new), src/codegen/src/runtime/mod.rs ONLY.
GOAL: Create the Rust-level arena allocator for testing + callback path.
CONTRACT:
pub struct Arena { base: *mut u8, offset: usize, capacity: usize }
pub fn arena_create(capacity: usize) -> Arena;  // mmap
pub fn arena_alloc<T>(arena: &mut Arena) -> *mut T;  // bump + bounds check (panic on overflow)
pub fn arena_grow(arena: &mut Arena, min_capacity: usize);  // mremap
pub fn arena_destroy(arena: Arena);  // munmap
5+ unit tests: create, alloc, grow, destroy, bounds_check_overflow.
DoD: cargo test -p vuma-codegen arena passes.
```

---

## Wave 4 — IVE: Arena Bounds Checking (depends on W2, 1 agent)

**Goal:** Add compile-time + runtime bounds checking for arena states. The IVE proves that arena_alloc's bounds check is present and correct.

### Tasks

1. **Create `src/ive/src/arena_bounds.rs`** (new): A verifier that checks every `ArenaAlloc` node has a preceding bounds check (`offset + layout_size <= capacity`). At the SCG level, this is a structural check: the ArenaAlloc node must be preceded by a conditional branch that traps on overflow. If the codegen (W3a) always emits the bounds check, this verifier confirms it's present.

2. **Register in `src/ive/src/lib.rs`**: `pub mod arena_bounds;`

3. **Wire into the invariant aggregator** (`src/ive/src/invariant_aggregator.rs`): Add `ArenaAlloc` to the consumed-set tracking (the arena is consumed by arena_alloc, same as StateTransform).

### DoD

- [ ] `arena_bounds.rs` exists with the verifier
- [ ] `lib.rs` declares `pub mod arena_bounds;`
- [ ] `invariant_aggregator.rs` treats `ArenaAlloc` arena input as consumed (linearity)
- [ ] 3+ unit tests (bounds_check_present, bounds_check_missing, linearity)
- [ ] ALL existing tests pass
- [ ] Committed

### Dispatch Box — Wave 4

```
You are Wave 4 of the VUMA Arena State Model. Task ID: W4.
READ FIRST: /home/z/my-project/worklog.md (Waves 1-3 must be complete). APPEND your section when done.
FILES: src/ive/src/arena_bounds.rs (new), src/ive/src/lib.rs, src/ive/src/invariant_aggregator.rs ONLY.
GOAL: Add arena bounds checking verifier + linearity tracking for ArenaAlloc.
CONTRACT: [paste Wave 4 tasks above]
BUILD: cd /home/z/vuma-analysis && source /home/z/.vuma_env && cargo build --profile dev --bin compile_dump 2>&1 | tail -5
RULES: No stubs. No shortcuts. CARGO_BUILD_JOBS=1. Commit, do NOT push.
```

---

## Wave 5 — Syscall Stubs: arena_mmap/arena_mremap/arena_munmap (depends on W3, 14 parallel agents — MAX)

**Goal:** Ensure `mmap`, `mremap`, and `munmap` syscall stubs are registered in `func_offsets` on ALL 19 backends. Most backends already have these (from the FFI work), but this wave verifies completeness and adds any missing ones.

**NOTE:** `mmap`/`munmap`/`mremap` already exist on all 14 native backends (verified in audit). This wave is primarily VERIFICATION + adding `__arena_overflow` trap stub (used by arena_alloc's bounds check) to every backend. The trap stub is a real `exit(1)` syscall (not a no-op).

### Tasks (per backend)

For each of the 15 primary backends, verify `mmap`/`mremap`/`munmap` are in `func_offsets`, and add `__arena_overflow` (a real `exit(1)` syscall stub) if missing.

### DoD (per backend)

- [ ] `mmap` stub registered in func_offsets ✓
- [ ] `mremap` stub registered in func_offsets ✓
- [ ] `munmap` stub registered in func_offsets ✓
- [ ] `__arena_overflow` stub registered (real `exit(1)` syscall, not a no-op)
- [ ] Committed

### Dispatch Box — Wave 5 (dispatch up to 4 at a time)

> **Orchestrator:** dispatch in rounds of 4: (5a,5b,5c,5d), then (5e,5f,5g,5h), then (5i,5j,5k,5l), then (5m,5n).

```
=== COMMON PREAMBLE ===
You are Wave {ID} of the VUMA Arena State Model.
READ FIRST: /home/z/my-project/worklog.md. APPEND your section when done.
RULES: No stubs. No shortcuts. CARGO_BUILD_JOBS=1. Commit, do NOT push.
BUILD: cd /home/z/vuma-analysis && source /home/z/.vuma_env && cargo build --profile dev --bin compile_dump 2>&1 | tail -5

=== 5a: x86_64 ===
TASK ID: W5a. FILE: src/codegen/src/x86_64/mod.rs ONLY.
GOAL: Verify mmap/mremap/munmap stubs exist. Add __arena_overflow stub
(real exit(1) syscall: mov eax, 60; mov edi, 1; syscall; int3).
CONTRACT: grep for "mmap", "mremap", "munmap" — confirm each is in stubs.push.
Add __arena_overflow as: mov eax, 60 (sys_exit); mov edi, 1 (exit code 1);
syscall; int3 (safety guard). Push as ("__arena_overflow", code).
DoD: all 4 stubs registered in func_offsets.

=== 5b: x86_32 ===
TASK ID: W5b. FILE: src/codegen/src/x86_32/mod.rs ONLY.
Same as 5a but i386: mov eax, 1 (sys_exit); mov ebx, 1; int 0x80; int3.

=== 5c: aarch64 (backend.rs) ===
TASK ID: W5c. FILE: src/codegen/src/backend.rs ONLY.
Same but aarch64: movz x0, #1; movz x8, #93 (sys_exit); svc #0; brk #0.

=== 5d: riscv64 ===
TASK ID: W5d. FILE: src/codegen/src/riscv64.rs ONLY.
Same but riscv64: li a0, 1; li a7, 93 (sys_exit); ecall; unimp.

=== 5e: riscv32 ===
TASK ID: W5e. FILE: src/codegen/src/riscv32.rs ONLY. Same as 5d.

=== 5f: arm32 ===
TASK ID: W5f. FILE: src/codegen/src/arm32/mod.rs ONLY.
arm32: mov r0, #1; mov r7, #1 (sys_exit); svc #0; bkpt #0.

=== 5g: mips64 ===
TASK ID: W5g. FILE: src/codegen/src/mips64/mod.rs ONLY.
mips64: li a0, 1; li v0, 5058 (sys_exit); syscall; break.

=== 5h: ppc64 ===
TASK ID: W5h. FILE: src/codegen/src/ppc64/mod.rs ONLY.
ppc64: li r3, 1; li r0, 1 (sys_exit); sc; trap.

=== 5i: loongarch64 ===
TASK ID: W5i. FILE: src/codegen/src/loongarch64/mod.rs ONLY.
loongarch64: li a0, 1; li a7, 93 (sys_exit); syscall; break 0.

=== 5j: s390x ===
TASK ID: W5j. FILE: src/codegen/src/s390x.rs ONLY.
s390x: lghi r2, 1; lghi r1, 1 (sys_exit); svc 0; trap.

=== 5k: sparc64 ===
TASK ID: W5k. FILE: src/codegen/src/sparc64.rs ONLY.
sparc64: mov 1, %o0; mov 1, %g1 (sys_exit); ta 0x6d; unimp.

=== 5l: alpha ===
TASK ID: W5l. FILE: src/codegen/src/alpha.rs ONLY.
alpha: lda v0, 1; callsys (with v0=exit nr); call_pal 0x83; unop.

=== 5m: hppa ===
TASK ID: W5m. FILE: src/codegen/src/hppa.rs ONLY.
hppa: li r26, 1; li r20, __NR_exit; gate; bv; nop.

=== 5n: m68k ===
TASK ID: W5n. FILE: src/codegen/src/m68k.rs ONLY.
m68k: moveq #1, d1; moveq #1, d0 (sys_exit); trap #0; illegal.

=== 5o: wasm32 ===
TASK ID: W5o. FILE: src/codegen/src/wasm32/mod.rs ONLY.
wasm32: __arena_overflow calls proc_exit(1) (WASI). Register in func_name_to_idx.
```

---

## Wave 6 — Tests + Docs (depends on all, 3 parallel agents)

### Task 6a — Arena functional tests

**Files:** `tests/gold_standard/arena_wave1/` (new).

**Contract:** Create tests that exercise the arena model end-to-end:
- `arena_basic.vuma`: arena_new → arena_alloc → field write/read → arena_free (exit 0)
- `arena_grow.vuma`: arena_new(64) → arena_alloc → arena_grow(4096) → arena_alloc → arena_free (exit 0)
- `arena_multiple.vuma`: arena_new → 3× arena_alloc (different layouts) → field access → arena_free (exit 0)
- `arena_overflow.vuma`: arena_new(8) → arena_alloc(Widget=72 bytes) → should trap (exit 1)

### DoD

- [ ] 4 arena tests pass on x86_64 + aarch64 (at minimum)
- [ ] `arena_basic.vuma` passes on ALL 19 backends
- [ ] Committed

### Task 6b — Arena regression tests

**Files:** `tests/gold_standard/arena_wave2/` (new).

**Contract:** Create tests that verify arena states interact correctly with static states:
- `mixed_static_arena.vuma`: state_new(Point) + arena_alloc(Widget) — both work
- `arena_in_transform.vuma`: transform that uses arena_alloc internally
- `arena_linearity.vuma`: arena consumed by arena_alloc is re-produced (PMT linearity)

### DoD

- [ ] 3 regression tests pass on x86_64 + aarch64
- [ ] Committed

### Task 6c — Docs

**Files:** `docs/language-reference.md`, `docs/architecture.md`.

**Contract:** Add a new section "15. Arena States" to language-reference.md documenting the 4 arena builtins, the Arena layout, and worked examples. Add a new section "10. Arena State Model" to architecture.md documenting the tiered model, the memory layout, and the bounds checking.

### DoD

- [ ] language-reference.md §15 documents arena builtins + examples
- [ ] architecture.md §10 documents tiered model + memory layout
- [ ] Committed

### Dispatch Box — Wave 6 (dispatch 6a + 6b + 6c in ONE message)

```
=== COMMON PREAMBLE ===
You are Wave {ID} of the VUMA Arena State Model.
READ FIRST: /home/z/my-project/worklog.md (Waves 1-5 must be complete). APPEND your section when done.
RULES: No stubs. No shortcuts. CARGO_BUILD_JOBS=1. Commit, do NOT push.
BUILD: cd /home/z/vuma-analysis && source /home/z/.vuma_env && cargo build --profile dev --bin compile_dump 2>&1 | tail -5

=== 6a: Arena functional tests ===
TASK ID: W6a. FILES: tests/gold_standard/arena_wave1/ (new).
CONTRACT: [paste Task 6a contract above]
DoD: 4 tests pass on x86_64 + aarch64; arena_basic passes on all 19 backends.

=== 6b: Arena regression tests ===
TASK ID: W6b. FILES: tests/gold_standard/arena_wave2/ (new).
CONTRACT: [paste Task 6b contract above]
DoD: 3 tests pass on x86_64 + aarch64.

=== 6c: Docs ===
TASK ID: W6c. FILES: docs/language-reference.md, docs/architecture.md.
CONTRACT: [paste Task 6c contract above]
DoD: both docs updated with arena sections.
```

---

## 3. Wave Dependency Graph

```
W1 (parser/AST) ──► W2 (SCG nodes) ──┬──► W3a (pipeline lowering) ──► W6 (tests + docs)
                                     ├──► W3b (IR lowering)         ──►
                                     ├──► W3c (runtime arena)       ──►
                                     └──► W4 (IVE bounds)           ──►
                                          │
W3 (codegen) ──► W5 (syscall stubs) ──────┘
```

**Parallel opportunities:**
- **W3:** 3a ∥ 3b ∥ 3c (3 agents — disjoint files)
- **W5:** up to 4 at a time (14 backend tasks, 4 rounds)
- **W6:** 6a ∥ 6b ∥ 6c (3 agents — tests vs docs)

---

## 4. Definition of Done (whole effort)

- [ ] `arena_new` / `arena_alloc` / `arena_grow` / `arena_free` parse, lower, and execute on ALL 19 backends
- [ ] Arena states support field access (w.x) identical to static states
- [ ] Arena bounds checking works (overflow → trap)
- [ ] Arena grow works (mremap — existing offsets stay valid)
- [ ] Arena free works (munmap — no per-object free)
- [ ] `mmap`/`mremap`/`munmap`/`__arena_overflow` stubs on ALL 19 backends
- [ ] Static states (`state_new`) unchanged — no regression
- [ ] Sacred invariants preserved (no pointers, ___pmt_buffer pure, 3 PMT verifiers canonical)
- [ ] ALL existing 704+ gold-standard tests pass at 100%
- [ ] Arena tests pass on ALL 19 backends
- [ ] Docs updated

---

## 5. Success Criteria

1. **Dynamic memory without pointers.** VUMA programs can allocate runtime-sized collections (DOM trees, widget trees, request pools) using `arena_alloc` — all via `State<T>`, no pointer syntax.
2. **Deterministic mode preserved.** Programs using only `state_new` are unchanged — compile-time sized, zero-overhead, suitable for embedded/kernel.
3. **All 19 backends.** Every backend has real `mmap`/`mremap`/`munmap` syscall stubs. No backend is missing arena support.
4. **No per-object malloc/free.** Arena uses bump-allocation + bulk-free. Sacred invariant preserved.
5. **PMT-pure.** Arena builtins are transforms that consume/produce `State<Arena>` linearly. The 3 PMT verifiers work on both static and arena states.

---

*This spec is a living document. Update the DoD checkboxes as waves complete. Record deviations in `/home/z/my-project/worklog.md`.*
