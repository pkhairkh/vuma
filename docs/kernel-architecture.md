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
8. [Cross-References](#8-cross-references)

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
│    console.vuma  kmain.vuma  kernel.vuma                               │
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
│  L2 — FFI Trampolines (womb/kernel/arch/<arch>/trampoline.vuma +       │
│        *_trampoline.vuma + mm_trampoline.vuma + switch.vuma)           │
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
                │  fn kmain() -> i32   │   (womb/kernel/kmain.vuma)
                │  c ← console_init()  │
                │  kmain_print_banner  │   ("vuma kernel: hello\n")
                │  return 0            │
                └──────────────────────┘
```

Future-wave additions (documented in `kmain.vuma`):

```
    pmm_init(c)   ← K2  physical memory manager (buddy)
    vmm_init(c)   ← K3  virtual memory / page tables
    trap_init(c)  ← K4  IDT + exception/syscall dispatch
    sched_init(c) ← K5  scheduler + first task
    vfs_init(c)   ← K6  rootfs + init binary
```

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
has a `count` field and `nodes[i].free = 1` markers; `vfs.vuma`'s `FdTable`
reuses freed fd indices; `proc/scheduler.vuma` reuses freed pid slots. This
matches the "transform over typed state" model — reclamation is a state
mutation, not a memory operation.

---

## 5. Per-Architecture Abstraction

The kernel currently targets three architectures, each in
`womb/kernel/arch/<arch>/`:

```
    womb/kernel/arch/
        x86_64/
            bootinfo.vuma            (BootInfo layout + bootinfo_init)
            trampoline.vuma          (write/read/exit/mmap/... externs)
            mm_trampoline.vuma       (pte_read/pte_write/tlb_flush/invlpg/cr3_*)
            trap_trampoline.vuma     (TrapFrame layout + idt_load/irq_*/pic_eoi/cr2_read)
            switch.vuma              (Task saved-reg subset + context_switch/cr3_write)
            pt.vuma                  (PTE bit layout + pte_make/pte_addr/pte_* helpers)
        aarch64/
            mm_trampoline.vuma       (TTBR0/1 + TLBI aliases)
            trap_trampoline.vuma     (ESR/FAR-based TrapFrame + eret)
            switch.vuma              (x19-x30 + sp callee-saved set)
            pt.vuma                  (4 KB-granule, 48-bit VA, 3-level paging)
        riscv64/
            mm_trampoline.vuma       (satp + sfence.vma aliases)
            trap_trampoline.vuma     (scause/stval-based TrapFrame + sret)
            switch.vuma              (s0/s1/x8-x23/x30 callee-saved set)
            pt.vuma                  (Sv39 / Sv48 paging)
```

The arch layer is the **only** place that mentions CPU registers, MMU
intrinsics, or trap-frame geometry. Above the arch layer, everything talks
about abstract `State<TrapFrame>`, `State<PageTable>`, `State<Task>`, etc.
— the same code in `womb/kernel/trap/trap.vuma`, `womb/kernel/proc/scheduler.vuma`,
`womb/kernel/syscall/abi.vuma` works on all three archs (modulo per-arch field
re-ordering of `TrapFrame`).

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
    fn pte_present(pte: u64) -> u8;
    fn pte_writable(pte: u64) -> u8;
    fn pte_user(pte: u64) -> u8;
    fn pte_no_exec(pte: u64) -> u8;
```

The arch-agnostic `vmm.vuma` walks page tables by calling these helpers +
`pte_read`/`pte_write` from `mm_trampoline.vuma`. Adding a fourth arch (e.g.
loongarch64) is a matter of writing five files (bootinfo, trampoline,
mm_trampoline, trap_trampoline, switch, pt) — the rest of the kernel is
unchanged. See [`kernel-porting-guide.md`](./kernel-porting-guide.md) for the
step-by-step.

---

## 6. FFI Trampoline Patterns

The kernel's FFI surface is concentrated in `womb/kernel/arch/<arch>/trampoline.vuma`
and its sibling `*_trampoline.vuma` files. Every extern is declared as
`extern "C" { fn name(args) -> ret; }`. The codegen lowers each call site per
the target ABI (SysV AMD64 on x86_64, AAPCS on aarch64, RV64GC on riscv64).

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
exit=nr 60, mmap=nr 9). They run on the host Linux kernel.

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

---

## 8. Cross-References

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
and the self-test command. The shared worklog at
`/home/z/my-project/worklog.md` records every K-wave's design decisions,
deviations from the contract, and forward-looking notes for K11+ (real asm
trampolines, real crypto algorithms, real networking stack).
