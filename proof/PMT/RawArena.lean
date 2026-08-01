import PMT.Basic
import PMT.Soundness  -- for TrapCode

/-!
## RawArena — Faithful model of the Rust `Arena` struct (sorry-free)

This module mirrors `src/codegen/src/runtime/arena.rs` (554 lines, PMT-owned
Rust file). Unlike the toy `Arena` in `PMT.Basic` (which is just
`Nat × Nat × Nat`), `RawArena` captures:

  - Pointer types (modeled as `Nat` addresses, but typed distinctly from
    offsets via the `Ptr` abbreviation).
  - `ThreadId` of the constructing thread (PMT-1-F gap #1 — Rust field
    `created_thread: ThreadId` at arena.rs:81 was missing in Lean).
  - Alignment (8-byte via bitmask `(size + 7) & !7`, NOT `Layout::align_to`).
  - Overflow checking (`checked_add` semantics on `offset + aligned_size`).
  - Lifecycle phases (`initialized` → `alive` → `destroyed`).
  - `Drop` / `dealloc` and the `destroy` + `mem::forget` pattern
    (PMT-1-F gap #9 — modeled via `raw_destroy` / `raw_drop` /
    `raw_panic_drop`).
  - `grow` / `realloc` that mutate `base`, `capacity`, `layout`
    (PMT-1-F gap #6 — `raw_grow_nondet` models realloc-may-relocate).
  - `create` constructor with `Layout::from_size_align` failure path
    (PMT-1-F gap #5 — `raw_create` with corrected threshold matching
    Rust's `layout_for`).
  - Arithmetic-overflow path distinct from capacity-overflow path
    (PMT-1-F gap #8 — `raw_alloc_with_overflow` models BOTH paths).

The simulation relation `RawArena_simulates_Arena` (faithful — captures
all Rust state incl. layout, phase, thread, usize bounds) connects this
faithful model to the abstract `Arena` from `PMT.Basic`. The companion
Lean-side sim-rel preservation lemmas (`initial_state_sim`,
`arena_sim_preserved_by_alloc`, `full_simulation`) live in `PMT.SimRel`.

### PMT-1-F: 9 RawArena↔Rust gaps closed

This file closes the following gaps (per the PMT-1-F task brief):

  1. **`ThreadId` field missing in Lean** — added `created_thread : ThreadId`
     field to `RawArena`, mirroring Rust `created_thread: ThreadId`
     (arena.rs:81). Modeled as `Nat` (Rust `ThreadId` is opaque but
     uniquely identifies a thread).
  2. **`base`/`offset`/`capacity` type mismatches** — Rust uses `usize`
     (64-bit on most platforms). We keep `Nat` for decidable arithmetic
     but add explicit `usize` bounds (`< 2^64`) in the faithful
     well-formedness predicate `WF_RawArena_faithful` and in the
     simulation relations (`RawArena_simulates_Arena`, `arena_sim`).
     This makes the Lean model faithful to Rust's `usize` without
     sacrificing `omega`-based proof automation.
  3. **Layout type mismatch** — Rust uses `std::alloc::Layout`; Lean
     uses `AllocLayout` (size + align). The simulation relations now
     require `layout.align = 8 ∧ layout.size = capacity`, matching
     Rust's `Layout::from_size_align(capacity, 8)` (arena.rs:117-121).
  4. **`phase` field Lean-only** — Rust's lifecycle is implicit (via
     `Drop`); Lean's `phase` field makes it explicit. Documented as a
     Lean-side abstraction (not a divergence — the field captures real
     Rust state that's implicit in the source).
  5. **`raw_create` threshold mismatch** — corrected threshold from
     `4 GiB` (conservative under-approximation) to `> 2^63 - 1`
     (= `isize::MAX` on 64-bit), matching Rust's
     `Layout::from_size_align(capacity, 8)` failure condition
     (arena.rs:117-121, `layout_for`).
  6. **`raw_grow` relocation mismatch** — added `raw_grow_nondet` that
     takes `new_base` as a parameter, modeling the allocator's
     nondeterministic choice (Rust `alloc::realloc` may return a
     different pointer). The existing `raw_grow` (deterministic) is
     shown to be a special case via `raw_grow_eq_raw_grow_nondet`.
  7. **Alignment precondition undischarged (`haligned`)** — discharged
     in `PMT.SimRel` by introducing `aligned_alloc` (abstract alloc that
     advances `used` by `align8_nat total_size`) and rewriting
     `arena_sim_preserved_by_alloc` to use `aligned_alloc` instead of
     `alloc`, eliminating the `haligned : size % 8 = 0` precondition.
  8. **Overflow path mismatch** — added `raw_alloc_with_overflow` with
     BOTH overflow paths (arithmetic via `≥ 2^64` check, capacity via
     `> capacity` check), mirroring Rust's `checked_add` + `> capacity`
     pair (arena.rs:220-234). Proven equivalent to `raw_alloc` under
     `WF_RawArena_faithful` (the arithmetic path is vacuous when
     `offset, capacity < 2^64`).
  9. **Drop/panic modeling** — added `raw_drop` (models `Drop::drop`
     at arena.rs:329-342) and `raw_panic_drop` (models `Drop` during
     panic — `dealloc` runs but `assert_owner_thread` is skipped per
     arena.rs:337-339). Both transition to `.destroyed`, matching
     `raw_destroy`. Proven equal to `raw_destroy` via
     `raw_drop_eq_raw_destroy`, `raw_panic_drop_eq_raw_destroy`.

**Status.** `lake build PMT.RawArena` produces no errors and no
`sorry` warnings. All new theorems are closed via `omega`, `rfl`, or
direct unfolding — no sorries, no axioms.

**References.**
  * Rust source: `src/codegen/src/runtime/arena.rs` (PMT-owned).
  * Related modules: `PMT.Basic` (abstract `Arena`), `PMT.SimRel`
    (Lean↔Rust simulation), `PMT.Soundness` (`TrapCode`),
    `PMT.BitVecArena` (BitVec-based companion model, unified via
    `bitvec_arena_equiv_raw_arena`), `PMT.MmapArena` (composition
    layer that imports `raw_create` from this module).

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job in
`.github/workflows/proof-verify.yml` runs the same command.
-/

namespace PMT

/-! ## §1. Pointer, Layout, Phase, ThreadId, RawArena types -/

/-- A raw byte pointer. Distinct from `Nat` offset to prevent mixing
(the Rust arena uses `*mut u8` for `base` and `usize` for `offset`).

Implemented as an `abbrev` of `Nat` so that arithmetic and `Repr` instances
are inherited transparently. The semantic distinction between `Ptr` and
`Nat` (offset) is enforced by field naming (`base : Ptr` vs `offset : Nat`),
not by a separate opaque type. A future refinement could make `Ptr` a
`structure` with its own instances, but that would require re-deriving
`Repr`, `HAdd`, etc. -/
abbrev Ptr := Nat

/-- **PMT-1-F gap #1.** Thread identifier — mirrors Rust
`std::thread::ThreadId` (arena.rs:81, `created_thread` field).

Rust's `ThreadId` is opaque (a wrapped `u64`); we model it as `Nat`. The
`Arena::create` constructor (arena.rs:161) sets `created_thread` to
`std::thread::current().id()`, which is always non-null in practice. We
adopt the convention that `created_thread = 0` means "uninitialized"
(never properly created) and `created_thread > 0` means "properly
created by a real thread" — this is captured in the faithful
well-formedness predicate `WF_RawArena_faithful` and in the simulation
relations. -/
abbrev ThreadId := Nat

/-- A memory layout: size + alignment. Mirrors `std::alloc::Layout`.

**PMT-1-F gap #3.** The Rust arena uses `Layout::from_size_align(capacity, 8)`
(arena.rs:117-121, `layout_for`); the Lean `AllocLayout` captures the
same two fields (`size`, `align`). The simulation relations
(`RawArena_simulates_Arena`, `arena_sim`) require `layout.align = 8`
and `layout.size = capacity`, matching the Rust invariant. -/
structure AllocLayout where
  size  : Nat
  align : Nat  -- must be a power of 2; we model 8-byte alignment
  deriving Repr

/-- Lifecycle phase of a `RawArena`. In Rust this is implicit (the arena is
"alive" between `create` and `destroy`/`Drop`); we make it explicit so we can
state preconditions like `raw_alloc` requiring `phase = .alive`.

**PMT-1-F gap #4.** The Rust `Arena` struct (arena.rs:68-82) does NOT
have an explicit `phase` field — the lifecycle is implicit (the arena
is "alive" between `Arena::create` and `Arena::destroy`/`Drop`). The
Lean `phase` field is an EXPLICIT MODELING of this implicit Rust state,
not a divergence. The three phases map to Rust states as follows:
  - `initialized` — post-`Arena::create`, pre-`alloc_raw` (Rust has no
    distinct state here; modeled for proof ergonomics).
  - `alive` — actively allocating (the normal Rust state).
  - `destroyed` — post-`Arena::destroy` or post-`Drop` (in Rust, the
    value is consumed; in Lean, we keep it around with `phase = .destroyed`
    so we can state "no further `raw_alloc`/`raw_grow`"). -/
inductive ArenaPhase where
  | initialized : ArenaPhase  -- mmap'd, not yet used (Rust: post-create, pre-alloc)
  | alive       : ArenaPhase  -- actively allocating
  | destroyed   : ArenaPhase  -- `destroy()` called, memory released
  deriving Repr, DecidableEq

/-- §1 (faithful): `RawArena` mirrors the Rust `Arena` struct.

Fields correspond 1:1 to `src/codegen/src/runtime/arena.rs` (lines 68–82):
  - `base`            ↔ `base: *mut u8`            (modeled as `Ptr`)
  - `offset`          ↔ `offset: usize`            (Nat, bounded `< 2^64`)
  - `capacity`        ↔ `capacity: usize`          (Nat, bounded `< 2^64`)
  - `layout`          ↔ `layout: Layout`           (`AllocLayout`)
  - `created_thread`  ↔ `created_thread: ThreadId` (PMT-1-F gap #1, NEW)
  - `phase`           ↔ (implicit in Rust via `Drop`) — we make it explicit.

**PMT-1-F gap #1.** The `created_thread : ThreadId` field is NEW in this
revision — it mirrors Rust's `created_thread: ThreadId` (arena.rs:81),
which was missing in the prior Lean model. The Rust field is used by
`assert_owner_thread` (arena.rs:132-138) to enforce the single-thread
invariant in debug builds.

**PMT-1-F gap #2.** Rust uses `usize` (64-bit on most platforms) for
`offset` and `capacity`. We keep `Nat` (for decidable arithmetic via
`omega`) but add explicit `< 2^64` bounds in the faithful
well-formedness predicate `WF_RawArena_faithful` and in the simulation
relations. This makes the Lean model faithful to Rust's `usize` without
sacrificing proof automation. -/
structure RawArena where
  base           : Ptr
  offset         : Nat
  capacity       : Nat
  layout         : AllocLayout
  phase          : ArenaPhase
  created_thread : ThreadId  -- PMT-1-F gap #1 (NEW)
  deriving Repr

/-! ## §1.0 usize bounds (PMT-1-F gap #2) -/

/-- The bit-width of `usize` on 64-bit platforms. Rust's `usize` is
platform-dependent (32-bit on some targets); the VUMA arena runtime is
documented as 64-bit-biased (see arena.rs:206-211), so we fix `64`. -/
def USIZE_BITS : Nat := 64

/-- `2^64`, the exclusive upper bound for `usize` values. Rust's
`usize::MAX = 2^64 - 1`; we use `2^64` as the bound (i.e., `x < 2^64`)
so that `offset + aligned_size ≥ 2^64` is the arithmetic-overflow
condition (mirroring `usize::checked_add` returning `None`). -/
def USIZE_BOUND : Nat := 2^USIZE_BITS  -- = 2^64

/-- `isize::MAX = 2^63 - 1` on 64-bit. Rust's `Layout::from_size_align`
returns `Err` when `size > isize::MAX` (Rust stdlib invariant: the
allocated size must fit in `isize` so that pointer arithmetic is sound).
This is the threshold for `raw_create` to trap (PMT-1-F gap #5). -/
def ISIZE_MAX : Nat := 2^(USIZE_BITS - 1) - 1  -- = 2^63 - 1

/-! ## §1.1 Alignment: 8-byte bitmask `((size + 7) / 8) * 8` -/

/-- 8-byte alignment bitmask: `(size + 7) & !7`.
Mirrors `arena.rs:alloc_raw`'s `let aligned_size = (size + 7) & !7;` (line 176).

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

/-! ## §1.2 `raw_create` — the mmap/alloc constructor (PMT-1-F gap #5) -/

/-- §1.2 (faithful): `raw_create capacity thread` — allocate a new arena.

Mirrors Rust `Arena::create` (arena.rs:147-163), which calls
`layout_for(capacity)` (= `Layout::from_size_align(capacity, 8)`,
arena.rs:117-121) and traps if the layout is invalid (i.e., if
`capacity > isize::MAX = 2^63 - 1`).

**PMT-1-F gap #5 (corrected threshold).** The prior `raw_create` (in
`PMT.MmapArena`) used a `4 GiB` threshold as a conservative
under-approximation of OOM. This was a mismatch with Rust, which traps
at `Layout::from_size_align` failure (i.e., `capacity > 2^63 - 1`),
NOT at 4 GiB. This `raw_create` uses the CORRECT threshold
`capacity > ISIZE_MAX = 2^63 - 1`, matching Rust's `layout_for` exactly.

For successful allocations, the resulting arena mirrors the field
initialization that Rust's `Arena::create` performs (arena.rs:156-162):
  - `base`            — fresh pointer from `alloc::alloc` (placeholder `1000`).
  - `offset`          — initial bump pointer, `0`.
  - `capacity`        — the requested capacity.
  - `layout`          — `Layout::from_size_align(capacity, 8)`.
  - `phase`           — `.alive` (the arena is immediately usable).
  - `created_thread`  — the supplied `thread` (Rust: `std::thread::current().id()`).

The `thread` parameter defaults to `1` (a non-zero `ThreadId`, modeling
"the main thread"); callers can supply a different thread ID to model
multi-threaded construction. -/
def raw_create (capacity : Nat) (thread : ThreadId := 1) : Except TrapCode RawArena :=
  -- PMT-1-F gap #5: corrected threshold matching Rust's layout_for failure.
  -- Rust: `Layout::from_size_align(capacity, 8).unwrap_or_else(|_| trap)`
  -- traps when `capacity > isize::MAX = 2^63 - 1` on 64-bit.
  if capacity > ISIZE_MAX then
    .error .arena_overflow  -- allocator failure modeled as arena_overflow
  else
    .ok { base           := 1000,  -- placeholder base address (fresh alloc)
          offset         := 0,
          capacity       := capacity,
          layout         := { size := capacity, align := 8 },
          phase          := .alive,
          created_thread := thread }

/-! ## §1.3 `raw_alloc` — bump-allocate (with separate overflow-path model) -/

/-- §1.3 (faithful): `raw_alloc a size` — bump-allocate `size` bytes.

Mirrors `Arena::alloc_raw` (arena.rs:174-238):
  1. Compute `aligned_size = align8_nat size` (the `(size + 7) & !7` step).
  2. Check `offset + aligned_size ≤ capacity` (Rust uses `checked_add`, which
     in `Nat` is structurally total — but we model the *capacity* bound that
     `arena.rs:229` enforces).
  3. If phase ≠ alive, or overflow, return `Except.error .arena_overflow`
     (Rust calls `arena_overflow_trap` → `std::process::exit(1)`, which
     terminates the process; we model that as an `Except.error` so we can
     reason about it in Lean).
  4. Else advance `offset` by `aligned_size` and return the new arena.

**Note on the arithmetic-overflow path (PMT-1-F gap #8).** Rust's
`alloc_raw` has TWO distinct trap paths:
  1. Arithmetic overflow: `self.offset.checked_add(aligned_size)` returns
     `None` (arena.rs:220-228).
  2. Capacity overflow: `new_offset > self.capacity` (arena.rs:229-234).

This `raw_alloc` function models ONLY path 2 (capacity overflow), since
`Nat` is unbounded and path 1 is structurally impossible. The faithful
`raw_alloc_with_overflow` function below models BOTH paths. Under
`WF_RawArena_faithful` (which bounds `offset, capacity < 2^64`), the two
functions are equivalent (path 1 is vacuous). -/
def raw_alloc (a : RawArena) (size : Nat) : Except TrapCode RawArena :=
  if a.phase ≠ ArenaPhase.alive then
    .error .arena_overflow  -- shouldn't allocate on non-alive arena
  else if a.offset + align8_nat size > a.capacity then
    .error .arena_overflow
  else
    .ok { a with offset := a.offset + align8_nat size }

/-- §1.3 (faithful): `raw_alloc_with_overflow` — bump-allocate with BOTH
overflow paths modeled (PMT-1-F gap #8).

Mirrors Rust `Arena::alloc_raw` (arena.rs:174-238) which has TWO distinct
trap paths:
  1. **Arithmetic overflow**: `self.offset.checked_add(aligned_size)` returns
     `None` → `arena_overflow_trap` (arena.rs:220-228). Modeled here as
     `a.offset + align8_nat size ≥ USIZE_BOUND` (= `≥ 2^64`).
  2. **Capacity overflow**: `new_offset > self.capacity` →
     `arena_overflow_trap` (arena.rs:229-234). Modeled here as
     `a.offset + align8_nat size > a.capacity`.

Both paths route to `Except.error TrapCode.arena_overflow`, matching the
Rust binary's single `__arena_overflow` exit (exit code 1).

Under `WF_RawArena_faithful` (which bounds `offset, capacity < 2^64`), path
1 is vacuous (the sum `offset + aligned_size` cannot reach `2^64` when
`offset < 2^64` and the capacity guard fires first), so
`raw_alloc_with_overflow = raw_alloc`. See
`raw_alloc_with_overflow_eq_raw_alloc_under_wf` below. -/
def raw_alloc_with_overflow (a : RawArena) (size : Nat) : Except TrapCode RawArena :=
  if a.phase ≠ ArenaPhase.alive then
    .error .arena_overflow  -- phase guard (Rust: implicit; we model explicitly)
  else if a.offset + align8_nat size ≥ USIZE_BOUND then
    .error .arena_overflow  -- path 1: arithmetic overflow (checked_add = None)
  else if a.offset + align8_nat size > a.capacity then
    .error .arena_overflow  -- path 2: capacity overflow (new_offset > capacity)
  else
    .ok { a with offset := a.offset + align8_nat size }

/-! ## §1.4 `raw_grow` — grow the arena (with realloc-may-relocate model) -/

/-- §1.4 (faithful): `raw_grow a new_cap` — grow arena to `new_cap`.

Mirrors `Arena::grow` (arena.rs:253-271):
  1. If `new_cap ≤ capacity`, no-op (Rust's `grow` semantics for non-growths).
  2. If phase ≠ alive, abort (modeled as `Except.error`).
  3. Allocate new region of size `new_cap` via `alloc::realloc`.
  4. Copy old data (we elide the copy — it's a side-effect).
  5. Update `base`, `capacity`, `layout` (the new `Layout` for `dealloc`).

**PMT-1-F gap #6 (deterministic simplification).** This `raw_grow` sets
`base := a.base + a.capacity` deterministically — modeling ONE possible
relocation (the allocator places the new region right after the old one).
The Rust `alloc::realloc` is NONDETERMINISTIC: it may return the same
pointer (in-place growth) or a different pointer (relocation). The
faithful `raw_grow_nondet` function below takes `new_base` as a
parameter, modeling the allocator's nondeterministic choice. The
existing `raw_grow` is the special case `new_base = a.base + a.capacity`
(see `raw_grow_eq_raw_grow_nondet`). -/
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

/-- §1.4 (faithful): `raw_grow_nondet a new_cap new_base` — grow arena
with EXTERNALLY-SUPPLIED new base (PMT-1-F gap #6).

This is the FAITHFUL model of Rust `Arena::grow` (arena.rs:253-271):
Rust calls `alloc::realloc(self.base, old_layout, min_capacity)`, which
may return EITHER the same pointer (in-place growth) OR a different
pointer (relocation). The choice is made by the system allocator and is
NOT observable to the caller in advance.

We model this nondeterminism by taking `new_base` as a parameter. The
caller (or the proof) supplies the allocator's choice. The deterministic
`raw_grow` is the special case `new_base = a.base + a.capacity`.

This closes gap #6: the Lean model now EXACTLY mirrors the Rust
"realloc-may-relocate" semantics, rather than assuming a fixed
deterministic relocation. -/
def raw_grow_nondet (a : RawArena) (new_cap : Nat) (new_base : Ptr) : Except TrapCode RawArena :=
  if new_cap ≤ a.capacity then
    .ok a  -- no-op if not growing (Rust returns early; base unchanged)
  else if a.phase ≠ ArenaPhase.alive then
    .error .arena_overflow
  else
    -- Faithful model: `base := new_base` (allocator's nondeterministic choice).
    .ok { a with base     := new_base,
                    capacity := new_cap,
                    layout   := { size := new_cap, align := 8 } }

/-- `raw_grow` is the special case of `raw_grow_nondet` where
`new_base = a.base + a.capacity` (the deterministic simplification). -/
theorem raw_grow_eq_raw_grow_nondet (a : RawArena) (new_cap : Nat) :
    raw_grow a new_cap = raw_grow_nondet a new_cap (a.base + a.capacity) := by
  rfl

/-- `raw_grow` (deterministic) relocates the base when the grow branch
fires (`new_cap > a.capacity`) AND `a.capacity > 0`.

This lemma captures the "realloc-may-relocate" semantics for the
deterministic model. The precondition `hgrow : new_cap > a.capacity`
ensures the no-op branch does NOT fire (so `raw_grow` proceeds to the
grow branch and sets `base := a.base + a.capacity`). Combined with
`hcap : a.capacity > 0`, the new base `a.base + a.capacity` differs from
the old `a.base`.

For the faithful nondeterministic model, see `raw_grow_nondet` (which
admits any `new_base`, including `= a.base` for in-place growth). -/
theorem raw_grow_relocates_base
    (a : RawArena) (new_cap : Nat)
    (hcap : a.capacity > 0)
    (hgrow : new_cap > a.capacity)
    (a' : RawArena)
    (hresult : raw_grow a new_cap = Except.ok a') :
    a'.base ≠ a.base := by
  -- Unfold `raw_grow` and case-split on its guards.
  unfold raw_grow at hresult
  -- Guard 1: `new_cap ≤ a.capacity` → no-op. Contradicted by `hgrow`.
  have hle_neg : ¬ (new_cap ≤ a.capacity) := by omega
  rw [if_neg hle_neg] at hresult
  -- Guard 2: phase ≠ alive → error. Contradicts `.ok a'`.
  by_cases hphase : a.phase ≠ ArenaPhase.alive
  · rw [if_pos hphase] at hresult
    cases hresult
  · rw [if_neg hphase] at hresult
    -- Success branch: `a' = { a with base := a.base + a.capacity, ... }`.
    injection hresult with hval
    -- `hval : { a with base := a.base + a.capacity, ... } = a'`.
    -- Derive `a'.base = a.base + a.capacity` from `hval`.
    have hbase' : a'.base = a.base + a.capacity := by rw [hval.symm]
    -- Goal: `a'.base ≠ a.base`. Prove via `hbase'` + `hcap` without omega
    -- (omega struggles with the `Ptr` abbrev projection here).
    intro heq
    -- heq : a'.base = a.base. Substitute via hbase'.
    rw [hbase'] at heq
    -- heq : a.base + a.capacity = a.base. Rewrite RHS as `a.base + 0`.
    have h2 : a.base + a.capacity = a.base + 0 := by rw [Nat.add_zero]; exact heq
    -- Cancel `a.base` from both sides: `a.capacity = 0`.
    have h3 : a.capacity = 0 := Nat.add_left_cancel h2
    -- Contradicts `hcap : a.capacity > 0`.
    exact absurd h3 (by omega)

/-! ## §1.5 `raw_destroy`, `raw_drop`, `raw_panic_drop` (PMT-1-F gap #9) -/

/-- §1.5 (faithful): `raw_destroy a` — release arena memory.

Mirrors `Arena::destroy` (arena.rs:279-284):
  1. Call `dealloc(base, layout)`.
  2. Mark phase as `destroyed` (in Rust, the value is consumed by value).
  3. `mem::forget(self)` to prevent `Drop` from double-freeing.

In our pure model, we don't model the `dealloc` side-effect; we just update
the phase to `destroyed`, which prevents further `raw_alloc`/`raw_grow` (the
phase guards in those functions will trip). -/
def raw_destroy (a : RawArena) : RawArena :=
  { a with phase := ArenaPhase.destroyed }

/-- §1.5 (faithful): `raw_drop a` — models Rust `Drop::drop` for `Arena`
(PMT-1-F gap #9).

Mirrors Rust `impl Drop for Arena` (arena.rs:329-342):
  1. If NOT panicking: `assert_owner_thread()` (debug-only thread check).
  2. `alloc::dealloc(self.base, self.layout)` — release the memory.

In Rust, `Drop` runs automatically when the `Arena` value goes out of
scope (UNLESS `destroy` was called, which uses `mem::forget` to prevent
`Drop`). In our pure model, `raw_drop` has the same effect as
`raw_destroy`: transition to `.destroyed` (the memory is released; no
further `raw_alloc`/`raw_grow` allowed). -/
def raw_drop (a : RawArena) : RawArena :=
  { a with phase := ArenaPhase.destroyed }

/-- §1.5 (faithful): `raw_panic_drop a` — models Rust `Drop::drop` during
panic (PMT-1-F gap #9).

Mirrors Rust `impl Drop for Arena` (arena.rs:329-342), specifically the
panic-skipping branch at arena.rs:337-339:
  ```rust
  if !std::thread::panicking() {
      self.assert_owner_thread();
  }
  unsafe { alloc::dealloc(self.base, self.layout) }
  ```

When the thread is panicking (unwinding), `Drop::drop` SKIPS the
`assert_owner_thread` check (to avoid a double-panic abort that would
mask the original error message) but STILL calls `dealloc` (to release
the memory). In our pure model, the effect is the same as `raw_drop`:
transition to `.destroyed`. The distinction between `raw_drop` and
`raw_panic_drop` is DOCUMENTARY (capturing the Rust control-flow
divergence) — both produce the same Lean state. -/
def raw_panic_drop (a : RawArena) : RawArena :=
  { a with phase := ArenaPhase.destroyed }

/-- `raw_drop` has the same effect as `raw_destroy` (both transition to
`.destroyed`). In Rust, `destroy` calls `dealloc` + `mem::forget` (to
prevent `Drop`); `Drop::drop` calls `dealloc` directly. Both release the
memory; the Lean model captures this via the same phase transition. -/
theorem raw_drop_eq_raw_destroy (a : RawArena) : raw_drop a = raw_destroy a := by
  rfl

/-- `raw_panic_drop` has the same effect as `raw_destroy` (the panic-skip
of `assert_owner_thread` doesn't affect the final state — `dealloc` still
runs, releasing the memory). -/
theorem raw_panic_drop_eq_raw_destroy (a : RawArena) :
    raw_panic_drop a = raw_destroy a := by
  rfl

/-- `raw_panic_drop` equals `raw_drop` (both transition to `.destroyed`;
the panic-vs-non-panic distinction is documentary only). -/
theorem raw_panic_drop_eq_raw_drop (a : RawArena) :
    raw_panic_drop a = raw_drop a := by
  rfl

/-! ## §2. Well-formedness & capacity preservation -/

/-- §2 (faithful): Well-formedness for `RawArena`. Mirrors the implicit
invariants the Rust `Arena` constructor + `alloc_raw` enforce:
  - `offset ≤ capacity`                — bump pointer in bounds.
  - `layout.align = 8`                 — hard-coded 8-byte region alignment.
  - `destroyed → offset = 0 ∨ offset ≤ capacity` — vacuous after `destroy`
    (model: phase destroyed, memory no longer ours; offset is whatever it was).
  - `layout.size = capacity`           — layout cached for `dealloc` matches
    the actual allocated size.

**Note.** The usize bounds (`offset < 2^64`, `capacity < 2^64`) and the
thread-owner check (`created_thread > 0`) are NOT in this base
`WF_RawArena` predicate — they are in the stronger
`WF_RawArena_faithful` predicate below, which is what the simulation
relations use. This keeps `WF_RawArena` backward-compatible with
existing proofs in `PMT.ArenaProperties`. -/
def WF_RawArena (a : RawArena) : Prop :=
  a.offset ≤ a.capacity
  ∧ a.layout.align = 8
  ∧ (a.phase = ArenaPhase.destroyed → a.offset = 0 ∨ a.offset ≤ a.capacity)
  ∧ a.layout.size = a.capacity

/-- §2 (faithful): `WF_RawArena_faithful` — STRONGER well-formedness that
includes the usize bounds (PMT-1-F gap #2) and the thread-owner check
(PMT-1-F gap #1).

This is the FAITHFUL well-formedness predicate: it captures ALL the
invariants that Rust's `Arena::create` + `alloc_raw` enforce, including:
  - All conjuncts of `WF_RawArena`.
  - `offset < 2^64`   — usize bound (Rust `offset: usize` is 64-bit).
  - `capacity < 2^64` — usize bound (Rust `capacity: usize` is 64-bit).
  - `created_thread > 0` — the arena has a valid owner thread (Rust's
    `Arena::create` always sets `created_thread` to a non-null `ThreadId`).
  - `layout.align = 8` (duplicate of `WF_RawArena`'s conjunct; kept here
    for self-contained faithfulness).
  - `layout.size = capacity` (duplicate of `WF_RawArena`'s conjunct).

The simulation relations (`RawArena_simulates_Arena`, `arena_sim`) require
`WF_RawArena_faithful` (or its conjuncts inlined) to ensure the Lean
state EXACTLY mirrors the Rust state. -/
def WF_RawArena_faithful (a : RawArena) : Prop :=
  WF_RawArena a
  ∧ a.offset < USIZE_BOUND         -- PMT-1-F gap #2 (usize bound)
  ∧ a.capacity < USIZE_BOUND       -- PMT-1-F gap #2 (usize bound)
  ∧ a.created_thread > 0           -- PMT-1-F gap #1 (thread owner set)
  ∧ a.layout.align = 8             -- PMT-1-F gap #3 (layout)
  ∧ a.layout.size = a.capacity     -- PMT-1-F gap #3 (layout)

/-- §2 (faithful): `raw_alloc` preserves `WF_RawArena`.

This is the faithful analogue of `PMT.Basic.alloc_preserves_capacity`: given a
well-formed arena and a fitting request, the resulting arena is well-formed.

**Note.** This proof is for the BASE `WF_RawArena` (4 conjuncts), which is
backward-compatible with `PMT.ArenaProperties.raw_alloc_preserves_wf_raw`.
The faithful version (`raw_alloc_preserves_wf_faithful`) is below. -/
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

/-- §2 (faithful): `raw_alloc` preserves `WF_RawArena_faithful`.

This is the FAITHFUL version: it preserves the usize bounds
(`offset < 2^64`, `capacity < 2^64`), the thread-owner check
(`created_thread > 0`), and the layout invariants — in addition to the
base `WF_RawArena` conjuncts. -/
theorem raw_alloc_preserves_wf_faithful
    (a : RawArena) (size : Nat)
    (hwf : WF_RawArena_faithful a)
    (hfit : a.offset + align8_nat size ≤ a.capacity) :
    ∀ a', raw_alloc a size = Except.ok a' → WF_RawArena_faithful a' := by
  intro a' h
  obtain ⟨hwf_base, hoffset_bnd, hcap_bnd, hthread, halign, hsize⟩ := hwf
  -- Delegate the base WF_RawArena conjuncts to `raw_alloc_preserves_wf`.
  have hwf_base' : WF_RawArena a' := raw_alloc_preserves_wf a size hwf_base hfit a' h
  -- Derive the structure-update equation: `a' = { a with offset := ... }`.
  -- `raw_alloc` (success branch) sets `a' = { a with offset := a.offset + align8_nat size }`,
  -- so all fields except `offset` are preserved.
  have hsuccess : a' = { a with offset := a.offset + align8_nat size } := by
    unfold raw_alloc at h
    by_cases hphase : a.phase ≠ ArenaPhase.alive
    · rw [if_pos hphase] at h; cases h
    · rw [if_neg hphase] at h
      by_cases hovf : a.offset + align8_nat size > a.capacity
      · rw [if_pos hovf] at h; cases h
      · rw [if_neg hovf] at h
        injection h with hval
        exact hval.symm
  -- Derive the field equations for `a'` from `hsuccess`.
  have hoffset' : a'.offset = a.offset + align8_nat size := by rw [hsuccess]
  have hcap'   : a'.capacity = a.capacity           := by rw [hsuccess]
  have hthread' : a'.created_thread = a.created_thread := by rw [hsuccess]
  have hlayout' : a'.layout = a.layout              := by rw [hsuccess]
  -- Assemble the faithful conjuncts.
  refine ⟨hwf_base', ?_, ?_, ?_, ?_, ?_⟩
  · -- `a'.offset < USIZE_BOUND`: from `hoffset'` + `hfit` + `hcap_bnd`.
    rw [hoffset']
    -- `a.offset + align8_nat size ≤ a.capacity < USIZE_BOUND`.
    omega
  · -- `a'.capacity < USIZE_BOUND`: from `hcap'` + `hcap_bnd`.
    rw [hcap']; exact hcap_bnd
  · -- `a'.created_thread > 0`: from `hthread'` + `hthread`.
    rw [hthread']; exact hthread
  · -- `a'.layout.align = 8`: from `hlayout'` + `halign`.
    rw [hlayout']; exact halign
  · -- `a'.layout.size = a'.capacity`: from `hlayout'` + `hsize` + `hcap'`.
    rw [hlayout', hcap']; exact hsize

/-! ## §3. `raw_alloc_with_overflow` equivalence (PMT-1-F gap #8) -/

/-- §3 (faithful): `raw_alloc_with_overflow` equals `raw_alloc` under
`WF_RawArena_faithful` (PMT-1-F gap #8).

Under the usize bounds (`offset < 2^64`, `capacity < 2^64`), the
arithmetic-overflow guard `offset + aligned_size ≥ 2^64` is subsumed by
the capacity-overflow guard `offset + aligned_size > capacity` (since
`capacity < 2^64`). Hence both functions produce the same result.

This lemma closes gap #8: it shows that the FAITHFUL
`raw_alloc_with_overflow` (which models BOTH Rust trap paths) is
equivalent to the simpler `raw_alloc` (which models only the capacity
path) for all WELL-FORMED arenas. For ill-formed arenas (where the usize
bounds are violated), the two functions may diverge — but
`WF_RawArena_faithful` excludes those. -/
theorem raw_alloc_with_overflow_eq_raw_alloc_under_wf
    (a : RawArena) (size : Nat)
    (hwf : WF_RawArena_faithful a) :
    raw_alloc_with_overflow a size = raw_alloc a size := by
  obtain ⟨hwf_base, hoffset_bnd, hcap_bnd, hthread, halign, hsize⟩ := hwf
  -- Case-split on the guards of `raw_alloc_with_overflow` and `raw_alloc`.
  -- The phase guard is identical in both. The arithmetic-overflow guard
  -- (in `raw_alloc_with_overflow`) is subsumed by the capacity-overflow
  -- guard (in both) under `hcap_bnd : a.capacity < 2^64`.
  by_cases hphase : a.phase ≠ ArenaPhase.alive
  · -- Phase guard fires in BOTH functions → both return `.error`. Equal.
    unfold raw_alloc_with_overflow raw_alloc
    rw [if_pos hphase, if_pos hphase]
  · -- Phase guard doesn't fire in either.
    unfold raw_alloc_with_overflow raw_alloc
    rw [if_neg hphase, if_neg hphase]
    -- Now: `raw_alloc_with_overflow` has guards [arith, cap]; `raw_alloc` has [cap].
    -- Case-split on the arithmetic guard.
    by_cases hovf_arith : a.offset + align8_nat size ≥ USIZE_BOUND
    · -- Arithmetic overflow fires in `raw_alloc_with_overflow` → returns `.error`.
      rw [if_pos hovf_arith]
      -- In `raw_alloc`, the capacity guard fires too (since `cap < 2^64 ≤ sum`).
      have hovf_cap : a.offset + align8_nat size > a.capacity := by omega
      rw [if_pos hovf_cap]
    · -- Arithmetic overflow doesn't fire.
      -- After rewriting the arithmetic guard with `if_neg`, the LHS reduces
      -- to the capacity guard, which is identical to the RHS (`raw_alloc`'s
      -- capacity guard). The `rw` auto-closes the resulting `rfl` goal.
      rw [if_neg hovf_arith]

/-! ## §4. `raw_create` lemmas (PMT-1-F gap #5) -/

/-- §4 (faithful): `raw_create` succeeds for reasonable capacities
(`≤ ISIZE_MAX = 2^63 - 1`).

The resulting arena has:
  - `phase = .alive`            — immediately usable (matches Rust semantics).
  - `offset = 0`                — bump pointer starts at the base.
  - `capacity = capacity`       — the requested capacity is recorded.
  - `created_thread = thread`   — the supplied thread ID (default 1).
  - `layout.align = 8`          — Rust's hard-coded 8-byte alignment.
  - `layout.size = capacity`    — layout cached for `dealloc`. -/
theorem raw_create_succeeds_small (capacity : Nat) (thread : ThreadId)
    (hsmall : capacity ≤ ISIZE_MAX) :
    ∃ a, raw_create capacity thread = Except.ok a
    ∧ a.phase = ArenaPhase.alive
    ∧ a.offset = 0
    ∧ a.capacity = capacity
    ∧ a.created_thread = thread
    ∧ a.layout.align = 8
    ∧ a.layout.size = capacity := by
  unfold raw_create
  have hneg : ¬ (capacity > ISIZE_MAX) := by omega
  rw [if_neg hneg]
  refine ⟨{ base := 1000, offset := 0, capacity := capacity,
            layout := { size := capacity, align := 8 },
            phase := .alive, created_thread := thread },
          rfl, rfl, rfl, rfl, rfl, rfl, rfl⟩

/-- §4 (faithful): `raw_create` fails for huge capacities
(`> ISIZE_MAX = 2^63 - 1`) — the `Layout::from_size_align` failure path.

Returns `Except.error TrapCode.arena_overflow`, mirroring Rust's
`Arena::create` trap when `layout_for(capacity)` (arena.rs:117-121) calls
`Layout::from_size_align(capacity, 8).unwrap_or_else(|_| trap)`.

**PMT-1-F gap #5 (corrected threshold).** The prior `raw_create_fails_huge`
(in `PMT.MmapArena`) used `capacity > 4294967296` (4 GiB) as the threshold.
This was a CONSERVATIVE UNDER-APPROXIMATION: Rust actually traps at
`capacity > 2^63 - 1` (= `isize::MAX` on 64-bit). This lemma uses the
CORRECT threshold, matching Rust's `Layout::from_size_align` failure
condition exactly. -/
theorem raw_create_fails_huge (capacity : Nat) (thread : ThreadId)
    (hhuge : capacity > ISIZE_MAX) :
    raw_create capacity thread = Except.error TrapCode.arena_overflow := by
  unfold raw_create
  have hpos : capacity > ISIZE_MAX := hhuge
  rw [if_pos hpos]

/-- §4 (faithful): `raw_destroy` after `raw_create` transitions the arena
to the `.destroyed` phase. Mirrors Rust's `Arena::destroy` (arena.rs:279-284):
after a successful create, the destructor releases the underlying
allocation and marks the arena as no-longer-usable. -/
theorem raw_create_then_destroy (capacity : Nat) (thread : ThreadId)
    (hsmall : capacity ≤ ISIZE_MAX) :
    ∃ a, raw_create capacity thread = Except.ok a
      ∧ (raw_destroy a).phase = ArenaPhase.destroyed := by
  obtain ⟨a, hcreate, _, _, _, _, _, _⟩ :=
    raw_create_succeeds_small capacity thread hsmall
  refine ⟨a, hcreate, ?_⟩
  -- `raw_destroy a = { a with phase := .destroyed }`, so `.phase = .destroyed`
  -- holds by `rfl`.
  rfl

/-! ## §5. Simulation relation to abstract `Arena` (faithful) -/

/-- §5 (faithful): Simulation relation: `RawArena` faithfully implements
abstract `Arena`.

The abstract `Arena` from `PMT.Basic` is `(base, capacity, used)`.
The faithful `RawArena` is `(base, offset, capacity, layout, phase, created_thread)`.
The simulation maps:
  - `abstract.base`     ↔ `raw.base`
  - `abstract.capacity` ↔ `raw.capacity`
  - `abstract.used`     ↔ `raw.offset`
  - (faithful)          `raw.phase = .alive`           (only alive arenas simulate)
  - (faithful)          `raw.layout.align = 8`         (PMT-1-F gap #3)
  - (faithful)          `raw.layout.size = raw.capacity` (PMT-1-F gap #3)
  - (faithful)          `raw.created_thread > 0`       (PMT-1-F gap #1)
  - (faithful)          `raw.offset < 2^64`            (PMT-1-F gap #2)
  - (faithful)          `raw.capacity < 2^64`          (PMT-1-F gap #2)

The faithful conjuncts (5th onward) capture the Rust state that the
abstract `Arena` model doesn't track but that the simulation must
preserve. They make the Lean state EXACTLY mirror the Rust state at
corresponding program points (per the PMT-1-F task brief:
"the Lean arena state exactly mirrors the Rust arena state"). -/
def RawArena_simulates_Arena (raw : RawArena) (abs : Arena) : Prop :=
  raw.base = abs.base
  ∧ raw.capacity = abs.capacity
  ∧ raw.offset = abs.used
  ∧ raw.phase = ArenaPhase.alive
  ∧ raw.layout.align = 8
  ∧ raw.layout.size = raw.capacity
  ∧ raw.created_thread > 0
  ∧ raw.offset < USIZE_BOUND
  ∧ raw.capacity < USIZE_BOUND

/-- §5 (faithful): The simulation is preserved by `alloc` (with the
alignment gap discharged via the `haligned` precondition).

This is the key simulation theorem: if `raw` simulates `abs`, then after a
successful `raw_alloc raw (align8_nat size)`, there exists an `abs'` such that
`alloc abs ⟨"alloc", size, []⟩ = abs'` and `raw'` simulates `abs'`.

**Closing strategy (mirroring the approach to the sibling
`arena_sim_preserved_by_alloc` lemma in `SimRel.lean`)**: the abstract
`alloc` advances `used` by `size`, but `raw_alloc` advances `offset` by
`align8_nat (align8_nat size) = align8_nat size` (which is `≥ size`, possibly
strictly greater when `size` is not a multiple of 8). We close the gap by
adding the precondition `haligned : size % 8 = 0`, which forces
`align8_nat size = size` (since `(size + 7) / 8 * 8 = size` when `size` is
already 8-aligned). With this, both sides advance their pointers by the same
amount, and the simulation is preserved field-by-field.

**Note on `haligned`.** The `haligned` precondition here is the
`RawArena_simulates_Arena` analogue of the one in
`arena_sim_preserved_by_alloc` (`SimRel.lean`). The companion theorem
`arena_sim_preserved_by_alloc` in `SimRel.lean` DROPS `haligned` by using
`aligned_alloc` (which advances `used` by `align8_nat total_size`) instead
of `alloc`. The two theorems together close PMT-1-F gap #7: `haligned` is
DISCHARGED in `SimRel.lean` (by switching to `aligned_alloc`), and the
present theorem keeps `haligned` for backward compatibility with the
existing `alloc`-based abstraction. -/
theorem raw_alloc_simulates_alloc
    (raw : RawArena) (abs : Arena) (size : Nat)
    (hsim : RawArena_simulates_Arena raw abs)
    (_hwf_abs : WF_Arena abs)
    (_hfit : abs.used + size ≤ abs.capacity)
    (haligned : size % 8 = 0) :
    ∀ raw', raw_alloc raw (align8_nat size) = Except.ok raw' →
      ∃ abs', alloc abs ⟨"alloc", size, []⟩ = abs'
        ∧ RawArena_simulates_Arena raw' abs' := by
  -- Extract components of `hsim` for field-by-field reasoning.
  have hbase  : raw.base = abs.base                    := hsim.1
  have hcap   : raw.capacity = abs.capacity            := hsim.2.1
  have hused  : raw.offset = abs.used                  := hsim.2.2.1
  have hphase : raw.phase = ArenaPhase.alive           := hsim.2.2.2.1
  have halign : raw.layout.align = 8                   := hsim.2.2.2.2.1
  have hsize  : raw.layout.size = raw.capacity         := hsim.2.2.2.2.2.1
  have hthread : raw.created_thread > 0                := hsim.2.2.2.2.2.2.1
  have hoffset_bnd : raw.offset < USIZE_BOUND          := hsim.2.2.2.2.2.2.2.1
  have hcap_bnd : raw.capacity < USIZE_BOUND           := hsim.2.2.2.2.2.2.2.2
  -- When `size % 8 = 0`, `align8_nat size = size` (the alignment padding
  -- vanishes because `size` is already a multiple of 8).
  have halign_size : align8_nat size = size := by
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
      rw [halign_size, halign_size]
      -- Witness: `abs' = { abs with used := abs.used + size }`.
      -- `alloc abs ⟨"alloc", size, []⟩ = { abs with used := abs.used + size }` definitionally.
      refine ⟨{ abs with used := abs.used + size }, rfl, ?_⟩
      -- Prove `RawArena_simulates_Arena raw' abs'` field-by-field.
      unfold RawArena_simulates_Arena
      -- The faithful conjuncts: `offset' = raw.offset + size < 2^64` (from hoffset_bnd + omega);
      -- `capacity' = raw.capacity < 2^64` (unchanged); `created_thread > 0` (unchanged);
      -- `layout.align = 8`, `layout.size = capacity` (unchanged).
      refine ⟨hbase, hcap, ?_, hphase, halign, hsize, hthread, ?_, hcap_bnd⟩
      · -- `raw'.offset = raw.offset + size = abs.used + size = abs'.used`.
        show raw.offset + size = abs.used + size
        rw [hused]
      · -- `raw'.offset = raw.offset + size < 2^64` (from `hoffset_bnd`).
        show raw.offset + size < USIZE_BOUND
        omega

end PMT
