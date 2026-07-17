# VUMA — Programs as Memory Transformations

A systems programming language where memory is mutated only by typed state
transforms. Pointers, allocation, and deallocation are removed from the source
language: `allocate`, `free`, `*ptr`, and `&x` are hard compile errors.
Memory safety becomes a structural type-checking property, not a constraint
solver over the heap.

VUMA compiles PMT source through a full O2 pipeline (SCG, monomorphization,
closures, e-graph layout optimization, scheduler, LICM, escape/SROA,
vectorization, loop unrolling) to **19 bare-metal backends** — x86_64 through
wasm32, including big-endian variants — under a single-buffer runtime with
zero per-state `malloc`/`free`.

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
unbounded heap graph: liveness (no use-after-free), exclusivity (no aliasing
during mutation), cleanup (no double-free), origin (no uninitialized reads),
and interpretation (no type-punning). Five invariants over a heap graph is a
constraint problem — solvable, but slow, and any gap is a CVE. VUMA replaces
that graph with a fixed set of structural type checks on a single typed
buffer.

The result is that memory safety is a type-checking property. A `state.field`
read is type-checked against the layout's field set — a read of a non-existent
field is a compile error, not a runtime crash. A write after a state has been
consumed by a transform is a linearity violation caught by the `state_write`
verifier. Bounds on array access reduce to `offset + count * elem_size ≤
buffer_size`, a linear-arithmetic obligation the `state_transform` verifier
discharges at compile time. There is no verifier for "use-after-free" because
there is no `free`; there is no verifier for "double-free" because there is no
`free`; there is no verifier for "aliasing" because linear ownership of state
precludes aliasing syntactically.

## Quick start

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

Build and run on the host:

```bash
cargo build --profile release-fast --bin compile_dump
./target/release-fast/compile_dump counter.vuma counter.bin x86_64
./counter.bin; echo "exit=$?"
```

Cross-compile to aarch64 and execute under QEMU:

```bash
./target/release-fast/compile_dump counter.vuma counter.aarch64.bin aarch64
qemu-aarch64 counter.aarch64.bin; echo "exit=$?"
```

Run the verifier (3 PMT state checks, no pointer invariants):

```bash
./target/release-fast/compile_dump counter.vuma counter.bin x86_64 --verify
```

## Key features

- **PMT type system** — `layout`, `State<T>`, `state_new`, `state.field`,
  `transform`. No `*T`, no `&x`, no `allocate`, no `free`. The four pointer
  syntactic forms are hard errors under `--pmt-only`.
- **3 state verifiers** — `StateRead` (field-exists + initialized),
  `StateWrite` (linear ownership, no write-after-consume), `StateTransform`
  (layout-type match + bounds). Replaces the 5 pointer invariants
  (`liveness`, `exclusivity`, `cleanup`, `origin`, `interpretation`).
- **19 bare-metal backends** — `x86_64`, `aarch64`, `aarch64_be`, `riscv64`,
  `riscv32`, `arm32`, `armeb`, `mips64`, `mips64be`, `ppc64`, `ppc64le`,
  `loongarch64`, `s390x`, `sparc64`, `alpha`, `hppa`, `m68k`, `x86_32`,
  `wasm32`. Each backend emits real machine code (or wasm), not a stub.
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
- **FFI marshal pass** — `extern "C"` functions taking `State<T>` are
  flattened to raw pointers at the ABI boundary; `#[pure]` declares an
  extern fn that does not invalidate its state arguments.
- **Zero external dependencies** — the entire workspace uses only Rust
  `std`. No `serde`, no `libc`, no `clap`, no `rayon`. Every external crate
  was replaced with a hand-written in-tree implementation.

## The PMT model

### Layouts

A `layout` is a typed record describing the byte shape of a buffer. Fields
have primitive types (`u8`, `u32`, `i32`, `u64`, `i64`, `bool`), fixed-size
array types (`[u8; 4]`, `[u32; 8]`), or references to other layouts
(nested layouts).

```vuma
layout Point = { x: u32, y: u32 }
layout Line  = { a: Point, b: Point }   // nested layout field
layout Buf   = { data: [u8; 4] }        // fixed-size array field
```

The field-chain resolver descends into nested layouts, so `l.a.x` resolves
to a cumulative byte offset (`0 [a in Line] + 0 [x in Point] = 0`).

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

## Architecture

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

## Building

VUMA builds with Rust stable; the workspace has zero external dependencies.

```bash
# Fast iterative profile (LTO off, codegen-units=16) — the default for tests
cargo build --profile release-fast --bin compile_dump

# Release profile (LTO, codegen-units=1) — for benchmarking
cargo build --profile release --bin compile_dump

# The compile_dump binary is the test-harness entry point:
#   compile_dump <input.vuma> <output.bin> <backend> [--verify] [--pmt-only]
```

QEMU is required for cross-backend testing. Install the QEMU user binaries
for the architectures you intend to target:

```bash
# Debian/Ubuntu (or build from source if not root):
apt-get install qemu-user qemu-user-static
# Verify:
qemu-aarch64 --version
qemu-riscv64 --version
```

The `wasm32` backend uses `wasmtime` (Python package or CLI) for execution;
`scripts/wasm32_runner.py` provides the host functions (pipe/fork/execve/
dup2/waitpid/strcmp) that WASI does not support.

## Testing

```bash
# Full gold-standard suite: all 19 backends, all categories, IVE --verify
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

Test categories in `tests/gold_standard/` (manifest-driven, 704 programs):

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
| `pmt_wave1`–`pmt_wave10` | — | PMT-specific: transforms, single-buffer,
  negative tests (unknown field, write-after-consume), FFI marshal |

Each test carries an `// Expected exit code: N` header. The suite compiles
each test for each backend, runs under QEMU, and compares the actual exit
code to the expected. A `skip_on: wasm32, ppc64` header marks tests that
exercise architecturally-unavailable functionality (e.g. `fork` on wasm32).

```bash
# Single-test smoke check
./target/release-fast/compile_dump \
    tests/gold_standard/pmt_wave2/init_read.vuma /tmp/init.bin x86_64 --verify
/tmp/init.bin; echo "exit=$?"   # → exit=42
```

## Backends

All 19 backends emit real machine code (or wasm) — no interpreter stubs.

| Backend | Architecture | Notes |
|---|---|---|
| `x86_64` | AMD64 (Intel/AMD) | Default host backend on x86_64 Linux |
| `aarch64` | ARMv8 64-bit, little-endian | Servers, Apple Silicon, Raspberry Pi 4/5 |
| `aarch64_be` | ARMv8 64-bit, big-endian | Networking appliances, some embedded |
| `riscv64` | RISC-V 64-bit, little-endian | VisionFive, SiFive boards |
| `riscv32` | RISC-V 32-bit, little-endian | Embedded RV32 cores |
| `arm32` | ARMv7 32-bit, little-endian | Legacy mobile, Pi 1/2 |
| `armeb` | ARMv7 32-bit, big-endian | Specialty embedded |
| `mips64` | MIPS 64-bit, little-endian | Loongson-class little-endian MIPS |
| `mips64be` | MIPS 64-bit, big-endian | SGI/Loongson big-endian MIPS |
| `ppc64` | PowerPC 64-bit, big-endian | AIX, IBM POWER (BE mode) |
| `ppc64le` | PowerPC 64-bit, little-endian | ppc64le Linux (IBM POWER8/9 LE) |
| `loongarch64` | LoongArch 64-bit | Loongson 3A/3B |
| `s390x` | IBM Z mainframe, big-endian | z/Architecture |
| `sparc64` | SPARC V9 64-bit, big-endian | UltraSPARC, Fujitsu SPARC64 |
| `alpha` | DEC Alpha 64-bit, little-endian | Legacy 64-bit RISC |
| `hppa` | HP PA-RISC 32-bit, big-endian | Legacy HP workstations |
| `m68k` | Motorola 680x0 32-bit, big-endian | Amiga, Atari ST, classic Mac |
| `x86_32` | i386 32-bit | Legacy PC compatibles |
| `wasm32` | WebAssembly 32-bit | Browser/standalone wasm runtime |

Syscall numbers use VUMA-generic (Linux `asm-generic/unistd.h`) numbering;
the compiler translates to native per-arch automatically. Identity arches
(native == generic): aarch64, riscv64, riscv32, loongarch64, arm32.
Translated arches: x86_64, x86_32, mips64, ppc64, s390x, sparc64, alpha,
hppa, m68k.

## License

MIT
