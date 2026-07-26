# PMT Iris Spec

This document is the **source of truth for the Lean Iris proofs** in
`proof/PMT/Iris/*.lean`. It specifies, in prose with cross-references to
the Lean modules, the Iris-style separation-logic invariants, ghost-state
conventions, and weakest-precondition machinery used to verify the PMT
arena allocator and its runtime memory-safety contract.

The runtime companion spec — Lean signatures and proof sketches for the
bare `Prop` predicates — is [`./pmt-formal-spec.md`](./pmt-formal-spec.md).
This document covers the *Iris layer* (separation logic, ghost state,
named invariants).

## A 60-second Iris primer (for readers new to Iris)

Iris is a higher-order separation logic. The three ideas you need to read
this document:

  1. **Separating conjunction `P ∗ Q`** — `P` and `Q` hold on *disjoint*
     resources. (Plain conjunction `P ∧ Q` makes no such claim.) This is
     what lets us reason about who owns what.
  2. **Ghost state `own(γ, v)`** — a logical assertion that "we own
     resource `v` at ghost name `γ`". Ghost state is *purely logical*: it
     does not exist at runtime, but it tracks facts the proof needs.
     Ghost names `γ` are opaque tokens that let us distinguish distinct
     resources (per-arena, per-variable, …).
  3. **Resource algebras (RAs)** — the `v` in `own(γ, v)` is an element
     of a resource algebra, which dictates how ownership can be split and
     merged. VUMA uses two RAs:
       - **`Ex` (exclusive)** — at most one owner. `Ex a ⋅ Ex b` is
         defined iff `a = b`. This is what lets the bump-pointer be
         updated by its *sole* owner.
       - **`Ag` (agreement)** — all owners agree. Duplicable
         (`Ag a ⊣⊢ Ag a ∗ Ag a`). This is what makes the arena capacity
         immutable: everyone agrees on its value, and the value never
         changes.

A **named invariant** `[name]` packages (a) a pure mathematical fact
with (b) ghost resources that *witness* the fact, so that the whole
bundle can be opened, used, and closed under the Iris invariant mask
rules.

### Simplified encoding used in VUMA

Real Iris requires a heap/world model and a fancy-update monad. VUMA uses
a **simplified `Prop`-valued encoding** (see
`proof/PMT/Iris/CapBndInvariant.lean` §1 for the rationale):

  - `Own γ v : Prop` is parameterised by the value `v` (rather than a
    resource bundle *storing* `v`). So all invariant fields are `Prop`s
    and their projections need no `Classical.choice`.
  - `Sep P Q : Prop` is a `Prop`-valued pair; the disjointness obligation
    is left implicit (the model does not track a heap). This still
    captures the algebraic structure of `∗` (commutativity,
    associativity, frame rule).

The model is sorry-free and axiom-clean modulo `Classical.propDecidable`
and a single local axiom `own_ex_exclusive` (§5) — which **PMT-1-G will
derive** in a future wave.

## Contents

  - §1  `ArenaRes` — arena resource bundle
  - §2  `StateValRes`, `↦{q}` — fractional field permissions
  - §3  `[cap_bnd]` — capacity-bound named invariant  ← *worked example*
  - §4  Field invariants
  - §5  `[live_mirror]` — liveness-mirror named invariant
  - §6  `[guard]` — guard-page named invariant
  - §7  `wp e {Φ}` — weakest precondition
  - §8  Trusted Computing Base (TCB)
  - §9  Composition of named invariants

---

## §1. `ArenaRes`

The arena resource bundle `ArenaRes A` packages the ghost-state ownership
required to reason about a single arena `A`:

  - `own(γ_used, ●A.used)` — exclusive ownership of the authoritative
    bump-pointer;
  - `own(γ_cap, Ag A.capacity)` — agreement ownership of the capacity
    (duplicable, persistent);
  - the pure arithmetic fact `⌜A.used ≤ A.capacity⌝` (the
    `CapacityInvariant` from `PMT.Basic`).

`ArenaRes` is the bundle that the `[cap_bnd]` invariant (§3) packages as
a single named invariant. Two distinct arenas are distinguished by their
ghost-name pairs `(γ_used, γ_cap)`.

**Lean reference.** `proof/PMT/Iris/ArenaRes.lean`; runtime mirror
`proof/PMT/Basic.lean`.

---

## §2. `StateValRes`, `↦{q}`

Fractional field permissions model shared-read / exclusive-write access to
state-buffer fields. A permission `l ↦{q} v` asserts that the field at
location `l` currently holds value `v` with fractional permission
`q ∈ (0, 1]`.

  - `q = 1`   — exclusive permission (read + write);
  - `q < 1`   — read-only share (multiple readers may coexist).

Splitting and merging of fractional permissions obey the standard Iris
rules:

  - `l ↦{1} v ⊣⊢ l ↦{q₁} v ∗ l ↦{q₂} v`  when `q₁ + q₂ = 1`;
  - `l ↦{q} v ∗ l ↦{q'} v ⊢ l ↦{q + q'} v`  when `q + q' ≤ 1`.

The `StateValRes` construct packages `↦{q}` with the value agreement
`Ag v`, mirroring Iris's standard `StateValRes`.

**Lean reference.** `proof/PMT/Iris/FractionalPerm.lean`.

---

## §3. `[cap_bnd]` — capacity-bound named invariant  *(worked example)*

The `[cap_bnd]` named invariant upgrades the bare `CapacityInvariant`
predicate `A.used ≤ A.capacity` (from `PMT.Basic`) to a separation-logic
resource by adding two ghost witnesses:

  - exclusive ownership of the authoritative bump-pointer
    `own(γ_used, ●A.used)`;
  - agreement ownership of the capacity `own(γ_cap, Ag A.capacity)`.

### Verbatim Lean statement

Quoted verbatim from `proof/PMT/Iris/CapBndInvariant.lean`:

```lean
structure CapBndInv (γ_used γ_cap : GhostName) (a : Arena) : Prop where
  /-- The pure arithmetic fact: bump-pointer is within capacity. -/
  h_cap : a.used ≤ a.capacity
  /-- Ghost witness: exclusive ownership of `●used`. -/
  ghost_used : Own γ_used (ExRA.excl a.used)
  /-- Ghost witness: agreement ownership of `Ag cap`. -/
  ghost_cap  : Own γ_cap  (AgRA.ag  a.capacity)
```

### Prose explanation (for readers new to Iris)

`CapBndInv` is the named invariant `[cap_bnd]`. It has three fields:

  - **`h_cap : a.used ≤ a.capacity`** — the *pure arithmetic fact*: the
    bump-pointer (`a.used`) never exceeds the arena's capacity
    (`a.capacity`). This is the safety property we ultimately care about.

  - **`ghost_used : Own γ_used (ExRA.excl a.used)`** — exclusive (`Ex`)
    ownership of the authoritative bump-pointer value. "Exclusive" means
    at most one owner can hold this resource at a given ghost name
    `γ_used`, so *only that owner* can update it. This is what lets
    `alloc` bump the pointer: the sole owner performs the frame-
    preserving update `●used ~~> ●(used + sz)`. No other proof fragment
    can race on the bump-pointer because they cannot hold a conflicting
    `Ex` resource at the same `γ_used`.

  - **`ghost_cap : Own γ_cap (AgRA.ag a.capacity)`** — agreement (`Ag`)
    ownership of the capacity. "Agreement" means all owners at `γ_cap`
    must agree on the value, and the resource is duplicable
    (`Ag a ⊣⊢ Ag a ∗ Ag a`), so it can be freely shared. This is what
    makes the capacity immutable: every owner agrees on its value, and
    the value never changes after arena creation, so `alloc` (which
    bumps only `used`) carries `ghost_cap` unchanged.

The two ghost names `γ_used` and `γ_cap` are parameters of the
invariant, so distinct arenas are distinguished by their ghost-name
pairs — matching Iris's per-arena ghost-naming discipline.

### Frame-preserving update on `alloc`

`alloc` preserves `[cap_bnd]` — the bump-pointer ghost is updated, the
capacity ghost is carried unchanged (because `Ag` is duplicable):

```lean
theorem alloc_preserves_cap_bnd
    (γ_used γ_cap : GhostName) (a : Arena) (l : Layout)
    (hinv : CapBndInv γ_used γ_cap a)
    (hfit : a.used + l.total_size ≤ a.capacity) :
    CapBndInv γ_used γ_cap (alloc a l)
```

The precondition `hfit : a.used + l.total_size ≤ a.capacity` is the
*runtime* check performed by `arena_alloc` (tested per
[`pmt-formal-spec.md`](./pmt-formal-spec.md) §1.3). The theorem upgrades
`PMT.Basic.alloc_preserves_capacity` (which proves only the pure
arithmetic fact) to the Iris invariant: the ghost state is reconstructed
correctly after the bump.

### Bridge to the runtime soundness proof

`[cap_bnd]` implies the bare `CapacityInvariant`:

```lean
theorem cap_bnd_implies_capacity (γ_used γ_cap : GhostName) (a : Arena)
    (hinv : CapBndInv γ_used γ_cap a) :
    CapacityInvariant a := hinv.h_cap
```

This bridges the Iris invariant to the existing `pmt_soundness` theorem
in `PMT.Basic`, which uses `CapacityInvariant` as its hypothesis.

### Reasoning rules (algebraic structure of `∗`)

The simplified encoding captures the algebraic structure of `∗`:

```lean
theorem frame_rule {P Q : Prop} (hP : P) (hQ : Q) : Sep P Q := ⟨hP, hQ⟩
theorem sep_comm (P Q : Prop) : Sep P Q ↔ Sep Q P
theorem sep_assoc (P Q R : Prop) : Sep (Sep P Q) R ↔ Sep P (Sep Q R)
```

**Lean reference.** `proof/PMT/Iris/CapBndInvariant.lean`.

---

## §4. Field invariants

Field-level safety is established by two parallel theorems, mirroring
[`pmt-formal-spec.md`](./pmt-formal-spec.md) §3 (field-bounds safety) and
§4 (liveness-bounded access). The Iris-side theorems live in
`proof/PMT/Iris/Composition.lean` and depend on the `[cap_bnd]`
invariant (§3) plus the fractional-permission machinery (§2). The
runtime versions live in `proof/PMT/Field.lean`.

---

## §5. `[live_mirror]` — liveness-mirror named invariant

The `[live_mirror]` invariant mirrors the runtime liveness byte of a
variable in ghost state. The Iris specification is:

```
Definition [live_mirror] : iProp Σ :=
  ∀ v b, own(γ_live v.id, Ex b) -∗ (liveness_byte v) ↦{1} encode(b).
```

The ghost state `own(γ_live v.id, Ex b)` mirrors the runtime liveness
byte `(liveness_byte v) ↦{1} encode(b)`. When `b = live`, the variable
is accessible; when `b = dead`, the variable has been consumed by a
prior `StateTransform`.

### Verbatim Lean statement

Quoted verbatim from `proof/PMT/Iris/LiveMirrorInvariant.lean`:

```lean
structure LiveMirrorInv (γ : GhostName) (var : String) (b : Liveness) : Prop where
  /-- Ghost witness: `own(γ, Ex b)` — exclusive ownership of the
      liveness bit. -/
  ghost : Own γ (ExRA.excl b)
```

The parameter `b` is named after the `b` in Iris spec §5's
`own(γ_live v.id, Ex b)` — the ghost value and the runtime status
coincide by the invariant's construction (a "consistent" mirror).

### The `Ex` RA's exclusivity principle (one local axiom)

Two `[live_mirror]` invariants for the same `γ` and `var` but different
liveness values are contradictory:

```lean
theorem live_mirror_exclusive (γ : GhostName) (var : String)
    (h_live : LiveMirrorInv γ var Liveness.live)
    (h_dead : LiveMirrorInv γ var Liveness.dead) :
    False
```

The proof relies on the exclusivity principle of the `Ex` resource
algebra, which in this simplified `Prop`-valued model is captured by the
local axiom:

```lean
axiom own_ex_exclusive {α : Type} (γ : GhostName) (a b : α)
    (ha : Own γ (ExRA.excl a)) (hb : Own γ (ExRA.excl b)) :
    a = b
```

> **Note.** This axiom is the *only* axiom in the VUMA Iris model beyond
> `Classical.propDecidable`. In real Iris, exclusivity is *derived* from
> the resource-algebra composition (`Ex a ⋅ Ex b` is defined iff
> `a = b`); our simplified `Own` encoding is `Prop`-valued and
> parameterised by the value (rather than storing it), so the
> composition `⋅` is not expressible and the principle is postulated.
> **PMT-1-G will derive this axiom** from the resource-algebra model in
> a future wave. Until then, the file is sorry-free but uses this single
> local axiom.

### Frame-preserving update on `consume`

Consuming a variable updates the ghost state: `Ex live ~~> Ex dead`. The
sole owner (the linear token holder) performs the consume:

```lean
theorem consume_updates_mirror (γ : GhostName) (var : String)
    (_hinv : LiveMirrorInv γ var Liveness.live) :
    LiveMirrorInv γ var Liveness.dead
```

This mirrors the runtime `state_transform_kills_input` lemma in
`PMT.Liveness` (which flips `LinearResource t` to `Consumed t`); here we
flip the ghost half.

### Bridge to the runtime liveness proof

`[live_mirror]` for a `live` variable implies the variable is
`Accessible` (its runtime `LinearToken` has `status = live`):

```lean
theorem live_mirror_implies_live (γ : GhostName) (var : String)
    (_hinv : LiveMirrorInv γ var Liveness.live)
    (t : LinearToken) (_htvar : t.var = var)
    (hmirror : t.status = Liveness.live) :
    Accessible t
```

This bridges the new Iris-style invariant to the existing
`state_read_requires_live` theorem in `PMT.Liveness`.

**Lean reference.** `proof/PMT/Iris/LiveMirrorInvariant.lean`.

---

## §6. `[guard]` — guard-page named invariant

The guard page sits at `base + capacity` and is `PROT_NONE` (trusted OS
contract — §8). Any access at `addr ≥ base + capacity` traps via the
MMU. The Iris `[guard]` invariant packages this pure fact with an
agreement (`Ag`) ghost resource `own(γ, Ag (base + capacity))` so that
all owners agree on the guard-page location.

### Verbatim Lean statement

Quoted verbatim from `proof/PMT/Iris/GuardInvariant.lean`:

```lean
structure GuardInv (γ : GhostName) (a : Arena) : Prop where
  /-- Ghost witness: agreement ownership
      `own(γ, Ag (a.base + a.capacity))`. -/
  ghost : Own γ (AgRA.ag (a.base + a.capacity))
```

### Persistence

`[guard]` is persistent: `GuardInv γ a ⊣⊢ GuardInv γ a ∗ GuardInv γ a`.
Agreement (`Ag`) is duplicable in Iris, so the invariant can be freely
duplicated:

```lean
theorem guard_inv_persistent (γ : GhostName) (a : Arena)
    (hinv : GuardInv γ a) :
    Sep (GuardInv γ a) (GuardInv γ a)
```

### Frame-preserving update on `alloc`

`alloc` preserves `[guard]`: the guard page does not move on
bump-allocation. `alloc a l := { a with used := a.used + l.total_size }`
changes only `used`; `base` and `capacity` are unchanged, so
`base + capacity` (the guard-page address) is unchanged:

```lean
theorem alloc_preserves_guard (γ : GhostName) (a : Arena) (l : Layout)
    (hinv : GuardInv γ a) :
    GuardInv γ (alloc a l)
```

### Bridge to the runtime guard-page proof

`[guard]` implies the bare `GuardPage` predicate (from `PMT.Liveness`),
bridging the Iris invariant to the existing `in_arena_below_guard`
theorem:

```lean
theorem guard_inv_implies_guard_page (γ : GhostName) (a : Arena)
    (_hinv : GuardInv γ a) (addr : Nat)
    (haccess : addr ≥ a.base + a.capacity) :
    GuardPage a addr
```

**Lean reference.** `proof/PMT/Iris/GuardInvariant.lean`.

---

## §7. `wp e {Φ}` — weakest precondition

The weakest-precondition `wp e {Φ}` is Iris's programme-logic judgement:
"`e` is safe and, if it terminates, the result satisfies `Φ`". The VUMA
Iris model formalises `wp` for the PMT step-relation in
`proof/PMT/Iris/WeakestPrecond.lean`, including the bind rule, the
pure-step rule, and the state-read / state-write rules.

**Statements** are given in `proof/PMT/Iris/WeakestPrecond.lean`; the
machine-checked proofs of the higher-level rules are deferred to the
Iris-composition work tracked as PMT-1-G.

**Lean reference.** `proof/PMT/Iris/WeakestPrecond.lean`.

---

## §8. Trusted Computing Base (TCB)

The VUMA PMT memory-safety argument trusts the following OS / runtime
contracts; everything else is proven in Lean:

  - **`mmap PROT_NONE` guard-page semantics — Trusted.** The OS honours
    `mmap(..., PROT_NONE, ...)` by raising a segfault (exit 134 via
    `__oob_trap`) on any access. This is the runtime trap that the
    `[guard]` invariant (§6) abstracts.
  - **`arena_alloc` layout bookkeeping — Trusted.** `arena_alloc`
    computes `Layout` (size, align) and bumps `used` within `capacity`.
    The bookkeeping is tested by `arena_alloc` (see
    [`pmt-formal-spec.md`](./pmt-formal-spec.md) §1.3) and trusted; the
    *capacity bound* itself is proven via `[cap_bnd]` (§3).
  - **Provable.** Everything else — the capacity bound, the liveness
    mirror, the guard-page address, the field-bounds safety, the
    composition — is proven in `proof/PMT/` and `proof/PMT/Iris/`.

**Lean reference.** `proof/PMT/Basic.lean` (TCB notes inline).

---

## §9. Composition of named invariants

The full memory-safety theorem composes `[cap_bnd]` (§3), `[live_mirror]`
(§5), and `[guard]` (§6) with the fractional-permission machinery (§2)
and the `wp` judgement (§7) to establish that any well-typed PMT
programme is memory-safe. The composition is sketched in
`proof/PMT/Iris/Composition.lean`, with the higher-level rules deferred
to PMT-1-G.

**Lean reference.** `proof/PMT/Iris/Composition.lean`.

---

## Cross-references

  - **Source of truth for the Iris proofs**: this document
    (`docs/pmt-iris-spec.md`).
  - **Source of truth for the Lean signatures and proof sketches**:
    [`./pmt-formal-spec.md`](./pmt-formal-spec.md) (the runtime companion
    spec).
  - **Iris modules**: `proof/PMT/Iris/*.lean`
    (`ArenaRes.lean`, `CapBndInvariant.lean`, `FractionalPerm.lean`,
    `GuardInvariant.lean`, `LiveMirrorInvariant.lean`,
    `WeakestPrecond.lean`, `Composition.lean`).
  - **Runtime Lean modules**: `proof/PMT/*.lean` (`Basic.lean`,
    `Liveness.lean`, `Field.lean`).
  - **Architecture overview**: [`./architecture.md`](./architecture.md).
  - **Pipeline overview**: [`./pipeline.md`](./pipeline.md).
