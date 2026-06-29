# VUMA Gold-Standard Test Suite

A categorized collection of `.vuma` programs that serve as the gold standard
for testing the VUMA compiler across all 10 supported backends (x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64, LoongArch64, x86_32, RISC-V 32, Wasm32).

The suite is the primary regression-prevention gate for the VUMA compiler. It
covers every language feature, every codegen path, and every backend. Each
program has a header comment that documents the **expected exit code** so that
both "did it crash?" and "did it return the right value?" can be checked
automatically.

## Quick facts

- **Total programs:** 648 across 16 categories (snapshot at Task 11-a)
- **Source:** copies of `examples/*.vuma` plus hundreds of new programs added
  by waves 3-c through 9-b. Originals are preserved in `examples/`.
- **Reference backend:** x86_64 (most stable; used for the headline pass-rate
  number).
- **Other native backends:** aarch64, riscv64, arm32, mips64el, ppc64,
  loongarch64 (all under QEMU user-mode).
- **Wasm32:** not run in CI (no native execution); covered by the Rust
  integration-test suite (`vuma/src/tests/src/wasm_validation.rs`).
- **CI entry point:** [`scripts/ci_run_tests.sh`](../../scripts/ci_run_tests.sh)
  → invoked by [`.github/workflows/vuma-tests.yml`](../../.github/workflows/vuma-tests.yml)
- **Quick-start guide:** [`RUN_TESTS.md`](RUN_TESTS.md)
- **Baseline results:** [`results_baseline.txt`](results_baseline.txt)

## Categories

| # | Category | Programs | Title |
|---|----------|---------:|-------|
| 1 | [`arithmetic/`](arithmetic/) | 68 | Basic Integer Arithmetic |
| 2 | [`bitwise/`](bitwise/) | 46 | Bitwise Operations |
| 3 | [`memory/`](memory/) | 70 | Memory Allocation, Load, and Store |
| 4 | [`control_flow/`](control_flow/) | 47 | Control Flow: if/else, while, for, break, continue |
| 5 | [`pointers/`](pointers/) | 43 | Pointer Arithmetic and Dereference |
| 6 | [`functions/`](functions/) | 44 | Function Calls, Recursion, and Parameters |
| 7 | [`structs/`](structs/) | 42 | Struct and Enum (Tagged Union) Types |
| 8 | [`atomics/`](atomics/) | 32 | Atomic Operations and Memory Ordering |
| 9 | [`u32_arith/`](u32_arith/) | 34 | 32-bit Arithmetic with Overflow Masking |
| 10 | [`edge_cases/`](edge_cases/) | 38 | Boundary Conditions and Unusual Features |
| 11 | [`multi_function/`](multi_function/) | 32 | Many Functions Calling Each Other |
| 12 | [`complex_stores/`](complex_stores/) | 42 | Multi-byte Stores and Computed Addresses |
| 13 | [`nested_loops/`](nested_loops/) | 16 | Two- and Three-Level Nested Loops |
| 14 | [`linked_structures/`](linked_structures/) | 29 | Linked Lists, Trees, and Ring Buffers |
| 15 | [`crypto_patterns/`](crypto_patterns/) | 33 | Cryptographic Hash and Checksum Patterns |
| 16 | [`concurrency/`](concurrency/) | 32 | Lock-Free Structures, Atomics, and Channels |

Each category subdirectory has its own `README.md` describing the kinds of
programs that belong there and the conventions for adding new ones.

### Category descriptions

- **arithmetic** — Fundamental integer arithmetic operators (`+`, `-`, `*`,
  `/`, `%`) on `i32`/`i64`/`u64`/`u32` values, including literal returns,
  single-operator expressions, factorials, GCD/LCM, primality, Fibonacci,
  Ackermann, Collatz, modular exponentiation, continued fractions, Stern–
  Brocot, Catalan, Bell, partition numbers, and more. Smoke-tests the
  register allocator and prologue/epilogue codegen on every backend.
- **bitwise** — Bitwise operators (`&`, `|`, `^`, `<<`, `>>`) and the bit-
  manipulation patterns built on top of them: masks, rotations, popcount,
  Gray codes, byte swaps, priority encoders, Hamming distance, bit
  interleave, bit-twiddle abs, round-up-to-power-of-2.
- **memory** — VUMA's `allocate` / `free` intrinsics and the `*ptr` /
  `*(ptr + offset)` load/store syntax. Ranges from the canonical 4-operation
  program (allocate / write / read / free) through arena-style region
  allocators, ring buffers, stacks/queues, hash tables, and pool/arena
  allocators. Core programs that exercise IVE's Liveness, Exclusivity,
  Origin, and Cleanup invariants.
- **control_flow** — Branching and looping constructs: `if`/`else`, `while`
  with mid-loop condition updates, and `for i in 0..N` ranges. Includes
  nested conditionals and loops with early-exit / sentinel patterns
  (binary search, conditional assignment, return-in-loop).
- **pointers** — `Address`-typed values, pointer arithmetic (`base + offset`),
  and `*ptr` dereference. Stresses the backend's address-computation
  instruction selection (LEA / ADD with shift, etc.) and the IVE bounds
  check on derived pointers. Includes multi-level indirection and pointer-
  to-pointer.
- **functions** — VUMA's calling convention: parameter passing, return values,
  recursion (with base cases), nested calls, and the call/ret prologue/
  epilogue. Includes leaf functions, recursive Fibonacci and quicksort,
  functions returning addresses, and built-in runtime calls (`print_int`).
- **structs** — `struct` definitions, struct-literal initialization
  (`Foo { a: 1, b: 2 }`), field access via `(*ptr).field`, and `enum`
  (tagged-union) types with `match` expressions. Exercises struct layout /
  field-offset computation in codegen and the discriminant + payload memory
  model for enums.
- **atomics** — VUMA's atomic primitives: `atomic_load`, `atomic_store`,
  `atomic_cas`, `AtomicU64` with `fetch_add` / `fetch_sub` /
  `compare_exchange`, and the `Acquire` / `Release` / `Relaxed` orderings.
  These lower to `LDXR`/`STXR` (AArch64), `LOCK CMPXCHG` (x86_64), and
  equivalent atomic sequences on the other backends.
- **u32_arith** — `u32` arithmetic that explicitly masks results with
  `& 4294967295` to defeat 64-bit host widening. Dominant pattern in
  SHA-256 / SHA256d code and any code that must preserve exact 32-bit
  wrap-around semantics on a 64-bit ISA.
- **edge_cases** — Unusual corners of the language: hardware register access
  via `map_device`, floating-point type conversions (`f32`/`f64`,
  `inttofloat`, `floattoint`), the `extern "C"` FFI block syntax,
  boundary allocations (0, 1, max bytes), constant folding, and self-
  overwrite patterns.
- **multi_function** — Large function counts and dense call graphs. Stresses
  the Static Call Graph (SCG) builder, the function-offset resolution pass
  (`resolve_call_relocs`), and the `BL`/`CALL` relocation range checks.
  Also useful for verifying DWARF subprogram DIE generation.
- **complex_stores** — Multi-byte stores (u32, u64) as sequences of byte
  stores, or writes to addresses computed from complex expressions.
  Exercises the codegen's store-lowering paths and the IVE bounds analysis
  on non-trivial address expressions.
- **nested_loops** — Deeply nested loops (matrix-multiply shape,
  `for i { for j { for k { ... } } }`). Stresses the backend's loop-label
  register allocation, branch-target encoding, and instruction-cache
  footprint.
- **linked_structures** — Pointer-linked data structures: singly- and
  doubly-linked lists, AVL-balanced trees with rotations, ring buffers,
  sorted maps, and any structure where nodes reference each other via
  `Address` fields. The showcase programs for VUMA's IVE — they require
  `unsafe` in Rust but verify cleanly in VUMA.
- **crypto_patterns** — Cryptographic primitives: SHA-256 (full and single-
  round), SHA256d (double SHA-256), CRC32, memory-mapped file hashing,
  XOR ciphers, S-boxes, byte-swap, popcount, Gray code, Adler-32,
  Fletcher-32. The most algorithmically dense programs in the suite.
- **concurrency** — Concurrent programming: lock-free queues, thread pools
  with mutex/condvar, MPSC channels, fork/exec pipelines, epoll servers,
  signal handlers. Exercises VUMA's `spawn`/`join`, `Mutex`/`Condvar`,
  `Channel`, FFI to Linux syscalls, and IVE's concurrent-access
  verification.

## Layout

```
tests/gold_standard/
+-- manifest.json              # machine-readable index of all categories
+-- README.md                  # this file (comprehensive docs)
+-- RUN_TESTS.md               # quick-start guide
+-- build_categories.py        # script that (re)generated this tree
+-- results_baseline.txt       # baseline pass-rate snapshot (Task 8-a)
+-- differential_results.txt   # 100-program × 7-backend differential run
+-- differential_raw.tsv       # raw TSV of the differential run
+-- <category>/
    +-- README.md              # what kinds of tests belong here
    +-- *.vuma                 # the test programs
```

## How to run tests

See [`RUN_TESTS.md`](RUN_TESTS.md) for the quick-start commands. In summary:

```bash
# Build the test-driver binaries (one-time per checkout).
cd /tmp/my-project
cargo build --release --bin compile_dump --bin differential_test \
                       --bin opt_level_test --bin fuzz_driver

# Run the full gold-standard suite on x86_64 (about 2 minutes).
./scripts/ci_run_tests.sh

# Run a single category on x86_64.
for f in tests/gold_standard/bitwise/*.vuma; do
    ./target/release/compile_dump "$f" /tmp/test.bin x86_64 2>/dev/null
    chmod +x /tmp/test.bin
    timeout 3 /tmp/test.bin
    echo "$(basename $f): exit=$?"
done

# Run on a non-x86 backend (QEMU user-mode required).
./target/release/compile_dump diag aarch64 tests/gold_standard/bitwise \
    /tmp/qemu_bins/qemu-aarch64
```

### Pass / fail criteria

The CI runner uses two distinct pass criteria:

- **Pass (any)** — the binary ran to completion without crash or timeout
  (i.e., exit code is not 124/139/134/136). This is `compile_dump diag`'s
  own definition.
- **Pass (strict)** — exit code matches the file's
  `Expected exit code: N` header comment, or the file has no such header
  and ran without crash/timeout. This is what the per-category README
  baseline tables report.

A **Crash** is a process killed by a signal (`SIGSEGV=139`, `SIGABRT=134`,
`SIGFPE=136`, etc.); a **Timeout** is `timeout 3` killing the process
(exit 124); a **CompileFail** is `compile_dump` failing to produce a binary
at any stage (parse / SCG / IR / regalloc / encode); a **WrongExit** is a
clean run whose exit code does not match the documented expected value.

## Current pass rates per backend

These numbers come from the Task 8-a baseline run
([`results_baseline.txt`](results_baseline.txt)) and the Task 9-a differential
run ([`differential_results.txt`](differential_results.txt)). The snapshot
of the suite at the time of that baseline had 527 programs; the suite has
since grown to 648. Re-running CI on the current snapshot will produce
slightly different absolute counts but the same per-category pattern.

### Step 1 — original 47 examples × 7 backends (`compile_dump diag`)

| Backend     | Total | Pass | Crash | Timeout | CompileFail |
|-------------|------:|-----:|------:|--------:|------------:|
| x86_64      |    47 |   47 |     0 |       0 |           0 |
| arm32       |    47 |   47 |     0 |       0 |           0 |
| mips64      |    47 |   47 |     0 |       0 |           0 |
| aarch64     |    47 |   25 |    14 |       8 |           0 |
| riscv64     |    47 |   46 |     0 |       1 |           0 |
| ppc64       |    47 |   40 |     0 |       7 |           0 |
| loongarch64 |    47 |    5 |    21 |      21 |           0 |

### Step 2 — gold-standard suite × x86_64 (reference backend, 527-file snapshot)

Overall:

| Metric        | Count |  Rate |
|---------------|------:|------:|
| Total         |   527 |       |
| Pass (any)    |   519 | 98.5% |
| Pass (strict) |   403 | 76.5% |
| Crash         |     8 |       |
| Timeout       |     0 |       |
| CompileFail   |     0 |       |
| Wrong-exit    |   116 |       |

Per-category (x86_64 strict pass / total):

| Category           | Total | Strict | Any | WrongExit | Crash |
|--------------------|------:|-------:|----:|----------:|------:|
| arithmetic         |    68 |     46 |  67 |        21 |     1 |
| atomics            |    32 |      4 |  31 |        27 |     1 |
| bitwise            |    46 |     46 |  46 |         0 |     0 |
| complex_stores     |    22 |     22 |  22 |         0 |     0 |
| concurrency        |    22 |     20 |  20 |         0 |     2 |
| control_flow       |    47 |     15 |  47 |        32 |     0 |
| crypto_patterns    |    23 |     22 |  23 |         1 |     0 |
| edge_cases         |    18 |     17 |  17 |         0 |     1 |
| functions          |    34 |     34 |  34 |         0 |     0 |
| linked_structures  |    18 |     18 |  18 |         0 |     0 |
| memory             |    70 |     49 |  70 |        21 |     0 |
| multi_function     |    22 |     22 |  22 |         0 |     0 |
| nested_loops       |    16 |     16 |  16 |         0 |     0 |
| pointers           |    33 |     23 |  31 |         8 |     2 |
| structs            |    32 |     25 |  31 |         6 |     1 |
| u32_arith          |    24 |     24 |  24 |         0 |     0 |
| **TOTAL**          |   527 |    403 | 519 |       116 |     8 |

### Step 4 — 50-test stratified sample × 7 backends (strict pass)

| Backend     | Total | Strict | Any | WrongExit | Crash | Timeout |
|-------------|------:|-------:|----:|----------:|------:|--------:|
| x86_64      |    50 |     37 |  48 |        11 |     2 |       0 |
| aarch64     |    50 |     31 |  38 |         7 |    11 |       1 |
| riscv64     |    50 |     37 |  48 |        11 |     2 |       0 |
| arm32       |    50 |     35 |  48 |        13 |     2 |       0 |
| mips64      |    50 |     37 |  48 |        11 |     2 |       0 |
| ppc64       |    50 |     37 |  48 |        11 |     2 |       0 |
| loongarch64 |    50 |     25 |  37 |        12 |     5 |       8 |

### Differential agreement

- Original 47 examples × 7 backends (all-must-agree on exit code AND stdout):
  **0 / 47** programs have all 7 backends agreeing.
- 50-test sample × 7 backends (all-must-agree on exit code only):
  **26 / 50** programs have all 7 backends agreeing.

The 0/47 figure on the original examples is dominated by exit-code divergence
(`exit=0` on stable backends vs. `exit=255` from QEMU capturing a guest fault
on aarch64/loongarch64). The 52% agreement on the gold-standard sample is
much better because most gold-standard programs are straight-line code that
avoids the known universal codegen gaps described below.

## Known bugs and their impact

The gold-standard suite is designed to **surface** codegen bugs, so the
"wrong-exit" rows above are not test bugs — they are the suite doing its
job. The bugs responsible for the bulk of the wrong-exit cases are
documented here so that downstream fix waves can target them.

### Universal codegen gaps (hit by all 7 backends uniformly)

1. **for/while body assignment propagation** (worklog Task 6-e). Variables
   assigned inside a loop body do not propagate to the outer scope, so the
   post-loop read returns the pre-loop value (typically 0). Affects ~32
   control_flow tests on x86_64 and 3/3 sampled control_flow tests on every
   backend.
2. **SCG-to-codegen atomic bridge** (worklog Task 6-d / 7-d).
   `atomic_store` / `atomic_load` / `atomic_cas` are silently dropped
   during SCG→IR lowering, so the post-atomic read returns 0. Affects ~27
   atomics tests on x86_64 and 3/3 sampled atomics tests on every backend.
3. **Store-loaded-variable** (worklog Task 7-d). `*ptr = loaded_var`
   silently drops the store; `*ptr = loaded_var + 0` works. Affects ~21
   memory tests and ~8 pointer tests on x86_64, plus shared structs /
   `struct_swap_fields` cases.
4. **Hex-literal store bug** (worklog Task 7-d). `*ptr = 0xNN` followed by
   a load sometimes returns 0 instead of `0xNN`. Found by several
   `mem_*_pattern_42` / `mem_*_pattern_255` tests.
5. **First-allocation overwrite bug** (worklog Task 7-d). The first
   `allocate` in a program can be silently overwritten by subsequent
   allocations in some codegen paths.

### Backend-specific gaps

1. **aarch64 — non-deterministic crashes.** 14/47 original examples crash
   with sig 11 and 8/47 time out. Re-running the *same* binary gives
   different exit codes run-to-run (e.g. `arena_allocator` returns 1
   cleanly on one run and 139 on the next). Indicates uninitialized
   register or stack slot in the prologue / regalloc spilling path
   (worklog Task 8-a).
2. **loongarch64 — broken call/return path.** 21/47 original examples
   crash (mix of sig 11 and sig 5 = trap), 21/47 time out. The sample
   shows 0/3 functions and 0/3 multi_function strict-pass — the call /
   return convention or prologue / epilogue appears broken on this
   backend specifically (worklog Task 8-a).
3. **ppc64 — Address-typed return calling-convention bug.** 7/47 timeouts
   (atomics_demo, ffi_demo, float_math, lock_free_queue, test_print,
   test_print2, thread_pool). The `*_func_load` / `struct2_func_load` /
   `*_address_return` tests typically exit 0 on ppc64 due to an
   Address-typed-return calling-convention bug (worklog Task 9-b).
4. **mips64 — big-endian ELF vs. little-endian QEMU.** The mips64 backend
   emits big-endian MIPS64 ELF, but the only available QEMU binary is
   `qemu-mips64el` (little-endian). QEMU refuses the resulting binaries
   with an "Invalid ELF" error (worklog Task 9-b). MIPS numbers in the
   differential tester are therefore not real execution results — they
   reflect QEMU's ELF rejection (typically exit code 1).
5. **riscv64 — solid except for one timeout.** 1/47 timeout
   (`lock_free_queue` only). Otherwise the most stable non-x86 backend.

### IVE (Invariant Verification Engine) gaps

The IVE's current detection capabilities (documented by the property-test
suite at `tests/property_tests.rs`, worklog Task 10-a):

| Property                | IVE status                              |
|-------------------------|-----------------------------------------|
| Double-free             | **Detected** (Cleanup invariant)        |
| Memory leak (no `free`) | **Detected** (Liveness invariant)       |
| Use-after-free          | Not detected (known gap)                |
| Buffer overflow (read)  | Not detected (known gap)                |
| Buffer overflow (write) | Not detected (known gap)                |
| Null pointer deref      | Not detected (known gap)                |
| Uninitialized memory    | Not detected (known gap)                |
| Valid program w/ `free` | **False positive** (spurious "leak" report) |

The false positive on valid programs is caused by the SCG builder not
always populating `Deallocation.allocation_node`, so the `LivenessVerifier`
cannot see the matching free. The Cleanup invariant, by contrast, does see
the free. (worklog Task 10-a.)

## How to add new tests

1. **Pick a category.** Read the category's `README.md` to confirm the kind
   of program belongs there. If it could plausibly fit in two categories,
   prefer the one whose other programs are most similar in shape.
2. **Write the program.** Use the `*.vuma` syntax described in
   [`docs/language-reference.md`](../../docs/language-reference.md). The
   program must:
   - Parse cleanly under `vuma_parser`.
   - Compile to a binary under `compile_dump` (no `CompileFail`).
   - Exit with a predictable code on x86_64.
3. **Document the expected exit code.** Add a header comment to the file:
   ```vuma
   // Expected exit code: 42
   // Tests: store-loaded-variable workaround (`*p = x + 0`).
   fn main() -> i32 {
       let p = allocate(1);
       let x = 42;
       *p = x + 0;
       let r = *p;
       free(p);
       return r;
   }
   ```
4. **Avoid known-broken patterns** unless the test is specifically meant
   to document a known gap (in which case name it `*_known_gap.vuma` and
   note the worklog Task ID in the header). The currently-broken patterns
   are listed above under "Known bugs and their impact".
5. **Run the test locally** to confirm it returns the expected exit code
   on x86_64:
   ```bash
   ./target/release/compile_dump tests/gold_standard/<cat>/new_test.vuma \
       /tmp/new_test.bin x86_64
   chmod +x /tmp/new_test.bin
   timeout 3 /tmp/new_test.bin
   echo "exit=$?"
   ```
6. **Update the manifest.** Append the file name to the appropriate
   category's `programs` array in [`manifest.json`](manifest.json) and bump
   the `program_count` for that category and the `total_programs` field.
   (The `build_categories.py` script can also regenerate the manifest if
   you prefer.)
7. **Update the category README.** Add a one-line entry to the file list
   in the category's `README.md` so the human-readable index stays in sync.
8. **Re-run the baseline** if your addition changes a category's strict-pass
   rate materially. The `results_baseline.txt` file should be regenerated
   by a future wave after the next full CI run.

### Naming conventions

- Original example programs keep their `examples/` names (e.g.
  `arena_allocator.vuma`).
- New programs in a category use a short prefix matching the category
  (`mem_*` for memory, `bit_*` for bitwise, `cf_*` for control_flow,
  `ptr_*` for pointers, `fn_*` for functions, `struct_*` for structs,
  `atom_*` for atomics, `arith_*` for arithmetic, `cs_*` for
  complex_stores, `nl_*` for nested_loops, `ls_*` for linked_structures,
  `crypto_*` for crypto_patterns, `conc_*` for concurrency, `mf_*` for
  multi_function, `s3_*` for stage-3 additions).
- A `_2` / `2_` suffix on a name denotes a second batch of similar tests
  added by a later wave (e.g. `mem2_*` is the second memory batch,
  `cf2_*` is the second control_flow batch).

## See also

- [`manifest.json`](manifest.json) — full machine-readable index.
- [`RUN_TESTS.md`](RUN_TESTS.md) — quick-start guide.
- [`results_baseline.txt`](results_baseline.txt) — baseline pass-rate
  snapshot from Task 8-a (the source of every number in the pass-rate
  tables above).
- [`differential_results.txt`](differential_results.txt) — 100-program ×
  7-backend differential run from Task 9-a.
- [`../../examples/README.md`](../../examples/README.md) — narrative
  descriptions of each original example program.
- [`../../scripts/ci_run_tests.sh`](../../scripts/ci_run_tests.sh) —
  CI entry point that runs the full suite.
- [`../../.github/workflows/vuma-tests.yml`](../../.github/workflows/vuma-tests.yml)
  — GitHub Actions workflow that invokes the CI script on every push and
  pull request.
- Project worklog (`worklog.md`) — Tasks 1-c created the initial suite;
  Tasks 3-c through 9-b expanded it; Task 8-a produced the baseline;
  Task 10-a produced the IVE property-test suite; Task 11-a created this
  documentation and the CI scripts.
