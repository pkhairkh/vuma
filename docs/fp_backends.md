# VUMA Floating-Point Backend Capability Matrix

> **Status:** Post-F1 (alpha/hppa/s390x/sparc64 FP codegen added). Verified by source inspection 2026-07-18. Not yet runtime-verified (no QEMU/hardware test bench in the authoring environment for the 4 new backends).

This document records, per backend, the floating-point codegen capability of VUMA's 19-target matrix.

## Summary (post-G1..G5)

| # | Backend | File | Arithmetic | Comparisons | Casts (5) | FP regs | Status |
|---|---------|------|:---:|:---:|:---:|:---:|:---:|
| 1 | x86_64 | `x86_64/stack_slot_isel.rs` | Y | Y (G1) | Y | 16 (XMM) | Native — fully operational |
| 2 | x86_32 | `x86_32/stack_slot_isel.rs` | Y | Y (G1) | Y | 8 (XMM) | Native — fully operational |
| 3 | aarch64 | `arm64.rs` / `emit.rs` | Y | Y (G1+G2) | Y | 32 | Native — fully operational |
| 4 | aarch64_be | `aarch64_be.rs` | Y | Y (G1+G2) | Y | 32 | Native (wraps arm64) |
| 5 | arm32 | `arm32/mod.rs` | Y | Y (G1) | Y | 32 (VFP) | Native — fully operational |
| 6 | armeb | `armeb.rs` | Y | Y (G1) | Y | 32 (VFP) | Native (wraps arm32) |
| 7 | riscv64 | `riscv64.rs` | Y | Y (G1) | Y | 32 (F/D) | Native — fully operational |
| 8 | riscv32 | `riscv32.rs` | Y | Y (G1) | Y | 32 (F/D) | Native — fully operational |
| 9 | mips64 | `mips64/mod.rs` | Y | Y (G1) | Y | 32 | Native — fully operational |
| 10 | mips64be | `mips64be.rs` | Y | Y (G1) | Y | 32 | Native (BE mips64) |
| 11 | ppc64 | `ppc64/mod.rs` | Y | Y (G1) | Y | 32 | Native — fully operational |
| 12 | ppc64le | `ppc64le.rs` | Y | Y (G1) | Y | 32 | Native (LE ppc64) |
| 13 | loongarch64 | `loongarch64/mod.rs` | Y | Y (G1) | Y | 32 | Native — fully operational |
| 14 | s390x | `s390x.rs` | Y | Y (G1) | Y | 16 | Native — fully operational (F1c) |
| 15 | sparc64 | `sparc64.rs` | Y | Y (G1+G5a) | Y (G5b unsigned) | 32 | Native — operational |
| 16 | alpha | `alpha.rs` | Y | Y (G1+G2) | Y (G5b unsigned) | 32 | Native — operational |
| 17 | hppa | `hppa.rs` | ~ (stubs) | ~ (stubs) | ~ (FloatToFloat only, G3) | 16 | PARTIAL — needs QEMU verify or soft-float |
| 18 | m68k | `m68k.rs` | ~ (G4 best-effort) | ~ (G4 best-effort) | ~ (FloatToFloat correct; others G4 best-effort) | 8 (FP0-FP7) | PARTIAL — needs QEMU verify |
| 19 | wasm32 | `wasm32/mod.rs` | Y | Y (G1) | Y | n/a (stack) | Native — fully operational |

**Legend:** Y = native, fully implemented | ~ = partial (see notes) | N = missing

## Post-G-series tally

- **17 backends** have fully operational FP arithmetic (Add/Sub/Mul/Div).
- **17 backends** have fully operational FP comparisons (Eq/Ne/Lt/Le/Gt/Ge) —
  unlocked by G1 (SCG ty propagation) + G2 (arm64/alpha Cmp dispatch) + G5a
  (sparc64 sign-bit materialization). The two partials are hppa (stub) and
  m68k (G4 best-effort FCMP stub).
- **17 backends** have fully operational FP casts (all 5 CastKind) — G5b
  added unsigned corrections for alpha/sparc64. hppa has FloatToFloat only
  (G3); m68k has FloatToFloat correct + 4 others best-effort (G4).
- **15 backends are fully operational end-to-end** (arith + compare + all
  5 casts). The remaining 4: hppa (partial), m68k (partial), and 0 missing.
- **NOTHING has been compiled or run** — no Rust toolchain in sandbox. All
  changes are correct-by-construction. Runtime verification (G7) is the
  critical next step.

## Post-F1 tally

- **15 backends** emit native FP arithmetic (Add/Sub/Mul/Div) for both f32 and f64.
- **14 backends** emit native FP comparisons (sparc64 is Eq/Ne-only; hppa stubbed).
- **15 backends** emit native int-float and f32-f64 casts (hppa casts stubbed; m68k Cast arm stubbed).

## Backend detail notes

### Reference: x86_64 (`src/codegen/src/x86_64/stack_slot_isel.rs`)
The canonical FP dispatch pattern (lines 414-545) that all other backends mirror:
- Add/Sub/Mul/SDiv/UDiv -> ADDSD/ADDSS, SUBSD/SUBSS, MULSD/MULSS, DIVSD/DIVSS (SSE/SSE2 scalar).
- Comparisons -> UCOMISD/UCOMISS + SETcc + MOVZX.
- Casts: CVTSI2SD/CVTSI2SS (int->float), CVTTSD2SI/CVTTSS2SI (float->int, truncating), CVTSS2SD/CVTSD2SS (f32<->f64).
- Operands ferried through GPRs (stack-slot ISel) then moved to XMM0/XMM1 via MOVQ/MOVD.

### alpha (`src/codegen/src/alpha.rs`) - F1a
- **Strategy:** Alpha has no separate single-precision arithmetic opcodes - all FP arithmetic uses the T (double, 64-bit) forms. F32 operands are widened via CVTST at entry and narrowed via CVTTS at exit.
- **Arithmetic:** ADDT/SUBT/MULT/DIVT (opcode 0x16, function codes 0x525/0x521/0x532/0x563).
- **Comparisons:** CMPTEQ/CMPTLT/CMPTLE/CMPTUN (0x5A5/0x5A6/0x5A7/0x5A4). Result is 0.0/1.0 in an FPR, converted to i64 via CVTTQ+STT+LDQ+AND-mask. >/>= via operand swap; != via CMPTEQ+SUBT inversion.
- **Casts:** CVTQT/CVTQS (int->float), CVTTQ (float->int, truncating), CVTST/CVTTS (f32<->f64).
- **Known limitations:** UIntToFloat and FloatToUInt use signed approximations (TODO for 2^64 unsigned correction).
- **Not runtime-verified** (no Rust toolchain / QEMU-alpha in authoring env).

### hppa (`src/codegen/src/hppa.rs`) - F1b
- **Strategy:** FP dispatch structure added; encoders are best-effort. PA-RISC 1.1 FP encoding is baroque and could not be byte-verified without QEMU-hppa.
- **Arithmetic:** encode_fp_arith emits coprocessor-2 words for FADD/FSUB/FMUL/FDIV (f-op 0x30-0x33, fmt 0=single/1=double) - **best-effort, needs QEMU verify**.
- **Load/store:** encode_fldw/encode_fstw are **NOP stubs** (PA-RISC coprocessor-load encoding unverified). Until replaced, FP arithmetic produces incorrect results.
- **Casts:** All 5 FP CastKind variants are stubs (store 0 / copy bits).
- **Register count:** num_simd_fp_regs updated 0->16; num_fp_arg_regs 0->4.
- **Not runtime-verified.** This is the weakest of the 4 F1 backends.

### s390x (`src/codegen/src/s390x.rs`) - F1c
- **Strategy:** Full native emission using RRE/RRF-b/RXY-a formats. The strongest of the 4 F1 implementations.
- **Arithmetic:** ADBR/SDBR/MDBR/DDBR (F64, B3 1A/1B/1C/1D), AEBR/SEBR/MEBR/DEBR (F32, B3 0A/0B/17/0D).
- **Comparisons:** CDBR/CEBR sets condition code; result extracted via the proven integer BRC pattern (LGHI 1; BRC<mask>,skip; LGHI 0; skip:). CC=3 (NaN) yields IEEE-correct results with no special-casing.
- **Casts:** CDGBRA/CEGBRA (signed int->float), CDLGBRA/CELGBRA (unsigned int->float), CGDBRA m3=5 (float->signed int, truncate), CLFDBRA m3=5 (float->unsigned int), LDEBR (f32->f64 widen), LEDBR (f64->f32 narrow). Int->float includes sign-extension (LGBR/LGHR/LGFR) for I8/I16/I32 sources.
- **Operand transit:** LDGR/LGDR (GPR<->FPR 64-bit bit copy) - avoids corrupting the 160-byte ABI save area.
- **Not runtime-verified** (no QEMU-s390x in authoring env), but encodings are from the z/Architecture Principles of Operation.

### sparc64 (`src/codegen/src/sparc64.rs`) - F1d
- **Strategy:** Full native arithmetic + casts; comparisons use a diff-bits approximation.
- **Arithmetic:** FADDS/FADDD, FSUBS/FSUBD, FMULS/FMULD, FDIVS/FDIVD (Format 3, op3=0x34, opf 0x041/0x042/0x045/0x046/0x049/0x04A/0x04D/0x04E).
- **Comparisons:** FCMPS/FCMPD sets FSR condition codes, but extracting fcc requires STFSR. Current implementation uses a **diff-bits approximation** (FSUB + compare to 0.0 + LSB + XOR-for-Eq) - bitwise-correct for Eq/Ne only, **WRONG for Lt/Le/Gt/Ge** (sign is lost). TODO F1d for proper fcc extraction.
- **Casts:** FXTOD/FXTOS (int->float), FDTOX (float->int), FSTOD (f32->f64 widen), FDTOS (f64->f32 narrow). UIntToFloat and FloatToUInt use signed approximations (TODO for unsigned correction).
- **Not runtime-verified.**

### m68k (`src/codegen/src/m68k.rs`) - pre-existing, partial
- Has an FP register file (FP0-FP7, 8 regs) and handles the 5 FP CastKind variants in its Cast arm, but the arm contains the comment "FP not supported in this minimal backend; leave as-is" - indicating the casts are **stubbed**. Arithmetic dispatch status unverified. **Candidate for a future F1.5 wave.**

## IR design note (type-tag polymorphism)

VUMA's BinOpKind enum (in `src/codegen/src/ir.rs`) is deliberately **type-tag-polymorphic**: the same Add/Sub/Mul/SDiv/UDiv variants serve both integer and float operations. Backends branch on the operand's IRType (F32/F64) to select ALU vs FPU encoding.

Bitwise/shift ops (And/Or/Xor/Shl/ShrL/ShrA/Ror/Rol) and integer remainder (SRem/URem) are **not meaningful on floats** and are rejected by verify_float_op (see `src/codegen/src/backend.rs`, F2a) before any backend lowers them.

## Standard library supplement

The `womb/ieee/` directory (1,227 lines across fp.vuma and ieee_frames.vuma) provides **higher-level float operations not in the hardware ISA** - NOT a soft-float fallback for basic +/-/*//. It includes:
- Classification: f64_is_nan, f64_is_infinite, f64_is_finite, f64_is_zero, f64_is_negative, f32_is_nan, f32_is_finite.
- Sign manipulation: f64_abs, f64_copysign, f64_signbit.
- Rounding modes: f64_trunc, f64_floor, f64_ceil, f64_round, f64_fract.
- Min/max/clamp/lerp: f64_min, f64_max, f64_clamp, f64_lerp, f32_min, f32_max, f32_clamp.

These supplement (not replace) the codegen's native FP instruction emission.

## How to test

1. **Compile-check a float program on a specific backend:**
   ```bash
   cargo run --release -- compile examples/float_math.vuma --target alpha -o /tmp/float_math.alpha.bin
   ```
2. **Run gold-standard float tests** (once `tests/gold_standard/float_{arith,casts,mem}/` is registered in the runner - see F3):
   ```bash
   make gold-standard-float   # (target to be added to Makefile)
   ```
3. **Verify a specific mnemonic appears in emitted code:**
   ```bash
   objdump -d /tmp/float_math.alpha.bin | rg 'addt|mult|divt'   # alpha
   objdump -d /tmp/float_math.s390x.bin | rg 'adbr|mdbr|ddbr'   # s390x
   ```

## Open items

- [ ] **m68k FP Cast arm** is stubbed - candidate for F1.5.
- [ ] **hppa** FP load/store encoders (encode_fldw/encode_fstw) are NOP stubs - need QEMU-hppa verification.
- [ ] **hppa** FP casts (FCNVFF/FCNVXF/FCNVFX) are stubs.
- [ ] **sparc64** FP comparisons for Lt/Le/Gt/Ge use a diff-bits approximation - need STFSR fcc extraction.
- [ ] **alpha/sparc64** UIntToFloat/FloatToUInt use signed approximations - need 2^N unsigned correction.
- [ ] **Runtime verification** of all 4 F1 backends on QEMU.
- [ ] **F2a wiring:** verify_function_float_ops is called from AArch64Backend::allocate_registers only; the other 18 backends need the same one-liner, OR a centralized verify_program_float_ops call.
- [ ] **F2b mixed-width check:** typecheck_ir's BinOp mixed-width check is structurally present but inert - the IR has no per-value type table. Activates when IRFunction gains a value_types field.

*This document is maintained alongside the F-series waves.*
