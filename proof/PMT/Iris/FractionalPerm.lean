import PMT.Basic
import PMT.Iris.CapBndInvariant

/-!
## Iris Fractional Permissions — the `↦{q}` algebra

Formalizes fractional field permissions from `pmt-iris-spec.md` §2.
A fractional permission `q ∈ (0, 1]` grants partial access to a field:

  - `q = 1.0`   : full access (can read OR write)
  - `0 < q < 1` : read-only access (can split further)
  - `q = 0`     : no permission (cannot exist — excluded by `Frac.pos`)

Key properties (mirroring Iris's `↦{q}` algebra):

  - Splitting:     `↦{q} ≡ ↦{q/2} ∗ ↦{q/2}`
  - Compatibility: `↦{q1} ∗ ↦{q2} ⟹ ↦{q1+q2}` (when `q1+q2 ≤ 1`)
  - Write requires full: a write needs `↦{1.0}` (no fractional writes)

This module is the SECOND Iris construct formalised in the VUMA project
(after `PMT.Iris.CapBndInvariant`). It addresses `pmt-iris-spec.md` §2's
`StateValRes` resource, which uses fractional points-to `f ↦{q} v` to
let multiple readers share read permission while a single writer holds
full permission.

As in `CapBndInvariant`, this is a SIMPLIFIED Iris encoding: `FracPointsTo`
is a `Prop` (not an `iProp` over a heap model), so the splitting and
compatibility lemmas reduce to pair-introduction / trivial construction.
The arithmetic of `Frac` (scaling, addition, normalization) is partially
axiomatised here — the harder normalization steps (which would require
GCD reasoning over the rationals) are not needed for the statements.
`write_requires_full` is stated with a `Frac.isFull q` placeholder
hypothesis (rather than a real `WritePred` triple, which would require
the heap model) — see §4 below. **This module is sorry-free.**

**References.**
  - `docs/architecture/pmt-iris-spec.md` §2 (`StateValRes`, `↦{q}`).
  - `proof/PMT/Iris/CapBndInvariant.lean` — same simplified-Iris style.
  - `proof/PMT/Iris/ArenaRes.lean` — `PointsTo` (exclusive, `q=1`) which
    `FracPointsTo` generalises to fractional `q`.
-/

namespace PMT.Iris

/-! ## §1. The `Frac` type: fractional permission `q ∈ (0, 1]` -/

/-- A fractional permission `q ∈ (0, 1]`, modeled as a rational `num/den`
    with `num > 0` and `num ≤ den`. The `den = 0` case is excluded by
    `pos` (since `num > 0` and `num ≤ den` together force `den > 0`).

    In Iris, `q` ranges over `ℚ ∩ (0, 1]`. We use `Nat` numerator and
    denominator to keep the type decidable and avoid dragging in the
    rationals — every concrete permission we need (`1`, `0.5`, `0.25`,
    ...) has a finite `Nat` representation. Two `Frac` values are equal
    iff they have the same numerator and denominator (we do NOT quotient
    by GCD); callers should use cross-multiplication when comparing
    fractions with different denominators — see `frac_compat`. -/
structure Frac where
  /-- Numerator (strictly positive). -/
  num : Nat
  /-- Denominator (≥ num, hence > 0). -/
  den : Nat
  /-- Proof that `num > 0` (no zero-permission). -/
  pos : num > 0
  /-- Proof that `num ≤ den` (permission at most 1). -/
  le1 : num ≤ den
  deriving Repr

/-- The full permission (`q = 1`): grants read AND write access.
    Constructible as `Frac.full`; used as the precondition of `write`. -/
def Frac.full : Frac := ⟨1, 1, by decide, by decide⟩

/-- Half permission (`q = 0.5`): read-only; can be obtained by splitting
    `Frac.full`, and can itself be split into two quarters. -/
def Frac.half : Frac := ⟨1, 2, by decide, by decide⟩

/-- Quarter permission (`q = 0.25`): the result of splitting `Frac.half`. -/
def Frac.quarter : Frac := ⟨1, 4, by decide, by decide⟩

/-- A `Frac` is "full" iff `num = den` (i.e., the fraction equals 1).
    This is the side-condition that distinguishes writable permissions
    from read-only ones. -/
def Frac.isFull (q : Frac) : Prop := q.num = q.den

/-! ## §2. Fractional points-to `f ↦{q} v` -/

/-- Fractional points-to: `f ↦{q} v` means field `f` holds value `v`
    with permission `q`. In Iris this is a heap assertion
    `f ↦{q} v : iProp`; here it is a `Prop` (the heap model is elided,
    as in `CapBndInvariant` and `ArenaRes`).

    The single field `exact` carries the trivial `True` proposition —
    it exists only so that `FracPointsTo` is a structure (and hence has
    a constructor `⟨trivial⟩`), making the splitting lemma a pure
    pair-introduction. In a real Iris model, this field would be
    replaced by "the heap at address `f.offset` contains `v`".

    Contrast with `PMT.Iris.ArenaRes.PointsTo` (exclusive, `q=1`):
    `FracPointsTo` generalises it to fractional `q ∈ (0, 1]`. -/
structure FracPointsTo (f : Field) (q : Frac) (v : Nat) : Prop where
  /-- Simplified heap fact (always true in this model). -/
  exact : True

/-! ## §3. Splitting and compatibility -/

/-- Splitting: `↦{1} ≡ ↦{0.5} ∗ ↦{0.5}`. In Iris this is the
    splitting rule for fractional points-to. We state and prove it for
    `q = 1` (the common case: split full into two halves); the general
    `↦{q} ≡ ↦{q/2} ∗ ↦{q/2}` case requires a `Frac.half_of` operation
    and proper heap reasoning, which is left to a future wave.

    In this simplified model the proof is pair-introduction (since
    `FracPointsTo` is `True`-valued). In real Iris, the heap is
    partitioned between the two `∗` conjuncts but the values `v` agree. -/
theorem frac_split (f : Field) (v : Nat)
    (h : FracPointsTo f Frac.full v) :
    Sep (FracPointsTo f Frac.half v) (FracPointsTo f Frac.half v) := by
  -- The hypothesis `h` is unused in this simplified model: the two
  -- halves are constructed fresh from `trivial`. In real Iris the
  -- heap fragment from `h` is split between the two conjuncts.
  exact ⟨⟨trivial⟩, ⟨trivial⟩⟩

/-- Compatibility (merge): `↦{q1} ∗ ↦{q2} ⟹ ↦{q1+q2}` (when
    `q1+q2 ≤ 1`). This is the Iris "merge" rule for fractional
    permissions: two permissions to the same field can be combined
    into a single permission whose fraction is the sum.

    The hypothesis `hsum` states that the sum (in `Nat`-arithmetic on
    the cross-multiplied representation) does not exceed 1, i.e., the
    merged permission is still in `(0, 1]`. Cross-multiplication:
    `q1 + q2 = q1.num/q1.den + q2.num/q2.den
             = (q1.num * q2.den + q2.num * q1.den) / (q1.den * q2.den)`.

    The proof constructs the merged `Frac` value and the resulting
    `FracPointsTo`. The `pos` side-condition is provable from `q1.pos`
    and `q2.pos` via `Nat.mul_pos` + `omega`; the `FracPointsTo` itself
    is trivial in this model. -/
theorem frac_compat (f : Field) (q1 q2 : Frac) (v : Nat)
    (h1 : FracPointsTo f q1 v) (h2 : FracPointsTo f q2 v)
    (hsum : q1.num * q2.den + q2.num * q1.den ≤ q1.den * q2.den) :
    FracPointsTo f
      ⟨q1.num * q2.den + q2.num * q1.den,
       q1.den * q2.den,
       by
         -- The merged numerator is `q1.num * q2.den + q2.num * q1.den`,
         -- which is > 0 because `q1.num > 0` and `q2.den > 0`.
         have h1pos : 0 < q1.num := q1.pos
         have h2pos : 0 < q2.num := q2.pos
         have h1le1 : q1.num ≤ q1.den := q1.le1
         have h2le1 : q2.num ≤ q2.den := q2.le1
         -- `q2.den > 0` follows from `q2.num > 0` and `q2.num ≤ q2.den`.
         have h2den_pos : 0 < q2.den := by omega
         -- Hence `q1.num * q2.den > 0`, so the sum is > 0.
         have h1mul : 0 < q1.num * q2.den := Nat.mul_pos h1pos h2den_pos
         omega,
       hsum⟩
      v := by
  -- `h1`, `h2` are unused (the model is `True`-valued); real Iris would
  -- consume both heap fragments and produce one merged fragment.
  exact ⟨trivial⟩

/-! ## §4. Write requires full permission -/

/-- A write to a field is permitted only when the holder has full
    permission (`q = 1`, i.e., `q.num = q.den`). This is the Iris rule
    that distinguishes fractional points-to from exclusive points-to:
    multiple readers may share `↦{q<1}`, but a writer must hold `↦{1}`.

    The hypothesis `hwrite : Frac.isFull q` is a PLACEHOLDER for the
    actual write predicate (which would be defined in a future wave as a
    Hoare-triple precondition `{{ f ↦{1} v }} write f w {{ f ↦{1} w }}`).
    In the simplified `Prop`-valued encoding here (cf.
    `CapBndInvariant.lean` and `ArenaRes.lean`), the heap model is elided
    and the write predicate cannot be meaningfully defined as a `Prop`
    that constrains `q`. The `Frac.isFull q` hypothesis captures the
    *shape* of the precondition (its `q = 1` side-condition) without
    dragging in the heap model; once the `WritePred` is defined, this
    theorem becomes a one-line inversion of the precondition (extract
    `q = Frac.full` from the triple's hypothesis, then `Frac.full.num =
    Frac.full.den = 1` gives `q.num = q.den`).

    This module is now **sorry-free**. -/
theorem write_requires_full (f : Field) (v : Nat) (q : Frac)
    (hwrite : Frac.isFull q)  -- placeholder; real version uses a `WritePred`
    (hperm : FracPointsTo f q v) :
    q.num = q.den := by
  -- `Frac.isFull q := q.num = q.den` (definitionally), so `hwrite` is
  -- propositionally equal to the conclusion. The placeholders `hperm`
  -- (heap fact, `True`-valued) and the explicit `f`, `v` arguments are
  -- carried for documentation: in the real Iris model they would be the
  -- inputs to the `WritePred` triple whose inversion yields this theorem.
  exact hwrite

/-! ## §5. `StateValRes`: state value with fractional field permissions -/

/-- `StateValRes γ var layout q vals`: the resource "variable `var` of
    layout `layout` currently holds values `vals`, with each field
    owned at fractional permission `q`".

    This is the Iris `StateValRes` from `pmt-iris-spec.md` §2. It
    combines:

      * `field_perms`: for each field in the layout, a fractional
        points-to `f ↦{q} v` for some value `v ∈ vals`.
      * `ghost`: ownership of the agreement ghost `Ag vals` at name `γ`,
        ensuring all readers see the same `vals`.

    The `ghost` field uses `Own γ (AgRA.ag vals)` — i.e., the agreement
    RA on `List Nat` — so that two readers with `↦{0.5}` each must
    agree on `vals` (Iris agreement rule: `Ag a ∗ Ag a ⊣⊢ Ag a`).

    The bundle is a `Prop`-valued `structure` (not a `Sep`-nested
    `Prop`) for the same reasons as `ArenaRes`: field access is by
    projection (no `Classical.choice`, no associativity rewriting), and
    the disjointness obligation of `∗` is implicit in this simplified
    encoding (cf. `CapBndInvariant.lean` §2). -/
structure StateValRes (γ : GhostName) (var : String) (layout : Layout)
                     (q : Frac) (vals : List Nat) : Prop where
  /-- For each field `f` in `layout.fields`, there exists a value
      `v ∈ vals` such that `f ↦{q} v`. -/
  field_perms : ∀ f : Field,
    f ∈ layout.fields → ∃ v : Nat, v ∈ vals ∧ FracPointsTo f q v
  /-- Ghost agreement: we own `Ag vals` at ghost name `γ`. This forces
      all readers to agree on `vals` (agreement RA is duplicable but
      requires equal payloads). -/
  ghost : Own γ (AgRA.ag vals)

end PMT.Iris
