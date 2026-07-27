# FFI_BRIDGE_PLAN.md — Lean↔Rust FFI Bridge Blueprint (Waves 4-6)

Status: BLUEPRINT. The bridge is currently **declared but not wired**:
`pmt_check.rs` has an `extern "C"` block under `#[cfg(feature="pmt-runtime-check")]`,
`verification.rs:836-857` is comments-only, `build.rs` only detects the rustc
version, and `tests/pmt_parity_test.rs` defines its own hand-translated
`lean_verify_*` duplicates instead of calling the externs.

## §1 — The 7 Lean functions to export

Found in `proof/PMT/Extraction.lean` (export-eligibility list at L230-236;
`@[export lean_verify_transform]` annotation confirmed at L246). Group A
are pure-primitive and already C-marshallable; Group B carry complex Lean
structs and are the marshalling risk.

| # | Lean export name | Lean type (abbrev) | Rust signature (target) |
|---|---|---|---|
| 1 | `verified_capacity_check` | `Nat→Nat→Nat→Bool` | `fn(u64,u64,u64)->u8` |
| 2 | `verified_field_bounds_check` | `Nat→Nat→Nat→Bool` | `fn(u64,u64,u64)->u8` |
| 3 | `verified_linearity_check` | `String→List String→Bool` | `fn(*const c_char, *const *const c_char, usize)->u8` |
| 4 | `verified_pmt_check` | composed of 1-3 | `fn(...)->u8` |
| 5 | `lean_verify_transform` | `LayoutRegistry→StateTransform→Bool` | `fn(*mut LeanObject)->u8` (boxed) |
| 6 | `lean_verify_state_reads` | `List (String×LayoutInfo)→List StateRead→Bool` | `fn(*mut LeanObject,*mut LeanObject)->u8` |
| 7 | `lean_verify_state_writes` | `List (String×LayoutInfo)→List String→List StateWrite→Bool` | `fn(*mut LeanObject,*mut LeanObject,*mut LeanObject)->u8` |

**Marshalling decision (Wave 4):** Group A → re-export with **primitive C
signatures** (Lean-side `@[export]` wrappers that unbox internally; avoids
Rust building Lean objects). Group B → either (i) keep `lean_object*` and
link `lean_runtime` for `lean_ctor`/`lean_alloc`, or (ii) add **flattened
Lean wrappers** taking `char*` + `usize` arrays. Option (ii) is
recommended: it keeps Rust free of Lean-runtime calls and is the only
viable path without a full `lean_runtime` link.

## §2 — build.rs linkage plan (Wave 5)

Current `build.rs` = rustc-version detection only. Replace with:

1. `cargo:rerun-if-changed=proof/PMT/Extraction.lean` + `proof/lakefile.lean`.
2. Invoke `lake build` (or `lake env lean --root=proof`) gated on
   `cfg(feature="pmt-runtime-check")` and a `VUMA_LEAN_C=1` env var, so
   default builds stay Lean-free. On failure → `cargo:warning` + fall back
   to hand-translated path (do **not** `panic!` in build.rs).
3. Locate emitted C: `proof/.lake/build/lib/PMT/Extraction.c` (+ `.o`).
   Also require `lean_runtime` object(s) from `.lake/build/lib/lean_runtime`.
4. `cc::Build::file(c).compile("lean_pmt")` — compile each `.c` into a
   static archive; include lean runtime objects.
5. Emit `cargo:rustc-link-lib=static=lean_pmt`,
   `cargo:rustc-link-search=native=proof/.lake/build/lib`,
   `cargo:rustc-cfg=lean_ffi_linked`.
6. `cargo:rustc-env=LEAN_FFI_LINKED=1` so `verification.rs` can runtime-gate.

## §3 — verification.rs routing plan (Wave 5)

In `verify_state_reads/writes/transform` call sites (L836-857 region),
replace the unconditional hand-written call with:

```rust
#[cfg(all(feature="pmt-runtime-check", lean_ffi_linked))]
let read_ok = unsafe { pmt_check::lean_ffi::call_lean_verify_state_reads(env_obj, reads_obj) };
#[cfg(not(all(feature="pmt-runtime-check", lean_ffi_linked)))]
let read_results = verify_state_reads(&layouts, &write_layouts, &reads); // fallback
```

Add a `marshal` submodule (Wave 4) that builds `lean_object*` inputs **only
when `lean_ffi_linked`** is set, so the Lean-runtime dependency is fully
isolated. Keep the hand-written verifiers as the documented fallback —
they are what the parity test guards.

## §4 — Test binding plan (Wave 6)

`tests/pmt_parity_test.rs` currently **defines** `lean_verify_transform`
(L119), `lean_verify_state_reads` (L139), `lean_verify_state_writes` (L161),
plus `_v2` variants (L196, L232) — these shadow the externs. Plan:

1. Rename the in-test duplicates to `hand_*` (e.g. `hand_verify_transform`)
   so they no longer collide with extern names.
2. Add `#[cfg(feature="pmt-runtime-check")] extern "C" { ... }` block
   mirroring `pmt_check.rs::lean_ffi` (or re-export it `pub`).
3. Parity test = `assert_eq!(hand_verify_transform(c), unsafe{lean_verify_transform(marshalled)})`
   over the full corpus; runs only when `lean_ffi_linked`.
4. Default test build (no feature) keeps running `hand_*` against the
   Rust verifiers — the existing safety net stays intact.

## §5 — CI parity gate plan (Wave 6)

- Job `pmt-parity` runs **only** on the Lean-toolchain image; sets
  `VUMA_LEAN_C=1`, enables `--features pmt-runtime-check`, runs
  `lake build` then `cargo test -p ive --features pmt-runtime-check
  pmt_parity_test`.
- Gate is **advisory** (non-blocking) for the first sprint, then
  **hard-fail** once 100 consecutive green runs land. Track via
  `proof/.ci-parity-streak` counter file.
- Job `pmt-fallback` runs the default build (no Lean) on every PR —
  guarantees the hand-translated path never regresses.

## §6 — Rollback plan if Lean C output is unstable

1. **Instant rollback:** unset `VUMA_LEAN_C` / drop
   `--features pmt-runtime-check`. `build.rs` skips lake entirely;
   `verification.rs` compiles out the `lean_ffi_linked` branch; zero
   behavior change vs. today.
2. **Symbol-mismatch fallback:** if `lake build` succeeds but linking
   fails (missing symbols, ABI drift), `build.rs` must emit
   `cargo:warning=lean ffi link failed; using hand-translated path` and
   **not** set `lean_ffi_linked` — verification.rs then silently uses the
   hand-written path. Never `panic!` in build.rs.
3. **Lean-toolchain drift:** pin `lean-toolchain` to a specific Lean
   release; bump only after a full `pmt-parity` green run. If a bump
   breaks extraction, revert the toolchain file (single-line revert).
4. **Marshalling regression:** if the Wave 4 `marshal` submodule produces
   wrong `lean_object*` graphs, disable Group B (functions 5-7) but keep
   Group A (1-4, primitive) live — partial FFI is strictly better than
   none and still exercises the Lean proofs.

This file is the contract Waves 4-6 implement against.
