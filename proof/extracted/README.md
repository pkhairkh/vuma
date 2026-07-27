# Extracted Verified Checkers

This directory holds the Rust side of the Lean<->Rust FFI bridge for the
PMT bounds-checking logic proven in `proof/PMT/Extraction.lean`. It also
documents the **current, honest status** of that bridge after Waves 4-A
through 5-C (see `## Current Status` below).

> The Lean definitions in `proof/PMT/Extraction.lean` remain the formal
> source of truth. Each checker has a machine-checked soundness theorem.
> The bridge's job is to let the Rust runtime call those *same* Lean
> definitions (via C extraction) instead of a hand-translation. As of
> Wave 5 the bridge is **wired end-to-end but running on a stub**: the
> real Lean runtime is not yet linked, so every call resolves to a
> fail-closed C stub. The hand-written Rust verifiers remain the
> production path.

## Source of Truth

The Lean definitions in `proof/PMT/Extraction.lean` are the formal specification.
Each function has a machine-checked soundness theorem:

| Lean Function | Soundness Theorem | Statement |
|---------------|-------------------|-----------|
| `verified_capacity_check` | `verified_capacity_check_correct` | If check returns true, then `used + size <= capacity` |
| `verified_field_bounds_check` | `verified_field_bounds_check_correct` | If check returns true, then `f.offset + f.size <= layout.total_size` |
| `verified_linearity_check` | `verified_linearity_check_correct` | If check returns true, then `var not in consumed` |
| `verified_pmt_check` | `verified_pmt_check_correct` | If check returns true, all three sub-checks hold |

## Current Status (post-Wave 5)

### DONE

1. **7 `@[export]` symbols + 7 `_prim` wrappers in `Extraction.lean` (Wave 4-A).**
   The 7 canonical exports (`lean_verified_{capacity,field_bounds,linearity,pmt}_check`,
   `lean_verify_{transform,state_reads,state_writes}`) are present, plus 7 flattened
   `_prim` primitive wrappers with C-marshallable signatures
   (`Bool`/`UInt64`/`String` only - no boxed `lean_object*` needed for the `_prim`
   path). `lake build PMT.Extraction` passes.

2. **7 matching `extern "C"` declarations in `proof/extracted/pmt_check.rs` (Wave 4-B).**
   Each `extern "C"` block in the `lean_ffi` module is arity-aligned to the
   corresponding `@[export]` (args boxed as `*mut LeanObject = *mut c_void`,
   returns `u8` = Lean `Bool`'s `uint8_t`). Safe `call_lean_*` wrappers wrap the
   unsafe externs.

3. **`build.rs` linkage pipeline with stub fallback (Wave 4-D).**
   `link_lean_ffi()` is gated entirely behind `#[cfg(feature = "pmt-runtime-check")]`.
   When the feature is on it **attempts the real Lean C pipeline** - detects
   `lake --version` on `PATH` and `LEAN_HOME`, runs `lake build`, looks for
   `.lake/build/lib/PMT/Extraction.c` + `.lake/build/lib/lean_runtime`, and on
   success compiles them via `cc::Build` and emits
   `cargo:rustc-cfg=lean_ffi_linked` + `cargo:rustc-env=LEAN_FFI_LINKED=1`.
   On **any** failure (or missing `lake`/`LEAN_HOME`) it prints a
   `cargo:warning=... - using stub` and instead compiles
   `proof/extracted/lean_stub.c` into `liblean_extraction.a`, emitting only
   `cargo:rustc-link-lib=static=lean_extraction` (and **not** `lean_ffi_linked`).
   Feature-off builds never invoke `cc` at all - pre-Wave-4 behavior preserved.

4. **Feature-gated runtime dispatch (Wave 5-A).**
   The `pmt-runtime-check` feature gates the codegen runtime's arena
   capacity-check path: `src/codegen/src/runtime/arena.rs::alloc` dispatches to
   `pmt_check::verified_capacity_check` under
   `#[cfg(feature = "pmt-runtime-check")]` and falls back to the hand-written
   `checked_add` + `> capacity` pair under `#[cfg(not(...))]`. The `pmt_check`
   module itself (`src/codegen/src/runtime/pmt_check.rs`,
   `src/codegen/src/runtime/mod.rs`) is compiled only when the feature is on.
   The IVE state-verifier routing in `src/ive/src/verification.rs`
   (`lean_verify_{transform,state_reads,state_writes}`) is **documented but not
   yet dispatching** - see PARTIAL below.

5. **Cargo feature wired across all manifests, default-off (Wave 5-B).**
   `pmt-runtime-check` is defined in the root `Cargo.toml`
   (`pmt-runtime-check = ["vuma-codegen/pmt-runtime-check"]`) and in
   `src/codegen/Cargo.toml` (`pmt-runtime-check = []`). No `default = [...]`
   list references it, so it is **off by default** - `cargo build` /
   `cargo test` without `--features` is bit-for-bit identical to pre-Wave-4
   behavior.

6. **Two tests (Wave 4-C + Wave 5-C).**
   - `tests/ffi_signature_conformance.rs` (Wave 4-C, structural): a no-op skip
     when the feature is off; when on, it `dlsym`s all 7 expected symbols and
     reports which are missing (today: 7/7 missing, because the stub symbols
     are statically linked and not exported into `.dynsym` - expected until
     real linkage + `-Wl,--export-dynamic`).
   - `tests/pmt_feature_flag_test.rs` (Wave 5-C, behavioral smoke): gated by
     `#![cfg(feature = "pmt-runtime-check")]`; asserts the codegen
     `pmt_check::verified_capacity_check` is callable and returns the right
     `bool` for overflow / valid / boundary inputs.

### PARTIAL

- **Real Lean C linkage is NOT yet functional.** `try_real_lean_pipeline()` in
  `build.rs` intentionally returns `Err` on the missing
  `.lake/build/lib/lean_runtime` objects (the Lean runtime archive is not yet
  wired into the `cc::Build` invocation - see FFI_BRIDGE_PLAN section 2.3). In
  every observed build the script prints `Lean FFI linkage skipped (lake=absent
  LEAN_HOME=unset) - using stub` and compiles `lean_stub.c` instead.

- **`lean_ffi_linked` cfg is never emitted yet.** Because the real pipeline
  always fails, the stub path is always taken and `lean_ffi_linked` is never
  set. Consequently any code gated behind `#[cfg(lean_ffi_linked)]` (the
  future "really call Lean" branch) is dead today - the stub symbols satisfy
  the linker but the routing never trusts them.

- **The stub is fail-closed, not a silent pass.** `lean_stub.c` returns:
  - `0` (false) for `lean_verified_{capacity,field_bounds,linearity,pmt}_check`
    -> a failed capacity/bounds/linearity check would **reject** the program
    (safe under-approximation);
  - `1` (true) for `lean_verify_{transform,state_reads,state_writes}` -> a
    passed state-verifier check would **accept** (this is the unsound
    direction, which is why `lean_ffi_linked` is never emitted and the IVE
    state verifiers are **not** routed to FFI yet).

- **IVE state-verifier routing (`verification.rs`) is comment-only.**
  `src/ive/src/verification.rs` (around the `verify_pmt` block) still contains
  the "Wave 1 task IVE-1-B" comment describing the planned FFI routing for
  `lean_verify_{transform,state_reads,state_writes}`, but has **no
  `#[cfg(...)]` dispatch** - the hand-written `verify_state_reads` /
  `verify_state_writes` / `verify_all_transforms` always run. This is
  intentional and safe: the stub returns `1` (true) for those, so routing to
  it would be unsound. Routing lands together with real Lean linkage.

### STILL TODO

- **Real Lean runtime linkage** - link `lean_runtime` objects alongside
  `Extraction.c` in `build.rs` so `try_real_lean_pipeline()` succeeds and
  `lean_ffi_linked` is emitted. Tracked as a **Wave 6 follow-up** (or
  `PMT-1-G`).
- **Full behavioral parity test** replacing the hand-translated duplicates in
  `tests/pmt_parity_test.rs` - run the real Lean FFI checkers against all
  1,536+ gold-standard `.vuma` fixtures and diff against the hand-written
  verifiers. Tracked as **Wave 6-A**.
- **CI parity gate** - a CI job that fails on Lean<->Rust drift (today no such
  job exists; Lean and Rust jobs run as independent siblings). Tracked as
  **Wave 7-A**.

## How to enable

```bash
# Compile + run tests with the Lean-FFI bridge wired (stub today, real later):
cargo test --features pmt-runtime-check

# Or just build:
cargo build --features pmt-runtime-check
```

When the feature is on you will see a `cargo:warning=Lean FFI linkage skipped
... - using stub` line from `build.rs` unless a real Lean toolchain
(`lake` on `PATH` + `LEAN_HOME` set + `.lake/build/lib/lean_runtime` present)
is available. That warning is expected today.

## Fallback behavior (feature off)

With the feature off (the default), **nothing changes**: `build.rs` never
invokes `cc`, no `lean_stub.c` is compiled, the codegen `pmt_check` module is
not compiled, and `arena.rs::alloc` uses the original hand-written
`checked_add` + `> capacity` pair. The IVE verifiers in `verification.rs`
use the hand-written Rust verifiers (`verify_state_reads`,
`verify_state_writes`, `verify_all_transforms`) exactly as before. This is the
production path and is unchanged by Waves 4-5.

## Extraction Pipeline (reference)

The intended end-to-end pipeline, for reference:

### Stage 1: Lean -> C
`lake build` produces `.c` files in `proof/.lake/build/ir/`. The key files:
- `PMT_Extraction.c.o` - compiled object file
- `PMT_Extraction.olean` - Lean interface file

### Stage 2: C -> Rust FFI  (Wave 4-B, partial; Wave 6 completes)
`proof/extracted/pmt_check.rs::lean_ffi` declares the `extern "C"` surface
matching the Lean `@[export]` ABI (`*mut lean_object*` args, `u8` return).
Today these externs resolve against `lean_stub.c`; once `lean_runtime` is
linked (Wave 6) they will resolve against the real extracted C.

### Stage 3: Integration + Parity Test  (Wave 5-A partial; Wave 6-A completes)
- Feature flag `pmt-runtime-check` wired across `Cargo.toml` files _(done,
  Wave 5-B)_.
- Codegen arena capacity-check dispatches to `verified_capacity_check` under
  the feature _(done, Wave 5-A)_.
- IVE state-verifier routing under `#[cfg(lean_ffi_linked)]` _(TODO - gated
  on real Lean linkage)_.
- Full 1,536+-fixture parity differential _(TODO - Wave 6-A)_.

## Build

```bash
# Build the Lean checkers (produces .c files)
cd proof && lake build PMT.Extraction

# The .c files are in:
ls .lake/build/ir/PMT_Extraction.c
```

## Verification

The soundness of each checker is machine-checked by Lean:

```bash
cd proof && lake build PMT.Extraction
# No sorry warnings - all theorems proven
```

## References

- `proof/PMT/Extraction.lean` - Lean source + soundness theorems + 7 `@[export]` + 7 `_prim` wrappers (Wave 4-A)
- `proof/extracted/pmt_check.rs` - Rust `extern "C"` FFI surface (Wave 4-B)
- `proof/extracted/lean_stub.c` - fail-closed C stub compiled by `build.rs` when real Lean linkage is unavailable (Wave 4-D)
- `build.rs` - `link_lean_ffi()` real-pipeline-or-stub selector (Wave 4-D)
- `src/codegen/src/runtime/arena.rs` - feature-gated capacity-check dispatch (Wave 5-A)
- `src/codegen/src/runtime/pmt_check.rs` - feature-gated runtime checker module
- `src/ive/src/verification.rs` - IVE state-verifier routing (comment-only, pending real linkage)
- `tests/ffi_signature_conformance.rs` - structural FFI conformance test (Wave 4-C)
- `tests/pmt_feature_flag_test.rs` - behavioral smoke test (Wave 5-C)
- `tests/pmt_parity_test.rs` - hand-translation parity test (to be replaced by Wave 6-A)
- `FFI_BRIDGE_PLAN.md` - full bridge plan (section 1 symbol table, section 2 build.rs, section 3 routing, section 4 parity)
