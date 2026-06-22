# Basic Integer Arithmetic

Tests covering the fundamental integer arithmetic operators (`+`, `-`, `*`, `/`, `%`) on `i32`, `i64`, `u64` values, including literal returns, single-operator expressions, and small function-call arithmetic. These are the simplest possible programs - they should compile, execute, and exit with a predictable code on every backend.

## What belongs here

- Addition / subtraction / multiplication of integer literals and locals
- Constant returns (smallest valid programs)
- Single-expression function bodies (`fn add1(x) -> x + 1`)
- Smoke tests for the register allocator and prologue/epilogue codegen

## Files (3)

- [`minimal.vuma`](minimal.vuma)
- [`test_call.vuma`](test_call.vuma)
- [`test_exit.vuma`](test_exit.vuma)
