import PMT.Basic

/-!
# PMT Field — §3 Field-Bounds & §4 Linearity Invariants (sorry-free)

This module contains the pure-arithmetic and resource-algebra
invariants of the PMT (Programs as Memory Transformations) memory
model used by the VUMA compiler:

  * §3 — `FieldBounds`, `access_safe`, `wf_layout_implies_field_bounds`
    (mirrors `pmt-iris-spec.md` §3 / `pmt-formal-spec.md` §3).
  * §4 — `Liveness`, `LinearToken`, `LinearResource`, `Consumed`,
    `Accessible`, `live_ne_dead`, `no_uaf`
    (mirrors `pmt-iris-spec.md` §4 / `pmt-formal-spec.md` §4).

This file depends on the data model (`Arena`, `Layout`, `Field`,
`WF_Layout`, …) defined in `PMT.Basic`. It uses only Lean 4 core
(no Iris import — the affine fragment is encoded with explicit
resource tokens). All theorems close without `sorry`.

Note: the ghost-state liveness content (`state_read_requires_live`,
`state_transform`, `state_transform_kills_input`) lives in
`PMT.Liveness` (§5), as documented in `lakefile.toml`.

**Module dependency.** `PMT.Field` is depended on by `PMT.Liveness`
(which transitively re-exports the linearity primitives to
`PMT.Soundness`) and by `PMT.WellTypedStrong` (which uses
`FieldBounds` for the strengthened static field-access check).

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job runs the same command. The
legacy single-file `lean PMT/Field.lean` invocation does not work
since the multi-module split.
-/

namespace PMT

/-! ## §3. Field-Bounds Invariant -/

/-- §3: `FieldBounds l f := f.offset + f.size ≤ l.total_size`.
Mirrors the Iris pure fact `⌜f.off + f.size ≤ L_T.total_size⌝` embedded
in `StateValRes` (`pmt-iris-spec.md` §2). -/
def FieldBounds (l : Layout) (f : Field) : Prop :=
  f.offset + f.size ≤ l.total_size

/-- §3: A field access is safe iff its byte range `[offset, offset+size)`
fits inside the layout. Combined with the StateVal invariant
`p.ofs + L_T.total_size ≤ A.used` (§1.4 well-typedness), this gives the
field-bounds safety theorem of `pmt-formal-spec.md` §3:
`touched bytes lie inside [base+ofs, base+ofs+size) ⊆ arena`.

The lemma is essentially definitional — `FieldBounds` *is* the bound —
but stating it explicitly makes the soundness composition in §7
cleaner. -/
theorem access_safe
    (l : Layout) (f : Field)
    (_hwf : WF_Layout l)
    (hfb  : FieldBounds l f) :
    f.offset + f.size ≤ l.total_size := by
  -- `FieldBounds l f` unfolds definitionally to the goal.
  exact hfb

/-- §3 corollary: every field registered in a well-formed layout
satisfies `FieldBounds`. This is the left conjunct of `WF_Layout`. -/
theorem wf_layout_implies_field_bounds
    (l : Layout) (f : Field)
    (hwf : WF_Layout l)
    (hmem : f ∈ l.fields) :
    FieldBounds l f := by
  -- `WF_Layout`'s first conjunct: ∀ f ∈ fields, f.offset + f.size ≤ total_size.
  exact hwf.1 f hmem

/-! ## §4. Linearity Invariant — State Consumption as Resource Transfer -/

/-- §4: Liveness status of a state vreg. -/
inductive Liveness where
  | live : Liveness
  | dead : Liveness
  deriving Repr

instance : DecidableEq Liveness
  | Liveness.live, Liveness.live => isTrue rfl
  | Liveness.live, Liveness.dead => isFalse (by intro h; cases h)
  | Liveness.dead, Liveness.live => isFalse (by intro h; cases h)
  | Liveness.dead, Liveness.dead => isTrue rfl

/-- §4: A *linear resource token* for a state variable. We model Iris's
exclusive monoid `own(γ_state v, Ex (Some p))` as a structure: the
`status` field records whether the token is `live` (held) or `dead`
(consumed). Because `consume` flips `live → dead` and the Iris
`Ex`-monoid excludes two `live` tokens for the same `γ`, a consumed
variable cannot be accessed again. -/
structure LinearToken where
  var    : String
  status : Liveness

/-- §4: `LinearResource t` — the variable currently holds an exclusive
ownership token (status `live`). -/
def LinearResource (t : LinearToken) : Prop := t.status = Liveness.live

/-- §4: `Consumed t` — the variable has been consumed by a
`StateTransform`; its token now reads `dead`. -/
def Consumed (t : LinearToken) : Prop := t.status = Liveness.dead

/-- §4: `Accessible t` — the variable may be read/written; requires
`LinearResource` (i.e. `live`). -/
def Accessible (t : LinearToken) : Prop := LinearResource t

/-- Helper: `live` and `dead` are distinct constructors. -/
theorem live_ne_dead : Liveness.live ≠ Liveness.dead := by
  intro h; cases h

/-- §4 lemma (no UAF): after a `StateTransform` consumes `var`, no
access is possible. This is the Lean rendering of the Iris corollary
in `pmt-iris-spec.md` §4:

    Corollary (no UAF). After StateTransform, any access to v_in
    requires own(γ_state v_in, Ex (Some _)), which no longer holds —
    the access cannot be proven.

Proof: by contradiction. `LinearResource t` gives `t.status = live`;
`Consumed t` gives `t.status = dead`; `live ≠ dead` so the hypotheses
are inconsistent, and from `False` anything follows (including
`¬ Accessible t`). -/
theorem no_uaf
    (t : LinearToken)
    (_hlin : LinearResource t)
    (hcon : Consumed t) :
    ¬ Accessible t := by
  intro hacc
  -- `Accessible t` is definitionally `LinearResource t` is
  -- definitionally `t.status = Liveness.live`. Same for `Consumed`.
  have h1 : t.status = Liveness.live := hacc
  have h2 : t.status = Liveness.dead := hcon
  -- `h1.symm.trans h2 : Liveness.live = Liveness.dead` — impossible.
  exact live_ne_dead (h1.symm.trans h2)

end PMT
