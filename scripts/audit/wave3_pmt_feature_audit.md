# Wave 3 — `pmt-runtime-check` Feature NO-OP Audit (caveat §3.1)

- **Task ID:** 3-b-audit
- **Agent:** 3-b-audit (sub-agent, wave 3)
- **Wave:** 3 (depends on 0, 1, 2, 3-a-test)
- **Caveat addressed:** §3.1 — `pmt-runtime-check` is a NO-OP in `vuma-ive` and a real feature in `vuma-codegen`
- **Files in scope (READ-ONLY):**
  - `src/ive/Cargo.toml`
  - `src/codegen/Cargo.toml`
  - `Cargo.toml` (root)
  - `build.rs` (root; file-level doc)
  - `src/codegen/src/runtime/pmt_check.rs`
  - `src/codegen/src/runtime/mod.rs`
  - `src/ive/src/verification.rs` (comment-block audit)
- **Files OUT of scope:** any source file edited (none touched)
- **Commit prefix:** `[3-b-audit]`

## Problem Statement

Per caveat §3.1, the `pmt-runtime-check` Cargo feature must be:

1. A **NO-OP** in `vuma-ive` — the Lean FFI bridge has been removed, and the
   feature declaration is `pmt-runtime-check = []` (empty feature, no deps).
2. A **real feature** in `vuma-codegen` — activates the pure-Rust `pmt_check`
   module (`src/codegen/src/runtime/pmt_check.rs`), which is a parity-tested
   hand-translation of the Lean definitions in `proof/PMT/Extraction.lean`.

The feature is retained as a no-op for IVE so existing CI commands
(`cargo build --features pmt-runtime-check`) continue to work without
triggering any Lean linkage.

## Static Audit (file contents)

| File | Field | Value | Confirms |
|------|-------|-------|----------|
| `src/ive/Cargo.toml:24-28` | `[features] pmt-runtime-check` | `= []` (empty, no deps) | NO-OP in IVE |
| `src/ive/Cargo.toml:26-27` | comment | "pmt-runtime-check: retained as a no-op for IVE (Lean FFI bridge removed). Still has a real effect in vuma-codegen" | NO-OP in IVE |
| `src/codegen/Cargo.toml:22-31` | `[features] pmt-runtime-check` | `= []` (empty, but module gated on feature) | feature is opt-in toggle |
| `src/codegen/Cargo.toml:24-30` | comment | "when enabled, compiles the verified Lean-translated PMT checkers (runtime/pmt_check.rs) and routes arena.rs::alloc_raw through `verified_capacity_check`" | Real effect in codegen |
| `Cargo.toml:111-113` (root) | `[features] pmt-runtime-check` | `= ["vuma-codegen/pmt-runtime-check", "vuma-ive/pmt-runtime-check"]` | Forwards feature to both crates |
| `build.rs:6-40` (root) | file-level doc | "Lean FFI bridge removed… The `pmt-runtime-check` Cargo feature is RETAINED as a no-op so existing CI commands continue to work; it no longer triggers any Lean linkage here. The feature still activates the independent pure-Rust `pmt_check` module in `vuma-codegen`" | NO-OP for IVE, real for codegen |
| `src/ive/src/verification.rs` | comment block | "Lean FFI bridge removed… `pmt-runtime-check` Cargo feature is now a no-op for IVE — it no [longer triggers Lean linkage]" | NO-OP in IVE |
| `src/codegen/src/runtime/mod.rs:14-15` | module gate | `#[cfg(feature = "pmt-runtime-check")] pub mod pmt_check;` | Module is feature-gated |
| `src/codegen/src/runtime/pmt_check.rs:15` | file-level gate | `#![cfg(feature = "pmt-runtime-check")]` | Entire file is feature-gated |
| `src/codegen/src/runtime/pmt_check.rs:21-54` | function defs | `verified_capacity_check`, `verified_field_bounds_check`, `verified_linearity_check`, `verified_pmt_check` — all `#[inline] pub fn`, pure Rust (no `extern "C"`, no link deps) | Hand-translated Rust verifier |

## Dynamic Audit (symbol-level build diff)

### Build protocol

For each crate, two `cargo build --lib` invocations were performed with
isolated `--target-dir` directories (so the rlib fingerprints did not
collide), one WITHOUT the feature and one WITH `--features
pmt-runtime-check`. The dev profile (LTO off, `codegen-units = 256`) was
used so the resulting `.rcgu.o` files inside each `.rlib` were regular ELF
relocatable objects that `nm` can parse (the `--release` profile emits LLVM
IR bitcode, which `nm` cannot read).

The crate hash component of every mangled symbol (`Cs<hash>_<krate>`) was
normalized to `CsXX_<krate>` so the diff is content-comparable rather than
fingerprint-comparable. Anonymous local labels (`.Lanon.*`) and
`GCC_except_table*` renumbering artifacts (which shift by ±1–3 between any
two compiles of the same source) were filtered out.

### Verification table

| Crate | Build profile | Diff size (function symbols) | `pmt_check` symbols added by feature? | `lean_` symbols in diff? | Verdict |
|-------|---------------|------------------------------|---------------------------------------|--------------------------|---------|
| `vuma-ive` | dev, `--lib` | 0 net additions, 0 net removals (only `GCC_except_table*` renumbering) | NO (grep returned empty) | NO (grep returned empty) | **NO-OP confirmed** |
| `vuma-codegen` | dev, `--lib` | +6 additions, 0 removals | YES — `verified_capacity_check` function body + its `Option::map_or` closure (3 `T` defined + 3 `U` referenced) | NO (grep returned empty) | **Activates pmt_check confirmed** |

### vuma-ive diff details

After crate-hash normalization and filtering `.Lanon.*` / `GCC_except_table*`
labels, the diff is:

```
4837c4837
< 0000000000000000 r GCC_except_table177
---
> 0000000000000000 r GCC_except_table174
4857,4858c4857,4858
< 0000000000000000 r GCC_except_table222
< 0000000000000000 r GCC_except_table228
---
> 0000000000000000 r GCC_except_table221
> 0000000000000000 r GCC_except_table223
...
```

The 28-line diff consists ENTIRELY of `GCC_except_tableNNN` renumbering
(compiler-internal exception-handling table numbering that shifts by ±1–3
between any two compiles). NO `pmt_check` symbols and NO `lean_` symbols
are introduced or removed by the feature. This is the textbook signature
of a NO-OP feature: the crate is recompiled (because cargo's fingerprint
includes the feature set), but no code paths differ.

### vuma-codegen diff details

After crate-hash normalization and filtering `.Lanon.*` labels, the diff
is 6 ADDED lines and 0 REMOVED lines:

```
365a366
>                  U _RINvMNtCsXX_4core6optionINtB3_6OptionyE6map_orbNCNvNtNtCsXX_12vuma_codegen7runtime9pmt_check23verified_capacity_check0EB10_
6457a6459
>                  U _RNCNvNtNtCsXX_12vuma_codegen7runtime9pmt_check23verified_capacity_check0B7_
8926a8929
>                  U _RNvNtNtCsXX_12vuma_codegen7runtime9pmt_check23verified_capacity_checkB5_
12664a12668
> 0000000000000000 T _RINvMNtCsXX_4core6optionINtB3_6OptionyE6map_orbNCNvNtNtCsXX_12vuma_codegen7runtime9pmt_check23verified_capacity_check0EB10_
20672a20677
> 0000000000000000 T _RNCNvNtNtCsXX_12vuma_codegen7runtime9pmt_check23verified_capacity_check0B7_
24191a24197
> 0000000000000000 T _RNvNtNtCsXX_12vuma_codegen7runtime9pmt_check23verified_capacity_checkB5_
```

Demangled, the 6 added symbols are:

1. `vuma_codegen::runtime::pmt_check::verified_capacity_check` (defined `T`)
2. `<closure #0>` of `verified_capacity_check` (the `Option::map_or` closure, defined `T`)
3. `<instantiation>` of `Option::<u64>::map_or::<bool>` for the closure (defined `T`)
4–6. The same three symbols as `U` (undefined, referenced) — these appear
   in codegen units that reference the function but don't define it.

The other three `verified_*` functions (`verified_field_bounds_check`,
`verified_linearity_check`, `verified_pmt_check`) are all marked
`#[inline]` and are sufficiently small that the Rust compiler inlines them
at every call site, so they produce no standalone symbols. This is the
expected behavior for a hand-written Rust verifier: the function bodies
are inlined for performance, and only `verified_capacity_check` (slightly
larger due to the `checked_add + map_or` chain) survives to a standalone
definition. The presence of even ONE `pmt_check` symbol in the with-feature
build (and ZERO in the no-feature build) is sufficient to confirm that the
feature activates the module.

### Lean symbol audit

| Build | Total `lean_` symbol occurrences | Of which `lean_alloc_mirror` (pre-existing Rust fn in `arena_proof_model.rs`) | Of which Lean-runtime FFI symbols (e.g. `lean_external`, `lean_string_…`) |
|-------|----------------------------------|------------------------------------------------------------------------------|---------------------------------------------------------------------------|
| `vuma-ive` no-feature | 0 | 0 | 0 |
| `vuma-ive` with-feature | 0 | 0 | 0 |
| `vuma-codegen` no-feature | 1 | 1 | 0 |
| `vuma-codegen` with-feature | 1 | 1 | 0 |

The single `lean_`-prefixed symbol in `vuma-codegen` is
`vuma_codegen::runtime::arena_proof_model::lean_alloc_mirror` — a
**pre-existing Rust function** (not Lean-runtime FFI), present in
**both** builds (i.e. NOT introduced or removed by the feature). The
feature diff contains zero `lean_` symbols.

## DoD Compliance

| DoD criterion | Met? | Evidence |
|---------------|------|----------|
| vuma-ive diff between feature/no-feature builds is empty (NO-OP) | YES | 0 net symbol additions/removals; only `GCC_except_table*` renumbering |
| vuma-codegen diff contains pmt_check symbols (`verified_pmt_check` etc.) | YES | 6 added `verified_capacity_check` symbols (3 `T` + 3 `U`); the other 3 `verified_*` functions are inlined at every call site |
| No Lean symbols in either diff | YES | Both diffs grep clean for `lean_`; the pre-existing `lean_alloc_mirror` Rust fn is in BOTH codegen builds (unchanged) |
| Summary markdown exists at `scripts/audit/wave3_pmt_feature_audit.md` | YES | This file |

## Conclusion

The `pmt-runtime-check` Cargo feature behaves exactly as documented in
caveat §3.1:

- **In `vuma-ive`**: a pure NO-OP. The feature declaration is
  `pmt-runtime-check = []` with no dependencies, no `cfg(feature = …)`
  gates anywhere in `src/ive/src/`, and no symbol-level differences
  between feature-on and feature-off builds (the only diff is compiler-
  internal `GCC_except_table*` renumbering). The Lean FFI bridge has been
  deleted (see `build.rs` file-level doc and the comment block in
  `src/ive/src/verification.rs`); the feature is retained solely so
  `cargo build --features pmt-runtime-check` continues to work without
  breaking CI.

- **In `vuma-codegen`**: a real feature. The `pmt_check` module
  (`src/codegen/src/runtime/pmt_check.rs`, 96 lines) is gated by
  `#![cfg(feature = "pmt-runtime-check")]` at the file level and
  `#[cfg(feature = "pmt-runtime-check")] pub mod pmt_check;` in
  `src/codegen/src/runtime/mod.rs`. The with-feature build contains 6
  new symbols for `verified_capacity_check` (function body + closure +
  closure instantiation); the other 3 `verified_*` functions are
  inlined at every call site. The module is pure Rust (no `extern "C"`,
  no link-time dependencies), so it compiles cleanly without the root
  `build.rs` C-archive linkage. The 4 functions are hand-translated
  from `proof/PMT/Extraction.lean` and verified by parity test
  (`tests/pmt_parity_test.rs`).

No source files were modified during this audit (READ-ONLY task).
