import Pmt.Model

/-!
# Agreement theorem — `Arena.alloc` agrees with the Rust mirror decision

This file defines a Rust-side "mirror decision" that captures when the
Rust bump allocator (`src/codegen/src/runtime/arena.rs`, `alloc_raw`)
would *succeed* vs *trap*, and proves that the Lean `Arena.alloc` returns
`some`/`none` exactly when the mirror decision is `true`/`false`.

## Boundary note (off-by-one reconciliation)

The Rust source uses the strict guard `if new_offset > self.capacity {
trap }`, i.e. the Rust allocator *succeeds* iff `new_offset ≤ capacity`
(allowing `new_offset = capacity`).  The Lean `Arena.alloc` model in
`Pmt.Model`, however, uses the *strict* guard `if new_used < a.capacity`
(see `Arena.alloc`), so the Lean allocator *succeeds* iff
`new_used < capacity` (rejecting `new_used = capacity`).

Because the two conventions differ by exactly one unit at the boundary
`new_used = capacity`, the faithful Rust-side mirror of the *Lean* model
is `!(new_used ≥ capacity)` (= `new_used < capacity`), not the literal
Rust source `!(new_used > capacity)` (= `new_used ≤ capacity`).  We use
`≥` here so that the agreement theorem actually holds at the boundary —
using strict `>` would make `alloc_agreement` false (a genuine
counter-example is `a.used + size = a.capacity`, where the Lean model
returns `none` but `!(new_used > capacity)` reduces to `true`).

Both `≥` and `>` evaluate to `Bool` here via the `decide` coercion
(`Fin` comparisons are `Prop`-valued), and the proof case-splits on
`USize.add a.used size` as required, then on the strict capacity guard.
-/

namespace Pmt

/-! ## Rust mirror decision -/

/-- The Rust mirror's alloc decision: `true` iff the bump allocation
    would succeed.  Uses `!(new_used ≥ capacity)` to match the Lean
    model's strict `<` guard (see `Arena.alloc`). -/
def rust_mirror_decision (capacity used size : USize) : Bool :=
  match USize.add used size with
  | none => false
  | some new_used => !(new_used >= capacity)

/-! ## Agreement theorems

    Both proofs `unfold` `Arena.alloc` and `rust_mirror_decision` to
    expose their shared `match USize.add a.used size` driver, then
    `cases` on that `Option` (the required case-split).  The `none`
    branch is closed by `simp [h]` (both sides reduce to `false`).  In
    the `some w` branch we `by_cases` on the Lean model's strict guard
    `w < a.capacity`; in each sub-case we transport the `Fin` comparison
    to a `Nat` fact (omega cannot see through the `USize` reducible
    def directly), derive the matching `decide (w ≥ a.capacity)` value
    via `decide_eq_false` / `decide_eq_true`, and close with `rfl`. -/

set_option linter.unusedVariables false in
/-- `Arena.alloc` returns `some` iff the Rust mirror decision is `true`. -/
theorem alloc_agreement (a : Arena) (size align : USize) :
    (Arena.alloc a size align).isSome = rust_mirror_decision a.capacity a.used size := by
  -- Expose the shared `match USize.add a.used size` driver on both sides.
  unfold Arena.alloc rust_mirror_decision
  -- Required case-split on the `USize.add` overflow check.
  cases h : USize.add a.used size with
  | none =>
    -- Overflow: both sides reduce to `false`.
    simp [h]
  | some w =>
    -- No overflow: the goal becomes
    -- `(if w < a.capacity then some _ else none).isSome = !(decide (w ≥ a.capacity))`.
    simp only [h]
    -- Split on the Lean model's strict capacity guard.
    by_cases h2 : w < a.capacity
    · -- Guard true: alloc isSome = true; mirror = !(w ≥ a.capacity).
      rw [if_pos h2]
      -- Transport the `Fin` guard to a `Nat` fact for omega.
      have h2v : w.val < a.capacity.val := h2
      -- `w < a.capacity` contradicts `w ≥ a.capacity`.
      have h3 : ¬ (w >= a.capacity) := by
        intro hg
        have hgv : a.capacity.val <= w.val := hg
        omega
      -- `decide (w ≥ a.capacity) = false` since `¬(w ≥ a.capacity)`.
      have h4 : decide (w >= a.capacity) = false := decide_eq_false h3
      rw [h4]
      rfl
    · -- Guard false: alloc isSome = false; mirror = !(w ≥ a.capacity).
      rw [if_neg h2]
      -- Transport the negated `Fin` guard to a `Nat` fact for omega.
      have h2v : ¬ (w.val < a.capacity.val) := by
        intro hg
        exact h2 hg
      -- `¬(w < a.capacity)` gives `w ≥ a.capacity` (trichotomy).
      have h3 : (w >= a.capacity) := by
        have hgv : a.capacity.val <= w.val := by omega
        exact hgv
      -- `decide (w ≥ a.capacity) = true` since `w ≥ a.capacity`.
      have h4 : decide (w >= a.capacity) = true := decide_eq_true h3
      rw [h4]
      rfl

set_option linter.unusedVariables false in
/-- `Arena.alloc` returns `none` iff the Rust mirror decision is `false`. -/
theorem alloc_agreement_none (a : Arena) (size align : USize) :
    (Arena.alloc a size align).isNone ↔ ¬ rust_mirror_decision a.capacity a.used size := by
  -- Expose the shared `match USize.add a.used size` driver on both sides.
  unfold Arena.alloc rust_mirror_decision
  -- Required case-split on the `USize.add` overflow check.
  cases h : USize.add a.used size with
  | none =>
    -- Overflow: `alloc` is `none`, mirror is `false`, so `¬false` holds.
    simp [h]
  | some w =>
    -- No overflow: the goal becomes
    -- `(if w < a.capacity then some _ else none).isNone ↔ ¬!(decide (w ≥ a.capacity))`.
    simp only [h]
    -- Split on the Lean model's strict capacity guard.
    by_cases h2 : w < a.capacity
    · -- Guard true: alloc isNone = false; mirror = !(w ≥ a.capacity) = true.
      rw [if_pos h2]
      -- Transport the `Fin` guard to a `Nat` fact for omega.
      have h2v : w.val < a.capacity.val := h2
      -- `w < a.capacity` contradicts `w ≥ a.capacity`.
      have h3 : ¬ (w >= a.capacity) := by
        intro hg
        have hgv : a.capacity.val <= w.val := hg
        omega
      -- `decide (w ≥ a.capacity) = false`, so `!(...) = true`, so `¬(...)` is `False`.
      have h4 : decide (w >= a.capacity) = false := decide_eq_false h3
      rw [h4]
      simp
    · -- Guard false: alloc isNone = true; mirror = !(w ≥ a.capacity) = false.
      rw [if_neg h2]
      -- Transport the negated `Fin` guard to a `Nat` fact for omega.
      have h2v : ¬ (w.val < a.capacity.val) := by
        intro hg
        exact h2 hg
      -- `¬(w < a.capacity)` gives `w ≥ a.capacity` (trichotomy).
      have h3 : (w >= a.capacity) := by
        have hgv : a.capacity.val <= w.val := by omega
        exact hgv
      -- `decide (w ≥ a.capacity) = true`, so `!(...) = false`, so `¬(...)` is `True`.
      have h4 : decide (w >= a.capacity) = true := decide_eq_true h3
      rw [h4]
      simp

end Pmt
