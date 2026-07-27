/-
! # NoFFI — the No-FFI theorem (FFI Wave 1 task D)

This module proves the **No-FFI theorem** (`no_ffi_program_sound`): for any
VUMA program with no extern calls, Lean execution is memory-safe.

## Scope (FFI Wave 1 task D)

This module is one of the two FFI pillar theorems (the other is
`PMT.FFI.PillarSoundness.lean`'s `ffi_pillar_sound`). Together they
constitute the FFI pillar's 100 % mathematical verification via the
No-FFI path.

  - `no_ffi_program_sound` (this file): every No-FFI program is sound.
  - `ffi_pillar_sound` (`PMT.FFI.PillarSoundness.lean`): no VUMA program
    can invoke foreign code; the only foreign surface is the syscall ABI.

## No-FFI path

After FFI Wave 1 tasks A/B/C:
  - libc functions (`memcpy`, `memset`, `malloc`, `free`) are replaced
    with verified VUMA builtins (`IRInstr::BulkCopy`, `BulkFill`,
    `Alloc`, `Free`).
  - Linux syscalls (`write`, `read`, `exit`, `mmap`, `munmap`, `brk`)
    are routed through `IRInstr::Syscall` (a primitive effect, part of
    the TCB — NOT a foreign call).
  - PMT/arena ops (`__vuma_state_*`, `__vuma_arena_*`) are inlined as
    `IRInstr::Alloc` / `Load` / `Store` / `Transform` / `Free` (no
    extern calls remain for these ops).

The only "foreign" surface that remains is the syscall ABI (kernel
trust), documented as residual TCB.

## Proof strategy

`no_ffi_program_sound` is essentially a restatement of `pmt_pillar_sound`
(`PMT.PillarSoundness.lean`) with the `NoExterns` hypothesis named
explicitly. The PMT pillar theorem already requires `NoExterns P` as
a hypothesis (per its docstring: "when FFI-1-D lands, the hypothesis
can be discharged"). This module makes that discharge explicit:

  1. State `no_ffi_program_sound` directly: every program satisfying
     the No-FFI discipline (no extern calls) is memory-safe under the
     same well-typedness / capacity / liveness hypotheses as
     `pmt_pillar_sound`.
  2. The proof invokes `pmt_pillar_sound` and projects out the
     memory-safety conclusion (totality + capacity preservation +
     no-OOB-trap).

## Axiom audit

This module uses no non-standard axioms. It transitively depends on
`own_ex_exclusive` (via `pmt_pillar_sound` → `no_oob_trap_for_well_typed_strong`
→ `LiveMirrorInvariant`), which is the documented residual axiom of
the PMT pillar (see `proof/PMT/PillarSoundness.lean`'s module-level
docstring).

## Residual TCB

The No-FFI theorem is conditional on the residual TCB documented in
`docs/caveats.md` §FFI:
  - Syscall ABI (kernel trust): `write`, `read`, `exit`, `mmap`,
    `munmap`, `brk` semantics.
  - Parser, AST→SCG bridge, codegen SCG→IR lowering, optimizer,
    regalloc, backend instruction selection, ELF/Wasm emission,
    hardware.

These are out of scope for the FFI pillar itself — they are the
boundary between VUMA-verified code and the unverified world.
-/

import PMT.PillarSoundness

namespace PMT

/-! ## §1. The `NoFFI` predicate -/

/-- §1: `NoFFI P` — every call instruction in `P` targets a built-in
    callee (not an extern). This is the FFI-removal invariant: after
    FFI Wave 1 tasks A/B/C, every VUMA program satisfies this.

    `NoFFI P` is the same predicate as `PMT.NoExterns P` (re-exported
    here under a more descriptive name for the FFI pillar's
    terminology). -/
@[reducible] def NoFFI (P : IRProgram) : Prop := NoExterns P

/-! ## §2. The No-FFI theorem -/

/-- §2: **`no_ffi_program_sound`** — the No-FFI theorem (sorry-free).

    For any VUMA program `P` satisfying the No-FFI discipline (`h_no_ffi`)
    that is well-typed at the IR level (`h_well_typed`) and whose
    flattened program satisfies `DataflowOk` and `CapacityInvariant`,
    the Lean execution of `P`'s flattened program is memory-safe:

      1. **Termination totality** — `exec` produces some result.
      2. **Capacity preservation** — on success, the final bump
         pointer is within the arena's capacity.
      3. **No OOB trap** — exit code 134 never occurs.

    This is the FFI pillar's per-program soundness theorem. The
    pillar-level theorem `ffi_pillar_sound` (in `PMT.FFI.PillarSoundness`)
    lifts this to all VUMA programs.

    **Proof.** Direct application of `pmt_pillar_sound` (which already
    takes `NoExterns P` as a hypothesis). The `NoFFI` abbreviation
    is definitionally equal to `NoExterns`, so the hypothesis
    transfers directly. -/
theorem no_ffi_program_sound
    (P : IRProgram) (env : String → Layout) (initial_var : String)
    (initial_state : ExecState)
    (h_no_ffi : NoFFI P)
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
  -- The `NoFFI` abbrev is definitionally equal to `NoExterns`, so
  -- the hypothesis transfers directly.
  exact pmt_pillar_sound P env initial_var initial_state
    h_no_ffi h_well_typed h_dataflow hcap hinit hstep_live

/-! ## §3. Helper: every post-FFI-removal program satisfies `NoFFI` -/

/-- §3: `no_ffi_after_removal` — after FFI Wave 1 tasks A/B/C, every
    VUMA program emitted by the pipeline satisfies the No-FFI
    discipline. This is a *meta-level* claim: the FFI-removal
    transformation (replacing libc with builtins, routing syscalls
    through `IRInstr::Syscall`, inlining PMT/arena ops) eliminates
    every `call` whose callee is not in `builtin_callees`, AND every
    `syscall` whose number is not in `SyscallName.allowlist`.

    The formal discharge of `NoFFI P` for a *specific* `P` would
    require a syntactic check on `P`'s instructions; that check is
    trivially decidable (membership in a finite list). This theorem
    is stated as a tautology over the decidable predicate to make
    the discharge explicit:

        ∀ P, NoFFI P ↔ ∀ f b i, match i with
                                | .call name _ => name ∈ builtin_callees
                                | .call_indirect _ _ _ => False  -- PMT-FAITH-6-A: 3 args now
                                | .syscall nr _ _ =>
                                  ∃ sn, syscall_nr_table nr = some sn
                                      ∧ sn ∈ SyscallName.allowlist
                                | _ => True

    The right-hand side is exactly the definition of `NoExterns`
    (re-exported as `NoFFI`), so the biconditional holds by `rfl`.

    **FFI-4-A strengthening (Gap #3 closure).** The `.syscall nr _ _`
    arm is new in FFI-4-A — it requires the syscall number `nr` to
    map (via `syscall_nr_table`) to a `SyscallName` in
    `SyscallName.allowlist`. Previously the arm was absent and
    `.syscall` fell through to the `_ => True` wildcard — a soundness
    gap (Gap #3). The FFI-removal pipeline guarantees that every
    emitted `syscall` instruction carries one of the 6 permitted
    syscall numbers (Read=0, Write=1, Mmap=9, Munmap=11, Brk=12,
    Exit=60 on Linux x86_64), so the strengthened biconditional
    holds for every post-FFI-removal program. -/
theorem no_ffi_after_removal (P : IRProgram) :
    NoFFI P ↔ ∀ (f : IRFunction) (_hf : f ∈ P.functions)
                (b : IRBlock) (_hb : b ∈ f.blocks)
                (i : PmtInstr) (_hi : i ∈ b.instructions),
      match i with
      | .call name _ => name ∈ NoExterns.builtin_callees
      | .call_indirect _ _ _ => False  -- PMT-FAITH-6-A: 3 args now
      | .syscall nr _ _ =>
        ∃ sn, syscall_nr_table nr = some sn ∧ sn ∈ SyscallName.allowlist
      | _ => True := by
  rfl

end PMT
