import Pmt.Model

/-!
# Theorem 4.3 — Extraction correctness for `Arena.alloc`

`Arena.alloc a size align` returns `none` *exactly* when its overflow
check (`USize.add a.used size`) overflows *or* the (non-overflowing)
bumped offset `w` meets/exceeds `a.capacity` (so the success guard
`w < a.capacity` fails).

The proof case-splits on `USize.add a.used size` — the driver of the
`match` inside `Arena.alloc` — and uses `USize.add_overflow_safe` to
relate the `none` branch to the underlying `Fin (2^64)` arithmetic
(`a.used.val + size.val ≥ 2^64`). In the `some w` branch the capacity
guard `w < a.capacity` decides whether `alloc` is `some _` or `none`,
which `omega` discharges over the underlying `Nat` `.val`s.

No axioms, no placeholders. Only Lean's standard `propext`/`Classical`
infrastructure (via `omega` and `Decidable`) appears behind the scenes.
-/

namespace Pmt

set_option linter.unusedVariables false in
theorem extraction_correct (a : Arena) (size align : USize) :
    (Arena.alloc a size align).isNone ↔
      (USize.add a.used size).isNone ∨
        (∃ w, USize.add a.used size = some w ∧ w ≥ a.capacity) := by
  constructor
  · -- (→) `alloc` none ⇒ (overflow OR capacity exceeded).
    intro h
    -- Convert the first disjunct's `.isNone` to the arithmetic overflow
    -- condition via `add_overflow_safe`, so the case split leaves the
    -- arithmetic fact (rather than a substituted `none.isNone`).
    rw [USize.add_overflow_safe]
    -- Expose the `match USize.add a.used size` driver of `Arena.alloc` in h.
    unfold Arena.alloc at h
    -- Case-split on the overflow check.
    cases h2 : USize.add a.used size with
    | none =>
      -- Overflow branch: RHS first disjunct `a.used.val + size.val ≥ 2^64`.
      left
      -- Derive the arithmetic overflow fact from `h2 : … = none`.
      unfold USize.add at h2
      by_cases h3 : a.used.val + size.val < 2^64
      · -- No overflow: `USize.add` is `some _`, contradicting `h2`.
        rw [if_pos h3] at h2; simp at h2
      · -- Overflow: `h3 : ¬ (… < 2^64)` is exactly `… ≥ 2^64`.
        omega
    | some w =>
      -- Non-overflow branch: `alloc` is `none` iff the capacity guard
      -- `w < a.capacity` fails, i.e. `w ≥ a.capacity`.
      right
      -- Provide `w` as the existential witness; `some w = some w` is `rfl`;
      -- the remaining goal is `w ≥ a.capacity`.
      refine ⟨w, rfl, ?_⟩
      -- Goal: `w ≥ a.capacity`. Reduce `h` using `h2` to constrain the guard.
      simp only [h2] at h
      by_cases h3 : w < a.capacity
      · -- Guard true: `alloc` reduces to `some _`, contradicting `h`.
        rw [if_pos h3] at h; simp at h
      · -- Guard false: `h3 : ¬ (w < a.capacity)` gives `w ≥ a.capacity`.
        -- Unfold the `Fin` comparison to `Nat` `.val`s, then close via
        -- `Nat.not_lt` (Fin's `≥` is defeq to the underlying `Nat` `.val` ≤).
        have h3v : ¬ (w.val < a.capacity.val) := h3
        exact Nat.not_lt.mp h3v
  · -- (←) (overflow OR capacity exceeded) ⇒ `alloc` none.
    intro h
    cases h with
    | inl hadd =>
      -- Overflow case: `Arena.alloc` reduces to `none` via the match.
      unfold Arena.alloc
      cases h2 : USize.add a.used size with
      | none => simp [h2]
      | some w =>
        -- `hadd : (some w).isNone` is contradictory.
        rw [h2] at hadd; simp at hadd
    | inr hexist =>
      -- Capacity-exceeded: `∃ w, add = some w ∧ w ≥ a.capacity`.
      obtain ⟨w, h2, hoob⟩ := hexist
      unfold Arena.alloc
      simp only [h2]
      -- Goal: `(if w < a.capacity then some _ else none).isNone`.
      by_cases h3 : w < a.capacity
      · -- `w < a.capacity` contradicts `hoob : w ≥ a.capacity`.
        exfalso
        have h3v : w.val < a.capacity.val := h3
        have hoobv : w.val ≥ a.capacity.val := hoob
        omega
      · -- Guard false: `alloc` is `none`. `simp [h3]` rewrites the guard
        -- to `False`, reducing the `if` to its `none` branch.
        simp [h3]

end Pmt
