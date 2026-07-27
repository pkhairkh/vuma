import PMT.Soundness

/-!
# PMT Test — Arena Overflow Detection (§7.3 Overflow Trap)

A small test harness for the PMT execution model's arena-overflow
detection. We construct an `ExecState` whose arena has a 16-byte
capacity but whose only step attempts to allocate a 32-byte layout.
The `step` function's second guard — `s.arena.used + i.layout.total_size
> s.arena.capacity` — trips the `.arena_overflow` trap, and `exec`
propagates this to `Result.trap 1` (the canonical overflow exit code
from `TrapCode.to_exit`).

Unlike the commented-out `#eval` regression checks in
`PMT/Soundness.lean`, the assertions below are *machine-checked
proofs*. Changing the model's overflow guard (e.g. swapping `>` for
`≥`), reordering the guards so the UAF check would mask an overflow,
or removing the `.arena_overflow` branch will break the build.

**Type-checking.** `lean PMT/Test/OverflowProgram.lean` should produce
no errors and no `sorry` warnings. Every assertion closes by `rfl` —
the `step` function is structurally recursive enough that the kernel
reduces it definitionally on concrete inputs.
-/

namespace PMT.Test.OverflowProgram

/-! ## §7.3 — Single-step arena-overflow trap. -/

/-- Small arena: `base = 0`, `capacity = 16` bytes, `used = 0`.
This is the smallest arena that still admits an overflow demo. -/
def smallArena : Arena := ⟨0, 16, 0⟩

/-- Initial state: every variable is `Liveness.live`, hosted on the
16-byte `smallArena`. The input variable `"in"` is therefore live, so
the UAF guard of `step` does not fire — execution proceeds to the
overflow guard, which is what we want to exercise. -/
def initState : ExecState :=
  { arena := smallArena,
    live  := fun _ => Liveness.live }

/-- A step that tries to allocate a 32-byte layout — twice the
arena's 16-byte capacity. The layout has no fields; the overflow
guard only consults `layout.total_size`, so the field list is
irrelevant for this trap. -/
def overflowStep : Step := ⟨"in", "out", ⟨"layout", 32, []⟩, .transform⟩

/-- §7.3: `step` on a live input whose `layout.total_size` exceeds
the arena's remaining capacity traps with `.arena_overflow`. The
reduction is definitional:

  1. `initState.live overflowStep.in_var` reduces to
     `Liveness.live` (the function ignores its argument), so
     `Liveness.live = Liveness.dead` reduces to `false` via the
     `DecidableEq Liveness` instance — the UAF guard's `if` falls
     through to the else branch.
  2. The overflow guard `s.arena.used + i.layout.total_size >
     s.arena.capacity` reduces to `0 + 32 > 16`, i.e. `32 > 16`,
     i.e. `true` (kernel reduction on `Nat` literals) — so the
     overflow `if` selects its then-branch, yielding
     `.error .arena_overflow`.

No `sorry`, no `decide` on opaque terms — pure kernel reduction. -/
example : step initState overflowStep = Except.error TrapCode.arena_overflow := by
  rfl

/-! ## §7.4 — Whole-program overflow propagation. -/

/-- §7.4: `exec` propagates the overflow trap to the canonical exit
code `1` (`TrapCode.arena_overflow.to_exit`). The reduction unfolds
`exec`'s cons-case, matches on `step`'s `.error` constructor, and
reduces `TrapCode.arena_overflow.to_exit` to `1`. -/
example : exec [overflowStep] initState = Result.trap 1 := by
  rfl

/-- §7.4 (multi-step): a longer program whose *first* step overflows
does not execute the remaining steps. The trap short-circuits and the
final exit code is still `1`, regardless of what `rest` would have
done (here `rest` is itself a valid step that would otherwise
succeed on a sufficiently large arena). -/
def okLayout : Layout := ⟨"layout", 8, [⟨"f", 0, 8, "i32"⟩]⟩
def okStep   : Step   := ⟨"out", "z", okLayout, .transform⟩

example : exec [overflowStep, okStep] initState = Result.trap 1 := by
  rfl

/-! ## §1.2 — The overflow layout is itself well-formed. -/

/-- Sanity: the overflow-triggering layout `⟨"layout", 32, []⟩` satisfies
`WF_Layout`. The trap is *not* a layout-malformation trap — the layout
is perfectly well-formed (32 > 0, no fields to be out-of-bounds or
non-disjoint). The trap fires purely because the layout's
`total_size` exceeds the arena's *remaining* capacity, which is a
runtime/resource condition, not a static/well-formedness condition.

This guards against a regression that would conflate `WF_Layout` with
the runtime capacity check — they are independent properties. -/
example : WF_Layout ⟨"layout", 32, []⟩ := by
  unfold WF_Layout
  intro f hf; cases hf

/-! ## §7.3 (parametricity) — The overflow guard is output-agnostic. -/

/-- Sanity: the same oversized layout with a *different* output
variable still traps overflow — the overflow guard consults only
`layout.total_size` and arena geometry, never the variable names. -/
def altOverflowStep : Step := ⟨"in", "alt", ⟨"layout", 32, []⟩, .transform⟩

example : step initState altOverflowStep = Except.error TrapCode.arena_overflow := by
  rfl

/-! ## §7.3 (boundary) — Exactly-at-capacity does *not* overflow. -/

/-- A layout whose `total_size` equals the arena's remaining capacity
(16 bytes) — the boundary case. The overflow guard uses strict `>`,
so an exact fit is *not* an overflow. -/
def exactLayout : Layout := ⟨"layout", 16, []⟩
def exactStep   : Step   := ⟨"in", "out", exactLayout, .transform⟩

/-- Negative control: at exactly the capacity boundary, `step`
succeeds. The result is `Except.ok` of a state whose `arena.used` is
bumped by `exactLayout.total_size = 16` (here `0 + 16`), and the
arena's capacity is unchanged.

This guards against a regression that would make the overflow guard
fire on exact fits (i.e. swapping `>` for `≥`), which would be a
soundness regression: a well-typed program that fits exactly should
succeed. -/
example : (step initState exactStep).isOk = true := by
  rfl

/-- §7.4: a single exact-fit step yields `Result.ok 16` — the bump
pointer advanced by exactly `exactLayout.total_size`, with no trap.
This is the dual of the overflow demo above. -/
example : exec [exactStep] initState = Result.ok 16 := by
  rfl

end PMT.Test.OverflowProgram
