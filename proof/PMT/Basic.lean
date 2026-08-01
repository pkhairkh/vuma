/-!
# PMT Basic — §1 Arena Model + §2 Capacity Invariant (sorry-free)

A machine-checkable formalization of the core data model of the PMT
(Programs as Memory Transformations) memory model used by the VUMA
compiler. This module encodes the Iris specification in
`docs/architecture/pmt-iris-spec.md` (§1–§2) into plain Lean 4.

**Scope.** This module defines the arena/field/layout data model
(§1) and the capacity-preservation invariant (§2). The pure-arithmetic
`alloc_preserves_capacity` lemma is proved sorry-free via `omega`.

This is the bottom of the PMT module dependency stack:
  * `PMT.Basic` (this file) — depended on by `PMT.Field`, `PMT.Liveness`,
    `PMT.Soundness`, `PMT.PmtInstr`, `PMT.IRProgram`, `PMT.RawArena`,
    `PMT.WellTypedStrong`, `PMT.SimRel`, `PMT.ExecFunction`,
    `PMT.Extraction`, and the `PMT/Test/*` regression suite.

**References.**
  * `docs/architecture/pmt-formal-spec.md` — invariants + proof sketches.
  * `docs/architecture/pmt-iris-spec.md`  — Iris encoding (§1 ArenaRes,
    §2 StateValRes, §3 capacity_inv).

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job runs the same command. The
legacy single-file `lean PMT/Basic.lean` invocation does not work
since the multi-module split.
-/

namespace PMT

/-! ## §1. PMT State Model -/

/-- §1.1: Arena = (base, capacity, used). `base` is a byte offset into the
single mmap'd backing buffer; `capacity` is fixed at allocation;
`used` is the bump pointer. -/
structure Arena where
  base     : Nat
  capacity : Nat
  used     : Nat
  deriving Repr

/-- §1.2: A field is a (offset, size) pair inside a layout.

**PMT-FAITH-6-C:** added `name` and `type_name` fields to bit-faithfully
mirror Rust `FieldInfo { name, offset, size, type_name }` (state_read.rs:29-34).
The previous Lean `Field` had only `{offset, size}` — dropped `name` (needed
for field-name lookup) and `type_name` (needed for type-mismatch check).
Closes FAITH-1-A. -/
structure Field where
  name      : String
  offset    : Nat
  size      : Nat
  type_name : String
  deriving Repr

/-- §1.2: A layout is a total size plus a list of fields.

**PMT-FAITH-6-C:** added `name` field to bit-faithfully mirror Rust
`LayoutInfo { name, total_size, fields }` (state_read.rs:22-26). The
previous Lean `Layout` had only `{total_size, fields}` — dropped `name`
(needed for layout-not-found failure path). Closes FAITH-1-B. -/
structure Layout where
  name       : String
  total_size : Nat
  fields     : List Field
  deriving Repr

/-- §1.1: Arena well-formedness — `0 ≤ used ≤ capacity` (in `Nat`, `0 ≤ _`
is trivial, so only the upper bound matters). -/
def WF_Arena (a : Arena) : Prop := a.used ≤ a.capacity

/-- §1.2: Two fields are disjoint if their byte ranges do not overlap. -/
def Disjoint (f₁ f₂ : Field) : Prop :=
  f₁.offset + f₁.size ≤ f₂.offset ∨ f₂.offset + f₂.size ≤ f₁.offset

/-- §1.2: `WF_Layout l` — every field is in bounds.

**PMT-FAITH-6-C (closes FAITH-1-C/D):** the previous `WF_Layout` had THREE
conjuncts: (1) per-field bounds, (2) pairwise disjointness, (3) size>0 ∨
fields=[]. Rust's IVE (`verify_state_reads` state_read.rs:82) only checks
conjunct (1) — conjuncts (2) and (3) were INVENTED by the Lean model,
making the Lean hypothesis STRICTER than Rust's actual check. Per
faithfulness rule 10, soundness-strengthening assumptions must be explicit
hypotheses, not buried in the predicate. The disjointness and non-empty
conjuncts are now SEPARATE predicates (`WF_Layout_Disjoint`,
`WF_Layout_NonEmpty`) that callers must provide explicitly where needed. -/
def WF_Layout (l : Layout) : Prop :=
  (∀ f : Field, f ∈ l.fields → f.offset + f.size ≤ l.total_size)

/-- §1.2 (PMT-FAITH-6-C): `WF_Layout_Disjoint l` — every pair of distinct
fields is disjoint. This is a Lean-side STRENGTHENING assumption (Rust's
IVE does NOT enforce disjointness). Callers that need this must provide it
as an explicit hypothesis. -/
def WF_Layout_Disjoint (l : Layout) : Prop :=
  ∀ f₁ f₂ : Field, f₁ ∈ l.fields → f₂ ∈ l.fields → f₁ ≠ f₂ → Disjoint f₁ f₂

/-- §1.2 (PMT-FAITH-6-C): `WF_Layout_NonEmpty l` — `0 < total_size ∨
fields = []`. This is a Lean-side sanity check (Rust's IVE does NOT enforce
it). Callers that need this must provide it as an explicit hypothesis. -/
def WF_Layout_NonEmpty (l : Layout) : Prop :=
  0 < l.total_size ∨ l.fields = []

/-- The empty (unit-sized) layout is well-formed. -/
def emptyLayout : Layout := ⟨"empty", 1, []⟩

theorem WF_Layout_empty : WF_Layout emptyLayout := by
  unfold WF_Layout
  intro _ h; cases h

/-! ## §2. Capacity Preservation Invariant -/

/-- §2: `CapacityInvariant a := a.used ≤ a.capacity`. Mirrors Iris
`capacity_inv A := ⌜A.used ≤ A.cap⌝` (`pmt-iris-spec.md` §3). -/
def CapacityInvariant (a : Arena) : Prop := a.used ≤ a.capacity

/-- §1.3: `alloc a l` — bump-allocate a layout-sized region. Returns the
new arena (used advanced by `l.total_size`). The overflow path
(`__arena_overflow`, exit 1) is modeled at the theorem level: the
precondition `l.total_size + a.used ≤ a.capacity` is exactly the guard
tested by `arena_alloc` (`pmt-formal-spec.md` §1.3). -/
def alloc (a : Arena) (l : Layout) : Arena :=
  { a with used := a.used + l.total_size }

/-- §3 (Iris) / §2 (formal): `alloc_preserves_capacity`.

    `{{ ArenaRes A ∗ ⌜A.used + sz ≤ A.cap⌝ }} arena_alloc L
     {{ v, SubArena A A.used sz ∗ ArenaRes⟨A.used += sz⟩ }}`

In our pure model, this collapses to a single arithmetic step:
the new `used` is `a.used + l.total_size`, which is `≤ a.capacity` by
the guard hypothesis. -/
theorem alloc_preserves_capacity
    (a : Arena) (l : Layout)
    (_hcap : CapacityInvariant a)
    (_hwf  : WF_Layout l)
    (hfit : l.total_size + a.used ≤ a.capacity) :
    CapacityInvariant (alloc a l) := by
  -- `(alloc a l).used` reduces definitionally to `a.used + l.total_size`.
  show a.used + l.total_size ≤ a.capacity
  -- `hfit` is `l.total_size + a.used ≤ a.capacity`; commutative — `omega`.
  omega

end PMT
