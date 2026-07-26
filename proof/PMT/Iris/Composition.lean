import PMT.Iris.CapBndInvariant
import PMT.Iris.LiveMirrorInvariant
import PMT.Iris.GuardInvariant

/-!
## Iris Composition — combining `[cap_bnd] ∗ [live_mirror] ∗ [guard]`

This module proves that the three named Iris invariants formalised
earlier — `[cap_bnd]` (`PMT.Iris.CapBndInvariant`), `[live_mirror]`
(`PMT.Iris.LiveMirrorInvariant`), and `[guard]` (`PMT.Iris.GuardInvariant`)
— compose: if all three hold before an `alloc`, all three hold after.

**Key constructs**

  - `PMTInvariants`                       — the full invariant bundle
    `[cap_bnd] ∗ [live_mirror] ∗ [guard]`, packaged as a `structure`
    over the four ghost names (`γ_used`, `γ_cap`, `γ_live`, `γ_guard`)
    plus the arena `a`, the variable name `var`, and the liveness bit
    `live`.
  - `alloc_preserves_all_invariants`      — composition theorem: `alloc`
    preserves every component of the bundle.
  - `pmt_invariants_implies_capacity`     — the bundle implies the bare
    `CapacityInvariant` from `PMT.Basic` (via `cap_bnd_implies_capacity`).
  - `pmt_invariants_implies_guard_page`   — the bundle implies `GuardPage`
    for overflow addresses (via `guard_inv_implies_guard_page`).

**Design note — why no `sorry`.** The `[live_mirror]` preservation step
in `alloc_preserves_all_invariants` is, in this simplified encoding,
trivial: `LiveMirrorInv γ var live` has *no* `Arena` parameter (only
`γ`, `var`, `live`), and `alloc` touches only `a.used` (the `used`
bump-pointer), leaving both `var` and `live` unchanged. Hence the
`live_mirror` field of the pre-`alloc` bundle is *definitionally* the
same proposition as the `live_mirror` field of the post-`alloc` bundle,
and preservation follows by `exact hinv.live_mirror` (no `sorry`).
Real Iris would still need the heap model to show that the physical
`(liveness_byte v) ↦{1} encode(b)` points-to is preserved when `used`
is bumped (because `alloc` doesn't touch the liveness byte region) —
that obligation is implicit in this simplified encoding, exactly as
the disjointness obligation of `Sep` is implicit in
`CapBndInvariant.lean` §2.

**References.**
  - `docs/architecture/pmt-iris-spec.md` §3, §5, §6, §9 (composition).
  - `proof/PMT/Iris/CapBndInvariant.lean`   — `[cap_bnd]`.
  - `proof/PMT/Iris/LiveMirrorInvariant.lean` — `[live_mirror]`.
  - `proof/PMT/Iris/GuardInvariant.lean`    — `[guard]`.
-/

namespace PMT.Iris

/-! ## §7. The full PMT invariant bundle `[cap_bnd] ∗ [live_mirror] ∗ [guard]`
-/

/-- The full PMT invariant bundle: `[cap_bnd] ∗ [live_mirror] ∗ [guard]`.

    This is the composition of the three named Iris invariants.
    Each field is one of the three named invariants:

      * `cap_bnd`     — `CapBndInv γ_used γ_cap a`        (§3)
      * `live_mirror` — `LiveMirrorInv γ_live var live`   (§5)
      * `guard`       — `GuardInv γ_guard a`              (§6)

    The four ghost names `γ_used`, `γ_cap`, `γ_live`, `γ_guard` are all
    parameters: in Iris each named invariant lives at its own ghost
    name, so the bundle uses four distinct names (one per invariant's
    ghost state). The arena `a`, variable name `var`, and liveness bit
    `live` are also parameters — they identify the runtime objects the
    invariants govern.

    The bundle is a `structure` (not a `Sep`-nested `Prop`) for two
    reasons: (1) field access is by projection (`hinv.cap_bnd`,
    `hinv.live_mirror`, `hinv.guard`) — no `Classical.choice`, no
    associativity rewriting; (2) the structure captures the same
    semantic content as the Iris `∗`-chain
    `[cap_bnd] ∗ [live_mirror] ∗ [guard]` because the three invariants
    live at disjoint ghost names (cf. `CapBndInvariant.lean` §2: the
    disjointness obligation of `∗` is left implicit in this simplified
    encoding; the structure records the three witnesses without
    duplicating the disjointness bookkeeping). -/
structure PMTInvariants (γ_used γ_cap γ_live γ_guard : GhostName)
                         (a : Arena) (var : String) (live : Liveness) : Prop where
  /-- The `[cap_bnd]` invariant: `a.used ≤ a.capacity` with ghost
      witnesses `own(γ_used, ●used) ∗ own(γ_cap, Ag cap)`. -/
  cap_bnd : CapBndInv γ_used γ_cap a
  /-- The `[live_mirror]` invariant: ghost `own(γ_live, Ex live)`
      mirrors the runtime liveness of variable `var`. -/
  live_mirror : LiveMirrorInv γ_live var live
  /-- The `[guard]` invariant: guard page is `PROT_NONE` at
      `a.base + a.capacity`, witnessed by `own(γ_guard, Ag (base+cap))`. -/
  guard : GuardInv γ_guard a

/-! ## §7.1. The composition theorem -/

/-- `alloc` preserves all three invariants. If the bundle
    `[cap_bnd] ∗ [live_mirror] ∗ [guard]` holds before `alloc`, and
    the `alloc` precondition `a.used + l.total_size ≤ a.capacity`
    holds, then the bundle holds after `alloc` (with the `cap_bnd`
    ghost state updated: `●used` is bumped to `●(used + sz)`).

    This is the composition theorem of the three preservation
    lemmas:

      * `alloc_preserves_cap_bnd`     (`CapBndInvariant.lean`)
      * `alloc_preserves_guard`       (`GuardInvariant.lean`)
      * (`LiveMirrorInvariant.lean` has no `alloc_preserves_*` lemma —
        `LiveMirrorInv` has no `Arena` parameter, so `alloc` (which
        touches only `a.used`) cannot affect it; the witness is reused
        verbatim.)

    The proof is the conjunction of the three component proofs; the
    `refine ⟨?_, ?_, ?_⟩` tactic opens the three goals and each is
    discharged by the corresponding component lemma (or, for
    `live_mirror`, by direct field reuse). -/
theorem alloc_preserves_all_invariants
    (γ_used γ_cap γ_live γ_guard : GhostName)
    (a : Arena) (l : Layout) (var : String) (live : Liveness)
    (hinv : PMTInvariants γ_used γ_cap γ_live γ_guard a var live)
    (hfit : a.used + l.total_size ≤ a.capacity) :
    PMTInvariants γ_used γ_cap γ_live γ_guard (alloc a l) var live := by
  refine ⟨?_, ?_, ?_⟩
  · -- `[cap_bnd]` preserved: the bump-pointer is updated and stays
    -- within capacity. Ghost `●used` bumped to `●(used+sz)`; ghost
    -- `Ag cap` persistent.
    exact alloc_preserves_cap_bnd γ_used γ_cap a l hinv.cap_bnd hfit
  · -- `[live_mirror]` preserved: `LiveMirrorInv γ_live var live`
    -- has no `Arena` parameter — only `γ_live`, `var`, and `live` —
    -- and `alloc` touches only `a.used` (the bump-pointer), leaving
    -- `var` and `live` unchanged. Hence the pre-`alloc` witness is
    -- definitionally the same proposition as the post-`alloc` goal.
    -- (In real Iris the physical `(liveness_byte v) ↦{1} encode(b)`
    -- points-to would also need to be shown preserved; that
    -- disjointness obligation is implicit in this simplified encoding
    -- — cf. `CapBndInvariant.lean` §2.)
    exact hinv.live_mirror
  · -- `[guard]` preserved: `alloc` does not move the guard page
    -- (only `used` is bumped; `base`/`capacity` are unchanged, so
    -- `base + capacity` is unchanged). Ghost `Ag (base+cap)`
    -- persistent.
    exact alloc_preserves_guard γ_guard a l hinv.guard

/-! ## §7.2. Bridging theorems — bundle implies bare `Prop`s -/

/-- The invariant bundle implies `CapacityInvariant` (the bare `Prop`
    from `PMT.Basic` that `pmt_soundness` uses as its hypothesis).
    This delegates to `cap_bnd_implies_capacity`. -/
theorem pmt_invariants_implies_capacity
    (γ_used γ_cap γ_live γ_guard : GhostName)
    (a : Arena) (var : String) (live : Liveness)
    (hinv : PMTInvariants γ_used γ_cap γ_live γ_guard a var live) :
    CapacityInvariant a :=
  cap_bnd_implies_capacity γ_used γ_cap a hinv.cap_bnd

/-- The invariant bundle implies `GuardPage` for addresses
    `addr ≥ a.base + a.capacity` (the overflow condition: any access
    past the live arena trips the guard). This delegates to
    `guard_inv_implies_guard_page`. -/
theorem pmt_invariants_implies_guard_page
    (γ_used γ_cap γ_live γ_guard : GhostName)
    (a : Arena) (var : String) (live : Liveness)
    (hinv : PMTInvariants γ_used γ_cap γ_live γ_guard a var live)
    (addr : Nat) (haccess : addr ≥ a.base + a.capacity) :
    GuardPage a addr :=
  guard_inv_implies_guard_page γ_guard a hinv.guard addr haccess

end PMT.Iris
