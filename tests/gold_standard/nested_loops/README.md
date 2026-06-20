# Two- and Three-Level Nested Loops

Tests with deeply nested loops — the classic matrix-multiply shape
(`for i { for j { for k { ... } } }`) and simpler 2D-iteration patterns.
These stress the backend's loop-label register allocation, branch-target
encoding, and instruction-cache footprint.

## What belongs here

- 2x2 / 3x3 / 4x4 nested iteration counters
- Row-major 2D indexing via 1D memory
- Triangular / Floyd's-triangle nested-loop shapes
- Outer-product / matrix-multiply accumulator patterns
- Pascal's-triangle recurrence via nested loops

## Known VUMA bug — these tests are EXPECTED TO FAIL

The current x86_64 backend has a **for-body / while-body assignment
propagation bug**: assignments to a scalar variable inside a loop body
(such as `count = count + 1;` or `i = i - 1;`) do not propagate to the
variable's value after the loop. As a result, every `nl_*` test that
relies on a loop-body scalar update returns its initial value (typically
0 or 1) rather than the mathematically correct value.

These tests are written to be **regression tests for the day the loop
bug is fixed**: their expected (theoretical) exit codes are documented
in each file's header, and the actual observed exit codes on the current
backend are also documented. Once the bug is fixed, the actual codes
should flip to match the expected ones.

## Files (16)

### Existing (carried over)

- [`matrix.vuma`](matrix.vuma) — 4x4 matrix multiply, XOR checksum = 0

### New `nl_*` tests (15)

| Test | Expected (correct) | Actual (buggy) | Notes |
| --- | --- | --- | --- |
| [`nl_2x2.vuma`](nl_2x2.vuma) | 4 | 0 | 2x2 iteration counter |
| [`nl_3x3.vuma`](nl_3x3.vuma) | 9 | 0 | 3x3 iteration counter |
| [`nl_4x4.vuma`](nl_4x4.vuma) | 16 | 0 | 4x4 iteration counter |
| [`nl_sum_2d.vuma`](nl_sum_2d.vuma) | 10 | 0 | 2x5 grid sum |
| [`nl_matrix_mul.vuma`](nl_matrix_mul.vuma) | 0 | 0 | degenerate (XOR=0 either way) |
| [`nl_triangle.vuma`](nl_triangle.vuma) | 10 | 0 | 1+2+3+4 triangular |
| [`nl_identity.vuma`](nl_identity.vuma) | 1 | 0 | 2x2 identity [0][0] |
| [`nl_countdown.vuma`](nl_countdown.vuma) | 0 | 4 | nested while countdown |
| [`nl_accumulate.vuma`](nl_accumulate.vuma) | 45 | 0 | sum 0..9 via nested for |
| [`nl_product.vuma`](nl_product.vuma) | 36 | 1 | (i+1)² product over i=0..2 |
| [`nl_grid_sum.vuma`](nl_grid_sum.vuma) | 6 | 0 | 3x2 grid sum |
| [`nl_diagonal.vuma`](nl_diagonal.vuma) | 3 | 0 | 3x3 diagonal via (i==j) |
| [`nl_outer_product.vuma`](nl_outer_product.vuma) | 4 | 0 | outer product [2]·[2] |
| [`nl_pascals.vuma`](nl_pascals.vuma) | 3 | 0 | Pascal row 3 cell [1] |
| [`nl_floyd.vuma`](nl_floyd.vuma) | 1 | 1 | coincidental pass (see note) |

## Notes on individual tests

- **nl_matrix_mul** is degenerate on the current backend: the chosen
  matrices (A=B=[[1,1],[1,1]] → C=[[2,2],[2,2]]) XOR to 0 both when the
  matrix-multiply runs correctly and when the buggy loop body leaves
  every C cell at its initial 0. It is included to exercise the
  triple-nested matrix-multiply code path; once the loop bug is fixed it
  remains a regression test for matrix-multiply codegen.
- **nl_floyd** passes on the current backend by coincidence: the loop
  body's `counter = counter + le_v;` doesn't propagate, so `counter`
  stays at its initial value 1, and the very first inner-loop iteration
  (i=0, j=0) computes `to_store = counter * le_v = 1 * 1 = 1` and stores
  that at offset 0. The post-loop `*(buf + 0)` load therefore yields 1
  — which happens to equal the correct (theoretical) value of the
  Floyd's-triangle [0][0] entry. The test will continue to pass once
  the loop bug is fixed.
- The other 13 tests have distinct expected-vs-actual exit codes, so
  they will visibly flip from FAIL to PASS when the loop bug is fixed.

## Verification

All 15 `nl_*` tests **compile cleanly** on the current x86_64 backend
(no panics, no compile errors). All 15 binaries terminate within the
3-second timeout (no infinite-loop hangs — the buggy loop body fails
to advance the loop counter, but the loop is still bounded by the
codegen's termination behaviour). All 15 are deterministic across
multiple runs (3× verified). 13/15 FAIL on the current backend (as
expected); 2/15 PASS (1 degenerate, 1 coincidental — both documented
above).
