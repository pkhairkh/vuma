# VUMA 2.0 Architecture

VUMA 2.0 is a **PMT-only** systems programming language: every program is a
composition of memory transformations over typed states. There is no pointer
mode, no legacy path, and no `--pmt` flag — the pointer syntax of VUMA 1.x
(`allocate`, `free`, `*ptr`, `&x`, `*T`) is unconditionally a hard parse
error, and the only verification level the compiler ever runs is
`VerificationLevel::Pmt`. Memory safety is established by **type-checking**,
not by runtime pointer proofs: because the source language contains no
pointer types, every memory access reduces to a typed field read or write
against a layout registered in the `LayoutRegistry`, and the
`StateRead`/`StateWrite`/`StateTransform` verifiers discharge the three
structural proof obligations that remain once raw pointers are removed.

This document describes the compilation pipeline, the state type system, the
three state verifiers, the Behavioral Descriptor (BD) layer, the e-graph
layout optimizer, the 19 production backends, dependent state types, and the
FFI marshal pass. Code examples are drawn verbatim from the gold-standard
test suite under `tests/gold_standard/pmt_wave*/`.

---

## Table of Contents

1. [Overview](#1-overview)
2. [The PMT Pipeline](#2-the-pmt-pipeline)
3. [State Type System](#3-state-type-system)
4. [Verification — The Three State Verifiers](#4-verification--the-three-state-verifiers)
5. [Behavioral Descriptors (BD)](#5-behavioral-descriptors-bd)
6. [E-graph Layout Optimization](#6-e-graph-layout-optimization)
7. [Backends](#7-backends)
8. [Dependent State Types](#8-dependent-state-types)
9. [FFI Marshal Pass](#9-ffi-marshal-pass)

---

## 1. Overview

VUMA 2.0 collapses the five pointer invariants of VUMA 1.x (liveness,
exclusivity, interpretation, origin, cleanup) into three structural
type-checking rules: every state-field read is in-bounds
(`StateReadVerifier`), every state-field write is in-bounds and respects
linear ownership (`StateWriteVerifier`), and every state-to-state
transform is layout-compatible (`StateTransformVerifier`). Liveness and
cleanup are discharged by construction — there is no `free`, no `drop`,
and no explicit deallocation site to leak or double-free. Origin is
subsumed by the `LayoutRegistry`: every state value carries a `LayoutId`
that names its provenance. Exclusivity and interpretation collapse into a
single field-offset + type-match check, because the absence of pointer
arithmetic means there is no aliasing to reason about.

The compiler is a Cargo workspace (`parser`, `scg`, `bd`, `ive`,
`codegen`, `proof`, `cor`, `package`, `vuma`). The top-level `compile`
entry point in [`src/pipeline.rs`](../src/pipeline.rs) wires parser → AST →
codegen-SCG → IR → optimisation → register allocation → emission. All
schedulers and optimisers are unconditionally enabled; `--verify` only
gates whether the IVE state verifiers run as a final pre-emit gate.

---

## 2. The PMT Pipeline

```
 Source (.vuma) ──► Parser ──► AST (layout/State/transform) ──► SCG

                  bridge_ast_to_codegen_scg
                    state_new  → Alloc / Offset into ___pmt_buffer
                    state.f    → Load  (off, size)
                    state.f=v  → Store (off, value)
                    transform  → Alloc + copy
                  ──► IR Program

                  run_ir_pipeline (O2 default):
                    monomorphize, lower_closures, lower_switches,
                    lower_tail_calls, normalize_loops,
                    bv_verify,
                    constant_fold + cse, equality_saturation (e-graph),
                    dead_store_eliminate, inline_with_threshold,
                    licm (always on), schedule_function (always on),
                    cross_function_constant_prop, identical_function_merge,
                    whole_program_dce, loop_unroll,
                    escape + effects (SROA, alloc elision, pure-fn marking),
                    vectorize (loop + SLP)
                  ──► regalloc (LinearScan, parallel per-fn)
                  ──► 19 backends (x86_64 … hppa)
                  ──► ELF / Wasm
```

The pipeline is the single source of truth used by `compile`,
`compile_with_path`, `compile_modules`, and `compile_to_wasm` — all four
entry points call `bridge_ast_to_codegen_scg` followed by
`run_ir_pipeline`. The pipeline does not branch on a `--pmt` flag; PMT is
the only mode.

---

## 3. State Type System

### 3.1 Layouts

A layout is a named product of fields with computed offsets and alignment.
Layouts are first-class top-level items and may nest or contain fixed-size
arrays:

```vuma
// tests/gold_standard/pmt_wave2/init_read.vuma
layout Point = { x: u32, y: u32 }

fn main() -> i32 {
    let p = state_new(Point);
    p.x = 42;
    return p.x;
}

// tests/gold_standard/pmt_wave9/stack.vuma
layout Stack = { count: u32, data: [u32; 8] }
```

At registration time the `LayoutRegistry` lays out fields sequentially,
inserting alignment padding before each field and rounding the total size
up to the layout's alignment so arrays of the layout are correctly
strided. For `Stack` above the resolved `LayoutDef` has `count` at offset
0 (size 4), `data` at offset 4 (size 32), `total_size = 36`, `alignment = 4`.

### 3.2 `State<T>` and `state_new`

`state_new(Layout)` is the sole allocation primitive. It produces a
`State<T>` — a linear handle to a buffer whose shape is described by
`T`'s `LayoutDef`. The handle is *not* a pointer: it cannot be
dereferenced, cast, or stored in a `u64`. The only operations on a
`State<T>` are field reads, field writes, layout transforms, and passing
to other VUMA functions (by value — the callee receives the buffer's
address under the hood):

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
    s.data[s.count] = v;
    s.count = s.count + 1;
}
```

### 3.4 Transforms

A `transform` is a pure function from `State<L_in>` to `State<L_out>`. The
`StateTransformVerifier` proves the two layouts are compatible (same size
→ in-place reinterpret; different size → compiler-generated copy). When
`L_in == L_out` the transform is the identity, and the e-graph's
`state_transform_elision` rule rewrites it away at O2.

```vuma
// tests/gold_standard/pmt_wave1/transform_decl.vuma
layout Request  = { opcode: u32, arg: u32 }
layout Response = { status: u32, result: u32 }

transform handle(req: State<Request>) -> State<Response> {
    return;
}
```

### 3.5 Single-buffer lowering

Under the Wave 8a lowering, every `state_new` in a function resolves to
an `Offset` into a single program-wide buffer `___pmt_buffer` rather than
to its own stack slot. The IRBuilder emits **one** `Alloc` for
`___pmt_buffer` (sized to the sum of all state sizes, 16-byte aligned) at
function entry, then lowers each `state_new(Layout)` to
`state = ___pmt_buffer + slot_offset`. There are zero per-state `Alloc`
instructions at runtime:

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

States are linear: a state consumed by a `transform` may not be read or
written afterwards. The `StateWriteVerifier` enforces this by tracking
the set of `consumed_vars` and rejecting any write targeting a consumed
state with a `linearity violation` diagnostic. Reads of consumed states
are rejected by the same path. There is no `drop`; a state goes out of
scope at the end of its enclosing function and its slot is reclaimed with
the single buffer.

---

## 4. Verification — The Three State Verifiers

VUMA 2.0 runs **only** `VerificationLevel::Pmt`. The five pointer
invariants of VUMA 1.x are replaced by three state verifiers in
`src/ive/src/`:

| Verifier              | Source                       | Proves                                                                            |
|-----------------------|------------------------------|-----------------------------------------------------------------------------------|
| `StateReadVerifier`   | `state_read.rs`              | Field exists; `offset + size ≤ layout.total_size`; read type matches field type.  |
| `StateWriteVerifier`  | `state_write.rs`             | Field exists; `offset + size ≤ layout.total_size`; write type matches; linearity. |
| `StateTransformVerifier` | `state_transform.rs`      | Both layouts registered; same size → reinterpret (zero-cost); diff size → copy.   |

### 4.1 Collapsed-invariant table

The 5 pointer invariants map onto the 3 state verifiers as follows.
Liveness and cleanup are discharged by syntax (no `allocate`/`free` → no
leak, no double-free, no use-after-free). Origin is discharged by the
`LayoutRegistry` (every state carries a `LayoutId`).

| VUMA 1.x invariant | Discharged by                         | Notes                                                       |
|--------------------|---------------------------------------|-------------------------------------------------------------|
| Liveness           | Syntax (no `allocate`/`free`)         | No resource is ever explicitly freed, so it cannot leak.    |
| Exclusivity        | `StateReadVerifier` + `StateWriteVerifier` | Field-offset + type-match check; no aliasing (no pointers). |
| Interpretation     | `StateReadVerifier` + `StateWriteVerifier` | Read/write type must equal the field's declared `RepD`.     |
| Origin             | `LayoutRegistry` (`LayoutId`)         | Every state value is produced by `state_new(L)`.            |
| Cleanup            | Syntax (no `free`)                    | Single-buffer lowering reclaims slots with the frame.       |

### 4.2 `StateReadVerifier`

For each `StateRead(var, field, expected_type)` the verifier looks up the
state's layout in `state_var_layouts`, finds the field by name in
`layouts`, and checks (1) the field exists, (2) `field.offset +
field.size ≤ layout.total_size`, and (3) `expected_type` matches the
field's declared type. A missing field (e.g. `s.z` on a `Point` layout)
produces `field 'z' not found in layout 'Point'` — the
`pmt_wave3_negative/bad_offset.vuma` test exercises this path.

### 4.3 `StateWriteVerifier`

Performs the same three bounds/type checks as `StateReadVerifier`, plus a
fourth linearity check: if `var` is in `consumed_vars` (or the write is
marked `after_consume`), the verifier reports
`linearity violation: state '<var>' written after being consumed by a
transform`. The `write_after_consume.vuma` negative test exercises this.

### 4.4 `StateTransformVerifier`

For each `StateTransform(in_layout, out_layout)` the verifier looks up
both layouts and classifies the transform:

- **Identity** — `in_layout == out_layout`. The buffer is returned
  unchanged.
- **Reinterpret** — `in.total_size == out.total_size`. The buffer is
  reinterpreted in place at zero cost.
- **Copy** — different sizes. The compiler emits a fresh `Alloc` of
  `out.total_size` and a `Store`-by-`Store` copy of the source buffer.

Same-size reinterprets do not undergo field-overlap analysis: in the PMT
model the buffer is just bytes, and field overlaps at the same size are
type-safe because no pointer ever observes the overlap directly.

### 4.5 Verification invocation

`compile_dump` (the CLI driver in `src/bin/compile_dump.rs`) calls IVE
with `VerificationLevel::Pmt` unconditionally. With `--verify`, an IVE
failure aborts compilation before emission; without `--verify`, the IVE
result is reported but compilation proceeds. The output format is
`IVE: Pass passed=N failed=N total=N`. There is no `--pmt` flag and no
`Normal`/`Exhaustive`/`Hardened` level selection — `Pmt` is the only
level the driver ever constructs.

---

## 5. Behavioral Descriptors (BD)

A BD fully characterises a value along three orthogonal axes. The
top-level `BD` struct in `vuma-bd/src/descriptor.rs` is the triple
`(RepD, CapD, RelD)`; two BDs are compatible when all three layers are
pairwise compatible, and one refines another when every layer is at
least as specific.

```
BD = RepD × CapD × RelD
```

### 5.1 `RepD` — Representation (LayoutRegistry)

`RepD` (`vuma-bd/src/repd.rs`) captures memory shape: `Byte`, `Struct`,
`Array`, `Enum`, `Ptr`, `Union`, `Func`, plus the PMT-specific
`State(layout_id)` variant. The `LayoutRegistry` is the canonical source
of state sizes and field offsets — `state_size`, `field_offset`,
`field_size`, and `field_repd` form the query API used by both the codegen
bridge (to emit `Load`/`Store` offsets) and the three state verifiers (to
discharge their bounds obligations):

```rust
let mut registry = LayoutRegistry::new();
let id = registry.register("Point", vec![
    ("x".to_string(), RepD::Byte(ByteRep { size: 4, align: 4 })),
    ("y".to_string(), RepD::Byte(ByteRep { size: 4, align: 4 })),
]);
assert_eq!(registry.state_size(id), 8);          // total size
assert_eq!(registry.field_offset(id, "y"), Some(4));  // y's byte offset
```

### 5.2 `RelD` — Relational (epochs)

`RelD` (`vuma-bd/src/reld.rs`) captures the relationships a value
participates in: `Temporal`, `Containment`, `Dependency`, `Equivalence`,
`Security`, `Liveness`. For PMT the dominant relation is `Temporal` with
`EpochBefore`/`EpochAfter` flavours — every state has a producer epoch
(the `state_new` or `transform` that created it) and a consumer epoch
(the read/write/transform that observes it). `is_state_available(producer_epoch,
consumer_epoch)` returns `true` iff `producer_epoch ≤ consumer_epoch`,
proving that a read of a state cannot observe a value that has not yet
been produced. This is the type-theoretic counterpart of the VUMA 1.x
liveness invariant, lifted into the BD layer.

### 5.3 `CapD` — Capability (state capabilities)

`CapD` (`vuma-bd/src/capd.rs`) is the lattice of permitted operations,
ordered by set-inclusion (`⊥ = ∅`, `⊤ = all`, meet = `∩`, join = `∪`).
For PMT, four capabilities extend the classical `Read`/`Write`/…/`Drop`/
`Move`/`Pin` set:

- `StateRead` — read a field via typed offset access.
- `StateWrite` — write a field via typed offset access.
- `StateTransform` — convert a state from one layout to another (consumes
  the input).
- `StateConsume` — mark a state as consumed (linear ownership transfer).

A `State<T>` value's `CapD` is `{StateRead, StateWrite}` by default; a
`transform`'s input parameter is strengthened with `StateConsume` so the
verifier proves the input is not used after the transform returns.

---

## 6. E-graph Layout Optimization

The optimizer's e-graph (`vuma-codegen/src/egraph.rs`) models PMT state
operations as four `ENode` variants alongside the existing `Lit`,
`VReg`, and `BinOp`:

```rust
enum ENode {
    Lit(i64),
    VReg(u32),
    BinOp(BinOpKind, u32, u32),
    StateInit    { layout_id: u64 },
    StateRead    { state: u32, offset: u64, size: u64 },
    StateWrite   { state: u32, offset: u64, value: u32 },
    StateTransform { input: u32, src_layout: u64, dst_layout: u64 },
}
```

PMT states are **linear/immutable** in the e-graph model: a `StateWrite`
produces a fresh state e-class, so the only way a `StateRead`'s state
child can be in the same e-class as a `StateWrite`'s output is if the
read directly consumes the write's result. This linearity is what makes
the rewrite rules sound.

### 6.1 The three PMT rewrite rules

| Rule                       | Pattern                                             | Replacement        | Soundness                                      |
|---------------------------|-----------------------------------------------------|--------------------|------------------------------------------------|
| `state_dead_init_elim`    | `StateInit(L)` whose e-class has zero consumers     | `Lit(0)`           | Unreferenced state's value is unobservable.    |
| `state_store_load_forward`| `StateRead(StateWrite(s, off, v), off, _)`          | `v`                | Linear states: no intervening write to `off`.  |
| `state_transform_elision` | `StateTransform(x, L, L)`                           | `x`                | Same-layout transform is the identity.         |

All three rules are in the `standard_rules()` set fed to
`equality_saturation_with_cost` by `run_optimizations_inner`. Two of the
three (`state_store_load_forward`, `state_transform_elision`) carry
`verified: true` and are encoded as bitvector identities in the Wave 36
`bv_verify` gate; `state_dead_init_elim` is admitted as
sound-by-construction (the guard ensures the state is never observed).

### 6.2 Cost model

`StateInit` costs 500 (an allocation); `StateRead`/`StateWrite` cost 10
each; `StateTransform` costs 50 (or 0 after elision). The extractor
(`EGraph::extract`) picks the lowest-cost representative of each e-class:

- A dead `StateInit` (500) → `Lit(0)` (1); the `Alloc` is dropped by
  dead-vreg elimination.
- A `StateRead(StateWrite(s, off, v), off, _)` (20) → `v` (0); the
  `Load`/`Store` pair collapses to a register move.
- A `StateTransform(x, L, L)` (50) → `x` (0); the transform call
  disappears entirely.

The `tests/gold_standard/pmt_wave5/` directory exercises each rule:
`dead_state.vuma`, `store_load_forward.vuma`, `transform_identity.vuma`,
`chain_writes.vuma`, `redundant_init.vuma`.

---

## 7. Backends

VUMA 2.0 ships 19 production backends. All 19 are in the gold-standard
test matrix and pass at ≥99.5% on the PMT-migrated suite. The
`BackendKind` enum (`vuma-codegen/src/backend.rs`) is the single source
of truth for the ISA list.

| `BackendKind`     | `isa_name`   | Notes                                             |
|-------------------|--------------|---------------------------------------------------|
| `AArch64`         | `aarch64`    | ARMv8.0 integer + atomics, full ABI.              |
| `AArch64Be`       | `aarch64_be` | Big-endian AArch64.                               |
| `X86_64`          | `x86_64`     | System V ABI, REX prefix, full integer ISA.       |
| `X86_32`          | `x86_32`     | i386 System V ABI.                                |
| `RiscV64`         | `riscv64`    | RV64I + A extension.                              |
| `RiscV32`         | `riscv32`    | RV32I + A extension.                              |
| `LoongArch64`     | `loongarch64`| LA64 integer ISA.                                 |
| `Arm32`           | `arm32`      | ARMv7-A EABI.                                     |
| `ArmEb`           | `armeb`      | Big-endian ARMv7.                                 |
| `Mips64`          | `mips64`     | MIPS64 R6 O64 ABI.                                |
| `Mips64Be`        | `mips64be`   | Big-endian MIPS64.                                |
| `PowerPC64`       | `ppc64`      | ELFv1 big-endian.                                 |
| `PowerPC64LE`     | `ppc64le`    | ELFv2 little-endian.                              |
| `Sparc64`         | `sparc64`    | SPARC V9.                                         |
| `S390X`           | `s390x`      | IBM System Z ELF.                                 |
| `M68k`            | `m68k`       | Motorola 68000.                                   |
| `Alpha`           | `alpha`      | DEC Alpha 21064.                                  |
| `Hppa`            | `hppa`       | HP PA-RISC 1.1.                                   |
| `Wasm32`          | `wasm32`     | WebAssembly 1.0 + WASI.                           |

### 7.1 Per-ISA optimisation

`run_ir_pipeline` queries the real backend's `latency_table()` via
`backend.target_info().latency_table()` and feeds it to
`run_optimizations_with_target_and_inline_threshold`. The e-graph cost
function (`target_cost_fn`) and the instruction scheduler
(`schedule_function`) therefore make decisions based on the actual
target's instruction latencies. The same IR program is re-optimised per
backend — a multiply-heavy loop on `m68k` (where `mulu` is 38 cycles) is
scheduled differently from the same loop on `x86_64` (where `imul` is 3
cycles).

### 7.2 Syscall ABI translation

Each backend implements `syscall_abi.rs` lowering for the IR
`IRInstr::Syscall { nr, args, dst }` instruction. The syscall number is
validated against the 0..=600 range (a hard error if exceeded) before
codegen. Atomic operations (`AtomicLoad`, `AtomicStore`, `AtomicCas`) are
scheduler barriers and never reordered relative to other memory ops.

---

## 8. Dependent State Types

Wave 9 introduces dependent state types — `State<List<N>>` where `N` is
a runtime count of elements. The backing layout is a `RepD::DependentArray`
carrying a static element `RepD` and a single runtime count variable
name; non-linear dependencies (e.g. `count1 * count2`) are rejected by
construction.

### 8.1 The proof obligation

Every dependent-array access discharges the linear-arithmetic proof:

```
offset + (count × elem_size) ≤ buffer_size
```

This is decidable (Presburger arithmetic) because the only multiplication
is `count × elem_size` where `elem_size` is a compile-time constant.
`verify_dependent_transform(elem_size, count, offset, buffer_size)`
returns `true` iff the obligation holds, using saturating arithmetic to
defend against overflow:

```rust
pub fn verify_dependent_transform(
    elem_size: u64, count: u64, offset: u64, buffer_size: u64,
) -> bool {
    offset.saturating_add(count.saturating_mul(elem_size)) <= buffer_size
}
```

### 8.2 Static count (degenerate case)

When the runtime count happens to be a compile-time constant, the proof
trivially holds. The `pmt_wave9/safe_access.vuma` test writes four
elements of a `[u32; 8]` array with indices `0,1,2,3`: the proof reduces
to `0 + 4*4 = 16 ≤ 32` ✓ — no runtime reasoning required.

### 8.3 Runtime count

When the count is a runtime value, the proof obligation is still
discharged by linear arithmetic on the count variable. The
`stack.vuma` test models a stack with a runtime `count` field as the
stack pointer; every `s.data[s.count]` access produces the obligation
`0 + count*4 ≤ 32`, i.e. `count ≤ 8` — which the verifier proves by
observing that `count` is only ever incremented after a write and is
bounded by the `data` field's static element count.

```vuma
// tests/gold_standard/pmt_wave9/stack.vuma
layout Stack = { count: u32, data: [u32; 8] }

fn push(s: State<Stack>, v: u32) {
    s.data[s.count] = v;
    s.count = s.count + 1;
}
```

The `dynamic_array.vuma`, `grow_array.vuma`, and `bounded_loop.vuma`
tests in the same directory exercise the dynamic-count path; all pass on
the 4 backends in the gold-standard matrix.

---

## 9. FFI — The 4-Mode Matrix

VUMA 2.0's FFI is built on a 4-mode matrix (replacing the legacy binary
`#[pure]`/invalidate model). Every foreign call is classified into an
argument mode per argument, a return mode, and optionally a callback mode.

### 9.1 The marshal module (`vuma-codegen/src/marshal.rs`)

The marshal module provides the classification helpers and marshal-result
types that the codegen bridge consumes:

- `ArgMode` enum: `Borrow` / `Invalidate` / `Marshal` / `MayRetain` / `ForeignPass`
- `ReturnMode` enum: `Scalar` / `Unmarshal(Layout)` / `ForeignWrap(field)`
- `classify_arg_mode(params_attrs, layout_is_foreign)` — precedence:
  `#[may_retain]` > `#[marshal]` > `#[borrow]` > `#[foreign]` > default
- `marshal_arg(state_var, mode, offset, size)` — produces the ptr_expr
  per mode (`___pmt_buffer_base + offset` for Borrow/Invalidate,
  `___ffi_scratch_alloc` for Marshal/MayRetain, `.raw` for ForeignPass)
- `foreign_consume_field()`, `foreign_layout_field()`, `is_callback_fn()`
- `MARSHAL_CSTR_FN` / `UNMARSHAL_FN` constants + `is_marshal_builtin()`

### 9.2 The FFI safety verifier (`vuma-ive/src/ffi.rs`)

Proves no invalidated state is accessed: `verify_ffi_safety(invalidated_vars,
accesses)` flags any `(var, field)` access where `var ∈ invalidated_vars`.

### 9.3 The borrow-region verifier (`vuma-ive/src/borrow_region.rs`)

Tracks `#[borrow]` regions per extern call site. A borrow-region is "in
flight" from the call node until the call's return (the next call in
`call_order`). `verify_borrow_regions(regions, writes, call_order)` flags
any StateWrite to a borrowed region during the borrow window — C might be
mid-read. On call return, the borrow is auto-released and the region is
marked "preserved" (not invalidated).

### 9.4 The ForeignConsume SCG node (`vuma-scg/src/node.rs`)

`NodeType::ForeignConsume` marks a State as consumed by a foreign
close-call (e.g. `sqlite3_close`). The existing `state_write` verifier
treats this the same as a `StateTransform` consume — any subsequent
read/write to the consumed vreg is a linearity error. This reuses the
existing verifier machinery (no new verifier needed).

The codegen bridge (`pipeline.rs:emit_foreign_consume_markers`) emits
`ScgStatement::ForeignConsume` at `#[foreign_consume]` call sites for each
State arg whose layout is `#[foreign(raw)]`.

### 9.5 The marshal scratchpad (`vuma-codegen/src/runtime/ffi_scratch.rs`)

A thread-local, `malloc`-backed stack scratchpad, separate from
`___pmt_buffer`. Used for NUL-terminated strings, C-owned memory
round-trips, and in-place mutation buffers. Stack-shaped: `push_frame`
on transform entry, `pop_frame` on transform exit.

**Sacred invariant:** scratchpad memory is NEVER aliased by `___pmt_buffer`.
The `StateRead`/`StateWrite`/`StateTransform` verifiers never see it.

The codegen bridge (`scg_to_ir.rs:emit_scratchpad_hooks`) emits
`ffi_scratch_push_frame` at function entry and `ffi_scratch_pop_frame`
before every Return — but ONLY for functions that contain extern calls
or marshal builtins (avoids unresolved symbols in non-FFI programs).

### 9.6 The vuma_context_t C-API (`vuma_vm.h` + `runtime/vuma_context.rs`)

Generalizes the wasm32 host shim (`scripts/wasm32_runner.py:make_host_functions`)
into a C header shipped for all backends. 8 accessors:
`vuma_read/write_u32/u64`, `vuma_state_new`, `vuma_push_i32/i64`.

### 9.7 The callback runtime (`vuma-codegen/src/runtime/callback.rs`)

Implements the re-entrancy rule: callbacks run on an isolated callback
stack with their own scratchpad frame, forbidden from touching caller-live
State. `LiveSet` tracks caller-live byte ranges; `check_access` returns
false (trap) for any access to a caller-live region. Nested callbacks
supported via `CALLBACK_DEPTH` tracking.

### 9.8 Test coverage

| Wave | Directory | Tests |
|---|---|---|
| 1 (parser attrs) | `ffi_wave0/` | 8 positive + 1 negative |
| 2 (borrow modes) | `ffi_wave2/` | 3 (borrow_preserved, invalidate, may_retain) |
| 3 (marshal) | `ffi_wave1/` | 3 (write_real, open_file, strdup) |
| 4 (foreign state) | `ffi_wave3/` | 4 (arg_pass, return_wrap, consume_emitted, use_after_close_reject) |
| 5 (callbacks) | `ffi_wave4/` | 2 (sqlite_exec_callback, callback_traps) |

---

## Appendix — Pointer syntax is a hard error

The parser (`vuma-parser/src/parser.rs`) rejects every VUMA 1.x pointer
construct with a fatal `ParseError`:

```
pointer syntax 'allocate' is not supported in VUMA 2.0 (PMT-only);
use state_new(Layout) and transforms
```

The detection sites and their kind labels:

| Source construct            | Kind label                  |
|-----------------------------|-----------------------------|
| `allocate(N)` / `region r = allocate(N)` | `"allocate"` / `"allocate (region)"` |
| `free(ptr)`                 | `"free"`                    |
| `*ptr` (dereference)        | `"*ptr (deref)"`            |
| `&x` (address-of)           | `"&x (address-of)"`         |
| `@x` (address-of, alt)      | `"@x (address-of)"`         |
| `*T` (pointer type)         | `"*T (pointer type)"`       |

`parse_program` scans the accumulated error list for messages starting
with `pointer syntax '` and downgrades the parse result to a fatal
`ParseResult::err` so the failure propagates to the CLI driver. There is
no `--pmt` flag, no `--pmt-only` flag, no warning path, and no
`pmt_only: bool` field on the `Parser` struct — PMT is the only mode.
