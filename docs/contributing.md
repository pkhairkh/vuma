# Contributing to VUMA 2.0

Thanks for your interest in contributing to VUMA — the Verified-Unsafe Memory
Access language framework. VUMA 2.0 is **PMT-only**: every test in the suite
is written in PMT syntax (`layout` / `State` / `state_new`), and the legacy
pointer dialect is no longer accepted.

This document covers getting a dev environment running, following the code
style, writing PMT tests, adding new backends, and landing a pull request.
For the full build / cross-compilation reference see
[`building.md`](building.md).

---

## 1. Getting Started

### Clone

```bash
git clone https://github.com/pkhairkh/vuma.git
cd vuma
```

The toolchain is pinned via [`rust-toolchain.toml`](../rust-toolchain.toml)
to **`nightly-2026-03-01`** (with `rustfmt`, `clippy`, `rust-src`, and the
`aarch64-unknown-linux-gnu` / `aarch64-unknown-none` targets). `cargo`
auto-installs it on first use; to install explicitly:

```bash
rustup toolchain install nightly-2026-03-01 \
  --component rustfmt,clippy,rust-src \
  --target aarch64-unknown-linux-gnu \
  --target aarch64-unknown-none
```

### Build

The standard test driver is `compile_dump`. Build it with the `release-fast`
profile (the profile the test runner uses):

```bash
cargo build --profile release-fast --bin compile_dump
# → target/release-fast/compile_dump
```

> On a constrained host (≤ 4 GiB RAM) `release-fast` OOMs. Use the
> dev-profile workaround in [`building.md` §7](building.md#7-constrained-memory-workaround).

### Run tests

The end-to-end cross-backend runner is
[`scripts/pi5_test_suite.sh`](../scripts/pi5_test_suite.sh). It builds
`compile_dump`, walks `tests/gold_standard/`, compiles every `.vuma` file on
every backend, runs each under QEMU / wasmtime, and checks the exit code
against the `// Expected exit code: N` header.

```bash
scripts/pi5_test_suite.sh --workers 8 --fresh --verify
```

For a single quick check (one file, one backend, no QEMU):

```bash
./target/release-fast/compile_dump diag x86_64 \
    tests/gold_standard/pmt_wave2/two_states.vuma
```

For unit / integration tests: `cargo test --workspace` or
`cargo test -p vuma-codegen`.

### Verify the setup

A clean checkout should pass these three from the repo root:

```bash
cargo build --workspace                       # compiles
cargo fmt --all -- --check                    # no formatting diffs
cargo clippy --workspace -- -D warnings       # no clippy warnings
```

---

## 2. Code Style

### Rust nightly

All code targets the pinned `nightly-2026-03-01` toolchain. Do not introduce
features that require a newer nightly, and do not gate code on `#[stable]` —
the workspace is nightly-only by design.

### Crate-root clippy allows

Each crate root (`src/main.rs`, `src/*/src/lib.rs`) carries a single
crate-wide clippy allow-list:

```rust
#![allow(clippy::manual_range_contains, clippy::map_unwrap_or,
         clippy::unnecessary_cast,    clippy::redundant_closure,
         clippy::if_same_then_else,   clippy::collapsible_if,
         clippy::useless_format)]
```

When adding a new crate root, copy this line verbatim. Do **not** scatter
`#[allow(clippy::...)]` attributes inside modules — if a lint truly must be
suppressed crate-wide, add it to the crate-root allow-list. Otherwise fix the
code. The strict clippy gate `cargo clippy --workspace -- -D warnings` runs
on every PR.

### rustfmt

Formatting is governed by [`rustfmt.toml`](../rustfmt.toml):

- Maximum line width: **100 columns**
- Indentation: **4 spaces, no tabs**
- Edition: **2021**

Run `cargo fmt --all` before every commit.

### Naming and conventions

- `UpperCamelCase` for types, traits, enum variants.
- `snake_case` for functions, methods, variables, modules.
- `SCREAMING_SNAKE_CASE` for `const` and `static`.
- Public items need a `///` doc comment; private items should where the
  intent is non-obvious.
- Prefer `&str` / `&[T]` over `String` / `Vec<T>` in function signatures.
- No `unsafe` blocks without a `// SAFETY:` comment explaining the invariant.

### Zero external dependencies

The workspace depends on **no external crates** — only `std` and the internal
`vuma-*` path crates. There is no `serde`, no `clap`, no `libc`, no `rayon`.
Do not add `[dependencies]` entries for external crates; if you need a
capability, hand-write it. `Cargo.lock` after your change should contain only
`vuma-*` packages.

---

## 3. PMT-Only Test Policy

VUMA 2.0 is **PMT-only**. All new tests MUST be written in PMT syntax:

- Declare a **`layout`** — a pure type-level description of a record (fields,
  types, offsets, size, alignment). A layout does not allocate storage.
- Construct a **`State`** (typed view over the program's single backing
  memory buffer) with **`state_new(LayoutName)`**. This carves a slot of the
  buffer with the layout's size and alignment; the slot's address is not
  exposed to the program.
- Access fields with `s.field` — reads and writes are statically known to be
  in-bounds by the type checker.

A minimal PMT test:

```vuma
// two_states — PMT Wave 2: two independent states
// Expected exit code: 30
//
// Allocates two Points, sets x=10 on the first and x=20 on the second,
// returns their sum (30). Verifies that each state has its own buffer
// and field writes don't cross-contaminate.

layout Point = { x: u32, y: u32 }

fn main() -> i32 {
    let p = state_new(Point);
    let q = state_new(Point);
    p.x = 10;
    q.x = 20;
    return p.x + q.x;
}
```

### What is NOT accepted

- **Pointer syntax** — `*ptr = expr`, `&x`, `allocate(n)`, `free(p)` — is
  legacy and not accepted by the 2.0 test runner. Pointer-dialect programs
  from 1.x have either been migrated to PMT (see `pmt_wave7/` for migrations
  of `concurrency/conc_swap.vuma` and friends) or removed.
- **Tests that bypass the type checker** — every PMT test must type-check
  cleanly; the IVE runs only the three state verifiers (`state_read`,
  `state_write`, `state_transform`).
- **Tests without an `// Expected exit code: N` header** — the runner reads
  this line and compares it against the process exit status.

If you are migrating a legacy pointer test to PMT, put the result under the
appropriate `pmt_wave*` directory (see
[`building.md` §5](building.md#5-test-categories)).

---

## 4. Adding a New Backend

VUMA 2.0 currently ships 19 backends (see the `BackendKind` enum in
[`src/codegen/src/backend.rs`](../src/codegen/src/backend.rs)). To add a new
one:

### Step 1 — implement the `Backend` trait

Add a new module `src/codegen/src/<arch>.rs` and implement the `Backend`
trait (defined in `backend.rs`). The trait requires:

| Method                  | Responsibility                                             |
|-------------------------|------------------------------------------------------------|
| `target_info()`         | Return this backend's target info (pointer width, ABI, …)  |
| `allocate_registers()`  | Allocate physical registers for an IR function             |
| `encode_function()`     | Encode one allocated function into machine code bytes      |
| `encode_program()`      | Encode an allocated program into ELF / .wasm / raw binary  |
| `return_stub()`         | Minimal return stub (`RET`, `mov eax,0; ret`, `end`, …)    |
| `trampoline(addr)`      | Trampoline that jumps to `entry_addr`                      |
| `disassemble(bytes, addr)` | Disassemble `bytes` at virtual address `addr`           |
| `name()`                | Human-readable name (e.g. `"aarch64"`)                     |

You will also need a `LatencyTable` entry for the new architecture (see
[`src/codegen/src/scheduler.rs`](../src/codegen/src/scheduler.rs)).

### Step 2 — add to `BackendKind`

Add a new variant to the `BackendKind` enum in `backend.rs`, and extend the
`isa_name()`, `from_str()`, and `qemu_binary()` match arms.

### Step 3 — add the QEMU mapping to the test runner

Edit [`scripts/pi5_test_suite.sh`](../scripts/pi5_test_suite.sh) and add a
new `binfmt_misc` entry to the `entries` array. Each entry has the form
`name|qemu_binary|magic_hex|mask_hex`:

```bash
"qemu-<arch>|qemu-<arch>|<elf_magic>|<elf_mask>"
```

The magic and mask are the ELF header bytes that let `binfmt_misc` recognise
the architecture. If the new backend is wasm-based (not QEMU), extend
[`scripts/wasm32_runner.py`](../scripts/wasm32_runner.py) instead.

### Step 4 — add tests and verify

Add at least one PMT test under `tests/gold_standard/<arch>/` (or under an
existing category if the test exercises a general feature) with an
`// Expected exit code: N` header. Run the new backend on the full
gold-standard suite to confirm agreement with the other 18 backends:

```bash
scripts/pi5_test_suite.sh --workers 8 --backends <arch> --verify
```

---

## 5. Adding a New Test

### Step 1 — pick a category

Tests live under [`tests/gold_standard/`](../tests/gold_standard/) in
category directories. Pick the most specific one (see
[`building.md` §5](building.md#5-test-categories) for the full list). If the
test exercises a PMT-specific feature, use the matching `pmt_wave*` directory.

### Step 2 — write the `.vuma` file

Every test file MUST begin with the standard header:

```
// <name> — <one-line description>
// Expected exit code: <N>
//
// <longer description / what this tests>
//
// VUMA Key Concepts:
//   - <bullet list of PMT features exercised>
```

The runner parses the `Expected exit code:` line; without it the test is
skipped. The body is PMT-only — see [§3 PMT-Only Test Policy](#3-pmt-only-test-policy).

### Step 3 — verify locally

Build `compile_dump` and run the test under `x86_64` (native, fastest) plus
at least one cross-backend (e.g. `aarch64` via QEMU):

```bash
cargo build --profile release-fast --bin compile_dump

./target/release-fast/compile_dump diag x86_64 \
    tests/gold_standard/<category>/<name>.vuma

./target/release-fast/compile_dump diag aarch64 \
    tests/gold_standard/<category>/<name>.vuma qemu-aarch64
```

The exit code printed by `compile_dump` must match the `// Expected exit code:`
header on **every** backend. If any backend disagrees, do not commit the test —
file an issue against the offending backend instead.

### Step 4 — run the full category

```bash
scripts/pi5_test_suite.sh --workers 8 --backends x86_64,aarch64,riscv64 --verify
```

---

## 6. Pull Request Process

### Before opening a PR

1. `cargo fmt --all` — apply formatting.
2. `cargo clippy --workspace -- -D warnings` — no clippy warnings.
3. `cargo test --workspace` — unit / integration tests pass.
4. `scripts/pi5_test_suite.sh --workers 8 --verify` — gold-standard suite
   passes on every backend you touched.
5. No external dependencies added (`Cargo.lock` contains only `vuma-*`).
6. New public API has a `///` doc comment and, where reasonable, a test.
7. Bug-fix PRs include a regression test that fails before the fix.

### Commit messages

Use the conventional, imperative style:

```
<area>: <short imperative summary>

<optional body explaining why, not what>
```

`<area>` is the affected crate or subsystem — `codegen`, `parser`, `ive`,
`bd`, `proof`, `scg`, `cor`, `package`, `docs`, `ci`, `tests`. Keep the
summary under 72 characters. Reference issues in the body
(`Closes #123`, `Refs #456`), not the summary. Examples:

```
codegen: fix AArch64 shift encoding for imm=0
parser: recover from missing `;` after return expr
ive: cache interprocedural escape results per callsite
docs: rewrite building.md for VUMA 2.0 PMT-only
```

### Squash before merge

PRs are **squash-merged** — one commit per PR on `main`. Your branch history
is not preserved; make sure the squashed commit message follows the format
above. If your PR addresses multiple unrelated concerns, split it into
separate PRs first.

### What CI runs

Every PR targeting `main` runs:

- **Build** — `cargo build --workspace` (matrix: Ubuntu x86_64 + macOS aarch64).
- **Lint** — `cargo fmt --all -- --check` + `cargo clippy --workspace -- -D warnings`.
- **Unit tests** — `cargo test --workspace`.
- **Cross-compile** — builds for 8 targets (x86_64, aarch64, riscv64gc, armv7,
  mips64, powerpc64, loongarch64, wasm32).
- **Gold-standard** — `scripts/pi5_test_suite.sh --workers 8 --verify` across
  all 18 QEMU backends + `wasm32` under `wasmtime`.
- **Proof verify** — `bv_verify`, `proof_artifacts`, `proof_log` subsets.

All of them must be green before merge.

### Review expectations

- Reviewers will check that PMT tests use `layout` / `State` / `state_new`
  only — no pointer syntax (see [§3](#3-pmt-only-test-policy)).
- New backends must agree with the existing 18 on the full gold-standard
  suite (see [§4](#4-adding-a-new-backend)).
- Large architectural changes should be discussed in an issue first.
- The default answer to "can we add an external crate?" is "no, reimplement
  it" (see [§2 Zero external dependencies](#2-code-style)).

---

## Questions

- Bugs and feature requests: [GitHub Issues](https://github.com/pkhairkh/vuma/issues)
- Repository: <https://github.com/pkhairkh/vuma>
- License: MIT (see [`LICENSE`](../LICENSE))

When in doubt, match the surrounding code. The codebase is its own style guide.
