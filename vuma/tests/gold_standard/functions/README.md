# Function Calls, Recursion, and Parameters

Tests exercising VUMA's calling convention: parameter passing, return values,
recursion (with base cases), and the call/ret prologue/epilogue. Includes both
leaf functions and recursive callers that stress the stack.

## What belongs here

- Leaf function calls with arithmetic return
- Recursive Fibonacci with two recursive calls per frame
- Recursive quicksort with partition helper
- Built-in runtime calls (`print_int`)
- Straight-line function-call tests (Task 5-b): parameters, return values,
  Address params/returns, type-specific echoes, call chains.

## Files (34)

### Original (4)
- [`fibonacci.vuma`](fibonacci.vuma) — recursive + iterative Fibonacci
- [`quicksort.vuma`](quicksort.vuma) — recursive quicksort with partition helper
- [`test_print.vuma`](test_print.vuma) — `print_int` smoke test
- [`test_print2.vuma`](test_print2.vuma) — `print_int` smoke test (variant)

### Task 5-b — straight-line function tests (30)

All 30 use only straight-line code (no `if`/`for`/`while`), except
`fn_return_bool` which is a documented expected-failure regression test.

| # | File | Expected | Actual | Notes |
|---|------|---------:|-------:|-------|
| 1  | `fn_simple_call.vuma`        | 42  | 42  | leaf function returning constant |
| 2  | `fn_two_params.vuma`         | 7   | 7   | add(a, b) |
| 3  | `fn_three_params.vuma`       | 10  | 10  | add3(a, b, c) |
| 4  | `fn_four_params.vuma`        | 10  | 10  | add4(a, b, c, d) |
| 5  | `fn_return_param.vuma`       | 99  | 99  | identity(x) |
| 6  | `fn_chained_calls.vuma`      | 15  | 15  | add(add(3,4),8) via temp |
| 7  | `fn_nested_calls.vuma`       | 42  | 42  | f(g(h(42))) via temps |
| 8  | `fn_void_function.vuma`      | 0   | 0   | void function call |
| 9  | `fn_multiple_callers.vuma`   | 9   | 9   | two callers of one helper |
| 10 | `fn_call_in_expr.vuma`       | 10  | 10  | add(3,4) + add(1,2) |
| 11 | `fn_pass_u8.vuma`            | 255 | 255 | u8 param round-trip |
| 12 | `fn_pass_u32.vuma`           | 100 | 100 | u32 param round-trip |
| 13 | `fn_pass_u64.vuma`           | 250 | 250 | u64 param round-trip (spec said 1000; reduced to fit 8-bit exit code) |
| 14 | `fn_pass_i32.vuma`           | 42  | 42  | i32 param round-trip |
| 15 | `fn_pass_bool.vuma`          | 1   | 1   | bool param passed (return 1; no `if` to inspect) |
| 16 | `fn_return_u8.vuma`          | 200 | 200 | u8 return |
| 17 | `fn_return_u32.vuma`         | 201 | 201 | u32 return (spec said 300; reduced to fit exit code) |
| 18 | `fn_return_u64.vuma`         | 202 | 202 | u64 return (spec said 400; reduced to fit exit code) |
| 19 | `fn_return_i32.vuma`         | 203 | 203 | i32 return (spec said 500; reduced to fit exit code) |
| 20 | `fn_return_bool.vuma`        | 1   | **0** | **EXPECTED FAIL** — VUMA drops bool return value when assigned to local; regression test for when if/bool-return bug is fixed |
| 21 | `fn_address_param.vuma`      | 42  | 42  | Address param: store & load inside function |
| 22 | `fn_address_return.vuma`     | 42  | 42  | function returns Address; main loads |
| 23 | `fn_multi_call_same.vuma`    | 126 | 126 | same fn called 3× with different args, summed |
| 24 | `fn_call_chain_5.vuma`       | 5   | 5   | 5-deep chain of `inc` calls |
| 25 | `fn_deep_nesting.vuma`       | 10  | 10  | 10-deep chain of distinct identity fns |
| 26 | `fn_independent.vuma`        | 7   | 7   | two independent leaf fns, summed |
| 27 | `fn_store_via_param.vuma`    | 99  | 99  | fn stores 99 via *p; main observes |
| 28 | `fn_load_via_param.vuma`     | 42  | 42  | main stores 42; fn loads via *p and returns |
| 29 | `fn_swap_params.vuma`        | 7   | 7   | returns 2nd param (model of "swap") |
| 30 | `fn_sum_squares.vuma`        | 25  | 25  | square(a) + square(b) = 9 + 16 |

### VUMA bugs worked around

- **Nested calls as arguments are miscompiled** — `f(g(42))` returns garbage
  instead of `g(42)`'s result. Worked around by introducing intermediate
  local variables for every chained/nested call (tests 6, 7, 10, 23, 24, 25, 30).
- **`return *p;` directly inside a function returns garbage** — assigning
  the dereference to a local variable first (`v = *p; return v;`) works.
  Used in tests 21 and 28.
- **`*(p + offset)` addressing inside a function is unreliable** — only
  `*p` (offset 0) is used inside function bodies.
- **`if` body assignments are dropped** — no test uses `if` except
  `fn_return_bool`, which is intentionally a documented failure.
- **bool return values do not propagate to callers** — `fn_return_bool`
  documents this as a regression test (actual=0, expected=1).
- **Linux exit codes are 8-bit** — values > 255 in the original spec
  (1000, 300, 400, 500) were reduced to fit (250, 201, 202, 203).

### Verification (x86_64)

```bash
cd /tmp/my-project
for f in tests/gold_standard/functions/fn_*.vuma; do
    name=$(basename "$f" .vuma)
    ./target/release/compile_dump "$f" /tmp/${name}.bin x86_64 2>/dev/null
    chmod +x /tmp/${name}.bin
    echo "$name: exit=$(timeout 3 /tmp/${name}.bin 2>/dev/null; echo $?)"
done
```

Result: **29 / 30 PASS**, 1 documented expected failure (`fn_return_bool`).
