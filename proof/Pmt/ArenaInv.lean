import Init.Prelude

/-!
# ArenaInv — the `[cap_bnd]` invariant for the arena allocator

This file establishes three small facts about a simplified `CapBnd`
(capacity-bound) resource bundle, mirroring the Iris-style ghost-state
invariant used in the Pmt arena allocator:

1. `cap_bnd_init`         — the invariant holds initially (`used = 0`).
2. `cap_bnd_alloc`        — allocating `size` bytes preserves the invariant.
3. `cap_bnd_never_exceeds`— the invariant really does bound `used` by `cap`.

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

set_option linter.unusedVariables false in
/-- The invariant holds initially when the arena is empty. -/
theorem cap_bnd_init (cap : Nat) (hcap : cap > 0) :
    CapBnd 0 cap where
  auth  := ⟨⟩
  agree := ⟨⟩
  bound := Nat.zero_le cap
  res   := ⟨hcap⟩

set_option linter.unusedVariables false in
/-- Allocating `size` bytes preserves the invariant, provided the new
    `used + size` stays within `cap`. The authoritative/agreement ghosts
    are updated to the new `used`; the underlying `ArenaRes` is reused. -/
theorem cap_bnd_alloc (used cap size : Nat)
    (h : CapBnd used cap) (hsize : used + size ≤ cap) :
    CapBnd (used + size) cap where
  auth  := ⟨⟩
  agree := ⟨⟩
  bound := hsize
  res   := h.res

set_option linter.unusedVariables false in
/-- The invariant really does bound `used` by `cap`. -/
theorem cap_bnd_never_exceeds (used cap : Nat)
    (h : CapBnd used cap) : used ≤ cap := by
  exact h.bound

end Pmt
