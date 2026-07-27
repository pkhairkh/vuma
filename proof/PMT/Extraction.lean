import PMT.Basic
import PMT.Soundness
import PMT.RawArena
import PMT.WellTypedStrong
import PMT.IVE.Soundness.StateReads
import PMT.IVE.Soundness.StateWrites
import PMT.IVE.Soundness.Transform

/-! ## Extraction — Verified Bounds-Checking Logic (sorry-free)

This module provides the Lean-side interface for extracting verified
bounds-checking logic into the Rust compiler. All theorems in this file
close without `sorry`.

Two extraction paths:
  1. **Automatic (via Lean C backend)**: `lake build` produces `.c` files
     in `proof/.lake/build/ir/`. These can be compiled and linked into
     Rust via FFI. See `proof/lakefile.toml` for the build config.
  2. **Manual (hand-translation)**: The key verified constants and
     functions are translated into Rust by hand, with a parity test
     ensuring the Rust matches the Lean semantics.

This module defines the EXACT interface that extraction produces:
  * `verified_capacity_check` — mirrors `arena.rs::Arena::alloc`'s
    overflow check.
  * `verified_field_bounds_check` — mirrors
    `memory_safety.rs::inject_bounds_check_ir`'s UGe check.
  * `verified_linearity_check` — mirrors IVE's `verify_state_writes`
    consumed-set check.
  * `verified_pmt_check` — the composition of all three.
The actual extraction pipeline is implemented separately.

**References.**
  * Lean 4 C backend: https://lean-lang.org/doc/reference/other-features.html#code-generation
  * Related modules: `PMT.Basic` (`Arena`, `Field`, `Layout`),
    `PMT.Soundness` (`CapacityInvariant`, `FieldBounds`),
    `PMT.RawArena` (Rust-side faithful model), `PMT.WellTypedStrong`
    (composed PMT check).

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job in
`.github/workflows/proof-verify.yml` runs the same command.
-/

namespace PMT.Extraction

/-- §1: The verified capacity check.
This is the Lean-side function that gets extracted to Rust.
Mirrors `arena.rs::Arena::alloc`'s overflow check.

FFI: `@[export lean_verified_capacity_check]` makes this callable
from C/Rust as `lean_verified_capacity_check(lean_object*,
lean_object*, lean_object*) -> uint8_t` (Lean `Bool` is `uint8_t`
via the unboxed representation; `Nat` is `lean_object*`). The export
name carries the `lean_` prefix because Lean 4.21 emits the export
name verbatim as the C symbol (no auto-prefix); this matches the
Rust FFI signatures in `proof/extracted/README.md` §Stage 2. -/
@[export lean_verified_capacity_check]
def verified_capacity_check (used : Nat) (size : Nat) (capacity : Nat) : Bool :=
  used + size ≤ capacity

/-- §2: The verified field-bounds check.
Mirrors `memory_safety.rs::inject_bounds_check_ir`'s UGe check.

FFI: `@[export lean_verified_field_bounds_check]` makes this callable
from C/Rust as `lean_verified_field_bounds_check(lean_object*,
lean_object*) -> uint8_t`. The `Field` and `Layout` arguments are
boxed `lean_object*` values; callers must construct them via
`lean_alloc_struct` or use the `___boxed` wrapper. -/
@[export lean_verified_field_bounds_check]
def verified_field_bounds_check (f : Field) (layout : Layout) : Bool :=
  f.offset + f.size ≤ layout.total_size

/-- §3: The verified linearity check.
Mirrors IVE's `verify_state_writes` consumed-set check.

FFI: `@[export lean_verified_linearity_check]` makes this callable
from C/Rust as `lean_verified_linearity_check(lean_object*,
lean_object*) -> uint8_t`. Both `var : String` and
`consumed : List String` are boxed `lean_object*` values. -/
@[export lean_verified_linearity_check]
def verified_linearity_check (var : String) (consumed : List String) : Bool :=
  var ∉ consumed

/-- §4: The composed PMT check — all three checks together.

FFI: `@[export lean_verified_pmt_check]` makes this callable from
C/Rust as `lean_verified_pmt_check(...)` with six `lean_object*`
arguments (used, capacity, f, layout, var, consumed) returning
`uint8_t`. -/
@[export lean_verified_pmt_check]
def verified_pmt_check
    (used capacity : Nat)
    (f : Field) (layout : Layout)
    (var : String) (consumed : List String) : Bool :=
  verified_capacity_check used layout.total_size capacity
  ∧ verified_field_bounds_check f layout
  ∧ verified_linearity_check var consumed

/-- §5: Soundness — the verified checks are correct. -/
theorem verified_capacity_check_correct
    (used size capacity : Nat)
    (hcheck : verified_capacity_check used size capacity = true) :
    used + size ≤ capacity := by
  unfold verified_capacity_check at hcheck
  simp at hcheck
  exact hcheck

theorem verified_field_bounds_check_correct
    (f : Field) (layout : Layout)
    (hcheck : verified_field_bounds_check f layout = true) :
    f.offset + f.size ≤ layout.total_size := by
  unfold verified_field_bounds_check at hcheck
  simp at hcheck
  exact hcheck

theorem verified_linearity_check_correct
    (var : String) (consumed : List String)
    (hcheck : verified_linearity_check var consumed = true) :
    var ∉ consumed := by
  unfold verified_linearity_check at hcheck
  simp at hcheck
  exact hcheck

/-- §6: The composed check is correct (all three sub-checks hold). -/
theorem verified_pmt_check_correct
    (used capacity : Nat)
    (f : Field) (layout : Layout)
    (var : String) (consumed : List String)
    (hcheck : verified_pmt_check used capacity f layout var consumed = true) :
    used + layout.total_size ≤ capacity
    ∧ f.offset + f.size ≤ layout.total_size
    ∧ var ∉ consumed := by
  unfold verified_pmt_check at hcheck
  simp at hcheck
  refine ⟨?_, ?_, ?_⟩
  · exact verified_capacity_check_correct _ _ _ hcheck.1
  · exact verified_field_bounds_check_correct _ _ hcheck.2.1
  · exact verified_linearity_check_correct _ _ hcheck.2.2

/-- §7: Extraction manifest — the list of functions to extract.
This is read by the extraction script to know which Lean
definitions become Rust functions. -/
def extraction_manifest : List String :=
  [ "verified_capacity_check",
    "verified_field_bounds_check",
    "verified_linearity_check",
    "verified_pmt_check",
    "lean_verify_transform",
    "lean_verify_state_reads",
    "lean_verify_state_writes" ]

/-! ## §8: IVE state-verifier exports (Wave 1 task IVE-1-B)

The three IVE state verifiers (`verify_state_reads`, `verify_state_writes`,
`verify_transform`) have sorry-free Lean soundness proofs
(`PMT.IVE.Soundness.StateReads`, `StateWrites`, `Transform`).
After IVE-1-A made `verify_transform` computable (no `noncomputable`,
no `Classical.propDecidable`), all three are now eligible for `@[export]`
extraction to Rust via Lean's C backend.

The challenge: `verify_state_reads` and `verify_state_writes` take
`env : String → Layout` (a function type), which is NOT directly
C-marshallable. We export wrapper functions that take a list of
`(var_name, layout)` pairs instead; the wrapper reconstructs the total
function internally (returning `emptyLayout` for unknown vars, matching
the Rust-side `HashMap.get()` → default behavior).

`verify_transform` takes `StateTransform` (a structure with
`String × String × Layout × Layout × TransformKind`), which IS
directly C-marshallable. We export it directly.
-/

/-- Helper: reconstruct a `String → Option Layout` function from a list of
(var, layout) pairs. Unknown vars map to `none` (mirrors Rust's
`HashMap.get()` returning `None`). This is the Option-based env model
introduced by Wave 1 task IVE-1-C gap 3. -/
def layout_env_from_list (env_list : List (String × PMT.Layout))
    (var : String) : Option PMT.Layout :=
  env_list.lookup var

/-- Helper: reconstruct a `String → Option (List (Field × String))` function
from a list of (var, field_types) pairs. Unknown vars map to `none`.
This carries the declared field types needed by the type-matching check
(Wave 1 task IVE-1-C gap 1, 2). -/
def field_types_env_from_list (ft_list : List (String × List (PMT.Field × String)))
    (var : String) : Option (List (PMT.Field × String)) :=
  ft_list.lookup var

/-- `@[export lean_verify_transform]` — extracted form of
`PMT.IVE.Soundness.verify_transform`. Takes a `StateTransform` and
returns a `StateTransformVerification` (a `Bool × Option String`
structure). The Rust FFI marshals `StateTransform` as a struct of
(offsets/sizes for the two layouts + a kind tag) and the return as
`uint8_t` (the `valid` field; the `error` string is dropped on the
FFI boundary since it's only diagnostic). -/
@[export lean_verify_transform]
def leanVerifyTransform (t : PMT.IVE.Soundness.StateTransform) :
    Bool :=
  (PMT.IVE.Soundness.verify_transform t).valid

/-- `@[export lean_verify_state_reads]` — extracted form of
`PMT.IVE.Soundness.verify_state_reads`. Takes a list of (var, layout)
pairs (the env), a list of (var, field_types) pairs, and a list of
`StateRead` (var + field + expected_type), returns `true` iff every
read passes (i.e., the verification list is all `valid = true`).
The Rust FFI marshals each list as a `lean_object*`. -/
@[export lean_verify_state_reads]
def leanVerifyStateReads
    (env_list : List (String × PMT.Layout))
    (ft_list : List (String × List (PMT.Field × String)))
    (reads : List PMT.IVE.Soundness.StateRead) : Bool :=
  let env := layout_env_from_list env_list
  let fts := field_types_env_from_list ft_list
  let results := PMT.IVE.Soundness.verify_state_reads env fts reads
  results.all (fun r => r.valid)

/-- `@[export lean_verify_state_writes]` — extracted form of
`PMT.IVE.Soundness.verify_state_writes`. Takes a list of (var, layout)
pairs, a list of (var, field_types) pairs, a list of consumed var names,
and a list of `StateWrite` (var + field + value_type + after_consume),
returns `true` iff every write passes. -/
@[export lean_verify_state_writes]
def leanVerifyStateWrites
    (env_list : List (String × PMT.Layout))
    (ft_list : List (String × List (PMT.Field × String)))
    (consumed : List String)
    (writes : List PMT.IVE.Soundness.StateWrite) : Bool :=
  let env := layout_env_from_list env_list
  let fts := field_types_env_from_list ft_list
  let results := PMT.IVE.Soundness.verify_state_writes env fts consumed writes
  results.all (fun r => r.valid)

/-- Soundness bridge for the extracted `lean_verify_transform`:
if the extracted function returns `true`, then the propositional
soundness theorem `verify_transform_sound` applies. -/
theorem lean_verify_transform_sound
    (t : PMT.IVE.Soundness.StateTransform)
    (hcheck : leanVerifyTransform t = true) :
    PMT.WF_Layout t.in_layout
    ∧ PMT.WF_Layout t.out_layout
    ∧ (match t.kind with
       | PMT.IVE.Soundness.TransformKind.identity    => t.in_layout = t.out_layout
       | PMT.IVE.Soundness.TransformKind.reinterpret => t.in_layout.total_size = t.out_layout.total_size
       | PMT.IVE.Soundness.TransformKind.copy        => True) := by
  unfold leanVerifyTransform at hcheck
  -- hcheck : (verify_transform t).valid = true
  exact PMT.IVE.Soundness.verify_transform_sound t hcheck

end PMT.Extraction
