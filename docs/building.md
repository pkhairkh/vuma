# Building VUMA 2.0

VUMA 2.0 is **PMT-only** (Programs as Memory Transformations). Every test in
the gold-standard suite is written in PMT syntax (`layout` / `State<T>` /
`state_new`); the legacy pointer dialect is no longer accepted by the test
runner. The same compiler builds the **VWK** (Vuma Womb Kernel) — 75 PMT-pure
`.vuma` files across 13 waves (K0–K12) under `womb/kernel/`. This document is
the complete build reference for both the compiler and the kernel.

For day-to-day contributor workflow see [`contributing.md`](contributing.md).
For the PMT language itself see [`language-reference.md`](language-reference.md).
For the kernel's architecture see [`kernel-architecture.md`](kernel-architecture.md).
For a project overview see the root [`README.md`](../README.md).

---

## Table of Contents

1. [Prerequisites](#1-prerequisites)
2. [Building the Compiler](#2-building-the-compiler)
3. [Project Structure](#3-project-structure)
4. [Cross-Backend Testing](#4-cross-backend-testing)
5. [Test Categories](#5-test-categories)
6. [Build Profiles](#6-build-profiles)
7. [QEMU Installation](#7-qemu-installation)
8. [Kernel Testing](#8-kernel-testing)
9. [Troubleshooting](#9-troubleshooting)

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
test runner expects binaries named `qemu-<arch>` on `PATH`. Of the 19 VUMA
backends, 7 are executable in the standard sweep (the rest are compile-only):

| Backend           | QEMU binary         | Backend        | QEMU binary         |
|-------------------|---------------------|----------------|---------------------|
| `aarch64`         | `qemu-aarch64`      | `ppc64le`      | `qemu-ppc64le`      |
| `riscv64`         | `qemu-riscv64`      | `mips64`       | `qemu-mips64`       |
| `arm32`           | `qemu-arm`          | `s390x`        | `qemu-s390x`        |
| `x86_64`          | (native — no QEMU)  | `loongarch64`  | `qemu-loongarch64`  |

The 12 compile-only backends (`aarch64_be`, `riscv32`, `armeb`, `mips64be`,
`ppc64`, `sparc64`, `alpha`, `hppa`, `m68k`, `x86_32`, `wasm32`, plus any of
the above if their QEMU binary is absent) emit valid ELF machine code and pass
IVE verification, but the standard parity sweep does not execute them.

In this build environment the QEMU binaries are statically linked and live at
`/usr/local/bin/qemu-*`. The runner also registers `binfmt_misc` entries for
every architecture above so fork+exec of a cross-compiled ELF goes through the
right interpreter. The host's native architecture is intentionally skipped to
avoid infinite QEMU recursion. See [§7 QEMU Installation](#7-qemu-installation)
for non-root and sandboxed-CI install paths.

### wasmtime (for the `wasm32` backend)

The `wasm32` backend does not use QEMU. Its emitted `.wasm` modules are
executed by [`wasmtime`](https://github.com/bytecodealliance/wasmtime), driven
by [`scripts/wasm32_runner.py`](../scripts/wasm32_runner.py) which provides the
host functions (pipe/fork/execve/dup2/waitpid/strcmp) that WASI does not
support. Install both the CLI binary and the Python package:

```bash
curl -fSL -o /tmp/wasmtime.tar.xz \
  https://github.com/bytecodealliance/wasmtime/releases/latest/download/wasmtime-<ver>-<arch>-linux.tar.xz
tar xf /tmp/wasmtime.tar.xz -C /tmp/ && cp /tmp/wasmtime-*/wasmtime ~/.local/bin/
pip3 install --user wasmtime
```

If `wasmtime` is missing the runner skips the `wasm32` backend rather than
failing the suite.

### Optional: `just` / `make`

The repo ships both a [`justfile`](../justfile) and a [`Makefile`](../Makefile)
with identical recipes (`build`, `check`, `test`, `fmt`, `clippy`, `lint`,
`doc`, `clean`, …). Either works; `just` is a single static binary
(`cargo install just`). Both define `setup` / `toolchain` targets that install
the pinned nightly + components.

### No external crates

The workspace depends on **no external crates** — only `std` and the internal
`vuma-*` path crates declared in [`Cargo.toml`](../Cargo.toml) under
`[workspace.dependencies]`. There is no `serde`, no `clap`, no `libc`, no
`rayon`. Builds require no network access to crates.io; `Cargo.lock` contains
only `vuma-*` packages.

---

## 2. Building the Compiler

The primary build artifact for testing is the `compile_dump` driver binary,
which compiles a `.vuma` source file to a given backend, emits the binary, and
runs it under QEMU / wasmtime. It is the test-harness entry point for both the
gold-standard suite and the kernel smoke / parity harnesses.

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

### Driver binaries

The root `vuma` crate provides the CLI binary (`src/main.rs`) plus the
driver binaries in [`src/bin/`](../src/bin/):

| Binary              | Purpose                                                    |
|---------------------|------------------------------------------------------------|
| `compile_dump`      | Compile + run a single `.vuma` file on a given backend     |
| `dump_ir`           | Dump the lowered IR / SCG for a `.vuma` file               |
| `dump_codegen_scg`  | Dump the codegen-side SCG after pipeline transforms        |
| `scg_dump`          | Dump the parser-side SCG before lowering                   |
| `parse_test`        | Parse-only smoke driver (no codegen)                       |

The CLI invocation pattern is:

```bash
./target/release-fast/compile_dump <input.vuma> <output.bin> <backend> [--verify]
```

`--verify` runs the three IVE state verifiers (`StateRead`, `StateWrite`,
`StateTransform`) and prints `IVE: Pass passed=N failed=0 total=N` on success.
All kernel commits require `--verify` to pass on `womb/kernel/kernel.vuma`.

### Constrained-memory workaround

Some build environments (CI sandboxes, small VMs, Raspberry Pi 3/4) cap
available RAM at **4 GiB** (cgroup `memory.max=4294967296`). Under that cap
the `release-fast` profile — `opt-level = 3` with 16 parallel codegen units —
**OOMs** during the link/codegen step, even with `--workers 1`. Host-side
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
cargo build --profile dev --bin compile_dump        # constrained build
scripts/vuma_test_suite.sh --skip-build --profile dev --workers 1 --verify
```

---

## 3. Project Structure

VUMA is a Cargo workspace of **10 internal crates** (~329K LOC of Rust) plus
a PMT standard library, a PMT kernel, a gold-standard test suite, and
supporting scripts and docs. Each crate lives under `src/<name>/` and is
named `vuma-<name>`:

| Crate (`src/...`)  | Package          | One-line description                                            | LOC (Rust) |
|--------------------|------------------|-----------------------------------------------------------------|------------|
| `src/parser/`      | `vuma-parser`    | Frontend: lexer, parser, AST, AST→SCG lowering, error recovery  | ~21K       |
| `src/scg/`         | `vuma-scg`       | Semantic Computation Graph — the formal graph IR                | ~22K       |
| `src/bd/`          | `vuma-bd`        | Behavioral Descriptors — RepD, CapD, RelD lattices + inference  | ~16K       |
| `src/ive/`         | `vuma-ive`       | Inference & Verification Engine — the 3 state verifiers + FFI    | ~19K       |
| `src/codegen/`     | `vuma-codegen`   | 19-architecture backend, register allocator, scheduler, optimizer | ~156K    |
| `src/proof/`       | `vuma-proof`     | Formal proof system — checker, tactics, counterexamples         | ~11K       |
| `src/cor/`         | `vuma-cor`       | Continuous Optimization Runtime — JIT, profiling, speculation   | ~11K       |
| `src/vuma/`        | `vuma-core`      | Memory model, MSG construction, invariant checking, security    | ~15K       |
| `src/package/`     | `vuma-package`   | Package manager — manifest parser, dependency resolver, registry| ~2K        |
| `src/tests/`       | `vuma-tests`     | Integration test framework                                      | ~31K       |
| (root)             | `vuma`           | CLI binary (`src/main.rs`) + `compile_dump` / `dump_ir` drivers | —          |

All path dependencies are declared in the root [`Cargo.toml`](../Cargo.toml)
under `[workspace.dependencies]`. There are no external crates. The full
workspace tree is documented in [`src/README.md`](../src/README.md); the test
crate is documented in [`tests/README.md`](../tests/README.md).

The PMT standard library lives under [`womb/`](../womb/) (183 `.vuma` files
total). Major subtrees:

| Path                  | Files | Description                                             |
|-----------------------|-------|---------------------------------------------------------|
| `womb/kernel/`        | 75    | VWK kernel — see [`womb/kernel/README.md`](../womb/kernel/README.md) |
| `womb/crypto/`        | 45    | Crypto primitives — see [`womb/crypto/README.md`](../womb/crypto/README.md) |
| `womb/net/`           | 5     | TLS 1.2/1.3, SSH, QUIC, TCP — see [`womb/net/README.md`](../womb/net/README.md) |
| `womb/lib/`           | 28    | Application library: stdio, string, json, http, dns, deflate, … |
| `womb/collections/`   | 4     | vec, hashmap, btree_map, enum_map                       |
| `womb/alloc/`         | 1     | arena state model (`arena.vuma`)                        |
| `womb/{codec,encoding,env,fs,graph,ieee,io,lang,string,containers}/` | rest | supporting libraries |

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
`wasmtime`. 7 of the 19 are executable in the standard sweep (native x86_64 +
6 QEMU emulators + `wasm32` under wasmtime). The remaining 12 are compile-only
— they emit valid ELF machine code and pass IVE verification but are not
executed by the default sweep.

### The gold-standard runner

[`scripts/vuma_test_suite.sh`](../scripts/vuma_test_suite.sh) is the canonical
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
scripts/vuma_test_suite.sh --workers 8 --fresh --verify
```

| Flag              | Effect                                                          |
|-------------------|-----------------------------------------------------------------|
| `--workers N`     | Parallel compile+run workers (default 4)                        |
| `--fresh`         | Force a from-scratch `cargo build` (no incremental reuse)       |
| `--verify`        | Run the IVE verifier on each program before executing it        |
| `--skip-build`    | Reuse an existing `target/release-fast/compile_dump`            |
| `--backends LIST` | Comma-separated subset of backends to run                       |
| `--release`       | Use the slow `release` profile (fat LTO) instead of `release-fast` |
| `--profile dev`   | Point at a constrained-memory build (see [§2](#2-building-the-compiler)) |
| `--no-push`       | Do not push results to the results git remote                   |

For cross-backend *agreement* checks (rather than per-file pass/fail), use
[`scripts/cross_backend_test.sh`](../scripts/cross_backend_test.sh), which
compiles every `.vuma` in a directory on all 7 native QEMU backends and
reports any exit-code disagreement.

### The kernel parity sweep

[`scripts/kernel_parity.sh`](../scripts/kernel_parity.sh) compiles + runs
`womb/kernel/kernel.vuma` and a curated subset of gold-standard tests across
**all 19 backends** using QEMU user-mode for non-x86_64 arches. It also
compile-verifies (IVE only, no execution) 19 kernel modules covering mm,
proc, vfs, ipc, sync, net, crypto, panic, and power. Exits 0 only if every
backend passes. See [§8 Kernel Testing](#8-kernel-testing) for the full
breakdown.

### Other runner scripts

| Script | Scope |
|--------|-------|
| [`scripts/run_all_gold.sh`](../scripts/run_all_gold.sh) | Run all gold-standard tests on x86_64 (fast loop) |
| [`scripts/run_real_kat.sh`](../scripts/run_real_kat.sh) | Run real-KAT suite (cross-arch known-answer) |
| [`scripts/cross_backend_test.sh`](../scripts/cross_backend_test.sh) | Cross-backend agreement sweep |
| [`scripts/run_backend_resilient.py`](../scripts/run_backend_resilient.py) | Resilient runner that retries failed backends |
| [`scripts/supervisor.py`](../scripts/supervisor.py) | Long-running test supervisor |
| [`scripts/wasm32_runner.py`](../scripts/wasm32_runner.py) | Wasm host function provider for `wasm32` backend |
| [`scripts/ci_run_tests.sh`](../scripts/ci_run_tests.sh) | CI entry point |
| [`scripts/run_fuzz.sh`](../scripts/run_fuzz.sh) | Fuzz driver harness |

See [`tests/README.md`](../tests/README.md) for the test-suite layout and
[`scripts/`](../scripts/) for the full list of 24 scripts.

---

## 5. Test Categories

All gold-standard tests live under
[`tests/gold_standard/`](../tests/gold_standard/) as `.vuma` source files
grouped by category. See [`tests/gold_standard/manifest.json`](../tests/gold_standard/manifest.json)
for the current program count and per-category breakdown (the suite was
curated down from ~5,851 files to its current size; see
[`tests/gold_standard/README.md`](../tests/gold_standard/README.md) for the
history). **Every test is PMT-only** — there are no pointer-dialect
tests in the suite.

### Feature categories (16 directories)

| Directory            | Count | Focus                                                        |
|----------------------|-------|--------------------------------------------------------------|
| `arithmetic/`        | 72    | Integer arithmetic (add/sub/mul/div, overflow, mixed widths)|
| `atomics/`           | 35    | Atomic read-modify-write patterns                           |
| `bitwise/`           | 50    | AND/OR/XOR/shifts, bit extraction                           |
| `complex_stores/`    | 45    | Multi-cell stores, overwrites, scatter/gather               |
| `concurrency/`       | 35    | Multi-state interaction patterns                            |
| `control_flow/`      | 50    | Branches, loops, early returns                              |
| `crypto_patterns/`   | 36    | Reduced-step crypto primitives (AES, SHA, ChaCha rounds)    |
| `edge_cases/`        | 42    | Boundary values (0, MAX, MIN), empty functions, alloc edges |
| `functions/`         | 48    | Single-function call/return semantics                       |
| `linked_structures/` | 32    | State-based linked lists, trees                              |
| `memory/`            | 74    | Buffer allocation, reuse, lifetime                           |
| `multi_function/`    | 35    | Multi-function programs, cross-function data flow           |
| `nested_loops/`      | 19    | Loop nesting, induction-variable correctness                |
| `pointers/`          | 47    | PMT-translated pointer programs (state-as-pointer)          |
| `structs/`           | 46    | Multi-field layouts, field access patterns                  |
| `u32_arith/`         | 38    | 32-bit unsigned arithmetic stress                           |

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

### Arena + FFI + kernel wave directories

| Directory               | Focus                                                    |
|-------------------------|----------------------------------------------------------|
| `arena_wave0/`          | K0 arena-builtin tests (3 files)                         |
| `arena_wave1/`          | K0 arena-overflow regression (4 files)                   |
| `arena_wave2/`          | K0 arena-multiple + grow tests                           |
| `ffi_wave0/`–`ffi_wave4/` | FFI marshal waves: borrow modes, marshal scratch, foreign state, callbacks |
| `kernel_crypto/`        | SHA-256 KAT test for the kernel crypto subsystem         |

Every `.vuma` file begins with a header of the form:

```
// <name> — <one-line description>
// Expected exit code: <N>
//
// <longer description / what this tests>
```

The runner reads the `Expected exit code:` line and compares it against the
process exit status after QEMU/wasmtime execution. A `skip_on: wasm32, ppc64`
header marks tests that exercise architecturally-unavailable functionality
(e.g. `fork` on wasm32) — those tests are skipped on the listed backends
rather than failing.

### Kernel test categories

Beyond the gold-standard categories above, the kernel has three test layers:

1. **Per-module self-tests** — every `.vuma` file in `womb/kernel/` ends with
   a `fn main() -> i32` self-test that exercises the module's API surface. A
   non-zero exit code pinpoints the broken check by number.
2. **Boot smoke test** — `scripts/kernel_smoke.sh` compiles
   `womb/kernel/kernel.vuma` for x86_64 with `--verify`, runs it as a
   regular Linux process, greps stdout for `vuma kernel: hello`, and checks
   exit code 0. This is the minimum bar every commit must clear.
3. **19-backend parity sweep** — `scripts/kernel_parity.sh` compiles + runs
   the kernel and 10 gold-standard tests across all 19 backends, and
   compile-verifies 19 kernel modules on 4 backends.

See [§8 Kernel Testing](#8-kernel-testing) for usage.

### KAT tests for crypto

Known-answer tests for crypto algorithms live in two directories:
[`scripts/womb_kat_tests/`](../scripts/womb_kat_tests/) (86 test files for
the `womb/crypto/` library) and [`scripts/real_kat_tests/`](../scripts/real_kat_tests/)
(127 cross-architecture known-answer tests). Run the cross-arch suite with:

```bash
bash scripts/run_real_kat.sh       # real cross-arch KAT suite
```

The `womb_kat_tests/` directory holds `.vuma` test data consumed by
`scripts/womb_test_harness.sh`; the standalone `run_all_kat.sh` runner was
removed during the 2026-07 cleanup (its functionality is folded into the
womb smoke harness).

See [`womb/crypto/README.md`](../womb/crypto/README.md) for the algorithm
coverage matrix and [`tests/README.md`](../tests/README.md) for the test
harness layout.

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
and debugging, and as the fallback when `release-fast` OOMs on
memory-constrained hosts (see [§2](#2-building-the-compiler)).

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
cost. **This is the profile used by `scripts/vuma_test_suite.sh`,
`scripts/kernel_smoke.sh`, `scripts/kernel_parity.sh`, and CI.**

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

### `just` / `make` recipes

Both [`justfile`](../justfile) and [`Makefile`](../Makefile) expose the same
recipe names:

| Recipe             | Effect                                                |
|--------------------|-------------------------------------------------------|
| `build`            | `cargo build --workspace` (dev profile)               |
| `release`          | `cargo build --workspace --release`                   |
| `check` / `check-fast` | `cargo check` for the workspace / core crates    |
| `test`             | `cargo test --workspace`                              |
| `test-crate crate=vuma-bd` | `cargo test -p <crate>`                       |
| `bench`            | `cargo bench --workspace`                             |
| `doc`              | `cargo doc --workspace --no-deps`                     |
| `fmt` / `fmt-check` | `cargo fmt --all` / `--check`                        |
| `clippy` / `clippy-fix` | `cargo clippy --workspace -- -D warnings`       |
| `lint`             | `fmt-check` + `clippy`                                |
| `setup`            | Install pinned nightly + components + targets         |
| `clean` / `clean-doc` | Wipe `target/` / `target/doc/`                     |
| `cross-aarch64`    | `cargo build --target aarch64-unknown-linux-gnu`      |
| `x86-64-run` / `riscv64-run` | Boot the bare-metal kernel in `qemu-system-*` |

---

## 7. QEMU Installation

QEMU user-mode is required for cross-backend testing. There are three install
paths depending on your privileges.

### Path 1 — system package manager (root required)

On Debian/Ubuntu:

```bash
sudo apt-get install qemu-user qemu-user-static
qemu-aarch64 --version
qemu-riscv64 --version
```

This installs all `qemu-<arch>` user-mode emulators system-wide. Recommended
for local dev boxes where you have root.

### Path 2 — stage QEMU binaries under `/tmp/qemu_bins/` (CI convention)

If you cannot install via the system package manager (no root, sandboxed CI,
containerized build), use the **staged-symlink approach** that the CI
workflow ([`.github/workflows/vuma-tests.yml`](../.github/workflows/vuma-tests.yml),
"Stage QEMU binaries under /tmp/qemu_bins/" step) and the runner scripts
(`scripts/run_all_gold.sh`, `scripts/kernel_parity.sh`,
`scripts/run_backend_resilient.py`, etc.) both look at first. The runner's
`qemu_bin_for()` / `_qemu_path()` helpers search in this order:

1. `/tmp/qemu_bins/qemu-<arch>` (CI-staged symlinks)
2. `/usr/bin/qemu-<arch>` (system install)
3. `command -v qemu-<arch>` / `shutil.which` (`PATH`)

So a no-root install that mimics CI looks like:

```bash
mkdir -p /tmp/qemu_bins
# Either extract a qemu-user-static .deb you downloaded:
ar x qemu-user-static_*.deb && tar xf data.tar.* --strip-components=4 \
    --wildcards './usr/bin/qemu-*'
# …then symlink each emulator into /tmp/qemu_bins/:
for q in qemu-aarch64 qemu-riscv64 qemu-arm qemu-ppc64le \
         qemu-mips64el qemu-loongarch64 qemu-s390x; do
    [ -x "$q" ] && ln -sf "$(pwd)/$q" "/tmp/qemu_bins/$q"
done
ls -l /tmp/qemu_bins/
```

Or, if you already have the binaries on `PATH` (e.g. from a previous
`apt-get install` on a different prefix), just mirror what the CI workflow
does and symlink them in:

```bash
mkdir -p /tmp/qemu_bins
for q in qemu-aarch64 qemu-riscv64 qemu-arm qemu-ppc64le \
         qemu-mips64el qemu-loongarch64; do
    src=$(command -v "$q") && ln -sf "$src" "/tmp/qemu_bins/$q"
done
```

In this build environment the QEMU binaries are already installed at
`/usr/local/bin/qemu-{aarch64,riscv64,arm,ppc64le,mips64,s390x,loongarch64}`,
which is on `PATH` by default. Verify with `ls /usr/local/bin/qemu-*`.

### Path 3 — pre-built static binaries shipped with the repo

Some CI environments ship a `qemu-user-static` tarball alongside the repo.
The runner auto-detects it by symlinking every `qemu-*` it finds into
`/tmp/qemu_bins/` (see Path 2) and prepending that directory to `PATH` for
the duration of the sweep.

### `binfmt_misc` registration

The runner also registers `binfmt_misc` entries for every cross-architecture
(skipping the host's native arch to avoid QEMU recursion) so fork+exec of a
cross-compiled ELF goes through the right interpreter. This needs root or a
pre-configured `binfmt_misc` mount. Run `sudo ./scripts/vuma_test_suite.sh`
once; the registrations persist across reboots on most Linux distros.

If `binfmt_misc` registration fails (no root, no mount), the runner falls
back to invoking QEMU explicitly (`qemu-<arch> ./test-binary`) — slightly
slower due to per-invocation fork+exec overhead but functionally identical.

### Verifying the install

```bash
# All 7 executable backends should resolve:
for q in qemu-aarch64 qemu-riscv64 qemu-arm qemu-ppc64le qemu-mips64 \
         qemu-s390x qemu-loongarch64; do
    command -v "$q" >/dev/null && echo "OK: $q" || echo "MISSING: $q"
done

# wasmtime (for wasm32):
command -v wasmtime >/dev/null && echo "OK: wasmtime" || echo "MISSING: wasmtime"
```

---

## 8. Kernel Testing

The VWK kernel under [`womb/kernel/`](../womb/kernel/) is a complete PMT-pure
kernel that compiles for all 19 backends. Three scripts drive kernel testing;
together they form the kernel-side equivalent of the gold-standard sweep.

### 8.1 `scripts/kernel_smoke.sh` — boot smoke test (single-arch)

[`scripts/kernel_smoke.sh`](../scripts/kernel_smoke.sh) is the minimum bar
every commit must clear. It:

1. Builds `compile_dump` (release-fast) if missing or older than `Cargo.toml`.
2. Compiles `womb/kernel/kernel.vuma` for `x86_64` with `--verify` (IVE on).
3. Verifies the IVE log contains `IVE: Pass` and not `IVE: Fail`.
4. Runs the resulting ELF as a regular Linux process.
5. Greps stdout for the exact banner line `vuma kernel: hello`.
6. Verifies the process exit code is 0.
7. Prints `PASS: kernel boots, prints banner, exits 0` on success.

```bash
bash scripts/kernel_smoke.sh
# Expected: "PASS: kernel boots, prints banner, exits 0"
```

This is hosted-mode only: the kernel's `host_*` abstraction in
[`womb/kernel/hosted/host.vuma`](../womb/kernel/hosted/host.vuma) is wired to
the host's libc syscalls (write/read/exit/mmap/...), so the kernel boots,
prints, and exits like any other userspace program. K11+ will add a
bare-metal QEMU system-mode harness; until then this hosted smoke test is
the gate.

### 8.2 `scripts/kernel_parity.sh` — 19-backend parity sweep

[`scripts/kernel_parity.sh`](../scripts/kernel_parity.sh) is the multi-backend
sweep. It runs in two phases:

**Phase 1 — Gold-standard test execution** across all 19 backends. The
script compiles + executes the following 10 gold-standard tests on every
backend, comparing the exit code against the expected value:

| Test | Expected exit |
|------|---------------|
| `tests/gold_standard/arena_wave1/arena_basic.vuma` | 42 |
| `tests/gold_standard/arena_wave1/arena_grow.vuma` | 0 |
| `tests/gold_standard/arena_wave1/arena_multiple.vuma` | 0 |
| `tests/gold_standard/arena_wave1/arena_overflow.vuma` | 1 |
| `tests/gold_standard/pmt_wave2/init_read.vuma` | 42 |
| `tests/gold_standard/arithmetic/arith_clamp.vuma` | 100 |
| `tests/gold_standard/control_flow/cf2_for_count.vuma` | 5 |
| `tests/gold_standard/functions/fn2_add_two.vuma` | 7 |
| `tests/gold_standard/bitwise/bit2_and_chain.vuma` | 3 |
| `tests/gold_standard/structs/enum_demo.vuma` | 141 |

That is **190 compile+execute checks** (10 tests × 19 backends). The 7
executable backends are run under QEMU; the 12 compile-only backends are
verified to compile + IVE-pass (marked `COMPILE_OK` in the table).

**Phase 2 — Kernel module compile-verify** on 4 representative backends
(`x86_64`, `aarch64`, `riscv64`, `wasm32`). The script compile-verifies (IVE
only, no execution) 19 kernel modules covering every major subsystem:

| Subsystem | Modules |
|-----------|---------|
| Boot | `kernel.vuma` |
| mm | `pmm.vuma`, `vmm.vuma` |
| proc | `task.vuma`, `scheduler.vuma` |
| vfs | `inode.vuma`, `dentry.vuma`, `file.vuma` |
| ipc | `pipe.vuma`, `signal.vuma`, `futex.vuma` |
| sync | `spinlock.vuma`, `mutex.vuma` |
| net | `socket.vuma` |
| crypto | `api.vuma`, `aes.vuma`, `sha.vuma` |
| panic | `panic.vuma` |
| power | `pm.vuma` |

That is **76 kernel module compiles** (19 modules × 4 backends). Combined
with Phase 1's 190 checks, the parity sweep runs **266 backend compilations**
per invocation.

```bash
bash scripts/kernel_parity.sh            # full sweep (~10 minutes)
bash scripts/kernel_parity.sh --quick    # arena_basic + kernel smoke only
```

The script exits 0 only if every backend passes. The summary block at the
end reports:

```
Gold-standard tests:   PASS=190  FAIL=0  COMPILE_FAIL=0
Kernel module compiles: PASS=76  FAIL=0
✓ ALL BACKENDS PASS
```

### 8.3 Per-module self-tests

Every `.vuma` file in `womb/kernel/` ends with a `fn main() -> i32`
self-test that exercises the module's API surface. The convention is:

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

So a future CI failure pinpoints the broken check by the exit code. Run a
module's self-test:

```bash
./target/release-fast/compile_dump womb/kernel/mm/pmm.vuma \
    /tmp/pmm.bin x86_64 --verify
/tmp/pmm.bin; echo "exit=$?"
# Expected: "IVE: Pass passed=1 failed=0 total=1" + exit=0
```

The full inventory of 75 kernel modules with their subsystems, K-wave, and
purpose is in [`womb/kernel/README.md`](../womb/kernel/README.md). The full
architecture (4-layer cake, FFI trampolines, arena memory model, sentinel
conventions, stub inventory) is in
[`kernel-architecture.md`](kernel-architecture.md).

### 8.4 Cross-compile + run a single kernel module under QEMU

```bash
# Build compile_dump once:
. "$HOME/.cargo/env"
cargo build --profile release-fast --bin compile_dump

# Compile the kernel for a non-x86_64 arch:
./target/release-fast/compile_dump womb/kernel/kernel.vuma \
    /tmp/kernel-aarch64.bin aarch64 --verify

# Run under QEMU user-mode:
qemu-aarch64 /tmp/kernel-aarch64.bin
# Expected output: "vuma kernel: hello"
```

---

## 9. Troubleshooting

### Build issues

- **`linker 'aarch64-linux-gnu-gcc' not found`** — install the cross linker
  (`sudo apt-get install gcc-aarch64-linux-gnu` on Debian/Ubuntu), or build
  for the host architecture only (drop `--target`).
- **Build OOMs / killed by kernel** — you are hitting the 4 GiB (or similar)
  cap. Switch to the [constrained-memory build](#2-building-the-compiler).
  If swap is available, adding 4–8 GiB usually lets `release-fast` link.
- **`rustup` fails to install nightly-2026-03-01** — check that the date is
  valid (the toolchain file pins it). If the upstream nightly was withdrawn,
  pin to the closest available date and update `rust-toolchain.toml`.
- **`cargo build` tries to reach crates.io** — every dependency should be a
  path dependency. If a `[dependencies]` entry slipped in for an external
  crate, remove it. `Cargo.lock` should contain only `vuma-*` packages.

### QEMU / wasmtime issues

- **`qemu-aarch64: command not found`** — the runner looks for QEMU in
  `/tmp/qemu_bins/qemu-<arch>` (CI-staged symlinks), then `/usr/bin/qemu-<arch>`,
  then `PATH`. Install `qemu-user` via `apt-get`, or symlink the binaries into
  `/tmp/qemu_bins/`. See [§7 QEMU Installation](#7-qemu-installation).
- **`wasm32` backend silently skipped** — `wasmtime` (CLI or Python package)
  is missing. See [§1 Prerequisites](#1-prerequisites).
- **`binfmt_misc` registration fails** — needs root or a pre-configured
  `binfmt_misc` mount. Run `sudo ./scripts/vuma_test_suite.sh` once; the
  registrations persist. If you can't get root, the runner falls back to
  explicit `qemu-<arch>` invocation.
- **QEMU emulator segfaults on a cross-compiled binary** — usually a
  codegen bug on the failing backend. Bisect by running the same program
  under `x86_64` (native) and the failing backend, and compare emitted code
  with `cargo run --bin dump_ir`.

### Test failures

- **Gold-standard test exits 139 / 134** — SIGSEGV / SIGABRT. The emitted
  code for that backend is unsound for that program. Bisect by running the
  same program under `x86_64` (native) and the failing backend, and compare
  emitted code with `cargo run --bin dump_ir`.
- **Cross-backend disagreement (same program, different exit codes)** — file
  an issue against the offending backend. The full gold-standard suite is
  expected to agree across all executable backends.
- **`--verify` reports `IVE: Fail`** — the failing line names the verifier
  that tripped (`StateReadVerifier`, `StateWriteVerifier`, or
  `StateTransformVerifier`) and the line number. The fix is almost always
  one of: (a) add `#[borrow]` to the offending extern, (b) flip a
  return-style helper to init-style, or (c) split the function so the extern
  call is in a different function than the post-call field access. See
  [`kernel-developer-guide.md`](kernel-developer-guide.md) §6 for the full
  debugging recipe.

### Kernel-specific issues

- **`kernel_smoke.sh` reports `FAIL: banner not found in output`** — the
  kernel either crashed before printing, or printed to stderr instead of
  stdout. Inspect `/tmp/kernel_smoke.out` for the actual output. Most often
  this means a kernel module's `console_putc` is broken (returning early,
  writing to the wrong fd, or the IVE verifier silently nulled a state
  field).
- **`kernel_smoke.sh` reports `FAIL: exit code N (expected 0)`** — the
  kernel's `kmain()` returned a non-zero value, which means one of the
  self-test checks inside the kernel failed. The exit code N is the test
  number; cross-reference `womb/kernel/kernel.vuma::kmain` to find the
  failing check.
- **`kernel_parity.sh` reports `COMPILE_FAIL` on a backend** — the kernel
  module failed to compile or IVE-verify on that backend. Re-run
  `compile_dump womb/kernel/<module>.vuma /tmp/out.bin <backend> --verify`
  manually to see the diagnostic. Most often this is a backend-specific
  codegen bug (e.g. a missing instruction selection pattern for a particular
  IR node).
- **`kernel_parity.sh` reports `FAIL(N/expected)` on a backend** — the
  compiled program ran but returned a different exit code than expected.
  Usually a backend codegen bug; bisect against `x86_64`.
- **`WARNING: unsupported FieldAccess (not state-typed)` from
  `flatten_expr`** — a function is returning `State<T>` and the caller is
  trying to access fields on the result. The codegen does not propagate
  `State`-typedness through return values (see
  [`kernel-architecture.md` §10.3](kernel-architecture.md)). Flip the helper
  to init-style (caller allocates the state, function populates it).
- **A kernel module's self-test passes alone but fails when compiled with
  the full kernel** — usually a layout drift between the module's local
  re-declaration and the canonical declaration elsewhere. The
  `LayoutRegistry` rejects conflicting field offsets at compile time; check
  the IVE log for a layout-conflict diagnostic.

### Clean everything

```bash
cargo clean        # wipe target/
rm -rf target/doc  # wipe just docs
rm -f /tmp/parity_*.bin /tmp/kernel_smoke.{bin,out} /tmp/pmm.bin
```

---

For anything not covered here, check the [`README`](../README.md),
[`contributing.md`](contributing.md), and
[`kernel-architecture.md`](kernel-architecture.md), then open an
[issue](https://github.com/pkhairkh/vuma/issues).
