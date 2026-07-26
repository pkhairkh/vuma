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
  * `docs/verification-reports/W6-multi-module-test.md` — Lake split.

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job runs the same command. The
legacy single-file `lean PMT/Basic.lean` invocation does not work
since the multi-module split in Wave 6.
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

/-- §1.2: A field is a (offset, size) pair inside a layout. -/
structure Field where
  offset : Nat
  size   : Nat
  deriving Repr

/-- §1.2: A layout is a total size plus a list of fields. -/
structure Layout where
  total_size : Nat
  fields     : List Field
  deriving Repr

/-- §1.1: Arena well-formedness — `0 ≤ used ≤ capacity` (in `Nat`, `0 ≤ _`
is trivial, so only the upper bound matters). -/
def WF_Arena (a : Arena) : Prop := a.used ≤ a.capacity

/-- §1.2: Two fields are disjoint if their byte ranges do not overlap. -/
def Disjoint (f₁ f₂ : Field) : Prop :=
  f₁.offset + f₁.size ≤ f₂.offset ∨ f₂.offset + f₂.size ≤ f₁.offset

/-- §1.2: `WF_Layout l` — every field is in bounds and every pair of
fields is disjoint. This is the pure-Coq lemma over `PmtLayoutSpec`
mentioned in `pmt-iris-spec.md` §8 (TCB row "Provable"). -/
def WF_Layout (l : Layout) : Prop :=
  (∀ f : Field, f ∈ l.fields → f.offset + f.size ≤ l.total_size)
  ∧ (∀ f₁ f₂ : Field, f₁ ∈ l.fields → f₂ ∈ l.fields → f₁ ≠ f₂ → Disjoint f₁ f₂)
  ∧ (0 < l.total_size ∨ l.fields = [])

/-- The empty (unit-sized) layout is well-formed. -/
def emptyLayout : Layout := ⟨1, []⟩

theorem WF_Layout_empty : WF_Layout emptyLayout := by
  unfold WF_Layout
  refine ⟨?_, ?_, ?_⟩
  · intro _ h; cases h
  · intros _ _ h₁ h₂ _; cases h₁
  · exact Or.inl (by decide)

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
