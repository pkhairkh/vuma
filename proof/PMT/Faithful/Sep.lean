import Init.Prelude

/-!
# Minimal separation logic framework

A from-scratch separation logic over a partial heap model
(`HeapModel := Nat → Option Nat`), with a disjoint-domains separating
conjunction, an empty heap, points-to, and fractional points-to.

No Mathlib, no Iris — everything is built on `Init.Prelude`.
-/

namespace Pmt

/-- Heap model: a partial function from addresses (`Nat`) to values (`Nat`). -/
abbrev HeapModel := Nat → Option Nat

/-- Separating conjunction on heaps.

    `HeapModel.sep h1 h2` holds exactly when `h1` and `h2` have *disjoint
    domains*: at every address `x`, `h1 x` is `none` iff `h2 x` is *not*
    `none` (the domains partition the address space between the two heaps),
    and moreover whenever `h1 x = some v` then `h2 x = none` (so the two
    heaps never both contribute a value at the same address).

    The second conjunct is the "no overlap" half of disjointness; the first
    conjunct pins down the domain partition. -/
def HeapModel.sep (h1 h2 : HeapModel) : Prop :=
  ∀ x, (h1 x = none ↔ h2 x ≠ none) ∧
        (∀ v, h1 x = some v → h2 x = none)

/-- The empty heap: undefined at every address. -/
def HeapModel.emp : HeapModel := fun _ => none

set_option linter.unusedVariables false in
/-- Points-to: a single resource cell `addr ↦ val`, carrying a heap
    interpretation `sep : HeapModel → Prop`.

    (The spec's skeleton wrote `structure Ptsto … : Prop where
    sep : HeapModel → Prop`, but a `Prop`-valued structure may only carry
    proof-valued fields — `HeapModel → Prop` lives in `Type`, so Lean
    refuses to generate the projection. We make `Ptsto` a `Type`-valued
    record instead, the minimal change that preserves the template's shape
    — a `Ptsto addr val` package with a single `sep` field — while still
    compiling under `Init.Prelude` alone.) -/
structure Ptsto (addr : Nat) (val : Nat) where
  sep : HeapModel → Prop

/-- A minimal fraction: numerator over (positive) denominator. -/
structure Rat where
  num : Int
  den : Nat

set_option linter.unusedVariables false in
/-- Fractional points-to: a points-to resource with a fractional
    permission `q`, well-formed iff `valid : q.num > 0 ∧ q.den > 0`. -/
structure FracPtsto (addr : Nat) (q : Rat) (val : Nat) where
  valid : q.num > 0 ∧ q.den > 0

/-! ## Theorems -/

set_option linter.unusedVariables false in
/-- Disjoint-domains elimination: if `h1` and `h2` separate, then any
    address at which `h1` is defined is empty in `h2`.

    Proof sketch: unpack the disjoint-domains condition at `x` to get the
    "no overlap" half `∀ v, h1 x = some v → h2 x = none`, then case-split
    on `h1 x`. The `none` branch contradicts the hypothesis `h1 x ≠ none`;
    the `some v` branch feeds the equation `h1 x = some v` straight into
    the no-overlap half to conclude `h2 x = none`. Pure intuitionistic
    reasoning about the disjoint-domains definition — no axioms, no
    classical steps. -/
theorem sep_disjoint (h1 h2 : HeapModel) (h : HeapModel.sep h1 h2) (x : Nat) :
    h1 x ≠ none → h2 x = none := by
  intro hne
  -- Unpack the disjoint-domains condition at address `x`.
  have hx : (h1 x = none ↔ h2 x ≠ none) ∧
            (∀ v, h1 x = some v → h2 x = none) := h x
  obtain ⟨_, h2nd⟩ := hx
  -- `h1 x` is either `none` (contradicts `hne`) or `some v` (use `h2nd`).
  cases h1x : h1 x with
  | none => contradiction
  | some v => exact h2nd v h1x

set_option linter.unusedVariables false in
/-- Fractional split (skeleton): combining two fractional points-to
    resources whose fractions sum to `1` is, in this minimal skeleton,
    vacuously well-formed — the statement's conclusion is `True`, so the
    two `FracPtsto` hypotheses are simply discarded. The fraction-sum
    hypothesis `q1.num + q2.num = 1` is the usual "permissions recombine
    to a full permission" side-condition; a full development would use it
    to discharge the well-formedness of the combined resource, but the
    skeleton here only asserts the trivial post-condition. -/
theorem frac_split (addr val : Nat) (q1 q2 : Rat) (hq : q1.num + q2.num = 1) :
    FracPtsto addr q1 val → FracPtsto addr q2 val → True := by
  intro _ _
  trivial

end Pmt
