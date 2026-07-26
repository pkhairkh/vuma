import PMT.RawArena
import PMT.ArenaProperties  -- for `raw_alloc_alive_succeeds` (composition target)

/-! ## MmapArena — composition layer for `raw_create` + `raw_alloc`

**PMT-1-F.** The `raw_create` function and its three lemmas
(`raw_create_succeeds_small`, `raw_create_fails_huge`,
`raw_create_then_destroy`) have been MOVED to `PMT.RawArena` (with the
CORRECTED threshold `> ISIZE_MAX = 2^63 - 1`, matching Rust's
`Layout::from_size_align` failure condition — see PMT-1-F gap #5).
This module now contains ONLY the composition lemma
`raw_alloc_on_fresh_arena`, which composes `raw_create_succeeds_small`
(from `PMT.RawArena`) with `raw_alloc_alive_succeeds` (from
`PMT.ArenaProperties`).

### Why this module exists (historical)

Per the arena-fidelity audit, the prior `RawArena` model had no
`raw_create` function — `RawArena` was constructed by supplying fields
directly, making allocator-null impossible. This was the most acute
simulation-soundness gap: any proof that "every well-formed `RawArena`
corresponds to a reachable Rust `Arena`" would be **false** in the
model, since the model admits arenas the Rust constructor would have
trapped before producing.

`raw_create` was added (originally in this module, now in `PMT.RawArena`)
to close that gap by providing a constructor that CAN fail (modeling
the `Layout::from_size_align` failure path).

### PMT-1-F changes

  - `raw_create` and its three lemmas moved to `PMT.RawArena` (with the
    corrected threshold `> ISIZE_MAX = 2^63 - 1`, PMT-1-F gap #5).
  - The `raw_create` signature now takes a `thread : ThreadId` parameter
    (default `1`), modeling Rust's `Arena::create` which sets
    `created_thread` to `std::thread::current().id()` (PMT-1-F gap #1).
  - The composition lemma `raw_alloc_on_fresh_arena` is updated to use
    the new `raw_create_succeeds_small` signature (7-tuple, with `thread`).
  - The `haligned : size % 8 = 0` precondition on
    `raw_alloc_on_fresh_arena` is KEPT (it bridges the `align8_nat size`
    vs `size` gap in the COMPOSITION, not in the sim-rel — the sim-rel
    `haligned` was discharged in `PMT.SimRel` via `aligned_alloc`).

**Status**: `lake build PMT.MmapArena` produces no errors and no
`sorry` warnings — the file is fully sorry-free.

**References**.
  * `src/codegen/src/runtime/arena.rs:117-121` — Rust `layout_for`
    (the `Layout::from_size_align` failure path, now correctly modeled).
  * `src/codegen/src/runtime/arena.rs:147-163` — Rust `Arena::create`.
  * `PMT.RawArena` — the `raw_create` function + lemmas (PMT-1-F).
  * `PMT.ArenaProperties` — `raw_alloc_alive_succeeds` (composition
    target).
-/

namespace PMT

/-! ## §1. Composition lemma -/

/-! ## §1. Composition: `raw_alloc` on a freshly-`raw_create`'d arena

The proof composes `raw_create_succeeds_small` (from `PMT.RawArena`, to
obtain `a` from `raw_create capacity thread`) with `raw_alloc_alive_succeeds`
(from `PMT.ArenaProperties`) to show that `raw_alloc a size` succeeds and
yields an arena whose `offset` advanced by `align8_nat size`.

**Alignment precondition.** `raw_alloc` advances `offset` by
`align8_nat size`, but the user-supplied bound `hfit : size ≤ capacity`
is phrased in terms of `size`, not `align8_nat size`. We add a
`haligned : size % 8 = 0` precondition (the same one used by
`raw_alloc_simulates_alloc` in `PMT.RawArena`); via
`align8_preserves_aligned`, this gives `align8_nat size = size`, so
`hfit` rewrites directly to the form `raw_alloc_alive_succeeds`
expects.

**Note on `haligned` vs PMT-1-F gap #7.** The `haligned` here is for
the COMPOSITION (bridging `align8_nat size` vs `size` in the `hfit`
precondition), NOT for the sim-rel. The sim-rel `haligned` was
DISCHARGED in `PMT.SimRel` via `aligned_alloc` (PMT-1-F gap #7). This
composition lemma keeps `haligned` because it uses the BASE `raw_alloc`
(not the sim-rel), and the `hfit` precondition is naturally phrased in
terms of `size` (the user's request), not `align8_nat size` (the
aligned size). A future refinement may relax this by rephrasing `hfit`
in terms of `align8_nat size`.
-/

/-- `raw_alloc` on a freshly-`raw_create`'d arena succeeds and advances
    `offset` by `align8_nat size`.

    The composition is:
      1. `raw_create_succeeds_small` — produces `a` with `phase = .alive`,
         `offset = 0`, `capacity = capacity`, `created_thread = thread`.
      2. `raw_alloc_alive_succeeds` — on `a`, succeeds and advances
         `offset` by `align8_nat size`.

    The `haligned` precondition bridges the alignment gap (see the
    section header above). -/
theorem raw_alloc_on_fresh_arena (capacity size : Nat)
    (hsmall_cap : capacity ≤ ISIZE_MAX)
    (hfit : size ≤ capacity)
    (haligned : size % 8 = 0) :
    ∃ a a', raw_create capacity 1 = Except.ok a
      ∧ raw_alloc a size = Except.ok a'
      ∧ a'.offset = align8_nat size := by
  -- Step 1: `raw_create` succeeds and yields `a` with the expected fields.
  -- (The new `raw_create_succeeds_small` from `PMT.RawArena` takes a
  -- `thread` parameter and returns a 7-tuple.)
  obtain ⟨a, hcreate, halive, hoffset, hcapacity, _, _, _⟩ :=
    raw_create_succeeds_small capacity 1 hsmall_cap
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
