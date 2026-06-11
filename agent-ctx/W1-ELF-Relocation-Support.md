# Task W1: ELF Relocation Support for 6 ISAs

## Agent
ELF Relocation Support

## Status
✅ Complete

## Summary
Added ELF relocation support for 6 ISAs (x86_64, RISC-V64, MIPS64, PPC64, LoongArch64, ARM32) to the VUMA codegen emit module. AArch64 relocations already existed.

## Changes Made to `/home/z/my-project/vuma/src/codegen/src/emit.rs`

### 1. EM_* Machine Type Constants (lines 89-105)
- `EM_X86_64: u16 = 62`
- `EM_RISCV: u16 = 243`
- `EM_MIPS: u16 = 8`
- `EM_PPC64: u16 = 21`
- `EM_LOONGARCH: u16 = 258`
- `EM_ARM: u16 = 40`

### 2. Relocation Type Constants (lines 174-304)
- x86_64: 5 constants (R_X86_64_64, R_X86_64_PC32, R_X86_64_PLT32, R_X86_64_32, R_X86_64_32S)
- RISC-V64: 8 constants (R_RISCV_CALL, R_RISCV_CALL_PLT, R_RISCV_PCREL_HI20, R_RISCV_PCREL_LO12_I, R_RISCV_HI20, R_RISCV_LO12_I, R_RISCV_JAL, R_RISCV_BRANCH)
- MIPS64: 7 constants (R_MIPS_26, R_MIPS_32, R_MIPS_64, R_MIPS_HI16, R_MIPS_LO16, R_MIPS_CALL16, R_MIPS_GPREL16)
- PowerPC64: 4 constants (R_PPC64_ADDR64, R_PPC64_ADDR32, R_PPC64_REL24, R_PPC64_REL32)
- LoongArch64: 6 constants (R_LARCH_64, R_LARCH_32, R_LARCH_B26, R_LARCH_PCALA_HI20, R_LARCH_PCALA_LO12, R_LARCH_CALL36)
- ARM32: 6 constants (R_ARM_CALL, R_ARM_JUMP24, R_ARM_MOVW_ABS_NC, R_ARM_MOVT_ABS, R_ARM_REL32, R_ARM_ABS32)

### 3. EmitConfig.backend Field
- Added `backend: BackendKind` field to `EmitConfig` struct
- Defaults to `BackendKind::AArch64` in all existing constructors
- Added `EmitConfig::relocatable_obj_for(backend: BackendKind)` constructor

### 4. ISA-Aware Helper Functions (lines 1191-1225)
- `em_machine_for_backend(backend: BackendKind) -> u16`
- `call_reloc_type_for_backend(backend: BackendKind) -> u32`

### 5. emit_elf() Modifications
- Line 1370: `e_machine` from `em_machine_for_backend(config.backend)` instead of hardcoded `EM_AARCH64`
- Line 1313: `call_reloc_type` from `call_reloc_type_for_backend(config.backend)` instead of hardcoded `R_AARCH64_CALL26`

### 6. New emit_obj() Function (lines 1541-1549)
- `pub fn emit_obj(functions, data_sections, backend) -> Result<Vec<u8>>`
- Wraps `emit_elf()` with `OutputFormat::Obj` and specified backend

### 7. New Tests (16 tests added)
Per ISA (6 ISAs × 2-3 tests = 16 tests):
- Constant value verification test
- Machine type in ELF header test
- Relocation type in .rela.text test
Plus 2 cross-ISA helper mapping tests.

## Verification
- `cargo clippy -p vuma-codegen -- -D warnings`: 0 warnings
- `cargo test -p vuma-codegen -- -q`: 695 passed, 0 failed
