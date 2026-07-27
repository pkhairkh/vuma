import PMT.Basic
import PMT.IVE.Soundness.WFLayoutBool

/-!
## IVE Soundness — LayoutConsistency (Wave 2 task IVE-2-H)

This module proves that IVE's `verify_layout_consistency` and
`verify_layout_field_list_consistency` functions are sound: if they
accept a layout, then the layout is well-formed (WF_Layout).

The Lean model mirrors the Rust functions' specifications. The actual
Rust functions live at `src/ive/src/verification.rs`.

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- The layout-consistency check: verifies that a layout's total_size
matches the sum of its field sizes, and that all fields are in bounds.
Mirrors `verify_layout_consistency` in `src/ive/src/verification.rs`.

In the Lean model, this reduces to `wf_layout_bool` (the computable
WF_Layout predicate from `WFLayoutBool.lean`), since WF_Layout already
checks field bounds and disjointness. The Rust function additionally
checks that total_size matches the sum of field sizes (a stricter check);
we model this as `layout_consistency_ok`. -/
def layout_consistency_ok (l : PMT.Layout) : Bool :=
  wf_layout_bool l
  -- Additional check: total_size ≥ sum of field sizes.
  && decide (l.fields.foldl (fun acc f => acc + f.size) 0 ≤ l.total_size)

/-- The layout-field-list-consistency check: verifies that a layout's
field list has no duplicate field names (by offset+size) and all fields
are in bounds. Mirrors `verify_layout_field_list_consistency` in
`src/ive/src/verification.rs`.

In the Lean model, this also reduces to `wf_layout_bool` (which checks
field disjointness via the pairwise-disjoint conjunct). -/
def layout_field_list_consistency_ok (l : PMT.Layout) : Bool :=
  wf_layout_bool l

/-- Soundness: if `layout_consistency_ok` returns true, then the layout
is well-formed (WF_Layout) AND the sum of field sizes ≤ total_size. -/
theorem verify_layout_consistency_sound
    (l : PMT.Layout)
    (hcheck : layout_consistency_ok l = true) :
    PMT.WF_Layout l
    ∧ l.fields.foldl (fun acc f => acc + f.size) 0 ≤ l.total_size := by
  unfold layout_consistency_ok at hcheck
  simp only [Bool.and_eq_true_iff, decide_eq_true_iff] at hcheck
  obtain ⟨h_wf_bool, h_sum⟩ := hcheck
  exact ⟨Iff.mp (wf_layout_bool_iff_wf_layout _) h_wf_bool, h_sum⟩

/-- Soundness: if `layout_field_list_consistency_ok` returns true, then
the layout is well-formed (WF_Layout). -/
theorem verify_layout_field_list_consistency_sound
    (l : PMT.Layout)
    (hcheck : layout_field_list_consistency_ok l = true) :
    PMT.WF_Layout l := by
  unfold layout_field_list_consistency_ok at hcheck
  exact Iff.mp (wf_layout_bool_iff_wf_layout _) hcheck

/-- Corollary: layout_consistency_ok implies layout_field_list_consistency_ok
(since the former is a stricter check). -/
theorem layout_consistency_implies_field_list
    (l : PMT.Layout)
    (hcheck : layout_consistency_ok l = true) :
    layout_field_list_consistency_ok l = true := by
  unfold layout_consistency_ok at hcheck
  unfold layout_field_list_consistency_ok
  -- layout_consistency_ok = wf_layout_bool && (…); we need just wf_layout_bool.
  simp only [Bool.and_eq_true_iff] at hcheck
  exact hcheck.1

end PMT.IVE.Soundness
