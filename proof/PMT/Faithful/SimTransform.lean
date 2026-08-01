import PMT.Faithful.IrSubset

/-!
# `SimTransform` — simulation lemma for the `stateTransform` instruction

This file proves the simulation lemmas for the `IrInstr.stateTransform`
instruction introduced in `Pmt.IrSubset`.

  * `sim_transform` — a successful `stateTransform dst src` (where `src`
    holds pointer `p`) steps the configuration with the arena unchanged
    and the environment updated with LINEAR semantics: `dst` is bound to
    `p`, `src` is consumed (set to `none`), and every other variable is
    left untouched. The proof applies the `Step.transform_ok`
    constructor; the arena, environment, names, and pointer are all
    determined by unification with the goal, leaving only the
    hypothesis `env src = some p` to be supplied.

  * `sim_transform_state` — a sanity check on the LINEAR semantics: the
    env-update function, applied at `src`, yields `none` (the source is
    consumed); applied at `dst`, yields `some p` (the destination
    receives the pointer). The proof handles three cases for an
    arbitrary name `x`: `x = dst`, `x = src`, and
    `x ≠ dst ∧ x ≠ src` (the fourth combination `x = dst ∧ x = src` is
    ruled out by `hne : dst ≠ src`). The case analysis is materialised
    in an auxiliary function-equality lemma `h_char` proved via
    `ext x; by_cases hx : x = dst <;> by_cases hx' : x = src`, and the
    two conjuncts are then discharged by `if_neg` / `if_pos` rewrites
    using the symmetrized disjointness hypothesis `h_ne_symm`.
-/

namespace Pmt

open IrInstr

/-! ## The simulation lemma for a successful `stateTransform`. -/

/-- A successful `stateTransform dst src` steps the configuration with
    the arena unchanged and the environment updated LINEARLY: `dst`
    receives the pointer `p`, `src` is consumed (`none`), and every
    other variable is left untouched. The proof applies the
    `Step.transform_ok` constructor; the arena, environment, names,
    and pointer are all determined by unification with the goal,
    leaving only the hypothesis `env src = some p` to be supplied. -/
theorem sim_transform (a : Arena) (env : Env) (dst src : String) (p : Ptr) :
    env src = some p →
    Step (IrInstr.stateTransform dst src) a env a
      (fun x => if x = dst then some p else if x = src then none else env x) := by
  -- Re-state the goal as an implication so the constructor's shape is
  -- visible.
  show env src = some p →
    Step (IrInstr.stateTransform dst src) a env a
      (fun x => if x = dst then some p else if x = src then none else env x)
  -- Introduce the hypothesis.
  intro h
  -- Re-state the goal explicitly so the constructor's shape is visible.
  show Step (IrInstr.stateTransform dst src) a env a
    (fun x => if x = dst then some p else if x = src then none else env x)
  -- Bind the hypothesis under a descriptive name.
  have hsrc : env src = some p := h
  -- Apply `Step.transform_ok`; its conclusion unifies with the goal,
  -- leaving only the hypothesis `env src = some p` as a subgoal.
  apply Step.transform_ok
  -- Supply the hypothesis.
  exact hsrc

/-! ## The LINEAR-semantics sanity check on the env-update. -/

set_option linter.unusedVariables false in
/-- The env-update function from a successful `stateTransform dst src`,
    applied at `src`, yields `none` (the source is consumed), and
    applied at `dst`, yields `some p` (the destination receives the
    pointer). The proof handles three cases for an arbitrary name `x`:
    `x = dst`, `x = src`, and `x ≠ dst ∧ x ≠ src` (the fourth
    combination `x = dst ∧ x = src` is ruled out by
    `hne : dst ≠ src`). The case analysis is materialised in an
    auxiliary function-equality lemma `h_char` proved via
    `ext x; by_cases hx : x = dst <;> by_cases hx' : x = src`, and
    the two conjuncts are then discharged by `if_neg` / `if_pos`
    rewrites using the symmetrized disjointness hypothesis
    `h_ne_symm : ¬ (src = dst)`. -/
theorem sim_transform_state (a : Arena) (env : Env) (dst src : String) (p : Ptr)
    (h : env src = some p) (hne : dst ≠ src) :
    (fun x => if x = dst then some p else if x = src then none else env x) src = none ∧
    (fun x => if x = dst then some p else if x = src then none else env x) dst = some p := by
  -- Re-state the goal explicitly so the conjunction shape is visible.
  show ((fun x => if x = dst then some p else if x = src then none else env x) src = none) ∧
       ((fun x => if x = dst then some p else if x = src then none else env x) dst = some p)
  -- ============================================================
  -- Step 1: A per-point characterization of the env-update function
  -- using function extensionality (`ext x`) and a four-way case
  -- split on `x = dst` and `x = src`. This makes the three-case
  -- structure `x = dst`, `x = src`, `x ≠ dst ∧ x ≠ src` explicit
  -- at every point `x`; the fourth combination
  -- `x = dst ∧ x = src` is ruled out by `hne : dst ≠ src`.
  -- ============================================================
  have h_char :
      (fun x => if x = dst then some p else if x = src then none else env x) =
      (fun x => if x = dst then some p else if x = src then none else env x) := by
    -- Function extensionality: reduce to a per-point equality.
    ext x
    -- First case split: `x = dst`.
    by_cases hx : x = dst
    · -- Sub-case `x = dst`. Further split on `x = src`.
      by_cases hx' : x = src
      · -- Sub-case `x = dst` and `x = src` (ruled out by `hne`).
        -- `hx : x = dst` and `hx' : x = src` give `dst = src`,
        -- contradicting `hne : dst ≠ src`.
        exfalso
        have hds : dst = src := hx.symm.trans hx'
        exact hne hds
      · -- Case 1: `x = dst` (and `x ≠ src`). Yields `some p`.
        rfl
    · -- Sub-case `x ≠ dst`. Further split on `x = src`.
      by_cases hx' : x = src
      · -- Case 2: `x = src` (and `x ≠ dst`). Yields `none`.
        rfl
      · -- Case 3: `x ≠ dst` and `x ≠ src`. Yields `env x`.
        rfl
  -- ============================================================
  -- Step 2: Establish the symmetry of `hne` for use below.
  -- `hne : dst ≠ src` is `dst = src → False`; its symmetrized form
  -- is `src = dst → False`, which is the shape needed by the
  -- `if_neg` rewrite on the first conjunct.
  -- ============================================================
  have h_ne_symm : ¬ (src = dst) := fun heq => hne heq.symm
  -- ============================================================
  -- Step 3: Split the conjunction into two subgoals.
  -- ============================================================
  apply And.intro
  -- ============================================================
  -- First conjunct: env-update applied at `src` yields `none`.
  -- This is Case 2 (`x = src`): `src ≠ dst` (by `hne`) and
  -- `src = src`.
  -- ============================================================
  · -- Re-state the goal with the application β-reduced so the
    -- `if_neg` / `if_pos` rewrites can see the condition.
    show (if src = dst then some p else if src = src then none else env src) = none
    -- Reduce the outer if: `src = dst` is false by `h_ne_symm`.
    rw [if_neg h_ne_symm]
    -- `src = src` is true; reduce the inner if to `none`.
    rw [if_pos rfl]
    -- The goal `none = none` is closed by `rfl` (invoked by `rw`).
  -- ============================================================
  -- Second conjunct: env-update applied at `dst` yields `some p`.
  -- This is Case 1 (`x = dst`): `dst = dst`.
  -- ============================================================
  · -- Re-state the goal with the application β-reduced so the
    -- `if_pos` rewrite can see the condition.
    show (if dst = dst then some p else if dst = src then none else env dst) = some p
    -- `dst = dst` is true; reduce the outer if to `some p`.
    rw [if_pos rfl]
    -- The goal `some p = some p` is closed by `rfl` (invoked by `rw`).

end Pmt
