# Task 1: Fix RISC-V64 Alloc/Free Placeholder Instructions

## Summary

Fixed placeholder `alloc` and `free` instruction emission across three backends (RISC-V64, LoongArch64, ARM32). MIPS64 and PPC64 were already correctly implemented and did not need changes.

## Changes Made

### 1. RISC-V64 (`src/codegen/src/riscv64.rs`)

**Alloc** (was: `ADDI d, s0, 0` — placeholder pointing to frame pointer with zero offset):
- Now emits: `ADDI sp, sp, -size` (decrement stack pointer by allocation size) + `ADDI d, sp, 0` (copy new SP to destination)
- Uses the `size` field from `IRInstr::Alloc` to compute the negative immediate

**Free** (was: `ADDI a7, zero, 0` + `ECALL` — syscall number 0 is invalid):
- Now emits: `ADDI a7, zero, 214` + `ECALL` (Linux brk syscall for heap deallocation)
- Syscall number 214 = Linux `brk` on RISC-V64

**Tests added** (3 new tests):
- `test_isel_alloc_emits_addi_sp` — verifies alloc emits ADDI sp,sp,-size with non-zero immediate
- `test_isel_alloc_dst_gets_sp` — verifies alloc copies new SP to destination register
- `test_isel_free_emits_brk_syscall` — verifies free emits ADDI a7,zero,214 and not the old a7=0 placeholder

### 2. LoongArch64 (`src/codegen/src/loongarch64/mod.rs`)

**Alloc** (was: `addi.d d, $fp, 0` — placeholder pointing to frame pointer with zero offset):
- Now emits: `addi.d $sp, $sp, -size` + `add.d d, $sp, $r0` (copy new SP to destination)
- Uses `AddiD` with negative immediate and `AddD` with zero register for the copy

**Free** (was: NOP — no-op placeholder):
- Now emits: `break` instruction (trap on accidental execution, matching MIPS64's pattern)

### 3. ARM32 (`src/codegen/src/arm32/mod.rs`)

**Alloc** (was: `ADD d, R11, #0` — placeholder pointing to FP with zero offset):
- Now emits: `SUB SP, SP, #size` + `ADD d, SP, #0` (copy new SP to destination)
- Handles sizes that don't fit in ARM rotated-immediate format by loading into scratch register first

**Free** (was: NOP — no-op placeholder):
- Now emits: UDF (undefined instruction, `0xE7F000F0`) to trap if executed

### 4. Pre-existing fix (`src/codegen/src/scg_to_ir.rs`)

Fixed a pre-existing compilation error in the test module where `CodegenError` was used without being imported. Added `use crate::CodegenError;` to the test module.

## Backends Not Changed

- **MIPS64**: Already correctly emits `daddiu $sp, $sp, -size` + `daddu dst, $sp, $zero` for alloc, and `break 0xFF` for free
- **PPC64**: Already correctly emits `stdu r1, -size(r1)` + `mr d, r1` for alloc, and `trap` for free

## Verification

- `cargo check -p vuma-codegen` passes cleanly
- All 3 new RISC-V64 tests pass:
  - `test_isel_alloc_emits_addi_sp`
  - `test_isel_alloc_dst_gets_sp`
  - `test_isel_free_emits_brk_syscall`
