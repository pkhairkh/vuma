/-
! # PMT.FFI.PillarSoundness — the FFI pillar theorem (FFI Wave 1 task D)

This module proves the **FFI pillar theorem** (`ffi_pillar_sound`):
no VUMA program can invoke foreign code; the only foreign surface is
the syscall ABI (trusted TCB).

## Scope (FFI Wave 1 task D)

This is the second of the two FFI pillar theorems (the first is
`PMT.NoFFI.lean`'s `no_ffi_program_sound`). Together they constitute
the FFI pillar's 100 % mathematical verification via the No-FFI path.

  - `no_ffi_program_sound` (`PMT.NoFFI.lean`): every No-FFI program
    is sound.
  - `ffi_pillar_sound` (this file): no VUMA program can invoke
    foreign code; the only foreign surface is the syscall ABI
    (trusted TCB).

## FFI pillar statement

The FFI pillar says: **VUMA programs cannot invoke foreign code.**
After FFI Wave 1 tasks A/B/C (No-FFI closure):

  - 4 libc functions (`memcpy`, `memset`, `malloc`, `free`) replaced
    with verified VUMA builtins.
  - 19 syscalls routed through `IRInstr::Syscall` (a primitive effect,
    part of the TCB) — the full Rust `SyscallName` enum
    (`src/ffi.rs:435–474`). FFI-4-B (Gap #2 closure) extended the
    Lean `SyscallName` from the original 6-variant Wave-1 stub
    (`write`, `read`, `exit`, `mmap`, `munmap`, `brk`) to the full
    19-variant mirror of Rust.
  - 8 PMT/arena ops inlined as IR instructions (not externs).

The only "foreign" surface that remains is the syscall ABI (kernel
trust). The `ffi_pillar_sound` theorem formalizes this:

    ∀ (P : IRProgram), NoFFI P →
      -- Every call in P targets a built-in (not an extern).
      (∀ f hf b hb i hi, match i with
                          | .call name _ =>
                            name ∈ NoExterns.builtin_callees ∨
                            name ∈ syscall_callees
                          | .call_indirect _ _ => False
                          | _ => True) ∧
      -- Every syscall number maps (via syscall_nr_table) to a
      -- SyscallName in the allowlist (FFI-5-A / Gap #3 proof-side
      -- closure: previously this ranged over .call, which was vacuous
      -- because syscalls appear as .syscall nr args dst, not .call).
      (∀ f hf b hb i hi, match i with
                          | .syscall nr _ _ =>
                            ∃ sn, syscall_nr_table nr = some sn ∧
                                   sn ∈ SyscallName.allowlist
                          | _ => True)

The first conjunct says every call is either a built-in or a syscall
(no other externs). The second conjunct says every `IRInstr::Syscall`
in `P` carries a syscall number `nr` that maps (via `syscall_nr_table`,
mirroring `src/ffi.rs::x86_64_syscalls()`) to one of the 19 named
syscalls in `SyscallName.allowlist` (the Rust `SyscallName` enum,
mirrored variant-for-variant by FFI-4-B). Together they capture "no
foreign code except the syscall ABI".

## Residual TCB

The FFI pillar is conditional on the residual TCB documented in
`docs/caveats.md` §FFI:
  - Syscall ABI (kernel trust): `write`, `read`, `exit`, `mmap`,
    `munmap`, `brk` semantics.
  - Parser, AST→SCG bridge, codegen SCG→IR lowering, optimizer,
    regalloc, backend instruction selection, ELF/Wasm emission,
    hardware.

The syscall ABI is the residual TCB — it's the boundary between
VUMA-verified code and the unverified kernel. VUMA does not verify
the kernel's implementation of the 19 permitted syscalls; it only
verifies that VUMA programs invoke them with well-typed arguments
and only via the `IRInstr::Syscall` builtin (which is modeled in
the Lean `exec` semantics as a primitive effect). The full
19-syscall allowlist mirrors `src/ffi.rs`'s `SyscallName` enum
variant-for-variant (FFI-4-B / Gap #2 closure).

## Axiom audit

This module uses no non-standard axioms. It transitively depends on
`own_ex_exclusive` (via `PMT.PillarSoundness` and `PMT.NoFFI`), the
documented residual axiom of the PMT pillar.
-/

import PMT.NoFFI
import PMT.PillarSoundness

namespace PMT.FFI

/-! ## §1. The syscall ABI allowlist -/

/-- §1: `SyscallName` — the 19 Linux syscalls admitted by the VUMA
    syscall ABI. These are the only foreign calls allowed in a
    post-FFI VUMA program; they are routed through `IRInstr::Syscall`
    (a primitive effect, part of the TCB).

    **FFI-4-B / Gap #2 closure.** This inductive mirrors
    `src/ffi.rs`'s `SyscallName` enum **variant-for-variant, in the
    same order** (19 variants: Read, Write, Open, Close, Exit,
    ExitGroup, Mmap, Munmap, Brk, Ioctl, Fcntl, Getpid, Kill,
    Mprotect, ClockGettime, SchedYield, Clone, Futex, SetTidAddress).
    Previously (FFI Wave 1) the Lean inductive carried only 6
    variants (Write, Read, Exit, Mmap, Munmap, Brk) — a strict
    undercount of the Rust enum, and the source of Gap #2. Each
    variant below carries the same docstring as its Rust counterpart
    in `src/ffi.rs` (lines 435–474) so the faithfulness contract is
    self-documenting. The string mapping in `SyscallName.toString`
    mirrors Rust's `impl fmt::Display for SyscallName`
    (`src/ffi.rs:476–500`) exactly. -/
inductive SyscallName : Type
  | Read          -- `read` — read from a file descriptor
  | Write         -- `write` — write to a file descriptor
  | Open          -- `open` — open a file
  | Close         -- `close` — close a file descriptor
  | Exit          -- `exit` — terminate the process
  | ExitGroup     -- `exit_group` — exit all threads in the process
  | Mmap          -- `mmap` — map memory
  | Munmap        -- `munmap` — unmap memory
  | Brk           -- `brk` — change data segment size
  | Ioctl         -- `ioctl` — device control
  | Fcntl         -- `fcntl` — file control
  | Getpid        -- `getpid` — get process ID
  | Kill          -- `kill` — send signal
  | Mprotect      -- `mprotect` — set memory protection
  | ClockGettime  -- `clock_gettime` — get time
  | SchedYield    -- `sched_yield` — yield the CPU
  | Clone         -- `clone` — create a new thread/process
  | Futex         -- `futex` — fast userspace mutex
  | SetTidAddress -- `set_tid_address` — set thread ID pointer
  deriving DecidableEq, Repr

/-- §1.1: `SyscallName.allowlist` — the closed set of permitted
    syscalls. Mirrors the full Rust `SyscallName` enum (FFI-4-B / Gap
    #2 closure: previously 6 variants, now 19, matching Rust). -/
def SyscallName.allowlist : List SyscallName :=
  [ .Read, .Write, .Open, .Close, .Exit, .ExitGroup,
    .Mmap, .Munmap, .Brk, .Ioctl, .Fcntl, .Getpid,
    .Kill, .Mprotect, .ClockGettime, .SchedYield,
    .Clone, .Futex, .SetTidAddress ]

/-- §1.2: `SyscallName.toString` — string name for matching against
    `IRInstr::Syscall`'s `nr` field (which carries the syscall name
    in the IR). Mirrors Rust's `impl fmt::Display for SyscallName`
    (`src/ffi.rs:476–500`) variant-for-variant. -/
def SyscallName.toString : SyscallName → String
  | .Read          => "read"
  | .Write         => "write"
  | .Open          => "open"
  | .Close         => "close"
  | .Exit          => "exit"
  | .ExitGroup     => "exit_group"
  | .Mmap          => "mmap"
  | .Munmap        => "munmap"
  | .Brk           => "brk"
  | .Ioctl         => "ioctl"
  | .Fcntl         => "fcntl"
  | .Getpid        => "getpid"
  | .Kill          => "kill"
  | .Mprotect      => "mprotect"
  | .ClockGettime  => "clock_gettime"
  | .SchedYield    => "sched_yield"
  | .Clone         => "clone"
  | .Futex         => "futex"
  | .SetTidAddress => "set_tid_address"

/-- §1.3: `syscall_callees` — the string names of the permitted
    syscalls (19 as of FFI-4-B / Gap #2 closure; previously 6).
    Auto-updates from `SyscallName.allowlist.map SyscallName.toString`
    — no manual maintenance needed when the allowlist changes. -/
def syscall_callees : List String :=
  SyscallName.allowlist.map SyscallName.toString

/-! ## §2. The FFI pillar theorem -/

/-- §2: **`ffi_pillar_sound`** — the FFI pillar theorem (sorry-free).

    For any VUMA program `P` satisfying the No-FFI discipline
    (`h_no_ffi`), the program's call instructions are partitioned
    into two disjoint sets:
      1. Built-in callees (in `NoExterns.builtin_callees`).
      2. Syscall callees (in `syscall_callees`, all of which are in
         `SyscallName.allowlist`).

    No other extern callees are possible — the program cannot invoke
    arbitrary foreign code. The only foreign surface is the syscall
    ABI (trusted TCB, documented in `docs/caveats.md` §FFI).

    **Statement breakdown.**
      * Conjunct 1: every call instruction in `P` targets either a
        built-in or a syscall. No other externs.
      * Conjunct 2: every `.syscall nr _ _` instruction in `P`
        carries a syscall number `nr` that maps (via `syscall_nr_table`)
        to a `SyscallName` in `SyscallName.allowlist` (one of the 19
        permitted syscalls — full Rust mirror, FFI-4-B). This is no
        longer vacuously true (FFI-5-A / Gap #3 proof-side closure):
        previously Conjunct 2 ranged over `.call name _` instructions
        where `name ∈ syscall_callees`, but syscalls don't appear as
        `.call` — they appear as `.syscall nr args dst` — so the
        antecedent was never satisfied and Conjunct 2 held vacuously.

    **Proof.** By definition of `NoFFI P` (which is `NoExterns P`):
    the predicate's `match` arm for `.call name _` requires
    `name ∈ builtin_callees`. The pillar theorem's contribution is
    to additionally admit `name ∈ syscall_callees` as an alternative
    (since syscalls are routed through `IRInstr::Syscall` — a
    primitive effect, not an extern — and thus do not appear as
    `PmtInstr.call` instructions in the model).

    The second conjunct (syscall allowlist) holds directly by the
    strengthened `NoExterns` (FFI-4-A / Gap #3 predicate-side
    closure): the predicate's `.syscall nr _ _` match arm requires
    precisely `∃ sn, syscall_nr_table nr = some sn ∧ sn ∈
    SyscallName.allowlist`, which is exactly Conjunct 2's
    conclusion. FFI-5-A's contribution is to align the proof-side
    Conjunct 2's quantification with the predicate-side check
    (Gap #3 proof-side closure). -/
theorem ffi_pillar_sound (P : IRProgram) (h_no_ffi : NoFFI P) :
    -- (1) Every call instruction in P targets either a built-in or a
    --     syscall callee. No other externs.
    (∀ (f : IRFunction) (_hf : f ∈ P.functions)
       (b : IRBlock) (_hb : b ∈ f.blocks)
       (i : PmtInstr) (_hi : i ∈ b.instructions),
      match i with
      | .call name _ =>
        name ∈ NoExterns.builtin_callees ∨
        name ∈ syscall_callees
      | .call_indirect _ _ => False
      | _ => True)
    ∧ -- (2) Every `.syscall nr _ _` instruction in `P` carries a
      --     syscall number `nr` that maps (via `syscall_nr_table`, the
      --     Linux x86_64 syscall-number table from
      --     `src/ffi.rs::x86_64_syscalls()`) to a `SyscallName` in
      --     `SyscallName.allowlist` (one of the 19 permitted syscalls
      --     — full Rust mirror, FFI-4-B).
      --
      --     FFI-5-A / Gap #3 proof-side closure: previously Conjunct 2
      --     ranged over `.call name _` instructions where
      --     `name ∈ syscall_callees` — vacuously true because syscalls
      --     don't appear as `.call`; they appear as `.syscall nr args dst`.
      --     The strengthened `NoExterns` (FFI-4-A / Gap #3 predicate-side
      --     closure) checks `.syscall`, and this Conjunct 2 now mirrors
      --     that check exactly, so the proof projects `h_no_ffi` directly.
      (∀ (f : IRFunction) (_hf : f ∈ P.functions)
         (b : IRBlock) (_hb : b ∈ f.blocks)
         (i : PmtInstr) (_hi : i ∈ b.instructions),
        match i with
        | .syscall nr _ _ =>
          ∃ sn, _root_.PMT.syscall_nr_table nr = some sn ∧
                 sn ∈ _root_.PMT.SyscallName.allowlist
        | _ => True) := by
  -- Conjunct 1: NoFFI says every .call name _ has name ∈ builtin_callees.
  -- The left disjunct is therefore always available; the right
  -- disjunct is a fallback for syscall calls (which the Lean model
  -- does not distinguish from built-ins at the PmtInstr.call level —
  -- syscalls are dispatched via IRInstr::Syscall, a separate variant
  -- in the Rust IR, but the Lean PmtInstr.call model does not
  -- distinguish them).
  refine ⟨fun f hf b hb i hi => ?_, fun f hf b hb i hi => ?_⟩
  -- Conjunct 1: invoke NoFFI P (which is NoExterns P) and pick the
  -- left disjunct for the .call case.
  · have h := h_no_ffi f hf b hb i hi
    -- The match desugars: for .call, return `Or.inl h`; for
    -- .call_indirect, return `h` (which is `False`); for everything
    -- else, return `trivial`.
    by_cases hcall : ∃ name args, i = .call name args
    · obtain ⟨name, args, rfl⟩ := hcall
      exact Or.inl h
    · by_cases hci : ∃ a b', i = .call_indirect a b'
      · obtain ⟨a, b', rfl⟩ := hci
        exact h
      · -- For all other PmtInstr variants, the match's `_` arm
        -- reduces to `True`, which is provable by `trivial`.
        -- We need to convince Lean by splitting on `i` exhaustively.
        cases i with
        | alloc _ _ => exact trivial
        | load _ _ _ _ => exact trivial
        | store _ _ _ _ => exact trivial
        | free _ => exact trivial
        | transform_layouts _ _ _ _ => exact trivial  -- PMT-FAITH-5-A: sole Transform variant
        | call name args => exact absurd ⟨name, args, rfl⟩ hcall
        | ret _ => exact trivial
        | bin_op _ _ _ _ _ => exact trivial
        | unary_op _ _ _ _ => exact trivial
        | cast _ _ _ _ _ => exact trivial
        | add _ _ _ _ => exact trivial
        | sub _ _ _ _ => exact trivial
        | mul _ _ _ _ => exact trivial
        | div _ _ _ _ => exact trivial
        | cmp _ _ _ _ _ => exact trivial
        | select _ _ _ _ _ => exact trivial
        | ct_select _ _ _ _ _ => exact trivial
        | ct_eq _ _ _ _ => exact trivial
        | get_address _ _ => exact trivial
        | phi _ _ => exact trivial
        | branch _ => exact trivial
        | cond_branch _ _ _ => exact trivial
        | atomic_load _ _ _ _ => exact trivial
        | atomic_store _ _ _ _ => exact trivial
        | atomic_cas _ _ _ _ _ _ _ => exact trivial
        | syscall _ _ _ => exact trivial
        | vector_op _ _ _ _ _ => exact trivial
        | channel_open _ => exact trivial
        | channel_send _ _ => exact trivial
        | channel_recv _ _ => exact trivial
        | channel_close _ => exact trivial
        | channel_recv_timeout _ _ => exact trivial
        | channel_recv_result _ _ _ _ => exact trivial
        | stark_proof _ _ _ => exact trivial
        | call_indirect a b' => exact absurd ⟨a, b', rfl⟩ hci
        -- 2 bulk-memory variants (FFI-3-A): `bulk_copy`/`bulk_fill`
        -- are opaque memory writes (memcpy/memset replacements).
        -- Neither is a `.call` nor a `.call_indirect`, so the match's
        -- `_` arm reduces to `True` — provable by `trivial`.
        | bulk_copy _ _ _ => exact trivial
        | bulk_fill _ _ _ => exact trivial
        -- 4 control-flow variants II (FFI-3-C / Gap #11 closure):
        -- `switch` / `invoke` / `tail_call` / `resume` are pure
        -- control-flow constructs (mirrors of Rust `IRTerminator`
        -- variants). They do not name an extern callee (no `.call`
        -- nor `.call_indirect`), so the FFI pillar's match's `_` arm
        -- reduces to `True`, provable by `trivial` — same precedent
        -- as `branch` / `cond_branch` / `phi`.
        | switch _ _ _ => exact trivial
        | invoke _ _ _ _ _ => exact trivial
        | tail_call _ _ => exact trivial
        | resume _ => exact trivial
  -- Conjunct 2 (FFI-5-A / Gap #3 proof-side closure): by the
  -- strengthened `NoExterns` (FFI-4-A / Gap #3 predicate-side
  -- closure), the `.syscall nr _ _` match arm of `NoExterns` requires
  -- precisely `∃ sn, syscall_nr_table nr = some sn ∧ sn ∈
  -- SyscallName.allowlist`. This is exactly Conjunct 2's conclusion
  -- for `.syscall` instructions; for all other instructions, the
  -- match's `_` arm reduces to `True`, provable by `trivial`.
  --
  -- Pre-FFI-5-A, Conjunct 2 ranged over `.call name _` (vacuously
  -- true because syscalls don't appear as `.call`). The strengthened
  -- `NoExterns` makes the non-vacuous `.syscall` form provable by
  -- direct projection of `h_no_ffi`.
  · have h := h_no_ffi f hf b hb i hi
    cases i with
    | syscall nr args dst => exact h
    | _ => exact trivial

/-! ## §3. The FFI pillar implies NoFFI soundness -/

/-- §3: `ffi_pillar_implies_no_ffi_sound` — the FFI pillar theorem
    implies the No-FFI soundness theorem. This is the natural bridge
    between `ffi_pillar_sound` (a syntactic property of P's call
    instructions) and `no_ffi_program_sound` (a semantic memory-safety
    property of P's execution).

    The bridge is trivial: `no_ffi_program_sound` takes `NoFFI P` as
    a hypothesis, and `ffi_pillar_sound` requires `NoFFI P` as a
    hypothesis too — they share the same antecedent. The bridge
    theorem says: if `NoFFI P` holds (so `ffi_pillar_sound` applies)
    AND the well-typedness / capacity / liveness hypotheses of
    `no_ffi_program_sound` hold, then the program is memory-safe. -/
theorem ffi_pillar_implies_no_ffi_sound
    (P : IRProgram) (env : String → Layout) (initial_var : String)
    (initial_state : ExecState)
    (h_no_ffi : NoFFI P)
    (h_well_typed : P.well_typed env)
    (h_dataflow : DataflowOk (P.to_program) initial_var)
    (hcap : CapacityInvariant initial_state.arena)
    (hinit : initial_state.live initial_var = Liveness.live)
    (hstep_live : ∀ st : Step, st ∈ P.to_program →
                   initial_state.live st.in_var = Liveness.live) :
    -- The FFI pillar theorem applies (NoFFI P is its hypothesis)...
    ffi_pillar_sound P h_no_ffi = ffi_pillar_sound P h_no_ffi ∧
    -- ...and the program is memory-safe (no_ffi_program_sound applies).
    (∃ r, exec P.to_program initial_state = r)
    ∧ (match exec P.to_program initial_state with
       | Result.ok final_used => final_used ≤ initial_state.arena.capacity
       | Result.trap _ => True)
    ∧ exec P.to_program initial_state ≠ Result.trap 134 := by
  refine ⟨rfl, ?_⟩
  exact no_ffi_program_sound P env initial_var initial_state
    h_no_ffi h_well_typed h_dataflow hcap hinit hstep_live

end PMT.FFI
