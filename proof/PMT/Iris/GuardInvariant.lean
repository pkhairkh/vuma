import PMT.Basic
import PMT.Liveness
import PMT.Iris.CapBndInvariant

/-!
## Iris-style `[guard]` Named Invariant

This module formalises the `[guard]` invariant from
`docs/architecture/pmt-iris-spec.md` §6 as a separation-logic resource
with ghost state, following the Iris methodology.

**Key constructs**

  - `GuardInv` — the `[guard]` named invariant: the guard page sits at
    `base + capacity` with `PROT_NONE`, witnessed by an agreement ghost
    resource `own(γ, Ag (base + capacity))`.
  - `guard_inv_implies_guard_page` — bridges to the bare `GuardPage`
    predicate from `PMT.Liveness`.
  - `alloc_preserves_guard` — frame-preserving update: `alloc` does not
    move the guard page (only `used` is bumped; `base`/`capacity` are
    unchanged).
  - `guard_inv_persistent` — `Ag` agreement is duplicable, so `[guard]`
    is persistent (`GuardInv ⊣⊢ GuardInv ∗ GuardInv`).

This is the SECOND Iris named invariant formalised in the VUMA project
(after `PMT.Iris.CapBndInvariant`). It upgrades the bare `GuardPage`
predicate (defined in `PMT.Liveness` as a pure `Prop` with no resource
content) to a separation-logic resource by adding an agreement ghost
witness. As with `CapBndInv`, the encoding is a SIMPLIFIED Iris model:
`Own γ v` is parameterised by the value `v` (rather than a resource
bundle storing `v`), so all fields of `GuardInv` are `Prop`s — no
`Classical.choice` is needed for field access, and the module is
sorry-free and axiom-clean modulo `Classical.propDecidable` already
used elsewhere in `PMT`. `Sep P Q` is a `Prop`-valued pair, simplified
from Iris's heap-disjointness semantics; the disjointness obligation is
left implicit (the model does not track a heap). This captures the
algebraic structure of `∗` (commutativity, associativity, frame rule)
without the heap model.

**Design note.** A `Prop`-valued `structure` cannot carry data fields
of type `Type` (e.g. `Nat`) — only proof fields. So the guard-page
address is NOT stored as a separate `guard_start : Nat` witness; it is
computed inline as `a.base + a.capacity` (the arena-derived address)
and used directly as the parameter of the `AgRA.ag` ghost ownership.
This mirrors `CapBndInv`, where `a.capacity` is likewise used directly
in `AgRA.ag a.capacity`.

**References.**
  - `docs/architecture/pmt-iris-spec.md` §6 (guard page), §8 (TCB row
    "mmap PROT_NONE guard page semantics — Trusted").
  - `proof/PMT/Liveness.lean` — `GuardPage` (the bare `Prop` that
    `GuardInv` upgrades to a separation-logic resource).
  - `proof/PMT/Iris/CapBndInvariant.lean` — sibling `[cap_bnd]`
    invariant whose pattern this file mirrors.
-/

namespace PMT.Iris

/-! ## §1. The `[guard]` named invariant

The guard page sits at `base + capacity` and is `PROT_NONE` (trusted OS
contract — `pmt-iris-spec.md` §8). Any access at `addr ≥ base +
capacity` traps via the MMU. The Iris `[guard]` invariant packages this
pure fact with an agreement (`Ag`) ghost resource
`own(γ, Ag (base + capacity))` so that all owners agree on the
guard-page location. -/

/-- The `[guard]` named invariant: the guard page is `PROT_NONE` at
    `base + capacity`, witnessed by ghost state
    `own(γ, Ag (a.base + a.capacity))`.

    This is the SECOND Iris named invariant in the VUMA project (after
    `CapBndInv`). It upgrades the bare `GuardPage` predicate from
    `PMT.Liveness` (a pure `Prop` with no resource content) to a
    separation-logic resource by adding an agreement ghost witness:

      * `ghost` — agreement ownership `own(γ, Ag (a.base + a.capacity))`.

    The single ghost name `γ` is a parameter, so distinct arenas can be
    distinguished by their guard-page ghost name (matching Iris's
    per-arena ghost naming). The agreement RA `Ag` is duplicable, so
    `GuardInv` is persistent (see `guard_inv_persistent`).

    The guard-page address `a.base + a.capacity` is computed inline
    from the arena projections rather than stored as a separate `Nat`
    field, because a `Prop`-valued `structure` cannot carry data fields
    of type `Type` (only proofs). This mirrors `CapBndInv`, where
    `a.capacity` is likewise used directly in `AgRA.ag a.capacity`. -/
structure GuardInv (γ : GhostName) (a : Arena) : Prop where
  /-- Ghost witness: agreement ownership
      `own(γ, Ag (a.base + a.capacity))`. -/
  ghost : Own γ (AgRA.ag (a.base + a.capacity))

/-! ## §2. Iris reasoning rules -/

/-- `[guard]` implies `GuardPage` (the bare `Prop` from `PMT.Liveness`).
    This bridges the new Iris-style invariant to the existing
    `in_arena_below_guard` theorem, which uses `GuardPage` as its
    hypothesis. The hypothesis `addr ≥ a.base + a.capacity` is the
    "overflow" condition: any access past the live arena trips the
    guard. -/
theorem guard_inv_implies_guard_page (γ : GhostName) (a : Arena)
    (_hinv : GuardInv γ a) (addr : Nat)
    (haccess : addr ≥ a.base + a.capacity) :
    GuardPage a addr := by
  -- `GuardPage a addr := a.base + a.capacity ≤ addr`, and `addr ≥ …`
  -- is the same fact in `GE` notation. (`_hinv` is the Iris precondition
  -- — present in the signature for API completeness; the proof is the
  -- pure arithmetic identity, since `GuardPage` is purely arithmetic.)
  unfold GuardPage
  exact haccess

/-- `alloc` preserves `[guard]`: the guard page does not move on
    bump-allocation. `alloc a l := { a with used := a.used + l.total_size }`
    changes only `used`; `base` and `capacity` are unchanged, so
    `base + capacity` (the guard-page address) is unchanged.

    This is the frame-preserving update for the `[guard]` invariant —
    the guard-page ghost resource `own(γ, Ag (base + capacity))` is
    persistent (Agreement is duplicable) and is carried unchanged. -/
theorem alloc_preserves_guard (γ : GhostName) (a : Arena) (l : Layout)
    (hinv : GuardInv γ a) :
    GuardInv γ (alloc a l) := by
  -- `(alloc a l).base = a.base` and `(alloc a l).capacity = a.capacity`
  -- by defeq (structure-update projection reduction), so
  -- `(alloc a l).base + (alloc a l).capacity` reduces definitionally
  -- to `a.base + a.capacity`. Hence `Own γ (AgRA.ag (a.base + a.capacity))`
  -- — the field type — is defeq to `hinv.ghost`'s type. The single-field
  -- structure is then constructed via the anonymous constructor.
  exact ⟨hinv.ghost⟩

/-- `[guard]` is persistent: `GuardInv γ a ⊣⊢ GuardInv γ a ∗ GuardInv γ a`.
    Agreement (`Ag`) is duplicable in Iris (`Ag x ⊣⊢ Ag x ∗ Ag x`), so
    the invariant can be freely duplicated. In our simplified encoding
    this is just the pair-introduction for `Sep` (disjointness of ghost
    resources is implicit). -/
theorem guard_inv_persistent (γ : GhostName) (a : Arena)
    (hinv : GuardInv γ a) :
    Sep (GuardInv γ a) (GuardInv γ a) :=
  ⟨hinv, hinv⟩

end PMT.Iris
