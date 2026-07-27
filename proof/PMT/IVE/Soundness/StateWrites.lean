import PMT.PmtInstr
import PMT.IRProgram

/-!
## IVE Soundness — verify_state_writes (Wave 1 task IVE-1-C: 8 gaps closed)

This module proves that IVE's `verify_state_writes` function is sound:
if it accepts a program (all `valid = true`), then every state write
accesses a field within the layout's bounds AND writes to a live
(non-consumed) variable AND the value type matches the field's declared type.

The Lean model mirrors the Rust function's specification. The actual
Rust function lives at `src/ive/src/state_write.rs:56`.

**Rust reference** (`src/ive/src/state_write.rs:56-153`):
```rust
pub fn verify_state_writes(
    state_var_layouts: &HashMap<String, String>,
    layouts: &HashMap<String, LayoutInfo>,
    writes: &[StateWriteOp],
    consumed_vars: &HashSet<String>,
) -> Vec<StateWriteVerification>
```
The Rust function performs six checks per write:
  1. `after_consume` flag is `false` (line 65) — linearity (per-write).
  2. `consumed_vars.contains(&w.var_name)` is `false` (line 65) — linearity (set-membership).
  3. `state_var_layouts.get(&w.var_name)` is `Some` (line 78) — var is state-typed.
  4. `layouts.get(&layout_name)` is `Some` (line 91) — layout exists.
  5. The named field exists in the layout and `fi.offset + fi.size ≤
     layout.total_size` (lines 104–117) — in-bounds.
  6. `fi.type_name == w.value_type` (line 118) — type match.

**Wave 1 task IVE-1-C gap closures** (gaps 2, 3, 4, 8 from the orchestrator spec):
  - Gap 2 (type-match): Added `value_type : String` to `StateWrite` and a
    `fieldTypeMatches` check. The soundness theorem now concludes the
    type matches.
  - Gap 3 (HashMap-lookup-vs-total-function): Changed `env : String → Layout`
    to `env : String → Option Layout`. The soundness theorem now handles
    the `none` case (var not state-typed or layout not found → verification
    fails → `hverify` contradiction).
  - Gap 4 (after_consume vs consumed_vars): Added `after_consume : Bool`
    to `StateWrite`. The Lean model now checks BOTH `after_consume` and
    `consumed.contains` separately, matching Rust line 65 exactly.
  - Gap 8 (layout-not-found): Subsumed by gap 3's Option model — the
    `none` case is now explicit in both the verifier and the theorem.

The two "shared gaps" (6: Copy accepts any pair, 7: Reinterpret accepts
any same-size pair) are in `Transform.lean`, not here — they are
documented there as accepted spec choices (the soundness theorem
already requires `WF_Layout` for both layouts, which is the correct
contract).

This module is `sorry`-free.
-/

namespace PMT.IVE.Soundness

/-- A state write: write to field `f` of variable `var`.
Mirrors Rust `StateWriteOp { var_name, field_name, value_type, after_consume }`
(`state_write.rs:42-48`).

Wave 1 task IVE-1-C: Added `value_type` (gap 2) and `after_consume` (gap 4).
  - `field_name : String` (Rust) is replaced by `f : Field` (Lean), which
    carries `(offset, size)` directly. The caller resolves the Rust
    field *name* to a `Field` via `Layout.fields.find?` before invoking
    the Lean model — this is the "string-tagged" simplification.
  - `value_type : String` (Rust) is now retained (gap 2 closure) — the
    Lean model checks `fieldTypeMatches` against the layout's declared
    field type.
  - `after_consume : Bool` (Rust) is now retained (gap 4 closure) — the
    Lean model checks BOTH `after_consume` AND `consumed.contains`,
    matching Rust line 65 exactly. (Previously, `after_consume` was
    folded into the `consumed` list, which conflated the two checks.)
  - The declared field type is carried in the `Layout` structure's
    field list. Since `PMT.Field` (in `PMT.Basic`) only has `offset` and
    `size` (no `type_name`), we carry `value_type` as the EXPECTED type
    and use a separate `field_types : List (Field × String)` mapping
    (passed alongside the layout) to look up the DECLARED type. This
    avoids modifying `PMT.Field` (PMT-owned codedomain). -/
structure StateWrite where
  var           : String
  f             : PMT.Field
  value_type    : String
  after_consume : Bool
  deriving Repr

/-- The Lean model of IVE's `verify_state_writes` output.
Mirrors Rust `StateWriteVerification { var_name, layout_name, field_name,
valid, error }` (`state_write.rs:14-21`). The Lean simplification drops
the diagnostic strings (`var_name` / `layout_name` / `field_name`) — they
are not consulted by the soundness proof; only `valid` and (optionally)
`error` matter. -/
structure StateWriteVerification where
  valid : Bool
  error : Option String
  deriving Repr

/-- Helper: is field `f` registered in layout `l`?
Compares by `(offset, size)` since `PMT.Field` does not derive `BEq` /
`DecidableEq` (see `PMT.Basic`). The corresponding propositional form is
`f ∈ l.fields`; the lemma `fieldInLayout_eq_mem` connects the two. -/
def fieldInLayout (l : PMT.Layout) (f : PMT.Field) : Bool :=
  l.fields.any (fun g => g.offset == f.offset && g.size == f.size)

/-- Helper: does field `f`'s byte range `[offset, offset+size)` fit
inside `l.total_size`? Mirrors Rust check `fi.offset + fi.size >
layout.total_size` (`state_write.rs:107`) — note Rust uses `>` (rejects
if exceeds) so we use `≤` (accepts if fits). -/
def fieldInBounds (l : PMT.Layout) (f : PMT.Field) : Bool :=
  f.offset + f.size ≤ l.total_size

/-- Helper: is variable `v` NOT in the consumed set?
Mirrors Rust check `consumed_vars.contains(&w.var_name)` (`state_write.rs:65`).

Wave 1 task IVE-1-C gap 4: This now models ONLY the set-membership
check (line 65, second operand of `||`). The `after_consume` flag
(line 65, first operand of `||`) is checked separately in
`verify_state_writes` below, matching Rust's `w.after_consume ||
consumed_vars.contains(&w.var_name)` exactly. -/
def varLive (consumed : List String) (v : String) : Bool :=
  Bool.not (consumed.contains v)

/-- Helper: does the field's declared type match the expected type?
Mirrors Rust check `fi.type_name == w.value_type` (`state_write.rs:118`).

Wave 1 task IVE-1-C gap 2: This is the type-matching check that was
previously omitted from the Lean model. The `field_types` parameter is
a list of `(Field, type_name)` pairs parallel to `layout.fields`; we
look up the declared type by matching `(offset, size)` (same
comparison as `fieldInLayout`). If the field is not found in
`field_types`, we default to `"unknown"` (which will fail the equality
check, matching Rust's behavior of rejecting unmatched types). -/
def fieldTypeMatches (field_types : List (PMT.Field × String))
    (f : PMT.Field) (expected_type : String) : Bool :=
  match (field_types.find? (fun (g, _) => g.offset == f.offset && g.size == f.size)) with
  | some (_, declared_type) => decide (declared_type = expected_type)
  | none => false

/-- The Lean model of IVE's `verify_state_writes`.
Checks: (1) `after_consume` is false (gap 4), (2) variable is not
consumed (gap 4), (3) field is in layout, (4) field is in bounds,
(5) field type matches expected type (gap 2). Returns one
`StateWriteVerification` per input write, mirroring the Rust function's
`for w in writes { … }` loop (`state_write.rs:63-151`).

Wave 1 task IVE-1-C gap 3: `env : String → Option Layout` (was
`String → Layout`). The `none` case corresponds to Rust's
`state_var_layouts.get(&w.var_name)` returning `None` (line 78) OR
`layouts.get(&layout_name)` returning `None` (line 91) — the Lean model
collapses both into a single `none` from `env`. The soundness theorem
handles this by case-split: `some layout` → proceed with checks; `none`
→ verification fails → `hverify` contradiction.

The `field_types` parameter is a per-var mapping carrying the declared
types of each field in the var's layout. It's passed as
`String → Option (List (Field × String))` to mirror the Option-based env
(gap 3): if `env var = none`, then `field_types var = none` too. -/
def verify_state_writes
    (env : String → Option PMT.Layout)
    (field_types : String → Option (List (PMT.Field × String)))
    (consumed : List String)
    (writes : List StateWrite) :
    List StateWriteVerification :=
  writes.map fun w =>
    match env w.var with
    | none =>
      { valid := false,
        error := some ("write to " ++ w.var ++ " invalid: variable not state-typed or layout not found") }
    | some layout =>
      let fin_layout := fieldInLayout layout w.f
      let fin_bounds := fieldInBounds layout w.f
      let vlive := varLive consumed w.var
      let tmatch := match field_types w.var with
        | some fts => fieldTypeMatches fts w.f w.value_type
        | none => false
      -- Gap 4: check after_consume OR consumed.contains (Rust line 65).
      -- If either is true, the write is invalid (linearity violation).
      let linearity_ok := Bool.not w.after_consume && vlive
      let ok := fin_layout && fin_bounds && linearity_ok && tmatch
      { valid := ok,
        error := if ok then none
                 else some ("write to " ++ w.var ++ " invalid") }

/-- Soundness: if `verify_state_writes` returns all `valid = true`,
then every write is to a live variable with an in-bounds, registered
field whose declared type matches the value type. This is the Lean
rendering of the soundness obligation for `src/ive/src/state_write.rs:56`.

Wave 1 task IVE-1-C: The theorem now concludes 4 conjuncts (was 3):
  (1) `w.f ∈ (env w.var).fields` — field registered.
  (2) `w.f.offset + w.f.size ≤ (env w.var).total_size` — in bounds.
  (3) `w.var ∉ consumed ∧ ¬w.after_consume` — linearity (gap 4: both checks).
  (4) `fieldTypeMatches … = true` — type match (gap 2).

Gap 3 (Option env): The hypothesis `hverify` forces `valid = true`,
which (by the `match env w.var` in `verify_state_writes`) implies
`env w.var = some layout` for some `layout` (the `none` case gives
`valid := false`, contradicting `hverify`). The `some` case then
unfolds to the 4 checks above.

The `hwf_env` hypothesis is the Lean-side analog of Rust's
`layouts.get(&layout_name)` returning `Some` with a well-formed
`LayoutInfo` — it ensures the layout `env w.var` (when present) is
itself well-formed (every registered field is in-bounds), so the
`fieldInLayout` conjunct implies the `fieldInBounds` conjunct transitively. -/
theorem verify_state_writes_sound
    (env : String → Option PMT.Layout)
    (field_types : String → Option (List (PMT.Field × String)))
    (consumed : List String)
    (writes : List StateWrite)
    (_hwf_env : ∀ var, ∀ l, env var = some l → PMT.WF_Layout l)
    (hverify : ∀ v, v ∈ verify_state_writes env field_types consumed writes → v.valid = true) :
    ∀ w : StateWrite, w ∈ writes →
      -- (1) env must be `some layout` (gap 3: layout-not-found is now explicit)
      (∃ layout, env w.var = some layout
        ∧ w.f ∈ layout.fields
        ∧ w.f.offset + w.f.size ≤ layout.total_size)
      -- (2) linearity: both after_consume and consumed.contains must be false (gap 4)
      ∧ (¬w.after_consume ∧ w.var ∉ consumed)
      -- (3) type match (gap 2)
      ∧ (∃ fts, field_types w.var = some fts ∧ fieldTypeMatches fts w.f w.value_type = true) := by
  intro w hw
  -- Step 1: from `hw : w ∈ writes`, derive that the per-write verification
  -- record is in the output list.
  -- We case-split on `env w.var` to determine which branch of the match
  -- in `verify_state_writes` was taken.
  by_cases h_env_none : env w.var = none
  · -- env w.var = none → verification fails → contradicts hverify.
    -- The verification record for w is `{ valid := false, … }`.
    -- We show this record is in the output of `verify_state_writes`.
    -- `verify_state_writes` is `writes.map (fun w => match env w.var with …)`.
    -- For our specific `w`, the match reduces (via `h_env_none`) to the `none` branch.
    -- We use `List.mem_map` to find the witness.
    have h_in :
        ({ valid := false,
           error := some ("write to " ++ w.var ++ " invalid: variable not state-typed or layout not found") :
           StateWriteVerification })
          ∈ verify_state_writes env field_types consumed writes := by
      rw [verify_state_writes, List.mem_map]
      -- After rw, the goal is: ∃ a, a ∈ writes ∧ (match env a.var with …) = our record.
      -- We claim the witness is `w` itself.
      refine ⟨w, hw, ?_⟩
      -- Now we need: (match env w.var with | none => {valid := false, …} | some layout => …) = our record.
      -- Since `env w.var = none` (h_env_none), the match reduces to the `none` branch.
      -- `simp only [h_env_none]` rewrites `env w.var` to `none`, then the match reduces.
      simp only [h_env_none]
    have hvalid := hverify _ h_in
    simp at hvalid
  · -- env w.var ≠ none → env w.var = some layout for some layout.
    obtain ⟨layout, h_env⟩ := Option.ne_none_iff_exists.mp h_env_none
    -- h_env : some layout = env w.var ; flip to env w.var = some layout.
    have h_env : env w.var = some layout := h_env.symm
    -- The verification record for w (in the `some` branch).
    have h_in :
        ({ valid := fieldInLayout layout w.f
            && fieldInBounds layout w.f
            && (Bool.not w.after_consume && varLive consumed w.var)
            && (match field_types w.var with
                | some fts => fieldTypeMatches fts w.f w.value_type
                | none => false),
           error := if (fieldInLayout layout w.f
            && fieldInBounds layout w.f
            && (Bool.not w.after_consume && varLive consumed w.var)
            && (match field_types w.var with
                | some fts => fieldTypeMatches fts w.f w.value_type
                | none => false)) then none else some ("write to " ++ w.var ++ " invalid") :
           StateWriteVerification })
          ∈ verify_state_writes env field_types consumed writes := by
      rw [verify_state_writes, List.mem_map]
      refine ⟨w, hw, ?_⟩
      simp only [h_env]
    have hvalid := hverify _ h_in
    -- Step 2: decompose `valid = true` into the 4 conjuncts.
    simp only [Bool.and_eq_true] at hvalid
    obtain ⟨⟨⟨hfl, hfb⟩, ⟨hac, hvl⟩⟩, htm⟩ := hvalid
    -- htm has the form `match field_types w.var with | some fts => … | none => false = true`.
    by_cases h_fts_none : field_types w.var = none
    · -- field_types w.var = none → htm : false = true → contradiction.
      rw [h_fts_none] at htm
      simp at htm
    · -- field_types w.var = some fts → htm : fieldTypeMatches fts w.f w.value_type = true.
      obtain ⟨fts, h_fts⟩ := Option.ne_none_iff_exists.mp h_fts_none
      have h_fts : field_types w.var = some fts := h_fts.symm
      rw [h_fts] at htm
      -- Step 3: assemble the 3 conjuncts.
      refine ⟨?_, ?_, ?_⟩
      · -- (1) ∃ layout, env w.var = some layout ∧ field registered ∧ in bounds.
        refine ⟨layout, h_env, ?_, ?_⟩
        · -- field registered: from hfl : fieldInLayout layout w.f = true.
          simp only [fieldInLayout, List.any_eq_true] at hfl
          obtain ⟨g, hg_mem, hg_eq⟩ := hfl
          simp only [Bool.and_eq_true] at hg_eq
          obtain ⟨ho, hs⟩ := hg_eq
          have ho_eq : g.offset = w.f.offset := LawfulBEq.eq_of_beq ho
          have hs_eq : g.size  = w.f.size  := LawfulBEq.eq_of_beq hs
          obtain ⟨go, gs⟩ := g
          have ho_eq' : go = w.f.offset := ho_eq
          have hs_eq' : gs = w.f.size    := hs_eq
          subst ho_eq'
          subst hs_eq'
          exact hg_mem
        · -- in bounds: from hfb : fieldInBounds layout w.f = true.
          simp only [fieldInBounds, decide_eq_true_iff] at hfb
          exact hfb
      · -- (2) linearity: ¬after_consume ∧ w.var ∉ consumed.
        refine ⟨?_, ?_⟩
        · -- ¬after_consume: from hac : Bool.not w.after_consume = true.
          by_cases hac' : w.after_consume
          · simp [hac'] at hac
          · exact hac'
        · -- w.var ∉ consumed: from hvl : varLive consumed w.var = true.
          intro hmem
          simp [varLive, List.contains_eq_mem, hmem] at hvl
      · -- (3) type match: ∃ fts, field_types w.var = some fts ∧ fieldTypeMatches … = true.
        refine ⟨fts, h_fts, htm⟩

/-- Corollary: if all writes pass verification, no write targets a
consumed variable or has `after_consume = true`. This is the linearity
half of `verify_state_writes_sound` — the "no use-after-free on the write path"
guarantee. Mirrors Rust's `linearity violation` rejection at
`state_write.rs:65-77`.

Wave 1 task IVE-1-C gap 4: The corollary now concludes BOTH
`¬w.after_consume` AND `w.var ∉ consumed` (was only the latter). -/
theorem verify_state_writes_no_uaf
    (env : String → Option PMT.Layout)
    (field_types : String → Option (List (PMT.Field × String)))
    (consumed : List String)
    (writes : List StateWrite)
    (hwf_env : ∀ var, ∀ l, env var = some l → PMT.WF_Layout l)
    (hverify : ∀ v, v ∈ verify_state_writes env field_types consumed writes → v.valid = true) :
    ∀ w : StateWrite, w ∈ writes → (¬w.after_consume ∧ w.var ∉ consumed) := by
  intro w hw
  have h := verify_state_writes_sound env field_types consumed writes hwf_env hverify w hw
  -- h : (∃ layout, …) ∧ (¬w.after_consume ∧ w.var ∉ consumed) ∧ (∃ fts, …)
  -- We need only the second conjunct (the linearity half).
  exact h.2.1

end PMT.IVE.Soundness
