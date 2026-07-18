# VWK Kernel Developer Guide

This guide explains how to extend the VWK (Vuma Womb Kernel) with new
functionality: new syscalls, new drivers, new filesystems, and new PMT kernel
code in general. It assumes you have read
[`kernel-architecture.md`](./kernel-architecture.md) (the four-layer cake,
PMT discipline, arena memory model) and the relevant VUMA language docs
([`architecture.md`](./architecture.md) §3 State Type System, §9 FFI Marshal
Pass).

The kernel is **PMT-only**. There is no pointer syntax, no `allocate`/`free`,
no escape hatch. Every contribution must compile cleanly with `--verify`
(IVE Pass, no `flatten_expr` warnings) and exit 0 on its self-test. The
do/don't examples below illustrate the patterns established by waves K0–K12.

---

## Table of Contents

1. [How to Add a New Syscall](#1-how-to-add-a-new-syscall)
2. [How to Add a New Driver](#2-how-to-add-a-new-driver)
3. [How to Add a New Filesystem](#3-how-to-add-a-new-filesystem)
4. [How to Write PMT Kernel Code](#4-how-to-write-pmt-kernel-code)
5. [Contribution Workflow](#5-contribution-workflow)

---

## 1. How to Add a New Syscall

Adding a syscall is a five-step process. The syscall layer lives in
`womb/kernel/syscall/`:

```
    syscall/
        abi.vuma              TrapFrame ↔ SyscallArgs marshaling
        table.vuma            512-entry handler registry (SyscallTable)
        dispatch.vuma         nr → handler lookup + invocation
        handlers/
            io.vuma           sys_write, sys_read, ...
            mm.vuma           sys_brk, sys_mmap, sys_munmap, ...
            proc.vuma         sys_getpid, sys_exit, ...
```

### Step-by-step

1. **Pick a syscall number.** Use the Linux asm-generic/unistd.h numbering
   (see `womb/syscalls.vuma`). The table has 512 slots; numbers 0..~449 are
   already claimed by Linux, so for kernel-internal syscalls use 512+.

2. **Add a handler function** in the appropriate `handlers/<group>.vuma`
   file. The handler signature is uniform:

   ```
       fn sys_<name>(a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64
   ```

   (All six args are u64 because `SyscallArgs` packs them as u64; the
   handler is responsible for any cast to `Address` or smaller integer
   types.) The return value is i64 — positive errno convention (see
   `dispatch.vuma` header: return positive errno, the trap layer negates
   before writing to `tf.rax`).

3. **Register the handler** in `kmain`'s init path. Today (K12) the
   registration is stubbed because VUMA has no fn-pointer call; K11+ will
   add real `syscall_register(tbl, nr, &sys_<name>)`. Document the
   registration call in a comment in `kmain.vuma` so K11+ knows to wire it.

4. **Add a self-test** in the handler file's `main()`. The pattern is:
   allocate a `ByteBuf` (or relevant state), call the handler directly
   (bypassing the dispatch layer), check the result.

5. **Verify.**

   ```
       ./target/release-fast/compile_dump \
           womb/kernel/syscall/handlers/<group>.vuma \
           /tmp/<group>.bin x86_64 --verify
       /tmp/<group>.bin ; echo "exit=$?"
   ```

   Expect `IVE: Pass passed=1 failed=0 total=1` and `exit=0`.

### Do / Don't

**Do** follow the routing convention in `handlers/io.vuma`:

```
    fn sys_write(fd: u64, buf: u64, count: u64, _a3: u64, _a4: u64, _a5: u64) -> i64 {
        if fd == 1 { return write(1, buf as Address, count as i64); }
        if fd == 2 { return write(2, buf as Address, count as i64); }
        // else: VFS write (K5 stub returns 0)
        return 0;
    }
```

Cast `u64` to `Address` at the FFI boundary — this is the documented PMT
pattern (`as Address` reinterpretation, no-op codegen on 64-bit targets).

**Don't** declare a `State<T>` parameter on a syscall handler. The dispatch
layer hands you raw `u64` from `SyscallArgs`; you marshal inside the handler
if needed. A signature like `fn sys_write(fd: u64, buf: State<ByteBuf>, ...)`
won't compile — `SyscallArgs.a1` is `u64`, not `State<ByteBuf>`.

**Don't** forget the `_a3`/`_a4`/`_a5` placeholders. The handler signature
is uniform across all syscalls; the dispatcher passes all six args whether
or not your syscall uses them. Omitting them is a parse error.

**Don't** call `exit()` from inside a handler — return the errno instead.
The dispatcher writes the return value to `tf.rax` and `iret`s back to
user-space. Calling `exit()` would terminate the kernel, not just the
user-space caller.

---

## 2. How to Add a New Driver

Drivers live in `womb/kernel/drivers/`. The directory currently contains:

```
    drivers/
        uart.vuma         8250 (x86_64) + PL011 (aarch64) UART
        char.vuma         character-device framework
        virtio_net.vuma   virtio-net PCI device (MMIO + IRQ)
```

A driver has three parts:

1. **A register layout** for the device's MMIO interface (e.g.
   `Uart8250Regs`, `Pl011Regs` in `drivers/uart.vuma`). This is a VUMA
   layout with one field per register; the field offsets match the device's
   register map from its spec.

2. **A set of MMIO externs** for byte/word access:
   `mmio_read8`/`mmio_write8`/`mmio_read32`/`mmio_write32`. In hosted mode
   these resolve to `__ffi_fallback_stub` (no-op); in bare-metal mode
   (K11+) they resolve to real asm stubs (`mov al, [rdi]; ret` etc.).

3. **Driver functions** (`uart_init_*`, `uart_putc_*`, `uart_getc_*`,
   `uart_puts_*` for UART). These take a `base: u64` MMIO address and the
   relevant data args; they call the MMIO externs with explicit byte
   offsets.

### Step-by-step

1. **Pick a device.** Document the device's MMIO register map in the file
   header. Cite the spec (e.g. PC16550D datasheet for 8250, ARM DDI 0183
   for PL011, virtio-v1.1 spec §5 for virtio-net).

2. **Declare the register layout.** Use u8 fields for 8-bit registers,
   u32 for 32-bit. Document each field's offset in both the layout's
   comment AND the field-level comment.

3. **Declare the MMIO externs** if they aren't already declared in another
   driver you can mirror. Follow `drivers/uart.vuma`'s pattern.

4. **Write init / putc / getc / puts (or device-equivalent) functions.**
   In hosted mode, route them to the pre-registered `write`/`read` externs
   for unit testing. In bare-metal mode (commented out for K11+), use the
   MMIO externs.

5. **Self-test.** In hosted mode this exercises the routing logic (e.g.
   "putc routes to write(1, ...) and the test sees the byte on stdout").
   In bare-metal mode the self-test exercises the MMIO trampoline path
   against QEMU's emulated device.

6. **Verify.**

   ```
       ./target/release-fast/compile_dump womb/kernel/drivers/<name>.vuma \
           /tmp/<name>.bin x86_64 --verify
       /tmp/<name>.bin ; echo "exit=$?"
   ```

### Do / Don't

**Do** use explicit byte offsets for MMIO:

```
    fn uart_putc_8250(base: u64, c: u8) {
        // Spin on LSR.TX-empty (bit 5 of register at offset 5)
        while (mmio_read8(base + 5) & 32) == 0 { }
        mmio_write8(base + 0, c);
    }
```

This is the bare-metal body. The `32` is `1 << 5` (LSR TX-empty bit) in
decimal — the kernel's decimal-constants discipline (see `arch/x86_64/pt.vuma`
header for the rationale).

**Don't** try to dereference a `State<Uart8250Regs>` pointer to access MMIO.
VUMA's PMT discipline forbids pointer dereference. The `Uart8250Regs` layout
exists for documentation + future use; the actual MMIO access goes through
`mmio_read8(base + offset)`. (See `drivers/uart.vuma` header "Why stubs for
MMIO in hosted mode?" for the full rationale.)

**Don't** forget that `mmio_read8` resolves to `__ffi_fallback_stub`
(returns 0) in hosted mode. Any `while (mmio_read8(...) & bit) == 0 { }`
loop will spin forever in hosted mode — that's why the hosted path uses
`write()` instead. The MMIO path is a comment-only sketch today; K11+ will
swap it in.

**Do** document the device's register map with both byte offsets and slot
indices if the device uses word-aligned registers (PL011's `fr` register
is at byte offset 24, slot 6 in u32 terms — `drivers/uart.vuma` calls this
out explicitly so a future maintainer doesn't confuse the two).

---

## 3. How to Add a New Filesystem

Filesystems live in `womb/kernel/fs/` and consume the VFS layer in
`womb/kernel/vfs/` (`inode.vuma`, `dentry.vuma`, `file.vuma`, `namei.vuma`,
`mount.vuma`, `file_ops.vuma`). The VFS provides the inode/dentry/file
abstractions; the filesystem provides the on-disk (or in-memory) layout
and the per-inode operations.

The directory currently contains:

```
    fs/
        tmpfs.vuma        in-memory filesystem (RAM-backed, 64×256B pages)
        initramfs.vuma    boot-time initramfs parser (cpio format)
```

### Step-by-step

1. **Pick a backing store.** Tmpfs uses a flat `[u8; 16384]` array (64
   pages × 256 bytes). Initramfs uses the cpio archive passed by the
   bootloader. A disk-backed filesystem (ext2, fat) would use a block-
   device driver (K11+). Document the backing store's layout in the file
   header.

2. **Re-declare the VFS layouts byte-identically.** VUMA has no `import`,
   so every filesystem file must re-declare `InodeTable`, `DentryTable`,
   `FileTable` (and any other VFS layout it consumes) byte-identically to
   `vfs/inode.vuma` etc. The verifiers catch any drift.

3. **Implement the per-inode operations.** At minimum: `fs_alloc_inode`,
   `fs_free_inode`, `fs_read_inode`, `fs_write_inode`, `fs_lookup`,
   `fs_read`, `fs_write`, `fs_readdir`. Each takes the relevant VFS state
   (`State<InodeTable>`, `State<FileTable>`, ...) by reference (init-style
   API — see `kernel-architecture.md` §3) plus the filesystem-private
   state.

4. **Wire the filesystem into `mount.vuma`.** The mount layer dispatches
   VFS operations to the per-filesystem function table. Add an entry for
   your filesystem in the mount table.

5. **Self-test.** Allocate the filesystem's private state, allocate the
   VFS tables, do a `create → write → close → open → read → close` cycle,
   check the round-trip. See `fs/tmpfs.vuma`'s self-test for the canonical
   pattern.

6. **Verify.**

   ```
       ./target/release-fast/compile_dump womb/kernel/fs/<name>.vuma \
           /tmp/<name>.bin x86_64 --verify
       /tmp/<name>.bin ; echo "exit=$?"
   ```

### Do / Don't

**Do** use parallel flat byte arrays for fixed-size tables. The kernel's
convention (established in K2a's `pmm.vuma`, K3d's `irq.vuma`, K3e's
`syscall/table.vuma`, K5a's VFS tables) is:

```
    layout InodeTable = {
        ino:      [u8; 1024],   // 128 inodes × 8 bytes, packed LE
        mode:     [u8; 128],    // 128 inodes × 1 byte
        size:     [u8; 1024],   // 128 inodes × 8 bytes, packed LE
        ...
    }
```

The codegen only lowers `state.array[idx]` correctly for `[u8; N]` arrays
(see `arch/x86_64/pt.vuma`'s "Why a flat byte-array pool?" header for the
full rationale). Use pack/unpack helpers for u16/u32/u64 fields.

**Don't** use nested arrays like `[[u8; 256]; 64]` — VUMA's parser does
not support nested arrays today (see `fs/tmpfs.vuma` header "Why a flat
[u8; 16384] instead of [[u8; 256]; 64]?"). Compute the per-element offset
manually: `base = idx * width`, then `data.bytes[base + off]`.

**Do** document the per-inode page list / block list strategy. Tmpfs gives
each file one page (simplified); a real filesystem uses a per-inode array
of page/block pointers. Document the simplification in the header so K11+
knows what to extend.

**Don't** allocate per-file state with `state_new(...)` from inside a VFS
callback — the arena is bump-only and never frees. Use a pre-allocated
pool (like tmpfs's 64-page `pages` array) and a free-bitmap (`page_used`)
to track which slots are claimed.

---

## 4. How to Write PMT Kernel Code

This section covers the general discipline for any new `.vuma` file under
`womb/kernel/`. The four rules below are enforced by the compiler; the
patterns below are enforced by code review.

### Rule 1 — No pointer syntax

`*T`, `&x`, `allocate`, and `free` are hard parse errors. Use `State<T>`
for all multi-word data; pass scalars (u8/u16/u32/u64/i32/i64/Address) by
value.

**Do:**

```
    layout Console = { buf: [u8; 256], len: u32 }

    fn console_putc(c: State<Console>, ch: u8) {
        let idx = c.len;
        if idx < 256 {
            c.buf[idx] = ch;
            c.len = idx + 1;
        }
    }
```

**Don't:**

```
    fn console_putc(c: *Console, ch: u8) {    // PARSE ERROR — *T forbidden
        (*c).buf[(*c).len] = ch;
        (*c).len = (*c).len + 1;
    }
```

### Rule 2 — Use the init-style API

The codegen does not propagate `State`-typedness through function return
values (Open Work §"Pipeline: State-typedness through return values"). A
`let s = make_state()` binding is NOT registered as state-typed in the
caller; subsequent `s.field` accesses silently return 0 with a
`WARNING: unsupported FieldAccess (not state-typed)` from `flatten_expr`.

**Do:**

```
    fn pmm_init(pool: State<FlatPool>, pmm: State<PmmState>,
                mem_start: u64, mem_size: u64) {
        // ... populate pmm's fields ...
    }

    // caller:
    let pool = state_new(FlatPool);
    let pmm  = state_new(PmmState);
    pmm_init(pool, pmm, 0, 16777216);
    // pmm.field accesses work correctly here
```

**Don't:**

```
    fn pmm_init(...) -> State<PmmState> {           // WARNING + broken caller
        let pmm = state_new(PmmState);
        // ... populate pmm ...
        return pmm;
    }

    // caller:
    let pmm = pmm_init(...);
    pmm.field    // silently returns 0; flatten_expr warning emitted
```

K2a's `pmm.vuma`, K3e's `syscall/abi.vuma`, and every other stateful
subsystem in the kernel uses init-style. K11+ will flip the codegen;
until then, init-style is the only correct pattern.

### Rule 3 — Use `State<T> as Address` for FFI hand-off

When an FFI callee needs a raw pointer to a state buffer, cast with
`as Address`. The cast yields the buffer's base address (offset 0).

**Do:**

```
    extern "C" { fn write(fd: i64, buf: Address, count: i64) -> i64; }

    layout Console = { buf: [u8; 256], len: u32 }   // buf at offset 0

    fn console_flush(c: State<Console>) {
        if c.len > 0 {
            let base = c as Address;
            let _n = write(1, base, c.len as i64);
            c.len = 0;
        }
    }
```

**Don't:**

```
    fn console_flush(c: State<Console>) {
        // No way to write `&c.buf[0]` — VUMA has no `&`.
        // No way to write `*(u8*)c` — VUMA has no pointer cast syntax.
    }
```

The convention is: put the buffer field at offset 0 in the layout, so the
`as Address` cast yields `&buf[0]`. This is documented in `console.vuma`,
`kmsg.vuma`, `panic.vuma`, and `hw_trampoline.vuma` — the pattern is
uniform across the kernel's I/O paths.

### Rule 4 — Use transforms for ownership transfer

When a function consumes a state and the caller should not touch it again,
use the `StateTransform` verifier's discipline: the function takes the
state by value (no `#[borrow]`), and the codegen marks it consumed. The
next `state.field` access in the caller trips the use-after-invalidate
verifier.

When a function borrows a state (the caller wants to keep using it), use
`#[borrow]` on the extern declaration:

**Do (borrow):**

```
    extern "C" {
        #[borrow] fn pte_read(pt: State<PageTable>, level: u8, idx: u32) -> u64;
    }

    fn vmm_walk(pt: State<PageTable>, vaddr: u64) -> u64 {
        let pte = pte_read(pt, 3, idx);   // pt still valid
        let pte2 = pte_read(pt, 2, idx2); // pt still valid (#[borrow])
        return pte2;
    }
```

**Don't (forget #[borrow]):**

```
    extern "C" {
        fn pte_read(pt: State<PageTable>, level: u8, idx: u32) -> u64;
        //                                                 ^^^ no #[borrow]
    }

    fn vmm_walk(pt: State<PageTable>, vaddr: u64) -> u64 {
        let pte = pte_read(pt, 3, idx);   // pt now CONSUMED
        let pte2 = pte_read(pt, 2, idx2); // VERIFIER FAILURE — use-after-invalidate
        return pte2;
    }
```

For regular (non-extern) VUMA fn calls, `#[borrow]` is NOT needed — the
codegen tracks state lifetimes through regular fn calls. Only extern "C"
calls need it, because the codegen conservatively invalidates on extern
calls (the extern's body is opaque to PMT analysis).

### General kernel-code conventions

- **Decimal constants in self-tests.** All non-zero constants in self-tests
  use decimal (e.g. `4096`, not `0x1000`). Decimal avoids the parser's
  hex-literal path (`parse_int_radix` width-extension subtleties). See
  `arch/x86_64/pt.vuma` header for the full rationale.

- **Layouts before functions.** Declare every layout BEFORE the first
  function that uses it. VUMA's parser accepts forward references but the
  established convention (K7a, K10b, K10c, K12b) is layout-first.

- **Self-test exits 0 with a per-check return code.** Pattern:
  `if check1 { return 1; } if check2 { return 2; } ... return 0;` — so a
  future CI failure pinpoints the broken check. See `arch/x86_64/pt.vuma`'s
  self-test for the canonical form.

- **Header comment.** Every kernel file begins with a `════` boxed header
  documenting: file path, one-line description, PMT discipline notes,
  build/verify/run command, K-wave lineage, and any contract deviations.
  See any file under `womb/kernel/` for the template.

---

## 5. Contribution Workflow

The VWK effort is organized as numbered waves (K0, K1, ..., K12, K13+). Each
wave is a single Git commit on the main branch with a structured message:

```
    Wave K<NN><a/b/c/...>: <title>
```

The wave dispatch boxes (one per wave) live in the orchestration layer; each
box specifies:

- **Task ID** (e.g. `K12d+e+f`)
- **Files to touch** (explicit allow-list — touching other files is a
  contract violation)
- **Contract** (DoD checklist, code patterns, signatures)
- **Commit message** (exact text)

### To contribute a new wave

1. **Read the worklog.** `/home/z/my-project/worklog.md` is the single
   shared worklog. Every wave's section records: design decisions, contract
   deviations (with rationale), deferred-to-future-wave notes, and the
   final commit hash. Read at least the last 3 waves before starting.

2. **Touch only the contracted files.** The allow-list is exhaustive. If
   you need to edit a file outside the list, escalate to the orchestrator
   — don't silently expand scope.

3. **Verify before commit.**

   ```
       . "$HOME/.cargo/env"
       cargo build --profile release-fast --bin compile_dump
       ./target/release-fast/compile_dump <your-file>.vuma \
           /tmp/test.bin x86_64 --verify
       /tmp/test.bin ; echo "exit=$?"
   ```

   The DoD requires: `IVE: Pass passed=1 failed=0 total=1`, exit 0, no
   `flatten_expr` warnings, no new cargo warnings.

4. **Commit with the exact contracted message.** Do NOT push.

   ```
       git add <files>
       git commit -m "Wave K<NN><a/b/c/...>: <title>"
   ```

5. **Append to the worklog.** Add a section with:

   ```
       ---
       Task ID: K<NN><a/b/c/...>
       Agent: <your-agent-type>
       Task: <wave title>

       Work Log:
       - <what you did, with code snippets + decisions>

       Stage Summary:
       - K<NN> DoD fully satisfied:
         - [x] <each DoD item>
       - <design notes for downstream waves>
       - Blockers: <none / list>
       - Deferred to K<NN+>: <list>
   ```

   See the existing worklog entries (K0 through K12) for the canonical
   form. Future-wave authors rely on the "Deferred to" notes to plan their
   work.

### Cross-wave parity

Every kernel module must compile + verify + self-test exit 0 standalone
as well as when inlined into `kernel.vuma`. The byte-identical
re-declaration invariant (K2c, extended to layouts in K3d) is what makes
this work — when you copy a layout or extern signature, copy it byte-for-
byte. If you need to change a layout, change it in its canonical source
AND in every re-declaration; the verifiers will catch any drift, but
silent drift across the codebase is a maintainability hazard.

The kernel is feature-complete at the API-contract level as of K12.
K13+ will replace stubs with real algorithms (real AES, real SHA-256,
real Ed25519, real TCP data path, real ACPI/PSCI/SBI suspend drivers,
real asm trampolines for `halt`/`wfi`/IRQ-disable, real syscall fn-pointer
dispatch) keeping the same layout definitions + function signatures +
self-test structure that K0–K12 established. The contribution workflow
above is how those replacements will land.
