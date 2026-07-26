# Building and Development Guide

## Prerequisites

### Rust Toolchain

VUMA requires Rust nightly pinned to `nightly-2026-03-01`:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly
rustup toolchain install nightly-2026-03-01 --profile default
```

The `rust-toolchain.toml` file enforces this automatically.

### QEMU User-Mode Emulation

For cross-architecture testing, install the **14** QEMU static binaries
(v7.2.0-1, matching the version the compiler's QEMU-quirk workarounds target):

```bash
for arch in aarch64 arm x86_64 mips64 mipsel ppc64 ppc64le riscv64 \
 s390x sparc64 m68k hppa alpha loongarch64; do
 curl -sL "https://github.com/multiarch/qemu-user-static/releases/download/v7.2.0-1/qemu-${arch}-static" -o ~/.local/bin/qemu-${arch}-static
 chmod +x ~/.local/bin/qemu-${arch}-static
done
```

Then install the additional `qemu-mips64el-static` binary required by the
`--isa mips64` backend (VUMA emits a **little-endian** MIPS64 ELF — see
note below):

```bash
curl -sL "https://github.com/multiarch/qemu-user-static/releases/download/v7.2.0-1/qemu-mips64el-static" -o ~/.local/bin/qemu-mips64el-static
chmod +x ~/.local/bin/qemu-mips64el-static
```

The 14 arches above plus `mips64el` give the 15-binary set the
environment-setup script installed and the set the QEMU smoke tests
(-a/b/c/d, re-run in the QEMU smoke rerun) exercise. Note that VUMA's
`--isa mips64` emits a
**little-endian** MIPS64 ELF, so it must be run under
`qemu-mips64el-static` (NOT `qemu-mips64-static`, which is big-endian).

> **Note:** QEMU 7.2.0-1 is the latest static release. Several QEMU bugs are
> worked around in the compiler — see
> [Caveats](caveats.md#5-ipc) for details.

### Wasmtime (for wasm32 backend)

```bash
# The test suite installs this automatically
WASMTIME_VER=v47.0.2
curl -L "https://github.com/bytecodealliance/wasmtime/releases/download/${WASMTIME_VER}/wasmtime-${WASMTIME_VER}-aarch64-linux.tar.xz" | tar xJ
cp wasmtime-*/wasmtime ~/.local/bin/
pip install wasmtime
```

Wasmtime **47.0.2** is the pinned version. Older releases
(e.g. v29.x) are NOT compatible with the wasm32 modules VUMA emits.

## Building

### Release (fast)

```bash
cargo build --profile release-fast --bin compile_dump
```

The `release-fast` profile enables LTO and optimized codegen. Build time:
~50-90s on a Pi 5.

### Debug

```bash
cargo build --bin vuma
```

### Build Profiles

| Profile | Use Case | LTO | Opt Level |
|---------|----------|-----|-----------|
| `dev` (default) | Development | off | 0 |
| `release-fast` | Production / testing | thin | 3 |
| `release` | Not used | fat | 3 |

## Lean Proofs (Formal Verification)

VUMA's PMT memory model is formally verified in Lean 4. The proofs live in
`proof/` and are built with [Lake](https://github.com/leanprover/lake).

### Prerequisites

Install [elan](https://github.com/leanprover/elan) (Lean toolchain manager,
analogous to `rustup`):

```bash
curl -sSf https://raw.githubusercontent.com/leanprover/elan/master/elan-init.sh | sh -s -- -y
source $HOME/.elan/env
```

The `proof/lean-toolchain` file pins Lean to `leanprover/lean4:v4.21.0`;
elan picks it up automatically when you run any `lake` command in `proof/`.

Verify:
```bash
lean --version # Lean (version 4.21.0, ...)
lake --version # Lake (version 4.21.0, ...)
```

### Building

```bash
# Via Make (recommended)
make proof # = cd proof && lake build
make proof-check # = ./scripts/check-lean.sh
make proof-test # = cd proof && lake exe test
make proof-clean # = rm -rf proof/.lake proof/build

# Via just
just proof
just proof-check
just proof-test

# Directly via Lake
cd proof && lake build && lake exe test
```

### Strict vs Non-Strict Sorry Check

`scripts/check-lean.sh` has two modes:

- **Non-strict** (default): allows sorries that are documented TODOs. Exits 0.
- **Strict** (`PROOF_CHECK_STRICT=1`): fails on ANY sorry. Use in CI gates.

```bash
./scripts/check-lean.sh # non-strict (allows documented TODOs)
PROOF_CHECK_STRICT=1 ./scripts/check-lean.sh # strict (fails on any sorry)
```

The proof library has been **sorry-free since ** (the last `sorry` in
`full_simulation_strong` was closed by an empty-`live_vars` UAF-trap argument).
Strict CI mode (`PROOF_CHECK_STRICT=1`) was enabled in and ran
continuously through; subsequently added a small number of
*deliberately documented* hard-proof `sorry` stubs as a depth-over-tidiness
trade (see `docs/proof/S2-W32-status.md` for the current count
and `S2-W1-E-soundness-conclusion.md` for per-stub strategies), and the
`lean-proofs` CI job now runs `check-lean.sh` in advisory mode
(`PROOF_CHECK_STRICT=0` exported) so the documented stubs do not block CI
while still surfacing any *undocumented* `sorry` token in the build log. For
local strict enforcement:

```bash
PROOF_CHECK_STRICT=1 ./scripts/check-lean.sh # fails on any sorry token
```

### Troubleshooting

- **`lake: command not found`**: Install elan (see Prerequisites).
- **`lean: command not found`**: Same as above.
- **`PMT_Soundness.lean: error: unknown identifier`**: Run `lake build` from
 `proof/`, not the repo root.
- **`sorry` warning**: Run `./scripts/check-lean.sh` to identify the line.

### `pmt-runtime-check` Cargo Feature (Lean↔Rust Runtime Checkers)

The hand-translated Rust versions of the Lean-verified PMT checkers
(`verified_capacity_check`, `verified_field_bounds_check`,
`verified_linearity_check`, `verified_pmt_check`) live at
`src/codegen/src/runtime/pmt_check.rs`. They are **gated behind the
`pmt-runtime-check` Cargo feature** (declared in `src/codegen/Cargo.toml`
and **forwarded from the root `Cargo.toml`** via
`pmt-runtime-check = ["vuma-codegen/pmt-runtime-check"]`), so they are
NOT compiled into the default build — downstream consumers opt in
incrementally. **The feature is now FUNCTIONAL** (not a stub): the
checkers are wired into `src/codegen/src/runtime/arena.rs`, so when the
feature is enabled the verified `verified_capacity_check` runs on every
arena allocation in production. `@[export]` attributes on
`proof/PMT/Extraction.lean` (`lean_verified_capacity_check`,
`lean_verified_field_bounds_check`, `lean_verified_linearity_check`,
`lean_verified_pmt_check`) emit the C symbols used by the FFI bridge.

Enable the feature when you want to exercise the verified checkers
locally (e.g. to run the parity test against the Lean definitions, or
to enable the runtime checkers in production builds):

```bash
# Build with the verified Lean-translated PMT checkers compiled in
# (works from the repo root — Cargo.toml forwards the feature).
cargo build --features pmt-runtime-check

# Run the parity test (5 tests, passes):
# tests/pmt_parity_test.rs — confirms the Rust hand-translations of the
# Lean `Extraction.lean` checkers match the expected Lean behavior on
# every test case.
cargo test --features pmt-runtime-check --test pmt_parity_test

# Run the feature-flag wiring test (3 tests, passes):
# tests/pmt_feature_flag_test.rs — confirms the feature flag compiles,
# the checkers are callable from the codegen crate, and the arena
# overflow check uses the verified path.
cargo test --features pmt-runtime-check --test pmt_feature_flag_test
```

The Lean source of truth lives at `proof/PMT/Extraction.lean` (with
extraction lemmas in `proof/PMT/ExtractionLemmas.lean`); the
`proof/extracted/pmt_check.rs` artifact is a snapshot of the
hand-translation for cross-reference. See
[README ](../README.md#5-formal-verification-lean-4) and
[docs/caveats.md](architecture/pmt-fix-proposals.md)
Stage 8 for the full Lean↔Rust integration narrative.

## Compiler CLI Flags 

The following global flags are accepted by `vuma build` / `vuma compile` /
`vuma emit` / `vuma run` / `vuma check` / `vuma verify` (before or after the
subcommand):

| Flag | Default | Meaning |
|------|---------|---------|
| `--safe` | ON (mandatory) | Enables runtime bounds-check IR injection + liveness (UAF) trap injection. **Cannot be disabled** since -a (`main.rs:607` hard-codes `safe: true`); accepted for backwards-compat. `--no-memory-safety` is rejected. |
| `--strict-ive` | OFF | Promote the `bv_verify` e-graph soundness advisory IVE verifier (Stage 7a) to HARD-FAIL on any unsound rule. The linear-channel verifier is **always** HARD-FAIL as of -a (independent of this flag). |
| `--max-expr-depth <N>` | `1024` | Maximum expression nesting depth. Raised from 256 in -b to accommodate machine-generated bignum-KAT programs. `0` is rejected. Flows through `CompileConfig::max_expr_depth` → `Parser::with_max_depth` at every parser entry point. |
| `--allow-inconclusive` | OFF | Soft-pass on `OverallVerdict::Inconclusive` from the PMT IVE verifiers (logs a `SOUNDNESS WAIVER` when `VUMA_LOG=1`). Without this flag, `Inconclusive` is a HARD compile error (-f). |
| `--opt-level <O0\|O1\|O2\|O3>` | `O3` | Codegen optimization level. |
| `--isa <isa>` | `aarch64` | Target backend. `aarch64` uses the canonical pipeline (IVE + telemetry); all other ISAs use the direct AST→codegen path (-a). |

Run `vuma --help` for the full list.

## Running Tests

### Full Test Suite

```bash
bash scripts/pi5_test_suite.sh --workers 4 --fresh --verify
```

Flags:
- `--workers N` — parallel workers for non-IPC tests (default 4)
- `--fresh` — clear checkpoint, run all tests from scratch
- `--verify` — enable IVE verification (always on in production)
- `--commit` — **opt-in** auto-commit + push of test results to `origin HEAD`
 (default OFF since -c). Without `--commit`, the script prints a
 summary of what *would* be committed (staged files + byte sizes + proposed
 commit message + `git status --porcelain` preview) and manual-commit
 instructions, then exits the commit/push step WITHOUT calling `git commit`
 or `git push`.
- `--dry-run` — show what would be committed without committing (-c).
 Flag precedence: `--no-push` → `--dry-run` → `--commit` → default-off summary.
- `--trend [N]` — print a pass-rate trend table over the last N archived runs
 (default 10) from `test_results/history/` and exit without running the suite
 (-c). Includes min/max/mean/Δ stats. No-op on first run (history empty).
- `--trend-n N` — override the `--trend` default of 10.
- `--skip-build` — skip the compiler rebuild step.
- `--no-push` — legacy flag, equivalent to the new default (no commit/push).

#### `VUMA_IPC_WORKER_CAP` environment variable (-e)

IPC tests (fork+exec+wait) always run with a reduced worker count regardless
of `--workers` to avoid QEMU translation-cache warm-up latency and
pipe-buffer contention. The cap defaults to **3** workers and is now
**configurable via the `VUMA_IPC_WORKER_CAP` environment variable**:

```bash
# Default (3 workers for IPC phase)
bash scripts/pi5_test_suite.sh --workers 8

# CI host with ≥16 cores and no QEMU contention:
VUMA_IPC_WORKER_CAP=8 bash scripts/pi5_test_suite.sh --workers 8
```

Invalid/non-integer values fall back to 3; values `<1` are floored to 1. The
override is logged to stdout (`[K11C] VUMA_IPC_WORKER_CAP=N …`).

> **IPC Phase:** IPC tests always run with `≤VUMA_IPC_WORKER_CAP` workers
> (default 3) regardless of `--workers` to avoid fork+exec timeouts. See
> [Testing Overview](testing/overview.md) and
> [IPC Audit](caveats.md) item 2.

### Individual Tests

```bash
# Compile a test
./target/release-fast/compile_dump tests/gold_standard/ipc/simple_send.vuma /tmp/out.bin aarch64 --opt-level=O3

# Run natively (x86_64)
chmod +x /tmp/out.bin && /tmp/out.bin

# Run under QEMU
~/.local/bin/qemu-aarch64-static /tmp/out.bin

# Run wasm32
./target/release-fast/compile_dump tests/gold_standard/ipc/simple_send.vuma /tmp/out.wasm wasm32 --opt-level=O3
python3 scripts/wasm32_runner.py /tmp/out.wasm
```

### Test Exit Codes

Each `.vuma` test file has an `// Expected exit code: N` header. The test
passes if the program exits with code N. See
[Testing Overview](testing/overview.md) for details.

### QEMU Smoke Test (all 13 backends)

The QEMU smoke-test matrix (`scripts/qemu_smoke_test.sh`, last re-run as
-qemu-smoke) builds the release `vuma` binary once and then
compiles a small set of gold-standard `.vuma` programs on every supported
backend (12 QEMU + wasm32 via wasmtime = **13 backends**, 52 program×backend
pairs), running each under the appropriate emulator and checking the exit
code against the `// Expected exit code:` header.

```bash
# Via Make (recommended):
make qemu-smoke

# Or invoke the script directly:
bash scripts/qemu_smoke_test.sh
```

Useful environment-variable overrides (the script honours all of them):

- `VUMA_SMOKE_ISAS="x86_64 aarch64 wasm32"` — restrict to a subset of backends.
- `VUMA_SMOKE_TESTS="arithmetic/arith_add_basic arithmetic/test_exit"` — restrict to a subset of tests.
- `VUMA_SMOKE_NO_BUILD=1` — skip the `cargo build --release --bin vuma` step (assumes a previously-built binary at `target/release/vuma`).

The script maps `arm32 → qemu-arm-static` and `mips64 → qemu-mips64el-static`
(the only two ISAs whose QEMU binary name doesn't directly match the VUMA
ISA name); every other ISA uses `qemu-<isa>-static`. `wasm32` is routed
through `wasmtime`. Exit status is 0 iff every (backend, test) pair passes.

> **Prerequisites:** the smoke test requires all 15 QEMU static binaries
> listed in [QEMU User-Mode Emulation](#qemu-user-mode-emulation) above
> (including `qemu-mips64el-static` for the `mips64` LE backend) plus
> `wasmtime` 47.0.2 on `$PATH` for the `wasm32` row.

## Development Workflow

### Adding a Test

1. Create `tests/gold_standard/<category>/<name>.vuma`
2. Add `// Expected exit code: N` header
3. Run the test on all backends:
 ```bash
 for b in aarch64 x86_64 hppa wasm32; do
 ./target/release-fast/compile_dump tests/gold_standard/<category>/<name>.vuma /tmp/t.bin $b --opt-level=O3
 chmod +x /tmp/t.bin
 # run it...
 done
 ```

### Debugging a Failing Backend

1. Compile with strace:
 ```bash
 ~/.local/bin/qemu-<arch>-static -strace /tmp/out.bin
 ```

2. Compile with instruction trace:
 ```bash
 ~/.local/bin/qemu-<arch>-static -d in_asm /tmp/out.bin 2>/tmp/trace.log
 ```

3. Check the backend's known quirks in
 [Backend Matrix](backends/matrix.md) and
 [Caveats](caveats.md).

### Useful Binaries

| Binary | Purpose |
|--------|---------|
| `vuma` | Main compiler CLI |
| `compile_dump` | Compile + dump IR/ELF (used by test suite) |
| `dump_ir` | Dump IR for a .vuma file |
| `scg_dump` | Dump SCG for a .vuma file |

## Project Structure

```
vuma/
├── src/
│ ├── parser/ # Lexer + parser + AST
│ ├── scg/ # Structured Call Graph
│ ├── ive/ # Intermediate Verification Engine
│ ├── codegen/ # IR + optimizer + 19 backends
│ ├── cor/ # Continuous Optimization Runtime
│ ├── bd/ # Behavioral Descriptors
│ ├── proof/ # Proof system
│ ├── vuma/ # CLI + REPL
│ ├── pipeline.rs # Compilation pipeline
│ └── main.rs # Entry point
├── tests/ # Gold standard test programs
├── scripts/ # Test runner + wasm32 runner
├── womb/ # Standard library
├── examples/ # Example programs
└── docs/ # This documentation
```

See [Architecture Overview](architecture/overview.md) for details.
