import PMT.PmtInstr
import PMT.IRProgram
import PMT.IVE.Soundness.StateWrites

/-!
## IVE Soundness — `verify_state_reads`

This module proves that IVE's `verify_state_reads` function is sound:
if it accepts a program (every returned verification has `valid = true`),
then every state read in that program accesses a field within the layout's
bounds — i.e., the field is registered in `layout.fields` AND the field's
byte range `[offset, offset + size)` fits within `layout.total_size`.

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

The Lean model below simplifies (1)+(2)+(5) by collapsing
`state_var_layouts` + `layouts` into a single total function
`env : String → Layout` (the caller-side well-formedness hypothesis
`hwf_env` ensures `WF_Layout (env var)` for every var). The type-matching
check (5) is omitted from this Lean model — it is a pure syntactic check
on `String`s that does not bear on memory safety; it may be added later
if the simulation relation requires it. The two soundness-critical
conjuncts (3) + (4) are retained and match `WF_Layout`'s first conjunct
in `PMT.Basic`.

This module's two original
`sorry` placeholders have been closed. Its
structure mirrors the sibling module `PMT.IVE.Soundness.StateWrites`:
both reuse the shared helpers `fieldInLayout` (a `List.any`-
based Bool mirror of `f ∈ l.fields`) and `fieldInBounds` (a `≤`-coerced
Bool mirror of `f.offset + f.size ≤ l.total_size`) defined in
`StateWrites.lean`. The theorem `verify_state_reads_sound` is proved by
`List.mem_map` reasoning plus an inline bridge from the Bool
`fieldInLayout` to the Prop `f ∈ l.fields` (the shared
`fieldInLayout_eq_mem` lemma is deferred to future work, but the bridge is
inlined here so the proof is `sorry`-free).
-/

namespace PMT.IVE.Soundness

/-- A state read: read field `f` from variable `var`.
Mirrors one element of the `reads: &[(String, String, String)]` parameter
to `verify_state_reads` at `src/ive/src/state_read.rs:44` (the third tuple
component, `expected_type`, is omitted — see module doc). -/
structure StateRead where
  var : String
  f   : PMT.Field
  deriving Repr

/-- The Lean model of IVE's `verify_state_reads` output item.
Mirrors `StateReadVerification { valid: bool, error: Option<String> }`
from `src/ive/src/state_read.rs`. -/
structure StateReadVerification where
  valid : Bool
  error : Option String
  deriving Repr

/-- The Lean model of IVE's `verify_state_reads`.
Checks: (1) field is in layout, (2) field is in bounds. Returns one
`StateReadVerification` per input read, mirroring the Rust function's
`for (var, field, ty) in reads { … }` loop (`state_read.rs:44+`).

The helpers `fieldInLayout` and `fieldInBounds` are reused from
`PMT.IVE.Soundness.StateWrites` (W11-A) — they are identical for reads
and writes since the bounds check is symmetric. -/
def verify_state_reads
    (env : String → PMT.Layout)
    (reads : List StateRead) :
    List StateReadVerification :=
  reads.map fun r =>
    let layout := env r.var
    let fin_layout := fieldInLayout layout r.f
    let fin_bounds := fieldInBounds layout r.f
    let ok := fin_layout && fin_bounds
    { valid := ok,
      error := if ok then none
               else some ("read from " ++ r.var ++ " invalid") }

/-- Soundness: if `verify_state_reads` returns all `valid = true`,
then every read accesses a registered, in-bounds field. This is the Lean
rendering of the soundness obligation for `src/ive/src/state_read.rs:44`.

The proof goes by `List.mem_map` reasoning: from `r ∈ reads` we obtain
the corresponding `StateReadVerification` in the output list; `hverify`
then forces `valid = true`, which (by unfolding `fieldInLayout` /
`fieldInBounds`) gives the two conjuncts directly. The first conjunct
(`r.f ∈ (env r.var).fields`) is obtained by inlining the bridge from
the Bool `fieldInLayout` (a `List.any` over `Field`s matching on
`(offset, size)`) to the Prop `r.f ∈ …` — this bridges through
`List.any_eq_true`, `decide_eq_true_iff`, and the auto-generated
`Field.mk.injEq`.

The `hwf_env` hypothesis (named `_hwf_env` in the signature, per the
codebase's "intentionally unused" convention — see `alloc_preserves_capacity`
in `PMT.Basic`) is the Lean-side analog of Rust's
`layouts.get(&layout_name)` returning `Some` with a well-formed
`LayoutInfo`. It is not needed for *this* soundness proof — the
in-bounds conjunct comes directly from `hverify` plus the
`fieldInBounds` check — but it is retained so the theorem statement
mirrors the sibling `verify_state_writes_sound` (and so downstream
strengthenings that DO need `WF_Layout` can re-use the signature). -/
theorem verify_state_reads_sound
    (env : String → PMT.Layout)
    (reads : List StateRead)
    (_hwf_env : ∀ var, PMT.WF_Layout (env var))
    (hverify : ∀ v, v ∈ verify_state_reads env reads → v.valid = true) :
    ∀ r : StateRead, r ∈ reads →
      r.f ∈ (env r.var).fields
      ∧ r.f.offset + r.f.size ≤ (env r.var).total_size := by
  intro r hr
  -- Step 1: from `hr : r ∈ reads`, derive that the per-read verification
  -- record is in the output of `verify_state_reads`. We use
  -- `List.mem_map : b ∈ List.map f l ↔ ∃ a, a ∈ l ∧ f a = b`.
  -- After `verify_state_reads` is unfolded to `reads.map …`, the witness
  -- is `r` itself, and `f r` reduces (by `let`-substitution) to the
  -- explicit record below, so the equality is `rfl`.
  have h_in :
      ({ valid := fieldInLayout (env r.var) r.f && fieldInBounds (env r.var) r.f,
         error := if fieldInLayout (env r.var) r.f && fieldInBounds (env r.var) r.f then none
                  else some ("read from " ++ r.var ++ " invalid") }
        : StateReadVerification)
        ∈ verify_state_reads env reads := by
    rw [verify_state_reads, List.mem_map]
    refine ⟨r, hr, ?_⟩
    rfl
  -- Step 2: apply the all-valid hypothesis to that verification record.
  have hvalid := hverify _ h_in
  -- Step 3: unfold `fieldInBounds` so its `decide (… ≤ …)` form is visible,
  -- then split the `&&` into two equalities (`Bool.and_eq_true_iff`).
  -- `decide_eq_true_iff` rewrites each `decide p = true` back to `p`.
  unfold fieldInBounds at hvalid
  simp only [Bool.and_eq_true_iff, decide_eq_true_iff] at hvalid
  obtain ⟨h_in_layout, h_in_bounds⟩ := hvalid
  -- `h_in_bounds` is now the second goal conjunct verbatim, so we keep it
  -- for the final `⟨…, h_in_bounds⟩`.
  -- Step 4: convert `fieldInLayout (env r.var) r.f = true` to the
  -- Prop `r.f ∈ (env r.var).fields`. `fieldInLayout` is `l.fields.any …`;
  -- `List.any_eq_true` gives us a witness `g` in `l.fields` whose
  -- `(offset, size)` pair matches `r.f`'s.
  unfold fieldInLayout at h_in_layout
  rw [List.any_eq_true] at h_in_layout
  obtain ⟨g, hg_mem, hg⟩ := h_in_layout
  simp only [Bool.and_eq_true_iff, decide_eq_true_iff] at hg
  obtain ⟨hg_off, hg_size⟩ := hg
  -- Step 5: `g.offset == r.f.offset = true` (a `decide`-backed `BEq`
  -- since `Nat`'s `BEq` is `instBEqOfDecidableEq`) becomes `g.offset = r.f.offset`
  -- via `decide_eq_true_iff`. Same for `size`.
  have hg_off_eq : g.offset = r.f.offset := by
    rw [show (g.offset == r.f.offset) = decide (g.offset = r.f.offset) from rfl,
        decide_eq_true_iff] at hg_off
    exact hg_off
  have hg_size_eq : g.size = r.f.size := by
    rw [show (g.size == r.f.size) = decide (g.size = r.f.size) from rfl,
        decide_eq_true_iff] at hg_size
    exact hg_size
  -- Step 6: assemble `g = r.f` from the two component equalities via the
  -- auto-generated `Field.mk.injEq`, then substitute `g` for `r.f` in
  -- `hg_mem : g ∈ (env r.var).fields` to finish the first conjunct.
  have hg_eq : g = r.f := by
    rw [show (g : PMT.Field) = (⟨g.offset, g.size⟩ : PMT.Field) from rfl,
        show (r.f : PMT.Field) = (⟨r.f.offset, r.f.size⟩ : PMT.Field) from rfl,
        PMT.Field.mk.injEq]
    exact ⟨hg_off_eq, hg_size_eq⟩
  refine ⟨?_, h_in_bounds⟩
  rw [← hg_eq]; exact hg_mem

end PMT.IVE.Soundness
