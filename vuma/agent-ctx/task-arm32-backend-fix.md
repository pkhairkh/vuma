# ARM32 Backend Fix — Task Summary

## Task
Fix the ARM32 backend in VUMA to produce working binaries that correctly exit via Linux syscall instead of crashing on return from the ELF entry point.

## Bugs Fixed

### Bug 1: ELF entry point uses return instruction instead of exit syscall
**Root cause**: When the kernel jumps to the ELF entry point, there is no return address on the stack. The `POP {R11, PC}` epilogue pops garbage into PC, causing a crash.

**Fix**: In `encode_program()`, after concatenating all function code, find the last 8 bytes of the first function's last instruction (which is always the epilogue: `MOV SP, R11` + `POP {R11, PC}`) and replace them with the Linux exit syscall:
- `MOV R7, #1` (0xE3A07001) — sys_exit syscall number
- `SWI 0x0` (0xEF000000) — invoke kernel

The return value already in R0 becomes the exit code. Both the old epilogue and the replacement are exactly 8 bytes, so no code buffer expansion was needed.

### Bug 2: R11/FP included in allocatable registers
**Root cause**: R11 is the frame pointer (FP) in ARM32 AAPCS. It is set up in the prologue (`MOV R11, SP`) and used in the epilogue (`MOV SP, R11`). If the register allocator assigns R11 to a virtual register, the frame pointer gets clobbered, corrupting the stack frame.

**Fix**: 
- Removed `Gpr::R11` from the allocatable list in `allocate_registers()`
- Updated `is_allocatable()` to exclude `Gpr::R11` alongside SP/LR/PC
- Updated the test `test_gpr_allocatable()` to assert `!Gpr::R11.is_allocatable()`

### Bug 3: Alloc instruction double-decrements SP
**Root cause**: The prologue already reserves stack space for all Alloc instructions via `SUB SP, SP, #frame_size` (where `frame_size` includes all Alloc sizes). The Alloc instruction then also decremented SP with `SUB SP, SP, #size`, effectively allocating the space twice.

**Fix**: Changed Alloc to compute the allocation address from the frame pointer (R11) instead of decrementing SP:
- Added `alloc_offset: i32` tracking variable (starts at 0, increments by aligned_size for each Alloc)
- Replaced `SUB SP, SP, #size` + `ADD d, SP, #0` with `SUB d, R11, #alloc_offset`
- For offsets that don't fit in ARM rotated-immediate format, loads offset into R12 scratch register first: `load_immediate_arm32(R12, offset)` + `SUB d, R11, R12`

## Files Changed
- `src/codegen/src/arm32/mod.rs` — All three fixes applied to this file

## Verification
- Build: `cargo build --workspace` succeeds
- Test: `cargo run --bin vuma -- emit arm32 examples/sha256d.vuma -o /tmp/sha256d_arm32.bin` produces a 3376-byte ELF32 binary
- Execution: `/tmp/qemu-arm-static /tmp/sha256d_arm32.bin` exits with **code 0**
- Binary analysis confirms:
  - Exit syscall (`MOV R7, #1` + `SWI 0x0`) correctly replaces first function's epilogue
  - New Alloc pattern (`SUB Rd, R11, #offset`) present in functions with allocations
  - Old Alloc pattern (`ADD Rd, SP, #0`) completely eliminated
