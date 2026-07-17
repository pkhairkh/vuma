# Building VUMA 2.0

VUMA 2.0 is **PMT-only** (Programs as Memory Transformations). Every test in
the gold-standard suite is written in PMT syntax (`layout` / `State` /
`state_new`); the legacy pointer dialect is no longer accepted by the test
runner. This document is the complete build reference.

For day-to-day contributor workflow see [`contributing.md`](contributing.md).
For the PMT language itself see [`language-reference.md`](language-reference.md).

---

## 1. Prerequisites

### Rust toolchain (pinned nightly)

VUMA 2.0 requires Rust **`nightly-2026-03-01`**, pinned via
[`rust-toolchain.toml`](../rust-toolchain.toml):

```toml
[toolchain]
channel = "nightly-2026-03-01"
components = ["rustfmt", "clippy", "rust-src"]
targets   = ["aarch64-unknown-linux-gnu", "aarch64-unknown-none"]
profile   = "default"
```

The nightly channel is required for `naked_asm`, the bare-metal target, and
the const-generic / trait-system features used by the codegen crate. The
toolchain file is checked in, so `cargo` auto-installs it on first use:

```bash
rustup toolchain install nightly-2026-03-01 \
  --component rustfmt,clippy,rust-src \
  --target aarch64-unknown-linux-gnu \
  --target aarch64-unknown-none
```

### QEMU user-mode (cross-backend execution)

Cross-compiled VUMA binaries are executed under QEMU user-mode emulators. The
test runner expects the following binaries on `PATH`:

| Backend           | QEMU binary         | Backend        | QEMU binary         |
|-------------------|---------------------|----------------|---------------------|
| `aarch64`         | `qemu-aarch64`      | `ppc64le`      | `qemu-ppc64le`      |
| `aarch64_be`      | `qemu-aarch64_be`   | `mips64`       | `qemu-mips64`       |
| `x86_64`          | `qemu-x86_64`       | `mips64el`     | `qemu-mips64el`     |
| `x86_32`          | `qemu-i386`         | `s390x`        | `qemu-s390x`        |
| `riscv64`         | `qemu-riscv64`      | `alpha`        | `qemu-alpha`        |
| `riscv32`         | `qemu-riscv32`      | `m68k`         | `qemu-m68k`         |
| `arm32`           | `qemu-arm`          | `sparc64`      | `qemu-sparc64`      |
| `armeb`           | `qemu-armeb`        | `hppa`         | `qemu-hppa`         |
| `ppc64`           | `qemu-ppc64`        | `loongarch64`  | `qemu-loongarch64`  |

On Debian/Ubuntu: `sudo apt-get install qemu-user qemu-user-static`.

The runner also registers `binfmt_misc` entries for every architecture above
so fork+exec of a cross-compiled ELF goes through the right interpreter. The
host's native architecture is intentionally skipped to avoid infinite QEMU
recursion.

### wasmtime (for the `wasm32` backend)

The `wasm32` backend does not use QEMU. Its emitted `.wasm` modules are
executed by [`wasmtime`](https://github.com/bytecodealliance/wasmtime), driven
by [`scripts/wasm32_runner.py`](../scripts/wasm32_runner.py). Install both the
CLI binary and the Python package:

```bash
curl -fSL -o /tmp/wasmtime.tar.xz \
  https://github.com/bytecodealliance/wasmtime/releases/latest/download/wasmtime-<ver>-<arch>-linux.tar.xz
tar xf /tmp/wasmtime.tar.xz -C /tmp/ && cp /tmp/wasmtime-*/wasmtime ~/.local/bin/
pip3 install --user wasmtime
```

If `wasmtime` is missing the runner skips the `wasm32` backend rather than
failing the suite.

### Optional: `just` / `make`

The repo ships both a `justfile` and a [`Makefile`](../Makefile) with
identical recipes. Either works; `just` is a single static binary
(`cargo install just`).

### No external crates

The workspace depends on **no external crates** — only `std` and the internal
`vuma-*` path crates. Builds require no network access to crates.io.

---

## 2. Building

The primary build artifact for testing is the `compile_dump` driver binary,
which compiles a `.vuma` source file to a given backend, emits the binary, and
runs it under QEMU / wasmtime.

```bash
cargo build --profile release-fast --bin compile_dump
```

The binary lands in `target/release-fast/compile_dump` (NOT
`target/release/`). `release-fast` is the profile the test runner uses by
default — see [§6 Build Profiles](#6-build-profiles).

Other useful invocations:

```bash
cargo build --workspace                              # dev (debug, opt-0)
cargo build --workspace --profile release-fast       # opt-3, LTO off
cargo build --workspace --release                    # opt-3, fat LTO
cargo check -p vuma -p vuma-scg -p vuma-ive -p vuma-bd   # core-only type-check
```

| Binary              | Purpose                                                    |
|---------------------|------------------------------------------------------------|
| `compile_dump`      | Compile + run a single `.vuma` file on a given backend     |
| `dump_ir`           | Dump the lowered IR / SCG for a `.vuma` file               |
| `differential_test` | Cross-backend differential run (compares exit codes)       |
| `opt_level_test`    | O0-vs-O3 soundness check                                   |
| `fuzz_driver`       | Property-based fuzzing of parser + codegen                 |

---

## 3. Project Structure

VUMA is a Cargo workspace of 9 internal library crates plus the root binary
crate. Each crate lives under `src/<name>/` and is named `vuma-<name>`:

| Crate (`src/...`)  | Package          | One-line description                                            |
|--------------------|------------------|-----------------------------------------------------------------|
| `src/parser/`      | `vuma-parser`    | Frontend: lexer, parser, AST, AST→SCG lowering, error recovery  |
| `src/scg/`         | `vuma-scg`       | Semantic Computation Graph — the formal graph IR                |
| `src/bd/`          | `vuma-bd`        | Behavioral Descriptors — RepD, CapD, RelD lattices + inference  |
| `src/ive/`         | `vuma-ive`       | Inference & Verification Engine — descriptor inference + checks  |
| `src/codegen/`     | `vuma-codegen`   | 19-architecture backend, register allocator, scheduler, optimizer |
| `src/proof/`       | `vuma-proof`     | Formal proof system — checker, tactics, counterexamples         |
| `src/cor/`         | `vuma-cor`       | Continuous Optimization Runtime — JIT, profiling, speculation   |
| `src/vuma/`        | `vuma-core`      | Memory model, MSG construction, invariant checking, security    |
| `src/package/`     | `vuma-package`   | Package manager — manifest parser, dependency resolver, registry |
| (root)             | `vuma`           | CLI binary (`src/main.rs`) + `compile_dump` / `dump_ir` drivers |

All path dependencies are declared in the root [`Cargo.toml`](../Cargo.toml)
under `[workspace.dependencies]`. There are no external crates.

---

## 4. Cross-Backend Testing

VUMA 2.0 supports **19 backends** (see the `BackendKind` enum in
[`src/codegen/src/backend.rs`](../src/codegen/src/backend.rs)):

```
AArch64, AArch64Be, RiscV64, RiscV32, Wasm32, LoongArch64,
X86_64, X86_32, Arm32, ArmEb, Mips64, Mips64Be, PowerPC64,
PowerPC64LE, Sparc64, S390X, M68k, Alpha, Hppa
```

18 of those run under a QEMU user-mode emulator (see the table in
[§1 Prerequisites](#1-prerequisites)); the `wasm32` backend runs under
`wasmtime`.

### The test runner

[`scripts/pi5_test_suite.sh`](../scripts/pi5_test_suite.sh) is the canonical
end-to-end runner. It:

1. Verifies / installs QEMU user-mode and `wasmtime`.
2. Registers `binfmt_misc` entries for every cross-architecture (skipping the
   host's native arch to avoid QEMU recursion).
3. Builds `compile_dump` with the `release-fast` profile.
4. Walks `tests/gold_standard/`, compiles every `.vuma` file on every
   backend, executes it (QEMU or wasmtime), and compares the exit code against
   the `// Expected exit code: N` header in the file.

Typical invocation:

```bash
scripts/pi5_test_suite.sh --workers 8 --fresh --verify
```

| Flag              | Effect                                                          |
|-------------------|-----------------------------------------------------------------|
| `--workers N`     | Parallel compile+run workers (default 4)                        |
| `--fresh`         | Force a from-scratch `cargo build` (no incremental reuse)       |
| `--verify`        | Run the IVE verifier on each program before executing it        |
| `--skip-build`    | Reuse an existing `target/release-fast/compile_dump`            |
| `--backends LIST` | Comma-separated subset of backends to run                       |
| `--release`       | Use the slow `release` profile (fat LTO) instead of `release-fast` |
| `--no-push`       | Do not push results to the results git remote                   |

For cross-backend *agreement* checks (rather than per-file pass/fail), use
[`scripts/cross_backend_test.sh`](../scripts/cross_backend_test.sh), which
compiles every `.vuma` in a directory on all 7 native QEMU backends and
reports any exit-code disagreement.

---

## 5. Test Categories

All gold-standard tests live under
[`tests/gold_standard/`](../tests/gold_standard/) as `.vuma` source files
grouped by category. **Every test is PMT-only** — there are no pointer-dialect
tests in the suite.

### Feature categories (16 directories)

| Directory            | Focus                                                        |
|----------------------|--------------------------------------------------------------|
| `arithmetic/`        | Integer arithmetic (add/sub/mul/div, overflow, mixed widths)|
| `atomics/`           | Atomic read-modify-write patterns                           |
| `bitwise/`           | AND/OR/XOR/shifts, bit extraction                           |
| `complex_stores/`    | Multi-cell stores, overwrites, scatter/gather               |
| `concurrency/`       | Multi-state interaction patterns                            |
| `control_flow/`      | Branches, loops, early returns                              |
| `crypto_patterns/`   | Reduced-step crypto primitives (AES, SHA, ChaCha rounds)    |
| `edge_cases/`        | Boundary values (0, MAX, MIN), empty functions, alloc edges |
| `functions/`         | Single-function call/return semantics                       |
| `linked_structures/` | State-based linked lists, trees                              |
| `memory/`            | Buffer allocation, reuse, lifetime                           |
| `multi_function/`    | Multi-function programs, cross-function data flow           |
| `nested_loops/`      | Loop nesting, induction-variable correctness                |
| `pointers/`          | PMT-translated pointer programs (state-as-pointer)          |
| `structs/`           | Multi-field layouts, field access patterns                  |
| `u32_arith/`         | 32-bit unsigned arithmetic stress                           |

### PMT wave directories (8 directories)

| Directory               | Focus                                                    |
|-------------------------|----------------------------------------------------------|
| `pmt_wave1/`            | Basic `layout` / `state_new` / single-field access      |
| `pmt_wave2/`            | Multiple states, multi-field layouts, u32/i64 fields    |
| `pmt_wave3_negative/`   | Negative tests — programs the type checker must reject   |
| `pmt_wave5/`            | State lifetimes, buffer reuse                           |
| `pmt_wave7/`            | Field swaps, copies, linked states, accumulators        |
| `pmt_wave8/`            | Buffer sizing (single, large, reuse)                    |
| `pmt_wave9/`            | Advanced PMT patterns                                    |
| `pmt_wave10/`           | Final PMT conformance wave                               |

Every `.vuma` file begins with a header of the form:

```
// <name> — <one-line description>
// Expected exit code: <N>
//
// <longer description / what this tests>
```

The runner reads the `Expected exit code:` line and compares it against the
process exit status after QEMU/wasmtime execution.

---

## 6. Build Profiles

Three profiles are defined across the root [`Cargo.toml`](../Cargo.toml) and
[`.cargo/config.toml`](../.cargo/config.toml).

### `dev` (default — debug, opt-0)

```toml
[profile.dev]
opt-level = 0
debug     = 2
```

Incremental compilation is enabled globally. Use for day-to-day development
and debugging.

```bash
cargo build --workspace              # binaries land in target/debug/
```

### `release-fast` (opt-3, LTO off — the test profile)

```toml
[profile.release-fast]
inherits       = "release"
opt-level      = 3
lto            = false
codegen-units  = 16
debug          = false
strip          = true
```

Keeps `opt-level = 3` (so QEMU-emulated executions stay fast) but disables LTO
and bumps `codegen-units` to 16. A from-scratch build that takes 10+ minutes
under `release` drops to 1–2 minutes under `release-fast`, at a 5–10% runtime
cost. **This is the profile used by `scripts/pi5_test_suite.sh` and CI.**

```bash
cargo build --profile release-fast --bin compile_dump
# → target/release-fast/compile_dump   (NOT target/release/)
```

### `release` (opt-3, fat LTO — maximum optimization)

```toml
[profile.release]
opt-level     = 3
lto           = "fat"
codegen-units = 1
strip         = true
panic         = "abort"
```

Slow to link (fat LTO, single codegen unit) but produces the fastest runtime
code. Use for release artifacts; opt in via `--release` on the test runner.

### Profile summary

| Profile        | `opt-level` | `lto` | `codegen-units` | `debug` | `strip` | `panic`  | Output dir              |
|----------------|-------------|-------|-----------------|---------|---------|----------|-------------------------|
| `dev`          | 0           | —     | —               | 2       | —       | unwind   | `target/debug/`         |
| `release-fast` | 3           | false | 16              | false   | true    | abort    | `target/release-fast/`  |
| `release`      | 3           | fat   | 1               | —       | true    | abort    | `target/release/`       |

---

## 7. Constrained-Memory Workaround

Some build environments (CI sandboxes, small VMs, Raspberry Pi 3/4) cap
available RAM at **4 GiB** (cgroup `memory.max=4294967296`). Under that cap
the `release-fast` profile — `opt-level = 3` with 16 parallel codegen units —
**OOMs** during the link/codegen step, even with `--workers 1`.

The workaround is to drop to the `dev` profile with single-threaded codegen
and symbol stripping. Host-side `opt-level` does **not** affect VUMA
correctness (VUMA's own optimization passes are the algorithms that matter);
only build time and runtime speed change.

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
cargo build --profile dev --bin compile_dump        # constrained build
scripts/pi5_test_suite.sh --skip-build --profile dev --workers 1 --verify
```

---

## 8. Troubleshooting

- **`linker 'aarch64-linux-gnu-gcc' not found`** — install the cross linker
  (`sudo apt-get install gcc-aarch64-linux-gnu` on Debian/Ubuntu), or build
  for the host architecture only (drop `--target`).
- **Build OOMs / killed by kernel** — you are hitting the 4 GiB (or similar)
  cap. Switch to the [constrained-memory build](#7-constrained-memory-workaround).
  If swap is available, adding 4–8 GiB usually lets `release-fast` link.
- **`qemu-aarch64: command not found`** — the runner looks for QEMU in
  `/tmp/my-project/bin`, `/tmp/my-project/qemu-user-extract/usr/bin`, then
  `PATH`. Install `qemu-user` or extract the bundled tarball into one of those
  paths.
- **`wasm32` backend silently skipped** — `wasmtime` (CLI or Python package)
  is missing. See [§1 Prerequisites](#1-prerequisites).
- **`binfmt_misc` registration fails** — needs root or a pre-configured
  `binfmt_misc` mount. Run `sudo ./scripts/pi5_test_suite.sh` once; the
  registrations persist.
- **Gold-standard test exits 139 / 134** — SIGSEGV / SIGABRT. The emitted
  code for that backend is unsound for that program. Bisect by running the
  same program under `x86_64` (native) and the failing backend, and compare
  emitted code with `cargo run --bin dump_ir`.
- **Clean everything** — `cargo clean` (just `target/`) or
  `rm -rf target/doc`.

---

For anything not covered here, check the [`README`](../README.md) and
[`contributing.md`](contributing.md), then open an
[issue](https://github.com/pkhairkh/vuma/issues).
