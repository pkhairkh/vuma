# 32-bit Arithmetic with Overflow Masking

Tests for `u32` arithmetic that explicitly masks results with `& 4294967295` to defeat 64-bit host widening. This is the dominant pattern in SHA-256 / SHA256d code and any code that must preserve exact 32-bit wrap-around semantics on a 64-bit ISA.

## What belongs here

- u32 add / xor / and / rotate with `& 4294967295`
- u32 store/load via 4 individual byte stores (big-endian)
- W-schedule style u32 word copy through byte buffers
- Endianness helpers (`read_u32_be`)

## Files (4)

- [`test_endian.vuma`](test_endian.vuma)
- [`test_u32_arith.vuma`](test_u32_arith.vuma)
- [`test_u32_mem.vuma`](test_u32_mem.vuma)
- [`test_w_sched.vuma`](test_w_sched.vuma)
