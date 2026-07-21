# VUMA — Programs as Memory Transformations

**VUMA** (Verified-Unsafe Memory Access) is a systems programming language
where every program is a composition of typed *state transforms* over a single
memory buffer. Pointers, allocation, and deallocation are not features the
compiler reluctantly checks — they are **hard parse errors**. `allocate`,
`free`, `*ptr`, and `&x` never reach the type checker; the lexer rejects them.
What remains is **PMT** (Programs as Memory Transformations): a `layout`
describes the byte shape of a buffer, a `State<T>` is a typed view of the
program-wide memory buffer, and a `transform` is a pure function
`State<T> -> State<U>` that reads fields, computes new values, and writes
fields back. Memory safety becomes a structural type-checking property, not a
constraint solver over an unbounded heap graph. VUMA compiles PMT source
through a full O2 pipeline — SCG construction, monomorphization, closure
conversion, e-graph equality saturation, instruction scheduling, LICM,
escape analysis + SROA, vectorization, loop unrolling — to **19 backends (7 executable + 12 compile-only)
backends** (x86_64 through wasm32, including big-endian variants), under a
single-buffer runtime with zero per-state `malloc`/`free`. The same compiler
also builds **VWK** (the Vuma Womb Kernel), a kernel written entirely in
VUMA's own PMT syntax — 75 `.vuma` files across 13 waves (K0–K12) covering
memory management, scheduling, VFS, IPC, networking, crypto, and a shell.

---

## What is VUMA?

VUMA is **PMT** (Programs as Memory Transformations). A program is a sequence
of *transforms* on typed memory buffers. A `layout` describes the byte shape
of a buffer; a `State<T>` is a typed view of the program's single program-wide
memory buffer; a `transform` is a pure function `State<T> -> State<U>` that
reads fields, computes new values, and writes fields. The runtime never
allocates or frees per-object — the program-wide `Alloc` is sized once from
the union of all states, and each `state_new(Layout)` lowers to an `Offset`
into that one buffer.

Pointers are eliminated because they collapse memory safety into a runtime
problem. With pointers, the compiler must prove five invariants over an
unbounded heap graph: **liveness** (no use-after-free), **exclusivity** (no
aliasing during mutation), **cleanup** (no double-free), **origin** (no
uninitialized reads), and **interpretation** (no type-punning). Five
invariants over a heap graph is a constraint problem — solvable, but slow,
and any gap is a CVE. VUMA replaces that graph with a fixed set of structural
type checks on a single typed buffer.

The result is that memory safety is a type-checking property. A
`state.field` read is type-checked against the layout's field set — a read of
a non-existent field is a compile error, not a runtime crash. A write after a
state has been consumed by a transform is a linearity violation caught by the
`state_write` verifier. Bounds on array access reduce to
`offset + count * elem_size ≤ buffer_size`, a linear-arithmetic obligation the
`state_transform` verifier discharges at compile time. There is no verifier
for "use-after-free" because there is no `free`; there is no verifier for
"double-free" because there is no `free`; there is no verifier for "aliasing"
because linear ownership of state precludes aliasing syntactically.

The win is structural, not algorithmic. The five pointer invariants of VUMA
1.x — liveness, exclusivity, cleanup, origin, interpretation — collapse to
**three** structural type checks in VUMA 2.0: `StateRead` (field-exists +
initialized), `StateWrite` (linear ownership, no write-after-consume), and
`StateTransform` (layout-type match + bounds). The other two — liveness and
cleanup — are discharged by construction because the source language has no
`free`. This is the 5→3 invariant collapse that gives PMT its power: every
memory-safety obligation is either eliminated by syntax or reduced to a
compile-time type check that runs at SCG construction time, before codegen.

---

## The VWK Kernel

> **Status (as of Wave 57):** The VWK kernel has been refined through 57 waves of work. Language-level cascade limitations are resolved (string literals, struct literals, State-return, array scaling, function pointers). Kernel subsystems have real implementations (VMM translate, CFS scheduler, COW fork refcount, ELF parser, syscall dispatch via __call_indirect1, signal delivery, real SHA-256 compression, AES SubBytes+ShiftRows, VFS ops dispatch, TCP 10-state machine, DNS/HTTP round-trip). Shell has 12 built-ins with tab completion, pipes, redirection, color. Bare-metal boot.S (GDT/IDT/paging/long-mode) is written. kernel_smoke.sh passes. All kernel modules compile with IVE: Pass.

VUMA now hosts a kernel written in its own PMT syntax: the **VWK** (Vuma Womb
Kernel), living under `womb/kernel/`. The kernel is **PMT-only**: there is no
pointer syntax, no `--pmt` flag, no escape hatch (kernel subtree only; stdlib crypto/net use legacy pointer syntax). Every kernel module is a
composition of typed-state transformations over arena-allocated `State<T>`
buffers. The compiler's three IVE state verifiers (`StateRead`,
`StateWrite`, `StateTransform`) discharge all memory-safety obligations at
compile time; the runtime arena (`runtime/arena.rs` + `__arena_overflow` trap
on all 19 backends) discharges the only remaining runtime obligation —
out-of-arena bounds. The kernel tree is **75 `.vuma` files**, all PMT-pure,
with 19-backend parity verified by `scripts/kernel_parity.sh`.

The kernel was built across **13 waves (K0–K12)**, each domain-scoped and
code-specific:

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

The 12 major subsystems are: **mm** (memory management), **proc** (processes
+ scheduler), **vfs** (virtual file system), **trap** (trap + IRQ dispatch),
**ipc** (pipes, signals, shm, futex), **sync** (spinlock, mutex, semaphore,
rwlock), **net** (sockets, TCP, DNS, HTTP), **crypto** (AES, SHA, asym),
**tty** (console, line discipline, VT100), **shell** (interactive shell),
**panic** (kernel panic + kmsg ring buffer), and **power** (power management).
Three further support layers — **arch** (per-architecture trampolines for
x86_64/aarch64/riscv64), **drivers** (UART, chardev, virtio-net), and **smp**
(multi-CPU + IPI) — round out the tree.

---

## Quick start

A PMT counter — increment, read, exit with the value:

```vuma
// counter.vuma — a PMT counter: increment, read, exit with the value.
// Expected exit code: 7

layout Counter = { value: u32 }

fn inc(c: State<Counter>, by: u32) {
    c.value = c.value + by;
}

fn main() -> i32 {
    let c = state_new(Counter);
    c.value = 0;
    inc(c, 3);
    inc(c, 4);
    return c.value;   // 7
}
```

### Build the compiler

```bash
# The toolchain is pinned via rust-toolchain.toml (nightly-2026-03-01);
# cargo auto-installs it on first use.
cargo build --profile release-fast --bin compile_dump
# The binary lands in target/release-fast/compile_dump (NOT target/release/)
```

### Compile + run a `.vuma` file on the host

```bash
./target/release-fast/compile_dump counter.vuma counter.bin x86_64
./counter.bin; echo "exit=$?"     # → exit=7
```

### Cross-compile to aarch64 and execute under QEMU

```bash
./target/release-fast/compile_dump counter.vuma counter.aarch64.bin aarch64
qemu-aarch64 counter.aarch64.bin; echo "exit=$?"     # → exit=7
```

### Run the verifier

```bash
# 3 PMT state checks (StateRead + StateWrite + StateTransform),
# no pointer invariants to prove:
./target/release-fast/compile_dump counter.vuma counter.bin x86_64 --verify
```

### Run the kernel smoke test

`scripts/kernel_smoke.sh` compiles `womb/kernel/kernel.vuma` for x86_64 with
`--verify`, runs the resulting ELF as a regular Linux process, greps stdout
for `vuma kernel: hello`, and checks exit code 0:

```bash
bash scripts/kernel_smoke.sh
# Expected: "PASS: kernel boots, prints banner, exits 0"
```

### Run the 19-backend parity sweep

`scripts/kernel_parity.sh` compiles + runs `kernel.vuma` and a subset of
gold-standard tests across **all 19 backends** using QEMU user-mode emulators
for non-x86_64 architectures:

```bash
bash scripts/kernel_parity.sh            # full sweep (~10 minutes)
bash scripts/kernel_parity.sh --quick    # arena_basic + kernel smoke only
```

---

## Key features

- **PMT type system** — `layout`, `State<T>`, `state_new`, `state.field`,
  `transform`. No `*T`, no `&x`, no `allocate`, no `free`. The four pointer
  syntactic forms are hard parse errors; there is no `--pmt` flag and no
  legacy path.
- **3 state verifiers** — `StateRead` (field-exists + initialized),
  `StateWrite` (linear ownership, no write-after-consume), `StateTransform`
  (layout-type match + bounds). Replaces the 5 pointer invariants
  (`liveness`, `exclusivity`, `cleanup`, `origin`, `interpretation`) — the
  5→3 invariant collapse.
- **Arena state model** — runtime-growable memory without pointers. Every
  `state_new(Layout)` bump-allocates against the program-wide
  `___pmt_buffer`; the runtime `arena_alloc` checks the capacity (stored at
  `[arena_ptr+16]`) and traps via `__arena_overflow` on overflow. There is no
  `free` and one deallocation site — program exit.
- **19 backends (7 executable + 12 compile-only)** — `x86_64`, `aarch64`, `aarch64_be`, `riscv64`,
  `riscv32`, `arm32`, `armeb`, `mips64`, `mips64be`, `ppc64`, `ppc64le`,
  `loongarch64`, `s390x`, `sparc64`, `alpha`, `hppa`, `m68k`, `x86_32`,
  `wasm32`. Each backend emits real machine code (or wasm), not a stub.
- **19-backend parity sweep** — `scripts/kernel_parity.sh` compiles and runs
  the kernel + a gold-standard subset across all 19 backends, with 7
  executable under QEMU user-mode and 12 compile-only.
- **Full O2 pipeline, always on** — SCG transforms, monomorphize, closures,
  `bv_verify`, e-graph equality saturation, instruction scheduler, LICM,
  cross-function constant propagation, escape analysis + SROA, vectorize,
  loop unroll. Schedulers and optimizers are enabled on every build (no
  `--release` opt-in needed).
- **Single-buffer runtime** — one `Alloc` for all states. Each
  `state_new(Layout)` lowers to `Offset` into `___pmt_buffer`; there are
  zero per-state `Alloc`s and zero `free`s at runtime.
- **E-graph layout optimization** — equality saturation over state ops
  includes `state_transform_elision` (a transform whose src layout equals
  its dst layout is rewritten to its input) and field-access fusion.
- **Dependent state types** — `State<Stack<N>>` where `N` is the runtime
  count of pushed elements; the `state_transform` verifier proves
  `offset + count * elem_size ≤ buffer_size` by linear arithmetic.
- **Nested layout support** — layouts can reference other layouts as field
  types; the field-chain resolver descends into nested layouts so `l.a.x`
  resolves to a cumulative byte offset (`0 [a in Line] + 0 [x in Point] = 0`).
- **If-expression support** — `let x = if cond { a } else { b };` produces a
  value-typed branch, complementing the statement-form `if`/`else`. Both
  forms lower to the same SCG branch node; the expression form threads the
  value through a phi-style merge.
- **FFI 4-mode marshal matrix** — `extern "C"` functions taking `State<T>`
  are classified per-argument into `Borrow` (pass base address, keep state
  alive), `Invalidate` (default — mark consumed after the call), `Marshal`
  (copy into a scratch buffer), or `ForeignPass` (hand off a `#[foreign]`
  layout's `.raw`). `#[borrow]`, `#[marshal]`, `#[may_retain]`, and
  `#[foreign]` attributes select the mode.
- **VWK kernel subsystems** — 84 PMT-pure `.vuma` files across 13 waves
  (K0–K12): mm, proc, vfs, trap, ipc, sync, net, crypto, tty, shell, panic,
  power. The kernel is a complete PMT program that compiles for every
  backend.
- **Comprehensive documentation** — 7 docs totaling 34K+ words: this README,
  `architecture.md`, `kernel-architecture.md`, `building.md`,
  `language-reference.md`, `kernel-developer-guide.md`,
  `kernel-porting-guide.md`, `contributing.md`.
- **Zero external dependencies** — the entire workspace (~329K LOC across 10
  crates) uses only Rust `std`. No `serde`, no `libc`, no `clap`, no
  `rayon`. Every external crate was replaced with a hand-written in-tree
  implementation. Builds require no network access to crates.io.

---

## The PMT model

### Layouts

A `layout` is a typed record describing the byte shape of a buffer. Fields
have primitive types (`u8`, `u32`, `i32`, `u64`, `i64`, `bool`), fixed-size
array types (`[u8; 4]`, `[u32; 8]`), or references to other layouts (nested
layouts).

```vuma
layout Point = { x: u32, y: u32 }
layout Line  = { a: Point, b: Point }   // nested layout field
layout Buf   = { data: [u8; 4] }        // fixed-size array field
```

The field-chain resolver descends into nested layouts, so `l.a.x` resolves
to a cumulative byte offset (`0 [a in Line] + 0 [x in Point] = 0`). Field
offsets and sizes are compile-time constants registered in the
`LayoutRegistry`; the verifiers catch any drift between consumers at compile
time.

### States

`State<T>` is a typed view of the program-wide memory buffer at the byte
offset assigned to a particular `state_new`. States are **linear**: each
state has exactly one owner, and ownership is transferred when the state is
passed to a transform. After a transform consumes a state, any subsequent
read or write to the original variable is a `state_write` linearity
violation.

```vuma
let p = state_new(Point);   // p: State<Point>, offset 0 in ___pmt_buffer
let q = state_new(Point);   // q: State<Point>, offset 16 (16-byte aligned)
p.x = 10;                   // StateWrite at p's offset + 0
q.x = 20;                   // StateWrite at q's offset + 0
return p.x + q.x;           // 30 — no aliasing, no cross-contamination
```

Reads (`state.field`) and writes (`state.field = val`) lower to typed
`Load`/`Store` at the field's compile-time-known offset. Array access
(`s.data[i]`) lowers to a `Load` of the `data` field (yielding a pointer to
the array's first element) followed by an `Index` load at offset
`i * elem_size`.

### Transforms

A `transform` is a pure function from one state layout to another.

```vuma
transform handle(req: State<Request>) -> State<Response> {
    // reads req.opcode, req.arg; writes result.status, result.result
    ...
}
```

A `StateTransform(x, L, U)` SCG node marks `x` as consumed — the
`state_write` verifier tracks the set of vregs consumed by transforms and
rejects any subsequent `StateWrite` to a consumed vreg.

### Arena states

Runtime-growable memory without pointers is provided by the arena runtime
(`src/codegen/src/runtime/arena.rs`). The arena is a bump allocator backed
by a single `mmap`'d region; `arena_alloc` bumps an offset and `arena_free`
unmaps the whole region. There is no per-object `malloc`/`free`. The PMT
surface mirrors this with the `womb/alloc/arena.vuma` library module:

```vuma
layout Arena = { offset: u32, capacity: u32, data: [u8; 256] }

// Caller allocates the state; init function populates it (init-style API):
let a = state_new(Arena);
arena_init(a, 256);
let off = arena_alloc(a, 64);    // returns a u32 logical offset, NOT a pointer
// Access bytes via typed array indexing — never *(addr + i):
a.data[off] = 0x41;
```

`arena_alloc` returns a **logical byte offset** (a `u32` handle into
`arena.data`), not a raw address. Callers access bytes via typed array
indexing (`arena.data[off]`), never via pointer arithmetic. The runtime
counterpart, `arena_grow`, reallocates the region (preserving contents) so
the arena can expand at runtime without ever exposing a `*T` to source code.
The `__arena_overflow` symbol is defined on all 19 backends as a trap
instruction (`ud2` on x86_64, `brk #0` on aarch64, `unimp` on riscv64, etc.)
— on hosted x86_64 it surfaces as a non-zero exit code; on bare metal it
halts the CPU.

### Init-style API pattern

Because the current codegen does not propagate `State`-typedness through
function return values, every kernel subsystem uses the **init-style API**:
the caller allocates the state with `state_new(Layout)` and passes it by
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
and is used by `pmm_init`, `vmm_init`, `trap_frame_init`, `task_init_for_switch`,
`syscall_args_from_frame`, `kmsg_init`, `pm_init`, and every other stateful
subsystem. A historical data point: K3e's `syscall_args_from_frame` was
originally written in return-style and the codegen emitted four
`WARNING: unsupported FieldAccess (not state-typed)` diagnostics; the fix
was to flip to init-style. The convention has been canonical ever since.

### FFI 4-mode marshal matrix

VUMA 2.0's FFI is built on a 4-mode matrix (replacing the legacy binary
`#[pure]`/invalidate model). Every foreign call is classified into an
argument mode per argument, a return mode, and optionally a callback mode.
The marshal module (`vuma-codegen/src/marshal.rs`) provides the
classification helpers:

| Mode | Behavior | Use case |
|------|----------|----------|
| **Borrow** | Pass `___pmt_buffer_base + offset`; keep state alive | `pte_read(pt, ...)`, `context_switch(prev, next)` — caller reads state after the call |
| **Invalidate** | Pass base address; mark state consumed (default) | Foreign calls that may free or move the buffer |
| **Marshal** | Copy into `___ffi_scratch_alloc` scratchpad; pass scratch ptr | NUL-terminated strings, C-owned memory round-trips |
| **ForeignPass** | Pass `.raw` of a `#[foreign]` layout; consume the state | `sqlite3_close(db)` — hand off ownership |

A fifth mode, `MayRetain`, is a Marshal variant where the callee may keep a
reference past the call (selected by `#[may_retain]`). The FFI safety
verifier (`vuma-ive/src/ffi.rs`) proves no invalidated state is accessed
after the call; the borrow-region verifier (`borrow_region.rs`) flags any
`StateWrite` to a borrowed region during the borrow window. The marshal
scratchpad (`runtime/ffi_scratch.rs`) is a thread-local stack-shaped buffer,
**never aliased by `___pmt_buffer`** — the state verifiers never see it.

### Collapsed-invariant table

VUMA 1.x verified 5 pointer invariants over an unbounded heap graph. VUMA
2.0 collapses them to 3 structural type checks on a single typed buffer.

| VUMA 1.x pointer invariant | VUMA 2.0 PMT structural check | How it is discharged |
|---|---|---|
| Liveness (no use-after-free) | — (no `free` in source) | eliminated by syntax |
| Exclusivity (no aliasing during mutation) | `StateWrite` linearity | linear ownership, no alias syntactically |
| Cleanup (no double-free) | — (no `free` in source) | eliminated by syntax |
| Origin (no uninitialized reads) | `StateRead` field-exists + initialized | type-check against layout registry |
| Interpretation (no type-punning) | `StateTransform` layout-type match | src/dst layout types must be equal where required |

The two `—` rows are the structural win: with no `free` in the source
language, liveness and cleanup are not verifier obligations at all. The
remaining three pointer concerns map one-to-one onto three structural
checks that the compiler runs at SCG construction time, before codegen
runs.

---

## Architecture

### Compiler pipeline

```
   .vuma source
        │
        ▼
   parser (lexer, recursive descent, typed AST)
        │
        ▼
   SCG — Semantic Computation Graph
   (nodes: StateInit, StateRead, StateWrite, StateTransform, BinOp, …)
        │
        ▼
   IVE — Invariant Verification Engine (VerificationLevel::Pmt)
   (state_read, state_write, state_transform verifiers)
        │
        ▼
   IR lowering (monomorphize, closures, bv_verify)
        │
        ▼
   O2 optimizer pipeline
   (e-graph equality saturation → scheduler → LICM →
    cross-function const prop → escape/SROA → vectorize → loop_unroll)
        │
        ▼
   19 backends (isel + register allocation + ELF emission)
        │
        ▼
   .bin (ELF for the target arch, or wasm32 module)
```

The pipeline is monolithic in source order but each stage is independently
testable. The scheduler models memory dependencies via cast-aware
type-based alias analysis (TBAA) with IVE-proven non-aliasing overrides.
The e-graph feeds both binop algebraic rules (35 rules) and state-op
rewrites (`state_transform_elision`).

### VWK kernel — the 4-layer cake

The VWK kernel is a four-layer system. Each layer is a complete, verifiable
compilation unit; layers compose by **byte-identical re-declaration** (VUMA
has no `import` yet — Open Work §7 — so each consumer of a layout or extern
re-declares it from the canonical source). Only L4 is PMT-pure; L1–L3 are
the substrate the PMT verifiers target.

```
┌────────────────────────────────────────────────────────────────────────┐
│  L4 — PMT Kernel Logic (womb/kernel/*.vuma — 75 files)                 │
│    console.vuma  kernel.vuma                                          │
│    mm/{pmm,vmm,kmalloc,mmap}   trap/{trap,irq}   proc/{task,...}      │
│    vfs/{inode,dentry,file,namei,mount,file_ops}  fs/{tmpfs,initramfs} │
│    ipc/{pipe,waitq,shm,futex,signal}  sync/{spinlock,mutex,...}       │
│    smp/{smp,percpu,ipi}  net/{socket,sk_buff,tcp,dns,http}            │
│    drivers/{uart,char,virtio_net}  tty/{console,line_discipline,vt100}│
│    syscall/{abi,table,dispatch,handlers/{io,mm,proc}}                 │
│    crypto/{api,aes,sha,asym,hw_trampoline}  panic/{panic,kmsg}        │
│    power/pm.vuma  shell/shell.vuma  hosted/host.vuma                  │
│                                                                        │
│  State: pure PMT — State<T>, state_new, layout field access.           │
│  Verification: IVE StateRead + StateWrite + StateTransform (compile).  │
└────────────────────────────────────────────────────────────────────────┘
                                 ▲  (extern "C" + State<T> as Address casts)
┌────────────────────────────────────────────────────────────────────────┐
│  L3 — Arena Runtime (runtime/arena.rs + 19 backend __arena_overflow)   │
│    bump allocator over ___pmt_buffer (capacity from BootInfo.mem_size) │
│    arena_alloc, arena_new, arena_grow, arena_overflow trap             │
│    State<T> lowered to (base_addr, LayoutId) — all field access        │
│      lowered to Load/Store at compile-time-known offset+size           │
└────────────────────────────────────────────────────────────────────────┘
                                 ▲  (called by FFI trampolines)
┌────────────────────────────────────────────────────────────────────────┐
│  L2 — FFI Trampolines (womb/kernel/arch/<arch>/*.vuma)                 │
│        x86_64: trampoline + mm_trampoline + trap_trampoline +          │
│                switch + pt + bootinfo (6 files)                        │
│        aarch64 / riscv64: mm_trampoline + trap_trampoline +            │
│                switch + pt (4 files each)                              │
│    extern "C" { fn write(...); fn mmap(...); fn context_switch(...); } │
│    Hosted: pre-registered Linux-syscall stubs in x86_64 backend.       │
│    Bare-metal (K11+): real asm stubs registered in backend.            │
│    Unregistered externs → __ffi_fallback_stub (xor eax,eax; ret).      │
└────────────────────────────────────────────────────────────────────────┘
                                 ▲  (asm entry stubs)
┌────────────────────────────────────────────────────────────────────────┐
│  L1 — boot.S (hosted: _start in backend; bare-metal: multiboot entry)  │
│    Set up argc/argv in BSS slot; call main().                          │
│    Bare-metal: parse multiboot2/stivale2, set up GDT/IDT/paging,       │
│    jump to kmain().                                                    │
└────────────────────────────────────────────────────────────────────────┘
```

| Layer | Proved by           | Memory-safety mechanism                    |
|-------|---------------------|--------------------------------------------|
| L1    | asm review          | Manual; constrained entry invariants       |
| L2    | asm review + ABI    | SysV/AArch64/RV ABI compliance             |
| L3    | runtime arena check | `__arena_overflow` trap on alloc > cap     |
| L4    | IVE (compile time)  | `StateRead`/`StateWrite`/`StateTransform`  |

The four-layer split means **no kernel module can corrupt another kernel
module's state by construction**. There is no pointer arithmetic to misuse,
no `free`-then-use, no `double-free`, no buffer overrun past a layout's
bounds. The only failure modes left are logic bugs (wrong field value, wrong
syscall return code, wrong scheduling decision) and resource exhaustion
(arena overflow → trap, slot pool exhausted → return error).

---

## Building

VUMA requires Rust **`nightly-2026-03-01`**, pinned via
[`rust-toolchain.toml`](rust-toolchain.toml) (the toolchain file is checked
in, so `cargo` auto-installs it on first use). The nightly channel is
required for `naked_asm`, the bare-metal target, and the const-generic /
trait-system features used by the codegen crate. The workspace has zero
external dependencies — only Rust `std`.

```bash
# Fast iterative profile (LTO off, codegen-units=16) — the default for tests
cargo build --profile release-fast --bin compile_dump
# → target/release-fast/compile_dump   (NOT target/release/)

# Release profile (fat LTO, codegen-units=1) — for benchmarking
cargo build --profile release --bin compile_dump

# The compile_dump binary is the test-harness entry point:
#   compile_dump <input.vuma> <output.bin> <backend> [--verify] [--pmt-only]
```

### Constrained-memory workaround

Some build environments (CI sandboxes, small VMs, Raspberry Pi 3/4) cap
available RAM at **4 GiB** (cgroup `memory.max=4294967296`). Under that cap
the `release-fast` profile — `opt-level = 3` with 16 parallel codegen units
— OOMs during the link/codegen step, even with `--workers 1`. Host-side
`opt-level` does **not** affect VUMA correctness (VUMA's own optimization
passes are the algorithms that matter); only build time and runtime speed
change. The workaround is to drop to the `dev` profile with single-threaded
codegen and symbol stripping:

```bash
# Constrained-memory build (≤ 4 GiB RAM)
CARGO_BUILD_JOBS=1 \
CARGO_PROFILE_DEV_CODEGEN_UNITS=1 \
CARGO_PROFILE_DEV_OPT_LEVEL=0 \
CARGO_PROFILE_DEV_DEBUG=0 \
CARGO_PROFILE_DEV_INCREMENTAL=true \
CARGO_INCREMENTAL=1 \
RUSTFLAGS="-C debug-assertions=off -C overflow-checks=off -C strip=symbols" \
  cargo build --profile dev --bin compile_dump --bin dump_ir
```

With this config a from-scratch build completes inside the 4 GiB cap and
incremental rebuilds take ~8 seconds. The emitted `compile_dump` is
functionally identical to the `release-fast` build — it produces the same
machine code for every VUMA program. To point the test runner at the
constrained build, pass `--profile dev` and build `compile_dump` separately
first (`--skip-build` skips the runner's own build step):

```bash
cargo build --profile dev --bin compile_dump
scripts/pi5_test_suite.sh --skip-build --profile dev --workers 1 --verify
```

### QEMU installation

QEMU user-mode is required for cross-backend testing. Install the QEMU user
binaries for the architectures you intend to target:

```bash
# Debian/Ubuntu (or build from source if not root):
apt-get install qemu-user qemu-user-static
# Verify:
qemu-aarch64 --version
qemu-riscv64 --version
```

If you cannot install via the system package manager (no root, sandboxed
CI), use the **static binary approach**: download the
`qemu-{arch}-static` binaries from the
[multiarch/qemu-user-static](https://github.com/multiarch/qemu-user-static/releases)
releases into a local directory on `PATH` (the convention used by this
repo and `TASKS.md` §0.2 is `$HOME/.local/bin/`). The runner also
registers `binfmt_misc` entries for every cross-architecture (skipping the
host's native arch to avoid QEMU recursion) so fork+exec of a cross-compiled
ELF goes through the right interpreter.

The `wasm32` backend does not use QEMU. Its emitted `.wasm` modules are
executed by [`wasmtime`](https://github.com/bytecodealliance/wasmtime),
driven by [`scripts/wasm32_runner.py`](scripts/wasm32_runner.py) which
provides the host functions (pipe/fork/execve/dup2/waitpid/strcmp) that WASI
does not support. Install both the CLI binary and the Python package. If
`wasmtime` is missing the runner skips the `wasm32` backend rather than
failing the suite.

### Kernel scripts

```bash
# Boot smoke test — compiles womb/kernel/kernel.vuma for x86_64 with --verify,
# runs it, greps for "vuma kernel: hello", checks exit 0.
bash scripts/kernel_smoke.sh

# 19-backend parity sweep — compiles + runs kernel.vuma and a gold-standard
# subset across ALL 19 backends using QEMU user-mode for non-x86_64.
bash scripts/kernel_parity.sh            # full sweep (~10 minutes)
bash scripts/kernel_parity.sh --quick    # arena_basic + kernel smoke only
```

---

## Testing

### Full gold-standard suite

```bash
# All 19 backends, all categories, IVE --verify
bash scripts/pi5_test_suite.sh --workers 8 --fresh --verify
```

Flags:

- `--workers N` — parallel compile/run workers (default 4; 8 is typical on a
  Pi 5 or 4-core x86 host).
- `--fresh` — rebuild the compiler before running (skips caching).
- `--verify` — pass `--verify` to `compile_dump` so IVE runs on every test.
- `--backends aarch64,x86_64` — restrict to a subset of backends.
- `--skip-build` — assume `target/.../compile_dump` is already built.
- `--release` — build with the `release` profile instead of `release-fast`.
- `--profile dev` — point at a constrained-memory build (see Building above).

### Kernel smoke test

`scripts/kernel_smoke.sh` is the minimum bar every commit must clear. It
compiles `womb/kernel/kernel.vuma` for `x86_64` with `--verify`, runs the
resulting ELF as a regular Linux process, greps stdout for
`vuma kernel: hello`, and checks exit code 0.

```bash
bash scripts/kernel_smoke.sh
# Expected: "PASS: kernel boots, prints banner, exits 0"
```

### 19-backend parity sweep

`scripts/kernel_parity.sh` compiles + runs `kernel.vuma` and a subset of
gold-standard tests (arena_basic, arena_grow, arena_overflow, init_read,
arith_clamp, cf2_for_count, fn2_add_two, bit2_and_chain, enum_demo) across
**all 19 backends** using QEMU user-mode emulators for non-x86_64. It also
compile-verifies (IVE only, no execution) 19 kernel modules covering mm,
proc, vfs, ipc, sync, net, crypto, panic, and power. Exits 0 only if every
backend passes.

### Gold-standard categories

Test categories in `tests/gold_standard/` (manifest-driven, 1,502 programs (post-cleanup)):

| Category | Count | What it exercises |
|---|---|---|
| `arithmetic` | 72 | `+ - * / %` on `i32`/`i64`/`u64` |
| `atomics` | 35 | atomic load/store/CAS patterns |
| `bitwise` | 50 | `& \| ^ << >>` and rotations |
| `complex_stores` | 45 | multi-field, multi-buffer writes |
| `concurrency` | 35 | concurrent overwrite/publish patterns |
| `control_flow` | 50 | `if`/`else`, `while`, `for`, `break`, `continue` |
| `crypto_patterns` | 36 | AES/SHA/HMAC/ChaCha20 round constants |
| `edge_cases` | 42 | overflow, div-by-zero, sign edge cases |
| `functions` | 48 | 0-to-4 arg calls, recursion, nested calls |
| `linked_structures` | 32 | PMT-linked nodes (data, not pointers) |
| `memory` | 74 | the PMT migration of pointer store/load tests |
| `multi_function` | 35 | cross-function state passing |
| `nested_loops` | 19 | 2-/3-deep loop nests |
| `pointers` | 47 | PMT-migrated pointer programs |
| `structs` | 46 | layout field access, nested layouts |
| `u32_arith` | 38 | 32-bit unsigned arithmetic |
| `pmt_wave1`–`pmt_wave10` | — | PMT-specific: transforms, single-buffer, negative tests (unknown field, write-after-consume), FFI marshal |
| `arena_wave0`–`arena_wave2` | — | arena_alloc bounds check, arena_grow, arena_overflow regression |
| `ffi_wave0`–`ffi_wave4` | — | FFI marshal: borrow modes, marshal scratch, foreign state, callbacks |

Each test carries an `// Expected exit code: N` header. The suite compiles
each test for each backend, runs under QEMU, and compares the actual exit
code to the expected. A `skip_on: wasm32, ppc64` header marks tests that
exercise architecturally-unavailable functionality (e.g. `fork` on wasm32).

### KAT tests for crypto

Known-answer tests for crypto algorithms (SHA-256, AES-128/192/256, Ed25519,
ECDSA P-256/P-384, ML-DSA, ML-KEM, SLH-DSA, Falcon, HQC, X25519, ChaCha20,
Poly1305, Argon2, scrypt, HKDF, HMAC, RSA-OAEP-PSS, and more) live in
`scripts/womb_kat_tests/` and `scripts/real_kat_tests/`. Each test is a
`.vuma` program that computes a hash/ciphertext/signature and checks it
against a known value.

```bash
# Run all gold-standard tests:
bash scripts/run_all_gold.sh
# Run the real KAT suite (cross-architecture known-answer tests):
bash scripts/run_real_kat.sh
```

### Single-test smoke check

```bash
./target/release-fast/compile_dump \
    tests/gold_standard/pmt_wave2/init_read.vuma /tmp/init.bin x86_64 --verify
/tmp/init.bin; echo "exit=$?"   # → exit=42
```

### Per-module kernel self-tests

Every `.vuma` file in `womb/kernel/` ends with a `fn main() -> i32`
self-test that exercises the module's API surface. A non-zero exit code
pinpoints the broken check by number.

```bash
./target/release-fast/compile_dump womb/kernel/mm/pmm.vuma \
    /tmp/pmm.bin x86_64 --verify
/tmp/pmm.bin; echo "exit=$?"
# Expected: "IVE: Pass passed=1 failed=0 total=1" + exit=0
```

---

## Backends

All 19 backends emit real machine code (or wasm) — no interpreter stubs.

| Backend | Architecture | Executable? | Notes |
|---|---|---|---|
| `x86_64` | AMD64 (Intel/AMD) | native | Default host backend on x86_64 Linux |
| `aarch64` | ARMv8 64-bit, little-endian | QEMU | Servers, Apple Silicon, Raspberry Pi 4/5 |
| `aarch64_be` | ARMv8 64-bit, big-endian | compile-only | Networking appliances, some embedded |
| `riscv64` | RISC-V 64-bit, little-endian | QEMU | VisionFive, SiFive boards |
| `riscv32` | RISC-V 32-bit, little-endian | compile-only | Embedded RV32 cores |
| `arm32` | ARMv7 32-bit, little-endian | QEMU | Legacy mobile, Pi 1/2 |
| `armeb` | ARMv7 32-bit, big-endian | compile-only | Specialty embedded |
| `mips64` | MIPS 64-bit, little-endian | compile-only | Loongson-class little-endian MIPS |
| `mips64be` | MIPS 64-bit, big-endian | compile-only | SGI/Loongson big-endian MIPS |
| `ppc64` | PowerPC 64-bit, big-endian | compile-only | AIX, IBM POWER (BE mode) |
| `ppc64le` | PowerPC 64-bit, little-endian | QEMU | ppc64le Linux (IBM POWER8/9 LE) |
| `loongarch64` | LoongArch 64-bit | QEMU | Loongson 3A/3B |
| `s390x` | IBM Z mainframe, big-endian | QEMU | z/Architecture |
| `sparc64` | SPARC V9 64-bit, big-endian | compile-only | UltraSPARC, Fujitsu SPARC64 |
| `alpha` | DEC Alpha 64-bit, little-endian | compile-only | Legacy 64-bit RISC |
| `hppa` | HP PA-RISC 32-bit, big-endian | compile-only | Legacy HP workstations |
| `m68k` | Motorola 680x0 32-bit, big-endian | compile-only | Amiga, Atari ST, classic Mac |
| `x86_32` | i386 32-bit | compile-only | Legacy PC compatibles |
| `wasm32` | WebAssembly 32-bit | wasmtime | Browser/standalone wasm runtime |

**7 backends are executable** via QEMU user-mode (or natively on x86_64, or
under `wasmtime` for wasm32): `x86_64`, `aarch64`, `riscv64`, `arm32`,
`ppc64le`, `loongarch64`, `s390x`. The remaining **12 are compile-only** —
they emit valid ELF machine code and pass IVE verification, but a QEMU
user-mode binary for that architecture is not in the standard sweep (some,
like `wasm32`, run under `wasmtime`; others like `m68k` and `hppa` would
need their QEMU binary installed manually). The `kernel_parity.sh` sweep
compile-verifies all 19; it executes the 7 in the executable set.

Syscall numbers use VUMA-generic (Linux `asm-generic/unistd.h`) numbering;
the compiler translates to native per-arch automatically. Identity arches
(native == generic): aarch64, riscv64, riscv32, loongarch64, arm32.
Translated arches: x86_64, x86_32, mips64, ppc64, s390x, sparc64, alpha,
hppa, m68k.

---

## Project structure

VUMA is a Cargo workspace of **10 internal crates** (~329K LOC of Rust)
plus a PMT standard library, a PMT kernel, a gold-standard test suite, and
supporting scripts and docs.

```
vuma/
├── src/                      # 10 workspace crates (the compiler)
│   ├── parser/               # vuma-parser  — lexer, parser, AST, AST→SCG
│   ├── scg/                  # vuma-scg     — Semantic Computation Graph IR
│   ├── bd/                   # vuma-bd      — Behavioral Descriptors
│   ├── ive/                  # vuma-ive     — Inference & Verification Engine
│   ├── codegen/              # vuma-codegen — 19 backends + scheduler + optimizer
│   ├── proof/                # vuma-proof   — formal proof system
│   ├── cor/                  # vuma-cor     — Continuous Optimization Runtime
│   ├── vuma/                 # vuma-core    — memory model, MSG, security
│   ├── package/              # vuma-package — manifest parser, resolver
│   ├── tests/                # vuma-tests   — integration test framework
│   ├── bin/                  # compile_dump, dump_ir, parse_test, scg_dump
│   ├── main.rs               # CLI driver
│   └── lib.rs
├── womb/                     # PMT standard library (183 .vuma files)
│   ├── core.vuma             # core intrinsics
│   ├── alloc/arena.vuma      # arena state model
│   ├── crypto/               # 50+ crypto modules (AES, SHA, Ed25519, ML-KEM, …)
│   ├── lib/                  # stdlib: stdio, string, json, http, deflate, …
│   ├── net/                  # tls12, tls13, ssh, quic, tcp
│   ├── collections/          # vec, hashmap, btree_map, enum_map
│   ├── string/  io/  fs/  codec/  encoding/  env/  ieee/  graph/  lang/
│   └── kernel/               # VWK kernel (75 .vuma files, 13 waves K0–K12)
├── tests/
│   ├── gold_standard/        # 1,502 manifest-driven (post-cleanup) PMT test programs
│   └── *.rs                  # Rust integration tests (scheduler, egraph, …)
├── examples/                 # 48 example .vuma programs
├── docs/                     # 7 documentation files (34K+ words)
├── scripts/                  # 24 build/test scripts (.sh + .py)
├── Cargo.toml                # workspace root
├── rust-toolchain.toml       # pinned nightly-2026-03-01
├── build.rs  Makefile  justfile  clippy.toml  rustfmt.toml
├── vuma_vm.h                 # C header for the vuma_context_t host API
└── LICENSE                   # MIT
```

### `src/` — the compiler (10 workspace crates)

| Crate (`src/...`)  | Package          | One-line description                                            |
|--------------------|------------------|-----------------------------------------------------------------|
| `src/parser/`      | `vuma-parser`    | Frontend: lexer, parser, AST, AST→SCG lowering, error recovery  |
| `src/scg/`         | `vuma-scg`       | Semantic Computation Graph — the formal graph IR                |
| `src/bd/`          | `vuma-bd`        | Behavioral Descriptors — RepD, CapD, RelD lattices + inference  |
| `src/ive/`         | `vuma-ive`       | Inference & Verification Engine — the 3 state verifiers + FFI   |
| `src/codegen/`     | `vuma-codegen`   | 19-architecture backend, register allocator, scheduler, optimizer |
| `src/proof/`       | `vuma-proof`     | Formal proof system — checker, tactics, counterexamples         |
| `src/cor/`         | `vuma-cor`       | Continuous Optimization Runtime — JIT, profiling, speculation   |
| `src/vuma/`        | `vuma-core`      | Memory model, MSG construction, invariant checking, security    |
| `src/package/`     | `vuma-package`   | Package manager — manifest parser, dependency resolver, registry |
| `src/tests/`       | `vuma-tests`     | Integration test framework                                      |

The root `vuma` crate provides the CLI binary (`src/main.rs`) plus the
`compile_dump` and `dump_ir` drivers in `src/bin/`.

### `womb/` — the PMT standard library (183 `.vuma` files)

The `womb/` tree is the VUMA standard library written in PMT syntax. It
includes 50+ crypto modules (AES-128/192/256 and modes, SHA-1/256/384/512,
SHA-3, BLAKE2/3, Ed25519, ECDSA P-256/P-384, X25519, ECDH, ML-DSA, ML-KEM,
SLH-DSA, Falcon, HQC, RSA, ChaCha20-Poly1305, Salsa20, Argon2, scrypt,
HKDF, PBKDF2, HMAC, DRBG), a broad application library (stdio, string,
JSON, HTTP/1/2, HPACK, WebSocket, DNS, TLS 1.2/1.3, SSH, QUIC, JWT, X.509,
PKI, ASN.1, email, deflate, bignum, bignum2048, unicode, math, time,
threading, event_loop, socket, fileio, auth), collections (vec, hashmap,
btree_map, enum_map), containers, IEEE floating-point, graph algorithms,
encoding (base64, hex, url), and a self-hosted language toolchain
(full_lexer, full_parser, ir_builder, codegen, elf).

### `womb/kernel/` — the VWK kernel (75 `.vuma` files)

The kernel subtree is described in [The VWK Kernel](#the-vwk-kernel)
above. It is PMT-pure, IVE-verified, and compiles for all 19 backends.

### `tests/gold_standard/` — 1,502 (post-cleanup) test programs

Manifest-driven PMT test programs grouped by feature category (arithmetic,
atomics, bitwise, control_flow, functions, structs, …) and PMT wave
(pmt_wave1–10, arena_wave0–2, ffi_wave0–4). Each test carries an
`// Expected exit code: N` header.

### `examples/` — 48 example programs

Standalone `.vuma` programs demonstrating the language: `fibonacci`,
`quicksort`, `crc32`, `sha256d`, `matrix`, `linked_list`,
`doubly_linked_list`, `lock_free_queue`, `thread_pool`, `spinlock`,
`channel_demo`, `memory_arena`, `arena_allocator`, `ffi_demo`,
`base64_encode`, `hex_dump`, `gpio_blink`, `epoll_echo`, `mmap_sha256d`,
`signal_hash`, `self_exec`, `float_math`, `debug_info`, `pipeline`, and
more.

### `docs/` — 7 documentation files

See [Documentation](#documentation) below.

### `scripts/` — 20 build/test scripts

Bash and Python scripts covering the full test + build surface
(verify with `ls scripts/`):
`pi5_test_suite.sh` (gold-standard sweep), `kernel_smoke.sh` (boot smoke
test), `kernel_parity.sh` (19-backend parity sweep), `run_all_gold.sh`,
`run_real_kat.sh`, `cross_backend_test.sh`, `run_backend_resilient.py`,
`supervisor.py`, `wasm32_runner.py`, `gen_real_kat.py`,
`ci_run_tests.sh`, `run_fuzz.sh`, `test_womb_compile.sh`,
`womb_test_harness.sh`, `run_differential.sh`, `generate_report.sh`,
`add_expected_codes.py`, `run_one_batch.py`, `qemu_boot.sh`,
`qemu_system_boot.sh`. Plus two KAT test-data directories:
`scripts/womb_kat_tests/` and `scripts/real_kat_tests/`.

---

## Documentation

This README plus seven documents under `docs/` (totaling 34,000+ words across
the `docs/` directory alone) cover the compiler, the kernel, the build, the
language, and the contribution workflow:

| Document | Words | Description |
|---|---|---|
| [`README.md`](README.md) | this file | Project overview, quick start, feature tour |
| [`docs/architecture.md`](docs/architecture.md) | 3,678 | VUMA 2.0 compiler architecture: PMT pipeline, state type system, the 3 state verifiers, Behavioral Descriptors, e-graph layout optimization, 19 backends, dependent state types, the FFI 4-mode marshal matrix |
| [`docs/kernel-architecture.md`](docs/kernel-architecture.md) | 9,935 | VWK kernel architecture: the 4-layer cake, boot flow, PMT-in-the-kernel design, arena memory model, per-arch abstraction, FFI trampoline patterns, IVE guarantees, complete 75-file inventory, data flow diagrams, memory layout |
| [`docs/building.md`](docs/building.md) | 2,019 | Complete build reference: prerequisites, Rust toolchain, QEMU installation, build profiles, constrained-memory workaround, troubleshooting |
| [`docs/language-reference.md`](docs/language-reference.md) | 2,382 | VUMA 2.0 language reference: layouts, `State<T>`, `state_new`, `state.field`, transforms, `extern "C"`, `#[borrow]`, `as Address` |
| [`docs/kernel-developer-guide.md`](docs/kernel-developer-guide.md) | 7,581 | How to add syscalls, drivers, filesystems, and PMT kernel code; do/don't examples; IVE failure debugging recipe |
| [`docs/kernel-porting-guide.md`](docs/kernel-porting-guide.md) | 6,535 | Step-by-step guide to porting the kernel to a new architecture (worked example: x86_64) |
| [`docs/contributing.md`](docs/contributing.md) | 1,796 | General contribution workflow |

Read [`architecture.md`](docs/architecture.md) first if any term in this
README is unfamiliar; read [`kernel-architecture.md`](docs/kernel-architecture.md)
next if you intend to work on the kernel. For day-to-day contributor
workflow see [`contributing.md`](docs/contributing.md); for the PMT language
itself see [`language-reference.md`](docs/language-reference.md).

---

## License

MIT — see [`LICENSE`](LICENSE).
