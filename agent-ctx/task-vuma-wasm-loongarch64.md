# Task: Fix WASM32 panic and add LoongArch64 FP instruction decoding

## Summary of Changes

### PART 1: WASM32 panic fix

**File: `src/codegen/src/lib.rs`**
- Added `WasmSectionNotFound { section: String }` variant to the `CodegenError` enum
- This provides a proper error type for missing WASM sections instead of panicking

**File: `src/codegen/src/wasm32/mod.rs`**
- Changed `test_wasm_memory_section` test to return `crate::Result<()>` instead of `()`
- Replaced `panic!("Memory section not found")` with `Err(crate::CodegenError::WasmSectionNotFound { section: "Memory".to_string() })`
- Changed `return;` to `return Ok(());` on the success path

### PART 2: LoongArch64 FP instruction decoding

**File: `src/codegen/src/loongarch64/mod.rs`**

1. **Added opcode constants** for FP instructions:
   - 3R-format FP arithmetic: `OPC_FADD_S` (0x0100), `OPC_FADD_D` (0x0101), `OPC_FSUB_S` (0x0102), `OPC_FSUB_D` (0x0103), `OPC_FMUL_S` (0x0104), `OPC_FMUL_D` (0x0105), `OPC_FDIV_S` (0x0106), `OPC_FDIV_D` (0x0107)
   - 2R-format FP move: `OPC_FMOV_S` (0x000004E), `OPC_FMOV_D` (0x000004F)
   - 4R-format FP compare: `OPC_FCMP_S` (0x0C4), `OPC_FCMP_D` (0x0C5)
   - 2RI12-format FP load/store: `OPC_FLD_S` (0x0AB), `OPC_FLD_D` (0x0AC), `OPC_FST_S` (0x0AD), `OPC_FST_D` (0x0AE)
   - 2R-format FP GPR<->FPR move: `OPC_MOVFR2GR_D` (0x0000052), `OPC_MOVGR2FR_D` (0x0000053)

2. **Fixed incorrect FP opcodes** for existing double-precision instructions:
   - `FsubD`: was 0x0102, corrected to 0x0103 (was colliding with FSUB.S)
   - `FmulD`: was 0x0103, corrected to 0x0105 (was colliding with FMUL.S)
   - `FdivD`: was 0x0104, corrected to 0x0107 (was colliding with FDIV.S)

3. **Added new Instruction enum variants**:
   - `FaddS`, `FsubS`, `FmulS`, `FdivS` (single-precision FP arithmetic, 3R format)
   - `FmovS`, `FmovD` (FP register-to-register moves, 2R format)
   - `FCmpS`, `FCmpD` (FP compare with condition code, 4R-like format)

4. **Added encoding** for all new variants in `Instruction::encode()`

5. **Added mnemonic** for all new variants in `Instruction::mnemonic()`

6. **Added Display** for all new variants in `fmt::Display`, including `fcmp_cond_mnemonic()` helper

7. **Replaced hardcoded opcode literals** with named constants for existing FP instructions (FldS/D, FstS/D, FmovGr2FprD, FmovFpr2GrD)

**File: `src/codegen/src/loongarch64/disasm.rs`**

1. **Removed `#[allow(dead_code)]`** from `fpr_from_bits()` and the "not yet implemented" comment

2. **Added FP arithmetic decode paths** (3R format): FADD.S/D (0x0100-0x0107)

3. **Added FP move decode paths** (2R format): FMOV.S (0x000004E), FMOV.D (0x000004F), MOVFR2GR.D (0x0000052), MOVGR2FR.D (0x0000053)

4. **Added FP load/store decode paths** (2RI12 format): FLD.S (0x0AB), FLD.D (0x0AC), FST.S (0x0AD), FST.D (0x0AE)

5. **Added FP compare decode paths** (4R format): FCMP.cond.S (0x0C4), FCMP.cond.D (0x0C5)

6. **Added 2 new tests**:
   - `test_decode_fp_arithmetic_s_d`: Round-trip tests for all 8 FP arithmetic instructions (4 .S + 4 .D)
   - `test_decode_fp_mov_fcmp`: Round-trip tests for FMOV.S/D, FCmpS/D, FLD.S/D, FST.S/D

## Cargo Check Output

```
Checking vuma-codegen v0.1.0
Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.73s
```

## Test Results

- All LoongArch64 tests: **52 passed**
- WASM memory section test: **passed**
- New FP decode tests: **2 passed** (covering 16 instruction variants)
- Pre-existing failure in `scg_to_ir::tests::test_if_without_else` (unrelated to this change)
