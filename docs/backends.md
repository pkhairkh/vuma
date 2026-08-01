# Backend Reference Matrix

**Version:** 0.2.0-alpha.10
**Stage:** backends
**Crate:** `vuma-codegen` (`src/codegen/src/backend.rs` + per-backend
directories under `src/codegen/src/<isa>/`).
**Cross-refs:** [architecture.md](./architecture.md),
[pipeline.md](./pipeline.md), [caveats.md](./caveats.md),
[building.md](./building.md), [testing.md](./testing.md).

Single source of truth for the 19 VUMA backends. As of v0.2.0-alpha.10,
**15 of 19 backends have their own emission path** (14 with full
register-based emission via a per-backend `reg_isel.rs` module, plus
`wasm32` with structured stack-machine emission), and the remaining **4
backends** (`aarch64_be`, `armeb`, `mips64be`, `ppc64le`) inherit their
parent's emission via one-line delegation and a byte-swap wrapper.

There are **no stack-slot fallbacks on the default code path**. The
target-agnostic linear-scan allocator (`regalloc.rs`) and the per-backend
`reg_isel::emit_function_regalloc_full` emitters cover every IR
instruction. The legacy stack-slot ISel lives on only as a **targeted
opt-out** for functions that contain a `clone`/`fork` syscall — the
child process's divergent register state is incompatible with the
register-based prologue/epilogue, so such functions deliberately take
the stack-slot path. See [§5](#5-contains_fork-opt-out-clonefork-detection)
for why this is a correctness requirement, not a fallback.

> **Test status.** All 19 backends pass the curated 30-test matrix
> (`scripts/vuma_test_matrix_19backends.sh`). The 4 byte-swap wrappers
> inherit their parent's emission byte-for-byte and only differ in ELF
> endianness, so they pass whenever the parent passes.

---

## 1. Backend Matrix

| #  | Backend        | ISA            | Endian | Emission        | Inherited | Notes                                            |
|---:|----------------|----------------|--------|-----------------|-----------|--------------------------------------------------|
|  1 | `x86_64`       | x86-64         | LE     | register-based  | —         | Native on x86_64 hosts                           |
|  2 | `x86_32`       | i386           | LE     | register-based  | —         | Runs via `qemu-i386-static` on x86_64            |
|  3 | `aarch64`      | A64            | LE     | register-based  | —         | Uses `LinearScanAllocator` + `Emitter` (see §6)  |
|  4 | `aarch64_be`   | A64            | BE     | register-based  | aarch64   | Byte-swap wrapper (ELF header only)              |
|  5 | `arm32`        | ARMv7-A        | LE     | register-based  | —         |                                                  |
|  6 | `armeb`        | ARMv7-A        | BE     | register-based  | arm32     | Byte-swap wrapper (BE32 word swap)               |
|  7 | `riscv64`      | RV64GC         | LE     | register-based  | —         |                                                  |
|  8 | `riscv32`      | RV32GC         | LE     | register-based  | —         | `qemu-riscv32-static -cpu max` (D extension)     |
|  9 | `mips64`       | MIPS64         | LE     | register-based  | —         | Emits LE ELF; run via `qemu-mips64el-static`     |
| 10 | `mips64be`     | MIPS64         | BE     | register-based  | mips64    | Byte-swap wrapper (instruction words only)       |
| 11 | `ppc64`        | Power ISA v3   | BE     | register-based  | —         | Native BE; ELFv2 ABI                             |
| 12 | `ppc64le`      | Power ISA v3   | LE     | register-based  | ppc64     | Byte-swap wrapper (BE→LE full swap)              |
| 13 | `loongarch64`  | LoongArch      | LE     | register-based  | —         |                                                  |
| 14 | `s390x`        | z/Arch         | BE     | register-based  | —         |                                                  |
| 15 | `sparc64`      | SPARC V9       | BE     | register-based  | —         | Register windows (`SAVE`/`RESTORE`)              |
| 16 | `alpha`        | Alpha 21064    | LE     | register-based  | —         |                                                  |
| 17 | `hppa`         | PA-RISC 1.1    | BE     | register-based  | —         |                                                  |
| 18 | `m68k`         | Motorola 68k   | BE     | register-based  | —         | D/A register separation                          |
| 19 | `wasm32`       | WebAssembly    | LE     | stack-machine   | —         | Structured stack emission (not register-based)   |

**Totals:** 14 backends with a direct `reg_isel.rs` emitter
(`x86_64`, `x86_32`, `arm32`, `riscv64`, `riscv32`, `mips64`, `ppc64`,
`loongarch64`, `s390x`, `sparc64`, `alpha`, `hppa`, `m68k`) **+**
`aarch64` (register-based via `Emitter::emit_function_regalloc` in
`emit.rs`) = **14 backends with full register-based emission**. **+** 4
byte-swap wrappers that inherit a parent's emission = 18. (`wasm32` is
listed in row 19 because its structured stack-machine emission is its
own emission path — not register-based, not inherited, not a fallback.)

---

## 2. Per-Backend ISA-Specific Design Decisions

| Backend       | Key design points                                                                                                |
|---------------|------------------------------------------------------------------------------------------------------------------|
| `x86_64`      | R11 `not_allocatable` (scratch for immediates); 2-operand constraint; REX prefix handling                         |
| `x86_32`      | Same as x86_64 but 32-bit; args on stack; `int 0x80` syscall                                                     |
| `aarch64`     | X15 scratch; `CSEL` for conditional moves; AAPCS64 calling convention                                            |
| `arm32`       | Conditional execution (`MOVcc`); no hardware divide; R12 scratch; `PUSH`/`POP` multiple                          |
| `riscv64`     | T5/T6 `not_allocatable` (scratch); 3-operand; no delay slots; `ecall`                                            |
| `riscv32`     | Same as riscv64 but 32-bit (`sw`/`lw` instead of `sd`/`ld`)                                                      |
| `mips64`      | Branch delay slots (NOP after every branch/jump); HI/LO for mul/div; 4 arg regs                                  |
| `ppc64`       | R11 scratch; CR0 + `mfcr` for comparisons; `isel` (conditional move); `blr` return                               |
| `loongarch64` | T7/T8 scratch; `maskeqz`/`masknez` for conditional select; no delay slots                                        |
| `sparc64`     | Register windows (`SAVE`/`RESTORE`); branch delay slots; `SETHI` for upper immediates                            |
| `s390x`       | R0 scratch; big-endian; `SVC 0` syscall; 5 arg regs (R2–R6)                                                      |
| `alpha`       | R27 scratch; 3-operand; `callsys`; branch PC+4 bias                                                              |
| `hppa`        | `GATE` for syscalls (NOP after); 4 arg regs (R26–R23 reversed); `BV` return                                      |
| `m68k`        | D/A register separation (only D0–D7 allocatable); 2-operand; variable-length encoding                            |
| `wasm32`      | Stack machine — structured emission (`local.get`, `local.set`, `i32.add`, etc.)                                  |

The four byte-swap wrappers (`aarch64_be`, `armeb`, `mips64be`,
`ppc64le`) inherit their parent's design decisions verbatim; only the
ELF endianness and (where the ISA requires it) the instruction-word
byte order are flipped. See [§7](#7-big-endian-backends).

---

## 3. Register Allocation Pipeline

```
IRFunction
    │
    ▼
LiveRangeComputer::compute()     ← global position numbering (pos += 2 per instr + terminator)
    │
    ├── intervals: Vec<LiveInterval>   (start, end, use_positions, def_positions, crosses_call)
    ├── call_positions: BTreeSet<u32>  (Call + Syscall positions)
    └── coalesced_map                 (copy-related vreg merging)
    │
    ▼
TargetAgnosticRegAlloc::allocate_intervals()   ← linear-scan
    │
    ├── Sort intervals by start position (longer first at same start)
    ├── For each interval:
    │   ├── Expire old intervals (return registers to caller/callee pools)
    │   ├── Try alloc: caller-saved first, callee-saved if crosses_call
    │   └── If no free reg: spill_or_evict (lowest weight per length)
    │
    ▼
resolve_register_reuse_conflicts()   ← post-allocation verification
    │
    ├── For each instruction, check (use_vreg, def_vreg) pairs:
    │   ├── If same physical register AND use_vreg is live after → CONFLICT
    │   └── Reassign def_vreg to a different ALLOCATABLE register
    │       (checking caller_saved + callee_saved lists, not arbitrary indices)
    │       If no free register → spill def_vreg to stack slot
    │
    ▼
RegAllocResult
    ├── vreg_to_preg: HashMap<vreg, PhysicalReg>
    ├── spill_slots: HashMap<vreg, GenericSpillSlot>
    ├── spill_code: BTreeMap<pos, Vec<SpillCode>>
    ├── coalesced_map
    └── total_spill_slots
    │
    ▼
reg_isel::emit_function_regalloc_full(func, &alloc)   ← per-backend emission
    │
    ├── Prologue (SAVE/push/stmg/link — ISA-specific)
    ├── Argument shuffle (ABI arg regs → allocator-assigned regs)
    ├── Body: per-IR-instruction emission using Instruction::encode()
    ├── Spill/reload insertion at positions from alloc.spill_code
    ├── Epilogue at EVERY Return path (restore SP from FP, pop callee-saved, ret)
    ├── Branch fixup resolution (patch rel32/rel21/rel16 displacements)
    └── Re-slice AllocatedInstruction.encoded from patched all_code
```

Key data structures and the file:line where they live:

| Symbol | File:line |
|--------|-----------|
| `LiveRangeComputer` (struct) | `regalloc.rs:863` |
| `LiveRangeComputer::compute` | `regalloc.rs:896` |
| `TargetAgnosticRegAlloc` (struct) | `regalloc.rs:2899` |
| `TargetAgnosticRegAlloc::new` | `regalloc.rs:2919` |
| `TargetAgnosticRegAlloc::allocate_intervals` | `regalloc.rs:3056` |
| `resolve_register_reuse_conflicts` | `regalloc.rs:2769` |

`TargetAgnosticRegAlloc` is **target-agnostic** in the literal sense:
it does not contain a single ISA-specific constant. The per-ISA
register file (allocatable / caller-saved / callee-saved pools,
register names, register classes) is supplied at construction time by a
`TargetDesc` looked up from
`target_desc::TargetDescRegistry::get(<isa>)`. The allocator filters
`TargetDesc::registers` on `is_allocatable` / `is_callee_saved` to
derive its pools, so adding a new ISA requires only a new `TargetDesc`
entry — no allocator changes.

`resolve_register_reuse_conflicts` is the **post-allocation
verification** pass that eliminates the need for stack-slot fallbacks
on syscall-heavy functions. It detects the case where a single
instruction's `use_vreg` and `def_vreg` would land in the same physical
register and the `use_vreg` is still live afterwards — a hazard that
arises naturally on 2-operand ISAs (x86, m68k) and on instructions
where the allocator coalesced a copy. The fix is to reassign the
`def_vreg` to a different allocatable register (drawn from the
`caller_saved_gprs` + `callee_saved_gprs` lists, not from arbitrary
physical-register indices), or, if every allocatable register is
taken, to spill the `def_vreg` to a stack slot for that one
instruction. The pass runs after `allocate_intervals` and patches the
`RegAllocResult` in place before it is handed to the per-backend
emitter.

The older AArch64-specific `LinearScanAllocator` (`regalloc.rs:1284`,
`new` at `:2307`, `allocate_intervals` at `:1426`) is still on the
aarch64 path — see [§6](#6-per-backend-file-locations) for why aarch64
is the one backend that does not use `TargetAgnosticRegAlloc`.

---

## 4. How `reg_isel.rs` Works

Every register-based backend (the 14 with a `<isa>/reg_isel.rs` file)
exposes a single public entry point:

```rust
pub fn emit_function_regalloc_full(
    func: &IRFunction,
    alloc: &RegAllocResult,
) -> Result<AllocatedFunction, BackendError>;
```

The function consumes a `RegAllocResult` (produced by
`TargetAgnosticRegAlloc`) and emits register-to-register machine code
for **every** IR instruction. It is structured as seven phases:

### 4.1 Prologue (ISA-specific frame setup)

Save callee-saved registers that the allocator has assigned, save the
return address (LR / `X30` / `RA` / `LR` / `%r0+1`), set up the frame
pointer, and reserve the stack frame. The frame size is computed from
`alloc.total_spill_slots` plus the callee-saved-register count plus
any alignment padding required by the ABI (16 bytes on most ISAs,
8 on alpha/hppa/m68k/sparc64 pre-V9).

Examples:
- `x86_64` — `push rbp; mov rbp, rsp; sub rsp, frame_size` + a run of
  `push` for each callee-saved GPR the allocator touched.
- `aarch64` (via `emit.rs`) — `stp x29, x30, [sp, #-16]!; mov x29, sp;
  sub sp, sp, frame_size` + `str` for each callee-saved.
- `ppc64` — `mflr r0; std r0, 16(r1); stdu r1, -frame_size(r1)` +
  `std` for each callee-saved.
- `sparc64` — `save %sp, -frame_size, %sp` (the register-window spin).

### 4.2 Argument shuffle (ABI arg regs → allocator-assigned regs)

The calling convention places incoming arguments in a fixed set of
ABI argument registers (e.g. `RDI, RSI, RDX, RCX, R8, R9` on x86_64;
`X0–X7` on aarch64; `R2–R6` on s390x). The register allocator assigns
each parameter vreg to whatever allocatable register it picks — which
is usually *not* the ABI register. The shuffle phase emits a run of
register-to-register moves to bring each argument from its ABI
location into its allocator-assigned location.

For arguments that don't fit in registers (overflow args, large
aggregates by value), the shuffle loads them from the caller's stack
frame at `[fp + abi_offset]`.

### 4.3 Body (per-IR-instruction emission)

Walks each basic block in program order. For each `IRInstr` variant,
the backend's `Instruction::encode()` produces the bytes for that
opcode with the allocator-assigned physical registers substituted in
for the IR's virtual registers. The encoded bytes are appended to a
growing `all_code: Vec<u8>` (or `Vec<u32>` for fixed-width ISAs).

Branch and call targets that point to forward blocks are recorded as
**fixups** (see §4.6) because their displacement cannot be computed
until the target block's offset in `all_code` is known. Calls to
external symbols are recorded as **relocations** (see §4.7).

Spill/reload positions (from `alloc.spill_code`) are inserted at the
right place in the instruction stream — typically *before* the
instruction that needs the reloaded value and *after* the instruction
that defines a value to be spilled.

### 4.4 Spill/reload insertion

`alloc.spill_code` is a `BTreeMap<pos, Vec<SpillCode>>` keyed by the
global position number of the instruction at which the spill or
reload must occur. The emitter, when it reaches position `pos`, emits
the spill (`store reg, [fp + spill_off]`) or reload
(`load reg, [fp + spill_off]`) before emitting the instruction at
that position. This is the **only** place where the register-based
path touches the stack slot area; the rest of the function is
register-to-register.

### 4.5 Epilogue (at *every* Return path)

A function may have multiple `IRTerminator::Return` blocks (early
returns, error paths). The emitter inserts the epilogue at *every*
return, not just the function's final block. The epilogue:

1. Restores SP from FP (handles dynamic `Alloc` that may have moved
   SP during execution — the FP-relative frame layout is preserved).
2. Pops/restores each callee-saved register that the prologue saved.
3. Restores the return address into the ISA's link register.
4. Emits the ISA's return instruction (`ret`, `bx lr`, `jr $ra`,
   `blr`, `b ra`, `br %rp`, `rts`, `jmpi`, etc.).

### 4.6 Branch fixup resolution

After the entire function body is emitted, the emitter walks the list
of recorded fixups and patches each branch's displacement field to
point at the now-known offset of its target block in `all_code`. The
displacement width is ISA-specific:

| ISA family | Displacement width | Encoding |
|------------|--------------------|----------|
| x86_64 / x86_32 | rel32 (Jcc) / rel8 (short jmp) | 4 / 1 byte signed |
| aarch64 | rel26 (B/BL) / rel19 (B.cond) | 26 / 19 bit signed, word-scaled |
| arm32 | rel24 (B/BL) / rel8 (Bcc) | 24 / 8 bit signed, word-scaled |
| riscv64 / riscv32 | rel21 (JAL) / rel13 (branch) | 21 / 13 bit signed, word-scaled |
| mips64 | rel26 (BEQ/BNE/J) | 26 bit signed, word-scaled (delay slot) |
| ppc64 | rel24 (B/BL) / rel14 (BC) | 24 / 14 bit signed, word-scaled |
| loongarch64 | rel20 (B) / rel16 (BEQ/BNE) | 20 / 16 bit signed, word-scaled |
| s390x | rel32 (BRASL) / rel16 (BRCL) | 32 / 16 bit signed, halfword-scaled |
| sparc64 | rel22 (BPcc) / rel19 (CALL) | 22 / 19 bit signed, word-scaled (delay slot) |
| alpha | rel21 (BR) / rel23 (BSR) | 21 / 23 bit signed, word-scaled |
| hppa | rel17 (BL) / rel12 (BLR) | 17 / 12 bit signed, word-scaled |
| m68k | rel16 (JMP/JSR) / rel8 (Bcc) | 16 / 8 bit signed |

Once every fixup is patched, the emitter re-slices each
`AllocatedInstruction.encoded` field from the now-final `all_code` so
that the disassembler, debugger, and relocator see the actual emitted
bytes.

### 4.7 Relocation recording

Calls to external symbols (other functions in the same compilation
unit, libcalls, runtime trap stubs) are recorded as relocations so
that the ELF linker (or Wasm module finaliser) can patch the call
site once the symbol's final address is known. The relocation type
is ISA-specific:

| ISA | Relocation type |
|-----|-----------------|
| x86_64 | `R_X86_64_PLT32` (PC-rel32) |
| x86_32 | `R_386_PLT32` (PC-rel32) |
| aarch64 | `R_AARCH64_CALL26` (BL) / `R_AARCH64_ADR_PREL_PG_HI21` + `R_AARCH64_ADD_ABS_LO12_NC` (ADRP+ADD) |
| arm32 | `R_ARM_CALL` (BL) / `R_ARM_THM_CALL` (BL Thumb) |
| riscv64 / riscv32 | `R_RISCV_CALL_PLT` (auipc + jalr pair) |
| mips64 | `R_MIPS_26` (BAL) / `R_MIPS_HI16` + `R_MIPS_LO16` |
| ppc64 | `R_PPC64_REL24` (BL) — TOC restore handled by linker |
| loongarch64 | `R_LARCH_B26` (B/BL) |
| s390x | `R_390_PC32DBL` (BRASL) |
| sparc64 | `R_SPARC_WDISP30` (CALL) / `R_SPARC_HI22` + `R_SPARC_LO10` (SETHI+OR) |
| alpha | `R_ALPHA_BRADDR` (BR/BSR) |
| hppa | `R_PARISC_PCREL17F` (BL) — far-call fallback for >17-bit displacements |
| m68k | `R_68K_PC32` (JMP/JSR) |
| wasm32 | `R_WASM_TABLE_INDEX` (call_indirect function table) |

---

## 5. `contains_fork` Opt-Out (clone/fork detection)

The `contains_fork` opt-out is the **one and only** situation in
which the register-based path is bypassed. It is a **correctness
requirement**, not a fallback for register-pressure problems or
unimplemented IR ops.

### 5.1 The hazard

A `clone(2)` (Linux syscall nr=220 on aarch64) or `vfork(2)` (nr=221)
creates a child process whose register state diverges from the
parent's at the syscall return. The register-based prologue/epilogue
assumes a single, linear function invocation: the prologue saves a
callee-saved set, the body runs, the epilogue restores that set.
After `clone`, the child returns from the syscall with the parent's
callee-saved set already saved in the prologue — but the child may
then take a completely different code path that doesn't restore them
correctly, leading to corrupted callee-saved state in the child.

### 5.2 The detection

Every register-based backend's `allocate_registers` computes a
`contains_fork: bool` over the IR function before deciding which
emitter to call:

```rust
let contains_fork = func.blocks.iter().any(|block| {
    block.instructions.iter().any(|inst| {
        match inst {
            crate::ir::IRInstr::Call { func: fname, .. } => {
                fname == "spawn_worker" || fname == "fork"
            }
            // Generic clone/vfork numbers used by the IR before
            // syscall_abi::translate resolves them to the per-ISA
            // native number.
            crate::ir::IRInstr::Syscall { nr, .. } => *nr == 220 || *nr == 221,
            _ => false,
        }
    })
});
```

`spawn_worker` is the IPC-level name for `clone` lowered by
`expand_spawn_worker` (`ipc_lowering.rs`) to `Syscall{nr: 220, ...}`.
By the time `allocate_registers` runs, the `Call{func: "spawn_worker"}`
may have been replaced by the lowered `Syscall`, so the detection
catches both forms.

### 5.3 The dispatch

```rust
let code = if real_regalloc && !contains_fork {
    // Default path: register-based emission.
    if let Some(ar) = try_real_regalloc(func) {
        if let Ok(full) = reg_isel::emit_function_regalloc_full(func, &ar) {
            return Ok(full);
        }
    }
    // …fall through to stack-slot path only if the allocator or
    // emitter itself errored (never on the happy path).
    stack_slot_isel::allocate_registers(func)
} else if real_regalloc && contains_fork {
    // Correctness opt-out: function contains clone/fork. Take the
    // stack-slot path because the register-based prologue/epilogue
    // doesn't interact correctly with fork().
    stack_slot_isel::allocate_registers(func)
} else {
    // Env-var opt-out (VUMA_REAL_REGALLOC_<ISA>=0) for debugging.
    stack_slot_isel::allocate_registers(func)
};
```

The `contains_fork` arm runs the legacy stack-slot ISel because the
stack-slot path doesn't have the callee-saved prologue/epilogue
hazard — every vreg lives in its own stack slot, so the child's
divergent register state is irrelevant.

### 5.4 Why this is not "the production path"

The stack-slot path is **only** taken for functions that contain a
`clone`/`fork` syscall. For the overwhelming majority of compiled
functions (no fork), the register-based `reg_isel.rs` is the
production emission path. The `contains_fork` opt-out exists for a
specific, narrow correctness reason and is not a generic fallback
for register pressure, unimplemented IR ops, or allocator failure.

### 5.5 `contains_fork` on `wasm32`

`wasm32` computes `contains_fork` for parity with the other backends
(`wasm32/mod.rs:4632-4676`), but the boolean is **purely
observational** — wasm32 is a stack machine with no register-based
emitter to fall back from, so its single `lower_function` path runs
regardless. The check exists so that downstream tooling (debug logs,
audit reports, future fork-emulation hooks) can observe that a
function contains a clone. The actual fork emulation on wasm32 is
handled separately by `wasm32_fork_emulation_pass`
(`ipc_lowering.rs:232`), which rewrites the child branch to run
in-process (see [§8](#8-wasm32-special-handling)).

### 5.6 Env-var gate

Each backend reads `VUMA_REAL_REGALLOC_<ISA>` (e.g.
`VUMA_REAL_REGALLOC_AARCH64`, `VUMA_REAL_REGALLOC_X86_64`,
`VUMA_REAL_REGALLOC_PPC64`, `VUMA_REAL_REGALLOC_RISCV64`, …) with a
default of **ON** (`unwrap_or(true)`). Setting the env var to `0`
forces the stack-slot path for debugging — this is independent of
`contains_fork` and exists for bisecting allocator bugs.

| Backend       | Env var                         | Default |
|---------------|---------------------------------|---------|
| `x86_64`      | `VUMA_REAL_REGALLOC_X86_64`     | ON      |
| `x86_32`      | `VUMA_REAL_REGALLOC_X86_32`     | ON      |
| `aarch64`     | `VUMA_REAL_REGALLOC_AARCH64`    | ON      |
| `arm32`       | `VUMA_REAL_REGALLOC_ARM32`      | ON      |
| `riscv64`     | `VUMA_REAL_REGALLOC_RISCV64`    | ON      |
| `riscv32`     | `VUMA_REAL_REGALLOC_RISCV32`    | ON      |
| `mips64`      | `VUMA_REAL_REGALLOC_MIPS64`     | ON      |
| `ppc64`       | `VUMA_REAL_REGALLOC_PPC64`      | ON      |
| `loongarch64` | `VUMA_REAL_REGALLOC_LOONGARCH64`| ON      |
| `s390x`       | `VUMA_REAL_REGALLOC_S390X`      | ON      |
| `sparc64`     | `VUMA_REAL_REGALLOC_SPARC64`    | ON      |
| `alpha`       | `VUMA_REAL_REGALLOC_ALPHA`      | ON      |
| `hppa`        | `VUMA_REAL_REGALLOC_HPPA`       | ON      |
| `m68k`        | `VUMA_REAL_REGALLOC_M68K`       | ON      |

The 4 byte-swap wrappers do not have their own env-var gate — they
inherit the parent's allocation result via one-line delegation, so
the parent's env-var governs both endianness variants.

---

## 6. Per-Backend File Locations

All 14 native register-based backends live in a per-backend
**directory** under `src/codegen/src/<isa>/` containing at minimum
`mod.rs` (the `Backend` impl, `TargetInfo`, `allocate_registers`
driver) and `reg_isel.rs` (the `emit_function_regalloc_full`
emitter). Most also have `disasm.rs` (disassembler) and some retain
`stack_slot_isel.rs` for the `contains_fork` opt-out path.

### 6.1 Directory-style backends (all 14 native register-based emitters)

| Backend       | Directory                          | Files in directory                                |
|---------------|------------------------------------|---------------------------------------------------|
| `aarch64`     | `src/codegen/src/aarch64/`         | `mod.rs`, `reg_isel.rs`                           |
| `x86_64`      | `src/codegen/src/x86_64/`          | `mod.rs`, `reg_isel.rs`, `disasm.rs`, `stack_slot_isel.rs` |
| `x86_32`      | `src/codegen/src/x86_32/`          | `mod.rs`, `reg_isel.rs`, `disasm.rs`, `stack_slot_isel.rs` |
| `arm32`       | `src/codegen/src/arm32/`           | `mod.rs`, `reg_isel.rs`, `disasm.rs`              |
| `riscv64`     | `src/codegen/src/riscv64/`         | `mod.rs`, `reg_isel.rs`                           |
| `riscv32`     | `src/codegen/src/riscv32/`         | `mod.rs`, `reg_isel.rs`                           |
| `mips64`      | `src/codegen/src/mips64/`          | `mod.rs`, `reg_isel.rs`, `disasm.rs`              |
| `ppc64`       | `src/codegen/src/ppc64/`           | `mod.rs`, `reg_isel.rs`, `disasm.rs`              |
| `loongarch64` | `src/codegen/src/loongarch64/`     | `mod.rs`, `reg_isel.rs`, `disasm.rs`, `stack_slot_isel.rs`, `reg_alloc_isel.rs`† |
| `s390x`       | `src/codegen/src/s390x/`           | `mod.rs`, `reg_isel.rs`                           |
| `sparc64`     | `src/codegen/src/sparc64/`         | `mod.rs`, `reg_isel.rs`                           |
| `alpha`       | `src/codegen/src/alpha/`           | `mod.rs`, `reg_isel.rs`                           |
| `hppa`        | `src/codegen/src/hppa/`            | `mod.rs`, `reg_isel.rs`                           |
| `m68k`        | `src/codegen/src/m68k/`            | `mod.rs`, `reg_isel.rs`                           |

† `loongarch64/reg_alloc_isel.rs` is **dead code** — the module
declaration is commented out at `loongarch64/mod.rs` and the production
`allocate_registers` calls `reg_isel::emit_function_regalloc_full`
through `try_real_regalloc`. The file is retained for historical
reference; it is not compiled.

### 6.2 `aarch64` — directory-style since W7-impl

Since the W7-impl wave, `aarch64` follows the same directory-style
pattern as the other 13 native backends, with its own
`aarch64/reg_isel.rs` exposing `emit_function_regalloc_full`. The
`AArch64Backend` struct, `TargetInfo` impl, and `allocate_registers`
driver live in `src/codegen/src/backend.rs` (the `allocate_registers`
driver is at `:3162`; the `contains_fork` check at `:3230`; the
`try_real_regalloc` + `reg_isel` dispatch at `:3358`).

The older `LinearScanAllocator` (`regalloc.rs:1284`) +
`Emitter::emit_function_regalloc` (`emit.rs:1056`) path survives as a
**fallback only** — invoked when `aarch64::reg_isel::emit_function_regalloc_full`
returns an error (e.g. for an unimplemented IR instruction). This mirrors
the fallback pattern in every other native backend's `allocate_registers`
driver.

### 6.3 The 4 byte-swap wrappers (directory-style)

Each wrapper is a directory under `src/codegen/src/` with a `mod.rs`
and a `reg_isel.rs` that re-exports the parent's emitter:

| Backend       | File                                  | Wraps            | `allocate_registers` delegation line |
|---------------|---------------------------------------|------------------|--------------------------------------|
| `aarch64_be`  | `src/codegen/src/aarch64_be/mod.rs`   | `AArch64Backend` | `:153-155` (one-line `self.inner.allocate_registers(func)`) |
| `armeb`       | `src/codegen/src/armeb/mod.rs`        | `Arm32Backend`   | `:188-190`                           |
| `mips64be`    | `src/codegen/src/mips64be/mod.rs`     | `Mips64Backend`  | `:203-205`                           |
| `ppc64le`     | `src/codegen/src/ppc64le/mod.rs`      | `PPC64Backend`   | `:403-409`                           |

"One-line" delegation means the wrapper's `allocate_registers` body is
literally `self.inner.allocate_registers(func)` — the wrapper adds no
register-allocation logic of its own. The wrapper's job is to
byte-swap the parent's emitted bytes and ELF header to the target
endianness (see [§7](#7-big-endian-backends)).

### 6.4 `wasm32` (single directory, no `reg_isel.rs`)

| Backend  | Directory                       | Files in directory                  |
|----------|---------------------------------|-------------------------------------|
| `wasm32` | `src/codegen/src/wasm32/`       | `mod.rs`, `disasm.rs`               |

`wasm32` has no `reg_isel.rs` because WebAssembly is a stack machine
— there are no physical registers to allocate. The backend's
`allocate_registers` is the IR-to-Wasm-bytecode lowering
(`lower_function`), which maps each vreg to a Wasm `local` and emits
structured stack-machine code (`local.get`, `i32.add`, `i32.store`,
`call`, `br_table`, etc.). See [§8](#8-wasm32-special-handling).

### 6.5 Shared infrastructure files

| File                              | Role                                                              |
|-----------------------------------|-------------------------------------------------------------------|
| `src/codegen/src/backend.rs`      | `Backend` trait, `BackendKind` enum, `aarch64`'s backend impl, decode/disasm helpers, ELF builders. |
| `src/codegen/src/regalloc.rs`     | `LiveRangeComputer`, `TargetAgnosticRegAlloc`, `LinearScanAllocator` (aarch64), `resolve_register_reuse_conflicts`, `RegAllocResult`, `SpillCode`, `verify_callee_saved`. |
| `src/codegen/src/regalloc_emit.rs`| Helpers for wiring a `RegAllocResult` into an `AllocatedFunction`. |
| `src/codegen/src/emit.rs`         | `Emitter` — the aarch64 register-based emitter (also provides `emit_function_regalloc` plumbing). |
| `src/codegen/src/target_desc.rs`  | `TargetDesc`, `TargetDescRegistry` — per-ISA register file metadata consumed by `TargetAgnosticRegAlloc`. |
| `src/codegen/src/syscall_abi.rs`  | `translate_or_warn(backend, generic_nr) -> u32` — asm-generic → per-ISA syscall number. |
| `src/codegen/src/riscv_common.rs` | Shared RISC-V opcode tables consumed by both `riscv64` and `riscv32`. |
| `src/codegen/src/ipc_lowering.rs` | `expand_spawn_worker` (lowers `spawn_worker`/`fork` to `Syscall{nr: 220}`), `wasm32_fork_emulation_pass`. |

---

## 7. Big-Endian Backends

VUMA ships four big-endian backends. Three are thin wrappers around a
little-endian parent; `ppc64` is natively big-endian. The fourth
wrapper (`ppc64le`) wraps a big-endian parent (`ppc64`) and produces
little-endian output.

### 7.1 Wrapper backends — byte-swap policy matrix

| #  | Wrapper      | Wraps                   | Instruction byte-swap                                 | ELF header swap                            | `allocate_registers` delegation   |
|---:|--------------|-------------------------|-------------------------------------------------------|--------------------------------------------|-----------------------------------|
|  4 | `aarch64_be` | `AArch64Backend::new`   | **None** (ARM ARM D6.1.3 — instr. fetches always LE)  | `swap_le_elf_to_be` (header/PHDR/SHDR)     | one-line (`aarch64_be/mod.rs:153-155`)  |
|  6 | `armeb`      | `Arm32Backend::new`     | **LE→BE** (BE32 mode, every 4-byte instr. word)       | `swap_le_elf32_to_be` (header + instr.)    | one-line (`armeb/mod.rs:188-190`)       |
| 10 | `mips64be`   | `Mips64Backend::new_be` | **LE→BE** (instr. words only — parent already BE hdr) | None (parent already BE on header)         | one-line (`mips64be/mod.rs:203-205`)    |
| 12 | `ppc64le`    | `PPC64Backend::new_le`  | **BE→LE** (instr. words + `EI_DATA` `MSB→LSB` + hdr)  | `swap_be_elf_to_le` (full)                 | one-line (`ppc64le/mod.rs:403-409`)     |

### 7.2 `aarch64_be` — ELF-only swap, instructions stay LE

Per ARM ARM DDI 0487 §D6.1.3, AArch64 instruction fetches are always
little-endian regardless of `PSTATE.E`. `aarch64_be/mod.rs` therefore
forwards the parent's instruction bytes unchanged and only swaps the
ELF header/PHDR/SHDR fields via `swap_le_elf_to_be`.

### 7.3 `armeb` — BE32 word-swap wrapper

ARMv7 BE32 mode requires each 4-byte instruction word stored
big-endian. `armeb/mod.rs` byte-swaps every 4-byte instruction word
LE→BE inside `encode_function`, `return_stub`, `trampoline`, and the
executable `PT_LOAD` segment.

### 7.4 `mips64be` — instruction word swap, native BE ELF

The parent `mips64` backend emits a big-endian ELF header natively
(`build_mips64_elf_2seg`), so the wrapper only swaps the 32-bit
instruction words in the `PT_LOAD` segment from LE to BE.

### 7.5 `ppc64` — native big-endian

`ppc64/mod.rs` is implemented natively as a big-endian backend
(ELFv2 ABI, `ELFDATA2MSB`). All encoders write 4-byte big-endian
words directly. The `ppc64le` wrapper inherits `ppc64`'s encoders
unchanged and flips the ELF header endianness back to LE.

---

## 8. `wasm32` Special Handling

`wasm32/mod.rs` is structurally different from every other backend
because WebAssembly requires **structured control flow** (no
arbitrary jumps). Four design decisions stand out:

### 8.1 Trampoline loop

All IR basic blocks are nested inside a single
`(loop $trampoline (block $b_outer ... (block $b_inner (br_table
$b_inner ... $b_outer $trampoline))))`. A `local $pc:i32` is updated
at every terminator; `br_table` dispatches to the right nested
block. `Break`/`Continue` map to `br` at the appropriate depth.

This is **architectural**, not a QEMU bug — WebAssembly's structured
control flow permits no arbitrary jump-to-label, and VUMA's IR is a
basic-block CFG with arbitrary successor edges, so the trampoline
emulates a computed goto. Works on every Wasm runtime (wasmtime
47.0.2, wasmer, node.js). Performance cost: one `local.set $pc` +
one `br $trampoline` + one `br_table` dispatch per branch. **No
removal condition** — fundamental IR↔Wasm impedance mismatch.

### 8.2 Ring-buffer channels

`channel_open` lowers to a heap-allocated 8-byte buffer holding
`{read_fd, write_fd}`. On wasm32 there is no `pipe2` syscall; the
runner (`scripts/wasm32_runner.py`) provides host-side `fdio`
functions backed by a ring buffer in host memory.

### 8.3 Fork emulation (in-process, no isolation)

`vuma_fork` cannot `os.fork` because wasmtime runs background threads
that break the child's state. Instead, the
`wasm32_fork_emulation_pass` (`ipc_lowering.rs:232`) rewrites the
child branch's `Return` to `Store(exit_val, 4096); Jump(parent_post_block)`
and rewrites `wait_worker` to `Load(4096)`.
`WASM32_CHILD_EXIT_ADDR = 4096`.

The parent and child branches therefore run **sequentially in the
same Wasm process**, with **no isolation** between them: the child
can read and write the parent's memory, and a crash in the child
crashes the parent. This is a deliberate design trade-off — wasm32
has no process primitive, and the in-process emulation is sufficient
for the IPC test matrix. The wasm32 child-branch code is dead in
the emitted binary because the rewriter replaces the child's first
`Return` with a `Jump` back to the parent's post-fork block.

### 8.4 Function table for `CallIndirect`

`IRInstr::CallIndirect` lowers to `WasmInstr::CallIndirect`. Each
`GetAddress` of a function emits a table-index relocation; at module
finalisation, the function table is built and the relocations are
patched.

---

## 9. QEMU Execution Notes

VUMA backends are tested under QEMU user-mode emulation (or wasmtime
for `wasm32`). The QEMU binary name does not always match the VUMA
backend name; the canonical mapping lives in
`scripts/vuma_test_matrix_19backends.sh` (function `qemu_for()`) and
`scripts/qemu_smoke_test.sh` (associative array `QEMU_BIN`).

### 9.1 Per-backend QEMU binary

| #  | Backend        | Emulator binary             | Notes                                                                                              |
|---:|----------------|-----------------------------|----------------------------------------------------------------------------------------------------|
|  1 | `x86_64`       | (native, no emulator)       | Runs natively on x86_64 hosts.                                                                     |
|  2 | `x86_32`       | `qemu-i386-static`          | 32-bit x86 user-mode.                                                                              |
|  3 | `aarch64`      | `qemu-aarch64-static`       |                                                                                                    |
|  4 | `aarch64_be`   | `qemu-aarch64_be-static`    | Big-endian AArch64 variant; QEMU recognises the BE ELF header.                                     |
|  5 | `arm32`        | `qemu-arm-static`           | **Naming mismatch**: VUMA calls it `arm32`, QEMU calls the binary `qemu-arm`.                      |
|  6 | `armeb`        | `qemu-armeb-static`         | Big-endian ARMv7 variant.                                                                          |
|  7 | `riscv64`      | `qemu-riscv64-static`       |                                                                                                    |
|  8 | `riscv32`      | `qemu-riscv32-static -cpu max` | **`-cpu max` required**: QEMU's default rv32 CPU lacks the D (double-float) extension.          |
|  9 | `mips64`       | `qemu-mips64el-static`      | **Naming mismatch**: VUMA's `mips64` emits a *little-endian* ELF, so the LE emulator is required. `qemu-mips64-static` (BE) rejects it. |
| 10 | `mips64be`     | `qemu-mips64-static`        | Big-endian MIPS64; the BE emulator matches the BE ELF.                                             |
| 11 | `ppc64`        | `qemu-ppc64-static`         |                                                                                                    |
| 12 | `ppc64le`      | `qemu-ppc64le-static`       | Little-endian PowerPC variant.                                                                     |
| 13 | `loongarch64`  | `qemu-loongarch64-static`   |                                                                                                    |
| 14 | `s390x`        | `qemu-s390x-static`         |                                                                                                    |
| 15 | `sparc64`      | `qemu-sparc64-static`       |                                                                                                    |
| 16 | `alpha`        | `qemu-alpha-static`         | Requires QEMU ≥ 10.0 (alpha support was incomplete in 7.x).                                        |
| 17 | `hppa`         | `qemu-hppa-static`          |                                                                                                    |
| 18 | `m68k`         | `qemu-m68k-static`          |                                                                                                    |
| 19 | `wasm32`       | `wasmtime` (≥ 47.0.2)       | Not QEMU — `wasm32` runs under the Bytecode Alliance wasmtime runtime.                             |

### 9.2 Minimum QEMU versions

| ISA family                           | Minimum QEMU | Recommended | Notes                                                                          |
|--------------------------------------|-------------:|------------:|--------------------------------------------------------------------------------|
| `aarch64` / `aarch64_be`             | 7.2          | 10.x        |                                                                                |
| `arm32` / `armeb`                    | 7.2          | 10.x        |                                                                                |
| `x86_64` / `x86_32`                  | 7.2          | 10.x        |                                                                                |
| `riscv64`                            | 7.2          | 10.x        | Uses default CPU; no `-cpu max` needed.                                        |
| `riscv32`                            | 7.2 + `-cpu max` | 10.x + `-cpu max` | D extension requires `-cpu max`.                                          |
| `loongarch64`                        | 7.2          | 10.x        |                                                                                |
| `mips64`                             | 7.2          | 10.x        | Use `qemu-mips64el-static` (LE).                                               |
| `mips64be`                           | 7.2          | 10.x        | Use `qemu-mips64-static` (BE).                                                 |
| `ppc64` / `ppc64le`                  | 7.2          | 10.x        |                                                                                |
| `sparc64`                            | 7.2          | 10.x        |                                                                                |
| `s390x`                              | 7.2          | 10.x        |                                                                                |
| `m68k`                               | 7.2          | 8.x+        | QEMU 7.2 m68k has known translator bugs that VUMA works around; 8.x removes them. |
| `alpha`                              | 10.0         | 11.x        | QEMU 10.0-alpha rejects `CMPULE` (function 0x3D); VUMA emulates via `CMPULT`. Removal: QEMU 11.x. |
| `hppa`                               | 7.2          | 8.x+        | QEMU 7.2 hppa has an `LDIL` decoder bug VUMA works around; 8.x removes it.     |
| `wasm32`                             | wasmtime 47.0.2 | wasmtime 47+ | Trampoline loop is architectural, not a runtime bug.                          |

### 9.3 Smoke runner

`scripts/qemu_smoke_test.sh` builds the release `vuma` binary once
and compiles a small set of gold-standard `.vuma` programs on every
supported backend (12 QEMU + wasm32 via wasmtime), running each
under the appropriate emulator and checking the exit code against
the `// Expected exit code:` header. The per-ISA QEMU/wasmtime
binary mapping lives in the `QEMU_BIN` associative array at the top
of the script. The full 19-backend matrix is driven by
`scripts/vuma_test_matrix_19backends.sh`.

---

## 10. Syscall ABI Translation

VUMA IR uses **asm-generic** (Linux generic syscall) numbers
internally. Each backend translates to its native numbering via
`syscall_abi::translate_or_warn(backend, generic_nr) -> u32`
(`syscall_abi.rs:281-300`).

**Identity arches** (no translation): `aarch64`, `riscv64`,
`riscv32`, `loongarch64`, `arm32`, `wasm32`. These return the input
verbatim.

**Translated arches**: `x86_64` (`syscall_abi.rs:304`), `x86_32`
(`:445`), `mips64` (`:583`), `ppc64` (`:728`), `s390x` (`:870`),
`sparc64` (`:1013`), `alpha` (`:1153`), `hppa` (`:1293`), `m68k`.
The MIPS, PPC, s390x, sparc64, alpha, and hppa tables differ
significantly from asm-generic (e.g. s390x `read`=3 and MIPS
`read`=5000 vs asm-generic `read`=63).

**Warning behaviour**: if `translate(backend, generic_nr)` returns
`None` (unknown syscall), `translate_or_warn` logs a
`vuma_log!(warn, ...)` and returns the generic number verbatim
(`syscall_abi.rs:291-298`). This is non-fatal: the program is still
emitted, and the syscall may be wrong on the target arch.

**Production callers**: 16 of 19 backends call `translate_or_warn`
directly (arm32, aarch64, alpha, x86_32, riscv32, riscv64, hppa,
wasm32, ppc64, mips64, loongarch64, m68k, s390x, sparc64, x86_64).
The four wrapper backends inherit the parent's call.

---

## 11. Runtime Trap Stubs

Every backend emits three named syscall-stub symbols that implement
the runtime side of the PMT safety invariants. The exit codes match
the Lean `TrapCode.to_exit` mapping (`proof/PMT/Soundness.lean:162-171`):

| Runtime stub (emitted by every backend)                | Exit code | Lean `TrapCode` constructor |
|--------------------------------------------------------|----------:|-----------------------------|
| `__arena_overflow` (`x86_64/mod.rs:3648-3654`, 18 siblings) |   1 | `TrapCode.arena_overflow` |
| `__oob_trap`       (`x86_64/mod.rs:3657-3666`, 18 siblings) | 134 | `TrapCode.oob`            |
| `__uaf_trap`       (`x86_64/mod.rs:3669-3679`, 18 siblings) | 135 | `TrapCode.uaf`            |

Each of the 19 backends emits its own copy of each stub
(19 × 3 = 57 stub definitions). There is **no Rust `TrapCode` enum**
— `TrapCode` is Lean-only; the runtime uses named exit-code stubs.

---

## 12. Cross-references

- [Architecture overview](./architecture.md) — 10-stage pipeline, IVE
  with Z3, two-pipe IPC, register allocation, formal verification scope.
- [Pipeline](./pipeline.md) — stage-by-stage compilation walkthrough.
- [Caveats](./caveats.md) — documented surprises for backend
  developers, each carrying a resolution-status annotation
  (`RESOLVED` / `PARTIALLY RESOLVED` / `STALE` / `OPEN`).
- [Testing](./testing.md) — gold-standard harness, CI, KATs, test
  matrix.
- [Building](./building.md) — prerequisites (including `libz3-dev`),
  quick start, troubleshooting.
