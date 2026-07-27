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

/-! ## §1. The verified capacity check (PMT-FAITH-5-C: BitVec 64, bit-faithful)

**PMT-FAITH-5-C (closes FAITH-4-D CRITICAL):** the previous Lean version used
`Nat` arithmetic (no overflow), so `verified_capacity_check (2^64-1) 1 (2^64)`
returned `true` while Rust's `checked_add` returns `false` on overflow. The
fix uses `BitVec 64` arithmetic with an explicit no-overflow check (matching
Rust's `checked_add` semantics bit-faithfully). The check is:
  `verified_capacity_check used size capacity := no_overflow ∧ sum ≤ capacity`
where `no_overflow := size ≤ usizeMax64 - used` (equivalent to
`used.checked_add(size).is_some` in Rust) and `sum := used + size` (BitVec
wrapping add, but the no_overflow guard ensures no wrap). -/

/-- §1: The verified capacity check (bit-faithful to Rust `checked_add`).

Mirrors `arena.rs::Arena::alloc`'s `checked_add` + `> capacity` pair. Uses
`BitVec 64` to model Rust's `u64` overflow semantics bit-faithfully.

FFI: `@[export lean_verified_capacity_check]` makes this callable
from C/Rust as `lean_verified_capacity_check(lean_object*,
lean_object*, lean_object*) -> uint8_t`. The three `BitVec 64` arguments
are boxed `lean_object*` values. -/
@[export lean_verified_capacity_check]
def verified_capacity_check (used size capacity : BitVec 64) : Bool :=
  -- Mirrors Rust's `used.checked_add(size).map_or(false, |sum| sum <= capacity)`.
  -- The no-overflow guard: `size ≤ usizeMax64 - used` iff `used + size` does
  -- not wrap (where `usizeMax64 = BitVec.allOnes 64 = 2^64 - 1`).
  -- Inlined (no `let`) for easier proof automation.
  (size ≤ BitVec.allOnes 64 - used) ∧ (used + size ≤ capacity)

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
  -- PMT-FAITH-5-C: convert Nat → BitVec 64 for the capacity check (matches
  -- Rust's u64 overflow semantics). The conversion is lossy if used/capacity
  -- ≥ 2^64, but Rust's arena runtime is 64-bit-biased (arena.rs:1), so this
  -- matches the production target.
  verified_capacity_check (BitVec.ofNat 64 used) (BitVec.ofNat 64 layout.total_size) (BitVec.ofNat 64 capacity)
  ∧ verified_field_bounds_check f layout
  ∧ verified_linearity_check var consumed

/-- §5: Soundness — the verified checks are correct.

    **PMT-FAITH-5-C:** `verified_capacity_check_correct` now operates over
    `BitVec 64`. The conclusion is the integer-level bound
    `used.toNat + size.toNat ≤ capacity.toNat` (which, combined with the
    no-overflow guard, is equivalent to Rust's
    `used.checked_add(size) = Some(sum) ∧ sum ≤ capacity`). -/
theorem verified_capacity_check_correct
    (used size capacity : BitVec 64)
    (hcheck : verified_capacity_check used size capacity = true) :
    -- The no-overflow guard ensures `used + size` does not wrap, so the
    -- BitVec sum equals the integer sum. The conclusion is the integer bound.
    size.toNat ≤ (BitVec.allOnes 64).toNat - used.toNat
    ∧ (used + size).toNat ≤ capacity.toNat := by
  unfold verified_capacity_check at hcheck
  -- hcheck : decide (size ≤ BitVec.allOnes 64 - used ∧ used + size ≤ capacity) = true
  rw [decide_eq_true_eq] at hcheck
  obtain ⟨hnoovf, hsum⟩ := hcheck
  refine ⟨?_, ?_⟩
  · -- hnoovf : size ≤ BitVec.allOnes 64 - used
    -- Unfold to toNat-level, then use omega with the allOnes bound.
    have h1 : size.toNat ≤ (BitVec.allOnes 64 - used).toNat := BitVec.le_def.mp hnoovf
    have h_allones : (BitVec.allOnes 64).toNat = 2^64 - 1 := BitVec.toNat_allOnes
    have h_sub : (BitVec.allOnes 64 - used).toNat = 2^64 - 1 - used.toNat := by
      rw [BitVec.toNat_sub, h_allones]
      -- (2^64 - used.toNat + (2^64 - 1)) % 2^64 = 2^64 - 1 - used.toNat
      have h_used : used.toNat < 2^64 := BitVec.isLt used
      omega
    rw [h_sub] at h1
    -- Goal: size.toNat ≤ (BitVec.allOnes 64).toNat - used.toNat
    -- h1 : size.toNat ≤ 2^64 - 1 - used.toNat
    -- Substitute h_allones in the goal.
    rw [h_allones]
    exact h1
  · exact BitVec.le_def.mp hsum

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
    (hcheck : verified_pmt_check used capacity f layout var consumed = true)
    -- PMT-FAITH-5-C: the capacity check now uses BitVec 64, so we need
    -- boundedness hypotheses to ensure the Nat→BitVec conversion is lossless.
    (h_used_bounded : used < 2^64)
    (h_size_bounded : layout.total_size < 2^64)
    (h_cap_bounded : capacity < 2^64) :
    used + layout.total_size ≤ capacity
    ∧ f.offset + f.size ≤ layout.total_size
    ∧ var ∉ consumed := by
  unfold verified_pmt_check at hcheck
  -- hcheck : (verified_capacity_check (BitVec.ofNat 64 used) ... ∧ ...) = true
  -- The conjunction is decidable, so it's wrapped in `decide`. Reduce.
  rw [decide_eq_true_eq] at hcheck
  obtain ⟨hcap, hrest⟩ := hcheck
  obtain ⟨hfb, hlin⟩ := hrest
  refine ⟨?_, ?_, ?_⟩
  · -- Derive the Nat-level capacity bound from the BitVec-level check.
    have hbv := verified_capacity_check_correct
      (BitVec.ofNat 64 used) (BitVec.ofNat 64 layout.total_size) (BitVec.ofNat 64 capacity) hcap
    -- hbv : (BitVec.ofNat 64 layout.total_size).toNat ≤ ... ∧ (used_bv + size_bv).toNat ≤ capacity_bv.toNat
    -- Under the boundedness hypotheses, ofNat.toNat = identity, and the
    -- no-overflow guard ensures the BitVec add doesn't wrap, so the BitVec
    -- sum equals the Nat sum.
    have h_used_eq : (BitVec.ofNat 64 used).toNat = used := by
      rw [BitVec.toNat_ofNat]; omega
    have h_size_eq : (BitVec.ofNat 64 layout.total_size).toNat = layout.total_size := by
      rw [BitVec.toNat_ofNat]; omega
    have h_cap_eq : (BitVec.ofNat 64 capacity).toNat = capacity := by
      rw [BitVec.toNat_ofNat]; omega
    -- The no-overflow conjunct: size_bv.toNat ≤ usizeMax - used_bv.toNat
    -- ensures used + size < 2^64, so the BitVec add doesn't wrap, so
    -- (used_bv + size_bv).toNat = used + size.
    have h_no_overflow_nat : layout.total_size ≤ 2^64 - 1 - used := by
      have := hbv.1
      rw [h_size_eq, h_used_eq, BitVec.toNat_allOnes] at this
      omega
    have h_sum_eq : (BitVec.ofNat 64 used + BitVec.ofNat 64 layout.total_size).toNat = used + layout.total_size := by
      rw [BitVec.toNat_add, h_used_eq, h_size_eq]
      -- Goal: (used + layout.total_size) % 2^64 = used + layout.total_size
      -- Under h_no_overflow_nat (used + size ≤ 2^64 - 1), the modulo is identity.
      omega
    -- Now derive used + layout.total_size ≤ capacity.
    have hsum_cap := hbv.2
    rw [h_sum_eq, h_cap_eq] at hsum_cap
    exact hsum_cap
  · exact verified_field_bounds_check_correct _ _ hfb
  · exact verified_linearity_check_correct _ _ hlin

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
`PMT.IVE.Soundness.verify_transform`. Takes a `LayoutRegistry` (layout
name → LayoutInfo lookup) and a `StateTransform` (pair of layout names),
returns the `valid` Bool. Faithful to Rust's `verify_transform(layouts, input_layout, output_layout)`. -/
@[export lean_verify_transform]
def leanVerifyTransform
    (layouts : PMT.IVE.Soundness.LayoutRegistry)
    (t : PMT.IVE.Soundness.StateTransform) :
    Bool :=
  (PMT.IVE.Soundness.verify_transform_st layouts t).valid

/-- Helper: reconstruct a `String → Option LayoutInfo` function from a list of
(var, LayoutInfo) pairs. Unknown vars map to `none` (mirrors Rust's HashMap.get() → None).
Faithful model: LayoutInfo carries FieldInfo with name/offset/size/type_name. -/
def layout_env_from_list_faith (env_list : List (String × PMT.IVE.Soundness.LayoutInfo))
    (var : String) : Option PMT.IVE.Soundness.LayoutInfo :=
  env_list.lookup var

/-- `@[export lean_verify_state_reads]` — extracted form of
`PMT.IVE.Soundness.verify_state_reads`. Faithful: takes a list of (var, LayoutInfo)
pairs (the env) and a list of StateRead (var + field_name + expected_type).
Returns true iff every read passes. -/
@[export lean_verify_state_reads]
def leanVerifyStateReads
    (env_list : List (String × PMT.IVE.Soundness.LayoutInfo))
    (reads : List PMT.IVE.Soundness.StateRead) : Bool :=
  let env := layout_env_from_list_faith env_list
  let results := PMT.IVE.Soundness.verify_state_reads env reads
  results.all (fun r => r.valid)

/-- `@[export lean_verify_state_writes]` — extracted form of
`PMT.IVE.Soundness.verify_state_writes`. Faithful: takes a list of (var, LayoutInfo)
pairs, a list of consumed var names, and a list of StateWrite (var + field_name +
value_type + after_consume). Returns true iff every write passes. -/
@[export lean_verify_state_writes]
def leanVerifyStateWrites
    (env_list : List (String × PMT.IVE.Soundness.LayoutInfo))
    (consumed : List String)
    (writes : List PMT.IVE.Soundness.StateWrite) : Bool :=
  let env := layout_env_from_list_faith env_list
  let results := PMT.IVE.Soundness.verify_state_writes env consumed writes
  results.all (fun r => r.valid)

/-- Soundness bridge for the extracted `lean_verify_transform`:
if the extracted function returns `true`, then the propositional
soundness theorem `verify_transform_sound` applies. Faithful: concludes
layout existence (NOT WF_Layout — Rust doesn't check it). -/
theorem lean_verify_transform_sound
    (layouts : PMT.IVE.Soundness.LayoutRegistry)
    (t : PMT.IVE.Soundness.StateTransform)
    (hcheck : leanVerifyTransform layouts t = true) :
    (∃ in_info, layouts t.input_layout = some in_info)
    ∧ (∃ out_info, layouts t.output_layout = some out_info) := by
  unfold leanVerifyTransform at hcheck
  have h_sound := PMT.IVE.Soundness.verify_transform_sound layouts t.input_layout t.output_layout hcheck
  exact ⟨h_sound.1, h_sound.2.1⟩

/-! ## §9. Flattened primitive FFI wrappers (Wave 4-A)

The 7 exports in §§1,8 carry Lean-only types in FFI-crossing positions
(`BitVec 64`, `Field`, `Layout`, `List String`, `LayoutRegistry`,
`List (String × LayoutInfo)`, `List StateRead`, `List StateWrite`,
`StateTransform`). Through `@[export]` these become opaque
`lean_object*` arguments that Rust can only build by linking the full
Lean runtime (`lean_ctor` / `lean_alloc`). To keep Rust free of arbitrary
Lean-runtime construction (FFI_BRIDGE_PLAN.md §1, option ii), we add
**flattened primitive wrappers** that accept only C-compatible scalar
types — `Bool`, `UInt64` (unboxed `uint64_t`), and `String` (a single
`lean_object*` Rust builds via `lean_mk_string`) — and reconstruct the
Lean structures internally.

Variable-length inputs (lists of records) are passed as newline /
tab-delimited `String` payloads:
  • `consumed` (List String)            : vars joined by `\n`.
  • registry (List (String × LayoutInfo)): one layout header
    `name \t total_size \t field_count` followed by `field_count`
    field lines `fname \t offset \t size \t type_name`.
  • reads   (List StateRead)            : one line `var \t field_name \t expected_type`.
  • writes  (List StateWrite)           : one line
    `var \t field_name \t value_type \t after_consume` with
    `after_consume ∈ {"0","1"}`.

The original `@[export]`s in §§1,8 are RETAINED (not removed): Rust
callers that do link `lean_runtime` may still pass boxed objects, while
these `_prim` wrappers are the recommended C-marshallable entry points
that need no `lean_ctor`/`lean_alloc` on the Rust side.
-/

/-- Split a string on `\n`, dropping empty lines. `"" → []`. -/
def splitLines (s : String) : List String :=
  (s.splitOn "\n").filter (fun l => l != "")

/-- Parse a `Nat` from a string, defaulting to 0 on failure. -/
def parseNat (s : String) : Nat :=
  s.toNat?.getD 0

/-- Parse a single `FieldInfo` line `name \t offset \t size \t type_name`.
Malformed lines decode to an all-empty/zero FieldInfo. -/
def parseFieldInfo (line : String) : PMT.IVE.Soundness.FieldInfo :=
  match line.splitOn "\t" with
  | [n, o, sz, tn] =>
    { name := n, offset := parseNat o, size := parseNat sz, type_name := tn }
  | _ => { name := "", offset := 0, size := 0, type_name := "" }

/-- Parse a registry payload into `List (String × LayoutInfo)`.
Format: a layout header `name \t total_size \t field_count` followed by
`field_count` field lines. Malformed headers are skipped.

Structurally recursive on a Nat fuel bound (the input line count) so that
Lean's structural-recursion checker accepts it without a termination
proof: each iteration consumes one header line, and the fuel strictly
decreases. -/
def parseRegistryAux : Nat → List String → List (String × PMT.IVE.Soundness.LayoutInfo)
  | 0, _ => []
  | _ + 1, [] => []
  | k + 1, hdr :: rest =>
    match hdr.splitOn "\t" with
    | [name, ts, fc] =>
      let n := parseNat fc
      let fields := (rest.take n).map parseFieldInfo
      let info : PMT.IVE.Soundness.LayoutInfo :=
        { name := name, total_size := parseNat ts, fields := fields }
      (name, info) :: parseRegistryAux k (rest.drop n)
    | _ => parseRegistryAux k rest

/-- Parse a registry payload string (see §9 format). -/
def parseRegistry (s : String) : List (String × PMT.IVE.Soundness.LayoutInfo) :=
  let lines := splitLines s
  parseRegistryAux lines.length lines

/-- Parse a single `StateRead` line `var \t field_name \t expected_type`. -/
def parseStateRead (line : String) : PMT.IVE.Soundness.StateRead :=
  match line.splitOn "\t" with
  | [v, fn, et] => { var := v, field_name := fn, expected_type := et }
  | _ => { var := "", field_name := "", expected_type := "" }

/-- Parse a reads payload string into `List StateRead`. -/
def parseReads (s : String) : List PMT.IVE.Soundness.StateRead :=
  (splitLines s).map parseStateRead

/-- Parse a Bool token: "1" or "true" → true, else false. -/
def parseBoolField (s : String) : Bool :=
  s = "1" ∨ s = "true"

/-- Parse a single `StateWrite` line
`var \t field_name \t value_type \t after_consume`. -/
def parseStateWrite (line : String) : PMT.IVE.Soundness.StateWrite :=
  match line.splitOn "\t" with
  | [v, fn, vt, ac] =>
    { var := v, field_name := fn, value_type := vt, after_consume := parseBoolField ac }
  | _ => { var := "", field_name := "", value_type := "", after_consume := false }

/-- Parse a writes payload string into `List StateWrite`. -/
def parseWrites (s : String) : List PMT.IVE.Soundness.StateWrite :=
  (splitLines s).map parseStateWrite

/-- `@[export lean_verified_capacity_check_prim]` — primitive-signature
wrapper for `verified_capacity_check`. Takes three unboxed `UInt64`
(Rust `u64`) and converts to `BitVec 64` internally. C ABI:
`(uint64_t, uint64_t, uint64_t) -> uint8_t`. -/
@[export lean_verified_capacity_check_prim]
def leanVerifiedCapacityCheckPrim (used size capacity : UInt64) : Bool :=
  verified_capacity_check (BitVec.ofNat 64 used.toNat)
    (BitVec.ofNat 64 size.toNat) (BitVec.ofNat 64 capacity.toNat)

/-- `@[export lean_verified_field_bounds_check_prim]` — primitive-signature
wrapper. Takes `offset size total_size : UInt64` and rebuilds a minimal
`Field` / `Layout` internally (only the three numeric fields affect the
`offset + size ≤ total_size` check). C ABI:
`(uint64_t, uint64_t, uint64_t) -> uint8_t`. -/
@[export lean_verified_field_bounds_check_prim]
def leanVerifiedFieldBoundsCheckPrim (offset size total_size : UInt64) : Bool :=
  let f : Field := { name := "", offset := offset.toNat, size := size.toNat, type_name := "" }
  let l : Layout := { name := "", total_size := total_size.toNat, fields := [] }
  verified_field_bounds_check f l

/-- `@[export lean_verified_linearity_check_prim]` — primitive-signature
wrapper. `consumed` is a `\n`-joined list of already-consumed var names
(empty string ⇒ no consumed vars). C ABI:
`(lean_object*, lean_object*) -> uint8_t`. -/
@[export lean_verified_linearity_check_prim]
def leanVerifiedLinearityCheckPrim (var consumed : String) : Bool :=
  verified_linearity_check var (splitLines consumed)

/-- `@[export lean_verified_pmt_check_prim]` — primitive-signature wrapper
composing the three checks. Takes 5 `UInt64` + `var` + `consumed` and
rebuilds `Field` / `Layout` / consumed-list internally. C ABI:
`(uint64_t ×5, lean_object*, lean_object*) -> uint8_t`. -/
@[export lean_verified_pmt_check_prim]
def leanVerifiedPmtCheckPrim
    (used capacity offset size total_size : UInt64)
    (var consumed : String) : Bool :=
  let f : Field := { name := "", offset := offset.toNat, size := size.toNat, type_name := "" }
  let l : Layout := { name := "", total_size := total_size.toNat, fields := [] }
  verified_pmt_check used.toNat capacity.toNat f l var (splitLines consumed)

/-- `@[export lean_verify_transform_prim]` — primitive-signature wrapper
for `leanVerifyTransform`. `registry` is the serialized registry payload
(see §9 format); `input_layout` / `output_layout` are layout names; `kind`
is the explicit transform kind ("identity" / "reinterpret" / "copy"). The
registry function is reconstructed via `layout_env_from_list_faith`.

Unlike `leanVerifyTransform` (which INFERS the kind from layout-name /
total-size comparison and does NOT check `WF_Layout`), this wrapper takes
an EXPLICIT `kind` and performs the well-formedness + kind-specific checks
that mirror the Rust hand-translation `hand_verify_transform`
(`lean_wf_layout_bool` + `TransformKind` match). This closes the three
parity gaps where the inference-based path returned `valid = true`:
  - Identity with different fields  -> must be `false`.
  - Reinterpret with size mismatch  -> must be `false`.
  - Ill-formed input layout         -> must be `false` (wf check).

C ABI: `(lean_object*, lean_object*, lean_object*, lean_object*) -> uint8_t`. -/
@[export lean_verify_transform_prim]
def leanVerifyTransformPrim (registry input_layout output_layout kind : String) : Bool :=
  let layouts : PMT.IVE.Soundness.LayoutRegistry :=
    layout_env_from_list_faith (parseRegistry registry)
  -- Look up both layouts by name.
  match layouts input_layout, layouts output_layout with
  | some in_info, some out_info =>
    -- wf_layout check (mirrors Rust `lean_wf_layout_bool` first conjunct:
    -- every field is in bounds, i.e. offset + size <= total_size).
    let in_wf := in_info.fields.all (fun f => f.offset + f.size <= in_info.total_size)
    let out_wf := out_info.fields.all (fun f => f.offset + f.size <= out_info.total_size)
    in_wf && out_wf &&
    (match kind with
     | "identity" =>
       -- Identity: same field count + same fields (name, offset, size).
       in_info.fields.length = out_info.fields.length &&
       (List.zip in_info.fields out_info.fields).all
         (fun (a, b) => a.name = b.name && a.offset = b.offset && a.size = b.size)
     | "reinterpret" =>
       -- Reinterpret: same total_size.
       in_info.total_size = out_info.total_size
     | "copy" => true
     | _ => false)
  | _, _ => false

/-- `@[export lean_verify_state_reads_prim]` — primitive-signature wrapper
for `leanVerifyStateReads`. `registry` and `reads` are serialized
payloads. C ABI: `(lean_object*, lean_object*) -> uint8_t`. -/
@[export lean_verify_state_reads_prim]
def leanVerifyStateReadsPrim (registry reads : String) : Bool :=
  leanVerifyStateReads (parseRegistry registry) (parseReads reads)

/-- `@[export lean_verify_state_writes_prim]` — primitive-signature wrapper
for `leanVerifyStateWrites`. `registry`, `consumed`, `writes` are
serialized payloads. C ABI: `(lean_object*, lean_object*, lean_object*) -> uint8_t`. -/
@[export lean_verify_state_writes_prim]
def leanVerifyStateWritesPrim (registry consumed writes : String) : Bool :=
  leanVerifyStateWrites (parseRegistry registry) (splitLines consumed) (parseWrites writes)

end PMT.Extraction
