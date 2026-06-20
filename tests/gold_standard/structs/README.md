# Struct and Enum (Tagged Union) Types

Tests for `struct` definitions, struct-literal initialization (`Foo { a: 1, b: 2 }`), field access via `(*ptr).field`, and `enum` (tagged-union) types with `match` expressions. These exercise the struct layout / field-offset computation in codegen and the discriminant + payload memory model for enums.

## What belongs here

- Plain struct definitions and field access
- Struct literals with shorthand field init
- Enum (tagged union) with `Some` / `None`-style variants
- `match` expressions for pattern matching on enum tag

## Files (2)

- [`enum_demo.vuma`](enum_demo.vuma)
- [`struct_demo.vuma`](struct_demo.vuma)
