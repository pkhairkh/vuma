# Task 3: Fix AArch64 Backend to Produce Working Binaries

## Summary

The AArch64 backend had multiple critical bugs that prevented it from producing working ELF binaries. All bugs have been identified and fixed. The binary now runs correctly under QEMU and exits with code 0.

## Bugs Found and Fixed

### Bug 1: STP/LDP Register Field Positions Swapped
**File**: `src/codegen/src/arm64.rs` (encode method, lines ~1490-1510)

The STP and LDP instruction encodings had the register fields in wrong bit positions:
- **Wrong**: `(rn.encoding() << 10) | (rt1.encoding() << 5) | rt2.encoding()`
- **Correct**: `(rt2.encoding() << 10) | (rn.encoding() << 5) | rt1.encoding()`

AArch64 STP/LDP encoding format: `[31:30]=opc, [29:27]=101, [26]=V, [25:24]=01, [23:22]=addr_mode, [21:15]=imm7, [14:10]=Rt2, [9:5]=Rn, [4:0]=Rt`

The old code put Rn at bits[14:10] (Rt2 position) and Rt1 at bits[9:5] (Rn position), resulting in completely wrong instructions being generated. For example, `STP X29, X30, [SP, #-16]` would encode as `STP X30, XZR, [X29, #-16]`.

### Bug 2: MOV with SP Source Register
**File**: `src/codegen/src/arm64.rs` (encode method, lines ~1734 and ~2099)

`MOV Xd, SP` was encoded as `ORR Xd, XZR, SP`, but in the ORR instruction, register 31 in the Rm position is XZR (zero register), not SP. This resulted in `MOV X29, XZR` instead of `MOV X29, SP`.

**Fix**: When the source register is SP, emit `ADD Xd, SP, #0` instead, which correctly reads SP (register 31 in Rn position of ADD is SP).

Applied to both `encode()` and `encode_with_width()` methods.

### Bug 3: Missing Pre-Indexed STP and Post-Indexed LDP
**File**: `src/codegen/src/arm64.rs` (new instruction variants)

The prologue needed `STP X29, X30, [SP, #-16]!` (pre-indexed, updates SP) and the epilogue needed `LDP X29, X30, [SP], #16` (post-indexed, updates SP). The existing STP/LDP only supported signed offset addressing.

**Fix**: Added `STP_PRE` and `LDP_POST` instruction variants with correct base encodings:
- STP_PRE: 0xA9800000 (bits[23:22]=10 for pre-indexed store)
- LDP_POST: 0xA8C00000 (bits[23:22]=11 for post-indexed load)

Also added Display, decode, and is_memory_operation support for the new variants.

### Bug 4: Wrong MOVZ Immediate in Phase 3 Exit Syscall
**File**: `src/codegen/src/backend.rs` (encode_program, line ~1562)

The Phase 3 code that replaces RET with exit syscall had the wrong encoding for `MOV X8, #93`:
- **Wrong**: `0xD2800E88` = `MOVZ X8, #0x74` = `MOV X8, #116` (syslog syscall)
- **Correct**: `0xD2800BA8` = `MOVZ X8, #0x5D` = `MOV X8, #93` (exit syscall)

The little-endian bytes changed from `[0x88, 0x0E, 0x80, 0xD2]` to `[0xA8, 0x0B, 0x80, 0xD2]`.

### Bug 5: Decoder Pattern Mismatch
**File**: `src/codegen/src/arm64.rs` (decode method)

The existing decoder had wrong bit patterns for LDP and STP:
- LDP checked `0b1010100110` (actually STP pre-indexed) instead of `0b1010100101`
- STP checked `0b1010100010` (actually STP post-indexed) instead of `0b1010100100`

Both fixed to use correct AArch64 encoding patterns.

## Updated Files

1. **`src/codegen/src/arm64.rs`**:
   - Added `STP_PRE` and `LDP_POST` instruction enum variants
   - Fixed STP encoding register field positions
   - Fixed LDP encoding register field positions
   - Added STP_PRE and LDP_POST encoding
   - Fixed MOV with SP to use ADD Xd, SP, #0
   - Added Display implementations for new variants
   - Added to is_memory_operation match
   - Fixed decoder patterns for LDP and STP
   - Added decoder for STP_PRE and LDP_POST

2. **`src/codegen/src/emit.rs`**:
   - Changed prologue from `STP` to `STP_PRE` (pre-indexed)
   - Changed epilogue from `LDP` to `LDP_POST` (post-indexed) with offset +16

3. **`src/codegen/src/backend.rs`**:
   - Fixed MOV X8, #93 encoding in Phase 3 exit syscall replacement

## Test Result

```
$ cargo build --workspace && cargo run --bin vuma -- emit aarch64 examples/sha256d.vuma -o /tmp/sha256d_aarch64.bin && chmod +x /tmp/sha256d_aarch64.bin && /tmp/qemu-aarch64-static /tmp/sha256d_aarch64.bin; echo "Exit: $?"
Exit: 0
```

QEMU strace confirms: `exit(0)` — the binary executes correctly.
