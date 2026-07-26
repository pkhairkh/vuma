import PMT.RawArena
import PMT.Basic

/-! ## Arena Properties — additional lemmas about RawArena

These lemmas establish properties of the `RawArena` model (defined in
`PMT.RawArena`) that support the simulation relation and extraction
work. They are companions to the lemmas already in `PMT.RawArena`
(`align8_multiple_of_8`, `align8_ge`, `raw_alloc_preserves_wf`,
`raw_alloc_simulates_alloc`).

Each theorem below corresponds to a property of the underlying Rust
arena (`src/codegen/src/runtime/arena.rs`):
  - `align8_idempotent`, `align8_preserves_aligned`: alignment math.
  - `raw_alloc_destroyed_fails`, `raw_alloc_alive_succeeds`: lifecycle
    guards on `alloc_raw`.
  - `raw_destroy_marks_destroyed`: `destroy` is a one-way transition.
  - `raw_grow_preserves_alive`: `grow` retains the alive phase.
  - `raw_alloc_preserves_wf_raw`: alias / re-statement of
    `raw_alloc_preserves_wf` (kept under a `_raw` suffix so callers
    targeting the faithful model have a matching name).
-/

namespace PMT

/-- §1: `align8_nat` is idempotent — aligning an already-aligned size
is a no-op. Mirrors the Rust `(size + 7) & !7` bitmask being stable
under re-application. -/
theorem align8_idempotent (size : Nat) :
    align8_nat (align8_nat size) = align8_nat size := by
  unfold align8_nat
  omega

/-- §2: `align8_nat` preserves the multiple-of-8 property — if `size`
is already a multiple of 8, alignment is a no-op. Used to bridge the
`align8_nat size = size` gap in the simulation relation. -/
theorem align8_preserves_aligned (size : Nat)
    (haligned : size % 8 = 0) :
    align8_nat size = size := by
  unfold align8_nat
  omega

/-- §3: `raw_alloc` on a destroyed arena fails. The phase guard in
`alloc_raw` (arena.rs line 92) trips, returning `arena_overflow`
(which in Rust is `std::process::abort()`). -/
theorem raw_alloc_destroyed_fails
    (a : RawArena) (size : Nat)
    (hdestroyed : a.phase = ArenaPhase.destroyed) :
    raw_alloc a size = Except.error TrapCode.arena_overflow := by
  unfold raw_alloc
  have hcond : a.phase ≠ ArenaPhase.alive := by
    intro h
    rw [hdestroyed] at h
    exact absurd h (by decide)
  rw [if_pos hcond]

/-- §4: `raw_alloc` on an alive arena with sufficient space succeeds.
The resulting arena's `offset` advances by `align8_nat size`, and the
`phase` remains `alive`. -/
theorem raw_alloc_alive_succeeds
    (a : RawArena) (size : Nat)
    (halive : a.phase = ArenaPhase.alive)
    (hfit : a.offset + align8_nat size ≤ a.capacity) :
    ∃ a', raw_alloc a size = Except.ok a'
      ∧ a'.offset = a.offset + align8_nat size
      ∧ a'.phase = ArenaPhase.alive := by
  unfold raw_alloc
  have hcond1 : ¬ (a.phase ≠ ArenaPhase.alive) := fun h => h halive
  have hcond2 : ¬ (a.offset + align8_nat size > a.capacity) := by omega
  rw [if_neg hcond1, if_neg hcond2]
  refine ⟨{ a with offset := a.offset + align8_nat size }, ?_, ?_, ?_⟩
  · rfl
  · rfl
  · exact halive

/-- §5: `raw_destroy` marks the arena as destroyed. The `phase` field
becomes `ArenaPhase.destroyed`, preventing further `raw_alloc` /
`raw_grow` (the phase guards in those functions trip). -/
theorem raw_destroy_marks_destroyed
    (a : RawArena) :
    (raw_destroy a).phase = ArenaPhase.destroyed := by
  rfl

/-- §6: `raw_grow` preserves the alive phase. Given a successful grow
(`new_cap > capacity`, `phase = alive`), the resulting arena has phase
`alive` and `capacity = new_cap`. -/
theorem raw_grow_preserves_alive
    (a : RawArena) (new_cap : Nat)
    (halive : a.phase = ArenaPhase.alive)
    (hgrow : new_cap > a.capacity) :
    ∃ a', raw_grow a new_cap = Except.ok a'
      ∧ a'.phase = ArenaPhase.alive
      ∧ a'.capacity = new_cap := by
  unfold raw_grow
  by_cases hle : new_cap ≤ a.capacity
  · exfalso
    omega
  · rw [if_neg hle]
    have hcond : ¬ (a.phase ≠ ArenaPhase.alive) := fun h => h halive
    rw [if_neg hcond]
    refine ⟨{ a with base := a.base + a.capacity, capacity := new_cap, layout := { size := new_cap, align := 8 } }, ?_, ?_, ?_⟩
    · rfl
    · exact halive
    · rfl

/-- §7: `WF_RawArena` is preserved by `raw_alloc`. This is a faithful
companion to `PMT.Basic.alloc_preserves_capacity` and a named alias of
`PMT.RawArena.raw_alloc_preserves_wf` (kept under a `_raw` suffix for
callers that want the faithful-model name). -/
theorem raw_alloc_preserves_wf_raw
    (a : RawArena) (size : Nat)
    (hwf : WF_RawArena a)
    (hfit : a.offset + align8_nat size ≤ a.capacity) :
    ∀ a', raw_alloc a size = Except.ok a' → WF_RawArena a' := by
  intro a' hresult
  unfold raw_alloc at hresult
  by_cases hphase : a.phase ≠ ArenaPhase.alive
  · rw [if_pos hphase] at hresult
    cases hresult
  · rw [if_neg hphase] at hresult
    by_cases hovf : a.offset + align8_nat size > a.capacity
    · rw [if_pos hovf] at hresult
      cases hresult
    · rw [if_neg hovf] at hresult
      injection hresult with hval
      subst hval
      have halive : a.phase = ArenaPhase.alive :=
        Decidable.byContradiction hphase
      unfold WF_RawArena at hwf ⊢
      refine ⟨?_, ?_, ?_, ?_⟩
      · -- offset ≤ capacity (new offset = a.offset + align8_nat size)
        exact hfit
      · -- layout.align = 8 (unchanged by offset-only update)
        exact hwf.2.1
      · -- destroyed → ...: vacuous, since phase = .alive ≠ .destroyed
        intro hdest
        exfalso
        have hdest' : a.phase = ArenaPhase.destroyed := hdest
        rw [halive] at hdest'
        exact absurd hdest' (by decide)
      · -- layout.size = capacity (both unchanged by offset-only update)
        exact hwf.2.2.2

end PMT
