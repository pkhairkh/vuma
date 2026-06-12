# Task: Fix PPC64 Backend to Produce Working Binaries

## Summary

Fixed three critical bugs in the PPC64 (PowerPC 64-bit) backend at `src/codegen/src/ppc64/mod.rs` that prevented the generated binaries from running under QEMU.

## Bugs Found and Fixed

### 1. Wrong Endianness in `encode_word` (Critical)

**File**: `src/codegen/src/ppc64/mod.rs`, line 424

**Problem**: `encode_word` used `word.to_le_bytes()` (little-endian), but PPC64 big-endian targets require `word.to_be_bytes()`. Every instruction in the generated binary was byte-swapped.

**Fix**: Changed `word.to_le_bytes()` to `word.to_be_bytes()` and updated all `from_le_bytes` references in the disassembler and test code to `from_be_bytes`.

### 2. DS-Form Encoding Double-Shift Bug (Critical)

**File**: `src/codegen/src/ppc64/mod.rs`, line 435-442 (`encode_ds_form`)

**Problem**: The DS-form encoding function had `(((ds as u32) & 0x3FFC) << 2)` which:
- Takes the displacement value (e.g., 48 for `std r0, 48(r1)`)
- Masks out the low 2 bits (correct)
- Then shifts left by 2 again, doubling the displacement

For example, `stdu r1, -32(r1)` was encoded as `stdu r1, -128(r1)` and `std r0, 48(r1)` was encoded as `std r0, 192(r1)`.

**Root Cause**: The `ds` parameter is the byte displacement, but the DS-form field needs `displacement >> 2`. The code was shifting the displacement left instead of right before placing it in the instruction word.

**Fix**: Changed to `(ds >> 2) as u32` to properly convert the displacement to the 14-bit ds field, then mask with `0x3FFF` and shift left by 2 to position it in the instruction word.

### 3. Missing ELFv2 ABI Flag in ELF Header (Critical)

**File**: `src/codegen/src/ppc64/mod.rs`, line 1235 (in `build_minimal_ppc64_elf`)

**Problem**: `e_flags` was set to 0, which indicates ELFv1 ABI. In ELFv1, the entry point is interpreted as a function descriptor (containing a pointer to the actual code), not the code address itself. This caused the kernel/QEMU to jump to an invalid address.

**Fix**: Set `e_flags = 2` (EF_PPC64_ABI_V2), which tells the loader that the entry point is the actual code address.

### 4. Additional Fixes

- Changed `ELFDATA2LSB` (1) to `ELFDATA2MSB` (2) in the ELF header for big-endian PPC64
- Changed all ELF header fields from `.to_le_bytes()` to `.to_be_bytes()`
- Updated comments from "ppc64le" / "little-endian" to "ppc64" / "big-endian"

## Test Results

```
$ cargo run --bin vuma -- emit ppc64 examples/sha256d.vuma -o /tmp/sha256d_ppc64.bin
$ chmod +x /tmp/sha256d_ppc64.bin
$ /tmp/qemu-ppc64-static /tmp/sha256d_ppc64.bin
Exit: 0
```

## Notes

- The `encode_program` method already had the BLR-to-exit-syscall replacement logic. It was correct in structure but produced wrong bytes due to the endianness bug.
- The Alloc instruction handling was already correct (no double-decrement) — the prologue accounts for Alloc sizes in the frame_size, and the Alloc handler only computes the address offset without decrementing SP.
