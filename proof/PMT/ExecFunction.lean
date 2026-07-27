import PMT.IRProgram
import PMT.PmtInstr
import PMT.Soundness

/-!
## ExecFunction — flatten IRFunction to Program (sorry-free)

This module provides `IRFunction.to_program`, which flattens an `IRFunction`
(list of `IRBlock`s of `PmtInstr`s) into a `Program` (list of `Step`s) that
`exec` (from `PMT.Soundness`) can run.

The flattening is structural:
  - Each `IRBlock.instructions` is a list of `PmtInstr`.
  - Each `PmtInstr` is converted to one or more `Step`s.
  - The `IRTerminator` is currently ignored (no control flow — we flatten
    to a single straight-line program). A future refinement will add control flow.
    The 3 control-flow `PmtInstr` variants added in PMT-1-B (`phi`, `branch`,
    `cond_branch`) likewise flatten to `[]`: their semantics are resolved at
    the CFG level (`PmtInstr.successor_labels` in `PMT.IRProgram` §6.5), not
    as `Step`s. They are kept in the model so that `instr_sim` /
    `block_sim` traversal in `PMT.SimRel` can carry them through structurally.
  - The 3 atomic `PmtInstr` variants added in PMT-1-C (`atomic_load`,
    `atomic_store`, `atomic_cas`) likewise flatten to `[]`: under PMT's
    single-threaded soundness model, atomicity is vacuous (no other thread
    to race with), and the underlying load/store/CAS memory effect is
    modeled at the IVE / runtime layer, not as a PMT `Step`. The
    `AtomicOrdering` tag is preserved for forward-compatibility but never
    inspected by `to_steps`. A full concurrent-execution semantics is
    out of scope for PMT-1-C.
  - The 10 channel/special `PmtInstr` variants added in PMT-1-D
    (`vector_op`, `channel_open`, `channel_send`, `channel_recv`,
    `channel_close`, `channel_recv_timeout`, `channel_recv_result`,
    `stark_proof`, `call_indirect`, `syscall`) flatten to `[]` for 9
    of the 10 (their effects are either pure SIMD computation, out-of-band
    IVE capability effects, opaque proof verification, or out-of-scope
    syscalls — none of which interact with PMT's arena state). The 10th,
    `call_indirect`, flattens to `args.map (fun v => ⟨v, v, ⟨1, []⟩,
    .transform⟩)` — exactly mirroring `.call` (placeholder self-loop
    steps per argument vreg). The 9 `[]`-flattening variants contribute
    no `Step`s to `IRFunction.flat_steps` and therefore do not perturb
    the name-uniqueness conjuncts of `WellTyped`; the `call_indirect`
    variant contributes one placeholder `Step` per argument vreg
    (carrying the `⟨1, []⟩` layout, well-formed by `WF_Layout_empty`),
    and its name-uniqueness obligation is discharged by the existing
    `IRFunction.in_vars_unique` / `out_vars_unique` conjuncts in
    `IRFunction.well_typed`, exactly as for `.call`. The
    `pmt_soundness` argument — which operates over the flattened
    `Program = List Step` — is therefore preserved sorry-free.

All theorems in this file close without `sorry`, including the
previously-`sorry`-backed `to_program_preserves_well_typed_full` (§5.1),
which was closed sorry-free by strengthening
`IRFunction.well_typed` (in `PMT/IRProgram.lean` §10) with the
`in_vars_unique` / `out_vars_unique` conjuncts. See the §5.1 docstring
for the proof strategy.

**References.**
  * Related modules: `PMT.PmtInstr` (instruction type + `PmtInstr.to_steps`),
    `PMT.IRProgram` (program structure + `IRFunction.flat_steps` /
    `IRFunction.well_typed`), `PMT.Soundness` (`Step`, `Program`, `exec`,
    `WellTyped`), `PMT.SimRel` (uses `to_program` for the
    `full_simulation` / `full_simulation_strong` theorems),
    `PMT.WellTypedStrong` (uses `to_program_preserves_well_typed_full`
    + `PmtInstr.to_steps_op_transform` to lift `IRProgram.well_typed`
    all the way to `WellTypedStrong (p.to_program)`).
  * **PMT-1-E.** The previous `IRProgram.first_function_body` STUB in
    `PMT/SimRel.lean` (which always returned `[]`, closing
    `full_simulation` trivially via `exec [] _ = Result.ok _`) has been
    REMOVED. Both `full_simulation` and `full_simulation_strong` now
    invoke the real `IRProgram.to_program` flattening defined here.
    `PmtInstr.to_steps_op_transform` (§1.9 below) is added to support
    the `FieldAccessOk` half of the `WellTypedStrong` lift.

**Build.** This module is part of the Lake package rooted at
`proof/lakefile.toml`. Build with `lake build` (or `make proof` /
`just proof`); the `lean-proofs` CI job in
`.github/workflows/proof-verify.yml` runs the same command.
-/

universe u v

namespace PMT

/-! ## §1. Per-instruction flattening: `PmtInstr.to_steps`

`PmtInstr.to_steps` itself is defined in `PMT/PmtInstr.lean` (§14),
not here, so that `IRFunction.flat_steps` and `IRFunction.well_typed`
(in `PMT/IRProgram.lean`) can refer to it without a circular import
(`ExecFunction.lean` imports `IRProgram.lean`). The mapping rationale
is documented at the definition site. This section keeps the trivial
"reduces to a singleton"/"reduces to empty" reflection lemmas and the
non-trivial `PmtInstr.to_steps_preserves_WF_Layout` preservation lemma
here in `ExecFunction.lean`, since they are used by
`IRProgram.to_program_preserves_well_typed` below. -/

/-- §1.1: `ret` flattens to the empty list of steps. -/
theorem PmtInstr.to_steps_ret (v : IRValue) :
    PmtInstr.to_steps (.ret v) = [] := by
  rfl

/-- §1.2: `alloc` flattens to a singleton list. -/
theorem PmtInstr.to_steps_alloc (out : String) (layout : Layout) :
    PmtInstr.to_steps (.alloc out layout) = [⟨out, out, layout, .transform⟩] := by
  rfl

/-- §1.3: `load` flattens to a singleton list with the placeholder layout. -/
theorem PmtInstr.to_steps_load (in_var out : String) (offset : Nat) (ty : IRType) :
    PmtInstr.to_steps (.load in_var out offset ty)
      = [⟨in_var, out, ⟨1, []⟩, .transform⟩] := by
  rfl

/-- §1.4: `store` flattens to a singleton list with the placeholder layout. -/
theorem PmtInstr.to_steps_store (in_var : String) (val : IRValue) (offset : Nat)
    (ty : IRType) :
    PmtInstr.to_steps (.store in_var val offset ty)
      = [⟨in_var, in_var, ⟨1, []⟩, .transform⟩] := by
  rfl

/-- §1.5: `free` flattens to a singleton list with the placeholder layout. -/
theorem PmtInstr.to_steps_free (in_var : String) :
    PmtInstr.to_steps (.free in_var)
      = [⟨in_var, in_var, ⟨1, []⟩, .transform⟩] := by
  rfl

/-- §1.6: `transform` flattens to a singleton list with the source layout. -/
theorem PmtInstr.to_steps_transform (in_var out : String) (layout : Layout) :
    PmtInstr.to_steps (.transform in_var out layout)
      = [⟨in_var, out, layout, .transform⟩] := by
  rfl

/-- §1.7: `call` flattens to one self-loop step per argument. -/
theorem PmtInstr.to_steps_call (fn : String) (args : List String) :
    PmtInstr.to_steps (.call fn args)
      = args.map (fun v => ⟨v, v, ⟨1, []⟩, .transform⟩) := by
  rfl

/-! ## §1.7a. Reflection lemmas for the 12 pure-arithmetic variants (PMT-1-A)

Each arithmetic `PmtInstr` variant flattens to the empty `List Step` —
they are pure register-to-register computations with no memory effect.
The 12 lemmas below are each provable by `rfl` (the `PmtInstr.to_steps`
definition maps each arithmetic constructor to `[]` literally). They
feed the `cases i with` block of `PmtInstr.to_steps_preserves_WF_Layout`
(§1.8) so that the per-instruction `WF_Layout` preservation proof
remains exhaustive over the enlarged `PmtInstr` inductive (19
constructors: 7 memory + 12 arithmetic). -/

/-- §1.7a.1: `bin_op` flattens to `[]`. -/
theorem PmtInstr.to_steps_bin_op (op : BinOpKind) (dst lhs rhs : IRValue)
    (ty : IRType) :
    PmtInstr.to_steps (.bin_op op dst lhs rhs ty) = [] := by
  rfl

/-- §1.7a.2: `unary_op` flattens to `[]`. -/
theorem PmtInstr.to_steps_unary_op (op : UnaryOpKind) (dst src : IRValue)
    (ty : IRType) :
    PmtInstr.to_steps (.unary_op op dst src ty) = [] := by
  rfl

/-- §1.7a.3: `cast` flattens to `[]`. -/
theorem PmtInstr.to_steps_cast (k : CastKind) (dst src : IRValue)
    (from_ty to_ty : IRType) :
    PmtInstr.to_steps (.cast k dst src from_ty to_ty) = [] := by
  rfl

/-- §1.7a.4: `add` flattens to `[]`. -/
theorem PmtInstr.to_steps_add (dst lhs rhs : IRValue) (ty : IRType) :
    PmtInstr.to_steps (.add dst lhs rhs ty) = [] := by
  rfl

/-- §1.7a.5: `sub` flattens to `[]`. -/
theorem PmtInstr.to_steps_sub (dst lhs rhs : IRValue) (ty : IRType) :
    PmtInstr.to_steps (.sub dst lhs rhs ty) = [] := by
  rfl

/-- §1.7a.6: `mul` flattens to `[]`. -/
theorem PmtInstr.to_steps_mul (dst lhs rhs : IRValue) (ty : IRType) :
    PmtInstr.to_steps (.mul dst lhs rhs ty) = [] := by
  rfl

/-- §1.7a.7: `div` flattens to `[]`. -/
theorem PmtInstr.to_steps_div (dst lhs rhs : IRValue) (ty : IRType) :
    PmtInstr.to_steps (.div dst lhs rhs ty) = [] := by
  rfl

/-- §1.7a.8: `cmp` flattens to `[]`. -/
theorem PmtInstr.to_steps_cmp (k : CmpKind) (dst lhs rhs : IRValue)
    (ty : IRType) :
    PmtInstr.to_steps (.cmp k dst lhs rhs ty) = [] := by
  rfl

/-- §1.7a.9: `select` flattens to `[]`. -/
theorem PmtInstr.to_steps_select (dst cond then_val else_val : IRValue)
    (ty : IRType) :
    PmtInstr.to_steps (.select dst cond then_val else_val ty) = [] := by
  rfl

/-- §1.7a.10: `ct_select` flattens to `[]`. -/
theorem PmtInstr.to_steps_ct_select (dst cond then_val else_val : IRValue)
    (ty : IRType) :
    PmtInstr.to_steps (.ct_select dst cond then_val else_val ty) = [] := by
  rfl

/-- §1.7a.11: `ct_eq` flattens to `[]`. -/
theorem PmtInstr.to_steps_ct_eq (dst lhs rhs : IRValue) (ty : IRType) :
    PmtInstr.to_steps (.ct_eq dst lhs rhs ty) = [] := by
  rfl

/-- §1.7a.12: `get_address` flattens to `[]`. -/
theorem PmtInstr.to_steps_get_address (dst : IRValue) (name : String) :
    PmtInstr.to_steps (.get_address dst name) = [] := by
  rfl

/-! ## §1.7b. Reflection lemmas for the 3 control-flow variants (PMT-1-B)

Each control-flow `PmtInstr` variant flattens to the empty `List Step` —
their semantics are resolved at the CFG level (`PmtInstr.successor_labels`,
`IRBlock.successors_from_instrs`) rather than as `Step`s. The 3 lemmas
below are each provable by `rfl` (the `PmtInstr.to_steps` definition
maps each control-flow constructor to `[]` literally). They feed the
`cases i with` block of `PmtInstr.to_steps_preserves_WF_Layout` (§1.8)
so that the per-instruction `WF_Layout` preservation proof remains
exhaustive over the enlarged `PmtInstr` inductive (22 constructors:
7 memory + 12 arithmetic + 3 control-flow). -/

/-- §1.7b.1: `phi` flattens to `[]`. -/
theorem PmtInstr.to_steps_phi (dst : IRValue) (incoming : List (IRValue × String)) :
    PmtInstr.to_steps (.phi dst incoming) = [] := by
  rfl

/-- §1.7b.2: `branch` flattens to `[]`. -/
theorem PmtInstr.to_steps_branch (target : String) :
    PmtInstr.to_steps (.branch target) = [] := by
  rfl

/-- §1.7b.3: `cond_branch` flattens to `[]`. -/
theorem PmtInstr.to_steps_cond_branch (cond : IRValue)
    (true_target false_target : String) :
    PmtInstr.to_steps (.cond_branch cond true_target false_target) = [] := by
  rfl

/-! ## §1.7c. Reflection lemmas for the 3 atomic variants (PMT-1-C)

Each atomic `PmtInstr` variant flattens to the empty `List Step` — under
PMT's single-threaded soundness model, atomicity is vacuous (there is no
other thread to race with), and the underlying load/store/CAS memory
effect is modeled at the IVE / runtime layer, not as a PMT `Step`. The
3 lemmas below are each provable by `rfl` (the `PmtInstr.to_steps`
definition maps each atomic constructor to `[]` literally, regardless
of the `AtomicOrdering` tag value). They feed the `cases i with` block
of `PmtInstr.to_steps_preserves_WF_Layout` (§1.8) so that the
per-instruction `WF_Layout` preservation proof remains exhaustive
over the enlarged `PmtInstr` inductive (25 constructors: 7 memory +
12 arithmetic + 3 control-flow + 3 atomic). -/

/-- §1.7c.1: `atomic_load` flattens to `[]` (single-threaded: atomicity
vacuous; memory effect modeled at the IVE / runtime layer). -/
theorem PmtInstr.to_steps_atomic_load (dst addr : IRValue) (ty : IRType)
    (ordering : AtomicOrdering) :
    PmtInstr.to_steps (.atomic_load dst addr ty ordering) = [] := by
  rfl

/-- §1.7c.2: `atomic_store` flattens to `[]` (single-threaded: atomicity
vacuous; memory effect modeled at the IVE / runtime layer). -/
theorem PmtInstr.to_steps_atomic_store (value addr : IRValue) (ty : IRType)
    (ordering : AtomicOrdering) :
    PmtInstr.to_steps (.atomic_store value addr ty ordering) = [] := by
  rfl

/-- §1.7c.3: `atomic_cas` flattens to `[]` (single-threaded: atomicity
vacuous; memory effect modeled at the IVE / runtime layer). -/
theorem PmtInstr.to_steps_atomic_cas (dst addr expected desired : IRValue)
    (ty : IRType) (success_order failure_order : AtomicOrdering) :
    PmtInstr.to_steps
      (.atomic_cas dst addr expected desired ty success_order failure_order) = [] := by
  rfl

/-! ## §1.7d. Reflection lemmas for the 10 channel/special variants (PMT-1-D)

Each of the 10 channel/special `PmtInstr` variants flattens as follows:
9 of the 10 (`vector_op`, `channel_open`, `channel_send`, `channel_recv`,
`channel_close`, `channel_recv_timeout`, `channel_recv_result`,
`stark_proof`, `syscall`) flatten to the empty `List Step` — their effects
are either pure SIMD computation, out-of-band IVE capability effects,
opaque proof verification, or out-of-scope syscalls, none of which
interact with PMT's arena state. The 10th, `call_indirect`, flattens to
`args.map (fun v => ⟨v, v, ⟨1, []⟩, .transform⟩)` — exactly mirroring
`.call` (placeholder self-loop steps per argument vreg).

The 10 lemmas below are each provable by `rfl` (the `PmtInstr.to_steps`
definition maps each channel/special constructor literally). They feed
the `cases i with` block of `PmtInstr.to_steps_preserves_WF_Layout`
(§1.8) so that the per-instruction `WF_Layout` preservation proof
remains exhaustive over the enlarged `PmtInstr` inductive (35
constructors: 7 memory + 12 arithmetic + 3 control-flow + 3 atomic +
10 channel/special). -/

/-- §1.7d.1: `vector_op` flattens to `[]` (pure SIMD computation; the
underlying packed arithmetic is modeled at the IVE-from-IR layer). -/
theorem PmtInstr.to_steps_vector_op (op : VectorOpKind) (lanes elem_size : Nat)
    (dst lhs rhs : IRValue) :
    PmtInstr.to_steps (.vector_op op lanes elem_size dst lhs rhs) = [] := by
  rfl

/-- §1.7d.2: `channel_open` flattens to `[]` (out-of-band capability
effect; channel state is modeled by IVE's capability system, not by
PMT's arena state). -/
theorem PmtInstr.to_steps_channel_open (dst : IRValue) (elem_ty : IRType) :
    PmtInstr.to_steps (.channel_open dst elem_ty) = [] := by
  rfl

/-- §1.7d.3: `channel_send` flattens to `[]` (out-of-band capability
effect). -/
theorem PmtInstr.to_steps_channel_send (ch msg : IRValue) (ty : Option IRType) :
    PmtInstr.to_steps (.channel_send ch msg ty) = [] := by
  rfl

/-- §1.7d.4: `channel_recv` flattens to `[]` (out-of-band capability
effect). -/
theorem PmtInstr.to_steps_channel_recv (ch dst : IRValue) (ty : Option IRType) :
    PmtInstr.to_steps (.channel_recv ch dst ty) = [] := by
  rfl

/-- §1.7d.5: `channel_close` flattens to `[]` (out-of-band capability
effect). -/
theorem PmtInstr.to_steps_channel_close (ch : IRValue) :
    PmtInstr.to_steps (.channel_close ch) = [] := by
  rfl

/-- §1.7d.6: `channel_recv_timeout` flattens to `[]` (out-of-band
capability effect; the timeout is a runtime scheduling concern, not a
PMT `Step`). -/
theorem PmtInstr.to_steps_channel_recv_timeout (ch dst : IRValue)
    (ty : Option IRType) (timeout_ms : Nat) :
    PmtInstr.to_steps (.channel_recv_timeout ch dst ty timeout_ms) = [] := by
  rfl

/-- §1.7d.7: `channel_recv_result` flattens to `[]` (out-of-band
capability effect; both the value-result and error-code-result are
runtime channel effects, not PMT `Step`s). -/
theorem PmtInstr.to_steps_channel_recv_result (ch dst err_dst : IRValue)
    (ty : Option IRType) :
    PmtInstr.to_steps (.channel_recv_result ch dst err_dst ty) = [] := by
  rfl

/-- §1.7d.8: `stark_proof` flattens to `[]` (proof verification is an
opaque effect delegated to the verifier; the proof buffer is allocated
and tracked at the IPC / runtime layer, not as a PMT arena region). -/
theorem PmtInstr.to_steps_stark_proof (input dst : IRValue)
    (constraints : List Nat) :
    PmtInstr.to_steps (.stark_proof input dst constraints) = [] := by
  rfl

/-- §1.7d.9: `call_indirect` flattens to one self-loop step per argument
vreg — exactly mirroring `.call` (placeholder self-loops until call
semantics are modeled). -/
theorem PmtInstr.to_steps_call_indirect (func_ptr : String) (args : List String) :
    PmtInstr.to_steps (.call_indirect func_ptr args)
      = args.map (fun v => ⟨v, v, ⟨1, []⟩, .transform⟩) := by
  rfl

/-- §1.7d.10: `syscall` flattens to `[]` (opaque effect; syscalls are
out-of-scope for PMT — no arena state interaction; the syscall ABI is a
runtime concern). -/
theorem PmtInstr.to_steps_syscall (nr : Nat) (args : List IRValue)
    (dst : Option IRValue) :
    PmtInstr.to_steps (.syscall nr args dst) = [] := by
  rfl

/-! ## §1.7e: Bulk-memory variants (FFI-3-A) — `bulk_copy` / `bulk_fill`

The 2 bulk-memory `PmtInstr` variants added by FFI-3-A (mirroring the
Rust `IRInstr::BulkCopy` and `IRInstr::BulkFill` from FFI-1-A) both
flatten to the empty `List Step`. They are opaque memory writes —
memcpy/memset replacements that write to a raw pointer region
`[dst, dst+len)`, not to a PMT arena region. Their underlying byte-level
semantics is a runtime concern (x86_64 `CLD; REP MOVSB` for `bulk_copy`,
`CLD; REP STOSB` for `bulk_fill`), not a PMT `Step` interaction.

The 2 lemmas below are each provable by `rfl` (the `PmtInstr.to_steps`
definition maps each bulk-memory constructor literally to `[]`). They
feed the `cases i with` block of `PmtInstr.to_steps_preserves_WF_Layout`
(§1.8) and `PmtInstr.to_steps_op_transform` (§1.9) so those proofs
remain exhaustive over the enlarged `PmtInstr` inductive (37
constructors: 7 memory + 12 arithmetic + 3 control-flow + 3 atomic +
10 channel/special + 2 bulk-memory). -/

/-- §1.7e.1: `bulk_copy` flattens to `[]` (opaque memory write — memcpy
replacement; the underlying `REP MOVSB` byte copy is a runtime concern,
not a PMT arena `Step`). Mirrors Rust `IRInstr::BulkCopy { dst, src,
len }` field-for-field. -/
theorem PmtInstr.to_steps_bulk_copy (dst src len : IRValue) :
    PmtInstr.to_steps (.bulk_copy dst src len) = [] := by
  rfl

/-- §1.7e.2: `bulk_fill` flattens to `[]` (opaque memory write — memset
replacement; the underlying `REP STOSB` byte fill is a runtime concern,
not a PMT arena `Step`). Mirrors Rust `IRInstr::BulkFill { dst, val,
len }` field-for-field. -/
theorem PmtInstr.to_steps_bulk_fill (dst val len : IRValue) :
    PmtInstr.to_steps (.bulk_fill dst val len) = [] := by
  rfl

/-- §1.8: Every `Step` produced by `PmtInstr.to_steps` carries a
`WF_Layout` when the instruction is well-typed under `env`.

This is the per-instruction preservation lemma used to lift
`WF_Layout` through `IRBlock.to_steps` and `IRFunction.to_program`
(see `IRProgram.to_program_preserves_well_typed` below). The proof
case-splits on the instruction constructor; the `alloc`/`transform`
cases inherit `WF_Layout` directly from the per-instruction
`well_typed` hypothesis, and the `load`/`store`/`free`/`call` cases
use the placeholder layout `⟨1, []⟩ = emptyLayout`, whose
well-formedness is `WF_Layout_empty` (`PMT.Basic`). -/
theorem PmtInstr.to_steps_preserves_WF_Layout
    (i : PmtInstr) (env : String → Layout)
    (hi : i.well_typed env) :
    ∀ s : Step, s ∈ i.to_steps → WF_Layout s.layout := by
  intro s hs
  cases i with
  | alloc out layout =>
    rw [PmtInstr.to_steps_alloc, List.mem_singleton] at hs
    subst hs
    exact hi
  | load in_var out offset ty =>
    rw [PmtInstr.to_steps_load, List.mem_singleton] at hs
    subst hs
    exact WF_Layout_empty
  | store in_var val offset ty =>
    rw [PmtInstr.to_steps_store, List.mem_singleton] at hs
    subst hs
    exact WF_Layout_empty
  | free in_var =>
    rw [PmtInstr.to_steps_free, List.mem_singleton] at hs
    subst hs
    exact WF_Layout_empty
  | transform in_var out layout =>
    have hi' : WF_Layout layout ∧ WF_Layout (env in_var) := hi
    obtain ⟨hwt_layout, _⟩ := hi'
    rw [PmtInstr.to_steps_transform, List.mem_singleton] at hs
    subst hs
    exact hwt_layout
  | call fn args =>
    rw [PmtInstr.to_steps_call, List.mem_map] at hs
    obtain ⟨v, _, rfl⟩ := hs
    exact WF_Layout_empty
  | ret val =>
    rw [PmtInstr.to_steps_ret] at hs
    simp at hs
  -- 12 pure-arithmetic variants (PMT-1-A): each flattens to `[]`, so the
  -- membership hypothesis `hs : s ∈ []` is vacuous. The per-instruction
  -- `well_typed` predicate is `True` for all arithmetic variants, so
  -- `hi` is `trivial` and not consulted.
  | bin_op op dst lhs rhs ty =>
    rw [PmtInstr.to_steps_bin_op] at hs
    simp at hs
  | unary_op op dst src ty =>
    rw [PmtInstr.to_steps_unary_op] at hs
    simp at hs
  | cast k dst src from_ty to_ty =>
    rw [PmtInstr.to_steps_cast] at hs
    simp at hs
  | add dst lhs rhs ty =>
    rw [PmtInstr.to_steps_add] at hs
    simp at hs
  | sub dst lhs rhs ty =>
    rw [PmtInstr.to_steps_sub] at hs
    simp at hs
  | mul dst lhs rhs ty =>
    rw [PmtInstr.to_steps_mul] at hs
    simp at hs
  | div dst lhs rhs ty =>
    rw [PmtInstr.to_steps_div] at hs
    simp at hs
  | cmp k dst lhs rhs ty =>
    rw [PmtInstr.to_steps_cmp] at hs
    simp at hs
  | select dst cond then_val else_val ty =>
    rw [PmtInstr.to_steps_select] at hs
    simp at hs
  | ct_select dst cond then_val else_val ty =>
    rw [PmtInstr.to_steps_ct_select] at hs
    simp at hs
  | ct_eq dst lhs rhs ty =>
    rw [PmtInstr.to_steps_ct_eq] at hs
    simp at hs
  | get_address dst name =>
    rw [PmtInstr.to_steps_get_address] at hs
    simp at hs
  -- 3 control-flow variants (PMT-1-B): each flattens to `[]`, so the
  -- membership hypothesis `hs : s ∈ []` is vacuous. The per-instruction
  -- `well_typed` predicate is `True` for all control-flow variants, so
  -- `hi` is `trivial` and not consulted. Control-flow semantics are
  -- resolved at the CFG level (`PmtInstr.successor_labels`, §6.5 of
  -- `PMT.IRProgram`), not as `Step`s.
  | phi dst incoming =>
    rw [PmtInstr.to_steps_phi] at hs
    simp at hs
  | branch target =>
    rw [PmtInstr.to_steps_branch] at hs
    simp at hs
  | cond_branch cond true_target false_target =>
    rw [PmtInstr.to_steps_cond_branch] at hs
    simp at hs
  -- 3 atomic variants (PMT-1-C): each flattens to `[]`, so the
  -- membership hypothesis `hs : s ∈ []` is vacuous. Under PMT's
  -- single-threaded semantics the atomicity is vacuous (no other thread
  -- to race with), and the underlying load/store/CAS memory effect is
  -- modeled at the IVE / runtime layer — not as a PMT `Step`. The
  -- per-instruction `well_typed` predicate is `True` for all atomic
  -- variants, so `hi` is `trivial` and not consulted. The
  -- `AtomicOrdering` tag is not inspected.
  | atomic_load dst addr ty ordering =>
    rw [PmtInstr.to_steps_atomic_load] at hs
    simp at hs
  | atomic_store value addr ty ordering =>
    rw [PmtInstr.to_steps_atomic_store] at hs
    simp at hs
  | atomic_cas dst addr expected desired ty success_order failure_order =>
    rw [PmtInstr.to_steps_atomic_cas] at hs
    simp at hs
  -- 10 channel/special variants (PMT-1-D):
  --  * 9 of the 10 (`vector_op`, `channel_*`, `stark_proof`, `syscall`)
  --    flatten to `[]`, so the membership hypothesis `hs : s ∈ []` is
  --    vacuous. The per-instruction `well_typed` predicate is `True` for
  --    all 9 variants, so `hi` is `trivial` and not consulted. Their
  --    effects are pure SIMD computation, out-of-band IVE capability
  --    effects, opaque proof verification, or out-of-scope syscalls —
  --    none of which interact with PMT's arena state.
  --  * `call_indirect` mirrors `.call`: `to_steps` produces
  --    `args.map (fun v => ⟨v, v, ⟨1, []⟩, .transform⟩)`, each element
  --    of which carries the well-formed placeholder layout `⟨1, []⟩`
  --    (by `WF_Layout_empty`).
  | vector_op op lanes elem_size dst lhs rhs =>
    rw [PmtInstr.to_steps_vector_op] at hs
    simp at hs
  | channel_open dst elem_ty =>
    rw [PmtInstr.to_steps_channel_open] at hs
    simp at hs
  | channel_send ch msg ty =>
    rw [PmtInstr.to_steps_channel_send] at hs
    simp at hs
  | channel_recv ch dst ty =>
    rw [PmtInstr.to_steps_channel_recv] at hs
    simp at hs
  | channel_close ch =>
    rw [PmtInstr.to_steps_channel_close] at hs
    simp at hs
  | channel_recv_timeout ch dst ty timeout_ms =>
    rw [PmtInstr.to_steps_channel_recv_timeout] at hs
    simp at hs
  | channel_recv_result ch dst err_dst ty =>
    rw [PmtInstr.to_steps_channel_recv_result] at hs
    simp at hs
  | stark_proof input dst constraints =>
    rw [PmtInstr.to_steps_stark_proof] at hs
    simp at hs
  | call_indirect func_ptr args =>
    rw [PmtInstr.to_steps_call_indirect, List.mem_map] at hs
    obtain ⟨v, _, rfl⟩ := hs
    exact WF_Layout_empty
  | syscall nr args dst =>
    rw [PmtInstr.to_steps_syscall] at hs
    simp at hs
  -- 2 bulk-memory variants (FFI-3-A): each flattens to `[]`, so the
  -- membership hypothesis `hs : s ∈ []` is vacuous. The per-instruction
  -- `well_typed` predicate is `True` for both variants, so `hi` is
  -- `trivial` and not consulted. Their effects are opaque memory writes
  -- (memcpy/memset replacements) — they do not interact with PMT's
  -- arena state.
  | bulk_copy dst src len =>
    rw [PmtInstr.to_steps_bulk_copy] at hs
    simp at hs
  | bulk_fill dst val len =>
    rw [PmtInstr.to_steps_bulk_fill] at hs
    simp at hs

/-! ## §1.9: Every `Step` produced by `PmtInstr.to_steps` carries `op = .transform` (PMT-1-E)

This is the per-instruction `op`-tag lemma used to lift `FieldAccessOk`
(the third conjunct of `WellTypedStrong`) through `IRBlock.to_steps` and
`IRFunction.to_program`. The proof mirrors `to_steps_preserves_WF_Layout`
(§1.8): case-split on the 35 `PmtInstr` constructors; for each
non-`[]`-flattening variant (`alloc`, `load`, `store`, `free`,
`transform`, `call`, `call_indirect`) the produced `Step`s all carry
`op = .transform` literally (by the corresponding reflection lemma); for
each `[]`-flattening variant the membership hypothesis `s ∈ []` is
vacuous.

`FieldAccessOk` (defined in `PMT.WellTypedStrong`) requires that every
`Step` whose `op = .field_access f` has `f ∈ s.layout.fields`. Since
`to_steps` NEVER produces a `.field_access` op, `FieldAccessOk` holds
trivially on any flattened `IRProgram.to_program`. This closes the
"field-access safety" half of the `WellTypedStrong` lift from
`IRProgram.well_typed` — see `IRProgram.well_typed.to_program_well_typed_strong`
in `PMT/WellTypedStrong.lean`. -/

theorem PmtInstr.to_steps_op_transform
    (i : PmtInstr) (s : Step) (hs : s ∈ i.to_steps) :
    s.op = PmtOp.transform := by
  cases i with
  | alloc out layout =>
    rw [PmtInstr.to_steps_alloc, List.mem_singleton] at hs
    subst hs; rfl
  | load in_var out offset ty =>
    rw [PmtInstr.to_steps_load, List.mem_singleton] at hs
    subst hs; rfl
  | store in_var val offset ty =>
    rw [PmtInstr.to_steps_store, List.mem_singleton] at hs
    subst hs; rfl
  | free in_var =>
    rw [PmtInstr.to_steps_free, List.mem_singleton] at hs
    subst hs; rfl
  | transform in_var out layout =>
    rw [PmtInstr.to_steps_transform, List.mem_singleton] at hs
    subst hs; rfl
  | call fn args =>
    rw [PmtInstr.to_steps_call, List.mem_map] at hs
    obtain ⟨v, _, rfl⟩ := hs
    rfl
  | ret val =>
    rw [PmtInstr.to_steps_ret] at hs
    simp at hs
  | bin_op op dst lhs rhs ty =>
    rw [PmtInstr.to_steps_bin_op] at hs
    simp at hs
  | unary_op op dst src ty =>
    rw [PmtInstr.to_steps_unary_op] at hs
    simp at hs
  | cast k dst src from_ty to_ty =>
    rw [PmtInstr.to_steps_cast] at hs
    simp at hs
  | add dst lhs rhs ty =>
    rw [PmtInstr.to_steps_add] at hs
    simp at hs
  | sub dst lhs rhs ty =>
    rw [PmtInstr.to_steps_sub] at hs
    simp at hs
  | mul dst lhs rhs ty =>
    rw [PmtInstr.to_steps_mul] at hs
    simp at hs
  | div dst lhs rhs ty =>
    rw [PmtInstr.to_steps_div] at hs
    simp at hs
  | cmp k dst lhs rhs ty =>
    rw [PmtInstr.to_steps_cmp] at hs
    simp at hs
  | select dst cond then_val else_val ty =>
    rw [PmtInstr.to_steps_select] at hs
    simp at hs
  | ct_select dst cond then_val else_val ty =>
    rw [PmtInstr.to_steps_ct_select] at hs
    simp at hs
  | ct_eq dst lhs rhs ty =>
    rw [PmtInstr.to_steps_ct_eq] at hs
    simp at hs
  | get_address dst name =>
    rw [PmtInstr.to_steps_get_address] at hs
    simp at hs
  | phi dst incoming =>
    rw [PmtInstr.to_steps_phi] at hs
    simp at hs
  | branch target =>
    rw [PmtInstr.to_steps_branch] at hs
    simp at hs
  | cond_branch cond true_target false_target =>
    rw [PmtInstr.to_steps_cond_branch] at hs
    simp at hs
  | atomic_load dst addr ty ordering =>
    rw [PmtInstr.to_steps_atomic_load] at hs
    simp at hs
  | atomic_store value addr ty ordering =>
    rw [PmtInstr.to_steps_atomic_store] at hs
    simp at hs
  | atomic_cas dst addr expected desired ty success_order failure_order =>
    rw [PmtInstr.to_steps_atomic_cas] at hs
    simp at hs
  | vector_op op lanes elem_size dst lhs rhs =>
    rw [PmtInstr.to_steps_vector_op] at hs
    simp at hs
  | channel_open dst elem_ty =>
    rw [PmtInstr.to_steps_channel_open] at hs
    simp at hs
  | channel_send ch msg ty =>
    rw [PmtInstr.to_steps_channel_send] at hs
    simp at hs
  | channel_recv ch dst ty =>
    rw [PmtInstr.to_steps_channel_recv] at hs
    simp at hs
  | channel_close ch =>
    rw [PmtInstr.to_steps_channel_close] at hs
    simp at hs
  | channel_recv_timeout ch dst ty timeout_ms =>
    rw [PmtInstr.to_steps_channel_recv_timeout] at hs
    simp at hs
  | channel_recv_result ch dst err_dst ty =>
    rw [PmtInstr.to_steps_channel_recv_result] at hs
    simp at hs
  | stark_proof input dst constraints =>
    rw [PmtInstr.to_steps_stark_proof] at hs
    simp at hs
  | call_indirect func_ptr args =>
    rw [PmtInstr.to_steps_call_indirect, List.mem_map] at hs
    obtain ⟨v, _, rfl⟩ := hs
    rfl
  | syscall nr args dst =>
    rw [PmtInstr.to_steps_syscall] at hs
    simp at hs
  -- 2 bulk-memory variants (FFI-3-A): each flattens to `[]`, so the
  -- membership hypothesis `hs : s ∈ []` is vacuous.
  | bulk_copy dst src len =>
    rw [PmtInstr.to_steps_bulk_copy] at hs
    simp at hs
  | bulk_fill dst val len =>
    rw [PmtInstr.to_steps_bulk_fill] at hs
    simp at hs

/-! ## §2. Per-block flattening: `IRBlock.to_steps` -/

/-- §2: Flatten an `IRBlock`'s `instructions` to a list of `Step`s.

The `terminator`, `predecessors`, `successors`, and `label` are ignored
in this straight-line flattening. A future refinement will add control-flow-aware
flattening that respects the terminator (jump/branch/ret). -/
def IRBlock.to_steps (b : IRBlock) : List Step :=
  b.instructions.flatMap PmtInstr.to_steps

/-- §2.1: An empty block flattens to `[]`. -/
theorem IRBlock.to_steps_empty (b : IRBlock) (h : b.instructions = []) :
    b.to_steps = [] := by
  rw [IRBlock.to_steps, h, List.flatMap_nil]

/-! ## §3. Per-function flattening: `IRFunction.to_program` -/

/-- §3: Flatten an `IRFunction`'s `blocks` to a `Program`.

Note: this ignores control flow (terminators, branches). It produces a
straight-line program by concatenating all blocks in order. A future
refinement will replace this with control-flow-aware flattening. -/
def IRFunction.to_program (f : IRFunction) : Program :=
  f.blocks.flatMap IRBlock.to_steps

/-- §3.1: A function with no blocks flattens to `[]`.
(Note: such a function violates `IRFunction.well_typed`, which requires
`f.blocks ≠ []`, but the flattening itself is still well-defined.) -/
theorem IRFunction.to_program_empty_blocks (f : IRFunction)
    (h : f.blocks = []) : f.to_program = [] := by
  simp only [IRFunction.to_program, h, List.flatMap_nil]

/-! ## §4. Top-level: `IRProgram.to_program` -/

/-- §4: Flatten an `IRProgram`'s first function to a `Program`.

Returns `[]` if the program has no functions. (Such a program is
degenerate — it has no entry point — but the flattening is total.) -/
def IRProgram.to_program (p : IRProgram) : Program :=
  match p.functions with
  | [] => []
  | f :: _ => f.to_program

/-- §4.1: `to_program` of an empty `IRProgram` (no functions) is `[]`. -/
theorem IRProgram.to_program_empty :
    IRProgram.to_program ⟨[], []⟩ = [] := by
  rfl

/-- §4.2: `to_program` of an `IRProgram` whose first function has no
blocks is `[]`. -/
theorem IRProgram.to_program_empty_first_function
    (f : IRFunction) (rest : List IRFunction) (h : f.blocks = []) :
    IRProgram.to_program ⟨f :: rest, []⟩ = [] := by
  simp only [IRProgram.to_program, IRFunction.to_program, h, List.flatMap_nil]

/-! ## §5. Well-typedness preservation

The full preservation theorem — "if `p.well_typed env` then
`(p.to_program)` is `WellTyped`" — requires aligning the `Layout`
assigned by `env` with the `Layout` embedded in each `PmtInstr` (e.g.
`alloc out layout` uses `layout`, but `load`/`store`/`free` use a
placeholder `⟨1, []⟩`). A future refinement will refine `to_steps` to consult `env`
for the load/store/free layouts, at which point the full lemma below
will be provable.

**The previous §5 was a hard `True` stub —
vacuously satisfied, carrying no logical content, and identified by the
proof-complexity audit as the *bottleneck* blocking the strengthening of
`full_simulation` and `full_simulation_strong` (SimRel.lean §"Stub
helper"). We replace the vacuous `True` with two real, non-trivially
stated theorems:

  * `to_program_preserves_well_typed` — the **layout-well-formedness
    half** of `WellTyped` (the first conjunct:
    `∀ s ∈ prog, WF_Layout s.layout`). This is the weakest non-vacuous
    preservation property. **Closed sorry-free.**
  * `to_program_preserves_well_typed_full` — the **full** `WellTyped`
    predicate of `PMT.Soundness`, including the two name-uniqueness
    conjuncts. This is the theorem the simulation relation
    will ultimately invoke. **Closed sorry-free**
    by strengthening `IRFunction.well_typed` with the
    `in_vars_unique` / `out_vars_unique` conjuncts (see
    `PMT/IRProgram.lean` §10 and the gap report).

The §5.1 docstring below documents the proof strategy in detail.
-/

/-- §5: `to_program` preserves the layout-well-formedness half of
`WellTyped`. Every `Step` in the flattened `Program` carries a
`WF_Layout` layout.

This is the **weakest non-trivial** half of the full preservation
theorem: the first conjunct of `WellTyped` (see `PMT.Soundness`).
The other two conjuncts (name uniqueness for `in_var` / `out_var`) are
stated in `to_program_preserves_well_typed_full` below.

**Proof obligation (TODO).** For each `PmtInstr` constructor, show
`PmtInstr.to_steps i` only produces `Step`s whose `.layout` is
`WF_Layout` under the per-instruction `well_typed` hypothesis coming
from `IRBlock.well_typed` → `IRFunction.well_typed` →
`IRProgram.well_typed`. The `alloc` and `transform` cases inherit
`WF_Layout` directly (their `layout` field is the source). The
`load`/`store`/`free`/`call` cases use placeholder layouts `⟨1, []⟩`
that require either (a) showing `WF_Layout ⟨1, []⟩` directly (which
holds — see `PMT.Basic.WF_Layout_empty` and the `0 < total_size ∨
fields = []` disjunct), or (b) refining `to_steps` to consult `env`
(planned). The lift through `IRBlock.to_steps` (`flatMap`) and
`IRFunction.to_program` (`flatMap`) is via `List.mem_flatMap`. -/
theorem IRProgram.to_program_preserves_well_typed
    (p : IRProgram) (env : String → Layout)
    (hwf : p.well_typed env) :
    -- The flattened Program's steps all carry a well-formed layout
    -- (first conjunct of `WellTyped`).
    ∀ s : Step, s ∈ IRProgram.to_program p → WF_Layout s.layout := by
  -- closed sorry-free. Strategy: destructure `p` into its
  -- `functions` / `data_sections` fields, case-split on `functions`,
  -- then for the cons case lift the per-instruction preservation lemma
  -- `PmtInstr.to_steps_preserves_WF_Layout` through two layers of
  -- `List.flatMap` (`IRFunction.to_program` and `IRBlock.to_steps`)
  -- using `List.mem_flatMap` to expose the originating `PmtInstr`.
  intro s hs
  obtain ⟨functions, data_sections⟩ := p
  -- After destructuring, `hwf : (⟨functions, data_sections⟩).well_typed env`
  -- and `hs : s ∈ IRProgram.to_program ⟨functions, data_sections⟩`.
  cases functions with
  | nil =>
    -- `IRProgram.to_program ⟨[], _⟩ = []`; nothing to show.
    simp only [IRProgram.to_program, List.not_mem_nil] at hs
  | cons f rest =>
    -- Extract `f.well_typed env` from `hwf` (f is the head of `functions`).
    -- `hwf`'s type reduces definitionally to `∀ f', f' ∈ f :: rest → f'.well_typed env`.
    have hwf' : ∀ f' : IRFunction, f' ∈ f :: rest → f'.well_typed env := hwf
    have hwf_f : f.well_typed env := hwf' f (List.mem_cons_self)
    obtain ⟨hwf_f_blocks, _⟩ := hwf_f
    -- Reduce `IRProgram.to_program` and `IRFunction.to_program` to the
    -- underlying `flatMap`, then lift membership through it.
    simp only [IRProgram.to_program, IRFunction.to_program] at hs
    rw [List.mem_flatMap] at hs
    obtain ⟨b, hb_in_blocks, hs_b⟩ := hs
    have hb_wt : b.well_typed env := hwf_f_blocks b hb_in_blocks
    obtain ⟨hb_wt_instrs, _⟩ := hb_wt
    simp only [IRBlock.to_steps] at hs_b
    rw [List.mem_flatMap] at hs_b
    obtain ⟨i, hi_in_instrs, hs_i⟩ := hs_b
    have hi_wt : i.well_typed env := hb_wt_instrs i hi_in_instrs
    exact PmtInstr.to_steps_preserves_WF_Layout i env hi_wt s hs_i

/-! ## §5.1. The full `WellTyped` preservation theorem

This section proves `IRProgram.to_program_preserves_well_typed_full`
(the full `WellTyped` preservation theorem, including the two
name-uniqueness conjuncts that previously required a `sorry`).

The proof factors the two name-uniqueness conjuncts out into two
private helpers (`to_program_preserves_in_var_uniqueness` and
`to_program_preserves_out_var_uniqueness`), which in turn delegate to
the generic `filter_length_eq_one_of_pairwise_ne` lemma (a
`Pairwise (· ≠ ·)` hypothesis on a list implies that filtering the list
for elements matching a given `g`-image yields a singleton). The
generic lemma is stated and proven first, then the two IR-specific
helpers, then the main theorem. -/

/-- §5.1.1 (private helper): If `l.Pairwise (fun a b => g a ≠ g b)` and
`x ∈ l`, then filtering `l` for elements whose `g`-image equals `g x`
yields the singleton `[x]`.

This is the key name-uniqueness lemma used by
`to_program_preserves_in_var_uniqueness` /
`to_program_preserves_out_var_uniqueness`. It is proven by induction on
`l` and case-splitting on whether the head is `x` (in which case the
tail must contribute nothing, by `Pairwise`) or `x` is in the tail (in
which case recursion applies). -/
private theorem filter_eq_singleton_of_pairwise_ne
    {α : Type u} {β : Type v} [DecidableEq β]
    (l : List α) (g : α → β)
    (hp : l.Pairwise (fun a b => g a ≠ g b))
    (x : α) (hx : x ∈ l) :
    l.filter (fun a => g a == g x) = [x] := by
  induction l with
  | nil => simp at hx
  | cons head tail ih =>
    rw [List.pairwise_cons] at hp
    obtain ⟨hhead, hptail⟩ := hp
    rw [List.mem_cons] at hx
    simp only [List.filter_cons]
    cases hx with
    | inl h_eq =>
      -- head = x case.
      subst h_eq
      -- `(g x == g x) = true` (reflexivity of BEq on a DecidableEq type).
      have h_refl : (g x == g x) = true := by
        show decide (g x = g x) = true
        exact decide_eq_true_iff.mpr rfl
      rw [if_pos h_refl]
      -- Tail must contribute nothing: every `a ∈ tail` has `g a ≠ g x`
      -- (from `Pairwise.head`), so `g a == g x = false`.
      have h_tail_empty : tail.filter (fun a => g a == g x) = [] := by
        rw [List.filter_eq_nil_iff]
        intro a ha
        intro hcontra
        -- `hcontra : (g a == g x) = true` ↔ `g a = g x` (via `simp`).
        simp at hcontra
        -- `hhead a ha : g head ≠ g a` (now `g x ≠ g a` after subst);
        -- `hcontra : g a = g x` is its symmetric form.
        exact hhead a ha hcontra.symm
      rw [h_tail_empty]
    | inr h_in_tail =>
      -- `x ∈ tail` case: `head ≠ x` is enforced by `Pairwise.head`,
      -- giving `g head ≠ g x`, which means `(g head == g x) = false`.
      have h_ne : g head ≠ g x := hhead x h_in_tail
      have h_bool : (g head == g x) = false := by
        show decide (g head = g x) = false
        exact decide_eq_false h_ne
      -- `if_neg` needs `¬((g head == g x) = true)`, derived from
      -- `(g head == g x) = false` via `Bool.eq_false_iff`.
      rw [if_neg (Bool.eq_false_iff.mp h_bool)]
      exact ih hptail h_in_tail

/-- §5.1.2 (private helper): The length-version of
`filter_eq_singleton_of_pairwise_ne` — the form needed by
`WellTyped`'s conjuncts 2 and 3. -/
private theorem filter_length_eq_one_of_pairwise_ne
    {α : Type u} {β : Type v} [DecidableEq β]
    (l : List α) (g : α → β)
    (hp : l.Pairwise (fun a b => g a ≠ g b))
    (x : α) (hx : x ∈ l) :
    (l.filter (fun a => g a == g x)).length = 1 := by
  rw [filter_eq_singleton_of_pairwise_ne l g hp x hx]
  simp

/-- §5.1.3 (private helper): If `p.well_typed env`, then for every
`Step s` in `IRProgram.to_program p`, the filter selecting steps with
the same `in_var` has length exactly 1. This is conjunct 2 of
`WellTyped`, factored out so it can be reused in
`to_program_preserves_well_typed_full`. -/
private theorem IRProgram.to_program_preserves_in_var_uniqueness
    (p : IRProgram) (env : String → Layout)
    (hwf : p.well_typed env) :
    ∀ s : Step, s ∈ IRProgram.to_program p →
      (List.filter (fun s' => s'.in_var == s.in_var)
        (IRProgram.to_program p)).length = 1 := by
  obtain ⟨functions, data_sections⟩ := p
  cases functions with
  | nil =>
    -- `IRProgram.to_program ⟨[], _⟩ = []`; vacuous.
    simp only [IRProgram.to_program]
    intro s hs
    simp at hs
  | cons f rest =>
    -- Extract `f.well_typed env` from `hwf` (f is the head of `functions`).
    have hwf_f : f.well_typed env := hwf f List.mem_cons_self
    obtain ⟨_, _, h_in_uniq, _⟩ := hwf_f
    -- Key rewrite: `f.flat_steps = IRFunction.to_program f` (modulo `flatMap_assoc`).
    have h_eq : f.flat_steps = IRFunction.to_program f := by
      unfold IRFunction.flat_steps IRFunction.all_instrs
        IRFunction.to_program IRBlock.to_steps
      exact List.flatMap_assoc
    simp only [IRProgram.to_program]
    intro s hs
    -- Rewrite the membership and filter to be over `f.flat_steps`.
    rw [← h_eq] at hs
    rw [← h_eq]
    exact filter_length_eq_one_of_pairwise_ne f.flat_steps (·.in_var) h_in_uniq s hs

/-- §5.1.4 (private helper): Same as `to_program_preserves_in_var_uniqueness`
but for `out_var` — conjunct 3 of `WellTyped`. -/
private theorem IRProgram.to_program_preserves_out_var_uniqueness
    (p : IRProgram) (env : String → Layout)
    (hwf : p.well_typed env) :
    ∀ s : Step, s ∈ IRProgram.to_program p →
      (List.filter (fun s' => s'.out_var == s.out_var)
        (IRProgram.to_program p)).length = 1 := by
  obtain ⟨functions, data_sections⟩ := p
  cases functions with
  | nil =>
    simp only [IRProgram.to_program]
    intro s hs
    simp at hs
  | cons f rest =>
    have hwf_f : f.well_typed env := hwf f List.mem_cons_self
    obtain ⟨_, _, _, h_out_uniq⟩ := hwf_f
    have h_eq : f.flat_steps = IRFunction.to_program f := by
      unfold IRFunction.flat_steps IRFunction.all_instrs
        IRFunction.to_program IRBlock.to_steps
      exact List.flatMap_assoc
    simp only [IRProgram.to_program]
    intro s hs
    rw [← h_eq] at hs
    rw [← h_eq]
    exact filter_length_eq_one_of_pairwise_ne f.flat_steps (·.out_var) h_out_uniq s hs

/-- §5.1: `to_program` preserves the **full** `WellTyped` predicate of
`PMT.Soundness`. This is the stronger statement that
identifies as the bottleneck for `full_simulation` and
`full_simulation_strong`.

The three conjuncts of `WellTyped` (see `PMT.Soundness`):
  1. `∀ s ∈ prog, WF_Layout s.layout`  — **CLOSED** in §5
     (`to_program_preserves_well_typed` above).
  2. `∀ s ∈ prog, (List.filter (fun s' => s'.in_var == s.in_var) prog).length = 1`
     — uniqueness of `in_var` across the flattened program. Closed
     sorry-free using the new
     `IRFunction.in_vars_unique` conjunct (see `PMT/IRProgram.lean` §10).
  3. `∀ s ∈ prog, (List.filter (fun s' => s'.out_var == s.out_var) prog).length = 1`
     — same as (2) for `out_var`. Closed via `IRFunction.out_vars_unique`.

**Status: CLOSED sorry-free.** The previous
`IRProgram.well_typed` did not enforce name-uniqueness across the IR,
and the flattening admitted counterexamples (e.g. a block whose
`instructions` are `[.alloc "x" ⟨1,[]⟩, .store "x" v o t]` produces two
`Step`s with `in_var = "x"`, violating the `length = 1` filter). The
fix is to strengthen `IRFunction.well_typed` with two new conjuncts —
`in_vars_unique` and `out_vars_unique` — that require pairwise
distinctness of `in_var`s and `out_var`s over the *flattened* step
list `f.flat_steps` (which is `f.all_instrs.flatMap PmtInstr.to_steps`).
See the gap report for the full
gap analysis and counterexamples.

**Proof strategy.**
  * **Conjunct 1.** Delegate to `to_program_preserves_well_typed`.
  * **Conjunct 2 / 3.** Case-split on `p.functions`:
      - Empty list: `IRProgram.to_program p = []`, the universal
        quantifier is vacuous.
      - `f :: rest`: `IRProgram.to_program p = IRFunction.to_program f`,
        which is *definitionally equal* to `f.flat_steps` via
        `List.flatMap_assoc` (both reduce to
        `f.blocks.flatMap (fun b => b.instructions.flatMap PmtInstr.to_steps)`).
        Extract `f.in_vars_unique` (resp. `f.out_vars_unique`) from
        `f.well_typed env`, rewrite the goal's step-list to
        `f.flat_steps`, then apply the helper
        `filter_length_eq_one_of_pairwise_ne` (§5.1.2), which converts
        a `Pairwise (· ≠ ·)` hypothesis into the singleton-filter-length-1
        fact via induction on the list.
-/
theorem IRProgram.to_program_preserves_well_typed_full
    (p : IRProgram) (env : String → Layout)
    (hwf : p.well_typed env) :
    WellTyped (IRProgram.to_program p) := by
  -- Unfold WellTyped into its 3 conjuncts and discharge each.
  -- Helpers `to_program_preserves_in_var_uniqueness` and
  -- `to_program_preserves_out_var_uniqueness` are defined above
  -- (§5.1.3, §5.1.4); `to_program_preserves_well_typed` is in §5.
  unfold WellTyped
  refine ⟨?_, ?_, ?_⟩
  · -- Conjunct 1: WF_Layout — closed sorry-free.
    exact IRProgram.to_program_preserves_well_typed p env hwf
  · -- Conjunct 2: in_var uniqueness.
    exact IRProgram.to_program_preserves_in_var_uniqueness p env hwf
  · -- Conjunct 3: out_var uniqueness.
    exact IRProgram.to_program_preserves_out_var_uniqueness p env hwf

end PMT
