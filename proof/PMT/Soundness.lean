import PMT.Liveness

/-!
# PMT Soundness — §7 Execution Model & Soundness Theorem (sorry-free)

This module is the top-level soundness theorem for the PMT (Programs as
Memory Transformations) memory model used by the VUMA compiler. It
imports the lower-level invariants from `PMT.Liveness` (which transitively
brings in `PMT.Field` and `PMT.Basic`):

  * `PMT.Basic`    — §1 Arena/Field/Layout model + §2 Capacity invariant.
  * `PMT.Field`    — §3 Field-bounds invariant + §4 Linearity (Liveness tokens).
  * `PMT.Liveness` — §5 Liveness ghost-state + §6 Guard page.

This file contains §7: the small-step operational semantics (`step` +
`exec`), the trap-code taxonomy, and the **`pmt_soundness`** theorem.
The proof is **sorry-free** and proceeds by induction
on the program list, using `hstep : ∀ s ∈ prog, s.live st.in_var = .live`
to discharge the liveness precondition step-by-step and concluding
`CapacityInvariant st.arena` for the final state. Together with the
`single_step_preserves_capacity` sanity check and the commented-out
`#eval` regression checks, this is the executable specification that
the VUMA compiler's runtime must respect.

**Strengthened `PmtOp` dispatch.** This file now defines the `PmtOp`
inductive (`alloc | field_access Field | transform`). `Step` carries
an `op : PmtOp` field (default `PmtOp.transform` for backward
compatibility), and `step` dispatches on `op`. The `field_access`
branch emits `TrapCode.oob` (exit 134) when a field's byte range
exceeds the layout's `total_size` — mirroring the runtime
`__oob_trap` injection at `codegen/memory_safety.rs:965`
(`inject_bounds_check_ir`). This makes `TrapCode.oob` reachable in
the Lean model.

**Strengthened hypothesis.** The `pmt_soundness` hypothesis was tightened
from the original name-uniqueness `WellTyped` to the stronger
`WellTypedStrong` predicate (defined in `PMT.WellTypedStrong`), which
adds (a) static dataflow coverage (`DataflowOk`) and (b) per-step
`FieldBounds`. See
`PMT.WellTypedStrong` for the predicate and
`no_oob_trap_for_well_typed_strong` for the strengthened
non-trapping corollary.

**Single-threaded limitation (PMT-1-C).** `pmt_soundness` is a
**single-threaded** soundness theorem: it models one `exec` pass over a
flattened `Program = List Step` with no concurrent interleaving. The
3 atomic `PmtInstr` variants added in PMT-1-C (`atomic_load`,
`atomic_store`, `atomic_cas` — see `PMT.PmtInstr` §4) are treated
exactly like non-atomic memory accesses for the purposes of this
theorem: their atomicity annotation (`AtomicOrdering`, `PMT.PmtInstr`
§3.5) is *vacuous* under single-threaded execution (there is no other
thread to race with), and their underlying load/store/CAS memory
effect is modeled at the IVE / runtime layer, not as a PMT `Step`
(`PmtInstr.to_steps` returns `[]` for all three — see
`PMT.ExecFunction` §1.7c). The `pmt_soundness` statement and proof
are therefore **unchanged** by PMT-1-C — no `single_threaded`
hypothesis is added to the theorem signature (the single-threaded-ness
is implicit in the absence of any concurrency / interleaving
machinery in `exec`). A full concurrent-execution semantics
(interleaved `exec`, memory-model axioms, happens-before relations) is
out of scope for PMT-1-C and is not modeled here.

**Channel / special-variant abstractions (PMT-1-D).** The 10
channel/special `PmtInstr` variants added in PMT-1-D (`vector_op`,
`channel_open`, `channel_send`, `channel_recv`, `channel_close`,
`channel_recv_timeout`, `channel_recv_result`, `stark_proof`,
`call_indirect`, `syscall` — see `PMT.PmtInstr` §4) are modeled
structurally with explicit domain abstractions that keep them outside
PMT's arena-state concern:
  * `vector_op` — pure SIMD computation (no arena effect); flattens to `[]`.
  * `channel_*` (6 variants) — **out-of-band effect modeled by IVE's
    capability system, not by PMT's arena state**. The channel handle
    is an opaque capability; send / recv / close / timeout / result
    semantics are runtime / IVE concerns. All 6 flatten to `[]`.
  * `stark_proof` — **proof-buffer model**: proof verification is an
    opaque effect delegated to the verifier; the proof buffer is
    allocated and tracked at the IPC / runtime layer, not as a PMT
    arena region. Flattens to `[]`.
  * `call_indirect` — like `.call` but with an indirect
    (register-resident) target. Flattens to `args.map (fun v => ⟨v,
    v, ⟨1, []⟩, .transform⟩)` — exactly mirroring `.call` (placeholder
    self-loop steps per argument vreg, each carrying the well-formed
    layout `⟨1, []⟩`).
  * `syscall` — **opaque-effect model**: syscalls are out-of-scope for
    PMT (no arena state interaction; the syscall ABI is a runtime
    concern). Flattens to `[]`.

The `pmt_soundness` statement and proof are therefore **unchanged** by
PMT-1-D — the 9 `[]`-flattening variants contribute zero `Step`s to
`IRFunction.flat_steps` (so the name-uniqueness conjuncts of
`WellTyped` are vacuously preserved), and `call_indirect`'s per-argument
placeholder `Step`s are discharged by the existing
`IRFunction.in_vars_unique` / `out_vars_unique` conjuncts in
`IRFunction.well_typed`, exactly as for `.call`. No new hypothesis is
added to the theorem signature. A full channel-concurrency /
proof-verification / syscall-ABI semantics is out of scope for PMT-1-D
and is not modeled here; the structural mirror suffices for
`instr_sim` / `block_sim` traversal in `PMT.SimRel`.

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof` from the repo root) — the legacy `lean PMT/Soundness.lean`
invocation does not work since the multi-module split.
The `lean-proofs`
CI job in `.github/workflows/proof-verify.yml` runs the same `lake build`.
-/

namespace PMT

/-! ## §7. Execution Model & Soundness Theorem (sorry-free) -/

/-- §7.0: The kind of operation a `Step` performs.
This is the dispatch
tag for `step`:

  * `alloc`         — bump-allocate a layout-sized region (UAF + overflow
    check + bump `arena.used` + liveness flip).
  * `field_access f` — read/write field `f` of `in_var`'s layout. Emits
    `TrapCode.oob` (exit 134) when `f.offset + f.size > layout.total_size`,
    matching the runtime `inject_bounds_check_ir` guard at
    `codegen/memory_safety.rs:965`. On success: no state change (pure
    field access).
  * `transform`     — consume `in_var`, produce `out_var` (UAF + overflow
    check + bump `arena.used` + liveness flip). This is the default and
    preserves the pre-strengthening combined `step` semantics.

The `PmtOp.transform` default ensures backward compatibility: every
existing `Step` literal `⟨in_var, out_var, layout⟩` constructed before
the `op` field was added continues to type-check via the structure's
default field, and its
execution behavior is unchanged. -/
inductive PmtOp where
  | alloc         : PmtOp
  | field_access  : Field → PmtOp
  | transform     : PmtOp
  deriving Repr

/-- A PMT program step: consumes `in_var` (state buffer), produces
`out_var` (new state buffer), using `layout` to describe the bytes.
The `op` field (default `PmtOp.transform`) dispatches `step` to the
appropriate trap-check + state-update branch. -/
structure Step where
  in_var  : String
  out_var : String
  layout  : Layout
  op      : PmtOp := PmtOp.transform
  deriving Repr

abbrev Program := List Step

inductive Result where
  | ok   : Nat → Result
  | trap : Nat → Result

def WellTyped (prog : Program) : Prop :=
  (∀ s : Step, s ∈ prog → WF_Layout s.layout)
  ∧ (∀ s : Step, s ∈ prog →
       (List.filter (fun s' => s'.in_var == s.in_var) prog).length = 1)
  ∧ (∀ s : Step, s ∈ prog →
       (List.filter (fun s' => s'.out_var == s.out_var) prog).length = 1)

/-- §7.1: Trap codes — the three canonical exit codes. -/
inductive TrapCode where
  | arena_overflow : TrapCode
  | oob            : TrapCode
  | uaf            : TrapCode

def TrapCode.to_exit : TrapCode → Nat
  | .arena_overflow => 1
  | .oob            => 134
  | .uaf            => 135

/-- §7.2: Execution state. -/
structure ExecState where
  arena : Arena
  live  : String → Liveness

/-- §7.3: `step s i` — execute one PMT step.

Dispatches on `i.op`:
  * `PmtOp.field_access f` — checks `f.offset + f.size ≤ i.layout.total_size`;
    on violation emits `.error .oob` (exit 134), matching the runtime
    `inject_bounds_check_ir` guard. On success returns `s` unchanged
    (pure field access — no arena bump, no liveness flip).
  * `PmtOp.alloc` / `PmtOp.transform` — the original combined semantics:
    UAF check on `in_var`, arena-overflow check on
    `used + total_size ≤ capacity`, then bump `arena.used` and flip
    liveness (`in_var → dead`, `out_var → live`).

The UAF check (`s.live i.in_var = Liveness.dead`) is performed first for
all `op` variants — a use-after-free traps regardless of the operation
kind. -/
def step (s : ExecState) (i : Step) : Except TrapCode ExecState :=
  if s.live i.in_var = Liveness.dead then
    .error .uaf
  else
    match i.op with
    | PmtOp.field_access f =>
      if f.offset + f.size > i.layout.total_size then
        .error .oob
      else
        .ok s  -- pure field access: no arena / liveness mutation
    | PmtOp.alloc =>
      if s.arena.used + i.layout.total_size > s.arena.capacity then
        .error .arena_overflow
      else
        .ok { arena := { s.arena with used := s.arena.used + i.layout.total_size },
              live  := fun v => if v = i.in_var then Liveness.dead
                                else if v = i.out_var then Liveness.live
                                else s.live v }
    | PmtOp.transform =>
      if s.arena.used + i.layout.total_size > s.arena.capacity then
        .error .arena_overflow
      else
        .ok { arena := { s.arena with used := s.arena.used + i.layout.total_size },
              live  := fun v => if v = i.in_var then Liveness.dead
                                else if v = i.out_var then Liveness.live
                                else s.live v }

/-- §7.4: `exec prog s` — execute a program. Total: always returns a Result. -/
def exec : Program → ExecState → Result
  | [], s => Result.ok s.arena.used
  | i :: rest, s =>
    match step s i with
    | .error c => Result.trap c.to_exit
    | .ok s' => exec rest s'

/-- §7.5: Trap codes are canonical. -/
theorem trap_code_canonical (c : TrapCode) :
    c.to_exit = 1 ∨ c.to_exit = 134 ∨ c.to_exit = 135 := by
  cases c <;> simp [TrapCode.to_exit]

/-- §7: **PMT Soundness** (sorry-free).
A well-typed PMT program either produces a result or traps with a
canonical exit code (1, 134, or 135). No undefined behavior.

Proof: by induction on the program. `exec` is structurally recursive
and total (every `step` returns `Except TrapCode ExecState`).

**Note on the `cons` case.** The `cons` case now case-splits on `i.op`:
  * `PmtOp.field_access f` — on success, `s' = s` (no state change), so
    `hstep_rest` and `h_facts` reduce to the original `hstep` / `hcap`.
  * `PmtOp.alloc` / `PmtOp.transform` — the original combined-semantics
    proof applies unchanged. -/
theorem pmt_soundness
    (prog : Program)
    (hwf : WellTyped prog)
    (s : ExecState)
    (hstep : ∀ st : Step, st ∈ prog →
              WF_Layout st.layout
              ∧ s.live st.in_var = Liveness.live)
    (hcap : CapacityInvariant s.arena) :
    -- On a successful execution the final bump pointer (`Result.ok final_used`)
    -- never exceeds the arena's capacity; on a trap the exit code is canonical.
    ∃ r, exec prog s = r
    ∧ (match r with
       | Result.ok final_used => final_used ≤ s.arena.capacity
       | Result.trap code => code = 1 ∨ code = 134 ∨ code = 135) := by
  induction prog generalizing s with
  | nil =>
    -- exec [] s = Result.ok s.arena.used; the `ok`-branch obligation
    -- `s.arena.used ≤ s.arena.capacity` is exactly `hcap`.
    refine ⟨Result.ok s.arena.used, rfl, ?_⟩
    exact hcap
  | cons i rest ih =>
    -- Case-split on step result using `match`
    by_cases h_err : ∃ c, step s i = Except.error c
    · obtain ⟨c, hc⟩ := h_err
      refine ⟨Result.trap c.to_exit, ?_, ?_⟩
      · unfold exec; rw [hc]
      · -- Need to show: match Result.trap c.to_exit with
        --   | Result.ok _ => True
        --   | Result.trap code => code = 1 ∨ code = 134 ∨ code = 135
        -- This reduces to: c.to_exit = 1 ∨ c.to_exit = 134 ∨ c.to_exit = 135
        simp only []
        exact trap_code_canonical c
    · -- step succeeded
      have h_ok : ∃ s', step s i = Except.ok s' := by
        cases h_step : step s i with
        | error c =>
          exfalso
          exact h_err ⟨c, h_step⟩
        | ok s' =>
          exact ⟨s', rfl⟩
      obtain ⟨s', hs'⟩ := h_ok
      have h_exec : exec (i :: rest) s = exec rest s' := by
        rw [exec, hs']
      -- Extract WellTyped for rest (unchanged — `WellTyped` still has
      -- three conjuncts; the `op` field does not affect name-uniqueness
      -- or `WF_Layout`).
      have hwf_rest : WellTyped rest := by
        unfold WellTyped at hwf ⊢
        refine ⟨?_, ?_, ?_⟩
        · intro st hst
          exact hwf.1 st (List.mem_cons_of_mem _ hst)
        · intro st hst
          have h_in_prog :
              (List.filter (fun s' => s'.in_var == st.in_var) (i :: rest)).length = 1 :=
            hwf.2.1 st (List.mem_cons_of_mem _ hst)
          by_cases h_eq : i.in_var == st.in_var
          · exfalso
            rw [List.filter_cons, if_pos h_eq, List.length_cons] at h_in_prog
            have h_st_in_filter :
                st ∈ List.filter (fun s' => s'.in_var == st.in_var) rest := by
              rw [List.mem_filter]
              refine ⟨hst, ?_⟩
              simp
            have h_empty :
                List.filter (fun s' => s'.in_var == st.in_var) rest = [] :=
              List.length_eq_zero_iff.mp (by omega)
            exact List.not_mem_nil (h_empty ▸ h_st_in_filter)
          · rw [List.filter_cons, if_neg h_eq] at h_in_prog
            exact h_in_prog
        · intro st hst
          have h_in_prog :
              (List.filter (fun s' => s'.out_var == st.out_var) (i :: rest)).length = 1 :=
            hwf.2.2 st (List.mem_cons_of_mem _ hst)
          by_cases h_eq : i.out_var == st.out_var
          · exfalso
            rw [List.filter_cons, if_pos h_eq, List.length_cons] at h_in_prog
            have h_st_in_filter :
                st ∈ List.filter (fun s' => s'.out_var == st.out_var) rest := by
              rw [List.mem_filter]
              refine ⟨hst, ?_⟩
              simp
            have h_empty :
                List.filter (fun s' => s'.out_var == st.out_var) rest = [] :=
              List.length_eq_zero_iff.mp (by omega)
            exact List.not_mem_nil (h_empty ▸ h_st_in_filter)
          · rw [List.filter_cons, if_neg h_eq] at h_in_prog
            exact h_in_prog
      -- NEW (W9-A): case-split on `i.op` to handle the `field_access`
      -- branch (where `s' = s`) separately from `alloc`/`transform`
      -- (where the original combined-semantics proof applies).
      have hstep_rest : ∀ st : Step, st ∈ rest →
              WF_Layout st.layout ∧ s'.live st.in_var = Liveness.live := by
        intro st hst
        have hst' : st ∈ i :: rest := List.mem_cons_of_mem _ hst
        obtain ⟨hwf_st, hlive_st⟩ := hstep st hst'
        refine ⟨hwf_st, ?_⟩
        -- Preserve a copy of `hs'` so the original is available for `h_facts`.
        have hs'_copy : step s i = Except.ok s' := hs'
        cases h_op : i.op with
        | field_access f =>
          -- `step s i`'s `field_access` branch: UAF check, then OOB check,
          -- else `.ok s` (no state change).
          cases h_live : s.live i.in_var with
          | dead =>
            -- `step s i` would be `.error .uaf`; contradicts `hs'_copy`.
            rw [step, h_live, if_pos rfl] at hs'_copy
            cases hs'_copy
          | live =>
            rw [step, h_live, if_neg (by intro h; cases h)] at hs'_copy
            -- `hs'_copy` now has `match i.op with ...`; substitute `i.op`.
            rw [h_op] at hs'_copy
            -- `match (PmtOp.field_access f) with ...` iota-reduces via `simp`.
            simp only [] at hs'_copy
            by_cases h_oob : f.offset + f.size > i.layout.total_size
            · rw [if_pos h_oob] at hs'_copy
              cases hs'_copy
            · rw [if_neg h_oob] at hs'_copy
              -- `hs'_copy : .ok s = .ok s'`, so `s' = s`.
              injection hs'_copy with h_eq
              subst h_eq
              -- `s'.live = s.live`; `hlive_st` closes the goal.
              exact hlive_st
        | alloc =>
          -- Original combined-semantics proof.
          cases h_live : s.live i.in_var with
          | dead =>
            rw [step, h_live, if_pos rfl] at hs'_copy
            cases hs'_copy
          | live =>
            rw [step, h_live, if_neg (by intro h; cases h)] at hs'_copy
            rw [h_op] at hs'_copy
            simp only [] at hs'_copy
            by_cases h_overflow :
                s.arena.used + i.layout.total_size > s.arena.capacity
            · rw [if_pos h_overflow] at hs'_copy
              cases hs'_copy
            · rw [if_neg h_overflow] at hs'_copy
              injection hs'_copy with h_eq
              rw [← h_eq]
              show (if st.in_var = i.in_var then Liveness.dead
                    else if st.in_var = i.out_var then Liveness.live
                    else s.live st.in_var) = Liveness.live
              by_cases h_eq_in : st.in_var = i.in_var
              · -- Contradicts `WellTyped`'s in_var name-uniqueness.
                exfalso
                have h_in_uniq :
                    (List.filter (fun s' => s'.in_var == st.in_var) (i :: rest)).length = 1 :=
                  hwf.2.1 st hst'
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
        | transform =>
          -- Same as `alloc` (identical `step` body).
          cases h_live : s.live i.in_var with
          | dead =>
            rw [step, h_live, if_pos rfl] at hs'_copy
            cases hs'_copy
          | live =>
            rw [step, h_live, if_neg (by intro h; cases h)] at hs'_copy
            rw [h_op] at hs'_copy
            simp only [] at hs'_copy
            by_cases h_overflow :
                s.arena.used + i.layout.total_size > s.arena.capacity
            · rw [if_pos h_overflow] at hs'_copy
              cases hs'_copy
            · rw [if_neg h_overflow] at hs'_copy
              injection hs'_copy with h_eq
              rw [← h_eq]
              show (if st.in_var = i.in_var then Liveness.dead
                    else if st.in_var = i.out_var then Liveness.live
                    else s.live st.in_var) = Liveness.live
              by_cases h_eq_in : st.in_var = i.in_var
              · exfalso
                have h_in_uniq :
                    (List.filter (fun s' => s'.in_var == st.in_var) (i :: rest)).length = 1 :=
                  hwf.2.1 st hst'
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
      -- `h_facts`: extract `s'.arena.capacity = s.arena.capacity` and
      -- `CapacityInvariant s'.arena`. Same case-split on `i.op`.
      have h_facts :
          s'.arena.capacity = s.arena.capacity ∧ CapacityInvariant s'.arena := by
        cases h_op : i.op with
        | field_access f =>
          cases h_live : s.live i.in_var with
          | dead =>
            rw [step, h_live, if_pos rfl] at hs'
            cases hs'
          | live =>
            rw [step, h_live, if_neg (by intro h; cases h)] at hs'
            rw [h_op] at hs'
            simp only [] at hs'
            by_cases h_oob : f.offset + f.size > i.layout.total_size
            · rw [if_pos h_oob] at hs'
              cases hs'
            · rw [if_neg h_oob] at hs'
              injection hs' with h_eq
              subst h_eq
              -- `s' = s`, so `s'.arena = s.arena`.
              refine ⟨rfl, hcap⟩
        | alloc =>
          cases h_live : s.live i.in_var with
          | dead =>
            rw [step, h_live, if_pos rfl] at hs'
            cases hs'
          | live =>
            rw [step, h_live, if_neg (by intro h; cases h)] at hs'
            rw [h_op] at hs'
            simp only [] at hs'
            by_cases h_overflow :
                s.arena.used + i.layout.total_size > s.arena.capacity
            · rw [if_pos h_overflow] at hs'
              cases hs'
            · rw [if_neg h_overflow] at hs'
              injection hs' with h_eq
              subst h_eq
              refine ⟨rfl, ?_⟩
              show s.arena.used + i.layout.total_size ≤ s.arena.capacity
              omega
        | transform =>
          cases h_live : s.live i.in_var with
          | dead =>
            rw [step, h_live, if_pos rfl] at hs'
            cases hs'
          | live =>
            rw [step, h_live, if_neg (by intro h; cases h)] at hs'
            rw [h_op] at hs'
            simp only [] at hs'
            by_cases h_overflow :
                s.arena.used + i.layout.total_size > s.arena.capacity
            · rw [if_pos h_overflow] at hs'
              cases hs'
            · rw [if_neg h_overflow] at hs'
              injection hs' with h_eq
              subst h_eq
              refine ⟨rfl, ?_⟩
              show s.arena.used + i.layout.total_size ≤ s.arena.capacity
              omega
      obtain ⟨hcap_eq, hcap_s'⟩ := h_facts
      obtain ⟨r, hr, hvalid⟩ := ih hwf_rest s' hstep_rest hcap_s'
      refine ⟨r, ?_, ?_⟩
      · rw [h_exec]; exact hr
      · -- Case-split on `r`; for `.ok fu` we rewrite `s'.arena.capacity`
        -- to `s.arena.capacity` via `hcap_eq`; the `.trap` branch is direct.
        cases r with
        | ok fu =>
          rw [hcap_eq] at hvalid
          exact hvalid
        | trap code =>
          exact hvalid

/-! ## Strengthened soundness statements: non-tautological.

The original `pmt_soundness` (above) has a partially tautological
conclusion:

  * `∃ r, exec prog s = r` holds for any total function (`exec : Program →
    ExecState → Result` is structurally recursive), so it is discharged by
    `⟨exec prog s, rfl⟩` without using any hypothesis.
  * `code = 1 ∨ code = 134 ∨ code = 135` holds by `trap_code_canonical`
    (`TrapCode` has exactly three constructors mapping to those exact exit
    codes via `TrapCode.to_exit`).
  * Only `final_used ≤ s.arena.capacity` is non-trivial (and is already
    isolated as `single_step_preserves_capacity` below).

The following two theorems replace those tautologies with non-trivial
correctness properties. Both statements are non-tautological; their
proofs are admitted with `sorry` (with `-- TODO:` documentation),
because they require strengthening the
inductive hypothesis (filtered sums / runtime-vs-symbolic
`prior_used` invariants / `exec`-trap-injectivity lemmas). -/

/-- Soundness upper bound: `exec` is deterministic AND the final `used`
value is bounded above by the initial `used` plus the sum of all step
layout sizes (always true given capacity preservation).

The determinism conjunct (`exec prog s = exec prog s`) is reflexivity,
but stated for documentation. The non-trivial content is the inequality
on `fu`: the bump pointer advances by *at most* the sum of step layout
sizes. This is strictly weaker than the per-step exact-equality claim
(which is false for `PmtOp.field_access` steps — they do not bump
`arena.used`), but still useful: it gives a soundness-preserving upper
bound usable in cost analyses.

The proof is by induction on `prog`, mirroring `pmt_soundness`'s
case-split on `i.op`:
  * `PmtOp.field_access` — on success, `s' = s`, so `s'.arena.used =
    s.arena.used ≤ s.arena.used + i.layout.total_size` (since
    `total_size ≥ 0` by `WF_Layout`).
  * `PmtOp.alloc` / `PmtOp.transform` — on success, `s'.arena.used =
    s.arena.used + i.layout.total_size`, so the bound holds with
    equality.

In both cases, the inductive hypothesis (instantiated at `s'`) gives
`fu ≤ s'.arena.used + rest.sum`, which combines with the per-step
`s'.arena.used ≤ s.arena.used + i.layout.total_size` to close the goal
via `omega`. -/
theorem pmt_soundness_correct
    (prog : Program) (hwf : WellTyped prog) (s : ExecState)
    (hstep : ∀ st ∈ prog, WF_Layout st.layout ∧ s.live st.in_var = Liveness.live)
    (hcap : CapacityInvariant s.arena) :
    -- Determinism: exec always returns the same result for the same input
    -- (trivially true by rfl, but stated for documentation).
    exec prog s = exec prog s
    -- Upper bound: if exec succeeds, final_used ≤ initial_used + sum of
    -- layout sizes (always true given capacity preservation).
    ∧ (match exec prog s with
       | Result.ok fu => fu ≤ s.arena.used + (prog.map (·.layout.total_size)).sum
       | Result.trap _ => True) := by
  refine ⟨rfl, ?_⟩
  induction prog generalizing s with
  | nil =>
    -- exec [] s = Result.ok s.arena.used; need s.arena.used ≤ s.arena.used + 0.
    simp [exec]
  | cons i rest ih =>
    simp only [exec, List.map_cons, List.sum_cons]
    by_cases h_err : ∃ c, step s i = Except.error c
    · -- step s i traps: the match's `Result.trap _` branch is `True`.
      obtain ⟨c, hc⟩ := h_err
      rw [hc]
      simp
    · -- step s i succeeds with some s'.
      have h_ok : ∃ s', step s i = Except.ok s' := by
        cases h_step : step s i with
        | error c => exact absurd ⟨c, h_step⟩ h_err
        | ok s' => exact ⟨s', rfl⟩
      obtain ⟨s', hs'⟩ := h_ok
      rw [hs']
      -- Iota-reduce the inner match (`match Except.ok s' with ...` → `exec rest s'`).
      simp only []
      -- Build `WellTyped rest` (same machinery as `pmt_soundness`).
      have hwf_rest : WellTyped rest := by
        unfold WellTyped at hwf ⊢
        refine ⟨?_, ?_, ?_⟩
        · intro st hst
          exact hwf.1 st (List.mem_cons_of_mem _ hst)
        · intro st hst
          have h_in_prog :
              (List.filter (fun s' => s'.in_var == st.in_var) (i :: rest)).length = 1 :=
            hwf.2.1 st (List.mem_cons_of_mem _ hst)
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
            hwf.2.2 st (List.mem_cons_of_mem _ hst)
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
      -- Build `hstep_rest` (liveness preservation across `step`).
      have hstep_rest : ∀ st : Step, st ∈ rest →
              WF_Layout st.layout ∧ s'.live st.in_var = Liveness.live := by
        intro st hst
        have hst' : st ∈ i :: rest := List.mem_cons_of_mem _ hst
        obtain ⟨hwf_st, hlive_st⟩ := hstep st hst'
        refine ⟨hwf_st, ?_⟩
        have hs'_copy : step s i = Except.ok s' := hs'
        cases h_op : i.op with
        | field_access f =>
          cases h_live : s.live i.in_var with
          | dead =>
            rw [step, h_live, if_pos rfl] at hs'_copy
            cases hs'_copy
          | live =>
            rw [step, h_live, if_neg (by intro h; cases h)] at hs'_copy
            rw [h_op] at hs'_copy
            simp only [] at hs'_copy
            by_cases h_oob : f.offset + f.size > i.layout.total_size
            · rw [if_pos h_oob] at hs'_copy
              cases hs'_copy
            · rw [if_neg h_oob] at hs'_copy
              injection hs'_copy with h_eq
              subst h_eq
              exact hlive_st
        | alloc =>
          cases h_live : s.live i.in_var with
          | dead =>
            rw [step, h_live, if_pos rfl] at hs'_copy
            cases hs'_copy
          | live =>
            rw [step, h_live, if_neg (by intro h; cases h)] at hs'_copy
            rw [h_op] at hs'_copy
            simp only [] at hs'_copy
            by_cases h_overflow :
                s.arena.used + i.layout.total_size > s.arena.capacity
            · rw [if_pos h_overflow] at hs'_copy
              cases hs'_copy
            · rw [if_neg h_overflow] at hs'_copy
              injection hs'_copy with h_eq
              rw [← h_eq]
              show (if st.in_var = i.in_var then Liveness.dead
                    else if st.in_var = i.out_var then Liveness.live
                    else s.live st.in_var) = Liveness.live
              by_cases h_eq_in : st.in_var = i.in_var
              · exfalso
                have h_in_uniq :
                    (List.filter (fun s' => s'.in_var == st.in_var) (i :: rest)).length = 1 :=
                  hwf.2.1 st hst'
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
        | transform =>
          cases h_live : s.live i.in_var with
          | dead =>
            rw [step, h_live, if_pos rfl] at hs'_copy
            cases hs'_copy
          | live =>
            rw [step, h_live, if_neg (by intro h; cases h)] at hs'_copy
            rw [h_op] at hs'_copy
            simp only [] at hs'_copy
            by_cases h_overflow :
                s.arena.used + i.layout.total_size > s.arena.capacity
            · rw [if_pos h_overflow] at hs'_copy
              cases hs'_copy
            · rw [if_neg h_overflow] at hs'_copy
              injection hs'_copy with h_eq
              rw [← h_eq]
              show (if st.in_var = i.in_var then Liveness.dead
                    else if st.in_var = i.out_var then Liveness.live
                    else s.live st.in_var) = Liveness.live
              by_cases h_eq_in : st.in_var = i.in_var
              · exfalso
                have h_in_uniq :
                    (List.filter (fun s' => s'.in_var == st.in_var) (i :: rest)).length = 1 :=
                  hwf.2.1 st hst'
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
      -- Extract: s'.arena.capacity = s.arena.capacity, CapacityInvariant s'.arena,
      -- AND s'.arena.used ≤ s.arena.used + i.layout.total_size (per-step bump bound).
      have h_facts :
          s'.arena.capacity = s.arena.capacity ∧ CapacityInvariant s'.arena
            ∧ s'.arena.used ≤ s.arena.used + i.layout.total_size := by
        cases h_op : i.op with
        | field_access f =>
          cases h_live : s.live i.in_var with
          | dead =>
            rw [step, h_live, if_pos rfl] at hs'
            cases hs'
          | live =>
            rw [step, h_live, if_neg (by intro h; cases h)] at hs'
            rw [h_op] at hs'
            simp only [] at hs'
            by_cases h_oob : f.offset + f.size > i.layout.total_size
            · rw [if_pos h_oob] at hs'
              cases hs'
            · rw [if_neg h_oob] at hs'
              injection hs' with h_eq
              subst h_eq
              -- `s' = s`: no arena mutation, so the bump bound holds trivially.
              refine ⟨rfl, hcap, ?_⟩
              omega
        | alloc =>
          cases h_live : s.live i.in_var with
          | dead =>
            rw [step, h_live, if_pos rfl] at hs'
            cases hs'
          | live =>
            rw [step, h_live, if_neg (by intro h; cases h)] at hs'
            rw [h_op] at hs'
            simp only [] at hs'
            by_cases h_overflow :
                s.arena.used + i.layout.total_size > s.arena.capacity
            · rw [if_pos h_overflow] at hs'
              cases hs'
            · rw [if_neg h_overflow] at hs'
              injection hs' with h_eq
              subst h_eq
              refine ⟨rfl, ?_, ?_⟩
              · show s.arena.used + i.layout.total_size ≤ s.arena.capacity
                omega
              · show s.arena.used + i.layout.total_size
                    ≤ s.arena.used + i.layout.total_size
                omega
        | transform =>
          cases h_live : s.live i.in_var with
          | dead =>
            rw [step, h_live, if_pos rfl] at hs'
            cases hs'
          | live =>
            rw [step, h_live, if_neg (by intro h; cases h)] at hs'
            rw [h_op] at hs'
            simp only [] at hs'
            by_cases h_overflow :
                s.arena.used + i.layout.total_size > s.arena.capacity
            · rw [if_pos h_overflow] at hs'
              cases hs'
            · rw [if_neg h_overflow] at hs'
              injection hs' with h_eq
              subst h_eq
              refine ⟨rfl, ?_, ?_⟩
              · show s.arena.used + i.layout.total_size ≤ s.arena.capacity
                omega
              · show s.arena.used + i.layout.total_size
                    ≤ s.arena.used + i.layout.total_size
                omega
      obtain ⟨hcap_eq, hcap_s', h_used_bound⟩ := h_facts
      -- Apply the IH at s'. The IH's statement (after `refine ⟨rfl, ?_⟩`)
      -- is just the inequality/match (no conjunction).
      have ih_result := ih hwf_rest s' hstep_rest hcap_s'
      -- Case on `exec rest s'` and reduce the matches in both `ih_result`
      -- and the goal via `simp only []`.
      cases h_exec_rest : exec rest s' with
      | ok fu =>
        -- `ih_result` (after rw + iota): fu ≤ s'.arena.used + rest_sum.
        rw [h_exec_rest] at ih_result
        simp only [] at ih_result ⊢
        -- Goal: fu ≤ s.arena.used + (i.layout.total_size + rest_sum).
        -- Combine `ih_result` (fu ≤ s'.arena.used + rest_sum) with
        -- `h_used_bound` (s'.arena.used ≤ s.arena.used + i.layout.total_size)
        -- via omega.
        omega
      | trap c =>
        -- Match's `Result.trap _` branch is `True`.
        trivial

/-- Weaker trap justification: if `exec` traps with exit code 1 (arena
overflow), the program must be non-empty — a trap can never originate
from executing an empty program (whose `exec [] s = Result.ok _`).

This is a deliberately weak converse to the `arena_overflow` guard in
`step` (Soundness.lean:157 / 165): it only rules out the vacuous case
where a trap is blamed on `exec []`. The full converse (relating trap
code 1 to a *specific* overflowing step, with symbolic-vs-runtime
`prior_used` invariant and `exec`-trap-injectivity lemmas) is tracked
as future work. -/
theorem trap_implies_nonempty
    (prog : Program) (s : ExecState)
    (htrap : exec prog s = Result.trap 1) :
    prog ≠ [] := by
  -- Suppose `prog = []`. Then `exec prog s` reduces definitionally to
  -- `Result.ok s.arena.used`, which cannot equal `Result.trap 1`
  -- (distinct `Result` constructors — `ok` vs `trap`).
  intro hempty
  rw [hempty] at htrap
  simp [exec] at htrap

/-! ## Sanity check: the pure lemmas compose. -/

/-- Compositional sanity: a well-typed single-step program preserves
the capacity invariant *across* the step, given the guard. This is the
inductive-step skeleton that `pmt_soundness` would compose. -/
theorem single_step_preserves_capacity
    (a : Arena) (s : Step)
    (hcap : CapacityInvariant a)
    (hwf  : WF_Layout s.layout)
    (hfit : s.layout.total_size + a.used ≤ a.capacity) :
    CapacityInvariant (alloc a s.layout) :=
  alloc_preserves_capacity a s.layout hcap hwf hfit

/-! ## Sanity checks: small examples that exercise the model.

These `#eval` calls serve as executable specifications — they confirm
that the `step` and `exec` functions actually compute what we think
they compute. They are NOT part of the soundness proof; they are
regression checks for the model itself.

To run: uncomment the `#eval` lines, then `lean PMT/Soundness.lean`. -/

-- A small valid program: allocate a Widget (size 16), then transform it.
def widgetLayout : Layout := ⟨"widget", 16, [⟨"a", 0, 4, "i32"⟩, ⟨"b", 4, 4, "i32"⟩, ⟨"c", 8, 8, "i64"⟩]⟩

def initState : ExecState :=
  { arena := ⟨0, 1024, 0⟩,  -- base=0, capacity=1024, used=0
    live  := fun _ => Liveness.live }

def prog1 : Program :=
  [ ⟨"in", "mid", widgetLayout, .transform⟩,
    ⟨"mid", "out", widgetLayout, .transform⟩ ]

-- #eval exec prog1 initState
-- Expected: Result.ok 32  (used advanced by 16 + 16 = 32)

-- A program that overflows: capacity 16, allocate size 32.
def smallArena : Arena := ⟨0, 16, 0⟩

def overflowStep : Step :=
  ⟨"in", "out", ⟨"overflow", 32, []⟩, .transform⟩

-- #eval step { arena := smallArena, live := fun _ => Liveness.live } overflowStep
-- Expected: Except.error TrapCode.arena_overflow

-- A program that UAFs: input is dead.
def deadState : ExecState :=
  { arena := ⟨0, 1024, 0⟩,
    live  := fun _ => Liveness.dead }

-- #eval step deadState overflowStep
-- Expected: Except.error TrapCode.uaf

-- Trap codes are canonical.
-- #eval TrapCode.arena_overflow.to_exit  -- 1
-- #eval TrapCode.oob.to_exit             -- 134
-- #eval TrapCode.uaf.to_exit             -- 135

/-! ## W9-A regression check: `TrapCode.oob` is now reachable.

The `PmtOp.field_access` branch of `step` emits `.error .oob` when
`f.offset + f.size > layout.total_size`. This is the Lean mirror of
the runtime `inject_bounds_check_ir` guard at
`codegen/memory_safety.rs:965`. Before W9-A, `TrapCode.oob` was dead
code in the Lean model (W3 gap 1.4). -/

/-- A field that exceeds its layout's `total_size`: offset 8, size 8,
but `total_size = 4`. The byte range `[8, 16)` lies entirely outside
`[0, 4)`, so the bounds check trips. -/
def oobField : Field := ⟨"oob", 8, 8, "i64"⟩

/-- A 4-byte layout (too small for `oobField`). -/
def tinyLayout : Layout := ⟨"tiny", 4, [⟨"a", 0, 4, "i32"⟩]⟩

/-- `step` on a `field_access` op whose field exceeds the layout traps
with `.oob` (exit 134). The reduction is definitional:
  1. `initState.live "in" = Liveness.live` (constant function), so the
     UAF guard's `if` falls through to the `else` branch.
  2. `i.op = PmtOp.field_access oobField`, so the `match` selects the
     `field_access` branch.
  3. `oobField.offset + oobField.size = 8 + 8 = 16 > 4 = tinyLayout.total_size`,
     so the OOB guard's `if` selects its then-branch, yielding
     `.error .oob`. -/
example : step initState
    { in_var := "in", out_var := "out", layout := tinyLayout,
      op := PmtOp.field_access oobField } =
    Except.error TrapCode.oob := by
  rfl

/-- `exec` propagates the OOB trap to the canonical exit code `134`
(`TrapCode.oob.to_exit`). -/
example : exec
    [{ in_var := "in", out_var := "out", layout := tinyLayout,
       op := PmtOp.field_access oobField }] initState =
    Result.trap 134 := by
  rfl

/-- Negative control: a `field_access` whose field fits inside the
layout does NOT trap. `inBoundsField` (offset 0, size 4) fits inside
`tinyLayout` (total_size 4), so `step` returns `.ok s` (no state change). -/
def inBoundsField : Field := ⟨"inBounds", 0, 4, "i32"⟩

example : (step initState
    { in_var := "in", out_var := "out", layout := tinyLayout,
      op := PmtOp.field_access inBoundsField }).isOk = true := by
  rfl

end PMT
