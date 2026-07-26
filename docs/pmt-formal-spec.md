# PMT Formal Spec

This document is the **formal Lean signature and proof-sketch spec** for
the PMT runtime memory-safety argument. It complements
[`./pmt-iris-spec.md`](./pmt-iris-spec.md) — the Iris separation-logic
spec — by giving the bare-`Prop` Lean signatures, runtime hypotheses,
and proof sketches for the runtime side of each theorem.

  - The **Iris layer** (separation logic, ghost state, named invariants)
    is specified in [`./pmt-iris-spec.md`](./pmt-iris-spec.md).
  - This document covers the **runtime layer** (Lean `Prop` predicates
    over `Arena`, `Layout`, `LinearToken`, and the PMT step-relation).

## Contents

  - §1  Arena model
    - §1.3  `arena_alloc` test
  - §2  `StateValRes` (runtime half) — *see Iris spec §2*
  - §3  Field-bounds safety theorem
  - §4  Liveness-bounded access theorem
  - §5  Liveness theorem (runtime half)
  - §6  Guard page (runtime half) — *see Iris spec §6*
  - §7  Codegen trap contract
  - §8  Trusted Computing Base — *see Iris spec §8*

---

## §1. Arena model

An arena `A : Arena` is a contiguous region of memory with:

  - `A.base     : Nat` — the base address;
  - `A.capacity : Nat` — the total capacity (in bytes);
  - `A.used     : Nat` — the bump pointer (bytes already allocated).

The arena is created by the runtime `arena_alloc`, which `mmap`s
`capacity + guard_size` bytes and `mprotect`s the trailing `guard_size`
bytes to `PROT_NONE` (the guard page — see §6 of
[`./pmt-iris-spec.md`](./pmt-iris-spec.md)).

### `CapacityInvariant`

```lean
capacity_inv A := ⌜A.used ≤ A.capacity⌝
```

This is the bare `Prop` that the Iris `[cap_bnd]` invariant (Iris spec
§3) upgrades to a separation-logic resource. It is the hypothesis of the
top-level `pmt_soundness` theorem in `PMT.Basic`.

### §1.3. `arena_alloc` test

The runtime `arena_alloc` is the *trusted* entry point that bumps `used`
within `capacity`. The Lean side *tests* (rather than trusts) the
bookkeeping by checking:

```lean
arena_alloc_preserves_capacity :
  ∀ A l, A.used + l.total_size ≤ A.capacity →
         capacity_inv (alloc A l)
```

i.e. `arena_alloc`'s bump-pointer update `used ↦ used + l.total_size`
preserves `capacity_inv` whenever the precondition
`A.used + l.total_size ≤ A.capacity` holds. This is the runtime
precondition tested by `arena_alloc`; the Iris side (Iris spec §3)
reconstructs the ghost state.

**Lean reference.** `proof/PMT/Basic.lean`.

---

## §2. `StateValRes` (runtime half)

The runtime half of fractional field permissions is the bare
`points-to` relation `l ↦ v` (full permission `q = 1`). The fractional
permission machinery (`q ∈ (0, 1]`) lives in the Iris layer — see
[`./pmt-iris-spec.md`](./pmt-iris-spec.md) §2.

---

## §3. Field-bounds safety theorem

The field-bounds safety theorem states that any `StateRead` / `StateWrite`
whose `vreg` has been registered with a `Layout` (size, align) and whose
offset is within the layout is in-bounds with respect to the arena.

**Statement.** Given a `vreg` with layout `l`, an offset `off`, and an
arena `A` such that:

  - `vreg`'s storage is allocated within `A` (i.e. `A.used` was bumped
    past `vreg`'s storage when `vreg` was created);
  - `off + l.size ≤ vreg.size` (the access is within the vreg's
    layout);

then the address `vreg.base + off` satisfies
`A.base ≤ vreg.base + off ∧ vreg.base + off + l.size ≤ A.base + A.used`,
hence is in-bounds with respect to the arena.

This is the runtime field-bounds safety theorem — the bare-`Prop`
companion of the Iris-side composition (Iris spec §4). It is the
*field-bounds* half; the *liveness-bounded* half is §4 below.

**Lean reference.** `proof/PMT/Field.lean` (mirrors Iris spec §3).

---

## §4. Liveness-bounded access theorem

The liveness-bounded access theorem states that any `StateRead` /
`StateWrite` whose `vreg` is `Accessible` (its `LinearToken` has
`status = live`) reads / writes a live storage cell, and conversely
that any `StateRead` / `StateWrite` whose `vreg` has been `Consumed`
(status = `dead`) is rejected at compile time by IVE linearity
checking.

This is the *liveness-bounded* half of the field safety argument — the
bare-`Prop` companion of the Iris-side composition (Iris spec §4). It
is the dual of §3 (which is the *field-bounds* half).

**Compile-time rejection.** Any `StateWrite` / `StateRead` whose `vreg`
has been consumed by a prior `StateTransform` is rejected by IVE
linearity (see `tests/gold_standard/bounds_basic/uaf_compile_time.vuma`).
The runtime trap (§7) is the *fallback* for the cases IVE cannot
statically reject.

**Lean reference.** `proof/PMT/Field.lean` (mirrors Iris spec §4).

---

## §5. Liveness theorem (runtime half)

The runtime half of the liveness theorem states that `StateTransform`
kills its input: after a `StateTransform` on `vreg`, the `LinearToken`
for `vreg` flips from `Accessible` (status = `live`) to `Consumed`
(status = `dead`).

```lean
state_transform_kills_input :
  ∀ t, Accessible t → Consumed (state_transform t)
```

This is the runtime half of the liveness theorem; the ghost-mirror half
is the Iris `[live_mirror]` invariant (Iris spec §5), whose
`consume_updates_mirror` lemma flips `own(γ, Ex live) ~~> own(γ, Ex dead)`
in lock-step with this runtime transition.

**Compile-time vs. runtime.** IVE linearity statically rejects any
*syntactic* use-after-consume (§4); this theorem is the runtime
guarantee that the `LinearToken` actually flips state, so the
`live_mirror` ghost state can be soundly updated.

**Lean reference.** `proof/PMT/Liveness.lean`.

---

## §6. Guard page (runtime half)

The runtime half of the guard-page argument is the bare `GuardPage`
predicate:

```lean
GuardPage A addr := A.base + A.capacity ≤ addr
```

i.e. any address `addr ≥ A.base + A.capacity` is in the guard page.
Combined with the trusted `PROT_NONE` mmap (Iris spec §8), any access to
such an address traps via the MMU. The Iris `[guard]` invariant (Iris
spec §6) upgrades this bare `Prop` to a separation-logic resource.

**Lean reference.** `proof/PMT/Liveness.lean` (`GuardPage`,
`in_arena_below_guard`).

---

## §7. Codegen trap contract

The codegen trap contract is the runtime contract between the VUMA
codegen and the PMT memory-safety argument:

  - Every `Seq` access into an arena-allocated state buffer is preceded
    by an `UGe` bounds check that traps via `__oob_trap` (exit 134) on
    out-of-bounds access.
  - `__oob_trap` is an extern stub that calls `exit(134)` (SIGABRT). It
    exists on all 19 backends.
  - Raw-pointer / `length_expr=None` accesses are *not* covered by this
    contract (future SoftBound work).

The trap is the *runtime fallback* for the cases IVE linearity cannot
statically reject (§4). The contract is documented in
`src/codegen/src/runtime/arena.rs` (the `arena_alloc` and
`__oob_trap` stubs) and tested by the gold-standard negative tests in
`tests/gold_standard/bounds_safe/` and `tests/gold_standard/bounds_basic/`.

**Codegen reference.** `src/codegen/src/runtime/arena.rs`,
`src/codegen/src/memory_safety.rs` (`inject_bounds_check_ir`),
`src/pipeline.rs` (Stage 5 invocation).

---

## §8. Trusted Computing Base — *see Iris spec §8*

The TCB is specified in [`./pmt-iris-spec.md`](./pmt-iris-spec.md) §8.
The runtime contracts (mmap `PROT_NONE` semantics, `arena_alloc`
bookkeeping) are the same; the Lean proofs (capacity bound, liveness
mirror, guard-page address, field-bounds safety, composition) are
summarised in §1–§6 above and proven in `proof/PMT/*.lean`.

---

## Cross-references

  - **Source of truth for the Lean signatures and proof sketches**: this
    document (`docs/pmt-formal-spec.md`).
  - **Source of truth for the Iris proofs**:
    [`./pmt-iris-spec.md`](./pmt-iris-spec.md).
  - **Runtime Lean modules**: `proof/PMT/*.lean` (`Basic.lean`,
    `Liveness.lean`, `Field.lean`).
  - **Iris modules**: `proof/PMT/Iris/*.lean`.
  - **Codegen trap contract**: `src/codegen/src/runtime/arena.rs`,
    `src/codegen/src/memory_safety.rs`.
  - **Architecture overview**: [`./architecture.md`](./architecture.md).
  - **Pipeline overview**: [`./pipeline.md`](./pipeline.md).
