# VUMA 2.0 Architecture

VUMA (Verified-Unsafe Memory Access) 2.0 is a **PMT-only** systems
programming language: every program is a composition of typed *state
transforms* over a single memory buffer. Pointers, allocation, and
deallocation are not features the compiler reluctantly checks — they are
**hard parse errors**. `allocate`, `free`, `*ptr`, `&x`, and `*T` never
reach the type checker; the lexer rejects them in
[`src/parser/src/parser.rs`](../src/parser/src/parser.rs) via
`check_pointer_syntax`. What remains is **PMT** (Programs as Memory
Transformations): a `layout` describes the byte shape of a buffer; a
`State<T>` is a typed view of the program-wide memory buffer; a
`transform` is a pure function `State<T> -> State<U>` that reads fields,
computes new values, and writes fields back. Memory safety becomes a
*structural type-checking property*, not a constraint solver over an
unbounded heap graph.

This document is the canonical reference for the VUMA 2.0 *compiler*. It
covers the compilation pipeline, the state type system, the arena state
model, the three IVE state verifiers, the Behavioral Descriptor layer,
the e-graph layout optimizer, the 19 production backends, the FFI 4-mode
marshal matrix, if-expression lowering, nested layout resolution, and the
known VUMA parser limitations. For the language itself (syntax,
examples, semantics) see [`language-reference.md`](./language-reference.md);
for the VWK kernel that compiles with this compiler see
[`kernel-architecture.md`](./kernel-architecture.md).

---

## Table of Contents

1. [Overview](#1-overview)
2. [The PMT Pipeline](#2-the-pmt-pipeline)
3. [State Type System](#3-state-type-system)
4. [Arena State Model](#4-arena-state-model)
5. [Verification — The Three State Verifiers](#5-verification--the-three-state-verifiers)
6. [Behavioral Descriptors (BD)](#6-behavioral-descriptors-bd)
7. [E-graph Layout Optimization](#7-e-graph-layout-optimization)
8. [Backends](#8-backends)
9. [FFI — The 4-Mode Matrix](#9-ffi--the-4-mode-matrix)
10. [If-Expression Lowering](#10-if-expression-lowering)
11. [Nested Layout Resolution](#11-nested-layout-resolution)
12. [VUMA Parser Limitations](#12-vuma-parser-limitations)

---

## 1. Overview

VUMA 2.0 collapses the **five pointer invariants** of VUMA 1.x —
*liveness* (no use-after-free), *exclusivity* (no aliasing during
mutation), *cleanup* (no double-free), *origin* (no uninitialized
reads), and *interpretation* (no type-punning) — into **three
structural type-checking rules**:

- **`StateReadVerifier`** — every state-field read is in-bounds and
  reads an initialized field whose declared type matches the read type.
- **`StateWriteVerifier`** — every state-field write is in-bounds,
  type-matches, and respects linear ownership (no write-after-consume).
- **`StateTransformVerifier`** — every state-to-state transform is
  layout-compatible (identity, same-size reinterpret, or copy).

The remaining two invariants — *liveness* and *cleanup* — are
discharged by construction: the source language contains no `free` and
no `drop`, so there is no resource to leak, double-free, or use after
freeing. *Origin* is discharged by the `LayoutRegistry` (every state
value carries a `LayoutId` that names its provenance). *Exclusivity*
and *interpretation* collapse into a single field-offset + type-match
check, because the absence of pointer arithmetic means there is no
aliasing to reason about.

This is the **5→3 invariant collapse** that gives PMT its power: every
memory-safety obligation is either eliminated by syntax or reduced to a
compile-time type check that runs at SCG construction time, *before*
codegen. There is no `--pmt` flag and no `Normal`/`Exhaustive`/`Hardened`
verification level — `VerificationLevel::Pmt` is the only level the
compiler ever constructs.

The compiler is a Cargo workspace of **10 internal crates** (~329K LOC
of Rust):

| Crate            | Package          | Role                                                                 |
|------------------|------------------|----------------------------------------------------------------------|
| `src/parser/`    | `vuma-parser`    | Lexer, recursive-descent parser, AST, AST→SCG lowering, error path. |
| `src/scg/`       | `vuma-scg`       | Semantic Computation Graph — the formal graph IR.                    |
| `src/bd/`        | `vuma-bd`        | Behavioral Descriptors — `RepD`, `CapD`, `RelD` lattices + inference.|
| `src/ive/`       | `vuma-ive`       | Inference & Verification Engine — the 3 state verifiers + FFI.       |
| `src/codegen/`   | `vuma-codegen`   | 19 backends, scheduler, e-graph, regalloc, ELF / Wasm emission.      |
| `src/proof/`     | `vuma-proof`     | Formal proof system — checker, tactics, counterexamples.             |
| `src/cor/`       | `vuma-cor`       | Continuous Optimization Runtime — JIT, profiling, speculation.       |
| `src/vuma/`      | `vuma-core`      | Memory model, MSG construction, security.                            |
| `src/package/`   | `vuma-package`   | Package manager — manifest parser, dependency resolver, registry.    |
| `src/tests/`     | `vuma-tests`     | Integration test framework.                                          |

The top-level `compile` entry point in
[`src/pipeline.rs`](../src/pipeline.rs) wires parser → AST → codegen-SCG
→ IR → optimisation → register allocation → emission. All schedulers
and optimisers are unconditionally enabled; `--verify` only gates
whether the IVE state verifiers run as a final pre-emit gate (with
`--verify`, an IVE failure aborts compilation before emission;
without `--verify`, the IVE result is reported but compilation
proceeds).

---

## 2. The PMT Pipeline

The pipeline is the single source of truth used by `compile`,
`compile_with_path`, `compile_modules`, and `compile_to_wasm` — all four
entry points call `bridge_ast_to_codegen_scg` followed by
`run_ir_pipeline`. The pipeline does **not** branch on a `--pmt` flag;
PMT is the only mode.

```
   .vuma source
        │
        ▼
   parser  (vuma-parser)
     · lexer → tokens
     · recursive-descent → AST (Item::FnDef, Item::LayoutDef,
                                  Item::ExternBlock, transform decls)
     · PMT-only enforcement: pointer syntax → fatal ParseError
        │
        ▼
   AST  (layout, State<T>, state_new, transform, extern "C")
        │
        ▼
   bridge_ast_to_codegen_scg  (src/pipeline.rs)
     · build_layout_registry (3-pass: collect → resolve sizes
                                iteratively → assign offsets)
     · state_new   → Alloc / Offset into ___pmt_buffer
     · state.f     → Load  (offset, size)  — offset is a compile-time constant
     · state.f = v → Store (offset, value, size)
     · state.arr[i]→ Load + Index (offset + i*elem_size)
     · transform   → Alloc + copy (or identity when L_in == L_out)
     · arena_new   → mmap; arena_alloc → bump + bounds check
     · if-expr     → ControlNode::If + result temp
        │
        ▼
   SCG  (vuma-scg)
     · Semantic Computation Graph IR
     · nodes: StateInit, StateRead, StateWrite, StateTransform,
              ForeignConsume, BinOp, CondBranch, Call, ControlNode::If
        │
        ▼
   IVE  (vuma-ive, VerificationLevel::Pmt)
     · StateReadVerifier   (state_read.rs)
     · StateWriteVerifier  (state_write.rs — linearity check)
     · StateTransformVerifier (state_transform.rs — Identity/Reinterpret/Copy)
     · FFI safety verifier (ffi.rs — no access to invalidated state)
     · Borrow-region verifier (borrow_region.rs — no write during borrow)
     · Arena-bounds verifier (arena_bounds.rs — no access after arena_free)
        │
        ▼
   IR lowering  (vuma-codegen/src/scg_to_ir.rs)
     · monomorphize  · lower_closures  · lower_switches
     · lower_tail_calls  · normalize_loops  · bv_verify
        │
        ▼
   O2 optimizer pipeline  (always on)
     · constant_fold + cse  · equality_saturation (e-graph, 35+ rules)
     · dead_store_eliminate · inline_with_threshold
     · licm (always on)    · schedule_function (always on)
     · cross_function_constant_prop · identical_function_merge
     · whole_program_dce   · loop_unroll
     · escape + effects (SROA, alloc elision, pure-fn marking)
     · vectorize (loop + SLP)
        │
        ▼
   regalloc  (LinearScan, parallel per-fn)
        │
        ▼
   19 backends  (isel + per-ISA optimisation + syscall ABI translation)
   x86_64 aarch64 aarch64_be riscv64 riscv32 arm32 armeb
   mips64 mips64be ppc64 ppc64le loongarch64 s390x sparc64
   alpha hppa m68k x86_32 wasm32
        │
        ▼
   .bin  (ELF for the target arch, or wasm32 module)
```

The pipeline is monolithic in source order but each stage is
independently testable. The scheduler models memory dependencies via
cast-aware type-based alias analysis (TBAA) with IVE-proven
non-aliasing overrides. The e-graph feeds both binop algebraic rules
(35+ rules) and PMT state-op rewrites (`state_dead_init_elim`,
`state_store_load_forward`, `state_transform_elision`).

---

## 3. State Type System

The PMT type system has four orthogonal constructs: `layout`,
`State<T>`, `state_new`, and `transform`. Together they make memory
safety a structural type-checking property.

### 3.1 Layouts

A `layout` is a named product of fields with computed offsets and
alignment. Layouts are first-class top-level items and may nest or
contain fixed-size arrays:

```vuma
// tests/gold_standard/pmt_wave2/init_read.vuma
layout Point = { x: u32, y: u32 }

fn main() -> i32 {
    let p = state_new(Point);
    p.x = 42;
    return p.x;   // → 42
}

// tests/gold_standard/pmt_wave9/stack.vuma
layout Stack = { count: u32, data: [u32; 8] }
```

At registration time the `LayoutRegistry`
([`vuma-bd/src/repd.rs`](../src/bd/src/repd.rs)) lays out fields
sequentially, inserting alignment padding before each field and rounding
the total size up to the layout's alignment so arrays of the layout are
correctly strided. For `Stack` above the resolved `LayoutDef` has
`count` at offset 0 (size 4), `data` at offset 4 (size 32),
`total_size = 36`, `alignment = 4`. Layouts **must** be declared before
the first function that uses them — the layout registry is single-pass
within a function (the multi-pass size resolver only fixes
cross-layout nesting, not forward references).

### 3.2 `State<T>` and `state_new`

`state_new(Layout)` is the sole allocation primitive. It produces a
`State<T>` — a linear handle to a buffer whose shape is described by
`T`'s `LayoutDef`. The handle is *not* a pointer: it cannot be
dereferenced or stored in a `u64`. The only operations on a `State<T>`
are field reads, field writes, layout transforms, the `as Address` cast
at the FFI boundary (see §9), and passing to other VUMA functions (by
value — the callee receives the buffer's address under the hood):

```vuma
// tests/gold_standard/pmt_wave2/transform_id.vuma
layout Point = { x: u32, y: u32 }

fn id(s: State<Point>) -> u32 {
    return s.x;
}

fn main() -> i32 {
    let p = state_new(Point);
    p.x = 99;
    return id(p);
}
```

### 3.3 Field access

`state.field` is a typed offset load; `state.field = expr` is a typed
offset store. The bridge resolves the field name against the state's
layout and emits an IR `Load { addr, offset, size }` or
`Store { addr, offset, value, size }`. There is no address-of, no
pointer arithmetic, no reinterpretation — the offset and size come from
the `LayoutDef`, not from the source program. Array fields use
`state.arr[i]`; the index is multiplied by the element size at bridge
time and added to the field offset:

```vuma
// tests/gold_standard/pmt_wave9/stack.vuma
fn push(s: State<Stack>, v: u32) {
    s.data[s.count] = v;     // Store at offset 4 + (s.count * 4)
    s.count = s.count + 1;
}
```

Nested layouts (`l.a.x`) lower through a field-chain resolver that
descends into nested `LayoutDef`s, summing offsets (see §11).

### 3.4 Transforms

A `transform` is a pure function from `State<L_in>` to `State<L_out>`.
The `StateTransformVerifier` proves the two layouts are compatible:

- **Identity** — `in_layout == out_layout`. The buffer is returned
  unchanged; the e-graph's `state_transform_elision` rule rewrites the
  transform away at O2.
- **Reinterpret** — `in.total_size == out.total_size`. The buffer is
  reinterpreted in place at zero cost.
- **Copy** — different sizes. The compiler emits a fresh `Alloc` of
  `out.total_size` and a `Store`-by-`Store` copy of the source buffer.

```vuma
// tests/gold_standard/pmt_wave1/transform_decl.vuma
layout Request  = { opcode: u32, arg: u32 }
layout Response = { status: u32, result: u32 }

transform handle(req: State<Request>) -> State<Response> {
    // Body: read req.opcode, req.arg; write result.status, result.result.
    // (Request and Response are both 8 bytes → Reinterpret.)
    return;
}
```

When a transform consumes a state, the consumed variable is added to
`consumed_vars`; any subsequent `state.field` access on it is a
linearity violation rejected by `StateWriteVerifier`.

### 3.5 Single-buffer lowering

Under the Wave 8a lowering, every `state_new` in a function resolves to
an `Offset` into a single program-wide buffer `___pmt_buffer` rather
than to its own stack slot. The IRBuilder emits **one** `Alloc` for
`___pmt_buffer` (sized to the sum of all state sizes, 16-byte aligned)
at function entry, then lowers each `state_new(Layout)` to
`state = ___pmt_buffer + slot_offset`. There are zero per-state
`Alloc` instructions at runtime:

```vuma
// tests/gold_standard/pmt_wave8/single_buffer.vuma
layout Point = { x: u32, y: u32 }

fn main() -> i32 {
    let p = state_new(Point);   // → p = ___pmt_buffer + 0
    let q = state_new(Point);   // → q = ___pmt_buffer + 16
    p.x = 10;
    q.x = 20;
    return p.x + q.x;           // 30 — no aliasing
}
```

### 3.6 Linearity

States are linear: a state consumed by a `transform` (or by an
`as Address` cast at an FFI boundary, or by a `#[foreign_consume]`
close-call) may not be read or written afterwards. The
`StateWriteVerifier` enforces this by tracking the set of
`consumed_vars` and rejecting any write targeting a consumed state with
a `linearity violation` diagnostic. Reads of consumed states are
rejected by the same path. There is no `drop`; a state goes out of
scope at the end of its enclosing function and its slot is reclaimed
with the single buffer.

---

## 4. Arena State Model

`state_new` is the *sole static allocation primitive* — every
`state_new(Layout)` reserves a fixed slot at compile time. Many
programs (kernels, network stacks, parsers) need *runtime-growable*
memory without giving up the no-pointers discipline. The **arena
state model** (`womb/alloc/arena.vuma` library module + the runtime in
[`vuma-codegen/src/runtime/arena.rs`](../src/codegen/src/runtime/arena.rs))
provides it. The arena surface has four primitives:

| Primitive                  | AST node              | Lowers to                                                    |
|----------------------------|-----------------------|--------------------------------------------------------------|
| `arena_new(capacity)`      | `Expr::ArenaNew`      | `mmap(NULL, capacity, RW, ANON, -1, 0)` → `State<Arena>`     |
| `arena_alloc(a, L)`        | `Expr::ArenaAlloc`    | bump `a.offset` by `sizeof(L)`; bounds-check; return base    |
| `arena_grow(a, min_cap)`   | `Expr::ArenaGrow`     | `mremap(a.base, a.capacity, min_cap, MREMAP_MAYMOVE)`        |
| `arena_free(a)`            | `Expr::ArenaFree`     | `munmap(a.base, a.capacity)`                                 |

### 4.1 The Arena layout

The runtime treats the arena's first 24 bytes as a header
(`runtime/arena.rs`):

```
   arena_ptr  ┌──────────────────────────────────────────────────┐
              │  [ptr+0]   base address (== arena_ptr itself)    │
              │  [ptr+8]   current bump offset                   │
              │  [ptr+16]  capacity  (the mmap'd region size)    │
              ├──────────────────────────────────────────────────┤
              │  … arena data starts here at offset 24 …         │
              │  every arena_alloc returns (arena_ptr + offset)  │
              ▼                                                  │
             bump offset ──────►  (grows toward capacity)        │
              ───────────────────────────────────────────────────│
              │  unmapped / overflow zone                        │
              └──────────────────────────────────────────────────┘
```

`arena_alloc(a, LayoutName)` is linear (consumes and re-emits the
arena), and the result is a fresh `State<LayoutName>` whose base
address is `arena_ptr + offset`. Field access on the result is then
identical to a `state_new`'d state — same `Load`/`Store` lowering,
same IVE checks. There is no way for the program to learn the arena's
base pointer as a `u64` and produce pointer arithmetic; the only
exposure to the address is through the typed `State<T>` view.

### 4.2 Bounds checking — the `__arena_overflow` trap

Every `arena_alloc` site emits a runtime bounds check
([`src/pipeline.rs`](../src/pipeline.rs), the `Expr::ArenaAlloc` arm
of `flatten_expr`):

```
   off      = load([arena_ptr + 8])           // current offset
   new_off  = off + layout_size
   cap      = load([arena_ptr + 16])          // capacity
   if new_off > cap:                          // unsigned compare
       call __arena_overflow(layout_id)       // exits(1) / halts CPU
   store(new_off, [arena_ptr + 8])            // bump
   return arena_ptr + off                     // base of new State<T>
```

`__arena_overflow` is defined on all 19 backends as a trap
instruction (`ud2` on x86_64, `brk #0` on aarch64, `unimp` on
riscv64, etc.). On hosted x86_64 it surfaces as a non-zero exit code;
on bare metal it halts the CPU. The IVE `arena_bounds` verifier
([`vuma-ive/src/arena_bounds.rs`](../src/ive/src/arena_bounds.rs))
checks the static side — no arena variable is read or written after
`arena_free` consumes it (a linearity check, mirroring the
`StateTransform` consume mechanism).

### 4.3 `arena_grow` and `arena_free`

`arena_grow(a, min_capacity)` calls `mremap` (Linux) or the
moral equivalent on other systems to expand the region, preserving
contents. The arena's `capacity` field is updated to the new size and
the bumped offset is unchanged. `arena_free(a)` calls `munmap(a.base,
a.capacity)` and marks the arena consumed. There is **no per-object
free**; the only deallocation site in the entire language is
`arena_free` on a whole arena.

### 4.4 The Arena PMT surface

The library-level PMT surface mirrors the runtime:

```vuma
layout Arena = { offset: u32, capacity: u32, data: [u8; 256] }

// Caller allocates the state; init function populates it (init-style API).
let a = state_new(Arena);
arena_init(a, 256);
let off = arena_alloc(a, 64);    // returns a u32 logical offset, NOT a pointer
a.data[off] = 0x41;              // access bytes via typed array indexing
```

In the PMT surface `arena_alloc` returns a **logical byte offset** (a
`u32` handle into `arena.data`), not a raw address — callers access
bytes via typed array indexing (`arena.data[off]`), never via pointer
arithmetic. The runtime form (`mmap`/`mremap`/`munmap`) is what the
kernel uses for physically growable memory; the PMT form is what user
programs use when they want a growable byte buffer without leaving
the typed-state discipline.

---

## 5. Verification — The Three State Verifiers

VUMA 2.0 runs **only** `VerificationLevel::Pmt`. The five pointer
invariants of VUMA 1.x are replaced by three state verifiers in
[`vuma-ive/src/`](../src/ive/src/):

| Verifier                  | Source                       | Proves                                                                            |
|---------------------------|------------------------------|-----------------------------------------------------------------------------------|
| `StateReadVerifier`       | `state_read.rs`              | Field exists; `offset + size ≤ layout.total_size`; read type matches field type.  |
| `StateWriteVerifier`      | `state_write.rs`             | Field exists; `offset + size ≤ layout.total_size`; write type matches; linearity. |
| `StateTransformVerifier`  | `state_transform.rs`         | Both layouts registered; same size → reinterpret (zero-cost); diff size → copy.   |

### 5.1 Collapsed-invariant table

The 5 pointer invariants map onto the 3 state verifiers (and the two
*syntax-discharged* rows) as follows. Liveness and cleanup are
discharged by syntax (no `allocate`/`free` → no leak, no double-free,
no use-after-free). Origin is discharged by the `LayoutRegistry`
(every state carries a `LayoutId`).

| VUMA 1.x invariant  | Discharged by                              | Notes                                                                |
|---------------------|--------------------------------------------|----------------------------------------------------------------------|
| Liveness            | Syntax (no `allocate`/`free`)              | No resource is ever explicitly freed, so it cannot leak.             |
| Exclusivity         | `StateReadVerifier` + `StateWriteVerifier` | Field-offset + type-match check; no aliasing (no pointers).          |
| Interpretation      | `StateReadVerifier` + `StateWriteVerifier` | Read/write type must equal the field's declared `RepD`.              |
| Origin              | `LayoutRegistry` (`LayoutId`)              | Every state value is produced by `state_new(L)` or `arena_alloc`.    |
| Cleanup             | Syntax (no `free`)                         | Single-buffer lowering reclaims slots with the frame.                |

### 5.2 `StateReadVerifier`

For each `StateRead(var, field, expected_type)` the verifier looks up
the state's layout in `state_var_layouts`, finds the field by name in
`layouts`, and checks (1) the field exists, (2)
`field.offset + field.size ≤ layout.total_size`, and (3) `expected_type`
matches the field's declared type. A missing field (e.g. `s.z` on a
`Point` layout) produces `field 'z' not found in layout 'Point'` —
the `pmt_wave3_negative/bad_offset.vuma` test exercises this path.

### 5.3 `StateWriteVerifier`

Performs the same three bounds/type checks as `StateReadVerifier`, plus
a fourth linearity check: if `var` is in `consumed_vars` (or the write
is marked `after_consume`), the verifier reports
`linearity violation: state '<var>' written after being consumed by a
transform`. The `write_after_consume.vuma` negative test exercises
this. The same path catches `ForeignConsume`-marked states (see §9.4).

### 5.4 `StateTransformVerifier`

For each `StateTransform(in_layout, out_layout)` the verifier looks up
both layouts and classifies the transform:

- **Identity** — `in_layout == out_layout`. The buffer is returned
  unchanged.
- **Reinterpret** — `in.total_size == out.total_size`. The buffer is
  reinterpreted in place at zero cost.
- **Copy** — different sizes. The compiler emits a fresh `Alloc` of
  `out.total_size` and a `Store`-by-`Store` copy of the source buffer.

Same-size reinterprets do not undergo field-overlap analysis: in the
PMT model the buffer is just bytes, and field overlaps at the same
size are type-safe because no pointer ever observes the overlap
directly.

### 5.5 Dependent-array proof obligation

When a layout has an array field with a runtime count (`Stack<N>`,
`List<N>`), every dependent-array access discharges the
linear-arithmetic proof:

```
   offset + (count × elem_size) ≤ buffer_size
```

This is decidable (Presburger arithmetic) because the only
multiplication is `count × elem_size` where `elem_size` is a
compile-time constant. The runtime count is a `u32`/`u64` field whose
value the verifier tracks symbolically. The
`pmt_wave9/safe_access.vuma` test writes four elements of a `[u32; 8]`
array with indices `0,1,2,3`: the proof reduces to
`0 + 4*4 = 16 ≤ 32` ✓ — no runtime reasoning required. The
`pmt_wave9/stack.vuma` test models a stack with a runtime `count`
field; every `s.data[s.count]` access produces the obligation
`0 + count*4 ≤ 32`, i.e. `count ≤ 8`, which the verifier proves by
observing that `count` is only ever incremented after a write and is
bounded by the `data` field's static element count.

### 5.6 Verification invocation

`compile_dump` (the CLI driver in
[`src/bin/compile_dump.rs`](../src/bin/compile_dump.rs)) calls IVE
with `VerificationLevel::Pmt` unconditionally. With `--verify`, an IVE
failure aborts compilation before emission; without `--verify`, the
IVE result is reported but compilation proceeds. The output format is
`IVE: Pass passed=N failed=N total=N`. There is no `--pmt` flag and
no `Normal`/`Exhaustive`/`Hardened` level selection — `Pmt` is the
only level the driver ever constructs.

---

## 6. Behavioral Descriptors (BD)

A BD fully characterises a value along three orthogonal axes. The
top-level `BD` struct in
[`vuma-bd/src/descriptor.rs`](../src/bd/src/descriptor.rs) is the
triple `(RepD, CapD, RelD)`; two BDs are compatible when all three
layers are pairwise compatible, and one refines another when every
layer is at least as specific.

```
BD = RepD × CapD × RelD
```

### 6.1 `RepD` — Representation

`RepD` ([`vuma-bd/src/repd.rs`](../src/bd/src/repd.rs)) captures
memory shape: `Byte`, `Struct`, `Array`, `Enum`, `Ptr`, `Union`,
`Func`, plus the PMT-specific `State(layout_id)` variant. The
`LayoutRegistry` is the canonical source of state sizes and field
offsets — `state_size`, `field_offset`, `field_size`, and `field_repd`
form the query API used by both the codegen bridge (to emit
`Load`/`Store` offsets) and the three state verifiers (to discharge
their bounds obligations):

```rust
let mut registry = LayoutRegistry::new();
let id = registry.register("Point", vec![
    ("x".to_string(), RepD::Byte(ByteRep { size: 4, align: 4 })),
    ("y".to_string(), RepD::Byte(ByteRep { size: 4, align: 4 })),
]);
assert_eq!(registry.state_size(id), 8);          // total size
assert_eq!(registry.field_offset(id, "y"), Some(4));  // y's byte offset
```

### 6.2 `RelD` — Relational

`RelD` ([`vuma-bd/src/reld.rs`](../src/bd/src/reld.rs)) captures the
relationships a value participates in: `Temporal`, `Containment`,
`Dependency`, `Equivalence`, `Security`, `Liveness`. For PMT the
dominant relation is `Temporal` with `EpochBefore`/`EpochAfter`
flavours — every state has a producer epoch (the `state_new` or
`transform` that created it) and a consumer epoch (the read/write/
transform that observes it). `is_state_available(producer_epoch,
consumer_epoch)` returns `true` iff `producer_epoch ≤ consumer_epoch`,
proving that a read of a state cannot observe a value that has not yet
been produced. This is the type-theoretic counterpart of the VUMA 1.x
liveness invariant, lifted into the BD layer.

### 6.3 `CapD` — Capability

`CapD` ([`vuma-bd/src/capd.rs`](../src/bd/src/capd.rs)) is the lattice
of permitted operations, ordered by set-inclusion (`⊥ = ∅`, `⊤ = all`,
meet = `∩`, join = `∪`). For PMT, four capabilities extend the
classical `Read`/`Write`/…/`Drop`/`Move`/`Pin` set:

- `StateRead` — read a field via typed offset access.
- `StateWrite` — write a field via typed offset access.
- `StateTransform` — convert a state from one layout to another
  (consumes the input).
- `StateConsume` — mark a state as consumed (linear ownership transfer).

A `State<T>` value's `CapD` is `{StateRead, StateWrite}` by default; a
`transform`'s input parameter is strengthened with `StateConsume` so
the verifier proves the input is not used after the transform returns.
The same `StateConsume` capability is attached at FFI close-call sites
(`#[foreign_consume]` — see §9).

---

## 7. E-graph Layout Optimization

The optimizer's e-graph
([`vuma-codegen/src/egraph.rs`](../src/codegen/src/egraph.rs)) models
PMT state operations as four `ENode` variants alongside the existing
`Lit`, `VReg`, and `BinOp`:

```rust
enum ENode {
    Lit(i64),
    VReg(u32),
    BinOp(BinOpKind, u32, u32),
    StateInit      { layout_id: u64 },
    StateRead      { state: u32, offset: u64, size: u64 },
    StateWrite     { state: u32, offset: u64, value: u32 },
    StateTransform { input: u32, src_layout: u64, dst_layout: u64 },
}
```

PMT states are **linear/immutable** in the e-graph model: a
`StateWrite` produces a fresh state e-class, so the only way a
`StateRead`'s state child can be in the same e-class as a
`StateWrite`'s *output* is if the read directly consumes the write's
result. This linearity is what makes the rewrite rules sound.

### 7.1 The three PMT rewrite rules

| Rule                       | Pattern                                             | Replacement        | Soundness                                      |
|---------------------------|-----------------------------------------------------|--------------------|------------------------------------------------|
| `state_dead_init_elim`    | `StateInit(L)` whose e-class has zero consumers     | `Lit(0)`           | Unreferenced state's value is unobservable.    |
| `state_store_load_forward`| `StateRead(StateWrite(s, off, v), off, _)`          | `v`                | Linear states: no intervening write to `off`.  |
| `state_transform_elision` | `StateTransform(x, L, L)`                           | `x`                | Same-layout transform is the identity.         |

All three rules are in the `standard_rules()` set fed to
`equality_saturation_with_cost` by `run_optimizations_inner`. Two of
the three (`state_store_load_forward`, `state_transform_elision`)
carry `verified: true` and are encoded as bitvector identities in the
Wave 36 `bv_verify` gate (`bv_verify.rs`); `state_dead_init_elim` is
admitted as sound-by-construction (the guard ensures the state is
never observed — `bv_verify` cannot encode the "unreferenced"
guard).

A fourth rule, `state_merge_compatible_layouts` (merge two
non-overlapping `StateInit`s into one), is **registered but currently
a no-op**: it requires lifetime analysis the e-graph cannot express.
The stub is kept in the rule set so its name appears for
documentation and a future wave can implement it in a separate
post-equality-saturation pass.

### 7.2 Cost model

`default_cost` (`egraph.rs`) is:

| ENode                | Cost | Meaning                                 |
|----------------------|------|-----------------------------------------|
| `Lit(_)`             |    1 | literal — cheapest                      |
| `VReg(_)`            |   10 | virtual register                        |
| `BinOp(Add/Sub, ..)` |  100 | ALU op                                  |
| `BinOp(And/Or/Xor)`  |   90 | bitwise                                |
| `BinOp(Shl/Shr)`     |   95 | shift                                   |
| `BinOp(Mul)`         |  200 | multiply (more expensive than ALU)      |
| `BinOp(Div/Rem)`     | 1000 | divide (very expensive)                 |
| `StateInit`          |  500 | allocation                              |
| `StateRead`          |  150 | memory load                             |
| `StateWrite`         |  160 | memory store                            |
| `StateTransform`     |  300 | layout conversion                       |

The extractor (`EGraph::extract`) picks the lowest-cost representative
of each e-class:

- A dead `StateInit` (500) → `Lit(0)` (1); the `Alloc` is dropped by
  dead-vreg elimination.
- A `StateRead(StateWrite(s, off, v), off, _)` (300) → `v` (10); the
  `Load`/`Store` pair collapses to a register move.
- A `StateTransform(x, L, L)` (300) → `x` (10); the transform call
  disappears entirely.

Per-ISA cost overrides come from
`target_cost_fn(latency_table)` — a multiply-heavy loop on `m68k`
(where `mulu` is 38 cycles) is scheduled differently from the same
loop on `x86_64` (where `imul` is 3 cycles).

The `tests/gold_standard/pmt_wave5/` directory exercises each rule:
`dead_state.vuma`, `store_load_forward.vuma`, `transform_identity.vuma`,
`chain_writes.vuma`, `redundant_init.vuma`.

---

## 8. Backends

VUMA 2.0 ships 19 production backends. All 19 are in the gold-standard
test matrix and pass at ≥99.5% on the PMT-migrated suite. The
`BackendKind` enum
([`vuma-codegen/src/backend.rs`](../src/codegen/src/backend.rs)) is
the single source of truth for the ISA list:

| `BackendKind`     | `isa_name`   | Tier         | Notes                                          |
|-------------------|--------------|--------------|------------------------------------------------|
| `AArch64`         | `aarch64`    | Complete     | ARMv8.0 integer + atomics, full ABI. QEMU-run. |
| `AArch64Be`       | `aarch64_be` | Complete     | Big-endian AArch64. Compile-only.              |
| `X86_64`          | `x86_64`     | Complete     | System V ABI, REX prefix, full integer ISA. Native host. |
| `X86_32`          | `x86_32`     | Complete     | i386 System V ABI. Compile-only.               |
| `RiscV64`         | `riscv64`    | Complete     | RV64I + A extension. QEMU-run.                 |
| `RiscV32`         | `riscv32`    | Complete     | RV32I + A extension. Compile-only.             |
| `LoongArch64`     | `loongarch64`| Complete     | LA64 integer ISA. QEMU-run.                    |
| `Arm32`           | `arm32`      | Complete     | ARMv7-A EABI. QEMU-run.                        |
| `ArmEb`           | `armeb`      | Complete     | Big-endian ARMv7. Compile-only.                |
| `Mips64`          | `mips64`     | Complete     | MIPS64 R6 O64 ABI. Compile-only.               |
| `Mips64Be`        | `mips64be`   | Complete     | Big-endian MIPS64. Compile-only.               |
| `PowerPC64`       | `ppc64`      | Complete     | ELFv1 big-endian. Compile-only.                |
| `PowerPC64LE`     | `ppc64le`    | Complete     | ELFv2 little-endian. QEMU-run.                 |
| `Sparc64`         | `sparc64`    | Complete     | SPARC V9. Compile-only.                        |
| `S390X`           | `s390x`      | Complete     | IBM System Z ELF. QEMU-run.                    |
| `M68k`            | `m68k`       | Complete     | Motorola 68000. Compile-only.                  |
| `Alpha`           | `alpha`      | Complete     | DEC Alpha 21064. Compile-only.                 |
| `Hppa`            | `hppa`       | Complete     | HP PA-RISC 1.1. Compile-only.                  |
| `Wasm32`          | `wasm32`     | Complete     | WebAssembly 1.0 + WASI. Run under `wasmtime`.  |

**7 backends are executable** via QEMU user-mode (or natively on
x86_64, or under `wasmtime` for wasm32): `x86_64`, `aarch64`,
`riscv64`, `arm32`, `ppc64le`, `loongarch64`, `s390x`. The remaining
12 are compile-only — they emit valid ELF machine code and pass IVE
verification, but a QEMU user-mode binary for that architecture is not
in the standard sweep. The `kernel_parity.sh` sweep compile-verifies
all 19; it executes the 7 in the executable set.

### 8.1 Per-ISA optimisation

`run_ir_pipeline` queries the real backend's `latency_table()` via
`backend.target_info().latency_table()` and feeds it to
`run_optimizations_with_target_and_inline_threshold`. The e-graph cost
function (`target_cost_fn`) and the instruction scheduler
(`schedule_function`) therefore make decisions based on the actual
target's instruction latencies. The same IR program is re-optimised
per backend — a multiply-heavy loop on `m68k` (where `mulu` is 38
cycles) is scheduled differently from the same loop on `x86_64`
(where `imul` is 3 cycles).

### 8.2 Syscall ABI translation

Each backend implements `syscall_abi.rs` lowering for the IR
`IRInstr::Syscall { nr, args, dst }` instruction. The syscall number
is the VUMA-generic (Linux `asm-generic/unistd.h`) number; the
compiler translates to the native per-arch ABI automatically:

| Native ABI                  | Translation                                                       |
|-----------------------------|-------------------------------------------------------------------|
| Identity (no translation)   | aarch64, riscv64, riscv32, loongarch64, arm32                    |
| Translated                  | x86_64, x86_32, mips64, ppc64, s390x, sparc64, alpha, hppa, m68k |

The syscall number is validated against the 0..=600 range (a hard
error if exceeded) before codegen. Atomic operations (`AtomicLoad`,
`AtomicStore`, `AtomicCas`) are scheduler barriers and never reordered
relative to other memory ops. `atomic_cas` is hardcoded to U64 (8
bytes); see the `_pad0` workaround in
[`kernel-developer-guide.md` §8.5](./kernel-developer-guide.md#85-atomic-cas-loop).

---

## 9. FFI — The 4-Mode Matrix

VUMA 2.0's FFI is built on a **4-mode matrix** (replacing the legacy
binary `#[pure]`/invalidate model). Every foreign call is classified
into an **argument mode** per argument, a **return mode**, and
optionally a **callback mode**. The marshal module
([`vuma-codegen/src/marshal.rs`](../src/codegen/src/marshal.rs))
provides the classification helpers; the codegen bridge
(`scg_to_ir.rs`, Wave 5) consumes their output.

### 9.1 Argument modes

| Mode            | Attribute                  | What C gets                              | After-call state                       |
|-----------------|----------------------------|------------------------------------------|----------------------------------------|
| **Borrow**      | `#[borrow]`                | `___pmt_buffer_base + offset` (zero-copy)| **preserved** — reads/writes still valid |
| **Invalidate**  | *(default, no attr)*       | `___pmt_buffer_base + offset`            | **invalidated** — must re-init before next read/write |
| **Marshal**     | `#[marshal]`               | scratchpad pointer (copy made)           | state untouched                        |
| **MayRetain**   | `#[may_retain]`            | scratchpad pointer (copy made)           | state untouched (C may stash the ptr)  |
| **ForeignPass** | `#[foreign(raw)]` on layout| the `raw` field value (the C pointer)    | preserved (unless `#[foreign_consume]`)|

Precedence: `#[may_retain]` > `#[marshal]` > `#[borrow]` >
`#[foreign]` > default (Invalidate). Only one mode applies; the first
matching attribute wins. If the layout is `#[foreign(raw)]` and no
param attr overrides, the mode is `ForeignPass`.

### 9.2 Return modes

| Mode             | Attribute                   | What VUMA gets                                              |
|------------------|-----------------------------|-------------------------------------------------------------|
| **Scalar**       | *(default)*                 | a plain scalar (`i64`/`u64`/`Address`)                      |
| **Unmarshal**    | `#[unmarshal(Layout)]`      | a fresh `State<Layout>` (copied into `___pmt_buffer`)       |
| **ForeignWrap**  | `#[foreign_return(raw)]`    | a `State<ForeignLayout>` whose `raw` field holds the C ptr  |

Unmarshalling is **always a copy** back into `___pmt_buffer`, never a
borrow — the StateRead/StateWrite/StateTransform verifiers never see
scratchpad or C-owned memory.

### 9.3 Callback mode

| Mode           | Attribute     | Contract                                                                  |
|----------------|---------------|---------------------------------------------------------------------------|
| **Callback**   | `#[callback]` | C may invoke VUMA functions via a `vuma_context_t*` during the call       |

The callback runtime
([`vuma-codegen/src/runtime/callback.rs`](../src/codegen/src/runtime/callback.rs))
enforces the re-entrancy rule: callbacks run on an isolated callback
stack with their own scratchpad frame, forbidden from touching
caller-live State. A `LiveSet` tracks caller-live byte ranges;
`check_access` returns `false` (trap) for any access to a caller-live
region. Nested callbacks are supported via `CALLBACK_DEPTH` tracking.

### 9.4 The 8 FFI attributes

The parser (`src/parser/src/parser.rs::is_ffi_attr`) recognises
exactly 8 FFI attribute names. Their allowed placement is enforced by
`validate_extern_fn_ffi_attrs` and `validate_layout_ffi_attrs`;
misplacement is a fatal parse error.

```vuma
// On extern fn params (and as fn-level shorthand applying to all State args):
#[borrow]       // C reads, does not mutate, does not hold the pointer
#[marshal]      // force scratchpad routing (NUL-term, C-ownership)
#[may_retain]   // C may stash the pointer (epoll_ctl, sigaction)

// On the extern fn itself:
#[callback]              // C may call back into VUMA via vuma_context_t
#[foreign_consume(raw)]  // consumes the State arg linearly (e.g. sqlite3_close)
#[unmarshal(Response)]   // return mode: copy C return into State<Response>
#[foreign_return(raw)]   // return mode: wrap C pointer into State<ForeignLayout>

// On layout declarations:
#[foreign(raw)]  // the layout wraps an opaque C pointer (raw: u64 field)
```

The `#[foreign_consume]` attribute causes the codegen bridge
(`pipeline.rs::emit_foreign_consume_markers`) to emit an
`ScgStatement::ForeignConsume` at the call site for each State arg
whose layout is `#[foreign(raw)]`. The existing `state_write` verifier
treats this the same as a `StateTransform` consume — any subsequent
read/write to the consumed vreg is a linearity error. No new verifier
is needed.

### 9.5 Worked examples

**Borrowed read (zero-copy I/O):**
```vuma
layout IoBuf = { len: u64, data: [u8; 16] }

extern "C" {
    #[borrow]
    fn write(fd: i64, buf: Address, count: i64) -> i64;
}

fn emit(b: State<IoBuf>) -> State<IoBuf> {
    let n = write(1, b as Address, b.len as i64);
    // b is #[borrow] — preserved after the call. b.len is still valid.
    b.len = n as u64;
    return b;
}
```

**Foreign handle (open/close with linear safety):**
```vuma
#[foreign(raw)]
layout DbHandle = { raw: u64 }

extern "C" {
    #[foreign_return(raw)]
    fn sqlite3_open(path: Address) -> State<DbHandle>;

    #[foreign_consume(raw)]
    fn sqlite3_close(db: State<DbHandle>);
}

fn main() -> i32 {
    let db = sqlite3_open(0 as Address);   // wraps C pointer into State<DbHandle>
    sqlite3_close(db);                     // consumes db — post-close use is a linearity error
    return 0;
}
```

**Callback (sqlite3_exec):**
```vuma
extern "C" {
    #[callback]
    fn sqlite3_exec(db: State<DbHandle>, sql: Address,
                    cb: Address, arg: Address, err: Address) -> i32;
}

fn row_callback(ctx: Address, argc: i32, argv: Address, col: Address) -> i32 {
    // C invokes this for each row. The callback runs on an isolated stack
    // with its own scratchpad frame. It may NOT touch caller-live State
    // (enforced by the callback_live_set guard — trap on violation).
    return 0;
}
```

### 9.6 The marshal scratchpad

The scratchpad (`___ffi_scratch`) is a thread-local, `malloc`-backed
stack, separate from `___pmt_buffer`
([`vuma-codegen/src/runtime/ffi_scratch.rs`](../src/codegen/src/runtime/ffi_scratch.rs)).
It is used for:

- NUL-terminated C strings (`marshal_cstr` copies + appends `'\0'`)
- C-owned memory round-trips (`strdup`, `getline`)
- In-place mutation buffers for C APIs demanding specific layouts

**Sacred invariant:** scratchpad memory is **never** aliased by
`___pmt_buffer`. The `StateRead`/`StateWrite`/`StateTransform`
verifiers never see it. Unmarshalling is **always** a copy back into
`___pmt_buffer`, never a borrow. The scratchpad is stack-shaped:
`push_frame` on transform entry, `pop_frame` on transform exit.
Nested transforms get nested frames.

The codegen bridge (`scg_to_ir.rs::emit_scratchpad_hooks`) emits
`ffi_scratch_push_frame` at function entry and `ffi_scratch_pop_frame`
before every Return — but **only** for functions that contain extern
calls or marshal builtins (avoids unresolved symbols in non-FFI
programs).

### 9.7 The `vuma_context_t` C-API

[`vuma_vm.h`](../vuma_vm.h) at the workspace root plus the runtime in
[`vuma-codegen/src/runtime/vuma_context.rs`](../src/codegen/src/runtime/vuma_context.rs)
generalize the wasm32 host shim
(`scripts/wasm32_runner.py::make_host_functions`) into a C header
shipped for all backends. It exposes 8 accessors:
`vuma_read_u32`, `vuma_read_u64`, `vuma_write_u32`, `vuma_write_u64`,
`vuma_state_new`, `vuma_push_i32`, `vuma_push_i64`, and a
`vuma_context_t*` opaque handle. C uses these to call back into a
VUMA `#[callback]` extern fn safely.

### 9.8 FFI test coverage

| Wave | Directory    | Tests                                                                  |
|------|--------------|------------------------------------------------------------------------|
| 1    | `ffi_wave0/` | 8 positive + 1 negative (parser attrs, validation)                    |
| 2    | `ffi_wave2/` | 3 (borrow_preserved, invalidate, may_retain)                          |
| 3    | `ffi_wave1/` | 3 (write_real, open_file, strdup)                                      |
| 4    | `ffi_wave3/` | 4 (arg_pass, return_wrap, consume_emitted, use_after_close_reject)    |
| 5    | `ffi_wave4/` | 2 (sqlite_exec_callback, callback_traps)                              |

---

## 10. If-Expression Lowering

VUMA 2.0 supports both the **statement form** of `if`/`else` (no value)
and the **expression form** (`let x = if cond { a } else { b };`).
Both forms are first-class in the parser:

- `Stmt::If` (`parse_if_stmt`) — the statement form, producing no
  value. Lowered directly to `ControlNode::If { cond, then_body,
  else_body }`.
- `Expr::IfExpr` (`parse_if_expr`) — the expression form. The
  `else`-branch is **mandatory** (a missing else would make the value
  undefined when `cond` is false). `else if` chains desugar to a
  single-statement block wrapping the inner if-expression.

The AST node:

```rust
Expr::IfExpr {
    condition: Box<Expr>,
    then_block: Block,
    else_block: Block,
    span: Span,
}
```

### 10.1 Lowering through a result temp

The bridge (`src/pipeline.rs::flatten_expr`, the `Expr::IfExpr` arm)
lowers the expression form to the *same* `ControlNode::If` SCG node
the statement form uses, but threads the branch value through a
result temporary:

```rust
// 1. Evaluate the condition.
let cond_expr = flatten_expr(condition, stmts, ctx);
// 2. Allocate a temp for the result.
let result_tmp = ctx.alloc_temp();
// 3. Lower the then-block; its trailing expression becomes the value.
let mut then_body = bridge_block_to_scg_stmts(then_block, ctx);
then_body.push(/* result_tmp = then_val + 0 */);
// 4. Lower the else-block; its trailing expression becomes the value.
let mut else_body = bridge_block_to_scg_stmts(else_block, ctx);
else_body.push(/* result_tmp = else_val + 0 */);
// 5. Emit the ControlNode::If with both branches.
stmts.push(ScgStatement::Control(ControlNode::If {
    cond: cond_expr,
    then_body,
    else_body: Some(else_body),
}));
// 6. The IfExpr's value is result_tmp.
ScgExpr::Var(result_tmp)
```

The `then_block` and `else_block` are required to each have a trailing
expression (the last statement is treated as the value if it is a bare
expression statement). The trailing expression's value is captured
into `ctx.last_expr_result` by `bridge_block_to_scg_stmts`; if absent,
the branch defaults to `ScgExpr::Int(0)` (the unit value).

### 10.2 IR-level lowering

`scg_to_ir.rs::lower_if` lowers `ControlNode::If` to a CFG of three
IR blocks — `then`, `else`, `merge` — connected by `CondBranch`
edges, with a phi-style merge at the join point. The expression form's
`result_tmp` becomes a phi input from each branch; the statement form
lowers with no phi (the merge block has no value). Both forms share
the same `lower_if` codepath — only the presence or absence of a
result temp differs.

### 10.3 Example

```vuma
// A value-typed if-expression:
fn classify(score: u32) -> u32 {
    let grade = if score >= 90 {
        4    // A
    } else {
        if score >= 80 {
            3    // B
        } else {
            2    // C or below
        }
    };
    return grade;
}
```

The outer `if` produces `grade`; the inner `if` is the entire
`else`-block (an `else if` chain). Each branch's trailing integer
literal is the branch value; the bridge threads them through nested
result temps.

---

## 11. Nested Layout Resolution

A layout field can be another layout type (`layout Line = { a: Point, b: Point }`).
The field-chain resolver then needs to descend into the nested layout
to compute cumulative offsets: `l.a.x` resolves to
`0 [a in Line] + 0 [x in Point] = 0`, `l.b.y` to
`8 [b in Line] + 4 [y in Point] = 12`.

This is non-trivial because layout sizes are computed *from* field
types, and a field type may itself be a layout whose size has not yet
been resolved. The bridge solves this with a **multi-pass** layout
registry builder, `build_layout_registry`
([`src/pipeline.rs`](../src/pipeline.rs)):

### 11.1 Pass 1 — collect

Walk the AST, collecting every `Item::LayoutDef` into a
`Vec<(&str, &Vec<(String, Type)>)>`. This is the canonical list of
layouts in the program.

### 11.2 Pass 2 — iteratively resolve sizes

Repeat up to 10 iterations (or until no change):

```
for each (name, fields) in layout_defs:
    size = 0; max_align = 1
    for (_fname, ftype) in fields:
        falign = bridge_type_align(ftype)
        fsize  = bridge_type_size_with_layouts(ftype, &layout_sizes)
        size = align_up(size, falign) + fsize
        max_align = max(max_align, falign)
    size = align_up(size, max_align)
    if layout_sizes[name] != size:
        layout_sizes[name] = size
        changed = true
```

`bridge_type_size_with_layouts` looks up user-defined layout names in
the running `layout_sizes` map — if a size is not yet known (because
the layout references one declared later in the file), it returns 0
and the iteration continues. The fixed-point is reached when no
`layout_sizes[name]` changes during a full pass. The 10-iteration cap
is a safety bound; in practice the fixed-point is reached in ≤3
iterations for any non-cyclic layout set (cycles would be a parse
error — a layout cannot contain itself by value).

### 11.3 Pass 3 — assign offsets

With sizes resolved, walk the layouts again and assign each field a
cumulative offset, producing the final
`HashMap<String, (u64 /* total_size */, Vec<(String, IRType, u64 offset,
u64 size, String type_name)>)>` that the bridge and the IVE
verifiers query.

### 11.4 Field-chain descent

The field-chain resolver (`resolve_field_chain` in
[`src/pipeline.rs`](../src/pipeline.rs) and the parallel
`field_repd` walker in
[`vuma-bd/src/repd.rs`](../src/bd/src/repd.rs)) descends into
nested layout-typed fields. Given `l.a.x`:

1. Look up `Line` in the registry; find `a` at offset 0, type `Point`.
2. The field's type is a layout name — recurse into `Point`.
3. Look up `Point` in the registry; find `x` at offset 0, type `u32`.
4. Cumulative offset: `0 [a in Line] + 0 [x in Point] = 0`.

For `l.b.y`:
1. `Line.b` at offset 8, type `Point`.
2. `Point.y` at offset 4.
3. Cumulative: `8 + 4 = 12`.

The IVE `StateReadVerifier` and `StateWriteVerifier` use the same
descent to verify nested access — the bounds check
`field.offset + field.size ≤ layout.total_size` is performed against
the *leaf* field's offset within the *root* layout.

---

## 12. VUMA Parser Limitations

The VUMA 2.0 parser (`src/parser/src/parser.rs`) and the codegen
bridge (`src/pipeline.rs::flatten_expr`) have several known
limitations that shape kernel and library code style. Each is
documented in the file header of the kernel module that works around
it; this section consolidates them. See also
[`kernel-architecture.md` §10](./kernel-architecture.md#10-vuma-parser-limitations)
for the kernel-specific workarounds.

### 12.1 No `import` (Open Work §7)

VUMA 2.0 has no `import` statement. Every module that wants to call
another module's functions or use another module's layouts must
**re-declare them locally**, byte-identically. The
`byte-identical-redeclaration invariant` is enforced by the
`LayoutRegistry` — if two files declare the same layout name with
different field offsets/types/order, the verifiers catch the drift at
compile time.

```vuma
// womb/kernel/syscall/dispatch.vuma re-declares:
layout SyscallArgs = { nr: u64, a0: u64, a1: u64, a2: u64,
                       a3: u64, a4: u64, a5: u64 }
// byte-identical to womb/kernel/syscall/abi.vuma's SyscallArgs.
```

A future wave will add a real `import` mechanism (likely
`import fs.inode;` bringing `InodeTable` + its helpers into scope);
until then, every kernel contributor does the copy-paste.

### 12.2 No string-literal lowering

The lexer recognises string literals (`"hello"`) and produces
`Lit::String(String)`, but the codegen bridge does not lower them to
data — there is no `.rodata` emission, no string-interning table, and
no `state_new(String)` lowering. Programs that need string data write
the bytes into a `State<Buffer>` field-by-field:

```vuma
layout Buffer = { data: [u8; 16] }

fn set_hello(b: State<Buffer>) {
    b.data[0] = 104;  // 'h'
    b.data[1] = 101;  // 'e'
    b.data[2] = 108;  // 'l'
    b.data[3] = 108;  // 'l'
    b.data[4] = 111;  // 'o'
}
```

The `marshal_cstr` builtin (§9.6) copies a buffer to the scratchpad
and appends `'\0'`, producing a NUL-terminated C string for FFI;
this is the only supported stringification path.

### 12.3 `State`-typedness doesn't propagate through function returns

The codegen does not propagate `State<T>`-typedness through function
return values. A binding `let s = make_state()` where `make_state`
returns `State<T>` is **not** registered as state-typed in the caller;
subsequent `s.field` accesses silently return 0 with a
`WARNING: unsupported FieldAccess (not state-typed)` from
`flatten_expr`.

The workaround is the **init-style API**: the caller allocates the
state via `state_new(...)` (which marks the binding as state-typed)
and passes it by reference to a function that populates it in place.

```vuma
// DON'T (return-style — s not registered as state-typed):
fn make_point(x: u32, y: u32) -> State<Point> {
    let p = state_new(Point);
    p.x = x; p.y = y;
    return p;
}
fn use_point() -> u32 {
    let s = make_point(10, 20);
    return s.x;       // WARNING, returns 0
}

// DO (init-style — caller-allocated, callee populates):
fn init_point(p: State<Point>, x: u32, y: u32) {
    p.x = x;
    p.y = y;
}
fn use_point() -> u32 {
    let s = state_new(Point);
    init_point(s, 10, 20);
    return s.x;       // 10
}
```

This pattern is used by `pmm_init`, `vmm_init`, `trap_frame_init`,
`task_init_for_switch`, `syscall_args_from_frame`, `kmsg_init`,
`pm_init`, and every other stateful kernel subsystem. See
[`kernel-architecture.md` §3](./kernel-architecture.md#3-pmt-in-the-kernel-design).

### 12.4 Array index is byte-granular for non-`u8` element types

The codegen only lowers `state.array[idx]` correctly for `[u8; N]`
arrays. For `[u16; N]`, `[u32; N]`, or `[u64; N]` arrays, the
indexed-access path goes through a code path the IVE verifiers don't
fully understand — accesses compile but read back wrong values. The
kernel's convention is to **store every "array of N u32/u64" as a
parallel flat `[u8; N * width]` array** with pack/unpack helpers:

```vuma
layout InodeTable = {
    ino:  [u8; 1024],   // 128 inodes × 8 bytes, packed little-endian
    mode: [u8; 128],    // 128 inodes × 1 byte
    size: [u8; 1024],   // 128 inodes × 8 bytes, packed little-endian
}

fn inode_get_ino(tbl: State<InodeTable>, idx: u32) -> u64 {
    let off = idx * 8;
    let v: u64 = 0;
    let i = 0;
    while i < 8 {
        let sh = i * 8;
        let b = tbl.ino[off + i] as u64;
        v = v + (b << sh);
        i = i + 1;
    }
    return v;
}
```

The pack helper is an 8-iteration while loop (shift by `i*8`, mask
255 on writes, sum-of-shifted-bytes on reads). The same pattern
appears in `pmm.vuma::pool_get_base`, `irq.vuma::irq_get_handler`,
`syscall/table.vuma::syscall_table_get`, `task.vuma::pt_get_vruntime`,
and every other kernel table module.

### 12.5 `transform` is limited to a single `State<T>` parameter

The `StateTransform` verifier requires that an `as Address` cast
appears in a context where the `State<T>` is the function's parameter
— i.e. the transform is on a state owned by the caller. Casting a
state created inside the function
(`let s = state_new(...); let a = s as Address; ...`) sometimes trips
the verifier because the lifetime of `s` is local. The kernel's
convention is to **always pass states from the caller**: even a helper
that just needs a scratch buffer takes it as a parameter (the caller
allocates and owns it).

```vuma
// DON'T (transform-on-local-state — sometimes trips verifier):
fn console_flush_local() {
    let c = state_new(Console);
    let base = c as Address;       // transform on local State — risky
    let _n = write(1, base, c.len as i64);
}

// DO (transform-on-parameter — always safe):
fn console_flush(c: State<Console>) {
    let base = c as Address;       // transform on parameter — OK
    let _n = write(1, base, c.len as i64);
    c.len = 0;
}
```

This is why every FFI-trampoline function in the kernel takes the
state by parameter rather than allocating it locally.

### 12.6 The `0 - N` negative-literal workaround

VUMA's integer-literal path goes through `parse_int_radix`, which on a
negative literal like `-1` interprets it as a signed `i64` and then
sign-extends to `u64` — producing `0xFFFFFFFFFFFFFFFF` correctly in
most cases but occasionally tripping a width-extension subtlety that
produces surprising values. The kernel's convention is to **always
write negative numbers as `0 - N`** (e.g. `0 - 1` for -1, `0 - 11`
for -EAGAIN, `0 - 38` for -ENOSYS) rather than as the literal `-1` /
`-11` / `-38`:

```vuma
// DON'T:
return -1;          // parser's signed-literal path — risky

// DO:
return 0 - 1;       // flatten_expr's BinOp::Sub arm — verified safe
```

The `0 - N` form lowers to the same machine code as `-N` (the
codegen's constant folder collapses it), so there is no perf cost.
The safety gain is that the expression goes through `flatten_expr`'s
`BinOp::Sub` arm (which handles type promotions correctly) rather
than the lexer's negative-number path.

### 12.7 The hex-literal width-extension subtlety

VUMA accepts `0x..` hex literals, but the parser's hex path shares
code with the decimal path through `parse_int_radix`, which has
subtle width-extension behavior at the 64-bit boundary. The kernel's
convention is to **use decimal literals in self-tests** (`4096`, not
`0x1000`; `17592186028032`, not `0x000FFFFFFFFFF000`). The decimal
form lowers to identical machine code; the safety gain is avoiding
the hex path entirely.

### 12.8 The `no_struct_literal` trap

VUMA has no struct-literal syntax (`Layout { field: value, ... }`).
State must be allocated with `state_new(Layout)` (zero-initialized)
and then populated field-by-field. There is no way to "construct" a
state inline:

```vuma
// DON'T (parse error — VUMA has no struct literal):
fn make_task(pid: u32) -> State<Task> {
    return Task { pid: pid, state: 1 };
}

// DO (allocate-then-populate):
fn make_task(tbl: State<ProcessTable>, pid: u32) {
    let idx = task_alloc(tbl);
    pt_set_pid(tbl, idx, pid);
    pt_set_state(tbl, idx, 1);
}
```

This forces the init-style API pattern (see §12.3); it's not really a
workaround so much as a language-design choice that aligns with the
PMT discipline (no implicit allocation sites).

### 12.9 Forward references

Unlike C, VUMA allows forward references to functions: a function can
call another function declared later in the file. The parser does a
two-pass scan (pass 1 collects all `fn` signatures into the symbol
table, pass 2 resolves call sites). Layouts, however, **must** be
declared before the first function that uses them — the layout
registry is single-pass within a function (the multi-pass size
resolver only fixes cross-layout nesting, not forward references to
layouts declared later in the file).

---

## Appendix — Pointer syntax is a hard error

The parser
([`vuma-parser/src/parser.rs`](../src/parser/src/parser.rs)) rejects
every VUMA 1.x pointer construct with a fatal `ParseError`:

```
pointer syntax 'allocate' is not supported in VUMA 2.0 (PMT-only);
use state_new(Layout) and transforms
```

The detection sites and their kind labels:

| Source construct                          | Kind label                       |
|-------------------------------------------|----------------------------------|
| `allocate(N)` / `region r = allocate(N)`  | `"allocate"` / `"allocate (region)"` |
| `free(ptr)`                               | `"free"`                         |
| `*ptr` (dereference)                      | `"*ptr (deref)"`                 |
| `&x` (address-of)                         | `"&x (address-of)"`              |
| `@x` (address-of, alt)                    | `"@x (address-of)"`              |
| `*T` (pointer type)                       | `"*T (pointer type)"`            |

`parse_program` scans the accumulated error list for messages starting
with `pointer syntax '` and downgrades the parse result to a fatal
`ParseResult::err` so the failure propagates to the CLI driver. There
is no `--pmt` flag, no `--pmt-only` flag, no warning path, and no
`pmt_only: bool` field on the `Parser` struct — PMT is the only mode.

---

## Cross-references

- [`language-reference.md`](./language-reference.md) — the VUMA 2.0
  language reference: syntax, types, layouts, states, transforms,
  arena primitives, FFI attributes, complete examples.
- [`kernel-architecture.md`](./kernel-architecture.md) — the VWK
  kernel architecture: the four-layer cake, boot flow, PMT-in-the-kernel
  design, arena memory model, per-arch abstraction, FFI trampoline
  patterns, complete 75-file inventory.
- [`kernel-developer-guide.md`](./kernel-developer-guide.md) — how to
  extend the kernel with new syscalls, drivers, filesystems, and PMT
  kernel code; do/don't examples; IVE failure debugging recipe.
- [`building.md`](./building.md) — complete build reference:
  prerequisites, Rust toolchain, QEMU installation, build profiles,
  constrained-memory workaround, troubleshooting.
- [`kernel-porting-guide.md`](./kernel-porting-guide.md) — step-by-step
  guide to porting the kernel to a new architecture.
- [`contributing.md`](./contributing.md) — general contribution
  workflow.
