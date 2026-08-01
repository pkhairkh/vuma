import PMT.PmtInstr
import PMT.IRProgram
import PMT.IVE.Soundness.WFLayoutBool

/-!
## IVE Soundness — verify_transform (FAITHFUL model, Wave 5 task IVE-FAITH-5-A)

This module is a **bit-faithful** Lean rendering of the Rust function
`src/ive/src/state_transform.rs::verify_transform`. It replaces the
previous (unfaithful) model that took `Layout` structs and checked
`WF_Layout`. The Rust function does NOT check `WF_Layout`; it checks
layout-existence (by name) and infers the transform kind from name/size
comparison.

**Rust reference** (`src/ive/src/state_transform.rs::verify_transform`):
```rust
pub fn verify_transform(
    layouts: &HashMap<String, LayoutInfo>,
    input_layout: &str,
    output_layout: &str,
) -> StateTransformVerification
```

**Rust logic** (faithfully mirrored below):
  1. Look up `input_layout` by NAME in `layouts` (not-found → invalid, kind=Copy).
  2. Look up `output_layout` by NAME (not-found → invalid, kind=Copy).
  3. If `input_layout == output_layout` (STRING equality) → Identity, valid.
  4. If `in_info.total_size == out_info.total_size` → Reinterpret, valid.
  5. Otherwise → Copy, valid.
  - Does NOT check `WF_Layout` for either layout.

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- FieldInfo mirroring Rust `src/ive/src/state_transform.rs::FieldInfo`:
`{name : String, offset : u64, size : u64, type_name : String}`. -/
structure FieldInfo where
  name      : String
  offset    : Nat
  size      : Nat
  type_name : String
  deriving Repr

/-- LayoutInfo mirroring Rust `src/ive/src/state_transform.rs::LayoutInfo`:
`{name : String, total_size : u64, fields : Vec<FieldInfo>}`. -/
structure LayoutInfo where
  name       : String
  total_size : Nat
  fields     : List FieldInfo
  deriving Repr

/-- A layout registry: maps layout names to LayoutInfo.
Models Rust's `HashMap<String, LayoutInfo>`. -/
def LayoutRegistry := String → Option LayoutInfo

/-- TransformKind mirroring Rust `TransformKind` (Reinterpret, Copy, Identity). -/
inductive TransformKind where
  | reinterpret : TransformKind
  | copy        : TransformKind
  | identity    : TransformKind
  deriving Repr

/-- StateTransformVerification mirroring Rust `StateTransformVerification`:
`{input_layout: String, output_layout: String, valid: bool, transform_kind: TransformKind, error: Option<String>}`. -/
structure StateTransformVerification where
  input_layout   : String
  output_layout  : String
  valid          : Bool
  transform_kind : TransformKind
  error          : Option String
  deriving Repr

/-- A state transform: identified by a pair of layout names.
Mirrors Rust's `&[(String, String)]` in `verify_all_transforms` — Rust does
NOT have a `StateTransform` struct carrying Layout values; it uses layout
NAME pairs. This structure is a named wrapper around the pair, for use in
Composition.lean and Extraction.lean. -/
structure StateTransform where
  input_layout  : String
  output_layout : String
  deriving Repr

/-- The Lean model of IVE's `verify_transform`. **Faithful** to the Rust
function at `src/ive/src/state_transform.rs::verify_transform`:
  1. Look up `input_layout` by NAME (not-found → invalid, kind=Copy).
  2. Look up `output_layout` by NAME (not-found → invalid, kind=Copy).
  3. If `input_layout == output_layout` (STRING equality) → Identity, valid.
  4. If `in_info.total_size == out_info.total_size` → Reinterpret, valid.
  5. Otherwise → Copy, valid.
  - Does NOT check `WF_Layout` (Rust doesn't). -/
def verify_transform
    (layouts : LayoutRegistry)
    (input_layout output_layout : String) :
    StateTransformVerification :=
  match layouts input_layout with
  | none =>
    { input_layout := input_layout,
      output_layout := output_layout,
      valid := false,
      transform_kind := TransformKind.copy,
      error := some ("input layout '" ++ input_layout ++ "' not found") }
  | some in_info =>
    match layouts output_layout with
    | none =>
      { input_layout := input_layout,
        output_layout := output_layout,
        valid := false,
        transform_kind := TransformKind.copy,
        error := some ("output layout '" ++ output_layout ++ "' not found") }
    | some out_info =>
      -- Step 3: Identity (STRING equality, not structural Layout equality).
      if input_layout = output_layout then
        { input_layout := input_layout,
          output_layout := output_layout,
          valid := true,
          transform_kind := TransformKind.identity,
          error := none }
      -- Step 4: Reinterpret (same total_size).
      else if in_info.total_size = out_info.total_size then
        { input_layout := input_layout,
          output_layout := output_layout,
          valid := true,
          transform_kind := TransformKind.reinterpret,
          error := none }
      -- Step 5: Copy (different sizes — always valid).
      else
        { input_layout := input_layout,
          output_layout := output_layout,
          valid := true,
          transform_kind := TransformKind.copy,
          error := none }

/-- Convenience wrapper: verify a `StateTransform` (pair of layout names). -/
def verify_transform_st (layouts : LayoutRegistry) (t : StateTransform) :
    StateTransformVerification :=
  verify_transform layouts t.input_layout t.output_layout

/-- Verify all transforms in a list. Mirrors Rust's `verify_all_transforms`
which takes `&[(String, String)]` and maps `verify_transform` over each pair. -/
def verify_all_transforms (layouts : LayoutRegistry) (transforms : List StateTransform) :
    List StateTransformVerification :=
  transforms.map (verify_transform_st layouts)

/-- Soundness: if `verify_transform` returns `valid = true`, then:
  (1) The input layout exists in the registry.
  (2) The output layout exists in the registry.
  (3) The `transform_kind` is determined by the faithful rules:
      - `identity` iff `input_layout = output_layout` (string equality).
      - `reinterpret` iff names differ AND total_sizes match.
      - `copy` iff names differ AND total_sizes differ.

This is the Lean rendering of the soundness obligation for
`src/ive/src/state_transform.rs::verify_transform`. **No `WF_Layout`
conclusion** — Rust does not check it. -/
theorem verify_transform_sound
    (layouts : LayoutRegistry)
    (input_layout output_layout : String)
    (hverify : (verify_transform layouts input_layout output_layout).valid = true) :
    (∃ in_info, layouts input_layout = some in_info)
    ∧ (∃ out_info, layouts output_layout = some out_info)
    ∧ ((input_layout = output_layout
        ∧ (verify_transform layouts input_layout output_layout).transform_kind = TransformKind.identity)
      ∨ (∃ in_info out_info,
          layouts input_layout = some in_info
          ∧ layouts output_layout = some out_info
          ∧ input_layout ≠ output_layout
          ∧ in_info.total_size = out_info.total_size
          ∧ (verify_transform layouts input_layout output_layout).transform_kind = TransformKind.reinterpret)
      ∨ (∃ in_info out_info,
          layouts input_layout = some in_info
          ∧ layouts output_layout = some out_info
          ∧ input_layout ≠ output_layout
          ∧ in_info.total_size ≠ out_info.total_size
          ∧ (verify_transform layouts input_layout output_layout).transform_kind = TransformKind.copy)) := by
  by_cases h_in_none : layouts input_layout = none
  · -- layouts input_layout = none → verify_transform returns valid=false → contradiction.
    have h_val : (verify_transform layouts input_layout output_layout).valid = false := by
      unfold verify_transform
      rw [h_in_none]
    rw [h_val] at hverify
    exact absurd hverify (by decide : ¬ (false = true))
  · obtain ⟨in_info, h_in'⟩ := Option.ne_none_iff_exists.mp h_in_none
    have h_in : layouts input_layout = some in_info := h_in'.symm
    by_cases h_out_none : layouts output_layout = none
    · have h_val : (verify_transform layouts input_layout output_layout).valid = false := by
        unfold verify_transform
        rw [h_in, h_out_none]
      rw [h_val] at hverify
      exact absurd hverify (by decide : ¬ (false = true))
    · obtain ⟨out_info, h_out'⟩ := Option.ne_none_iff_exists.mp h_out_none
      have h_out : layouts output_layout = some out_info := h_out'.symm
      by_cases h_eq : input_layout = output_layout
      · -- Identity branch.
        have h_kind : (verify_transform layouts input_layout output_layout).transform_kind = TransformKind.identity := by
          unfold verify_transform
          simp only [h_in, h_out, if_pos h_eq]
        refine ⟨⟨in_info, h_in⟩, ⟨out_info, h_out⟩, ?_⟩
        left
        exact ⟨h_eq, h_kind⟩
      · by_cases h_size : in_info.total_size = out_info.total_size
        · -- Reinterpret branch.
          have h_kind : (verify_transform layouts input_layout output_layout).transform_kind = TransformKind.reinterpret := by
            unfold verify_transform
            simp only [h_in, h_out, if_neg h_eq, if_pos h_size]
          refine ⟨⟨in_info, h_in⟩, ⟨out_info, h_out⟩, ?_⟩
          right
          left
          exact ⟨in_info, out_info, h_in, h_out, h_eq, h_size, h_kind⟩
        · -- Copy branch.
          have h_kind : (verify_transform layouts input_layout output_layout).transform_kind = TransformKind.copy := by
            unfold verify_transform
            simp only [h_in, h_out, if_neg h_eq, if_neg h_size]
          refine ⟨⟨in_info, h_in⟩, ⟨out_info, h_out⟩, ?_⟩
          right
          right
          exact ⟨in_info, out_info, h_in, h_out, h_eq, h_size, h_kind⟩

end PMT.IVE.Soundness
