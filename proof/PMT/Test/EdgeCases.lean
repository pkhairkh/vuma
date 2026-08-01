import PMT.Soundness

/-!
# PMT Test — Edge Cases (boundary conditions for the PMT model)

This module is a *test* (not part of the soundness proof) exercising
boundary conditions of the PMT execution model defined in
`PMT.Soundness`. Each example below is a machine-checked proof that
locks down a specific edge case of the `step` / `exec` semantics:

  * §1: Zero-capacity arena rejects any allocation.
  * §2: Exact-fit allocation succeeds (`used + size = capacity`).
  * §3: Single-step program with empty `rest`.
  * §4: Empty layout (`total_size = 0`, `fields = []`) is well-formed.
  * §5: Layout with size but no fields is well-formed.
  * §6: Two steps sharing the same `in_var` (and `out_var`) are NOT
    `WellTyped` — the name-uniqueness conjunct of `WellTyped` rejects
    this program.

These guard against regressions in:
  * The strict-`>` overflow guard (§1 vs §2 — exact-fit must succeed).
  * The well-formedness third conjunct
    `0 < total_size ∨ fields = []` (§4 and §5).
  * The `WellTyped` name-uniqueness check (§6).

**Type-checking.** `lake build PMT.Test.EdgeCases` should produce no
errors and no `sorry` warnings.
-/

namespace PMT.Test.EdgeCases

/-! ## §1. Zero-capacity arena rejects any allocation. -/

/-- An arena with `capacity = 0` (and `used = 0`). No allocation of any
positive size can succeed — the overflow guard `0 + size > 0` is
trivially true for any `size > 0`. -/
def zeroArena : Arena := ⟨0, 0, 0⟩

/-- Initial state on the zero-capacity arena; all variables `live`. -/
def zeroState : ExecState :=
  { arena := zeroArena, live := fun _ => Liveness.live }

/-- `step` on a 1-byte allocation against a 0-byte arena traps with
`.arena_overflow`. The reduction is definitional:
  1. `zeroState.live "in" = Liveness.live` (constant function), so the
     UAF guard falls through to the else branch.
  2. `i.op = PmtOp.transform`, so the `match` selects the transform
     branch.
  3. `0 + 1 > 0` reduces to `true`, so the overflow guard's
     then-branch yields `.error .arena_overflow`. -/
example :
    step zeroState ⟨"in", "out", ⟨"layout", 1, []⟩, .transform⟩ =
    Except.error TrapCode.arena_overflow := by
  rfl

/-! ## §2. Exact-fit allocation succeeds (`used + size = capacity`). -/

/-- An arena with `capacity = 16` and `used = 0`. A 16-byte allocation
fits exactly — the overflow guard uses strict `>`, so `0 + 16 > 16` is
`false` and the step succeeds. -/
def exactFitState : ExecState :=
  { arena := ⟨0, 16, 0⟩, live := fun _ => Liveness.live }

/-- Negative control for §1: at the exact-fit boundary, `step`
succeeds (no overflow trap). This guards against a regression that
would swap `>` for `≥` in the overflow guard. -/
example :
    (step exactFitState ⟨"in", "out", ⟨"layout", 16, []⟩, .transform⟩).isOk = true := by
  rfl

/-! ## §3. Single-step program with empty rest. -/

/-- `exec` on a single-element program advances the bump pointer by the
layout's `total_size` and returns `Result.ok final_used`. Here the
8-byte layout on the 16-byte arena yields `Result.ok 8`. The reduction
unfolds `exec` once, matches on `step`'s `.ok` constructor, then
unfolds `exec []` to `Result.ok s.arena.used`. -/
example :
    exec [⟨"in", "out", ⟨"layout", 8, []⟩, .transform⟩] exactFitState = Result.ok 8 := by
  rfl

/-! ## §4. Empty layout (`total_size = 0`, `fields = []`) is well-formed. -/

/-- The third conjunct of `WF_Layout` is `0 < total_size ∨ fields = []`.
For `⟨"layout", 0, []⟩` the left disjunct is `0 < 0 = false`, but the right
disjunct is `[] = [] = true`, so the layout is well-formed.

The first two conjuncts (field-bounds and disjointness) are vacuous
because there are no fields. -/
example : WF_Layout ⟨"layout", 0, []⟩ := by
  unfold WF_Layout
  intro f hf; cases hf

/-! ## §5. Layout with size but no fields is well-formed. -/

/-- A layout with `total_size = 8` and empty `fields` is well-formed:
the third conjunct holds via `0 < 8` (left disjunct). The first two
conjuncts are vacuous. -/
example : WF_Layout ⟨"layout", 8, []⟩ := by
  unfold WF_Layout
  intro f hf; cases hf

/-! ## §6. Two steps sharing `in_var`/`out_var` are NOT `WellTyped`. -/

/-- A two-step program in which both steps share the same `in_var`
(`"a"`) and `out_var` (`"b"`). The `WellTyped` predicate's second
conjunct requires, for each `s ∈ prog`, that the filter of `prog` by
`in_var == s.in_var` has length `1`. But filtering by `in_var == "a"`
keeps both steps, giving length `2 ≠ 1`. So `WellTyped` does not hold.

The proof instantiates the universal with the first step, computes the
filter's actual length (2), and derives the contradiction `2 = 1`. -/
def dupStep : Step := ⟨"a", "b", ⟨"layout", 8, []⟩, .transform⟩

/-- A 2-element program whose steps share `in_var` and `out_var` —
this violates the name-uniqueness conjunct of `WellTyped`. -/
def dupProg : Program := [dupStep, dupStep]

example : ¬ WellTyped dupProg := by
  intro h
  unfold WellTyped at h
  obtain ⟨_, h_in, _⟩ := h
  -- `dupStep` is a member of `dupProg` (it appears twice).
  have h_mem : dupStep ∈ dupProg := by simp [dupProg]
  -- `h_eq` claims the in_var-filter has length 1.
  have h_eq := h_in dupStep h_mem
  -- The actual filter length is 2 (both steps share `in_var = "a"`).
  have h_two :
      (List.filter (fun (s' : Step) => s'.in_var == dupStep.in_var)
        dupProg).length = 2 := by
    simp [dupProg, dupStep]
  rw [h_two] at h_eq
  exact absurd h_eq (by decide)

end PMT.Test.EdgeCases
