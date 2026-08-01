import PMT.Soundness
import PMT.WellTypedStrong

/-!
## Additional Theorems — small strengthening results

This module collects small theorems that don't fit in the other modules
but strengthen the overall proof story.

  * §1: `WellTypedStrong_empty`           — the empty program is `WellTypedStrong`.
  * §2: `CapacityInvariant_fresh`         — a fresh arena (used=0) satisfies the
    capacity invariant.
  * §3: `TrapCode_to_exit_injective`      — the `TrapCode.to_exit` map is injective.
  * §4: `TrapCode_exit_codes_distinct`    — the three trap codes have pairwise
    distinct exit codes.
  * §5: `exec_empty_returns_used`         — `exec [] s` returns `Result.ok s.arena.used`
    (definitional unfolding).
  * §6: `exec_cons_trap_propagates`       — a step that traps propagates the trap
    through `exec`.

All theorems in this file close without `sorry`.

**References.**
  * `PMT.Soundness`        — `step`, `exec`, `TrapCode`, `Result`, `WellTyped`.
  * `PMT.WellTypedStrong`  — `WellTypedStrong`, `DataflowOk`, `FieldAccessOk`.
  * `PMT.Basic`            — `Arena`, `CapacityInvariant`.

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build PMT.AdditionalTheorems`
(or `lake build`); the `lean-proofs` CI job in
`.github/workflows/proof-verify.yml` runs the same command.
-/

namespace PMT

/-! ## §1. Empty program is `WellTypedStrong` -/

/-- §1: The empty program is `WellTypedStrong` for any `initial_var`.

Vacuously: no `Step` is a member of `[]`, so all three conjuncts
(`WellTyped`, `DataflowOk`, `FieldAccessOk`) hold trivially. -/
theorem WellTypedStrong_empty (initial_var : String) :
    WellTypedStrong [] initial_var := by
  unfold WellTypedStrong WellTyped DataflowOk FieldAccessOk
  refine ⟨?_, ?_, ?_⟩
  · -- `WellTyped []` has three conjuncts, each vacuously true.
    refine ⟨?_, ?_, ?_⟩
    · intro _ h; cases h
    · intro _ h; cases h
    · intro _ h; cases h
  · intro _ h; cases h
  · intro _ h; cases h

/-! ## §2. Fresh arena satisfies `CapacityInvariant` -/

/-- §2: A fresh arena (used = 0) satisfies `CapacityInvariant` for any
non-negative `capacity`. -/
theorem CapacityInvariant_fresh (capacity : Nat) :
    CapacityInvariant ⟨0, capacity, 0⟩ := by
  show (0 : Nat) ≤ capacity
  omega

/-! ## §3. `TrapCode.to_exit` is injective -/

/-- §3: The exit-code map `TrapCode.to_exit` is injective.

Each `TrapCode` constructor maps to a distinct `Nat` exit code
(1 / 134 / 135), so equal exit codes imply equal trap codes. -/
theorem TrapCode_to_exit_injective (c1 c2 : TrapCode) :
    c1.to_exit = c2.to_exit → c1 = c2 := by
  cases c1 <;> cases c2 <;> simp [TrapCode.to_exit]

/-! ## §4. Distinct trap codes have distinct exit codes -/

/-- §4: The three canonical trap codes have pairwise distinct exit codes.

  * `arena_overflow.to_exit = 1`
  * `oob.to_exit            = 134`
  * `uaf.to_exit            = 135`

These three are pairwise unequal. -/
theorem TrapCode_exit_codes_distinct :
    TrapCode.arena_overflow.to_exit ≠ TrapCode.oob.to_exit
    ∧ TrapCode.arena_overflow.to_exit ≠ TrapCode.uaf.to_exit
    ∧ TrapCode.oob.to_exit ≠ TrapCode.uaf.to_exit := by
  refine ⟨?_, ?_, ?_⟩ <;> decide

/-! ## §5. `exec [] s` returns the arena's `used` -/

/-- §5: Executing the empty program returns `Result.ok s.arena.used`.

This is the definitional unfolding of `exec` on the empty program. -/
theorem exec_empty_returns_used (s : ExecState) :
    exec [] s = Result.ok s.arena.used := by
  rfl

/-! ## §6. Trap propagation through `exec` -/

/-- §6: If `step s i` traps with `Except.error c`, then
`exec (i :: rest) s` is `Result.trap c.to_exit`.

This is the definitional unfolding of `exec` on a non-empty program
when the head step traps. The rest of the program is not executed
(short-circuit). -/
theorem exec_cons_trap_propagates
    (i : Step) (rest : Program) (s : ExecState) (c : TrapCode)
    (hstep : step s i = Except.error c) :
    exec (i :: rest) s = Result.trap c.to_exit := by
  rw [exec, hstep]

end PMT
