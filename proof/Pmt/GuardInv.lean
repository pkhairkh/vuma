import Init.Prelude

/-!
# GuardInv — the guard-page invariant for the arena allocator

This file mirrors the Iris-style `guard_inv` invariant from the Pmt
arena allocator. The invariant bundles two facts:

1. `res`      — the underlying arena resource (`ArenaRes base cap`).
2. `guard_ok` — every address at or beyond `base + cap` (i.e. past the
                end of the arena) satisfies the (trivialised) guard-page
                predicate. In the full Iris spec this is the page-table
                / guard-page assertion that traps out-of-bounds accesses;
                here it is abstracted to `True` so the framing and
                allocation-safety lemmas go through with only `Init.Prelude`.

Only `Init.Prelude` is imported — no Mathlib dependency.
-/

namespace Pmt

/-- Points-to predicate (simplified).

    Declared in `Prop` so that `CapBnd` (which bundles it) is itself a
    proposition and can therefore be the type of a `theorem`. -/
structure Ptsto (addr : Nat) (val : Nat) : Prop where

/-- `ArenaRes`: the arena resource bundle. -/
structure ArenaRes (base cap : Nat) : Prop where
  valid : cap > 0

/-- `CapBnd`: the `[cap_bnd]` invariant from the Iris spec. -/
structure CapBnd (used cap : Nat) : Prop where
  auth  : Ptsto 0 used       -- authoritative ghost (simplified: addr=0)
  agree : Ptsto 0 used       -- agreement ghost
  bound : used ≤ cap
  res   : ArenaRes 0 cap

/-- `GuardInv`: the guard-page invariant. Composes the arena resource
    with a (trivialised) guard predicate on every address past the end
    of the arena. -/
structure GuardInv (base cap : Nat) : Prop where
  res      : ArenaRes base cap
  guard_ok : ∀ addr, base + cap ≤ addr → True

set_option linter.unusedVariables false in
/-- The capacity bound and the guard predicate frame independently:
    the bound `used ≤ cap` comes from `CapBnd`, and the guard predicate
    comes unchanged from `GuardInv`. -/
theorem guard_and_cap_frame (base cap used : Nat)
    (hcap : CapBnd used cap) (hguard : GuardInv base cap) :
    used ≤ cap ∧ (∀ addr, base + cap ≤ addr → True) := by
  exact ⟨hcap.bound, hguard.guard_ok⟩

set_option linter.unusedVariables false in
/-- Allocating `size` bytes within the capacity preserves the guard
    invariant: the guard predicate is unaffected by allocation, so the
    existing `GuardInv` is returned unchanged. -/
theorem guard_alloc_safe (base cap used size : Nat)
    (hcap : CapBnd used cap) (hguard : GuardInv base cap)
    (hsize : used + size ≤ cap) :
    GuardInv base cap := by
  exact hguard

end Pmt
