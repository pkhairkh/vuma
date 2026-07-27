import PMT.Basic

/-!
## Genuine separating conjunction (Wave 1-D, task 1-D)

A REAL separation-logic `Sep` with disjoint-domains semantics, defined
over a heap `Heap := Nat → Option Val`. This is the genuine Iris
separating conjunction `P ∗ Q`, contrasted with the degenerate AND-Sep
in `PMT.Iris.CapBndInvariant`:

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

### Why this file does not import `PMT.Iris.HeapModel`

`PMT.Iris.HeapModel` defines an identical `Heap := Nat → Option Val` and
`Val`, BUT it `import`s `PMT.Iris.CapBndInvariant`. Task 1-D must add
`import PMT.Iris.SepGenuine` to `CapBndInvariant.lean`, so importing
`HeapModel` here would create an import CYCLE:

    HeapModel → CapBndInvariant → SepGenuine → HeapModel

Lean rejects import cycles. To stay within the 1-D file budget (≤1 new
file + ≤1 modified file — `HeapModel` cannot be touched) AND keep the
build green, this file declares its own `Heap`/`Val`/`dom`/`merge`
(mirroring `HeapModel`'s API). Wave 2-A will break the cycle — e.g. by
removing `HeapModel`'s import of `CapBndInvariant` (the heap model is a
leaf and should not depend on a named invariant), or by relocating
`Heap`/`dom`/`merge` into a leaf module that both `HeapModel` and
`SepGenuine` import — after which `SepGenuine` can reference
`HeapModel`'s definitions directly and the local copies here are
deleted.

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
  - `proof/PMT/Iris/HeapModel.lean` — the non-degenerate heap model
    whose `Heap`/`Val` this file mirrors (to be unified in Wave 2-A).
-/

namespace PMT.Iris.GenuineSep

/-! ## §1. Heap model (mirrors `PMT.Iris.Heap`) -/

/-- Value type for the heap model. Mirrors `PMT.Iris.Heap.Val` in
    `HeapModel.lean`; kept local here to avoid the import cycle
    documented above (Wave 2-A will unify). -/
inductive Val where
  | nat  : Nat → Val
  | bool : Bool → Val
  | unit : Val
  deriving Repr, DecidableEq

/-- A heap is a partial function from addresses (natural numbers) to
    optional values — the standard separation-logic heap model,
    identical in shape to `PMT.Iris.Heap.Heap`. -/
def Heap : Type := Nat → Option Val

/-- The empty heap: undefined at every address. -/
def Heap.emp : Heap := fun _ => none

/-- `Heap.dom h x` holds iff `h` is defined at address `x`. Represented
    as a predicate (`Nat → Prop`) rather than a `Set` to avoid a Mathlib
    dependency. -/
def Heap.dom (h : Heap) (x : Nat) : Prop := h x ≠ none

/-- Disjointness of two heap domains: no address is defined in both
    `h1` and `h2`. -/
def Heap.disjoint (h1 h2 : Heap) : Prop := ∀ x, ¬ (h1.dom x ∧ h2.dom x)

/-- Pointwise disjoint union of two optional values: `some` wins on the
    left, otherwise the right value is used. Factored out as a named
    function (rather than an anonymous `match`) so that lemmas about it
    — e.g. `mergeOpt_comm` below — have a clean return type without a
    literal `match` that would over-generalise the `disjoint`-ness
    hypothesis. -/
def mergeOpt (a b : Option Val) : Option Val :=
  match a, b with
  | some v, _ => some v
  | none,   v => v

/-- Disjoint union of two heaps. At each address, the result is `h1`'s
    value if `h1` is defined there, otherwise `h2`'s value. When `h1`,
    `h2` are disjoint this is the true disjoint union (well-defined
    regardless, but the disjoint case is what `Sep` requires). -/
def Heap.merge (h1 h2 : Heap) : Heap := fun x => mergeOpt (h1 x) (h2 x)

/-- Real points-to fact: `HeapPointsTo h loc v` holds iff `h loc = some
    v`. Mirrors `PMT.Iris.Heap.HeapPointsTo`; provided here so Wave 2
    can state the non-degeneracy of `Sep` (two points-to at the same
    `loc` cannot both live in the same `Sep`). -/
def HeapPointsTo (h : Heap) (loc : Nat) (v : Val) : Prop :=
  h loc = some v

/-! ## §2. Genuine separating conjunction -/

/-- Genuine separating conjunction `P ∗ Q` on a heap `h`.

    `Sep P Q h` holds iff `h` splits into disjoint sub-heaps `h1`, `h2`
    with `P h1`, `Q h2`, `h1.disjoint h2`, and `h1.merge h2 = h`.

    Unlike the degenerate `Sep (P Q : Prop)` in `CapBndInvariant.lean`
    (plain AND, no heap), this `Sep` ENFORCES disjointness — the
    defining feature of Iris's `∗`.

    Encoded as a `∃` (see the module docstring for why this is the
    faithful `Prop`-valued adaptation of the template's
    `structure … : Prop`). -/
def Sep (P Q : Heap → Prop) (h : Heap) : Prop :=
  ∃ h1 h2 : Heap, P h1 ∧ Q h2 ∧ h1.disjoint h2 ∧ h1.merge h2 = h

/-! ## §3. Sanity lemmas (genuineness witnesses) -/

/-- `mergeOpt` is commutative on disjoint optional values: when `a`,
    `b` are not both `some`, `mergeOpt a b = mergeOpt b a`. The proof is
    a 4-way case split; the `some, some` case is ruled out by the
    disjointness hypothesis `hd`. -/
theorem mergeOpt_comm (a b : Option Val)
    (hd : ¬ (a ≠ none ∧ b ≠ none)) : mergeOpt a b = mergeOpt b a := by
  rcases a with _ | v <;> rcases b with _ | w
  · rfl
  · rfl
  · rfl
  · exfalso
    apply hd
    refine ⟨?_, ?_⟩ <;> (intro c; cases c)

/-- `merge` is commutative on disjoint heaps: when `h1`, `h2` have
    disjoint domains, `h1.merge h2 = h2.merge h1`. (Without
    disjointness this fails at addresses where both are defined with
    different values.) -/
theorem Heap.merge_comm_of_disjoint (h1 h2 : Heap)
    (hd : h1.disjoint h2) : h1.merge h2 = h2.merge h1 := by
  funext x
  unfold Heap.merge
  exact mergeOpt_comm (h1 x) (h2 x) (hd x)

/-- `emp` is a left unit for `merge`. -/
theorem Heap.merge_emp_left (h : Heap) : Heap.emp.merge h = h := by
  funext x
  unfold Heap.merge Heap.emp
  rfl

/-- `emp` is a right unit for `merge`. -/
theorem Heap.merge_emp_right (h : Heap) : h.merge Heap.emp = h := by
  funext x
  unfold Heap.merge Heap.emp
  cases h x with
  | none => rfl
  | some _ => rfl

/-- Commutativity of the genuine `Sep`: `P ∗ Q ⊢ Q ∗ P`. Mirrors the
    Iris `sep_comm` rule, and witnesses that this `Sep` is a real
    separating conjunction (the degenerate AND-Sep's "commutativity" is
    trivial; here disjointness must be re-established symmetrically and
    `merge` re-ordered via `merge_comm_of_disjoint`). -/
theorem sep_comm (P Q : Heap → Prop) (h : Heap) (H : Sep P Q h) :
    Sep Q P h := by
  obtain ⟨h1, h2, hp, hq, hd, hu⟩ := H
  refine ⟨h2, h1, hq, hp, ?_, ?_⟩
  · -- disjointness is symmetric
    intro x contra
    exact hd x ⟨contra.2, contra.1⟩
  · -- merge commutes under disjointness
    rw [← Heap.merge_comm_of_disjoint h1 h2 hd]
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
