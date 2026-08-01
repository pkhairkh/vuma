import PMT.Basic
import PMT.Iris.HeapModel

/-!
## Genuine separating conjunction (Wave 1-D, task 1-D; consolidated B-2)

A REAL separation-logic `Sep` with disjoint-domains semantics, defined
over the canonical heap `Heap := Nat → Option Val` from
`PMT.Iris.HeapModel`. This is the genuine Iris separating conjunction
`P ∗ Q`, contrasted with the degenerate AND-Sep in
`PMT.Iris.CapBndInvariant`:

    structure Sep (P Q : Prop) where left : P; right : Q   -- plain AND

which the Round 7 audit flagged as plain conjunction ("the model does
not track a heap", `CapBndInvariant.lean:90-96`).

`Sep P Q h` holds iff there EXIST sub-heaps `h1`, `h2` such that
  * `P h1`              — the left assertion holds on `h1`,
  * `Q h2`              — the right assertion holds on `h2`,
  * `h1.disjoint h2`    — `h1` and `h2` have disjoint domains, and
  * `h1.merge h2 = h`   — `h` is exactly the disjoint union of `h1`, `h2`.

This is the standard "splitting a heap into two disjoint pieces" reading
of `∗`, and is NON-DEGENERATE: two `HeapPointsTo` facts at the same
address cannot both live in the same `Sep` (they would have to occupy
disjoint sub-heaps, but a single address cannot be split).

### Consolidation (task B-2): this file now imports `PMT.Iris.HeapModel`

Previously this file declared its OWN `Heap`/`Val`/`dom`/`merge`/
`disjoint`/`HeapPointsTo` (mirroring `HeapModel`'s API) to dodge an
import cycle: `HeapModel` imported `CapBndInvariant`, and
`CapBndInvariant` imports this file, so importing `HeapModel` here
would have closed `HeapModel → CapBndInvariant → SepGenuine → HeapModel`.

Task B-2 broke that cycle by relocating `HeapModel`'s Ex-RA /
`RealOwn` / `GhostState` block (which depended on `CapBndInvariant`'s
`GhostName`/`ExRA`/`AgRA`) into `CapBndInvariant.lean`, leaving
`HeapModel` a pure heap-API leaf. This file therefore now imports
`HeapModel` and DELETES its local heap-API mirrors — the heap model
logic (`Val`, `dom`, `disjoint`, `merge`, `mergeOpt`, `HeapPointsTo`,
and the `mergeOpt_comm` / `merge_comm_of_disjoint` / `merge_emp_*`
sanity lemmas) lives ONLY in `HeapModel.lean`.

A single thin alias `abbrev Heap := PMT.Iris.Heap.Heap` is retained so
that downstream references to the `GenuineSep.Heap` name (in
`CapBndInvariant.lean`'s `CapBndInvH` / `frame_rule_genuine` and in
`LiveMirrorInvariant.lean`'s `live_mirror_sep_genuine`) keep resolving
without modification — the alias carries NO logic, it just re-exports
the canonical type.

### Why `Sep` is a `def … := ∃ …` rather than a `structure … : Prop`

The 1-D template wrote

    structure Sep (P Q : Heap → Prop) (h : Heap) : Prop where
      left : P h1; right : Q h2; disjoint : …; union_eq : …

with `h1`, `h2` as existentially-bound witnesses. Two project
constraints force an adaptation:

  1. `lakefile.toml` sets `autoImplicit = false`, so the bare `h1`/`h2`
     in the field types cannot be auto-bound as structure fields.
  2. Lean refuses to generate a projection from a `Prop`-valued
     structure to a `Type`-valued field (the same constraint that made
     `Pmt.Ptsto` `Type`-valued in `HeapModel.lean`). Storing
     `h1 h2 : Heap` as fields of a `: Prop` structure is rejected.

The faithful `Prop`-valued encoding is the built-in existential
(`Exists`), which permits `Type` witnesses and is exactly the semantic
reading of the template's structure. We therefore define

    def Sep (P Q : Heap → Prop) (h : Heap) : Prop :=
      ∃ h1 h2 : Heap, P h1 ∧ Q h2 ∧ h1.disjoint h2 ∧ h1.merge h2 = h

which is propositionally the genuine separating conjunction. Wave 2
proofs destructure it with `obtain ⟨h1, h2, hp, hq, hd, hu⟩`.

### Notation

The ascii `P * Q` and the Iris-standard `P ∗ Q` are SCOPED notations in
namespace `PMT.Iris.GenuineSep`, so they only shadow the global `HMul`
`*` when `GenuineSep` is explicitly `open`ed — no clash with the
degenerate AND-Sep (which carries no notation) in `PMT.Iris`.

**References.**
  - `proof/PMT/Iris/CapBndInvariant.lean` — the degenerate `Sep`
    (Wave 2-C will swap it for this genuine one).
  - `proof/Pmt/Sep.lean` — an independent genuine separation-logic
    skeleton (reference for the disjoint-domains API).
  - `proof/PMT/Iris/HeapModel.lean` — the canonical heap model
    (single source of truth for `Heap`/`Val`/`dom`/`merge`/`disjoint`).
-/

namespace PMT.Iris.GenuineSep

/-! ## §1. Heap model (canonical: re-exported from `PMT.Iris.Heap`) -/

/-- Canonical heap alias. `GenuineSep.Heap` is `PMT.Iris.Heap.Heap`
    (defined in `HeapModel.lean`), kept here as a thin `abbrev` so that
    downstream references (`CapBndInvH`, `frame_rule_genuine`,
    `live_mirror_sep_genuine`) that name `GenuineSep.Heap` continue to
    resolve. The full heap API (`Val`, `dom`, `disjoint`, `merge`,
    `mergeOpt`, `HeapPointsTo`, …) lives ONLY in `HeapModel.lean` —
    this module no longer duplicates it. -/
abbrev Heap : Type := PMT.Iris.Heap.Heap

/-! ## §2. Genuine separating conjunction -/

/-- Genuine separating conjunction `P ∗ Q` on a heap `h`.

    `Sep P Q h` holds iff `h` splits into disjoint sub-heaps `h1`, `h2`
    with `P h1`, `Q h2`, `h1.disjoint h2`, and `h1.merge h2 = h`.

    Unlike the degenerate `Sep (P Q : Prop)` in `CapBndInvariant.lean`
    (plain AND, no heap), this `Sep` ENFORCES disjointness — the
    defining feature of Iris's `∗`.

    The disjointness / merge operations are the canonical ones from
    `PMT.Iris.Heap` (`HeapModel.lean`); encoded as a `∃` (see the module
    docstring for why this is the faithful `Prop`-valued adaptation of
    the template's `structure … : Prop`). -/
def Sep (P Q : Heap → Prop) (h : Heap) : Prop :=
  ∃ h1 h2 : Heap,
    P h1 ∧ Q h2 ∧ PMT.Iris.Heap.Heap.disjoint h1 h2 ∧
      PMT.Iris.Heap.Heap.merge h1 h2 = h

/-! ## §3. Sanity lemmas (genuineness witnesses) -/

/-- Commutativity of the genuine `Sep`: `P ∗ Q ⊢ Q ∗ P`. Mirrors the
    Iris `sep_comm` rule, and witnesses that this `Sep` is a real
    separating conjunction (the degenerate AND-Sep's "commutativity" is
    trivial; here disjointness must be re-established symmetrically and
    `merge` re-ordered via `Heap.merge_comm_of_disjoint`). -/
theorem sep_comm (P Q : Heap → Prop) (h : Heap) (H : Sep P Q h) :
    Sep Q P h := by
  obtain ⟨h1, h2, hp, hq, hd, hu⟩ := H
  refine ⟨h2, h1, hq, hp, ?_, ?_⟩
  · -- disjointness is symmetric
    intro x contra
    exact hd x ⟨contra.2, contra.1⟩
  · -- merge commutes under disjointness
    rw [← PMT.Iris.Heap.Heap.merge_comm_of_disjoint h1 h2 hd]
    exact hu

/-! ## §4. Notation -/

/-- The Iris-standard separating-conjunction symbol `∗`, scoped to this
    namespace so it never clashes with the global `HMul` `*`. -/
scoped notation:50 P " ∗ " Q => Sep P Q

/-- The ascii `*` for the genuine `Sep`, scoped to this namespace.
    Active only when `PMT.Iris.GenuineSep` is explicitly `open`ed (or
    `open scoped PMT.Iris.GenuineSep`), so it does NOT clobber the
    global `HMul` `*` by default — opt in to use `P * Q` for the
    genuine separating conjunction. -/
scoped notation:50 P " * " Q => Sep P Q

end PMT.Iris.GenuineSep
