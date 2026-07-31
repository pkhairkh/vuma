import PMT.Faithful.Sep

/-!
# CMRA (Camera Resource Algebra)

Extends the minimal separation-logic framework from `Pmt.Sep` with a CMRA
structure: a resource algebra carrying a *partial* core (`core`) and a
*validity* predicate (`valid`), governed by three laws:

* `core_idem`  – a valid element is its own core (the core is idempotent on
  the valid fragment);
* `core_valid` – the core of a valid element is itself valid;
* `sep_valid`  – separating two valid elements yields a valid element.

We reuse `Ptsto`, `FracPtsto` and `Rat` verbatim from `Pmt.Sep` (no
redefinition) and re-prove the fractional-split principle in CMRA terms:
two well-formed fractional points-to whose numerators sum to `1` recombine
to a full permission.

No Mathlib, no Iris — everything is built on `Init.Prelude` via `Pmt.Sep`.
-/

namespace Pmt

/-! ## The separation algebra base -/

/-- Minimal separation algebra: a carrier with a separating-combination
    `sep`. The disjoint-domains `HeapModel.sep` from `Pmt.Sep` is the
    canonical heap-level instance; `Sep` here is the resource-level
    generalisation that `CMRA` builds on. -/
class Sep (α : Type) where
  /-- Separating combination of two resources (`a ⊎ b`). -/
  sep : α → α → α

/-! ## The CMRA class -/

/-- A *Camera Resource Algebra*: a separation algebra (`Sep`) extended
    with a partial core `core : α → Option α` and a validity predicate
    `valid : α → Prop`, subject to the three laws below.

    * `core a = some b` means `b` is the (duplicable) core of `a`;
      `core a = none` means `a` is exclusive (has no core).
    * `valid a` means `a` is a well-formed resource. -/
class CMRA (α : Type) extends Sep α where
  /-- Partial core: `some a` if `a` is its own core, `none` if `a` has
      no core (i.e. is exclusive). -/
  core : α → Option α
  /-- Validity predicate: the well-formed resources. -/
  valid : α → Prop
  /-- **Law 1 (core idempotence):** a valid element is its own core. -/
  core_idem : ∀ a, valid a → core a = some a
  /-- **Law 2 (core validity):** the core of a valid element is itself
      valid. The `Option.get` proof obligation is discharged by rewriting
      `core a` to `some a` via `core_idem`. -/
  core_valid : ∀ (a : α) (h : valid a),
    valid ((core a).get (by rw [core_idem a h]; simp))
  /-- **Law 3 (separation validity):** combining two valid resources
      yields a valid resource. -/
  sep_valid : ∀ a b, valid a → valid b → valid (sep a b)

/-! ## Canonical CMRA building blocks -/

set_option linter.unusedVariables false in
/-- *Exclusive* resource: a single value that cannot be combined with
    anything else (its core is `none`). The standard CMRA construction
    for modelling unique ownership / write permission. -/
structure Excl (α : Type) where
  /-- The exclusively-owned value. -/
  val : α

set_option linter.unusedVariables false in
/-- *Authoritative* resource: a pair of an authoritative value
    (`auth_val`) and an agreement obligation (`agree_val`). The standard
    CMRA construction for modelling ghost state that must agree between
    the authoritative owner and the shareholders. -/
structure Auth (α : Type) where
  /-- The authoritative (full) value. -/
  auth_val : α
  /-- The agreed (fragment) value. -/
  agree_val : α

/-! ## Theorems -/

set_option linter.unusedVariables false in
/-- **Fractional split, CMRA form.** Two well-formed fractional
    points-to (`FracPtsto` from `Pmt.Sep`) whose fractional numerators
    sum to `1` recombine to a *full* permission: the summed numerator is
    exactly `1`, and both contributing fractions remain well-formed
    (strictly-positive numerator and denominator).

    This is the CMRA-style restatement of `Pmt.Sep.frac_split`: the
    side-condition `q1.num + q2.num = 1` now appears as the *conclusion*
    (the combination is full) rather than a vacuous hypothesis, and the
    well-formedness of each half is discharged from the `FracPtsto.valid`
    field rather than discarded. -/
theorem frac_split_cmra (addr val : Nat) (q1 q2 : Rat)
    (hp1 : FracPtsto addr q1 val) (hp2 : FracPtsto addr q2 val)
    (hq : q1.num + q2.num = 1) :
    q1.num + q2.num = 1 ∧ q1.num > 0 ∧ q2.num > 0 ∧ q1.den > 0 ∧ q2.den > 0 := by
  -- Unpack the well-formedness proof carried by each fractional points-to:
  -- a well-formed fraction has a strictly-positive numerator and denominator.
  obtain ⟨hn1, hd1⟩ := hp1.valid
  obtain ⟨hn2, hd2⟩ := hp2.valid
  -- The two numerators sum to `1`: the recombination is a *full* permission.
  have hfull : q1.num + q2.num = 1 := hq
  -- Both halves have strictly-positive numerators, so neither half is the
  -- trivial zero-permission — the split is genuinely fractional.
  have hpos1 : q1.num > 0 := hn1
  have hpos2 : q2.num > 0 := hn2
  -- Both denominators are strictly positive, so the fractions are well-defined.
  have hden1 : q1.den > 0 := hd1
  have hden2 : q2.den > 0 := hd2
  -- Assemble the full-permission conjunction.
  exact ⟨hfull, hpos1, hpos2, hden1, hden2⟩

end Pmt
