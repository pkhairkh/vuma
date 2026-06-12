# Task: Fix LoongArch64 Backend to Produce Working Binaries

## Summary

All three required fixes were already applied as uncommitted changes in the working tree. The binary builds successfully and the test passes with **exit code 0**.

## Fixes Applied

### 1. ELF Entry Point: Replace JIRL Return with Linux Exit Syscall

**File**: `src/codegen/src/loongarch64/mod.rs`, method `encode_program` (lines 3292-3347)

Added Phase 3 code that:
- Finds the last instruction of the first (entry) function
- Checks if it's `JIRL $r0, $ra, 0` (return instruction)
- Replaces it with `ADDI.D $a7, $r0, 93` + `SYSCALL 0` (Linux exit syscall)
- The replacement adds 4 bytes (8 bytes replacing 4), shifting subsequent code

Also fixed the SYSCALL/BREAK encoding:
- Old (wrong): `0x0000002B` / `0x0000002A`  
- New (correct): `0x002B0000` / `0x002A0000`

### 2. Remove FP (r22) from Allocatable Registers

- `is_allocatable()` now excludes `Gpr::Fp` (line 104)
- `Gpr::Fp` removed from `ALLOCATABLE_GPRS` const (was last item, removed along with comment)
- Test updated: `assert!(!Gpr::Fp.is_allocatable())` (line 3488)

### 3. Fix Alloc Double-Decrement

**Before**: Each `Alloc` decremented SP by the allocation size, then copied SP to the destination register. This double-decremented because the prologue already decremented SP by `frame_size` (which includes all Alloc sizes).

**After**: Each `Alloc` computes its address as `FP + alloc_offset` using a running offset. The prologue's SP decrement covers all the space, so Alloc just needs to compute the correct address within the frame.

Key changes:
- Added `alloc_offset: &mut i32` parameter to `lower_ir_instr_la64`
- Alloc instruction changed from `addi.d $sp, $sp, -size` + `add.d d, $sp, $r0` to `addi.d d, $fp, alloc_offset`
- `alloc_offset` initialized to 0 and incremented by aligned allocation size after each Alloc
- Removed duplicate JIRL from Ret handler (epilogue in `allocate_registers` already handles return)

## Verification

```bash
source "$HOME/.cargo/env" && cd /home/z/my-project/vuma && \
  cargo run --bin vuma -- emit loongarch64 examples/sha256d.vuma -o /tmp/sha256d_loongarch64.bin && \
  chmod +x /tmp/sha256d_loongarch64.bin && \
  /tmp/qemu-loongarch64-static /tmp/sha256d_loongarch64.bin; echo "Exit: $?"
```

**Result**: Exit code 0 (SHA256d computation matches expected NIST test vector)
