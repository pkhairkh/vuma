# Multi-Architecture ISA Research for VUMA Codegen

**VUMA Project — Specification Document**
**Purpose: Research all target ISAs for the multi-architecture compiler backend**
**Date:** June 2026

---

> **Implementation note (2026-07):** This is a research document surveying each ISA's general characteristics. VUMA's actual codegen choices (in `src/codegen/`) may differ from the defaults listed here. Key VUMA-specific decisions: (1) **MIPS64 is Big-endian** in VUMA (not little-endian as the `mips64el` Rust target triple on line 858 might suggest); the codegen uses `to_be_bytes()` for MIPS64 emission. (2) **PPC64 uses ELFv2 ABI** (`e_flags = 0x2`), required by `qemu-ppc64`. (3) **Wasm32 uses a bump allocator** for `__vuma_alloc` (no mmap in Wasm). (4) The CLI `vuma emit`/`vuma compile` accept only 8 ISAs (missing RISC-V 32 and x86_32); all 10 backends are tested via `compile_dump`. Latest test results: 57,377/57,380 runs pass (99.99%).

## Overview

This document provides a comprehensive reference for each instruction set architecture that VUMA should target as a multi-architecture compiler. For each ISA, we detail register files, calling conventions, instruction encoding, ELF types, base addresses, key instructions for basic codegen, relevance domains, implementation complexity, and toolchain/ecosystem readiness.

VUMA's existing codegen (`src/codegen/`) currently targets only **AArch64** (ARM64/ARMv8-A). The information below is intended to guide the design of a retargetable codegen backend that shares the IR layer (`ir.rs`) and register allocation framework (`regalloc.rs`) while adding per-ISA instruction selection, encoding, and emission modules.

---

## 1. x86_64 (AMD64 / Intel 64)

### 1.1 Register File

| Category | Registers | Count | Notes |
|----------|-----------|-------|-------|
| GP regs | RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, R8–R15 | 16 | RSP = stack pointer, RBP = frame pointer |
| SIMD/FP (SSE) | XMM0–XMM15 | 16 (32 with AVX-512) | 128-bit each; YMM (256-bit) and ZMM (512-bit) with AVX/AVX-512 |
| x87 FP | ST(0)–ST(7) | 8 | Legacy; avoid in new code |
| Segment | CS, DS, ES, FS, GS, SS | 6 | FS/GS used for thread-local storage |
| Control/Status | RFLAGS, RIP | 2 | RIP is instruction pointer; RFLAGS has condition codes |
| System | CR0–CR4, DR0–DR7, MSRs | Many | Kernel-only |

### 1.2 Calling Convention — System V AMD64 ABI (Linux/macOS)

| Register(s) | Role |
|-------------|------|
| RDI, RSI, RDX, RCX, R8, R9 | Integer/pointer arguments 1–6 |
| XMM0–XMM7 | Floating-point arguments 1–8 |
| RAX | Return value (integer/pointer); also AL = number of vector args for variadics |
| XMM0 | Return value (floating-point) |
| RAX, RDX | 128-bit return (RDX = high half) |
| RBX, RBP, R12–R15 | Callee-saved |
| R10, R11 | Caller-saved (used by syscall too) |
| RSP | 16-byte aligned at call sites (mandatory) |

**Note:** Windows uses a different convention (Microsoft x64): RCX, RDX, R8, R9 for first 4 args; shadow store required.

### 1.3 Instruction Encoding

- **Variable-length:** 1–15 bytes per instruction
- **Endianness:** Little-endian
- **Prefix-based encoding:** REX/VEX/EVEX prefixes extend opcode space
- **Complex encoding:** ModR/M + SIB bytes for addressing modes; immediates vary in size
- **No fixed alignment:** Instructions can start at any byte boundary

### 1.4 ELF & Binary

| Property | Value |
|----------|-------|
| ELF machine type | `EM_X86_64` = 62 (0x3E) |
| ELF class | `ELFCLASS64` |
| ELF data | `ELFDATA2LSB` (little-endian) |
| Typical base address (Linux) | `0x400000` (legacy), `0x555555554000` (PIE) |
| Typical page size | 4 KB |

### 1.5 Key Instructions for Basic Codegen

| Category | Instructions |
|----------|-------------|
| Arithmetic | `ADD`, `SUB`, `IMUL`, `IDIV`, `INC`, `DEC`, `NEG` |
| Logic | `AND`, `OR`, `XOR`, `NOT`, `SHL`, `SHR`, `SAR` |
| Memory | `MOV`, `LEA`, `PUSH`, `POP`, `MOVSX`, `MOVZX` |
| Control flow | `JMP`, `JE/JNE`, `JL/JG`, `CALL`, `RET`, `CMP`, `TEST` |
| Conditional | `SETcc` (set byte on condition), `CMOVcc` (conditional move) |
| Immediate loads | `MOV reg, imm32/64`; for 64-bit: `MOV reg, imm32` + sign-extend, or `MOVABS reg, imm64` |

### 1.6 Relevance

| Domain | Relevant? |
|--------|-----------|
| Embedded | No (too power-hungry) |
| Mobile | No |
| Desktop | **Yes** — dominant |
| Server | **Yes** — dominant |
| Browser | No (not a Wasm target) |

### 1.7 Complexity: **8/10**

Variable-length encoding is the primary difficulty. The ModR/M + SIB + displacement + immediate encoding scheme has hundreds of valid combinations. REX prefix handling, the 1–15 byte variable length, and the lack of a simple "template" for instruction encoding make binary emission significantly harder than fixed-length RISC ISAs. However, the massive x86_64 ecosystem (assemblers, disassemblers, emulators) provides excellent reference material.

### 1.8 Toolchain & Cross-Compilation

| Property | Status |
|----------|--------|
| Rust target triple | `x86_64-unknown-linux-gnu` (tier 1) |
| Cross-compile from x86_64 Linux | N/A (native) |
| QEMU emulator | `qemu-system-x86_64` (user-mode: `qemu-x86_64`) |
| GNU cross-toolchain | Native; `x86_64-linux-gnu-*` available |
| LLVM support | Excellent (primary target) |

---

## 2. AArch64 (ARM64 / ARMv8-A)

### 2.1 Register File

| Category | Registers | Count | Notes |
|----------|-----------|-------|-------|
| GP regs | X0–X30 | 31 | 64-bit; W0–W30 are lower 32-bit views |
| Special | SP (X31), XZR (X31) | 2 | SP in load/store context; XZR in arithmetic |
| SIMD/FP | V0–V31 | 32 | 128-bit; D0–D31 (64-bit), S0–S31 (32-bit) views |
| System | NZCV, FPCR, FPSR, ELR_EL1, SPSR_EL1 | Many | Condition flags, FP control, exception regs |
| SVE (optional) | Z0–Z31 | 32 | Scalable Vector Extension, 128–2048 bits |
| SVE predicates | P0–P15 | 16 | Predicate registers for SVE |

### 2.2 Calling Convention — AAPCS64

| Register(s) | Role |
|-------------|------|
| X0–X7 | Integer/pointer arguments 1–8 and return values |
| X8 | Indirect result location register (large struct returns) |
| X9–X15 | Caller-saved temporaries |
| X16–X17 | Intra-procedure-call scratch (IP0/IP1, PLT veneers) |
| X18 | Platform register (shadow stack on Android) |
| X19–X28 | Callee-saved |
| X29 | Frame pointer (FP) |
| X30 | Link register (LR) |
| V0–V7 | FP/SIMD arguments 1–8 and return values |
| V8–V15 | Callee-saved (lower 64 bits only: D8–D15) |
| V16–V31 | Caller-saved temporaries |

**VUMA note:** This calling convention is already fully implemented in `src/codegen/src/ir.rs` (`compute_calling_conv`) and `src/codegen/src/arm64.rs`.

### 2.3 Instruction Encoding

- **Fixed-length:** 32 bits (4 bytes) for all instructions
- **Endianness:** Little-endian (mandatory on Linux; big-endian possible but unused)
- **Alignment:** Instructions must be 4-byte aligned
- **Clean encoding:** Top-level classification by bits [28:25]; systematic bit-field layout

### 2.4 ELF & Binary

| Property | Value |
|----------|-------|
| ELF machine type | `EM_AARCH64` = 183 (0xB7) |
| ELF class | `ELFCLASS64` |
| ELF data | `ELFDATA2LSB` |
| Typical base address (Linux) | `0x400000` |
| Typical base address (bare-metal AArch64) | `0x80000` |
| Typical page size | 4 KB (16 KB on Apple M-series) |

**VUMA note:** Already implemented in `src/codegen/src/emit.rs` with `EM_AARCH64` constant and both Linux and bare-metal base addresses.

### 2.5 Key Instructions for Basic Codegen

| Category | Instructions |
|----------|-------------|
| Arithmetic | `ADD`, `SUB`, `MUL`, `SDIV`, `UDIV`, `MADD`, `MSUB` |
| Logic | `AND`, `ORR`, `EOR`, `LSL`, `LSR`, `ASR`, `ROR` |
| Memory | `LDR`, `STR`, `LDP`, `STP`, `LDRB/H/SW`, `STRB/H` |
| Control flow | `B`, `BL`, `BR`, `BLR`, `RET`, `B.cond`, `CBZ`, `CBNZ`, `TBZ`, `TBNZ` |
| Conditional | `CSEL`, `CSET`, `CSINC` |
| Move | `MOV` (alias for ORR Xd, XZR, Xn), `MOVZ`, `MOVK` |
| Extend | `SXTW`, `SXTB`, `SXTH`, `UXTB`, `UXTH` (aliases for SBFM/UBFM) |
| Atomics | `LDXR`, `STXR`, `LDAXR`, `STLXR`, `CAS`, `LDAR`, `STLR` |
| Barriers | `DMB`, `DSB`, `ISB` |

**VUMA note:** All of the above are already implemented in `src/codegen/src/arm64.rs` with full binary encoding.

### 2.6 Relevance

| Domain | Relevant? |
|--------|-----------|
| Embedded | **Yes** — Cortex-A series |
| Mobile | **Yes** — dominant |
| Desktop | **Yes** — Apple M-series, Snapdragon X |
| Server | **Yes** — AWS Graviton, Ampere Altra |
| Browser | No (not a Wasm target) |

### 2.7 Complexity: **4/10**

Fixed-length 32-bit encoding with systematic bit-field layout makes binary emission straightforward. The instruction set is orthogonal and well-documented. The main complexity is the large number of instruction variants (shifted register, extended register, immediate forms), but each has a clean encoding template. This is VUMA's existing target and serves as the reference implementation for retargetable codegen.

### 2.8 Toolchain & Cross-Compilation

| Property | Status |
|----------|--------|
| Rust target triple | `aarch64-unknown-linux-gnu` (tier 1), `aarch64-unknown-none` (bare-metal, tier 2) |
| Cross-compile from x86_64 Linux | **Yes** — `aarch64-linux-gnu-gcc`, `rustup target add aarch64-unknown-linux-gnu` |
| QEMU emulator | `qemu-system-aarch64` (system), `qemu-aarch64` (user-mode) |
| GNU cross-toolchain | `aarch64-linux-gnu-*` widely available |
| LLVM support | Excellent |

---

## 3. RISC-V 64 (RV64GC)

### 3.1 Register File

| Category | Registers | Count | Notes |
|----------|-----------|-------|-------|
| GP regs (x0–x31) | x0 (zero), x1 (ra), x2 (sp), x3 (gp), x4 (tp), x5–x7 (t0–t2), x8–x9 (s0–s1), x10–x17 (a0–a7), x18–x27 (s2–s11), x28–x31 (t3–t6) | 32 | x0 is hardwired zero |
| FP regs (f0–f31) | f0–f7 (ft0–ft7), f8–f9 (fs0–fs1), f10–f17 (fa0–fa7), f18–f27 (fs2–fs11), f28–f31 (ft8–ft11) | 32 | 64-bit (D extension); F extension gives 32-bit single |
| CSR | Various | Hundreds | mstatus, sstatus, mepc, sepc, satp, etc. |
| Vector (V extension) | v0–v31 | 32 | Variable-length (128–4096 bits per reg, configurable) |

### 3.2 Calling Convention — RV64G ABI (LP64D)

| Register(s) | Role |
|-------------|------|
| a0–a7 (x10–x17) | Integer/pointer arguments 1–8 and return values (a0–a1) |
| fa0–fa7 (f10–f17) | FP arguments 1–8 and return values (fa0–fa1) |
| ra (x1) | Return address (link register) |
| sp (x2) | Stack pointer |
| gp (x3) | Global pointer (Global addresses) |
| tp (x4) | Thread pointer (TLS) |
| s0–s1 (x8–x9), s2–s11 (x18–x27) | Callee-saved (12 GP regs) |
| fs0–fs1 (f8–f9), fs2–fs11 (f18–f27) | Callee-saved FP (12 FP regs) |
| t0–t6 (x5–x7, x28–x31) | Caller-saved temporaries |
| ft0–ft11 (f0–f7, f28–f31) | Caller-saved FP temporaries |

**Stack alignment:** 16-byte aligned at call sites.

### 3.3 Instruction Encoding

- **Variable-length:** Base ISA is 32-bit fixed; RVC (compressed) extension adds 16-bit encodings
- **Endianness:** Little-endian (dominant); big-endian also specified
- **Alignment:** 32-bit instructions are 4-byte aligned; 16-bit (RVC) are 2-byte aligned
- **Base ISA encoding:** 4 major formats: R-type, I-type, S-type, U-type; plus B-type (branch), J-type (jump)
- **RVC encoding:** 8 major compressed formats; ~40 common instructions have 16-bit forms

### 3.4 ELF & Binary

| Property | Value |
|----------|-------|
| ELF machine type | `EM_RISCV` = 243 (0xF3) |
| ELF class | `ELFCLASS64` |
| ELF data | `ELFDATA2LSB` (LE) or `ELFDATA2MSB` (BE) |
| ELF flags | Specify ISA extensions: `EF_RISCV_RVC`, `EF_RISCV_FLOAT_ABI_SOFT/SINGLE/DOUBLE/QUAD` |
| Typical base address (Linux) | `0x10000` (default), `0x400000` (some distros) |
| Typical page size | 4 KB |

### 3.5 Key Instructions for Basic Codegen

| Category | Instructions |
|----------|-------------|
| Arithmetic | `ADD`, `SUB`, `ADDI`, `MUL`, `DIV`, `DIVU`, `REM`, `REMU` (M extension) |
| Logic | `AND`, `OR`, `XOR`, `ANDI`, `ORI`, `XORI` |
| Shift | `SLL`, `SRL`, `SRA`, `SLLI`, `SRLI`, `SRAI` |
| Memory | `LD`, `SD`, `LW`, `SW`, `LH`, `SH`, `LB`, `SB`, `LWU`, `LHU` |
| Address | `LUI`, `AUIPC` (upper immediate for PC-relative addressing) |
| Control flow | `JAL`, `JALR`, `BEQ`, `BNE`, `BLT`, `BGE`, `BLTU`, `BGEU` |
| Compare | `SLT`, `SLTU`, `SLTI`, `SLTIU` (set on less-than) |
| FP (D ext) | `FADD.D`, `FSUB.D`, `FMUL.D`, `FDIV.D`, `FCVT.D.S`, `FCVT.S.D`, `FMV.X.D`, `FMV.D.X` |
| Extend | `SLLI` for zero-extend; `SRAI` for sign-extend |
| Atomics (A ext) | `LR.D`, `SC.D`, `AMOSWAP.D`, `AMOADD.D`, etc. |
| Fence | `FENCE`, `FENCE.I` |

### 3.6 Relevance

| Domain | Relevant? |
|--------|-----------|
| Embedded | **Yes** — rapidly growing (SiFive, StarFive) |
| Mobile | Emerging (not yet mainstream) |
| Desktop | Emerging (SiFive HiFive, RISC-V laptops) |
| Server | Emerging (datacenter SoCs in development) |
| Browser | No |

### 3.7 Complexity: **3/10**

RISC-V is arguably the easiest 64-bit ISA to implement a codegen for. The base ISA (RV64I) has only ~50 instructions with 4 clean encoding formats. No complex addressing modes — all memory ops are register + 12-bit signed immediate. The variable-length RVC extension adds moderate complexity but is optional. The main gotcha is that some operations require multi-instruction sequences (e.g., 32-bit immediate loads use LUI+ADDI; function calls may need AUIPC+JALR for range).

### 3.8 Toolchain & Cross-Compilation

| Property | Status |
|----------|--------|
| Rust target triple | `riscv64gc-unknown-linux-gnu` (tier 2), `riscv64imac-unknown-none-elf` (tier 2) |
| Cross-compile from x86_64 Linux | **Yes** — `riscv64-linux-gnu-gcc`, `rustup target add riscv64gc-unknown-linux-gnu` |
| QEMU emulator | `qemu-system-riscv64` (system), `qemu-riscv64` (user-mode) |
| GNU cross-toolchain | `riscv64-linux-gnu-*` available |
| LLVM support | Good (RISC-V is a first-class LLVM target since LLVM 9) |

---

## 4. WebAssembly (Wasm32 + Wasm64)

### 4.1 Register File

WebAssembly is a **stack machine**, not a register machine. There are no named registers.

| Category | Description |
|----------|-------------|
| Value stack | Operand stack; implicitly popped/pushed by instructions |
| Local variables | Per-function locals (typed); accessed by index; effectively infinite |
| Global variables | Per-module globals (mutable or immutable); accessed by index |
| Table | Indirect function references (for dynamic dispatch) |
| Memory | Linear memory (byte-addressable array buffer); 1+ pages (64 KB each) |
| PC | Program counter; implicit |

**Wasm64 vs Wasm32:** Wasm64 uses 64-bit addresses/indices (pointers are i64) while Wasm32 uses 32-bit (pointers are i32). Everything else is identical.

### 4.2 Calling Convention

WebAssembly has no ABI in the traditional sense — the stack machine model abstracts it away:

| Property | Description |
|----------|-------------|
| Arguments | Pushed on the value stack before `call`; callee receives them as locals |
| Return values | Pushed on the value stack before `end`/`return`; callee's results |
| Callee-saved | N/A — locals are per-function; there are no shared registers |
| Stack | The WebAssembly value stack is separate from the "shadow stack" in linear memory |
| Multi-value | Wasm MVP supports 1 return value; Multi-value extension allows multiple |

**Interface to host:** When Wasm calls imported functions, the Wasm-to-native ABI is defined by the embedder (wasmi, Wasmtime, etc.). The standard Wasm C ABI (wasm32-wasi) maps to the stack-based calling convention with shadow stack.

### 4.3 Instruction Encoding

- **Variable-length:** 1+ bytes per instruction; LEB128-encoded immediates
- **Endianness:** Little-endian for all multi-byte values
- **Binary format:** Structured as a module with sections (Type, Import, Function, Table, Memory, Global, Export, Start, Element, Code, Data)
- **Code section:** Sequence of functions; each function body is a byte vector
- **Alignment:** No alignment requirement for instructions (byte-addressable)

### 4.4 ELF & Binary

| Property | Value |
|----------|-------|
| Format | **Not ELF** — `.wasm` binary format (magic: `\0asm`, version 1) |
| Object format | `.wasm` files; or `*.o` with Wasm object format for linking |
| No ELF machine type | N/A |
| Base address | N/A — linear memory starts at address 0 by default |
| Page size | 64 KB (minimum memory unit) |

### 4.5 Key Instructions for Basic Codegen

| Category | Instructions |
|----------|-------------|
| Arithmetic (i32) | `i32.add`, `i32.sub`, `i32.mul`, `i32.div_s`, `i32.div_u`, `i32.rem_s`, `i32.rem_u` |
| Arithmetic (i64) | `i64.add`, `i64.sub`, `i64.mul`, `i64.div_s`, `i64.div_u`, `i64.rem_s`, `i64.rem_u` |
| Logic | `i32.and`, `i32.or`, `i32.xor`, `i32.shl`, `i32.shr_u`, `i32.shr_s`, `i32.rotl`, `i32.rotr` |
| FP | `f32.add`, `f32.sub`, `f32.mul`, `f32.div`, `f64.add`, etc. |
| Memory | `i32.load`, `i32.store`, `i64.load`, `i64.store`, `i32.load8_u`, `i32.store8`, etc. |
| Constants | `i32.const`, `i64.const`, `f32.const`, `f64.const` (LEB128-encoded) |
| Control flow | `block`, `loop`, `if`/`else`/`end`, `br`, `br_if`, `br_table`, `return` |
| Call | `call`, `call_indirect` |
| Local | `local.get`, `local.set`, `local.tee` |
| Global | `global.get`, `global.set` |
| Compare | `i32.eq`, `i32.ne`, `i32.lt_s`, `i32.lt_u`, `i32.gt_s`, `i32.gt_u`, `i32.le_s`, `i32.ge_s`, etc. |
| Convert | `i32.wrap_i64`, `i64.extend_i32_s`, `i64.extend_i32_u`, `i32.trunc_f64_s`, etc. |
| Memory size/grow | `memory.size`, `memory.grow` |

### 4.6 Relevance

| Domain | Relevant? |
|--------|-----------|
| Embedded | Emerging (WASM micro runtimes) |
| Mobile | Emerging |
| Desktop | Emerging (WASM outside browser) |
| Server | Emerging (WASM edge computing, Fermyon, Fastly) |
| Browser | **Yes** — dominant (the only ISA that runs natively in browsers) |

### 4.7 Complexity: **2/10**

WebAssembly is by far the easiest target to implement. No register allocation needed (stack machine). No binary encoding complexity (LEB128 + simple opcodes). No calling convention to manage. The main work is: (1) emitting structured control flow (blocks/loops with branch labels), (2) managing the value stack depth, and (3) generating the module structure with sections. A basic Wasm32 codegen can be implemented in ~1000 lines of Rust.

### 4.8 Toolchain & Cross-Compilation

| Property | Status |
|----------|--------|
| Rust target triple | `wasm32-unknown-unknown` (tier 2), `wasm32-wasip1` (tier 2), `wasm32-wasip2` (tier 3) |
| Cross-compile from x86_64 Linux | **Yes** — `rustup target add wasm32-unknown-unknown` |
| Emulator/runtime | Wasmtime, Wasmer, wasmi, Node.js, Chrome/Firefox, WasmEdge |
| Toolchain | `wasm-ld` (LLD), `wasm-opt`, `wasm2wat`, `wat2wasm` |
| LLVM support | Excellent (primary Wasm backend) |

---

## 5. ARM32 (AArch32 / ARMv7-A)

### 5.1 Register File

| Category | Registers | Count | Notes |
|----------|-----------|-------|-------|
| GP regs | R0–R12, SP (R13), LR (R14), PC (R15) | 16 | 32-bit each |
| SIMD/FP (VFPv3/NEON) | D0–D31 (or S0–S63) | 32×64-bit | D0–D15 = S0–S31; D16–D31 = extended (VFPv3-D32) |
| NEON | Q0–Q15 | 16×128-bit | Q0 = D0+D1, etc. |
| CPSR | Current Program Status Register | 1 | N, Z, C, V flags + mode bits |
| Banked | R8_fiq–R14_fiq, R13_svc, R14_svc, etc. | Per-mode | FIQ, SVC, ABT, IRQ, UND modes |

### 5.2 Calling Convention — AAPCS (ARM32)

| Register(s) | Role |
|-------------|------|
| R0–R3 | Integer/pointer arguments 1–4 and return values (R0, R0+R1 for 64-bit) |
| R4–R11 | Callee-saved (V1–V8 in AAPCS naming) |
| R9 | Platform-specific (may be callee-saved or reserved for SB/TR) |
| R12 (IP) | Intra-procedure-call scratch |
| SP (R13) | Stack pointer |
| LR (R14) | Link register |
| PC (R15) | Program counter |
| S0–S15 / D0–D7 | FP arguments and return values |
| S16–S31 / D8–D15 | Callee-saved FP |

**Stack alignment:** 8-byte aligned at public interfaces (AAPCS); some systems require 16-byte.

### 5.3 Instruction Encoding

- **Variable-length:** ARM = 32-bit; Thumb = 16-bit; Thumb-2 = mixed 16/32-bit
- **Endianness:** Bi-endian (LE dominant on Linux)
- **Alignment:** ARM: 4-byte; Thumb: 2-byte
- **Conditional execution:** Most ARM instructions have 4-bit condition field (can execute conditionally without branch)
- **Thumb-2:** IT blocks allow conditional Thumb instructions; complex encoding

### 5.4 ELF & Binary

| Property | Value |
|----------|-------|
| ELF machine type | `EM_ARM` = 40 (0x28) |
| ELF class | `ELFCLASS32` |
| ELF data | `ELFDATA2LSB` (LE) or `ELFDATA2MSB` (BE) |
| ELF flags | `EF_ARM_ABI_VER5`, `EF_ARM_SOFT_FLOAT`/`EF_ARM_VFP_FLOAT` |
| Typical base address (Linux) | `0x10000` |
| Typical page size | 4 KB |

### 5.5 Key Instructions for Basic Codegen

| Category | Instructions (ARM mode) |
|----------|------------------------|
| Arithmetic | `ADD`, `SUB`, `MUL`, `MLA`, `UMULL`, `SMULL`, `SDIV`, `UDIV` (ARMv7-A) |
| Logic | `AND`, `ORR`, `EOR`, `BIC`, `LSL`, `LSR`, `ASR`, `ROR` |
| Memory | `LDR`, `STR`, `LDM`, `STM`, `LDRB/H/SB/SH`, `STRB/H`, `PUSH`, `POP` |
| Control flow | `B`, `BL`, `BX`, `BLX`, `B<cond>`, `CBZ`, `CBNZ` (Thumb-2) |
| Conditional | Conditional suffix on any ARM instruction: `ADDGE`, `MOVEQ`, etc. |
| Move | `MOV`, `MOVT` (top 16 bits), `MVN` |
| Extend | `SXTB`, `SXTH`, `UXTB`, `UXTH` |
| Atomics | `LDREX`, `STREX`, `LDREXB/H/D`, `STREXB/H/D` |
| Barriers | `DMB`, `DSB`, `ISB` |

### 5.6 Relevance

| Domain | Relevant? |
|--------|-----------|
| Embedded | **Yes** — still extremely common (Cortex-M, Cortex-R, Cortex-A7/A9) |
| Mobile | Legacy (pre-ARMv8 Android phones) |
| Desktop | No |
| Server | No |
| Browser | No |

### 5.7 Complexity: **6/10**

The ARM32 ISA is more complex than AArch64 due to: (1) variable-length encoding (ARM/Thumb/Thumb-2), (2) conditional execution on every ARM-mode instruction, (3) the IT block mechanism in Thumb-2, (4) banked registers in different processor modes, (5) the PC being a general register (leads to implicit side effects), (6) the complex multi-register load/store (LDM/STM) with addressing mode options. However, for a basic codegen targeting only ARM mode (no Thumb), complexity drops to ~4/10.

### 5.8 Toolchain & Cross-Compilation

| Property | Status |
|----------|--------|
| Rust target triple | `armv7-unknown-linux-gnueabihf` (tier 2), `arm-unknown-linux-gnueabi` (tier 2) |
| Cross-compile from x86_64 Linux | **Yes** — `arm-linux-gnueabihf-gcc`, `rustup target add armv7-unknown-linux-gnueabihf` |
| QEMU emulator | `qemu-system-arm` (system), `qemu-arm` (user-mode) |
| GNU cross-toolchain | `arm-linux-gnueabihf-*` widely available |
| LLVM support | Good |

---

## 6. MIPS64 (MIPS III+)

### 6.1 Register File

| Category | Registers | Count | Notes |
|----------|-----------|-------|-------|
| GP regs | $0 (zero), $1 (at), $2–$3 (v0–v1), $4–$7 (a0–a3), $8–$15 (t0–t7), $16–$23 (s0–s7), $24–$25 (t8–t9), $26–$27 (k0–k1), $28 (gp), $29 (sp), $30 (fp/s8), $31 (ra) | 32 | $0 is hardwired zero |
| FP regs | $f0–$f31 | 32 | 64-bit (MIPS III); paired singles ($f0+$f1 = 128-bit) in MIPS64r2 |
| HI/LO | Multiply/divide result registers | 2 | Used by `MULT`/`DIV` instructions |
| CP0 | System coprocessor registers | ~32 | Status, Cause, EPC, etc. |

### 6.2 Calling Convention — O64 ABI (MIPS64 Linux)

| Register(s) | Role |
|-------------|------|
| $a0–$a3 ($4–$7) | Integer arguments 1–4 |
| $f12–$f15 | FP arguments 1–4 (O64 ABI; N64 uses $f12–$f19) |
| $v0–$v1 ($2–$3) | Return values |
| $f0–$f1 | FP return values |
| $s0–$s7 ($16–$23) | Callee-saved |
| $gp ($28) | Global pointer |
| $sp ($29) | Stack pointer |
| $fp/s8 ($30) | Frame pointer (callee-saved) |
| $ra ($31) | Return address (callee-saved) |
| $t0–$t9 ($8–$15, $24–$25) | Caller-saved temporaries |

**Note:** There are 3 MIPS64 ABIs: O32 (32-bit, compat), N32 (64-bit regs, 32-bit pointers), N64 (full 64-bit). N64 is the native 64-bit ABI.

**Stack alignment:** 16-byte aligned.

**Branch delay slots:** MIPS has branch delay slots — the instruction after a branch is always executed. This is a critical codegen consideration.

### 6.3 Instruction Encoding

- **Fixed-length:** 32 bits for all instructions
- **Endianness:** Bi-endian; most Linux systems are big-endian (SGI, some MIPS); little-endian growing (Loongson, some embedded)
- **Alignment:** 4-byte aligned
- **3-operand format:** Most instructions are 3-register: `add $d, $s, $t`

### 6.4 ELF & Binary

| Property | Value |
|----------|-------|
| ELF machine type | `EM_MIPS` = 8 (0x08) |
| ELF class | `ELFCLASS64` |
| ELF data | `ELFDATA2MSB` (BE) or `ELFDATA2LSB` (LE) |
| ELF flags | `EF_MIPS_ABI_O64`, `EF_MIPS_ARCH_64`, etc. |
| Typical base address (Linux) | `0x120000000` (N64, big-endian), `0x400000` (little-endian) |
| Typical page size | 4 KB (64 KB on some systems) |

### 6.5 Key Instructions for Basic Codegen

| Category | Instructions |
|----------|-------------|
| Arithmetic | `DADD`, `DSUB`, `DMUL`, `DDIV`, `DADDI`, `DADDIU` (64-bit); `ADD`, `SUB` (32-bit) |
| Logic | `AND`, `OR`, `XOR`, `NOR`, `DSLL`, `DSRL`, `DSRA`, `DSLLV`, `DSRLV`, `DSRAV` |
| Memory | `LD`, `SD`, `LW`, `SW`, `LH`, `SH`, `LB`, `SB`, `LWU`, `LHU`, `LBU` |
| Address | `LUI`, `DADDIU` (for 64-bit address construction) |
| Control flow | `BEQ`, `BNE`, `BGTZ`, `BLEZ`, `BGEZ`, `BLTZ`, `J`, `JAL`, `JR`, `JALR` |
| Compare | `SLT`, `SLTU`, `SLTI`, `SLTIU` |
| Move | `MOVE` (pseudo: `or $d, $s, $zero`), `DMTC1`, `DMFC1` (GP↔FP) |
| Multiply/Divide | `DMULT`, `DMULTU`, `DDIV`, `DDIVU` → result in HI/LO → `MFLO`, `MFHI` |
| Atomics | `LL`, `SC` (load-linked, store-conditional) |
| Barriers | `SYNC`, `SYNCI` |

### 6.6 Relevance

| Domain | Relevant? |
|--------|-----------|
| Embedded | **Yes** — routers, IoT (MT7621, etc.) |
| Mobile | No (historically some) |
| Desktop | No |
| Server | No (historically SGI) |
| Browser | No |

### 6.7 Complexity: **5/10**

MIPS64 has a clean 32-bit fixed-length encoding and orthogonal instruction set. The main complications are: (1) **branch delay slots** — every branch/jump instruction is followed by an instruction that always executes, requiring the codegen to fill (or NOP) the slot, (2) multiply/divide results go to HI/LO registers rather than a destination register, requiring MFLO/MFHI to retrieve results, (3) no direct condition codes — comparisons write to a register (SLT/SLTU), (4) address construction requires LUI + DADDIU sequences for large constants.

### 6.8 Toolchain & Cross-Compilation

| Property | Status |
|----------|--------|
| Rust target triple | `mips64-unknown-linux-gnuabi64` (tier 2), `mips64el-unknown-linux-gnuabi64` (tier 2) |
| Cross-compile from x86_64 Linux | **Yes** — `mips64-linux-gnuabi64-gcc` |
| QEMU emulator | `qemu-system-mips64` (system), `qemu-mips64` (user-mode) |
| GNU cross-toolchain | `mips64-linux-gnuabi64-*` available |
| LLVM support | Good |

---

## 7. LoongArch64

### 7.1 Register File

| Category | Registers | Count | Notes |
|----------|-----------|-------|-------|
| GP regs | $r0 (zero), $r1 (ra), $r2 (tp), $r3 (sp), $r4–$r5 (a0–a1), $r6–$r7 (a2–a3), $r8–$r9 (a4–a5), $r10–$r11 (a6–a7), $r12–$r20 (t0–t8), $r21 (reserved), $r22 (fp/s9), $r23–$r31 (s0–s8) | 32 | 64-bit each; $r0 hardwired zero |
| FP regs | $f0–$f1 (fa0–fa1), $f2–$f3 (fa2–fa3), $f4–$f5 (fa4–fa5), $f6–$f7 (fa6–fa7), $f8–$f23 (ft0–ft15), $f24–$f31 (fs0–fs7) | 32 | 64-bit each; also accessible as 32-bit (single) |
| CSR | Various | Many | CRMD, PRMD, ESTAT, EENTRY, etc. |

### 7.2 Calling Convention — LoongArch64 LP64 ABI

| Register(s) | Role |
|-------------|------|
| $a0–$a7 ($r4–$r11) | Integer/pointer arguments 1–8 and return values ($a0–$a1) |
| $fa0–$fa7 ($f0–$f7) | FP arguments 1–8 and return values ($fa0–$fa1) |
| $ra ($r1) | Return address |
| $sp ($r3) | Stack pointer |
| $fp/s9 ($r22) | Frame pointer (callee-saved) |
| $s0–$s8 ($r23–$r31) | Callee-saved (9 GP regs) |
| $fs0–$fs7 ($f24–$f31) | Callee-saved FP (8 FP regs) |
| $t0–$t8 ($r12–$r20) | Caller-saved temporaries |
| $ft0–$ft15 ($f8–$f23) | Caller-saved FP temporaries |
| $tp ($r2) | Thread pointer (TLS) |

**Stack alignment:** 16-byte aligned.

### 7.3 Instruction Encoding

- **Fixed-length:** 32 bits for all instructions
- **Endianness:** Little-endian only
- **Alignment:** 4-byte aligned
- **Format types:** 9 major encoding formats: 2R, 3R, 4R, 2RI8, 2RI12, 2RI14, 2RI16, 1RI21, I26
- **Clean design:** Heavily inspired by MIPS and RISC-V; orthogonal instruction set

### 7.4 ELF & Binary

| Property | Value |
|----------|-------|
| ELF machine type | `EM_LOONGARCH` = 258 (0x102) |
| ELF class | `ELFCLASS64` |
| ELF data | `ELFDATA2LSB` |
| ELF flags | ABI version flags |
| Typical base address (Linux) | `0x120000000` |
| Typical page size | 16 KB |

### 7.5 Key Instructions for Basic Codegen

| Category | Instructions |
|----------|-------------|
| Arithmetic | `ADD.D`, `SUB.D`, `MUL.D`, `DIV.D`, `MOD.D`, `ADDI.D`, `ADDIU16I.D` |
| Logic | `AND`, `OR`, `XOR`, `NOR`, `SLL.D`, `SRL.D`, `SRA.D`, `ROTR.D` |
| Memory | `LD.D`, `ST.D`, `LD.W`, `ST.W`, `LD.H`, `ST.H`, `LD.B`, `ST.B`, `LD.WU`, `LD.HU`, `LD.BU` |
| Address | `LU12I.W`, `LU32I.D`, `LU52I.D` (for 64-bit immediate construction) |
| Control flow | `BEQ`, `BNE`, `BLT`, `BGE`, `BLTU`, `BGEU`, `BEQZ`, `BNEZ`, `B`, `BL`, `JIRL` |
| Compare | `SLT`, `SLTU`, `SLTI`, `SLTUI` |
| Move | `MOVGR2FR.D`, `MOVFR2GR.D` (GP↔FP) |
| Extend | `EXT.W.B`, `EXT.W.H`, `EXT.W.W` (sign-extend byte/half/word to doubleword) |
| Atomics | `LL.D`, `SC.D`, `AMSWAP.D`, `AMADD.D`, etc. |
| Barriers | `DBAR`, `IBAR` |

### 7.6 Relevance

| Domain | Relevant? |
|--------|-----------|
| Embedded | Emerging (Loongson embedded SoCs) |
| Mobile | No |
| Desktop | **Yes** — China domestic (Loongson 3A5000/3A6000) |
| Server | **Yes** — China domestic (Loongson 3C5000) |
| Browser | No |

### 7.7 Complexity: **3/10**

LoongArch64 is very similar to RISC-V in design philosophy — clean, orthogonal, fixed-length 32-bit encoding. The register file and calling convention are straightforward. The main difference from RISC-V is the 3-instruction sequence for 64-bit immediate loads (LU12I.W + LU32I.D + LU52I.D instead of RISC-V's LUI + ADDI). No branch delay slots. No condition codes. An experienced codegen implementer could add LoongArch64 support in 1–2 weeks given the existing IR framework.

### 7.8 Toolchain & Cross-Compilation

| Property | Status |
|----------|--------|
| Rust target triple | `loongarch64-unknown-linux-gnu` (tier 2, since Rust 1.71) |
| Cross-compile from x86_64 Linux | **Yes** — `loongarch64-linux-gnu-gcc`, `rustup target add loongarch64-unknown-linux-gnu` |
| QEMU emulator | `qemu-system-loongarch64` (system, since QEMU 7.2), `qemu-loongarch64` (user-mode) |
| GNU cross-toolchain | `loongarch64-linux-gnu-*` available (Loongson provides packages) |
| LLVM support | Good (upstream since LLVM 16) |

---

## 8. PowerPC64 (POWER9+)

### 8.1 Register File

| Category | Registers | Count | Notes |
|----------|-----------|-------|-------|
| GP regs | R0–R31 | 32 | 64-bit; R0 sometimes implied zero in some forms |
| FP regs | F0–F31 | 32 | 64-bit IEEE 754 doubles |
| VSX regs | VSR0–VSR63 | 64 | 128-bit; lower 32 overlap with F0–F31; upper 32 overlap with V0–V31 |
| VMX/Altivec | V0–V31 | 32 | 128-bit SIMD |
| CR | CR0–CR7 | 8 fields | Condition Register (8 × 4-bit fields = 32 bits) |
| LR | Link Register | 1 | Return address |
| CTR | Count Register | 1 | Loop counter / branch target |
| XER | Fixed-Point Exception Register | 1 | SO, OV, CA, byte count |
| FPSCR | FP Status/Control | 1 | FP exceptions and control |
| SPR | Special Purpose Registers | Many | PVR, MSR, SDR1, etc. |
| PMR | Privileged MMU Regs | Many | PTCR, etc. (POWER9+) |

### 8.2 Calling Convention — ELFv2 ABI (POWER9+ Linux)

| Register(s) | Role |
|-------------|------|
| R3–R10 | Integer/pointer arguments 1–8 and return values (R3, R3+R4) |
| F1–F13 | FP arguments 1–13 and return values (F1, F1+F2) |
| VSR0–VSR13 (V0–V13) | Vector arguments (overlap with F0–F13 for scalar FP in VSR) |
| R0, R11–R12 | Caller-saved temporaries |
| R2 | TOC pointer (Table of Contents, like GOT) |
| R13 | Thread pointer (TLS) |
| R14–R31 | Callee-saved (18 GP regs) |
| F14–F31 | Callee-saved FP (18 FP regs) |
| LR | Link register (caller-saved — must be saved before `bl`) |
| CR2–CR4 | Callee-saved condition register fields |

**Note:** ELFv1 (big-endian) is the older ABI. ELFv2 (little-endian) is used by modern POWER9+ Linux. They differ in struct return conventions and function descriptors.

**Stack alignment:** 16-byte aligned.

### 8.3 Instruction Encoding

- **Fixed-length:** 32 bits for all instructions
- **Endianness:** Bi-endian; POWER9+ Linux defaults to **little-endian** (ppc64le)
- **Alignment:** 4-byte aligned
- **4 major formats:** A-form (arithmetic), B-form (branch), D-form (imm offset), X-form (extended)
- **Condition register:** Separate CR fields that record comparison results; many instructions can set CR0 with `.` (record) suffix
- **Unusual features:** `mtctr`/`bctr` for computed branches; `rlwinm` (rotate-left-then-mask-insert) for bit manipulation

### 8.4 ELF & Binary

| Property | Value |
|----------|-------|
| ELF machine type | `EM_PPC64` = 21 (0x15) |
| ELF class | `ELFCLASS64` |
| ELF data | `ELFDATA2LSB` (ppc64le) or `ELFDATA2MSB` (ppc64be) |
| ELF flags | `EF_PPC64_ABI_V2` for ELFv2 |
| Typical base address (Linux) | `0x10000000` |
| Typical page size | 64 KB (POWER9 default) |

### 8.5 Key Instructions for Basic Codegen

| Category | Instructions |
|----------|-------------|
| Arithmetic | `ADD`, `SUBF`, `MULLD`, `DIVD`, `DIVDU`, `ADDI`, `ADDIS` |
| Logic | `AND`, `OR`, `XOR`, `NAND`, `NOR`, `ANDC`, `ORC`, `SLD`, `SRD`, `SRAD` |
| Rotate/Insert | `RLWINM`, `RLDICL`, `RLDICR`, `RLDIMI`, `ROTRDI` (bit-field operations) |
| Memory | `LD`, `STD`, `LWZ`, `STW`, `LHZ`, `STH`, `LBZ`, `STB`, `LWA`, `LHA` |
| Address | `ADDIS` + `ADDI` (or `LIS` + `ORI` for 32-bit) for immediate construction |
| Control flow | `B`, `BL`, `BCTR`, `BCTRL`, `BEQ`, `BNE`, `BLT`, `BGT`, `BC` (branch conditional) |
| Compare | `CMPD` (signed), `CMPLD` (unsigned) — set CR field |
| Conditional move | `ISEL` (conditional select, Power ISA 2.06+) |
| Move | `MR` (pseudo: `OR Rd, Rs, Rs`), `LI`, `LIS` |
| Extend | `EXTSB`, `EXTSH`, `EXTSW` |
| Atomics | `LDARX`, `STDCX.` (load-reserve, store-conditional) |
| Barriers | `SYNC`, `LWSYNC`, `ISYNC`, `EIEIO` |
| CR manipulation | `CRAND`, `CROR`, `CRXOR`, `CRNAND`, `MFcr`, `MTcrf` |
| TOC | `ADDIS R2, R12, .TOC.@ha` + `ADDI R2, R2, .TOC.@l` (function prologue) |

### 8.6 Relevance

| Domain | Relevant? |
|--------|-----------|
| Embedded | No (some NXP QorIQ) |
| Mobile | No |
| Desktop | No |
| Server | **Yes** — IBM POWER9/10/11, some HPC |
| Browser | No |

### 8.7 Complexity: **7/10**

PowerPC64 has several features that complicate codegen: (1) **Condition Register (CR)** — 8 independent 4-bit CR fields; comparisons target a specific CR field; branches test CR fields; CR logical operations are available; (2) **TOC (Table of Contents)** — function prologues must set up R2 (TOC pointer) using a 2-instruction sequence from R12; (3) **rlwinm family** — bit-field operations use a complex rotate-left-then-mask encoding with mask start/end bits; (4) **Count Register (CTR)** — used for computed calls and decrement-and-branch loops; (5) **Function descriptors** (ELFv1 only); (6) **Little-endian quirks** — some bit-numbering conventions differ from other LE ISAs. The ELFv2 ABI is significantly cleaner than ELFv1.

### 8.8 Toolchain & Cross-Compilation

| Property | Status |
|----------|--------|
| Rust target triple | `powerpc64le-unknown-linux-gnu` (tier 2), `powerpc64-unknown-linux-gnu` (tier 2) |
| Cross-compile from x86_64 Linux | **Yes** — `powerpc64le-linux-gnu-gcc`, `rustup target add powerpc64le-unknown-linux-gnu` |
| QEMU emulator | `qemu-system-ppc64` (system), `qemu-ppc64le` (user-mode) |
| GNU cross-toolchain | `powerpc64le-linux-gnu-*` available |
| LLVM support | Good |

---

## Summary Comparison Table

| Property | x86_64 | AArch64 | RISC-V64 | Wasm32/64 | ARM32 | MIPS64 | LoongArch64 | PPC64 |
|----------|--------|---------|----------|-----------|-------|--------|-------------|-------|
| **GP Regs** | 16 | 31 | 32 | 0 (stack) | 16 | 32 | 32 | 32 |
| **FP/SIMD Regs** | 16 (32 AVX-512) | 32 | 32 | 0 (stack) | 32 (D) | 32 | 32 | 64 (VSX) |
| **Arg Regs (GP)** | 6 | 8 | 8 | N/A | 4 | 4 | 8 | 8 |
| **Arg Regs (FP)** | 8 (XMM) | 8 (V) | 8 (F) | N/A | 16 (S) | 4 (F) | 8 (F) | 13 (F) |
| **Callee-saved GP** | 6 | 10 | 12 | N/A | 8 | 10 | 9 | 18 |
| **Encoding** | Variable (1–15B) | Fixed 32b | Variable 16/32b | Variable LEB128 | Variable 16/32b | Fixed 32b | Fixed 32b | Fixed 32b |
| **Endianness** | LE | LE | LE/BE | LE | LE/BE | LE/BE | LE | LE/BE |
| **ELF Machine** | 62 | 183 | 243 | N/A (.wasm) | 40 | 8 | 258 | 21 |
| **Base Address** | 0x400000 | 0x400000 | 0x10000 | 0 | 0x10000 | 0x120000000 | 0x120000000 | 0x10000000 |
| **Branch Delay** | No | No | No | No | No | **Yes** | No | No |
| **Cond Codes** | RFLAGS | NZCV | None (SLT reg) | None (i32.eq) | CPSR NZCV | None (SLT reg) | None (SLT reg) | CR0–CR7 |
| **Complexity** | 8/10 | 4/10 | 3/10 | 2/10 | 6/10 | 5/10 | 3/10 | 7/10 |
| **Rust Tier** | 1 | 1 | 2 | 2 | 2 | 2 | 2 | 2 |
| **Cross-compile** | Native | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| **QEMU** | Yes | Yes | Yes | N/A (runtime) | Yes | Yes | Yes | Yes |
| **Embedded** | — | ✓ | ✓ | Emerging | ✓ | ✓ | Emerging | — |
| **Mobile** | — | ✓ | Emerging | Emerging | Legacy | — | — | — |
| **Desktop** | ✓ | ✓ | Emerging | Emerging | — | — | ✓ (China) | — |
| **Server** | ✓ | ✓ | Emerging | Emerging | — | — | ✓ (China) | ✓ |
| **Browser** | — | — | — | ✓ | — | — | — | — |

---

## Implementation Priority Recommendation

Based on the combination of relevance, complexity, and ecosystem readiness, the recommended implementation order for VUMA's multi-arch codegen is:

### Tier 1 — Immediate (leverage existing AArch64 codegen patterns)
1. **AArch64** — Already implemented ✅
2. **RISC-V64** — Cleanest RISC ISA; fixed 32-bit encoding; 3/10 complexity; growing ecosystem
3. **WebAssembly (Wasm32)** — Simplest target; stack machine; 2/10 complexity; browser reach

### Tier 2 — Near-term (medium effort, high value)
4. **LoongArch64** — Very similar to RISC-V; 3/10 complexity; China market
5. **x86_64** — Highest effort but largest installed base; 8/10 complexity; essential for server/desktop

### Tier 3 — Medium-term (established ISAs, moderate complexity)
6. **ARM32** — Legacy but still important for embedded; 6/10 complexity
7. **MIPS64** — Branch delay slots add complexity; 5/10 complexity; embedded routers

### Tier 4 — Later (specialized, high complexity)
8. **PowerPC64** — Smallest market; highest complexity among RISC ISAs (7/10); CR manipulation; TOC overhead

---

## Retargetable Codegen Architecture Recommendations

Based on this research, VUMA's codegen should be refactored to support multiple backends with shared infrastructure:

```
IR (target-independent)
    │
    ├── Instruction Selection (per-ISA)
    │     ├── arm64/isel.rs      (existing)
    │     ├── riscv64/isel.rs    (new)
    │     ├── wasm32/isel.rs     (new)
    │     ├── x86_64/isel.rs     (new)
    │     └── ...
    │
    ├── Register Allocation (shared framework)
    │     ├── RegClass (ISA-specific pools)
    │     ├── LinearScanAllocator (parameterized by ISA)
    │     └── Spill/Reload (ISA-specific instructions)
    │
    ├── Binary Encoding (per-ISA)
    │     ├── arm64/encode.rs    (existing)
    │     ├── riscv64/encode.rs  (new)
    │     ├── wasm32/encode.rs   (new)
    │     ├── x86_64/encode.rs   (new)
    │     └── ...
    │
    └── Emission (per-ISA ELF/binary format)
          ├── arm64/emit.rs      (existing)
          ├── riscv64/emit.rs    (new)
          ├── wasm32/emit.rs     (new — .wasm format, not ELF)
          ├── x86_64/emit.rs     (new)
          └── ...
```

Key shared abstractions:
- **`TargetDef` trait**: defines register counts, argument conventions, ELF machine type, base address, encoding width
- **`InstructionSelector` trait**: maps IR nodes to target instructions
- **`RegisterAllocator` trait**: parameterized by `TargetDef` for register pool sizes
- **`Encoder` trait**: converts target instructions to bytes
- **`Emitter` trait**: produces final binary output (ELF, .wasm, raw)

---

## Quick Reference: ELF Machine Types

| ISA | EM_* Constant | Value |
|-----|--------------|-------|
| x86_64 | `EM_X86_64` | 62 |
| AArch64 | `EM_AARCH64` | 183 |
| RISC-V | `EM_RISCV` | 243 |
| ARM32 | `EM_ARM` | 40 |
| MIPS64 | `EM_MIPS` | 8 |
| LoongArch64 | `EM_LOONGARCH` | 258 |
| PowerPC64 | `EM_PPC64` | 21 |
| WebAssembly | N/A | N/A (.wasm format, not ELF) |

---

## Quick Reference: Rust Target Triples

| ISA | Linux GNU | Bare-Metal | Tier |
|-----|-----------|-----------|------|
| x86_64 | `x86_64-unknown-linux-gnu` | N/A | 1 |
| AArch64 | `aarch64-unknown-linux-gnu` | `aarch64-unknown-none` | 1 / 2 |
| RISC-V64 | `riscv64gc-unknown-linux-gnu` | `riscv64imac-unknown-none-elf` | 2 |
| Wasm32 | `wasm32-wasip1` | `wasm32-unknown-unknown` | 2 |
| ARM32 | `armv7-unknown-linux-gnueabihf` | `thumbv7em-none-eabihf` | 2 |
| MIPS64 | `mips64el-unknown-linux-gnuabi64` | N/A | 2 |
| LoongArch64 | `loongarch64-unknown-linux-gnu` | N/A | 2 |
| PPC64 | `powerpc64le-unknown-linux-gnu` | N/A | 2 |
