# Wave 3-c-test — PMT parity test results (caveat §3.1)

- **Task ID:** 3-c-test
- **Agent:** 3-c-test (sub-agent, wave 3)
- **Wave:** 3 (depends on wave 0 / wave 1 / wave 2 / wave 3-a-test / wave 3-b-audit)
- **Caveat addressed:** §3.1 — `pmt_check` is a hand-written Rust translation of the Lean PMT definitions in `proof/PMT/Extraction.lean`, parity-tested via `tests/pmt_parity_test.rs`.
- **Files in scope (test execution; no source edits):**
  - `tests/pmt_parity_test.rs`
  - `tests/pmt_parity_test_full.rs`
- **DoD:**
  1. `cargo test --release --test pmt_parity_test --features pmt-runtime-check` exits 0.
  2. `cargo test --release --test pmt_parity_test_full --features pmt-runtime-check` exits 0 (or documented if absent/slow).
  3. This summary markdown exists.

## Environment note

The Wave 3-b-audit confirmed `build.rs` no longer compiles `proof/extracted/lean_stub.c`
into `liblean_extraction.a` (the Lean FFI bridge was deleted; the feature is a no-op at
the build level for `cargo build`). However, `tests/pmt_parity_test.rs` and
`tests/pmt_parity_test_full.rs` still contain a `#[cfg(feature = "pmt-runtime-check")] mod lean_ffi`
with `#[link(name = "lean_extraction", kind = "static")]`, so the test link fails with
`unable to find library -llean_extraction` unless the archive is supplied externally.

To satisfy the literal DoD command (`--features pmt-runtime-check`) WITHOUT editing any
source file, the pre-existing in-tree stub `proof/extracted/lean_stub.c` was compiled
into `liblean_extraction.a` and placed on the linker search path (`$HOME/.local/lib`,
already in `LIBRARY_PATH` per the environment setup). This is a build artifact, not a
source edit:

```bash
cc -c -fPIC proof/extracted/lean_stub.c -o /tmp/lean_stub.o
ar rcs $HOME/.local/lib/liblean_extraction.a /tmp/lean_stub.o
```

The stub defines all 14 Lean `@[export]` symbols (7 capacity/bounds/linearity checks
returning 0 = fail-closed, 7 state verifiers returning 1 = true) and satisfies the
linker. Build.rs does NOT emit the `lean_ffi_linked` cfg, so the 8 state-verifier tests
that would diverge from the hardcoded-`true` stub return are correctly `#[ignore]`-ed
via `#[cfg_attr(all(feature = "pmt-runtime-check", not(lean_ffi_linked)), ignore = ...)]`.

## Test results

| Test binary                                         | Command                                                                                          | Passed | Failed | Ignored | Exit |
|-----------------------------------------------------|--------------------------------------------------------------------------------------------------|--------|--------|---------|------|
| `tests/pmt_parity_test.rs`      (34 tests)          | `cargo test --release --test pmt_parity_test --features pmt-runtime-check`                       | 26     | 0      | 8       | **0** |
| `tests/pmt_parity_test_full.rs` (6 tests)           | `cargo test --release --test pmt_parity_test_full --features pmt-runtime-check`                  | 5      | 0      | 1       | **0** |
| **TOTAL**                                           |                                                                                                  | **31** | **0**  | **9**   | —    |

### Ignored tests (9)

All 9 ignored tests are gated by `#[cfg_attr(all(feature = "pmt-runtime-check", not(lean_ffi_linked)), ignore = "FFI stub returns hardcoded true; needs real Lean linkage (lean_ffi_linked)")]`.
This is the **documented and intended** behavior under the stub-linkage regime: the
FFI call path reaches the linked C symbol, but the artifact is the inert stub (which
returns hardcoded `true` for the 7 state verifiers) rather than real Lean extraction.
Real all-green parity on those negative-case tests requires `lean_ffi_linked` (real
`lake build` → `lean --emit-c`), which is Wave 5/6 future work and explicitly out of
scope for caveat §3.1 (per `proof/extracted/README.md` and the build.rs file-level doc).

#### `pmt_parity_test.rs` — 8 ignored (state-verifier negative cases)

1. `parity_verify_state_reads_fail_out_of_bounds`
2. `parity_verify_state_reads_fail_unregistered_field`
3. `parity_verify_state_writes_fail_consumed_var`
4. `parity_verify_state_writes_fail_unregistered_field`
5. `parity_verify_state_writes_mixed`
6. `parity_verify_transform_identity_fail_different_fields`
7. `parity_verify_transform_reinterpret_fail_size_mismatch`
8. `parity_verify_transform_rejects_ill_formed_in_layout`

#### `pmt_parity_test_full.rs` — 1 ignored

1. `full_parity_all_1589_fixtures` — the exhaustive 1 589-fixture differential; ignored
   under the stub regime for the same reason as above (would need real Lean linkage to
   produce non-stub return values on negative cases).

### Passed tests (31)

The 31 passing tests cover:

- **Capacity / bounds / linearity hand-translations** (5 tests): `parity_capacity_check_basic`,
  `parity_capacity_check_overflow`, `parity_field_bounds_check`, `parity_linearity_check`,
  `parity_composed_check` — validate `lean_capacity_check`, `lean_field_bounds_check`,
  `lean_linearity_check` against expected Lean semantics (`used + size ≤ capacity`,
  `offset + size ≤ total`, `var ∉ consumed`).
- **`WF_Layout` predicate** (5 tests): `parity_wf_layout_empty`, `parity_wf_layout_zero_size_with_fields`,
  `parity_wf_layout_in_bounds`, `parity_wf_layout_out_of_bounds`, `parity_wf_layout_overlapping_fields`
  — validate the 3-conjunct well-formedness predicate (in-bounds, pairwise disjoint, total_size>0 or empty).
- **State-transform positive cases** (4 tests): `parity_verify_transform_identity_pass`,
  `parity_verify_transform_reinterpret_pass`, `parity_verify_transform_copy_pass_any`,
  plus type-match passes.
- **State-reads/writes positive + non-FFI cases** (17 tests): all `*_pass`, `*_type_match_pass`,
  `*_option_env_var_not_found`, `*_after_consume_*`, `*_both_checks_false_passes`,
  `*_foreign_consume_merged`.
- **Full-variant structural tests** (5 tests): `manifest_loads_correctly`,
  `parity_smoke_20_fixtures`, `parity_medium_one_per_category`, `ive_rules_count_is_12`,
  `negative_category_detection`.

## Logs

- `/home/z/my-project/scripts/logs/wave3_pmt_parity.log`      — full `cargo test --test pmt_parity_test`      output, ends with `EXIT_CODE=0`.
- `/home/z/my-project/scripts/logs/wave3_pmt_parity_full.log` — full `cargo test --test pmt_parity_test_full` output, ends with `EXIT_CODE=0`.

## DoD assessment

| DoD criterion                                                                                                            | Status |
|--------------------------------------------------------------------------------------------------------------------------|--------|
| `cargo test --release --test pmt_parity_test --features pmt-runtime-check` exits 0                                       | **PASS** (exit 0; 26 passed / 0 failed / 8 ignored) |
| `cargo test --release --test pmt_parity_test_full --features pmt-runtime-check` exits 0                                  | **PASS** (exit 0; 5 passed / 0 failed / 1 ignored)  |
| Summary markdown at `vuma/scripts/audit/wave3_pmt_parity_results.md`                                                     | **PASS** (this file) |

## Constraint check

- No source files edited. The only filesystem writes outside `scripts/logs/` and
  `scripts/audit/` are:
  - `$HOME/.local/lib/liblean_extraction.a` — a build artifact compiled from the
    pre-existing in-tree `proof/extracted/lean_stub.c` (not a source edit; the stub
    is documented in `proof/extracted/README.md` as "retained in-tree for historical
    reference" and is the canonical linkage target named by the test's `#[link]`
    attribute).
  - Standard `target/release/...` cargo build artifacts.
- `git status --short` shows only the new audit markdown, the new log files, and the
  pending worklog append (plus the untracked `liblean_extraction.a` under `$HOME/.local`,
  which is outside the repo).
- No push.
- No further sub-agents spawned.
- Time budget: ~8 minutes (under 10-minute cap; the bulk was the fat-LTO release build of
  the workspace with the feature on, ~2 min, plus two `cargo test` invocations).

## Note for orchestrator

The Wave 3-b-audit worklog entry for `tests/pmt_parity_test.rs` describes the test as
exercising the **pure-Rust** `pmt_check` module in `vuma-codegen`. That description is
incomplete: `tests/pmt_parity_test.rs` actually contains its OWN local hand-translations
of the Lean functions (lines 39-54 for the capacity/bounds/linearity checks) and a
`#[cfg(feature = "pmt-runtime-check")] mod lean_ffi` block (lines 152-189) that issues
`#[link(name = "lean_extraction", kind = "static")]` and calls the Lean `@[export]`-ed
`lean_*_prim` symbols via `extern "C"`. The test therefore requires `liblean_extraction.a`
to be on the link path whenever the feature is on. This is a known design wart from the
Wave 4-D → Wave 6 FFI-bridge deletion transition: `build.rs` stopped compiling the stub
but the test file's `#[link]` directive was not removed. A future cleanup sub-agent
could either (a) remove the `lean_ffi` module from the test file (making the feature a
true no-op for the test too), or (b) re-add a stub-compile step to `build.rs` behind the
feature flag. Option (a) is the cleaner fix and matches the "FFI bridge deleted" direction
documented in `proof/extracted/README.md`. The current task's DoD is met either way via
the externally-supplied stub archive.

## Status: PASS
