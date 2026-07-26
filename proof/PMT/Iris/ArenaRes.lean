import PMT.Basic
import PMT.Iris.CapBndInvariant

/-!
## Iris `ArenaRes` — the arena resource bundle

This module formalises `ArenaRes` from `docs/architecture/pmt-iris-spec.md`
§1 as a separation-logic resource, following the Iris methodology and the
pattern established by `PMT.Iris.CapBndInvariant` (the `[cap_bnd]`
invariant, formalised earlier).

**Spec (§1).**

```
Definition ArenaRes (A : Arena) : iProp Σ :=
  (A.base ↦{1} bytes_of A)                  (* exclusive bytes, |bytes|=cap *)
  ∗ own(γ_used, ●A.used)                     (* authoritative bump ptr    *)
  ∗ own(γ_cap,  Ag A.cap)                    (* immutable capacity        *)
  ∗ [guard] ((A.base + A.cap) ↦{1} PROT_NONE). (* guard page, see §6      *)
```

`ArenaRes A` is exclusive — a program holds it at most once. A `SubArena`
`⟨ofs, sz⟩` carved out of `A` is a slice points-to with bounds; the
splitting rule
`ArenaRes A -∗ SubArena A ofs sz ∗ ArenaRes⟨A.used+=sz⟩` (provided
`ofs+sz ≤ cap`) is the Iris statement of `arena_alloc`.

**Encoding (this module).**

  - `PointsTo addr bytes` — the points-to predicate `addr ↦ bytes`.
    In our simplified Iris model (cf. `CapBndInvariant.lean` §1), the
    heap is left implicit, so `PointsTo` is a bare `Prop` parameterised
    by the address and the bytes (the model captures only the algebraic
    shape of `↦`, not the heap model).
  - `ArenaRes γ_used γ_cap a` — the resource bundle, packaging:
      1. `used_own  : Own γ_used (ExRA.excl a.used)`   — the
         authoritative bump-pointer (exclusive RA, updated on `alloc`).
      2. `cap_own   : Own γ_cap (AgRA.ag a.capacity)`  — the immutable
         capacity (agreement RA, persistent across `alloc`).
      3. `points_to : PointsTo a.base []`              — the points-to
         for the arena's backing memory at `a.base` (bytes elided).
      4. `cap_bnd   : CapBndInv γ_used γ_cap a`        — the derived
         `[cap_bnd]` invariant (`PMT.Iris.CapBndInvariant`).

  - `arena_res_implies_cap_bnd`    — bundle ⇒ `[cap_bnd]` (projection).
  - `arena_res_implies_capacity`   — bundle ⇒ bare `CapacityInvariant`.
  - `alloc_preserves_arena_res`    — `alloc` preserves the bundle
    (bump-pointer ghost updated, capacity agreement persistent,
    points-to framed out, derived `[cap_bnd]` preserved).
  - `arena_res_split`              — `Sep`-splitting: bundle ⇒
    `own(γ_used, ●A.used) ∗ CapBndInv γ_used γ_cap a`.

**Adaptation note.** The task sketch used `γ_base`/`●base`; we adapt to
the actual `CapBndInv` API which uses `γ_used`/`●used` (the bump-pointer
is the exclusive authoritative resource, not `base`). The points-to
witnesses the base pointer's bytes. `Own` is `Prop`-valued and
parameterised by the value `v` (cf. `CapBndInvariant.lean` §1) — so
`Own` has no `.val` field; "consistency" between ghost and runtime
state is encoded at the *type* level (the parameter `v` is the value).
The heap/disjointness obligation of `∗` is left implicit in this
simplified encoding, exactly as in `CapBndInvariant.lean` §2. The
guard-page component (`[guard]`) is modelled in the sibling
`PMT.Iris.GuardInvariant` module; `ArenaRes` here captures the
bump-pointer + capacity + points-to triple (the part of §1 that does
not depend on the guard page).

**References.**
  - `docs/architecture/pmt-iris-spec.md` §1 (ArenaRes), §3 (`[cap_bnd]`).
  - `proof/PMT/Iris/CapBndInvariant.lean` — the `[cap_bnd]` invariant
    whose pattern this file mirrors (sibling module).
  - `proof/PMT/Basic.lean` — `Arena`, `alloc`, `CapacityInvariant`
    (the bare `Prop` that `ArenaRes` refines to a separation-logic
    resource).
-/

namespace PMT.Iris

/-! ## §1. The points-to predicate `addr ↦ bytes` {-/

/-- Points-to relation: `addr ↦ bytes` (exclusive). In real Iris this is
    `addr ↦{1} bytes : iProp`, modelled against a heap. In our simplified
    encoding (cf. `CapBndInvariant.lean` §1) we leave the heap implicit:
    `PointsTo addr bytes` is a `Prop` parameterised by the address and
    the bytes, capturing only the algebraic shape of the points-to
    resource. Downstream lemmas (e.g. `alloc_preserves_arena_res`) treat
    the points-to as a frame that is preserved across `alloc` because
    `alloc` only bumps `used` (the bump-pointer), not `base` — so the
    points-to at `a.base` is definitionally unchanged.

    Like `Own` in `CapBndInvariant.lean`, this is a `Prop`-valued empty
    `structure`: no data fields (only the address and bytes are
    parameters, not fields), so projections don't need `Classical.choice`,
    and the module is sorry-free and axiom-clean. -/
structure PointsTo (addr : Nat) (bytes : List Nat) : Prop

/-! ## §2. The `ArenaRes` resource bundle {-/

/-- `ArenaRes`: the arena resource bundle from `pmt-iris-spec.md` §1.

    The bundle owns three separation-logic resources (plus the derived
    `[cap_bnd]` invariant from `PMT.Iris.CapBndInvariant`):

      1. `used_own  : Own γ_used (ExRA.excl a.used)`   — authoritative
         bump-pointer (exclusive RA, updatable by the sole owner on
         `alloc`).
      2. `cap_own   : Own γ_cap (AgRA.ag a.capacity)`  — immutable
         capacity (agreement RA, persistent across `alloc`).
      3. `points_to : PointsTo a.base []`              — points-to for
         the arena's backing memory at `a.base` (bytes elided; the
         model does not track actual byte contents).
      4. `cap_bnd   : CapBndInv γ_used γ_cap a`        — derived:
         `[cap_bnd]` invariant holds (the bump-pointer is within
         capacity, witnessed by the same two ghost names).

    The two ghost names `γ_used`, `γ_cap` are parameters, so distinct
    arenas can be distinguished by their ghost-name pairs (matching
    Iris's per-arena ghost naming). The bundle is a `Prop`-valued
    `structure` rather than a `Sep`-nested `Prop` for the same reasons
    `PMTInvariants` (in `PMT.Iris.Composition`) is a structure:
    field access is by projection (no `Classical.choice`, no
    associativity rewriting), and the structure captures the same
    semantic content as the Iris `∗`-chain because the components live
    at disjoint ghost names (the disjointness obligation of `∗` is
    implicit in this simplified encoding — cf. `CapBndInvariant.lean`
    §2). -/
structure ArenaRes (γ_used γ_cap : GhostName) (a : Arena) : Prop where
  /-- Authoritative bump-pointer ownership `own(γ_used, ●A.used)`.
      Exclusive RA — the sole owner can update it on `alloc`. -/
  used_own  : Own γ_used (ExRA.excl a.used)
  /-- Capacity agreement ownership `own(γ_cap, Ag A.cap)`.
      Agreement RA — duplicable, hence persistent across `alloc`. -/
  cap_own   : Own γ_cap  (AgRA.ag  a.capacity)
  /-- Points-to relation for the arena's bytes at `A.base`
      (`A.base ↦{1} bytes_of A` in Iris; bytes elided here). -/
  points_to : PointsTo a.base []
  /-- Derived: the `[cap_bnd]` invariant holds at the same ghost
      names. (`ArenaRes` is a strict refinement of `CapBndInv`: every
      `ArenaRes` witness gives a `CapBndInv` witness via projection —
      see `arena_res_implies_cap_bnd`.) -/
  cap_bnd   : CapBndInv γ_used γ_cap a

/-! ## §3. Iris reasoning rules {-/

/-- `ArenaRes` implies `[cap_bnd]` (projection of the derived field).
    This is the trivial direction of the refinement
    `ArenaRes γ_used γ_cap a ⊣⊢ CapBndInv γ_used γ_cap a ∗ PointsTo …`
    — every `ArenaRes` witness packages a `CapBndInv` witness as its
    `cap_bnd` field. -/
theorem arena_res_implies_cap_bnd (γ_used γ_cap : GhostName) (a : Arena)
    (hres : ArenaRes γ_used γ_cap a) :
    CapBndInv γ_used γ_cap a :=
  hres.cap_bnd

/-- `ArenaRes` implies `CapacityInvariant` (the bare `Prop` from
    `PMT.Basic` that `pmt_soundness` uses as its hypothesis). This
    bridges the new Iris-style resource to the existing soundness proof
    by composing the projection `arena_res_implies_cap_bnd` with
    `cap_bnd_implies_capacity`. -/
theorem arena_res_implies_capacity (γ_used γ_cap : GhostName) (a : Arena)
    (hres : ArenaRes γ_used γ_cap a) :
    CapacityInvariant a :=
  cap_bnd_implies_capacity γ_used γ_cap a hres.cap_bnd

/-- `alloc` preserves `ArenaRes`: bumping `used` keeps the bundle intact.

    `alloc a l := { a with used := a.used + l.total_size }` only changes
    `a.used`; the points-to at `a.base` is preserved (the heap is
    unchanged), the capacity agreement is preserved (capacity is
    unchanged, and agreement is duplicable), the bump-pointer ghost
    `●A.used` is updated to `●(A.used + sz)`, and the derived
    `[cap_bnd]` invariant is preserved by `alloc_preserves_cap_bnd`.
    This is the Iris `alloc_preserves_cap` lemma from §3,
    upgraded to the full `ArenaRes` bundle. -/
theorem alloc_preserves_arena_res
    (γ_used γ_cap : GhostName) (a : Arena) (l : Layout)
    (hres : ArenaRes γ_used γ_cap a)
    (hfit : a.used + l.total_size ≤ a.capacity) :
    ArenaRes γ_used γ_cap (alloc a l) := by
  -- `(alloc a l).used = a.used + l.total_size`, `(alloc a l).capacity =
  -- a.capacity`, `(alloc a l).base = a.base` — all by defeq
  -- (structure-update projection reduction only touches `used`).
  refine ⟨?_, ?_, ?_, ?_⟩
  · -- Bump-pointer ghost updated: `own(γ_used, ●(A.used+sz))`.
    -- In our encoding `Own` is parameterised by the value (not a
    -- resource bundle storing it), so a fresh `Own` witness at the new
    -- bump-pointer value suffices (cf. `alloc_preserves_cap_bnd` in
    -- `CapBndInvariant.lean`, which does the same for the `[cap_bnd]`
    -- ghost).
    show Own γ_used (ExRA.excl (a.used + l.total_size))
    exact ⟨⟩
  · -- Capacity agreement ghost persistent: Agreement is duplicable,
    -- so `own(γ_cap, Ag A.cap)` is the same resource before and after
    -- `alloc`. `(alloc a l).capacity = a.capacity` by defeq, so the
    -- field type defeq-resolves to `hres.cap_own`'s type.
    show Own γ_cap (AgRA.ag a.capacity)
    exact hres.cap_own
  · -- Points-to preserved: `alloc` only bumps `used`; `base` is
    -- unchanged, so `(alloc a l).base = a.base` by defeq. The
    -- points-to `a.base ↦ bytes` therefore holds unchanged (in real
    -- Iris this is the frame rule on the heap; here the heap is
    -- implicit, so the witness is reused verbatim).
    show PointsTo a.base []
    exact hres.points_to
  · -- Derived `[cap_bnd]` invariant preserved:
    -- `alloc_preserves_cap_bnd : CapBndInv … a → … → CapBndInv … (alloc a l)`.
    exact alloc_preserves_cap_bnd γ_used γ_cap a l hres.cap_bnd hfit

/-- Sep splitting: `ArenaRes` can be split into the bump-pointer ghost
    `own(γ_used, ●A.used)` and the rest (`CapBndInv γ_used γ_cap a`).

    This is the Iris `∗`-splitting
    `ArenaRes A -∗ own(γ_used, ●A.used) ∗ CapBndInv γ_used γ_cap A`,
    used to thread the bump-pointer ghost through `arena_alloc` while
    framing out the capacity-agreement ghost and the points-to. In our
    simplified encoding disjointness is implicit, so the splitting is a
    pair-introduction (cf. `frame_rule` in `CapBndInvariant.lean` §4). -/
theorem arena_res_split
    (γ_used γ_cap : GhostName) (a : Arena)
    (hres : ArenaRes γ_used γ_cap a) :
    Sep (Own γ_used (ExRA.excl a.used)) (CapBndInv γ_used γ_cap a) :=
  ⟨hres.used_own, hres.cap_bnd⟩

/-- `ArenaRes` is *not* duplicable: the exclusive `ExRA.excl a.used`
    ghost makes the bundle linear (a program holds `ArenaRes A` at most
    once). We record this by extracting the bump-pointer ghost as a
    singleton resource (via `arena_res_split`'s left projection) — the
    exclusivity principle `own(γ, Ex a) ∗ own(γ, Ex b) -∗ False` is
    axiomatised in `LiveMirrorInvariant.lean` (`own_ex_exclusive`) for
    the `Ex` RA, and applies here to `γ_used`.

    This theorem — the *projection* of the bump-pointer ghost — is the
    shape of the exclusivity witness without invoking the `Ex`-RA
    exclusivity axiom. It says: from `ArenaRes` you can extract the
    exclusive `●A.used` ghost as a standalone resource. -/
theorem arena_res_used_own (γ_used γ_cap : GhostName) (a : Arena)
    (hres : ArenaRes γ_used γ_cap a) :
    Own γ_used (ExRA.excl a.used) :=
  hres.used_own

end PMT.Iris
