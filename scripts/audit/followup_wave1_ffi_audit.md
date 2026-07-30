# Follow-up Wave 1 — Test-File FFI Cleanup Audit (F1-a-audit)

- **Task ID:** F1-a-audit
- **Agent:** F1-a-audit (sub-agent, follow-up Wave 1)
- **Wave:** 1 (test-file FFI cleanup; runs after follow-up Wave 0)
- **Prior-run context:** Wave 3 task `3-c-test` (commit `5a18ac86`) noted:
  > "A future cleanup sub-agent could remove the `lean_ffi` module from the
  > test file to make the feature a true no-op for tests too."
  This follow-up wave executes that cleanup. F1-a-audit is the **READ-ONLY
  audit** stage; a downstream task (F1-b or similar) will perform the edits.
- **Files in scope (READ-ONLY):**
  - `tests/pmt_parity_test.rs` (975 lines)
  - `tests/pmt_parity_test_full.rs` (750 lines)
  - `tests/pmt_extraction_diff.rs` (268 lines)
  - `proof/extracted/lean_stub.c` (167 lines — the prior-run C stub archive
    source; symbols exported listed below)
  - `proof/extracted/README.md` (249 lines — confirms the FFI bridge deletion
    direction)
  - `src/codegen/src/runtime/pmt_check.rs` (96 lines — canonical in-tree Rust
    hand-translation; `lean_ffi` already removed per Wave 3 `3-b-audit`)
- **Files OUT of scope:** any source file to be edited (no source edits in
  this audit stage).
- **Commit prefix:** `[F1-a-audit]`

## 1. Canonical reference (already-clean module)

`src/codegen/src/runtime/pmt_check.rs` is the canonical in-tree Rust
hand-translation. It is feature-gated `#![cfg(feature = "pmt-runtime-check")]`
(line 15) and exports 4 pure-Rust functions:

| Function | Signature | Line |
|---|---|---|
| `verified_capacity_check` | `(used: u64, size: u64, capacity: u64) -> bool` | 21 |
| `verified_field_bounds_check` | `(offset: u64, size: u64, total: u64) -> bool` | 28 |
| `verified_linearity_check` | `(var: &str, consumed: &[&str]) -> bool` | 35 |
| `verified_pmt_check` | `(used: u64, capacity: u64, offset: u64, size: u64, total: u64, var: &str, consumed: &[&str]) -> bool` | 42 |

**No `extern "C"` block, no `#[link]` attribute, no `lean_ffi` module.**
This is the target end-state for the test files.

## 2. Stub archive — exported symbols (`proof/extracted/lean_stub.c`)

The prior Wave 3 / 3-c-test run compiled this file into `liblean_extraction.a`
so the test files' `#[link(name = "lean_extraction", kind = "static")]`
directives would resolve. The stub exports **14 C symbols** (per the file's
header comment, "7 `@[export]` + 7 `_prim` wrappers"):

| Group | Symbol | Signature (C) | Return |
|---|---|---|---|
| A | `lean_verified_capacity_check` | `(lean_object*, lean_object*, lean_object*)` | `0` |
| A | `lean_verified_field_bounds_check` | `(lean_object*, lean_object*)` | `0` |
| A | `lean_verified_linearity_check` | `(lean_object*, lean_object*)` | `0` |
| A | `lean_verified_pmt_check` | `(lean_object* ×6)` | `0` |
| B | `lean_verify_transform` | `(lean_object*)` | `1` |
| B | `lean_verify_state_reads` | `(lean_object*, lean_object*)` | `1` |
| B | `lean_verify_state_writes` | `(lean_object*, lean_object*, lean_object*)` | `1` |
| A-prim | `lean_verified_capacity_check_prim` | `(uint64_t, uint64_t, uint64_t)` | `0` |
| A-prim | `lean_verified_field_bounds_check_prim` | `(uint64_t, uint64_t, uint64_t)` | `0` |
| A-prim | `lean_verified_linearity_check_prim` | `(lean_object*, lean_object*)` | `0` |
| A-prim | `lean_verified_pmt_check_prim` | `(uint64_t ×5, lean_object*, lean_object*)` | `0` |
| B-prim | `lean_verify_transform_prim` | `(lean_object*, lean_object*, lean_object*)` | `1` |
| B-prim | `lean_verify_state_reads_prim` | `(lean_object*, lean_object*)` | `1` |
| B-prim | `lean_verify_state_writes_prim` | `(lean_object*, lean_object*, lean_object*)` | `1` |

**`proof/extracted/README.md` confirms the deletion direction** (lines 1–7,
34–67, 122–144): the FFI bridge is **deleted** in the production codegen path
(`build.rs` no longer compiles `lean_stub.c`; no `lean_ffi_linked` cfg is
emitted; no `lean_verify_*` / `lean_verified_*` extern surface in
`src/codegen/src/runtime/pmt_check.rs`). The README also notes (lines 89–97)
that two test files (`tests/ffi_signature_conformance.rs` and
`tests/pmt_feature_flag_test.rs`) were already cleaned up; the three files in
this audit are the **remaining** FFI-bearing test files.

## 3. Per-file audit table

### 3.1 `tests/pmt_parity_test.rs` (975 lines)

| Field | Value |
|---|---|
| **`#[link]` + `extern "C"` block (primary)** | **Lines 152–189** (inside `mod lean_ffi`, feature-gated `#[cfg(feature = "pmt-runtime-check")]` at line 152); `#[link(name = "lean_extraction", kind = "static")]` at line 159; `extern "C" { ... }` spans lines 160–177 |
| **`#[link]` + `extern "C"` block (secondary)** | **Lines 549–552** (inside `mod lean_init`, gated `#[cfg(lean_ffi_linked)]` at line 549); declares `initialize_PMT(builtin: u8, w: *mut c_void) -> *mut c_void` |
| **Symbols declared (primary extern block)** | (1) `lean_verify_transform(layouts: *mut LeanObject, t: *mut LeanObject) -> u8` (line 161)  <br> (2) `lean_verify_state_reads(env_list: *mut LeanObject, reads: *mut LeanObject) -> u8` (line 162)  <br> (3) `lean_verify_state_writes(env_list: *mut LeanObject, consumed: *mut LeanObject, writes: *mut LeanObject) -> u8` (line 163)  <br> (4) `lean_verify_transform_prim(registry: *mut LeanObject, input_layout: *mut LeanObject, output_layout: *mut LeanObject, kind: *mut LeanObject) -> u8` (lines 166–171)  <br> (5) `lean_verify_state_reads_prim(registry: *mut LeanObject, reads: *mut LeanObject) -> u8` (line 172)  <br> (6) `lean_verify_state_writes_prim(registry: *mut LeanObject, consumed: *mut LeanObject, writes: *mut LeanObject) -> u8` (line 173)  <br> (7) `lean_mk_string(s: *const c_char) -> *mut LeanObject` (lines 175–176, gated `#[cfg(lean_ffi_linked)]`) |
| **Symbols declared (secondary extern block)** | `initialize_PMT(builtin: u8, w: *mut c_void) -> *mut c_void` (line 551) |
| **Symbols actually CALLED from test bodies** | None of the 7 primary externs are called directly from `#[test]` bodies. They are called **indirectly** via the cfg-polymorphic wrappers (`lean_verify_transform` at line 263 → `lean_ffi::lean_verify_transform_prim` at line 283; `lean_verify_state_reads` at line 322 → `lean_ffi::lean_verify_state_reads_prim` at line 328; `lean_verify_state_writes` at line 368 → `lean_ffi::lean_verify_state_writes_prim` at line 376), which the `#[test]` bodies invoke. `lean_mk_string` is also indirect, via `lean_ffi::str_to_lean` calls at lines 279–282, 326–327, 373–375. `initialize_PMT` is called via `lean_init::ensure_init()` at line 564 (gated `#[cfg(lean_ffi_linked)]`). <br>**Symbols (1)–(3) are declared but NEVER referenced anywhere in the file** — pure dead extern surface. |
| **Local hand-translations present?** | **YES** (these are the real test surface and must be PRESERVED): <br> • `lean_capacity_check(used, size, capacity) -> bool` (line 40) <br> • `lean_field_bounds_check(offset, size, total) -> bool` (line 47) <br> • `lean_linearity_check(var, consumed) -> bool` (line 52) <br> • `lean_wf_layout_bool(&Layout) -> bool` (referenced at lines 248, 249, 633, 641, 650, 658, 667) <br> • `hand_verify_transform(in_layout, out_layout, kind) -> bool` (line 243, gated `#[cfg(not(feature = "pmt-runtime-check"))]`) <br> • `hand_verify_state_reads(env, reads) -> bool` (line 301, same gate) <br> • `hand_verify_state_writes(env, consumed, writes) -> bool` (line 344, same gate) <br> • `hand_verify_state_reads_v2(env, ft_env, reads) -> bool` (line 411, no gate — no Lean export exists) <br> • `hand_verify_state_writes_v2(env, ft_env, consumed, writes) -> bool` (line 457, no gate — no Lean export exists) |
| **Imports from `vuma_codegen::runtime::pmt_check`?** | **NO.** The file duplicates the canonical hand-translations locally (as `lean_capacity_check` etc., with different names). The canonical `verified_*` symbols are not referenced. |
| **Stub-regime `#[cfg_attr(ignore)]` tests** | **8 tests** carry `#[cfg_attr(all(feature = "pmt-runtime-check", not(lean_ffi_linked)), ignore = "FFI stub returns hardcoded true; needs real Lean linkage (lean_ffi_linked)")]`: <br> 1. Line 678 — `parity_verify_transform_identity_fail_different_fields` <br> 2. Line 700 — `parity_verify_transform_reinterpret_fail_size_mismatch` <br> 3. Line 722 — `parity_verify_transform_rejects_ill_formed_in_layout` <br> 4. Line 743 — `parity_verify_state_reads_fail_unregistered_field` <br> 5. Line 757 — `parity_verify_state_reads_fail_out_of_bounds` <br> 6. Line 781 — `parity_verify_state_writes_fail_consumed_var` <br> 7. Line 794 — `parity_verify_state_writes_fail_unregistered_field` <br> 8. Line 808 — `parity_verify_state_writes_mixed` <br> **Note:** the prior 3-c-test commit message reports "9 ignored tests" across both files; that count is 8 (here) + 1 (the duration-gated `full_parity_all_1589_fixtures` in `pmt_parity_test_full.rs` — see §3.2). Only the 8 here are `lean_ffi_linked`-gated; the 9th is gated on duration, not stub-regime. |

### 3.2 `tests/pmt_parity_test_full.rs` (750 lines)

| Field | Value |
|---|---|
| **`#[link]` + `extern "C"` block (primary)** | **Lines 456–476** (inside `mod lean_ffi`, feature-gated `#[cfg(feature = "pmt-runtime-check")]` at line 456); `#[link(name = "lean_extraction", kind = "static")]` at line 463; `extern "C" { ... }` spans lines 464–475 |
| **Symbols declared (primary extern block)** | (1) `lean_verified_capacity_check_prim(used: u64, size: u64, capacity: u64) -> u8` (lines 470–474) |
| **Symbols actually CALLED from test bodies** | Called **indirectly** via `lean_capacity_crosscheck(_fixture)` at line 506: the `#[cfg(feature = "pmt-runtime-check")]` arm (lines 508–516) invokes `lean_ffi::lean_verified_capacity_check_prim(used, size, capacity)` at line 514. The function is called from `full_parity_all_1589_fixtures` (the `#[ignore]`'d master harness) at line 582. <br> **Direct `unsafe` call site:** line 514. |
| **Local hand-translations present?** | **YES** (must be PRESERVED): <br> • `hand_capacity_check(used: u64, size: u64, capacity: u64) -> u8` (line 485, gated `#[cfg(not(feature = "pmt-runtime-check"))]`) <br> The function `lean_capacity_crosscheck` (line 506) is cfg-polymorphic: the `#[cfg(feature = "pmt-runtime-check")]` arm calls the FFI; the `#[cfg(not(feature = "pmt-runtime-check"))]` arm (lines 517–520) calls `hand_capacity_check`. |
| **Imports from `vuma_codegen::runtime::pmt_check`?** | **NO.** The file duplicates only the capacity-check hand-translation locally (as `hand_capacity_check`). The other 3 canonical `verified_*` functions are not referenced. |
| **Stub-regime `#[cfg_attr(ignore)]` tests** | **0** (none). <br> The single `#[ignore]` at line 538 is a plain duration-based ignore (the 1 589-fixture master harness takes ~2–4 min), NOT a `lean_ffi_linked`-gated ignore. The README (lines 46–55) explicitly says this `#[ignore]` must be retained. <br> The cfg-polymorphic cross-check expected-count branches at lines 629–632 DO reference `lean_ffi_linked`: `#[cfg(all(feature = "pmt-runtime-check", lean_ffi_linked))] let expected_crosscheck: usize = total;` (line 629), `#[cfg(all(feature = "pmt-runtime-check", not(lean_ffi_linked)))] let expected_crosscheck: usize = 0;` (line 631). These two branches plus the `#[cfg(not(feature = "pmt-runtime-check"))]` branch at line 627 form a 3-way cfg dispatch that should collapse to a single `let expected_crosscheck: usize = total;` after the FFI removal. |

### 3.3 `tests/pmt_extraction_diff.rs` (268 lines)

| Field | Value |
|---|---|
| **`#[link]` + `extern "C"` block (primary)** | **Lines 93–97** (top-level, NOT inside a `mod`); `#[link(name = "lean_extraction", kind = "static")]` at line 93; `extern "C" { ... }` spans lines 94–97. The block is a "link anchor" — its only purpose (per the comment at lines 83–92) is to attach the `#[link]` attribute so the linker searches `liblean_extraction.a` for the externs declared in Copy B's `lean_ffi` module (see below). |
| **Symbols declared (primary extern block)** | (1) `_diff_link_anchor() -> u8` (lines 95–96) with `#[link_name = "lean_verify_state_writes"]` — i.e. the Rust symbol `_diff_link_anchor` is bound to the C symbol `lean_verify_state_writes`. The function is **never called** (intentionally — see comment at lines 256–267); only the link attribute matters. |
| **Symbols actually CALLED from test bodies** | `_diff_link_anchor` is **never called**. The link anchor exists solely to pull `liblean_extraction.a` into the linker search path so that the `extern "C"` declarations in Copy B's `lean_ffi` module (see "Copy B note" below) resolve. The 5 `#[test]` bodies (lines 177–268) call only `parity_test::lean_*` (Copy A) and `pmt_check::verified_*` (Copy B), neither of which goes through FFI. |
| **Local hand-translations present?** | **NO** (this file is a pure differential harness). It imports Copy A and Copy B via `#[path]`: <br> • `#[path = "../proof/extracted/pmt_check.rs"] mod pmt_check;` (lines 69–70) — Copy B <br> • `#[path = "pmt_parity_test.rs"] mod parity_test;` (lines 80–81) — Copy A <br> The 5 `#[test]` functions (`diff_capacity_check`, `diff_field_bounds_check`, `diff_linearity_check`, `diff_pmt_check_composed`, `link_anchor_compiles`) compare Copy A's `lean_*` against Copy B's `verified_*`. The 5th test (`link_anchor_compiles`, line 259) is a build-time assertion that the link anchor compiles and links — it has no runtime assertion and exists only to document that the `#[link]` anchor pulls in `liblean_extraction.a`. <br> **Copy B note:** `proof/extracted/pmt_check.rs` is NOT in this audit's scope, but it ALSO contains a `lean_ffi` module (lines 100–377, gated `#[cfg(feature = "pmt-runtime-check")]`) with its own 7-symbol `extern "C"` block (lines 117–194) — without its own `#[link]` attribute, it relies on this file's anchor. The audit of `proof/extracted/pmt_check.rs` is **out of scope** for F1-a but flagged here because the removal plan for `pmt_extraction_diff.rs` cannot be completed without also touching that file (see §4.3). |
| **Imports from `vuma_codegen::runtime::pmt_check`?** | **NO.** Copy B is imported via `#[path = "../proof/extracted/pmt_check.rs"]` (the standalone copy in `proof/extracted/`), NOT via `use vuma_codegen::runtime::pmt_check::*` (the in-tree codegen copy). The two copies differ: the codegen copy has `lean_ffi` removed (per Wave 3 `3-b-audit`); the proof/extracted copy still has `lean_ffi` (lines 100–377). |
| **Stub-regime `#[cfg_attr(ignore)]` tests** | **0** (none). The whole file is gated `#![cfg(feature = "pmt-runtime-check")]` at line 63. |

## 4. Removal plan (per-file, exact line ranges)

The cleanup direction (per prior 3-c-test commit + `proof/extracted/README.md`)
is to make `pmt-runtime-check` a **true no-op** in the test files too: remove
all `#[link]` / `extern "C"` / `lean_ffi` modules / `lean_init` modules, and
collapse every cfg-polymorphic dispatch to its hand-translated branch. The
canonical reference end-state is `src/codegen/src/runtime/pmt_check.rs` (no
FFI surface, pure Rust).

### 4.1 `tests/pmt_parity_test.rs` — proposed removals

| # | Line range | What to remove | Notes |
|---|---|---|---|
| 1 | **Lines 111–189** | The entire `mod lean_ffi` block (header comment + `#[cfg(feature = "pmt-runtime-check")] mod lean_ffi { ... }` with `LeanObject` type, `#[link]`, 7-symbol `extern "C"` block, `str_to_lean` helpers) | Includes the comment block at lines 111–150 explaining the FFI bridge. After removal, no `#[link(name = "lean_extraction", …")]` remains in this file. |
| 2 | **Lines 259–284** | The `#[cfg(feature = "pmt-runtime-check")] fn lean_verify_transform(...)` FFI-routing variant (calls `lean_ffi::lean_verify_transform_prim`) | The `#[cfg(not(feature = "pmt-runtime-check"))] fn lean_verify_transform(...)` delegate (lines 286–293) should be retained but its `#[cfg(not(...))]` gate removed, so `lean_verify_transform` always delegates to `hand_verify_transform`. |
| 3 | **Lines 320–329** | The `#[cfg(feature = "pmt-runtime-check")] fn lean_verify_state_reads(...)` FFI-routing variant | Same pattern: retain lines 331–337 (`#[cfg(not(...))]` delegate), drop the gate. |
| 4 | **Lines 366–377** | The `#[cfg(feature = "pmt-runtime-check")] fn lean_verify_state_writes(...)` FFI-routing variant | Same pattern: retain lines 379–386 (`#[cfg(not(...))]` delegate), drop the gate. |
| 5 | **Lines 503–574** | The entire `mod lean_init` block (header comment + `#[cfg(lean_ffi_linked)]` extern `initialize_PMT` + `ensure_init` variants) | After removal, all `lean_init::ensure_init();` calls in `#[test]` bodies (lines 671, 681, 691, 703, 713, 725, 734, 746, 760, 771, 783, 796, 811) become dangling and must also be removed (one-line deletion each). |
| 6 | **Line 671, 681, 691, 703, 713, 725, 734, 746, 760, 771, 783, 796, 811** | The 13 `lean_init::ensure_init();` statements at the heads of the state-verifier `#[test]` bodies | After step 5 these are undefined references. Delete each line (one statement per test). |
| 7 | **Lines 678, 700, 722, 743, 757, 781, 794, 808** | The 8 `#[cfg_attr(all(feature = "pmt-runtime-check", not(lean_ffi_linked)), ignore = "...")]` annotations on the stub-regime tests | After FFI removal, the `#[cfg(feature = "pmt-runtime-check")]` branch of each `lean_verify_*` is gone, so the call always goes through `hand_verify_*` (the hand-translation). These 8 tests would then exercise the hand-translation directly — they would PASS (hand-translation correctly rejects the negative cases), so the `#[ignore]` is no longer needed. **CAUTION:** verify this assumption by running `cargo test --test pmt_parity_test` after edits; if any test fails, the hand-translation has a bug and the test should be left `#[ignore]`'d with a new rationale. |
| 8 | (No change) | **Lines 39–72** (`lean_capacity_check`, `lean_field_bounds_check`, `lean_linearity_check`, `lean_wf_layout_bool`, `Field`, `Layout`, `TransformKind` definitions) | PRESERVE — these are the local hand-translations that form the real test surface. |
| 9 | (No change) | **Lines 242–257, 300–318, 343–364, 411–441, 457–490, 445–451, 493–501** (the `hand_verify_*` and `lean_verify_*_v2` functions) | PRESERVE — local hand-translations. After step 2–4, the `hand_` prefix on the three non-`_v2` functions becomes the only definition; optionally rename `hand_verify_transform` → `lean_verify_transform` (etc.) for naming consistency and delete the now-redundant delegate wrappers (lines 286–293, 331–337, 379–386). |

**Net effect:** ~150–180 lines removed; the file becomes a pure-Rust parity
test with no FFI surface; `#[cfg(feature = "pmt-runtime-check")]` no longer
gates any code in this file (the local hand-translations are always compiled,
matching the canonical end-state in `src/codegen/src/runtime/pmt_check.rs`).

### 4.2 `tests/pmt_parity_test_full.rs` — proposed removals

| # | Line range | What to remove | Notes |
|---|---|---|---|
| 1 | **Lines 454–476** | The entire `mod lean_ffi` block (`#[cfg(feature = "pmt-runtime-check")] mod lean_ffi { ... }` with `#[link]` and the single-symbol `extern "C"` block declaring `lean_verified_capacity_check_prim`) | The preceding comment block (lines 423–452) explains the FFI routing; trim or remove the now-stale portions. |
| 2 | **Lines 484–485** | The `#[cfg(not(feature = "pmt-runtime-check"))]` gate on `hand_capacity_check` | Drop the gate so `hand_capacity_check` is always compiled. |
| 3 | **Lines 506–521** | Rewrite `lean_capacity_crosscheck` to drop the `#[cfg(feature = "pmt-runtime-check")]` arm (lines 508–516) and the `#[cfg(not(feature = "pmt-runtime-check"))]` arm wrapper (lines 517–520); collapse to a single body that calls `hand_capacity_check(used, size, capacity) == 1`. | The function signature (`fn lean_capacity_crosscheck(_fixture: &Fixture) -> bool`) is unchanged; only the body collapses. |
| 4 | **Lines 627–632** | Collapse the 3-way `cfg` dispatch on `expected_crosscheck` to a single `let expected_crosscheck: usize = total;` | Remove the `#[cfg(all(feature = "pmt-runtime-check", lean_ffi_linked))]` and `#[cfg(all(feature = "pmt-runtime-check", not(lean_ffi_linked)))]` branches; keep only the `#[cfg(not(feature = "pmt-runtime-check"))]` value (`total`). |
| 5 | **Lines 65–83** | Trim the doc-comment block titled "Lean cross-check (Wave 6-C cfg branch)" to remove references to the FFI path | The block at lines 65–83 describes the now-deleted cfg dispatch; replace with a one-paragraph note that the cross-check uses the local hand-translation. |
| 6 | (No change) | **Line 538** `#[ignore]` on `full_parity_all_1589_fixtures` | PRESERVE per README lines 46–55 — this is a duration-based ignore, NOT a stub-regime ignore. |
| 7 | (No change) | All `#[test]` bodies (lines 537–749) | PRESERVE — they call `lean_capacity_crosscheck`, `run_ive_on_fixture`, `check_parity`, etc., none of which goes through FFI after step 3. |

**Net effect:** ~30–50 lines removed; the file becomes a pure-Rust parity
harness; the only `#[cfg(...)]` remaining is the `#[ignore]` on the master
harness (duration-based, retained per README).

### 4.3 `tests/pmt_extraction_diff.rs` — proposed removals

| # | Line range | What to remove | Notes |
|---|---|---|---|
| 1 | **Lines 83–97** | The link-anchor comment block (lines 83–92) plus the `#[link(name = "lean_extraction", kind = "static")] extern "C" { ... }` block (lines 93–97) declaring `_diff_link_anchor` | After removal, no `#[link(name = "lean_extraction", …")]` remains in this file. |
| 2 | **Lines 256–268** | The `link_anchor_compiles` `#[test]` function (lines 259–268) and its preceding doc-comment (lines 256–258) | This test existed solely to assert the link anchor compiles; with the anchor gone, the test has no purpose. |
| 3 | **Lines 24–61, 83–92** | Trim doc-comment blocks that reference the FFI bridge / link anchor / stub archive | Specifically the "Linkage strategy" block (lines 24–50) and the "Link anchor" block (lines 83–92). Replace with a one-paragraph note that Copy A and Copy B are both pure-Rust hand-translations compared directly. |
| 4 | **(Out of scope)** `proof/extracted/pmt_check.rs` lines **100–377** | The `lean_ffi` module (with 7-symbol `extern "C"` block + safe-bool wrappers) in Copy B | **This file is OUT OF SCOPE for F1-a-audit** but the removal is a **hard dependency** for step 1 above: if the top-level `#[link]` anchor in `pmt_extraction_diff.rs` is removed but Copy B's `lean_ffi` module (pulled in via `#[path = "../proof/extracted/pmt_check.rs"]` at line 69) is still compiled, its `extern "C"` declarations will produce undefined-symbol link errors (the linker will no longer search `liblean_extraction.a`). **Two options for the downstream editor:** <br> (a) **Delete `lean_ffi` from `proof/extracted/pmt_check.rs`** (lines 100–377) — symmetric to the Wave 3 `3-b-audit` cleanup of `src/codegen/src/runtime/pmt_check.rs`. This is the cleanest end-state. <br> (b) **Switch Copy B's import** from `#[path = "../proof/extracted/pmt_check.rs"]` to `use vuma_codegen::runtime::pmt_check::*` — uses the canonical in-tree copy where `lean_ffi` is already removed. This drops the dependency on the standalone `proof/extracted/pmt_check.rs` entirely. <br> The downstream editor should pick option (a) or (b) before merging step 1 of this removal plan; otherwise `cargo test --test pmt_extraction_diff --features pmt-runtime-check` will fail to link. |
| 5 | (No change) | **Lines 99–168** (corpus constants), **Lines 170–254** (the 4 differential `#[test]` functions) | PRESERVE — these are the real differential test surface (Copy A vs Copy B, both pure Rust). |
| 6 | (Optional) **Line 63** | The file-level `#![cfg(feature = "pmt-runtime-check")]` gate | After steps 1–4, the file no longer depends on the feature. **OPTIONAL:** drop the gate so the differential test runs in default `cargo test` (no `--features`); or retain the gate if the test is too slow for default CI. **Recommendation:** drop the gate to maximize differential coverage in default CI, unless the `proof/extracted/pmt_check.rs` Copy B is also feature-gated (it is, at its line 1 — `#![cfg(feature = "pmt-runtime-check")]`? — verify before dropping the gate). |

**Net effect:** ~30–40 lines removed in-scope; **+1 out-of-scope file**
(`proof/extracted/pmt_check.rs`) requires parallel cleanup (option (a)) OR
a one-line import swap (option (b)) for the link to succeed.

## 5. Aggregate removal statistics

| File | Lines (current) | Lines to remove (in-scope) | Out-of-scope deps | Stubs-regime `#[cfg_attr(ignore)]` tests to un-ignore |
|---|---|---|---|---|
| `tests/pmt_parity_test.rs` | 975 | ~150–180 (mod lean_ffi + mod lean_init + 3 cfg-routing variants + 8 cfg_attr annotations + 13 `ensure_init` calls) | none | 8 |
| `tests/pmt_parity_test_full.rs` | 750 | ~30–50 (mod lean_ffi + cfg dispatch in `lean_capacity_crosscheck` + 3-way `expected_crosscheck` collapse) | none | 0 (the `#[ignore]` at line 538 is duration-based, retained per README) |
| `tests/pmt_extraction_diff.rs` | 268 | ~30–40 (link-anchor extern block + `link_anchor_compiles` test + stale doc comments) | **`proof/extracted/pmt_check.rs` lines 100–377** (the `lean_ffi` module of Copy B — must also be removed OR the import must switch to `vuma_codegen::runtime::pmt_check::*`) | 0 |
| **Totals** | **1 993** | **~210–270 in-scope + 1 out-of-scope file** | 1 file | **8 tests un-ignored** |

## 6. Cross-file consistency check

| Question | Answer |
|---|---|
| Do any of the 3 test files import the canonical `vuma_codegen::runtime::pmt_check`? | **NO.** All 3 files use either local duplicates (`lean_capacity_check`, `hand_capacity_check`) or the standalone `proof/extracted/pmt_check.rs` copy via `#[path]`. None reference the canonical in-tree module that Wave 3 `3-b-audit` already cleaned up. |
| Do any of the 3 test files declare `lean_mk_string` or `initialize_PMT`? | `lean_mk_string`: only `tests/pmt_parity_test.rs` (line 176, gated `#[cfg(lean_ffi_linked)]`). <br> `initialize_PMT`: only `tests/pmt_parity_test.rs` (line 551, gated `#[cfg(lean_ffi_linked)]`). <br> Neither symbol is defined in `lean_stub.c` (Wave 4-C `lean_mk_string` is a Lean-runtime symbol; `initialize_PMT` is a Lean-generated module initialiser). Both would link-fail if the `lean_ffi_linked` cfg were ever emitted; the cfg is never emitted (per `proof/extracted/README.md` lines 129–131), so these declarations are dead code today. |
| Are the 8 stub-regime `#[cfg_attr(ignore)]` tests in `pmt_parity_test.rs` correct to un-ignore after FFI removal? | **Mostly yes.** All 8 tests assert that `lean_verify_*` returns `false` on a negative case. After FFI removal, `lean_verify_*` delegates to `hand_verify_*`, which is a hand-translation of the same Lean semantics. The hand-translations correctly reject negative cases (they have been verified to do so under the `#[cfg(not(feature = "pmt-runtime-check"))]` path in prior CI runs — `cargo test --test pmt_parity_test` exits 0 with all 26 non-ignored tests passing per the 3-c-test commit). **Caveat:** the editor should run `cargo test --test pmt_parity_test` after the FFI removal to confirm 26→34 tests pass (26 + 8 newly-un-ignored); if any of the 8 fails, the hand-translation has a latent bug. |
| Will `cargo test --features pmt-runtime-check` still work after the cleanup? | **YES.** With all `#[link(name = "lean_extraction", …")]` directives removed from the 3 test files (and from `proof/extracted/pmt_check.rs` if option (a) is taken), the feature becomes a true no-op for tests too — matching the production codegen path. `liblean_extraction.a` is no longer needed in `$HOME/.local/lib/` (the prior 3-c-test run's build artifact). The feature flag still activates the canonical `pmt_check` module in `vuma-codegen`, but that is pure Rust (no link deps). |

## 7. DoD for this audit (F1-a-audit)

| DoD criterion | Met? | Evidence |
|---|---|---|
| Audit markdown exists at `scripts/audit/followup_wave1_ffi_audit.md` | YES | This file |
| Audit includes per-file `#[link]` + `extern "C"` block line ranges | YES | §3.1, §3.2, §3.3 (column 2 of each table) |
| Audit lists symbols declared in each extern block | YES | §3.1, §3.2, §3.3 (column 3) |
| Audit lists symbols actually called from test bodies (with call-site line numbers) | YES | §3.1, §3.2, §3.3 (column 4) |
| Audit notes whether each test file has local hand-translations | YES | §3.1, §3.2, §3.3 (column 5) |
| Audit notes whether each test file imports from `vuma_codegen::runtime::pmt_check` | YES | §3.1, §3.2, §3.3 (column 6) |
| Audit lists `proof/extracted/lean_stub.c` exported symbols | YES | §2 (14-symbol table) |
| Audit confirms FFI bridge deletion direction per `proof/extracted/README.md` | YES | §2 (paragraph 2) |
| Audit includes a per-file removal plan with exact line ranges | YES | §4.1, §4.2, §4.3 |
| No source files edited (READ-ONLY audit) | YES | `git status --short` clean except for the new audit markdown |

## 8. Conclusion

The 3 test files contain **3 `#[link]` + `extern "C"` blocks** total
(`pmt_parity_test.rs` has 2 — the primary `lean_ffi` module and the secondary
`lean_init` module; `pmt_parity_test_full.rs` has 1; `pmt_extraction_diff.rs`
has 1 link-anchor block). Together they declare **9 unique extern symbols**
(7 in `pmt_parity_test.rs::lean_ffi` + 1 in `pmt_parity_test.rs::lean_init` +
1 in `pmt_parity_test_full.rs::lean_ffi` + 1 link-anchor in
`pmt_extraction_diff.rs`; one symbol — `lean_verify_state_writes` — is
referenced by both `pmt_parity_test.rs` line 163 and
`pmt_extraction_diff.rs` line 95). The cleanup removes **~210–270 in-scope
lines** across the 3 files, un-ignores **8 stub-regime tests** in
`pmt_parity_test.rs`, and flags **1 out-of-scope file**
(`proof/extracted/pmt_check.rs`) that requires parallel `lean_ffi` removal
(or an import swap to `vuma_codegen::runtime::pmt_check::*`) for the
differential test to keep linking.

The downstream editor (F1-b or similar) should:

1. Apply §4.1 → §4.2 → §4.3 in order.
2. Resolve the §4.3 step 4 out-of-scope dependency BEFORE applying §4.3 step 1.
3. Run `cargo test --test pmt_parity_test`, `cargo test --test
   pmt_parity_test_full -- --include-ignored`, and `cargo test --test
   pmt_extraction_diff --features pmt-runtime-check` after each file's edits.
4. Verify the 8 newly-un-ignored tests in `pmt_parity_test.rs` all PASS (if
   any fail, the hand-translation has a latent bug — investigate before
   re-ignoring).
5. Optionally delete `$HOME/.local/lib/liblean_extraction.a` (the prior
   3-c-test build artifact) — no longer needed after the cleanup.
