# X1-impl — x86_64 Register-Based Emitter Wire-Up (env-var gate + fork opt-out)

- **Task ID:** X1-impl
- **Wave:** 1 (x86_64 regalloc wire-up + minimal emitter)
- **Prior-run context:**
  - aarch64 regalloc path is default-on as of `v0.2.0-alpha.6`
    (commit `51ae66be` W2-c-impl / `17d47c38` W5-release).
  - aarch64 uses `LinearScanAllocator` (`regalloc.rs:1214`) +
    `Emitter::emit_function_regalloc` (`emit.rs:1056`) + env-var gate
    + fork detection in `backend.rs:3207-3289`.
  - x86_64 G7 fix (RBP `.not_allocatable()`) in place since commit
    `00b6318f` (E2-a-fix).
  - Design doc: `scripts/audit/regalloc_endianness_wave2_x86_64_design.md`
    classifies x86_64 as LOW readiness for `emit_function_regalloc`
    wire-up — needs a *new* register-based emitter (the existing
    `Emitter::emit_function_regalloc` is aarch64-only).
- **HEAD before this task:** `17d47c38` (W5-release).
- **HEAD after this task:** `31ee347b` (`[X1-impl]`).
- **Files changed:** `src/codegen/src/x86_64/mod.rs` (+90 LOC, 0 deletions).

## §1 What was implemented

`X86_64Backend::allocate_registers` (`x86_64/mod.rs:4141`) was extended
with two pieces of infrastructure that mirror the aarch64 wire-up
(`backend.rs:3207-3289`):

1. **`VUMA_REAL_REGALLOC_X86_64` env-var gate (default OFF).**
   Read at the top of `allocate_registers`. When unset (the default),
   today's behaviour is preserved: stack-slot ISel bytes + target-
   agnostic allocator metadata annotation (additive, no byte changes).
   When set to `"1"`, the same metadata annotation runs PLUS the
   `contains_fork` opt-out (see below). The gate is wired but does
   not yet change bytes — a future wave 2 R2-b-impl will dispatch to
   a real register-based byte emitter here.

2. **`contains_fork` opt-out.** Same predicate as aarch64's R1-b2-fix
   (`backend.rs:3218-3242`), but with x86_64-specific Linux syscall
   numbers: `clone=56`, `vfork=58` (aarch64 uses 220/221). Functions
   containing `Call{func: "spawn_worker"|"fork"}` or
   `Syscall{nr: 56|58}` are flagged and log a fallback message. The
   opt-out exists because a register-based prologue's callee-saved
   `push rbx; push r12; ...` / `pop ...; pop rbx` doesn't interact
   correctly with `clone()` — the child process runs with a different
   register state than the parent. Same hazard documented in
   `docs/caveats.md §2.1.1` for aarch64.

## §2 Why the byte path is unchanged

The aarch64 `Emitter::emit_function_regalloc` at `emit.rs:1056` is
**aarch64-only** at the byte level — it uses `Register::X0..X30` and
`Instruction::SUB/STP/ADD` (aarch64 enums). x86_64 does NOT dispatch
through `Emitter::emit_function` at all (per design doc §1.6: the x86_64
backend calls `stack_slot_isel::allocate_registers(func)` directly).
A complete register-based x86_64 byte emitter that consumes
`RegAllocResult.vreg_to_preg` and emits register-to-register machine
code (with spill/reload insertion + callee-saved prologue/epilogue) is
deferred to wave 2 R2-b-impl — design doc §5.1 proposes a new
`x86_64/reg_isel.rs` module with `pub fn allocate_registers(func,
&RegAllocResult) -> Result<AllocatedFunction, BackendError>`.

Today the env-var gate is a **no-op at the byte level**: the same
`try_real_regalloc(func)` + `annotate_with_regalloc(&mut allocated,
&alloc)` step runs in both modes. The gate exists so that the future
byte-changing emitter can be flipped on without touching this call
site again. This is documented inline in `allocate_registers` and in
the `[X1-impl]` commit message.

## §3 Verification

### §3.1 Build

```
$ cargo build --release --bin compile_dump
   Compiling vuma v0.2.0-alpha.6
    Finished `release` profile [optimized] target(s) in 57.78s
$ echo $?
0
```

Zero warnings. (Note: the sandbox did not have `cargo`/`rustc`
installed at task start; `rustup-init.sh -y --default-toolchain
nightly-2026-03-01 --profile minimal` was run to install the pinned
toolchain. `libz3-dev` is also absent — only the runtime `libz3.so.4`
is present; a `libz3.so` symlink was created in `/tmp/z3lib/` and
exposed via `Z3_LIBRARY_PATH_OVERRIDE=/tmp/z3lib` per `z3-sys`'s
`build.rs`.)

### §3.2 Functional tests (QEMU not required — x86_64 is the host ISA)

```
$ VUMA_REAL_REGALLOC_X86_64=1 target/release/compile_dump \
    tests/gold_standard/u32_arith/u32_add.vuma /tmp/x1_add.bin x86_64
$ /tmp/x1_add.bin; echo "exit=$? (expected 100)"
exit=100 (expected 100)            # PASS

$ VUMA_REAL_REGALLOC_X86_64=1 target/release/compile_dump \
    tests/gold_standard/u32_arith/u32_sub.vuma /tmp/x1_sub.bin x86_64
$ /tmp/x1_sub.bin; echo "exit=$? (expected 30)"
exit=30 (expected 30)              # PASS

$ VUMA_REAL_REGALLOC_X86_64=1 target/release/compile_dump \
    tests/gold_standard/complex_stores/cs_single_store_load.vuma \
    /tmp/x1_cs.bin x86_64
$ /tmp/x1_cs.bin; echo "exit=$? (expected 73)"
exit=73 (expected 73)              # PASS
```

### §3.3 Regression check (gate OFF — default behaviour)

All three tests were re-run WITHOUT `VUMA_REAL_REGALLOC_X86_64=1` to
confirm the wire-up does not regress existing behaviour. Bytes are
**byte-identical** with/without the gate (verified via `cmp`):

```
$ cmp /tmp/no_u32_add.bin /tmp/x1_add.bin && echo identical
u32_add: byte-identical with/without gate
u32_sub: byte-identical
cs_single_store_load: byte-identical
```

This is expected — the gate is metadata-only today. It confirms the
wire-up is purely additive and cannot regress any existing x86_64 test.

## §4 DoD checklist

| DoD criterion | Status | Evidence |
|---|---|---|
| `cargo build --release --bin compile_dump` exits 0 | ✅ PASS | §3.1 (0 warnings, 57.78s) |
| `VUMA_REAL_REGALLOC_X86_64=1` env-var gate in `x86_64/mod.rs allocate_registers` | ✅ PASS | `x86_64/mod.rs:4161-4163` |
| `u32_add` test passes with gate ON (exit 100) | ✅ PASS | §3.2 — exit=100 |

## §5 Out of scope (deferred to wave 2 R2-b-impl)

Per design doc `regalloc_endianness_wave2_x86_64_design.md` §5:

- `src/codegen/src/x86_64/reg_isel.rs` — new register-based byte
  emitter module consuming `RegAllocResult.vreg_to_preg` /
  `spill_code` / `used_callee_saved`.
- Callee-saved prologue/epilogue honouring `used_callee_saved`
  (`push rbx; push r12; ...` / `pop ...; pop rbx`).
- Spill/reload insertion at `RegAllocResult.spill_code` positions
  (position = 2*instruction_index — same convention as aarch64).
- Two-operand ISA handling: `mov dst, lhs; op dst, rhs` whenever
  `dst != lhs`.
- `RegOrSlot` abstraction (design doc §5.1) — vregs in registers vs
  spill slots vs immediates.
- `verify_callee_saved_x86_64` — new verifier parameterized for x86_64
  (caller-saved = RAX, RCX, RDX, RSI, RDI, R8–R11; callee-saved =
  RBX, R12–R15, RBP; always-allowed = RSP). The existing
  `verify_callee_saved` (`regalloc.rs:4860`) is hard-coded to
  aarch64's `PhysReg::Gpr(Register)` and cannot be called on a
  `RegAllocResult`.
- `EmitResult` API change (design doc §7.2, deferred): return
  `frame_size` + `callee_saved` from the emitter for accurate
  debug/unwind info.

## §6 Hazard notes

1. **`contains_fork` syscall numbers are ISA-specific.** aarch64 uses
   220/221 (clone/vfork); x86_64 uses 56/58. The wire-up correctly
   uses 56/58 — verified by code inspection (`x86_64/mod.rs:4188`).
   A future cross-ISA refactor should extract this into a per-target
   predicate to avoid copy-paste drift.

2. **Default-OFF (not default-ON like aarch64).** Rationale: x86_64
   has no byte-changing register-based emitter yet. Flipping the
   default to ON before that emitter exists would be a no-op (bytes
   are stack-slot either way), but it would set a false precedent
   that x86_64 regalloc is "done". Default-OFF makes the
   not-yet-implemented status explicit.

3. **`try_real_regalloc` runs in BOTH modes.** Today the gate is a
   no-op at the byte level — the same `try_real_regalloc` +
   `annotate_with_regalloc` step runs whether the gate is ON or OFF.
   This preserves the existing metadata annotation behaviour (some
   downstream consumers may depend on the `reads`/`writes`/`spill_slots`
   fields being populated). When wave 2 R2-b-impl introduces the
   byte-changing emitter, the OFF branch should keep today's
   metadata-only annotation; the ON branch should switch to the new
   byte path (and skip the redundant `try_real_regalloc` call since
   the new emitter will run the allocator itself).
