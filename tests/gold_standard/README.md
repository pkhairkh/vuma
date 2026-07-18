# VUMA Gold Standard Test Suite

The **gold-standard suite** is the canonical VUMA compiler correctness
benchmark. Each `.vuma` file is a self-contained VUMA program. The
program's expected behavior is declared in a header comment of the form

```vuma
// Expected exit code: 42
```

and a test harness compares the program's actual exit code against that
integer. A mismatch is a regression. Categories whose tests are *meant
to fail* (see `pmt_wave3_negative/` below) carry a different header and
a different pass/fail convention.

This directory was reduced from ~5,851 `.vuma` files to **1,502** on
**2026-07** by removing:

| Removed                                                 | Count |
|---------------------------------------------------------|------:|
| `sN_<family>.vuma` sweep near-duplicates (1 kept/family) | 3,842 |
| Hollow PMT stubs (header describes real code, body returns a constant) | 507 |
| `kernel_boot/` category (had only `hello.expected`, no `.vuma`) | 1 dir |
| `build_categories.py` (dead footgun — hard-coded `/tmp/my-project/`) | 1 file |
| Run-log artifacts at the suite root (no script consumes them) | 6 files |

The pre-cleanup `manifest.json` claimed 704 programs while the disk
held ~5,851; both numbers were wrong. The new `manifest.json` is
regenerated from the cleaned disk and lists every `.vuma` actually
present.

## Categories

Counts below are read from the regenerated `manifest.json` (36
categories, 1,502 programs total).

### Core feature categories (the "16 main")

| Directory           | Title                  | Count |
|---------------------|------------------------|------:|
| `arithmetic/`       | Arithmetic             |   136 |
| `atomics/`          | Atomics                |    52 |
| `bitwise/`          | Bitwise Operations     |   123 |
| `complex_stores/`   | Complex Stores         |    94 |
| `concurrency/`      | Concurrency            |    56 |
| `control_flow/`     | Control Flow           |    96 |
| `crypto_patterns/`  | Cryptographic Patterns |   105 |
| `edge_cases/`       | Edge Cases             |   142 |
| `functions/`        | Functions              |    83 |
| `linked_structures/`| Linked Structures      |    58 |
| `memory/`           | Memory                 |    38 |
| `multi_function/`   | Multi-Function         |    77 |
| `nested_loops/`     | Nested Loops           |   175 |
| `pointers/`         | Pointers               |    42 |
| `structs/`          | Structs                |    32 |
| `u32_arith/`        | u32 Arithmetic         |    96 |

### Curated wave categories (kept verbatim — do NOT sweep-dedupe)

| Directory              | Title                                | Count |
|------------------------|--------------------------------------|------:|
| `pmt_wave1/`           | PMT Wave 1                           |     5 |
| `pmt_wave2/`           | PMT Wave 2                           |    16 |
| `pmt_wave3_negative/`  | PMT Wave 3 — Negative (must-fail)    |     5 |
| `pmt_wave5/`           | PMT Wave 5                           |     5 |
| `pmt_wave7/`           | PMT Wave 7                           |     5 |
| `pmt_wave8/`           | PMT Wave 8                           |     3 |
| `pmt_wave9/`           | PMT Wave 9                           |     5 |
| `pmt_wave10/`          | PMT Wave 10                          |     3 |
| `ffi_wave0/`           | FFI Wave 0 (parse-only)              |     9 |
| `ffi_wave1/`           | FFI Wave 1                           |     3 |
| `ffi_wave2/`           | FFI Wave 2                           |     4 |
| `ffi_wave3/`           | FFI Wave 3                           |     4 |
| `ffi_wave4/`           | FFI Wave 4                           |     2 |
| `arena_wave0/`         | Arena Allocator Wave 0               |     3 |
| `arena_wave1/`         | Arena Allocator Wave 1               |     4 |
| `arena_wave2/`         | Arena Allocator Wave 2               |     1 |
| `float_arith/`         | Float Arithmetic                     |     8 |
| `float_casts/`         | Float Casts                          |     7 |
| `float_mem/`           | Float Memory                         |     4 |
| `kernel_crypto/`       | Kernel Crypto                        |     1 |

The `pmt_wave*`, `ffi_wave*`, `arena_wave*`, `float_*`, and
`kernel_crypto/` directories are smaller, hand-curated suites that
target specific features. They are intentionally NOT subject to the
sweep-deduplication that collapsed the 16 main categories — every file
in them is a deliberate test.

## The legacy `sN_` filename prefix

Many files in the 16 main categories carry a prefix of the form
`s<number>_`, e.g. `s3_mf_chained_adders.vuma`, `s37_mf_three_chain.vuma`.

**What it means.** Each `sN_` prefix was a *sweep-batch ID* — at some
point in the suite's history a now-deleted generator produced ~16
near-duplicate variants per family, one per sweep ID (`s3`, `s4`, …,
`s106`), where the variants differed only in the integer constants used
(the code shape, function structure, and expected behavior were
identical). On 2026-07 these near-duplicate families were collapsed to
**one representative per family** (the file with the smallest sweep ID,
deterministically). The other ~3,842 duplicates were deleted.

**Why the prefix is retained.** The surviving representative still
carries its original `sN_` prefix. Renaming it would risk breaking any
external reference (the audit found none, but renaming is still riskier
than retaining). Treat the prefix as cosmetic — it has no operational
meaning going forward. **Do not use the `sN_` prefix for new tests.**

## File-naming convention (going forward)

New gold-standard tests should be named

```
<feature>_<variant>.vuma
```

for example `arith_add_overflow.vuma`, `ptr_two_cell_diff.vuma`,
`crypto_xor_pair.vuma`. Do **not** introduce new `sN_` prefixes. The
existing `sN_` files are legacy and are not being renamed.

## The `pmt_wave3_negative/` must-fail convention

The five files in `pmt_wave3_negative/` (`bad_offset.vuma`,
`bad_type.vuma`, `oob_field.vuma`, `unknown_layout.vuma`,
`write_after_consume.vuma`) are **expected to fail IVE verification**.
Their header reads

```vuma
// Expected: IVE rejects at verification (VerificationLevel::Pmt).
```

A test harness running these files must classify a verification failure
(`compile_fail`) as **PASS** and a successful compile as **FAIL**. These
files do not carry an `Expected exit code:` header because they never
produce an exit code — the compiler must reject them before codegen.

## Running the suite

### Canonical runner — `scripts/run_all_gold.sh`

```bash
./scripts/run_all_gold.sh [jobs]
```

Runs every `.vuma` in this directory across the available native
backends in parallel. For each file, the script:

1. Extracts the expected exit code from the
   `// Expected exit code: N` header (regex
   `^// *[Ee]xpected exit code: *([0-9]+)`).
2. Compiles the program with `vuma compile`.
3. Runs the compiled binary under each backend.
4. Compares the actual exit code to the expected one and records
   pass/fail.

Output goes to `/tmp/vuma_results/<backend>.tsv` and
`/tmp/vuma_results/summary.txt`.

### Cross-backend runner — `scripts/kernel_parity.sh`

```bash
./scripts/kernel_parity.sh
```

Runs a curated subset of the suite across all available backends
(native x86_64 + QEMU for aarch64 / riscv64 / arm32 / mips64 / ppc64,
plus the wasm32 backend) and reports per-backend parity. Use this to
catch backend-specific regressions.

### Must-fail handling

When running `pmt_wave3_negative/`, the harness must invert the
pass/fail decision: `compile_fail` (verification rejection) is PASS,
successful compile is FAIL. `run_all_gold.sh` skips
`pmt_wave3_negative/` by design; these files are run by the dedicated
negative-test path in CI.

## History

* **Pre-2026-07:** The suite grew to ~5,851 `.vuma` files, of which
  ~89% was noise — `sN_` sweep near-duplicates, hollow PMT stubs whose
  headers claimed to test real features (adler32, hash table, SHA256d,
  spinlock, …) but whose bodies just stored and returned a constant,
  shrunk-from-example stubs whose bodies were trivial while the real
  implementation lived in `examples/`, committed run-log artifacts at
  the suite root, and a dead `kernel_boot/` category containing only
  `hello.expected`. The `manifest.json` claimed 704 programs (also
  wrong).
* **2026-07 (this cleanup):** The 6 run-log artifacts, the dead
  `build_categories.py` script, and the empty `kernel_boot/` directory
  were deleted. A classification script identified 507 hollow PMT
  stubs (header describes a real program, body is
  `state_new(Layout); c.v = K; return c.v;`) and deleted them.
  The surviving `sN_<family>.vuma` files were grouped by family name
  (leading `s\d+_` stripped) and each family collapsed to its
  smallest-`sN` representative, deleting 3,842 sweep duplicates.
  `manifest.json` was regenerated from the cleaned disk; `README.md`
  was rewritten. Final count: **1,502 `.vuma` files across 36
  categories**.
