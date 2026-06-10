# VUMA Multi-Architecture Plan — 32 Waves

> **Goal**: Make VUMA a true multi-target compiler supporting 8 ISAs natively. Every ISA gets a first-class backend with correct instruction encoding, register allocation, calling convention, ELF emission, and test suite. Adding a new ISA should be a well-defined process, not a hack.

> **The 8 Targets** (ordered by implementation complexity, easiest first):
>
> | # | ISA | Complexity | ELF Machine | Registers | Calling Conv | Status |
> |---|-----|-----------|-------------|-----------|--------------|--------|
> | 1 | AArch64 | 4/10 | EM_AARCH64=183 | 31 GP, 32 FP | AAPCS64 | ✅ Done |
> | 2 | RISC-V64 | 3/10 | EM_RISCV=243 | 32 GP, 32 FP | LP64D | 🎯 Next |
> | 3 | Wasm32 | 2/10 | N/A (binary format) | Stack machine | Stack-based | 🎯 Next |
> | 4 | LoongArch64 | 3/10 | EM_LOONGARCH=258 | 32 GP, 32 FP | LP64 | 🎯 Next |
> | 5 | x86_64 | 8/10 | EM_X86_64=62 | 16 GP, 16 XMM | SystemV | 🎯 Next |
> | 6 | ARM32 | 6/10 | EM_ARM=40 | 16 GP, 32 FP | AAPCS | 🎯 Next |
> | 7 | MIPS64 | 5/10 | EM_MIPS=8 | 32 GP, 32 FP | N64 | 🎯 Next |
> | 8 | PowerPC64 | 7/10 | EM_PPC64=21 | 32 GP, 64 VSX | ELFv2 | 🎯 Next |

> **Constraints**:
> - Host: x86_64 Intel Xeon, Linux — ALL backends must compile and test here
> - ARM64 backend already exists — must NOT regress
> - x86_64 backend must produce EXECUTABLE code (we can run it natively)
> - Other backends produce ELF/binary that can be validated structurally + via QEMU
> - Max 32 subagents per wave
> - Zero tolerance for stubs, TODOs, or "looks about right" encodings
> - If a subagent produces code that doesn't compile, it is REJECTED and must be redone

---

## Architecture: The Multi-Backend System

```
                        ┌─────────────┐
                        │ VUMA Source │
                        └──────┬──────┘
                               │
                        ┌──────▼──────┐
                        │    Parser   │
                        └──────┬──────┘
                               │
                        ┌──────▼──────┐
                        │     SCG     │  (target-independent)
                        └──────┬──────┘
                               │
                        ┌──────▼──────┐
                        │  BD Infer   │  (target-independent)
                        └──────┬──────┘
                               │
                        ┌──────▼──────┐
                        │  IVE Verify │  (target-independent)
                        └──────┬──────┘
                               │
                        ┌──────▼──────┐
                        │     IR      │  (target-parameterized via TargetInfo)
                        └──────┬──────┘
                               │
                  ┌────────────┼────────────┐
                  │     Backend trait       │
                  └────────────┼────────────┘
                               │
        ┌──────────┬───────────┼───────────┬──────────┬──────────┬──────────┬──────────┬──────────┐
        │  AArch64 │  RISC-V64 │  Wasm32   │ LoongArch│  x86_64  │  ARM32   │  MIPS64  │  PPC64   │
        │  Backend │  Backend  │  Backend  │   64     │  Backend │  Backend │  Backend │  Backend │
        └────┬─────┴─────┬─────┴─────┬─────┴────┬─────┴────┬─────┴────┬─────┴────┬─────┴────┬─────┘
             │           │           │          │          │          │          │          │
        ┌────▼────┐┌─────▼────┐┌────▼────┐┌────▼────┐┌────▼────┐┌────▼────┐┌────▼────┐┌────▼────┐
        │ARM64 ELF││RV64 ELF  ││.wasm    ││LA64 ELF ││x86 ELF  ││ARM ELF  ││MIPS ELF ││PPC ELF  │
        └─────────┘└──────────┘└─────────┘└─────────┘└─────────┘└─────────┘└─────────┘└─────────┘
```

**Key Design Principles**:
1. `TargetInfo` trait — describes an ISA's properties (register counts, type sizes, calling conv, ELF type)
2. `Backend` trait — code generation interface (regalloc, encode, disassemble, emit)
3. `ControlFlow` module — target-agnostic lowering (switch, exception, tailcall, coroutine, loop)
4. `TargetDesc` — machine-readable target description that can generate test scaffolding
5. Each backend lives in its own file: `arm64.rs`, `riscv64.rs`, `wasm32.rs`, etc.
6. `BackendKind` enum with 8 variants + `create_backend()` factory
7. Cross-compilation: any host → any target
8. Native execution: x86_64 host can run x86_64 output directly; QEMU for others

---

## Dependency DAG — Maximum Parallelism

```
Time ──►

T0:  [W1]
T1:  [W2]
T2:  [W3]
T3:  [W4] [W5]
T4:  [W6-RV64] [W7-Wasm] [W8-LA64] [W9-x86] [W23]            ← 5 parallel waves
T5:  [W10-RV64] [W11-Wasm] [W12-LA64] [W13-x86] [W24]         ← 5 parallel waves
T6:  [W14-RV64] [W15-x86-exec] [W16-ARM32] [W25]               ← 4 parallel waves
T7:  [W17-MIPS64] [W18-PPC64] [W26]                             ← 3 parallel waves
T8:  [W19-multi-integration] [W27]                               ← 2 parallel waves
T9:  [W20-QEMU] [W28]                                           ← 2 parallel waves
T10: [W21- audit] [W29]                                         ← 2 parallel waves
T11: [W22-release] [W30]                                        ← 2 parallel waves
T12: [W31]
T13: [W32]
```

**Parallel group summary:**

| Group | Time Slot | Waves (parallel) | Total Subagents |
|-------|-----------|-------------------|-----------------|
| G0  | T0  | W1 | 8 |
| G1  | T1  | W2 | 12 |
| G2  | T2  | W3 | 16 |
| G3  | T3  | W4, W5 | 16 |
| G4  | T4  | W6, W7, W8, W9, W23 | 32 |
| G5  | T5  | W10, W11, W12, W13, W24 | 32 |
| G6  | T6  | W14, W15, W16, W25 | 28 |
| G7  | T7  | W17, W18, W26 | 24 |
| G8  | T8  | W19, W27 | 20 |
| G9  | T9  | W20, W28 | 20 |
| G10 | T10 | W21, W29 | 24 |
| G11 | T11 | W22, W30 | 24 |
| G12 | T12 | W31 | 16 |
| G13 | T13 | W32 | 12 |

**Peak parallelism: 5 waves simultaneously, 32 subagents.**

---

## Wave 1: Build Recovery — Fix All Compilation Errors

**Dependencies**: None (first wave)
**Estimated subagents**: 8

### Subagent Tasks:

**W1-S1: Fix vuma-ive hashbrown dependency**
```
You are fixing a COMPILATION BLOCKER in the VUMA project at /home/z/my-project/vuma.
The crate vuma-ive (src/ive/) uses hashbrown::HashMap and hashbrown::HashSet in 
5 files but the dependency is NOT declared in its Cargo.toml. This causes 33 errors.

YOUR TASK:
1. Add `hashbrown = { version = "0.14", features = ["serde"] }` to src/ive/Cargo.toml [dependencies]
2. Verify version matches the one used in vuma-bd, vuma-scg, and root Cargo.toml
3. Run: export PATH="/home/z/.cargo/bin:$PATH" && cd /home/z/my-project/vuma && cargo check -p vuma-ive
4. If ANY errors remain, FIX THEM. Do not stop until `cargo check -p vuma-ive` passes with ZERO errors.
5. Do NOT modify any .rs files — the code is correct, only the Cargo.toml is wrong.

HARSH RULES:
- If cargo check still fails after your changes, you have FAILED.
- If you modify any .rs file, you have FAILED.
- If you add the wrong version of hashbrown, you have FAILED.
- You MUST verify with actual cargo check, not just visual inspection.
```

**W1-S2: Fix root Cargo.toml invalid key**
```
The root Cargo.toml at /home/z/my-project/vuma/Cargo.toml has an invalid manifest key:
`profile.release.target-cpu` is NOT a valid Cargo.toml key.

YOUR TASK:
1. REMOVE `target-cpu = "native"` from [profile.release]
2. Create/edit /home/z/my-project/vuma/.cargo/config.toml:
   ```toml
   [build]
   rustflags = ["-C", "target-cpu=native"]
   
   [target.aarch64-unknown-linux-gnu]
   rustflags = []
   
   [target.aarch64-unknown-none]
   rustflags = []
   ```
3. Verify the warning is gone with cargo check.

HARSH RULES:
- If the warning persists, you have FAILED.
- If you break any existing build, you have FAILED.
```

**W1-S3 through W1-S8: Fix compilation errors in each crate**

```
You are assigned to crate CRATE_NAME in the VUMA project at /home/z/my-project/vuma.

YOUR TASK:
1. Run: export PATH="/home/z/.cargo/bin:$PATH" && cd /home/z/my-project/vuma && cargo check -p CRATE_NAME 2>&1
2. If there are ZERO errors, report "CRATE_NAME: CLEAN" and stop.
3. If there are errors, FIX EVERY SINGLE ONE. Common patterns:
   - Missing dependencies in Cargo.toml → add them
   - Unresolved imports → fix the import path or add the dep
   - Type mismatches → fix the code
   - Missing trait implementations → implement them
4. After fixing, run cargo check -p CRATE_NAME again. REPEAT until zero errors.
5. Do NOT add #[allow(...)] to suppress errors. Fix the ROOT CAUSE.

CRATE ASSIGNMENTS:
- W1-S3: vuma-bd
- W1-S4: vuma-scg
- W1-S5: vuma-parser
- W1-S6: vuma-codegen
- W1-S7: vuma-cor
- W1-S8: vuma-std, vuma-proof, vuma-projection, vuma-pi5, vuma-tests, vuma-core

HARSH RULES:
- If ANY error remains after your changes, you have FAILED.
- If you suppress errors with #[allow] instead of fixing them, you have FAILED.
- If you delete working code instead of fixing it, you have FAILED.
- You MUST run cargo check and verify ZERO errors before reporting completion.
```

### Success Criteria:
- `cargo check --workspace` passes with ZERO errors

---

## Wave 2: Warning Elimination & Dead Code Cleanup

**Dependencies**: W1
**Estimated subagents**: 12

### Subagent Tasks:

**W2-S1 through W2-S7: Fix warnings per crate**

```
You are assigned to clean ALL warnings in CRATE_NAME at /home/z/my-project/vuma.

YOUR TASK:
1. Run: export PATH="/home/z/.cargo/bin:$PATH" && cd /home/z/my-project/vuma && cargo check -p CRATE_NAME 2>&1 | grep "warning"
2. For EACH warning, determine the correct fix:
   - unused_imports → REMOVE the unused import
   - unused_variables → either use the variable or prefix with _
   - dead_code → if truly unused, REMOVE it. If part of a public API, add #[allow(dead_code)] with justification comment.
   - unused_mut → remove the mut if not needed
   - static_mut_refs (Rust 2024) → replace with &raw mut or refactor to UnsafeCell
3. After fixing, run cargo check -p CRATE_NAME again. REPEAT until zero warnings.
4. Run the FULL test suite: cargo test -p CRATE_NAME
   If any test breaks, your fix was WRONG. Revert and fix properly.

CRATE ASSIGNMENTS:
- W2-S1: vuma-ive (11 warnings)
- W2-S2: vuma-scg (2 warnings)
- W2-S3: vuma-codegen (5 warnings)
- W2-S4: vuma-parser (1 warning)
- W2-S5: vuma-pi5 (6 warnings — Rust 2024 static_mut_refs)
- W2-S6: vuma-proof (3 warnings)
- W2-S7: vuma-std (11 warnings)

HARSH RULES:
- If ANY warning remains, you have FAILED.
- If you break any existing test, you have FAILED.
- If you add #[allow(dead_code)] without a justification comment, you have FAILED.
- You are NOT allowed to add #[allow(warnings)] at the crate level.
- Every fix must be surgical — do not refactor unrelated code.
```

**W2-S8 through W2-S12: Deep dead code analysis**

```
You are performing a DEAD CODE AUDIT on CRATE_NAME.

CRATE ASSIGNMENTS:
- W2-S8: vuma-core
- W2-S9: vuma-codegen
- W2-S10: vuma-std
- W2-S11: vuma-projection
- W2-S12: vuma-tests

HARSH RULES:
- If you delete code that IS used somewhere else, you have FAILED CATASTROPHICALLY.
- If you leave truly dead code without marking it, you have FAILED.
- You MUST verify cargo check + cargo test still pass.
```

### Success Criteria:
- `cargo check --workspace 2>&1 | grep "warning" | wc -l` returns 0
- `cargo test --workspace` passes

---

## Wave 3: Multi-Backend Trait Architecture

**Dependencies**: W2
**Estimated subagents**: 16

This is the MOST CRITICAL wave — defining the abstraction that scales to 8+ ISAs.

### Subagent Tasks:

**W3-S1: Define the Backend trait for 8+ targets**

```
You are designing the core multi-architecture abstraction for VUMA's codegen.

The trait must support ALL of these ISAs without any ISA-specific assumptions:
- AArch64: 31 GP regs, 32 FP regs, link register, AAPCS64, fixed 32-bit encoding
- RISC-V64: 32 GP regs (x0=zero), 32 FP regs, link register, LP64D, variable 16/32-bit encoding
- Wasm32: STACK MACHINE (no registers!), stack-based calling, LEB128 encoding, binary format (not ELF)
- LoongArch64: 32 GP regs (r0=zero), 32 FP regs, link register, LP64, fixed 32-bit encoding
- x86_64: 16 GP regs, 16 XMM regs, NO link register (push return addr), SystemV, variable-length encoding
- ARM32: 16 GP regs (PC=R15), 32 FP regs, link register, AAPCS, variable 16/32-bit encoding
- MIPS64: 32 GP regs (r0=zero), 32 FP regs, link register, N64 ABI, fixed 32-bit, BRANCH DELAY SLOTS
- PowerPC64: 32 GP regs, 64 VSX regs, link register, ELFv2, fixed 32-bit, TOC pointer

YOUR TASK: Create /home/z/my-project/vuma/src/codegen/src/backend.rs with:

```rust
/// Target-specific information needed during code generation.
/// This trait describes WHAT a target looks like, not HOW to generate code for it.
pub trait TargetInfo: Send + Sync + 'static {
    // === Identity ===
    fn isa_name(&self) -> &'static str;           // "aarch64", "riscv64", "wasm32", etc.
    fn target_triple(&self) -> &'static str;       // "aarch64-unknown-linux-gnu"
    fn elf_machine_type(&self) -> u16;             // EM_AARCH64=183, or 0 for non-ELF (Wasm)
    fn default_base_address(&self) -> u64;         // 0x400000 for ARM64/x86_64, 0x10000 for RV64

    // === Data model ===
    fn pointer_width(&self) -> usize;              // 4 or 8 bytes
    fn size_of(&self, ty: &IRType) -> usize;
    fn alignment_of(&self, ty: &IRType) -> usize;
    fn endianness(&self) -> Endianness;            // Little, Big, or Bi (PPC64)

    // === Register architecture ===
    fn has_registers(&self) -> bool;               // false for Wasm (stack machine)
    fn num_gp_regs(&self) -> usize;                // 0 for Wasm
    fn num_simd_fp_regs(&self) -> usize;           // 0 for Wasm
    fn has_hardwired_zero(&self) -> bool;          // true for RISC-V, LoongArch
    fn has_link_register(&self) -> bool;           // true for ARM/RISC-V/MIPS/PPC, false for x86_64
    fn has_branch_delay_slots(&self) -> bool;      // true for MIPS
    fn has_toc_pointer(&self) -> bool;             // true for PPC64
    fn has_condition_registers(&self) -> bool;      // true for PPC64 (8 CR fields)

    // === Calling convention ===
    fn calling_convention_name(&self) -> &'static str;  // "aapcs64", "lp64d", "stack", "systemv"
    fn num_int_arg_regs(&self) -> usize;           // 8 for ARM64, 6 for x86_64, 8 for RV64, 0 for Wasm
    fn num_fp_arg_regs(&self) -> usize;            // 8 for ARM64, 8 for x86_64, 8 for RV64, 0 for Wasm
    fn stack_alignment(&self) -> usize;            // 16 for most, 8 for MIPS

    // === Instruction encoding ===
    fn instruction_alignment(&self) -> usize;       // 4 for fixed-width RISCs, 1 for x86_64/Wasm
    fn instruction_width_range(&self) -> (usize, usize);  // (min, max) bytes: (4,4) for ARM64, (1,15) for x86_64

    // === Output format ===
    fn output_format(&self) -> OutputFormat;       // Elf64, Elf32, WasmBinary, RawBinary
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Endianness { Little, Big, Bi }

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat { Elf64, Elf32, WasmBinary, RawBinary }

/// A code generation backend. Implement this for each target architecture.
pub trait Backend: Send + Sync + 'static {
    fn target_info(&self) -> &dyn TargetInfo;
    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError>;
    fn encode_function(&self, func: &AllocatedFunction) -> Result<Vec<u8>, BackendError>;
    fn encode_program(&self, program: &AllocatedProgram) -> Result<Vec<u8>, BackendError>;
    fn return_stub(&self) -> Vec<u8>;
    fn trampoline(&self, entry_addr: u64) -> Vec<u8>;
    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String>;
    fn name(&self) -> &'static str;
}

/// The 8 supported backends.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BackendKind {
    AArch64,
    RiscV64,
    Wasm32,
    LoongArch64,
    X86_64,
    Arm32,
    Mips64,
    PowerPC64,
}

/// Factory: create a backend by kind.
pub fn create_backend(kind: BackendKind) -> Box<dyn Backend> {
    match kind {
        BackendKind::AArch64 => Box::new(Arm64Backend::new()),
        BackendKind::RiscV64 => Box::new(RiscV64Backend::new()),
        BackendKind::Wasm32 => Box::new(Wasm32Backend::new()),
        BackendKind::LoongArch64 => Box::new(LoongArch64Backend::new()),
        BackendKind::X86_64 => Box::new(X86_64Backend::new()),
        BackendKind::Arm32 => Box::new(Arm32Backend::new()),
        BackendKind::Mips64 => Box::new(Mips64Backend::new()),
        BackendKind::PowerPC64 => Box::new(PowerPC64Backend::new()),
    }
}

// ... AllocatedFunction, AllocatedBlock, PhysicalReg, BackendError, etc.
```

DESIGN REQUIREMENTS:
- The trait must handle Wasm's stack machine (has_registers() = false)
- The trait must handle MIPS delay slots (has_branch_delay_slots())
- The trait must handle PPC64 TOC pointer and condition registers
- The trait must handle x86_64's variable-length encoding
- The trait must be object-safe
- OutputFormat must support non-ELF targets (Wasm, raw binary for bare-metal)
- Backend::encode_program handles the full output: ELF for register machines, .wasm for Wasm

HARSH RULES:
- If the trait has ANY ISA-specific concept that doesn't generalize, you have FAILED.
- If Wasm can't implement this trait, you have FAILED.
- If the trait is not object-safe, you have FAILED.
- You MUST write 8 mock TargetInfo implementations (one per ISA) to prove the trait works.
- You MUST write unit tests verifying each mock returns correct values.
- cargo check -p vuma-codegen must pass.
```

**W3-S2: Make IR types multi-target**

```
You are refactoring /home/z/my-project/vuma/src/codegen/src/ir.rs to support 8 ISAs.

CURRENT PROBLEMS:
- size_of/alignment_of hardcode ARM64 LP64
- compute_calling_conv implements AAPCS64 only
- compute_stack_layout assumes ARM64 conventions

YOUR TASK:
1. All functions that hardcode target assumptions get a &dyn TargetInfo parameter.
2. Import TargetInfo from backend module.
3. Provide Arm64Target implementing TargetInfo with current hardcoded values.
4. Update ALL call sites in the entire codegen crate.
5. Ensure cargo check -p vuma-codegen passes.
6. Ensure cargo test -p vuma-codegen passes.

HARSH RULES:
- If you break any existing test, you have FAILED.
- If you leave any hardcoded ARM64 assumptions, you have FAILED.
- If cargo check or cargo test fails, you have FAILED.
```

**W3-S3 through W3-S8: Refactor existing codegen modules**

```
You are updating MODULE in /home/z/my-project/vuma/src/codegen/src/ to work with the new multi-backend trait.

MODULE ASSIGNMENTS:
- W3-S3: arm64.rs — Implement TargetInfo + Backend for AArch64 (wrap existing code)
- W3-S4: emit.rs — Refactor to use TargetInfo for ELF headers, base addresses
- W3-S5: regalloc.rs — Make register pool configurable via TargetInfo
- W3-S6: scg_to_ir.rs — Pass TargetInfo through for target-dependent decisions
- W3-S7: lib.rs — Add mod backend, re-export, BackendKind enum with 8 variants, create_backend factory
- W3-S8: Comprehensive tests for Backend trait dispatch with all 8 mock TargetInfo impls

HARSH RULES FOR ALL:
- ARM64 codegen output must be BIT-FOR-BIT IDENTICAL after refactoring.
- If any existing test breaks, you have FAILED.
- If cargo check -p vuma-codegen fails, you have FAILED.
```

**W3-S9 through W3-S16: Update all dependent crates**

```
You are updating CRATE_NAME to pass TargetInfo/Backend through the pipeline.

CRATE ASSIGNMENTS:
- W3-S9:  vuma-core — Pipeline uses Backend trait, selects target via BackendKind
- W3-S10: vuma-cor — Runtime uses Backend trait for code generation + execution
- W3-S11: vuma-cor — compile_region uses Backend::encode_function
- W3-S12: vuma-tests — All codegen tests use Backend trait
- W3-S13: vuma-ive — Check if IVE needs TargetInfo
- W3-S14: vuma-bd — Check if BD needs any target params
- W3-S15: vuma-scg — Check if SCG needs any target params
- W3-S16: End-to-end ARM64 integration test via Backend trait

HARSH RULES:
- If you leave any ARM64 hardcoded assumption, you have FAILED.
- The ARM64 codegen output must be BIT-FOR-BIT identical after refactoring.
```

### Success Criteria:
- Backend trait handles all 8 ISAs (proven by 8 mock TargetInfo implementations)
- `cargo check --workspace` passes
- `cargo test --workspace` passes
- ARM64 output is unchanged

---

## Wave 4: ARM64 Regression + Target-Agnostic Control Flow

**Dependencies**: W3
**Estimated subagents**: 8

### Subagent Tasks:

**W4-S1 through W4-S4: ARM64 regression validation**

```
You are validating that the ARM64 backend produces IDENTICAL output after the multi-backend refactor.

SUBAGENT ASSIGNMENTS:
- W4-S1: ARM64 encoding round-trip tests (every instruction → encode → decode → match)
- W4-S2: ARM64 calling convention tests via Backend trait
- W4-S3: ARM64 ELF emission tests via Backend trait
- W4-S4: Full ARM64 pipeline test (source → parse → SCG → BD → IVE → IR → Backend → ELF)

HARSH RULES:
- If any ARM64 encoding changes, you have FAILED.
- If any test fails, FIX IT before reporting.
```

**W4-S5 through W4-S8: Extract target-agnostic control flow module**

```
You are creating /home/z/my-project/vuma/src/codegen/src/control_flow.rs — a TARGET-AGNOSTIC
control flow module that works for ALL 8 ISAs.

COMPONENTS:
1. SwitchLowerer — jump_table/binary_search/if_else_chain, parameterized by TargetInfo
2. ExceptionLowerer — invoke/landing-pad, parameterized by TargetInfo
3. TailCallLowerer — eligibility + arg shuffle, parameterized by TargetInfo
4. CoroutineLowerer — state machine + frame layout, parameterized by TargetInfo
5. LoopOptimizer — unrolling + cost model, parameterized by TargetInfo

KEY CONSIDERATIONS for multi-ISA:
- MIPS has branch delay slots: the instruction AFTER a branch ALWAYS executes.
  SwitchLowerer must insert NOPs or useful instructions in delay slots.
- Wasm has br_table instead of jump tables — different encoding, same concept.
- x86_64 jump tables use 32-bit relative offsets (PC-relative).
- ARM64 uses ADRP+ADD for 64-bit absolute addresses in jump tables.
- PPC64 uses TOC-relative addressing for jump tables.

SUBAGENT ASSIGNMENTS:
- W4-S5: SwitchLowerer with multi-ISA support (delay slots, br_table, PC-relative)
- W4-S6: ExceptionLowerer + TailCallLowerer
- W4-S7: CoroutineLowerer + LoopOptimizer
- W4-S8: Integration tests with all 8 mock TargetInfo implementations

HARSH RULES:
- If any function references a specific ISA by name, you have FAILED.
- If MIPS delay slots are not handled, you have FAILED.
- If Wasm br_table is not handled, you have FAILED.
- Each component must have 10+ tests.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- ARM64 backend produces identical output
- Control flow module is fully target-agnostic
- Tests pass for all 8 ISA mocks

---

## Wave 5: Target Description System

**Dependencies**: W3
**Estimated subagents**: 8

### Subagent Tasks:

**W5-S1: Create TargetDesc — machine-readable ISA specification**

```
You are creating /home/z/my-project/vuma/src/codegen/src/target_desc.rs — a machine-readable
target description system that makes adding new ISAs a DATA-DRIVEN process.

The idea: instead of writing thousands of lines of Rust for each backend, describe the ISA
in a structured format and generate boilerplate automatically.

```rust
/// Machine-readable description of an ISA.
pub struct TargetDesc {
    pub name: &'static str,
    pub triple: &'static str,
    pub elf_machine: u16,
    pub base_addr: u64,
    pub pointer_width: usize,
    pub endianness: Endianness,
    pub output_format: OutputFormat,

    // Register descriptions
    pub registers: Vec<RegDesc>,
    pub calling_convention: CallingConventionDesc,
    pub instruction_set: Vec<InstDesc>,
}

pub struct RegDesc {
    pub name: &'static str,
    pub class: RegClass,
    pub index: usize,
    pub is_allocatable: bool,
    pub is_hardwired_zero: bool,
    pub is_stack_pointer: bool,
    pub is_frame_pointer: bool,
    pub is_link_register: bool,
    pub is_toc_pointer: bool,
    pub is_callee_saved: bool,
    pub is_arg_reg: bool,
    pub arg_position: Option<usize>,
    pub is_return_reg: bool,
}

pub struct CallingConventionDesc {
    pub name: &'static str,
    pub int_arg_regs: Vec<usize>,    // indices into registers[]
    pub fp_arg_regs: Vec<usize>,
    pub int_return_regs: Vec<usize>,
    pub fp_return_regs: Vec<usize>,
    pub callee_saved: Vec<usize>,
    pub stack_alignment: usize,
    pub has_link_register: bool,
    pub has_branch_delay_slots: bool,
    pub has_toc_pointer: bool,
}

pub struct InstDesc {
    pub mnemonic: &'static str,
    pub operands: Vec<OperandDesc>,
    pub encoding: EncodingDesc,
    pub semantic: InstSemantic,
}
```

Also create a `TargetDescRegistry` that holds all 8 ISA descriptions and can be queried.

HARSH RULES:
- Every field in TargetDesc must be populated for all 8 ISAs.
- The registry must be testable.
- This is NOT a replacement for the Backend trait — it's supplementary metadata.
- cargo check -p vuma-codegen must pass.
```

**W5-S2 through W5-S8: Define TargetDesc for all 8 ISAs**

```
You are defining the TargetDesc for ISA_NAME.

SUBAGENT ASSIGNMENTS:
- W5-S2: AArch64 TargetDesc
- W5-S3: RISC-V64 TargetDesc
- W5-S4: Wasm32 TargetDesc
- W5-S5: LoongArch64 TargetDesc
- W5-S6: x86_64 TargetDesc
- W5-S7: ARM32 TargetDesc
- W5-S8: MIPS64 + PowerPC64 TargetDesc

YOUR TASK:
1. Define ALL registers with their properties (allocatable, callee-saved, arg, return, special).
2. Define the calling convention in full detail.
3. Define instruction categories (not every instruction — just the categories needed for codegen).
4. Verify against the ISA's official specification.
5. Write tests that validate TargetDesc consistency (e.g., no register is both arg and callee-saved).

HARSH RULES:
- If any register property is wrong, you have FAILED.
- If the calling convention doesn't match the ABI specification, you have FAILED.
- You MUST cross-reference with official ISA/ABI documentation.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- TargetDesc system works for all 8 ISAs
- All descriptions validated against official specs
- `cargo test -p vuma-codegen` passes

---

## Wave 6: RISC-V64 Backend (Easiest New Target)

**Dependencies**: W4, W5
**Estimated subagents**: 12

RISC-V64 is the cleanest RISC ISA (3/10 complexity). Same family as ARM64 (fixed 32-bit encoding, link register, clean register file) but with hardwired zero register and compressed instructions (RVC).

### Subagent Tasks:

**W6-S1: Create riscv64.rs — register definitions + instruction types**

```
You are creating /home/z/my-project/vuma/src/codegen/src/riscv64.rs — the RISC-V64 backend.

YOUR TASK: Define the complete RISC-V64 register and instruction model:

Registers:
- x0 (zero, hardwired to 0), x1 (ra, return address), x2 (sp), x3 (gp), x4 (tp),
  x5-x7 (t0-t2, temporaries), x8 (s0/fp), x9 (s1), x10-x17 (a0-a7, arguments),
  x18-x27 (s2-s11, callee-saved), x28-x31 (t3-t6, temporaries)
- f0-f31 (fa0-fa7 for args, ft0-ft11 for temps, fs0-fs11 callee-saved)

Instruction formats (ALL 6):
- R-type: funct7[31:25] rs2[24:20] rs1[19:15] funct3[14:12] rd[11:7] opcode[6:0]
- I-type: imm[31:20] rs1[19:15] funct3[14:12] rd[11:7] opcode[6:0]
- S-type: imm[31:25] rs2[24:20] rs1[19:15] funct3[14:12] imm[11:7] opcode[6:0]
- B-type: imm[12|10:5] rs2[24:20] rs1[19:15] funct3[14:12] imm[4:1|11] opcode[6:0]
- U-type: imm[31:12] rd[11:7] opcode[6:0]
- J-type: imm[20|10:1|11|19:12] rd[11:7] opcode[6:0]

Required instructions (RV64IMAFD = RV64GC):
- RV64I: LUI, AUIPC, JAL, JALR, BEQ, BNE, BLT, BGE, BLTU, BGEU,
  LB, LH, LW, LD, LBU, LHU, LWU, SB, SH, SW, SD,
  ADDI, SLTI, SLTIU, XORI, ORI, ANDI, SLLI, SRLI, SRAI,
  ADD, SUB, SLL, SLT, SLTU, XOR, SRL, SRA, OR, AND,
  ADDIW, SLLIW, SRLIW, SRAIW, ADDW, SUBW, SLLW, SRLW, SRAW,
  FENCE, FENCE.I, ECALL, EBREAK
- M extension: MUL, MULH, MULHSU, MULHU, DIV, DIVU, REM, REMU,
  MULW, DIVW, REMW, DIVUW, REMUW
- F/D extensions: FLW, FLD, FSW, FSD, FMADD.S, FMSUB.S, FNMSUB.S, FNMADD.S,
  FMADD.D, FMSUB.D, FNMSUB.D, FNMADD.D,
  FADD.S, FSUB.S, FMUL.S, FDIV.S, FSQRT.S,
  FADD.D, FSUB.D, FMUL.D, FDIV.D, FSQRT.D,
  FSGNJ.S, FSGNJN.S, FSGNJX.S, FMIN.S, FMAX.S,
  FSGNJ.D, FSGNJN.D, FSGNJX.D, FMIN.D, FMAX.D,
  FCVT.W.S, FCVT.WU.S, FCVT.L.S, FCVT.LU.S, FCVT.S.W, FCVT.S.WU, FCVT.S.L, FCVT.S.LU,
  FCVT.W.D, FCVT.WU.D, FCVT.L.D, FCVT.LU.D, FCVT.D.W, FCVT.D.WU, FCVT.D.L, FCVT.D.LU,
  FEQ.S, FLT.S, FLE.S, FEQ.D, FLT.D, FLE.D,
  FCLASS.S, FCLASS.D
- RVC (compressed): C.ADDI4SPN, C.LW, C.LD, C.SW, C.SD, C.ADDI, C.ADDIW,
  C.LI, C.LUI, C.SRLI, C.SRAI, C.ANDI, C.SUB, C.XOR, C.OR, C.AND,
  C.SUBW, C.ADDW, C.J, C.BEQZ, C.BNEZ, C.LI, C.LUI, C.SLLI,
  C.LDSP, C.LWSP, C.LD, C.JR, C.MV, C.EBREAK, C.JALR, C.ADD,
  C.SWSP, C.SDSP

HARSH RULES:
- If you miss ANY instruction from RV64GC, you have FAILED.
- Every encoding MUST be correct per the RISC-V ISA Specification (Volume 1, 20191213).
- No TODOs. No stubs. Every instruction must have a working encode() and decode().
- cargo check -p vuma-codegen must pass.
```

**W6-S2 through W6-S7: RISC-V64 encoding implementation**

```
You are implementing the encode/decode for RISC-V64 instructions.

SUBAGENT ASSIGNMENTS:
- W6-S2: RV64I base integer instructions (R-type + I-type: arithmetic, logical, compare)
- W6-S3: RV64I load/store + branch + jump instructions (S-type, B-type, U-type, J-type)
- W6-S4: M extension (multiply/divide) + Zicsr/Zifencei
- W6-S5: F/D extensions (floating-point, all formats)
- W6-S6: RVC compressed instructions (16-bit encodings)
- W6-S7: Disassembler (decode bytes → Inst, round-trip with encode)

HARSH RULES:
- If encode→decode round-trip fails for ANY instruction, you have FAILED.
- If branch offset calculation is wrong, you have FAILED.
- If RVC compression/decompression is incorrect, you have FAILED.
- You MUST verify encodings against the RISC-V ISA Spec.
- 5+ test cases per instruction variant.
```

**W6-S8 through W6-S12: RISC-V64 Backend trait implementation + ELF**

```
You are implementing the Backend trait for RISC-V64.

SUBAGENT ASSIGNMENTS:
- W6-S8: RiscV64Target implementing TargetInfo
- W6-S9: RiscV64Backend implementing Backend (regalloc + encode_function)
- W6-S10: RISC-V64 register allocator (LP64D ABI: a0-a7 int args, fa0-fa7 FP args, s2-s11 callee-saved)
- W6-S11: RISC-V64 ELF64 emission (EM_RISCV=243, proper program headers, relocations)
- W6-S12: RISC-V64 test suite (50+ tests: encoding, calling conv, ELF, full pipeline)

HARSH RULES:
- If LP64D ABI is violated, you have FAILED.
- If ELF headers are malformed, you have FAILED.
- If register allocation is wrong, you have FAILED.
- cargo test -p vuma-codegen must pass.
- ALL ARM64 tests must still pass (no regressions).
```

### Success Criteria:
- Complete RISC-V64 backend with all RV64GC instructions
- Encoding/decoding round-trip works
- LP64D ABI implemented
- ELF64 emission works
- 50+ new tests pass
- No ARM64 regressions

---

## Wave 7: Wasm32 Backend (Stack Machine — Fundamentally Different)

**Dependencies**: W4, W5
**Estimated subagents**: 12

Wasm is the most unique target: no registers, stack machine, binary format instead of ELF, LEB128 encoding, and it runs in browsers. This proves the Backend trait truly generalizes.

### Subagent Tasks:

**W7-S1: Create wasm32.rs — Wasm types and instructions**

```
You are creating /home/z/my-project/vuma/src/codegen/src/wasm32.rs — the WebAssembly backend.

Wasm is fundamentally different from register machines:
- Stack-based: instructions push/pop values on an implicit stack
- No registers: has_registers() = false, all computation is stack-based
- Local variables instead of registers: each function has a list of typed locals
- Structured control flow: blocks, loops, ifs — NO arbitrary jumps
- Binary format: .wasm, not ELF
- LEB128 encoding for integers

YOUR TASK: Define the complete Wasm instruction set:

Value types: i32, i32, f32, f64
Control: block, loop, if, else, end, br, br_if, br_table, return, call, call_indirect
Parametric: select, drop
Variable: local.get, local.set, local.tee, global.get, global.set
Memory: i32.load, i64.load, f32.load, f64.load, i32.store, i64.store, f32.store, f64.store,
        memory.size, memory.grow
Numeric (i32): i32.const, i32.eqz, i32.eq, i32.ne, i32.lt_s, i32.lt_u, i32.gt_s, i32.gt_u,
               i32.le_s, i32.le_u, i32.ge_s, i32.ge_u, i32.clz, i32.ctz, i32.popcnt,
               i32.add, i32.sub, i32.mul, i32.div_s, i32.div_u, i32.rem_s, i32.rem_u,
               i32.and, i32.or, i32.xor, i32.shl, i32.shr_s, i32.shr_u, i32.rotl, i32.rotr
Numeric (i64): same operations as i32
Numeric (f32/f64): f32.const, f64.const, f32.eq, ..., f32.add, ..., f64.add, ...,
                   f32.convert_i32_s, f64.convert_i32_s, i32.trunc_f32_s, i32.trunc_f64_s, ...
Conversions: i32.wrap_i64, i64.extend_i32_s, i64.extend_i32_u,
             f32.convert_i32_s, f32.convert_i64_s, f64.convert_i32_s, f64.convert_i64_s,
             i32.trunc_f32_s, i32.trunc_f64_s, i64.trunc_f32_s, i64.trunc_f64_s,
             f32.demote_f64, f64.promote_f32, f32.reinterpret_i32, f64.reinterpret_i64,
             i32.reinterpret_f32, i64.reinterpret_f64

HARSH RULES:
- If you miss ANY Wasm instruction needed for the VUMA IR, you have FAILED.
- Wasm encoding uses LEB128 — implement it correctly (both unsigned and signed).
- Section encoding must follow the Wasm binary format spec.
- cargo check -p vuma-codegen must pass.
```

**W7-S2 through W7-S7: Wasm32 encoding + module generation**

```
You are implementing Wasm32 encoding and module generation.

SUBAGENT ASSIGNMENTS:
- W7-S2: LEB128 encoding/decoding + Wasm section structure (type, import, function, table, memory, global, export, start, element, code, data sections)
- W7-S3: Wasm binary format encoder (module → bytes)
- W7-S4: Wasm control flow lowering (IR structured control flow → Wasm block/loop/if)
- W7-S5: Wasm function code generation (IR → Wasm bytecode with stack discipline)
- W7-S6: Wasm32Target implementing TargetInfo (has_registers=false, output_format=WasmBinary)
- W7-S7: Wasm32Backend implementing Backend (encode_program produces .wasm bytes)

HARSH RULES:
- If the .wasm output doesn't validate with `wasm-validate`, you have FAILED.
- If the .wasm output can't run in wasmtime or wasmer, you have FAILED.
- Stack balance must be correct: every code path must leave the stack in the right state.
- Structured control flow must be correct (no dangling blocks).
- cargo test -p vuma-codegen must pass.
```

**W7-S8 through W7-S12: Wasm32 testing**

```
You are testing the Wasm32 backend.

SUBAGENT ASSIGNMENTS:
- W7-S8: Wasm encoding round-trip tests (encode → decode → verify)
- W7-S9: Wasm validation tests (every emitted .wasm must pass wasm-validate)
- W7-S10: Wasm execution tests (emit .wasm → run in wasmtime → verify result)
- W7-S11: Wasm calling convention tests (function import/export)
- W7-S12: Wasm memory model tests (linear memory load/store)

HARSH RULES:
- You MUST install wasmtime: curl https://wasmtime.dev/install.sh -sSf | bash
- You MUST test with actual Wasm execution, not just "produces bytes."
- If any .wasm fails validation, you have FAILED.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- Wasm32 backend produces valid .wasm files
- Emitted .wasm passes validation and executes correctly in wasmtime
- Proves the Backend trait works for non-register, non-ELF targets
- No regressions in other backends

---

## Wave 8: LoongArch64 Backend (Chinese Domestic ISA)

**Dependencies**: W4, W5
**Estimated subagents**: 12

LoongArch64 is architecturally similar to RISC-V (fixed 32-bit encoding, hardwired zero, link register) but for the Chinese domestic market. Implementing it alongside RISC-V validates the codebase handles similar-but-different ISAs cleanly.

### Subagent Tasks:

**W8-S1 through W8-S12: Full LoongArch64 backend**

```
You are implementing the LoongArch64 backend for VUMA.

LoongArch64 spec: 32 GP regs (r0=zero, r1=ra, r3=sp, r22=fp), 32 FP regs (f0-f31)
LP64 calling convention: a0-a7 (r4-r11) for int args, fa0-fa7 for FP args
Fixed 32-bit encoding, 9 instruction formats (2R, 3R, 4R, 2RI8, 2RI12, 2RI14, 2RI16, 1RI21, I26)
No branch delay slots.

SUBAGENT ASSIGNMENTS:
- W8-S1: LoongArch64 register + instruction type definitions
- W8-S2: Arithmetic + logical instruction encoding (3R format: ADD.W, SUB.W, AND, OR, XOR, etc.)
- W8-S3: Memory + branch instruction encoding (2RI12 format: LD.W, ST.W, BEQ, BNE, etc.)
- W8-S4: Jump + constant instruction encoding (1RI21, I26, 2RI16 formats: LU12I.W, JIRL, B, BL)
- W8-S5: Floating-point instruction encoding
- W8-S6: Disassembler (round-trip encode→decode)
- W8-S7: LoongArch64Target implementing TargetInfo
- W8-S8: LoongArch64Backend implementing Backend (regalloc, encode_function)
- W8-S9: LoongArch64 register allocator (LP64 ABI)
- W8-S10: LoongArch64 ELF64 emission (EM_LOONGARCH=258)
- W8-S11: LoongArch64 test suite (50+ tests)
- W8-S12: Cross-verification with RISC-V64 backend (same IR → both → verify semantic equivalence)

HARSH RULES:
- If encoding doesn't match the LoongArch Architecture Reference Manual, you have FAILED.
- If LP64 calling convention is violated, you have FAILED.
- If ELF headers are malformed, you have FAILED.
- cargo test -p vuma-codegen must pass.
- ALL other backend tests must still pass.
```

### Success Criteria:
- Complete LoongArch64 backend
- 50+ new tests
- No regressions

---

## Wave 9: x86_64 Backend (The Hard One)

**Dependencies**: W4, W5
**Estimated subagents**: 16

x86_64 is the most complex ISA (8/10) but the most important for us — it's our sandbox's native ISA and we can EXECUTE the output.

### Subagent Tasks:

**W9-S1: Create x86_64.rs — register + instruction model**

```
You are creating /home/z/my-project/vuma/src/codegen/src/x86_64.rs — the x86_64 backend.

x86_64 is the MOST COMPLEX ISA: variable-length encoding (1-15 bytes), REX prefixes,
ModR/M + SIB addressing, SystemV ABI with 6 int arg registers.

Registers:
- GP: RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, R8-R15
- XMM: XMM0-XMM15
- RSP = stack pointer (NOT allocatable), RBP = frame pointer (NOT allocatable)

SystemV ABI:
- Int args: RDI, RSI, RDX, RCX, R8, R9 (6 regs, then stack)
- FP args: XMM0-XMM7 (8 regs, then stack)
- Return: RAX (int), XMM0 (FP), RDX:RAX (128-bit)
- Callee-saved: RBX, R12-R15, RBP
- Stack must be 16-byte aligned before CALL

Required instructions:
- Data: MOV, MOVZX, MOVSX, LEA, XCHG, PUSH, POP
- Arithmetic: ADD, SUB, IMUL, IDIV, INC, DEC, NEG, NOT
- Logical: AND, OR, XOR, SHL, SHR, SAR, ROL, ROR
- Compare: CMP, TEST
- Conditional: SETcc, CMOVcc, Jcc
- Control: JMP, CALL, RET, NOP, INT3
- System: SYSCALL
- SSE2: MOVSD, ADDSD, SUBSD, MULSD, DIVSD, UCOMISD, CVTSI2SD, CVTTSD2SI
- Fences: MFENCE, LFENCE, SFENCE

HARSH RULES:
- Every instruction must have correct operand types.
- encode() MUST produce EXACT bytes that a real x86_64 CPU executes.
- You MUST handle REX prefix (0x40-0x4F) for 64-bit ops and extended registers.
- You MUST handle ModR/M + SIB for all memory addressing modes.
- No TODOs. No stubs.
```

**W9-S2 through W9-S9: x86_64 instruction encoding**

```
You are implementing x86_64 instruction encoding.

SUBAGENT ASSIGNMENTS:
- W9-S2: Data movement encoding (MOV, MOVZX, MOVSX, LEA, XCHG)
- W9-S3: Arithmetic encoding (ADD, SUB, IMUL, IDIV, INC, DEC, NEG, NOT)
- W9-S4: Logical + shift encoding (AND, OR, XOR, SHL, SHR, SAR, ROL, ROR)
- W9-S5: Compare + conditional (CMP, TEST, SETcc, CMOVcc, Jcc)
- W9-S6: Control flow + stack (JMP, CALL, RET, NOP, PUSH, POP, INT3, SYSCALL)
- W9-S7: SSE2 encoding (MOVSD, ADDSD, SUBSD, MULSD, DIVSD, UCOMISD, CVTSI2SD, CVTTSD2SI, fences)
- W9-S8: Disassembler (decode → Inst, round-trip with encode)
- W9-S9: Comprehensive encoding tests (5+ per instruction, verify against objdump)

HARSH RULES:
- If ANY encoding is wrong, the ENTIRE BACKEND IS BROKEN.
- You MUST test with objdump -d to verify emitted bytes are correct.
- If REX prefix handling is wrong, you have FAILED.
- If ModR/M + SIB is wrong, you have FAILED.
- Every encode() must return EXACT bytes that a real x86_64 CPU would execute.
```

**W9-S10 through W9-S16: x86_64 Backend trait + ELF + execution**

```
You are implementing the Backend trait for x86_64 and making it EXECUTABLE.

SUBAGENT ASSIGNMENTS:
- W9-S10: X86_64Target implementing TargetInfo
- W9-S11: X86_64Backend implementing Backend
- W9-S12: x86_64 register allocator (SystemV ABI, 14 allocatable GP + 16 XMM)
- W9-S13: SystemV ABI calling convention (arg classification, struct passing, stack alignment)
- W9-S14: x86_64 IR emission (IR → x86_64 Inst)
- W9-S15: x86_64 ELF64 emission (EM_X86_64=62, valid executable)
- W9-S16: EXECUTION TEST: emit "return 42" program, chmod +x, run it, verify exit code 42

HARSH RULES:
- If the emitted binary doesn't execute on this x86_64 machine, you have FAILED.
- If SystemV ABI is violated, you have FAILED.
- If stack alignment is wrong (must be 16-byte before CALL), you have FAILED.
- You MUST test with actual execution.
- If callee-saved regs aren't preserved, you have FAILED.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- x86_64 backend produces NATIVE EXECUTABLE code on this machine
- "return 42" program runs and returns 42
- SystemV ABI correctly implemented
- All other backends still pass

---

## Wave 10: RISC-V64 Emission + QEMU Validation

**Dependencies**: W6
**Estimated subagents**: 8

### Subagent Tasks:

**W10-S1 through W10-S8: RISC-V64 IR emission + QEMU execution**

```
You are implementing RISC-V64 IR emission and validating with QEMU.

SUBAGENT ASSIGNMENTS:
- W10-S1: IR → RISC-V64 instruction emission (every IR instruction mapped)
- W10-S2: RISC-V64 calling convention tests (LP64D ABI compliance)
- W10-S3: RISC-V64 switch/match lowering (using target-agnostic control_flow.rs)
- W10-S4: RISC-V64 exception handling lowering
- W10-S5: RISC-V64 tail call optimization
- W10-S6: Install QEMU RISC-V64: apt install qemu-user-static
          Test: emit RV64 ELF → run with qemu-riscv64-static → verify result
- W10-S7: RISC-V64 QEMU execution tests (20+ programs: arithmetic, loops, recursion, memory)
- W10-S8: RISC-V64 ELF validation (readelf, objdump)

HARSH RULES:
- If QEMU can't execute the emitted RISC-V64 code, you have FAILED.
- If any test produces wrong results in QEMU, you have FAILED.
- You MUST use qemu-riscv64-static to actually run the emitted code.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- RISC-V64 code executes correctly in QEMU
- 20+ QEMU execution tests pass

---

## Wave 11: Wasm32 Emission + Runtime Validation

**Dependencies**: W7
**Estimated subagents**: 8

### Subagent Tasks:

**W11-S1 through W11-S8: Wasm32 full emission + wasmtime validation**

```
You are implementing Wasm32 full emission and validating with wasmtime.

SUBAGENT ASSIGNMENTS:
- W11-S1: Full Wasm module generation (type section, import section, function section, export section, code section, data section)
- W11-S2: Wasm memory model (linear memory, data segments)
- W11-S3: Wasm WASI integration (fd_write for stdout, args_get for CLI args)
- W11-S4: Wasm execution in wasmtime (compile .wasm → instantiate → call → verify)
- W11-S5: Wasm execution in wasmer (same tests, different runtime)
- W11-S6: Wasm browser test (generate HTML+JS that loads and runs .wasm)
- W11-S7: Wasm control flow tests (block, loop, if, br_table)
- W11-S8: Wasm multi-function programs

HARSH RULES:
- If .wasm doesn't validate with `wasm-validate`, you have FAILED.
- If .wasm doesn't execute in wasmtime, you have FAILED.
- If stack balance is wrong, you have FAILED.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- Wasm32 code validates and executes in wasmtime
- 20+ wasmtime execution tests pass

---

## Wave 12: LoongArch64 Emission + QEMU Validation

**Dependencies**: W8
**Estimated subagents**: 8

### Subagent Tasks:

**W12-S1 through W12-S8: LoongArch64 emission + QEMU execution**

```
SUBAGENT ASSIGNMENTS:
- W12-S1: IR → LoongArch64 instruction emission
- W12-S2: LoongArch64 calling convention tests
- W12-S3: LoongArch64 control flow lowering
- W12-S4: Install QEMU LoongArch64: build from source (QEMU 7.2+)
- W12-S5: LoongArch64 QEMU execution tests (20+ programs)
- W12-S6: LoongArch64 ELF validation
- W12-S7: Cross-verification: same IR → RV64 + LA64 → both produce correct results
- W12-S8: LoongArch64 regression tests

HARSH RULES:
- If QEMU can't execute the emitted LoongArch64 code, you have FAILED.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- LoongArch64 code executes in QEMU
- Cross-verification with RISC-V64 passes

---

## Wave 13: x86_64 Emission + Native Execution

**Dependencies**: W9
**Estimated subagents**: 12

### Subagent Tasks:

**W13-S1 through W13-S12: x86_64 full emission + native execution**

```
You are implementing x86_64 full emission and EXECUTING IT NATIVELY on this machine.

SUBAGENT ASSIGNMENTS:
- W13-S1: IR → x86_64 emission (every IR instruction category)
- W13-S2: Arithmetic functions (add, sub, mul, div, rem, neg, not)
- W13-S3: Bitwise functions (and, or, xor, shl, shr, sar)
- W13-S4: Control flow (if/else, loops, switch, nested control flow)
- W13-S5: Functions with 0-20 arguments
- W13-S6: Recursive functions (factorial, fibonacci, tree)
- W13-S7: Memory operations (load, store, stack, heap)
- W13-S8: Exception handling and coroutines
- W13-S9: Full pipeline: source → parse → SCG → BD → IVE → IR → x86_64 → ELF → EXECUTE
- W13-S10: Cross-backend: same source → ARM64 + x86_64 → both produce correct results
- W13-S11: Performance: compile 100+ function program, execute, verify
- W13-S12: x86_64 ELF validation (readelf, objdump, file)

FOR EACH EXECUTION TEST:
1. Emit x86_64 ELF to /tmp/vuma_test_N.elf
2. chmod +x
3. Execute: /tmp/vuma_test_N.elf
4. Verify exit code = expected result
5. If wrong, objdump -d the ELF and debug the emitted code

HARSH RULES:
- If the emitted binary crashes (segfault), you have FAILED.
- If the emitted binary returns wrong result, you have FAILED.
- You MUST test with actual execution on THIS x86_64 machine.
- If stack is misaligned, you have FAILED.
- If calling convention is wrong, you have FAILED.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- x86_64 code executes natively with correct results
- 50+ native execution tests pass
- Cross-backend parity with ARM64 verified

---

## Wave 14: RISC-V64 + LoongArch64 Integration

**Dependencies**: W10, W12
**Estimated subagents**: 8

### Subagent Tasks:

**W14-S1 through W14-S8: RISC-V64 + LoongArch64 full pipeline integration**

```
You are testing full pipeline integration for RISC-V64 and LoongArch64.

SUBAGENT ASSIGNMENTS:
- W14-S1: Full RISC-V64 pipeline (source → SCG → BD → IVE → IR → RV64 → QEMU)
- W14-S2: Full LoongArch64 pipeline (source → SCG → BD → IVE → IR → LA64 → QEMU)
- W14-S3: Cross-ISA parity: 20 programs → ARM64 + RV64 + LA64 + x86_64 → all correct
- W14-S4: RISC-V64 COR execution (compile → QEMU execute → profile → optimize → re-execute)
- W14-S5: LoongArch64 COR execution
- W14-S6: RISC-V64 + LoongArch64 REPL (compile → disassemble → verify)
- W14-S7: RISC-V64 edge cases (compressed instructions, misaligned access)
- W14-S8: LoongArch64 edge cases

HARSH RULES:
- If any ISA produces incorrect results, you have FAILED.
- Cross-ISA parity must hold: same source → same semantic result on all 4 ISAs.
- cargo test --workspace must pass.
```

### Success Criteria:
- 4 ISA backends (ARM64, RV64, LA64, x86_64) all produce correct results
- Cross-ISA parity verified

---

## Wave 15: COR x86_64 Native Execution

**Dependencies**: W13
**Estimated subagents**: 8

### Subagent Tasks:

**W15-S1: COR execute_code for x86_64**

```
You are implementing COR runtime code execution on x86_64.

CURRENT STATE:
- ARM64: mmap + mprotect(PROT_READ|PROT_EXEC) + transmute to fn pointer + call
- x86_64: Returns Ok(0) (STUB!)

YOUR TASK:
1. Add #[cfg(all(unix, target_arch = "x86_64"))] implementation:
   - mmap code region with PROT_READ|PROT_WRITE
   - copy compiled x86_64 code
   - mprotect(PROT_READ|PROT_EXEC)
   - transmute to fn pointer
   - call it
   - munmap
2. Wire Backend selection: Config::TargetArch::X86_64 → X86_64Backend
3. Update return_zero_stub: x86_64 returns MOV RAX,0 + RET (0xC3)
4. Test: compile "return arg*2" → execute via COR → verify result

HARSH RULES:
- If COR still returns 0 on x86_64, you have FAILED.
- If compiled code doesn't execute correctly, you have FAILED.
- You MUST test with actual mmap/mprotect/execute.
- cargo test -p vuma-cor must pass.
```

**W15-S2 through W15-S8: COR full cycle on x86_64**

```
SUBAGENT ASSIGNMENTS:
- W15-S2: COR compile → execute for simple functions (10+ tests)
- W15-S3: COR recursive functions (factorial, fibonacci)
- W15-S4: COR profiling on x86_64 (perf counters)
- W15-S5: COR optimization passes on x86_64
- W15-S6: COR full cycle: compile → execute → profile → optimize → re-execute → verify improvement
- W15-S7: COR incremental compilation on x86_64
- W15-S8: COR stress test (100 functions, 1000 executions each)

HARSH RULES:
- If COR optimization produces incorrect code, you have FAILED.
- If there are memory leaks, you have FAILED.
- cargo test -p vuma-cor must pass.
```

### Success Criteria:
- COR works fully on x86_64 (compile, execute, profile, optimize, re-execute)
- All COR tests pass

---

## Wave 16: ARM32 Backend (Legacy + Embedded)

**Dependencies**: W13 (Backend pattern established)
**Estimated subagents**: 16

ARM32 is important for the embedded market. It's harder than ARM64 (variable-length Thumb, conditional execution, PC as R15) but follows the established backend pattern.

### Subagent Tasks:

**W16-S1 through W16-S16: Full ARM32 backend**

```
You are implementing the ARM32 backend for VUMA.

ARM32 is 6/10 complexity. Key challenges:
- Two instruction sets: ARM (32-bit) and Thumb/Thumb-2 (16/32-bit mixed)
- Conditional execution: most ARM instructions can be conditional (EQ, NE, LT, etc.)
- PC is R15 — visible to programmers, causes pipeline complications
- AAPCS calling convention: 4 int args (R0-R3), 16 FP args (S0-S15), stack for overflow
- 16 GP regs (R0-R15), 32 FP/SIMD (D0-D31, S0-S31)

SUBAGENT ASSIGNMENTS:
- W16-S1: ARM32 register + instruction type definitions (ARM + Thumb)
- W16-S2: ARM encoding (data processing, load/store, branch)
- W16-S3: Thumb encoding (16-bit + 32-bit Thumb-2)
- W16-S4: ARM conditional instruction encoding
- W16-S5: ARM multiply, divide, DSP instructions
- W16-S6: VFP/NEON FP instruction encoding
- W16-S7: ARM32 disassembler (round-trip encode→decode)
- W16-S8: ARM32Target implementing TargetInfo
- W16-S9: ARM32Backend implementing Backend
- W16-S10: ARM32 register allocator (AAPCS)
- W16-S11: ARM32 calling convention (AAPCS)
- W16-S12: ARM32 ELF32 emission (EM_ARM=40)
- W16-S13: ARM32 IR emission (IR → ARM/Thumb instructions)
- W16-S14: ARM32 QEMU execution tests (qemu-arm-static)
- W16-S15: ARM32 control flow lowering (switch, exception, tailcall)
- W16-S16: ARM32 test suite (50+ tests)

HARSH RULES:
- If ARM32 encoding doesn't match ARM Architecture Reference Manual, you have FAILED.
- If AAPCS calling convention is violated, you have FAILED.
- If conditional execution is not supported, you have FAILED.
- You MUST test with qemu-arm-static.
- If Thumb interworking is wrong, you have FAILED.
- cargo test -p vuma-codegen must pass.
- ALL other backend tests must still pass.
```

### Success Criteria:
- Complete ARM32 backend with ARM + Thumb support
- AAPCS calling convention implemented
- Code executes in QEMU
- 50+ new tests

---

## Wave 17: MIPS64 Backend (Branch Delay Slots)

**Dependencies**: W13
**Estimated subagents**: 16

MIPS64 is unique for its branch delay slots — the instruction AFTER a branch ALWAYS executes. This requires special handling in the code generator.

### Subagent Tasks:

**W17-S1 through W17-S16: Full MIPS64 backend**

```
You are implementing the MIPS64 backend for VUMA.

MIPS64 is 5/10 complexity. Key challenges:
- BRANCH DELAY SLOTS: instruction after branch always executes. Must insert NOP or useful instr.
- HI/LO registers for multiply/divide results
- N64 ABI: 4 int args ($a0-$a3), 4 FP args ($f12-$f15), more on stack
- 32 GP regs ($zero=0, $at, $v0-$v1, $a0-$a3, $t0-$t9, $s0-$s7, $k0-$k1, $gp, $sp, $fp, $ra)
- 32 FP regs ($f0-$f31)

SUBAGENT ASSIGNMENTS:
- W17-S1: MIPS64 register + instruction type definitions (R, I, J formats)
- W17-S2: R-type encoding (arithmetic, logical, shift, multiply, divide)
- W17-S3: I-type encoding (immediate, load/store, branch)
- W17-S4: J-type encoding (jump, jal)
- W17-S5: FP instruction encoding
- W17-S6: Branch delay slot handling (insert NOPs or reorder instructions)
- W17-S7: MIPS64 disassembler (round-trip)
- W17-S8: MIPS64Target implementing TargetInfo (has_branch_delay_slots=true!)
- W17-S9: MIPS64Backend implementing Backend
- W17-S10: MIPS64 register allocator (N64 ABI)
- W17-S11: MIPS64 calling convention (N64 ABI)
- W17-S12: MIPS64 ELF64 emission (EM_MIPS=8, big-endian by default)
- W17-S13: MIPS64 IR emission with delay slot insertion
- W17-S14: MIPS64 QEMU execution tests (qemu-mips64-static)
- W17-S15: MIPS64 control flow lowering (switch with delay slots)
- W17-S16: MIPS64 test suite (50+ tests)

HARSH RULES:
- If branch delay slots are not handled correctly, you have FAILED CATASTROPHICALLY.
- If HI/LO register usage is wrong for multiply/divide, you have FAILED.
- If N64 ABI is violated, you have FAILED.
- You MUST test with qemu-mips64-static.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- Complete MIPS64 backend with branch delay slot handling
- N64 ABI implemented
- Code executes in QEMU
- 50+ new tests

---

## Wave 18: PowerPC64 Backend (TOC + Condition Registers)

**Dependencies**: W13
**Estimated subagents**: 16

PowerPC64 is the most complex RISC ISA (7/10). TOC pointer, 8 condition register fields, VSX overlapping register file, rlwinm bit-field operations, and ELFv2 ABI.

### Subagent Tasks:

**W18-S1 through W18-S16: Full PowerPC64 backend**

```
You are implementing the PowerPC64 backend for VUMA.

PowerPC64 is 7/10 complexity. Key challenges:
- TOC pointer (r2): all function calls and global variable access go through TOC
- 8 condition register fields (CR0-CR7), each with 4 bits (LT, GT, EQ, SO)
- VSX registers: 64 registers overlapping FPRs and VMXs
- rlwinm family: rotate-left-then-mask-insert — used for ALL bit-field operations
- ELFv2 ABI: 8 int args (R3-R10), 13 FP args (F1-F13), TOC in R2, R12 is entry point

SUBAGENT ASSIGNMENTS:
- W18-S1: PPC64 register definitions (GP, FPR, VMX, VSX, CR fields)
- W18-S2: PPC64 instruction format encoding (all 6 formats)
- W18-S3: Integer arithmetic + logical + shift + rotate encoding
- W18-S4: Load/store encoding (indexed, update, string)
- W18-S5: Branch + CR manipulation encoding
- W18-S6: FP + VSX instruction encoding
- W18-S7: PPC64 disassembler (round-trip)
- W18-S8: PPC64Target implementing TargetInfo (has_toc_pointer=true, has_condition_registers=true)
- W18-S9: PPC64Backend implementing Backend
- W18-S10: PPC64 register allocator (ELFv2 ABI)
- W18-S11: PPC64 calling convention (ELFv2, TOC handling, function descriptors)
- W18-S12: PPC64 ELF64 emission (EM_PPC64=21, big-endian by default)
- W18-S13: PPC64 IR emission (including rlwinm generation for bit-field ops)
- W18-S14: PPC64 QEMU execution tests (qemu-ppc64-static or qemu-system-ppc64)
- W18-S15: PPC64 control flow lowering (CR-based conditional branches)
- W18-S16: PPC64 test suite (50+ tests)

HARSH RULES:
- If TOC handling is wrong, you have FAILED.
- If CR field manipulation is incorrect, you have FAILED.
- If ELFv2 ABI is violated, you have FAILED.
- If rlwinm encoding is wrong, you have FAILED.
- You MUST test with QEMU.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- Complete PowerPC64 backend with TOC and CR handling
- ELFv2 ABI implemented
- Code executes in QEMU
- 50+ new tests

---

## Wave 19: Multi-Backend Integration Test

**Dependencies**: W14, W15
**Estimated subagents**: 12

### Subagent Tasks:

**W19-S1 through W19-S12: Cross-ISA integration testing**

```
You are writing integration tests that verify ALL backends work together.

SUBAGENT ASSIGNMENTS:
- W19-S1: Cross-ISA parity test: 30 programs → ARM64 + RV64 + x86_64 + Wasm → all correct
- W19-S2: Cross-ISA parity: add LA64, ARM32 → 30 programs → 5 ISAs → all correct
- W19-S3: Cross-ISA parity: add MIPS64, PPC64 → 30 programs → ALL 8 ISAs → all correct
- W19-S4: Backend factory test: create_backend for each of 8 kinds → all work
- W19-S5: Pipeline with target selection: --target riscv64 → produces RV64 ELF
- W19-S6: Full pipeline: source → all 8 backends → 8 outputs → all semantically correct
- W19-S7: COR with multi-backend: compile for x86_64 → execute natively
- W19-S8: COR with multi-backend: compile for RV64 → execute in QEMU
- W19-S9: Cross-compilation: x86_64 host → ARM64 target → valid ARM64 ELF
- W19-S10: Error handling: invalid target → error message
- W19-S11: REPL with multi-backend: :target riscv64 → switch backend
- W19-S12: Performance benchmark: compile same program for all 8 ISAs, measure compile time

HARSH RULES:
- If ANY backend produces incorrect results, you have FAILED.
- If cross-ISA parity doesn't hold, you have FAILED.
- You MUST test with actual execution where possible (x86_64 native, RV64/LA64/ARM32/MIPS/PPC in QEMU, Wasm in wasmtime).
- cargo test --workspace must pass.
```

### Success Criteria:
- All 8 backends produce correct results
- Cross-ISA parity verified for 30+ programs
- Full pipeline works with any target selection

---

## Wave 20: QEMU Test Infrastructure

**Dependencies**: W19
**Estimated subagents**: 8

### Subagent Tasks:

**W20-S1 through W20-S8: Automated QEMU testing for all ISAs**

```
You are building automated QEMU-based testing infrastructure for VUMA.

SUBAGENT ASSIGNMENTS:
- W20-S1: Install all QEMU variants: qemu-{arm,aarch64,riscv64,mips64,ppc64}-static
- W20-S2: Create QemuTestHarness struct: compile → emit ELF → run in QEMU → capture exit code
- W20-S3: RISC-V64 QEMU test suite (30+ programs)
- W20-S4: LoongArch64 QEMU test suite (30+ programs)
- W20-S5: ARM32 QEMU test suite (30+ programs)
- W20-S6: MIPS64 QEMU test suite (30+ programs)
- W20-S7: PowerPC64 QEMU test suite (30+ programs)
- W20-S8: Wasm wasmtime test suite (30+ programs)

FOR EACH QEMU TEST:
1. Compile VUMA source to target ISA
2. Emit ELF (or .wasm for Wasm)
3. Run in QEMU (or wasmtime for Wasm)
4. Capture exit code / output
5. Verify against expected result

HARSH RULES:
- If QEMU can't run any emitted binary, you have FAILED.
- If any test produces wrong results, you have FAILED.
- cargo test --workspace must pass.
```

### Success Criteria:
- All 8 ISAs have 30+ QEMU/wasmtime execution tests
- All tests pass

---

## Wave 21: Full Codebase Audit

**Dependencies**: W20
**Estimated subagents**: 12

### Subagent Tasks:

**W21-S1 through W21-S12: Per-crate comprehensive audit**

```
You are auditing CRATE_NAME for quality, correctness, and consistency across all 8 backends.

CRATE ASSIGNMENTS:
- W21-S1: vuma-parser
- W21-S2: vuma-scg
- W21-S3: vuma-bd
- W21-S4: vuma-ive
- W21-S5: vuma-codegen (CRITICAL — audit all 8 backends for consistency)
- W21-S6: vuma-cor
- W21-S7: vuma-core
- W21-S8: vuma-std
- W21-S9: vuma-pi5
- W21-S10: vuma-proof
- W21-S11: vuma-projection
- W21-S12: vuma-tests

FOR EACH CRATE:
1. cargo clippy -p CRATE_NAME -- -D warnings → fix ALL warnings
2. cargo fmt -p CRATE_NAME -- --check → fix ALL formatting
3. Check for: unwrap() in non-test, panic!() in non-test, unsafe without SAFETY comment
4. Check for: hardcoded ISA assumptions (only ARM64 is acceptable in arm64.rs)
5. Generate audit report: lines, public APIs, tests, unsafe blocks, TODOs

HARSH RULES:
- If clippy has ANY warnings, you have FAILED.
- If formatting is wrong, you have FAILED.
- If you find a bug and don't fix it, you have FAILED.
- cargo test --workspace must pass.
```

### Success Criteria:
- `cargo clippy --workspace -- -D warnings` passes
- `cargo fmt -- --check` passes
- All crates audited

---

## Wave 22: Cross-Compilation + Release

**Dependencies**: W21
**Estimated subagents**: 12

### Subagent Tasks:

**W22-S1 through W22-S12: Final validation + release**

```
You are preparing VUMA v0.2.0 — the first multi-architecture release.

SUBAGENT ASSIGNMENTS:
- W22-S1: Version bump all Cargo.toml to 0.2.0
- W22-S2: Update CHANGELOG.md
- W22-S3: cargo clean && cargo build --workspace --release
- W22-S4: cargo test --workspace --release
- W22-S5: cargo clippy --workspace -- -D warnings -W clippy::pedantic
- W22-S6: cargo doc --workspace --no-deps
- W22-S7: Cross-compilation: build for all 8 targets, validate ELF headers
- W22-S8: QEMU execution: run 30+ programs on each of 7 QEMU targets
- W22-S9: x86_64 native execution: run 50+ programs
- W22-S10: Wasm execution: run 30+ programs in wasmtime
- W22-S11: Performance benchmark: compile time, execution time per ISA
- W22-S12: git tag v0.2.0 && git push origin main --tags

VERIFICATION (ALL must pass):
```bash
export PATH="/home/z/.cargo/bin:$PATH"
cd /home/z/my-project/vuma
cargo clean
cargo build --workspace --release
cargo test --workspace --release
cargo clippy --workspace -- -D warnings
cargo fmt -- --check
cargo doc --workspace --no-deps
```

HARSH RULES:
- If ANY verification step fails, the release is BLOCKED.
- If there are any TODO/FIXME in non-test code, the release is BLOCKED.
- If any backend produces incorrect results, the release is BLOCKED.
- THERE ARE NO EXCEPTIONS.
```

### Success Criteria:
- All verification commands pass with zero errors/warnings
- Version tagged as v0.2.0
- Pushed to GitHub

---

## Wave 23: Test Framework for Multi-ISA

**Dependencies**: W3
**Estimated subagents**: 8

### Subagent Tasks:

**W23-S1: Multi-ISA test harness**

```
You are creating a test harness at /home/z/my-project/vuma/src/tests/src/multi_isa_harness.rs
that can compile and test VUMA code across ALL 8 ISAs.

```rust
pub struct MultiIsaHarness {
    backends: Vec<BackendKind>,
}

impl MultiIsaHarness {
    /// Compile source for all registered backends.
    pub fn compile_all(&self, source: &str) -> HashMap<BackendKind, CompilationResult>;
    
    /// Compile and execute for a specific backend.
    pub fn execute(&self, source: &str, target: BackendKind, arg: usize) -> Result<usize>;
    
    /// Cross-ISA parity test: compile + execute on all backends, verify all agree.
    pub fn cross_isa_parity(&self, source: &str, arg: usize) -> Result<()>;
}
```

Support execution strategies:
- Native: x86_64 (run directly)
- QEMU: ARM64, RV64, LA64, ARM32, MIPS64, PPC64 (run via qemu-<isa>-static)
- Wasmtime: Wasm32 (run via wasmtime)
- Validation-only: check ELF/binary format without execution

HARSH RULES:
- The harness must work for ALL 8 ISAs.
- If cross-ISA parity testing doesn't work, you have FAILED.
- cargo test -p vuma-tests must pass.
```

**W23-S2 through W23-S8: Multi-ISA test suites**

```
SUBAGENT ASSIGNMENTS:
- W23-S2: Parser tests (ISA-independent, 50+ tests)
- W23-S3: SCG + BD tests (ISA-independent, 40+ tests)
- W23-S4: IVE verification tests (ISA-independent, 40+ tests)
- W23-S5: Cross-ISA parity tests (same source → all 8 backends → verify, 30+ tests)
- W23-S6: Backend-specific edge case tests (per-ISA quirks: delay slots, REX prefixes, RVC, etc.)
- W23-S7: COR tests (compile → execute → profile → optimize, multi-ISA)
- W23-S8: Stress tests (large programs, 100+ functions, all ISAs)

HARSH RULES:
- Each subagent must write at least 30 tests.
- cargo test --workspace must pass.
```

### Success Criteria:
- Multi-ISA test harness works for all 8 ISAs
- 200+ new tests across all suites

---

## Wave 24: Pi5 Platform Abstraction

**Dependencies**: W3
**Estimated subagents**: 8

### Subagent Tasks:

**W24-S1 through W24-S8: Pi5 platform abstraction for multi-ISA testing**

```
You are making vuma-pi5 testable on any architecture, not just ARM64.

SUBAGENT ASSIGNMENTS:
- W24-S1: Create Pi5Backend trait (mmio_read, mmio_write, barrier_dmb, barrier_dsb, timer_read)
- W24-S2: RealPi5Backend (ARM64 only, inline asm)
- W24-S3: MockPi5Backend (x86_64 + any, simulated state)
- W24-S4: Refactor uart.rs to use Pi5Backend
- W24-S5: Refactor gpio.rs to use Pi5Backend
- W24-S6: Full MMIO emulator (simulated PL011 UART, RP1 GPIO, BCM2712 timer)
- W24-S7: Pi5 driver tests against emulator on x86_64 (30+ tests)
- W24-S8: Pi5 integration: compile Pi5 program with ARM64 backend → verify ARM64 ELF

HARSH RULES:
- If Pi5 tests can't run on x86_64, you have FAILED.
- If ARM64-only functionality is broken, you have FAILED.
- cargo test -p vuma-pi5 must pass on x86_64.
```

### Success Criteria:
- vuma-pi5 fully testable on x86_64
- All ARM64 functionality preserved

---

## Wave 25: Error Handling Audit

**Dependencies**: W19
**Estimated subagents**: 8

### Subagent Tasks:

**W25-S1 through W25-S8: Per-crate error handling audit**

```
You are auditing error handling in CRATE_NAME for all 8 backends.

CRATE ASSIGNMENTS:
- W25-S1: vuma-parser (parse errors must be informative)
- W25-S2: vuma-scg (SCG construction errors)
- W25-S3: vuma-bd (BD inference errors)
- W25-S4: vuma-ive (IVE verification errors — must say WHICH invariant failed and WHY)
- W25-S5: vuma-codegen (backend errors — must say WHICH ISA failed and WHAT went wrong)
- W25-S6: vuma-cor (runtime errors — mmap, mprotect, execution failures)
- W25-S7: vuma-core (pipeline errors — must propagate ISA context)
- W25-S8: vuma-std, vuma-pi5, vuma-proof, vuma-projection

AUDIT CRITERIA:
1. No unwrap() in non-test code
2. No panic!() in non-test code (except impossible states)
3. All errors include: what failed, why, where, how to fix
4. Backend errors include ISA name (e.g., "RISC-V64 encoding failed: ...")
5. No silent error swallowing

HARSH RULES:
- If any unwrap() remains in non-test code, you have FAILED.
- If any error message doesn't include ISA context in codegen, you have FAILED.
- cargo test --workspace must pass.
```

### Success Criteria:
- Zero unwrap/panic in non-test code
- All error messages are informative and include ISA context

---

## Wave 26: Memory Safety Audit

**Dependencies**: W19
**Estimated subagents**: 8

### Subagent Tasks:

**W26-S1 through W26-S8: Per-crate memory safety audit**

```
You are auditing memory safety across all 8 backends.

CRATE ASSIGNMENTS:
- W26-S1: vuma-codegen (unsafe in 8 backend encode/decode + ELF emission)
- W26-S2: vuma-cor (unsafe in mmap/mprotect/transmute/execute for ALL ISA execution paths)
- W26-S3: vuma-pi5 (unsafe in MMIO, UART, GPIO)
- W26-S4: vuma-std (unsafe in alloc, collections)
- W26-S5: vuma-core (unsafe in pipeline, security)
- W26-S6: Remaining crates (check for unsafe)
- W26-S7: Static mut audit (replace with AtomicX, OnceCell, etc.)
- W26-S8: Integer overflow audit (checked arithmetic for size calculations)

HARSH RULES:
- If any unsafe block lacks a SAFETY comment, you have FAILED.
- If any unsafe can be replaced with safe code, you have FAILED.
- If any static mut can be replaced, you have FAILED.
- cargo test --workspace must pass.
```

### Success Criteria:
- All unsafe blocks have SAFETY comments
- No unnecessary unsafe
- No unjustified static mut

---

## Wave 27: UB Sanitization

**Dependencies**: W26
**Estimated subagents**: 8

### Subagent Tasks:

**W27-S1 through W27-S8: Miri, ASAN, TSAN across all backends**

```
SUBAGENT ASSIGNMENTS:
- W27-S1: cargo miri test --workspace → fix ALL UB
- W27-S2: RUSTFLAGS="-Zsanitizer=address" cargo test → fix ALL memory errors
- W27-S3: RUSTFLAGS="-Zsanitizer=thread" cargo test → fix ALL data races
- W27-S4: Fix Miri findings in vuma-codegen (all backends)
- W27-S5: Fix Miri findings in vuma-cor (all execution paths)
- W27-S6: Fix ASAN findings
- W27-S7: Fix TSAN findings
- W27-S8: Verify all sanitizers pass clean

HARSH RULES:
- If Miri finds UB and you don't fix it, you have FAILED.
- If ASAN finds memory errors and you don't fix them, you have FAILED.
- If TSAN finds data races and you don't fix them, you have FAILED.
- ALL sanitizers must pass with ZERO findings.
```

### Success Criteria:
- Miri: zero UB
- ASAN: zero memory errors
- TSAN: zero data races

---

## Wave 28: Documentation for Multi-Architecture System

**Dependencies**: W27
**Estimated subagents**: 10

### Subagent Tasks:

**W28-S1: Architecture documentation**

```
You are writing architecture documentation for VUMA's multi-ISA system.

Write docs/architecture.md covering:
1. System overview (BD reasoning, five invariants, SCG, COR)
2. 12-crate architecture
3. Multi-backend architecture (Backend trait, TargetInfo, 8 ISAs)
4. How to add a new ISA backend (step-by-step guide)
5. Cross-compilation model
6. QEMU test infrastructure
7. ISA-specific considerations (delay slots, TOC, condition registers, stack machines)

Minimum 3000 words. Every paragraph must convey information.

HARSH RULES:
- If the documentation is inaccurate, you have FAILED.
- If "How to add a new ISA" section is incomplete, you have FAILED.
- If a new contributor couldn't add a 9th ISA from the guide, you have FAILED.
```

**W28-S2 through W28-S10: Per-crate documentation**

```
SUBAGENT ASSIGNMENTS:
- W28-S2: vuma-parser docs
- W28-S3: vuma-scg docs
- W28-S4: vuma-bd docs
- W28-S5: vuma-ive docs
- W28-S6: vuma-codegen docs (CRITICAL — document Backend trait, all 8 ISAs, TargetDesc)
- W28-S7: vuma-cor docs
- W28-S8: vuma-core docs
- W28-S9: vuma-std + vuma-pi5 docs
- W28-S10: vuma-proof + vuma-projection + vuma-tests docs

FOR EACH CRATE:
1. Add rustdoc to ALL public APIs.
2. Add module-level doc comments.
3. Add code examples in doc comments.
4. cargo doc -p CRATE_NAME --no-deps → zero warnings.
5. Write docs/CRATE_NAME.md with usage guide.

HARSH RULES:
- If any public API lacks documentation, you have FAILED.
- If doc tests fail, you have FAILED.
- cargo doc --workspace must pass with zero warnings.
```

### Success Criteria:
- All public APIs documented
- Architecture docs cover all 8 ISAs
- `cargo doc --workspace` passes

---

## Wave 29: CI/CD for Multi-ISA

**Dependencies**: W27
**Estimated subagents**: 8

### Subagent Tasks:

**W29-S1 through W29-S8: GitHub Actions CI for all 8 ISAs**

```
SUBAGENT ASSIGNMENTS:
- W29-S1: Main CI workflow (check, test, clippy, fmt, doc)
- W29-S2: QEMU CI workflow (ARM64, RV64, LA64, ARM32, MIPS64, PPC64 execution tests)
- W29-S3: Wasm CI workflow (wasmtime + wasmer validation)
- W29-S4: Cross-compilation CI (all 8 targets produce valid output)
- W29-S5: Miri nightly CI
- W29-S6: Coverage CI (cargo tarpaulin)
- W29-S7: Security audit CI (cargo audit)
- W29-S8: Release automation (tag → build all 8 targets → publish)

HARSH RULES:
- CI must cover ALL 8 ISAs.
- If CI doesn't catch a backend regression, you have FAILED.
- All workflows must pass on the current codebase.
```

### Success Criteria:
- Full CI/CD covering all 8 ISAs
- All CI jobs pass

---

## Wave 30: Performance Optimization

**Dependencies**: W28
**Estimated subagents**: 8

### Subagent Tasks:

**W30-S1 through W30-S8: Performance optimization across all backends**

```
SUBAGENT ASSIGNMENTS:
- W30-S1: Compile-time benchmarks (measure compilation speed per ISA)
- W30-S2: ARM64 codegen optimization (instruction scheduling, better regalloc)
- W30-S3: x86_64 codegen optimization (instruction fusion, better addressing modes)
- W30-S4: RISC-V64 codegen optimization (RVC compression, instruction scheduling)
- W30-S5: Wasm32 optimization (stack machine optimization, local reuse)
- W30-S6: Cross-ISA compile-time comparison (identify bottlenecks)
- W30-S7: Memory usage optimization (reduce allocations during compilation)
- W30-S8: COR optimization for multi-ISA (profile-guided optimization for each backend)

HARSH RULES:
- If optimization breaks correctness, you have FAILED.
- If optimization makes any backend slower, you have FAILED.
- You MUST measure before and after with actual benchmarks.
- cargo test --workspace must pass.
```

### Success Criteria:
- Measurable performance improvement
- No correctness regressions

---

## Wave 31: Final Validation

**Dependencies**: W30, W29
**Estimated subagents**: 16

### Subagent Tasks:

**W31-S1 through W31-S16: Final validation for all 8 ISAs**

```
You are performing FINAL validation for VUMA v0.2.0 across all 8 ISAs.

SUBAGENT ASSIGNMENTS:
- W31-S1: AArch64: 50 QEMU execution tests + golden file regression
- W31-S2: RISC-V64: 50 QEMU execution tests
- W31-S3: Wasm32: 50 wasmtime execution tests
- W31-S4: LoongArch64: 50 QEMU execution tests
- W31-S5: x86_64: 50 NATIVE execution tests
- W31-S6: ARM32: 50 QEMU execution tests
- W31-S7: MIPS64: 50 QEMU execution tests (with delay slot verification)
- W31-S8: PowerPC64: 50 QEMU execution tests (with TOC/CR verification)
- W31-S9: Cross-ISA parity: 50 programs → all 8 ISAs → all produce same result
- W31-S10: Full pipeline: source → parse → SCG → BD → IVE → IR → each backend → valid output
- W31-S11: COR cycle: compile → execute → profile → optimize → re-execute (x86_64 native)
- W31-S12: COR QEMU cycle: compile → QEMU execute → verify (ARM64, RV64)
- W31-S13: REPL test: 50+ expressions per backend
- W31-S14: Error handling: 30 error scenarios, verify informative messages
- W31-S15: Stress: 200+ function program, all 8 ISAs
- W31-S16: Clean build: cargo clean && cargo build --release && cargo test --release

HARSH RULES:
- If ANY test fails on ANY ISA, the release is BLOCKED.
- If cross-ISA parity doesn't hold, the release is BLOCKED.
- If ANY binary crashes, the release is BLOCKED.
- THERE ARE NO EXCEPTIONS.
```

### Success Criteria:
- 400+ execution tests across all 8 ISAs, all passing
- Cross-ISA parity verified
- Clean build passes

---

## Wave 32: Release

**Dependencies**: W31
**Estimated subagents**: 12

### Subagent Tasks:

**W32-S1 through W32-S12: Release v0.2.0**

```
SUBAGENT ASSIGNMENTS:
- W32-S1: Final version bump (0.2.0) across all Cargo.toml
- W32-S2: CHANGELOG.md update
- W32-S3: README.md update (multi-ISA architecture, 8 backends)
- W32-S4: cargo clean && cargo build --workspace --release
- W32-S5: cargo test --workspace --release
- W32-S6: cargo clippy --workspace -- -D warnings
- W32-S7: cargo fmt -- --check
- W32-S8: cargo doc --workspace --no-deps
- W32-S9: Verify all 8 ISA backends produce valid output
- W32-S10: Performance report (compile time, execution time per ISA)
- W32-S11: git tag v0.2.0
- W32-S12: git push origin main --tags

VERIFICATION (ALL must pass with ZERO errors):
```bash
export PATH="/home/z/.cargo/bin:$PATH"
cd /home/z/my-project/vuma
cargo clean
cargo build --workspace --release
cargo test --workspace --release
cargo clippy --workspace -- -D warnings
cargo fmt -- --check
cargo doc --workspace --no-deps
```

HARSH RULES:
- If ANY verification fails, the release is BLOCKED. NO EXCEPTIONS.
- If there are TODOs in non-test code, the release is BLOCKED.
- If any backend is broken, the release is BLOCKED.
```

### Success Criteria:
- VUMA v0.2.0 released with 8 ISA backends
- All verification passes
- Tagged and pushed to GitHub

---

## Summary

| Metric | Value |
|--------|-------|
| Total waves | 32 |
| Total ISA backends | 8 (AArch64, RISC-V64, Wasm32, LoongArch64, x86_64, ARM32, MIPS64, PowerPC64) |
| Max parallel waves | 5 |
| Max subagents per wave | 32 |
| Total subagent tasks | ~280 |
| Critical path | 13 waves |
| Time slots with parallelism | 14 |

### ISA Implementation Complexity vs. Value

| ISA | Complexity | Market | Execution | Priority |
|-----|-----------|--------|-----------|----------|
| AArch64 | 4/10 ✅ | Mobile, Pi 5, Apple Silicon, Graviton | QEMU | Done |
| RISC-V64 | 3/10 | Fastest growing, embedded, China | QEMU | Wave 6 |
| Wasm32 | 2/10 | Browsers, edge, serverless | wasmtime | Wave 7 |
| LoongArch64 | 3/10 | China domestic | QEMU | Wave 8 |
| x86_64 | 8/10 | Desktop, server, MY SANDBOX | Native | Wave 9 |
| ARM32 | 6/10 | Embedded, legacy | QEMU | Wave 16 |
| MIPS64 | 5/10 | Routers, embedded | QEMU | Wave 17 |
| PowerPC64 | 7/10 | IBM POWER servers | QEMU | Wave 18 |

### Adding a 9th ISA — The Process

After this plan, adding a new ISA should take 3-5 waves:

1. Define TargetDesc (register file, calling convention, instruction categories)
2. Implement TargetInfo trait
3. Implement instruction encoding/decoding
4. Implement Backend trait (regalloc, encode_function)
5. Add ELF/binary emission + QEMU tests

The TargetDesc system (Wave 5) provides the scaffolding. Each new ISA follows the same pattern proven by all 8 backends.
