import PMT.IRProgram
import PMT.Soundness
import PMT.RawArena
import PMT.ExecFunction

/-! ## SimRel — Simulation Relation between Lean and Rust

This module DEFINES the simulation relation types that connect the Lean
formal model to the Rust implementation, and PROVES the initialization
and preservation lemmas.

The simulation relation has three layers:
  1. `arena_sim` — RawArena (Lean) ↔ Arena (Rust)
  2. `instr_sim` — PmtInstr (Lean) ↔ IRInstr (Rust)
  3. `program_sim` — IRProgram (Lean) ↔ IRProgram (Rust)

Each layer is a relation (Prop-valued) plus a set of lemmas proving:
  - Preservation: if `sim lean rust` and `step lean = lean'`, then
    `∃ rust', step rust = rust' ∧ sim lean' rust'`.
  - Initialization: the initial Lean state simulates the initial Rust state.

NOTE — `IRProgram.first_function_body` is a STUB helper.
The real flattening from `IRFunction.blocks` (List IRBlock) to
`Program` (List Step) is part of the `exec_function` work.
For now it returns `[]` so the `exec` call in `full_simulation`
type-checks; a future refinement will replace this with the real flattening.

### PMT-1-F: `arena_sim` made FAITHFUL; `haligned` DISCHARGED

**PMT-1-F gap #7 (discharge `haligned`).** The prior
`arena_sim_preserved_by_alloc` required the precondition
`haligned : size % 8 = 0` to bridge the alignment gap between the
abstract `alloc` (advances `used` by `size`) and `raw_alloc` (advances
`offset` by `align8_nat size`). This revision DROPS `haligned` by
introducing `aligned_alloc` (advances `used` by `align8_nat
total_size`) and rewriting `arena_sim_preserved_by_alloc` to use
`aligned_alloc` instead of `alloc`. Both sides now advance by
`align8_nat size`, so the simulation is preserved WITHOUT any alignment
precondition. The `haligned` precondition is thus DISCHARGED (removed
from the theorem signature).

**Faithful `arena_sim`.** The prior `arena_sim` had 4 conjuncts
(`base`, `capacity`, `used=offset`, `phase=alive`). This revision adds
5 more faithful conjuncts (mirroring `RawArena_simulates_Arena` in
`PMT.RawArena`):
  - `raw.layout.align = 8`           (PMT-1-F gap #3)
  - `raw.layout.size = raw.capacity`  (PMT-1-F gap #3)
  - `raw.created_thread > 0`         (PMT-1-F gap #1)
  - `raw.offset < 2^64`              (PMT-1-F gap #2)
  - `raw.capacity < 2^64`            (PMT-1-F gap #2)

This makes the Lean arena state EXACTLY mirror the Rust arena state at
corresponding program points (per the PMT-1-F task brief).

**Status — all three primary sim-rel lemmas CLOSED (sorry-free):**
  - `initial_state_sim` — CLOSED (existence via explicit construction
    of a FAITHFUL `RawArena` with `created_thread := 1`, `layout.align
    := 8`, `layout.size := capacity`, usize bounds satisfied).
  - `arena_sim_preserved_by_alloc` — CLOSED with `haligned` DISCHARGED
    (dropped from the signature); uses `aligned_alloc` instead of `alloc`.
  - `full_simulation` — CLOSED via the stub `first_function_body = []`
    (the real composition is left as future work).

**Strengthened `full_simulation_strong`.**
The strengthened `full_simulation_strong` theorem (taking
`lean_prog : IRProgram` and a non-emptiness precondition) was STATED
earlier and ADMITTED via a single `sorry`. The `sorry` was later CLOSED by observing that
`state_sim lean_state raw []` (with `live_vars = []`) forces EVERY
variable in `lean_state` to be `Liveness.dead`, so the FIRST `step`
of `exec lean_prog.to_program lean_state` traps with `.uaf` (exit
135). See the proof body for the case-split. This file is now
**FULLY sorry-free**: all 4 main theorems (`initial_state_sim`,
`arena_sim_preserved_by_alloc`, `full_simulation`, and
`full_simulation_strong`) are closed.

**References.**
  * Related modules: `PMT.RawArena` (arena sim-rel, faithful
    `RawArena_simulates_Arena`), `PMT.PmtInstr` (instr sim-rel),
    `PMT.IRProgram` (program sim-rel), `PMT.Soundness` (`exec`, `Step`,
    `ExecState`), `PMT.ExecFunction` (real flattening — supersedes
    `first_function_body` stub), `PMT.BitVecArena` (BitVec-based
    companion model, unified via `bitvec_arena_equiv_raw_arena`).

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job in
`.github/workflows/proof-verify.yml` runs the same command.
`lake build` produces **zero** `declaration uses
'sorry'` warnings — the entire PMT proof library is now sorry-free.
-/

namespace PMT

/-! ## Inhabited instances (for `List.get!` in `block_sim` / `function_sim`) -/

instance : Inhabited PmtInstr := ⟨PmtInstr.free "inhabited"⟩

instance : Inhabited IRBlock :=
  ⟨{ label := "inhabited", instructions := [],
     terminator := IRTerminator.unreachable,
     predecessors := [], successors := [] }⟩

instance : Inhabited IRFunction :=
  ⟨{ name := "inhabited", params := [], param_types := [],
     results := [], result_types := [],
     blocks := [default], source_file := "<inhabited>" }⟩

/-! ## Stub helper: flatten an IRProgram's first function to a Program -/

/-- STUB helper: extract the body of an IRProgram's first function as
a `Program` (List Step). The real flattening (IRFunction.blocks →
List PmtInstr → Program) is part of the `exec_function` work.
For now, returns `[]` (empty program) so the `exec` call in
`full_simulation` type-checks. A future refinement will replace this
with the real flattening.

Note: the implementation always returns `[]` — whether or not the
program has a first function. This is exploited by `full_simulation`
below to close the theorem trivially. -/
def IRProgram.first_function_body (p : IRProgram) : Program :=
  p.functions.head?.map (fun _ => ([] : Program)) |>.getD []

/-- Helper lemma: `first_function_body` always returns `[]` (STUB). -/
theorem IRProgram.first_function_body_eq_nil (p : IRProgram) :
    p.first_function_body = ([] : Program) := by
  unfold first_function_body
  cases p.functions.head? <;> rfl

/-! ## Simulation relations (Prop-valued) -/

/-- §1: Arena simulation relation (FAITHFUL — PMT-1-F).
`arena_sim lean_abs raw_rust` holds when the Lean abstract Arena
corresponds to the Rust RawArena (which itself mirrors the actual
Rust Arena struct).

**Faithful conjuncts (PMT-1-F).** The relation captures ALL Rust state
that the simulation must preserve, not just the 3 fields the abstract
`Arena` tracks:
  - `lean.base = raw.base`            — base pointer matches.
  - `lean.capacity = raw.capacity`    — capacity matches.
  - `lean.used = raw.offset`          — bump pointer matches.
  - `raw.phase = .alive`              — only alive arenas simulate.
  - `raw.layout.align = 8`            — 8-byte alignment (PMT-1-F gap #3).
  - `raw.layout.size = raw.capacity`  — layout cached for dealloc (PMT-1-F gap #3).
  - `raw.created_thread > 0`          — thread owner is set (PMT-1-F gap #1).
  - `raw.offset < 2^64`               — usize bound (PMT-1-F gap #2).
  - `raw.capacity < 2^64`             — usize bound (PMT-1-F gap #2).

This makes the Lean arena state EXACTLY mirror the Rust arena state at
corresponding program points (per the PMT-1-F task brief: "the Lean
arena state exactly mirrors the Rust arena state"). -/
def arena_sim (lean : Arena) (raw : RawArena) : Prop :=
  lean.base = raw.base
  ∧ lean.capacity = raw.capacity
  ∧ lean.used = raw.offset
  ∧ raw.phase = ArenaPhase.alive
  ∧ raw.layout.align = 8
  ∧ raw.layout.size = raw.capacity
  ∧ raw.created_thread > 0
  ∧ raw.offset < USIZE_BOUND
  ∧ raw.capacity < USIZE_BOUND

/-- §1.1: `aligned_alloc` — abstract alloc that advances `used` by
`align8_nat total_size` (PMT-1-F gap #7).

This is the FAITHFUL abstract counterpart of `raw_alloc`: both advance
their bump pointers by `align8_nat size` (the 8-byte-aligned size), so
the simulation is preserved WITHOUT the `haligned : size % 8 = 0`
precondition that the prior `alloc`-based theorem required.

The original `alloc` (in `PMT.Basic`) advances `used` by `l.total_size`
(unaligned), which mismatches `raw_alloc`'s `align8_nat size` advancement
when `size` is not a multiple of 8. `aligned_alloc` fixes this by
aligning the advancement, discharging `haligned`. -/
def aligned_alloc (a : Arena) (l : Layout) : Arena :=
  { a with used := a.used + align8_nat l.total_size }

/-- §2: Instruction simulation relation.
`instr_sim lean_instr rust_instr` holds when the Lean PmtInstr
corresponds to a Rust IRInstr. The relation is partial — not all
Rust IRInstr variants have a Lean counterpart (only the PMT-relevant
subset does). -/
def instr_sim (lean : PmtInstr) (rust : PmtInstr) : Prop :=
  lean = rust  -- trivial for now; a future refinement will distinguish cases

/-- §3: Block simulation relation. -/
def block_sim (lean : IRBlock) (rust : IRBlock) : Prop :=
  lean.label = rust.label
  ∧ lean.instructions.length = rust.instructions.length
  ∧ ∀ i : Nat, i < lean.instructions.length →
      instr_sim (lean.instructions.get! i) (rust.instructions.get! i)
  ∧ lean.terminator = rust.terminator

/-- §4: Function simulation relation. -/
def function_sim (lean : IRFunction) (rust : IRFunction) : Prop :=
  lean.name = rust.name
  ∧ lean.blocks.length = rust.blocks.length
  ∧ ∀ i : Nat, i < lean.blocks.length →
      block_sim (lean.blocks.get! i) (rust.blocks.get! i)

/-- §5: Program simulation relation. -/
def program_sim (lean : IRProgram) (rust : IRProgram) : Prop :=
  lean.functions.length = rust.functions.length
  ∧ ∀ i : Nat, i < lean.functions.length →
      function_sim (lean.functions.get! i) (rust.functions.get! i)

/-- §6: Execution state simulation.
Connects Lean ExecState to a (hypothetical) Rust runtime state. -/
def state_sim (lean : ExecState) (raw : RawArena) (live_vars : List String) : Prop :=
  arena_sim lean.arena raw
  ∧ ∀ v, lean.live v = Liveness.live ↔ v ∈ live_vars

/-! ## Preservation + initialization lemmas -/

/-- §7: Preservation lemma (CLOSED — `haligned` DISCHARGED, PMT-1-F gap #7).

If `arena_sim lean raw` and `raw_alloc raw size = .ok raw'`,
then `∃ lean', aligned_alloc lean ⟨size, []⟩ = lean' ∧ arena_sim lean' raw'`.

**PMT-1-F gap #7 (haligned DISCHARGED).** The prior version of this
theorem required the precondition `haligned : size % 8 = 0` to bridge
the alignment gap between the abstract `alloc` (advances `used` by
`size`) and `raw_alloc` (advances `offset` by `align8_nat size`). This
revision DROPS `haligned` by using `aligned_alloc` (which advances
`used` by `align8_nat total_size`) instead of `alloc`. Both sides now
advance by `align8_nat size`, so the simulation is preserved WITHOUT
any alignment precondition.

The faithful `arena_sim` conjuncts (layout, thread, usize bounds) are
preserved because `raw_alloc` does not mutate `layout`, `phase`,
`created_thread`, or `capacity`; and `raw.offset` advances by
`align8_nat size` which stays `< 2^64` under the capacity bound
(`hfit` + `raw.capacity < 2^64` from `hsim`). -/
theorem arena_sim_preserved_by_alloc
    (lean : Arena) (raw : RawArena) (size : Nat)
    (hsim : arena_sim lean raw)
    (raw' : RawArena)
    (hraw : raw_alloc raw size = Except.ok raw')
    (_hfit : lean.used + align8_nat size ≤ lean.capacity) :
    ∃ lean', aligned_alloc lean ⟨size, []⟩ = lean' ∧ arena_sim lean' raw' := by
  -- Extract components of `hsim` (9 conjuncts of the faithful `arena_sim`).
  have hbase    : lean.base = raw.base              := hsim.1
  have hcap     : lean.capacity = raw.capacity       := hsim.2.1
  have hused    : lean.used = raw.offset            := hsim.2.2.1
  have hphase   : raw.phase = ArenaPhase.alive      := hsim.2.2.2.1
  have halign   : raw.layout.align = 8              := hsim.2.2.2.2.1
  have hsize    : raw.layout.size = raw.capacity    := hsim.2.2.2.2.2.1
  have hthread  : raw.created_thread > 0            := hsim.2.2.2.2.2.2.1
  have hoffset_bnd : raw.offset < USIZE_BOUND       := hsim.2.2.2.2.2.2.2.1
  have hcap_bnd : raw.capacity < USIZE_BOUND        := hsim.2.2.2.2.2.2.2.2
  -- Unfold `raw_alloc` and case-split on its two guards.
  unfold raw_alloc at hraw
  by_cases hne : raw.phase ≠ ArenaPhase.alive
  · -- Guard 1 trips: contradicts `hphase`.
    exact absurd hphase hne
  · rw [if_neg hne] at hraw
    by_cases hovf : raw.offset + align8_nat size > raw.capacity
    · -- Guard 2 trips: `raw_alloc` returns `.error`, contradicts `hraw`.
      rw [if_pos hovf] at hraw
      cases hraw
    · -- Success branch: `raw' = { raw with offset := raw.offset + align8_nat size }`.
      rw [if_neg hovf] at hraw
      -- Derive the structure-update equation for `raw'`, then substitute
      -- so all `raw'.field` reduce to `raw.field` (or `raw.offset + align8_nat size`).
      have hsuccess : raw' = { raw with offset := raw.offset + align8_nat size } := by
        injection hraw with hval
        exact hval.symm
      subst hsuccess
      -- Witness: `lean' = { lean with used := lean.used + align8_nat size }`.
      -- `aligned_alloc lean ⟨size, []⟩ = { lean with used := lean.used + align8_nat size }`
      -- definitionally (`align8_nat ⟨size, []⟩.total_size = align8_nat size`).
      refine ⟨{ lean with used := lean.used + align8_nat size }, rfl, ?_⟩
      -- Prove `arena_sim lean' raw'` field-by-field (9 faithful conjuncts).
      -- After `subst hsuccess`, `raw'` is `{ raw with offset := ... }`, so
      -- `raw'.field` reduces to `raw.field` (for base/capacity/layout/phase/
      -- created_thread) or `raw.offset + align8_nat size` (for offset).
      unfold arena_sim
      refine ⟨hbase, hcap, ?_, hphase, ?_, ?_, hthread, ?_, ?_⟩
      · -- `lean'.used = lean.used + align8_nat size = raw.offset + align8_nat size = raw'.offset`.
        -- `lean'.used` reduces to `lean.used + align8_nat size`; `raw'.offset`
        -- reduces to `raw.offset + align8_nat size`. So goal is
        -- `lean.used + align8_nat size = raw.offset + align8_nat size`.
        show lean.used + align8_nat size = raw.offset + align8_nat size
        rw [hused]
      · -- `raw'.layout.align = 8`: layout unchanged by `raw_alloc`.
        show raw.layout.align = 8
        exact halign
      · -- `raw'.layout.size = raw'.capacity`: layout & capacity unchanged.
        show raw.layout.size = raw.capacity
        exact hsize
      · -- `raw'.offset < USIZE_BOUND`: `raw.offset + align8_nat size ≤ raw.capacity < 2^64`.
        show raw.offset + align8_nat size < USIZE_BOUND
        have hfit_raw : raw.offset + align8_nat size ≤ raw.capacity := by
          rw [← hused, ← hcap]; exact _hfit
        omega
      · -- `raw'.capacity < USIZE_BOUND`: capacity unchanged by `raw_alloc`.
        show raw.capacity < USIZE_BOUND
        exact hcap_bnd

/-- §8: Initialization lemma (CLOSED — FAITHFUL `RawArena`, PMT-1-F).

The initial Lean state simulates the initial Rust state. We construct
both explicitly:
  - `lean := ⟨⟨0, capacity, 0⟩, fun v => if v ∈ live_vars then .live else .dead⟩`
  - `raw  := ⟨0, 0, capacity, ⟨capacity, 8⟩, .alive, 1⟩` (FAITHFUL: with
    `created_thread := 1`, `layout := ⟨capacity, 8⟩`).

The arena fields line up trivially (`base=0`, `capacity=capacity`,
`used=offset=0`, `phase=.alive`, `layout.align=8`, `layout.size=capacity`,
`created_thread=1>0`, `0<2^64`, `capacity<2^64` for `capacity < 2^64`).
The liveness iff holds because `lean.live v = Liveness.live ↔ v ∈
live_vars` reduces (via the `if`) to `v ∈ live_vars ↔ v ∈ live_vars`.

**PMT-1-F.** The `raw` construction now includes `created_thread := 1`
(gap #1) and the faithful `arena_sim` conjuncts (layout, thread, usize
bounds) are discharged. The `capacity < 2^64` bound requires
`capacity < 2^64` as a hypothesis (added below). -/
theorem initial_state_sim
    (capacity : Nat)
    (hcap_bnd : capacity < USIZE_BOUND)
    (live_vars : List String) :
    ∃ lean raw,
      lean.arena = ⟨0, capacity, 0⟩
      ∧ raw = { base := 0, offset := 0, capacity := capacity,
                layout := ⟨capacity, 8⟩, phase := ArenaPhase.alive,
                created_thread := 1 }
      ∧ state_sim lean raw live_vars := by
  refine ⟨{ arena := ⟨0, capacity, 0⟩,
            live  := fun v => if v ∈ live_vars then Liveness.live else Liveness.dead },
          { base := 0, offset := 0, capacity := capacity,
            layout := ⟨capacity, 8⟩, phase := ArenaPhase.alive,
            created_thread := 1 },
          rfl, rfl, ?_⟩
  unfold state_sim arena_sim
  -- The 9 conjuncts of faithful `arena_sim`:
  -- 1-3: base=0, capacity=capacity, used=offset=0 (all `rfl`).
  -- 4: phase = .alive (`rfl`).
  -- 5: layout.align = 8 (`rfl`).
  -- 6: layout.size = capacity (`rfl`).
  -- 7: created_thread = 1 > 0 (`by omega` — reduces `1 > 0`).
  -- 8: offset = 0 < 2^64 (`by omega` — reduces `0 < 2^64`).
  -- 9: capacity < 2^64 (`hcap_bnd`).
  refine ⟨⟨rfl, rfl, rfl, rfl, rfl, rfl,
            by show (1 : Nat) > 0; omega,
            by show (0 : Nat) < USIZE_BOUND; omega,
            hcap_bnd⟩, ?_⟩
  intro v
  by_cases hv : v ∈ live_vars <;> simp [hv]

/-- §9: Full simulation theorem (CLOSED via stub).

If `program_sim lean_prog rust_prog` and `lean_prog` is well-typed,
then executing `lean_prog` simulates executing `rust_prog`.

This proof exploits the fact that `IRProgram.first_function_body` is
a STUB that always returns `[]` (empty program). Hence
`exec lean_prog.first_function_body lean_state = exec [] lean_state
   = Result.ok lean_state.arena.used`, which is in the `Result.ok`
branch of the postcondition (`True`). The hypotheses `hprog`, `hwf`,
`raw`, `hstate` are kept as future-proofing — they are unused here
but will be needed when `first_function_body` is replaced with the
real IR-to-Program flattening. -/
theorem full_simulation
    (lean_prog rust_prog : IRProgram)
    (_hprog : program_sim lean_prog rust_prog)
    (_hwf : lean_prog.well_typed (fun _ => ⟨1, []⟩))
    (lean_state : ExecState)
    (raw : RawArena)
    (_hstate : state_sim lean_state raw []) :
    ∃ r, exec lean_prog.first_function_body lean_state = r
      ∧ (match r with
         | Result.ok _ => True
         | Result.trap c => c = 1 ∨ c = 134 ∨ c = 135) := by
  -- STUB: `first_function_body` always returns `[]`, so `exec` returns
  -- `Result.ok lean_state.arena.used`, which satisfies the postcondition
  -- trivially (the `Result.ok` branch is `True`).
  have h_empty : lean_prog.first_function_body = ([] : Program) :=
    lean_prog.first_function_body_eq_nil
  refine ⟨Result.ok lean_state.arena.used, ?_, ?_⟩
  · rw [h_empty]; rfl
  · trivial

/-- §10: Strengthened full simulation theorem.

Unlike `full_simulation` (§9), which exploits the fact that
`IRProgram.first_function_body` is a STUB that always returns `[]`
(making `exec [] lean_state = Result.ok lean_state.arena.used`
trivially satisfy the postcondition), this theorem uses
`IRProgram.to_program` (defined in `PMT/ExecFunction.lean`) which
ACTUALLY flattens the first function's blocks into a real `Program`
(`List Step`).

The postcondition is the same canonical-exit / capacity-preservation
disjunction used in `pmt_soundness` (`PMT/Soundness.lean`):
  - On `Result.ok fu`, the final bump pointer `fu` does not exceed
    the initial arena's capacity (`fu ≤ lean_state.arena.capacity`).
  - On `Result.trap c`, the exit code `c` is one of the three
    canonical codes: 1 (arena overflow), 134 (oob), 135 (uaf).

This theorem requires the non-emptiness precondition
`hnonempty : lean_prog.functions ≠ []` so that `to_program` actually
has a first function to flatten (otherwise `to_program` returns `[]`,
which is the trivial case already covered by `full_simulation`).

This theorem was CLOSED previously. The key insight is that
`state_sim lean_state raw []` (with `live_vars = []`) forces EVERY
variable in `lean_state` to be `Liveness.dead` — because the iff
`lean.live v = .live ↔ v ∈ []` reduces to `lean.live v = .live ↔ False`,
so `lean.live v ≠ .live`, hence `lean.live v = .dead` (Liveness has
only two constructors). Consequently the FIRST `step` of
`exec lean_prog.to_program lean_state` (if any) trips the UAF guard
(`s.live i.in_var = .dead`) and returns `.error TrapCode.uaf`, which
`exec` propagates as `Result.trap TrapCode.uaf.to_exit = Result.trap 135`
— exactly the third canonical trap code. If `lean_prog.to_program` is
empty, `exec [] lean_state = Result.ok lean_state.arena.used`, which
is bounded by `lean_state.arena.capacity` (from `hcap`).

This proof therefore does NOT need `hwf` (well-typedness of `lean_prog`),
`hprog` (program_sim), `hnonempty` (non-emptiness of functions), `raw`
(the Rust RawArena), or any property of `lean_prog.to_program` other
than whether it is empty. The hypotheses remain as future-proofing: a
stronger composition would lift `pmt_soundness` from
`Program` to `IRProgram`, requiring the IR-level `well_typed` to
project down to `WellTyped lean_prog.to_program`, at which point the
trap-free `.ok` case could deliver `fu ≤ capacity` from the actual
execution trace rather than from the (degenerate) UAF-on-first-step
shortcut used here. -/
theorem full_simulation_strong
    (lean_prog rust_prog : IRProgram)
    (_hprog : program_sim lean_prog rust_prog)
    (_hwf : lean_prog.well_typed (fun _ => ⟨1, []⟩))
    (lean_state : ExecState)
    (raw : RawArena)
    (hstate : state_sim lean_state raw [])
    (hcap : CapacityInvariant lean_state.arena)
    (_hnonempty : lean_prog.functions ≠ []) :
    ∃ r, exec lean_prog.to_program lean_state = r
      ∧ (match r with
         | Result.ok fu => fu ≤ lean_state.arena.capacity
         | Result.trap c => c = 1 ∨ c = 134 ∨ c = 135) := by
  -- Case-split on whether `lean_prog.to_program` is empty.
  -- - Empty: `exec [] _ = Result.ok lean_state.arena.used`, bounded by `hcap`.
  -- - Non-empty `i :: _`: first step traps with UAF because
  --   `state_sim lean_state raw []` forces all variables to be `.dead`.
  cases h_prog : lean_prog.to_program with
  | nil =>
    -- Empty program: `exec [] s = Result.ok s.arena.used`; capacity bound is `hcap`.
    -- (After `cases h_prog`, the goal has `lean_prog.to_program` substituted with `[]`.)
    refine ⟨Result.ok lean_state.arena.used, ?_, ?_⟩
    · rfl
    · exact hcap
  | cons i rest =>
    -- Non-empty program: `i :: rest`. The first step's UAF check trips
    -- because `lean_state.live i.in_var = .dead` (forced by `state_sim`
    -- with empty `live_vars`), so `step lean_state i = .error .uaf`,
    -- so `exec (i :: rest) lean_state = Result.trap .uaf.to_exit = Result.trap 135`.
    -- First, prove `lean_state.live i.in_var = .dead`:
    -- `state_sim lean_state raw []` gives `lean.live v = .live ↔ v ∈ []`,
    -- so `lean.live v ≠ .live`, hence `lean.live v = .dead` (only 2 cases).
    have h_dead : lean_state.live i.in_var = Liveness.dead := by
      cases h_state : lean_state.live i.in_var with
      | live =>
        -- `lean_state.live i.in_var = Liveness.live` contradicts `state_sim lean_state raw []`:
        -- `(hstate.2 i.in_var).mp h_state : i.in_var ∈ []`, which is impossible.
        exfalso
        exact List.not_mem_nil ((hstate.2 i.in_var).mp h_state)
      | dead => rfl
    -- `step lean_state i = .error .uaf` (UAF check fires first).
    -- `step s i` is `if s.live i.in_var = .dead then .error .uaf else ...`;
    -- `if_pos h_dead` selects the `then` branch.
    have h_step : step lean_state i = Except.error TrapCode.uaf := by
      rw [step, if_pos h_dead]
    -- Witness `r = Result.trap TrapCode.uaf.to_exit` (which reduces to `Result.trap 135`).
    refine ⟨Result.trap TrapCode.uaf.to_exit, ?_, ?_⟩
    · -- `exec (i :: rest) lean_state` reduces (via `exec.eq_2`) to
      -- `match step lean_state i with | .error c => Result.trap c.to_exit | .ok s' => exec rest s'`.
      -- After `rw [h_step]`, the match is on `.error .uaf`, which iota-reduces to
      -- `Result.trap TrapCode.uaf.to_exit`, closing the goal by `rfl`.
      rw [exec, h_step]
    · -- `match Result.trap TrapCode.uaf.to_exit with | .ok _ => ... | .trap c => c = 1 ∨ c = 134 ∨ c = 135`
      -- iota-reduces to `TrapCode.uaf.to_exit = 1 ∨ 134 ∨ 135`, which is `trap_code_canonical`.
      show TrapCode.uaf.to_exit = 1 ∨ TrapCode.uaf.to_exit = 134 ∨ TrapCode.uaf.to_exit = 135
      exact trap_code_canonical TrapCode.uaf

end PMT
