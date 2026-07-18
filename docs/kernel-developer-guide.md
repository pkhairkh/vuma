# VWK Kernel Developer Guide

This guide explains how to extend the VWK (Vuma Womb Kernel) with new
functionality: new syscalls, new drivers, new filesystems, and new PMT kernel
code in general. It assumes you have read
[`kernel-architecture.md`](./kernel-architecture.md) (the four-layer cake,
PMT discipline, arena memory model, stub inventory, parser limitations) and
the relevant VUMA language docs ([`architecture.md`](./architecture.md) §3
State Type System, §9 FFI Marshal Pass).

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
6. [Debugging Self-Tests](#6-debugging-self-tests)
7. [Stub Inventory (Cross-Reference)](#7-stub-inventory-cross-reference)
8. [VUMA Code Patterns](#8-vuma-code-patterns)
9. [Code Style Guide](#9-code-style-guide)
10. [Integration Testing](#10-integration-testing)

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
            io.vuma           sys_write, sys_read (3 args: fd, buf, count)
            mm.vuma           sys_brk, sys_mmap, sys_munmap
            proc.vuma         sys_getpid, sys_exit
```

### Step-by-step

1. **Pick a syscall number.** Use the Linux asm-generic/unistd.h numbering
   (see `womb/syscalls.vuma` — the documentation-only reference file one
   directory above `womb/kernel/`). The SyscallTable has 512 slots; numbers
   0..~449 are already claimed by Linux asm-generic, so for kernel-internal
   syscalls use 512+.

2. **Add a handler function** in the appropriate `handlers/<group>.vuma`
   file. The handler signature is uniform per group, but **not all handlers
   take six args** — the I/O group (`io.vuma`) uses a 3-arg signature
   matching `write`/`read`:

   ```
       // handlers/io.vuma (3-arg signature — matches the write/read ABI):
       fn sys_write(fd: u64, buf: u64, count: u64) -> i64 {
           if fd == 1 { return write(1, buf as Address, count as i64); }
           if fd == 2 { return write(2, buf as Address, count as i64); }
           return 0;   // VFS write stub (K5)
       }
   ```

   The `mm.vuma` and `proc.vuma` groups use the canonical 6-arg form for
   syscalls that need more arguments:

   ```
       // handlers/proc.vuma (6-arg signature):
       fn sys_getpid(_a0: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64) -> i64 {
           return 1;   // stub: return init's pid
       }
   ```

   All args are u64 because `SyscallArgs` packs them as u64 (see
   `syscall/abi.vuma::SyscallArgs`). The handler is responsible for any cast
   to `Address` or smaller integer types. The return value is i64 —
   positive errno convention (see `dispatch.vuma` header: return positive
   errno, the trap layer negates before writing to `tf.rax`). Some handlers
   use `0 - N` for negative returns directly (see
   [`kernel-architecture.md` §13](./kernel-architecture.md#13-error-code-convention)).

3. **Register the handler** in the SyscallTable via `syscall_table_set(tbl,
   nr, &sys_<name>)`. The registration mechanism **works today** —
   `SyscallTable` (declared in `syscall/table.vuma`) is a real 512-entry
   u64-handler-address table with working pack/unpack helpers, and
   `syscall_table_set` correctly stores the handler address (verified by
   `dispatch.vuma`'s self-test).

   What's **stubbed** is the *dispatch step*: `syscall_dispatch` looks up
   the handler, checks `handler == 0` (unregistered → return -ENOSYS), and
   then... returns 0 (because VUMA has no `call_indirect` intrinsic yet —
   calling a function pointer requires either an indirect-call instruction
   that the codegen doesn't emit, or a giant `switch` statement in trap.S).
   K11+ adds the real `call_indirect(handler_addr, args)` intrinsic.

   So today: **registration works** (you can store a handler address in the
   table), **lookup works** (the dispatch reads it back), **call doesn't
   happen** (the dispatch returns 0 instead of invoking the handler). For
   hosted mode this is fine — the host kernel uses its own syscall ABI, not
   int 0x80; the kernel's int 0x80 path is never exercised at runtime in
   hosted mode. For bare-metal K11+, the real `call_indirect` will close
   the loop.

   Document the registration call in a comment in `kernel.vuma::kmain()`
   so K11+ knows to wire it:

   ```
       // K11+ wiring (when call_indirect lands):
       //   syscall_table_set(tbl, 64, &sys_write);  // __NR_write
       //   syscall_table_set(tbl, 63, &sys_read);   // __NR_read
       //   syscall_table_set(tbl, 93, &sys_exit);   // __NR_exit
   ```

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
    fn sys_write(fd: u64, buf: u64, count: u64) -> i64 {
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

**Don't** forget the `_a3`/`_a4`/`_a5` placeholders if you use the 6-arg
form. The handler signature is uniform across all syscalls in a group; the
dispatcher passes all six args whether or not your syscall uses them.
Omitting them is a parse error. The leading underscore (`_a3` not `a3`) is
a documentation convention — VUMA silently drops unused params (no
warning, no error), so the underscore is for human readers.

**Don't** call `exit()` from inside a handler — return the errno instead.
The dispatcher writes the return value to `tf.rax` and `iret`s back to
user-space. Calling `exit()` would terminate the kernel, not just the
user-space caller.

### When to use 3-arg vs 6-arg

The I/O group uses 3 args because `write`/`read` only take 3 user args
(fd, buf, count). The mm group uses 6 args because `mmap` takes 6. Pick
the form that matches your syscall's user-space ABI — don't add 3 unused
`_a3`/`_a4`/`_a5` placeholders to a 3-arg syscall "for consistency"; the
group's existing convention is the consistency bar.

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
decimal — the kernel's decimal-constants discipline (see
[`kernel-architecture.md` §10.6](./kernel-architecture.md#106-the-hex-literal-width-extension-subtlety)).

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

3. **Implement the per-inode operations.** The naming convention is
   `<fs>_<op>` for filesystem-private operations and re-declared
   `inode_*` / `dentry_*` / `file_*` helpers for VFS-layer accessors:

   ```
       // VFS-layer helpers (re-declared byte-identically to vfs/inode.vuma):
       fn inode_alloc(tbl: State<InodeTable>) -> u32 { ... }
       fn inode_free(tbl: State<InodeTable>, idx: u32) { ... }
       fn inode_get_ino(tbl: State<InodeTable>, idx: u32) -> u64 { ... }
       fn inode_set_ino(tbl: State<InodeTable>, idx: u32, val: u64) { ... }
       // ... 12 more inode_get_* / inode_set_* helpers

       // Filesystem-private operations (the <fs>_<op> pattern):
       fn tmpfs_mount(inodes, dentries, data) -> u32 { ... }
       fn tmpfs_create(inodes, dentries, parent_idx, name) -> u32 { ... }
       fn tmpfs_mkdir(inodes, dentries, parent_idx, name) -> u32 { ... }
       fn tmpfs_lookup(dentries, parent_idx, name) -> u32 { ... }
       fn tmpfs_read(data, file, buf, count) -> i64 { ... }
       fn tmpfs_write(data, file, buf, count) -> i64 { ... }
   ```

   At minimum, implement: `<fs>_mount`, `<fs>_create`, `<fs>_lookup`,
   `<fs>_read`, `<fs>_write`. Each takes the relevant VFS state
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
full rationale, and
[`kernel-architecture.md` §10.4](./kernel-architecture.md#104-the-array-index-bytewise-limitation)).
Use pack/unpack helpers for u16/u32/u64 fields.

**Don't** use nested arrays like `[[u8; 256]; 64]` — VUMA's parser does
not support nested arrays today (see `fs/tmpfs.vuma` header "Why a flat
[u8; 16384] instead of [[u8; 256]; 64]?"). Compute the per-element offset
manually: `base = idx * width`, then `data.bytes[base + off]`.

**Do** use the `<fs>_<op>` naming convention for filesystem-private
operations and the bare `inode_*`/`dentry_*`/`file_*` names for re-declared
VFS helpers. This matches `fs/tmpfs.vuma`'s pattern: `tmpfs_create`,
`tmpfs_lookup`, `tmpfs_read` for the fs-specific operations; `inode_alloc`,
`inode_get_ino`, `dentry_set_parent` for the VFS-layer accessors.

**Don't** invent names like `fs_alloc_inode` or `fs_read_inode` — the
convention is `<fs>_<op>` for fs-private ops and the bare `inode_<op>`
for VFS-layer accessors. The `fs_*` prefix is not used anywhere in the
kernel today.

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
        let idx3 = vmm_walk_idx(vaddr, 3);   // extract 9-bit index
        let pte  = pte_read(pt, 3, idx3);    // pt still valid
        let idx2 = vmm_walk_idx(vaddr, 2);
        let pte2 = pte_read(pt, 2, idx2);    // pt still valid (#[borrow])
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
        let idx3 = vmm_walk_idx(vaddr, 3);
        let pte  = pte_read(pt, 3, idx3);    // pt now CONSUMED
        let idx2 = vmm_walk_idx(vaddr, 2);
        let pte2 = pte_read(pt, 2, idx2);    // VERIFIER FAILURE — use-after-invalidate
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
  hex-literal path (`parse_int_radix` width-extension subtleties — see
  [`kernel-architecture.md` §10.6](./kernel-architecture.md#106-the-hex-literal-width-extension-subtlety)).

- **Layouts before functions.** Declare every layout BEFORE the first
  function that uses it. VUMA's parser accepts forward references for fns
  but the layout registry is single-pass — a layout must be registered
  before any function that touches its fields is lowered.

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

1. **Read the worklog.** The worklog is the single shared VWK worklog kept
   alongside the orchestration tooling. It is **not** inside the vuma repo
   — ask your orchestrator for the path. Every wave's section records:
   design decisions, contract deviations (with rationale),
   deferred-to-future-wave notes, and the final commit hash. Read at
   least the last 3 waves before starting.

   To find a specific K-wave's notes, search the worklog for the literal
   string `Task ID: K<NN>` (e.g. `Task ID: K2a` for the PMM wave). The
   worklog is append-only — never edit a prior entry; add a new section
   for your wave.

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

---

## 6. Debugging Self-Tests

When a kernel module's self-test fails (exits non-zero, or `--verify`
reports IVE: Fail), the failure mode is usually one of:

1. **IVE verifier failure** — `flatten_expr` warning or `StateRead`/`Write`/
   `Transform` verifier trip.
2. **Self-test exit code N > 0** — one of the per-check returns fired; the
   N tells you which check.
3. **Compile error** — parser rejected the file; you'll see a parse error
   message with a line number.
4. **Runtime trap** — `__arena_overflow` or `__ffi_fallback_stub` returned
   0 when the test expected a real value.

### 6.1 Interpreting IVE failures

The `--verify` flag runs three IVE verifiers (`StateRead`, `StateWrite`,
`StateTransform`) plus the arena-bounds check. Output format:

```
    IVE: Pass passed=1 failed=0 total=1       ← green
    IVE: Fail passed=0 failed=1 total=1       ← red
      <VerifierName>: <reason>
        at line <N>: <failing expression>
        prev invalidate: <prior site that consumed the state>
```

Common IVE failure patterns:

**StateReadVerifier: no such field `state.lennth`** — typo. Fix the field
name. The verifier checks against the registered layout's field list.

**StateWriteVerifier: write to invalidated State<Console>** — you wrote
`c.field = value` after an extern "C" call that consumed `c`. Fix: add
`#[borrow]` to the extern's `c: State<Console>` parameter, OR re-order
the call so the field write happens before the extern call.

**StateWriteVerifier: write past end of array** — you indexed `buf[N]`
where N >= the array's length. The verifier computes the array bound from
the layout; a constant-foldable index that exceeds the bound is caught
at compile time. A dynamic index (`buf[c.len]`) is NOT caught at compile
time — it's a runtime bounds check that the arena runtime handles (or
doesn't, if the access is in-bounds of the layout but out-of-bounds of
the logical array).

**StateTransformVerifier: layout-incompatible cast** — you wrote
`state1 as State<T2>` where `T1` and `T2` have different sizes or field
layouts. Fix: make T1 and T2 byte-identical, OR use `as Address` instead
of `as State<T2>`.

### 6.2 Interpreting `flatten_expr` warnings

Even if IVE passes, the compiler's stderr may contain `WARNING:
unsupported FieldAccess (not state-typed)` lines from `flatten_expr`.
These warnings mean a `state.field` access will silently return 0 at
runtime — they are NOT benign. The fix is almost always to convert a
return-style helper to init-style.

Common `flatten_expr` warning patterns:

**`let s = make_state(); s.field`** — `make_state` returns `State<T>` but
the codegen doesn't propagate the State-typedness through the return. Fix:
make `make_state` take `s` as a parameter (init-style).

**`let s = state_new(T); helper(s); s.field`** — `helper` is an extern
"C" call that didn't have `#[borrow]`. Fix: add `#[borrow]` to `helper`'s
`s: State<T>` parameter.

**`fn f(s: State<T>) { ... s.field ... }` called as `f(state_new(T))`** —
the inline `state_new(T)` doesn't get a binding, so its State-typedness
isn't tracked. Fix: bind it first (`let s = state_new(T); f(s);`).

### 6.3 Common compile errors

**`parse error: expected ';' after expression`** — usually a missing
semicolon, but on self-tests it's often a `Layout { ... }` struct literal
(VUMA has no struct literals — see
[`kernel-architecture.md` §10.7](./kernel-architecture.md#107-the-no_struct_literal-trap)).
Fix: use `state_new(Layout)` + field-by-field assignment.

**`parse error: unexpected token '*'`** — you wrote `*ptr` or `&x`. VUMA
is PMT-only; there is no pointer syntax. Fix: use `State<T>` and
`state.field` access.

**`parse error: unknown type`** — you used a layout name that wasn't
declared in this file. VUMA has no `import` — every layout must be
re-declared locally. Fix: copy the layout block from the canonical source
byte-identically.

**`syscall number (first argument) must be an integer literal`** — you
wrote `syscall(NR_WRITE, ...)` instead of `syscall(64, ...)`. The parser
requires `nr` to be a literal, not a const. Fix: inline the literal.

**`error: undeclared identifier 'idx'`** — you used a variable without
declaring it (VUMA requires `let` for every binding). The
`kernel-developer-guide.md` Rule 4 example previously had this bug —
`vmm_walk` used `idx` and `idx2` without `let`. The fix is to either
declare them (`let idx = ...`) or rewrite the example to use the actual
`vmm_walk_idx(vaddr, level)` helper from `mm/vmm.vuma`:

```
    fn vmm_walk(pt: State<PageTable>, vaddr: u64) -> u64 {
        let idx3 = vmm_walk_idx(vaddr, 3);   // 9-bit index for level 3
        let pte  = pte_read(pt, 3, idx3);
        let idx2 = vmm_walk_idx(vaddr, 2);
        let pte2 = pte_read(pt, 2, idx2);
        return pte2;
    }
```

### 6.4 Debugging runtime exit codes

If `--verify` passes (IVE: Pass) but `/tmp/test.bin` exits non-zero, the
self-test's per-check return fired. The exit code tells you which check:

```
    fn main() -> i32 {
        if check1_fails { return 1; }
        if check2_fails { return 2; }
        if check3_fails { return 3; }
        return 0;
    }

    // /tmp/test.bin; echo "exit=$?"
    // exit=2  → check2 failed
```

Read the self-test's `main()` to map the exit code to the specific check.
The pattern is enforced across all kernel modules — see
`arch/x86_64/pt.vuma`'s self-test for the canonical form.

### 6.5 Debugging arena overflow

If `--verify` passes but the binary exits with SIGILL (or a non-zero code
that doesn't match any self-test check), the arena overflow trap may have
fired. Symptoms:

- Process exits with `signal: 4 (SIGILL)` or `signal: 6 (SIGABRT)`.
- `dmesg` shows `trap: invalid opcode` (x86_64 `ud2` instruction).
- The self-test never reaches its first check.

The fix is to reduce the test's allocation count or increase the arena
capacity (`BootInfo.mem_size` in `arch/x86_64/bootinfo.vuma` — default
16 MB). The arena is shared across all `state_new(...)` calls; a buggy
loop that allocates in each iteration will exhaust it.

To confirm the overflow, add a `kmsg_write` call before the suspected
loop (K12b's `kmsg.vuma` provides a 256-byte ring buffer that you can
dump on exit). Or compile with `RUST_LOG=vuma::arena=trace` to see each
allocation.

### 6.6 Debugging cross-arch failures

If a module passes on x86_64 but fails on aarch64 or riscv64, the most
likely cause is a per-arch field-name asymmetry (e.g. `tf.vector` on
x86_64 vs `tf.esr_el1` on aarch64 vs `tf.scause` on riscv64). Diff the
per-arch `trap_trampoline.vuma` to find the right field name.

The second most likely cause is a per-arch syscall-number difference
(see [`kernel-architecture.md` §15](./kernel-architecture.md#15-cross-compilation)).
The hosted x86_64 stubs use x86_64-native numbers; aarch64/riscv64 use
asm-generic numbers. If you're hard-coding a syscall number in a test,
make sure it matches the target arch.

---

## 7. Stub Inventory (Cross-Reference)

The full stub inventory lives in
[`kernel-architecture.md` §9](./kernel-architecture.md#9-stub-inventory).
This section is a quick-reference for developers writing new code that
might depend on a stub.

### How to identify a stub

A stub is any function whose body is a deliberate no-op (or trivial
placeholder) AND whose header comment explicitly defers the real
implementation to a future wave. The DoD allows stubs as long as they are
documented. To identify a stub:

1. **Grep for `K11` or `K13` in the file header.** Stubs are usually
   tagged with "K11+ will replace this" or "K13+ will replace this".

2. **Grep for `return 0` or `return;` followed by `// stub`.** Most stubs
   have an explicit `// stub` comment on the no-op line.

3. **Grep for `__ffi_fallback_stub`.** Unregistered externs resolve to
   this symbol — they're not "stubs" in the function-body sense, but they
   behave like stubs at runtime (return 0 / no-op).

### Quick-reference: stubs by category

| Category | Stub functions                                             | Replacement wave |
|----------|------------------------------------------------------------|------------------|
| Trap     | `trap_panic`, `trap_syscall`, `trap_irq`                   | K11/K12          |
| Syscall  | `syscall_dispatch` (call_indirect step)                    | K11              |
| Process  | `sys_exec`, `sys_waitpid` (partial — only specific-pid + sleep branches stubbed) | K11+ |
| Crypto   | `cipher_encrypt`, `cipher_decrypt`, `hash_update`, `hash_final` | K10b-K10e / K13+ |
| Power    | `pm_cpu_idle`, `pm_suspend`                                | K11              |
| SMP      | `smp_boot_cpu`, `smp_call_function`, `ipi_send`, `ipi_broadcast`, `ipi_dispatch` | K11 |
| Net      | `tcp_connect`, `tcp_send`, `tcp_recv` (state-machine stubs)| K11+             |
| Hardware externs | `halt`, `wfi`, `cr3_*`, `ttbr0_*`, `satp_*`, `pte_*`, `tlb_flush`, `invlpg`, `idt_load`, `irq_*`, `pic_eoi`, `cr2_read`, `aesni_*`, `mmio_*`, `lapic_write`, `context_switch` | K11 |

### Writing code that depends on a stub

If your new code calls a stub, document the dependency in your file
header. Example:

```
    // ── Dependencies on stubs ─────────────────────────────────────────
    // This file calls:
    //   - sys_exec (proc/exec.vuma) — K4d stub overwrites mm_root with
    //     0xDEAD; K11+ will run the real ELF loader.
    //   - pte_read (arch/x86_64/mm_trampoline.vuma) — K2c stub returns 0;
    //     K11+ will do the real MMIO read.
    // The self-test compensates for the stubs by checking the stub
    // behavior (mm_root == 0xDEAD after sys_exec) rather than the
    // real-kernel behavior (mm_root == new PGD phys).
```

When K11+ replaces the stub, your self-test will need to be updated to
check the real behavior. Tag such self-test checks with `// K11+: update
this check when <stub> is replaced` so the wave that swaps the stub knows
to update the test.

---

## 8. VUMA Code Patterns

The kernel uses a small set of recurring code patterns. This section is a
comprehensive reference for the patterns you'll see (and write) in kernel
code.

### 8.1 Init-style API

The init-style API is the canonical pattern for any function that creates
or populates a State<T>. The caller allocates the state via
`state_new(Layout)` (which marks the binding as state-typed), then passes
it by reference to the function:

```
    // Function signature: takes State<T> by value (regular VUMA-fn borrow).
    fn pmm_init(pool: State<FlatPool>, pmm: State<PmmState>,
                mem_start: u64, mem_size: u64) {
        pmm.total_pages = mem_size / 4096;
        // ... populate pmm's fields ...
    }

    // Caller:
    let pool = state_new(FlatPool);
    let pmm  = state_new(PmmState);
    pmm_init(pool, pmm, 0, 16777216);
    // pmm.field accesses work correctly here (state-typed binding).
```

Used by: `pmm_init`, `vmm_init`, `trap_frame_init`, `task_init_for_switch`,
`syscall_args_from_frame`, `kmsg_init`, `pm_init`, `spinlock_init`,
`mutex_init`, `rwlock_init`, `waitq_init`, `futex_table_init`,
`shm_table_init`, `pipe_init`, `signal_table_init`, `socket_table_init`,
`ipi_table_init`, `smp_init`, `percpu_init`, `inode_init`,
`dentry_init`, `file_table_init`, `mount_table_init`, `tmpfs_data_init`.

### 8.2 Flat byte arrays + pack/unpack helpers

For tables of N u32/u64 entries, the kernel stores the data as a flat
`[u8; N * width]` array (because the codegen only lowers `state.array[idx]`
correctly for `[u8; N]` — see
[`kernel-architecture.md` §10.4](./kernel-architecture.md#104-the-array-index-bytewise-limitation)).
Pack/unpack helpers convert between the byte array and the u64 value:

```
    layout IrqTable = {
        handlers: [u8; 2048],   // 256 × 8 bytes (u64 per IRQ)
        count: u32,
    }

    // Read the u64 handler address for IRQ `n` from IrqTable.handlers.
    fn irq_get_handler(tbl: State<IrqTable>, n: u32) -> u64 {
        let off = n * 8;
        let v: u64 = 0;
        let i = 0;
        while i < 8 {
            let sh = i * 8;
            let b = tbl.handlers[off + i] as u64;
            v = v + (b << sh);
            i = i + 1;
        }
        return v;
    }

    // Write the u64 handler address for IRQ `n` into IrqTable.handlers.
    fn irq_set_handler(tbl: State<IrqTable>, n: u32, handler: u64) {
        let off = n * 8;
        let i = 0;
        while i < 8 {
            let sh = i * 8;
            let b = (handler >> sh) & 255;
            tbl.handlers[off + i] = b as u8;
            i = i + 1;
        }
    }
```

The pack/unpack pattern is byte-identical (8-iteration while loop, shift
by `i * 8`, mask 255 on writes, sum-of-shifted-bytes on reads) across
`pmm.vuma::pool_get_base`, `irq.vuma::irq_get_handler`,
`syscall/table.vuma::syscall_table_get`, `task.vuma::pt_get_vruntime`,
and every other kernel table module. When writing a new table, copy the
pack/unpack helper verbatim and rename it.

### 8.3 If-expressions and nested-if state machines

VUMA's `if`/`else if`/`else` syntax is C-like. There's no `match`/`switch`
statement; multi-branch state machines use nested `if`s:

```
    fn trap_handler(tf: State<TrapFrame>) {
        let vec = tf.vector;
        if vec < 32 {
            trap_panic(tf);
        } else {
            if vec == 128 {
                trap_syscall(tf);
            } else {
                trap_irq(tf);
            }
        }
    }
```

VUMA also supports if-expressions (`let x = if cond { a } else { b };`)
which K2a added to the parser + codegen. These are used for short
conditional assignments:

```
    let sign = if val >= 0 { 1 } else { 0 - 1 };
```

### 8.4 State-as-address cast

The `State<T> as Address` cast hands the state's base address to an FFI
callee that expects a raw pointer. The cast yields offset 0 of the
layout:

```
    layout Console = { buf: [u8; 256], len: u32 }   // buf at offset 0

    fn console_flush(c: State<Console>) {
        if c.len > 0 {
            let base = c as Address;       // ← yields &c.buf[0]
            let _n = write(1, base, c.len as i64);
            c.len = 0;
        }
    }
```

The convention is: put the buffer field at offset 0 in the layout, so
the cast yields `&buf[0]`. This is the only sanctioned "lossy" ownership
transfer in the language — the State<T> is treated as an Address for the
duration of the FFI call, then the State<T> is still valid (the cast
doesn't consume it).

Used by: `console_flush`, `kmsg_write`, `panic`, `hw_trampoline`'s
AES-NI block calls, and every kernel I/O path.

### 8.5 Atomic CAS loop

For spinlocks and lock-free data structures, VUMA's `atomic_cas(addr, old,
new)` intrinsic lowers to `LOCK CMPXCHG` on x86_64 (and the equivalent on
other arches). The pattern is a busy-wait loop:

```
    fn spinlock_acquire(lock: State<Spinlock>) {
        // CAS target word: bytes [0..7] of lock = { locked: u32, _pad0: u32 }
        let addr = lock as Address;
        let i = 0;
        while i < 1000 {   // bounded spin (1000 iterations)
            if atomic_cas(addr, 0, 1) == 0 {
                // CAS succeeded (returned 0 = "old value matched")
                lock.holder = current_task_idx;
                lock.depth = 1;
                return;
            }
            i = i + 1;
        }
        // Fallback: yield (K11+ will call sched_yield here)
    }
```

The `_pad0: u32` field between `locked` and `holder` is load-bearing —
see [`kernel-porting-guide.md` "Common Pitfalls"](./kernel-porting-guide.md#common-pitfalls).
AtomicCas is hardcoded to U64 (8 bytes); without `_pad0`, the CAS window
includes `holder` and the round-trip fails.

### 8.6 Sentinel-based free list

For tables with a fixed slot count (e.g. tmpfs's 64 pages, futex's 64
slots, ProcessTable's 256 tasks), the kernel uses a sentinel-based free
list: a `free_head` index + `next` pointers threaded through the slots
themselves:

```
    layout TmpfsData = {
        pages: [u8; 16384],     // 64 pages × 256 bytes
        page_used: [u8; 64],    // 1 = page is in use, 0 = free
        free_list: u8,          // head of free list (255 = empty)
        ...
    }

    fn tmpfs_data_alloc_page(data: State<TmpfsData>) -> u32 {
        if data.free_list == 255 {
            return 64;   // sentinel: all pages used
        }
        let idx = data.free_list as u32;
        data.page_used[idx] = 1;
        // Walk the free list to find the next free page (linear scan for
        // simplicity — a real allocator would thread `next` pointers).
        let i = idx + 1;
        while i < 64 {
            if data.page_used[i] == 0 {
                data.free_list = i as u8;
                return idx;
            }
            i = i + 1;
        }
        data.free_list = 255;   // no more free pages
        return idx;
    }
```

The sentinel conventions are documented in
[`kernel-architecture.md` §14](./kernel-architecture.md#14-sentinel-value-convention):
256 = empty for 256-slot tables, 64 = full for 64-slot tables, 255 =
end-of-list for u8 free-lists, 0 = free slot for state == 0 / handler == 0
tables.

---

## 9. Code Style Guide

### 9.1 Naming conventions

- **snake_case** for all identifiers: functions, layouts, fields, variables.
  VUMA's parser accepts camelCase but the kernel convention is snake_case
  (matches Rust).
- **Layout names**: PascalCase (`Spinlock`, `InodeTable`, `TrapFrame`).
  VUMA's parser accepts both but the kernel convention is PascalCase for
  type-level names.
- **Decimal constants**: all non-zero constants use decimal, not hex (see
  [`kernel-architecture.md` §10.6](./kernel-architecture.md#106-the-hex-literal-width-extension-subtlety)).
  Use `4096`, not `0x1000`. The hex form is acceptable in comments and
  prose for readability.
- **Sentinel values**: see
  [`kernel-architecture.md` §14](./kernel-architecture.md#14-sentinel-value-convention).
  256 = empty, 64 = full, 255 = end-of-list, 0 = free slot.
- **`_` prefix for unused params**: `_entry`, `_stack`, `_a3` — VUMA
  silently drops unused params (no warning); the underscore is for human
  readers (matches Rust's convention).
- **`<fs>_<op>` for filesystem-private ops**: `tmpfs_create`,
  `tmpfs_lookup`, `initramfs_fill_super`. Bare `inode_*`/`dentry_*`/
  `file_*` for re-declared VFS helpers.

### 9.2 Comment style

- **File header**: boxed `════` comment block at the top of every file
  (see any file under `womb/kernel/` for the template). Includes: file
  path, one-line description, PMT discipline notes, build/verify/run
  command, K-wave lineage, contract deviations.
- **Section dividers**: `──` box comments between major sections of a
  file (layouts, externs, helpers, public API, self-test).
- **Inline comments**: `//` for short notes on a single line. For longer
  rationale, use a multi-line `//` block above the code (not `/* */` —
  VUMA's parser accepts `/* */` but the kernel convention is `//`).

### 9.3 File structure

Every kernel `.vuma` file follows this top-to-bottom structure:

```
    1. File header (boxed `════` comment)
    2. Layouts (declared BEFORE any function that uses them)
    3. Extern "C" blocks (with #[borrow] where needed)
    4. Helper functions (pack/unpack, get/set accessors)
    5. Public API functions (init, alloc, free, ops)
    6. Self-test (fn main() -> i32 with per-check returns)
```

The K7a / K10b / K10c / K12b contracts enforce this layout. The
self-test's `main()` is always last; forward references to helpers above
it are allowed (the parser does a two-pass scan — see
[`kernel-architecture.md` §10.8](./kernel-architecture.md#108-the-forward-reference-allowance)).

### 9.4 Decimal-constants discipline

Every kernel contract since K2c includes an "IMPORTANT: use decimal
constants" rule. The rationale:

- VUMA's hex-literal path goes through `parse_int_radix`, which shares
  code with the decimal path. There's a subtle width-extension bug at
  the 64-bit boundary that occasionally produces wrong values for hex
  literals near `0x7FFFFFFFFFFFFFFF` or `0xFFFFFFFFFFFFFFFF`.
- Decimal literals go through the same `parse_int_radix` but the
  width-extension subtlety doesn't trigger for decimal inputs (the bug
  is specific to the hex prefix-handling code path).
- The codegen lowers decimal and hex literals to identical machine code
  (the constant folder doesn't care about the source form).

So decimal is **strictly safer** than hex for self-test constants. Use
hex only in comments and prose for readability:

```
    // DON'T (hex literal — risky):
    let mask = 0x000FFFFFFFFFF000;

    // DO (decimal literal — verified safe):
    let mask = 17592186028032;   // 0x000FFFFFFFFFF000 — bits 12-51
```

### 9.5 Self-test exit-code convention

Every self-test follows this pattern:

```
    fn main() -> i32 {
        // Setup.
        let tbl = state_new(SomeTable);
        some_init(tbl);

        // Test 1: <what it checks>
        if some_check_fails { return 1; }

        // Test 2: <what it checks>
        if other_check_fails { return 2; }

        // ...

        return 0;   // all checks passed
    }
```

The exit code N > 0 means "check N failed" — a future CI failure
pinpoints the broken check by the exit code. Document each check in a
`// Test N: ...` comment above the `if`. See `arch/x86_64/pt.vuma`'s
self-test for the canonical form (4 checks, exit codes 1..4).

### 9.6 The `0 - N` negative-literal form

For negative errno returns, use `0 - N` instead of `-N` (see
[`kernel-architecture.md` §10.1](./kernel-architecture.md#101-the-0--1-negative-literal-workaround)):

```
    // DON'T:
    return -11;       // parser's signed-literal path — risky

    // DO:
    return 0 - 11;    // flatten_expr's BinOp::Sub arm — verified safe
```

This applies to any negative integer literal in return position. For
negative comparisons (`if x < -1`), use the positive form
(`if x < 0 - 1`) or refactor to avoid the negative literal.

---

## 10. Integration Testing

An integration test exercises multiple subsystems together — e.g. pipe +
signal, VFS + tmpfs + file_ops, scheduler + fork + wait. The kernel
doesn't have a separate "integration test" directory; integration tests
live alongside the unit self-tests in each module's `main()`, gated by
the multi-subsystem setup they perform.

### 10.1 Writing an integration test

The pattern is: allocate the states for every subsystem involved, init
each one, then exercise the cross-subsystem path. Example: a pipe +
signal integration test would:

```
    fn main() -> i32 {
        // ── Setup: pipe + signal ──
        let pipe = state_new(Pipe);
        pipe_init(pipe);
        let sig_tbl = state_new(SignalTable);
        signal_table_init(sig_tbl);

        // ── Test 1: write to pipe, signal the reader ──
        let buf = state_new(ByteBuf);
        buf.data[0] = 104;  // 'h'
        let n = pipe_write(pipe, buf, 1);
        if n != 1 { return 1; }
        let sret = signal_send(sig_tbl, 1, 1 /* SIGUSR1 */);
        if sret != 0 { return 2; }

        // ── Test 2: read from pipe, check byte round-trips ──
        let rbuf = state_new(ByteBuf);
        let r = pipe_read(pipe, rbuf, 1);
        if r != 1 { return 3; }
        if rbuf.data[0] != 104 { return 4; }

        return 0;
    }
```

The test exercises:
- `state_new(Pipe)` + `state_new(SignalTable)` + `state_new(ByteBuf)` (×2)
  — PMT allocation across multiple subsystems
- `pipe_init` + `signal_table_init` — multi-subsystem init
- `pipe_write` → `signal_send` → `pipe_read` — cross-subsystem data flow
- Byte round-trip through the pipe

The exit codes 1..4 pinpoint which check failed.

### 10.2 Multi-subsystem test patterns

The kernel's existing integration tests cover these combinations:

- **VFS + tmpfs + file_ops** (`fs/tmpfs.vuma`'s self-test): create a
  tmpfs file, write to it via `tmpfs_write`, close + reopen via
  `file_open`, read back via `tmpfs_read`, check round-trip. Exercises
  `InodeTable`, `DentryTable`, `FileTable`, `TmpfsData` together.

- **Process + scheduler + fork + wait** (`proc/wait.vuma`'s self-test):
  allocate `ProcessTable`, init parent + child tasks, set child to
  ZOMBIE state, call `sys_waitpid`, check reaped pid + status. Exercises
  `ProcessTable` + `Status` + the wait layer.

- **IPC + signal + pipe** (`ipc/pipe.vuma`'s self-test): write to a
  pipe, send a signal, read back. Exercises `Pipe` + `SignalTable`.

- **Syscall + trap + dispatch** (`syscall/dispatch.vuma`'s self-test):
  register a handler in `SyscallTable`, dispatch an out-of-range nr
  (expect -ENOSYS), dispatch a registered nr (expect 0 from the stub).
  Exercises `SyscallTable` + `SyscallArgs` + `syscall_dispatch`.

- **Sync + SMP** (`smp/smp.vuma`'s self-test): allocate `SmpState` +
  `Spinlock`, init both, simulate a CPU boot + IPI. Exercises `SmpState`
  + `Spinlock` + `IpiTable`.

### 10.3 Test harness integration

Integration tests run as part of the regular test harnesses:

- **`scripts/kernel_smoke.sh`** — runs `womb/kernel/kernel.vuma` (the
  most-integrated test, since `kernel.vuma` inlines `console.vuma` +
  `kmain` + future subsystems).
- **`scripts/kernel_parity.sh`** — runs the kernel + a subset of
  gold-standard tests across all 19 backends.
- **`tests/gold_standard/kernel_boot/`** — the kernel-boot smoke test
  (greps stdout for "vuma kernel: hello").
- **`tests/gold_standard/kernel_crypto/`** — the SHA-256 KAT test
  (exercises `crypto/api.vuma` end-to-end).

To add a new integration test, place it in the module whose self-test
you're extending. The test runs as part of that module's `main()`, so
it's automatically picked up by `kernel_smoke.sh` and `kernel_parity.sh`
when they compile + run that module.

### 10.4 Cross-arch integration testing

For tests that should pass identically on all arches (the gold-standard
suite), use `scripts/kernel_parity.sh`:

```
    ./scripts/kernel_parity.sh
    # Compiles + runs every gold-standard test × every backend.
    # Uses QEMU user-mode for non-x86_64 arches.
    # Exits 0 only if every backend passes every test.
```

A differential failure (test passes on x86_64 but fails on aarch64)
points to a codegen bug in one of the backends, not a kernel bug. Report
differential failures to the compiler team — the kernel code is
arch-agnostic by construction (modulo the per-arch `arch/` layer, which
is the only place arch-specific code lives).

### 10.5 Test-isolation rules

When writing an integration test, follow these rules to keep tests
isolated:

1. **Each test allocates its own states.** Don't share a State<T>
   between tests — the arena is bump-only, so each allocation is
   permanent, but the test logic should be independent. If test 1
   corrupts a state, test 2 shouldn't see the corruption.

2. **Each test has its own exit-code range.** Test 1 uses 1..N, test 2
   uses N+1..M, etc. Document the ranges in the `main()` header.

3. **Tests run in source order.** VUMA executes `main()` top-to-bottom;
   there's no parallelism. If test 2 depends on test 1's side effects
   (e.g. test 1 creates a file, test 2 reads it), document the
   dependency in a comment.

4. **Tests clean up after themselves (where possible).** The arena
   can't free, but stateful tables can: `task_free`, `inode_free`,
   `dentry_free`, `file_free`, `pipe_close`, etc. Call the cleanup
   function at the end of each test so the next test starts from a
   clean state.

### 10.6 Regression testing

When you fix a bug, add a regression test that would have caught it. The
kernel's regression tests live in `tests/gold_standard/` organized by
the wave that introduced them. For a bug fix in (say) `pmm.vuma`, add a
test file `tests/gold_standard/pmt_wave<N>/pmm_bug_<short_desc>.vuma`
that exercises the buggy code path and checks the fixed behavior.

The test file's header should document:
- The bug ID (or worklog reference).
- The buggy behavior (exit code N on the unfixed code).
- The fixed behavior (exit code 0 on the fixed code).
- The minimal reproduction (the smallest `.vuma` program that triggers
  the bug).

See `tests/gold_standard/arena_wave1/arena_overflow.vuma` for the
canonical regression-test format (added in K0 to catch the
`arena_alloc` bounds-check bug).
