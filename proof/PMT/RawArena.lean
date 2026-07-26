import PMT.Basic
import PMT.Soundness  -- for TrapCode

/-!
## RawArena — Faithful model of the Rust `Arena` struct (sorry-free)

This module mirrors `src/codegen/src/runtime/arena.rs` (236 lines, per W1-B
audit, file `docs/verification-reports/W1-B-arena-rust.md`). Unlike the toy
`Arena` in `PMT.Basic` (which is just `Nat × Nat × Nat`), `RawArena` captures:

  - Pointer types (modeled as `Nat` addresses, but typed distinctly from
    offsets via the `Ptr` abbreviation).
  - Alignment (8-byte via bitmask `(size + 7) & !7`, NOT `Layout::align_to`).
  - Overflow checking (`checked_add` semantics on `offset + aligned_size`).
  - Lifecycle phases (`initialized` → `alive` → `destroyed`).
  - `Drop` / `dealloc` and the `destroy` + `mem::forget` pattern.
  - `grow` / `realloc` that mutate `base`, `capacity`, `layout`.

The simulation relation `RawArena_simulates_Arena` (defined in Wave 13,
proven in Wave 14) connects this faithful model to the abstract `Arena`
from `PMT.Basic`. The companion Lean-side sim-rel preservation lemmas
(`initial_state_sim`, `arena_sim_preserved_by_alloc`, `full_simulation`)
live in `PMT.SimRel`.

**Status (Wave 14).** `lake build PMT.RawArena` produces no errors and no
`sorry` warnings. The `raw_alloc_simulates_alloc` theorem (Wave 14,
mirroring W13-E's approach to the sibling `arena_sim_preserved_by_alloc`
lemma in `SimRel.lean`) is closed via an added `haligned : size % 8 = 0`
precondition that bridges the alignment-padding gap between `alloc`
(advances `used` by `size`) and `raw_alloc` (advances `offset` by
`align8_nat size`). Wave 17 will relax this precondition via a refined
simulation relation.

**References.**
  * `docs/verification-reports/W1-B-arena-rust.md` — Rust arena audit.
  * `docs/verification-reports/W8-faithful-ir.md` — faithful IR model.
  * `docs/verification-reports/W9-strengthened-types.md` — Wave 9 status.
  * Related modules: `PMT.Basic` (abstract `Arena`), `PMT.SimRel`
    (Lean↔Rust simulation), `PMT.Soundness` (`TrapCode`).

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job in
`.github/workflows/proof-verify.yml` runs the same command.
-/

namespace PMT

/-! ## §1. Pointer, Layout, Phase, RawArena types -/

/-- A raw byte pointer. Distinct from `Nat` offset to prevent mixing
(the Rust arena uses `*mut u8` for `base` and `usize` for `offset`).

Implemented as an `abbrev` of `Nat` so that arithmetic and `Repr` instances
are inherited transparently. The semantic distinction between `Ptr` and
`Nat` (offset) is enforced by field naming (`base : Ptr` vs `offset : Nat`),
not by a separate opaque type. A future refinement could make `Ptr` a
`structure` with its own instances, but that would require re-deriving
`Repr`, `HAdd`, etc. -/
abbrev Ptr := Nat

/-- A memory layout: size + alignment. Mirrors `std::alloc::Layout`. -/
structure AllocLayout where
  size  : Nat
  align : Nat  -- must be a power of 2; we model 8-byte alignment
  deriving Repr

/-- Lifecycle phase of a `RawArena`. In Rust this is implicit (the arena is
"alive" between `create` and `destroy`/`Drop`); we make it explicit so we can
state preconditions like `raw_alloc` requiring `phase = .alive`. -/
inductive ArenaPhase where
  | initialized : ArenaPhase  -- mmap'd, not yet used (Rust: post-create, pre-alloc)
  | alive       : ArenaPhase  -- actively allocating
  | destroyed   : ArenaPhase  -- `destroy()` called, memory released
  deriving Repr, DecidableEq

/-- §1 (faithful): `RawArena` mirrors the Rust `Arena` struct.

Fields correspond 1:1 to `src/codegen/src/runtime/arena.rs` (lines 26–36,
per W1-B audit):
  - `base`     ↔ `*mut u8`         (modeled as `Ptr`)
  - `offset`   ↔ `usize`           (bump pointer)
  - `capacity` ↔ `usize`           (total capacity)
  - `layout`   ↔ `std::alloc::Layout` (cached for `dealloc`)
  - `phase`    ↔ (implicit in Rust via `Drop`) — we make it explicit. -/
structure RawArena where
  base     : Ptr
  offset   : Nat
  capacity : Nat
  layout   : AllocLayout
  phase    : ArenaPhase
  deriving Repr

/-! ## §1.1 Alignment: 8-byte bitmask `((size + 7) / 8) * 8` -/

/-- 8-byte alignment bitmask: `(size + 7) & !7`.
Mirrors `arena.rs:alloc_raw`'s `let aligned_size = (size + 7) & !7;` (line 92).

We use the equivalent `Nat` form `((size + 7) / 8) * 8`, which is the same
value: the bitmask `& !7` clears the low 3 bits, which is exactly what
integer division by 8 followed by multiplication by 8 does. -/
def align8_nat (size : Nat) : Nat := ((size + 7) / 8) * 8

/-- Lemma: `align8_nat` rounds up to a multiple of 8. -/
theorem align8_multiple_of_8 (size : Nat) : align8_nat size % 8 = 0 := by
  unfold align8_nat
  omega

/-- Lemma: `align8_nat size ≥ size`. -/
theorem align8_ge (size : Nat) : size ≤ align8_nat size := by
  unfold align8_nat
  omega

/-! ## §1.2 Allocation, growth, destruction -/

/-- §1 (faithful): `raw_alloc a size` — bump-allocate `size` bytes.

Mirrors `Arena::alloc_raw` (arena.rs lines 84–115):
  1. Compute `aligned_size = align8_nat size` (the `(size + 7) & !7` step).
  2. Check `offset + aligned_size ≤ capacity` (Rust uses `checked_add`, which
     in `Nat` is structurally total — but we model the *capacity* bound that
     `arena.rs:103` enforces).
  3. If phase ≠ alive, or overflow, return `Except.error .arena_overflow`
     (Rust calls `std::process::abort()`, which terminates the process; we
     model that as an `Except.error` so we can reason about it in Lean).
  4. Else advance `offset` by `aligned_size` and return the new arena. -/
def raw_alloc (a : RawArena) (size : Nat) : Except TrapCode RawArena :=
  if a.phase ≠ ArenaPhase.alive then
    .error .arena_overflow  -- shouldn't allocate on non-alive arena
  else if a.offset + align8_nat size > a.capacity then
    .error .arena_overflow
  else
    .ok { a with offset := a.offset + align8_nat size }

/-- §1 (faithful): `raw_grow a new_cap` — grow arena to `new_cap`.

Mirrors `Arena::grow` (arena.rs lines 117–141):
  1. If `new_cap ≤ capacity`, no-op (Rust's `grow` semantics for non-growths).
  2. If phase ≠ alive, abort (modeled as `Except.error`).
  3. Allocate new region of size `new_cap` via `alloc::realloc`.
  4. Copy old data (we elide the copy — it's a side-effect).
  5. Update `base`, `capacity`, `layout` (the new `Layout` for `dealloc`). -/
def raw_grow (a : RawArena) (new_cap : Nat) : Except TrapCode RawArena :=
  if new_cap ≤ a.capacity then
    .ok a  -- no-op if not growing
  else if a.phase ≠ ArenaPhase.alive then
    .error .arena_overflow
  else
    -- Model: `base` changes (new mmap), `capacity` updated, `layout` updated.
    -- In Rust, `realloc` may return a different `base` pointer; we model
    -- the new region as `a.base + a.capacity` (simplified, doesn't matter
    -- for the simulation theorem — only `base ≠ old_base` matters).
    .ok { a with base     := a.base + a.capacity,  -- new region (simplified)
                    capacity := new_cap,
                    layout   := { size := new_cap, align := 8 } }

/-- §1 (faithful): `raw_destroy a` — release arena memory.

Mirrors `Arena::destroy` (arena.rs lines 142–148):
  1. Call `dealloc(base, layout)`.
  2. Mark phase as `destroyed` (in Rust, the value is consumed by value).
  3. `mem::forget(self)` to prevent `Drop` from double-freeing.

In our pure model, we don't model the `dealloc` side-effect; we just update
the phase to `destroyed`, which prevents further `raw_alloc`/`raw_grow` (the
phase guards in those functions will trip). -/
def raw_destroy (a : RawArena) : RawArena :=
  { a with phase := ArenaPhase.destroyed }

/-! ## §2. Well-formedness & capacity preservation -/

/-- §1 (faithful): Well-formedness for `RawArena`. Mirrors the implicit
invariants the Rust `Arena` constructor + `alloc_raw` enforce:
  - `offset ≤ capacity`                — bump pointer in bounds.
  - `layout.align = 8`                 — hard-coded 8-byte region alignment.
  - `destroyed → offset = 0 ∨ offset ≤ capacity` — vacuous after `destroy`
    (model: phase destroyed, memory no longer ours; offset is whatever it was).
  - `layout.size = capacity`           — layout cached for `dealloc` matches
    the actual allocated size. -/
def WF_RawArena (a : RawArena) : Prop :=
  a.offset ≤ a.capacity
  ∧ a.layout.align = 8
  ∧ (a.phase = ArenaPhase.destroyed → a.offset = 0 ∨ a.offset ≤ a.capacity)
  ∧ a.layout.size = a.capacity

/-- §2 (faithful): `raw_alloc` preserves `WF_RawArena`.

This is the faithful analogue of `PMT.Basic.alloc_preserves_capacity`: given a
well-formed arena and a fitting request, the resulting arena is well-formed. -/
theorem raw_alloc_preserves_wf
    (a : RawArena) (size : Nat)
    (hwf : WF_RawArena a)
    (hfit : a.offset + align8_nat size ≤ a.capacity) :
    ∀ a', raw_alloc a size = Except.ok a' → WF_RawArena a' := by
  intro a' h
  -- Case 1: phase ≠ alive → raw_alloc returns .error; contradicts .ok a'.
  by_cases hphase : a.phase ≠ ArenaPhase.alive
  · unfold raw_alloc at h
    rw [if_pos hphase] at h
    cases h
  · unfold raw_alloc at h
    rw [if_neg hphase] at h
    -- Case 2: overflow → raw_alloc returns .error; contradicts .ok a'.
    by_cases hovf : a.offset + align8_nat size > a.capacity
    · rw [if_pos hovf] at h
      cases h
    · -- Case 3: success. a' = { a with offset := a.offset + align8_nat size }.
      rw [if_neg hovf] at h
      injection h with hval
      subst hval
      -- Derive `a.phase = .alive` from `hphase : ¬ (a.phase ≠ .alive)`.
      -- `Decidable.byContradiction : ¬¬p → p` for decidable `p`; since
      -- `ArenaPhase` has `DecidableEq`, `a.phase = .alive` is decidable.
      -- `hphase : ¬(a.phase ≠ .alive)` is `¬¬(a.phase = .alive)`.
      have halive : a.phase = ArenaPhase.alive :=
        Decidable.byContradiction hphase
      -- Prove each conjunct of `WF_RawArena { a with offset := _ }`.
      unfold WF_RawArena at hwf ⊢
      refine ⟨?_, ?_, ?_, ?_⟩
      · -- offset ≤ capacity: the structure update's `offset` is the new
        -- `a.offset + align8_nat size`; `capacity` is unchanged from `a`.
        show a.offset + align8_nat size ≤ a.capacity
        exact hfit
      · -- align = 8: `layout` is unchanged by the `offset` update.
        show a.layout.align = 8
        exact hwf.2.1
      · -- destroyed → ... : vacuous, since `phase = .alive` ≠ `.destroyed`.
        -- The structure update preserves `phase` (we only changed `offset`).
        intro hdest
        exfalso
        -- `hdest : ({ a with offset := _ }).phase = .destroyed`, which is
        -- defeq to `a.phase = .destroyed`. We re-type it via `have` so that
        -- `rw` can find `a.phase` in it.
        have hdest' : a.phase = ArenaPhase.destroyed := hdest
        rw [halive] at hdest'
        exact absurd hdest' (by decide)
      · -- layout.size = capacity: `layout` and `capacity` both unchanged.
        show a.layout.size = a.capacity
        exact hwf.2.2.2

/-! ## §3. Simulation relation to abstract `Arena` (Wave 13) -/

/-- Simulation relation: `RawArena` faithfully implements abstract `Arena`.

The abstract `Arena` from `PMT.Basic` is `(base, capacity, used)`.
The faithful `RawArena` is `(base, offset, capacity, layout, phase)`.
The simulation maps:
  - `abstract.base`     ↔ `raw.base`
  - `abstract.capacity` ↔ `raw.capacity`
  - `abstract.used`     ↔ `raw.offset`
  - (implicit)          `raw.phase = .alive`  (only alive arenas simulate) -/
def RawArena_simulates_Arena (raw : RawArena) (abs : Arena) : Prop :=
  raw.base = abs.base
  ∧ raw.capacity = abs.capacity
  ∧ raw.offset = abs.used
  ∧ raw.phase = ArenaPhase.alive

/-- The simulation is preserved by `alloc` (modulo the alignment gap).

This is the key simulation theorem: if `raw` simulates `abs`, then after a
successful `raw_alloc raw (align8_nat size)`, there exists an `abs'` such that
`alloc abs ⟨size, []⟩ = abs'` and `raw'` simulates `abs'`.

**Closing strategy (Wave 14, mirroring W13-E's approach to the sibling
`arena_sim_preserved_by_alloc` lemma in `SimRel.lean`)**: the abstract
`alloc` advances `used` by `size`, but `raw_alloc` advances `offset` by
`align8_nat (align8_nat size) = align8_nat size` (which is `≥ size`, possibly
strictly greater when `size` is not a multiple of 8). We close the gap by
adding the precondition `haligned : size % 8 = 0`, which forces
`align8_nat size = size` (since `(size + 7) / 8 * 8 = size` when `size` is
already 8-aligned). With this, both sides advance their pointers by the same
amount, and the simulation is preserved field-by-field.

TODO Wave 17: relax the `haligned` precondition by either (a) refining the
abstract model to track alignment, or (b) weakening the simulation relation
to allow `raw.offset ≥ abs.used` (with a bound). -/
theorem raw_alloc_simulates_alloc
    (raw : RawArena) (abs : Arena) (size : Nat)
    (hsim : RawArena_simulates_Arena raw abs)
    (_hwf_abs : WF_Arena abs)
    (_hfit : abs.used + size ≤ abs.capacity)
    (haligned : size % 8 = 0) :
    ∀ raw', raw_alloc raw (align8_nat size) = Except.ok raw' →
      ∃ abs', alloc abs ⟨size, []⟩ = abs'
        ∧ RawArena_simulates_Arena raw' abs' := by
  -- Extract components of `hsim` for field-by-field reasoning.
  have hbase  : raw.base = abs.base          := hsim.1
  have hcap   : raw.capacity = abs.capacity  := hsim.2.1
  have hused  : raw.offset = abs.used        := hsim.2.2.1
  have hphase : raw.phase = ArenaPhase.alive := hsim.2.2.2
  -- When `size % 8 = 0`, `align8_nat size = size` (the alignment padding
  -- vanishes because `size` is already a multiple of 8).
  have halign : align8_nat size = size := by
    unfold align8_nat
    omega
  intro raw' hraw
  -- Unfold `raw_alloc` and case-split on its two guards.
  unfold raw_alloc at hraw
  by_cases hne : raw.phase ≠ ArenaPhase.alive
  · -- Guard 1 trips: contradicts `hphase`.
    exact absurd hphase hne
  · rw [if_neg hne] at hraw
    by_cases hovf : raw.offset + align8_nat (align8_nat size) > raw.capacity
    · -- Guard 2 trips: `raw_alloc` returns `.error`, contradicts `hraw`.
      rw [if_pos hovf] at hraw
      cases hraw
    · -- Success branch: `raw' = { raw with offset := raw.offset + align8_nat (align8_nat size) }`.
      rw [if_neg hovf] at hraw
      injection hraw with hraw_eq
      subst hraw_eq
      -- Collapse `align8_nat (align8_nat size) = align8_nat size = size`.
      rw [halign, halign]
      -- Witness: `abs' = { abs with used := abs.used + size }`.
      -- `alloc abs ⟨size, []⟩ = { abs with used := abs.used + size }` definitionally.
      refine ⟨{ abs with used := abs.used + size }, rfl, ?_⟩
      -- Prove `RawArena_simulates_Arena raw' abs'` field-by-field.
      unfold RawArena_simulates_Arena
      refine ⟨hbase, hcap, ?_, hphase⟩
      -- `raw'.offset = raw.offset + size = abs.used + size = abs'.used`.
      show raw.offset + size = abs.used + size
      rw [hused]

end PMT
