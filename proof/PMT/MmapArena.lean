import PMT.RawArena
import PMT.ArenaProperties  -- for `raw_alloc_alive_succeeds` (composition target)

/-! ## MmapArena — models mmap/alloc failure path

The Rust `Arena::create` (see `src/codegen/src/runtime/arena.rs:138-146`)
calls `alloc::alloc(layout)` and **traps if it returns null** (OOM).
The same hazard exists for `Arena::grow` (arena.rs:209-214) after
`alloc::realloc`.

The Lean `RawArena` model (`PMT.RawArena`) has no `raw_create` function —
`RawArena` is constructed by supplying fields directly, making
allocator-null impossible. Per the W1-D arena-fidelity audit (see
`docs/verification-reports/S2-W1-D-arena-fidelity.md`, §3 Gap 3), this is
the most acute simulation-soundness gap: any proof that "every well-formed
`RawArena` corresponds to a reachable Rust `Arena`" would be **false** in
the model, since the model admits arenas the Rust constructor would have
trapped before producing.

This module closes that gap by adding:

  - `raw_create : Nat → Except TrapCode RawArena` — the first function in
    the PMT model that produces `Except.error .arena_overflow` from a
    *constructor* (rather than from a capacity guard inside `raw_alloc`).
    Allocator failure is modeled as a threshold check (`capacity > 4 GiB`)
    that stands in for "the system allocator returned null on OOM". The
    threshold is conservative — Rust's `alloc::alloc` will in practice
    return null long before 4 GiB on most targets — but the model only
    needs *some* decidable failure predicate to make the failure path
    syntactically expressible.

  - Three proven lemmas about `raw_create`:
      * `raw_create_succeeds_small` — reasonable capacities succeed and
        yield an arena with `phase = .alive`, `offset = 0`,
        `capacity = capacity`.
      * `raw_create_fails_huge` — capacities past the allocator threshold
        trap with `.arena_overflow` (the OOM path).
      * `raw_create_then_destroy` — `raw_destroy` after `raw_create`
        transitions to `.destroyed` (the dealloc contract).

  - One composition lemma (`raw_alloc_on_fresh_arena`) — composes
    `raw_create_succeeds_small` with `raw_alloc_alive_succeeds` (from
    `PMT.ArenaProperties`) to show that `raw_alloc` on a freshly-`raw_create`'d
    arena succeeds and advances `offset` by `align8_nat size`. The proof
    adds a `haligned : size % 8 = 0` precondition to bridge the
    `align8_nat size` vs `size` alignment gap (the same precondition used
    by `raw_alloc_simulates_alloc` in `PMT.RawArena`, q.v. for rationale).
    Wave 17's alignment-relaxation work may relax this precondition.

**Status**: `lake build PMT.MmapArena` produces no errors and no
`sorry` warnings — the file is fully sorry-free.

**References**.
  * `docs/verification-reports/S2-W1-D-arena-fidelity.md` §3 Gap 3 —
    the original audit finding this module addresses.
  * `src/codegen/src/runtime/arena.rs:138-146` — Rust `Arena::create`.
  * `src/codegen/src/runtime/arena.rs:209-214` — Rust `Arena::grow`
    (companion realloc-null path, not modeled here).
  * `PMT.RawArena` — the underlying `RawArena` model.
  * `PMT.ArenaProperties` — `raw_alloc_alive_succeeds` (composition
    target for the previously-sorry'd lemma).
-/

namespace PMT

/-! ## §1. The `raw_create` constructor (mmap/alloc failure path) -/

/-- `raw_create capacity` — allocate a new arena of the given capacity.

Models Rust `Arena::create` (arena.rs:138-146), which calls
`alloc::alloc(layout)` and traps on null return (OOM).

In our model, allocator failure is approximated by a 4 GiB capacity
threshold: any request for more than `4294967296` (= 2^32) bytes is
treated as failing the underlying `alloc::alloc` and traps with
`Except.error TrapCode.arena_overflow`. This is a *conservative*
under-approximation of OOM (real allocators will OOM well below 4 GiB
on constrained targets) but suffices to make the failure path
syntactically expressible in the Lean model.

For successful allocations, the resulting arena mirrors the field
initialization that Rust's `Arena::create` performs (arena.rs:140-145):
  - `base`      — fresh pointer from `alloc::alloc` (placeholder `1000`).
  - `offset`    — initial bump pointer, `0`.
  - `capacity`  — the requested capacity.
  - `layout`    — `Layout::from_size_align(capacity, 8)`.
  - `phase`     — `.alive` (the arena is immediately usable). -/
def raw_create (capacity : Nat) : Except TrapCode RawArena :=
  -- Model: allocator fails if capacity exceeds the 4 GiB threshold.
  -- In Rust reality, `alloc::alloc` returns null on OOM and `Arena::create`
  -- traps via `arena_overflow_trap` (exit code 1).
  if capacity > 4294967296 then  -- 4 GiB threshold
    .error .arena_overflow  -- allocator failure modeled as arena_overflow
  else
    .ok { base     := 1000,  -- placeholder base address (fresh alloc)
          offset   := 0,
          capacity := capacity,
          layout   := { size := capacity, align := 8 },
          phase    := .alive }

/-! ## §2. Lemmas about `raw_create` -/

/-- `raw_create` succeeds for reasonable capacities (≤ 4 GiB).

The resulting arena has:
  - `phase = .alive`     — immediately usable (matches Rust semantics).
  - `offset = 0`         — bump pointer starts at the base.
  - `capacity = capacity` — the requested capacity is recorded. -/
theorem raw_create_succeeds_small (capacity : Nat)
    (hsmall : capacity ≤ 4294967296) :
    ∃ a, raw_create capacity = Except.ok a
    ∧ a.phase = ArenaPhase.alive
    ∧ a.offset = 0
    ∧ a.capacity = capacity := by
  unfold raw_create
  have hneg : ¬ (capacity > 4294967296) := by omega
  rw [if_neg hneg]
  refine ⟨{ base := 1000, offset := 0, capacity := capacity,
            layout := { size := capacity, align := 8 },
            phase := .alive }, rfl, rfl, rfl, rfl⟩

/-- `raw_create` fails for huge capacities (> 4 GiB) — the allocator OOM
path. Returns `Except.error TrapCode.arena_overflow`, mirroring Rust's
`Arena::create` trap when `alloc::alloc` returns null. -/
theorem raw_create_fails_huge (capacity : Nat)
    (hhuge : capacity > 4294967296) :
    raw_create capacity = Except.error TrapCode.arena_overflow := by
  unfold raw_create
  have hpos : capacity > 4294967296 := hhuge
  rw [if_pos hpos]

/-- `raw_destroy` after `raw_create` transitions the arena to the
`.destroyed` phase. Mirrors Rust's `Arena::destroy` (arena.rs:226-231):
after a successful create, the destructor releases the underlying
allocation and marks the arena as no-longer-usable. -/
theorem raw_create_then_destroy (capacity : Nat)
    (hsmall : capacity ≤ 4294967296) :
    ∃ a, raw_create capacity = Except.ok a
      ∧ (raw_destroy a).phase = ArenaPhase.destroyed := by
  obtain ⟨a, hcreate, _, _, _⟩ := raw_create_succeeds_small capacity hsmall
  refine ⟨a, hcreate, ?_⟩
  -- `raw_destroy a = { a with phase := .destroyed }`, so `.phase = .destroyed`
  -- holds by `rfl`.
  rfl

/-! ## §3. Composition lemma

The proof composes `raw_create_succeeds_small` (to obtain `a` from
`raw_create capacity`) with `raw_alloc_alive_succeeds` (from
`PMT.ArenaProperties`) to show that `raw_alloc a size` succeeds and
yields an arena whose `offset` advanced by `align8_nat size`.

**Alignment precondition.** `raw_alloc` advances `offset` by
`align8_nat size`, but the user-supplied bound `hfit : size ≤ capacity`
is phrased in terms of `size`, not `align8_nat size`. We add a
`haligned : size % 8 = 0` precondition (the same one used by
`raw_alloc_simulates_alloc` in `PMT.RawArena`); via
`align8_preserves_aligned`, this gives `align8_nat size = size`, so
`hfit` rewrites directly to the form `raw_alloc_alive_succeeds`
expects. Wave 17's alignment-relaxation work may relax this
precondition (see `PMT.SimRel`'s TODO).
-/

/-- `raw_alloc` on a freshly-`raw_create`'d arena succeeds and advances
    `offset` by `align8_nat size`.

    The composition is:
      1. `raw_create_succeeds_small` — produces `a` with `phase = .alive`,
         `offset = 0`, `capacity = capacity`.
      2. `raw_alloc_alive_succeeds` — on `a`, succeeds and advances
         `offset` by `align8_nat size`.

    The `haligned` precondition bridges the alignment gap (see the
    section header above). -/
theorem raw_alloc_on_fresh_arena (capacity size : Nat)
    (hsmall_cap : capacity ≤ 4294967296)
    (hfit : size ≤ capacity)
    (haligned : size % 8 = 0) :
    ∃ a a', raw_create capacity = Except.ok a
      ∧ raw_alloc a size = Except.ok a'
      ∧ a'.offset = align8_nat size := by
  -- Step 1: `raw_create` succeeds and yields `a` with the expected fields.
  obtain ⟨a, hcreate, halive, hoffset, hcapacity⟩ :=
    raw_create_succeeds_small capacity hsmall_cap
  -- Step 2: alignment precondition ⇒ `align8_nat size = size`.
  have halign_eq : align8_nat size = size :=
    align8_preserves_aligned size haligned
  -- Step 3: derive the fit precondition `raw_alloc_alive_succeeds` expects:
  -- `a.offset + align8_nat size ≤ a.capacity`, which (after rewriting
  -- `a.offset = 0`, `a.capacity = capacity`, `align8_nat size = size`)
  -- reduces to `size ≤ capacity` (i.e., `hfit`).
  have hfit' : a.offset + align8_nat size ≤ a.capacity := by
    rw [hoffset, hcapacity, halign_eq]
    omega
  -- Step 4: `raw_alloc` on the alive arena `a` succeeds and yields `a'`
  -- with `a'.offset = a.offset + align8_nat size` and `a'.phase = .alive`.
  obtain ⟨a', halloc, hoffset', hphase'⟩ :=
    raw_alloc_alive_succeeds a size halive hfit'
  -- Step 5: assemble the existential. `a'.offset = a.offset + align8_nat size`
  -- and `a.offset = 0`, so `a'.offset = 0 + align8_nat size = align8_nat size`.
  refine ⟨a, a', hcreate, halloc, ?_⟩
  rw [hoffset', hoffset]
  omega

end PMT
