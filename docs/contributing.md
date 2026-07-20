# Contributing to VUMA 2.0

Thanks for your interest in contributing to VUMA — the Verified-Unsafe Memory
Access language framework. VUMA 2.0 is **PMT-only**: every test in the suite
is written in PMT syntax (`layout` / `State<T>` / `state_new`), and the
legacy pointer dialect is no longer accepted. The same compiler builds the
**VWK** kernel — 75 PMT-pure `.vuma` files under `womb/kernel/` — and
contributions to the kernel follow the same patterns as contributions to
the compiler.

This document covers getting a dev environment running, following the code
style, writing PMT tests, adding new backends, contributing to the VWK
kernel, the VUMA code patterns every contributor should know, and landing a
pull request. For the full build / cross-compilation reference see
[`building.md`](building.md). For the kernel's architecture see
[`kernel-architecture.md`](kernel-architecture.md). For the kernel
developer's recipe book (adding syscalls, drivers, filesystems) see
[`kernel-developer-guide.md`](kernel-developer-guide.md).

---

## Table of Contents

1. [Getting Started](#1-getting-started)
2. [Code Style](#2-code-style)
3. [PMT-Only Test Policy](#3-pmt-only-test-policy)
4. [Adding a New Backend](#4-adding-a-new-backend)
5. [Adding a New Test](#5-adding-a-new-test)
6. [Contributing to the VWK Kernel](#6-contributing-to-the-vwk-kernel)
7. [VUMA Code Patterns](#7-vuma-code-patterns)
8. [Pull Request Process](#8-pull-request-process)
9. [Commit Messages](#9-commit-messages)

---

## 1. Getting Started

### Clone

```bash
git clone https://github.com/pkhairkh/vuma.git
cd vuma
```

The toolchain is pinned via [`rust-toolchain.toml`](../rust-toolchain.toml)
to **`nightly-2026-03-01`** (with `rustfmt`, `clippy`, `rust-src`, and the
`aarch64-unknown-linux-gnu` / `aarch64-unknown-none` targets). `cargo`
auto-installs it on first use; to install explicitly:

```bash
rustup toolchain install nightly-2026-03-01 \
  --component rustfmt,clippy,rust-src \
  --target aarch64-unknown-linux-gnu \
  --target aarch64-unknown-none
```

### Install QEMU user-mode

Cross-backend testing needs QEMU user-mode emulators. On Debian/Ubuntu:

```bash
sudo apt-get install qemu-user qemu-user-static
```

For no-root / sandboxed-CI environments, see
[`building.md` §7 QEMU Installation](building.md#7-qemu-installation) for
the static-binary tarball approach. The 7 executable backends in the standard
sweep are `x86_64` (native), `aarch64`, `riscv64`, `arm32`, `ppc64le`,
`mips64`, `s390x`, `loongarch64`, plus `wasm32` under `wasmtime`.

### Build

The standard test driver is `compile_dump`. Build it with the `release-fast`
profile (the profile the test runner uses):

```bash
cargo build --profile release-fast --bin compile_dump
# → target/release-fast/compile_dump
```

> On a constrained host (≤ 4 GiB RAM) `release-fast` OOMs. Use the
> dev-profile workaround in [`building.md` §2](building.md#2-building-the-compiler).

### Run tests

The end-to-end cross-backend runner is
[`scripts/vuma_test_suite.sh`](../scripts/vuma_test_suite.sh). It builds
`compile_dump`, walks `tests/gold_standard/`, compiles every `.vuma` file on
every backend, runs each under QEMU / wasmtime, and checks the exit code
against the `// Expected exit code: N` header.

```bash
scripts/vuma_test_suite.sh --workers 8 --fresh --verify
```

For a single quick check (one file, one backend, no QEMU):

```bash
./target/release-fast/compile_dump diag x86_64 \
    tests/gold_standard/pmt_wave2/two_states.vuma
```

For unit / integration tests: `cargo test --workspace` or
`cargo test -p vuma-codegen`.

For the kernel smoke test:

```bash
bash scripts/kernel_smoke.sh
# Expected: "PASS: kernel boots, prints banner, exits 0"
```

For the 19-backend kernel parity sweep:

```bash
bash scripts/kernel_parity.sh            # full sweep (~10 minutes)
bash scripts/kernel_parity.sh --quick    # arena_basic + kernel smoke only
```

### Verify the setup

A clean checkout should pass these three from the repo root:

```bash
cargo build --workspace                       # compiles
cargo fmt --all -- --check                    # no formatting diffs
cargo clippy --workspace -- -D warnings       # no clippy warnings
```

---

## 2. Code Style

### Rust nightly

All code targets the pinned `nightly-2026-03-01` toolchain. Do not introduce
features that require a newer nightly, and do not gate code on `#[stable]` —
the workspace is nightly-only by design.

### Crate-root clippy allows

Each crate root (`src/main.rs`, `src/*/src/lib.rs`) carries a single
crate-wide clippy allow-list:

```rust
#![allow(clippy::manual_range_contains, clippy::map_unwrap_or,
         clippy::unnecessary_cast,    clippy::redundant_closure,
         clippy::if_same_then_else,   clippy::collapsible_if,
         clippy::useless_format)]
```

When adding a new crate root, copy this line verbatim. Do **not** scatter
`#[allow(clippy::...)]` attributes inside modules — if a lint truly must be
suppressed crate-wide, add it to the crate-root allow-list. Otherwise fix the
code. The strict clippy gate `cargo clippy --workspace -- -D warnings` runs
on every PR.

### rustfmt

Formatting is governed by [`rustfmt.toml`](../rustfmt.toml):

- Maximum line width: **100 columns**
- Indentation: **4 spaces, no tabs**
- Edition: **2021**

Run `cargo fmt --all` before every commit.

### Naming and conventions

- `UpperCamelCase` for types, traits, enum variants.
- `snake_case` for functions, methods, variables, modules.
- `SCREAMING_SNAKE_CASE` for `const` and `static`.
- Public items need a `///` doc comment; private items should where the
  intent is non-obvious.
- Prefer `&str` / `&[T]` over `String` / `Vec<T>` in function signatures.
- No `unsafe` blocks without a `// SAFETY:` comment explaining the invariant.

### Zero external dependencies

The workspace depends on **no external crates** — only `std` and the internal
`vuma-*` path crates. There is no `serde`, no `clap`, no `libc`, no `rayon`.
Do not add `[dependencies]` entries for external crates; if you need a
capability, hand-write it. `Cargo.lock` after your change should contain only
`vuma-*` packages.

This rule applies to the standard library (`womb/`) too. The womb tree is
written in PMT syntax and uses no `extern "C"` other than the kernel's own
hosted trampolines; it cannot pull in third-party code even if it wanted to.

---

## 3. PMT-Only Test Policy

VUMA 2.0 is **PMT-only**. All new tests MUST be written in PMT syntax:

- Declare a **`layout`** — a pure type-level description of a record (fields,
  types, offsets, size, alignment). A layout does not allocate storage.
- Construct a **`State<T>`** (typed view over the program's single backing
  memory buffer) with **`state_new(LayoutName)`**. This carves a slot of the
  buffer with the layout's size and alignment; the slot's address is not
  exposed to the program.
- Access fields with `s.field` — reads and writes are statically known to be
  in-bounds by the type checker.

A minimal PMT test:

```vuma
// two_states — PMT Wave 2: two independent states
// Expected exit code: 30
//
// Allocates two Points, sets x=10 on the first and x=20 on the second,
// returns their sum (30). Verifies that each state has its own buffer
// and field writes don't cross-contaminate.

layout Point = { x: u32, y: u32 }

fn main() -> i32 {
    let p = state_new(Point);
    let q = state_new(Point);
    p.x = 10;
    q.x = 20;
    return p.x + q.x;
}
```

### What is NOT accepted

- **Pointer syntax** — `*ptr = expr`, `&x`, `allocate(n)`, `free(p)` — is
  legacy and not accepted by the 2.0 test runner. Pointer-dialect programs
  from 1.x have either been migrated to PMT (see `pmt_wave7/` for migrations
  of `concurrency/conc_swap.vuma` and friends) or removed.
- **Tests that bypass the type checker** — every PMT test must type-check
  cleanly; the IVE runs only the three state verifiers (`state_read`,
  `state_write`, `state_transform`).
- **Tests without an `// Expected exit code: N` header** — the runner reads
  this line and compares it against the process exit status.

If you are migrating a legacy pointer test to PMT, put the result under the
appropriate `pmt_wave*` directory (see
[`building.md` §5](building.md#5-test-categories)).

> Note: the `womb/crypto/` and `womb/net/` libraries still use the legacy
> pointer dialect (`allocate(...)`, `*ptr`). These are pre-PMT library
> modules that have not yet been migrated. New code in `womb/kernel/` is
> always PMT-pure; new code in `womb/lib/` should be PMT-pure whenever
> feasible. See [`womb/crypto/README.md`](../womb/crypto/README.md) and
> [`womb/net/README.md`](../womb/net/README.md) for the migration status.

---

## 4. Adding a New Backend

VUMA 2.0 currently ships 19 backends (see the `BackendKind` enum in
[`src/codegen/src/backend.rs`](../src/codegen/src/backend.rs)). To add a new
one:

### Step 1 — implement the `Backend` trait

Add a new module `src/codegen/src/<arch>.rs` and implement the `Backend`
trait (defined in `backend.rs`). The trait requires:

| Method                  | Responsibility                                             |
|-------------------------|------------------------------------------------------------|
| `target_info()`         | Return this backend's target info (pointer width, ABI, …)  |
| `allocate_registers()`  | Allocate physical registers for an IR function             |
| `encode_function()`     | Encode one allocated function into machine code bytes      |
| `encode_program()`      | Encode an allocated program into ELF / .wasm / raw binary  |
| `return_stub()`         | Minimal return stub (`RET`, `mov eax,0; ret`, `end`, …)    |
| `trampoline(addr)`      | Trampoline that jumps to `entry_addr`                      |
| `disassemble(bytes, addr)` | Disassemble `bytes` at virtual address `addr`           |
| `name()`                | Human-readable name (e.g. `"aarch64"`)                     |

You will also need a `LatencyTable` entry for the new architecture (see
[`src/codegen/src/scheduler.rs`](../src/codegen/src/scheduler.rs)).

### Step 2 — add to `BackendKind`

Add a new variant to the `BackendKind` enum in `backend.rs`, and extend the
`isa_name()`, `from_str()`, and `qemu_binary()` match arms. Also extend the
`backend_from_name` helper in [`src/bin/compile_dump.rs`](../src/bin/compile_dump.rs)
so the CLI accepts the new backend name.

### Step 3 — add the QEMU mapping to the test runner

Edit [`scripts/kernel_parity.sh`](../scripts/kernel_parity.sh) and
[`scripts/vuma_test_suite.sh`](../scripts/vuma_test_suite.sh) and add a new
entry to the `QEMU_MAP` array (in `kernel_parity.sh`) or the `binfmt_misc`
`entries` array (in `vuma_test_suite.sh`). Each entry has the form
`backend:qemu_binary` or `name|qemu_binary|magic_hex|mask_hex`:

```bash
"qemu-<arch>|qemu-<arch>|<elf_magic>|<elf_mask>"
```

The magic and mask are the ELF header bytes that let `binfmt_misc` recognise
the architecture. If the new backend is wasm-based (not QEMU), extend
[`scripts/wasm32_runner.py`](../scripts/wasm32_runner.py) instead.

### Step 4 — add tests and verify

Add at least one PMT test under `tests/gold_standard/<arch>/` (or under an
existing category if the test exercises a general feature) with an
`// Expected exit code: N` header. Run the new backend on the full
gold-standard suite to confirm agreement with the other 18 backends:

```bash
scripts/vuma_test_suite.sh --workers 8 --backends <arch> --verify
```

Also confirm the kernel compiles on the new backend:

```bash
./target/release-fast/compile_dump womb/kernel/kernel.vuma \
    /tmp/kernel-<arch>.bin <arch> --verify
```

The full backend-adding guide (with worked examples for x86_64, aarch64, and
riscv64) is in [`src/README.md` §Adding a new backend](../src/README.md).

---

## 5. Adding a New Test

### Step 1 — pick a category

Tests live under [`tests/gold_standard/`](../tests/gold_standard/) in
category directories. Pick the most specific one (see
[`building.md` §5](building.md#5-test-categories) for the full list). If the
test exercises a PMT-specific feature, use the matching `pmt_wave*` directory.
If it exercises the arena runtime, use `arena_wave*`. If it exercises the
FFI marshal matrix, use `ffi_wave*`.

### Step 2 — write the `.vuma` file

Every test file MUST begin with the standard header:

```
// <name> — <one-line description>
// Expected exit code: <N>
//
// <longer description / what this tests>
//
// VUMA Key Concepts:
//   - <bullet list of PMT features exercised>
```

The runner parses the `Expected exit code:` line; without it the test is
skipped. The body is PMT-only — see [§3 PMT-Only Test Policy](#3-pmt-only-test-policy).
For tests that exercise architecturally-unavailable functionality (e.g.
`fork` on wasm32), add a `skip_on: <backend>,<backend>` header.

### Step 3 — verify locally

Build `compile_dump` and run the test under `x86_64` (native, fastest) plus
at least one cross-backend (e.g. `aarch64` via QEMU):

```bash
cargo build --profile release-fast --bin compile_dump

./target/release-fast/compile_dump diag x86_64 \
    tests/gold_standard/<category>/<name>.vuma

./target/release-fast/compile_dump diag aarch64 \
    tests/gold_standard/<category>/<name>.vuma qemu-aarch64
```

The exit code printed by `compile_dump` must match the `// Expected exit code:`
header on **every** backend. If any backend disagrees, do not commit the test —
file an issue against the offending backend instead.

### Step 4 — run the full category

```bash
scripts/vuma_test_suite.sh --workers 8 --backends x86_64,aarch64,riscv64 --verify
```

See [`tests/README.md`](../tests/README.md) for the test-suite layout and
the runner-script reference.

---

## 6. Contributing to the VWK Kernel

The VWK kernel under [`womb/kernel/`](../womb/kernel/) is a complete PMT-pure
kernel written in VUMA's own syntax. It is built across **13 waves (K0–K12)**,
all complete; K11 is the bare-metal parity sweep (real asm syscall stubs per
arch, QEMU system-mode boot). Future work (K13+) will replace the stub
inventory (no-op trap dispatch, no-op syscall indirect-call, AES-NI trampoline
stubs, etc.) with real implementations.

The full kernel architecture is in [`kernel-architecture.md`](kernel-architecture.md).
The per-module inventory is in [`womb/kernel/README.md`](../womb/kernel/README.md).
The kernel developer's recipe book (adding syscalls, drivers, filesystems,
PMT kernel code; do/don't examples; IVE failure debugging recipe) is in
[`kernel-developer-guide.md`](kernel-developer-guide.md). The porting guide
(worked example: x86_64) is in [`kernel-porting-guide.md`](kernel-porting-guide.md).

### 6.1 The wave-based workflow

Kernel work is organized into **waves**, each domain-scoped and code-specific.
Each wave has a `Task ID: K<NN>` marker in the worklog and a contract that
spells out the deliverables, the test gate, and the K13+ forward-looking
notes. The 13 waves so far:

| Wave | Scope | Key subsystems delivered |
|------|-------|--------------------------|
| **K0** | Arena foundation | `arena_alloc` runtime bounds check, `__arena_overflow` trap on all 19 backends |
| **K1** | Console + kernel entry | `console.vuma`, `kernel.vuma` (`main → kmain`), hosted-mode `trampoline.vuma` + `bootinfo.vuma` |
| **K2** | Memory management | `pmm` (buddy allocator), `vmm` (page-table walk), `kmalloc` (slab), `mmap` (VMA tracking), per-arch `mm_trampoline` + `pt` for x86_64/aarch64/riscv64 |
| **K3** | Trap + IRQ + syscall | `trap_trampoline` (TrapFrame layout), `trap`/`irq` dispatchers, `syscall/{abi,table,dispatch}` + `handlers/{io,mm,proc}` |
| **K4** | Process + scheduler | `task` (TCB + ProcessTable), `scheduler` (CFS-like runqueue), `switch` (context switch), `fork`/`exec`/`wait`/`exit` |
| **K5** | VFS + filesystems | `inode`/`dentry`/`file`/`namei`/`mount`/`file_ops`, `tmpfs`, `initramfs` (cpio parser) |
| **K6** | Drivers + TTY | `uart` (8250 + PL011), `char` (cdev framework), `virtio_net`, `tty/{console,line_discipline,vt100}` |
| **K7** | IPC | `pipe` (ring buffer), `signal`, `shm`, `futex`, `waitq` |
| **K8** | Sync + SMP | `spinlock`, `mutex`, `semaphore`, `rwlock`, `smp`/`percpu`/`ipi` |
| **K9** | Networking | `socket`, `sk_buff`, `tcp` (10-state machine), `dns`, `http` |
| **K10** | Crypto | `crypto/{api,aes,sha,asym,hw_trampoline}` — AES-NI / SHA-Ext trampolines |
| **K11** | Bare-metal parity | real asm syscall stubs per arch, QEMU system-mode boot |
| **K12** | Panic + power + shell | `panic`/`kmsg` (ring buffer), `power/pm` (halt/wfi), `shell` |

When you add a new kernel subsystem, **claim the next K-wave** in the
worklog. Write the contract first (what the subsystem does, what its test
gate is, what stubs are allowed, what K13+ will replace), then implement.

### 6.2 How to add a kernel subsystem

The 6-step recipe (full version in
[`kernel-developer-guide.md`](kernel-developer-guide.md)):

1. **Pick the wave and write the contract.** Document what the subsystem
   does, which existing modules it depends on (and re-declares layouts from),
   and which stubs you'll need.
2. **Design the layout.** Every kernel subsystem has at least one
   `layout Foo = { ... }` declaration that holds its state. Use **flat byte
   arrays** + pack/unpack helpers for any "array of u32/u64" — see
   [§7 VUMA Code Patterns](#7-vuma-code-patterns) below. Use sentinel values
   (`0` for FREE, `256` for EMPTY in 256-slot tables, `64` for FULL in
   64-slot tables, `255` for EOL in u8 free-lists) for "not found" returns.
3. **Use the init-style API.** The caller allocates the state with
   `state_new(Layout)` and passes it by reference to an init function that
   populates its fields. Do **not** return `State<T>` from a function — the
   codegen does not propagate State-typedness through return values (see
   [§7](#7-vuma-code-patterns)).
4. **Re-declare every layout you consume, byte-identically.** VUMA has no
   `import` statement yet. The `LayoutRegistry` catches drift at compile
   time. Copy-paste from the canonical source — do not paraphrase.
5. **Write a `fn main() -> i32` self-test.** Every kernel module ends with a
   self-test that exercises the module's API surface. Use the convention
   `if <check N fails> { return N; }` so a future CI failure pinpoints the
   broken check by exit code.
6. **Add the module to `kernel_parity.sh`'s `KERNEL_MODULES` array** if you
   want it covered by the parity sweep. Add a per-module self-test command
   in the file header so future contributors can re-run it.

### 6.3 Init-style API pattern

Because the codegen does not propagate `State<T>`-typedness through function
return values, every kernel subsystem uses the **init-style API**: the
caller allocates the state with `state_new(Layout)` and passes it by
reference to an init function that populates its fields.

```vuma
// Caller allocates:
let pmm = state_new(PmmState);
let pool = state_new(FlatPool);
// Caller passes by reference:
pmm_init(pool, pmm, mem_start, mem_size);
// Caller reads fields back:
let page = pmm_alloc(pmm, order);     // returns u64 page-frame address
```

This pattern is documented in `womb/kernel/mm/pmm.vuma::"Why init-style?"`
and is used by `pmm_init`, `vmm_init`, `trap_frame_init`,
`task_init_for_switch`, `syscall_args_from_frame`, `kmsg_init`, `pm_init`,
and every other stateful subsystem. A historical data point: K3e's
`syscall_args_from_frame` was originally written in return-style and the
codegen emitted four `WARNING: unsupported FieldAccess (not state-typed)`
diagnostics; the fix was to flip to init-style. The convention has been
canonical ever since.

### 6.4 Flat byte array pattern

The codegen only lowers `state.array[idx]` correctly for `[u8; N]` arrays.
For `[u16; N]`, `[u32; N]`, or `[u64; N]` arrays, the indexed-access path
goes through a code path that the IVE verifiers don't fully understand —
accesses compile but read back wrong values. The kernel's convention is to
**store every "array of N u32/u64" as a parallel flat `[u8; N * width]`
array** with pack/unpack helpers:

```vuma
layout InodeTable = {
    ino:  [u8; 1024],   // 128 inodes × 8 bytes, packed LE
    mode: [u8; 128],    // 128 inodes × 1 byte
    size: [u8; 1024],   // 128 inodes × 8 bytes, packed LE
    // ...
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
`pmm.vuma::pool_get_base`, `irq.vuma::irq_get_handler`,
`syscall/table.vuma::syscall_table_get`, `task.vuma::pt_get_vruntime`, and
every other kernel table module.

### 6.5 Pack/unpack helpers

A canonical pair of helpers for u64-in-`[u8; N]`:

```vuma
// Read 8 bytes from tbl.field at byte offset `off` as a little-endian u64.
fn unpack_u64_le(buf: [u8; 1024], off: u32) -> u64 {
    let v: u64 = 0;
    let i = 0;
    while i < 8 {
        let sh = i * 8;
        let b = buf[off + i] as u64;
        v = v + (b << sh);
        i = i + 1;
    }
    return v;
}

// Write u64 `val` to tbl.field at byte offset `off` as little-endian.
fn pack_u64_le(buf: [u8; 1024], off: u32, val: u64) {
    let i = 0;
    while i < 8 {
        let sh = i * 8;
        let b = (val >> sh) & 255;
        buf[off + i] = b as u8;
        i = i + 1;
    }
}
```

For u32-in-`[u8; N]`, the same loop with `while i < 4`. For big-endian (used
by `aarch64_be`, `mips64be`, `ppc64`, `sparc64`, `s390x`), reverse the byte
order (iterate `i = width - 1; i >= 0; i = i - 1`). K13+ will fix the
codegen to support `[u64; N]` directly and these helpers will collapse to
`tbl.field[idx]`.

### 6.6 Sentinel conventions

The kernel uses **out-of-band sentinel values** to signal "empty", "full",
"end-of-list", or "not found" without incurring an extra error channel. The
sentinels are picked so they can never be a valid index/pointer.

| Sentinel | Value | Meaning                          | Used by                                   |
|----------|-------|----------------------------------|-------------------------------------------|
| `EMPTY`  | 256   | Empty queue / table-full         | `pmm.vuma::pmm_get_free_list`, `waitq.vuma`, `pipe.vuma`, `tmpfs.vuma`, `task_alloc` |
| `FULL`   | 64    | Slot pool exhausted              | `tmpfs.vuma`, `futex.vuma`, `shm.vuma`, `inode_alloc`/`dentry_alloc`/`file_alloc` |
| `EOL`    | 255   | End-of-list marker in u8 slots   | `sk_buff.vuma::free_list`, `kmsg.vuma` ring wrap (`& 255` mask) |
| `FREE`   | 0     | Slot is free / unallocated       | `task.vuma::ProcessTable.states`, `irq.vuma::IrqTable.handlers`, `syscall/table.vuma::SyscallTable.handlers`, `vfs/inode.vuma::InodeTable.ino` |

The rule: pick the sentinel so it cannot appear as a valid index. For a
256-slot table, 256 is unambiguous. For a 64-slot table, 64 is unambiguous.
For a u8 free-list head, 255 (or any value ≥ table size, up to 255) is
unambiguous. See [`kernel-architecture.md` §14](kernel-architecture.md) for
the full convention.

### 6.7 Self-test requirements

Every `.vuma` file in `womb/kernel/` ends with a `fn main() -> i32`
self-test that exercises the module's API surface. The convention:

```vuma
fn main() -> i32 {
    // Test 1: <first check>
    if <check1 fails> { return 1; }
    // Test 2: <second check>
    if <check2 fails> { return 2; }
    // ...
    return 0;
}
```

So a future CI failure pinpoints the broken check by the exit code. The
self-test must:

- Allocate any state the module needs via `state_new(...)`.
- Initialize it via the module's init function.
- Exercise at least one positive path (the happy case).
- Exercise at least one negative path (sentinel return, error code, etc.).
- End with `return 0;` on success.

Run a module's self-test:

```bash
./target/release-fast/compile_dump womb/kernel/<subsys>/<module>.vuma \
    /tmp/<module>.bin x86_64 --verify
/tmp/<module>.bin; echo "exit=$?"
# Expected: "IVE: Pass passed=1 failed=0 total=1" + exit=0
```

### 6.8 IVE failure debugging

When `compile_dump ... --verify` reports `IVE: Fail`, the next line names the
verifier that tripped:

```
IVE: Fail passed=0 failed=1 total=1
  StateWriteVerifier: write to invalidated State<Console> in console_flush
    at line 92: c.len = 0;
    prev invalidate: extern call to write() at line 91
```

The diagnostic names the state, the failing line, and the prior invalidate
site. The fix is almost always one of:

- **(a) Add `#[borrow]` to the offending extern.** If the caller needs to
  access the state after the foreign call, the extern parameter must be
  declared `#[borrow]` so the marshal module keeps the state alive.
- **(b) Flip a return-style helper to init-style.** If the helper returns
  `State<T>` and the caller accesses fields on the result, switch to the
  caller-allocates pattern.
- **(c) Split the function.** Put the extern call in a different function
  than the post-call field access. The invalidate scope is per-function.

See [`kernel-developer-guide.md` §6](kernel-developer-guide.md) for the full
debugging recipe.

---

## 7. VUMA Code Patterns

Quick reference for the patterns every VUMA contributor should know. These
are the same patterns used throughout `womb/kernel/` and the gold-standard
test suite.

### 7.1 Init-style API

**Problem:** The codegen does not propagate `State<T>`-typedness through
function return values. A binding `let s = make_state()` where `make_state`
returns `State<T>` is NOT registered as state-typed in the caller;
subsequent `s.field` accesses silently return 0 with a
`WARNING: unsupported FieldAccess (not state-typed)` from `flatten_expr`.

**Solution:** The caller allocates the state via `state_new(...)` (which
marks the binding as state-typed) and passes it by reference to a function
that populates it in place.

```vuma
// DON'T (return-style — caller's field access silently returns 0):
fn make_console() -> State<Console> {
    let c = state_new(Console);
    c.len = 0;
    return c;
}
fn main() -> i32 {
    let c = make_console();
    c.len = c.len + 1;   // WARNING: unsupported FieldAccess
    return c.len as i32; // returns 0, not 1
}

// DO (init-style — caller's field access works):
fn console_init(c: State<Console>) {
    c.len = 0;
}
fn main() -> i32 {
    let c = state_new(Console);
    console_init(c);
    c.len = c.len + 1;   // OK
    return c.len as i32; // returns 1
}
```

### 7.2 Flat byte arrays

**Problem:** The codegen only lowers `state.array[idx]` correctly for
`[u8; N]` arrays. `[u16; N]`, `[u32; N]`, `[u64; N]` compile but read back
wrong values.

**Solution:** Store every "array of N u32/u64" as a parallel flat
`[u8; N * width]` array with pack/unpack helpers (see [§6.5](#65-packunpack-helpers)).

### 7.3 If-expressions

VUMA supports both statement-form `if`/`else` and value-typed `if`-expressions:

```vuma
// Statement form (no value):
if x > 0 {
    y = 1;
} else {
    y = 2;
}

// Expression form (yields a value):
let y = if x > 0 { 1 } else { 2 };
```

Both forms lower to the same SCG branch node; the expression form threads
the value through a phi-style merge. Use the expression form when a branch
is purely value-producing (no side effects).

### 7.4 Atomic CAS

Atomic compare-and-swap is the building block for `spinlock.vuma`,
`mutex.vuma`, and the lock-free data structures in `womb/collections/`. The
pattern:

```vuma
// Spin until we win the CAS:
let locked = 0;
while locked == 0 {
    let cur = atomic_load(&lock, Ordering::Acquire);
    if cur != 0 {
        continue;   // already locked
    }
    locked = atomic_cas(&lock, 0, 1, Ordering::AcqRel);
}
// critical section ...
atomic_store(&lock, 0, Ordering::Release);
```

`atomic_cas` returns the previous value (0 on success, non-zero on failure).
The gold-standard `atomics/` category has 35 tests covering CAS patterns.

### 7.5 State-as-address cast

The `State<T> as Address` cast is the only sanctioned "lossy" ownership
transfer in the language. It hands the state buffer's base address to an FFI
callee that expects a raw pointer:

```vuma
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

The `buf` field is at offset 0 by convention so the cast yields `&buf[0]`.
This pattern is uniform across `console.vuma`, `kmsg.vuma`, `panic.vuma`,
and `hw_trampoline.vuma` — every byte of kernel I/O traverses this one cast.

### 7.6 Sentinel values

Sentinels signal "empty", "full", "end-of-list", or "not found" without an
extra error channel. See [§6.6](#66-sentinel-conventions) for the full
table. The rule: pick the sentinel so it cannot appear as a valid index.

### 7.7 Negative literals via `0 - N`

VUMA's integer-literal path has subtle width-extension behavior at the
64-bit boundary. The kernel's convention is to **always write negative
numbers as `0 - N`** (e.g. `0 - 1` for -1, `0 - 11` for -EAGAIN, `0 - 38`
for -ENOSYS) rather than as the literal `-1` / `-11` / `-38`:

```vuma
// DON'T:
return -1;          // parser's signed-literal path — risky

// DO:
return 0 - 1;       // flatten_expr's BinOp::Sub arm — verified safe
```

The `0 - N` form lowers to the same machine code (the codegen's constant
folder collapses it). The safety gain is that the expression goes through
`flatten_expr`'s `BinOp::Sub` arm rather than the lexer's negative-number
path.

### 7.8 Decimal literals in self-tests

VUMA accepts `0x..` hex literals but the parser's hex path shares code with
the decimal path through `parse_int_radix`, which has subtle width-extension
behavior at the 64-bit boundary. The kernel's convention is to **use decimal
literals in self-tests** (`4096`, not `0x1000`; `17592186028032`, not
`0x000FFFFFFFFFF000`). The decimal form lowers to identical machine code;
the safety gain is avoiding the hex path entirely. This is enforced by the
K2c / K3d / K4a / K5a / K10a contracts' "IMPORTANT: use decimal constants"
rule.

### 7.9 No struct literal

VUMA has no struct-literal syntax (`Layout { field: value, ... }`). State
must be allocated with `state_new(Layout)` (zero-initialized) and then
populated field-by-field. There is no way to "construct" a state inline.
This forces the init-style API pattern; it's not really a workaround so much
as a language-design choice that aligns with the PMT discipline (no implicit
allocation sites).

### 7.10 Forward references

Unlike C, VUMA allows forward references to functions: a function can call
another function declared later in the file. The parser does a two-pass scan
(pass 1 collects all fn signatures into the symbol table, pass 2 resolves
call sites). Layouts, however, MUST be declared before the first function
that uses them — the layout registry is single-pass.

---

## 8. Pull Request Process

### Before opening a PR

1. `cargo fmt --all` — apply formatting.
2. `cargo clippy --workspace -- -D warnings` — no clippy warnings.
3. `cargo test --workspace` — unit / integration tests pass.
4. `scripts/vuma_test_suite.sh --workers 8 --verify` — gold-standard suite
   passes on every backend you touched.
5. `bash scripts/kernel_smoke.sh` — kernel boots, prints banner, exits 0
   (required for any PR that touches `womb/kernel/`, the codegen, or the
   parser).
6. `bash scripts/kernel_parity.sh --quick` — quick parity check (full sweep
   for kernel-touching PRs).
7. No external dependencies added (`Cargo.lock` contains only `vuma-*`).
8. New public API has a `///` doc comment and, where reasonable, a test.
9. Bug-fix PRs include a regression test that fails before the fix.
10. New kernel modules end with a `fn main() -> i32` self-test.

### What CI runs

Every PR targeting `main` runs:

- **Build** — `cargo build --workspace` (matrix: Ubuntu x86_64 + macOS aarch64).
- **Lint** — `cargo fmt --all -- --check` + `cargo clippy --workspace -- -D warnings`.
- **Unit tests** — `cargo test --workspace` (13 integration test files under
  `tests/`: `backend_latency_tests.rs`, `egraph_extraction_tests.rs`,
  `ive_loop_tests.rs`, `latency_table_tests.rs`, `loop_depth_tests.rs`,
  `loop_unroll_tests.rs`, `lto_tests.rs`, `parallel_codegen_tests.rs`,
  `pgo_tests.rs`, `property_tests.rs`, `provenance_tests.rs`,
  `scheduler_tests.rs`, `verification_tests.rs`).
- **Cross-compile** — builds for 8 targets (x86_64, aarch64, riscv64gc, armv7,
  mips64, powerpc64, loongarch64, wasm32).
- **Gold-standard** — `scripts/vuma_test_suite.sh --workers 8 --verify` across
  all 18 QEMU backends + `wasm32` under `wasmtime` (program count is in
  `tests/gold_standard/manifest.json`).
- **Kernel smoke** — `scripts/kernel_smoke.sh` (boots `womb/kernel/kernel.vuma`
  on x86_64).
- **Kernel parity** — `scripts/kernel_parity.sh` (full 19-backend sweep:
  190 gold checks + 76 kernel module compiles).
- **KAT tests** — `scripts/run_real_kat.sh` (213 cross-architecture crypto
  known-answer tests; the standalone `run_all_kat.sh` runner was removed
  during the 2026-07 cleanup).
- **Proof verify** — `bv_verify`, `proof_artifacts`, `proof_log` subsets.

All of them must be green before merge.

### Squash before merge

PRs are **squash-merged** — one commit per PR on `main`. Your branch history
is not preserved; make sure the squashed commit message follows the format
below. If your PR addresses multiple unrelated concerns, split it into
separate PRs first.

### Review expectations

- Reviewers will check that PMT tests use `layout` / `State<T>` / `state_new`
  only — no pointer syntax (see [§3](#3-pmt-only-test-policy)).
- New backends must agree with the existing 18 on the full gold-standard
  suite (see [§4](#4-adding-a-new-backend)).
- New kernel modules must follow the init-style API, use flat byte arrays
  for u32/u64 tables, use sentinel values correctly, and end with a
  self-test (see [§6](#6-contributing-to-the-vwk-kernel)).
- Large architectural changes should be discussed in an issue first.
- The default answer to "can we add an external crate?" is "no, reimplement
  it" (see [§2 Zero external dependencies](#2-code-style)).

---

## 9. Commit Messages

Use the conventional, imperative style:

```
<area>: <short imperative summary>

<optional body explaining why, not what>
```

`<area>` is the affected crate or subsystem — `codegen`, `parser`, `ive`,
`bd`, `proof`, `scg`, `cor`, `package`, `docs`, `ci`, `tests`, or one of
the kernel subsystems (`kernel-mm`, `kernel-proc`, `kernel-vfs`,
`kernel-trap`, `kernel-ipc`, `kernel-sync`, `kernel-smp`, `kernel-net`,
`kernel-crypto`, `kernel-tty`, `kernel-shell`, `kernel-panic`,
`kernel-power`, `kernel-arch`). Keep the summary under 72 characters.
Reference issues in the body (`Closes #123`, `Refs #456`), not the summary.
Examples:

```
codegen: fix AArch64 shift encoding for imm=0
parser: recover from missing `;` after return expr
ive: cache interprocedural escape results per callsite
kernel-mm: add mmap VMA tracking + sys_munmap
kernel-crypto: wire AES-NI encrypt_block trampoline
docs: rewrite building.md for VUMA 2.0 PMT-only
```

For multi-line bodies, wrap at 72 columns and separate the body from the
summary with a blank line. The squash-merge commit message should follow
the same format.

---

## Questions

- Bugs and feature requests: [GitHub Issues](https://github.com/pkhairkh/vuma/issues)
- Repository: <https://github.com/pkhairkh/vuma>
- License: MIT (see [`LICENSE`](../LICENSE))

When in doubt, match the surrounding code. The codebase is its own style
guide. For kernel questions specifically, read
[`kernel-architecture.md`](kernel-architecture.md) and
[`womb/kernel/README.md`](../womb/kernel/README.md) first — most of the
"why is X like this" answers are there.
