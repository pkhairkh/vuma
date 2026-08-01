import PMT.Soundness

/-!
# PMT Test — Empty Program (nil case of `pmt_soundness`)

This is the test harness for the empty-program case of `pmt_soundness`.
It exercises three properties of `exec [] initState`:

  1. **Computation**: `exec prog initState = Result.ok 42` by `rfl`.
  2. **Vacuous well-typedness**: `WellTyped prog` holds trivially for `prog = []`.
  3. **Capacity preservation**: the empty program returns `Result.ok fu`
     with `fu ≤ initState.arena.capacity`, discharged directly by `hcap`.

These three examples form the executable regression check for the `nil`
case of the inductive proof in `PMT.Soundness`.
-/

namespace PMT.Test.EmptyProgram

/-- Empty program. -/
def prog : Program := []

/-- Initial state: arena with `base = 0`, `capacity = 1024`, `used = 42`. -/
def initState : ExecState :=
  { arena := ⟨0, 1024, 42⟩,
    live  := fun _ => Liveness.live }

/-- `exec` on the empty program returns `Result.ok s.arena.used` (= 42). -/
example : exec prog initState = Result.ok 42 := by
  rfl

/-- The empty program is well-typed (vacuously — no steps to check). -/
example : WellTyped prog := by
  unfold WellTyped
  refine ⟨?_, ?_, ?_⟩
  · intro _ h; simp [prog] at h
  · intro _ h; simp [prog] at h
  · intro _ h; simp [prog] at h

/-- The empty program preserves the `CapacityInvariant`:
    `exec prog initState = Result.ok 42` and `42 ≤ 1024`. -/
example (hcap : CapacityInvariant initState.arena) :
    ∃ r, exec prog initState = r
    ∧ (match r with
       | Result.ok fu => fu ≤ initState.arena.capacity
       | Result.trap c => c = 1 ∨ c = 134 ∨ c = 135) := by
  refine ⟨Result.ok 42, rfl, ?_⟩
  exact hcap

end PMT.Test.EmptyProgram
