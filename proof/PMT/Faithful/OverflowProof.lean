import Init.Prelude
import Init.Data.Fin.Basic

/-!
# Overflow / out-of-bounds soundness for `Arena.alloc`

This file is self-contained: it pastes the `USize`, `Ptr`, `Arena`, and
`Arena.alloc` definitions inline (everything lives in `namespace Pmt` so it
does not clash with Lean's built-in `USize`) and proves the two soundness
properties the surrounding verification harness relies on:

  * `alloc_overflow_returns_none` — if the `USize.add a.used size` bump
    overflows (returns `none`), then `Arena.alloc` returns `none`.
  * `alloc_oob_returns_none`   — if the (non-overflowing) bumped offset
    exceeds `capacity`, then `Arena.alloc` returns `none`.

Both proofs `cases` on the `Option (USize.add a.used size)` that drives
`Arena.alloc` and discharge the resulting arithmetic goals with `omega`
over the underlying `Nat` `.val`s. No axioms, no placeholders.
-/

namespace Pmt

/-! ## `USize` — `usize` on a 64-bit target, modelled as `Fin (2^64)`.

    Marked `@[reducible]` so the `Fin` `Ord`/`Decidable` instances behind
    the `<`/`>` comparisons in `Arena.alloc` resolve by unfolding. -/

@[reducible] def USize := Fin (2^64)

/-- `checked_add` for `USize`: returns `none` exactly on overflow. -/
def USize.add (a b : USize) : Option USize :=
  if a.val + b.val < 2^64 then some (Fin.add a b) else none

/-! ## `Ptr` — an address with allocation-site provenance. -/

/-- A pointer carries an absolute address and a provenance tag identifying
    the allocation it was derived from. -/
structure Ptr where
  addr : Nat
  provenance : Nat

/-! ## `Arena` — a bump allocator over one mmap'd region. -/

/-- A faithful arena: a single mmap'd region starting at `base`, a total
    `capacity`, a bump `used` offset, and a monotone `alloc_id` provenance
    counter. -/
structure Arena where
  base : Ptr
  capacity : USize
  used : USize
  alloc_id : Nat

/-! ## `Arena.alloc` — checked bump allocation.

    Returns `none` when `USize.add a.used size` overflows *or* the resulting
    bump offset meets/exceeds `capacity`. On success the arena's `used` is
    bumped to `w`, `alloc_id` is incremented, and the returned pointer has
    `addr = base.addr + used.val` (the *old* offset) and
    `provenance = alloc_id`. `align` is carried for API fidelity with the
    Rust allocator and is unused by the model. -/

set_option linter.unusedVariables false in
def Arena.alloc (a : Arena) (size align : USize) : Option (Arena × Ptr) :=
  match USize.add a.used size with
  | none => none
  | some w =>
    if w < a.capacity then
      some ({ a with used := w, alloc_id := a.alloc_id + 1 },
            { addr := a.base.addr + a.used.val, provenance := a.alloc_id })
    else none

/-! ## Overflow / out-of-bounds theorems. -/

/- Theorem 1: if `USize.add a.used size` overflows, `alloc` returns `none`.

    We `unfold Arena.alloc` to expose the `match` on `USize.add a.used size`
    and `cases` on that `Option`. In the `none` branch the `match` reduces to
    `none`, so `(.isNone)` holds trivially. In the `some w` branch the
    overflow hypothesis `h : (USize.add a.used size).isNone` is rewritten by
    `h2 : USize.add a.used size = some w` into `(some w).isNone`, which
    `simp at h` reduces to `False`, closing the goal. -/
set_option linter.unusedVariables false in
theorem alloc_overflow_returns_none (a : Arena) (size align : USize)
    (h : (USize.add a.used size).isNone) :
    (Arena.alloc a size align).isNone := by
  -- Expose the `match USize.add a.used size` driver of `Arena.alloc`.
  unfold Arena.alloc
  -- Split on the overflow check. `h2` records which branch we are in.
  cases h2 : USize.add a.used size with
  | none =>
    -- No carry-free sum exists: the `match` selects `none`, so `alloc` is
    -- `none` and `.isNone` holds. `simp [h2]` reduces the `match`/`isNone`.
    simp [h2]
  | some w =>
    -- A carry-free sum `w` exists, contradicting the overflow hypothesis.
    -- Transport `h2` into `h`, then `simp at h` derives `False`.
    rw [h2] at h
    simp at h

/- Theorem 2: if the (non-overflowing) bump offset exceeds `capacity`,
    `alloc` returns `none`.

    `h_oob` depends on `h_add`, whose type mentions `USize.add a.used size`,
    so we `revert` both before `cases`-ing on that `Option` (this lets the
    dependent motive typecheck). In the `none` branch `h_add` is
    `(none).isSome`, which `simp at h_add` reduces to `False`. In the
    `some w` branch `Option.get_some` turns `(some w).get h_add` into `w`,
    so `h_oob : w > a.capacity`; this contradicts the success guard
    `w < a.capacity` (discharged by `omega` over the `Nat` `.val`s), and if
    the guard is false the `if` selects `none` (closed by `simp [h3]`). -/
set_option linter.unusedVariables false in
theorem alloc_oob_returns_none (a : Arena) (size align : USize)
    (h_add : (USize.add a.used size).isSome)
    (h_oob : (USize.add a.used size).get h_add > a.capacity) :
    (Arena.alloc a size align).isNone := by
  -- Expose the `match USize.add a.used size` driver of `Arena.alloc`.
  unfold Arena.alloc
  -- `h_oob` depends on `h_add`; revert both so the `cases` motive typechecks.
  revert h_oob
  revert h_add
  -- Split on the overflow check. `h2` records which branch we are in.
  cases h2 : USize.add a.used size with
  | none =>
    -- `h_add : (none).isSome` is contradictory; `simp at h_add` derives
    -- `False` and closes the goal.
    intro h_add _
    simp at h_add
  | some w =>
    -- Re-introduce the (now some-typed) hypotheses.
    intro _ h_oob
    -- `(some w).get _ = w`, so `h_oob : w > a.capacity`.
    rw [Option.get_some] at h_oob
    -- Split on the success guard `w < a.capacity`.
    by_cases h3 : w < a.capacity
    · -- Guard true: `w < a.capacity` contradicts `h_oob : w > a.capacity`.
      -- Unfold the `Fin` comparisons to `Nat` `.val` facts for `omega`.
      have h3v : w.val < a.capacity.val := h3
      have hobv : w.val > a.capacity.val := h_oob
      -- `omega` case-splits the `if` guard; the `w < a.capacity` branch is
      -- contradictory (`h3v`/`hobv`), the other reduces `none.isNone`.
      omega
    · -- Guard false: the `if` selects `none`, so `alloc` is `none`.
      simp [h3]

end Pmt
