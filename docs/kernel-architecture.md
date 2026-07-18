# VWK Kernel Architecture

This document describes the architecture of the **VWK** (Vuma Womb Kernel) — the
PMT-native kernel written in VUMA 2.0 and built across waves K0–K12 of the VWK
effort. It is a companion to [`architecture.md`](./architecture.md), which
documents the VUMA 2.0 *compiler* (parser, SCG, IVE verifiers, codegen, 19
backends). Where `architecture.md` answers "how does the compiler work?", this
document answers "how does the kernel that compiles down with it work?".

The kernel is **PMT-only**: there is no pointer syntax (`*T`, `&x`, `allocate`,
`free`), no `--pmt` flag, no escape hatch. Every kernel module is a composition
of typed-state transformations over arena-allocated `State<T>` buffers. The
compiler's three IVE state verifiers (`StateRead`, `StateWrite`,
`StateTransform`) discharge all memory-safety obligations at compile time; the
runtime arena (`runtime/arena.rs` + `__arena_overflow` trap on all 19 backends)
discharges the only remaining runtime obligation — out-of-arena bounds. This
document explains how those guarantees map to the four runtime layers of the
kernel.

---

## Table of Contents

1. [The Four-Layer Cake](#1-the-four-layer-cake)
2. [Boot Flow](#2-boot-flow)
3. [PMT-in-the-Kernel Design](#3-pmt-in-the-kernel-design)
4. [Arena Memory Model](#4-arena-memory-model)
5. [Per-Architecture Abstraction](#5-per-architecture-abstraction)
6. [FFI Trampoline Patterns](#6-ffi-trampoline-patterns)
7. [IVE Guarantees for Kernel State](#7-ive-guarantees-for-kernel-state)
8. [Complete File Inventory](#8-complete-file-inventory)
9. [Stub Inventory](#9-stub-inventory)
10. [VUMA Parser Limitations](#10-vuma-parser-limitations)
11. [Data Flow Diagrams](#11-data-flow-diagrams)
12. [Memory Layout](#12-memory-layout)
13. [Error Code Convention](#13-error-code-convention)
14. [Sentinel Value Convention](#14-sentinel-value-convention)
15. [Cross-Compilation](#15-cross-compilation)
16. [Testing](#16-testing)
17. [Cross-References](#17-cross-references)

---

## 1. The Four-Layer Cake

The VWK kernel is a four-layer system. Each layer is a complete, verifiable
compilation unit; layers compose by **byte-identical re-declaration** (VUMA has
no `import` yet — Open Work §7 — so each consumer of a layout or extern
re-declares it from the canonical source).

```
┌────────────────────────────────────────────────────────────────────────┐
│  L4 — PMT Kernel Logic (womb/kernel/*.vuma)                            │
│  ───────────────────────────────────────────────────────────────────── │
│    console.vuma  kernel.vuma                                          │
│    mm/{pmm,vmm,kmalloc,mmap}.vuma                                      │
│    trap/{trap,irq}.vuma   proc/{task,scheduler,exec,fork,wait,exit}    │
│    vfs/{inode,dentry,file,namei,mount,file_ops}.vuma                    │
│    fs/{tmpfs,initramfs}.vuma   tty/{console,line_discipline,vt100}     │
│    ipc/{pipe,waitq,shm,futex,signal}.vuma                              │
│    sync/{spinlock,mutex,semaphore,rwlock}.vuma                         │
│    smp/{smp,percpu,ipi}.vuma   net/{socket,sk_buff,tcp,dns,http}       │
│    drivers/{uart,char,virtio_net}.vuma                                 │
│    syscall/{abi,table,dispatch,handlers/{io,mm,proc}}.vuma             │
│    crypto/{api,aes,sha,asym,hw_trampoline}.vuma                        │
│    panic/{panic,kmsg}.vuma   power/pm.vuma   shell/shell.vuma          │
│    hosted/host.vuma                                                    │
│                                                                        │
│  State: pure PMT — State<T>, state_new, layout field access.           │
│  Verification: IVE StateRead + StateWrite + StateTransform (compile).  │
└────────────────────────────────────────────────────────────────────────┘
                                 ▲  (extern "C" + State<T> as Address casts)
┌────────────────────────────────────────────────────────────────────────┐
│  L3 — Arena Runtime (runtime/arena.rs + 19 backend __arena_overflow)   │
│  ───────────────────────────────────────────────────────────────────── │
│    bump allocator over ___pmt_buffer (capacity from BootInfo.mem_size) │
│    arena_alloc, arena_new, arena_overflow trap                         │
│    State<T> lowered to (base_addr, LayoutId) — all field access        │
│      lowered to Load/Store at compile-time-known offset+size           │
└────────────────────────────────────────────────────────────────────────┘
                                 ▲  (called by FFI trampolines)
┌────────────────────────────────────────────────────────────────────────┐
│  L2 — FFI Trampolines (womb/kernel/arch/<arch>/*.vuma)                 │
│        x86_64 only: trampoline.vuma + mm_trampoline.vuma +             │
│                    trap_trampoline.vuma + switch.vuma + pt.vuma +      │
│                    bootinfo.vuma (6 files)                             │
│        aarch64 / riscv64: mm_trampoline.vuma + trap_trampoline.vuma +  │
│                    switch.vuma + pt.vuma (4 files each — no            │
│                    trampoline.vuma, no bootinfo.vuma)                  │
│  ───────────────────────────────────────────────────────────────────── │
│    extern "C" { fn write(...); fn mmap(...); fn context_switch(...); } │
│    Hosted: pre-registered Linux-syscall stubs in x86_64 backend.       │
│    Bare-metal (K11+): real asm stubs registered in backend.            │
│    Unregistered externs → __ffi_fallback_stub (xor eax,eax; ret).      │
└────────────────────────────────────────────────────────────────────────┘
                                 ▲  (asm entry stubs)
┌────────────────────────────────────────────────────────────────────────┐
│  L1 — boot.S (hosted: _start in backend; bare-metal: multiboot entry)  │
│  ───────────────────────────────────────────────────────────────────── │
│    Set up argc/argv in BSS slot; call main().                          │
│    Bare-metal: parse multiboot2/stivale2, set up GDT/IDT/paging,       │
│    jump to kmain().                                                    │
└────────────────────────────────────────────────────────────────────────┘
```

The four layers differ in **who proves what**:

| Layer | Proved by           | Memory-safety mechanism                    |
|-------|---------------------|--------------------------------------------|
| L1    | asm review          | Manual; constrained entry invariants       |
| L2    | asm review + ABI    | SysV/AArch64/RV ABI compliance             |
| L3    | runtime arena check | `__arena_overflow` trap on alloc > cap     |
| L4    | IVE (compile time)  | `StateRead`/`StateWrite`/`StateTransform`  |

Only L4 is PMT-pure. L1–L3 are the *substrate* that the PMT verifiers target;
they themselves live outside PMT but are constrained to interfaces the verifiers
can reason about.

A subtle consequence of the L2 split is that **only `x86_64` has a
`trampoline.vuma` and a `bootinfo.vuma`** today. The aarch64 and riscv64 ports
were added later (K2c and K2d respectively) and re-use the hosted-mode x86_64
syscall stubs and BootInfo path — there is no separate hosted aarch64 or
riscv64 trampoline. When K11 brings up real bare-metal ports for those arches,
each will gain its own `bootinfo.vuma` (parsing SBI/multiboot) and a
`trampoline.vuma` (declaring its own SBI-convention syscall stubs).

---

## 2. Boot Flow

The boot flow is intentionally minimal today and extends by appending one line
per subsystem to `kmain()`.

```
                ┌──────────────────────┐
                │  L1 boot.S _start    │
                │  (hosted: backend)   │
                └──────────┬───────────┘
                           │  populates argc/argv in BSS slot
                           ▼
                ┌──────────────────────┐
                │  fn main() -> i32    │   (womb/kernel/kernel.vuma)
                │  return kmain();     │
                └──────────┬───────────┘
                           │
                           ▼
                ┌──────────────────────┐
                │  fn kmain() -> i32   │   (womb/kernel/kernel.vuma —
                │  c ← console_init()  │    inlined from the deleted
                │  kmain_print_banner  │    kmain.vuma during CLEANUP-1)
                │  return 0            │
                └──────────────────────┘
```

Future-wave additions (documented in `kernel.vuma`'s `kmain()` body, currently
inlined verbatim from the historical `kmain.vuma`):

```
    pmm_init(c)   ← K2a physical memory manager (buddy)
    vmm_init(c)   ← K2b virtual memory / page tables
    trap_init(c)  ← K3a/K3d IDT + exception/syscall dispatch
    task_init(c)  ← K4a Task control block + ProcessTable
    sched_init(c) ← K4b scheduler + first task
    vfs_init(c)   ← K5  rootfs + init binary
```

> **Note on K-wave numbering.** Earlier drafts of this document collapsed
> "K2=memory, K3=trap, K4=scheduler, K5=vfs, K6=…". The actual codebase is
> finer-grained: K2 splits into K2a (`pmm.vuma`) and K2b (`vmm.vuma`); K3
> splits into K3a (`arch/x86_64/trap_trampoline.vuma` — the TrapFrame layout
> + externs), K3d (`trap/trap.vuma` — the dispatcher + `trap_irq.vuma`
> companion), and K3e (`syscall/{abi,table,dispatch}.vuma`); K4 splits into
> K4a (`proc/task.vuma`) and K4b (`proc/scheduler.vuma`); K5 is the entire
> VFS layer (`vfs/{inode,dentry,file,namei,mount,file_ops}.vuma` plus
> `fs/{tmpfs,initramfs}.vuma`). There is no K6 in the kernel-waves contract
> — K6 through K12 cover sync, SMP, net, drivers, syscall handlers, crypto,
> panic/kmsg/pm. See the worklog for the canonical wave map.

Each subsystem init takes the shared `State<Console>` for logging and returns
`i32` (or `void` in the canonical form). Because VUMA has no `import`, each
subsystem that wants to call into `console_putc`/`console_flush` re-declares
the `Console` layout + `write` extern byte-identically (see §6).

The boot flow's only hardware dependencies today are the two pre-registered
x86_64 stubs `write` (sys_write) and `exit` (sys_exit). Every other extern
(`mmap`, `munmap`, `cr3_write`, `context_switch`, `irq_mask`, `idt_load`,
`halt`, `wfi`, `aesni_encrypt_block`, ...) resolves to `__ffi_fallback_stub`
on hosted x86_64 — i.e. returns 0 / no-ops — and to real asm stubs on the
bare-metal target (K11+).

---

## 3. PMT-in-the-Kernel Design

The kernel's PMT discipline rests on four invariants, all enforced by the
compiler:

1. **No pointer types.** `*T`, `&x`, `allocate`, and `free` are hard parse
   errors. There is no raw-memory dereference site to audit.

2. **Every allocation is `state_new(Layout)`.** The arena runtime bumps the
   `___pmt_buffer` offset by `Layout.size` and returns a `(base_addr, LayoutId)`
   pair. The runtime bounds check (`arena_alloc` in `src/pipeline.rs`) loads
   the arena capacity (stored at `[arena_ptr+16]`), compares the new offset
   against it, and traps via `__arena_overflow` on overflow. K0 added this
   check; before K0 the trap was unreachable.

3. **Every field access is `state.field` or `state.field = value`.** The
   codegen lowers these to `Load(off, size)` / `Store(off, size)` against the
   state's base address. The offset and size are compile-time constants
   (registered in the `LayoutRegistry`); the IVE `StateReadVerifier` and
   `StateWriteVerifier` discharge in-bounds + linear-ownership obligations.

4. **Ownership transfer is via `transform`, not pointer aliasing.** When a
   function needs to consume a state (e.g. hand its buffer to FFI for
   in-place modification and reclaim it), it uses the `StateTransform`
   verifier — the source layout and destination layout must be
   layout-compatible. The kernel's idiom is the **`State<T> as Address` cast**
   at the FFI boundary (§6), which is the only sanctioned "lossy" ownership
   transfer in the language.

The kernel exploits these invariants uniformly: `pmm.vuma` stores free-list
heads in `[u8; 88]` (packed little-endian u64 slots), `kmsg.vuma` runs a 256-byte
ring buffer with `& 255` wraparound, `tcp.vuma` keeps a parallel-flat-byte-array
connection table — all PMT-pure, all arena-bounded, all IVE-verified.

### The init-style API pattern

Because the current codegen does not propagate `State`-typedness through
function return values (Open Work §"Pipeline: State-typedness through return
values"), every kernel subsystem uses the **init-style API**:

```
    // Caller allocates:
    let pmm = state_new(PmmState);
    let pool = state_new(FlatPool);
    // Caller passes by reference:
    pmm_init(pool, pmm, mem_start, mem_size);
    // Caller reads fields back:
    pmm_alloc(pmm, order);     // returns u64 page-frame address
```

This pattern is documented in `womb/kernel/mm/pmm.vuma::"Why init-style?"` and
is used by `pmm_init`, `vmm_init`, `trap_frame_init`, `task_init_for_switch`,
`syscall_args_from_frame`, `kmsg_init`, `pm_init`, and every other stateful
subsystem. K11+ will eventually flip the codegen to propagate `State` through
returns; until then, init-style is the canonical kernel convention.

A historical data point: K3e's `syscall_args_from_frame` was originally
written in return-style (`fn syscall_args_from_frame(tf) -> State<SyscallArgs>`).
The codegen emitted four `WARNING: unsupported FieldAccess (not state-typed)`
diagnostics from `flatten_expr`, and the self-test exited 1 (the caller's
`args.nr` access silently returned 0). The fix was to flip to init-style
(caller allocates `args = state_new(SyscallArgs)`, function populates it).
The wave K3e worklog entry records the regression. Every kernel contributor
since has been spared the same trap by this pattern.

---

## 4. Arena Memory Model

Every `state_new(Layout)` call lowers to a bump allocation against the
per-program arena `___pmt_buffer`:

```
   ___pmt_buffer  ┌────────────────────────────────────────────┐
                  │  arena header (32 B):                      │
                  │    [ptr+0]   base address (== ptr itself)  │
                  │    [ptr+8]   current offset                │
                  │    [ptr+16]  capacity  (BootInfo.mem_size) │
                  │    [ptr+24]  overflow-handler fn ptr       │
                  ├────────────────────────────────────────────┤
                  │  State<Console>           (260 B)          │
                  ├────────────────────────────────────────────┤
                  │  State<PmmState>          (~1 KB)          │
                  ├────────────────────────────────────────────┤
                  │  State<FlatPool>          (~9 KB)          │
                  ├────────────────────────────────────────────┤
                  │  State<Task> × N           (120 B each)    │
                  ├────────────────────────────────────────────┤
                  │  ...                                       │
                  ▼                                            │
                 bump offset ──────►  (grows toward capacity)  │
                  ───────────────────────────────────────────  │
                  │  unmapped / overflow zone                  │
                  └────────────────────────────────────────────┘
```

The capacity is 16 MB in hosted mode (`BootInfo.mem_size = 16777216`,
`womb/kernel/arch/x86_64/bootinfo.vuma`). The `arena_alloc` codegen sequence
(added in K0) is:

```
    new_offset = offset + layout_size
    if new_offset > load(capacity, [arena_ptr + 16]):
        call __arena_overflow(layout_id)
    store(new_offset, [arena_ptr + 8])
    return arena_ptr + old_offset
```

The `__arena_overflow` symbol is defined on all 19 backends as a trap
instruction (`ud2` on x86_64, `brk #0` on aarch64, `unimp` on riscv64, etc.).
On hosted x86_64 the trap is caught by the kernel's signal handler and
surfaced as a non-zero exit code; on bare metal it halts the CPU.

Because every kernel allocation goes through this single arena, **the entire
kernel has one deallocation site** — program exit. There is no `free`, no
`drop`, no `munmap`-of-arena-state. The arena's lifetime == the process's
lifetime. This is the source of the "liveness discharged by construction"
property in the IVE state verifiers (see `docs/architecture.md` §1).

The arena is **not** garbage-collected; allocations are permanent. Subsystems
that need reclamation use **slot recycling** instead: `pmm.vuma`'s `FlatPool`
has a `count` field and `nodes[i].free = 1` markers; `vfs/file.vuma`'s
`FdTable` reuses freed fd indices; `proc/scheduler.vuma` reuses freed pid slots.
This matches the "transform over typed state" model — reclamation is a state
mutation, not a memory operation.

### Why no `mmap`-of-State<T>?

The `hosted/host.vuma` module declares `host_alloc_pages` (a thin wrapper over
the `mmap` extern), but it returns a raw `Address`, not a `State<T>`. The PMT
verifiers only reason about states allocated via `state_new(...)` — they have
no way to attach a layout to an externally-mmap'd region. K11+ may add a
`state_extern(Layout, addr)` intrinsic that registers an externally-allocated
buffer as a State<T> for verification purposes; until then, kernel code that
needs an mmap'd region uses the hosted `host_alloc_pages` path and treats the
result as an opaque `Address` (passed to FFI but never `state.field`-accessed).

---

## 5. Per-Architecture Abstraction

The kernel currently targets three architectures, each in
`womb/kernel/arch/<arch>/`. The file counts differ per arch — only `x86_64`
has the hosted-mode `trampoline.vuma` and `bootinfo.vuma`; aarch64 and riscv64
have only the four "core" arch files:

```
    womb/kernel/arch/
        x86_64/   (6 files)
            bootinfo.vuma            (BootInfo layout + bootinfo_init)
            trampoline.vuma          (write/read/exit/mmap/... externs —
                                      HOSTED-MODE ONLY; aarch64/riscv64
                                      do not have this file today)
            mm_trampoline.vuma       (pte_read/pte_write/tlb_flush/invlpg/cr3_*)
            trap_trampoline.vuma     (TrapFrame layout + idt_load/irq_*/pic_eoi/cr2_read)
            switch.vuma              (Task saved-reg subset + context_switch/cr3_write)
            pt.vuma                  (PTE bit layout + pte_make/pte_addr/pte_* helpers)
        aarch64/  (4 files — NO bootinfo.vuma, NO trampoline.vuma)
            mm_trampoline.vuma       (TTBR0/1 + TLBI aliases)
            trap_trampoline.vuma     (ESR/FAR-based TrapFrame + eret)
            switch.vuma              (x19-x30 + sp callee-saved set)
            pt.vuma                  (4 KB-granule, 48-bit VA, 3-level paging)
        riscv64/  (4 files — NO bootinfo.vuma, NO trampoline.vuma)
            mm_trampoline.vuma       (satp + sfence.vma aliases)
            trap_trampoline.vuma     (scause/stval-based TrapFrame + sret)
            switch.vuma              (s0/s1/x8-x23/x30 callee-saved set)
            pt.vuma                  (Sv39 / Sv48 paging)
```

The arch layer is the **only** place that mentions CPU registers, MMU
intrinsics, or trap-frame geometry. Above the arch layer, everything talks
about abstract `State<TrapFrame>`, `State<PageTable>`, `State<Task>`, etc.
— the same code in `womb/kernel/trap/trap.vuma`,
`womb/kernel/proc/scheduler.vuma`, `womb/kernel/syscall/abi.vuma` works on
all three arches (modulo per-arch field re-ordering of `TrapFrame`).

The abstraction boundary is enforced by the **byte-identical re-declaration
invariant** (K2c, extended to layouts in K3d): every consumer of a layout must
re-declare it byte-identically to the canonical source. So
`trap.vuma`'s `TrapFrame` is byte-identical to
`arch/x86_64/trap_trampoline.vuma`'s `TrapFrame`; `syscall/abi.vuma`'s
`TrapFrame` is byte-identical again; etc. The verifiers catch any drift at
compile time (the `LayoutRegistry` rejects conflicting field offsets).

Per-arch `pt.vuma` exposes a uniform API:

```
    fn pte_make(paddr: u64, flags: u64) -> u64;
    fn pte_addr(pte: u64) -> u64;
    fn pte_present(pte: u64) -> u8;   // x86_64 — "valid" on aarch64/riscv64
    fn pte_writable(pte: u64) -> u8;  // x86_64 + riscv64
    fn pte_user(pte: u64) -> u8;
    fn pte_no_exec(pte: u64) -> u8;   // x86_64 + aarch64
```

(Note: the field-name vocabulary differs slightly per arch — aarch64 exposes
`pte_valid` / `pte_accessible` / `pte_no_exec` / `pte_user`; riscv64 exposes
`pte_valid` / `pte_readable` / `pte_writable` / `pte_executable` / `pte_user`.
The arch-agnostic `vmm.vuma` only calls `pte_present` (x86_64) — on aarch64/
riscv64 ports the helper is renamed to `pte_valid` in the local re-declaration.
The K2d worklog records this naming asymmetry as a known issue.)

The arch-agnostic `vmm.vuma` walks page tables by calling these helpers +
`pte_read`/`pte_write` from `mm_trampoline.vuma`. Adding a fourth arch (e.g.
loongarch64) is a matter of writing four files (mm_trampoline,
trap_trampoline, switch, pt) — the rest of the kernel is unchanged. For a
hosted port you would also add `bootinfo.vuma` and `trampoline.vuma`; for a
bare-metal port you'd add `boot.S` instead. See
[`kernel-porting-guide.md`](./kernel-porting-guide.md) for the step-by-step.

---

## 6. FFI Trampoline Patterns

The kernel's FFI surface is concentrated in `womb/kernel/arch/x86_64/trampoline.vuma`
(hosted-mode syscalls) and the per-arch `*_trampoline.vuma` siblings. Every
extern is declared as `extern "C" { fn name(args) -> ret; }`. The codegen
lowers each call site per the target ABI (SysV AMD64 on x86_64, AAPCS on
aarch64, RV64GC on riscv64).

There are three idioms:

### 6.1 Pre-registered syscall stubs (hosted mode)

```
    extern "C" {
        fn write(fd: i64, buf: Address, count: i64) -> i64;
        fn exit(code: i64);
        fn mmap(addr: Address, length: u64, prot: i32, flags: i32,
                fd: i32, offset: i64) -> Address;
    }
```

These link against 3-instruction stubs in
`src/codegen/src/x86_64/mod.rs::build_runtime_syscall_stubs`:
`mov eax, #nr ; syscall ; ret` (or with a `mov r10, rcx` shuffle for ≥4 args).
The stubs are documented in `trampoline.vuma`'s header (e.g. write=nr 1,
exit=nr 60, mmap=nr 9 — these are the x86_64-native Linux syscall numbers,
NOT the asm-generic numbers; see §15 below). They run on the host Linux kernel.

### 6.2 Unregistered externs (resolve to `__ffi_fallback_stub`)

```
    extern "C" {
        fn halt();
        fn wfi();
        fn aesni_encrypt_block(key: Address, in: Address, out: Address);
    }
```

These have no pre-registered stub. The linker resolves them to
`__ffi_fallback_stub` (xor eax, eax; ret — returns 0, void calls no-op).
K12c's `pm.vuma` `halt`/`wfi` and K10d's `hw_trampoline.vuma` AES-NI externs
both use this pattern. K11+ will swap them for real asm stubs registered in
the backend's `build_runtime_syscall_stubs`.

### 6.3 State<T> as Address cast (FFI hand-off)

```
    extern "C" {
        fn write(fd: i64, buf: Address, count: i64) -> i64;
    }
    layout Console = { buf: [u8; 256], len: u32 }
    fn console_flush(c: State<Console>) {
        let base = c as Address;
        let _n = write(1, base, c.len as i64);
        c.len = 0;
    }
```

The `State<T> as Address` cast (`docs/architecture.md` §14.5) hands the state
buffer's base address to an FFI callee that expects a raw pointer. The
`buf` field is at offset 0 by convention so the cast yields `&buf[0]`.
This pattern is uniform across `console.vuma` (K1a), `kmsg.vuma` (K12b),
`panic.vuma` (K12a), and `hw_trampoline.vuma` (K10d) — every byte of kernel
I/O traverses this one cast.

### 6.4 `#[borrow]` on State<T> extern params

```
    extern "C" {
        #[borrow] fn pte_read(pt: State<PageTable>, level: u8, idx: u32) -> u64;
        #[borrow] fn context_switch(prev: State<Task>, next: State<Task>);
    }
```

Without `#[borrow]`, the marshal module defaults to `Invalidate` mode — the
state is marked consumed after the call, and the next `state.field` access
trips the use-after-invalidate verifier. `#[borrow]` tells the marshal to
pass the buffer's base address AND keep the state alive. This is load-bearing
for `pte_read`/`pte_write` (called in a loop by `vmm_walk_idx`) and
`context_switch` (the scheduler reads `prev.vruntime` after the switch).

---

## 7. IVE Guarantees for Kernel State

The kernel relies on three compile-time IVE verifiers (`docs/architecture.md`
§4). Every `.vuma` file in `womb/kernel/` is verified with `--verify` in CI:

- **`StateReadVerifier`** — every `state.field` read is in-bounds against the
  registered layout. Catches: typos like `state.lennth` (no such field),
  reading a `u64` field as `u8` (size mismatch), reading past the end of a
  `[u8; N]` array.
- **`StateWriteVerifier`** — every `state.field = value` is in-bounds AND
  respects linear ownership (no double-write-after-invalidate, no write to a
  consumed state).
- **`StateTransformVerifier`** — every `state1 as State<T2>` (or `state1 as
  Address` followed by an FFI write-back) is layout-compatible. Catches
  aliasing mismatches when two states claim the same buffer.

The arena runtime check (§4) is the only *runtime* safety net; it catches
out-of-arena overflow that the compile-time verifiers can't predict (because
allocation counts depend on the program's dynamic control flow).

Combined, these give the kernel the property that **no kernel module can
corrupt another kernel module's state by construction**. There is no pointer
arithmetic to misuse, no `free`-then-use, no `double-free`, no buffer overrun
past a layout's bounds. The only failure modes left are logic bugs (wrong
field value, wrong syscall return code, wrong scheduling decision) and
resource exhaustion (arena overflow → trap, slot pool exhausted → return
error).

The K0–K12 wave history records the IVE pass count for every committed file:
`IVE: Pass passed=1 failed=0 total=1` is the canonical green signal. Any
file that emits `WARNING: unsupported FieldAccess (not state-typed)` from
`flatten_expr` is non-compliant and was fixed before commit (e.g. K3e's
`syscall_args_from_frame` was converted from return-style to init-style to
eliminate four such warnings).

### Identifying IVE failures

When `compile_dump ... --verify` reports `IVE: Fail`, the next line names the
verifier that tripped:

```
    IVE: Fail passed=0 failed=1 total=1
      StateWriteVerifier: write to invalidated State<Console> in console_flush
        at line 92: c.len = 0;
        prev invalidate: extern call to write() at line 91
```

The diagnostic names the state, the failing line, and the prior invalidate
site. The fix is almost always one of: (a) add `#[borrow]` to the offending
extern, (b) flip a return-style helper to init-style, or (c) split the
function so the extern call is in a different function than the post-call
field access. See `kernel-developer-guide.md` §6 for the full debugging
recipe.

---

## 8. Complete File Inventory

The kernel source tree under `womb/kernel/` contains 75 `.vuma` files. The
companion documentation file `womb/syscalls.vuma` (one directory up) is a
syscall-number reference, not kernel code — it is included in the table
below for completeness but is not part of the 75-file kernel build.

LOC counts are the line counts from `wc -l` against each file (header
comments + blank lines + code). The "Wave" column gives the K-wave that
introduced the file; sub-waves are shown parenthetically.

### 8.1 Per-arch layer (`womb/kernel/arch/<arch>/`)

| Path                              | LOC | Wave  | Purpose                                                       |
|-----------------------------------|-----|-------|---------------------------------------------------------------|
| `arch/x86_64/bootinfo.vuma`       |  74 | K1a   | BootInfo layout + bootinfo_init (hosted argc/argv/mem_size)   |
| `arch/x86_64/trampoline.vuma`     | 101 | K1a   | hosted externs: write/read/exit/mmap/munmap/__vuma_argc/argv  |
| `arch/x86_64/mm_trampoline.vuma`  | 146 | K2c   | pte_read/pte_write/tlb_flush/invlpg/cr3_read/cr3_write        |
| `arch/x86_64/trap_trampoline.vuma`| 262 | K3a   | 22-field TrapFrame + idt_load/irq_mask/irq_unmask/pic_eoi/cr2_read |
| `arch/x86_64/switch.vuma`         | 308 | K4c   | 17-field Task + #[borrow] context_switch + cr3_write          |
| `arch/x86_64/pt.vuma`             | 190 | K2c   | PTE bit layout + pte_make/pte_addr/pte_present/writable/user/no_exec |
| `arch/aarch64/mm_trampoline.vuma` | 237 | K2d   | TTBR0/1 + TLBI aliases                                        |
| `arch/aarch64/trap_trampoline.vuma`|412 | K3a   | 35-field TrapFrame (x0-x30 + sp_el0/elr_el1/spsr_el1/esr_el1) |
| `arch/aarch64/switch.vuma`        | 336 | K4c   | Task with x19-x29 callee-saved + ttbr0_write                 |
| `arch/aarch64/pt.vuma`            | 260 | K2d   | ARMv8 PTE: pte_valid/accessible/no_exec/user/make             |
| `arch/riscv64/mm_trampoline.vuma` | 283 | K2d   | satp + sfence.vma aliases                                     |
| `arch/riscv64/trap_trampoline.vuma`|562 | K3a   | 35-field TrapFrame (ra/sp/gp/tp/t0-t6/s0-s11/a0-a7 + mepc/mstatus/scause/stval) |
| `arch/riscv64/switch.vuma`        | 344 | K4c   | Task with s0-s11 callee-saved + satp_write                   |
| `arch/riscv64/pt.vuma`            | 310 | K2d   | Sv39 PTE: pte_valid/readable/writable/executable/user/make    |

Subtotal: **14 files, 3,825 LOC.**

### 8.2 Console + kernel entry (`womb/kernel/`)

| Path                       | LOC | Wave | Purpose                                                |
|----------------------------|-----|------|--------------------------------------------------------|
| `console.vuma`             | 117 | K1a  | Console layout + console_init/putc/flush (hosted mode) |
| `kernel.vuma`              | 154 | K1d  | ELF entry — main() → kmain(); inlines console.vuma     |

Subtotal: **2 files, 271 LOC.**

### 8.3 Memory management (`womb/kernel/mm/`)

| Path             | LOC | Wave  | Purpose                                                      |
|------------------|-----|-------|--------------------------------------------------------------|
| `pmm.vuma`       | 557 | K2a   | buddy page-frame allocator (orders 0..10, FlatPool 256 slots)|
| `vmm.vuma`       | 432 | K2b   | page-table walk: vmm_map/vmm_unmap/vmm_translate + walk_idx  |
| `kmalloc.vuma`   | 622 | K2f   | slab allocator (kmalloc/kfree + 4-size cache)                |
| `mmap.vuma`      | 502 | K4g   | sys_mmap/sys_munmap + VMA tracking                           |

Subtotal: **4 files, 2,113 LOC.**

### 8.4 Trap + IRQ (`womb/kernel/trap/`)

| Path           | LOC | Wave | Purpose                                              |
|----------------|-----|------|------------------------------------------------------|
| `trap.vuma`    | 329 | K3d  | vector dispatcher: trap_handler + trap_panic/syscall/irq stubs |
| `irq.vuma`     | 333 | K3d  | 256-entry IRQ handler table + irq_register/dispatch |

Subtotal: **2 files, 662 LOC.**

### 8.5 Process / scheduler (`womb/kernel/proc/`)

| Path              | LOC | Wave | Purpose                                                |
|-------------------|-----|------|--------------------------------------------------------|
| `task.vuma`       | 603 | K4a  | Task control block + ProcessTable (256-slot parallel arrays) |
| `scheduler.vuma`  | 510 | K4b  | CFS-like runqueue + sched_enqueue/dequeue/pick         |
| `fork.vuma`       | 549 | K4d  | sys_fork — clone ProcessTable slot + copy mm_root     |
| `exec.vuma`       | 474 | K4d  | sys_exec — stub: overwrites task mm_root with 0xDEAD  |
| `wait.vuma`       | 826 | K4e  | sys_waitpid + wait_reap_zombie + wait_has_children     |
| `exit.vuma`       | 500 | K4e  | sys_exit — ZOMBIE state + ZombieTask reap              |

Subtotal: **6 files, 3,462 LOC.**

### 8.6 VFS (`womb/kernel/vfs/`)

| Path              | LOC | Wave  | Purpose                                              |
|-------------------|-----|-------|------------------------------------------------------|
| `inode.vuma`      | 677 | K5a#1 | 64-slot InodeTable + inode_alloc/free + pack/unpack  |
| `dentry.vuma`     | 616 | K5a#2 | 64-slot DentryTable + dentry_alloc/link/lookup       |
| `file.vuma`       | 599 | K5c   | FileTable + file_open/close/read/write               |
| `namei.vuma`      | 659 | K5b   | pathname resolution: namei_walk + component parsing  |
| `mount.vuma`      | 546 | K5b   | MountTable + mount/umount + per-fs dispatch table    |
| `file_ops.vuma`   | 496 | K5c   | sys_open/close/read/write/lseek dispatch to VFS      |

Subtotal: **6 files, 3,593 LOC.**

### 8.7 Filesystems (`womb/kernel/fs/`)

| Path              | LOC | Wave | Purpose                                              |
|-------------------|-----|------|------------------------------------------------------|
| `tmpfs.vuma`      |1264 | K5d  | RAM-backed fs: TmpfsData (64×256B pages) + tmpfs_* ops |
| `initramfs.vuma`  | 800 | K5e  | cpio parser + initramfs_fill_super                   |

Subtotal: **2 files, 2,064 LOC.**

### 8.8 IPC (`womb/kernel/ipc/`)

| Path           | LOC | Wave | Purpose                                              |
|----------------|-----|------|------------------------------------------------------|
| `pipe.vuma`    | 732 | K7a  | 256-byte ring buffer + pipe_read/write + sys_pipe    |
| `waitq.vuma`   | 478 | K7e  | WaitQueue + waitq_add/remove/wake_one/wake_all       |
| `shm.vuma`     |1062 | K7c  | 64-segment ShmTable + sys_shmget/shmat/shmdt         |
| `futex.vuma`   | 901 | K7c  | 64-slot FutexTable + sys_futex (WAIT/WAKE)           |
| `signal.vuma`  | 642 | K7b  | SignalTable + signal_send/deliver + sys_kill         |

Subtotal: **5 files, 3,815 LOC.**

### 8.9 Sync primitives (`womb/kernel/sync/`)

| Path             | LOC | Wave | Purpose                                              |
|------------------|-----|------|------------------------------------------------------|
| `spinlock.vuma`  | 583 | K8a  | SpinLock + spin_lock/unlock (atomic CAS loop)        |
| `mutex.vuma`     | 565 | K8b  | Mutex + mutex_lock/unlock (sleep contended)          |
| `semaphore.vuma` | 582 | K8c  | Semaphore + sem_wait/post                            |
| `rwlock.vuma`    | 858 | K8d  | RWLock + read_lock/read_unlock/write_lock/write_unlock |

Subtotal: **4 files, 2,588 LOC.**

### 8.10 SMP (`womb/kernel/smp/`)

| Path           | LOC | Wave | Purpose                                              |
|----------------|-----|------|------------------------------------------------------|
| `smp.vuma`     | 647 | K8e  | SmpState + smp_init/boot_cpu/call_function           |
| `percpu.vuma`  | 568 | K8f  | per-CPU data areas + percpu_get/set                  |
| `ipi.vuma`     | 515 | K8g  | IpiTable + ipi_send/broadcast/dispatch (LAPIC stub)  |

Subtotal: **3 files, 1,730 LOC.**

### 8.11 Networking (`womb/kernel/net/`)

| Path           | LOC | Wave | Purpose                                              |
|----------------|-----|------|------------------------------------------------------|
| `socket.vuma`  | 905 | K9a  | 64-slot SocketTable + sys_socket/bind/listen/accept/send/recv |
| `sk_buff.vuma` |  80 | K9b  | sk_buff layout + alloc/free (sentinel free-list)     |
| `tcp.vuma`     | 356 | K9c  | TCP state machine (10 states) + tcp_connect/send/recv |
| `dns.vuma`     | 334 | K9d  | DNS header + label parser + dns_query                |
| `http.vuma`    | 323 | K9e  | HTTP request parser + http_get                       |

Subtotal: **5 files, 1,998 LOC.**

### 8.12 Drivers (`womb/kernel/drivers/`)

| Path              | LOC | Wave  | Purpose                                              |
|-------------------|-----|-------|------------------------------------------------------|
| `uart.vuma`       | 465 | K6c   | 8250 (x86_64) + PL011 (aarch64) UART driver          |
| `char.vuma`       | 728 | K6c   | character-device framework + cdev_register           |
| `virtio_net.vuma` |  65 | K6d   | virtio-net PCI device skeleton (MMIO + IRQ)          |

Subtotal: **3 files, 1,258 LOC.**

### 8.13 TTY (`womb/kernel/tty/`)

| Path                | LOC | Wave | Purpose                                              |
|---------------------|-----|------|------------------------------------------------------|
| `console.vuma`      | 502 | K6e  | rich console: VGA framebuffer + escape sequences    |
| `line_discipline.vuma` | 441 | K6e | N_TTY line discipline + cook raw input               |
| `vt100.vuma`        | 573 | K6e  | VT100 terminal emulator (cursor, scroll, attrs)     |

Subtotal: **3 files, 1,516 LOC.**

### 8.14 Crypto (`womb/kernel/crypto/`)

| Path                | LOC | Wave  | Purpose                                              |
|---------------------|-----|-------|------------------------------------------------------|
| `api.vuma`          | 553 | K10a  | CipherCtx/HashCtx + cipher_encrypt/decrypt + hash_update/final stubs |
| `aes.vuma`          | 190 | K10b  | AES-128/192/256 key schedule + round function (skeleton) |
| `sha.vuma`          | 171 | K10c  | SHA-256 compression (skeleton)                       |
| `asym.vuma`         | 252 | K10e  | Ed25519/RSA skeleton                                 |
| `hw_trampoline.vuma`| 271 | K10d  | aesni_encrypt_block + shani_* + hw detection stubs   |

Subtotal: **5 files, 1,437 LOC.**

### 8.15 Syscall layer (`womb/kernel/syscall/`)

| Path                       | LOC | Wave | Purpose                                              |
|----------------------------|-----|------|------------------------------------------------------|
| `abi.vuma`                 | 260 | K3e  | SyscallArgs + syscall_args_from_frame + syscall_write_ret |
| `table.vuma`               | 255 | K3e  | 512-entry SyscallTable + pack/unpack helpers         |
| `dispatch.vuma`            | 297 | K3e  | syscall_dispatch + bounds check + registered-handler stub |
| `handlers/io.vuma`         | 222 | K3f  | sys_write/sys_read (3-arg, routes by fd)             |
| `handlers/mm.vuma`         | 228 | K3f  | sys_brk/sys_mmap/sys_munmap                          |
| `handlers/proc.vuma`       | 168 | K3f  | sys_getpid/sys_exit                                  |

Subtotal: **6 files, 1,430 LOC.**

### 8.16 Panic / power / shell / hosted

| Path                  | LOC | Wave | Purpose                                              |
|-----------------------|-----|------|------------------------------------------------------|
| `panic/panic.vuma`    | 266 | K12a | panic(msg) + assert(cond, msg)                       |
| `panic/kmsg.vuma`     | 348 | K12b | 256-byte ring buffer + kmsg_write                    |
| `power/pm.vuma`       | 354 | K12c | PmState + pm_cpu_idle + pm_suspend (level 0..3)      |
| `shell/shell.vuma`    | 584 | K6f  | shell prompt + cmd dispatch (echo/ls/cat/exit)       |
| `hosted/host.vuma`    | 156 | K1a  | host_* wrappers around trampoline.vuma externs       |

Subtotal: **5 files, 1,708 LOC.**

### 8.17 Documentation reference (not part of the kernel build)

| Path                       | LOC | Wave | Purpose                                              |
|----------------------------|-----|------|------------------------------------------------------|
| `womb/syscalls.vuma`       | ~200 | K1a  | asm-generic syscall-number reference (DOC-ONLY)     |

### 8.18 Grand totals

| Layer                      | Files | LOC     |
|----------------------------|-------|---------|
| Per-arch (3 arches)        |    14 |   3,825 |
| Console + kernel entry     |     2 |     271 |
| mm                         |     4 |   2,113 |
| trap                       |     2 |     662 |
| proc                       |     6 |   3,462 |
| vfs                        |     6 |   3,593 |
| fs                         |     2 |   2,064 |
| ipc                        |     5 |   3,815 |
| sync                       |     4 |   2,588 |
| smp                        |     3 |   1,730 |
| net                        |     5 |   1,998 |
| drivers                    |     3 |   1,258 |
| tty                        |     3 |   1,516 |
| crypto                     |     5 |   1,437 |
| syscall                    |     6 |   1,430 |
| panic/power/shell/hosted   |     5 |   1,708 |
| **Kernel total**           | **75**| **33,470** |
| Documentation reference    |     1 |    ~200 |

The "76+ source files" cited in some audit summaries counts the documentation
reference (`womb/syscalls.vuma`); the strictly-kernel source tree is 75 files.

---

## 9. Stub Inventory

A "stub" is a function whose body is a deliberate no-op (or trivial
placeholder) and whose header comment explicitly defers the real implementation
to a future wave (typically K11+ for hardware-touching paths, K13+ for crypto
algorithm bodies). Every stub is documented in its file header with (a) what
the real implementation will do, (b) which wave will replace it, and (c) why
the stub is safe for hosted-mode testing. The DoD explicitly allows stubs as
long as they are documented.

### 9.1 Trap dispatcher stubs (`womb/kernel/trap/trap.vuma`)

| Function       | Line | Stub behavior            | K11+ replacement                                |
|----------------|------|--------------------------|--------------------------------------------------|
| `trap_panic`   |  262 | `return;` (no-op)        | K12a calls `panic(tf)` — print TrapFrame + halt  |
| `trap_syscall` |  273 | `return;` (no-op)        | K3e/K11 reads tf.rax, indexes SyscallTable, calls handler, writes ret to tf.rax |
| `trap_irq`     |  285 | `return;` (no-op)        | K11 subtracts 32 from tf.vector, calls `irq_dispatch(tbl, irq)`, sends `pic_eoi(irq)` |

### 9.2 Syscall dispatch stub (`womb/kernel/syscall/dispatch.vuma`)

| Function           | Line | Stub behavior            | K11+ replacement                                |
|--------------------|------|--------------------------|--------------------------------------------------|
| `syscall_dispatch` |  239 | bounds-check + `return 0` (call_indirect missing) | K11 adds `call_indirect(handler, args)` intrinsic (call rax / blr x8 / jalr x0,x7) |

The `SyscallTable` itself is NOT a stub — `syscall_table_get`/`set` round-trip
correctly (verified by the dispatch.vuma self-test). Only the indirect-call
step is stubbed; registration works today.

### 9.3 Process lifecycle stubs (`womb/kernel/proc/`)

| Function       | File           | Line | Stub behavior            | K11+ replacement                                |
|----------------|----------------|------|--------------------------|--------------------------------------------------|
| `sys_exec`     | `exec.vuma`    |  377 | overwrites task.mm_root with 57005 (0xDEAD) | K11+ runs ELF loader, sets mm_root to new PGD phys, sets saved rip/rsp via task_init_for_switch |
| `sys_waitpid`  | `wait.vuma`    |  686 | scans ProcessTable for any ZOMBIE child of parent_pid=1 | K11+ dispatches on pid > 0 vs -1, sleeps on WaitQueue for EAGAIN case |

Note: `sys_waitpid` is *partially* implemented (it correctly reaps zombies and
returns the right errno codes); only the "wait for any specific pid" branch and
the sleep-on-WaitQueue behavior are stubbed.

### 9.4 Crypto API stubs (`womb/kernel/crypto/api.vuma`)

| Function         | Line | Stub behavior              | K10/K13+ replacement                          |
|------------------|------|----------------------------|------------------------------------------------|
| `cipher_encrypt` |  330 | byte-wise copy input→output | K10b: AES-128/256 round-function loop; K10d: ChaCha20 20-round QR loop; K10e: AES-NI dispatch |
| `cipher_decrypt` |  357 | delegates to `cipher_encrypt` | K10b: AES-CBC inverts round-key schedule; K10d: ChaCha20 same codepath (XOR keystream symmetric) |
| `hash_update`    |  461 | bumps `ctx.total += len` only | K10c: SHA-256 compression — buffer 64-byte blocks, run 64-round compression loop |
| `hash_final`     |  479 | writes 32 zero bytes to out  | K10c: append 0x80 + zero-pad to 56 mod 64, append 64-bit BE bit-count, run final compression, emit 8 LE u32 words |

The crypto API surface (`CipherCtx`, `HashCtx`, `cipher_init`, `cipher_set_key`,
`cipher_set_iv`, `hash_init`) is fully implemented — only the algorithm bodies
are stubs. The stubs preserve the streaming-API contract so callers can swap
K10a's stub for K10b's real AES by replacing just the body of `cipher_encrypt`.

### 9.5 Power management stubs (`womb/kernel/power/pm.vuma`)

| Function       | Line | Stub behavior            | K11+ replacement                                |
|----------------|------|--------------------------|--------------------------------------------------|
| `pm_cpu_idle`  |  266 | `return;` (no-op)        | K11: `hlt` (x86_64) / `wfi` (aarch64/riscv64)    |
| `pm_suspend`   |  299 | returns 0 (success)      | K11: ACPI S-state (x86_64) / PSCI (aarch64) / SBI suspend (riscv64) |

### 9.6 SMP / IPI stubs (`womb/kernel/smp/`)

| Function              | File        | Line | Stub behavior            | K11+ replacement                                |
|-----------------------|-------------|------|--------------------------|--------------------------------------------------|
| `smp_boot_cpu`        | `smp.vuma`  |  464 | returns 0 (success)      | K11: LAPIC INIT-SIPI-SIPI sequence (x86_64) / PSCI CPU_ON (aarch64) / SBI hart_start (riscv64) |
| `smp_call_function`   | `smp.vuma`  |  548 | `return;` (no-op)        | K11: enqueue IPI to all other CPUs, wait for ack |
| `ipi_send`            | `ipi.vuma`  |  407 | calls `lapic_write` (stub) | K11: real LAPIC ICR MMIO write at offset 768    |
| `ipi_broadcast`       | `ipi.vuma`  |  431 | calls `lapic_write` (stub) | K11: real LAPIC ICR write with shorthand bits  |
| `ipi_dispatch`        | `ipi.vuma`  |  463 | looks up handler, `return;` | K11: real `call_indirect(handler)` invocation |

### 9.7 Networking stubs (`womb/kernel/net/`)

| Function               | File           | Line | Stub behavior            | K11+ replacement                                |
|------------------------|----------------|------|--------------------------|--------------------------------------------------|
| `tcp_connect`/`send`/`recv` | `tcp.vuma` | 284+ | returns 107 (-ENOTCONN) if state != ESTABLISHED; otherwise copies bytes to/from sk_buff | K11+: real TCP state machine + segment TX/RX via virtio-net |

### 9.8 Hardware externs (resolve to `__ffi_fallback_stub`)

Every extern that has no pre-registered syscall stub resolves to the
`__ffi_fallback_stub` symbol (xor eax, eax; ret — returns 0, void calls
no-op). These are not "stubs" in the function-body sense; they are
**unregistered externs** that the linker resolves to a no-op:

| Extern                    | Declared in                          | Effect (hosted) | K11+ replacement                       |
|---------------------------|--------------------------------------|-----------------|----------------------------------------|
| `halt`, `wfi`             | `power/pm.vuma`                      | no-op           | `hlt` / `wfi` asm stub                 |
| `cr3_read`/`cr3_write`    | `arch/x86_64/mm_trampoline.vuma`     | returns 0       | `mov rax, cr3; ret` / `mov cr3, rdi; ret` |
| `ttbr0_read`/`ttbr0_write`| `arch/aarch64/mm_trampoline.vuma`    | returns 0       | `mrs`/`msr` TTBR0_EL1                  |
| `satp_read`/`satp_write`  | `arch/riscv64/mm_trampoline.vuma`    | returns 0       | `csrr`/`csrw satp`                     |
| `pte_read`/`pte_write`    | all `mm_trampoline.vuma`             | returns 0       | MMIO at PTE's physical address         |
| `tlb_flush`/`invlpg`      | all `mm_trampoline.vuma`             | no-op           | `invlpg` / `tlbi vmalle1` / `sfence.vma` |
| `idt_load`/`irq_mask`/`irq_unmask`/`pic_eoi`/`cr2_read` | `arch/x86_64/trap_trampoline.vuma` | no-op / 0 | `lidt` / PIC MMIO / `mov rax, cr2` |
| `aesni_encrypt_block`/`aesni_available`/`shani_*` | `crypto/hw_trampoline.vuma` | returns 0 | AES-NI / SHA-NI intrinsics |
| `mmio_read8`/`mmio_write8`/`mmio_read32`/`mmio_write32` | `drivers/uart.vuma` | returns 0 | `mov al, [rdi]; ret` etc. |
| `lapic_write`             | `smp/ipi.vuma`                       | no-op           | LAPIC MMIO write                       |
| `context_switch`          | all `switch.vuma`                    | no-op           | save/restore callee-saved regs + cr3_write/ttbr0_write/satp_write |

The hosted-mode kernel never actually exercises these — every code path that
would call them either (a) has an `if` short-circuit routing to the `write`/
`read`/`exit` host syscalls instead, or (b) is a self-test that just checks
"did we crash?". The `__ffi_fallback_stub` resolution is what makes the
hosted-mode kernel bootable today.

---

## 10. VUMA Parser Limitations

The VUMA 2.0 parser (`src/parser/src/parser.rs`) has several known
limitations that shape kernel code style. Each is documented in the file
header of the kernel module that works around it; this section consolidates
them.

### 10.1 The `0 - 1` negative-literal workaround

VUMA's integer-literal path goes through `parse_int_radix`, which on a
negative literal like `-1` interprets it as a signed i64 and then sign-extends
to u64 — producing `0xFFFFFFFFFFFFFFFF` correctly in most cases but
occasionally tripping a width-extension subtlety that produces surprising
values. The kernel's convention (established in K4e's `wait.vuma` and
repeated in K7a-K7e and K9) is to **always write negative numbers as
`0 - N`** (e.g. `0 - 1` for -1, `0 - 11` for -EAGAIN, `0 - 38` for -ENOSYS)
rather than as the literal `-1` / `-11` / `-38`:

```
    // DON'T:
    return -1;          // parser's signed-literal path — risky

    // DO:
    return 0 - 1;       // flatten_expr's BinOp::Sub arm — verified safe
```

The `0 - N` form lowers to the same machine code as `-N` (the codegen's
constant folder collapses it), so there is no perf cost. The safety gain is
that the expression goes through `flatten_expr`'s `BinOp::Sub` arm (which
handles the type promotions correctly) rather than the lexer's
negative-number path. The K4e worklog records the original regression: a
literal `18446744073709551615` (intended as u64 -1) was misinterpreted as a
signed value and sign-extended incorrectly; switching to `0 - 1` fixed it.

This convention is enforced by the K3d / K3e / K4a-K4e / K7a-K7e / K9
contracts' "Use decimal constants" rule and is checked by code review (there
is no compiler warning for `-N` literals — they just occasionally produce
wrong values).

### 10.2 The no-`import` rule

VUMA 2.0 has no `import` statement (Open Work §7). Every kernel module that
wants to call another module's functions or use another module's layouts
must **re-declare them locally**, byte-identically. The
`byte-identical-redeclaration invariant` (K2c, extended to layouts in K3d) is
enforced by the `LayoutRegistry` — if two files declare the same layout name
with different field offsets/types/order, the verifiers catch the drift at
compile time.

```
    // womb/kernel/syscall/dispatch.vuma re-declares:
    layout SyscallArgs = { nr: u64, a0: u64, a1: u64, a2: u64,
                           a3: u64, a4: u64, a5: u64 }
    // byte-identical to womb/kernel/syscall/abi.vuma's SyscallArgs.
```

This is a maintainability hazard — a layout change has to be propagated to
every consumer by hand. K13+ will add a real `import` mechanism (likely
`import fs.inode;` bringing `InodeTable` + its helpers into scope); until
then, every kernel contributor does the copy-paste.

### 10.3 The State-return limitation

The codegen does not propagate `State<T>`-typedness through function return
values. A binding `let s = make_state()` where `make_state` returns
`State<T>` is NOT registered as state-typed in the caller; subsequent
`s.field` accesses silently return 0 with a
`WARNING: unsupported FieldAccess (not state-typed)` from `flatten_expr`.

The workaround is the **init-style API** (§3): the caller allocates the state
via `state_new(...)` (which marks the binding as state-typed) and passes it
by reference to a function that populates it in place. This is the canonical
kernel pattern; K11+ will fix the codegen to propagate State through
returns, at which point the init-style API can collapse to return-style.

### 10.4 The array-index-bytewise limitation

The codegen only lowers `state.array[idx]` correctly for `[u8; N]` arrays.
For `[u16; N]`, `[u32; N]`, or `[u64; N]` arrays, the indexed-access path
goes through a code path that the IVE verifiers don't fully understand —
accesses compile but read back wrong values. The kernel's convention is to
**store every "array of N u32/u64" as a parallel flat `[u8; N * width]`
array** with pack/unpack helpers:

```
    layout InodeTable = {
        ino:  [u8; 1024],   // 128 inodes × 8 bytes, packed LE
        mode: [u8; 128],    // 128 inodes × 1 byte
        size: [u8; 1024],   // 128 inodes × 8 bytes, packed LE
        ...
    }

    fn inode_get_ino(tbl: State<InodeTable>, idx: u32) -> u64 {
        let off = idx * 8;
        let v: u64 = 0;
        let i = 0;
        while i < 8 {
            let sh = i * 8;
            let b = tbl.ino[off + i] as u64;
            v = v + (b << sh);
            i = i + 1;
        }
        return v;
    }
```

The pack helper is an 8-iteration while loop (shift by `i*8`, mask 255 on
writes, sum-of-shifted-bytes on reads). The same pattern appears in
`pmm.vuma::pool_get_base` (K2a), `irq.vuma::irq_get_handler` (K3d),
`syscall/table.vuma::syscall_table_get` (K3e), `task.vuma::pt_get_vruntime`
(K4a), and every other kernel table module. K13+ will fix the codegen to
support `[u64; N]` directly.

### 10.5 The transform-1-param limitation

The `StateTransform` verifier requires that an `as Address` cast appears in a
context where the State<T> is the function's parameter — i.e. the transform
is on a state owned by the caller. Casting a state created inside the
function (`let s = state_new(...); let a = s as Address; ...`) sometimes
trips the verifier because the lifetime of `s` is local. The kernel's
convention is to **always pass states from the caller**: even a helper that
just needs a scratch buffer takes it as a parameter (the caller allocates
and owns it).

```
    // DON'T (transform-on-local-state — sometimes trips verifier):
    fn console_flush_local() {
        let c = state_new(Console);
        let base = c as Address;       // transform on local State — risky
        let _n = write(1, base, c.len as i64);
    }

    // DO (transform-on-parameter — always safe):
    fn console_flush(c: State<Console>) {
        let base = c as Address;       // transform on parameter — OK
        let _n = write(1, base, c.len as i64);
        c.len = 0;
    }
```

### 10.6 The hex-literal width-extension subtlety

VUMA accepts `0x..` hex literals but the parser's hex path shares code with
the decimal path through `parse_int_radix`, which has subtle width-extension
behavior at the 64-bit boundary. The kernel's convention is to **use decimal
literals in self-tests** (`4096`, not `0x1000`; `17592186028032`, not
`0x000FFFFFFFFFF000`). This is enforced by the K2c / K3d / K4a / K5a / K10a
contracts' "IMPORTANT: use decimal constants" rule. The decimal form lowers
to identical machine code; the safety gain is avoiding the hex path entirely.

### 10.7 The `no_struct_literal` trap

VUMA has no struct-literal syntax (`Layout { field: value, ... }`). State
must be allocated with `state_new(Layout)` (zero-initialized) and then
populated field-by-field. There is no way to "construct" a state inline:

```
    // DON'T (parse error — VUMA has no struct literal):
    fn make_task(pid: u32) -> State<Task> {
        return Task { pid: pid, state: 1, ... };
    }

    // DO (allocate-then-populate):
    fn make_task(tbl: State<ProcessTable>, pid: u32) {
        let idx = task_alloc(tbl);
        pt_set_pid(tbl, idx, pid);
        pt_set_state(tbl, idx, 1);
        ...
    }
```

This forces the init-style API pattern; it's not really a workaround so much
as a language-design choice that aligns with the PMT discipline (no implicit
allocation sites).

### 10.8 The forward-reference allowance

Unlike C, VUMA allows forward references to functions: a function can call
another function declared later in the file. The parser does a two-pass scan
(pass 1 collects all fn signatures into the symbol table, pass 2 resolves
call sites). This is verified by `syscall/dispatch.vuma`'s self-test, which
calls `syscall_table_set` before it is declared. Layouts, however, MUST be
declared before the first function that uses them — the layout registry is
single-pass.

---

## 11. Data Flow Diagrams

### 11.1 Boot sequence

```
                  ┌──────────────────────────────┐
                  │ L1: _start (hosted) or boot.S │
                  │ - reads argc/argv off stack   │
                  │ - stores into BSS slot        │
                  └────────────┬─────────────────┘
                               │ call main()
                               ▼
                  ┌──────────────────────────────┐
                  │ kernel.vuma::main()          │
                  │ - calls kmain()              │
                  │ - returns kmain()'s i32      │
                  └────────────┬─────────────────┘
                               │ call kmain()
                               ▼
                  ┌──────────────────────────────┐
                  │ kernel.vuma::kmain()         │
                  │ 1. c = console_init()        │
                  │    └─ state_new(Console)     │
                  │    └─ c.len = 0              │
                  │ 2. kmain_print_banner(c)     │
                  │    └─ for each byte:         │
                  │       console_putc(c, byte)  │
                  │ 3. (future: pmm_init(c)...)  │
                  │ 4. return 0                  │
                  └────────────┬─────────────────┘
                               │ return 0
                               ▼
                  ┌──────────────────────────────┐
                  │ _start: mov edi, eax         │
                  │         mov eax, 60 ; syscall│
                  │         (sys_exit, nr=60)    │
                  └──────────────────────────────┘
```

### 11.2 Syscall dispatch flow (K11+ target)

```
   user-space
        │
        │ mov rax, 64 ; syscall        (__NR_write = 1 native x86_64)
        ▼
   ┌─────────────────────────────────┐
   │ trap_entry asm (K11)            │
   │ - push 15 GP regs + vector + err│
   │ - call trap_handler(tf)         │
   └────────────┬────────────────────┘
                │ State<TrapFrame>
                ▼
   ┌─────────────────────────────────┐
   │ trap.vuma::trap_handler         │
   │ if tf.vector == 128:            │
   │   trap_syscall(tf)              │
   └────────────┬────────────────────┘
                │
                ▼
   ┌─────────────────────────────────┐
   │ trap.vuma::trap_syscall (K3e)   │
   │ - args = state_new(SyscallArgs) │
   │ - syscall_args_from_frame(tf, args) │
   │ - ret = syscall_dispatch(tbl, args) │
   │ - syscall_write_ret(tf, ret)    │
   └────────────┬────────────────────┘
                │
                ▼
   ┌─────────────────────────────────┐
   │ dispatch.vuma::syscall_dispatch │
   │ - bounds-check nr < 512         │
   │ - lookup handler via            │
   │   syscall_table_get(tbl, nr)    │
   │ - if handler == 0: return 38    │
   │ - call_indirect(handler, args)  │ ← K11 intrinsic (today: stub return 0)
   └────────────┬────────────────────┘
                │
                ▼
   ┌─────────────────────────────────┐
   │ handlers/io.vuma::sys_write     │
   │ - if fd == 1:                   │
   │   return write(1, buf as Addr, count) │
   │ - else: VFS write (K5)          │
   └────────────┬────────────────────┘
                │ ret = byte count or -errno
                ▼
   ┌─────────────────────────────────┐
   │ trap_exit asm (K11)             │
   │ - pop 15 GP regs                │
   │ - iretq (or sysretq)            │
   └─────────────────────────────────┘
```

### 11.3 Trap handling flow (vectors 0..255)

```
                  ┌──────────────────────┐
                  │ trap_entry asm (K11) │
                  │ saves TrapFrame      │
                  └──────────┬───────────┘
                             │
                             ▼
                  ┌──────────────────────┐
                  │ trap_handler(tf)     │
                  └──────────┬───────────┘
                             │
              ┌──────────────┼──────────────┐
              │              │              │
              ▼              ▼              ▼
        ┌─────────┐   ┌─────────┐   ┌──────────┐
        │ vec 0..31│   │ vec 128 │   │ vec 32..127│
        │ CPU exc  │   │ syscall │   │ + 129..255│
        └────┬────┘   └────┬────┘   │  hardware │
             │             │        │   IRQ     │
             ▼             ▼        └─────┬────┘
       ┌──────────┐  ┌──────────┐         │
       │trap_panic│  │trap_sysc │         ▼
       │  (stub)  │  │  (stub)  │   ┌──────────┐
       └──────────┘  └──────────┘   │ trap_irq │
                                     └────┬─────┘
                                          │
                                          ▼
                                   ┌──────────────┐
                                   │ irq_dispatch │
                                   │  (K3d stub)  │
                                   └──────┬───────┘
                                          │
                                          ▼
                                   ┌──────────────┐
                                   │ pic_eoi(irq) │
                                   │  (K11 asm)   │
                                   └──────────────┘
```

### 11.4 VFS path resolution flow (`namei.vuma`)

```
   user-space: open("/tmp/foo.txt", O_RDWR)
        │
        ▼
   ┌─────────────────────────────────────┐
   │ namei.vuma::namei_walk              │
   │ - start at root dentry (idx 0)      │
   │ - for each component "tmp", "foo.txt":│
   │   scan dentry.children for match    │
   └────────────────┬────────────────────┘
                    │ resolved dentry idx
                    ▼
   ┌─────────────────────────────────────┐
   │ inode.vuma::inode_lookup            │
   │ - read dentry.inode                 │
   │ - return State<Inode>               │
   └────────────────┬────────────────────┘
                    │
                    ▼
   ┌─────────────────────────────────────┐
   │ file.vuma::file_open                │
   │ - allocate fd in FileTable          │
   │ - fd.dentry = resolved dentry idx   │
   │ - fd.pos = 0                        │
   │ - fd.ops = tmpfs_file_ops (K5d)     │
   └────────────────┬────────────────────┘
                    │ fd
                    ▼
              return fd to user
```

### 11.5 Context switch flow

```
   ┌────────────────────────────────────┐
   │ scheduler.vuma::sched_pick         │
   │ - scan Runqueue for min vruntime   │
   │ - return next_task_idx             │
   └────────────────┬───────────────────┘
                    │
                    ▼
   ┌────────────────────────────────────┐
   │ scheduler.vuma::sched_switch       │
   │ - prev = current task              │
   │ - next = sched_pick() result       │
   │ - context_switch(prev, next)       │
   └────────────────┬───────────────────┘
                    │
                    ▼
   ┌────────────────────────────────────┐
   │ arch/<arch>/switch.vuma extern:    │
   │ #[borrow] context_switch(prev, next)│
   │                                    │
   │ K11 asm body:                      │
   │ - push callee-saved regs to        │
   │   prev.{rsp, rip, rbp, rbx, r12-r15}│
   │ - mov rsp, next.rsp                │
   │ - pop callee-saved regs from next  │
   │ - ret (pops next.saved_rip)        │
   │                                    │
   │ - cr3_write(next.mm_root) if mm switch│
   └────────────────┬───────────────────┘
                    │
                    ▼
   ┌────────────────────────────────────┐
   │ back in scheduler:                 │
   │ - prev.vruntime += tick            │
   │ - current = next                   │
   └────────────────────────────────────┘
```

---

## 12. Memory Layout

The arena's `___pmt_buffer` is laid out sequentially: a 32-byte header, then
each `state_new(...)` allocation in source-code order (or more precisely, in
the order the codegen emits Alloc nodes during the SCG→IR lowering pass).

```
   Address          Content                                   Size
   ──────────────── ───────────────────────────────────────── ────────
   arena_ptr + 0    base address (== arena_ptr itself)         8 B
   arena_ptr + 8    current bump offset                        8 B
   arena_ptr + 16   capacity  (BootInfo.mem_size = 16777216)   8 B
   arena_ptr + 24   overflow-handler fn ptr                    8 B
   ──────────────── ───────────────────────────────────────── ────────
   arena_ptr + 32   State<Console>          (console_init)     260 B
                    ├ buf: [u8; 256]                           256 B
                    └ len: u32                                  4 B
   ──────────────── ───────────────────────────────────────── ────────
   arena_ptr + 292  State<PmmState>         (pmm_init)        ~1 KB
                    ├ free_lists: [u8; 88]                     88 B
                    ├ pool_addr: u64                             8 B
                    ├ total_pages: u64                           8 B
                    └ free_count: [u8; 88]                     88 B
   ──────────────── ───────────────────────────────────────── ────────
   arena_ptr + ~1.3K State<FlatPool>        (pmm_init)        ~9 KB
                    ├ bases: [u8; 2048]    (256 × 8B)       2,048 B
                    ├ orders: [u8; 256]                       256 B
                    ├ nexts: [u8; 2048]                      2,048 B
                    └ free: [u8; 256]                        256 B
   ──────────────── ───────────────────────────────────────── ────────
   arena_ptr + ~10K State<ProcessTable>     (task_init)       ~9 KB
                    ├ pids: [u8; 1024]                        1 KB
                    ├ ppids: [u8; 1024]                       1 KB
                    ├ states: [u8; 256]                       256 B
                    ├ prios: [u8; 256]                        256 B
                    ├ vruntimes: [u8; 2048]                   2 KB
                    ├ mm_roots: [u8; 2048]                    2 KB
                    ├ fs_roots: [u8; 2048]                    2 KB
                    ├ fds: [u8; 2048]                         2 KB
                    └ nexts: [u8; 2048]                       2 KB
   ──────────────── ───────────────────────────────────────── ────────
   ...              (every subsequent state_new call)
   ──────────────── ───────────────────────────────────────── ────────
   bump offset      ▲ (grows toward capacity)
   ──────────────── ───────────────────────────────────────── ────────
                    unmapped / overflow zone
                    (triggers __arena_overflow on alloc past cap)
   ──────────────── ───────────────────────────────────────── ────────
   arena_ptr + cap  end of arena
```

The 16 MB hosted capacity is more than sufficient for the current kernel's
~80 KB of state allocations. The arena is **not** pre-partitioned per
subsystem — every `state_new(...)` call just bumps the offset. This means a
buggy subsystem that allocates in a loop can starve the rest of the kernel;
K11+ may add per-subsystem quotas (a capacity field per State-creation site).

### `___pmt_buffer` symbol resolution

The arena symbol `___pmt_buffer` is declared in
`src/codegen/src/runtime/arena.rs` and is registered as a BSS-style
zero-initialized region of `BootInfo.mem_size` bytes. On hosted x86_64 it's
a static `[u8; 16_777_216]` in the data segment (zero-filled by the loader);
on bare-metal K11 it's the physical memory region the boot protocol handed
to the kernel.

---

## 13. Error Code Convention

The kernel uses Linux asm-generic errno values, returned as **positive
integers** by most syscall handlers (the trap layer negates before writing
to `tf.rax`). The `0 - N` form is used for the actual negation in places
where the handler returns `i64` directly (see §10.1).

| Errno    | Value | Meaning                          | Used by                                   |
|----------|-------|----------------------------------|-------------------------------------------|
| `EAGAIN` | 11    | Resource temporarily unavailable | `pipe_read` (empty pipe), `pipe_write` (full pipe), `sys_waitpid` (children still running), `sys_futex` (FUTEX_WAIT would block), `signal_send` (sigmask full) |
| `ENOMEM` | 12    | Out of memory                    | `sys_futex` (FutexTable full — K7c converts the `return 64` sentinel to -ENOMEM in the syscall layer), `task_alloc` (ProcessTable full) |
| `ECHILD` | 10    | No child processes               | `sys_waitpid` (parent has no children)    |
| `EBADF`  | 9     | Bad file descriptor              | `sys_send`/`sys_recv`/`sys_listen`/`sys_accept` (fd unused or out of range), `file_ops` VFS path |
| `EINVAL` | 22    | Invalid argument                 | `signal_send` (sig == 0 or sig > 32), `sys_socket`/`sys_bind` (bad family/type), `sys_futex` (op != 0 and op != 1), `sys_listen` (state != 2) |
| `ENOSYS` | 38    | Function not implemented         | `syscall_dispatch` (nr >= 512 OR handler == 0) |
| `EPIPE`  | 32    | Broken pipe                      | `pipe_write` (pipe is closed — no readers) |
| `ENOTCONN`| 107  | Transport endpoint not connected | `tcp_send`/`tcp_recv` (state != ESTABLISHED) |
| `-1`     | -1    | Generic failure (via `0 - 1`)    | `sys_shmget` (table full), `sys_shmdt` (not found), `socket.vuma` (table full) |

The kernel does NOT use the full Linux errno table (~100 values); only the
subset above appears in source. Future waves (K13+) may extend this as more
syscall paths are implemented.

### Sign convention

There are two conventions in the codebase:

1. **Positive errno** (`return 11;`) — used by `dispatch.vuma`,
   `pipe.vuma`, `signal.vuma`, `socket.vuma`, `tcp.vuma`, `futex.vuma`. The
   trap layer (K3e) negates before writing to `tf.rax`.

2. **Negative errno via `0 - N`** (`return 0 - 11;`) — used by `wait.vuma`
   and `shm.vuma`, where the function signature returns `i64` and the caller
   expects a directly-usable negative value.

K11+ will pick one (likely: positive errno internally, trap layer negates
uniformly). The current mix is documented in each file header.

---

## 14. Sentinel Value Convention

The kernel uses **out-of-band sentinel values** to signal "empty", "full",
"end-of-list", or "not found" without incurring an extra error channel. The
sentinels are picked so they can never be a valid index/pointer.

| Sentinel | Value | Meaning                          | Used by                                   |
|----------|-------|----------------------------------|-------------------------------------------|
| `EMPTY`  | 256   | Empty queue / table-full         | `pmm.vuma::pmm_get_free_list` (free-list head == 256 = no free pages of that order), `waitq.vuma` (WQ empty), `pipe.vuma` (no waiter), `tmpfs.vuma` (lookup not-found — 256 = "no such dentry"), `task_alloc` returns 256 when ProcessTable is full |
| `FULL`   | 64    | Slot pool exhausted              | `tmpfs.vuma::tmpfs_data_alloc_page` (64 = all pages used), `futex.vuma::futex_find` (64 = table full), `shm.vuma::shm_find` (64 = table full), `inode_alloc`/`dentry_alloc`/`file_alloc` (64 = table full) |
| `EOL`    | 255   | End-of-list marker in u8 slots   | `sk_buff.vuma::free_list` (255 = no free sk_buff), `kmsg.vuma` ring wrap (`& 255` mask) |
| `FREE`   | 0     | Slot is free / unallocated       | `task.vuma::ProcessTable.states` (state == 0 = slot FREE), `irq.vuma::IrqTable.handlers` (handler addr == 0 = unregistered), `syscall/table.vuma::SyscallTable.handlers` (handler addr == 0 = unregistered), `vfs/inode.vuma::InodeTable.ino` (ino == 0 = invalid inode, matches Linux convention) |

### Why 256 works as "empty" for u8-indexed tables

Every table that uses 256 as the empty sentinel has exactly 256 slots
(indexed 0..255). The number 256 cannot appear as a valid slot index, so it
is unambiguous. The trade-off is that the table-size constant (256) appears
both as the array length AND as the sentinel value — a future maintainer
must keep them in sync. The K4a / K5a / K7e contracts document this
convention explicitly.

### Why 64 works as "full" for u8-indexed tables

Every table that uses 64 as the full sentinel has exactly 64 slots (indexed
0..63). The number 64 cannot appear as a valid slot index, so it is
unambiguous. Used by `tmpfs.vuma` (64 inodes, 64 dentries, 64 files, 64
pages), `futex.vuma` (64 futex slots), `shm.vuma` (64 segments),
`socket.vuma` (64 sockets). The same trade-off applies: the table-size
constant must stay 64.

### Why 255 works as "end-of-list" for u8 free-lists

For tables with fewer than 256 slots but where the free-list head is stored
as a u8 (so the table-size + sentinel pair doesn't fit the 256 pattern
above), the kernel uses 255 as the "empty" sentinel. This works for any
table of size ≤ 255 (sk_buff's 64-slot free-list, kmsg's ring buffer
indices). The `& 255` wraparound in `kmsg.vuma` is the same constant
re-used as a mask, not a sentinel.

---

## 15. Cross-Compilation

The kernel compiles on **all 19 VUMA backends** (the 19 `BackendKind` variants
in `src/codegen/src/backend.rs`), but only the `x86_64` backend has
hosted-mode pre-registered syscall stubs (so only `x86_64` can actually *run*
the kernel as a hosted process today). The other 18 backends compile + verify
+ IVE-pass but the resulting ELF is bare-metal-shaped (real `boot.S` entry,
no `_start` stub, every extern resolves to `__ffi_fallback_stub`).

Of those 19 codegen backends, only **4 have per-arch kernel source** under
`womb/kernel/arch/`: `x86_64`, `aarch64`, `riscv64`, and `wasm32` (which
ships only `sched_hal.vuma`). The remaining 15 have no per-arch kernel
files at all — they compile the kernel's arch-agnostic core, with every
hardware extern routed to `__ffi_fallback_stub`. Adding a new *real* arch
port (a 5th directory under `womb/kernel/arch/`) is covered in
[`kernel-porting-guide.md`](./kernel-porting-guide.md).

### The 19 backends

```
    x86_64       aarch64      aarch64_be   riscv64      riscv32
    arm32        armeb        mips64       mips64be     ppc64
    ppc64le      loongarch64  s390x        sparc64      alpha
    hppa         m68k         x86_32       wasm32
```

(Lookup table: `src/codegen/src/backend.rs::BackendKind::isa_name`.)

### Syscall number mapping per arch

The hosted-mode `trampoline.vuma` documents the **x86_64-native** Linux
syscall numbers (write=1, read=0, exit=60, mmap=9, munmap=11). The
`womb/syscalls.vuma` reference uses the **asm-generic** numbers (write=64,
read=63, exit=93, mmap=222, munmap=215). The mapping is:

| Syscall  | asm-generic | x86_64-native | aarch64-native | riscv64-native |
|----------|-------------|---------------|----------------|----------------|
| read     | 63          | 0             | 63 (identity)  | 63 (identity)  |
| write    | 64          | 1             | 64 (identity)  | 64 (identity)  |
| exit     | 93          | 60            | 93 (identity)  | 93 (identity)  |
| mmap     | 222         | 9             | 222 (identity) | 222 (identity) |
| munmap   | 215         | 11            | 215 (identity) | 215 (identity) |

aarch64 and riscv64 use the asm-generic table verbatim (the numbers are
identical). x86_64 has its own legacy table from the i386 era; the kernel's
hosted stubs use the x86_64-native numbers because they're invoked by the
`syscall` instruction which expects the native ABI.

### Syscall ABI per arch

The syscall-number register + argument registers differ per arch:

| Arch     | nr reg | a0   | a1   | a2   | a3   | a4   | a5   | return | entry        |
|----------|--------|------|------|------|------|------|------|--------|--------------|
| x86_64   | rax    | rdi  | rsi  | rdx  | r10  | r8   | r9   | rax    | `syscall`    |
| aarch64  | x8     | x0   | x1   | x2   | x3   | x4   | x5   | x0     | `svc #0`     |
| riscv64  | a7     | a0   | a1   | a2   | a3   | a4   | a5   | a0     | `ecall`      |

Note that x86_64 uses **r10** (not rcx) for `a3` — SYSCALL clobbers rcx with
RFLAGS, so the SysV AMD64 syscall ABI promotes rcx→r10. The kernel's
`syscall/abi.vuma::syscall_args_from_frame` reflects this: `args.a3 ← tf.r10`
(not `tf.rcx`). On aarch64/riscv64 the syscall ABI matches the regular
calling convention (no register shuffle).

### Cross-compile + QEMU run (per arch)

```
    # Build compile_dump once:
    . "$HOME/.cargo/env"
    cargo build --profile release-fast --bin compile_dump

    # Compile the kernel for a non-x86_64 arch:
    ./target/release-fast/compile_dump womb/kernel/kernel.vuma \
        /tmp/kernel-aarch64.bin aarch64 --verify

    # Run under QEMU user-mode (aarch64 example):
    qemu-aarch64 /tmp/kernel-aarch64.bin
    # Expected output: "vuma kernel: hello"
```

The kernel_smoke.sh / kernel_parity.sh scripts (§16) automate this across
all 19 backends.

---

## 16. Testing

The kernel has three test harnesses, each covering a different scope:

### 16.1 `scripts/kernel_smoke.sh` — single-arch boot test

Compiles `womb/kernel/kernel.vuma` for `x86_64` with `--verify`, runs the
resulting ELF as a regular Linux process, greps stdout for
`"vuma kernel: hello"`, and checks exit code 0. This is the minimum bar
every commit must clear.

```
    ./scripts/kernel_smoke.sh
    # Expected: "PASS: kernel boots, prints banner, exits 0"
```

### 16.2 `scripts/kernel_parity.sh` — multi-backend sweep

Compiles + runs `kernel.vuma` and a subset of gold-standard tests across
**all 19 backends** using QEMU user-mode emulators for non-x86_64 arches.

```
    ./scripts/kernel_parity.sh          # full sweep (~10 minutes)
    ./scripts/kernel_parity.sh --quick  # arena_basic + kernel smoke only
```

The script rebuilds `compile_dump` if Cargo.toml is newer than the binary,
then iterates the 19 backends, compiling + running each test. Exits 0 only
if every backend passes.

### 16.3 `tests/gold_standard/` — wave-pinned regression tests

The gold-standard suite is a curated set of `.vuma` programs pinned to
specific expected exit codes, organized by the wave that introduced them:

```
    tests/gold_standard/
        arena_wave0/         K0 arena-builtin tests (4 files)
        arena_wave1/         K0 arena-overflow regression (4 files)
        arena_wave2/         K0 arena-multiple + grow tests
        pmt_wave1/           K1 PMT basics: layout, state, transform (5 files)
        pmt_wave2/           nested layouts
        pmt_wave3_negative/  negative tests (parse errors → expected failure)
        pmt_wave5/           if-expressions
        pmt_wave7/           transform + #[borrow]
        pmt_wave8/           FFI marshal
        pmt_wave9/           arena + transform interaction
        pmt_wave10/          FFI reinit + pure
        atomics/             ~200 atomic CAS tests (s4..s106 series)
        arithmetic/          arithmetic primitives
        bitwise/             bitwise ops
        control_flow/        if/while/for
        functions/           fn calls + recursion
        structs/             layout field access
        complex_stores/      multi-field stores
        concurrency/         (placeholder for K8 SMP tests)
        crypto_patterns/     K10 crypto primitives
        edge_cases/          parser edge cases
        ffi_wave0..4/        FFI marshal waves
        kernel_crypto/       sha256 KAT test
        linked_structures/   linked list / tree
        memory/              state_new + field access
        multi_function/      cross-fn calls
        nested_loops/        nested loop nests
        pointers/            (no real pointer tests — PMT-forbidden)
        u32_arith/           u32 arithmetic
```

Run them all:

```
    ./scripts/run_all_gold.sh
    # or selectively:
    ./target/release-fast/compile_dump tests/gold_standard/arena_wave1/arena_basic.vuma \
        /tmp/test.bin x86_64 --verify
    /tmp/test.bin; echo "exit=$?"
    # Expected exit: 0 (matches the file's "Expected exit code:" comment)
```

### 16.4 Per-module self-tests

Every `.vuma` file in `womb/kernel/` ends with a `fn main() -> i32` self-test
that exercises the module's API surface. The convention is:

```
    fn main() -> i32 {
        // Test 1: <first check>
        if <check1 fails> { return 1; }
        // Test 2: <second check>
        if <check2 fails> { return 2; }
        // ...
        return 0;
    }
```

So a future CI failure pinpoints the broken check by the exit code. Run a
module's self-test:

```
    ./target/release-fast/compile_dump womb/kernel/mm/pmm.vuma \
        /tmp/pmm.bin x86_64 --verify
    /tmp/pmm.bin; echo "exit=$?"
    # Expected: "IVE: Pass passed=1 failed=0 total=1" + exit=0
```

### 16.5 KAT tests (`scripts/womb_kat_tests/`)

The `womb_kat_tests/` directory contains known-answer tests for crypto
algorithms (SHA-256, AES, Ed25519, etc.). Each test is a `.vuma` program
that computes a hash/ciphertext/signature and checks it against a known
value. The directory holds `.vuma` test data consumed by
`scripts/womb_test_harness.sh`; the standalone `run_all_kat.sh` runner was
removed during the 2026-07 cleanup. Run the cross-architecture real-KAT
suite (`scripts/real_kat_tests/`) instead:

```
    ./scripts/run_real_kat.sh
```

The KAT tests are mostly used by the womb library (`womb/crypto/`) but the
kernel's `crypto/api.vuma` shares the same KAT infrastructure.

---

## 17. Cross-References

- [`architecture.md`](./architecture.md) — VUMA 2.0 compiler architecture
  (parser, SCG, IR pipeline, IVE verifiers, e-graph layout optimizer, 19
  backends, dependent state types, FFI marshal pass). Read this first if any
  term in this document is unfamiliar.
- [`language-reference.md`](./language-reference.md) — VUMA 2.0 language
  reference (layouts, `State<T>`, `state_new`, `state.field`, transforms,
  extern "C", `#[borrow]`, `as Address`).
- [`building.md`](./building.md) — how to build the compiler and run
  `compile_dump --verify` on kernel modules.
- [`kernel-porting-guide.md`](./kernel-porting-guide.md) — step-by-step guide
  to porting the kernel to a new architecture (worked example: x86_64).
- [`kernel-developer-guide.md`](./kernel-developer-guide.md) — how to add
  syscalls, drivers, filesystems, and PMT kernel code; do/don't examples.
- [`contributing.md`](./contributing.md) — general contribution workflow.

The canonical kernel source lives under `womb/kernel/`. Each subsystem file
carries a header comment documenting its K-wave lineage, its PMT discipline,
and the self-test command. The shared worklog (kept alongside the VWK
orchestration tooling — it is **not** inside this repository; ask your
orchestrator for the path) records every K-wave's design decisions,
deviations from the contract, and forward-looking notes for K11+ (real asm
trampolines, real crypto algorithms, real networking stack).

To find a specific K-wave's design notes in the worklog, search for the
`Task ID: K<NN>` marker (e.g. `Task ID: K2a` for the PMM wave). Each entry
includes the design decisions, contract deviations with rationale,
deferred-to-future-wave notes, and the final commit hash. The worklog is
append-only — never edit a prior entry; add a new section for your wave.
