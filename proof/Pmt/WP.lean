import Pmt.IrSubset
import Pmt.CMRA

/-!
# `WP` — Inductive Weakest Precondition

This file defines an *inductive* weakest-precondition predicate `wp` over the
eight-instruction IR subset from `Pmt.IrSubset`. The predicate has exactly two
constructors:

* `wp_done`  — the empty instruction sequence is safe iff the postcondition
  already holds in the current `(Arena, Env)` state.
* `wp_step`  — a non-empty sequence `i :: is` is safe iff there exists a
  single small step `Step i a env a' env'` to an intermediate state from
  which the tail `is` is safe.

There is **deliberately no stuck-state constructor**: the absence of a stuck
rule *is* the safety property. If `wp is a env Φ` holds, then no execution of
`is` starting from `(a, env)` can get stuck — every instruction must step.

Two structural lemmas are proved:

* `wp_frame` (monotonicity): a stronger postcondition can always be weakened.
* `wp_bind`  (sequential composition): `wp` composes over `List.append`.
-/

namespace Pmt

/-- Pointwise implication between postconditions: `Φ ⇒ Ψ` iff every state
    satisfying `Φ` also satisfies `Ψ`. -/
def WPImpl (Φ Ψ : Arena → Env → Prop) : Prop :=
  ∀ a env, Φ a env → Ψ a env

local notation:50 lhs " ⇒ " rhs => WPImpl lhs rhs

set_option linter.unusedVariables false in
/-- Inductive weakest precondition. `wp is a env Φ` holds when every
    execution of the instruction list `is` from `(a, env)` reaches a state
    satisfying `Φ` without getting stuck. The two constructors are the only
    ways to establish `wp`; there is no stuck case. -/
inductive wp : List IrInstr → Arena → Env → (Arena → Env → Prop) → Prop where
  | wp_done : ∀ a env Φ, Φ a env → wp [] a env Φ
  | wp_step : ∀ i is a env a' env' Φ,
      Step i a env a' env' →
      wp is a' env' Φ →
      wp (i :: is) a env Φ

set_option linter.unusedVariables false in
/-- **Monotonicity / framing.** If `wp is a env Φ` and `Φ ⇒ Ψ` pointwise,
    then `wp is a env Ψ`. Proved by induction on the `wp` derivation; the
    pointwise implication is reverted into the goal so it threads cleanly
    through the generalisation of the postcondition. -/
theorem wp_frame {is a env Φ Ψ}
    (h : wp is a env Φ) (himpl : Φ ⇒ Ψ) : wp is a env Ψ := by
  revert himpl
  induction h with
  | wp_done a env Φ hΦ =>
    intro himpl
    exact wp.wp_done a env Ψ (himpl a env hΦ)
  | wp_step i is a env a' env' Φ hstep hwp ih =>
    intro himpl
    exact wp.wp_step i is a env a' env' Ψ hstep (ih himpl)

set_option linter.unusedVariables false in
/-- **Sequential composition / bind.** If
    `wp is1 a env (fun a' env' => wp is2 a' env' Φ)`, then
    `wp (is1 ++ is2) a env Φ`. Proved via an auxiliary generalised statement
    whose postcondition is a variable `Ψ` paired with a linking hypothesis
    `∀ a' env', Ψ a' env' → wp is2 a' env' Φ`; this keeps the connection
    between the intermediate postcondition and `wp is2 ... Φ` alive across
    the induction. `List.append` on `[]`/`::` reduces definitionally, so no
    list lemmas are needed. -/
theorem wp_bind {is1 is2 a env Φ}
    (h : wp is1 a env (fun a' env' => wp is2 a' env' Φ)) :
    wp (is1 ++ is2) a env Φ := by
  have gen : ∀ (Ψ : Arena → Env → Prop) is0 a0 env0,
      (∀ a' env', Ψ a' env' → wp is2 a' env' Φ) →
      wp is0 a0 env0 Ψ → wp (is0 ++ is2) a0 env0 Φ := by
    intro Ψ is0 a0 env0 hlink hwp
    revert hlink
    induction hwp with
    | wp_done a env Ψ hΨ =>
      intro hlink
      exact hlink a env hΨ
    | wp_step i is a env a' env' Ψ hstep hwp ih =>
      intro hlink
      exact wp.wp_step i (is ++ is2) a env a' env' Φ hstep (ih hlink)
  exact gen (fun a' env' => wp is2 a' env' Φ) is1 a env (fun _ _ hx => hx) h

end Pmt
