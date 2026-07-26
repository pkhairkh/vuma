import PMT.Soundness

/-!
# PMT Test — Multi-Step Program Capacity Preservation

A test harness exercising the PMT execution model (`PMT.Soundness.exec`)
across a 4-step program. The program threads state through five
variables (`a → b → c → d → e`), each step allocating a fixed 16-byte
layout from a 1024-byte arena. After four steps the bump pointer has
advanced by 4 × 16 = 64 bytes — well under capacity — so `exec` returns
`Result.ok 64`.

This module is a *test*, not part of the soundness proof: it confirms
that the executable specification computes the expected arithmetic and
preserves the capacity invariant across multiple steps. The
`pmt_soundness` theorem (in `PMT.Soundness`) gives the general result;
the lemmas below instantiate it on a concrete program.

**Methodology.** Each `step` invocation is proved equal to `Except.ok s'`
by `rfl` (definitional reduction): the `if s.live i.in_var = Liveness.dead`
and `if s.arena.used + sz > s.arena.capacity` guards reduce via the
`DecidableEq Liveness` and `Nat` decidable instances, while the
`if v = i.in_var then …` branches of the `live`-function update reduce
via the `DecidableEq String` instance (which reduces for literal
strings). The top-level `exec` example then composes the four step
lemmas by rewriting `exec` and the step lemmas in sequence, finishing
with `rfl` on the final `arena.used = 64` arithmetic.

**Type-checking.** `lake build` should produce no errors and no `sorry`
warnings.
-/

namespace PMT.Test.MultiStepProgram

open PMT

/-! ## A 16-byte layout. -/

/-- A simple 16-byte layout with no sub-fields. Well-formed because the
total size is strictly positive and the field list is empty. -/
def layout16 : Layout := ⟨16, []⟩

/-- `layout16` is well-formed: positive total size, no fields to
overlap. Mirrors `WF_Layout_empty` (in `PMT.Basic`) but at size 16. -/
example : WF_Layout layout16 := by
  unfold WF_Layout layout16
  refine ⟨?_, ?_, ?_⟩
  · intro f hf; cases hf
  · intros _ _ h₁ _ _; cases h₁
  · exact Or.inl (by decide)

/-! ## Initial execution state. -/

/-- Initial state: a fresh 1024-byte arena (`base=0, capacity=1024,
used=0`) with every variable considered `live` (no consumption yet). -/
def initState : ExecState :=
  { arena := ⟨0, 1024, 0⟩,
    live  := fun _ => Liveness.live }

/-- The initial arena satisfies the capacity invariant: `0 ≤ 1024`. -/
example : CapacityInvariant initState.arena := by
  unfold CapacityInvariant initState
  decide

/-! ## A 4-step program: `a → b → c → d → e`. -/

/-- A 4-step PMT program. Each step consumes the previous step's output
variable and produces a fresh output, allocating 16 bytes per step.

`WellTyped prog` holds: every layout is `WF_Layout` (vacuously, all
`layout16`), and each `in_var`/`out_var` name appears exactly once in
the program. -/
def prog : Program :=
  [ ⟨"a", "b", layout16, .transform⟩,
    ⟨"b", "c", layout16, .transform⟩,
    ⟨"c", "d", layout16, .transform⟩,
    ⟨"d", "e", layout16, .transform⟩ ]

/-! ## Intermediate states after each step.

Each `sK` is the `ExecState` that `step s(K-1) stepK` reduces to.
The `live` function is exactly the one constructed by `step`'s `.ok`
branch: `if v = stepK.in_var then dead else if v = stepK.out_var then
live else s(K-1).live v`.
-/

/-- State after step 1 (`a → b`): bump pointer at 16, `a` is dead, `b`
is live, everything else inherits `initState.live` (= live). -/
def s1 : ExecState :=
  { arena := ⟨0, 1024, 16⟩,
    live  := fun v =>
      if v = "a" then Liveness.dead
      else if v = "b" then Liveness.live
      else initState.live v }

/-- State after step 2 (`b → c`): bump pointer at 32, `b` is dead, `c`
is live, everything else inherits `s1.live`. -/
def s2 : ExecState :=
  { arena := ⟨0, 1024, 32⟩,
    live  := fun v =>
      if v = "b" then Liveness.dead
      else if v = "c" then Liveness.live
      else s1.live v }

/-- State after step 3 (`c → d`): bump pointer at 48, `c` is dead, `d`
is live. -/
def s3 : ExecState :=
  { arena := ⟨0, 1024, 48⟩,
    live  := fun v =>
      if v = "c" then Liveness.dead
      else if v = "d" then Liveness.live
      else s2.live v }

/-- State after step 4 (`d → e`): bump pointer at 64, `d` is dead, `e`
is live. -/
def s4 : ExecState :=
  { arena := ⟨0, 1024, 64⟩,
    live  := fun v =>
      if v = "d" then Liveness.dead
      else if v = "e" then Liveness.live
      else s3.live v }

/-! ## Step lemmas: each `step` reduces to `Except.ok sK`.

Each step reduction is closed by `rfl`. Definitional reduction handles the
guard conditions:
  * `s.live i.in_var = Liveness.dead` — `s.live i.in_var` reduces via
    `DecidableEq String` (for literal strings) and `DecidableEq Liveness`
    to `Liveness.live`, then `Liveness.live = Liveness.dead` reduces
    via the `DecidableEq Liveness` instance to `isFalse _`.
  * `s.arena.used + i.layout.total_size > s.arena.capacity` — pure
    `Nat` arithmetic, reduces via `Nat` decidable comparison.
  * The `live`-function update — both sides have the same `fun v => ...`
    body, so function extensionality is unnecessary.
-/

example : step initState ⟨"a", "b", layout16, .transform⟩ = Except.ok s1 := by
  rfl

example : step s1 ⟨"b", "c", layout16, .transform⟩ = Except.ok s2 := by
  rfl

example : step s2 ⟨"c", "d", layout16, .transform⟩ = Except.ok s3 := by
  rfl

example : step s3 ⟨"d", "e", layout16, .transform⟩ = Except.ok s4 := by
  rfl

/-! ## Top-level: capacity preserved across all four steps.

`exec prog initState` reduces by unfolding `exec` once per step,
rewriting with the step lemma, and recursing. After four steps the
final state is `s4`, and `exec [] s4 = Result.ok s4.arena.used =
Result.ok 64`.
-/

/-- Executing the 4-step program on the initial state yields
`Result.ok 64` — capacity is preserved across all four steps (final
`used = 64 ≤ 1024`). -/
example : exec prog initState = Result.ok 64 := by
  unfold prog
  -- Unfold exec once per step, evaluating each `step` to `Except.ok sK`
  -- via definitional reduction (the same reduction that closes the
  -- per-step `rfl` examples above).
  rfl

/-! ## Sanity: the final state's bump pointer is within capacity.

This is the post-condition that `pmt_soundness` guarantees for
well-typed programs: the final `Result.ok final_used` satisfies
`final_used ≤ capacity`. Here we instantiate it concretely by chaining
the top-level example with `decidable` arithmetic. -/
example : match exec prog initState with
          | Result.ok fu => fu ≤ initState.arena.capacity
          | Result.trap _ => True := by
  -- Reduce `exec prog initState` to `Result.ok 64` (definitional),
  -- then `64 ≤ 1024` is decidable arithmetic.
  have h : exec prog initState = Result.ok 64 := by
    unfold prog
    rfl
  rw [h]
  decide

end PMT.Test.MultiStepProgram
