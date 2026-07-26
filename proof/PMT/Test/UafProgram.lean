import PMT.Soundness

/-!
# PMT Test — Use-After-Free Detection (§7.3 UAF Trap)

A small test harness for the PMT execution model's use-after-free
(UAF) detection. We construct an `ExecState` in which the input
variable is already `Liveness.dead` (consumed by a prior hypothetical
`state_transform`), then issue a `Step` that tries to read it. The
`step` function's first guard trips the `.uaf` trap, and `exec`
propagates this to `Result.trap 135` — the canonical UAF exit code
from `TrapCode.to_exit`.

Unlike the `#eval` regression checks in `PMT/Soundness.lean` (which
are commented out and are merely runtime sanity checks), the
assertions below are *machine-checked proofs*. Changing the model's
UAF guard from `Liveness.dead` to anything else, reordering the
guards, or removing the `.uaf` branch will break the build.

**Type-checking.** `lean PMT/Test/UafProgram.lean` should produce no
errors and no `sorry` warnings. Every assertion closes by `rfl` —
the `step` function is structurally recursive enough that the kernel
reduces it definitionally on concrete inputs.
-/

namespace PMT.Test.UafProgram

/-! ## §7.3 — Single-step UAF trap. -/

/-- A small 16-byte layout with one 4-byte field at offset 0.
Mirrors the `widgetLayout` shape from `PMT/Soundness.lean` but
trimmed to the minimum needed to exercise the UAF path. -/
def uafLayout : Layout := ⟨16, [⟨0, 4⟩]⟩

/-- Initial state in which the variable `"x"` is `dead` (its token
has already been consumed by a prior `state_transform`), while every
other variable remains `live`. The `if` is the simplest way to make
one specific variable dead while keeping the rest live — exactly the
shape of a real use-after-free scenario. -/
def deadState : ExecState :=
  { arena := ⟨0, 1024, 0⟩,  -- base=0, capacity=1024, used=0
    live  := fun v => if v = "x" then Liveness.dead else Liveness.live }

/-- A step that tries to read `"x"` (already dead) and produce `"y"`.
The layout is irrelevant to the UAF trap — the input is dead, so the
first guard of `step` fires before the layout is consulted. -/
def badStep : Step := ⟨"x", "y", uafLayout, .transform⟩

/-- §7.3: `step` on a dead input traps with `.uaf` immediately —
before any arena-overflow check is consulted. The reduction is
definitional:

  1. `deadState.live badStep.in_var` reduces to
     `deadState.live "x"`, which reduces to
     `if "x" = "x" then Liveness.dead else Liveness.live`, which
     reduces to `Liveness.dead` (string-literal decidable equality).
  2. The first guard of `step` therefore reduces to
     `if Liveness.dead = Liveness.dead then .error .uaf else …`,
     which reduces to `.error .uaf`.

No `sorry`, no `decide` on opaque terms — pure kernel reduction. -/
example : step deadState badStep = Except.error TrapCode.uaf := by
  rfl

/-! ## §7.4 — Whole-program UAF propagation. -/

/-- §7.4: `exec` propagates the UAF trap to the canonical exit code
`135` (`TrapCode.uaf.to_exit`). The reduction unfolds `exec`'s
cons-case, matches on `step`'s `.error` constructor, and reduces
`TrapCode.uaf.to_exit` to `135`. -/
example : exec [badStep] deadState = Result.trap 135 := by
  rfl

/-- §7.4 (multi-step): a longer program whose *first* step UAFs does
not execute the remaining steps. The trap short-circuits and the
final exit code is still `135`, regardless of what `rest` would have
done (here `rest` is itself a valid step that would otherwise
succeed). -/
def okLayout : Layout := ⟨8, [⟨0, 8⟩]⟩
def okStep  : Step := ⟨"y", "z", okLayout, .transform⟩

example : exec [badStep, okStep] deadState = Result.trap 135 := by
  rfl

/-! ## §7.3 (parametricity) — The UAF guard is layout-independent. -/

/-- Sanity: the same dead input with a *different* layout still traps
UAF. The layout is irrelevant once the input is dead — the UAF guard
fires before the layout is ever consulted. -/
def altLayout   : Layout := ⟨8, [⟨0, 8⟩]⟩
def altBadStep  : Step   := ⟨"x", "z", altLayout, .transform⟩

example : step deadState altBadStep = Except.error TrapCode.uaf := by
  rfl

/-- Sanity: the UAF trap fires regardless of the output variable
name — only the *input* liveness matters. -/
def altBadStep2 : Step := ⟨"x", "out", uafLayout, .transform⟩

example : step deadState altBadStep2 = Except.error TrapCode.uaf := by
  rfl

/-! ## §7.3 (negative control) — A live input does *not* trap UAF. -/

/-- The mirror state in which `"x"` is `live` (its token has not been
consumed). With the same arena, `step` should succeed and advance the
bump pointer by `uafLayout.total_size = 16`. -/
def liveState : ExecState :=
  { arena := ⟨0, 1024, 0⟩,
    live  := fun _ => Liveness.live }

/-- Negative control: on a *live* input with enough arena capacity,
`step` succeeds. The result is `Except.ok` of a state whose
`arena.used` is bumped by `uafLayout.total_size = 16` (here `0 + 16`).
This guards against a regression that would make the UAF trap fire
unconditionally. -/
example : (step liveState badStep).isOk = true := by
  rfl

end PMT.Test.UafProgram
