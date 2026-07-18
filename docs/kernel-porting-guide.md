# VWK Kernel Porting Guide

This guide walks through porting the VWK (Vuma Womb Kernel) to a new CPU
architecture. The kernel currently ships per-arch files for **x86_64**,
**aarch64**, and **riscv64** under `womb/kernel/arch/<arch>/`. To add a fourth
architecture (e.g. loongarch64, ppc64le, s390x), follow the nine steps below.

The x86_64 port is the worked example throughout — it is the most mature port
(the hosted-mode build target since K1) and its `bootinfo.vuma`,
`trampoline.vuma`, `pt.vuma`, `mm_trampoline.vuma`, `trap_trampoline.vuma`, and
`switch.vuma` are the canonical templates. Copy them as your starting point.

The kernel's per-arch abstraction is documented in
[`kernel-architecture.md` §5](./kernel-architecture.md#5-per-architecture-abstraction);
read that section first if you haven't.

---

## Prerequisites

Before starting, confirm:

- The VUMA 2.0 compiler already emits code for the target arch. Run
  `./target/release-fast/compile_dump --help` and check that `<arch>` is
  listed. If not, add the backend first (out of scope for this guide — see
  `src/codegen/src/<arch>/` for the existing 19 backends and
  [`architecture.md` §7](./architecture.md#7-backends) for the backend
  contract).
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

You will populate it with six files over the next eight steps. The remaining
files (`mm/pmm.vuma`, `proc/scheduler.vuma`, `net/tcp.vuma`, ...) are arch-
agnostic and need no porting — they consume the per-arch interfaces you are
about to define.

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
read, exit, mmap, munmap — these cover `console.vuma` and `kmain.vuma`).
Document the calling convention. Document the syscall-number mapping (Linux
syscall numbers are per-arch — `__NR_write` is 1 on x86_64, 64 on aarch64,
64 on riscv64).

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
`TrapFrame` (RAX, RBX, ..., R15, vector, error_code, RIP, CS, RFLAGS, RSP,
SS) — the layout the trap_entry asm pushes and trap_exit pops. It declares:

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
`pt.vuma`'s header) is to use the bits-12-51 mask `17592186028032`
(`0x000FFFFFFFFFF000`) for both build and read.

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

Verify each of the six files compiles standalone:

```
    cd /home/z/vuma
    . "$HOME/.cargo/env"
    cargo build --profile release-fast --bin compile_dump

    for f in bootinfo trampoline trap_trampoline switch pt mm_trampoline; do
        ./target/release-fast/compile_dump \
            womb/kernel/arch/<arch>/$f.vuma \
            /tmp/$f.bin <arch> --verify
        /tmp/$f.bin ; echo "$f exit=$?"
    done
```

Each must print `IVE: Pass passed=1 failed=0 total=1` and exit 0.

Then verify the arch-agnostic kernel still compiles + links (it consumes your
new arch layer):

```
    ./target/release-fast/compile_dump womb/kernel/kernel.vuma \
        /tmp/kernel.bin <arch> --verify
    /tmp/kernel.bin ; echo "kernel exit=$?"
```

Expected stdout on hosted x86_64: `vuma kernel: hello`. Expected exit: 0.

Finally, run the full gold-standard test suite for any arch-specific tests:

```
    ls tests/gold_standard/ | grep <arch>
```

If any of the above fails, consult the file's header comment — every kernel
arch file documents its K-wave lineage, the contract it satisfies, and the
deviations it took. The worklog at `/home/z/my-project/worklog.md` records
the design decisions for each wave.

---

## Porting Checklist

Before declaring the port complete, tick every box:

- [ ] `womb/kernel/arch/<arch>/` directory exists.
- [ ] `bootinfo.vuma` declares `BootInfo` + `bootinfo_init()`. IVE Pass,
      self-test exits 0.
- [ ] `trampoline.vuma` declares `write`, `read`, `exit`, `mmap`, `munmap`
      (minimum). Header documents syscall numbers + calling convention.
      IVE Pass, self-test exits 0.
- [ ] `trap_trampoline.vuma` declares per-arch `TrapFrame` (22+ fields)
      + `idt_load` / `irq_mask` / `irq_unmask` / `pic_eoi` / `cr2_read`
      (or arch equivalents). `trap_frame_init` zeroes all fields. IVE Pass,
      self-test exits 0.
- [ ] `switch.vuma` declares `Task` saved-register subset +
      `#[borrow] fn context_switch(prev, next)` + address-space-switch
      extern. `task_init_for_switch` populates `rip`/`rsp`. IVE Pass,
      self-test exits 0.
- [ ] `pt.vuma` declares `pte_make`/`pte_addr`/`pte_present`/`pte_writable`/
      `pte_user`/`pte_no_exec`. Round-trip self-test
      `pte_addr(pte_make(p, f)) == p` exits 0. IVE Pass.
- [ ] `mm_trampoline.vuma` re-declares `PageTable` byte-identically to
      `mm/vmm.vuma` + declares `#[borrow] pte_read` / `#[borrow] pte_write`
      / `tlb_flush` / `invlpg` / address-space-switch externs. IVE Pass,
      self-test exits 0.
- [ ] `womb/kernel/kernel.vuma` compiles + links + runs on the new arch
      (`/tmp/kernel.bin` prints `vuma kernel: hello` and exits 0).
- [ ] No `WARNING: unsupported FieldAccess (not state-typed)` from
      `flatten_expr` in any file.
- [ ] No pointer syntax (`*T`, `&x`, `allocate`, `free`) anywhere in the
      new files.
- [ ] All non-zero self-test constants are DECIMAL (not `0x..` hex) —
      decimal avoids the parser's hex-literal width-extension path (see
      `arch/x86_64/pt.vuma` header for the rationale).
- [ ] Every layout re-declaration is byte-identical to its canonical source
      (same field names, types, order).
- [ ] Commit with message `Wave K<NN>: <arch> port` (or extend the existing
      K-wave that introduced the arch).

Once all boxes are ticked, the port is feature-complete at the API-contract
level. K11+ (real asm trampolines) and K13+ (real drivers) build on this
foundation.
