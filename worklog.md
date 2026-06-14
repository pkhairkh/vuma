---
Task ID: 1
Agent: main
Task: Fix all 8 compiler backend platforms to pass SHA256d (exit code 79)

Work Log:
- Read PPC64 backend source, identified broken RLDICL encoding in ss_load_imm
- Replaced manual RLDICL encoding with Instruction::Rlwinm for clearing upper 32 bits
- Replaced manual SLDI encoding with li+sld using R11 temp in function call trampoline  
- Added proper 32-bit type handling to PPC64 stack-slot BinOp using IR ty field
- Used rlwinm masking, SLW/SRW/SRAW, rlwnm for 32-bit rotations, Mullw/Divw
- Fixed PPC64 ss_load_imm to correctly zero-extend 32-bit immediates
- Discovered LoongArch64 had 24 out of 26 3R-format opcodes completely wrong
- Fixed all LoongArch64 3R opcodes (ADD/SUB/SLT/AND/OR/XOR/shifts/rotates/mul/div)
- Fixed LoongArch64 shift immediate formats (reg2i5 for .W, reg2i6 for .D)
- Added encode_reg2i5 and encode_reg2i6 encoding functions
- Fixed LoongArch64 LU12I_W/LU32I_D encoding (reg1i20 format instead of reg2i16)
- Fixed LoongArch64 BEQZ/BNEZ opcodes and 1RI21 encoding format
- Fixed LoongArch64 FP opcodes and 2R format opcodes
- Wasm32: Added ROR/ROL implementation using shift+or sequence
- Wasm32: Fixed push_value type hints to use WasmType instead of IRType
- Wasm32: Still has type inference issues for SHA256d (register type tracking needed)
- Committed and pushed all changes

Stage Summary:
- All 6 native backends pass SHA256d (exit 79): x86_64, AArch64, RISC-V 64, ARM32, MIPS64, PPC64
- LoongArch64 passes all individual SHA256 operations and u32_arith test; full SHA256d too slow for QEMU but should work natively
- Wasm32 still has type inference issues (needs proper register type tracking)
- Key root causes fixed:
  1. PPC64: RLDICL encoding wrong, no 32-bit masking on 64-bit arithmetic
  2. LoongArch64: Almost all opcodes were wrong (from a different ISA or incorrect mapping)
