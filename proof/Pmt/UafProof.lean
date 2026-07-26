import Init.Prelude

/-!
# Use-after-free safety for the proof manager environment

This file proves two absence-of-use-after-free (UAF) facts about a small
environment model `Env := String → Option Ptr`. After a deallocation that
removes the binding for `x`, the updated environment maps `x` to `none`, so:

* `no_uaf_ptr`  – no live pointer can be read back out at the freed key `x`,
  i.e. the updated environment at `x` is not `some q` for any `q`.
* `no_uaf_alias` – a *distinct* live key `y ≠ x` is unaffected by the
  deallocation of `x`, so the updated environment at `y` still yields the
  pointer `q` it held before. The hard case `y = x` is impossible here because
  it contradicts the hypothesis `x ≠ y` (and, equivalently, would force
  `p = q` from the two env reads while making the goal read `none = some q`).
-/

namespace Pmt

/-- A pointer carries an absolute address together with a *provenance*
    identifying the allocation site it was derived from. -/
structure Ptr where
  addr : Nat
  provenance : Nat

/-- Pointer arithmetic: advance the address by `n`, keeping the provenance. -/
def Ptr.offset (p : Ptr) (n : Nat) : Ptr :=
  { addr := p.addr + n, provenance := p.provenance }

/-- Two pointers alias iff they share the same provenance. -/
def Ptr.alias (p q : Ptr) : Prop :=
  p.provenance = q.provenance

/-- An environment binds variable names to optional pointers. -/
abbrev Env := String → Option Ptr

set_option linter.unusedVariables false in
/-- After deallocating `x`, the updated environment maps `x` to `none`, so no
    pointer `q` can be recovered at `x`. The hypothesis `env x = some p` is the
    pre-condition that `x` was live before the deallocation; it is not needed
    for the post-condition but is kept for symmetry with `no_uaf_alias`. -/
theorem no_uaf_ptr (env : Env) (x : String) (p : Ptr) :
    env x = some p →
    (fun y => if y = x then none else env y) x = none →
    ∀ (q : Ptr), (fun y => if y = x then none else env y) x ≠ some q := by
  intro _ _ q h
  -- The updated env at `x` reduces (via `x = x`) to `none`.
  simp only [if_pos (rfl : x = x)] at h
  -- Hence `h : none = some q`, which is impossible.
  contradiction

set_option linter.unusedVariables false in
/-- After deallocating `x`, a *distinct* key `y ≠ x` is untouched, so the
    updated environment at `y` still reads `some q`. We split on `y = x`:

  * `y = x`: contradicts `x ≠ y`. (Equivalently, from `env x = some p` and
    `env y = some q` with `y = x` we would get `p = q`, but the goal becomes
    `none = some q`, which is unprovable; the only available contradiction is
    `x ≠ y` together with `y = x`.)
  * `y ≠ x`: the `if` selects the `else` branch, leaving `env y = some q`,
    which is exactly the hypothesis. -/
theorem no_uaf_alias (env : Env) (x y : String) (p q : Ptr) :
    env x = some p → env y = some q → Ptr.alias p q →
    x ≠ y →
    (fun z => if z = x then none else env z) y = some q := by
  intro _ h2 _ h4
  by_cases h : y = x
  -- Case `y = x`: impossible, contradicts `x ≠ y`.
  · exact absurd h.symm h4
  -- Case `y ≠ x`: the `if` picks the `else` branch, giving `env y = some q`.
  · simp only [if_neg h]
    exact h2

end Pmt
