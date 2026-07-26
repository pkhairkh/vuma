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
example : WF_Layout ⟨0, []⟩ := by
  unfold WF_Layout
  refine ⟨?_, ?_, ?_⟩
  · intro f hf; cases hf
  · intros _ _ h₁ _ _; cases h₁
  · exact Or.inr rfl

-- §2: Single-field layout is well-formed if the field fits in bounds.
example : WF_Layout ⟨8, [⟨0, 8⟩]⟩ := by
  unfold WF_Layout
  refine ⟨?_, ?_, ?_⟩
  · intro f hf
    simp at hf
    rcases hf with rfl
    decide
  · intros f₁ f₂ h₁ h₂ hne
    simp at h₁ h₂
    rcases h₁ with rfl
    rcases h₂ with rfl
    exact (hne rfl).elim
  · exact Or.inl (by decide)

-- §3: Two non-overlapping fields are well-formed.
example : WF_Layout ⟨16, [⟨0, 4⟩, ⟨4, 4⟩]⟩ := by
  unfold WF_Layout Disjoint
  refine ⟨?_, ?_, ?_⟩
  · intro f hf
    simp at hf
    rcases hf with rfl | rfl
    · decide
    · decide
  · intros f₁ f₂ h₁ h₂ hne
    simp at h₁ h₂
    rcases h₁ with rfl | rfl
    · rcases h₂ with rfl | rfl
      · exact (hne rfl).elim
      · left; decide
    · rcases h₂ with rfl | rfl
      · right; decide
      · exact (hne rfl).elim
  · exact Or.inl (by decide)

-- §4: Overlapping fields are NOT well-formed.
example : ¬ WF_Layout ⟨8, [⟨0, 4⟩, ⟨2, 4⟩]⟩ := by
  intro h
  unfold WF_Layout at h
  obtain ⟨h1, h2, h3⟩ := h
  have h_ne : (⟨0, 4⟩ : Field) ≠ ⟨2, 4⟩ := by
    intro heq
    injection heq with h1 h2
    omega
  have h_contra : Disjoint ⟨0, 4⟩ ⟨2, 4⟩ :=
    h2 ⟨0, 4⟩ ⟨2, 4⟩ (by simp) (by simp) h_ne
  unfold Disjoint at h_contra
  simp at h_contra

-- §5: Capacity check is monotonic in capacity.
example (used size : Nat) (cap1 cap2 : Nat)
    (hcap : cap1 ≤ cap2)
    (hcheck : used + size ≤ cap1) :
    used + size ≤ cap2 := by omega

-- §6: The verified_capacity_check matches the mathematical condition.
example (used size capacity : Nat) :
    PMT.Extraction.verified_capacity_check used size capacity = true
    ↔ used + size ≤ capacity := by
  unfold PMT.Extraction.verified_capacity_check
  simp

-- §7: The verified_field_bounds_check matches the mathematical condition.
example (f_offset f_size layout_total : Nat) :
    PMT.Extraction.verified_field_bounds_check ⟨f_offset, f_size⟩ ⟨layout_total, []⟩ = true
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
    (hwf : WF_Layout ⟨8, []⟩)
    (hlive : s.live "in" = Liveness.live)
    (_hfit : s.arena.used + 8 ≤ s.arena.capacity) :
    ∃ r, exec [⟨"in", "out", ⟨8, []⟩, .transform⟩] s = r
    ∧ (match r with
       | Result.ok fu => fu ≤ s.arena.capacity
       | Result.trap c => c = 1 ∨ c = 134 ∨ c = 135) := by
  -- Per-step hypothesis for pmt_soundness: every step in the (singleton)
  -- program has a well-formed layout and a live input variable.
  have hstep : ∀ st : Step, st ∈ [⟨"in", "out", ⟨8, []⟩, .transform⟩] →
                WF_Layout st.layout ∧ s.live st.in_var = Liveness.live := by
    intro st hst
    simp at hst
    rcases hst with rfl
    exact ⟨hwf, hlive⟩
  -- WellTypedness of the singleton program: layout WF (via `hwf`),
  -- plus name-uniqueness for `in_var` and `out_var` (each appears once).
  have hwf_prog : WellTyped [⟨"in", "out", ⟨8, []⟩, .transform⟩] := by
    unfold WellTyped
    refine ⟨?_, ?_, ?_⟩
    · intro st hst; simp at hst; rcases hst with rfl; exact hwf
    · intro st hst; simp at hst; rcases hst with rfl
      simp [List.filter]
    · intro st hst; simp at hst; rcases hst with rfl
      simp [List.filter]
  exact pmt_soundness _ hwf_prog s hstep hcap

end PMT.Test.PropertyTests
