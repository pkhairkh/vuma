# VWK Kernel Porting Guide

This guide walks through porting the VWK (Vuma Womb Kernel) to a new CPU
architecture. The kernel currently ships per-arch files for **x86_64**,
**aarch64**, **riscv64**, and **wasm32** under `womb/kernel/arch/<arch>/`
(4 ports — see `womb/kernel/arch/`). To add a fifth architecture (e.g.
loongarch64, ppc64le, s390x), follow the nine steps below.

Note the distinction from the compiler: VUMA's codegen has 19 `BackendKind`
variants (see [`architecture.md` §8](./architecture.md#8-backends)), and the
kernel *compiles* on all 19. But only 4 of those have per-arch kernel source
under `womb/kernel/arch/`; the remaining 15 run the kernel in hosted mode
where every hardware extern resolves to `__ffi_fallback_stub` (no real
boot path). This guide is about adding a 5th *real* arch port.

The x86_64 port is the worked example throughout — it is the most mature port
(the hosted-mode build target since K1) and its `bootinfo.vuma`,
`trampoline.vuma`, `pt.vuma`, `mm_trampoline.vuma`, `trap_trampoline.vuma`, and
`switch.vuma` are the canonical templates. Copy them as your starting point.

The kernel's per-arch abstraction is documented in
[`kernel-architecture.md` §5](./kernel-architecture.md#5-per-architecture-abstraction);
read that section first if you haven't.

---

## Table of Contents

1. [Prerequisites](#prerequisites)
2. [Step 1 — Pick the architecture](#step-1--pick-the-architecture)
3. [Step 2 — Write boot.S / `_start` stub](#step-2--write-boots--_start-stub)
4. [Step 3 — Write bootinfo.vuma](#step-3--write-bootinfovuma)
5. [Step 4 — Write trampoline.vuma](#step-4--write-trampolinevuma)
6. [Step 5 — Write trap.S / trap_trampoline.vuma](#step-5--write-traps--trap_trampolinevuma)
7. [Step 6 — Write switch.S / switch.vuma](#step-6--write-switchs--switchvuma)
8. [Step 7 — Write pt.vuma](#step-7--write-ptvuma)
9. [Step 8 — Write mm_trampoline.vuma](#step-8--write-mm_trampolinevuma)
10. [Step 9 — Run the smoke test](#step-9--run-the-smoke-test)
11. [Per-Arch TrapFrame Comparison](#per-arch-trapframe-comparison)
12. [Per-Arch PTE Comparison](#per-arch-pte-comparison)
13. [Per-Arch Context Switch](#per-arch-context-switch)
14. [Per-Arch Syscall ABI](#per-arch-syscall-abi)
15. [Testing Your Port](#testing-your-port)
16. [Common Pitfalls](#common-pitfalls)
17. [Porting Checklist](#porting-checklist)

---

## Prerequisites

Before starting, confirm:

- The VUMA 2.0 compiler already emits code for the target arch. Run
  `./target/release-fast/compile_dump --help` and check that `<arch>` is
  listed. If not, add the backend first (out of scope for this guide — see
  `src/codegen/src/<arch>/` for the existing 19 codegen backends in the
  `BackendKind` enum, and [`architecture.md` §8](./architecture.md#8-backends)
  for the backend contract).
- You have a QEMU (or real hardware) boot target. The hosted-mode test path
  uses Linux syscalls; the bare-metal test path uses QEMU `-kernel` /
  `-bios`. Both should work before you start.
- You have read [`architecture.md`](./architecture.md) §3 (State Type System)
  and §9 (FFI Marshal Pass) — the porting work touches both surfaces.

---

## Step 1 — Pick the architecture

Choose the target. The naming convention is the LLVM/Rust target-triple arch
name: `x86_64`, `aarch64`, `riscv64`, `loongarch64`, `ppc64le`, `s390x`, etc.

Create the directory:

```
mkdir -p womb/kernel/arch/<arch>/
```

The file count differs per arch:

| Arch     | Files                                                                                                       | Count |
|----------|-------------------------------------------------------------------------------------------------------------|-------|
| x86_64   | bootinfo, trampoline, mm_trampoline, trap_trampoline, switch, pt, vmm_hal                                   | **7** |
| aarch64  | bootinfo, mm_trampoline, trap_trampoline, switch, pt, vmm_hal (no trampoline.vuma — hosted path reuses x86_64's) | **6** |
| riscv64  | bootinfo, mm_trampoline, trap_trampoline, switch, pt, vmm_hal (same as aarch64)                             | **6** |
| wasm32   | sched_hal (wasm-specific scheduler HAL shim; no MMU/paging/trap files — wasm32 has no concept of these)     | **1** |

If you're adding a **hosted** port (the new arch will run as a regular Linux
process for testing), follow the x86_64 pattern: write all 7 files (bootinfo
+ trampoline + the 5 core files). If you're adding a **bare-metal-only**
port (no hosted test path), follow the aarch64/riscv64 pattern: write only
the 6 core files; the boot protocol handling goes in `boot.S` (Step 2) instead.
The wasm32 port is a special case (single HAL shim, no paging or traps —
wasm has no MMU), not a template for new ports.

The remaining files (`mm/pmm.vuma`, `proc/scheduler.vuma`, `net/tcp.vuma`,
...) are arch-agnostic and need no porting — they consume the per-arch
interfaces you are about to define.

**Worked example (x86_64):** the directory is `womb/kernel/arch/x86_64/` and
it contains exactly these six files (plus the `main()` stubs that let each
compile standalone for verification):

```
womb/kernel/arch/x86_64/
    bootinfo.vuma          (Step 3)
    trampoline.vuma        (Step 4)
    trap_trampoline.vuma   (Step 5 — note: file is trap_trampoline.vuma,
                            the trap.S file from the contract maps to this)
    switch.vuma            (Step 6)
    pt.vuma                (Step 7)
    mm_trampoline.vuma     (Step 8)
```

---

## Step 2 — Write boot.S (or the backend `_start` stub)

The L1 boot entry (see `kernel-architecture.md` §1) is responsible for:

1. Setting up the stack pointer (bare-metal) or accepting the stack the
   kernel loader already set up (hosted).
2. Populating the `argc`/`argv` slot in BSS that `__vuma_argc` and
   `__vuma_argv` read from (hosted mode).
3. Calling `main()` (which calls `kmain()`).
4. After `main()` returns, calling `exit(ret)` (hosted) or halting the CPU
   (bare-metal).

**Hosted mode** (`x86_64`): the boot entry is `_start` in
`src/codegen/src/x86_64/mod.rs::build_runtime_syscall_stubs`. It is a 20-line
asm stub that reads `argc`/`argv` off the stack at process entry, stores them
in the BSS slot, calls `main`, and calls `sys_exit` with the return value.
This is already implemented — no action needed for hosted ports.

**Bare-metal mode** (K11+): you write `womb/kernel/arch/<arch>/boot.S`. It
parses the multiboot2 / stivale2 / SBI boot protocol, sets up the GDT (x86_64)
or SCTLR (aarch64) or `sstatus` (riscv64), enables paging, and jumps to
`kmain`. This file lives outside PMT (L1) and is reviewed by hand.

For now (K12), skip this step — hosted mode handles it.

---

## Step 3 — Write bootinfo.vuma

This file declares the `BootInfo` layout (the per-arch boot-protocol
structure) and a `bootinfo_init()` helper that populates it from the runtime
stubs. The layout must include at minimum:

```
    layout BootInfo = {
        argc:      u64,    // process argc (hosted) or 0 (bare-metal)
        argv:      u64,    // address of argv[] (hosted) or 0 (bare-metal)
        mem_size:  u64,    // usable memory for the arena, in bytes
        cmdline:   u64,    // address of cmdline string
    }
```

**Worked example (x86_64):** see `womb/kernel/arch/x86_64/bootinfo.vuma`. It
re-declares the `__vuma_argc`/`__vuma_argv` externs (VUMA has no `import`),
calls them in `bootinfo_init`, and defaults `mem_size` to 16 MB
(`16777216u64`). The `cmdline` field is set to `argv` for now — a later wave
will dereference `argv[0]` once the kernel has a safe deref helper.

For bare-metal ports, `bootinfo_init` reads the multiboot2 / stivale2 / SBI
structure pointer from a register (e.g. `rdi` on x86_64 multiboot2, `x0` on
aarch64 SBI, `a0` on riscv64 SBI) and parses the mem-map tag to compute
`mem_size`. The parsing logic is per-boot-protocol; consult your bootloader's
spec.

---

## Step 4 — Write trampoline.vuma

This file declares the **hosted-mode** FFI externs that the kernel logic
calls. Every extern is a Linux syscall stub on hosted, a real asm stub on
bare-metal.

**Worked example (x86_64):** see `womb/kernel/arch/x86_64/trampoline.vuma`. It
declares:

```
    extern "C" {
        fn write(fd: i64, buf: Address, count: i64) -> i64;
        fn read(fd: i64, buf: Address, count: i64) -> i64;
        fn exit(code: i64);
        fn getpid() -> i64;
        fn fork() -> i64;
        fn waitpid(pid: i64, status: Address, options: i64) -> i64;
        fn mmap(addr: Address, length: u64, prot: i32, flags: i32,
                fd: i32, offset: i64) -> Address;
        fn munmap(addr: Address, length: u64) -> i64;
        fn mremap(...) -> Address;
        fn __vuma_argc() -> i64;
        fn __vuma_argv() -> Address;
    }
```

The header comment for each extern documents:
- The Linux syscall number it wraps (write=1, exit=60, mmap=9, ...).
- The SysV AMD64 calling-convention arg registers (RDI/RSI/RDX/RCX/R8/R9).
- Whether the stub needs a register shuffle (mmap and mremap shuffle arg4
  from RCX to R10 because SYSCALL clobbers RCX with RFLAGS).
- Whether the stub is no-return (exit ends with INT3 guard).

For your arch: enumerate the syscalls the kernel needs (start with write,
read, exit, mmap, munmap — these cover `console.vuma` and `kernel.vuma`).
Document the calling convention. Document the syscall-number mapping.

> **Syscall-number clarification.** The Linux kernel maintains **per-arch**
> syscall-number tables. The asm-generic table (defined in
> `asm-generic/unistd.h`) is what new architectures (aarch64, riscv64,
> loongarch64, riscv32) use verbatim. Legacy architectures (x86_64, x86_32,
> arm32, mips, ppc, sparc, s390, m68k, alpha, hppa) have their own
> historical tables that predate asm-generic.
>
> Concretely:
> - `__NR_write` is **1 on x86_64** (legacy x86_64 table)
> - `__NR_write` is **64 on aarch64** (asm-generic, identity)
> - `__NR_write` is **64 on riscv64** (asm-generic, identity)
>
> The asm-generic number 64 is **the identity** on aarch64/riscv64 (no
> remapping needed), but on x86_64 the same logical syscall (`write(2)`)
> uses the legacy number 1. The VUMA compiler's hosted x86_64 stubs use the
> **x86_64-native** numbers (1, 0, 60, 9, 11) — these are what the
> `syscall` instruction expects. The `womb/syscalls.vuma` reference file
> uses the **asm-generic** numbers (64, 63, 93, 222, 215) for portability
> documentation. The two never mix: the syscall intrinsic's first arg
> is per-arch-native; everything else uses asm-generic for cross-arch
> consistency.
>
> See [`kernel-architecture.md` §15](./kernel-architecture.md#15-cross-compilation)
> for the full per-arch syscall-number table.

**Important:** any extern not pre-registered in the backend's
`build_runtime_syscall_stubs` resolves to `__ffi_fallback_stub` at link time
(returns 0 / no-op). This is intentional for K12 — K11+ will register real
asm stubs for bare-metal targets. The trampoline's job today is to declare
the contract; the backend's job is to satisfy it.

---

## Step 5 — Write trap.S / trap_trampoline.vuma

The trap layer is split: `trap_trampoline.vuma` declares the per-arch
`TrapFrame` layout + the externs the trap dispatcher calls (IDT load, PIC
mask/unmask/EOI, CR2 read on x86_64; ESR/FAR + TLBI on aarch64; scause/stval
+ sfence.vma on riscv64). The actual asm stubs that push registers and call
`dispatch_trap(tf)` live in `trap.S` (K11+).

**Worked example (x86_64):** see
`womb/kernel/arch/x86_64/trap_trampoline.vuma`. It declares the 22-field
`TrapFrame` (RAX, RBX, RCX, RDX, RSI, RDI, RBP, R8..R15, vector, error_code,
RIP, CS, RFLAGS, RSP, SS) — the layout the trap_entry asm pushes and
trap_exit pops. It declares:

```
    extern "C" {
        fn idt_load(idt_ptr: Address);
        fn irq_mask(irq: u8);
        fn irq_unmask(irq: u8);
        fn pic_eoi(irq: u8);
        fn cr2_read() -> u64;
    }
```

And the helper `trap_frame_init(tf: State<TrapFrame>)` (init-style API — see
`kernel-architecture.md` §3) zeroes all 22 fields.

For your arch:
- Pick the trap-entry register save order. It must match the trap_entry asm
  exactly. The x86_64 convention is: CPU pushes SS/RSP/RFLAGS/CS/RIP + error
  code (some exceptions); stub pushes vector; trap_entry pushes 15 GP
  registers.
- Declare the equivalent `idt_load`/`irq_mask`/`pic_eoi` externs (or GIC
  redistributor ops on aarch64, PLIC ops on riscv64).
- Declare the page-fault address reader (`cr2_read` → `far_read` on aarch64
  → `stval_read` on riscv64).

The `TrapFrame` layout MUST be re-declared byte-identically by every
consumer (`trap.vuma`, `syscall/abi.vuma`, `proc/scheduler.vuma`). The
verifiers catch any drift.

See [Per-Arch TrapFrame Comparison](#per-arch-trapframe-comparison) below
for the side-by-side layout of all three existing ports.

---

## Step 6 — Write switch.S / switch.vuma

The context-switch layer is also split: `switch.vuma` declares the per-arch
`Task` saved-register subset + the `context_switch(prev, next)` extern. The
asm that saves callee-saved regs to `prev.{...}` and loads them from
`next.{...}` lives in `switch.S` (K11+).

**Worked example (x86_64):** see `womb/kernel/arch/x86_64/switch.vuma`. The
Task layout has 17 fields: 5 header (pid/ppid/state/prio/vruntime), 8
callee-saved regs (rsp/rip/rbp/rbx/r12-r15), 4 trailer (mm_root/fs_root/fds/
next). The asm (sketched in the file header) is:

```
    context_switch:
        push rbx; push rbp; push r12; push r13; push r14; push r15
        mov [rdi + offsetof(Task.rsp)], rsp    ; save prev's rsp
        ... (save rbp, rbx, r12-r15 to prev) ...
        mov rsp, [rsi + offsetof(Task.rsp)]    ; load next's rsp
        ... (load rbp, rbx, r12-r15 from next) ...
        pop r15; pop r14; pop r13; pop r12; pop rbp; pop rbx
        ret                                     ; pops next's saved rip
```

The extern declarations carry `#[borrow]`:

```
    extern "C" {
        #[borrow] fn context_switch(prev: State<Task>, next: State<Task>);
        fn cr3_write(val: u64);
    }
```

`#[borrow]` is load-bearing: without it, the marshal would mark `prev` and
`next` as consumed after the call, and the scheduler's `prev.vruntime += tick`
after the switch would trip the use-after-invalidate verifier.

For your arch: pick the callee-saved register set (SysV AMD64: rbx, rbp,
r12-r15; AAPCS: x19-x29, sp; RV64: s0-s11). Declare `context_switch` with
`#[borrow]` on both State<Task> params. Declare the address-space switch
(`cr3_write` → `ttbr0_write` on aarch64 → `satp_write` on riscv64).

See [Per-Arch Context Switch](#per-arch-context-switch) below for the
side-by-side Task-layout comparison.

---

## Step 7 — Write pt.vuma

This file declares the per-arch `PageTableEntry` layout (or, on arches where
the PTE is a single u64, just the bit-layout helpers operating on raw u64)
and the field-access helpers:

```
    fn pte_make(paddr: u64, flags: u64) -> u64;
    fn pte_addr(pte: u64) -> u64;
    fn pte_present(pte: u64) -> u8;
    fn pte_writable(pte: u64) -> u8;
    fn pte_user(pte: u64) -> u8;
    fn pte_no_exec(pte: u64) -> u8;
```

**Worked example (x86_64):** see `womb/kernel/arch/x86_64/pt.vuma`. The PTE
is a single u64 with the bit layout documented in the Intel SDM Vol. 3,
Fig. 4-19 (Present=bit 0, R/W=bit 1, U/S=bit 2, ..., NX=bit 63). The helpers
operate on the raw u64; the layout declaration is just
`layout PageTableEntry = { raw: u64 }` (kept for DoD compliance — the helpers
themselves take/return u64 because PTEs fit in a register).

The arch-agnostic `mm/vmm.vuma` walks page tables by calling these helpers +
the `pte_read`/`pte_write` externs from `mm_trampoline.vuma`. The walk is
the same on every arch; only the bit positions in the PTE differ.

For your arch: look up your MMU's PTE format in the architecture reference
manual (Intel SDM for x86_64, ARM ARM D8.2 for aarch64, RISC-V Privileged
ISA §4.3 for riscv64). Translate each bit position to a decimal mask, write
the helpers, and add a self-test that round-trips a PTE
(`pte_addr(pte_make(p, f)) == p`).

**Pitfall:** Use the SAME mask for build and read. K2c's contract specified
a `pte_addr` mask of `0x000FFFFFFFFFFFFF` (bits 0-51) but `pte_make` already
sanitized `paddr` to bits 12-51 — the round-trip self-test would have failed
with `pte_addr(pte_make(p, f)) == p | flags`. The fix (documented in
`pt.vuma`'s header) is to use the bits-12-51 mask — written either as
decimal **`17592186028032`** or as hex **`0x000FFFFFFFFFF000`** — for both
build and read. The decimal form is preferred per the kernel's
decimal-constants discipline (see [`kernel-architecture.md` §10.6](./kernel-architecture.md#106-the-hex-literal-width-extension-subtlety)),
but the hex form is acceptable in comments and prose for readability.

See [Per-Arch PTE Comparison](#per-arch-pte-comparison) below for the
side-by-side PTE bit layout of all three existing ports.

---

## Step 8 — Write mm_trampoline.vuma

This file declares the MMU-specific externs: `pte_read`, `pte_write`,
`tlb_flush`, `invlpg`, and (for arches with multiple address spaces) the
address-space switch externs (`cr3_read`/`cr3_write` on x86_64,
`ttbr0_read`/`ttbr0_write` on aarch64, `satp_read`/`satp_write` on riscv64).

**Worked example (x86_64):** see `womb/kernel/arch/x86_64/mm_trampoline.vuma`.
It re-declares `PageTable` byte-identically to `mm/vmm.vuma`:

```
    layout PageTable = { root_phys: u64, levels: u8 }
```

And declares:

```
    extern "C" {
        #[borrow] fn pte_read(pt: State<PageTable>, level: u8, idx: u32) -> u64;
        #[borrow] fn pte_write(pt: State<PageTable>, level: u8, idx: u32, val: u64);
        fn tlb_flush();
        fn invlpg(vaddr: u64);
        fn cr3_read() -> u64;
        fn cr3_write(val: u64);
    }
```

Again `#[borrow]` is load-bearing for `pte_read`/`pte_write` — without it,
`vmm.vuma`'s walk loop would trip the use-after-invalidate verifier after
the first `pte_read`.

For your arch: declare the equivalent externs. The arch-agnostic
`mm/vmm.vuma` consumes this interface; if you match the x86_64 shape, no
changes are needed in `vmm.vuma`. The asm stubs themselves are K11+ work
(`pte_read` is `mov rax, [rdi + idx*8]; ret` on x86_64, etc.).

---

## Step 9 — Run the smoke test

Verify each of the per-arch files compiles standalone:

```
    cd /home/z/vuma
    . "$HOME/.cargo/env"
    cargo build --profile release-fast --bin compile_dump

    # x86_64 (6 files):
    for f in bootinfo trampoline trap_trampoline switch pt mm_trampoline; do
        ./target/release-fast/compile_dump \
            womb/kernel/arch/x86_64/$f.vuma \
            /tmp/$f.bin x86_64 --verify
        /tmp/$f.bin ; echo "$f exit=$?"
    done

    # aarch64 or riscv64 (4 files — no bootinfo, no trampoline):
    for f in trap_trampoline switch pt mm_trampoline; do
        ./target/release-fast/compile_dump \
            womb/kernel/arch/aarch64/$f.vuma \
            /tmp/$f.bin aarch64 --verify
        # Can't run aarch64 binary on x86_64 host without QEMU; just verify IVE Pass:
        # ./tmp/$f.bin would need: qemu-aarch64 /tmp/$f.bin
    done
```

Each must print `IVE: Pass passed=1 failed=0 total=1` and (when run via
QEMU on its native arch) exit 0.

Then verify the arch-agnostic kernel still compiles + links (it consumes your
new arch layer):

```
    ./target/release-fast/compile_dump womb/kernel/kernel.vuma \
        /tmp/kernel.bin <arch> --verify
    /tmp/kernel.bin ; echo "kernel exit=$?"
```

Expected stdout on hosted x86_64: `vuma kernel: hello`. Expected exit: 0.
For other arches, run via QEMU user-mode (see [Testing Your Port](#testing-your-port)
below).

Finally, run the full gold-standard test suite for any arch-specific tests:

```
    ls tests/gold_standard/ | grep <arch>
```

If any of the above fails, consult the file's header comment — every kernel
arch file documents its K-wave lineage, the contract it satisfies, and the
deviations it took. The worklog (kept alongside the VWK orchestration
tooling — it is **not** inside this repository; ask your orchestrator for
the path) records the design decisions for each wave. Search for
`Task ID: K<NN>` to find a specific wave's notes.

---

## Per-Arch TrapFrame Comparison

The three existing ports declare TrapFrame with different field counts and
naming conventions:

### x86_64 — 22 fields (1,760 bytes)

```vuma
layout TrapFrame = {
    rax: u64,  rbx: u64,  rcx: u64,  rdx: u64,
    rsi: u64,  rdi: u64,  rbp: u64,  r8:  u64,
    r9:  u64,  r10: u64,  r11: u64,  r12: u64,
    r13: u64,  r14: u64,  r15: u64,
    vector:      u64,   // interrupt vector (0..255)
    error_code:  u64,   // CPU-pushed error code
    rip:         u64,   // instruction pointer at trap
    cs:          u64,   // code segment
    rflags:      u64,   // flags register
    rsp:         u64,   // stack pointer
    ss:          u64,   // stack segment
}
```

Push order: CPU pushes SS/RSP/RFLAGS/CS/RIP + (optional) error_code; stub
pushes vector; trap_entry pushes 15 GP regs (RAX, RBX, RCX, RDX, RSI, RDI,
RBP, R8-R15).

### aarch64 — 35 fields (2,800 bytes)

```vuma
layout TrapFrame = {
    x0:  u64,  x1:  u64,  x2:  u64,  x3:  u64,
    x4:  u64,  x5:  u64,  x6:  u64,  x7:  u64,
    x8:  u64,  x9:  u64,  x10: u64,  x11: u64,
    x12: u64,  x13: u64,  x14: u64,  x15: u64,
    x16: u64,  x17: u64,  x18: u64,  x19: u64,
    x20: u64,  x21: u64,  x22: u64,  x23: u64,
    x24: u64,  x25: u64,  x26: u64,  x27: u64,
    x28: u64,  x29: u64,   // frame pointer (FP)
    x30: u64,              // link register (LR)
    sp_el0:    u64,        // EL0 stack pointer (preserved across exception)
    elr_el1:   u64,        // exception link register (return address)
    spsr_el1:  u64,        // saved processor state (PSTATE at trap)
    esr_el1:   u64,        // exception syndrome register (EC + IL + ISS)
}
```

Push order: trap_entry pushes x0-x30 (31 regs), then reads SP_EL0, ELR_EL1,
SPSR_EL1, ESR_EL1 from system registers. No CPU-pushed error code field;
ESR_EL1 carries the exception class + ISS.

### riscv64 — 35 fields (2,800 bytes)

```vuma
layout TrapFrame = {
    ra:   u64,   // x1  — return address
    sp:   u64,   // x2  — stack pointer
    gp:   u64,   // x3  — global pointer
    tp:   u64,   // x4  — thread pointer
    t0:   u64,   // x5  — temporary
    t1:   u64,   // x6  — temporary
    t2:   u64,   // x7  — temporary
    s0:   u64,   // x8  — callee-saved (frame pointer)
    s1:   u64,   // x9  — callee-saved
    a0:   u64,   // x10 — argument / return value
    a1:   u64,   // x11 — argument / return value
    a2:   u64,   // x12 — argument
    a3:   u64,   // x13 — argument
    a4:   u64,   // x14 — argument
    a5:   u64,   // x15 — argument
    a6:   u64,   // x16 — argument
    a7:   u64,   // x17 — argument (syscall number on RISC-V Linux)
    s2:   u64,   // x18 — callee-saved
    s3:   u64,   // x19 — callee-saved
    s4:   u64,   // x20 — callee-saved
    s5:   u64,   // x21 — callee-saved
    s6:   u64,   // x22 — callee-saved
    s7:   u64,   // x23 — callee-saved
    s8:   u64,   // x24 — callee-saved
    s9:   u64,   // x25 — callee-saved
    s10:  u64,   // x26 — callee-saved
    s11:  u64,   // x27 — callee-saved
    t3:   u64,   // x28 — temporary
    t4:   u64,   // x29 — temporary
    t5:   u64,   // x30 — temporary
    t6:   u64,   // x31 — temporary
    mepc:     u64,  // machine exception program counter (CSR 0x341)
    mstatus:  u64,  // machine status register (CSR 0x300)
    scause:   u64,  // supervisor cause register (CSR 0x142)
    stval:    u64,  // supervisor trap value register (CSR 0x143)
}
```

Push order: trap_entry pushes x1-x31 (30 regs; x0 is hardwired zero so not
saved), then reads mepc, mstatus, scause, stval from CSRs.

### Field-name asymmetry

The arch-agnostic `trap.vuma` dispatcher reads `tf.vector` (x86_64) to
classify the trap. On aarch64, the equivalent is `tf.esr_el1`'s EC field
(bits 31-26); on riscv64, it's `tf.scause`. The K3a/K3d contracts document
this asymmetry — the dispatcher's per-arch re-declaration includes an
`trap_exception_class(tf)` helper that extracts the trap class from the
arch-specific register. Until K11+ unifies the field naming (or until VUMA
gains `import` and the dispatcher can call arch-specific helpers from a
shared module), each arch's trap.vuma is a separate file.

### Syscall-arg register convention

| Arch     | nr  | a0  | a1  | a2  | a3  | a4  | a5  | return |
|----------|-----|-----|-----|-----|-----|-----|-----|--------|
| x86_64   | rax | rdi | rsi | rdx | r10 | r8  | r9  | rax    |
| aarch64  | x8  | x0  | x1  | x2  | x3  | x4  | x5  | x0     |
| riscv64  | a7  | a0  | a1  | a2  | a3  | a4  | a5  | a0     |

x86_64 uses **r10** (not rcx) for `a3` — SYSCALL clobbers rcx with RFLAGS,
so the SysV AMD64 syscall ABI promotes rcx→r10. aarch64 and riscv64 use the
regular calling convention (no register shuffle).

---

## Per-Arch PTE Comparison

The three existing ports declare PTE bit-layout helpers operating on raw
u64. The bit positions and field vocabulary differ:

### x86_64 — Intel SDM Vol. 3, Fig. 4-19

```
    bit 0      Present (P)         — pte_present  → 1
    bit 1      Read/Write (R/W)    — pte_writable → 2
    bit 2      User/Supervisor     — pte_user     → 4
    bit 3      Page-WT (PWT)       → 8
    bit 4      Cache-Disable (PCD) → 16
    bit 5      Accessed (A)        → 32
    bit 6      Dirty (D)           → 64
    bit 7      Page-Size / PAT     → 128
    bits 12-51 Physical address    — pte_addr mask = 17592186028032
                                      (= 0x000FFFFFFFFFF000)
    bit 63     Execute-Disable (NX)— pte_no_exec  → 9223372036854775808
                                      (= 0x8000000000000000)

    Helpers: pte_make(paddr, flags) | pte_addr | pte_present | pte_writable
           | pte_user | pte_no_exec
```

4-level paging: PML4 → PDPT → PD → PT (4 KB pages, 512 entries per table,
9-bit indices per level).

### aarch64 — ARM ARM D8.2 (4 KB granule, 48-bit VA)

```
    bit 0      Valid               — pte_valid       → 1
    bit 1      Access flag         → 2
    bit 2      Memory attrs index  → 4
    bit 6      AP[1] (read/write)  → 64
    bit 7      AP[2] (EL0 access)  — pte_user         → 128
    bit 10     Not-XN (executable) → 1024
    bit 53     XN (no-exec)        — pte_no_exec       → bit 53 mask
    bits 12-47 Output address      — pte_addr mask (smaller than x86_64)

    Helpers: pte_make | pte_addr | pte_valid | pte_accessible
           | pte_no_exec | pte_user
```

Field-name vocabulary differs: `pte_valid` (not `pte_present`), `pte_accessible`
(no x86_64 equivalent), `pte_no_exec` (matches x86_64). 4-level paging: PGD →
PUD → PMD → PTE (4 KB granule, 9-bit indices, 48-bit VA).

### riscv64 — RISC-V Privileged ISA §4.3 (Sv39 / Sv48)

```
    bit 0      Valid (V)           — pte_valid       → 1
    bit 1      Readable (R)        — pte_readable     → 2
    bit 2      Writable (W)        — pte_writable     → 4
    bit 3      Executable (X)      — pte_executable   → 8
    bit 4      User (U)            — pte_user         → 16
    bit 6      Accessed (A)        → 64
    bit 7      Dirty (D)           → 128
    bits 10-17 RSW (reserved for SW)
    bits 12-53 PPN                 — pte_addr mask

    Helpers: pte_make | pte_addr | pte_valid | pte_readable
           | pte_writable | pte_executable | pte_user
```

3-level paging (Sv39): PGD → PMD → PTE (4 KB pages, 9-bit indices, 39-bit
VA). Sv48 (4-level, 48-bit VA) is an optional extension — the kernel
supports both via the `PageTable.levels` field read by `vmm.vuma::vmm_map`.

### Naming asymmetry

The arch-agnostic `vmm.vuma` only calls `pte_present` (the x86_64 name) on
the x86_64 path. On aarch64/riscv64 ports, the local re-declaration of the
helper is named `pte_valid` instead. K11+ will unify the naming (likely by
adding a `pte_present` alias on aarch64/riscv64 that calls `pte_valid`).

---

## Per-Arch Context Switch

The three existing ports declare Task with a shared 5-field header + 4-field
trailer and an arch-specific middle (the callee-saved register set):

### Common header (5 fields, 24 bytes)

```
    pid:      u32,    // process ID
    ppid:     u32,    // parent process ID
    state:    u8,     // RUNNING=1, READY=2, BLOCKED=3, ZOMBIE=4, FREE=0
    prio:     u8,     // priority (lower = higher priority)
    vruntime: u64,    // CFS virtual runtime (ns)
```

### Common trailer (4 fields, 32 bytes)

```
    mm_root:  u64,    // address-space root (PGD phys on x86_64, etc.)
    fs_root:  u64,    // VFS root dentry idx
    fds:      u64,    // FileTable idx
    next:     u64,    // next task in runqueue (256 = end of list)
```

### Arch-specific middle (callee-saved regs)

| Arch     | Callee-saved set               | Field count | Field names                          |
|----------|--------------------------------|-------------|--------------------------------------|
| x86_64   | rbx, rbp, r12, r13, r14, r15   | 6 + rsp/rip = 8 | rsp, rip, rbp, rbx, r12, r13, r14, r15 |
| aarch64  | x19-x29 (11 regs) + sp         | 12          | sp, pc, x19, x20, x21, x22, x23, x24, x25, x26, x27, x28, x29 |
| riscv64  | s0-s11 (12 regs) + sp          | 13          | sp, pc, s0, s1, s2, s3, s4, s5, s6, s7, s8, s9, s10, s11 |

The total Task layout sizes:

| Arch     | Header | Middle | Trailer | Total fields | Total bytes |
|----------|--------|--------|---------|--------------|-------------|
| x86_64   | 5      | 8      | 4       | 17           | 136         |
| aarch64  | 5      | 13     | 4       | 22           | 176         |
| riscv64  | 5      | 14     | 4       | 23           | 184         |

### Address-space switch extern

| Arch     | Extern         | asm body                       |
|----------|----------------|--------------------------------|
| x86_64   | `cr3_write`    | `mov cr3, rdi; ret`            |
| aarch64  | `ttbr0_write`  | `msr ttbr0_el1, x0; isb; ret`  |
| riscv64  | `satp_write`   | `csrw satp, a0; sfence.vma; ret` |

The scheduler calls the address-space switch extern **after** the
register-save/restore (so the new task's stack is in place when the MMU
switches). On x86_64, the order is: save prev regs → switch stack →
`cr3_write(next.mm_root)` → restore next regs → `ret`. The `cr3_write` step
is conditional: if `prev.mm_root == next.mm_root` (same process, different
thread), the switch is skipped.

---

## Per-Arch Syscall ABI

The kernel's `syscall/abi.vuma::syscall_args_from_frame` extracts the 7
syscall-arg fields (nr + a0..a5) from a TrapFrame. The field mapping is
per-arch:

### x86_64 (Linux syscall ABI = SysV AMD64 with rcx→r10)

```
    args.nr ← tf.rax    (syscall number)
    args.a0 ← tf.rdi    (1st arg)
    args.a1 ← tf.rsi    (2nd arg)
    args.a2 ← tf.rdx    (3rd arg)
    args.a3 ← tf.r10    (4th arg — r10 NOT rcx, SYSCALL clobbers rcx)
    args.a4 ← tf.r8     (5th arg)
    args.a5 ← tf.r9     (6th arg)
    // return value → tf.rax (via syscall_write_ret)
```

Entry: `syscall` instruction. The kernel stub at `arch/x86_64/trap.S`
(K11) pushes the TrapFrame and calls `trap_handler(tf)`. On return, it pops
the TrapFrame and executes `sysretq` (the syscall-return instruction).

### aarch64 (Linux AArch64 syscall ABI = AAPCS)

```
    args.nr ← tf.x8     (syscall number)
    args.a0 ← tf.x0     (1st arg)
    args.a1 ← tf.x1     (2nd arg)
    args.a2 ← tf.x2     (3rd arg)
    args.a3 ← tf.x3     (4th arg)
    args.a4 ← tf.x4     (5th arg)
    args.a5 ← tf.x5     (6th arg)
    // return value → tf.x0 (via syscall_write_ret, arch-specific)
```

Entry: `svc #0` instruction. The kernel stub pushes x0-x30 + ELR_EL1 +
SPSR_EL1 + ESR_EL1 into TrapFrame, calls `trap_handler(tf)`, on return pops
and executes `eret`.

### riscv64 (Linux RISC-V syscall ABI = standard RV calling convention)

```
    args.nr ← tf.a7     (syscall number)
    args.a0 ← tf.a0     (1st arg)
    args.a1 ← tf.a1     (2nd arg)
    args.a2 ← tf.a2     (3rd arg)
    args.a3 ← tf.a3     (4th arg)
    args.a4 ← tf.a4     (5th arg)
    args.a5 ← tf.a5     (6th arg)
    // return value → tf.a0 (via syscall_write_ret, arch-specific)
```

Entry: `ecall` instruction. The kernel stub pushes x1-x31 + mepc + mstatus
+ scause + stval into TrapFrame, calls `trap_handler(tf)`, on return pops
and executes `sret`.

### Syscall-number convention

- **x86_64** uses the legacy x86_64-native table (write=1, read=0,
  exit=60, mmap=9, ...).
- **aarch64** and **riscv64** use the asm-generic table verbatim (write=64,
  read=63, exit=93, mmap=222, ...).

The `womb/syscalls.vuma` reference uses asm-generic numbers for
documentation. The hosted x86_64 stubs use the x86_64-native numbers
because they're invoked via the `syscall` instruction.

---

## Testing Your Port

Once your per-arch files compile + IVE-pass standalone, you need to verify
they actually work at runtime. The testing strategy depends on whether
your port has a hosted path (x86_64-style) or is bare-metal-only
(aarch64/riscv64-style).

### Compiling a test

Pick a test from `tests/gold_standard/` — start with the simplest
self-tests in `tests/gold_standard/pmt_wave1/`:

```
    # Compile for the new arch:
    ./target/release-fast/compile_dump \
        tests/gold_standard/pmt_wave1/state_type.vuma \
        /tmp/state_type-<arch>.bin <arch> --verify
    # Expected: "IVE: Pass passed=1 failed=0 total=1"
```

Then compile the kernel itself:

```
    ./target/release-fast/compile_dump womb/kernel/kernel.vuma \
        /tmp/kernel-<arch>.bin <arch> --verify
```

### Running under QEMU user-mode

For non-x86_64 arches, you need QEMU user-mode emulators:

```
    # Install QEMU user-mode emulators (Debian/Ubuntu):
    sudo apt install qemu-user qemu-user-static

    # Run an aarch64 binary:
    qemu-aarch64 /tmp/state_type-aarch64.bin
    echo "exit=$?"

    # Run a riscv64 binary:
    qemu-riscv64 /tmp/state_type-riscv64.bin
    echo "exit=$?"
```

Each test file's header comment documents its expected exit code (usually
0 for a passing self-test). Match the exit code against the expected.

### Running the full kernel

```
    qemu-<arch> /tmp/kernel-<arch>.bin
    # Expected stdout: "vuma kernel: hello"
    # Expected exit: 0
```

If the kernel doesn't print the banner, the most likely cause is a missing
or mis-routed `write` extern. On hosted x86_64, `write` is a pre-registered
syscall stub; on other arches running under QEMU user-mode, the kernel
relies on the same Linux syscalls — but the syscall numbers differ per arch
(see [Per-Arch Syscall ABI](#per-arch-syscall-abi)).

### Debugging tips

1. **`--verify` first, then run.** Always compile with `--verify` and
   confirm `IVE: Pass` before trying to run. If IVE fails, the runtime
   behavior is undefined.

2. **Check for `flatten_expr` warnings.** Even if IVE passes, scan the
   compiler's stderr for `WARNING: unsupported FieldAccess (not
   state-typed)` lines. Each warning means a state.field access will
   silently return 0 at runtime. Fix the warning (usually by converting
   a return-style helper to init-style — see
   [`kernel-developer-guide.md`](./kernel-developer-guide.md) §6).

3. **Use `strace` / `ltrace` under QEMU.** QEMU user-mode supports
   `-strace`:
   ```
       qemu-aarch64 -strace /tmp/kernel-aarch64.bin
   ```
   This prints every syscall the kernel makes. If you see `write(1, ...)` →
   `= 19` (19 bytes for "vuma kernel: hello\n"), the syscall path works.

4. **Disassemble the binary.** Use `objdump -d /tmp/kernel-<arch>.bin` to
   inspect the generated asm. Look for the `mov eax, 1 ; syscall` pattern
   (x86_64 write stub) or the equivalent per-arch.

5. **Reduce the test.** If the full kernel doesn't boot, try the per-arch
   files standalone — `bootinfo.vuma` and `trampoline.vuma` have their own
   self-tests that don't depend on the rest of the kernel.

6. **Compare against x86_64.** The x86_64 port is the reference. If your
   port's `arch/<arch>/pt.vuma` round-trip self-test fails, diff your file
   against `arch/x86_64/pt.vuma` and check the mask constants.

7. **Check the worklog.** Search for `Task ID: K2c` (x86_64 pt.vuma), `K2d`
   (aarch64/riscv64 ports), `K3a` (all trap_trampoline.vuma), `K4c` (all
   switch.vuma). Each entry records the design decisions, contract
   deviations, and forward-looking notes for that port.

### Gold-standard test suite

The `tests/gold_standard/` directory contains the curated PMT test suite
organized by wave (see `tests/gold_standard/manifest.json` for the current
program count). After your port compiles + the kernel boots, run the parity
sweep:

```
    ./scripts/kernel_parity.sh --quick
    # Compiles arena_basic + kernel smoke across all 19 codegen backends
    # (7 executable via QEMU + wasmtime, 12 compile-only).
    # Exits 0 only if every backend passes.

    ./scripts/kernel_parity.sh
    # Full sweep — 10 gold-standard tests × every codegen backend + 19
    # kernel modules × 4 arch ports. Takes ~10 min.
```

The parity sweep uses QEMU user-mode for non-x86_64 arches. If QEMU is not
installed, the sweep skips that arch's runtime test but still verifies the
compile + IVE pass.

### Cross-arch differential testing

For tests that have known-answer semantics (the `womb_kat_tests/`
directory), you can compare the output across arches:

```
    # Compile a KAT test for x86_64 and aarch64:
    ./target/release-fast/compile_dump scripts/womb_kat_tests/test_sha256_empty.vuma \
        /tmp/sha-x86_64.bin x86_64 --verify
    ./target/release-fast/compile_dump scripts/womb_kat_tests/test_sha256_empty.vuma \
        /tmp/sha-aarch64.bin aarch64 --verify

    # Run both, compare exit codes:
    /tmp/sha-x86_64.bin; echo "x86_64 exit=$?"
    qemu-aarch64 /tmp/sha-aarch64.bin; echo "aarch64 exit=$?"
```

Both should produce the same exit code (0 for a KAT that passes). A
differential failure points to a codegen bug in one of the backends.

---

## Common Pitfalls

### The `_pad0` AtomicCas workaround

VUMA's `AtomicCas` intrinsic is hardcoded to `IRType::U64` (8 bytes). It
operates on bytes `[offset+0..offset+7]` of the target address — it does
NOT respect the field type at the offset you pass. If you have a layout
like:

```
    layout Spinlock = {
        locked: u32,   // offset 0
        holder: u32,   // offset 4 — INSIDE the 8-byte CAS window!
        depth:  u32,   // offset 8
        ...
    }
```

…and you call `atomic_cas(lock as Address, 0, 1)`, the CAS will compare
8 bytes (locked + holder) against 0, not just `locked`. After init
(locked=0, holder=256), the 8-byte value is `0x0000010000000000` — NOT 0 —
so the CAS always fails.

The fix is to insert a `_pad0: u32` field between `locked` and `holder`,
pushing `holder` outside the 8-byte CAS window:

```
    layout Spinlock = {
        locked: u32,   // offset 0 — CAS target word
        _pad0:  u32,   // offset 4 — padding (pushes holder out of CAS window)
        holder: u32,   // offset 8 — outside CAS window
        depth:  u32,   // offset 12
        ...
    }
```

After init (locked=0, _pad0=0), the 8-byte value at offset 0 is
`0x0000000000000000` — the CAS succeeds. The `_pad0` field is never read
or written by kernel code; it exists solely to make the CAS window
correct. See `womb/kernel/sync/spinlock.vuma` header "Why _pad0?" and
`womb/kernel/sync/mutex.vuma` header for the full derivation.

K13+ will fix this by adding a width-suffixed `atomic_cas_32` intrinsic
(or by making AtomicCas respect the field type at the offset).

### The `0 - 1` negative-literal workaround

VUMA's parser has a subtle width-extension bug in its negative-literal
path. The literal `-1` (intended as u64 -1 = 0xFFFFFFFFFFFFFFFF) is
sometimes misinterpreted as a signed i64 and sign-extended incorrectly.
The kernel's convention is to **always write negative numbers as `0 - N`**:

```
    // DON'T:
    return -1;          // parser's signed-literal path — risky

    // DO:
    return 0 - 1;       // flatten_expr's BinOp::Sub arm — verified safe
```

The `0 - N` form lowers to identical machine code (the codegen's constant
folder collapses it). See
[`kernel-architecture.md` §10.1](./kernel-architecture.md#101-the-0--1-negative-literal-workaround)
for the full rationale.

### The `no_struct_literal` trap

VUMA has no struct-literal syntax (`Layout { field: value, ... }`). You
cannot "construct" a state inline — every state must be allocated with
`state_new(Layout)` (zero-initialized) and then populated field-by-field:

```
    // DON'T (parse error — VUMA has no struct literal):
    fn make_task(pid: u32) -> State<Task> {
        return Task { pid: pid, state: 1, ... };
    }

    // DO (allocate-then-populate — init-style API):
    fn make_task(tbl: State<ProcessTable>, pid: u32) {
        let idx = task_alloc(tbl);
        pt_set_pid(tbl, idx, pid);
        pt_set_state(tbl, idx, 1);
    }
```

This is a language-design choice aligned with the PMT discipline (no
implicit allocation sites). See
[`kernel-architecture.md` §10.7](./kernel-architecture.md#107-the-no_struct_literal-trap).

### The decimal-constants discipline

VUMA's parser accepts `0x..` hex literals, but the hex path shares code
with the decimal path through `parse_int_radix`, which has subtle
width-extension behavior at the 64-bit boundary. The kernel's convention
is to **use decimal literals in self-tests** (`4096`, not `0x1000`;
`17592186028032`, not `0x000FFFFFFFFFF000`):

```
    // DON'T (hex literal — risky):
    let mask = 0x000FFFFFFFFFF000;

    // DO (decimal literal — verified safe):
    let mask = 17592186028032;
```

The decimal form lowers to identical machine code. The hex form is
acceptable in comments and prose for readability.

### The byte-identical re-declaration invariant

VUMA has no `import`. Every kernel module that uses another module's
layouts or externs must re-declare them **byte-identically** — same field
names, same types, same order. A single-byte difference (e.g. `u32` vs
`u64` for one field, or fields in different order) will silently produce
different offsets in different translation units, and the bug won't surface
until runtime.

The IVE verifiers catch *some* drift (if two files declare the same layout
name with different field offsets, the LayoutRegistry rejects it). But
they don't catch *all* drift — e.g. if you rename a field but keep the
type and order, the verifiers won't flag it (each file sees its own
declaration as correct). The kernel's convention is to copy-paste the
entire layout block from the canonical source.

### The init-style API requirement

The codegen does not propagate `State<T>`-typedness through function return
values. A binding `let s = make_state()` where `make_state` returns
`State<T>` is NOT registered as state-typed in the caller; subsequent
`s.field` accesses silently return 0 with a `flatten_expr` warning.

**Always use init-style**: the caller allocates the state via
`state_new(...)` and passes it by reference to a function that populates
it in place. See [`kernel-architecture.md` §3](./kernel-architecture.md#3-pmt-in-the-kernel-design)
and [`kernel-developer-guide.md` §4](./kernel-developer-guide.md#4-how-to-write-pmt-kernel-code).

### Forgetting `#[borrow]` on extern State<T> params

The codegen conservatively invalidates State<T> params after every extern
"C" call. If you call `pte_read(pt, ...)` and then read `pt.levels` in
the next line, the read trips the use-after-invalidate verifier — UNLESS
the extern declares `#[borrow]`:

```
    // DON'T (forgets #[borrow] — verifier failure on next pt access):
    extern "C" {
        fn pte_read(pt: State<PageTable>, level: u8, idx: u32) -> u64;
    }

    // DO (#[borrow] keeps pt alive after the call):
    extern "C" {
        #[borrow] fn pte_read(pt: State<PageTable>, level: u8, idx: u32) -> u64;
    }
```

Only extern "C" calls need `#[borrow]` — regular VUMA fn calls track State
lifetimes automatically.

### Mixing positive and negative errno conventions

Some kernel files return **positive** errno (`return 11;` for -EAGAIN —
the trap layer negates before writing to tf.rax). Others return **negative**
errno via `0 - N` (`return 0 - 11;`). The two conventions are
incompatible — a dispatcher expecting positive errno will mis-interpret a
negative return as a large positive success value.

Check the file header of the function you're calling: if it says "returns
positive errno", return positive. If it says "returns -errno", use `0 - N`.
The K11+ work will unify the convention (likely: positive errno
internally, trap layer negates uniformly). See
[`kernel-architecture.md` §13](./kernel-architecture.md#13-error-code-convention).

### Sentinel-value / table-size coupling

The kernel uses `256` as the "empty" sentinel for tables with 256 slots,
`64` as the "full" sentinel for tables with 64 slots, etc. The sentinel
value is **coupled** to the table size — if you change the table size
without changing the sentinel, the sentinel becomes a valid index and the
"empty"/"full" detection breaks.

When you add a new table, document the size + sentinel pair in the layout
comment. If you ever resize a table, search the whole file for the old
sentinel value and update it everywhere. See
[`kernel-architecture.md` §14](./kernel-architecture.md#14-sentinel-value-convention).

---

## Porting Checklist

Before declaring the port complete, tick every box:

- [ ] `womb/kernel/arch/<arch>/` directory exists.
- [ ] (Hosted ports only) `bootinfo.vuma` declares `BootInfo` +
      `bootinfo_init()`. IVE Pass, self-test exits 0.
- [ ] (Hosted ports only) `trampoline.vuma` declares `write`, `read`,
      `exit`, `mmap`, `munmap` (minimum). Header documents syscall numbers
      + calling convention. IVE Pass, self-test exits 0.
- [ ] (Bare-metal-only ports) `boot.S` parses the boot protocol and jumps
      to `kmain` (K11+ work; skip for now).
- [ ] `trap_trampoline.vuma` declares per-arch `TrapFrame` (x86_64=22
      fields, aarch64=35, riscv64=35) + `idt_load` / `irq_mask` /
      `irq_unmask` / `pic_eoi` / `cr2_read` (or arch equivalents).
      `trap_frame_init` zeroes all fields. IVE Pass, self-test exits 0.
- [ ] `switch.vuma` declares `Task` saved-register subset +
      `#[borrow] fn context_switch(prev, next)` + address-space-switch
      extern. `task_init_for_switch` populates `rip`/`rsp`. IVE Pass,
      self-test exits 0.
- [ ] `pt.vuma` declares `pte_make`/`pte_addr`/`pte_present`/`pte_writable`/
      `pte_user`/`pte_no_exec` (or arch equivalents). Round-trip self-test
      `pte_addr(pte_make(p, f)) == p` exits 0. IVE Pass.
- [ ] `mm_trampoline.vuma` re-declares `PageTable` byte-identically to
      `mm/vmm.vuma` + declares `#[borrow] pte_read` / `#[borrow] pte_write`
      / `tlb_flush` / `invlpg` / address-space-switch externs. IVE Pass,
      self-test exits 0.
- [ ] `womb/kernel/kernel.vuma` compiles + links + runs on the new arch
      (`/tmp/kernel-<arch>.bin` prints `vuma kernel: hello` and exits 0
      when run via QEMU user-mode).
- [ ] No `WARNING: unsupported FieldAccess (not state-typed)` from
      `flatten_expr` in any file.
- [ ] No pointer syntax (`*T`, `&x`, `allocate`, `free`) anywhere in the
      new files.
- [ ] All non-zero self-test constants are DECIMAL (not `0x..` hex) —
      decimal avoids the parser's hex-literal width-extension path (see
      `arch/x86_64/pt.vuma` header for the rationale).
- [ ] Every layout re-declaration is byte-identical to its canonical source
      (same field names, types, order).
- [ ] Every extern that takes `State<T>` params uses `#[borrow]` where the
      caller needs the state alive after the call (pte_read/pte_write/
      context_switch — see [`kernel-architecture.md` §6.4](./kernel-architecture.md#64-borrow-on-statet-extern-params)).
- [ ] No `_pad0`-less Spinlock/Mutex layout (the AtomicCas workaround —
      see [Common Pitfalls](#common-pitfalls) above).
- [ ] No `-N` negative literals — use `0 - N` form (see [Common Pitfalls](#common-pitfalls)).
- [ ] No struct-literal syntax (`Layout { ... }`) — use init-style API.
- [ ] Run `./scripts/kernel_parity.sh --quick` — your new arch passes.
- [ ] Commit with message `Wave K<NN>: <arch> port` (or extend the existing
      K-wave that introduced the arch).

Once all boxes are ticked, the port is feature-complete at the API-contract
level. K11+ (real asm trampolines) and K13+ (real drivers) build on this
foundation.
