# Running VUMA Gold Standard Tests

This is the quick-start guide. For full documentation see
[`README.md`](README.md).

## Quick Start

```bash
# Build tools
cargo build --release --bin compile_dump

# Run original 47 examples on x86_64
./target/release/compile_dump diag x86_64 examples

# Run gold standard tests
for f in tests/gold_standard/*/*.vuma; do
    ./target/release/compile_dump "$f" /tmp/test.bin x86_64
    chmod +x /tmp/test.bin
    timeout 3 /tmp/test.bin
    echo "$f: exit=$?"
done

# Run differential testing
./target/release/differential_test

# Run O0 vs O3 comparison
./target/release/opt_level_test
```

## Full CI suite

The CI runner script builds every test driver and runs all five test
categories end-to-end:

```bash
./scripts/ci_run_tests.sh
```

It produces output for:

1. **47 original examples × 7 backends** — `compile_dump diag` for each
   of x86_64, arm32, mips64, aarch64, riscv64, ppc64, loongarch64. For
   non-x86 backends the QEMU user-mode emulator path must be supplied.
2. **Gold standard suite on x86_64** — every `tests/gold_standard/*/*.vuma`
   program compiled and run natively with a 3-second timeout. Crashes
   (exit 139 / 134) and timeouts (exit 124) count as failures.
3. **O0 vs O3 comparison** — `opt_level_test` on the 47 examples.
4. **Fuzzer** — `fuzz_driver --count 50 --seed 42`.

## Running on a single backend

To run a single category on a non-x86 backend (QEMU user-mode required):

```bash
# Stage QEMU binaries under /tmp/qemu_bins/ (one-time setup).
mkdir -p /tmp/qemu_bins
ln -sf $(command -v qemu-aarch64) /tmp/qemu_bins/qemu-aarch64
ln -sf $(command -v qemu-riscv64) /tmp/qemu_bins/qemu-riscv64
ln -sf $(command -v qemu-arm)      /tmp/qemu_bins/qemu-arm
ln -sf $(command -v qemu-mips64el) /tmp/qemu_bins/qemu-mips64el
ln -sf $(command -v qemu-ppc64)    /tmp/qemu_bins/qemu-ppc64
ln -sf $(command -v qemu-loongarch64) /tmp/qemu_bins/qemu-loongarch64

# Run bitwise category on aarch64.
./target/release/compile_dump diag aarch64 \
    tests/gold_standard/bitwise /tmp/qemu_bins/qemu-aarch64
```

## Running a single test program

```bash
# Compile to a binary, then run it.
./target/release/compile_dump tests/gold_standard/memory/mem_alloc_free.vuma \
    /tmp/test.bin x86_64
chmod +x /tmp/test.bin
timeout 3 /tmp/test.bin
echo "exit=$?"
```

The file's header comment documents the expected exit code:

```bash
head -5 tests/gold_standard/memory/mem_alloc_free.vuma
```

## Pass / fail criteria

- **Pass (any)** — ran to completion without crash (139 / 134 / 136) or
  timeout (124). This is `compile_dump diag`'s own definition.
- **Pass (strict)** — exit code matches the file's
  `Expected exit code: N` header comment, or the file has no such header
  and ran without crash/timeout.

## Expected run times (release build, x86_64-only)

| Step                                  | Approx time |
|---------------------------------------|------------:|
| `cargo build --release` (cold)        | 5–8 min     |
| `cargo build --release` (warm cache)  | 30–60 s     |
| 47 examples × 7 backends              | ~2 min      |
| 648 gold-standard programs × x86_64   | ~3 min      |
| O0 vs O3 on 47 examples               | ~30 s       |
| Fuzz 50 programs × 7 backends         | ~1 min      |
| **Total CI wall time (warm cache)**   | **~7 min**  |

## Troubleshooting

- **`qemu-<arch>: command not found`** — install with
  `sudo apt-get install qemu-user qemu-user-static` (Debian/Ubuntu) or the
  equivalent on your distro.
- **`compile_dump: not found`** — run `cargo build --release` first; the
  binary lands in `target/release/`.
- **All tests time out on a backend** — check that the QEMU binary
  actually works on a known-good ELF:
  `qemu-aarch64 /bin/true; echo $?` should print `0`.
- **aarch64 results are non-deterministic** — this is a known VUMA codegen
  bug, not a tester issue; see "Known bugs" in [`README.md`](README.md).
- **mips64 results all exit 1** — the mips64 backend emits big-endian ELF
  but only `qemu-mips64el` (little-endian) is available; QEMU refuses the
  binary. See worklog Task 9-b.

## See also

- [`README.md`](README.md) — comprehensive suite documentation, pass-rate
  tables, known-bug list, and contributor guide.
- [`manifest.json`](manifest.json) — machine-readable index of every
  program.
- [`results_baseline.txt`](results_baseline.txt) — full baseline snapshot
  from Task 8-a.
- [`../../scripts/ci_run_tests.sh`](../../scripts/ci_run_tests.sh) — the
  CI runner script invoked by GitHub Actions.
