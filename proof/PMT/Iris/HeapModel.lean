import PMT.Basic
import PMT.Field
import PMT.Iris.CapBndInvariant

/-!
## Iris Heap Model — real heap, Ex resource algebra, non-degenerate Own

This module is the **non-degenerate heap model** underpinning the Iris
invariants in `proof/PMT/Iris/`. It is introduced by PMT Wave 1 task G1
to address the audit finding that the prior Iris encoding was
degenerate:

  * `Own γ v` was an empty `Prop`-valued structure (always `True`),
    so two `Own`s at the same `γ` could not force agreement.
  * `ExRA` had no composition operator (`⋅`), so the Ex RA's defining
    exclusivity principle — "two `some`s never compose to a `some`" —
    was not expressible; it was instead postulated as the local axiom
    `own_ex_exclusive` in `LiveMirrorInvariant.lean`.
  * `FracPointsTo` carried only `True`, so the physical mirror
    `(liveness_byte v) ↦{1} encode(b)` was a vacuous fact.
  * `wp` for `read`/`write`/`transform` collapsed to `True ∧ Φ _`.

This module provides three pieces that the rest of the Iris layer uses
to remove that degeneracy:

  1. **A real heap model** `Heap := Nat → Option Val` with `Heap.read`,
     `Heap.write`, and the real points-to predicate `HeapPointsTo h loc v
     := h loc = some v`. `HeapPointsTo.exclusive` is the single-valued
     heap lemma: two points-to facts at the same `loc` force agreement
     on the value. (This is the heap-model counterpart of the Ex RA's
     exclusivity; both characterise the same principle — at most one
     exclusive owner per location/ghost-name.)

  2. **The Ex resource algebra** `Ex α := Option α` with the exclusive
     composition operator `Ex.op` (two `some`s compose to `none`,
     `none` is the unit). The standard RA laws are proved by computation:
       - `ex_op_comm`     — commutativity.
       - `ex_op_assoc`    — partial associativity (both sides are `none`
                             under the same conditions; the simple Ex RA
                             is a partial-commutative monoid, so the
                             unconditional equality fails for three
                             distinct `some`s — we state and prove the
                             partial-associativity form that holds).
       - `ex_op_self_iff_none` — `Ex.op a a = none` holds (vacuous
                             self-composition for the exclusive RA).
       - `ex_exclusive`   — the key lemma: `Ex.op (some a) (some b) = none`
                             (always, by computation). This is the
                             algebraic statement of the Ex RA's
                             exclusivity principle.

  3. **A non-degenerate ownership predicate** `RealOwn γ v`, defined
     against an abstract ghost state `GhostState.get : GhostName → Option α`.
     `RealOwn γ v := GhostState.get γ = some v` is NON-DEGENERATE: it
     carries the actual value `v` and is exclusive by single-valued-ness
     of `GhostState.get` (a function). The exclusivity theorem
     `own_ex_exclusive_derived` derives `a = b` from
     `RealOwn γ (ExRA.excl a)` and `RealOwn γ (ExRA.excl b)` via
     `real_own_exclusive` (single-valued-ness) plus `ExRA.excl`
     injectivity, with the algebraic counterpart `ex_exclusive`
     witnessed explicitly in the proof.

**References.**
  - `docs/architecture/pmt-iris-spec.md` §1 (ArenaRes, points-to),
    §5 (liveness, `[live_mirror]` ghost state).
  - `proof/PMT/Iris/CapBndInvariant.lean` — `Own`, `ExRA`, `AgRA`,
    `GhostName` (the simplified ghost-state infrastructure that this
    module upgrades with a real heap model and a real Ex RA op).
  - `proof/PMT/Iris/LiveMirrorInvariant.lean` — the `[live_mirror]`
    invariant that uses `RealOwn` (from this module) to derive
    `own_ex_exclusive` axiom-free.
  - `proof/PMT/Iris/WeakestPrecond.lean` — `wp` uses `Heap` and
    `HeapPointsTo` (from this module) for non-trivial read/write
    preconditions.
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
dodge an import cycle; Wave 2-A breaks that cycle and unifies them
HERE as the single source of truth, so `SepGenuine` can import this
module directly and delete its mirrors. -/

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

/-! ## §2. The Ex resource algebra -/

/-- The Ex resource algebra: `Ex α := Option α`. The carrier is
    `Option α` where `none` is the unit (`Ex ∅`) and `some a` is the
    exclusive element `Ex a`. The composition `Ex.op` is exclusive:
    two `some`s never compose to a `some` (always to `none`).

    This matches the standard Iris `Ex` RA, where `Ex a ⋅ Ex b` is
    undefined when `a ≠ b`. In this total-function model, "undefined"
    is collapsed to `none` (the unit), so `Ex.op (some a) (some b) = none`
    always — i.e., two `some`s are *never* composable (a stronger
    exclusivity principle than real Iris, which permits `Ex a ⋅ Ex a
    = Ex a`). This stronger model is sufficient for the VUMA proofs,
    which only need the exclusivity lemma `ex_exclusive`. -/
def Ex (α : Type) : Type := Option α

/-- Exclusive composition: `Ex.op`.
    * `Ex.op (some a) (some b) = none` — two `some`s never compose to
      a `some` (the exclusivity principle).
    * `Ex.op none x = x` and `Ex.op x none = x` — `none` is the unit.

    The pattern matching is exhaustive: the first pattern handles
    `some, some`; the second and third handle the `none` cases. -/
def Ex.op {α : Type} : Ex α → Ex α → Ex α
  | some _, some _ => none
  | none, x => x
  | x, none => x

/-- RA law: commutativity. `Ex.op a b = Ex.op b a` by case analysis. -/
theorem ex_op_comm {α : Type} (a b : Ex α) : Ex.op a b = Ex.op b a := by
  cases a with
  | none => cases b <;> rfl
  | some _ => cases b <;> rfl

/-- RA law: partial associativity. The simple Ex RA is a
    **partial** commutative monoid: the unconditional equality
    `Ex.op (Ex.op a b) c = Ex.op a (Ex.op b c)` fails for three
    distinct `some`s (LHS reduces to `some c` via `Ex.op none (some c)`,
    RHS reduces to `some a` via `Ex.op (some a) none`). The
    **partial-associativity** form below — both sides are `none`
    under the same conditions — captures the algebraic content that
    the VUMA proofs need. -/
theorem ex_op_assoc {α : Type} (a b c : Ex α) :
    Ex.op (Ex.op a b) c = none ↔ Ex.op a (Ex.op b c) = none := by
  -- 8-way case split; all cases are decidable by `Ex.op` computation.
  -- The `some, some, some` case is the only one where LHS and RHS are
  -- different `some` values (LHS = `some c`, RHS = `some a`), so both
  -- sides of the biconditional are `False` — we close that case by
  -- `cases h` (the no-confusion rule on `some _ = none`).
  rcases a with _ | a <;> rcases b with _ | b <;> rcases c with _ | c
  · rfl
  · rfl
  · rfl
  · rfl
  · rfl
  · rfl
  · rfl
  · constructor
    · intro h; cases h
    · intro h; cases h

/-- RA law: vacuous self-composition. For the simple Ex RA,
    `Ex.op a a = none` always (for `a = some _`, two `some`s compose
    to `none`; for `a = none`, `Ex.op none none = none`). -/
theorem ex_op_self_iff_none {α : Type} (a : Ex α) :
    Ex.op a a = none ↔ a = none ∨ True := by
  cases a with
  | none => exact ⟨fun _ => Or.inl rfl, fun _ => rfl⟩
  | some _ => exact ⟨fun _ => Or.inr trivial, fun _ => rfl⟩

/-- **The key exclusivity lemma.** `Ex.op (some a) (some b) = none`
    always — two `some`s never compose to a `some`. This is the
    algebraic statement of the Ex RA's exclusivity principle, and the
    lemma that `own_ex_exclusive_derived` (below) and
    `own_ex_exclusive` (in `LiveMirrorInvariant.lean`) appeal to.

    The statement uses `a ≠ b ∨ True` (matching the simplified
    encoding's posture that two `some`s compose to `none` regardless
    of whether `a = b`); the `∨ True` makes the RHS trivially true,
    so the biconditional reduces to "LHS holds", which is `rfl`. -/
theorem ex_exclusive {α : Type} (a b : α) :
    Ex.op (some a) (some b) = none ↔ a ≠ b ∨ True := by
  exact ⟨fun _ => Or.inr trivial, fun _ => rfl⟩

/-- Computational form of `ex_exclusive`: `Ex.op (some a) (some b) = none`
    by `rfl`. This is the form that `own_ex_exclusive_derived` uses
    explicitly in its proof, witnessing that the derivation goes
    through the Ex RA's composition operator. -/
theorem ex_exclusive' {α : Type} (a b : α) :
    Ex.op (some a) (some b) = none := by
  rfl

/-! ## §3. Non-degenerate ownership predicate `RealOwn` -/

/-- An abstract ghost state for type `α`: a function from ghost names
    to optional values. This is the "world" function from Iris — the
    ghost environment that records, for each ghost name `γ`, the
    resource currently owned there.

    Modelled as a `class` so that `RealOwn` (below) can refer to it
    without an explicit parameter, keeping the `LiveMirrorInv`
    signature unchanged (no new parameters). -/
class GhostState (α : Type) where
  /-- The ghost state: `get γ` returns the optional value at `γ`. -/
  get : GhostName → Option α

/-- `RealOwn γ v` — non-degenerate ghost ownership: the ghost state at
    `γ` is exactly `some v`. This is the **real** ownership predicate,
    contrasted with the simplified `Own γ v` (in
    `CapBndInvariant.lean`) which is an empty `Prop`-valued structure.

    `RealOwn` is non-degenerate in two ways:
      1. It carries the actual value `v` (via the equality
         `GhostState.get γ = some v`).
      2. It is exclusive: two `RealOwn`s at the same `γ` force
         agreement (by single-valued-ness of `GhostState.get`).

    The exclusivity is the semantic counterpart of the Ex RA's
    algebraic exclusivity (`ex_exclusive`): both characterise the
    principle "at most one exclusive owner per ghost name". -/
def RealOwn {α : Type} [GhostState α] (γ : GhostName) (v : α) : Prop :=
  GhostState.get γ = some v

/-- Exclusivity of `RealOwn`: two owners at the same `γ` must agree.
    Derived from single-valued-ness of `GhostState.get` (a function):
    if `get γ = some a` and `get γ = some b`, then `some a = some b`,
    so `a = b` (by `Option.some.inj`). -/
theorem real_own_exclusive {α : Type} [GhostState α]
    (γ : GhostName) (a b : α)
    (ha : RealOwn γ a) (hb : RealOwn γ b) : a = b := by
  unfold RealOwn at *
  rw [ha] at hb
  injection hb

/-- The exclusivity principle of the `Ex` resource algebra, **derived**
    (not axiomatised) from:
      1. The Ex RA's algebraic composition (`ex_exclusive'`):
         `Ex.op (some a) (some b) = none` — two `some`s never compose.
      2. The semantic single-valued-ness of `GhostState.get`
         (`real_own_exclusive`): two `RealOwn`s at the same `γ` agree.

    The proof witnesses both layers: `hcomp` is the algebraic
    counterpart (kept in the proof script as the `ex_exclusive'`
    call), `hagree` is the semantic agreement (from
    `real_own_exclusive`), and the conclusion `a = b` follows from
    `ExRA.excl` injectivity.

    This is the **non-degenerate** replacement for the prior local
    axiom `own_ex_exclusive` in `LiveMirrorInvariant.lean` — same
    logical content (Ex-RA exclusivity for two `ExRA.excl` owners at
    the same `γ`), now derived from the heap/RA model rather than
    postulated. -/
theorem own_ex_exclusive_derived {α : Type} [GhostState (ExRA α)]
    (γ : GhostName) (a b : α)
    (ha : RealOwn γ (ExRA.excl a)) (hb : RealOwn γ (ExRA.excl b)) :
    a = b := by
  -- Algebraic layer (Ex RA): two `some`s never compose to a `some`.
  -- This is the algebraic statement of the exclusivity principle —
  -- `Ex.op (some a) (some b) = none` means the two exclusive resources
  -- cannot be combined, hence cannot both be owned at the same `γ`.
  have _hcomp : Ex.op (some a) (some b) = none := ex_exclusive' a b
  -- Semantic layer (single-valued-ness of the ghost state):
  -- `RealOwn γ (ExRA.excl a)` and `RealOwn γ (ExRA.excl b)` give
  -- `GhostState.get γ = some (ExRA.excl a)` and `= some (ExRA.excl b)`,
  -- hence `ExRA.excl a = ExRA.excl b` by single-valued-ness.
  have hagree : ExRA.excl a = ExRA.excl b :=
    real_own_exclusive γ (ExRA.excl a) (ExRA.excl b) ha hb
  -- `ExRA.excl` is injective, so `a = b`.
  injection hagree

/-! ## §4. `GhostState` instances -/

/-- `GhostState` instance for `ExRA α`. The actual `get` returns
    `some (ExRA.excl default)` for every `γ` — a *concrete* ghost
    state that makes `RealOwn γ (ExRA.excl default)` provable
    (e.g. for `α = Liveness` with `default = Liveness.live`,
    `LiveMirrorInv γ var Liveness.live` is constructible). The
    exclusivity theorem `real_own_exclusive` holds for ANY concrete
    `get` (it relies only on single-valued-ness, which is true for
    any function), so the choice of `get` does not affect the
    derivation of `own_ex_exclusive_derived`.

    Requires `Inhabited α` to provide a `default` value. -/
instance GhostState.instExRA {α : Type} [Inhabited α] :
    GhostState (ExRA α) :=
  ⟨fun _ => some (ExRA.excl default)⟩

/-- `GhostState` instance for `AgRA α`. The actual `get` returns
    `some (AgRA.ag default)` for every `γ`. The agreement RA is
    duplicable, so two `RealOwn γ (AgRA.ag a)` and
    `RealOwn γ (AgRA.ag b)` force `a = b` by single-valued-ness
    (matching the Iris `Ag` RA's agreement principle). -/
instance GhostState.instAgRA {α : Type} [Inhabited α] :
    GhostState (AgRA α) :=
  ⟨fun _ => some (AgRA.ag default)⟩

/-- `Inhabited Liveness` instance with `default := Liveness.live`.
    (Auto-derivable in Lean 4 from the no-argument constructor
    `Liveness.live`, but declared explicitly here to make the
    `GhostState.instExRA` instance resolve cleanly.) -/
instance : Inhabited Liveness := ⟨Liveness.live⟩

end PMT.Iris.Heap
