import PMT.Basic
import PMT.Soundness
import PMT.RawArena
import PMT.WellTypedStrong

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
    "verified_pmt_check" ]

end PMT.Extraction
