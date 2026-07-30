# Wave 3 — IVE Z3 Discharge Rate Audit (Caveat §3.1)

- **Task ID:** 3-a-test
- **Agent:** 3-a-test (sub-agent, wave 3)
- **Wave:** 3 (depends on wave 0: env / wave 1: build baseline / wave 2: allocator+stack-slot audits)
- **Caveat addressed:** §3.1 — Z3 discharges IVE verification conditions
- **DoD:** Average Z3 discharge rate ≥ 99% across sampled `.vuma` files; cargo test for `ive_loop_tests` and `verification_tests` exits 0.

## Method

1. Loaded Z3 / Rust / Lean env shims from `scripts/env/*.sh`.
2. Confirmed `target/release/compile_dump` (the IVE verifier driver built in wave 1-d5) emits a summary line of the form:
   ```
   IVE: Pass passed=N failed=N unverified=N total=N discharge_rate=NN%
   ```
   `discharge_rate = passed / total` where `total = passed + failed + unverified`. `unverified` counts IVE conditions that were *postponed* (i.e., not discharged by Z3). Hence `discharge_rate` is exactly the metric specified by caveat §3.1.
3. Invoked `compile_dump <source.vuma> <output.bin> x86_64 --verify` on every `.vuma` file in the five required gold-standard categories (`u32_arith`, `complex_stores`, `multi_function`, `crypto_patterns`, `concurrency`).
4. Aggregated per-file `passed / failed / unverified / discharge_rate` into per-category and corpus-wide averages.
5. Ran `cargo test --release --test ive_loop_tests --test verification_tests` (with Z3 env sourced so the `-lz3` link succeeds) and recorded exit code and per-test results.

## Per-Category Discharge Rate (exhaustive sweep, all .vuma files in category)

| Category          | Files | Discharged (passed) | Failed | Unverified | Avg per-file rate |
|-------------------|------:|--------------------:|-------:|-----------:|-------------------:|
| u32_arith         |    96 |                  96 |      0 |          0 |             100.00% |
| complex_stores    |    94 |                  94 |      0 |          0 |             100.00% |
| multi_function    |    77 |                  77 |      0 |          0 |             100.00% |
| crypto_patterns   |   105 |                 105 |      0 |          0 |             100.00% |
| concurrency       |    56 |                  56 |      0 |          0 |             100.00% |
| **TOTAL**         | **428** |            **428** |  **0** |      **0** |       **100.00%**   |

- Corpus-wide weighted discharge rate (passed / (passed+failed+unverified)): **100.00%**
- Average per-file discharge rate: **100.00%**

Sample representative file outputs (all 11 protocol samples = 100%):

```
tests/gold_standard/u32_arith/u32_add.vuma            IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
tests/gold_standard/u32_arith/u32_2_mul.vuma          IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
tests/gold_standard/u32_arith/u32_add_chain_4.vuma    IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
tests/gold_standard/complex_stores/cs2_chain.vuma     IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
tests/gold_standard/complex_stores/cs2_double_buf.vuma IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
tests/gold_standard/multi_function/mf_calculator.vuma IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
tests/gold_standard/multi_function/mf_chain_5.vuma    IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
tests/gold_standard/crypto_patterns/crypto2_xor.vuma  IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
tests/gold_standard/crypto_patterns/crypto2_popcount.vuma IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
tests/gold_standard/concurrency/conc_swap.vuma        IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
tests/gold_standard/concurrency/conc_four_cells.vuma  IVE: Pass passed=1 failed=0 unverified=0 total=1 discharge_rate=100%
```

Full per-file log: `/home/z/my-project/scripts/logs/wave3_compile_dump_full.log`.
Representative-sample log: `/home/z/my-project/scripts/logs/wave3_compile_dump_sample.log`.

## Cargo Test Results

Command:
```
cargo test --release --test ive_loop_tests --test verification_tests
```
Environment: Z3 env (`scripts/env/z3-env.sh`) sourced so `-lz3` resolves to `$HOME/.local/lib`.

Result: **EXIT_CODE=0** (all tests passed).

Per-test breakdown:
- `ive_loop_tests`: **15 passed; 0 failed; 0 ignored**
- `verification_tests`: **5 passed; 0 failed; 0 ignored**
  - `wave7_all_rules_verified_sound` ... ok
  - `wave7_assert_all_rules_sound_api` ... ok
  - `wave7_verification_detects_unsound_rules` ... ok
  - `wave7_verification_evaluates_all_cases` ... ok
  - `wave7_verify_count_meets_minimum` ... ok

Full log: `/home/z/my-project/scripts/logs/wave3_ive_cargo.log`.

## DoD Assessment

| DoD criterion                                                                | Status |
|------------------------------------------------------------------------------|--------|
| Average Z3 discharge rate ≥ 99% across sampled .vuma files                   | **PASS** (100.00% across 428 files) |
| `cargo test --release --test ive_loop_tests --test verification_tests` exits 0 | **PASS** (exit 0; 20/20 tests ok) |
| Summary markdown exists at `vuma/scripts/audit/wave3_ive_discharge.md`       | **PASS** (this file) |

## Conclusion

Caveat §3.1 is satisfied: Z3 discharges **100%** of IVE verification conditions
(contract / invariant / linearity / information-flow `ensures` clauses) across
the IVE gold-standard corpus — 428/428 conditions passed, 0 failed, 0 postponed.
No `unverified` (postponed) conditions remain. The 1-d5 fix that added the
`discharge_rate` metric to `compile_dump`'s summary output is functioning as
designed.

No source files were modified; this task is read-only audit + test execution.
