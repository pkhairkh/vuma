import PMT.IVE.Soundness.Transform
import PMT.IVE.Soundness.StateReads
import PMT.IVE.Soundness.StateWrites

/-!
## IVE Soundness — Composition (FAITHFUL model, Wave 6 task IVE-FAITH-6-E)

Updated to use the faithful StateRead/StateWrite API (field_name strings,
LayoutInfo env, no separate field_types map).

This module is `sorry`-free.
-/

/-!
## Known Tautological Restatements (Round 7 verified)

The following three theorems in this file are TAUTOLOGICAL restatements of
`FullyVerified` structure fields — they prove nothing beyond what the
structure's constructors already assume. They have been renamed with the
`_restat` suffix to make the restatement nature explicit. The genuine
memory-safety content of the IVE pipeline lives elsewhere:

  * `no_oob_trap_for_well_typed_strong` (PMT/IVE/Soundness/WellTypedStrong.lean)
    is the real out-of-bounds-trap guarantee.
  * `pmt_soundness` (PMT/PipelineSim.lean) is the real end-to-end soundness
    theorem linking the IVE verifiers to execution traps.

Do NOT cite the three `_restat` theorems below as the safety guarantee.

  1. `fully_verified_implies_pmt_invariants_restat` (below):
     each conjunct is discharged by `exact hfv.reads_ok` /
     `exact hfv.writes_ok` / `hfv.transforms_ok` — i.e. the conclusion
     restates the `FullyVerified` fields; the third conjunct additionally
     relies on `verify_transform_sound`, which is the soundness contract of
     `verify_transform`, not new composition content.
  2. `fully_verified_no_memory_safety_traps_restat` (below):
     proof body is `exact hfv.reads_ok` and `exact hfv.writes_ok` — the two
     conjuncts of the conclusion are *literally* the two `FullyVerified`
     fields. This is the canonical tautology identified in Round 7.
  3. `fully_verified_no_uaf_including_foreign_consumes_restat` (below):
     proof body is `exact hfv.writes_ok`; the `foreign_consumes` parameter
     and the `h_merge : ∀ v, v ∈ foreign_consumes → v ∈ consumed` hypothesis
     are NEVER referenced (now underscore-prefixed `_foreign_consumes` /
     `_h_merge` to make the unused-ness explicit). Despite its "Gap 5 closure:
     no UAF including foreign consumes" framing, it closes no gap — it is a
     pure restatement of `FullyVerified.writes_ok`.
-/

namespace PMT.IVE.Soundness

/-- A program is "fully verified" if all three IVE verifiers accept it.
Uses the faithful model: env is String → Option LayoutInfo (carrying
FieldInfo with name/offset/size/type_name), no separate field_types. -/
structure FullyVerified
    (env : String → Option LayoutInfo)
    (layouts : LayoutRegistry)
    (consumed : List String)
    (foreign_consumes : List String)
    (reads : List StateRead)
    (writes : List StateWrite)
    (transforms : List StateTransform) : Prop where
  reads_ok      : ∀ v, v ∈ verify_state_reads env reads → v.valid = true
  writes_ok     : ∀ v, v ∈ verify_state_writes env consumed writes → v.valid = true
  transforms_ok : ∀ t, t ∈ transforms → (verify_transform_st layouts t).valid = true

/-- This theorem restates FullyVerified fields; the genuine memory-safety
content lives in no_oob_trap_for_well_typed_strong (WellTypedStrong.lean) and
pmt_soundness (PipelineSim.lean). Do not cite this as the safety guarantee.

(Renamed from `fully_verified_implies_pmt_invariants` in Wave 1-A to make the
restatement nature explicit. Each conjunct is discharged by re-invoking the
corresponding `FullyVerified` field; the third conjunct additionally unfolds
`verify_transform_sound`, which is the soundness contract of `verify_transform`,
not new composition content.) -/
theorem fully_verified_implies_pmt_invariants_restat
    (env : String → Option LayoutInfo)
    (layouts : LayoutRegistry)
    (consumed : List String)
    (foreign_consumes : List String)
    (reads : List StateRead)
    (writes : List StateWrite)
    (transforms : List StateTransform)
    (hfv : FullyVerified env layouts consumed foreign_consumes reads writes transforms) :
    -- All reads pass verification.
    (∀ r : StateRead, r ∈ reads →
      (∀ v : StateReadVerification, v ∈ verify_state_reads env reads → v.valid = true))
    -- All writes pass verification.
    ∧ (∀ w : StateWrite, w ∈ writes →
      (∀ v : StateWriteVerification, v ∈ verify_state_writes env consumed writes → v.valid = true))
    -- All transforms have both layouts existing.
    ∧ (∀ t : StateTransform, t ∈ transforms →
      (∃ in_info, layouts t.input_layout = some in_info)
      ∧ (∃ out_info, layouts t.output_layout = some out_info)) := by
  refine ⟨?_, ?_, ?_⟩
  · intro r hr v hv
    exact hfv.reads_ok v hv
  · intro w hw v hv
    exact hfv.writes_ok v hv
  · intro t ht
    have h := hfv.transforms_ok t ht
    have h_sound := verify_transform_sound layouts t.input_layout t.output_layout h
    exact ⟨h_sound.1, h_sound.2.1⟩

/-- This theorem restates FullyVerified fields; the genuine memory-safety
content lives in no_oob_trap_for_well_typed_strong (WellTypedStrong.lean) and
pmt_soundness (PipelineSim.lean). Do not cite this as the safety guarantee.

(Renamed from `fully_verified_no_memory_safety_traps` in Wave 1-A. This is the
canonical tautology identified in Round 7: both conjuncts of the conclusion
are discharged by `exact hfv.reads_ok` and `exact hfv.writes_ok`, i.e. they
are *literally* the two `FullyVerified` fields.) -/
theorem fully_verified_no_memory_safety_traps_restat
    (env : String → Option LayoutInfo)
    (layouts : LayoutRegistry)
    (consumed : List String)
    (foreign_consumes : List String)
    (reads : List StateRead)
    (writes : List StateWrite)
    (transforms : List StateTransform)
    (hfv : FullyVerified env layouts consumed foreign_consumes reads writes transforms) :
    (∀ v : StateReadVerification, v ∈ verify_state_reads env reads → v.valid = true)
    ∧ (∀ v : StateWriteVerification, v ∈ verify_state_writes env consumed writes → v.valid = true) := by
  refine ⟨?_, ?_⟩
  · exact hfv.reads_ok
  · exact hfv.writes_ok

/-- This theorem restates FullyVerified fields; the genuine memory-safety
content lives in no_oob_trap_for_well_typed_strong (WellTypedStrong.lean) and
pmt_soundness (PipelineSim.lean). Do not cite this as the safety guarantee.

(Renamed from `fully_verified_no_uaf_including_foreign_consumes` in Wave 1-A.
Despite its "Gap 5 closure: no UAF including foreign consumes" framing, the
proof body is `exact hfv.writes_ok` — a pure restatement of
`FullyVerified.writes_ok`. The `foreign_consumes` parameter and the
`h_merge : ∀ v, v ∈ foreign_consumes → v ∈ consumed` hypothesis are NEVER
referenced; they are now underscore-prefixed (`_foreign_consumes` / `_h_merge`)
to make the unused-ness explicit. This theorem closes no gap.) -/
theorem fully_verified_no_uaf_including_foreign_consumes_restat
    (env : String → Option LayoutInfo)
    (layouts : LayoutRegistry)
    (consumed : List String)
    (_foreign_consumes : List String)
    (reads : List StateRead)
    (writes : List StateWrite)
    (transforms : List StateTransform)
    (hfv : FullyVerified env layouts consumed _foreign_consumes reads writes transforms)
    (_h_merge : ∀ v, v ∈ _foreign_consumes → v ∈ consumed) :
    ∀ v : StateWriteVerification, v ∈ verify_state_writes env consumed writes → v.valid = true := by
  exact hfv.writes_ok

end PMT.IVE.Soundness
