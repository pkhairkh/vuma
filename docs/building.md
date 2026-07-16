# Building VUMA

This document is the complete build reference for VUMA: prerequisites, build
profiles, cross-compilation, the test suite, the self-hosting bootstrap
compiler, and common troubleshooting.

For day-to-day contributor workflow (setup, style, PR process), see
[`contributing.md`](contributing.md). For a project overview, see the
[`README`](../README.md).

---

## Table of Contents

1. [Prerequisites](#1-prerequisites)
2. [Build Profiles](#2-build-profiles)
3. [Cross-Compilation](#3-cross-compilation)
4. [Testing](#4-testing)
5. [The Bootstrap Compiler](#5-the-bootstrap-compiler)
6. [Troubleshooting](#6-troubleshooting)

---

## 1. Prerequisites

### Rust toolchain (pinned nightly)

VUMA requires Rust **nightly-2026-03-01**, pinned via
[`rust-toolchain.toml`](../rust-toolchain.toml):

```toml
[toolchain]
channel = "nightly-2026-03-01"
components = ["rustfmt", "clippy", "rust-src"]
targets = ["aarch64-unknown-linux-gnu", "aarch64-unknown-none"]
profile = "default"
```

The nightly channel is required for `naked_asm` and other inline-assembly
features, unstable features used by the bare-metal target, and advanced
const-generics / trait-system features. Because the toolchain file is checked
in, `cargo` auto-installs the pinned toolchain on first use. To install it
explicitly:

```bash
rustup toolchain install nightly-2026-03-01 \
  --component rustfmt,clippy,rust-src \
  --target aarch64-unknown-linux-gnu \
  --target aarch64-unknown-none
```

The workspace `rust-version` is `1.87`, but the **effective** requirement is
the pinned nightly above — a stable compiler will not build the bare-metal
target.

### Cross linker for aarch64 Linux

`.cargo/config.toml` configures `aarch64-linux-gnu-gcc` as the linker for
`aarch64-unknown-linux-gnu` (with `+neon` and `--as-needed`). Install it on
Debian/Ubuntu:

```bash
sudo apt-get install gcc-aarch64-linux-gnu
```

On Fedora:

```bash
sudo dnf install gcc-aarch64-linux-gnu
```

### QEMU (for cross-architecture execution)

Cross-compiled binaries are executed under QEMU user-mode emulators. The
gold-standard and differential test drivers expect these on `PATH`:

- `qemu-aarch64`
- `qemu-riscv64`
- `qemu-arm`
- `qemu-mips64el`
- `qemu-ppc64`
- `qemu-loongarch64`

On Debian/Ubuntu:

```bash
sudo apt-get install qemu-user qemu-user-static
```

The `differential_test` binary additionally hard-codes the path
`/tmp/qemu_bins/qemu-<arch>`. The CI workflow stages symlinks there; if you
run that driver locally, replicate the step:

```bash
mkdir -p /tmp/qemu_bins
for q in qemu-aarch64 qemu-riscv64 qemu-arm qemu-mips64el qemu-ppc64 qemu-loongarch64; do
  ln -sf "$(command -v $q)" "/tmp/qemu_bins/$q"
done
```

For bare-metal targets (`aarch64-unknown-none`, `x86_64-unknown-none`,
`riscv64gc-unknown-none-elf`), the system emulators are used via the
`just x86-64-run` and `just riscv64-run` recipes:

```bash
sudo apt-get install qemu-system-arm qemu-system-x86 qemu-system-riscv64   # Debian/Ubuntu
```

### Optional: `just` or `make`

The repository ships both a [`justfile`](../justfile) and a [`Makefile`](../Makefile)
with identical recipes. Either works; `just` is a single static binary
(`cargo install just` or your distro's package). All examples below show the
raw `cargo` command and, where one exists, the `just` / `make` shorthand.

### No external crates

The workspace depends on **no external crates** — only the Rust standard
library and the internal `vuma-*` path crates. Builds require no network
access to crates.io. See [`contributing.md` §6](contributing.md#6-zero-dependency-policy)
for the policy details.

---

## 2. Build Profiles

Three profiles are defined across the root [`Cargo.toml`](../Cargo.toml) and
[`.cargo/config.toml`](../.cargo/config.toml).

### `dev` (default)

```toml
[profile.dev]
opt-level = 0
debug = 2
```

Use for incremental development and debugging. Incremental compilation is
enabled globally in `.cargo/config.toml` (`incremental = true`).

```bash
cargo build --workspace              # debug binaries land in target/debug/
cargo build                          # just the root vuma binary
cargo check --workspace              # type-check only, no artifacts
```

### `release` (LTO, maximum optimization)

Defined in `Cargo.toml`:

```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
```

And tightened in `.cargo/config.toml`:

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
strip = true
panic = "abort"
```

This is the profile used for release artifacts, the gold-standard test
drivers, and the bootstrap `vumac` binary. It is slow to link (fat LTO,
single codegen unit) but produces the fastest runtime code.

```bash
cargo build --workspace --release     # binaries land in target/release/
```

### `release-fast` (no LTO, fast iterative builds)

```toml
[profile.release-fast]
inherits = "release"
opt-level = 3
lto = false
codegen-units = 16
debug = false
strip = true
```

A fast iterative profile for test-suite runs. It keeps `opt-level = 3` (so
QEMU-emulated executions stay fast) but disables LTO and bumps `codegen-units`
to 16. On a Raspberry Pi 5 a from-scratch build drops from 10+ minutes
(`release`) to roughly 1–2 minutes (`release-fast`), at a 5–10% runtime cost.

```bash
cargo build --profile release-fast --bin compile_dump --bin dump_ir
```

> **Important:** `--profile release-fast` puts binaries in
> `target/release-fast/`, **not** `target/release/`. Update your paths
> accordingly when invoking the driver binaries.

### Profile summary

| Profile | `opt-level` | `lto` | `codegen-units` | `debug` | `strip` | `panic` | Output dir |
|---|---|---|---|---|---|---|---|
| `dev` (default) | 0 | — | — | 2 | — | unwind | `target/debug/` |
| `release` | 3 | fat | 1 | — | true | abort | `target/release/` |
| `release-fast` | 3 | false | 16 | false | true | (inherits abort) | `target/release-fast/` |

### Quick check (core crates only)

When iterating on the core compiler without touching codegen/parser/tests:

```bash
cargo check -p vuma -p vuma-scg -p vuma-ive -p vuma-bd
# or: just check-fast / make check-fast
```

---

## 3. Cross-Compilation

### aarch64 Linux (user-space)

The primary cross target is `aarch64-unknown-linux-gnu`, configured in
`.cargo/config.toml` with the `aarch64-linux-gnu-gcc` linker and `+neon`:

```bash
cargo build --target aarch64-unknown-linux-gnu --workspace
cargo build --target aarch64-unknown-linux-gnu --workspace --release

# or via justfile:
just cross-aarch64
just cross-aarch64-release
```

### Other cross targets

The Cross-Compile CI workflow builds 8 targets. For targets with good
`cross-rs` support, use [`cross`](https://github.com/cross-rs/cross) (which
runs the build inside a Docker container with the right sysroot and linker):

```bash
cargo install cross
cross build --workspace --target riscv64gc-unknown-linux-gnu
cross build --workspace --target armv7-unknown-linux-gnueabihf
cross build --workspace --target mips64-unknown-linux-gnuabi64
cross build --workspace --target powerpc64-unknown-linux-gnu
```

For `loongarch64-unknown-linux-gnu`, `cross-rs` support is limited; the CI
workflow falls back to `cargo check` only:

```bash
rustup target add loongarch64-unknown-linux-gnu
cargo check --workspace --target loongarch64-unknown-linux-gnu
```

For `wasm32-unknown-unknown` (no OS, build only):

```bash
rustup target add wasm32-unknown-unknown
cargo build --workspace --target wasm32-unknown-unknown
```

### Bare-metal targets

Two bare-metal targets are pinned in `rust-toolchain.toml`:

- **`aarch64-unknown-none`** — Pi 5 / `raspi4b` machine. `.cargo/config.toml`
  sets `qemu-system-aarch64 -M raspi4b -serial stdio -kernel` as the runner
  and passes `-nostartfiles`, a custom linker script
  (`src/pi5/link.ld`), `--gc-sections`, and `-nodefaultlibs`.
- **`aarch64-unknown-none`** components come from the `rust-src` component
  listed in `rust-toolchain.toml`.

The `justfile` / `Makefile` also provide `x86-64-run` and `riscv64-run`
recipes for the `x86_64-unknown-none` and `riscv64gc-unknown-none-elf`
bare-metal targets:

```bash
just x86-64-run     # qemu-system-x86_64 -drive format=raw,file=target/x86_64-unknown-none/release/vuma-x86_64.bin -serial stdio
just riscv64-run    # qemu-system-riscv64 -machine virt -nographic -bios default -kernel target/riscv64gc-unknown-none-elf/release/vuma-riscv64
```

### Running cross-compiled binaries under QEMU

After building for a cross target, execute the binary with the matching
user-mode emulator:

```bash
# Build and run a single VUMA program for aarch64 via the vuma CLI
./target/release/vuma emit aarch64 hello.vuma -o hello.aarch64
qemu-aarch64 hello.aarch64

# Or build + run in one step:
./target/release/vuma run --isa aarch64 hello.vuma
```

The `vuma run` subcommand automatically selects `qemu-<isa>` for non-host
ISAs. On an AArch64 host, `aarch64` is native (no QEMU needed); on x86_64 it
is emulated.

---

## 4. Testing

### Workspace test suite

```bash
cargo test --workspace                  # all unit + integration tests
cargo test --workspace -- --nocapture   # visible println! output
cargo test --workspace --doc            # doc-tests
cargo test --workspace --no-fail-fast   # keep going after a failure
```

Task-runner equivalents:

```bash
just test            # or: make test
just test-verbose    # or: make test-verbose
just test-doc        # or: make test-doc
just test-filter filter=uart    # cargo test --workspace uart
```

### Per-crate tests

```bash
cargo test -p vuma-scg
cargo test -p vuma-ive
cargo test -p vuma-bd
cargo test -p vuma-core
cargo test -p vuma-codegen
cargo test -p vuma-parser
cargo test -p vuma-cor
cargo test -p vuma-proof
cargo test -p vuma-package
cargo test -p vuma-tests

# or: just test-crate crate=vuma-codegen / make test-single CRATE=vuma-codegen
```

### Useful selectors by area

```bash
# Codegen
cargo test -p vuma-codegen --lib emit            # emission (104 tests)
cargo test -p vuma-codegen --lib syscall_abi     # syscall translation (15 tests)
cargo test -p vuma-codegen --lib escape          # escape analysis + SROA
cargo test -p vuma-codegen --lib scheduler       # instruction scheduler
cargo test -p vuma-codegen --lib opt             # optimizer (DCE, CSE, e-graph, LICM)
cargo test -p vuma-codegen --lib memory_safety   # E041–E050 checks (16 tests)
cargo test -p vuma-codegen --lib bv_verify       # bitvector soundness gate

# Parser
cargo test -p vuma-parser --lib                  # lexer + parser (289 tests)
cargo test -p vuma-parser --test edge_cases

# Proof system
cargo test -p vuma-proof --lib                   # tactics, checker, counterexamples (132 tests)

# Integration / bootstrap
cargo test -p vuma-tests --lib self_host         # bootstrap self-host end-to-end
cargo test -p vuma-tests --lib bootstrap
cargo test -p vuma-tests --lib full_pipeline     # end-to-end compile() pipeline
cargo test -p vuma-tests --lib parser_roundtrip
cargo test -p vuma-tests --lib cross_backend

# Proof / soundness gate (the Proof-Verify CI subset)
cargo test --workspace --no-fail-fast -- bv_verify proof_artifacts proof_log
```

### Gold-standard test suite

The [`tests/gold_standard/`](../tests/gold_standard/) directory contains
`.vuma` programs grouped by category (`structs/`, `arithmetic/`,
`concurrency/`, …). They are compiled and executed by the `compile_dump`
driver binary across 7 native QEMU backends. The CI runner is
[`scripts/ci_run_tests.sh`](../scripts/ci_run_tests.sh); to run it locally:

```bash
# 1. Build the driver binaries (release for speed)
cargo build --release --bin compile_dump --bin differential_test \
                         --bin opt_level_test --bin fuzz_driver

# 2. Stage QEMU emulators where differential_test expects them
mkdir -p /tmp/qemu_bins
for q in qemu-aarch64 qemu-riscv64 qemu-arm qemu-mips64el qemu-ppc64 qemu-loongarch64; do
  ln -sf "$(command -v $q)" "/tmp/qemu_bins/$q"
done

# 3. Run the examples across all backends
./target/release/compile_dump diag x86_64 examples
./target/release/compile_dump diag aarch64 examples /tmp/qemu_bins/qemu-aarch64
# ...etc

# 4. Gold-standard run on x86_64
for f in tests/gold_standard/*/*.vuma; do
  name=$(basename "$f" .vuma)
  ./target/release/compile_dump "$f" /tmp/gs_${name}.bin x86_64
  chmod +x /tmp/gs_${name}.bin
  timeout 3 /tmp/gs_${name}.bin; echo "exit=$?"
done

# 5. O0 vs O3 optimizer soundness comparison
./target/release/opt_level_test examples

# 6. Fuzz driver (50 randomly generated programs)
./target/release/fuzz_driver --count 50 --seed 42
```

The CI treats exit codes `124` (timeout), `139` (segfault), and `134` (abort)
as failures; any other exit code is a pass.

Additional harness scripts:

- [`scripts/run_all_gold.sh`](../scripts/run_all_gold.sh) — runs the full
  gold-standard sweep.
- [`scripts/cross_backend_test.sh`](../scripts/cross_backend_test.sh) —
  cross-backend comparison.
- [`scripts/run_differential.sh`](../scripts/run_differential.sh) —
  differential testing across backends.
- [`scripts/run_fuzz.sh`](../scripts/run_fuzz.sh) — fuzz harness.
- [`scripts/run_all_kat.sh`](../scripts/run_all_kat.sh) /
  [`scripts/run_real_kat.sh`](../scripts/run_real_kat.sh) — known-answer
  tests for the `womb/` standard library.

### Benchmarks

```bash
cargo bench --workspace              # or: just bench / make bench
cargo bench -p vuma-codegen          # or: just bench-crate crate=vuma-codegen
```

The `vuma-tests` benchmark suite produces `BenchmarkResult { mean_ns,
median_ns, iterations }` across 8 categories: SCG construction, BD inference,
MSG construction, IVE verification, ARM64 codegen, C-equivalent comparison,
memory usage, and end-to-end pipeline.

### Documentation

```bash
cargo doc --workspace --no-deps                # just docs / make doc
cargo doc --workspace --no-deps --open         # just doc-open
cargo doc --workspace --no-deps --document-private-items   # just doc-private
```

The `ci.yml` `docs` job builds `cargo doc --workspace --no-deps` and uploads
`target/doc/` as an artifact (7-day retention).

---

## 5. The Bootstrap Compiler

VUMA is self-hosting. The directory [`womb/lang/`](../womb/lang/) contains a
small VUMA compiler written in VUMA itself. It is the bootstrap compiler:
it lexes, parses, lowers to IR, and emits x86_64 ELF executables — enough to
compile simple VUMA programs end-to-end.

### The five bootstrap files

| File | Role |
|---|---|
| [`womb/lang/full_lexer.vuma`](../womb/lang/full_lexer.vuma) | Entry point. Lexer with string literals and all operators. Token layout: `[type:u32][start:u32][len:u32][value:u64]` (24 bytes). Orchestrates the full pipeline: read source → lex → parse → IR → codegen → ELF → exit. |
| [`womb/lang/full_parser.vuma`](../womb/lang/full_parser.vuma) | Recursive-descent parser producing an AST arena. |
| [`womb/lang/ir_builder.vuma`](../womb/lang/ir_builder.vuma) | AST → IR buffer lowering. Stubs SCG construction, BD inference, and IVE verification. |
| [`womb/lang/codegen.vuma`](../womb/lang/codegen.vuma) | IR → x86_64 machine-code bytes. |
| [`womb/lang/elf.vuma`](../womb/lang/elf.vuma) | Writes the emitted code into an `a.out` ELF64 executable. |

Two sample inputs live alongside them: [`womb/lang/hello.vuma`](../womb/lang/hello.vuma)
and [`womb/lang/hello2.vuma`](../womb/lang/hello2.vuma).

### Building the bootstrap compiler

The Rust-hosted VUMA compiler compiles the five `.vuma` files into a single
`vumac` ELF, which in turn reads and compiles `womb/lang/hello.vuma`.

```bash
# 1. Build the Rust-hosted compiler (release recommended)
cargo build --release

# 2. Link the five bootstrap files into a vumac ELF
./target/release/vuma link \
  womb/lang/full_lexer.vuma \
  womb/lang/full_parser.vuma \
  womb/lang/ir_builder.vuma \
  womb/lang/codegen.vuma \
  womb/lang/elf.vuma \
  -o vumac

# 3. Run the bootstrap compiler on a sample input
./vumac womb/lang/hello.vuma
# → writes a.out (an x86_64 ELF produced entirely by the VUMA-written compiler)

./a.out
# → executes the bootstrapped binary
```

The bootstrap compiler emits **x86_64 ELF**, so it runs on an x86_64 host.
The end-to-end self-host test is `#[cfg(target_arch = "x86_64")]`-gated for
this reason.

### Testing the bootstrap

The self-host contract is exercised by the integration test in
[`src/tests/src/wave48_self_host.rs`](../src/tests/src/wave48_self_host.rs).
It compiles the five bootstrap files into a `vumac` ELF, runs
`./vumac womb/lang/hello.vuma`, then runs the emitted `a.out` and asserts
its stdout contains the expected output.

```bash
# Run the bootstrap self-host tests (x86_64 host only)
cargo test -p vuma-tests --lib self_host
cargo test -p vuma-tests --lib bootstrap

# The specific end-to-end test:
cargo test -p vuma-tests --lib -- bootstrap_self_host
```

The same test file also covers the multi-module `compile_modules` API and
the `merge_module_asts` helper (deduplication of identical functions,
rejection of conflicting definitions). The bootstrap tests are strict
(non-`#[ignore]`) and run as part of the standard `cargo test --workspace`
gate.

### What the bootstrap supports

The bootstrap compiler implements a deliberately small subset of VUMA:

- `syscall()` intrinsic (Linux x86_64 native numbering),
- `allocate()` / `free()` heap intrinsics,
- `if` / `else` and `while` control flow,
- byte-level memory access (`*(addr + offset)`),
- `print_int`.

It is not a feature-complete VUMA compiler — it is the existence proof that
VUMA can compile itself. The full compiler is the Rust workspace under
`src/`.

---

## 6. Troubleshooting

### `VUMA_NO_SCHED=1` — instruction scheduler miscompiles

The O2 instruction scheduler models memory dependencies via cast-aware
type-based alias analysis (TBAA), with IVE-proven non-aliasing overrides. If
you hit a miscompilation that you suspect is scheduler-related, disable it
as a debugging escape hatch:

```bash
VUMA_NO_SCHED=1 ./target/release/vuma build program.vuma -o program
```

With `VUMA_NO_SCHED=1` the scheduler pass is skipped entirely. If the
miscompilation disappears, the bug is in the scheduler's alias / pressure
model. Several historical bootstrap miscompilations were traced to exactly
this pass; see the root-cause notes in
`src/tests/src/wave48_self_host.rs`.

### Memory-safety verification errors

By default the compiler performs static verification of five memory
invariants (liveness, exclusivity, cleanup, origin, interpretation) and
emits **hard errors** on violations. If you are working on code that
intentionally violates an invariant (e.g. a test case for the verifier), use
the escape hatch:

```bash
./target/release/vuma build program.vuma -o program --no-memory-safety
```

`--no-memory-safety` is an explicit opt-out for development and testing; it
is not intended for production code. Verification levels can also be tuned
(`quick`, `normal`, `exhaustive`, `modular`, `constant-time`, `hardened`)
through the `VumaCompiler` API.

### `error: linker 'aarch64-linux-gnu-gcc' not found`

The `aarch64-unknown-linux-gnu` target is configured to use
`aarch64-linux-gnu-gcc` as its linker (`.cargo/config.toml`). Install the
cross toolchain:

```bash
sudo apt-get install gcc-aarch64-linux-gnu     # Debian/Ubuntu
sudo dnf install gcc-aarch64-linux-gnu         # Fedora
```

### `qemu-aarch64: command not found` (or other `qemu-<arch>`)

Install QEMU user-mode emulators:

```bash
sudo apt-get install qemu-user qemu-user-static
```

Then verify each emulator the test driver expects is on `PATH`:

```bash
for q in qemu-aarch64 qemu-riscv64 qemu-arm qemu-mips64el qemu-ppc64 qemu-loongarch64; do
  command -v "$q" || echo "missing $q"
done
```

### `differential_test` cannot find `/tmp/qemu_bins/qemu-<arch>`

The `differential_test` binary hard-codes the path
`/tmp/qemu_bins/qemu-<arch>`. Stage symlinks there:

```bash
mkdir -p /tmp/qemu_bins
for q in qemu-aarch64 qemu-riscv64 qemu-arm qemu-mips64el qemu-ppc64 qemu-loongarch64; do
  ln -sf "$(command -v $q)" "/tmp/qemu_bins/$q"
done
```

This is exactly what the CI workflow does; see
[`scripts/ci_run_tests.sh`](../scripts/ci_run_tests.sh).

### Binaries not found in `target/release/` after a `release-fast` build

`--profile release-fast` writes to **`target/release-fast/`**, not
`target/release/`. Either invoke the binaries from
`target/release-fast/<bin>` or pass `--profile release` (the slow LTO
profile) if you need them in `target/release/`.

### `cargo build` fails on stable Rust

VUMA requires the **pinned nightly** (`nightly-2026-03-01`). A stable
toolchain will fail on `naked_asm` and other nightly features. The
[`rust-toolchain.toml`](../rust-toolchain.toml) file should auto-select the
pinned nightly; if it does not, install it explicitly:

```bash
rustup toolchain install nightly-2026-03-01 \
  --component rustfmt,clippy,rust-src \
  --target aarch64-unknown-linux-gnu \
  --target aarch64-unknown-none
```

### `cargo fmt --check` fails in CI but not locally

Your local `rustfmt` may differ from the pinned nightly's. Run
`rustup which rustfmt` and confirm it resolves to the
`nightly-2026-03-01` toolchain. If you have a default stable `rustfmt`
shadowing it, invoke via `rustup run nightly-2026-03-01 cargo fmt --all -- --check`.

### Clippy fails with `-D warnings` on existing code

The strict clippy gate (`cargo clippy --workspace -- -D warnings`) in
`ci.yml` blocks on any warning. If your change touches a file with
pre-existing warnings unrelated to your diff, you will need to fix them or
the PR will not pass. The Hardening workflow's advisory clippy job tracks
the warning backlog separately; the strict gate in `ci.yml` is the one that
matters for merge.

### Build is slow on a Raspberry Pi 5 (or other ARM SBC)

Use the `release-fast` profile for iterative test-suite runs — it disables
LTO and bumps `codegen-units` to 16, cutting a 10+ minute release build down
to roughly 1–2 minutes at a 5–10% runtime cost:

```bash
cargo build --profile release-fast --bin compile_dump --bin dump_ir
```

### Gold-standard test exits with code 139 or 134

Exit 139 = segfault (SIGSEGV), exit 134 = abort (SIGABRT). Both are treated
as failures by the CI runner. This usually means the emitted code for that
backend is unsound for that specific program. Bisect by running the same
program under `x86_64` (native) and the failing backend, and compare the
emitted code with `cargo run --bin dump_ir` / `cargo run --bin scg_dump`.

### Cleaning everything

```bash
cargo clean                 # just clean / make clean
rm -rf target/doc           # just clean-doc / make clean-doc
```

---

If you hit something not covered here, check the [README](../README.md) and
[`contributing.md`](contributing.md), then open an
[issue](https://github.com/pkhairkh/vuma/issues).
