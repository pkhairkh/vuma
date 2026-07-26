import PMT.PmtInstr
import PMT.IRProgram

/-!
## IVE Soundness — verify_transform

This module proves that IVE's `verify_transform` function is sound:
if it accepts a transform (valid=true), then both the input and output
layouts are well-formed and compatible.

The Lean model mirrors the Rust function's specification. The actual
Rust function lives at `src/ive/src/state_transform.rs:57`.

Per the IVE deep-read:
  - The Rust `verify_transform` (`src/ive/src/state_transform.rs:57`)
    takes `&HashMap<String, LayoutInfo>` + a transform op, returns
    `StateTransformVerification`.
  - It classifies the transform (Identity/Reinterpret/Copy) and
    validates both layouts exist.
  - The "kill old state" semantics is NOT in IVE itself — it lives in
    `VerificationEngine::verify_pmt` at `verification.rs:474-506`,
    where `StateTransform` and `ForeignConsume` insert the input vreg
    into `consumed_vars`. There is no "produce new state" — the
    output side of `StateTransform` is recorded only as the
    `(input_layout, output_layout)` pair; the new state's vreg/name
    is untracked. The simulation relation must model the kill on input
    vregs only.

This module provides the `verify_transform` model. It
uses `Classical.propDecidable` to obtain `Decidable` instances
for `WF_Layout` and `Layout =` (both are `Prop`s over universally
quantified `Field`s, which `PMT.Basic` does not derive `DecidableEq`
for). This avoids introducing a second `DecidableEq Field` instance
that would conflict with the one defined in
`PMT.IVE.Soundness.StateReads` when the whole `PMT` library is
linked together. The actual theorems close without `sorry` — the
`decide P = true ↔ P` bridge via `decide_eq_true_eq` discharges
both soundness goals directly.
-/

namespace PMT.IVE.Soundness

/-- Transform kind: mirrors Rust `TransformKind`
(`src/ive/src/state_transform.rs`).
  - `identity`   : same layout, just rename the variable.
  - `reinterpret` : same bytes, different field structure.
  - `copy`        : copy bytes to a new layout. -/
inductive TransformKind where
  | identity    : TransformKind
  | reinterpret : TransformKind
  | copy        : TransformKind
  deriving Repr

/-- A state transform: consume `in_var`, produce `out_var`. Mirrors
the Rust `StateTransform` node payload
(`src/ive/src/verification.rs:474-506`), which synthesises a
`"_state_{node_id}_{vreg}"` name for the input and records the
`(input_layout, output_layout)` pair. The output vreg/name is
untracked in the Rust code path (W2-D §5 finding 3); the Lean model
carries `out_var` for documentation but does not impose a uniqueness
constraint on it. -/
structure StateTransform where
  in_var     : String
  out_var    : String
  in_layout  : Layout
  out_layout : Layout
  kind       : TransformKind
  deriving Repr

/-- The Lean model of IVE's `verify_transform` output.
Mirrors Rust `StateTransformVerification { valid, error }`
(`src/ive/src/state_transform.rs:36-49`). -/
structure StateTransformVerification where
  valid : Bool
  error : Option String
  deriving Repr

/-- The spec checked by `verify_transform`:
  (1) both layouts are well-formed (`WF_Layout`),
  (2) for `Identity`, layouts match (same `total_size` + same `fields`),
  (3) for `Reinterpret`, `total_size` matches (same byte budget,
      different fields),
  (4) for `Copy`, no constraint (any pair of layouts accepted).

This is the Lean rendering of the four Rust checks performed in
`verify_transform` at `state_transform.rs:57` (modulo the
`HashMap<String, LayoutInfo>` lookup, which is collapsed into the
`WF_Layout` precondition via the caller-side `env : String → Layout`
pattern — see `PMT.IVE.Soundness.StateReads` for the same
simplification). -/
def verify_transform_spec (t : StateTransform) : Prop :=
  WF_Layout t.in_layout
  ∧ WF_Layout t.out_layout
  ∧ (match t.kind with
     | TransformKind.identity    => t.in_layout = t.out_layout
     | TransformKind.reinterpret => t.in_layout.total_size = t.out_layout.total_size
     | TransformKind.copy        => True)

/-- The Lean model of IVE's `verify_transform`.
Returns `valid=true` iff the spec holds. Marked `noncomputable` because
the spec involves `WF_Layout` (a `Prop` with universal quantifiers
over `Field`), whose `Decidable` instance is provided by
`Classical.propDecidable` (not constructively computable in general).
The actual Rust function uses a `HashMap`-based lookup to discharge
the well-formedness check; this Lean model factors that into the
`WF_Layout` predicate.

The `Decidable` instance is supplied explicitly via
`@decide _ (Classical.propDecidable _)` rather than relying on instance
search — `Classical.propDecidable` is a low-priority instance and is
not always picked up automatically for `WF_Layout`'s universally
quantified conjunction. -/
noncomputable def verify_transform (t : StateTransform) :
    StateTransformVerification :=
  { valid := @decide (verify_transform_spec t) (Classical.propDecidable _),
    error := if @decide (verify_transform_spec t) (Classical.propDecidable _)
             then none
             else some "transform invalid" }

/-- Soundness: if `verify_transform` returns `valid=true`, then both
layouts are well-formed and the kind constraint holds.

This is the Lean statement of "IVE's `verify_transform` is sound":
acceptance implies well-formedness + compatibility. The proof bridges
`(decide P) = true` to `P` via `decide_eq_true_eq`, then closes by
definitional equality (`verify_transform_spec t` unfolds to the
goal's conjunction form).

**No `sorry`** — the theorem is closed. -/
theorem verify_transform_sound
    (t : StateTransform)
    (hverify : (verify_transform t).valid = true) :
    WF_Layout t.in_layout
    ∧ WF_Layout t.out_layout
    ∧ (match t.kind with
       | TransformKind.identity    => t.in_layout = t.out_layout
       | TransformKind.reinterpret => t.in_layout.total_size = t.out_layout.total_size
       | TransformKind.copy        => True) := by
  unfold verify_transform at hverify
  -- `hverify : decide (verify_transform_spec t) = true`, where the
  -- `Decidable` instance is `Classical.propDecidable _` (provided
  -- explicitly in `verify_transform`). `decide_eq_true_eq.mp` then
  -- extracts the propositional content; we pass the instance
  -- explicitly via `@decide_eq_true_eq _ (Classical.propDecidable _)`
  -- because instance search does not pick up `Classical.propDecidable`
  -- for `WF_Layout`'s universally quantified conjunction.
  exact (@decide_eq_true_eq _ (Classical.propDecidable _)).mp hverify

/-- Corollary: `verify_transform` preserves layout well-formedness.
This is the "no ill-formed layout slips through IVE's transform check"
guarantee, mirroring the Rust-side contract that `verify_transform`
rejects transforms whose `input_layout` or `output_layout` is not in
the `HashMap<String, LayoutInfo>` of registered layouts.

**No `sorry`** — the corollary follows directly from
`verify_transform_sound`. -/
theorem verify_transform_preserves_wf
    (t : StateTransform)
    (hverify : (verify_transform t).valid = true) :
    WF_Layout t.in_layout ∧ WF_Layout t.out_layout := by
  have h := verify_transform_sound t hverify
  exact ⟨h.1, h.2.1⟩

end PMT.IVE.Soundness
