import PMT.Soundness

/-!
# PMT Test — Valid 2-Step Program (Happy Path)

W7-A test harness: a small but valid 2-step PMT program (`in → mid → out`)
exercising the happy path of `PMT.Soundness.exec`. Verifies:

  1. The widget `Layout` (16 bytes, 3 fields) is well-formed.
  2. The 2-step program is `WellTyped` (layouts WF, names unique).
  3. `exec prog initState = Result.ok 32` — the bump pointer advances by
     `16 + 16 = 32` bytes, well within the 1024-byte arena capacity.

This exercises the successful-execution branch of `PMT.Soundness.exec`
and complements the `pmt_soundness` theorem (which guarantees that any
well-typed program either succeeds with `final_used ≤ capacity` or
traps with a canonical exit code).

**Type-checking.** `lake build` should produce no errors and no `sorry`
warnings for this module.
-/

namespace PMT.Test.ValidProgram

/-! ## §1. A widget layout: 16 bytes total, 3 fields. -/

/-- A 16-byte widget layout with three fields at offsets 0, 4, and 8.
This mirrors the `widgetLayout` sanity-check constant in
`PMT/Soundness.lean`, but lives in the test namespace so it can be
referenced by name from the well-typedness proof below. -/
def widgetLayout : Layout := ⟨"layout", 16, [⟨"f", 0, 4, "i32"⟩, ⟨"f", 4, 4, "i32"⟩, ⟨"f", 8, 8, "i32"⟩]⟩

/-- The widget layout is well-formed: every field is in bounds
(`offset + size ≤ 16`), every distinct pair of fields is disjoint,
and `total_size > 0`. -/
theorem wf_widgetLayout : WF_Layout widgetLayout := by
  unfold WF_Layout
  intro f hf
  simp [widgetLayout] at hf
  rcases hf with rfl | rfl | rfl
  all_goals simp [widgetLayout]

/-! ## §2. Initial execution state. -/

/-- Empty arena (base 0, capacity 1024, used 0); all variables live.
This is the starting state for `exec prog initState`. -/
def initState : ExecState :=
  { arena := ⟨0, 1024, 0⟩, live := fun _ => Liveness.live }

/-! ## §3. A valid 2-step program: `in → mid → out`. -/

/-- The 2-step program:
  * Step 1: consume `"in"`, produce `"mid"`, using `widgetLayout`.
  * Step 2: consume `"mid"`, produce `"out"`, using `widgetLayout`.

Both `in_var` and `out_var` names are unique across steps. -/
def prog : Program :=
  [ ⟨"in", "mid", widgetLayout, .transform⟩,
    ⟨"mid", "out", widgetLayout, .transform⟩ ]

/-- The program is well-typed:
  * Every step's layout is `WF_Layout` (delegates to `wf_widgetLayout`).
  * Each `in_var` name appears exactly once across steps
    (`List.filter ... .length = 1`).
  * Each `out_var` name appears exactly once across steps. -/
theorem wellTyped_prog : WellTyped prog := by
  unfold WellTyped
  refine ⟨?_, ?_, ?_⟩
  · -- All layouts are well-formed.
    intro st hst
    simp [prog] at hst
    rcases hst with rfl | rfl
    all_goals exact wf_widgetLayout
  · -- in_var uniqueness: each step's in_var appears exactly once.
    intro st hst
    simp [prog] at hst
    rcases hst with rfl | rfl
    all_goals simp [prog]
  · -- out_var uniqueness: each step's out_var appears exactly once.
    intro st hst
    simp [prog] at hst
    rcases hst with rfl | rfl
    all_goals simp [prog]

/-! ## §4. Execution succeeds, advancing `used` by 32 bytes. -/

/-- `exec prog initState = Result.ok 32`. The bump pointer advances
`0 → 16 → 32`. Each step's guards pass:
  * Step 1 (`in → mid`): `initState.live "in" = Liveness.live`
    (constant function), so the UAF guard fails. `0 + 16 ≤ 1024`,
    so the overflow guard fails.
  * Step 2 (`mid → out`): after step 1, `live "mid" = Liveness.live`
    (just made live); `16 + 16 ≤ 1024`.

The reduction is definitional: `exec`, `step`, `Liveness.decEq`,
`Nat.decLt`, and `String.instDecidableEq` all reduce on literal
strings and small Nats, so `rfl` closes the goal. -/
theorem exec_prog : exec prog initState = Result.ok 32 := by
  rfl

end PMT.Test.ValidProgram
