import PMT.Soundness
import PMT.Extraction

/-! ## Misc Lemmas — small utility results

This module collects small lemmas that don't fit elsewhere.
-/

namespace PMT

/-- §1: alloc is commutative in its effect on used. -/
theorem alloc_used_comm (a : Arena) (l : Layout) :
    (alloc a l).used = a.used + l.total_size := by
  rfl

/-- §2: Capacity invariant is about used ≤ capacity. -/
theorem CapacityInvariant_eq (a : Arena) :
    CapacityInvariant a ↔ a.used ≤ a.capacity := by
  rfl

/-- §3: WF_Arena is the same as CapacityInvariant. -/
theorem WF_Arena_eq_CapacityInvariant (a : Arena) :
    WF_Arena a ↔ CapacityInvariant a := by
  rfl

/-- §4: emptyLayout has total_size = 1. -/
theorem emptyLayout_size : emptyLayout.total_size = 1 := by
  rfl

/-- §5: verified_capacity_check with zero used always passes. -/
theorem verified_capacity_check_zero_used (size capacity : Nat)
    (hfit : size ≤ capacity) :
    PMT.Extraction.verified_capacity_check 0 size capacity = true := by
  unfold PMT.Extraction.verified_capacity_check
  simp [hfit]

/-- §6: Result.ok is not Result.trap. -/
theorem Result_ok_ne_trap (n : Nat) (c : Nat) :
    Result.ok n ≠ Result.trap c := by
  intro h
  cases h

/-- §7: TrapCode.arena_overflow has exit 1. -/
theorem TrapCode_arena_overflow_exit : TrapCode.arena_overflow.to_exit = 1 := by
  rfl

/-- §8: TrapCode.oob has exit 134. -/
theorem TrapCode_oob_exit : TrapCode.oob.to_exit = 134 := by
  rfl

/-- §9: TrapCode.uaf has exit 135. -/
theorem TrapCode_uaf_exit : TrapCode.uaf.to_exit = 135 := by
  rfl

end PMT
