import PMT.Soundness
import PMT.WellTypedStrong
import PMT.Extraction

/-!
## Property Tests — exercise the model more thoroughly

These tests exercise properties of the PMT model that go beyond the
basic happy-path / UAF / overflow tests in the other `Test/` modules:

  * §1–§4: `WF_Layout` well-formedness across several layout shapes
    (empty, single-field, two-field non-overlapping, two-field
    overlapping — the last being a *negative* test).
  * §5: Capacity-check monotonicity (pure arithmetic sanity).
  * §6, §7: The verified extraction checkers
    (`verified_capacity_check`, `verified_field_bounds_check`)
    agree with their mathematical conditions.
  * §8, §9: `pmt_soundness` instantiated on an empty program and on a
    single-step program (the latter exercises the inductive step's
    `transform` case end-to-end).

All examples close without `sorry`.
-/

namespace PMT.Test.PropertyTests

-- §1: Empty layout (zero total_size, no fields) is well-formed.
example : WF_Layout ⟨"layout", 0, []⟩ := by
  unfold WF_Layout
  intro f hf; cases hf

-- §2: Single-field layout is well-formed if the field fits in bounds.
example : WF_Layout ⟨"layout", 8, [⟨"f", 0, 8, "i32"⟩]⟩ := by
  unfold WF_Layout
  intro f hf
  simp at hf
  rcases hf with rfl
  decide

-- §3: Two non-overlapping fields are well-formed (PMT-FAITH-6-C: WF_Layout
-- is now 1 conjunct — field bounds only. Disjointness is a separate predicate.)
example : WF_Layout ⟨"layout", 16, [⟨"f", 0, 4, "i32"⟩, ⟨"f", 4, 4, "i32"⟩]⟩ := by
  unfold WF_Layout
  intro f hf
  simp at hf
  rcases hf with rfl | rfl
  · decide
  · decide

-- §3b: The same layout is also disjoint (separate predicate, PMT-FAITH-6-C).
example : WF_Layout_Disjoint ⟨"layout", 16, [⟨"f", 0, 4, "i32"⟩, ⟨"f", 4, 4, "i32"⟩]⟩ := by
  unfold WF_Layout_Disjoint
  intros f₁ f₂ h₁ h₂ hne
  simp at h₁ h₂
  rcases h₁ with rfl | rfl
  · rcases h₂ with rfl | rfl
    · exact absurd rfl hne
    · left; decide
  · rcases h₂ with rfl | rfl
    · right; decide
    · exact absurd rfl hne

-- §4: Overlapping fields ARE WF_Layout under the new 1-conjunct definition
-- (Rust's IVE accepts overlapping fields — PMT-FAITH-6-C closes FAITH-1-C).
-- They are NOT WF_Layout_Disjoint (the separate disjointness predicate).
example : WF_Layout ⟨"layout", 8, [⟨"f", 0, 4, "i32"⟩, ⟨"f", 2, 4, "i32"⟩]⟩ := by
  unfold WF_Layout
  intro f hf
  simp at hf
  rcases hf with rfl | rfl
  · decide
  · decide

example : ¬ WF_Layout_Disjoint ⟨"layout", 8, [⟨"f", 0, 4, "i32"⟩, ⟨"f", 2, 4, "i32"⟩]⟩ := by
  intro h
  have h_ne : (⟨"f", 0, 4, "i32"⟩ : Field) ≠ ⟨"f", 2, 4, "i32"⟩ := by
    intro heq; injection heq with _ h2; omega
  have h_contra : Disjoint ⟨"f", 0, 4, "i32"⟩ ⟨"f", 2, 4, "i32"⟩ :=
    h ⟨"f", 0, 4, "i32"⟩ ⟨"f", 2, 4, "i32"⟩ (by simp) (by simp) h_ne
  unfold Disjoint at h_contra
  simp at h_contra

-- §5: Capacity check is monotonic in capacity.
example (used size : Nat) (cap1 cap2 : Nat)
    (hcap : cap1 ≤ cap2)
    (hcheck : used + size ≤ cap1) :
    used + size ≤ cap2 := by omega

-- §6: The verified_capacity_check matches the mathematical condition.
-- PMT-FAITH-5-C: now uses BitVec 64 (with boundedness hypothesis for lossless conversion).
example (used size capacity : Nat)
    (h_used : used < 2^64) (h_size : size < 2^64) (h_cap : capacity < 2^64)
    (h_no_ovf : used + size < 2^64) :
    PMT.Extraction.verified_capacity_check (BitVec.ofNat 64 used) (BitVec.ofNat 64 size) (BitVec.ofNat 64 capacity) = true
    ↔ used + size ≤ capacity := by
  unfold PMT.Extraction.verified_capacity_check
  rw [decide_eq_true_iff]
  refine ⟨?_, ?_⟩
  · -- Forward: check = true → used + size ≤ capacity
    intro ⟨hnoovf, hsum⟩
    have eq_used : (BitVec.ofNat 64 used).toNat = used := by rw [BitVec.toNat_ofNat]; omega
    have eq_size : (BitVec.ofNat 64 size).toNat = size := by rw [BitVec.toNat_ofNat]; omega
    have eq_cap : (BitVec.ofNat 64 capacity).toNat = capacity := by rw [BitVec.toNat_ofNat]; omega
    have hsum_nat := BitVec.le_def.mp hsum
    rw [BitVec.toNat_add, eq_used, eq_size] at hsum_nat
    rw [eq_cap] at hsum_nat
    -- hsum_nat : (used + size) % 2^64 ≤ capacity; under h_no_ovf, modulo is identity.
    omega
  · -- Backward: used + size ≤ capacity → check = true
    intro hfit
    refine ⟨?_, ?_⟩
    · -- no_overflow: size ≤ usizeMax - used
      rw [BitVec.le_def, BitVec.toNat_sub, BitVec.toNat_allOnes, BitVec.toNat_ofNat, BitVec.toNat_ofNat]
      omega
    · -- sum ≤ capacity
      rw [BitVec.le_def, BitVec.toNat_add, BitVec.toNat_ofNat, BitVec.toNat_ofNat, BitVec.toNat_ofNat]
      omega

-- §7: The verified_field_bounds_check matches the mathematical condition.
-- PMT-FAITH-6-C: Field now has 4 fields, Layout has 3 fields.
example (f_offset f_size layout_total : Nat) :
    PMT.Extraction.verified_field_bounds_check ⟨"f", f_offset, f_size, "i32"⟩ ⟨"layout", layout_total, []⟩ = true
    ↔ f_offset + f_size ≤ layout_total := by
  unfold PMT.Extraction.verified_field_bounds_check
  simp

-- §8: pmt_soundness holds for an empty program.
example (s : ExecState) (hcap : CapacityInvariant s.arena) :
    ∃ r, exec [] s = r
    ∧ (match r with
       | Result.ok fu => fu ≤ s.arena.capacity
       | Result.trap c => c = 1 ∨ c = 134 ∨ c = 135) := by
  refine ⟨Result.ok s.arena.used, rfl, ?_⟩
  exact hcap

-- §9: pmt_soundness holds for a single-step program.
example (s : ExecState)
    (hcap : CapacityInvariant s.arena)
    (hwf : WF_Layout ⟨"layout", 8, []⟩)
    (hlive : s.live "in" = Liveness.live)
    (_hfit : s.arena.used + 8 ≤ s.arena.capacity) :
    ∃ r, exec [⟨"in", "out", ⟨"layout", 8, []⟩, .transform⟩] s = r
    ∧ (match r with
       | Result.ok fu => fu ≤ s.arena.capacity
       | Result.trap c => c = 1 ∨ c = 134 ∨ c = 135) := by
  -- Per-step hypothesis for pmt_soundness: every step in the (singleton)
  -- program has a well-formed layout and a live input variable.
  have hstep : ∀ st : Step, st ∈ [⟨"in", "out", ⟨"layout", 8, []⟩, .transform⟩] →
                WF_Layout st.layout ∧ s.live st.in_var = Liveness.live := by
    intro st hst
    simp at hst
    rcases hst with rfl
    exact ⟨hwf, hlive⟩
  -- WellTypedness of the singleton program: layout WF (via `hwf`),
  -- plus name-uniqueness for `in_var` and `out_var` (each appears once).
  have hwf_prog : WellTyped [⟨"in", "out", ⟨"layout", 8, []⟩, .transform⟩] := by
    unfold WellTyped
    refine ⟨?_, ?_, ?_⟩
    · intro st hst; simp at hst; rcases hst with rfl; exact hwf
    · intro st hst; simp at hst; rcases hst with rfl
      simp [List.filter]
    · intro st hst; simp at hst; rcases hst with rfl
      simp [List.filter]
  exact pmt_soundness _ hwf_prog s hstep hcap

end PMT.Test.PropertyTests
