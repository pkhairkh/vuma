import Init.Prelude
import Init.Data.Fin.Basic
import Init.Data.List.Basic

/-! # Faithful PMT Model — USize, Ptr, Arena, Layout -/

namespace Pmt

/-! ## `USize` — `usize` on a 64-bit target, modelled as `Fin (2^64)`. -/

/-- `usize` on a 64-bit target is exactly the finite ordinal `Fin (2^64)`.

    Marked `@[reducible]` so that the `Fin` `Ord`/`Decidable` instances
    (used by the `<`/`>`/`≥` comparisons in `Arena.alloc` and the
    overflow theorems) resolve by unfolding. -/
@[reducible] def USize := Fin (2^64)

/-- `checked_add` for `USize`: returns `none` exactly on overflow. -/
def USize.add (a b : USize) : Option USize :=
  if a.val + b.val < 2^64 then some (Fin.add a b) else none

/-- `add` overflows exactly when the unbounded sum meets/exceeds `2^64`. -/
theorem USize.add_overflow_safe (a b : USize) :
    (USize.add a b).isNone ↔ a.val + b.val ≥ 2^64 := by
  unfold USize.add; by_cases h : a.val + b.val < 2^64
  · rw [if_pos h]; simp; omega
  · rw [if_neg h]; simp; omega

/-! ## `Ptr` — an address with allocation-site provenance. -/

/-- A pointer carries an absolute address and a provenance tag identifying
    the allocation it was derived from. -/
structure Ptr where
  addr : Nat
  provenance : Nat

/-- Pointer arithmetic: advance the address by `n`, keeping the provenance. -/
def Ptr.offset (p : Ptr) (n : Nat) : Ptr :=
  { addr := p.addr + n, provenance := p.provenance }

/-- A pointer is in bounds of `[base, base + size)`. -/
def Ptr.inBounds (p : Ptr) (base size : Nat) : Prop :=
  base ≤ p.addr ∧ p.addr < base + size

/-! ## `Arena` — a bump allocator over one mmap'd region. -/

/-- A faithful arena: a single mmap'd region `[base, base + capacity)`,
    a bump `used` offset, and a monotone `alloc_id` provenance counter. -/
structure Arena where
  base     : Ptr          -- mmap'd region start (provenance = allocation ID)
  capacity : USize        -- total mmap size
  used     : USize        -- bump offset
  alloc_id : Nat          -- next provenance tag

/-! ## `Arena.alloc` — checked bump allocation.

    Returns `none` when `USize.add a.used size` overflows *or* the
    resulting bump offset meets/exceeds `capacity` (so a successful
    allocation always hands back a pointer strictly inside
    `[base.addr, base.addr + capacity.val)`). On success the arena's
    `used` is bumped, `alloc_id` is incremented, and the returned
    pointer has `addr = base.addr + used.val` (the *old* offset) and
    `provenance = alloc_id`. -/

set_option linter.unusedVariables false in
def Arena.alloc (a : Arena) (size align : USize) : Option (Arena × Ptr) :=
  match USize.add a.used size with
  | none => none
  | some new_used =>
    if new_used < a.capacity then
      some (
        { base := a.base,
          capacity := a.capacity,
          used := new_used,
          alloc_id := a.alloc_id + 1 },
        { addr := a.base.addr + a.used.val,
          provenance := a.alloc_id })
    else none

/-! ## Overflow / out-of-bounds / success theorems.

    Each theorem destructs the `Option (USize.add a.used size)` and
    reasons about the `USize.add` overflow guard directly — no axioms,
    no placeholders, and the bounds check in theorem 4 is discharged with
    `omega` over the underlying `Nat` `.val`s. -/

/-- Theorem 2: if `USize.add a.used size` overflows, `alloc` returns
    `none`. We `cases` on the `Option`; the `none` branch reduces to
    `none.isNone = true`, and the `some` branch contradicts the
    overflow hypothesis `h`. -/
theorem alloc_overflow_returns_none (a : Arena) (size align : USize)
    (h : (USize.add a.used size).isNone) :
    (Arena.alloc a size align).isNone := by
  unfold Arena.alloc
  cases h2 : USize.add a.used size with
  | none => simp [h2]
  | some w =>
    rw [h2] at h
    simp at h

/-- Theorem 3: if the (non-overflowing) bump offset exceeds `capacity`,
    `alloc` returns `none`. We `cases` on the `Option`; the `none`
    branch contradicts `h_add`, and the `some w` branch uses the
    out-of-bounds hypothesis `h_oob : w > a.capacity` (extracted from
    `(USize.add a.used size).get h_add` via `Option.get_some`) to
    contradict the success guard `w < a.capacity`. -/
theorem alloc_oob_returns_none (a : Arena) (size align : USize)
    (h_add : (USize.add a.used size).isSome)
    (h_oob : (USize.add a.used size).get h_add > a.capacity) :
    (Arena.alloc a size align).isNone := by
  unfold Arena.alloc
  -- `h_oob` depends on `h_add`, whose type mentions `USize.add a.used size`;
  -- reverting both before the `cases` lets the dependent motive typecheck.
  revert h_oob
  revert h_add
  cases h2 : USize.add a.used size with
  | none =>
    intro h_add h_oob
    simp at h_add
  | some w =>
    intro h_add h_oob
    -- `(some w).get h_add = w`, so `h_oob : w > a.capacity`.
    rw [Option.get_some] at h_oob
    by_cases h3 : w < a.capacity
    · -- `w < a.capacity` (the success guard) contradicts `h_oob : w > a.capacity`.
      have h3v : w.val < a.capacity.val := h3
      have hobv : w.val > a.capacity.val := h_oob
      omega
    · -- guard false: the `if` selects `none`, so `alloc` is `none`.
      simp [h3]

set_option linter.unusedVariables false in
/-- Theorem 4: a successful allocation yields a pointer strictly inside
    `[base.addr, base.addr + capacity.val)`. We `cases` on the
    `Option (USize.add a.used size)`; the `none` branch contradicts
    `h`, and the `some w` branch splits on the success guard
    `w < a.capacity`. In the success sub-case we transport the `isSome`
    proof across the reduction of `Arena.alloc` to `some (arena', ptr)`,
    reduce the `let (a', p) := … .get h` to `ptr.inBounds …`, then close
    the resulting `Nat` bounds goal with `omega` using:
      * `h3`      : `w < a.capacity`  (the success guard),
      * `h4`      : `a.used.val + size.val < 2^64` (no overflow), and
      * `hwv`     : `w.val = a.used.val + size.val` (the unwrapped sum). -/
theorem alloc_success_inBounds (a : Arena) (size align : USize)
    (h : (Arena.alloc a size align).isSome) :
    let (a', p) := (Arena.alloc a size align).get h
    p.inBounds a.base.addr a.capacity.val := by
  -- Destructure the `Option (USize.add a.used size)` (the overflow check).
  cases h2 : USize.add a.used size with
  | none =>
    -- `Arena.alloc` reduces to `none`, contradicting `h : … .isSome`.
    have heq : Arena.alloc a size align = none := by
      unfold Arena.alloc; rw [h2]
    rw [heq] at h
    simp at h
  | some w =>
    -- Split on the success guard `w < a.capacity`.
    by_cases h3 : w < a.capacity
    case pos =>
      -- On success `Arena.alloc` reduces to a concrete `some (arena', ptr)`.
      have heq : Arena.alloc a size align = some
          ({ base := a.base, capacity := a.capacity, used := w,
             alloc_id := a.alloc_id + 1 },
           { addr := a.base.addr + a.used.val, provenance := a.alloc_id }) := by
        unfold Arena.alloc
        simp only [h2]
        rw [if_pos h3]
      -- Transport `h` across `heq` (revert/intro avoids the dependent
      -- motive issue from `h`'s type mentioning `Arena.alloc a size align`).
      revert h
      rw [heq]
      intro h
      -- Reduce `(some (arena', ptr)).get h` to `(arena', ptr)` and the
      -- `let (a', p) := (arena', ptr)` to the projected `Ptr.inBounds` goal.
      dsimp only [Option.get_some]
      unfold Ptr.inBounds
      dsimp only []
      -- Split on the `USize.add` overflow guard. This `unfold … at h2` is
      -- done in the *main* proof context (not inside a `have`) so that the
      -- unfolding of `h2` persists for the `hwv` derivation below.
      unfold USize.add at h2
      by_cases h4 : a.used.val + size.val < 2^64
      · -- No overflow: `USize.add` yields `some (Fin.add a.used size)`, so
        -- `w = Fin.add a.used size` and, since there is no carry,
        -- `w.val = a.used.val + size.val`.
        rw [if_pos h4] at h2
        have h2eq : Fin.add a.used size = w := Option.some.inj h2
        have hwv : w.val = a.used.val + size.val := by
          rw [← h2eq]
          show (a.used.val + size.val) % 2^64 = a.used.val + size.val
          exact Nat.mod_eq_of_lt h4
        -- Convert the `Fin` success guard to a `Nat` bound for `omega`.
        have h3v : w.val < a.capacity.val := h3
        -- `omega` closes the `Nat` bounds from `h3v`, `hwv` (and `h4`).
        omega
      · -- Overflow: `USize.add` is `none`, contradicting `h2 : … = some w`.
        rw [if_neg h4] at h2
        simp at h2
    case neg =>
      -- Guard false: `Arena.alloc` reduces to `none`, contradicting `h`.
      have heq : Arena.alloc a size align = none := by
        unfold Arena.alloc
        simp only [h2]
        rw [if_neg h3]
      rw [heq] at h
      simp at h


/-! ## Layout — memory layout with well-formedness -/


end Pmt
