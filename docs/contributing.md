# Contributing to VUMA

Thanks for your interest in contributing to VUMA — the Verified-Unsafe Memory
Access language framework. This document covers everything you need to get a
local development environment running, follow the project's code style, write
and run tests, and land a pull request.

VUMA is a **zero-external-dependency** Rust workspace: every crate compiles
against `std` only. There is no `serde`, no `clap`, no `libc`, no `rayon`. Every
external crate that the project once used has been replaced with a hand-written
implementation. This constraint shapes most of the conventions below — please
read [§6 Zero-Dependency Policy](#6-zero-dependency-policy) before adding any
new code.

---

## Table of Contents

1. [Development Setup](#1-development-setup)
2. [Code Style](#2-code-style)
3. [Testing](#3-testing)
4. [Pull Request Process](#4-pull-request-process)
5. [Project Structure](#5-project-structure)
6. [Zero-Dependency Policy](#6-zero-dependency-policy)

---

## 1. Development Setup

### Toolchain

VUMA pins a specific nightly compiler in [`rust-toolchain.toml`](../rust-toolchain.toml).
The pinned channel is **`nightly-2026-03-01`**, with the components
`rustfmt`, `clippy`, and `rust-src`, and the targets
`aarch64-unknown-linux-gnu` and `aarch64-unknown-none`. The nightly channel is
required for:

- inline assembly enhancements (`naked_asm`, etc.),
- unstable features used by the bare-metal crate,
- advanced const-generics and trait-system features.

Because the toolchain file is checked in, `cargo` will auto-install the pinned
toolchain on first invocation. To set things up explicitly:

```bash
# Install the pinned nightly toolchain and required components/targets.
rustup toolchain install nightly-2026-03-01 \
  --component rustfmt,clippy,rust-src \
  --target aarch64-unknown-linux-gnu \
  --target aarch64-unknown-none
```

Or, equivalently, use the project's task runner (see [Build commands](#build-commands)
below):

```bash
just setup        # or: make setup
```

### System dependencies

For native development you only need the Rust toolchain. For cross-compilation
and cross-architecture test execution you will additionally need:

- `aarch64-linux-gnu-gcc` — the cross linker configured in
  [`.cargo/config.toml`](../.cargo/config.toml) for `aarch64-unknown-linux-gnu`.
  On Debian/Ubuntu: `sudo apt-get install gcc-aarch64-linux-gnu`.
- `qemu-user` / `qemu-user-static` — QEMU user-mode emulators used to execute
  cross-compiled binaries in the gold-standard and differential test suites.
  The `vuma-tests` CI workflow requires `qemu-aarch64`, `qemu-riscv64`,
  `qemu-arm`, `qemu-mips64el`, `qemu-ppc64`, and `qemu-loongarch64` on `PATH`.
- `qemu-system-aarch64`, `qemu-system-x86_64`, `qemu-system-riscv64` — system
  emulators used by the `just x86-64-run` / `just riscv64-run` recipes for
  bare-metal targets.

### Build commands

VUMA ships both a [`justfile`](../justfile) and a [`Makefile`](../Makefile)
with identical recipes. Use whichever you prefer. The most common ones:

| Task | `just` | `make` | `cargo` |
|---|---|---|---|
| Build (debug) | `just build` | `make build` | `cargo build --workspace` |
| Build (release) | `just release` | `make build-release` | `cargo build --workspace --release` |
| Fast release | — | — | `cargo build --workspace --profile release-fast` |
| Type-check | `just check` | `make check` | `cargo check --workspace` |
| Fast check (core crates) | `just check-fast` | `make check-fast` | `cargo check -p vuma -p vuma-scg -p vuma-ive -p vuma-bd` |
| Run all tests | `just test` | `make test` | `cargo test --workspace` |
| Format | `just fmt` | `make fmt` | `cargo fmt --all` |
| Lint | `just lint` | `make lint` | `cargo fmt --all -- --check && cargo clippy --workspace -- -D warnings` |

See [`docs/building.md`](building.md) for the full build, profile, and
cross-compilation reference.

### Verifying the setup

A clean setup should pass these three checks from the repository root:

```bash
cargo build --workspace            # compiles
cargo fmt --all -- --check         # no formatting diffs
cargo clippy --workspace -- -D warnings   # no clippy warnings
```

---

## 2. Code Style

### rustfmt

Formatting is governed by [`rustfmt.toml`](../rustfmt.toml):

```toml
max_width = 100
tab_spaces = 4
edition = "2021"
```

- Maximum line width is **100 columns**.
- Indentation is **4 spaces** (no tabs).
- The 2021 edition ruleset is used.

Run `just fmt` (or `cargo fmt --all`) before committing. CI runs
`cargo fmt --all -- --check` and fails the build on any diff. Do not disable
`rustfmt` per-file with `#[rustfmt::skip]` without a comment explaining why.

### clippy

[`clippy.toml`](../clippy.toml) sets a single project-wide threshold:

```toml
cognitive-complexity-threshold = 50
```

Clippy is the **strict** lint gate. CI runs:

```bash
cargo clippy --workspace -- -D warnings
```

`-D warnings` means every clippy warning is a hard error. A separate advisory
clippy job (in the Hardening workflow) additionally runs
`cargo clippy --workspace --all-targets` to surface warnings in test and
example targets; the strict gate in `ci.yml` is the one that blocks merge.

When you can, fix the warning rather than suppressing it. If a lint is
genuinely a false positive, suppress it with an attribute scoped as tightly as
possible and a short comment:

```rust
#[allow(clippy::needless_range_loop)] // index used as both value and position
```

Note that a small number of project-wide allows live at the top of
`src/codegen/src/lib.rs` for lints that fire pervasively on the codegen IR;
do not add to that list casually.

### Naming and conventions

- Follow standard Rust naming ([`Rust API Guidelines`][api-naming]):
  `UpperCamelCase` for types/traits/enums, `snake_case` for functions/variables/
  modules, `SCREAMING_SNAKE_CASE` for consts and statics.
- Module-level docs (`//!`) at the top of every file. The codegen, parser, IVE,
  and proof crates all carry a prose overview of their pipeline role — match
  that style in new files.
- Public items get doc comments. Private items that implement non-obvious
  logic should too.
- Error types implement `std::fmt::Display` and `std::error::Error`; the
  codebase has no `anyhow` / `thiserror` to lean on (see
  [§6 Zero-Dependency Policy](#6-zero-dependency-policy)).

[api-naming]: https://rust-lang.github.io/api-guidelines/naming.html

---

## 3. Testing

### How to run tests

The full test suite is the workspace test suite:

```bash
cargo test --workspace                  # all unit + integration tests
cargo test --workspace -- --nocapture   # with println! output visible
cargo test --workspace --doc            # doc-tests only
```

Per-crate:

```bash
cargo test -p vuma-codegen
cargo test -p vuma-parser
cargo test -p vuma-proof
cargo test -p vuma-ive
cargo test -p vuma-tests
```

Or via the task runner:

```bash
just test-crate crate=vuma-codegen   # or: make test-single CRATE=vuma-codegen
```

### Test categories

Tests live in two places:

1. **In-crate unit tests** — `#[cfg(test)] mod tests { ... }` blocks at the
   bottom of each module in every `vuma-*` crate. These cover function-level
   edge cases and are the first line of defense.
2. **The `vuma-tests` crate** (`src/tests/`) — the integration and
   cross-crate test harness. Its module-level docs (`src/tests/src/lib.rs`)
   describe the categories:

| Category | Module | Scope |
|---|---|---|
| Unit | all crates | Individual crate functions, edge cases |
| Integration | `framework` | Cross-crate pipelines (parse → SCG → verify) |
| Verification | `trivial`, `dlist` | IVE invariant checks and proofs |
| Codegen | `codegen` | ARM64 / multi-arch code emission, ELF generation |
| Pipeline | `full_pipeline` | Full `compile()` pipeline end-to-end |
| Parser | `parser_roundtrip` | Parse roundtrip: source → AST → SCG |
| Benchmark | `benchmarks` | Performance benchmarks across 8 categories |

A separate **gold-standard test suite** lives under [`tests/gold_standard/`](../tests/gold_standard/)
as `.vuma` source programs grouped by category (`structs/`, `arithmetic/`,
`concurrency/`, …). These are compiled and executed by the `compile_dump`
driver binary under each QEMU backend in CI (see
[`scripts/ci_run_tests.sh`](../scripts/ci_run_tests.sh) and the
`VUMA Compiler Tests` workflow).

The **differential test** (`differential_test` binary) compares emitted code
across the 7 native QEMU backends. It hard-codes the path
`/tmp/qemu_bins/qemu-<arch>` for the emulators; the CI workflow stages
symlinks there (see [§4 Pull Request Process](#4-pull-request-process)).

### Useful test selectors

```bash
# Codegen — emission, syscall ABI, escape analysis, scheduler, optimizer
cargo test -p vuma-codegen --lib emit
cargo test -p vuma-codegen --lib syscall_abi
cargo test -p vuma-codegen --lib escape
cargo test -p vuma-codegen --lib scheduler
cargo test -p vuma-codegen --lib opt
cargo test -p vuma-codegen --lib memory_safety

# Parser, proof system
cargo test -p vuma-parser --lib
cargo test -p vuma-proof --lib

# Bootstrap self-host end-to-end (x86_64 host only)
cargo test -p vuma-tests --lib self_host
cargo test -p vuma-tests --lib bootstrap

# Proof / bitvector soundness gate
cargo test --workspace --no-fail-fast -- bv_verify proof_artifacts proof_log
```

### Adding new tests

- **Unit tests** for a function go in a `#[cfg(test)] mod tests` block at the
  bottom of the same file. Use plain `assert!` / `assert_eq!`.
- **Integration tests** that exercise more than one crate belong in
  `src/tests/src/`. Add a new module there and register it in
  [`src/tests/src/lib.rs`](../src/tests/src/lib.rs). Reuse the helpers in
  `src/tests/src/framework.rs` (pipeline builders, SCG builders, helper macros)
  rather than re-implementing them.
- **Gold-standard `.vuma` programs** are added under `tests/gold_standard/` in
  the appropriate category directory. They must compile to a deterministic
  exit code across all backends; the CI runner treats anything other than a
  timeout (124), segfault (139), or abort (134) as a pass.
- Tests that execute cross-compiled binaries must be gated with
  `#[cfg(target_arch = "x86_64")]` (or the relevant host arch) and must not
  assume a QEMU emulator is present unless they stage one themselves.

---

## 4. Pull Request Process

### Before opening a PR

Run the local equivalent of what CI will run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

These three are the blocking gates in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml).
If any of them fail locally, they will fail CI.

### Commit messages

Use the conventional, imperative style:

```
<area>: <short imperative summary>

<optional body explaining why, not what>
```

Where `<area>` is the affected crate or subsystem (e.g. `codegen`,
`parser`, `ive`, `bd`, `proof`, `scg`, `cor`, `package`, `docs`, `ci`,
`tests`). Examples:

```
codegen: fix AArch64 shift encoding for imm=0
parser: recover from missing `;` after return expr
ive: cache interprocedural escape results per callsite
docs: add cross-compilation section to building.md
```

Keep the summary line under 72 characters. Reference issues in the body
(`Closes #123`, `Refs #456`) rather than the summary.

### What CI runs on a PR

Every push to `main` and every PR targeting `main` runs the following
workflows. All of them must be green before merge:

| Workflow | File | What it checks |
|---|---|---|
| **CI** | [`ci.yml`](../.github/workflows/ci.yml) | Build, test (matrix: Ubuntu x86_64 + macOS aarch64), `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo doc --no-deps` |
| **Cross-Compile** | [`cross-compile.yml`](../.github/workflows/cross-compile.yml) | Builds for 8 targets: x86_64, aarch64, riscv64gc, armv7, mips64, powerpc64, loongarch64, wasm32 (via `cross` + QEMU where supported) |
| **VUMA Compiler Tests** | [`vuma-tests.yml`](../.github/workflows/vuma-tests.yml) | Gold-standard suite across 7 QEMU backends, O0-vs-O3 soundness, 50-program fuzz run |
| **Proof Verify** | [`proof-verify.yml`](../.github/workflows/proof-verify.yml) | `bv_verify`, `proof_artifacts`, and `proof_log` test subsets |
| **Hardening** | [`wave50-hardening.yml`](../.github/workflows/wave50-hardening.yml) | Advisory clippy with `--all-targets` (errors-only) + strict `cargo test --workspace --no-fail-fast` |

Notes:

- The Hardening workflow's clippy job is **advisory** (it tolerates the
  existing warning backlog and fails only on clippy errors). The strict
  clippy gate is the one in `ci.yml`. New code should not add to the warning
  backlog.
- The VUMA Compiler Tests workflow stages QEMU emulator symlinks under
  `/tmp/qemu_bins/` because the `differential_test` binary hard-codes that
  path. If you run the gold-standard driver locally, replicate that step
  (see [`scripts/ci_run_tests.sh`](../scripts/ci_run_tests.sh)).
- Release artifacts are produced by [`release.yml`](../.github/workflows/release.yml)
  on version tags (`v*`) for x86_64 Linux, aarch64 Linux, x86_64 macOS, and
  x86_64 Windows.

### Review expectations

- PRs must not introduce external dependencies (see
  [§6 Zero-Dependency Policy](#6-zero-dependency-policy)). `Cargo.lock` after
  your change should contain only `vuma-*` packages.
- New public API needs a doc comment and, where reasonable, a test.
- Bug-fix PRs should include a regression test that fails before the fix.
- Large architectural changes should be discussed in an issue first.

---

## 5. Project Structure

The repository is a Cargo workspace of 10 internal crates plus the root
binary crate. The layout (abbreviated; see the [README](../README.md) for the
full tree):

```
vuma/
├── Cargo.toml              # workspace root — internal path deps only
├── rust-toolchain.toml     # pinned nightly + components + targets
├── rustfmt.toml            # max_width=100, tab_spaces=4
├── clippy.toml             # cognitive-complexity-threshold=50
├── justfile / Makefile     # developer task recipes
├── build.rs                # injects rustc version into --version output
├── .cargo/config.toml      # per-target linkers + rustflags
├── src/
│   ├── main.rs             # CLI: build, run, check, emit, compile, link, disasm
│   ├── pipeline.rs         # full compile() pipeline
│   ├── api.rs              # public VumaCompiler / CompileConfig API
│   ├── scg/                # vuma-scg — Semantic Computation Graph IR
│   ├── ive/                # vuma-ive — Invariant Verification Engine
│   ├── bd/                 # vuma-bd — Behavioral Descriptors (RepD/CapD/RelD)
│   ├── vuma/               # vuma-core — MSG, memory model, security
│   ├── codegen/            # vuma-codegen — multi-arch backends + optimizer
│   ├── parser/             # vuma-parser — lexer, parser, AST, AST→SCG
│   ├── cor/                # vuma-cor — Continuous Optimization Runtime (JIT)
│   ├── proof/              # vuma-proof — formal proof system
│   ├── package/            # vuma-package — manifest parser, dep resolver
│   ├── tests/              # vuma-tests — integration test suite
│   └── bin/                # driver binaries: compile_dump, dump_ir, scg_dump, ...
├── womb/                   # VUMA-native standard library (.vuma files)
│   ├── lang/               # 5-file self-hosting bootstrap compiler
│   ├── lib/  crypto/  net/  collections/  ...
├── tests/
│   ├── gold_standard/      # .vuma programs grouped by category
│   └── *.rs                # top-level integration tests
└── scripts/                # test harnesses (ci_run_tests.sh, run_all_gold.sh, ...)
```

### Where to add new code

**A new codegen backend** (new ISA):

1. Create a module under `src/codegen/src/<arch>.rs` (or `<arch>/mod.rs` for
   multi-file backends — see `x86_64/`, `arm32/`, `loongarch64/`, `mips64/`,
   `ppc64/`, `wasm32/` for the pattern). Implement the `Backend` and
   `TargetInfo` traits defined in [`src/codegen/src/backend.rs`](../src/codegen/src/backend.rs)
   (`Endianness`, `OutputFormat`, register classes, encoding, ELF section
   emission).
2. Register the module in [`src/codegen/src/lib.rs`](../src/codegen/src/lib.rs)
   (`pub mod <arch>;`) and wire it into the backend-dispatch match in
   `backend.rs` (alongside the existing `Arm32Backend`, `X86_64Backend`, …).
3. Add the ISA to the `IsaArg` enum and `IsaArg::parse` in
   [`src/main.rs`](../src/main.rs) so the CLI accepts `--isa <arch>` and
   `vuma emit <arch>`.
4. Add unit tests under `#[cfg(test)] mod tests` in your new module. If the
   target can run under QEMU user-mode, add a gold-standard entry and extend
   the backend list in `scripts/ci_run_tests.sh`.

**A new `womb/` standard-library module** (new VUMA code):

1. Add a `.vuma` file under the appropriate `womb/` subdirectory
   (`lib/`, `crypto/`, `net/`, `collections/`, `fs/`, `io/`, `string/`,
   `encoding/`, `codec/`, `graph/`, `ieee/`, `containers/`, `env/`).
2. If it exposes externally consumable symbols, document them in the file's
   header comment (the womb modules use a `// <file> — <one-line description>`
   convention at the top).
3. Add a KAT (known-answer test) under `scripts/womb_kat_tests/` or
   `scripts/real_kat_tests/` if the module implements a testable algorithm.
   See [`scripts/run_all_kat.sh`](../scripts/run_all_kat.sh) and
   [`scripts/womb_test_harness.sh`](../scripts/womb_test_harness.sh).
4. Cross-module references resolve at link time via `import` declarations and
   `vuma link`; see the README's "Module System" section.

**A new test**:

- Unit test for a crate function → `#[cfg(test)] mod tests` at the bottom of
  the same file.
- Cross-crate integration test → new module in [`src/tests/src/`](../src/tests/src/),
  registered in `src/tests/src/lib.rs`, using helpers from
  `src/tests/src/framework.rs`.
- End-to-end `.vuma` program → `tests/gold_standard/<category>/`.
- Long-running / property / fuzz test → [`tests/`](../tests/) top-level
  integration tests (`property_tests.rs`, `verification_tests.rs`,
  `loop_unroll_tests.rs`, …) or [`src/parser/fuzz/`](../src/parser/fuzz/).

**A new CLI subcommand**:

1. Add a variant to the `Commands` enum in [`src/main.rs`](../src/main.rs)
   (around line 268), with a doc comment describing usage.
2. Write a `parse_<subcommand>` function (following the `parse_emit` /
   `parse_compile` / `parse_check` pattern) and wire it into the top-level
   arg loop.
3. Implement the command in the `match Commands { ... }` dispatch (the
   command-implementation section of `main.rs`).
4. Add a test in `vuma-tests` that exercises the new subcommand through the
   public `VumaCompiler` API where possible.

---

## 6. Zero-Dependency Policy

> **The entire VUMA workspace depends on nothing but the Rust standard
> library.** `Cargo.lock` contains only `vuma-*` packages. There are no
> external crates.

This is a deliberate, load-bearing design constraint, not a stylistic
preference. It exists so that:

- the compiler can be audited end-to-end without trusting third-party code,
- the bootstrap story stays tractable (the self-hosting compiler in
  `womb/lang/` reimplements VUMA in VUMA, with no Rust ecosystem to port),
- builds are reproducible and fast, with no registry fetches.

### What this means for contributors

**Do not add an entry to `[dependencies]` or `[dev-dependencies]` in any
`Cargo.toml` that points at a crates.io package.** The only allowed
dependencies are the internal path crates declared in the root
`Cargo.toml` under `[workspace.dependencies]` (`vuma-scg`, `vuma-ive`,
`vuma-core`, `vuma-bd`, `vuma-codegen`, `vuma-parser`, `vuma-cor`,
`vuma-proof`, `vuma-package`).

If you need functionality that a third-party crate would normally provide,
write it by hand inside the appropriate `vuma-*` crate. The codebase already
contains hand-written replacements for many common crates:

| What you might reach for | What VUMA uses instead |
|---|---|
| `serde` / `serde_json` | hand-written recursive-descent JSON parser (`src/json_value.rs`, `ProfileData::from_json` in `src/codegen/src/egraph.rs`) |
| `clap` | hand-written arg parser in `src/main.rs` |
| `log` / `env_logger` | the `vuma_log!` macro (per-crate, no-op in release) |
| `toml` | `src/package/src/toml_lite.rs` |
| `libc` | direct `syscall()` lowering in codegen; no `libc` bindings |
| `rayon` / `crossbeam` | single-threaded, or `womb/lib/threading.vuma` at the language level |
| `regex` | hand-written matchers where needed |
| `sha2` / `aes` / etc. | `womb/crypto/*.vuma` (pure VUMA implementations) |

A representative example: the codegen crate's `Cargo.toml` explicitly notes
that `serde`/`serde_json` were removed and the single JSON call-site was
migrated to a hand-written parser — the on-disk JSON format was preserved.
Follow that precedent: when removing a dependency, keep the public data
format stable and replace the parsing/serialization with hand-written code.

### Enforcement

- **`Cargo.lock` review.** Because the workspace has no external deps, the
  lockfile lists only `vuma-*` packages. A PR that adds any other package is
  a red flag and will be caught in review.
- **CI does not currently run `cargo deny`**, but the workspace's
  `[dependencies]` sections are empty of external crates by construction;
  adding one shows up as a diff in `Cargo.toml` and `Cargo.lock`.
- **`[net] offline = false`** is set in `.cargo/config.toml`, but in practice
  the workspace never needs the network to build.

If you believe an external dependency is genuinely unavoidable, open an issue
first. The default answer is "reimplement it"; exceptions are rare and
require maintainer sign-off.

---

## Questions

- Bugs and feature requests: [GitHub Issues](https://github.com/pkhairkh/vuma/issues)
- Repository: <https://github.com/pkhairkh/vuma>
- License: MIT (see [`LICENSE`](../LICENSE))

When in doubt, match the surrounding code. The codebase is its own style
guide.
