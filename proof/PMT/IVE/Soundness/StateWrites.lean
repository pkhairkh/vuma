import PMT.PmtInstr
import PMT.IRProgram

/-!
## IVE Soundness — verify_state_writes

This module proves that IVE's `verify_state_writes` function is sound:
if it accepts a program (all `valid = true`), then every state write
accesses a field within the layout's bounds AND writes to a live
(non-consumed) variable.

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
The Rust function performs five checks per write:
  1. `after_consume` flag is `false` (line 65) — linearity.
  2. `consumed_vars.contains(&w.var_name)` is `false` (line 65) — linearity.
  3. `state_var_layouts.get(&w.var_name)` is `Some` (line 78) — var is state-typed.
  4. `layouts.get(&layout_name)` is `Some` (line 91) — layout exists.
  5. The named field exists in the layout and `fi.offset + fi.size ≤
     layout.total_size` (lines 104–117) — in-bounds.
  6. `fi.type_name == w.value_type` (line 118) — type match.

The Lean model below simplifies (3)+(4) by collapsing `state_var_layouts`
and `layouts` into a single total function `env : String → Layout` (the
caller-side well-formedness hypothesis `hwf_env` ensures `WF_Layout (env var)`
for every var). The type-matching check (6) is omitted from this Lean
model — it is a pure syntactic check on `String`s that does not bear on
memory safety; it will be added in Wave 18 if the simulation relation
requires it.

This module is part of Wave 11 (subagent W11-C). The two theorems
`verify_state_writes_sound` and `verify_state_writes_no_uaf` carry
`sorry` placeholders that Waves 19–20 will close via `List.map_mem`
reasoning and the `fieldInLayout_eq_mem` lemma.
-/

namespace PMT.IVE.Soundness

/-- A state write: write to field `f` of variable `var`.
Mirrors Rust `StateWriteOp { var_name, field_name, value_type, after_consume }`
(`state_write.rs:42-48`). The Lean simplification:
  - `field_name : String` (Rust) is replaced by `f : Field` (Lean), which
    carries `(offset, size)` directly. The caller resolves the Rust
    field *name* to a `Field` via `Layout.fields.find?` before invoking
    the Lean model — this is the W2-D "string-tagged" simplification.
  - `value_type : String` (Rust) is dropped — see module doc.
  - `after_consume : bool` (Rust) is folded into the `consumed` list:
    a write with `after_consume = true` corresponds to `w.var ∈ consumed`.
-/
structure StateWrite where
  var : String
  f   : PMT.Field
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
`f ∈ l.fields`; the lemma `fieldInLayout_eq_mem` (proven in Wave 19)
connects the two. -/
def fieldInLayout (l : PMT.Layout) (f : PMT.Field) : Bool :=
  l.fields.any (fun g => g.offset == f.offset && g.size == f.size)

/-- Helper: does field `f`'s byte range `[offset, offset+size)` fit
inside `l.total_size`? Mirrors Rust check `fi.offset + fi.size >
layout.total_size` (`state_write.rs:107`) — note Rust uses `>` (rejects
if exceeds) so we use `≤` (accepts if fits). -/
def fieldInBounds (l : PMT.Layout) (f : PMT.Field) : Bool :=
  f.offset + f.size ≤ l.total_size

/-- Helper: is variable `v` NOT in the consumed set?
Mirrors Rust check `consumed_vars.contains(&w.var_name)` (`state_write.rs:65`)
negated. The `after_consume` flag (`state_write.rs:65`) is folded in:
the caller adds `w.var` to `consumed` when `after_consume = true`. -/
def varLive (consumed : List String) (v : String) : Bool :=
  Bool.not (consumed.contains v)

/-- The Lean model of IVE's `verify_state_writes`.
Checks: (1) field is in layout, (2) field is in bounds, (3) variable
is not consumed. Returns one `StateWriteVerification` per input write,
mirroring the Rust function's `for w in writes { … }` loop
(`state_write.rs:63-151`). -/
def verify_state_writes
    (env : String → PMT.Layout)
    (consumed : List String)
    (writes : List StateWrite) :
    List StateWriteVerification :=
  writes.map fun w =>
    let layout := env w.var
    let fin_layout := fieldInLayout layout w.f
    let fin_bounds := fieldInBounds layout w.f
    let vlive := varLive consumed w.var
    let ok := fin_layout && fin_bounds && vlive
    { valid := ok,
      error := if ok then none
               else some ("write to " ++ w.var ++ " invalid") }

/-- Soundness: if `verify_state_writes` returns all `valid = true`,
then every write is to a live variable with an in-bounds, registered
field. This is the Lean rendering of the soundness obligation for
`src/ive/src/state_write.rs:56`.

The proof goes by `List.map_mem` reasoning: from `w ∈ writes` we obtain
`verify_state_writes env consumed writes = … :: …`, so the verification
record for `w` is in the output list; `hverify` then forces
`valid = true`, which (by unfolding `fieldInLayout` / `fieldInBounds` /
`varLive`) gives the three conjuncts.

The `hwf_env` hypothesis is the Lean-side analog of Rust's
`layouts.get(&layout_name)` returning `Some` with a well-formed
`LayoutInfo` — it ensures the layout `env w.var` is itself well-formed
(every registered field is in-bounds), so the `fieldInLayout` conjunct
implies the `fieldInBounds` conjunct transitively. -/
theorem verify_state_writes_sound
    (env : String → PMT.Layout)
    (consumed : List String)
    (writes : List StateWrite)
    (_hwf_env : ∀ var, PMT.WF_Layout (env var))
    (hverify : ∀ v, v ∈ verify_state_writes env consumed writes → v.valid = true) :
    ∀ w : StateWrite, w ∈ writes →
      w.f ∈ (env w.var).fields
      ∧ w.f.offset + w.f.size ≤ (env w.var).total_size
      ∧ w.var ∉ consumed := by
  intro w hw
  -- Step 1: from `hw : w ∈ writes`, derive that the per-write verification
  -- record (the image of `w` under `verify_state_writes`'s map function) is
  -- in the output list, via `List.mem_map_of_mem : a ∈ l → f a ∈ List.map f l`.
  have h_in :
      { valid := fieldInLayout (env w.var) w.f && fieldInBounds (env w.var) w.f && varLive consumed w.var,
        error := if fieldInLayout (env w.var) w.f && fieldInBounds (env w.var) w.f && varLive consumed w.var
                  then none else some ("write to " ++ w.var ++ " invalid") :
        StateWriteVerification }
        ∈ verify_state_writes env consumed writes := by
    simp only [verify_state_writes]
    apply List.mem_map_of_mem
    exact hw
  -- Step 2: apply the all-valid hypothesis to that verification record.
  have hvalid := hverify _ h_in
  -- Step 3: decompose `valid = true` (after the structure projection
  -- reduces) into the three-way conjunction `fieldInLayout ∧ fieldInBounds ∧
  -- varLive` (each `= true`), via `Bool.and_eq_true`.
  simp only [Bool.and_eq_true] at hvalid
  obtain ⟨⟨hfl, hfb⟩, hvl⟩ := hvalid
  refine ⟨?_, ?_, ?_⟩
  · -- Conjunct 1: `fieldInLayout (env w.var) w.f = true` → `w.f ∈ (env w.var).fields`.
    -- Unfold `fieldInLayout` (a `List.any` over `(offset,size)` equality)
    -- and use `List.any_eq_true` to extract a witness `g ∈ (env w.var).fields`
    -- with `g.offset == w.f.offset ∧ g.size == w.f.size = true`. Then
    -- `LawfulBEq.eq_of_beq` (for `Nat`) promotes each `==` to `=`, giving
    -- `g.offset = w.f.offset` and `g.size = w.f.size`, hence `g = w.f`
    -- (the only two `Field` projections are `offset` and `size`), so
    -- `hg_mem : g ∈ _` becomes `w.f ∈ _`.
    simp only [fieldInLayout, List.any_eq_true] at hfl
    obtain ⟨g, hg_mem, hg_eq⟩ := hfl
    simp only [Bool.and_eq_true] at hg_eq
    obtain ⟨ho, hs⟩ := hg_eq
    have ho_eq : g.offset = w.f.offset := LawfulBEq.eq_of_beq ho
    have hs_eq : g.size  = w.f.size  := LawfulBEq.eq_of_beq hs
    obtain ⟨go, gs⟩ := g
    -- After destructuring `g`, `ho_eq` and `hs_eq` reduce (by `.offset`/`.size`
    -- projections on `Field.mk go gs`) to equalities on the bare `Nat`s.
    have ho_eq' : go = w.f.offset := ho_eq
    have hs_eq' : gs = w.f.size    := hs_eq
    -- Replace `go`/`gs` in `hg_mem` with `w.f.offset`/`w.f.size`; then
    -- `hg_mem : (Field.mk w.f.offset w.f.size) ∈ _` is defeq to `w.f ∈ _`.
    subst ho_eq'
    subst hs_eq'
    exact hg_mem
  · -- Conjunct 2: `fieldInBounds (env w.var) w.f = true` →
    -- `w.f.offset + w.f.size ≤ (env w.var).total_size`.
    -- `fieldInBounds` is the `decide` of the `≤`-Prop (Lean auto-inserts
    -- `decide` because the function's return type is `Bool`), so
    -- `decide_eq_true_iff` recovers the Prop.
    simp only [fieldInBounds, decide_eq_true_iff] at hfb
    exact hfb
  · -- Conjunct 3: `varLive consumed w.var = true` → `w.var ∉ consumed`.
    -- `varLive` is `Bool.not (consumed.contains _)`; with `v ∈ consumed`
    -- we have `List.contains_eq_mem` giving `decide (v ∈ _) = true`, so
    -- `varLive` reduces to `Bool.not true = false`, contradicting `hvl`.
    intro hmem
    simp [varLive, List.contains_eq_mem, hmem] at hvl

/-- Corollary: if all writes pass verification, no write targets a
consumed variable. This is the linearity half of
`verify_state_writes_sound` — the "no use-after-free on the write path"
guarantee. Mirrors Rust's `linearity violation` rejection at
`state_write.rs:65-77`. -/
theorem verify_state_writes_no_uaf
    (env : String → PMT.Layout)
    (consumed : List String)
    (writes : List StateWrite)
    (hwf_env : ∀ var, PMT.WF_Layout (env var))
    (hverify : ∀ v, v ∈ verify_state_writes env consumed writes → v.valid = true) :
    ∀ w : StateWrite, w ∈ writes → w.var ∉ consumed := by
  intro w hw
  -- The no-UAF corollary is the third conjunct of the main soundness
  -- theorem `verify_state_writes_sound` (proven above), so we delegate.
  have h := verify_state_writes_sound env consumed writes hwf_env hverify w hw
  exact h.2.2

end PMT.IVE.Soundness
