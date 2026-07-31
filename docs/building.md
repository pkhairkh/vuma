# Building and Development Guide

This document covers everything you need to build the VUMA compiler,
run the test suite, and develop against the codebase. For known
limitations and current caveats, see [`caveats.md`](caveats.md).

## Prerequisites

### Rust toolchain

VUMA requires Rust nightly pinned to `nightly-2026-03-01`. The
`rust-toolchain.toml` enforces this automatically — any `cargo`
invocation in the repo will install and select the pinned toolchain
on first use.

```bash
# Manual install (if you prefer not to rely on rust-toolchain.toml auto-install):
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly
rustup toolchain install nightly-2026-03-01 --profile default
rustup component add rustfmt clippy rust-src
rustup target add aarch64-unknown-linux-gnu aarch64-unknown-none
```

The toolchain file also pins the `aarch64-unknown-linux-gnu` and
`aarch64-unknown-none` targets (needed by the bare-metal kernel
crate and `naked_asm` usage).

### Z3 SMT solver (hard build dependency)

Z3 is now a **hard** build dependency — the `vuma-ive` crate links
against the system `libz3` (`z3 = "0.20"` in `src/ive/Cargo.toml`)
and the build will fail at link time if Z3 is not present. Z3 is what
discharges the IVE verification conditions (contract / invariant /
linearity / information-flow); without it the compiler cannot produce
a verified binary.

```bash
# Debian / Ubuntu
sudo apt install libz3-dev

# macOS (Homebrew)
brew install z3

# Arch Linux
sudo pacman -S z3
```

Verify the install:

```bash
pkg-config --modversion z3     # e.g. "4.13.0"
```

The `pi5_test_suite.sh` runner also probes for Z3 via `pkg-config`
and will attempt to install `libz3-dev` automatically on Debian-family
hosts if it is missing.

### QEMU user-mode emulation (10.0+)

Cross-architecture testing needs one QEMU user-mode binary per
non-host backend. **QEMU 10.0 or newer is recommended** — older
7.2.0-1 static builds still work for most backends but several
workarounds that targeted QEMU 7.2 bugs have been removed, so old
QEMU may surface encoding-related failures that no longer reproduce
on 10.0+.

Install via your distribution's package manager (preferred — gives
you 10.0+ on recent distros):

```bash
# Debian / Ubuntu (QEMU 10.x is in Debian trixie / Ubuntu 25.04+)
sudo apt install qemu-user-static

# Arch
sudo pacman -S qemu-user-static
```

Or fetch the multiarch static binaries manually:

```bash
mkdir -p ~/.local/bin
for arch in aarch64 arm armeb aarch64_be x86_64 i386 \
            mips64 mips64el mips riscv64 riscv32 \
            ppc64 ppc64le sparc64 s390x m68k hppa alpha loongarch64; do
  curl -sL "https://github.com/multiarch/qemu-user-static/releases/latest/qemu-${arch}-static" \
    -o ~/.local/bin/qemu-${arch}-static
  chmod +x ~/.local/bin/qemu-${arch}-static
done
```

The 19-backend test matrix maps VUMA ISA names to QEMU binary names
as follows (the two non-obvious ones are `arm32 → qemu-arm-static`
and `mips64 → qemu-mips64el-static`, because VUMA's `mips64` backend
emits a **little-endian** MIPS64 ELF):

| VUMA ISA | QEMU binary |
|----------|-------------|
| `x86_64` | (native; no emulator) |
| `aarch64` | `qemu-aarch64-static` |
| `aarch64_be` | `qemu-aarch64_be-static` |
| `riscv64` | `qemu-riscv64-static` |
| `riscv32` | `qemu-riscv32-static` |
| `arm32` | `qemu-arm-static` |
| `armeb` | `qemu-armeb-static` |
| `x86_32` | `qemu-i386-static` |
| `mips64` | `qemu-mips64el-static` (little-endian) |
| `mips64be` | `qemu-mips64-static` (big-endian) |
| `ppc64` | `qemu-ppc64-static` |
| `ppc64le` | `qemu-ppc64le-static` |
| `sparc64` | `qemu-sparc64-static` |
| `s390x` | `qemu-s390x-static` |
| `m68k` | `qemu-m68k-static` |
| `alpha` | `qemu-alpha-static` |
| `hppa` | `qemu-hppa-static` |
| `loongarch64` | `qemu-loongarch64-static` |
| `wasm32` | (run via `wasmtime`, see below) |

### Wasmtime (for the `wasm32` backend)

The `wasm32` row of the test matrix runs under `wasmtime`. **v29.0 or
newer is required** — older `wasmtime` does not support the WASI
preview features the wasm32 backend emits and will reject the module.

```bash
WASMTIME_VER=v29.0.1   # or any newer release
ARCH=$(uname -m)       # x86_64 or aarch64
curl -L "https://github.com/bytecodealliance/wasmtime/releases/download/${WASMTIME_VER}/wasmtime-${WASMTIME_VER}-${ARCH}-linux.tar.xz" \
  | tar xJ
cp wasmtime-*/wasmtime ~/.local/bin/
pip install wasmtime   # Python bindings used by scripts/wasm32_runner.py
```

Verify:

```bash
wasmtime --version     # wasmtime-cli 29.x (or newer)
```

## Building

### Quick start

```bash
# Iterative build (fast, used by the test suite):
cargo build --profile release --bin compile_dump --bin dump_ir

# Production build (slow, optimised):
cargo build --profile release --bin compile_dump
```

The binaries land in `target/release/` or `target/release/`
respectively.

### Build profiles

| Profile | Use case | LTO | Opt level | Codegen units |
|---------|----------|-----|-----------|---------------|
| `dev` (default) | Local development / debugging | off | 0 | 256 |
| `release` | Test-suite runs, iterative work | **off** | 3 | **16** |
| `release` | Production / release builds | **on (fat)** | 3 | **1** |

The `release` profile (defined in `Cargo.toml`) deliberately
disables LTO and bumps `codegen-units` to 16 so a from-scratch build
that would take 10+ minutes on a Pi 5 with the `release` profile
completes in ~1–2 minutes. Runtime is still `O3` (so QEMU-emulated
executions stay fast); only the link-time optimisation pass is
skipped, costing ~5–10% runtime but ~5–10× build-time speedup. Use
`release` for everyday work and `release` only when you need
maximum runtime performance or are cutting a release artifact.

### Verifying the Z3 link

If your build fails with `could not find z3` / `-lz3 not found`,
confirm the system library is installed and discoverable:

```bash
pkg-config --modversion z3     # should print e.g. "4.13.0"
pkg-config --libs z3           # should print e.g. "-lz3"
```

If `pkg-config` succeeds but the link still fails, ensure
`PKG_CONFIG_PATH` includes the directory containing `z3.pc` (often
`/usr/lib/<triplet>/pkgconfig` on Debian multiarch).

## Compiler CLI

```bash
vuma --help
```

Common flags (run `vuma --help` for the full list):

| Flag | Default | Meaning |
|------|---------|---------|
| `--opt-level <O0\|O1\|O2\|O3>` | `O3` | Codegen optimisation level. |
| `--isa <isa>` | `aarch64` | Target backend. One of the 19 ISAs in the table above. |
| `--strict-ive` | OFF | Promote the IVE e-graph soundness advisory to a HARD-FAIL. The channel/linearity/information-flow/session-type verifiers always HARD-FAIL. |
| `--max-expr-depth <N>` | `1024` | Maximum expression nesting depth. `0` is rejected. |
| `--allow-inconclusive` | OFF | Soft-pass on `OverallVerdict::Inconclusive` from the PMT IVE verifiers. Without it, `Inconclusive` is a HARD compile error. |

> **Removed flags.** `--safe`, `--no-memory-safety`, and `--repl`
> have been removed. Runtime bounds-check injection is always on;
> there is no REPL. See [`caveats.md`](caveats.md) §5 for the full
> list of removed flags.

## Running tests

### Full test suite (`pi5_test_suite.sh`)

The end-to-end runner. It builds the compiler, probes/installs Z3 and
QEMU if missing, runs the gold-standard `.vuma` programs across the
19-backend matrix, and archives the results.

```bash
bash scripts/pi5_test_suite.sh --workers 4 --fresh --verify
```

Flags:

| Flag | Meaning |
|------|---------|
| `--workers N` | Parallel workers for non-IPC tests (default 4). |
| `--fresh` | Clear the checkpoint and run every test from scratch. |
| `--verify` | Enable IVE verification (always on in production; this just makes the runner assert it). |
| `--commit` | **Opt-in** auto-stage + commit + push of test results to `origin HEAD`. Default OFF. |
| `--dry-run` | Show what *would* be committed (staged files, byte sizes, proposed message, `git status --porcelain`) without running `git commit`. |
| `--no-push` | Legacy flag; equivalent to the new default (no commit/push). |
| `--trend [N]` | Print a pass-rate trend table over the last N archived runs (default 10) from `test_results/history/` and exit without running the suite. |
| `--trend-n N` | Override the `--trend` default of 10. |
| `--skip-build` | Skip the compiler rebuild step. |
| `--backends LIST` | Restrict to a subset of backends (space-separated). |
| `--profile NAME` | Use a specific Cargo profile (default `release`). `--release` is a shortcut for `--profile release` (slow LTO build). |

Flag precedence for the commit step: `--no-push` → `--dry-run` →
`--commit` → default-off summary.

#### IPC worker cap (`VUMA_IPC_WORKER_CAP`)

IPC tests (`fork + exec + wait` under QEMU) always run with a reduced
worker count regardless of `--workers` — high parallelism causes
fork+exec timeouts from QEMU translation-cache warm-up and
pipe-buffer contention. The cap defaults to **3** and is configurable:

```bash
# Default (3 workers for the IPC phase, even with --workers 8):
bash scripts/pi5_test_suite.sh --workers 8

# CI host with ≥16 cores and no QEMU contention:
VUMA_IPC_WORKER_CAP=8 bash scripts/pi5_test_suite.sh --workers 8
```

Invalid / non-integer values fall back to 3; values `<1` are floored
to 1. The chosen value is logged (`[K11C] VUMA_IPC_WORKER_CAP=N …`).
See [`caveats.md`](caveats.md) §4.1 for the rationale.

### 19-backend smoke matrix

```bash
# Via the dedicated matrix script:
bash scripts/vuma_test_matrix_19backends.sh

# Or via the QEMU smoke test (subset of gold-standard programs):
bash scripts/qemu_smoke_test.sh
# make qemu-smoke   # equivalent
```

The matrix script (`scripts/vuma_test_matrix_19backends.sh`) builds
`compile_dump` once, then compiles and runs every gold-standard IPC
test on all **19** backends (18 QEMU-emulated + `wasm32` via
`wasmtime`), reporting a pass/fail matrix. Useful environment-variable
overrides for `qemu_smoke_test.sh`:

- `VUMA_SMOKE_ISAS="x86_64 aarch64 wasm32"` — restrict to a subset.
- `VUMA_SMOKE_TESTS="arithmetic/arith_add_basic arithmetic/test_exit"` — restrict tests.
- `VUMA_SMOKE_NO_BUILD=1` — skip the rebuild step (assume a previously-built binary).

### Individual tests

```bash
# Compile a single test:
./target/release/compile_dump \
  tests/gold_standard/ipc/simple_send.vuma /tmp/out.bin aarch64 --opt-level=O3

# Run natively (x86_64 host):
chmod +x /tmp/out.bin && /tmp/out.bin

# Run under QEMU:
~/.local/bin/qemu-aarch64-static /tmp/out.bin

# Run wasm32:
./target/release/compile_dump \
  tests/gold_standard/ipc/simple_send.vuma /tmp/out.wasm wasm32 --opt-level=O3
python3 scripts/wasm32_runner.py /tmp/out.wasm
```

### Test exit codes

Each `.vuma` test file has an `// Expected exit code: N` header. The
test passes if the program exits with code N. See
[`testing.md`](testing.md) for the full testing overview.

## Lean proofs (formal specification, standalone)

The Lean 4 specification of the PMT memory model lives in `proof/`
and builds with [Lake](https://github.com/leanprover/lake) via
[elan](https://github.com/leanprover/elan). The
`proof/lean-toolchain` file pins Lean to `leanprover/lean4:v4.21.0`.

> **Note.** The Lean proofs are a **standalone formal specification**.
> The Lean↔Rust FFI bridge that used to link verified Lean checkers
> into the runtime has been **removed**. Compile-time verification
> now goes through Z3 and the hand-written Rust verifiers in
> `vuma-ive` / `vuma-codegen`. Building the Lean proofs is optional
> and does **not** affect the compiler binary. See
> [`caveats.md`](caveats.md) §3 for the full story.

### Prerequisites (optional — only if you want to build the Lean spec)

```bash
curl -sSf https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh | sh -s -- -y
source $HOME/.elan/env
lean --version    # Lean (version 4.21.0, ...)
lake --version    # Lake (version 4.21.0, ...)
```

### Building

```bash
# Via Make (recommended):
make proof         # = cd proof && lake build
make proof-check   # = ./scripts/check-lean.sh
make proof-test    # = cd proof && lake exe test
make proof-clean   # = rm -rf proof/.lake proof/build

# Or directly via Lake:
cd proof && lake build && lake exe test
```

### Sorry check (`scripts/check-lean.sh`)

The check script runs `lake build` from `proof/` and greps the
combined stdout+stderr for the literal token `sorry`. It has two
modes:

- **Default:** fails on any `sorry` (exit 1) or build failure. Exits 0
  if the build is clean.
- **`PROOF_CHECK_STRICT=1`:** same as default but additionally fails
  on any `unused variable` warning. Use in strict CI gates.

```bash
./scripts/check-lean.sh                       # default
PROOF_CHECK_STRICT=1 ./scripts/check-lean.sh  # strict
```

### `pmt-runtime-check` Cargo feature

The `pmt-runtime-check` Cargo feature is **retained as a no-op at the
IVE layer** (the Lean FFI bridge is gone) but still has a real effect
in `vuma-codegen`: it activates the independent pure-Rust `pmt_check`
module (a parity-tested hand-translation of the Lean definitions in
`proof/PMT/Extraction.lean`). It does **not** depend on any Lean
linkage — the build never invokes `lake` or `cc` to compile Lean.

```bash
# Build with the pure-Rust verified PMT checkers compiled in:
cargo build --features pmt-runtime-check

# Parity test against the Lean definitions:
cargo test --features pmt-runtime-check --test pmt_parity_test

# Feature-flag wiring test:
cargo test --features pmt-runtime-check --test pmt_feature_flag_test
```

## Useful binaries

| Binary | Purpose |
|--------|---------|
| `vuma` | Main compiler CLI. |
| `compile_dump` | Compile a `.vuma` file and dump IR/ELF. Used by the test suite. |
| `dump_ir` | Dump IR for a `.vuma` file. |
| `scg_dump` | Dump SCG for a `.vuma` file. |

## Development workflow

### Adding a test

1. Create `tests/gold_standard/<category>/<name>.vuma`.
2. Add an `// Expected exit code: N` header.
3. Compile and run on a representative subset of backends:

   ```bash
   for b in aarch64 x86_64 hppa wasm32; do
     ./target/release/compile_dump \
       tests/gold_standard/<category>/<name>.vuma /tmp/t.bin $b --opt-level=O3
     chmod +x /tmp/t.bin
     # run via native, qemu, or wasmtime as appropriate
   done
   ```

4. Once it passes on the subset, run the full 19-backend matrix:

   ```bash
   bash scripts/vuma_test_matrix_19backends.sh <name>
   ```

### Debugging a failing backend

```bash
# Strace under QEMU:
~/.local/bin/qemu-<arch>-static -strace /tmp/out.bin

# Instruction trace under QEMU:
~/.local/bin/qemu-<arch>-static -d in_asm /tmp/out.bin 2>/tmp/trace.log
```

Then check the backend's known quirks in
[`backends.md`](backends.md) / [`fp_backends.md`](fp_backends.md)
and [`caveats.md`](caveats.md).

## Project structure

```
vuma/
├── src/
│   ├── parser/      # Lexer + parser + AST
│   ├── scg/         # Structured Call Graph
│   ├── ive/         # Intermediate Verification Engine (Z3-backed)
│   ├── codegen/     # IR + optimizer + 19 backends
│   ├── cor/         # Continuous Optimisation Runtime
│   ├── bd/          # Behavioural Descriptors
│   ├── proof/       # Rust-side proof artifacts (Lean spec lives in /proof)
│   ├── vuma/        # CLI
│   ├── pipeline.rs  # Compilation pipeline
│   └── main.rs      # Entry point
├── tests/           # Gold-standard test programs
├── scripts/         # Test runner, wasm32 runner, 19-backend matrix
├── proof/           # Lean 4 formal spec of the PMT memory model (standalone)
├── womb/            # Standard library
├── examples/        # Example programs
└── docs/            # This documentation
```

See [`architecture.md`](architecture.md) and
[`kernel-architecture.md`](kernel-architecture.md) for details.
