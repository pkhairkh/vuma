import PMT.PmtInstr
import PMT.IRProgram
import PMT.IVE.Soundness.StateWrites

/-!
## IVE Soundness — `verify_state_reads` (Wave 1 task IVE-1-C: 8 gaps closed)

This module proves that IVE's `verify_state_reads` function is sound:
if it accepts a program (every returned verification has `valid = true`),
then every state read in that program accesses a field within the layout's
bounds — i.e., the field is registered in `layout.fields` AND the field's
byte range `[offset, offset + size)` fits within `layout.total_size` AND
the field's declared type matches the expected type.

The Lean model mirrors the Rust function's specification. The actual Rust
function lives at `src/ive/src/state_read.rs:44`:

```rust
pub fn verify_state_reads(
    state_var_layouts: &HashMap<String, String>,   // var → layout name
    layouts: &HashMap<String, LayoutInfo>,          // layout name → fields
    reads: &[(String, String, String)],             // (var, field, expected_type)
) -> Vec<StateReadVerification>
```

Per the IVE audit, each item of
the output is `StateReadVerification { valid: bool, error: Option<String> }`,
and the per-read check has five conjuncts:
  1. var is state-typed,
  2. layout exists,
  3. field exists in layout,
  4. `offset + size ≤ total_size`,
  5. `field.type_name == expected_type`.

**Wave 1 task IVE-1-C gap closures** (gaps 1, 3, 8 from the orchestrator spec):
  - Gap 1 (type-match): Added `expected_type : String` to `StateRead` and a
    `fieldTypeMatches` check (reused from `StateWrites.lean`). The
    soundness theorem now concludes the type matches.
  - Gap 3 (HashMap-lookup-vs-total-function): Changed `env : String → Layout`
    to `env : String → Option Layout`. The soundness theorem now handles
    the `none` case (var not state-typed or layout not found → verification
    fails → `hverify` contradiction).
  - Gap 8 (layout-not-found): Subsumed by gap 3's Option model — the
    `none` case is now explicit in both the verifier and the theorem.

This module's structure mirrors the sibling module `PMT.IVE.Soundness.StateWrites`:
both reuse the shared helpers `fieldInLayout`, `fieldInBounds`,
`fieldTypeMatches` defined in `StateWrites.lean`. This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- A state read: read field `f` from variable `var` with expected type
`expected_type`.

Wave 1 task IVE-1-C gap 1: Added `expected_type : String` (was omitted).
Mirrors the third tuple component of `reads: &[(String, String, String)]`
in `src/ive/src/state_read.rs:44`. The Lean model now checks
`fieldTypeMatches` against the layout's declared field type. -/
structure StateRead where
  var           : String
  f             : PMT.Field
  expected_type : String
  deriving Repr

/-- The Lean model of IVE's `verify_state_reads` output item.
Mirrors `StateReadVerification { valid: bool, error: Option<String> }`
from `src/ive/src/state_read.rs`. -/
structure StateReadVerification where
  valid : Bool
  error : Option String
  deriving Repr

/-- The Lean model of IVE's `verify_state_reads`.
Checks: (1) field is in layout, (2) field is in bounds, (3) field type
matches expected type (gap 1). Returns one `StateReadVerification` per
input read, mirroring the Rust function's `for (var, field, ty) in reads { … }`
loop (`state_read.rs:44+`).

Wave 1 task IVE-1-C gap 3: `env : String → Option Layout` (was
`String → Layout`). The `none` case corresponds to Rust's
`state_var_layouts.get(&var)` returning `None` (line 51) OR
`layouts.get(&layout_name)` returning `None` (line 64) — the Lean model
collapses both into a single `none` from `env`. The soundness theorem
handles this by case-split.

The `field_types` parameter is a per-var mapping carrying the declared
types of each field in the var's layout (same as in `StateWrites.lean`). -/
def verify_state_reads
    (env : String → Option PMT.Layout)
    (field_types : String → Option (List (PMT.Field × String)))
    (reads : List StateRead) :
    List StateReadVerification :=
  reads.map fun r =>
    match env r.var with
    | none =>
      { valid := false,
        error := some ("read from " ++ r.var ++ " invalid: variable not state-typed or layout not found") }
    | some layout =>
      let fin_layout := fieldInLayout layout r.f
      let fin_bounds := fieldInBounds layout r.f
      let tmatch := match field_types r.var with
        | some fts => fieldTypeMatches fts r.f r.expected_type
        | none => false
      let ok := fin_layout && fin_bounds && tmatch
      { valid := ok,
        error := if ok then none
                 else some ("read from " ++ r.var ++ " invalid") }

/-- Soundness: if `verify_state_reads` returns all `valid = true`,
then every read accesses a registered, in-bounds field whose declared
type matches the expected type. This is the Lean rendering of the
soundness obligation for `src/ive/src/state_read.rs:44`.

Wave 1 task IVE-1-C: The theorem now concludes 3 conjuncts (was 2):
  (1) `∃ layout, env r.var = some layout ∧ r.f ∈ layout.fields` — env present + field registered (gap 3, 8).
  (2) `r.f.offset + r.f.size ≤ layout.total_size` — in bounds.
  (3) `∃ fts, field_types r.var = some fts ∧ fieldTypeMatches fts r.f r.expected_type = true` — type match (gap 1).

Gap 3 (Option env): The hypothesis `hverify` forces `valid = true`,
which (by the `match env r.var` in `verify_state_reads`) implies
`env r.var = some layout` for some `layout` (the `none` case gives
`valid := false`, contradicting `hverify`). The `some` case then
unfolds to the 3 checks above.

The `hwf_env` hypothesis is the Lean-side analog of Rust's
`layouts.get(&layout_name)` returning `Some` with a well-formed
`LayoutInfo`. -/
theorem verify_state_reads_sound
    (env : String → Option PMT.Layout)
    (field_types : String → Option (List (PMT.Field × String)))
    (reads : List StateRead)
    (_hwf_env : ∀ var, ∀ l, env var = some l → PMT.WF_Layout l)
    (hverify : ∀ v, v ∈ verify_state_reads env field_types reads → v.valid = true) :
    ∀ r : StateRead, r ∈ reads →
      -- (1) env must be `some layout` (gap 3, 8: layout-not-found is now explicit)
      (∃ layout, env r.var = some layout
        ∧ r.f ∈ layout.fields
        ∧ r.f.offset + r.f.size ≤ layout.total_size)
      -- (2) type match (gap 1)
      ∧ (∃ fts, field_types r.var = some fts ∧ fieldTypeMatches fts r.f r.expected_type = true) := by
  intro r hr
  by_cases h_env_none : env r.var = none
  · -- env r.var = none → verification fails → contradicts hverify.
    have h_in :
        ({ valid := false,
           error := some ("read from " ++ r.var ++ " invalid: variable not state-typed or layout not found") :
           StateReadVerification })
          ∈ verify_state_reads env field_types reads := by
      rw [verify_state_reads, List.mem_map]
      refine ⟨r, hr, ?_⟩
      simp only [h_env_none]
    have hvalid := hverify _ h_in
    simp at hvalid
  · -- env r.var ≠ none → env r.var = some layout for some layout.
    obtain ⟨layout, h_env'⟩ := Option.ne_none_iff_exists.mp h_env_none
    have h_env : env r.var = some layout := h_env'.symm
    -- The verification record for r (in the `some` branch).
    have h_in :
        ({ valid := fieldInLayout layout r.f
            && fieldInBounds layout r.f
            && (match field_types r.var with
                | some fts => fieldTypeMatches fts r.f r.expected_type
                | none => false),
           error := if (fieldInLayout layout r.f
            && fieldInBounds layout r.f
            && (match field_types r.var with
                | some fts => fieldTypeMatches fts r.f r.expected_type
                | none => false)) then none else some ("read from " ++ r.var ++ " invalid") :
           StateReadVerification })
          ∈ verify_state_reads env field_types reads := by
      rw [verify_state_reads, List.mem_map]
      refine ⟨r, hr, ?_⟩
      simp only [h_env]
    have hvalid := hverify _ h_in
    -- Step 2: decompose `valid = true` into the 3 conjuncts.
    simp only [Bool.and_eq_true] at hvalid
    obtain ⟨⟨hfl, hfb⟩, htm⟩ := hvalid
    -- htm has the form `match field_types r.var with | some fts => … | none => false = true`.
    by_cases h_fts_none : field_types r.var = none
    · -- field_types r.var = none → htm : false = true → contradiction.
      rw [h_fts_none] at htm
      simp at htm
    · -- field_types r.var = some fts → htm : fieldTypeMatches fts r.f r.expected_type = true.
      obtain ⟨fts, h_fts'⟩ := Option.ne_none_iff_exists.mp h_fts_none
      have h_fts : field_types r.var = some fts := h_fts'.symm
      rw [h_fts] at htm
      -- Step 3: assemble the 2 conjuncts.
      refine ⟨?_, ?_⟩
      · -- (1) ∃ layout, env r.var = some layout ∧ field registered ∧ in bounds.
        refine ⟨layout, h_env, ?_, ?_⟩
        · -- field registered: from hfl : fieldInLayout layout r.f = true.
          -- Bridge: List.any_eq_true + LawfulBEq.eq_of_beq + Field.mk.injEq.
          unfold fieldInLayout at hfl
          rw [List.any_eq_true] at hfl
          obtain ⟨g, hg_mem, hg⟩ := hfl
          simp only [Bool.and_eq_true_iff, decide_eq_true_iff] at hg
          obtain ⟨hg_off, hg_size⟩ := hg
          have hg_off_eq : g.offset = r.f.offset :=
            LawfulBEq.eq_of_beq hg_off
          have hg_size_eq : g.size = r.f.size :=
            LawfulBEq.eq_of_beq hg_size
          have hg_eq : g = r.f := by
            rw [show (g : PMT.Field) = (⟨g.offset, g.size⟩ : PMT.Field) from rfl,
                show (r.f : PMT.Field) = (⟨r.f.offset, r.f.size⟩ : PMT.Field) from rfl,
                PMT.Field.mk.injEq]
            exact ⟨hg_off_eq, hg_size_eq⟩
          rw [← hg_eq]; exact hg_mem
        · -- in bounds: from hfb : fieldInBounds layout r.f = true.
          -- `fieldInBounds` unfolds to `decide (r.f.offset + r.f.size ≤ layout.total_size)`,
          -- so `decide_eq_true_iff` recovers the Prop.
          unfold fieldInBounds at hfb
          exact decide_eq_true_iff.mp hfb
      · -- (2) type match: ∃ fts, field_types r.var = some fts ∧ fieldTypeMatches … = true.
        refine ⟨fts, h_fts, htm⟩

end PMT.IVE.Soundness
