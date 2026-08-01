# VUMA — Verified-Unsafe Memory Access

**Version 0.2.0-alpha.10**

VUMA is a statically-typed systems programming language and compiler framework
whose distinguishing feature is **behavioral contract verification at compile
time** — discharged by a hard-wired Z3 SMT solver — augmented by **mandatory
runtime bounds checks on arena-allocated accesses**. Bounds checks are always
on; the runtime memory-safety overhead they incur is intentionally **paid**,
not avoided. Source programs are parsed to an AST, lifted to a Semantic Code
Graph (SCG), verified by the Invariant Verification Engine (IVE) — which
emits a `discharge_rate=N%` summary — lowered to a backend-neutral IR,
optimized, register-allocated, and emitted as native object code (ELF) or
WebAssembly.

The compiler targets **19 production backends**. All 19 pass the curated
30-test matrix (76/76 on the 4-test spot check, 30/30 on the full matrix for
the primary backends). **18 of 19 backends use full register-based emission**
(`reg_isel.rs`) with a target-agnostic linear-scan register allocator — no
stack-slot fallbacks. Of these, 14 are native backends with their own
`reg_isel.rs`; 4 byte-swap wrappers (aarch64_be, armeb, mips64be, ppc64le)
inherit their parent's register-based emitter. The 19th backend, wasm32,
uses structured stack-machine emission (the correct architecture for a
stack machine, not a fallback).

---

## 1. Key Metrics

| Metric | Value |
|--------------------------------|------------------------------------|
| Backends | 19 (all pass curated test matrix) |
| Register-based emission | 18 backends (14 native + 4 wrappers) |
| Register allocator | Target-agnostic linear-scan with post-allocation conflict resolution |
| Contract / invariant discharge | Z3 SMT solver (hard dependency) |
| Implementation language | Rust (nightly-2026-03-01) |
| License | MIT |
| Minimum Rust toolchain | nightly-2026-03-01 (rustc 1.87+) |

---

## 2. Feature List

### Language
- **`transform` is the only function keyword.** A VUMA program is a
  composition of named transforms over typed state.
- **`Result` + `?` operator** for error handling — no exceptions, no panics
  on the happy path.
- **`requires` / `ensures` contracts** on every transform, discharged by Z3
  at compile time.
- **`prove` blocks** for inline assertions that Z3 must discharge before
  code emission.
- **`#[secret]` labels** drive a real information-flow verifier (not a
  stub): the lattice check operates over real vregs, not source names.
- **Session types** are checked by a real session-type verifier that tracks
  real vregs across channel operations.

### Memory model
- **PMT (Programs as Memory Transformations)** — every program is a typed
  state-transformation over a single backing arena.
- **Mandatory runtime bounds checks** on every arena-allocated access
  (`UGe` against the arena length, trap via `__oob_trap`, exit code 134).
  The overhead is paid; raw-pointer arithmetic and `length_expr=None`
  accesses remain unchecked.
- **Two-pipe channel architecture** with a handle registry: one pipe for
  data, one for protocol continuation; the registry maps opaque handles to
  live channel state.

### Verification (IVE)
- **Z3 SMT solver is a hard dependency.** Contracts, `prove` blocks,
  session-type linearity, information-flow lattice, and PMT invariants are
  all discharged by Z3 — no Lean FFI bridge, no `sorry` stubs.
- **Hand-written Rust verifiers** drive Z3 directly. The verifiers emit
  SMT constraints, call Z3, and report discharge status per obligation.
- **IVE output reports `discharge_rate=N%`** — the fraction of obligations
  Z3 discharged automatically.
- **Proof-directed compilation**: the `LinearityReport` from the
  session-type verifier is wired into codegen — non-linear programs are
  rejected before register allocation.

### Codegen
- **Full register-based emission** on 18 backends (14 native backends with
  their own `reg_isel.rs` — aarch64, x86_64, x86_32, riscv64, riscv32,
  arm32, mips64, ppc64, loongarch64, sparc64, s390x, m68k, alpha, hppa —
  plus 4 byte-swap wrappers aarch64_be, armeb, mips64be, ppc64le that
  delegate to a parent's `reg_isel.rs`). Each `reg_isel.rs` consumes the
  `RegAllocResult` from the target-agnostic linear-scan allocator and
  produces register-to-register machine code for ALL IR instructions.
- **Target-agnostic linear-scan register allocator** (`regalloc.rs`) with:
  - Live-range computation across all blocks (global position numbering)
  - Caller-saved / callee-saved register pools
  - Spill with eviction (lowest-weight-first)
  - **Post-allocation conflict resolution** (`resolve_register_reuse_conflicts`)
    that detects and fixes cases where a used vreg and a defined vreg share
    a physical register at the same instruction — the defined vreg is
    reassigned to a different allocatable register or spilled. This
    eliminates the need for stack-slot fallbacks on syscall-heavy functions.
- **4 inherited backends** (aarch64_be, armeb, mips64be, ppc64le) delegate
  to their parent's `allocate_registers` and byte-swap the output.
- **wasm32** uses structured stack-machine emission (not a register-based
  emitter) — this is the correct architecture for WebAssembly, not a
  fallback.
- **`contains_fork` opt-out**: functions containing `clone`/`fork` syscalls
  (nr=220/221, or native equivalents) fall back to stack-slot ISel. This is
  a **correctness requirement** (child process has different register
  state), not a shortcut.

---

## 3. Architecture

VUMA is organized as a 10-stage pipeline — **parse → AST → SCG → IVE → IR →
channel-lowering → opt → regalloc → backend → ELF/Wasm** — implemented as a
Cargo workspace of Rust crates.

```
        ┌─────────┐   ┌─────┐   ┌─────┐   ┌─────┐   ┌────┐
source →│ parser  │→ │ AST │ → │ SCG │ → │ IVE │ → │ IR │ → …
        └─────────┘   └─────┘   └─────┘   └──┬──┘   └────┘
                                            │
                                  ┌─────────┴─────────┐
                                  │ Z3 (hard dep)     │
                                  │ Rust verifiers    │
                                  │ discharge_rate=N% │
                                  └───────────────────┘
```

### Workspace crates

| Crate | Role |
|----------------|------------------------------------------------------|
| `vuma-parser` | Lexer + parser, source → AST |
| `vuma-scg` | AST → Semantic Code Graph |
| `vuma-ive` | Invariant Verification Engine; Z3 discharge; `discharge_rate` |
| `vuma-core` | IR, optimizer, channel lowering, handle registry |
| `vuma-codegen` | Regalloc, ISel, ELF/Wasm emission, runtime PMT checker |
| `vuma-bd` | Behavior descriptor / contract metadata |
| `vuma-package` | Package manager |
| `vuma-tests` | Gold-standard harness, KAT vectors, parity tests |

### Register allocation pipeline

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

---

## 4. Quick Start

### 4.1 Build

Prerequisites:

- **Rust nightly-2026-03-01** (rustc 1.87+):
  `rustup toolchain install nightly-2026-03-01`
- **Z3** ≥ 4.12 (hard dependency). Install via `apt install libz3-dev` or
  download from <https://github.com/Z3Prover/z3>.
- **QEMU user-mode** (for cross-arch execution): `apt install qemu-user`
- **wasmtime** ≥ 47.0 (for wasm32 backend): download from
  <https://github.com/bytecodealliance/wasmtime/releases>

```bash
git clone https://github.com/pkhairkh/vuma
cd vuma
cargo build --release --bin compile_dump --bin dump_ir
# Binaries land in target/release/
```

### 4.2 Compile and run a test program

```bash
# Compile a VUMA source to an x86_64 ELF executable, then run it natively.
./target/release/compile_dump examples/fibonacci.vuma out.bin x86_64 --no-verify
./out.bin; echo "exit=$?"

# Cross-compile to aarch64 and run under QEMU.
./target/release/compile_dump examples/fibonacci.vuma out.arm64 aarch64 --no-verify
qemu-aarch64 ./out.arm64; echo "exit=$?"

# Compile to wasm32 and run under wasmtime.
./target/release/compile_dump examples/fibonacci.vuma out.wasm wasm32 --no-verify
wasmtime out.wasm; echo "exit=$?"
```

The `--no-verify` flag skips IVE PMT verification for quick testing.
Remove it for full contract/invariant discharge.

### 4.3 Run the curated test matrix

```bash
# 30 curated tests across all 19 backends.
# Each test compiles a .vuma source and checks the exit code.
bash scripts/vuma_test_matrix_19backends.sh
```

---

## 5. Backend Matrix

All 19 backends pass the curated test matrix.

| # | Backend | ISA | Endian | Emission | Inherited | Notes |
|---|---------|-----|--------|----------|-----------|-------|
| 1 | `x86_64` | x86-64 | LE | register-based | — | Native on x86_64 hosts |
| 2 | `x86_32` | i386 | LE | register-based | — | Runs via qemu-i386 on x86_64 |
| 3 | `aarch64` | A64 | LE | register-based | — | |
| 4 | `aarch64_be` | A64 | BE | register-based | aarch64 | Byte-swap wrapper |
| 5 | `arm32` | ARMv7-A | LE | register-based | — | |
| 6 | `armeb` | ARMv7-A | BE | register-based | arm32 | Byte-swap wrapper |
| 7 | `riscv64` | RV64GC | LE | register-based | — | |
| 8 | `riscv32` | RV32GC | LE | register-based | — | |
| 9 | `mips64` | MIPS64 | LE | register-based | — | Emits LE ELF; run via qemu-mips64el |
| 10 | `mips64be` | MIPS64 | BE | register-based | mips64 | Byte-swap wrapper |
| 11 | `ppc64` | Power ISA v3 | BE | register-based | — | |
| 12 | `ppc64le` | Power ISA v3 | LE | register-based | ppc64 | Byte-swap wrapper |
| 13 | `loongarch64` | LoongArch | LE | register-based | — | |
| 14 | `s390x` | z/Arch | BE | register-based | — | |
| 15 | `sparc64` | SPARC V9 | BE | register-based | — | Register windows (SAVE/RESTORE) |
| 16 | `alpha` | Alpha 21064 | LE | register-based | — | |
| 17 | `hppa` | PA-RISC 1.1 | BE | register-based | — | |
| 18 | `m68k` | Motorola 68k | BE | register-based | — | D/A register separation |
| 19 | `wasm32` | WebAssembly | LE | stack-machine | — | Structured stack emission (not register-based) |

### Register-based emitter architecture

Each register-based backend has a `reg_isel.rs` module with:
- `emit_function_regalloc_full(func, alloc)` — the entry point
- Prologue: save callee-saved registers + LR + FP, set up frame pointer
- Argument shuffle: move args from ABI registers to allocator-assigned registers
- Body: emit register-to-register machine code for each IR instruction
- Spill/reload: insert save/restore at positions from `alloc.spill_code`
- Epilogue: restore SP from FP (handles dynamic Alloc), restore callee-saved, return
- Branch fixup: patch relative displacements after all code is emitted
- Relocation recording: for calls (R_X86_64_PLT32, R_RISCV_JAL, etc.)

### Per-backend ISA-specific design decisions

| Backend | Key design points |
|---------|-------------------|
| x86_64 | R11 not_allocatable (scratch for immediates); 2-operand constraint; REX prefix handling |
| x86_32 | Same as x86_64 but 32-bit; args on stack; int 0x80 syscall |
| aarch64 | X15 scratch; CSEL for conditional moves; AAPCS64 calling convention |
| arm32 | Conditional execution (MOVcc); no hardware divide; R12 scratch; PUSH/POP multiple |
| riscv64 | T5/T6 not_allocatable (scratch); 3-operand; no delay slots; ecall |
| riscv32 | Same as riscv64 but 32-bit (sw/lw instead of sd/ld) |
| mips64 | Branch delay slots (NOP after every branch/jump); HI/LO for mul/div; 4 arg regs |
| ppc64 | R11 scratch; CR0 + mfcr for comparisons; isel (conditional move); blr return |
| loongarch64 | T7/T8 scratch; maskeqz/masknez for conditional select; no delay slots |
| sparc64 | Register windows (SAVE/RESTORE); branch delay slots; SETHI for upper immediates |
| s390x | R0 scratch; big-endian; SVC 0 syscall; 5 arg regs (R2-R6) |
| alpha | R27 scratch; 3-operand; callsys; branch PC+4 bias |
| hppa | GATE for syscalls (NOP after); 4 arg regs (R26-R23 reversed); BV return |
| m68k | D/A register separation (only D0-D7 allocatable); 2-operand; variable-length encoding |
| wasm32 | Stack machine — structured emission (local.get, local.set, i32.add, etc.) |

---

## 6. Verification

### 6.1 Z3 — hard dependency

Z3 is the single SMT backend for VUMA's verification obligations:

- **Contract discharge** — every `requires` / `ensures` clause on every
  `transform` is encoded to SMT-LIB and discharged by Z3 before code
  emission.
- **`prove` blocks** — inline assertions, same path.
- **Session-type linearity** — the session-type verifier tracks real
  vregs across channel ops; Z3 discharges the linearity obligations.
- **Information-flow lattice** — operates over real `#[secret]` labels
  and real vregs; Z3 discharges the no-leak obligations.
- **PMT invariants** — capacity, field-bounds, liveness, exclusivity,
  origin, interpretation, cleanup — encoded and discharged by Z3.

### 6.2 Rust verifiers (Lean FFI bridge removed)

Verification is performed by hand-written Rust verifiers that:
1. Walk the SCG / IR and emit SMT-LIB constraints.
2. Call Z3 in-process.
3. Collect `(discharged | unknown | failed)` per obligation.
4. Emit reports that codegen consumes.

No `lake build`, no `elan`, no Lean toolchain required.

---

## 7. Documentation Index

| Document | Scope |
|-----------------------------------------------------|----------------------------------------------------|
| [CHANGELOG.md](CHANGELOG.md) | Per-version change log |
| [docs/architecture.md](docs/architecture.md) | System architecture overview |
| [docs/backends.md](docs/backends.md) | Backend matrix and ISA details |
| [docs/building.md](docs/building.md) | Build prerequisites and troubleshooting |
| [docs/caveats.md](docs/caveats.md) | Known limitations and architectural issues |
| [docs/language-reference.md](docs/language-reference.md) | VUMA language reference |
| [docs/pipeline.md](docs/pipeline.md) | Compilation pipeline stages |
| [docs/testing.md](docs/testing.md) | Test harness and gold-standard suite |

---

## 8. Caveats

VUMA is at `0.2.0-alpha.10`. Key limitations:

1. **Runtime bounds checks are always on.** Every arena-allocated access
   gets a `UGe` bounds check that traps via `__oob_trap` (exit 134).
2. The 4 thin-wrapper backends (`aarch64_be`, `armeb`, `mips64be`,
   `ppc64le`) byte-swap around their parent backends.
3. **`spawn_worker` on wasm32 emulates `fork(2)`** by running parent and
   child branches sequentially in a single Wasm process. This is not
   process isolation.
4. **Z3 is a hard dependency.** A missing or too-old Z3 is a build error.
5. **`contains_fork` opt-out**: functions containing clone/fork syscalls
   fall back to stack-slot ISel. This is a correctness requirement (child
   process has different register state after clone), not a shortcut.

---

## 9. License

Copyright © 2026 VUMA Project Contributors. Released under the **MIT
License**; see [LICENSE](LICENSE) for the full text.
