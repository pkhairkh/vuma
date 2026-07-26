# Backend Reference Matrix

**Stage:** backends
**Crate:** `vuma-codegen` (`src/codegen/src/backend.rs`, `src/codegen/src/{arm64,arm32,x86_64,...}.rs`).
**Cross-refs:** `../architecture/overview.md`, `../architecture/pipeline.md` –10,
`../caveats.md`, `../caveats.md` –6.

Single source of truth for the 19 VUMA backends. Tier classification,
allocation strategy, ELF format, key quirks, and syscall-ABI wiring
are tabulated; per-backend design notes follow.

> **Formal verification scope.** All 19 backends share the same PMT
> memory model (arena allocation, `State<T>` management, and the three
> trap stubs), and that PMT abstraction is formally verified in **Lean 4**
> under `proof/PMT/` (build: `make proof`; sorry-check: `make proof-check`;
> tests: `make proof-test`). The backends themselves are **not**
> individually verified — there is no per-backend machine-code proof.
> The matrix below therefore carries a uniform `Formal = "PMT only"`
> column for all 19 backends. See for the full scope and the
> runtime-stub ↔ Lean `TrapCode` mapping.

---

## 1. Backend Overview Table

LOC measured by `wc -l` of the listed file (or, for directory-style
backends, the sum of all `.rs` files in the directory). Tier values
follow `BackendTier` in `backend.rs:852-879`.

| # | Name | File | LOC | Tier | Allocation Strategy | ELF Format | Key Quirks | Formal |
|----|--------------|--------------------------------------|-------:|---------------|-----------------------|-------------------|---------------------------------------------------------|-----------|
| 1 | `aarch64` | `arm64.rs` | 6 235 | Complete | Stack-slot ISel | ELF64 LE | Only backend calling `verify_function_float_ops` (`backend.rs:2747-2752`) | PMT only |
| 2 | `aarch64_be` | `aarch64_be.rs` | 197 | Complete | Wrapper → `arm64` | ELF64 BE | Forwards LE instr. bytes; only ELF header swapped (ARM ARM D6.1.3) | PMT only |
| 3 | `x86_64` | `x86_64/{mod,stack_slot_isel,disasm}.rs` | 10 243 | Complete | Stack-slot ISel | ELF64 LE | SIMD emits zero bytes (TODO, `x86_64/mod.rs:934`) | PMT only |
| 4 | `x86_32` | `x86_32/{mod,stack_slot_isel,disasm}.rs` | 6 277 | Complete | Stack-slot ISel | ELF32 LE | I64 channel handle stored in 4-byte slot (K13A workaround, see `caveats.md`) | PMT only |
| 5 | `riscv64` | `riscv64.rs` | 11 057 | Complete | Stack-slot ISel | ELF64 LE | Largest single-file backend; syscall numbers translated via `translate_or_warn` | PMT only |
| 6 | `riscv32` | `riscv32.rs` | 9 589 | Complete | Stack-slot ISel | ELF32 LE | QEMU run requires `-cpu max` (D extension, `pi5_test_suite.sh:664`) | PMT only |
| 7 | `loongarch64`| `loongarch64/{mod,reg_alloc_isel,...}.rs` | 11 220 | Complete | **Real reg-alloc ISel** | ELF64 LE | Only backend with a register cache (`loongarch64/reg_alloc_isel.rs:1-25`) | PMT only |
| 8 | `arm32` | `arm32/{mod,disasm}.rs` | 11 786 | Complete | Stack-slot ISel | ELF32 LE | Parallel-alloc race fixed by `preregister_param_types` (`arm32/mod.rs:88-101`) | PMT only |
| 9 | `armeb` | `armeb.rs` | 242 | Complete | Wrapper → `arm32` | ELF32 BE | BE32: word-swaps every 4-byte instr. (`armeb.rs:13-27`) | PMT only |
| 10 | `mips64` | `mips64/{mod,disasm}.rs` | 5 953 | Complete | Stack-slot ISel | ELF64 LE | Emits BE ELF header natively; `mips64be` wrapper only swaps instr. words | PMT only |
| 11 | `mips64be` | `mips64be.rs` | 300 | Complete | Wrapper → `mips64` | ELF64 BE | Word-swaps each 4-byte instr. word LE→BE (`mips64be.rs:8-25`) | PMT only |
| 12 | `ppc64` | `ppc64/{mod,disasm}.rs` | 6 994 | Complete | Stack-slot ISel | ELF64 BE (ELFv2) | Big-endian U8-load QEMU bug pre-worked-around (`ppc64/mod.rs:2665-2680`) | PMT only |
| 13 | `ppc64le` | `ppc64le.rs` | 530 | Complete | Wrapper → `ppc64` | ELF64 LE | Reuses ppc64 encoders; only ELF endianness flipped | PMT only |
| 14 | `wasm32` | `wasm32/{mod,disasm}.rs` | 9 202 | Complete | Wasm-structured | Wasm module | Trampoline loop + ring-buffer channels + fork emulation (see) | PMT only |
| 15 | `sparc64` | `sparc64.rs` | 6 030 | Experimental | Stack-slot ISel | ELF64 BE | Unsigned-compare correction TODO F1d (`sparc64.rs:2824`) | PMT only |
| 16 | `s390x` | `s390x.rs` | 4 239 | Experimental | Stack-slot ISel | ELF64 BE | Secondary `IRInstr::Ret` path skips callee-saved restore (`s390x.rs:1872-1876`) | PMT only |
| 17 | `m68k` | `m68k.rs` | 5 057 | Experimental | Stack-slot ISel | ELF32 BE | FP emitters marked "TODO G4: needs QEMU-m68k verification" (`m68k.rs:2521`) | PMT only |
| 18 | `alpha` | `alpha.rs` | 3 365 | Experimental | Stack-slot ISel | ELF64 LE | f64→u64 truncation TODO G5b for f≥2⁶³ (`alpha.rs:1377`) | PMT only |
| 19 | `hppa` | `hppa.rs` | 6 310 | Scaffolded | Stack-slot ISel | ELF32 BE | FP load/store encodings unverified (`hppa.rs:552, 619`); stub F32 ops (`hppa.rs:3424`) | PMT only |

Totals: 14 Complete + 4 Experimental + 1 Scaffolded = 19. The README's
"15 Complete" figure (README.md:21) folds the scaffolded `hppa` backend
into "Experimental".

---

## 2. Stack-Slot ISel Pattern (16 of 19 backends)

The dominant allocation strategy is **stack-slot instruction
selection**: every virtual register is assigned a fixed slot at
`[frame_pointer − offset]` at function entry, and a small set of
**scratch physical registers** is reused as ephemeral operands inside
each instruction. The pattern is implemented per-backend in
`{arm32,x86_64,x86_32,mips64,ppc64,riscv64,riscv32,s390x,sparc64,loongarch64,...}/stack_slot_isel.rs`
or inlined into `{m68k,alpha,hppa}.rs`. The 3 big-endian wrappers
(`armeb`→`arm32`, `mips64be`→`mips64`, `ppc64le`→`ppc64`) inherit the
parent's ISel verbatim. The 4th wrapper, `aarch64_be`→`aarch64`, also
inherits — but `aarch64` itself uses LinearScan (see), so
`aarch64_be` is in the LinearScan column, not the stack-slot column.

An IR op such as `BinOp{Add, dst: v5, lhs: v3, rhs: v4}` is lowered
as: `load scratch0,[fp+v3_off]; load scratch1,[fp+v4_off]; add
scratch0,scratch0,scratch1; store [fp+v5_off],scratch0` — three
memory operations per IR op. Under QEMU TCG user-mode emulation each
load/store is ~10–100× slower than a register op (measured against the
former `loongarch64/reg_alloc_isel.rs` register-cache path, now dead
code — see historical note); this is acceptable for correctness
testing but is the primary reason emitted VUMA binaries are not
benchmark-grade.

The three backends that **do not** use stack-slot ISel are:
- `aarch64` + `aarch64_be` — LinearScan via `Emitter` (`backend.rs:3095`
 → `regalloc.rs`'s `RegAllocator` / `LinearScanAllocator`); see;
- `wasm32` — Wasm structured control flow uses locals and a
 trampoline `br_table` dispatcher (`wasm32/mod.rs:2252-2300`).

(The prior "17 of 19 backends" and "Only loongarch64 uses a real
register cache" claims are **STALE**. re-audited the codegen
and found that `loongarch64/reg_alloc_isel.rs` was already dead code —
the module declaration was commented out at `mod.rs:6943` and the
production `allocate_registers` impl calls
`stack_slot_isel::allocate_registers` (`mod.rs:2619`). re-verified this and corrected the caveat; loongarch64 is now counted
in the stack-slot column.)

---

## 3. LinearScan Backends — AArch64

Two of the 19 backends use a real register allocator instead of
stack-slot ISel: `aarch64` (the reference backend, `arm64.rs`) and
its big-endian wrapper `aarch64_be`. `AArch64Backend::allocate_registers`
(`backend.rs:3095`) does NOT call any stack-slot ISel — it delegates
to `Emitter::emit_function(func, None)` (`backend.rs:3110`), which in
turn drives `regalloc.rs`'s `RegAllocator` / `LinearScanAllocator`
(legacy greedy + linear-scan). The `aarch64_be` wrapper
(`aarch64_be.rs`) inherits this verbatim via its one-line
`allocate_registers` delegation (.0).

The float-op verifier (`verify_function_float_ops`, `backend.rs:154`)
was previously wired ONLY inside `AArch64Backend::allocate_registers`
at `backend.rs:2748`. removed that AArch64-only call site and
re-wired the verifier **centrally** as
`verify_program_float_ops(&ir_program)` (`backend.rs:187`) in all 5
compilation drivers (`src/main.rs:1803`, `src/pipeline.rs:6904` &
`:7939`, `src/api.rs:1338`, `src/bin/compile_dump.rs:380`). All 19
backends are now covered (15 native + 4 wrappers via delegation). See
caveat the relevant row.

**Historical note on `loongarch64/reg_alloc_isel.rs`.** The previous
version of this section (pre-audit) described a
register-cache ISel on loongarch64 keyed by vreg mapping each live
vreg to one of `R12–R20` (caller-saved scratch) and `F0–F7` for floats,
spilling only at block boundaries, function-call boundaries, or under
>8-way register pressure. That module was 1 637 LOC. The audit found
that the module declaration was commented out at `loongarch64/mod.rs:6943`
and the production `allocate_registers` (`mod.rs:2619`) calls
`stack_slot_isel::allocate_registers`. The `reg_alloc_isel.rs` file
remains on disk as dead code (marked it for deletion in its, but the actual git commit only updated the comment
block — the file is still present and not compiled). Resurrection
path: `git show 1d83da8:src/codegen/src/loongarch64/reg_alloc_isel.rs`.
A future cleanup task should `git rm` the file to remove the 1.6 K LOC
of uncompilable code from the source tree.

---

## 4. Big-Endian Backends

VUMA ships four big-endian backends. Three are thin wrappers around a
little-endian parent; `ppc64` is natively big-endian. The fourth wrapper
(`ppc64le`) wraps a big-endian parent (`ppc64`) and produces
little-endian output.

### 4.0 Wrapper backends — byte-swap policy matrix (post-7-a/7-f)

| # | Wrapper | Wraps (`Sibling` constructor) | Instruction byte-swap | ELF header swap | `allocate_registers` delegation | Syscall status | Float-op verifier coverage |
|----|--------------|---------------------------------------------|-----------------------|-----------------------------|---------------------------------|-----------------------------------------------------------|----------------------------|
| 2 | `aarch64_be` | `AArch64Backend::new` | **None** (ARM ARM D6.1.3 — instr. fetches always LE) | `swap_le_elf_to_be` (header/PHDR/SHDR only) | one-line (`:150-151`) | **Works** (— `MOVZ X8, nr; SVC #0` at `arm64.rs:4758-4783`) | Central driver (removed AArch64-only `backend.rs:2748` site) |
| 9 | `armeb` | `Arm32Backend::new` | **LE→BE** (BE32 mode) | `swap_le_elf32_to_be` (ELF32 header/PHDR/SHDR + instr. words) | one-line (`:185-186`) | **Works** (— `MOV R7, nr; SVC #0` at `arm32/mod.rs:7350-7370`) | Central driver — parent `Arm32Backend` still skips |
| 11 | `mips64be` | `Mips64Backend::new_be` (parent emits BE header natively) | **LE→BE** (instr. words only — parent already BE on header) | None (parent already BE) | one-line (`:200-201`) | **Works** (-f — parent's Syscall arm at `mips64/mod.rs:3472-3532`; emits N64-ABI `LI V0, nr; SYSCALL; NOP; BEQ A3, Zero, +8; NOP; DSUBU V0, Zero, V0` sequence; 's "PENDING" claim is STALE) | Central driver |
| 13 | `ppc64le` | `PPC64Backend::new_le` (parent always ELFv2) | **BE→LE** (instr. words + `EI_DATA` `MSB→LSB` + header/PHDR/SHDR) | `swap_be_elf_to_le` (full) | one-line (`:404-409`) | **Works** (-f — parent's Syscall arm at `ppc64/mod.rs:4563`; emits PPC64 positive→negative-errno conversion `LI R0, nr; SC; BC 4, 3, +2; NEG R3, R3` sequence; 's "PENDING" claim is STALE) | Central driver |

**Notes:**
- "One-line" delegation means the wrapper's `allocate_registers` body is
 literally `self.inner.allocate_registers(func)` — the wrapper adds no
 register-allocation logic of its own.
- "Float-op verifier coverage" refers to `verify_function_float_ops`
 (`backend.rs:150`). Per, the verifier is now called
 **centrally in all 5 compilation drivers** (`src/main.rs`,
 `src/pipeline.rs`, `src/api.rs`, `src/bin/compile_dump.rs`) via
 `verify_program_float_ops` (`backend.rs:187`) — so all 4 wrappers are
 covered regardless of whether their parent's `allocate_registers`
 wires the verifier. The previous AArch64-only call site at
 `AArch64Backend::allocate_registers` (`backend.rs:2748`) has been
 removed. See caveat the relevant row.
- was the tracked TODO to implement `IRInstr::Syscall` on the
 `mips64` and `ppc64` parents. / -f landed this — both
 parents' Syscall arms are now fully implemented (`mips64/mod.rs:3472`,
 `ppc64/mod.rs:4563`), and `unimplemented!` has been removed from
 both files. The "PENDING" claim in 's survey is STALE.

### 4.1 `aarch64_be` — ELF-only swap, instructions stay LE
Per ARM ARM DDI 0487 §D6.1.3, AArch64 instruction fetches are always
little-endian regardless of `PSTATE.E`. `aarch64_be.rs:23-33` therefore
**forwards the parent's instruction bytes unchanged** and only swaps
the ELF header/PHDR/SHDR fields via `swap_le_elf_to_be`.

### 4.2 `armeb` — BE32 word-swap wrapper
ARMv7 BE32 mode requires each 4-byte instruction word stored
big-endian. `armeb.rs:13-27` byte-swaps every 4-byte instruction word
LE→BE inside `encode_function`, `return_stub`, `trampoline`, and the
executable `PT_LOAD` segment.

### 4.3 `mips64be` — instruction word swap, native BE ELF
The parent `mips64` backend emits a **big-endian ELF header**
natively (`build_mips64_elf_2seg`), so the wrapper only swaps the
32-bit instruction words in the `PT_LOAD` segment from LE to BE
(`mips64be.rs:8-25`).

### 4.4 `ppc64` — native big-endian
`ppc64/mod.rs` is implemented natively as a big-endian backend
(ELFv2 ABI, `ELFDATA2MSB`, `ppc64/mod.rs:1773`). All encoders write
4-byte big-endian words directly (`ppc64/mod.rs:422`). The `ppc64le`
wrapper (`ppc64le.rs`, 530 LOC) inherits `ppc64`'s encoders and only
flips the ELF header endianness back to LE.

---

## 5. wasm32 Special Handling

`wasm32/mod.rs` (8 150 LOC) is structurally different from every other
backend because WebAssembly requires **structured control flow** (no
arbitrary jumps). Four design decisions stand out:

### 5.1 Trampoline Loop
All IR basic blocks are nested inside a single `(loop $trampoline
(block $b_outer ... (block $b_inner (br_table $b_inner ... $b_outer
$trampoline))))` (`wasm32/mod.rs:2252-2300`). A `local $pc:i32` is
updated at every terminator; `br_table` dispatches to the right
nested block. `Break`/`Continue` map to `br` at the appropriate depth.
**ARCHITECTURAL, not a QEMU bug** — inline comment tagged
`[ARCH:wasm32-trampoline]` at `wasm32/mod.rs:2250` (`lower_function`
trampoline setup) and `:4107` (`lower_terminator_trampoline`).
WebAssembly's structured control flow permits no arbitrary
jump-to-label; VUMA's IR is a basic-block CFG with arbitrary successor
edges, so the trampoline emulates a computed goto. Works on every Wasm
runtime (wasmtime 47.0.2, wasmer, node.js). Performance cost: one
`local.set $pc` + one `br $trampoline` + one `br_table` dispatch per
branch. **No removal condition** — fundamental IR↔Wasm impedance
mismatch. Future optimisation: detect simple if-then-else patterns and
lower to native wasm `if/else` (partially done at
`lower_terminator_trampoline` `:4197`). See
`../caveats.md` the relevant row.

### 5.2 Ring-Buffer Channels
`channel_open` lowers to a heap-allocated 8-byte buffer holding
`{read_fd, write_fd}` (`ipc_lowering.rs:890-914`). On wasm32 there is
no `pipe2` syscall; the runner (`scripts/wasm32_runner.py`) provides
host-side `fdio` functions backed by a ring buffer in host memory
(see `../testing/overview.md`).

### 5.3 Fork Emulation
`vuma_fork` cannot `os.fork` because wasmtime runs background
threads that break the child's state (`wasm32_runner.py:111-117`).
Instead, `subprocess.Popen` spawns a fresh wasmtime instance pointing
at the same wasm module (`wasm32_runner.py:129-148`). The child's
return value is communicated via a fixed memory address:
`WASM32_CHILD_EXIT_ADDR = 4096` (`ipc_lowering.rs:961`).
`wasm32_fork_emulation_pass` (`ipc_lowering.rs:232`) rewrites the
child branch to `Store(exit_val, 4096); Jump(parent_post)` and
rewrites `wait_worker` to `Load(4096)`. The wasm32 child-branch code
is **dead** in the emitted binary — see
`../caveats.md` .1.

### 5.4 Function Table for `CallIndirect`
`IRInstr::CallIndirect` lowers to `WasmInstr::CallIndirect`
(`wasm32/mod.rs:4026-4059`). Each `GetAddress` of a function emits a
table-index relocation (`wasm32/mod.rs:2376, 2416-2420`); at module
finalisation, the function table is built and the relocations are
patched (`wasm32/mod.rs:4383, 4974, 5087`).

---

## 6. Per-Backend Quirks

Notable design decisions and QEMU workarounds, with file:line
references. The full QEMU workaround list is in
`../caveats.md`; only backend-specific items appear
here.

**aarch64** (`arm64.rs`). Reference backend. Float-op verifier
(`verify_function_float_ops`, `backend.rs:154`) was previously wired
ONLY inside `AArch64Backend::allocate_registers` at `backend.rs:2748`.
removed that AArch64-only call site and re-wired the verifier
**centrally** as `verify_program_float_ops(&ir_program)` (`backend.rs:187`)
in all 5 compilation drivers (`src/main.rs:1803`, `src/pipeline.rs:6904`
& `:7939`, `src/api.rs:1338`, `src/bin/compile_dump.rs:380`). All 19
backends (15 native + 4 wrappers via delegation) are now covered. The
`backend.rs:69-86` WIRING doc-comment was updated to reflect the central
call site. See caveat the relevant row.

**aarch64_be** (`aarch64_be.rs:23-44`). No instruction byte-swap
(ARM ARM D6.1.3); only ELF header fields flipped.

**x86_64** (`x86_64/mod.rs:934`). SIMD codegen is a stub; `emit_simd`
returns zero bytes pending SIMD integration.

**x86_32** (`x86_32/stack_slot_isel.rs:3410`). Syscall numbers
translated via `translate_or_warn` (x86_32 uses a separate table,
e.g. `read`=3 vs asm-generic `read`=63).

**riscv64 / riscv32** (`riscv64.rs:8360`, `riscv32.rs:5446`). Share
`riscv_common.rs` for encoding. riscv32 tests run with `qemu-riscv32
-cpu max` (QEMU's default rv32 CPU lacks the D extension,
`pi5_test_suite.sh:664-665`).

**loongarch64** (`loongarch64/mod.rs:2619`). Production
`allocate_registers` calls `stack_slot_isel::allocate_registers`
(same pattern as the other 15 stack-slot backends). The
`loongarch64/reg_alloc_isel.rs` file (1 637 LOC, formerly the only
register-cache ISel in VUMA) is **dead code** — the module declaration
is commented out at `loongarch64/mod.rs:6943`. marked the file
for deletion but the actual git commit only updated the comment
block; the file remains on disk as 1.6 K LOC of uncompilable code. A
future cleanup task should `git rm` it. See historical note.

**arm32** (`arm32/mod.rs:88-101`). `preregister_param_types` is
**load-bearing**: it pre-populates a thread-local map of function
parameter types before the parallel `allocate_registers` loop.
Without it, function A's `Call` handler can race on function B's
registration, fall back to "all-64-bit", and corrupt the calling
convention (32-bit params land in the wrong physical register).
Symptoms: `fn_chained_calls` returns 3 instead of 15
(`arm32/mod.rs:78-87`).

**armeb** (`armeb.rs:13-27`). BE32 word-swap (see .2).

**mips64 / mips64be** (`mips64/mod.rs:3906`, `mips64be.rs:8-25`).
Parent `mips64` emits a native BE ELF header; wrapper only swaps
instruction words.

**ppc64 / ppc64le** (`ppc64/mod.rs:2665-2680`, `ppc64le.rs`).
`ppc64` is natively big-endian (ELFv2 ABI). A pre-pass works around
a QEMU ppc64 bug where big-endian `LBUZ` (U8 load) silently returns
0; the pre-pass replaces every U8 array load with a 32-bit `LBZ`
plus explicit shift (`ppc64/mod.rs:2678-2680`). QEMU ppc64 also
reports `connect` and `poll` errors as **positive errno**
(`caveats.md`). `ppc64le` inherits `ppc64` encoders unchanged;
flips ELF header to LE only.

**wasm32** (`wasm32/mod.rs`). See .

**sparc64** (`sparc64.rs:2824, 4135, 4148, 4254`). Three open TODOs
for unsigned-comparison correction (F1d). Float-compare results are
"correct for same-sign non-NaN; TODO G5 otherwise" (`sparc64.rs:4254`).
QEMU sparc64 reports positive errno (`caveats.md`).

**s390x** (`s390x.rs:1911-1931`). **RESOLVED ** — the
secondary `IRInstr::Ret { values }` arm in `emit_instr` previously
emitted the function epilogue but **did not restore callee-saved
scratch registers S0–S5** (R6–R10/R12 in the ABI), causing corrupted
S0–S5 when `IRInstr::Ret` was emitted as a real instruction (e.g. via
`ipc_lowering.rs:758` which lowers `IRTerminator::Return` into an
`IRInstr::Ret` inside the emitted block). threaded
`s0_save_off..s5_save_off` (and `_frame_size`/`_lr_save_off`/`_fp_save_off`)
into `emit_instr` (`s390x.rs:1372-1380`) and now emits 6 `LG Sn, n(SP)`
restores before `adjust_sp` (`s390x.rs:1923-1928`), mirroring the
primary `IRTerminator::Return` path at `s390x.rs:1110-1130`. Verified
end-to-end on QEMU s390x via `functions/fibonacci.vuma` (-f):
recursive early-returns through the secondary Ret path preserve
callee-saved state across calls. QEMU s390x has known AGFI/AGHI
ambiguity (`caveats.md`).

**m68k** (`m68k.rs:2358, 2521, 3278, 3531, 3787, 4373`). Every FP emitter is
marked `// TODO G4: needs QEMU-m68k verification — encoding
uncertain`. Some are signed approximations of unsigned operations.
Two QEMU 7.2.0-m68k translator bugs are worked around with inline
comments tagged `[QEMU-WA:…]`: (a) **MOVEM SIGILL** at `m68k.rs:3787`
(primary comment) + 4 sites — `MOVEM.L Dn,-(SP)` / `MOVEM.L (SP)+,Dn`
rejected with "Disassembler disagrees with translator" SIGILL;
replaced with individual `MOVE.L` instructions. Removal: when QEMU 8.x
is the minimum supported version. (b) **ADDI.B/CMPI.B SIGILL** at
`m68k.rs:4373` (primary comment) + 3 sites — byte-form
immediate-to-register ops on `0x06xx`/`0x0Cxx` opcodes rejected;
replaced with `MOVEQ #imm, D0 + ADD.L/CMP.L D0, Dn`. Same removal
condition. See `../caveats.md` rows 9-10 and
`../caveats.md` item 7.

**alpha** (`alpha.rs:278, 1377, 1600`). f64→u64 truncation for `f ≥ 2^63`
is unimplemented (TODO G5b); the encoder emits a wrong result for
out-of-range inputs. One QEMU 7.2.0-alpha translator bug is worked
around at `alpha.rs:278` (`Instruction::encode` CMPULE special case)
with inline comment tagged `[QEMU-WA:alpha-cmpule]`: QEMU rejects INTA
function 0x3F (CMPULE) as a reserved encoding with SIGILL; emulated
via `CMPULT rb, ra, rc` + `XOR rc, 1, rc` (8 bytes instead of 4).
Breaks every `arena_wave*` test without the workaround. Removal: when
QEMU 8.x is the minimum supported version. See
`../caveats.md` the relevant row.

**hppa** (`hppa.rs:504, 552, 619, 660, 704, 1329, 3424, 3928, 4443, 4539`).
Tier-Scaffolded: Mul/Div/Cmp/conditional-branches emit stub code. F32
ops are stubs that store 0. FP load/store encodings are unverified
("TODO G3: replace with verified FLDW encoding", `hppa.rs:552`). The
conformance test uses `catch_unwind` to report this as "pending
" rather than failing (`backend.rs:4187-4191`). One QEMU
7.2.0-hppa translator bug is worked around at `hppa.rs:704`
(`ss_load_imm`) and `hppa.rs:4539` (`GetAddress` relocator) with
inline comment tagged `[QEMU-WA:hppa-ldil]`: QEMU's LDIL decoder
shifts left by 19 instead of 11, making the canonical `LDIL+LDO`
immediate-materialisation pair unusable; `ss_load_imm` materialises
32-bit immediates via `LDO` (format 14) + 11×`ADD` (left shift by 11)
+ `LDO` (add low 11 bits) instead. The `patch_call_site` far-call
fallback at `hppa.rs:1329` (Case 4 `BL,n` 17-bit displacement,
`[QEMU-WA:hppa-far-call]`) is NOT a QEMU bug — it is the standard
long-call codegen strategy for binaries > ~32 KB (no removal
condition). See `../caveats.md` rows 7-8.

---

## 7. Syscall ABI Translation

VUMA IR uses **asm-generic** (Linux generic syscall) numbers
internally. Each backend translates to its native numbering via
`syscall_abi::translate_or_warn(backend, generic_nr) -> u32`
(`syscall_abi.rs:281-300`).

**Identity arches** (no translation): `aarch64`, `riscv64`,
`riscv32`, `loongarch64`, `arm32`, `wasm32`. These return the input
verbatim.

**Translated arches**: `x86_64` (`syscall_abi.rs:304`),
`x86_32` (`:445`), `mips64` (`:583`), `ppc64` (`:728`), `s390x`
(`:870`), `sparc64` (`:1013`), `alpha` (`:1153`), `hppa` (`:1293`),
`m68k`. The MIPS, PPC, s390x, sparc64, alpha, and hppa tables differ
significantly from asm-generic (e.g. s390x `read`=3 and MIPS
`read`=5000 vs asm-generic `read`=63).

**Warning behaviour**: if `translate(backend, generic_nr)` returns
`None` (unknown syscall), `translate_or_warn` logs a `vuma_log!(warn,
...)` and returns the generic number verbatim
(`syscall_abi.rs:291-298`). This is **non-fatal**: the program is
still emitted, and the syscall may be wrong on the target arch.

**Production callers**: 16 of 19 backends call `translate_or_warn`
(arm32, arm64, alpha, x86_32, riscv32, riscv64, hppa, wasm32, ppc64,
mips64, loongarch64, m68k, s390x, sparc64, x86_64; plus two indirect
calls in `emit.rs:2188, 5386`). The four wrapper backends inherit
the parent's call. `vuma_log!` is a no-op in release builds
(`ive/src/lib.rs:53-60`), so warnings are **invisible in production
builds** — see `../caveats.md` .6. This corrects the
prior DOC-4 audit claim that `syscall_abi::translate` is dead code;
it is wrapped by `translate_or_warn` with 16+ production callers
(cf. `../caveats.md`).

---

## 8. Cross-references

- Allocation strategy & ISel: `../architecture/pipeline.md` –10.
- QEMU workarounds, syscall ABI, fork emulation:
 `../caveats.md` –6.
- Codegen caveats: `../caveats.md` .
- Test runner dispatch table: `../testing/overview.md` .
- Lean proof test harness (PMT.Test.*): `../testing/overview.md` .

---

## 9. Lean Formal Verification Scope

The PMT memory model that every VUMA backend implements — arena
allocation, `State<T>` read/write discipline, and the three trap
stubs — is formally verified in **Lean 4** under `proof/PMT/`. The
build is driven by Lake (`proof/lakefile.toml`, `proof/lean-toolchain`)
and surfaced via the top-level Makefile: `make proof` (build),
`make proof-check` (sorry-free check via `scripts/check-lean.sh`),
`make proof-test` (run the test harness, of testing overview).

**What the Lean proofs verify.** The proofs establish soundness of
the PMT *abstraction*: the IR-level semantics of `arena_new` /
`arena_alloc` / `arena_grow` / `arena_free`, the `State<T>`
load/store discipline, and the IVE-side rewrite rules
(`verify_state_reads`, `verify_state_writes`, `verify_transform`).
The verified artefacts are `proof/PMT/Soundness.lean`
(the `pmt_soundness` theorem), the IVE soundness files under
`proof/PMT/IVE/Soundness/` (`StateReads.lean`, `StateWrites.lean`,
`Transform.lean`), the simulation relation in
`proof/PMT/SimRel.lean` that links Lean's `Program` inductive to
Rust's lowered SCG IR, the Iris invariant bundle
`[cap_bnd] ∗ [live_mirror] ∗ [guard]` (`proof/PMT/Iris/Composition.lean`,
no `sorry`), the `BitVecArena` overflow model (`proof/PMT/BitVecArena.lean`),
the `MmapArena` allocator-failure model (`proof/PMT/MmapArena.lean`), and
the `PipelineSim` Lean↔Rust simulation (`proof/PMT/PipelineSim.lean`).
The library spans 20+ modules / ~90 theorems / 2 `sorry`s.

**In-tree verified checkers (`pmt-runtime-check` feature).** The
Lean-verified PMT checkers from `proof/PMT/Extraction.lean` are now
hand-translated into Rust at **`src/codegen/src/runtime/pmt_check.rs`** —
no longer living only under `proof/extracted/`. The translation is
*verified by parity test* (`tests/pmt_parity_test.rs`) rather than by
FFI extraction: each Rust function mirrors a Lean function that carries
a machine-checked soundness theorem, and the parity test asserts the two
agree on every test case (including the `u64::MAX + 1` overflow case
— see of the testing overview). Enabling the `pmt-runtime-check`
Cargo feature on `vuma-codegen` swaps the hand-written checkers in
`arena.rs` for this verified set:
`cargo build -p vuma-codegen --features pmt-runtime-check`. The feature
is off by default; the matrix `Formal` column therefore still reads
`PMT only` for all 19 backends until the feature is flipped on in the
release configuration.

**BitVec overflow model is more faithful than Lean `Nat`.** The Lean
operational semantics model sizes as `Nat`, which silently cannot
overflow. `proof/PMT/BitVecArena.lean` re-develops the arena model over
fixed-width bitvectors so that `used + size` can wrap. The Rust parity
translation follows the `BitVec` model —
`verified_capacity_check` uses `u64::checked_add` and returns `false` on
`None` — and is therefore *more* faithful to actual `usize` runtime
behaviour than the bare Lean `Nat` model. `parity_capacity_check_overflow`
in `tests/pmt_parity_test.rs` pins this behaviour.

**What the Lean proofs do NOT verify.** Each of the 19 backends
emits real machine code (or Wasm bytecode) via its own ISel and
ELF builder. **No backend is individually verified** — there is no
machine-code soundness proof per backend, no SMT verification of the
emitted encodings, and no proof that the four wrapper backends
(`aarch64_be`, `armeb`, `mips64be`, `ppc64le`) preserve the parent's
semantics under byte/word swapping. The matrix `Formal` column
therefore reads `PMT only` for all 19 backends.

**Runtime trap stubs ↔ Lean `TrapCode`.** Every backend emits three
named syscall-stub symbols that implement the runtime side of the
PMT safety invariants. These stubs are modeled in Lean as the
`TrapCode` inductive (`proof/PMT/Soundness.lean:90-94`) with a
`to_exit` evaluator (`proof/PMT/Soundness.lean:96-99`):

| Runtime stub (emitted by every backend) | Exit code | Lean `TrapCode` constructor | `TrapCode.to_exit` |
|----------------------------------------------------------|----------:|-----------------------------|-------------------:|
| `__arena_overflow` (`x86_64/mod.rs:3648-3654`, 18 siblings) | 1 | `TrapCode.arena_overflow` | 1 |
| `__oob_trap` (`x86_64/mod.rs:3657-3666`, 18 siblings) | 134 | `TrapCode.oob` | 134 |
| `__uaf_trap` (`x86_64/mod.rs:3669-3679`, 18 siblings) | 135 | `TrapCode.uaf` | 135 |

The exit codes (`1`, `134`, `135`) defined by `TrapCode.to_exit`
match the runtime stubs byte-for-byte. The full runtime-stub
inventory (one definition per backend, 19 copies of each stub) is
documented in `../proof/W2C-runtime.md`. The
audit's critical finding is that there is **no Rust `TrapCode`
enum** — `TrapCode` is Lean-only; the runtime uses named
exit-code stubs.

**Structural mismatch caveat.** The Lean `step` function
operates on pre-instrumentation IR, while Rust runs
post-instrumentation IR: the OOB and UAF checks are injected at
the SCG layer by `codegen/src/memory_safety.rs:965`
(`inject_bounds_check_ir`) **before** `step` runs. Lean therefore
never produces `Except.error TrapCode.oob` inside `step`; the trap
is realised by the injected `__oob_trap` stub at runtime. The
simulation relation in `proof/PMT/SimRel.lean` bridges
this gap by relating Lean's pre-instrumentation `Program` to
Rust's post-instrumentation SCG.

**Cross-references.**
- Runtime stub inventory (all 19 backends): `../proof/W2C-runtime.md`.
- Lean proof test harness (6 modules under `proof/PMT/Test/`): `../testing/overview.md` .
- PMT runtime-check parity test (`tests/pmt_parity_test.rs`, 5 tests):
 `../testing/overview.md` .
- Lake build integration: `../proof/W5-lake-build.md`,
 `../proof/W6-multi-module-test.md`.
- IVE soundness proofs vs op semantics:
 `proof/PMT/IVE/Soundness/{StateReads,StateWrites,Transform}.lean`.
- Iris invariants `[cap_bnd]`, `[live_mirror]`, `[guard]`:
 `proof/PMT/Iris/{CapBnd,LiveMirror,Guard}Invariant.lean`,
 `proof/PMT/Iris/Composition.lean`.
- Faithful IR model matching Rust: `../proof/W8-faithful-ir.md`.

---

## 10. QEMU Smoke Runner

`scripts/qemu_smoke_test.sh` builds the release `vuma` binary once and
then compiles a small set of gold-standard `.vuma` programs on every
supported backend (12 QEMU + wasm32 via wasmtime = 13 backends),
running each under the appropriate emulator and checking the exit code
against the `// Expected exit code:` header. The per-ISA QEMU/wasmtime
binary mapping lives in the `QEMU_BIN` associative array at the top of
the script; the test programs and expected exit codes are listed in the
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
  on the direct path (a known design limitation, not a correctness
  bug).
- The 6 untested backends (`aarch64_be`, `armeb`, `mips64be`,
  `ppc64le`, `x86_32`, `riscv32`) are either BE wrappers (inherit
  parent's correctness — see the byte-swap policy matrix) or 32-bit
  variants (share encoder tables with their 64-bit siblings).
- `mips64` smoke test requires installing `qemu-mips64el-static` (the
  LE MIPS64 emulator) since VUMA's `--isa mips64` emits a little-endian
  ELF; `qemu-mips64-static` (BE) rejects it. The `mips64be` backend
  exists in source but is not exposed via the `--isa` CLI flag.
