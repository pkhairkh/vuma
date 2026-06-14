# ARM32 Stack-Slot Register Allocation Rewrite

## Task
Rewrite `allocate_registers` in the ARM32 backend (`/home/z/my-project/vuma/src/codegen/src/arm32/mod.rs`) to use a stack-slot approach instead of the old round-robin register allocation that failed under high register pressure.

## Problem
The old approach tried to map vregs to physical registers (R0-R12), spilling to stack when registers ran out. With SHA256d's 147 vregs but only ~12 allocatable GPRs, most vregs ended up spilled, and the spill handling was buggy — values got clobbered causing segfaults.

## Solution
Rewrote to a pure stack-slot approach (same pattern as AArch64 and RISC-V64):
1. Every vreg gets a dedicated 4-byte stack slot at a known offset from R11 (frame pointer)
2. R0-R3 used as scratch registers for computation
3. Each operation: load operands from stack slots → compute in scratch regs → store result back
4. R11 as frame pointer, R12 as temp for large offsets

## Stack Layout
- After prologue: R11 = SP + frame_size (points to saved R11)
- [R11, #0] = saved R11 (old FP)
- [R11, #4] = saved LR
- [R11, #-(4*(i+1))] = vreg slot i
- Alloc regions at larger negative offsets below vreg slots

## Key Changes
- Prologue: SUB SP, SP, #total; STR LR; STR R11; ADD R11, SP, #frame_size
- Parameters stored from R0-R3 to their stack slots in prologue
- Helper functions: `ss_load_from_slot`, `ss_store_to_slot`, `ss_load_value`
- Branch fixup tracking (instead of relocations for intra-function branches)
- Epilogue: LDR R11; LDR LR; ADD SP; BX LR

## Test Results
- return 42: ✅ exit code 42
- SHA256d: ✅ exit code 79
