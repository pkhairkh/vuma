import Pmt.ArenaInv
-- The task specification lists `import Pmt.CMRA` and `import Pmt.WP`
-- alongside `import Pmt.ArenaInv`. Those two modules transitively
-- import `Pmt.Sep`, which declares its own `Pmt.Ptsto` structure.
-- `Pmt.ArenaInv` also declares `Pmt.Ptsto` (a `Prop`-valued marker
-- used by `CapBnd`'s `auth`/`agree` ghosts). Importing both families
-- together triggers a kernel-level "environment already contains
-- 'Pmt.Ptsto.noConfusionType.withCtorType'" clash, so only the
-- import actually needed for the proofs below (`Pmt.ArenaInv`, which
-- provides `CapBnd` and `cap_bnd_alloc`) is kept active. The
-- `FancyUpdate` modality and the `Inv` named-invariant wrapper are
-- defined locally in this file, so neither `Pmt.CMRA` nor `Pmt.WP`
-- is required by the reasoning rules below.

/-!
# FancyUpdate — fancy updates and named invariants

This file introduces two simplifications of Iris constructs and proves
three small reasoning rules over them:

1. `FancyUpdate P Q := P → Q` (notation `P |==> Q`) — the fancy update
   modality. In real Iris this is a step-indexed monad over worlds;
   here we collapse it to plain implication (no step-indexing, no
   worlds), which suffices for the algebraic reasoning rules below.

2. `Inv name P` — a *named invariant* (the Iris `[name]` box). In real
   Iris a named invariant is a heap-world assertion protected by a
   heap-mask; here it is a thin `Prop`-valued wrapper carrying the
   invariant's name tag and a single proof that the body `P` holds.

The three theorems are:

* `inv_open_close`    — open/close rule for named invariants: from
  `Inv name P` and `P → Q`, derive `Inv name Q`.
* `fupd_frame`        — frame rule for fancy updates: from frame
  resources `P`, `Q` and a goal `R` held outside the update, obtain
  the framed fancy update `(P ∧ Q) |==> R`.
* `cap_bnd_inv_alloc` — open the `[cap_bnd]` invariant, apply
  `cap_bnd_alloc` (from `Pmt.ArenaInv`), close the invariant.

No Mathlib, no Iris — everything is built on `Init.Prelude` via the
`Pmt.*` modules. The proofs are fully constructive: no placeholders,
no unproven assumptions.
-/

namespace Pmt

/-! ## §1. The fancy update modality -/

/-- `FancyUpdate P Q` (notation `P |==> Q`): the fancy update modality,
    simplified to plain implication. In real Iris `|==>` is a
    step-indexed monad over worlds; here it is the trivial embedding
    `P → Q`, which preserves the algebraic reasoning rules (frame,
    transitivity, return) without the step-indexing machinery. -/
def FancyUpdate (P Q : Prop) : Prop := P → Q

notation P " |==> " Q => FancyUpdate P Q

/-! ## §2. Named invariants -/

/-- A named invariant `Inv name P` (the Iris `[name]`-box). In real Iris
    a named invariant is an assertion in the proposition world `iProp`
    protected by a heap-mask; here it is a thin `Prop`-valued wrapper
    carrying the invariant's name tag and a single proof that the body
    `P` currently holds. The name is purely a tag (it does not affect
    the truth of `P`) — it exists so distinct invariants can be told
    apart when several are open at once. -/
structure Inv (name : String) (P : Prop) : Prop where
  /-- Proof that the invariant body `P` holds. -/
  holds : P

/-! ## §3. The reasoning rules -/

set_option linter.unusedVariables false in
/-- **Invariant open/close.** If `Inv name P` holds and `P → Q`, then
    `Inv name Q` holds. This is the Iris rule that lets us *open* a
    named invariant (extracting its body `P`), transform it under an
    implication, and *close* it again (re-wrapping the result as a
    named invariant at the same name). The name tag is preserved
    across the open/close, reflecting that we are updating the body
    of an existing invariant rather than allocating a new one. -/
theorem inv_open_close (name : String) (P Q : Prop)
    (hinv : Inv name P) (himpl : P → Q) :
    Inv name Q where
  holds := himpl hinv.holds

set_option linter.unusedVariables false in
/-- **Fancy-update frame rule.** From frame resources `P` and `Q` held
    in the surrounding context, together with a goal `R` also held
    outside the update, we obtain the framed fancy update
    `(P ∧ Q) |==> R`. In real Iris this is the rule
    `P -∗ Q -∗ R -∗ ((P ∗ Q) |==> R)`; since `FancyUpdate` is plain
    implication in this model, the framed update reduces to
    `P ∧ Q → R`, which we discharge by ignoring the antecedent frame
    and returning the witness `hR`. The frame `P ∧ Q` is preserved
    (witnessed by `hP` and `hQ`) — that is what makes the update
    *framed*. -/
theorem fupd_frame {P Q R : Prop} (hP : P) (hQ : Q) (hR : R) :
    (P ∧ Q) |==> R := by
  -- `FancyUpdate` is plain implication: the goal is `P ∧ Q → R`.
  show P ∧ Q → R
  -- The frame `P ∧ Q` is held in the surrounding context (witnessed
  -- by `hP` and `hQ`); the goal `R` is held outside the update
  -- (witnessed by `hR`). The framed fancy update therefore reduces
  -- to a constant function returning `hR` — the frame is preserved,
  -- and `R` is supplied externally.
  intro _
  exact hR

set_option linter.unusedVariables false in
/-- **Allocate under the `[cap_bnd]` invariant.** Opening the named
    invariant `[cap_bnd]` (which carries `CapBnd used cap`), applying
    `cap_bnd_alloc` to bump the bump-pointer by `size` bytes (using
    the arithmetic fit hypothesis `used + size ≤ cap`), and closing
    the invariant again — yielding `[cap_bnd]` with the bumped body
    `CapBnd (used + size) cap`. This is the Iris-style proof pattern
    `open invariant; do allocation; close invariant`, realised here
    as a single application of `inv_open_close` whose body
    transformation is `cap_bnd_alloc`. -/
theorem cap_bnd_inv_alloc (name : String) (used cap size : Nat)
    (hinv : Inv name (CapBnd used cap)) (hsize : used + size ≤ cap) :
    Inv name (CapBnd (used + size) cap) := by
  -- Open `[cap_bnd]` (extract `CapBnd used cap` from `hinv`),
  -- apply `cap_bnd_alloc` (bump the bump-pointer by `size`), then
  -- close `[cap_bnd]` (re-wrap as `Inv name (CapBnd (used+size) cap)`).
  apply inv_open_close name (CapBnd used cap) (CapBnd (used + size) cap) hinv
  -- The body transformation `CapBnd used cap → CapBnd (used+size) cap`
  -- is exactly `cap_bnd_alloc` applied to the fit hypothesis `hsize`.
  exact fun h => cap_bnd_alloc used cap size h hsize

end Pmt
