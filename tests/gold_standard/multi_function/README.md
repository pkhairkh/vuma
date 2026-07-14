# Many Functions Calling Each Other

Tests with large function counts and dense call graphs. These stress the Static Call Graph (SCG) builder, the function-offset resolution pass (`resolve_call_relocs`), and the `BL`/`CALL` relocation range checks. Also useful for verifying DWARF subprogram DIE generation.

## What belongs here

- SHA256d with ~10 helper functions (rotr, ch, maj, sigma, ...)
- Multi-function program designed to exercise DWARF debug info
- Cross-function pointer passing and return

## Files (2)

- [`debug_info.vuma`](debug_info.vuma)
- [`sha256d.vuma`](sha256d.vuma)
