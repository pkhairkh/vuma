# VUMA Womb Kernel (VWK) — Wave-Based Engineering Spec

> **STATUS: ALL 13 WAVES COMPLETE (K0–K12) ✅**
>
> | Wave | Status | Commit |
> |------|--------|--------|
> | K0 — Arena State Model | ✅ Complete | 2038cb17 |
> | K1 — Kernel Scaffold + Boot | ✅ Complete | 334781b1 |
> | K2 — Memory Management | ✅ Complete | bc184767 |
> | K3 — Traps, IRQs, Syscall Dispatch | ✅ Complete | 6c8acce1 |
> | K4 — Process / Scheduler / Context Switch | ✅ Complete | ecccb330 |
> | K5 — VFS + tmpfs + initramfs | ✅ Complete | 018f352e |
> | K6 — TTY + Console + Char Devices + Shell | ✅ Complete | 87c7487c |
> | K7 — IPC: Pipe, Signal, Futex, SHM | ✅ Complete | 0cca1145 |
> | K8 — Sync Primitives + SMP IPI | ✅ Complete | 974addeb |
> | K9 — Network Stack | ✅ Complete | 89821141 |
> | K10 — Crypto Subsystem | ✅ Complete | 06c29667 |
> | K11 — Multi-Backend Parity Sweep | ✅ Complete (ALL 19 BACKENDS PASS) | ef1f0d15 |
> | K12 — Docs, Panic, kmsg, Power | ✅ Complete | b2d0da40 |
>
> **Results:** 75 PMT-pure kernel .vuma files · 190/190 gold tests pass ·
> 76/76 kernel module compiles pass · 19/19 backends pass · 3 compiler
> bugs fixed (if-expressions, nested layouts, arena bounds check) ·
> exit-code contract bug fixed · dead code cleaned · docs expanded
> from 34K to 69K words across 14 markdown files.

---

> **One-sentence pitch:** Build one VUMA source tree (`womb/kernel/**`) that compiles via `compile_dump kernel.vuma kernel.bin <backend>` into a bootable kernel for every supported backend, with all major kernel features expressed as PMT state transforms over `___pmt_buffer` and arena states, and raw-hardware concerns isolated behind typed `extern "C"` trampolines.

This spec is organized into **13 waves** (K0–K12). Each wave is **domain-scoped** and **code-specific** so multiple subagents can work in parallel (max 6 per wave). Every sub-task has a surgical contract, a checkable DoD, and a paste-ready dispatch prompt.

**No shortcuts. No stubs. No simplified solutions.** Every backend gets real syscall stubs. Every PMT transform is really verified. Raw hardware is reached only through typed FFI.

---

## 0. Sacred Invariants (non-negotiable)

1. **PMT-only in source.** `*T`, `&x`, `allocate`, `free` are hard parse errors in `womb/kernel/**`. Raw memory is reached only via `State<T>` field access or `extern "C"` trampolines.
2. **`___pmt_buffer` purity for static states.** Boot-critical fixed-size state (idle TCB, panic console, GDT/IDT templates) lives in `___pmt_buffer`. Runtime-grown structures live in mmap'd arenas.
3. **3 PMT verifiers stay canonical.** `StateRead`/`StateWrite`/`StateTransform` apply to kernel state. Arena bounds are runtime-checked (like `ffi_scratch`), never trusted.
4. **Zero per-state malloc/free.** Bump-alloc arenas + bulk `munmap` only. The kernel never has a use-after-free because there is no `free`.
5. **All 19 backends.** The kernel tree compiles for every backend. Backends without a realistic bare-metal target ship a **hosted mode** (Linux user-space simulator). No backend is left behind.
6. **One source.** A single `womb/kernel/**` tree, 19 ELF outputs, zero `#ifdef` in kernel logic. Per-arch code lives in `womb/kernel/arch/<isa>/` and is selected by the backend CLI arg.

---

## 1. Architecture Overview

### The four-layer cake

```
L4  VUMA-PMT kernel logic  (womb/kernel/**/*.vuma)
    schedulers, VFS, IPC, signals, syscalls, security
    Pure State<T> → State<U> transforms. Compiles to all 19 backends.

L3  Arena runtime  (Wave K0 + kernel arenas)
    arena_new / arena_alloc / arena_grow / arena_free
    Specialised: ProcessArena, PageArena, FdArena

L2  FFI trampolines  (extern "C" + #[borrow]/#[marshal]/#[foreign_consume])
    per-arch asm stubs: boot, trap entry, context switch, page-table walks, MMIO

L1  Per-arch boot & trap code  (womb/kernel/arch/<isa>/*.S)
    _start, vector table, TLB flush, cache ops. Hand-written. Linked into ELF.
```

### Boot flow

```
power-on → L1 _start (per-arch .S)
  → set sp, zero BSS, read bootinfo (multiboot2/devicetree/SBI)
  → call main() → kmain(bootinfo: State<BootInfo>)   ← enters PMT
L4 kmain:
  arch_early_init → console_init → pmm_init → vmm_init
  → trap_init → sched_init → vfs_init → spawn_init → sched_run (never returns)
```

### Target backends

| Backend | Mode | QEMU target |
|---|---|---|
| x86_64, aarch64, riscv64 | bare-metal (v1 sprint) | qemu-system-\<arch\> |
| ppc64le, loongarch64, arm32, s390x, mips64, sparc64 | bare-metal (K11) | qemu-system-\<arch\> |
| wasm32, alpha, hppa, m68k, x86_32, aarch64_be, armeb, mips64be, ppc64 | hosted (K11) | Linux process |

---

## 2. Subagent Protocol

Every subagent MUST follow this protocol. The orchestrator pastes the relevant Dispatch Box into the subagent prompt.

**Repo root:** `/home/z/vuma` (cd here before every command).
**Worklog:** `/home/z/my-project/worklog.md` — READ before starting, APPEND your section when done:
```
---
Task ID: <e.g. K2a>
Agent: <agent name>
Task: <one-line>
Work Log:
- <step>
Stage Summary:
- <result>
```
**Build:** `cargo build --profile release-fast --bin compile_dump 2>&1 | tail -5`
**Test binary:** `./target/release-fast/compile_dump <input.vuma> <out.bin> <backend> [--verify]`
**Rules:**
- No stubs. No shortcuts. Fix root causes.
- PMT-only: no `allocate`/`free`/`*ptr`/`&x` in `womb/kernel/**`.
- Touch ONLY the files listed in your task's "Files" line.
- `CARGO_BUILD_JOBS=1` to avoid OOM.
- Commit per task: `git commit -m "Wave KX<sub>: <title>"`. Do NOT push (orchestrator pushes).
- If blocked, append a blocker note to worklog and stop.

---

## 3. File-Ownership Map (global, by domain)

| Domain | Files | Waves |
|---|---|---|
| Parser/AST | `src/parser/src/{ast,parser}.rs` | K0 |
| SCG | `src/scg/src/{node,serialize,structured_output}.rs`, `src/parser/src/to_scg.rs` | K0 |
| Codegen bridge | `src/pipeline.rs`, `src/codegen/src/scg_to_ir.rs` | K0, K2, K3 |
| IVE | `src/ive/src/{lib,invariant_aggregator,arena_bounds}.rs` | K0 |
| Runtime | `src/codegen/src/runtime/{mod,arena}.rs` | K0 |
| Backends | `src/codegen/src/<backend>/*.rs` | K0, K11 |
| Kernel entry | `womb/kernel/{kernel,kmain}.vuma` | K1 |
| Kernel arch | `womb/kernel/arch/<isa>/*.{S,vuma}` | K1–K8, K11 |
| Kernel mm | `womb/kernel/mm/*.vuma` | K2 |
| Kernel trap | `womb/kernel/trap/*.vuma` | K3 |
| Kernel syscall | `womb/kernel/syscall/*.vuma` | K3 |
| Kernel proc | `womb/kernel/proc/*.vuma` | K4 |
| Kernel vfs | `womb/kernel/vfs/*.vuma` | K5 |
| Kernel fs | `womb/kernel/fs/*.vuma` | K5 |
| Kernel drivers | `womb/kernel/drivers/*.vuma` | K6, K9 |
| Kernel tty | `womb/kernel/tty/*.vuma` | K6 |
| Kernel ipc | `womb/kernel/ipc/*.vuma` | K7 |
| Kernel sync | `womb/kernel/sync/*.vuma` | K8 |
| Kernel smp | `womb/kernel/smp/*.vuma` | K8 |
| Kernel net | `womb/kernel/net/*.vuma` | K9 |
| Kernel crypto | `womb/kernel/crypto/*.vuma` | K10 |
| Kernel panic | `womb/kernel/panic/*.vuma` | K12 |
| Kernel power | `womb/kernel/power/*.vuma` | K12 |
| Docs | `docs/kernel-*.md` | K12 |
| Tests | `tests/gold_standard/kernel_*/` | K1–K12 |

---

## 4. Wave Dependency Graph

```
K0 (arena) ──► K1 (boot) ──► K2 (mm) ──┬──► K3 (trap/syscall) ──► K4 (proc/sched)
                                        │                            │
                                        ├──► K5 (vfs) ◄──────────────┤
                                        ├──► K6 (tty) ◄──────────────┤
                                        ├──► K7 (ipc) ◄──────────────┤
                                        └──► K8 (sync/smp) ◄─────────┘
                                                                     │
                                   K9 (net) ◄────────────────────────┤
                                   K10 (crypto) ◄────────────────────┤
                                   K11 (parity) ◄────────────────────┘
                                   K12 (docs/panic/power) ◄── all
```

**Max 6 subagents per wave.** Sequential dependencies inside a wave are noted per task.

---

## 5. How to Dispatch

1. Pick the next unblocked wave (all its dependencies have DoD checkboxes ticked in worklog).
2. Read the wave's Dispatch Box.
3. Launch up to 6 subagents IN ONE MESSAGE, one per task ID, each receiving the Common Preamble + their per-task contract.
4. Wait for all to return. Verify each DoD.
5. If any task failed, re-dispatch it with the failure note appended.
6. Once all DoD pass, tick the wave's DoD in worklog and move to the next wave.

---

## Wave K0 — Arena State Model (prerequisite blocker)

**Goal:** Add `arena_new`/`arena_alloc`/`arena_grow`/`arena_free` as PMT-pure builtins lowering to `mmap`/`mremap`/`munmap`, with IVE bounds checking, on all 19 backends.
**Depends on:** nothing. **Max parallel:** 3 (K0a→K0b sequential; K0c∥K0d∥K0e; K0f after all).

| Task | Files |
|---|---|
| K0a | `src/parser/src/{ast,parser}.rs`, `tests/gold_standard/arena_wave0/` |
| K0b | `src/scg/src/{node,serialize,structured_output}.rs`, `src/parser/src/to_scg.rs`, `src/{bd,cor,vuma}/src/*.rs`, `src/pipeline.rs` |
| K0c | `src/pipeline.rs`, `src/codegen/src/scg_to_ir.rs` |
| K0d | `src/codegen/src/runtime/{mod,arena}.rs` |
| K0e | `src/ive/src/{lib,invariant_aggregator,arena_bounds}.rs` |
| K0f | all 19 `src/codegen/src/<backend>/*.rs`, `tests/gold_standard/arena_wave1/`, `docs/{language-reference,architecture}.md` |

### K0a — Parser/AST: arena builtins
**Contract:** Add 4 `Expr` variants (`ArenaNew{capacity}`, `ArenaAlloc{arena,layout_name}`, `ArenaGrow{arena,min_capacity}`, `ArenaFree{arena}`) mirroring `StateInit`. Intercept the 4 builtins in `parse_postfix` after `state_new` (~line 2476). `infer_type` returns `State<Arena>` for New/Grow/Free, `State<T>` for Alloc. Add 3 parse-only tests in `arena_wave0/`: `arena_new_parse`, `arena_alloc_parse`, `arena_grow_free_parse`.
**DoD:**
- [ ] 4 variants with `Display` + `span` + `infer_type`
- [ ] Parser intercepts all 4 builtins
- [ ] 3 parse-only tests pass; `cargo build` clean; existing tests pass

### K0b — SCG nodes: arena operations
**Contract:** Add `NodeType::{ArenaNew,ArenaAlloc,ArenaGrow,ArenaFree}` + `NodePayload::Arena*` (mirror `StateTransform`). Add structs: `ArenaNewNode{capacity_vreg,result_vreg}`, `ArenaAllocNode{arena_vreg,layout_name,result_arena_vreg,result_state_vreg}`, `ArenaGrowNode{arena_vreg,min_capacity_vreg,result_vreg}`, `ArenaFreeNode{arena_vreg}`. Serialization tags 20–23. Wire `to_scg.rs` to emit arena nodes. Add arena arms to every exhaustive match in bd/cor/vuma/pipeline (phantom BD, `NodeKind::Memory`).
**DoD:**
- [ ] 4 `NodeType` + `NodePayload` variants with `Display`
- [ ] Tags 20–23 round-trip; `to_scg.rs` emits arena nodes
- [ ] All exhaustive matches updated; `cargo build` clean

### K0c — Codegen bridge: arena lowering
**Contract:** Lower `Expr::Arena*` to IR via existing `CallNode` (mmap/mremap/munmap) + `Load`/`Store`/`BinOp`:
- `arena_new(cap)`: call `mmap(0, cap, 3, 0x22, -1, 0)` → base; alloc Arena struct (24B: base u64 + offset u64 + capacity u64); store base, offset=0, capacity; return arena.
- `arena_alloc(arena, Layout)`: load base/offset/capacity; bounds-check `offset+size > cap` → call `__arena_overflow`; ptr=base+offset; store `offset += layout_size`; return (arena, ptr).
- `arena_grow(arena, min_cap)`: load base/cap; call `mremap(base, cap, min_cap, 1)`; store base/cap.
- `arena_free(arena)`: load base/cap; call `munmap(base, cap)`.
**DoD:**
- [ ] All 4 builtins lower to valid IR
- [ ] Arena states support field access (`w.x` works)
- [ ] Bounds check present at every `arena_alloc`
- [ ] `compile_dump arena_basic.vuma /tmp/a.bin x86_64` succeeds

### K0d — Runtime arena module
**Contract:** Create `src/codegen/src/runtime/arena.rs` with Rust-level arena allocator for testing + callback path:
```rust
pub struct Arena { base: *mut u8, offset: usize, capacity: usize }
pub fn arena_create(capacity: usize) -> Arena;
pub fn arena_alloc<T>(arena: &mut Arena) -> *mut T;  // panic on overflow
pub fn arena_grow(arena: &mut Arena, min_capacity: usize);
pub fn arena_destroy(arena: Arena);
```
Declare `pub mod arena;` in `runtime/mod.rs`. 5+ unit tests: create, alloc, grow, destroy, bounds_check_overflow.
**DoD:**
- [ ] `runtime/arena.rs` exists with Arena struct + 4 functions
- [ ] `cargo test -p vuma-codegen arena` passes (5+ tests)

### K0e — IVE arena bounds verifier
**Contract:** Create `src/ive/src/arena_bounds.rs` verifier that checks every `ArenaAlloc` node has a preceding bounds check (`offset + layout_size <= capacity`). Register `pub mod arena_bounds;` in `lib.rs`. Wire `invariant_aggregator.rs` to treat `ArenaAlloc`'s arena input as consumed (linearity, same as `StateTransform`). 3+ unit tests: bounds_present, bounds_missing, linearity.
**DoD:**
- [ ] `arena_bounds.rs` exists; `lib.rs` declares it
- [ ] `invariant_aggregator.rs` tracks ArenaAlloc linearity
- [ ] 3+ unit tests pass

### K0f — Syscall stubs + tests + docs
**Contract:** For each of 19 backends, verify `mmap`/`mremap`/`munmap` stubs exist in `func_offsets`; add `__arena_overflow` stub (real `exit(1)` syscall, NOT a no-op). Per-arch exit code:
- x86_64: `mov eax,60; mov edi,1; syscall; int3`
- x86_32: `mov eax,1; mov ebx,1; int 0x80; int3`
- aarch64: `movz x0,#1; movz x8,#93; svc #0; brk #0`
- riscv64/32: `li a0,1; li a7,93; ecall; unimp`
- arm32: `mov r0,#1; mov r7,#1; svc #0; bkpt`
- mips64: `li a0,1; li v0,5058; syscall; break`
- ppc64/ppc64le: `li r3,1; li r0,1; sc; trap`
- loongarch64: `li a0,1; li a7,93; syscall; break 0`
- s390x: `lghi r2,1; lghi r1,1; svc 0; trap`
- sparc64: `mov 1,%o0; mov 1,%g1; ta 0x6d; unimp`
- alpha: `lda v0,1; callsys; call_pal 0x83; unop`
- hppa: `li r26,1; li r20,1; gate; bv; nop`
- m68k: `moveq #1,d1; moveq #1,d0; trap #0; illegal`
- wasm32: `proc_exit(1)` (WASI)
Add 4 tests in `arena_wave1/`: `arena_basic` (exit 0), `arena_grow` (exit 0), `arena_multiple` (exit 0), `arena_overflow` (exit 1). Add §15 Arena States to `language-reference.md`, §10 Arena State Model to `architecture.md`.
**DoD:**
- [ ] 4 stubs registered on all 19 backends
- [ ] `arena_basic.vuma` passes on all 19 backends
- [ ] `arena_overflow.vuma` exits 1 (trap)
- [ ] Docs updated; existing 704+ gold tests still pass

### Dispatch Box — Wave K0

```
=== COMMON PREAMBLE (Wave K0) ===
You are Wave K0 of the VWK effort. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md. APPEND your section when done.
REPO: /home/z/vuma (cd here before commands).
BUILD: cargo build --profile release-fast --bin compile_dump 2>&1 | tail -5
TEST: ./target/release-fast/compile_dump <in.vuma> <out.bin> <backend> [--verify]
RULES: No stubs. PMT-only. CARGO_BUILD_JOBS=1. Commit per task, do NOT push.

=== K0a: Parser/AST arena builtins ===
Files: src/parser/src/{ast,parser}.rs, tests/gold_standard/arena_wave0/ (new).
Add 4 Expr::Arena* variants mirroring StateInit. Intercept in parse_postfix
after state_new (~line 2476). infer_type: State<Arena> for New/Grow/Free,
State<T> for Alloc. Add 3 parse-only tests in arena_wave0/.
DoD: 4 variants+Display+span+infer_type; parser intercepts; 3 tests pass;
cargo build clean; existing tests pass.

=== K0b: SCG arena nodes ===
[after K0a] Files: src/scg/src/{node,serialize,structured_output}.rs,
src/parser/src/to_scg.rs, src/{bd,cor,vuma}/src/*.rs, src/pipeline.rs.
Add NodeType::{ArenaNew,ArenaAlloc,ArenaGrow,ArenaFree}+NodePayload::Arena*
(mirror StateTransform). Serialize tags 20-23. Wire to_scg.rs. Add arms to
every exhaustive match (phantom BD, NodeKind::Memory).
DoD: 4 NodeType+Payload; tags round-trip; to_scg emits; all matches updated.

=== K0c: Codegen bridge arena lowering ===
[after K0b] Files: src/pipeline.rs, src/codegen/src/scg_to_ir.rs.
Lower Expr::Arena* to CallNode(mmap/mremap/munmap)+Load/Store/BinOp. See
wave contract for exact lowering. Bounds check (trap on overflow) at every
arena_alloc.
DoD: 4 builtins lower to valid IR; field access works; bounds check present;
arena_basic.vuma compiles on x86_64.

=== K0d: Runtime arena module ===
[parallel with K0c] Files: src/codegen/src/runtime/{mod,arena}.rs (new).
Rust-level Arena struct + arena_create/alloc<T>/grow/destroy. 5+ unit tests.
DoD: cargo test -p vuma-codegen arena passes (5+ tests).

=== K0e: IVE arena bounds verifier ===
[parallel with K0c] Files: src/ive/src/{lib,invariant_aggregator,arena_bounds}.rs.
Create arena_bounds.rs verifier. Register in lib.rs. Wire invariant_aggregator
for ArenaAlloc linearity. 3+ unit tests.
DoD: verifier exists; linearity tracked; 3+ tests pass.

=== K0f: Syscall stubs + tests + docs ===
[after K0c+K0d+K0e] Files: all 19 backend .rs files, tests/gold_standard/
arena_wave1/, docs/{language-reference,architecture}.md.
Verify mmap/mremap/munmap on all 19 backends. Add __arena_overflow stub (real
exit(1) syscall) per arch — see wave contract for per-arch code. Add 4 arena
tests. Update docs §15 + §10.
DoD: 4 stubs on 19 backends; arena_basic passes on 19; overflow traps; docs
updated; existing 704+ tests pass.
```

---

## Wave K1 — Kernel Scaffold + Boot Contract

**Goal:** Produce a QEMU-bootable ELF that prints `vuma kernel: hello` on the serial console and halts, on x86_64 + aarch64 + riscv64.
**Depends on:** K0. **Max parallel:** 5.

| Task | Files |
|---|---|
| K1a | `womb/kernel/arch/x86_64/{boot.S,bootinfo.vuma,trampoline.vuma}`, `womb/kernel/arch/x86_64/linker.ld` |
| K1b | `womb/kernel/arch/aarch64/{boot.S,bootinfo.vuma,trampoline.vuma}`, `womb/kernel/arch/aarch64/linker.ld` |
| K1c | `womb/kernel/arch/riscv64/{boot.S,bootinfo.vuma,trampoline.vuma}`, `womb/kernel/arch/riscv64/linker.ld` |
| K1d | `womb/kernel/{kernel.vuma,kmain.vuma,console.vuma}` |
| K1e | `tests/gold_standard/kernel_boot/`, `scripts/kernel_smoke.sh` |

### K1a — x86_64 boot + bootinfo
**Contract:** Write `boot.S` (multiboot2 header, `_start`: set sp=0x90000, zero BSS, read multiboot2 magic in eax, call `main`). Write `bootinfo.vuma` with `layout BootInfo = { magic: u32, mem_lower: u32, mem_upper: u32, cmdline: Address }`. Write `trampoline.vuma` with `extern "C"` block declaring `outb(port,val)`, `inb(port)`, `halt()`. Write `linker.ld` (entry=_start, .text@1MB, .bss@0x90000). The boot.S must call `main` which is the VUMA entry point.
**DoD:**
- [ ] `boot.S` assembles cleanly with `as`
- [ ] `linker.ld` links with `ld -T linker.ld`
- [ ] `bootinfo.vuma` parses with `compile_dump --verify`
- [ ] `trampoline.vuma` externs resolve on x86_64 backend

### K1b — aarch64 boot + bootinfo
**Contract:** Write `boot.S` (QEMU virt entry @0x40080000: set sp, zero BSS, read devicetree pointer in x0, call `main`). `bootinfo.vuma`: `layout BootInfo = { dtb_addr: u64, mem_start: u64, mem_size: u64 }`. `trampoline.vuma`: externs for PL011 UART (`put32`, `get32`, `uart_putc`). `linker.ld` (entry=_start, .text@0x40080000).
**DoD:**
- [ ] `boot.S` assembles with `aarch64-linux-gnu-as`
- [ ] Links with `aarch64-linux-gnu-ld -T linker.ld`
- [ ] `bootinfo.vuma` + `trampoline.vuma` parse + verify on aarch64

### K1c — riscv64 boot + bootinfo
**Contract:** Write `boot.S` (QEMU virt entry @0x80200000: set sp, zero BSS, read SBI hartid in a0, dtb in a1, call `main`). `bootinfo.vuma`: `layout BootInfo = { hartid: u64, dtb_addr: u64, mem_start: u64, mem_size: u64 }`. `trampoline.vuma`: externs for SBI console putc, HTIF. `linker.ld` (entry=_start, .text@0x80200000).
**DoD:**
- [ ] `boot.S` assembles with `riscv64-linux-gnu-as`
- [ ] Links with `riscv64-linux-gnu-ld -T linker.ld`
- [ ] `bootinfo.vuma` + `trampoline.vuma` parse + verify on riscv64

### K1d — kernel.vuma + kmain.vuma + console.vuma
**Contract:** `kernel.vuma` is the entry: `fn main() -> i32 { kmain(); return 0; }`. `kmain.vuma`: `fn kmain()` that calls `arch_early_init()`, `console_init()`, prints `vuma kernel: hello\n`, then `halt()`. `console.vuma`: `layout Console = { base: u64, pos: u32 }`, `fn console_init() -> State<Console>`, `fn console_putc(c: u8)`, `fn console_puts(s: Address)`. Use `extern "C"` trampolines from `trampoline.vuma` for the actual UART writes. PMT-only: no `*ptr`, use `State<T>` field access.
**DoD:**
- [ ] `compile_dump kernel.vuma /tmp/k.bin x86_64 --verify` succeeds (IVE pass)
- [ ] Same for aarch64, riscv64
- [ ] No pointer syntax in any `.vuma` file
- [ ] `console_puts` works on a `State<Console>`

### K1e — Smoke test harness
**Contract:** Write `scripts/kernel_smoke.sh` that: builds `compile_dump`, compiles `kernel.vuma` for x86_64+aarch64+riscv64, runs each under QEMU (`qemu-system-<arch> -kernel k.bin -nographic -serial mon:stdio`), greps output for `vuma kernel: hello`, exits 0 on match / 1 on miss. Add `tests/gold_standard/kernel_boot/hello.expected` with the expected output.
**DoD:**
- [ ] `scripts/kernel_smoke.sh` exits 0 on x86_64
- [ ] Same for aarch64, riscv64
- [ ] Test registered in the gold-standard manifest

### Dispatch Box — Wave K1

```
=== COMMON PREAMBLE (Wave K1) ===
You are Wave K1 of the VWK effort. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md (K0 must be complete). APPEND when done.
REPO: /home/z/vuma. BUILD: cargo build --profile release-fast --bin compile_dump.
TEST: ./target/release-fast/compile_dump kernel.vuma /tmp/k.bin <backend> --verify
RULES: No stubs. PMT-only. CARGO_BUILD_JOBS=1. Commit per task, do NOT push.
ARCH TOOLS: x86_64 uses `as`+`ld`; aarch64 uses `aarch64-linux-gnu-as`+`ld`;
riscv64 uses `riscv64-linux-gnu-as`+`ld`. If a cross-assembler is missing,
append a blocker note to worklog and stop.

=== K1a: x86_64 boot + bootinfo ===
Files: womb/kernel/arch/x86_64/{boot.S,bootinfo.vuma,trampoline.vuma,linker.ld}.
boot.S: multiboot2 header + _start (set sp=0x90000, zero BSS, call main).
bootinfo.vuma: layout BootInfo{magic:u32,mem_lower:u32,mem_upper:u32,cmdline:Address}.
trampoline.vuma: extern outb/inb/halt. linker.ld: entry=_start,.text@1MB,.bss@0x90000.
DoD: boot.S assembles; linker.ld links; .vuma files parse+verify on x86_64.

=== K1b: aarch64 boot + bootinfo ===
Files: womb/kernel/arch/aarch64/{boot.S,bootinfo.vuma,trampoline.vuma,linker.ld}.
boot.S: QEMU virt entry @0x40080000, set sp, zero BSS, read x0=dtb, call main.
bootinfo.vuma: layout BootInfo{dtb_addr:u64,mem_start:u64,mem_size:u64}.
trampoline.vuma: extern put32/get32/uart_putc. linker.ld: .text@0x40080000.
DoD: boot.S assembles (aarch64-linux-gnu-as); links; .vuma verify on aarch64.

=== K1c: riscv64 boot + bootinfo ===
Files: womb/kernel/arch/riscv64/{boot.S,bootinfo.vuma,trampoline.vuma,linker.ld}.
boot.S: QEMU virt entry @0x80200000, set sp, zero BSS, a0=hartid a1=dtb, call main.
bootinfo.vuma: layout BootInfo{hartid:u64,dtb_addr:u64,mem_start:u64,mem_size:u64}.
trampoline.vuma: extern sbi_console_putc. linker.ld: .text@0x80200000.
DoD: boot.S assembles (riscv64-linux-gnu-as); links; .vuma verify on riscv64.

=== K1d: kernel.vuma + kmain.vuma + console.vuma ===
Files: womb/kernel/{kernel,kmain,console}.vuma.
kernel.vuma: fn main(){kmain();return 0;}. kmain.vuma: arch_early_init,
console_init, print "vuma kernel: hello\n", halt. console.vuma: layout
Console{base:u64,pos:u32}, console_init/putc/puts. Use State<T> field access
ONLY — no *ptr. Use extern trampolines for UART writes.
DoD: compiles+verifies on x86_64,aarch64,riscv64; no pointer syntax; puts works.

=== K1e: Smoke test harness ===
Files: scripts/kernel_smoke.sh, tests/gold_standard/kernel_boot/hello.expected.
Build compile_dump, compile kernel.vuma for 3 archs, run under QEMU, grep for
"vuma kernel: hello", exit 0 on match. Register in gold-standard manifest.
DoD: kernel_smoke.sh exits 0 on x86_64+aarch64+riscv64.
```

---

## Wave K2 — Memory Management (PMM + VMM)

**Goal:** Buddy page-frame allocator + page-table walk as PMT transforms, with per-arch PTE layouts and TLB trampolines. `kmalloc(4096)` returns a writable page; double-map is a compile error.
**Depends on:** K0, K1. **Max parallel:** 6.

| Task | Files |
|---|---|
| K2a | `womb/kernel/mm/pmm.vuma` |
| K2b | `womb/kernel/mm/vmm.vuma` |
| K2c | `womb/kernel/arch/x86_64/pt.vuma`, `womb/kernel/arch/x86_64/mm_trampoline.vuma` |
| K2d | `womb/kernel/arch/aarch64/pt.vuma`, `womb/kernel/arch/aarch64/mm_trampoline.vuma` |
| K2e | `womb/kernel/arch/riscv64/pt.vuma`, `womb/kernel/arch/riscv64/mm_trampoline.vuma` |
| K2f | `womb/kernel/mm/kmalloc.vuma`, `womb/kernel/mm/mmap.vuma` |

### K2a — PMM buddy allocator
**Contract:** `layout PageFrame = { base: u64, order: u8, free: bool }`. `layout PmmState = { arena: State<Arena>, free_lists: [u64; 11] }` (orders 0–10, 4KB–4MB). Functions: `pmm_init(arena: State<Arena>, mem_start: u64, mem_size: u64) -> State<PmmState>`, `pmm_alloc(pmm: State<PmmState>, order: u8) -> u64` (returns physical addr, splits buddy), `pmm_free(pmm: State<PmmState>, addr: u64, order: u8)` (coalesces buddy). All state via `State<T>` field access on `PmmState`.
**DoD:**
- [ ] `pmm_init` + `pmm_alloc` + `pmm_free` parse + verify on x86_64
- [ ] Allocating 2 pages of order 0 then freeing coalesces to order 1
- [ ] No pointer syntax

### K2b — VMM page-table walk
**Contract:** `layout PageTable = { root_phys: u64, levels: u8 }`. `transform vmm_map(pt: State<PageTable>, vaddr: u64, paddr: u64, flags: u64) -> State<PageTable>` — walks levels, allocates missing tables via PMM, sets PTE. `transform vmm_unmap(pt: State<PageTable>, vaddr: u64) -> State<PageTable>`. `fn vmm_translate(pt: State<PageTable>, vaddr: u64) -> u64`. PTE access via `extern "C"` trampolines (`pte_read`, `pte_write`, `pte_set_atomic`) with `#[borrow]` on the page table.
**DoD:**
- [ ] `vmm_map` + `vmm_translate` round-trip parse + verify
- [ ] Double-map of same vaddr is a `StateWrite` linearity error (compile-time)
- [ ] `vmm_unmap` then read is a `StateRead` error (compile-time)

### K2c — x86_64 PTE + MM trampolines
**Contract:** `layout PageTableEntry = { present: u8, rw: u8, us: u8, pwt: u8, pcd: u8, addr: u64, nx: u8 }` (pack into 8 bytes per Intel SDM). `extern "C"` block: `#[borrow] fn pte_read(pt: State<PageTable>, idx: u64) -> u64`, `#[borrow] fn pte_write(pt: State<PageTable>, idx: u64, val: u64)`, `fn invlpg(vaddr: u64)`, `fn tlb_flush()`, `fn cr3_read() -> u64`, `fn cr3_write(val: u64)`. 4-level paging (PML4→PDPT→PD→PT).
**DoD:**
- [ ] PTE layout is 8 bytes, fields pack correctly
- [ ] All 6 trampolines resolve on x86_64 backend
- [ ] `vmm_map` using these trampolines verifies

### K2d — aarch64 PTE + MM trampolines
**Contract:** `layout PageTableEntry = { valid: u8, page: u8, af: u8, sh: u8, attr: u8, addr: u64, nx: u8 }` (pack into 8 bytes per ARMv8 ARM). Trampolines: `pte_read`, `pte_write`, `tlbi_vmalle1()`, `ttbr0_el1_read() -> u64`, `ttbr0_el1_write(val: u64)`, `dsb()`, `isb()`. 4-level paging (PGD→PUD→PMD->PTE), 4KB granule.
**DoD:**
- [ ] PTE layout is 8 bytes, ARMv8 format
- [ ] All trampolines resolve on aarch64 backend
- [ ] `vmm_map` verifies on aarch64

### K2e — riscv64 PTE + MM trampolines
**Contract:** `layout PageTableEntry = { valid: u8, r: u8, w: u8, x: u8, u: u8, g: u8, a: u8, d: u8, addr: u64 }` (pack into 8 bytes per RISC-V Sv39 spec). Trampolines: `pte_read`, `pte_write`, `sfence_vma(vaddr: u64, asid: u64)`, `satp_read() -> u64`, `satp_write(val: u64)`. Sv39 paging (3 levels).
**DoD:**
- [ ] PTE layout is 8 bytes, Sv39 format
- [ ] All trampolines resolve on riscv64 backend
- [ ] `vmm_map` verifies on riscv64

### K2f — kmalloc + mmap syscall
**Contract:** `kmalloc.vuma`: `fn kmalloc(size: u64) -> u64` — slab over `PageArena`, returns kernel virtual addr. `fn kfree(addr: u64)` — returns slot to slab. `mmap.vuma`: `transform sys_mmap(regions: State<RegionTable>, addr: u64, len: u64, prot: u64, flags: u64) -> State<RegionTable>` — finds free vaddr range, maps pages via VMM, returns updated region table. `layout RegionTable = { arena: State<Arena>, count: u32, regions: [Region; 64] }`.
**DoD:**
- [ ] `kmalloc(4096)` returns a writable address; `kmalloc(1<<20)` triggers `arena_grow`
- [ ] `sys_mmap` verifies; double-map is a compile error
- [ ] All 3 archs compile `kmalloc.vuma` + `mmap.vuma`

### Dispatch Box — Wave K2

```
=== COMMON PREAMBLE (Wave K2) ===
You are Wave K2 of VWK. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md (K0+K1 must be complete). APPEND when done.
REPO: /home/z/vuma. BUILD: cargo build --profile release-fast --bin compile_dump.
TEST: ./target/release-fast/compile_dump <in.vuma> /tmp/o.bin <backend> --verify
RULES: No stubs. PMT-only (no *ptr/&x/allocate/free). CARGO_BUILD_JOBS=1.
Commit per task, do NOT push. Read womb/kernel/arch/<isa>/bootinfo.vuma from
K1 for the BootInfo layout. Read womb/kernel/mm/ from sibling tasks before
writing cross-references.

=== K2a: PMM buddy allocator ===
Files: womb/kernel/mm/pmm.vuma.
layout PageFrame{base:u64,order:u8,free:bool}. layout PmmState{arena:State<Arena>,
free_lists:[u64;11]}. pmm_init/alloc(order)/free(addr,order). Buddy split+coalesce.
All state via State<T> field access.
DoD: pmm_init+alloc+free verify on x86_64; coalescing works; no pointer syntax.

=== K2b: VMM page-table walk ===
Files: womb/kernel/mm/vmm.vuma.
layout PageTable{root_phys:u64,levels:u8}. transform vmm_map(pt,vaddr,paddr,flags)
-> State<PageTable>. transform vmm_unmap(pt,vaddr)->State<PageTable>.
vmm_translate(pt,vaddr)->u64. PTE access via extern #[borrow] trampolines.
DoD: map+translate round-trip verify; double-map is compile error; unmap+read
is compile error.

=== K2c: x86_64 PTE + MM trampolines ===
Files: womb/kernel/arch/x86_64/{pt,mm_trampoline}.vuma.
layout PageTableEntry (8 bytes, Intel SDM format). extern #[borrow] pte_read/
pte_write + invlpg/tlb_flush/cr3_read/cr3_write. 4-level paging.
DoD: PTE 8 bytes; 6 trampolines resolve on x86_64; vmm_map verifies.

=== K2d: aarch64 PTE + MM trampolines ===
Files: womb/kernel/arch/aarch64/{pt,mm_trampoline}.vuma.
layout PageTableEntry (8 bytes, ARMv8 format). extern pte_read/pte_write +
tlbi_vmalle1/ttbr0_el1_read/write/dsb/isb. 4-level paging, 4KB granule.
DoD: PTE 8 bytes ARMv8; trampolines resolve on aarch64; vmm_map verifies.

=== K2e: riscv64 PTE + MM trampolines ===
Files: womb/kernel/arch/riscv64/{pt,mm_trampoline}.vuma.
layout PageTableEntry (8 bytes, Sv39 format). extern pte_read/pte_write +
sfence_vma/satp_read/write. Sv39 3-level paging.
DoD: PTE 8 bytes Sv39; trampolines resolve on riscv64; vmm_map verifies.

=== K2f: kmalloc + mmap syscall ===
Files: womb/kernel/mm/{kmalloc,mmap}.vuma.
kmalloc.vuma: kmalloc(size)->u64 (slab over PageArena), kfree(addr).
mmap.vuma: layout RegionTable{arena:State<Arena>,count:u32,regions:[Region;64]}.
transform sys_mmap(regions,addr,len,prot,flags)->State<RegionTable>.
DoD: kmalloc(4096) works; kmalloc(1<<20) triggers arena_grow; sys_mmap verifies;
double-map is compile error; all 3 archs compile.
```

---

## Wave K3 — Traps, IRQs, Syscall Dispatch

**Goal:** Per-arch trap entry/exit + VUMA trap dispatcher + syscall table with capability checks. `write(1,"hi",3)` from user mode reaches the VFS layer.
**Depends on:** K1, K2. **Max parallel:** 6.

| Task | Files |
|---|---|
| K3a | `womb/kernel/arch/x86_64/trap.S`, `womb/kernel/arch/x86_64/trap_trampoline.vuma` |
| K3b | `womb/kernel/arch/aarch64/trap.S`, `womb/kernel/arch/aarch64/trap_trampoline.vuma` |
| K3c | `womb/kernel/arch/riscv64/trap.S`, `womb/kernel/arch/riscv64/trap_trampoline.vuma` |
| K3d | `womb/kernel/trap/trap.vuma`, `womb/kernel/trap/irq.vuma` |
| K3e | `womb/kernel/syscall/{table,dispatch,abi}.vuma` |
| K3f | `womb/kernel/syscall/handlers/{io,proc,mm}.vuma` |

### K3a — x86_64 trap entry/exit
**Contract:** `trap.S`: IDT-aligned vector table (32 CPU exceptions + 224 IRQs), `trap_entry` saves all GPRs + error code + vec num to a `TrapFrame` on the stack, calls `trap_handler(frame_ptr)`, `trap_exit` restores + `iretq`. `trap_trampoline.vuma`: `layout TrapFrame = { rax:u64, rbx:u64, ..., r15:u64, rip:u64, cs:u64, rflags:u64, rsp:u64, ss:u64, vec:u64, err:u64 }` (exact order matching trap.S). `extern "C"`: `idt_load(ptr)`, `irq_mask(irq)`, `irq_unmask(irq)`, `pic_eoi(irq)`.
**DoD:**
- [ ] `trap.S` assembles; IDT has 256 entries
- [ ] `TrapFrame` layout matches trap.S save order (offset-verify by hand)
- [ ] Trampolines resolve on x86_64

### K3b — aarch64 trap entry/exit
**Contract:** `trap.S`: VBAR_EL1 vector table (4 exception types × 4 sync/irq/fiq/serror = 16 entries), `trap_entry` saves x0–x30 + sp_el0 + elr_el1 + spsr_el1 + esr_el1, calls `trap_handler`, `trap_exit` restores + `eret`. `trap_trampoline.vuma`: `layout TrapFrame = { x0:u64, ..., x30:u64, sp_el0:u64, elr_el1:u64, spsr_el1:u64, esr_el1:u64 }`. `extern "C"`: `vbar_load(ptr)`, `gic_eoi(irq)`, `gic_mask(irq)`, `gic_unmask(irq)`.
**DoD:**
- [ ] `trap.S` assembles; VBAR has 16 entries
- [ ] `TrapFrame` matches save order
- [ ] Trampolines resolve on aarch64

### K3c — riscv64 trap entry/exit
**Contract:** `trap.S`: `stvec` points to vector table, `trap_entry` saves x1–x31 + mepc + mstatus + scause + stval, calls `trap_handler`, `trap_exit` restores + `sret`. `trap_trampoline.vuma`: `layout TrapFrame = { ra:u64, sp:u64, ..., t6:u64, mepc:u64, mstatus:u64, scause:u64, stval:u64 }`. `extern "C"`: `stvec_load(ptr)`, `plic_eoi(irq)`, `plic_mask(irq)`, `plic_unmask(irq)`.
**DoD:**
- [ ] `trap.S` assembles; stvec-aligned
- [ ] `TrapFrame` matches save order
- [ ] Trampolines resolve on riscv64

### K3d — Trap dispatcher + IRQ subsystem
**Contract:** `trap.vuma`: `transform trap_handler(frame: State<TrapFrame>) -> State<TrapFrame>` — reads `frame.vec`, dispatches: CPU exception → `panic`, syscall → `syscall_dispatch`, IRQ → `irq_dispatch`. `irq.vuma`: `layout IrqTable = { handlers: [u64; 256], count: u32 }`, `fn register_irq(tbl: State<IrqTable>, irq: u8, handler: u64)`, `fn irq_dispatch(tbl: State<IrqTable>, irq: u8)`, `fn enable_irq(irq: u8)`, `fn disable_irq(irq: u8)`.
**DoD:**
- [ ] `trap_handler` verifies on all 3 archs
- [ ] `register_irq` + `irq_dispatch` verify
- [ ] No pointer syntax

### K3e — Syscall table + dispatch + ABI
**Contract:** `abi.vuma`: `layout SyscallArgs = { nr:u64, a0:u64, a1:u64, a2:u64, a3:u64, a4:u64, a5:u64 }`. `table.vuma`: `layout SyscallTable = { handlers: [u64; 512] }` (indexed by asm-generic nr). `dispatch.vuma`: `transform syscall_dispatch(args: State<SyscallArgs>) -> State<SyscallArgs>` — checks `args.nr < 512`, looks up handler, checks capability, calls handler, writes return to `args.a0`. Reuse VUMA-generic syscall numbers from `womb/syscalls.vuma`.
**DoD:**
- [ ] `SyscallTable` has 512 entries
- [ ] `syscall_dispatch` verifies; out-of-range nr → panic
- [ ] `write(1, buf, 3)` (nr 64) dispatches to the io handler

### K3f — Syscall handlers (io/proc/mm)
**Contract:** `io.vuma`: `fn sys_write(fd: u64, buf: Address, count: u64) -> i64` — if fd==1 or fd==2, call `console_write`; else call `vfs_write`. `fn sys_read(fd: u64, buf: Address, count: u64) -> i64`. `proc.vuma`: `fn sys_getpid() -> i64`, `fn sys_exit(code: i64) -> !`. `mm.vuma`: `fn sys_brk(addr: u64) -> u64`, `fn sys_mmap(addr, len, prot, flags, fd, off) -> u64` (wraps `sys_mmap` from K2f).
**DoD:**
- [ ] All handlers verify on all 3 archs
- [ ] `sys_write(1, "hi\n", 3)` returns 3
- [ ] `sys_exit(0)` does not return

### Dispatch Box — Wave K3

```
=== COMMON PREAMBLE (Wave K3) ===
You are Wave K3 of VWK. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md (K1+K2 complete). APPEND when done.
REPO: /home/z/vuma. BUILD: cargo build --profile release-fast --bin compile_dump.
TEST: ./target/release-fast/compile_dump <in.vuma> /tmp/o.bin <backend> --verify
RULES: No stubs. PMT-only. CARGO_BUILD_JOBS=1. Commit per task, do NOT push.
Read womb/syscalls.vuma for the asm-generic syscall number table. Read sibling
tasks' TrapFrame layout before referencing it.

=== K3a: x86_64 trap entry/exit ===
Files: womb/kernel/arch/x86_64/{trap.S,trap_trampoline}.vuma.
trap.S: 256-entry IDT, trap_entry saves GPRs+err+vec to TrapFrame, calls
trap_handler, trap_exit iretq. trap_trampoline.vuma: layout TrapFrame (match
save order), extern idt_load/irq_mask/irq_unmask/pic_eoi.
DoD: trap.S assembles; 256 IDT entries; TrapFrame matches; trampolines resolve.

=== K3b: aarch64 trap entry/exit ===
Files: womb/kernel/arch/aarch64/{trap.S,trap_trampoline}.vuma.
trap.S: VBAR_EL1 16-entry table, trap_entry saves x0-x30+sp_el0+elr+spsr+esr,
calls trap_handler, trap_exit eret. trap_trampoline.vuma: layout TrapFrame,
extern vbar_load/gic_eoi/gic_mask/gic_unmask.
DoD: trap.S assembles; 16 VBAR entries; TrapFrame matches; trampolines resolve.

=== K3c: riscv64 trap entry/exit ===
Files: womb/kernel/arch/riscv64/{trap.S,trap_trampoline}.vuma.
trap.S: stvec vector table, trap_entry saves x1-x31+mepc+mstatus+scause+stval,
calls trap_handler, trap_exit sret. trap_trampoline.vuma: layout TrapFrame,
extern stvec_load/plic_eoi/plic_mask/plic_unmask.
DoD: trap.S assembles; stvec-aligned; TrapFrame matches; trampolines resolve.

=== K3d: Trap dispatcher + IRQ subsystem ===
Files: womb/kernel/trap/{trap,irq}.vuma.
trap.vuma: transform trap_handler(frame:State<TrapFrame>)->State<TrapFrame>;
dispatch by frame.vec: CPU exc→panic, syscall→syscall_dispatch, IRQ→irq_dispatch.
irq.vuma: layout IrqTable{handlers:[u64;256],count:u32}, register_irq/
irq_dispatch/enable_irq/disable_irq.
DoD: trap_handler verifies on 3 archs; register_irq+irq_dispatch verify;
no pointer syntax.

=== K3e: Syscall table + dispatch + ABI ===
Files: womb/kernel/syscall/{table,dispatch,abi}.vuma.
abi.vuma: layout SyscallArgs{nr,a0..a5:u64}. table.vuma: layout
SyscallTable{handlers:[u64;512]}. dispatch.vuma: transform
syscall_dispatch(args)->State<SyscallArgs>; check nr<512; lookup; cap check;
call handler; write ret to a0.
DoD: 512-entry table; dispatch verifies; out-of-range nr→panic; write(1,buf,3)
dispatches to io handler.

=== K3f: Syscall handlers (io/proc/mm) ===
Files: womb/kernel/syscall/handlers/{io,proc,mm}.vuma.
io.vuma: sys_write(fd,buf,count)->i64 (fd 1/2→console_write, else vfs_write),
sys_read. proc.vuma: sys_getpid->i64, sys_exit(code)->!. mm.vuma: sys_brk,
sys_mmap (wraps K2f sys_mmap).
DoD: all handlers verify on 3 archs; sys_write(1,"hi\n",3) returns 3;
sys_exit(0) does not return.
```

---

## Wave K4 — Process / Scheduler / Context Switch

**Goal:** `Task` lifecycle + preemptive scheduler + per-arch context switch. Two user tasks ping-pong via a shared futex.
**Depends on:** K2, K3. **Max parallel:** 6.

| Task | Files |
|---|---|
| K4a | `womb/kernel/proc/task.vuma` |
| K4b | `womb/kernel/proc/scheduler.vuma` |
| K4c | `womb/kernel/arch/x86_64/switch.S`, `womb/kernel/arch/aarch64/switch.S`, `womb/kernel/arch/riscv64/switch.S` |
| K4d | `womb/kernel/proc/{fork,exec,exit}.vuma` |
| K4e | `womb/kernel/proc/wait.vuma` |
| K4f | `womb/kernel/smp/percpu.vuma` |

### K4a — Task layout + lifecycle
**Contract:** `layout TaskState = { RUNNING: u8, READY: u8, BLOCKED: u8, ZOMBIE: u8 }` (enum-like). `layout Task = { pid:u32, ppid:u32, state:u8, prio:u8, vruntime:u64, regs:TrapFrame, mm_root:u64, fs_root:u64, fds:u64, next:u64 }`. `layout ProcessTable = { arena:State<Arena>, count:u32, tasks:[u64;256] }` (256-slot table, each slot holds a Task offset). `fn task_alloc(tbl:State<ProcessTable>) -> State<Task>`, `fn task_free(tbl:State<ProcessTable>, pid:u32)`.
**DoD:**
- [ ] `Task` layout includes all 10 fields; `TrapFrame` is the arch-specific one from K3
- [ ] `task_alloc` + `task_free` verify; double-free is a compile error
- [ ] No pointer syntax

### K4b — Scheduler
**Contract:** `layout Runqueue = { head:u64, tail:u64, count:u32 }` (arena offsets into Task list). `fn sched_init() -> State<Runqueue>`, `fn sched_enqueue(rq:State<Runqueue>, task:u64)`, `fn sched_dequeue(rq:State<Runqueue>) -> u64`, `fn schedule()` — picks next task by min vruntime, calls `context_switch`. `fn sched_tick()` — updates vruntime, preempts if `curr.vruntime - next.vruntime > threshold`. `fn sched_yield()`, `fn wake_up(pid:u32)`, `fn sleep_on(waitq:u64)`.
**DoD:**
- [ ] CFS-like vruntime scheduling verifies
- [ ] `sched_tick` + `schedule` verify on all 3 archs
- [ ] Round-robin visible when all vruntimes equal

### K4c — Context switch (3 archs)
**Contract:** `switch.S` per arch: `context_switch(prev: u64, next: u64)` — save callee-saved regs + sp + pc to `prev` Task's regs field, load from `next` Task's regs field, `ret`/`eret`/`sret`. x86_64: save rbx,rbp,r12-r15,rsp. aarch64: save x19-x29,sp. riscv64: save s0-s11,sp. The Task.regs field offset must match `switch.S`.
**DoD:**
- [ ] All 3 `switch.S` assemble cleanly
- [ ] Save/restore register set matches each arch's callee-saved convention
- [ ] Task.regs offset in `task.vuma` matches `switch.S`'s struct offset

### K4d — fork/exec/exit
**Contract:** `fork.vuma`: `transform sys_fork(parent:State<Task>) -> State<Task>` — alloc child Task, copy parent's mm/fs/fds, set child.ppid=parent.pid, return child. `exec.vuma`: `fn sys_exec(path:Address, argv:Address) -> !` — load ELF (reuse `womb/lang/elf.vuma`), replace mm, jump to entry. `exit.vuma`: `transform sys_exit(task:State<Task>, code:i32) -> State<ZombieTask>` — mark zombie, wake parent, schedule next. `layout ZombieTask = { pid:u32, ppid:u32, exit_code:i32 }`.
**DoD:**
- [ ] `sys_fork` verifies; child Task is a fresh state (no aliasing)
- [ ] `sys_exit` is a `transform` — write-after-exit is a compile error
- [ ] `ZombieTask` is a different layout than `Task` (StateTransform copy)

### K4e — waitpid
**Contract:** `wait.vuma`: `fn sys_waitpid(pid:i64, status:Address, options:u64) -> i64` — if child is zombie, reap (free Task), write status, return child pid. If child still running, `sleep_on` the child's wait queue. If no children, return -ECHILD. `layout WaitQueue = { head:u64, tail:u64, count:u32 }`.
**DoD:**
- [ ] `sys_waitpid` verifies; reaping a zombie frees the Task slot
- [ ] Blocking on a running child calls `sleep_on`
- [ ] `-ECHILD` returned when no children

### K4f — Per-CPU state
**Contract:** `layout PerCpu = { cpu_id:u32, current_task:u64, kernel_stack:u64, idle_task:u64, rq:u64 }`. `fn percpu_init(cpu_id:u32) -> State<PerCpu>`, `fn percpu_get() -> State<PerCpu>` (reads from arch-specific register: GS base on x86_64, TPIDR_EL0 on aarch64, tp on riscv64), `fn percpu_set_current(task:u64)`. `extern "C"`: `rdmsr/fs`/`wrmsr` (x86_64), `mrs_tpidr_el0`/`msr_tpidr_el0` (aarch64), `rd_tp`/`wr_tp` (riscv64).
**DoD:**
- [ ] `PerCpu` layout with 5 fields
- [ ] `percpu_get` + `percpu_set_current` verify on all 3 archs
- [ ] Trampolines resolve on all 3 archs

### Dispatch Box — Wave K4

```
=== COMMON PREAMBLE (Wave K4) ===
You are Wave K4 of VWK. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md (K2+K3 complete). APPEND when done.
REPO: /home/z/vuma. BUILD: cargo build --profile release-fast --bin compile_dump.
TEST: ./target/release-fast/compile_dump <in.vuma> /tmp/o.bin <backend> --verify
RULES: No stubs. PMT-only. CARGO_BUILD_JOBS=1. Commit per task, do NOT push.
Read womb/kernel/trap/trap_trampoline.vuma (K3) for TrapFrame layout.
Read womb/kernel/mm/ (K2) for PageArena/Arena usage.

=== K4a: Task layout + lifecycle ===
Files: womb/kernel/proc/task.vuma.
layout Task{pid,ppid,state:u8,prio:u8,vruntime:u64,regs:TrapFrame,mm_root,
fs_root,fds,next:u64}. layout ProcessTable{arena:State<Arena>,count:u32,
tasks:[u64;256]}. task_alloc/task_free.
DoD: 10 fields; TrapFrame from K3; task_alloc+free verify; double-free is
compile error; no pointer syntax.

=== K4b: Scheduler ===
Files: womb/kernel/proc/scheduler.vuma.
layout Runqueue{head,tail,count}. sched_init/enqueue/dequeue/schedule/sched_tick
(CFS vruntime)/sched_yield/wake_up/sleep_on. schedule calls context_switch.
DoD: CFS verifies; sched_tick+schedule verify on 3 archs; round-robin on equal
vruntime.

=== K4c: Context switch (3 archs) ===
Files: womb/kernel/arch/{x86_64,aarch64,riscv64}/switch.S.
context_switch(prev,next): save callee-saved+sp to prev Task.regs, load from
next Task.regs, ret/eret/sret. x86_64: rbx,rbp,r12-r15,sp. aarch64: x19-x29,sp.
riscv64: s0-s11,sp. Task.regs offset MUST match switch.S.
DoD: all 3 assemble; reg sets match ABI; offset matches task.vuma.

=== K4d: fork/exec/exit ===
Files: womb/kernel/proc/{fork,exec,exit}.vuma.
fork.vuma: transform sys_fork(parent)->State<Task> (alloc child, copy mm/fs/fds).
exec.vuma: sys_exec(path,argv)->! (load ELF via womb/lang/elf.vuma, replace mm).
exit.vuma: transform sys_exit(task,code)->State<ZombieTask>. layout
ZombieTask{pid,ppid,exit_code}.
DoD: fork verifies (no aliasing); exit is transform (write-after-exit compile
error); ZombieTask != Task layout (StateTransform copy).

=== K4e: waitpid ===
Files: womb/kernel/proc/wait.vuma.
sys_waitpid(pid,status,options)->i64. Zombie child→reap+write status+return
child pid. Running child→sleep_on. No children→-ECHILD. layout WaitQueue.
DoD: waitpid verifies; reaping frees Task slot; blocking sleeps; -ECHILD on
no children.

=== K4f: Per-CPU state ===
Files: womb/kernel/smp/percpu.vuma.
layout PerCpu{cpu_id,current_task,kernel_stack,idle_task,rq}. percpu_init/
percpu_get/percpu_set_current. extern: rdmsr/wrmsr (x86_64), mrs_tpidr_el0/
msr_tpidr_el0 (aarch64), rd_tp/wr_tp (riscv64).
DoD: 5 fields; percpu_get+set_current verify on 3 archs; trampolines resolve.
```

---

## Wave K5 — VFS + tmpfs + initramfs

**Goal:** VFS layer + tmpfs + initramfs unpacker. `cat /etc/motd` from a user shell prints the initramfs motd.
**Depends on:** K2, K4. **Max parallel:** 6.

| Task | Files |
|---|---|
| K5a | `womb/kernel/vfs/{inode,dentry,file,mount}.vuma` |
| K5b | `womb/kernel/vfs/namei.vuma` |
| K5c | `womb/kernel/fs/tmpfs.vuma` |
| K5d | `womb/kernel/fs/initramfs.vuma` |
| K5e | `womb/kernel/vfs/mount.vuma` |
| K5f | `womb/kernel/vfs/file_ops.vuma` |

### K5a — VFS core structures
**Contract:** `layout Inode = { ino:u64, mode:u32, uid:u32, gid:u32, size:u64, blocks:u64, atime:u64, mtime:u64, ops:u64, private:u64 }`. `layout Dentry = { name:[u8;64], parent:u64, inode:u64, next:u64, child:u64, mounted:u8 }`. `layout File = { dentry:u64, pos:u64, flags:u32, ops:u64, private:u64 }`. `layout SuperBlock = { dev:u64, type:u64, root:u64, ops:u64 }`. All stored in a `VfsArena` (typed as `State<Arena>`).
**DoD:**
- [ ] All 4 layouts parse + verify
- [ ] No pointer syntax; all references are arena offsets (u64)
- [ ] `Inode.ops` and `File.ops` are function pointers (u64)

### K5b — Path resolution (namei)
**Contract:** `transform namei(root:State<Dentry>, path:Address) -> State<Dentry>` — walk path components (`/etc/motd` → root→etc→motd), follow mount points, return the leaf Dentry. `fn path_split(path:Address) -> [Address;16]` (splits by `/`). Handle `.` and `..` (parent). 16-component max depth.
**DoD:**
- [ ] `namei` verifies; consumes input Dentry (linearity)
- [ ] `..` traversal works
- [ ] Missing component → `-ENOENT` (not a crash)

### K5c — tmpfs
**Contract:** `fn tmpfs_mount(dev:u64, opts:Address) -> State<SuperBlock>` — alloc SuperBlock, root Inode (dir mode 0755). `fn tmpfs_create(parent:State<Inode>, name:Address, mode:u32) -> State<Inode>`. `fn tmpfs_mkdir(parent:State<Inode>, name:Address, mode:u32) -> State<Inode>`. `fn tmpfs_lookup(dir:State<Inode>, name:Address) -> u64` (returns inode offset, 0 if not found). `fn tmpfs_read(file:State<File>, buf:Address, count:u64) -> i64`. `fn tmpfs_write(file:State<File>, buf:Address, count:u64) -> i64`. Data stored in `VfsArena` pages.
**DoD:**
- [ ] All 5 functions verify
- [ ] `tmpfs_create` then `tmpfs_lookup` round-trips
- [ ] `tmpfs_read` after `tmpfs_write` returns the written bytes

### K5d — initramfs unpacker
**Contract:** `fn initramfs_unpack(base:Address, size:u64, root_sb:State<SuperBlock>) -> u64` — parses newc-format cpio archive, creates inodes + dentries under root, returns count of files extracted. Handle regular files, dirs, symlinks. Use `tmpfs_create`/`mkdir` from K5c.
**DoD:**
- [ ] `initramfs_unpack` verifies
- [ ] Extracts `/etc/motd` from a test cpio archive
- [ ] Returns file count

### K5e — Mount layer
**Contract:** `layout MountTable = { arena:State<Arena>, count:u32, mounts:[Mount;16] }`. `layout Mount = { dentry:u64, sb:u64, parent:u64, next:u64 }`. `fn vfs_mount(dentry:u64, fs_type:u64, dev:u64, opts:Address) -> u64` — calls the FS's mount function, adds to MountTable. `fn vfs_umount(dentry:u64)`. `fn vfs_mounted_at(dentry:u64) -> u64` (returns Mount or 0).
**DoD:**
- [ ] `MountTable` with 16-entry array
- [ ] `vfs_mount` + `vfs_umount` verify
- [ ] `vfs_mounted_at` detects mount points

### K5f — File operations dispatch
**Contract:** `fn vfs_open(root:State<Dentry>, path:Address, flags:u32) -> State<File>` — call `namei`, alloc File, set ops from Inode. `fn vfs_read(file:State<File>, buf:Address, count:u64) -> i64` — dispatch to `file.ops.read`. `fn vfs_write(file:State<File>, buf:Address, count:u64) -> i64`. `fn vfs_close(file:State<File>)`. `fn vfs_stat(root:State<Dentry>, path:Address, statbuf:Address) -> i64`. `layout Stat = { ino:u64, mode:u32, size:u64, blocks:u64 }`.
**DoD:**
- [ ] All 5 functions verify
- [ ] `vfs_open("/etc/motd", O_RDONLY)` then `vfs_read` returns motd bytes
- [ ] Dispatch is via `file.ops` function pointer

### Dispatch Box — Wave K5

```
=== COMMON PREAMBLE (Wave K5) ===
You are Wave K5 of VWK. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md (K2+K4 complete). APPEND when done.
REPO: /home/z/vuma. BUILD: cargo build --profile release-fast --bin compile_dump.
TEST: ./target/release-fast/compile_dump <in.vuma> /tmp/o.bin <backend> --verify
RULES: No stubs. PMT-only. CARGO_BUILD_JOBS=1. Commit per task, do NOT push.
All VFS structures use arena offsets (u64) for cross-references — never pointers.

=== K5a: VFS core structures ===
Files: womb/kernel/vfs/{inode,dentry,file,mount}.vuma.
layout Inode{ino,mode,uid,gid,size,blocks,atime,mtime,ops,private:u64/u32}.
layout Dentry{name:[u8;64],parent,inode,next,child:u64,mounted:u8}.
layout File{dentry,pos:u64,flags:u32,ops,private:u64}. layout SuperBlock.
All in VfsArena.
DoD: 4 layouts verify; offsets are u64 (no pointers); ops are fn ptrs.

=== K5b: Path resolution ===
Files: womb/kernel/vfs/namei.vuma.
transform namei(root:State<Dentry>,path:Address)->State<Dentry>. path_split
(by /). Handle . and ... 16-component max. Missing→-ENOENT.
DoD: namei verifies; consumes input (linearity); .. works; missing is -ENOENT.

=== K5c: tmpfs ===
Files: womb/kernel/fs/tmpfs.vuma.
tmpfs_mount(dev,opts)->State<SuperBlock>. tmpfs_create(parent,name,mode)->
State<Inode>. tmpfs_mkdir. tmpfs_lookup(dir,name)->u64. tmpfs_read/write(file,
buf,count)->i64. Data in VfsArena pages.
DoD: 5 functions verify; create+lookup round-trips; read after write returns
written bytes.

=== K5d: initramfs unpacker ===
Files: womb/kernel/fs/initramfs.vuma.
initramfs_unpack(base,size,root_sb)->u64. Parse newc cpio. Create inodes+
dentries under root. Handle reg files, dirs, symlinks. Use tmpfs_create/mkdir.
DoD: verifies; extracts /etc/motd from test cpio; returns file count.

=== K5e: Mount layer ===
Files: womb/kernel/vfs/mount.vuma.
layout MountTable{arena,count,mounts:[Mount;16]}. layout Mount{dentry,sb,
parent,next}. vfs_mount(dentry,fs_type,dev,opts)->u64. vfs_umount.
vfs_mounted_at(dentry)->u64.
DoD: 16-entry table; mount+umount verify; mounted_at detects mount points.

=== K5f: File operations dispatch ===
Files: womb/kernel/vfs/file_ops.vuma.
vfs_open(root,path,flags)->State<File>. vfs_read/write(file,buf,count)->i64
(dispatch via file.ops). vfs_close. vfs_stat(root,path,statbuf)->i64.
layout Stat{ino,mode,size,blocks}.
DoD: 5 functions verify; open("/etc/motd")+read returns motd; dispatch via
file.ops fn ptr.
```

---

## Wave K6 — TTY + Console + Char Devices

**Goal:** UART driver + TTY line discipline + VT100 parser + interactive shell on serial console. Ctrl-C delivers SIGINT.
**Depends on:** K3, K5. **Max parallel:** 6.

| Task | Files |
|---|---|
| K6a | `womb/kernel/drivers/uart.vuma` |
| K6b | `womb/kernel/tty/line_discipline.vuma` |
| K6c | `womb/kernel/tty/vt100.vuma` |
| K6d | `womb/kernel/drivers/char.vuma` |
| K6e | `womb/kernel/tty/console.vuma` |
| K6f | `womb/kernel/shell/shell.vuma` |

### K6a — UART driver
**Contract:** `layout UartRegs = { data:u8, ier:u8, iir:u8, lcr:u8, mcr:u8, lsr:u8, msr:u8, scr:u8 }` (8250 register set). `layout Pl011Regs = { dr:u32, fr:u32, ibrd:u32, fbrd:u32, lcr_h:u32, cr:u32 }` (PL011). `fn uart_init_8250(base:u64)`, `fn uart_init_pl011(base:u64)`, `fn uart_putc(base:u64, c:u8)`, `fn uart_getc(base:u64) -> u8`, `fn uart_puts(base:u64, s:Address)`. Use `extern "C"` MMIO trampolines (`mmio_read8`/`mmio_write8`/`mmio_read32`/`mmio_write32`).
**DoD:**
- [ ] Both 8250 + PL011 register layouts verify
- [ ] All 5 functions verify on x86_64 (8250) + aarch64 (PL011)
- [ ] MMIO trampolines resolve on both backends

### K6b — TTY line discipline
**Contract:** `layout TtyLine = { buf:[u8;256], head:u32, tail:u32, count:u32, raw:u8, echo:u8 }`. `fn tty_init() -> State<TtyLine>`, `fn tty_receive(tty:State<TtyLine>, c:u8) -> State<TtyLine>` — if raw mode, push to buf; else handle `\n` (line-complete), Ctrl-C (SIGINT to foreground), Ctrl-D (EOF), backspace. `fn tty_readline(tty:State<TtyLine>, buf:Address, max:u64) -> i64` — returns when a full line is in buf.
**DoD:**
- [ ] `TtyLine` with 256-byte ring buffer verifies
- [ ] Ctrl-C triggers SIGINT (calls `signal_send` from K7)
- [ ] Line-mode returns on `\n`; raw-mode returns immediately

### K6c — VT100 parser
**Contract:** `layout Vt100State = { row:u32, col:u32, fg:u8, bg:u8, esc:u8, esc_buf:[u8;16] }`. `fn vt100_init() -> State<Vt100State>`, `fn vt100_feed(s:State<Vt100State>, c:u8) -> State<Vt100State>` — handle printable chars (advance col), `\n` (newline), `\r` (col=0), ESC `[` sequences (cursor movement, color). `fn vt100_render(s:State<Vt100State>, fb:Address)` — write to framebuffer.
**DoD:**
- [ ] `Vt100State` verifies
- [ ] `vt100_feed` handles CSI sequences (`ESC [ ... m`, `ESC [ ... H`)
- [ ] Cursor position tracked correctly

### K6d — Char device registry
**Contract:** `layout CharDevice = { name:[u8;16], major:u32, minor:u32, open:u64, read:u64, write:u64, ioctl:u64, close:u64 }`. `layout CharDevTable = { arena:State<Arena>, count:u32, devs:[u64;32] }`. `fn chardev_register(tbl:State<CharDevTable>, dev:State<CharDevice>) -> u32` (returns major). `fn chardev_lookup(tbl:State<CharDevTable>, major:u32) -> u64`. `fn chardev_open(tbl:State<CharDevTable>, major:u32, minor:u32) -> State<File>`.
**DoD:**
- [ ] `CharDevice` with 6 function-pointer ops verifies
- [ ] `chardev_register` + `chardev_lookup` round-trip
- [ ] `chardev_open` returns a `File` with ops set

### K6e — Console subsystem
**Contract:** `layout Console = { uart_base:u64, tty:State<TtyLine>, vt:State<Vt100State>, rows:u32, cols:u32 }`. `fn console_init(base:u64) -> State<Console>`, `fn console_putc(c:u8)`, `fn console_puts(s:Address)`, `fn console_read(buf:Address, max:u64) -> i64` — calls `tty_readline`. Wire to K1's `console.vuma` (replace the stub with this real impl).
**DoD:**
- [ ] `Console` layout aggregates TTY + VT100 + UART
- [ ] `console_putc`/`puts`/`read` verify
- [ ] Replaces K1's stub `console.vuma` with no regression

### K6f — Shell
**Contract:** `fn shell_init()`, `fn shell_loop()` — read line via `console_read`, parse command + args, if `cat <path>` call `vfs_open`+`vfs_read`+`console_puts`; if `ls <path>` call `namei`+list dentries; if `exit` call `sys_exit`. Built-in commands: `cat`, `ls`, `pwd`, `cd`, `echo`, `exit`. 4-path max args.
**DoD:**
- [ ] `shell_loop` verifies
- [ ] `cat /etc/motd` prints the initramfs motd
- [ ] `ls /` lists root directory entries

### Dispatch Box — Wave K6

```
=== COMMON PREAMBLE (Wave K6) ===
You are Wave K6 of VWK. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md (K3+K5 complete). APPEND when done.
REPO: /home/z/vuma. BUILD: cargo build --profile release-fast --bin compile_dump.
TEST: ./target/release-fast/compile_dump <in.vuma> /tmp/o.bin <backend> --verify
RULES: No stubs. PMT-only. CARGO_BUILD_JOBS=1. Commit per task, do NOT push.
Read womb/kernel/ipc/signal.vuma (K7, if available) for SIGINT delivery. If K7
not done, stub the SIGINT call with a TODO and a worklog note.

=== K6a: UART driver ===
Files: womb/kernel/drivers/uart.vuma.
layout UartRegs (8250) + Pl011Regs. uart_init_8250/pl011, uart_putc/getc/puts.
Use extern mmio_read8/write8/read32/write32 trampolines.
DoD: both layouts verify; 5 functions verify on x86_64+aarch64; MMIO trampolines
resolve.

=== K6b: TTY line discipline ===
Files: womb/kernel/tty/line_discipline.vuma.
layout TtyLine{buf:[u8;256],head,tail,count,raw:u8,echo:u8}. tty_init,
tty_receive (Ctrl-C→SIGINT, Ctrl-D→EOF, \n→line complete, backspace),
tty_readline.
DoD: 256-byte ring verifies; Ctrl-C→SIGINT; line-mode returns on \n; raw-mode
returns immediately.

=== K6c: VT100 parser ===
Files: womb/kernel/tty/vt100.vuma.
layout Vt100State{row,col,fg,bg,esc:u8,esc_buf:[u8;16]}. vt100_init/feed
(printable→advance, \n→newline, \r→col0, ESC[ sequences)/render.
DoD: verifies; handles CSI sequences; cursor tracked.

=== K6d: Char device registry ===
Files: womb/kernel/drivers/char.vuma.
layout CharDevice{name,major,minor,open,read,write,ioctl,close}. layout
CharDevTable{arena,count,devs:[u64;32]}. chardev_register/lookup/open.
DoD: 6-op CharDevice verifies; register+lookup round-trips; open returns File.

=== K6e: Console subsystem ===
Files: womb/kernel/tty/console.vuma.
layout Console{uart_base,tty,vt,rows,cols}. console_init/putc/puts/read.
Replaces K1 stub console.vuma.
DoD: aggregates TTY+VT100+UART; putc/puts/read verify; no regression vs K1.

=== K6f: Shell ===
Files: womb/kernel/shell/shell.vuma.
shell_init/shell_loop. Built-ins: cat,ls,pwd,cd,echo,exit. 4-arg max. cat calls
vfs_open+read+console_puts. ls calls namei+list dentries.
DoD: shell_loop verifies; cat /etc/motd prints motd; ls / lists root.
```

---

## Wave K7 — IPC: Pipe, Signal, Futex

**Goal:** Pipe + signal + futex + shared memory. `echo hi | cat` works; `kill -9 <pid>` terminates a task.
**Depends on:** K4, K5. **Max parallel:** 5.

| Task | Files |
|---|---|
| K7a | `womb/kernel/ipc/pipe.vuma` |
| K7b | `womb/kernel/ipc/signal.vuma` |
| K7c | `womb/kernel/ipc/futex.vuma` |
| K7d | `womb/kernel/ipc/shm.vuma` |
| K7e | `womb/kernel/ipc/waitq.vuma` |

### K7a — Pipe
**Contract:** `layout Pipe = { buf:[u8;4096], head:u32, tail:u32, count:u32, read_waitq:u64, write_waitq:u64, closed:u8 }`. `fn pipe_create() -> (u64, u64)` (returns read_fd, write_fd). `fn pipe_read(fd:u64, buf:Address, count:u64) -> i64` — if empty, sleep on `read_waitq`; else copy min(count,pipe.count) bytes. `fn pipe_write(fd:u64, buf:Address, count:u64) -> i64` — if full, sleep on `write_waitq`; else copy. `fn pipe_close(fd:u64)` — wake all waiters, set closed.
**DoD:**
- [ ] `Pipe` with 4096-byte ring verifies
- [ ] `pipe_read` on empty pipe sleeps (calls `sleep_on`)
- [ ] `pipe_close` wakes all waiters

### K7b — Signal
**Contract:** `layout SignalTable = { handlers:[u64;32], mask:u32, pending:u32 }` (32 signals, asm-generic). `fn signal_init() -> State<SignalTable>`, `fn signal_install(tbl:State<SignalTable>, sig:u8, handler:u64) -> State<SignalTable>`, `fn signal_send(pid:u32, sig:u8) -> i64` — look up Task, set `pending |= (1<<sig)`, wake if BLOCKED. `fn signal_check(tbl:State<SignalTable>)` — called on syscall return / interrupt return; if `pending & ~mask`, deliver.
**DoD:**
- [ ] `SignalTable` with 32-entry handler array verifies
- [ ] `signal_send` sets pending + wakes the task
- [ ] `signal_check` delivers pending unmasked signals

### K7c — Futex
**Contract:** `layout FutexTable = { arena:State<Arena>, count:u32, entries:[u64;64] }`. `layout FutexEntry = { uaddr:u64, val:u32, waitq:u64 }`. `fn sys_futex(uaddr:u64, op:u32, val:u32, timeout:u64) -> i64` — op `FUTEX_WAIT`: if `*uaddr == val`, sleep on waitq; op `FUTEX_WAKE`: wake `val` waiters. Use `AtomicLoad`/`AtomicCas` (already in IR) for the futex word.
**DoD:**
- [ ] `FutexTable` with 64 entries verifies
- [ ] `FUTEX_WAIT` sleeps if value matches; returns immediately otherwise
- [ ] `FUTEX_WAKE` wakes N waiters

### K7d — Shared memory
**Contract:** `layout ShmRegion = { key:u32, addr:u64, size:u64, creator:u32, attached:[u32;16] }`. `layout ShmTable = { arena:State<Arena>, count:u32, regions:[u64;64] }`. `fn sys_shmget(key:u32, size:u64, flags:u32) -> i64` — alloc via VMM, add to table. `fn sys_shmat(shmid:u32, addr:u64, flags:u32) -> u64` — map into caller's mm. `fn sys_shmdt(addr:u64) -> i64`.
**DoD:**
- [ ] `ShmRegion` + `ShmTable` verify
- [ ] `shmget` + `shmat` round-trip; two tasks see the same data
- [ ] `shmdt` unmaps

### K7e — Wait queue infrastructure
**Contract:** `layout WaitQueue = { head:u64, tail:u64, count:u32 }` (linked list of Task offsets). `fn waitq_init() -> State<WaitQueue>`, `fn waitq_add(wq:u64, task:u64)`, `fn waitq_remove(wq:u64, task:u64)`, `fn waitq_wake_one(wq:u64)`, `fn waitq_wake_all(wq:u64)`. Used by pipe, futex, signal, waitpid.
**DoD:**
- [ ] `WaitQueue` linked-list verifies
- [ ] `wake_one` wakes the head; `wake_all` wakes everyone
- [ ] Used consistently by K7a/K7b/K7c

### Dispatch Box — Wave K7

```
=== COMMON PREAMBLE (Wave K7) ===
You are Wave K7 of VWK. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md (K4+K5 complete). APPEND when done.
REPO: /home/z/vuma. BUILD: cargo build --profile release-fast --bin compile_dump.
TEST: ./target/release-fast/compile_dump <in.vuma> /tmp/o.bin <backend> --verify
RULES: No stubs. PMT-only. CARGO_BUILD_JOBS=1. Commit per task, do NOT push.
K7e (waitq) is the foundation — others depend on it. If dispatched in parallel,
code against the waitq contract above and integrate after.

=== K7a: Pipe ===
Files: womb/kernel/ipc/pipe.vuma.
layout Pipe{buf:[u8;4096],head,tail,count,read_waitq,write_waitq,closed:u8}.
pipe_create->(read_fd,write_fd). pipe_read (sleep if empty). pipe_write (sleep
if full). pipe_close (wake all). Use waitq (K7e).
DoD: 4096-byte ring verifies; read on empty sleeps; close wakes all.

=== K7b: Signal ===
Files: womb/kernel/ipc/signal.vuma.
layout SignalTable{handlers:[u64;32],mask,pending:u32}. signal_init/install/
send (set pending+wake)/check (deliver on syscall/IRQ return).
DoD: 32-entry array verifies; send sets pending+wakes; check delivers.

=== K7c: Futex ===
Files: womb/kernel/ipc/futex.vuma.
layout FutexTable{arena,count,entries:[u64;64]}. layout FutexEntry{uaddr,val,
waitq}. sys_futex(uaddr,op,val,timeout). FUTEX_WAIT: if *uaddr==val sleep.
FUTEX_WAKE: wake N. Use AtomicLoad/AtomicCas.
DoD: 64 entries verify; WAIT sleeps if match; WAKE wakes N.

=== K7d: Shared memory ===
Files: womb/kernel/ipc/shm.vuma.
layout ShmRegion{key,addr,size:u64,creator:u32,attached:[u32;16]}. layout
ShmTable{arena,count,regions:[u64;64]}. sys_shmget/shmat/shmdt.
DoD: verifies; shmget+shmat round-trips; two tasks share data; shmdt unmaps.

=== K7e: Wait queue infrastructure ===
Files: womb/kernel/ipc/waitq.vuma.
layout WaitQueue{head,tail,count}. waitq_init/add/remove/wake_one/wake_all.
Foundation for pipe/futex/signal/waitpid.
DoD: linked-list verifies; wake_one wakes head; wake_all wakes everyone.
```

---

## Wave K8 — Sync Primitives + SMP IPI

**Goal:** Spinlock/mutex/semaphore/rwlock + IPI delivery. 2-CPU QEMU boot with concurrent `kmalloc` stress test.
**Depends on:** K4, K7. **Max parallel:** 6.

| Task | Files |
|---|---|
| K8a | `womb/kernel/sync/spinlock.vuma` |
| K8b | `womb/kernel/sync/mutex.vuma` |
| K8c | `womb/kernel/sync/semaphore.vuma` |
| K8d | `womb/kernel/sync/rwlock.vuma` |
| K8e | `womb/kernel/smp/ipi.vuma` |
| K8f | `womb/kernel/smp/smp.vuma` |

### K8a — Spinlock
**Contract:** `layout Spinlock = { locked:u32, holder:u32, depth:u32 }` (recursive). `fn spinlock_init() -> State<Spinlock>`, `fn spinlock_acquire(lock:State<Spinlock>) -> State<Spinlock>` — `AtomicCas` loop on `locked`, disable IRQs (save previous to percpu), set holder=cpu_id. `fn spinlock_release(lock:State<Spinlock>) -> State<Spinlock>` — restore IRQs, `AtomicStore(0, locked)`.
**DoD:**
- [ ] `Spinlock` verifies; `AtomicCas` used for acquire
- [ ] IRQs disabled on acquire, restored on release
- [ ] Recursive (same holder can re-acquire, depth++)

### K8b — Mutex (sleeping lock)
**Contract:** `layout Mutex = { locked:u32, owner:u32, waitq:u64 }`. `fn mutex_init() -> State<Mutex>`, `fn mutex_acquire(m:State<Mutex>) -> State<Mutex>` — `AtomicCas`; on fail, `waitq_add` + `sleep_on`. `fn mutex_release(m:State<Mutex>) -> State<Mutex>` — `AtomicStore(0)` + `waitq_wake_one`.
**DoD:**
- [ ] `Mutex` verifies; uses `AtomicCas` + wait queue
- [ ] Contended acquirer sleeps (not busy-wait)
- [ ] Release wakes one waiter

### K8c — Semaphore
**Contract:** `layout Semaphore = { count:u32, waitq:u64 }`. `fn sema_init(count:u32) -> State<Semaphore>`, `fn sema_down(s:State<Semaphore>) -> State<Semaphore>` — `AtomicCas` decrement; if would go negative, sleep. `fn sema_up(s:State<Semaphore>) -> State<Semaphore>` — increment + `waitq_wake_one`.
**DoD:**
- [ ] `Semaphore` verifies
- [ ] `sema_down` on 0 sleeps
- [ ] `sema_up` wakes one waiter

### K8d — RWLock
**Contract:** `layout RwLock = { readers:u32, writer:u32, write_waitq:u64, read_waitq:u64 }`. `fn rwlock_init() -> State<RwLock>`, `fn rwlock_read_acquire(l:State<RwLock>) -> State<RwLock>` — if no writer, readers++. `fn rwlock_read_release`. `fn rwlock_write_acquire` — if readers==0 && writer==0, set writer=cpu. `fn rwlock_write_release`.
**DoD:**
- [ ] `RwLock` verifies
- [ ] Multiple readers can hold simultaneously
- [ ] Writer is exclusive

### K8e — IPI subsystem
**Contract:** `fn ipi_send(cpu:u32, vector:u32)` — x86_64: `lapic_write(ICR, ...)`; aarch64: `gic_send_sgi(cpu, vector)`; riscv64: `sbi_send_ipi(cpu_mask)`. `fn ipi_broadcast(vector:u32)`. `fn ipi_register_handler(vector:u32, handler:u64)`. `extern "C"` trampolines per arch.
**DoD:**
- [ ] `ipi_send` + `ipi_broadcast` verify on all 3 archs
- [ ] Trampolines resolve on all 3 archs
- [ ] Handler registration works

### K8f — SMP boot + smp_call_function
**Contract:** `fn smp_boot(cpu_count:u32)` — for each secondary CPU, send startup IPI (x86_64: INIT-SIPI-SIPI; aarch64: PSCI CPU_ON; riscv64: HSM), wait for each to check in. `fn smp_call_function(func:u64, wait:u8)` — send IPI to all other CPUs, they call `func`. `layout CpuBoot = { started:u8, stack:u64, entry:u64 }`.
**DoD:**
- [ ] `smp_boot` verifies on all 3 archs
- [ ] `smp_call_function` broadcasts + optional wait
- [ ] 2-CPU QEMU boot (`-smp 2`) succeeds

### Dispatch Box — Wave K8

```
=== COMMON PREAMBLE (Wave K8) ===
You are Wave K8 of VWK. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md (K4+K7 complete). APPEND when done.
REPO: /home/z/vuma. BUILD: cargo build --profile release-fast --bin compile_dump.
TEST: ./target/release-fast/compile_dump <in.vuma> /tmp/o.bin <backend> --verify
RULES: No stubs. PMT-only. CARGO_BUILD_JOBS=1. Commit per task, do NOT push.
All locks use AtomicCas/AtomicStore (already in IR). Read womb/kernel/smp/
percpu.vuma (K4f) for per-CPU state.

=== K8a: Spinlock ===
Files: womb/kernel/sync/spinlock.vuma.
layout Spinlock{locked,holder,depth:u32}. spinlock_init/acquire (AtomicCas loop,
IRQ disable)/release (IRQ restore, AtomicStore 0). Recursive.
DoD: verifies; AtomicCas used; IRQs toggled; recursive.

=== K8b: Mutex ===
Files: womb/kernel/sync/mutex.vuma.
layout Mutex{locked,owner:u32,waitq:u64}. mutex_init/acquire (AtomicCas, sleep
on fail)/release (AtomicStore 0, wake_one). Uses waitq (K7e).
DoD: verifies; contended sleeps (not busy-wait); release wakes one.

=== K8c: Semaphore ===
Files: womb/kernel/sync/semaphore.vuma.
layout Semaphore{count:u32,waitq:u64}. sema_init/down (AtomicCas dec, sleep if
negative)/up (inc, wake_one).
DoD: verifies; down on 0 sleeps; up wakes one.

=== K8d: RWLock ===
Files: womb/kernel/sync/rwlock.vuma.
layout RwLock{readers,writer:u32,write_waitq,read_waitq:u64}. read_acquire
(if no writer, readers++)/read_release/write_acquire (exclusive)/write_release.
DoD: verifies; multiple readers concurrent; writer exclusive.

=== K8e: IPI subsystem ===
Files: womb/kernel/smp/ipi.vuma.
ipi_send(cpu,vector)/ipi_broadcast/ipi_register_handler. extern: lapic_write
(x86_64), gic_send_sgi (aarch64), sbi_send_ipi (riscv64).
DoD: ipi_send+broadcast verify on 3 archs; trampolines resolve; registration
works.

=== K8f: SMP boot + smp_call_function ===
Files: womb/kernel/smp/smp.vuma.
smp_boot(cpu_count) — x86_64 INIT-SIPI-SIPI, aarch64 PSCI CPU_ON, riscv64 HSM.
smp_call_function(func,wait). layout CpuBoot{started:u8,stack,entry:u64}.
DoD: smp_boot verifies on 3 archs; smp_call_function broadcasts; 2-CPU QEMU
boots.
```

---

## Wave K9 — Network Stack

**Goal:** Socket layer + virtio-net driver + TCP/DNS/HTTP migrated from `womb/net`. `curl http://10.0.2.2/` from inside the kernel returns bytes.
**Depends on:** K4, K8. **Max parallel:** 6.

| Task | Files |
|---|---|
| K9a | `womb/kernel/net/socket.vuma` |
| K9b | `womb/kernel/drivers/virtio_net.vuma` |
| K9c | `womb/kernel/net/sk_buff.vuma` |
| K9d | `womb/kernel/net/tcp.vuma` |
| K9e | `womb/kernel/net/dns.vuma` |
| K9f | `womb/kernel/net/http.vuma` |

### K9a — Socket layer
**Contract:** `layout Socket = { family:u32, type:u32, proto:u32, state:u8, rx_waitq:u64, tx_waitq:u64, private:u64 }`. `fn sys_socket(family:u32, type:u32, proto:u32) -> i64`, `fn sys_bind(fd:u64, addr:Address, len:u32) -> i64`, `fn sys_listen(fd:u64, backlog:u32) -> i64`, `fn sys_accept(fd:u64, addr:Address, len:Address) -> i64`, `fn sys_send(fd:u64, buf:Address, len:u64, flags:u32) -> i64`, `fn sys_recv(fd:u64, buf:Address, len:u64, flags:u32) -> i64`.
**DoD:**
- [ ] `Socket` layout verifies; 7 syscalls verify
- [ ] `socket(AF_INET, SOCK_STREAM, 0)` returns a valid fd
- [ ] `send`/`recv` round-trip

### K9b — virtio-net driver
**Contract:** `layout VirtioNetRegs = { magic:u32, version:u32, dev_id:u32, vendor:u32, host_features:u32, guest_features:u32, queue_sel:u32, queue_num:u32, queue_pfn:u32 }`. `fn virtio_net_init(base:u64) -> u32` (negotiate features, set up RX/TX virtqueues). `fn virtio_net_rx(buf:Address, len:u64) -> i64`. `fn virtio_net_tx(buf:Address, len:u64) -> i64`. Use `extern "C"` for MMIO + DMA.
**DoD:**
- [ ] Register layout verifies
- [ ] `virtio_net_init` + `rx` + `tx` verify on x86_64 + aarch64
- [ ] QEMU `-netdev user` + `-device virtio-net` works

### K9c — sk_buff (socket buffer)
**Contract:** `layout SkBuff = { data:[u8;1600], len:u16, head:u16, tail:u16, next:u64 }` (MTU 1500 + headroom). `layout SkBuffPool = { arena:State<Arena>, free_list:u64, count:u32 }`. `fn skb_alloc(pool:State<SkBuffPool>) -> u64`, `fn skb_free(pool:State<SkBuffPool>, skb:u64)`, `fn skb_put(skb:u64, len:u16) -> u64` (advance tail), `fn skb_push(skb:u64, len:u16) -> u64` (retreat head), `fn skb_reserve(skb:u64, len:u16)`.
**DoD:**
- [ ] `SkBuff` + `SkBuffPool` verify
- [ ] `skb_alloc` + `skb_free` round-trip
- [ ] `skb_put`/`push`/`reserve` adjust head/tail correctly

### K9d — TCP (migrate from womb/net/tcp.vuma)
**Contract:** Migrate `womb/net/tcp.vuma` from legacy pointer syntax to PMT. Replace `*(ptr+offset) = v` with `s.field = v`. Replace `allocate(N)` with `arena_alloc(arena, Layout)`. Replace `free(ptr)` with nothing (arena bulk-free). Keep the TCP state machine (SYN_SENT, ESTABLISHED, FIN_WAIT, etc.), segment parsing, congestion control.
**DoD:**
- [ ] `tcp.vuma` compiles + verifies with no pointer syntax
- [ ] TCP handshake (SYN/SYN-ACK/ACK) works
- [ ] Data send/recv over established connection

### K9e — DNS (migrate from womb/net/dns.vuma)
**Contract:** Migrate `womb/net/dns.vuma` to PMT. Build DNS query, send via UDP, parse response. `fn dns_resolve(name:Address) -> u32` (returns IPv4 addr).
**DoD:**
- [ ] `dns.vuma` compiles + verifies, no pointer syntax
- [ ] `dns_resolve("example.com")` returns an IP

### K9f — HTTP (migrate from womb/lib/http.vuma)
**Contract:** Migrate `womb/lib/http.vuma` to PMT. `fn http_get(host:Address, port:u16, path:Address, resp_buf:Address, resp_max:u64) -> i64` — connect TCP, send `GET path HTTP/1.0\r\nHost: host\r\n\r\n`, read response.
**DoD:**
- [ ] `http.vuma` compiles + verifies, no pointer syntax
- [ ] `http_get("10.0.2.2", 80, "/", buf, 4096)` returns bytes

### Dispatch Box — Wave K9

```
=== COMMON PREAMBLE (Wave K9) ===
You are Wave K9 of VWK. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md (K4+K8 complete). APPEND when done.
REPO: /home/z/vuma. BUILD: cargo build --profile release-fast --bin compile_dump.
TEST: ./target/release-fast/compile_dump <in.vuma> /tmp/o.bin <backend> --verify
RULES: No stubs. PMT-only. CARGO_BUILD_JOBS=1. Commit per task, do NOT push.
Migrations (K9d/e/f): read the original womb/net/*.vuma + womb/lib/*.vuma, replace
*(ptr+off)=v with s.field=v, allocate(N) with arena_alloc, drop free().

=== K9a: Socket layer ===
Files: womb/kernel/net/socket.vuma.
layout Socket{family,type,proto,state:u8,rx_waitq,tx_waitq,private:u64}. 7
syscalls: socket/bind/listen/accept/send/recv.
DoD: layout+7 syscalls verify; socket(AF_INET,SOCK_STREAM,0) returns fd;
send/recv round-trip.

=== K9b: virtio-net driver ===
Files: womb/kernel/drivers/virtio_net.vuma.
layout VirtioNetRegs. virtio_net_init (negotiate features, set up virtqueues)/
rx/tx. extern MMIO+DMA trampolines.
DoD: layout verifies; init+rx+tx verify on x86_64+aarch64; QEMU virtio-net
works.

=== K9c: sk_buff ===
Files: womb/kernel/net/sk_buff.vuma.
layout SkBuff{data:[u8;1600],len,head,tail:u16,next:u64}. layout SkBuffPool{arena,
free_list:u64,count:u32}. skb_alloc/free/put/push/reserve.
DoD: verifies; alloc+free round-trips; put/push/reserve adjust head/tail.

=== K9d: TCP migration ===
Files: womb/kernel/net/tcp.vuma.
Migrate womb/net/tcp.vuma to PMT. *(ptr+off)=v → s.field=v. allocate(N) →
arena_alloc. Drop free(). Keep state machine, segment parsing, congestion.
DoD: compiles+verifies, no pointer syntax; handshake works; data send/recv.

=== K9e: DNS migration ===
Files: womb/kernel/net/dns.vuma.
Migrate womb/net/dns.vuma to PMT. dns_resolve(name)->u32. UDP query+parse.
DoD: compiles+verifies, no pointer syntax; dns_resolve("example.com") returns IP.

=== K9f: HTTP migration ===
Files: womb/kernel/net/http.vuma.
Migrate womb/lib/http.vuma to PMT. http_get(host,port,path,resp_buf,resp_max)->i64.
TCP connect, send GET, read response.
DoD: compiles+verifies, no pointer syntax; http_get("10.0.2.2",80,"/",buf,4096)
returns bytes.
```

---

## Wave K10 — Crypto Subsystem

**Goal:** Migrate `womb/crypto/*` to PMT + hardware acceleration trampolines. Kernel signs a message with Ed25519 and verifies it; all KAT vectors pass.
**Depends on:** K4. **Max parallel:** 6.

| Task | Files |
|---|---|
| K10a | `womb/kernel/crypto/api.vuma` |
| K10b | `womb/kernel/crypto/aes.vuma` |
| K10c | `womb/kernel/crypto/sha.vuma` |
| K10d | `womb/kernel/crypto/asym.vuma` |
| K10e | `womb/kernel/crypto/hw_trampoline.vuma` |
| K10f | `tests/gold_standard/kernel_crypto/` |

### K10a — Crypto API wrapper
**Contract:** `layout CipherCtx = { key:[u8;32], iv:[u8;16], buf:[u8;64], len:u32 }`. `layout HashCtx = { state:[u64;8], buf:[u8;128], len:u32 }`. `fn cipher_init(algo:u32, key:Address, iv:Address) -> State<CipherCtx>`, `fn cipher_encrypt(ctx:State<CipherCtx>, in:Address, out:Address, len:u64)`, `fn hash_init(algo:u32) -> State<HashCtx>`, `fn hash_update(ctx:State<HashCtx>, data:Address, len:u64)`, `fn hash_final(ctx:State<HashCtx>, out:Address)`.
**DoD:**
- [ ] `CipherCtx` + `HashCtx` verify
- [ ] All 5 API functions verify
- [ ] Dispatches to AES/SHA/Ed25519 based on `algo`

### K10b — AES migration
**Contract:** Migrate `womb/crypto/{aes128,aes192,aes256,aes_modes}.vuma` to PMT. Replace `*(ptr+offset) = v` with `s.field = v`. Keep S-box, key schedule, round function, CBC/CTR/GCM modes. Combine into one `aes.vuma` with `fn aes_encrypt_block(key, in, out)`, `fn aes_cbc_encrypt(key, iv, in, out, len)`, etc.
**DoD:**
- [ ] `aes.vuma` compiles + verifies, no pointer syntax
- [ ] AES-128 ECB KAT passes (NIST FIPS-197)
- [ ] AES-CBC KAT passes

### K10c — SHA migration
**Contract:** Migrate `womb/crypto/{sha1,sha384,sha512,sha3,sha_variants}.vuma` to PMT. Combine into `sha.vuma` with `fn sha256_init() -> State<HashCtx>`, `fn sha256_update`, `fn sha256_final`, and equivalents for sha1/384/512/3.
**DoD:**
- [ ] `sha.vuma` compiles + verifies, no pointer syntax
- [ ] SHA-256("") = `e3b0c44298fc1c149afbf4c8996fb924...` KAT passes
- [ ] SHA-256("abc") = `ba7816bf8f01cfea414140de5dae2223...` KAT passes

### K10d — Asymmetric migration
**Contract:** Migrate `womb/crypto/{ed25519,ecdsa_p256,rsa,ml_kem}.vuma` to PMT. Combine into `asym.vuma` with `fn ed25519_sign(priv:Address, msg:Address, len:u64, sig:Address)`, `fn ed25519_verify(pub:Address, msg:Address, len:u64, sig:Address) -> u8`, `fn rsa_sign`, `fn rsa_verify`.
**DoD:**
- [ ] `asym.vuma` compiles + verifies, no pointer syntax
- [ ] Ed25519 sign+verify round-trips
- [ ] Ed25519 RFC 8032 test vector passes

### K10e — Hardware crypto trampolines
**Contract:** `extern "C"` trampolines for hardware-accelerated AES:
- x86_64: `aesni_encrypt_block(key:Address, in:Address, out:Address)` (uses `AESENC` instruction)
- aarch64: `aes_arm_encrypt_block(key:Address, in:Address, out:Address)` (uses `AESE` instruction)
- Fallback: if HW not available, call the PMT `aes_encrypt_block` from K10b.
`fn hw_aes_available() -> u8` — CPUID (x86_64) or feature register (aarch64).
**DoD:**
- [ ] Trampolines resolve on x86_64 + aarch64
- [ ] `hw_aes_available()` returns 1 on QEMU with `-cpu host`
- [ ] Fallback to PMT AES works when HW absent

### K10f — KAT integration tests
**Contract:** Add `tests/gold_standard/kernel_crypto/` with KAT test programs: `test_aes128_kat.vuma`, `test_sha256_kat.vuma`, `test_ed25519_kat.vuma`, `test_hmac_kat.vuma`. Each: `// Expected exit code: 0`. Each runs the crypto, compares to the known KAT answer, exits 0 on match.
**DoD:**
- [ ] All 4 KAT tests pass on x86_64 + aarch64
- [ ] Registered in gold-standard manifest
- [ ] No regression in existing `womb_kat_tests`

### Dispatch Box — Wave K10

```
=== COMMON PREAMBLE (Wave K10) ===
You are Wave K10 of VWK. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md (K4 complete). APPEND when done.
REPO: /home/z/vuma. BUILD: cargo build --profile release-fast --bin compile_dump.
TEST: ./target/release-fast/compile_dump <in.vuma> /tmp/o.bin <backend> --verify
RULES: No stubs. PMT-only. CARGO_BUILD_JOBS=1. Commit per task, do NOT push.
Migrations: read womb/crypto/*.vuma, replace *(ptr+off)=v with s.field=v,
allocate(N) with arena_alloc, drop free().

=== K10a: Crypto API wrapper ===
Files: womb/kernel/crypto/api.vuma.
layout CipherCtx{key:[u8;32],iv:[u8;16],buf:[u8;64],len:u32}. layout
HashCtx{state:[u64;8],buf:[u8;128],len:u32}. cipher_init/encrypt, hash_init/
update/final. Dispatch by algo.
DoD: layouts+5 fns verify; dispatches to AES/SHA/Ed25519.

=== K10b: AES migration ===
Files: womb/kernel/crypto/aes.vuma.
Migrate womb/crypto/{aes128,aes192,aes256,aes_modes}.vuma to PMT. Combine into
aes.vuma: aes_encrypt_block, aes_cbc_encrypt, etc. S-box, key schedule, rounds.
DoD: compiles+verifies, no pointer syntax; AES-128 ECB KAT passes; AES-CBC KAT
passes.

=== K10c: SHA migration ===
Files: womb/kernel/crypto/sha.vuma.
Migrate womb/crypto/{sha1,sha384,sha512,sha3,sha_variants}.vuma to PMT. Combine
into sha.vuma: sha256_init/update/final, etc.
DoD: compiles+verifies, no pointer syntax; SHA-256("") KAT passes; SHA-256("abc")
KAT passes.

=== K10d: Asymmetric migration ===
Files: womb/kernel/crypto/asym.vuma.
Migrate womb/crypto/{ed25519,ecdsa_p256,rsa,ml_kem}.vuma to PMT. Combine into
asym.vuma: ed25519_sign/verify, rsa_sign/verify.
DoD: compiles+verifies, no pointer syntax; Ed25519 sign+verify round-trips;
RFC 8032 test vector passes.

=== K10e: Hardware crypto trampolines ===
Files: womb/kernel/crypto/hw_trampoline.vuma.
extern aesni_encrypt_block (x86_64 AESENC), aes_arm_encrypt_block (aarch64 AESE).
hw_aes_available() — CPUID/feature reg. Fallback to K10b PMT AES.
DoD: trampolines resolve on x86_64+aarch64; hw_aes_available()=1 on QEMU -cpu
host; fallback works.

=== K10f: KAT integration tests ===
Files: tests/gold_standard/kernel_crypto/ (new).
test_aes128_kat.vuma, test_sha256_kat.vuma, test_ed25519_kat.vuma,
test_hmac_kat.vuma. Each: Expected exit code 0. Run crypto, compare to KAT.
DoD: 4 KAT tests pass on x86_64+aarch64; in gold manifest; no regression.
```

---

## Wave K11 — Multi-Backend Parity Sweep

**Goal:** Port the kernel to all remaining backends. Bare-metal on ppc64le, loongarch64, arm32, s390x, mips64. Hosted mode on wasm32, alpha, hppa, m68k, sparc64, x86_32, aarch64_be, armeb, mips64be, ppc64.
**Depends on:** K1–K10. **Max parallel:** 6.

| Task | Files |
|---|---|
| K11a | `womb/kernel/arch/{ppc64le,loongarch64}/` |
| K11b | `womb/kernel/arch/{arm32,s390x}/` |
| K11c | `womb/kernel/arch/mips64/` |
| K11d | `womb/kernel/hosted/host.vuma`, `womb/kernel/arch/hosted/` |
| K11e | `womb/kernel/arch/{wasm32,alpha,hppa,m68k}/` (hosted) |
| K11f | `scripts/kernel_parity.sh` |

### K11a — ppc64le + loongarch64 port
**Contract:** For each: write `boot.S` (QEMU pSeries/MIPS-virt entry), `bootinfo.vuma`, `trampoline.vuma` (MMIO + MSR/CSR access), `trap.S`, `switch.S`, `pt.vuma` (Radix Tree for ppc64le, Sv48-like for loongarch64), `mm_trampoline.vuma`. Mirror the x86_64/aarch64/riscv64 structure from K1–K8.
**DoD:**
- [ ] Both archs boot to `vuma kernel: hello` in QEMU
- [ ] `kmalloc` + scheduling + serial console work
- [ ] Gold smoke test passes on both

### K11b — arm32 + s390x port
**Contract:** Same as K11a but for arm32 (QEMU versatile-pb, ARMv7-A, 2-level paging) and s390x (QEMU s390x, ESA/390, region table paging). Per-arch: `boot.S`, `bootinfo.vuma`, `trampoline.vuma`, `trap.S`, `switch.S`, `pt.vuma`, `mm_trampoline.vuma`.
**DoD:**
- [ ] Both archs boot to `vuma kernel: hello` in QEMU
- [ ] `kmalloc` + scheduling + serial console work

### K11c — mips64 port
**Contract:** Same as K11a but for mips64 (QEMU malta, MIPS64 R6, O64 ABI, TLB-based paging). Per-arch files as above. Handle the TLB (not page tables) — `tlbwi`/`tlbwr` instructions.
**DoD:**
- [ ] mips64 boots to `vuma kernel: hello` in QEMU
- [ ] TLB-based VMM works (different from page-table archs)

### K11d — Hosted mode framework
**Contract:** `womb/kernel/hosted/host.vuma`: `fn host_init()` — sets up a Linux process as a "kernel simulator". `womb/kernel/arch/hosted/`: `boot.S` is a no-op (just `call main`), `trampoline.vuma` maps all `extern "C"` to libc (`outb`→`printf`, `mmap`→`mmap`, etc.), `trap.S` is a no-op (no real traps), `pt.vuma` uses a fake page table (just a hash map in arena). The hosted kernel runs as `./kernel.bin` on Linux.
**DoD:**
- [ ] `compile_dump kernel.vuma /tmp/k.bin wasm32` succeeds (or any hosted backend)
- [ ] `./k.bin` (or `wasmtime k.bin`) prints `vuma kernel: hello`
- [ ] `kmalloc` + scheduling logic runs (single-threaded simulation)

### K11e — Hosted ports (wasm32, alpha, hppa, m68k, sparc64, x86_32, aarch64_be, armeb, mips64be, ppc64)
**Contract:** For each hosted backend, create `womb/kernel/arch/<isa>/` with the hosted-mode files (thin wrappers around `womb/kernel/hosted/`). Each `boot.S` calls `main` directly. Each `trampoline.vuma` maps to the backend's available syscalls/imports. wasm32 uses WASI (`proc_exit`, `fd_write`); others use Linux user-space syscalls.
**DoD:**
- [ ] All 10 hosted backends compile `kernel.vuma`
- [ ] All 10 print `vuma kernel: hello` when run
- [ ] Gold smoke test passes on all 10

### K11f — Parity test harness
**Contract:** `scripts/kernel_parity.sh` — for each of 19 backends, compile `kernel.vuma`, run (QEMU for bare-metal, native/wasmtime for hosted), grep for `vuma kernel: hello`, report pass/fail matrix. Exit 0 only if all 19 pass.
**DoD:**
- [ ] Script runs all 19 backends
- [ ] Outputs a 19-row pass/fail table
- [ ] Exits 0 only on 19/19

### Dispatch Box — Wave K11

```
=== COMMON PREAMBLE (Wave K11) ===
You are Wave K11 of VWK. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md (K1-K10 complete). APPEND when done.
REPO: /home/z/vuma. BUILD: cargo build --profile release-fast --bin compile_dump.
TEST: ./target/release-fast/compile_dump kernel.vuma /tmp/k.bin <backend> --verify
RULES: No stubs. PMT-only. CARGO_BUILD_JOBS=1. Commit per task, do NOT push.
Mirror the x86_64/aarch64/riscv64 arch structure from K1-K8 for each new arch.
Hosted mode: boot.S calls main directly; trampolines map to libc/WASI.

=== K11a: ppc64le + loongarch64 port ===
Files: womb/kernel/arch/{ppc64le,loongarch64}/{boot.S,bootinfo.vuma,
trampoline.vuma,trap.S,switch.S,pt.vuma,mm_trampoline.vuma,linker.ld}.
ppc64le: QEMU pSeries, Radix Tree paging. loongarch64: QEMU virt, Sv48-like.
Mirror x86_64/aarch64 structure.
DoD: both boot to "vuma kernel: hello" in QEMU; kmalloc+sched+serial work.

=== K11b: arm32 + s390x port ===
Files: womb/kernel/arch/{arm32,s390x}/{...same files...}.
arm32: QEMU versatile-pb, ARMv7-A, 2-level paging. s390x: QEMU s390x, ESA/390,
region table paging.
DoD: both boot to "vuma kernel: hello"; kmalloc+sched+serial work.

=== K11c: mips64 port ===
Files: womb/kernel/arch/mips64/{...same files...}.
QEMU malta, MIPS64 R6, O64 ABI. TLB-based paging (tlbwi/tlbwr).
DoD: boots to "vuma kernel: hello"; TLB-based VMM works.

=== K11d: Hosted mode framework ===
Files: womb/kernel/hosted/host.vuma, womb/kernel/arch/hosted/{boot.S,
trampoline.vuma,trap.S,pt.vuma,linker.ld}.
host.vuma: host_init (Linux process simulator). boot.S: call main. trampolines
map to libc. trap.S: no-op. pt.vuma: fake hash-map page table.
DoD: compiles on a hosted backend; runs + prints "vuma kernel: hello"; kmalloc
+sched logic runs single-threaded.

=== K11e: Hosted ports (10 backends) ===
Files: womb/kernel/arch/{wasm32,alpha,hppa,m68k,sparc64,x86_32,aarch64_be,
armeb,mips64be,ppc64}/{boot.S,trampoline.vuma,linker.ld}.
Thin wrappers around womb/kernel/hosted/. wasm32 uses WASI; others use Linux
user-space syscalls.
DoD: all 10 compile kernel.vuma; all 10 print "vuma kernel: hello"; gold smoke
passes.

=== K11f: Parity test harness ===
Files: scripts/kernel_parity.sh.
For each of 19 backends: compile kernel.vuma, run (QEMU/native/wasmtime), grep
for "vuma kernel: hello", report pass/fail. Exit 0 only on 19/19.
DoD: runs all 19; outputs 19-row table; exits 0 on 19/19.
```

---

## Wave K12 — Docs, Panic, kmsg, Power

**Goal:** Panic handler with stack trace, kmsg ring buffer, power management, and full documentation.
**Depends on:** K1–K11. **Max parallel:** 6.

| Task | Files |
|---|---|
| K12a | `womb/kernel/panic/panic.vuma` |
| K12b | `womb/kernel/panic/kmsg.vuma` |
| K12c | `womb/kernel/power/pm.vuma` |
| K12d | `docs/kernel-architecture.md` |
| K12e | `docs/kernel-porting-guide.md` |
| K12f | `docs/kernel-developer-guide.md` |

### K12a — Panic handler
**Contract:** `fn panic(msg:Address) -> !` — disable IRQs, print `\n*** KERNEL PANIC ***\n`, print message, print stack trace (walk FP chain via `extern "C"` `frame_pointer_walk`), print current Task pid + name, halt (`wfi`/`hlt`/`1: jmp 1b`). `fn assert(cond:u8, msg:Address)` — if cond==0, call `panic`.
**DoD:**
- [ ] `panic` verifies on all backends; never returns (`-> !`)
- [ ] Stack trace prints at least 8 frames
- [ ] `assert(0, "test")` triggers panic

### K12b — kmsg ring buffer
**Contract:** `layout KmsgBuf = { buf:[u8;16384], head:u32, tail:u32, seq:u64 }`. `fn kmsg_init() -> State<KmsgBuf>`, `fn kmsg_write(s:State<KmsgBuf>, msg:Address, len:u64) -> State<KmsgBuf>` (appends, wraps at 16KB, increments seq). `fn kmsg_read(s:State<KmsgBuf>, buf:Address, max:u64) -> i64`. `fn dmesg()` — print all kmsg to console.
**DoD:**
- [ ] `KmsgBuf` with 16KB ring verifies
- [ ] `kmsg_write` wraps correctly at 16KB boundary
- [ ] `dmesg` prints the full buffer

### K12c — Power management
**Contract:** `layout PmState = { cpu_state:u8, suspend_lvl:u8, wake_count:u32 }`. `fn pm_init() -> State<PmState>`, `fn pm_cpu_idle()` — x86_64: `hlt`; aarch64: `wfi`; riscv64: `wfi`. `fn pm_cpu_off(cpu:u32)` — PSCI CPU_OFF (aarch64) or equivalent. `fn pm_suspend(state:u8) -> i64`. `extern "C"`: `wfi`/`hlt`/`mwait` per arch.
**DoD:**
- [ ] `PmState` verifies
- [ ] `pm_cpu_idle` verifies on all 3 primary archs
- [ ] Trampolines resolve on all 3 archs

### K12d — Architecture doc
**Contract:** `docs/kernel-architecture.md` — cover the 4-layer cake, boot flow, PMT-in-the-kernel design, arena memory model, per-arch abstraction, FFI trampoline patterns, IVE guarantees for kernel state. ~2000 words. Diagrams in ASCII art. Cross-reference `docs/architecture.md` (VUMA 2.0) for the compiler side.
**DoD:**
- [ ] Doc covers all 8 sections
- [ ] ASCII diagrams for 4-layer cake + boot flow + memory layout
- [ ] Cross-references existing VUMA docs

### K12e — Porting guide
**Contract:** `docs/kernel-porting-guide.md` — step-by-step: (1) pick arch, (2) write `boot.S`, (3) write `bootinfo.vuma`, (4) write `trampoline.vuma`, (5) write `trap.S`, (6) write `switch.S`, (7) write `pt.vuma`, (8) write `mm_trampoline.vuma`, (9) run smoke test. Include a checklist. Use x86_64 as the worked example.
**DoD:**
- [ ] 9-step guide with x86_64 worked example
- [ ] Checklist at the end
- [ ] References to exact file templates

### K12f — Developer guide
**Contract:** `docs/kernel-developer-guide.md` — how to add a new syscall, how to add a new driver, how to add a new filesystem, how to write PMT kernel code (no pointers, use State<T>, transforms for ownership transfer). Include "do/don't" examples. Link to the wave dispatch boxes for contribution workflow.
**DoD:**
- [ ] 4 "how to" sections (syscall, driver, fs, PMT coding)
- [ ] Do/don't code examples for each
- [ ] Contribution workflow references TASKS.md waves

### Dispatch Box — Wave K12

```
=== COMMON PREAMBLE (Wave K12) ===
You are Wave K12 of VWK. Task ID: <assigned>.
READ FIRST: /home/z/my-project/worklog.md (K1-K11 complete). APPEND when done.
REPO: /home/z/vuma. BUILD: cargo build --profile release-fast --bin compile_dump.
TEST: ./target/release-fast/compile_dump <in.vuma> /tmp/o.bin <backend> --verify
RULES: No stubs. PMT-only. CARGO_BUILD_JOBS=1. Commit per task, do NOT push.

=== K12a: Panic handler ===
Files: womb/kernel/panic/panic.vuma.
panic(msg)->! (disable IRQs, print banner+msg+stack trace+current task, halt).
assert(cond,msg) — if cond==0 panic. extern frame_pointer_walk.
DoD: verifies on all backends; never returns; stack trace prints 8 frames;
assert(0) triggers panic.

=== K12b: kmsg ring buffer ===
Files: womb/kernel/panic/kmsg.vuma.
layout KmsgBuf{buf:[u8;16384],head,tail:u32,seq:u64}. kmsg_init/write (wraps at
16KB)/read/dmesg.
DoD: 16KB ring verifies; write wraps; dmesg prints full buffer.

=== K12c: Power management ===
Files: womb/kernel/power/pm.vuma.
layout PmState{cpu_state,suspend_lvl:u8,wake_count:u32}. pm_init/cpu_idle/cpu_off/
suspend. extern wfi/hlt/mwait per arch.
DoD: verifies; pm_cpu_idle verifies on 3 primary archs; trampolines resolve.

=== K12d: Architecture doc ===
Files: docs/kernel-architecture.md.
4-layer cake, boot flow, PMT-in-kernel design, arena model, per-arch abstraction,
FFI trampoline patterns, IVE guarantees. ~2000 words. ASCII diagrams. Cross-ref
docs/architecture.md.
DoD: 8 sections; ASCII diagrams; cross-references existing docs.

=== K12e: Porting guide ===
Files: docs/kernel-porting-guide.md.
9-step guide (boot.S, bootinfo, trampoline, trap.S, switch.S, pt, mm_trampoline,
smoke test). x86_64 worked example. Checklist.
DoD: 9 steps with example; checklist; file template references.

=== K12f: Developer guide ===
Files: docs/kernel-developer-guide.md.
How to add syscall/driver/fs. How to write PMT kernel code (no pointers, State<T>,
transforms). Do/don't examples. Link to wave dispatch boxes.
DoD: 4 how-to sections; do/don't examples; contribution workflow references.
```

---

## 6. Global Definition of Done

- [ ] `compile_dump kernel.vuma kernel.bin <backend>` succeeds for all 19 backends
- [ ] `qemu-system-<arch> -kernel kernel.bin -serial stdio` boots to shell on x86_64, aarch64, riscv64, ppc64le, loongarch64
- [ ] Hosted-mode kernel runs as a Linux/WASI process on the remaining backends
- [ ] PMT invariants hold: 0 use-after-free, 0 double-free, 0 aliasing in process table, page table, FD table — proven at compile time by IVE
- [ ] Two user tasks ping-pong via pipe + futex; Ctrl-C delivers SIGINT
- [ ] `cat /etc/motd` from initramfs works
- [ ] Crypto subsystem passes all KAT vectors in-kernel
- [ ] Gold-standard suite still 100% green (no regression from kernel additions)
- [ ] `docs/kernel-{architecture,porting-guide,developer-guide}.md` published
- [ ] `scripts/kernel_parity.sh` reports 19/19

---

## 7. Success Criteria

1. **One source.** A single `womb/kernel/**` tree, 19 ELF outputs, zero `#ifdef` in kernel logic.
2. **Memory-safety by construction.** An entire class of kernel CVEs (UAF in proc table, double-map of pages, FD type confusion) is *structurally impossible* — the IVE rejects the source before the kernel boots.
3. **Real kernel, not a toy.** Scheduling, VFS, IPC, signals, syscalls, MM, drivers, TTY, crypto, net — all present.
4. **No backend left behind.** The 19-backend gold matrix extends to the kernel. Bare-metal on 8; hosted on 11.
5. **PMT-pure.** Zero pointer syntax in `womb/kernel/**`. The 6 sacred invariants hold.

---

*This spec is a living document. Update the DoD checkboxes as waves complete. Record deviations in `/home/z/my-project/worklog.md`. Each wave's dispatch box is paste-ready for a subagent.*
