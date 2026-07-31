# X2-impl — riscv64 Register-Based Emitter Wire-Up (env-var gate + fork opt-out)

- **Task ID:** X2-impl
- **Wave:** 2 (riscv64 regalloc wire-up — env-var gate + fork opt-out, metadata-only)
- **Prior-run context:**
  - aarch64 regalloc path is default-on as of `v0.2.0-alpha.6`
    (commit `51ae66be` W2-c-impl / `17d47c38` W5-release).
  - x86_64 wire-up done in X1-impl (commit `31ee347b`, doc
    `scripts/audit/completion_wave1_x1_x86_64_regalloc_wireup.md`).
  - riscv64 S0/FP `.not_allocatable()` and Zero-register hazard fixed
    in E3-ab-fix (commit `8605dc98`).
  - Design doc: `scripts/audit/completion_wave_c_riscv64_design.md`
    classifies riscv64 as **LOW** readiness for byte-level
    `emit_function_regalloc` wire-up — it needs a *new* register-based
    emitter (the existing `Emitter::emit_function_regalloc` at
    `emit.rs:1056` is aarch64-only). Design doc §1.7 explicitly says
    "the metadata annotation can stay unconditional" — the env-var
    gate gates only the future byte-changing path.
- **HEAD before this task:** `8d031886` (X1-impl worklog doc).
- **HEAD after this task:** `b6a97940` (`[X2-impl]` code) + this doc.
- **Files changed:** `src/codegen/src/riscv64.rs` (+82 LOC, 0 deletions).

## §1 What was implemented

`RiscV64Backend::allocate_registers` (`riscv64.rs:6607`) was extended
with two pieces of infrastructure that mirror the x86_64 wire-up
(X1-impl, `x86_64/mod.rs:4141`) and the aarch64 wire-up
(`backend.rs:3207-3289`):

1. **`VUMA_REAL_REGALLOC_RISCV64` env-var gate (default OFF).**
   Read at the top of `allocate_registers`. When unset (the default),
   today's behaviour is preserved: stack-slot ISel bytes + target-
   agnostic allocator metadata annotation (additive, no byte changes).
   When set to `"1"`, the same metadata annotation runs PLUS the
   `contains_fork` opt-out is consulted for logging. The gate is wired
   but does not yet change bytes — a future wave (CC-b-impl, design
   doc §5.1) will dispatch to a real register-based byte emitter
   (`src/codegen/src/riscv64/reg_isel.rs`) here.

2. **`contains_fork` opt-out.** Same predicate as aarch64's R1-b2-fix
   (`backend.rs:3218-3242`) and x86_64's X1-impl
   (`x86_64/mod.rs:4165-4192`), but with riscv64-specific Linux syscall
   numbers: `clone=220`, `vfork=221` — **the SAME as aarch64** (per
   CD-a-audit finding, design doc §1.5), unlike x86_64 which uses
   `56/58`. RISC-V uses the generic Linux unified syscall ABI (newer
   ports share the asm-generic table with aarch64). The existing inline
   `spawn_worker` arm at `riscv64.rs:8070-8077` already emits
   `clone=220` via `Addi { rd: A7, rs1: Zero, imm: 220 }`, confirming
   the convention. Functions containing `Call{func: "spawn_worker"|"fork"}`
   or `Syscall{nr: 220|221}` are flagged and log a fallback message.
   The opt-out exists because a register-based prologue's callee-saved
   `sd s1, ...; sd s2, ...` / `ld ...; ld s2; ld s1` doesn't interact
   correctly with `clone()` — the child process runs with a different
   register state. Same hazard documented in `docs/caveats.md §2.1.1`
   for aarch64.

## §2 Why the byte path is unchanged

Per design doc §1.7: "the metadata annotation can stay unconditional".
The existing `try_real_regalloc(func)` + `annotate_with_regalloc` at
`riscv64.rs:10388-10389` runs in **both** gate-ON and gate-OFF modes
(matching X1-impl x86_64's `x86_64/mod.rs:4239`). The
`real_regalloc` and `contains_fork` flags are computed at the top of
the function and used **only for logging** today.

The byte-changing register-based riscv64 emitter that consumes
`RegAllocResult.vreg_to_preg` / `spill_code` / `used_callee_saved` and
emits register-to-register RISC-V machine code (with spill/reload
insertion + callee-saved `sd s1/s2/...` prologue/epilogue) is deferred
to wave CC-b-impl — design doc §5.1 proposes a new
`src/codegen/src/riscv64/reg_isel.rs` module with
`pub fn allocate_registers(func, &RegAllocResult) -> Result<AllocatedFunction,
BackendError>`.

Today the env-var gate is a **no-op at the byte level**: the same
`try_real_regalloc` + `annotate_with_regalloc` step runs in both modes.
The gate exists so that the future byte-changing emitter can be flipped
on without touching this call site again. This mirrors X1-impl x86_64
exactly.

## §3 Verification

### §3.1 Build

```
$ cargo build --release --bin compile_dump
   Compiling z3-sys v0.11.0
   Compiling vuma-codegen v0.2.0-alpha.6
   ...
   Compiling vuma v0.2.0-alpha.6
    Finished `release` profile [optimized] target(s) in 57.42s
$ echo $?
0
```

Zero warnings. (Note: the sandbox lacked a `libz3.so` symlink — only
the runtime `libz3.so.4` was present at
`/usr/lib/x86_64-linux-gnu/libz3.so.4`. A symlink at `/tmp/z3lib/libz3.so`
— created by prior wave 0 work — was exposed via
`LIBRARY_PATH=/tmp/z3lib` + `LD_LIBRARY_PATH=/tmp/z3lib`, matching the
X1-impl workaround documented in
`scripts/audit/completion_wave1_x1_x86_64_regalloc_wireup.md` §3.1.)

### §3.2 QEMU user-mode install

The sandbox had no `qemu-riscv64-static` on PATH (the
`scripts/env/qemu-env.sh` shim expects `$HOME/.local/bin/qemu-*` but
that dir was absent at task start — likely lost between sessions). The
static binaries were re-installed by extracting the Debian trixie
`qemu-user` .deb (no root available, so `apt-get install` is blocked
by the dpkg lock — same extraction approach as wave 0's 0-c-install
task documented in `scripts/env/qemu-env.sh`):

```
$ apt-get download qemu-user      # 71.1 MB .deb
$ dpkg-deb -x qemu-user_*.deb /tmp/qemu-user-extract
$ cp /tmp/qemu-user-extract/usr/bin/qemu-riscv64 $HOME/.local/bin/qemu-riscv64-static
$ cp /tmp/qemu-user-extract/usr/bin/qemu-riscv64 $HOME/.local/bin/qemu-riscv64
$ qemu-riscv64-static --version | head -1
qemu-riscv64 version 10.0.11 (Debian 1:10.0.11+ds-0+deb13u1)
```

(Note: the `qemu-user-static` .deb is a *transitional* package whose
`usr/bin/qemu-riscv64-static` is a symlink to a non-existent
`qemu-riscv64` — the real statically-linked binaries live in the
`qemu-user` package itself in Debian trixie.)

### §3.3 Functional test (gate ON)

```
$ VUMA_REAL_REGALLOC_RISCV64=1 target/release/compile_dump \
    tests/gold_standard/u32_arith/u32_add.vuma /tmp/x2.bin riscv64
IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
Wrote 2732 bytes to /tmp/x2.bin
$ qemu-riscv64-static /tmp/x2.bin; echo "exit=$? (expected 100)"
exit=100 (expected 100)            # PASS
```

### §3.4 Regression check (gate OFF — default behaviour)

```
$ target/release/compile_dump \
    tests/gold_standard/u32_arith/u32_add.vuma /tmp/x2_nogate.bin riscv64
Wrote 2732 bytes to /tmp/x2_nogate.bin
$ qemu-riscv64-static /tmp/x2_nogate.bin; echo "exit=$? (expected 100)"
exit=100 (expected 100)            # PASS
$ cmp /tmp/x2_nogate.bin /tmp/x2.bin && echo identical
BYTE-IDENTICAL with/without gate
```

Bytes are **byte-identical** with/without the gate (verified via
`cmp`). This is expected — the gate is metadata-only today. It
confirms the wire-up is purely additive and cannot regress any
existing riscv64 test.

## §4 DoD checklist

| DoD criterion | Status | Evidence |
|---|---|---|
| `cargo build --release --bin compile_dump` exits 0 | ✅ PASS | §3.1 (0 warnings, 57.42s) |
| `VUMA_REAL_REGALLOC_RISCV64=1` env-var gate in `riscv64.rs allocate_registers` | ✅ PASS | `riscv64.rs:6632-6634` |
| `u32_add` passes with gate ON (exit 100) | ✅ PASS | §3.3 — exit=100 |

## §5 Out of scope (deferred to wave CC-b-impl)

Per design doc `completion_wave_c_riscv64_design.md` §5:

- `src/codegen/src/riscv64/reg_isel.rs` — new register-based byte
  emitter module consuming `RegAllocResult.vreg_to_preg` /
  `spill_code` / `used_callee_saved`. Design doc §6.1 estimates
  ~2000–2500 LOC.
- Callee-saved prologue/epilogue honouring `used_callee_saved`
  (`sd s1, -8(sp); sd s2, -16(sp); ...` / `ld ...; ld s2; ld s1`).
- Spill/reload insertion at `RegAllocResult.spill_code` positions
  (position = 2*instruction_index — same convention as aarch64).
- **Zero-register hazard mitigation** (design doc §7.5): the emitter
  must NOT honour the `spill_code` `preg` field literally when it
  resolves to `Register::Zero` (x0) — Zero is hard-wired to 0 and
  cannot hold a value. Spills to x0 must be redirected to a scratch
  register or skipped. This is a riscv64-specific complication vs
  x86_64/aarch64.
- Three-operand ISA simplification (design doc §2.4): RISC-V's
  `op rd, rs1, rs2` form means no two-operand `mov dst, lhs`
  insertions are needed (unlike x86_64). Net effort roughly the same
  as x86_64 Phase 2a per design doc §6.2.
- `verify_callee_saved_riscv64` — new verifier parameterized for
  riscv64 (caller-saved = a0-a7, t0-t6; callee-saved = s0-s11;
  always-allowed = zero, ra, sp, gp, tp). The existing
  `verify_callee_saved` (`regalloc.rs:4860`) is hard-coded to
  aarch64's `PhysReg::Gpr(Register)`.
- Flip `VUMA_REAL_REGALLOC_RISCV64` default to `1` after the curated
  test suite passes (design doc §6.3, Phase Cd).

## §6 Hazard notes

1. **`contains_fork` syscall numbers are ISA-specific.** aarch64 and
   riscv64 both use 220/221 (clone/vfork — the asm-generic unified
   syscall ABI); x86_64 uses 56/58. The wire-up correctly uses 220/221
   for riscv64 — verified by code inspection (`riscv64.rs:6665`) and
   cross-checked against the existing inline `spawn_worker` arm at
   `riscv64.rs:8070-8077` which emits `Addi { rd: A7, rs1: Zero, imm:
   220 }`. A future cross-ISA refactor should extract this into a
   per-target predicate to avoid copy-paste drift (same TODO as
   X1-impl §6.1).

2. **Default-OFF (not default-ON like aarch64).** Rationale: riscv64
   has no byte-changing register-based emitter yet. Flipping the
   default to ON before that emitter exists would be a no-op (bytes
   are stack-slot either way), but it would set a false precedent
   that riscv64 regalloc is "done". Default-OFF makes the
   not-yet-implemented status explicit. Same rationale as X1-impl
   x86_64 §6.2.

3. **`try_real_regalloc` runs in BOTH modes.** Today the gate is a
   no-op at the byte level — the same `try_real_regalloc` +
   `annotate_with_regalloc` step runs whether the gate is ON or OFF
   (per design doc §1.7). This preserves the existing metadata
   annotation behaviour (downstream consumers — disassemblers,
   debuggers, future codegen waves — depend on the `reads`/`writes`/
   `spill_slots` fields being populated). When wave CC-b-impl
   introduces the byte-changing emitter, the OFF branch should keep
   today's metadata-only annotation; the ON branch should switch to
   the new byte path. Same forward-compat note as X1-impl §6.3.

4. **qemu-riscv64-static was re-installed from Debian trixie's
   `qemu-user` .deb** (not the `qemu-user-static` transitional
   package, whose `usr/bin/qemu-riscv64-static` is a dangling symlink
   in trixie). This matches the wave 0 0-c-install extraction
   approach documented in `scripts/env/qemu-env.sh`. The
   `qemu-user-static` .deb was a red herring — only 70.2 KB because
   it's transitional; the real 71.1 MB of statically-linked binaries
   live in `qemu-user`.
