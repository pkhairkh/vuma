import PMT.Basic
import PMT.Field

/-!
## Iris Heap Model — real heap, points-to, disjoint union (canonical)

This module is the **canonical heap model** underpinning the Iris
invariants in `proof/PMT/Iris/`. It provides the real separation-logic
heap `Heap := Nat → Option Val` together with `Heap.read`, `Heap.write`,
the points-to predicate `HeapPointsTo`, and the disjoint-domains API
(`Heap.emp`, `Heap.dom`, `Heap.disjoint`, `Heap.merge`) that the genuine
separating conjunction in `SepGenuine.lean` consumes directly.

### Consolidation note (Wave 2-A follow-up, task B-2)

This module is now a **leaf**: it imports only `PMT.Basic` and
`PMT.Field` (the latter for `Liveness`, used by `encode_liveness`).

It previously imported `PMT.Iris.CapBndInvariant`. That import created
an import-cycle OBSTACLE: `CapBndInvariant.lean` imports `SepGenuine`,
and the genuine separating conjunction wanted to import `HeapModel` for
the heap API, which would have closed the cycle
`HeapModel → CapBndInvariant → SepGenuine → HeapModel`.

To break the cycle within the ≤3-file budget, two relocations were made
(task B-2):

  1. **Ex RA + non-degenerate ownership moved to `CapBndInvariant.lean`.**
     `HeapModel` previously also defined the Ex resource algebra
     (`Ex`, `Ex.op`, `ex_op_comm`, `ex_op_assoc`, `ex_op_self_iff_none`,
     `ex_exclusive`, `ex_exclusive'`) and the non-degenerate ownership
     predicate (`GhostState`, `RealOwn`, `real_own_exclusive`,
     `own_ex_exclusive_derived`, `GhostState.instExRA`,
     `GhostState.instAgRA`). Those definitions depend on `GhostName` /
     `ExRA` / `AgRA`, which live in `CapBndInvariant.lean`; they were
     referenced by NO other code module (only by audit docs), so they
     were **relocated to `CapBndInvariant.lean`** — the module that
     already owns that ghost-state infrastructure. The relocation is
     build-neutral and removes `HeapModel`'s dependency on
     `CapBndInvariant`.

  2. **`SepGenuine.lean` now imports `HeapModel`.** Its local mirrors of
     the heap API (`inductive Val`, `def Heap`, `def Heap.dom`,
     `def Heap.disjoint`, `def mergeOpt`, `def Heap.merge`,
     `def HeapPointsTo`, and the duplicated sanity lemmas) are DELETED;
     the genuine `Sep` now references `PMT.Iris.Heap.Heap` /
     `PMT.Iris.Heap.Heap.disjoint` / `PMT.Iris.Heap.Heap.merge`
     directly. A thin `abbrev Heap := PMT.Iris.Heap.Heap` is retained so
     that downstream references to the `GenuineSep.Heap` name (in
     `CapBndInvariant.lean` and `LiveMirrorInvariant.lean`) keep
     resolving without touching those files.

After B-2 the import graph is acyclic:

    HeapModel (leaf)  ←  SepGenuine  ←  CapBndInvariant
                                ↑                  │
                                └──────────────────┘ (CapBndInvH /
                                                     frame_rule_genuine
                                                     use GenuineSep.Sep)

**References.**
  - `proof/PMT/Iris/SepGenuine.lean` — the genuine `Sep`, now importing
    this module for the heap API.
  - `proof/PMT/Iris/CapBndInvariant.lean` — `GhostName`, `ExRA`, `AgRA`,
    `Own`, and (relocated) the Ex RA + `RealOwn` / `GhostState`.
-/

namespace PMT.Iris.Heap

/-! ## §1. Values and the real heap model -/

/-- Value type for the heap model. Real Iris values are a much richer
    inductive (closures, thunks, ...); here we model only the three
    value shapes that the PMT execution model produces: naturals
    (bump pointers / read results), booleans (liveness flags), and
    unit (write acks, transform acks).

    This is the SAME type as `PMT.Iris.Val` in
    `WeakestPrecond.lean`; we declare it here so that `Heap` can refer
    to it, and `WeakestPrecond.lean` re-uses it via `open PMT.Iris.Heap`. -/
inductive Val where
  | nat  : Nat → Val
  | bool : Bool → Val
  | unit : Val
  deriving Repr

/-- A heap is a function from locations (natural numbers) to optional
    values. This is the standard separation-logic heap model. -/
def Heap : Type := Nat → Option Val

/-- `Heap.read h loc` — read the value at `loc` (returns `none` if the
    location is uninitialised). -/
def Heap.read (h : Heap) (loc : Nat) : Option Val := h loc

/-- `Heap.write h loc v` — write `v` to `loc`, returning the updated
    heap. -/
def Heap.write (h : Heap) (loc : Nat) (v : Val) : Heap :=
  fun l => if l = loc then some v else h l

/-- Real points-to fact: `HeapPointsTo h loc v` holds iff `h loc = some v`.
    This is the standard separation-logic points-to predicate, modelled
    against the concrete heap `h`. -/
def HeapPointsTo (h : Heap) (loc : Nat) (v : Val) : Prop :=
  h loc = some v

/-- Single-valued-ness of the heap: two points-to facts at the same
    location force agreement on the value. This is the **heap-model
    counterpart** of the Ex RA's exclusivity principle — both express
    "at most one exclusive owner per location". -/
theorem HeapPointsTo.exclusive (h : Heap) (loc : Nat) (v₁ v₂ : Val)
    (h₁ : HeapPointsTo h loc v₁) (h₂ : HeapPointsTo h loc v₂) :
    v₁ = v₂ := by
  unfold HeapPointsTo at *
  rw [h₁] at h₂
  injection h₂

/-- Encoding of `Liveness` as a `Val` (for the physical mirror
    `(liveness_byte v) ↦{1} encode(b)`). `live` ↦ `bool true`,
    `dead` ↦ `bool false`. -/
def encode_liveness : Liveness → Val
  | Liveness.live => Val.bool true
  | Liveness.dead => Val.bool false

/-- `encode_liveness` is injective: two distinct `Liveness` values
    encode to distinct `Val`s. Used by `live_mirror_exclusive` to
    derive `live = dead` from the heap-model contradiction
    `encode_liveness live = encode_liveness dead`. -/
theorem encode_liveness_inj {b₁ b₂ : Liveness}
    (h : encode_liveness b₁ = encode_liveness b₂) : b₁ = b₂ := by
  cases b₁ <;> cases b₂ <;> simp [encode_liveness] at h ⊢

/-! ## §1½. Heap domain, disjointness, and disjoint union

These are the separation-logic operations on `Heap` that the genuine
separating conjunction (in `SepGenuine.lean`) needs. They were
previously mirrored as LOCAL copies in `SepGenuine.lean` (Wave 1-D) to
dodge an import cycle; task B-2 breaks that cycle and unifies them
HERE as the single source of truth, so `SepGenuine` imports this module
directly and deletes its mirrors. -/

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
    function (rather than an anonymous `match`) so that
    `mergeOpt_comm` below has a clean return type without a literal
    `match` that would over-generalise the disjointness hypothesis. -/
def mergeOpt (a b : Option Val) : Option Val :=
  match a, b with
  | some v, _ => some v
  | none,   v => v

/-- Disjoint union of two heaps. At each address, the result is `h1`'s
    value if `h1` is defined there, otherwise `h2`'s value. When `h1`,
    `h2` are disjoint this is the true disjoint union (well-defined
    regardless, but the disjoint case is what `Sep` requires). -/
def Heap.merge (h1 h2 : Heap) : Heap := fun x => mergeOpt (h1 x) (h2 x)

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
    disjoint domains, `h1.merge h2 = h2.merge h1`. -/
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

/-- `Inhabited Liveness` instance with `default := Liveness.live`.
    (Auto-derivable in Lean 4 from the no-argument constructor
    `Liveness.live`, but declared explicitly here so that the relocated
    `GhostState.instExRA` instance in `CapBndInvariant.lean` — which is
    general in `[Inhabited α]` — resolves cleanly for `α = Liveness` via
    this transitive instance.) -/
instance : Inhabited Liveness := ⟨Liveness.live⟩

end PMT.Iris.Heap
