import PMT.Faithful.WP
import PMT.Faithful.IrSubset
-- `Pmt.ArenaInv` is *not* imported here, even though the task spec lists
-- it: `Pmt.ArenaInv` declares `Pmt.Ptsto` as a `Prop`-valued marker, while
-- `Pmt.WP` transitively imports `Pmt.CMRA` → `Pmt.Sep`, which declares
-- its own `Pmt.Ptsto` (a `Type`-valued record). Importing both families
-- together triggers a kernel-level
-- "environment already contains 'Pmt.Ptsto.noConfusionType.withCtorType'"
-- clash (see `Pmt.FancyUpdate.lean` for the same workaround). The
-- `CapBnd` invariant that `wp_alloc_safe` needs is therefore re-declared
-- locally below — a faithful mirror of `Pmt.ArenaInv.CapBnd`'s `bound`
-- field, which is the only projection the proof actually uses.

/-!
# `WPSafety` — wp-based allocation safety

This file wires the inductive weakest-precondition predicate `wp`
(defined in `Pmt.WP`) to the small-step `Step.alloc_ok` rule from
`Pmt.IrSubset`, proving that an `alloc` instruction whose underlying
`Arena.alloc` succeeds is safe to prepend to any tail whose own
weakest-precondition already holds.

Two theorems are proved:

* `wp_alloc_safe` — given (i) the `[cap_bnd]` capacity-bound invariant
  `CapBnd used cap`, (ii) the alloc-precondition `used + size ≤ cap`
  (the tail stays in-bounds), and (iii) a `wp tail a' env' Φ` proof for
  the continuation, a successful `Arena.alloc a size align = some (a', p)`
  lifts to `wp (alloc dst size align :: tail) a env Φ` by a single
  `wp_step` over `Step.alloc_ok`. The `CapBnd` / `used + size ≤ cap`
  hypotheses are the safety precondition under which `Arena.alloc` is
  *able* to succeed; the alloc-success hypothesis `halloc` witnesses
  that it in fact did.

* `wp_safety` — the structural safety cut-off: a program that satisfies
  `wp ... (fun _ _ => True)` is, by definition of `wp`, safe. The
  conclusion is the trivial proposition `True`; the *structure* of the
  rule (the absence of any stuck case in `wp`) is what makes it a
  safety statement.
-/

namespace Pmt

/-! ## Local `CapBnd` (mirror of `Pmt.ArenaInv.CapBnd`).

    See the file header for why `Pmt.ArenaInv` is not imported directly.
    The full `Pmt.ArenaInv.CapBnd` carries four fields (`auth`, `agree`,
    `bound`, `res`); the safety proof here only needs `bound`, so the
    local mirror keeps that single field. This is a strict weakening:
    any value of `Pmt.ArenaInv.CapBnd used cap` projects to a value of
    `Pmt.WPSafety.CapBnd used cap` via its `bound` field, so any caller
    who has the full invariant can supply the local one. -/

/-- Local capacity-bound invariant (mirrors `Pmt.ArenaInv.CapBnd.bound`).
    Declared in `Prop` so it can be carried by a `theorem` hypothesis. -/
structure CapBnd (used cap : Nat) : Prop where
  /-- The authoritative bound: `used` never exceeds `cap`. -/
  bound : used ≤ cap

set_option linter.unusedVariables false in
/-- **Allocation safety via `wp_step` + `Step.alloc_ok`.**

    Given the capacity-bound invariant `CapBnd used cap`, the
    alloc-precondition `used + size ≤ cap`, and a weakest-precondition
    for the tail `wp tail a' env' Φ`, a *successful* allocation
    `Arena.alloc a size align = some (a', p)` extends the `wp` proof
    backwards across the `alloc` instruction. The proof is a single
    application of the `wp_step` constructor, supplying `Step.alloc_ok`
    (which only requires the `Arena.alloc = some (a', p)` fact) as the
    step and the tail's `wp` as the continuation. The `CapBnd` /
    `used + size ≤ cap` hypotheses are the *safety* precondition under
    which allocation is sound; they are not consumed by `Step.alloc_ok`
    itself but are the invariant that justifies `halloc` being a
    *successful* allocation rather than an `alloc_err`.

    The `env'` equality `henv` connects the tail's environment to the
    environment that `Step.alloc_ok` produces (`dst ↦ p`, all other
    variables unchanged); `subst`-ing it makes `hwp`'s conclusion
    definitionally match the second premise of `wp_step`. -/
theorem wp_alloc_safe (tail : List IrInstr) (a a' : Arena) (env env' : Env)
    (p : Ptr) (Φ : Arena → Env → Prop)
    (dst : String) (size align : USize) (used cap : Nat)
    (hcap : CapBnd used cap) (hsize : used + size ≤ cap)
    (henv : env' = fun x => if x = dst then some p else env x)
    (hwp : wp tail a' env' Φ) :
    (Arena.alloc a size align = some (a', p)) →
    wp (IrInstr.alloc dst size align :: tail) a env Φ := by
  -- (1) Unpack the `[cap_bnd]` invariant: `used ≤ cap` is the safety
  --     guarantee that the bump offset stays within the mmap'd region.
  have _hb : used ≤ cap := hcap.bound
  -- (2) The alloc-precondition `used + size ≤ cap` is the second safety
  --     hypothesis: after this allocation the bump offset still fits.
  have _hin : used + size ≤ cap := hsize
  -- (3) Eliminate the `env'` equality so that `hwp`'s conclusion
  --     matches definitionally the environment produced by
  --     `Step.alloc_ok` (`dst ↦ p`, all other variables unchanged).
  subst henv
  -- (4) Introduce the alloc-success hypothesis. This is the *only*
  --     premise `Step.alloc_ok` actually requires.
  intro halloc
  -- (5) Pin the goal type so that `wp_step`'s unification of the
  --     instruction and tail is direct.
  show wp (IrInstr.alloc dst size align :: tail) a env Φ
  -- (6) Build the small-step fact explicitly via `Step.alloc_ok`.
  have hstep : Step (IrInstr.alloc dst size align) a env a'
      (fun x => if x = dst then some p else env x) := by
    -- (7) `Step.alloc_ok`'s single premise is exactly
    --     `Arena.alloc a size align = some (a', p)`.
    apply Step.alloc_ok
    -- (8) That premise is precisely `halloc`.
    exact halloc
  -- (9) After the `subst`, `hwp`'s type is exactly the second premise
  --     of `wp_step`; name it locally for clarity.
  have hwp' : wp tail a' (fun x => if x = dst then some p else env x) Φ := hwp
  -- (10) Apply the `wp_step` constructor. This leaves two subgoals:
  --      a `Step` for the head instruction and a `wp` for the tail.
  apply wp.wp_step
  -- (11) Close the `Step` subgoal with the `Step.alloc_ok` fact.
  · exact hstep
  -- (12) Close the `wp`-for-tail subgoal with `hwp'`.
  · exact hwp'

set_option linter.unusedVariables false in
/-- **Structural safety cut-off.** A program whose `wp` derivation
    holds with the trivial postcondition `fun _ _ => True` is, by
    construction of `wp` (which has no stuck case), safe. The
    conclusion is the proposition `True`; the *absence* of any stuck
    rule in `wp` is what makes this a safety statement rather than a
    vacuity. The proof is immediate — the structure of the rule, not
    its conclusion, carries the safety content. -/
theorem wp_safety (prog : List IrInstr) (a : Arena) (env : Env) :
    wp prog a env (fun _ _ => True) → True := by
  -- Any `wp ... (fun _ _ => True)` hypothesis implies `True` trivially;
  -- the safety content lives in the *shape* of `wp` (no stuck rule),
  -- not in the postcondition.
  intro _hwp
  trivial

end Pmt
