import PMT.Basic
import PMT.IVE.Soundness.Transform

/-!
## IVE Soundness — DependentTransform (Wave 2 task IVE-2-F)

This module proves that IVE's `verify_dependent_transform` function is
sound: if it accepts a dependent transform (where the output layout
depends on a runtime value), then both layouts are well-formed and the
dependency is within bounds.

The Lean model mirrors the Rust function's specification. The actual Rust
function lives at `src/ive/src/state_transform.rs::verify_dependent_transform`.

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- A dependent transform: like a StateTransform, but the output layout's
size depends on a runtime value (`dep_value`). Mirrors the Rust
`verify_dependent_transform` input. -/
structure DependentTransform where
  in_layout  : PMT.Layout
  out_layout : PMT.Layout
  dep_value  : Nat  -- runtime value that determines the actual output size
  deriving Repr

/-- The dependent-transform verification result. -/
structure DependentTransformVerification where
  valid : Bool
  error : Option String
  deriving Repr

/-- The dependent-transform check: both layouts must be well-formed, AND
the dependency value must fit within the output layout's total_size
(i.e., `dep_value ≤ out_layout.total_size`). This is the Presburger
bounds check from the Rust side. -/
def dependent_transform_ok (t : DependentTransform) : Bool :=
  wf_layout_bool t.in_layout
  && wf_layout_bool t.out_layout
  && decide (t.dep_value ≤ t.out_layout.total_size)

/-- The Lean model of IVE's `verify_dependent_transform`. -/
def verify_dependent_transform (t : DependentTransform) : DependentTransformVerification :=
  let ok := dependent_transform_ok t
  { valid := ok,
    error := if ok then none else some "dependent transform invalid" }

/-- Soundness: if `verify_dependent_transform` returns `valid = true`,
then both layouts are well-formed and the dependency fits. -/
theorem verify_dependent_transform_sound
    (t : DependentTransform)
    (hverify : (verify_dependent_transform t).valid = true) :
    PMT.WF_Layout t.in_layout
    ∧ PMT.WF_Layout t.out_layout
    ∧ t.dep_value ≤ t.out_layout.total_size := by
  unfold verify_dependent_transform dependent_transform_ok at hverify
  simp only [Bool.and_eq_true_iff, decide_eq_true_iff] at hverify
  obtain ⟨⟨h_in_wf, h_out_wf⟩, h_dep⟩ := hverify
  -- h_in_wf : wf_layout_bool t.in_layout = true
  -- Use the wf_layout_bool_iff_wf_layout bridge to recover WF_Layout.
  exact ⟨Iff.mp (wf_layout_bool_iff_wf_layout _) h_in_wf,
         Iff.mp (wf_layout_bool_iff_wf_layout _) h_out_wf,
         h_dep⟩

end PMT.IVE.Soundness
