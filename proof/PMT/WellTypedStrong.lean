import PMT.Soundness
import PMT.PmtInstr
import PMT.ExecFunction

/-!
## WellTypedStrong — strengthened WellTyped predicate (sorry-free)

The basic `WellTyped` (in `PMT.Soundness`) only enforces name-uniqueness
plus `WF_Layout` per step. `WellTypedStrong` adds:

  1. **Dataflow**: every `in_var` (except the program's initial input) is
     the `out_var` of some prior step. Closes the gap that a program like
     `[⟨"a","b",_⟩, ⟨"c","d",_⟩]` is `WellTyped` but cannot execute
     without trapping on step 2 (input `c` was never produced).
  2. **Field access safety**: every `Field` registered in a step's
     `Layout` satisfies `FieldBounds` (the static bound). In a future
     refinement, when `Step` gains an `op : PmtOp` field, this will be
     refined to match the `PmtOp.field_access f` case of
     `PmtInstr.well_typed` — proving that the accessed field `f`
     is in `s.layout.fields`.
  3. **(Implicit via `WellTyped`)**: `WF_Layout s.layout` for each step,
     plus `in_var`/`out_var` name-uniqueness across the program.

This closes **gap 1.2** ("WellTyped only enforces name-uniqueness"). The companion non-trapping
corollary `no_oob_trap_for_well_typed_strong` lives in this module
(`WellTypedStrong.lean`); the top-level `pmt_soundness` in
`PMT.Soundness` accepts `WellTypedStrong` as its (strengthened)
hypothesis. All theorems in this file close without `sorry`.

**References.**
  * `PMT.PmtInstr` — `PmtInstr.well_typed`, the per-instruction
    check that IVE's `verify_state_reads` / `verify_state_writes` /
    `verify_transform` perform; `WellTypedStrong` is the program-level
    analog for the `Step` model.
  * `PMT.Soundness` — basic `WellTyped` and `pmt_soundness`.
  * `PMT.Field` — `FieldBounds` definition used in (2).

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job in
`.github/workflows/proof-verify.yml` runs the same command.
-/

namespace PMT

/-! ## §1. Dataflow predicate -/

/-- §1.1: `DataflowOk prog initial_var` — every `in_var` (except
`initial_var`) is produced by some step in the program.

The runtime ordering (the producer must execute before the consumer) is
enforced by the `hstep` hypothesis of `pmt_soundness`, which requires
`s.live st.in_var = Liveness.live` for the *initial* state `s`;
`DataflowOk` is the static name-availability check that closes the
"input `c` was never produced" hole.

Note: `prior` need not textually precede `s` in the program list. The
uniqueness of `out_var` (enforced by `WellTyped`'s third conjunct)
together with the runtime `hstep` hypothesis ensures correct ordering at
execution time. -/
def DataflowOk (prog : Program) (initial_var : String) : Prop :=
  ∀ s : Step, s ∈ prog →
    s.in_var = initial_var
    ∨ ∃ prior : Step, prior ∈ prog ∧ prior.out_var = s.in_var

/-! ## §2. Field access safety predicate (strengthened) -/

/-- §2.1: `FieldAccessOk prog` — for every `Step` in `prog` whose `op`
is `PmtOp.field_access f`, the accessed field `f` is registered in
`s.layout.fields`.

This is the program-level analog of `PmtInstr.well_typed`'s
`field_access` branch: it ensures that every runtime
`PmtOp.field_access f` op references a field whose byte range is
statically known to fit inside the layout — via `WF_Layout`'s first
conjunct, which gives `f.offset + f.size ≤ layout.total_size` for
every `f ∈ layout.fields`.

**Strengthening.** Previously, `FieldAccessOk` checked that
every field `f ∈ s.layout.fields` satisfies `FieldBounds s.layout f`.
That check was implied by `WF_Layout`'s first conjunct and did *not*
constrain `PmtOp.field_access f` ops whose `f` was not in
`s.layout.fields`. The strengthening closes that gap by requiring
`f ∈ s.layout.fields` for every `PmtOp.field_access f` op. Combined
with `WF_Layout`, this makes `TrapCode.oob` (exit 134) unreachable for
`WellTypedStrong` programs — see `no_oob_trap_for_well_typed_strong`
(§7.1) for the corollary. -/
def FieldAccessOk (prog : Program) : Prop :=
  ∀ s : Step, s ∈ prog →
    match s.op with
    | PmtOp.field_access f => f ∈ s.layout.fields
    | _ => True

/-! ## §3. The strengthened predicate -/

/-- §3.1: `WellTypedStrong prog initial_var` — the strengthened
predicate.

Combines the basic `WellTyped` (name uniqueness + `WF_Layout`) with
`DataflowOk` (dataflow) and `FieldAccessOk` (field safety). This is the
predicate that closes gap 1.2. -/
def WellTypedStrong (prog : Program) (initial_var : String) : Prop :=
  WellTyped prog  -- basic uniqueness + WF_Layout (from PMT.Soundness)
  ∧ DataflowOk prog initial_var
  ∧ FieldAccessOk prog

/-! ## §4. Projection lemmas -/

/-- §4.1: `WellTypedStrong` implies `WellTyped` (basic predicate). -/
theorem well_typed_strong_implies_well_typed
    (prog : Program) (initial_var : String)
    (h : WellTypedStrong prog initial_var) :
    WellTyped prog := h.1

/-- §4.2: `WellTypedStrong` implies `DataflowOk`. -/
theorem well_typed_strong_implies_dataflow
    (prog : Program) (initial_var : String)
    (h : WellTypedStrong prog initial_var) :
    DataflowOk prog initial_var := h.2.1

/-- §4.3: `WellTypedStrong` implies `FieldAccessOk`. -/
theorem well_typed_strong_implies_field_access
    (prog : Program) (initial_var : String)
    (h : WellTypedStrong prog initial_var) :
    FieldAccessOk prog := h.2.2

/-! ## §5. Bridge to `PmtInstr.well_typed` (REMOVED in PMT-FAITH-5-A)

The previous bridge `Step.to_pmt_instr` + `step_wf_implies_pmt_instr_well_typed`
constructed `PmtInstr.transform s.in_var s.out_var s.layout` from a `Step`.
PMT-FAITH-5-A removed the unfaithful `PmtInstr.transform` variant (closes
FAITH-2-C CRITICAL gap) — the faithful `transform_layouts` variant takes
`IRValue → IRValue → String → String`, which does not match `Step`'s
`String → String → Layout` fields. The String-vs-IRValue abstraction gap
(FAITH-2-L) is scheduled for Wave 6; once closed, a new bridge can be
constructed using the faithful variant. Until then, this bridge is removed
(it was a demonstration, not in the critical path of `pmt_soundness`).

The `WellTypedStrong` predicate and `pmt_soundness_strong` theorem are
unaffected — they operate on `Step` directly, not via `to_pmt_instr`. -/

/-! ## §6. Strengthened soundness theorem -/

/-- §6.1: Strengthened soundness — `WellTypedStrong` programs satisfy
the same soundness property as `WellTyped` programs (no UB; canonical
trap codes 1/134/135).

The proof reuses `pmt_soundness` via `well_typed_strong_implies_well_typed`.

The extra `hinit` hypothesis (that the program's `initial_var` is live
in the initial state) is the runtime counterpart of `DataflowOk`'s
static check; it is preserved here for use by the
`no_oob_trap_for_well_typed_strong` corollary below. -/
theorem pmt_soundness_strong
    (prog : Program) (initial_var : String)
    (hwf : WellTypedStrong prog initial_var)
    (s : ExecState)
    (hstep : ∀ st : Step, st ∈ prog →
              WF_Layout st.layout ∧ s.live st.in_var = Liveness.live)
    (hcap : CapacityInvariant s.arena)
    (_hinit : s.live initial_var = Liveness.live) :
    ∃ r, exec prog s = r
    ∧ (match r with
       | Result.ok final_used => final_used ≤ s.arena.capacity
       | Result.trap code => code = 1 ∨ code = 134 ∨ code = 135) := by
  -- `WellTypedStrong` implies `WellTyped`, so we can reuse `pmt_soundness`.
  have hwf_basic := well_typed_strong_implies_well_typed prog initial_var hwf
  exact pmt_soundness prog hwf_basic s hstep hcap

/-! ## §7. Corollary: no `.oob` trap (proved) -/

/-- Helper: `WellTyped (i :: rest)` projects to `WellTyped rest`.
Used by `no_oob_trap_aux` to invoke the induction hypothesis on the
program tail. The proof mirrors the `hwf_rest` derivation in
`pmt_soundness` (`PMT.Soundness` §7). -/
private theorem WellTyped_cons_proj (i : Step) (rest : Program)
    (hwt : WellTyped (i :: rest)) : WellTyped rest := by
  unfold WellTyped at hwt ⊢
  refine ⟨?_, ?_, ?_⟩
  · intro st hst
    exact hwt.1 st (List.mem_cons_of_mem _ hst)
  · intro st hst
    have h_in_prog :
        (List.filter (fun s' => s'.in_var == st.in_var) (i :: rest)).length = 1 :=
      hwt.2.1 st (List.mem_cons_of_mem _ hst)
    by_cases h_eq : i.in_var == st.in_var
    · exfalso
      rw [List.filter_cons, if_pos h_eq, List.length_cons] at h_in_prog
      have h_st_in_filter :
          st ∈ List.filter (fun s' => s'.in_var == st.in_var) rest := by
        rw [List.mem_filter]; refine ⟨hst, ?_⟩; simp
      have h_empty :
          List.filter (fun s' => s'.in_var == st.in_var) rest = [] :=
        List.length_eq_zero_iff.mp (by omega)
      exact List.not_mem_nil (h_empty ▸ h_st_in_filter)
    · rw [List.filter_cons, if_neg h_eq] at h_in_prog
      exact h_in_prog
  · intro st hst
    have h_in_prog :
        (List.filter (fun s' => s'.out_var == st.out_var) (i :: rest)).length = 1 :=
      hwt.2.2 st (List.mem_cons_of_mem _ hst)
    by_cases h_eq : i.out_var == st.out_var
    · exfalso
      rw [List.filter_cons, if_pos h_eq, List.length_cons] at h_in_prog
      have h_st_in_filter :
          st ∈ List.filter (fun s' => s'.out_var == st.out_var) rest := by
        rw [List.mem_filter]; refine ⟨hst, ?_⟩; simp
      have h_empty :
          List.filter (fun s' => s'.out_var == st.out_var) rest = [] :=
        List.length_eq_zero_iff.mp (by omega)
      exact List.not_mem_nil (h_empty ▸ h_st_in_filter)
    · rw [List.filter_cons, if_neg h_eq] at h_in_prog
      exact h_in_prog

/-- Helper: `FieldAccessOk (i :: rest)` projects to `FieldAccessOk rest`. -/
private theorem FieldAccessOk_cons_proj (i : Step) (rest : Program)
    (hfa : FieldAccessOk (i :: rest)) : FieldAccessOk rest := by
  intro st hst
  exact hfa st (List.mem_cons_of_mem _ hst)

/-- Internal auxiliary: same conclusion as
`no_oob_trap_for_well_typed_strong` but without the unused `hinit`
hypothesis.

`hinit : s.live initial_var = Liveness.live` is not preserved by
`PmtOp.alloc` / `PmtOp.transform` steps (which kill `i.in_var`); since
`initial_var = i.in_var` is possible, the induction hypothesis cannot
be applied with `hinit` in its original shape. The aux lemma drops
`hinit` entirely (it is unused — see the `unused variable` warning on
`pmt_soundness_strong`); the public theorem invokes the aux lemma and
ignores `hinit`. -/
private theorem no_oob_trap_aux
    (prog : Program)
    (hwf : WellTyped prog)
    (hfield : FieldAccessOk prog)
    (s : ExecState)
    (hstep : ∀ st : Step, st ∈ prog →
              WF_Layout st.layout ∧ s.live st.in_var = Liveness.live)
    (hcap : CapacityInvariant s.arena) :
    ∀ r, exec prog s = r → r ≠ Result.trap 134 := by
  induction prog generalizing s with
  | nil =>
    intro r hr
    -- `exec [] s = Result.ok s.arena.used` (definitionally).
    rw [exec] at hr
    subst hr
    -- `Result.ok _ ≠ Result.trap _` — distinct constructors.
    intro h; injection h
  | cons i rest ih =>
    intro r hr
    rw [exec] at hr
    -- Case-split on the result of `step s i`.
    cases h_step : step s i with
    | error c =>
      -- `exec (i :: rest) s = Result.trap c.to_exit`
      rw [h_step] at hr
      -- `hr` has an unreduced `match` (since `c` is a variable); after
      -- `cases c`, the scrutinee becomes a concrete constructor and
      -- `simp only []` iota-reduces.
      cases c with
      | arena_overflow =>
        -- `to_exit = 1 ≠ 134`
        simp only [] at hr
        rw [show TrapCode.arena_overflow.to_exit = 1 from rfl] at hr
        subst hr
        intro h; injection h with h'; omega
      | uaf =>
        -- `to_exit = 135 ≠ 134`
        simp only [] at hr
        rw [show TrapCode.uaf.to_exit = 135 from rfl] at hr
        subst hr
        intro h; injection h with h'; omega
      | oob =>
        -- `to_exit = 134` — derive contradiction.
        -- `step s i = .error .oob` requires:
        --   (1) `s.live i.in_var ≠ .dead` (else `.uaf`)
        --   (2) `i.op = .field_access f` for some `f`
        --   (3) `f.offset + f.size > i.layout.total_size`
        -- But `FieldAccessOk` (strengthened) gives
        -- `f ∈ i.layout.fields`, and `WF_Layout`'s first conjunct gives
        -- `f.offset + f.size ≤ i.layout.total_size`. Contradiction.
        exfalso
        have h_i_mem : i ∈ i :: rest := @List.mem_cons_self _ i rest
        obtain ⟨hwf_i, hlive_i⟩ := hstep i h_i_mem
        have h_field_ok_i := hfield i h_i_mem
        -- Unfold `step`; the UAF guard is false (since `s.live i.in_var = .live`).
        rw [step, hlive_i, if_neg (by intro h; cases h)] at h_step
        cases h_op : i.op with
        | field_access f =>
          rw [h_op] at h_step
          simp only [] at h_step
          by_cases h_oob : f.offset + f.size > i.layout.total_size
          · -- `h_step` is trivially `.error .oob = .error .oob`.
            -- `FieldAccessOk` (strengthened) ⇒ `f ∈ i.layout.fields`.
            rw [if_pos h_oob] at h_step
            have h_f_in : f ∈ i.layout.fields := by
              have := h_field_ok_i
              rw [h_op] at this
              exact this
            -- `WF_Layout` ⇒ `f.offset + f.size ≤ i.layout.total_size`.
            have h_f_bound : f.offset + f.size ≤ i.layout.total_size :=
              hwf_i.1 f h_f_in
            omega
          · -- `h_step : .ok s = .error .oob` — constructor mismatch.
            rw [if_neg h_oob] at h_step
            cases h_step
        | alloc =>
          rw [h_op] at h_step
          simp only [] at h_step
          by_cases h_ovf : s.arena.used + i.layout.total_size > s.arena.capacity
          · -- `h_step : .error .arena_overflow = .error .oob` — mismatch.
            rw [if_pos h_ovf] at h_step
            cases h_step
          · -- `h_step : .ok _ = .error .oob` — mismatch.
            rw [if_neg h_ovf] at h_step
            cases h_step
        | transform =>
          rw [h_op] at h_step
          simp only [] at h_step
          by_cases h_ovf : s.arena.used + i.layout.total_size > s.arena.capacity
          · rw [if_pos h_ovf] at h_step
            cases h_step
          · rw [if_neg h_ovf] at h_step
            cases h_step
    | ok s' =>
      -- `exec (i :: rest) s = exec rest s'`
      rw [h_step] at hr
      -- Case-split on `i.op` to derive `hstep'` and `hcap'` for the IH.
      cases h_op : i.op with
      | field_access f =>
        -- `step`'s `field_access` success branch returns `.ok s` (no state
        -- change), so `s' = s`.
        have h_i_mem : i ∈ i :: rest := @List.mem_cons_self _ i rest
        obtain ⟨_, hlive_i⟩ := hstep i h_i_mem
        have h_unfold : step s i = Except.ok s' := h_step
        rw [step, hlive_i, if_neg (by intro h; cases h), h_op] at h_unfold
        simp only [] at h_unfold
        by_cases h_oob : f.offset + f.size > i.layout.total_size
        · rw [if_pos h_oob] at h_unfold
          cases h_unfold
        · rw [if_neg h_oob] at h_unfold
          injection h_unfold with h_eq
          -- `h_eq : s = s'`. Rewrite `s'` to `s` in `hr`.
          rw [← h_eq] at hr
          -- `hstep'` for `rest` at `s` (= `s'`): reuse original `hstep`.
          have hstep' : ∀ st : Step, st ∈ rest →
                WF_Layout st.layout ∧ s.live st.in_var = Liveness.live := by
            intro st hst
            exact hstep st (List.mem_cons_of_mem _ hst)
          exact ih (WellTyped_cons_proj i rest hwf)
                     (FieldAccessOk_cons_proj i rest hfield) s hstep' hcap r hr
      | alloc =>
        have h_i_mem : i ∈ i :: rest := @List.mem_cons_self _ i rest
        obtain ⟨_, hlive_i⟩ := hstep i h_i_mem
        have h_unfold : step s i = Except.ok s' := h_step
        rw [step, hlive_i, if_neg (by intro h; cases h), h_op] at h_unfold
        simp only [] at h_unfold
        by_cases h_ovf : s.arena.used + i.layout.total_size > s.arena.capacity
        · rw [if_pos h_ovf] at h_unfold
          cases h_unfold
        · rw [if_neg h_ovf] at h_unfold
          injection h_unfold with h_eq
          -- `h_eq : {arena := ..., live := ...} = s'`.
          have hstep' : ∀ st : Step, st ∈ rest →
                WF_Layout st.layout ∧ s'.live st.in_var = Liveness.live := by
            intro st hst
            have hst_mem : st ∈ i :: rest := List.mem_cons_of_mem _ hst
            obtain ⟨hwf_st, hlive_st⟩ := hstep st hst_mem
            refine ⟨hwf_st, ?_⟩
            rw [← h_eq]
            show (if st.in_var = i.in_var then Liveness.dead
                  else if st.in_var = i.out_var then Liveness.live
                  else s.live st.in_var) = Liveness.live
            by_cases h_eq_in : st.in_var = i.in_var
            · -- Contradicts `WellTyped`'s `in_var` name-uniqueness.
              exfalso
              have h_in_uniq :
                  (List.filter (fun s' => s'.in_var == st.in_var) (i :: rest)).length = 1 :=
                hwf.2.1 st hst_mem
              rw [List.filter_cons] at h_in_uniq
              by_cases hbeq : i.in_var == st.in_var
              · rw [if_pos hbeq, List.length_cons] at h_in_uniq
                have h_st_in_filter :
                    st ∈ List.filter (fun s' => s'.in_var == st.in_var) rest := by
                  rw [List.mem_filter]; refine ⟨hst, ?_⟩; simp
                have h_empty :
                    List.filter (fun s' => s'.in_var == st.in_var) rest = [] :=
                  List.length_eq_zero_iff.mp (by omega)
                exact List.not_mem_nil (h_empty ▸ h_st_in_filter)
              · rw [h_eq_in] at hbeq
                have h_refl : (i.in_var == i.in_var) = true := by simp
                exact hbeq h_refl
            · rw [if_neg h_eq_in]
              by_cases h_eq_out : st.in_var = i.out_var
              · rw [if_pos h_eq_out]
              · rw [if_neg h_eq_out]
                exact hlive_st
          have hcap' : CapacityInvariant s'.arena := by
            rw [← h_eq]
            show s.arena.used + i.layout.total_size ≤ s.arena.capacity
            omega
          exact ih (WellTyped_cons_proj i rest hwf)
                      (FieldAccessOk_cons_proj i rest hfield) s' hstep' hcap' r hr
      | transform =>
        have h_i_mem : i ∈ i :: rest := @List.mem_cons_self _ i rest
        obtain ⟨_, hlive_i⟩ := hstep i h_i_mem
        have h_unfold : step s i = Except.ok s' := h_step
        rw [step, hlive_i, if_neg (by intro h; cases h), h_op] at h_unfold
        simp only [] at h_unfold
        by_cases h_ovf : s.arena.used + i.layout.total_size > s.arena.capacity
        · rw [if_pos h_ovf] at h_unfold
          cases h_unfold
        · rw [if_neg h_ovf] at h_unfold
          injection h_unfold with h_eq
          have hstep' : ∀ st : Step, st ∈ rest →
                WF_Layout st.layout ∧ s'.live st.in_var = Liveness.live := by
            intro st hst
            have hst_mem : st ∈ i :: rest := List.mem_cons_of_mem _ hst
            obtain ⟨hwf_st, hlive_st⟩ := hstep st hst_mem
            refine ⟨hwf_st, ?_⟩
            rw [← h_eq]
            show (if st.in_var = i.in_var then Liveness.dead
                  else if st.in_var = i.out_var then Liveness.live
                  else s.live st.in_var) = Liveness.live
            by_cases h_eq_in : st.in_var = i.in_var
            · exfalso
              have h_in_uniq :
                  (List.filter (fun s' => s'.in_var == st.in_var) (i :: rest)).length = 1 :=
                hwf.2.1 st hst_mem
              rw [List.filter_cons] at h_in_uniq
              by_cases hbeq : i.in_var == st.in_var
              · rw [if_pos hbeq, List.length_cons] at h_in_uniq
                have h_st_in_filter :
                    st ∈ List.filter (fun s' => s'.in_var == st.in_var) rest := by
                  rw [List.mem_filter]; refine ⟨hst, ?_⟩; simp
                have h_empty :
                    List.filter (fun s' => s'.in_var == st.in_var) rest = [] :=
                  List.length_eq_zero_iff.mp (by omega)
                exact List.not_mem_nil (h_empty ▸ h_st_in_filter)
              · rw [h_eq_in] at hbeq
                have h_refl : (i.in_var == i.in_var) = true := by simp
                exact hbeq h_refl
            · rw [if_neg h_eq_in]
              by_cases h_eq_out : st.in_var = i.out_var
              · rw [if_pos h_eq_out]
              · rw [if_neg h_eq_out]
                exact hlive_st
          have hcap' : CapacityInvariant s'.arena := by
            rw [← h_eq]
            show s.arena.used + i.layout.total_size ≤ s.arena.capacity
            omega
          exact ih (WellTyped_cons_proj i rest hwf)
                      (FieldAccessOk_cons_proj i rest hfield) s' hstep' hcap' r hr

/-- §7.1: Corollary — `WellTypedStrong` programs never trap with `.oob`
(exit code 134).

This is the Lean proof that the runtime `__oob_trap` injection (in
`codegen::memory_safety::inject_bounds_check_ir`) is
**redundant defense-in-depth** for `WellTypedStrong` programs: the
`FieldAccessOk` conjunct (strengthened) guarantees that every
runtime `PmtOp.field_access f` op references a field `f` registered in
`s.layout.fields`; combined with `WF_Layout`'s first conjunct
(`f.offset + f.size ≤ layout.total_size` for every registered `f`),
the OOB guard `f.offset + f.size > layout.total_size` in `step`'s
`field_access` branch is never satisfied, so `.error .oob` (exit 134)
is never produced.

**Proof.** By induction on `prog` (generalizing `s`), case-splitting on
`step s i`. In the `.error c` case, `c = .oob` is the only trap code
with exit `134`; for it we unfold `step`, case on `i.op`, and use
`FieldAccessOk` + `WF_Layout` to derive
`f.offset + f.size ≤ i.layout.total_size`, contradicting the OOB
guard's `>`. The other trap codes (`.arena_overflow` = 1, `.uaf` = 135)
give non-`134` exits trivially. In the `.ok s'` case, we recurse on
`rest` with the projected `WellTyped` / `FieldAccessOk` and the
step-preserved `hstep'` / `hcap'`. The proof is factored into
`no_oob_trap_aux` to drop the unused `hinit` hypothesis (which is not
preserved by `alloc` / `transform` steps that kill `initial_var`). -/
theorem no_oob_trap_for_well_typed_strong
    (prog : Program) (initial_var : String)
    (hwf : WellTypedStrong prog initial_var)
    (s : ExecState)
    (hstep : ∀ st : Step, st ∈ prog →
              WF_Layout st.layout ∧ s.live st.in_var = Liveness.live)
    (hcap : CapacityInvariant s.arena)
    (_hinit : s.live initial_var = Liveness.live) :
    -- `.oob` (exit code 134) never traps for `WellTypedStrong` programs.
    -- Equivalent to: `∀ r, exec prog s = r → match r with
    --                  | Result.trap 134 => False | _ => True`.
    ∀ r, exec prog s = r → r ≠ Result.trap 134 :=
  no_oob_trap_aux prog hwf.1 hwf.2.2 s hstep hcap

/-! ## §7.5. No-UAF-trap theorem (PMT-3-B)

The `no_oob_trap_aux` proof above already establishes (in its `cons` case)
that `step s i ≠ .error .uaf` for every step `i` — the UAF guard
`s.live i.in_var = .dead` is contradicted by `hstep i h_i_mem` which gives
`s.live i.in_var = .live`. The OOB proof uses this fact only to proceed
past the UAF guard to the OOB check; the UAF trap itself is not excluded
from the conclusion (`to_exit = 135 ≠ 134` is enough for the OOB exclusion).

This section states and proves the UAF exclusion directly: for
`WellTypedStrong` programs with the `hstep` liveness hypothesis, the
execution never traps with `.uaf` (exit code 135). The proof is by
induction on `prog`, mirroring `no_oob_trap_aux`'s structure but with a
stronger conclusion.

The key insight: `hstep` is PRESERVED by `step` (the `no_oob_trap_aux`
proof establishes `hstep'` for the `rest` of the program in the `ok s'`
case). So at every step, the current `in_var` is live in the current
state, which directly contradicts the UAF guard. -/

private theorem no_uaf_trap_aux
    (prog : Program)
    (hwf : WellTyped prog)
    (s : ExecState)
    (hstep : ∀ st : Step, st ∈ prog →
              WF_Layout st.layout ∧ s.live st.in_var = Liveness.live)
    (hcap : CapacityInvariant s.arena) :
    ∀ r, exec prog s = r → r ≠ Result.trap 135 := by
  induction prog generalizing s with
  | nil =>
    intro r hr
    rw [exec] at hr
    subst hr
    intro h; injection h
  | cons i rest ih =>
    intro r hr
    rw [exec] at hr
    cases h_step : step s i with
    | error c =>
      rw [h_step] at hr
      -- Case-split on whether c = .uaf.
      by_cases hcu : c = TrapCode.uaf
      · -- c = .uaf: derive contradiction from hstep liveness.
        exfalso
        subst hcu
        have h_i_mem : i ∈ i :: rest := @List.mem_cons_self _ i rest
        obtain ⟨_, hlive_i⟩ := hstep i h_i_mem
        rw [step, hlive_i, if_neg (by intro h; cases h)] at h_step
        -- h_step : (match i.op with ...) = .error .uaf
        -- None of the branches produce .uaf (the UAF guard was the only one).
        cases h_op : i.op with
        | field_access f =>
          rw [h_op] at h_step
          simp only [] at h_step
          by_cases h : f.offset + f.size > i.layout.total_size
          · rw [if_pos h] at h_step; cases h_step
          · rw [if_neg h] at h_step; cases h_step
        | alloc =>
          rw [h_op] at h_step
          simp only [] at h_step
          by_cases h : s.arena.used + i.layout.total_size > s.arena.capacity
          · rw [if_pos h] at h_step; cases h_step
          · rw [if_neg h] at h_step; cases h_step
        | transform =>
          rw [h_op] at h_step
          simp only [] at h_step
          by_cases h : s.arena.used + i.layout.total_size > s.arena.capacity
          · rw [if_pos h] at h_step; cases h_step
          · rw [if_neg h] at h_step; cases h_step
      · -- c ≠ .uaf: c.to_exit ≠ 135 (only .uaf maps to 135).
        cases c with
        | arena_overflow =>
          simp only [] at hr
          rw [show TrapCode.arena_overflow.to_exit = 1 from rfl] at hr
          subst hr
          intro h; injection h with h'; omega
        | oob =>
          simp only [] at hr
          rw [show TrapCode.oob.to_exit = 134 from rfl] at hr
          subst hr
          intro h; injection h with h'; omega
        | uaf => exact absurd rfl hcu
    | ok s' =>
      rw [h_step] at hr
      -- Derive `hstep'` for `rest` at `s'` — same structure as `no_oob_trap_aux`.
      cases h_op : i.op with
      | field_access f =>
        have h_i_mem : i ∈ i :: rest := @List.mem_cons_self _ i rest
        obtain ⟨_, hlive_i⟩ := hstep i h_i_mem
        have h_unfold : step s i = Except.ok s' := h_step
        rw [step, hlive_i, if_neg (by intro h; cases h), h_op] at h_unfold
        simp only [] at h_unfold
        by_cases h_oob : f.offset + f.size > i.layout.total_size
        · rw [if_pos h_oob] at h_unfold
          cases h_unfold
        · rw [if_neg h_oob] at h_unfold
          injection h_unfold with h_eq
          rw [← h_eq] at hr
          have hstep' : ∀ st : Step, st ∈ rest →
                WF_Layout st.layout ∧ s.live st.in_var = Liveness.live := by
            intro st hst
            exact hstep st (List.mem_cons_of_mem _ hst)
          exact ih (WellTyped_cons_proj i rest hwf) s hstep' hcap r hr
      | alloc =>
        have h_i_mem : i ∈ i :: rest := @List.mem_cons_self _ i rest
        obtain ⟨_, hlive_i⟩ := hstep i h_i_mem
        have h_unfold : step s i = Except.ok s' := h_step
        rw [step, hlive_i, if_neg (by intro h; cases h), h_op] at h_unfold
        simp only [] at h_unfold
        by_cases h_ovf : s.arena.used + i.layout.total_size > s.arena.capacity
        · rw [if_pos h_ovf] at h_unfold
          cases h_unfold
        · rw [if_neg h_ovf] at h_unfold
          injection h_unfold with h_eq
          have hstep' : ∀ st : Step, st ∈ rest →
                WF_Layout st.layout ∧ s'.live st.in_var = Liveness.live := by
            intro st hst
            have hst_mem : st ∈ i :: rest := List.mem_cons_of_mem _ hst
            obtain ⟨hwf_st, hlive_st⟩ := hstep st hst_mem
            refine ⟨hwf_st, ?_⟩
            rw [← h_eq]
            show (if st.in_var = i.in_var then Liveness.dead
                  else if st.in_var = i.out_var then Liveness.live
                  else s.live st.in_var) = Liveness.live
            by_cases h_eq_in : st.in_var = i.in_var
            · exfalso
              have h_in_uniq :
                  (List.filter (fun s' => s'.in_var == st.in_var) (i :: rest)).length = 1 :=
                hwf.2.1 st hst_mem
              rw [List.filter_cons] at h_in_uniq
              by_cases hbeq : i.in_var == st.in_var
              · rw [if_pos hbeq, List.length_cons] at h_in_uniq
                have h_st_in_filter :
                    st ∈ List.filter (fun s' => s'.in_var == st.in_var) rest := by
                  rw [List.mem_filter]; refine ⟨hst, ?_⟩; simp
                have h_empty :
                    List.filter (fun s' => s'.in_var == st.in_var) rest = [] :=
                  List.length_eq_zero_iff.mp (by omega)
                exact List.not_mem_nil (h_empty ▸ h_st_in_filter)
              · rw [h_eq_in] at hbeq
                have h_refl : (i.in_var == i.in_var) = true := by simp
                exact hbeq h_refl
            · rw [if_neg h_eq_in]
              by_cases h_eq_out : st.in_var = i.out_var
              · rw [if_pos h_eq_out]
              · rw [if_neg h_eq_out]
                exact hlive_st
          have hcap' : CapacityInvariant s'.arena := by
            rw [← h_eq]
            show s.arena.used + i.layout.total_size ≤ s.arena.capacity
            omega
          exact ih (WellTyped_cons_proj i rest hwf) s' hstep' hcap' r hr
      | transform =>
        have h_i_mem : i ∈ i :: rest := @List.mem_cons_self _ i rest
        obtain ⟨_, hlive_i⟩ := hstep i h_i_mem
        have h_unfold : step s i = Except.ok s' := h_step
        rw [step, hlive_i, if_neg (by intro h; cases h), h_op] at h_unfold
        simp only [] at h_unfold
        by_cases h_ovf : s.arena.used + i.layout.total_size > s.arena.capacity
        · rw [if_pos h_ovf] at h_unfold
          cases h_unfold
        · rw [if_neg h_ovf] at h_unfold
          injection h_unfold with h_eq
          have hstep' : ∀ st : Step, st ∈ rest →
                WF_Layout st.layout ∧ s'.live st.in_var = Liveness.live := by
            intro st hst
            have hst_mem : st ∈ i :: rest := List.mem_cons_of_mem _ hst
            obtain ⟨hwf_st, hlive_st⟩ := hstep st hst_mem
            refine ⟨hwf_st, ?_⟩
            rw [← h_eq]
            show (if st.in_var = i.in_var then Liveness.dead
                  else if st.in_var = i.out_var then Liveness.live
                  else s.live st.in_var) = Liveness.live
            by_cases h_eq_in : st.in_var = i.in_var
            · exfalso
              have h_in_uniq :
                  (List.filter (fun s' => s'.in_var == st.in_var) (i :: rest)).length = 1 :=
                hwf.2.1 st hst_mem
              rw [List.filter_cons] at h_in_uniq
              by_cases hbeq : i.in_var == st.in_var
              · rw [if_pos hbeq, List.length_cons] at h_in_uniq
                have h_st_in_filter :
                    st ∈ List.filter (fun s' => s'.in_var == st.in_var) rest := by
                  rw [List.mem_filter]; refine ⟨hst, ?_⟩; simp
                have h_empty :
                    List.filter (fun s' => s'.in_var == st.in_var) rest = [] :=
                  List.length_eq_zero_iff.mp (by omega)
                exact List.not_mem_nil (h_empty ▸ h_st_in_filter)
              · rw [h_eq_in] at hbeq
                have h_refl : (i.in_var == i.in_var) = true := by simp
                exact hbeq h_refl
            · rw [if_neg h_eq_in]
              by_cases h_eq_out : st.in_var = i.out_var
              · rw [if_pos h_eq_out]
              · rw [if_neg h_eq_out]
                exact hlive_st
          have hcap' : CapacityInvariant s'.arena := by
            rw [← h_eq]
            show s.arena.used + i.layout.total_size ≤ s.arena.capacity
            omega
          exact ih (WellTyped_cons_proj i rest hwf) s' hstep' hcap' r hr

/-- §7.5: **`no_uaf_trap_for_well_typed_strong`** — for `WellTypedStrong`
    programs with the `hstep` liveness hypothesis, the execution never
    traps with `.uaf` (exit code 135).

    The UAF trap requires `s.live i.in_var = .dead` at some step `i`.
    The `hstep` hypothesis gives `s.live i.in_var = .live` for every
    step in the initial state, and `hstep` is preserved by `step`
    (established in the `no_uaf_trap_aux` proof's `ok s'` case via
    `WellTyped`'s in_var name-uniqueness). So at every step, the
    current `in_var` is live in the current state, contradicting the
    UAF guard.

    **PMT-3-B.** This theorem is the UAF counterpart of
    `no_oob_trap_for_well_typed_strong` (§7). Together, the two
    theorems exclude exit codes 134 and 135 for `WellTypedStrong`
    programs. The remaining trap code (1, arena-overflow) requires
    an additional "total allocation fits in capacity" hypothesis that
    is out of scope. -/
theorem no_uaf_trap_for_well_typed_strong
    (prog : Program) (initial_var : String)
    (hwf : WellTypedStrong prog initial_var)
    (s : ExecState)
    (hstep : ∀ st : Step, st ∈ prog →
              WF_Layout st.layout ∧ s.live st.in_var = Liveness.live)
    (hcap : CapacityInvariant s.arena)
    (_hinit : s.live initial_var = Liveness.live) :
    ∀ r, exec prog s = r → r ≠ Result.trap 135 :=
  no_uaf_trap_aux prog hwf.1 s hstep hcap

/-! ## §8. Program-level lift: `IRProgram.well_typed → WellTypedStrong (to_program)` (PMT-1-E)

This section proves the program-level lift theorem that closes the
non-degenerate `full_simulation_strong` proof in `PMT.SimRel`. The lift
takes `IRProgram.well_typed env` (the IR-level well-typedness predicate
defined in `PMT.IRProgram` §11) plus an IR-level dataflow hypothesis and
produces `WellTypedStrong (IRProgram.to_program p) initial_var` (the
flat-program-level strengthened predicate defined in §3.1 above).

The lift has three conjuncts:

  1. **`WellTyped`** (basic — name-uniqueness + `WF_Layout`). Closed by
     `IRProgram.to_program_preserves_well_typed_full` in
     `PMT.ExecFunction` §5.1, which lifts `IRProgram.well_typed` through
     `IRFunction.to_program`/`IRBlock.to_steps`/`PmtInstr.to_steps` and
     uses the `IRFunction.in_vars_unique`/`out_vars_unique` conjuncts of
     `IRFunction.well_typed` for the two name-uniqueness conjuncts.
  2. **`DataflowOk`** (every `in_var` is `initial_var` or produced by
     some `out_var`). NOT implied by `IRProgram.well_typed` alone (the
     IR-level predicate enforces name-uniqueness but not producer-for-
     every-reader dataflow). Taken as an explicit hypothesis
     `hdataflow : DataflowOk p.to_program initial_var`. A future
     refinement may add an IR-level `IRFunction.dataflow_ok` conjunct
     (analogous to `in_vars_unique`/`out_vars_unique`) so the hypothesis
     can be discharged at the IR level; for PMT-1-E the flat-program
     `DataflowOk` hypothesis suffices.
  3. **`FieldAccessOk`** (every `PmtOp.field_access f` op references a
     field `f` registered in `s.layout.fields`). Closed trivially by
     `IRProgram.to_program_FieldAccessOk` (§8.1 below), because
     `PmtInstr.to_steps` NEVER produces a `Step` with
     `op = .field_access` — every step carries `op = .transform`
     (per `PmtInstr.to_steps_op_transform` in `PMT.ExecFunction` §1.9).
     The `FieldAccessOk` match therefore reduces to `True` for every
     step in the flattened program.

The lift theorem is the bridge that lets `full_simulation_strong`
(`PMT.SimRel` §10) invoke `pmt_soundness_strong` (§6.1 above) on the
flattened program, yielding a non-degenerate simulation in which the
program actually executes (rather than trivially trapping with UAF on
the first step as in the prior vacuous proof). -/

/-- §8.1: `IRProgram.to_program` satisfies `FieldAccessOk` — every
runtime `PmtOp.field_access f` op references a field `f` registered in
`s.layout.fields`.

This holds TRIVIALLY for any `IRProgram.to_program` because
`PmtInstr.to_steps` never produces a `Step` with `op = .field_access`:
every `Step` carries `op = PmtOp.transform` (per
`PmtInstr.to_steps_op_transform` in `PMT.ExecFunction` §1.9). The
`FieldAccessOk` match therefore reduces to `True` for every step.

The proof lifts `PmtInstr.to_steps_op_transform` through two layers of
`List.flatMap` (`IRFunction.to_program` and `IRBlock.to_steps`) using
`List.mem_flatMap` to expose the originating `PmtInstr` — mirroring the
structure of `IRProgram.to_program_preserves_well_typed` in
`PMT.ExecFunction` §5. -/
theorem IRProgram.to_program_FieldAccessOk (p : IRProgram) :
    FieldAccessOk (IRProgram.to_program p) := by
  intro s hs
  obtain ⟨functions, data_sections⟩ := p
  cases functions with
  | nil =>
    simp only [IRProgram.to_program, List.not_mem_nil] at hs
  | cons f rest =>
    simp only [IRProgram.to_program, IRFunction.to_program] at hs
    rw [List.mem_flatMap] at hs
    obtain ⟨b, hb_in_blocks, hs_b⟩ := hs
    simp only [IRBlock.to_steps] at hs_b
    rw [List.mem_flatMap] at hs_b
    obtain ⟨i, hi_in_instrs, hs_i⟩ := hs_b
    -- `s.op = .transform` by `PmtInstr.to_steps_op_transform`.
    have h_op : s.op = PmtOp.transform :=
      PmtInstr.to_steps_op_transform i s hs_i
    -- `FieldAccessOk` match: for `op = .transform`, reduces to `True`.
    rw [h_op]
    trivial

/-- §8.2: `IRProgram.well_typed` lifts to `WellTypedStrong (to_program)`.

The lift combines:
  - `WellTyped (p.to_program)` — from `to_program_preserves_well_typed_full`
    (`PMT.ExecFunction` §5.1).
  - `DataflowOk (p.to_program) initial_var` — taken as hypothesis
    `hdataflow` (NOT implied by `IRProgram.well_typed` alone).
  - `FieldAccessOk (p.to_program)` — from `to_program_FieldAccessOk`
    (§8.1 above), holds trivially because `PmtInstr.to_steps` never
    produces a `.field_access` op.

This is the bridge that lets `full_simulation_strong` (`PMT.SimRel` §10)
invoke `pmt_soundness_strong` on the flattened program — yielding a
non-degenerate simulation in which the program actually executes. -/
theorem IRProgram.well_typed.to_program_well_typed_strong
    (p : IRProgram) (env : String → Layout) (initial_var : String)
    (hwf : p.well_typed env)
    (hdataflow : DataflowOk (p.to_program) initial_var) :
    WellTypedStrong (p.to_program) initial_var := by
  unfold WellTypedStrong
  refine ⟨?_, hdataflow, ?_⟩
  · exact IRProgram.to_program_preserves_well_typed_full p env hwf
  · exact IRProgram.to_program_FieldAccessOk p

end PMT
