# Task: Implement Real ARM64 CLZ, CTZ, POPCNT Instructions

## Summary

Replaced the `MOV Xd, XZR` (return 0) placeholder implementations for CLZ, CTZ, and POPCNT in the ARM64 codegen with real instruction emissions.

## Changes Made

### 1. New Instruction Variants (`src/codegen/src/arm64.rs`)

Added 7 new instruction variants to the `Instruction` enum:

| Variant | Assembly | Encoding Base | Description |
|---------|----------|---------------|-------------|
| `CLZ { rd, rn }` | `clz Xd, Xn` | `0xDAC01000` | Count Leading Zeros (native ARM64) |
| `RBIT { rd, rn }` | `rbit Xd, Xn` | `0xDAC00000` | Reverse Bits (native ARM64) |
| `FMOV_DX { vd, rn }` | `fmov Dd, Xn` | `0x9E670000` | GPR → SIMD/FP double register |
| `FMOV_XD { rd, vn }` | `fmov Xd, Dn` | `0x9E6F0000` | SIMD/FP double register → GPR |
| `CNT { vd, vn }` | `cnt Vd.8B, Vn.8B` | `0x0E205800` | Population count per byte (SIMD) |
| `ADDV { vd, vn }` | `addv Bd, Vn.8B` | `0x0E71B800` | Horizontal add across vector (SIMD) |
| `UMOV { rd, vn }` | `umov Xd, Vn.B[0]` | `0x0E204000` | Move SIMD element to GPR (zero-extends) |

Each variant has:
- Full `encode()` implementation with correct ARM64 A64 encoding
- `Display` implementation for disassembly text output

### 2. Instruction Selector (`src/codegen/src/arm64.rs`)

Updated the `UnaryOp` match arm in the instruction selector:

- **CLZ**: Emits single `CLZ Xd, Xn` instruction
- **CTZ**: Emits `RBIT` + `CLZ` sequence (reverse bits then count leading zeros)
  - When `rd == rn`: uses X9 scratch register to avoid clobbering input
  - When `rd != rn`: RBIT into rd, then CLZ rd, rd
- **POPCNT**: Emits 4-instruction SIMD sequence using V8 (caller-saved):
  1. `FMOV D8, Xn` — move GPR value to SIMD register
  2. `CNT V8.8B, V8.8B` — count bits per byte
  3. `ADDV B8, V8.8B` — horizontal sum of byte counts
  4. `UMOV Xd, V8.B[0]` — move result back to GPR (zero-extends to 64-bit)

### 3. Emitter (`src/codegen/src/emit.rs`)

Updated the `UnaryOp` match arm in `emit_ir_instr()` with the same instruction sequences as the instruction selector.

### 4. Tests (`src/codegen/src/arm64.rs`)

Added 3 new test functions:

- **`test_clz_emission`**: Verifies CLZ encoding base (0xDAC01000), register field placement, display text, and instruction selector output
- **`test_ctz_emission`**: Verifies RBIT encoding (0xDAC00000), RBIT+CLZ sequence for both `rd != rn` and `rd == rn` cases, and scratch register usage
- **`test_popcnt_emission`**: Verifies all 4 SIMD instruction encodings (FMOV_DX, CNT, ADDV, UMOV), display text, and FMOV_XD encoding

## Verification

- `cargo check -p vuma-codegen` — passes with no errors
- All 41 ARM64 tests pass (including 3 new ones)
- All 11 codegen integration tests pass
- Pre-existing `scg_to_ir::tests::test_if_without_else` failure is unrelated

## Design Notes

- SIMD register V8 was chosen as scratch because it's caller-saved in AAPCS64
- UMOV writes to Wd (32-bit), which zero-extends to Xd automatically — no masking needed
- The `#[allow(non_camel_case_types)]` attribute was added to the `Instruction` enum to suppress warnings for ALL-CAPS variant names (consistent with existing ADD, SUB, etc.)
