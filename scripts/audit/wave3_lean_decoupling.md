# Wave 3 — Lean proofs decoupling audit (caveat §3.2)

- **Task ID:** 3-d-audit
- **Agent:** 3-d-audit (sub-agent, wave 3)
- **Wave:** 3 (depends on wave 0 / 1 / 2 / 3-a-test / 3-b-audit / 3-c-test)
- **Caveat addressed:** §3.2 — *"The proofs are no longer linked into the compiler binary. Build, link, and runtime verification now go through Z3 and the hand-written Rust verifiers. The Lean proofs are documentation/specification only."*
- **Files in scope (test execution; no source edits):**
  - `/home/z/my-project/vuma/proof/` (entire directory — temporarily relocated)
  - `/home/z/my-project/vuma/Cargo.toml` (read-only verification)
  - `/home/z/my-project/vuma/build.rs` (root; read-only verification — the protocol referenced `src/ive/build.rs`, but no such file exists; the only `build.rs` in the workspace is the root one)
- **DoD:**
  - `cargo build --release --workspace` exits 0 WITHOUT the `proof/` directory present.
  - `proof/` directory restored to its original location after the test.
  - `git status` shows no changes (no diff to `proof/` or any source).
  - This summary markdown exists.

## Verification protocol

1. Read `/home/z/my-project/worklog.md` last 3 sections (waves 3-a-test → 3-b-audit → 3-c-test). HEAD before this task: `5a18ac86a9cd04ce477256eb1bee67ab380cfb70` (branch `main`).
2. Confirmed `proof/` exists with **11 top-level entries** (`PMT/`, `Pmt/`, `Test/`, `extracted/`, `PMT.lean`, `Pmt.lean`, `check_pmt.lean`, `lakefile.toml`, `lake-manifest.json`, `lean-toolchain`, `README.md`) — Lean source tree is genuine.
3. Pre-audit static check that NOTHING in the workspace build pipeline references `proof/` at compile time:
   - `Cargo.toml` (root): `rg "proof/"` → **0 matches** (no path dependency, no `include!`, no `[workspace]` member pointing at `proof/`).
   - `build.rs` (root; the only `build.rs` in the workspace — `src/ive/build.rs` does NOT exist): `rg "proof/"` → matches only on **lines 20 and 42**, both inside the file-level doc comment (`//!`); the actual `fn main()` body (lines 53-69) emits only `cargo::rustc-check-cfg=cfg(lean_ffi_linked)` and `cargo:rerun-if-changed=build.rs`. No `Path::new("proof/...")`, no `cc::Build`, no `include!`, no `cargo:rustc-link-lib`.
   - `src/**/*.rs`: `rg "proof/"` → 5 matches, **all in `//!` / `//` comments** (`arena_bounds.rs:23`, `verification.rs:962`, `verification.rs:982`, `runtime/arena.rs:181`, `runtime/pmt_check.rs:3` & `:19`). No `include!` / `include_str!` / `include_bytes!` referencing `proof/`.
   - `tests/**/*.rs`: `rg "proof/"` → many comment matches + exactly ONE code-level reference: `tests/pmt_extraction_diff.rs:69` has `#[path = "../proof/extracted/pmt_check.rs"]`. This is a **test target**, not built by `cargo build --workspace` (which compiles only library + binary targets). The reference is harmless for this DoD.
4. Temporarily moved `proof/` → `/tmp/proof_backup_wave3d`:
   ```
   mv /home/z/my-project/vuma/proof /tmp/proof_backup_wave3d
   ```
   Verified `ls /home/z/my-project/vuma/proof` → "No such file or directory" (directory genuinely absent during the build).
5. Ran `cargo build --release --workspace` with environment shims sourced:
   ```
   cargo build --release --workspace 2>&1 | tee /home/z/my-project/scripts/logs/wave3_no_proof_build.log
   ```
6. Captured exit code: **EXIT_CODE=0**. Log final line:
   ```
   Finished `release` profile [optimized] target(s) in 27.23s
   ```
   (Incremental on top of a partial build from a prior killed invocation; the prior invocation had already compiled through `vuma-tests` without error before being terminated by the agent tool timeout. The follow-up invocation completed in 27.23s with exit 0.)
7. Restored `proof/`:
   ```
   mv /tmp/proof_backup_wave3d /home/z/my-project/vuma/proof
   ```
   Verified `ls /home/z/my-project/vuma/proof` returns the same 11 top-level entries as before the move.
8. `git status --short` → **empty output** (no diff to `proof/` or any source file). HEAD unchanged at `5a18ac86a9cd04ce477256eb1bee67ab380cfb70`.

## Build log excerpt

```
$ cargo build --release --workspace
    Blocking waiting for file lock on artifact directory
    Finished `release` profile [optimized] target(s) in 27.23s
EXIT_CODE=0
```

Full log: `scripts/logs/wave3_no_proof_build.log`.

## Why the build still succeeds without `proof/`

Per the file-level doc on `build.rs` (lines 6-51) and the `Cargo.toml` comment block (lines 67-75, 99-113):

- The "Lean FFI bridge" (the `lake build → lean --emit-c → cc::Build → cargo:rustc-link-lib` pipeline) was **deleted** in an earlier wave.
- `build.rs::main()` no longer compiles any C, no longer links any Lean archive, and no longer sets `cargo:rustc-link-lib=...`. It only (a) detects the rustc version and (b) declares the `lean_ffi_linked` cfg via `cargo::rustc-check-cfg` so `tests/*` files that reference it stay lint-clean (the cfg is **never emitted**, so those branches consistently take the `not(lean_ffi_linked)` path).
- The `cc` build-dependency is retained unconditionally only so a future C-build step would not require a `Cargo.toml` edit; it is not invoked.
- The `pmt-runtime-check` feature still routes (via `vuma-codegen/pmt-runtime-check`) to the **pure-Rust** `runtime/pmt_check` module (a hand-translation of `proof/PMT/Extraction.lean`, parity-tested by `tests/pmt_parity_test.rs`); that module has no `extern "C"`, no `#[link]`, and no `proof/` path reference.
- IVE verification (`src/ive/src/verification.rs`) goes through **Z3** and hand-written Rust verifiers; the prior `verify_pmt_via_lean` FFI call site is gone (the comment block at lines 962 / 982 documents the removal).

The `proof/` tree is therefore **documentation/specification only** — its physical absence has zero effect on `cargo build --release --workspace`.

## DoD assessment

| DoD criterion | Status |
|---|---|
| `cargo build --release --workspace` exits 0 WITHOUT `proof/` present | **PASS** (exit 0; `Finished release profile [optimized] target(s) in 27.23s`) |
| `proof/` directory restored to original location | **PASS** (11 top-level entries match; `ls /home/z/my-project/vuma/proof` confirms) |
| `git status` shows no changes (no diff to `proof/` or any source) | **PASS** (`git status --short` empty; HEAD unchanged at `5a18ac86a9cd04ce477256eb1bee67ab380cfb70`) |
| Summary markdown at `vuma/scripts/audit/wave3_lean_decoupling.md` | **PASS** (this file) |

## Constraint check

- **No source files edited.** The only filesystem mutation outside `scripts/logs/` and `scripts/audit/` was the temporary `mv` of `proof/` to `/tmp/proof_backup_wave3d` and back — net-zero, verified by `git status` clean.
- **`proof/` restored before exit.** Verified by `ls /home/z/my-project/vuma/proof` showing 11 top-level entries.
- **No push.** Local commit only.
- **No further sub-agents spawned.**
- **Time budget:** ~12 minutes wall clock. (One `cargo build --release --workspace` invocation was killed by the agent tool's 9-minute command timeout with the build at the `vuma-tests` crate; a second invocation finished in 27.23s on the now-warm incremental cache. Net cargo wall time across both invocations ≈ 9-10 min, dominated by LTO link of the workspace release binaries on the Pi 5.)

## Status: PASS
