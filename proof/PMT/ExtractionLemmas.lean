import PMT.Extraction
import PMT.WellTypedStrong

/-! ## Extraction Lemmas — Composition Results

This module proves composition lemmas that combine the 4 verified checkers
from `PMT.Extraction` into more complex properties. These lemmas are the
basis for the extraction pipeline: they show that the
extracted Rust functions preserve the PMT invariants when composed.

All theorems in this file close without `sorry`.

**PMT-FAITH-5-C:** the lemmas that call `verified_capacity_check` now use
`BitVec.ofNat 64` to convert Nat args to BitVec 64 (matching the bit-faithful
signature). Boundedness hypotheses (`< 2^64`) are added where the Nat-level
conclusion is needed (to ensure the conversion is lossless).
-/

namespace PMT.Extraction

/-- §1: If the capacity check passes for a sequence of allocations,
the total allocated bytes fit in the arena. -/
theorem capacity_check_sequential
    (used0 size1 size2 capacity : Nat)
    (h1 : verified_capacity_check (BitVec.ofNat 64 used0) (BitVec.ofNat 64 size1) (BitVec.ofNat 64 capacity) = true)
    (h2 : verified_capacity_check (BitVec.ofNat 64 (used0 + size1)) (BitVec.ofNat 64 size2) (BitVec.ofNat 64 capacity) = true)
    (h_used0 : used0 < 2^64) (h_size1 : size1 < 2^64) (h_size2 : size2 < 2^64)
    (h_cap : capacity < 2^64) (h_no_ovf1 : used0 + size1 < 2^64) :
    used0 + size1 + size2 ≤ capacity := by
  have h1' := verified_capacity_check_correct (BitVec.ofNat 64 used0) (BitVec.ofNat 64 size1) (BitVec.ofNat 64 capacity) h1
  have h2' := verified_capacity_check_correct (BitVec.ofNat 64 (used0 + size1)) (BitVec.ofNat 64 size2) (BitVec.ofNat 64 capacity) h2
  -- Extract the no-overflow + bounds conjuncts, convert to Nat via boundedness.
  have h1_noovf := h1'.1
  have h1_sum := h1'.2
  have h2_noovf := h2'.1
  have h2_sum := h2'.2
  -- Convert ofNat.toNat = identity (under boundedness).
  have eq_used0 : (BitVec.ofNat 64 used0).toNat = used0 := by rw [BitVec.toNat_ofNat]; omega
  have eq_size1 : (BitVec.ofNat 64 size1).toNat = size1 := by rw [BitVec.toNat_ofNat]; omega
  have eq_size2 : (BitVec.ofNat 64 size2).toNat = size2 := by rw [BitVec.toNat_ofNat]; omega
  have eq_cap : (BitVec.ofNat 64 capacity).toNat = capacity := by rw [BitVec.toNat_ofNat]; omega
  have eq_sum1 : (BitVec.ofNat 64 used0 + BitVec.ofNat 64 size1).toNat = used0 + size1 := by
    rw [BitVec.toNat_add, eq_used0, eq_size1]; omega
  have eq_sum01 : (BitVec.ofNat 64 (used0 + size1)).toNat = used0 + size1 := by rw [BitVec.toNat_ofNat]; omega
  -- Convert h1_sum and h2_sum to Nat level.
  rw [eq_sum1, eq_cap] at h1_sum
  rw [BitVec.toNat_add, eq_sum01, eq_size2] at h2_sum
  rw [eq_cap] at h2_sum
  -- h1_sum : used0 + size1 ≤ capacity
  -- h2_sum : (used0 + size1 + size2) % 2^64 ≤ capacity
  -- Under h_no_ovf1 + h2_noovf, the modulo is identity.
  -- Extract h2_noovf to Nat level.
  rw [eq_sum01, eq_size2, BitVec.toNat_allOnes] at h2_noovf
  -- h2_noovf : size2 ≤ 2^64 - 1 - (used0 + size1)  → used0 + size1 + size2 < 2^64
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
the three sub-checks.

**PMT-FAITH-5-C:** `verified_pmt_check` now converts Nat→BitVec 64 internally
for the capacity check. The decomposition reflects this: the capacity
sub-check is over BitVec 64 (with `ofNat` conversion), while the field-bounds
and linearity sub-checks remain Nat/String-based (unchanged). -/
theorem pmt_check_decomposes
    (used capacity : Nat)
    (f : Field) (layout : Layout)
    (var : String) (consumed : List String) :
    verified_pmt_check used capacity f layout var consumed = true
    ↔ verified_capacity_check (BitVec.ofNat 64 used) (BitVec.ofNat 64 layout.total_size) (BitVec.ofNat 64 capacity) = true
      ∧ verified_field_bounds_check f layout = true
      ∧ verified_linearity_check var consumed = true := by
  unfold verified_pmt_check verified_capacity_check verified_field_bounds_check
         verified_linearity_check
  simp [Bool.and_eq_true_iff, decide_eq_true_iff]

/-- §5: If the composed PMT check passes, all three invariants hold. -/
theorem pmt_check_implies_all_invariants
    (used capacity : Nat)
    (f : Field) (layout : Layout)
    (var : String) (consumed : List String)
    (hcheck : verified_pmt_check used capacity f layout var consumed = true)
    (h_used : used < 2^64) (h_size : layout.total_size < 2^64) (h_cap : capacity < 2^64)
    (h_no_ovf : used + layout.total_size < 2^64) :
    used + layout.total_size ≤ capacity
    ∧ f.offset + f.size ≤ layout.total_size
    ∧ var ∉ consumed := by
  rw [pmt_check_decomposes] at hcheck
  obtain ⟨hcap, hfb, hlin⟩ := hcheck
  refine ⟨?_, ?_, ?_⟩
  · -- Derive Nat-level bound from BitVec-level check.
    have hbv := verified_capacity_check_correct (BitVec.ofNat 64 used) (BitVec.ofNat 64 layout.total_size) (BitVec.ofNat 64 capacity) hcap
    have eq_used : (BitVec.ofNat 64 used).toNat = used := by rw [BitVec.toNat_ofNat]; omega
    have eq_size : (BitVec.ofNat 64 layout.total_size).toNat = layout.total_size := by rw [BitVec.toNat_ofNat]; omega
    have eq_cap : (BitVec.ofNat 64 capacity).toNat = capacity := by rw [BitVec.toNat_ofNat]; omega
    have eq_sum : (BitVec.ofNat 64 used + BitVec.ofNat 64 layout.total_size).toNat = used + layout.total_size := by
      rw [BitVec.toNat_add, eq_used, eq_size]; omega
    rw [eq_sum, eq_cap] at hbv
    exact hbv.2
  · exact verified_field_bounds_check_correct f layout hfb
  · exact verified_linearity_check_correct var consumed hlin

/-- §6: The composed check is monotonic — if it passes for a larger arena,
it passes for a smaller one (with the same used). -/
theorem pmt_check_monotonic_capacity
    (used cap1 cap2 : Nat)
    (f : Field) (layout : Layout)
    (var : String) (consumed : List String)
    (h_used : used < 2^64) (h_size : layout.total_size < 2^64)
    (h_cap1 : cap1 < 2^64) (h_cap2 : cap2 < 2^64)
    (h_no_ovf : used + layout.total_size < 2^64)
    (_hcap2 : cap2 ≤ cap1)
    (hcheck : verified_pmt_check used cap1 f layout var consumed = true) :
    verified_pmt_check used cap2 f layout var consumed = true ∨ used + layout.total_size > cap2 := by
  rw [pmt_check_decomposes] at hcheck
  obtain ⟨hcap, hfb, hlin⟩ := hcheck
  have hbv := verified_capacity_check_correct (BitVec.ofNat 64 used) (BitVec.ofNat 64 layout.total_size) (BitVec.ofNat 64 cap1) hcap
  have eq_used : (BitVec.ofNat 64 used).toNat = used := by rw [BitVec.toNat_ofNat]; omega
  have eq_size : (BitVec.ofNat 64 layout.total_size).toNat = layout.total_size := by rw [BitVec.toNat_ofNat]; omega
  have eq_cap1 : (BitVec.ofNat 64 cap1).toNat = cap1 := by rw [BitVec.toNat_ofNat]; omega
  have eq_sum : (BitVec.ofNat 64 used + BitVec.ofNat 64 layout.total_size).toNat = used + layout.total_size := by
    rw [BitVec.toNat_add, eq_used, eq_size]; omega
  have hcap_correct : used + layout.total_size ≤ cap1 := by
    rw [eq_sum, eq_cap1] at hbv; exact hbv.2
  by_cases hfit : used + layout.total_size ≤ cap2
  · left
    rw [pmt_check_decomposes]
    refine ⟨?_, hfb, hlin⟩
    · unfold verified_capacity_check
      rw [decide_eq_true_iff]
      refine ⟨?_, ?_⟩
      · -- no_overflow: size ≤ usizeMax - used
        rw [BitVec.le_def]
        have eq_size' : (BitVec.ofNat 64 layout.total_size).toNat = layout.total_size := eq_size
        have eq_used' : (BitVec.ofNat 64 used).toNat = used := eq_used
        rw [BitVec.toNat_sub, BitVec.toNat_allOnes, eq_size', eq_used']
        omega
      · -- sum ≤ capacity
        rw [BitVec.le_def, BitVec.toNat_add, eq_used, eq_size, BitVec.toNat_ofNat]
        omega
  · right
    omega

end PMT.Extraction
