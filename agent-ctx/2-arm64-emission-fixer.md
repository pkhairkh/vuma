# Task 2: ARM64 Emission Fixer

## Summary
Fixed 6 critical ARM64 emission bugs in `src/codegen/src/emit.rs` and `src/codegen/src/arm64.rs`.

## Bugs Fixed

### Bug 1: CSET not implemented (all comparisons return 0)
- Added `CSET { rd, cond }` to Instruction enum
- Implemented encode() as CSINC alias
- Added `cmp_kind_to_condition()` and `binop_kind_to_condition()` helpers
- Fixed `IRInstr::Cmp` and comparison `BinOp` arms to emit CSET instead of MOV XZR

### Bug 2: MSUB not implemented (remainder returns quotient)
- Added `MSUB { rd, rn, rm, ra }` to Instruction enum
- Implemented encode()
- After SDIV/UDIV, emit MSUB to compute dividend - quotient * divisor

### Bug 3: Cast lowering is just MOV (no zext/sext/trunc)
- Added `UBFM { rd, rn, immr, imms }` and `SBFM { rd, rn, immr, imms }` to Instruction enum
- Implemented encode() for both
- ZExt emits UBFM rd, rn, #0, #31; SExt emits SBFM rd, rn, #0, #31
- Trunc/BitCast remain MOV (correct)

### Bug 4: Hardcoded 64-byte stack frame
- Added `compute_frame_size(func)` helper
- Added `frame_size: u16` field to Emitter struct
- Prologue and epilogue use computed size

### Bug 5: Free is a no-op
- Emits MOV to X0 + BL __vuma_free via call relocation

### Bug 6: GetAddress always returns null
- Emits name hash to X0 + BL __vuma_getaddr via call relocation

## Verification
- `cargo check -p vuma-codegen` passes successfully
