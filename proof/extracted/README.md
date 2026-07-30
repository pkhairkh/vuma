# Extracted Verified Checkers (legacy FFI bridge — deleted)

This directory previously held the Rust side of the Lean↔Rust FFI bridge
for the PMT bounds-checking logic proven in `proof/PMT/Extraction.lean`.
**The FFI bridge has been deleted.** This README documents the historical
context and the current state.

> The Lean definitions in `proof/PMT/Extraction.lean` remain the formal
> source of truth. Each checker has a machine-checked soundness theorem.
> The bridge's job was to let the Rust runtime call those *same* Lean
> definitions (via C extraction) instead of a hand-translation. That
> bridge is gone; the hand-written Rust verifiers in `src/ive/` plus
> **Z3** (the SMT solver, hard build-time dependency in
> `src/ive/Cargo.toml`: `z3 = "0.20"`) now do the executable
> verification. The hand-translated Rust checkers in `pmt_check.rs`
> (below) remain as a parity-tested reference; they are not themselves
> formally verified.

## Source of Truth

The Lean definitions in `proof/PMT/Extraction.lean` are the formal specification.
Each function has a machine-checked soundness theorem:

| Lean Function | Soundness Theorem | Statement |
|---------------|-------------------|-----------|
| `verified_capacity_check` | `verified_capacity_check_correct` | If check returns true, then `used + size <= capacity` |
| `verified_field_bounds_check` | `verified_field_bounds_check_correct` | If check returns true, then `f.offset + f.size <= layout.total_size` |
| `verified_linearity_check` | `verified_linearity_check_correct` | If check returns true, then `var not in consumed` |
| `verified_pmt_check` | `verified_pmt_check_correct` | If check returns true, all three sub-checks hold |

These theorems remain machine-checked by `lake build`. They are the
formal specification; they are **not linked into the compiler binary**.

## Current Status (FFI bridge deleted)

### DONE — and now historical

The FFI bridge was developed through several milestones, all of which
have been **deleted** in the current codebase. The historical record
below is retained for context.

1. **7 `@[export]` symbols + 7 `_prim` wrappers in `Extraction.lean`**
   (historical). The 7 canonical exports
   (`lean_verified_{capacity,field_bounds,linearity,pmt}_check`,
   `lean_verify_{transform,state_reads,state_writes}`) plus 7 flattened
   `_prim` primitive wrappers were present. **Status:** the `@[export]`
   attributes remain in the Lean source for self-documentation, but no C
   archive is produced and no `extern "C"` bindings resolve against Lean
   symbols in the current build.

2. **7 matching `extern "C"` declarations in `proof/extracted/pmt_check.rs`**
   (historical). The `lean_ffi` module's `extern "C"` block was
   **removed** when the bridge was deleted. The hand-translated
   `verified_*` Rust functions in the same file are retained and are
   parity-tested against the Lean definitions.

3. **`build.rs` linkage pipeline with stub fallback** (historical,
   deleted). The `link_lean_ffi()` function — gated behind
   `#[cfg(feature = "pmt-runtime-check")]` — used to attempt the real
   Lean C pipeline (detect `lake`, run `lake build`, look for
   `.lake/build/lib/PMT/Extraction.c` + `.lake/build/lib/lean_runtime`)
   and on failure fall back to compiling `proof/extracted/lean_stub.c`
   into `liblean_extraction.a`. **Status:** the `link_lean_ffi()`
   function and the stub-fallback path are **deleted**. The
   `pmt-runtime-check` feature is retained as a no-op at the IVE layer
   (see [`docs/caveats.md` §3.1](../docs/caveats.md)) so existing CI
   commands continue to work without changes.

4. **Feature-gated runtime dispatch** (historical, removed). The
   `pmt-runtime-check` feature used to gate the codegen runtime's arena
   capacity-check path between the Lean-FFI call and the hand-written
   `checked_add` + `> capacity` pair. **Status:** the dispatch is
   **deleted**; `src/codegen/src/runtime/arena.rs::alloc` always uses
   the hand-written `checked_add` + `> capacity` pair (with a
   `__arena_overflow` trap on violation). The IVE state verifiers in
   `src/ive/` emit `contract_assert(…)` obligations that **Z3
   discharges** at compile time; there is no Lean linkage in the
   executable path.

5. **Cargo feature wired across all manifests, default-off**
   (historical). `pmt-runtime-check` is still defined in the root
   `Cargo.toml` (`pmt-runtime-check = ["vuma-codegen/pmt-runtime-check"]`)
   and in `src/codegen/Cargo.toml` (`pmt-runtime-check = []`), but it
   is now a **no-op at the IVE layer**. In `vuma-codegen` the feature
   still has a real effect: it activates the independent pure-Rust
   `pmt_check` module (a parity-tested hand-translation of the Lean
   definitions in `proof/PMT/Extraction.lean`).

6. **Two tests**:
   - `tests/ffi_signature_conformance.rs` — historical structural FFI
     conformance test. **Status:** retained but skips cleanly when the
     feature is off; the `dlsym` lookup it used to perform no longer
     finds Lean symbols (because they are not linked).
   - `tests/pmt_feature_flag_test.rs` — behavioral smoke test. **Status:**
     retained; asserts the codegen `pmt_check::verified_capacity_check`
     (the hand-translation) is callable and returns the right `bool`
     for overflow / valid / boundary inputs.

### Current executable verifier (Z3-based)

The **executable verifier** is **Z3** (the SMT solver). The IVE state
verifiers in `src/ive/` (`state_read.rs`, `state_write.rs`,
`state_transform.rs`, `borrow_region.rs`, `information_flow.rs`,
`session_type.rs`, `arena_bounds.rs`, `verification.rs`) emit
`contract_assert(…)` obligations for every memory-safety check. Z3
discharges the obligations at compile time.

- **Discharge rate on the gold-standard suite:** **100 %**
  (29 944 / 29 944 = 100.00 % across all 19 backends).
- **Failure mode:** when Z3 cannot discharge a contract, the pipeline
  hard-fails with `VumaError::Verification`. There is no advisory path
  and no `WARNING + TODO` stub for deferred contract discharge.

The hand-translated Rust checkers in `pmt_check.rs` (this directory,
plus the in-tree copy at `src/codegen/src/runtime/pmt_check.rs`) are
**parity-tested** against the Lean definitions in
`proof/PMT/Extraction.lean` via `tests/pmt_parity_test.rs` (5 tests).
They are not themselves formally verified — the formal verification
lives in the Lean theorems (the formal spec); the executable
verification lives in Z3 + the Rust verifiers.

### What is NOT happening anymore

- **No `lean_stub.c` compilation.** The stub file
  (`proof/extracted/lean_stub.c`) is retained in-tree for historical
  reference but is **not compiled by any build target**. The
  `link_lean_ffi()` function that used to invoke `cc::Build` on it is
  deleted.
- **No `lean_ffi_linked` cfg.** The `cargo:rustc-cfg=lean_ffi_linked`
  emission is deleted; no code is gated on `#[cfg(lean_ffi_linked)]`
  anymore (any such code would be dead).
- **No `lean_verify_*` / `lean_verified_*` extern surface.** The
  `extern "C"` block in `pmt_check.rs::lean_ffi` is deleted; no Rust
  code calls Lean symbols via FFI.
- **No real Lean C linkage.** The `try_real_lean_pipeline()` function
  in `build.rs` is deleted; there is no path that compiles
  `.lake/build/lib/PMT/Extraction.c` into the Rust binary.
- **No IVE state-verifier routing to FFI.** The
  `lean_verify_{transform,state_reads,state_writes}` routing in
  `src/ive/src/verification.rs` is comment-only and the comments have
  been removed; the hand-written `verify_state_reads` /
  `verify_state_writes` / `verify_all_transforms` always run, with Z3
  discharging their `contract_assert(…)` obligations.

## How to enable the hand-translated checkers

The `pmt-runtime-check` Cargo feature on `vuma-codegen` activates the
in-tree `pmt_check` module — the hand-translated Rust checkers that
mirror `proof/PMT/Extraction.lean`. This is *not* a Lean linkage; it is
a pure-Rust hand-translation, parity-tested against the Lean definitions.

```bash
# Compile + run tests with the hand-translated checkers wired in:
cargo test --features pmt-runtime-check

# Or just build:
cargo build --features pmt-runtime-check
```

The feature is **off by default** — `cargo build` / `cargo test` without
`--features` uses the unverified hand-written checkers in `arena.rs`.
When the feature is on, `arena.rs::alloc` dispatches to
`pmt_check::verified_capacity_check` (the hand-translation) instead of
the inline `checked_add` + `> capacity` pair.

## Fallback behavior (feature off)

With the feature off (the default), `arena.rs::alloc` uses the
hand-written `checked_add` + `> capacity` pair. The IVE state verifiers
in `src/ive/src/verification.rs` use the hand-written Rust verifiers
(`verify_state_reads`, `verify_state_writes`, `verify_all_transforms`).
**Z3 discharges the `contract_assert(…)` obligations these verifiers
emit** at compile time; the runtime `__arena_overflow` trap is the
fallback for cases Z3 cannot predict (e.g. branch-dependent allocation
counts).

## Extraction Pipeline (historical reference)

The intended end-to-end pipeline, for historical reference (this is
**not** the current architecture — the bridge is deleted):

### Stage 1: Lean -> C (historical)
`lake build` produced `.c` files in `proof/.lake/build/ir/`. The key
files were `PMT_Extraction.c.o` (compiled object file) and
`PMT_Extraction.olean` (Lean interface file).

### Stage 2: C -> Rust FFI (historical, deleted)
`proof/extracted/pmt_check.rs::lean_ffi` declared the `extern "C"`
surface matching the Lean `@[export]` ABI (`*mut lean_object*` args,
`u8` return). These externs resolved against `lean_stub.c` (the
fail-closed stub); the real Lean runtime linkage was never wired up.
Both the extern block and the stub are now deleted.

### Stage 3: Integration + Parity Test (current)
- Feature flag `pmt-runtime-check` wired across `Cargo.toml` files
  (retained as a no-op at the IVE layer).
- Codegen arena capacity-check dispatches to `verified_capacity_check`
  under the feature (hand-translation, not Lean FFI).
- IVE state-verifier routing under `#[cfg(lean_ffi_linked)]` is
  **deleted** (no `lean_ffi_linked` cfg is ever emitted).
- Full 1 589-fixture parity differential is exercised by
  `tests/pmt_parity_test.rs` (5 tests) and the gold-standard suite
  (29 944 / 29 944 = 100.00 % across 19 backends).

## Build

```bash
# Build the Lean formal specification (produces .c files, but they are
# not linked into the Rust binary anymore):
cd proof && lake build PMT.Extraction

# The .c files are in:
ls .lake/build/ir/PMT_Extraction.c
```

## Verification

The soundness of each checker is machine-checked by Lean (the formal
specification):

```bash
cd proof && lake build PMT.Extraction
# No sorry warnings - all theorems proven
```

The **executable** verification is Z3-based and runs in the regular
`cargo build` / `cargo test` path (no Lean linkage required):

```bash
cargo build    # requires libz3-dev installed (apt install libz3-dev)
cargo test --workspace
```

## References

- `proof/PMT/Extraction.lean` — Lean source + soundness theorems + 7 `@[export]` + 7 `_prim` wrappers (historical — the `@[export]` attributes remain in the Lean source for self-documentation; the FFI bridge is deleted)
- `proof/extracted/pmt_check.rs` — Rust hand-translation of the Lean checkers (parity-tested; the `extern "C"` `lean_ffi` block is deleted)
- `proof/extracted/lean_stub.c` — **no longer compiled**; the previous fail-closed C stub is retained in-tree for historical reference only
- `build.rs` — the `link_lean_ffi()` real-pipeline-or-stub selector is **deleted**
- `src/codegen/src/runtime/arena.rs` — hand-written capacity-check dispatch (the `pmt-runtime-check` feature still swaps in the hand-translated `verified_capacity_check` from `pmt_check.rs`)
- `src/codegen/src/runtime/pmt_check.rs` — in-tree copy of the hand-translated runtime checker module
- `src/ive/src/verification.rs` — IVE state-verifier entry points; the `lean_verify_*` routing is deleted, the hand-written verifiers always run, and Z3 discharges their `contract_assert(…)` obligations
- `src/ive/Cargo.toml` — `z3 = "0.20"` (hard build-time dependency; the executable verifier)
- `tests/ffi_signature_conformance.rs` — historical structural FFI conformance test (skips cleanly when the feature is off; no Lean symbols to `dlsym` anymore)
- `tests/pmt_feature_flag_test.rs` — behavioral smoke test for the hand-translated checkers
- `tests/pmt_parity_test.rs` — hand-translation parity test (5 tests; validates the Rust hand-translation against the Lean definitions)
- [`docs/caveats.md` §3](../docs/caveats.md) — full statement of the
  Lean-standalone / Z3-executable separation
