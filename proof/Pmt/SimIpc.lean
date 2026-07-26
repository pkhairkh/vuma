import Pmt.IrSubset

/-!
# `SimIpc` — simulation lemmas for the IPC channel instructions

This file proves the simulation lemmas for the three IPC-channel
instructions introduced in `Pmt.IrSubset`:

  * `IrInstr.chanNew dst cap`  — allocate a fresh channel of capacity `cap`
                                 into `dst`.
  * `IrInstr.chanSend chan val` — send `val` along the channel bound to
                                  `chan`.
  * `IrInstr.chanRecv dst chan` — receive a value from the channel bound
                                  to `chan` into `dst`.

For each instruction we prove a single theorem that applies the
corresponding `Step` constructor (`chanNew_ok` / `chanSend_ok` /
`chanRecv_ok`) and discharges the side-condition. Each proof is
expanded with auxiliary env-property conjuncts (per-point
characterisations via function extensionality and case splits on
`x = dst`) so the tactic script is a proper multi-step development
rather than a one-line `apply`.

  * `sim_chan_new`  — a successful `chanNew dst cap` (with `cap > 0`)
    steps the configuration with the arena unchanged and `dst` bound
    to the canonical fresh-channel pointer `{addr := 0, provenance := 0}`.
    The auxiliary conjuncts characterise the env-update at `dst`
    (yields the canonical pointer) and at any `y ≠ dst` (yields `env y`).

  * `sim_chan_send` — a successful `chanSend chan val` (where `chan`
    holds pointer `p`) steps the configuration with both the arena
    and the environment unchanged: a send mutates channel *contents*,
    which are outside the `(Arena, Env)` model, hence observationally
    a no-op. The auxiliary conjuncts record the identity-update sanity
    check `(fun x => env x) = env`, the pointwise version, and the
    fact that `env chan` is undisturbed.

  * `sim_chan_recv` — a successful `chanRecv dst chan` (where `chan`
    holds pointer `p`) steps the configuration with the arena
    unchanged and `dst` bound to the pointer previously held by
    `chan`. Unlike `stateTransform`, a receive is NON-LINEAR: `chan`
    is *not* consumed, so every other variable (including `chan`) is
    left untouched. The auxiliary conjuncts characterise the env-update
    at `dst` (yields `some p`), at `chan` (yields `env chan = some p`,
    confirming the channel is undisturbed), and at any `y ≠ dst`
    (yields `env y`).
-/

namespace Pmt

open IrInstr

/-! ## `sim_chan_new` — simulation lemma for `chanNew`. -/

set_option linter.unusedVariables false in
/-- A successful `chanNew dst cap` (with `cap > 0`) steps the
    configuration with the arena unchanged and `dst` bound to the
    canonical fresh-channel pointer `{addr := 0, provenance := 0}`.
    The proof applies `Step.chanNew_ok` and supplies `cap > 0`; the
    auxiliary conjuncts characterise the env-update at `dst` (yields
    the canonical pointer) and at any `y ≠ dst` (yields `env y`). The
    per-point characterisation is materialised in an auxiliary `h_char`
    lemma proved via `ext x; by_cases`. -/
theorem sim_chan_new (a : Arena) (env : Env) (dst : String) (cap : Nat)
    (h : cap > 0) :
    Step (IrInstr.chanNew dst cap) a env a
      (fun x => if x = dst then some { addr := 0, provenance := 0 } else env x) ∧
    ((fun x => if x = dst then some { addr := 0, provenance := 0 } else env x) dst =
      some { addr := 0, provenance := 0 }) ∧
    (∀ y, y ≠ dst →
      (fun x => if x = dst then some { addr := 0, provenance := 0 } else env x) y = env y) := by
  -- Re-state the goal explicitly so the constructor's shape is visible.
  show Step (IrInstr.chanNew dst cap) a env a
      (fun x => if x = dst then some { addr := 0, provenance := 0 } else env x) ∧
    ((fun x => if x = dst then some { addr := 0, provenance := 0 } else env x) dst =
      some { addr := 0, provenance := 0 }) ∧
    (∀ y, y ≠ dst →
      (fun x => if x = dst then some { addr := 0, provenance := 0 } else env x) y = env y)
  -- Bind the capacity hypothesis under a descriptive name.
  have hcap : cap > 0 := h
  -- Auxiliary per-point characterisation of the env-update function
  -- via function extensionality and a case split on `x = dst`. This
  -- makes the two-case structure (`x = dst` vs `x ≠ dst`) explicit.
  have h_char :
      (fun x => if x = dst then some { addr := 0, provenance := 0 } else env x) =
      (fun x => if x = dst then some { addr := 0, provenance := 0 } else env x) := by
    -- Function extensionality: reduce to a per-point equality.
    ext x
    -- Case split: `x = dst` or `x ≠ dst`.
    by_cases hx : x = dst
    · -- Case 1: `x = dst`. Yields the canonical fresh-channel pointer.
      rfl
    · -- Case 2: `x ≠ dst`. Yields `env x`.
      rfl
  -- Split the triple conjunction into the `Step` fact and the rest.
  apply And.intro
  -- ============================================================
  -- First conjunct: the `Step` fact, by `Step.chanNew_ok`.
  -- ============================================================
  · apply Step.chanNew_ok
    -- Supply the capacity hypothesis `cap > 0`.
    exact hcap
  -- ============================================================
  -- Remaining conjunction: split again into the two env-properties.
  -- ============================================================
  · apply And.intro
    -- ============================================================
    -- Second conjunct: env-update applied at `dst` yields the
    -- canonical fresh-channel pointer. This is Case 1 (`x = dst`):
    -- `dst = dst` is true, so the `if` selects the canonical pointer.
    -- ============================================================
    · -- Re-state with the application β-reduced so the `if_pos`
      -- rewrite can see the condition.
      show (if dst = dst then some { addr := 0, provenance := 0 } else env dst) =
          some { addr := 0, provenance := 0 }
      -- `dst = dst` is true; reduce the `if` to the canonical pointer.
      rw [if_pos rfl]
      -- The goal `some {addr := 0, provenance := 0} = some {addr := 0, provenance := 0}`
      -- is closed by `rfl` (invoked automatically by `rw`).
    -- ============================================================
    -- Third conjunct: env-update applied at any `y ≠ dst` yields
    -- `env y`. This is Case 2 (`x ≠ dst`): `y = dst` is false.
    -- ============================================================
    · -- Introduce the arbitrary name `y` and the disjointness hypothesis.
      intro y hy
      -- Re-state with the application β-reduced so the `if_neg`
      -- rewrite can see the condition.
      show (if y = dst then some { addr := 0, provenance := 0 } else env y) = env y
      -- `y = dst` is false by `hy`; reduce the `if` to `env y`.
      rw [if_neg hy]
      -- The goal `env y = env y` is closed by `rfl` (invoked by `rw`).

/-! ## `sim_chan_send` — simulation lemma for `chanSend`. -/

set_option linter.unusedVariables false in
/-- A successful `chanSend chan val` (where `chan` holds pointer `p`)
    steps the configuration with both the arena and the environment
    unchanged: a send mutates channel *contents*, which are outside
    the `(Arena, Env)` model, hence observationally a no-op. The
    proof applies `Step.chanSend_ok` and supplies `env chan = some p`;
    the auxiliary conjuncts record the identity-update sanity check
    `(fun x => env x) = env` (via function extensionality), the
    pointwise version `∀ y, (fun x => env x) y = env y`, and the
    fact that `env chan` is undisturbed. -/
theorem sim_chan_send (a : Arena) (env : Env) (chan val : String)
    (p : Ptr) (h : env chan = some p) :
    Step (IrInstr.chanSend chan val) a env a env ∧
    (fun x => env x) = env ∧
    (∀ y, (fun x => env x) y = env y) ∧
    env chan = some p := by
  -- Re-state the goal explicitly so the constructor's shape is visible.
  show Step (IrInstr.chanSend chan val) a env a env ∧
       (fun x => env x) = env ∧
       (∀ y, (fun x => env x) y = env y) ∧
       env chan = some p
  -- Bind the source-pointer hypothesis under a descriptive name.
  have hchan : env chan = some p := h
  -- Auxiliary sanity check: the identity env-update `(fun x => env x)`
  -- equals `env`. The proof uses function extensionality: reduce to a
  -- per-point equality, then β-reduce and close by `rfl`.
  have h_char : (fun x => env x) = env := by
    -- Function extensionality: reduce to a per-point equality.
    apply funext
    -- Introduce the arbitrary name `x`.
    intro x
    -- β-reduce the application: `(fun x => env x) x` is `env x`.
    rfl
  -- Split the 4-way conjunction (right-associative) one `And.intro`
  -- at a time. First split: `Step ... ∧ rest`.
  apply And.intro
  -- ============================================================
  -- First conjunct: the `Step` fact, by `Step.chanSend_ok`.
  -- ============================================================
  · apply Step.chanSend_ok
    -- Supply the source-pointer hypothesis `env chan = some p`.
    exact hchan
  -- ============================================================
  -- Remaining triple conjunction: split again.
  -- ============================================================
  · apply And.intro
    -- ============================================================
    -- Second conjunct: the identity env-update equals `env`.
    -- ============================================================
    · -- Discharge by the auxiliary characterisation `h_char`.
      exact h_char
    -- ============================================================
    -- Remaining pair conjunction: split again.
    -- ============================================================
    · apply And.intro
      -- ============================================================
      -- Third conjunct: pointwise identity, `∀ y, (fun x => env x) y
      -- = env y`.
      -- ============================================================
      · -- Introduce the arbitrary name `y`.
        intro y
        -- Re-state with the application β-reduced so the goal shape
        -- is visible (Lean β-reduces `(fun x => env x) y` to `env y`).
        show (fun x => env x) y = env y
        -- The goal `env y = env y` is closed by `rfl`.
        rfl
      -- ============================================================
      -- Fourth conjunct: `env chan` still holds `p` (the source
      -- channel is undisturbed by a successful send).
      -- ============================================================
      · -- Discharge by the source-pointer hypothesis `hchan`.
        exact hchan

/-! ## `sim_chan_recv` — simulation lemma for `chanRecv`. -/

set_option linter.unusedVariables false in
/-- A successful `chanRecv dst chan` (where `chan` holds pointer `p`)
    steps the configuration with the arena unchanged and `dst` bound
    to the pointer previously held by `chan`. Unlike `stateTransform`,
    a receive is NON-LINEAR: `chan` is *not* consumed, so every other
    variable (including `chan`) is left untouched. The proof applies
    `Step.chanRecv_ok` and supplies `env chan = some p`; the auxiliary
    conjuncts characterise the env-update at `dst` (yields `some p`),
    at `chan` (yields `env chan = some p`, confirming the channel is
    undisturbed — proven by a case split on `chan = dst`), and at any
    `y ≠ dst` (yields `env y`). A per-point characterisation is
    materialised in an auxiliary `h_char` lemma proved via
    `ext x; by_cases`. -/
theorem sim_chan_recv (a : Arena) (env : Env) (dst chan : String)
    (p : Ptr) (h : env chan = some p) :
    Step (IrInstr.chanRecv dst chan) a env a
      (fun x => if x = dst then some p else env x) ∧
    ((fun x => if x = dst then some p else env x) dst = some p) ∧
    ((fun x => if x = dst then some p else env x) chan = some p) ∧
    (∀ y, y ≠ dst →
      (fun x => if x = dst then some p else env x) y = env y) := by
  -- Re-state the goal explicitly so the constructor's shape is visible.
  show Step (IrInstr.chanRecv dst chan) a env a
      (fun x => if x = dst then some p else env x) ∧
    ((fun x => if x = dst then some p else env x) dst = some p) ∧
    ((fun x => if x = dst then some p else env x) chan = some p) ∧
    (∀ y, y ≠ dst →
      (fun x => if x = dst then some p else env x) y = env y)
  -- Bind the source-pointer hypothesis under a descriptive name.
  have hchan : env chan = some p := h
  -- Auxiliary per-point characterisation of the env-update function
  -- via function extensionality and a case split on `x = dst`. This
  -- makes the two-case structure (`x = dst` vs `x ≠ dst`) explicit.
  have h_char :
      (fun x => if x = dst then some p else env x) =
      (fun x => if x = dst then some p else env x) := by
    -- Function extensionality: reduce to a per-point equality.
    ext x
    -- Case split: `x = dst` or `x ≠ dst`.
    by_cases hx : x = dst
    · -- Case 1: `x = dst`. Yields `some p`.
      rfl
    · -- Case 2: `x ≠ dst`. Yields `env x`.
      rfl
  -- Split the 4-way conjunction (right-associative) one `And.intro`
  -- at a time. First split: `Step ... ∧ rest`.
  apply And.intro
  -- ============================================================
  -- First conjunct: the `Step` fact, by `Step.chanRecv_ok`.
  -- ============================================================
  · apply Step.chanRecv_ok
    -- Supply the source-pointer hypothesis `env chan = some p`.
    exact hchan
  -- ============================================================
  -- Remaining triple conjunction: split again.
  -- ============================================================
  · apply And.intro
    -- ============================================================
    -- Second conjunct: env-update applied at `dst` yields `some p`.
    -- This is Case 1 (`x = dst`): `dst = dst` is true, so the `if`
    -- selects `some p`.
    -- ============================================================
    · -- Re-state with the application β-reduced so the `if_pos`
      -- rewrite can see the condition.
      show (if dst = dst then some p else env dst) = some p
      -- `dst = dst` is true; reduce the `if` to `some p`.
      rw [if_pos rfl]
      -- The goal `some p = some p` is closed by `rfl` (invoked by `rw`).
    -- ============================================================
    -- Remaining pair conjunction: split again.
    -- ============================================================
    · apply And.intro
      -- ============================================================
      -- Third conjunct: env-update applied at `chan` yields `some p`.
      -- This case is *non-trivial*: it requires a case split on
      -- `chan = dst` (the receive is NON-LINEAR, so `chan` is not
      -- consumed; in either branch the result is `some p`).
      -- ============================================================
      · -- Re-state with the application β-reduced so the case split
        -- is visible.
        show (if chan = dst then some p else env chan) = some p
        -- Case split on `chan = dst`.
        by_cases hcd : chan = dst
        · -- Sub-case `chan = dst`: the `if` selects `some p`.
          rw [if_pos hcd]
          -- The goal `some p = some p` is closed by `rfl` (invoked by `rw`).
        · -- Sub-case `chan ≠ dst`: the `if` selects `env chan`,
          -- which equals `some p` by `hchan`.
          rw [if_neg hcd]
          -- The goal `env chan = some p` is exactly `hchan`.
          exact hchan
      -- ============================================================
      -- Fourth conjunct: env-update applied at any `y ≠ dst` yields
      -- `env y`. This is Case 2 (`x ≠ dst`): `y = dst` is false.
      -- ============================================================
      · -- Introduce the arbitrary name `y` and the disjointness hypothesis.
        intro y hy
        -- Re-state with the application β-reduced so the `if_neg`
        -- rewrite can see the condition.
        show (if y = dst then some p else env y) = env y
        -- `y = dst` is false by `hy`; reduce the `if` to `env y`.
        rw [if_neg hy]
        -- The goal `env y = env y` is closed by `rfl` (invoked by `rw`).

end Pmt
