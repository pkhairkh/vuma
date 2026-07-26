import PMT.Soundness

/-! ## Helper Lemmas — small utilities for List/Nat reasoning.

This module collects small, broadly-useful lemmas about `Nat` arithmetic
and `List.length` that are used across the proof library. They are
trivially provable (`rfl` / `omega`) but centralised here so callers can
`import PMT.HelperLemmas` rather than re-deriving them at each call site.

Note: an earlier draft of this file included a
`filter_length_one_unique` theorem. That theorem as originally stated
was *false*: the hypothesis `∀ y ∈ l, y == x = true → y = x` only
guarantees BEq-uniqueness, not list-count uniqueness. The counterexample
is `l = [x, x]`, where the filter length is 2, not 1. To avoid leaving
a `sorry`, that theorem has been omitted; a correct version would
require an additional `List.Nodup l` (or `l.count x = 1`) hypothesis
and is left for a future wave.
-/

namespace PMT

/-- Nat.add_right_comm: `a + b + c = a + c + b`. -/
theorem nat_add_right_comm (a b c : Nat) : a + b + c = a + c + b := by
  omega

/-- List.length_cons: `(x :: l).length = l.length + 1`. -/
theorem list_length_cons {α : Type} (x : α) (l : List α) :
    (x :: l).length = l.length + 1 := by
  rfl

/-- Empty list has length 0. -/
theorem list_length_nil {α : Type} : ([] : List α).length = 0 := by
  rfl

end PMT
