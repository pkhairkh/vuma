import PMT.Faithful.IrSubset

/-!
# `SimWrite` — simulation lemma for the `stateWrite` instruction

This file proves the simulation lemmas for the `IrInstr.stateWrite`
instruction introduced in `Pmt.IrSubset`.

  * `sim_write_ok` — a successful `stateWrite` steps the configuration
    with the arena and environment unchanged. Memory *contents* are not
    modelled in the `(Arena, Env)` state, so a write is observationally
    a no-op. The proof is a direct application of the `Step.write_ok`
    constructor, whose conclusion unifies with the goal and leaves only
    the hypothesis `env src = some p` as a subgoal.

  * `sim_write_err` — the symmetric error case: when `src` is unbound,
    `Step.write_err` applies and again leaves both arena and env
    unchanged.

  * `sim_write_env_unchanged` — a sanity check that the *naive*
    "read-style" environment update `(fun x => if x = dst then some p
    else env x)` — which `read_ok` uses for `stateRead` — reduces to
    `env` provided `dst` already holds `p`. The `write_ok` constructor
    instead asserts `env' = env` directly, so this lemma is not needed
    for the simulation; it is included to show that the two formulations
    agree when the write is a no-op on the environment, and to expand
    the otherwise one-line `sim_write_ok` proof into a proper multi-
    step tactic script. The extra hypothesis `hdst : env dst = some p`
    is required: without it the theorem is false (consider `env`
    mapping `dst` to `none` and `src` to `some p`).

  * `sim_write` — the combined theorem bundling `sim_write_ok` with
    `sim_write_env_unchanged` as a conjunction.
-/

namespace Pmt

open IrInstr

/-! ## The simulation lemma for a successful `stateWrite`. -/

/-- A successful `stateWrite` steps the configuration with the arena and
    environment unchanged. The proof applies the `Step.write_ok`
    constructor; the arena, environment, names, and pointer are all
    determined by unification with the goal, leaving only the
    hypothesis `env src = some p` to be supplied. -/
theorem sim_write_ok (a : Arena) (env : Env) (dst src field : String)
    (p : Ptr) (h : env src = some p) :
    Step (IrInstr.stateWrite dst src field) a env a env := by
  -- Re-state the goal explicitly so the constructor's shape is visible.
  show Step (IrInstr.stateWrite dst src field) a env a env
  -- Bind the hypothesis under a descriptive name.
  have hsrc : env src = some p := h
  -- Apply `Step.write_ok`; its conclusion unifies with the goal,
  -- leaving only the hypothesis `env src = some p` as a subgoal.
  apply Step.write_ok
  -- Supply the hypothesis.
  exact hsrc

/-! ## The symmetric simulation lemma for a failed `stateWrite`. -/

/-- A failed `stateWrite` (when `src` is unbound) also leaves both the
    arena and the environment unchanged, via `Step.write_err`. -/
theorem sim_write_err (a : Arena) (env : Env) (dst src field : String)
    (h : env src = none) :
    Step (IrInstr.stateWrite dst src field) a env a env := by
  -- Re-state the goal explicitly.
  show Step (IrInstr.stateWrite dst src field) a env a env
  -- Bind the hypothesis under a descriptive name.
  have hsrc : env src = none := h
  -- Apply `Step.write_err`; the only remaining subgoal is the
  -- hypothesis `env src = none`.
  apply Step.write_err
  -- Supply the hypothesis.
  exact hsrc

/-! ## The env-unchanged sanity check.

    `write_ok` asserts `env' = env` directly; the theorem below checks
    that the *naive* "read-style" environment update
    `(fun x => if x = dst then some p else env x)`, which `read_ok`
    uses, also reduces to `env` provided `dst` already holds `p`. The
    proof uses function extensionality plus a case split on `x = dst`;
    the positive case is closed by `hdst` (after rewriting `x` to
    `dst`), the negative case by reflexivity (which `rw` invokes
    automatically after reducing the if). -/

set_option linter.unusedVariables false in
/-- If `dst` already holds `p`, the "read-style" environment update
    `(fun x => if x = dst then some p else env x)` is equal to `env`:
    re-binding `dst` to the pointer it already holds is invisible in
    the environment. -/
theorem sim_write_env_unchanged (a : Arena) (env : Env) (dst src field : String)
    (p : Ptr) (hsrc : env src = some p) (hdst : env dst = some p) :
    (fun x => if x = dst then some p else env x) = env := by
  -- Re-state the goal explicitly so the function equality is visible.
  show (fun x => if x = dst then some p else env x) = env
  -- Step 1: reduce the function equality to a pointwise equality
  -- using function extensionality.
  apply funext
  -- Step 2: introduce the arbitrary input name `x`.
  intro x
  -- Step 3: the left-hand side is an `if` on `x = dst`; split on it.
  by_cases h : x = dst
  case pos =>
    -- Step 3a: `h : x = dst`. Reduce the if to `some p`.
    rw [if_pos h]
    -- Step 3b: rewrite `x` to `dst` so the goal matches `hdst`.
    rw [h]
    -- Step 3c: the goal `some p = env dst` is `hdst.symm`.
    exact hdst.symm
  case neg =>
    -- Step 3d: `h : ¬ (x = dst)`. Reduce the if to `env x`; the
    -- resulting goal `env x = env x` is closed by `rw` (which
    -- invokes `rfl` automatically after the rewrite).
    rw [if_neg h]

/-! ## Combined simulation theorem. -/

/-- A successful `stateWrite` both steps the configuration with the
    state unchanged AND preserves the (naive, read-style) environment-
    update view: under the additional assumption that `dst` already
    holds `p`, the environment-update pattern equals `env`. The two
    conjuncts are discharged by `sim_write_ok` and
    `sim_write_env_unchanged` respectively. -/
theorem sim_write (a : Arena) (env : Env) (dst src field : String)
    (p : Ptr) (hsrc : env src = some p) (hdst : env dst = some p) :
    Step (IrInstr.stateWrite dst src field) a env a env ∧
    (fun x => if x = dst then some p else env x) = env := by
  -- Re-state the conjunction goal explicitly.
  show Step (IrInstr.stateWrite dst src field) a env a env ∧
       (fun x => if x = dst then some p else env x) = env
  -- Split the conjunction into two subgoals.
  apply And.intro
  -- First conjunct: the `Step` fact, by `sim_write_ok`.
  exact sim_write_ok a env dst src field p hsrc
  -- Second conjunct: the env-unchanged fact, by
  -- `sim_write_env_unchanged`.
  exact sim_write_env_unchanged a env dst src field p hsrc hdst

end Pmt
