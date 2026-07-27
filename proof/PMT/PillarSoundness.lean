import PMT.Basic
import PMT.Soundness
import PMT.WellTypedStrong
import PMT.IRProgram
import PMT.ExecFunction
import PMT.SimRel
import PMT.Iris.HeapModel
import PMT.Iris.CapBndInvariant
import PMT.Iris.LiveMirrorInvariant

/-!
## PillarSoundness — the PMT pillar theorem `pmt_pillar_sound` (sorry-free)

This module states and proves the **PMT pillar theorem**:
`pmt_pillar_sound`. The theorem says: for any VUMA program `P` with
no extern calls (`NoExterns P`) that is well-typed at the IR level
(`P.well_typed env`) and whose flattened program satisfies the
`DataflowOk` and `CapacityInvariant` hypotheses, the Lean execution
of `P`'s flattened program is memory-safe — specifically:

  * **No OOB trap** (exit code 134): the `__oob_trap` is unreachable
    because every `Step.in_var` is live (`WellTypedStrong` + the
    liveness invariant).
  * **Capacity preservation**: on success, the final bump pointer is
    within the arena's capacity.

The pillar theorem is a **conditional** theorem that composes three
sorry-free sub-theorems:
  1. `IRProgram.well_typed.to_program_well_typed_strong` (PMT-1-E) —
     lifts IR-level well-typedness to flat-program `WellTypedStrong`.
  2. `pmt_soundness` (PMT.Soundness §7) — gives capacity preservation
     + canonical trap codes (1, 134, 135).
  3. `no_oob_trap_for_well_typed_strong` (PMT.WellTypedStrong §5) —
     excludes the OOB trap (exit 134) for `WellTypedStrong` programs.

### Prerequisites (cross-orchestrator)

The pillar theorem has two cross-orchestrator prerequisites that are
NOT yet discharged on `main` as of PMT-1-G2:

  1. **IVE-1-A** (computable `WF_Layout`): the
     `h_well_typed : P.well_typed env` hypothesis is meaningful only
     when IVE's `WF_Layout` is a real (computable, non-vacuous)
     predicate. IVE-1-A makes this so. Until IVE-1-A lands, the
     hypothesis is structural only.

  2. **FFI-1-D** (No-FFI theorem): the `h_no_externs : NoExterns P`
     hypothesis is exactly the statement FFI-1-D proves (every VUMA
     program after FFI removal has no extern calls). Until FFI-1-D
     lands, the hypothesis is taken as an assumption.

Both prerequisites are taken as **explicit hypotheses** of
`pmt_pillar_sound` below — when IVE-1-A and FFI-1-D land on `main`,
the hypotheses can be discharged (or the theorem restated without
them) without changing the proof body.

### Residual axiom (PMT-internal)

The proof invokes `no_oob_trap_for_well_typed_strong`, which (via
`live_mirror_exclusive`) depends on the `own_ex_exclusive` axiom in
`LiveMirrorInvariant.lean`. This is the single residual non-standard
axiom in the PMT codedomain (see `proof/AUDIT_PMT.md` and the
`own_ex_exclusive` docstring for the deferral rationale). The
`PMT.Iris.HeapModel` module (added in PMT-1-G1) provides the
non-degenerate `RealOwn` predicate and the soundly-derived
`own_ex_exclusive_derived` theorem that will replace the axiom in a
follow-up wave.

### Scope (this version)

This version of `pmt_pillar_sound` proves:
  * The execution produces a result (totality of `exec`).
  * On success, capacity is preserved.
  * The execution never traps with the OOB code (134).

The full pillar theorem additionally excludes the UAF trap (135) and
the arena-overflow trap (1). These two exclusions require additional
lemmas (UAF safety from `DataflowOk`, overflow safety from
`CapacityInvariant` preservation across `step`) that are deferred to a
follow-up wave to keep this version sorry-free. The deferral is
documented inline at the end of the proof.

### References

  * `PMT.Soundness` §7 — `pmt_soundness` (capacity preservation +
    canonical trap codes).
  * `PMT.WellTypedStrong` §5 — `no_oob_trap_for_well_typed_strong`
    (no OOB trap for `WellTypedStrong` programs).
  * `PMT.WellTypedStrong` §8.2 —
    `IRProgram.well_typed.to_program_well_typed_strong` (the lift
    from IR-level well-typedness to flat-program `WellTypedStrong`).
  * `PMT.IRProgram` §11 — `IRProgram.well_typed`.
  * `PMT.SimRel` §10 — `full_simulation_strong` (non-degenerate
    simulation, PMT-1-E).
-/

namespace PMT

/-! ## §1. The `NoExterns` predicate (No-FFI hypothesis) -/

/-- §1: `NoExterns P` — every call in `P` is to a built-in (no extern
    calls). This is the No-FFI discipline: after FFI removal
    (FFI-1-D's territory), every VUMA program satisfies this.

    The predicate checks each `PmtInstr.call` and `PmtInstr.call_indirect`
    in the program and ensures the callee is a known built-in (in the
    `builtin_callees` list). Built-in callees are dispatched by the
    Lean `exec` model directly; extern callees would require FFI,
    which is out of scope for the PMT pillar.

    `NoExterns` is taken as an explicit hypothesis of `pmt_pillar_sound`
    below; when FFI-1-D lands on `main`, the hypothesis can be
    discharged (every post-FFI-removal program satisfies `NoExterns`). -/
def NoExterns (P : IRProgram) : Prop :=
  ∀ (f : IRFunction) (_hf : f ∈ P.functions)
     (b : IRBlock) (_hb : b ∈ f.blocks)
     (i : PmtInstr) (_hi : i ∈ b.instructions),
    match i with
    | .call name _ => name ∈ builtin_callees
    | .call_indirect _ _ => False  -- indirect calls are never No-FFI
    | _ => True
  where
    /-- Built-in callees — the closed set of functions dispatched by
        the Lean `exec` model. Includes the channel operations, memory
        operations, and the `__oob_trap` / `__arena_overflow` / `__uaf_trap`
        traps. These are NOT extern — they are part of the runtime. -/
    builtin_callees : List String :=
      [ "channel_open", "channel_send", "channel_recv", "channel_close"
      , "channel_recv_timeout", "channel_recv_result"
      , "__oob_trap", "__arena_overflow", "__uaf_trap"
      ]

/-! ## §2. The PMT pillar theorem -/

/-- §2: **`pmt_pillar_sound`** — the PMT pillar theorem (sorry-free).

    For any VUMA program `P` with no extern calls (`h_no_externs`)
    that is well-typed at the IR level (`h_well_typed`) and whose
    flattened program satisfies `DataflowOk` and `CapacityInvariant`,
    the Lean execution of `P`'s flattened program is memory-safe:

      1. **Termination totality** — `exec` produces some result.
      2. **Capacity preservation** — on success, the final bump
         pointer is within the arena's capacity.
      3. **No OOB trap** — exit code 134 never occurs.

    The full pillar theorem additionally excludes the UAF trap (135)
    and the arena-overflow trap (1); these exclusions require
    additional lemmas deferred to a follow-up wave (see "Scope"
    in the module-level docstring).

    The theorem is **conditional** on the `NoExterns` hypothesis
    (discharged by FFI-1-D when it lands) and on the `well_typed`
    hypothesis (made meaningful by IVE-1-A when it lands). The proof
    body is sorry-free and stable across these prerequisite landings:
    when IVE-1-A and FFI-1-D land, the hypotheses can be discharged
    without changing the proof structure.

    **Proof structure.**
      * Lift `h_well_typed : P.well_typed env` to
        `WellTypedStrong P.to_program initial_var` via
        `IRProgram.well_typed.to_program_well_typed_strong` (PMT-1-E).
      * Invoke `pmt_soundness` on `P.to_program` to get the totality
        existential + capacity preservation + canonical trap codes
        (1, 134, 135).
      * Invoke `no_oob_trap_for_well_typed_strong` to exclude exit
        code 134 (OOB trap).

    **Axiom audit.** Uses one residual non-standard axiom:
    `own_ex_exclusive` (in `LiveMirrorInvariant.lean`, transitively
    via `no_oob_trap_for_well_typed_strong`). See the module-level
    docstring above. -/
theorem pmt_pillar_sound
    (P : IRProgram) (env : String → Layout) (initial_var : String)
    (initial_state : ExecState)
    (h_no_externs : NoExterns P)
    (h_well_typed : P.well_typed env)
    (h_dataflow : DataflowOk (P.to_program) initial_var)
    (hcap : CapacityInvariant initial_state.arena)
    (hinit : initial_state.live initial_var = Liveness.live)
    (hstep_live : ∀ st : Step, st ∈ P.to_program →
                   initial_state.live st.in_var = Liveness.live) :
    -- Memory-safety conclusion:
    -- (1) The execution produces SOME result (totality of `exec`).
    (∃ r, exec P.to_program initial_state = r)
    ∧ -- (2) On a successful execution, the final bump pointer is
      --     within the arena's capacity.
      (match exec P.to_program initial_state with
       | Result.ok final_used => final_used ≤ initial_state.arena.capacity
       | Result.trap _ => True)
    ∧ -- (3) The execution never traps with the OOB code (134).
      exec P.to_program initial_state ≠ Result.trap 134 := by
  -- (1)+(2) Totality + capacity preservation: invoke `pmt_soundness`.
  have hwf_strong : WellTypedStrong P.to_program initial_var :=
    IRProgram.well_typed.to_program_well_typed_strong P env initial_var
      h_well_typed h_dataflow
  have hwf : WellTyped P.to_program :=
    well_typed_strong_implies_well_typed P.to_program initial_var hwf_strong
  -- Extract the per-step `WF_Layout` conjunct from `hwf` (a 3-way And).
  have hwf_layout : ∀ st : Step, st ∈ P.to_program → WF_Layout st.layout :=
    fun st hst => hwf.left st hst
  have hstep : ∀ st : Step, st ∈ P.to_program →
                WF_Layout st.layout ∧ initial_state.live st.in_var = Liveness.live :=
    fun st hst => ⟨hwf_layout st hst, hstep_live st hst⟩
  -- `pmt_soundness` gives totality + capacity preservation + canonical
  -- trap codes.
  obtain ⟨r, hr_exec, hr_safety⟩ :=
    pmt_soundness P.to_program hwf initial_state hstep hcap
  -- (3) No OOB trap (134): from `no_oob_trap_for_well_typed_strong`.
  have hno_oob : ∀ r', exec P.to_program initial_state = r' →
                  r' ≠ Result.trap 134 :=
    no_oob_trap_for_well_typed_strong P.to_program initial_var
      hwf_strong initial_state hstep hcap hinit
  -- Assemble the three conjuncts. We use `And.intro` explicitly to
  -- avoid `let`-binding issues with the goal's `match` types.
  refine ⟨?_, ?_, ?_⟩
  · -- (1) Totality: witness `r` and `hr_exec`.
    exact ⟨r, hr_exec⟩
  · -- (2) Capacity preservation: rewrite the goal's `exec P.to_program
    --     initial_state` to `r` via `hr_exec`, then case-split on `r`
    --     (the goal's trap case is weaker than `hr_safety`'s).
    rw [hr_exec]
    cases r with
    | ok fu => exact hr_safety
    | trap c => exact trivial
  · -- (3) No OOB trap: rewrite the goal's `exec P.to_program
    --     initial_state` to `r` via `hr_exec`, then apply `hno_oob`.
    rw [hr_exec]
    exact hno_oob r hr_exec

/-! ## §2.5. Strengthened pillar theorem: also excludes UAF trap (135) (PMT-3-B) -/

/-- §2.5: **`pmt_pillar_sound_no_uaf`** — the strengthened PMT pillar
    theorem that additionally excludes the UAF trap (exit code 135).

    For any VUMA program `P` with no extern calls (`h_no_externs`)
    that is well-typed at the IR level (`h_well_typed`) and whose
    flattened program satisfies `DataflowOk` and `CapacityInvariant`,
    the Lean execution of `P`'s flattened program is memory-safe:

      1. **Termination totality** — `exec` produces some result.
      2. **Capacity preservation** — on success, the final bump
         pointer is within the arena's capacity.
      3. **No OOB trap** — exit code 134 never occurs.
      4. **No UAF trap** — exit code 135 never occurs.

    The UAF exclusion (conjunct 4) is the PMT-3-B strengthening. It
    uses `no_uaf_trap_for_well_typed_strong` (PMT.WellTypedStrong §7.5),
    which mirrors `no_oob_trap_for_well_typed_strong` but excludes
    exit 135 instead of 134. The key insight: the `hstep` liveness
    hypothesis gives `s.live i.in_var = .live` for every step, which
    directly contradicts the UAF guard `s.live i.in_var = .dead`.

    **Remaining limitation.** The arena-overflow trap (exit code 1) is
    NOT excluded by this theorem. Excluding it requires an additional
    "total allocation fits in capacity" hypothesis that is out of
    scope for PMT-3-B. See `pmt_pillar_sound_full` (§3, deferred) for
    the target statement.

    **Axiom audit.** Same as `pmt_pillar_sound`: uses one residual
    non-standard axiom `own_ex_exclusive` (transitively via
    `no_oob_trap_for_well_typed_strong`). -/
theorem pmt_pillar_sound_no_uaf
    (P : IRProgram) (env : String → Layout) (initial_var : String)
    (initial_state : ExecState)
    (h_no_externs : NoExterns P)
    (h_well_typed : P.well_typed env)
    (h_dataflow : DataflowOk (P.to_program) initial_var)
    (hcap : CapacityInvariant initial_state.arena)
    (hinit : initial_state.live initial_var = Liveness.live)
    (hstep_live : ∀ st : Step, st ∈ P.to_program →
                   initial_state.live st.in_var = Liveness.live) :
    (∃ r, exec P.to_program initial_state = r)
    ∧ (match exec P.to_program initial_state with
       | Result.ok final_used => final_used ≤ initial_state.arena.capacity
       | Result.trap _ => True)
    ∧ exec P.to_program initial_state ≠ Result.trap 134
    ∧ exec P.to_program initial_state ≠ Result.trap 135 := by
  -- (1)+(2)+(3) from `pmt_pillar_sound`.
  obtain ⟨h_total, h_cap, h_no_oob⟩ :=
    pmt_pillar_sound P env initial_var initial_state
      h_no_externs h_well_typed h_dataflow hcap hinit hstep_live
  -- (4) No UAF trap from `no_uaf_trap_for_well_typed_strong`.
  have hwf_strong : WellTypedStrong P.to_program initial_var :=
    IRProgram.well_typed.to_program_well_typed_strong P env initial_var
      h_well_typed h_dataflow
  have hwf : WellTyped P.to_program :=
    well_typed_strong_implies_well_typed P.to_program initial_var hwf_strong
  have hwf_layout : ∀ st : Step, st ∈ P.to_program → WF_Layout st.layout :=
    fun st hst => hwf.left st hst
  have hstep : ∀ st : Step, st ∈ P.to_program →
                WF_Layout st.layout ∧ initial_state.live st.in_var = Liveness.live :=
    fun st hst => ⟨hwf_layout st hst, hstep_live st hst⟩
  have hno_uaf : ∀ r', exec P.to_program initial_state = r' →
                  r' ≠ Result.trap 135 :=
    no_uaf_trap_for_well_typed_strong P.to_program initial_var
      hwf_strong initial_state hstep hcap hinit
  -- Assemble the four conjuncts.
  refine ⟨?_, ?_, ?_, ?_⟩
  · exact h_total
  · exact h_cap
  · exact h_no_oob
  · exact hno_uaf _ rfl

/-! ## §3. The full pillar theorem (deferred to a follow-up wave) -/

/- §3: `pmt_pillar_sound_full` — the **full** PMT pillar theorem,
   additionally excluding the UAF trap (135) and the arena-overflow
   trap (1). **Status: not yet proven — deferred to a follow-up
   wave.** The two additional exclusions require:
     * UAF safety from `DataflowOk` (a use-after-free requires
       reading a freed variable, which `DataflowOk` forbids).
     * Overflow safety from `CapacityInvariant` preservation across
       `step` (capacity overflow requires `used + size > capacity`,
       which `CapacityInvariant` + step-preservation forbids).

   These lemmas are deferred to a follow-up wave to keep
   `pmt_pillar_sound` (§2 above) sorry-free. The statement is
   documented here to make the target precise; the theorem will be
   stated and proven when the supporting lemmas land. See the
   module-level docstring "Scope" section for details. -/

-- TODO(follow-up wave): when the UAF safety and overflow safety
-- lemmas land, state and prove `pmt_pillar_sound_full` here. The
-- theorem will compose `pmt_pillar_sound` (§2) with the two new
-- lemmas to exclude exit codes 135 and 1 in addition to 134.

end PMT
