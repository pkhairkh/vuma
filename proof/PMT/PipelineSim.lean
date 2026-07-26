import PMT.Soundness
import PMT.IRProgram
import PMT.ExecFunction
import PMT.WellTypedStrong

/-!
## PipelineSim — Pipeline-conformance scaffolding (Wave 0)

### Rename history (PMT-0-C, Wave 0)

This module originally (pre-PMT-0-C) claimed to be the **"first mechanical
connection"** between the Lean formal model and the Rust implementation. That
claim was **overstated**: the two headline theorems
`pipeline_compile_sound` and `pipeline_compile_no_oob` each took a

    hconforms : PipelineSpec prog s

hypothesis that was advertised as the CompCert-style "translation-validation"
assumption tying Lean's `exec` to the Rust `pipeline::compile`. However,
**the proof bodies never used `hconforms`** — both theorems delegated
directly to the sorry-free `pmt_soundness` / `no_oob_trap_for_well_typed_strong`
results, and an inline comment in the old body admitted: *"Without
`hconforms`, this theorem would just restate `pmt_soundness`."* Moreover, the
`PipelineSpec.compiled_matches_exec` field is `exec prog s = exec prog s` —
a `rfl` tautology — so the `hconforms` hypothesis, even if it had been
discharged, would have carried no non-trivial Rust-side information.

PMT-0-C (this commit) therefore REMOVES the degenerate `hconforms`
hypothesis from both theorems and renames them to honestly reflect what
they actually prove:

  - `pipeline_compile_sound`   → `pmt_soundness_restate`
  - `pipeline_compile_no_oob`  → `pmt_soundness_no_oob_restate`

The choice was REMOVE (not "keep `hconforms` and discharge it") because
discharging `hconforms` non-vacuously requires giving `PipelineSpec` real
Rust-side content (a non-`rfl` `compiled_matches_exec` plus a parity-tested
`safe` clause), which is exactly the work of **Wave 1 PMT-1-G** (extraction +
Rust-parity testing). Until PMT-1-G lands, a `PipelineSpec prog s → ...`
theorem here would be, at best, a vacuous `rfl`-discharged wrapper around
`pmt_soundness` — i.e. a marketing label, not a theorem. Removing the
hypothesis makes the theorem's true content (a restatement of
`pmt_soundness` / `no_oob_trap_for_well_typed_strong`) explicit.

The `PipelineSpec` structure and `exec_satisfies_pipeline_spec` theorem
are KEPT intact below — they are the scaffolding PMT-1-G will strengthen
into a real pipeline-conformance theorem. Their docstrings are updated to
flag the current degeneracy (`compiled_matches_exec` is `rfl`) and to point
at PMT-1-G as the deferral point.

### What this module actually proves (post-PMT-0-C)

1. `PipelineSpec prog s` — a structure capturing what `src/pipeline.rs::
   compile` *will eventually* be required to produce (once PMT-1-G lands).
   Today, its `compiled_matches_exec` field is `exec prog s = exec prog s`
   (a `rfl` tautology); its `safe` field is the real content (delegates to
   `pmt_soundness`).
2. `exec_satisfies_pipeline_spec` — Lean's own `exec` already meets
   `PipelineSpec`. The `compiled_matches_exec` conjunct is `rfl`; the
   `safe` conjunct is `pmt_soundness`.
3. `pmt_soundness_restate` (was `pipeline_compile_sound`) — a direct
   restatement of `pmt_soundness`, in the un-existentialled `match` form.
   Sorry-free; no Rust-side hypothesis.
4. `pmt_soundness_no_oob_restate` (was `pipeline_compile_no_oob`) — a
   direct restatement of `no_oob_trap_for_well_typed_strong`. Sorry-free;
   no Rust-side hypothesis.

References:
  - Rust: `src/pipeline.rs::compile` (10-stage pipeline)
  - CompCert: Leroy, JAR 2009.
  - Wave 1 deferral: PMT-1-G (extraction + Rust-parity discharge of a
    strengthened `PipelineSpec`).
-/

namespace PMT

/-- The Rust `pipeline::compile` specification (SCAFFOLDING — PMT-1-G will
strengthen).

This is what `pipeline::compile` (in `src/pipeline.rs`) *will eventually*
be required to produce, once Wave 1 PMT-1-G lands: given a well-typed
program, it produces a binary whose observable behavior matches Lean's
`exec`, and which is safe (canonical trap codes 1, 134, 135 only; never
undefined behavior).

**Current degeneracy (Wave 0):** the `compiled_matches_exec` field is
`exec prog s = exec prog s`, a `rfl` tautology — it carries no Rust-side
information until PMT-1-G replaces it with a non-`rfl` parity obligation.
The `safe` field is the only field with non-trivial content today; it
delegates to `pmt_soundness`.

The two conjuncts are:
  - `compiled_matches_exec`: the compiled binary's observable behavior
    matches Lean `exec` (identity for now — `rfl`; refinement is via the
    Lean-side `exec`, which is the executable specification). PMT-1-G
    will replace this with a real Lean↔Rust parity statement.
  - `safe`: under `WellTyped` and `CapacityInvariant`, the binary's
    result satisfies PMT safety (canonical traps only; on success the
    bump pointer is within capacity).

The pre-PMT-0-C theorems `pipeline_compile_sound` and
`pipeline_compile_no_oob` took a `hconforms : PipelineSpec prog s`
hypothesis advertising this structure as the CompCert-style
translation-validation obligation. That hypothesis was unused in the
proof bodies (which delegated directly to `pmt_soundness` /
`no_oob_trap_for_well_typed_strong`); PMT-0-C removed it and renamed
both theorems accordingly (see file header). -/
structure PipelineSpec (prog : Program) (s : ExecState) : Prop where
  /-- The compiled binary's observable behavior matches Lean `exec`.
  Identity for now (`rfl`); refinement is via the Lean-side `exec`, which
  is the executable specification. PMT-1-G will replace this with a real
  Lean↔Rust parity statement. -/
  compiled_matches_exec : exec prog s = exec prog s
  /-- If the program is well-typed and the arena is capacity-bounded,
  the binary's result satisfies PMT safety (canonical traps only). -/
  safe : WellTyped prog → CapacityInvariant s.arena →
    match exec prog s with
    | Result.ok _ => True
    | Result.trap c => c = 1 ∨ c = 134 ∨ c = 135

/-- The Lean `exec` satisfies the `PipelineSpec` (Lean-side half of the
simulation; PMT-1-G will supply the Rust-side half).

This is the Lean-side half: Lean's own execution already meets the
specification that the Rust `pipeline::compile` *will eventually* claim
to meet. The `compiled_matches_exec` conjunct is `rfl` (degenerate — see
`PipelineSpec` docstring); the `safe` conjunct is the real content and
delegates to `pmt_soundness` (sorry-free). The Rust-side half — discharging
`PipelineSpec` for the actual `pipeline::compile` output — is the work of
Wave 1 PMT-1-G (extraction + parity testing). -/
theorem exec_satisfies_pipeline_spec
    (prog : Program) (s : ExecState)
    (hwf : WellTyped prog)
    (hstep : ∀ st : Step, st ∈ prog →
              WF_Layout st.layout ∧ s.live st.in_var = Liveness.live)
    (hcap : CapacityInvariant s.arena) :
    PipelineSpec prog s := by
  refine ⟨rfl, ?_⟩
  intro _ _
  -- Delegate to `pmt_soundness` (sorry-free).
  obtain ⟨r, hr, hvalid⟩ := pmt_soundness prog hwf s hstep hcap
  -- Rewrite the goal's `exec prog s` to the witness `r`.
  rw [hr]
  -- Case on `r` to reduce the match.
  cases r with
  | ok _ => trivial
  | trap _ => exact hvalid

/-- `pmt_soundness` restated in un-existentialled `match` form (was
`pipeline_compile_sound`).

This is a direct restatement of `pmt_soundness` (sorry-free): on success
the final bump pointer is within capacity; on trap the exit code is
canonical (1 = arena_overflow, 134 = oob, 135 = uaf). It contains **no
Rust-side hypothesis** — the pre-PMT-0-C version took a
`hconforms : PipelineSpec prog s` hypothesis that was unused in the
proof body; PMT-0-C removed it (see file header).

The "real" pipeline-conformance theorem — one that discharges a
non-vacuous `PipelineSpec prog s` hypothesis tying Lean's `exec` to the
Rust `pipeline::compile` output — is deferred to Wave 1 PMT-1-G
(extraction + Rust-parity testing). -/
theorem pmt_soundness_restate
    (prog : Program) (s : ExecState)
    (hwf : WellTyped prog)
    (hstep : ∀ st : Step, st ∈ prog →
              WF_Layout st.layout ∧ s.live st.in_var = Liveness.live)
    (hcap : CapacityInvariant s.arena) :
    -- Conclusion: on success the final bump pointer is within capacity;
    -- on trap the exit code is canonical (1 = arena_overflow, 134 = oob,
    -- 135 = uaf). This is exactly `pmt_soundness` in `match` form.
    match exec prog s with
    | Result.ok fu => fu ≤ s.arena.capacity
    | Result.trap c => c = 1 ∨ c = 134 ∨ c = 135 := by
  -- Delegate to `pmt_soundness` (sorry-free). Pre-PMT-0-C this theorem
  -- also took a `hconforms : PipelineSpec prog s` hypothesis that was
  -- unused here; PMT-0-C removed it. The non-vacuous pipeline-conformance
  -- theorem is Wave 1 PMT-1-G's job.
  obtain ⟨r, hr, hvalid⟩ := pmt_soundness prog hwf s hstep hcap
  rw [hr]
  exact hvalid

/-- `no_oob_trap_for_well_typed_strong` restated in `≠` form (was
`pipeline_compile_no_oob`).

Corollary: if the program is `WellTypedStrong`, then `exec prog s` never
traps with exit code 134 (out-of-bounds). This is a direct restatement of
`no_oob_trap_for_well_typed_strong` (sorry-free) and contains **no
Rust-side hypothesis** — the pre-PMT-0-C version took a
`hconforms : PipelineSpec prog s` hypothesis that was unused in the proof
body; PMT-0-C removed it (see file header).

This is the Lean-side guarantee that the runtime `__oob_trap` injection
(in `codegen/memory_safety.rs::inject_bounds_check_ir`) is redundant
defense-in-depth for `WellTypedStrong` programs. The conclusion
`exec prog s ≠ Result.trap 134` is propositionally equivalent to
`match exec prog s with | Result.trap 134 => False | _ => True`
(since the only `False`-producing pattern is `Result.trap 134`). The
`≠` form is used because it composes directly with the sorry-free
`no_oob_trap_for_well_typed_strong`.

The "real" pipeline-conformance corollary — one that conditions on a
non-vacuous `PipelineSpec prog s` — is deferred to Wave 1 PMT-1-G. -/
theorem pmt_soundness_no_oob_restate
    (prog : Program) (initial_var : String) (s : ExecState)
    (hwf : WellTypedStrong prog initial_var)
    (hstep : ∀ st : Step, st ∈ prog →
              WF_Layout st.layout ∧ s.live st.in_var = Liveness.live)
    (hcap : CapacityInvariant s.arena)
    (hinit : s.live initial_var = Liveness.live) :
    -- Conclusion: `exec prog s` never traps with exit code 134.
    -- This is exactly `no_oob_trap_for_well_typed_strong` in `≠` form.
    exec prog s ≠ Result.trap 134 := by
  -- Delegate to `no_oob_trap_for_well_typed_strong` (sorry-free).
  -- Pre-PMT-0-C this theorem also took a `hconforms : PipelineSpec prog s`
  -- hypothesis that was unused here; PMT-0-C removed it. The non-vacuous
  -- pipeline-conformance corollary is Wave 1 PMT-1-G's job.
  have h := no_oob_trap_for_well_typed_strong prog initial_var hwf s hstep hcap hinit
  exact h (exec prog s) rfl

end PMT
