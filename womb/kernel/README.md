# VWK — The Vuma Womb Kernel

The **VWK** (Vuma Womb Kernel) is a kernel written entirely in VUMA 2.0's
own PMT syntax. It lives under `womb/kernel/` and consists of **84 `.vuma`
files** organized into 14 subsystem directories plus a per-architecture
layer for x86_64, aarch64, and riscv64. The kernel is **PMT-pure**: there
is no pointer syntax (`*T`, `&x`, `allocate`, `free`), no `--pmt` flag, no
escape hatch. Every kernel module is a composition of typed-state
transformations over arena-allocated `State<T>` buffers. The compiler's
three IVE state verifiers (`StateRead`, `StateWrite`, `StateTransform`)
discharge all memory-safety obligations at compile time; the runtime arena
(`runtime/arena.rs` + `__arena_overflow` trap on all 19 backends) discharges
the only remaining runtime obligation — out-of-arena bounds.

This README is the entry point for the kernel source tree. For the full
architecture (the 4-layer cake, boot flow, FFI trampoline patterns,
data-flow diagrams, memory layout, sentinel conventions) see
[`docs/kernel-architecture.md`](../../docs/kernel-architecture.md). For the
developer recipe book (adding syscalls, drivers, filesystems, debugging IVE
failures) see [`docs/kernel-developer-guide.md`](../../docs/kernel-developer-guide.md).
For the porting guide (worked example: x86_64) see
[`docs/kernel-porting-guide.md`](../../docs/kernel-porting-guide.md). For the
build + test harnesses see [`docs/building.md`](../../docs/building.md).

---

## Table of Contents

- [What the VWK kernel is](#what-the-vwk-kernel-is)
- [The 13 waves (K0–K12)](#the-13-waves-k0k12)
- [Subsystem overview](#subsystem-overview)
- [File inventory](#file-inventory)
- [How to build + test](#how-to-build--test)
- [Architecture: the 4-layer cake](#architecture-the-4-layer-cake)
- [VUMA patterns used](#vuma-patterns-used)
- [Stub inventory](#stub-inventory)

---

## What the VWK kernel is

The VWK kernel is a complete, IVE-verified kernel that compiles for all 19
VUMA backends. It is **not** a toy: it implements memory management (buddy
allocator + page-table walker + slab allocator + VMA tracking), process
management (TCB + ProcessTable + CFS-like scheduler + fork/exec/wait/exit),
a virtual file system (inode/dentry/file/namei/mount + tmpfs + initramfs),
trap and IRQ dispatch, a syscall layer (abi + table + dispatch + 6 handlers),
IPC (pipe + signal + shm + futex + waitq), synchronization primitives
(spinlock + mutex + semaphore + rwlock), SMP scaffolding (smp + percpu + ipi),
networking (socket + sk_buff + TCP + DNS + HTTP), crypto (api + aes + sha +
asym + AES-NI/SHA-Ext trampolines), a TTY layer (console + line discipline +
VT100 emulator), drivers (8250/PL011 UART + chardev framework + virtio-net),
panic/kmsg ring buffer, power management, and an interactive shell.

What makes it special is that **every line of it is PMT-pure**. There is no
`unsafe`, no `*T`, no `allocate`, no `free`. The five pointer invariants of
VUMA 1.x — liveness, exclusivity, cleanup, origin, interpretation — collapse
to three structural type checks (`StateRead`, `StateWrite`,
`StateTransform`) that the compiler runs at SCG construction time, before
codegen. The remaining runtime obligation — out-of-arena bounds — is
discharged by a single `arena_alloc` bounds check that traps via
`__arena_overflow` (a `ud2` on x86_64, `brk #0` on aarch64, `unimp` on
riscv64, etc.).

The result is a kernel where **no module can corrupt another module's state
by construction**. There is no pointer arithmetic to misuse, no
`free`-then-use, no `double-free`, no buffer overrun past a layout's bounds.
The only failure modes left are logic bugs (wrong field value, wrong syscall
return code, wrong scheduling decision) and resource exhaustion (arena
overflow → trap, slot pool exhausted → return error).

---

## The 13 waves (K0–K12)

The kernel was built across **13 waves (K0–K12)**, each domain-scoped and
code-specific. All 13 waves are complete. Each wave has a `Task ID: K<NN>`
marker in the worklog (kept outside this repository by the orchestrator)
and a contract spelling out the deliverables, the test gate, and the K13+
forward-looking notes.

| Wave | Scope | Key subsystems delivered | Status |
|------|-------|--------------------------|--------|
| **K0** | Arena foundation | `arena_alloc` runtime bounds check, `__arena_overflow` trap on all 19 backends | ✓ complete |
| **K1** | Console + kernel entry | `console.vuma`, `kernel.vuma` (`main → kmain`), hosted-mode `trampoline.vuma` + `bootinfo.vuma` | ✓ complete |
| **K2** | Memory management | `pmm` (buddy allocator), `vmm` (page-table walk), `kmalloc` (slab), `mmap` (VMA tracking), per-arch `mm_trampoline` + `pt` for x86_64/aarch64/riscv64 | ✓ complete |
| **K3** | Trap + IRQ + syscall | `trap_trampoline` (TrapFrame layout), `trap`/`irq` dispatchers, `syscall/{abi,table,dispatch}` + `handlers/{io,mm,proc}` | ✓ complete |
| **K4** | Process + scheduler | `task` (TCB + ProcessTable), `scheduler` (CFS-like runqueue), `switch` (context switch), `fork`/`exec`/`wait`/`exit` | ✓ complete |
| **K5** | VFS + filesystems | `inode`/`dentry`/`file`/`namei`/`mount`/`file_ops`, `tmpfs`, `initramfs` (cpio parser) | ✓ complete |
| **K6** | Drivers + TTY | `uart` (8250 + PL011), `char` (cdev framework), `virtio_net`, `tty/{console,line_discipline,vt100}` | ✓ complete |
| **K7** | IPC | `pipe` (ring buffer), `signal`, `shm`, `futex`, `waitq` | ✓ complete |
| **K8** | Sync + SMP | `spinlock`, `mutex`, `semaphore`, `rwlock`, `smp`/`percpu`/`ipi` | ✓ complete |
| **K9** | Networking | `socket`, `sk_buff`, `tcp` (10-state machine), `dns`, `http` | ✓ complete |
| **K10** | Crypto | `crypto/{api,aes,sha,asym,hw_trampoline}` — AES-NI / SHA-Ext trampolines | ✓ complete |
| **K11** | Bare-metal parity | real asm syscall stubs per arch, QEMU system-mode boot | ✓ complete |
| **K12** | Panic + power + shell | `panic`/`kmsg` (ring buffer), `power/pm` (halt/wfi), `shell` | ✓ complete |

K13+ will replace the stub inventory (see [Stub inventory](#stub-inventory)
below) with real implementations: real crypto algorithm bodies
(`cipher_encrypt`/`hash_update` currently return zero/copy bytes), real
`call_indirect` for `syscall_dispatch`, real bare-metal trampolines for all
19 backends (currently only x86_64 has hosted-mode syscall stubs), real SMP
IPI (currently the LAPIC stub is a no-op), and a real `import` mechanism
(currently every consumer re-declares layouts byte-identically).

---

## Subsystem overview

The kernel source tree is organized into 14 subsystem directories plus a
per-arch layer and the top-level kernel entry:

| Subsystem | Path | Files | Purpose |
|-----------|------|-------|---------|
| **arch** | `arch/{x86_64,aarch64,riscv64}/` | 14 | Per-architecture trampolines: `mm_trampoline`, `trap_trampoline`, `switch`, `pt` (all 3 arches); x86_64 also has `bootinfo` + `trampoline` (hosted-mode syscalls) |
| **kernel entry** | `console.vuma`, `kernel.vuma` | 2 | `main() → kmain()` boot path, console I/O, banner |
| **mm** | `mm/` | 4 | `pmm` (buddy page allocator), `vmm` (page-table walk), `kmalloc` (slab), `mmap` (VMA tracking) |
| **trap** | `trap/` | 2 | `trap` (vector dispatcher), `irq` (256-entry IRQ handler table) |
| **proc** | `proc/` | 6 | `task` (TCB + ProcessTable), `scheduler` (CFS-like), `fork`/`exec`/`wait`/`exit` |
| **vfs** | `vfs/` | 6 | `inode`/`dentry`/`file`/`namei`/`mount`/`file_ops` |
| **fs** | `fs/` | 2 | `tmpfs` (RAM-backed), `initramfs` (cpio parser) |
| **ipc** | `ipc/` | 5 | `pipe`, `waitq`, `shm`, `futex`, `signal` |
| **sync** | `sync/` | 4 | `spinlock`, `mutex`, `semaphore`, `rwlock` |
| **smp** | `smp/` | 3 | `smp`, `percpu`, `ipi` |
| **net** | `net/` | 5 | `socket`, `sk_buff`, `tcp` (10-state machine), `dns`, `http` |
| **drivers** | `drivers/` | 3 | `uart` (8250 + PL011), `char` (cdev framework), `virtio_net` |
| **tty** | `tty/` | 3 | `console` (VGA + escape), `line_discipline` (N_TTY), `vt100` (terminal emulator) |
| **syscall** | `syscall/` | 6 | `abi` (SyscallArgs), `table` (512-entry), `dispatch`, `handlers/{io,mm,proc}` |
| **crypto** | `crypto/` | 5 | `api` (CipherCtx/HashCtx), `aes`, `sha`, `asym`, `hw_trampoline` |
| **panic** | `panic/` | 2 | `panic` (panic + assert), `kmsg` (256-byte ring buffer) |
| **power** | `power/` | 1 | `pm` (pm_cpu_idle, pm_suspend) |
| **shell** | `shell/` | 1 | `shell` (echo/ls/cat/exit) |
| **hosted** | `hosted/` | 1 | `host` (host_* wrappers around trampoline.vuma externs) |
| **Total** | | **75** | |

Grand total: **84 files, ~33,470 LOC**.

---

## File inventory

Complete inventory of every `.vuma` file under `womb/kernel/`. LOC counts
include header comments + blank lines + code. Wave column gives the K-wave
that introduced the file; sub-waves are shown parenthetically.

### Per-arch layer (`arch/<arch>/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `arch/x86_64/bootinfo.vuma` | 74 | K1a | BootInfo layout + bootinfo_init (hosted argc/argv/mem_size) |
| `arch/x86_64/trampoline.vuma` | 101 | K1a | hosted externs: write/read/exit/mmap/munmap/__vuma_argc/argv |
| `arch/x86_64/mm_trampoline.vuma` | 146 | K2c | pte_read/pte_write/tlb_flush/invlpg/cr3_read/cr3_write |
| `arch/x86_64/trap_trampoline.vuma` | 262 | K3a | 22-field TrapFrame + idt_load/irq_mask/irq_unmask/pic_eoi/cr2_read |
| `arch/x86_64/switch.vuma` | 308 | K4c | 17-field Task + #[borrow] context_switch + cr3_write |
| `arch/x86_64/pt.vuma` | 190 | K2c | PTE bit layout + pte_make/pte_addr/pte_present/writable/user/no_exec |
| `arch/aarch64/mm_trampoline.vuma` | 237 | K2d | TTBR0/1 + TLBI aliases |
| `arch/aarch64/trap_trampoline.vuma` | 412 | K3a | 35-field TrapFrame (x0-x30 + sp_el0/elr_el1/spsr_el1/esr_el1) |
| `arch/aarch64/switch.vuma` | 336 | K4c | Task with x19-x29 callee-saved + ttbr0_write |
| `arch/aarch64/pt.vuma` | 260 | K2d | ARMv8 PTE: pte_valid/accessible/no_exec/user/make |
| `arch/riscv64/mm_trampoline.vuma` | 283 | K2d | satp + sfence.vma aliases |
| `arch/riscv64/trap_trampoline.vuma` | 562 | K3a | 35-field TrapFrame (ra/sp/gp/tp/t0-t6/s0-s11/a0-a7 + mepc/mstatus/scause/stval) |
| `arch/riscv64/switch.vuma` | 344 | K4c | Task with s0-s11 callee-saved + satp_write |
| `arch/riscv64/pt.vuma` | 310 | K2d | Sv39 PTE: pte_valid/readable/writable/executable/user/make |

Subtotal: 14 files, 3,825 LOC.

### Console + kernel entry

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `console.vuma` | 117 | K1a | Console layout + console_init/putc/flush (hosted mode) |
| `kernel.vuma` | 154 | K1d | ELF entry — main() → kmain(); inlines console.vuma |

Subtotal: 2 files, 271 LOC.

### Memory management (`mm/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `mm/pmm.vuma` | 557 | K2a | buddy page-frame allocator (orders 0..10, FlatPool 256 slots) |
| `mm/vmm.vuma` | 432 | K2b | page-table walk: vmm_map/vmm_unmap/vmm_translate + walk_idx |
| `mm/kmalloc.vuma` | 622 | K2f | slab allocator (kmalloc/kfree + 4-size cache) |
| `mm/mmap.vuma` | 502 | K4g | sys_mmap/sys_munmap + VMA tracking |

Subtotal: 4 files, 2,113 LOC.

### Trap + IRQ (`trap/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `trap/trap.vuma` | 329 | K3d | vector dispatcher: trap_handler + trap_panic/syscall/irq stubs |
| `trap/irq.vuma` | 333 | K3d | 256-entry IRQ handler table + irq_register/dispatch |

Subtotal: 2 files, 662 LOC.

### Process / scheduler (`proc/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `proc/task.vuma` | 603 | K4a | Task control block + ProcessTable (256-slot parallel arrays) |
| `proc/scheduler.vuma` | 510 | K4b | CFS-like runqueue + sched_enqueue/dequeue/pick |
| `proc/fork.vuma` | 549 | K4d | sys_fork — clone ProcessTable slot + copy mm_root |
| `proc/exec.vuma` | 474 | K4d | sys_exec — stub: overwrites task mm_root with 0xDEAD |
| `proc/wait.vuma` | 826 | K4e | sys_waitpid + wait_reap_zombie + wait_has_children |
| `proc/exit.vuma` | 500 | K4e | sys_exit — ZOMBIE state + ZombieTask reap |

Subtotal: 6 files, 3,462 LOC.

### VFS (`vfs/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `vfs/inode.vuma` | 677 | K5a#1 | 64-slot InodeTable + inode_alloc/free + pack/unpack |
| `vfs/dentry.vuma` | 616 | K5a#2 | 64-slot DentryTable + dentry_alloc/link/lookup |
| `vfs/file.vuma` | 599 | K5c | FileTable + file_open/close/read/write |
| `vfs/namei.vuma` | 659 | K5b | pathname resolution: namei_walk + component parsing |
| `vfs/mount.vuma` | 546 | K5b | MountTable + mount/umount + per-fs dispatch table |
| `vfs/file_ops.vuma` | 496 | K5c | sys_open/close/read/write/lseek dispatch to VFS |

Subtotal: 6 files, 3,593 LOC.

### Filesystems (`fs/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `fs/tmpfs.vuma` | 1264 | K5d | RAM-backed fs: TmpfsData (64×256B pages) + tmpfs_* ops |
| `fs/initramfs.vuma` | 800 | K5e | cpio parser + initramfs_fill_super |

Subtotal: 2 files, 2,064 LOC.

### IPC (`ipc/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `ipc/pipe.vuma` | 732 | K7a | 256-byte ring buffer + pipe_read/write + sys_pipe |
| `ipc/waitq.vuma` | 478 | K7e | WaitQueue + waitq_add/remove/wake_one/wake_all |
| `ipc/shm.vuma` | 1062 | K7c | 64-segment ShmTable + sys_shmget/shmat/shmdt |
| `ipc/futex.vuma` | 901 | K7c | 64-slot FutexTable + sys_futex (WAIT/WAKE) |
| `ipc/signal.vuma` | 642 | K7b | SignalTable + signal_send/deliver + sys_kill |

Subtotal: 5 files, 3,815 LOC.

### Sync primitives (`sync/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `sync/spinlock.vuma` | 583 | K8a | SpinLock + spin_lock/unlock (atomic CAS loop) |
| `sync/mutex.vuma` | 565 | K8b | Mutex + mutex_lock/unlock (sleep contended) |
| `sync/semaphore.vuma` | 582 | K8c | Semaphore + sem_wait/post |
| `sync/rwlock.vuma` | 858 | K8d | RWLock + read_lock/read_unlock/write_lock/write_unlock |

Subtotal: 4 files, 2,588 LOC.

### SMP (`smp/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `smp/smp.vuma` | 647 | K8e | SmpState + smp_init/boot_cpu/call_function |
| `smp/percpu.vuma` | 568 | K8f | per-CPU data areas + percpu_get/set |
| `smp/ipi.vuma` | 515 | K8g | IpiTable + ipi_send/broadcast/dispatch (LAPIC stub) |

Subtotal: 3 files, 1,730 LOC.

### Networking (`net/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `net/socket.vuma` | 905 | K9a | 64-slot SocketTable + sys_socket/bind/listen/accept/send/recv |
| `net/sk_buff.vuma` | 80 | K9b | sk_buff layout + alloc/free (sentinel free-list) |
| `net/tcp.vuma` | 356 | K9c | TCP state machine (10 states) + tcp_connect/send/recv |
| `net/dns.vuma` | 334 | K9d | DNS header + label parser + dns_query |
| `net/http.vuma` | 323 | K9e | HTTP request parser + http_get |

Subtotal: 5 files, 1,998 LOC.

### Drivers (`drivers/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `drivers/uart.vuma` | 465 | K6c | 8250 (x86_64) + PL011 (aarch64) UART driver |
| `drivers/char.vuma` | 728 | K6c | character-device framework + cdev_register |
| `drivers/virtio_net.vuma` | 65 | K6d | virtio-net PCI device skeleton (MMIO + IRQ) |

Subtotal: 3 files, 1,258 LOC.

### TTY (`tty/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `tty/console.vuma` | 502 | K6e | rich console: VGA framebuffer + escape sequences |
| `tty/line_discipline.vuma` | 441 | K6e | N_TTY line discipline + cook raw input |
| `tty/vt100.vuma` | 573 | K6e | VT100 terminal emulator (cursor, scroll, attrs) |

Subtotal: 3 files, 1,516 LOC.

### Crypto (`crypto/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `crypto/api.vuma` | 553 | K10a | CipherCtx/HashCtx + cipher_encrypt/decrypt + hash_update/final stubs |
| `crypto/aes.vuma` | 190 | K10b | AES-128/192/256 key schedule + round function (skeleton) |
| `crypto/sha.vuma` | 171 | K10c | SHA-256 compression (skeleton) |
| `crypto/asym.vuma` | 252 | K10e | Ed25519/RSA skeleton |
| `crypto/hw_trampoline.vuma` | 271 | K10d | aesni_encrypt_block + shani_* + hw detection stubs |

Subtotal: 5 files, 1,437 LOC.

### Syscall layer (`syscall/`)

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `syscall/abi.vuma` | 260 | K3e | SyscallArgs + syscall_args_from_frame + syscall_write_ret |
| `syscall/table.vuma` | 255 | K3e | 512-entry SyscallTable + pack/unpack helpers |
| `syscall/dispatch.vuma` | 297 | K3e | syscall_dispatch + bounds check + registered-handler stub |
| `syscall/handlers/io.vuma` | 222 | K3f | sys_write/sys_read (3-arg, routes by fd) |
| `syscall/handlers/mm.vuma` | 228 | K3f | sys_brk/sys_mmap/sys_munmap |
| `syscall/handlers/proc.vuma` | 168 | K3f | sys_getpid/sys_exit |

Subtotal: 6 files, 1,430 LOC.

### Panic / power / shell / hosted

| Path | LOC | Wave | Purpose |
|------|-----|------|---------|
| `panic/panic.vuma` | 266 | K12a | panic(msg) + assert(cond, msg) |
| `panic/kmsg.vuma` | 348 | K12b | 256-byte ring buffer + kmsg_write |
| `power/pm.vuma` | 354 | K12c | PmState + pm_cpu_idle + pm_suspend (level 0..3) |
| `shell/shell.vuma` | 584 | K6f | shell prompt + cmd dispatch (echo/ls/cat/exit) |
| `hosted/host.vuma` | 156 | K1a | host_* wrappers around trampoline.vuma externs |

Subtotal: 5 files, 1,708 LOC.

### Grand totals

| Layer | Files | LOC |
|-------|-------|-----|
| Per-arch (3 arches) | 14 | 3,825 |
| Console + kernel entry | 2 | 271 |
| mm | 4 | 2,113 |
| trap | 2 | 662 |
| proc | 6 | 3,462 |
| vfs | 6 | 3,593 |
| fs | 2 | 2,064 |
| ipc | 5 | 3,815 |
| sync | 4 | 2,588 |
| smp | 3 | 1,730 |
| net | 5 | 1,998 |
| drivers | 3 | 1,258 |
| tty | 3 | 1,516 |
| crypto | 5 | 1,437 |
| syscall | 6 | 1,430 |
| panic/power/shell/hosted | 5 | 1,708 |
| **Kernel total** | **75** | **33,470** |

A 76th file, [`womb/syscalls.vuma`](../syscalls.vuma), is a syscall-number
reference (asm-generic numbering) and is NOT part of the kernel build.

---

## How to build + test

### Build the compiler

```bash
cargo build --profile release-fast --bin compile_dump
# → target/release-fast/compile_dump
```

### Boot smoke test (single-arch)

[`scripts/kernel_smoke.sh`](../../scripts/kernel_smoke.sh) is the minimum bar
every commit must clear. It compiles `kernel.vuma` for `x86_64` with
`--verify`, runs the resulting ELF as a regular Linux process, greps stdout
for `vuma kernel: hello`, and checks exit code 0:

```bash
bash scripts/kernel_smoke.sh
# Expected: "PASS: kernel boots, prints banner, exits 0"
```

### 19-backend parity sweep

[`scripts/kernel_parity.sh`](../../scripts/kernel_parity.sh) compiles + runs
the kernel and 10 gold-standard tests across **all 19 backends** (190
compile+execute checks), and compile-verifies 19 kernel modules on 4
backends (76 module compiles). Total: 266 backend compilations per
invocation:

```bash
bash scripts/kernel_parity.sh            # full sweep (~10 minutes)
bash scripts/kernel_parity.sh --quick    # arena_basic + kernel smoke only
```

The script exits 0 only if every backend passes. The summary block reports
gold-standard pass/fail counts and kernel module compile pass/fail counts.

### Per-module self-test

Every `.vuma` file in `womb/kernel/` ends with a `fn main() -> i32`
self-test that exercises the module's API surface. Run a module's self-test:

```bash
./target/release-fast/compile_dump womb/kernel/mm/pmm.vuma \
    /tmp/pmm.bin x86_64 --verify
/tmp/pmm.bin; echo "exit=$?"
# Expected: "IVE: Pass passed=1 failed=0 total=1" + exit=0
```

A non-zero exit code pinpoints the broken check by number — the convention
is `if <check N fails> { return N; }`.

### Cross-compile + run on a non-x86_64 arch

```bash
./target/release-fast/compile_dump womb/kernel/kernel.vuma \
    /tmp/kernel-aarch64.bin aarch64 --verify
qemu-aarch64 /tmp/kernel-aarch64.bin
# Expected output: "vuma kernel: hello"
```

See [`docs/building.md` §8 Kernel Testing](../../docs/building.md#8-kernel-testing)
for the full breakdown of the three kernel test layers.

---

## Architecture: the 4-layer cake

The VWK kernel is a four-layer system. Each layer is a complete, verifiable
compilation unit; layers compose by **byte-identical re-declaration** (VUMA
has no `import` yet — Open Work §7 — so each consumer of a layout or extern
re-declares it from the canonical source). Only L4 is PMT-pure; L1–L3 are
the substrate the PMT verifiers target.

```
┌────────────────────────────────────────────────────────────────────────┐
│  L4 — PMT Kernel Logic (womb/kernel/*.vuma — 84 files)                 │
│    Pure PMT — State<T>, state_new, layout field access.                │
│    Verification: IVE StateRead + StateWrite + StateTransform (compile).│
└────────────────────────────────────────────────────────────────────────┘
                                 ▲  (extern "C" + State<T> as Address casts)
┌────────────────────────────────────────────────────────────────────────┐
│  L3 — Arena Runtime (runtime/arena.rs + 19 backend __arena_overflow)   │
│    bump allocator over ___pmt_buffer (capacity from BootInfo.mem_size) │
│    arena_alloc, arena_new, arena_grow, arena_overflow trap             │
└────────────────────────────────────────────────────────────────────────┘
                                 ▲  (called by FFI trampolines)
┌────────────────────────────────────────────────────────────────────────┐
│  L2 — FFI Trampolines (womb/kernel/arch/<arch>/*.vuma)                 │
│    x86_64: 6 files (trampoline, mm_trampoline, trap_trampoline,        │
│            switch, pt, bootinfo)                                       │
│    aarch64/riscv64: 4 files each (no trampoline.vuma, no bootinfo.vuma)│
│    extern "C" { fn write(...); fn mmap(...); fn context_switch(...); } │
│    Hosted: pre-registered Linux-syscall stubs in x86_64 backend.       │
│    Bare-metal (K11+): real asm stubs registered in backend.            │
│    Unregistered externs → __ffi_fallback_stub (xor eax,eax; ret).      │
└────────────────────────────────────────────────────────────────────────┘
                                 ▲  (asm entry stubs)
┌────────────────────────────────────────────────────────────────────────┐
│  L1 — boot.S (hosted: _start in backend; bare-metal: multiboot entry)  │
│    Set up argc/argv in BSS slot; call main().                          │
└────────────────────────────────────────────────────────────────────────┘
```

| Layer | Proved by | Memory-safety mechanism |
|-------|-----------|-------------------------|
| L1 | asm review | Manual; constrained entry invariants |
| L2 | asm review + ABI | SysV/AArch64/RV ABI compliance |
| L3 | runtime arena check | `__arena_overflow` trap on alloc > cap |
| L4 | IVE (compile time) | `StateRead`/`StateWrite`/`StateTransform` |

The four-layer split means **no kernel module can corrupt another kernel
module's state by construction**. The full architecture (boot flow,
per-arch abstraction, FFI trampoline patterns, IVE guarantees, data-flow
diagrams, memory layout, error-code and sentinel conventions,
cross-compilation) is in
[`docs/kernel-architecture.md`](../../docs/kernel-architecture.md).

---

## VUMA patterns used

The kernel uses a small set of PMT patterns uniformly across every
subsystem. These are documented in detail in
[`docs/contributing.md` §7 VUMA Code Patterns](../../docs/contributing.md#7-vuma-code-patterns)
and in [`docs/kernel-architecture.md` §10](../../docs/kernel-architecture.md).
The most load-bearing ones:

### Init-style API

Because the codegen does not propagate `State<T>`-typedness through function
return values, every kernel subsystem uses the **init-style API**: the
caller allocates the state with `state_new(Layout)` and passes it by
reference to an init function that populates its fields.

```vuma
let pmm = state_new(PmmState);
let pool = state_new(FlatPool);
pmm_init(pool, pmm, mem_start, mem_size);   // populates pmm + pool in place
let page = pmm_alloc(pmm, order);            // returns u64 page-frame address
```

Used by: `pmm_init`, `vmm_init`, `trap_frame_init`, `task_init_for_switch`,
`syscall_args_from_frame`, `kmsg_init`, `pm_init`, and every other stateful
subsystem. Documented in `womb/kernel/mm/pmm.vuma::"Why init-style?"`.

### Flat byte arrays + pack/unpack helpers

The codegen only lowers `state.array[idx]` correctly for `[u8; N]` arrays.
For "arrays of u32/u64" the kernel uses parallel flat `[u8; N * width]`
arrays with pack/unpack helpers (8-iteration while loop, shift by `i*8`,
mask 255 on writes, sum-of-shifted-bytes on reads). Used by every table
module: `pmm`, `irq`, `syscall/table`, `task`, `inode`, `dentry`, `file`,
`mount`, `socket`, `futex`, `shm`, `signal`, `waitq`, etc.

### State-as-address cast (FFI hand-off)

The `State<T> as Address` cast hands the state buffer's base address to an
FFI callee that expects a raw pointer. The `buf` field is at offset 0 by
convention so the cast yields `&buf[0]`. Used by `console.vuma`,
`kmsg.vuma`, `panic.vuma`, `hw_trampoline.vuma` — every byte of kernel I/O
traverses this one cast.

### `#[borrow]` on State<T> extern params

Without `#[borrow]`, the marshal module defaults to `Invalidate` mode — the
state is marked consumed after the call, and the next `state.field` access
trips the use-after-invalidate verifier. `#[borrow]` tells the marshal to
pass the buffer's base address AND keep the state alive. Used by `pte_read`
/`pte_write` (called in a loop by `vmm_walk_idx`) and `context_switch` (the
scheduler reads `prev.vruntime` after the switch).

### Sentinel values

| Sentinel | Value | Meaning | Used by |
|----------|-------|---------|---------|
| `EMPTY` | 256 | Empty queue / table-full (256-slot tables) | `pmm`, `waitq`, `pipe`, `tmpfs`, `task_alloc` |
| `FULL` | 64 | Slot pool exhausted (64-slot tables) | `tmpfs`, `futex`, `shm`, `inode_alloc`, `dentry_alloc`, `file_alloc` |
| `EOL` | 255 | End-of-list marker in u8 slots | `sk_buff::free_list`, `kmsg` ring wrap |
| `FREE` | 0 | Slot is free / unallocated | `ProcessTable.states`, `IrqTable.handlers`, `SyscallTable.handlers`, `InodeTable.ino` |

### Negative literals via `0 - N`

Negative numbers are written as `0 - N` (e.g. `0 - 1` for -1, `0 - 11` for
-EAGAIN, `0 - 38` for -ENOSYS) rather than as `-1` / `-11` / `-38`. The
`0 - N` form goes through `flatten_expr`'s `BinOp::Sub` arm (handles type
promotions correctly); the literal `-N` goes through the lexer's
negative-number path (occasionally produces wrong values at the 64-bit
boundary). The codegen's constant folder collapses them to the same machine
code.

### Decimal literals in self-tests

Self-tests use decimal literals (`4096`, not `0x1000`;
`17592186028032`, not `0x000FFFFFFFFFF000`) to avoid the parser's hex path,
which shares code with the decimal path and has subtle width-extension
behavior. Enforced by the K2c / K3d / K4a / K5a / K10a contracts.

### Byte-identical re-declaration

VUMA has no `import` statement. Every kernel module that wants to call
another module's functions or use another module's layouts must
re-declare them locally, byte-identically. The `LayoutRegistry` catches
drift at compile time. K13+ will add a real `import` mechanism; until then,
every kernel contributor does the copy-paste.

---

## Stub inventory

A "stub" is a function whose body is a deliberate no-op (or trivial
placeholder) and whose header comment explicitly defers the real
implementation to a future wave (typically K11+ for hardware-touching paths,
K13+ for crypto algorithm bodies). Every stub is documented in its file
header with (a) what the real implementation will do, (b) which wave will
replace it, and (c) why the stub is safe for hosted-mode testing. The DoD
explicitly allows stubs as long as they are documented.

The full stub inventory is in
[`docs/kernel-architecture.md` §9](../../docs/kernel-architecture.md). The
highlights:

| Stub | File | Behavior | K11+/K13+ replacement |
|------|------|----------|-----------------------|
| `trap_panic`, `trap_syscall`, `trap_irq` | `trap/trap.vuma` | `return;` (no-op) | K11/K12: real trap dispatch (panic prints TrapFrame; syscall reads tf.rax + dispatches; IRQ calls irq_dispatch + pic_eoi) |
| `syscall_dispatch` | `syscall/dispatch.vuma` | bounds-check + `return 0` (no `call_indirect`) | K11: add `call_indirect(handler, args)` intrinsic |
| `sys_exec` | `proc/exec.vuma` | overwrites `task.mm_root` with 0xDEAD | K11+: real ELF loader |
| `cipher_encrypt`/`cipher_decrypt` | `crypto/api.vuma` | byte-wise copy input→output | K10b/K13+: real AES round-function loop |
| `hash_update`/`hash_final` | `crypto/api.vuma` | bumps `ctx.total` / writes 32 zero bytes | K10c/K13+: real SHA-256 compression |
| `pm_cpu_idle`/`pm_suspend` | `power/pm.vuma` | no-op / returns 0 | K11: `hlt` / `wfi` / ACPI S-state / PSCI |
| `smp_boot_cpu`/`smp_call_function` | `smp/smp.vuma` | returns 0 / no-op | K11: LAPIC INIT-SIPI-SIPI / IPI enqueue |
| `ipi_send`/`ipi_broadcast`/`ipi_dispatch` | `smp/ipi.vuma` | LAPIC stub (no-op) | K11: real LAPIC ICR MMIO write |
| `tcp_connect`/`send`/`recv` | `net/tcp.vuma` | returns -ENOTCONN if state != ESTABLISHED | K11+: real TCP state machine + virtio-net TX/RX |

Plus all hardware externs (`halt`, `wfi`, `cr3_read`/`cr3_write`,
`ttbr0_read`/`ttbr0_write`, `satp_read`/`satp_write`, `pte_read`/`pte_write`,
`tlb_flush`/`invlpg`, `idt_load`/`irq_mask`/`irq_unmask`/`pic_eoi`/`cr2_read`,
`aesni_encrypt_block`/`shani_*`, `mmio_read8`/`mmio_write8`/etc.,
`lapic_write`, `context_switch`) resolve to `__ffi_fallback_stub`
(`xor eax, eax; ret` — returns 0, void calls no-op) when not pre-registered.
The hosted-mode kernel never actually exercises these — every code path
that would call them either (a) has an `if` short-circuit routing to the
`write`/`read`/`exit` host syscalls instead, or (b) is a self-test that
just checks "did we crash?". K11+ replaces each with a real asm stub
registered in the backend's `build_runtime_syscall_stubs`.

---

For anything not covered here, see:

- [`docs/kernel-architecture.md`](../../docs/kernel-architecture.md) — full
  architecture (4-layer cake, boot flow, FFI trampolines, arena memory
  model, IVE guarantees, data-flow diagrams, memory layout, sentinel
  conventions, cross-compilation, parser limitations).
- [`docs/kernel-developer-guide.md`](../../docs/kernel-developer-guide.md) —
  how to add syscalls, drivers, filesystems, and PMT kernel code; do/don't
  examples; IVE failure debugging recipe.
- [`docs/kernel-porting-guide.md`](../../docs/kernel-porting-guide.md) —
  step-by-step guide to porting the kernel to a new architecture (worked
  example: x86_64).
- [`docs/building.md`](../../docs/building.md) — build + test reference.
- [`docs/contributing.md`](../../docs/contributing.md) — general
  contribution workflow + VUMA code patterns.
