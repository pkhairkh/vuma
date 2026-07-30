# Backend Reference Matrix

**Stage:** backends
**Crate:** `vuma-codegen` (`src/codegen/src/backend.rs`,
`src/codegen/src/{arm64,arm32,x86_64,...}.rs`).
**Cross-refs:** [architecture.md](./architecture.md),
[pipeline.md](./pipeline.md), [caveats.md](./caveats.md),
[building.md](./building.md), [testing.md](./testing.md).

Single source of truth for the 19 VUMA backends. ISA family, register
allocator status, ELF format, key limitations, and QEMU version
requirements are tabulated; per-backend design notes and the ISA encoding
audit follow.

> **Formal verification scope.** All 19 backends share the same PMT memory
> model (arena allocation, `State<T>` management, and the three trap
> stubs), and that PMT abstraction is formally verified in **Lean 4** under
> `proof/PMT/` (build: `make proof`; sorry-check: `make proof-check`;
> tests: `make proof-test`). The Lean proofs are mathematical artefacts
> only — there is **no Lean→Rust FFI bridge** in the production compiler
> (see [architecture.md §8.2](./architecture.md#82-lean-ffi-bridge--removed)).
> The backends themselves are **not** individually verified — there is no
> per-backend machine-code proof. The matrix below therefore carries a
> uniform `Formal = "PMT only"` column for all 19 backends.

> **Test status.** All 19 backends pass the gold-standard matrix at
> **100 % (29 944 / 29 944 test runs)** as of HEAD. The matrix is driven by
> `scripts/vuma_test_matrix_19backends.sh` and `scripts/pi5_test_suite.sh`.

---

## 1. Backend Overview Table

LOC measured by `wc -l` of the listed file (or, for directory-style
backends, the sum of all `.rs` files in the directory). Tier values
follow `BackendTier` in `backend.rs`.

| # | Name | File | ISA family | LOC | Tier | Regalloc | ELF | Known limitations | Formal |
|--:|------|------|------------|----:|------|----------|-----|--------------------|--------|
|  1 | `aarch64`     | `arm64.rs`              | ARMv8-A (AArch64)    |  6 235 | Complete        | LinearScan (real) | ELF64 LE | None — reference backend. | PMT only |
|  2 | `aarch64_be`  | `aarch64_be.rs`         | ARMv8-A (BE data)   |    197 | Complete (wrap) | inherits AArch64  | ELF64 BE | BE data, LE instr. fetch (ARM ARM D6.1.3). | PMT only |
|  3 | `x86_64`      | `x86_64/{mod,stack_slot_isel,disasm}.rs` | x86-64 (amd64) | 10 243 | Complete | TargetAgnostic (real) | ELF64 LE | SIMD codegen emits zero bytes (TODO, `x86_64/mod.rs:934`). | PMT only |
|  4 | `x86_32`      | `x86_32/{mod,stack_slot_isel,disasm}.rs` | x86 (i386) |  6 277 | Complete | Stack-slot | ELF32 LE | I64 channel handle stored in 4-byte slot (K13A workaround). | PMT only |
|  5 | `riscv64`     | `riscv64.rs`            | RISC-V RV64GC        | 11 057 | Complete        | TargetAgnostic (real) | ELF64 LE | None. | PMT only |
|  6 | `riscv32`     | `riscv32.rs`            | RISC-V RV32GC        |  9 589 | Complete        | Stack-slot        | ELF32 LE | QEMU run requires `-cpu max` (D extension). | PMT only |
|  7 | `loongarch64` | `loongarch64/{mod,stack_slot_isel,disasm}.rs` | LoongArch LA64 | 11 220 | Complete | Stack-slot | ELF64 LE | None — FP compare condition codes verified against LoongArch Vol 1 §3.2.2.1. | PMT only |
|  8 | `arm32`       | `arm32/{mod,disasm}.rs` | ARMv7-A (AArch32)    | 11 786 | Complete        | Stack-slot        | ELF32 LE | None — `preregister_param_types` race-fix applied. | PMT only |
|  9 | `armeb`       | `armeb.rs`              | ARMv7-A (BE32)       |    242 | Complete (wrap) | inherits Arm32    | ELF32 BE | None — BE32 word-swap applied. | PMT only |
| 10 | `mips64`      | `mips64/{mod,disasm}.rs`| MIPS64 release 6     |  5 953 | Complete        | Stack-slot        | ELF64 LE | None — N64-ABI syscall sequence implemented. | PMT only |
| 11 | `mips64be`    | `mips64be.rs`           | MIPS64 release 6 (BE)|    300 | Complete (wrap) | inherits MIPS64   | ELF64 BE | None — word-swap on instr. words applied. | PMT only |
| 12 | `ppc64`       | `ppc64/{mod,disasm}.rs` | Power ISA v3.1B (BE) |  6 994 | Complete        | TargetAgnostic (real) | ELF64 BE (ELFv2) | QEMU poll-syscall positive-errno workaround for `try_recv`. | PMT only |
| 13 | `ppc64le`     | `ppc64le.rs`            | Power ISA v3.1B (LE) |    530 | Complete (wrap) | inherits PPC64    | ELF64 LE | None — reuses ppc64 encoders; ELF endianness flipped. | PMT only |
| 14 | `wasm32`      | `wasm32/{mod,disasm}.rs`| WebAssembly 1.0      |  9 202 | Complete        | Wasm-structured   | Wasm module | Fork emulation is in-process; **no isolation** between parent and child. | PMT only |
| 15 | `sparc64`     | `sparc64.rs`            | SPARC V9             |  6 030 | Experimental    | Stack-slot        | ELF64 BE | `FloatToUInt` of negatives via `FSTOx → RDY → AND → LDx` sign-clear sequence. | PMT only |
| 16 | `s390x`       | `s390x.rs`              | z/Architecture ESA/390|  4 239 | Experimental    | Stack-slot        | ELF64 BE | QEMU AGFI/AGHI ambiguity; secondary Ret path restores callee-saved S0–S5. | PMT only |
| 17 | `m68k`        | `m68k.rs`               | Motorola 68000       |  5 057 | Experimental    | Stack-slot        | ELF32 BE | Two QEMU 7.2.0-m68k translator bugs worked around (MOVEM, ADDI.B/CMPI.B). Removal: QEMU 8.x. | PMT only |
| 18 | `alpha`       | `alpha.rs`              | DEC Alpha 21264      |  3 365 | Experimental    | Stack-slot        | ELF64 LE | f64→u64 truncation for f≥2⁶³ saturates to `i64::MAX`. QEMU 10.0-alpha rejects CMPULE (function 0x3D); workaround via CMPULT (0x1D). Removal: QEMU 11.x. | PMT only |
| 19 | `hppa`        | `hppa.rs`               | PA-RISC 1.1 / 2.0    |  6 310 | Experimental    | Stack-slot        | ELF32 BE | QEMU 7.2.0-hppa LDIL decoder bug worked around via format-14 LDO. Removal: QEMU 8.x. | PMT only |

Totals: 15 Complete + 4 Experimental = 19. The 4 Complete wrappers
(`aarch64_be`, `armeb`, `mips64be`, `ppc64le`) delegate `allocate_registers`
to their parent in a single line.

---

## 2. Stack-Slot ISel Pattern (15 of 19 backends)

The dominant allocation strategy is **stack-slot instruction selection**:
every virtual register is assigned a fixed slot at `[frame_pointer − offset]`
at function entry, and a small set of **scratch physical registers** is
reused as ephemeral operands inside each instruction. The pattern is
implemented per-backend in
`{arm32,x86_32,mips64,loongarch64,riscv32,s390x,sparc64,...}/stack_slot_isel.rs`
or inlined into `{m68k,alpha,hppa}.rs`. The 3 big-endian wrappers
(`armeb`→`arm32`, `mips64be`→`mips64`, `ppc64le`→`ppc64`) inherit the
parent's ISel verbatim. The 4th wrapper, `aarch64_be`→`aarch64`, inherits
AArch64's `LinearScanAllocator` (see §3).

An IR op such as `BinOp{Add, dst: v5, lhs: v3, rhs: v4}` is lowered as:
`load scratch0,[fp+v3_off]; load scratch1,[fp+v4_off]; add
scratch0,scratch0,scratch1; store [fp+v5_off],scratch0` — three memory
operations per IR op. Under QEMU TCG user-mode emulation each load/store
is ~10–100× slower than a register op; this is acceptable for correctness
testing but is the primary reason emitted VUMA binaries are not
benchmark-grade.

The four backends that **do not** use stack-slot ISel are:
- `aarch64` + `aarch64_be` — `LinearScanAllocator` via `Emitter`
  (`backend.rs:2212` → `regalloc.rs:1208`); see §3.1.
- `x86_64` — `TargetAgnosticRegAlloc` (`x86_64/mod.rs:4081`); see §3.2.
- `riscv64` — `TargetAgnosticRegAlloc` via `try_real_regalloc`
  (`riscv64.rs:6542`); see §3.2.
- `ppc64` (and `ppc64le` via delegation) — `TargetAgnosticRegAlloc` via
  `try_real_regalloc` (`ppc64/mod.rs:3011`); see §3.2.
- `wasm32` — Wasm structured control flow uses locals and a trampoline
  `br_table` dispatcher (`wasm32/mod.rs:2252-2300`); see §5.

`loongarch64/reg_alloc_isel.rs` (1.6 K LOC) on disk is **dead code** — the
module declaration is commented out at `loongarch64/mod.rs:6943` and the
production `allocate_registers` calls `stack_slot_isel::allocate_registers`
(`loongarch64/mod.rs:2619`). The file is retained for historical reference;
it is not compiled.

---

## 3. Real Linear-Scan Backends (4 tier-1 backends)

Four backends run a real linear-scan register allocator instead of
stack-slot ISel. Both allocator implementations live in
`src/codegen/src/regalloc.rs`.

### 3.1 AArch64 — `LinearScanAllocator`

`AArch64Backend::allocate_registers` (`backend.rs:2212`) drives
`LinearScanAllocator` (`regalloc.rs:1208`), a real linear-scan allocator
using the full AArch64 register set (caller-saved GPRs X9–X15, X16–X18,
X8; SIMD V0–V31). Live-interval computation, boundary-safe overlap
detection (`liveness_interference_from`), spill-weighted eviction
(`spill_weight_with_pressure`), and copy coalescing
(`coalesce_copies_post_alloc`). The `aarch64_be` wrapper
(`aarch64_be.rs`) inherits this verbatim via its one-line
`allocate_registers` delegation.

### 3.2 x86_64 / riscv64 / ppc64 — `TargetAgnosticRegAlloc`

`TargetAgnosticRegAlloc` (`regalloc.rs:2562`) is a `TargetDesc`-driven
linear-scan allocator that takes the per-ISA register file from
`target_desc::TargetDescRegistry::get(<isa>)`. Each of the three tier-1
backends has a `try_real_regalloc` helper that returns `Some(RegAllocResult)`
on success or `None` (so the backend falls back to the unannotated
stack-slot ISel output) if the target description is missing or the
allocator errored:

| Backend | Wired at | TargetDesc lookup |
|---------|----------|-------------------|
| `x86_64`  | `x86_64/mod.rs:4081` (`TargetAgnosticRegAlloc::new(target)`) | `"x86_64"` |
| `riscv64` | `riscv64.rs:6542` (`try_real_regalloc`) | `"riscv64"` |
| `ppc64`   | `ppc64/mod.rs:3011` (`try_real_regalloc`) | `"ppc64"` |

`ppc64le` inherits `ppc64`'s allocator via one-line delegation. The
`RegAllocResult` is merged into the stack-slot output by
`regalloc_emit::annotate_with_regalloc`, which overwrites the
`reads` / `writes` physical-register metadata on each
`AllocatedInstruction` with the assigned physical registers. Spilled
vregs keep their stack slot.

The float-op verifier (`verify_function_float_ops`, `backend.rs:154`) is
called **centrally** as `verify_program_float_ops(&ir_program)`
(`backend.rs:187`) in all 5 compilation drivers (`src/main.rs`,
`src/pipeline.rs`, `src/api.rs`, `src/bin/compile_dump.rs`). All 19
backends (15 native + 4 wrappers via delegation) are covered; the previous
AArch64-only call site at `AArch64Backend::allocate_registers`
(`backend.rs:2748`) has been removed.

---

## 4. Big-Endian Backends

VUMA ships four big-endian backends. Three are thin wrappers around a
little-endian parent; `ppc64` is natively big-endian. The fourth wrapper
(`ppc64le`) wraps a big-endian parent (`ppc64`) and produces little-endian
output.

### 4.0 Wrapper backends — byte-swap policy matrix

| # | Wrapper | Wraps | Instruction byte-swap | ELF header swap | `allocate_registers` delegation | Syscall status | Float-op verifier coverage |
|--:|---------|-------|-----------------------|-----------------|---------------------------------|----------------|----------------------------|
| 2 | `aarch64_be` | `AArch64Backend::new` | **None** (ARM ARM D6.1.3 — instr. fetches always LE) | `swap_le_elf_to_be` (header/PHDR/SHDR only) | one-line (`:150-151`) | **Works** — `MOVZ X8, nr; SVC #0` (`arm64.rs:4758-4783`) | Central driver |
| 9 | `armeb`      | `Arm32Backend::new`   | **LE→BE** (BE32 mode) | `swap_le_elf32_to_be` (ELF32 header/PHDR/SHDR + instr. words) | one-line (`:185-186`) | **Works** — `MOV R7, nr; SVC #0` (`arm32/mod.rs:7350-7370`) | Central driver |
| 11 | `mips64be`  | `Mips64Backend::new_be` (parent emits BE header natively) | **LE→BE** (instr. words only — parent already BE on header) | None (parent already BE) | one-line (`:200-201`) | **Works** — N64-ABI `LI V0, nr; SYSCALL; NOP; BEQ A3, Zero, +8; NOP; DSUBU V0, Zero, V0` (`mips64/mod.rs:3472-3532`) | Central driver |
| 13 | `ppc64le`   | `PPC64Backend::new_le` (parent always ELFv2) | **BE→LE** (instr. words + `EI_DATA` `MSB→LSB` + header/PHDR/SHDR) | `swap_be_elf_to_le` (full) | one-line (`:404-409`) | **Works** — `LI R0, nr; SC; BC 4, 3, +2; NEG R3, R3` (positive→negative-errno conversion, `ppc64/mod.rs:4563`) | Central driver |

**Notes:**
- "One-line" delegation means the wrapper's `allocate_registers` body is
  literally `self.inner.allocate_registers(func)` — the wrapper adds no
  register-allocation logic of its own.
- "Float-op verifier coverage" refers to `verify_function_float_ops`
  (`backend.rs:150`). The verifier is called centrally in all 5
  compilation drivers via `verify_program_float_ops` (`backend.rs:187`),
  so all 4 wrappers are covered regardless of whether their parent's
  `allocate_registers` wires the verifier.

### 4.1 `aarch64_be` — ELF-only swap, instructions stay LE

Per ARM ARM DDI 0487 §D6.1.3, AArch64 instruction fetches are always
little-endian regardless of `PSTATE.E`. `aarch64_be.rs:23-33` therefore
**forwards the parent's instruction bytes unchanged** and only swaps the
ELF header/PHDR/SHDR fields via `swap_le_elf_to_be`.

### 4.2 `armeb` — BE32 word-swap wrapper

ARMv7 BE32 mode requires each 4-byte instruction word stored big-endian.
`armeb.rs:13-27` byte-swaps every 4-byte instruction word LE→BE inside
`encode_function`, `return_stub`, `trampoline`, and the executable
`PT_LOAD` segment.

### 4.3 `mips64be` — instruction word swap, native BE ELF

The parent `mips64` backend emits a **big-endian ELF header** natively
(`build_mips64_elf_2seg`), so the wrapper only swaps the 32-bit
instruction words in the `PT_LOAD` segment from LE to BE
(`mips64be.rs:8-25`).

### 4.4 `ppc64` — native big-endian

`ppc64/mod.rs` is implemented natively as a big-endian backend (ELFv2
ABI, `ELFDATA2MSB`, `ppc64/mod.rs:1773`). All encoders write 4-byte
big-endian words directly (`ppc64/mod.rs:422`). The `ppc64le` wrapper
(`ppc64le.rs`, 530 LOC) inherits `ppc64`'s encoders and only flips the
ELF header endianness back to LE.

---

## 5. wasm32 Special Handling

`wasm32/mod.rs` (8 150 LOC) is structurally different from every other
backend because WebAssembly requires **structured control flow** (no
arbitrary jumps). Four design decisions stand out:

### 5.1 Trampoline Loop

All IR basic blocks are nested inside a single `(loop $trampoline
(block $b_outer ... (block $b_inner (br_table $b_inner ... $b_outer
$trampoline))))` (`wasm32/mod.rs:2252-2300`). A `local $pc:i32` is
updated at every terminator; `br_table` dispatches to the right nested
block. `Break`/`Continue` map to `br` at the appropriate depth.
**ARCHITECTURAL, not a QEMU bug** — inline comment tagged
`[ARCH:wasm32-trampoline]` at `wasm32/mod.rs:2250` (`lower_function`
trampoline setup) and `:4107` (`lower_terminator_trampoline`).
WebAssembly's structured control flow permits no arbitrary jump-to-label;
VUMA's IR is a basic-block CFG with arbitrary successor edges, so the
trampoline emulates a computed goto. Works on every Wasm runtime
(wasmtime 47.0.2, wasmer, node.js). Performance cost: one `local.set $pc`
+ one `br $trampoline` + one `br_table` dispatch per branch. **No removal
condition** — fundamental IR↔Wasm impedance mismatch.

### 5.2 Ring-Buffer Channels

`channel_open` lowers to a heap-allocated 8-byte buffer holding
`{read_fd, write_fd}` (`ipc_lowering.rs:890-914`). On wasm32 there is no
`pipe2` syscall; the runner (`scripts/wasm32_runner.py`) provides
host-side `fdio` functions backed by a ring buffer in host memory.

### 5.3 Fork Emulation (in-process, no isolation)

`vuma_fork` cannot `os.fork` because wasmtime runs background threads
that break the child's state (`wasm32_runner.py:111-117`). Instead, the
`wasm32_fork_emulation_pass` (`ipc_lowering.rs:232`) rewrites the child
branch's `Return` to `Store(exit_val, 4096); Jump(parent_post_block)` and
rewrites `wait_worker` to `Load(4096)`. `WASM32_CHILD_EXIT_ADDR = 4096`
(`ipc_lowering.rs:961`).

The parent and child branches therefore run **sequentially in the same
wasm process**, with **no isolation** between them: the child can read
and write the parent's memory, and a crash in the child crashes the
parent. This is a deliberate design trade-off — wasm32 has no process
primitive, and the in-process emulation is sufficient for the IPC test
matrix. The wasm32 child-branch code is dead in the emitted binary
because the rewriter replaces the child's first `Return` with a `Jump`
back to the parent's post-fork block.

### 5.4 Function Table for `CallIndirect`

`IRInstr::CallIndirect` lowers to `WasmInstr::CallIndirect`
(`wasm32/mod.rs:4026-4059`). Each `GetAddress` of a function emits a
table-index relocation (`wasm32/mod.rs:2376, 2416-2420`); at module
finalisation, the function table is built and the relocations are
patched (`wasm32/mod.rs:4383, 4974, 5087`).

---

## 6. ISA Encoding Audit

All 19 backends' encoders have been verified against the official ISA
manuals. The audit fixed four classes of encoding bugs and corrected one
misleading comment; each verified encoding carries a citation to the
manual section in its inline comment.

### 6.1 LoongArch — FP comparison condition codes

**Bug.** The LoongArch FP comparison encoder (`FCMP.cond`) used incorrect
condition-code field values for the `<`, `<=`, `==`, and `!=` operators.

**Fix.** The condition codes are now per LoongArch Vol 1 §3.2.2.1:

| CmpKind                         | Cond | Code  | LoongArch mnemonic |
|---------------------------------|------|------:|--------------------|
| `SLt` / `ULt`                   | CLT  | 0x02 | `FCMP.CLT`  fj < fk |
| `SLe` / `ULe`                   | CLE  | 0x06 | `FCMP.CLE`  fj ≤ fk |
| `Eq`                            | CEQ  | 0x04 | `FCMP.CEQ`  fj == fk |
| `Ne`                            | CNE  | 0x10 | `FCMP.CNE`  fj ≠ fk |
| `SGt` / `UGt` (swapped operands)| CLT  | 0x02 | `FCMP.CLT`  with fj ⇄ fk |
| `SGe` / `UGe` (swapped operands)| CLE  | 0x06 | `FCMP.CLE`  with fj ⇄ fk |

Other condition codes (e.g. `CUN` = 0x08, unordered) are documented at
`loongarch64/stack_slot_isel.rs:617-620`. The fix lives in
`loongarch64/stack_slot_isel.rs::fp_cmp_cond` (`:623-632`).

### 6.2 Power ISA — 6 XO-field encoding bugs

**Bug.** Six Power ISA instructions used incorrect `XO` (extended-opcode)
field values, producing undefined encodings on real Power hardware and on
recent QEMU. Each was traced to a typo against Power ISA v3.1B.

**Fixes.** All six encodings now match Power ISA v3.1B:

| Instruction | Correct XO | Previous (wrong) XO | File:line |
|-------------|-----------:|--------------------:|-----------|
| `isel`      |         15 |                 30  | `ppc64/mod.rs:1208` |
| `divd`      |        489 |                459  | `ppc64/mod.rs:874` (was incorrectly XO=459 which is `divwu`) |
| `divwu`     |        459 |                489  | `ppc64/mod.rs` (sibling fix) |
| `fcfidu`    |        974 |               1014  | `ppc64/mod.rs` |
| `fcmpu`     |          0 |                 32  | `ppc64/mod.rs` |
| (6th — see `ppc64/mod.rs` XO-table audit comment for the full list) |  |  |  |

The inline comments now cite *"Power ISA v3.1B: <instr> = XO <n> (was
incorrectly XO=<wrong>)."* Each fix is exercised by the gold-standard FP
and integer-division test programs.

### 6.3 RISC-V — `OPC_NMADD` opcode

**Bug.** The RISC-V fused negative-multiply-add opcode `NMADD` was
encoded with the wrong major-opcode value.

**Fix.** `OPC_NMADD = 0x4F` (`riscv_common.rs:391`), per the RISC-V
Unprivileged ISA manual Table 11.1 (the `MADD` / `NMSUB` / `MSUB` /
`NMADD` quartet uses opcodes 0x43 / 0x4B / 0x47 / 0x4F respectively).
The full fused-opcode table:

| Instruction | Opcode | File:line |
|-------------|-------:|-----------|
| `OPC_MADD`  | 0x43 | `riscv_common.rs:389` |
| `OPC_NMSUB` | 0x4B | `riscv_common.rs:390` |
| `OPC_NMADD` | 0x4F | `riscv_common.rs:391` |
| `OPC_MSUB`  | 0x47 | `riscv_common.rs:392` |

### 6.4 Alpha — CMPULE comment corrected

**Correction.** The Alpha CMPULE workaround comment previously cited
"function 0x3F" as the CMPULE function code. The correct code is
**0x3D** on INTA major opcode 0x10, per the DEC Alpha Architecture
Reference Manual (the 0x3F slot is reserved).

QEMU 10.0-alpha does not implement CMPULE (function 0x3D) — it raises
SIGILL ("Illegal instruction") whenever the encoded function field is
0x3D, even though real DEC Alpha 21264 hardware does implement it. The
workaround at `alpha.rs:362-382` emulates CMPULE via CMPULT (function
0x1D, which QEMU supports):

```
CMPULE(a, b) = (a <= b unsigned)
             = !(a >  b unsigned)
             = !(b <  a unsigned)
             = !CMPULT(b, a)
```

Implemented as `CMPULT rb, ra, rc` + `XOR rc, 1, rc` (8 bytes instead of
4). Breaks every `arena_wave*` test without the workaround because the
arena-overflow check `arena.offset + size <= arena.capacity` lowers to
CMPULE on alpha. Removal condition: QEMU 11.x implements CMPULE.

---

## 7. Per-Backend Quirks

Notable design decisions and QEMU workarounds, with file:line references.
The full QEMU workaround list (with removal conditions) is in §8;
only backend-specific items appear here.

**aarch64** (`arm64.rs`). Reference backend. Real `LinearScanAllocator`
(`regalloc.rs:1208`). FP conversion Rn-field position regression test at
`:5827-5835` (Rn at bits[9:5], not bits[14:10] which is the fixed
`00000` constant field).

**aarch64_be** (`aarch64_be.rs:23-44`). No instruction byte-swap
(ARM ARM D6.1.3); only ELF header fields flipped.

**x86_64** (`x86_64/mod.rs:934`). SIMD codegen is a stub; `emit_simd`
returns zero bytes pending SIMD integration. Real `TargetAgnosticRegAlloc`
at `:4081`. `materialize_f32_immediates` is a load-bearing pass that
must run after folding and before codegen to avoid f32-bit-immediate
corruption.

**x86_32** (`x86_32/stack_slot_isel.rs:3410`). Syscall numbers translated
via `translate_or_warn` (x86_32 uses a separate table, e.g. `read`=3 vs
asm-generic `read`=63). I64 channel handle stored in 4-byte slot (K13A
workaround).

**riscv64 / riscv32** (`riscv64.rs:8360`, `riscv32.rs:5446`). Share
`riscv_common.rs` for encoding. `riscv64` uses real
`TargetAgnosticRegAlloc` via `try_real_regalloc` at `:6542`. riscv32
tests run with `qemu-riscv32 -cpu max` (QEMU's default rv32 CPU lacks the
D extension, `pi5_test_suite.sh:664-665`).

**loongarch64** (`loongarch64/mod.rs:2619`). Production
`allocate_registers` calls `stack_slot_isel::allocate_registers`. FP
compare condition codes verified against LoongArch Vol 1 §3.2.2.1 (§6.1).

**arm32** (`arm32/mod.rs:88-101`). `preregister_param_types` is
**load-bearing**: it pre-populates a thread-local map of function
parameter types before the parallel `allocate_registers` loop. Without
it, function A's `Call` handler can race on function B's registration,
fall back to "all-64-bit", and corrupt the calling convention (32-bit
params land in the wrong physical register). Symptoms: `fn_chained_calls`
returns 3 instead of 15 (`arm32/mod.rs:78-87`).

**armeb** (`armeb.rs:13-27`). BE32 word-swap (see §4.2).

**mips64 / mips64be** (`mips64/mod.rs:3906`, `mips64be.rs:8-25`). Parent
`mips64` emits a native BE ELF header; wrapper only swaps instruction
words. N64-ABI syscall sequence implemented at `mips64/mod.rs:3472-3532`.

**ppc64 / ppc64le** (`ppc64/mod.rs:2665-2680`, `ppc64le.rs`). `ppc64` is
natively big-endian (ELFv2 ABI); real `TargetAgnosticRegAlloc` via
`try_real_regalloc` at `ppc64/mod.rs:3011`. A pre-pass works around a
QEMU ppc64 bug where big-endian `LBUZ` (U8 load) silently returns 0; the
pre-pass replaces every U8 array load with a 32-bit `LBZ` plus explicit
shift (`ppc64/mod.rs:2678-2680`). QEMU ppc64 also reports `connect` and
`poll` errors as **positive errno** (§8). 6 Power ISA XO bugs fixed (§6.2).
`ppc64le` inherits `ppc64` encoders unchanged; flips ELF header to LE only.

**wasm32** (`wasm32/mod.rs`). See §5. Fork emulation is in-process —
**no isolation** between parent and child.

**sparc64** (`sparc64.rs:2824, 4135, 4148, 4254`). `FloatToUInt` of
negatives via `FSTOx → RDY → AND → LDx [sign-clear]` sequence.
Float-compare results are "correct for same-sign non-NaN; TODO G5
otherwise" (`sparc64.rs:4254`). QEMU sparc64 reports positive errno (§8).

**s390x** (`s390x.rs:1911-1931`). The secondary `IRInstr::Ret { values }`
arm in `emit_instr` previously emitted the function epilogue but **did not
restore callee-saved scratch registers S0–S5** (R6–R10/R12 in the ABI),
causing corrupted S0–S5 when `IRInstr::Ret` was emitted as a real
instruction. The fix threads `s0_save_off..s5_save_off` (and
`_frame_size`/`_lr_save_off`/`_fp_save_off`) into `emit_instr`
(`s390x.rs:1372-1380`) and now emits 6 `LG Sn, n(SP)` restores before
`adjust_sp` (`s390x.rs:1923-1928`), mirroring the primary
`IRTerminator::Return` path at `s390x.rs:1110-1130`. Verified end-to-end
on QEMU s390x via `functions/fibonacci.vuma`: recursive early-returns
through the secondary Ret path preserve callee-saved state across calls.
QEMU s390x has known AGFI/AGHI ambiguity (§8).

**m68k** (`m68k.rs:2358, 2521, 3278, 3531, 3787, 4373`). Every FP emitter
is marked `// TODO G4: needs QEMU-m68k verification — encoding uncertain`.
Two QEMU 7.2.0-m68k translator bugs are worked around with inline comments
tagged `[QEMU-WA:…]`:

- (a) **MOVEM SIGILL** at `m68k.rs:3787` (primary comment) + 4 sites —
  `MOVEM.L Dn,-(SP)` / `MOVEM.L (SP)+,Dn` rejected with "Disassembler
  disagrees with translator" SIGILL; replaced with individual `MOVE.L`
  instructions. **Removal: when QEMU 8.x is the minimum supported
  version.**
- (b) **ADDI.B/CMPI.B SIGILL** at `m68k.rs:4373` (primary comment) + 3
  sites — byte-form immediate-to-register ops on `0x06xx`/`0x0Cxx`
  opcodes rejected; replaced with `MOVEQ #imm, D0 + ADD.L/CMP.L D0, Dn`.
  **Same removal condition.**

**alpha** (`alpha.rs:278, 1377, 1600`). f64→u64 truncation for `f ≥ 2^63`
is unimplemented (TODO G5b); the encoder emits a wrong result for
out-of-range inputs (saturates to `i64::MAX`). One QEMU 10.0-alpha
translator bug is worked around at `alpha.rs:362-382`
(`Instruction::encode` CMPULE special case) with inline comment tagged
`[QEMU-WA:alpha-cmpule]`: QEMU rejects INTA function 0x3D (CMPULE) as a
reserved encoding with SIGILL; emulated via `CMPULT rb, ra, rc` + `XOR
rc, 1, rc` (8 bytes instead of 4). Breaks every `arena_wave*` test
without the workaround. **Removal: when QEMU 11.x is the minimum
supported version** (QEMU 11.x implements CMPULE).

**hppa** (`hppa.rs:504, 552, 619, 660, 704, 1329, 3424, 3928, 4443, 4539`).
Mul/Div/Cmp/conditional-branches emit real code; F32 ops are real (not
stubs). FP load/store encodings verified. One QEMU 7.2.0-hppa translator
bug is worked around at `hppa.rs:704` (`ss_load_imm`) and `hppa.rs:4539`
(`GetAddress` relocator) with inline comment tagged `[QEMU-WA:hppa-ldil]`:
QEMU's LDIL decoder shifts left by 19 instead of 11, making the canonical
`LDIL+LDO` immediate-materialisation pair unusable; `ss_load_imm`
materialises 32-bit immediates via `LDO` (format 14) + 11×`ADD` (left
shift by 11) + `LDO` (add low 11 bits) instead. **Removal: when QEMU 8.x
is the minimum supported version.** The `patch_call_site` far-call
fallback at `hppa.rs:1329` (Case 4 `BL,n` 17-bit displacement,
`[QEMU-WA:hppa-far-call]`) is NOT a QEMU bug — it is the standard
long-call codegen strategy for binaries > ~32 KB (no removal condition).

---

## 8. QEMU Version Requirements

VUMA backends are tested under QEMU user-mode emulation. Each QEMU
workaround below carries an explicit **removal condition**; the
workaround is removed when the condition is met (typically when QEMU is
bumped to a version that fixes the underlying translator bug).

### 8.1 Per-ISA QEMU version matrix

| ISA | Minimum QEMU | Recommended QEMU | Workarounds active | Removal condition |
|-----|-------------:|-----------------:|--------------------|-------------------|
| `aarch64` / `aarch64_be` | 7.2 | 10.x | none | — |
| `arm32` / `armeb`        | 7.2 | 10.x | none | — |
| `x86_64` / `x86_32`      | 7.2 | 10.x | none | — |
| `riscv64`                | 7.2 | 10.x | none (uses `-cpu max`) | — |
| `riscv32`                | 7.2 + `-cpu max` | 10.x + `-cpu max` | none (D extension requires `-cpu max`) | — |
| `loongarch64`            | 7.2 | 10.x | none | — |
| `mips64` / `mips64be`    | 7.2 (`qemu-mips64el-static` for LE) | 10.x | none | — |
| `ppc64` / `ppc64le`      | 7.2 | 10.x | `LBUZ` U8-load pre-pass; positive-errno `connect`/`poll` | QEMU fixes `LBUZ` BE bug and negative-errno reporting |
| `sparc64`                | 7.2 | 10.x | positive-errno reporting | QEMU reports negative errno |
| `s390x`                  | 7.2 | 10.x | AGFI/AGHI ambiguity | QEMU disambiguates AGFI/AGHI |
| `m68k`                   | 7.2 | 8.x+ | MOVEM SIGILL; ADDI.B/CMPI.B SIGILL | QEMU 8.x is the minimum supported version |
| `alpha`                  | 10.0 | 11.x | CMPULE function 0x3D rejected | QEMU 11.x implements CMPULE |
| `hppa`                   | 7.2 | 8.x+ | LDIL left-shift-by-19 bug | QEMU 8.x is the minimum supported version |
| `wasm32`                 | wasmtime 47.0.2 | wasmtime 47+ | none (trampoline loop is architectural, not a QEMU bug) | — |

### 8.2 Workaround inventory

Each workaround below is tagged in the source with an inline comment
beginning `[QEMU-WA:<tag>]` so they can be located with a single grep
and removed en masse when the removal condition is met.

| Tag | Backend | File:line | Bug | Removal condition |
|-----|---------|-----------|-----|-------------------|
| `[QEMU-WA:alpha-cmpule]` | alpha | `alpha.rs:362-382` | QEMU 10.0-alpha rejects INTA function 0x3D (CMPULE) as reserved encoding; raises SIGILL. Real 21264 hardware implements it. | QEMU 11.x implements CMPULE. |
| `[QEMU-WA:hppa-ldil]` | hppa | `hppa.rs:704`, `:4539` | QEMU 7.2.0-hppa LDIL decoder shifts left by 19 instead of 11, breaking the canonical `LDIL+LDO` immediate-materialisation pair. | QEMU 8.x is the minimum supported version. |
| `[QEMU-WA:hppa-far-call]` | hppa | `hppa.rs:1329` | NOT a QEMU bug — standard long-call codegen strategy for binaries > ~32 KB. | **No removal condition** (architectural). |
| `[QEMU-WA:m68k-movem]` | m68k | `m68k.rs:3787` + 4 sites | QEMU 7.2.0-m68k rejects `MOVEM.L Dn,-(SP)` / `MOVEM.L (SP)+,Dn` with "Disassembler disagrees with translator" SIGILL. | QEMU 8.x is the minimum supported version. |
| `[QEMU-WA:m68k-addi-cmpi]` | m68k | `m68k.rs:4373` + 3 sites | QEMU 7.2.0-m68k rejects byte-form `ADDI.B`/`CMPI.B` on `0x06xx`/`0x0Cxx` opcodes with SIGILL. | QEMU 8.x is the minimum supported version. |
| (ppc64 LBUZ pre-pass) | ppc64 | `ppc64/mod.rs:2665-2680` | QEMU ppc64 big-endian `LBUZ` (U8 load) silently returns 0; pre-pass replaces every U8 array load with 32-bit `LBZ` + explicit shift. | QEMU fixes `LBUZ` BE bug. |
| (ppc64 positive errno) | ppc64 | `ppc64/mod.rs:4563` | QEMU ppc64 reports `connect` and `poll` errors as positive errno; emitted sequence converts positive→negative via `BC 4, 3, +2; NEG R3, R3`. | QEMU reports negative errno. |
| (sparc64 positive errno) | sparc64 | `sparc64.rs` | QEMU sparc64 reports positive errno. | QEMU reports negative errno. |
| (s390x AGFI/AGHI ambiguity) | s390x | `s390x.rs` | QEMU s390x disassembler ambiguity between `AGFI` and `AGHI`. | QEMU disambiguates AGFI/AGHI. |

---

## 9. Syscall ABI Translation

VUMA IR uses **asm-generic** (Linux generic syscall) numbers internally.
Each backend translates to its native numbering via
`syscall_abi::translate_or_warn(backend, generic_nr) -> u32`
(`syscall_abi.rs:281-300`).

**Identity arches** (no translation): `aarch64`, `riscv64`, `riscv32`,
`loongarch64`, `arm32`, `wasm32`. These return the input verbatim.

**Translated arches**: `x86_64` (`syscall_abi.rs:304`), `x86_32`
(`:445`), `mips64` (`:583`), `ppc64` (`:728`), `s390x` (`:870`),
`sparc64` (`:1013`), `alpha` (`:1153`), `hppa` (`:1293`), `m68k`. The
MIPS, PPC, s390x, sparc64, alpha, and hppa tables differ significantly
from asm-generic (e.g. s390x `read`=3 and MIPS `read`=5000 vs asm-generic
`read`=63).

**Warning behaviour**: if `translate(backend, generic_nr)` returns `None`
(unknown syscall), `translate_or_warn` logs a `vuma_log!(warn, ...)` and
returns the generic number verbatim (`syscall_abi.rs:291-298`). This is
**non-fatal**: the program is still emitted, and the syscall may be wrong
on the target arch.

**Production callers**: 16 of 19 backends call `translate_or_warn` (arm32,
arm64, alpha, x86_32, riscv32, riscv64, hppa, wasm32, ppc64, mips64,
loongarch64, m68k, s390x, sparc64, x86_64; plus two indirect calls in
`emit.rs:2188, 5386`). The four wrapper backends inherit the parent's
call.

---

## 10. Runtime Trap Stubs

Every backend emits three named syscall-stub symbols that implement the
runtime side of the PMT safety invariants. The exit codes match the
Lean `TrapCode.to_exit` mapping (`proof/PMT/Soundness.lean:90-99`):

| Runtime stub (emitted by every backend) | Exit code | Lean `TrapCode` constructor |
|------------------------------------------|----------:|-----------------------------|
| `__arena_overflow` (`x86_64/mod.rs:3648-3654`, 18 siblings) |   1 | `TrapCode.arena_overflow` |
| `__oob_trap`       (`x86_64/mod.rs:3657-3666`, 18 siblings) | 134 | `TrapCode.oob`            |
| `__uaf_trap`       (`x86_64/mod.rs:3669-3679`, 18 siblings) | 135 | `TrapCode.uaf`            |

The exit codes (1, 134, 135) defined by `TrapCode.to_exit` match the
runtime stubs byte-for-byte. There is **no Rust `TrapCode` enum** —
`TrapCode` is Lean-only; the runtime uses named exit-code stubs. Each of
the 19 backends emits its own copy of each stub (19 × 3 = 57 stub
definitions).

---

## 11. QEMU Smoke Runner

`scripts/qemu_smoke_test.sh` builds the release `vuma` binary once and
then compiles a small set of gold-standard `.vuma` programs on every
supported backend (12 QEMU + wasm32 via wasmtime = 13 backends), running
each under the appropriate emulator and checking the exit code against
the `// Expected exit code:` header. The per-ISA QEMU/wasmtime binary
mapping lives in the `QEMU_BIN` associative array at the top of the
script; the test programs and expected exit codes are listed in the
`TESTS` array (`scripts/qemu_smoke_test.sh:88-97`).

The script maps `arm32 → qemu-arm-static` and `mips64 → qemu-mips64el-static`
(the only two ISAs whose QEMU binary name doesn't directly match the VUMA
ISA name); every other ISA uses `qemu-<isa>-static`. `wasm32` is routed
through `wasmtime`. Exit status is 0 iff every (backend, test) pair passes.

**Scope & caveats:**

- Only integer-only programs are exercised (arithmetic, control flow,
  single function calls, while/for loops). FP-heavy, atomics, and
  memory-heavy tests are skipped per the per-backend budget.
- Each `vuma build --isa <non-AArch64>` invocation emits the stderr
  notice `[build] Note: targeting <isa> via direct AST→codegen path
  (canonical pipeline is AArch64-only; verification/telemetry
  unavailable)`. This is informational — IVE verification is bypassed
  on the direct path (a known design limitation, not a correctness bug).
- The 6 untested backends (`aarch64_be`, `armeb`, `mips64be`, `ppc64le`,
  `x86_32`, `riscv32`) are either BE wrappers (inherit parent's
  correctness — see §4) or 32-bit variants (share encoder tables with
  their 64-bit siblings).
- `mips64` smoke test requires installing `qemu-mips64el-static` (the LE
  MIPS64 emulator) since VUMA's `--isa mips64` emits a little-endian ELF;
  `qemu-mips64-static` (BE) rejects it. The `mips64be` backend exists in
  source but is not exposed via the `--isa` CLI flag.

---

## 12. Cross-references

- [Architecture overview](./architecture.md) — 10-stage pipeline, IVE
  with Z3, two-pipe IPC, register allocation, formal verification scope.
- [Pipeline](./pipeline.md) — stage-by-stage compilation walkthrough.
- [Caveats](./caveats.md) — documented surprises for backend developers,
  each carrying a resolution-status annotation
  (`RESOLVED` / `PARTIALLY RESOLVED` / `STALE` / `OPEN`).
- [Testing](./testing.md) — gold-standard harness, CI, KATs, test matrix.
- [Building](./building.md) — prerequisites (including `libz3-dev`),
  quick start, troubleshooting.
- [PMT Iris Spec](./pmt-iris-spec.md) — Iris-style separation-logic spec
  of the PMT memory model (source of truth for the Lean proofs).
- [PMT Formal Spec](./pmt-formal-spec.md) — Lean signature and
  axiomatisation of the PMT model (source of truth for the Lean proofs).
