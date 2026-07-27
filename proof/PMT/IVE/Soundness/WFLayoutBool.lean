import PMT.Basic

/-!
## IVE Soundness — computable `WF_Layout`

This module provides a computable, `Bool`-valued reformulation of the
`WF_Layout` predicate (defined in `PMT.Basic`). The original
`WF_Layout` is a `Prop` with universal quantifiers over `Field`, so
its `Decidable` instance can only be obtained via `Classical.propDecidable`
— which is nonconstructive and therefore cannot survive extraction to
Rust. This file defines `wf_layout_bool`, a `Bool`-valued twin that
mirrors `WF_Layout`'s three conjuncts using `List.all` and `decide`,
together with a sorry-free proof of equivalence:

    theorem wf_layout_bool_iff_wf_layout (l : Layout) :
        wf_layout_bool l ↔ WF_Layout l

With this in hand, `PMT.IVE.Soundness.Transform.verify_transform`
becomes a plain `def` (no `noncomputable`), and the soundness theorems
bridge from the Bool acceptance check back to the `WF_Layout` Prop
through `wf_layout_bool_iff_wf_layout`. This unblocks extraction
(IVE-1-B) and the Iris soundness layer (PMT-1-G).

**Codedomain.** IVE-owned — this file lives under
`PMT/IVE/Soundness/`, so it does not modify `PMT.Basic` or `PMT.Field`
(PMT-owned codedomain). To make `f₁ = f₂` decidable inside the pair
disjointness check, we install `DecidableEq PMT.Field` and
`Decidable (PMT.Disjoint f₁ f₂)` instances here rather than in
`PMT.Basic`. No prior instance of `DecidableEq PMT.Field` exists in the
proof library, so this introduces no conflicts.
-/

namespace PMT.IVE.Soundness

/-! ## Decidable instances for `Field` equality and `Disjoint`

`PMT.Basic` defines `Field` as `structure Field where offset : Nat; size : Nat`
with only `deriving Repr` (no `DecidableEq`). It also defines
`Disjoint f₁ f₂ := f₁.offset + f₁.size ≤ f₂.offset ∨
f₂.offset + f₂.size ≤ f₁.offset` as a `Prop`, with no accompanying
`Decidable` instance. Both are needed inside `wf_layout_bool`'s
pair-disjointness check (`decide (f₁ = f₂ ∨ Disjoint f₁ f₂)`); we
install them here, in the IVE-codedomain, by composing `Nat`'s
decidable comparisons over `Field`'s two projections. The
auto-generated `Field.mk.inj` lemma witnesses the injectivity
direction; the positive direction is just `rw` over the projection
equalities. No `sorry`. -/

instance : DecidableEq PMT.Field := fun f₁ f₂ =>
  match f₁, f₂ with
  | ⟨o₁, s₁⟩, ⟨o₂, s₂⟩ =>
    if h : o₁ = o₂ ∧ s₁ = s₂ then
      isTrue (by
        obtain ⟨ho, hs⟩ := h
        rw [ho, hs])
    else
      isFalse (by
        intro heq
        apply h
        exact PMT.Field.mk.inj heq)

/-- `DecidableEq PMT.Layout` is needed by `Transform.verify_transform_spec`
(for the `identity` arm's `t.in_layout = t.out_layout` check) so that
`decide (verify_transform_spec t)` reduces constructively without
`Classical.propDecidable`. Built by composing `DecidableEq Nat` and
`DecidableEq (List Field)` (the latter auto-derived from
`DecidableEq Field` above). -/
instance : DecidableEq PMT.Layout := fun l₁ l₂ =>
  match l₁, l₂ with
  | ⟨ts₁, fs₁⟩, ⟨ts₂, fs₂⟩ =>
    if h : ts₁ = ts₂ ∧ fs₁ = fs₂ then
      isTrue (by
        obtain ⟨hts, hfs⟩ := h
        rw [hts, hfs])
    else
      isFalse (by
        intro heq
        apply h
        exact PMT.Layout.mk.inj heq)

/-- `Disjoint f₁ f₂` is decidable because it is a disjunction of two
`Nat` inequalities. We expose this via `inferInstanceAs` so that
`decide (Disjoint _ _)` reduces without unfolding `Disjoint`. -/
instance (f₁ f₂ : PMT.Field) : Decidable (PMT.Disjoint f₁ f₂) :=
  inferInstanceAs (Decidable
    (f₁.offset + f₁.size ≤ f₂.offset ∨ f₂.offset + f₂.size ≤ f₁.offset))

/-! ## `wf_layout_bool` — computable mirror of `WF_Layout` -/

/-- `wf_layout_bool l` is the computable, `Bool`-valued twin of
`WF_Layout l`. It mirrors the three conjuncts of `WF_Layout` exactly:

  (1) every `f ∈ l.fields` satisfies `f.offset + f.size ≤ l.total_size`,
  (2) every pair `f₁, f₂ ∈ l.fields` satisfies `f₁ = f₂ ∨ Disjoint f₁ f₂`
      — constructively equivalent to `f₁ ≠ f₂ → Disjoint f₁ f₂` once
      `DecidableEq Field` is in scope, and the iff proof below bridges
      the two constructively (case-splitting on `decide (f₁ = f₂)`),
  (3) `0 < l.total_size ∨ l.fields = []`.

Implementation notes:
  - Conjunct (1) is a single `List.all` over `l.fields`.
  - Conjunct (2) is a nested `List.all` over the cartesian product
    `l.fields × l.fields`, with the per-pair check `decide (f₁ = f₂ ∨
    Disjoint f₁ f₂)`. The `DecidableEq Field` and
    `Decidable (Disjoint _ _)` instances above make `decide` reduce.
  - Conjunct (3) is a single `decide` — `Nat.lt` and `List.eq_nil` are
    both decidable. -/
def wf_layout_bool (l : PMT.Layout) : Bool :=
  -- (1) every field is in-bounds.
  (l.fields.all (fun f => decide (f.offset + f.size ≤ l.total_size)))
  -- (2) every distinct pair is disjoint (covers same-field vacuously).
  && (l.fields.all (fun f₁ => l.fields.all (fun f₂ =>
        decide (f₁ = f₂ ∨ PMT.Disjoint f₁ f₂))))
  -- (3) total_size > 0 or fields is empty.
  && decide (0 < l.total_size ∨ l.fields = [])

/-- `wf_layout_bool l ↔ WF_Layout l` — sorry-free iff proof.

The proof first unfolds `wf_layout_bool` and `WF_Layout`, then rewrites
the `Bool &&` chain into nested `And` (via `Bool.and_eq_true_iff`),
each `List.all p l = true` into `∀ x ∈ l, p x = true` (via
`List.all_eq_true`), and each `decide p = true` into `p` (via
`decide_eq_true_iff`). The resulting goal is an `And` of three
conjuncts whose two sides differ only in the pair-disjointness
conjunct: the Bool side says `f₁ = f₂ ∨ Disjoint f₁ f₂`, the Prop
side says `f₁ ≠ f₂ → Disjoint f₁ f₂`. The two are bridged
constructively by case-splitting on `decide (f₁ = f₂)` (legitimate
since `DecidableEq Field` is in scope): if `f₁ = f₂` then `f₁ ≠ f₂`
is contradictory; otherwise `Disjoint f₁ f₂` is recovered directly.
 -/
theorem wf_layout_bool_iff_wf_layout (l : PMT.Layout) :
    wf_layout_bool l ↔ PMT.WF_Layout l := by
  unfold wf_layout_bool PMT.WF_Layout
  simp only [Bool.and_eq_true_iff, List.all_eq_true, decide_eq_true_iff]
  -- After simp the iff goal has shape `((A ∧ B) ∧ C) ↔ (A ∧ (B' ∧ C))`,
  -- where the LHS's `(A ∧ B) ∧ C` reflects `wf_layout_bool`'s
  -- left-associated `&&` chain and the RHS's `A ∧ (B' ∧ C)` reflects
  -- `WF_Layout`'s right-associated `∧` chain. We bridge the two with
  -- the appropriate constructor shapes.
  refine ⟨fun h => ?_, fun h => ?_⟩
  · -- forward direction: `wf_layout_bool` → `WF_Layout`.
    -- `h : (A ∧ B) ∧ C` (left-assoc).
    obtain ⟨⟨h1, h2⟩, h3⟩ := h
    -- Goal: `A ∧ (B' ∧ C)` (right-assoc).
    refine ⟨?_, ?_, ?_⟩
    · -- Conjunct 1: identical structure (`∀ f, f ∈ l.fields →
      -- f.offset + f.size ≤ l.total_size`).
      exact fun f hf => h1 f hf
    · -- Conjunct 2: bridge `f₁ = f₂ ∨ Disjoint f₁ f₂` to
      -- `f₁ ≠ f₂ → Disjoint f₁ f₂`.
      intro f₁ f₂ hf₁ hf₂ hne
      have hpair := h2 f₁ hf₁ f₂ hf₂
      -- `hpair : f₁ = f₂ ∨ Disjoint f₁ f₂`; `hne : f₁ ≠ f₂`.
      rcases hpair with heq | hdis
      · exact absurd heq hne
      · exact hdis
    · -- Conjunct 3: identical structure.
      exact h3
  · -- backward direction: `WF_Layout` → `wf_layout_bool`.
    -- `h : A ∧ (B' ∧ C)` (right-assoc).
    obtain ⟨h1, h2, h3⟩ := h
    -- Goal: `(A ∧ B) ∧ C` (left-assoc).
    refine ⟨⟨?_, ?_⟩, ?_⟩
    · -- Conjunct 1: identical structure.
      exact fun f hf => h1 f hf
    · -- Conjunct 2: bridge `f₁ ≠ f₂ → Disjoint f₁ f₂` to
      -- `f₁ = f₂ ∨ Disjoint f₁ f₂` via `DecidableEq Field`.
      intro f₁ hf₁ f₂ hf₂
      by_cases heq : f₁ = f₂
      · exact Or.inl heq
      · exact Or.inr (h2 f₁ f₂ hf₁ hf₂ heq)
    · -- Conjunct 3: identical structure.
      exact h3

end PMT.IVE.Soundness
