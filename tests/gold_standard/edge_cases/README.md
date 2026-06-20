# Boundary Conditions and Unusual Features

Tests that exercise unusual corners of the language: hardware register access via `map_device`, floating-point type conversions (`f32`/`f64`, `inttofloat`, `floattoint`), and the `extern "C"` FFI block syntax. These probe edge cases of the codegen, linker, and IVE that the core categories don't reach.

## What belongs here

- Hardware register access via `map_device()` (embedded)
- f32 / f64 conversion intrinsics on all 8 backends
- `extern "C" { fn write(...); }` FFI block + relocations
- FP arithmetic and FP store/load

## Files (3)

- [`ffi_demo.vuma`](ffi_demo.vuma)
- [`float_math.vuma`](float_math.vuma)
- [`gpio_blink.vuma`](gpio_blink.vuma)
