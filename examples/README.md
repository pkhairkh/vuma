# examples/ — VUMA Example Programs

The `examples/` directory contains **48 standalone `.vuma` programs**
demonstrating the VUMA language. Each example is a self-contained program
with a header comment describing what it does, what VUMA features it
exercises, and (where applicable) the expected exit code. Examples are
the fastest way to see VUMA in action — they cover algorithms, data
structures, memory management, arithmetic, concurrency, crypto, syscalls,
FFI, and language features.

This README is the entry point for the examples. For the language
reference see [`docs/language-reference.md`](../docs/language-reference.md).
For the build reference see [`docs/building.md`](../docs/building.md).

---

## What's here

48 `.vuma` files organized (loosely) by category. Each example is a single
file with no `import` dependencies — every example compiles standalone
with `compile_dump`.

### Algorithms (8 files)

| File | Description |
|------|-------------|
| `fibonacci.vuma` | Recursive and iterative Fibonacci; returns `fib(30) = 832040` |
| `quicksort.vuma` | In-place quicksort on an array of integers (Lomuto partition) |
| `bsearch.vuma` | Binary search on a sorted array of u32; expected exit 7 |
| `crc32.vuma` | CRC32 checksum (IEEE 802.3 / ITU-T V.42) — used by ZIP, PNG, Ethernet |
| `sha256d.vuma` | Double SHA-256 (SHA-256 of SHA-256 of message) — Bitcoin's hash function |
| `matrix.vuma` | 4×4 matrix multiplication (classic O(n³) algorithm) |
| `sorted_map.vuma` | AVL-balanced tree map with verified pointer safety |
| `mmap_sha256d.vuma` | SHA-256d hash computed in an mmap'd memory buffer; expected exit 229 |

### Data structures (5 files)

| File | Description |
|------|-------------|
| `linked_list.vuma` | Singly-linked list — head-only insertion, traversal |
| `doubly_linked_list.vuma` | Doubly-linked list — VUMA's showcase (requires `unsafe` in Rust, fully verified in VUMA) |
| `lock_free_queue.vuma` | SPSC ring buffer — push/pop with separate allocations; expected exit 0 |
| `channel_demo.vuma` | MPSC channel — sender cloning, message passing, select |
| `sorted_map.vuma` | (also listed under algorithms) AVL-balanced tree map |

### Memory management (5 files)

| File | Description |
|------|-------------|
| `arena_allocator.vuma` | Region-based arena allocator — all allocations freed at once |
| `memory_arena.vuma` | Typed arena allocator with nested scopes and reset |
| `hello_memory.vuma` | The simplest VUMA program — allocate, write, read, free |
| `test_alloc.vuma` | Test: allocate and return 0 |
| `test_store.vuma` | Test: allocate, store, return |

### Arithmetic (4 files)

| File | Description |
|------|-------------|
| `test_u32_arith.vuma` | u32 arithmetic exercising 32-bit overflow + function calls; expected exit 79 |
| `test_u32_mem.vuma` | u32 store/load through pointers (SHA256d-style); expected exit 79 |
| `test_w_sched.vuma` | Loop with memory operations (simplified SHA256d W schedule); expected exit 79 |
| `float_math.vuma` | FP conversion and math operations |

### Concurrency (5 files)

| File | Description |
|------|-------------|
| `atomics_demo.vuma` | Atomic CAS, load, store, and fetch operations |
| `spinlock.vuma` | Simple spinlock using VUMA atomic operations |
| `thread_pool.vuma` | Fixed-size thread pool with work stealing and verified synchronization |
| `channel_demo.vuma` | (also listed under data structures) MPSC channel |
| `lock_free_queue.vuma` | (also listed under data structures) SPSC ring buffer |

### Crypto (5 files)

| File | Description |
|------|-------------|
| `sha256d.vuma` | SHA-256d (double SHA-256) per NIST FIPS 180-4 |
| `mmap_sha256d.vuma` | (also listed under algorithms) SHA-256d in an mmap'd buffer |
| `test_sha_manual.vuma` | SHA-256 compression with loops and memory; expected exit 79 |
| `test_sha_round.vuma` | SHA-256 compression round simulation (simplified); expected exit 79 |
| `signal_hash.vuma` | Reentrant SHA256d in a "handler" function with AtomicStore |

### Syscalls (3 files)

| File | Description |
|------|-------------|
| `syscall_32bit.vuma` | FFI write/exit litmus test on 32-bit backends; expected exit 42 |
| `epoll_echo.vuma` | epoll_create1 + close syscall test; expected exit 0 |
| `self_exec.vuma` | Fork+exec pipeline — child execve()s itself with different args |

### FFI (1 file)

| File | Description |
|------|-------------|
| `ffi_demo.vuma` | Calling C library functions from VUMA; expected exit 0 |

### Language features (5 files)

| File | Description |
|------|-------------|
| `enum_demo.vuma` | Enum (tagged union) types in VUMA |
| `struct_demo.vuma` | Struct types in VUMA — compiled down to typed memory layouts |
| `test_call.vuma` | Function call — `add1(x) = x + 1` |
| `test_loop.vuma` | For loop — sum 0..10 |
| `test_endian.vuma` | Read/write u32 big-endian |

### I/O + runtime (6 files)

| File | Description |
|------|-------------|
| `minimal.vuma` | Minimal test — allocate and return 0 |
| `test_exit.vuma` | Simple test — return exit code 42 |
| `test_print.vuma` | Print integers and hex output to stdout |
| `test_print2.vuma` | Simpler print_int to verify runtime works |
| `test_hex.vuma` | Print hex output to stdout |
| `test_hex2.vuma` | Minimal hex test — allocate 1 byte, print_hex it |
| `test_rotr.vuma` | Nested function calls — `rotr(x, n)` |

### Hardware + debug (2 files)

| File | Description |
|------|-------------|
| `gpio_blink.vuma` | AArch64 hardware access — device memory mapping, VUMA with hardware |
| `debug_info.vuma` | Simple program that compiles with `--debug` flag (DWARF) |

---

## Notable examples

A few examples are particularly illustrative of VUMA's design:

### `fibonacci.vuma` — recursive + iterative fib

Computes Fibonacci numbers using both recursive and iterative approaches,
then verifies they agree. Returns `fib(30) = 832040`. Demonstrates
function calls, recursion, iteration, and integer arithmetic.

### `quicksort.vuma` — in-place quicksort

Classic quicksort using Lomuto partition scheme on an array of integers.
Demonstrates array indexing, in-place mutation, and recursion.

### `sha256d.vuma` — double SHA-256

Implements SHA-256 per NIST FIPS 180-4 and applies it twice to produce the
"double SHA-256" (SHA256d) digest used in Bitcoin and many other
cryptographic protocols. Demonstrates u32 arithmetic, bitwise operations,
memory loads/stores, and function calls — one of the most
backend-stressful programs in the suite.

### `pipeline.vuma` — pipe-fork-exec pipeline with signal handling

Creates a mini-shell pipeline: `stdin → [SHA256d filter] → pipe1 → [hex
encoder] → pipe2 → stdout`. Demonstrates `fork`, `execve`, `pipe`, `dup2`,
`waitpid`, signal handling, and inter-process communication.

### `epoll_echo.vuma` — epoll + close

Tests that `epoll_create1` and `close` syscalls work correctly. On
wasm32/WASI, epoll is not supported, so the program just returns 0.

### `thread_pool.vuma` — fixed-size thread pool

Fixed-size thread pool with work stealing and verified synchronization.
Thread pools are a cornerstone of concurrent systems programming; this
example showcases VUMA's concurrency primitives (atomics, channels, locks).

### `doubly_linked_list.vuma` — VUMA's showcase

This program requires `unsafe` in Rust but is fully verified in VUMA.
Demonstrates that the PMT type system can express doubly-linked data
structures that traditional pointer-borrow-checker systems struggle with.

### `mmap_sha256d.vuma` — SHA-256d in an mmap'd buffer

Computes SHA256d of a fixed message in a memory-buffer-based style.
Expected exit code 229 (0xe5). Demonstrates the `mmap` syscall + typed
buffer access.

### `signal_hash.vuma` — reentrant SHA256d with AtomicStore

Computes SHA256d of a fixed message in a "handler" function that can be
re-entered safely via AtomicStore. Demonstrates atomic operations in a
signal-handler-style reentrant context.

### `self_exec.vuma` — fork+exec meta-recursion

A VUMA program that `fork()`s, the child `execve()`s *itself* with
different args. A meta-recursive test of VUMA's process management
primitives.

### `gpio_blink.vuma` — AArch64 hardware access

Demonstrates device memory mapping and VUMA with hardware. Targets
AArch64 (Raspberry Pi 4/5, VisionFive).

---

## How to run

Build the compiler once:

```bash
cargo build --profile release-fast --bin compile_dump
# → target/release-fast/compile_dump
```

### Compile + run on the host

```bash
./target/release-fast/compile_dump examples/fibonacci.vuma /tmp/fib.bin x86_64
/tmp/fib.bin; echo "exit=$?"     # → exit=832040 (truncated to i32 = 832040 mod 256)
```

### Cross-compile to aarch64 and execute under QEMU

```bash
./target/release-fast/compile_dump examples/fibonacci.vuma /tmp/fib.aarch64.bin aarch64
qemu-aarch64 /tmp/fib.aarch64.bin; echo "exit=$?"
```

### Compile with IVE verification

```bash
./target/release-fast/compile_dump examples/sha256d.vuma /tmp/sha256d.bin x86_64 --verify
# Expected: "IVE: Pass passed=N failed=0 total=N"
```

### Run all examples (smoke check)

```bash
for f in examples/*.vuma; do
    ./target/release-fast/compile_dump "$f" /tmp/ex.bin x86_64 --verify \
        && /tmp/ex.bin; echo "  $? : $(basename $f)";
done
```

### Diagnose a single example

The `diag` subcommand of `compile_dump` compiles + runs a file and prints
the exit code:

```bash
./target/release-fast/compile_dump diag x86_64 examples/bsearch.vuma
# Expected: 7 (matches the // Expected exit code: 7 header)
```

### Run on multiple backends

```bash
for arch in x86_64 aarch64 riscv64; do
    echo "=== $arch ===";
    ./target/release-fast/compile_dump examples/sha256d.vuma /tmp/sha.$arch.bin $arch --verify \
        && qemu-$arch /tmp/sha.$arch.bin; echo "  exit=$?";
done
```

---

## See also

- [`docs/language-reference.md`](../docs/language-reference.md) — VUMA
  syntax reference.
- [`docs/building.md`](../docs/building.md) — build + run reference.
- [`docs/contributing.md` §3 PMT-Only Test Policy](../docs/contributing.md#3-pmt-only-test-policy)
  — note that some examples use legacy pointer syntax (`allocate`, `*ptr`,
  `free`) and are pre-PMT.
- [`tests/README.md`](../tests/README.md) — the gold-standard test suite
  (5,832+ programs).
- [`womb/kernel/README.md`](../womb/kernel/README.md) — the VWK kernel
  source tree (75 PMT-pure `.vuma` files).

### Floating-point (3 files)

| File | Description |
|------|-------------|
| `float_math.vuma` | FP type conversions (`inttofloat`/`uinttofloat`/`floattoint`/`floattouint`/`floattofloat`) and arithmetic on `f32`/`f64`; expected exit 100. Demonstrates all 5 `CastKind` variants and the IVE invariants (Liveness, Interpretation, Origin, Cleanup). |
| `fp_bench.vuma` | Performance microbenchmark: 1M `f64` additions in a loop. Used by F4a (FP-aware regalloc) to measure instruction-count reduction. Expected exit 0. |
| `fp_vec_sum.vuma` | Vectorization demo: sum a 1024-element `f32` array. Used by F4c (FP loop vectorization) to verify SSE `ADDPS` / AArch64 `FADD` vector emission. Expected exit 0. |

See `docs/fp_backends.md` for the per-backend FP capability matrix (which of the 19 backends emit native FPU instructions for each operation).
