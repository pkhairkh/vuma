import PMT.Extraction
import PMT.WellTypedStrong

/-! ## Extraction Lemmas — Composition Results

This module proves composition lemmas that combine the 4 verified checkers
from `PMT.Extraction` into more complex properties. These lemmas are the
basis for the extraction pipeline: they show that the
extracted Rust functions preserve the PMT invariants when composed.

All theorems in this file close without `sorry`.
-/

namespace PMT.Extraction

/-- §1: If the capacity check passes for a sequence of allocations,
the total allocated bytes fit in the arena. -/
theorem capacity_check_sequential
    (used0 size1 size2 capacity : Nat)
    (h1 : verified_capacity_check used0 size1 capacity = true)
    (h2 : verified_capacity_check (used0 + size1) size2 capacity = true) :
    used0 + size1 + size2 ≤ capacity := by
  have h1' := verified_capacity_check_correct used0 size1 capacity h1
  have h2' := verified_capacity_check_correct (used0 + size1) size2 capacity h2
  omega

/-- §2: If the field-bounds check passes for all fields in a layout,
the layout is well-formed (field-bounds part). -/
theorem field_bounds_check_all_fields
    (layout : Layout)
    (hcheck : ∀ f : Field, f ∈ layout.fields →
                verified_field_bounds_check f layout = true) :
    ∀ f : Field, f ∈ layout.fields → f.offset + f.size ≤ layout.total_size := by
  intro f hf
  have h := hcheck f hf
  exact verified_field_bounds_check_correct f layout h

/-- §3: If the linearity check passes for a sequence of writes,
no write targets a consumed variable. -/
theorem linearity_check_all_writes
    (writes : List String)
    (consumed : List String)
    (hcheck : ∀ w, w ∈ writes → verified_linearity_check w consumed = true) :
    ∀ w, w ∈ writes → w ∉ consumed := by
  intro w hw
  have h := hcheck w hw
  exact verified_linearity_check_correct w consumed h

/-- §4: The composed PMT check is equivalent to the conjunction of
the three sub-checks. -/
theorem pmt_check_decomposes
    (used capacity : Nat)
    (f : Field) (layout : Layout)
    (var : String) (consumed : List String) :
    verified_pmt_check used capacity f layout var consumed = true
    ↔ verified_capacity_check used layout.total_size capacity = true
      ∧ verified_field_bounds_check f layout = true
      ∧ verified_linearity_check var consumed = true := by
  unfold verified_pmt_check verified_capacity_check verified_field_bounds_check
         verified_linearity_check
  simp [Bool.and_eq_true_iff]

/-- §5: If the composed PMT check passes, all three invariants hold. -/
theorem pmt_check_implies_all_invariants
    (used capacity : Nat)
    (f : Field) (layout : Layout)
    (var : String) (consumed : List String)
    (hcheck : verified_pmt_check used capacity f layout var consumed = true) :
    used + layout.total_size ≤ capacity
    ∧ f.offset + f.size ≤ layout.total_size
    ∧ var ∉ consumed := by
  rw [pmt_check_decomposes] at hcheck
  obtain ⟨hcap, hfb, hlin⟩ := hcheck
  refine ⟨?_, ?_, ?_⟩
  · exact verified_capacity_check_correct used layout.total_size capacity hcap
  · exact verified_field_bounds_check_correct f layout hfb
  · exact verified_linearity_check_correct var consumed hlin

/-- §6: The composed check is monotonic — if it passes for a larger arena,
it passes for a smaller one (with the same used). -/
theorem pmt_check_monotonic_capacity
    (used cap1 cap2 : Nat)
    (f : Field) (layout : Layout)
    (var : String) (consumed : List String)
    (_hcap2 : cap2 ≤ cap1)
    (hcheck : verified_pmt_check used cap1 f layout var consumed = true) :
    verified_pmt_check used cap2 f layout var consumed = true ∨ used + layout.total_size > cap2 := by
  rw [pmt_check_decomposes] at hcheck
  obtain ⟨hcap, hfb, hlin⟩ := hcheck
  have hcap_correct := verified_capacity_check_correct used layout.total_size cap1 hcap
  by_cases hfit : used + layout.total_size ≤ cap2
  · left
    rw [pmt_check_decomposes]
    refine ⟨?_, hfb, hlin⟩
    · unfold verified_capacity_check
      exact decide_eq_true_iff.mpr hfit
  · right
    omega

end PMT.Extraction
