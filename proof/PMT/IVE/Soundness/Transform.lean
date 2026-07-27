import PMT.PmtInstr
import PMT.IRProgram
import PMT.IVE.Soundness.WFLayoutBool

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

This module provides the `verify_transform` model. **As of IVE Wave 1
task A** (`task/ive-1-a`), the spec uses the computable
`wf_layout_bool` predicate (defined in
`PMT.IVE.Soundness.WFLayoutBool`) in place of the `Prop`-valued
`WF_Layout`. The well-formedness check is therefore constructively
decidable — no `Classical.propDecidable` is required — and
`verify_transform` is a plain `def` (not `noncomputable`). This
unblocks extraction (IVE-1-B) and the Iris soundness layer (PMT-1-G).

The `Decidable (verify_transform_spec t)` instance is provided
explicitly below: it unfolds the spec, case-splits on `t.kind`, and
in each branch the match reduces so `inferInstance` picks up the
auto-derived `Decidable` for the resulting `And` form (composed of
`instDecidableEqBool` for the `wf_layout_bool _ = true` conjuncts and
`DecidableEq Layout` / `DecidableEq Nat` for the per-kind equality
conjuncts).

The soundness theorems close without `sorry`:
`decide_eq_true_iff` bridges `decide (verify_transform_spec t) = true`
to `verify_transform_spec t`, the And conjuncts are extracted with
`obtain`, and `wf_layout_bool_iff_wf_layout` recovers `WF_Layout` from
each `wf_layout_bool _ = true` conjunct. The third conjunct's match
is left untouched (same form on both sides of the goal), so `exact h3`
closes it directly — no case-split on `t.kind` is needed.
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
  (1) both layouts are well-formed (encoded as `wf_layout_bool _ = true`
      so the spec is constructively decidable — see
      `PMT.IVE.Soundness.WFLayoutBool`),
  (2) for `Identity`, layouts match (same `total_size` + same `fields`),
  (3) for `Reinterpret`, `total_size` matches (same byte budget,
      different fields),
  (4) for `Copy`, no constraint (any pair of layouts accepted).

This is the Lean rendering of the four Rust checks performed in
`verify_transform` at `state_transform.rs:57` (modulo the
`HashMap<String, LayoutInfo>` lookup, which is collapsed into the
`WF_Layout` precondition via the caller-side `env : String → Layout`
pattern — see `PMT.IVE.Soundness.StateReads` for the same
simplification).

Replacing `WF_Layout` with `wf_layout_bool _ = true` (vs. the original
`WF_Layout _`) is the key change for IVE-1-A: it makes
`Decidable (verify_transform_spec t)` derive from
`instDecidableEqBool` (and `DecidableEq Layout` for the identity arm)
instead of `Classical.propDecidable`, so
`decide (verify_transform_spec t)` — and hence `verify_transform` —
is computable and survives extraction to Rust. -/
def verify_transform_spec (t : StateTransform) : Prop :=
  wf_layout_bool t.in_layout = true
  ∧ wf_layout_bool t.out_layout = true
  ∧ (match t.kind with
     | TransformKind.identity    => t.in_layout = t.out_layout
     | TransformKind.reinterpret => t.in_layout.total_size = t.out_layout.total_size
     | TransformKind.copy        => True)

/-- Constructive `Decidable` instance for `verify_transform_spec`.

This is what replaces `Classical.propDecidable _` from the pre-Wave-1
formulation. The proof unfolds the spec (exposing the `match t.kind`
syntactically), case-splits on `t.kind`, and in each branch the match
reduces so `inferInstance` picks up the auto-derived `Decidable` for
the resulting `And` form. The per-arm decidability rests on:
  - `instDecidableEqBool` for `wf_layout_bool _ = true`,
  - `DecidableEq Layout` (added in `WFLayoutBool.lean`) for
    `t.in_layout = t.out_layout` (identity arm),
  - `instDecidableEqNat` for `t.in_layout.total_size = t.out_layout.total_size`
    (reinterpret arm),
  - `True` is decidable (copy arm).

The instance is computable (no `Classical`), so `decide` reduces to
a `Bool` and `verify_transform` below is a plain `def`. -/
instance (t : StateTransform) : Decidable (verify_transform_spec t) := by
  unfold verify_transform_spec
  cases t.kind with
  | identity    => exact inferInstance
  | reinterpret => exact inferInstance
  | copy        => exact inferInstance

/-- The Lean model of IVE's `verify_transform`.
Returns `valid=true` iff the spec holds.

**Computable** (no `noncomputable`): the `Decidable` instance above is
constructive (case-split on `t.kind`, no `Classical.propDecidable`),
so `decide (verify_transform_spec t)` reduces to a `Bool` and this
`def` survives extraction to Rust (unblocks IVE-1-B and PMT-1-G). -/
def verify_transform (t : StateTransform) :
    StateTransformVerification :=
  { valid := decide (verify_transform_spec t),
    error := if decide (verify_transform_spec t)
             then none
             else some "transform invalid" }

/-- Soundness: if `verify_transform` returns `valid=true`, then both
layouts are well-formed and the kind constraint holds.

This is the Lean statement of "IVE's `verify_transform` is sound":
acceptance implies well-formedness + compatibility. The proof bridges
`(decide P) = true` to `P` via `decide_eq_true_iff` (the constructive
`Decidable` instance above comes from the spec's `Bool`-backed form,
not `Classical.propDecidable`), then unfolds the spec and uses
`wf_layout_bool_iff_wf_layout` to recover `WF_Layout` from each
`wf_layout_bool _ = true` conjunct.

The third conjunct (`match t.kind with ...`) has the same form in
both `hspec` and the goal, so `exact h3` closes it directly — no
case-split on `t.kind` is needed.

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
  -- `hverify` after `verify_transform` unfolds reduces to
  -- `decide (verify_transform_spec t) = true`. The `Decidable` instance
  -- is the constructive one above (no `Classical.propDecidable`).
  unfold verify_transform at hverify
  -- Bridge `(decide (verify_transform_spec t)) = true` to
  -- `verify_transform_spec t` via `decide_eq_true_iff` (the `Iff`
  -- form of `decide_eq_true_eq`).
  have hspec : verify_transform_spec t :=
    decide_eq_true_iff.mp hverify
  -- `hspec : wf_layout_bool t.in_layout = true
  --         ∧ wf_layout_bool t.out_layout = true
  --         ∧ match t.kind with ...`
  obtain ⟨h1, h2, h3⟩ := hspec
  refine ⟨?_, ?_, ?_⟩
  · -- `h1 : wf_layout_bool t.in_layout = true` → `WF_Layout t.in_layout`
    -- via `wf_layout_bool_iff_wf_layout`.
    exact (wf_layout_bool_iff_wf_layout t.in_layout).mp h1
  · -- `h2 : wf_layout_bool t.out_layout = true` → `WF_Layout t.out_layout`.
    exact (wf_layout_bool_iff_wf_layout t.out_layout).mp h2
  · -- `h3 : match t.kind with ...` — same form as the goal's third
    -- conjunct, so `exact h3` closes it directly. No case-split on
    -- `t.kind` is needed (which would also work but is unnecessary).
    exact h3

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
