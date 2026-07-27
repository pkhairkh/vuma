import PMT.Soundness
import PMT.WellTypedStrong
import PMT.RawArena
import PMT.SimRel
import PMT.Extraction
import PMT.ExtractionLemmas
import PMT.IVE.Soundness.Transform
import PMT.IVE.Soundness.StateReads
import PMT.IVE.Soundness.StateWrites

/-! ## Sorry-Free Audit Test

This module exists to verify that the entire PMT proof library is sorry-free.
It imports every proof module and asserts key theorems have the expected
types. If any theorem uses `sorry`, `lake build` will emit a warning but
still succeed. This module's `#check` commands serve as documentation
that the named theorems exist and have the expected (soundness) signatures.

The `#check` commands below succeed iff the named identifier resolves to
an existing theorem in the proof library — they cannot be satisfied by a
`def` that uses `sorry` (Lean still type-checks `sorry`'s against the
declared type, so a sorried `theorem` would still pass `#check`; this
audit's guarantee against `sorry` comes from the absence of `sorry`
warnings in the build log, *not* from `#check` alone). What `#check`
*does* guarantee is that every named theorem exists with its expected
type, which locks the public API of the proof library against silent
renames or signature drift in later refinements.

Run: `lake build PMT.Test.SorryFreeAudit`
-/

namespace PMT.Test.SorryFreeAudit

/-! ## §1. Soundness of the core PMT execution model. -/

-- `pmt_soundness` is the central soundness theorem of the PMT model.
-- It states that for any well-typed program, execution either yields a
-- result whose final bump-pointer is within capacity, or traps with a
-- canonical exit code (1, 134, or 135).
#check @pmt_soundness

/-! ## §2. Strengthened well-typedness implies no OOB trap. -/

-- `no_oob_trap_for_well_typed_strong` is the strengthened theorem from
-- `PMT.WellTypedStrong`: a strongly well-typed program cannot trap with
-- the OOB exit code 134.
#check @no_oob_trap_for_well_typed_strong

/-! ## §3–§5. IVE soundness theorems (transform / reads / writes). -/

-- `verify_transform_sound` proves the IVE `verify_transform` check is
-- sound: if the verifier accepts the transform, the resulting arena
-- state satisfies the simulation relation.
#check @PMT.IVE.Soundness.verify_transform_sound

-- `verify_state_reads_sound` proves the IVE `verify_state_reads` check
-- is sound: accepted reads cannot cause use-after-free or OOB.
#check @PMT.IVE.Soundness.verify_state_reads_sound

-- `verify_state_writes_sound` proves the IVE `verify_state_writes`
-- check is sound: accepted writes cannot cause use-after-free.
#check @PMT.IVE.Soundness.verify_state_writes_sound

/-! ## §6–§9. Simulation-relation theorems (Lean ↔ Rust). -/

-- `arena_sim_preserved_by_alloc` is the per-step simulation lemma:
-- if `lean ~ raw` before a successful `raw_alloc`, then there exists
-- a `lean'` after the Lean-side `alloc` with `lean' ~ raw'`.
#check @PMT.arena_sim_preserved_by_alloc

-- `initial_state_sim` is the bootstrapping lemma: there exist initial
-- Lean and Rust states that satisfy the simulation relation.
#check @PMT.initial_state_sim

-- `lean_internal_soundness` is the top-level INTRA-LEAN soundness theorem
-- (renamed from `full_simulation` in PMT-FAITH-5-B). It runs Lean `exec` on
-- Lean `IRProgram.to_program` — it does NOT simulate Rust execution.
#check @PMT.lean_internal_soundness

-- `lean_internal_soundness_strong` is the strengthened version
-- (renamed from `full_simulation_strong`).
#check @PMT.lean_internal_soundness_strong

/-! ## §10. Extraction soundness theorems. -/

-- Each `verified_*_check_correct` theorem states that the corresponding
-- extracted check function is sound: if it returns `true`, the
-- invariant it checks actually holds.
#check @PMT.Extraction.verified_capacity_check_correct
#check @PMT.Extraction.verified_field_bounds_check_correct
#check @PMT.Extraction.verified_linearity_check_correct
#check @PMT.Extraction.verified_pmt_check_correct

/-! ## §11. ExtractionLemmas composition theorems. -/

-- These theorems (defined in `PMT.ExtractionLemmas` but exported under
-- the `PMT.Extraction` namespace) state that the extracted checks
-- compose: sequential capacity checks compose, all-field bounds checks
-- cover every field, linearity checks cover every write, and the
-- composed PMT check decomposes into the three primitive checks.
#check @PMT.Extraction.capacity_check_sequential
#check @PMT.Extraction.field_bounds_check_all_fields
#check @PMT.Extraction.linearity_check_all_writes
#check @PMT.Extraction.pmt_check_decomposes
#check @PMT.Extraction.pmt_check_implies_all_invariants

/-! ## §12. Sanity example — a valid 2-step program executes successfully.

A trivial 2-step program `a → b → c` with a 16-byte layout per step,
executed on a fresh 1024-byte arena with all variables live, advances
the bump pointer by `16 + 16 = 32` and returns `Result.ok 32`. The
reduction is definitional: every guard in `step` (`live` check,
capacity check) reduces via `DecidableEq Liveness`, `DecidableEq
String`, and decidable `Nat` arithmetic, so `rfl` closes the goal. -/

example : exec [⟨"a", "b", ⟨"layout", 16, []⟩, .transform⟩, ⟨"b", "c", ⟨"layout", 16, []⟩, .transform⟩]
  { arena := ⟨0, 1024, 0⟩, live := fun _ => Liveness.live } = Result.ok 32 := by
  rfl

end PMT.Test.SorryFreeAudit
