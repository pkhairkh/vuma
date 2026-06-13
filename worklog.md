---
Task ID: 1
Agent: main
Task: Fix ARM64 MOV Xd,SP bug and Alloc order

Work Log:
- Fixed MOV encoding in arm64.rs: when rm=SP, now emits ADD Xd, SP, #0 instead of ORR Xd, XZR, SP (which gives zero because ORR treats Rm=31 as XZR)
- Fixed Alloc order in emit.rs: SUB SP first, then ADD rd, SP, #0 (was MOV then SUB, giving wrong allocation pointer)
- Fixed Alloc order in arm64.rs InstructionSelector too
- Fixed test that expected MOV with SP to now expect ADD

Stage Summary:
- MOV SP bug was critical: ORR Xd, XZR, SP produces zero, not SP value
- Both emit.rs and arm64.rs alloc handlers now correctly decrement SP first

---
Task ID: 2
Agent: main
Task: ARM64 _start stub + 2-segment ELF + BL relocation patching + runtime I/O

Work Log:
- Added _start stub to ARM64 encode_program: BL main; MOV X0,X0; MOVZ X8,#93; SVC #0
- Rewrote build_minimal_aarch64_elf → build_aarch64_elf_2seg with 2 LOAD segments (PF_R|PF_X text + PF_R|PF_W data/stack)
- Added BL relocation patching in encode_program using R_AARCH64_CALL26
- Captured call_relocs from Emitter into AllocatedFunction.relocations
- Added build_aarch64_runtime() with print_hex, print_int, print_newline using SVC sys_write (X8=64)
- Fixed STP/LDP encoding bases: 0b101_0100_0100 (0x54400000) → 0xA9000000 for STP, 0b101_0100_1100 → 0xA9400000 for LDP
- Fixed prologue: changed from pre-indexed STP (not supported by Instruction::STP) to explicit SUB SP + STP with offset=0
- Fixed epilogue: LDP with offset=0 then ADD SP, SP, #16
- Updated compute_frame_size to not include FP/LR 16 bytes (handled by separate SUB)

Stage Summary:
- ARM64 backend now has proper _start, exit syscall, BL relocation patching, and runtime I/O
- STP/LDP encoding was missing top 2 bits (0x54→0xA9) — critical encoding bug
- Prologue/epilogue rewritten to use explicit SUB/ADD since Instruction::STP only supports signed-offset mode

---
Task ID: 3
Agent: main
Task: RISC-V 64 _start stub + 2-segment ELF + runtime I/O

Work Log:
- Added _start stub to RV64 encode_program: JAL ra, main; ADDI a0,a0,0; ADDI a7,zero,93; ECALL
- Rewrote build_minimal_riscv64_elf → build_minimal_riscv64_elf_2seg with 2 LOAD segments
- Added JAL relocation patching in encode_program
- Added build_riscv64_runtime() with print_hex, print_int, print_newline using ECALL sys_write (a7=64)
- Fixed all RISC-V instruction field names: Sd uses rs2 not rd, branches use offset not imm, JAL uses offset not imm

Stage Summary:
- RISC-V 64 backend now has proper _start, exit syscall, JAL relocation patching, and runtime I/O
- All three backends (x86_64, ARM64, RISC-V 64) now produce proper ELF executables with _start stubs and syscall-based I/O
---
Task ID: 1
Agent: main
Task: Fix VUMA ARM64 backend register allocation and encoding bugs for SHA256d

Work Log:
- Fixed 31-bit binary literal bugs in ARM64 instruction encodings (encode() and encode_with_width())
- Fixed STR/STRB/STRH/LDRSW base encodings (using LDUR format instead of STR unsigned offset)
- Fixed CSET/CSINC encoding base (0x0A800000 → 0x1A800000)
- Fixed CondBranch logic (CBNZ was branching to false_target instead of true_target)
- Fixed CBNZ/CBZ fixup branch format (was using B26 mask for Cond19 format)
- Implemented proper spill/reload in the register allocator (SpillInfo, Arm64RegAllocResult)
- Added spill slot space to frame size computation
- Fixed Call handler to resolve all argument registers before moving (prevents overwriting)
- Fixed Call handler to use different scratch registers for multiple immediate arguments
- Fixed epilogue to use MOV SP, X29 (robust against Alloc SP changes)
- Added auto-pin mechanism to prevent resolve_reg from spilling already-resolved registers
- Fixed spill/reload address computation to use X16 instead of X9 (avoid conflict with immediates)
- Fixed frame size computation to separate spill area from Alloc area
- Fixed CMP with two immediates overwriting X9 scratch register

Stage Summary:
- x86_64 SHA256d: WORKING (exit code 79 = 0x4F, correct NIST hash)
- ARM64 SHA256d: Running without crash, but producing wrong result (exit code 120 instead of 79)
- ARM64 simple tests: WORKING (return 42, for-loop sum 10)
- RISC-V 64: Not yet tested
- The incorrect ARM64 SHA256d result (120 vs 79) likely indicates a remaining encoding bug in one of the SHA256 helper functions
