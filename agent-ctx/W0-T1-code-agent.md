# Task W0-T1: Fix 3 dead-code warnings in disassemblers

## Agent
Code Agent

## Task
Fix 3 dead-code warnings in the VUMA compiler codegen crate disassemblers.

## Changes Made

### File 1: `/home/z/my-project/vuma/src/codegen/src/arm32/disasm.rs`
- **Line 95**: Added `#[allow(dead_code)]` to `fn sign_extend_12(val: u32) -> i32`
- **Rationale**: ARM32 load/store offsets use the U (up/down) bit rather than sign-extending the 12-bit immediate, so this helper is not wired into the current decode path. Kept for potential future use with media/DSP extensions.

### File 2: `/home/z/my-project/vuma/src/codegen/src/loongarch64/disasm.rs`
- **Line 81**: Added `#[allow(dead_code)]` to `fn fpr_from_bits(bits: u32) -> Fpr`
- **Rationale**: FP instruction decoding is not yet implemented in the disassembler. Kept for use when FP instruction decode paths are added.

- **Line 135**: Added `#[allow(dead_code)]` to `fn sign_extend_20(val: u32) -> i32`
- **Rationale**: Upper-immediate instructions (LU12I.W, LU32I.D, PCADDU12I, PCADDU18I) are not yet decoded by the disassembler. Kept for use when those decode paths are added.

## Verification
- `cargo clippy -p vuma-codegen 2>&1 | grep "dead_code"` → zero output (zero warnings)
- `cargo test -p vuma-codegen -- -q` → 675 passed, 0 failed
