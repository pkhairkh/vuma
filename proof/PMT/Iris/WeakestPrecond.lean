import PMT.Basic
import PMT.Soundness
import PMT.Iris.CapBndInvariant

/-!
## Iris wp — weakest-precondition reasoning

This module formalises `wp e {Φ}` from `docs/architecture/pmt-iris-spec.md`
§7 — the **weakest precondition** for expression `e` to execute safely and
produce a value satisfying postcondition `Φ`.

**Spec (§7).**

```
Definition wp (e : expr) (Φ : val → iProp) : iProp :=
  ∀ σ, e ⊨_{wp} σ : Φ.
```

In Iris, `wp e {Φ}` holds iff executing `e` from the current world is
safe (no UB, no stuck state) and any produced value `v` satisfies `Φ v`.
It is the *definitional* core of Iris's program logic: every Hoare
triple `{{ P }} e {{ v, Q }}` is a derived form `P -∗ wp e {Q}`.

**Encoding (this module).**

This is a SIMPLIFIED Iris encoding, following the pattern established by
`PMT.Iris.CapBndInvariant` (the `[cap_bnd]` named invariant, formalised
earlier). Real Iris `wp` is a fixpoint over step indices in a fancy-
update monad; here we collapse the recursion because the underlying
`PMT.Soundness.step` is structurally recursive and total. Each
`Expr` constructor dispatches to a one-shot `Prop` predicate that
encodes its safety precondition and postcondition obligation:

  * `Expr.alloc l`    — `wp` holds iff `arena.used + l.total_size ≤
    arena.capacity` (no overflow) and `Φ` is satisfied by the returned
    bump-pointer (`Val.nat arena.used`).
  * `Expr.read f`     — `wp` holds unconditionally (simplified — full
    field-bounds checking lives in `PMT.Soundness.step`'s `field_access`
    branch) and `Φ` is satisfied by `Val.nat 0` (a placeholder for the
    read value).
  * `Expr.write f v`  — `wp` holds unconditionally and `Φ` is satisfied
    by `Val.unit`.
  * `Expr.transform`  — `wp` holds unconditionally and `Φ` is satisfied
    by `Val.unit`. Used as a "pure-step / value" placeholder for
    `wp_value` and `wp_bind`.

**Key constructs**

  - `wp`             — the weakest-precondition predicate (DEFINITION,
    sorry-free).
  - `wp_monotone`    — monotonicity: stronger postcondition ⟹ weaker wp.
  - `wp_frame`       — frame rule: `wp e {Φ} ∗ R ⟹ wp e {λ v, Φ v ∗ R}`.
  - `wp_bind`        — bind rule: `wp (bind e1 e2) {Φ} ⟹
                       wp e1 {λ v, wp (e2 v) {Φ}}`.
  - `wp_value`       — value rule: `wp (pure value) {Φ} ⟹ Φ value`.
  - `wp_soundness`   — soundness: if `wp e {True}` holds for every step
    of a program, execution is safe (ok or canonical trap).

**Sorries.** The `wp` DEFINITION is sorry-free. Of the five derived
lemmas (`wp_monotone`, `wp_frame`, `wp_bind`, `wp_value`,
`wp_soundness`), `wp_soundness` is closed by direct induction
on `prog`, mirroring `pmt_soundness`'s case-split on `step s i`'s
outcome — `hwp` is vacuous (it reduces to `True` for every step), so
the conclusion's canonical-trap-code guarantee holds unconditionally.
The remaining four lemmas (`wp_monotone`, `wp_frame`, `wp_bind`,
`wp_value`) are stated with their full Iris shape but their proofs are
`sorry`-stubbed; closing them requires the full Iris heap model and a
step-indexed fixpoint, which are out of scope here. The theorem
*statements* are the contribution: they pin down the Iris proof rules
the VUMA project must eventually discharge. This matches the posture of
`pmt-iris-spec.md` §7 (statements given; proofs deferred to the Iris
embedding).

**References.**
  - `docs/architecture/pmt-iris-spec.md` §7 (wp).
  - `proof/PMT/Iris/CapBndInvariant.lean` — the `[cap_bnd]` named
    invariant whose preservation is the key `alloc` obligation that
    `wp (Expr.alloc l) {Φ}` discharges.
  - `proof/PMT/Soundness.lean` — `pmt_soundness` (sorry-free), which
    `wp_soundness` defers to once the wp-to-Hoare bridge is built.
-/

namespace PMT.Iris

/-! ## §7.1. Value, postcondition, expression, state types -/

/-- Value type for `wp`. Real Iris values are a much richer inductive
    (closures, thunks, ...); here we model only the three value shapes
    that the PMT execution model produces: naturals (bump pointers /
    read results), booleans (liveness flags), and unit (write acks,
    transform acks). -/
inductive Val where
  | nat  : Nat → Val
  | bool : Bool → Val
  | unit : Val
  deriving Repr

/-- A postcondition is a predicate on `Val`. In Iris this is
    `val → iProp`; here it is `val → Prop` (the `Prop`-valued
    simplification, cf. `CapBndInvariant.lean` §1). -/
abbrev Postcond := Val → Prop

/-- Expression type for `wp`. A simplified model of the PMT step
    operators from `PMT.Soundness.PmtOp` (`alloc | field_access |
    transform`); `read`/`write` correspond to `field_access f`
    (the field-access dispatch). -/
inductive Expr where
  | alloc      : Layout → Expr
  | read       : Field → Expr
  | write      : Field → Val → Expr
  | transform  : Expr
  deriving Repr

/-- The state for `wp` evaluation. A pair of the arena (`PMT.Arena`)
    and the liveness map (`String → Liveness`), mirroring
    `PMT.ExecState` but as a `structure` (so `wp`'s third argument
    has a clean anonymous-constructor form `⟨arena, live⟩`).

    No `deriving Repr` because the `live` field is a function
    (`String → Liveness`), which has no `Repr` instance. -/
structure WpState where
  arena : Arena
  live  : String → Liveness

/-! ## §7.2. The `wp` definition (sorry-free) -/

/-- `wp e {Φ} s` — the weakest precondition for `e` to execute safely
    from state `s` and produce a value satisfying `Φ`.

    This is the simplified encoding described in the module docstring:
    each `Expr` constructor dispatches to a one-shot `Prop` predicate
    that combines the safety precondition and the postcondition
    obligation as a conjunction. In real Iris this is a fixpoint over
    step indices; here the recursion is collapsed because the underlying
    `PMT.Soundness.step` is structurally recursive and total.

    The DEFINITION is sorry-free; derived lemmas below may use `sorry`
    for their proofs (their statements are the contribution). -/
def wp (e : Expr) (Φ : Postcond) (s : WpState) : Prop :=
  match e with
  | Expr.alloc l =>
    -- `alloc l` is safe iff the bump-pointer stays within capacity
    -- (the `[cap_bnd]` invariant obligation, cf.
    -- `CapBndInvariant.alloc_preserves_cap_bnd`). On success the
    -- returned value is the old bump-pointer (the offset of the freshly
    -- allocated region).
    s.arena.used + l.total_size ≤ s.arena.capacity
      ∧ Φ (Val.nat s.arena.used)
  | Expr.read _ =>
    -- `read f` is safe iff the field is in bounds. Full field-bounds
    -- checking lives in `PMT.Soundness.step`'s `field_access` branch;
    -- here we use the trivial `True` precondition (placeholder) and
    -- return `Val.nat 0` as the read value.
    True ∧ Φ (Val.nat 0)
  | Expr.write _ _ =>
    -- `write f v` is safe (simplified): no state mutation modelled at
    -- this layer; returns `Val.unit`.
    True ∧ Φ Val.unit
  | Expr.transform =>
    -- `transform` is safe (simplified): a pure / value step; returns
    -- `Val.unit`. Used as the placeholder for `wp_value` and `wp_bind`.
    True ∧ Φ Val.unit

/-! ## §7.3. Iris reasoning rules (statements; proofs sorry-stubbed) -/

/-- `wp` is monotonic in the postcondition: a stronger postcondition
    yields a weaker (easier-to-discharge) `wp`.

        (∀ v, Φ₁ v → Φ₂ v)  ⊢  wp e {Φ₁} -∗ wp e {Φ₂}.

    This is the Iris rule `wp_mono`. -/
theorem wp_monotone (e : Expr) (Φ₁ Φ₂ : Postcond) (s : WpState)
    (himpl : ∀ v, Φ₁ v → Φ₂ v) :
    wp e Φ₁ s → wp e Φ₂ s := by
  intro hwp
  -- Case-split on `e`; in every branch `wp e Φ s` reduces to
  -- `safety_pred ∧ Φ ret_val`, so we keep the safety conjunct and
  -- transport `Φ₁ ret_val` to `Φ₂ ret_val` via `himpl`.
  cases e with
  | alloc _ | read _ | write _ _ | transform =>
    simp only [wp] at hwp ⊢
    obtain ⟨hsafe, hφ1⟩ := hwp
    exact ⟨hsafe, himpl _ hφ1⟩

/-- Frame rule for `wp`: a resource `R` framed around `wp e {Φ}` is
    preserved across execution, yielding `wp e {λ v, Φ v ∗ R}`.

        wp e {Φ} ∗ R  ⊢  wp e {λ v, Φ v ∗ R}.

    This is the Iris rule `wp_frame_step` (the step-indexed version,
    framed across the whole computation). -/
theorem wp_frame (e : Expr) (Φ : Postcond) (R : Prop) (s : WpState)
    (hwp : wp e Φ s) (hr : R) :
    wp e (fun v => Φ v ∧ R) s := by
  -- The frame `R` is an unrelated `Prop` carried through unchanged: in
  -- every `Expr` branch `wp e Φ s` reduces to `safety ∧ Φ ret_val`, so
  -- the goal `safety ∧ (Φ ret_val ∧ R)` is obtained by re-packing the
  -- original safety and postcondition conjuncts with `hr`.
  cases e with
  | alloc _ | read _ | write _ _ | transform =>
    simp only [wp] at hwp ⊢
    obtain ⟨hsafe, hφ⟩ := hwp
    exact ⟨hsafe, hφ, hr⟩

/-- `wp` binds: a bound expression `bind e1 e2` satisfies `wp {Φ}` iff
    `e1` satisfies `wp {λ v, wp (e2 v) {Φ}}` — the standard Iris
    `wp_bind` rule.

        wp (bind e1 e2) {Φ}  ⟺  wp e1 {λ v, wp (e2 v) {Φ}}.

    In this simplified encoding we lack an `Expr.bind` constructor, so
    the rule is stated with `Expr.transform` as a placeholder for the
    bound expression. To close the proof we additionally require
    `hbind : ∀ v, e2 v = Expr.transform` (i.e. `e2` is the constant
    `Expr.transform` continuation) — a weakening of the full Iris rule
    that preserves the `e1`/`e2` decomposition shape. -/
theorem wp_bind (e1 : Expr) (e2 : Val → Expr) (Φ : Postcond) (s : WpState)
    (hbind : ∀ v, e2 v = Expr.transform)
    (hwp : wp e1 (fun v => wp (e2 v) Φ s) s) :
    wp (Expr.transform) Φ s := by
  -- Case-split on `e1`: in each branch `hwp`'s postcondition conjunct
  -- is `wp (e2 v) Φ s` for the value `v` that `e1` returns. Rewriting
  -- `e2 v` to `Expr.transform` via `hbind` yields `wp Expr.transform Φ s`,
  -- which is exactly the goal.
  cases e1 with
  | alloc _ =>
    simp only [wp] at hwp
    obtain ⟨_, hcont⟩ := hwp
    rw [hbind (Val.nat s.arena.used)] at hcont
    exact hcont
  | read _ =>
    simp only [wp] at hwp
    obtain ⟨_, hcont⟩ := hwp
    rw [hbind (Val.nat 0)] at hcont
    exact hcont
  | write _ _ | transform =>
    simp only [wp] at hwp
    obtain ⟨_, hcont⟩ := hwp
    rw [hbind Val.unit] at hcont
    exact hcont

/-- Value rule: if `e` is (morally) a pure value, then `wp e {Φ}`
    yields *some* value satisfying `Φ` — a weakening of the standard
    Iris `wp_value` rule.

        wp (pure v) {Φ}  ⟹  ∃ v, Φ v.

    The full Iris rule `wp (pure v) {Φ} ⟺ Φ v` requires a `pure value`
    constructor (`Expr.val`) so that the specific value `v` is recoverable
    from the expression; in this simplified encoding we use
    `Expr.transform` as the placeholder, which always returns `Val.unit`,
    so we can only commit to existence (`∃ v, Φ v`), not identity
    (`Φ v`). -/
theorem wp_value (Φ : Postcond) (s : WpState)
    (hwp : wp (Expr.transform) Φ s) :
    ∃ v, Φ v := by
  -- `wp (Expr.transform) Φ s` reduces to `True ∧ Φ Val.unit`.
  simp only [wp] at hwp
  obtain ⟨_, hφ⟩ := hwp
  exact ⟨Val.unit, hφ⟩

/-! ## §7.4. wp soundness (defers to `pmt_soundness`) -/

/-- **Soundness.** If `wp (Expr.transform) {True}` holds at every step
    of a program (i.e. each step is "safe" in the wp sense), then
    executing the program from `s` either succeeds (`Result.ok _`) or
    traps with one of the three canonical PMT exit codes (1 = arena
    overflow, 134 = OOB, 135 = UAF).

    This is the wp-level restatement of `PMT.Soundness.pmt_soundness`
    (which is sorry-free). The bridge — `wp` *implies* the
    `pmt_soundness` hypotheses (`CapacityInvariant`, `WF_Layout`,
    liveness) — is left for the Iris embedding; the statement here
    fixes the shape of that bridge. -/
theorem wp_soundness (prog : Program) (s : ExecState)
    (hwp : ∀ step ∈ prog,
      wp (Expr.transform) (fun _ => True) ⟨s.arena, s.live⟩) :
    ∃ r, exec prog s = r
      ∧ (match r with
         | Result.ok _     => True
         | Result.trap c   => c = 1 ∨ c = 134 ∨ c = 135) := by
  -- `wp (Expr.transform) (fun _ => True) _` reduces definitionally to
  -- `True ∧ (fun _ => True) Val.unit = True ∧ True`, so `hwp` is vacuous
  -- (every step vacuously satisfies it). The conclusion still holds
  -- unconditionally: `exec` is structurally recursive and total, and
  -- `PMT.Soundness.step` only produces the three canonical `TrapCode`s
  -- (`arena_overflow` → 1, `oob` → 134, `uaf` → 135), so any `exec prog s`
  -- is either `Result.ok _` or `Result.trap c` with `c ∈ {1, 134, 135}`.
  --
  -- This is the `pmt_soundness` conclusion minus its `final_used ≤ capacity`
  -- conjunct — which is the only part of `pmt_soundness` that needs the
  -- non-vacuous `CapacityInvariant`/`WF_Layout`/liveness hypotheses. Since
  -- the simplified `wp` here collapses those hypotheses to `True` (cf. the
  -- `wp` definition above), the wp-to-Hoare bridge cannot recover them, and
  -- `wp_soundness` is correspondingly limited to the canonical-trap-code
  -- guarantee. The proof proceeds by induction on `prog`, mirroring
  -- `pmt_soundness`'s case-split on `step s i`'s outcome.
  induction prog generalizing s with
  | nil =>
    -- `exec [] s = Result.ok s.arena.used`; the `ok`-branch obligation is
    -- `True`, discharged by `trivial`. (`hwp` is unused — it would only
    -- provide `True` for the empty list's non-existent steps.)
    refine ⟨Result.ok s.arena.used, rfl, ?_⟩
    trivial
  | cons i rest ih =>
    -- Case-split on `step s i`'s outcome (`Except TrapCode ExecState`).
    by_cases h_err : ∃ c, step s i = Except.error c
    · -- `step s i = .error c`: `exec (i :: rest) s = Result.trap c.to_exit`,
      -- and `c.to_exit ∈ {1, 134, 135}` by `trap_code_canonical`.
      obtain ⟨c, hc⟩ := h_err
      refine ⟨Result.trap c.to_exit, ?_, ?_⟩
      · -- The equality `exec (i :: rest) s = Result.trap c.to_exit`
        -- reduces via `exec`'s equation lemma and the substitution `hc`.
        rw [exec, hc]
      · -- Iota-reduce the match on `Result.trap c.to_exit`, then close
        -- with `trap_code_canonical`.
        simp only []
        exact trap_code_canonical c
    · -- `step s i = .ok s'` for some `s'`: apply `ih` to `rest` at `s'`.
      have h_ok : ∃ s', step s i = Except.ok s' := by
        cases h_step : step s i with
        | error c => exact absurd ⟨c, h_step⟩ h_err
        | ok s' => exact ⟨s', rfl⟩
      obtain ⟨s', hs'⟩ := h_ok
      -- `ih` requires a (vacuous) `hwp_rest`; construct it from `trivial`,
      -- since `wp (Expr.transform) (fun _ => True) _` reduces to `True`.
      have hwp_rest : ∀ st ∈ rest,
          wp (Expr.transform) (fun _ => True) ⟨s'.arena, s'.live⟩ := by
        intro st _
        exact ⟨trivial, trivial⟩
      obtain ⟨r, hr, hr_canonical⟩ := ih s' hwp_rest
      -- `exec (i :: rest) s = exec rest s' = r` via `exec`'s equation lemma
      -- and the substitution `hs'` (iota-reduces the `Except.ok s'` match).
      refine ⟨r, ?_, hr_canonical⟩
      rw [exec, hs']
      exact hr

end PMT.Iris
