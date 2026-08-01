import PMT.Soundness

/-!
# PMT Test — Arena Basic Simulation (`arena_basic.vuma`)

W9-D test harness: models the
`tests/gold_standard/arena_wave1/arena_basic.vuma` example (21 lines,
expected exit 42) as a Lean `Program` and verifies it executes correctly.

Per W2-F's recommendation, `arena_basic.vuma` is the primary
simulation-relation target because it exercises:

  * `arena_new`      → `Arena::create`   (lowered to mmap + 24-byte Arena struct)
  * `arena_alloc`    → `Arena::alloc<T>` (bump pointer, returns derived `State<Widget>`)
  * `w.x = 42`       → Store at field offset (IVE `verify_state_writes`)
  * `w.x`            → Load at field offset (IVE `verify_state_reads`)
  * `arena_free`     → `Arena::destroy`   (lowered to munmap; IVE `verify_transform`)

This hits all 3 IVE entry points (state reads, state writes, transform)
and the 4 Rust Arena API methods (`create`, `alloc<T>`, `grow`-implicit-via-
capacity, `destroy`/Drop), and is small enough (≤30 lines, 8 statements)
for a Lean simulation-relation proof to fit in a single file.

**Modeling decisions (adapted to the actual `PMT.Soundness` API).**

The Lean `Step` type is `⟨in_var, out_var, layout⟩` — it does not
distinguish store vs load vs alloc; every step is a `state_transform`
that consumes `in_var` (kills it) and produces `out_var` (makes it live).
To faithfully model the `.vuma` program's store/load sequence while
respecting `WellTyped`'s name-uniqueness conjunct (each `in_var`/`out_var`
must appear exactly once), we use **distinct variable names per step**:

  * Step 1: `arena_alloc(arena, Widget)` — `arena → w`        (alloc)
  * Step 2: `w.x = 42`                  — `w → w_store`        (store to field x)
  * Step 3: `let val = w.x`             — `w_store → w_load`   (load from field x)
  * Step 4: `arena_free(arena)`         — `w_load → freed`     (destroy arena)

The `arena_new(4096)` call is folded into `initState` (a fresh 1024-byte
arena) — it is the initial condition, not a PMT step. The real `Widget`
has two fields `{ x: u32, y: u32 }` (8 bytes); we model only field `x`
(4 bytes) since the `Step` type's field-value semantic is opaque to
`exec` (the bump pointer advances by `layout.total_size` regardless of
which field is accessed). The `w.y = 100` store is omitted without loss
of generality — it would be a fifth identical `state_transform` step.

**Type-checking.** `lake build PMT.Test.ArenaBasicSim` should produce no
errors and no `sorry` warnings.
-/

namespace PMT.Test.ArenaBasicSim

/-! ## §1. Widget layout: 4-byte field `x` at offset 0.

The real `.vuma` Widget is `{ x: u32, y: u32 }` (8 bytes, two fields).
Here we model only `x` (4 bytes, one field) — the Lean `Step` does not
track which field is accessed, so the extra field would be redundant
for the `exec` computation (it only consults `layout.total_size`). -/

/-- Simplified Widget layout: single 4-byte field `x` at offset 0. -/
def widgetLayout : Layout := ⟨"layout", 4, [⟨"f", 0, 4, "i32"⟩]⟩

/-- The widget layout is well-formed: field `x` is in bounds
(`0 + 4 ≤ 4`), disjointness is vacuous (single field), and
`total_size > 0`. -/
theorem wf_widgetLayout : WF_Layout widgetLayout := by
  unfold WF_Layout
  intro f hf
  simp [widgetLayout] at hf
  rcases hf with rfl
  simp [widgetLayout]

/-! ## §2. Initial execution state (models `arena_new(4096)`).

The `.vuma` program's first statement `let arena = arena_new(4096)`
lowers to `mmap(NULL, 4096, ...)` + a 24-byte `Arena` struct
(`{ base, offset, capacity }`). In the Lean PMT model this is the
initial `ExecState`: a fresh arena with `used = 0` and every variable
live (no consumption yet). We use `capacity = 1024` (smaller than the
`.vuma`'s 4096, but still far larger than the 16 bytes the program
actually allocates). -/

/-- Initial state: a fresh 1024-byte arena (`base=0, capacity=1024,
used=0`) with every variable live. -/
def initState : ExecState :=
  { arena := ⟨0, 1024, 0⟩,
    live  := fun _ => Liveness.live }

/-- The initial arena satisfies the capacity invariant: `0 ≤ 1024`. -/
example : CapacityInvariant initState.arena := by
  unfold CapacityInvariant initState
  decide

/-! ## §3. The `arena_basic.vuma` program, modeled as a Lean `Program`.

Each `Step` corresponds to one `.vuma` statement that lowers to a PMT
`state_transform` (consume input state, produce output state). Variable
names are unique per step to satisfy `WellTyped`'s name-uniqueness
conjunct. -/

/-- The 4-step program mirroring `arena_basic.vuma`:

  * Step 1: `arena_alloc(arena, Widget)` — `arena → w`
    (bump-allocate a Widget; `Arena::alloc<T>` in Rust).
  * Step 2: `w.x = 42` — `w → w_store`
    (store to field x; IVE `verify_state_writes`).
  * Step 3: `let val = w.x` — `w_store → w_load`
    (load from field x; IVE `verify_state_reads`).
  * Step 4: `arena_free(arena)` — `w_load → freed`
    (destroy arena; `Arena::destroy` / Drop; IVE `verify_transform`).

The final `freed` sink models the post-`arena_free` state — the
variable is still "live" in the Lean ghost-state (the `step` function
makes `out_var` live), but semantically it represents the freed arena.
This is a sound over-approximation: the Lean model does not track that
`freed` should never be accessed again; the Rust runtime enforces this
via `Drop`. -/
def arenaBasicProg : Program :=
  [ ⟨"arena", "w", widgetLayout, .transform⟩,
    ⟨"w", "w_store", widgetLayout, .transform⟩,
    ⟨"w_store", "w_load", widgetLayout, .transform⟩,
    ⟨"w_load", "freed", widgetLayout, .transform⟩ ]

/-! ## §4. Execution succeeds, advancing `used` by 16 bytes (4 × 4).

`exec arenaBasicProg initState = Result.ok 16`. The bump pointer
advances `0 → 4 → 8 → 12 → 16`. Each step's guards pass:

  * Step 1 (`arena → w`): `initState.live "arena" = Liveness.live`
    (constant function), so the UAF guard fails. `0 + 4 ≤ 1024`, so
    the overflow guard fails.
  * Step 2 (`w → w_store`): after step 1, `live "w" = Liveness.live`
    (just made live); `4 + 4 ≤ 1024`.
  * Step 3 (`w_store → w_load`): after step 2, `live "w_store" = live`;
    `8 + 4 ≤ 1024`.
  * Step 4 (`w_load → freed`): after step 3, `live "w_load" = live`;
    `12 + 4 ≤ 1024`.

The reduction is definitional: `exec`, `step`, `DecidableEq Liveness`,
`Nat` decidable comparison, and `DecidableEq String` (literal strings)
all reduce, so `rfl` closes the goal. -/

example : exec arenaBasicProg initState = Result.ok 16 := by
  rfl

/-! ## §5. The program is well-typed.

`WellTyped arenaBasicProg` holds:
  * Every step's layout is `WF_Layout` (all `widgetLayout`).
  * Each `in_var` name appears exactly once across steps.
  * Each `out_var` name appears exactly once across steps.

All three conjuncts are discharged by `simp [arenaBasicProg]` (which
unfolds the program list and evaluates `List.filter` on the literal
`String` BEq) plus `wf_widgetLayout` for the layout conjunct. -/

example : WellTyped arenaBasicProg := by
  unfold WellTyped
  refine ⟨?_, ?_, ?_⟩
  · -- All layouts are well-formed.
    intro st hst
    simp [arenaBasicProg] at hst
    rcases hst with rfl | rfl | rfl | rfl
    all_goals exact wf_widgetLayout
  · -- in_var uniqueness: each step's in_var appears exactly once.
    intro st hst
    simp [arenaBasicProg] at hst
    rcases hst with rfl | rfl | rfl | rfl
    all_goals simp [arenaBasicProg]
  · -- out_var uniqueness: each step's out_var appears exactly once.
    intro st hst
    simp [arenaBasicProg] at hst
    rcases hst with rfl | rfl | rfl | rfl
    all_goals simp [arenaBasicProg]

/-! ## §6. Sanity: the final bump pointer is within capacity.

This is the post-condition that `pmt_soundness` guarantees for
well-typed programs: the final `Result.ok final_used` satisfies
`final_used ≤ capacity`. Here we instantiate it concretely by
chaining the top-level `exec` example with decidable arithmetic. -/

example : match exec arenaBasicProg initState with
          | Result.ok fu => fu ≤ initState.arena.capacity
          | Result.trap _ => True := by
  have h : exec arenaBasicProg initState = Result.ok 16 := by
    rfl
  rw [h]
  decide

end PMT.Test.ArenaBasicSim
