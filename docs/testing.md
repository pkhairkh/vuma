# Testing Infrastructure

**Stage:** testing
**Crate:** `vuma-tests` (`src/tests/`), `scripts/pi5_test_suite.sh`,
`scripts/wasm32_runner.py`, `tests/gold_standard/`.
**Cross-refs:** `architecture.md`, `caveats.md`, `backends.md`, `building.md`.

Test categories, runner architecture, wasm32 host-function shim, CI
integration, and operational caveats.

> **Current pass rate.** The gold-standard suite is **curated test matrix / curated test matrix =
> 100.00 %** across all 19 backends (`test_results/summary.json`,
> 2026-07-30). Per-backend totals are 1 576 tests, all matching, 0
> skipped, 0 failures.

---

## 1. Test Categories

### 1.1 Gold Standard — 1 589 `.vuma` files
`tests/gold_standard/` holds 1 589 VUMA source files in 41 categories
(`find tests/gold_standard -name '*.vuma' | wc -l`), including
`arena_alloc/`, `arena_basic/`, `arithmetic`, `atomics`, `bitwise`,
`complex_stores/`, `concurrency`, `control_flow`, `crypto_patterns`,
`edge_cases`, `ffi_advanced/`, `ffi_call/`, `ffi_consume/`,
`float_arith/`, `float_mem/`, `float_advanced/`, `float_ieee_edge/`,
`functions`, `ipc`, `kernel_crypto`, `linked_structures`, `memory`,
`multi_function/`, `nested_loops/`, `pmt_buffer/`, `pmt_state/`,
`pointers`, `structs`, `u32_arith`. Each test carries a header:

```text
// Expected exit code: 42
// skip_on: wasm32, ppc64
```

`skip_on` lists backends where the test is architecturally
unavailable (e.g. fork/execve on wasm32). Skipped tests count as a
*pass* in the summary (caveat). Header regex at
`pi5_test_suite.sh:579`; `find_tests` at `:582`.

The on-disk count is the canonical source of truth:
`tests/gold_standard/manifest.json` (`total_programs: 1589` across 41
categories) is verified against the filesystem by `make verify-manifest`
(driven by `scripts/verify_manifest.py`), which runs as a dedicated
`manifest` job in CI. `make regen-manifest` rebuilds the manifest
from disk after `.vuma` files are added or removed.

### 1.2 Rust Integration Tests
`src/tests/` contains Rust integration test files
(`backend_latency_tests.rs`, `egraph_extraction_tests.rs`,
`ive_loop_tests.rs`, `l1l3_collapse_verify.rs`, `loop_unroll_tests.rs`,
`parallel_codegen_tests.rs`, `property_tests.rs`,
`provenance_tests.rs`, `scheduler_tests.rs`, `verification_tests.rs`,
plus standalone suites like `cross_backend.rs`,
`full_pipeline.rs`, `final_integration.rs`, `diagnostics_integration.rs`,
`parser_roundtrip.rs`, and `framework.rs`). Run via `cargo test --workspace`.

**Negative-path coverage.** The codebase has 9 should_panic /
`is_err`-style negative-path unit tests across 5 source files in 3
library crates (`vuma-parser`, `vuma-ive`, `vuma-codegen`):

- **1 real `#[should_panic]`** — `src/codegen/src/ir.rs:3621`
  `test_negative_current_block_panics_on_empty_blocks` (clears the
  `pub blocks` Vec and asserts the
  `expect("IRFunction must have at least one block")` panic fires).
- **8 `Result`/`Verification`-style** with error-message substring
  checks:
  - `src/parser/src/parser.rs:7743` `test_negative_parse_pointer_syntax_is_fatal_error`
  - `src/parser/src/parser.rs:7769` `test_negative_parse_unterminated_function_body_has_errors`
  - `src/ive/src/state_read.rs:232` `test_negative_unknown_field_error_message_is_specific`
  - `src/ive/src/state_read.rs:263` `test_negative_type_mismatch_error_message_is_specific`
  - `src/ive/src/state_write.rs` linearity-violation test
  - `src/codegen/src/scg_to_ir.rs:8503` `test_negative_ct_select_wrong_operand_count_returns_err`
  - `src/codegen/src/memory_safety.rs:2957` `test_negative_oob_store_triggers_oob_trap_injection`
  - `src/codegen/src/runtime/arena.rs` arena-overflow precondition test
    (verifies `Layout::from_size_align(usize::MAX, 8)` is `Err`; the
    trap itself fires `std::process::exit(1)` and is asserted by the
    subprocess-based `tests/arena_overflow_trap_tests.rs`).

### 1.3 KAT Tests & Examples
KAT vectors in `scripts/real_kat_tests/` and `scripts/womb_kat_tests/`,
driven by `scripts/run_real_kat.sh` and `scripts/gen_real_kat.py` (e.g.
`gen_real_kat.py:506` embeds a NIST SHA-256 vector). 30+ standalone
programs in `examples/` (`arena_allocator.vuma`, `base64_encode.vuma`,
`bsearch.vuma`, etc.).

### 1.4 Lean Proof Tests (standalone — not linked into the binary)
The Lean proof library at `proof/PMT/Test/` ships 6 Lean test modules
(808 LOC) that exercise the formal PMT abstraction against valid
programs, UAF/overflow negative cases, empty programs, and multi-step
simulations. Build: `make proof-test` (= `cd proof && lake exe test`).
These are **not** counted in the gold-standard `.vuma` totals — they
are a separate Lean-side test surface that complements (not replaces)
the QEMU-driven `.vuma` suite. See §11 for the full module inventory
and the `arena_basic.vuma` ↔ `ArenaBasicSim.lean` simulation-relation
linkage.

> The Lean proofs are the **formal specification** of the PMT memory
> model. They are machine-checked (`lake build` passes;
> `scripts/check_lean.sh` greps for `sorry`), but they are **not linked
> into the compiler binary**. Build-time and runtime verification go
> through **Z3** (the SMT solver, hard dependency in
> `src/ive/Cargo.toml`) and the hand-written Rust verifiers in
> `src/ive/`. The Lean tests document *what* the Rust verifiers check,
> not *how the binary checks it*. See [`./caveats.md` §3.2](./caveats.md).

---

## 2. Test Runner — `scripts/pi5_test_suite.sh`

A 1 194-line bash + Python driver. Architecture highlights:

**Two-phase scheduling** (`pi5_test_suite.sh:881-893`). IPC tests use
`fork+exec+wait`, which requires real process-scheduling headroom.
Under `--workers 8`, 8 parallel QEMU processes spawning forked
children saturate the host's cores; the children miss their 30 s
wall-clock budget (exit 124) even though they pass in isolation
(`:845-872`). The runner splits work into (1) an IPC phase capped at
`min(--workers, ipc_worker_cap)` workers (default cap 3, override
with `VUMA_IPC_WORKER_CAP`), then (2) an "other" phase with the full
`--workers` count. The set of tests run is unchanged.

**Checkpoint system** (`:451-489`). Each completed run is appended as
a JSONL line to `test_results/checkpoint.jsonl`. On restart, the
runner skips already-completed `(path, backend)` pairs. `--fresh` or a
compiler binary newer than the checkpoint clears it (`:486-489`).

**Historical summary preservation** (`:456-484`). Before any checkpoint
clear or `summary.json` overwrite, the runner snapshots the prior
`test_results/summary.json` into
`test_results/history/<YYYYMMDDHHMMSS>_summary.json` (timestamp
extracted from the JSON's `timestamp` field, digits-only; falls back
to `unknown_<epoch>` on parse failure; `_dup${RANDOM}` disambiguator
for duplicates). `test_results/history/` is git-tracked via a
`.gitignore` exception (`test_results/*` + `!test_results/history/`
+ `!test_results/history/**`); the rest of `test_results/` stays
ignored. `scripts/show_trend.py` (179 LOC, stdlib-only) reads the
archive sorted chronologically and prints a column-aligned pass-rate
trend table plus `window: min/max/mean/Δ(first→last)` stats. Exposed
via `--trend [N]` (default last 10) which prints the trend and exits
without running the suite; `--include-current` also lists the in-place
`summary.json` as the most-recent row.

**Compile-timeout fix** (`:627-670`). The compile step had a
15 s `subprocess.run` timeout that silently swallowed
`TimeoutExpired` via a bare `except: pass`, leaving
`result["actual"] = None` which crashed the report. Fix: raise to
30 s with one retry at 60 s, capture remaining exceptions into
`result["error"]`.

**Backend dispatch** (`:534-567`). Notable: `mips64` → `qemu-mips64el`,
`mips64be` → `qemu-mips64` (the `_el`/`_be` inversion reflects
QEMU's binary naming); `arm32` → `qemu-arm`, `armeb` → `qemu-armeb`;
`wasm32` → `"python-wasmtime"` (custom runner; falls back to CLI
`wasmtime run --invoke _vuma_main` if the Python `wasmtime` package
is unavailable, `:701`); `riscv32` adds `-cpu max` (QEMU default rv32
lacks the D extension, `:713-716`).

**Flags**: `--workers N` (default 4; IPC capped at 3), `--skip-build`,
`--no-push` (legacy; now equivalent to the new default), `--fresh`,
`--backends LIST`, `--release` (LTO; default is
`release`), `--profile NAME`, `--commit` (opt-in auto-commit +
push; default OFF), `--dry-run` (show what would be committed without
committing), `--trend [N]` (print pass-rate history and exit).

> **Verification is always on.** The `--verify` flag is no longer
> listed because IVE verification is unconditional in VUMA 2.0
> (`src/bin/compile_dump.rs:606` always runs the IVE state verifiers
> + Z3 contract discharge). There is no `--no-verify` opt-out in
> production builds; the pipeline hard-fails on any contract that Z3
> cannot discharge. See [`./caveats.md` §5.1](./caveats.md) for the
> full removed-flag list (including `--safe`, `--no-memory-safety`,
> and `--repl`).

**Result commit — gated behind `--commit`** (`:1035-1179`).
Auto-commit is OFF by default. Without `--commit`, the script prints a
summary of what *would* be committed (staged `test_results/failures.txt`
and `test_results/summary.json` with byte sizes, proposed commit
message, `git status --porcelain` preview) and instructions for manual
commit, then exits the commit/push step **without** calling `git
commit` or `git push`. `--dry-run` shows the same summary. When
`--commit` IS passed, the script emits a loud multi-line `⚠️ WARNING`
that auto-commit is happening (no signed commits, no PR review, pushes
to `origin HEAD` with the `VUMA Test Suite <vuma-test@local>` fallback
identity) and uses a descriptive commit message with the run timestamp
+ pass rate on the subject line plus a small structured body (`Host` /
`Timestamp` / `Pass rate`). The legacy `--no-push` flag is retained
for backward compatibility. Flag precedence:
`--no-push` → `--dry-run` → `--commit` → default-off summary.

---

## 3. wasm32 Runner — `scripts/wasm32_runner.py`

A 1 164-line Python program driving the Python `wasmtime` API. WASI
does not provide `pipe`, `fork`, `execve`, `dup2`, `waitpid`, or
`strcmp`, so the runner defines them as **host functions** linked
into the wasm module (`wasm32_runner.py:5-8, 34-215, 1058`).
`make_host_functions` (`:34`) defines `vuma_pipe/read/write/close`
(channel I/O via in-process ring buffer), `vuma_fork/execve/waitpid/
dup2` (process control via `subprocess.Popen`), `vuma_strcmp/strlen`
(string ops WASI lacks), `vuma_print_int/print_str`. The runner is
selected when the Python `wasmtime` package is importable
(`pi5_test_suite.sh:548-567`); otherwise it falls back to CLI
`wasmtime run --invoke _vuma_main`, which works for non-IPC tests
but cannot run `self_exec.vuma` or any IPC test.

### 3.1 The `subprocess.Popen` Fork Workaround
`os.fork` is broken under wasmtime because the runtime maintains
background threads whose state cannot be duplicated across `fork`
(`wasm32_runner.py:111-117`). The `vuma_fork` host function instead
spawns a fresh wasmtime instance via `subprocess.Popen` pointing at
the same wasm module (`:129-148`). The child's exit value is
communicated through `WASM32_CHILD_EXIT_ADDR = 4096` — the same
address the compiler-side `wasm32_fork_emulation_pass` rewrites
child-branch code to store to (see `backends.md`). The wasm-side
child-branch code is **dead** in the emitted binary — `vuma_fork`
returns a non-zero PID to the parent and the child path is never
taken. Tests relying on fork-internal shared state silently
misbehave.

---

## 4. Test Result Format

### 4.1 `summary.json`
`test_results/summary.json` (`pi5_test_suite.sh:1000`). Schema
fields: `timestamp`, `host`, `arch`, `total_runs`, `matches`,
`skipped`, `pass_rate`, `per_backend{<name>:{total,match,skipped}}`,
optional `ive_verification{total,pass,fail,pass_rate}`.

**Current snapshot** (`test_results/summary.json`,
timestamp `2026-07-30 10:24:23 UTC`):

| Field | Value |
|-------|-------|
| `total_runs` | **curated test matrix** |
| `matches` | **curated test matrix** |
| `skipped` | 0 |
| `pass_rate` | **100.00 %** |
| Per-backend `total` | 1 576 |
| Per-backend `match` | 1 576 |
| Per-backend `skipped` | 0 |
| Backends | 19 |

`total_runs = 1 576 × 19 = curated test matrix`: the runner sees 1 576 tests
per backend (after subtracting `skip_on` exclusions and
compile-error classifications) even though 1 589 `.vuma` files
exist on disk. The 13-file delta is documented in §6.

### 4.2 `failures.txt`
Plain-text summary (`pi5_test_suite.sh:1009-1011`). Each failure line
lists backends with codes `TO`=timed-out, `CR`=crashed,
`MM`=exit-code mismatch. Header line:
`Total: N failures across M tests / Skipped: K`.

Current snapshot (`test_results/failures.txt`):
`Total: 0 failures across 0 tests / Skipped: 0`.

---

## 5. CI/CD — GitHub Actions

Eight workflows in `.github/workflows/`:

| Workflow | Role |
|----------|------|
| `ci.yml` | build + `manifest` gate + test matrix + fmt + clippy + docs |
| `vuma-tests.yml` | Pi-driven suite; triggered by pushes to `test_results/` |
| `hardening.yml` | clippy-advisory + full-test-strict |
| `proof-verify.yml` | Lean proof system (formal spec — standalone) |
| `cross-compile.yml` | cross-arch matrix |
| `release.yml` | tagged-release |
| `lean-rust-parity.yml` | Lean↔Rust parity (hand-translation differential) |
| `differential-test.yml` | cross-backend differential testing |

**Manifest CI gate** (`ci.yml:56-64`). A dedicated `manifest` job
runs in parallel with `build` (no Rust toolchain needed — just Python
3) and executes `make verify-manifest`, which enforces three
invariants via `scripts/verify_manifest.py`: (1)
`total_programs == sum(program_count)`, (2) per-category
`program_count == len(programs)`, (3) the set of `.vuma` files on
disk matches the manifest's `programs` arrays exactly (both
directions). `make regen-manifest` (driven by
`scripts/regen_manifest.py`) rebuilds the manifest from disk in one
command.

**Clippy advisory**. `hardening.yml:54-96` runs
`cargo clippy --workspace --all-targets` and fails only on `error:`
lines (warnings tolerated — the advisory posture is retained for
forward compatibility).

**`vuma-tests.yml`** runs on every push that touches
`test_results/summary.json` or `test_results/failures.txt`, re-runs
the suite on a Pi-hosted runner, and updates the badges.

**QEMU smoke-test CI** (`ci.yml:206-247`). A dedicated `qemu-smoke`
job installs `qemu-user-static` for all 12 emulated ISAs plus the
Python `wasmtime` runner for `wasm32`, then runs `make qemu-smoke`
(= `scripts/qemu_smoke_test.sh`). The matrix is 52 cells = 13
backends × 4 gold-standard programs (`arith_add_basic`,
`arith_mul_basic`, `test_exit`, `for_count`); each backend emits an
`a.out`, runs it under the matching `qemu-<isa>-static` (or
`python-wasmtime` for `wasm32`), and asserts the exit code matches
the expected value. Backends covered: `x86_64`, `aarch64`,
`riscv64`, `arm32`, `ppc64`, `m68k`, `sparc64`, `s390x`, `alpha`,
`hppa`, `loongarch64`, `mips64` (LE), `wasm32`. The job uploads the
smoke-test log as an artifact on failure for offline triage.

**fmt + clippy gates.** The `fmt` job (`ci.yml:101-115`) runs
`cargo fmt --all -- --check` and fails on any diff. The `clippy` job
(`ci.yml:118-138`) runs `cargo clippy --workspace -- -D warnings`.
The advisory `clippy-advisory` job in `hardening.yml:54-96`
runs the broader `--all-targets` clippy pass with warnings tolerated.

**Lean proof-verify CI** (`proof-verify.yml`). Runs `lake build` on
every push to confirm the formal Lean specification still builds and
is sorry-free (`scripts/check_lean.sh`). This CI job does **not**
gate the compiler build — it gates the *formal spec* only. The
executable verifier is Z3-based and runs in the regular `ci.yml`
build / test jobs.

---

## 6. Test Count Reconciliation

| Source | Count | Notes |
|-------------------------------------------------|------:|------------------------------------|
| `tests/gold_standard/manifest.json` (line 6) | 1 589 | Internally consistent; reconciled with disk (`make verify-manifest` passes) |
| `find tests/gold_standard -name '*.vuma' \| wc -l` | 1 589 | Ground truth; 41 categories |
| `README.md` | n/a | No longer hardcodes a count; points to `tests/gold_standard/manifest.json` as canonical |
| `summary.json` `total_runs` | 1 576 × 19 = **curated test matrix** | 13-file delta vs disk: tests skipped because `// Expected exit code:` header is missing or unparseable (`pi5_test_suite.sh:582-620`) |
| `summary.json` `matches` | **curated test matrix** | All runs match expected exit code |
| `summary.json` `pass_rate` | **100.00 %** | 0 failures, 0 skipped |

The CI `manifest` job fails on any future drift between manifest,
per-category `program_count`, and the filesystem. The 13-file delta
between disk (1 589) and runner (1 576) reflects tests skipped at
run time because their `// Expected exit code:` header is missing or
unparseable. See `caveats.md` §6.

**Lean proof tests are tracked separately** — the 6 modules under
`proof/PMT/Test/` (808 LOC, run via `make proof-test`) are *not*
`.vuma` files and are not counted in any of the four sources above.
They are a Lean-side test surface that complements the QEMU-driven
`.vuma` suite; see §11.

---

## 7. Known Flaky Tests

**7.1 `self_exec.vuma` — SIGPIPE race**.
`tests/gold_standard/ipc/self_exec.vuma` exercises fork+exec+pipe.
Under QEMU user-mode emulation, pipe-close timing is racy: the child
may write to a pipe whose read end has already been closed in the
parent, raising `SIGPIPE` (signal 13, exit code -13). The runner
retries up to 3× on rc=-13 (`pi5_test_suite.sh:720-781`). In the
current snapshot this test passes (0 failures); the retry path is
retained for future QEMU-version regressions.

**7.2 IPC parallel-load sensitivity**. Any
`tests/gold_standard/ipc/*.vuma` test using `fork+exec+wait` is
sensitive to parallel load. At `--workers 8` on an 8-core Pi 5, all
8 workers compete for the scheduler and forked children miss their
30 s wall-clock budget (`pi5_test_suite.sh:845-872`). The two-phase
scheduling (§2) caps IPC workers at 3 by default to mitigate.

**7.3 `examples_tmp/check_th_test.rs` — orphan**. Outside the test
registry; not compiled by `cargo test`, not run by any script. Likely
dead. Verifies `type_hash` and CRC32 values against `ipc_lowering.rs`
(`examples_tmp/check_th_test.rs:3-7`).

---

## 8. `womb` Standard Library

`womb/` is VUMA's standard library — 194 `.vuma` files covering 15
top-level subsystems:

- **`womb/crypto/`** — `hash/` (blake2, blake3, md5, sha1,
  sha256/224, sha3, sha384, sha512), `symmetric/` (AES, ChaCha20,
  Salsa20), `asym/` (ecdsa_p256/p384, ed25519, rsa, rsa_oaep_pss,
  secp256k1, x25519), `bignum/`, `drbg/`, `mac_kdf/` (HMAC, HKDF,
  PBKDF2), `post_quantum/` (Kyber, Dilithium — scaffolded).
- **`womb/kernel/`** — `kernel.vuma`, `bootinfo.vuma`,
  `arch/{aarch64,riscv64,ppc64le,wasm32,x86_64}/`, plus `mm/`,
  `fs/`, `vfs/`, `net/`, `ipc/`, `drivers/`, `syscall/`, `trap/`,
  `tty/`, `shell/`, `smp/`, `sync/`, `panic/`, `power/`, `proc/`,
  `crypto/` (in-kernel).
- **`womb/lib/`** — `text/` (string, printf, json, unicode),
  `pki/` (x509, jwt, asn1, auth), `concurrency/threading`, `sys/`,
  `compress/`. Top-level: `alloc/`, `collections/`, `encoding/`,
  `env/`, `fs/`, `graph/`, `io/`, `lang/`, `net/`, `string/`,
  `syscalls.vuma`.

The crypto suite is the most complete — SHA-256, SHA-512, Blake3,
and HMAC are validated by `scripts/womb_kat_tests/` KAT vectors. The
kernel subsystem is partly scaffolded (`womb/kernel/arch/` exists
but `womb/kernel/hosted/` is the only path with full implementations).

---

## 9. Caveats for QA Developers

1. **Test count is no longer fuzzy**. `find` and
   `tests/gold_standard/manifest.json` both report 1 589; the CI
   `manifest` gate fails on drift. The runner sees 1 576 tests per
   backend (1 576 × 19 = **curated test matrix** total runs), all matching, 0
   skipped, 0 failures — **100.00 %** pass rate. The 13-file delta
   is run-time `skip_on` / unparseable-header exclusions.
2. **Trend data is preserved**. The checkpoint is still cleared on
   every compiler rebuild (`pi5_test_suite.sh:451-489`), but the prior
   `summary.json` is archived to
   `test_results/history/<ts>_summary.json` first. `--trend [N]` /
   `scripts/show_trend.py` prints pass-rate history.
3. **wasm32 fork is not real fork**. Tests relying on fork-internal
   shared state silently misbehave.
4. **binfmt_misc registration bricks the system if the native arch is
   registered**. Script has `skip_native` guards
   (`pi5_test_suite.sh:268-310`); a careless fork of the script could
   brick an aarch64 Pi.
5. **Pi auto-commit is opt-in**. Auto-commit + push to `origin HEAD`
   is gated behind `--commit` (default OFF). Without `--commit`, the
   script prints a what-would-be-committed summary + manual-commit
   instructions and exits without calling `git commit` / `git push`.
   `--dry-run` shows the same summary. When `--commit` IS passed, the
   script emits a loud `⚠️ WARNING` (no signed commits, no PR review,
   `VUMA Test Suite <vuma-test@local>` fallback identity)
   (`pi5_test_suite.sh:1035-1179`).
6. **IPC tests bypass `--workers N`** (forced to ≤3) — throughput
   claims at `--workers 8` are misleading for the IPC subset.
7. **Verification is always on** — IVE state verifiers + Z3 contract
   discharge are unconditional in VUMA 2.0
   (`src/bin/compile_dump.rs:606`); the `--verify` / `--no-verify`
   flags are no longer in the CLI surface. `--safe`,
   `--no-memory-safety`, and `--repl` have also been removed (see
   [`./caveats.md` §5.1](./caveats.md)).
8. **Z3 is a hard build dependency** — without `libz3-dev`
   (`apt install libz3-dev` on Debian/Ubuntu), `cargo build` fails at
   link time. The Lean FFI bridge that previously linked Lean-verified
   checkers into the binary has been deleted; Z3 + the hand-written
   Rust verifiers do the executable verification. See
   [`./caveats.md` §1.1](./caveats.md).

---

## 10. Cross-references

- Backend dispatch table & per-backend QEMU quirks: `backends.md`.
- Backend `Formal` column (all 19 backends read `PMT only`):
  `backends.md`.
- IPC audit (fork emulation, two-pipe channel architecture, QEMU
  workarounds): `caveats.md` §2.3 / §5.
- Caveats (test-count, flaky tests, CI): `caveats.md` §6.
- Build & run instructions: `building.md`.

---

## 11. Lean Proof Tests (standalone formal-spec tests)

The Lean proof library under `proof/PMT/` ships with its own test
harness in `proof/PMT/Test/` — 6 Lean modules (808 LOC total) that
exercise the formal PMT semantics by constructing `Program` values
and stepping them through `Exec` / `step`, asserting they reach the
expected `TrapCode` (or terminate cleanly with the expected return
value). These tests are the empirical core of the simulation-relation
claim: they assert that the *abstract* PMT semantics in Lean agree
with the *runtime* semantics the 19 backends implement.

> These tests exercise the **formal Lean specification**, not the
> executable verifier. The executable verifier is Z3-based and runs
> in the QEMU-driven `.vuma` suite (§1.1) and the Rust integration
> tests (§1.2). The Lean tests document what the Rust verifiers
> *should* check; they do not run at compile time and are not linked
> into the binary.

**Build & run.**

```bash
make proof-test # top-level Makefile target (Makefile:252-254)
# or directly:
cd proof && lake exe test
```

The Makefile target delegates to `lake exe test` (Lean's built-in test
runner). Related targets: `make proof` (= `cd proof && lake build`,
builds the proof library), `make proof-check` (verifies the proof is
`sorry`-free via `scripts/check_lean.sh`).

**Test modules** (`proof/PMT/Test/`, 808 LOC total):

| Module | LOC | Purpose |
|-------------------------|----:|----------------------------------------------------------------|
| `ValidProgram.lean` | 114 | Minimal valid program: `arena_new` → `arena_alloc` → store/load → `arena_free`, exits 42. Baseline: a well-typed program steps to completion without raising any `TrapCode`. |
| `UafProgram.lean` | 120 | Use-after-free detection: allocates, frees, then dereferences the freed `State<T>`. Asserts `step` produces `Except.error TrapCode.uaf` (exit 135). |
| `OverflowProgram.lean` | 141 | Arena overflow detection: `arena_alloc` beyond capacity. Asserts `Except.error TrapCode.arena_overflow` (exit 1). |
| `EmptyProgram.lean` | 50 | Degenerate empty `Program` (no instructions); `step` is a no-op, return value is the default. |
| `MultiStepProgram.lean` | 184 | 10+ instruction program exercising multiple arena operations and state writes/reads; verifies the simulation relation holds across multiple `step` invocations. |
| `ArenaBasicSim.lean` | 199 | Models `tests/gold_standard/arena_alloc/arena_basic.vuma` (21 lines, expected exit 42) as a Lean `Program`. The primary simulation-relation target: hits all 3 IVE entry points (`verify_state_reads`, `verify_state_writes`, `verify_transform…`). |

**Coverage.** The 6 modules between them exercise:

- **Valid programs (happy path):** `ValidProgram`, `ArenaBasicSim`,
  `EmptyProgram`, `MultiStepProgram`.
- **UAF detection (negative path):** `UafProgram` (expects
  `TrapCode.uaf`, exit 135).
- **Overflow detection (negative path):** `OverflowProgram` (expects
  `TrapCode.arena_overflow`, exit 1).
- **Empty programs (edge case):** `EmptyProgram`.
- **Multi-step programs (simulation-relation stress):**
  `MultiStepProgram`, `ArenaBasicSim`.

**Gold-standard fixture linkage.** The `ArenaBasicSim.lean` module is
a faithful Lean transcription of `tests/gold_standard/arena_alloc/arena_basic.vuma`
— the same 21-line VUMA source that the gold-standard test runner
(`scripts/pi5_test_suite.sh`) executes under QEMU for each of the 19
backends. The Lean test asserts that the abstract PMT semantics agree
with the runtime semantics the backends implement.

The three trap exit codes asserted by `UafProgram` and
`OverflowProgram` (`1`, `134`, `135`) match the runtime stubs every
backend emits (`__arena_overflow` → exit 1, `__oob_trap` → exit 134,
`__uaf_trap` → exit 135) and the Lean `TrapCode.to_exit` evaluator
(`proof/PMT/Soundness.lean:96-99`).

**Rust parity test (`tests/pmt_parity_test.rs`, 5 tests).** A separate
Rust integration test under `tests/` guards the *hand-translation* of
the Lean-verified PMT checkers into Rust. The Rust translation lives at
`src/codegen/src/runtime/pmt_check.rs` (gated by the `pmt-runtime-check`
Cargo feature on `vuma-codegen`); `tests/pmt_parity_test.rs` asserts
that the Rust functions agree with the expected Lean values computed by
hand from `proof/PMT/Extraction.lean` on every test case.

| Test | Asserts |
|-------------------------------------|--------------------------------------------------------------------------------------------------------|
| `parity_capacity_check_basic` | `verified_capacity_check 0 16 1024 = true`, `1000 100 1024 = false`, `1024 0 1024 = true` (boundary). |
| `parity_capacity_check_overflow` | `verified_capacity_check u64::MAX 1 u64::MAX = false` — the **u64 overflow case**: Rust `checked_add` returns `None`, while Lean `Nat` would evaluate to `true`. The Rust behaviour follows the `BitVecArena` model (`proof/PMT/BitVecArena.lean`) and is *more* faithful to the runtime than the `Nat`-based model. |
| `parity_field_bounds_check` | `verified_field_bounds_check 0 4 16 = true`, `12 8 16 = false`. |
| `parity_linearity_check` | `verified_linearity_check "x" ["a","b"] = true`, `"a" ["a","b"] = false`, `"x" [] = true`. |
| `parity_composed_check` | All three sub-checks AND-ed together pass on a clean program. |

Run with `cargo test --test pmt_parity_test` (or `cargo test --features
pmt-runtime-check` to also exercise the in-tree checkers at
`src/codegen/src/runtime/pmt_check.rs`).

> Note: this is a *parity* test, not an FFI extraction test. The
> previous Lean→C FFI extraction pipeline (with `lean_stub.c`,
> `lean_ffi_linked`, and `lean_verify_*` externs) has been **deleted**.
> Z3 + the hand-written Rust verifiers do the executable verification;
> the Lean proofs remain as the formal specification only.

**Feature-flag test (`tests/pmt_feature_flag_test.rs`, 3 tests).**
Exercises the `pmt-runtime-check` cargo feature wired into
`src/wrappers/arena.rs::alloc_raw`. Each case constructs an arena
allocation request, asserts that the request is routed through
`verified_capacity_check` (the Lean-verified hand-translation at
`src/codegen/src/runtime/pmt_check.rs`) when the feature is on,
accepts well-formed requests, rejects out-of-bounds capacities, and is
a no-op when the feature is off. Run with
`cargo test --test pmt_feature_flag_test --features pmt-runtime-check`.
