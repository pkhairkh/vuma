# Task: Fix scg_to_ir to return error instead of silently substituting 0 for unknown variables

## Summary of Changes

### 1. `src/codegen/src/lib.rs` — Added `UnknownVariable` error variant

Added a new `CodegenError::UnknownVariable { name: String }` variant to the `CodegenError` enum. This provides a structured, typed error for unknown variable references instead of the previous silent 0-substitution behavior.

### 2. `src/codegen/src/scg_to_ir.rs` — Core changes

#### a. `resolve_expr` function (lines ~1382-1398)
- **Before**: Returned `IRValue` directly. Unknown variables produced `IRValue::Immediate(0)` with a `log::warn!`.
- **After**: Returns `Result<IRValue>`. Unknown variables return `Err(CodegenError::UnknownVariable { name })`.
- Updated docstring to document the error case.

#### b. All 14 callers of `resolve_expr` updated to propagate errors:
- `lower_statement` (Return): `collect::<Result<Vec<_>>>()?`
- `lower_if`: `resolve_expr(cond, names)?`
- `lower_switch`: `resolve_expr(discriminant, names)?`
- `lower_allocation` (Heap): `resolve_expr(size_expr, names)?`
- `lower_access` (Load): `resolve_expr(ptr, names)?`, `resolve_expr(off, names)?`
- `lower_access` (Store): `resolve_expr(ptr, names)?`, `resolve_expr(value, names)?`, `resolve_expr(off, names)?`
- `lower_cast`: `resolve_expr(&cast.src, names)?`
- `lower_computation`: `resolve_expr(&comp.lhs, names)?`, `resolve_expr(&comp.rhs, names)?`
- `lower_unary_computation`: `resolve_expr(&unary.operand, names)?`
- `lower_call`: `collect::<Result<Vec<_>>>()?`

#### c. Phi node `unwrap_or(0)` fallbacks replaced:
- `lower_if` phi construction: Changed from `unwrap_or(0)` to `unwrap()` with a comment explaining the safety guarantee (both branches are confirmed defined by the `is_defined` check).
- `lower_switch` phi construction: Changed from `unwrap_or(0)` to `ok_or_else(|| CodegenError::UnknownVariable { .. })?` for proper error propagation when a variable can't be resolved from either the arm definitions or `names_before`.

#### d. Docstring updates:
- `lower_if`: Updated comment about pre-if variable resolution to mention error return instead of "Immediate(0) fallback".
- `resolve_expr`: Added `# Errors` section documenting the `UnknownVariable` error.

#### e. New tests:
- `test_unknown_variable_returns_error`: Builds a minimal SCG with a function that returns an undefined variable. Verifies that `convert` returns `Err(CodegenError::UnknownVariable { name: "undefined_var" })`.
- `test_unknown_variable_in_computation_returns_error`: Builds an SCG with a Computation node that references an undefined variable `y` (while `x` is a defined parameter). Verifies the error is `UnknownVariable { name: "y" }`.

## Cargo Check Output

```
Checking vuma-codegen v0.1.0 (/home/z/my-project/vuma/src/codegen)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.14s
```

All checks pass with no errors or warnings.
